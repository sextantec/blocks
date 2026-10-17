//! Fakes em memoria e helpers dos testes caixa-preta do grantd (spec `docs/grantd-spec-v0.md`, secao 7A).
//! Usa so a API publica: `blocklog` (Storage, LogFile, Clock, Log, Options) e `grantd` (Config, Grants,
//! Service, token_sha256_hex). Nao le `crates/grantd/src`. Os fakes guardam o "disco" em Arc<Mutex<..>>:
//! duas aberturas do mesmo `Disco` simulam reabrir o mesmo diretorio de estado.
#![allow(dead_code, unused_imports)]

use blocklog::{Clock, Commit, Log, LogFile, OpenError, Options, Storage};
use chrono::{DateTime, SecondsFormat, TimeDelta, Utc};
use grantd::{Config, Grants, Service, token_sha256_hex};
use serde_json::{Value, json};
use std::io;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};

// ---------------------------------------------------------------- tempo

/// Instante zero dos testes (relogio do fake comeca aqui).
pub const BASE: &str = "2026-10-16T22:00:00.000Z";

/// Instante `s` segundos depois de BASE (`s` pode ser negativo).
pub fn no(s: i64) -> DateTime<Utc> {
    DateTime::parse_from_rfc3339(BASE)
        .unwrap()
        .with_timezone(&Utc)
        + TimeDelta::seconds(s)
}

/// O mesmo instante em RFC 3339 com `Z` e sem fracao: formato dos campos `until`/`not_before` do cliente.
pub fn em(s: i64) -> String {
    no(s).to_rfc3339_opts(SecondsFormat::Secs, true)
}

/// Relogio controlavel, compartilhado (Clone) entre o `Log` e o `Service`.
#[derive(Clone)]
pub struct Relogio(Arc<Mutex<DateTime<Utc>>>);

impl Relogio {
    pub fn novo() -> Relogio {
        Relogio(Arc::new(Mutex::new(no(0))))
    }

    /// Ajusta o relogio para BASE + `s` segundos. Pode ir para tras.
    pub fn vai(&self, s: i64) {
        *self.0.lock().unwrap() = no(s);
    }
}

impl Clock for Relogio {
    fn now(&self) -> DateTime<Utc> {
        *self.0.lock().unwrap()
    }
}

// ---------------------------------------------------------------- disco falso

/// Falha injetada na proxima escrita (uma vez). EIO e ENOSPC: nada e gravado.
#[derive(Clone, Copy, Debug)]
pub enum Falha {
    Eio,
    Enospc,
}

#[derive(Default)]
struct Estado {
    bytes: Vec<u8>,
    trancado: bool,
    falha_escrita: Option<Falha>,
    falha_sync: Option<Falha>,
}

/// O "disco": conteudo do log.jsonl, estado do flock e falhas armadas.
#[derive(Clone, Default)]
pub struct Disco(Arc<Mutex<Estado>>);

impl Disco {
    pub fn novo() -> Disco {
        Disco::default()
    }

    fn st(&self) -> MutexGuard<'_, Estado> {
        self.0.lock().unwrap()
    }

    /// Substitui o conteudo do arquivo (preparo do cenario, sem falhas).
    pub fn semear(&self, bytes: &[u8]) {
        self.st().bytes = bytes.to_vec();
    }

    pub fn bytes(&self) -> Vec<u8> {
        self.st().bytes.clone()
    }

    /// Disco ja preenchido com `bytes` (cenario de cauda cortada ou de log montado a mao).
    pub fn com(bytes: &[u8]) -> Disco {
        let d = Disco::novo();
        d.semear(bytes);
        d
    }

    pub fn texto(&self) -> String {
        String::from_utf8(self.bytes()).unwrap()
    }

    /// Falha aplicada na proxima `write_all` (uma vez).
    pub fn arma_escrita(&self, f: Falha) {
        self.st().falha_escrita = Some(f);
    }

    /// Falha aplicada no proximo `sync_data` (uma vez; o buffer ja foi gravado).
    pub fn arma_sync(&self, f: Falha) {
        self.st().falha_sync = Some(f);
    }
}

fn erro_io(f: Falha) -> io::Error {
    match f {
        Falha::Eio => io::Error::from_raw_os_error(5),
        Falha::Enospc => io::Error::from_raw_os_error(28),
    }
}

/// Trava exclusiva simulada: solta quando o LogFile que a guarda morre.
struct Trava(Disco);

impl Drop for Trava {
    fn drop(&mut self) {
        self.0.st().trancado = false;
    }
}

pub struct ArmazenamentoFalso {
    disco: Disco,
    trava: Option<Trava>,
}

impl ArmazenamentoFalso {
    pub fn novo(disco: &Disco) -> ArmazenamentoFalso {
        ArmazenamentoFalso {
            disco: disco.clone(),
            trava: None,
        }
    }
}

impl Storage for ArmazenamentoFalso {
    fn lock(&mut self) -> Result<(), OpenError> {
        {
            let mut e = self.disco.st();
            if e.trancado {
                return Err(OpenError::AlreadyRunning);
            }
            e.trancado = true;
        }
        self.trava = Some(Trava(self.disco.clone()));
        Ok(())
    }

    fn open_log(&mut self) -> Result<Box<dyn LogFile>, OpenError> {
        Ok(Box::new(ArquivoFalso {
            disco: self.disco.clone(),
            _trava: self.trava.take(),
        }))
    }
}

pub struct ArquivoFalso {
    disco: Disco,
    _trava: Option<Trava>,
}

impl LogFile for ArquivoFalso {
    fn read_all(&mut self) -> io::Result<Vec<u8>> {
        Ok(self.disco.st().bytes.clone())
    }

    fn write_all(&mut self, buf: &[u8]) -> io::Result<()> {
        let mut e = self.disco.st();
        match e.falha_escrita.take() {
            None => {
                e.bytes.extend_from_slice(buf);
                Ok(())
            }
            Some(f) => Err(erro_io(f)),
        }
    }

    fn sync_data(&mut self) -> io::Result<()> {
        let mut e = self.disco.st();
        match e.falha_sync.take() {
            None => Ok(()),
            Some(f) => Err(erro_io(f)),
        }
    }

    fn sync_all(&mut self) -> io::Result<()> {
        Ok(())
    }

    fn set_len(&mut self, len: u64) -> io::Result<()> {
        let mut e = self.disco.st();
        if len > e.bytes.len() as u64 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "set_len alem do fim",
            ));
        }
        e.bytes.truncate(len as usize);
        Ok(())
    }

    fn len(&mut self) -> io::Result<u64> {
        Ok(self.disco.st().bytes.len() as u64)
    }
}

// ---------------------------------------------------------------- abertura do servico

/// Abre o `Log` sobre o disco falso e monta o `Service` (mesmo relogio nos dois).
pub fn sobe_com(disco: &Disco, relogio: &Relogio, cfg: Config, grants: &str) -> Service {
    let (log, replay) = Log::open(
        Box::new(ArmazenamentoFalso::novo(disco)),
        Box::new(relogio.clone()),
        Options::default(),
    )
    .unwrap_or_else(|e| panic!("Log::open deveria passar: {e:?}"));
    Service::new(
        cfg,
        Grants::from_json(grants).unwrap_or_else(|e| panic!("grants invalido: {e}")),
        log,
        replay,
        Box::new(relogio.clone()),
    )
    .unwrap_or_else(|e| panic!("Service::new deveria passar: {e}"))
}

/// `sobe_com` com `Config` padrao.
pub fn sobe(disco: &Disco, relogio: &Relogio, grants: &str) -> Service {
    sobe_com(disco, relogio, Config::default(), grants)
}

// ---------------------------------------------------------------- tokens e perfis

/// Token de teste: `gt_` + 43 caracteres base64url, determinado pelo nome (preenchido com `x`).
pub fn token_de(agente: &str) -> String {
    let corpo = format!("{agente:x<43}");
    let t = format!("gt_{corpo}");
    assert_eq!(t.len(), 46);
    t
}

pub const DONO: &str = "dono";
pub const A1: &str = "a1";
pub const A2: &str = "a2";
pub const A3: &str = "a3";

pub fn tok_dono() -> String {
    token_de(DONO)
}

pub struct Perfil {
    pub agente: &'static str,
    pub papel: &'static str,
    pub pode_pedir: bool,
    pub pode_delegar: bool,
    pub expira: &'static str,
    pub token: String,
}

/// Perfis padrao: `dono` (owner); `a1` (pede e delega); `a2` (pede); `a3` (delega, sem pedir).
pub fn perfis_padrao() -> Vec<Perfil> {
    vec![
        Perfil {
            agente: DONO,
            papel: "owner",
            pode_pedir: false,
            pode_delegar: false,
            expira: "2099-12-31",
            token: token_de(DONO),
        },
        Perfil {
            agente: A1,
            papel: "agent",
            pode_pedir: true,
            pode_delegar: true,
            expira: "2099-12-31",
            token: token_de(A1),
        },
        Perfil {
            agente: A2,
            papel: "agent",
            pode_pedir: true,
            pode_delegar: false,
            expira: "2099-12-31",
            token: token_de(A2),
        },
        Perfil {
            agente: A3,
            papel: "agent",
            pode_pedir: false,
            pode_delegar: true,
            expira: "2099-12-31",
            token: token_de(A3),
        },
    ]
}

/// `grants.json` (lista de perfis; so o SHA-256 do token vai ao arquivo).
pub fn grants_de(perfis: &[Perfil]) -> String {
    let lista: Vec<Value> = perfis
        .iter()
        .map(|p| {
            json!({
                "agent": p.agente,
                "token_sha256": token_sha256_hex(&p.token),
                "role": p.papel,
                "can_request": p.pode_pedir,
                "can_delegate": p.pode_delegar,
                "expires": p.expira,
            })
        })
        .collect();
    Value::Array(lista).to_string()
}

pub fn grants_padrao() -> String {
    grants_de(&perfis_padrao())
}

// ---------------------------------------------------------------- envelope e requisicoes

static CONTADOR: AtomicU64 = AtomicU64::new(1);

/// Id de requisicao do envelope (campo `id`, ecoado na resposta).
pub fn novo_id() -> String {
    format!("c{}", CONTADOR.fetch_add(1, Ordering::SeqCst))
}

/// `params.request_id` novo (`[A-Za-z0-9._-]{1,64}`).
pub fn novo_rid() -> String {
    format!("rq-{}", CONTADOR.fetch_add(1, Ordering::SeqCst))
}

/// Envia uma requisicao de uma linha com `token` e devolve a resposta parseada.
/// Confere que a resposta e uma linha so, com `v` = 1 e o `id` ecoado.
pub fn pede(svc: &mut Service, token: &str, metodo: &str, params: Value) -> Value {
    let id = novo_id();
    let linha =
        json!({"v": 1, "id": id, "token": token, "method": metodo, "params": params}).to_string();
    let resposta = svc.handle_line(&linha);
    assert!(
        !resposta.contains('\n'),
        "resposta com quebra de linha: {resposta}"
    );
    let v: Value = serde_json::from_str(&resposta)
        .unwrap_or_else(|e| panic!("resposta nao e JSON ({e}): {resposta}"));
    assert_eq!(v["v"], json!(1), "versao do envelope: {resposta}");
    assert_eq!(v["id"], json!(id), "id ecoado: {resposta}");
    v
}

/// Pede com o token do dono.
pub fn pede_dono(svc: &mut Service, metodo: &str, params: Value) -> Value {
    pede(svc, &tok_dono(), metodo, params)
}

/// Pede com o token de um agente (pelo nome do perfil).
pub fn pede_como(svc: &mut Service, agente: &str, metodo: &str, params: Value) -> Value {
    pede(svc, &token_de(agente), metodo, params)
}

/// Resposta de sucesso: confere `ok` true e devolve `result`.
pub fn resultado(v: &Value) -> &Value {
    assert_eq!(v["ok"], json!(true), "esperava sucesso: {v}");
    &v["result"]
}

/// Resposta de erro: confere `ok` false e devolve `error.code`.
pub fn codigo(v: &Value) -> &str {
    assert_eq!(v["ok"], json!(false), "esperava erro: {v}");
    v["error"]["code"]
        .as_str()
        .unwrap_or_else(|| panic!("erro sem codigo: {v}"))
}

/// Resposta de `check` (ou `use`) com decisao e motivo exatos.
pub fn assert_decisao(v: &Value, decisao: &str, motivo: &str) {
    let r = resultado(v);
    assert_eq!(r["decision"], json!(decisao), "decision: {v}");
    assert_eq!(r["reason"], json!(motivo), "reason: {v}");
}

/// Id da concessao criada, lido do `result` de `grant`/`deny_rule`/`delegate`/`approve_request`.
pub fn id_de(v: &Value) -> String {
    resultado(v)["id"]
        .as_str()
        .unwrap_or_else(|| panic!("resultado sem id: {v}"))
        .to_string()
}

/// Campos comuns de uma concessao `allow`: fonte e (opcional) validade.
pub fn fonte() -> Value {
    json!({"kind": "msg", "ref": "Siga em frente", "at": "2026-10-16"})
}

/// Corpo basico de `grant` (allow) para `grantee`, `acao`, `recurso`, sem validade.
pub fn allow(grantee: &str, acao: &str, recurso: &str) -> Value {
    json!({"effect": "allow", "grantee": grantee, "action": acao, "resource": recurso, "source": fonte()})
}

/// Mescla os campos de `extra` em `base` (objetos JSON).
pub fn com(mut base: Value, extra: Value) -> Value {
    if let (Value::Object(a), Value::Object(b)) = (&mut base, extra) {
        for (k, v) in b {
            a.insert(k, v);
        }
    }
    base
}

/// Cria uma concessao `allow` ate `ate` (segundos do relogio base) pelo dono, com `request_id`.
/// Devolve o id `G-...`.
pub fn concede(svc: &mut Service, grantee: &str, acao: &str, recurso: &str, ate: i64) -> String {
    let p = com(
        allow(grantee, acao, recurso),
        json!({"until": em(ate), "request_id": novo_rid()}),
    );
    id_de(&pede_dono(svc, "grant", p))
}

/// Decisao de `check` de `agente` para `acao` sobre `recurso`.
pub fn decide(svc: &mut Service, agente: &str, acao: &str, recurso: &str) -> Value {
    pede_como(
        svc,
        agente,
        "check",
        json!({"agent": agente, "action": acao, "resource": recurso}),
    )
}

/// Cada linha completa do log como JSON (o fake nao tem cauda quando o teste chama isto).
pub fn linhas_do_log(disco: &Disco) -> Vec<Value> {
    disco
        .texto()
        .lines()
        .map(|l| {
            serde_json::from_str(l).unwrap_or_else(|e| panic!("linha do log nao e JSON ({e}): {l}"))
        })
        .collect()
}

/// Os eventos (`tx`) do ultimo commit gravado.
pub fn ultimo_tx(disco: &Disco) -> Vec<Value> {
    linhas_do_log(disco).last().expect("log vazio")["tx"]
        .as_array()
        .expect("tx e lista")
        .clone()
}

/// Id do grant de `Value` de decisao (campo `grant`), como string.
pub fn grant_de(v: &Value) -> Value {
    resultado(v)["grant"].clone()
}

/// Concessao `deny` (regra do dono, `deny_rule`) ate `ate` segundos. Devolve o id.
pub fn nega(svc: &mut Service, grantee: &str, acao: &str, recurso: &str, ate: i64) -> String {
    let p = com(
        allow(grantee, acao, recurso),
        json!({"effect": "deny", "until": em(ate), "request_id": novo_rid()}),
    );
    id_de(&pede_dono(svc, "deny_rule", p))
}

/// `check` com quantidade e unidade usd (so assim a janela de `max_amount` e avaliada).
pub fn decide_qty(svc: &mut Service, agente: &str, acao: &str, recurso: &str, qty: u64) -> Value {
    pede_como(
        svc,
        agente,
        "check",
        json!({"agent": agente, "action": acao, "resource": recurso, "qty": qty, "unit": "usd"}),
    )
}

/// `use` de `agente` (mutacao: leva `request_id`). `extra` mescla campos (qty, unit, ...).
pub fn usa(
    svc: &mut Service,
    agente: &str,
    acao: &str,
    recurso: &str,
    rid: &str,
    extra: Value,
) -> Value {
    let base = json!({"agent": agente, "action": acao, "resource": recurso, "request_id": rid});
    pede_como(svc, agente, "use", com(base, extra))
}

/// `head_seq` do `status` (o dono le; status nao grava).
pub fn head(svc: &mut Service) -> u64 {
    resultado(&pede_dono(svc, "status", json!({})))["head_seq"]
        .as_u64()
        .expect("head_seq inteiro")
}

/// Resposta crua (uma linha) de uma requisicao, sem parsear: para comparar byte a byte.
pub fn bruto(svc: &mut Service, token: &str, id: &str, metodo: &str, params: Value) -> String {
    let linha =
        json!({"v": 1, "id": id, "token": token, "method": metodo, "params": params}).to_string();
    svc.handle_line(&linha)
}

/// Perfis de delegacao: como `perfis_padrao`, mas todos os agentes podem pedir e delegar.
pub fn perfis_delegacao() -> Vec<Perfil> {
    let mut perfis = perfis_padrao();
    for p in perfis.iter_mut().filter(|p| p.papel == "agent") {
        p.pode_pedir = true;
        p.pode_delegar = true;
    }
    perfis
}

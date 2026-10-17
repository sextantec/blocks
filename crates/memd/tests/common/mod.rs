//! Fakes e helpers dos testes caixa-preta do memd (docs/memd-spec-v0.md, secoes 8A e 10).
//!
//! So usa a API publica: `memd::{Config, Grants, Service, token_sha256_hex}` e
//! `blocklog::{Log, Storage, LogFile, Clock, Options, Commit}`. Nao le o codigo do crate.
//!
//! Disco e relogio falsos: `Disco` guarda o `log.jsonl` em memoria (Arc<Mutex>), para que
//! reabrir o mesmo `Disco` simule reiniciar o daemon. `RelogioFalso` e controlavel (avanca e
//! regride). Falhas injetaveis: erro em `write_all`, `sync_data` (EIO, ENOSPC, escrita curta).
#![allow(dead_code, unused_imports)]

use blocklog::{Clock, Commit, Log, LogFile, OpenError, Options, Storage};
use chrono::{DateTime, Utc};
use memd::{Config, Grants, Service};
use serde_json::{Value, json};
use std::io;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};

// ------------------------------------------------------------------ constantes

/// Instante inicial dos testes. Dia "hoje" (UTC) = 2026-10-16.
pub const T0: &str = "2026-10-16T22:00:00.000Z";

/// Tokens de teste: `mt_` + 43 caracteres base64url. Nunca reais.
pub const TOK_A: &str = "mt_Ol5Pk8Ji2Hu6Gy3Ft1Dr9Es4Wa7Qz0Xc-Vb_Nm3Lk6J";
pub const TOK_B: &str = "mt_Ab3Kd9Qz1Lm7Px5Rt2Vn8Wy4Hc6Js0Fg-Ue_TiMo1Xa";
pub const TOK_SPEC: &str = "mt_Pq2Zw8Xe5Rt1Yu7Io4Pa9Sd3Fg6Hj0Kl-Zx_Cv8Bn2M";
pub const TOK_SENTINELA: &str = "mt_SentinelaZx9Cv4Bn1Mq6Wr2Ty7Up3Ia8Sd5Fg0Hj-K";
pub const TOK_ERRADO: &str = "mt_Zz9Yy8Xx7Ww6Vv5Uu4Tt3Ss2Rr1Qq0Pp-Oo_Nn9Mm8L";

/// Configuracao completa (todas as chaves da secao 7) para testes que precisam de valores fixos.
pub fn base_cfg() -> Value {
    json!({
        "proposal_ttl_days": 14,
        "view_budget": 6000,
        "view_overdue_max": 5,
        "max_open_proposals": 20,
        "max_records": 20000,
        "max_connections": 32,
        "secret_patterns": [],
        "line_max": 16384,
        "response_max": 65536
    })
}

/// Configuracao com ajustes sobre `base_cfg`, como texto JSON.
pub fn cfg_json_com(ajustes: &[(&str, Value)]) -> String {
    let mut c = base_cfg();
    for (k, v) in ajustes {
        c[*k] = v.clone();
    }
    c.to_string()
}

/// `Config` com ajustes sobre `base_cfg`. Falha o teste se a configuracao for recusada.
pub fn cfg_com(ajustes: &[(&str, Value)]) -> Config {
    Config::from_json(&cfg_json_com(ajustes)).expect("config de teste deve ser valida")
}

// ------------------------------------------------------------------ relogio e disco

pub fn instante(ts: &str) -> DateTime<Utc> {
    DateTime::parse_from_rfc3339(ts)
        .expect("ts de teste")
        .with_timezone(&Utc)
}

/// Relogio controlavel; `poe` ajusta o instante (inclusive para tras).
#[derive(Clone)]
pub struct RelogioFalso(Arc<Mutex<DateTime<Utc>>>);

impl RelogioFalso {
    pub fn em(ts: &str) -> RelogioFalso {
        RelogioFalso(Arc::new(Mutex::new(instante(ts))))
    }

    pub fn poe(&self, ts: &str) {
        *self.0.lock().unwrap() = instante(ts);
    }
}

impl Clock for RelogioFalso {
    fn now(&self) -> DateTime<Utc> {
        *self.0.lock().unwrap()
    }
}

/// Falha injetada na proxima escrita ou sync (uma vez).
#[derive(Clone, Copy, Debug)]
pub enum Falha {
    /// EIO (raw os error 5).
    Eio,
    /// ENOSPC (raw os error 28).
    Enospc,
    /// Grava so os primeiros n bytes do buffer e devolve erro.
    EscritaCurta(usize),
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

    /// Disco ja com conteudo (simula um log.jsonl existente).
    pub fn com(bytes: &[u8]) -> Disco {
        let d = Disco::novo();
        d.0.lock().unwrap().bytes = bytes.to_vec();
        d
    }

    fn st(&self) -> MutexGuard<'_, Estado> {
        self.0.lock().unwrap()
    }

    /// Conteudo atual do log.jsonl.
    pub fn bytes(&self) -> Vec<u8> {
        self.st().bytes.clone()
    }

    /// Falha aplicada na proxima write_all.
    pub fn arma_escrita(&self, f: Falha) {
        self.st().falha_escrita = Some(f);
    }

    /// Falha aplicada no proximo sync_data (o buffer ja foi gravado).
    pub fn arma_sync(&self, f: Falha) {
        self.st().falha_sync = Some(f);
    }
}

fn erro_io(f: Falha) -> io::Error {
    match f {
        Falha::Eio => io::Error::from_raw_os_error(5),
        Falha::Enospc => io::Error::from_raw_os_error(28),
        Falha::EscritaCurta(_) => io::Error::other("escrita curta"),
    }
}

/// Trava exclusiva simulada; solta quando o LogFile e descartado.
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
            Some(Falha::EscritaCurta(n)) => {
                let k = n.min(buf.len());
                let prefixo = buf[..k].to_vec();
                e.bytes.extend_from_slice(&prefixo);
                Err(erro_io(Falha::EscritaCurta(n)))
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

// ------------------------------------------------------------------ servico

/// Disco + relogio de um cenario. Reabrir = `servico(&m, ...)` de novo, depois de soltar o anterior.
pub struct Mundo {
    pub disco: Disco,
    pub relogio: RelogioFalso,
}

impl Mundo {
    pub fn novo() -> Mundo {
        Mundo {
            disco: Disco::novo(),
            relogio: RelogioFalso::em(T0),
        }
    }

    /// Mesmo disco, relogio em outro instante.
    pub fn com_bytes(bytes: &[u8], relogio: &RelogioFalso) -> Mundo {
        Mundo {
            disco: Disco::com(bytes),
            relogio: relogio.clone(),
        }
    }
}

/// Abre o log sobre o disco falso e constroi o `Service` (replay incluido).
/// Pre-condicao: nenhum `Service` anterior sobre o mesmo disco pode estar vivo.
pub fn servico_com(m: &Mundo, cfg: Config, grants_json: &str) -> Service {
    let (log, replay) = Log::open(
        Box::new(ArmazenamentoFalso::novo(&m.disco)),
        Box::new(m.relogio.clone()),
        Options::default(),
    )
    .expect("Log::open deveria passar");
    let grants = Grants::from_json(grants_json).expect("grants de teste devem ser validos");
    Service::new(cfg, grants, log, replay, Box::new(m.relogio.clone()))
        .expect("Service::new deveria passar")
}

/// `Service` com `Config::default()`.
pub fn servico(m: &Mundo, grants_json: &str) -> Service {
    servico_com(m, Config::default(), grants_json)
}

/// `Service` com a configuracao dada.
pub fn servico_cfg(m: &Mundo, cfg: Config, grants_json: &str) -> Service {
    servico_com(m, cfg, grants_json)
}

// ------------------------------------------------------------------ perfis

/// Perfil em `grants.json` (secao 5). `expira` no formato YYYY-MM-DD.
pub fn perfil(
    agente: &str,
    token: &str,
    ops: &[&str],
    leitura: &[&str],
    escrita: &[&str],
    tipos: &[&str],
    expira: &str,
    intent: &str,
) -> Value {
    json!({
        "agent": agente,
        "token_sha256": memd::token_sha256_hex(token),
        "ops": ops,
        "read_scopes": leitura,
        "write_scopes": escrita,
        "types": tipos,
        "expires": expira,
        "intent": intent
    })
}

/// Perfil com tudo liberado (propose, promote, leitura e escrita em `*`, ambos os tipos).
pub fn perfil_total(agente: &str, token: &str) -> Value {
    perfil(
        agente,
        token,
        &["propose", "promote"],
        &["*"],
        &["*"],
        &["decision", "pending"],
        "2099-12-31",
        "teste",
    )
}

/// `grants.json` com a lista de perfis (lista no topo, secao 5).
pub fn grants(perfis: &[Value]) -> String {
    Value::Array(perfis.to_vec()).to_string()
}

// ------------------------------------------------------------------ requisicoes

static CONTADOR: AtomicU64 = AtomicU64::new(1);

/// `request_id` unico no processo de teste.
pub fn rid() -> String {
    format!("rq-{}", CONTADOR.fetch_add(1, Ordering::SeqCst))
}

/// Linha de requisicao (secao 6) com id de cliente `cid`.
pub fn linha_req(cid: &str, token: &str, metodo: &str, params: &Value) -> String {
    json!({"v":1,"id":cid,"token":token,"method":metodo,"params":params}).to_string()
}

/// Parseia uma resposta; uma linha JSON, sem `\n`.
pub fn parse(raw: &str) -> Value {
    assert!(!raw.contains('\n'), "resposta com quebra de linha: {raw}");
    serde_json::from_str(raw).unwrap_or_else(|e| panic!("resposta nao e JSON ({e}): {raw}"))
}

/// Confere o envelope (v:1 e id do cliente) e devolve a resposta parseada.
pub fn chama(s: &mut Service, token: &str, metodo: &str, params: &Value) -> Value {
    let v = parse(&s.handle_line(&linha_req("c", token, metodo, params)));
    assert_eq!(v["v"], json!(1), "envelope: v");
    assert_eq!(v["id"], json!("c"), "envelope: id");
    v
}

/// Chamada com `request_id` novo (obrigatorio em mutacoes).
pub fn muta(s: &mut Service, token: &str, metodo: &str, params: Value) -> Value {
    let mut p = params;
    p["request_id"] = json!(rid());
    chama(s, token, metodo, &p)
}

/// Resposta de sucesso: confere `ok:true` e devolve o `result`.
pub fn res(v: &Value) -> Value {
    assert_eq!(v["ok"], json!(true), "esperava ok:true, veio {v}");
    v["result"].clone()
}

/// Resposta de erro: confere `ok:false` e devolve o `error.code`.
pub fn cod(v: &Value) -> String {
    assert_eq!(v["ok"], json!(false), "esperava ok:false, veio {v}");
    v["error"]["code"]
        .as_str()
        .unwrap_or_else(|| panic!("erro sem code: {v}"))
        .to_string()
}

/// Proposta; devolve o `result` (id, rev, status, seq).
pub fn propoe(s: &mut Service, token: &str, params: Value) -> Value {
    res(&muta(s, token, "propose", params))
}

/// Promocao de `id`/`rev` com `expected_seq`; devolve o `result`.
pub fn promove(s: &mut Service, token: &str, id: &str, rev: u64, expected_seq: u64) -> Value {
    res(&muta(
        s,
        token,
        "promote",
        json!({"id": id, "rev": rev, "expected_seq": expected_seq}),
    ))
}

/// Proposta de D-nnn/P-nnn e promocao imediata; devolve o `result` da promocao.
pub fn propoe_e_promove(s: &mut Service, token: &str, params: Value) -> Value {
    let p = propoe(s, token, params);
    let id = p["id"].as_str().unwrap().to_string();
    let rev = p["rev"].as_u64().unwrap();
    let seq = p["seq"].as_u64().unwrap();
    res(&muta(
        s,
        token,
        "promote",
        json!({"id": id, "rev": rev, "expected_seq": seq}),
    ))
}

/// `get` sem `rev` (ou com `rev`): devolve a resposta completa.
pub fn get(s: &mut Service, token: &str, id: &str) -> Value {
    chama(s, token, "get", &json!({"id": id}))
}

/// `get` de uma versao especifica: devolve o `result`.
pub fn get_rev(s: &mut Service, token: &str, id: &str, rev: u64) -> Value {
    res(&chama(s, token, "get", &json!({"id": id, "rev": rev})))
}

/// `query` com parametros; devolve o `result`.
pub fn consulta(s: &mut Service, token: &str, params: Value) -> Value {
    res(&chama(s, token, "query", &params))
}

/// Ids (na ordem devolvida) dos itens de uma `query`.
pub fn ids_da_consulta(r: &Value) -> Vec<String> {
    r["items"]
        .as_array()
        .expect("items deve ser lista")
        .iter()
        .map(|i| i["id"].as_str().unwrap().to_string())
        .collect()
}

// ------------------------------------------------------------------ registros de teste

pub fn fonte() -> Value {
    json!({"kind": "doc", "ref": "docs/memd-spec-v0.md", "at": "2026-10-16"})
}

/// Decisao valida (propose sem `id`).
pub fn decisao(titulo: &str, escopo: &str) -> Value {
    json!({
        "type": "decision",
        "title": titulo,
        "body": "corpo de teste",
        "scope": escopo,
        "tags": ["teste"],
        "fields": {"criterion": "criterio de teste"},
        "source": fonte()
    })
}

/// Pendencia valida; `review_by` no formato YYYY-MM-DD, hoje ou futuro na data do teste.
pub fn pendencia(titulo: &str, escopo: &str, review_by: &str) -> Value {
    json!({
        "type": "pending",
        "title": titulo,
        "body": "",
        "scope": escopo,
        "tags": [],
        "fields": {"owner": "agente-a", "next_action": "acao de teste", "state": "open"},
        "review_by": review_by,
        "source": fonte()
    })
}

/// Copia de um JSON de params com um campo sobrescrito.
pub fn com(params: &Value, campo: &str, valor: Value) -> Value {
    let mut p = params.clone();
    p[campo] = valor;
    p
}

// ------------------------------------------------------------------ bytes do log

/// Linhas nao vazias de um log.jsonl (sem o `\n`).
pub fn linhas_do(bytes: &[u8]) -> Vec<Vec<u8>> {
    bytes
        .split(|b| *b == b'\n')
        .filter(|l| !l.is_empty())
        .map(|l| l.to_vec())
        .collect()
}

/// Monta um log.jsonl (cada linha seguida de `\n`).
pub fn monta(linhas: &[Vec<u8>]) -> Vec<u8> {
    let mut out = Vec::new();
    for l in linhas {
        out.extend_from_slice(l);
        out.push(b'\n');
    }
    out
}

/// Corta a ultima linha no meio, sem `\n` (queda durante a escrita da ultima linha).
pub fn corta_ultima_no_meio(bytes: &[u8]) -> Vec<u8> {
    let mut ls = linhas_do(bytes);
    let ultima = ls.pop().expect("log nao vazio");
    let mut out = monta(&ls);
    out.extend_from_slice(&ultima[..ultima.len() / 2]);
    out
}

/// Ultima linha inteira, mas sem o `\n` final (JSON valido sem terminador).
pub fn tira_newline_final(bytes: &[u8]) -> Vec<u8> {
    assert_eq!(bytes.last(), Some(&b'\n'), "log deve terminar em \\n");
    bytes[..bytes.len() - 1].to_vec()
}

// ------------------------------------------------------------------ visao e textos

/// Todas as strings de um JSON, recursivamente.
pub fn textos(v: &Value) -> Vec<String> {
    match v {
        Value::String(s) => vec![s.clone()],
        Value::Array(a) => a.iter().flat_map(textos).collect(),
        Value::Object(o) => o.values().flat_map(textos).collect(),
        _ => vec![],
    }
}

/// Texto da visao: `result.text` (formato normativo da view, spec secao 6).
pub fn texto_da_visao(r: &Value) -> String {
    r["text"]
        .as_str()
        .expect("view.text deve ser string")
        .to_string()
}

/// Linhas nao vazias do `text` da view, aparadas. `next_after` e `head_seq` ficam fora do text.
pub fn linhas_visao(r: &Value) -> Vec<String> {
    texto_da_visao(r)
        .split('\n')
        .map(|l| l.trim().to_string())
        .filter(|l| !l.is_empty())
        .collect()
}

/// Secoes em que `id` aparece na view: "due" e/ou "active" (o cabecalho `intent:` nao conta).
/// Cada linha de item e `id | title | scope | review_by`.
pub fn secoes_do_id(r: &Value, id: &str) -> Vec<String> {
    let prefixo = format!("{id} | ");
    let mut secao = String::new();
    let mut achou = Vec::new();
    for l in linhas_visao(r) {
        if l.starts_with("due:") {
            secao = "due".to_string();
        } else if l.starts_with("active:") {
            secao = "active".to_string();
        } else if l.starts_with("intent:") {
            secao = String::new();
        } else if l.starts_with(&prefixo) && !secao.is_empty() {
            achou.push(secao.clone());
        }
    }
    achou
}

/// Quantas linhas da visao comecam com `id | ` (uma por item na secao due/active).
pub fn conta_linhas_do_id(linhas: &[String], id: &str) -> usize {
    let prefixo = format!("{id} | ");
    linhas.iter().filter(|l| l.starts_with(&prefixo)).count()
}

/// Conta de caracteres Unicode do `text` da view (o que view_budget limita).
pub fn chars_do_texto(r: &Value) -> usize {
    texto_da_visao(r).chars().count()
}

/// Repeticao de um texto para montar corpos de tamanho exato.
pub fn repete(c: char, n: usize) -> String {
    std::iter::repeat_n(c, n).collect()
}

/// `Service` com opcoes explicitas do `blocklog` (ex.: `log_max_bytes`).
pub fn servico_com_opcoes(m: &Mundo, cfg: Config, grants_json: &str, opts: Options) -> Service {
    let (log, replay) = Log::open(
        Box::new(ArmazenamentoFalso::novo(&m.disco)),
        Box::new(m.relogio.clone()),
        opts,
    )
    .expect("Log::open deveria passar");
    let grants = Grants::from_json(grants_json).expect("grants de teste devem ser validos");
    Service::new(cfg, grants, log, replay, Box::new(m.relogio.clone()))
        .expect("Service::new deveria passar")
}

//! v1.8, Parte C (docs/spec-v1.8-stale-e-anexos.md, "Parte C", rev2): recibo `bloqueado`, `due` e `obligations`.
//! Escrito so a partir do contrato e dos modelos tests/common, tests/v17_stale.rs e tests/v18_stale_resultado.rs;
//! src/ nao foi lido. Caixa preta: so o binario, o socket e os arquivos. Cada teste tem diretorio temporario
//! proprio; HOME, XDG_CONFIG_HOME e XDG_STATE_HOME apontam para dentro dele; CHATD_*, HERDR_* e TMUX* sao
//! removidas do filho. Credenciais falsas (tokens novos no temporario). Nenhum daemon real, nenhuma rede,
//! nenhum acesso a ~/.local nem a ~/.config.
//! Devem falhar ate a Parte C existir (erro de uso, saida 2, ou resposta diferente do contrato); nunca por
//! erro de compilacao.
//! AMBIGUO: marcado onde o contrato nao fixa nome de campo, formato de saida, tipo de alerta ou codigo de erro.
//! Nao cobertos por falta de contrato ou de meio de teste (ver relatorio): C6 (salto de relogio), C9 (quem e o
//! orquestrador), C10 (events.resume), C12 (formato do stamp antigo e janela de duplicata), C13 (daemon antigo).
#![allow(dead_code)]
mod common;

use chrono::{SecondsFormat, TimeDelta, Utc};
use common::*;
use serde_json::{Value, json};
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::thread;
use std::time::{Duration, Instant};

const A: &str = "claude-main";
const B: &str = "codex-web";
const C: &str = "terceiro";
/// Vocabulario de `await` (Modelo): exatamente estas seis classes.
const CLASSES: [&str; 6] = [
    "permission",
    "user_input",
    "network",
    "identity",
    "policy",
    "unknown",
];
/// Marcas sentinela (C5, C14): nunca podem aparecer em obligations, alertas, --json de lista nem stamp.
const MARCA_ASSUNTO: &str = "ASSUNTO-SECRETO-V18C";
const MARCA_CORPO: &str = "CORPO-SECRETO-V18C";
const MARCA_NOTA: &str = "NOTA-SECRETA-V18C";
const PREFIXOS_LIMPOS: &[&str] = &["CHATD", "HERDR", "TMUX"];

static CONTADOR: AtomicUsize = AtomicUsize::new(0);

// ---------- infraestrutura ----------

/// Diretorio temporario com `home/`, `xdg/` e `xdg-state/`.
struct Amb {
    d: tempfile::TempDir,
}

impl Amb {
    fn novo() -> Amb {
        let d = tmp();
        fs::create_dir_all(d.path().join("home")).unwrap();
        fs::create_dir_all(d.path().join("xdg")).unwrap();
        fs::create_dir_all(d.path().join("xdg-state")).unwrap();
        Amb { d }
    }
    fn p(&self) -> &Path {
        self.d.path()
    }
    fn home(&self) -> PathBuf {
        self.p().join("home")
    }
    fn xdg(&self) -> PathBuf {
        self.p().join("xdg")
    }
    fn xdg_state(&self) -> PathBuf {
        self.p().join("xdg-state")
    }
    /// Estado de teste (dentro do temporario, nunca o estado real).
    fn state(&self) -> PathBuf {
        self.p().join("state")
    }
    fn socket(&self) -> PathBuf {
        self.state().join("chatd.sock")
    }
    /// Arquivo de stamp de teste (dentro do temporario).
    fn stamp(&self) -> PathBuf {
        self.p().join("stamp.json")
    }
    fn token(&self, nome: &str) -> PathBuf {
        self.p().join(format!("{nome}.tok"))
    }
}

/// Instante `min` minutos antes de agora, em RFC 3339 UTC com milissegundos (formato do log).
fn ha_min(min: i64) -> String {
    (Utc::now() - TimeDelta::seconds(min * 60)).to_rfc3339_opts(SecondsFormat::Millis, true)
}

/// Variaveis do ambiente de qualquer processo do teste: CHATD_*, HERDR_* e TMUX* removidas; HOME e XDG no temporario.
fn ambiente(cmd: &mut Command, a: &Amb) {
    for (k, _) in std::env::vars_os() {
        if let Some(ks) = k.to_str() {
            if PREFIXOS_LIMPOS.iter().any(|p| ks.starts_with(p)) {
                cmd.env_remove(&k);
            }
        }
    }
    cmd.env("HOME", a.home())
        .env("XDG_CONFIG_HOME", a.xdg())
        .env("XDG_STATE_HOME", a.xdg_state());
}

/// Roda o chatd ate sair (limite 10 s), com `envs` por cima do ambiente limpo.
fn roda(a: &Amb, args: &[&str], envs: &[(&str, &str)]) -> Saida {
    let n = CONTADOR.fetch_add(1, Ordering::SeqCst);
    let out_p = a.p().join(format!("saida-{n}.out"));
    let err_p = a.p().join(format!("saida-{n}.err"));
    let mut cmd = Command::new(BIN);
    cmd.args(args);
    ambiente(&mut cmd, a);
    cmd.envs(envs.iter().copied());
    let mut child = cmd
        .stdin(Stdio::null())
        .stdout(fs::File::create(&out_p).unwrap())
        .stderr(fs::File::create(&err_p).unwrap())
        .spawn()
        .expect("executar chatd");
    let fim = Instant::now() + Duration::from_secs(10);
    let code = loop {
        if let Some(st) = child.try_wait().expect("try_wait") {
            break st.code();
        }
        if Instant::now() >= fim {
            let _ = child.kill();
            let _ = child.wait();
            break None;
        }
        thread::sleep(Duration::from_millis(10));
    };
    Saida {
        code,
        stdout: fs::read_to_string(&out_p).unwrap_or_default(),
        stderr: fs::read_to_string(&err_p).unwrap_or_default(),
    }
}

/// CLI com a identidade `quem` pelo ambiente (socket, nome e token do temporario).
fn cli_id(a: &Amb, quem: &str, args: &[&str]) -> Saida {
    let sock = s(&a.socket()).to_string();
    let tok = s(&a.token(quem)).to_string();
    roda(
        a,
        args,
        &[
            ("CHATD_SOCKET", sock.as_str()),
            ("CHATD_AS", quem),
            ("CHATD_TOKEN_FILE", tok.as_str()),
        ],
    )
}

/// `send` pela CLI como `de`. `due` vai em --due; `thread` em --thread; `extra` no fim (ex.: --keep-open, --json).
fn envia_cli(
    a: &Amb,
    de: &str,
    para: &str,
    kind: &str,
    id: &str,
    due: Option<&str>,
    thread: Option<&str>,
    extra: &[&str],
) -> Saida {
    let mut v = vec![
        "send",
        "--from",
        de,
        "--to",
        para,
        "--kind",
        kind,
        "--subject",
        "assunto",
        "--body",
        "corpo",
        "--id",
        id,
    ];
    if let Some(d) = due {
        v.push("--due");
        v.push(d);
    }
    if let Some(t) = thread {
        v.push("--thread");
        v.push(t);
    }
    v.extend_from_slice(extra);
    cli_id(a, de, &v)
}

/// Token do temporario para `quem` (credencial falsa criada pelo proprio teste).
fn token_de(a: &Amb, quem: &str) -> String {
    fs::read_to_string(a.token(quem))
        .unwrap()
        .trim()
        .to_string()
}

/// Conexao ja autenticada como `quem`.
fn autenticada(a: &Amb, quem: &str) -> Conexao {
    conexao_auth(&a.socket(), quem, &token_de(a, quem))
}

/// Recibo `ack` pelo socket, autenticado como `by` (a conexao e de `quem`). `classe` vai em "await" quando dado.
/// AMBIGUO: o nome do campo de `await` no JSON nao esta fixado no contrato; usa-se "await".
fn ack_de(a: &Amb, quem: &str, id: &str, stage: &str, classe: Option<&str>) -> Value {
    let mut c = autenticada(a, quem);
    let mut p = json!({"v": 1, "id": id, "by": quem, "stage": stage});
    if let Some(k) = classe {
        p["await"] = json!(k);
    }
    c.chama("message.ack", p)
}

/// Estado da mensagem `id` como `quem` o ve (message.get autenticado).
fn estado_de(a: &Amb, quem: &str, id: &str) -> String {
    let mut c = autenticada(a, quem);
    let r = c.chama("message.get", json!({"v": 1, "id": id}));
    res(&r)["message"]["state"]
        .as_str()
        .unwrap_or_else(|| panic!("sem state em {r}"))
        .to_string()
}

/// Cria a mensagem `id` de A para `para` e a aceita por `para`.
fn criar_aceito(a: &Amb, id: &str, para: &str) {
    let r = envia_cli(a, A, para, "PEDIDO", id, None, None, &[]);
    assert_eq!(r.code, Some(0), "send de {id}: {r:?}");
    let ack = ack_de(a, para, id, "aceito", None);
    assert!(!is_err(&ack), "aceite de {id}: {ack}");
}

/// Campo `k` em qualquer nivel de um objeto JSON (o formato interno do log e das saidas nao e fixado no contrato).
fn acha_chave(v: &Value, k: &str) -> Option<Value> {
    match v {
        Value::Object(m) => {
            if let Some(x) = m.get(k) {
                return Some(x.clone());
            }
            m.values().find_map(|x| acha_chave(x, k))
        }
        Value::Array(a) => a.iter().find_map(|x| acha_chave(x, k)),
        _ => None,
    }
}

/// Objetos de uma saida `--json`: um array, um objeto, ou um objeto por linha.
fn objetos_json(saida: &str) -> Vec<Value> {
    let t = saida.trim();
    if let Ok(v) = serde_json::from_str::<Value>(t) {
        return match v {
            Value::Array(a) => a,
            outro => vec![outro],
        };
    }
    t.lines()
        .filter(|l| !l.trim().is_empty())
        .map(|l| {
            serde_json::from_str::<Value>(l)
                .unwrap_or_else(|e| panic!("linha nao e JSON ({e}): {l:?}"))
        })
        .collect()
}

/// Itens de `obligations --json`. AMBIGUO: o contrato nao fixa o envelope (array, objeto com lista, ou um por linha).
fn itens_obligations(saida: &str) -> Vec<Value> {
    if let Ok(v) = serde_json::from_str::<Value>(saida.trim()) {
        return match v {
            Value::Array(a) => a,
            Value::Object(m) => m
                .values()
                .find_map(|x| {
                    if let Value::Array(a) = x {
                        Some(a.clone())
                    } else {
                        None
                    }
                })
                .unwrap_or_default(),
            _ => vec![],
        };
    }
    objetos_json(saida)
}

/// Item de obligations para `id`, se houver.
fn achado<'a>(itens: &'a [Value], id: &str) -> Option<&'a Value> {
    itens
        .iter()
        .find(|o| acha_chave(o, "id") == Some(json!(id)))
}

/// `chatd obligations --json` (cliente puro sobre o log: so precisa de --state-dir). Falha se a saida nao for 0.
fn obrigacoes(a: &Amb) -> Vec<Value> {
    let st = s(&a.state()).to_string();
    let r = roda(
        a,
        &["obligations", "--json", "--state-dir", st.as_str()],
        &[],
    );
    assert_eq!(r.code, Some(0), "obligations: {r:?}");
    itens_obligations(&r.stdout)
}

/// Eventos do log.jsonl de um tipo (`blocked`, `registered`, `accepted`, `resulted`...).
fn eventos(a: &Amb, tipo: &str) -> Vec<Value> {
    fs::read_to_string(a.state().join("log.jsonl"))
        .unwrap_or_default()
        .lines()
        .filter_map(|l| serde_json::from_str::<Value>(l).ok())
        .filter(|v| v["type"] == tipo)
        .collect()
}

/// Eventos `tipo` ligados ao id `id` (o campo id fica em qualquer nivel do evento).
fn eventos_de(a: &Amb, tipo: &str, id: &str) -> Vec<Value> {
    eventos(a, tipo)
        .into_iter()
        .filter(|e| acha_chave(e, "id") == Some(json!(id)))
        .collect()
}

/// Verdadeiro se alguma marca sentinela aparece no texto.
fn tem_marca(t: &str) -> bool {
    [MARCA_ASSUNTO, MARCA_CORPO, MARCA_NOTA]
        .iter()
        .any(|m| t.contains(m))
}

// ---------- daemon real (sob o temporario) ----------

/// Configuracao do servidor. `auth` false vira `auth: off`. `features` None omite a chave (padrao).
fn cfg_json(features: Option<bool>, auth: bool) -> Value {
    let mut c = json!({
        "herdr_bin": "/nonexistent/chatd-herdr-v18c",
        "bell_poll_ms": 5000,
        "bell_rearm_s": 600,
        "recipients": {
            "claude-main": {"delivery": "watch"},
            "codex-web": {"delivery": "watch"},
            "terceiro": {"delivery": "watch"}
        }
    });
    if !auth {
        c["auth"] = json!("off");
    }
    if let Some(f) = features {
        c["features"] = json!({"blocked_receipts": f});
    }
    c
}

/// Processo `serve` do temporario; encerra no drop.
struct Srv {
    child: Child,
    out: PathBuf,
}

impl Srv {
    /// Sobe `serve --state-dir STATE --config CFG` e espera `chatd: ready` (10 s).
    fn sobe(a: &Amb, cfg: &Path, tag: &str) -> Srv {
        let out = a.p().join(format!("serve-{tag}.out"));
        let err = a.p().join(format!("serve-{tag}.err"));
        let st = s(&a.state()).to_string();
        let mut cmd = Command::new(BIN);
        cmd.args(["serve", "--state-dir", st.as_str(), "--config", s(cfg)]);
        ambiente(&mut cmd, a);
        let child = cmd
            .stdin(Stdio::null())
            .stdout(fs::File::create(&out).unwrap())
            .stderr(fs::File::create(&err).unwrap())
            .spawn()
            .expect("executar serve");
        let srv = Srv { child, out };
        let pronto = espera(Duration::from_secs(10), || {
            fs::read_to_string(&srv.out)
                .unwrap_or_default()
                .lines()
                .any(|l| l == PRONTO)
        });
        assert!(
            pronto,
            "servidor nao ficou pronto; stderr: {:?}",
            fs::read_to_string(&err).unwrap_or_default()
        );
        srv
    }

    /// SIGTERM e espera ate 5 s pela saida.
    fn para(&mut self) -> Option<i32> {
        unsafe { libc::kill(self.child.id() as libc::pid_t, libc::SIGTERM) };
        let fim = Instant::now() + Duration::from_secs(5);
        loop {
            if let Ok(Some(st)) = self.child.try_wait() {
                return st.code();
            }
            if Instant::now() >= fim {
                return None;
            }
            thread::sleep(Duration::from_millis(20));
        }
    }
}

impl Drop for Srv {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// Token falso de `nome` (emitido pelo proprio chatd no temporario, nunca o token real).
fn emite_token(a: &Amb, nome: &str) {
    fs::create_dir_all(a.state()).unwrap();
    let st = s(&a.state()).to_string();
    let out = s(&a.token(nome)).to_string();
    let r = roda(
        a,
        &[
            "token",
            "new",
            nome,
            "--state-dir",
            st.as_str(),
            "--out",
            out.as_str(),
        ],
        &[],
    );
    assert_eq!(r.code, Some(0), "token new {nome}: {r:?}");
}

/// Servidor com a configuracao dada, tokens de A, B e C no temporario.
fn amb_daemon(features: Option<bool>, auth: bool) -> (Amb, Srv) {
    let a = Amb::novo();
    fs::create_dir_all(a.state()).unwrap();
    let cfg = a.p().join("cfg.json");
    fs::write(&cfg, cfg_json(features, auth).to_string()).unwrap();
    let srv = Srv::sobe(&a, &cfg, "principal");
    for n in [A, B, C] {
        emite_token(&a, n);
    }
    (a, srv)
}

/// `watch --as QUEM` (com `extra`) em processo proprio; a saida vai para arquivo. Encerra e devolve a saida ao `fim`.
struct Vigia {
    child: Child,
    out: PathBuf,
}

impl Vigia {
    fn inicia(a: &Amb, quem: &str, extra: &[&str]) -> Vigia {
        let out = a.p().join(format!(
            "vigia-{quem}-{}.out",
            CONTADOR.fetch_add(1, Ordering::SeqCst)
        ));
        let sock = s(&a.socket()).to_string();
        let tok = s(&a.token(quem)).to_string();
        let mut cmd = Command::new(BIN);
        cmd.args(["watch", "--as", quem]).args(extra);
        ambiente(&mut cmd, a);
        cmd.env("CHATD_SOCKET", sock.as_str())
            .env("CHATD_TOKEN_FILE", tok.as_str());
        let child = cmd
            .stdin(Stdio::null())
            .stdout(fs::File::create(&out).unwrap())
            .stderr(Stdio::null())
            .spawn()
            .expect("executar watch");
        thread::sleep(Duration::from_millis(800));
        Vigia { child, out }
    }

    /// Encerra o watch e devolve tudo o que ele escreveu.
    fn fim(mut self) -> String {
        thread::sleep(Duration::from_millis(800));
        let _ = self.child.kill();
        let _ = self.child.wait();
        fs::read_to_string(&self.out).unwrap_or_default()
    }
}

// ---------- log montado pelo teste (sem daemon) ----------

/// Log `state/log.jsonl` como o daemon o escreveria: `v`, `seq` crescente a partir de 1, `ts` e `type`.
/// AMBIGUO: os campos de cada tipo (inclusive `due_s` e os de `blocked`) ficam no nivel de cima da linha.
struct Diario {
    arq: PathBuf,
    seq: u64,
}

impl Diario {
    fn novo(state: &Path) -> Diario {
        fs::create_dir_all(state).unwrap();
        let arq = state.join("log.jsonl");
        fs::write(&arq, "").unwrap();
        Diario { arq, seq: 0 }
    }

    /// Abre um log existente e continua a numeracao.
    fn abre(state: &Path) -> Diario {
        let arq = state.join("log.jsonl");
        let seq = fs::read_to_string(&arq).unwrap_or_default().lines().count() as u64;
        Diario { arq, seq }
    }

    fn evento(&mut self, tipo: &str, ts: &str, campos: Value) -> &mut Self {
        self.seq += 1;
        let mut linha = json!({"v": 1, "seq": self.seq, "ts": ts, "type": tipo});
        if let (Some(base), Value::Object(extra)) = (linha.as_object_mut(), campos) {
            for (k, v) in extra {
                base.insert(k, v);
            }
        }
        let mut f = OpenOptions::new()
            .append(true)
            .create(true)
            .open(&self.arq)
            .unwrap();
        writeln!(f, "{linha}").unwrap();
        self
    }

    /// Registro de mensagem `kind` (PEDIDO, PERGUNTA, AVISO...) com prazo `due_s` (None = sem prazo).
    fn pedido(
        &mut self,
        kind: &str,
        id: &str,
        de: &str,
        para: &str,
        ts: &str,
        due_s: Option<u64>,
    ) -> &mut Self {
        let campos = json!({
            "id": id, "from": de, "to": para, "kind": kind,
            "subject": MARCA_ASSUNTO, "body": MARCA_CORPO, "thread": null,
            "state": "registered", "refs": [], "note": null,
            "registered_at": ts, "accepted_at": null, "resulted_at": null, "result": null,
            "due_s": due_s
        });
        self.evento("registered", ts, campos)
    }

    fn aceite(&mut self, id: &str, ts: &str) -> &mut Self {
        self.evento("accepted", ts, json!({"id": id, "note": null}))
    }

    fn resultado(&mut self, id: &str, ts: &str, result: &str) -> &mut Self {
        self.evento(
            "resulted",
            ts,
            json!({"id": id, "note": null, "result": result}),
        )
    }

    fn bloqueado(&mut self, id: &str, ts: &str, classe: &str) -> &mut Self {
        self.evento(
            "blocked",
            ts,
            json!({"id": id, "await": classe, "since": ts}),
        )
    }
}

/// `chatd stale --state-dir STATE` mais os argumentos extras (o stale so le o log, nao conecta ao socket).
fn stale(a: &Amb, extra: &[&str]) -> Saida {
    let st = s(&a.state()).to_string();
    let mut v = vec!["stale", "--state-dir", st.as_str()];
    v.extend_from_slice(extra);
    roda(a, &v, &[])
}

/// Campo que diz o tipo do alerta. AMBIGUO: o contrato chama o tipo de `due_expired`, mas nao fixa o nome do campo.
fn tipo_de(o: &Value) -> Option<String> {
    o.get("kind")
        .or_else(|| o.get("type"))
        .and_then(|v| v.as_str())
        .map(String::from)
}

// =====================================================================
// C1: `bloqueado` mantem `aceito`, grava {await, since}; idempotencia; troca de classe; limpeza; resultado encerra; replay
// =====================================================================

#[test]
fn c01a_bloqueado_mantem_aceito_e_grava_await_e_since() {
    let (a, _srv) = amb_daemon(Some(true), true);
    criar_aceito(&a, "c01a", B);
    let r = ack_de(&a, B, "c01a", "bloqueado", Some("permission"));
    assert!(!is_err(&r), "bloqueado sobre aceito deve ser aceito: {r}");
    assert_eq!(
        estado_de(&a, A, "c01a"),
        "aceito",
        "bloqueado nao muda o estado"
    );
    let itens = obrigacoes(&a);
    let it =
        achado(&itens, "c01a").expect("PEDIDO aceito sem resultado deve aparecer em obligations");
    assert_eq!(
        acha_chave(it, "await"),
        Some(json!("permission")),
        "await: {it}"
    );
    assert!(
        acha_chave(it, "since").is_some_and(|v| !v.is_null()),
        "since deve estar preenchido: {it}"
    );
    assert_eq!(
        eventos_de(&a, "blocked", "c01a").len(),
        1,
        "um evento blocked"
    );
}

#[test]
fn c01b_repetir_a_mesma_classe_e_idempotente_sem_evento() {
    let (a, _srv) = amb_daemon(Some(true), true);
    criar_aceito(&a, "c01b", B);
    assert!(!is_err(&ack_de(
        &a,
        B,
        "c01b",
        "bloqueado",
        Some("network")
    )));
    let segunda = ack_de(&a, B, "c01b", "bloqueado", Some("network"));
    assert!(
        !is_err(&segunda),
        "a repeticao idempotente nao e erro: {segunda}"
    );
    assert_eq!(
        eventos_de(&a, "blocked", "c01b").len(),
        1,
        "repetir o mesmo await nao grava evento"
    );
}

#[test]
fn c01c_outra_classe_substitui_a_anterior() {
    let (a, _srv) = amb_daemon(Some(true), true);
    criar_aceito(&a, "c01c", B);
    assert!(!is_err(&ack_de(
        &a,
        B,
        "c01c",
        "bloqueado",
        Some("permission")
    )));
    assert!(!is_err(&ack_de(
        &a,
        B,
        "c01c",
        "bloqueado",
        Some("user_input")
    )));
    assert_eq!(
        eventos_de(&a, "blocked", "c01c").len(),
        2,
        "outra classe grava novo evento"
    );
    let itens = obrigacoes(&a);
    let it = achado(&itens, "c01c").expect("ainda aceito sem resultado");
    assert_eq!(
        acha_chave(it, "await"),
        Some(json!("user_input")),
        "a classe nova substitui: {it}"
    );
}

#[test]
fn c01d_await_none_limpa_a_anotacao() {
    let (a, _srv) = amb_daemon(Some(true), true);
    criar_aceito(&a, "c01d", B);
    assert!(!is_err(&ack_de(&a, B, "c01d", "bloqueado", Some("policy"))));
    let limpa = ack_de(&a, B, "c01d", "bloqueado", Some("none"));
    assert!(!is_err(&limpa), "--await none limpa: {limpa}");
    let itens = obrigacoes(&a);
    let it = achado(&itens, "c01d").expect("continua aceito sem resultado");
    assert!(
        acha_chave(it, "await").map_or(true, |v| v.is_null()),
        "await deve sumir apos none: {it}"
    );
}

#[test]
fn c01e_resultado_limpa_o_bloqueio_e_encerra_a_mensagem() {
    let (a, _srv) = amb_daemon(Some(true), true);
    criar_aceito(&a, "c01e", B);
    assert!(!is_err(&ack_de(
        &a,
        B,
        "c01e",
        "bloqueado",
        Some("identity")
    )));
    let r = ack_de(&a, B, "c01e", "resultado", None);
    assert!(!is_err(&r), "resultado manual: {r}");
    assert_eq!(estado_de(&a, A, "c01e"), "resultado", "resultado encerra");
    let itens = obrigacoes(&a);
    assert!(
        achado(&itens, "c01e").is_none(),
        "resultado tira a mensagem de obligations: {itens:?}"
    );
}

#[test]
fn c01f_replay_do_log_reconstroi_o_mesmo_estado() {
    let (a, mut srv) = amb_daemon(Some(true), true);
    criar_aceito(&a, "c01f", B);
    assert!(!is_err(&ack_de(
        &a,
        B,
        "c01f",
        "bloqueado",
        Some("user_input")
    )));
    let antes = obrigacoes(&a);
    let it_antes = achado(&antes, "c01f").expect("antes do reinicio").clone();
    assert_eq!(srv.para(), Some(0), "SIGTERM deve encerrar limpo");
    let cfg = a.p().join("cfg.json");
    let _srv2 = Srv::sobe(&a, &cfg, "reinicio");
    let depois = obrigacoes(&a);
    let it_depois = achado(&depois, "c01f").expect("depois do reinicio");
    assert_eq!(
        acha_chave(it_depois, "await"),
        acha_chave(&it_antes, "await"),
        "await apos replay"
    );
    assert_eq!(
        acha_chave(it_depois, "since"),
        acha_chave(&it_antes, "since"),
        "since apos replay"
    );
}

// =====================================================================
// C2: `await` fora do vocabulario => saida 2, nada gravado; as seis classes passam
// =====================================================================

#[test]
fn c02a_await_fora_do_vocabulario_sai_2_e_nada_e_gravado() {
    let (a, _srv) = amb_daemon(Some(true), true);
    criar_aceito(&a, "c02a", B);
    let r = cli_id(
        &a,
        B,
        &[
            "ack",
            "--id",
            "c02a",
            "--as",
            B,
            "--stage",
            "bloqueado",
            "--await",
            "banana",
        ],
    );
    assert_eq!(r.code, Some(2), "await fora do vocabulario: {r:?}");
    assert!(eventos_de(&a, "blocked", "c02a").is_empty(), "nada gravado");
    assert_eq!(estado_de(&a, A, "c02a"), "aceito");
}

#[test]
fn c02b_as_seis_classes_do_vocabulario_sao_aceitas() {
    let (a, _srv) = amb_daemon(Some(true), true);
    for (i, classe) in CLASSES.iter().enumerate() {
        let id = format!("c02b-{i}");
        criar_aceito(&a, &id, B);
        let r = ack_de(&a, B, &id, "bloqueado", Some(*classe));
        assert!(!is_err(&r), "classe {classe} deve ser aceita: {r}");
    }
    assert_eq!(
        eventos(&a, "blocked").len(),
        CLASSES.len(),
        "um evento por classe"
    );
}

#[test]
fn c02c_classe_invalida_pelo_socket_cru_e_recusada_sem_evento() {
    let (a, _srv) = amb_daemon(Some(true), true);
    criar_aceito(&a, "c02c", B);
    let r = ack_de(&a, B, "c02c", "bloqueado", Some("texto livre nao e classe"));
    assert!(
        is_err(&r),
        "classe invalida no socket cru deve ser recusada: {r}"
    );
    assert!(eventos_de(&a, "blocked", "c02c").is_empty(), "nada gravado");
}

// =====================================================================
// C3: `refused`/`failed` pedem --note so no cliente; o daemon aceita sem nota; replay v1.7 sobe
// =====================================================================

#[test]
fn c03a_cliente_refused_sem_note_sai_2() {
    let (a, _srv) = amb_daemon(Some(true), true);
    criar_aceito(&a, "c03a", B);
    let r = cli_id(
        &a,
        B,
        &[
            "ack",
            "--id",
            "c03a",
            "--as",
            B,
            "--stage",
            "resultado",
            "--result",
            "refused",
        ],
    );
    assert_eq!(r.code, Some(2), "refused sem --note: {r:?}");
    assert_eq!(estado_de(&a, A, "c03a"), "aceito", "nada gravado");
}

#[test]
fn c03b_cliente_failed_sem_note_sai_2() {
    let (a, _srv) = amb_daemon(Some(true), true);
    criar_aceito(&a, "c03b", B);
    let r = cli_id(
        &a,
        B,
        &[
            "ack",
            "--id",
            "c03b",
            "--as",
            B,
            "--stage",
            "resultado",
            "--result",
            "failed",
        ],
    );
    assert_eq!(r.code, Some(2), "failed sem --note: {r:?}");
    assert_eq!(estado_de(&a, A, "c03b"), "aceito", "nada gravado");
}

#[test]
fn c03c_daemon_aceita_refused_sem_nota_pelo_socket_cru() {
    let (a, _srv) = amb_daemon(Some(true), true);
    criar_aceito(&a, "c03c", B);
    let mut c = autenticada(&a, B);
    let r2 = c.chama(
        "message.ack",
        json!({"v": 1, "id": "c03c", "by": B, "stage": "resultado", "result": "refused"}),
    );
    assert!(!is_err(&r2), "o daemon aceita refused sem nota (cru): {r2}");
    assert_eq!(estado_de(&a, A, "c03c"), "resultado");
}

#[test]
fn c03d_replay_de_log_v17_com_refused_sem_nota_sobe() {
    let a = Amb::novo();
    let mut d = Diario::novo(&a.state());
    d.pedido("PEDIDO", "c03d", A, B, &ha_min(10), None)
        .aceite("c03d", &ha_min(9));
    d.resultado("c03d", &ha_min(8), "refused");
    emite_token(&a, A);
    let cfg = a.p().join("cfg.json");
    fs::write(&cfg, cfg_json(None, true).to_string()).unwrap();
    let _srv = Srv::sobe(&a, &cfg, "replay-v17");
    assert_eq!(
        estado_de(&a, A, "c03d"),
        "resultado",
        "o log v1.7 com refused sem nota continua valido"
    );
}

// =====================================================================
// C4: `send --due`: piso 5 min, teto 24 h; invalidos => saida 2; padroes por tipo; due fora do hash
// =====================================================================

#[test]
fn c04a_due_invalido_sai_2_e_nada_e_registrado() {
    let (a, _srv) = amb_daemon(Some(true), true);
    let invalidos = ["0", "-5m", "20", "1s", "4m", "25h", "99999999999999999999m"];
    for (i, v) in invalidos.iter().enumerate() {
        let id = format!("c04a-{i}");
        let r = envia_cli(&a, A, B, "PEDIDO", &id, Some(v), None, &[]);
        assert_eq!(r.code, Some(2), "--due {v} deve sair 2: {r:?}");
        assert!(
            eventos_de(&a, "registered", &id).is_empty(),
            "--due {v} nao grava nada"
        );
    }
}

#[test]
fn c04b_piso_5m_e_teto_24h_passam() {
    let (a, _srv) = amb_daemon(Some(true), true);
    for (id, v) in [("c04b-min", "5m"), ("c04b-max", "24h")] {
        let r = envia_cli(&a, A, B, "PEDIDO", id, Some(v), None, &[]);
        assert_eq!(r.code, Some(0), "--due {v} deve passar: {r:?}");
    }
}

#[test]
fn c04c_padrao_do_pedido_e_30_min_gravado_como_due_s() {
    let (a, _srv) = amb_daemon(Some(true), true);
    let r = envia_cli(&a, A, B, "PEDIDO", "c04c", None, None, &[]);
    assert_eq!(r.code, Some(0), "{r:?}");
    let reg = eventos_de(&a, "registered", "c04c");
    assert_eq!(reg.len(), 1, "um registered: {reg:?}");
    assert_eq!(
        acha_chave(&reg[0], "due_s"),
        Some(json!(1800)),
        "PEDIDO sem --due: 30 min = 1800 s"
    );
}

#[test]
fn c04d_padrao_da_pergunta_e_15_min_e_aviso_nao_tem_prazo() {
    let (a, _srv) = amb_daemon(Some(true), true);
    assert_eq!(
        envia_cli(&a, A, B, "PERGUNTA", "c04d-q", None, None, &[]).code,
        Some(0)
    );
    assert_eq!(
        envia_cli(&a, A, B, "AVISO", "c04d-a", None, None, &[]).code,
        Some(0)
    );
    let q = eventos_de(&a, "registered", "c04d-q");
    assert_eq!(
        acha_chave(&q[0], "due_s"),
        Some(json!(900)),
        "PERGUNTA sem --due: 15 min = 900 s"
    );
    let av = eventos_de(&a, "registered", "c04d-a");
    assert!(
        acha_chave(&av[0], "due_s").map_or(true, |v| v.is_null()),
        "AVISO nao tem prazo: {:?}",
        av[0]
    );
}

#[test]
fn c04e_due_fica_fora_do_hash_de_idempotencia() {
    let (a, _srv) = amb_daemon(Some(true), true);
    assert_eq!(
        envia_cli(&a, A, B, "PEDIDO", "c04e", Some("5m"), None, &[]).code,
        Some(0)
    );
    let r = envia_cli(&a, A, B, "PEDIDO", "c04e", None, None, &["--json"]);
    assert_eq!(r.code, Some(0), "reenvio: {r:?}");
    let dup = objetos_json(&r.stdout)
        .into_iter()
        .find_map(|o| acha_chave(&o, "duplicate"));
    assert_eq!(
        dup,
        Some(json!(true)),
        "reenvio sem --due e duplicata (due fora do hash): {r:?}"
    );
    assert_eq!(
        eventos_de(&a, "registered", "c04e").len(),
        1,
        "o original e devolvido, sem novo registro"
    );
    assert_eq!(
        acha_chave(&eventos_de(&a, "registered", "c04e")[0], "due_s"),
        Some(json!(300))
    );
}

// =====================================================================
// C5: obligations: so PEDIDO/PERGUNTA aceitos sem resultado; overdue; sem assunto/corpo/nota; scheduler_ok
// =====================================================================

#[test]
fn c05a_so_pedido_e_pergunta_aceitos_sem_resultado() {
    let (a, _srv) = amb_daemon(Some(true), true);
    assert_eq!(
        envia_cli(&a, A, B, "AVISO", "c05-av", None, None, &[]).code,
        Some(0)
    );
    assert!(!is_err(&ack_de(&a, B, "c05-av", "aceito", None)));
    criar_aceito(&a, "c05-p1", B);
    assert_eq!(
        envia_cli(&a, A, B, "PEDIDO", "c05-p2", None, None, &[]).code,
        Some(0)
    ); // registrado, nao aceito
    assert_eq!(
        envia_cli(&a, A, B, "PERGUNTA", "c05-q", None, None, &[]).code,
        Some(0)
    );
    assert!(!is_err(&ack_de(&a, B, "c05-q", "aceito", None)));
    criar_aceito(&a, "c05-r", B);
    assert!(!is_err(&ack_de(&a, B, "c05-r", "resultado", None)));
    let itens = obrigacoes(&a);
    let ids: Vec<String> = itens
        .iter()
        .filter_map(|o| acha_chave(o, "id").and_then(|v| v.as_str().map(String::from)))
        .collect();
    assert!(
        achado(&itens, "c05-p1").is_some(),
        "PEDIDO aceito sem resultado: {ids:?}"
    );
    assert!(
        achado(&itens, "c05-q").is_some(),
        "PERGUNTA aceita sem resultado: {ids:?}"
    );
    for proibido in ["c05-av", "c05-p2", "c05-r"] {
        assert!(
            achado(&itens, proibido).is_none(),
            "{proibido} nao deve aparecer: {ids:?}"
        );
    }
}

#[test]
fn c05b_overdue_reflete_o_prazo_contado_do_registro() {
    let a = Amb::novo();
    let mut d = Diario::novo(&a.state());
    // Prazo de 300 s, registrado ha 10 min: vencido. Prazo de 3600 s, registrado ha 10 min: no prazo.
    d.pedido("PEDIDO", "c05b-v", A, B, &ha_min(10), Some(300))
        .aceite("c05b-v", &ha_min(9));
    d.pedido("PEDIDO", "c05b-n", A, B, &ha_min(10), Some(3600))
        .aceite("c05b-n", &ha_min(9));
    let itens = obrigacoes(&a);
    let v = achado(&itens, "c05b-v").expect("vencido listado");
    let n = achado(&itens, "c05b-n").expect("no prazo listado");
    assert_eq!(acha_chave(v, "overdue"), Some(json!(true)), "{v}");
    assert_eq!(acha_chave(n, "overdue"), Some(json!(false)), "{n}");
}

#[test]
fn c05c_obligations_nao_mostra_assunto_corpo_nem_nota_em_nenhum_formato() {
    let a = Amb::novo();
    let mut d = Diario::novo(&a.state());
    d.pedido("PEDIDO", "c05c", A, B, &ha_min(10), Some(300));
    d.evento(
        "accepted",
        &ha_min(9),
        json!({"id": "c05c", "note": MARCA_NOTA}),
    );
    let st = s(&a.state()).to_string();
    let txt = roda(&a, &["obligations", "--state-dir", st.as_str()], &[]);
    let js = roda(
        &a,
        &["obligations", "--json", "--state-dir", st.as_str()],
        &[],
    );
    for (quem, t) in [("texto", &txt), ("json", &js)] {
        assert!(
            !tem_marca(&t.stdout),
            "{quem} stdout vazou marca: {}",
            t.stdout
        );
        assert!(
            !tem_marca(&t.stderr),
            "{quem} stderr vazou marca: {}",
            t.stderr
        );
    }
}

#[test]
fn c05d_obligations_json_traz_scheduler_ok() {
    let a = Amb::novo();
    let mut d = Diario::novo(&a.state());
    d.pedido("PEDIDO", "c05d", A, B, &ha_min(10), Some(300))
        .aceite("c05d", &ha_min(9));
    let st = s(&a.state()).to_string();
    let js = roda(
        &a,
        &["obligations", "--json", "--state-dir", st.as_str()],
        &[],
    );
    assert_eq!(js.code, Some(0), "{js:?}");
    // AMBIGUO: o contrato nao fixa o lugar do campo nem o valor enquanto nao houve execucao do agendador.
    assert!(
        js.stdout.contains("scheduler_ok"),
        "o --json deve dizer scheduler_ok: {}",
        js.stdout
    );
}

// =====================================================================
// C6: `chatd stale`: due_expired com e sem aceite, precedencia, `bloqueado` nao adia, idade nunca usa since
// =====================================================================

/// Alertas `--json` do stale, como objetos.
fn alertas(a: &Amb, extra: &[&str]) -> Vec<Value> {
    let mut v: Vec<&str> = vec!["--json"];
    v.extend_from_slice(extra);
    let r = stale(a, &v);
    assert!(
        r.code != Some(2) && r.code.is_some(),
        "stale nao pode dar erro de uso nem travar: {r:?}"
    );
    objetos_json(&r.stdout)
}

/// Tipos de alerta que mencionam `id`.
fn tipos_de(alertas: &[Value], id: &str) -> Vec<String> {
    alertas
        .iter()
        .filter(|o| o.to_string().contains(id))
        .filter_map(tipo_de)
        .collect()
}

#[test]
fn c06a_due_expired_com_e_sem_aceite() {
    let a = Amb::novo();
    let mut d = Diario::novo(&a.state());
    d.pedido("PEDIDO", "c06-com", A, B, &ha_min(10), Some(300))
        .aceite("c06-com", &ha_min(9));
    d.pedido("PEDIDO", "c06-sem", A, B, &ha_min(10), Some(300));
    let al = alertas(&a, &[]);
    assert!(
        tipos_de(&al, "c06-com").iter().any(|t| t == "due_expired"),
        "com aceite: {al:?}"
    );
    assert!(
        tipos_de(&al, "c06-sem").iter().any(|t| t == "due_expired"),
        "sem aceite: {al:?}"
    );
}

#[test]
fn c06b_nao_alerta_antes_do_prazo() {
    let a = Amb::novo();
    let mut d = Diario::novo(&a.state());
    d.pedido("PEDIDO", "c06-cedo", A, B, &ha_min(10), Some(1800))
        .aceite("c06-cedo", &ha_min(9));
    let al = alertas(&a, &[]);
    assert!(
        !tipos_de(&al, "c06-cedo").iter().any(|t| t == "due_expired"),
        "antes do prazo: {al:?}"
    );
}

#[test]
fn c06c_due_expired_tem_precedencia_sobre_unaccepted() {
    let a = Amb::novo();
    let mut d = Diario::novo(&a.state());
    // Vencido e nao aceito: so pode sair due_expired para essa mensagem.
    d.pedido("PEDIDO", "c06-prec", A, B, &ha_min(10), Some(300));
    let al = alertas(&a, &[]);
    let tipos = tipos_de(&al, "c06-prec");
    assert!(tipos.iter().any(|t| t == "due_expired"), "{al:?}");
    assert!(
        !tipos.iter().any(|t| t == "unaccepted"),
        "unaccepted nao pode coexistir: {tipos:?}"
    );
}

#[test]
fn c06d_bloqueado_nao_adia_o_prazo() {
    let a = Amb::novo();
    let mut d = Diario::novo(&a.state());
    d.pedido("PEDIDO", "c06-blq", A, B, &ha_min(10), Some(300))
        .aceite("c06-blq", &ha_min(9));
    d.bloqueado("c06-blq", &ha_min(1), "permission");
    let al = alertas(&a, &[]);
    assert!(
        tipos_de(&al, "c06-blq").iter().any(|t| t == "due_expired"),
        "bloqueado recente nao adia prazo vencido: {al:?}"
    );
}

#[test]
fn c06e_idade_do_no_result_conta_do_aceite_e_nao_do_since_do_bloqueio() {
    let a = Amb::novo();
    let mut d = Diario::novo(&a.state());
    // PEDIDO aceito ha 61 min sem prazo; bloqueio recente (since = agora). A idade continua contando do aceite.
    d.pedido("PEDIDO", "c06-idade", A, B, &ha_min(120), None)
        .aceite("c06-idade", &ha_min(61));
    d.bloqueado("c06-idade", &ha_min(0), "user_input");
    let al = alertas(&a, &[]);
    assert!(
        tipos_de(&al, "c06-idade").iter().any(|t| t == "no_result"),
        "no_result deve contar do aceite (61 min > 60): {al:?}"
    );
}

#[test]
fn c06f_resulted_encerra_os_alertas_da_mensagem() {
    let a = Amb::novo();
    let mut d = Diario::novo(&a.state());
    d.pedido("PEDIDO", "c06-fim", A, B, &ha_min(10), Some(300))
        .aceite("c06-fim", &ha_min(9));
    d.resultado("c06-fim", &ha_min(1), "done");
    let al = alertas(&a, &[]);
    assert!(
        tipos_de(&al, "c06-fim").is_empty(),
        "resultado encerra todos os alertas: {al:?}"
    );
}

#[test]
fn c06g_vence_aceita_resultado_gera_um_unico_alerta() {
    let a = Amb::novo();
    let mut d = Diario::novo(&a.state());
    d.pedido("PEDIDO", "c06-unico", A, B, &ha_min(10), Some(300));
    let stamp = s(&a.stamp()).to_string();
    let primeiro = alertas(&a, &["--stamp-file", stamp.as_str()]);
    // Aceita e resulta depois do vencimento, ainda no mesmo log.
    let mut d2 = Diario::abre(&a.state());
    d2.aceite("c06-unico", &ha_min(8))
        .resultado("c06-unico", &ha_min(7), "done");
    let segundo = alertas(&a, &["--stamp-file", stamp.as_str()]);
    let total = tipos_de(&primeiro, "c06-unico").len() + tipos_de(&segundo, "c06-unico").len();
    assert_eq!(
        total, 1,
        "um unico alerta ao longo das duas rodadas: {primeiro:?} / {segundo:?}"
    );
}

// Salto de relogio (C6): nao coberto; exigiria manipular o relogio do processo ou o `ts` do log, fora do contrato.

// =====================================================================
// C7: autoridade: remetente e terceiro => not_found, sem evento; --as nao concede; auth off => unauthenticated
// =====================================================================

#[test]
fn c07a_remetente_nao_pode_gravar_bloqueado_e_recebe_not_found() {
    let (a, _srv) = amb_daemon(Some(true), true);
    criar_aceito(&a, "c07a", B);
    let r = ack_de(&a, A, "c07a", "bloqueado", Some("permission"));
    assert_eq!(cod(&r), "not_found", "remetente: {r}");
    assert!(eventos_de(&a, "blocked", "c07a").is_empty(), "sem evento");
}

#[test]
fn c07b_terceiro_nao_pode_nem_com_as_do_destinatario() {
    let (a, _srv) = amb_daemon(Some(true), true);
    criar_aceito(&a, "c07b", B);
    // Autenticado como C, pedindo por B: a identidade autenticada e C, nao B.
    let r = ack_de(&a, C, "c07b", "bloqueado", Some("permission"));
    assert_eq!(cod(&r), "not_found", "terceiro: {r}");
    // `--as B` na CLI com o token de C: o daemon autentica C e o resultado nao muda.
    let st = cli_id(
        &a,
        B,
        &[
            "ack",
            "--id",
            "c07b",
            "--as",
            B,
            "--stage",
            "bloqueado",
            "--await",
            "permission",
        ],
    );
    assert!(
        st.code != Some(0),
        "CLI com token de C e --as B nao pode gravar: {st:?}"
    );
    assert!(eventos_de(&a, "blocked", "c07b").is_empty(), "sem evento");
}

#[test]
fn c07c_com_auth_off_o_bloqueado_responde_unauthenticated() {
    let (a, _srv) = amb_daemon(Some(true), false);
    assert_eq!(
        envia_cli(&a, A, B, "PEDIDO", "c07c", None, None, &[]).code,
        Some(0)
    );
    let mut c = Conexao::abre(&a.socket());
    let r = c.chama(
        "message.ack",
        json!({"v": 1, "id": "c07c", "by": B, "stage": "bloqueado", "await": "permission"}),
    );
    assert_eq!(
        cod(&r),
        "unauthenticated",
        "bloqueado exige auth sempre: {r}"
    );
}

// =====================================================================
// C8: transicoes: bloqueado sobre registrado/resultado => invalid_transition; RESPOSTA limpa; --keep-open
// =====================================================================

#[test]
fn c08a_bloqueado_sobre_registrado_e_invalid_transition_sem_evento() {
    let (a, _srv) = amb_daemon(Some(true), true);
    assert_eq!(
        envia_cli(&a, A, B, "PEDIDO", "c08a", None, None, &[]).code,
        Some(0)
    );
    let r = ack_de(&a, B, "c08a", "bloqueado", Some("permission"));
    assert_eq!(cod(&r), "invalid_transition", "{r}");
    assert!(eventos_de(&a, "blocked", "c08a").is_empty(), "sem evento");
}

#[test]
fn c08b_bloqueado_sobre_resultado_e_invalid_transition_sem_evento() {
    let (a, _srv) = amb_daemon(Some(true), true);
    criar_aceito(&a, "c08b", B);
    assert!(!is_err(&ack_de(&a, B, "c08b", "resultado", None)));
    let r = ack_de(&a, B, "c08b", "bloqueado", Some("permission"));
    assert_eq!(cod(&r), "invalid_transition", "resultado e terminal: {r}");
    assert!(eventos_de(&a, "blocked", "c08b").is_empty(), "sem evento");
}

#[test]
fn c08c_resposta_automatica_limpa_o_bloqueio() {
    let (a, _srv) = amb_daemon(Some(true), true);
    criar_aceito(&a, "c08c", B);
    assert!(!is_err(&ack_de(
        &a,
        B,
        "c08c",
        "bloqueado",
        Some("network")
    )));
    let r = envia_cli(&a, B, A, "RESPOSTA", "c08c-r", None, Some("c08c"), &[]);
    assert_eq!(r.code, Some(0), "RESPOSTA: {r:?}");
    assert_eq!(
        estado_de(&a, A, "c08c"),
        "resultado",
        "resposta automatica fecha a mensagem"
    );
    let itens = obrigacoes(&a);
    assert!(
        achado(&itens, "c08c").is_none(),
        "bloqueio limpo: {itens:?}"
    );
}

#[test]
fn c08d_keep_open_nao_fecha_e_o_bloqueio_continua() {
    let (a, _srv) = amb_daemon(Some(true), true);
    criar_aceito(&a, "c08d", B);
    assert!(!is_err(&ack_de(
        &a,
        B,
        "c08d",
        "bloqueado",
        Some("network")
    )));
    let r = envia_cli(
        &a,
        B,
        A,
        "RESPOSTA",
        "c08d-r",
        None,
        Some("c08d"),
        &["--keep-open"],
    );
    assert_eq!(r.code, Some(0), "RESPOSTA com --keep-open: {r:?}");
    assert_eq!(estado_de(&a, A, "c08d"), "aceito", "--keep-open nao fecha");
    let itens = obrigacoes(&a);
    let it = achado(&itens, "c08d").expect("continua em obligations");
    assert_eq!(acha_chave(it, "await"), Some(json!("network")), "{it}");
}

// Corrida real entre resultado e bloqueado nao e testada: o cliente de teste so faz chamadas em sequencia.

// =====================================================================
// C9: destino: nenhum recibo gera resposta automatica de outro agente
// =====================================================================

#[test]
fn c09a_nenhum_recibo_gera_mensagem_para_quem_recebe() {
    let (a, _srv) = amb_daemon(Some(true), true);
    criar_aceito(&a, "c09a", B);
    let conta = |a: &Amb| -> usize {
        let mut c = autenticada(a, A);
        let r = c.chama("message.list", json!({"v": 1, "to": A, "limit": 200}));
        res(&r)["messages"].as_array().map_or(0, |v| v.len())
    };
    let antes = conta(&a);
    assert!(!is_err(&ack_de(
        &a,
        B,
        "c09a",
        "bloqueado",
        Some("permission")
    )));
    assert_eq!(
        conta(&a),
        antes,
        "o bloqueado nao pode gerar mensagem automatica para A"
    );
}

// Orquestrador como destino (C9): nao coberto. O contrato nao diz como o cliente identifica o orquestrador.

// =====================================================================
// C10: visible()/watch --receipts: RECIBO sem nota; terceiro nao ve
// =====================================================================

#[test]
fn c10a_remetente_ve_recibo_bloqueado_sem_nota() {
    let (a, _srv) = amb_daemon(Some(true), true);
    assert_eq!(
        envia_cli(&a, A, B, "PEDIDO", "c10a", None, None, &[]).code,
        Some(0)
    );
    assert!(!is_err(&ack_de(&a, B, "c10a", "aceito", None)));
    let w = Vigia::inicia(&a, A, &["--receipts"]);
    assert!(!is_err(&ack_de(
        &a,
        B,
        "c10a",
        "bloqueado",
        Some("permission")
    )));
    let txt = w.fim();
    let linha = txt
        .lines()
        .find(|l| l.starts_with("RECIBO ") && l.contains("c10a"));
    let linha =
        linha.unwrap_or_else(|| panic!("remetente deveria receber RECIBO do bloqueado: {txt:?}"));
    assert!(
        linha.contains("bloqueado:permission"),
        "formato RECIBO SEQ ID bloqueado:CLASSE TO: {linha}"
    );
    assert!(
        linha.contains(B),
        "TO deve ser o destinatario que bloqueou: {linha}"
    );
    assert!(!tem_marca(&txt), "sem nota nem texto livre: {txt}");
}

#[test]
fn c10b_terceiro_nao_ve_o_recibo_bloqueado() {
    let (a, _srv) = amb_daemon(Some(true), true);
    assert_eq!(
        envia_cli(&a, A, B, "PEDIDO", "c10b", None, None, &[]).code,
        Some(0)
    );
    assert!(!is_err(&ack_de(&a, B, "c10b", "aceito", None)));
    let w = Vigia::inicia(&a, C, &["--receipts"]);
    assert!(!is_err(&ack_de(
        &a,
        B,
        "c10b",
        "bloqueado",
        Some("permission")
    )));
    let txt = w.fim();
    assert!(!txt.contains("c10b"), "terceiro nao ve bloqueado: {txt}");
}

// events.resume antes/depois do bloqueio, limpeza e reconexao (C10): nao coberto nesta rodada.

// =====================================================================
// C11: limites: 7o bloqueado na mesma mensagem e 31o recibo/min => rate_limited antes de persistir
// =====================================================================

#[test]
fn c11a_setimo_bloqueado_na_mesma_mensagem_e_rate_limited() {
    let (a, _srv) = amb_daemon(Some(true), true);
    criar_aceito(&a, "c11a", B);
    for classe in CLASSES {
        assert!(
            !is_err(&ack_de(&a, B, "c11a", "bloqueado", Some(classe))),
            "classe {classe}"
        );
    }
    assert_eq!(
        eventos_de(&a, "blocked", "c11a").len(),
        6,
        "seis eventos ate aqui"
    );
    // 7a mudanca de classe (permission != unknown, a ultima gravada).
    let r = ack_de(&a, B, "c11a", "bloqueado", Some("permission"));
    assert_eq!(cod(&r), "rate_limited", "7o bloqueado: {r}");
    assert_eq!(
        eventos_de(&a, "blocked", "c11a").len(),
        6,
        "rate_limited nao persiste"
    );
}

#[test]
fn c11b_31o_recibo_por_minuto_e_rate_limited_e_flood_nao_atrasa_outro() {
    let (a, _srv) = amb_daemon(Some(true), true);
    let mut c_a = autenticada(&a, A);
    for i in 0..31 {
        let id = format!("c11b-{i}");
        let m = msg(&id, A, B, "PEDIDO", "assunto", "corpo");
        let r = c_a.chama("message.send", m);
        assert!(!is_err(&r), "send {id}: {r}");
    }
    let mut c_b = autenticada(&a, B);
    for i in 0..30 {
        let r = c_b.chama(
            "message.ack",
            json!({"v": 1, "id": format!("c11b-{i}"), "by": B, "stage": "aceito"}),
        );
        assert!(!is_err(&r), "recibo {i} dentro do limite: {r}");
    }
    let r31 = c_b.chama(
        "message.ack",
        json!({"v": 1, "id": "c11b-30", "by": B, "stage": "aceito"}),
    );
    assert_eq!(cod(&r31), "rate_limited", "31o recibo no minuto: {r31}");
    // Medido: o message.send de outro remetente nao espera o flood.
    let t = Instant::now();
    let mut c_c = autenticada(&a, C);
    let r = c_c.chama(
        "message.send",
        msg("c11b-outro", C, A, "AVISO", "assunto", "corpo"),
    );
    assert!(!is_err(&r), "send de outro: {r}");
    assert!(
        t.elapsed() < Duration::from_secs(1),
        "flood atrasou outro remetente: {:?}",
        t.elapsed()
    );
}

// =====================================================================
// C12: alertas: --due 1s barrado; 50 PEDIDOs => uma linha agregada; dois stale concorrentes
// =====================================================================

#[test]
fn c12a_due_1s_e_barrado_pela_cli() {
    let (a, _srv) = amb_daemon(Some(true), true);
    let r = envia_cli(&a, A, B, "PEDIDO", "c12a", Some("1s"), None, &[]);
    assert_eq!(r.code, Some(2), "--due 1s: {r:?}");
}

#[test]
fn c12b_cinquenta_pedidos_a_um_destinatario_geram_uma_linha_agregada() {
    let a = Amb::novo();
    let mut d = Diario::novo(&a.state());
    for i in 0..50 {
        d.pedido("PEDIDO", &format!("p-{i:02}"), A, B, &ha_min(10), Some(300));
    }
    let r = stale(&a, &[]);
    let linhas: Vec<&str> = r
        .stdout
        .lines()
        .filter(|l| l.contains("due_expired"))
        .collect();
    // AMBIGUO: o texto da linha e o nome do tipo na saida texto nao estao fixados; so se exige uma linha por
    // (destinatario, tipo) com as 50 ids na mesma linha.
    assert_eq!(linhas.len(), 1, "uma linha agregada: {}", r.stdout);
    assert!(
        linhas[0].contains("p-00") && linhas[0].contains("p-49"),
        "as ids na mesma linha: {}",
        linhas[0]
    );
}

#[test]
fn c12c_dois_stale_concorrentes_nao_duplicam_alem_da_janela() {
    let a = Amb::novo();
    let mut d = Diario::novo(&a.state());
    d.pedido("PEDIDO", "c12c", A, B, &ha_min(10), Some(300));
    let stamp = s(&a.stamp()).to_string();
    let st = s(&a.state()).to_string();
    let (r1, r2) = thread::scope(|sc| {
        let h1 = sc.spawn(|| {
            roda(
                &a,
                &[
                    "stale",
                    "--state-dir",
                    st.as_str(),
                    "--stamp-file",
                    stamp.as_str(),
                ],
                &[],
            )
        });
        let h2 = sc.spawn(|| {
            roda(
                &a,
                &[
                    "stale",
                    "--state-dir",
                    st.as_str(),
                    "--stamp-file",
                    stamp.as_str(),
                ],
                &[],
            )
        });
        (h1.join().unwrap(), h2.join().unwrap())
    });
    let n = |r: &Saida| r.stdout.lines().filter(|l| l.contains("c12c")).count();
    let total = n(&r1) + n(&r2);
    // AMBIGUO: o tamanho da janela de duplicata nao esta fixado; so se exige que nao haja mais do que dois avisos.
    assert!(
        total >= 1 && total <= 2,
        "concorrentes: {total} avisos ({r1:?} / {r2:?})"
    );
}

// Stamp antigo sem `version` (C12) e crash entre marcar e notificar: nao cobertos, o contrato nao fixa o formato
// do stamp anterior nem ha como provocar a queda no meio do aviso.

// =====================================================================
// C13: compatibilidade: ack antigo inalterado; features.blocked_receipts false/ausente nao grava blocked
// =====================================================================

#[test]
fn c13a_ack_antigo_aceito_e_resultado_inalterados() {
    let (a, _srv) = amb_daemon(Some(true), true);
    criar_aceito(&a, "c13a", B);
    let r = cli_id(
        &a,
        B,
        &[
            "ack",
            "--id",
            "c13a",
            "--as",
            B,
            "--stage",
            "resultado",
            "--result",
            "done",
        ],
    );
    assert_eq!(r.code, Some(0), "resultado antigo: {r:?}");
    assert_eq!(estado_de(&a, A, "c13a"), "resultado");
    assert!(
        eventos(&a, "blocked").is_empty(),
        "nenhum evento novo sem bloqueado"
    );
}

#[test]
fn c13b_features_ausente_nao_grava_blocked() {
    let (a, _srv) = amb_daemon(None, true);
    criar_aceito(&a, "c13b", B);
    let _ = ack_de(&a, B, "c13b", "bloqueado", Some("permission"));
    assert!(
        eventos(&a, "blocked").is_empty(),
        "padrao (features ausente) nao grava recibo bloqueado"
    );
}

#[test]
fn c13c_features_false_nao_grava_blocked() {
    let (a, _srv) = amb_daemon(Some(false), true);
    criar_aceito(&a, "c13c", B);
    let _ = ack_de(&a, B, "c13c", "bloqueado", Some("permission"));
    // AMBIGUO: a resposta do daemon com a feature desligada nao esta fixada; so se exige que nao haja evento.
    assert!(
        eventos(&a, "blocked").is_empty(),
        "features.blocked_receipts false nao grava evento"
    );
}

// Daemon v1.7 recusando o estagio novo e log com `blocked` em daemon antigo (C13): nao cobertos (exigem o binario antigo).

// =====================================================================
// C14: show: nota de refused como dado nao confiavel, controles escapados; nota fora de obligations e de listas
// =====================================================================

#[test]
fn c14a_show_imprime_a_nota_escapada_e_sem_controle_cru() {
    let (a, _srv) = amb_daemon(Some(true), true);
    criar_aceito(&a, "c14a", B);
    let nota = format!("\x1b[31m{MARCA_NOTA}");
    let r = cli_id(
        &a,
        B,
        &[
            "ack",
            "--id",
            "c14a",
            "--as",
            B,
            "--stage",
            "resultado",
            "--result",
            "refused",
            "--note",
            nota.as_str(),
        ],
    );
    assert_eq!(r.code, Some(0), "refused com --note: {r:?}");
    let sh = cli_id(&a, A, &["show", "--id", "c14a"]);
    assert_eq!(sh.code, Some(0), "show: {sh:?}");
    assert!(
        !sh.stdout.contains('\u{1b}'),
        "controle cru no show: {:?}",
        sh.stdout
    );
    assert!(
        sh.stdout.contains(MARCA_NOTA),
        "a nota aparece (escapada) no show explicito: {:?}",
        sh.stdout
    );
}

#[test]
fn c14b_nota_nunca_aparece_em_obligations_alertas_nem_json_de_lista() {
    let (a, _srv) = amb_daemon(Some(true), true);
    criar_aceito(&a, "c14b", B);
    let r = cli_id(
        &a,
        B,
        &[
            "ack",
            "--id",
            "c14b",
            "--as",
            B,
            "--stage",
            "resultado",
            "--result",
            "refused",
            "--note",
            MARCA_NOTA,
        ],
    );
    assert_eq!(r.code, Some(0), "{r:?}");
    let st = s(&a.state()).to_string();
    let ob_txt = roda(&a, &["obligations", "--state-dir", st.as_str()], &[]);
    let ob_js = roda(
        &a,
        &["obligations", "--json", "--state-dir", st.as_str()],
        &[],
    );
    let al = stale(&a, &["--json"]);
    for (quem, t) in [
        ("obligations texto", &ob_txt),
        ("obligations json", &ob_js),
        ("stale json", &al),
    ] {
        assert!(
            !t.stdout.contains(MARCA_NOTA),
            "{quem} vazou a nota: {}",
            t.stdout
        );
    }
}

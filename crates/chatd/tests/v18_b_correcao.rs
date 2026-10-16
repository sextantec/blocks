//! Correcoes da auditoria da Parte B (docs/spec-v1.8-stale-e-anexos.md, secao "Correcoes da auditoria da Parte B",
//! 2026-10-16): D1 (taxa de `blob.get` por identidade), D2 (evento `blob_got` uma vez por par blob/identidade) e
//! D3 (SHA-256 incremental com pedacos de qualquer tamanho). Escrito so a partir do contrato e dos helpers de
//! tests/common/mod.rs e tests/v18_b_anexos.rs; src/ nao foi lido. Caixa preta: binario, socket e arquivos.
//! Diretorio temporario por teste; HOME, XDG_CONFIG_HOME e XDG_STATE_HOME dentro dele; CHATD_*, HERDR_* e TMUX*
//! removidas do filho; credenciais falsas; nenhuma rede. O SHA-256 de referencia e o `sha256sum` do sistema.
//! Os testes que falham hoje falham pelo defeito descrito no contrato, nao por erro de compilacao ou de teste.
//! AMBIGUO: marcado onde o contrato nao fixa formato (ex.: `data` do blob.get) ou comportamento (ex.: D2a).
#![allow(dead_code)]
mod common;

use common::*;

use chrono::{SecondsFormat, TimeDelta, Utc};
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

/// Sentinela do conteudo do anexo (B12): nunca pode aparecer no log, no status nem em erros.
const MARCA_CONTEUDO: &str = "CONTEUDO-SECRETO-V18B";

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

// ---------- base64 e SHA-256 (sem dependencias novas) ----------

/// Base64 padrao (RFC 4648) dos bytes; usado nos pedacos de `blob.put.chunk`.
fn b64(dados: &[u8]) -> String {
    const T: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::new();
    for c in dados.chunks(3) {
        let b = [c[0], *c.get(1).unwrap_or(&0), *c.get(2).unwrap_or(&0)];
        let n = ((b[0] as u32) << 16) | ((b[1] as u32) << 8) | b[2] as u32;
        for k in 0..4 {
            if k <= c.len() {
                out.push(T[((n >> (18 - 6 * k)) & 63) as usize] as char);
            } else {
                out.push('=');
            }
        }
    }
    out
}

/// SHA-256 em hexadecimal, calculado pelo `sha256sum` do sistema sobre a entrada padrao.
fn sha256_hex(dados: &[u8]) -> String {
    use std::io::Read;
    let mut f = Command::new("sha256sum")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .expect("sha256sum");
    f.stdin.take().unwrap().write_all(dados).unwrap();
    let mut out = String::new();
    f.stdout.take().unwrap().read_to_string(&mut out).unwrap();
    let _ = f.wait();
    out.split_whitespace().next().unwrap().to_string()
}

// ---------- protocolo de anexos (sessao na mesma conexao) ----------

const PEDACO: usize = 512 * 1024;

/// Envia `dados` como anexo de `quem` para `para`, pela mesma conexao (o upload e ligado a conexao).
/// Devolve a resposta do ultimo passo: o `end` quando tudo deu certo, ou o primeiro erro.
fn put_raw(a: &Amb, quem: &str, para: &str, nome: Option<&str>, dados: &[u8]) -> Value {
    let mut c = autenticada(a, quem);
    let mut b = json!({"v": 1, "size": dados.len(), "to": para});
    if let Some(n) = nome {
        b["name"] = json!(n);
    }
    let r = c.chama("blob.put.begin", b);
    if is_err(&r) {
        return r;
    }
    let up = res(&r)["upload_id"].as_str().unwrap().to_string();
    for (n, p) in dados.chunks(PEDACO).enumerate() {
        let r = c.chama(
            "blob.put.chunk",
            json!({"v": 1, "upload_id": up, "n": n, "data": b64(p)}),
        );
        if is_err(&r) {
            return r;
        }
    }
    c.chama(
        "blob.put.end",
        json!({"v": 1, "upload_id": up, "sha256": sha256_hex(dados)}),
    )
}

/// Id `blob:...` do resultado de `put_raw` bem-sucedido.
fn id_do_blob(r: &Value) -> String {
    res(r)["blob"]
        .as_str()
        .unwrap_or_else(|| panic!("sem blob em {r}"))
        .to_string()
}

/// `blob.get` como `quem`, sem decodificar os bytes (so o envelope).
fn get_raw(a: &Amb, quem: &str, blob: &str) -> Value {
    let mut c = autenticada(a, quem);
    c.chama("blob.get", json!({"v": 1, "blob": blob}))
}

/// `blob.rm` como `quem`.
fn rm_raw(a: &Amb, quem: &str, blob: &str) -> Value {
    let mut c = autenticada(a, quem);
    c.chama("blob.rm", json!({"v": 1, "blob": blob}))
}

/// Arquivo em disco do blob (procura o nome que contem o hex do id sob `state/blobs`). None se nao houver.
fn arquivo_do_blob(a: &Amb, id: &str) -> Option<PathBuf> {
    let hex = id.trim_start_matches("blob:").to_string();
    fn anda(d: &Path, hex: &str, achados: &mut Vec<PathBuf>) {
        if let Ok(rd) = fs::read_dir(d) {
            for e in rd.flatten() {
                let p = e.path();
                if p.is_dir() {
                    anda(&p, hex, achados);
                } else if p
                    .file_name()
                    .is_some_and(|n| n.to_string_lossy().contains(hex))
                {
                    achados.push(p);
                }
            }
        }
    }
    let mut v = Vec::new();
    anda(&a.state().join("blobs"), &hex, &mut v);
    v.into_iter().next()
}

/// Erros de `blob.*` tem `error.code` string (como nos demais metodos).
fn esperado(r: &Value, codigo: &str) {
    assert!(is_err(r), "esperava erro {codigo}, veio resultado: {r}");
    assert_eq!(cod(r), codigo, "codigo de erro esperado {codigo}: {r}");
}

/// Eventos `blob_*` do log, de qualquer tipo, como texto (para checar presenca e ausencia de sentinelas).
fn log_texto(a: &Amb) -> String {
    fs::read_to_string(a.state().join("log.jsonl")).unwrap_or_default()
}

/// Sobe o `serve` sem esperar pronto: devolve (saiu com erro dentro de 5 s?, texto de pronto visto?).
fn sobe_sem_esperar(a: &Amb, cfg: &Path) -> (Option<i32>, bool) {
    let out = a.p().join("serve-x.out");
    let err = a.p().join("serve-x.err");
    let st = s(&a.state()).to_string();
    let mut cmd = Command::new(BIN);
    cmd.args(["serve", "--state-dir", st.as_str(), "--config", s(cfg)]);
    ambiente(&mut cmd, a);
    let mut child = cmd
        .stdin(Stdio::null())
        .stdout(fs::File::create(&out).unwrap())
        .stderr(fs::File::create(&err).unwrap())
        .spawn()
        .expect("executar serve");
    let fim = Instant::now() + Duration::from_secs(5);
    let code = loop {
        if let Ok(Some(st)) = child.try_wait() {
            break st.code();
        }
        if Instant::now() >= fim {
            let _ = child.kill();
            let _ = child.wait();
            break None;
        }
        thread::sleep(Duration::from_millis(20));
    };
    let pronto = fs::read_to_string(&out)
        .unwrap_or_default()
        .lines()
        .any(|l| l == PRONTO);
    (code, pronto)
}

/// Arquivo de entrada de teste com `dados` dentro do temporario.
fn arquivo_com(a: &Amb, nome: &str, dados: &[u8]) -> PathBuf {
    let p = a.p().join(nome);
    fs::write(&p, dados).unwrap();
    p
}

// =====================================================================
// D1: taxa de blob.get: 600 por identidade em 60 s; toda chamada conta (inclusive not_found)
// =====================================================================

/// Chamadas de `blob.get` do blob `id` pela conexao `c` (uma por vez).
fn gets(c: &mut Conexao, id: &str, n: usize) -> Vec<Value> {
    (0..n)
        .map(|_| c.chama("blob.get", json!({"v": 1, "blob": id})))
        .collect()
}

#[test]
fn d1a_seiscentos_gets_passam_e_o_601o_e_rate_limited_sem_vazar_id() {
    let (a, _srv) = amb_daemon(Some(true), true);
    let id = id_do_blob(&put_raw(&a, A, B, None, b"d1a"));
    let mut c = autenticada(&a, A);
    for (i, r) in gets(&mut c, &id, 600).iter().enumerate() {
        assert!(!is_err(r), "get {} dentro do limite de 600: {r}", i + 1);
    }
    let r601 = c.chama("blob.get", json!({"v": 1, "blob": id}));
    esperado(&r601, "rate_limited");
    assert!(
        !r601.to_string().contains(&id),
        "a mensagem de rate_limited nao traz o id: {r601}"
    );
}

#[test]
fn d1b_identidade_b_continua_lendo_enquanto_a_esta_limitada() {
    let (a, _srv) = amb_daemon(Some(true), true);
    let id = id_do_blob(&put_raw(&a, A, B, None, b"d1b"));
    let mut ca = autenticada(&a, A);
    let _ = gets(&mut ca, &id, 601);
    let r_a = ca.chama("blob.get", json!({"v": 1, "blob": id}));
    esperado(&r_a, "rate_limited");
    let r_b = get_raw(&a, B, &id);
    assert!(!is_err(&r_b), "B nao herda o limite de A: {r_b}");
}

#[test]
fn d1c_not_found_tambem_conta_para_o_limite() {
    let (a, _srv) = amb_daemon(Some(true), true);
    let id = id_do_blob(&put_raw(&a, A, B, None, b"d1c"));
    let inexistente = "blob:00000000000000000000000000000001";
    let mut c = autenticada(&a, A);
    for r in gets(&mut c, inexistente, 600).iter() {
        esperado(r, "not_found");
    }
    let r601 = c.chama("blob.get", json!({"v": 1, "blob": id}));
    esperado(&r601, "rate_limited");
}

// =====================================================================
// D2: evento blob_got uma vez por par (blob, identidade que leu); limite de 2 por blob ativo
// =====================================================================

/// Eventos `blob_got` do log que citam o id.
fn blob_got_de(a: &Amb, id: &str) -> Vec<Value> {
    eventos(a, "blob_got")
        .into_iter()
        .filter(|e| e.to_string().contains(id))
        .collect()
}

#[test]
fn d2a_mil_gets_do_mesmo_blob_pela_mesma_identidade_deixam_um_blob_got() {
    let (a, _srv) = amb_daemon(Some(true), true);
    let id = id_do_blob(&put_raw(&a, A, B, None, b"d2a"));
    let mut c = autenticada(&a, A);
    let primeiro = c.chama("blob.get", json!({"v": 1, "blob": id}));
    assert!(
        !is_err(&primeiro),
        "o primeiro get e bem-sucedido: {primeiro}"
    );
    // AMBIGUO: 1000 gets excedem o limite de 600 por janela (D1); os recusados nao importam para o evento.
    for _ in 0..999 {
        let _ = c.chama("blob.get", json!({"v": 1, "blob": id}));
    }
    assert_eq!(
        blob_got_de(&a, &id).len(),
        1,
        "exatamente um blob_got para o par dono/leitor"
    );
}

#[test]
fn d2b_dono_e_destinatario_geram_dois_eventos_no_total_e_nunca_mais() {
    let (a, _srv) = amb_daemon(Some(true), true);
    let id = id_do_blob(&put_raw(&a, A, B, None, b"d2b"));
    let mut ca = autenticada(&a, A);
    let mut cb = autenticada(&a, B);
    for _ in 0..3 {
        assert!(
            !is_err(&ca.chama("blob.get", json!({"v": 1, "blob": id}))),
            "dono le"
        );
        assert!(
            !is_err(&cb.chama("blob.get", json!({"v": 1, "blob": id}))),
            "destinatario le"
        );
    }
    assert_eq!(
        blob_got_de(&a, &id).len(),
        2,
        "um evento por par, dois pares no total"
    );
}

#[test]
fn d2c_apos_remover_e_criar_outro_o_estado_e_novo_um_evento_por_par() {
    let (a, _srv) = amb_daemon(Some(true), true);
    let x = id_do_blob(&put_raw(&a, A, B, None, b"d2c-x"));
    assert!(!is_err(&get_raw(&a, A, &x)), "dono le X");
    assert!(!is_err(&rm_raw(&a, B, &x)), "destinatario remove X");
    esperado(&get_raw(&a, A, &x), "not_found");
    let y = id_do_blob(&put_raw(&a, A, B, None, b"d2c-y"));
    for _ in 0..2 {
        assert!(!is_err(&get_raw(&a, A, &y)));
        assert!(!is_err(&get_raw(&a, B, &y)));
    }
    assert_eq!(
        blob_got_de(&a, &y).len(),
        2,
        "o blob novo: um evento para o dono e um para o destinatario"
    );
    assert_eq!(
        blob_got_de(&a, &x).len(),
        1,
        "X nao ganha evento depois de removido"
    );
}

#[test]
fn d2d_dados_repetidos_sao_identicos_e_corretos() {
    let (a, _srv) = amb_daemon(Some(true), true);
    let dados: Vec<u8> = (0..120_000u32)
        .map(|i| (i.wrapping_mul(31) % 253) as u8)
        .collect();
    let id = id_do_blob(&put_raw(&a, A, B, None, &dados));
    for k in 0..3 {
        let out = a.p().join(format!("repetido-{k}.bin"));
        let o = s(&out).to_string();
        let r = cli_id(&a, B, &["get", id.as_str(), "--out", o.as_str()]);
        assert_eq!(r.code, Some(0), "get {k}: {r:?}");
        assert_eq!(
            fs::read(&out).unwrap(),
            dados,
            "leitura {k} identica e correta"
        );
    }
}

// =====================================================================
// D3: SHA-256 incremental: pedacos de qualquer tamanho dao o mesmo hash; hash errado continua hash_mismatch
// =====================================================================

/// Envia `dados` em pedacos: os `cortes` primeiro (em ordem), o resto em um pedaco final. Devolve o `end`.
fn put_em_pedacos(a: &Amb, quem: &str, para: &str, dados: &[u8], cortes: &[usize]) -> Value {
    let mut c = autenticada(a, quem);
    let r = c.chama(
        "blob.put.begin",
        json!({"v": 1, "size": dados.len(), "to": para}),
    );
    if is_err(&r) {
        return r;
    }
    let up = res(&r)["upload_id"].as_str().unwrap().to_string();
    let mut tam: Vec<usize> = cortes.to_vec();
    let mut pos = 0usize;
    let mut n = 0u64;
    while pos < dados.len() {
        let resto = dados.len() - pos;
        let t = if tam.is_empty() {
            resto
        } else {
            tam.remove(0).min(resto)
        };
        let r = c.chama(
            "blob.put.chunk",
            json!({"v": 1, "upload_id": up, "n": n, "data": b64(&dados[pos..pos + t])}),
        );
        if is_err(&r) {
            return r;
        }
        pos += t;
        n += 1;
    }
    c.chama(
        "blob.put.end",
        json!({"v": 1, "upload_id": up, "sha256": sha256_hex(dados)}),
    )
}

fn dados_d3() -> Vec<u8> {
    (0..250_000u32)
        .map(|i| (i.wrapping_mul(7) % 251) as u8)
        .collect()
}

#[test]
fn d3a_pedacos_nao_alinhados_fecham_e_o_get_devolve_os_bytes() {
    let (a, _srv) = amb_daemon(Some(true), true);
    let dados = dados_d3();
    // Alinhamentos: 1 byte + resto; 63; 64 + 1 + resto; 65; 100000 + resto; 19 pedacos de 1 byte + resto
    // (ate 20 pedacos por sessao: o limite do protocolo, nao e o defeito de hash).
    let casos: Vec<Vec<usize>> = vec![
        vec![1],
        vec![63],
        vec![64, 1],
        vec![65],
        vec![100_000],
        vec![1; 19],
    ];
    for (k, cortes) in casos.iter().enumerate() {
        let r = put_em_pedacos(&a, A, B, &dados, cortes);
        assert!(
            !is_err(&r),
            "put {k} com cortes {cortes:?} deve fechar com sucesso: {r}"
        );
        let id = id_do_blob(&r);
        let out = a.p().join(format!("d3a-{k}.bin"));
        let o = s(&out).to_string();
        let g = cli_id(&a, B, &["get", id.as_str(), "--out", o.as_str()]);
        assert_eq!(g.code, Some(0), "get do put {k}: {g:?}");
        assert_eq!(fs::read(&out).unwrap(), dados, "bytes do put {k} identicos");
    }
}

#[test]
fn d3b_hash_errado_com_pedacos_nao_alinhados_responde_hash_mismatch() {
    let (a, _srv) = amb_daemon(Some(true), true);
    let dados = dados_d3();
    let mut c = autenticada(&a, A);
    let up = res(&c.chama(
        "blob.put.begin",
        json!({"v": 1, "size": dados.len(), "to": B}),
    ))["upload_id"]
        .as_str()
        .unwrap()
        .to_string();
    assert!(!is_err(&c.chama(
        "blob.put.chunk",
        json!({"v": 1, "upload_id": up, "n": 0, "data": b64(&dados[..1])})
    )));
    assert!(!is_err(&c.chama(
        "blob.put.chunk",
        json!({"v": 1, "upload_id": up, "n": 1, "data": b64(&dados[1..])})
    )));
    let fim = c.chama(
        "blob.put.end",
        json!({"v": 1, "upload_id": up, "sha256": "0".repeat(64)}),
    );
    esperado(&fim, "hash_mismatch");
}

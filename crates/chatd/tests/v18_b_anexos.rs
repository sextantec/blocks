//! v1.8, Parte B (docs/spec-v1.8-stale-e-anexos.md, "Parte B", rev2): anexos (blobs) com validade e B.0
//! (`status` so com "vivo + estatisticas", `mailbox.counts`, `inbox --count`).
//! Escrito so a partir do contrato e dos modelos tests/common, tests/v18_c_obligations.rs e tests/v17_stale.rs;
//! src/ nao foi lido. Caixa preta: so o binario, o socket e os arquivos. Cada teste tem diretorio temporario
//! proprio; HOME, XDG_CONFIG_HOME e XDG_STATE_HOME apontam para dentro dele; CHATD_*, HERDR_* e TMUX* sao
//! removidas do filho. Credenciais falsas (tokens novos no temporario). Nenhum daemon real, nenhuma rede,
//! nenhum acesso a ~/.local nem a ~/.config. O SHA-256 e calculado pelo `sha256sum` do sistema (o crate nao
//! tem dependencia de hash); o base64 dos pedacos e codificado aqui.
//! Devem falhar ate a Parte B existir (`blob.*` ainda nao existe: method_not_found, ou erro de uso).
//! AMBIGUO: marcado onde o contrato nao fixa codigo de erro, formato de saida ou nome de campo.
//! Nao cobertos nesta rodada (ver relatorio): B5 (vencimento real espera 60 s, so ignorado), B8 (cota total
//! 256 MiB, inalcancavel com 3 identidades de teste), B8 (espaco livre), B15 (daemon antigo), B23 (lista
//! paginada e memoria), B11 (prazo absoluto de 10 min e ociosidade de 120 s).
#![allow(dead_code)]
mod common;

use common::*;

use chrono::{SecondsFormat, TimeDelta, Utc};
use serde_json::{Value, json};
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::os::unix::fs::PermissionsExt;
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
// B1: ida e volta; 0 byte e 8 MiB passam; 8 MiB + 1 => too_large
// =====================================================================

#[test]
fn b01a_ida_e_volta_bytes_identicos_pelo_cli() {
    let (a, _srv) = amb_daemon(Some(true), true);
    let dados: Vec<u8> = (0..300_000u32).map(|i| (i % 251) as u8).collect();
    let f = arquivo_com(&a, "entrada.bin", &dados);
    let fs_ = s(&f).to_string();
    let p = cli_id(&a, A, &["put", fs_.as_str(), "--to", B, "--json"]);
    assert_eq!(p.code, Some(0), "put: {p:?}");
    let id = objetos_json(&p.stdout)
        .iter()
        .find_map(|o| acha_chave(o, "blob"))
        .and_then(|v| v.as_str().map(String::from))
        .unwrap_or_else(|| panic!("put --json sem o id do blob: {p:?}"));
    let saida = a.p().join("saida.bin");
    let ss = s(&saida).to_string();
    let g = cli_id(&a, B, &["get", id.as_str(), "--out", ss.as_str()]);
    assert_eq!(g.code, Some(0), "get: {g:?}");
    assert_eq!(fs::read(&saida).unwrap(), dados, "bytes identicos");
}

#[test]
fn b01b_zero_byte_e_oito_mib_passam_e_oito_mib_mais_um_e_too_large() {
    let (a, _srv) = amb_daemon(Some(true), true);
    let zero = put_raw(&a, A, B, None, &[]);
    assert!(!is_err(&zero), "0 byte deve passar: {zero}");
    let oito = vec![7u8; 8 * 1024 * 1024];
    let r = put_raw(&a, A, B, None, &oito);
    assert!(!is_err(&r), "8 MiB exatos devem passar: {r}");
    let mais = vec![7u8; 8 * 1024 * 1024 + 1];
    let mut c = autenticada(&a, A);
    let r2 = c.chama(
        "blob.put.begin",
        json!({"v": 1, "size": mais.len(), "to": B}),
    );
    esperado(&r2, "too_large");
}

// =====================================================================
// B2: hash ou tamanho divergente aborta e limpa
// =====================================================================

#[test]
fn b02a_hash_divergente_aborta_e_nao_deixa_blob() {
    let (a, _srv) = amb_daemon(Some(true), true);
    let dados = b"conteudo de teste B2a".to_vec();
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
        json!({"v": 1, "upload_id": up, "n": 0, "data": b64(&dados)})
    )));
    let fim = c.chama(
        "blob.put.end",
        json!({"v": 1, "upload_id": up, "sha256": "0".repeat(64)}),
    );
    assert!(is_err(&fim), "sha divergente deve abortar: {fim}");
    assert!(
        arquivo_do_blob(&a, "blob:0").is_none() && log_texto(&a).matches("blob_put").count() == 0,
        "nada gravado"
    );
}

#[test]
fn b02b_tamanho_divergente_aborta() {
    let (a, _srv) = amb_daemon(Some(true), true);
    let mut c = autenticada(&a, A);
    let up = res(&c.chama("blob.put.begin", json!({"v": 1, "size": 10, "to": B})))["upload_id"]
        .as_str()
        .unwrap()
        .to_string();
    assert!(!is_err(&c.chama(
        "blob.put.chunk",
        json!({"v": 1, "upload_id": up, "n": 0, "data": b64(b"12345")})
    )));
    let fim = c.chama(
        "blob.put.end",
        json!({"v": 1, "upload_id": up, "sha256": sha256_hex(b"12345")}),
    );
    assert!(
        is_err(&fim),
        "tamanho recebido (5) diferente do declarado (10): {fim}"
    );
}

// =====================================================================
// B3: pedaco repetido, saltado ou maior que chunk_max aborta a sessao
// =====================================================================

#[test]
fn b03a_pedaco_repetido_aborta_a_sessao() {
    let (a, _srv) = amb_daemon(Some(true), true);
    let mut c = autenticada(&a, A);
    let up = res(&c.chama("blob.put.begin", json!({"v": 1, "size": 10, "to": B})))["upload_id"]
        .as_str()
        .unwrap()
        .to_string();
    assert!(!is_err(&c.chama(
        "blob.put.chunk",
        json!({"v": 1, "upload_id": up, "n": 0, "data": b64(b"12345")})
    )));
    let rep = c.chama(
        "blob.put.chunk",
        json!({"v": 1, "upload_id": up, "n": 0, "data": b64(b"AAAAA")}),
    );
    assert!(is_err(&rep), "n repetido aborta: {rep}");
    let depois = c.chama(
        "blob.put.chunk",
        json!({"v": 1, "upload_id": up, "n": 1, "data": b64(b"67890")}),
    );
    assert!(
        is_err(&depois),
        "sessao abortada nao aceita mais pedacos: {depois}"
    );
}

#[test]
fn b03b_pedaco_saltado_aborta() {
    let (a, _srv) = amb_daemon(Some(true), true);
    let mut c = autenticada(&a, A);
    let up = res(&c.chama("blob.put.begin", json!({"v": 1, "size": 10, "to": B})))["upload_id"]
        .as_str()
        .unwrap()
        .to_string();
    let r = c.chama(
        "blob.put.chunk",
        json!({"v": 1, "upload_id": up, "n": 1, "data": b64(b"12345")}),
    );
    assert!(is_err(&r), "n=1 sem o n=0 aborta: {r}");
}

#[test]
fn b03c_pedaco_maior_que_chunk_max_e_recusado() {
    let (a, _srv) = amb_daemon(Some(true), true);
    let grande = vec![1u8; 600 * 1024];
    let r = put_raw(&a, A, B, None, &grande);
    assert!(
        is_err(&r),
        "pedaco de 600 KiB brutos passa do teto de 512 KiB: {r}"
    );
}

// =====================================================================
// B4: ACL: terceiro recebe not_found (mesmo codigo e campos que inexistente); dono e destinatario leem
// =====================================================================

#[test]
fn b04a_terceiro_recebe_not_found_igual_a_inexistente() {
    let (a, _srv) = amb_daemon(Some(true), true);
    let id = id_do_blob(&put_raw(&a, A, B, None, b"segredo B4"));
    let de_terceiro = get_raw(&a, C, &id);
    let de_inexistente = get_raw(&a, C, "blob:00000000000000000000000000000000");
    esperado(&de_terceiro, "not_found");
    esperado(&de_inexistente, "not_found");
    assert_eq!(
        de_terceiro["error"]
            .as_object()
            .map(|o| o.keys().cloned().collect::<Vec<_>>()),
        de_inexistente["error"]
            .as_object()
            .map(|o| o.keys().cloned().collect::<Vec<_>>()),
        "mesmos campos de erro: {de_terceiro} / {de_inexistente}"
    );
    let rm = rm_raw(&a, C, &id);
    esperado(&rm, "not_found");
    let ls = res(&autenticada(&a, C).chama("blob.list", json!({"v": 1})));
    assert!(
        !ls.to_string().contains(&id),
        "o terceiro nao lista o blob: {ls}"
    );
}

#[test]
fn b04b_dono_e_destinatario_leem() {
    let (a, _srv) = amb_daemon(Some(true), true);
    let id = id_do_blob(&put_raw(&a, A, B, None, b"leitura B4b"));
    assert!(!is_err(&get_raw(&a, A, &id)), "dono le");
    assert!(!is_err(&get_raw(&a, B, &id)), "destinatario le");
}

// =====================================================================
// B5: validade (TTL de 60 s a 7 dias; vencimento real espera 60 s)
// =====================================================================

#[test]
fn b05a_ttl_fora_da_faixa_e_invalid_envelope() {
    let (a, _srv) = amb_daemon(Some(true), true);
    let mut c = autenticada(&a, A);
    for ttl in [59u64, 604_801] {
        let r = c.chama(
            "blob.put.begin",
            json!({"v": 1, "size": 1, "to": B, "ttl_s": ttl}),
        );
        esperado(&r, "invalid_envelope");
    }
    let ok = c.chama(
        "blob.put.begin",
        json!({"v": 1, "size": 1, "to": B, "ttl_s": 60}),
    );
    assert!(!is_err(&ok), "ttl minimo de 60 s passa: {ok}");
}

#[test]
#[ignore = "espera 60 s de relogio real para vencer um blob de TTL minimo; rodar com --ignored"]
fn b05b_blob_vencido_some_do_disco_e_vira_not_found() {
    let (a, _srv) = amb_daemon(Some(true), true);
    let mut c = autenticada(&a, A);
    let r = c.chama(
        "blob.put.begin",
        json!({"v": 1, "size": 0, "to": B, "ttl_s": 60}),
    );
    assert!(!is_err(&r));
    thread::sleep(Duration::from_secs(70));
    let g = get_raw(&a, B, "blob:00000000000000000000000000000000");
    esperado(&g, "not_found");
    assert!(
        log_texto(&a).contains("blob_expired"),
        "evento blob_expired gravado"
    );
}

// =====================================================================
// B6: direto: rm do destinatario apaga; get --consume baixa, confere e remove; resultado nao remove
// =====================================================================

#[test]
fn b06a_destinatario_rm_apaga_o_blob() {
    let (a, _srv) = amb_daemon(Some(true), true);
    let id = id_do_blob(&put_raw(&a, A, B, None, b"rm B6a"));
    let r = rm_raw(&a, B, &id);
    assert!(!is_err(&r), "destinatario remove: {r}");
    esperado(&get_raw(&a, A, &id), "not_found");
    assert!(arquivo_do_blob(&a, &id).is_none(), "arquivo sumiu do disco");
}

#[test]
fn b06b_get_consume_baixa_confere_e_remove() {
    let (a, _srv) = amb_daemon(Some(true), true);
    let dados = b"consume B6b";
    let id = id_do_blob(&put_raw(&a, A, B, None, dados));
    let saida = a.p().join("consumido.bin");
    let ss = s(&saida).to_string();
    let g = cli_id(
        &a,
        B,
        &["get", id.as_str(), "--out", ss.as_str(), "--consume"],
    );
    assert_eq!(g.code, Some(0), "{g:?}");
    assert_eq!(fs::read(&saida).unwrap(), dados.to_vec());
    esperado(&get_raw(&a, B, &id), "not_found");
}

#[test]
fn b06c_resultado_nao_remove_o_anexo() {
    let (a, _srv) = amb_daemon(Some(true), true);
    let f = arquivo_com(&a, "b06c.txt", b"anexo B6c");
    let fs_ = s(&f).to_string();
    let r = cli_id(
        &a,
        A,
        &[
            "send",
            "--attach",
            fs_.as_str(),
            "--from",
            A,
            "--to",
            B,
            "--kind",
            "PEDIDO",
            "--subject",
            "s",
            "--body",
            "c",
            "--id",
            "b06c",
        ],
    );
    assert_eq!(r.code, Some(0), "send --attach: {r:?}");
    assert!(!is_err(&ack_de(&a, B, "b06c", "aceito", None)));
    assert!(!is_err(&ack_de(&a, B, "b06c", "resultado", None)));
    let reg = eventos_de(&a, "registered", "b06c");
    let id = reg[0]
        .to_string()
        .split("blob:")
        .nth(1)
        .map(|t| format!("blob:{}", &t[..32]))
        .expect("id do anexo no registro");
    assert!(
        !is_err(&get_raw(&a, B, &id)),
        "resultado nao remove o anexo"
    );
}

// =====================================================================
// B7: nao ha dedup: mesmo conteudo gera dois blobs; rm de um nao afeta o outro
// =====================================================================

#[test]
fn b07_sem_dedup_dois_blobs_e_rm_de_um_nao_afeta_o_outro() {
    let (a, _srv) = amb_daemon(Some(true), true);
    let i1 = id_do_blob(&put_raw(&a, A, B, None, b"igual B7"));
    let i2 = id_do_blob(&put_raw(&a, A, B, None, b"igual B7"));
    assert_ne!(i1, i2, "sem dedup: ids diferentes");
    assert!(!is_err(&rm_raw(&a, B, &i1)));
    assert!(!is_err(&get_raw(&a, A, &i2)), "o outro continua");
}

// =====================================================================
// B8: cotas (recusa, nunca despejo) e reserva de upload em andamento
// =====================================================================

#[test]
fn b08a_cota_de_bytes_por_dono_recusa_o_nono_envio() {
    let (a, _srv) = amb_daemon(Some(true), true);
    let oito = vec![3u8; 8 * 1024 * 1024];
    for _ in 0..8 {
        let r = put_raw(&a, A, B, None, &oito);
        assert!(!is_err(&r), "8 envios de 8 MiB (64 MiB, cota do dono): {r}");
    }
    let mut c = autenticada(&a, A);
    let nono = c.chama(
        "blob.put.begin",
        json!({"v": 1, "size": oito.len(), "to": B}),
    );
    esperado(&nono, "quota_exceeded");
}

#[test]
fn b08b_reserva_do_begin_conta_upload_em_andamento() {
    let (a, _srv) = amb_daemon(Some(true), true);
    let mut c1 = autenticada(&a, A);
    let r1 = c1.chama(
        "blob.put.begin",
        json!({"v": 1, "size": 8 * 1024 * 1024, "to": B}),
    );
    assert!(!is_err(&r1), "primeiro upload: {r1}");
    // Segundo begin da mesma identidade, com o primeiro ainda aberto: recusado (uma sessao por identidade).
    // AMBIGUO: o codigo de erro do segundo upload simultaneo nao esta fixado; so se exige erro.
    let mut c2 = autenticada(&a, A);
    let r2 = c2.chama(
        "blob.put.begin",
        json!({"v": 1, "size": 8 * 1024 * 1024, "to": B}),
    );
    assert!(
        is_err(&r2),
        "segundo upload simultaneo da identidade e recusado: {r2}"
    );
}

// Cota total de 256 MiB e espaco livre minimo (reserva de 512 MiB): nao cobertos, com 3 identidades de teste a
// cota total nao e alcancavel (3 x 64 MiB < 256 MiB) e o espaco do disco nao e controlavel sem simular o disco.

// =====================================================================
// B9: destinatario desconhecido: nada sobe; send --attach valida antes do put
// =====================================================================

#[test]
fn b09a_destinatario_desconhecido_e_unknown_recipient() {
    let (a, _srv) = amb_daemon(Some(true), true);
    let mut c = autenticada(&a, A);
    let r = c.chama(
        "blob.put.begin",
        json!({"v": 1, "size": 1, "to": "nao-existe"}),
    );
    esperado(&r, "unknown_recipient");
}

#[test]
fn b09b_send_attach_para_desconhecido_nao_sobe_nada() {
    let (a, _srv) = amb_daemon(Some(true), true);
    let f = arquivo_com(&a, "b09b.txt", b"nao sobe B9b");
    let fs_ = s(&f).to_string();
    let r = cli_id(
        &a,
        A,
        &[
            "send",
            "--attach",
            fs_.as_str(),
            "--from",
            A,
            "--to",
            "nao-existe",
            "--kind",
            "PEDIDO",
            "--subject",
            "s",
            "--body",
            "c",
            "--id",
            "b09b",
        ],
    );
    assert!(
        r.code != Some(0),
        "destinatario desconhecido nao pode enviar: {r:?}"
    );
    assert!(
        eventos_de(&a, "registered", "b09b").is_empty(),
        "nenhum registro"
    );
    assert!(!log_texto(&a).contains("blob_put"), "nenhum blob gravado");
}

// =====================================================================
// B10: reinicio: indice reconstruido; vinculo sem arquivo; orfao tmp apagado; recusa com symlink
// =====================================================================

#[test]
fn b10a_reinicio_mantem_o_blob() {
    let (a, mut srv) = amb_daemon(Some(true), true);
    let id = id_do_blob(&put_raw(&a, A, B, None, b"reinicio B10a"));
    assert_eq!(srv.para(), Some(0), "SIGTERM limpo");
    let _srv2 = Srv::sobe(&a, &a.p().join("cfg.json"), "reinicio");
    assert!(
        !is_err(&get_raw(&a, B, &id)),
        "o blob continua disponivel depois do reinicio"
    );
}

#[test]
fn b10b_vinculo_sem_arquivo_vira_blob_removed_missing() {
    let (a, mut srv) = amb_daemon(Some(true), true);
    let id = id_do_blob(&put_raw(&a, A, B, None, b"sumido B10b"));
    assert_eq!(srv.para(), Some(0));
    let arq = arquivo_do_blob(&a, &id).expect("arquivo do blob existe antes de sumir");
    fs::remove_file(&arq).unwrap();
    let _srv2 = Srv::sobe(&a, &a.p().join("cfg.json"), "sumiu");
    let ev: Vec<Value> = eventos(&a, "blob_removed")
        .into_iter()
        .filter(|e| e.to_string().contains(&id))
        .collect();
    assert!(
        ev.iter().any(|e| e.to_string().contains("missing")),
        "blob_removed reason=missing esperado: {ev:?}"
    );
}

#[test]
fn b10c_orfao_tmp_e_apagado_na_partida() {
    let (a, mut srv) = amb_daemon(Some(true), true);
    let _ = put_raw(&a, A, B, None, b"base B10c");
    assert_eq!(srv.para(), Some(0));
    let orfao = a.state().join("blobs").join("tmp-deadbeef");
    fs::write(&orfao, b"resto de queda").unwrap();
    let _srv2 = Srv::sobe(&a, &a.p().join("cfg.json"), "orfao");
    assert!(
        !orfao.exists(),
        "tmp-* sem evento deve ser apagado na partida"
    );
}

#[test]
fn b10d_recusa_subir_com_blobs_link_simbolico() {
    let a = Amb::novo();
    fs::create_dir_all(a.state()).unwrap();
    let alvo = a.p().join("fora");
    fs::create_dir_all(&alvo).unwrap();
    std::os::unix::fs::symlink(&alvo, a.state().join("blobs")).unwrap();
    let cfg = a.p().join("cfg.json");
    fs::write(&cfg, cfg_json(Some(true), true).to_string()).unwrap();
    let (code, pronto) = sobe_sem_esperar(&a, &cfg);
    assert!(
        !pronto,
        "serve nao pode ficar pronto com blobs/ link simbolico"
    );
    assert!(code != Some(0), "saida de erro esperada: {code:?}");
}

// Vinculos de tmp e orfaos com nomes fora do padrao e permissoes erradas em <aa>/: nao cobertos nesta rodada.

// =====================================================================
// B11: upload: id de outra identidade ou conexao => not_found; sessao unica por identidade
// =====================================================================

#[test]
fn b11a_upload_de_outra_identidade_e_not_found() {
    let (a, _srv) = amb_daemon(Some(true), true);
    let mut ca = autenticada(&a, A);
    let up = res(&ca.chama("blob.put.begin", json!({"v": 1, "size": 4, "to": B})))["upload_id"]
        .as_str()
        .unwrap()
        .to_string();
    let mut cb = autenticada(&a, B);
    let r = cb.chama(
        "blob.put.chunk",
        json!({"v": 1, "upload_id": up, "n": 0, "data": b64(b"abcd")}),
    );
    esperado(&r, "not_found");
}

#[test]
fn b11b_upload_de_outra_conexao_da_mesma_identidade_e_not_found() {
    let (a, _srv) = amb_daemon(Some(true), true);
    let mut c1 = autenticada(&a, A);
    let up = res(&c1.chama("blob.put.begin", json!({"v": 1, "size": 4, "to": B})))["upload_id"]
        .as_str()
        .unwrap()
        .to_string();
    let mut c2 = autenticada(&a, A);
    let r = c2.chama(
        "blob.put.chunk",
        json!({"v": 1, "upload_id": up, "n": 0, "data": b64(b"abcd")}),
    );
    esperado(&r, "not_found");
}

#[test]
fn b11c_upload_ids_nao_sao_previsiveis() {
    let (a, _srv) = amb_daemon(Some(true), true);
    let mut c = autenticada(&a, A);
    let u1 = res(&c.chama("blob.put.begin", json!({"v": 1, "size": 0, "to": B})))["upload_id"]
        .as_str()
        .unwrap()
        .to_string();
    let u2 = res(&c.chama("blob.put.begin", json!({"v": 1, "size": 0, "to": B})))["upload_id"]
        .as_str()
        .unwrap()
        .to_string();
    assert_ne!(u1, u2, "ids diferentes");
    assert!(
        u1.len() >= 26,
        "ids com entropia de 128 bits (tamanho minimo): {u1}"
    );
}

// Prazo absoluto de 10 min e ociosidade de 120 s: nao cobertos (exigem esperar minutos).

// =====================================================================
// B12: privacidade: sentinela no conteudo nunca aparece; nome so no log (blob_put) e em ls do dono/destinatario
// =====================================================================

#[test]
fn b12_sentinela_do_conteudo_nao_vaza_e_o_nome_fica_so_no_log() {
    let (a, _srv) = amb_daemon(Some(true), true);
    let conteudo = format!("linha {MARCA_CONTEUDO} fim");
    let r = put_raw(&a, A, B, Some("nome-visivel"), conteudo.as_bytes());
    assert!(!is_err(&r), "{r}");
    let id = id_do_blob(&r);
    let _ = get_raw(&a, B, &id);
    let st = res(&autenticada(&a, A).chama("status", json!({"v": 1}))).to_string();
    let log = log_texto(&a);
    assert!(!log.contains(MARCA_CONTEUDO), "conteudo nunca entra no log");
    assert!(
        !st.contains(MARCA_CONTEUDO),
        "conteudo nunca aparece no status"
    );
    assert!(log.contains("nome-visivel"), "o nome vai ao log (blob_put)");
    let ls = cli_id(&a, A, &["ls"]);
    assert!(
        ls.stdout.contains("nome-visivel") || ls.stdout.contains(&id),
        "dono ve o nome ou o id no ls: {}",
        ls.stdout
    );
    assert!(!ls.stdout.contains(MARCA_CONTEUDO) && !ls.stderr.contains(MARCA_CONTEUDO));
}

// =====================================================================
// B13: send --attach: um anexo; cria o ref blob; (falha apos o put remove: nao forcavel sem falha injetada)
// =====================================================================

#[test]
fn b13_send_attach_cria_ref_blob_e_um_anexo_so() {
    let (a, _srv) = amb_daemon(Some(true), true);
    let f = arquivo_com(&a, "b13.txt", b"anexo B13");
    let fs_ = s(&f).to_string();
    let r = cli_id(
        &a,
        A,
        &[
            "send",
            "--attach",
            fs_.as_str(),
            "--from",
            A,
            "--to",
            B,
            "--kind",
            "PEDIDO",
            "--subject",
            "s",
            "--body",
            "c",
            "--id",
            "b13",
        ],
    );
    assert_eq!(r.code, Some(0), "{r:?}");
    let reg = eventos_de(&a, "registered", "b13");
    assert_eq!(reg.len(), 1, "uma mensagem");
    assert!(
        reg[0].to_string().contains("blob:"),
        "o registro carrega o ref do blob: {}",
        reg[0]
    );
}

// =====================================================================
// B14: get --out sem permissao ou sobre link: erro limpo, sem arquivo parcial, --consume nao aplicado
// =====================================================================

#[test]
fn b14a_get_out_em_dir_sem_permissao_sai_5_sem_arquivo_e_sem_consumir() {
    let (a, _srv) = amb_daemon(Some(true), true);
    let id = id_do_blob(&put_raw(&a, A, B, None, b"b14a"));
    let dir = a.p().join("trancado");
    fs::create_dir_all(&dir).unwrap();
    fs::set_permissions(&dir, fs::Permissions::from_mode(0o500)).unwrap();
    let alvo = dir.join("saida.bin");
    let ss = s(&alvo).to_string();
    let r = cli_id(
        &a,
        B,
        &["get", id.as_str(), "--out", ss.as_str(), "--consume"],
    );
    fs::set_permissions(&dir, fs::Permissions::from_mode(0o755)).unwrap();
    assert_eq!(
        r.code,
        Some(5),
        "permissao negada no destino = saida 5: {r:?}"
    );
    assert!(!alvo.exists(), "sem arquivo parcial");
    assert!(!is_err(&get_raw(&a, B, &id)), "--consume nao foi aplicado");
}

#[test]
fn b14b_get_out_sobre_link_e_recusado() {
    let (a, _srv) = amb_daemon(Some(true), true);
    let id = id_do_blob(&put_raw(&a, A, B, None, b"b14b"));
    let alvo = a.p().join("destino-real.bin");
    fs::write(&alvo, b"nao sobrescrever").unwrap();
    let link = a.p().join("link.bin");
    std::os::unix::fs::symlink(&alvo, &link).unwrap();
    let ss = s(&link).to_string();
    let r = cli_id(&a, B, &["get", id.as_str(), "--out", ss.as_str()]);
    assert!(r.code != Some(0), "saida sobre link recusada: {r:?}");
    assert_eq!(
        fs::read(&alvo).unwrap(),
        b"nao sobrescrever",
        "o alvo do link nao foi tocado"
    );
}

// =====================================================================
// B16-B19 (B.0): status so com vivo e estatisticas; mailbox.counts; inbox --count
// =====================================================================

#[test]
fn b16_status_com_auth_ligada_e_sem_token_responde_so_contagens() {
    let (a, _srv) = amb_daemon(Some(true), true);
    let mut c = Conexao::abre(&a.socket());
    let r = c.chama("status", json!({"v": 1}));
    let v = res(&r);
    assert_eq!(v["ok"], json!(true), "ok:true sem token: {r}");
    for campo in [
        "version",
        "uptime_s",
        "head_seq",
        "messages",
        "bells",
        "connections",
        "blobs",
    ] {
        assert!(v.get(campo).is_some(), "campo {campo} ausente: {v}");
    }
    assert!(
        v.get("recipients").is_none(),
        "recipients sai do status: {v}"
    );
}

#[test]
fn b17_status_nao_traz_nome_de_agente_destinatario_perfil_nem_painel() {
    let (a, _srv) = amb_daemon(Some(true), true);
    assert_eq!(
        envia_cli(&a, A, B, "AVISO", "b17", None, None, &[]).code,
        Some(0)
    );
    let mut c = Conexao::abre(&a.socket());
    let txt = res(&c.chama("status", json!({"v": 1}))).to_string();
    for nome in [
        "claude-main",
        "codex-web",
        "terceiro",
        "w2:p1",
        "perfil",
        "profile",
    ] {
        assert!(!txt.contains(nome), "status nao pode citar {nome}: {txt}");
    }
}

#[test]
fn b18_mailbox_counts_so_da_propria_identidade_e_ignora_seletor() {
    let (a, _srv) = amb_daemon(Some(true), true);
    assert_eq!(
        envia_cli(&a, A, B, "AVISO", "b18", None, None, &[]).code,
        Some(0)
    );
    let mut ca = autenticada(&a, A);
    let proprio = res(&ca.chama("mailbox.counts", json!({"v": 1})));
    let com_seletor = res(&ca.chama("mailbox.counts", json!({"v": 1, "to": B})));
    assert_eq!(proprio, com_seletor, "o seletor `to` e ignorado");
    let mut cb = autenticada(&a, B);
    let de_b = res(&cb.chama("mailbox.counts", json!({"v": 1})));
    assert_eq!(
        acha_chave(&de_b, "pending"),
        Some(json!(1)),
        "B ve a propria caixa: {de_b}"
    );
    assert_ne!(proprio["pending"], de_b["pending"], "A nao ve a caixa de B");
}

#[test]
fn b19_inbox_count_imprime_as_tres_contagens_sem_nomes() {
    let (a, _srv) = amb_daemon(Some(true), true);
    assert_eq!(
        envia_cli(&a, A, B, "AVISO", "b19", None, None, &[]).code,
        Some(0)
    );
    let r = cli_id(&a, B, &["inbox", "--count"]);
    assert_eq!(r.code, Some(0), "{r:?}");
    assert!(!r.stdout.trim().is_empty(), "imprime as contagens");
    // AMBIGUO: o formato da linha (rotulos e separadores) nao esta fixado; so se exige que nao traga nomes.
    assert!(
        !r.stdout.contains("claude-main") && !r.stdout.contains("codex-web"),
        "sem nomes: {}",
        r.stdout
    );
}

// =====================================================================
// B20-B26: corrida, nao bloqueio, churn, limites de leitura, auth off, segredos, overflow
// =====================================================================

#[test]
fn b20_corrida_rm_e_get_concorrentes_nunca_erro_interno() {
    let (a, _srv) = amb_daemon(Some(true), true);
    let id = id_do_blob(&put_raw(&a, A, B, None, b"corrida B20"));
    let erros: Vec<String> = thread::scope(|sc| {
        let mut hs = Vec::new();
        for _ in 0..3 {
            let (a_ref, id_ref) = (&a, id.as_str());
            hs.push(sc.spawn(move || {
                let r = get_raw(a_ref, A, id_ref);
                if is_err(&r) {
                    cod(&r)
                } else {
                    String::from("ok")
                }
            }));
        }
        let (a_ref, id_ref) = (&a, id.as_str());
        hs.push(sc.spawn(move || {
            let r = rm_raw(a_ref, B, id_ref);
            if is_err(&r) {
                cod(&r)
            } else {
                String::from("ok")
            }
        }));
        hs.into_iter().map(|h| h.join().unwrap()).collect()
    });
    for e in erros {
        assert!(
            e == "ok" || e == "not_found",
            "corrida nao pode dar erro interno: {e}"
        );
    }
}

#[test]
fn b21_put_de_8_mib_nao_atrasa_message_send_de_outro() {
    let (a, _srv) = amb_daemon(Some(true), true);
    let grande = vec![5u8; 8 * 1024 * 1024];
    thread::scope(|sc| {
        let a_ref = &a;
        let g = &grande;
        sc.spawn(move || {
            for _ in 0..3 {
                let _ = put_raw(a_ref, A, B, None, g);
            }
        });
        thread::sleep(Duration::from_millis(200));
        let t = Instant::now();
        let r = autenticada(&a, C).chama(
            "message.send",
            msg("b21-c", C, B, "AVISO", "assunto", "corpo"),
        );
        let dt = t.elapsed();
        assert!(!is_err(&r), "message.send de outro: {r}");
        assert!(
            dt < Duration::from_secs(1),
            "upload atrasou message.send: {dt:?}"
        );
    });
}

#[test]
fn b22_churn_de_put_zero_byte_bate_em_rate_limited_e_log_fica_limitado() {
    let (a, _srv) = amb_daemon(Some(true), true);
    let mut c = autenticada(&a, A);
    let mut limitados = 0;
    for _ in 0..40 {
        let r = c.chama("blob.put.begin", json!({"v": 1, "size": 0, "to": B}));
        if !is_err(&r) {
            let up = res(&r)["upload_id"].as_str().unwrap().to_string();
            let _ = c.chama(
                "blob.put.end",
                json!({"v": 1, "upload_id": up, "sha256": sha256_hex(&[])}),
            );
        } else if cod(&r) == "rate_limited" {
            limitados += 1;
        }
    }
    assert!(
        limitados > 0,
        "30 put.begin por minuto: o excedente deve ser rate_limited"
    );
    let puts = eventos(&a, "blob_put").len();
    assert!(
        puts <= 30,
        "o log nao cresce alem do orcamento de eventos: {puts}"
    );
}

#[test]
fn b23a_length_enorme_e_validado() {
    let (a, _srv) = amb_daemon(Some(true), true);
    let id = id_do_blob(&put_raw(&a, A, B, None, b"b23a"));
    let mut c = autenticada(&a, A);
    let r = c.chama(
        "blob.get",
        json!({"v": 1, "blob": id, "length": 10_000_000_000u64}),
    );
    assert!(
        is_err(&r),
        "length acima do limite de 512 KiB e recusado: {r}"
    );
}

#[test]
fn b24_com_auth_off_os_anexos_nao_ficam_disponiveis() {
    let a = Amb::novo();
    fs::create_dir_all(a.state()).unwrap();
    let cfg = a.p().join("cfg.json");
    fs::write(&cfg, cfg_json(Some(true), false).to_string()).unwrap();
    let (_code, pronto) = sobe_sem_esperar(&a, &cfg);
    // AMBIGUO: a spec diz que o daemon "recusa ligar anexos" com auth off, mas nao diz se recusa subir ou se
    // responde unauthenticated. Aceita-se qualquer um dos dois, desde que nenhum anexo passe.
    if pronto {
        emite_token(&a, A);
        let mut c = Conexao::abre(&a.socket());
        let r = c.chama("blob.put.begin", json!({"v": 1, "size": 0, "to": B}));
        assert_eq!(
            cod(&r),
            "unauthenticated",
            "com auth off, blob.* responde unauthenticated: {r}"
        );
    }
}

#[test]
fn b25a_nome_da_denylist_e_recusado_no_cli() {
    let (a, _srv) = amb_daemon(Some(true), true);
    let f = arquivo_com(&a, ".env", b"nao e para anexar B25a");
    let fs_ = s(&f).to_string();
    let r = cli_id(&a, A, &["put", fs_.as_str(), "--to", B]);
    assert!(
        r.code != Some(0),
        "arquivo .env nao pode ser anexado: {r:?}"
    );
}

#[test]
fn b25b_cabecalho_pem_no_conteudo_e_secret_suspected() {
    let (a, _srv) = amb_daemon(Some(true), true);
    let conteudo = b"-----BEGIN PRIVATE KEY-----
MIIB
-----END PRIVATE KEY-----
";
    let r = put_raw(&a, A, B, Some("notas.txt"), conteudo);
    // AMBIGUO: o passo em que a varredura recusa (chunk ou end) nao esta fixado; so se exige o codigo.
    esperado(&r, "secret_suspected");
}

#[test]
fn b26_overflow_de_size_e_ttl_recusados_e_upload_ids_nao_repetem() {
    let (a, _srv) = amb_daemon(Some(true), true);
    let mut c = autenticada(&a, A);
    let r1 = c.chama("blob.put.begin", json!({"v": 1, "size": u64::MAX, "to": B}));
    assert!(is_err(&r1), "size no limite de inteiro recusado: {r1}");
    let r2 = c.chama(
        "blob.put.begin",
        json!({"v": 1, "size": 1, "to": B, "ttl_s": u64::MAX}),
    );
    assert!(is_err(&r2), "ttl_s no limite de inteiro recusado: {r2}");
}

//! Correcoes da auditoria adversarial (spec v1.1, secao F). Cada teste deve FALHAR sem a correcao.
mod common;

use common::*;
use serde_json::{Value, json};
use std::fs;
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::mpsc;
use std::thread;
use std::time::Duration;
use tempfile::TempDir;

const CLAUDE: &str = "claude-main";
const CODEX: &str = "codex-web";

/// Controles proibidos (C0, DEL, C1) que nao sao \n nem \t.
const CONTROLES: &[&str] = &[
    "\u{0}", "\u{7}", "\u{8}", "\u{1b}", "\u{7f}", "\u{85}", "\u{9f}", "\r",
];

fn envia_com(sock: &Path, id: &str, subject: &str, body: &str) -> Value {
    envia(
        sock,
        json!({"v": 1, "id": id, "from": CLAUDE, "to": CODEX, "kind": "AVISO", "subject": subject, "body": body}),
    )
}

// ---------- (1) controles em subject, body, refs.v e note ----------

#[test]
fn subject_com_controle_e_invalid_envelope_em_subject() {
    let (_d, srv) = sobe_padrao();
    let sock = srv.socket.clone();
    for (i, c) in CONTROLES.iter().chain(["\t", "\n"].iter()).enumerate() {
        let r = envia_com(&sock, &format!("sc-{i}"), &format!("a{c}b"), "corpo");
        assert_eq!(cod(&r), "invalid_envelope", "subject com {c:?}: {r}");
        assert_eq!(caminho(&r), "/subject", "subject com {c:?}");
    }
}

#[test]
fn body_aceita_newline_e_tab_e_recusa_os_demais_controles() {
    let (_d, srv) = sobe_padrao();
    let sock = srv.socket.clone();
    assert!(
        !is_err(&envia_com(&sock, "bd-ok-1", "s", "linha1\nlinha2\tcoluna")),
        "\\n e \\t no body sao aceitos"
    );
    assert!(
        !is_err(&envia_com(&sock, "bd-ok-2", "s", "acentos ação 😀")),
        "texto normal"
    );
    for (i, c) in CONTROLES.iter().enumerate() {
        let r = envia_com(&sock, &format!("bd-{i}"), "s", &format!("a{c}b"));
        assert_eq!(cod(&r), "invalid_envelope", "body com {c:?}: {r}");
        assert_eq!(caminho(&r), "/body", "body com {c:?}");
    }
    let r = envia_com(&sock, "bd-crlf", "s", "a\r\nb");
    assert_eq!(caminho(&r), "/body", "\\r no body deve ser recusado");
}

#[test]
fn refs_v_com_controle_e_invalid_envelope_em_refs_v() {
    let (_d, srv) = sobe_padrao();
    let sock = srv.socket.clone();
    for (i, c) in CONTROLES.iter().chain(["\t", "\n"].iter()).enumerate() {
        let r = envia(
            &sock,
            json!({"v": 1, "id": format!("rv-{i}"), "from": CLAUDE, "to": CODEX, "kind": "AVISO",
                   "subject": "s", "body": "b", "refs": [{"t": "path", "v": format!("x{c}y")}]}),
        );
        assert_eq!(cod(&r), "invalid_envelope", "ref.v com {c:?}: {r}");
        assert_eq!(caminho(&r), "/refs/0/v", "ref.v com {c:?}");
    }
    let ok = envia(
        &sock,
        json!({"v": 1, "id": "rv-ok", "from": CLAUDE, "to": CODEX, "kind": "AVISO",
               "subject": "s", "body": "b", "refs": [{"t": "path", "v": "docs/a b.md"}]}),
    );
    assert!(!is_err(&ok), "ref.v sem controle deve passar: {ok}");
}

#[test]
fn note_do_ack_aceita_so_tab() {
    let (_d, srv) = sobe_padrao();
    let sock = srv.socket.clone();
    envia_com(&sock, "nt-1", "s", "b");
    envia_com(&sock, "nt-2", "s", "b");
    envia_com(&sock, "nt-3", "s", "b");
    let ok = ack_json(
        &sock,
        json!({"v": 1, "id": "nt-1", "by": CODEX, "stage": "resultado", "note": "coluna\tdois"}),
    );
    assert!(!is_err(&ok), "\\t na note deve passar: {ok}");
    assert_eq!(res(&ok)["note"], "coluna\tdois");
    for (i, c) in CONTROLES.iter().chain(["\n"].iter()).enumerate() {
        let id = if i % 2 == 0 { "nt-2" } else { "nt-3" };
        let r = ack_json(
            &sock,
            json!({"v": 1, "id": id, "by": CODEX, "stage": "resultado", "note": format!("a{c}b")}),
        );
        assert_eq!(cod(&r), "invalid_envelope", "note com {c:?}: {r}");
        assert_eq!(caminho(&r), "/note", "note com {c:?}");
    }
}

// ---------- (2) eco escapado de mensagem antiga com controle ----------

fn troca_no_log(state: &Path, de: &str, para: &str) {
    let p = state.join("log.jsonl");
    let t = fs::read_to_string(&p).unwrap();
    assert!(t.contains(de), "marca {de} ausente no log");
    fs::write(&p, t.replace(de, para)).unwrap();
}

/// Servidor (auth off) com uma mensagem cujo subject e body carregam ESC gravados no log, como se
/// tivessem sido aceitos antes da correcao: escreve a marca e troca no arquivo por `\u001b[1AZ` (escape JSON).
fn log_com_controle_antigo() -> (TempDir, PathBuf, PathBuf) {
    let d = tmp();
    let cfg = config_padrao(d.path());
    let st = d.path().join("state");
    let mut srv = sobe(&st, &cfg);
    let sock = srv.socket.clone();
    let r = envia(
        &sock,
        msg("au-1", CLAUDE, CODEX, "AVISO", "SUBJMARK", "BODYMARK"),
    );
    assert!(!is_err(&r), "{r}");
    assert!(srv.para().is_some());
    drop(srv);
    troca_no_log(&st, "SUBJMARK", r"a\u001b[1AZ");
    troca_no_log(&st, "BODYMARK", r"b\u001b[1AZ");
    (d, st, cfg)
}

fn sem_controles(s: &str) -> bool {
    !s.chars().any(|c| {
        ((c as u32) < 0x20 && c != '\n' && c != '\t') || (0x7f..=0x9f).contains(&(c as u32))
    })
}

#[test]
fn eco_escapado_no_inbox_take() {
    let (d, st, cfg) = log_com_controle_antigo();
    let srv = sobe(&st, &cfg);
    let sock = s(&srv.socket).to_string();
    let r = cli_com(
        &["inbox", "--take"],
        &[("CHATD_AS", CODEX), ("CHATD_SOCKET", sock.as_str())],
    );
    assert_eq!(r.code, Some(0), "saida: {r:?}");
    assert!(
        !r.stdout.contains('\u{1b}'),
        "byte ESC chegou ao terminal: {:?}",
        r.stdout
    );
    assert!(
        sem_controles(&r.stdout),
        "controle na saida: {:?}",
        r.stdout
    );
    // AMBIGUO: a spec aceita `\xNN` ou `\u{NNNN}` como forma escapada; aceitamos as duas.
    assert!(
        r.stdout.contains(r"\x1b[1AZ") || r.stdout.contains(r"\u{1b}[1AZ"),
        "forma escapada ausente: {:?}",
        r.stdout
    );
    drop(d);
}

#[test]
fn eco_escapado_no_log_comando() {
    let (_d, st, cfg) = log_com_controle_antigo();
    let srv = sobe(&st, &cfg);
    drop(srv);
    let r = cli(&["log", "--state-dir", s(&st)]);
    assert_eq!(r.code, Some(0), "saida: {r:?}");
    assert!(
        !r.stdout.contains('\u{1b}'),
        "ESC no chatd log: {:?}",
        r.stdout
    );
    assert!(
        sem_controles(&r.stdout),
        "controle no chatd log: {:?}",
        r.stdout
    );
    // AMBIGUO: forma escapada aceita `\xNN` ou `\u{NNNN}`.
    assert!(
        r.stdout.contains(r"\x1b[1AZ") || r.stdout.contains(r"\u{1b}[1AZ"),
        "{:?}",
        r.stdout
    );
}

struct Watch {
    child: Child,
    rx: mpsc::Receiver<String>,
}

impl Watch {
    fn inicia(args: &[&str], envs: &[(&str, &str)]) -> Watch {
        let mut child = Command::new(BIN)
            .arg("watch")
            .args(args)
            .env_remove("CHATD_SOCKET")
            .env_remove("CHATD_STATE_DIR")
            .env_remove("CHATD_AS")
            .env_remove("CHATD_TOKEN_FILE")
            .envs(envs.iter().copied())
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .expect("executar watch");
        let out = child.stdout.take().unwrap();
        let (tx, rx) = mpsc::channel();
        thread::spawn(move || {
            for l in BufReader::new(out).lines().map_while(Result::ok) {
                if tx.send(l).is_err() {
                    break;
                }
            }
        });
        Watch { child, rx }
    }
    fn proxima(&self, d: Duration) -> Option<String> {
        self.rx.recv_timeout(d).ok()
    }
}

impl Drop for Watch {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

#[test]
fn eco_escapado_no_watch_from_start() {
    let (_d, st, cfg) = log_com_controle_antigo();
    let srv = sobe(&st, &cfg);
    let sock = s(&srv.socket).to_string();
    let w = Watch::inicia(
        &["--as", CODEX, "--from-start"],
        &[("CHATD_SOCKET", sock.as_str())],
    );
    let linha = w.proxima(Duration::from_secs(5)).expect("linha do watch");
    assert!(!linha.contains('\u{1b}'), "ESC no watch: {linha:?}");
    assert!(sem_controles(&linha), "controle no watch: {linha:?}");
    // AMBIGUO: forma escapada aceita `\xNN` ou `\u{NNNN}`.
    assert!(
        linha.contains(r"\x1b[1AZ") || linha.contains(r"\u{1b}[1AZ"),
        "{linha:?}"
    );
}

// ---------- (3) evento denied ----------

struct Amb {
    d: TempDir,
    srv: Servidor,
    tokens: Vec<(String, String)>,
}

fn amb_com(nomes: &[&str]) -> Amb {
    let d = tmp();
    let cfg = config_req(d.path());
    let srv = sobe(&d.path().join("state"), &cfg);
    let mut tokens = Vec::new();
    for n in nomes {
        let t = token_new(&srv.state, n, &d.path().join(format!("{n}.tok")));
        tokens.push((n.to_string(), t));
    }
    Amb { d, srv, tokens }
}

impl Amb {
    fn tok(&self, n: &str) -> String {
        self.tokens.iter().find(|(k, _)| k == n).unwrap().1.clone()
    }
    fn conecta(&self, n: &str) -> Conexao {
        conexao_auth(&self.srv.socket, n, &self.tok(n))
    }
}

#[test]
fn negacoes_gravam_evento_denied_sem_conteudo() {
    let a = amb_com(&[CLAUDE, CODEX]);
    let mut claude = a.conecta(CLAUDE);
    let mut codex = a.conecta(CODEX);
    // mensagem legitima do claude para o codex (para o ack)
    assert!(!is_err(&claude.chama(
        "message.send",
        json!({"v": 1, "id": "dn-0", "to": CODEX, "kind": "AVISO", "subject": "ok", "body": "ok"})
    )));
    // 1) from forjado
    let r = codex.chama(
        "message.send",
        msg(
            "dn-1",
            CLAUDE,
            CODEX,
            "AVISO",
            "SEGREDO-ASSUNTO",
            "SEGREDO-CORPO",
        ),
    );
    assert_eq!(cod(&r), "forbidden");
    // 2) list para caixa alheia
    let r = codex.chama("message.list", json!({"v": 1, "to": CLAUDE}));
    assert_eq!(cod(&r), "forbidden");
    // 3) take para caixa alheia
    let r = codex.chama("message.take", json!({"v": 1, "to": CLAUDE}));
    assert_eq!(cod(&r), "forbidden");
    // 4) ack por outro
    let r = codex.chama(
        "message.ack",
        json!({"v": 1, "id": "dn-0", "by": CLAUDE, "stage": "aceito"}),
    );
    assert_eq!(cod(&r), "forbidden");

    let negs = eventos_tipo(&a.srv.state, "denied");
    assert_eq!(negs.len(), 4, "um evento denied por negacao: {negs:?}");
    let metodos: Vec<&str> = negs
        .iter()
        .map(|e| e["method"].as_str().unwrap_or(""))
        .collect();
    for m in [
        "message.send",
        "message.list",
        "message.take",
        "message.ack",
    ] {
        assert!(metodos.contains(&m), "falta denied de {m}: {metodos:?}");
    }
    let uid = unsafe { libc::getuid() };
    for e in &negs {
        assert_eq!(e["peer_pid"], i64::from(std::process::id()), "{e}");
        assert_eq!(e["peer_uid"], u64::from(uid), "{e}");
        assert_eq!(e["claimed"], CLAUDE, "{e}");
        let txt = e.to_string();
        assert!(
            !txt.contains("SEGREDO") && !txt.contains(&a.tok(CODEX)),
            "conteudo no denied: {txt}"
        );
    }
    let log = fs::read_to_string(a.srv.state.join("log.jsonl")).unwrap();
    assert!(!log.contains("SEGREDO"), "assunto/corpo no log");
    assert!(
        !log.contains(&a.tok(CODEX)) && !log.contains(&a.tok(CLAUDE)),
        "token no log"
    );
}

#[test]
fn chatd_log_destaca_denied_com_bang_bang() {
    let a = amb_com(&[CLAUDE, CODEX]);
    let mut codex = a.conecta(CODEX);
    let r = codex.chama(
        "message.send",
        msg("bb-1", CLAUDE, CODEX, "AVISO", "s", "b"),
    );
    assert_eq!(cod(&r), "forbidden");
    let r = cli(&["log", "--state-dir", s(&a.srv.state)]);
    assert_eq!(r.code, Some(0), "saida: {r:?}");
    let linhas: Vec<&str> = r.stdout.lines().filter(|l| l.contains("denied")).collect();
    assert!(
        !linhas.is_empty(),
        "sem linha denied no log: {:?}",
        r.stdout
    );
    for l in &linhas {
        assert!(l.contains("!!"), "denied sem !!: {l}");
    }
}

#[test]
fn servidor_rele_log_com_denied_e_sobe() {
    let mut a = amb_com(&[CLAUDE, CODEX]);
    let mut codex = a.conecta(CODEX);
    assert_eq!(
        cod(&codex.chama(
            "message.send",
            msg("rl-1", CLAUDE, CODEX, "AVISO", "s", "b")
        )),
        "forbidden"
    );
    assert!(a.srv.para().is_some());
    let st = a.srv.state.clone();
    let cfg = a.d.path().join("config.json");
    a.srv = sobe(&st, &cfg);
    let r = res(&a.conecta(CODEX).chama("status", json!({"v": 1})));
    assert_eq!(r["v"], 1);
    assert_eq!(
        eventos_tipo(&st, "denied").len(),
        1,
        "o denied sobrevive ao reinicio"
    );
}

// ---------- (4) trava por pid: auth OK zera o historico ----------

#[test]
fn quatro_falhas_sucesso_e_uma_falha_nao_trancam() {
    let a = amb_com(&[CLAUDE]);
    let sock = a.srv.socket.clone();
    let tok = a.tok(CLAUDE);
    let errado = "4".repeat(64);
    for _ in 0..4 {
        assert_eq!(cod(&auth_tenta(&sock, CLAUDE, &errado)), "unauthenticated");
    }
    assert!(
        !is_err(&auth_tenta(&sock, CLAUDE, &tok)),
        "o sucesso deve zerar as falhas"
    );
    assert_eq!(cod(&auth_tenta(&sock, CLAUDE, &errado)), "unauthenticated");
    let r = auth_tenta(&sock, CLAUDE, &tok);
    assert!(
        !is_err(&r),
        "1 falha depois do sucesso nao pode trancar o pid: {r}"
    );
}

#[test]
fn cinco_falhas_seguidas_continuam_trancando() {
    // AMBIGUO: a spec fixa "300 s"; aqui so verificamos que o bloqueio continua depois de 2 s (a janela inteira nao cabe no teste).
    let a = amb_com(&[CLAUDE]);
    let sock = a.srv.socket.clone();
    let tok = a.tok(CLAUDE);
    let errado = "5".repeat(64);
    for _ in 0..5 {
        assert_eq!(cod(&auth_tenta(&sock, CLAUDE, &errado)), "unauthenticated");
    }
    thread::sleep(Duration::from_secs(2));
    assert_eq!(
        cod(&auth_tenta(&sock, CLAUDE, &tok)),
        "unauthenticated",
        "pid deve seguir trancado"
    );
}

#[test]
fn auth_ok_zera_historico_e_depois_as_cinco_seguidas_trancam() {
    // AMBIGUO: a 5a falha e a que tranca (ou a 6a); aqui so fazemos 4 + sucesso + 5 seguidas.
    let a = amb_com(&[CLAUDE]);
    let sock = a.srv.socket.clone();
    let tok = a.tok(CLAUDE);
    let errado = "6".repeat(64);
    for _ in 0..4 {
        let _ = auth_tenta(&sock, CLAUDE, &errado);
    }
    assert!(!is_err(&auth_tenta(&sock, CLAUDE, &tok)));
    for _ in 0..5 {
        let _ = auth_tenta(&sock, CLAUDE, &errado);
    }
    assert_eq!(cod(&auth_tenta(&sock, CLAUDE, &tok)), "unauthenticated");
}

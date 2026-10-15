//! v1.1 secao D: `chatd watch` (linha por mensagem dirigida a identidade, reconexao sem duplicata,
//! keepalive, saida 0 em SIGTERM) e `chatd log --tail`. Contrato: docs/spec-v1.1-agentes.md.
mod common;

use common::*;
use serde_json::{Value, json};
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};
use tempfile::TempDir;

const CLAUDE: &str = "claude-main";
const CODEX: &str = "codex-web";

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
    fn tok(&self, nome: &str) -> String {
        self.tokens
            .iter()
            .find(|(k, _)| k == nome)
            .unwrap()
            .1
            .clone()
    }
    fn conecta(&self, nome: &str) -> Conexao {
        conexao_auth(&self.srv.socket, nome, &self.tok(nome))
    }
    fn tokfile(&self, nome: &str) -> PathBuf {
        self.d.path().join(format!("{nome}.tok"))
    }
    fn envs_de(&self, nome: &str) -> (String, String, String) {
        (
            nome.to_string(),
            s(&self.tokfile(nome)).to_string(),
            s(&self.srv.socket).to_string(),
        )
    }
}

/// Processo `chatd watch` com leitor de stdout em thread.
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
            use std::io::{BufRead, BufReader};
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

    /// Todas as linhas que chegarem dentro de `d`.
    fn linhas_por(&self, d: Duration) -> Vec<String> {
        let fim = Instant::now() + d;
        let mut v = Vec::new();
        while Instant::now() < fim {
            match self.rx.recv_timeout(fim - Instant::now()) {
                Ok(l) => v.push(l),
                Err(_) => break,
            }
        }
        v
    }

    /// Espera o processo sair sozinho (ate 5 s).
    fn espera_saida(&mut self) -> Option<i32> {
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

    fn sinal(&mut self, sig: libc::c_int) -> Option<i32> {
        unsafe { libc::kill(self.child.id() as libc::pid_t, sig) };
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

impl Drop for Watch {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn envia_de(c: &mut Conexao, id: &str, para: &str, assunto: &str) -> Value {
    c.chama(
        "message.send",
        json!({"v": 1, "id": id, "to": para, "kind": "AVISO", "subject": assunto, "body": "corpo"}),
    )
}

// ---------- watch ----------

#[test]
fn watch_imprime_uma_linha_por_mensagem_dirigida_a_identidade() {
    let a = amb_com(&[CLAUDE, CODEX]);
    let (n, tf, sk) = a.envs_de(CLAUDE);
    let w = Watch::inicia(
        &["--as", n.as_str()],
        &[
            ("CHATD_TOKEN_FILE", tf.as_str()),
            ("CHATD_SOCKET", sk.as_str()),
        ],
    );
    thread::sleep(Duration::from_millis(700));
    let mut codex = a.conecta(CODEX);
    assert!(!is_err(&envia_de(&mut codex, "wa-1", CLAUDE, "assunto")));
    let linha = w.proxima(Duration::from_secs(5)).expect("linha do watch");
    assert_eq!(linha, "1 wa-1 AVISO codex-web>claude-main | assunto");
}

#[test]
fn watch_nao_imprime_mensagens_de_outro_destinatario() {
    let a = amb_com(&[CLAUDE, CODEX]);
    let (n, tf, sk) = a.envs_de(CLAUDE);
    let w = Watch::inicia(
        &["--as", n.as_str()],
        &[
            ("CHATD_TOKEN_FILE", tf.as_str()),
            ("CHATD_SOCKET", sk.as_str()),
        ],
    );
    thread::sleep(Duration::from_millis(700));
    // claude -> codex: a identidade e o remetente, nao o destinatario
    let mut claude = a.conecta(CLAUDE);
    assert!(!is_err(&envia_de(
        &mut claude,
        "wb-1",
        CODEX,
        "nao e para mim"
    )));
    let mut codex = a.conecta(CODEX);
    assert!(!is_err(&envia_de(&mut codex, "wb-2", CLAUDE, "para mim")));
    let primeira = w.proxima(Duration::from_secs(5)).expect("linha");
    assert!(
        primeira.contains(" wb-2 "),
        "watch imprimiu mensagem alheia: {primeira}"
    );
    assert!(w.linhas_por(Duration::from_millis(500)).is_empty());
}

#[test]
fn watch_from_start_imprime_as_existentes() {
    let a = amb_com(&[CLAUDE, CODEX]);
    let mut codex = a.conecta(CODEX);
    assert!(!is_err(&envia_de(&mut codex, "fs-1", CLAUDE, "um")));
    assert!(!is_err(&envia_de(&mut codex, "fs-2", CLAUDE, "dois")));
    let (n, tf, sk) = a.envs_de(CLAUDE);
    let w = Watch::inicia(
        &["--as", n.as_str(), "--from-start"],
        &[
            ("CHATD_TOKEN_FILE", tf.as_str()),
            ("CHATD_SOCKET", sk.as_str()),
        ],
    );
    assert_eq!(
        w.proxima(Duration::from_secs(5)).unwrap(),
        "1 fs-1 AVISO codex-web>claude-main | um"
    );
    assert_eq!(
        w.proxima(Duration::from_secs(5)).unwrap(),
        "2 fs-2 AVISO codex-web>claude-main | dois"
    );
}

#[test]
fn watch_padrao_comeca_do_head_atual() {
    let a = amb_com(&[CLAUDE, CODEX]);
    let mut codex = a.conecta(CODEX);
    assert!(!is_err(&envia_de(&mut codex, "hd-1", CLAUDE, "antiga")));
    let (n, tf, sk) = a.envs_de(CLAUDE);
    let w = Watch::inicia(
        &["--as", n.as_str()],
        &[
            ("CHATD_TOKEN_FILE", tf.as_str()),
            ("CHATD_SOCKET", sk.as_str()),
        ],
    );
    thread::sleep(Duration::from_millis(700));
    assert!(!is_err(&envia_de(&mut codex, "hd-2", CLAUDE, "nova")));
    let primeira = w.proxima(Duration::from_secs(5)).expect("linha");
    assert!(
        primeira.contains(" hd-2 "),
        "a mensagem antiga nao deveria sair: {primeira}"
    );
}

#[test]
fn watch_usa_chatd_as_quando_nao_ha_flag() {
    let a = amb_com(&[CLAUDE, CODEX]);
    let (n, tf, sk) = a.envs_de(CLAUDE);
    let w = Watch::inicia(
        &[],
        &[
            ("CHATD_AS", n.as_str()),
            ("CHATD_TOKEN_FILE", tf.as_str()),
            ("CHATD_SOCKET", sk.as_str()),
        ],
    );
    thread::sleep(Duration::from_millis(700));
    let mut codex = a.conecta(CODEX);
    assert!(!is_err(&envia_de(&mut codex, "ca-w", CLAUDE, "env")));
    let linha = w.proxima(Duration::from_secs(5)).expect("linha");
    assert!(linha.contains(" ca-w "), "{linha}");
}

#[test]
fn watch_sem_token_com_auth_required_sai_2() {
    let a = amb_com(&[CLAUDE]);
    let (n, _tf, sk) = a.envs_de(CLAUDE);
    let mut w = Watch::inicia(&["--as", n.as_str()], &[("CHATD_SOCKET", sk.as_str())]);
    assert_eq!(
        w.espera_saida(),
        Some(2),
        "sem token o watch deve sair com 2"
    );
}

#[test]
fn watch_mantem_a_conexao_viva_com_keepalive() {
    // o servidor derruba conexao ociosa em 30 s; o watch envia status a cada 15 s
    let a = amb_com(&[CLAUDE, CODEX]);
    let (n, tf, sk) = a.envs_de(CLAUDE);
    let w = Watch::inicia(
        &["--as", n.as_str()],
        &[
            ("CHATD_TOKEN_FILE", tf.as_str()),
            ("CHATD_SOCKET", sk.as_str()),
        ],
    );
    thread::sleep(Duration::from_secs(35));
    let mut codex = a.conecta(CODEX);
    assert!(!is_err(&envia_de(
        &mut codex,
        "ka-1",
        CLAUDE,
        "depois de 35 s"
    )));
    let linha = w
        .proxima(Duration::from_secs(5))
        .expect("keepalive falhou: sem linha depois de 35 s");
    assert!(linha.contains(" ka-1 "), "{linha}");
}

#[test]
fn watch_reconecta_sem_imprimir_duplicata() {
    // AMBIGUO: a spec manda reconectar a cada 1 s com o ultimo seq visto; o tempo real de reconexao nao e medido aqui (so o resultado: cada mensagem uma vez).
    let mut a = amb_com(&[CLAUDE, CODEX]);
    let (n, tf, sk) = a.envs_de(CLAUDE);
    let w = Watch::inicia(
        &["--as", n.as_str()],
        &[
            ("CHATD_TOKEN_FILE", tf.as_str()),
            ("CHATD_SOCKET", sk.as_str()),
        ],
    );
    thread::sleep(Duration::from_millis(700));
    let mut codex = a.conecta(CODEX);
    assert!(!is_err(&envia_de(&mut codex, "wr-1", CLAUDE, "antes")));
    let l1 = w.proxima(Duration::from_secs(5)).expect("primeira linha");
    assert!(l1.contains(" wr-1 "), "{l1}");

    // derruba o servidor e sobe de novo no mesmo diretorio (mesmo socket, mesmos tokens)
    let st = a.srv.state.clone();
    let cfg = a.d.path().join("config.json");
    assert!(a.srv.para().is_some());
    thread::sleep(Duration::from_millis(1500));
    a.srv = sobe(&st, &cfg);

    let mut codex2 = a.conecta(CODEX);
    assert!(!is_err(&envia_de(&mut codex2, "wr-2", CLAUDE, "depois")));
    let resto = w.linhas_por(Duration::from_secs(6));
    let todas: Vec<&String> = std::iter::once(&l1).chain(resto.iter()).collect();
    let com_wr1 = todas.iter().filter(|l| l.contains(" wr-1 ")).count();
    let com_wr2 = todas.iter().filter(|l| l.contains(" wr-2 ")).count();
    assert_eq!(com_wr1, 1, "wr-1 impressa mais de uma vez: {todas:?}");
    assert_eq!(com_wr2, 1, "wr-2 nao chegou depois da reconexao: {todas:?}");
}

#[test]
fn watch_sai_0_em_sigterm() {
    let a = amb_com(&[CLAUDE]);
    let (n, tf, sk) = a.envs_de(CLAUDE);
    let mut w = Watch::inicia(
        &["--as", n.as_str()],
        &[
            ("CHATD_TOKEN_FILE", tf.as_str()),
            ("CHATD_SOCKET", sk.as_str()),
        ],
    );
    thread::sleep(Duration::from_millis(500));
    assert_eq!(
        w.sinal(libc::SIGTERM),
        Some(0),
        "SIGTERM deve encerrar com 0"
    );
}

#[test]
fn watch_sai_0_em_sigint() {
    let a = amb_com(&[CLAUDE]);
    let (n, tf, sk) = a.envs_de(CLAUDE);
    let mut w = Watch::inicia(
        &["--as", n.as_str()],
        &[
            ("CHATD_TOKEN_FILE", tf.as_str()),
            ("CHATD_SOCKET", sk.as_str()),
        ],
    );
    thread::sleep(Duration::from_millis(500));
    assert_eq!(w.sinal(libc::SIGINT), Some(0), "SIGINT deve encerrar com 0");
}

// ---------- log ----------

#[test]
fn log_tail_imprime_so_as_ultimas_linhas() {
    let (_d, mut srv) = sobe_padrao();
    let sock = srv.socket.clone();
    envia_ok(&sock, "log-1", "codex-web", "a");
    envia_ok(&sock, "log-2", "codex-web", "b");
    envia_ok(&sock, "log-3", "codex-web", "c");
    ack_ok(&sock, "log-2", "codex-web", "aceito");
    let r = cli(&["log", "--state-dir", s(&srv.state), "--tail", "2"]);
    assert_eq!(r.code, Some(0), "saida: {r:?}");
    let linhas: Vec<&str> = r.stdout.lines().collect();
    assert_eq!(linhas.len(), 2, "tail 2: {:?}", r.stdout);
    assert_eq!(
        linhas[0].split_whitespace().next(),
        Some("3"),
        "{:?}",
        r.stdout
    );
    assert_eq!(
        linhas[1].split_whitespace().next(),
        Some("4"),
        "{:?}",
        r.stdout
    );
    assert!(
        linhas[1].contains("accepted") && linhas[1].contains("log-2"),
        "{:?}",
        linhas[1]
    );
    assert!(srv.para().is_some());
}

#[test]
fn log_sem_tail_imprime_um_evento_por_linha() {
    let (_d, srv) = sobe_padrao();
    envia_ok(&srv.socket, "lt-1", "codex-web", "a");
    envia_ok(&srv.socket, "lt-2", "codex-web", "b");
    let head = status(&srv.socket)["head_seq"].as_i64().unwrap();
    let r = cli(&["log", "--state-dir", s(&srv.state)]);
    assert_eq!(r.code, Some(0), "saida: {r:?}");
    assert_eq!(r.stdout.lines().count() as i64, head, "{:?}", r.stdout);
}

#[test]
fn log_le_o_arquivo_sem_usar_o_socket() {
    let (_d, mut srv) = sobe_padrao();
    envia_ok(&srv.socket, "ls-1", "codex-web", "a");
    assert!(srv.para().is_some());
    let st = srv.state.clone();
    drop(srv);
    let r = cli(&["log", "--state-dir", s(&st)]);
    assert_eq!(
        r.code,
        Some(0),
        "log deve funcionar com o servidor parado: {r:?}"
    );
    assert!(r.stdout.contains("ls-1"), "{:?}", r.stdout);
}

#[test]
fn log_nao_imprime_token() {
    let a = amb_com(&[CLAUDE]);
    let tok = a.tok(CLAUDE);
    let _ = auth_tenta(&a.srv.socket, CLAUDE, &tok);
    let r = cli(&["log", "--state-dir", s(&a.srv.state)]);
    assert!(!r.stdout.contains(&tok), "token impresso pelo log");
}

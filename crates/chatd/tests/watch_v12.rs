//! v1.2 (docs/spec-v1.2-watch.md): `chatd watch` com --resume-file, --exit-after, --heartbeat-file e --reconnect.
//! Testes de integracao: so o binario e o socket. Devem falhar ate a implementacao chegar.
mod common;

use common::*;
use serde_json::json;
use std::fs;
use std::io::{BufRead, BufReader};
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, Mutex, mpsc};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use tempfile::TempDir;

const CODEX: &str = "codex-web";
const CLAUDE: &str = "claude-main";

/// `chatd watch` com stdout e stderr lidos por threads.
struct Watch {
    child: Child,
    out: mpsc::Receiver<String>,
    err: Arc<Mutex<String>>,
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
            .stderr(Stdio::piped())
            .spawn()
            .expect("executar watch");
        let out = child.stdout.take().unwrap();
        let errp = child.stderr.take().unwrap();
        let (tx, rx) = mpsc::channel();
        thread::spawn(move || {
            for l in BufReader::new(out).lines().map_while(Result::ok) {
                if tx.send(l).is_err() {
                    break;
                }
            }
        });
        let err = Arc::new(Mutex::new(String::new()));
        let e2 = Arc::clone(&err);
        thread::spawn(move || {
            for l in BufReader::new(errp).lines().map_while(Result::ok) {
                let mut g = e2.lock().unwrap();
                g.push_str(&l);
                g.push('\n');
            }
        });
        Watch {
            child,
            out: rx,
            err,
        }
    }

    fn proxima(&self, d: Duration) -> Option<String> {
        self.out.recv_timeout(d).ok()
    }

    /// Todas as linhas de stdout que chegarem dentro de `d`.
    fn linhas_por(&self, d: Duration) -> Vec<String> {
        let fim = Instant::now() + d;
        let mut v = Vec::new();
        while Instant::now() < fim {
            match self.out.recv_timeout(fim - Instant::now()) {
                Ok(l) => v.push(l),
                Err(_) => break,
            }
        }
        v
    }

    fn err(&self) -> String {
        self.err.lock().unwrap().clone()
    }

    fn vivo(&mut self) -> bool {
        matches!(self.child.try_wait(), Ok(None))
    }

    fn espera_saida(&mut self, d: Duration) -> Option<i32> {
        let fim = Instant::now() + d;
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

fn le_arq(p: &Path) -> Option<String> {
    fs::read_to_string(p).ok().map(|s| s.trim().to_string())
}

fn seq_da_linha(l: &str) -> i64 {
    l.split_whitespace()
        .next()
        .expect("seq na linha")
        .parse()
        .expect("seq numerico")
}

fn agora() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs()
}

fn envs_sock(sock: &Path) -> String {
    s(sock).to_string()
}

/// Auth obrigatoria com um token por nome; devolve tambem o arquivo do token.
struct AmbAuth {
    d: TempDir,
    srv: Servidor,
    token: String,
    tokfile: PathBuf,
}

fn amb_auth(nome: &str) -> AmbAuth {
    let d = tmp();
    let cfg = config_req(d.path());
    let srv = sobe(&d.path().join("state"), &cfg);
    let tokfile = d.path().join(format!("{nome}.tok"));
    let token = token_new(&srv.state, nome, &tokfile);
    AmbAuth {
        d,
        srv,
        token,
        tokfile,
    }
}

// ---------- A1: --resume-file e exclusivo com --from-start ----------

#[test]
fn a1_resume_file_com_from_start_e_uso_incorreto_e_nada_e_criado() {
    let (d, srv) = sobe_padrao();
    let arq = d.path().join("cur.txt");
    let sock = envs_sock(&srv.socket);
    let r = cli_env(
        &[
            "watch",
            "--as",
            CODEX,
            "--from-start",
            "--resume-file",
            s(&arq),
        ],
        &[("CHATD_SOCKET", sock.as_str())],
        None,
    );
    assert_eq!(r.code, Some(2), "saida: {r:?}");
    assert!(
        r.stdout.is_empty(),
        "nada deve ser impresso: {:?}",
        r.stdout
    );
    assert!(!arq.exists(), "uso incorreto nao cria o arquivo");
}

// ---------- A2: retoma de N e segue ao vivo ----------

#[test]
fn a2_resume_file_retoma_de_n_e_segue_ao_vivo() {
    let (d, srv) = sobe_padrao();
    let sock = envs_sock(&srv.socket);
    envia_ok(&srv.socket, "r-1", CODEX, "a");
    envia_ok(&srv.socket, "r-2", CODEX, "b");
    envia_ok(&srv.socket, "r-3", CODEX, "c");
    let arq = d.path().join("cur.txt");
    fs::write(&arq, "1\n").unwrap();
    let w = Watch::inicia(
        &["--as", CODEX, "--resume-file", s(&arq)],
        &[("CHATD_SOCKET", sock.as_str())],
    );
    assert_eq!(
        w.proxima(Duration::from_secs(5)).unwrap(),
        "2 r-2 AVISO claude-main>codex-web | assunto"
    );
    assert_eq!(
        w.proxima(Duration::from_secs(5)).unwrap(),
        "3 r-3 AVISO claude-main>codex-web | assunto"
    );
    envia_ok(&srv.socket, "r-4", CODEX, "d");
    assert_eq!(
        w.proxima(Duration::from_secs(5)).unwrap(),
        "4 r-4 AVISO claude-main>codex-web | assunto"
    );
}

// ---------- A3: arquivo ausente comeca no head e e criado com 0600 ----------

#[test]
fn a3_resume_file_ausente_comeca_no_head_e_cria_arquivo_0600() {
    // AMBIGUO: a spec manda criar ARQ com o head antes do primeiro evento novo; nao fixa o instante exato da criacao. Exigimos que exista com o head antes de enviar o novo evento.
    let (d, srv) = sobe_padrao();
    let sock = envs_sock(&srv.socket);
    envia_ok(&srv.socket, "h-1", CODEX, "antigo");
    let arq = d.path().join("cur.txt");
    let w = Watch::inicia(
        &["--as", CODEX, "--resume-file", s(&arq)],
        &[("CHATD_SOCKET", sock.as_str())],
    );
    assert!(
        espera(Duration::from_secs(3), || arq.exists()),
        "ARQ nao criado"
    );
    assert_eq!(
        le_arq(&arq).as_deref(),
        Some("1"),
        "ARQ deve conter o head atual"
    );
    assert_eq!(modo_de(&arq), 0o600, "ARQ deve ser 0600");
    envia_ok(&srv.socket, "h-2", CODEX, "novo");
    let l = w.proxima(Duration::from_secs(5)).expect("evento novo");
    assert!(l.contains(" h-2 "), "o historico nao deve sair: {l}");
    assert_eq!(seq_da_linha(&l), 2);
}

// ---------- A4: cursor gravado depois de imprimir; eventos alheios avancam o cursor ----------

#[test]
fn a4_cursor_e_o_seq_do_ultimo_evento_recebido() {
    // Eventos que a identidade nao pode ver nao chegam ao cliente (o daemon filtra): o cursor so anda com os recebidos.
    let (d, srv) = sobe_padrao();
    let sock = envs_sock(&srv.socket);
    envia_ok(&srv.socket, "c-1", CLAUDE, "para o claude"); // seq 1: nao e para o codex
    envia_ok(&srv.socket, "c-2", CODEX, "para o codex"); // seq 2
    envia_ok(&srv.socket, "c-3", CODEX, "outra para o codex"); // seq 3
    let arq = d.path().join("cur.txt");
    fs::write(&arq, "0\n").unwrap();
    let w = Watch::inicia(
        &["--as", CODEX, "--resume-file", s(&arq)],
        &[("CHATD_SOCKET", sock.as_str())],
    );
    let l2 = w.proxima(Duration::from_secs(5)).expect("linha 2");
    assert_eq!(
        seq_da_linha(&l2),
        2,
        "a mensagem do claude (seq 1) nao sai: {l2}"
    );
    let l3 = w.proxima(Duration::from_secs(5)).expect("linha 3");
    assert_eq!(seq_da_linha(&l3), 3, "{l3}");
    assert!(
        espera(Duration::from_secs(3), || le_arq(&arq).as_deref()
            == Some("3")),
        "cursor deve ser 3"
    );
    assert_eq!(modo_de(&arq), 0o600, "0600 depois das atualizacoes");
}

// ---------- B: --exit-after ----------

#[test]
fn b_exit_after_sai_0_sem_linhas_extras() {
    let (_d, srv) = sobe_padrao();
    let sock = envs_sock(&srv.socket);
    let t = Instant::now();
    let mut w = Watch::inicia(
        &["--as", CODEX, "--exit-after", "2s"],
        &[("CHATD_SOCKET", sock.as_str())],
    );
    assert_eq!(
        w.espera_saida(Duration::from_secs(8)),
        Some(0),
        "exit-after deve sair com 0"
    );
    let decorrido = t.elapsed();
    assert!(
        decorrido >= Duration::from_millis(1500),
        "saiu cedo demais: {decorrido:?}"
    );
    assert!(
        decorrido <= Duration::from_secs(7),
        "saiu tarde demais: {decorrido:?}"
    );
    assert!(
        w.linhas_por(Duration::from_millis(100)).is_empty(),
        "nenhuma linha extra"
    );
}

#[test]
fn b_exit_after_conclui_as_linhas_devidas_antes_de_sair() {
    let (_d, srv) = sobe_padrao();
    let sock = envs_sock(&srv.socket);
    envia_ok(&srv.socket, "bx-1", CODEX, "a");
    let mut w = Watch::inicia(
        &["--as", CODEX, "--from-start", "--exit-after", "2s"],
        &[("CHATD_SOCKET", sock.as_str())],
    );
    let l = w.proxima(Duration::from_secs(5)).expect("linha devida");
    assert!(l.contains(" bx-1 "), "{l}");
    assert_eq!(w.espera_saida(Duration::from_secs(8)), Some(0));
}

// ---------- C: --heartbeat-file ----------

#[test]
fn c_heartbeat_grava_instante_0600_e_atualiza_a_cada_5s_sem_saida() {
    let (d, srv) = sobe_padrao();
    let sock = envs_sock(&srv.socket);
    let hb = d.path().join("hb.txt");
    let w = Watch::inicia(
        &["--as", CODEX, "--heartbeat-file", s(&hb)],
        &[("CHATD_SOCKET", sock.as_str())],
    );
    assert!(
        espera(Duration::from_secs(3), || hb.exists()),
        "heartbeat nao criado"
    );
    let t1: u64 = le_arq(&hb)
        .unwrap()
        .parse()
        .expect("instante Unix em uma linha");
    assert!(agora().abs_diff(t1) <= 10, "instante fora de hora: {t1}");
    assert_eq!(modo_de(&hb), 0o600, "heartbeat 0600");
    thread::sleep(Duration::from_secs(6));
    let t2: u64 = le_arq(&hb).unwrap().parse().unwrap();
    assert!(
        t2 > t1,
        "heartbeat deve ser reescrito a cada 5 s ({t1} -> {t2})"
    );
    assert!(
        w.linhas_por(Duration::from_millis(100)).is_empty(),
        "heartbeat nao pode sair na saida padrao"
    );
}

#[test]
fn c_heartbeat_congela_com_daemon_fora_e_volta_com_ele() {
    // Sem --reconnect a v1.1 ja reconecta sozinha: o processo continua vivo; o heartbeat so e escrito enquanto conectado.
    let (d, mut srv) = sobe_padrao();
    let cfg = d.path().join("config.json");
    let st = srv.state.clone();
    let sock = envs_sock(&srv.socket);
    let hb = d.path().join("hb.txt");
    let mut w = Watch::inicia(
        &["--as", CODEX, "--heartbeat-file", s(&hb)],
        &[("CHATD_SOCKET", sock.as_str())],
    );
    assert!(espera(Duration::from_secs(3), || hb.exists()));
    assert!(srv.para().is_some());
    thread::sleep(Duration::from_millis(1500));
    assert!(
        w.vivo(),
        "sem --reconnect a v1.1 continua reconectando: o processo deve seguir vivo"
    );
    let fora = le_arq(&hb);
    thread::sleep(Duration::from_secs(6));
    assert_eq!(
        le_arq(&hb),
        fora,
        "heartbeat nao pode ser atualizado com o daemon fora"
    );
    srv = sobe(&st, &cfg);
    assert!(
        espera(Duration::from_secs(10), || le_arq(&hb) != fora),
        "heartbeat deve voltar a ser atualizado depois que o daemon volta"
    );
    assert!(w.vivo());
    drop(srv);
}

// ---------- D: --reconnect ----------

#[test]
fn d_reconnect_sobrevive_a_reinicio_e_entrega_cada_evento_uma_vez() {
    let (d, mut srv) = sobe_padrao();
    let cfg = d.path().join("config.json");
    let st = srv.state.clone();
    let sock = envs_sock(&srv.socket);
    let mut w = Watch::inicia(
        &["--as", CODEX, "--reconnect"],
        &[("CHATD_SOCKET", sock.as_str())],
    );
    thread::sleep(Duration::from_millis(700));
    envia_ok(&srv.socket, "rc-1", CODEX, "antes");
    let l1 = w.proxima(Duration::from_secs(5)).expect("primeira linha");
    assert!(l1.contains(" rc-1 "), "{l1}");

    assert!(srv.para().is_some());
    thread::sleep(Duration::from_millis(1500));
    assert!(w.vivo(), "--reconnect nao pode terminar na queda");
    assert!(
        espera(Duration::from_secs(4), || w
            .err()
            .contains("reconectando em")),
        "stderr sem aviso de reconexao: {:?}",
        w.err()
    );

    srv = sobe(&st, &cfg);
    envia_ok(&srv.socket, "rc-2", CODEX, "depois");
    let l2 = w
        .proxima(Duration::from_secs(15))
        .expect("evento depois do reinicio");
    assert!(l2.contains(" rc-2 "), "{l2}");
    let resto = w.linhas_por(Duration::from_secs(1));
    let todas: Vec<&String> = std::iter::once(&l1)
        .chain(std::iter::once(&l2))
        .chain(resto.iter())
        .collect();
    assert_eq!(
        todas.iter().filter(|l| l.contains(" rc-1 ")).count(),
        1,
        "rc-1 repetida: {todas:?}"
    );
    assert_eq!(
        todas.iter().filter(|l| l.contains(" rc-2 ")).count(),
        1,
        "rc-2 repetida ou perdida: {todas:?}"
    );
    assert!(w.vivo());
}

#[test]
fn d_reconnect_com_socket_ausente_faz_backoff_1_e_2_segundos() {
    // AMBIGUO: a spec fixa a sequencia 1, 2, 4, 8... de espera; o instante exato de cada aviso no stderr nao esta fixado.
    let d = tmp();
    let st = d.path().join("state");
    let cfg = config_padrao(d.path());
    let sock = st.join("chatd.sock");
    let sock_s = envs_sock(&sock);
    let w = Watch::inicia(
        &["--as", CODEX, "--reconnect"],
        &[("CHATD_SOCKET", sock_s.as_str())],
    );
    assert!(
        espera(Duration::from_secs(5), || w
            .err()
            .contains("reconectando em 1s")),
        "faltou 'em 1s': {:?}",
        w.err()
    );
    assert!(
        espera(Duration::from_secs(8), || w
            .err()
            .contains("reconectando em 2s")),
        "faltou 'em 2s': {:?}",
        w.err()
    );
    let srv = sobe(&st, &cfg);
    // o watch comeca no head da primeira conexao: so enviamos o evento real depois de ver a linha de sincronizacao
    let mut sincronizado = false;
    for i in 0..30 {
        envia_ok(&srv.socket, &format!("sync-{i}"), CODEX, "sync");
        if w.proxima(Duration::from_millis(400))
            .is_some_and(|l| l.contains(" sync-"))
        {
            sincronizado = true;
            break;
        }
    }
    assert!(sincronizado, "watch nunca se conectou ao daemon que subiu");
    envia_ok(&srv.socket, "bo-1", CODEX, "x");
    let l = w
        .linhas_por(Duration::from_secs(5))
        .into_iter()
        .find(|l| l.contains(" bo-1 "));
    assert!(l.is_some(), "evento bo-1 nao chegou depois da reconexao");
}

#[test]
fn d_reconnect_com_exit_after_sai_0_sem_servidor() {
    let d = tmp();
    let st = d.path().join("state");
    let sock = st.join("chatd.sock");
    let sock_s = envs_sock(&sock);
    let t = Instant::now();
    let mut w = Watch::inicia(
        &["--as", CODEX, "--reconnect", "--exit-after", "3s"],
        &[("CHATD_SOCKET", sock_s.as_str())],
    );
    assert_eq!(w.espera_saida(Duration::from_secs(10)), Some(0));
    assert!(
        t.elapsed() >= Duration::from_millis(2500),
        "{:?}",
        t.elapsed()
    );
}

#[test]
fn d_falha_de_auth_com_reconnect_e_definitiva() {
    let a = amb_auth(CLAUDE);
    let errado = a.d.path().join("errado.tok");
    fs::write(&errado, "0".repeat(64)).unwrap();
    fs::set_permissions(&errado, fs::Permissions::from_mode(0o600)).unwrap();
    let sock = envs_sock(&a.srv.socket);
    let errado_s = s(&errado).to_string();
    let mut w = Watch::inicia(
        &["--as", CLAUDE, "--reconnect"],
        &[
            ("CHATD_TOKEN_FILE", errado_s.as_str()),
            ("CHATD_SOCKET", sock.as_str()),
        ],
    );
    let code = w.espera_saida(Duration::from_secs(6));
    assert!(
        matches!(code, Some(c) if c != 0),
        "auth recusada deve sair com erro: {code:?}"
    );
    assert!(
        !w.err().contains("reconectando"),
        "auth recusada nao pode ter retry: {:?}",
        w.err()
    );
}

// ---------- extras: opcoes invalidas ----------

#[test]
fn extra_exit_after_invalido_e_uso_incorreto() {
    let (_d, srv) = sobe_padrao();
    let sock = envs_sock(&srv.socket);
    for v in ["0", "0s", "25h", "86401s", "abc", "30", "1.5s", "2d", "-1s"] {
        let r = cli_env(
            &["watch", "--as", CODEX, "--exit-after", v],
            &[("CHATD_SOCKET", sock.as_str())],
            None,
        );
        assert_eq!(
            r.code,
            Some(2),
            "--exit-after {v} deve ser uso incorreto: {r:?}"
        );
        assert!(
            r.stdout.is_empty(),
            "--exit-after {v} nao pode imprimir: {:?}",
            r.stdout
        );
    }
}

#[test]
fn extra_exit_after_valido_aceita_1s_e_24h() {
    let (_d, srv) = sobe_padrao();
    let sock = envs_sock(&srv.socket);
    let mut w = Watch::inicia(
        &["--as", CODEX, "--exit-after", "1s"],
        &[("CHATD_SOCKET", sock.as_str())],
    );
    assert_eq!(w.espera_saida(Duration::from_secs(6)), Some(0));
    let mut longo = Watch::inicia(
        &["--as", CODEX, "--exit-after", "24h"],
        &[("CHATD_SOCKET", sock.as_str())],
    );
    assert!(
        longo.espera_saida(Duration::from_millis(1500)).is_none(),
        "24h deve manter o watch ativo"
    );
    assert!(longo.vivo());
}

// ---------- extras: --resume-file com conteudo invalido ----------

#[test]
fn extra_resume_file_com_lixo_e_uso_incorreto_sem_entregar() {
    let (d, srv) = sobe_padrao();
    let sock = envs_sock(&srv.socket);
    envia_ok(&srv.socket, "lx-1", CODEX, "a");
    let arq = d.path().join("cur.txt");
    for conteudo in ["abc", "-1", "1.5"] {
        fs::write(&arq, format!("{conteudo}\n")).unwrap();
        let r = cli_env(
            &["watch", "--as", CODEX, "--resume-file", s(&arq)],
            &[("CHATD_SOCKET", sock.as_str())],
            None,
        );
        assert_eq!(r.code, Some(2), "conteudo {conteudo:?}: {r:?}");
        assert!(
            r.stdout.is_empty(),
            "conteudo {conteudo:?} nao pode entregar nada: {:?}",
            r.stdout
        );
    }
}

#[test]
fn extra_resume_file_com_n_maior_que_head_e_uso_incorreto() {
    let (d, srv) = sobe_padrao();
    let sock = envs_sock(&srv.socket);
    envia_ok(&srv.socket, "gh-1", CODEX, "a");
    envia_ok(&srv.socket, "gh-2", CODEX, "b");
    let arq = d.path().join("cur.txt");
    fs::write(&arq, "99\n").unwrap();
    let r = cli_env(
        &["watch", "--as", CODEX, "--resume-file", s(&arq)],
        &[("CHATD_SOCKET", sock.as_str())],
        None,
    );
    assert_eq!(r.code, Some(2), "{r:?}");
    assert!(r.stdout.is_empty(), "{:?}", r.stdout);
}

// ---------- extras: retomada em duas execucoes ----------

#[test]
fn extra_retomada_com_dois_watch_entrega_so_os_novos_em_ordem() {
    let (d, srv) = sobe_padrao();
    let sock = envs_sock(&srv.socket);
    envia_ok(&srv.socket, "rt-1", CODEX, "a");
    envia_ok(&srv.socket, "rt-2", CODEX, "b");
    let arq = d.path().join("cur.txt");
    fs::write(&arq, "0\n").unwrap();
    let mut w1 = Watch::inicia(
        &[
            "--as",
            CODEX,
            "--resume-file",
            s(&arq),
            "--exit-after",
            "2s",
        ],
        &[("CHATD_SOCKET", sock.as_str())],
    );
    let primeira = w1.linhas_por(Duration::from_secs(4));
    assert_eq!(w1.espera_saida(Duration::from_secs(5)), Some(0));
    assert_eq!(primeira.len(), 2, "primeira execucao: {primeira:?}");
    assert!(
        primeira[0].contains(" rt-1 ") && primeira[1].contains(" rt-2 "),
        "{primeira:?}"
    );

    envia_ok(&srv.socket, "rt-3", CODEX, "c");
    envia_ok(&srv.socket, "rt-4", CODEX, "d");
    let mut w2 = Watch::inicia(
        &[
            "--as",
            CODEX,
            "--resume-file",
            s(&arq),
            "--exit-after",
            "2s",
        ],
        &[("CHATD_SOCKET", sock.as_str())],
    );
    let segunda = w2.linhas_por(Duration::from_secs(4));
    assert_eq!(w2.espera_saida(Duration::from_secs(5)), Some(0));
    assert_eq!(segunda.len(), 2, "segunda execucao: {segunda:?}");
    assert!(
        segunda[0].contains(" rt-3 ") && segunda[1].contains(" rt-4 "),
        "{segunda:?}"
    );
}

// ---------- invariantes: token fora dos arquivos e das saidas ----------

#[test]
fn e_token_nunca_aparece_em_arquivos_de_v12_nem_em_saidas() {
    let a = amb_auth(CLAUDE);
    let sock = envs_sock(&a.srv.socket);
    let arq = a.d.path().join("cur.txt");
    let hb = a.d.path().join("hb.txt");
    let tokfile_s = s(&a.tokfile).to_string();
    let mut w = Watch::inicia(
        &[
            "--as",
            CLAUDE,
            "--resume-file",
            s(&arq),
            "--heartbeat-file",
            s(&hb),
            "--exit-after",
            "3s",
        ],
        &[
            ("CHATD_TOKEN_FILE", tokfile_s.as_str()),
            ("CHATD_SOCKET", sock.as_str()),
        ],
    );
    assert!(
        espera(Duration::from_secs(3), || hb.exists()),
        "heartbeat nao criado"
    );
    // mensagem para o claude depois da conexao, para haver linha impressa
    let mut codex = conexao_auth(
        &a.srv.socket,
        CODEX,
        &token_new(&a.srv.state, CODEX, &a.d.path().join("codex.tok")),
    );
    assert!(!is_err(&codex.chama(
        "message.send",
        json!({"v": 1, "id": "tk-1", "to": CLAUDE, "kind": "AVISO", "subject": "s", "body": "b"})
    )));
    let linhas = w.linhas_por(Duration::from_secs(5));
    assert_eq!(w.espera_saida(Duration::from_secs(5)), Some(0));
    assert!(linhas.iter().any(|l| l.contains(" tk-1 ")), "{linhas:?}");
    let tok = &a.token;
    for conteudo in [
        le_arq(&arq).unwrap_or_default(),
        le_arq(&hb).unwrap_or_default(),
        linhas.join("\n"),
        w.err(),
    ] {
        assert!(
            !conteudo.contains(tok.as_str()),
            "token vazou: {conteudo:?}"
        );
    }
}

// ---------- W03: N igual ao head e valido ----------

#[test]
fn w03_resume_file_com_n_igual_ao_head_e_valido() {
    let (d, srv) = sobe_padrao();
    let sock = envs_sock(&srv.socket);
    envia_ok(&srv.socket, "w3-1", CODEX, "a");
    envia_ok(&srv.socket, "w3-2", CODEX, "b");
    let arq = d.path().join("cur.txt");
    fs::write(&arq, "2\n").unwrap();
    let mut w = Watch::inicia(
        &["--as", CODEX, "--resume-file", s(&arq)],
        &[("CHATD_SOCKET", sock.as_str())],
    );
    // N == head_seq e valido: nao e erro de uso, o watch fica ligado e nao entrega historico
    assert!(
        w.espera_saida(Duration::from_millis(800)).is_none(),
        "N igual ao head nao pode sair"
    );
    assert!(w.vivo());
    assert!(
        w.linhas_por(Duration::from_millis(300)).is_empty(),
        "nada deve ser entregue"
    );
    envia_ok(&srv.socket, "w3-3", CODEX, "c");
    let l = w
        .proxima(Duration::from_secs(5))
        .expect("evento novo depois de N == head");
    assert!(l.contains(" w3-3 "), "{l}");
    assert_eq!(seq_da_linha(&l), 3);
    assert!(
        espera(Duration::from_secs(3), || le_arq(&arq).as_deref()
            == Some("3")),
        "cursor deve ser 3"
    );
}

// ---------- W10: sem --reconnect, a v1.1 reconecta em cerca de 1 s ----------

/// Daemon cai e volta apos 2 s. Com espera de 1 s entre tentativas (t = 0, 1, 2, 3...), a reconexao
/// acontece no maximo ~1 s depois da volta. O evento enviado logo apos a volta tem de sair em ate 2,2 s
/// (1 s de espera mais margem). Com a espera antiga de 5 s o teste falha.
#[test]
fn w10_sem_reconnect_reconecta_em_ate_2_2s_depois_da_volta() {
    let (d, mut srv) = sobe_padrao();
    let cfg = d.path().join("config.json");
    let st = srv.state.clone();
    let sock = envs_sock(&srv.socket);
    let mut w = Watch::inicia(&["--as", CODEX], &[("CHATD_SOCKET", sock.as_str())]);
    thread::sleep(Duration::from_millis(700));
    envia_ok(&srv.socket, "w10-0", CODEX, "sync");
    let l0 = w.proxima(Duration::from_secs(5)).expect("sincronizacao");
    assert!(l0.contains(" w10-0 "), "{l0}");

    assert!(srv.para().is_some());
    thread::sleep(Duration::from_secs(2));
    assert!(w.vivo(), "sem --reconnect a v1.1 continua reconectando");
    srv = sobe(&st, &cfg);

    let volta = Instant::now();
    envia_ok(&srv.socket, "w10-1", CODEX, "depois da volta");
    let l1 = w
        .proxima(Duration::from_millis(2200))
        .expect("evento enviado logo apos a volta deve chegar em ate 2,2 s");
    assert!(
        volta.elapsed() <= Duration::from_millis(2200),
        "chegou tarde: {:?}",
        volta.elapsed()
    );
    assert!(l1.contains(" w10-1 "), "{l1}");
    assert!(w.vivo());
}

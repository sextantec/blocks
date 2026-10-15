//! Lacunas da mutacao v1.1/F: comportamentos que nenhum teste anterior provava. Integracao, so binario e socket.
mod common;

use common::*;
use serde_json::{Value, json};
use std::fs;
use std::io::{BufRead, BufReader};
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
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

/// Auth obrigatoria, um token por nome.
fn amb(nomes: &[&str]) -> Amb {
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
    fn tokfile(&self, n: &str) -> PathBuf {
        self.d.path().join(format!("{n}.tok"))
    }
}

fn envia_de(c: &mut Conexao, id: &str, para: &str, corpo: &str) -> Value {
    c.chama(
        "message.send",
        json!({"v": 1, "id": id, "to": para, "kind": "AVISO", "subject": "assunto", "body": corpo}),
    )
}

fn ids_de(r: &Value) -> Vec<String> {
    r["messages"]
        .as_array()
        .expect("messages")
        .iter()
        .map(|m| m["id"].as_str().unwrap().to_string())
        .collect()
}

/// Caracteres de controle proibidos na saida (exceto \n e \t).
fn tem_controle(s: &str) -> bool {
    s.chars().any(|c| {
        ((c as u32) < 0x20 && c != '\n' && c != '\t') || (0x7f..=0x9f).contains(&(c as u32))
    })
}

/// `chatd watch` com stdout lido por thread.
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
    fn vivo(&mut self) -> bool {
        matches!(self.child.try_wait(), Ok(None))
    }
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
}

impl Drop for Watch {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn troca_no_log(state: &Path, de: &str, para: &str) {
    let p = state.join("log.jsonl");
    let t = fs::read_to_string(&p).unwrap();
    assert!(t.contains(de), "marca {de} ausente no log");
    fs::write(&p, t.replace(de, para)).unwrap();
}

// ---------- 1. take isola o destinatario ----------

#[test]
fn take_do_codex_nao_toca_nas_mensagens_do_claude() {
    let a = amb(&[CLAUDE, CODEX]);
    let mut claude = a.conecta(CLAUDE);
    let mut codex = a.conecta(CODEX);
    assert!(!is_err(&envia_de(&mut claude, "tk-c1", CODEX, "p1")));
    assert!(!is_err(&envia_de(&mut claude, "tk-c2", CODEX, "p2")));
    assert!(!is_err(&envia_de(&mut codex, "tk-k1", CLAUDE, "p3")));

    let tomadas = res(&codex.chama("message.take", json!({"v": 1})));
    assert_eq!(
        ids_de(&tomadas),
        vec!["tk-c1", "tk-c2"],
        "take devolve so a caixa do codex: {tomadas}"
    );

    // a caixa do claude segue intacta: mensagem ainda registrada
    let caixa = res(&claude.chama("message.list", json!({"v": 1})))["messages"].clone();
    assert_eq!(caixa[0]["id"], "tk-k1", "{caixa}");
    assert_eq!(caixa[0]["state"], "registrado", "{caixa}");
    assert_eq!(caixa.as_array().unwrap().len(), 1, "{caixa}");
}

#[test]
fn inbox_take_por_cli_do_codex_nao_toca_o_claude() {
    let a = amb(&[CLAUDE, CODEX]);
    let mut claude = a.conecta(CLAUDE);
    let mut codex = a.conecta(CODEX);
    assert!(!is_err(&envia_de(
        &mut claude,
        "ck-1",
        CODEX,
        "para o codex"
    )));
    assert!(!is_err(&envia_de(
        &mut codex,
        "ck-2",
        CLAUDE,
        "para o claude"
    )));
    let tok = s(&a.tokfile(CODEX)).to_string();
    let sock = s(&a.srv.socket).to_string();
    let r = cli_com(
        &["inbox", "--take"],
        &[
            ("CHATD_AS", CODEX),
            ("CHATD_TOKEN_FILE", tok.as_str()),
            ("CHATD_SOCKET", sock.as_str()),
        ],
    );
    assert_eq!(r.code, Some(0), "{r:?}");
    assert!(
        r.stdout.contains("para o codex") && !r.stdout.contains("para o claude"),
        "{:?}",
        r.stdout
    );
    let caixa = res(&claude.chama("message.list", json!({"v": 1})))["messages"].clone();
    assert_eq!(caixa[0]["id"], "ck-2", "{caixa}");
    assert_eq!(caixa[0]["state"], "registrado", "{caixa}");
}

// ---------- 2. eventos de seguranca nao vazam para o stream ----------

#[test]
fn denied_e_auth_failed_nao_chegam_ao_events_resume() {
    let a = amb(&[CLAUDE, CODEX]);
    let mut claude = a.conecta(CLAUDE);
    res(&claude.chama("events.resume", json!({"v": 1, "last_seq": 0})));
    let mut codex = a.conecta(CODEX);
    // evento denied: codex tenta enviar como claude
    assert_eq!(
        cod(&codex.chama(
            "message.send",
            msg("sv-1", CLAUDE, CLAUDE, "AVISO", "s", "b")
        )),
        "forbidden"
    );
    // evento auth_failed (senha errada)
    assert!(is_err(&auth_tenta(&a.srv.socket, CLAUDE, &"9".repeat(64))));
    // marcador: mensagem legitima para o claude
    assert!(!is_err(&envia_de(&mut codex, "sv-ok", CLAUDE, "x")));
    let evs = claude.eventos_ate(Duration::from_secs(5), |e| e["id"] == "sv-ok");
    assert!(
        evs.iter().any(|e| e["id"] == "sv-ok"),
        "marcador nao chegou: {evs:?}"
    );
    assert!(
        evs.iter()
            .all(|e| e["type"] != "denied" && e["type"] != "auth_failed"),
        "evento de seguranca vazou para o stream: {evs:?}"
    );
    // os eventos de seguranca existem no log (so o `log` os mostra)
    assert_eq!(eventos_tipo(&a.srv.state, "denied").len(), 1);
    assert_eq!(eventos_tipo(&a.srv.state, "auth_failed").len(), 1);
}

#[test]
fn denied_e_auth_failed_nao_aparecem_no_watch() {
    let a = amb(&[CLAUDE, CODEX]);
    let tok = s(&a.tokfile(CLAUDE)).to_string();
    let sock = s(&a.srv.socket).to_string();
    let w = Watch::inicia(
        &["--as", CLAUDE],
        &[
            ("CHATD_TOKEN_FILE", tok.as_str()),
            ("CHATD_SOCKET", sock.as_str()),
        ],
    );
    thread::sleep(Duration::from_millis(700));
    let mut codex = a.conecta(CODEX);
    assert_eq!(
        cod(&codex.chama(
            "message.send",
            msg("wv-1", CLAUDE, CLAUDE, "AVISO", "s", "b")
        )),
        "forbidden"
    );
    assert!(is_err(&auth_tenta(&a.srv.socket, CLAUDE, &"8".repeat(64))));
    assert!(!is_err(&envia_de(&mut codex, "wv-ok", CLAUDE, "x")));
    let primeira = w.proxima(Duration::from_secs(5)).expect("linha do watch");
    assert!(
        primeira.contains(" wv-ok "),
        "watch deveria mostrar so a mensagem legitima: {primeira}"
    );
    assert!(
        w.linhas_por(Duration::from_millis(500)).is_empty(),
        "linhas extras no watch"
    );
}

// ---------- 3. nome invalido no rastro ----------

#[test]
fn auth_com_nome_invalido_grava_auth_failed_sem_name() {
    let a = amb(&[CLAUDE]);
    let sock = a.srv.socket.clone();
    let nome = "x\u{1b}[2K";
    assert!(is_err(&auth_tenta(&sock, nome, &a.tok(CLAUDE))));
    let falhas = eventos_tipo(&a.srv.state, "auth_failed");
    assert_eq!(falhas.len(), 1, "{falhas:?}");
    assert!(
        falhas[0].get("name").is_none(),
        "nome invalido nao pode ir ao log: {}",
        falhas[0]
    );
    assert!(falhas[0].get("peer_pid").is_some(), "{}", falhas[0]);
    let raw = fs::read(a.srv.state.join("log.jsonl")).unwrap();
    assert!(!raw.contains(&0x1b), "byte ESC no log");
    let r = cli(&["log", "--state-dir", s(&a.srv.state)]);
    assert!(
        !tem_controle(&r.stdout),
        "controle na saida do chatd log: {:?}",
        r.stdout
    );
}

#[test]
fn auth_com_nome_com_espaco_grava_auth_failed_sem_name() {
    let a = amb(&[CLAUDE]);
    assert!(is_err(&auth_tenta(
        &a.srv.socket,
        "Bad Name",
        &a.tok(CLAUDE)
    )));
    let falhas = eventos_tipo(&a.srv.state, "auth_failed");
    assert_eq!(falhas.len(), 1, "{falhas:?}");
    assert!(falhas[0].get("name").is_none(), "{}", falhas[0]);
}

#[test]
fn denied_com_claimed_invalido_nao_grava_claimed() {
    let a = amb(&[CLAUDE, CODEX]);
    let mut codex = a.conecta(CODEX);
    // `to` com controle e formato invalido, pedido de caixa alheia: a negacao deve aparecer sem o nome
    let r = codex.chama("message.list", json!({"v": 1, "to": "x\u{1b}[2K"}));
    assert!(is_err(&r), "{r}");
    let negs = eventos_tipo(&a.srv.state, "denied");
    assert_eq!(negs.len(), 1, "esperava uma negacao gravada: {negs:?}");
    assert!(
        negs[0].get("claimed").is_none(),
        "claimed invalido nao pode ir ao log: {}",
        negs[0]
    );
    let raw = fs::read(a.srv.state.join("log.jsonl")).unwrap();
    assert!(!raw.contains(&0x1b), "byte ESC no log");
}

// ---------- 4. take com `to` desconhecido ----------

#[test]
fn take_com_to_desconhecido_e_unknown_recipient_como_list() {
    let (_d, srv) = sobe_padrao();
    let take = rpc(
        &srv.socket,
        "message.take",
        json!({"v": 1, "to": "ninguem"}),
    );
    let list = rpc(
        &srv.socket,
        "message.list",
        json!({"v": 1, "to": "ninguem"}),
    );
    assert_eq!(cod(&list), "unknown_recipient");
    assert_eq!(cod(&take), "unknown_recipient", "{take}");
}

// ---------- 5. auth.json com campo extra ----------

#[test]
fn auth_json_com_campo_extra_e_recusado() {
    let a = amb(&[CLAUDE]);
    let tok = a.tok(CLAUDE);
    fs::write(
        a.srv.state.join("auth.json"),
        r#"{"v":1,"tokens":{},"x":1}"#,
    )
    .unwrap();
    let r = cli(&["token", "list", "--state-dir", s(&a.srv.state)]);
    assert_ne!(
        r.code,
        Some(0),
        "token list deve falhar com auth.json extra: {r:?}"
    );
    let resp = auth_tenta(&a.srv.socket, CLAUDE, &tok);
    assert!(
        is_err(&resp),
        "auth.json recusado nao pode autenticar: {resp}"
    );
}

// ---------- 6. token new sem permissao de escrita nao deixa token orfao ----------

#[test]
fn token_new_sem_escrita_no_state_dir_nao_deixa_arquivo() {
    if unsafe { libc::geteuid() } == 0 {
        // como root a permissao 0500 nao impede a escrita; o teste so vale para usuario comum
        return;
    }
    let a = amb(&[CLAUDE]);
    let st = a.srv.state.clone();
    let out = a.d.path().join("novo.tok");
    fs::set_permissions(&st, fs::Permissions::from_mode(0o500)).unwrap();
    let r = cli(&[
        "token",
        "new",
        CODEX,
        "--state-dir",
        s(&st),
        "--out",
        s(&out),
    ]);
    let existe = out.exists();
    fs::set_permissions(&st, fs::Permissions::from_mode(0o700)).unwrap();
    assert_ne!(r.code, Some(0), "token new deve falhar sem escrita: {r:?}");
    assert!(!existe, "arquivo de token orfao em --out");
}

// ---------- 7. destinatario ve accepted e resulted ----------

#[test]
fn destinatario_recebe_accepted_e_resulted_no_stream() {
    let a = amb(&[CLAUDE, CODEX]);
    let mut codex = a.conecta(CODEX);
    res(&codex.chama("events.resume", json!({"v": 1, "last_seq": 0})));
    let mut claude = a.conecta(CLAUDE);
    assert!(!is_err(&envia_de(&mut claude, "ar-1", CODEX, "x")));
    assert!(!is_err(&codex.chama(
        "message.ack",
        json!({"v": 1, "id": "ar-1", "stage": "aceito"})
    )));
    assert!(!is_err(&codex.chama(
        "message.ack",
        json!({"v": 1, "id": "ar-1", "stage": "resultado", "result": "done"})
    )));
    let evs = codex.eventos_ate(Duration::from_secs(5), |e| {
        e["type"] == "resulted" && e["id"] == "ar-1"
    });
    let tipos: Vec<&str> = evs
        .iter()
        .filter(|e| e["id"] == "ar-1")
        .map(|e| e["type"].as_str().unwrap())
        .collect();
    assert_eq!(tipos, vec!["registered", "accepted", "resulted"], "{evs:?}");
}

// ---------- 8. exibicao escapada de controles C1 e BEL ----------

fn log_com_c1_e_bel() -> (TempDir, PathBuf, PathBuf) {
    let d = tmp();
    let cfg = config_padrao(d.path());
    let st = d.path().join("state");
    let mut srv = sobe(&st, &cfg);
    let r = envia(
        &srv.socket,
        msg("ce-1", CLAUDE, CODEX, "AVISO", "SUBJMARK", "BODYMARK"),
    );
    assert!(!is_err(&r), "{r}");
    assert!(srv.para().is_some());
    drop(srv);
    // escapes JSON: U+0085, U+009B, U+007F, U+0007
    troca_no_log(&st, "SUBJMARK", r"a\u0085b\u009bc\u007fd\u0007e");
    troca_no_log(&st, "BODYMARK", r"f\u0085g\u009bh\u007fi\u0007j");
    (d, st, cfg)
}

const ESC_OK: [&str; 4] = [r"\x85", r"\x9b", r"\x7f", r"\x07"];

#[test]
fn controles_c1_bel_e_del_saem_escapados_nas_quatro_visoes() {
    let (_d, st, cfg) = log_com_c1_e_bel();
    let srv = sobe(&st, &cfg);
    let sock = s(&srv.socket).to_string();
    let envs = [("CHATD_AS", CODEX), ("CHATD_SOCKET", sock.as_str())];

    let inbox = cli_com(&["inbox"], &envs);
    assert_eq!(inbox.code, Some(0), "{inbox:?}");
    assert!(
        !tem_controle(&inbox.stdout),
        "controle cru no inbox: {:?}",
        inbox.stdout
    );
    for e in ESC_OK {
        assert!(
            inbox.stdout.contains(e),
            "inbox sem {e}: {:?}",
            inbox.stdout
        );
    }

    let take = cli_com(&["inbox", "--take"], &envs);
    assert!(
        !tem_controle(&take.stdout),
        "controle cru no inbox --take: {:?}",
        take.stdout
    );
    for e in ESC_OK {
        assert!(
            take.stdout.contains(e),
            "inbox --take sem {e}: {:?}",
            take.stdout
        );
    }

    let log = cli(&["log", "--state-dir", s(&st)]);
    assert!(
        !tem_controle(&log.stdout),
        "controle cru no log: {:?}",
        log.stdout
    );
    for e in ESC_OK {
        assert!(log.stdout.contains(e), "log sem {e}: {:?}", log.stdout);
    }

    let mut w = Watch::inicia(
        &["--as", CODEX, "--from-start"],
        &[("CHATD_SOCKET", sock.as_str())],
    );
    let linha = w.proxima(Duration::from_secs(5)).expect("linha do watch");
    assert!(!tem_controle(&linha), "controle cru no watch: {linha:?}");
    for e in ESC_OK {
        assert!(linha.contains(e), "watch sem {e}: {linha:?}");
    }
    assert!(w.vivo());
}

// ---------- 9. flags incompativeis do inbox ----------

#[test]
fn inbox_take_com_all_ou_after_e_uso_incorreto_e_nao_consome() {
    // AMBIGUO: a spec nao diz o codigo de saida de `--take` com `--all` ou `--after`; lemos como uso incorreto (2).
    let (_d, srv) = sobe_padrao();
    let sock = s(&srv.socket).to_string();
    envia_ok(&srv.socket, "fl-1", CODEX, "a");
    envia_ok(&srv.socket, "fl-2", CODEX, "b");
    let envs = [("CHATD_AS", CODEX), ("CHATD_SOCKET", sock.as_str())];
    let r1 = cli_com(&["inbox", "--take", "--all"], &envs);
    assert_eq!(r1.code, Some(2), "{r1:?}");
    let r2 = cli_com(&["inbox", "--take", "--after", "1"], &envs);
    assert_eq!(r2.code, Some(2), "{r2:?}");
    let caixa = res(&rpc(
        &srv.socket,
        "message.list",
        json!({"v": 1, "to": CODEX}),
    ));
    assert_eq!(
        ids_de(&caixa),
        vec!["fl-1", "fl-2"],
        "as mensagens nao podem ser consumidas"
    );
    assert_eq!(caixa["messages"][0]["state"], "registrado");
    assert_eq!(caixa["messages"][1]["state"], "registrado");
}

// ---------- 10. inbox --take pela CLI ----------

#[test]
fn inbox_take_cli_imprime_tres_e_deixa_aceito_com_note_take() {
    let (_d, srv) = sobe_padrao();
    let sock = s(&srv.socket).to_string();
    for (i, c) in ["corpo1", "corpo2", "corpo3"].iter().enumerate() {
        envia_ok(&srv.socket, &format!("it-{i}"), CODEX, c);
    }
    let r = cli_com(
        &["inbox", "--take"],
        &[("CHATD_AS", CODEX), ("CHATD_SOCKET", sock.as_str())],
    );
    assert_eq!(r.code, Some(0), "{r:?}");
    for c in ["corpo1", "corpo2", "corpo3"] {
        assert!(
            r.stdout.contains(c),
            "take deve imprimir {c}: {:?}",
            r.stdout
        );
    }
    let caixa = res(&rpc(
        &srv.socket,
        "message.list",
        json!({"v": 1, "to": CODEX}),
    ))["messages"]
        .clone();
    for m in caixa.as_array().unwrap() {
        assert_eq!(m["state"], "aceito", "{m}");
    }
    // AMBIGUO: a spec fixa note "take" no evento accepted; nao fixa que a Mensagem exibida traga esse note.
    assert_eq!(caixa[0]["note"], "take", "{caixa}");
}

// ---------- 11. watch: identidade e token ----------

#[test]
fn watch_sem_token_com_auth_off_conecta_mesmo_com_chatd_as_diferente() {
    let (_d, srv) = sobe_padrao();
    let sock = s(&srv.socket).to_string();
    let mut w = Watch::inicia(
        &["--as", CODEX],
        &[("CHATD_AS", CLAUDE), ("CHATD_SOCKET", sock.as_str())],
    );
    thread::sleep(Duration::from_millis(500));
    assert!(w.vivo(), "watch nao pode falhar sem token com auth off");
    let mut remetente = Conexao::abre(&srv.socket);
    assert!(!is_err(&remetente.chama(
        "message.send",
        msg("wt-1", CLAUDE, CODEX, "AVISO", "s", "b")
    )));
    let linha = w.proxima(Duration::from_secs(5)).expect("linha do watch");
    assert!(linha.contains(" wt-1 "), "{linha}");
}

#[test]
fn watch_com_token_e_chatd_as_diferente_da_forbidden() {
    let a = amb(&[CLAUDE, CODEX]);
    let tok_claude = s(&a.tokfile(CLAUDE)).to_string();
    let sock = s(&a.srv.socket).to_string();
    // o token e do claude; a flag pede a caixa do codex: a conexao nao pode operar a caixa alheia
    let r = cli_com(
        &["watch", "--as", CODEX],
        &[
            ("CHATD_AS", CLAUDE),
            ("CHATD_TOKEN_FILE", tok_claude.as_str()),
            ("CHATD_SOCKET", sock.as_str()),
        ],
    );
    assert_ne!(
        r.code,
        Some(0),
        "watch com token alheio nao pode sair com sucesso: {r:?}"
    );
    assert!(
        r.stderr.contains("forbidden") || r.stdout.contains("forbidden"),
        "esperava forbidden: {r:?}"
    );
}

//! v1.1 secoes B (autenticacao) e D (identidade imposta). Contrato: docs/spec-v1.1-agentes.md.
mod common;

use common::*;
use serde_json::{Value, json};
use std::fs;
use std::path::PathBuf;
use std::time::Duration;
use tempfile::TempDir;

const CLAUDE: &str = "claude-main";
const CODEX: &str = "codex-web";
const TERCEIRO: &str = "terceiro";

struct Amb {
    d: TempDir,
    srv: Servidor,
    tokens: Vec<(String, String)>,
}

/// Servidor com auth obrigatoria e um token emitido para cada nome.
fn amb_com(nomes: &[&str]) -> Amb {
    let d = tmp();
    let cfg = config_req(d.path());
    let srv = sobe(&d.path().join("state"), &cfg);
    let mut tokens = Vec::new();
    for n in nomes {
        let out = d.path().join(format!("{n}.tok"));
        let t = token_new(&srv.state, n, &out);
        tokens.push((n.to_string(), t));
    }
    Amb { d, srv, tokens }
}

impl Amb {
    fn tok(&self, nome: &str) -> String {
        self.tokens
            .iter()
            .find(|(k, _)| k == nome)
            .unwrap_or_else(|| panic!("sem token para {nome}"))
            .1
            .clone()
    }
    fn conecta(&self, nome: &str) -> Conexao {
        conexao_auth(&self.srv.socket, nome, &self.tok(nome))
    }
    fn sock(&self) -> PathBuf {
        self.srv.socket.clone()
    }
}

fn e_hex64(s: &str) -> bool {
    s.len() == 64
        && s.chars()
            .all(|c| c.is_ascii_digit() || ('a'..='f').contains(&c))
}

// ---------- B: emissao de token ----------

#[test]
fn token_new_com_out_grava_64_hex_0600_e_imprime_so_ok_nome() {
    let a = amb_com(&[]);
    let out = a.d.path().join("x.tok");
    let r = cli(&[
        "token",
        "new",
        CLAUDE,
        "--state-dir",
        s(&a.srv.state),
        "--out",
        s(&out),
    ]);
    assert_eq!(r.code, Some(0), "saida: {r:?}");
    assert_eq!(
        r.stdout.trim(),
        format!("ok {CLAUDE}").as_str(),
        "stdout: {:?}",
        r.stdout
    );
    let t = fs::read_to_string(&out).expect("arquivo do token");
    assert!(e_hex64(t.trim()), "token deve ser 64 hex minusculos: {t:?}");
    assert_eq!(modo_de(&out), 0o600, "--out deve ser 0600");
}

#[test]
fn token_new_sem_out_imprime_o_token_em_stdout_e_ele_autentica() {
    let a = amb_com(&[]);
    let r = cli(&["token", "new", CLAUDE, "--state-dir", s(&a.srv.state)]);
    assert_eq!(r.code, Some(0), "saida: {r:?}");
    let tok = r
        .stdout
        .split_whitespace()
        .find(|w| e_hex64(w))
        .unwrap_or_else(|| panic!("token ausente em stdout: {:?}", r.stdout))
        .to_string();
    let resp = auth_tenta(&a.srv.socket, CLAUDE, &tok);
    assert!(!is_err(&resp), "token impresso deve autenticar: {resp}");
}

#[test]
fn auth_json_0600_guarda_so_o_hash() {
    let a = amb_com(&[CLAUDE]);
    let arq = a.srv.state.join("auth.json");
    assert_eq!(modo_de(&arq), 0o600, "auth.json deve ser 0600");
    let j = auth_json(&a.srv.state);
    assert_eq!(j["v"], 1);
    let h = j["tokens"][CLAUDE].as_str().expect("hash do claude");
    assert!(e_hex64(h), "hash SHA-256 em hex: {h}");
    assert_ne!(
        h,
        a.tok(CLAUDE),
        "o arquivo nao pode guardar o token em claro"
    );
    let txt = fs::read_to_string(&arq).unwrap();
    assert!(!txt.contains(&a.tok(CLAUDE)), "token em claro em auth.json");
}

#[test]
fn token_new_repetido_sem_rotate_sai_1_e_o_antigo_segue_valendo() {
    let a = amb_com(&[CLAUDE]);
    let out = a.d.path().join("outro.tok");
    let r = cli(&[
        "token",
        "new",
        CLAUDE,
        "--state-dir",
        s(&a.srv.state),
        "--out",
        s(&out),
    ]);
    assert_eq!(r.code, Some(1), "saida: {r:?}");
    assert!(!is_err(&auth_tenta(&a.srv.socket, CLAUDE, &a.tok(CLAUDE))));
}

#[test]
fn token_new_com_rotate_troca_o_token() {
    let a = amb_com(&[CLAUDE]);
    let antigo = a.tok(CLAUDE);
    let out = a.d.path().join("novo.tok");
    let r = cli(&[
        "token",
        "new",
        CLAUDE,
        "--state-dir",
        s(&a.srv.state),
        "--out",
        s(&out),
        "--rotate",
    ]);
    assert_eq!(r.code, Some(0), "saida: {r:?}");
    let novo = fs::read_to_string(&out).unwrap().trim().to_string();
    assert_ne!(novo, antigo);
    assert!(
        is_err(&auth_tenta(&a.srv.socket, CLAUDE, &antigo)),
        "token antigo deve deixar de valer"
    );
    assert!(
        !is_err(&auth_tenta(&a.srv.socket, CLAUDE, &novo)),
        "token novo deve valer"
    );
}

#[test]
fn token_new_nome_invalido_nao_e_gravado() {
    // AMBIGUO: a spec manda o NOME casar [a-z0-9_-]{1,32}, mas nao fixa o codigo de saida da recusa. Testamos so que nao sai 0 e que nada e gravado.
    let a = amb_com(&[]);
    let longo = "a".repeat(33);
    for ruim in ["Codex Web", longo.as_str()] {
        let out = a.d.path().join("ruim.tok");
        let r = cli(&[
            "token",
            "new",
            ruim,
            "--state-dir",
            s(&a.srv.state),
            "--out",
            s(&out),
        ]);
        assert_ne!(r.code, Some(0), "nome {ruim:?} aceito: {r:?}");
    }
    let txt = fs::read_to_string(a.srv.state.join("auth.json")).unwrap_or_default();
    assert!(
        !txt.contains("Codex Web") && !txt.contains("aaaaaaaa"),
        "nome invalido gravado: {txt}"
    );
}

#[test]
fn out_existente_e_recusado_sem_tocar_no_arquivo() {
    let a = amb_com(&[]);
    let out = a.d.path().join("existente.tok");
    fs::write(&out, "antigo").unwrap();
    let r = cli(&[
        "token",
        "new",
        CLAUDE,
        "--state-dir",
        s(&a.srv.state),
        "--out",
        s(&out),
    ]);
    assert_ne!(r.code, Some(0), "--out existente deve ser recusado: {r:?}");
    assert_eq!(fs::read_to_string(&out).unwrap(), "antigo");
}

#[test]
fn token_revoke_remove_e_list_mostra_so_nomes() {
    let a = amb_com(&[CLAUDE, CODEX]);
    let st = s(&a.srv.state);
    let l = cli(&["token", "list", "--state-dir", st]);
    assert_eq!(l.code, Some(0), "saida: {l:?}");
    let palavras: Vec<&str> = l.stdout.split_whitespace().collect();
    assert!(
        palavras.contains(&CLAUDE) && palavras.contains(&CODEX),
        "list: {:?}",
        l.stdout
    );
    assert!(
        !l.stdout.contains(&a.tok(CLAUDE)),
        "list nao pode imprimir token"
    );

    let r = cli(&["token", "revoke", CLAUDE, "--state-dir", st]);
    assert_eq!(r.code, Some(0), "saida: {r:?}");
    let l2 = cli(&["token", "list", "--state-dir", st]);
    assert!(
        !l2.stdout.split_whitespace().any(|w| w == CLAUDE),
        "revogado ainda listado: {:?}",
        l2.stdout
    );
    assert!(is_err(&auth_tenta(&a.srv.socket, CLAUDE, &a.tok(CLAUDE))));
    // o outro continua valendo
    assert!(!is_err(&auth_tenta(&a.srv.socket, CODEX, &a.tok(CODEX))));
}

#[test]
fn token_revogado_deixa_de_valer_sem_reiniciar_o_servidor() {
    let a = amb_com(&[CODEX]);
    let mut c = a.conecta(CODEX);
    res(&c.chama("status", json!({"v": 1})));
    let r = cli(&["token", "revoke", CODEX, "--state-dir", s(&a.srv.state)]);
    assert_eq!(r.code, Some(0), "saida: {r:?}");
    // mesmo servidor, nova conexao: a revogacao ja vale
    assert_eq!(
        cod(&auth_tenta(&a.srv.socket, CODEX, &a.tok(CODEX))),
        "unauthenticated"
    );
    assert_eq!(
        cod(&Conexao::abre(&a.srv.socket).chama("status", json!({"v": 1}))),
        "unauthenticated"
    );
}

// ---------- B: handshake ----------

#[test]
fn handshake_com_token_certo_devolve_v_e_name() {
    let a = amb_com(&[CLAUDE]);
    let r = auth_tenta(&a.srv.socket, CLAUDE, &a.tok(CLAUDE));
    let v = res(&r);
    assert_eq!(v["v"], 1);
    assert_eq!(v["name"], CLAUDE);
}

#[test]
fn metodos_antes_do_auth_sao_unauthenticated() {
    let a = amb_com(&[CLAUDE]);
    let sock = a.sock();
    let casos: Vec<(&str, Value)> = vec![
        ("status", json!({"v": 1})),
        ("message.list", json!({"v": 1})),
        ("message.take", json!({"v": 1})),
        (
            "message.send",
            msg("un-1", CLAUDE, CODEX, "AVISO", "s", "b"),
        ),
        ("message.ack", json!({"v": 1, "id": "x", "stage": "aceito"})),
    ];
    for (metodo, p) in casos {
        let r = Conexao::abre(&sock).chama(metodo, p);
        assert_eq!(cod(&r), "unauthenticated", "metodo {metodo}: {r}");
    }
}

#[test]
fn unauthenticated_tem_a_mesma_mensagem_nas_tres_causas() {
    let a = amb_com(&[CLAUDE]);
    let sock = a.sock();
    let r1 = auth_tenta(&sock, "nao-existe", &a.tok(CLAUDE)); // nome ausente
    let r2 = auth_tenta(&sock, CLAUDE, &"0".repeat(64)); // token errado
    let r3 = auth_tenta(&sock, "Formato Ruim", "xyz"); // formato invalido
    for r in [&r1, &r2, &r3] {
        assert_eq!(cod(r), "unauthenticated", "{r}");
    }
    assert_eq!(
        r1["error"], r2["error"],
        "nome ausente e token errado devem ser identicos"
    );
    assert_eq!(
        r2["error"], r3["error"],
        "token errado e formato invalido devem ser identicos"
    );
}

#[test]
fn segundo_auth_na_mesma_conexao_e_invalid_request() {
    let a = amb_com(&[CLAUDE]);
    let mut c = a.conecta(CLAUDE);
    let r = c.chama(
        "auth",
        json!({"v": 1, "name": CLAUDE, "token": a.tok(CLAUDE)}),
    );
    assert_eq!(cod(&r), "invalid_request");
}

#[test]
fn token_de_a_nao_autentica_como_b() {
    let a = amb_com(&[CLAUDE, CODEX]);
    let r = auth_tenta(&a.srv.socket, CODEX, &a.tok(CLAUDE));
    assert_eq!(cod(&r), "unauthenticated");
}

#[test]
fn cinco_falhas_do_mesmo_pid_trancam_novas_tentativas() {
    // AMBIGUO: a spec diz "5 falhas em 60 s trancam por 300 s"; nao fica claro se a 5a falha ja e recusada ou so a 6a. Testamos a 6a com o token certo.
    let a = amb_com(&[CLAUDE]);
    let sock = a.sock();
    let errado = "1".repeat(64);
    for _ in 0..5 {
        let r = auth_tenta(&sock, CLAUDE, &errado);
        assert_eq!(cod(&r), "unauthenticated");
    }
    let r = auth_tenta(&sock, CLAUDE, &a.tok(CLAUDE));
    assert_eq!(
        cod(&r),
        "unauthenticated",
        "pid trancado deve ser recusado mesmo com o token certo: {r}"
    );
}

#[test]
fn falhas_gravam_auth_failed_com_pid_e_sem_token() {
    // AMBIGUO: a spec manda gravar o evento auth_failed; nao fixa que ele fique em log.jsonl. Lemos que fica (e o unico lugar duravel).
    let a = amb_com(&[CLAUDE]);
    let sock = a.sock();
    let errado = "2".repeat(64);
    for _ in 0..3 {
        let _ = auth_tenta(&sock, CLAUDE, &errado);
    }
    // sucesso nao grava evento
    assert!(!is_err(&auth_tenta(&sock, CLAUDE, &a.tok(CLAUDE))));
    let falhas = eventos_tipo(&a.srv.state, "auth_failed");
    assert_eq!(falhas.len(), 3, "uma falha, um evento: {falhas:?}");
    for f in &falhas {
        assert_eq!(
            f["peer_pid"],
            i64::from(std::process::id()),
            "peer_pid: {f}"
        );
        assert!(f.get("peer_uid").is_some(), "peer_uid ausente: {f}");
    }
    let log = fs::read(a.srv.state.join("log.jsonl")).unwrap();
    assert!(
        !String::from_utf8_lossy(&log).contains(&errado),
        "token errado no log"
    );
    assert!(
        eventos_tipo(&a.srv.state, "auth_ok").is_empty(),
        "sucesso nao grava evento"
    );
}

#[test]
fn token_nunca_aparece_em_stdout_stderr_do_cliente_nem_no_log() {
    let a = amb_com(&[CLAUDE]);
    let tok = a.tok(CLAUDE);
    let tokfile = a.d.path().join(format!("{CLAUDE}.tok"));
    let sock = a.sock();
    // cliente com arquivo de token
    let r = cli_com(
        &["status"],
        &[
            ("CHATD_AS", CLAUDE),
            ("CHATD_TOKEN_FILE", s(&tokfile)),
            ("CHATD_SOCKET", s(&sock)),
        ],
    );
    assert_eq!(r.code, Some(0), "status com token: {r:?}");
    assert!(
        !r.stdout.contains(&tok) && !r.stderr.contains(&tok),
        "token vazou no cliente"
    );
    // tentativa com nome errado e token certo: o servidor nao pode ecoar o token
    let _ = auth_tenta(&sock, "outro-nome", &tok);
    assert!(
        !a.srv.stderr().contains(&tok),
        "token vazou no stderr do servidor"
    );
    let log = fs::read(a.srv.state.join("log.jsonl")).unwrap_or_default();
    assert!(
        !String::from_utf8_lossy(&log).contains(&tok),
        "token vazou no log"
    );
}

#[test]
fn config_com_campo_bell_antigo_e_recusada() {
    let d = tmp();
    let mut cfg = config_req_json(std::path::Path::new("/nonexistent/herdr"), 50, 600);
    cfg["recipients"]["codex-web"] = json!({"bell": {"kind": "herdr", "pane": "w2:p1"}});
    let c = escreve_config(d.path(), &cfg);
    let st = d.path().join("state");
    match tenta(
        &["serve", "--state-dir", s(&st), "--config", s(&c)],
        &[],
        &st,
        &st.join("chatd.sock"),
    ) {
        Ok(_) => panic!("config com o campo bell da v1 foi aceita"),
        Err(e) => assert_eq!(e.code, Some(1), "saida: {e:?}"),
    }
}

#[test]
fn com_auth_off_o_metodo_auth_nao_existe() {
    let (_d, srv) = sobe_padrao();
    let r = rpc(
        &srv.socket,
        "auth",
        json!({"v": 1, "name": CLAUDE, "token": "x"}),
    );
    assert_eq!(cod(&r), "method_not_found");
}

// ---------- D: identidade imposta ----------

#[test]
fn send_com_from_de_outro_e_forbidden() {
    let a = amb_com(&[CLAUDE, CODEX]);
    let mut c = a.conecta(CODEX);
    let r = c.chama(
        "message.send",
        msg("f-1", CLAUDE, CLAUDE, "AVISO", "s", "b"),
    );
    assert_eq!(cod(&r), "forbidden");
    let ok = c.chama("message.send", msg("f-2", CODEX, CLAUDE, "AVISO", "s", "b"));
    assert!(
        !is_err(&ok),
        "from igual a identidade deve ser aceito: {ok}"
    );
}

#[test]
fn send_sem_from_usa_a_identidade_da_conexao() {
    let a = amb_com(&[CLAUDE, CODEX]);
    let mut c = a.conecta(CODEX);
    let p =
        json!({"v": 1, "id": "fa-1", "to": CLAUDE, "kind": "AVISO", "subject": "s", "body": "b"});
    assert!(!is_err(&c.chama("message.send", p)));
    let mut d = a.conecta(CLAUDE);
    let ms = res(&d.chama("message.list", json!({"v": 1})))["messages"].clone();
    assert_eq!(ms[0]["id"], "fa-1");
    assert_eq!(ms[0]["from"], CODEX);
}

#[test]
fn list_de_caixa_alheia_e_forbidden_e_sem_to_e_a_propria() {
    let a = amb_com(&[CLAUDE, CODEX]);
    let mut codex = a.conecta(CODEX);
    let mut claude = a.conecta(CLAUDE);
    assert!(!is_err(&codex.chama(
        "message.send",
        msg("la-1", CODEX, CLAUDE, "AVISO", "s", "b")
    )));
    let r = codex.chama("message.list", json!({"v": 1, "to": CLAUDE}));
    assert_eq!(cod(&r), "forbidden");
    // sem `to` = a caixa da propria identidade (codex nao recebeu nada: lista vazia)
    let own = res(&codex.chama("message.list", json!({"v": 1})));
    assert_eq!(own["messages"].as_array().unwrap().len(), 0);
    // to igual a identidade: aceito
    let igual = codex.chama("message.list", json!({"v": 1, "to": CODEX}));
    assert!(!is_err(&igual), "{igual}");
    // claude ve a sua
    let caixa = res(&claude.chama("message.list", json!({"v": 1})));
    assert_eq!(caixa["messages"][0]["id"], "la-1");
}

#[test]
fn ack_com_by_de_outro_e_forbidden() {
    let a = amb_com(&[CLAUDE, CODEX]);
    let mut claude = a.conecta(CLAUDE);
    assert!(!is_err(&claude.chama(
        "message.send",
        msg("ak-1", CLAUDE, CODEX, "AVISO", "s", "b")
    )));
    // claude nao pode aceitar a caixa do codex, nem com by ausente (= claude) nem com by = codex
    let sem_by = claude.chama(
        "message.ack",
        json!({"v": 1, "id": "ak-1", "stage": "aceito"}),
    );
    assert_eq!(cod(&sem_by), "forbidden");
    let com_by = claude.chama(
        "message.ack",
        json!({"v": 1, "id": "ak-1", "by": CODEX, "stage": "aceito"}),
    );
    assert_eq!(cod(&com_by), "forbidden");
    // o destinatario, com by ausente, aceita
    let mut codex = a.conecta(CODEX);
    let ok = codex.chama(
        "message.ack",
        json!({"v": 1, "id": "ak-1", "stage": "aceito"}),
    );
    assert!(!is_err(&ok), "{ok}");
}

#[test]
fn events_resume_entrega_so_eventos_da_identidade() {
    let a = amb_com(&[CLAUDE, CODEX, TERCEIRO]);
    let mut claude = a.conecta(CLAUDE);
    res(&claude.chama("events.resume", json!({"v": 1, "last_seq": 0})));
    let mut codex = a.conecta(CODEX);
    // seq 1: codex -> terceiro (nao envolve o claude)
    assert!(!is_err(&codex.chama(
        "message.send",
        msg("ev-1", CODEX, TERCEIRO, "AVISO", "s", "b")
    )));
    // seq 2: claude -> codex (envolve o claude)
    assert!(!is_err(&claude.chama(
        "message.send",
        msg("ev-2", CLAUDE, CODEX, "AVISO", "s", "b")
    )));
    let evs = claude.eventos_ate(Duration::from_secs(3), |e| e["seq"] == 2);
    assert!(
        evs.iter().any(|e| e["seq"] == 2 && e["id"] == "ev-2"),
        "faltou o evento do claude: {evs:?}"
    );
    assert!(
        evs.iter().all(|e| e["seq"] != 1),
        "evento de codex->terceiro vazou para o claude: {evs:?}"
    );
}

#[test]
fn status_devolve_so_a_propria_entrada() {
    let a = amb_com(&[CLAUDE, CODEX, TERCEIRO]);
    let mut claude = a.conecta(CLAUDE);
    let r = res(&claude.chama("status", json!({"v": 1})));
    let chaves: Vec<String> = r["recipients"]
        .as_object()
        .expect("recipients objeto")
        .keys()
        .cloned()
        .collect();
    assert_eq!(
        chaves,
        vec![CLAUDE.to_string()],
        "status deve mostrar so a identidade: {r}"
    );
}

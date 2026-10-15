//! v1.1 secoes C (message.take, refs) e D (inbox --take/--ids, send --ref, CHATD_AS, precedencia da flag).
mod common;

use common::*;
use serde_json::{Value, json};
use std::path::PathBuf;
use std::thread;
use std::time::Duration;
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
}

/// Envia como a identidade da conexao (sem `from`).
fn envia_de(c: &mut Conexao, id: &str, para: &str, corpo: &str) -> Value {
    c.chama(
        "message.send",
        json!({"v": 1, "id": id, "to": para, "kind": "AVISO", "subject": "assunto", "body": corpo}),
    )
}

fn take(c: &mut Conexao, limit: Option<u64>) -> Value {
    let mut p = json!({"v": 1});
    if let Some(l) = limit {
        p["limit"] = json!(l);
    }
    res(&c.chama("message.take", p))
}

fn ids(r: &Value) -> Vec<String> {
    r["messages"]
        .as_array()
        .expect("messages")
        .iter()
        .map(|m| m["id"].as_str().unwrap().to_string())
        .collect()
}

// ---------- C.1 message.take ----------

#[test]
fn take_devolve_corpo_e_marca_aceito() {
    let a = amb_com(&[CLAUDE, CODEX]);
    let mut claude = a.conecta(CLAUDE);
    let mut codex = a.conecta(CODEX);
    assert!(!is_err(&envia_de(&mut claude, "tk-1", CODEX, "corpo um")));
    let r = take(&mut codex, None);
    let ms = r["messages"].as_array().unwrap();
    assert_eq!(ms.len(), 1, "{r}");
    assert_eq!(ms[0]["id"], "tk-1");
    assert_eq!(ms[0]["body"], "corpo um");
    assert_eq!(ms[0]["state"], "aceito");
    assert!(r["head_seq"].is_number());
    // AMBIGUO: a spec manda gravar o accepted com note "take"; nao fixa que a Mensagem devolvida traga esse note. Conferimos aqui e tambem no evento.
    assert_eq!(ms[0]["note"], "take");
    let seg = take(&mut codex, None);
    assert!(ids(&seg).is_empty(), "ja tomada nao volta: {seg}");
}

#[test]
fn take_evento_accepted_leva_note_take() {
    // AMBIGUO: a spec diz que o evento accepted do take tem note "take"; o nome do campo no Evento nao esta fixado na spec, usamos `note`.
    let a = amb_com(&[CLAUDE, CODEX]);
    let mut claude = a.conecta(CLAUDE);
    res(&claude.chama("events.resume", json!({"v": 1, "last_seq": 0})));
    let mut codex = a.conecta(CODEX);
    assert!(!is_err(&envia_de(&mut claude, "tn-1", CODEX, "x")));
    let _ = take(&mut codex, None);
    let evs = claude.eventos_ate(Duration::from_secs(3), |e| e["type"] == "accepted");
    let ev = evs.last().expect("evento accepted");
    assert_eq!(ev["id"], "tn-1");
    assert_eq!(ev["note"], "take");
}

#[test]
fn take_respeita_limit_e_a_ordem_de_seq() {
    let a = amb_com(&[CLAUDE, CODEX]);
    let mut claude = a.conecta(CLAUDE);
    let mut codex = a.conecta(CODEX);
    for i in 1..=5 {
        assert!(!is_err(&envia_de(
            &mut claude,
            &format!("t-{i}"),
            CODEX,
            "x"
        )));
    }
    assert_eq!(ids(&take(&mut codex, Some(2))), vec!["t-1", "t-2"]);
    assert_eq!(ids(&take(&mut codex, Some(2))), vec!["t-3", "t-4"]);
    assert_eq!(ids(&take(&mut codex, None)), vec!["t-5"]);
    assert!(ids(&take(&mut codex, None)).is_empty());
}

#[test]
fn take_limit_padrao_50_e_limites_1_a_200() {
    let a = amb_com(&[CLAUDE, CODEX]);
    let mut claude = a.conecta(CLAUDE);
    let mut codex = a.conecta(CODEX);
    for i in 0..52 {
        assert!(!is_err(&envia_de(
            &mut claude,
            &format!("lp-{i}"),
            CODEX,
            "x"
        )));
    }
    assert_eq!(ids(&take(&mut codex, None)).len(), 50, "padrao 50");
    assert_eq!(ids(&take(&mut codex, None)).len(), 2);
    for ruim in [0u64, 201] {
        let r = codex.chama("message.take", json!({"v": 1, "limit": ruim}));
        assert_eq!(cod(&r), "invalid_envelope", "limit {ruim}");
        assert_eq!(caminho(&r), "/limit", "limit {ruim}");
    }
}

#[test]
fn take_vazio_nao_grava_nada() {
    let a = amb_com(&[CLAUDE, CODEX]);
    let mut codex = a.conecta(CODEX);
    let linhas_antes = log_linhas(&a.srv.state).len();
    let r = take(&mut codex, None);
    assert!(ids(&r).is_empty(), "{r}");
    assert_eq!(
        log_linhas(&a.srv.state).len(),
        linhas_antes,
        "take vazio nao grava evento"
    );
}

#[test]
fn take_nao_devolve_as_ja_resolvidas() {
    let a = amb_com(&[CLAUDE, CODEX]);
    let mut claude = a.conecta(CLAUDE);
    let mut codex = a.conecta(CODEX);
    assert!(!is_err(&envia_de(&mut claude, "rs-1", CODEX, "a")));
    assert!(!is_err(&envia_de(&mut claude, "rs-2", CODEX, "b")));
    let r = codex.chama(
        "message.ack",
        json!({"v": 1, "id": "rs-1", "stage": "resultado", "result": "done"}),
    );
    assert!(!is_err(&r), "{r}");
    assert_eq!(ids(&take(&mut codex, None)), vec!["rs-2"]);
}

#[test]
fn take_de_caixa_alheia_e_forbidden() {
    let a = amb_com(&[CLAUDE, CODEX]);
    let mut codex = a.conecta(CODEX);
    let r = codex.chama("message.take", json!({"v": 1, "to": CLAUDE}));
    assert_eq!(cod(&r), "forbidden");
    let mut claude = a.conecta(CLAUDE);
    let ok = claude.chama("message.take", json!({"v": 1, "to": CLAUDE}));
    assert!(!is_err(&ok), "to igual a identidade: {ok}");
}

#[test]
fn take_atomico_sob_concorrencia_sem_duplicatas() {
    let a = amb_com(&[CLAUDE, CODEX]);
    let mut claude = a.conecta(CLAUDE);
    for i in 0..40 {
        assert!(!is_err(&envia_de(
            &mut claude,
            &format!("cc-{i:02}"),
            CODEX,
            "x"
        )));
    }
    let sock = a.srv.socket.clone();
    let tok = a.tok(CODEX);
    let hs: Vec<_> = (0..8)
        .map(|_| {
            let sock = sock.clone();
            let tok = tok.clone();
            thread::spawn(move || {
                let mut c = conexao_auth(&sock, CODEX, &tok);
                let mut tomados = Vec::new();
                loop {
                    let r = take(&mut c, Some(5));
                    let v = ids(&r);
                    if v.is_empty() {
                        break;
                    }
                    tomados.extend(v);
                }
                tomados
            })
        })
        .collect();
    let mut todos: Vec<String> = hs.into_iter().flat_map(|h| h.join().unwrap()).collect();
    assert_eq!(
        todos.len(),
        40,
        "cada mensagem deve ser tomada exatamente uma vez"
    );
    todos.sort();
    todos.dedup();
    assert_eq!(todos.len(), 40, "houve duplicata no take concorrente");
}

// ---------- C.2 refs ----------

fn refs_completas() -> Value {
    json!([
        {"t": "path", "v": "docs/spec.md"},
        {"t": "commit", "v": "3135e3a"},
        {"t": "msg", "v": "W-078"},
        {"t": "job", "v": "job-42"},
        {"t": "url", "v": "https://example.invalid/x"}
    ])
}

#[test]
fn refs_aceitas_com_os_cinco_tipos_e_devolvidas_na_mensagem() {
    let a = amb_com(&[CLAUDE, CODEX]);
    let mut claude = a.conecta(CLAUDE);
    let refs = refs_completas();
    let r = claude.chama(
        "message.send",
        json!({"v": 1, "id": "rf-1", "to": CODEX, "kind": "AVISO", "subject": "s", "body": "b", "refs": refs}),
    );
    assert!(!is_err(&r), "{r}");
    let mut codex = a.conecta(CODEX);
    let ms = res(&codex.chama("message.list", json!({"v": 1})))["messages"].clone();
    assert_eq!(ms[0]["refs"], refs);
}

#[test]
fn refs_ausente_vira_lista_vazia() {
    let a = amb_com(&[CLAUDE, CODEX]);
    let mut claude = a.conecta(CLAUDE);
    assert!(!is_err(&envia_de(&mut claude, "rv-1", CODEX, "b")));
    let mut codex = a.conecta(CODEX);
    let ms = res(&codex.chama("message.list", json!({"v": 1})))["messages"].clone();
    assert_eq!(ms[0]["refs"], json!([]));
}

#[test]
fn refs_limite_de_16_itens() {
    let a = amb_com(&[CLAUDE, CODEX]);
    let mut claude = a.conecta(CLAUDE);
    let dezesseis: Vec<Value> = (0..16)
        .map(|i| json!({"t": "path", "v": format!("f{i}")}))
        .collect();
    let ok = claude.chama(
        "message.send",
        json!({"v": 1, "id": "rl-16", "to": CODEX, "kind": "AVISO", "subject": "s", "body": "b", "refs": dezesseis}),
    );
    assert!(!is_err(&ok), "16 refs devem ser aceitas: {ok}");
    let mut dezessete = dezesseis.clone();
    dezessete.push(json!({"t": "path", "v": "extra"}));
    let r = claude.chama(
        "message.send",
        json!({"v": 1, "id": "rl-17", "to": CODEX, "kind": "AVISO", "subject": "s", "body": "b", "refs": dezessete}),
    );
    // AMBIGUO: a spec nao fixa o caminho de erro para o excesso de itens (so /refs/N/t e /refs/N/v); testamos so o codigo.
    assert_eq!(cod(&r), "invalid_envelope", "{r}");
}

#[test]
fn refs_tipo_invalido_aponta_o_indice() {
    let a = amb_com(&[CLAUDE, CODEX]);
    let mut claude = a.conecta(CLAUDE);
    let r = claude.chama(
        "message.send",
        json!({"v": 1, "id": "rt-1", "to": CODEX, "kind": "AVISO", "subject": "s", "body": "b",
               "refs": [{"t": "path", "v": "a"}, {"t": "file", "v": "b"}]}),
    );
    assert_eq!(cod(&r), "invalid_envelope");
    assert_eq!(caminho(&r), "/refs/1/t");
}

#[test]
fn refs_v_limites_512_caracteres_sem_quebra() {
    let a = amb_com(&[CLAUDE, CODEX]);
    let mut claude = a.conecta(CLAUDE);
    let envia = |c: &mut Conexao, id: &str, v: Value| {
        c.chama(
            "message.send",
            json!({"v": 1, "id": id, "to": CODEX, "kind": "AVISO", "subject": "s", "body": "b",
                   "refs": [{"t": "path", "v": v}]}),
        )
    };
    assert!(
        !is_err(&envia(&mut claude, "rv-ok", json!("a".repeat(512)))),
        "512 deve passar"
    );
    let r = envia(&mut claude, "rv-513", json!("a".repeat(513)));
    assert_eq!(cod(&r), "invalid_envelope");
    assert_eq!(caminho(&r), "/refs/0/v");
    let r = envia(&mut claude, "rv-vazio", json!(""));
    assert_eq!(caminho(&r), "/refs/0/v");
    let r = envia(&mut claude, "rv-nl", json!("a\nb"));
    assert_eq!(caminho(&r), "/refs/0/v");
}

#[test]
fn refs_entram_no_hash_do_envelope() {
    let a = amb_com(&[CLAUDE, CODEX]);
    let mut claude = a.conecta(CLAUDE);
    let com = |refs: Value| json!({"v": 1, "id": "rh-1", "to": CODEX, "kind": "AVISO", "subject": "s", "body": "b", "refs": refs});
    assert!(!is_err(
        &claude.chama("message.send", com(json!([{"t": "path", "v": "a"}])))
    ));
    // mesmo envelope, refs diferentes: mismatch
    let r = claude.chama("message.send", com(json!([{"t": "path", "v": "b"}])));
    assert_eq!(cod(&r), "envelope_mismatch");
    // mesmo envelope e mesmas refs: duplicate
    let r = res(&claude.chama("message.send", com(json!([{"t": "path", "v": "a"}]))));
    assert_eq!(r["duplicate"], true);
}

// ---------- D: CLI do agente ----------

#[test]
fn inbox_take_por_cli_imprime_corpo_e_nao_devolve_de_novo() {
    let a = amb_com(&[CLAUDE, CODEX]);
    let mut claude = a.conecta(CLAUDE);
    assert!(!is_err(&envia_de(
        &mut claude,
        "ct-1",
        CODEX,
        "texto do pedido"
    )));
    let tok = s(&a.tokfile(CODEX)).to_string();
    let sock = s(&a.srv.socket).to_string();
    let envs = [
        ("CHATD_AS", CODEX),
        ("CHATD_TOKEN_FILE", tok.as_str()),
        ("CHATD_SOCKET", sock.as_str()),
    ];
    let r = cli_com(&["inbox", "--take"], &envs);
    assert_eq!(r.code, Some(0), "saida: {r:?}");
    assert!(
        r.stdout.contains("texto do pedido"),
        "corpo deve sair no take: {:?}",
        r.stdout
    );
    let again = cli_com(&["inbox", "--take"], &envs);
    assert!(
        again.stdout.trim().is_empty(),
        "segundo take deve vir vazio: {:?}",
        again.stdout
    );
}

#[test]
fn inbox_ids_imprime_so_os_ids() {
    let a = amb_com(&[CLAUDE, CODEX]);
    let mut claude = a.conecta(CLAUDE);
    assert!(!is_err(&envia_de(&mut claude, "id-a", CODEX, "x")));
    assert!(!is_err(&envia_de(&mut claude, "id-b", CODEX, "y")));
    let tok = s(&a.tokfile(CODEX)).to_string();
    let sock = s(&a.srv.socket).to_string();
    let r = cli_com(
        &["inbox", "--ids"],
        &[
            ("CHATD_AS", CODEX),
            ("CHATD_TOKEN_FILE", tok.as_str()),
            ("CHATD_SOCKET", sock.as_str()),
        ],
    );
    assert_eq!(r.stdout, "id-a\nid-b\n", "saida: {:?}", r.stdout);
}

#[test]
fn send_por_cli_com_ref_imprime_id_seq_e_grava_refs() {
    let a = amb_com(&[CLAUDE, CODEX]);
    let tok = s(&a.tokfile(CLAUDE)).to_string();
    let sock = s(&a.srv.socket).to_string();
    let r = cli_com(
        &[
            "send",
            "--to",
            CODEX,
            "--kind",
            "AVISO",
            "--subject",
            "veja",
            "--body",
            "ver refs",
            "--id",
            "sr-1",
            "--ref",
            "path:a/b.md",
            "--ref",
            "commit:abc123",
        ],
        &[
            ("CHATD_AS", CLAUDE),
            ("CHATD_TOKEN_FILE", tok.as_str()),
            ("CHATD_SOCKET", sock.as_str()),
        ],
    );
    assert_eq!(r.code, Some(0), "saida: {r:?}");
    assert_eq!(r.stdout, "sr-1 1\n", "saida: {:?}", r.stdout);
    let mut codex = a.conecta(CODEX);
    let ms = res(&codex.chama("message.list", json!({"v": 1})))["messages"].clone();
    assert_eq!(
        ms[0]["refs"],
        json!([{"t": "path", "v": "a/b.md"}, {"t": "commit", "v": "abc123"}]),
        "{ms}"
    );
}

#[test]
fn chatd_as_faz_o_from_implicito_no_send() {
    let a = amb_com(&[CLAUDE, CODEX]);
    let tok = s(&a.tokfile(CLAUDE)).to_string();
    let sock = s(&a.srv.socket).to_string();
    let r = cli_com(
        &[
            "send",
            "--to",
            CODEX,
            "--kind",
            "AVISO",
            "--subject",
            "x",
            "--body",
            "y",
            "--id",
            "fi-1",
        ],
        &[
            ("CHATD_AS", CLAUDE),
            ("CHATD_TOKEN_FILE", tok.as_str()),
            ("CHATD_SOCKET", sock.as_str()),
        ],
    );
    assert_eq!(r.code, Some(0), "saida: {r:?}");
    let mut codex = a.conecta(CODEX);
    let ms = res(&codex.chama("message.list", json!({"v": 1})))["messages"].clone();
    assert_eq!(ms[0]["from"], CLAUDE);
}

#[test]
fn cli_ack_imprime_ok() {
    let a = amb_com(&[CLAUDE, CODEX]);
    let mut claude = a.conecta(CLAUDE);
    assert!(!is_err(&envia_de(&mut claude, "ok-1", CODEX, "x")));
    let tok = s(&a.tokfile(CODEX)).to_string();
    let sock = s(&a.srv.socket).to_string();
    let r = cli_com(
        &["ack", "--id", "ok-1", "--stage", "aceito"],
        &[
            ("CHATD_AS", CODEX),
            ("CHATD_TOKEN_FILE", tok.as_str()),
            ("CHATD_SOCKET", sock.as_str()),
        ],
    );
    assert_eq!(r.code, Some(0), "saida: {r:?}");
    assert_eq!(r.stdout.trim(), "ok");
}

#[test]
fn sem_token_com_auth_required_sai_2() {
    let a = amb_com(&[CLAUDE]);
    let sock = s(&a.srv.socket).to_string();
    let r = cli_com(
        &["status"],
        &[("CHATD_AS", CLAUDE), ("CHATD_SOCKET", sock.as_str())],
    );
    assert_eq!(r.code, Some(2), "saida: {r:?}");
}

#[test]
fn flag_token_file_vence_a_variavel_de_ambiente() {
    let a = amb_com(&[CLAUDE]);
    let ruim = a.d.path().join("nao-existe.tok");
    let bom = s(&a.tokfile(CLAUDE)).to_string();
    let ruim_s = s(&ruim).to_string();
    let sock = s(&a.srv.socket).to_string();
    let r = cli_com(
        &["status", "--token-file", bom.as_str()],
        &[
            ("CHATD_AS", CLAUDE),
            ("CHATD_TOKEN_FILE", ruim_s.as_str()),
            ("CHATD_SOCKET", sock.as_str()),
        ],
    );
    assert_eq!(r.code, Some(0), "a flag deve vencer a variavel: {r:?}");
}

#[test]
fn flag_as_vence_chatd_as_com_auth_off() {
    // sem auth o nome vem de CHATD_AS; a flag --as (ou --from no send) vence a variavel
    let (_d, srv) = sobe_padrao();
    let sock = s(&srv.socket).to_string();
    let envs = [("CHATD_AS", CODEX), ("CHATD_SOCKET", sock.as_str())];
    let r = cli_com(
        &[
            "send",
            "--from",
            CLAUDE,
            "--to",
            CODEX,
            "--kind",
            "AVISO",
            "--subject",
            "s",
            "--body",
            "b",
            "--id",
            "pf-1",
        ],
        &envs,
    );
    assert_eq!(r.code, Some(0), "saida: {r:?}");
    // CHATD_AS=codex: a caixa padrao e a do codex, que recebeu pf-1
    let caixa_codex = cli_com(&["inbox", "--ids"], &envs);
    assert_eq!(
        caixa_codex.stdout.trim(),
        "pf-1",
        "saida: {:?}",
        caixa_codex.stdout
    );
    // --as explicito vence CHATD_AS: a caixa do claude esta vazia
    let caixa_claude = cli_com(&["inbox", "--as", CLAUDE, "--ids"], &envs);
    assert!(
        caixa_claude.stdout.trim().is_empty(),
        "saida: {:?}",
        caixa_claude.stdout
    );
    // o from gravado e o da flag --from, nao o de CHATD_AS
    let mut c = Conexao::abre(&srv.socket);
    let caixa = res(&c.chama("message.list", json!({"v": 1, "to": CODEX})))["messages"].clone();
    assert_eq!(caixa[0]["from"], CLAUDE, "{caixa}");
}

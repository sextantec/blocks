//! Linha de comando (spec secao 4) e resolucao do socket (secao 4).
mod common;

use common::*;
use serde_json::{Value, json};
use std::fs;
use std::io::{Read, Write};
use std::path::Path;
use std::process::{Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

// atualizado pela v1.6: os helpers `cli`/`cli_env` de common nao isolam HOME nem XDG_CONFIG_HOME e nao removem
// as CHATD_* herdadas; o `client.json` real do dono (default_profile e socket) passava a valer nos filhos.
// Estes helpers sombreiam os de common (item local tem precedencia sobre o glob): HOME e XDG_CONFIG_HOME
// apontam para um diretorio temporario vazio e toda CHATD_* herdada e removida antes das do proprio teste.
fn cli(args: &[&str]) -> Saida {
    cli_env(args, &[], None)
}

fn cli_env(args: &[&str], envs: &[(&str, &str)], entrada: Option<&str>) -> Saida {
    let casa = tmp();
    let mut cmd = Command::new(BIN);
    cmd.args(args);
    for (k, _) in std::env::vars() {
        if k.starts_with("CHATD_") {
            cmd.env_remove(&k);
        }
    }
    cmd.env("HOME", casa.path())
        .env("XDG_CONFIG_HOME", casa.path().join("xdg"))
        .envs(envs.iter().copied())
        .stdin(if entrada.is_some() {
            Stdio::piped()
        } else {
            Stdio::null()
        })
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = cmd.spawn().expect("executar chatd");
    if let Some(t) = entrada {
        let mut si = child.stdin.take().unwrap();
        let _ = si.write_all(t.as_bytes());
    }
    let mut so = child.stdout.take().unwrap();
    let mut se = child.stderr.take().unwrap();
    let h1 = thread::spawn(move || {
        let mut b = String::new();
        let _ = so.read_to_string(&mut b);
        b
    });
    let h2 = thread::spawn(move || {
        let mut b = String::new();
        let _ = se.read_to_string(&mut b);
        b
    });
    let fim = Instant::now() + Duration::from_secs(10);
    let code = loop {
        if let Ok(Some(st)) = child.try_wait() {
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
        stdout: h1.join().unwrap(),
        stderr: h2.join().unwrap(),
    }
}

/// Uma unica linha JSON de stdout.
fn json_linha(saida: &str) -> Value {
    let linhas: Vec<&str> = saida.lines().collect();
    assert_eq!(linhas.len(), 1, "esperava uma linha JSON, veio {saida:?}");
    serde_json::from_str(linhas[0]).expect("JSON da saida")
}

/// AMBIGUO: a spec nao fixa o formato de `inbox --json` (array ou um objeto por linha). Aceitamos os dois.
fn mensagens_json(saida: &str) -> Vec<Value> {
    let t = saida.trim();
    if t.is_empty() {
        return vec![];
    }
    match serde_json::from_str::<Value>(t) {
        Ok(Value::Array(a)) => a,
        Ok(v) => vec![v],
        Err(_) => t
            .lines()
            .filter(|l| !l.trim().is_empty())
            .map(|l| serde_json::from_str(l).expect("linha JSON do inbox"))
            .collect(),
    }
}

fn envia_cli(sock: &str, id: &str, subj: &str, corpo: &str) -> Saida {
    cli(&[
        "send",
        "--socket",
        sock,
        "--from",
        "claude-main",
        "--to",
        "codex-web",
        "--kind",
        "AVISO",
        "--subject",
        subj,
        "--body",
        corpo,
        "--id",
        id,
    ])
}

fn inbox_cli(sock: &str, extra: &[&str]) -> Saida {
    let mut a = vec!["inbox", "--socket", sock, "--as", "codex-web"];
    a.extend_from_slice(extra);
    cli(&a)
}

fn ack_cli(sock: &str, id: &str, as_: &str, stage: &str, extra: &[&str]) -> Saida {
    let mut a = vec![
        "ack", "--socket", sock, "--id", id, "--as", as_, "--stage", stage,
    ];
    a.extend_from_slice(extra);
    cli(&a)
}

fn achar(ms: &[Value], id: &str) -> Value {
    ms.iter()
        .find(|m| m["id"] == id)
        .cloned()
        .unwrap_or_else(|| panic!("mensagem {id} ausente em {ms:?}"))
}

// ---------- send ----------

#[test]
fn send_imprime_uma_linha_id_e_seq() {
    // v1.1 (secao D): `send` imprime `ID SEQ` em uma linha, sem JSON.
    let (_d, srv) = sobe_padrao();
    let r = envia_cli(s(&srv.socket), "cli-1", "oi", "corpo");
    assert_eq!(r.code, Some(0), "saida: {r:?}");
    assert_eq!(r.stdout, "cli-1 1\n", "saida: {:?}", r.stdout);
}

#[test]
fn send_json_imprime_o_json_da_v1() {
    let (_d, srv) = sobe_padrao();
    let r = cli(&[
        "send",
        "--json",
        "--socket",
        s(&srv.socket),
        "--from",
        "claude-main",
        "--to",
        "codex-web",
        "--kind",
        "AVISO",
        "--subject",
        "oi",
        "--body",
        "corpo",
        "--id",
        "cli-j",
    ]);
    assert_eq!(r.code, Some(0), "saida: {r:?}");
    let v = json_linha(&r.stdout);
    assert_eq!(v["id"], "cli-j");
    assert_eq!(v["seq"], 1);
    assert_eq!(v["state"], "registrado");
    assert_eq!(v["duplicate"], false);
}

#[test]
fn send_sem_id_gera_id_aleatorio_de_32_hex() {
    let (_d, srv) = sobe_padrao();
    let sock = s(&srv.socket);
    let mut ids = Vec::new();
    for _ in 0..2 {
        let r = cli(&[
            "send",
            "--socket",
            sock,
            "--from",
            "claude-main",
            "--to",
            "codex-web",
            "--kind",
            "AVISO",
            "--subject",
            "a",
            "--body",
            "b",
        ]);
        assert_eq!(r.code, Some(0), "saida: {r:?}");
        let id = r.stdout.split_whitespace().next().expect("id").to_string();
        assert_eq!(id.len(), 32, "id gerado: {id}");
        assert!(
            id.chars().all(|c| c.is_ascii_hexdigit()),
            "id nao e hex: {id}"
        );
        ids.push(id);
    }
    assert_ne!(
        ids[0], ids[1],
        "dois envios sem --id devem gerar ids diferentes"
    );
}

#[test]
fn send_repetido_com_mesmo_id_mostra_duplicate_true() {
    let (_d, srv) = sobe_padrao();
    let sock = s(&srv.socket);
    let a = envia_cli(sock, "cli-dup", "x", "y");
    let seq_a = a.stdout.split_whitespace().nth(1).unwrap().to_string();
    let b = envia_cli(sock, "cli-dup", "x", "y");
    assert_eq!(b.code, Some(0), "saida: {b:?}");
    // v1.1: `ID SEQ` e ao final `dup` quando duplicate
    let tokens: Vec<&str> = b.stdout.split_whitespace().collect();
    assert_eq!(
        tokens,
        vec!["cli-dup", seq_a.as_str(), "dup"],
        "saida: {:?}",
        b.stdout
    );
}

#[test]
fn send_com_erro_do_servidor_sai_1_no_formato_chatd_code() {
    let (_d, srv) = sobe_padrao();
    let r = envia_cli(s(&srv.socket), "cli-e1", "linha1\nlinha2", "corpo");
    assert_eq!(r.code, Some(1), "saida: {r:?}");
    assert!(
        r.stderr.starts_with("chatd: invalid_envelope: "),
        "stderr deve ser `chatd: <code>: <message>`, veio {:?}",
        r.stderr
    );
}

#[test]
fn send_thread_inexistente_sai_1_unknown_message() {
    let (_d, srv) = sobe_padrao();
    let r = cli(&[
        "send",
        "--socket",
        s(&srv.socket),
        "--from",
        "claude-main",
        "--to",
        "codex-web",
        "--kind",
        "RESPOSTA",
        "--subject",
        "x",
        "--body",
        "y",
        "--thread",
        "nao-existe",
    ]);
    assert_eq!(r.code, Some(1), "saida: {r:?}");
    assert!(
        r.stderr.starts_with("chatd: unknown_message:"),
        "stderr: {:?}",
        r.stderr
    );
}

#[test]
fn send_body_file_le_o_arquivo_inteiro() {
    let (d, srv) = sobe_padrao();
    let sock = s(&srv.socket);
    let arq = d.path().join("corpo.txt");
    fs::write(&arq, "linha 1\nação").unwrap();
    let r = cli(&[
        "send",
        "--socket",
        sock,
        "--from",
        "claude-main",
        "--to",
        "codex-web",
        "--kind",
        "AVISO",
        "--subject",
        "arq",
        "--body-file",
        s(&arq),
        "--id",
        "cli-arq",
    ]);
    assert_eq!(r.code, Some(0), "saida: {r:?}");
    let ms = mensagens_json(&inbox_cli(sock, &["--all", "--json"]).stdout);
    assert_eq!(achar(&ms, "cli-arq")["body"], "linha 1\nação");
}

#[test]
fn send_body_file_menos_le_a_entrada_padrao() {
    let (_d, srv) = sobe_padrao();
    let sock = s(&srv.socket);
    let r = cli_env(
        &[
            "send",
            "--socket",
            sock,
            "--from",
            "claude-main",
            "--to",
            "codex-web",
            "--kind",
            "AVISO",
            "--subject",
            "stdin",
            "--body-file",
            "-",
            "--id",
            "cli-stdin",
        ],
        &[],
        Some("vindo do stdin"),
    );
    assert_eq!(r.code, Some(0), "saida: {r:?}");
    let ms = mensagens_json(&inbox_cli(sock, &["--all", "--json"]).stdout);
    assert_eq!(achar(&ms, "cli-stdin")["body"], "vindo do stdin");
}

#[test]
fn send_uso_incorreto_sai_2() {
    let (_d, srv) = sobe_padrao();
    let sock = s(&srv.socket);
    let base = [
        "send",
        "--socket",
        sock,
        "--from",
        "claude-main",
        "--to",
        "codex-web",
        "--kind",
        "AVISO",
        "--subject",
        "x",
    ];
    // sem corpo
    assert_eq!(
        cli(&base).code,
        Some(2),
        "sem --body deve ser uso incorreto"
    );
    // corpo duplicado
    let mut duplo: Vec<&str> = base.to_vec();
    duplo.extend_from_slice(&["--body", "a", "--body-file", "-"]);
    assert_eq!(cli(&duplo).code, Some(2), "--body e --body-file juntos");
    // sem --to
    assert_eq!(
        cli(&[
            "send",
            "--socket",
            sock,
            "--from",
            "claude-main",
            "--kind",
            "AVISO",
            "--subject",
            "x",
            "--body",
            "y"
        ])
        .code,
        Some(2),
        "sem --to"
    );
    // flag desconhecida
    let mut bogus: Vec<&str> = base.to_vec();
    bogus.extend_from_slice(&["--body", "a", "--bogus", "x"]);
    assert_eq!(cli(&bogus).code, Some(2), "flag desconhecida");
}

#[test]
fn sem_comando_ou_comando_desconhecido_sai_2() {
    assert_eq!(cli(&[]).code, Some(2));
    assert_eq!(cli(&["fazer-nada"]).code, Some(2));
}

// ---------- inbox ----------

#[test]
fn inbox_lista_cabecalho_e_corpo_recuado() {
    let (_d, srv) = sobe_padrao();
    let sock = s(&srv.socket);
    envia_ok(&srv.socket, "i-1", "codex-web", "a");
    envia_ok(&srv.socket, "i-2", "codex-web", "b");
    // v1.1 (secao D): `SEQ ID KIND FROM>TO | SUBJECT`, e o corpo em linhas recuadas em 2 espacos.
    let r = inbox_cli(sock, &[]);
    assert_eq!(r.code, Some(0), "saida: {r:?}");
    let linhas: Vec<&str> = r.stdout.lines().collect();
    assert_eq!(
        linhas,
        vec![
            "1 i-1 AVISO claude-main>codex-web | assunto",
            "  a",
            "2 i-2 AVISO claude-main>codex-web | assunto",
            "  b",
        ],
        "saida: {:?}",
        r.stdout
    );
}

#[test]
fn inbox_sem_all_esconde_resultado_e_all_mostra() {
    let (_d, srv) = sobe_padrao();
    let sock = s(&srv.socket);
    envia_ok(&srv.socket, "ia-1", "codex-web", "a");
    assert_eq!(
        ack_cli(sock, "ia-1", "codex-web", "resultado", &[]).code,
        Some(0)
    );
    assert!(
        inbox_cli(sock, &[]).stdout.trim().is_empty(),
        "resultado nao deve aparecer sem --all"
    );
    let todos = inbox_cli(sock, &["--all", "--json"]);
    assert_eq!(todos.code, Some(0));
    // a linha compacta da v1.1 nao traz o estado; conferimos no JSON
    let ms = mensagens_json(&todos.stdout);
    assert_eq!(achar(&ms, "ia-1")["state"], "resultado");
}

#[test]
fn inbox_after_mostra_so_seq_maior() {
    let (_d, srv) = sobe_padrao();
    let sock = s(&srv.socket);
    envia_ok(&srv.socket, "af-1", "codex-web", "a");
    envia_ok(&srv.socket, "af-2", "codex-web", "b");
    envia_ok(&srv.socket, "af-3", "codex-web", "c");
    let r = inbox_cli(sock, &["--after", "1"]);
    // so as linhas de cabecalho (sem recuo) carregam o SEQ
    let seqs: Vec<&str> = r
        .stdout
        .lines()
        .filter(|l| !l.starts_with(' '))
        .map(|l| l.split_whitespace().next().unwrap_or(""))
        .collect();
    assert_eq!(seqs, vec!["2", "3"], "saida: {:?}", r.stdout);
}

#[test]
fn inbox_json_traz_campos_da_mensagem() {
    let (_d, srv) = sobe_padrao();
    let sock = s(&srv.socket);
    envia_ok(&srv.socket, "ij-1", "codex-web", "corpo json");
    let r = inbox_cli(sock, &["--json"]);
    assert_eq!(r.code, Some(0), "saida: {r:?}");
    let ms = mensagens_json(&r.stdout);
    let m = achar(&ms, "ij-1");
    assert_eq!(m["body"], "corpo json");
    assert_eq!(m["state"], "registrado");
    assert_eq!(m["from"], "claude-main");
    assert_eq!(m["kind"], "AVISO");
    assert_eq!(m["subject"], "assunto");
}

#[test]
fn inbox_vazia_sai_0() {
    let (_d, srv) = sobe_padrao();
    let r = inbox_cli(s(&srv.socket), &[]);
    assert_eq!(r.code, Some(0));
    assert!(r.stdout.trim().is_empty());
}

// ---------- ack ----------

#[test]
fn ack_aceito_depois_resultado_refused_muda_o_estado() {
    let (_d, srv) = sobe_padrao();
    let sock = s(&srv.socket);
    envia_ok(&srv.socket, "ak-1", "codex-web", "a");
    assert_eq!(
        ack_cli(sock, "ak-1", "codex-web", "aceito", &[]).code,
        Some(0)
    );
    let parcial = inbox_cli(sock, &["--all", "--json"]);
    assert_eq!(
        achar(&mensagens_json(&parcial.stdout), "ak-1")["state"],
        "aceito"
    );
    let r = ack_cli(
        sock,
        "ak-1",
        "codex-web",
        "resultado",
        &["--result", "refused", "--note", "recusado no teste"],
    );
    assert_eq!(r.code, Some(0), "saida: {r:?}");
    let ms = mensagens_json(&inbox_cli(sock, &["--all", "--json"]).stdout);
    let m = achar(&ms, "ak-1");
    assert_eq!(m["state"], "resultado");
    assert_eq!(m["result"], "refused");
    assert!(
        inbox_cli(sock, &[]).stdout.trim().is_empty(),
        "resolvida nao aparece no inbox padrao"
    );
}

#[test]
fn ack_de_outro_destinatario_sai_1_forbidden() {
    let (_d, srv) = sobe_padrao();
    let sock = s(&srv.socket);
    envia_ok(&srv.socket, "af-x", "codex-web", "a");
    let r = ack_cli(sock, "af-x", "claude-main", "aceito", &[]);
    assert_eq!(r.code, Some(1), "saida: {r:?}");
    assert!(
        r.stderr.starts_with("chatd: forbidden:"),
        "stderr: {:?}",
        r.stderr
    );
}

#[test]
fn ack_result_sem_stage_resultado_e_erro_e_nao_muda_estado() {
    // AMBIGUO: a spec diz que --result so vale com resultado, mas nao diz se e uso incorreto (2) ou erro do servidor (1).
    let (_d, srv) = sobe_padrao();
    let sock = s(&srv.socket);
    envia_ok(&srv.socket, "ar-1", "codex-web", "a");
    let r = ack_cli(sock, "ar-1", "codex-web", "aceito", &["--result", "done"]);
    assert_ne!(
        r.code,
        Some(0),
        "--result com aceito nao pode dar sucesso: {r:?}"
    );
    let ms = mensagens_json(&inbox_cli(sock, &["--all", "--json"]).stdout);
    assert_eq!(achar(&ms, "ar-1")["state"], "registrado");
}

#[test]
fn ack_note_aparece_no_json() {
    let (_d, srv) = sobe_padrao();
    let sock = s(&srv.socket);
    envia_ok(&srv.socket, "an-1", "codex-web", "a");
    let r = ack_cli(
        sock,
        "an-1",
        "codex-web",
        "resultado",
        &["--note", "feito ok"],
    );
    assert_eq!(r.code, Some(0), "saida: {r:?}");
    let ms = mensagens_json(&inbox_cli(sock, &["--all", "--json"]).stdout);
    assert_eq!(achar(&ms, "an-1")["note"], "feito ok");
}

#[test]
fn ack_id_inexistente_sai_1_unknown_message() {
    let (_d, srv) = sobe_padrao();
    let r = ack_cli(s(&srv.socket), "nao-existe", "codex-web", "aceito", &[]);
    assert_eq!(r.code, Some(1));
    assert!(
        r.stderr.starts_with("chatd: unknown_message:"),
        "stderr: {:?}",
        r.stderr
    );
}

// ---------- status ----------

#[test]
fn status_texto_sai_0_e_traz_estatisticas() {
    let (_d, srv) = sobe_padrao();
    let r = cli(&["status", "--socket", s(&srv.socket)]);
    assert_eq!(r.code, Some(0), "saida: {r:?}");
    assert!(r.stdout.contains("ok true"), "saida: {:?}", r.stdout);
    assert!(r.stdout.contains("head_seq "), "saida: {:?}", r.stdout);
    assert!(
        r.stdout.contains("mensagens total="),
        "saida: {:?}",
        r.stdout
    );
    assert!(r.stdout.contains("blobs count="), "saida: {:?}", r.stdout);
    // spec B.0: nenhum nome de agente ou destinatario na saida
    assert!(!r.stdout.contains("codex-web"), "saida: {:?}", r.stdout);
    assert!(!r.stdout.contains("claude-main"), "saida: {:?}", r.stdout);
}

#[test]
fn status_json_tem_a_estrutura_da_spec() {
    let (_d, srv) = sobe_padrao();
    let sock = s(&srv.socket);
    envia_ok(&srv.socket, "st-1", "codex-web", "a");
    let r = cli(&["status", "--json", "--socket", sock]);
    assert_eq!(r.code, Some(0), "saida: {r:?}");
    let v: Value = serde_json::from_str(r.stdout.trim()).expect("JSON do status");
    assert_eq!(v["v"], 1);
    assert_eq!(v["ok"], true);
    assert!(v["version"].is_string(), "version: {v}");
    assert!(v["uptime_s"].is_u64(), "uptime_s: {v}");
    assert_eq!(v["head_seq"], 1);
    assert_eq!(v["messages"]["total"], 1);
    assert_eq!(v["messages"]["registered"], 1);
    assert_eq!(v["messages"]["accepted"], 0);
    assert_eq!(v["messages"]["resulted"], 0);
    assert!(v["bells"]["sent"].is_u64(), "bells: {v}");
    assert!(v["bells"]["failed"].is_u64(), "bells: {v}");
    assert!(v["connections"].is_u64(), "connections: {v}");
    assert_eq!(v["blobs"]["count"], 0);
    assert_eq!(v["blobs"]["bytes"], 0);
    // sem nomes de agente/destinatario/perfil/painel
    assert!(v.get("recipients").is_none(), "status sem recipients: {v}");
    assert!(!r.stdout.contains("codex-web"), "vazou nome: {}", r.stdout);
}

// ---------- resolucao do socket ----------

#[test]
fn comandos_cliente_sem_socket_sai_2() {
    assert_eq!(cli(&["status"]).code, Some(2));
    assert_eq!(cli(&["inbox", "--as", "codex-web"]).code, Some(2));
    assert_eq!(
        cli(&["ack", "--id", "x", "--as", "codex-web", "--stage", "aceito"]).code,
        Some(2)
    );
    assert_eq!(
        cli(&[
            "send",
            "--from",
            "claude-main",
            "--to",
            "codex-web",
            "--kind",
            "AVISO",
            "--subject",
            "x",
            "--body",
            "y"
        ])
        .code,
        Some(2)
    );
}

#[test]
fn chatd_socket_no_ambiente_e_usado() {
    let (_d, srv) = sobe_padrao();
    let r = cli_env(&["status"], &[("CHATD_SOCKET", s(&srv.socket))], None);
    assert_eq!(r.code, Some(0), "saida: {r:?}");
}

#[test]
fn state_dir_por_flag_resolve_chatd_sock_dentro_dele() {
    let (_d, srv) = sobe_padrao();
    let r = cli(&["status", "--state-dir", s(&srv.state)]);
    assert_eq!(r.code, Some(0), "saida: {r:?}");
}

#[test]
fn chatd_state_dir_no_ambiente_resolve_o_socket() {
    let (_d, srv) = sobe_padrao();
    let r = cli_env(&["status"], &[("CHATD_STATE_DIR", s(&srv.state))], None);
    assert_eq!(r.code, Some(0), "saida: {r:?}");
}

#[test]
fn flag_socket_vence_o_ambiente() {
    let (_d, srv) = sobe_padrao();
    let r = cli_env(
        &["status", "--socket", s(&srv.socket)],
        &[("CHATD_SOCKET", "/nonexistent-dir/x.sock")],
        None,
    );
    assert_eq!(r.code, Some(0), "saida: {r:?}");
}

#[test]
fn socket_inexistente_nao_e_uso_incorreto() {
    // atualizado pela v1.6 (Parte B, regra 1 e R2): socket inexistente, depois das repeticoes, sai 4 (daemon
    // indisponivel). O objetivo do teste e so provar que nao e erro de uso (!= 2).
    let r = cli(&["status", "--socket", "/nonexistent-dir/nao-existe.sock"]);
    assert_eq!(r.code, Some(4), "saida: {r:?}");
}

// ---------- serve: linha de comando e configuracao ----------

#[test]
fn serve_sem_config_sai_2() {
    let d = tmp();
    let r = cli(&["serve", "--state-dir", s(&d.path().join("state"))]);
    assert_eq!(r.code, Some(2), "saida: {r:?}");
}

#[test]
fn serve_config_com_campo_desconhecido_nao_sobe() {
    let d = tmp();
    let mut cfg = config(Path::new("/nonexistent/herdr"), 50, 600);
    cfg["campo_inventado"] = json!(1);
    let c = escreve_config(d.path(), &cfg);
    let st = d.path().join("state");
    match tenta(
        &["serve", "--state-dir", s(&st), "--config", s(&c)],
        &[],
        &st,
        &st.join("chatd.sock"),
    ) {
        Ok(_) => panic!("servidor subiu com campo desconhecido na configuracao"),
        Err(e) => assert_eq!(e.code, Some(1), "saida: {e:?}"),
    }
}

#[test]
fn serve_config_json_invalido_nao_sobe() {
    let d = tmp();
    let c = d.path().join("config.json");
    fs::write(&c, "{ nao fecha").unwrap();
    let st = d.path().join("state");
    match tenta(
        &["serve", "--state-dir", s(&st), "--config", s(&c)],
        &[],
        &st,
        &st.join("chatd.sock"),
    ) {
        Ok(_) => panic!("servidor subiu com JSON invalido"),
        Err(e) => assert_eq!(e.code, Some(1), "saida: {e:?}"),
    }
}

#[test]
fn serve_bell_poll_abaixo_do_minimo_nao_sobe() {
    // AMBIGUO: a spec diz "minimo 50" para bell_poll_ms, sem dizer se valor menor e recusado ou ajustado. Lemos como recusa.
    let d = tmp();
    let cfg = config(Path::new("/nonexistent/herdr"), 10, 600);
    let c = escreve_config(d.path(), &cfg);
    let st = d.path().join("state");
    match tenta(
        &["serve", "--state-dir", s(&st), "--config", s(&c)],
        &[],
        &st,
        &st.join("chatd.sock"),
    ) {
        Ok(_) => panic!("bell_poll_ms 10 foi aceito"),
        Err(e) => assert_eq!(e.code, Some(1), "saida: {e:?}"),
    }
}

#[test]
fn serve_com_socket_custom_nao_cria_chatd_sock_padrao() {
    let d = tmp();
    let cfg = config_padrao(d.path());
    let st = d.path().join("state");
    let custom = d.path().join("meu.sock");
    let srv = tenta(
        &[
            "serve",
            "--state-dir",
            s(&st),
            "--config",
            s(&cfg),
            "--socket",
            s(&custom),
        ],
        &[],
        &st,
        &custom,
    )
    .unwrap_or_else(|e| panic!("nao subiu: {e:?}"));
    assert!(custom.exists(), "socket custom nao criado");
    assert!(
        !st.join("chatd.sock").exists(),
        "socket padrao nao deveria existir"
    );
    let r = cli(&["status", "--socket", s(&custom)]);
    assert_eq!(r.code, Some(0), "saida: {r:?}");
    drop(srv);
}

#[test]
fn serve_com_state_dir_pelo_ambiente() {
    // AMBIGUO: a spec manda resolver o socket por CHATD_STATE_DIR; nao fica claro se o `serve` aceita o diretorio pelo ambiente no lugar de --state-dir.
    let d = tmp();
    let cfg = config_padrao(d.path());
    let st = d.path().join("state");
    let r = tenta(
        &["serve", "--config", s(&cfg)],
        &[("CHATD_STATE_DIR", s(&st))],
        &st,
        &st.join("chatd.sock"),
    );
    assert!(r.is_ok(), "serve com CHATD_STATE_DIR nao ficou pronto");
}

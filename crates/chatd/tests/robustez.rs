//! Lacunas achadas por mutacao (spec v1.1): log com seq/id invalidos, campo desconhecido,
//! jsonrpc/id invalidos e limite de 1 MiB exato. Caixa preta: binario e socket.
mod common;

use common::*;
use serde_json::{Value, json};
use std::fs;
use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;
use std::path::Path;
use std::thread;
use std::time::Duration;

// ---------- persistencia: seq e id no log ----------

fn cfg_de(d: &Path) -> std::path::PathBuf {
    config_padrao(d)
}

fn tenta_em(d: &Path) -> Result<Servidor, Saida> {
    let cfg = cfg_de(d);
    let st = d.join("state");
    let sock = st.join("chatd.sock");
    tenta(
        &["serve", "--state-dir", s(&st), "--config", s(&cfg)],
        &[],
        &st,
        &sock,
    )
}

/// Gera tres eventos `registered` reais (linhas do proprio servidor) e devolve-os.
fn gera_registros(d: &Path) -> Vec<Value> {
    let mut srv = sobe(&d.join("state"), &cfg_de(d));
    for id in ["base-1", "base-2", "base-3"] {
        envia_ok(&srv.socket, id, "codex-web", "corpo");
    }
    assert!(srv.para().is_some(), "servidor nao saiu com SIGTERM");
    drop(srv);
    let regs: Vec<Value> = log_linhas(&d.join("state"))
        .iter()
        .map(|l| serde_json::from_str::<Value>(l).expect("linha do log"))
        .filter(|v| v["type"] == "registered")
        .collect();
    assert_eq!(regs.len(), 3, "esperava exatamente 3 registros");
    regs
}

fn escreve_log(d: &Path, linhas: &[Value]) {
    let mut t = String::new();
    for l in linhas {
        t.push_str(&l.to_string());
        t.push('\n');
    }
    fs::write(d.join("state/log.jsonl"), t).expect("escrever log");
}

fn com_seq(v: &Value, n: i64) -> Value {
    let mut c = v.clone();
    c["seq"] = json!(n);
    c
}

/// Controle: o log com os tres registros originais sobe. Depois para o servidor.
fn controle_sobe(d: &Path, regs: &[Value]) {
    escreve_log(d, regs);
    let mut srv = tenta_em(d).unwrap_or_else(|e| panic!("log valido deveria subir: {e:?}"));
    assert!(srv.para().is_some());
}

#[test]
fn controle_log_original_com_seq_correto_sobe() {
    let d = tmp();
    let regs = gera_registros(d.path());
    controle_sobe(d.path(), &regs);
}

#[test]
fn log_com_seq_repetido_recusa_subir() {
    let d = tmp();
    let regs = gera_registros(d.path());
    // seq 1, 2, 2 (ids distintos)
    escreve_log(
        d.path(),
        &[regs[0].clone(), regs[1].clone(), com_seq(&regs[2], 2)],
    );
    match tenta_em(d.path()) {
        Ok(_) => panic!("servidor subiu com seq repetido no log"),
        Err(e) => assert_eq!(e.code, Some(1), "saida: {e:?}"),
    }
}

#[test]
fn log_com_seq_para_tras_recusa_subir() {
    let d = tmp();
    let regs = gera_registros(d.path());
    // seq 1, 2, 1 (ids distintos)
    escreve_log(
        d.path(),
        &[regs[0].clone(), regs[1].clone(), com_seq(&regs[2], 1)],
    );
    match tenta_em(d.path()) {
        Ok(_) => panic!("servidor subiu com seq voltando no log"),
        Err(e) => assert_eq!(e.code, Some(1), "saida: {e:?}"),
    }
}

#[test]
fn log_com_mesmo_id_registrado_duas_vezes_recusa_subir() {
    let d = tmp();
    let regs = gera_registros(d.path());
    // seq 1, 2, 3, mas o terceiro registro repete o id do primeiro
    escreve_log(
        d.path(),
        &[regs[0].clone(), regs[1].clone(), com_seq(&regs[0], 3)],
    );
    match tenta_em(d.path()) {
        Ok(_) => panic!("servidor subiu com id de mensagem registrado duas vezes"),
        Err(e) => assert_eq!(e.code, Some(1), "saida: {e:?}"),
    }
}

// ---------- campo desconhecido em params ----------

#[test]
fn campo_desconhecido_em_message_send_e_invalid_envelope() {
    let (_d, srv) = sobe_padrao();
    let mut p = msg("ce-send", "claude-main", "codex-web", "AVISO", "s", "b");
    p["zz_extra"] = json!(1);
    let r = envia(&srv.socket, p);
    assert_eq!(cod(&r), "invalid_envelope");
    assert_eq!(caminho(&r), "/zz_extra");
}

#[test]
fn campo_desconhecido_em_message_list_e_invalid_envelope() {
    let (_d, srv) = sobe_padrao();
    let r = rpc(
        &srv.socket,
        "message.list",
        json!({"v": 1, "to": "codex-web", "zz_extra": 1}),
    );
    assert_eq!(cod(&r), "invalid_envelope");
    assert_eq!(caminho(&r), "/zz_extra");
}

#[test]
fn campo_desconhecido_em_message_ack_e_invalid_envelope() {
    let (_d, srv) = sobe_padrao();
    envia_ok(&srv.socket, "ca-x", "codex-web", "a");
    let r = rpc(
        &srv.socket,
        "message.ack",
        json!({"v": 1, "id": "ca-x", "by": "codex-web", "stage": "aceito", "zz_extra": 1}),
    );
    assert_eq!(cod(&r), "invalid_envelope");
    assert_eq!(caminho(&r), "/zz_extra");
}

#[test]
fn campo_desconhecido_em_events_resume_e_invalid_envelope() {
    let (_d, srv) = sobe_padrao();
    let mut c = Conexao::abre(&srv.socket);
    let r = c.chama(
        "events.resume",
        json!({"v": 1, "last_seq": 0, "zz_extra": 1}),
    );
    assert_eq!(cod(&r), "invalid_envelope");
    assert_eq!(caminho(&r), "/zz_extra");
}

#[test]
fn campo_desconhecido_em_status_e_invalid_envelope() {
    let (_d, srv) = sobe_padrao();
    let r = rpc(&srv.socket, "status", json!({"v": 1, "zz_extra": 1}));
    assert_eq!(cod(&r), "invalid_envelope");
    assert_eq!(caminho(&r), "/zz_extra");
}

// ---------- jsonrpc e id ----------

fn linha_crua(sock: &Path, linha: &str) -> Value {
    let mut c = Conexao::abre(sock);
    c.envia_bruto(linha);
    c.le(Duration::from_secs(5)).expect("resposta")
}

#[test]
fn requisicao_sem_jsonrpc_2_0_e_invalid_request() {
    let (_d, srv) = sobe_padrao();
    let r = linha_crua(
        &srv.socket,
        r#"{"id":1,"method":"status","params":{"v":1}}"#,
    );
    assert_eq!(cod(&r), "invalid_request");
}

#[test]
fn requisicao_com_jsonrpc_1_0_e_invalid_request() {
    let (_d, srv) = sobe_padrao();
    let r = linha_crua(
        &srv.socket,
        r#"{"jsonrpc":"1.0","id":1,"method":"status","params":{"v":1}}"#,
    );
    assert_eq!(cod(&r), "invalid_request");
}

#[test]
fn requisicao_com_id_null_e_invalid_request_com_id_null_na_resposta() {
    let (_d, srv) = sobe_padrao();
    let r = linha_crua(
        &srv.socket,
        r#"{"jsonrpc":"2.0","id":null,"method":"status","params":{"v":1}}"#,
    );
    assert_eq!(cod(&r), "invalid_request");
    assert!(r["id"].is_null(), "resposta deve ter id null: {r}");
}

#[test]
fn requisicao_sem_id_e_invalid_request_com_id_null_na_resposta() {
    let (_d, srv) = sobe_padrao();
    let r = linha_crua(
        &srv.socket,
        r#"{"jsonrpc":"2.0","method":"status","params":{"v":1}}"#,
    );
    assert_eq!(cod(&r), "invalid_request");
    assert!(r["id"].is_null(), "resposta deve ter id null: {r}");
}

// ---------- limite de 1 MiB exato ----------

/// Manda uma linha de `n` bytes de lixo (sem JSON) e le a primeira resposta.
fn linha_de_bytes(sock: &Path, n: usize) -> Value {
    let st = UnixStream::connect(sock).expect("conectar");
    st.set_read_timeout(Some(Duration::from_secs(10))).unwrap();
    let mut escrita = st.try_clone().unwrap();
    let corpo = vec![b'x'; n];
    // sem join: se o servidor fechar antes, a escrita falha e a thread termina
    thread::spawn(move || {
        let _ = escrita.write_all(&corpo);
        let _ = escrita.write_all(b"\n");
    });
    let mut leitor = BufReader::new(st);
    let mut linha = String::new();
    leitor.read_line(&mut linha).expect("ler resposta");
    serde_json::from_str(linha.trim()).expect("resposta JSON")
}

#[test]
fn linha_de_1_mib_exato_com_lixo_e_parse_error() {
    let (_d, srv) = sobe_padrao();
    let r = linha_de_bytes(&srv.socket, 1_048_576);
    assert_eq!(
        cod(&r),
        "parse_error",
        "1 MiB exato esta dentro do limite: {r}"
    );
    let l = res(&rpc(
        &srv.socket,
        "message.list",
        json!({"v": 1, "to": "codex-web"}),
    ));
    assert_eq!(l["head_seq"], 0, "servidor deve seguir respondendo");
}

#[test]
fn linha_de_1_mib_mais_um_byte_e_invalid_request() {
    let (_d, srv) = sobe_padrao();
    let r = linha_de_bytes(&srv.socket, 1_048_577);
    assert_eq!(cod(&r), "invalid_request", "acima de 1 MiB: {r}");
    let l = res(&rpc(
        &srv.socket,
        "message.list",
        json!({"v": 1, "to": "codex-web"}),
    ));
    assert_eq!(l["head_seq"], 0, "servidor deve seguir respondendo");
}

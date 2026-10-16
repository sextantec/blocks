//! Persistencia (spec secao 6), trava exclusiva, encerramento (secoes 4 e 6).
mod common;

use common::*;
use serde_json::{Value, json};
use std::fs;
use std::io::Write;
use std::path::Path;
use std::time::Duration;

fn cfg_de(d: &Path) -> std::path::PathBuf {
    config_padrao(d)
}

fn sobe_em(d: &Path) -> Servidor {
    let cfg = cfg_de(d);
    sobe(&d.join("state"), &cfg)
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

fn para_ok(srv: &mut Servidor) {
    assert!(srv.para().is_some(), "servidor nao saiu em 5 s com SIGTERM");
}

// ---------- reinicio ----------

#[test]
fn reinicio_preserva_mensagens_recibos_e_continua_o_seq() {
    let d = tmp();
    let mut srv = sobe_em(d.path());
    let sock = srv.socket.clone();
    envia_ok(&sock, "p-1", "codex-web", "um");
    envia_ok(&sock, "p-2", "codex-web", "dois");
    ack_ok(&sock, "p-1", "codex-web", "aceito");
    ack_json(
        &sock,
        json!({"v": 1, "id": "p-2", "by": "codex-web", "stage": "resultado", "result": "refused"}),
    );
    para_ok(&mut srv);
    drop(srv);

    let srv2 = sobe_em(d.path());
    let ms = lista(&srv2.socket, "codex-web");
    assert_eq!(ms.len(), 2, "mensagens sobrevivem ao reinicio: {ms:?}");
    assert_eq!(ms[0]["id"], "p-1");
    assert_eq!(ms[0]["seq"], 1);
    assert_eq!(ms[0]["state"], "aceito");
    assert_eq!(ms[1]["state"], "resultado");
    assert_eq!(ms[1]["result"], "refused");
    // registros (2) + ack aceito (1) + ack resultado (1) = ultimo seq 4; o proximo e 5
    let r = res(&envia(
        &srv2.socket,
        msg("p-3", "claude-main", "codex-web", "AVISO", "s", "b"),
    ));
    assert_eq!(r["seq"], 5, "seq deve continuar do ultimo evento gravado");
}

#[test]
fn reinicio_mantem_idempotencia() {
    let d = tmp();
    let mut srv = sobe_em(d.path());
    let a = envia_ok(&srv.socket, "idr-1", "codex-web", "x");
    para_ok(&mut srv);
    drop(srv);
    let srv2 = sobe_em(d.path());
    let r = res(&envia(
        &srv2.socket,
        msg("idr-1", "claude-main", "codex-web", "AVISO", "assunto", "x"),
    ));
    assert_eq!(r["duplicate"], true);
    assert_eq!(r["seq"], a["seq"]);
}

#[test]
fn reinicio_devolve_campainha_a_idle_wait() {
    let d = tmp();
    let h = Herdr::novo(d.path());
    h.estado("working");
    let cfg = escreve_config(d.path(), &config_req_json(&h.bin, 50, 600));
    let st = d.path().join("state");
    let mut srv = sobe(&st, &cfg);
    // auth obrigatoria: o estado da campainha so sai do mailbox.counts autenticado (spec B.0)
    let tc = token_new(&st, "claude-main", &d.path().join("claude.tok"));
    let tx = token_new(&st, "codex-web", &d.path().join("codex.tok"));
    res(&conexao_auth(&srv.socket, "claude-main", &tc).chama(
        "message.send",
        msg("rb-1", "claude-main", "codex-web", "AVISO", "s", "x"),
    ));
    para_ok(&mut srv);
    drop(srv);
    let srv2 = sobe(&st, &cfg);
    let caixa =
        res(&conexao_auth(&srv2.socket, "codex-web", &tx).chama("mailbox.counts", json!({"v": 1})));
    assert_eq!(caixa["bell"], "idle_wait");
    assert!(
        h.chamadas().is_empty(),
        "com estado working nao pode haver campainha"
    );
}

// ---------- truncamento e corrupcao ----------

#[test]
fn ultima_linha_truncada_e_descartada_com_aviso() {
    let d = tmp();
    let mut srv = sobe_em(d.path());
    envia_ok(&srv.socket, "tr-1", "codex-web", "a");
    envia_ok(&srv.socket, "tr-2", "codex-web", "b");
    // eventos da campainha tambem consomem seq (mesmo log): a referencia e o head_seq, nao o numero de mensagens
    let head_antes = status(&srv.socket)["head_seq"].as_u64().unwrap();
    para_ok(&mut srv);
    drop(srv);

    let log = d.path().join("state/log.jsonl");
    let mut f = fs::OpenOptions::new().append(true).open(&log).unwrap();
    f.write_all(b"{\"v\":1,\"seq\":5,\"ts\":\"2026-10-1")
        .unwrap();
    drop(f);

    let srv2 = sobe_em(d.path());
    assert!(
        espera(Duration::from_secs(2), || !srv2.stderr().is_empty()),
        "esperava aviso em stderr ao descartar a linha truncada"
    );
    let ms = lista(&srv2.socket, "codex-web");
    assert_eq!(ms.len(), 2, "linha truncada nao pode virar mensagem");
    let r = res(&envia(
        &srv2.socket,
        msg("tr-3", "claude-main", "codex-web", "AVISO", "s", "b"),
    ));
    // eventos da campainha (apos o reinicio) tambem consomem seq: o invariante e "sem lacunas no log",
    // e a linha truncada (seq 5) nao pode ter consumido numero
    let n = r["seq"].as_u64().unwrap();
    assert!(
        n > head_antes,
        "seq novo precisa ser maior que o head anterior"
    );
    let seqs: Vec<u64> = fs::read_to_string(&log)
        .unwrap()
        .lines()
        .map(|l| {
            serde_json::from_str::<serde_json::Value>(l).unwrap()["seq"]
                .as_u64()
                .unwrap()
        })
        .collect();
    let esperado: Vec<u64> = (1..=seqs.len() as u64).collect();
    assert_eq!(
        seqs, esperado,
        "o log nao pode ter lacuna de seq depois de descartar a cauda truncada"
    );
    assert!(seqs.contains(&n));
    drop(srv2);

    // o log precisa continuar legivel depois de novo reinicio (nao pode ter ficado colado a linha quebrada)
    let srv3 = sobe_em(d.path());
    assert_eq!(lista(&srv3.socket, "codex-web").len(), 3);
}

#[test]
fn ultima_linha_sem_quebra_final_mesmo_valida_e_descartada() {
    // AMBIGUO: a spec diz "sem \n final ... : descartar"; lemos que um evento completo sem \n final tambem e descartado.
    let d = tmp();
    let mut srv = sobe_em(d.path());
    envia_ok(&srv.socket, "sn-1", "codex-web", "a");
    envia_ok(&srv.socket, "sn-2", "codex-web", "b");
    para_ok(&mut srv);
    drop(srv);

    let log = d.path().join("state/log.jsonl");
    let txt = fs::read_to_string(&log).unwrap();
    fs::write(&log, txt.trim_end_matches('\n')).unwrap();

    let srv2 = sobe_em(d.path());
    let ms = lista(&srv2.socket, "codex-web");
    assert_eq!(ms.len(), 1, "ultima linha sem \\n deve ser descartada");
    assert_eq!(ms[0]["id"], "sn-1");
    // se o servidor escrever depois, nao pode colar a linha nova na anterior quebrada
    envia_ok(&srv2.socket, "sn-3", "codex-web", "c");
    drop(srv2);
    let srv3 = sobe_em(d.path());
    let ids: Vec<String> = lista(&srv3.socket, "codex-web")
        .iter()
        .map(|m| m["id"].as_str().unwrap().to_string())
        .collect();
    assert_eq!(ids, vec!["sn-1", "sn-3"]);
}

#[test]
fn linha_invalida_no_meio_recusa_subir_com_codigo_1() {
    let d = tmp();
    let mut srv = sobe_em(d.path());
    envia_ok(&srv.socket, "mi-1", "codex-web", "a");
    envia_ok(&srv.socket, "mi-2", "codex-web", "b");
    envia_ok(&srv.socket, "mi-3", "codex-web", "c");
    para_ok(&mut srv);
    drop(srv);
    let log = d.path().join("state/log.jsonl");
    let txt = fs::read_to_string(&log).unwrap();
    let mut linhas: Vec<&str> = txt.lines().collect();
    linhas.insert(1, "lixo que nao e json");
    fs::write(&log, linhas.join("\n") + "\n").unwrap();
    match tenta_em(d.path()) {
        Ok(_) => panic!("servidor subiu com linha invalida no meio do log"),
        Err(e) => assert_eq!(e.code, Some(1), "saida: {e:?}"),
    }
}

#[test]
fn seq_fora_de_ordem_recusa_subir() {
    let d = tmp();
    let mut srv = sobe_em(d.path());
    envia_ok(&srv.socket, "fo-1", "codex-web", "a");
    envia_ok(&srv.socket, "fo-2", "codex-web", "b");
    para_ok(&mut srv);
    drop(srv);
    let log = d.path().join("state/log.jsonl");
    let txt = fs::read_to_string(&log).unwrap();
    let mut linhas: Vec<&str> = txt.lines().collect();
    linhas.swap(0, 1);
    fs::write(&log, linhas.join("\n") + "\n").unwrap();
    match tenta_em(d.path()) {
        Ok(_) => panic!("servidor subiu com seq fora de ordem"),
        Err(e) => assert_eq!(e.code, Some(1), "saida: {e:?}"),
    }
}

#[test]
fn lacuna_de_seq_recusa_subir() {
    let d = tmp();
    let mut srv = sobe_em(d.path());
    envia_ok(&srv.socket, "lc-1", "codex-web", "a");
    envia_ok(&srv.socket, "lc-2", "codex-web", "b");
    envia_ok(&srv.socket, "lc-3", "codex-web", "c");
    para_ok(&mut srv);
    drop(srv);
    let log = d.path().join("state/log.jsonl");
    let txt = fs::read_to_string(&log).unwrap();
    let mut linhas: Vec<&str> = txt.lines().collect();
    linhas.remove(1);
    fs::write(&log, linhas.join("\n") + "\n").unwrap();
    match tenta_em(d.path()) {
        Ok(_) => panic!("servidor subiu com lacuna de seq"),
        Err(e) => assert_eq!(e.code, Some(1), "saida: {e:?}"),
    }
}

#[test]
fn log_antigo_nunca_e_reescrito() {
    let d = tmp();
    let srv = sobe_em(d.path());
    envia_ok(&srv.socket, "nr-1", "codex-web", "a");
    let antes = log_bytes(&srv.state);
    envia_ok(&srv.socket, "nr-2", "codex-web", "b");
    ack_ok(&srv.socket, "nr-1", "codex-web", "resultado");
    let depois = log_bytes(&srv.state);
    assert!(depois.len() > antes.len());
    assert!(
        depois.starts_with(&antes),
        "bytes ja gravados no log foram alterados"
    );
}

// ---------- trava exclusiva ----------

#[test]
fn segundo_servidor_no_mesmo_diretorio_sai_1_e_o_primeiro_segue() {
    let d = tmp();
    let a = sobe_em(d.path());
    match tenta_em(d.path()) {
        Ok(_) => panic!("segundo servidor subiu no mesmo diretorio"),
        Err(e) => assert_eq!(e.code, Some(1), "saida: {e:?}"),
    }
    // o primeiro continua atendendo (o segundo nao pode ter removido o socket dele)
    let st = status(&a.socket);
    assert_eq!(st["head_seq"], 0);
}

#[test]
fn trava_e_liberada_apos_encerramento_limpo() {
    let d = tmp();
    let mut a = sobe_em(d.path());
    para_ok(&mut a);
    drop(a);
    assert!(
        tenta_em(d.path()).is_ok(),
        "apos encerrar, novo servidor deve subir"
    );
}

// ---------- encerramento ----------

#[test]
fn sigterm_encerra_com_codigo_0() {
    let (_d, mut srv) = sobe_padrao();
    assert_eq!(
        srv.para(),
        Some(0),
        "SIGTERM deve encerrar limpo com codigo 0"
    );
}

#[test]
fn sigint_encerra_com_codigo_0() {
    let (_d, mut srv) = sobe_padrao();
    assert_eq!(
        srv.sinal(libc::SIGINT),
        Some(0),
        "SIGINT deve encerrar limpo com codigo 0"
    );
}

#[test]
fn log_criado_no_disco_tem_linhas_json_de_evento() {
    let (_d, srv) = sobe_padrao();
    envia_ok(&srv.socket, "js-1", "codex-web", "a");
    for l in log_linhas(&srv.state) {
        let v: Value = serde_json::from_str(&l).expect("linha do log e JSON");
        assert!(
            v.get("seq").is_some() && v.get("type").is_some(),
            "evento sem seq/type: {v}"
        );
    }
}

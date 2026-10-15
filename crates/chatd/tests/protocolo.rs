//! Protocolo no socket (spec secao 5): message.send, message.list, message.ack, events.resume, status, erros.
mod common;

use common::*;
use serde_json::{Value, json};
use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;
use std::path::Path;
use std::thread;
use std::time::Duration;

fn envio_alterado(sock: &Path, id: &str, altera: impl FnOnce(&mut Value)) -> Value {
    let mut p = msg(id, "claude-main", "codex-web", "AVISO", "assunto", "corpo");
    altera(&mut p);
    envia(sock, p)
}

fn ack_p(sock: &Path, p: Value) -> Value {
    ack_json(sock, p)
}

fn resume_bruto(sock: &Path, last: Value) -> (Conexao, Value) {
    let mut c = Conexao::abre(sock);
    let r = c.chama("events.resume", json!({"v": 1, "last_seq": last}));
    (c, r)
}

fn achar(ms: &[Value], id: &str) -> Value {
    ms.iter()
        .find(|m| m["id"] == id)
        .cloned()
        .unwrap_or_else(|| panic!("mensagem {id} ausente"))
}

// ---------- message.send: campos ----------

#[test]
fn resposta_do_envio_tem_recibo_registrado() {
    let (_d, srv) = sobe_padrao();
    let r = envia(
        &srv.socket,
        msg("rec-1", "claude-main", "codex-web", "AVISO", "oi", "corpo"),
    );
    let v = res(&r);
    assert_eq!(v["v"], 1);
    assert_eq!(v["id"], "rec-1");
    assert_eq!(v["seq"], 1);
    assert_eq!(v["state"], "registrado");
    assert_eq!(v["duplicate"], false);
    assert!(v["registered_at"].is_string(), "registered_at: {v}");
}

#[test]
fn id_aceita_de_1_a_128_caracteres_e_recusa_vazio_e_129() {
    let (_d, srv) = sobe_padrao();
    let sock = srv.socket.clone();
    assert!(
        !is_err(&envio_alterado(&sock, "a", |_| {})),
        "id de 1 caractere"
    );
    assert!(
        !is_err(&envio_alterado(&sock, &"b".repeat(128), |_| {})),
        "id de 128 caracteres"
    );
    let r = envio_alterado(&sock, "", |_| {});
    assert_eq!(cod(&r), "invalid_envelope");
    assert_eq!(caminho(&r), "/id");
    let r = envio_alterado(&sock, &"c".repeat(129), |_| {});
    assert_eq!(cod(&r), "invalid_envelope");
    assert_eq!(caminho(&r), "/id");
}

#[test]
fn id_aceita_so_o_conjunto_de_caracteres_da_spec() {
    let (_d, srv) = sobe_padrao();
    let sock = srv.socket.clone();
    for ok in ["a.b_c:d/e-f", "ID-1", "Z9"] {
        assert!(
            !is_err(&envio_alterado(&sock, ok, |_| {})),
            "id {ok} deveria ser aceito"
        );
    }
    for ruim in ["a b", "a@b", "aç", "a#b", "a\tb"] {
        let r = envio_alterado(&sock, ruim, |_| {});
        assert_eq!(cod(&r), "invalid_envelope", "id {ruim:?}");
        assert_eq!(caminho(&r), "/id", "id {ruim:?}");
    }
}

#[test]
fn from_limites_de_nome_32_e_33_sem_exigir_destinatario() {
    let (_d, srv) = sobe_padrao();
    let sock = srv.socket.clone();
    // nome valido que nao esta na configuracao: aceito, porque from nao precisa ser destinatario
    assert!(!is_err(
        &envio_alterado(&sock, "f-ok-1", |p| p["from"] = json!("desconhecido"))
    ));
    assert!(!is_err(
        &envio_alterado(&sock, "f-ok-2", |p| p["from"] = json!("a".repeat(32)))
    ));
    for (i, ruim) in [
        "a".repeat(33),
        String::new(),
        "Codex".to_string(),
        "../x".to_string(),
    ]
    .into_iter()
    .enumerate()
    {
        let id = format!("f-ruim-{i}");
        let r = envio_alterado(&sock, &id, |p| p["from"] = json!(ruim));
        assert_eq!(cod(&r), "invalid_envelope", "from {ruim:?}");
        assert_eq!(caminho(&r), "/from", "from {ruim:?}");
    }
}

#[test]
fn to_precisa_existir_na_configuracao() {
    let (_d, srv) = sobe_padrao();
    let sock = srv.socket.clone();
    let r = envio_alterado(&sock, "t-1", |p| p["to"] = json!("ninguem"));
    assert_eq!(cod(&r), "unknown_recipient");
    let r = envio_alterado(&sock, "t-2", |p| p["to"] = json!("a".repeat(32)));
    assert_eq!(cod(&r), "unknown_recipient");
}

#[test]
fn to_com_formato_invalido_e_recusado() {
    // AMBIGUO: a spec nao diz se nome com formato invalido em `to` vira invalid_envelope ou unknown_recipient.
    let (_d, srv) = sobe_padrao();
    let r = envio_alterado(&srv.socket, "tf-1", |p| p["to"] = json!("Codex Web"));
    let c = cod(&r);
    assert!(
        c == "invalid_envelope" || c == "unknown_recipient",
        "codigo: {c}"
    );
}

#[test]
fn kind_aceita_os_cinco_valores_e_recusa_outro() {
    let (_d, srv) = sobe_padrao();
    let sock = srv.socket.clone();
    for k in ["AVISO", "PEDIDO", "PERGUNTA", "RESPOSTA", "ACK"] {
        let id = format!("k-{k}");
        let r = envio_alterado(&sock, &id, |p| p["kind"] = json!(k));
        assert!(!is_err(&r), "kind {k} deveria ser aceito: {r}");
    }
    for ruim in ["aviso", ""] {
        let r = envio_alterado(&sock, &format!("k-ruim-{}", ruim.len()), |p| {
            p["kind"] = json!(ruim)
        });
        assert_eq!(cod(&r), "invalid_envelope");
        assert_eq!(caminho(&r), "/kind");
    }
}

#[test]
fn subject_limites_de_1_e_200_caracteres() {
    let (_d, srv) = sobe_padrao();
    let sock = srv.socket.clone();
    assert!(!is_err(
        &envio_alterado(&sock, "s-1", |p| p["subject"] = json!("a"))
    ));
    assert!(!is_err(
        &envio_alterado(&sock, "s-2", |p| p["subject"] = json!("a".repeat(200)))
    ));
    // 200 caracteres com 2 bytes cada: o limite e em caracteres, nao em bytes
    assert!(!is_err(
        &envio_alterado(&sock, "s-3", |p| p["subject"] = json!("á".repeat(200)))
    ));
    let r = envio_alterado(&sock, "s-4", |p| p["subject"] = json!(""));
    assert_eq!(cod(&r), "invalid_envelope");
    assert_eq!(caminho(&r), "/subject");
    let r = envio_alterado(&sock, "s-5", |p| p["subject"] = json!("a".repeat(201)));
    assert_eq!(cod(&r), "invalid_envelope");
    assert_eq!(caminho(&r), "/subject");
}

#[test]
fn subject_recusa_quebra_de_linha() {
    let (_d, srv) = sobe_padrao();
    let sock = srv.socket.clone();
    for (i, ruim) in ["a\nb", "a\rb"].iter().enumerate() {
        let r = envio_alterado(&sock, &format!("sq-{i}"), |p| p["subject"] = json!(ruim));
        assert_eq!(cod(&r), "invalid_envelope", "subject {ruim:?}");
        assert_eq!(caminho(&r), "/subject");
    }
}

#[test]
fn body_limite_de_16384_bytes_utf8() {
    let (_d, srv) = sobe_padrao();
    let sock = srv.socket.clone();
    assert!(
        !is_err(&envio_alterado(&sock, "b-1", |p| p["body"] = json!(""))),
        "corpo vazio e valido"
    );
    assert!(!is_err(
        &envio_alterado(&sock, "b-2", |p| p["body"] = json!("a".repeat(16384)))
    ));
    let r = envio_alterado(&sock, "b-3", |p| p["body"] = json!("a".repeat(16385)));
    assert_eq!(cod(&r), "invalid_envelope");
    assert_eq!(caminho(&r), "/body");
    // 8192 caracteres de 2 bytes = 16384 bytes: no limite
    assert!(!is_err(
        &envio_alterado(&sock, "b-4", |p| p["body"] = json!("á".repeat(8192)))
    ));
    let r = envio_alterado(&sock, "b-5", |p| p["body"] = json!("á".repeat(8193)));
    assert_eq!(cod(&r), "invalid_envelope");
    assert_eq!(caminho(&r), "/body");
}

#[test]
fn body_precisa_ser_texto() {
    let (_d, srv) = sobe_padrao();
    let r = envio_alterado(&srv.socket, "bt-1", |p| p["body"] = json!(123));
    assert_eq!(cod(&r), "invalid_envelope");
    assert_eq!(caminho(&r), "/body");
}

#[test]
fn campo_obrigatorio_ausente_e_invalid_envelope() {
    let (_d, srv) = sobe_padrao();
    let r = envio_alterado(&srv.socket, "ca-1", |p| {
        p.as_object_mut().unwrap().remove("subject");
    });
    assert_eq!(cod(&r), "invalid_envelope");
}

#[test]
fn versao_diferente_de_1_e_unsupported_version() {
    let (_d, srv) = sobe_padrao();
    let r = envio_alterado(&srv.socket, "v-1", |p| p["v"] = json!(2));
    assert_eq!(cod(&r), "unsupported_version");
}

#[test]
fn thread_precisa_ser_mensagem_registrada() {
    let (_d, srv) = sobe_padrao();
    let sock = srv.socket.clone();
    let r = envio_alterado(&sock, "th-1", |p| p["thread"] = json!("nao-existe"));
    assert_eq!(cod(&r), "unknown_message");
    envia_ok(&sock, "th-raiz", "codex-web", "raiz");
    let r = envio_alterado(&sock, "th-2", |p| p["thread"] = json!("th-raiz"));
    assert!(!is_err(&r), "thread registrada deve ser aceita: {r}");
    let m = achar(&lista(&sock, "codex-web"), "th-2");
    assert_eq!(m["thread"], "th-raiz");
}

#[test]
fn thread_com_formato_invalido_e_recusada() {
    // AMBIGUO: a spec nao diz se formato invalido de thread e invalid_envelope ou unknown_message.
    let (_d, srv) = sobe_padrao();
    let r = envio_alterado(&srv.socket, "th-f", |p| p["thread"] = json!("a b"));
    let c = cod(&r);
    assert!(
        c == "invalid_envelope" || c == "unknown_message",
        "codigo: {c}"
    );
}

// ---------- idempotencia ----------

#[test]
fn reenvio_identico_devolve_duplicate_true_e_mesmo_seq() {
    let (_d, srv) = sobe_padrao();
    let sock = srv.socket.clone();
    let a = envia_ok(&sock, "id-1", "codex-web", "corpo");
    let head_antes = status(&sock)["head_seq"].clone();
    let b = envia_ok(&sock, "id-1", "codex-web", "corpo");
    assert_eq!(b["duplicate"], true);
    assert_eq!(b["seq"], a["seq"]);
    assert_eq!(b["state"], "registrado");
    assert_eq!(
        status(&sock)["head_seq"],
        head_antes,
        "reenvio nao pode gerar linha nova"
    );
}

#[test]
fn reenvio_depois_de_aceito_devolve_estado_atual() {
    let (_d, srv) = sobe_padrao();
    let sock = srv.socket.clone();
    let a = envia_ok(&sock, "ida-1", "codex-web", "corpo");
    ack_ok(&sock, "ida-1", "codex-web", "aceito");
    let b = envia_ok(&sock, "ida-1", "codex-web", "corpo");
    assert_eq!(b["duplicate"], true);
    assert_eq!(b["seq"], a["seq"]);
    assert_eq!(
        b["state"], "aceito",
        "devolve o estado atual, nao o inicial"
    );
}

#[test]
fn envelope_diferente_e_envelope_mismatch_e_nada_muda() {
    let (_d, srv) = sobe_padrao();
    let sock = srv.socket.clone();
    envia_ok(&sock, "mm-1", "codex-web", "original");
    let head = status(&sock)["head_seq"].clone();
    let linhas_antes = log_linhas(&srv.state).len();
    let variacoes: Vec<(&str, Value)> = vec![
        ("body", json!("outro corpo")),
        ("from", json!("codex-web")),
        ("to", json!("claude-main")),
        ("kind", json!("PEDIDO")),
        ("subject", json!("outro assunto")),
    ];
    for (campo, valor) in variacoes {
        let r = envio_alterado(&sock, "mm-1", |p| p[campo] = valor.clone());
        assert_eq!(cod(&r), "envelope_mismatch", "campo {campo}");
    }
    assert_eq!(
        status(&sock)["head_seq"],
        head,
        "mismatch nao pode mudar head_seq"
    );
    assert_eq!(
        log_linhas(&srv.state).len(),
        linhas_antes,
        "mismatch nao pode gerar linha no log"
    );
    let m = achar(&lista(&sock, "codex-web"), "mm-1");
    assert_eq!(m["body"], "original");
}

// ---------- seq ----------

#[test]
fn seq_comeca_em_1_e_o_log_nao_tem_lacunas() {
    let (_d, srv) = sobe_padrao();
    let sock = srv.socket.clone();
    envia_ok(&sock, "sq-a", "codex-web", "1");
    envia_ok(&sock, "sq-b", "claude-main", "2");
    ack_ok(&sock, "sq-a", "codex-web", "aceito");
    envia_ok(&sock, "sq-c", "codex-web", "3");
    let linhas = log_linhas(&srv.state);
    let head = status(&sock)["head_seq"].as_i64().unwrap();
    assert_eq!(linhas.len() as i64, head, "uma linha por evento");
    for (i, l) in linhas.iter().enumerate() {
        let v: Value = serde_json::from_str(l).expect("linha do log");
        assert_eq!(
            v["seq"],
            (i + 1) as i64,
            "seq fora da sequencia na linha {i}"
        );
    }
}

#[test]
fn envios_concorrentes_recebem_seq_unicos_e_contiguos() {
    let (_d, srv) = sobe_padrao();
    let sock = srv.socket.clone();
    let hs: Vec<_> = (0..16)
        .map(|i| {
            let sock = sock.clone();
            thread::spawn(move || {
                let id = format!("conc-{i}");
                seq(&res(&envia(
                    &sock,
                    msg(&id, "claude-main", "codex-web", "AVISO", "s", "b"),
                )))
            })
        })
        .collect();
    let mut seqs: Vec<i64> = hs.into_iter().map(|h| h.join().unwrap()).collect();
    seqs.sort();
    assert_eq!(seqs, (1..=16).collect::<Vec<i64>>());
}

// ---------- message.list ----------

#[test]
fn list_ordena_por_seq_e_filtra_o_destinatario() {
    let (_d, srv) = sobe_padrao();
    let sock = srv.socket.clone();
    envia_ok(&sock, "l-1", "codex-web", "a");
    envia_ok(&sock, "l-2", "claude-main", "b");
    envia_ok(&sock, "l-3", "codex-web", "c");
    let ms = lista(&sock, "codex-web");
    let seqs: Vec<i64> = ms.iter().map(seq).collect();
    assert_eq!(seqs, vec![1, 3]);
    assert!(ms.iter().all(|m| m["to"] == "codex-web"));
}

#[test]
fn list_after_seq_retorna_so_depois_do_cursor() {
    let (_d, srv) = sobe_padrao();
    let sock = srv.socket.clone();
    envia_ok(&sock, "la-1", "codex-web", "a");
    envia_ok(&sock, "la-2", "codex-web", "b");
    envia_ok(&sock, "la-3", "codex-web", "c");
    let r = res(&rpc(
        &sock,
        "message.list",
        json!({"v": 1, "to": "codex-web", "after_seq": 1}),
    ));
    let seqs: Vec<i64> = r["messages"].as_array().unwrap().iter().map(seq).collect();
    assert_eq!(seqs, vec![2, 3]);
}

#[test]
fn list_limit_padrao_50_e_limites_1_a_200() {
    let (_d, srv) = sobe_padrao();
    let sock = srv.socket.clone();
    for i in 0..52 {
        envia_ok(&sock, &format!("lim-{i}"), "codex-web", "x");
    }
    let padrao = res(&rpc(
        &sock,
        "message.list",
        json!({"v": 1, "to": "codex-web"}),
    ));
    assert_eq!(
        padrao["messages"].as_array().unwrap().len(),
        50,
        "limite padrao e 50"
    );
    assert_eq!(padrao["head_seq"], 52);
    let um = res(&rpc(
        &sock,
        "message.list",
        json!({"v": 1, "to": "codex-web", "limit": 1}),
    ));
    assert_eq!(um["messages"].as_array().unwrap().len(), 1);
    let todos = res(&rpc(
        &sock,
        "message.list",
        json!({"v": 1, "to": "codex-web", "limit": 200}),
    ));
    assert_eq!(todos["messages"].as_array().unwrap().len(), 52);
    for ruim in [0, 201] {
        let r = rpc(
            &sock,
            "message.list",
            json!({"v": 1, "to": "codex-web", "limit": ruim}),
        );
        assert_eq!(cod(&r), "invalid_envelope", "limit {ruim}");
        assert_eq!(caminho(&r), "/limit", "limit {ruim}");
    }
}

#[test]
fn list_unresolved_only_inclui_registrado_e_aceito_e_exclui_resultado() {
    let (_d, srv) = sobe_padrao();
    let sock = srv.socket.clone();
    envia_ok(&sock, "u-1", "codex-web", "a");
    envia_ok(&sock, "u-2", "codex-web", "b");
    ack_ok(&sock, "u-2", "codex-web", "aceito");
    envia_ok(&sock, "u-3", "codex-web", "c");
    ack_ok(&sock, "u-3", "codex-web", "aceito");
    ack_ok(&sock, "u-3", "codex-web", "resultado");
    let r = res(&rpc(
        &sock,
        "message.list",
        json!({"v": 1, "to": "codex-web", "unresolved_only": true}),
    ));
    let ids: Vec<&str> = r["messages"]
        .as_array()
        .unwrap()
        .iter()
        .map(|m| m["id"].as_str().unwrap())
        .collect();
    assert_eq!(ids, vec!["u-1", "u-2"]);
    let todos = lista(&sock, "codex-web");
    assert_eq!(todos.len(), 3, "unresolved_only padrao e false");
}

#[test]
fn list_campos_completos_com_nulls_quando_sem_valor() {
    let (_d, srv) = sobe_padrao();
    let sock = srv.socket.clone();
    envia_ok(&sock, "cp-1", "codex-web", "a");
    let m = achar(&lista(&sock, "codex-web"), "cp-1");
    for campo in [
        "v",
        "seq",
        "id",
        "from",
        "to",
        "kind",
        "subject",
        "body",
        "thread",
        "state",
        "registered_at",
        "accepted_at",
        "resulted_at",
        "result",
        "note",
    ] {
        assert!(m.get(campo).is_some(), "campo {campo} ausente: {m}");
    }
    assert!(m["thread"].is_null());
    assert!(m["accepted_at"].is_null());
    assert!(m["resulted_at"].is_null());
    assert!(m["result"].is_null());
    assert!(m["note"].is_null());
    assert_eq!(m["state"], "registrado");
    ack_ok(&sock, "cp-1", "codex-web", "aceito");
    let m = achar(&lista(&sock, "codex-web"), "cp-1");
    assert!(
        m["accepted_at"].is_string(),
        "accepted_at depois de aceito: {m}"
    );
    assert!(m["resulted_at"].is_null());
    assert!(m["result"].is_null());
}

#[test]
fn list_preserva_corpo_com_quebras_e_unicode() {
    let (_d, srv) = sobe_padrao();
    let sock = srv.socket.clone();
    let corpo = "linha1\nlinha2 ação 😀";
    envia_ok(&sock, "cu-1", "codex-web", corpo);
    assert_eq!(achar(&lista(&sock, "codex-web"), "cu-1")["body"], corpo);
}

#[test]
fn list_head_seq_e_global_e_seq_da_mensagem_e_o_do_registro() {
    let (_d, srv) = sobe_padrao();
    let sock = srv.socket.clone();
    let r1 = envia_ok(&sock, "hs-1", "codex-web", "a");
    envia_ok(&sock, "hs-2", "claude-main", "b");
    let r = res(&rpc(
        &sock,
        "message.list",
        json!({"v": 1, "to": "codex-web"}),
    ));
    assert_eq!(r["head_seq"], 2, "head_seq e o global do log");
    assert_eq!(
        achar(r["messages"].as_array().unwrap(), "hs-1")["seq"],
        r1["seq"]
    );
}

// ---------- message.ack ----------

#[test]
fn ack_aceito_registra_o_recibo() {
    let (_d, srv) = sobe_padrao();
    let sock = srv.socket.clone();
    envia_ok(&sock, "a-1", "codex-web", "x");
    let m = res(&ack(&sock, "a-1", "codex-web", "aceito"));
    assert_eq!(m["id"], "a-1");
    assert_eq!(m["state"], "aceito");
    assert!(m["accepted_at"].is_string());
    assert!(m["resulted_at"].is_null());
    assert_eq!(m["duplicate"], false);
}

#[test]
fn ack_de_quem_nao_e_destinatario_e_forbidden_sem_mudar_estado() {
    let (_d, srv) = sobe_padrao();
    let sock = srv.socket.clone();
    envia_ok(&sock, "a-2", "codex-web", "x");
    let r = ack(&sock, "a-2", "claude-main", "aceito");
    assert_eq!(cod(&r), "forbidden");
    assert_eq!(
        achar(&lista(&sock, "codex-web"), "a-2")["state"],
        "registrado"
    );
}

#[test]
fn ack_id_inexistente_e_unknown_message() {
    let (_d, srv) = sobe_padrao();
    assert_eq!(
        cod(&ack(&srv.socket, "nao-existe", "codex-web", "aceito")),
        "unknown_message"
    );
}

#[test]
fn ack_stage_invalido_e_invalid_envelope() {
    let (_d, srv) = sobe_padrao();
    let sock = srv.socket.clone();
    envia_ok(&sock, "a-3", "codex-web", "x");
    let r = ack(&sock, "a-3", "codex-web", "lido");
    assert_eq!(cod(&r), "invalid_envelope");
    assert_eq!(caminho(&r), "/stage");
}

#[test]
fn ack_aceito_repetido_e_idempotente() {
    let (_d, srv) = sobe_padrao();
    let sock = srv.socket.clone();
    envia_ok(&sock, "a-4", "codex-web", "x");
    let primeiro = res(&ack(&sock, "a-4", "codex-web", "aceito"));
    let segundo = res(&ack(&sock, "a-4", "codex-web", "aceito"));
    assert_eq!(segundo["duplicate"], true);
    assert_eq!(segundo["state"], "aceito");
    assert_eq!(segundo["accepted_at"], primeiro["accepted_at"]);
}

#[test]
fn ack_resultado_direto_de_registrado_com_refused_e_note() {
    let (_d, srv) = sobe_padrao();
    let sock = srv.socket.clone();
    envia_ok(&sock, "a-5", "codex-web", "x");
    let m = res(&ack_p(
        &sock,
        json!({"v": 1, "id": "a-5", "by": "codex-web", "stage": "resultado", "result": "refused", "note": "nao faco"}),
    ));
    assert_eq!(m["state"], "resultado");
    assert_eq!(m["result"], "refused");
    assert!(m["resulted_at"].is_string());
    assert_eq!(m["note"], "nao faco");
    assert_eq!(m["duplicate"], false);
}

#[test]
fn ack_resultado_sem_result_vale_done_e_failed_e_aceito() {
    let (_d, srv) = sobe_padrao();
    let sock = srv.socket.clone();
    envia_ok(&sock, "a-6", "codex-web", "x");
    assert_eq!(
        res(&ack(&sock, "a-6", "codex-web", "resultado"))["result"],
        "done"
    );
    envia_ok(&sock, "a-7", "codex-web", "x");
    let m = res(&ack_p(
        &sock,
        json!({"v": 1, "id": "a-7", "by": "codex-web", "stage": "resultado", "result": "failed"}),
    ));
    assert_eq!(m["result"], "failed");
}

#[test]
fn ack_aceito_depois_de_resultado_e_invalid_transition() {
    let (_d, srv) = sobe_padrao();
    let sock = srv.socket.clone();
    envia_ok(&sock, "a-8", "codex-web", "x");
    ack_ok(&sock, "a-8", "codex-web", "resultado");
    let r = ack(&sock, "a-8", "codex-web", "aceito");
    assert_eq!(cod(&r), "invalid_transition");
    assert_eq!(
        achar(&lista(&sock, "codex-web"), "a-8")["state"],
        "resultado"
    );
}

#[test]
fn ack_resultado_repetido_e_duplicate() {
    // AMBIGUO: a spec diz "repetir o mesmo estagio e idempotente (duplicate:true)"; nao diz se um resultado diferente no segundo envio e ignorado sem erro. Lemos como: sem erro e sem mudanca.
    let (_d, srv) = sobe_padrao();
    let sock = srv.socket.clone();
    envia_ok(&sock, "a-9", "codex-web", "x");
    ack_ok(&sock, "a-9", "codex-web", "resultado");
    let igual = res(&ack(&sock, "a-9", "codex-web", "resultado"));
    assert_eq!(igual["duplicate"], true);
    // spec 5.3 (esclarecida): result DIFERENTE do gravado e conflito, nao duplicata
    let outro = ack_p(
        &sock,
        json!({"v": 1, "id": "a-9", "by": "codex-web", "stage": "resultado", "result": "failed"}),
    );
    assert_eq!(cod(&outro), "invalid_transition");
    assert_eq!(
        achar(&lista(&sock, "codex-web"), "a-9")["result"],
        "done",
        "resultado nao pode mudar apos gravado"
    );
}

#[test]
fn ack_result_so_vale_com_stage_resultado() {
    // AMBIGUO: a spec diz "result so vale com resultado"; o codigo de erro nao esta fixado.
    let (_d, srv) = sobe_padrao();
    let sock = srv.socket.clone();
    envia_ok(&sock, "a-10", "codex-web", "x");
    let r = ack_p(
        &sock,
        json!({"v": 1, "id": "a-10", "by": "codex-web", "stage": "aceito", "result": "done"}),
    );
    assert!(is_err(&r), "result com aceito deve ser erro: {r}");
    assert_eq!(cod(&r), "invalid_envelope");
    assert_eq!(
        achar(&lista(&sock, "codex-web"), "a-10")["state"],
        "registrado"
    );
}

#[test]
fn ack_result_invalido_e_invalid_envelope() {
    let (_d, srv) = sobe_padrao();
    let sock = srv.socket.clone();
    envia_ok(&sock, "a-11", "codex-web", "x");
    let r = ack_p(
        &sock,
        json!({"v": 1, "id": "a-11", "by": "codex-web", "stage": "resultado", "result": "ok"}),
    );
    assert_eq!(cod(&r), "invalid_envelope");
    assert_eq!(caminho(&r), "/result");
}

#[test]
fn ack_note_ate_1024_caracteres() {
    let (_d, srv) = sobe_padrao();
    let sock = srv.socket.clone();
    envia_ok(&sock, "a-12", "codex-web", "x");
    envia_ok(&sock, "a-13", "codex-web", "x");
    let ok = ack_p(
        &sock,
        json!({"v": 1, "id": "a-12", "by": "codex-web", "stage": "resultado", "note": "a".repeat(1024)}),
    );
    assert!(!is_err(&ok), "note de 1024 deve ser aceita: {ok}");
    let r = ack_p(
        &sock,
        json!({"v": 1, "id": "a-13", "by": "codex-web", "stage": "resultado", "note": "a".repeat(1025)}),
    );
    assert_eq!(cod(&r), "invalid_envelope");
    assert_eq!(caminho(&r), "/note");
}

// ---------- events.resume ----------

#[test]
fn resume_responde_head_e_reenvia_eventos_depois_de_last_seq() {
    let (_d, srv) = sobe_padrao();
    let sock = srv.socket.clone();
    envia_ok(&sock, "e-1", "codex-web", "1");
    envia_ok(&sock, "e-2", "codex-web", "2");
    envia_ok(&sock, "e-3", "codex-web", "3");
    let (mut c, r) = resume_bruto(&sock, json!(1));
    let v = res(&r);
    assert_eq!(v["last_seq"], 1);
    assert_eq!(v["head_seq"], 3);
    assert!(
        !c.notif_antes_da_resposta,
        "evento chegou antes da resposta de events.resume"
    );
    let evs = c.eventos_ate(Duration::from_secs(3), |e| e["seq"] == 3);
    let regs: Vec<&Value> = evs.iter().filter(|e| e["type"] == "registered").collect();
    let seqs: Vec<i64> = regs.iter().map(|e| e["seq"].as_i64().unwrap()).collect();
    assert_eq!(seqs, vec![2, 3], "replay de seq > 1 em ordem");
    assert_eq!(regs[0]["id"], "e-2");
}

#[test]
fn resume_com_last_seq_zero_reenvia_tudo() {
    let (_d, srv) = sobe_padrao();
    let sock = srv.socket.clone();
    envia_ok(&sock, "z-1", "codex-web", "1");
    envia_ok(&sock, "z-2", "codex-web", "2");
    let (mut c, _) = resume_bruto(&sock, json!(0));
    let evs = c.eventos_ate(Duration::from_secs(3), |e| e["seq"] == 2);
    let seqs: Vec<i64> = evs.iter().map(|e| e["seq"].as_i64().unwrap()).collect();
    assert_eq!(seqs, vec![1, 2]);
}

#[test]
fn resume_entrega_ao_vivo_depois_do_replay() {
    let (_d, srv) = sobe_padrao();
    let sock = srv.socket.clone();
    envia_ok(&sock, "lv-1", "codex-web", "1");
    let (mut c, r) = resume_bruto(&sock, json!(1));
    assert_eq!(res(&r)["head_seq"], 1);
    envia_ok(&sock, "lv-2", "codex-web", "2");
    let evs = c.eventos_ate(Duration::from_secs(5), |e| e["seq"] == 2);
    let ultimo = evs.last().expect("evento ao vivo");
    assert_eq!(ultimo["seq"], 2);
    assert_eq!(ultimo["type"], "registered");
    assert_eq!(ultimo["id"], "lv-2");
}

#[test]
fn resume_eventos_em_ordem_sem_lacunas_incluindo_accepted() {
    let (_d, srv) = sobe_padrao();
    let sock = srv.socket.clone();
    let (mut c, r) = resume_bruto(&sock, json!(0));
    assert_eq!(res(&r)["head_seq"], 0);
    envia_ok(&sock, "o-1", "codex-web", "1");
    envia_ok(&sock, "o-2", "codex-web", "2");
    envia_ok(&sock, "o-3", "codex-web", "3");
    ack_ok(&sock, "o-1", "codex-web", "aceito");
    let evs = c.eventos_ate(Duration::from_secs(5), |e| e["seq"] == 4);
    let seqs: Vec<i64> = evs.iter().map(|e| e["seq"].as_i64().unwrap()).collect();
    assert_eq!(seqs, vec![1, 2, 3, 4]);
    let tipos: Vec<String> = evs
        .iter()
        .map(|e| e["type"].as_str().unwrap().to_string())
        .collect();
    assert_eq!(
        tipos,
        vec!["registered", "registered", "registered", "accepted"]
    );
}

#[test]
fn resume_recebe_resulted_quando_ha_resultado() {
    let (_d, srv) = sobe_padrao();
    let sock = srv.socket.clone();
    envia_ok(&sock, "rs-1", "codex-web", "1");
    let (mut c, _) = resume_bruto(&sock, json!(1));
    ack_p(
        &sock,
        json!({"v": 1, "id": "rs-1", "by": "codex-web", "stage": "resultado", "result": "refused"}),
    );
    let evs = c.eventos_ate(Duration::from_secs(5), |e| e["type"] == "resulted");
    let ev = evs.last().expect("evento resulted");
    assert_eq!(ev["id"], "rs-1");
}

#[test]
fn resume_nao_duplica_evento_de_reenvio_identico() {
    let (_d, srv) = sobe_padrao();
    let sock = srv.socket.clone();
    let (mut c, _) = resume_bruto(&sock, json!(0));
    envia_ok(&sock, "dup-ev", "codex-web", "1");
    envia_ok(&sock, "dup-ev", "codex-web", "1");
    let evs = c.eventos_ate(Duration::from_millis(800), |_| false);
    let n = evs
        .iter()
        .filter(|e| e["id"] == "dup-ev" && e["type"] == "registered")
        .count();
    assert_eq!(n, 1, "um evento registered por id");
}

#[test]
fn resume_segunda_chamada_na_mesma_conexao_e_invalid_request() {
    let (_d, srv) = sobe_padrao();
    let (mut c, r) = resume_bruto(&srv.socket, json!(0));
    assert!(!is_err(&r));
    let r2 = c.chama("events.resume", json!({"v": 1, "last_seq": 0}));
    assert_eq!(cod(&r2), "invalid_request");
}

#[test]
fn resume_last_seq_acima_do_head_e_invalid_envelope_com_path() {
    let (_d, srv) = sobe_padrao();
    let (_c, r) = resume_bruto(&srv.socket, json!(999));
    assert_eq!(cod(&r), "invalid_envelope");
    assert_eq!(caminho(&r), "/last_seq");
}

#[test]
fn conexao_sem_resume_nao_recebe_eventos() {
    let (_d, srv) = sobe_padrao();
    let sock = srv.socket.clone();
    let mut a = Conexao::abre(&sock);
    res(&a.chama("status", json!({"v": 1})));
    envia_ok(&sock, "nr-1", "codex-web", "1");
    assert!(
        a.evento(Duration::from_secs(1)).is_none(),
        "conexao sem resume recebeu evento"
    );
    res(&a.chama("status", json!({"v": 1})));
}

// ---------- status ----------

#[test]
fn status_tem_head_e_campos_por_destinatario() {
    let (_d, srv) = sobe_padrao();
    let sock = srv.socket.clone();
    envia_ok(&sock, "st-1", "codex-web", "x");
    let r = res(&rpc(&sock, "status", json!({"v": 1})));
    assert_eq!(r["v"], 1);
    assert_eq!(r["head_seq"], 1);
    for nome in ["codex-web", "claude-main"] {
        let d = &r["recipients"][nome];
        for campo in ["pending", "accepted", "bell", "last_bell_seq"] {
            assert!(d.get(campo).is_some(), "{nome} sem {campo}: {d}");
        }
    }
    assert_eq!(r["recipients"]["codex-web"]["pending"], 1);
}

// ---------- erros ----------

#[test]
fn linha_que_nao_e_json_e_parse_error_e_servidor_segue() {
    let (_d, srv) = sobe_padrao();
    let mut c = Conexao::abre(&srv.socket);
    c.envia_bruto("{nao e json");
    let r = c.le(Duration::from_secs(5)).expect("resposta de erro");
    assert_eq!(cod(&r), "parse_error");
    assert_eq!(
        status(&srv.socket)["head_seq"],
        0,
        "servidor deve seguir vivo"
    );
}

#[test]
fn lote_de_requisicoes_e_invalid_request() {
    let (_d, srv) = sobe_padrao();
    let mut c = Conexao::abre(&srv.socket);
    c.envia_bruto(r#"[{"jsonrpc":"2.0","id":1,"method":"status","params":{"v":1}}]"#);
    let r = c.le(Duration::from_secs(5)).expect("resposta");
    assert_eq!(cod(&r), "invalid_request");
    assert_eq!(status(&srv.socket)["head_seq"], 0);
}

#[test]
fn metodo_desconhecido_e_method_not_found() {
    let (_d, srv) = sobe_padrao();
    let r = rpc(&srv.socket, "message.nao_existe", json!({"v": 1}));
    assert_eq!(cod(&r), "method_not_found");
}

#[test]
fn versao_2_e_unsupported_version_em_status_list_e_ack() {
    let (_d, srv) = sobe_padrao();
    let sock = srv.socket.clone();
    assert_eq!(
        cod(&rpc(&sock, "status", json!({"v": 2}))),
        "unsupported_version"
    );
    assert_eq!(
        cod(&rpc(
            &sock,
            "message.list",
            json!({"v": 2, "to": "codex-web"})
        )),
        "unsupported_version"
    );
    assert_eq!(
        cod(&ack_p(
            &sock,
            json!({"v": 2, "id": "x", "by": "codex-web", "stage": "aceito"})
        )),
        "unsupported_version"
    );
}

#[test]
fn linha_acima_de_1_mib_e_invalid_request() {
    let (_d, srv) = sobe_padrao();
    let lixo = "x".repeat(1024 * 1024);
    let linha = format!(
        r#"{{"jsonrpc":"2.0","id":1,"method":"status","params":{{"v":1,"lixo":"{lixo}"}}}}"#
    );
    let st = UnixStream::connect(&srv.socket).expect("conectar");
    st.set_read_timeout(Some(Duration::from_secs(10))).unwrap();
    let mut escrita = st.try_clone().unwrap();
    let bytes = linha.into_bytes();
    let h = thread::spawn(move || {
        let _ = escrita.write_all(&bytes);
        let _ = escrita.write_all(b"\n");
    });
    let mut leitor = BufReader::new(st);
    let mut resp = String::new();
    let _ = leitor.read_line(&mut resp);
    let _ = h.join();
    let v: Value = serde_json::from_str(resp.trim()).expect("resposta para linha grande");
    assert_eq!(cod(&v), "invalid_request");
    assert_eq!(status(&srv.socket)["v"], 1, "servidor deve seguir vivo");
}

#[test]
fn erros_de_uma_conexao_nao_derrubam_outras() {
    let (_d, srv) = sobe_padrao();
    let sock = srv.socket.clone();
    let mut a = Conexao::abre(&sock);
    res(&a.chama("status", json!({"v": 1})));
    let mut b = Conexao::abre(&sock);
    b.envia_bruto("{x");
    assert_eq!(cod(&b.le(Duration::from_secs(5)).unwrap()), "parse_error");
    assert_eq!(
        cod(&b.chama("nao.existe", json!({"v": 1}))),
        "method_not_found"
    );
    res(&a.chama("status", json!({"v": 1})));
    assert_eq!(status(&sock)["v"], 1);
}

#[test]
fn teto_de_64_conexoes_recusa_a_65a() {
    let (_d, srv) = sobe_padrao();
    let sock = srv.socket.clone();
    let mut conns: Vec<Conexao> = (0..64).map(|_| Conexao::abre(&sock)).collect();
    for c in conns.iter_mut() {
        res(&c.chama("status", json!({"v": 1})));
    }
    let st = UnixStream::connect(&sock).expect("65a conexao");
    st.set_read_timeout(Some(Duration::from_secs(3))).unwrap();
    let mut escrita = st.try_clone().unwrap();
    let mut leitor = BufReader::new(st);
    let mut linha = String::new();
    let lidos = leitor.read_line(&mut linha).unwrap_or(0);
    if lidos == 0 {
        let _ = escrita.write_all(
            b"{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"status\",\"params\":{\"v\":1}}\n",
        );
        linha.clear();
        let _ = leitor.read_line(&mut linha);
    }
    let v: Value = serde_json::from_str(linha.trim()).expect("resposta da 65a conexao");
    assert_eq!(cod(&v), "too_many_connections");
    // as 64 primeiras seguem funcionando
    res(&conns[0].chama("status", json!({"v": 1})));
    res(&conns[63].chama("status", json!({"v": 1})));
    // ao liberar uma vaga, uma nova conexao e aceita
    conns.remove(0);
    let aceita = espera(Duration::from_secs(3), || {
        let mut c = Conexao::abre(&sock);
        c.chama("status", json!({"v": 1})).get("result").is_some()
    });
    assert!(
        aceita,
        "nova conexao nao foi aceita depois de liberar uma vaga"
    );
}

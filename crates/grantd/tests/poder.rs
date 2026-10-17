//! Poder (G15 a G21): quem cria, nega, revoga e aprova; validade obrigatoria; pedidos; visibilidade.
//! Oraculos: codigos de erro exatos, `head_seq` (status) e o conteudo da ultima linha do log.
mod common;

use common::*;
use serde_json::{Value, json};

fn itens(v: &Value, chave: &str) -> Vec<Value> {
    resultado(v)[chave]
        .as_array()
        .unwrap_or_else(|| panic!("list sem a lista `{chave}`: {v}"))
        .clone()
}

/// Corpo de `request` do agente: acao, recurso e validade (sem `reason`).
fn pedido(acao: &str, recurso: &str, ate: i64) -> Value {
    json!({"action": acao, "resource": recurso, "until": em(ate), "request_id": novo_rid()})
}

// G15: agente nao cria, nega, revoga nem aprova: tudo `forbidden`, e o log nao anda.
// Controles: o mesmo dono faz as quatro operacoes com sucesso no mesmo cenario.
#[test]
fn g15_agente_nao_cria_nega_revoga_nem_aprova() {
    let d = Disco::novo();
    let r = Relogio::novo();
    let mut s = sobe(&d, &r, &grants_padrao());
    let id_g = concede(&mut s, A2, "fs.read", "/p/x", 3600);
    let req_id = id_de(&pede_como(
        &mut s,
        A2,
        "request",
        pedido("fs.write", "/w/x", 600),
    ));
    let h0 = head(&mut s);

    let grant = pede_como(
        &mut s,
        A1,
        "grant",
        com(
            allow(A1, "fs.read", "/z/x"),
            json!({"until": em(600), "request_id": novo_rid()}),
        ),
    );
    assert_eq!(codigo(&grant), "forbidden", "{grant}");
    let deny = pede_como(
        &mut s,
        A1,
        "deny_rule",
        com(
            allow(A1, "fs.read", "/z/x"),
            json!({"effect": "deny", "until": em(600), "request_id": novo_rid()}),
        ),
    );
    assert_eq!(codigo(&deny), "forbidden", "{deny}");
    let revoke = pede_como(
        &mut s,
        A1,
        "revoke",
        json!({"id": id_g, "reason": "tentativa", "request_id": novo_rid()}),
    );
    assert_eq!(codigo(&revoke), "forbidden", "{revoke}");
    let approve = pede_como(
        &mut s,
        A1,
        "approve_request",
        json!({"req": req_id, "until": em(600), "request_id": novo_rid()}),
    );
    assert_eq!(codigo(&approve), "forbidden", "{approve}");

    assert_eq!(head(&mut s), h0, "tentativas recusadas nao gravam");
    assert_decisao(&decide(&mut s, A1, "fs.read", "/z/x"), "deny", "no_grant");
    assert_decisao(&decide(&mut s, A2, "fs.read", "/p/x"), "allow", "ok");

    // Controles: o dono faz as quatro operacoes.
    let ok_grant = pede_dono(
        &mut s,
        "grant",
        com(
            allow(A1, "fs.read", "/z/x"),
            json!({"until": em(600), "request_id": novo_rid()}),
        ),
    );
    resultado(&ok_grant);
    resultado(&pede_dono(
        &mut s,
        "approve_request",
        json!({"req": req_id, "until": em(600), "request_id": novo_rid()}),
    ));
    resultado(&pede_dono(
        &mut s,
        "deny_rule",
        com(
            allow(A1, "fs.read", "/z/y"),
            json!({"effect": "deny", "until": em(600), "request_id": novo_rid()}),
        ),
    ));
    resultado(&pede_dono(
        &mut s,
        "revoke",
        json!({"id": id_g, "reason": "fim do teste", "request_id": novo_rid()}),
    ));
    assert_decisao(&decide(&mut s, A1, "fs.read", "/z/x"), "allow", "ok");
}

// G16: `until` obrigatorio salvo `permanent`. Controles: com `until`, o mesmo pedido passa;
// `permanent:true` sem `until` passa.
#[test]
fn g16_sem_until_e_sem_permanent_e_missing_until() {
    let d = Disco::novo();
    let r = Relogio::novo();
    let mut s = sobe(&d, &r, &grants_padrao());

    let sem = pede_dono(
        &mut s,
        "grant",
        com(
            allow(A1, "fs.read", "/p/x"),
            json!({"request_id": novo_rid()}),
        ),
    );
    assert_eq!(codigo(&sem), "missing_until", "{sem}");

    let sem_negacao = pede_dono(
        &mut s,
        "deny_rule",
        com(
            allow(A1, "fs.read", "/p/x"),
            json!({"effect": "deny", "request_id": novo_rid()}),
        ),
    );
    assert_eq!(codigo(&sem_negacao), "missing_until", "{sem_negacao}");

    assert_decisao(&decide(&mut s, A1, "fs.read", "/p/x"), "deny", "no_grant");

    let com_ate = pede_dono(
        &mut s,
        "grant",
        com(
            allow(A1, "fs.read", "/p/x"),
            json!({"until": em(3600), "request_id": novo_rid()}),
        ),
    );
    resultado(&com_ate);
    assert_decisao(&decide(&mut s, A1, "fs.read", "/p/x"), "allow", "ok");

    let permanente = pede_dono(
        &mut s,
        "grant",
        com(
            allow(A1, "fs.read", "/perm/x"),
            json!({"permanent": true, "request_id": novo_rid()}),
        ),
    );
    resultado(&permanente);
    assert_decisao(&decide(&mut s, A1, "fs.read", "/perm/x"), "allow", "ok");
}

// G17: `permanent` so o dono cria (G15) e `list` do dono o destaca: campo `permanent` true na
// concessao permanente e false na que tem `until`. Campo `status` derivado `active`.
#[test]
fn g17_permanent_aparece_destacado_no_list_do_dono() {
    let d = Disco::novo();
    let r = Relogio::novo();
    let mut s = sobe(&d, &r, &grants_padrao());
    let id_perm = id_de(&pede_dono(
        &mut s,
        "grant",
        com(
            allow(A1, "fs.read", "/perm/x"),
            json!({"permanent": true, "request_id": novo_rid()}),
        ),
    ));
    let id_ate = concede(&mut s, A1, "fs.read", "/p/x", 3600);

    let lista = pede_dono(&mut s, "list", json!({}));
    let grants = itens(&lista, "grants");
    let perm = grants
        .iter()
        .find(|g| g["id"] == json!(id_perm))
        .unwrap_or_else(|| panic!("list do dono sem a permanente: {lista}"));
    assert_eq!(perm["permanent"], json!(true), "{perm}");
    assert_eq!(perm["status"], json!("active"), "{perm}");
    assert!(
        perm["until"].is_null(),
        "permanente: until null (5A.5): {perm}"
    );
    let ate = grants
        .iter()
        .find(|g| g["id"] == json!(id_ate))
        .unwrap_or_else(|| panic!("list do dono sem a concessao com until: {lista}"));
    assert_eq!(ate["permanent"], json!(false), "{ate}");
    assert_decisao(&decide(&mut s, A1, "fs.read", "/perm/x"), "allow", "ok");
}

// G18: `request` cria pedido `pending` e nao concede: `check` continua deny no_grant.
// Controle: o dono aprova e o mesmo `check` vira allow.
#[test]
fn g18_request_cria_pendente_e_nao_concede() {
    let d = Disco::novo();
    let r = Relogio::novo();
    let mut s = sobe(&d, &r, &grants_padrao());

    let v = pede_como(&mut s, A1, "request", pedido("fs.write", "/w/x", 600));
    assert_eq!(resultado(&v)["status"], json!("pending"), "{v}");
    let req_id = id_de(&v);

    assert_decisao(&decide(&mut s, A1, "fs.write", "/w/x"), "deny", "no_grant");

    let lista = pede_dono(&mut s, "list", json!({}));
    let pendentes = itens(&lista, "requests");
    let meu = pendentes
        .iter()
        .find(|p| p["id"] == json!(req_id))
        .unwrap_or_else(|| panic!("list do dono sem o pedido: {lista}"));
    assert_eq!(meu["status"], json!("pending"), "{meu}");

    resultado(&pede_dono(
        &mut s,
        "approve_request",
        json!({"req": req_id, "until": em(600), "request_id": novo_rid()}),
    ));
    assert_decisao(&decide(&mut s, A1, "fs.write", "/w/x"), "allow", "ok");
}

// G19: `approve_request` gera a concessao no mesmo commit (um commit: approve + grant, head_seq +1) e o
// `until` aprovado nunca excede o pedido. Pedido com until maior = invalid_params (6A). Controle: o aprovado ate antes do pedido passa e o `check` mostra o until aprovado.
#[test]
fn g19_approve_gera_concessao_no_mesmo_commit_e_nao_excede_o_pedido() {
    let d = Disco::novo();
    let r = Relogio::novo();
    let mut s = sobe(&d, &r, &grants_padrao());
    let req_id = id_de(&pede_como(
        &mut s,
        A1,
        "request",
        pedido("fs.read", "/p/x", 600),
    ));

    let h0 = head(&mut s);
    let estica = pede_dono(
        &mut s,
        "approve_request",
        json!({"req": req_id, "until": em(900), "request_id": novo_rid()}),
    );
    assert_eq!(codigo(&estica), "invalid_params", "{estica}");
    assert_eq!(head(&mut s), h0, "approve recusado nao grava");

    let aprova = pede_dono(
        &mut s,
        "approve_request",
        json!({"req": req_id, "until": em(300), "request_id": novo_rid()}),
    );
    resultado(&aprova);
    assert_eq!(
        head(&mut s),
        h0 + 1,
        "approve e a concessao sao um commit so"
    );

    let tx = ultimo_tx(&d);
    let tipos: Vec<Value> = tx.iter().map(|e| e["e"].clone()).collect();
    assert_eq!(tipos, vec![json!("approve"), json!("grant")], "{tx:?}");
    assert_eq!(tx[1]["issuer"], json!("owner"), "{tx:?}");

    let v = decide(&mut s, A1, "fs.read", "/p/x");
    assert_decisao(&v, "allow", "ok");
    let until = resultado(&v)["until"].as_str().expect("until");
    assert_eq!(
        chrono::DateTime::parse_from_rfc3339(until)
            .unwrap()
            .with_timezone(&chrono::Utc),
        no(300),
        "until aprovado: {v}"
    );
}

// G20: no maximo 5 pedidos `pending` por agente (config padrao). O 6o = quota. Controles: outro agente
// (a2) ainda pede; depois de o dono rejeitar um, o agente pede de novo.
#[test]
fn g20_mais_de_cinco_pendentes_e_quota() {
    let d = Disco::novo();
    let r = Relogio::novo();
    let mut s = sobe(&d, &r, &grants_padrao());
    let mut ids = Vec::new();
    for i in 0..5 {
        let v = pede_como(
            &mut s,
            A1,
            "request",
            pedido("fs.write", &format!("/w/{i}"), 600),
        );
        ids.push(id_de(&v));
    }
    let sexto = pede_como(&mut s, A1, "request", pedido("fs.write", "/w/6", 600));
    assert_eq!(codigo(&sexto), "quota", "{sexto}");

    let do_outro = pede_como(&mut s, A2, "request", pedido("fs.write", "/w/outro", 600));
    resultado(&do_outro);

    resultado(&pede_dono(
        &mut s,
        "reject_request",
        json!({"req": ids[0], "reason": "fora do plano", "request_id": novo_rid()}),
    ));
    let de_novo = pede_como(&mut s, A1, "request", pedido("fs.write", "/w/7", 600));
    resultado(&de_novo);
}

// G21: agente nao ve concessao alheia. `show` de id alheio responde IDENTICO ao de id inexistente
// (bruto, byte a byte, exceto o id da requisicao; os ids G e a eventual mencao ao id pedido sao trocados por
// um marcador, ver GRANTD-TESTES.md). `list` do agente nao muda quando o dono cria concessao para outro.
// Controle: o dono ve a concessao alheia e o dono do agente `a1` ve a propria.
#[test]
fn g21_show_e_list_alheios_iguais_a_inexistente() {
    let d = Disco::novo();
    let r = Relogio::novo();
    let mut s = sobe(&d, &r, &grants_padrao());

    let list_antes = bruto(&mut s, &token_de(A2), "fixo-list", "list", json!({}));
    let g = concede(&mut s, A1, "fs.read", "/p/x", 3600);
    let inexistente = "G-999999";

    let alheio = bruto(
        &mut s,
        &token_de(A2),
        "fixo-show-alheio",
        "show",
        json!({"id": g}),
    );
    let nao_existe = bruto(
        &mut s,
        &token_de(A2),
        "fixo-show-nao",
        "show",
        json!({"id": inexistente}),
    );
    let va: Value = serde_json::from_str(&alheio).unwrap();
    assert_eq!(codigo(&va), "not_found", "{alheio}");
    let normaliza = |t: &str, id_req: &str, id_g: &str| {
        t.replacen(&format!("\"id\":\"{id_req}\""), "\"id\":\"<REQ>\"", 1)
            .replace(id_g, "<G>")
    };
    assert_eq!(
        normaliza(&alheio, "fixo-show-alheio", &g),
        normaliza(&nao_existe, "fixo-show-nao", inexistente),
        "show alheio != inexistente"
    );

    let list_depois = bruto(&mut s, &token_de(A2), "fixo-list", "list", json!({}));
    // 6A: o list traz `head_seq` (estado global, nao existencia da concessao alheia): fora dele, nada muda.
    let sem_head = |t: &str| {
        let mut v: Value = serde_json::from_str(t).expect("list JSON");
        v["result"]
            .as_object_mut()
            .expect("result")
            .remove("head_seq");
        v
    };
    assert_eq!(
        sem_head(&list_antes),
        sem_head(&list_depois),
        "list do agente mudou com concessao alheia (sem head_seq)"
    );
    assert!(
        !list_depois.contains(&g),
        "list de a2 expos a concessao de a1: {list_depois}"
    );

    // Controles: o dono ve a concessao; a1 ve a propria.
    let dono_ve = pede_dono(&mut s, "show", json!({"id": g}));
    assert_eq!(resultado(&dono_ve)["id"], json!(g), "{dono_ve}");
    let a1_ve = pede_como(&mut s, A1, "show", json!({"id": g}));
    assert_eq!(resultado(&a1_ve)["id"], json!(g), "{a1_ve}");
    let a1_lista = itens(&pede_como(&mut s, A1, "list", json!({})), "grants");
    assert!(
        a1_lista.iter().any(|x| x["id"] == json!(g)),
        "a1 deveria listar a propria: {a1_lista:?}"
    );
}

// G37: `reason` e `source.ref` com padrao de segredo (AKIA + 16 de [0-9A-Z]) = secret_detected, sem commit
// (6A). Controle: o mesmo `revoke` com motivo limpo passa, e o agente ainda tinha a concessao antes.
#[test]
fn g37_reason_e_source_ref_com_segredo_sao_secret_detected() {
    let d = Disco::novo();
    let r = Relogio::novo();
    let mut s = sobe(&d, &r, &grants_padrao());
    let id_g = concede(&mut s, A1, "fs.read", "/p/x", 3600);
    let segredo = "AKIA0123456789ABCDEF";
    let h0 = head(&mut s);

    let com_motivo = pede_dono(
        &mut s,
        "revoke",
        json!({"id": id_g, "reason": format!("motivo {segredo}"), "request_id": novo_rid()}),
    );
    assert_eq!(codigo(&com_motivo), "secret_detected", "{com_motivo}");

    let com_fonte = pede_dono(
        &mut s,
        "grant",
        com(
            allow(A2, "fs.read", "/q/y"),
            json!({
                "until": em(3600),
                "source": {"kind": "msg", "ref": format!("chave {segredo}"), "at": "2026-10-16"},
                "request_id": novo_rid(),
            }),
        ),
    );
    assert_eq!(codigo(&com_fonte), "secret_detected", "{com_fonte}");
    assert_eq!(head(&mut s), h0, "recusados por segredo nao gravam");
    assert_decisao(&decide(&mut s, A1, "fs.read", "/p/x"), "allow", "ok");

    // Controle: texto limpo passa.
    resultado(&pede_dono(
        &mut s,
        "revoke",
        json!({"id": id_g, "reason": "motivo limpo", "request_id": novo_rid()}),
    ));
    assert_decisao(&decide(&mut s, A1, "fs.read", "/p/x"), "deny", "no_grant");
}

// G43: empates e regras fixas (5A.5). Entre allow ativas: a de `until` menor (permanent = infinito) e,
// no empate, a de menor id. `denied_by` = menor id entre as deny. `permanent` + `until` = invalid_params.
// Controle: uma permanente sozinha responde allow com `until` null.
#[test]
fn g43_empates_e_permanent_por_regra_fixa() {
    let d = Disco::novo();
    let r = Relogio::novo();
    let mut s = sobe(&d, &r, &grants_padrao());

    let perm = id_de(&pede_dono(
        &mut s,
        "grant",
        com(
            allow(A1, "fs.read", "/q/x"),
            json!({"permanent": true, "request_id": novo_rid()}),
        ),
    ));
    let fin = concede(&mut s, A1, "fs.read", "/q/x", 3600);
    let v = decide(&mut s, A1, "fs.read", "/q/x");
    assert_decisao(&v, "allow", "ok");
    assert_eq!(grant_de(&v), json!(fin), "a finita vence a permanente: {v}");
    assert_ne!(json!(perm), grant_de(&v));

    let so_perm = id_de(&pede_dono(
        &mut s,
        "grant",
        com(
            allow(A1, "fs.read", "/r/x"),
            json!({"permanent": true, "request_id": novo_rid()}),
        ),
    ));
    let v2 = decide(&mut s, A1, "fs.read", "/r/x");
    assert_decisao(&v2, "allow", "ok");
    assert_eq!(grant_de(&v2), json!(so_perm), "{v2}");
    assert!(
        resultado(&v2)["until"].is_null(),
        "permanente: until null: {v2}"
    );

    let a = concede(&mut s, A2, "fs.read", "/p/x", 3600);
    let b = concede(&mut s, A2, "fs.read", "/p/x", 3600);
    let menor = std::cmp::min(a.clone(), b.clone());
    assert_eq!(
        grant_de(&decide(&mut s, A2, "fs.read", "/p/x")),
        json!(menor)
    );

    concede(&mut s, A3, "fs.read", "/s/**", 3600);
    let d1 = nega(&mut s, A3, "fs.read", "/s/x", 3600);
    let d2 = nega(&mut s, A3, "fs.read", "/s/x", 3600);
    let menor_deny = std::cmp::min(d1.clone(), d2.clone());
    let negado = decide(&mut s, A3, "fs.read", "/s/x");
    assert_decisao(&negado, "deny", "denied_by");
    assert_eq!(
        resultado(&negado)["denied_by"],
        json!(menor_deny),
        "{negado}"
    );

    let mistura = pede_dono(
        &mut s,
        "grant",
        com(
            allow(A1, "fs.read", "/t/x"),
            json!({"permanent": true, "until": em(3600), "request_id": novo_rid()}),
        ),
    );
    assert_eq!(codigo(&mistura), "invalid_params", "{mistura}");
}

// G52: `list` paginado (5A.10 e 5A.15). Ordem: concessoes por numero de id, depois pedidos por numero de id.
// `after` e o id do ultimo item entregue ("G-..." continua nas concessoes e depois nos pedidos; "R-..."
// continua nos pedidos). `next_after` null no fim. limit 1..50 (padrao 20). Controle: limit 50 entrega tudo.
#[test]
fn g52_list_paginado_com_cursor_string() {
    let d = Disco::novo();
    let r = Relogio::novo();
    let mut s = sobe(&d, &r, &grants_padrao());
    let mut gs = Vec::new();
    for c in ["/g/1", "/g/2", "/g/3", "/g/4", "/g/5"] {
        gs.push(concede(&mut s, A1, "fs.read", c, 3600));
    }
    let mut rs = Vec::new();
    for i in 1..=3 {
        let v = pede_como(
            &mut s,
            A2,
            "request",
            pedido("fs.write", &format!("/w/{i}"), 600),
        );
        rs.push(id_de(&v));
    }

    let ids = |v: &serde_json::Value, chave: &str| -> Vec<String> {
        itens(v, chave)
            .iter()
            .map(|x| x["id"].as_str().expect("id").to_string())
            .collect()
    };
    let p1 = pede_dono(&mut s, "list", json!({"limit": 3}));
    assert_eq!(ids(&p1, "grants"), gs[0..3].to_vec(), "{p1}");
    assert!(ids(&p1, "requests").is_empty(), "{p1}");
    let after1 = resultado(&p1)["next_after"]
        .as_str()
        .expect("next_after")
        .to_string();
    assert_eq!(after1, gs[2], "cursor = ultimo item entregue: {p1}");

    let p2 = pede_dono(&mut s, "list", json!({"limit": 3, "after": after1}));
    assert_eq!(ids(&p2, "grants"), gs[3..5].to_vec(), "{p2}");
    assert_eq!(
        ids(&p2, "requests"),
        vec![rs[0].clone()],
        "pedidos depois das concessoes: {p2}"
    );
    let after2 = resultado(&p2)["next_after"]
        .as_str()
        .expect("next_after")
        .to_string();
    assert_eq!(after2, rs[0], "{p2}");

    let p3 = pede_dono(&mut s, "list", json!({"limit": 3, "after": after2}));
    assert!(ids(&p3, "grants").is_empty(), "{p3}");
    assert_eq!(ids(&p3, "requests"), rs[1..3].to_vec(), "{p3}");
    assert!(
        resultado(&p3)["next_after"].is_null(),
        "fim: next_after null: {p3}"
    );

    let tudo = pede_dono(&mut s, "list", json!({"limit": 50}));
    assert_eq!(ids(&tudo, "grants"), gs, "{tudo}");
    assert_eq!(ids(&tudo, "requests"), rs, "{tudo}");
    assert!(resultado(&tudo)["next_after"].is_null(), "{tudo}");

    for ruim in [0, 51] {
        let v = pede_dono(&mut s, "list", json!({"limit": ruim}));
        assert_eq!(codigo(&v), "invalid_params", "limit {ruim}: {v}");
    }
}

// G53: pedido vencido (until passado) nao conta na quota (5A.10). `approve`/`reject` de pedido que nao esta
// `pending` = conflict. `revoke` de concessao ja revogada = conflict. Controle: com 5 pendentes validos,
// o 6o e quota.
#[test]
fn g53_pedido_vencido_nao_conta_e_conflitos_de_estado() {
    let d = Disco::novo();
    let r = Relogio::novo();
    let mut s = sobe(&d, &r, &grants_padrao());
    for i in 0..5 {
        let v = pede_como(
            &mut s,
            A1,
            "request",
            json!({"action": "fs.write", "resource": format!("/w/{i}"), "until": em(100), "request_id": novo_rid()}),
        );
        resultado(&v);
    }
    let sexto = pede_como(&mut s, A1, "request", pedido("fs.write", "/w/6", 100));
    assert_eq!(codigo(&sexto), "quota", "5 pendentes validos: {sexto}");

    r.vai(200);
    let depois = pede_como(&mut s, A1, "request", pedido("fs.write", "/w/7", 1000));
    resultado(&depois);
    let rq = id_de(&depois);

    resultado(&pede_dono(
        &mut s,
        "approve_request",
        json!({"req": rq, "until": em(500), "request_id": novo_rid()}),
    ));
    let de_novo = pede_dono(
        &mut s,
        "approve_request",
        json!({"req": rq, "until": em(500), "request_id": novo_rid()}),
    );
    assert_eq!(
        codigo(&de_novo),
        "conflict",
        "approve de nao pending: {de_novo}"
    );
    let rejeita = pede_dono(
        &mut s,
        "reject_request",
        json!({"req": rq, "reason": "tarde", "request_id": novo_rid()}),
    );
    assert_eq!(
        codigo(&rejeita),
        "conflict",
        "reject de nao pending: {rejeita}"
    );

    let g = concede(&mut s, A2, "fs.read", "/p/x", 3600);
    resultado(&pede_dono(
        &mut s,
        "revoke",
        json!({"id": g, "reason": "uma vez", "request_id": novo_rid()}),
    ));
    let de_novo_revoke = pede_dono(
        &mut s,
        "revoke",
        json!({"id": g, "reason": "outra vez", "request_id": novo_rid()}),
    );
    assert_eq!(
        codigo(&de_novo_revoke),
        "conflict",
        "revoke de revogada: {de_novo_revoke}"
    );
}

// G55: `reason`, `source.ref`, `task_ref` e o `reason` do pedido com padrao de segredo = secret_detected
// (5A.11). Controle: os mesmos campos com texto limpo passam.
#[test]
fn g55_task_ref_e_reason_de_pedido_com_segredo_sao_secret_detected() {
    let d = Disco::novo();
    let r = Relogio::novo();
    let mut s = sobe(&d, &r, &grants_padrao());
    let segredo = "AKIA0123456789ABCDEF";
    let h0 = head(&mut s);

    let tarefa = pede_dono(
        &mut s,
        "grant",
        com(
            allow(A1, "fs.read", "/p/x"),
            json!({"until": em(3600), "task_ref": format!("PEND-{segredo}"), "request_id": novo_rid()}),
        ),
    );
    assert_eq!(codigo(&tarefa), "secret_detected", "{tarefa}");

    let motivo = pede_como(
        &mut s,
        A2,
        "request",
        json!({"action": "fs.write", "resource": "/w/x", "until": em(600), "reason": format!("por {segredo}"), "request_id": novo_rid()}),
    );
    assert_eq!(codigo(&motivo), "secret_detected", "{motivo}");
    assert_eq!(head(&mut s), h0, "recusas nao gravam");

    // Controle: os mesmos campos com texto limpo passam.
    let ok_tarefa = pede_dono(
        &mut s,
        "grant",
        com(
            allow(A1, "fs.read", "/p/x"),
            json!({"until": em(3600), "task_ref": "PEND-12", "request_id": novo_rid()}),
        ),
    );
    resultado(&ok_tarefa);
    let ok_motivo = pede_como(
        &mut s,
        A2,
        "request",
        json!({"action": "fs.write", "resource": "/w/x", "until": em(600), "reason": "pedido limpo", "request_id": novo_rid()}),
    );
    resultado(&ok_motivo);
}

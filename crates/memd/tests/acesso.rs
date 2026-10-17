//! Q1..Q7: leitura e acesso (memd-spec-v0.md, secoes 6 e 10): query, term, limit, view e resposta.

mod common;

use common::*;
use serde_json::{Value, json};

fn perfil_a() -> String {
    grants(&[perfil_total("agente-a", TOK_A)])
}

/// Titulo de tamanho exato (120 caracteres), sem 'D-' dentro (nao confunde a leitura da view).
fn titulo_longo(i: usize) -> String {
    let base = format!("Titulo longo numero {i:02} ");
    let falta = 120 - base.chars().count();
    format!("{base}{}", "w".repeat(falta))
}

// ------------------------------------------------------------------ Q1

#[test]
fn q01_query_ordena_por_tipo_e_id_e_pagina_por_after_sem_pular() {
    let m = Mundo::novo();
    let mut s = servico(&m, &perfil_a());
    propoe_e_promove(&mut s, TOK_A, decisao("Um", "agentd"));
    propoe_e_promove(&mut s, TOK_A, decisao("Dois", "agentd"));
    propoe_e_promove(
        &mut s,
        TOK_A,
        pendencia("Pendencia um", "agentd", "2026-11-01"),
    );
    propoe_e_promove(
        &mut s,
        TOK_A,
        pendencia("Pendencia dois", "agentd", "2026-11-01"),
    );
    propoe_e_promove(&mut s, TOK_A, decisao("Tres", "agentd"));

    // decisao antes de pendencia, depois por numero do id
    let tudo = consulta(&mut s, TOK_A, json!({}));
    assert_eq!(
        ids_da_consulta(&tudo),
        vec!["D-001", "D-002", "D-003", "P-001", "P-002"]
    );

    // paginas de 2: next_after = ultimo id entregue; a ultima pagina traz null
    let p1 = consulta(&mut s, TOK_A, json!({"limit": 2}));
    assert_eq!(ids_da_consulta(&p1), vec!["D-001", "D-002"]);
    assert_eq!(p1["next_after"], json!("D-002"));
    let p2 = consulta(&mut s, TOK_A, json!({"limit": 2, "after": "D-002"}));
    assert_eq!(ids_da_consulta(&p2), vec!["D-003", "P-001"]);
    assert_eq!(p2["next_after"], json!("P-001"));
    let p3 = consulta(&mut s, TOK_A, json!({"limit": 2, "after": "P-001"}));
    assert_eq!(ids_da_consulta(&p3), vec!["P-002"]);
    assert!(p3["next_after"].is_null(), "ultima pagina: next_after null");
}

#[test]
fn q01b_paginacao_nao_pula_item_que_existia_durante_toda_a_leitura() {
    let m = Mundo::novo();
    let mut s = servico(&m, &perfil_a());
    propoe_e_promove(&mut s, TOK_A, decisao("Um", "agentd"));
    propoe_e_promove(&mut s, TOK_A, decisao("Dois", "agentd"));
    propoe_e_promove(&mut s, TOK_A, decisao("Tres", "agentd"));
    propoe_e_promove(
        &mut s,
        TOK_A,
        pendencia("Pendencia um", "agentd", "2026-11-01"),
    );
    propoe_e_promove(
        &mut s,
        TOK_A,
        pendencia("Pendencia dois", "agentd", "2026-11-01"),
    );

    let p1 = consulta(&mut s, TOK_A, json!({"limit": 2}));
    assert_eq!(ids_da_consulta(&p1), vec!["D-001", "D-002"]);
    let cursor = p1["next_after"].as_str().unwrap().to_string();

    // mutacoes concorrentes entre as paginas: um item novo entra depois do cursor
    propoe_e_promove(&mut s, TOK_A, decisao("Quatro novo", "agentd"));

    let p2 = consulta(&mut s, TOK_A, json!({"limit": 50, "after": cursor}));
    let ids = ids_da_consulta(&p2);
    for obrigatorio in ["D-003", "P-001", "P-002"] {
        assert!(
            ids.iter().any(|i| i == obrigatorio),
            "{obrigatorio} existia durante toda a leitura: {ids:?}"
        );
    }
    let mut unicos = ids.clone();
    unicos.sort();
    unicos.dedup();
    assert_eq!(unicos.len(), ids.len(), "sem duplicados: {ids:?}");
}

// ------------------------------------------------------------------ Q2

#[test]
fn q02_term_dobra_so_a_z_em_titulo_e_tags() {
    let m = Mundo::novo();
    let mut s = servico(&m, &perfil_a());
    let mut d1 = decisao("Bloco de Notas", "agentd");
    d1["tags"] = json!(["sync-x"]);
    propoe_e_promove(&mut s, TOK_A, d1); // D-001
    let mut d2 = decisao("Decisão Final", "agentd");
    d2["tags"] = json!(["outro"]);
    propoe_e_promove(&mut s, TOK_A, d2); // D-002

    let busca = |s: &mut memd::Service, termo: &str| {
        ids_da_consulta(&consulta(s, TOK_A, json!({"term": termo})))
    };
    assert_eq!(busca(&mut s, "bloco de notas"), vec!["D-001"]);
    assert_eq!(
        busca(&mut s, "BLOCO"),
        vec!["D-001"],
        "A-Z do termo casa com titulo em minuscula ou maiuscula"
    );
    assert_eq!(
        busca(&mut s, "DECIS"),
        vec!["D-002"],
        "controle: termo ASCII dobrado"
    );
    assert_eq!(
        busca(&mut s, "decisão"),
        vec!["D-002"],
        "termo em minuscula com acento casa"
    );
    assert!(
        busca(&mut s, "DECISÃO").is_empty(),
        "o acento nao e dobrado: 'Ã' != 'ã'"
    );
    assert_eq!(
        busca(&mut s, "SYNC"),
        vec!["D-001"],
        "tags tambem sao pesquisadas"
    );
}

// ------------------------------------------------------------------ Q3

#[test]
fn q03_limit_fora_de_1_a_50_e_invalid_params_e_padrao_e_20() {
    let m = Mundo::novo();
    let cfg = cfg_com(&[("max_open_proposals", json!(100))]);
    let mut s = servico_cfg(&m, cfg, &perfil_a());
    for i in 1..=25 {
        propoe_e_promove(&mut s, TOK_A, decisao(&format!("Item {i:02}"), "agentd"));
    }

    for limit in [51, 0] {
        let v = chama(&mut s, TOK_A, "query", &json!({"limit": limit}));
        assert_eq!(cod(&v), "invalid_params", "limit={limit}");
    }
    // controles dentro da faixa
    assert_eq!(
        consulta(&mut s, TOK_A, json!({"limit": 50}))
            .get("items")
            .unwrap()
            .as_array()
            .unwrap()
            .len(),
        25
    );
    assert_eq!(
        consulta(&mut s, TOK_A, json!({"limit": 1}))["items"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    // padrao 20
    let padrao = consulta(&mut s, TOK_A, json!({}));
    assert_eq!(
        padrao["items"].as_array().unwrap().len(),
        20,
        "limit padrao = 20"
    );
    assert_eq!(padrao["next_after"], json!("D-020"));
}

// ------------------------------------------------------------------ Q4

#[test]
fn q04_view_nunca_excede_view_budget_em_caracteres_e_marca_truncated() {
    let m = Mundo::novo();
    let mut s = servico_cfg(
        &m,
        cfg_com(&[("view_budget", json!(1000))]),
        &grants(&[perfil_com_intent()]),
    );
    for i in 1..=10 {
        propoe_e_promove(&mut s, TOK_A, decisao(&titulo_longo(i), "agentd"));
    }
    let v = res(&chama(&mut s, TOK_A, "view", &json!({})));
    assert!(
        chars_do_texto(&v) <= 1000,
        "view acima do orcamento: {} caracteres",
        chars_do_texto(&v)
    );
    assert_eq!(v["view_truncated"], json!(true));
    assert_eq!(v["trust"], json!("data"), "texto da view e dado");
    let linhas = linhas_visao(&v);
    assert!(
        conta_linhas_do_id(&linhas, "D-001") >= 1,
        "a primeira linha cabe"
    );
    assert_eq!(conta_linhas_do_id(&linhas, "D-010"), 0, "a ultima nao cabe");

    // controle: com orcamento grande, tudo cabe e nada e truncado
    let m2 = Mundo::novo();
    let mut s2 = servico_cfg(
        &m2,
        cfg_com(&[("view_budget", json!(20000))]),
        &grants(&[perfil_com_intent()]),
    );
    for i in 1..=10 {
        propoe_e_promove(&mut s2, TOK_A, decisao(&titulo_longo(i), "agentd"));
    }
    let v2 = res(&chama(&mut s2, TOK_A, "view", &json!({})));
    assert_ne!(v2["view_truncated"], json!(true), "controle: nao trunca");
    let linhas2 = linhas_visao(&v2);
    for n in 1..=10 {
        assert!(
            conta_linhas_do_id(&linhas2, &format!("D-{n:03}")) >= 1,
            "D-{n:03} na visao completa"
        );
    }
}

fn perfil_com_intent() -> Value {
    perfil(
        "agente-a",
        TOK_A,
        &["propose", "promote"],
        &["*"],
        &["*"],
        &["decision", "pending"],
        "2099-12-31",
        "Intencao de teste para o orcamento",
    )
}

// ------------------------------------------------------------------ Q5

#[test]
fn q05_item_maior_que_o_orcamento_restante_nao_avanca_o_cursor() {
    let m = Mundo::novo();
    let mut s = servico_cfg(&m, cfg_com(&[("view_budget", json!(1000))]), &perfil_a());
    for i in 1..=7 {
        propoe_e_promove(&mut s, TOK_A, decisao(&titulo_longo(i), "agentd"));
    }
    propoe_e_promove(&mut s, TOK_A, decisao("x", "agentd")); // D-008, pequeno, depois do grande

    let v = res(&chama(&mut s, TOK_A, "view", &json!({})));
    assert_eq!(v["view_truncated"], json!(true));
    let linhas = linhas_visao(&v);
    let mut entregues: Vec<String> = Vec::new();
    for l in &linhas {
        if let Some(id) = l.split(" | ").next() {
            if id.starts_with("D-") && !entregues.iter().any(|x| x == id) {
                entregues.push(id.to_string());
            }
        }
    }
    let k = entregues.len();
    assert!(
        (1..7).contains(&k),
        "entrega um prefixo que nao inclui tudo: {entregues:?}"
    );
    for (i, id) in entregues.iter().enumerate() {
        assert_eq!(
            id,
            &format!("D-{:03}", i + 1),
            "entrega sem pular: {entregues:?}"
        );
    }
    assert_eq!(
        conta_linhas_do_id(&linhas, "D-008"),
        0,
        "o pequeno depois do grande nao e entregue"
    );

    // o cursor e o ultimo item entregue, nao o que ficou de fora (next_after fica fora do text)
    let ultimo = format!("D-{k:03}");
    assert_eq!(v["next_after"], json!(ultimo));
    let depois = consulta(&mut s, TOK_A, json!({"limit": 1, "after": ultimo}));
    assert_eq!(
        ids_da_consulta(&depois),
        vec![format!("D-{:03}", k + 1)],
        "primeiro nao entregue"
    );
}

// ------------------------------------------------------------------ Q6

#[test]
fn q06_secao_due_respeita_view_overdue_max_os_mais_antigos_e_secoes_sao_disjuntas() {
    // sete pendencias vencidas + uma no futuro (controle). due = as view_overdue_max mais antigas;
    // as demais vencidas e a futura ficam em active. Cada item aparece em uma so secao.
    let m = Mundo::novo();
    let mut s = servico(&m, &perfil_a());
    let datas = [
        "2026-10-17",
        "2026-10-18",
        "2026-10-19",
        "2026-10-20",
        "2026-10-21",
        "2026-10-22",
        "2026-10-23",
    ];
    for (i, d) in datas.iter().enumerate() {
        propoe_e_promove(
            &mut s,
            TOK_A,
            pendencia(&format!("Pend {}", i + 1), "agentd", d),
        );
    }
    propoe_e_promove(
        &mut s,
        TOK_A,
        pendencia("Pend futura", "agentd", "2026-12-01"),
    ); // P-008
    m.relogio.poe("2026-10-25T00:00:00.000Z"); // P-001..P-007 vencidas; P-008 nao

    let v = res(&chama(&mut s, TOK_A, "view", &json!({})));
    for n in 1..=5 {
        assert_eq!(
            secoes_do_id(&v, &format!("P-{n:03}")),
            vec!["due".to_string()],
            "P-{n:03} so em due"
        );
    }
    for n in 6..=7 {
        assert_eq!(
            secoes_do_id(&v, &format!("P-{n:03}")),
            vec!["active".to_string()],
            "P-{n:03} so em active"
        );
    }
    assert_eq!(
        secoes_do_id(&v, "P-008"),
        vec!["active".to_string()],
        "controle: nao vencida so em active"
    );
}

#[test]
fn q06b_view_overdue_max_1_mostra_so_a_mais_antiga_em_due() {
    let m = Mundo::novo();
    let mut s = servico_cfg(&m, cfg_com(&[("view_overdue_max", json!(1))]), &perfil_a());
    for (i, d) in ["2026-10-17", "2026-10-18", "2026-10-19"]
        .iter()
        .enumerate()
    {
        propoe_e_promove(
            &mut s,
            TOK_A,
            pendencia(&format!("Pend {}", i + 1), "agentd", d),
        );
    }
    m.relogio.poe("2026-10-25T00:00:00.000Z");
    let v = res(&chama(&mut s, TOK_A, "view", &json!({})));
    assert_eq!(secoes_do_id(&v, "P-001"), vec!["due".to_string()]);
    assert_eq!(secoes_do_id(&v, "P-002"), vec!["active".to_string()]);
    assert_eq!(secoes_do_id(&v, "P-003"), vec!["active".to_string()]);
}

// ------------------------------------------------------------------ Q7

#[test]
fn q07_resposta_maior_que_response_max_nunca_e_enviada_e_vira_overloaded() {
    let m = Mundo::novo();
    let cfg = cfg_com(&[("response_max", json!(1024))]);
    let mut s = servico_cfg(&m, cfg, &perfil_a());

    let mut grande = decisao("Corpo grande", "agentd");
    grande["body"] = json!("a".repeat(1500));
    assert_eq!(
        res(&muta(&mut s, TOK_A, "propose", grande))["id"],
        json!("D-001"),
        "a mutacao grava; so a resposta e grande"
    );

    let raw = s.handle_line(&linha_req("c", TOK_A, "get", &json!({"id": "D-001"})));
    assert!(
        raw.len() <= 1024,
        "resposta enviada respeita response_max: {} bytes",
        raw.len()
    );
    assert_eq!(cod(&parse(&raw)), "overloaded");

    // controle: resposta pequena passa
    let mut pequena = decisao("Corpo pequeno", "agentd");
    pequena["body"] = json!("curto");
    propoe(&mut s, TOK_A, pequena);
    let raw2 = s.handle_line(&linha_req("c", TOK_A, "get", &json!({"id": "D-002"})));
    assert!(raw2.len() <= 1024);
    assert_eq!(res(&parse(&raw2))["id"], json!("D-002"));
}

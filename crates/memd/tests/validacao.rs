//! V1..V7 e faixas de `Config`/`Grants` (memd-spec-v0.md, secoes 3, 7 e 10).
//! Codigos nao listados na spec para uma condicao sao escolhidos e registrados em MEMD-TESTES.md.

mod common;

use common::*;
use serde_json::{Value, json};

fn perfil_a() -> String {
    grants(&[perfil_total("agente-a", TOK_A)])
}

/// Copia de `base` com um campo do registro removido (raiz ou um nivel dentro de `fields`).
fn sem(base: &Value, campo: &str) -> Value {
    let mut p = base.clone();
    p.as_object_mut().unwrap().remove(campo);
    p
}

fn sem_em_fields(base: &Value, campo: &str) -> Value {
    let mut p = base.clone();
    p["fields"].as_object_mut().unwrap().remove(campo);
    p
}

// ------------------------------------------------------------------ V1

#[test]
fn v01_sem_source_e_missing_source_e_source_malformada_e_invalid_params() {
    let m = Mundo::novo();
    let mut s = servico(&m, &perfil_a());

    let v = muta(
        &mut s,
        TOK_A,
        "propose",
        sem(&decisao("Sem fonte", "agentd"), "source"),
    );
    assert_eq!(cod(&v), "missing_source");
    assert_eq!(s.head_seq(), 0);

    // controle positivo: com source valida, grava
    assert_eq!(
        propoe(&mut s, TOK_A, decisao("Com fonte", "agentd"))["id"],
        json!("D-001")
    );

    // source com kind fora da lista, data invalida, ref vazia: a spec nao fixa o codigo; usamos invalid_params
    let mut kind = decisao("Kind errado", "agentd");
    kind["source"]["kind"] = json!("web");
    assert_eq!(cod(&muta(&mut s, TOK_A, "propose", kind)), "invalid_params");
    let mut data = decisao("Data errada", "agentd");
    data["source"]["at"] = json!("2026-02-30");
    assert_eq!(cod(&muta(&mut s, TOK_A, "propose", data)), "invalid_params");
    let mut vazia = decisao("Ref vazia", "agentd");
    vazia["source"]["ref"] = json!("");
    assert_eq!(
        cod(&muta(&mut s, TOK_A, "propose", vazia)),
        "invalid_params"
    );
    assert_eq!(s.head_seq(), 1);
}

// ------------------------------------------------------------------ V2

/// Um padrao da lista de segredos e se ele pode aparecer em `tags` (so [a-z0-9_-]).
fn padroes_de_segredo() -> Vec<(String, bool)> {
    vec![
        ("-----BEGIN OPENSSH PRIVATE KEY-----".to_string(), false),
        ("AKIAABCDEFGHIJKLMNOP".to_string(), false),
        (format!("ghp_{}", "a".repeat(36)), false), // 40 caracteres: tag tem no maximo 24
        (format!("sk-{}", "abcdefghijklmnopqrst"), true),
        ("xoxb-0123456789".to_string(), true),
        (
            "eyJhbGciOiJIUzI1NiJ9.eyJzdWIiOiIxMjM0In0.abcdefghijklmn".to_string(),
            false,
        ),
        ("segredo-operador".to_string(), true), // secret_patterns do operador (substring literal)
    ]
}

/// Decisao com o campo textual `campo` substituido por `texto`.
fn com_texto(campo: &str, texto: &str) -> Value {
    let mut p = decisao("Titulo base", "agentd");
    match campo {
        "title" => p["title"] = json!(texto),
        "body" => p["body"] = json!(texto),
        "tags" => p["tags"] = json!([texto]),
        "criterion" => p["fields"]["criterion"] = json!(texto),
        "source.ref" => p["source"]["ref"] = json!(texto),
        "refs.ref" => p["refs"] = json!([{"kind": "git", "ref": texto}]),
        outro => panic!("campo desconhecido no teste: {outro}"),
    }
    p
}

#[test]
fn v02_segredo_em_cada_campo_textual_e_secret_detected() {
    let m = Mundo::novo();
    let cfg = cfg_com(&[("secret_patterns", json!(["segredo-operador"]))]);
    let mut s = servico_cfg(&m, cfg, &perfil_a());

    let campos = [
        "title",
        "body",
        "tags",
        "criterion",
        "source.ref",
        "refs.ref",
    ];
    for (padrao, tag_ok) in padroes_de_segredo() {
        for campo in campos {
            if campo == "tags" && !tag_ok {
                continue; // tags so aceitam [a-z0-9_-]: o padrao nao chega a ser testado la
            }
            let texto = format!("prefixo {padrao} sufixo");
            let texto = if campo == "tags" {
                padrao.clone()
            } else {
                texto
            };
            let v = muta(&mut s, TOK_A, "propose", com_texto(campo, &texto));
            assert_eq!(
                cod(&v),
                "secret_detected",
                "padrao {padrao:?} no campo {campo}"
            );
            let bruto = serde_json::to_string(&v).unwrap();
            assert!(
                !bruto.contains(&padrao),
                "erro nao pode ecoar o segredo ({campo})"
            );
        }
    }
    assert_eq!(s.head_seq(), 0, "nenhum segredo gravou");

    // controle: o mesmo campo com texto inocente grava
    for campo in campos {
        let v = muta(&mut s, TOK_A, "propose", com_texto(campo, "texto-inocente"));
        assert!(v["ok"] == json!(true), "controle {campo}: {v}");
    }
}

#[test]
fn v02b_segredo_no_motivo_de_reject_e_secret_detected() {
    let m = Mundo::novo();
    let cfg = cfg_com(&[("secret_patterns", json!(["segredo-operador"]))]);
    let mut s = servico_cfg(&m, cfg, &perfil_a());
    propoe(&mut s, TOK_A, decisao("Para rejeitar", "agentd")); // D-001 (seq 1)

    for (padrao, _) in padroes_de_segredo() {
        let v = muta(
            &mut s,
            TOK_A,
            "reject",
            json!({"id": "D-001", "rev": 1, "reason": format!("motivo {padrao}")}),
        );
        assert_eq!(cod(&v), "secret_detected", "reason com {padrao:?}");
    }
    assert_eq!(s.head_seq(), 1);
    assert_eq!(
        res(&get(&mut s, TOK_A, "D-001"))["status"],
        json!("proposed")
    );

    // controle: motivo inocente rejeita
    let ok = res(&muta(
        &mut s,
        TOK_A,
        "reject",
        json!({"id": "D-001", "rev": 1, "reason": "fora de escopo"}),
    ));
    assert_eq!(ok["status"], json!("rejected"));
}

// ------------------------------------------------------------------ V3

#[test]
fn v03_body_limite_de_4096_bytes_em_utf8() {
    let m = Mundo::novo();
    let mut s = servico(&m, &perfil_a());

    let mut no_limite = decisao("Corpo no limite", "agentd");
    no_limite["body"] = json!("a".repeat(4096));
    assert_eq!(
        res(&muta(&mut s, TOK_A, "propose", no_limite))["id"],
        json!("D-001"),
        "4096 bytes passa"
    );

    let mut acima = decisao("Corpo acima", "agentd");
    acima["body"] = json!("a".repeat(4097));
    assert_eq!(
        cod(&muta(&mut s, TOK_A, "propose", acima)),
        "body_too_large"
    );

    // limite em bytes, nao em caracteres: 2048 'é' = 4096 bytes passa; 'a' + 2048 'é' = 4097 nao
    let mut dois_bytes = decisao("Corpo de dois bytes", "agentd");
    dois_bytes["body"] = json!("é".repeat(2048));
    assert_eq!(
        res(&muta(&mut s, TOK_A, "propose", dois_bytes))["id"],
        json!("D-002")
    );

    let mut dois_bytes_mais = decisao("Corpo de dois bytes mais um", "agentd");
    dois_bytes_mais["body"] = json!(format!("a{}", "é".repeat(2048)));
    assert_eq!(
        cod(&muta(&mut s, TOK_A, "propose", dois_bytes_mais)),
        "body_too_large"
    );
    assert_eq!(s.head_seq(), 2);
}

// ------------------------------------------------------------------ V4

#[test]
fn v04_pending_exige_review_by_owner_next_action_e_state() {
    let m = Mundo::novo();
    let mut s = servico(&m, &perfil_a());
    let base = pendencia("Pendencia", "agentd", "2026-11-01");

    let recusas = [
        ("sem review_by", sem(&base, "review_by")),
        ("sem owner", sem_em_fields(&base, "owner")),
        ("sem next_action", sem_em_fields(&base, "next_action")),
        ("sem state", sem_em_fields(&base, "state")),
        ("state fora da lista", {
            let mut p = base.clone();
            p["fields"]["state"] = json!("aberto");
            p
        }),
        (
            "review_by no passado (ontem)",
            com(&base, "review_by", json!("2026-10-15")),
        ),
        (
            "review_by data invalida",
            com(&base, "review_by", json!("2026-02-30")),
        ),
    ];
    for (nome, p) in recusas {
        let v = muta(&mut s, TOK_A, "propose", p);
        assert_eq!(cod(&v), "invalid_params", "{nome}");
    }
    assert_eq!(s.head_seq(), 0);

    // controles: completa grava; review_by hoje (2026-10-16) tambem vale
    assert_eq!(propoe(&mut s, TOK_A, base)["id"], json!("P-001"));
    assert_eq!(
        propoe(
            &mut s,
            TOK_A,
            com(
                &pendencia("Hoje", "agentd", "2026-10-16"),
                "review_by",
                json!("2026-10-16")
            )
        )["id"],
        json!("P-002")
    );
}

// ------------------------------------------------------------------ V5

#[test]
fn v05_campos_desconhecidos_e_reservados_do_cliente_sao_recusados() {
    let m = Mundo::novo();
    let mut s = servico(&m, &perfil_a());
    let base = decisao("Base", "agentd");

    for campo in [
        "author",
        "status",
        "seq",
        "ts",
        "promoted_by",
        "proposed_seq",
        "promoted_seq",
        "superseded_by",
        "extra",
    ] {
        let p = com(&base, campo, json!("valor"));
        assert_eq!(
            cod(&muta(&mut s, TOK_A, "propose", p)),
            "invalid_params",
            "campo {campo}"
        );
    }
    // campo desconhecido dentro de fields (objeto fechado) e campo de pendencia em decisao
    let mut dentro = base.clone();
    dentro["fields"]["extra"] = json!(1);
    assert_eq!(
        cod(&muta(&mut s, TOK_A, "propose", dentro)),
        "invalid_params"
    );
    let mut owner_em_decisao = base.clone();
    owner_em_decisao["fields"]["owner"] = json!("agente-a");
    assert_eq!(
        cod(&muta(&mut s, TOK_A, "propose", owner_em_decisao)),
        "invalid_params"
    );
    assert_eq!(s.head_seq(), 0);

    // controle: o mesmo registro sem extras grava
    assert_eq!(propoe(&mut s, TOK_A, base)["id"], json!("D-001"));
}

// ------------------------------------------------------------------ V6

#[test]
fn v06_review_by_vencido_vira_unconfirmed_e_status_continua_active() {
    let m = Mundo::novo();
    let mut s = servico(&m, &perfil_a());
    propoe_e_promove(
        &mut s,
        TOK_A,
        pendencia("Pendencia de teste", "agentd", "2026-10-20"),
    ); // P-001

    // controle: no proprio dia do review_by (2026-10-20 UTC) ainda nao esta vencido
    m.relogio.poe("2026-10-20T23:59:00.000Z");
    let g0 = res(&get(&mut s, TOK_A, "P-001"));
    assert_eq!(g0["unconfirmed"], json!(false));
    assert_eq!(g0["status"], json!("active"));

    // no dia seguinte: vencido, derivado; status nao muda sozinho
    m.relogio.poe("2026-10-21T00:00:00.000Z");
    let g1 = res(&get(&mut s, TOK_A, "P-001"));
    assert_eq!(g1["unconfirmed"], json!(true));
    assert_eq!(g1["status"], json!("active"));

    let q = consulta(&mut s, TOK_A, json!({"type": "pending"}));
    let item = &q["items"][0];
    assert_eq!(item["id"], json!("P-001"));
    assert_eq!(item["unconfirmed"], json!(true));

    let linhas = linhas_visao(&res(&chama(&mut s, TOK_A, "view", &json!({}))));
    assert!(
        linhas
            .iter()
            .any(|l| l == "P-001 | Pendencia de teste | agentd | 2026-10-20"),
        "view mostra a linha id | title | scope | review_by: {linhas:?}"
    );
}

// ------------------------------------------------------------------ V7

#[test]
fn v07_pendencia_done_sai_da_view_e_do_query_padrao_mas_fica_ativa() {
    let m = Mundo::novo();
    let mut s = servico(&m, &perfil_a());
    propoe_e_promove(
        &mut s,
        TOK_A,
        pendencia("Fazer isto", "agentd", "2026-11-01"),
    ); // P-001 rev1 active

    // controle: antes de concluir, aparece na view e no query padrao
    let q0 = consulta(&mut s, TOK_A, json!({"type": "pending"}));
    assert_eq!(ids_da_consulta(&q0), vec!["P-001".to_string()]);
    assert!(
        conta_linhas_do_id(
            &linhas_visao(&res(&chama(&mut s, TOK_A, "view", &json!({})))),
            "P-001"
        ) >= 1
    );

    // conclui por revisao (imutabilidade: propose de revisao + promote)
    let mut concluida = pendencia("Fazer isto", "agentd", "2026-11-01");
    concluida["fields"]["state"] = json!("done");
    concluida["id"] = json!("P-001");
    concluida["expected_rev"] = json!(1);
    let p = propoe(&mut s, TOK_A, concluida); // seq 3
    promove(&mut s, TOK_A, "P-001", 2, p["seq"].as_u64().unwrap());

    let g = res(&get(&mut s, TOK_A, "P-001"));
    assert_eq!(g["rev"], json!(2));
    assert_eq!(g["status"], json!("active"), "done continua active");
    assert_eq!(g["fields"]["state"], json!("done"));

    assert!(
        ids_da_consulta(&consulta(&mut s, TOK_A, json!({"type": "pending"}))).is_empty(),
        "query padrao esconde done"
    );
    let feitas = consulta(&mut s, TOK_A, json!({"type": "pending", "state": "done"}));
    assert_eq!(
        ids_da_consulta(&feitas),
        vec!["P-001".to_string()],
        "state=done explicito mostra"
    );

    let linhas = linhas_visao(&res(&chama(&mut s, TOK_A, "view", &json!({}))));
    assert_eq!(
        conta_linhas_do_id(&linhas, "P-001"),
        0,
        "view nao mostra done"
    );
}

// ------------------------------------------------------------------ faixas de Config e Grants

#[test]
fn v08_config_aceita_as_faixas_e_recusa_fora_delas_e_chave_desconhecida() {
    assert!(
        memd::Config::from_json(&cfg_json_com(&[])).is_ok(),
        "configuracao completa valida"
    );

    let fora = [
        ("proposal_ttl_days", json!(0)),
        ("proposal_ttl_days", json!(91)),
        ("view_budget", json!(999)),
        ("view_budget", json!(20001)),
        ("view_overdue_max", json!(0)),
        ("view_overdue_max", json!(21)),
    ];
    for (k, v) in fora {
        let r = memd::Config::from_json(&cfg_json_com(&[(k, v.clone())]));
        assert!(r.is_err(), "{k}={v} deve ser recusado");
    }
    let dentro = [
        ("proposal_ttl_days", json!(1)),
        ("proposal_ttl_days", json!(90)),
        ("view_budget", json!(1000)),
        ("view_budget", json!(20000)),
        ("view_overdue_max", json!(1)),
        ("view_overdue_max", json!(20)),
    ];
    for (k, v) in dentro {
        let r = memd::Config::from_json(&cfg_json_com(&[(k, v.clone())]));
        assert!(r.is_ok(), "{k}={v} deve ser aceito: {:?}", r.err());
    }

    // chave desconhecida = recusa
    let mut c = base_cfg();
    c["chave_inventada"] = json!(1);
    assert!(memd::Config::from_json(&c.to_string()).is_err());
}

#[test]
fn v08b_grants_malformado_e_recusado_e_lista_valida_e_aceita() {
    assert!(memd::Grants::from_json("{").is_err());
    assert!(memd::Grants::from_json("nao e json").is_err());
    assert!(memd::Grants::from_json(&grants(&[perfil_total("agente-a", TOK_A)])).is_ok());
}

#[test]
fn v08c_token_de_teste_tem_mt_e_43_base64url() {
    for t in [TOK_A, TOK_B, TOK_SPEC, TOK_SENTINELA, TOK_ERRADO] {
        let corpo = t.strip_prefix("mt_").expect("prefixo mt_");
        assert_eq!(corpo.len(), 43, "{t}");
        assert!(
            corpo
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_'),
            "{t}"
        );
        assert_eq!(memd::token_sha256_hex(t).len(), 64);
    }
}

// ------------------------------------------------------------------ limites da secao 3 (fora da lista da secao 10)

fn recusa_invalida(s: &mut memd::Service, p: Value) {
    let v = muta(s, TOK_A, "propose", p.clone());
    assert_eq!(cod(&v), "invalid_params", "deveria recusar: {p}");
}

#[test]
fn v09_limites_de_campos_da_secao_3_e_round_trip_de_refs_e_source() {
    let m = Mundo::novo();
    let mut s = servico(&m, &perfil_a());

    // title: 1 a 120 caracteres
    recusa_invalida(&mut s, decisao(&"t".repeat(121), "agentd"));
    recusa_invalida(&mut s, decisao("", "agentd"));
    assert_eq!(
        propoe(&mut s, TOK_A, decisao(&"t".repeat(120), "agentd"))["id"],
        json!("D-001"),
        "120 caracteres passa"
    );

    // scope: [a-z0-9._/-]{1,64}
    recusa_invalida(&mut s, decisao("Escopo maiusculo", "AGENTD"));
    recusa_invalida(&mut s, decisao("Escopo longo", &"a".repeat(65)));
    assert_eq!(
        propoe(&mut s, TOK_A, decisao("Escopo com barra", "agentd/sub.x-1"))["id"],
        json!("D-002")
    );

    // tags: 0 a 8, de [a-z0-9_-]{1,24}
    let nove: Vec<String> = (0..9).map(|i| format!("t{i}")).collect();
    let mut p = decisao("Nove tags", "agentd");
    p["tags"] = json!(nove);
    recusa_invalida(&mut s, p);
    let mut maiuscula = decisao("Tag maiuscula", "agentd");
    maiuscula["tags"] = json!(["Tag"]);
    recusa_invalida(&mut s, maiuscula);
    let oito: Vec<String> = (0..8).map(|i| format!("t{i}")).collect();
    let mut p8 = decisao("Oito tags", "agentd");
    p8["tags"] = json!(oito);
    assert_eq!(
        propoe(&mut s, TOK_A, p8)["id"],
        json!("D-003"),
        "8 tags passam"
    );

    // supersedes: 0 a 4 ids
    let mut cinco = decisao("Cinco supersedes", "agentd");
    cinco["supersedes"] = json!(["D-001", "D-002", "D-003", "D-004", "D-005"]);
    recusa_invalida(&mut s, cinco);

    // refs: 0 a 8, kind em chatd|git|doc|grant, ref ate 200
    let mut kind = decisao("Kind de ref", "agentd");
    kind["refs"] = json!([{"kind": "web", "ref": "x"}]);
    recusa_invalida(&mut s, kind);
    let mut nove_refs = decisao("Nove refs", "agentd");
    nove_refs["refs"] = json!(
        (0..9)
            .map(|i| json!({"kind": "git", "ref": format!("r{i}")}))
            .collect::<Vec<_>>()
    );
    recusa_invalida(&mut s, nove_refs);

    // fields.criterion: ate 400 caracteres
    let mut longo = decisao("Criterio longo", "agentd");
    longo["fields"]["criterion"] = json!("c".repeat(401));
    recusa_invalida(&mut s, longo);

    // round trip: refs e source sao guardados e devolvidos como vieram
    let mut com_refs = decisao("Com refs", "agentd");
    com_refs["refs"] =
        json!([{"kind": "chatd", "ref": "msg-621"}, {"kind": "grant", "ref": "g-1"}]);
    com_refs["source"] = json!({"kind": "msg", "ref": "codex-blocks 621", "at": "2026-10-16"});
    let id = propoe(&mut s, TOK_A, com_refs)["id"]
        .as_str()
        .unwrap()
        .to_string();
    let g = res(&get(&mut s, TOK_A, &id));
    assert_eq!(
        g["refs"],
        json!([{"kind": "chatd", "ref": "msg-621"}, {"kind": "grant", "ref": "g-1"}])
    );
    assert_eq!(
        g["source"],
        json!({"kind": "msg", "ref": "codex-blocks 621", "at": "2026-10-16"})
    );

    // criterion de 400 caracteres passa (controle do limite)
    let mut no_limite = decisao("Criterio no limite", "agentd");
    no_limite["fields"]["criterion"] = json!("c".repeat(400));
    assert_eq!(
        res(&muta(&mut s, TOK_A, "propose", no_limite))["id"],
        json!("D-005")
    );
}

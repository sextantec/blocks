//! A1..A7, A9, A10: autorizacao (memd-spec-v0.md, secoes 5 e 10). Caixa-preta por `Service`.
//! A8 (nenhuma operacao altera grants.json) e binario: ver binario.rs.

mod common;

use common::*;
use serde_json::{Value, json};

fn grants_objeto(perfis: &[Value]) -> memd::Grants {
    memd::Grants::from_json(&grants(perfis)).expect("grants de teste")
}

fn perfil_com(
    agente: &str,
    token: &str,
    ops: &[&str],
    leitura: &[&str],
    escrita: &[&str],
) -> Value {
    perfil(
        agente,
        token,
        ops,
        leitura,
        escrita,
        &["decision", "pending"],
        "2099-12-31",
        "teste",
    )
}

// ------------------------------------------------------------------ A1

#[test]
fn a01_sem_op_e_forbidden_em_cada_mutacao_com_controle_positivo() {
    let m = Mundo::novo();
    let perfis = grants(&[
        perfil_total("agente-a", TOK_A),
        perfil_com("agente-leitor", TOK_B, &[], &["*"], &["*"]),
        perfil_com("agente-propoe", TOK_SPEC, &["propose"], &["*"], &["*"]),
    ]);
    let mut s = servico(&m, &perfis);
    propoe_e_promove(&mut s, TOK_A, decisao("Ativa", "agentd")); // D-001 active (seq 1, 2)
    propoe(&mut s, TOK_A, decisao("Proposta", "agentd")); // D-002 proposed (seq 3)
    let head = s.head_seq();

    // leitor (ops vazio): todas as mutacoes sao forbidden
    assert_eq!(
        cod(&muta(&mut s, TOK_B, "propose", decisao("Nova", "agentd"))),
        "forbidden"
    );
    assert_eq!(
        cod(&muta(
            &mut s,
            TOK_B,
            "propose",
            json!({"type":"decision","id":"D-001","expected_rev":1,"title":"x","body":"","scope":"agentd","tags":[],"fields":{"criterion":"c"},"source":fonte()})
        )),
        "forbidden",
        "revisao tambem"
    );
    assert_eq!(
        cod(&muta(
            &mut s,
            TOK_B,
            "promote",
            json!({"id":"D-002","rev":1,"expected_seq":3})
        )),
        "forbidden"
    );
    assert_eq!(
        cod(&muta(
            &mut s,
            TOK_B,
            "reject",
            json!({"id":"D-002","rev":1,"reason":"x"})
        )),
        "forbidden"
    );
    assert_eq!(
        cod(&muta(
            &mut s,
            TOK_B,
            "retire",
            json!({"id":"D-001","rev":1,"reason":"x"})
        )),
        "forbidden"
    );
    assert_eq!(s.head_seq(), head, "nada gravado pelo leitor");
    assert_eq!(
        res(&get(&mut s, TOK_B, "D-001"))["status"],
        json!("active"),
        "leitura continua permitida"
    );

    // controle positivo: agente com ops = ["propose"] propoe, mas nao promove
    let p = propoe(&mut s, TOK_SPEC, decisao("Do proponente", "agentd"));
    assert_eq!(p["id"], json!("D-003"));
    assert_eq!(
        cod(&muta(
            &mut s,
            TOK_SPEC,
            "promote",
            json!({"id":"D-002","rev":1,"expected_seq":3})
        )),
        "forbidden",
        "ops sem promote nao promove"
    );
}

// ------------------------------------------------------------------ A2

#[test]
fn a02_write_scopes_que_nao_casam_sao_forbidden() {
    let m = Mundo::novo();
    let perfis = grants(&[
        perfil_total("agente-a", TOK_A),
        perfil_com(
            "agente-w",
            TOK_B,
            &["propose", "promote"],
            &["*"],
            &["agentd"],
        ),
    ]);
    let mut s = servico(&m, &perfis);
    propoe(&mut s, TOK_A, decisao("Escopo blocklog", "blocklog")); // D-001 (seq 1)
    propoe(&mut s, TOK_A, decisao("Escopo agentd", "agentd")); // D-002 (seq 2)
    let head = s.head_seq();

    assert_eq!(
        cod(&muta(&mut s, TOK_B, "propose", decisao("Fora", "blocklog"))),
        "forbidden"
    );
    assert_eq!(s.head_seq(), head);
    // promote de registro fora do write_scopes
    assert_eq!(
        cod(&muta(
            &mut s,
            TOK_B,
            "promote",
            json!({"id":"D-001","rev":1,"expected_seq":1})
        )),
        "forbidden"
    );
    assert_eq!(s.head_seq(), head);

    // controles positivos: escopo casando com write_scopes
    let ok = propoe(&mut s, TOK_B, decisao("Dentro", "agentd"));
    assert_eq!(ok["id"], json!("D-003"));
    assert_eq!(
        promove(&mut s, TOK_B, "D-002", 1, 2)["status"],
        json!("active")
    );
}

// ------------------------------------------------------------------ A3

#[test]
fn a03_read_scopes_que_nao_casam_nao_mostram_nem_confirmam_o_registro() {
    let m = Mundo::novo();
    let perfis = grants(&[
        perfil_total("agente-a", TOK_A),
        perfil_com("agente-b", TOK_B, &[], &["agentd"], &[]),
    ]);
    let mut s = servico(&m, &perfis);

    // A cria a cadeia: D-001 (blocklog) <- D-002 (agentd); D-003 (agentd) <- D-004 (blocklog)
    propoe_e_promove(&mut s, TOK_A, decisao("Oculta antiga", "blocklog")); // D-001 seq 1,2
    let mut d2 = decisao("Visivel nova", "agentd");
    d2["supersedes"] = json!(["D-001"]);
    propoe(&mut s, TOK_A, d2); // D-002 seq 3
    promove(&mut s, TOK_A, "D-002", 1, 3); // seq 4
    propoe_e_promove(&mut s, TOK_A, decisao("Visivel antiga", "agentd")); // D-003 seq 5,6
    let mut d4 = decisao("Oculta nova", "blocklog");
    d4["supersedes"] = json!(["D-003"]);
    propoe(&mut s, TOK_A, d4); // D-004 seq 7
    promove(&mut s, TOK_A, "D-004", 1, 7); // seq 8

    // controle: A ve a cadeia completa
    assert_eq!(
        res(&get(&mut s, TOK_A, "D-002"))["supersedes"],
        json!(["D-001"])
    );

    // B (le so "agentd") nao ve D-001 nem D-004 por id, por rev, nem em query/view
    assert_eq!(cod(&get(&mut s, TOK_B, "D-001")), "not_found");
    assert_eq!(cod(&get_com_rev(&mut s, TOK_B, "D-001", 1)), "not_found");
    assert_eq!(cod(&get(&mut s, TOK_B, "D-004")), "not_found");

    let ativos = consulta(&mut s, TOK_B, json!({"type": "decision"}));
    assert_eq!(
        ids_da_consulta(&ativos),
        vec!["D-002".to_string()],
        "query nao mostra D-001/D-004"
    );
    let supers = consulta(
        &mut s,
        TOK_B,
        json!({"type": "decision", "status": "superseded"}),
    );
    assert_eq!(ids_da_consulta(&supers), vec!["D-003".to_string()]);

    let visao = linhas_visao(&res(&chama(&mut s, TOK_B, "view", &json!({}))));
    assert!(
        visao.iter().any(|l| l.starts_with("D-002 | ")),
        "view mostra D-002: {visao:?}"
    );
    assert!(
        !visao
            .iter()
            .any(|l| l.contains("D-001") || l.contains("D-004")),
        "view nao vaza: {visao:?}"
    );

    // controle da cadeia: B ve D-002 com supersedes oculto; D-003 com superseded_by oculto
    let d2b = res(&get(&mut s, TOK_B, "D-002"));
    assert_eq!(
        d2b["supersedes"],
        json!(["(oculto)"]),
        "id ilegivel sai como (oculto)"
    );
    let d3b = res(&get(&mut s, TOK_B, "D-003"));
    assert_eq!(
        d3b["superseded_by"],
        json!("(oculto)"),
        "superseded_by ilegivel sai como (oculto)"
    );
}

fn get_com_rev(s: &mut memd::Service, token: &str, id: &str, rev: u64) -> serde_json::Value {
    chama(s, token, "get", &json!({"id": id, "rev": rev}))
}

// ------------------------------------------------------------------ A4

#[test]
fn a04_id_existente_sem_permissao_e_id_inexistente_respondem_identico() {
    let m = Mundo::novo();
    let perfis = grants(&[
        perfil_total("agente-a", TOK_A),
        perfil_com(
            "agente-b",
            TOK_B,
            &["propose", "promote"],
            &["agentd"],
            &["agentd"],
        ),
    ]);
    let mut s = servico(&m, &perfis);
    propoe_e_promove(&mut s, TOK_A, decisao("Oculta", "blocklog")); // D-001 active (seq 1,2)
    let head = s.head_seq();

    // pares: (requisicao com id existente sem permissao, requisicao com id inexistente).
    // Mesmo id de cliente: a linha de resposta inteira tem que ser igual, byte a byte.
    let pares: Vec<(&str, Value, Value)> = vec![
        ("get", json!({"id": "D-001"}), json!({"id": "D-999"})),
        (
            "get",
            json!({"id": "D-001", "rev": 1}),
            json!({"id": "D-999", "rev": 1}),
        ),
        (
            "promote",
            json!({"id": "D-001", "rev": 1, "expected_seq": 2, "request_id": "a4-p1"}),
            json!({"id": "D-999", "rev": 1, "expected_seq": 2, "request_id": "a4-p2"}),
        ),
        (
            "reject",
            json!({"id": "D-001", "rev": 1, "reason": "x", "request_id": "a4-r1"}),
            json!({"id": "D-999", "rev": 1, "reason": "x", "request_id": "a4-r2"}),
        ),
        (
            "retire",
            json!({"id": "D-001", "rev": 1, "reason": "x", "request_id": "a4-t1"}),
            json!({"id": "D-999", "rev": 1, "reason": "x", "request_id": "a4-t2"}),
        ),
        (
            "propose",
            json!({"type": "decision", "id": "D-001", "expected_rev": 1, "title": "Rev", "body": "", "scope": "agentd", "tags": [], "fields": {"criterion": "c"}, "source": fonte(), "request_id": "a4-v1"}),
            json!({"type": "decision", "id": "D-999", "expected_rev": 1, "title": "Rev", "body": "", "scope": "agentd", "tags": [], "fields": {"criterion": "c"}, "source": fonte(), "request_id": "a4-v2"}),
        ),
    ];

    for (metodo, com_permissao_falta, inexistente) in &pares {
        let linha_a = s.handle_line(&linha_req("c", TOK_B, metodo, com_permissao_falta));
        let linha_b = s.handle_line(&linha_req("c", TOK_B, metodo, inexistente));
        assert_eq!(linha_a, linha_b, "{metodo}: respostas devem ser identicas");
        assert_eq!(cod(&parse(&linha_a)), "not_found", "{metodo}");
    }
    assert_eq!(s.head_seq(), head, "mutacoes cegas nao gravam");
    assert_eq!(res(&get(&mut s, TOK_A, "D-001"))["status"], json!("active"));

    // controle positivo: o mesmo agente le o que pode ler
    propoe(&mut s, TOK_B, decisao("Dentro", "agentd"));
    assert_eq!(
        res(&get(&mut s, TOK_B, "D-002"))["status"],
        json!("proposed")
    );
}

// ------------------------------------------------------------------ A5

#[test]
fn a05_token_errado_expirado_ou_removido_e_unauthorized() {
    let m = Mundo::novo(); // 2026-10-16T22:00Z
    let perfis = grants(&[
        perfil_total("agente-a", TOK_A),
        perfil(
            "agente-b",
            TOK_B,
            &["propose", "promote"],
            &["*"],
            &["*"],
            &["decision", "pending"],
            "2026-10-17",
            "expira",
        ),
        perfil_total("agente-spec", TOK_SPEC),
    ]);
    let mut s = servico(&m, &perfis);

    // controle: o token com validade ate 2026-10-17 vale no proprio dia (UTC)
    assert_eq!(
        res(&chama(&mut s, TOK_B, "whoami", &json!({})))["agent"],
        json!("agente-b")
    );
    m.relogio.poe("2026-10-17T12:00:00.000Z");
    assert_eq!(
        res(&chama(&mut s, TOK_B, "whoami", &json!({})))["agent"],
        json!("agente-b")
    );

    // expirado: a partir de 2026-10-18 = unauthorized; o outro token segue valendo
    m.relogio.poe("2026-10-18T00:00:00.000Z");
    assert_eq!(
        cod(&chama(&mut s, TOK_B, "whoami", &json!({}))),
        "unauthorized"
    );
    assert_eq!(
        cod(&chama(&mut s, TOK_B, "status", &json!({}))),
        "unauthorized"
    );
    assert_eq!(
        res(&chama(&mut s, TOK_A, "whoami", &json!({})))["agent"],
        json!("agente-a")
    );

    // token que nao esta no grants
    assert_eq!(
        cod(&chama(&mut s, TOK_ERRADO, "whoami", &json!({}))),
        "unauthorized"
    );
    let head = s.head_seq();
    assert_eq!(
        cod(&muta(
            &mut s,
            TOK_ERRADO,
            "propose",
            decisao("Intruso", "agentd")
        )),
        "unauthorized"
    );
    assert_eq!(s.head_seq(), head);

    // removido: controle antes, unauthorized depois do recarregamento (SIGHUP equivalente)
    assert_eq!(
        res(&chama(&mut s, TOK_SPEC, "whoami", &json!({})))["agent"],
        json!("agente-spec")
    );
    s.reload_grants(grants_objeto(&[perfil_total("agente-a", TOK_A)]));
    assert_eq!(
        cod(&chama(&mut s, TOK_SPEC, "whoami", &json!({}))),
        "unauthorized"
    );
    assert_eq!(
        res(&chama(&mut s, TOK_A, "whoami", &json!({})))["agent"],
        json!("agente-a")
    );
}

// ------------------------------------------------------------------ A6

#[test]
fn a06_types_restringe_leitura_e_escrita_por_tipo() {
    let m = Mundo::novo();
    let perfis = grants(&[
        perfil_total("agente-a", TOK_A),
        perfil(
            "agente-d",
            TOK_B,
            &["propose", "promote"],
            &["*"],
            &["*"],
            &["decision"],
            "2099-12-31",
            "so decisoes",
        ),
    ]);
    let mut s = servico(&m, &perfis);
    propoe(&mut s, TOK_A, decisao("Decisao", "agentd")); // D-001 (seq 1)
    propoe(
        &mut s,
        TOK_A,
        pendencia("Pendencia", "agentd", "2026-11-01"),
    ); // P-001 (seq 2)
    let head = s.head_seq();

    // escrita de tipo fora de types: forbidden (a spec so diz isso para leitura; ver MEMD-TESTES.md)
    assert_eq!(
        cod(&muta(
            &mut s,
            TOK_B,
            "propose",
            pendencia("Nao pode", "agentd", "2026-11-01")
        )),
        "forbidden"
    );
    assert_eq!(s.head_seq(), head);

    // leitura: P-001 nao aparece nem por get, nem por query de tipo pending
    assert_eq!(cod(&get(&mut s, TOK_B, "P-001")), "not_found");
    let pend = consulta(
        &mut s,
        TOK_B,
        json!({"type": "pending", "status": "proposed"}),
    );
    assert!(
        ids_da_consulta(&pend).is_empty(),
        "query de tipo fora de types nao lista"
    );

    // controles: decisao lida e escrita
    let dec = consulta(&mut s, TOK_B, json!({"status": "proposed"}));
    assert_eq!(ids_da_consulta(&dec), vec!["D-001".to_string()]);
    assert_eq!(
        propoe(&mut s, TOK_B, decisao("Segunda decisao", "agentd"))["id"],
        json!("D-002")
    );
}

// ------------------------------------------------------------------ A7

#[test]
fn a07_whoami_devolve_o_perfil_e_o_intent() {
    let m = Mundo::novo();
    let perfis = grants(&[perfil(
        "codex-blocks",
        TOK_A,
        &["propose"],
        &["agentd", "blocklog/**"],
        &["agentd"],
        &["decision"],
        "2099-12-31",
        "Registrar decisoes do agentd",
    )]);
    let mut s = servico(&m, &perfis);
    let r = res(&chama(&mut s, TOK_A, "whoami", &json!({})));
    assert_eq!(
        r,
        json!({
            "agent": "codex-blocks",
            "ops": ["propose"],
            "read_scopes": ["agentd", "blocklog/**"],
            "write_scopes": ["agentd"],
            "types": ["decision"],
            "expires": "2099-12-31",
            "intent": "Registrar decisoes do agentd"
        })
    );
}

// ------------------------------------------------------------------ A9

#[test]
fn a09_status_nao_revela_escopos_nem_titulos() {
    let m = Mundo::novo();
    let perfis = grants(&[
        perfil_total("agente-a", TOK_A),
        perfil_com("agente-b", TOK_B, &[], &["agentd"], &[]),
    ]);
    let mut s = servico(&m, &perfis);
    propoe_e_promove(
        &mut s,
        TOK_A,
        decisao("Titulo secreto da blocklog", "blocklog"),
    ); // seq 1,2
    propoe(&mut s, TOK_A, decisao("Visivel", "agentd")); // seq 3

    let st_a = res(&chama(&mut s, TOK_A, "status", &json!({})));
    let st_b = res(&chama(&mut s, TOK_B, "status", &json!({})));
    let chaves: Vec<String> = {
        let mut k: Vec<String> = st_b.as_object().unwrap().keys().cloned().collect();
        k.sort();
        k
    };
    assert_eq!(chaves, vec!["broken", "counts", "head_seq", "uptime_s"]);
    assert_eq!(st_b["head_seq"], json!(3));
    assert_eq!(st_b["broken"], json!(false));

    // status global: igual para quem tem e quem nao tem leitura (exceto uptime)
    let sem_uptime = |v: &Value| {
        let mut c = v.clone();
        c.as_object_mut().unwrap().remove("uptime_s");
        c
    };
    assert_eq!(sem_uptime(&st_a), sem_uptime(&st_b));

    let texto = serde_json::to_string(&st_b).unwrap();
    assert!(
        !texto.contains("blocklog"),
        "status nao mostra escopos: {texto}"
    );
    assert!(
        !texto.contains("secreto"),
        "status nao mostra titulos: {texto}"
    );
}

// ------------------------------------------------------------------ A10

#[test]
fn a10_perfil_spec_only_nao_enxerga_impl_por_metodo_algum() {
    let m = Mundo::novo();
    let perfis = grants(&[
        perfil_total("agente-a", TOK_A),
        perfil_com(
            "agente-spec",
            TOK_SPEC,
            &["propose", "promote"],
            &["spec"],
            &["spec"],
        ),
    ]);
    let mut s = servico(&m, &perfis);
    propoe_e_promove(&mut s, TOK_A, decisao("Modulo interno", "impl")); // D-001 active (seq 1,2)
    propoe_e_promove(&mut s, TOK_A, decisao("Especificacao publica", "spec")); // D-002 active (seq 3,4)
    propoe(&mut s, TOK_A, decisao("Proposta impl", "impl")); // D-003 proposed (seq 5)
    let head = s.head_seq();

    // controle: o spec_only le o que e do escopo spec
    assert_eq!(
        res(&get(&mut s, TOK_SPEC, "D-002"))["title"],
        json!("Especificacao publica")
    );

    // leitura de impl: get, get com rev, query (padrao, por escopo, por termo) e view
    assert_eq!(cod(&get(&mut s, TOK_SPEC, "D-001")), "not_found");
    assert_eq!(cod(&get_com_rev(&mut s, TOK_SPEC, "D-001", 1)), "not_found");
    assert_eq!(
        ids_da_consulta(&consulta(&mut s, TOK_SPEC, json!({}))),
        vec!["D-002".to_string()]
    );
    assert!(ids_da_consulta(&consulta(&mut s, TOK_SPEC, json!({"scope": "impl"}))).is_empty());
    assert!(ids_da_consulta(&consulta(&mut s, TOK_SPEC, json!({"term": "interno"}))).is_empty());
    let visao = linhas_visao(&res(&chama(&mut s, TOK_SPEC, "view", &json!({}))));
    assert!(
        !visao
            .iter()
            .any(|l| l.contains("D-001") || l.contains("D-003")),
        "view: {visao:?}"
    );

    // escrita sobre impl: revisao, promote, reject, retire = not_found; propose novo impl = forbidden
    assert_eq!(
        cod(&muta(&mut s, TOK_SPEC, "propose", revisao_de("D-001", 1))),
        "not_found"
    );
    assert_eq!(
        cod(&muta(
            &mut s,
            TOK_SPEC,
            "promote",
            json!({"id":"D-003","rev":1,"expected_seq":5})
        )),
        "not_found"
    );
    assert_eq!(
        cod(&muta(
            &mut s,
            TOK_SPEC,
            "reject",
            json!({"id":"D-003","rev":1,"reason":"x"})
        )),
        "not_found"
    );
    assert_eq!(
        cod(&muta(
            &mut s,
            TOK_SPEC,
            "retire",
            json!({"id":"D-001","rev":1,"reason":"x"})
        )),
        "not_found"
    );
    assert_eq!(
        cod(&muta(
            &mut s,
            TOK_SPEC,
            "propose",
            decisao("Nova impl", "impl")
        )),
        "forbidden"
    );
    assert_eq!(s.head_seq(), head, "nada gravado");

    // controle positivo: o spec_only escreve em spec
    let ok = propoe(&mut s, TOK_SPEC, decisao("Nova spec", "spec"));
    assert_eq!(ok["id"], json!("D-004"));
}

fn revisao_de(id: &str, expected_rev: u64) -> Value {
    let mut p = decisao("Revisao", "impl");
    p["id"] = json!(id);
    p["expected_rev"] = json!(expected_rev);
    p
}

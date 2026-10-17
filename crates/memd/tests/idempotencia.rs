//! I1..I5: idempotencia por `request_id` (memd-spec-v0.md, secao 6; blocklog secao 6).

mod common;

use common::*;
use serde_json::{Value, json};

/// Propose com `request_id` fixo.
fn com_rid(mut p: Value, rid: &str) -> Value {
    p["request_id"] = json!(rid);
    p
}

fn sem_replayed(v: &Value) -> Value {
    let mut c = v.clone();
    c.as_object_mut().unwrap().remove("replayed");
    c
}

fn perfil_a() -> String {
    grants(&[perfil_total("agente-a", TOK_A)])
}

// ------------------------------------------------------------------ I1

#[test]
fn i01_mesmo_request_id_e_carga_devolve_original_com_replayed_sem_novo_commit() {
    let m = Mundo::novo();
    let mut s = servico(&m, &perfil_a());
    let p = com_rid(decisao("Usar memd", "agentd"), "r-1");

    let r1 = res(&chama(&mut s, TOK_A, "propose", &p));
    assert_eq!(r1["id"], json!("D-001"));
    let r2 = res(&chama(&mut s, TOK_A, "propose", &p));

    assert_eq!(r2["replayed"], json!(true), "repeticao marca replayed:true");
    assert_eq!(
        sem_replayed(&r2),
        sem_replayed(&r1),
        "mesmos campos, mesmo seq"
    );
    assert_eq!(r2["seq"], json!(1));
    assert_eq!(s.head_seq(), 1, "repeticao nao grava");

    let q = consulta(&mut s, TOK_A, json!({"status": "proposed"}));
    assert_eq!(
        ids_da_consulta(&q),
        vec!["D-001".to_string()],
        "nao criou D-002"
    );
}

#[test]
fn i01b_carga_canonica_ignora_ordem_de_chaves_e_espacos() {
    let m = Mundo::novo();
    let mut s = servico(&m, &perfil_a());
    let r1 = res(&chama(
        &mut s,
        TOK_A,
        "propose",
        &com_rid(decisao("Usar memd", "agentd"), "r-2"),
    ));
    assert_eq!(r1["seq"], json!(1));

    // mesmos params, chaves em outra ordem e com espacos: o hash e do JSON canonico
    let linha = format!(
        r#"{{ "v": 1, "id": "c", "token": "{TOK_A}", "method": "propose", "params": {{
            "request_id": "r-2",
            "source": {{"at": "2026-10-16", "ref": "docs/memd-spec-v0.md", "kind": "doc"}},
            "fields": {{"criterion": "criterio de teste"}},
            "tags": ["teste"],
            "body": "corpo de teste",
            "title": "Usar memd",
            "scope": "agentd",
            "type": "decision"
        }} }}"#
    );
    let v = parse(&s.handle_line(&linha));
    let r2 = res(&v);
    assert_eq!(
        r2["replayed"],
        json!(true),
        "carga igual com outra forma textual = replayed"
    );
    assert_eq!(r2["seq"], json!(1));
    assert_eq!(s.head_seq(), 1);
}

// ------------------------------------------------------------------ I2

#[test]
fn i02_mesmo_request_id_com_carga_diferente_e_request_conflict() {
    let m = Mundo::novo();
    let mut s = servico(&m, &perfil_a());
    res(&chama(
        &mut s,
        TOK_A,
        "propose",
        &com_rid(decisao("Usar memd", "agentd"), "r-1"),
    ));

    let v = chama(
        &mut s,
        TOK_A,
        "propose",
        &com_rid(decisao("Outro titulo", "agentd"), "r-1"),
    );
    assert_eq!(cod(&v), "request_conflict");
    assert_eq!(s.head_seq(), 1, "conflito nao grava");
    assert_eq!(
        res(&get(&mut s, TOK_A, "D-001"))["title"],
        json!("Usar memd")
    );

    // controle: a mesma carga original continua sendo repeticao
    let r = res(&chama(
        &mut s,
        TOK_A,
        "propose",
        &com_rid(decisao("Usar memd", "agentd"), "r-1"),
    ));
    assert_eq!(r["replayed"], json!(true));
}

#[test]
fn i02b_mesmo_request_id_de_outro_agente_e_pedido_independente() {
    let m = Mundo::novo();
    let perfis = grants(&[
        perfil_total("agente-a", TOK_A),
        perfil_total("agente-b", TOK_B),
    ]);
    let mut s = servico(&m, &perfis);
    res(&chama(
        &mut s,
        TOK_A,
        "propose",
        &com_rid(decisao("Do agente A", "agentd"), "r-1"),
    ));

    // B usa o mesmo request_id com outra carga: chave e (agente, id), nao so o id
    let r = res(&chama(
        &mut s,
        TOK_B,
        "propose",
        &com_rid(decisao("Do agente B", "agentd"), "r-1"),
    ));
    assert_eq!(
        r,
        json!({"id": "D-002", "rev": 1, "status": "proposed", "seq": 2})
    );
}

// ------------------------------------------------------------------ I3

#[test]
fn i03_repeticao_depois_de_reiniciar_devolve_o_original() {
    let m = Mundo::novo();
    let p = com_rid(decisao("Usar memd", "agentd"), "r-3");
    {
        let mut s = servico(&m, &perfil_a());
        let r1 = res(&chama(&mut s, TOK_A, "propose", &p));
        assert_eq!(r1["seq"], json!(1));
    } // daemon "reiniciado": o Service e o log sao descartados

    let mut s = servico(&m, &perfil_a());
    let r2 = res(&chama(&mut s, TOK_A, "propose", &p));
    assert_eq!(r2["replayed"], json!(true));
    assert_eq!(r2["id"], json!("D-001"));
    assert_eq!(r2["seq"], json!(1));
    assert_eq!(s.head_seq(), 1);

    // controle: o pedido novo depois do reinicio grava normalmente
    let d = propoe(&mut s, TOK_A, decisao("Depois do reinicio", "agentd"));
    assert_eq!(d["seq"], json!(2));
}

// ------------------------------------------------------------------ I4

#[test]
fn i04_repeticao_apos_perder_leitura_do_registro_e_not_found() {
    let m = Mundo::novo();
    let mut s = servico(&m, &perfil_a());
    let p = com_rid(decisao("Usar memd", "agentd"), "r-4");
    res(&chama(&mut s, TOK_A, "propose", &p));

    // controle: antes de perder a leitura, a repeticao devolve o original
    let antes = res(&chama(&mut s, TOK_A, "propose", &p));
    assert_eq!(antes["replayed"], json!(true));

    // o dono remove a leitura de "agentd" e recarrega (SIGHUP equivalente)
    s.reload_grants(grants_objeto(&[perfil(
        "agente-a",
        TOK_A,
        &["propose", "promote"],
        &["outra"],
        &["*"],
        &["decision", "pending"],
        "2099-12-31",
        "teste",
    )]));
    let depois = chama(&mut s, TOK_A, "propose", &p);
    assert_eq!(
        cod(&depois),
        "not_found",
        "nao devolve o resultado guardado"
    );
    assert_eq!(s.head_seq(), 1, "nada novo gravado");
}

fn grants_objeto(perfis: &[Value]) -> memd::Grants {
    memd::Grants::from_json(&grants(perfis)).expect("grants de teste")
}

// ------------------------------------------------------------------ I5

#[test]
fn i05_mutacao_sem_request_id_e_invalid_params() {
    let m = Mundo::novo();
    let mut s = servico(&m, &perfil_a());

    let sem = chama(&mut s, TOK_A, "propose", &decisao("Sem id", "agentd"));
    assert_eq!(cod(&sem), "invalid_params");
    assert_eq!(s.head_seq(), 0);

    // os quatro metodos de mutacao exigem request_id
    res(&chama(
        &mut s,
        TOK_A,
        "propose",
        &com_rid(decisao("Base", "agentd"), "r-base"),
    ));
    for (metodo, params) in [
        (
            "promote",
            json!({"id": "D-001", "rev": 1, "expected_seq": 1}),
        ),
        ("reject", json!({"id": "D-001", "rev": 1, "reason": "x"})),
        ("retire", json!({"id": "D-001", "rev": 1, "reason": "x"})),
    ] {
        assert_eq!(
            cod(&chama(&mut s, TOK_A, metodo, &params)),
            "invalid_params",
            "{metodo}"
        );
    }
    assert_eq!(s.head_seq(), 1);

    // request_id fora de [A-Za-z0-9._-]{1,64}: espaco e 65 caracteres
    for ruim in ["r 1".to_string(), "x".repeat(65)] {
        let v = chama(
            &mut s,
            TOK_A,
            "propose",
            &com_rid(decisao("Ruim", "agentd"), &ruim),
        );
        assert_eq!(cod(&v), "invalid_params", "request_id {ruim:?}");
    }
    assert_eq!(s.head_seq(), 1);

    // controle positivo: request_id valido nos limites (64 caracteres) grava
    let ok = chama(
        &mut s,
        TOK_A,
        "propose",
        &com_rid(decisao("Limite", "agentd"), &"y".repeat(64)),
    );
    assert_eq!(res(&ok)["id"], json!("D-002"));
}

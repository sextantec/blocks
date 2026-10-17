//! M1..M11: mutacoes (memd-spec-v0.md, secoes 4 e 10). Caixa-preta por `memd::Service`
//! sobre `blocklog` com disco e relogio falsos (tests/common).

mod common;

use common::*;
use serde_json::json;
use std::sync::{Arc, Barrier, Mutex};
use std::thread;

/// Parametros de revisao: `propose` com `id` e `expected_rev`.
fn revisao(mut p: serde_json::Value, id: &str, expected_rev: u64) -> serde_json::Value {
    p["id"] = json!(id);
    p["expected_rev"] = json!(expected_rev);
    p
}

fn perfil_a() -> String {
    grants(&[perfil_total("agente-a", TOK_A)])
}

// ------------------------------------------------------------------ M1

#[test]
fn m01_propose_novo_gera_d001_rev1_proposed_seq1() {
    let m = Mundo::novo();
    let mut s = servico(&m, &perfil_a());

    let r = muta(&mut s, TOK_A, "propose", decisao("Usar memd", "agentd"));
    assert_eq!(
        res(&r),
        json!({"id": "D-001", "rev": 1, "status": "proposed", "seq": 1})
    );
    assert_eq!(s.head_seq(), 1);

    let g = res(&get(&mut s, TOK_A, "D-001"));
    assert_eq!(g["status"], json!("proposed"));
    assert_eq!(g["rev"], json!(1));
    assert_eq!(
        g["author"],
        json!("agente-a"),
        "author vem do perfil, nao do cliente"
    );
}

#[test]
fn m01b_ids_por_tipo_sem_lacuna_nem_repeticao() {
    let m = Mundo::novo();
    let mut s = servico(&m, &perfil_a());

    let d1 = propoe(&mut s, TOK_A, decisao("Primeira", "agentd"));
    let d2 = propoe(&mut s, TOK_A, decisao("Segunda", "agentd"));
    let p1 = propoe(
        &mut s,
        TOK_A,
        pendencia("Pendencia", "agentd", "2026-11-01"),
    );
    let d3 = propoe(&mut s, TOK_A, decisao("Terceira", "agentd"));

    assert_eq!(d1["id"], json!("D-001"));
    assert_eq!(d2["id"], json!("D-002"));
    assert_eq!(p1["id"], json!("P-001"), "contador de pendencia e separado");
    assert_eq!(d3["id"], json!("D-003"));
    assert_eq!(
        [
            d1["seq"].clone(),
            d2["seq"].clone(),
            p1["seq"].clone(),
            d3["seq"].clone()
        ],
        [json!(1), json!(2), json!(3), json!(4)]
    );
}

#[test]
fn m01c_32_propose_em_threads_geram_d001_a_d032_sem_repeticao() {
    let m = Mundo::novo();
    let cfg = cfg_com(&[("max_open_proposals", json!(100))]);
    let s = Arc::new(Mutex::new(servico_cfg(&m, cfg, &perfil_a())));
    let barreira = Arc::new(Barrier::new(32));

    let handles: Vec<_> = (0..32)
        .map(|i| {
            let s = Arc::clone(&s);
            let barreira = Arc::clone(&barreira);
            thread::spawn(move || {
                barreira.wait();
                let mut g = s.lock().unwrap();
                let r = muta(
                    &mut g,
                    TOK_A,
                    "propose",
                    decisao(&format!("Titulo {i:02}"), "agentd"),
                );
                res(&r)
            })
        })
        .collect();

    let mut ids: Vec<String> = handles
        .into_iter()
        .map(|h| h.join().unwrap()["id"].as_str().unwrap().to_string())
        .collect();
    ids.sort();
    let esperado: Vec<String> = (1..=32).map(|n| format!("D-{n:03}")).collect();
    assert_eq!(
        ids, esperado,
        "32 propose simultaneos = 32 ids distintos, sem lacuna"
    );

    let g = s.lock().unwrap();
    assert_eq!(g.head_seq(), 32, "um commit por propose");
}

// ------------------------------------------------------------------ M2

#[test]
fn m02_revisao_com_expected_rev_errado_e_stale_rev() {
    let m = Mundo::novo();
    let mut s = servico(&m, &perfil_a());
    let d = propoe(&mut s, TOK_A, decisao("Usar memd", "agentd"));
    promove(&mut s, TOK_A, "D-001", 1, d["seq"].as_u64().unwrap());

    // controle positivo: expected_rev = rev active (1) -> rev 2
    let r2 = propoe(
        &mut s,
        TOK_A,
        revisao(decisao("Usar memd v2", "agentd"), "D-001", 1),
    );
    assert_eq!(
        r2,
        json!({"id": "D-001", "rev": 2, "status": "proposed", "seq": 3})
    );
    let head = s.head_seq();

    // negativos: expected_rev errado (0 e 2) -> stale_rev, sem commit
    for errado in [0u64, 2] {
        let v = muta(
            &mut s,
            TOK_A,
            "propose",
            revisao(decisao("Usar memd v3", "agentd"), "D-001", errado),
        );
        assert_eq!(cod(&v), "stale_rev", "expected_rev={errado}");
    }
    assert_eq!(s.head_seq(), head, "stale_rev nao grava nada");
    let g = res(&get(&mut s, TOK_A, "D-001"));
    assert_eq!(
        g["rev"],
        json!(1),
        "sem rev, a active (rev 1) continua sendo a leitura padrao"
    );
    assert_eq!(g["status"], json!("active"));
}

#[test]
fn m02b_revisao_de_id_inexistente_e_not_found() {
    let m = Mundo::novo();
    let mut s = servico(&m, &perfil_a());
    let v = muta(
        &mut s,
        TOK_A,
        "propose",
        revisao(decisao("X", "agentd"), "D-404", 0),
    );
    assert_eq!(cod(&v), "not_found");
    assert_eq!(s.head_seq(), 0);
}

// ------------------------------------------------------------------ M3

#[test]
fn m03_promote_com_expected_seq_errado_e_stale_seq() {
    let m = Mundo::novo();
    let mut s = servico(&m, &perfil_a());
    let d = propoe(&mut s, TOK_A, decisao("Usar memd", "agentd"));
    assert_eq!(d["seq"], json!(1));

    let v = muta(
        &mut s,
        TOK_A,
        "promote",
        json!({"id": "D-001", "rev": 1, "expected_seq": 99}),
    );
    assert_eq!(cod(&v), "stale_seq");
    assert_eq!(s.head_seq(), 1);
    assert_eq!(
        res(&get(&mut s, TOK_A, "D-001"))["status"],
        json!("proposed")
    );

    // controle positivo: expected_seq = proposed_seq (1)
    let p = promove(&mut s, TOK_A, "D-001", 1, 1);
    assert_eq!(p["status"], json!("active"));
    assert_eq!(
        p["superseded"],
        json!([]),
        "primeira promocao nao substitui nada"
    );
    assert_eq!(p["seq"], json!(2));
}

// ------------------------------------------------------------------ M4

/// D-001 rev1 active (seq 1 e 2); D-001 rev2 proposta (seq 3). Devolve a sessao com a rev2 pendente.
fn cenario_revisao_proposta(m: &Mundo) {
    let mut s = servico(m, &perfil_a());
    propoe_e_promove(&mut s, TOK_A, decisao("Usar memd", "agentd"));
    propoe(
        &mut s,
        TOK_A,
        revisao(decisao("Usar memd v2", "agentd"), "D-001", 1),
    );
    assert_eq!(s.head_seq(), 3);
}

#[test]
fn m04_promote_de_revisao_supersede_a_anterior_no_mesmo_commit() {
    let m = Mundo::novo();
    cenario_revisao_proposta(&m);
    let mut s = servico(&m, &perfil_a());

    let p = promove(&mut s, TOK_A, "D-001", 2, 3);
    assert_eq!(p["status"], json!("active"));
    assert_eq!(p["seq"], json!(4), "promocao e exatamente um commit");
    assert_eq!(p["superseded"].as_array().map(|a| a.len()), Some(1));
    assert_eq!(s.head_seq(), 4);

    let r1 = get_rev(&mut s, TOK_A, "D-001", 1);
    assert_eq!(r1["status"], json!("superseded"));
    assert_eq!(r1["superseded_by"], json!("D-001@2"));
    assert_eq!(
        get_rev(&mut s, TOK_A, "D-001", 2)["status"],
        json!("active")
    );
}

#[test]
fn m04b_cauda_cortada_no_meio_do_promote_de_revisao_nao_aplica_nada() {
    let m = Mundo::novo();
    cenario_revisao_proposta(&m);
    {
        let mut s = servico(&m, &perfil_a());
        promove(&mut s, TOK_A, "D-001", 2, 3);
    }
    let cortado = corta_ultima_no_meio(&m.disco.bytes());

    let m2 = Mundo::com_bytes(&cortado, &m.relogio);
    let mut s = servico(&m2, &perfil_a());
    assert_eq!(s.head_seq(), 3, "a linha cortada e descartada inteira");
    assert_eq!(
        get_rev(&mut s, TOK_A, "D-001", 1)["status"],
        json!("active"),
        "rev1 segue active"
    );
    assert_eq!(
        get_rev(&mut s, TOK_A, "D-001", 2)["status"],
        json!("proposed"),
        "rev2 segue proposed"
    );
    assert!(get_rev(&mut s, TOK_A, "D-001", 1)["superseded_by"].is_null());

    // o commit seguinte recebe seq correto (continuidade apos recuperacao)
    let d = propoe(&mut s, TOK_A, decisao("Depois da queda", "agentd"));
    assert_eq!(d["seq"], json!(4));
}

#[test]
fn m04c_sem_o_newline_final_o_promote_de_revisao_tambem_nao_aplica() {
    let m = Mundo::novo();
    cenario_revisao_proposta(&m);
    {
        let mut s = servico(&m, &perfil_a());
        promove(&mut s, TOK_A, "D-001", 2, 3);
    }
    // JSON completo da ultima linha, sem o '\n' (B2: descartada mesmo assim)
    let sem_nl = tira_newline_final(&m.disco.bytes());
    let m2 = Mundo::com_bytes(&sem_nl, &m.relogio);
    let mut s = servico(&m2, &perfil_a());
    assert_eq!(s.head_seq(), 3);
    assert_eq!(
        get_rev(&mut s, TOK_A, "D-001", 1)["status"],
        json!("active")
    );
    assert_eq!(
        get_rev(&mut s, TOK_A, "D-001", 2)["status"],
        json!("proposed")
    );
}

#[test]
fn m04d_log_completo_reaberto_aplica_o_promote_inteiro() {
    let m = Mundo::novo();
    cenario_revisao_proposta(&m);
    {
        let mut s = servico(&m, &perfil_a());
        promove(&mut s, TOK_A, "D-001", 2, 3);
    }
    let m2 = Mundo::com_bytes(&m.disco.bytes(), &m.relogio);
    let mut s = servico(&m2, &perfil_a());
    assert_eq!(s.head_seq(), 4);
    assert_eq!(
        get_rev(&mut s, TOK_A, "D-001", 1)["status"],
        json!("superseded")
    );
    assert_eq!(
        get_rev(&mut s, TOK_A, "D-001", 1)["superseded_by"],
        json!("D-001@2")
    );
    assert_eq!(
        get_rev(&mut s, TOK_A, "D-001", 2)["status"],
        json!("active")
    );
}

// ------------------------------------------------------------------ M5

/// D-001 active (seq 1, 2); D-002 proposta com supersedes ["D-001"] (seq 3).
fn cenario_supersedes_proposto(m: &Mundo) {
    let mut s = servico(m, &perfil_a());
    propoe_e_promove(&mut s, TOK_A, decisao("Modelo antigo", "agentd"));
    let mut p = decisao("Modelo novo", "agentd");
    p["supersedes"] = json!(["D-001"]);
    propoe(&mut s, TOK_A, p);
    assert_eq!(s.head_seq(), 3);
}

#[test]
fn m05_supersedes_marca_a_active_so_na_promocao_do_novo() {
    let m = Mundo::novo();
    cenario_supersedes_proposto(&m);
    let mut s = servico(&m, &perfil_a());
    assert_eq!(
        res(&get(&mut s, TOK_A, "D-001"))["status"],
        json!("active"),
        "propor com supersedes nao muda a active"
    );

    let p = promove(&mut s, TOK_A, "D-002", 1, 3);
    assert_eq!(p["status"], json!("active"));
    assert_eq!(p["seq"], json!(4));
    assert_eq!(s.head_seq(), 4);

    let antiga = res(&get(&mut s, TOK_A, "D-001"));
    assert_eq!(antiga["status"], json!("superseded"));
    assert_eq!(antiga["superseded_by"], json!("D-002@1"));
    assert_eq!(res(&get(&mut s, TOK_A, "D-002"))["status"], json!("active"));
}

#[test]
fn m05b_cauda_cortada_no_promote_com_supersedes_nao_aplica_nenhuma_ponta() {
    let m = Mundo::novo();
    cenario_supersedes_proposto(&m);
    {
        let mut s = servico(&m, &perfil_a());
        promove(&mut s, TOK_A, "D-002", 1, 3);
    }
    let cortado = corta_ultima_no_meio(&m.disco.bytes());
    let m2 = Mundo::com_bytes(&cortado, &m.relogio);
    let mut s = servico(&m2, &perfil_a());
    assert_eq!(s.head_seq(), 3);
    assert_eq!(
        res(&get(&mut s, TOK_A, "D-001"))["status"],
        json!("active"),
        "ponta antiga intacta"
    );
    assert_eq!(
        res(&get(&mut s, TOK_A, "D-002"))["status"],
        json!("proposed"),
        "ponta nova intacta"
    );
}

#[test]
fn m05c_log_completo_com_supersedes_aplica_as_duas_pontas() {
    let m = Mundo::novo();
    cenario_supersedes_proposto(&m);
    {
        let mut s = servico(&m, &perfil_a());
        promove(&mut s, TOK_A, "D-002", 1, 3);
    }
    let m2 = Mundo::com_bytes(&m.disco.bytes(), &m.relogio);
    let mut s = servico(&m2, &perfil_a());
    assert_eq!(s.head_seq(), 4);
    assert_eq!(
        res(&get(&mut s, TOK_A, "D-001"))["status"],
        json!("superseded")
    );
    assert_eq!(
        res(&get(&mut s, TOK_A, "D-001"))["superseded_by"],
        json!("D-002@1")
    );
    assert_eq!(res(&get(&mut s, TOK_A, "D-002"))["status"], json!("active"));
}

// ------------------------------------------------------------------ M6

#[test]
fn m06_supersedes_de_id_inexistente_e_not_found_e_nada_muda() {
    let m = Mundo::novo();
    let mut s = servico(&m, &perfil_a());
    propoe_e_promove(&mut s, TOK_A, decisao("Modelo atual", "agentd")); // D-001 active, seq 1..2

    let mut p = decisao("Substituto fantasma", "agentd");
    p["supersedes"] = json!(["D-999"]);
    let prop = propoe(&mut s, TOK_A, p); // a spec nao lista not_found para propose
    assert_eq!(prop["seq"], json!(3));
    let head = s.head_seq();

    let v = muta(
        &mut s,
        TOK_A,
        "promote",
        json!({"id": "D-002", "rev": 1, "expected_seq": 3}),
    );
    assert_eq!(cod(&v), "not_found");
    assert_eq!(s.head_seq(), head, "not_found nao grava commit");
    assert_eq!(
        res(&get(&mut s, TOK_A, "D-002"))["status"],
        json!("proposed")
    );
    assert_eq!(res(&get(&mut s, TOK_A, "D-001"))["status"], json!("active"));

    // controle positivo no mesmo cenario: supersedes de id existente promove
    let mut p2 = decisao("Substituto de verdade", "agentd");
    p2["supersedes"] = json!(["D-001"]);
    let prop2 = propoe(&mut s, TOK_A, p2);
    let r = promove(&mut s, TOK_A, "D-003", 1, prop2["seq"].as_u64().unwrap());
    assert_eq!(r["status"], json!("active"));
    assert_eq!(
        res(&get(&mut s, TOK_A, "D-001"))["status"],
        json!("superseded")
    );
}

#[test]
fn m06b_supersedes_ilegivel_e_not_found_e_nada_muda_mas_legivel_promove() {
    // A le tudo; B so le e escreve em "agentd". D-001 esta no escopo "blocklog" (ilegivel para B).
    let m = Mundo::novo();
    let perfis = grants(&[
        perfil_total("agente-a", TOK_A),
        perfil(
            "agente-b",
            TOK_B,
            &["propose", "promote"],
            &["agentd"],
            &["agentd"],
            &["decision", "pending"],
            "2099-12-31",
            "teste",
        ),
    ]);
    let mut s = servico(&m, &perfis);
    propoe_e_promove(&mut s, TOK_A, decisao("Escopo protegido", "blocklog")); // D-001 active

    let mut p = decisao("Substituto de B", "agentd");
    p["supersedes"] = json!(["D-001"]);
    propoe(&mut s, TOK_B, p); // D-002 proposto por B
    let head = s.head_seq();

    let v = muta(
        &mut s,
        TOK_B,
        "promote",
        json!({"id": "D-002", "rev": 1, "expected_seq": 3}),
    );
    assert_eq!(cod(&v), "not_found", "ponta ilegivel = not_found");
    assert_eq!(s.head_seq(), head);
    assert_eq!(res(&get(&mut s, TOK_A, "D-001"))["status"], json!("active"));
    assert_eq!(
        res(&get(&mut s, TOK_B, "D-002"))["status"],
        json!("proposed")
    );

    // controle positivo: A, que le D-001, promove D-002
    let r = promove(&mut s, TOK_A, "D-002", 1, 3);
    assert_eq!(r["status"], json!("active"));
    assert_eq!(
        res(&get(&mut s, TOK_A, "D-001"))["status"],
        json!("superseded")
    );
}

// ------------------------------------------------------------------ M7

#[test]
fn m07_reject_e_retire_mudam_so_o_estado_esperado() {
    let m = Mundo::novo();
    let mut s = servico(&m, &perfil_a());
    propoe(&mut s, TOK_A, decisao("Um", "agentd")); // 1
    propoe(&mut s, TOK_A, decisao("Dois", "agentd")); // 2
    propoe(&mut s, TOK_A, decisao("Tres", "agentd")); // 3

    let rej = res(&muta(
        &mut s,
        TOK_A,
        "reject",
        json!({"id": "D-001", "rev": 1, "reason": "fora de escopo"}),
    ));
    assert_eq!(
        rej,
        json!({"id": "D-001", "rev": 1, "status": "rejected", "seq": 4})
    );
    assert_eq!(
        res(&get(&mut s, TOK_A, "D-002"))["status"],
        json!("proposed"),
        "vizinho intacto"
    );

    let p = promove(&mut s, TOK_A, "D-002", 1, 2);
    assert_eq!(p["seq"], json!(5));
    let ret = res(&muta(
        &mut s,
        TOK_A,
        "retire",
        json!({"id": "D-002", "rev": 1, "reason": "substituida depois"}),
    ));
    assert_eq!(
        ret,
        json!({"id": "D-002", "rev": 1, "status": "retired", "seq": 6})
    );
    assert_eq!(
        res(&get(&mut s, TOK_A, "D-003"))["status"],
        json!("proposed")
    );
    assert_eq!(
        res(&get(&mut s, TOK_A, "D-001"))["status"],
        json!("rejected")
    );

    // estado errado = conflict (a spec nao lista codigo para isto; ver MEMD-TESTES.md).
    // D-002 esta retired, D-003 esta proposed: nenhuma das duas transicoes e valida.
    let head = s.head_seq();
    assert_eq!(
        cod(&muta(
            &mut s,
            TOK_A,
            "reject",
            json!({"id": "D-002", "rev": 1, "reason": "x"})
        )),
        "conflict"
    );
    assert_eq!(
        cod(&muta(
            &mut s,
            TOK_A,
            "retire",
            json!({"id": "D-003", "rev": 1, "reason": "x"})
        )),
        "conflict"
    );
    assert_eq!(s.head_seq(), head, "transicao recusada nao grava");
    assert_eq!(
        res(&get(&mut s, TOK_A, "D-003"))["status"],
        json!("proposed")
    );
    assert_eq!(
        res(&get(&mut s, TOK_A, "D-002"))["status"],
        json!("retired")
    );

    // id inexistente = not_found
    assert_eq!(
        cod(&muta(
            &mut s,
            TOK_A,
            "reject",
            json!({"id": "D-404", "rev": 1, "reason": "x"})
        )),
        "not_found"
    );
}

// ------------------------------------------------------------------ M8 e M8b

#[test]
fn m08_expire_no_limite_de_proposal_ttl_days() {
    let m = Mundo::novo();
    let mut s = servico(&m, &perfil_a()); // Config::default(): proposal_ttl_days = 14
    propoe(&mut s, TOK_A, decisao("Vai expirar", "agentd")); // ts T0 = 2026-10-16T22:00:00Z

    // um instante antes do limite (13 dias 23h 59min 59,999s): ainda proposed
    m.relogio.poe("2026-10-30T21:59:59.999Z");
    s.tick();
    assert_eq!(
        res(&get(&mut s, TOK_A, "D-001"))["status"],
        json!("proposed"),
        "idade menor que ttl"
    );

    // no limite exato (14 dias): expired
    m.relogio.poe("2026-10-30T22:00:00.000Z");
    let head = s.head_seq();
    s.tick();
    assert_eq!(
        res(&get(&mut s, TOK_A, "D-001"))["status"],
        json!("expired")
    );
    assert_eq!(s.head_seq(), head + 1, "expire grava um commit");
}

#[test]
fn m08b_dezessete_expiracoes_em_dois_commits_de_ate_16() {
    let m = Mundo::novo();
    let mut s = servico(&m, &perfil_a());
    for i in 1..=17 {
        propoe(
            &mut s,
            TOK_A,
            decisao(&format!("Proposta {i:02}"), "agentd"),
        );
    }
    assert_eq!(s.head_seq(), 17);

    m.relogio.poe("2026-10-31T22:00:00.000Z"); // 15 dias depois
    s.tick();
    assert_eq!(
        s.head_seq(),
        19,
        "17 expiracoes com limite de 16 por tx = 2 commits"
    );
    for n in 1..=17 {
        assert_eq!(
            res(&get(&mut s, TOK_A, &format!("D-{n:03}")))["status"],
            json!("expired")
        );
    }
}

// ------------------------------------------------------------------ M9

#[test]
fn m09_relogio_regressivo_nao_expira_antes_nem_ressuscita() {
    let m = Mundo::novo();
    let mut s = servico(&m, &perfil_a());
    propoe(&mut s, TOK_A, decisao("Primeira", "agentd")); // D-001, ts T0

    // controle: 15 dias depois, expira
    m.relogio.poe("2026-10-31T22:00:00.000Z");
    s.tick();
    assert_eq!(
        res(&get(&mut s, TOK_A, "D-001"))["status"],
        json!("expired")
    );

    // relogio volta para antes de tudo: nao ressuscita
    m.relogio.poe("2026-10-16T12:00:00.000Z");
    s.tick();
    assert_eq!(
        res(&get(&mut s, TOK_A, "D-001"))["status"],
        json!("expired"),
        "nao ressuscita"
    );

    // proposta feita com relogio ja regredido: ts do commit nao regride (max com o ultimo)
    let d2 = propoe(&mut s, TOK_A, decisao("Segunda", "agentd"));
    assert_eq!(d2["id"], json!("D-002"));
    m.relogio.poe("2026-10-17T00:00:00.000Z"); // regride de novo, para antes da proposta
    s.tick();
    assert_eq!(
        res(&get(&mut s, TOK_A, "D-002"))["status"],
        json!("proposed"),
        "relogio regressivo nao expira a proposta cedo"
    );

    // e expira no tempo normal (14 dias depois do ts da proposta, 2026-10-31T22:00Z)
    m.relogio.poe("2026-11-14T22:00:00.000Z");
    s.tick();
    assert_eq!(
        res(&get(&mut s, TOK_A, "D-002"))["status"],
        json!("expired")
    );
}

// ------------------------------------------------------------------ M10

#[test]
fn m10_duas_active_com_mesmo_tipo_escopo_e_titulo_sao_conflict() {
    let m = Mundo::novo();
    let mut s = servico(&m, &perfil_a());
    propoe_e_promove(&mut s, TOK_A, decisao("Titulo X", "agentd")); // D-001 active

    // mesmo tipo, escopo e titulo (apos aparar espacos e dobrar A-Z), sem supersedes -> conflict
    let p = propoe(&mut s, TOK_A, decisao("  titulo x ", "agentd"));
    let v = muta(
        &mut s,
        TOK_A,
        "promote",
        json!({"id": "D-002", "rev": 1, "expected_seq": p["seq"]}),
    );
    assert_eq!(cod(&v), "conflict");
    assert_eq!(
        res(&get(&mut s, TOK_A, "D-002"))["status"],
        json!("proposed")
    );

    // controles positivos no mesmo cenario:
    // (a) titulo diferente promove
    let c = propoe(&mut s, TOK_A, decisao("Titulo Y", "agentd"));
    assert_eq!(
        promove(&mut s, TOK_A, "D-003", 1, c["seq"].as_u64().unwrap())["status"],
        json!("active")
    );
    // (b) mesmo titulo em outro escopo promove
    let e = propoe(&mut s, TOK_A, decisao("Titulo X", "blocklog"));
    assert_eq!(
        promove(&mut s, TOK_A, "D-004", 1, e["seq"].as_u64().unwrap())["status"],
        json!("active")
    );
    // (c) mesmo titulo e escopo, mas listada em supersedes promove e substitui a D-001
    let mut sub = decisao("titulo x", "agentd");
    sub["supersedes"] = json!(["D-001"]);
    let f = propoe(&mut s, TOK_A, sub);
    assert_eq!(
        promove(&mut s, TOK_A, "D-005", 1, f["seq"].as_u64().unwrap())["status"],
        json!("active")
    );
    assert_eq!(
        res(&get(&mut s, TOK_A, "D-001"))["status"],
        json!("superseded")
    );
}

// ------------------------------------------------------------------ M11

#[test]
fn m11_max_open_proposals_padrao_20_estoura_com_quota() {
    let m = Mundo::novo();
    let perfis = grants(&[
        perfil_total("agente-a", TOK_A),
        perfil_total("agente-b", TOK_B),
    ]);
    let mut s = servico(&m, &perfis); // padrao: 20 propostas abertas por agente

    for i in 1..=20 {
        propoe(&mut s, TOK_A, decisao(&format!("Aberta {i:02}"), "agentd"));
    }
    let v = muta(
        &mut s,
        TOK_A,
        "propose",
        decisao("Vigesima primeira", "agentd"),
    );
    assert_eq!(cod(&v), "quota");
    assert_eq!(s.head_seq(), 20, "quota nao grava");

    // a cota e por agente: B ainda propoe
    let b = propoe(&mut s, TOK_B, decisao("Do agente B", "agentd"));
    assert_eq!(b["id"], json!("D-021"));

    // "abertas" = propostas ainda nao decididas: rejeitar uma libera vaga (controle)
    res(&muta(
        &mut s,
        TOK_A,
        "reject",
        json!({"id": "D-001", "rev": 1, "reason": "liberar vaga"}),
    ));
    let nova = propoe(&mut s, TOK_A, decisao("Depois de liberar", "agentd"));
    assert_eq!(nova["id"], json!("D-022"));
}

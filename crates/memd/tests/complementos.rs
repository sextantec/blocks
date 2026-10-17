//! Complementos da rodada de mutacao: glob `a/**`, token com prefixo errado, `max_records`,
//! `supersedes` de alvo legivel mas nao gravavel, e `log_max_bytes`. Caixa-preta por `Service`.

mod common;

use blocklog::Options;
use common::*;
use serde_json::json;

fn perfil_a() -> String {
    grants(&[perfil_total("agente-a", TOK_A)])
}

/// Token com prefixo diferente de `mt_` e o mesmo corpo de TOK_A (mesmo tamanho, mesmos caracteres).
const TOK_X: &str = "xx_Ol5Pk8Ji2Hu6Gy3Ft1Dr9Es4Wa7Qz0Xc-Vb_Nm3Lk6J";

// ------------------------------------------------------------------ glob a/**

#[test]
fn a11_glob_a_barra_casa_o_proprio_a_e_os_de_baixo() {
    let m = Mundo::novo();
    let perfis = grants(&[
        perfil_total("agente-a", TOK_A),
        perfil(
            "agente-l",
            TOK_B,
            &[],
            &["a/**"],
            &[],
            &["decision"],
            "2099-12-31",
            "leitor",
        ),
    ]);
    let mut s = servico(&m, &perfis);
    propoe_e_promove(&mut s, TOK_A, decisao("Escopo a", "a")); // D-001
    propoe_e_promove(&mut s, TOK_A, decisao("Escopo b", "b")); // D-002
    propoe_e_promove(&mut s, TOK_A, decisao("Escopo a x", "a/x")); // D-003

    // a/** casa o proprio "a" e tudo abaixo
    assert_eq!(res(&get(&mut s, TOK_B, "D-001"))["scope"], json!("a"));
    // controle positivo: filho de a le; b nao
    assert_eq!(res(&get(&mut s, TOK_B, "D-003"))["scope"], json!("a/x"));
    assert_eq!(cod(&get(&mut s, TOK_B, "D-002")), "not_found");
}

#[test]
fn a12_glob_a_barra_nao_casa_prefixo_sem_barra() {
    let m = Mundo::novo();
    let perfis = grants(&[
        perfil_total("agente-a", TOK_A),
        perfil(
            "agente-l",
            TOK_B,
            &[],
            &["a/**"],
            &[],
            &["decision"],
            "2099-12-31",
            "leitor",
        ),
    ]);
    let mut s = servico(&m, &perfis);
    propoe_e_promove(&mut s, TOK_A, decisao("Escopo ab", "ab")); // D-001
    propoe_e_promove(&mut s, TOK_A, decisao("Escopo a-b", "a-b")); // D-002
    propoe_e_promove(&mut s, TOK_A, decisao("Escopo a x", "a/x")); // D-003

    assert_eq!(
        cod(&get(&mut s, TOK_B, "D-001")),
        "not_found",
        "ab nao casa a/**"
    );
    assert_eq!(
        cod(&get(&mut s, TOK_B, "D-002")),
        "not_found",
        "a-b nao casa a/**"
    );
    // controle positivo: a/x le
    assert_eq!(res(&get(&mut s, TOK_B, "D-003"))["scope"], json!("a/x"));
    // query do leitor so mostra o legivel
    let q = consulta(&mut s, TOK_B, json!({}));
    assert_eq!(ids_da_consulta(&q), vec!["D-003".to_string()]);
}

// ------------------------------------------------------------------ token com prefixo errado

#[test]
fn a13_token_com_prefixo_diferente_de_mt_e_unauthorized_mesmo_com_hash_no_grants() {
    assert_eq!(TOK_X.len(), 46, "mesmo tamanho de um token mt_");
    assert_eq!(&TOK_X[3..], &TOK_A[3..], "mesmo corpo");
    // o SHA-256 de TOK_X esta no grants.json: so o formato do token o barra
    let m = Mundo::novo();
    let perfis = grants(&[
        perfil_total("agente-a", TOK_A),
        perfil_total("agente-x", TOK_X),
    ]);
    let mut s = servico(&m, &perfis);

    // controle positivo: o token mt_ correspondente autentica
    assert_eq!(
        res(&chama(&mut s, TOK_A, "whoami", &json!({})))["agent"],
        json!("agente-a")
    );
    // negativos: o mesmo corpo com prefixo xx_ nao autentica (leitura e escrita)
    assert_eq!(
        cod(&chama(&mut s, TOK_X, "whoami", &json!({}))),
        "unauthorized"
    );
    assert_eq!(
        cod(&muta(
            &mut s,
            TOK_X,
            "propose",
            decisao("Intruso", "agentd")
        )),
        "unauthorized"
    );
    assert_eq!(s.head_seq(), 0, "nada gravado com o token invalido");
}

// ------------------------------------------------------------------ max_records

#[test]
fn m12_max_records_fronteira_exata() {
    // max_records = 3: a 3a proposta passa (2 < 3), a 4a = quota
    let m = Mundo::novo();
    let mut s = servico_cfg(&m, cfg_com(&[("max_records", json!(3))]), &perfil_a());
    for i in 1..=3 {
        propoe(&mut s, TOK_A, decisao(&format!("Registro {i}"), "agentd"));
    }
    assert_eq!(
        cod(&muta(&mut s, TOK_A, "propose", decisao("Quarto", "agentd"))),
        "quota"
    );
    assert_eq!(s.head_seq(), 3);

    // max_records = 1: a primeira passa, a segunda = quota (fronteira exata)
    let m1 = Mundo::novo();
    let mut s1 = servico_cfg(&m1, cfg_com(&[("max_records", json!(1))]), &perfil_a());
    assert_eq!(
        propoe(&mut s1, TOK_A, decisao("Unico", "agentd"))["id"],
        json!("D-001")
    );
    assert_eq!(
        cod(&muta(
            &mut s1,
            TOK_A,
            "propose",
            decisao("Segundo", "agentd")
        )),
        "quota"
    );
    assert_eq!(s1.head_seq(), 1);
}

// ------------------------------------------------------------------ supersedes ilegivel para escrita

#[test]
fn a14_promote_com_supersedes_de_alvo_legivel_e_nao_gravavel_e_not_found() {
    // A le e escreve tudo. W le tudo, escreve so em "agentd". D-001 esta em "blocklog" (W le, nao grava).
    let m = Mundo::novo();
    let perfis = grants(&[
        perfil_total("agente-a", TOK_A),
        perfil(
            "agente-w",
            TOK_B,
            &["propose", "promote"],
            &["*"],
            &["agentd"],
            &["decision", "pending"],
            "2099-12-31",
            "w",
        ),
    ]);
    let mut s = servico(&m, &perfis);
    propoe_e_promove(&mut s, TOK_A, decisao("Alvo blocklog", "blocklog")); // D-001 active (seq 1,2)
    let mut sup = decisao("Substituto com alvo", "agentd");
    sup["supersedes"] = json!(["D-001"]);
    let p2 = propoe(&mut s, TOK_A, sup); // D-002 proposed (seq 3)
    let mut fant = decisao("Substituto fantasma", "agentd");
    fant["supersedes"] = json!(["D-999"]);
    let p3 = propoe(&mut s, TOK_A, fant); // D-003 proposed (seq 4)
    let head = s.head_seq();

    // W: alvo legivel mas nao gravavel = not_found
    let com_alvo = chama(
        &mut s,
        TOK_B,
        "promote",
        &json!({"id": "D-002", "rev": 1, "expected_seq": p2["seq"], "request_id": rid()}),
    );
    assert_eq!(cod(&com_alvo), "not_found");
    // A4: igual ao caso de id inexistente (linha inteira, mesmo id de cliente)
    let inexistente = s.handle_line(&linha_req(
        "c",
        TOK_B,
        "promote",
        &json!({"id": "D-003", "rev": 1, "expected_seq": p3["seq"], "request_id": rid()}),
    ));
    let alvo_linha = s.handle_line(&linha_req(
        "c",
        TOK_B,
        "promote",
        &json!({"id": "D-002", "rev": 1, "expected_seq": p2["seq"], "request_id": rid()}),
    ));
    assert_eq!(
        alvo_linha, inexistente,
        "alvo ilegivel para escrita responde igual a inexistente"
    );
    assert_eq!(s.head_seq(), head, "nada gravado");
    assert_eq!(
        res(&get(&mut s, TOK_A, "D-001"))["status"],
        json!("active"),
        "alvo intacto"
    );
    assert_eq!(
        res(&get(&mut s, TOK_A, "D-002"))["status"],
        json!("proposed"),
        "promocao nao aconteceu"
    );

    // controle positivo: A, que escreve no alvo, promove D-002 e supersede D-001
    let r = promove(&mut s, TOK_A, "D-002", 1, p2["seq"].as_u64().unwrap());
    assert_eq!(r["status"], json!("active"));
    assert_eq!(
        res(&get(&mut s, TOK_A, "D-001"))["status"],
        json!("superseded")
    );
}

// ------------------------------------------------------------------ log_max_bytes

#[test]
fn m13_log_max_bytes_estourado_e_quota_sem_broken_e_com_limite_folgado_passa() {
    // minimo da faixa (1 MiB); propostas com corpo de 4096 bytes enchem o log em ~200 commits
    let limite_min = Options {
        log_max_bytes: 1_048_576,
        ..Options::default()
    };
    let m = Mundo::novo();
    let cfg = cfg_com(&[("max_open_proposals", json!(1000))]);
    let mut s = servico_com_opcoes(&m, cfg, &perfil_a(), limite_min);

    let mut ok = 0u64;
    let mut estourou = false;
    for i in 0..600 {
        let mut p = decisao(&format!("Corpo grande {i:04}"), "agentd");
        p["body"] = json!(repete('a', 4096));
        let v = muta(&mut s, TOK_A, "propose", p);
        if v["ok"] == json!(true) {
            ok += 1;
        } else {
            assert_eq!(cod(&v), "quota", "estouro de log_max_bytes = quota");
            estourou = true;
            break;
        }
    }
    assert!(estourou, "1 MiB deveria estourar antes de 600 propostas");
    assert!(ok >= 100, "cabem centenas de linhas de 4 KiB: {ok}");
    assert!(!s.is_broken(), "estouro de tamanho nao torna o log broken");
    assert_eq!(s.head_seq(), ok, "a proposta recusada nao grava");

    // leitura segue e a mutacao seguinte tambem e quota, nao broken
    assert_eq!(
        res(&get(&mut s, TOK_A, "D-001"))["status"],
        json!("proposed")
    );
    // a checagem e por linha: outra proposta de 4 KiB nao cabe e vira quota; nao e broken
    let mut grande = decisao("Depois do estouro", "agentd");
    grande["body"] = json!(repete('a', 4096));
    assert_eq!(cod(&muta(&mut s, TOK_A, "propose", grande)), "quota");
    assert_eq!(s.head_seq(), ok, "nada gravado depois do estouro");
    assert!(!s.is_broken());

    // controle: com o limite padrao (256 MiB) as mesmas propostas passam todas
    let m2 = Mundo::novo();
    let cfg2 = cfg_com(&[("max_open_proposals", json!(1000))]);
    let mut s2 = servico_cfg(&m2, cfg2, &perfil_a());
    for i in 0..300 {
        let mut p = decisao(&format!("Corpo grande {i:04}"), "agentd");
        p["body"] = json!(repete('a', 4096));
        let v = muta(&mut s2, TOK_A, "propose", p);
        assert_eq!(v["ok"], json!(true), "controle com limite folgado: {v}");
    }
    assert_eq!(s2.head_seq(), 300);
}

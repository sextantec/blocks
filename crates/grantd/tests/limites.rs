//! Restricoes de uso (G12 a G14): `max_uses`, `max_amount` com janela deslizante, e idempotencia de `use`.
mod common;

use common::*;
use serde_json::{Value, json};

// G12: max_uses = 2. `check` nao consome; o terceiro `use` e recusado (`denied`) e o `check` seguinte
// diz `exhausted`. Controle: antes dos usos, o `check` repetido continua allow.
#[test]
fn g12_max_uses_conta_so_use_e_check_nao_consome() {
    let d = Disco::novo();
    let r = Relogio::novo();
    let mut s = sobe(&d, &r, &grants_padrao());
    let p = com(
        allow(A1, "fs.read", "/p/x"),
        json!({"until": em(3600), "constraints": {"max_uses": 2}, "request_id": novo_rid()}),
    );
    let id = id_de(&pede_dono(&mut s, "grant", p));

    for _ in 0..5 {
        let v = decide(&mut s, A1, "fs.read", "/p/x");
        assert_decisao(&v, "allow", "ok");
        assert_eq!(grant_de(&v), json!(id));
    }

    let u1 = usa(&mut s, A1, "fs.read", "/p/x", &novo_rid(), json!({}));
    assert_eq!(resultado(&u1)["replayed"], json!(false), "{u1}");
    let u2 = usa(&mut s, A1, "fs.read", "/p/x", &novo_rid(), json!({}));
    assert_eq!(resultado(&u2)["replayed"], json!(false), "{u2}");

    let u3 = usa(&mut s, A1, "fs.read", "/p/x", &novo_rid(), json!({}));
    assert_eq!(codigo(&u3), "denied", "{u3}");
    assert_decisao(&decide(&mut s, A1, "fs.read", "/p/x"), "deny", "exhausted");
}

// G13: max_amount 10 usd, window_s 60. Uso de 6 e de 4 passam; 1 a mais e recusado. A janela e
// deslizante: soma os `use` com ts >= agora - window_s. Em t=60 ainda conta o uso de t=0 (intervalo fechado);
// em t=61 a janela esta vazia e volta a permitir.
#[test]
fn g13_max_amount_com_janela_deslizante() {
    let d = Disco::novo();
    let r = Relogio::novo();
    let mut s = sobe(&d, &r, &grants_padrao());
    let p = com(
        allow(A1, "spend", "openai:gpt-6-astra"),
        json!({
            "until": em(3600),
            "constraints": {"max_amount": 10, "unit": "usd", "window_s": 60},
            "request_id": novo_rid(),
        }),
    );
    id_de(&pede_dono(&mut s, "grant", p));
    let rec = "openai:gpt-6-astra";

    let u6 = usa(
        &mut s,
        A1,
        "spend",
        rec,
        &novo_rid(),
        json!({"qty": 6, "unit": "usd"}),
    );
    assert_eq!(resultado(&u6)["replayed"], json!(false), "{u6}");
    let u4 = usa(
        &mut s,
        A1,
        "spend",
        rec,
        &novo_rid(),
        json!({"qty": 4, "unit": "usd"}),
    );
    assert_eq!(resultado(&u4)["replayed"], json!(false), "{u4}");

    let u1 = usa(
        &mut s,
        A1,
        "spend",
        rec,
        &novo_rid(),
        json!({"qty": 1, "unit": "usd"}),
    );
    assert_eq!(codigo(&u1), "denied", "{u1}");
    assert_decisao(
        &decide_qty(&mut s, A1, "spend", rec, 1),
        "deny",
        "exhausted",
    );

    r.vai(60);
    assert_eq!(
        codigo(&usa(
            &mut s,
            A1,
            "spend",
            rec,
            &novo_rid(),
            json!({"qty": 1, "unit": "usd"})
        )),
        "denied"
    );
    assert_decisao(
        &decide_qty(&mut s, A1, "spend", rec, 1),
        "deny",
        "exhausted",
    );

    r.vai(61);
    assert_decisao(&decide_qty(&mut s, A1, "spend", rec, 1), "allow", "ok");
    let u_ok = usa(
        &mut s,
        A1,
        "spend",
        rec,
        &novo_rid(),
        json!({"qty": 1, "unit": "usd"}),
    );
    assert_eq!(resultado(&u_ok)["replayed"], json!(false), "{u_ok}");
}

// G14: `use` com o mesmo request_id e a mesma carga consome uma vez (`replayed:true`, sem commit novo);
// a mesma id com carga diferente = request_conflict. Controles: o `status` (head_seq) nao anda na repeticao,
// e o segundo pedido distinto ainda cabe (max_uses 2); o terceiro e recusado.
#[test]
fn g14_use_repetido_consome_uma_vez_e_carga_diferente_conflita() {
    let d = Disco::novo();
    let r = Relogio::novo();
    let mut s = sobe(&d, &r, &grants_padrao());
    let p = com(
        allow(A1, "fs.read", "/p/x"),
        json!({"until": em(3600), "constraints": {"max_uses": 2}, "request_id": novo_rid()}),
    );
    id_de(&pede_dono(&mut s, "grant", p));

    let r1 = novo_rid();
    let primeiro = usa(&mut s, A1, "fs.read", "/p/x", &r1, json!({}));
    assert_eq!(resultado(&primeiro)["replayed"], json!(false), "{primeiro}");
    let seq_depois_do_primeiro = head(&mut s);

    let repetido = usa(&mut s, A1, "fs.read", "/p/x", &r1, json!({}));
    assert_eq!(resultado(&repetido)["replayed"], json!(true), "{repetido}");
    assert_eq!(head(&mut s), seq_depois_do_primeiro, "repeticao nao grava");

    let conflito = usa(&mut s, A1, "fs.read", "/p/x", &r1, json!({"qty": 1}));
    assert_eq!(codigo(&conflito), "request_conflict", "{conflito}");

    let segundo = usa(&mut s, A1, "fs.read", "/p/x", &novo_rid(), json!({}));
    assert_eq!(
        resultado(&segundo)["replayed"],
        json!(false),
        "a repeticao nao consumiu o segundo slot: {segundo}"
    );
    let terceiro = usa(&mut s, A1, "fs.read", "/p/x", &novo_rid(), json!({}));
    assert_eq!(codigo(&terceiro), "denied", "{terceiro}");
}

// G36: `use` sem `qty` em concessao com `max_amount` = invalid_params (6A). Nao consome e nao grava.
// Controle: o mesmo `use` com qty e unit passa.
#[test]
fn g36_use_sem_qty_em_concessao_com_max_amount_e_invalid_params() {
    let d = Disco::novo();
    let r = Relogio::novo();
    let mut s = sobe(&d, &r, &grants_padrao());
    let p = com(
        allow(A1, "spend", "openai:gpt-6-astra"),
        json!({
            "until": em(3600),
            "constraints": {"max_amount": 10, "unit": "usd", "window_s": 60},
            "request_id": novo_rid(),
        }),
    );
    id_de(&pede_dono(&mut s, "grant", p));
    let rec = "openai:gpt-6-astra";

    let h0 = head(&mut s);
    let sem_qty = usa(&mut s, A1, "spend", rec, &novo_rid(), json!({}));
    assert_eq!(codigo(&sem_qty), "invalid_params", "{sem_qty}");
    assert_eq!(head(&mut s), h0, "use recusado nao grava");

    let com_qty = usa(
        &mut s,
        A1,
        "spend",
        rec,
        &novo_rid(),
        json!({"qty": 1, "unit": "usd"}),
    );
    assert_eq!(resultado(&com_qty)["replayed"], json!(false), "{com_qty}");
}

// G40: `use` repetido depois de `revoke` devolve a resposta historica e nao grava (5A.2). Controle: um
// `use` com request_id novo depois do revoke e recusado (`denied`), e o `check` diz no_grant.
#[test]
fn g40_use_repetido_apos_revoke_devolve_historico_e_nao_grava() {
    let d = Disco::novo();
    let r = Relogio::novo();
    let mut s = sobe(&d, &r, &grants_padrao());
    let id = concede(&mut s, A1, "fs.read", "/p/x", 3600);
    let rid = novo_rid();

    let primeiro = usa(&mut s, A1, "fs.read", "/p/x", &rid, json!({}));
    assert_eq!(resultado(&primeiro)["replayed"], json!(false), "{primeiro}");
    let r1 = resultado(&primeiro).clone();

    resultado(&pede_dono(
        &mut s,
        "revoke",
        json!({"id": id, "reason": "fim", "request_id": novo_rid()}),
    ));
    let h = head(&mut s);

    let repetido = usa(&mut s, A1, "fs.read", "/p/x", &rid, json!({}));
    let r2 = resultado(&repetido);
    assert_eq!(r2["replayed"], json!(true), "{repetido}");
    assert_eq!(r2["decision"], r1["decision"], "{repetido}");
    assert_eq!(r2["reason"], r1["reason"], "{repetido}");
    assert_eq!(r2["grant"], r1["grant"], "{repetido}");
    assert_eq!(r2["until"], r1["until"], "{repetido}");
    assert_eq!(head(&mut s), h, "repeticao nao grava");

    let novo = usa(&mut s, A1, "fs.read", "/p/x", &novo_rid(), json!({}));
    assert_eq!(codigo(&novo), "denied", "{novo}");
    assert_decisao(&decide(&mut s, A1, "fs.read", "/p/x"), "deny", "no_grant");
}

// G41: qty e max_amount sao inteiros 0..9007199254740991; unit obrigatoria e igual a da constraint;
// `check` sem qty vale qty 0 (saldo > 0 = allow; saldo 0 = exhausted); qty 0 e valida e nao consome saldo.
// Controles: o limite exato 9007199254740991 e aceito; qty 1 passa; check sem qty com saldo 10 = allow.
#[test]
fn g41_qty_e_max_amount_inteiros_unit_obrigatoria_e_check_sem_qty() {
    let d = Disco::novo();
    let r = Relogio::novo();
    let mut s = sobe(&d, &r, &grants_padrao());
    let grant_com = |max: Value, unit: Option<&str>| {
        let mut c = json!({"max_amount": max, "window_s": 60});
        if let Some(u) = unit {
            c["unit"] = json!(u);
        }
        com(
            allow(A1, "spend", "openai:y"),
            json!({"until": em(3600), "constraints": c, "request_id": novo_rid()}),
        )
    };

    for ruim in [
        json!(1.5),
        json!(-1),
        json!(9007199254740992u64),
        json!("10"),
    ] {
        let v = pede_dono(&mut s, "grant", grant_com(ruim.clone(), Some("usd")));
        assert_eq!(codigo(&v), "invalid_params", "max_amount {ruim}: {v}");
    }
    let sem_unit = pede_dono(&mut s, "grant", grant_com(json!(10), None));
    assert_eq!(codigo(&sem_unit), "invalid_params", "{sem_unit}");

    // Controle: o limite exato e aceito (em outro recurso, para nao misturar saldos).
    let limite = pede_dono(
        &mut s,
        "grant",
        com(
            allow(A2, "spend", "openai:lim"),
            json!({
                "until": em(3600),
                "constraints": {"max_amount": 9007199254740991u64, "unit": "usd", "window_s": 60},
                "request_id": novo_rid(),
            }),
        ),
    );
    resultado(&limite);

    let id = id_de(&pede_dono(
        &mut s,
        "grant",
        grant_com(json!(10), Some("usd")),
    ));
    let rec = "openai:y";
    assert_eq!(
        grant_de(&decide_sem_qty(&mut s, A1, "spend", rec)),
        json!(id)
    );

    let h = head(&mut s);
    for qty in [
        json!(1.5),
        json!(-1),
        json!("1"),
        json!(9007199254740992u64),
    ] {
        let v = usa(
            &mut s,
            A1,
            "spend",
            rec,
            &novo_rid(),
            json!({"qty": qty, "unit": "usd"}),
        );
        assert_eq!(codigo(&v), "invalid_params", "qty {qty}: {v}");
    }
    let unit_ruim = usa(
        &mut s,
        A1,
        "spend",
        rec,
        &novo_rid(),
        json!({"qty": 1, "unit": "eur"}),
    );
    assert_eq!(codigo(&unit_ruim), "invalid_params", "{unit_ruim}");
    assert_eq!(head(&mut s), h, "recusas nao gravam");

    let um = usa(
        &mut s,
        A1,
        "spend",
        rec,
        &novo_rid(),
        json!({"qty": 1, "unit": "usd"}),
    );
    assert_eq!(resultado(&um)["replayed"], json!(false), "{um}");
    let zero = usa(
        &mut s,
        A1,
        "spend",
        rec,
        &novo_rid(),
        json!({"qty": 0, "unit": "usd"}),
    );
    assert_eq!(
        resultado(&zero)["replayed"],
        json!(false),
        "qty 0 e valida: {zero}"
    );
    let nove = usa(
        &mut s,
        A1,
        "spend",
        rec,
        &novo_rid(),
        json!({"qty": 9, "unit": "usd"}),
    );
    assert_eq!(resultado(&nove)["replayed"], json!(false), "{nove}");

    let cheio = usa(
        &mut s,
        A1,
        "spend",
        rec,
        &novo_rid(),
        json!({"qty": 1, "unit": "usd"}),
    );
    assert_eq!(codigo(&cheio), "denied", "{cheio}");
    assert_decisao(
        &decide_sem_qty(&mut s, A1, "spend", rec),
        "deny",
        "exhausted",
    );
    assert_decisao(
        &decide_qty(&mut s, A1, "spend", rec, 0),
        "deny",
        "exhausted",
    );
}

// `check` sem `qty` (5A.3 = qty 0) no recurso `spend` indicado.
fn decide_sem_qty(
    s: &mut grantd::Service,
    agente: &str,
    acao: &str,
    recurso: &str,
) -> serde_json::Value {
    pede_como(
        s,
        agente,
        "check",
        serde_json::json!({"agent": agente, "action": acao, "resource": recurso, "unit": "usd"}),
    )
}

// G42: `grantee:"*"` com max_amount compartilha o saldo da concessao entre os agentes (5A.4).
// Controles: a3 com concessao propria no mesmo tipo de recurso tem saldo proprio (nao e o compartilhado).
#[test]
fn g42_grantee_curinga_compartilha_o_saldo_da_concessao() {
    let d = Disco::novo();
    let r = Relogio::novo();
    let mut s = sobe(&d, &r, &grants_padrao());
    let p = com(
        allow("*", "spend", "openai:c"),
        json!({
            "until": em(3600),
            "constraints": {"max_amount": 10, "unit": "usd", "window_s": 60},
            "request_id": novo_rid(),
        }),
    );
    let id = id_de(&pede_dono(&mut s, "grant", p));
    let rec = "openai:c";

    let a1 = usa(
        &mut s,
        A1,
        "spend",
        rec,
        &novo_rid(),
        json!({"qty": 6, "unit": "usd"}),
    );
    assert_eq!(grant_de(&a1), json!(id), "{a1}");
    let a2 = usa(
        &mut s,
        A2,
        "spend",
        rec,
        &novo_rid(),
        json!({"qty": 4, "unit": "usd"}),
    );
    assert_eq!(grant_de(&a2), json!(id), "{a2}");
    let a3 = usa(
        &mut s,
        A3,
        "spend",
        rec,
        &novo_rid(),
        json!({"qty": 1, "unit": "usd"}),
    );
    assert_eq!(
        codigo(&a3),
        "denied",
        "saldo e compartilhado, a3 esgotou: {a3}"
    );
    assert_decisao(
        &decide_qty(&mut s, A2, "spend", rec, 1),
        "deny",
        "exhausted",
    );

    // Controle: a3 com concessao propria, no mesmo tipo de recurso, tem saldo proprio.
    let proprio = com(
        allow(A3, "spend", "openai:d"),
        json!({
            "until": em(3600),
            "constraints": {"max_amount": 10, "unit": "usd", "window_s": 60},
            "request_id": novo_rid(),
        }),
    );
    resultado(&pede_dono(&mut s, "grant", proprio));
    let ok = usa(
        &mut s,
        A3,
        "spend",
        "openai:d",
        &novo_rid(),
        json!({"qty": 1, "unit": "usd"}),
    );
    assert_eq!(resultado(&ok)["replayed"], json!(false), "{ok}");
}

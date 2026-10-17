//! Decisao (G1 a G11): `check`, `explain` e o relogio, pela biblioteca `grantd::Service`
//! com `blocklog` sobre disco e relogio falsos. Oraculos exatos (decision, reason, grant, codigo).
mod common;

use chrono::{DateTime, SecondsFormat, Utc};
use common::*;
use serde_json::json;

fn instante(s: &str) -> DateTime<Utc> {
    DateTime::parse_from_rfc3339(s).unwrap().with_timezone(&Utc)
}

// G1: sem concessao, check = deny no_grant. Controle: a mesma consulta vira allow quando o dono concede.
#[test]
fn g01_sem_concessao_e_deny_no_grant() {
    let d = Disco::novo();
    let r = Relogio::novo();
    let mut s = sobe(&d, &r, &grants_padrao());

    let v = decide(&mut s, A1, "fs.read", "/p/x");
    assert_decisao(&v, "deny", "no_grant");
    assert!(
        grant_de(&v).is_null(),
        "deny sem concessao: grant deveria ser null: {v}"
    );

    let id = concede(&mut s, A1, "fs.read", "/p/x", 3600);
    let v = decide(&mut s, A1, "fs.read", "/p/x");
    assert_decisao(&v, "allow", "ok");
    assert_eq!(grant_de(&v), json!(id));
}

// G2: concessao casando devolve allow com o id; `until` ecoado no instante certo.
// Controle: a mesma acao para outro agente (a2) e sem casamento de grantee: deny no_grant.
#[test]
fn g02_allow_casando_devolve_o_id_e_o_until() {
    let d = Disco::novo();
    let r = Relogio::novo();
    let mut s = sobe(&d, &r, &grants_padrao());
    let id = concede(&mut s, A1, "fs.read", "/p/x", 3600);

    let v = decide(&mut s, A1, "fs.read", "/p/x");
    assert_decisao(&v, "allow", "ok");
    assert_eq!(grant_de(&v), json!(id));
    assert_eq!(resultado(&v)["signed"], json!(false));
    let until = resultado(&v)["until"].as_str().expect("until string");
    assert_eq!(instante(until), no(3600), "until ecoado: {v}");

    let outro = decide(&mut s, A2, "fs.read", "/p/x");
    assert_decisao(&outro, "deny", "no_grant");
}

// G3: deny casando vence allow casando, nos dois sentidos de criacao.
// Controle: um caminho que so o allow casa continua allow.
#[test]
fn g03_deny_vence_allow_nas_duas_ordens() {
    for deny_primeiro in [true, false] {
        let d = Disco::novo();
        let r = Relogio::novo();
        let mut s = sobe(&d, &r, &grants_padrao());
        let (id_allow, id_deny) = if deny_primeiro {
            let nd = nega(&mut s, A1, "fs.read", "/p/secreto", 3600);
            let na = concede(&mut s, A1, "fs.read", "/p/**", 3600);
            (na, nd)
        } else {
            let na = concede(&mut s, A1, "fs.read", "/p/**", 3600);
            let nd = nega(&mut s, A1, "fs.read", "/p/secreto", 3600);
            (na, nd)
        };

        let v = decide(&mut s, A1, "fs.read", "/p/secreto");
        assert_decisao(&v, "deny", "denied_by");
        assert_eq!(
            resultado(&v)["denied_by"],
            json!(id_deny),
            "ordem deny_primeiro={deny_primeiro}: {v}"
        );

        let controle = decide(&mut s, A1, "fs.read", "/p/outro");
        assert_decisao(&controle, "allow", "ok");
        assert_eq!(grant_de(&controle), json!(id_allow));
    }
}

// G4: `until` e o fim do intervalo: active em [not_before, until). Controle: um segundo antes, allow.
#[test]
fn g04_until_no_passado_e_deny_no_grant() {
    let d = Disco::novo();
    let r = Relogio::novo();
    let mut s = sobe(&d, &r, &grants_padrao());
    concede(&mut s, A1, "fs.read", "/p/x", 60);

    r.vai(59);
    assert_decisao(&decide(&mut s, A1, "fs.read", "/p/x"), "allow", "ok");

    r.vai(60);
    assert_decisao(&decide(&mut s, A1, "fs.read", "/p/x"), "deny", "no_grant");

    r.vai(61);
    assert_decisao(&decide(&mut s, A1, "fs.read", "/p/x"), "deny", "no_grant");
}

// G5: `not_before` futuro nao e active. Controle: no instante de not_before, allow.
#[test]
fn g05_not_before_futuro_e_deny_no_grant() {
    let d = Disco::novo();
    let r = Relogio::novo();
    let mut s = sobe(&d, &r, &grants_padrao());
    let p = com(
        allow(A1, "fs.read", "/p/x"),
        json!({"not_before": em(100), "until": em(1000), "request_id": novo_rid()}),
    );
    let id = id_de(&pede_dono(&mut s, "grant", p));

    assert_decisao(&decide(&mut s, A1, "fs.read", "/p/x"), "deny", "no_grant");

    r.vai(100);
    let v = decide(&mut s, A1, "fs.read", "/p/x");
    assert_decisao(&v, "allow", "ok");
    assert_eq!(grant_de(&v), json!(id));
}

// G6: glob por segmentos (`*` = um segmento, `**` = zero ou mais), sem regex.
// Tabela de 12 casos: (padrao da concessao, caminho consultado, decisao, caminho que o padrao deve casar).
// Cada negativo tem controle positivo no mesmo cenario: o `amostra` tem de dar allow.
const TABELA: &[(&str, &str, &str, &str)] = &[
    ("/a/b", "/a/b", "allow", "/a/b"),
    ("/a/b", "/a/c", "deny", "/a/b"),
    ("/a/*", "/a/b", "allow", "/a/b"),
    ("/a/*", "/a", "deny", "/a/z"),
    ("/a/*", "/a/b/c", "deny", "/a/z"),
    ("/a/**", "/a", "allow", "/a"),
    ("/a/**", "/a/b/c", "allow", "/a/b/c"),
    ("/a/**", "/ab", "deny", "/a/b"),
    ("/a/**/c", "/a/c", "allow", "/a/c"),
    ("/a/**/c", "/a/x/y/c", "allow", "/a/x/y/c"),
    ("/a/*/c", "/a/x/y/c", "deny", "/a/x/c"),
    ("/a/**", "/b/a", "deny", "/a/b"),
];

#[test]
fn g06_glob_por_segmentos_doze_caminhos() {
    for (padrao, caminho, esperado, amostra) in TABELA {
        let d = Disco::novo();
        let r = Relogio::novo();
        let mut s = sobe(&d, &r, &grants_padrao());
        concede(&mut s, A1, "fs.read", padrao, 3600);

        let v = decide(&mut s, A1, "fs.read", caminho);
        let motivo = if *esperado == "allow" {
            "ok"
        } else {
            "no_grant"
        };
        assert_eq!(
            resultado(&v)["decision"],
            json!(esperado),
            "padrao {padrao} caminho {caminho}: {v}"
        );
        assert_eq!(
            resultado(&v)["reason"],
            json!(motivo),
            "padrao {padrao} caminho {caminho}: {v}"
        );

        let controle = decide(&mut s, A1, "fs.read", amostra);
        assert_eq!(
            resultado(&controle)["decision"],
            json!("allow"),
            "controle: padrao {padrao} deveria casar {amostra}: {controle}"
        );
    }
}

// G7: `check` com resource nao canonico, com glob ou relativo = invalid_params (nao deny).
// Controles: o caminho canonico do mesmo cenario responde allow; `/` (raiz) e canonico e responde deny no_grant.
#[test]
fn g07_resource_nao_canonico_em_check_e_invalid_params() {
    let d = Disco::novo();
    let r = Relogio::novo();
    let mut s = sobe(&d, &r, &grants_padrao());
    concede(&mut s, A1, "fs.read", "/a/**", 3600);

    for ruim in ["/a/./b", "/a/../b", "a/b", "/a//b", "/a/", "/a/*", "/a/**"] {
        let v = decide(&mut s, A1, "fs.read", ruim);
        assert_eq!(codigo(&v), "invalid_params", "resource {ruim:?}: {v}");
    }
    assert_decisao(&decide(&mut s, A1, "fs.read", "/a/b"), "allow", "ok");
    assert_decisao(&decide(&mut s, A1, "fs.read", "/"), "deny", "no_grant");
}

// G8: grantee `*` vale para qualquer agente. Controle: concessao so de a1 nao vale para a2.
#[test]
fn g08_grantee_curinga_vale_para_qualquer_agente() {
    let d = Disco::novo();
    let r = Relogio::novo();
    let mut s = sobe(&d, &r, &grants_padrao());
    let id = concede(&mut s, "*", "fs.read", "/pub/**", 3600);
    for agente in [A1, A2, A3] {
        let v = decide(&mut s, agente, "fs.read", "/pub/x");
        assert_decisao(&v, "allow", "ok");
        assert_eq!(grant_de(&v), json!(id), "agente {agente}");
    }

    concede(&mut s, A1, "fs.read", "/priv/x", 3600);
    assert_decisao(&decide(&mut s, A1, "fs.read", "/priv/x"), "allow", "ok");
    assert_decisao(
        &decide(&mut s, A2, "fs.read", "/priv/x"),
        "deny",
        "no_grant",
    );
}

// G9: acao fora da lista = invalid_action (erro de uso), nao deny. Controle: acao valida sem concessao = deny.
#[test]
fn g09_acao_fora_da_lista_e_invalid_action_nao_deny() {
    let d = Disco::novo();
    let r = Relogio::novo();
    let mut s = sobe(&d, &r, &grants_padrao());

    let v = pede_como(
        &mut s,
        A1,
        "check",
        json!({"agent": A1, "action": "fs.delete", "resource": "/p/x"}),
    );
    assert_eq!(codigo(&v), "invalid_action", "{v}");

    assert_decisao(&decide(&mut s, A1, "fs.read", "/p/x"), "deny", "no_grant");
}

// G10: relogio regressivo. Ambos os casos usam max(agora, ultimo ts).
// (a) concessao vencida nao reviva quando o relogio volta para antes do `until`;
// (b) o `ts` de um commit nunca regride, mesmo com o relogio para tras.
#[test]
fn g10_relogio_regressivo_nao_reviva_concessao_nem_reabre_janela() {
    let d = Disco::novo();
    let r = Relogio::novo();
    let mut s = sobe(&d, &r, &grants_padrao());
    concede(&mut s, A1, "fs.read", "/p/x", 60);
    assert_decisao(&decide(&mut s, A1, "fs.read", "/p/x"), "allow", "ok");

    // Passa do until e grava um commit depois dele: o ultimo ts vira 61 s.
    r.vai(61);
    concede(&mut s, A2, "fs.read", "/q/y", 3600);
    assert_decisao(&decide(&mut s, A1, "fs.read", "/p/x"), "deny", "no_grant");

    // Relogio volta para 0 s (antes do until original): a concessao continua vencida.
    r.vai(0);
    assert_decisao(&decide(&mut s, A1, "fs.read", "/p/x"), "deny", "no_grant");
    // A concessao de a2 (valida ate 3600 s) continua valendo: o controle de que o relogio nao zerou nada.
    assert_decisao(&decide(&mut s, A2, "fs.read", "/q/y"), "allow", "ok");

    // (b) Um commit gravado com o relogio em 0 s recebe ts 61 s (o ultimo), nao 0 s.
    let p = com(
        allow(A1, "fs.read", "/w/z"),
        json!({"until": em(3600), "request_id": novo_rid()}),
    );
    pede_dono(&mut s, "grant", p);
    let linhas = linhas_do_log(&d);
    let ts_anterior = linhas[linhas.len() - 2]["ts"].as_str().unwrap().to_string();
    let ts_ultimo = linhas[linhas.len() - 1]["ts"].as_str().unwrap().to_string();
    assert_eq!(
        ts_anterior,
        no(61).to_rfc3339_opts(SecondsFormat::Millis, true)
    );
    assert_eq!(ts_ultimo, ts_anterior, "ts nao pode regredir: {ts_ultimo}");
}

// G10 (janela): com o relogio para tras, a janela deslizante usa o instante efetivo (o ultimo ts).
// Concessao max_amount 10 usd, janela 60 s: gasta 10 em t=0; em t=61 (ultimo ts) a janela esta vazia.
// Voltar o relogio para 0 nao fecha a janela de novo: o `check` responde como em t=61.
#[test]
fn g10b_relogio_regressivo_nao_fecha_nem_reabre_janela() {
    let d = Disco::novo();
    let r = Relogio::novo();
    let mut s = sobe(&d, &r, &grants_padrao());
    let p = com(
        allow(A1, "fs.read", "/p/x"),
        json!({
            "until": em(3600),
            "constraints": {"max_amount": 10, "unit": "usd", "window_s": 60},
            "request_id": novo_rid(),
        }),
    );
    id_de(&pede_dono(&mut s, "grant", p));

    let gasto = pede_como(
        &mut s,
        A1,
        "use",
        json!({"agent": A1, "action": "fs.read", "resource": "/p/x", "qty": 10, "unit": "usd", "request_id": novo_rid()}),
    );
    assert_eq!(resultado(&gasto)["replayed"], json!(false), "{gasto}");
    assert_decisao(
        &decide_qty(&mut s, A1, "fs.read", "/p/x", 1),
        "deny",
        "exhausted",
    );

    r.vai(61);
    concede(&mut s, A2, "fs.read", "/q/y", 3600);
    r.vai(0);
    assert_decisao(&decide_qty(&mut s, A1, "fs.read", "/p/x", 1), "allow", "ok");
}

// G11: explain lista em `matched` as concessoes que casaram, com `id`, `effect`, `active` e `why`.
// `denied_by` traz o id da deny quando o motivo e `denied_by` (6A). Controle: um caminho so casado pela allow.
fn ids_casados(r: &serde_json::Value) -> Vec<String> {
    let mut ids: Vec<String> = r["matched"]
        .as_array()
        .unwrap_or_else(|| panic!("explain sem `matched`: {r}"))
        .iter()
        .map(|m| m["id"].as_str().expect("id").to_string())
        .collect();
    ids.sort();
    ids
}

#[test]
fn g11_explain_lista_as_que_casaram_e_a_regra() {
    let d = Disco::novo();
    let r = Relogio::novo();
    let mut s = sobe(&d, &r, &grants_padrao());
    let id_a = concede(&mut s, A1, "fs.read", "/p/**", 3600);
    let id_b = concede(&mut s, A1, "fs.read", "/q/**", 3600);
    let id_d = nega(&mut s, A1, "fs.read", "/p/x", 3600);

    let v = pede_como(
        &mut s,
        A1,
        "explain",
        json!({"agent": A1, "action": "fs.read", "resource": "/p/x"}),
    );
    let r1 = resultado(&v);
    assert_eq!(r1["decision"], json!("deny"), "{v}");
    assert_eq!(r1["reason"], json!("denied_by"), "{v}");
    assert_eq!(r1["denied_by"], json!(id_d), "{v}");
    let mut esperado = vec![id_a.clone(), id_d.clone()];
    esperado.sort();
    assert_eq!(ids_casados(r1), esperado, "matched exato: {v}");
    assert!(!ids_casados(r1).contains(&id_b), "{v}");
    for m in r1["matched"].as_array().unwrap() {
        assert_eq!(m["active"], json!(true), "{m}");
        assert!(m["why"].is_string(), "why ausente: {m}");
        if m["id"] == json!(id_d) {
            assert_eq!(m["effect"], json!("deny"), "{m}");
        } else {
            assert_eq!(m["effect"], json!("allow"), "{m}");
        }
    }

    let controle = pede_como(
        &mut s,
        A1,
        "explain",
        json!({"agent": A1, "action": "fs.read", "resource": "/p/y"}),
    );
    let r2 = resultado(&controle);
    assert_eq!(r2["decision"], json!("allow"), "{controle}");
    assert_eq!(r2["reason"], json!("ok"), "{controle}");
    assert_eq!(r2["grant"], json!(id_a), "{controle}");
    assert_eq!(
        ids_casados(r2),
        vec![id_a],
        "so a allow casa /p/y: {controle}"
    );
}

// G35: `check` e `explain` de um agente sobre OUTRO agente com token de agente = forbidden (6A).
// O dono consulta qualquer agente. Controles: a1 consulta a si mesmo; o dono consulta a2 e recebe allow.
#[test]
fn g35_agente_nao_consulta_outro_agente_dono_consulta() {
    let d = Disco::novo();
    let r = Relogio::novo();
    let mut s = sobe(&d, &r, &grants_padrao());
    let id_a2 = concede(&mut s, A2, "fs.read", "/p/x", 3600);

    for metodo in ["check", "explain"] {
        let v = pede_como(
            &mut s,
            A1,
            metodo,
            json!({"agent": A2, "action": "fs.read", "resource": "/p/x"}),
        );
        assert_eq!(codigo(&v), "forbidden", "{metodo} de a1 sobre a2: {v}");
    }

    assert_decisao(&decide(&mut s, A1, "fs.read", "/p/x"), "deny", "no_grant");
    assert_decisao(&decide(&mut s, A2, "fs.read", "/p/x"), "allow", "ok");

    let do_dono = pede_dono(
        &mut s,
        "check",
        json!({"agent": A2, "action": "fs.read", "resource": "/p/x"}),
    );
    assert_decisao(&do_dono, "allow", "ok");
    assert_eq!(grant_de(&do_dono), json!(id_a2), "{do_dono}");
    let explica = pede_dono(
        &mut s,
        "explain",
        json!({"agent": A2, "action": "fs.read", "resource": "/p/x"}),
    );
    assert_eq!(resultado(&explica)["decision"], json!("allow"), "{explica}");
}

// G38: `use` e `check` so do proprio principal (5A.1): `agent` diferente do token = forbidden. O owner pode
// `check` de terceiro, mas nao `use` em nome de terceiro. Controles: a1 usa para si; o dono consulta a2 e
// recebe allow com o id de a2. Nenhuma recusa grava.
#[test]
fn g38_use_e_check_so_do_proprio_principal() {
    let d = Disco::novo();
    let r = Relogio::novo();
    let mut s = sobe(&d, &r, &grants_padrao());
    let id1 = concede(&mut s, A1, "fs.read", "/p/x", 3600);
    let id2 = concede(&mut s, A2, "fs.read", "/p/x", 3600);
    let h0 = head(&mut s);

    let use_alheio = usa(
        &mut s,
        A1,
        "fs.read",
        "/p/x",
        &novo_rid(),
        json!({"agent": A2}),
    );
    assert_eq!(codigo(&use_alheio), "forbidden", "{use_alheio}");
    let use_dono = pede_dono(
        &mut s,
        "use",
        json!({"agent": A2, "action": "fs.read", "resource": "/p/x", "request_id": novo_rid()}),
    );
    assert_eq!(codigo(&use_dono), "forbidden", "{use_dono}");
    let check_alheio = pede_como(
        &mut s,
        A1,
        "check",
        json!({"agent": A2, "action": "fs.read", "resource": "/p/x"}),
    );
    assert_eq!(codigo(&check_alheio), "forbidden", "{check_alheio}");
    assert_eq!(head(&mut s), h0, "recusas nao gravam");

    let proprio = usa(&mut s, A1, "fs.read", "/p/x", &novo_rid(), json!({}));
    assert_eq!(resultado(&proprio)["replayed"], json!(false), "{proprio}");
    assert_eq!(grant_de(&proprio), json!(id1), "{proprio}");

    let dono_ve = pede_dono(
        &mut s,
        "check",
        json!({"agent": A2, "action": "fs.read", "resource": "/p/x"}),
    );
    assert_decisao(&dono_ve, "allow", "ok");
    assert_eq!(grant_de(&dono_ve), json!(id2), "{dono_ve}");
}

// G39: o mesmo `request_id` de principais diferentes e chave independente (5A.1): a2 com o rid de a1
// nao recebe o resultado de a1 nem e recusado; cada repeticao devolve o historico do proprio principal.
#[test]
fn g39_request_id_igual_de_principais_diferentes_sao_chaves_independentes() {
    let d = Disco::novo();
    let r = Relogio::novo();
    let mut s = sobe(&d, &r, &grants_padrao());
    let id1 = concede(&mut s, A1, "fs.read", "/p/x", 3600);
    let id2 = concede(&mut s, A2, "fs.read", "/p/x", 3600);
    let rid = novo_rid();

    let u1 = usa(&mut s, A1, "fs.read", "/p/x", &rid, json!({}));
    assert_eq!(resultado(&u1)["replayed"], json!(false), "{u1}");
    assert_eq!(grant_de(&u1), json!(id1), "{u1}");

    let u2 = usa(&mut s, A2, "fs.read", "/p/x", &rid, json!({}));
    assert_eq!(
        resultado(&u2)["replayed"],
        json!(false),
        "a2 nao recebe o resultado de a1: {u2}"
    );
    assert_eq!(grant_de(&u2), json!(id2), "{u2}");

    let u1b = usa(&mut s, A1, "fs.read", "/p/x", &rid, json!({}));
    assert_eq!(resultado(&u1b)["replayed"], json!(true), "{u1b}");
    assert_eq!(grant_de(&u1b), json!(id1), "{u1b}");

    let u2b = usa(&mut s, A2, "fs.read", "/p/x", &rid, json!({}));
    assert_eq!(resultado(&u2b)["replayed"], json!(true), "{u2b}");
    assert_eq!(grant_de(&u2b), json!(id2), "{u2b}");
}

// G54: a leitura observada avanca `t_max` sem commit e nunca regride (5A.6). Concessao ate 60 s.
// Em 59 s: allow (controle). Em 61 s (so leitura): deny no_grant, sem commit. Relogio voltando a 0 s:
// continua deny, porque `t_max` ja e 61 s. `status.t_max` = 61 s (formato RFC 3339 assumido).
#[test]
fn g54_leitura_observada_avanca_t_max_sem_commit_e_nao_regride() {
    let d = Disco::novo();
    let r = Relogio::novo();
    let mut s = sobe(&d, &r, &grants_padrao());
    concede(&mut s, A1, "fs.read", "/p/x", 60);

    r.vai(59);
    assert_decisao(&decide(&mut s, A1, "fs.read", "/p/x"), "allow", "ok");

    r.vai(61);
    let h = head(&mut s);
    assert_decisao(&decide(&mut s, A1, "fs.read", "/p/x"), "deny", "no_grant");
    assert_eq!(head(&mut s), h, "leitura nao grava");

    r.vai(0);
    assert_decisao(&decide(&mut s, A1, "fs.read", "/p/x"), "deny", "no_grant");

    let st_resp = pede_dono(&mut s, "status", json!({}));
    let st = resultado(&st_resp);
    let t_max = st["t_max"].as_str().expect("t_max string RFC 3339");
    assert_eq!(instante(t_max), no(61), "{st}");

    // Controle: uma concessao criada agora (ts = t_max) vale com o relogio em 0 s.
    concede(&mut s, A2, "fs.read", "/q/y", 3600);
    assert_decisao(&decide(&mut s, A2, "fs.read", "/q/y"), "allow", "ok");
}

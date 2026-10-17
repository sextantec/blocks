//! Durabilidade e falhas (G28 e G33) pela biblioteca, com disco falso.
mod common;

use common::*;
use serde_json::{Value, json};

/// Gerador pseudoaleatorio de semente fixa (LCG de 64 bits). Nao e criptografico; so da sequencia repetivel.
fn prox(semente: &mut u64) -> u64 {
    *semente = semente
        .wrapping_mul(6364136223846793005)
        .wrapping_add(1442695040888963407);
    *semente >> 33
}

// G28: 150 mutacoes pseudoaleatorias (semente fixa) com o relogio andando. Cada resposta `ok` de mutacao
// e exatamente um commit, entao head_seq == numero de respostas ok. Depois de reabrir o mesmo disco,
// head_seq e o `list` (dono e a1) sao byte a byte iguais aos de antes da reabertura.
#[test]
fn g28_reconstruir_do_log_igual_ao_vivo_apos_150_mutacoes() {
    let d = Disco::novo();
    let r = Relogio::novo();
    let perfis = perfis_delegacao();
    let grants = grants_de(&perfis);
    let mut s = sobe(&d, &r, &grants);

    let agentes = [A1, A2, A3];
    let mut semente: u64 = 0x5EED_0028;
    let mut concessoes: Vec<String> = Vec::new();
    let mut pedidos: Vec<String> = Vec::new();
    let mut sucessos: u64 = 0;
    let mut t: i64 = 0;

    for n in 0..150u64 {
        t += (prox(&mut semente) % 30) as i64;
        r.vai(t);
        let escolha = prox(&mut semente) % 9;
        let agente = agentes[(prox(&mut semente) % 3) as usize];
        let ate = t + 100 + (prox(&mut semente) % 5000) as i64;
        let resposta: Value = match escolha {
            0 | 1 => {
                let grantee = ["*", A1, A2, A3][(prox(&mut semente) % 4) as usize];
                let v = pede_dono(
                    &mut s,
                    "grant",
                    com(
                        allow(grantee, "fs.read", &format!("/r/{n}/**")),
                        json!({"until": em(ate), "request_id": novo_rid()}),
                    ),
                );
                if v["ok"] == json!(true) {
                    concessoes.push(id_de(&v));
                }
                v
            }
            2 => pede_dono(
                &mut s,
                "deny_rule",
                com(
                    allow(agente, "fs.read", &format!("/r/{}/x", n / 2)),
                    json!({"effect": "deny", "until": em(ate), "request_id": novo_rid()}),
                ),
            ),
            3 if !concessoes.is_empty() => {
                let alvo = concessoes[(prox(&mut semente) as usize) % concessoes.len()].clone();
                pede_dono(
                    &mut s,
                    "revoke",
                    json!({"id": alvo, "reason": "sorteio", "request_id": novo_rid()}),
                )
            }
            4 => {
                let v = pede_como(
                    &mut s,
                    agente,
                    "request",
                    json!({"action": "fs.write", "resource": format!("/w/{n}"), "until": em(ate), "request_id": novo_rid()}),
                );
                if v["ok"] == json!(true) {
                    pedidos.push(id_de(&v));
                }
                v
            }
            5 if !pedidos.is_empty() => {
                let alvo = pedidos[(prox(&mut semente) as usize) % pedidos.len()].clone();
                pede_dono(
                    &mut s,
                    "approve_request",
                    json!({"req": alvo, "until": em(ate), "request_id": novo_rid()}),
                )
            }
            6 if !pedidos.is_empty() => {
                let alvo = pedidos[(prox(&mut semente) as usize) % pedidos.len()].clone();
                pede_dono(
                    &mut s,
                    "reject_request",
                    json!({"req": alvo, "reason": "sorteio", "request_id": novo_rid()}),
                )
            }
            7 if !concessoes.is_empty() => {
                let pai = concessoes[(prox(&mut semente) as usize) % concessoes.len()].clone();
                pede_como(
                    &mut s,
                    A1,
                    "delegate",
                    json!({"parent": pai, "grantee": A2, "action": "fs.read", "resource": format!("/r/{n}/sub"), "until": em(ate), "request_id": novo_rid()}),
                )
            }
            _ => pede_como(
                &mut s,
                agente,
                "use",
                json!({"agent": agente, "action": "fs.read", "resource": format!("/r/{}/x", n / 2), "request_id": novo_rid()}),
            ),
        };
        if resposta["ok"] == json!(true) {
            sucessos += 1;
        }
    }

    assert!(
        sucessos > 0,
        "a sequencia nao produziu nenhuma mutacao aceita"
    );
    assert_eq!(
        head(&mut s),
        sucessos,
        "cada resposta ok de mutacao e um commit"
    );

    let antes_head = head(&mut s);
    let antes_dono = bruto(&mut s, &tok_dono(), "fixo-list-dono", "list", json!({}));
    let antes_a1 = bruto(&mut s, &token_de(A1), "fixo-list-a1", "list", json!({}));
    drop(s);

    let mut reaberto = sobe(&d, &r, &grants);
    assert_eq!(
        head(&mut reaberto),
        antes_head,
        "head_seq depois de reabrir"
    );
    let depois_dono = bruto(
        &mut reaberto,
        &tok_dono(),
        "fixo-list-dono",
        "list",
        json!({}),
    );
    let depois_a1 = bruto(
        &mut reaberto,
        &token_de(A1),
        "fixo-list-a1",
        "list",
        json!({}),
    );
    assert_eq!(depois_dono, antes_dono, "list do dono mudou apos reabrir");
    assert_eq!(depois_a1, antes_a1, "list de a1 mudou apos reabrir");
}

// G33: falha de escrita (EIO ou ENOSPC, nada gravado) deixa o log `broken` (5A.7): mutacoes respondem
// `broken`; `check` responde deny com reason `broken` (fail-closed, nao mais allow); a concessao que falhou
// nao existe. Controles: antes da falha a consulta e allow e `status.broken` e false; depois, `status.broken`
// e true, `status` segue ok e `list` ainda mostra a concessao anterior.
#[test]
fn g33_falha_de_escrita_deixa_broken_e_check_continua() {
    for falha in [Falha::Eio, Falha::Enospc] {
        let d = Disco::novo();
        let r = Relogio::novo();
        let mut s = sobe(&d, &r, &grants_padrao());
        let id_antes = concede(&mut s, A1, "fs.read", "/p/x", 3600);
        assert!(!s.is_broken());
        assert_decisao(&decide(&mut s, A1, "fs.read", "/p/x"), "allow", "ok");
        assert_eq!(
            resultado(&pede_dono(&mut s, "status", json!({})))["broken"],
            json!(false)
        );
        let bytes_antes = d.bytes();

        d.arma_escrita(falha);
        let falhou = pede_dono(
            &mut s,
            "grant",
            com(
                allow(A2, "fs.read", "/q/y"),
                json!({"until": em(3600), "request_id": novo_rid()}),
            ),
        );
        assert_eq!(codigo(&falhou), "broken", "{falha:?}: {falhou}");
        assert!(s.is_broken(), "{falha:?}: o servico deveria estar broken");
        assert_eq!(d.bytes(), bytes_antes, "{falha:?}: nada foi gravado");

        let st_resp = pede_dono(&mut s, "status", json!({}));
        let st = resultado(&st_resp);
        assert_eq!(st["broken"], json!(true), "{falha:?}: {st}");
        assert!(st["uptime_s"].is_number(), "{st}");

        let seguinte = pede_dono(
            &mut s,
            "revoke",
            json!({"id": id_antes, "reason": "depois da falha", "request_id": novo_rid()}),
        );
        assert_eq!(codigo(&seguinte), "broken", "{falha:?}: {seguinte}");

        let v = decide(&mut s, A1, "fs.read", "/p/x");
        assert_decisao(&v, "deny", "broken");
        assert!(grant_de(&v).is_null(), "{v}");
        assert_decisao(&decide(&mut s, A2, "fs.read", "/q/y"), "deny", "broken");

        let lista = pede_dono(&mut s, "list", json!({}));
        let ids: Vec<Value> = resultado(&lista)["grants"]
            .as_array()
            .expect("grants")
            .iter()
            .map(|g| g["id"].clone())
            .collect();
        assert!(ids.contains(&json!(id_antes)), "{falha:?}: {lista}");
        assert_eq!(head(&mut s), 1, "{falha:?}: so o primeiro commit existe");
    }
}

// G49: `broken` e fail-closed (5A.7): `check`, `explain` e `use` respondem deny com reason `broken`
// (`use` negado = erro `denied`, assumido; a spec nao diz em que envelope o reason aparece).
// `list`, `show` e `status` seguem ok. Controle: antes da falha a mesma consulta e allow.
#[test]
fn g49_broken_e_fail_closed_mas_leitura_segue() {
    let d = Disco::novo();
    let r = Relogio::novo();
    let mut s = sobe(&d, &r, &grants_padrao());
    let id = concede(&mut s, A1, "fs.read", "/p/x", 3600);
    assert_decisao(&decide(&mut s, A1, "fs.read", "/p/x"), "allow", "ok");

    d.arma_escrita(Falha::Eio);
    let falhou = pede_dono(
        &mut s,
        "grant",
        com(
            allow(A2, "fs.read", "/q/y"),
            json!({"until": em(3600), "request_id": novo_rid()}),
        ),
    );
    assert_eq!(codigo(&falhou), "broken", "{falhou}");

    assert_decisao(&decide(&mut s, A1, "fs.read", "/p/x"), "deny", "broken");

    let explica = pede_como(
        &mut s,
        A1,
        "explain",
        json!({"agent": A1, "action": "fs.read", "resource": "/p/x"}),
    );
    assert_eq!(resultado(&explica)["decision"], json!("deny"), "{explica}");
    assert_eq!(resultado(&explica)["reason"], json!("broken"), "{explica}");

    let h = head(&mut s);
    let uso = usa(&mut s, A1, "fs.read", "/p/x", &novo_rid(), json!({}));
    assert_eq!(codigo(&uso), "denied", "{uso}");
    assert_eq!(head(&mut s), h, "use em broken nao grava");

    let mostra = pede_dono(&mut s, "show", json!({"id": id}));
    assert_eq!(resultado(&mostra)["id"], json!(id), "{mostra}");
    let lista = pede_como(&mut s, A1, "list", json!({}));
    let ids: Vec<Value> = resultado(&lista)["grants"]
        .as_array()
        .expect("grants")
        .iter()
        .map(|g| g["id"].clone())
        .collect();
    assert!(ids.contains(&json!(id)), "{lista}");
    let st_resp = pede_dono(&mut s, "status", json!({}));
    let st = resultado(&st_resp);
    assert_eq!(st["broken"], json!(true), "{st}");
}

// G59 (5A.16): em `broken`, `use` = erro `denied` com `error.message` exatamente "broken"; `check` =
// `decision:"deny"` com `reason:"broken"` (5A.7). Controle: antes da falha, o mesmo `use` passa e o `check`
// e allow.
#[test]
fn g59_use_em_broken_tem_message_broken_e_check_reason_broken() {
    let d = Disco::novo();
    let r = Relogio::novo();
    let mut s = sobe(&d, &r, &grants_padrao());
    concede(&mut s, A1, "fs.read", "/p/x", 3600);
    let antes = usa(&mut s, A1, "fs.read", "/p/x", &novo_rid(), json!({}));
    assert_eq!(resultado(&antes)["replayed"], json!(false), "{antes}");
    assert_decisao(&decide(&mut s, A1, "fs.read", "/p/x"), "allow", "ok");

    d.arma_escrita(Falha::Eio);
    let falhou = pede_dono(
        &mut s,
        "grant",
        com(
            allow(A2, "fs.read", "/q/y"),
            json!({"until": em(3600), "request_id": novo_rid()}),
        ),
    );
    assert_eq!(codigo(&falhou), "broken", "{falhou}");

    let h = head(&mut s);
    let uso = usa(&mut s, A1, "fs.read", "/p/x", &novo_rid(), json!({}));
    assert_eq!(codigo(&uso), "denied", "{uso}");
    assert_eq!(
        uso["error"]["message"],
        json!("broken"),
        "message exata: {uso}"
    );
    assert_eq!(head(&mut s), h, "use em broken nao grava");

    let v = decide(&mut s, A1, "fs.read", "/p/x");
    assert_decisao(&v, "deny", "broken");
    assert!(grant_de(&v).is_null(), "{v}");
}

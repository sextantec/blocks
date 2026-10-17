//! O4 (reconstrucao igual ao vivo), O5 (broken por falha injetada) e robustez de `handle_line`
//! (memd-spec-v0.md, secoes 8A e 10). Os testes binarios de operacao (O1, O2, O3) estao em binario.rs.

mod common;

use common::*;
use serde_json::{Value, json};

fn perfil_a() -> String {
    grants(&[perfil_total("agente-a", TOK_A)])
}

/// Gerador pseudoaleatorio deterministico (LCG); a semente e fixa.
struct Semente(u64);

impl Semente {
    fn proximo(&mut self) -> u64 {
        self.0 = self
            .0
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        self.0 >> 33
    }

    fn ate(&mut self, n: u64) -> u64 {
        self.proximo() % n
    }
}

/// Estado observavel completo: todas as versoes conhecidas (get com rev) e as listagens por status.
fn fotografia(s: &mut memd::Service, versoes: &[(String, u64)]) -> Value {
    let mut listagens = Vec::new();
    for status in [
        "proposed",
        "active",
        "superseded",
        "retired",
        "rejected",
        "expired",
    ] {
        for tipo in ["decision", "pending"] {
            let mut todos = Vec::new();
            let mut after: Option<String> = None;
            loop {
                let mut p = json!({"type": tipo, "status": status, "limit": 50});
                if let Some(a) = &after {
                    p["after"] = json!(a);
                }
                let r = consulta(s, TOK_A, p);
                todos.push(r["items"].clone());
                match r["next_after"].as_str() {
                    Some(a) => after = Some(a.to_string()),
                    None => break,
                }
            }
            listagens.push(json!({"status": status, "type": tipo, "paginas": todos}));
        }
    }
    let mut regs = Vec::new();
    for (id, rev) in versoes {
        let v = chama(s, TOK_A, "get", &json!({"id": id, "rev": rev}));
        regs.push(json!({"id": id, "rev": rev, "resp": v["result"].clone(), "erro": v["error"]["code"].clone()}));
    }
    json!({"head_seq": s.head_seq(), "listagens": listagens, "registros": regs})
}

// ------------------------------------------------------------------ O4

#[test]
fn o04_estado_reconstruido_do_log_e_igual_ao_vivo_apos_200_mutacoes() {
    let m = Mundo::novo();
    let cfg = cfg_com(&[("max_open_proposals", json!(100))]);
    let mut vivo = servico_cfg(&m, cfg, &perfil_a());
    let mut sem = Semente(0x5EED_0B5E);
    let mut versoes: Vec<(String, u64)> = Vec::new();
    // (id, rev, seq de proposta) das versoes que ainda podem ser promovidas
    let mut abertas: Vec<(String, u64, u64)> = Vec::new();
    let mut sucessos = 0u32;
    let mut titulo = 0u32;

    for _ in 0..200 {
        let escolha = sem.ate(100);
        let resp = match escolha {
            // propose novo (decisao ou pendencia), 30%
            0..=29 => {
                titulo += 1;
                let escopo = if sem.ate(2) == 0 {
                    "agentd"
                } else {
                    "blocklog"
                };
                let p = if sem.ate(4) == 0 {
                    pendencia(&format!("Pendencia {titulo}"), escopo, "2026-12-01")
                } else {
                    decisao(&format!("Decisao {titulo}"), escopo)
                };
                Some(muta(&mut vivo, TOK_A, "propose", p))
            }
            // revisao de um id conhecido, com expected_rev lido da active (0 se nao ha), 20%
            30..=49 if !versoes.is_empty() => {
                let (id, _) = versoes[sem.ate(versoes.len() as u64) as usize].clone();
                let g = chama(&mut vivo, TOK_A, "get", &json!({"id": id}));
                let expected = match g["result"]["status"].as_str() {
                    Some("active") => g["result"]["rev"].as_u64().unwrap(),
                    _ => 0,
                };
                let mut p = if id.starts_with('P') {
                    pendencia(&format!("Rev {titulo}"), "agentd", "2026-12-01")
                } else {
                    decisao(&format!("Rev {titulo}"), "agentd")
                };
                p["id"] = json!(id);
                p["expected_rev"] = json!(expected);
                Some(muta(&mut vivo, TOK_A, "propose", p))
            }
            // promocao de uma versao aberta, 30%
            50..=79 if !abertas.is_empty() => {
                let i = sem.ate(abertas.len() as u64) as usize;
                let (id, rev, seq) = abertas[i].clone();
                Some(muta(
                    &mut vivo,
                    TOK_A,
                    "promote",
                    json!({"id": id, "rev": rev, "expected_seq": seq}),
                ))
            }
            // rejeicao ou aposentadoria, 10%
            80..=89 if !versoes.is_empty() => {
                let (id, rev) = versoes[sem.ate(versoes.len() as u64) as usize].clone();
                let metodo = if sem.ate(2) == 0 { "reject" } else { "retire" };
                Some(muta(
                    &mut vivo,
                    TOK_A,
                    metodo,
                    json!({"id": id, "rev": rev, "reason": "teste"}),
                ))
            }
            // relogio anda (e as vezes volta), e tick, 10%
            _ => {
                let dias = sem.ate(9) as i64 - 2; // -2 .. +6 dias
                let base = instante(T0);
                let passo = chrono_add_dias(base, dias + sem.ate(3) as i64 * 4);
                m.relogio.poe(&passo);
                vivo.tick();
                None
            }
        };

        if let Some(v) = resp {
            if v["ok"] == json!(true) {
                sucessos += 1;
                let r = &v["result"];
                if let (Some(id), Some(rev)) = (r["id"].as_str(), r["rev"].as_u64()) {
                    if r["status"] == json!("proposed") {
                        let seq = r["seq"].as_u64().unwrap();
                        versoes.push((id.to_string(), rev));
                        abertas.push((id.to_string(), rev, seq));
                    }
                }
            }
        }
    }

    assert!(
        sucessos >= 50,
        "a sequencia precisa exercitar o sistema: {sucessos} sucessos"
    );
    let head_vivo = vivo.head_seq();
    assert!(head_vivo >= 50, "commits gravados: {head_vivo}");
    let foto_viva = fotografia(&mut vivo, &versoes);

    // reinicio: mesmo disco, mesmo relogio, novo Service a partir do log inteiro
    drop(vivo);
    let mut reaberto = servico_cfg(
        &m,
        cfg_com(&[("max_open_proposals", json!(100))]),
        &perfil_a(),
    );
    let foto_reaberta = fotografia(&mut reaberto, &versoes);

    assert_eq!(
        foto_reaberta, foto_viva,
        "estado reconstruido = estado vivo"
    );
    assert_eq!(reaberto.head_seq(), head_vivo);
}

/// Soma dias (inteiro, pode ser negativo) a um instante, e devolve RFC 3339 com milissegundos.
fn chrono_add_dias(base: chrono::DateTime<chrono::Utc>, dias: i64) -> String {
    let t = base + chrono::Duration::days(dias);
    t.format("%Y-%m-%dT%H:%M:%S%.3fZ").to_string()
}

// ------------------------------------------------------------------ O5

#[test]
fn o05_falha_injetada_torna_broken_sem_aplicar_e_leituras_continuam() {
    let falhas: Vec<(&str, fn(&Mundo))> = vec![
        ("EIO na escrita", |m| m.disco.arma_escrita(Falha::Eio)),
        ("ENOSPC na escrita", |m| m.disco.arma_escrita(Falha::Enospc)),
        ("escrita curta", |m| {
            m.disco.arma_escrita(Falha::EscritaCurta(5))
        }),
        ("EIO no sync_data", |m| m.disco.arma_sync(Falha::Eio)),
        ("ENOSPC no sync_data", |m| m.disco.arma_sync(Falha::Enospc)),
    ];
    for (nome, arma) in falhas {
        let m = Mundo::novo();
        let mut s = servico(&m, &perfil_a());
        propoe_e_promove(&mut s, TOK_A, decisao("Antes da falha", "agentd")); // seq 1,2
        let head = s.head_seq();
        assert!(!s.is_broken(), "{nome}: controle, ainda nao quebrou");

        arma(&m);
        let v = muta(
            &mut s,
            TOK_A,
            "propose",
            decisao("Durante a falha", "agentd"),
        );
        assert_eq!(cod(&v), "broken", "{nome}");
        assert!(s.is_broken(), "{nome}");
        assert_eq!(s.head_seq(), head, "{nome}: nada aplicado");
        assert_eq!(
            cod(&get(&mut s, TOK_A, "D-002")),
            "not_found",
            "{nome}: o que nao foi commitado nao existe na memoria"
        );

        // mutacoes seguintes: broken (fail-closed)
        assert_eq!(
            cod(&muta(&mut s, TOK_A, "propose", decisao("Depois", "agentd"))),
            "broken",
            "{nome}"
        );
        assert_eq!(
            cod(&muta(
                &mut s,
                TOK_A,
                "retire",
                json!({"id": "D-001", "rev": 1, "reason": "x"})
            )),
            "broken",
            "{nome}"
        );

        // leituras seguem
        assert_eq!(
            res(&get(&mut s, TOK_A, "D-001"))["status"],
            json!("active"),
            "{nome}"
        );
        assert_eq!(
            ids_da_consulta(&consulta(&mut s, TOK_A, json!({}))),
            vec!["D-001".to_string()],
            "{nome}"
        );
        assert_eq!(
            res(&chama(&mut s, TOK_A, "status", &json!({})))["broken"],
            json!(true),
            "{nome}"
        );
    }
}

// ------------------------------------------------------------------ robustez de handle_line

#[test]
fn o09_handle_line_nao_entra_em_panico_e_responde_com_erro_fechado() {
    let m = Mundo::novo();
    let mut s = servico_cfg(&m, cfg_com(&[]), &perfil_a());

    let entradas: Vec<String> = vec![
        String::new(),
        "{".to_string(),
        "null".to_string(),
        "[]".to_string(),
        "42".to_string(),
        "\"texto\"".to_string(),
        "{}".to_string(),
        "{\"v\":1,\"id\":\"c\",\"token\":\"mt_\\u0000\"}".to_string(),
        format!(r#"{{"v":2,"id":"c","token":"{TOK_A}","method":"status","params":{{}}}}"#),
        format!(r#"{{"v":1,"id":"c","token":"{TOK_A}","method":"nao_existe","params":{{}}}}"#),
        format!(r#"{{"v":1,"id":"c","token":"{TOK_A}","method":"query","params":[1]}}"#),
        format!(r#"{{"v":1,"id":"c","token":"{TOK_A}","method":"get","params":{{"id":7}}}}"#),
    ];
    for e in &entradas {
        let raw = s.handle_line(e);
        let v = parse(&raw);
        assert_eq!(v["ok"], json!(false), "entrada {e:?} deve dar ok:false");
        assert_eq!(cod(&v), "invalid_params", "entrada {e:?}");
    }

    // linha de requisicao: line_max = 16384 bytes (configuracao de teste). Spec 8A: maior = invalid_params.
    // Controle: exatamente line_max passa (padding de espacos dentro do JSON, valido).
    let base = |pad: usize| {
        format!(
            r#"{{"v":1,"id":"c","token":"{TOK_A}","method":"whoami","params":{{}}{}}}"#,
            " ".repeat(pad)
        )
    };
    let sem_pad = base(0).len();
    let no_limite = base(16384 - sem_pad);
    assert_eq!(no_limite.len(), 16384);
    assert_eq!(
        res(&parse(&s.handle_line(&no_limite)))["agent"],
        json!("agente-a"),
        "exatamente line_max passa"
    );

    let acima = base(16384 - sem_pad + 1);
    assert_eq!(acima.len(), 16385);
    let raw = s.handle_line(&acima);
    assert_eq!(
        cod(&parse(&raw)),
        "invalid_params",
        "line_max + 1 = invalid_params"
    );
    assert!(raw.len() < 16384, "a resposta de erro e curta");

    // controle: uma requisicao valida ainda funciona depois de todas as entradas ruins
    assert_eq!(
        res(&chama(&mut s, TOK_A, "whoami", &json!({})))["agent"],
        json!("agente-a")
    );
}

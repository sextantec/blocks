//! Testes de caixa preta das regras de estado (spec rev5, secao 11): R1, R6, R7, R8, R9, R15, R16, R17.
//! Cada teste sobe o agentd com um tmux FALSO (telas das fixtures reais) e le `agent.get`.
//! Hoje devem FALHAR: o binario e da rev4 (herdr) e nao entende `tmux` nem `rules_file`.

mod rev5_common;

use rev5_common::*;
use serde_json::{Value, json};
use std::time::{Duration, Instant};

/// Um agente com uma tela de fixture (ou sintetica) e titulo; daemon subido com `ajuste`.
fn um(
    kind: &str,
    tela: &[u8],
    titulo: &str,
    poll_ms: u64,
    ajuste: impl FnOnce(&mut Value),
) -> (Mundo, Servidor, &'static str) {
    let (id, tag, cmd) = agente_do_kind(kind);
    let mut m = Mundo::novo();
    m.paineis(&[Painel::new("%1", cmd).tag(tag).titulo(titulo)]);
    m.tela("%1", tela);
    let srv = inicia(&m, poll_ms, &[(id, kind, tag)], ajuste);
    (m, srv, id)
}

/// Amostra `agent.get` durante `d` (a cada 20 ms). Devolve todos os `state` vistos.
fn estados_durante(socket: &std::path::Path, id: &str, d: Duration) -> Vec<String> {
    let fim = Instant::now() + d;
    let mut v = Vec::new();
    loop {
        let a = agente(socket, id);
        v.push(a["state"].as_str().unwrap_or("?").to_string());
        if Instant::now() >= fim {
            return v;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
}

/// Verifica uma entrada `golden` (ou equivalente) contra o daemon. Erro = texto explicando o desvio.
fn verifica_golden(g: &Value) -> Result<(), String> {
    let fx = g["fixture"].as_str().expect("fixture");
    let kind = g["kind"].as_str().expect("kind");
    let (dir, nome) = fx.split_once('/').expect("fixture kind/nome");
    let e = &g["expect"];
    let esperado = e["state"].as_str().expect("expect.state");
    let (id, tag, cmd) = agente_do_kind(kind);
    let mut m = Mundo::novo();
    m.paineis(&[Painel::new("%1", cmd)
        .tag(tag)
        .titulo(&titulo_fixture(dir, nome))]);
    m.tela("%1", &tela_fixture(dir, nome));
    let srv = inicia(&m, 200, &[(id, kind, tag)], |_| {});

    // Espera o estado publicado, sem nunca aceitar `idle` onde nao se espera.
    let fim = Instant::now() + Duration::from_secs(8);
    let a = loop {
        let a = agente(&srv.socket, id);
        if a["state"] == "idle" && esperado != "idle" {
            return Err(format!("publicou idle indevido: {a}"));
        }
        // `observed_at` nulo = nenhuma leitura valida ainda (spec rev4 secao 13.3): nao vale como resultado.
        let lida = !a["observed_at"].is_null();
        let pronto = lida && a["state"] == esperado && !marcas(&a).contains("idle_unconfirmed");
        if pronto {
            break a;
        }
        if Instant::now() >= fim {
            return Err(format!("nao chegou a {esperado} em 8 s; ultimo visto: {a}"));
        }
        std::thread::sleep(Duration::from_millis(20));
    };

    let mut erros = Vec::new();
    if let Some(r) = e.get("rule").filter(|r| a["rule"] != **r) {
        erros.push(format!("rule: esperado {r}, veio {}", a["rule"]));
    }
    if esperado == "blocked" {
        if a["blocked_reason"] != e["reason"] {
            erros.push(format!(
                "blocked_reason: esperado {}, veio {}",
                e["reason"], a["blocked_reason"]
            ));
        }
    } else if !a["blocked_reason"].is_null() {
        erros.push(format!(
            "blocked_reason deveria ser null fora de blocked: {}",
            a["blocked_reason"]
        ));
    }
    let esperadas: std::collections::BTreeSet<String> = e["marks"]
        .as_array()
        .map(|v| {
            v.iter()
                .filter_map(|x| x.as_str().map(String::from))
                .collect()
        })
        .unwrap_or_default();
    if marcas(&a) != esperadas {
        erros.push(format!(
            "marks: esperado {esperadas:?}, veio {:?}",
            marcas(&a)
        ));
    }
    if let Some(inp) = e.get("input").filter(|i| a["input"] != **i) {
        erros.push(format!("input: esperado {inp}, veio {}", a["input"]));
    }
    if erros.is_empty() {
        Ok(())
    } else {
        Err(erros.join("; "))
    }
}

// ---------- R1: golden, todas as fixtures do default.json ----------

#[test]
fn r01_golden_todas_as_fixtures_do_default_json() {
    let gold = goldens();
    let mut falhas = Vec::new();
    for g in &gold {
        if let Err(e) = verifica_golden(g) {
            falhas.push(format!("  {}: {e}", g["fixture"].as_str().unwrap_or("?")));
        }
    }
    assert!(
        falhas.is_empty(),
        "R1: {} de {} fixtures do golden falharam:\n{}",
        falhas.len(),
        gold.len(),
        falhas.join("\n")
    );
}

// ---------- R6: confirmacao de idle (modo polling, window_activity) ----------

#[test]
fn r06_idle_isolado_entre_working_nao_publica_idle() {
    let (mut m, srv, id) = um(
        "claude",
        &tela_fixture("claude", "working-generation"),
        "",
        200,
        |c| {
            c["idle_confirm_ms"] = json!(1000);
        },
    );
    assert!(espera(
        Duration::from_secs(5),
        || agente(&srv.socket, id)["state"] == "working"
    ));
    m.tela_fixture("%1", "claude", "box-empty");
    assert!(
        espera(Duration::from_secs(4), || marcas(&agente(&srv.socket, id))
            .contains("idle_unconfirmed")),
        "R6: leitura idle isolada devia deixar a marca idle_unconfirmed"
    );
    m.tela_fixture("%1", "claude", "working-generation");
    let amostra = estados_durante(&srv.socket, id, Duration::from_secs(2));
    assert!(
        !amostra.iter().any(|s| s == "idle"),
        "R6: idle publicado por uma leitura isolada: {amostra:?}"
    );
}

#[test]
fn r06_idle_confirma_so_depois_de_idle_confirm_ms() {
    let t0 = Instant::now();
    let (_m, srv, id) = um(
        "claude",
        &tela_fixture("claude", "box-empty"),
        "",
        200,
        |c| {
            c["idle_confirm_ms"] = json!(1000);
        },
    );
    let a = espera_estado(&srv.socket, id, "idle", Duration::from_secs(8));
    assert_eq!(a["state"], "idle", "R6: nunca confirmou idle: {a}");
    // O painel ja tinha a tela ociosa antes do daemon subir, entao a primeira leitura ocorre depois de t0.
    assert!(
        t0.elapsed() >= Duration::from_millis(1000),
        "R6: idle confirmou antes de idle_confirm_ms"
    );
    assert_eq!(a["input"], "empty");
}

#[test]
fn r06_polls_1_e_ms_0_confirma_idle_no_primeiro_ciclos() {
    let (_m, srv, id) = um(
        "claude",
        &tela_fixture("claude", "box-empty"),
        "",
        200,
        |c| {
            c["idle_confirm_polls"] = json!(1);
            c["idle_confirm_ms"] = json!(0);
        },
    );
    let a = espera_estado(&srv.socket, id, "idle", Duration::from_millis(1500));
    assert_eq!(
        a["state"], "idle",
        "R6: com polls 1 e ms 0 o idle devia ser imediato: {a}"
    );
}

#[test]
fn r06_janela_zera_quando_window_activity_muda() {
    let (mut m, srv, id) = um(
        "claude",
        &tela_fixture("claude", "box-empty"),
        "",
        200,
        |c| {
            c["idle_confirm_ms"] = json!(800);
        },
    );
    let base = 1_700_000_000u64;
    for i in 0..25u64 {
        m.paineis(&[Painel::new("%1", "claude").tag("claude").ativ(base + i)]);
        let a = agente(&srv.socket, id);
        assert_ne!(
            a["state"], "idle",
            "R6: idle com window_activity mudando (iter {i}): {a}"
        );
        std::thread::sleep(Duration::from_millis(100));
    }
}

#[test]
fn r06_troca_de_pid_zera_a_janela() {
    let (mut m, srv, id) = um(
        "claude",
        &tela_fixture("claude", "box-empty"),
        "",
        200,
        |c| {
            c["idle_confirm_ms"] = json!(800);
        },
    );
    for i in 0..20u64 {
        m.paineis(&[Painel::new("%1", "claude")
            .tag("claude")
            .pid(&format!("{}", 5000 + i))]);
        let a = agente(&srv.socket, id);
        assert_ne!(
            a["state"], "idle",
            "R6: idle com pane_pid mudando (iter {i}): {a}"
        );
        std::thread::sleep(Duration::from_millis(100));
    }
}

#[test]
fn r06_erro_de_leitura_no_meio_zera_a_janela() {
    let (mut m, srv, id) = um(
        "claude",
        &tela_fixture("claude", "box-empty"),
        "",
        200,
        |c| {
            c["idle_confirm_ms"] = json!(1500);
        },
    );
    assert!(espera(Duration::from_secs(4), || marcas(&agente(
        &srv.socket,
        id
    ))
    .contains("idle_unconfirmed")));
    m.listar("falha");
    std::thread::sleep(Duration::from_millis(300));
    m.listar("ok");
    let amostra = estados_durante(&srv.socket, id, Duration::from_millis(1200));
    assert!(
        !amostra.iter().any(|s| s == "idle"),
        "R6: erro no meio da janela nao zerou a confirmacao: {amostra:?}"
    );
    let a = espera_estado(&srv.socket, id, "idle", Duration::from_secs(6));
    assert_eq!(
        a["state"], "idle",
        "R6: depois da janela limpa devia confirmar: {a}"
    );
}

#[test]
fn r06_falso_idle_do_codex_com_spinner_no_titulo_fica_working() {
    let (mut m, srv, id) = um(
        "codex",
        &tela_fixture("codex", "idle"),
        "\u{2820} work-cx1",
        200,
        |c| {
            c["idle_confirm_ms"] = json!(800);
        },
    );
    assert!(espera(
        Duration::from_secs(4),
        || agente(&srv.socket, id)["state"] == "working"
    ));
    let amostra = estados_durante(&srv.socket, id, Duration::from_millis(1500));
    assert!(
        amostra.iter().all(|s| s == "working"),
        "R6 (falso idle do Codex): spinner no titulo devia manter working: {amostra:?}"
    );
    // A tag do agente codex e "codex" (agente_do_kind); o ID do agente e "codex-blocks".
    let (_, tag, cmd) = agente_do_kind("codex");
    m.paineis(&[Painel::new("%1", cmd).tag(tag).titulo("work-cx1")]);
    let a = espera_estado(&srv.socket, id, "idle", Duration::from_secs(6));
    assert_eq!(
        a["state"], "idle",
        "R6: fim do spinner devia virar idle apos a janela: {a}"
    );
}

// ---------- R7: caixa de entrada ----------

#[test]
fn r07_input_e_null_fora_de_idle() {
    let (_m, srv, id) = um(
        "claude",
        &tela_fixture("claude", "working-generation"),
        "",
        200,
        |_| {},
    );
    let a = espera_estado(&srv.socket, id, "working", Duration::from_secs(5));
    assert_eq!(a["state"], "working", "R7: nao chegou a working: {a}");
    assert!(
        a["input"].is_null(),
        "R7: input deve ser null fora de idle: {a}"
    );
}

#[test]
fn r07_input_unknown_quando_o_localizador_falha() {
    // Idle pela regra claude.idle.box, mas sem as duas reguas `^─{20,}$`: a caixa nao se localiza.
    let tela = "\u{276F} \n  \u{23F5}\u{23F5} modo\n".as_bytes().to_vec();
    let (_m, srv, id) = um("claude", &tela, "", 200, |_| {});
    let a = espera_estado(&srv.socket, id, "idle", Duration::from_secs(6));
    assert_eq!(
        a["state"], "idle",
        "R7: a tela sintetica devia ser idle: {a}"
    );
    assert_eq!(
        a["input"], "unknown",
        "R7: localizador sem reguas deve dar unknown: {a}"
    );
}

#[test]
fn r07_mudar_so_input_nao_gera_evento() {
    let (mut m, srv, id) = um(
        "claude",
        &tela_fixture("claude", "box-empty"),
        "",
        200,
        |_| {},
    );
    let a = espera_estado(&srv.socket, id, "idle", Duration::from_secs(6));
    assert_eq!(a["state"], "idle", "R7: sem idle inicial: {a}");
    let head = status(&srv.socket)["head_seq"].as_u64().unwrap();
    m.tela_fixture("%1", "claude", "box-draft");
    let a2 = espera_agente(&srv.socket, id, Duration::from_secs(4), |x| {
        x["input"] == "draft"
    });
    assert_eq!(a2["input"], "draft", "R7: input nao mudou para draft: {a2}");
    std::thread::sleep(Duration::from_millis(400));
    let head2 = status(&srv.socket)["head_seq"].as_u64().unwrap();
    assert_eq!(
        head, head2,
        "R7: mudar so o input gerou evento (head_seq {head} -> {head2})"
    );
    assert_eq!(agente(&srv.socket, id)["state"], "idle");
}

// ---------- R8: marca background ----------

#[test]
fn r08_marca_background_nao_muda_o_estado_e_gera_evento_so_pela_marca() {
    let (mut m, srv, id) = um(
        "claude",
        &tela_fixture("claude", "idle-manual"),
        "",
        200,
        |_| {},
    );
    let a = espera_estado(&srv.socket, id, "idle", Duration::from_secs(6));
    assert_eq!(a["state"], "idle", "R8: idle inicial: {a}");
    assert!(marcas(&a).is_empty(), "R8: sem subagente nao ha marca: {a}");
    let head = status(&srv.socket)["head_seq"].as_u64().unwrap();
    m.tela_fixture("%1", "claude", "background");
    let b = espera_agente(&srv.socket, id, Duration::from_secs(4), |x| {
        marcas(x).contains("background")
    });
    assert!(
        marcas(&b).contains("background"),
        "R8: marca background ausente: {b}"
    );
    assert_eq!(
        b["state"], "idle",
        "R8: a marca nao pode mudar o estado: {b}"
    );
    let evs = eventos_desde(&srv.socket, head);
    let troca = evs.iter().any(|e| {
        e["state"] == "idle"
            && e["previous_state"] == "idle"
            && e["marks"]
                .as_array()
                .map(|v| v.iter().any(|x| x == "background"))
                .unwrap_or(false)
    });
    assert!(
        troca,
        "R8: mudar so a marca devia gerar evento state_changed: {evs:?}"
    );
}

#[test]
fn r08_marca_sai_quando_some_o_subagente() {
    let (mut m, srv, id) = um(
        "claude",
        &tela_fixture("claude", "background"),
        "",
        200,
        |_| {},
    );
    let a = espera_agente(&srv.socket, id, Duration::from_secs(6), |x| {
        marcas(x).contains("background") && x["state"] == "idle"
    });
    assert!(marcas(&a).contains("background"), "R8: {a}");
    m.tela_fixture("%1", "claude", "idle-manual");
    let b = espera_agente(&srv.socket, id, Duration::from_secs(4), |x| {
        !marcas(x).contains("background")
    });
    assert!(
        !marcas(&b).contains("background"),
        "R8: a marca devia sair: {b}"
    );
    assert_eq!(b["state"], "idle");
}

// ---------- R9: precedencia blocked > working > idle ----------

#[test]
fn r09_blocked_vence_working_na_mesma_tela() {
    let mut tela = tela_fixture("claude", "permission-bash");
    tela.extend_from_slice(b"\n  esc to interrupt\n");
    let (_m, srv, id) = um("claude", &tela, "", 200, |_| {});
    let a = espera_estado(&srv.socket, id, "blocked", Duration::from_secs(5));
    assert_eq!(a["state"], "blocked", "R9: {a}");
    assert_eq!(a["blocked_reason"], "permission");
    assert_eq!(a["rule"], "claude.blocked.permission");
}

#[test]
fn r09_titulo_action_required_vence_spinner_do_titulo() {
    let (_m, srv, id) = um(
        "codex",
        &tela_fixture("codex", "idle"),
        "\u{2820} Action Required | work",
        200,
        |_| {},
    );
    let a = espera_estado(&srv.socket, id, "blocked", Duration::from_secs(5));
    assert_eq!(
        a["state"], "blocked",
        "R9: titulo com Action Required devia ser blocked: {a}"
    );
    assert_eq!(a["blocked_reason"], "permission");
    assert_eq!(a["rule"], "codex.blocked.title");
}

#[test]
fn r09_duas_blocked_que_casam_vale_a_reason_da_primeira_do_arquivo() {
    // permission-bash casa claude.blocked.permission e tambem claude.blocked.menu (Esc to cancel nas ultimas 3).
    let (_m, srv, id) = um(
        "claude",
        &tela_fixture("claude", "permission-bash"),
        "",
        200,
        |_| {},
    );
    let a = espera_estado(&srv.socket, id, "blocked", Duration::from_secs(5));
    assert_eq!(a["state"], "blocked", "{a}");
    assert_eq!(
        a["rule"], "claude.blocked.permission",
        "R9: a primeira blocked do arquivo deve vencer: {a}"
    );
    assert_eq!(a["blocked_reason"], "permission");
}

// ---------- R15: regressao dos sinais medidos (uma por linha, com a tela real) ----------

#[test]
fn r15_regressao_dos_sinais_medidos() {
    let casos = [
        (
            "claude",
            "plan",
            "blocked",
            Some("permission"),
            "claude.blocked.plan",
        ),
        (
            "claude",
            "menu-model",
            "blocked",
            Some("user_input"),
            "claude.blocked.menu",
        ),
        (
            "claude",
            "ask-user",
            "blocked",
            Some("user_input"),
            "claude.blocked.menu",
        ),
        (
            "claude",
            "trust",
            "blocked",
            Some("permission"),
            "claude.blocked.trust",
        ),
        (
            "claude",
            "working-narrow",
            "working",
            None,
            "claude.working.footer",
        ),
        (
            "codex",
            "approval-dot",
            "blocked",
            Some("permission"),
            "codex.blocked.title",
        ),
        (
            "codex",
            "approval-bang",
            "blocked",
            Some("permission"),
            "codex.blocked.title",
        ),
        (
            "codex",
            "update-banner",
            "blocked",
            Some("user_input"),
            "codex.blocked.menu",
        ),
        (
            "codex",
            "transcript",
            "blocked",
            Some("user_input"),
            "codex.blocked.menu",
        ),
        ("pi", "narrow-working", "working", None, "pi.working.ruler"),
        (
            "pi",
            "menu-model",
            "blocked",
            Some("user_input"),
            "pi.blocked.menu",
        ),
    ];
    let mut falhas = Vec::new();
    for (kind, nome, estado, motivo, regra) in casos {
        let mut e = json!({"state": estado, "rule": regra, "marks": []});
        if let Some(r) = motivo {
            e["reason"] = json!(r);
        }
        let g = json!({"fixture": format!("{kind}/{nome}"), "kind": kind, "expect": e});
        if let Err(err) = verifica_golden(&g) {
            falhas.push(format!("  {kind}/{nome}: {err}"));
        }
    }
    assert!(
        falhas.is_empty(),
        "R15: sinais medidos que falharam:\n{}",
        falhas.join("\n")
    );
}

// ---------- R16: deriva de versao ----------

#[test]
fn r16_tela_de_outra_versao_nunca_vira_idle() {
    // Mesmas telas ociosas, mas sem a linha de modo (o que uma versao nova poderia mudar).
    let mut casos: Vec<(&str, Vec<u8>)> = Vec::new();
    for (kind, nome) in [
        ("claude", "box-empty"),
        ("claude", "idle-manual"),
        ("codex", "idle"),
    ] {
        let original = String::from_utf8_lossy(&tela_fixture(kind, nome)).into_owned();
        let sem: String = original
            .lines()
            .filter(|l| {
                !(l.contains('\u{23F8}')
                    || l.contains("\u{23F5}\u{23F5}")
                    || l.contains(" \u{00B7} /"))
            })
            .map(|l| format!("{l}\n"))
            .collect();
        casos.push((kind, sem.into_bytes()));
    }
    for (kind, tela) in casos {
        let (id, tag, cmd) = agente_do_kind(kind);
        let mut m = Mundo::novo();
        m.paineis(&[Painel::new("%1", cmd).tag(tag)]);
        m.tela("%1", &tela);
        let srv = inicia(&m, 200, &[(id, kind, tag)], |_| {});
        let amostra = estados_durante(&srv.socket, id, Duration::from_millis(1500));
        assert!(
            !amostra.iter().any(|s| s == "idle"),
            "R16: tela de {kind} sem a linha de modo virou idle: {amostra:?}"
        );
        let a = agente(&srv.socket, id);
        assert_eq!(a["state"], "unknown", "R16 ({kind}): {a}");
        assert!(
            marcas(&a).contains("unmatched"),
            "R16 ({kind}): devia ser unmatched: {a}"
        );
        assert!(a["rule"].is_null(), "R16 ({kind}): rule deve ser null: {a}");
    }
}

// ---------- R17: janela do Enter no Claude ----------

#[test]
fn r17_rascunho_logo_apos_working_nunca_publica_idle() {
    let (mut m, srv, id) = um(
        "claude",
        &tela_fixture("claude", "working-generation"),
        "",
        200,
        |c| {
            c["idle_confirm_ms"] = json!(1000);
        },
    );
    assert!(espera(
        Duration::from_secs(5),
        || agente(&srv.socket, id)["state"] == "working"
    ));
    // Eco do envio: uma tela com rascunho (cabe na regra idle) por uma leitura, depois working de novo.
    m.tela_fixture("%1", "claude", "box-draft");
    std::thread::sleep(Duration::from_millis(300));
    m.tela_fixture("%1", "claude", "working-generation");
    let amostra = estados_durante(&srv.socket, id, Duration::from_millis(2500));
    assert!(
        !amostra.iter().any(|s| s == "idle"),
        "R17: a janela do Enter publicou idle: {amostra:?}"
    );
}

#[test]
fn r17_rascunho_estavel_de_working_confirma_idle_com_input_draft() {
    let (mut m, srv, id) = um(
        "claude",
        &tela_fixture("claude", "working-generation"),
        "",
        200,
        |c| {
            c["idle_confirm_ms"] = json!(1000);
        },
    );
    assert!(espera(
        Duration::from_secs(5),
        || agente(&srv.socket, id)["state"] == "working"
    ));
    m.tela_fixture("%1", "claude", "box-draft");
    let a = espera_estado(&srv.socket, id, "idle", Duration::from_secs(6));
    assert_eq!(
        a["state"], "idle",
        "R17: rascunho estavel devia confirmar idle: {a}"
    );
    assert_eq!(a["input"], "draft", "R17: {a}");
}

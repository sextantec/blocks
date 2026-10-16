//! Testes independentes da rev6 para as regras de fusao (spec-rev6 secao 6), a marca `diverged`, a excecao
//! `screen_override`, a regra 1 (sem fonte estruturada) e a saude da tela (secao 3 e 8). Contrato F1 a F6
//! da secao 11. Caixa preta: relatos pelo `agentd report`, estado por `agent.get`/`status`/eventos, tela
//! pelo tmux FALSO (rev5_common). Nao le `src/`. Ambiguidades: `tests/REV6-TESTES.md`.
//!
//! Tempos: `override_after_s` 5 (faixa minima) em quase todos os testes; `diverge_after_ms` 1000; e um
//! teste com o padrao de 15 s (F2 padrao, ~18 s).
//!
//! Rodar: cargo test --offline -p agentd --test rev6_fusao

mod rev6_common;

use rev6_common::*;
use serde_json::{Value, json};
use std::thread;
use std::time::{Duration, Instant};

/// Mundo com o painel do Claude (`%1`, tag `claude`) e a tela de fixture `nome` do kind `claude`.
fn mundo_claude_fixture(nome: &str) -> Mundo {
    let mut m = Mundo::novo();
    m.paineis(&[Painel::new("%1", "claude").tag("claude").pid("5001")]);
    m.tela_fixture("%1", "claude", nome);
    m
}

fn mundo_claude_tela(tela: &[u8]) -> Mundo {
    let mut m = Mundo::novo();
    m.paineis(&[Painel::new("%1", "claude").tag("claude").pid("5001")]);
    m.tela("%1", tela);
    m
}

/// Sobe o daemon com o agente `claude` (hooks). `ajuste` altera a configuracao antes de subir.
fn sobe_claude(m: &Mundo, ajuste: impl FnOnce(&mut Value)) -> Servidor {
    let mut c = cfg6(
        m,
        json!({"claude": ag_claude("claude")}),
        &m.raiz.path().join("sem-codex.sock"),
    );
    ajuste(&mut c);
    sobe6(m, &c)
}

/// Tempo ate `t` (zero se ja passou), para prazos de espera.
fn ate(t: Instant) -> Duration {
    t.saturating_duration_since(Instant::now())
}

#[test]
fn f1_stop_com_tela_working_publica_working_e_marca_diverged_depois_de_diverge_after_ms() {
    // Relato `idle` (Stop) com a tela mostrando trabalho (regra casa): a tela decide (regra 3 da fusao).
    let mut m = mundo_claude_fixture("working-generation");
    let srv = sobe_claude(&m, |c| c["diverge_after_ms"] = json!(1000));
    let sk = srv.socket.clone();
    espera_observado(&sk, "claude", Duration::from_secs(5));
    report(&m, &sk, Some("%1"), "UserPromptSubmit", b"");
    publica_em(&sk, "claude", "working", SEG);

    let t0 = Instant::now();
    report(&m, &sk, Some("%1"), "Stop", b"");
    dorme_ate(t0 + Duration::from_millis(400));
    let a = agente(&sk, "claude");
    assert_eq!(
        a["state"], "working",
        "relato idle com tela working deve publicar working: {a}"
    );
    assert_eq!(
        a["source"], "tmux",
        "a tela decidiu, a fonte deve ser tmux: {a}"
    );
    assert!(
        !tem_marca(&a, "diverged"),
        "diverged antes de diverge_after_ms: {a}"
    );

    dorme_ate(t0 + Duration::from_millis(700));
    assert!(
        !tem_marca(&agente(&sk, "claude"), "diverged"),
        "diverged antes de diverge_after_ms (700 ms de 1000)"
    );
    let a = espera_agente(
        &sk,
        "claude",
        ate(t0 + Duration::from_millis(1000 + 1500)),
        |a| tem_marca(a, "diverged"),
    );
    assert!(
        tem_marca(&a, "diverged"),
        "diverged nao apareceu depois de diverge_after_ms: {a}"
    );
    assert_eq!(a["state"], "working", "{a}");

    // Controle: a tela passa a mostrar ocioso (box), e o idle da fonte estruturada e a tela concordam.
    m.tela_fixture("%1", "claude", "box-empty");
    let a = publica_em(&sk, "claude", "idle", Duration::from_millis(1500));
    assert!(
        !tem_marca(&a, "diverged"),
        "diverged deveria sair quando as fontes concordam: {a}"
    );
}

#[test]
fn f2_override_com_valor_configurado_segura_working_ate_o_limite_e_depois_publica_idle_com_marca() {
    // Relato working e tela idle POSITIVA (box): so depois de override_after_s (5 s) o idle entra.
    let m = mundo_claude_fixture("box-empty");
    let srv = sobe_claude(&m, |c| c["override_after_s"] = json!(5));
    let sk = srv.socket.clone();
    espera_observado(&sk, "claude", Duration::from_secs(5));

    let t0 = Instant::now();
    report(&m, &sk, Some("%1"), "UserPromptSubmit", b"");
    publica_em(&sk, "claude", "working", SEG);
    dorme_ate(t0 + Duration::from_millis(3500));
    let a = agente(&sk, "claude");
    assert_eq!(
        a["state"], "working",
        "override antes do limite (3,5 s de 5 s): {a}"
    );
    dorme_ate(t0 + Duration::from_millis(4500));
    let a = agente(&sk, "claude");
    assert_eq!(
        a["state"], "working",
        "override antes do limite (4,5 s de 5 s): {a}"
    );
    assert!(!tem_marca(&a, "screen_override"), "{a}");

    let a = espera_estado(
        &sk,
        "claude",
        "idle",
        ate(t0 + Duration::from_millis(5000 + 2500)),
    );
    assert_eq!(
        a["state"], "idle",
        "sem idle depois de override_after_s: {a}"
    );
    assert!(
        tem_marca(&a, "screen_override"),
        "idle sem a marca screen_override: {a}"
    );
}

#[test]
fn f2_override_com_padrao_de_15_s() {
    // Mesmo teste sem `override_after_s` (padrao 15 s): o idle nao entra antes de 15 s.
    let m = mundo_claude_fixture("box-empty");
    let srv = sobe_claude(&m, |_| {});
    let sk = srv.socket.clone();
    espera_observado(&sk, "claude", Duration::from_secs(5));

    let t0 = Instant::now();
    report(&m, &sk, Some("%1"), "UserPromptSubmit", b"");
    publica_em(&sk, "claude", "working", SEG);
    dorme_ate(t0 + Duration::from_secs(10));
    assert_eq!(
        agente(&sk, "claude")["state"],
        "working",
        "override no padrao antes de 10 s"
    );
    dorme_ate(t0 + Duration::from_millis(14000));
    let a = agente(&sk, "claude");
    assert_eq!(
        a["state"], "working",
        "override no padrao antes de 14 s: {a}"
    );
    assert!(!tem_marca(&a, "screen_override"), "{a}");

    let a = espera_estado(&sk, "claude", "idle", ate(t0 + Duration::from_secs(15 + 3)));
    assert_eq!(a["state"], "idle", "sem idle depois de 15 s (padrao): {a}");
    assert!(tem_marca(&a, "screen_override"), "{a}");
}

#[test]
fn f2_sem_regra_idle_na_tela_nao_ha_override() {
    // Tela sem regra (unknown): mesmo com working velho, nao ha override; o proximo relato corrige.
    let m = mundo_claude_tela(TELA_SEM_REGRA);
    let srv = sobe_claude(&m, |c| c["override_after_s"] = json!(5));
    let sk = srv.socket.clone();
    espera_observado(&sk, "claude", Duration::from_secs(5));

    let t0 = Instant::now();
    report(&m, &sk, Some("%1"), "UserPromptSubmit", b"");
    publica_em(&sk, "claude", "working", SEG);
    assert!(
        !ve_estado(&sk, "claude", "idle", ate(t0 + Duration::from_millis(6500))),
        "override sem regra de tela que case idle"
    );
    let a = agente(&sk, "claude");
    assert_eq!(a["state"], "working", "{a}");
    assert!(!tem_marca(&a, "screen_override"), "{a}");
}

#[test]
fn f3_blocked_nunca_sofre_override_e_o_controle_working_com_a_mesma_tela_vira_idle() {
    // Claude em `blocked` e Codex em `working` (estruturada), ambos com tela idle POSITIVA.
    let mut m = Mundo::novo();
    m.paineis(&[
        Painel::new("%1", "claude").tag("claude").pid("5001"),
        Painel::new("%2", "codex").tag("codex-blocks").pid("5002"),
    ]);
    m.tela_fixture("%1", "claude", "box-empty");
    m.tela_fixture("%2", "codex", "idle");
    let fake = FakeCodex::inicia(m.raiz.path(), UUID, json!({"type": "idle"}));
    let tf = thread_file(&m, "thread-codex", UUID.as_bytes(), 0o600);
    let c = {
        let mut c = cfg6(
            &m,
            json!({
                "claude": ag_claude("claude"),
                "codex-blocks": ag_codex("codex-blocks", &tf)
            }),
            &fake.socket,
        );
        c["override_after_s"] = json!(5);
        c
    };
    let srv = sobe6(&m, &c);
    let sk = srv.socket.clone();
    espera_observado(&sk, "claude", Duration::from_secs(5));
    espera_observado(&sk, "codex-blocks", Duration::from_secs(5));
    assert!(espera(Duration::from_secs(2), || fake.conta("thread/read") >= 1));

    report(&m, &sk, Some("%1"), "PermissionRequest", b"");
    publica_em(&sk, "claude", "blocked", SEG);
    fake.muda(json!({"type": "active", "activeFlags": []}));
    publica_em(&sk, "codex-blocks", "working", SEG);

    let t0 = Instant::now();
    while Instant::now() < t0 + Duration::from_secs(7) {
        let a = agente(&sk, "claude");
        assert_eq!(a["state"], "blocked", "blocked sofreu override: {a}");
        assert!(!tem_marca(&a, "screen_override"), "{a}");
        thread::sleep(Duration::from_millis(250));
    }
    // Controle positivo: o Codex, com a mesma tela idle e working por app_server, vira idle com a marca.
    let a = espera_estado(
        &sk,
        "codex-blocks",
        "idle",
        ate(t0 + Duration::from_secs(8)),
    );
    assert_eq!(
        a["state"], "idle",
        "controle: working sem relato novo nao virou idle: {a}"
    );
    assert!(
        tem_marca(&a, "screen_override"),
        "controle: sem screen_override: {a}"
    );
    assert_eq!(
        agente(&sk, "claude")["state"],
        "blocked",
        "claude saiu de blocked"
    );
}

#[test]
fn f4_stop_falso_repetido_nao_tira_working_quando_a_tela_mostra_trabalho() {
    let mut m = mundo_claude_fixture("working-generation");
    let srv = sobe_claude(&m, |_| {});
    let sk = srv.socket.clone();
    espera_observado(&sk, "claude", Duration::from_secs(5));
    report(&m, &sk, Some("%1"), "UserPromptSubmit", b"");
    publica_em(&sk, "claude", "working", SEG);

    for _ in 0..15 {
        report(&m, &sk, Some("%1"), "Stop", b"");
        let a = agente(&sk, "claude");
        assert_ne!(
            a["state"], "idle",
            "um Stop falso tirou o agente de working: {a}"
        );
        thread::sleep(Duration::from_millis(100));
    }
    let eventos = eventos_todos(&sk);
    assert!(
        !eventos.iter().any(|e| e["state"] == "idle"),
        "houve evento state_changed para idle durante os Stops falsos"
    );
    // Controle positivo: quando a tela passa a ocioso, o ultimo Stop (fonte idle) publica idle.
    m.tela_fixture("%1", "claude", "box-empty");
    publica_em(&sk, "claude", "idle", Duration::from_secs(2));
}

#[test]
fn f5_sem_fonte_estruturada_o_comportamento_e_o_da_rev5() {
    // Dois agentes so com tela: nenhuma marca estruturada aparece, a evidencia e `observed`.
    let mut m = Mundo::novo();
    m.paineis(&[
        Painel::new("%1", "claude").tag("claude").pid("5001"),
        Painel::new("%2", "codex").tag("codex-blocks").pid("5002"),
    ]);
    m.tela_fixture("%1", "claude", "box-empty");
    m.tela_fixture("%2", "codex", "idle");
    let c = cfg6(
        &m,
        json!({
            "claude": ag_so_tela("claude", "claude"),
            "codex-blocks": ag_so_tela("codex", "codex-blocks")
        }),
        &m.raiz.path().join("sem-codex.sock"),
    );
    let srv = sobe6(&m, &c);
    let sk = srv.socket.clone();

    let a = publica_em(&sk, "claude", "idle", Duration::from_secs(3));
    assert_eq!(a["source"], "tmux", "{a}");
    assert_eq!(a["evidence"], "observed", "{a}");
    let b = publica_em(&sk, "codex-blocks", "idle", Duration::from_secs(3));
    assert_eq!(b["source"], "tmux", "{b}");

    m.tela_fixture("%1", "claude", "working-generation");
    let a = publica_em(&sk, "claude", "working", SEG * 2);
    assert_eq!(a["source"], "tmux", "{a}");
    m.tela_fixture("%1", "claude", "permission-bash");
    let a = publica_em(&sk, "claude", "blocked", SEG * 2);
    assert_eq!(a["blocked_reason"], "permission", "{a}");
    m.tela_fixture("%2", "codex", "working-indicator");
    publica_em(&sk, "codex-blocks", "working", SEG * 2);

    for id in ["claude", "codex-blocks"] {
        let a = agente(&sk, id);
        for marca in ["structured_lost", "diverged", "screen_override"] {
            assert!(
                !tem_marca(&a, marca),
                "`{id}` ganhou {marca} sem fonte estruturada: {a}"
            );
        }
        assert_eq!(a["evidence"], "observed", "{id}: {a}");
    }
}

#[test]
fn f6_unmatched_ratio_sobe_quando_a_tela_nao_casa_e_o_estado_nao_muda() {
    // Codex com fonte estruturada `working` e tela sem regra: a razao de nao casadas sobe; o estado nao muda.
    // Claude com tela casada (box) e relatos: a razao do kind `claude` fica baixa. Contadores em `status`.
    let mut m = Mundo::novo();
    m.paineis(&[
        Painel::new("%1", "claude").tag("claude").pid("5001"),
        Painel::new("%2", "codex").tag("codex-blocks").pid("5002"),
    ]);
    m.tela_fixture("%1", "claude", "box-empty");
    m.tela("%2", TELA_SEM_REGRA);
    let fake = FakeCodex::inicia(
        m.raiz.path(),
        UUID,
        json!({"type": "active", "activeFlags": []}),
    );
    let tf = thread_file(&m, "thread-codex", UUID.as_bytes(), 0o600);
    let c = cfg6(
        &m,
        json!({
            "claude": ag_claude("claude"),
            "codex-blocks": ag_codex("codex-blocks", &tf)
        }),
        &fake.socket,
    );
    let srv = sobe6(&m, &c);
    let sk = srv.socket.clone();
    espera_observado(&sk, "claude", Duration::from_secs(5));
    publica_em(&sk, "codex-blocks", "working", Duration::from_secs(5));
    report(&m, &sk, Some("%1"), "UserPromptSubmit", b"");
    publica_em(&sk, "claude", "working", SEG);

    let t0 = Instant::now();
    while Instant::now() < t0 + Duration::from_secs(2) {
        assert_eq!(
            agente(&sk, "codex-blocks")["state"],
            "working",
            "a tela sem regra mudou o estado"
        );
        thread::sleep(Duration::from_millis(100));
    }
    // Forma literal da secao 3: `screen.reads`, `screen.unmatched` e `screen.unmatched_ratio`, cada um por kind.
    // Contadores da secao 3 (`screen.reads`, `screen.unmatched_ratio`, por kind). A localizacao nao e
    // fixada: a secao 3 diz "em status" e a secao 8 lista `screen` junto de `sources.*`. Aceita-se
    // `status.screen` ou `status.sources.screen` (ver REV6-TESTES.md).
    let st = status(&sk);
    let tela = if st["screen"].is_null() {
        st["sources"]["screen"].clone()
    } else {
        st["screen"].clone()
    };
    let r_codex = tela["unmatched_ratio"]["codex"].as_f64().unwrap_or(-1.0);
    let r_claude = tela["unmatched_ratio"]["claude"].as_f64().unwrap_or(-1.0);
    assert!(
        tela["reads"]["codex"].as_u64().unwrap_or(0) >= 5,
        "poucas leituras de tela no kind codex: {st}"
    );
    assert!(
        r_codex > 0.5,
        "razao de nao casadas do codex deveria subir: {st}"
    );
    assert!(
        r_claude >= 0.0 && r_claude < 0.5,
        "razao de nao casadas do claude (tela casada) deveria ser baixa: {st}"
    );
}

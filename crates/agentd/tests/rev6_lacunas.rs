//! Segunda rodada de testes independentes da rev6 (lacunas achadas pela mutacao). Caixa preta: relatos
//! pelo `agentd report`, tela pelo tmux FALSO (rev5_common), estado por `agent.get`. Nao le `src/`.
//! Spec: `docs/spec-rev6.md` secao 6, regras 4 e 5, e secao 7 (`override_after_s`, `idle_confirm_ms` da rev5).
//! Ambiguidades: `tests/REV6-TESTES.md` (A24 a A26).
//!
//! Rodar: cargo test --offline -p agentd --test rev6_lacunas

mod rev6_common;

use rev6_common::*;
use serde_json::json;
use std::thread;
use std::time::{Duration, Instant};

/// Tempo ate `t` (zero se ja passou).
fn ate(t: Instant) -> Duration {
    t.saturating_duration_since(Instant::now())
}

/// Sobe o daemon com o agente `claude` (hooks), com `ajuste` aplicado na configuracao.
fn sobe_claude(m: &Mundo, ajuste: impl FnOnce(&mut serde_json::Value)) -> Servidor {
    let mut c = cfg6(
        m,
        json!({"claude": ag_claude("claude")}),
        &m.raiz.path().join("sem-codex.sock"),
    );
    ajuste(&mut c);
    sobe6(m, &c)
}

#[test]
fn l1_override_conta_a_idade_do_ultimo_relato_working_nao_so_a_tela() {
    // Tela `box-empty` (idle positiva) desde o boot. Relatos `working` repetidos a cada 1 s por 8 s,
    // bem acima de override_after_s = 5: o estado nao pode virar `idle`, nem ganhar screen_override.
    let mut m = Mundo::novo();
    m.paineis(&[Painel::new("%1", "claude").tag("claude").pid("5001")]);
    m.tela_fixture("%1", "claude", "box-empty");
    let srv = sobe_claude(&m, |c| c["override_after_s"] = json!(5));
    let sk = srv.socket.clone();
    espera_observado(&sk, "claude", Duration::from_secs(5));

    report(&m, &sk, Some("%1"), "UserPromptSubmit", b"");
    publica_em(&sk, "claude", "working", SEG);
    for rodada in 0..8 {
        thread::sleep(Duration::from_millis(1000));
        report(&m, &sk, Some("%1"), "UserPromptSubmit", b"");
        let a = agente(&sk, "claude");
        assert_eq!(
            a["state"], "working",
            "rodada {rodada}: relato working recente virou outro estado (override pela tela): {a}"
        );
        assert!(!tem_marca(&a, "screen_override"), "rodada {rodada}: {a}");
    }

    // Controle positivo: parados os relatos, passados override_after_s, vira idle com screen_override.
    let t_ultimo = Instant::now();
    dorme_ate(t_ultimo + Duration::from_millis(3500));
    assert_eq!(
        agente(&sk, "claude")["state"],
        "working",
        "override antes de override_after_s"
    );
    let a = espera_estado(
        &sk,
        "claude",
        "idle",
        ate(t_ultimo + Duration::from_millis(5000 + 3000)),
    );
    assert_eq!(
        a["state"], "idle",
        "sem idle depois de override_after_s sem relatos: {a}"
    );
    assert!(
        tem_marca(&a, "screen_override"),
        "idle sem screen_override: {a}"
    );
}

#[test]
fn l2_override_conta_a_tela_idle_desde_que_ela_ficou_idle_nao_o_relato() {
    // Relato `working` velho (6,5 s, acima de override_after_s = 5). Tela `working` ate t_virada,
    // depois `idle` positiva. O override exige tela idle ininterrupta por 5 s contados de t_virada.
    let mut m = Mundo::novo();
    m.paineis(&[Painel::new("%1", "claude").tag("claude").pid("5001")]);
    m.tela_fixture("%1", "claude", "working-generation");
    let srv = sobe_claude(&m, |c| c["override_after_s"] = json!(5));
    let sk = srv.socket.clone();
    espera_observado(&sk, "claude", Duration::from_secs(5));

    report(&m, &sk, Some("%1"), "UserPromptSubmit", b"");
    publica_em(&sk, "claude", "working", SEG);
    thread::sleep(Duration::from_millis(6500));
    assert_eq!(
        agente(&sk, "claude")["state"],
        "working",
        "tela working nao pode virar idle"
    );

    m.tela_fixture("%1", "claude", "box-empty");
    let t_virada = Instant::now();
    dorme_ate(t_virada + Duration::from_millis(1000));
    let a = agente(&sk, "claude");
    assert_eq!(
        a["state"], "working",
        "a tela acabou de ficar idle e o relato ja era velho: override indevido: {a}"
    );
    assert!(!tem_marca(&a, "screen_override"), "{a}");

    dorme_ate(t_virada + Duration::from_millis(3500));
    let a = agente(&sk, "claude");
    assert_eq!(
        a["state"], "working",
        "tela idle ha 3,5 s < 5 s: ainda nao ha override: {a}"
    );

    // Controle positivo: depois de 5 s de tela idle, o mesmo cenario termina em idle com screen_override.
    let a = espera_estado(
        &sk,
        "claude",
        "idle",
        ate(t_virada + Duration::from_millis(5000 + 3000)),
    );
    assert_eq!(
        a["state"], "idle",
        "sem override depois de 5 s de tela idle: {a}"
    );
    assert!(tem_marca(&a, "screen_override"), "{a}");
    assert!(
        Instant::now() >= t_virada + Duration::from_millis(4500),
        "idle observado antes de 4,5 s de tela idle"
    );
}

#[test]
fn l3_relato_idle_logo_apos_troca_de_pid_nao_publica_idle_antes_da_segunda_leitura() {
    // Esclarecimento rev6.2 (regra 4): depois da troca do par (pane_id, pane_pid), o idle exige o MESMO par em
    // DUAS leituras validas consecutivas (a leitura em que o par mudou conta como a primeira), sem
    // idle_confirm_ms. poll_ms = 1000 torna a janela negativa mensuravel; idle_confirm_ms = 5000 prova que
    // ele nao e exigido (o idle tem de aparecer bem antes de 5 s da troca).
    let mut m = Mundo::novo();
    m.paineis(&[Painel::new("%1", "claude").tag("claude").pid("5001")]);
    m.tela("%1", TELA_SEM_REGRA);
    let srv = sobe_claude(&m, |c| {
        c["poll_ms"] = json!(1000);
        c["idle_confirm_ms"] = json!(5000);
    });
    let sk = srv.socket.clone();
    espera_observado(&sk, "claude", Duration::from_secs(5));
    thread::sleep(Duration::from_millis(2200));
    // Controle A: com o par antigo ja lido duas vezes, o relato idle publica idle (sem esperar idle_confirm_ms).
    report(&m, &sk, Some("%1"), "Stop", b"");
    publica_em(&sk, "claude", "idle", Duration::from_secs(2));
    // Estado nao-idle antes da troca: assim um `idle` antigo nao mascara o teste.
    report(&m, &sk, Some("%1"), "UserPromptSubmit", b"");
    publica_em(&sk, "claude", "working", SEG);

    // Troca de pane_pid no mesmo pane_id. A primeira leitura com o par novo e detectada pelo proprio tmux
    // falso: o primeiro `list-panes` depois da troca (contador de chamadas). O estado nao e usado como sinal:
    // com o relato em working, a rev6 (regra 2) mantem working, e a troca nao aparece no estado.
    let n0 = m.listagens();
    m.paineis(&[Painel::new("%1", "claude").tag("claude").pid("6001")]);
    let t_troca = Instant::now();
    assert!(
        espera(Duration::from_millis(2500), || m.listagens() > n0),
        "nenhuma leitura do tmux depois da troca de pane_pid"
    );
    let t_primeira = Instant::now();
    thread::sleep(Duration::from_millis(30));

    // Relato idle logo depois da primeira leitura do par novo, antes da segunda: nao pode publicar idle.
    report(&m, &sk, Some("%1"), "Stop", b"");
    assert!(
        !ve_estado(&sk, "claude", "idle", Duration::from_millis(800)),
        "relato idle antes da segunda leitura do par novo publicou idle"
    );

    // Controle positivo: depois da segunda leitura estavel (poll 1 s), o idle aparece, bem antes de 5 s.
    dorme_ate(t_primeira + Duration::from_millis(1400));
    report(&m, &sk, Some("%1"), "Stop", b"");
    let a = publica_em(
        &sk,
        "claude",
        "idle",
        ate(t_troca + Duration::from_millis(4000)),
    );
    assert_eq!(a["state"], "idle", "{a}");
}

#[test]
fn b1_tela_com_blocked_rebaixa_working_do_relato_e_volta_quando_a_tela_sai() {
    // Esclarecimento rev6.2 (regra 2): E working + T blocked (regra positiva de blocked da tela) publica
    // blocked com blocked_reason da tela, source tmux, de imediato. Quando a tela deixa de mostrar blocked,
    // volta a valer E. O sentido inverso nao existe: E blocked nunca e rebaixado pela tela.
    // Fixture `claude/permission-bash` (regra claude.blocked.permission, usada pelos testes rev5).
    let mut m = Mundo::novo();
    m.paineis(&[Painel::new("%1", "claude").tag("claude").pid("5001")]);
    m.tela_fixture("%1", "claude", "box-empty");
    let srv = sobe_claude(&m, |_| {});
    let sk = srv.socket.clone();
    espera_observado(&sk, "claude", Duration::from_secs(5));

    report(&m, &sk, Some("%1"), "UserPromptSubmit", b"");
    publica_em(&sk, "claude", "working", SEG);

    // A tela passa a mostrar a aprovacao (blocked pela tela), com o relato ainda working.
    m.tela_fixture("%1", "claude", "permission-bash");
    let a = publica_em(&sk, "claude", "blocked", SEG);
    assert_eq!(
        a["blocked_reason"], "permission",
        "blocked_reason deve vir da regra da tela: {a}"
    );
    assert_eq!(
        a["source"], "tmux",
        "blocked pela tela deve ter source tmux: {a}"
    );
    assert_eq!(a["evidence"], "observed", "{a}");

    // A tela volta a ocioso: vale de novo o relato working.
    m.tela_fixture("%1", "claude", "box-empty");
    let a = publica_em(&sk, "claude", "working", SEG * 2);
    assert_eq!(
        a["source"], "hooks",
        "voltou a valer o relato (fonte hooks): {a}"
    );

    // Controle: blocked do relato (PermissionRequest) com tela idle positiva NAO e rebaixado pela tela.
    report(&m, &sk, Some("%1"), "PermissionRequest", b"");
    let a = publica_em(&sk, "claude", "blocked", SEG);
    assert_eq!(a["blocked_reason"], "permission", "{a}");
    assert_eq!(a["source"], "hooks", "{a}");
    let t0 = Instant::now();
    while Instant::now() < t0 + Duration::from_millis(2500) {
        let a = agente(&sk, "claude");
        assert_eq!(
            a["state"], "blocked",
            "tela idle rebaixou blocked do relato: {a}"
        );
        assert!(!tem_marca(&a, "screen_override"), "{a}");
        thread::sleep(Duration::from_millis(100));
    }
}

#[test]
fn b1_controle_tela_com_blocked_sem_fonte_estruturada_publica_blocked() {
    // Controle da B1: a mesma tela `permission-bash`, sem relato (so tmux), publica blocked/permission.
    // Se este teste falhar, o problema e do harness ou da regra da tela, nao da precedencia E x T.
    let mut m = Mundo::novo();
    m.paineis(&[Painel::new("%1", "claude").tag("claude").pid("5001")]);
    m.tela_fixture("%1", "claude", "permission-bash");
    let c = cfg6(
        &m,
        json!({"claude": ag_so_tela("claude", "claude")}),
        &m.raiz.path().join("sem-codex.sock"),
    );
    let srv = sobe6(&m, &c);
    let sk = srv.socket.clone();
    let a = publica_em(&sk, "claude", "blocked", Duration::from_secs(3));
    assert_eq!(a["blocked_reason"], "permission", "{a}");
    assert_eq!(a["source"], "tmux", "{a}");
}

#[test]
fn b2_relato_blocked_nao_e_rebaixado_pela_tela_blocked_de_outra_classe() {
    // Regra 2 (rev6.2): E `blocked` e publicado como E e nunca e rebaixado pela tela. Aqui a tela
    // mostra blocked de OUTRA classe (`claude/ask-user`, user_input); o relato e permission (hooks).
    // Controle no mesmo daemon: agente `claude-tela`, so tela com `ask-user`, publica blocked/user_input
    // com source tmux.
    let mut m = Mundo::novo();
    m.paineis(&[
        Painel::new("%1", "claude").tag("claude").pid("5001"),
        Painel::new("%2", "claude").tag("claude-tela").pid("5002"),
    ]);
    m.tela_fixture("%1", "claude", "box-empty");
    m.tela_fixture("%2", "claude", "ask-user");
    let c = cfg6(
        &m,
        json!({
            "claude": ag_claude("claude"),
            "claude-tela": ag_so_tela("claude", "claude-tela")
        }),
        &m.raiz.path().join("sem-codex.sock"),
    );
    let srv = sobe6(&m, &c);
    let sk = srv.socket.clone();
    espera_observado(&sk, "claude", Duration::from_secs(5));

    // Controle: so a tela, sem relato, publica blocked de user_input com source tmux.
    let a = publica_em(&sk, "claude-tela", "blocked", Duration::from_secs(3));
    assert_eq!(a["blocked_reason"], "user_input", "controle: {a}");
    assert_eq!(a["source"], "tmux", "controle: {a}");

    // Relato PermissionRequest: blocked/permission pelo relato.
    report(&m, &sk, Some("%1"), "PermissionRequest", b"");
    let a = publica_em(&sk, "claude", "blocked", SEG);
    assert_eq!(a["blocked_reason"], "permission", "{a}");
    assert_eq!(a["source"], "hooks", "{a}");

    // A tela do agente de relato passa a mostrar blocked de outra classe (user_input).
    m.tela_fixture("%1", "claude", "ask-user");
    let t0 = Instant::now();
    while Instant::now() < t0 + Duration::from_millis(2500) {
        let a = agente(&sk, "claude");
        assert_eq!(
            a["state"], "blocked",
            "a tela rebaixou o blocked do relato: {a}"
        );
        assert_eq!(
            a["blocked_reason"], "permission",
            "a tela trocou o motivo do relato: {a}"
        );
        assert_eq!(a["source"], "hooks", "a tela assumiu a decisao: {a}");
        thread::sleep(Duration::from_millis(100));
    }
}

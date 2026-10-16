//! Testes independentes da rev6 para a fonte `claude_hooks` (spec-rev6 secoes 0, 2, 5, 6, 8 e o contrato
//! H1 a H6 da secao 11). Caixa preta: o relato entra pelo binario `agentd report EVENTO` com `TMUX_PANE`
//! e `AGENTD_SOCKET` controlados; o estado se le por `agent.get`, `status` e eventos. Tmux FALSO (rev5_common).
//! Nao le `src/`, nao chama o Claude real, nao toca `~/.claude`, `~/.config` ou a rede.
//! Ambiguidades e suposicoes: `tests/REV6-TESTES.md`.
//!
//! Rodar: cargo test --offline -p agentd --test rev6_hooks

mod rev6_common;

use rev6_common::*;
use serde_json::json;
use std::io::Read;
use std::os::unix::net::UnixListener;
use std::thread;
use std::time::{Duration, Instant};

/// Mundo com o painel do Claude (`%1`, tag `claude`) e a tela `tela`.
fn mundo_claude(tela: &[u8]) -> Mundo {
    let mut m = Mundo::novo();
    m.paineis(&[Painel::new("%1", "claude").tag("claude").pid("5001")]);
    m.tela("%1", tela);
    m
}

/// Sobe o daemon com o agente `claude` (fonte tmux + claude_hooks). Socket de codex inexistente: nao usado.
fn sobe_claude(m: &Mundo) -> Servidor {
    let c = cfg6(
        m,
        json!({"claude": ag_claude("claude")}),
        &m.raiz.path().join("sem-codex.sock"),
    );
    sobe6(m, &c)
}

/// Um relato valido do painel `%1` (ou de outro painel) ja com o prazo restante de 1 s a partir de `inicio`.
fn restante(inicio: Instant) -> Duration {
    SEG.saturating_sub(inicio.elapsed())
}

#[test]
fn h1_userpromptsubmit_working_e_stop_idle_com_tela_desconhecida_em_menos_de_1_s() {
    // Tela sem regra (o caso do Claude 2.1.302: a tela nao casa nada). O Stop tem de publicar idle.
    let m = mundo_claude(TELA_SEM_REGRA);
    let srv = sobe_claude(&m);
    let sk = srv.socket.clone();
    espera_observado(&sk, "claude", Duration::from_secs(5));

    let inicio = Instant::now();
    let r = report(&m, &sk, Some("%1"), "UserPromptSubmit", b"");
    assert_eq!(r.saida.code, Some(0), "report saiu com erro: {:?}", r.saida);
    let a = publica_em(&sk, "claude", "working", restante(inicio));
    assert_eq!(a["source"], "hooks", "{a}");
    assert_eq!(
        a["evidence"], "reported",
        "tela sem regra nao e concordancia: {a}"
    );

    let inicio = Instant::now();
    let r = report(&m, &sk, Some("%1"), "Stop", b"");
    assert_eq!(r.saida.code, Some(0));
    let a = publica_em(&sk, "claude", "idle", restante(inicio));
    assert_eq!(a["source"], "hooks", "{a}");
    assert!(a["blocked_reason"].is_null(), "{a}");
}

#[test]
fn h2_permissionrequest_vira_blocked_permission_e_elicitation_user_input() {
    let m = mundo_claude(TELA_SEM_REGRA);
    let srv = sobe_claude(&m);
    let sk = srv.socket.clone();
    espera_observado(&sk, "claude", Duration::from_secs(5));
    report(&m, &sk, Some("%1"), "UserPromptSubmit", b"");
    publica_em(&sk, "claude", "working", SEG);

    let inicio = Instant::now();
    report(&m, &sk, Some("%1"), "PermissionRequest", b"");
    let a = publica_em(&sk, "claude", "blocked", restante(inicio));
    assert_eq!(a["blocked_reason"], "permission", "{a}");
    assert_eq!(a["source"], "hooks", "{a}");

    let inicio = Instant::now();
    report(&m, &sk, Some("%1"), "Elicitation", b"");
    let a = publica_em(&sk, "claude", "blocked", restante(inicio));
    assert_eq!(a["blocked_reason"], "user_input", "{a}");
    assert_eq!(a["source"], "hooks", "{a}");
}

#[test]
fn h3_relato_de_painel_nao_configurado_e_descartado_e_contado_como_rejeitado() {
    // %8 roda `claude` mas NAO e o painel configurado do agente (descoberta desligada, sem marca).
    let mut m = Mundo::novo();
    m.paineis(&[
        Painel::new("%1", "claude").tag("claude").pid("5001"),
        Painel::new("%8", "claude").pid("5008"),
    ]);
    m.tela("%1", TELA_SEM_REGRA);
    m.tela("%8", TELA_SEM_REGRA);
    let srv = sobe_claude(&m);
    let sk = srv.socket.clone();
    espera_observado(&sk, "claude", Duration::from_secs(5));
    let (aceitos0, rejeitados0) = relatos(&sk);

    let r = report(&m, &sk, Some("%8"), "UserPromptSubmit", b"");
    assert_eq!(r.saida.code, Some(0));
    assert!(
        !ve_estado(&sk, "claude", "working", Duration::from_millis(800)),
        "relato de painel nao configurado mudou o estado"
    );
    let (aceitos1, rejeitados1) = relatos(&sk);
    assert_eq!(
        rejeitados1,
        rejeitados0 + 1,
        "relato rejeitado nao foi contado"
    );
    assert_eq!(
        aceitos1, aceitos0,
        "relato de painel nao configurado foi aceito"
    );

    // Controle positivo: o mesmo evento vindo do painel configurado muda o estado e conta como aceito.
    report(&m, &sk, Some("%1"), "UserPromptSubmit", b"");
    publica_em(&sk, "claude", "working", SEG);
    let (aceitos2, _) = relatos(&sk);
    assert_eq!(
        aceitos2,
        aceitos1 + 1,
        "relato do painel configurado nao foi contado como aceito"
    );
}

#[test]
fn h4_evento_fora_da_lista_e_descartado() {
    let m = mundo_claude(TELA_SEM_REGRA);
    let srv = sobe_claude(&m);
    let sk = srv.socket.clone();
    espera_observado(&sk, "claude", Duration::from_secs(5));

    // Nomes que o Claude tem, mas que a lista fechada da rev6 nao aceita (e um nome em minusculas).
    for evento in ["PreToolUse", "Notification", "userpromptsubmit"] {
        let r = report(&m, &sk, Some("%1"), evento, b"");
        assert_eq!(r.saida.code, Some(0), "{evento}: report deve sair 0");
        assert!(
            !ve_estado(&sk, "claude", "working", Duration::from_millis(500)),
            "evento fora da lista ({evento}) mudou o estado"
        );
    }
    // Controle positivo: um evento da lista, no mesmo daemon, muda o estado.
    report(&m, &sk, Some("%1"), "UserPromptSubmit", b"");
    publica_em(&sk, "claude", "working", SEG);
}

#[test]
fn h5_report_sem_daemon_sai_0_em_menos_de_1_s_sem_saida_padrao() {
    let m = mundo_claude(TELA_SEM_REGRA);
    let nao_existe = m.raiz.path().join("nao-existe.sock");
    for pane in [Some("%1"), None] {
        let r = report(&m, &nao_existe, pane, "UserPromptSubmit", b"");
        assert_eq!(
            r.saida.code,
            Some(0),
            "pane {pane:?}: sem daemon deve sair 0 ({:?})",
            r.saida
        );
        assert!(
            r.duracao < SEG,
            "pane {pane:?}: report demorou {:?} sem daemon",
            r.duracao
        );
        assert!(
            r.saida.stdout.is_empty(),
            "pane {pane:?}: escreveu na saida padrao: {:?}",
            r.saida.stdout
        );
    }
}

#[test]
fn h5_report_com_socket_mudo_respeita_o_timeout_e_so_envia_evento_e_painel() {
    // Servidor de socket que aceita e NAO responde: o report precisa sair 0 dentro do timeout configurado.
    // Tambem registra o que o report envia: o evento e o painel, nunca o payload do stdin.
    let m = mundo_claude(TELA_SEM_REGRA);
    let sock = m.raiz.path().join("mudo.sock");
    let ouvinte = UnixListener::bind(&sock).expect("bind do socket mudo");
    let (tx, rx) = std::sync::mpsc::channel::<String>();
    thread::spawn(move || {
        for c in ouvinte.incoming() {
            let Ok(mut st) = c else { continue };
            let _ = st.set_read_timeout(Some(Duration::from_secs(3)));
            let mut recebido = Vec::new();
            let mut b = [0u8; 4096];
            loop {
                match st.read(&mut b) {
                    Ok(0) | Err(_) => break,
                    Ok(n) => {
                        recebido.extend_from_slice(&b[..n]);
                        if recebido.contains(&b'\n') {
                            break;
                        }
                    }
                }
            }
            let _ = tx.send(String::from_utf8_lossy(&recebido).into_owned());
        }
    });
    let segredo = sentinela("hook-stdin");
    let payload = format!("{{\"prompt\":\"{segredo}\",\"last_assistant_message\":\"{segredo}\"}}");
    let r = report_com(
        &m,
        &sock,
        Some("%1"),
        "Stop",
        payload.as_bytes(),
        Some("200"),
    );
    assert_eq!(r.saida.code, Some(0), "{:?}", r.saida);
    assert!(
        r.duracao < SEG,
        "report com socket mudo demorou {:?}",
        r.duracao
    );
    assert!(r.saida.stdout.is_empty());
    assert!(
        !r.saida.stderr.contains(&segredo),
        "sentinela do stdin apareceu no stderr"
    );
    let enviado = rx
        .recv_timeout(Duration::from_secs(3))
        .expect("o report nao enviou nada ao socket");
    assert!(
        enviado.contains("Stop"),
        "o evento nao foi enviado: {enviado}"
    );
    assert!(
        enviado.contains("%1"),
        "o painel nao foi enviado: {enviado}"
    );
    assert!(
        !enviado.contains(&segredo),
        "o payload do stdin foi enviado ao daemon: {enviado}"
    );
}

#[test]
fn h5_stdin_do_hook_e_ignorado_mesmo_com_daemon_no_ar() {
    let m = mundo_claude(TELA_SEM_REGRA);
    let srv = sobe_claude(&m);
    let sk = srv.socket.clone();
    espera_observado(&sk, "claude", Duration::from_secs(5));
    let segredo = sentinela("stdin-do-hook");
    let payload =
        format!("{{\"prompt\":\"{segredo}\",\"tool_input\":{{\"command\":\"{segredo}\"}}}}");
    let r = report(&m, &sk, Some("%1"), "UserPromptSubmit", payload.as_bytes());
    assert_eq!(r.saida.code, Some(0));
    // Controle positivo: o relato com esse stdin foi aceito e o estado mudou.
    publica_em(&sk, "claude", "working", SEG);
    let texto = dump_daemon(&srv, &sk) + &dump_cli(&m, &sk, &["claude"]) + &todo_texto(&r.saida);
    assert!(
        !texto.contains(&segredo),
        "o stdin do hook vazou para uma saida"
    );
}

#[test]
fn h6_sessionend_com_painel_que_some_vira_gone_so_depois_de_gone_after_s() {
    let mut m = mundo_claude(TELA_SEM_REGRA);
    let srv = sobe_claude(&m);
    let sk = srv.socket.clone();
    espera_observado(&sk, "claude", Duration::from_secs(5));
    report(&m, &sk, Some("%1"), "UserPromptSubmit", b"");
    publica_em(&sk, "claude", "working", SEG);

    // O SessionEnd chega com o painel ainda presente (relato aceito).
    let (aceitos0, _) = relatos(&sk);
    let r = report(&m, &sk, Some("%1"), "SessionEnd", b"");
    assert_eq!(r.saida.code, Some(0));
    assert_eq!(
        relatos(&sk).0,
        aceitos0 + 1,
        "SessionEnd com painel presente nao foi aceito"
    );

    // O painel some (o Claude saiu e sobrou o shell): regra 1b, gone so depois de gone_after_s (5 s).
    m.paineis(&[]);
    let t_some = Instant::now();
    thread::sleep(Duration::from_millis(2000));
    let a = agente(&sk, "claude");
    assert_ne!(a["state"], "gone", "gone antes de gone_after_s: {a}");
    assert!(
        tem_marca(&a, "error"),
        "painel ausente sem marca error: {a}"
    );
    // gone_after_s = 5 s; folga de 2,5 s (um ciclo de tmux e o atraso aceito na spec 15.9).
    publica_em(
        &sk,
        "claude",
        "gone",
        Duration::from_millis(5000 + 2500).saturating_sub(t_some.elapsed()),
    );
}

//! Testes independentes da rev6 para a configuracao das chaves de tempo (F2b, secao 7), a privacidade
//! (P1, secao 9) e a lista fechada do tmux (P2, regressao R3 da rev5). Caixa preta: o binario `agentd`
//! e o tmux FALSO (rev5_common); o app-server e o WebSocket FALSO (rev6_common). Nao le `src/`.
//! Ambiguidades e suposicoes: `tests/REV6-TESTES.md`.
//!
//! Rodar: cargo test --offline -p agentd --test rev6_config_privacidade

mod rev6_common;

use rev6_common::*;
use serde_json::{Value, json};
use std::thread;
use std::time::Duration;

/// Chaves de tempo da tabela da secao 7 (chave, minimo, maximo).
const TEMPO: &[(&str, u64, u64)] = &[
    ("override_after_s", 5, 300),
    ("diverge_after_ms", 1000, 60000),
    ("hooks_silence_s", 60, 3600),
    ("thread_file_reread_s", 5, 600),
    ("codex_reconnect_min_s", 1, 30),
    ("codex_reconnect_max_s", 1, 300),
    ("screen_health_window_s", 60, 3600),
];

/// Configuracao valida de referencia (claude com hooks e codex com app_server). Controle de todo teste F2b.
fn base(m: &Mundo) -> Value {
    let tf = thread_file(m, "thread-f2b", UUID.as_bytes(), 0o600);
    cfg6(
        m,
        json!({"claude": ag_claude("claude"), "codex-blocks": ag_codex("codex-blocks", &tf)}),
        &m.raiz.path().join("app-server-control.sock"),
    )
}

/// Espera que a recusa seja de saida 1 (o daemon nao sobe). Outro codigo ou aceite falha o teste.
fn exige_recusa(m: &Mundo, nome: &str, c: &Value) {
    match tenta_subir(m, nome, c) {
        Boot::Subiu => panic!("`{nome}`: configuracao deveria ser recusada e o daemon subiu"),
        Boot::Recusou(code, err) => assert_eq!(
            code,
            Some(1),
            "`{nome}`: saida deveria ser 1; stderr: {err}"
        ),
    }
}

#[test]
fn f2b_chaves_de_tempo_sao_aceitas_nos_extremos_da_faixa() {
    let m = Mundo::novo();
    // Controle: a configuracao de referencia sobe.
    assert!(
        matches!(tenta_subir(&m, "base", &base(&m)), Boot::Subiu),
        "controle: a configuracao valida de referencia foi recusada"
    );
    for (chave, min, max) in TEMPO {
        for v in [*min, *max] {
            let mut c = base(&m);
            c[*chave] = json!(v);
            match tenta_subir(&m, &format!("{chave}-{v}"), &c) {
                Boot::Subiu => {}
                Boot::Recusou(code, err) => panic!(
                    "{chave}={v} deveria ser aceito (faixa {min}..{max}); saiu {code:?}: {err}"
                ),
            }
        }
    }
    // Limites combinados: minimo e maximo de reconexao juntos, com min == max.
    let mut c = base(&m);
    c["codex_reconnect_min_s"] = json!(30);
    c["codex_reconnect_max_s"] = json!(30);
    assert!(
        matches!(tenta_subir(&m, "reconnect-30-30", &c), Boot::Subiu),
        "min == max == 30 deveria subir"
    );
}

#[test]
fn f2b_valores_zero_abaixo_e_acima_da_faixa_sao_recusados() {
    let m = Mundo::novo();
    assert!(
        matches!(tenta_subir(&m, "base-2", &base(&m)), Boot::Subiu),
        "controle"
    );
    for (chave, min, max) in TEMPO {
        let mut fora: Vec<u64> = vec![0, max + 1];
        if *min > 1 {
            fora.push(min - 1);
        }
        for v in fora {
            let mut c = base(&m);
            c[*chave] = json!(v);
            exige_recusa(&m, &format!("fora-{chave}-{v}"), &c);
        }
    }
}

#[test]
fn f2b_codex_reconnect_min_maior_que_max_e_recusado() {
    let m = Mundo::novo();
    // Controle: min <= max sobe (min igual a max tambem).
    for (mn, mx) in [(5u64, 10u64), (5, 5)] {
        let mut c = base(&m);
        c["codex_reconnect_min_s"] = json!(mn);
        c["codex_reconnect_max_s"] = json!(mx);
        assert!(
            matches!(tenta_subir(&m, &format!("ok-{mn}-{mx}"), &c), Boot::Subiu),
            "min {mn} max {mx} deveria subir"
        );
    }
    let mut c = base(&m);
    c["codex_reconnect_min_s"] = json!(10);
    c["codex_reconnect_max_s"] = json!(5);
    exige_recusa(&m, "min-maior-que-max", &c);
}

#[test]
fn f2b_chave_desconhecida_inclusive_de_tempo_com_outro_nome_e_recusada() {
    let m = Mundo::novo();
    assert!(
        matches!(tenta_subir(&m, "base-3", &base(&m)), Boot::Subiu),
        "controle"
    );
    for chave in [
        "override_s",
        "override_after_ms",
        "hooks_silence_ms",
        "codex_reconnect_s",
        "thread_file_reread",
    ] {
        let mut c = base(&m);
        c[chave] = json!(15);
        exige_recusa(&m, &format!("desconhecida-{chave}"), &c);
    }
}

// ---------- P1: privacidade ----------

#[test]
fn p1_sentinelas_do_stdin_do_hook_do_app_server_e_do_nome_de_evento_nunca_saem() {
    let mut m = Mundo::novo();
    m.paineis(&[
        Painel::new("%1", "claude").tag("claude").pid("5001"),
        Painel::new("%2", "codex").tag("codex-blocks").pid("5002"),
    ]);
    m.tela("%1", TELA_SEM_REGRA);
    m.tela("%2", TELA_SEM_REGRA);
    let fake = FakeCodex::inicia(m.raiz.path(), UUID, json!({"type": "idle"}));
    let tf = thread_file(&m, "thread-p1", UUID.as_bytes(), 0o600);
    let c = cfg6(
        &m,
        json!({"claude": ag_claude("claude"), "codex-blocks": ag_codex("codex-blocks", &tf)}),
        &fake.socket,
    );
    let srv = sobe6(&m, &c);
    let sk = srv.socket.clone();
    espera_observado(&sk, "claude", Duration::from_secs(5));
    publica_em(&sk, "codex-blocks", "idle", Duration::from_secs(5));

    let s_stdin = sentinela("p1-stdin-do-hook");
    let s_flag = sentinela("p1-activeflag");
    let s_tipo = sentinela("p1-tipo-de-status");
    let s_evento = sentinela("p1-nome-de-evento");
    let mut saidas: Vec<String> = Vec::new();

    // 1. Stdin do hook com o segredo. Controle: o relato e aceito e o Claude vai a working.
    let payload = format!("{{\"prompt\":\"{s_stdin}\",\"last_assistant_message\":\"{s_stdin}\"}}");
    let r = report(&m, &sk, Some("%1"), "UserPromptSubmit", payload.as_bytes());
    assert_eq!(r.saida.code, Some(0));
    saidas.push(todo_texto(&r.saida));
    publica_em(&sk, "claude", "working", SEG);

    // 2. activeFlags inesperado. Controle: o daemon processou (saiu de idle). A regra de estado e S3c.
    fake.muda(json!({"type": "active", "activeFlags": [s_flag.clone()]}));
    espera_agente(&sk, "codex-blocks", SEG, |a| a["state"] != "idle");
    assert_ne!(
        estado_de(&sk, "codex-blocks"),
        "idle",
        "activeFlags inesperada publicou idle"
    );

    // 3. Tipo de status inesperado. Controle: volta a idle antes; o tipo novo tira de idle.
    fake.muda(json!({"type": "idle"}));
    publica_em(&sk, "codex-blocks", "idle", SEG);
    fake.muda(json!({"type": s_tipo.clone()}));
    espera_agente(&sk, "codex-blocks", SEG, |a| a["state"] != "idle");
    assert_ne!(
        estado_de(&sk, "codex-blocks"),
        "idle",
        "tipo inesperado publicou idle"
    );

    // 4. Nome de evento fora da lista: descartado, sem efeito e sem eco.
    let r = report(&m, &sk, Some("%1"), &s_evento, b"");
    assert_eq!(r.saida.code, Some(0), "evento invalido deve sair 0");
    saidas.push(todo_texto(&r.saida));

    thread::sleep(Duration::from_millis(300));
    let texto = format!(
        "{}\n{}\n{}",
        dump_daemon(&srv, &sk),
        dump_cli(&m, &sk, &["claude", "codex-blocks"]),
        saidas.join("\n")
    );
    for s in [&s_stdin, &s_flag, &s_tipo, &s_evento] {
        assert!(
            !texto.contains(s.as_str()),
            "sentinela {s} apareceu em alguma saida"
        );
    }
}

// ---------- P2: o daemon nunca escreve no tmux e so captura o que e configurado ----------

#[test]
fn p2_daemon_so_usa_list_panes_e_capture_pane_e_nunca_captura_painel_nao_configurado() {
    let mut m = Mundo::novo();
    m.paineis(&[
        Painel::new("%1", "claude").tag("claude").pid("5001"),
        Painel::new("%2", "codex").tag("codex-blocks").pid("5002"),
        // Claude sem marca e fora da configuracao (descoberta desligada): nunca pode ser capturado.
        Painel::new("%8", "claude").pid("5008"),
        Painel::new("%9", "zsh").pid("5009"),
    ]);
    m.tela("%1", TELA_SEM_REGRA);
    m.tela("%2", TELA_SEM_REGRA);
    m.tela_fixture("%8", "claude", "box-empty");
    m.tela("%9", TELA_SEM_REGRA);
    let fake = FakeCodex::inicia(m.raiz.path(), UUID, json!({"type": "idle"}));
    let tf = thread_file(&m, "thread-p2", UUID.as_bytes(), 0o600);
    let c = cfg6(
        &m,
        json!({"claude": ag_claude("claude"), "codex-blocks": ag_codex("codex-blocks", &tf)}),
        &fake.socket,
    );
    let srv = sobe6(&m, &c);
    let sk = srv.socket.clone();
    espera_observado(&sk, "claude", Duration::from_secs(5));
    publica_em(&sk, "codex-blocks", "idle", Duration::from_secs(5));

    // Um pouco de movimento: notificacoes, relatos e trocas de tela.
    fake.muda(json!({"type": "active", "activeFlags": []}));
    report(&m, &sk, Some("%1"), "UserPromptSubmit", b"");
    m.tela_fixture("%2", "codex", "working-indicator");
    thread::sleep(Duration::from_millis(800));
    fake.muda(json!({"type": "idle"}));
    report(&m, &sk, Some("%1"), "Stop", b"");
    thread::sleep(Duration::from_millis(1200));

    let violacoes = m.violacoes();
    assert!(
        violacoes.is_empty(),
        "chamadas fora da lista fechada: {violacoes:?}"
    );
    let chamadas = m.chamadas();
    assert!(
        !chamadas.is_empty(),
        "o daemon nao chamou o tmux (controle)"
    );
    for a in &chamadas {
        let listagem = a.len() == 4 && a[0] == "list-panes" && a[1] == "-a" && a[2] == "-F";
        let captura = a.len() == 6
            && a[0] == "capture-pane"
            && a[1] == "-p"
            && a[2] == "-e"
            && a[3] == "-J"
            && a[4] == "-t"
            && (a[5] == "%1" || a[5] == "%2");
        assert!(listagem || captura, "chamada fora da lista fechada: {a:?}");
    }
    let capturas = m.capturas();
    assert!(
        capturas.iter().any(|c| c.as_str() == "%1"),
        "controle: nao capturou o painel configurado %1"
    );
    assert!(
        capturas.iter().any(|c| c.as_str() == "%2"),
        "controle: nao capturou o painel configurado %2"
    );
    for c in &capturas {
        assert!(
            c.as_str() == "%1" || c.as_str() == "%2",
            "capturou painel nao configurado: {c}"
        );
    }
}

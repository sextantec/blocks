//! Testes de caixa preta do modo de eventos (spec rev5, secao 4A e 11): R18 (eventos antecipam o ciclo),
//! R19 (quietude por painel e degradacao) e R20 (o cliente de eventos so observa). O tmux e FALSO: os
//! `%output` vem de `eventos-N.txt` escritos pelo teste. Hoje devem FALHAR (binario da rev4, sem `events`).

mod rev5_common;

use rev5_common::*;
use serde_json::{Value, json};
use std::time::{Duration, Instant};

/// Sobe com `events: true`. `poll_ms` e `safety_poll_ms` ficam por conta do teste.
fn com_eventos(
    m: &Mundo,
    poll_ms: u64,
    safety_ms: u64,
    agentes: &[(&str, &str, &str)],
    ajuste: impl FnOnce(&mut Value),
) -> Servidor {
    inicia(m, poll_ms, agentes, move |c| {
        c["events"] = json!(true);
        c["safety_poll_ms"] = json!(safety_ms);
        ajuste(c);
    })
}

fn tem_marca(a: &Value, marca: &str) -> bool {
    marcas(a).contains(marca)
}

/// Espera ate haver `n` processos de anexo (`-C attach`) iniciados. Depois, uma folga curta: o falso
/// registra o inicio antes de fixar o ponto de leitura do feed, e um %output escrito nessa janela se perderia.
fn espera_anexos(m: &Mundo, n: usize, d: Duration) -> bool {
    let ok = espera(d, || m.anexos().len() >= n);
    std::thread::sleep(Duration::from_millis(300));
    ok
}

/// Amostra os estados por `d`, a cada 20 ms.
fn amostra(socket: &std::path::Path, id: &str, d: Duration) -> Vec<String> {
    let fim = Instant::now() + d;
    let mut v = Vec::new();
    loop {
        v.push(
            agente(socket, id)["state"]
                .as_str()
                .unwrap_or("?")
                .to_string(),
        );
        if Instant::now() >= fim {
            return v;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
}

// ---------- R18: eventos antecipam o ciclo ----------

#[test]
fn r18_output_do_painel_antecipa_o_ciclo_em_vez_de_esperar_o_poll() {
    let mut m = Mundo::novo();
    m.paineis(&[Painel::new("%1", "claude").tag("claude").sessao("$1")]);
    m.tela("%1", b"texto sem regra\n");
    // poll e seguranca em 10 s: qualquer leitura antes disso veio do evento.
    let srv = com_eventos(&m, 10000, 10000, &[("claude", "claude", "claude")], |_| {});
    assert!(
        espera_anexos(&m, 1, Duration::from_secs(5)),
        "R18: o cliente de eventos nao subiu"
    );
    std::thread::sleep(Duration::from_millis(200));
    m.tela_fixture("%1", "claude", "working-generation");
    let t0 = Instant::now();
    m.feed("$1", "%output %1 \u{1b}[?2026h quadro");
    let a = espera_estado(&srv.socket, "claude", "working", Duration::from_secs(5));
    assert_eq!(
        a["state"], "working",
        "R18: o evento devia disparar a leitura: {a}"
    );
    assert!(
        t0.elapsed() < Duration::from_secs(2),
        "R18: a leitura veio em {:?}, muito perto do poll de 10 s",
        t0.elapsed()
    );
}

#[test]
fn r18_rajada_de_output_gera_poucos_ciclos_nao_um_por_linha() {
    let mut m = Mundo::novo();
    m.paineis(&[Painel::new("%1", "claude").tag("claude").sessao("$1")]);
    m.tela_fixture("%1", "claude", "box-empty");
    let srv = com_eventos(&m, 10000, 10000, &[("claude", "claude", "claude")], |_| {});
    assert!(espera_anexos(&m, 1, Duration::from_secs(5)));
    std::thread::sleep(Duration::from_millis(400));
    let antes = m.listagens();
    for i in 0..50 {
        m.feed("$1", &format!("%output %1 quadro {i}"));
    }
    std::thread::sleep(Duration::from_millis(1000));
    let depois = m.listagens();
    let ciclos = depois - antes;
    assert!(ciclos >= 1, "R18: a rajada nao disparou nenhum ciclo");
    assert!(
        ciclos <= 4,
        "R18: 50 linhas em rajada geraram {ciclos} ciclos (esperado: um por janela de debounce)"
    );
    assert_eq!(
        agente(&srv.socket, "claude")["state"],
        "idle",
        "R18: estado de ocioso manteve-se"
    );
}

#[test]
fn r18_output_de_painel_nao_configurado_nao_gera_ciclo() {
    let mut m = Mundo::novo();
    m.paineis(&[
        Painel::new("%1", "claude").tag("claude").sessao("$1"),
        Painel::new("%9", "zsh").sessao("$1"),
    ]);
    m.tela_fixture("%1", "claude", "box-empty");
    let _srv = com_eventos(&m, 10000, 10000, &[("claude", "claude", "claude")], |_| {});
    assert!(espera_anexos(&m, 1, Duration::from_secs(5)));
    std::thread::sleep(Duration::from_millis(400));
    let antes = m.listagens();
    for _ in 0..5 {
        m.feed("$1", "%output %9 ruido de outro painel");
        std::thread::sleep(Duration::from_millis(200));
    }
    let depois = m.listagens();
    assert_eq!(
        depois - antes,
        0,
        "R18: %output de painel nao configurado nao pode gerar ciclo"
    );
}

#[test]
fn r18_conteudo_do_output_nunca_aparece_em_saida_nem_log() {
    let marcador = sentinela("SAIDA-EVENTO");
    let mut m = Mundo::novo();
    m.paineis(&[Painel::new("%1", "claude").tag("claude").sessao("$1")]);
    m.tela_fixture("%1", "claude", "box-empty");
    let srv = com_eventos(&m, 10000, 10000, &[("claude", "claude", "claude")], |_| {});
    assert!(espera_anexos(&m, 1, Duration::from_secs(5)));
    std::thread::sleep(Duration::from_millis(300));
    m.feed("$1", &format!("%output %1 {marcador} conteudo da tela"));
    std::thread::sleep(Duration::from_millis(500));
    let home = m.raiz.path().join("home");
    std::fs::create_dir_all(&home).unwrap();
    let saidas = [
        serde_json::to_string(&agente(&srv.socket, "claude")).unwrap(),
        serde_json::to_string(&status(&srv.socket)).unwrap(),
        serde_json::to_string(&eventos_todos(&srv.socket)).unwrap(),
        todo_texto(&cli(&home, &srv.socket, &["status", "--json"])),
        srv.stderr(),
    ];
    for (i, t) in saidas.iter().enumerate() {
        assert!(
            !t.contains(&marcador),
            "R18: conteudo de %output vazou (saida {i})"
        );
    }
}

// ---------- R19: quietude por painel e degradacao ----------

#[test]
fn r19_saida_do_proprio_painel_zera_a_confirmacao_de_idle() {
    let mut m = Mundo::novo();
    m.paineis(&[Painel::new("%1", "claude").tag("claude").sessao("$1")]);
    m.tela_fixture("%1", "claude", "box-empty");
    let srv = com_eventos(&m, 200, 5000, &[("claude", "claude", "claude")], |c| {
        c["idle_confirm_ms"] = json!(1500);
    });
    assert!(espera_anexos(&m, 1, Duration::from_secs(5)));
    assert!(espera(Duration::from_secs(4), || tem_marca(
        &agente(&srv.socket, "claude"),
        "idle_unconfirmed"
    )));
    // Enquanto o proprio painel produz saida, nunca pode virar idle.
    let fim = Instant::now() + Duration::from_millis(2400);
    let mut visto = Vec::new();
    while Instant::now() < fim {
        m.feed("$1", "%output %1 quadro");
        visto.push(
            agente(&srv.socket, "claude")["state"]
                .as_str()
                .unwrap_or("?")
                .to_string(),
        );
        std::thread::sleep(Duration::from_millis(300));
    }
    assert!(
        !visto.iter().any(|s| s == "idle"),
        "R19: saida do proprio painel nao zerou a janela: {visto:?}"
    );
    let a = espera_estado(&srv.socket, "claude", "idle", Duration::from_secs(5));
    assert_eq!(
        a["state"], "idle",
        "R19: depois de parar a saida devia confirmar: {a}"
    );
}

#[test]
fn r19_saida_de_outro_painel_da_mesma_janela_nao_zera() {
    let mut m = Mundo::novo();
    m.paineis(&[
        Painel::new("%1", "claude").tag("claude").sessao("$1"),
        Painel::new("%9", "zsh").sessao("$1"),
    ]);
    m.tela_fixture("%1", "claude", "box-empty");
    let srv = com_eventos(&m, 200, 5000, &[("claude", "claude", "claude")], |c| {
        c["idle_confirm_ms"] = json!(1200);
    });
    assert!(espera_anexos(&m, 1, Duration::from_secs(5)));
    let fim = Instant::now() + Duration::from_millis(2000);
    while Instant::now() < fim {
        m.feed("$1", "%output %9 saida do vizinho");
        std::thread::sleep(Duration::from_millis(250));
        if agente(&srv.socket, "claude")["state"] == "idle" {
            break;
        }
    }
    let a = espera_estado(&srv.socket, "claude", "idle", Duration::from_secs(4));
    assert_eq!(
        a["state"], "idle",
        "R19: saida de outro painel nao pode zerar a confirmacao: {a}"
    );
}

#[test]
fn r19_cliente_morto_degrada_nada_vira_idle_e_religa() {
    let mut m = Mundo::novo();
    m.paineis(&[Painel::new("%1", "claude").tag("claude").sessao("$1")]);
    m.tela_fixture("%1", "claude", "working-generation");
    let srv = com_eventos(&m, 200, 5000, &[("claude", "claude", "claude")], |c| {
        c["idle_confirm_ms"] = json!(1000);
    });
    assert!(espera_anexos(&m, 1, Duration::from_secs(5)));
    assert!(
        espera_estado(&srv.socket, "claude", "working", Duration::from_secs(5))["state"]
            == "working"
    );
    let n_antes = m.anexos().len();
    m.feed("$1", "%exit");
    assert!(
        espera(Duration::from_secs(3), || fonte_tmux(&srv.socket)["events"]
            == "degraded"),
        "R19: cliente morto devia deixar events: degraded ({})",
        fonte_tmux(&srv.socket)
    );
    let amostra_morte = amostra(&srv.socket, "claude", Duration::from_millis(1200));
    assert!(
        !amostra_morte.iter().any(|s| s == "idle"),
        "R19: a morte do cliente virou idle: {amostra_morte:?}"
    );
    assert!(
        espera(Duration::from_secs(6), || m.anexos().len() > n_antes),
        "R19: o cliente devia ser religado com recuo (1 s)"
    );
    m.tela_fixture("%1", "claude", "box-empty");
    let a = espera_estado(&srv.socket, "claude", "idle", Duration::from_secs(8));
    assert_eq!(
        a["state"], "idle",
        "R19: sem eventos a confirmacao por window_activity devia funcionar: {a}"
    );
}

#[test]
fn r19_cliente_que_nao_sobe_degrada_e_o_estado_segue_por_polling() {
    let mut m = Mundo::novo();
    m.paineis(&[Painel::new("%1", "claude").tag("claude").sessao("$1")]);
    m.tela_fixture("%1", "claude", "box-empty");
    m.ajusta("anexo_falha", json!(true));
    let srv = com_eventos(&m, 200, 5000, &[("claude", "claude", "claude")], |c| {
        c["idle_confirm_ms"] = json!(800);
    });
    assert!(espera(
        Duration::from_secs(4),
        || fonte_tmux(&srv.socket)["events"] == "degraded"
    ));
    let a = espera_estado(&srv.socket, "claude", "idle", Duration::from_secs(6));
    assert_eq!(
        a["state"], "idle",
        "R19: sem cliente de eventos o polling ainda confirma idle: {a}"
    );
}

// ---------- R20: o cliente de eventos so observa ----------

#[test]
fn r20_argv_exato_stdin_zero_e_um_cliente_por_sessao() {
    let mut m = Mundo::novo();
    m.paineis(&[
        Painel::new("%1", "claude").tag("claude").sessao("$1"),
        Painel::new("%2", "codex").tag("codex-blocks").sessao("$2"),
    ]);
    m.tela_fixture("%1", "claude", "box-empty");
    m.tela_fixture("%2", "codex", "idle");
    let srv = com_eventos(&m, 200, 5000, &agentes_todos()[..2], |_| {});
    assert!(
        espera_anexos(&m, 2, Duration::from_secs(5)),
        "R20: esperava um cliente por sessao"
    );
    // Spec 15.2: com o cliente vivo em todas as sessoes com painel configurado, events = "on".
    assert_eq!(
        fonte_tmux(&srv.socket)["events"],
        "on",
        "R20 (spec 15.2): status.sources.tmux.events devia ser on com o cliente vivo"
    );
    m.feed("$1", "%output %1 um");
    m.feed("$2", "%output %2 dois");
    std::thread::sleep(Duration::from_millis(1200));
    let anexos = m.anexos();
    for a in &anexos {
        // Sem tmux_socket configurado o argv e exatamente este (sem -S).
        let forma_exata = a.len() == 6
            && a[0] == "-C"
            && a[1] == "attach"
            && a[2] == "-f"
            && a[3] == "ignore-size"
            && a[4] == "-t"
            && (a[5] == "$1" || a[5] == "$2");
        assert!(
            forma_exata,
            "R20: argv do cliente de eventos fora da forma exata: {a:?}"
        );
    }
    let alvos: std::collections::BTreeSet<String> =
        anexos.iter().map(|a| a.last().cloned().unwrap()).collect();
    assert_eq!(
        alvos.len(),
        anexos.len(),
        "R20: dois clientes para a mesma sessao: {anexos:?}"
    );
    assert_eq!(
        m.bytes_stdin(),
        0,
        "R20: o daemon escreveu no stdin do cliente de eventos"
    );
    assert!(
        !m.registros().iter().any(|r| r["ev"] == "stdin_eof"),
        "R20: o stdin do cliente foi fechado enquanto vivo"
    );
    assert_eq!(
        m.violacoes(),
        Vec::<Vec<String>>::new(),
        "R20: subcomando fora da lista"
    );
    let _ = srv.socket.exists();
}

#[test]
fn r20_com_tmux_socket_o_s_vem_antes_do_c() {
    // Spec 15.7: TMUX -S SOCKET -C attach ... (o -S antes do subcomando, tambem no cliente de eventos).
    let mut m = Mundo::novo();
    m.paineis(&[Painel::new("%1", "claude").tag("claude").sessao("$1")]);
    m.tela_fixture("%1", "claude", "box-empty");
    let sock = m.raiz.path().join("tmux.sock").display().to_string();
    let sock2 = sock.clone();
    let srv = com_eventos(&m, 200, 5000, &[("claude", "claude", "claude")], move |c| {
        c["tmux_socket"] = json!(sock2);
    });
    assert!(
        espera_anexos(&m, 1, Duration::from_secs(5)),
        "R20: o cliente nao subiu"
    );
    let anexos = m.anexos();
    assert!(
        anexos
            .iter()
            .all(|a| a.len() == 8 && a[0] == "-S" && a[1] == sock && a[2] == "-C"),
        "R20 (spec 15.7): -S SOCKET deve vir antes de -C attach: {anexos:?}"
    );
    assert!(
        m.chamadas()
            .iter()
            .filter(|a| a.iter().any(|x| x == "list-panes"))
            .all(|a| a.len() >= 2 && a[0] == "-S" && a[1] == sock),
        "R20 (spec 15.7): list-panes com -S antes do subcomando"
    );
    assert_eq!(fonte_tmux(&srv.socket)["events"], "on");
}

#[test]
fn r19_status_events_off_quando_desligado() {
    // Spec 15.2: com `events: false` a fonte reporta "off" (nao "degraded").
    let mut m = Mundo::novo();
    m.paineis(&[Painel::new("%1", "claude").tag("claude").sessao("$1")]);
    m.tela_fixture("%1", "claude", "box-empty");
    let srv = inicia(&m, 200, &[("claude", "claude", "claude")], |_| {});
    assert!(espera_fonte(&srv.socket, true, Duration::from_secs(5)));
    assert_eq!(
        fonte_tmux(&srv.socket)["events"],
        "off",
        "R19 (spec 15.2): events: false devia reportar off"
    );
    assert!(
        m.anexos().is_empty(),
        "R19: sem eventos nao ha cliente de eventos"
    );
}

#[test]
fn r19_window_activity_nao_zera_a_janela_com_cliente_vivo() {
    // Spec 15.8: com o cliente vivo a quietude e so o tempo desde o ultimo %output DO PROPRIO painel.
    // Mudar window_activity (sem %output do painel) nao zera a confirmacao.
    let mut m = Mundo::novo();
    m.paineis(&[Painel::new("%1", "claude").tag("claude").sessao("$1")]);
    m.tela_fixture("%1", "claude", "box-empty");
    let srv = com_eventos(&m, 200, 5000, &[("claude", "claude", "claude")], |c| {
        c["idle_confirm_ms"] = json!(1200);
    });
    assert!(espera_anexos(&m, 1, Duration::from_secs(5)));
    let base = 1_700_000_000u64;
    let inicio = Instant::now();
    let mut i = 0u64;
    let mut visto_idle = None;
    while inicio.elapsed() < Duration::from_millis(3500) {
        m.paineis(&[Painel::new("%1", "claude")
            .tag("claude")
            .sessao("$1")
            .ativ(base + i)]);
        i += 1;
        if agente(&srv.socket, "claude")["state"] == "idle" {
            visto_idle = Some(inicio.elapsed());
            break;
        }
        std::thread::sleep(Duration::from_millis(300));
    }
    let quando = visto_idle
        .expect("R19 (spec 15.8): window_activity mudando zerou a quietude; idle nunca veio");
    assert!(
        quando < Duration::from_millis(3000),
        "R19 (spec 15.8): idle demorou {quando:?}; a janela devia correr sem reset"
    );
}

#[test]
fn r20_no_maximo_oito_clientes_de_eventos() {
    let mut m = Mundo::novo();
    let mut ps = Vec::new();
    let mut ags = Vec::new();
    let nomes: Vec<String> = (1..=9).map(|i| format!("a{i}")).collect();
    for (i, nome) in nomes.iter().enumerate() {
        let sess = format!("${}", i + 1);
        ps.push(
            Painel::new(&format!("%{}", i + 1), "claude")
                .tag(nome)
                .sessao(&sess),
        );
        ags.push((nome.as_str(), "claude", nome.as_str()));
    }
    m.paineis(&ps);
    let _srv = com_eventos(&m, 200, 5000, &ags, |_| {});
    assert!(
        espera(Duration::from_secs(8), || m.anexos().len() >= 8),
        "R20: nao subiu nem 8 clientes"
    );
    std::thread::sleep(Duration::from_millis(800));
    let n = m.anexos().len();
    assert!(n <= 8, "R20: {n} clientes de eventos (limite 8)");
    assert_eq!(m.violacoes(), Vec::<Vec<String>>::new());
}

#[test]
fn r19_depois_de_idle_confirmado_uma_saida_do_proprio_painel_reconfirma_sem_laco() {
    // Spec 4A (gatilho) e 5' (confirmacao): com idle confirmado e uma unica linha %output do proprio
    // painel, a janela reabre e fecha em idle_confirm_ms sem nova saida; e sem saida nova nao ha laco
    // de ciclos (so o ciclo de seguranca, safety_poll_ms = 5000).
    let mut m = Mundo::novo();
    m.paineis(&[Painel::new("%1", "claude").tag("claude").sessao("$1")]);
    m.tela_fixture("%1", "claude", "box-empty");
    let srv = com_eventos(&m, 200, 5000, &[("claude", "claude", "claude")], |c| {
        c["idle_confirm_ms"] = json!(1500);
    });
    assert!(espera_anexos(&m, 1, Duration::from_secs(5)));
    let a = espera_estado(&srv.socket, "claude", "idle", Duration::from_secs(6));
    assert_eq!(a["state"], "idle", "R19: idle inicial nao confirmou: {a}");

    m.feed("$1", "%output %1 quadro");
    let reconfirmou = espera(Duration::from_secs(6), || {
        let x = agente(&srv.socket, "claude");
        x["state"] == "idle" && !marcas(&x).contains("idle_unconfirmed")
    });
    assert!(
        reconfirmou,
        "R19 (spec 5'): depois de uma saida do proprio painel a confirmacao nao voltou sem nova saida: {}",
        agente(&srv.socket, "claude")
    );

    let c0 = m.listagens();
    std::thread::sleep(Duration::from_millis(2000));
    let ciclos = m.listagens() - c0;
    assert!(
        ciclos <= 3,
        "R19 (spec 4A): {ciclos} ciclos em 2 s sem saida nova; so o ciclo de seguranca devia rodar"
    );
    assert_eq!(agente(&srv.socket, "claude")["state"], "idle");
}

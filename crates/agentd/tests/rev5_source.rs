//! Testes de caixa preta da fonte tmux (spec rev5, secoes 4, 5 e 11): R2 (falhas), R3 (lista fechada),
//! R4 (sem shell), R5 (privacidade), R10 (painel), R13 (stale). O tmux e FALSO (fake_tmux.py).
//! Hoje devem FALHAR: o binario e da rev4 (herdr) e nao entende a fonte `tmux`.

mod rev5_common;

use rev5_common::*;
use serde_json::{Value, json};
use std::time::{Duration, Instant};

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

/// Amostra `agent.get` durante `d`; devolve os estados vistos.
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

fn tem_erro(a: &Value) -> bool {
    marcas(a).contains("error")
}

// ---------- R2: unknown por padrao e falhas da fonte ----------

#[test]
fn r02_tela_sem_regra_e_unknown_com_unmatched_sem_error_nunca_idle() {
    let (_m, srv, id) = um(
        "claude",
        b"texto qualquer, sem nenhuma regra\n",
        "",
        200,
        |_| {},
    );
    let a = espera_agente(&srv.socket, id, Duration::from_secs(4), |x| {
        !x["observed_at"].is_null()
    });
    assert_eq!(a["state"], "unknown", "R2: {a}");
    assert_eq!(
        marcas(&a),
        marcas_de(&["unmatched"]),
        "R2: so unmatched, sem error: {a}"
    );
    assert!(a["rule"].is_null(), "R2: rule deve ser null: {a}");
    assert!(
        !amostra(&srv.socket, id, Duration::from_millis(1200))
            .iter()
            .any(|s| s == "idle")
    );
}

#[test]
fn r02_tela_vazia_e_unknown() {
    let (_m, srv, id) = um("claude", b"", "", 200, |_| {});
    let a = espera_agente(&srv.socket, id, Duration::from_secs(4), |x| {
        !x["observed_at"].is_null()
    });
    assert_eq!(a["state"], "unknown", "R2 tela vazia: {a}");
    assert!(marcas(&a).contains("unmatched"), "R2 tela vazia: {a}");
}

#[test]
fn r02_tela_de_outro_programa_e_unknown() {
    // Tela do Pi num painel marcado como Claude: nenhuma regra de Claude casa.
    let (_m, srv, id) = um("claude", &tela_fixture("pi", "idle"), "", 200, |_| {});
    let a = espera_agente(&srv.socket, id, Duration::from_secs(4), |x| {
        !x["observed_at"].is_null()
    });
    assert_eq!(a["state"], "unknown", "R2 outro programa: {a}");
    assert!(
        !amostra(&srv.socket, id, Duration::from_millis(800))
            .iter()
            .any(|s| s == "idle")
    );
}

#[test]
fn r02_servidor_tmux_fora_e_source_exit_nonzero_nunca_gone() {
    let mut m = Mundo::novo();
    m.paineis(&[Painel::new("%1", "claude").tag("claude")]);
    m.listar("sem_servidor");
    let srv = inicia(&m, 200, &[("claude", "claude", "claude")], |_| {});
    let t0 = Instant::now();
    let estados = amostra(&srv.socket, "claude", Duration::from_secs(6));
    assert!(
        !estados.iter().any(|s| s == "gone"),
        "R2: 'no server running' e leitura falha, nunca gone (visto {:?} em {:?})",
        estados.iter().collect::<std::collections::BTreeSet<_>>(),
        t0.elapsed()
    );
    let a = agente(&srv.socket, "claude");
    assert_eq!(a["state"], "unknown");
    assert!(tem_erro(&a), "R2: devia ter error: {a}");
    let f = fonte_tmux(&srv.socket);
    assert_eq!(
        f["ok"], false,
        "R2: list-panes com saida nao zero: ok deve ser false: {f}"
    );
    assert_eq!(f["last_error"], "source_exit_nonzero", "R2: {f}");
}

#[test]
fn r02_saida_invalida_do_list_panes_e_source_bad_output() {
    let mut m = Mundo::novo();
    m.paineis(&[Painel::new("%1", "claude").tag("claude")]);
    m.tela_fixture("%1", "claude", "box-empty");
    m.listar_com("texto", json!({"texto": "linha sem tabs nenhuma\n"}));
    let srv = inicia(&m, 200, &[("claude", "claude", "claude")], |_| {});
    let a = espera_agente(&srv.socket, "claude", Duration::from_secs(4), tem_erro);
    assert!(tem_erro(&a), "R2: saida invalida devia gerar error: {a}");
    assert_eq!(a["state"], "unknown", "R2: {a}");
    let f = fonte_tmux(&srv.socket);
    assert_eq!(f["ok"], false, "R2: {f}");
    assert_eq!(f["last_error"], "source_bad_output", "R2: {f}");
}

#[test]
fn r02_pane_id_duplicado_falha_toda_a_leitura() {
    let mut m = Mundo::novo();
    m.paineis(&[
        Painel::new("%1", "claude").tag("claude"),
        Painel::new("%1", "claude").tag("codex-blocks"),
    ]);
    m.tela_fixture("%1", "claude", "box-empty");
    let srv = inicia(&m, 200, &agentes_todos(), |_| {});
    let a = espera_agente(&srv.socket, "claude", Duration::from_secs(4), tem_erro);
    assert!(
        tem_erro(&a),
        "R2: pane_id repetido devia falhar a leitura: {a}"
    );
    assert_ne!(a["state"], "idle", "R2: {a}");
    assert_eq!(fonte_tmux(&srv.socket)["last_error"], "duplicate_pane");
}

#[test]
fn r02_tag_ambigua_falha_so_aquele_agente() {
    let mut m = Mundo::novo();
    m.paineis(&[
        Painel::new("%1", "claude").tag("claude"),
        Painel::new("%4", "claude").tag("claude"),
        Painel::new("%2", "codex").tag("codex-blocks"),
    ]);
    m.tela_fixture("%1", "claude", "box-empty");
    m.tela_fixture("%4", "claude", "box-empty");
    m.tela_fixture("%2", "codex", "idle");
    let srv = inicia(&m, 200, &agentes_todos()[..2], |_| {});
    let a = espera_agente(&srv.socket, "claude", Duration::from_secs(4), tem_erro);
    assert!(
        tem_erro(&a),
        "R2: tag ambigua devia gerar error no agente: {a}"
    );
    assert_eq!(a["state"], "unknown", "R2: {a}");
    let b = espera_estado(&srv.socket, "codex-blocks", "idle", Duration::from_secs(6));
    assert_eq!(b["state"], "idle", "R2: o outro agente segue normal: {b}");
    assert_eq!(
        fonte_tmux(&srv.socket)["ok"],
        true,
        "R2: a execucao do list-panes deu certo"
    );
}

#[test]
fn r02_captura_falha_de_um_agente_nao_derruba_o_outro() {
    let mut m = Mundo::novo();
    m.paineis(&[
        Painel::new("%1", "claude").tag("claude"),
        Painel::new("%2", "codex").tag("codex-blocks"),
    ]);
    m.tela_fixture("%1", "claude", "box-empty");
    m.tela_fixture("%2", "codex", "idle");
    m.captura_falha("%1", true);
    let srv = inicia(&m, 200, &agentes_todos()[..2], |_| {});
    let b = espera_estado(&srv.socket, "codex-blocks", "idle", Duration::from_secs(6));
    assert_eq!(b["state"], "idle", "R2: {b}");
    let a = agente(&srv.socket, "claude");
    assert_ne!(
        a["state"], "idle",
        "R2: capture-pane falho nao pode virar idle: {a}"
    );
    assert!(tem_erro(&a), "R2: capture falho devia marcar error: {a}");
}

#[test]
fn r02_tela_acima_de_256_kib_falha_so_desse_agente() {
    let mut m = Mundo::novo();
    m.paineis(&[
        Painel::new("%1", "claude").tag("claude"),
        Painel::new("%2", "codex").tag("codex-blocks"),
    ]);
    let grande: Vec<u8> = b"linha enorme\n"
        .iter()
        .copied()
        .cycle()
        .take(300 * 1024)
        .collect();
    m.tela("%1", &grande);
    m.tela_fixture("%2", "codex", "idle");
    let srv = inicia(&m, 200, &agentes_todos()[..2], |_| {});
    let a = espera_agente(&srv.socket, "claude", Duration::from_secs(4), tem_erro);
    assert!(tem_erro(&a), "R2: tela > 256 KiB devia gerar error: {a}");
    assert_ne!(a["state"], "idle", "R2: {a}");
    let b = espera_estado(&srv.socket, "codex-blocks", "idle", Duration::from_secs(6));
    assert_eq!(b["state"], "idle", "R2: o outro agente segue: {b}");
    assert_eq!(
        fonte_tmux(&srv.socket)["ok"],
        true,
        "R2: list-panes deu certo"
    );
}

#[test]
fn r02_saida_do_list_panes_acima_de_1_mib_falha_toda_a_leitura() {
    let mut m = Mundo::novo();
    m.paineis(&[Painel::new("%1", "claude").tag("claude")]);
    m.tela_fixture("%1", "claude", "box-empty");
    let arq = m.raiz.path().join("grande.txt");
    let corpo: Vec<u8> = b"x"
        .iter()
        .copied()
        .cycle()
        .take(1024 * 1024 + 16)
        .collect();
    std::fs::write(&arq, corpo).unwrap();
    m.listar_com("arquivo", json!({"arquivo": arq.display().to_string()}));
    let srv = inicia(&m, 200, &[("claude", "claude", "claude")], |_| {});
    let a = espera_agente(&srv.socket, "claude", Duration::from_secs(4), tem_erro);
    assert!(tem_erro(&a), "R2: stdout > 1 MiB devia falhar: {a}");
    assert_ne!(a["state"], "idle", "R2: {a}");
    let f = fonte_tmux(&srv.socket);
    assert_eq!(f["ok"], false, "R2: {f}");
    // Spec 15.3: a saida acima de 1 MiB do list-panes tem o mesmo codigo da captura acima de 256 KiB.
    assert_eq!(f["last_error"], "source_too_large", "R2 (spec 15.3): {f}");
}

// ---------- R3: lista fechada ----------

#[test]
fn r03_lista_fechada_por_dez_ciclos_so_list_panes_e_capture_de_presentes() {
    let mut m = Mundo::novo();
    m.paineis(&[
        Painel::new("%1", "claude").tag("claude"),
        Painel::new("%2", "codex").tag("codex-blocks"),
        Painel::new("%3", "zsh").tag("pi"),
        Painel::new("%5", "claude"),
        Painel::new("%7", "codex").tag("codex-morto").morto(true),
    ]);
    for p in ["%1", "%2", "%3", "%5", "%7"] {
        m.tela_fixture(p, "claude", "box-empty");
    }
    let agentes = [
        ("claude", "claude", "claude"),
        ("codex-blocks", "codex", "codex-blocks"),
        ("pi", "pi", "pi"),
        ("codex-morto", "codex", "codex-morto"),
    ];
    let srv = inicia(&m, 200, &agentes, |_| {});
    assert!(
        espera(Duration::from_secs(10), || m.listagens() >= 10),
        "R3: menos de 10 ciclos"
    );
    assert_eq!(
        m.violacoes(),
        Vec::<Vec<String>>::new(),
        "R3: chamada fora da lista fechada"
    );
    assert_eq!(
        m.sobreposicoes(),
        0,
        "R3: duas leituras em voo ao mesmo tempo"
    );
    let caps = m.capturas();
    for proibido in ["%3", "%5", "%7"] {
        assert!(
            !caps.iter().any(|c| c == proibido),
            "R3: capturou painel nao configurado, morto ou com shell ({proibido}): {caps:?}"
        );
    }
    assert!(
        caps.iter().any(|c| c == "%1"),
        "R3: %1 devia ser capturado: {caps:?}"
    );
    assert!(
        caps.iter().any(|c| c == "%2"),
        "R3: %2 devia ser capturado: {caps:?}"
    );
    let n_list = m.listagens();
    let n_cap1 = caps.iter().filter(|c| *c == "%1").count();
    assert!(
        n_cap1 <= n_list,
        "R3: mais de uma captura por ciclo para %1 ({n_cap1} capturas, {n_list} listagens)"
    );
    // Spec 15.6 (e secao 4): o -F tem TAB literal (0x09) entre nove campos, nesta ordem.
    let formatos: Vec<String> = m
        .chamadas()
        .iter()
        .filter(|a| a.iter().any(|x| x == "list-panes"))
        .filter_map(|a| a.last().cloned())
        .collect();
    assert!(!formatos.is_empty(), "R3: nenhum list-panes registrado");
    let esperado = [
        "#{pane_id}",
        "#{pane_pid}",
        "#{pane_dead}",
        "#{pane_dead_status}",
        "#{window_activity}",
        "#{pane_current_command}",
        "#{session_id}",
        "#{@agentd}",
        "#{pane_title}",
    ];
    for f in &formatos {
        let campos: Vec<&str> = f.split('\t').collect();
        assert_eq!(
            campos, esperado,
            "R3 (spec 15.6): o -F deve ser os nove campos separados por TAB: {f:?}"
        );
    }
    assert!(srv.socket.exists());
}

// ---------- R4: sem shell ----------

#[test]
fn r04_caminhos_hostis_do_tmux_e_do_socket_nao_executam_nada() {
    let mut m = Mundo::novo_em("bin x;touch PWN1;$(touch PWN2) 'q'/tmux");
    m.paineis(&[Painel::new("%1", "claude").tag("claude")]);
    m.tela_fixture("%1", "claude", "box-empty");
    let raiz = m.raiz.path().to_path_buf();
    let socket_hostil = format!("{}/sock ;touch PWN3;$(touch PWN4) \"a\"", raiz.display());
    let regras = raiz.join("regras.json");
    copia_regras(&regras, &regras_padrao());
    let mut c = cfg(&m, &regras, 200, &[("claude", "claude", "claude")]);
    c["tmux_socket"] = json!(socket_hostil);
    let cfgp = escreve_cfg(&raiz, &c);
    let srv = sobe_em(Some(raiz.as_path()), &estado_dir(&m), &cfgp);
    let a = espera_estado(&srv.socket, "claude", "idle", Duration::from_secs(6));
    assert_eq!(
        a["state"], "idle",
        "R4: o tmux de caminho hostil devia ser executado sem shell: {a}"
    );
    let ch = m.chamadas();
    assert!(
        ch.iter().any(|x| x.len() >= 3
            && x[0] == "-S"
            && x[1] == socket_hostil
            && x[2] == "capture-pane"),
        "R4 (spec 15.7): -S antes do subcomando, com o caminho literal, tambem no capture-pane: {ch:?}"
    );
    for marca in ["PWN1", "PWN2", "PWN3", "PWN4"] {
        assert!(
            !raiz.join(marca).exists(),
            "R4: um shell executou o texto do caminho ({marca})"
        );
    }
}

// ---------- R5: privacidade (sentinelas nunca saem) ----------

#[test]
fn r05_sentinelas_de_tela_titulo_stderr_saida_invalida_e_cwd_nunca_saem() {
    let s_tela = sentinela("TELA");
    let s_tit = sentinela("TITULO");
    let s_err = sentinela("STDERR");
    let s_cwd = sentinela("CWD");
    let s_bad = sentinela("SAIDA");
    let mut m = Mundo::novo();
    m.paineis(&[Painel::new("%1", "claude").tag("claude").titulo(&s_tit)]);
    m.tela("%1", format!("{s_tela} texto sem regra\n").as_bytes());
    m.ajusta("stderr", json!(s_err));
    let raiz = m.raiz.path().to_path_buf();
    let dir_cwd = raiz.join(&s_cwd);
    std::fs::create_dir_all(&dir_cwd).unwrap();
    let regras = raiz.join("regras.json");
    copia_regras(&regras, &regras_padrao());
    let c = cfg(&m, &regras, 200, &[("claude", "claude", "claude")]);
    let cfgp = escreve_cfg(&raiz, &c);
    let srv = sobe_em(Some(dir_cwd.as_path()), &estado_dir(&m), &cfgp);
    let _ = espera_agente(&srv.socket, "claude", Duration::from_secs(4), |x| {
        !x["observed_at"].is_null()
    });

    let home = raiz.join("home");
    std::fs::create_dir_all(&home).unwrap();
    let so = srv.socket.clone();
    let mut todos = vec![
        serde_json::to_string(&agente(&so, "claude")).unwrap(),
        serde_json::to_string(&lista(&so)).unwrap(),
        serde_json::to_string(&status(&so)).unwrap(),
        serde_json::to_string(&eventos_todos(&so)).unwrap(),
    ];
    for args in [
        vec!["get", "claude", "--json"],
        vec!["get", "claude"],
        vec!["list", "--json"],
        vec!["status", "--json"],
        vec!["health"],
    ] {
        todos.push(todo_texto(&cli(&home, &so, &args)));
    }
    todos.push(srv.stderr());

    // Saida invalida: o proximo ciclo falha com a sentinela na saida.
    m.listar_com("texto", json!({"texto": format!("{s_bad} nao e tabela\n")}));
    let _ = espera_agente(&so, "claude", Duration::from_secs(4), |x| {
        marcas(x).contains("error")
    });
    todos.push(serde_json::to_string(&agente(&so, "claude")).unwrap());
    todos.push(serde_json::to_string(&status(&so)).unwrap());
    todos.push(srv.stderr());

    for (nome, s) in [
        ("tela", &s_tela),
        ("titulo", &s_tit),
        ("stderr", &s_err),
        ("saida", &s_bad),
        ("cwd", &s_cwd),
    ] {
        for (i, t) in todos.iter().enumerate() {
            assert!(
                !t.contains(s.as_str()),
                "R5: sentinela de {nome} vazou (saida {i}): {t}"
            );
        }
    }
}

// ---------- R10: painel ausente, morto ou trocado ----------

#[test]
fn r10_pid_que_muda_vira_unknown_com_error_e_evento() {
    let (mut m, srv, id) = um(
        "claude",
        &tela_fixture("claude", "working-generation"),
        "",
        200,
        |_| {},
    );
    assert!(espera(
        Duration::from_secs(5),
        || agente(&srv.socket, id)["state"] == "working"
    ));
    let head = status(&srv.socket)["head_seq"].as_u64().unwrap();
    m.paineis(&[Painel::new("%1", "claude").tag("claude").pid("9999")]);
    let a = espera_agente(&srv.socket, id, Duration::from_secs(4), tem_erro);
    assert_eq!(a["state"], "unknown", "R10: pane_pid mudou: {a}");
    assert!(tem_erro(&a), "R10: {a}");
    let evs = eventos_desde(&srv.socket, head);
    assert!(
        evs.iter().any(|e| e["state"] == "unknown"),
        "R10: devia haver evento: {evs:?}"
    );
}

#[test]
fn r10_painel_some_vira_gone_so_depois_de_gone_after_s() {
    let (mut m, srv, id) = um(
        "claude",
        &tela_fixture("claude", "working-generation"),
        "",
        200,
        |_| {},
    );
    assert!(espera(
        Duration::from_secs(5),
        || agente(&srv.socket, id)["state"] == "working"
    ));
    let t0 = Instant::now();
    m.paineis(&[]);
    let mut gone_em = None;
    let fim = t0 + Duration::from_secs(12);
    while Instant::now() < fim {
        let a = agente(&srv.socket, id);
        if a["state"] == "gone" {
            gone_em = Some(t0.elapsed());
            break;
        }
        // Antes da primeira leitura que mostra a ausencia o estado ainda pode ser o anterior (working).
        // Depois que `error` aparece, o estado publicado tem que ser unknown ate gone.
        if tem_erro(&a) {
            assert_eq!(
                a["state"], "unknown",
                "R10: ausente deve ser unknown ate gone: {a}"
            );
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    let quando = gone_em.expect("R10: nao virou gone em 12 s");
    assert!(
        quando >= Duration::from_millis(4900),
        "R10: gone antes de gone_after_s=5 (em {quando:?})"
    );
}

#[test]
fn r10_painel_morto_ou_que_volta_ao_shell_e_ausente_e_volta_com_evento() {
    let (mut m, srv, id) = um(
        "claude",
        &tela_fixture("claude", "working-generation"),
        "",
        200,
        |_| {},
    );
    assert!(espera(
        Duration::from_secs(5),
        || agente(&srv.socket, id)["state"] == "working"
    ));
    // Morto (pane_dead=1): ausente.
    m.paineis(&[Painel::new("%1", "claude").tag("claude").morto(true)]);
    let a = espera_estado(&srv.socket, id, "gone", Duration::from_secs(12));
    assert_eq!(
        a["state"], "gone",
        "R10: painel morto devia virar gone: {a}"
    );
    // Volta ao shell: continua ausente; depois o claude volta: working, com evento.
    m.paineis(&[Painel::new("%1", "zsh").tag("claude")]);
    std::thread::sleep(Duration::from_millis(400));
    assert_eq!(
        agente(&srv.socket, id)["state"],
        "gone",
        "R10: shell nunca e lido: {}",
        agente(&srv.socket, id)
    );
    m.paineis(&[Painel::new("%1", "claude").tag("claude")]);
    let b = espera_estado(&srv.socket, id, "working", Duration::from_secs(6));
    assert_eq!(
        b["state"], "working",
        "R10: gone volta a working com leitura valida: {b}"
    );
    let evs = eventos_todos(&srv.socket);
    assert!(
        evs.iter()
            .any(|e| e["previous_state"] == "gone" && e["state"] == "working"),
        "R10: devia haver evento gone -> working: {evs:?}"
    );
}

#[test]
fn r10_lista_vazia_apos_erros_nao_gera_gone_imediato() {
    let (mut m, srv, id) = um(
        "claude",
        &tela_fixture("claude", "working-generation"),
        "",
        200,
        |_| {},
    );
    assert!(espera(
        Duration::from_secs(5),
        || agente(&srv.socket, id)["state"] == "working"
    ));
    m.listar("falha");
    std::thread::sleep(Duration::from_millis(500));
    m.listar("ok");
    m.paineis(&[]);
    let fim = Instant::now() + Duration::from_millis(2000);
    while Instant::now() < fim {
        let a = agente(&srv.socket, id);
        assert_ne!(
            a["state"], "gone",
            "R10: lista vazia logo apos erros nao pode dar gone imediato: {a}"
        );
        std::thread::sleep(Duration::from_millis(20));
    }
}

// ---------- R13: stale pela consulta (leitura travada) ----------

#[test]
fn r13_leitura_travada_vira_stale_com_last_known() {
    let stale_ms = 1500u64;
    let (mut m, srv, id) = um(
        "claude",
        &tela_fixture("claude", "working-generation"),
        "",
        200,
        |c| {
            c["stale_after_ms"] = json!(stale_ms);
        },
    );
    assert!(espera(
        Duration::from_secs(5),
        || agente(&srv.socket, id)["state"] == "working"
    ));
    m.listar_com("ok", json!({"delay_ms": 10000}));
    let fim = Instant::now() + Duration::from_secs(10);
    let a = loop {
        let a = agente(&srv.socket, id);
        if marcas(&a).contains("stale") || Instant::now() >= fim {
            break a;
        }
        // Antes do primeiro timeout o estado ainda e o anterior; com error e sem stale, unknown.
        if tem_erro(&a) && !marcas(&a).contains("stale") {
            assert_eq!(
                a["state"], "unknown",
                "R13: leitura travada com error deve ser unknown antes de stale: {a}"
            );
        }
        std::thread::sleep(Duration::from_millis(20));
    };
    assert!(marcas(&a).contains("stale"), "R13: nunca ficou stale: {a}");
    assert_eq!(a["state"], "unknown", "R13: stale vira unknown: {a}");
    assert_eq!(
        a["last_known"], "working",
        "R13: last_known deve guardar o anterior: {a}"
    );
    let idade_ms = ms_unix().saturating_sub(a["observed_at"].as_u64().unwrap_or(0));
    assert!(
        idade_ms + 150 >= stale_ms,
        "R13: stale antes de stale_after_ms (idade {idade_ms} ms, limite {stale_ms} ms)"
    );
    // Leitura valida volta e limpa stale/error.
    m.listar("ok");
    let b = espera_agente(&srv.socket, id, Duration::from_secs(6), |x| {
        !marcas(x).contains("stale") && x["state"] == "working"
    });
    assert_eq!(
        b["state"], "working",
        "R13: a leitura valida seguinte deve voltar a working: {b}"
    );
    assert!(
        !marcas(&b).contains("stale") && !marcas(&b).contains("error"),
        "R13: {b}"
    );
}

//! Testes independentes da rev6 para a fonte `codex_app_server` (spec-rev6 secoes 0, 2, 3, 4, 6, 7 e o
//! contrato S1 a S6 da secao 11). Caixa preta: sobe o binario `agentd`, um tmux FALSO (rev5_common) e um
//! servidor WebSocket FALSO do app-server (rev6_common/fake_codex.rs), que registra cada metodo e caminho.
//! Nao le `src/`, nao toca o Codex real, `~/.codex`, `~/.config` ou a rede. Ambiguidades e suposicoes:
//! `tests/REV6-TESTES.md`.
//!
//! Rodar: cargo test --offline -p agentd --test rev6_codex
//! (Os testes `harness6_*` validam o servidor falso e passam sem o daemon; os demais falham ate a rev6.)

mod rev6_common;

use rev6_common::*;
use serde_json::{Value, json};
use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;
use std::thread;
use std::time::{Duration, Instant};

const TOL_GAP: Duration = Duration::from_millis(500);

/// Mundo com o painel do Codex (`%2`, tag `codex-blocks`) e a tela `tela`.
fn mundo_codex(tela: &[u8]) -> Mundo {
    let mut m = Mundo::novo();
    m.paineis(&[Painel::new("%2", "codex").tag("codex-blocks").pid("5002")]);
    m.tela("%2", tela);
    m
}

/// Sobe o daemon com `codex-blocks` (fonte tmux + app_server com o thread_file valido `UUID`).
/// `ajuste` altera a configuracao antes de subir.
fn sobe_codex(m: &Mundo, fake: &FakeCodex, ajuste: impl FnOnce(&mut Value)) -> Servidor {
    let tf = thread_file(m, "thread-codex", UUID.as_bytes(), 0o600);
    let mut c = cfg6(
        m,
        json!({"codex-blocks": ag_codex("codex-blocks", &tf)}),
        &fake.socket,
    );
    ajuste(&mut c);
    sobe6(m, &c)
}

// ---------- autotestes do servidor falso (passam sem o daemon) ----------

#[test]
fn harness6_aceite_websocket_segue_a_rfc6455() {
    // Exemplo oficial da RFC 6455, secao 1.3.
    assert_eq!(
        aceite_websocket("dGhlIHNhbXBsZSBub25jZQ=="),
        "s3pPLMBiTxaQ9kYGzzhZRbK+xOo="
    );
}

#[test]
fn harness6_servidor_falso_faz_handshake_responde_initialize_e_registra_caminho() {
    let dir = tempfile::tempdir().expect("diretorio temporario");
    let fake = FakeCodex::inicia(dir.path(), UUID, json!({"type": "idle"}));
    let mut s = UnixStream::connect(&fake.socket).expect("conectar");
    s.write_all(
        b"GET / HTTP/1.1\r\nHost: localhost\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==\r\nSec-WebSocket-Version: 13\r\n\r\n",
    )
    .expect("enviar upgrade");
    let mut leitor = BufReader::new(s.try_clone().expect("clonar"));
    let mut linha = String::new();
    leitor.read_line(&mut linha).expect("status");
    assert!(linha.starts_with("HTTP/1.1 101"), "resposta: {linha}");
    let mut accept = String::new();
    loop {
        let mut l = String::new();
        leitor.read_line(&mut l).expect("cabecalho");
        let t = l.trim_end();
        if t.is_empty() {
            break;
        }
        if let Some(v) = t.strip_prefix("Sec-WebSocket-Accept: ") {
            accept = v.to_string();
        }
    }
    assert_eq!(accept, "s3pPLMBiTxaQ9kYGzzhZRbK+xOo=");
    let msg = br#"{"id":7,"method":"initialize","params":{"capabilities":{}}}"#;
    let mut quadro = vec![0x81, 0x80 | msg.len() as u8, 1, 2, 3, 4];
    quadro.extend(
        msg.iter()
            .enumerate()
            .map(|(i, b)| b ^ [1u8, 2, 3, 4][i % 4]),
    );
    s.write_all(&quadro).expect("enviar quadro");
    let (op, p) = le_quadro(&mut leitor).expect("ler resposta");
    assert_eq!(op, 1);
    let v: Value = serde_json::from_slice(&p).expect("JSON");
    assert_eq!(v["id"], 7);
    assert!(v.get("result").is_some(), "{v}");
    assert!(fake.metodos().contains(&"initialize".to_string()));

    let mut s2 = UnixStream::connect(&fake.socket).expect("conectar 2");
    s2.write_all(
        b"GET /daemon/shutdown HTTP/1.1\r\nHost: x\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==\r\nSec-WebSocket-Version: 13\r\n\r\n",
    )
    .expect("enviar upgrade 2");
    let mut r2 = String::new();
    BufReader::new(s2).read_line(&mut r2).expect("status 2");
    assert!(r2.starts_with("HTTP/1.1 404"), "{r2}");
    assert!(fake.caminhos().contains(&"/daemon/shutdown".to_string()));
}

// ---------- S1 a S6 ----------

#[test]
fn s1_idle_active_aprovacao_idle_publicam_em_menos_de_1_s_com_source_app_server() {
    let m = mundo_codex(TELA_SEM_REGRA);
    let fake = FakeCodex::inicia(m.raiz.path(), UUID, json!({"type": "idle"}));
    let srv = sobe_codex(&m, &fake, |_| {});
    let sk = srv.socket.clone();

    // Partida: thread/read traz `idle`; a tela sem regra nao contradiz, entao `idle` publica.
    let a = publica_em(&sk, "codex-blocks", "idle", Duration::from_secs(5));
    assert_eq!(a["source"], "app_server", "{a}");
    assert_eq!(
        a["evidence"], "reported",
        "tela sem regra nao e concordancia: {a}"
    );
    assert!(
        espera(Duration::from_secs(2), || fake.conta("thread/read") >= 1),
        "o daemon nao chamou thread/read"
    );

    fake.muda(json!({"type": "active", "activeFlags": []}));
    let a = publica_em(&sk, "codex-blocks", "working", SEG);
    assert_eq!(a["source"], "app_server", "{a}");

    fake.muda(json!({"type": "active", "activeFlags": ["waitingOnApproval"]}));
    let a = publica_em(&sk, "codex-blocks", "blocked", SEG);
    assert_eq!(a["blocked_reason"], "permission", "{a}");
    assert_eq!(a["source"], "app_server", "{a}");

    fake.muda(json!({"type": "idle"}));
    let a = publica_em(&sk, "codex-blocks", "idle", SEG);
    assert_eq!(a["source"], "app_server", "{a}");
    assert!(
        a["blocked_reason"].is_null(),
        "blocked_reason so existe em blocked: {a}"
    );
}

#[test]
fn s1b_notificacao_de_outra_thread_e_ignorada_e_a_da_configurada_vale() {
    // Contrato: "depois apenas escuta thread/status/changed da thread configurada" (secao 4).
    let m = mundo_codex(TELA_SEM_REGRA);
    let fake = FakeCodex::inicia(m.raiz.path(), UUID, json!({"type": "idle"}));
    let srv = sobe_codex(&m, &fake, |_| {});
    let sk = srv.socket.clone();
    publica_em(&sk, "codex-blocks", "idle", Duration::from_secs(5));

    fake.notifica(UUID_OUTRA, json!({"type": "active", "activeFlags": []}));
    assert!(
        !ve_estado(&sk, "codex-blocks", "working", Duration::from_millis(800)),
        "notificacao de outra thread mudou o estado"
    );
    // Controle positivo: a notificacao da thread configurada muda o estado no mesmo daemon.
    fake.muda(json!({"type": "active", "activeFlags": []}));
    publica_em(&sk, "codex-blocks", "working", SEG);
}

#[test]
fn s2_waiting_on_user_input_vira_blocked_com_reason_user_input() {
    let m = mundo_codex(TELA_SEM_REGRA);
    let fake = FakeCodex::inicia(m.raiz.path(), UUID, json!({"type": "idle"}));
    let srv = sobe_codex(&m, &fake, |_| {});
    let sk = srv.socket.clone();
    publica_em(&sk, "codex-blocks", "idle", Duration::from_secs(5));

    fake.muda(json!({"type": "active", "activeFlags": ["waitingOnUserInput"]}));
    let a = publica_em(&sk, "codex-blocks", "blocked", SEG);
    assert_eq!(a["blocked_reason"], "user_input", "{a}");
    assert_eq!(a["source"], "app_server", "{a}");
}

#[test]
fn s3_status_de_erro_e_tipo_desconhecido_viram_unknown_e_nunca_idle_mesmo_com_tela_idle() {
    // Tela `codex/idle` e POSITIVA: se o daemon usasse a tela para publicar `idle` aqui, falharia.
    let mut m = Mundo::novo();
    m.paineis(&[Painel::new("%2", "codex").tag("codex-blocks").pid("5002")]);
    m.tela_fixture("%2", "codex", "idle");
    let fake = FakeCodex::inicia(
        m.raiz.path(),
        UUID,
        json!({"type": "active", "activeFlags": []}),
    );
    let srv = sobe_codex(&m, &fake, |_| {});
    let sk = srv.socket.clone();
    publica_em(&sk, "codex-blocks", "working", Duration::from_secs(5));

    for st in [
        json!({"type": "systemError"}),
        json!({"type": "tipo-inventado-pela-spec"}),
    ] {
        let rotulo = st.to_string();
        fake.muda(st);
        publica_em(&sk, "codex-blocks", "unknown", SEG);
        assert!(
            !ve_estado(&sk, "codex-blocks", "idle", Duration::from_millis(1200)),
            "status {rotulo} publicou idle"
        );
    }
    // Controle positivo: o `idle` do app-server publica `idle` com a mesma tela.
    fake.muda(json!({"type": "idle"}));
    publica_em(&sk, "codex-blocks", "idle", SEG);
}

#[test]
fn s3c_active_flag_inesperada_vira_unknown_e_nunca_idle() {
    // Secao 4: "qualquer outro tipo ou campo inesperado => unknown (nunca idle)". Tela idle positiva.
    let mut m = Mundo::novo();
    m.paineis(&[Painel::new("%2", "codex").tag("codex-blocks").pid("5002")]);
    m.tela_fixture("%2", "codex", "idle");
    let fake = FakeCodex::inicia(
        m.raiz.path(),
        UUID,
        json!({"type": "active", "activeFlags": []}),
    );
    let srv = sobe_codex(&m, &fake, |_| {});
    let sk = srv.socket.clone();
    publica_em(&sk, "codex-blocks", "working", Duration::from_secs(5));

    fake.muda(json!({"type": "active", "activeFlags": ["flag-inventada-pela-spec"]}));
    publica_em(&sk, "codex-blocks", "unknown", SEG);
    assert!(
        !ve_estado(&sk, "codex-blocks", "idle", Duration::from_millis(1200)),
        "activeFlags inesperada publicou idle"
    );
}

#[test]
fn s3b_not_loaded_vira_unknown_com_structured_lost_e_nunca_idle() {
    // ATENCAO (ambiguidade registrada em REV6-TESTES.md): a secao 4 manda `notLoaded` para `unknown` com
    // marca `structured_lost`, e a regra 1 da fusao manda a tela decidir sem estruturada ativa. Com tela
    // idle positiva, a regra 1 publicaria `idle`. Este teste segue o contrato S3 ("nunca idle").
    let mut m = Mundo::novo();
    m.paineis(&[Painel::new("%2", "codex").tag("codex-blocks").pid("5002")]);
    m.tela_fixture("%2", "codex", "idle");
    let fake = FakeCodex::inicia(
        m.raiz.path(),
        UUID,
        json!({"type": "active", "activeFlags": []}),
    );
    let srv = sobe_codex(&m, &fake, |_| {});
    let sk = srv.socket.clone();
    publica_em(&sk, "codex-blocks", "working", Duration::from_secs(5));

    fake.muda(json!({"type": "notLoaded"}));
    let a = espera_agente(&sk, "codex-blocks", SEG, |a| {
        tem_marca(a, "structured_lost")
    });
    assert!(
        tem_marca(&a, "structured_lost"),
        "notLoaded sem marca structured_lost: {a}"
    );
    assert_eq!(a["state"], "unknown", "notLoaded deve ser unknown: {a}");
    assert!(
        !ve_estado(&sk, "codex-blocks", "idle", Duration::from_millis(1200)),
        "notLoaded publicou idle"
    );
}

#[test]
fn s4_lista_fechada_so_initialize_initialized_loaded_list_e_read_na_raiz() {
    let m = mundo_codex(TELA_SEM_REGRA);
    let fake = FakeCodex::inicia(m.raiz.path(), UUID, json!({"type": "idle"}));
    let _srv = sobe_codex(&m, &fake, |_| {});
    assert!(espera(Duration::from_secs(5), || fake.conta("thread/read") >= 1));

    // Muitas mudancas, uma queda e a reconexao: a lista fechada vale em todo o percurso.
    for st in [
        json!({"type": "active", "activeFlags": []}),
        json!({"type": "active", "activeFlags": ["waitingOnApproval"]}),
        json!({"type": "active", "activeFlags": ["waitingOnUserInput"]}),
        json!({"type": "idle"}),
    ] {
        fake.muda(st);
        thread::sleep(Duration::from_millis(150));
    }
    fake.derruba();
    thread::sleep(Duration::from_millis(400));
    fake.religa();
    assert!(
        espera(Duration::from_secs(6), || fake.conta("thread/read") >= 2),
        "nao releu o estado inicial depois de reconectar"
    );
    thread::sleep(Duration::from_millis(500));

    let permitidos = [
        "initialize",
        "initialized",
        "thread/loaded/list",
        "thread/read",
    ];
    let vistos = fake.metodos();
    for met in &vistos {
        assert!(
            permitidos.contains(&met.as_str()),
            "metodo fora da lista fechada: {met}"
        );
    }
    // Controle positivo: a sequencia de partida realmente aconteceu.
    for met in permitidos {
        assert!(
            vistos.iter().any(|v| v == met),
            "faltou {met} (controle): {vistos:?}"
        );
    }
    let caminhos = fake.caminhos();
    assert!(!caminhos.is_empty(), "nenhum caminho HTTP registrado");
    for c in &caminhos {
        assert_eq!(c, "/", "caminho proibido: {c}");
    }
    let pedidos = fake.pedidos();
    for p in pedidos.iter().filter(|p| p.metodo == "thread/read") {
        assert_eq!(
            p.params["includeTurns"],
            json!(false),
            "thread/read sem includeTurns:false: {}",
            p.params
        );
        assert_eq!(
            p.params["threadId"], UUID,
            "thread/read de thread nao configurada"
        );
    }
    let init = pedidos
        .iter()
        .position(|p| p.metodo == "initialize")
        .expect("initialize");
    let inited = pedidos
        .iter()
        .position(|p| p.metodo == "initialized")
        .expect("initialized");
    assert!(inited > init, "initialized antes de initialize");
    let p_init = &pedidos[init];
    assert!(p_init.id.is_some(), "initialize sem id");
    assert!(
        pedidos[inited].id.is_none(),
        "initialized deve ser notificacao (sem id)"
    );
    assert!(
        p_init.params["capabilities"]["experimentalApi"].is_null(),
        "initialize com experimentalApi: {}",
        p_init.params
    );
}

#[test]
fn s5_queda_do_socket_cai_para_a_tela_e_reconecta_relendo_o_estado() {
    let mut m = Mundo::novo();
    m.paineis(&[Painel::new("%2", "codex").tag("codex-blocks").pid("5002")]);
    m.tela("%2", TELA_SEM_REGRA);
    let fake = FakeCodex::inicia(m.raiz.path(), UUID, json!({"type": "idle"}));
    let srv = sobe_codex(&m, &fake, |c| {
        c["codex_reconnect_min_s"] = json!(1);
        c["codex_reconnect_max_s"] = json!(2);
    });
    let sk = srv.socket.clone();
    publica_em(&sk, "codex-blocks", "idle", Duration::from_secs(5));
    assert_eq!(agente(&sk, "codex-blocks")["source"], "app_server");

    fake.derruba();
    let a = espera_agente(&sk, "codex-blocks", Duration::from_secs(3), |a| {
        tem_marca(a, "structured_lost")
    });
    assert!(
        tem_marca(&a, "structured_lost"),
        "queda sem marca structured_lost: {a}"
    );

    // Sem a estruturada, a tela decide (regra 1 da fusao = rev5): trabalho e depois ocioso, ambos por tmux.
    m.tela_fixture("%2", "codex", "working-indicator");
    let a = publica_em(&sk, "codex-blocks", "working", Duration::from_secs(2));
    assert_eq!(a["source"], "tmux", "{a}");
    m.tela_fixture("%2", "codex", "idle");
    let a = publica_em(&sk, "codex-blocks", "idle", Duration::from_secs(2));
    assert_eq!(a["source"], "tmux", "{a}");

    // Enquanto o socket esta fora, o app-server muda para aprovacao sem avisar: so a releitura o ve.
    fake.muda_sem_avisar(json!({"type": "active", "activeFlags": ["waitingOnApproval"]}));
    fake.religa();
    // Teto de reconexao 2 s; folga de 1,5 s.
    let a = publica_em(&sk, "codex-blocks", "blocked", Duration::from_millis(3500));
    assert_eq!(a["blocked_reason"], "permission", "{a}");
    assert_eq!(a["source"], "app_server", "{a}");
    assert!(
        !tem_marca(&a, "structured_lost"),
        "structured_lost nao saiu apos reconectar: {a}"
    );
    assert!(
        fake.conta("thread/read") >= 2,
        "nao releu thread/read ao reconectar"
    );
}

#[test]
fn s5_tentativas_de_reconexao_respeitam_recuo_entre_min_e_max() {
    let m = mundo_codex(TELA_SEM_REGRA);
    let fake = FakeCodex::inicia(m.raiz.path(), UUID, json!({"type": "idle"}));
    let srv = sobe_codex(&m, &fake, |c| {
        c["codex_reconnect_min_s"] = json!(1);
        c["codex_reconnect_max_s"] = json!(2);
    });
    let sk = srv.socket.clone();
    publica_em(&sk, "codex-blocks", "idle", Duration::from_secs(5));
    assert!(espera(Duration::from_secs(2), || fake.conta("thread/read") >= 1));

    let t_queda = Instant::now();
    fake.derruba();
    thread::sleep(Duration::from_millis(6500));
    let ts: Vec<Instant> = fake
        .tentativas()
        .into_iter()
        .filter(|t| *t > t_queda)
        .collect();
    assert!(
        ts.len() >= 3,
        "poucas tentativas em 6,5 s com recuo entre 1 e 2 s: {}",
        ts.len()
    );
    // O primeiro intervalo e ignorado (ele pode comecar no instante da queda).
    for par in ts.windows(2).skip(1) {
        let g = par[1].duration_since(par[0]);
        assert!(
            g + TOL_GAP >= Duration::from_secs(1) && g <= Duration::from_secs(2) + TOL_GAP,
            "intervalo entre tentativas {g:?} fora de [min 1 s, max 2 s]"
        );
    }
    fake.religa();
    assert!(
        espera(Duration::from_millis(3500), || fake.conexoes() >= 1),
        "nao reconectou depois de religar"
    );
}

#[test]
fn s6_thread_file_ausente_e_structured_lost_sem_erro_fatal() {
    let mut m = Mundo::novo();
    m.paineis(&[Painel::new("%2", "codex").tag("codex-blocks").pid("5002")]);
    m.tela_fixture("%2", "codex", "working-indicator");
    let fake = FakeCodex::inicia(
        m.raiz.path(),
        UUID,
        json!({"type": "active", "activeFlags": ["waitingOnApproval"]}),
    );
    let tf = m.raiz.path().join("thread-ausente");
    let c = cfg6(
        &m,
        json!({"codex-blocks": ag_codex("codex-blocks", &tf)}),
        &fake.socket,
    );
    let srv = sobe6(&m, &c);
    let sk = srv.socket.clone();
    let a = publica_em(&sk, "codex-blocks", "working", Duration::from_secs(3));
    assert_eq!(a["source"], "tmux", "{a}");
    assert!(tem_marca(&a, "structured_lost"), "{a}");
    assert_eq!(
        fake.conta("thread/read"),
        0,
        "sem thread_file nao ha thread para ler"
    );
    // Controle: o mesmo cenario com thread_file valido vira `blocked` por app_server.
    controle_thread_valido();
}

#[test]
fn s6_thread_file_invalido_e_structured_lost() {
    // Conteudos fora do contrato: UUID em maiusculas, texto, vazio e mais de 64 bytes.
    let casos: Vec<(&str, Vec<u8>)> = vec![
        ("maiusculas", UUID.to_uppercase().into_bytes()),
        ("texto", b"nao-e-um-uuid".to_vec()),
        ("vazio", Vec::new()),
        ("longo", format!("{UUID}{}", "x".repeat(40)).into_bytes()),
    ];
    for (nome, conteudo) in casos {
        let mut m = Mundo::novo();
        m.paineis(&[Painel::new("%2", "codex").tag("codex-blocks").pid("5002")]);
        m.tela_fixture("%2", "codex", "working-indicator");
        let fake = FakeCodex::inicia(
            m.raiz.path(),
            UUID,
            json!({"type": "active", "activeFlags": ["waitingOnApproval"]}),
        );
        let tf = thread_file(&m, "thread-invalido", &conteudo, 0o600);
        let c = cfg6(
            &m,
            json!({"codex-blocks": ag_codex("codex-blocks", &tf)}),
            &fake.socket,
        );
        let srv = sobe6(&m, &c);
        let sk = srv.socket.clone();
        let a = publica_em(&sk, "codex-blocks", "working", Duration::from_secs(3));
        assert_eq!(a["source"], "tmux", "caso {nome}: {a}");
        assert!(tem_marca(&a, "structured_lost"), "caso {nome}: {a}");
        assert_eq!(
            fake.conta("thread/read"),
            0,
            "caso {nome}: thread_file invalido foi usado"
        );
    }
    controle_thread_valido();
}

#[test]
fn s6_thread_file_symlink_nao_e_seguido() {
    let mut m = Mundo::novo();
    m.paineis(&[Painel::new("%2", "codex").tag("codex-blocks").pid("5002")]);
    m.tela_fixture("%2", "codex", "working-indicator");
    let fake = FakeCodex::inicia(
        m.raiz.path(),
        UUID,
        json!({"type": "active", "activeFlags": ["waitingOnApproval"]}),
    );
    let alvo = thread_file(&m, "thread-alvo", UUID.as_bytes(), 0o600);
    let caminho_link = m.raiz.path().join("thread-link");
    link(&alvo, &caminho_link);
    let c = cfg6(
        &m,
        json!({"codex-blocks": ag_codex("codex-blocks", &caminho_link)}),
        &fake.socket,
    );
    let srv = sobe6(&m, &c);
    let sk = srv.socket.clone();
    let a = publica_em(&sk, "codex-blocks", "working", Duration::from_secs(3));
    assert_eq!(a["source"], "tmux", "{a}");
    assert!(tem_marca(&a, "structured_lost"), "{a}");
    assert_eq!(
        fake.conta("thread/read"),
        0,
        "symlink foi seguido ate o alvo"
    );
    // Controle: o mesmo UUID num arquivo comum 0600 funciona.
    controle_thread_valido();
}

#[test]
fn s6_thread_file_gravavel_por_grupo_ou_outros_e_structured_lost() {
    for modo in [0o620u32, 0o602, 0o666] {
        let mut m = Mundo::novo();
        m.paineis(&[Painel::new("%2", "codex").tag("codex-blocks").pid("5002")]);
        m.tela_fixture("%2", "codex", "working-indicator");
        let fake = FakeCodex::inicia(
            m.raiz.path(),
            UUID,
            json!({"type": "active", "activeFlags": ["waitingOnApproval"]}),
        );
        let tf = thread_file(&m, "thread-aberto", UUID.as_bytes(), modo);
        let c = cfg6(
            &m,
            json!({"codex-blocks": ag_codex("codex-blocks", &tf)}),
            &fake.socket,
        );
        let srv = sobe6(&m, &c);
        let sk = srv.socket.clone();
        let a = publica_em(&sk, "codex-blocks", "working", Duration::from_secs(3));
        assert_eq!(a["source"], "tmux", "modo {modo:o}: {a}");
        assert!(tem_marca(&a, "structured_lost"), "modo {modo:o}: {a}");
        assert_eq!(
            fake.conta("thread/read"),
            0,
            "modo {modo:o}: thread_file gravavel foi usado"
        );
    }
    controle_thread_valido();
}

#[test]
fn s6_thread_file_criado_depois_e_relido_em_thread_file_reread_s() {
    let mut m = Mundo::novo();
    m.paineis(&[Painel::new("%2", "codex").tag("codex-blocks").pid("5002")]);
    m.tela_fixture("%2", "codex", "working-indicator");
    let fake = FakeCodex::inicia(
        m.raiz.path(),
        UUID,
        json!({"type": "active", "activeFlags": ["waitingOnApproval"]}),
    );
    let tf = m.raiz.path().join("thread-depois");
    let mut c = cfg6(
        &m,
        json!({"codex-blocks": ag_codex("codex-blocks", &tf)}),
        &fake.socket,
    );
    c["thread_file_reread_s"] = json!(5);
    let srv = sobe6(&m, &c);
    let sk = srv.socket.clone();
    publica_em(&sk, "codex-blocks", "working", Duration::from_secs(3));
    assert_eq!(fake.conta("thread/read"), 0);

    thread_file(&m, "thread-depois", UUID.as_bytes(), 0o600);
    // thread_file_reread_s = 5 s; folga de 3 s.
    let a = espera_agente(&sk, "codex-blocks", Duration::from_secs(8), |a| {
        a["source"] == "app_server"
    });
    assert_eq!(
        a["blocked_reason"], "permission",
        "arquivo criado depois nao foi relido: {a}"
    );
    assert!(!tem_marca(&a, "structured_lost"), "{a}");
}

#[test]
fn s6_thread_fora_da_lista_de_carregadas_e_structured_lost() {
    let mut m = Mundo::novo();
    m.paineis(&[Painel::new("%2", "codex").tag("codex-blocks").pid("5002")]);
    m.tela_fixture("%2", "codex", "working-indicator");
    let fake = FakeCodex::inicia(
        m.raiz.path(),
        UUID,
        json!({"type": "active", "activeFlags": ["waitingOnApproval"]}),
    );
    fake.define_carregadas(&[UUID_OUTRA]);
    let tf = thread_file(&m, "thread-fora", UUID.as_bytes(), 0o600);
    let c = cfg6(
        &m,
        json!({"codex-blocks": ag_codex("codex-blocks", &tf)}),
        &fake.socket,
    );
    let srv = sobe6(&m, &c);
    let sk = srv.socket.clone();
    let a = publica_em(&sk, "codex-blocks", "working", Duration::from_secs(3));
    assert_eq!(a["source"], "tmux", "{a}");
    assert!(tem_marca(&a, "structured_lost"), "{a}");
    assert!(
        fake.conta("thread/loaded/list") >= 1,
        "nao consultou thread/loaded/list"
    );
    controle_thread_valido();
}

/// Controle positivo de S6: thread_file valido (0600, UUID minusculo) e a thread carregada => `blocked`
/// por `app_server`, sem `structured_lost`. Sem isto, um daemon que ignora sempre a fonte passaria.
fn controle_thread_valido() {
    let mut m = Mundo::novo();
    m.paineis(&[Painel::new("%2", "codex").tag("codex-blocks").pid("5002")]);
    m.tela_fixture("%2", "codex", "working-indicator");
    let fake = FakeCodex::inicia(
        m.raiz.path(),
        UUID,
        json!({"type": "active", "activeFlags": ["waitingOnApproval"]}),
    );
    let tf = thread_file(&m, "thread-controle", UUID.as_bytes(), 0o600);
    let c = cfg6(
        &m,
        json!({"codex-blocks": ag_codex("codex-blocks", &tf)}),
        &fake.socket,
    );
    let srv = sobe6(&m, &c);
    let sk = srv.socket.clone();
    let a = publica_em(&sk, "codex-blocks", "blocked", Duration::from_secs(3));
    assert_eq!(a["source"], "app_server", "controle de S6: {a}");
    assert!(!tem_marca(&a, "structured_lost"), "controle de S6: {a}");
    assert!(
        fake.conta("thread/read") >= 1,
        "controle de S6: nao leu a thread"
    );
}

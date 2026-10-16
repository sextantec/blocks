//! Testes de protocolo, CLI, atomicidade, assinante lento e reinicio, migrados de `agentd_v1.rs` (herdr)
//! para o tmux falso (spec rev5, R14; grupos G5, G6, G9, G10, G12, G14, G20 e G22 da rev4 secao 10).
//! Tambem o formato de `Agente`, `epoch`, `unknown_agent`, ordem do `agent.list` e forma do `status`
//! (contrato de protocolo da rev4 secao 5; sem estado de herdr).
//! Caixa preta: binario, socket e CLI; o tmux e o falso de `rev5_common`. Nada de rede, nada de agente real.
//! Onde a spec nao fixa algo, o teste afirma so o garantido; o restante esta em REV5-TESTES.md (rodada 5).

mod rev5_common;

use rev5_common::*;
use serde_json::{Value, json};
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

const CLAUDE: &str = "claude";
const CODEX: &str = "codex-blocks";

/// Daemon com um agente `claude` (painel %1, tag claude) numa tela de `working`.
fn sobe_padrao(poll_ms: u64, ajuste: impl FnOnce(&mut Value)) -> (Mundo, Servidor) {
    let mut m = Mundo::novo();
    m.paineis(&[Painel::new("%1", "claude").tag("claude")]);
    m.tela_fixture("%1", "claude", "working-generation");
    let srv = inicia(&m, poll_ms, &[("claude", "claude", "claude")], ajuste);
    (m, srv)
}

/// Assina eventos desde o inicio: epoch do `agent.list`, depois `events.resume` com last_seq 0.
fn assina(socket: &Path) -> Conexao {
    let epoch = res(&rpc(socket, "agent.list", json!({"v": 1})))["epoch"].clone();
    let mut c = Conexao::abre(socket);
    let r = c.chama(
        "events.resume",
        json!({"v": 1, "epoch": epoch, "last_seq": 0}),
    );
    assert!(!is_err(&r), "events.resume desde o inicio falhou: {r}");
    c
}

/// Coleta eventos ate `d` decorrer ou o servidor parar de enviar.
fn coleta(c: &mut Conexao, d: Duration) -> Vec<Value> {
    let fim = Instant::now() + d;
    let mut v = Vec::new();
    while Instant::now() < fim {
        let restante = fim.saturating_duration_since(Instant::now());
        match c.evento(restante) {
            Some(e) => v.push(e),
            None => break,
        }
    }
    v
}

/// Troca a tela de todos os paineis `%1..%n` por `nome` (fixture de claude).
fn flip_all(m: &mut Mundo, n: usize, nome: &str) {
    for i in 1..=n {
        m.tela_fixture(&format!("%{i}"), "claude", nome);
    }
}

/// Lista de agentes `a00..a{n-1}` (todos claude, tag igual ao id), com paineis %1..%n em `working`.
fn sobe_32(ring: u64) -> (Mundo, Servidor) {
    let owned: Vec<(String, String, String)> = (0..32)
        .map(|i| (format!("a{i:02}"), "claude".to_string(), format!("a{i:02}")))
        .collect();
    let agentes: Vec<(&str, &str, &str)> = owned
        .iter()
        .map(|(a, b, c)| (a.as_str(), b.as_str(), c.as_str()))
        .collect();
    let mut m = Mundo::novo();
    let ps: Vec<Painel> = (0..32)
        .map(|i| Painel::new(&format!("%{}", i + 1), "claude").tag(&format!("a{i:02}")))
        .collect();
    m.paineis(&ps);
    flip_all(&mut m, 32, "working-generation");
    let srv = inicia(&m, 200, &agentes, move |c| {
        c["ring"] = json!(ring);
    });
    (m, srv)
}

/// Espera ate todos os agentes `a00..a{n-1}` publicarem `estado` (um ciclo com 32 paineis leva ~0,5 s).
fn espera_todos(socket: &Path, n: usize, estado: &str, d: Duration) -> bool {
    espera(d, || {
        let l = lista(socket);
        l.len() == n && l.iter().all(|a| a["state"] == estado)
    })
}

/// `status` do `agent.get` como estado textual.
fn estado_str(a: &Value) -> String {
    a["state"].as_str().unwrap_or("?").to_string()
}

/// Roda a CLI com variaveis de ambiente extras (`AGENTD_SOCKET`), sem depender do ambiente do teste.
fn roda_com_env(home: &Path, args: &[&str], envs: &[(&str, &str)]) -> Saida {
    let mut cmd = Command::new(BIN);
    cmd.args(args)
        .env_remove("AGENTD_SOCKET")
        .env_remove("AGENTD_STATE_DIR")
        .env("HOME", home)
        .env("XDG_CONFIG_HOME", home.join("xdg"))
        .envs(envs.iter().copied())
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = cmd.spawn().expect("executar agentd");
    let so = child.stdout.take().unwrap();
    let se = child.stderr.take().unwrap();
    let h1 = std::thread::spawn(move || {
        let mut b = String::new();
        let _ = std::io::Read::read_to_string(&mut std::io::BufReader::new(so), &mut b);
        b
    });
    let h2 = std::thread::spawn(move || {
        let mut b = String::new();
        let _ = std::io::Read::read_to_string(&mut std::io::BufReader::new(se), &mut b);
        b
    });
    let fim = Instant::now() + Duration::from_secs(10);
    let code = loop {
        if let Ok(Some(st)) = child.try_wait() {
            break st.code();
        }
        if Instant::now() >= fim {
            let _ = child.kill();
            let _ = child.wait();
            break None;
        }
        std::thread::sleep(Duration::from_millis(10));
    };
    Saida {
        code,
        stdout: h1.join().unwrap(),
        stderr: h2.join().unwrap(),
    }
}

fn cli_d(home: &Path, args: &[&str]) -> Saida {
    roda(home, None, args, Duration::from_secs(10))
}

// ---------- G5: eventos ----------

#[test]
fn r14_g5_seq_sem_lacunas_e_previous_state_encadeado() {
    let (mut m, srv) = sobe_padrao(200, |_| {});
    assert!(
        espera_estado(&srv.socket, CLAUDE, "working", Duration::from_secs(5))["state"] == "working"
    );
    let mut ev = assina(&srv.socket);
    m.tela_fixture("%1", "claude", "permission-bash");
    assert!(
        espera_estado(&srv.socket, CLAUDE, "blocked", Duration::from_secs(5))["state"] == "blocked"
    );
    m.tela_fixture("%1", "claude", "working-generation");
    assert!(
        espera_estado(&srv.socket, CLAUDE, "working", Duration::from_secs(5))["state"] == "working"
    );
    let evs = coleta(&mut ev, Duration::from_secs(1));
    let mine: Vec<&Value> = evs.iter().filter(|e| e["id"] == CLAUDE).collect();
    assert!(
        mine.len() >= 3,
        "esperava >= 3 eventos de {CLAUDE} (unknown->working, working->blocked, blocked->working): {evs:?}"
    );
    for w in mine.windows(2) {
        assert_eq!(
            w[1]["seq"].as_u64().unwrap(),
            w[0]["seq"].as_u64().unwrap() + 1,
            "lacuna em seq: {evs:?}"
        );
        assert_eq!(
            w[1]["previous_state"], w[0]["state"],
            "previous_state nao encadeia: {evs:?}"
        );
    }
    for e in &mine {
        assert_eq!(e["type"], "state_changed", "{e}");
        assert!(e["previous_marks"].is_array(), "{e}");
        assert!(e["epoch"].is_string(), "epoch como string: {e}");
    }
    assert_eq!(mine[0]["previous_state"], "unknown", "{mine:?}");
    assert_eq!(mine[0]["state"], "working", "{mine:?}");
    assert_eq!(
        mine[0]["seq"], 1,
        "o primeiro seq de uma epoch e 1 (rev4 13.1): {mine:?}"
    );
    assert_eq!(mine[1]["previous_state"], "working", "{mine:?}");
    assert_eq!(mine[1]["state"], "blocked", "{mine:?}");
}

#[test]
fn r14_g5_primeira_leitura_valida_gera_evento_com_evidence_observed_e_source_tmux() {
    // Antes da primeira leitura valida: evidence none, source null (rev4 13.4). A primeira leitura valida muda
    // evidence e source, e por isso gera evento (rev4 regra 4).
    let mut m = Mundo::novo();
    m.paineis(&[Painel::new("%1", "claude").tag("claude")]);
    m.tela_fixture("%1", "claude", "working-generation");
    m.listar("falha");
    let srv = inicia(&m, 200, &[("claude", "claude", "claude")], |_| {});
    assert!(
        espera(Duration::from_secs(4), || m.listagens() >= 2),
        "poucos ciclos"
    );
    let antes = agente(&srv.socket, CLAUDE);
    assert_eq!(
        antes["evidence"], "none",
        "antes da primeira leitura valida: {antes}"
    );
    assert!(antes["source"].is_null(), "{antes}");
    let mut ev = assina(&srv.socket);
    m.listar("ok");
    assert!(
        espera_estado(&srv.socket, CLAUDE, "working", Duration::from_secs(5))["state"] == "working"
    );
    let evs = coleta(&mut ev, Duration::from_secs(1));
    assert!(
        evs.iter()
            .any(|e| e["id"] == CLAUDE && e["evidence"] == "observed" && e["source"] == "tmux"),
        "esperava evento com evidence observed e source tmux: {evs:?}"
    );
    assert_eq!(agente(&srv.socket, CLAUDE)["source"], "tmux");
}

#[test]
fn r14_g5_observed_at_freshness_e_cwd_nao_geram_evento() {
    // Sem mudanca de estado, observed_at muda a cada ciclo e o caminho do painel tambem: nenhum evento novo.
    let (mut m, srv) = sobe_padrao(200, |_| {});
    assert!(
        espera_estado(&srv.socket, CLAUDE, "working", Duration::from_secs(5))["state"] == "working"
    );
    let head = status(&srv.socket)["head_seq"].as_u64().unwrap();
    let obs0 = agente(&srv.socket, CLAUDE)["observed_at"].clone();
    for i in 0..8 {
        m.paineis(&[Painel::new("%1", "claude")
            .tag("claude")
            .cwd(&format!("/projeto/{i}"))]);
        std::thread::sleep(Duration::from_millis(250));
    }
    let obs1 = agente(&srv.socket, CLAUDE)["observed_at"].clone();
    assert_ne!(obs0, obs1, "controle: o ciclo rodou (observed_at mudou)");
    let novos: Vec<Value> = eventos_desde(&srv.socket, head)
        .into_iter()
        .filter(|e| e["id"] == CLAUDE)
        .collect();
    assert!(
        novos.is_empty(),
        "observed_at/cwd/freshness nao geram evento: {novos:?}"
    );
}

// ---------- G6: events.resume ----------

#[test]
fn r14_g6_retoma_valido_com_last_seq_igual_ao_head() {
    let (mut m, srv) = sobe_padrao(200, |_| {});
    espera_estado(&srv.socket, CLAUDE, "working", Duration::from_secs(5));
    let st = status(&srv.socket);
    let head = st["head_seq"].as_u64().unwrap();
    let epoch = st["epoch"].clone();
    let mut c = Conexao::abre(&srv.socket);
    let r = c.chama(
        "events.resume",
        json!({"v": 1, "epoch": epoch, "last_seq": head}),
    );
    assert_eq!(res(&r)["head_seq"].as_u64().unwrap(), head, "{r}");
    m.tela_fixture("%1", "claude", "permission-bash");
    let e = coleta(&mut c, Duration::from_secs(5))
        .into_iter()
        .find(|e| e["id"] == CLAUDE)
        .expect("evento ao vivo");
    assert_eq!(e["seq"].as_u64().unwrap(), head + 1, "{e}");
    assert_eq!(e["epoch"], epoch, "{e}");
}

#[test]
fn r14_g6_resume_gap_informa_oldest_seq_e_fronteira_exata() {
    // Anel de 64 (minimo valido). 32 agentes trocam de estado 3 vezes: ~128 eventos, o anel fica cheio.
    let (mut m, srv) = sobe_32(64);
    espera_estado(&srv.socket, "a31", "working", Duration::from_secs(5));
    for (nome, alvo) in [
        ("permission-bash", "blocked"),
        ("working-generation", "working"),
        ("permission-bash", "blocked"),
    ] {
        flip_all(&mut m, 32, nome);
        assert!(
            espera_todos(&srv.socket, 32, alvo, Duration::from_secs(15)),
            "troca para {alvo} nao convergiu"
        );
    }
    let l = res(&rpc(&srv.socket, "agent.list", json!({"v": 1})));
    let head = l["head_seq"].as_u64().unwrap();
    let epoch = l["epoch"].clone();
    assert!(head >= 64 + 10, "anel nao foi lotado: head {head}");
    // Rev4 13.5: com o anel cheio, oldest = head - ring + 1.
    let oldest = head - 64 + 1;
    let mut g = Conexao::abre(&srv.socket);
    let r = g.chama(
        "events.resume",
        json!({"v": 1, "epoch": epoch, "last_seq": 0}),
    );
    assert_eq!(cod(&r), "resume_gap", "{r}");
    assert_eq!(
        r["error"]["data"]["oldest_seq"].as_u64(),
        Some(oldest),
        "{r}"
    );
    // Fronteira: oldest - 1 retoma e o primeiro evento recebido tem seq == oldest.
    let mut ok = Conexao::abre(&srv.socket);
    let r1 = ok.chama(
        "events.resume",
        json!({"v": 1, "epoch": epoch, "last_seq": oldest - 1}),
    );
    assert!(!is_err(&r1), "oldest-1 deveria retomar: {r1}");
    let e = ok.evento(Duration::from_secs(3)).expect("replay do anel");
    assert_eq!(e["seq"].as_u64(), Some(oldest), "{e}");
    // oldest - 2 e gap.
    let mut g2 = Conexao::abre(&srv.socket);
    let r2 = g2.chama(
        "events.resume",
        json!({"v": 1, "epoch": epoch, "last_seq": oldest - 2}),
    );
    assert_eq!(cod(&r2), "resume_gap", "{r2}");
}

#[test]
fn r14_g6_epoch_mismatch_apos_reinicio() {
    let (mut m, mut srv) = sobe_padrao(200, |_| {});
    espera_estado(&srv.socket, CLAUDE, "working", Duration::from_secs(5));
    let antiga = res(&rpc(&srv.socket, "agent.list", json!({"v": 1})))["epoch"].clone();
    assert_eq!(
        srv.para(),
        Some(0),
        "SIGTERM deve sair limpo com saida 0 (rev4 13.2)"
    );
    let _ = fs::remove_file(&srv.socket);
    // Sem o painel no cenario antes de subir de novo: nada do estado anterior pode voltar.
    m.paineis(&[]);
    let cfg = m.raiz.path().join("config.json");
    let srv2 = sobe(&estado_dir(&m), &cfg);
    let nova = res(&rpc(&srv2.socket, "agent.list", json!({"v": 1})))["epoch"].clone();
    assert_ne!(nova, antiga, "epoch deve mudar a cada partida");
    let r = rpc(
        &srv2.socket,
        "events.resume",
        json!({"v": 1, "epoch": antiga, "last_seq": 0}),
    );
    assert_eq!(cod(&r), "epoch_mismatch", "{r}");
    // Controle: com a epoch nova o mesmo pedido retoma (senao um daemon que erra sempre passaria).
    let ok = rpc(
        &srv2.socket,
        "events.resume",
        json!({"v": 1, "epoch": nova.clone(), "last_seq": 0}),
    );
    assert!(!is_err(&ok), "controle: a epoch nova deve retomar: {ok}");
}

#[test]
fn r14_g6_last_seq_maior_que_head_e_negativo_sao_invalid_envelope() {
    let (_m, srv) = sobe_padrao(200, |_| {});
    espera_estado(&srv.socket, CLAUDE, "working", Duration::from_secs(5));
    let st = status(&srv.socket);
    let (head, epoch) = (st["head_seq"].as_u64().unwrap(), st["epoch"].clone());
    let r = rpc(
        &srv.socket,
        "events.resume",
        json!({"v": 1, "epoch": epoch.clone(), "last_seq": head + 5}),
    );
    assert_eq!(cod(&r), "invalid_envelope", "{r}");
    let n = rpc(
        &srv.socket,
        "events.resume",
        json!({"v": 1, "epoch": epoch, "last_seq": -1}),
    );
    assert_eq!(cod(&n), "invalid_envelope", "{n}");
    // Controle: last_seq == head (valido) retoma com a epoch atual.
    let epoch_atual = status(&srv.socket)["epoch"].clone();
    let ok = rpc(
        &srv.socket,
        "events.resume",
        json!({"v": 1, "epoch": epoch_atual, "last_seq": head}),
    );
    assert!(
        !is_err(&ok),
        "controle: last_seq == head deve retomar: {ok}"
    );
}

#[test]
fn r14_g6_segunda_chamada_de_resume_na_mesma_conexao_e_invalid_request() {
    let (_m, srv) = sobe_padrao(200, |_| {});
    espera_estado(&srv.socket, CLAUDE, "working", Duration::from_secs(5));
    let st = status(&srv.socket);
    let (head, epoch) = (st["head_seq"].as_u64().unwrap(), st["epoch"].clone());
    let mut c = Conexao::abre(&srv.socket);
    let a = c.chama(
        "events.resume",
        json!({"v": 1, "epoch": epoch.clone(), "last_seq": head}),
    );
    assert!(!is_err(&a), "controle: a primeira chamada retoma: {a}");
    let b = c.chama(
        "events.resume",
        json!({"v": 1, "epoch": epoch, "last_seq": head}),
    );
    assert_eq!(cod(&b), "invalid_request", "{b}");
}

#[test]
fn r14_g6_sem_eventos_head_zero_retoma_e_mais_um_falha() {
    // Painel ausente: a leitura e valida (agente sem painel) e nao muda estado, marcas, evidence nem source,
    // entao nenhum evento. head_seq 0 = nenhum evento (rev4 13.1).
    let mut m = Mundo::novo();
    m.paineis(&[]);
    let srv = inicia(&m, 200, &[("claude", "claude", "claude")], |_| {});
    assert!(
        espera(Duration::from_secs(4), || m.listagens() >= 3),
        "poucos ciclos"
    );
    let st = status(&srv.socket);
    assert_eq!(st["head_seq"], 0, "sem eventos o head e 0: {st}");
    let epoch = st["epoch"].clone();
    let ok = rpc(
        &srv.socket,
        "events.resume",
        json!({"v": 1, "epoch": epoch.clone(), "last_seq": 0}),
    );
    assert!(!is_err(&ok), "last_seq == head (0) deve retomar: {ok}");
    let mais = rpc(
        &srv.socket,
        "events.resume",
        json!({"v": 1, "epoch": epoch, "last_seq": 1}),
    );
    assert_eq!(cod(&mais), "invalid_envelope", "{mais}");
}

// ---------- G9: protocolo (rev4 secao 5 e 13) ----------

/// Linha JSON de tamanho exato `total` bytes (com ou sem LF no fim); o `id` (string) ajusta o tamanho.
fn linha_de_tamanho(total: usize, com_lf: bool) -> Vec<u8> {
    let lf = usize::from(com_lf);
    let base =
        json!({"jsonrpc": "2.0", "id": "", "method": "status", "params": {"v": 1}}).to_string();
    let n = total - lf - base.len();
    let id = "a".repeat(n);
    let s = json!({"jsonrpc": "2.0", "id": id, "method": "status", "params": {"v": 1}}).to_string();
    assert_eq!(s.len() + lf, total);
    let mut b = s.into_bytes();
    if com_lf {
        b.push(b'\n');
    }
    b
}

#[test]
fn r14_g9_campo_desconhecido_em_params_e_invalid_envelope_com_path() {
    let (_m, srv) = sobe_padrao(200, |_| {});
    let r = rpc(
        &srv.socket,
        "agent.get",
        json!({"v": 1, "id": CLAUDE, "extra": 1}),
    );
    assert_eq!(cod(&r), "invalid_envelope", "{r}");
    // Rev4 13.2: data.path e a trilha com pontos (params.campo).
    assert_eq!(caminho(&r), "params.extra", "{r}");
    let ok = rpc(&srv.socket, "agent.get", json!({"v": 1, "id": CLAUDE}));
    assert!(
        !is_err(&ok),
        "controle: o mesmo pedido sem o campo extra deve dar resultado: {ok}"
    );
    assert!(
        !status(&srv.socket).is_null(),
        "controle: o daemon segue vivo"
    );
}

#[test]
fn r14_g9_linha_de_exatamente_1_mib_com_lf_e_aceita() {
    let (_m, srv) = sobe_padrao(200, |_| {});
    let mut c = Conexao::abre(&srv.socket);
    c.bruto(&linha_de_tamanho(1024 * 1024, true));
    let v = c
        .le(Duration::from_secs(5))
        .expect("resposta a linha de 1 MiB");
    assert!(
        v["id"]
            .as_str()
            .map(|s| s.len() > 1024 * 1000)
            .unwrap_or(false),
        "id ecoado: {v}"
    );
    assert!(
        v.get("result").is_some(),
        "linha de 1 MiB com LF deve ser processada: {v}"
    );
}

#[test]
fn r14_g9_linha_de_1_mib_mais_um_byte_e_invalid_request_e_fecha() {
    // Rev4 13.2: linha acima de 1 MiB => invalid_request e a conexao e fechada.
    let (_m, srv) = sobe_padrao(200, |_| {});
    let mut c = Conexao::abre(&srv.socket);
    c.bruto(&linha_de_tamanho(1024 * 1024 + 1, true));
    let v = c
        .le(Duration::from_secs(5))
        .expect("resposta a linha acima de 1 MiB");
    assert_eq!(cod(&v), "invalid_request", "{v}");
    assert!(
        c.fechada_em(Duration::from_secs(3)),
        "a conexao devia ser fechada"
    );
    assert!(!status(&srv.socket).is_null(), "o daemon continua vivo");
}

#[test]
fn r14_g9_eof_sem_lf_processa_a_linha() {
    let (_m, srv) = sobe_padrao(200, |_| {});
    let mut c = Conexao::abre(&srv.socket);
    c.bruto(br#"{"jsonrpc":"2.0","id":7,"method":"status","params":{"v":1}}"#);
    c.fecha_escrita();
    let v = c
        .le(Duration::from_secs(5))
        .expect("resposta sem LF no fim");
    assert_eq!(v["id"], 7, "{v}");
    assert!(v.get("result").is_some(), "{v}");
}

#[test]
fn r14_g9_utf8_invalido_e_parse_error() {
    let (_m, srv) = sobe_padrao(200, |_| {});
    let mut c = Conexao::abre(&srv.socket);
    c.bruto(b"{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"status\",\"params\":{\"v\":1,\"x\":\"\xff\"}}\n");
    let v = c
        .le(Duration::from_secs(5))
        .expect("resposta a UTF-8 invalido");
    assert_eq!(cod(&v), "parse_error", "{v}");
    assert!(
        !status(&srv.socket).is_null(),
        "controle: o daemon segue vivo"
    );
}

#[test]
fn r14_g9_params_null_e_recusado() {
    let (_m, srv) = sobe_padrao(200, |_| {});
    let r = rpc(&srv.socket, "status", Value::Null);
    // Rev4 5: params deve ser objeto. O codigo exato entre os dois aceitos nao e fixado (AMBIGUIDADE).
    assert!(
        matches!(cod(&r).as_str(), "invalid_request" | "invalid_envelope"),
        "{r}"
    );
    assert!(
        !status(&srv.socket).is_null(),
        "controle: o daemon segue vivo"
    );
}

#[test]
fn r14_g9_pedido_sem_id_e_erro_e_daemon_continua() {
    let (_m, srv) = sobe_padrao(200, |_| {});
    let mut c = Conexao::abre(&srv.socket);
    c.envia_bruto(r#"{"jsonrpc":"2.0","method":"status","params":{"v":1}}"#);
    let v = c
        .le(Duration::from_secs(5))
        .expect("resposta ao pedido sem id");
    assert!(is_err(&v), "pedido sem id deve dar erro: {v}");
    assert!(!status(&srv.socket).is_null(), "daemon continua vivo");
}

#[test]
fn r14_g9_lote_e_invalid_request() {
    // Rev4 13.2: lote (array) => invalid_request.
    let (_m, srv) = sobe_padrao(200, |_| {});
    let mut c = Conexao::abre(&srv.socket);
    c.envia_bruto(r#"[{"jsonrpc":"2.0","id":1,"method":"status","params":{"v":1}}]"#);
    let v = c.le(Duration::from_secs(5)).expect("resposta ao lote");
    assert_eq!(cod(&v), "invalid_request", "{v}");
    assert!(
        v.get("result").is_none(),
        "lote nao pode ser processado: {v}"
    );
    assert!(!status(&srv.socket).is_null(), "daemon continua vivo");
}

#[test]
fn r14_g9_metodo_inexistente_e_method_not_found() {
    let (_m, srv) = sobe_padrao(200, |_| {});
    let r = rpc(&srv.socket, "agent.bind", json!({"v": 1}));
    assert_eq!(cod(&r), "method_not_found", "{r}");
    assert!(
        !status(&srv.socket).is_null(),
        "controle: status continua respondendo"
    );
}

#[test]
fn r14_g9_versao_inesperada_e_unsupported_version() {
    let (_m, srv) = sobe_padrao(200, |_| {});
    let r = rpc(&srv.socket, "status", json!({"v": 2}));
    assert_eq!(cod(&r), "unsupported_version", "{r}");
    assert!(
        !status(&srv.socket).is_null(),
        "controle: versao 1 segue valida"
    );
}

// ---------- G10: seguranca (rev4 9) ----------

fn modo_de(p: &Path) -> u32 {
    fs::metadata(p).expect("metadata").permissions().mode() & 0o777
}

#[test]
fn r14_g10_diretorio_0700_e_socket_0600() {
    let (_m, srv) = sobe_padrao(200, |_| {});
    assert_eq!(modo_de(&srv.state), 0o700, "diretorio de estado");
    assert_eq!(modo_de(&srv.socket), 0o600, "socket");
}

#[test]
#[ignore = "exige um segundo uid (outro usuario do sistema); o ambiente de teste nao permite trocar de uid"]
fn r14_g10_outro_uid_e_recusado_com_forbidden() {
    panic!("nao executado: precisa de outro uid");
}

// ---------- G12: CLI (rev4 8; rev5 8 e 15.1) ----------

#[test]
fn r14_g12_get_sem_json_uma_linha_no_formato_da_spec() {
    let (m, srv) = sobe_padrao(200, |_| {});
    espera_estado(&srv.socket, CLAUDE, "working", Duration::from_secs(5));
    let r = cli(m.raiz.path(), &srv.socket, &["get", CLAUDE]);
    assert_eq!(r.code, Some(0), "{r:?}");
    let linhas: Vec<&str> = r.stdout.lines().collect();
    assert_eq!(linhas.len(), 1, "uma linha por agente: {r:?}");
    let l = linhas[0];
    assert!(l.starts_with("claude "), "{l}");
    assert!(
        l.contains("working")
            && l.contains("observed")
            && l.contains("tmux")
            && l.contains("idade="),
        "{l}"
    );
}

#[test]
fn r14_g12_get_json_tem_envelope_schema_1_e_data_da_15_1() {
    let (m, srv) = sobe_padrao(200, |_| {});
    espera_estado(&srv.socket, CLAUDE, "working", Duration::from_secs(5));
    let r = cli(m.raiz.path(), &srv.socket, &["get", CLAUDE, "--json"]);
    assert_eq!(r.code, Some(0), "{r:?}");
    let j: Value = serde_json::from_str(r.stdout.trim()).expect("saida JSON");
    assert_eq!(j["schema_version"], 1, "{j}");
    assert_eq!(j["ok"], true, "{j}");
    assert!(j["code"].is_string() && j["message"].is_string(), "{j}");
    // Rev5.2 15.1: data = {agent, epoch, head_seq}.
    assert_eq!(j["data"]["agent"]["state"], "working", "{j}");
    assert_eq!(j["data"]["agent"]["id"], CLAUDE, "{j}");
    assert!(j["data"]["epoch"].is_string(), "{j}");
    assert!(j["data"]["head_seq"].is_u64(), "{j}");
}

#[test]
fn r14_g12_list_com_kind_filtra_e_tem_json() {
    let mut m = Mundo::novo();
    m.paineis(&[
        Painel::new("%1", "claude").tag("claude"),
        Painel::new("%2", "codex").tag("codex-blocks"),
    ]);
    m.tela_fixture("%1", "claude", "working-generation");
    m.tela_fixture("%2", "codex", "menu-model");
    let srv = inicia(&m, 200, &agentes_todos()[..2], |_| {});
    espera_estado(&srv.socket, CODEX, "blocked", Duration::from_secs(5));
    let sem = cli(m.raiz.path(), &srv.socket, &["list"]);
    assert_eq!(sem.code, Some(0), "{sem:?}");
    assert!(
        sem.stdout.lines().count() >= 2,
        "uma linha por agente: {sem:?}"
    );
    let filtro = cli(
        m.raiz.path(),
        &srv.socket,
        &["list", "--kind", "codex", "--json"],
    );
    assert_eq!(filtro.code, Some(0), "{filtro:?}");
    let j: Value = serde_json::from_str(filtro.stdout.trim()).expect("JSON");
    let ids: Vec<&str> = j["data"]["agents"]
        .as_array()
        .unwrap()
        .iter()
        .map(|a| a["id"].as_str().unwrap())
        .collect();
    assert!(
        ids.contains(&CODEX) && !ids.contains(&CLAUDE),
        "filtro por kind: {j}"
    );
}

#[test]
fn r14_g12_health_0_com_agente_unknown_e_fonte_ok() {
    // Painel ausente: o agente e unknown, mas a execucao da fonte deu certo. health sai 0 (rev4 8).
    let mut m = Mundo::novo();
    m.paineis(&[]);
    let srv = inicia(&m, 200, &[("claude", "claude", "claude")], |_| {});
    assert!(
        espera_fonte(&srv.socket, true, Duration::from_secs(5)),
        "a fonte devia estar ok"
    );
    let r = cli(m.raiz.path(), &srv.socket, &["health"]);
    assert_eq!(
        r.code,
        Some(0),
        "unknown por agente nao derruba health: {r:?}"
    );
}

#[test]
fn r14_g12_health_falha_quando_a_execucao_da_fonte_falha() {
    // Rev4 13.3: com a fonte falha, health sai 1.
    let mut m = Mundo::novo();
    m.paineis(&[Painel::new("%1", "claude").tag("claude")]);
    m.listar("falha");
    let srv = inicia(&m, 200, &[("claude", "claude", "claude")], |_| {});
    assert!(
        espera_fonte(&srv.socket, false, Duration::from_secs(5)),
        "a fonte devia falhar"
    );
    let r = cli(m.raiz.path(), &srv.socket, &["health"]);
    assert_eq!(r.code, Some(1), "{r:?}");
    m.listar("ok");
    assert!(
        espera_fonte(&srv.socket, true, Duration::from_secs(5)),
        "controle: a fonte devia voltar a ok"
    );
    let ok = cli(m.raiz.path(), &srv.socket, &["health"]);
    assert_eq!(
        ok.code,
        Some(0),
        "controle: com a fonte ok, health sai 0: {ok:?}"
    );
}

#[test]
fn r14_g12_status_cli_com_e_sem_json() {
    let (m, srv) = sobe_padrao(200, |_| {});
    espera_estado(&srv.socket, CLAUDE, "working", Duration::from_secs(5));
    let a = cli(m.raiz.path(), &srv.socket, &["status"]);
    assert_eq!(a.code, Some(0), "{a:?}");
    let b = cli(m.raiz.path(), &srv.socket, &["status", "--json"]);
    assert_eq!(b.code, Some(0), "{b:?}");
    let j: Value = serde_json::from_str(b.stdout.trim()).expect("JSON");
    assert_eq!(j["schema_version"], 1, "{j}");
    assert_eq!(j["ok"], true, "{j}");
}

#[test]
fn r14_g12_codigos_de_saida_2_uso_incorreto() {
    let (m, srv) = sobe_padrao(200, |_| {});
    espera_estado(&srv.socket, CLAUDE, "working", Duration::from_secs(5));
    // Controle: a forma correta sai 0.
    assert_eq!(
        cli(m.raiz.path(), &srv.socket, &["get", CLAUDE]).code,
        Some(0)
    );
    // Sem socket nenhum (nem flag, nem env, nem --state-dir): 2.
    assert_eq!(cli_d(m.raiz.path(), &["get", CLAUDE]).code, Some(2));
    // --since sem --epoch e vice-versa: 2.
    assert_eq!(
        cli(m.raiz.path(), &srv.socket, &["watch", "--since", "1"]).code,
        Some(2)
    );
    assert_eq!(
        cli(m.raiz.path(), &srv.socket, &["watch", "--epoch", "1"]).code,
        Some(2)
    );
}

#[test]
fn r14_g12_codigo_3_agente_inexistente_e_unknown_agent() {
    let (m, srv) = sobe_padrao(200, |_| {});
    espera_estado(&srv.socket, CLAUDE, "working", Duration::from_secs(5));
    let r = cli(m.raiz.path(), &srv.socket, &["get", "nao-existe"]);
    assert_eq!(r.code, Some(3), "{r:?}");
    assert_eq!(
        cli(m.raiz.path(), &srv.socket, &["get", CLAUDE]).code,
        Some(0),
        "controle: id existente sai 0"
    );
    let j = cli(m.raiz.path(), &srv.socket, &["get", "nao-existe", "--json"]);
    assert_eq!(j.code, Some(3), "{j:?}");
    let v: Value = serde_json::from_str(j.stdout.trim()).expect("JSON");
    assert_eq!(v["ok"], false, "{v}");
    assert_eq!(v["code"], "unknown_agent", "{v}");
}

#[test]
fn r14_g12_codigo_4_daemon_indisponivel() {
    let m = Mundo::novo();
    let morto = m.raiz.path().join("nao-existe.sock");
    let r = cli(m.raiz.path(), &morto, &["get", CLAUDE]);
    assert_eq!(r.code, Some(4), "{r:?}");
    let (m2, srv2) = sobe_padrao(200, |_| {});
    assert_eq!(
        cli(m2.raiz.path(), &srv2.socket, &["get", CLAUDE]).code,
        Some(0),
        "controle: com o daemon no ar sai 0"
    );
}

#[test]
fn r14_g12_socket_por_state_dir_e_por_env() {
    let (m, srv) = sobe_padrao(200, |_| {});
    espera_estado(&srv.socket, CLAUDE, "working", Duration::from_secs(5));
    let a = cli_d(
        m.raiz.path(),
        &["get", CLAUDE, "--state-dir", s(&estado_dir(&m))],
    );
    assert_eq!(a.code, Some(0), "--state-dir: {a:?}");
    let sock = s(&srv.socket).to_string();
    let b = roda_com_env(
        m.raiz.path(),
        &["get", CLAUDE],
        &[("AGENTD_SOCKET", sock.as_str())],
    );
    assert_eq!(b.code, Some(0), "AGENTD_SOCKET: {b:?}");
}

#[test]
fn r14_g12_watch_imprime_uma_linha_por_state_changed() {
    // AMBIGUIDADE (rodada 5): o formato da linha do `watch` sem --json nao e fixado; o teste so afirma que a
    // linha traz o id e o novo estado.
    let (mut m, srv) = sobe_padrao(200, |_| {});
    espera_estado(&srv.socket, CLAUDE, "working", Duration::from_secs(5));
    let out = m.raiz.path().join("watch.out");
    let mut child = Command::new(BIN)
        .args(["watch", "--socket", s(&srv.socket)])
        .env("HOME", m.raiz.path())
        .stdin(Stdio::null())
        .stdout(fs::File::create(&out).unwrap())
        .stderr(Stdio::null())
        .spawn()
        .expect("executar watch");
    std::thread::sleep(Duration::from_millis(500));
    m.tela_fixture("%1", "claude", "permission-bash");
    espera_estado(&srv.socket, CLAUDE, "blocked", Duration::from_secs(5));
    std::thread::sleep(Duration::from_millis(800));
    let _ = child.kill();
    let _ = child.wait();
    let txt = fs::read_to_string(&out).unwrap_or_default();
    assert!(
        txt.lines()
            .any(|l| l.contains("claude") && l.contains("blocked")),
        "watch nao imprimiu a troca: {txt}"
    );
}

#[test]
fn r14_g12_watch_json_cada_linha_traz_o_evento_em_data() {
    // Rev5.2 15.1: cada linha de `watch --json` traz o Evento em `data`.
    let (mut m, srv) = sobe_padrao(200, |_| {});
    espera_estado(&srv.socket, CLAUDE, "working", Duration::from_secs(5));
    let out = m.raiz.path().join("watch-json.out");
    let mut child = Command::new(BIN)
        .args(["watch", "--json", "--socket", s(&srv.socket)])
        .env("HOME", m.raiz.path())
        .stdin(Stdio::null())
        .stdout(fs::File::create(&out).unwrap())
        .stderr(Stdio::null())
        .spawn()
        .expect("executar watch --json");
    std::thread::sleep(Duration::from_millis(500));
    m.tela_fixture("%1", "claude", "permission-bash");
    espera_estado(&srv.socket, CLAUDE, "blocked", Duration::from_secs(5));
    std::thread::sleep(Duration::from_millis(800));
    let _ = child.kill();
    let _ = child.wait();
    let txt = fs::read_to_string(&out).unwrap_or_default();
    let eventos: Vec<Value> = txt
        .lines()
        .filter_map(|l| serde_json::from_str::<Value>(l).ok())
        .map(|v| v["data"].clone())
        .collect();
    assert!(
        eventos
            .iter()
            .any(|e| e["id"] == CLAUDE && e["state"] == "blocked" && e["seq"].is_u64()),
        "watch --json devia trazer o Evento em data: {txt}"
    );
}

// ---------- G14: reinicio ----------

#[test]
fn r14_g14_reinicio_sobe_de_novo_com_epoch_nova_e_nada_persiste() {
    let (mut m, mut srv) = sobe_padrao(200, |_| {});
    espera_estado(&srv.socket, CLAUDE, "working", Duration::from_secs(5));
    let antiga = res(&rpc(&srv.socket, "agent.list", json!({"v": 1})))["epoch"].clone();
    assert!(srv.para().is_some(), "servidor nao saiu com SIGTERM");
    let _ = fs::remove_file(&srv.socket);
    // Mesmo cenario (painel em working): se algo persistisse, o agente nasceria working com last_known.
    m.paineis(&[]);
    let cfg = m.raiz.path().join("config.json");
    let srv2 = sobe(&estado_dir(&m), &cfg);
    assert!(
        srv2.stderr().contains("agentd: pronto")
            || espera(Duration::from_secs(3), || srv2
                .stderr()
                .contains("agentd: pronto")),
        "rev4 13.6: a linha `agentd: pronto` devia sair em stderr"
    );
    let nova = res(&rpc(&srv2.socket, "agent.list", json!({"v": 1})))["epoch"].clone();
    assert_ne!(nova, antiga, "epoch deve mudar a cada partida");
    let a = agente(&srv2.socket, CLAUDE);
    assert!(
        a["last_known"].is_null(),
        "nada persiste: last_known deve ser null apos reinicio: {a}"
    );
    assert_ne!(
        estado_str(&a),
        "working",
        "nada persiste: o estado antigo nao volta: {a}"
    );
}

// ---------- G20: atomicidade ----------

#[test]
fn r14_g20_snapshot_e_head_do_mesmo_instante_e_replay_sem_lacunas() {
    // 40 trocas (working/blocked) a cada 200 ms. Cada snapshot (agent.get) traz estado e head_seq do mesmo
    // instante: o estado tem de ser o que os eventos ate head_seq determinam.
    let (mut m, srv) = sobe_padrao(200, |_| {});
    espera_estado(&srv.socket, CLAUDE, "working", Duration::from_secs(5));
    let mut ev = assina(&srv.socket);
    let mut snaps: Vec<(String, u64)> = Vec::new();
    let fim = Instant::now() + Duration::from_millis(8500);
    let mut i = 0usize;
    let mut proximo = Instant::now();
    while Instant::now() < fim {
        if Instant::now() >= proximo {
            let nome = if i.is_multiple_of(2) {
                "permission-bash"
            } else {
                "working-generation"
            };
            m.tela_fixture("%1", "claude", nome);
            i += 1;
            proximo += Duration::from_millis(200);
        }
        let r = res(&rpc(
            &srv.socket,
            "agent.get",
            json!({"v": 1, "id": CLAUDE}),
        ));
        snaps.push((
            r["agent"]["state"].as_str().unwrap().to_string(),
            r["head_seq"].as_u64().unwrap(),
        ));
        std::thread::sleep(Duration::from_millis(30));
    }
    let evs = coleta(&mut ev, Duration::from_secs(2));
    let seqs: Vec<u64> = evs.iter().map(|e| e["seq"].as_u64().unwrap()).collect();
    assert!(
        seqs.len() >= 20,
        "controle: houve trocas suficientes: {}",
        seqs.len()
    );
    for w in seqs.windows(2) {
        assert_eq!(w[1], w[0] + 1, "lacuna ou duplicata no replay+ao vivo");
    }
    for (st, head) in &snaps {
        let esperado = evs
            .iter()
            .rfind(|e| e["id"] == CLAUDE && e["seq"].as_u64().unwrap() <= *head)
            .map(|e| e["state"].as_str().unwrap().to_string())
            .unwrap_or_else(|| "unknown".to_string());
        assert_eq!(
            st, &esperado,
            "snapshot com head {head} nao bate com os eventos ate ali"
        );
    }
}

// ---------- G22: assinante lento ----------

#[test]
fn r14_g22_assinante_lento_recebe_closed_e_os_polls_seguem() {
    // 32 agentes trocando todos a cada ciclo: ~32 eventos por ciclo sem leitura do cliente: a fila de 256 estoura.
    let (mut m, srv) = sobe_32(4096);
    espera_estado(&srv.socket, "a31", "working", Duration::from_secs(5));
    let mut lento = assina(&srv.socket); // nunca le
    let t = Instant::now();
    let _ = agente(&srv.socket, "a00");
    assert!(
        t.elapsed() < Duration::from_secs(1),
        "controle: agent.get lento antes da troca"
    );
    // Ate o head andar 300 alem do inicio: sem leitura do cliente, a fila de 256 tem de estourar.
    let head0 = status(&srv.socket)["head_seq"].as_u64().unwrap();
    let fim = Instant::now() + Duration::from_secs(25);
    let mut i = 0usize;
    while status(&srv.socket)["head_seq"].as_u64().unwrap() < head0 + 300 && Instant::now() < fim {
        flip_all(
            &mut m,
            32,
            if i.is_multiple_of(2) {
                "permission-bash"
            } else {
                "working-generation"
            },
        );
        i += 1;
        let t = Instant::now();
        let _ = agente(&srv.socket, "a00");
        assert!(
            t.elapsed() < Duration::from_secs(1),
            "poll atrasado por assinante lento (troca {i})"
        );
        std::thread::sleep(Duration::from_millis(150));
    }
    assert!(
        status(&srv.socket)["head_seq"].as_u64().unwrap() >= head0 + 300,
        "controle: as trocas nao geraram eventos suficientes"
    );
    let mut fechado = false;
    for _ in 0..3000 {
        match lento.le(Duration::from_secs(2)) {
            Some(v) if v.get("method") == Some(&json!("closed")) => {
                assert_eq!(v["params"]["reason"], "slow_consumer", "{v}");
                assert!(v["params"]["last_seq"].is_u64(), "{v}");
                fechado = true;
                break;
            }
            Some(_) => continue,
            None => break,
        }
    }
    assert!(fechado, "assinante lento nao recebeu closed(slow_consumer)");
}

// ---------- Contrato do Agente, epoch, unknown_agent, agent.list e status (rev4 5 e 3) ----------

#[test]
fn r14_agente_tem_os_campos_da_secao_3_com_tipos_certos() {
    let (_m, srv) = sobe_padrao(200, |_| {});
    espera_estado(&srv.socket, CLAUDE, "working", Duration::from_secs(5));
    let a = agente(&srv.socket, CLAUDE);
    for campo in [
        "v",
        "id",
        "kind",
        "state",
        "marks",
        "evidence",
        "source",
        "last_seen",
        "observed_at",
        "freshness_ms",
        "last_known",
        "cwd",
    ] {
        assert!(a.get(campo).is_some(), "campo {campo} ausente: {a}");
    }
    assert_eq!(a["v"], 1, "{a}");
    assert_eq!(a["kind"], "claude", "{a}");
    assert_eq!(a["evidence"], "observed", "{a}");
    assert_eq!(a["source"], "tmux", "rev5 4: source e tmux: {a}");
    assert!(a["last_known"].is_null(), "{a}");
    let agora_ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis() as u64;
    for campo in ["last_seen", "observed_at"] {
        let t = a[campo]
            .as_u64()
            .unwrap_or_else(|| panic!("{campo} deve ser inteiro ms: {a}"));
        assert!(
            t > 1_600_000_000_000 && t <= agora_ms + 60_000,
            "{campo} fora de ms Unix: {t}"
        );
    }
    let f = a["freshness_ms"]
        .as_u64()
        .unwrap_or_else(|| panic!("freshness_ms inteiro: {a}"));
    assert!(f < 60_000, "idade pequena: {f}");
}

#[test]
fn r14_agente_freshness_e_null_sem_leitura_valida() {
    // Listagem falha desde a partida: nenhuma leitura valida ainda (rev4 13.3).
    let mut m = Mundo::novo();
    m.paineis(&[Painel::new("%1", "claude").tag("claude")]);
    m.listar("falha");
    let srv = inicia(&m, 200, &[("claude", "claude", "claude")], |_| {});
    assert!(
        espera(Duration::from_secs(4), || m.listagens() >= 2),
        "poucos ciclos"
    );
    let a = agente(&srv.socket, CLAUDE);
    assert!(
        a["freshness_ms"].is_null(),
        "freshness antes da primeira leitura valida: {a}"
    );
    assert!(
        a["last_seen"].is_null(),
        "last_seen sem painel presente: {a}"
    );
    assert!(
        a["observed_at"].is_null(),
        "observed_at antes da primeira leitura valida: {a}"
    );
}

#[test]
fn r14_agente_epoch_e_string_decimal_de_64_bits() {
    let (_m, srv) = sobe_padrao(200, |_| {});
    let e = res(&rpc(&srv.socket, "agent.list", json!({"v": 1})))["epoch"].clone();
    let txt = e
        .as_str()
        .unwrap_or_else(|| panic!("epoch deve ser string: {e}"));
    assert!(
        !txt.is_empty() && txt.len() <= 20 && txt.bytes().all(|b| b.is_ascii_digit()),
        "{txt}"
    );
    assert!(txt.parse::<u64>().is_ok(), "{txt} nao cabe em u64");
}

#[test]
fn r14_agente_erro_unknown_agent() {
    let (_m, srv) = sobe_padrao(200, |_| {});
    let r = rpc(
        &srv.socket,
        "agent.get",
        json!({"v": 1, "id": "nao-existe"}),
    );
    assert_eq!(cod(&r), "unknown_agent", "{r}");
    let ok = rpc(&srv.socket, "agent.get", json!({"v": 1, "id": CLAUDE}));
    assert!(
        !is_err(&ok),
        "controle: o id existente deve responder: {ok}"
    );
}

#[test]
fn r14_agent_list_ordenada_por_id_e_filtrada_por_kind() {
    let mut m = Mundo::novo();
    m.paineis(&[
        Painel::new("%1", "claude").tag("claude"),
        Painel::new("%2", "codex").tag("codex-blocks"),
    ]);
    m.tela_fixture("%1", "claude", "working-generation");
    m.tela_fixture("%2", "codex", "menu-model");
    let srv = inicia(&m, 200, &agentes_todos()[..2], |_| {});
    espera_estado(&srv.socket, CODEX, "blocked", Duration::from_secs(5));
    let ids: Vec<String> = lista(&srv.socket)
        .iter()
        .map(|a| a["id"].as_str().unwrap().to_string())
        .collect();
    let mut ord = ids.clone();
    ord.sort();
    assert_eq!(ids, ord, "agent.list deve vir ordenada por id");
    let f = res(&rpc(
        &srv.socket,
        "agent.list",
        json!({"v": 1, "kind": "codex"}),
    ));
    let kinds: Vec<&str> = f["agents"]
        .as_array()
        .unwrap()
        .iter()
        .map(|a| a["kind"].as_str().unwrap())
        .collect();
    assert!(
        !kinds.is_empty() && kinds.iter().all(|k| *k == "codex"),
        "{f}"
    );
}

#[test]
fn r14_status_tem_forma_da_secao_5_e_last_error_so_codigo() {
    let mut m = Mundo::novo();
    m.paineis(&[Painel::new("%1", "claude").tag("claude")]);
    m.listar("falha");
    let srv = inicia(&m, 200, &[("claude", "claude", "claude")], |_| {});
    assert!(
        espera_fonte(&srv.socket, false, Duration::from_secs(5)),
        "a fonte devia falhar"
    );
    let st = status(&srv.socket);
    assert!(
        st["epoch"].is_string() && st["head_seq"].is_u64() && st["uptime_s"].is_number(),
        "{st}"
    );
    assert!(st["agents"][CLAUDE]["state"].is_string(), "{st}");
    assert_eq!(st["sources"]["tmux"]["ok"], false, "{st}");
    let e = st["sources"]["tmux"]["last_error"]
        .as_str()
        .unwrap_or_else(|| panic!("last_error codigo: {st}"));
    // Rev5 4: codigos de falha da fonte tmux; saida nao zero do list-panes = source_exit_nonzero.
    assert_eq!(e, "source_exit_nonzero", "{st}");
}

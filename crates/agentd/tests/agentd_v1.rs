//! Testes independentes do agentd v1 (G1 a G23 da secao 10 de docs/spec.md, mais formato do Agente,
//! erros, `status` e CLI). Escritos SO a partir do contrato (rev3); a implementacao nao foi lida (nao existe:
//! o binario e um esqueleto que sai 2). Caixa preta: binario, socket e herdr falso (common/mod.rs).
//! Todo diretorio e temporario; HOME e XDG_CONFIG_HOME apontam para dentro dele. Nenhum herdr real,
//! nenhuma rede, nenhum processo de agente. Devem falhar ate a implementacao existir.
//! Convencao: `// AMBIGUO:` marca onde o contrato nao fixa o texto, o formato ou o codigo lido.
#![allow(dead_code)]
mod common;

use common::*;
use serde_json::{Value, json};
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::time::{Duration, Instant};

const CLAUDE: &str = "claude";
const CODEX: &str = "codex-blocks";
const PANE_C: &str = "w1:p1";
const PANE_X: &str = "w3:p1";

fn marcas(a: &Value) -> Vec<String> {
    a["marks"]
        .as_array()
        .expect("marks e array")
        .iter()
        .map(|m| m.as_str().unwrap().to_string())
        .collect()
}

fn tem(a: &Value, marca: &str) -> bool {
    marcas(a).iter().any(|m| m == marca)
}

fn estado(a: &Value) -> String {
    a["state"].as_str().expect("state string").to_string()
}

fn espera_estado(socket: &Path, id: &str, alvo: &str, d: Duration) -> Value {
    espera_agente(socket, id, d, |a| estado(a) == alvo)
}

/// Assina eventos desde o inicio: epoch via agent.list, depois events.resume com last_seq 0.
// AMBIGUO: a spec nao diz se o primeiro seq e 0 ou 1; last_seq 0 cobre os dois, desde que o anel comece em seq <= 1.
fn assina(socket: &Path) -> Conexao {
    let l = rpc(socket, "agent.list", json!({"v": 1}));
    let epoch = res(&l)["epoch"].clone();
    let mut c = Conexao::abre(socket);
    let r = c.chama(
        "events.resume",
        json!({"v": 1, "epoch": epoch, "last_seq": 0}),
    );
    assert!(!is_err(&r), "events.resume desde o inicio falhou: {r}");
    c
}

/// Coleta eventos por `d` (ou ate `pare` ser verdadeiro para um deles).
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

/// Sobe um servidor com a fila de leituras dada (fallback = `depois`); poll 200 ms.
fn sobe_fila(
    poll: u64,
    fila: &[Leitura],
    depois: &[Value],
    ajuste: impl FnOnce(&mut Value),
) -> (tempfile::TempDir, Servidor, Herdr) {
    let d = tmp();
    let h = Herdr::novo(d.path());
    h.fila(fila, depois);
    let cfg = config_com(d.path(), &h, poll, ajuste);
    let srv = sobe(&d.path().join("state"), &cfg);
    (d, srv, h)
}

// ---------- G1: mapeamento ----------

fn estado_de_status(st: &str) -> Value {
    let (_d, srv, h) = amb(200);
    h.resposta(&[info(PANE_C, CLAUDE, st, "t-1", 1)]);
    h.espera_chamadas(2, Duration::from_secs(5));
    espera_agente(&srv.socket, CLAUDE, Duration::from_secs(6), |a| {
        estado(a) != "unknown" || tem(a, "error")
    })
}

#[test]
fn g1_working_mapeia_para_working() {
    let a = estado_de_status("working");
    assert_eq!(estado(&a), "working", "{a}");
    assert!(marcas(&a).is_empty(), "{a}");
}

#[test]
fn g1_blocked_mapeia_para_blocked() {
    let a = estado_de_status("blocked");
    assert_eq!(estado(&a), "blocked", "{a}");
}

#[test]
fn g1_idle_mapeia_para_idle_apos_confirmacao() {
    // idle_confirm_polls 2, idle_confirm_ms 0: duas leituras idle iguais confirmam.
    let a = espera_estado_idle_padrao();
    assert_eq!(estado(&a), "idle", "{a}");
    assert_eq!(a["evidence"], "observed", "{a}");
}

fn espera_estado_idle_padrao() -> Value {
    let (_d, srv, h) = amb(200);
    h.resposta(&[info(PANE_C, CLAUDE, "idle", "t-1", 1)]);
    espera_estado(&srv.socket, CLAUDE, "idle", Duration::from_secs(6))
}

#[test]
fn g1_done_mapeia_para_idle() {
    // `done` e `idle` para o herdr; publicado sempre `idle` (regra 5 vale para os dois).
    let (_d, srv, h) = amb(200);
    h.resposta(&[info(PANE_C, CLAUDE, "done", "t-1", 1)]);
    let a = espera_estado(&srv.socket, CLAUDE, "idle", Duration::from_secs(6));
    assert_eq!(estado(&a), "idle", "{a}");
    assert!(!tem(&a, "error"), "{a}");
}

#[test]
fn g1_unknown_do_herdr_vira_unknown_sem_error() {
    let a = estado_de_status("unknown");
    assert_eq!(estado(&a), "unknown", "{a}");
    assert!(!tem(&a, "error"), "unknown mapeado nao deve ter error: {a}");
}

#[test]
fn g1_valor_desconhecido_vira_unknown_com_error_nunca_idle() {
    let (_d, srv, h) = amb(200);
    h.resposta(&[info(PANE_C, CLAUDE, "xyz", "t-1", 1)]);
    h.espera_chamadas(4, Duration::from_secs(5));
    let a = agente(&srv.socket, CLAUDE);
    assert_eq!(estado(&a), "unknown", "{a}");
    assert!(tem(&a, "error"), "{a}");
    let st = status(&srv.socket);
    assert_eq!(
        st["sources"]["herdr"]["last_error"], "status_unmapped",
        "{st}"
    );
    assert_eq!(
        st["sources"]["herdr"]["ok"], true,
        "execucao deu certo: {st}"
    );
}

// ---------- G2: unknown por padrao e leitura falha ----------

#[test]
fn g2_sem_painel_vira_unknown_com_error_e_nao_gone_no_inicio() {
    let (_d, srv, h) = amb(200);
    h.resposta(&[]);
    h.espera_chamadas(3, Duration::from_secs(5));
    let a = agente(&srv.socket, CLAUDE);
    assert_eq!(estado(&a), "unknown", "{a}");
    assert!(tem(&a, "error"), "{a}");
    assert_ne!(estado(&a), "gone", "ausencia ainda dentro do prazo: {a}");
}

#[test]
fn g2_saida_nao_zero_vira_unknown_com_error() {
    let (_d, srv, h) = amb(200);
    h.resposta(&[info(PANE_C, CLAUDE, "working", "t-1", 1)]);
    h.flag("saida.flag", true);
    h.espera_chamadas(2, Duration::from_secs(5));
    let a = agente(&srv.socket, CLAUDE);
    assert_eq!(estado(&a), "unknown", "{a}");
    assert!(tem(&a, "error"), "{a}");
    let st = status(&srv.socket);
    assert_eq!(st["sources"]["herdr"]["ok"], false, "{st}");
    assert_eq!(
        st["sources"]["herdr"]["last_error"], "source_exit_nonzero",
        "{st}"
    );
}

#[test]
fn g2_json_invalido_vira_unknown_com_error() {
    let (_d, srv, h) = amb(200);
    h.flag("lixo.flag", true);
    h.espera_chamadas(2, Duration::from_secs(5));
    let a = agente(&srv.socket, CLAUDE);
    assert_eq!(estado(&a), "unknown", "{a}");
    assert!(tem(&a, "error"), "{a}");
    assert_eq!(
        status(&srv.socket)["sources"]["herdr"]["last_error"],
        "source_bad_json"
    );
}

#[test]
fn g2_stdout_acima_de_1_mib_vira_leitura_falha() {
    let (_d, srv, h) = amb(200);
    h.flag("grande.flag", true);
    h.espera_chamadas(2, Duration::from_secs(5));
    let a = agente(&srv.socket, CLAUDE);
    assert_eq!(estado(&a), "unknown", "{a}");
    assert!(tem(&a, "error"), "{a}");
    assert_eq!(
        status(&srv.socket)["sources"]["herdr"]["last_error"],
        "source_too_large"
    );
}

#[test]
fn g2_pane_duplicado_falha_a_leitura_toda() {
    let (_d, srv, h) = amb(200);
    h.resposta(&[
        info(PANE_C, CLAUDE, "working", "t-1", 1),
        info(PANE_C, CLAUDE, "working", "t-2", 1),
        info(PANE_X, CODEX, "working", "t-3", 1),
    ]);
    h.espera_chamadas(2, Duration::from_secs(5));
    for id in [CLAUDE, CODEX] {
        let a = agente(&srv.socket, id);
        assert_eq!(estado(&a), "unknown", "{id}: {a}");
        assert!(tem(&a, "error"), "{id}: {a}");
    }
    assert_eq!(
        status(&srv.socket)["sources"]["herdr"]["last_error"],
        "duplicate_pane"
    );
}

#[test]
fn g2_agent_nulo_falha_so_aquele_agente() {
    let (_d, srv, h) = amb(200);
    let mut nulo = info(PANE_C, CLAUDE, "working", "t-1", 1);
    nulo["agent"] = Value::Null;
    h.resposta(&[nulo, info(PANE_X, CODEX, "working", "t-2", 1)]);
    espera_estado(&srv.socket, CODEX, "working", Duration::from_secs(5));
    let c = agente(&srv.socket, CLAUDE);
    assert_eq!(estado(&c), "unknown", "{c}");
    assert!(tem(&c, "error"), "{c}");
    let x = agente(&srv.socket, CODEX);
    assert_eq!(
        estado(&x),
        "working",
        "outro agente nao deve ser afetado: {x}"
    );
}

#[test]
fn g2_kind_divergente_falha_so_aquele_agente() {
    let (_d, srv, h) = amb(200);
    // painel de `claude` ocupado por um codex: kind_mismatch so para `claude`.
    h.resposta(&[
        info(PANE_C, "codex", "working", "t-1", 1),
        info(PANE_X, CODEX, "working", "t-2", 1),
    ]);
    espera_estado(&srv.socket, CODEX, "working", Duration::from_secs(5));
    let c = agente(&srv.socket, CLAUDE);
    assert_eq!(estado(&c), "unknown", "{c}");
    assert!(tem(&c, "error"), "{c}");
    assert_eq!(
        status(&srv.socket)["sources"]["herdr"]["last_error"],
        "kind_mismatch"
    );
}

// ---------- G3: stale pela consulta e leitura travada ----------

/// stale_after_ms da config de teste (common::config com poll 200 => 200*3+400+200).
const STALE_MS: u64 = 1200;

#[test]
fn g3_leitura_travada_vira_erro_depois_stale_com_last_known() {
    let (_d, srv, h) = amb(200);
    h.resposta(&[info(PANE_C, CLAUDE, "working", "t-1", 1)]);
    espera_estado(&srv.socket, CLAUDE, "working", Duration::from_secs(5));
    h.flag("trava.flag", true);
    // Antes de stale_after_ms: so erro (timeout da fonte), sem stale.
    let a = espera_agente(&srv.socket, CLAUDE, Duration::from_secs(3), |a| {
        tem(a, "error")
    });
    assert_eq!(estado(&a), "unknown", "{a}");
    assert_eq!(a["last_known"], "working", "{a}");
    assert_eq!(
        status(&srv.socket)["sources"]["herdr"]["last_error"],
        "source_timeout"
    );
    // Depois de stale_after_ms: stale tambem, e o ultimo estado conhecido fica guardado.
    let b = espera_agente(&srv.socket, CLAUDE, Duration::from_secs(5), |a| {
        tem(a, "stale")
    });
    assert!(tem(&b, "stale"), "stale nao apareceu: {b}");
    assert!(tem(&b, "error"), "{b}");
    assert_eq!(estado(&b), "unknown", "{b}");
    assert_eq!(b["last_known"], "working", "{b}");
    // Leitura valida seguinte limpa error e stale.
    h.flag("trava.flag", false);
    let c = espera_estado(&srv.socket, CLAUDE, "working", Duration::from_secs(5));
    assert!(!tem(&c, "error") && !tem(&c, "stale"), "{c}");
    assert!(
        c["last_known"].is_null(),
        "last_known limpo por leitura valida: {c}"
    );
}

#[test]
fn g18_stale_so_acima_de_stale_after_ms() {
    let (_d, srv, h) = amb(200);
    h.resposta(&[info(PANE_C, CLAUDE, "working", "t-1", 1)]);
    espera_estado(&srv.socket, CLAUDE, "working", Duration::from_secs(5));
    let t_valid = Instant::now();
    h.flag("trava.flag", true);
    // Tolerancia de um ciclo para a origem monotonica (observed_at e o inicio do herdr).
    std::thread::sleep(Duration::from_millis(STALE_MS - 200));
    assert!(
        !tem(&agente(&srv.socket, CLAUDE), "stale"),
        "stale antes do limite"
    );
    let a = espera_agente(&srv.socket, CLAUDE, Duration::from_secs(4), |a| {
        tem(a, "stale")
    });
    let el = t_valid.elapsed();
    assert!(tem(&a, "stale"), "stale nunca apareceu: {a}");
    assert!(
        el <= Duration::from_millis(STALE_MS + 800),
        "stale tarde demais: {el:?}"
    );
}

// ---------- G4: gone ----------

/// gone_after_s da config de teste (common::config).
const GONE_S: u64 = 5;

#[test]
fn g4_ausencia_valida_vira_gone_apos_gone_after_s_e_volta_com_evento() {
    let t0 = Instant::now();
    let (_d, srv, h) = amb(200);
    h.resposta(&[]);
    // Nunca visto e ausente: no inicio, unknown com error (nunca gone imediato).
    h.espera_chamadas(3, Duration::from_secs(5));
    let a = agente(&srv.socket, CLAUDE);
    assert_eq!(estado(&a), "unknown", "{a}");
    assert!(tem(&a, "error"), "{a}");
    let mut ev = assina(&srv.socket);
    let g = espera_estado(&srv.socket, CLAUDE, "gone", Duration::from_secs(GONE_S + 4));
    let el = t0.elapsed();
    assert_eq!(estado(&g), "gone", "{g}");
    assert!(
        el >= Duration::from_millis(GONE_S * 1000 - 1000),
        "gone antes do prazo gone_after_s: {el:?}"
    );
    // Reversivel: painel volta a aparecer com leitura valida.
    h.resposta(&[info(PANE_C, CLAUDE, "working", "t-2", 1)]);
    let w = espera_estado(&srv.socket, CLAUDE, "working", Duration::from_secs(5));
    assert_eq!(estado(&w), "working", "{w}");
    let evs = coleta(&mut ev, Duration::from_secs(2));
    assert!(
        evs.iter()
            .any(|e| e["id"] == CLAUDE && e["previous_state"] == "gone" && e["state"] == "working"),
        "esperava evento gone -> working: {evs:?}"
    );
}

#[test]
fn g4_lista_vazia_apos_erros_nao_gera_gone_imediato() {
    let (_d, srv, h) = sobe_fila(
        200,
        &[Leitura::Erro, Leitura::Erro, Leitura::Ok(vec![])],
        &[],
        |_| {},
    );
    h.espera_chamadas(4, Duration::from_secs(5));
    let a = agente(&srv.socket, CLAUDE);
    assert_eq!(estado(&a), "unknown", "{a}");
    assert!(tem(&a, "error"), "{a}");
    assert_ne!(
        estado(&a),
        "gone",
        "lista vazia apos erros nao e ausencia valida com prazo vencido: {a}"
    );
    drop(srv);
}

#[test]
fn g18_gone_nao_antes_de_gone_after_s_nem_muito_depois() {
    let t0 = Instant::now();
    let (_d, srv, _h) = amb(200);
    let g = espera_estado(&srv.socket, CLAUDE, "gone", Duration::from_secs(GONE_S + 4));
    let el = t0.elapsed();
    assert_eq!(estado(&g), "gone", "{g}");
    assert!(
        el >= Duration::from_millis(GONE_S * 1000 - 1000),
        "cedo demais: {el:?}"
    );
    // +- 1 ciclo de 200 ms de poll, mais folga para o processo subir.
    assert!(
        el <= Duration::from_millis(GONE_S * 1000 + 1500),
        "tarde demais: {el:?}"
    );
}

// ---------- G17: precedencia ----------

#[test]
fn g17_erro_mais_ausencia_e_unknown_com_error_nunca_gone() {
    // Duas ausencias validas e depois so erros (a fila e longa o bastante para o teste inteiro).
    let mut fila = vec![Leitura::Ok(vec![]), Leitura::Ok(vec![])];
    fila.extend((0..40).map(|_| Leitura::Erro));
    let (_d, srv, h) = sobe_fila(200, &fila, &[], |_| {});
    h.espera_chamadas(6, Duration::from_secs(5));
    let a = agente(&srv.socket, CLAUDE);
    assert_eq!(estado(&a), "unknown", "{a}");
    assert!(tem(&a, "error"), "{a}");
    assert_ne!(estado(&a), "gone", "{a}");
    drop(srv);
}

// ---------- G5 e G6: eventos e events.resume ----------

/// Config com 32 agentes `a00`..`a31` (kind claude, pane wN:p1) e anel de `ring` eventos. Lota o anel rapido.
fn config_32(dir: &Path, h: &Herdr, ring: u64) -> std::path::PathBuf {
    let mut c = config(&h.bin, 200);
    let mut ags = serde_json::Map::new();
    for i in 0..32 {
        ags.insert(
            format!("a{:02}", i),
            json!({"kind": "claude", "sources": [{"type": "herdr", "pane": format!("w{}:p1", i + 1)}]}),
        );
    }
    c["agents"] = Value::Object(ags);
    c["ring"] = json!(ring);
    escreve_config(dir, &c)
}

/// Lista de 32 paineis, todos no mesmo `status`.
fn painel32(st: &str, seq: u64) -> Vec<Value> {
    (0..32)
        .map(|i| {
            info(
                &format!("w{}:p1", i + 1),
                "claude",
                st,
                &format!("t-{i}"),
                seq,
            )
        })
        .collect()
}

fn sobe_32(ring: u64, fila: &[Leitura]) -> (tempfile::TempDir, Servidor, Herdr) {
    let d = tmp();
    let h = Herdr::novo(d.path());
    h.fila(fila, &painel32("working", 1));
    let cfg = config_32(d.path(), &h, ring);
    let srv = sobe(&d.path().join("state"), &cfg);
    (d, srv, h)
}

#[test]
fn g5_seq_sem_lacunas_e_previous_state_encadeado() {
    let (_d, srv, h) = sobe_fila(
        200,
        &[
            Leitura::Ok(vec![info(PANE_C, CLAUDE, "working", "t-1", 1)]),
            Leitura::Ok(vec![info(PANE_C, CLAUDE, "blocked", "t-1", 2)]),
            Leitura::Ok(vec![info(PANE_C, CLAUDE, "working", "t-1", 3)]),
        ],
        &[info(PANE_C, CLAUDE, "working", "t-1", 3)],
        |_| {},
    );
    let mut ev = assina(&srv.socket);
    h.espera_chamadas(5, Duration::from_secs(5));
    let evs = coleta(&mut ev, Duration::from_secs(1));
    let mine: Vec<&Value> = evs.iter().filter(|e| e["id"] == CLAUDE).collect();
    assert!(
        mine.len() >= 3,
        "esperava >= 3 eventos de {CLAUDE}: {evs:?}"
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
    assert_eq!(mine[1]["previous_state"], "working", "{mine:?}");
    assert_eq!(mine[1]["state"], "blocked", "{mine:?}");
}

#[test]
fn g5_evidence_e_source_mudam_geram_evento() {
    // AMBIGUO: o estado inicial (agente nunca lido) tem evidence `none` e source null; a primeira leitura valida
    // com status `unknown` (sem error) muda so evidence e source, e isso deve gerar evento.
    let (_d, srv, h) = sobe_fila(
        200,
        &[Leitura::Ok(vec![info(PANE_C, CLAUDE, "unknown", "t-1", 1)])],
        &[info(PANE_C, CLAUDE, "unknown", "t-1", 1)],
        |_| {},
    );
    let mut ev = assina(&srv.socket);
    h.espera_chamadas(2, Duration::from_secs(5));
    let evs = coleta(&mut ev, Duration::from_secs(1));
    assert!(
        evs.iter()
            .any(|e| e["id"] == CLAUDE && e["evidence"] == "observed" && e["source"] == "herdr"),
        "esperava evento de evidence/source: {evs:?}"
    );
    let a = agente(&srv.socket, CLAUDE);
    assert_eq!(a["source"], "herdr", "{a}");
}

#[test]
fn g5_observed_at_freshness_e_cwd_nao_geram_evento() {
    let mut fila = Vec::new();
    for n in 0..8 {
        let mut i = info(PANE_C, CLAUDE, "working", "t-1", 1);
        i["cwd"] = json!(format!("/projeto/{n}"));
        fila.push(Leitura::Ok(vec![i]));
    }
    let depois = vec![info(PANE_C, CLAUDE, "working", "t-1", 1)];
    let (_d, srv, h) = sobe_fila(200, &fila, &depois, |_| {});
    let mut ev = assina(&srv.socket);
    h.espera_chamadas(8, Duration::from_secs(6));
    let evs = coleta(&mut ev, Duration::from_millis(1500));
    let mine: Vec<&Value> = evs.iter().filter(|e| e["id"] == CLAUDE).collect();
    assert_eq!(
        mine.len(),
        1,
        "so a primeira mudanca de estado deve gerar evento: {evs:?}"
    );
    assert_eq!(mine[0]["state"], "working", "{evs:?}");
}

#[test]
fn g6_retoma_valido_com_last_seq_igual_ao_head() {
    let (_d, srv, h) = sobe_padrao_h();
    espera_estado(&srv.socket, CLAUDE, "working", Duration::from_secs(5));
    let head = res(&rpc(&srv.socket, "agent.list", json!({"v": 1})))["head_seq"]
        .as_u64()
        .unwrap();
    let epoch = res(&rpc(&srv.socket, "agent.list", json!({"v": 1})))["epoch"].clone();
    let mut c = Conexao::abre(&srv.socket);
    let r = c.chama(
        "events.resume",
        json!({"v": 1, "epoch": epoch, "last_seq": head}),
    );
    assert_eq!(res(&r)["head_seq"].as_u64().unwrap(), head, "{r}");
    h.resposta(&[info(PANE_C, CLAUDE, "blocked", "t-1", 2)]);
    let e = c.evento(Duration::from_secs(5)).expect("evento ao vivo");
    assert_eq!(e["seq"].as_u64().unwrap(), head + 1, "{e}");
    assert_eq!(e["epoch"], epoch, "{e}");
}

/// Servidor de um agente com herdr falso (poll 200), ja com o `resposta` padrao.
fn sobe_padrao_h() -> (tempfile::TempDir, Servidor, Herdr) {
    let (d, srv, h) = amb(200);
    h.resposta(&[info(PANE_C, CLAUDE, "working", "t-1", 1)]);
    (d, srv, h)
}

#[test]
fn g6_resume_gap_informa_oldest_seq_e_fronteira_exata() {
    // Anel de 64. Tres trocas de estado em 32 agentes geram ~96 eventos; depois a fila para em `working`.
    let (_d, srv, h) = sobe_32(
        64,
        &[
            Leitura::Ok(painel32("working", 1)),
            Leitura::Ok(painel32("blocked", 2)),
            Leitura::Ok(painel32("working", 3)),
        ],
    );
    espera_agente(&srv.socket, "a31", Duration::from_secs(5), |a| {
        estado(a) == "working"
    });
    h.espera_chamadas(4, Duration::from_secs(5));
    std::thread::sleep(Duration::from_millis(400));
    let l = res(&rpc(&srv.socket, "agent.list", json!({"v": 1})));
    let head = l["head_seq"].as_u64().unwrap();
    let epoch = l["epoch"].clone();
    assert!(head >= 64 + 10, "anel nao foi lotado: head {head}");
    let oldest = head - 63; // AMBIGUO: seq retidos sao os 64 ultimos; oldest = head - ring + 1.
    // last_seq 0 esta antes do anel: gap com data.oldest_seq.
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
    // Fronteira: oldest - 1 retoma, e o primeiro evento recebido tem seq == oldest.
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
fn g6_epoch_mismatch_apos_reinicio_e_nada_persiste() {
    let (d, mut srv, h) = sobe_padrao_h();
    espera_estado(&srv.socket, CLAUDE, "working", Duration::from_secs(5));
    let antiga = res(&rpc(&srv.socket, "agent.list", json!({"v": 1})))["epoch"].clone();
    assert_eq!(
        srv.para(),
        Some(0),
        "SIGTERM deveria sair limpo (AMBIGUO: codigo 0)"
    );
    let _ = fs::remove_file(srv.socket.clone());
    let cfg = d.path().join("config.json");
    let srv2 = sobe(&d.path().join("state"), &cfg);
    let nova = res(&rpc(&srv2.socket, "agent.list", json!({"v": 1})))["epoch"].clone();
    assert_ne!(nova, antiga, "epoch deve mudar a cada partida");
    let r = rpc(
        &srv2.socket,
        "events.resume",
        json!({"v": 1, "epoch": antiga, "last_seq": 0}),
    );
    assert_eq!(cod(&r), "epoch_mismatch", "{r}");
    // Nada persiste: o estado de antes nao volta (o agente comeca de novo, sem evento de partida).
    let _ = h;
}

#[test]
fn g6_last_seq_maior_que_head_e_negativo_sao_invalid_envelope() {
    let (_d, srv, _h) = sobe_padrao_h();
    espera_estado(&srv.socket, CLAUDE, "working", Duration::from_secs(5));
    let l = res(&rpc(&srv.socket, "agent.list", json!({"v": 1})));
    let head = l["head_seq"].as_u64().unwrap();
    let epoch = l["epoch"].clone();
    let r = rpc(
        &srv.socket,
        "events.resume",
        json!({"v": 1, "epoch": epoch, "last_seq": head + 5}),
    );
    assert_eq!(cod(&r), "invalid_envelope", "{r}");
    let n = rpc(
        &srv.socket,
        "events.resume",
        json!({"v": 1, "epoch": epoch, "last_seq": -1}),
    );
    assert_eq!(cod(&n), "invalid_envelope", "{n}");
}

#[test]
fn g6_segunda_chamada_de_resume_na_mesma_conexao_e_invalid_request() {
    let (_d, srv, _h) = sobe_padrao_h();
    espera_estado(&srv.socket, CLAUDE, "working", Duration::from_secs(5));
    let l = res(&rpc(&srv.socket, "agent.list", json!({"v": 1})));
    let (head, epoch) = (l["head_seq"].as_u64().unwrap(), l["epoch"].clone());
    let mut c = Conexao::abre(&srv.socket);
    let a = c.chama(
        "events.resume",
        json!({"v": 1, "epoch": epoch.clone(), "last_seq": head}),
    );
    assert!(!is_err(&a), "{a}");
    let b = c.chama(
        "events.resume",
        json!({"v": 1, "epoch": epoch, "last_seq": head}),
    );
    assert_eq!(cod(&b), "invalid_request", "{b}");
}

#[test]
fn g6_anel_vazio_so_head_retoma_e_mais_um_falha() {
    // AMBIGUO: nao da para garantir anel vazio sem controlar a partida; testa a regra de `last_seq == head`
    // (retoma) e `head + 1` (invalid_envelope) no estado atual.
    let (_d, srv, _h) = amb(200);
    let l = res(&rpc(&srv.socket, "agent.list", json!({"v": 1})));
    let (head, epoch) = (l["head_seq"].as_u64().unwrap(), l["epoch"].clone());
    let ok = rpc(
        &srv.socket,
        "events.resume",
        json!({"v": 1, "epoch": epoch.clone(), "last_seq": head}),
    );
    assert!(!is_err(&ok), "last_seq == head deve retomar: {ok}");
    let mais = rpc(
        &srv.socket,
        "events.resume",
        json!({"v": 1, "epoch": epoch, "last_seq": head + 1}),
    );
    assert_eq!(cod(&mais), "invalid_envelope", "{mais}");
}

// ---------- G7: lista fechada do herdr ----------

#[test]
fn g7_herdr_so_recebe_agent_list_em_dez_ciclos() {
    let (_d, _srv, h) = amb(200);
    h.espera_chamadas(10, Duration::from_secs(8));
    let calls = h.chamadas();
    assert!(
        calls.len() >= 10,
        "esperava >= 10 chamadas, veio {}",
        calls.len()
    );
    for c in &calls {
        // AMBIGUO: a spec proibe --wait e os demais subcomandos; a forma exata `agent list` e lida literalmente.
        assert_eq!(
            c,
            &vec!["agent".to_string(), "list".to_string()],
            "chamada fora da lista fechada"
        );
    }
}

// ---------- G8: sem shell ----------

#[test]
fn g8_pane_com_metacaracteres_nao_executa_nada() {
    let d = tmp();
    let h = Herdr::novo(d.path());
    let m = d.path().join("PWNED");
    let pane = format!(
        "w1:p1;touch {m};$(touch {m}.2)`touch {m}.3` 'x' \"y\"",
        m = s(&m)
    );
    let mut c = config(&h.bin, 200);
    c["agents"] =
        json!({"claude": {"kind": "claude", "sources": [{"type": "herdr", "pane": pane}]}});
    h.resposta(&[info("w1:p1", "claude", "working", "t-1", 1)]);
    let cfg = escreve_config(d.path(), &c);
    let srv = sobe(&d.path().join("state"), &cfg);
    h.espera_chamadas(3, Duration::from_secs(5));
    for f in [
        m.clone(),
        d.path().join("PWNED.2"),
        d.path().join("PWNED.3"),
    ] {
        assert!(!f.exists(), "shell executou algo: {}", f.display());
    }
    // O pane configurado nao e o do herdr falso: painel ausente, nunca `working`.
    let a = agente(&srv.socket, "claude");
    assert_ne!(estado(&a), "working", "{a}");
}

#[test]
fn g8_herdr_bin_com_metacaracteres_nao_executa_shell() {
    let d = tmp();
    let m = d.path().join("PWNED-BIN");
    let bin = format!("{}/herdr ; touch {}", s(d.path()), s(&m));
    let mut c = config(Path::new(&bin), 200);
    c["poll_ms"] = json!(200);
    let cfg = escreve_config(d.path(), &c);
    let srv = sobe(&d.path().join("state"), &cfg);
    let ok = espera(Duration::from_secs(3), || {
        status(&srv.socket)["sources"]["herdr"]["last_error"] == "source_spawn_failed"
    });
    assert!(ok, "esperava source_spawn_failed: {}", status(&srv.socket));
    assert!(!m.exists(), "herdr_bin foi executado como shell");
}

// ---------- G9: protocolo ----------

fn srv_teste() -> (tempfile::TempDir, Servidor, Herdr) {
    amb(200)
}

/// Linha JSON de tamanho exato `total` bytes (com ou sem LF no fim). O tamanho e ajustado pelo `id` (string).
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
fn g9_campo_desconhecido_em_params_e_invalid_envelope_com_path() {
    let (_d, srv, _h) = srv_teste();
    let r = rpc(
        &srv.socket,
        "agent.get",
        json!({"v": 1, "id": CLAUDE, "extra": 1}),
    );
    assert_eq!(cod(&r), "invalid_envelope", "{r}");
    // AMBIGUO: o formato exato de data.path nao e fixado; exige so que o campo desconhecido seja citado.
    assert!(caminho(&r).contains("extra"), "{r}");
    assert!(!status(&srv.socket).is_null(), "daemon continua vivo");
}

#[test]
fn g9_linha_de_exatamente_1_mib_com_lf_e_aceita() {
    let (_d, srv, _h) = srv_teste();
    let mut c = Conexao::abre(&srv.socket);
    c.bruto(&linha_de_tamanho(1024 * 1024, true));
    let v = c
        .le(Duration::from_secs(5))
        .expect("resposta a linha de 1 MiB");
    // O id (string) ocupa quase toda a linha: ecoado de volta, a linha foi lida inteira.
    assert!(
        v["id"]
            .as_str()
            .map(|s| s.len() > 1024 * 1000)
            .unwrap_or(false),
        "id ecoado: {v}"
    );
    assert!(v.get("result").is_some() || v.get("error").is_some(), "{v}");
}

#[test]
fn g9_linha_de_1_mib_mais_um_byte_nao_e_processada() {
    let (_d, srv, _h) = srv_teste();
    let mut c = Conexao::abre(&srv.socket);
    c.bruto(&linha_de_tamanho(1024 * 1024 + 1, true));
    // AMBIGUO: a spec nao diz o codigo do excesso; exige so que nao haja `result` e que o daemon continue vivo.
    if let Some(v) = c.le(Duration::from_secs(5)) {
        assert!(
            v.get("result").is_none(),
            "linha acima de 1 MiB foi processada: {v}"
        );
    }
    assert!(!status(&srv.socket).is_null(), "daemon continua vivo");
}

#[test]
fn g9_eof_sem_lf_processa_a_linha() {
    let (_d, srv, _h) = srv_teste();
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
fn g9_utf8_invalido_e_parse_error() {
    let (_d, srv, _h) = srv_teste();
    let mut c = Conexao::abre(&srv.socket);
    c.bruto(b"{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"status\",\"params\":{\"v\":1,\"x\":\"\xff\"}}\n");
    let v = c
        .le(Duration::from_secs(5))
        .expect("resposta a UTF-8 invalido");
    assert_eq!(cod(&v), "parse_error", "{v}");
}

#[test]
fn g9_params_null_e_recusado() {
    let (_d, srv, _h) = srv_teste();
    let r = rpc(&srv.socket, "status", Value::Null);
    // AMBIGUO: o codigo exato nao e fixado (invalid_request ou invalid_envelope); exige erro com code string.
    assert!(
        matches!(cod(&r).as_str(), "invalid_request" | "invalid_envelope"),
        "{r}"
    );
}

#[test]
fn g9_pedido_sem_id_e_erro_e_daemon_continua() {
    let (_d, srv, _h) = srv_teste();
    let mut c = Conexao::abre(&srv.socket);
    c.envia_bruto(r#"{"jsonrpc":"2.0","method":"status","params":{"v":1}}"#);
    let v = c
        .le(Duration::from_secs(5))
        .expect("resposta ao pedido sem id");
    assert!(is_err(&v), "pedido sem id deve dar erro: {v}");
    assert!(!status(&srv.socket).is_null(), "daemon continua vivo");
}

#[test]
fn g9_lote_e_recusado() {
    let (_d, srv, _h) = srv_teste();
    let mut c = Conexao::abre(&srv.socket);
    c.envia_bruto(r#"[{"jsonrpc":"2.0","id":1,"method":"status","params":{"v":1}}]"#);
    // AMBIGUO: codigo do erro de lote nao fixado; exige erro (ou fechamento) e daemon vivo.
    if let Some(v) = c.le(Duration::from_secs(5)) {
        assert!(v.get("result").is_none(), "lote foi processado: {v}");
    }
    assert!(!status(&srv.socket).is_null(), "daemon continua vivo");
}

#[test]
fn g9_metodo_inexistente_e_method_not_found() {
    let (_d, srv, _h) = srv_teste();
    let r = rpc(&srv.socket, "agent.bind", json!({"v": 1}));
    assert_eq!(cod(&r), "method_not_found", "{r}");
}

#[test]
fn g9_versao_inesperada_e_unsupported_version() {
    let (_d, srv, _h) = srv_teste();
    let r = rpc(&srv.socket, "status", json!({"v": 2}));
    assert_eq!(cod(&r), "unsupported_version", "{r}");
}

// ---------- G10: seguranca ----------

fn modo_de(p: &Path) -> u32 {
    fs::metadata(p).expect("metadata").permissions().mode() & 0o777
}

#[test]
fn g10_diretorio_0700_e_socket_0600() {
    let (_d, srv, _h) = srv_teste();
    assert_eq!(modo_de(&srv.state), 0o700, "diretorio de estado");
    assert_eq!(modo_de(&srv.socket), 0o600, "socket");
}

#[test]
#[ignore = "exige um segundo uid (outro usuario do sistema); o ambiente de teste nao permite trocar de uid"]
fn g10_outro_uid_e_recusado_com_forbidden() {
    panic!("nao executado: precisa de outro uid");
}

// ---------- G11: privacidade ----------

#[test]
fn g11_sentinelas_de_titulo_extra_stderr_e_lixo_nunca_saem() {
    let (d, srv, h) = amb(200);
    let mut ev = assina(&srv.socket);
    let mut i = info(PANE_C, CLAUDE, "working", "t-1", 1);
    i["terminal_title"] = json!("SENTINELA-TITULO-AGENTD");
    i["campo_extra"] = json!("SENTINELA-EXTRA-AGENTD");
    h.resposta(&[i]);
    h.flag("stderr.flag", true);
    espera_estado(&srv.socket, CLAUDE, "working", Duration::from_secs(5));
    // Depois, JSON invalido com sentinela no stdout.
    h.flag("stderr.flag", false);
    h.flag("lixo.flag", true);
    let n = h.chamadas().len();
    h.espera_chamadas(n + 2, Duration::from_secs(5));
    let mut dump = String::new();
    dump.push_str(
        &serde_json::to_string(&rpc(
            &srv.socket,
            "agent.get",
            json!({"v": 1, "id": CLAUDE}),
        ))
        .unwrap(),
    );
    dump.push_str(
        &serde_json::to_string(&rpc(&srv.socket, "agent.list", json!({"v": 1}))).unwrap(),
    );
    dump.push_str(&serde_json::to_string(&rpc(&srv.socket, "status", json!({"v": 1}))).unwrap());
    for e in coleta(&mut ev, Duration::from_millis(500)) {
        dump.push_str(&e.to_string());
    }
    dump.push_str(&srv.stderr());
    for f in fs::read_dir(d.path().join("state"))
        .into_iter()
        .flatten()
        .flatten()
    {
        if let Ok(t) = fs::read_to_string(f.path()) {
            dump.push_str(&t);
        }
    }
    assert!(!dump.contains("SENTINELA"), "sentinela vazou: {dump}");
}

// ---------- G12: CLI ----------

fn cli_d(d: &Path, args: &[&str]) -> Saida {
    cli(d, args, &[])
}

#[test]
fn g12_get_sem_json_uma_linha_no_formato_da_spec() {
    let (d, srv, h) = amb(200);
    h.resposta(&[info(PANE_C, CLAUDE, "working", "t-1", 1)]);
    espera_estado(&srv.socket, CLAUDE, "working", Duration::from_secs(5));
    let r = cli_d(d.path(), &["get", CLAUDE, "--socket", s(&srv.socket)]);
    assert_eq!(r.code, Some(0), "{r:?}");
    let linhas: Vec<&str> = r.stdout.lines().collect();
    assert_eq!(linhas.len(), 1, "uma linha por agente: {r:?}");
    let l = linhas[0];
    assert!(l.starts_with("claude "), "{l}");
    assert!(
        l.contains("working")
            && l.contains("observed")
            && l.contains("herdr")
            && l.contains("idade="),
        "{l}"
    );
}

#[test]
fn g12_get_json_tem_envelope_schema_1() {
    let (d, srv, h) = amb(200);
    h.resposta(&[info(PANE_C, CLAUDE, "working", "t-1", 1)]);
    espera_estado(&srv.socket, CLAUDE, "working", Duration::from_secs(5));
    let r = cli_d(
        d.path(),
        &["get", CLAUDE, "--json", "--socket", s(&srv.socket)],
    );
    assert_eq!(r.code, Some(0), "{r:?}");
    let j: Value = serde_json::from_str(r.stdout.trim()).expect("saida JSON");
    assert_eq!(j["schema_version"], 1, "{j}");
    assert_eq!(j["ok"], true, "{j}");
    assert!(j["code"].is_string() && j["message"].is_string(), "{j}");
    // AMBIGUO: o formato exato de `data` nao e fixado; exige o estado e o id em algum lugar de data.
    let data = j["data"].to_string();
    assert!(data.contains("working") && data.contains(CLAUDE), "{j}");
}

#[test]
fn g12_list_com_kind_filtra_e_tem_json() {
    let (d, srv, h) = amb(200);
    h.resposta(&[
        info(PANE_C, CLAUDE, "working", "t-1", 1),
        info(PANE_X, CODEX, "blocked", "t-2", 1),
    ]);
    espera_estado(&srv.socket, CODEX, "blocked", Duration::from_secs(5));
    let sem = cli_d(d.path(), &["list", "--socket", s(&srv.socket)]);
    assert_eq!(sem.code, Some(0), "{sem:?}");
    assert!(
        sem.stdout.lines().count() >= 2,
        "uma linha por agente: {sem:?}"
    );
    let filtro = cli_d(
        d.path(),
        &[
            "list",
            "--kind",
            "codex",
            "--json",
            "--socket",
            s(&srv.socket),
        ],
    );
    assert_eq!(filtro.code, Some(0), "{filtro:?}");
    let j: Value = serde_json::from_str(filtro.stdout.trim()).expect("JSON");
    let txt = j["data"].to_string();
    assert!(
        txt.contains(CODEX) && !txt.contains(CLAUDE),
        "filtro por kind: {j}"
    );
}

#[test]
fn g12_health_0_com_agente_unknown_e_fonte_ok() {
    // Painel ausente: o agente e unknown, mas a execucao da fonte deu certo. health sai 0.
    let (d, srv, h) = amb(200);
    h.resposta(&[]);
    espera(Duration::from_secs(3), || {
        status(&srv.socket)["sources"]["herdr"]["ok"] == true
    });
    let r = cli_d(d.path(), &["health", "--socket", s(&srv.socket)]);
    assert_eq!(
        r.code,
        Some(0),
        "unknown por agente nao derruba health: {r:?}"
    );
}

#[test]
fn g12_health_falha_quando_a_execucao_da_fonte_falha() {
    let (d, srv, h) = amb(200);
    h.flag("saida.flag", true);
    espera(Duration::from_secs(3), || {
        status(&srv.socket)["sources"]["herdr"]["ok"] == false
    });
    let r = cli_d(d.path(), &["health", "--socket", s(&srv.socket)]);
    // AMBIGUO: o codigo de saida de health com fonte falha nao e fixado alem de ser diferente de 0; a spec
    // aponta "1 falha do servidor".
    assert_eq!(r.code, Some(1), "{r:?}");
}

#[test]
fn g12_status_cli_com_e_sem_json() {
    let (d, srv, _h) = amb(200);
    let a = cli_d(d.path(), &["status", "--socket", s(&srv.socket)]);
    assert_eq!(a.code, Some(0), "{a:?}");
    let b = cli_d(d.path(), &["status", "--json", "--socket", s(&srv.socket)]);
    assert_eq!(b.code, Some(0), "{b:?}");
    let j: Value = serde_json::from_str(b.stdout.trim()).expect("JSON");
    assert_eq!(j["schema_version"], 1, "{j}");
    assert_eq!(j["ok"], true, "{j}");
}

#[test]
fn g12_codigos_de_saida_2_uso_incorreto() {
    let (d, srv, _h) = amb(200);
    // sem socket nenhum (nem flag, nem env, nem --state-dir)
    assert_eq!(cli_d(d.path(), &["get", CLAUDE]).code, Some(2));
    // --since sem --epoch e vice-versa
    assert_eq!(
        cli_d(
            d.path(),
            &["watch", "--since", "1", "--socket", s(&srv.socket)]
        )
        .code,
        Some(2)
    );
    assert_eq!(
        cli_d(
            d.path(),
            &["watch", "--epoch", "1", "--socket", s(&srv.socket)]
        )
        .code,
        Some(2)
    );
}

#[test]
fn g12_codigo_3_agente_inexistente_e_unknown_agent() {
    let (d, srv, _h) = amb(200);
    let r = cli_d(d.path(), &["get", "nao-existe", "--socket", s(&srv.socket)]);
    assert_eq!(r.code, Some(3), "{r:?}");
    let j = cli_d(
        d.path(),
        &["get", "nao-existe", "--json", "--socket", s(&srv.socket)],
    );
    assert_eq!(j.code, Some(3), "{j:?}");
    let v: Value = serde_json::from_str(j.stdout.trim()).expect("JSON");
    assert_eq!(v["ok"], false, "{v}");
    assert_eq!(v["code"], "unknown_agent", "{v}");
}

#[test]
fn g12_codigo_4_daemon_indisponivel() {
    let (d, _srv, _h) = amb(200);
    let morto = d.path().join("nao-existe.sock");
    let r = cli_d(d.path(), &["get", CLAUDE, "--socket", s(&morto)]);
    assert_eq!(r.code, Some(4), "{r:?}");
}

#[test]
fn g12_socket_por_state_dir_e_por_env() {
    let (d, srv, h) = amb(200);
    h.resposta(&[info(PANE_C, CLAUDE, "working", "t-1", 1)]);
    espera_estado(&srv.socket, CLAUDE, "working", Duration::from_secs(5));
    let a = cli_d(
        d.path(),
        &["get", CLAUDE, "--state-dir", s(&d.path().join("state"))],
    );
    assert_eq!(a.code, Some(0), "--state-dir: {a:?}");
    let sock = s(&srv.socket).to_string();
    let b = cli(
        d.path(),
        &["get", CLAUDE],
        &[("AGENTD_SOCKET", sock.as_str())],
    );
    assert_eq!(b.code, Some(0), "AGENTD_SOCKET: {b:?}");
}

#[test]
fn g12_watch_imprime_uma_linha_por_state_changed() {
    let (d, srv, h) = amb(200);
    h.resposta(&[info(PANE_C, CLAUDE, "working", "t-1", 1)]);
    espera_estado(&srv.socket, CLAUDE, "working", Duration::from_secs(5));
    let out = d.path().join("watch.out");
    let mut child = std::process::Command::new(BIN)
        .args(["watch", "--socket", s(&srv.socket)])
        .env("HOME", d.path())
        .stdin(std::process::Stdio::null())
        .stdout(std::fs::File::create(&out).unwrap())
        .stderr(std::process::Stdio::null())
        .spawn()
        .expect("executar watch");
    std::thread::sleep(Duration::from_millis(500));
    h.resposta(&[info(PANE_C, CLAUDE, "blocked", "t-2", 2)]);
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

// ---------- G13: configuracao ----------

/// Roda `serve` com o texto de config dado; devolve o codigo de saida (None se nao saiu em 10 s).
fn serve_com_texto(texto: &str) -> Option<i32> {
    let d = tmp();
    let p = d.path().join("config.json");
    fs::write(&p, texto).expect("escrever config");
    cli(
        d.path(),
        &[
            "serve",
            "--config",
            s(&p),
            "--state-dir",
            s(&d.path().join("state")),
        ],
        &[],
    )
    .code
}

/// Config valida de teste com um ajuste; herdr inexistente (nao sobe de qualquer jeito se a config for recusada).
fn cfg_ajustada(ajuste: impl FnOnce(&mut Value)) -> String {
    let mut c = config(Path::new("/nonexistent/herdr-agentd"), 200);
    ajuste(&mut c);
    c.to_string()
}

#[test]
fn g13_campo_desconhecido_nao_sobe() {
    let t = cfg_ajustada(|c| c["campo_novo"] = json!(1));
    assert_eq!(serve_com_texto(&t), Some(1));
}

#[test]
fn g13_chave_duplicada_nao_sobe() {
    let t = cfg_ajustada(|_| {});
    let dup = t.replacen("\"poll_ms\":200", "\"poll_ms\":200,\"poll_ms\":300", 1);
    assert_ne!(dup, t, "a chave duplicada precisa estar no texto");
    assert_eq!(serve_com_texto(&dup), Some(1));
}

#[test]
fn g13_limites_de_valor_recusados() {
    let casos = [
        ("poll_ms", json!(199)),
        ("poll_ms", json!(10001)),
        ("source_timeout_ms", json!(199)),
        ("source_timeout_ms", json!(30001)),
        ("gone_after_s", json!(4)),
        ("gone_after_s", json!(86401)),
        ("idle_confirm_polls", json!(0)),
        ("idle_confirm_polls", json!(6)),
        ("idle_confirm_ms", json!(30001)),
        ("ring", json!(63)),
        ("ring", json!(65537)),
    ];
    for (campo, v) in casos {
        let t = cfg_ajustada(|c| c[campo] = v.clone());
        assert_eq!(
            serve_com_texto(&t),
            Some(1),
            "{campo}={v} deveria ser recusado"
        );
    }
}

#[test]
fn g13_stale_deve_exceder_poll_mais_timeout() {
    // poll 200 + timeout 400 = 600: igual ja e recusado (regra "stale > poll + timeout", estrita).
    let t = cfg_ajustada(|c| c["stale_after_ms"] = json!(600));
    assert_eq!(serve_com_texto(&t), Some(1));
}

#[test]
fn g13_pane_repetido_e_id_repetido_nao_sobem() {
    let pane_rep = cfg_ajustada(|c| {
        c["agents"]["codex-blocks"]["sources"][0]["pane"] = json!("w1:p1");
    });
    assert_eq!(serve_com_texto(&pane_rep), Some(1), "pane repetido");
    let t = cfg_ajustada(|_| {});
    let id_rep = t.replacen(
        "\"claude\":",
        "\"claude\":{\"kind\":\"claude\",\"sources\":[{\"type\":\"herdr\",\"pane\":\"w9:p1\"}]},\"claude\":",
        1,
    );
    assert_eq!(
        serve_com_texto(&id_rep),
        Some(1),
        "id repetido (chave duplicada em agents)"
    );
}

#[test]
fn g13_mais_de_32_agentes_sources_invalidos_e_kind_invalido_nao_sobem() {
    let mut ags = serde_json::Map::new();
    for i in 0..33 {
        ags.insert(format!("a{i:02}"), json!({"kind": "claude", "sources": [{"type": "herdr", "pane": format!("w{}:p1", i + 1)}]}));
    }
    let mais = cfg_ajustada(|c| c["agents"] = Value::Object(ags.clone()));
    assert_eq!(serve_com_texto(&mais), Some(1), "33 agentes");
    let vazio = cfg_ajustada(|c| c["agents"]["claude"]["sources"] = json!([]));
    assert_eq!(serve_com_texto(&vazio), Some(1), "sources vazio");
    let dois = cfg_ajustada(|c| {
        c["agents"]["claude"]["sources"] = json!([
            {"type": "herdr", "pane": "w7:p1"},
            {"type": "herdr", "pane": "w8:p1"}
        ])
    });
    assert_eq!(serve_com_texto(&dois), Some(1), "dois sources");
    let kind = cfg_ajustada(|c| c["agents"]["claude"]["kind"] = json!("pi"));
    assert_eq!(serve_com_texto(&kind), Some(1), "kind fora de claude|codex");
}

// ---------- G14: reinicio ----------

#[test]
fn g14_reinicio_sobe_de_novo_com_epoch_nova_e_nada_persiste() {
    let (d, mut srv, h) = amb(200);
    h.resposta(&[info(PANE_C, CLAUDE, "working", "t-1", 1)]);
    espera_estado(&srv.socket, CLAUDE, "working", Duration::from_secs(5));
    let antiga = res(&rpc(&srv.socket, "agent.list", json!({"v": 1})))["epoch"].clone();
    // AMBIGUO: o codigo de saida do SIGTERM nao e fixado; exige apenas que o processo tenha saido.
    assert!(srv.para().is_some(), "servidor nao saiu com SIGTERM");
    let _ = fs::remove_file(&srv.socket);
    let srv2 = sobe(&d.path().join("state"), &d.path().join("config.json"));
    let nova = res(&rpc(&srv2.socket, "agent.list", json!({"v": 1})))["epoch"].clone();
    assert_ne!(nova, antiga, "epoch deve mudar a cada partida");
    // AMBIGUO: lista de arquivos do diretorio de estado nao fixada; a spec proibe persistencia, entao so o socket.
    let arquivos: Vec<String> = fs::read_dir(d.path().join("state"))
        .expect("diretorio de estado")
        .flatten()
        .map(|e| e.file_name().to_string_lossy().to_string())
        .collect();
    assert!(
        arquivos.iter().all(|a| a == "agentd.sock"),
        "arquivos inesperados no estado: {arquivos:?}"
    );
}

// ---------- G15 e G16: confirmacao de idle (fila de leituras, sem depender de tempo) ----------

fn ajusta_confirm(ms: u64, polls: u64) -> impl FnOnce(&mut Value) {
    move |c: &mut Value| {
        c["idle_confirm_ms"] = json!(ms);
        c["idle_confirm_polls"] = json!(polls);
    }
}

fn leitura(st: &str, term: &str, seq: u64) -> Leitura {
    Leitura::Ok(vec![info(PANE_C, CLAUDE, st, term, seq)])
}

fn houve_idle(evs: &[Value]) -> bool {
    evs.iter().any(|e| e["state"] == "idle")
}

#[test]
fn g15_idle_isolado_entre_working_nao_publica() {
    let fila = [
        leitura("working", "t-1", 1),
        leitura("idle", "t-1", 1),
        leitura("working", "t-1", 1),
    ];
    let (_d, srv, h) = sobe_fila(
        200,
        &fila,
        &[info(PANE_C, CLAUDE, "working", "t-1", 1)],
        ajusta_confirm(0, 2),
    );
    let mut ev = assina(&srv.socket);
    h.espera_chamadas(6, Duration::from_secs(5));
    let evs = coleta(&mut ev, Duration::from_millis(1200));
    assert!(
        !houve_idle(&evs),
        "idle isolado nao pode ser publicado: {evs:?}"
    );
    assert_eq!(estado(&agente(&srv.socket, CLAUDE)), "working");
}

#[test]
fn g15_idle_unconfirmed_visivel_e_confirma_so_apos_idle_confirm_ms() {
    let (_d, srv, h) = sobe_fila(
        200,
        &[leitura("working", "t-1", 1), leitura("idle", "t-1", 1)],
        &[leitura_info("idle", "t-1", 1)],
        ajusta_confirm(1000, 2),
    );
    h.espera_chamadas(2, Duration::from_secs(5));
    let a = agente(&srv.socket, CLAUDE);
    assert_eq!(
        estado(&a),
        "working",
        "idle nao confirmado nao e publicado: {a}"
    );
    assert!(tem(&a, "idle_unconfirmed"), "{a}");
    let t_s = Instant::now();
    let _ = espera_estado(&srv.socket, CLAUDE, "idle", Duration::from_secs(5));
    let depois = t_s.elapsed();
    // Confirmacao pelo tempo: idle_confirm_ms = 1000 desde a primeira leitura idle (tolerancia de 500 ms).
    assert!(
        depois >= Duration::from_millis(500),
        "confirmou cedo demais: {depois:?}"
    );
    assert_eq!(estado(&agente(&srv.socket, CLAUDE)), "idle");
}

fn leitura_info(st: &str, term: &str, seq: u64) -> Value {
    info(PANE_C, CLAUDE, st, term, seq)
}

#[test]
fn g15_primeira_leitura_idle_no_inicio_passa_pela_confirmacao() {
    // Partida com idle: nao pode publicar idle antes de idle_confirm_ms (1000) desde a primeira leitura.
    let t0 = Instant::now();
    let (_d, srv, _h) = sobe_fila(
        200,
        &[],
        &[leitura_info("idle", "t-1", 1)],
        ajusta_confirm(1000, 2),
    );
    let mut ev = assina(&srv.socket);
    let fim = Instant::now() + Duration::from_secs(5);
    let mut quando = None;
    while Instant::now() < fim && quando.is_none() {
        if let Some(e) = ev.evento(Duration::from_millis(200)) {
            if e["state"] == "idle" {
                quando = Some(t0.elapsed());
            }
        }
    }
    let q = quando.expect("idle nunca publicado");
    assert!(
        q >= Duration::from_millis(800),
        "idle publicado antes da confirmacao: {q:?}"
    );
}

#[test]
fn g15_idle_com_polls_1_e_ms_0_e_imediato() {
    let (_d, srv, _h) = sobe_fila(
        200,
        &[],
        &[leitura_info("idle", "t-1", 1)],
        ajusta_confirm(0, 1),
    );
    let a = espera_estado(&srv.socket, CLAUDE, "idle", Duration::from_millis(1500));
    assert_eq!(
        estado(&a),
        "idle",
        "idle_confirm_polls 1 e ms 0 deveria ser imediato: {a}"
    );
}

#[test]
fn g15_working_e_blocked_sao_imediatos() {
    let (_d, srv, _h) = sobe_fila(
        200,
        &[],
        &[leitura_info("blocked", "t-1", 1)],
        ajusta_confirm(5000, 2),
    );
    let a = espera_estado(&srv.socket, CLAUDE, "blocked", Duration::from_millis(1200));
    assert_eq!(estado(&a), "blocked", "blocked e imediato: {a}");
}

#[test]
fn g15_erro_depois_idle_nao_confirmado_nao_publica_idle() {
    let fila = [
        leitura("working", "t-1", 1),
        Leitura::Erro,
        leitura("idle", "t-1", 1),
    ];
    let (_d, srv, h) = sobe_fila(
        200,
        &fila,
        &[leitura_info("working", "t-1", 1)],
        ajusta_confirm(0, 2),
    );
    let mut ev = assina(&srv.socket);
    h.espera_chamadas(6, Duration::from_secs(5));
    let evs = coleta(&mut ev, Duration::from_millis(800));
    assert!(
        !houve_idle(&evs),
        "idle logo apos erro nao pode ser publicado: {evs:?}"
    );
}

#[test]
fn g15_erro_depois_idle_confirmado_publica_idle() {
    let fila = [
        leitura("working", "t-1", 1),
        Leitura::Erro,
        leitura("idle", "t-1", 1),
        leitura("idle", "t-1", 1),
    ];
    let (_d, srv, _h) = sobe_fila(
        200,
        &fila,
        &[leitura_info("idle", "t-1", 1)],
        ajusta_confirm(0, 2),
    );
    let a = espera_estado(&srv.socket, CLAUDE, "idle", Duration::from_secs(5));
    assert_eq!(estado(&a), "idle", "{a}");
    assert!(!tem(&a, "error") && !tem(&a, "idle_unconfirmed"), "{a}");
}

#[test]
fn g16_zeragem_da_janela_nenhuma_sequencia_confirma() {
    // Cada caso: uma leitura idle, uma quebra, outra idle. A fila e seguida de `working` (fallback),
    // entao nunca deve haver evento `idle`. Polls 2, idle_confirm_ms 0.
    let quebras: Vec<(&str, Vec<Leitura>)> = vec![
        (
            "erro",
            vec![
                leitura("idle", "t-1", 1),
                Leitura::Erro,
                leitura("idle", "t-1", 1),
            ],
        ),
        (
            "ausente",
            vec![
                leitura("idle", "t-1", 1),
                Leitura::Ok(vec![]),
                leitura("idle", "t-1", 1),
            ],
        ),
        (
            "unknown",
            vec![
                leitura("idle", "t-1", 1),
                leitura("unknown", "t-1", 1),
                leitura("idle", "t-1", 1),
            ],
        ),
        (
            "state_change_seq",
            vec![leitura("idle", "t-1", 1), leitura("idle", "t-1", 2)],
        ),
        (
            "terminal_id",
            vec![leitura("idle", "t-1", 1), leitura("idle", "t-2", 1)],
        ),
        (
            "trava",
            vec![
                leitura("idle", "t-1", 1),
                Leitura::Trava,
                leitura("idle", "t-1", 1),
            ],
        ),
    ];
    for (nome, fila) in quebras {
        let (_d, srv, h) = sobe_fila(
            200,
            &fila,
            &[leitura_info("working", "t-1", 1)],
            ajusta_confirm(0, 2),
        );
        let mut ev = assina(&srv.socket);
        h.espera_chamadas(fila.len() + 3, Duration::from_secs(6));
        let evs = coleta(&mut ev, Duration::from_millis(600));
        assert!(
            !houve_idle(&evs),
            "quebra '{nome}' nao zerou a janela: {evs:?}"
        );
    }
}

#[test]
fn g16_controle_positivo_duas_idle_iguais_confirmam() {
    let (_d, srv, _h) = sobe_fila(
        200,
        &[leitura("idle", "t-1", 1), leitura("idle", "t-1", 1)],
        &[leitura_info("idle", "t-1", 1)],
        ajusta_confirm(0, 2),
    );
    let a = espera_estado(&srv.socket, CLAUDE, "idle", Duration::from_secs(5));
    assert_eq!(estado(&a), "idle", "{a}");
}

#[test]
fn g19_terminal_id_muda_gera_evento_erro_e_zera_janela() {
    let fila = [
        leitura("idle", "t-1", 1),
        leitura("idle", "t-1", 1),
        leitura("idle", "t-2", 1),
    ];
    let (_d, srv, _h) = sobe_fila(
        200,
        &fila,
        &[leitura_info("idle", "t-2", 1)],
        ajusta_confirm(0, 2),
    );
    let mut ev = assina(&srv.socket);
    let evs = coleta(&mut ev, Duration::from_secs(3));
    let troca = evs.iter().any(|e| {
        e["id"] == CLAUDE
            && e["previous_state"] == "idle"
            && e["state"] == "unknown"
            && e["marks"]
                .as_array()
                .is_some_and(|m| m.iter().any(|x| x.as_str() == Some("error")))
    });
    assert!(
        troca,
        "esperava idle -> unknown,error na troca de terminal: {evs:?}"
    );
    // Depois de confirmar o novo terminal (duas leituras), volta a idle.
    assert_eq!(
        estado(&espera_estado(
            &srv.socket,
            CLAUDE,
            "idle",
            Duration::from_secs(3)
        )),
        "idle"
    );
}

#[test]
fn g20_snapshot_e_head_do_mesmo_instante_e_replay_sem_lacunas() {
    // 40 trocas (working/blocked) a cada 200 ms. Cada snapshot (agent.get) traz estado e head_seq do mesmo instante:
    // o estado tem de ser o que os eventos ate head_seq determinam.
    let fila: Vec<Leitura> = (0..40)
        .map(|i| leitura(if i % 2 == 0 { "working" } else { "blocked" }, "t-1", 1))
        .collect();
    let (_d, srv, _h) = sobe_fila(200, &fila, &[leitura_info("working", "t-1", 1)], |_| {});
    let mut ev = assina(&srv.socket);
    let mut snaps: Vec<(String, u64)> = Vec::new();
    let fim = Instant::now() + Duration::from_millis(8500);
    while Instant::now() < fim {
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
    for w in seqs.windows(2) {
        assert_eq!(w[1], w[0] + 1, "lacuna ou duplicata no replay+ao vivo");
    }
    for (st, head) in &snaps {
        let esperado = evs
            .iter()
            .filter(|e| e["id"] == CLAUDE && e["seq"].as_u64().unwrap() <= *head)
            .last()
            .map(|e| e["state"].as_str().unwrap().to_string())
            .unwrap_or_else(|| "unknown".to_string());
        assert_eq!(
            st, &esperado,
            "snapshot com head {head} nao bate com os eventos ate ali"
        );
    }
}

// ---------- G21: subprocesso ----------

#[test]
fn g21_travamento_nao_derruba_e_nunca_ha_dois_herdr_em_voo() {
    let (_d, srv, h) = amb(200);
    h.resposta(&[info(PANE_C, CLAUDE, "working", "t-1", 1)]);
    espera_estado(&srv.socket, CLAUDE, "working", Duration::from_secs(5));
    h.flag("trava.flag", true);
    let n0 = h.chamadas().len();
    std::thread::sleep(Duration::from_millis(1500));
    let n1 = h.chamadas().len();
    // Com source_timeout 400 ms e um herdr por vez, ha no maximo ~3 inicios em 1,5 s. Sem serializacao haveria ~7.
    assert!(
        n1 - n0 <= 4,
        "{} inicios em 1,5 s: possivel herdr em voo em paralelo",
        n1 - n0
    );
    assert!(n1 > n0, "o ciclo parou");
    assert!(
        !status(&srv.socket).is_null(),
        "daemon nao responde com fonte travada"
    );
}

#[test]
fn g21_descendente_vivo_nao_impede_os_ciclos() {
    let (_d, srv, h) = amb(200);
    h.resposta(&[info(PANE_C, CLAUDE, "working", "t-1", 1)]);
    espera_estado(&srv.socket, CLAUDE, "working", Duration::from_secs(5));
    h.flag("filho.flag", true);
    let n0 = h.chamadas().len();
    std::thread::sleep(Duration::from_millis(2000));
    assert!(
        h.chamadas().len() >= n0 + 3,
        "ciclos pararam com descendente vivo"
    );
    assert!(!status(&srv.socket).is_null(), "daemon nao responde");
}

// ---------- G22: assinante lento ----------

#[test]
fn g22_assinante_lento_recebe_closed_e_os_polls_seguem() {
    // 30 trocas de 32 agentes = ~960 eventos sem leitura do cliente: o limite de 256 da fila estoura.
    let fila: Vec<Leitura> = (0..30)
        .map(|i| Leitura::Ok(painel32(if i % 2 == 0 { "working" } else { "blocked" }, 1)))
        .collect();
    let (_d, srv, _h) = sobe_fila_32(&fila);
    let mut lento = assina(&srv.socket); // nunca le
    let t = Instant::now();
    let _ = agente(&srv.socket, "a00");
    assert!(
        t.elapsed() < Duration::from_secs(1),
        "agent.get demorou com assinante lento"
    );
    std::thread::sleep(Duration::from_secs(7));
    let t2 = Instant::now();
    let _ = agente(&srv.socket, "a00");
    assert!(
        t2.elapsed() < Duration::from_secs(1),
        "poll atrasado por assinante lento"
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

/// Como `sobe_32` com a fila dada e fallback de 32 agentes `working`.
fn sobe_fila_32(fila: &[Leitura]) -> (tempfile::TempDir, Servidor, Herdr) {
    sobe_32(4096, fila)
}

// ---------- G23: launch_pending e screen_detection_skipped ----------

#[test]
fn g23_launch_pending_e_screen_skipped_sao_unknown_sem_error() {
    for campo in ["launch_pending", "screen_detection_skipped"] {
        let (_d, srv, h) = amb(200);
        let mut i = info(PANE_C, CLAUDE, "working", "t-1", 1);
        i[campo] = json!(true);
        h.resposta(&[i]);
        h.espera_chamadas(3, Duration::from_secs(5));
        let a = agente(&srv.socket, CLAUDE);
        assert_eq!(estado(&a), "unknown", "{campo}: {a}");
        assert!(!tem(&a, "error"), "{campo} nao e falha de leitura: {a}");
    }
}

// ---------- formato do Agente, status e lista (secoes 3 e 5) ----------

#[test]
fn agente_tem_os_campos_da_secao_3_com_tipos_certos() {
    let (_d, srv, h) = amb(200);
    h.resposta(&[info(PANE_C, CLAUDE, "working", "t-1", 1)]);
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
    assert_eq!(a["source"], "herdr", "{a}");
    assert_eq!(a["cwd"], "/tmp/projeto", "cwd vem da fonte: {a}");
    assert!(a["last_known"].is_null(), "{a}");
    // Tempo em milissegundos Unix inteiros (hora de parede): depois de 2020 e nao no futuro distante.
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
fn agente_freshness_e_null_sem_leitura_valida() {
    // Fonte com execucao falha desde a partida: nenhuma leitura valida ainda.
    let d = tmp();
    let h = Herdr::novo(d.path());
    h.flag("saida.flag", true);
    let cfg = config_com(d.path(), &h, 200, |_| {});
    let srv = sobe(&d.path().join("state"), &cfg);
    h.espera_chamadas(2, Duration::from_secs(5));
    let a = agente(&srv.socket, CLAUDE);
    assert!(
        a["freshness_ms"].is_null(),
        "freshness antes da primeira leitura valida: {a}"
    );
    assert!(
        a["last_seen"].is_null(),
        "last_seen sem painel presente: {a}"
    );
}

#[test]
fn agente_epoch_e_string_decimal_de_64_bits() {
    let (_d, srv, _h) = amb(200);
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
fn agente_erro_unknown_agent() {
    let (_d, srv, _h) = amb(200);
    let r = rpc(
        &srv.socket,
        "agent.get",
        json!({"v": 1, "id": "nao-existe"}),
    );
    assert_eq!(cod(&r), "unknown_agent", "{r}");
}

#[test]
fn agent_list_ordenada_por_id_e_filtrada_por_kind() {
    let (_d, srv, h) = amb(200);
    h.resposta(&[
        info(PANE_C, CLAUDE, "working", "t-1", 1),
        info(PANE_X, CODEX, "blocked", "t-2", 1),
    ]);
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
    let s_ids: Vec<&str> = f["agents"]
        .as_array()
        .unwrap()
        .iter()
        .map(|a| a["kind"].as_str().unwrap())
        .collect();
    assert!(
        !s_ids.is_empty() && s_ids.iter().all(|k| *k == "codex"),
        "{f}"
    );
}

#[test]
fn status_tem_forma_da_secao_5_e_last_error_so_codigo() {
    let (_d, srv, h) = amb(200);
    h.flag("saida.flag", true);
    h.espera_chamadas(2, Duration::from_secs(5));
    let st = status(&srv.socket);
    assert!(
        st["epoch"].is_string() && st["head_seq"].is_u64() && st["uptime_s"].is_number(),
        "{st}"
    );
    assert!(st["agents"][CLAUDE]["state"].is_string(), "{st}");
    assert_eq!(st["sources"]["herdr"]["ok"], false, "{st}");
    let e = st["sources"]["herdr"]["last_error"]
        .as_str()
        .unwrap_or_else(|| panic!("last_error codigo: {st}"));
    assert!(
        matches!(
            e,
            "source_spawn_failed"
                | "source_timeout"
                | "source_exit_nonzero"
                | "source_bad_json"
                | "source_too_large"
                | "duplicate_pane"
                | "kind_mismatch"
                | "status_unmapped"
        ),
        "codigo fora da lista fixa: {e}"
    );
}

//! Campainha (spec secao 8) contra um herdr falso (script shell que grava o argv).
mod common;

use common::*;
use serde_json::{Value, json};
use std::path::Path;
use std::thread;
use std::time::{Duration, Instant};
use tempfile::TempDir;

const CLAUDE: &str = "claude-main";
const CODEX: &str = "codex-web";

// v1.1 (secao C.3): texto fixo e literal, sem nome do destinatario.
const TEXTO_1: &str = "[chat] 1 pendente(s). Rode: chatd inbox --take";

fn pausa(ms: u64) {
    thread::sleep(Duration::from_millis(ms));
}

fn conta_bell_failed(evs: &[Value]) -> usize {
    evs.iter().filter(|e| e["type"] == "bell_failed").count()
}

/// Como amb_campainha, mas com auth obrigatoria: o estado da campainha so sai pelo
/// `mailbox.counts` autenticado (spec v1.8 B.0). Devolve os tokens de claude-main e codex-web.
fn amb_campainha_auth(poll_ms: u64, rearm_s: u64) -> (TempDir, Servidor, Herdr, (String, String)) {
    let d = tmp();
    let h = Herdr::novo(d.path());
    let cfg = escreve_config(d.path(), &config_req_json(&h.bin, poll_ms, rearm_s));
    let srv = sobe(&d.path().join("state"), &cfg);
    let tc = token_new(&srv.state, CLAUDE, &d.path().join("claude.tok"));
    let tx = token_new(&srv.state, CODEX, &d.path().join("codex.tok"));
    (d, srv, h, (tc, tx))
}

/// `mailbox.counts` da propria identidade (conexao autenticada como `nome`).
fn caixa(socket: &Path, nome: &str, token: &str) -> Value {
    res(&conexao_auth(socket, nome, token).chama("mailbox.counts", json!({"v": 1})))
}

fn bell_de(socket: &Path, nome: &str, token: &str) -> String {
    caixa(socket, nome, token)["bell"]
        .as_str()
        .unwrap_or("<ausente>")
        .to_string()
}

/// AVISO de claude-main para `para`, por conexao autenticada.
fn envia_claude(socket: &Path, token: &str, id: &str, para: &str, corpo: &str) -> Value {
    res(&conexao_auth(socket, CLAUDE, token).chama(
        "message.send",
        msg(id, CLAUDE, para, "AVISO", "assunto", corpo),
    ))
}

/// `message.ack` feito pelo proprio `nome`, por conexao autenticada.
fn ack_como(socket: &Path, nome: &str, token: &str, id: &str, stage: &str) -> Value {
    res(&conexao_auth(socket, nome, token).chama(
        "message.ack",
        json!({"v": 1, "id": id, "by": nome, "stage": stage}),
    ))
}

// ---------- quando toca ----------

#[test]
fn toca_com_idle_com_texto_fixo_e_argv_sem_wait() {
    let (_d, srv, h) = amb_campainha(50, 600);
    envia_ok(&srv.socket, "c-1", "codex-web", "corpo");
    h.estado("idle");
    assert!(
        h.espera_chamadas(1, Duration::from_secs(5)),
        "campainha nao tocou com idle"
    );
    let c = h.chamadas();
    assert_eq!(c.len(), 1, "uma campainha: {c:?}");
    let argv: Vec<&str> = c[0].iter().map(String::as_str).collect();
    assert_eq!(
        argv,
        vec!["agent", "prompt", "w2:p1", TEXTO_1],
        "argv exato, sem --wait"
    );
}

#[test]
fn toca_com_done() {
    let (_d, srv, h) = amb_campainha(50, 600);
    envia_ok(&srv.socket, "c-2", "codex-web", "corpo");
    h.estado("done");
    assert!(
        h.espera_chamadas(1, Duration::from_secs(5)),
        "campainha nao tocou com done"
    );
    assert_eq!(h.textos()[0], TEXTO_1);
}

#[test]
fn nao_toca_com_working_blocked_ou_estado_desconhecido() {
    let (_d, srv, h) = amb_campainha(50, 600);
    envia_ok(&srv.socket, "nt-1", "codex-web", "corpo");
    for estado in ["working", "blocked", "unknown", "", "IDLE"] {
        h.estado(estado);
        pausa(400);
        assert!(h.chamadas().is_empty(), "tocou com estado {estado:?}");
    }
    // controle: com idle o mesmo servidor toca, entao o silencio acima veio do portao de estado
    h.estado("idle");
    assert!(
        h.espera_chamadas(1, Duration::from_secs(5)),
        "nao tocou com idle depois dos outros estados"
    );
}

#[test]
fn nao_toca_com_bell_none() {
    let (_d, srv, h) = amb_campainha(50, 600);
    envia_ok(&srv.socket, "bn-1", "claude-main", "corpo");
    h.estado("idle");
    pausa(800);
    assert!(h.chamadas().is_empty(), "bell none nunca toca");
    envia_ok(&srv.socket, "bn-2", "codex-web", "corpo");
    assert!(
        h.espera_chamadas(1, Duration::from_secs(5)),
        "controle: codex-web deveria tocar"
    );
    let c = h.chamadas();
    assert_eq!(c.len(), 1);
    assert_eq!(c[0][2], "w2:p1");
}

#[test]
fn nao_toca_sem_mensagem_em_registrado() {
    let (_d, srv, h) = amb_campainha(50, 600);
    envia_ok(&srv.socket, "nr-1", "codex-web", "corpo");
    ack_ok(&srv.socket, "nr-1", "codex-web", "aceito");
    h.estado("idle");
    pausa(800);
    assert!(
        h.chamadas().is_empty(),
        "sem nada em registrado nao ha campainha"
    );
}

// ---------- texto ----------

#[test]
fn texto_nao_contem_remetente_assunto_nem_corpo() {
    let (_d, srv, h) = amb_campainha(50, 600);
    let p = msg(
        "sec-1",
        "remetente-secreto",
        "codex-web",
        "PEDIDO",
        "ASSUNTO-SECRETO-9",
        "CORPO-SECRETO-9",
    );
    let r = res(&envia(&srv.socket, p));
    assert_eq!(r["state"], "registrado");
    h.estado("idle");
    assert!(h.espera_chamadas(1, Duration::from_secs(5)), "nao tocou");
    for c in h.chamadas() {
        for arg in &c {
            assert!(
                !arg.contains("SECRETO") && !arg.contains("secreto"),
                "vazou conteudo: {arg}"
            );
        }
    }
    assert_eq!(h.textos()[0], TEXTO_1);
}

#[test]
fn texto_conta_as_mensagens_pendentes() {
    let (_d, srv, h) = amb_campainha(50, 600);
    h.estado("working");
    envia_ok(&srv.socket, "tc-1", "codex-web", "a");
    envia_ok(&srv.socket, "tc-2", "codex-web", "b");
    h.estado("idle");
    assert!(h.espera_chamadas(1, Duration::from_secs(5)), "nao tocou");
    assert_eq!(
        h.textos()[0],
        "[chat] 2 pendente(s). Rode: chatd inbox --take"
    );
}

// ---------- uma por rearm ----------

#[test]
fn uma_campainha_por_rearm() {
    let (_d, srv, h, (tc, tx)) = amb_campainha_auth(50, 600);
    h.estado("idle");
    envia_claude(&srv.socket, &tc, "ra-1", CODEX, "a");
    assert!(h.espera_chamadas(1, Duration::from_secs(5)), "nao tocou");
    envia_claude(&srv.socket, &tc, "ra-2", CODEX, "b");
    pausa(1000);
    assert_eq!(
        h.chamadas().len(),
        1,
        "sem rearm nao pode haver segunda campainha"
    );
    assert_eq!(bell_de(&srv.socket, CODEX, &tx), "outstanding");
}

#[test]
fn rearm_curto_permite_nova_campainha_sem_recibo() {
    let (_d, srv, h) = amb_campainha(50, 1);
    h.estado("idle");
    envia_ok(&srv.socket, "rc-1", "codex-web", "a");
    assert!(
        h.espera_chamadas(1, Duration::from_secs(5)),
        "nao tocou a primeira vez"
    );
    assert!(
        h.espera_chamadas(2, Duration::from_secs(5)),
        "com bell_rearm_s 1 e mensagem ainda registrada, deveria tocar de novo"
    );
}

#[test]
fn aceite_volta_a_idle_wait_e_nova_mensagem_toca() {
    let (_d, srv, h, (tc, tx)) = amb_campainha_auth(50, 600);
    h.estado("idle");
    envia_claude(&srv.socket, &tc, "av-1", CODEX, "a");
    assert!(h.espera_chamadas(1, Duration::from_secs(5)), "nao tocou");
    assert!(
        espera(Duration::from_secs(3), || bell_de(&srv.socket, CODEX, &tx)
            == "outstanding"),
        "esperava outstanding"
    );
    ack_como(&srv.socket, CODEX, &tx, "av-1", "aceito");
    assert!(
        espera(Duration::from_secs(3), || bell_de(&srv.socket, CODEX, &tx)
            == "idle_wait"),
        "aceite deve voltar a idle_wait"
    );
    envia_claude(&srv.socket, &tc, "av-2", CODEX, "b");
    assert!(
        h.espera_chamadas(2, Duration::from_secs(5)),
        "nova mensagem depois do aceite deveria tocar"
    );
}

#[test]
fn resultado_direto_de_registrado_tambem_volta_a_idle_wait() {
    let (_d, srv, h, (tc, tx)) = amb_campainha_auth(50, 600);
    h.estado("idle");
    envia_claude(&srv.socket, &tc, "rd-1", CODEX, "a");
    assert!(h.espera_chamadas(1, Duration::from_secs(5)), "nao tocou");
    assert!(
        espera(Duration::from_secs(3), || bell_de(&srv.socket, CODEX, &tx)
            == "outstanding"),
        "esperava outstanding"
    );
    ack_como(&srv.socket, CODEX, &tx, "rd-1", "resultado");
    assert!(
        espera(Duration::from_secs(3), || bell_de(&srv.socket, CODEX, &tx)
            == "idle_wait"),
        "quando todas saem de registrado, volta a idle_wait"
    );
}

#[test]
fn estado_inicial_da_campainha() {
    let (_d, srv, _h, (tc, tx)) = amb_campainha_auth(50, 600);
    assert_eq!(bell_de(&srv.socket, CODEX, &tx), "idle_wait");
    assert_eq!(bell_de(&srv.socket, CLAUDE, &tc), "none");
}

// ---------- eventos e status ----------

#[test]
fn bell_sent_aparece_no_stream_e_last_bell_seq_confere() {
    let (_d, srv, h, (tc, tx)) = amb_campainha_auth(50, 600);
    // bell_sent so e visivel ao destinatario: o stream e lido como codex-web
    let mut c = conexao_auth(&srv.socket, CODEX, &tx);
    res(&c.chama("events.resume", json!({"v": 1, "last_seq": 0})));
    h.estado("idle");
    envia_claude(&srv.socket, &tc, "bs-1", CODEX, "a");
    let evs = c.eventos_ate(Duration::from_secs(5), |e| e["type"] == "bell_sent");
    let ev = evs.last().expect("evento bell_sent");
    assert_eq!(ev["type"], "bell_sent");
    let st = caixa(&srv.socket, CODEX, &tx);
    assert_eq!(st["last_bell_seq"], ev["seq"]);
}

#[test]
fn falha_do_prompt_vira_bell_failed_e_o_recibo_fica_registrado() {
    let (_d, srv, h) = amb_campainha(50, 600);
    h.flag("falha.flag", true);
    let mut c = Conexao::abre(&srv.socket);
    res(&c.chama("events.resume", json!({"v": 1, "last_seq": 0})));
    h.estado("idle");
    let r = res(&envia(
        &srv.socket,
        msg("bf-1", "claude-main", "codex-web", "AVISO", "s", "b"),
    ));
    assert_eq!(
        r["state"], "registrado",
        "recibo registrado nao depende da campainha"
    );
    let evs = c.eventos_ate(Duration::from_secs(5), |e| e["type"] == "bell_failed");
    let ev = evs.last().expect("evento bell_failed");
    assert_eq!(ev["recipient"], "codex-web");
    let reason = ev["reason"].as_str().expect("reason string");
    // AMBIGUO: a spec pede `reason` curto, sem saida de comando; o limite exato nao esta escrito.
    assert!(
        reason.len() <= 200 && !reason.contains('\n'),
        "reason nao curto: {reason:?}"
    );
    let ms = lista(&srv.socket, "codex-web");
    assert_eq!(ms[0]["state"], "registrado");
    assert!(
        !h.chamadas().is_empty(),
        "a tentativa de prompt deveria ter sido feita"
    );
}

#[test]
fn falha_do_get_registra_bell_failed_no_maximo_um_por_minuto() {
    let (_d, srv, h) = amb_campainha(50, 600);
    h.flag("get_falha.flag", true);
    let mut c = Conexao::abre(&srv.socket);
    res(&c.chama("events.resume", json!({"v": 1, "last_seq": 0})));
    envia_ok(&srv.socket, "gf-1", "codex-web", "a");
    h.estado("idle");
    let primeiro = c.eventos_ate(Duration::from_secs(5), |e| e["type"] == "bell_failed");
    assert_eq!(
        conta_bell_failed(&primeiro),
        1,
        "primeiro bell_failed: {primeiro:?}"
    );
    // janela de 2,5 s com erro de get persistente: nenhum novo bell_failed (limite de 1 por minuto)
    let depois = c.eventos_ate(Duration::from_millis(2500), |_| false);
    assert_eq!(
        conta_bell_failed(&depois),
        0,
        "bell_failed repetido antes de 1 minuto"
    );
    assert!(
        h.chamadas().is_empty(),
        "com get falhando nao pode haver prompt"
    );
}

#[test]
fn herdr_travado_nao_toca_e_status_responde() {
    // AMBIGUO: a spec manda tratar tempo esgotado (5 s) como nao tocar; a exigencia de o servidor seguir atendendo durante a espera nao esta escrita. Testamos o segundo ponto com prazo folgado.
    let (_d, srv, h) = amb_campainha(50, 600);
    h.flag("trava.flag", true);
    h.estado("idle");
    envia_ok(&srv.socket, "tv-1", "codex-web", "a");
    pausa(1500);
    assert!(h.chamadas().is_empty(), "herdr travado nao pode tocar");
    let t = Instant::now();
    let _ = status(&srv.socket);
    assert!(
        t.elapsed() < Duration::from_secs(3),
        "status demorou com herdr travado: {:?}",
        t.elapsed()
    );
    h.flag("trava.flag", false);
}

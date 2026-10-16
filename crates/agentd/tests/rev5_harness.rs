//! Auto-testes do harness rev5 (`harness_*`). Exercitam o tmux FALSO diretamente, sem o daemon, e
//! devem passar hoje: se falharem, o problema e do harness, nao do agentd.

mod rev5_common;

use rev5_common::*;
use std::io::{BufRead, BufReader, Write};
use std::process::{Command, Stdio};
use std::time::Duration;

fn roda_tmux(m: &Mundo, args: &[&str]) -> (Option<i32>, String, String) {
    let o = Command::new(&m.tmux)
        .args(args)
        .stdin(Stdio::null())
        .output()
        .expect("executar wrapper do tmux falso");
    (
        o.status.code(),
        String::from_utf8_lossy(&o.stdout).into_owned(),
        String::from_utf8_lossy(&o.stderr).into_owned(),
    )
}

#[test]
fn harness_list_panes_expande_formato_e_variavel_desconhecida_vira_vazio() {
    let mut m = Mundo::novo();
    m.paineis(&[
        Painel::new("%1", "claude").tag("claude").titulo("T1"),
        Painel::new("%2", "zsh").sessao("$2"),
    ]);
    let (rc, out, _) = roda_tmux(
        &m,
        &[
            "list-panes",
            "-a",
            "-F",
            "#{pane_id}|#{pane_pid}|#{nao_existe}|#{@agentd}|#{pane_title}",
        ],
    );
    assert_eq!(rc, Some(0));
    assert_eq!(out, "%1|4242||claude|T1\n%2|4242|||\n");
    assert!(m.violacoes().is_empty());
}

#[test]
fn harness_list_panes_com_socket_antes_e_registrado() {
    let mut m = Mundo::novo();
    m.paineis(&[Painel::new("%1", "claude")]);
    let (rc, out, _) = roda_tmux(
        &m,
        &["-S", "/tmp/x.sock", "list-panes", "-a", "-F", "#{pane_id}"],
    );
    assert_eq!(rc, Some(0));
    assert_eq!(out, "%1\n");
    assert_eq!(
        m.chamadas(),
        vec![vec![
            "-S".to_string(),
            "/tmp/x.sock".to_string(),
            "list-panes".to_string(),
            "-a".to_string(),
            "-F".to_string(),
            "#{pane_id}".to_string()
        ]]
    );
}

#[test]
fn harness_capture_pane_devolve_bytes_crus_e_falha_em_pane_desconhecido() {
    let mut m = Mundo::novo();
    m.paineis(&[Painel::new("%1", "claude")]);
    let tela = b"\x1b[2m\xe2\x9d\xaf rascunho\x1b[22m\n  linha 2\n".to_vec();
    m.tela("%1", &tela);
    let (rc, out, _) = roda_tmux(&m, &["capture-pane", "-p", "-e", "-J", "-t", "%1"]);
    assert_eq!(rc, Some(0));
    assert_eq!(out.as_bytes(), tela.as_slice());
    let (rc2, _, err2) = roda_tmux(&m, &["capture-pane", "-p", "-e", "-J", "-t", "%9"]);
    assert_eq!(rc2, Some(1));
    assert!(err2.contains("can't find pane"));
    assert_eq!(m.capturas(), vec!["%1".to_string(), "%9".to_string()]);
}

#[test]
fn harness_subcomando_fora_da_lista_e_violacao_e_sai_2() {
    let m = Mundo::novo();
    let (rc, _, _) = roda_tmux(&m, &["send-keys", "-t", "%1", "x"]);
    assert_eq!(rc, Some(2));
    let (rc2, _, _) = roda_tmux(&m, &["list-panes", "-a", "-F", "#{pane_id}", "-r"]);
    assert_eq!(rc2, Some(2));
    assert_eq!(m.violacoes().len(), 2);
    assert_eq!(m.violacoes()[0][0], "send-keys");
}

#[test]
fn harness_attach_registra_argv_conta_stdin_e_sai_com_linha_de_exit() {
    let m = Mundo::novo();
    let mut child = Command::new(&m.tmux)
        .args(["-C", "attach", "-f", "ignore-size", "-t", "$1"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("anexo");
    assert!(espera(Duration::from_secs(5), || m.anexos().len() == 1));
    std::thread::sleep(Duration::from_millis(150));
    m.feed("$1", "%output %7 oi");
    let mut out = child.stdout.take().unwrap();
    let mut leitor = BufReader::new(&mut out);
    let mut linha = String::new();
    leitor.read_line(&mut linha).unwrap();
    assert_eq!(linha, "%output %7 oi\n");
    let mut stdin = child.stdin.take().unwrap();
    stdin.write_all(b"12345").unwrap();
    stdin.flush().unwrap();
    assert!(espera(Duration::from_secs(5), || m.bytes_stdin() == 5));
    m.feed("$1", "%exit");
    let st = child.wait().unwrap();
    assert_eq!(st.code(), Some(0));
    assert_eq!(
        m.anexos()[0],
        vec!["-C", "attach", "-f", "ignore-size", "-t", "$1"]
    );
}

#[test]
fn harness_attach_com_argv_fora_da_forma_exata_e_violacao() {
    let m = Mundo::novo();
    let (rc, _, _) = roda_tmux(&m, &["-C", "-r", "attach", "-t", "$1"]);
    assert_eq!(rc, Some(2));
    assert_eq!(m.violacoes().len(), 1);
}

#[test]
fn harness_duas_leituras_simultaneas_sao_marcadas_como_sobreposicao() {
    let mut m = Mundo::novo();
    m.paineis(&[Painel::new("%1", "claude")]);
    m.listar_com("ok", json_delay(300));
    let bin = m.tmux.clone();
    let h: Vec<_> = (0..2)
        .map(|_| {
            let b = bin.clone();
            std::thread::spawn(move || {
                Command::new(b)
                    .args(["list-panes", "-a", "-F", "#{pane_id}"])
                    .output()
                    .unwrap()
                    .status
                    .code()
            })
        })
        .collect();
    for x in h {
        assert_eq!(x.join().unwrap(), Some(0));
    }
    assert!(m.sobreposicoes() >= 1, "a trava nao detectou concorrencia");
}

fn json_delay(ms: u64) -> serde_json::Value {
    serde_json::json!({"delay_ms": ms})
}

#[test]
fn harness_modos_de_falha_do_list_panes() {
    let mut m = Mundo::novo();
    m.paineis(&[Painel::new("%1", "claude")]);
    m.listar("sem_servidor");
    let (rc, out, err) = roda_tmux(&m, &["list-panes", "-a", "-F", "#{pane_id}"]);
    assert_eq!((rc, out.as_str()), (Some(1), ""));
    assert!(err.contains("no server running"));
    m.listar_com("texto", serde_json::json!({"texto": "linha ruim\n"}));
    let (rc2, out2, _) = roda_tmux(&m, &["list-panes", "-a", "-F", "#{pane_id}"]);
    assert_eq!((rc2, out2.as_str()), (Some(0), "linha ruim\n"));
}

#[test]
fn harness_sentinela_de_stderr_aparece_em_toda_chamada() {
    let mut m = Mundo::novo();
    m.paineis(&[Painel::new("%1", "claude")]);
    m.ajusta("stderr", serde_json::json!("SENTINELA-STDERR-X"));
    let (_, _, err) = roda_tmux(&m, &["list-panes", "-a", "-F", "#{pane_id}"]);
    assert!(err.contains("SENTINELA-STDERR-X"));
}

#[test]
fn harness_wrapper_em_caminho_hostil_nao_executa_nada() {
    let marca = tempfile::tempdir().unwrap();
    let nome = format!(
        "pasta ;touch {0}/PWN;$(touch {0}/PWN2) \"q\"/tmux",
        marca.path().display()
    );
    let mut m = Mundo::novo_em(&nome);
    m.paineis(&[Painel::new("%1", "claude")]);
    let (rc, out, _) = roda_tmux(&m, &["list-panes", "-a", "-F", "#{pane_id}"]);
    assert_eq!(rc, Some(0));
    assert_eq!(out, "%1\n");
    assert!(!marca.path().join("PWN").exists());
    assert!(!marca.path().join("PWN2").exists());
}

#[test]
fn harness_fixtures_e_goldens_existem() {
    let gold = goldens();
    assert!(gold.len() >= 38, "golden incompleto: {}", gold.len());
    for g in &gold {
        let fx = g["fixture"].as_str().expect("fixture");
        let p = fixtures().join(format!("{fx}.screen"));
        assert!(p.exists(), "fixture ausente: {fx}");
        assert!(g["expect"]["state"].is_string(), "golden sem estado: {fx}");
    }
    assert!(regra_padrao("claude.idle.box")["state"] == "idle");
}

#[test]
fn harness_config_base_tem_o_contrato_de_tmux() {
    let m = Mundo::novo();
    let c = cfg(&m, &regras_padrao(), 200, &[("claude", "claude", "claude")]);
    assert_eq!(c["agents"]["claude"]["sources"][0]["type"], "tmux");
    assert_eq!(c["agents"]["claude"]["sources"][0]["tag"], "claude");
    assert_eq!(c["tmux_bin"], s(&m.tmux));
    assert!(c.get("herdr_bin").is_none());
}

#[test]
fn harness_config_gerada_respeita_os_limites_da_secao_7() {
    // Spec secao 7: se o harness gerar uma config fora dos limites, o daemon recusa e o teste deixa de
    // exercitar o que queria (ex.: eventos). Este auto-teste pega isso antes.
    let m = Mundo::novo();
    let regras = regras_padrao();
    for poll in [200u64, 1000, 2000, 10000] {
        let c = cfg(&m, &regras, poll, &agentes_todos());
        let p = c["poll_ms"].as_u64().unwrap();
        let t = c["source_timeout_ms"].as_u64().unwrap();
        let stale = c["stale_after_ms"].as_u64().unwrap();
        let safety = c["safety_poll_ms"].as_u64().unwrap();
        assert!(
            (200..=10000).contains(&p),
            "poll_ms fora de 200..10000: {p}"
        );
        assert!(
            (200..=30000).contains(&t),
            "source_timeout_ms fora de 200..30000: {t}"
        );
        assert!(
            stale > p + t,
            "stale_after_ms {stale} deve ser > poll {p} + timeout {t}"
        );
        assert!(stale <= 300000, "stale_after_ms acima de 300000: {stale}");
        assert!(
            (1000..=60000).contains(&safety),
            "safety_poll_ms fora de 1000..60000: {safety}"
        );
        assert!(
            safety >= p,
            "safety_poll_ms {safety} deve ser >= poll_ms {p}"
        );
        let g = c["gone_after_s"].as_u64().unwrap();
        assert!(
            (5..=86400).contains(&g),
            "gone_after_s fora de 5..86400: {g}"
        );
        let conf = c["idle_confirm_polls"].as_u64().unwrap();
        assert!(
            (1..=5).contains(&conf),
            "idle_confirm_polls fora de 1..5: {conf}"
        );
        let ms = c["idle_confirm_ms"].as_u64().unwrap();
        assert!(ms <= 30000, "idle_confirm_ms acima de 30000: {ms}");
        let ring = c["ring"].as_u64().unwrap();
        assert!(
            (64..=65536).contains(&ring),
            "ring fora de 64..65536: {ring}"
        );
        let deb = c["event_debounce_ms"].as_u64().unwrap();
        assert!(
            (50..=1000).contains(&deb),
            "event_debounce_ms fora de 50..1000: {deb}"
        );
        let ags = c["agents"].as_object().unwrap();
        assert!(ags.len() <= 32, "mais de 32 agentes");
        for (id, a) in ags {
            assert!(
                id.len() <= 32
                    && id.chars().all(|ch| ch.is_ascii_lowercase()
                        || ch.is_ascii_digit()
                        || ch == '-'
                        || ch == '_'),
                "id fora de [a-z0-9_-]{{1,32}}: {id}"
            );
            let tag = a["sources"][0]["tag"].as_str().unwrap();
            assert!(
                tag.len() <= 32
                    && tag.chars().all(|ch| ch.is_ascii_lowercase()
                        || ch.is_ascii_digit()
                        || ch == '-'
                        || ch == '_'),
                "tag fora de [a-z0-9_-]{{1,32}}: {tag}"
            );
            assert_eq!(
                a["sources"].as_array().unwrap().len(),
                1,
                "sources deve ter 1 elemento"
            );
        }
    }
}

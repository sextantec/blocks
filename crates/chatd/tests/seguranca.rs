//! Seguranca e limites (spec secao 9): permissoes, nomes, corpo nunca interpretado, conexao ociosa.
mod common;

use common::*;
use serde_json::json;
use std::collections::BTreeSet;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::time::{Duration, Instant};

fn modo(p: &Path) -> u32 {
    fs::metadata(p).expect("metadata").permissions().mode() & 0o777
}

#[test]
fn diretorio_0700_socket_0600_log_0600_e_trava_presente() {
    let (_d, srv) = sobe_padrao();
    envia_ok(&srv.socket, "sg-1", "codex-web", "x");
    assert_eq!(modo(&srv.state), 0o700, "diretorio de estado");
    assert_eq!(modo(&srv.socket), 0o600, "socket");
    assert_eq!(modo(&srv.state.join("log.jsonl")), 0o600, "log");
    assert!(
        srv.state.join("chatd.lock").exists(),
        "chatd.lock deve existir em DIR"
    );
}

#[test]
fn log_continua_0600_depois_do_reinicio() {
    let d = tmp();
    let cfg = config_padrao(d.path());
    let st = d.path().join("state");
    let mut srv = sobe(&st, &cfg);
    envia_ok(&srv.socket, "lr-1", "codex-web", "x");
    assert!(srv.para().is_some());
    drop(srv);
    let srv2 = sobe(&st, &cfg);
    envia_ok(&srv2.socket, "lr-2", "codex-web", "y");
    assert_eq!(modo(&st.join("log.jsonl")), 0o600);
    assert_eq!(modo(&st), 0o700);
}

#[test]
fn diretorio_preexistente_com_0755_e_corrigido_ou_recusado() {
    // AMBIGUO: a spec diz "0700 (recusar subir se nao consegue)"; nao diz se corrige um diretorio ja existente com permissao aberta. Aceitamos subir com 0700 ou recusar.
    let d = tmp();
    let st = d.path().join("state");
    fs::create_dir(&st).unwrap();
    fs::set_permissions(&st, fs::Permissions::from_mode(0o755)).unwrap();
    let cfg = config_padrao(d.path());
    match tenta(
        &["serve", "--state-dir", s(&st), "--config", s(&cfg)],
        &[],
        &st,
        &st.join("chatd.sock"),
    ) {
        Ok(_srv) => assert_eq!(modo(&st), 0o700, "subiu com diretorio aberto"),
        Err(e) => assert_eq!(e.code, Some(1), "saida: {e:?}"),
    }
}

#[test]
fn nome_com_travessia_e_recusado_sem_criar_arquivo() {
    let (d, srv) = sobe_padrao();
    let antes: BTreeSet<String> = fs::read_dir(&srv.state)
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    let r = envia(
        &srv.socket,
        msg("tr-1", "../escapou", "codex-web", "AVISO", "s", "b"),
    );
    assert_eq!(cod(&r), "invalid_envelope");
    let r = envia(
        &srv.socket,
        msg("tr-2", "codex-web/../../x", "codex-web", "AVISO", "s", "b"),
    );
    assert_eq!(cod(&r), "invalid_envelope");
    let depois: BTreeSet<String> = fs::read_dir(&srv.state)
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    assert_eq!(antes, depois, "nome invalido nao pode criar arquivo");
    let raiz: BTreeSet<String> = fs::read_dir(d.path())
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    assert_eq!(
        raiz,
        ["config.json", "state"]
            .iter()
            .map(|s| s.to_string())
            .collect()
    );
}

#[test]
fn corpo_com_substituicao_de_comando_nao_e_executado() {
    let (d, srv, h) = amb_campainha(50, 600);
    let corpo = "$(touch PWNED_A) `touch PWNED_B`; touch PWNED_C | sh -c 'touch PWNED_D'";
    envia_ok(&srv.socket, "inj-1", "codex-web", corpo);
    h.estado("idle");
    assert!(h.espera_chamadas(1, Duration::from_secs(5)), "nao tocou");
    assert!(
        h.textos().iter().all(|t| t.starts_with("[chat] ")),
        "texto da campainha deve ser o fixo"
    );
    for nome in ["PWNED_A", "PWNED_B", "PWNED_C", "PWNED_D"] {
        assert!(
            !d.path().join(nome).exists(),
            "{nome} criado no diretorio temporario"
        );
        assert!(!srv.state.join(nome).exists(), "{nome} criado no estado");
        assert!(
            !Path::new(env!("CARGO_MANIFEST_DIR")).join(nome).exists(),
            "{nome} criado no pacote"
        );
    }
    // o corpo continua guardado literalmente
    assert_eq!(lista(&srv.socket, "codex-web")[0]["body"], corpo);
}

#[test]
fn conexao_ociosa_por_30s_e_fechada() {
    let (_d, srv) = sobe_padrao();
    let mut c = Conexao::abre(&srv.socket);
    let t = Instant::now();
    assert!(
        c.fechada_em(Duration::from_secs(40)),
        "conexao ociosa nao foi fechada em 40 s"
    );
    assert!(
        t.elapsed() >= Duration::from_secs(25),
        "conexao ociosa fechada cedo demais: {:?}",
        t.elapsed()
    );
}

#[test]
fn conexao_com_requisicao_nao_e_fechada_antes_de_30s() {
    let (_d, srv) = sobe_padrao();
    let mut c = Conexao::abre(&srv.socket);
    for _ in 0..3 {
        res(&c.chama("status", json!({"v": 1})));
        std::thread::sleep(Duration::from_secs(2));
    }
    res(&c.chama("status", json!({"v": 1})));
}

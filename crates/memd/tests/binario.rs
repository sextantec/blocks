//! Testes do tipo B: exigem o binario (`env!("CARGO_BIN_EXE_memd")`), diretorio temporario,
//! socket Unix real, SIGHUP real ou arquivos reais. Cobrem O1 (init), O2 (validate), O3 (token
//! fora de logs e saidas), O6 (modos), O7 (recusa de partida), A5 (SIGHUP em conexao aberta),
//! A8 (nenhuma operacao altera grants.json), M1 (32 clientes simultaneos pelo socket) e
//! o token-hash. Sem rede. Nao usa ~/.config, ~/.local, ~/.claude, ~/.codex.

mod common;

use common::*;
use serde_json::{Value, json};
use std::fs;
use std::io::{BufRead, BufReader, Read, Write};
use std::os::unix::fs::PermissionsExt;
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, ExitStatus, Stdio};
use std::sync::{Arc, Barrier, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

const ESPERA: Duration = Duration::from_secs(10);

fn memd() -> Command {
    Command::new(env!("CARGO_BIN_EXE_memd"))
}

fn modo(p: &Path) -> u32 {
    fs::metadata(p).unwrap().permissions().mode() & 0o777
}

fn fixa_modo(p: &Path, m: u32) {
    fs::set_permissions(p, fs::Permissions::from_mode(m)).unwrap();
}

/// Diretorio de teste: estado (0700), config.json, grants.json (0600) e socket em caminho curto.
struct Estado {
    _t: tempfile::TempDir,
    estado: PathBuf,
    config: PathBuf,
    grants: PathBuf,
    sock: PathBuf,
}

fn prepara(grants_json: &str, cfg_json: &str) -> Estado {
    let t = tempfile::tempdir().unwrap();
    let estado = t.path().join("estado");
    fs::create_dir(&estado).unwrap();
    fixa_modo(&estado, 0o700);
    let config = t.path().join("config.json");
    fs::write(&config, cfg_json).unwrap();
    let grants = t.path().join("grants.json");
    fs::write(&grants, grants_json).unwrap();
    fixa_modo(&grants, 0o600);
    let sock = t.path().join("m.sock");
    Estado {
        _t: t,
        estado,
        config,
        grants,
        sock,
    }
}

/// Servidor `memd serve` em processo filho; a saida e coletada por threads e morto no Drop.
struct Servidor {
    filho: Child,
    saida: Arc<Mutex<Vec<u8>>>,
    leitores: Vec<JoinHandle<()>>,
}

impl Servidor {
    fn pid(&self) -> i32 {
        self.filho.id() as i32
    }

    /// Mata o processo e devolve stdout+stderr capturados.
    fn encerra(&mut self) -> Vec<u8> {
        let _ = self.filho.kill();
        let _ = self.filho.wait();
        for h in self.leitores.drain(..) {
            h.join().unwrap();
        }
        self.saida.lock().unwrap().clone()
    }
}

impl Drop for Servidor {
    fn drop(&mut self) {
        let _ = self.filho.kill();
        let _ = self.filho.wait();
    }
}

fn captura<R: Read + Send + 'static>(mut r: R, saida: Arc<Mutex<Vec<u8>>>) -> JoinHandle<()> {
    thread::spawn(move || {
        let mut buf = Vec::new();
        let _ = r.read_to_end(&mut buf);
        saida.lock().unwrap().extend_from_slice(&buf);
    })
}

/// Sobe `memd serve` com o socket dado (ou o padrao memd.sock no diretorio de estado).
fn sobe_com(e: &Estado, sock: Option<&Path>) -> Servidor {
    let alvo: PathBuf = match sock {
        Some(s) => s.to_path_buf(),
        None => e.estado.join("memd.sock"),
    };
    let mut cmd = memd();
    cmd.arg("serve")
        .arg("--state-dir")
        .arg(&e.estado)
        .arg("--config")
        .arg(&e.config)
        .arg("--grants")
        .arg(&e.grants);
    if let Some(s) = sock {
        cmd.arg("--socket").arg(s);
    }
    cmd.stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut filho = cmd.spawn().expect("spawn memd serve");
    let saida = Arc::new(Mutex::new(Vec::new()));
    let leitores = vec![
        captura(filho.stdout.take().unwrap(), Arc::clone(&saida)),
        captura(filho.stderr.take().unwrap(), Arc::clone(&saida)),
    ];
    let mut srv = Servidor {
        filho,
        saida,
        leitores,
    };
    let t0 = Instant::now();
    loop {
        if let Some(st) = srv.filho.try_wait().unwrap() {
            let s = srv.encerra();
            panic!(
                "memd serve saiu cedo ({st:?}): {}",
                String::from_utf8_lossy(&s)
            );
        }
        if UnixStream::connect(&alvo).is_ok() {
            return srv;
        }
        assert!(t0.elapsed() < ESPERA, "socket nao apareceu em {alvo:?}");
        thread::sleep(Duration::from_millis(20));
    }
}

fn sobe(e: &Estado) -> Servidor {
    sobe_com(e, Some(&e.sock))
}

/// Conexao com o socket: escrita e leitura de linhas.
struct Conexao {
    escreve: UnixStream,
    le: BufReader<UnixStream>,
}

fn conecta(sock: &Path) -> Conexao {
    let s = UnixStream::connect(sock).expect("connect ao socket");
    s.set_read_timeout(Some(ESPERA)).unwrap();
    let l = s.try_clone().unwrap();
    Conexao {
        escreve: s,
        le: BufReader::new(l),
    }
}

fn envia(c: &mut Conexao, linha: &str) -> Value {
    c.escreve.write_all(linha.as_bytes()).unwrap();
    c.escreve.write_all(b"\n").unwrap();
    let mut r = String::new();
    c.le.read_line(&mut r).expect("resposta do socket");
    assert!(!r.is_empty(), "conexao fechada pelo servidor");
    parse(r.trim_end_matches('\n'))
}

/// Uma requisicao em conexao nova.
fn uma(sock: &Path, token: &str, metodo: &str, params: &Value) -> Value {
    let mut c = conecta(sock);
    envia(&mut c, &linha_req("c", token, metodo, params))
}

/// Mutacao em conexao nova, com `request_id` novo.
fn muta_sock(sock: &Path, token: &str, metodo: &str, mut params: Value) -> Value {
    params["request_id"] = json!(rid());
    uma(sock, token, metodo, &params)
}

fn todos_os_perfis_a() -> String {
    grants(&[perfil_total("agente-a", TOK_A)])
}

fn cfg_padrao() -> String {
    cfg_json_com(&[])
}

// ------------------------------------------------------------------ O1 init

#[test]
fn o01_init_cria_modelo_sem_tokens_e_e_idempotente_sem_sobrescrever() {
    let t = tempfile::tempdir().unwrap();
    let dir = t.path().join("novo");
    let out = memd().arg("init").arg(&dir).output().unwrap();
    assert!(
        out.status.success(),
        "init: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert_eq!(modo(&dir), 0o700, "diretorio de estado 0700");

    let config = dir.join("config.json");
    let grants_p = dir.join("grants.json");
    let leia = dir.join("LEIA-ME.md");
    assert!(
        config.is_file() && grants_p.is_file() && leia.is_file(),
        "arquivos criados"
    );

    // modelo sem tokens
    let g = fs::read_to_string(&grants_p).unwrap();
    assert!(!g.contains("mt_"), "grants.json de modelo nao tem token");
    memd::Grants::from_json(&g).expect("grants de modelo e valido");
    memd::Config::from_json(&fs::read_to_string(&config).unwrap())
        .expect("config de modelo e valida");
    let tudo_out = [out.stdout.clone(), out.stderr.clone()].concat();
    assert!(!String::from_utf8_lossy(&tudo_out).contains("mt_"));

    // idempotente: segunda execucao nao muda os bytes
    let antes = (
        fs::read(&config).unwrap(),
        fs::read(&grants_p).unwrap(),
        fs::read(&leia).unwrap(),
    );
    let out2 = memd().arg("init").arg(&dir).output().unwrap();
    assert!(out2.status.success(), "init repetido deve passar");
    let depois = (
        fs::read(&config).unwrap(),
        fs::read(&grants_p).unwrap(),
        fs::read(&leia).unwrap(),
    );
    assert_eq!(antes, depois);

    // nunca sobrescreve: configuracao do dono e preservada
    fs::write(&config, r#"{"minha":"configuracao"}"#).unwrap();
    let out3 = memd().arg("init").arg(&dir).output().unwrap();
    assert!(out3.status.success());
    assert_eq!(
        fs::read_to_string(&config).unwrap(),
        r#"{"minha":"configuracao"}"#
    );
}

// ------------------------------------------------------------------ O2 validate

/// Log real (FsStorage + SystemClock) com D-001 active (seq 1,2) e D-002 proposta com supersedes.
/// Devolve o caminho do diretorio de estado.
fn cria_log_real(dir: &Path, supersedes_d2: bool) {
    let (log, replay) = blocklog::Log::open_dir(dir, blocklog::Options::default()).unwrap();
    let grants = memd::Grants::from_json(&todos_os_perfis_a()).unwrap();
    let cfg = memd::Config::from_json(&cfg_padrao()).unwrap();
    let mut s =
        memd::Service::new(cfg, grants, log, replay, Box::new(blocklog::SystemClock)).unwrap();
    propoe_e_promove(&mut s, TOK_A, decisao("Primeira", "agentd"));
    let mut d2 = decisao("Segunda", "agentd");
    if supersedes_d2 {
        d2["supersedes"] = json!(["D-001"]);
    }
    propoe(&mut s, TOK_A, d2);
}

fn valida(dir: &Path) -> std::process::Output {
    memd()
        .arg("validate")
        .arg("--state-dir")
        .arg(dir)
        .output()
        .unwrap()
}

/// Reescreve uma linha do log.jsonl aplicando `f` ao primeiro elemento de `tx`.
fn edita_tx(dir: &Path, indice_linha: usize, f: impl FnOnce(&mut Value)) {
    let caminho = dir.join("log.jsonl");
    let bytes = fs::read(&caminho).unwrap();
    let mut linhas = linhas_do(&bytes);
    let mut v: Value = serde_json::from_slice(&linhas[indice_linha]).unwrap();
    f(&mut v["tx"][0]);
    linhas[indice_linha] = serde_json::to_vec(&v).unwrap();
    fs::write(&caminho, monta(&linhas)).unwrap();
}

#[test]
fn o02_validate_aceita_log_valido_e_recusa_lacuna_e_supersedes_quebrado() {
    // controle: log real valido
    let t = tempfile::tempdir().unwrap();
    let dir = t.path().join("ok");
    fs::create_dir(&dir).unwrap();
    fixa_modo(&dir, 0o700);
    cria_log_real(&dir, false);
    let out = valida(&dir);
    assert!(
        out.status.success(),
        "log valido: {}",
        String::from_utf8_lossy(&out.stderr)
    );

    // lacuna de id: a proposta D-002 (linha 3) vira D-003, sobrando um buraco em D
    let t2 = tempfile::tempdir().unwrap();
    let dir2 = t2.path().join("lacuna");
    fs::create_dir(&dir2).unwrap();
    fixa_modo(&dir2, 0o700);
    cria_log_real(&dir2, false);
    // linhas: 1 propose D-001, 2 promote D-001, 3 propose D-002
    edita_tx(&dir2, 2, |ev| {
        assert_eq!(
            ev["id"],
            json!("D-002"),
            "esquema do evento propose mudou: {ev}"
        );
        ev["id"] = json!("D-003");
    });
    assert!(
        !valida(&dir2).status.success(),
        "lacuna de id deve ser recusada"
    );

    // supersedes quebrado: proposta com supersedes ["D-001"] trocada por ["D-099"]
    let t3 = tempfile::tempdir().unwrap();
    let dir3 = t3.path().join("sup");
    fs::create_dir(&dir3).unwrap();
    fixa_modo(&dir3, 0o700);
    cria_log_real(&dir3, true);
    edita_tx(&dir3, 2, |ev| {
        assert_eq!(
            ev["supersedes"],
            json!(["D-001"]),
            "esquema do evento propose mudou: {ev}"
        );
        ev["supersedes"] = json!(["D-099"]);
    });
    assert!(
        !valida(&dir3).status.success(),
        "supersedes quebrado deve ser recusado"
    );
}

// ------------------------------------------------------------------ O3 token e token-hash

#[test]
fn o03_token_nao_aparece_em_logs_respostas_status_whoami_nem_erros() {
    let perfis = grants(&[
        perfil_total("agente-a", TOK_A),
        perfil_total("agente-sentinela", TOK_SENTINELA),
    ]);
    let e = prepara(&perfis, &cfg_padrao());
    let mut srv = sobe(&e);
    let sentinela = TOK_SENTINELA;
    let corpo_sentinela = &TOK_SENTINELA[3..];

    let mut respostas = Vec::new();
    // controle: o token funciona
    let w = uma(&e.sock, sentinela, "whoami", &json!({}));
    assert_eq!(res(&w)["agent"], json!("agente-sentinela"));
    respostas.push(w);
    respostas.push(muta_sock(
        &e.sock,
        sentinela,
        "propose",
        decisao("Com sentinela", "agentd"),
    ));
    respostas.push(uma(&e.sock, sentinela, "status", &json!({})));
    respostas.push(uma(&e.sock, sentinela, "get", &json!({"id": "D-001"})));
    respostas.push(uma(&e.sock, sentinela, "metodo_inexistente", &json!({})));
    respostas.push(uma(&e.sock, TOK_ERRADO, "whoami", &json!({})));
    respostas.push(uma(&e.sock, sentinela, "query", &json!({"limit": 99})));

    for r in &respostas {
        let texto = r.to_string();
        assert!(
            !texto.contains(corpo_sentinela),
            "token em resposta: {texto}"
        );
    }
    assert_eq!(respostas[5]["error"]["code"], json!("unauthorized"));

    let saida = srv.encerra();
    let texto_saida = String::from_utf8_lossy(&saida).to_string();
    assert!(
        !texto_saida.contains(corpo_sentinela),
        "token em log de diagnostico: {texto_saida}"
    );
    let log = fs::read(e.estado.join("log.jsonl")).unwrap();
    assert!(
        !String::from_utf8_lossy(&log).contains(corpo_sentinela),
        "token no log.jsonl"
    );
}

#[test]
fn o03b_token_hash_le_do_stdin_e_imprime_so_o_sha256() {
    let mut filho = memd()
        .arg("token-hash")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    filho
        .stdin
        .take()
        .unwrap()
        .write_all(TOK_SENTINELA.as_bytes())
        .unwrap();
    let out = filho.wait_with_output().unwrap();
    assert!(
        out.status.success(),
        "token-hash: {}",
        String::from_utf8_lossy(&out.stderr)
    );

    let impresso = String::from_utf8_lossy(&out.stdout).trim().to_string();
    assert_eq!(
        impresso,
        memd::token_sha256_hex(TOK_SENTINELA),
        "SHA-256 hexadecimal do token"
    );
    assert_eq!(impresso.len(), 64);
    assert!(
        impresso
            .bytes()
            .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
    );
    assert!(!String::from_utf8_lossy(&out.stdout).contains("SentinelaZx9"));
    assert!(!String::from_utf8_lossy(&out.stderr).contains("SentinelaZx9"));
}

// ------------------------------------------------------------------ O6 modos e socket padrao

#[test]
fn o06_socket_padrao_fica_no_estado_com_modo_0600_e_estado_0700() {
    let e = prepara(&todos_os_perfis_a(), &cfg_padrao());
    let mut srv = sobe_com(&e, None);
    let sock = e.estado.join("memd.sock");
    assert!(sock.exists(), "socket padrao em <estado>/memd.sock");
    assert_eq!(modo(&sock), 0o600, "socket 0600");
    assert_eq!(modo(&e.estado), 0o700, "diretorio de estado 0700");
    assert_eq!(
        res(&uma(&sock, TOK_A, "whoami", &json!({})))["agent"],
        json!("agente-a")
    );
    srv.encerra();
}

// ------------------------------------------------------------------ O7 recusa de partida

/// Espera a saida do processo; `None` se ele nao sair no prazo (o teste mata e falha).
fn espera_saida(mut filho: Child) -> Option<ExitStatus> {
    let t0 = Instant::now();
    loop {
        if let Some(st) = filho.try_wait().unwrap() {
            return Some(st);
        }
        if t0.elapsed() > ESPERA {
            let _ = filho.kill();
            let _ = filho.wait();
            return None;
        }
        thread::sleep(Duration::from_millis(20));
    }
}

fn serve_sem_subir(e: &Estado, cfg: &Path, grants_p: &Path) -> Option<ExitStatus> {
    let filho = memd()
        .arg("serve")
        .arg("--state-dir")
        .arg(&e.estado)
        .arg("--config")
        .arg(cfg)
        .arg("--grants")
        .arg(grants_p)
        .arg("--socket")
        .arg(&e.sock)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    espera_saida(filho)
}

#[test]
fn o07_serve_recusa_partida_com_config_fora_da_faixa_ou_chave_desconhecida() {
    let e = prepara(&todos_os_perfis_a(), &cfg_padrao());
    let ruins = [
        cfg_json_com(&[("proposal_ttl_days", json!(0))]),
        cfg_json_com(&[("view_budget", json!(999))]),
        cfg_json_com(&[("view_overdue_max", json!(21))]),
        {
            let mut c = base_cfg();
            c["chave_inventada"] = json!(1);
            c.to_string()
        },
    ];
    for (i, texto) in ruins.iter().enumerate() {
        let cfg = e.estado.parent().unwrap().join(format!("ruim-{i}.json"));
        fs::write(&cfg, texto).unwrap();
        let st = serve_sem_subir(&e, &cfg, &e.grants);
        match st {
            Some(s) => assert!(!s.success(), "config ruim {i} deve recusar a partida"),
            None => panic!("config ruim {i}: serve nao saiu (subiu sem validar)"),
        }
        assert!(!e.sock.exists(), "config ruim {i}: nenhum socket criado");
    }

    // controle: a mesma rotina com config valida sobe
    let srv = sobe(&e);
    drop(srv);
}

// ------------------------------------------------------------------ A5 SIGHUP em conexao aberta

#[test]
fn a05_sighup_revoga_token_mesmo_em_conexao_ja_aberta() {
    let perfis = grants(&[
        perfil_total("agente-a", TOK_A),
        perfil_total("agente-revogado", TOK_B),
    ]);
    let e = prepara(&perfis, &cfg_padrao());
    let mut srv = sobe(&e);

    let mut conexao_b = conecta(&e.sock);
    let mut conexao_a = conecta(&e.sock);
    // controle: B funciona antes da revogacao, inclusive nesta conexao
    assert_eq!(
        res(&envia(
            &mut conexao_b,
            &linha_req("c", TOK_B, "whoami", &json!({}))
        ))["agent"],
        json!("agente-revogado")
    );

    // token errado: unauthorized
    assert_eq!(
        cod(&uma(&e.sock, TOK_ERRADO, "whoami", &json!({}))),
        "unauthorized"
    );

    // revoga: remove B do grants.json (mesmo modo 0600) e manda SIGHUP
    fs::write(&e.grants, grants(&[perfil_total("agente-a", TOK_A)])).unwrap();
    fixa_modo(&e.grants, 0o600);
    let rc = envia_sighup(srv.pid());
    assert!(rc, "kill -HUP deve funcionar");

    // na MESMA conexao aberta, B passa a receber unauthorized (com prazo para o processo recarregar)
    let t0 = Instant::now();
    loop {
        let v = envia(&mut conexao_b, &linha_req("c", TOK_B, "whoami", &json!({})));
        if v["ok"] == json!(false) {
            assert_eq!(cod(&v), "unauthorized");
            break;
        }
        assert!(t0.elapsed() < ESPERA, "SIGHUP nao revogou B em {ESPERA:?}");
        thread::sleep(Duration::from_millis(50));
    }

    // A continua valido na conexao que ja estava aberta, e em conexao nova B segue negado
    assert_eq!(
        res(&envia(
            &mut conexao_a,
            &linha_req("c", TOK_A, "whoami", &json!({}))
        ))["agent"],
        json!("agente-a")
    );
    assert_eq!(
        cod(&uma(&e.sock, TOK_B, "whoami", &json!({}))),
        "unauthorized"
    );
    srv.encerra();
}

/// SIGHUP ao processo `pid` pelo utilitario `kill` (o teste nao usa `unsafe`).
fn envia_sighup(pid: i32) -> bool {
    Command::new("kill")
        .arg("-HUP")
        .arg(pid.to_string())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

// ------------------------------------------------------------------ A8 grants.json intocado

#[test]
fn a08_nenhuma_operacao_altera_grants_json() {
    let perfis = grants(&[
        perfil_total("agente-a", TOK_A),
        perfil(
            "agente-b",
            TOK_B,
            &[],
            &["agentd"],
            &[],
            &["decision"],
            "2099-12-31",
            "so leitura",
        ),
    ]);
    let e = prepara(&perfis, &cfg_padrao());
    let mut srv = sobe(&e);
    let bytes_antes = fs::read(&e.grants).unwrap();
    let mtime_antes = fs::metadata(&e.grants).unwrap().modified().unwrap();

    let sock = &e.sock;
    muta_sock(sock, TOK_A, "propose", decisao("Primeira", "agentd"));
    muta_sock(sock, TOK_A, "propose", decisao("Segunda", "agentd"));
    muta_sock(
        sock,
        TOK_A,
        "promote",
        json!({"id": "D-001", "rev": 1, "expected_seq": 1}),
    );
    muta_sock(
        sock,
        TOK_A,
        "reject",
        json!({"id": "D-002", "rev": 1, "reason": "x"}),
    );
    muta_sock(sock, TOK_B, "propose", decisao("Negada", "agentd"));
    uma(sock, TOK_A, "get", &json!({"id": "D-001"}));
    uma(sock, TOK_A, "query", &json!({"status": "proposed"}));
    uma(sock, TOK_A, "view", &json!({}));
    uma(sock, TOK_B, "whoami", &json!({}));
    uma(sock, TOK_B, "status", &json!({}));
    uma(sock, TOK_ERRADO, "whoami", &json!({}));

    assert_eq!(
        fs::read(&e.grants).unwrap(),
        bytes_antes,
        "grants.json com os mesmos bytes"
    );
    assert_eq!(
        fs::metadata(&e.grants).unwrap().modified().unwrap(),
        mtime_antes,
        "mesmo mtime"
    );
    assert_eq!(modo(&e.grants), 0o600);
    srv.encerra();
}

// ------------------------------------------------------------------ M1 32 clientes pelo socket

#[test]
fn m01_socket_32_clientes_simultaneos_geram_d001_a_d032() {
    let cfg = cfg_json_com(&[
        ("max_open_proposals", json!(100)),
        ("max_connections", json!(32)),
    ]);
    let e = prepara(&todos_os_perfis_a(), &cfg);
    let mut srv = sobe(&e);
    let sock = e.sock.clone();

    let barreira = Arc::new(Barrier::new(32));
    let handles: Vec<_> = (0..32)
        .map(|i| {
            let sock = sock.clone();
            let barreira = Arc::clone(&barreira);
            thread::spawn(move || {
                let mut c = conecta(&sock);
                barreira.wait(); // todos conectados antes de qualquer envio
                let mut p = decisao(&format!("Cliente {i:02}"), "agentd");
                p["request_id"] = json!(format!("cli-{i:02}"));
                let v = envia(&mut c, &linha_req("c", TOK_A, "propose", &p));
                res(&v)["id"].as_str().unwrap().to_string()
            })
        })
        .collect();

    let mut ids: Vec<String> = handles.into_iter().map(|h| h.join().unwrap()).collect();
    ids.sort();
    let esperado: Vec<String> = (1..=32).map(|n| format!("D-{n:03}")).collect();
    assert_eq!(ids, esperado, "32 ids distintos, sem lacuna, pelo socket");

    let st = uma(&sock, TOK_A, "status", &json!({}));
    assert_eq!(res(&st)["head_seq"], json!(32), "um commit por cliente");
    srv.encerra();
}

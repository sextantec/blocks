//! v1.5 (docs/spec-v1.5-cliente-e-resultado.md): configuracao do cliente com perfis (Parte A, C1-C8)
//! e resultado automatico por RESPOSTA (Parte B, R1-R8). Escrito so a partir do contrato; o codigo
//! de src/ nao foi lido. Integracao: so o binario, o socket e os arquivos.
//! Todo diretorio e temporario; HOME, XDG_CONFIG_HOME e CHATD_CONFIG apontam para dentro dele.
//! Nenhum processo real, nenhuma rede. Devem falhar ate a implementacao chegar (e compilar com o binario atual).
#![allow(dead_code)]
mod common;

use common::*;
use serde_json::{Value, json};
use std::fs::{self, File};
use std::io::{BufRead, BufReader};
use std::os::unix::fs::PermissionsExt;
use std::os::unix::net::UnixListener;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};

const A: &str = "claude-main";
const B: &str = "codex-web";
const C: &str = "terceiro";
/// Marcador que o arquivo de configuracao (C5) carrega; nunca pode aparecer na saida de erro.
const MARCA: &str = "SEGREDO-CONFIG-V15";

/// Variaveis lidas do ambiente pelo cliente e pelo servidor; removidas antes de cada execucao.
const VARS_LIMPAS: &[&str] = &[
    "CHATD_SOCKET",
    "CHATD_STATE_DIR",
    "CHATD_AS",
    "CHATD_TOKEN_FILE",
    "CHATD_PROFILE",
    "CHATD_CONFIG",
    "HERDR_PANE_ID",
];

static CONTADOR: AtomicUsize = AtomicUsize::new(0);

// ---------- infraestrutura ----------

/// Diretorio temporario com `home/` e `xdg/` (HOME e XDG_CONFIG_HOME apontam para la), `state/` e tokens.
struct Amb {
    d: tempfile::TempDir,
}

impl Amb {
    fn novo() -> Amb {
        let d = tmp();
        fs::create_dir_all(d.path().join("home")).unwrap();
        fs::create_dir_all(d.path().join("xdg")).unwrap();
        Amb { d }
    }
    fn p(&self) -> &Path {
        self.d.path()
    }
    fn home(&self) -> PathBuf {
        self.p().join("home")
    }
    fn xdg(&self) -> PathBuf {
        self.p().join("xdg")
    }
    /// Caminho padrao do arquivo do cliente: $XDG_CONFIG_HOME/chatd/client.json.
    fn cfg_padrao(&self) -> PathBuf {
        self.xdg().join("chatd").join("client.json")
    }
    fn state(&self) -> PathBuf {
        self.p().join("state")
    }
    fn socket(&self) -> PathBuf {
        self.state().join("chatd.sock")
    }
    fn token(&self, nome: &str) -> PathBuf {
        self.p().join(format!("{nome}.tok"))
    }
}

/// Grava `conteudo` em `p` com o modo dado (o modo e fixado depois, para ignorar a umask).
fn escreve(p: &Path, conteudo: &str, modo: u32) -> PathBuf {
    fs::create_dir_all(p.parent().unwrap()).unwrap();
    fs::write(p, conteudo).unwrap();
    fs::set_permissions(p, fs::Permissions::from_mode(modo)).unwrap();
    p.to_path_buf()
}

/// Arquivo do cliente no caminho padrao, modo 0600 (o unico modo aceito sem erro).
fn grava_cfg(a: &Amb, valor: &Value) {
    escreve(&a.cfg_padrao(), &valor.to_string(), 0o600);
}

/// Roda o chatd com ambiente limpo: variaveis do cliente removidas, HOME e XDG_CONFIG_HOME no temporario,
/// `envs` por cima. Saida em arquivos; limite de 10 s.
fn roda(a: &Amb, args: &[&str], envs: &[(&str, &str)]) -> Saida {
    let n = CONTADOR.fetch_add(1, Ordering::SeqCst);
    let out_p = a.p().join(format!("saida-{n}.out"));
    let err_p = a.p().join(format!("saida-{n}.err"));
    let mut cmd = Command::new(BIN);
    cmd.args(args);
    for v in VARS_LIMPAS {
        cmd.env_remove(v);
    }
    cmd.env("HOME", a.home()).env("XDG_CONFIG_HOME", a.xdg());
    cmd.envs(envs.iter().copied());
    let mut child = cmd
        .stdin(Stdio::null())
        .stdout(File::create(&out_p).unwrap())
        .stderr(File::create(&err_p).unwrap())
        .spawn()
        .expect("executar chatd");
    let fim = Instant::now() + Duration::from_secs(10);
    let code = loop {
        if let Some(st) = child.try_wait().expect("try_wait") {
            break st.code();
        }
        if Instant::now() >= fim {
            let _ = child.kill();
            let _ = child.wait();
            break None;
        }
        thread::sleep(Duration::from_millis(10));
    };
    Saida {
        code,
        stdout: fs::read_to_string(&out_p).unwrap_or_default(),
        stderr: fs::read_to_string(&err_p).unwrap_or_default(),
    }
}

/// Como `roda`, com a identidade `quem` pelo ambiente (socket, token e nome do proprio agente).
fn como(a: &Amb, quem: &str, args: &[&str]) -> Saida {
    let sock = a.socket().display().to_string();
    let tok = a.token(quem).display().to_string();
    roda(
        a,
        args,
        &[
            ("CHATD_SOCKET", sock.as_str()),
            ("CHATD_AS", quem),
            ("CHATD_TOKEN_FILE", tok.as_str()),
        ],
    )
}

/// `whoami --json` com argumentos extras; devolve a saida e o JSON (Null se nao for JSON).
fn whoami_json(a: &Amb, extra: &[&str], envs: &[(&str, &str)]) -> (Saida, Value) {
    let mut v = vec!["whoami", "--json"];
    v.extend_from_slice(extra);
    let r = roda(a, &v, envs);
    let j = serde_json::from_str(r.stdout.trim()).unwrap_or(Value::Null);
    (r, j)
}

/// Origem de um campo no `whoami --json` (`sources`): "flag", "env", "config" ou "-".
fn fonte(j: &Value, campo: &str) -> String {
    j["sources"][campo]
        .as_str()
        .unwrap_or_else(|| panic!("origem de {campo} ausente em {j}"))
        .to_string()
}

/// Campo texto de um JSON; falha se ausente.
fn txt(j: &Value, campo: &str) -> String {
    j[campo]
        .as_str()
        .unwrap_or_else(|| panic!("campo {campo} ausente ou nao texto em {j}"))
        .to_string()
}

/// Configuracao do cliente com socket e estado (sem perfis).
fn cfg_base(a: &Amb) -> Value {
    json!({
        "socket": a.socket().display().to_string(),
        "state_dir": a.state().display().to_string(),
    })
}

/// Configuracao do cliente com perfis: `claude` (padrao, as=claude-main) e `codex` (as=codex-web).
fn cfg_perfis(a: &Amb) -> Value {
    json!({
        "socket": a.socket().display().to_string(),
        "state_dir": a.state().display().to_string(),
        "default_profile": "claude",
        "profiles": {
            "claude": {"as": A, "token_file": a.token(A).display().to_string()},
            "codex": {"as": B, "token_file": a.token(B).display().to_string()}
        }
    })
}

/// Erro de uso esperado (saida 2) sem conectar: o socket da config aponta para um ouvinte nosso,
/// que nao pode aceitar nenhuma conexao.
fn erro_sem_conectar(a: &Amb, args: &[&str], envs: &[(&str, &str)]) -> Saida {
    fs::create_dir_all(a.state()).unwrap();
    // Soltar o UnixListener nao apaga o arquivo do socket: remove antes, para permitir mais de uma chamada.
    let _ = fs::remove_file(a.socket());
    let ouvinte = UnixListener::bind(a.socket()).expect("ouvinte no socket de teste");
    let r = roda(a, args, envs);
    ouvinte.set_nonblocking(true).unwrap();
    let conectou = ouvinte.accept().is_ok();
    assert!(
        !conectou,
        "o cliente conectou ao socket apesar do erro de configuracao: {r:?}"
    );
    assert_eq!(r.code, Some(2), "esperava erro de uso (saida 2): {r:?}");
    r
}

/// Configuracao do servidor: auth obrigatoria (padrao), campainha inerte, tres destinatarios por watch.
fn cfg_servidor() -> Value {
    json!({
        "herdr_bin": "/nonexistent/chatd-herdr-v15",
        "bell_poll_ms": 5000,
        "bell_rearm_s": 600,
        "recipients": {
            "claude-main": {"delivery": "watch"},
            "codex-web": {"delivery": "watch"},
            "terceiro": {"delivery": "watch"}
        }
    })
}

/// Servidor do chatd, com ambiente limpo. Encerra no drop.
struct Srv {
    child: Child,
    out: PathBuf,
    err: PathBuf,
}

impl Srv {
    /// Sobe `serve --state-dir STATE --config CFG` com `envs` por cima; espera `chatd: ready` (10 s).
    fn sobe(a: &Amb, state: &Path, cfg: &Path, envs: &[(&str, &str)], tag: &str) -> Srv {
        let out = a.p().join(format!("serve-{tag}.out"));
        let err = a.p().join(format!("serve-{tag}.err"));
        let mut cmd = Command::new(BIN);
        cmd.args(["serve", "--state-dir", s(state), "--config", s(cfg)]);
        for v in VARS_LIMPAS {
            cmd.env_remove(v);
        }
        cmd.env("HOME", a.home()).env("XDG_CONFIG_HOME", a.xdg());
        cmd.envs(envs.iter().copied());
        let child = cmd
            .stdin(Stdio::null())
            .stdout(File::create(&out).unwrap())
            .stderr(File::create(&err).unwrap())
            .spawn()
            .expect("executar serve");
        let srv = Srv { child, out, err };
        let pronto = espera(Duration::from_secs(10), || {
            fs::read_to_string(&srv.out)
                .unwrap_or_default()
                .lines()
                .any(|l| l == PRONTO)
        });
        assert!(
            pronto,
            "servidor nao ficou pronto; stderr: {}",
            srv.stderr_txt()
        );
        srv
    }

    fn stderr_txt(&self) -> String {
        fs::read_to_string(&self.err).unwrap_or_default()
    }

    /// SIGTERM e espera ate 5 s pela saida.
    fn para(&mut self) -> Option<i32> {
        unsafe { libc::kill(self.child.id() as libc::pid_t, libc::SIGTERM) };
        let fim = Instant::now() + Duration::from_secs(5);
        loop {
            if let Ok(Some(st)) = self.child.try_wait() {
                return st.code();
            }
            if Instant::now() >= fim {
                return None;
            }
            thread::sleep(Duration::from_millis(20));
        }
    }
}

impl Drop for Srv {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// Emite token de `nome` no estado `state`, gravando em `out` (0600). Devolve o token.
fn emite_token(a: &Amb, state: &Path, nome: &str, out: &Path) -> String {
    let r = roda(
        a,
        &[
            "token",
            "new",
            nome,
            "--state-dir",
            s(state),
            "--out",
            s(out),
        ],
        &[],
    );
    assert_eq!(r.code, Some(0), "token new {nome}: {r:?}");
    fs::read_to_string(out)
        .expect("ler arquivo do token")
        .trim()
        .to_string()
}

/// Servidor no estado `amb/state` e tokens de A, B e C em `amb/<nome>.tok`.
fn amb_servidor() -> (Amb, Srv) {
    let a = Amb::novo();
    let cfg = escreve(&a.p().join("srv.json"), &cfg_servidor().to_string(), 0o600);
    let srv = Srv::sobe(&a, &a.state(), &cfg, &[], "principal");
    for n in [A, B, C] {
        emite_token(&a, &a.state(), n, &a.token(n));
    }
    (a, srv)
}

/// `chatd watch --as QUEM` (com `extra`) lido por thread. Espera o heartbeat (conectado e autenticado).
struct Watch {
    child: Child,
    linhas: mpsc::Receiver<String>,
}

impl Watch {
    fn inicia(a: &Amb, quem: &str, extra: &[&str]) -> Watch {
        let hb = a.p().join(format!(
            "hb-{quem}-{}.txt",
            CONTADOR.fetch_add(1, Ordering::SeqCst)
        ));
        let sock = a.socket().display().to_string();
        let tok = a.token(quem).display().to_string();
        let mut cmd = Command::new(BIN);
        cmd.args(["watch", "--as", quem, "--heartbeat-file", s(&hb)])
            .args(extra);
        for v in VARS_LIMPAS {
            cmd.env_remove(v);
        }
        cmd.env("HOME", a.home())
            .env("XDG_CONFIG_HOME", a.xdg())
            .env("CHATD_SOCKET", sock.as_str())
            .env("CHATD_TOKEN_FILE", tok.as_str());
        let mut child = cmd
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .expect("executar watch");
        let out = child.stdout.take().unwrap();
        let (tx, rx) = mpsc::channel();
        thread::spawn(move || {
            for l in BufReader::new(out).lines().map_while(Result::ok) {
                if tx.send(l).is_err() {
                    break;
                }
            }
        });
        let w = Watch { child, linhas: rx };
        assert!(
            espera(Duration::from_secs(5), || hb.exists()),
            "watch de {quem} nao conectou (sem heartbeat)"
        );
        w
    }

    /// Todas as linhas que chegarem nesse prazo.
    fn linhas_por(&self, d: Duration) -> Vec<String> {
        let fim = Instant::now() + d;
        let mut v = Vec::new();
        while Instant::now() < fim {
            match self.linhas.recv_timeout(fim - Instant::now()) {
                Ok(l) => v.push(l),
                Err(_) => break,
            }
        }
        v
    }
}

impl Drop for Watch {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

// ---------- Parte B: helpers ----------

/// `send` pela CLI como `de`. `thread` vai em --thread; `extra` (ex.: --json, --keep-open) no fim.
fn envia_cli(
    a: &Amb,
    de: &str,
    para: &str,
    kind: &str,
    id: &str,
    thread: Option<&str>,
    extra: &[&str],
) -> Saida {
    let mut v = vec![
        "send",
        "--from",
        de,
        "--to",
        para,
        "--kind",
        kind,
        "--subject",
        "assunto",
        "--body",
        "corpo",
        "--id",
        id,
    ];
    if let Some(t) = thread {
        v.push("--thread");
        v.push(t);
    }
    v.extend_from_slice(extra);
    como(a, de, &v)
}

/// `inbox --take` como `quem` (aceita as mensagens para ele).
fn pega(a: &Amb, quem: &str) -> Saida {
    como(a, quem, &["inbox", "--take", "--as", quem])
}

/// `ack` como `quem` por CLI.
fn confirma(a: &Amb, quem: &str, id: &str, estagio: &str, resultado: Option<&str>) -> Saida {
    let mut v = vec!["ack", "--id", id, "--as", quem, "--stage", estagio];
    if let Some(r) = resultado {
        v.push("--result");
        v.push(r);
    }
    como(a, quem, &v)
}

/// Mensagem `id` vista por `quem` (message.get, conexao autenticada).
fn mensagem(a: &Amb, quem: &str, id: &str) -> Value {
    let tok = fs::read_to_string(a.token(quem))
        .unwrap()
        .trim()
        .to_string();
    let mut c = conexao_auth(&a.socket(), quem, &tok);
    let r = c.chama("message.get", json!({"v": 1, "id": id}));
    res(&r)["message"].clone()
}

/// Eventos `tipo` do log.jsonl ligados a `id`.
fn eventos_do_log(a: &Amb, tipo: &str, id: &str) -> Vec<Value> {
    eventos_tipo(&a.state(), tipo)
        .into_iter()
        .filter(|e| e["id"] == id)
        .collect()
}

/// Saida da CLI como JSON (uma linha); falha com a saida crua se nao for JSON.
fn json_da_saida(r: &Saida) -> Value {
    serde_json::from_str(r.stdout.trim())
        .unwrap_or_else(|e| panic!("saida nao e JSON ({e}): {r:?}"))
}

/// Sem nenhuma ocorrencia de `tok` no texto.
fn sem(texto: &str, tok: &str) -> bool {
    !texto.contains(tok)
}

// =====================================================================
// Parte A: configuracao do cliente com perfis
// =====================================================================

// ---------- C1: sem arquivo e sem variaveis = erro de uso; com as da v1.4 = igual ----------

#[test]
fn c1_sem_arquivo_e_sem_variaveis_e_erro_de_uso() {
    let a = Amb::novo();
    let r = roda(&a, &["status"], &[]);
    assert_eq!(r.code, Some(2), "saida: {r:?}");
    assert!(
        !r.stderr.trim().is_empty(),
        "erro de uso deve dizer algo em stderr"
    );
}

// atualizado pela v1.6: so CHATD_SOCKET nao tem identidade explicita; a saida 2 vem com
// "identidade nao definida" antes de qualquer checagem de token (regra A.3). O cenario completo da v1.4
// (CHATD_AS + CHATD_TOKEN_FILE + CHATD_SOCKET) continua abaixo, igual.
#[test]
fn c1_com_variaveis_da_v14_a_saida_e_igual_regressao_zero() {
    let (a, _srv) = amb_servidor();
    let sock = a.socket().display().to_string();
    // So o socket: sem --as/CHATD_AS e sem --profile, o cliente nao define identidade (fail-closed).
    // atualizado pela v1.6 (Esclarecimento 12): status nao exige identidade; o fail-closed vale para inbox.
    let so_socket = roda(&a, &["inbox", "--json"], &[("CHATD_SOCKET", sock.as_str())]);
    assert_eq!(so_socket.code, Some(2), "so CHATD_SOCKET: {so_socket:?}");
    assert!(
        so_socket.stderr.contains("identidade nao definida"),
        "mensagem de identidade ausente (v1.6, regra A.3): {so_socket:?}"
    );
    assert!(
        !so_socket.stderr.contains("token ausente"),
        "com identidade nao definida, o erro de token nao aparece antes: {so_socket:?}"
    );
    // Identidade completa da v1.4 (CHATD_SOCKET, CHATD_AS, CHATD_TOKEN_FILE de A): saida 0 e regressao zero.
    let tok = a.token(A).display().to_string();
    let completo = roda(
        &a,
        &["status"],
        &[
            ("CHATD_SOCKET", sock.as_str()),
            ("CHATD_AS", A),
            ("CHATD_TOKEN_FILE", tok.as_str()),
        ],
    );
    assert_eq!(
        completo.code,
        Some(0),
        "status com a identidade da v1.4: {completo:?}"
    );
    let via_como = como(&a, A, &["status"]);
    assert_eq!(via_como.code, Some(0), "{via_como:?}");
    assert_eq!(
        completo.stdout, via_como.stdout,
        "a saida do status deve ser a mesma nos dois caminhos"
    );
    let r2 = como(&a, A, &["inbox", "--json"]);
    assert_eq!(
        r2.code,
        Some(0),
        "inbox com CHATD_AS e CHATD_TOKEN_FILE: {r2:?}"
    );
}

// ---------- C2: whoami mostra a origem; default_profile nao da identidade (v1.6) ----------

// atualizado pela v1.6: default_profile e ignorado como fonte de identidade (regra A.5 revogada); sem
// --as/--profile e sem variaveis, status e inbox saem 2 sem conectar.
// atualizado pela v1.6 (Esclarecimento 12): inbox sem identidade sai 2 sem conectar; status nao exige identidade,
// nao abre o token do default_profile (FIFO aqui) e nao sai 2 por falta de identidade. Sem daemon no teste:
// o status cai no ENOENT/ECONNREFUSED (saida 4), e so se confere que o FIFO nao travou o processo.
#[test]
fn c2_default_profile_nao_fornece_mais_identidade_v16() {
    let a = Amb::novo();
    let tok = a.p().join("token-fifo-c2.tok");
    let c =
        std::ffi::CString::new(std::os::unix::ffi::OsStrExt::as_bytes(tok.as_os_str())).unwrap();
    assert_eq!(unsafe { libc::mkfifo(c.as_ptr(), 0o600) }, 0, "mkfifo");
    grava_cfg(
        &a,
        &json!({
            "socket": a.socket().display().to_string(),
            "state_dir": a.state().display().to_string(),
            "default_profile": "claude",
            "profiles": {"claude": {"as": A, "token_file": tok.display().to_string()}}
        }),
    );
    let r = erro_sem_conectar(&a, &["inbox", "--json"], &[]);
    assert!(
        r.stderr.contains("identidade nao definida"),
        "inbox sem identidade explicita: {r:?}"
    );
    let _ = fs::remove_file(a.socket());
    let st = roda(&a, &["status"], &[]);
    assert!(
        st.code.is_some(),
        "status travou: o cliente abriu o FIFO do token do default_profile"
    );
    assert_ne!(st.code, Some(2), "status nao exige identidade: {st:?}");
    assert!(
        !st.stderr.contains("identidade nao definida"),
        "status nao deve reclamar de identidade: {st:?}"
    );
}

// atualizado pela v1.6: o teste escolhe o perfil com --profile; default_profile sozinho nao define identidade.
#[test]
fn c2_whoami_mostra_a_identidade_do_perfil_escolhido() {
    let a = Amb::novo();
    grava_cfg(&a, &cfg_perfis(&a));
    let (r, j) = whoami_json(&a, &["--profile", "claude"], &[]);
    assert_eq!(r.code, Some(0), "whoami: {r:?}");
    assert_eq!(txt(&j, "as"), A);
    assert_eq!(txt(&j, "profile"), "claude");
    assert_eq!(txt(&j, "socket"), a.socket().display().to_string());
    assert_eq!(txt(&j, "token_file"), a.token(A).display().to_string());
}

// atualizado pela v1.6: com --profile explicito, a identidade vem do perfil e a origem e profile:NOME
// (Esclarecimento 1). O socket continua vindo do arquivo ("config").
#[test]
fn c2_whoami_mostra_origem_config_para_o_perfil() {
    let a = Amb::novo();
    grava_cfg(&a, &cfg_perfis(&a));
    let r = roda(&a, &["whoami", "--profile", "claude"], &[]);
    assert_eq!(r.code, Some(0), "{r:?}");
    let linhas: Vec<&str> = r.stdout.lines().collect();
    let sock = a.socket().display().to_string();
    assert!(
        linhas.contains(&format!("as={A} (profile:claude)").as_str()),
        "linha as sem origem profile:claude: {linhas:?}"
    );
    assert!(
        linhas.contains(&format!("socket={sock} (config)").as_str()),
        "linha socket sem origem config: {linhas:?}"
    );
    let (_, j) = whoami_json(&a, &["--profile", "claude"], &[]);
    assert_eq!(fonte(&j, "as"), "profile:claude");
}

// ---------- C3: precedencia por valor: flag > env > arquivo ----------

// atualizado pela v1.6: a terceira checagem usa --profile explicito (default_profile nao da identidade).
#[test]
fn c3_as_de_flag_e_env_vencem_o_perfil() {
    let a = Amb::novo();
    grava_cfg(&a, &cfg_perfis(&a));
    let (r, j) = whoami_json(&a, &["--as", "outro-flag"], &[]);
    assert_eq!(r.code, Some(0), "{r:?}");
    assert_eq!(txt(&j, "as"), "outro-flag", "--as deve vencer o perfil");
    let (_, j2) = whoami_json(&a, &[], &[("CHATD_AS", "outro-env")]);
    assert_eq!(txt(&j2, "as"), "outro-env", "CHATD_AS deve vencer o perfil");
    // v1.6: sem flag, env nem --profile, default_profile nao escolhe perfil: as fica null.
    let (_, j3) = whoami_json(&a, &["--profile", "claude"], &[]);
    assert_eq!(
        txt(&j3, "as"),
        A,
        "sem flag nem env, vale o perfil escolhido"
    );
}

#[test]
fn c3_socket_token_flag_env_e_arquivo_nesta_ordem() {
    let a = Amb::novo();
    let mut cfg = cfg_perfis(&a);
    cfg["socket"] = json!("/caminho/do/arquivo.sock");
    grava_cfg(&a, &cfg);
    let (_, j) = whoami_json(
        &a,
        &["--socket", "/caminho/da/flag.sock"],
        &[("CHATD_SOCKET", "/caminho/do/env.sock")],
    );
    assert_eq!(
        txt(&j, "socket"),
        "/caminho/da/flag.sock",
        "flag vence env e arquivo"
    );
    let (_, j2) = whoami_json(&a, &[], &[("CHATD_SOCKET", "/caminho/do/env.sock")]);
    assert_eq!(
        txt(&j2, "socket"),
        "/caminho/do/env.sock",
        "env vence o arquivo"
    );
    let (_, j3) = whoami_json(&a, &[], &[]);
    assert_eq!(
        txt(&j3, "socket"),
        "/caminho/do/arquivo.sock",
        "arquivo vale sem flag nem env"
    );
    let (_, j4) = whoami_json(
        &a,
        &["--token-file", "/caminho/da/flag.tok"],
        &[("CHATD_TOKEN_FILE", "/caminho/do/env.tok")],
    );
    assert_eq!(
        txt(&j4, "token_file"),
        "/caminho/da/flag.tok",
        "--token-file vence env e perfil"
    );
    let (_, j5) = whoami_json(&a, &[], &[("CHATD_TOKEN_FILE", "/caminho/do/env.tok")]);
    assert_eq!(
        txt(&j5, "token_file"),
        "/caminho/do/env.tok",
        "CHATD_TOKEN_FILE vence o perfil"
    );
}

// atualizado pela v1.6: token_file e profile sem perfil escolhido tem origem unset/ignored, nao config.
#[test]
fn c3_whoami_mostra_a_origem_certa_em_cada_campo() {
    let a = Amb::novo();
    grava_cfg(&a, &cfg_perfis(&a));
    let (r, j) = whoami_json(
        &a,
        &["--as", "outro-flag"],
        &[("CHATD_SOCKET", "/caminho/do/env.sock")],
    );
    assert_eq!(r.code, Some(0), "{r:?}");
    assert_eq!(fonte(&j, "as"), "flag");
    assert_eq!(fonte(&j, "socket"), "env");
    assert_eq!(fonte(&j, "state_dir"), "config");
    // v1.6: sem --profile, default_profile nao fornece token nem perfil: origens "unset" e "ignored".
    assert_eq!(fonte(&j, "token_file"), "unset");
    assert_eq!(fonte(&j, "profile"), "ignored");
    // Sem config, so com CHATD_SOCKET: campo nao resolvido tem valor null e origem "unset" (atualizado pela v1.6, Esclarecimento 11).
    let b = Amb::novo();
    let (rb, jb) = whoami_json(&b, &[], &[("CHATD_SOCKET", "/caminho/do/env.sock")]);
    assert_eq!(rb.code, Some(0), "{rb:?}");
    assert_eq!(jb["as"], Value::Null);
    assert_eq!(fonte(&jb, "as"), "unset");
    assert_eq!(fonte(&jb, "socket"), "env");
    // Sem --json, a linha de campo nao resolvido e `as=- (unset)` (atualizado pela v1.6, Esclarecimento 11).
    let rt = roda(&b, &["whoami"], &[("CHATD_SOCKET", "/caminho/do/env.sock")]);
    assert_eq!(rt.code, Some(0), "{rt:?}");
    assert!(
        rt.stdout.lines().any(|l| l == "as=- (unset)"),
        "linha de campo nao resolvido ausente: {:?}",
        rt.stdout
    );
}

#[test]
fn c3_flag_e_env_juntas_a_flag_vence() {
    let a = Amb::novo();
    grava_cfg(&a, &cfg_perfis(&a));
    let f = a.p().join("f.tok").display().to_string();
    let g = a.p().join("g.tok").display().to_string();
    let (r, j) = whoami_json(
        &a,
        &["--as", "x-flag", "--token-file", f.as_str()],
        &[("CHATD_AS", "y-env"), ("CHATD_TOKEN_FILE", g.as_str())],
    );
    assert_eq!(r.code, Some(0), "{r:?}");
    assert_eq!(
        txt(&j, "as"),
        "x-flag",
        "--as vence CHATD_AS no mesmo comando"
    );
    assert_eq!(fonte(&j, "as"), "flag");
    assert_eq!(
        txt(&j, "token_file"),
        f,
        "--token-file vence CHATD_TOKEN_FILE no mesmo comando"
    );
    assert_eq!(fonte(&j, "token_file"), "flag");
}

// ---------- C4: --profile e CHATD_PROFILE selecionam o perfil ----------

// atualizado pela v1.6: sem --profile/CHATD_PROFILE, default_profile nao escolhe perfil (as fica null).
#[test]
fn c4_profile_flag_e_env_selecionam_o_perfil() {
    let a = Amb::novo();
    grava_cfg(&a, &cfg_perfis(&a));
    let (r, j) = whoami_json(&a, &["--profile", "codex"], &[]);
    assert_eq!(r.code, Some(0), "{r:?}");
    assert_eq!(txt(&j, "as"), B, "--profile codex deve dar as=codex-web");
    assert_eq!(txt(&j, "profile"), "codex");
    let (_, j2) = whoami_json(&a, &[], &[("CHATD_PROFILE", "codex")]);
    assert_eq!(
        txt(&j2, "as"),
        B,
        "CHATD_PROFILE=codex deve dar as=codex-web"
    );
    let (_, j3) = whoami_json(&a, &[], &[]);
    assert!(
        j3["as"].is_null(),
        "sem escolha explicita, as e null (v1.6): {j3}"
    );
}

#[test]
fn c4_perfil_inexistente_e_saida_2() {
    let a = Amb::novo();
    grava_cfg(&a, &cfg_perfis(&a));
    let r = roda(&a, &["whoami", "--profile", "nao-existe"], &[]);
    assert_eq!(r.code, Some(2), "--profile inexistente: {r:?}");
    let r2 = roda(&a, &["whoami"], &[("CHATD_PROFILE", "nao-existe")]);
    assert_eq!(r2.code, Some(2), "CHATD_PROFILE inexistente: {r2:?}");
    let _ = erro_sem_conectar(&a, &["status", "--profile", "nao-existe"], &[]);
}

// ---------- C5: permissoes do arquivo ----------

#[test]
fn c5_arquivo_0666_gravavel_por_outros_e_recusado_sem_conectar_nem_ecoar() {
    let a = Amb::novo();
    let mut cfg = cfg_base(&a);
    cfg["state_dir"] = json!(format!("{}/{}", a.state().display(), MARCA));
    escreve(&a.cfg_padrao(), &cfg.to_string(), 0o666);
    let r = erro_sem_conectar(&a, &["status"], &[]);
    assert!(
        sem(&r.stderr, MARCA) && sem(&r.stdout, MARCA),
        "a mensagem de erro nao pode conter o conteudo do arquivo: {r:?}"
    );
}

// atualizado pela v1.6: a identidade vem de --profile explicito, nao de default_profile.
#[test]
fn c5_arquivo_0600_e_aceito() {
    let (a, _srv) = amb_servidor();
    escreve(&a.cfg_padrao(), &cfg_perfis(&a).to_string(), 0o600);
    // v1.6: sem --profile o default_profile nao da identidade; o teste de permissao usa o perfil explicito.
    let r = roda(&a, &["status", "--profile", "claude"], &[]);
    assert_eq!(
        r.code,
        Some(0),
        "arquivo 0600 do uid do processo deve valer: {r:?}"
    );
}

#[test]
#[ignore = "AMBIGUO: exige outro uid (root ou segundo usuario); sem isso nao ha como criar um arquivo de outro dono"]
fn c5_arquivo_de_outro_uid_e_recusado() {
    panic!("exige outro uid: nao simulavel sem root");
}

#[test]
fn c5_arquivo_nao_regular_e_recusado() {
    let a = Amb::novo();
    grava_cfg(&a, &cfg_perfis(&a));
    let dir = a.p().join("nao-e-arquivo.json");
    fs::create_dir_all(&dir).unwrap();
    let dir_s = dir.display().to_string();
    let r = erro_sem_conectar(&a, &["status"], &[("CHATD_CONFIG", dir_s.as_str())]);
    assert!(!r.stdout.contains("chatd: ready"), "{r:?}");
}

// ---------- C6: campo desconhecido, JSON invalido, default_profile inexistente e tipo errado ----------

#[test]
fn c6_campo_desconhecido_e_saida_2() {
    let a = Amb::novo();
    let mut top = cfg_base(&a);
    top["campo_novo"] = json!(1);
    grava_cfg(&a, &top);
    let _ = erro_sem_conectar(&a, &["status"], &[]);
    let prof = json!({
        "socket": a.socket().display().to_string(),
        "default_profile": "claude",
        "profiles": {"claude": {"as": A, "token_file": a.token(A).display().to_string(), "extra": 1}}
    });
    grava_cfg(&a, &prof);
    let _ = erro_sem_conectar(&a, &["status"], &[]);
}

#[test]
fn c6_json_invalido_e_saida_2() {
    let a = Amb::novo();
    escreve(&a.cfg_padrao(), "{\"socket\": ", 0o600);
    let _ = erro_sem_conectar(&a, &["status"], &[]);
}

#[test]
fn c6_default_profile_inexistente_e_saida_2() {
    let a = Amb::novo();
    let mut cfg = cfg_perfis(&a);
    cfg["default_profile"] = json!("nao-existe");
    grava_cfg(&a, &cfg);
    let _ = erro_sem_conectar(&a, &["status"], &[]);
}

#[test]
fn c6_tipo_errado_e_saida_2() {
    let a = Amb::novo();
    let mut cfg = cfg_base(&a);
    cfg["socket"] = json!(123);
    grava_cfg(&a, &cfg);
    let _ = erro_sem_conectar(&a, &["status"], &[]);
}

// ---------- C7: token nunca aparece; serve e token new ignoram o arquivo do cliente ----------

// atualizado pela v1.6: o perfil e escolhido com --profile (default_profile nao fornece identidade).
#[test]
fn c7_token_nunca_aparece_em_whoami_stderr_ou_log() {
    let (a, _srv) = amb_servidor();
    grava_cfg(&a, &cfg_perfis(&a));
    let tok = fs::read_to_string(a.token(A)).unwrap().trim().to_string();
    // v1.6: a identidade (e o token) vem de --profile explicito; default_profile sozinho nao basta.
    let (wr, wj) = whoami_json(&a, &["--profile", "claude"], &[]);
    let w = roda(&a, &["whoami", "--profile", "claude"], &[]);
    let st = roda(&a, &["status", "--profile", "claude"], &[]);
    let envio = roda(
        &a,
        &[
            "send",
            "--profile",
            "claude",
            "--from",
            A,
            "--to",
            B,
            "--kind",
            "AVISO",
            "--subject",
            "assunto",
            "--body",
            "corpo",
            "--id",
            "c7-1",
        ],
        &[],
    );
    assert_eq!(envio.code, Some(0), "send pelo perfil: {envio:?}");
    for (quem, texto) in [
        ("whoami --json stdout", wr.stdout.clone()),
        ("whoami --json stderr", wr.stderr.clone()),
        ("whoami stdout", w.stdout.clone()),
        ("whoami stderr", w.stderr.clone()),
        ("status stdout", st.stdout.clone()),
        ("status stderr", st.stderr.clone()),
        ("send stderr", envio.stderr.clone()),
        (
            "log.jsonl",
            fs::read_to_string(a.state().join("log.jsonl")).unwrap(),
        ),
    ] {
        assert!(sem(&texto, &tok), "token apareceu em {quem}");
    }
    assert!(!wj.to_string().contains(&tok), "token no JSON do whoami");
}

#[test]
fn c7_serve_e_token_new_ignoram_o_arquivo_do_cliente() {
    let a = Amb::novo();
    let ruim = json!({"socket": "/caminho/qualquer.sock", "default_profile": "nao-existe", "campo_novo": true});
    escreve(&a.cfg_padrao(), &ruim.to_string(), 0o600);
    let bad = escreve(&a.p().join("ruim.json"), &ruim.to_string(), 0o600);
    let cfg = escreve(&a.p().join("srv.json"), &cfg_servidor().to_string(), 0o600);
    let bad_s = bad.display().to_string();
    let _srv = Srv::sobe(
        &a,
        &a.state(),
        &cfg,
        &[("CHATD_CONFIG", bad_s.as_str())],
        "c7",
    );
    let sanidade = roda(&a, &["status"], &[]);
    assert_eq!(
        sanidade.code,
        Some(2),
        "a config ruim precisa ser erro para o cliente: {sanidade:?}"
    );
    let r = roda(
        &a,
        &[
            "token",
            "new",
            A,
            "--state-dir",
            s(&a.state()),
            "--out",
            s(&a.token(A)),
        ],
        &[("CHATD_CONFIG", bad_s.as_str())],
    );
    assert_eq!(
        r.code,
        Some(0),
        "token new nao deve ler o arquivo do cliente: {r:?}"
    );
    let tok = fs::read_to_string(a.token(A)).unwrap().trim().to_string();
    let _conn = conexao_auth(&a.socket(), A, &tok);
}

// ---------- C8: ~/ e expandido, $VAR nao; CHATD_CONFIG ausente = saida 2 ----------

// atualizado pela v1.6: o perfil e escolhido com --profile (default_profile nao fornece identidade).
#[test]
fn c8_til_e_expandido_com_home() {
    let a = Amb::novo();
    let st = a.home().join("srv");
    let cfg = escreve(&a.p().join("srv.json"), &cfg_servidor().to_string(), 0o600);
    let _srv = Srv::sobe(&a, &st, &cfg, &[], "c8");
    fs::create_dir_all(a.home().join("tok")).unwrap();
    let tok = a.home().join("tok").join("claude.tok");
    emite_token(&a, &st, A, &tok);
    grava_cfg(
        &a,
        &json!({
            "socket": "~/srv/chatd.sock",
            "state_dir": "~/srv",
            "default_profile": "claude",
            "profiles": {"claude": {"as": A, "token_file": "~/tok/claude.tok"}}
        }),
    );
    let (r, j) = whoami_json(&a, &["--profile", "claude"], &[]);
    assert_eq!(r.code, Some(0), "{r:?}");
    assert_eq!(
        txt(&j, "socket"),
        st.join("chatd.sock").display().to_string()
    );
    assert_eq!(txt(&j, "state_dir"), st.display().to_string());
    assert_eq!(txt(&j, "token_file"), tok.display().to_string());
    let s2 = roda(&a, &["status", "--profile", "claude"], &[]);
    assert_eq!(s2.code, Some(0), "status pelo socket com ~/: {s2:?}");
}

#[test]
fn c8_dollar_var_nao_e_expandida() {
    let a = Amb::novo();
    grava_cfg(
        &a,
        &json!({"socket": "$HOME/srv/chatd.sock", "state_dir": "$HOME/srv"}),
    );
    let (_, j) = whoami_json(&a, &[], &[]);
    assert_eq!(
        txt(&j, "socket"),
        "$HOME/srv/chatd.sock",
        "$HOME nao pode ser expandido"
    );
    let st = roda(&a, &["status"], &[]);
    assert_ne!(
        st.code,
        Some(0),
        "o socket literal nao existe; status nao pode funcionar"
    );
}

#[test]
fn c8_chatd_config_ausente_e_saida_2() {
    let a = Amb::novo();
    let ausente = a.p().join("nao-existe.json");
    let ausente_s = ausente.display().to_string();
    let r = roda(&a, &["status"], &[("CHATD_CONFIG", ausente_s.as_str())]);
    assert_eq!(
        r.code,
        Some(2),
        "CHATD_CONFIG apontando para arquivo ausente: {r:?}"
    );
}

// =====================================================================
// Parte B: resultado automatico por RESPOSTA
// =====================================================================

// ---------- R1: RESPOSTA fecha M como resultado/done; recibo chega ao remetente ----------

#[test]
fn r1_resposta_fecha_pedido_como_resultado_done_e_recibo_chega_ao_remetente() {
    let (a, _srv) = amb_servidor();
    let w = Watch::inicia(&a, A, &["--receipts"]);
    let m = envia_cli(&a, A, B, "PEDIDO", "r1-m", None, &[]);
    assert_eq!(m.code, Some(0), "{m:?}");
    assert_eq!(pega(&a, B).code, Some(0));
    let r = envia_cli(&a, B, A, "RESPOSTA", "r1-r", Some("r1-m"), &[]);
    assert_eq!(r.code, Some(0), "{r:?}");
    let msg = mensagem(&a, A, "r1-m");
    assert_eq!(msg["state"], "resultado");
    assert_eq!(msg["result"], "done");
    assert_eq!(msg["note"], "auto:r1-r");
    let linhas = w.linhas_por(Duration::from_millis(1500));
    assert!(
        linhas
            .iter()
            .any(|l| l.starts_with("RECIBO ") && l.contains(" r1-m resultado:done codex-web")),
        "A deveria receber RECIBO ... resultado:done codex-web: {linhas:?}"
    );
}

// ---------- R2: M ainda registrado (B nunca aceitou) e a RESPOSTA vai direto a resultado ----------

#[test]
fn r2_resposta_sem_aceite_previo_vai_direto_a_resultado() {
    let (a, _srv) = amb_servidor();
    assert_eq!(
        envia_cli(&a, A, B, "PEDIDO", "r2-m", None, &[]).code,
        Some(0)
    );
    let r = envia_cli(&a, B, A, "RESPOSTA", "r2-r", Some("r2-m"), &[]);
    assert_eq!(r.code, Some(0), "{r:?}");
    let msg = mensagem(&a, A, "r2-m");
    assert_eq!(msg["state"], "resultado");
    assert_eq!(msg["result"], "done");
    assert_eq!(msg["note"], "auto:r2-r");
}

// ---------- R3: M ja em resultado/refused nao muda; sem evento novo, sem erro ----------

#[test]
fn r3_m_ja_recusada_nao_e_alterada_pela_resposta() {
    let (a, _srv) = amb_servidor();
    assert_eq!(
        envia_cli(&a, A, B, "PEDIDO", "r3-m", None, &[]).code,
        Some(0)
    );
    assert_eq!(pega(&a, B).code, Some(0));
    let rec = confirma(&a, B, "r3-m", "resultado", Some("refused"));
    assert_eq!(rec.code, Some(0), "ack refused antes de responder: {rec:?}");
    let r = envia_cli(&a, B, A, "RESPOSTA", "r3-r", Some("r3-m"), &[]);
    assert_eq!(r.code, Some(0), "a RESPOSTA em si nao pode dar erro: {r:?}");
    let msg = mensagem(&a, A, "r3-m");
    assert_eq!(
        msg["result"], "refused",
        "resultado nao pode ser sobrescrito"
    );
    assert_ne!(msg["note"], "auto:r3-r");
    assert_eq!(
        eventos_do_log(&a, "resulted", "r3-m").len(),
        1,
        "so o resulted do ack manual deve existir"
    );
}

// ---------- R4: reply de terceiro, ou reply.to diferente de M.from, nao fecha ----------

#[test]
fn r4_resposta_de_terceiro_nao_fecha() {
    let (a, _srv) = amb_servidor();
    assert_eq!(
        envia_cli(&a, A, B, "PEDIDO", "r4a-m", None, &[]).code,
        Some(0)
    );
    let r = envia_cli(&a, C, A, "RESPOSTA", "r4a-r", Some("r4a-m"), &[]);
    assert_eq!(r.code, Some(0), "{r:?}");
    let msg = mensagem(&a, A, "r4a-m");
    assert_ne!(
        msg["state"], "resultado",
        "reply de terceiro nao fecha: {msg}"
    );
    assert!(eventos_do_log(&a, "resulted", "r4a-m").is_empty());
}

#[test]
fn r4_destino_da_resposta_diferente_do_remetente_de_m_nao_fecha() {
    let (a, _srv) = amb_servidor();
    assert_eq!(
        envia_cli(&a, A, B, "PEDIDO", "r4b-m", None, &[]).code,
        Some(0)
    );
    let r = envia_cli(&a, B, C, "RESPOSTA", "r4b-r", Some("r4b-m"), &[]);
    assert_eq!(r.code, Some(0), "{r:?}");
    let msg = mensagem(&a, A, "r4b-m");
    assert_ne!(
        msg["state"], "resultado",
        "reply.to != M.from nao fecha: {msg}"
    );
    assert!(eventos_do_log(&a, "resulted", "r4b-m").is_empty());
}

// ---------- R5: --keep-open nao fecha; AVISO, PERGUNTA e ACK com thread nao fecham ----------

#[test]
fn r5_keep_open_nao_fecha_a_mensagem() {
    let (a, _srv) = amb_servidor();
    assert_eq!(
        envia_cli(&a, A, B, "PEDIDO", "r5k-m", None, &[]).code,
        Some(0)
    );
    assert_eq!(pega(&a, B).code, Some(0));
    let r = envia_cli(
        &a,
        B,
        A,
        "RESPOSTA",
        "r5k-r",
        Some("r5k-m"),
        &["--keep-open"],
    );
    assert_eq!(r.code, Some(0), "{r:?}");
    let msg = mensagem(&a, A, "r5k-m");
    assert_eq!(msg["state"], "aceito", "--keep-open nao pode fechar: {msg}");
    assert!(eventos_do_log(&a, "resulted", "r5k-m").is_empty());
}

#[test]
fn r5_avisos_perguntas_e_acks_com_thread_nao_fecham() {
    let (a, _srv) = amb_servidor();
    assert_eq!(
        envia_cli(&a, A, B, "PEDIDO", "r5x-m", None, &[]).code,
        Some(0)
    );
    assert_eq!(pega(&a, B).code, Some(0));
    for (kind, id) in [
        ("AVISO", "r5x-av"),
        ("PERGUNTA", "r5x-pg"),
        ("ACK", "r5x-ack"),
    ] {
        let r = envia_cli(&a, B, A, kind, id, Some("r5x-m"), &[]);
        assert_eq!(r.code, Some(0), "{kind} com thread: {r:?}");
    }
    let msg = mensagem(&a, A, "r5x-m");
    assert_eq!(msg["state"], "aceito", "so RESPOSTA fecha: {msg}");
    assert!(eventos_do_log(&a, "resulted", "r5x-m").is_empty());
}

// ---------- R6: reenvio nao duplica; seq consecutivos; closed em --json ----------

#[test]
fn r6_reenvio_nao_duplica_resulted_e_seq_sao_consecutivos() {
    let (a, _srv) = amb_servidor();
    assert_eq!(
        envia_cli(&a, A, B, "PEDIDO", "r6-m", None, &[]).code,
        Some(0)
    );
    assert_eq!(pega(&a, B).code, Some(0));
    let r1 = envia_cli(&a, B, A, "RESPOSTA", "r6-r", Some("r6-m"), &["--json"]);
    assert_eq!(r1.code, Some(0), "{r1:?}");
    let j1 = json_da_saida(&r1);
    assert_eq!(j1["closed"], "r6-m", "closed deve ser o id de M: {j1}");
    let seq1 = j1["seq"].as_i64().expect("seq numerico");
    let r2 = envia_cli(&a, B, A, "RESPOSTA", "r6-r", Some("r6-m"), &["--json"]);
    assert_eq!(r2.code, Some(0), "{r2:?}");
    let j2 = json_da_saida(&r2);
    assert_eq!(j2["duplicate"], true, "reenvio deve ser duplicata: {j2}");
    assert_eq!(j2["seq"], j1["seq"], "reenvio devolve o mesmo seq");
    let resultados = eventos_do_log(&a, "resulted", "r6-m");
    assert_eq!(
        resultados.len(),
        1,
        "reenvio nao pode gravar outro resulted"
    );
    let registrada = eventos_do_log(&a, "registered", "r6-r");
    assert_eq!(registrada.len(), 1);
    assert_eq!(
        registrada[0]["seq"].as_i64(),
        Some(seq1),
        "registered da resposta"
    );
    assert_eq!(
        resultados[0]["seq"].as_i64(),
        Some(seq1 + 1),
        "resulted logo depois"
    );
    assert_eq!(resultados[0]["note"], "auto:r6-r");
}

#[test]
fn r6_saida_sem_json_nao_ganha_closed() {
    let (a, _srv) = amb_servidor();
    assert_eq!(
        envia_cli(&a, A, B, "PEDIDO", "r6s-m", None, &[]).code,
        Some(0)
    );
    assert_eq!(pega(&a, B).code, Some(0));
    let r = envia_cli(&a, B, A, "RESPOSTA", "r6s-r", Some("r6s-m"), &[]);
    assert_eq!(r.code, Some(0), "{r:?}");
    // Sem --json a saida e o texto da v1.4 (nao o JSON do --json): nao pode mencionar `closed`.
    assert!(
        !r.stdout.contains("closed"),
        "sem --json a saida nao ganha closed: {r:?}"
    );
}

#[test]
fn r6_closed_no_reenvio_repete_o_primeiro_e_nao_grava_resulted() {
    let (a, _srv) = amb_servidor();
    assert_eq!(
        envia_cli(&a, A, B, "PEDIDO", "r6d-m", None, &[]).code,
        Some(0)
    );
    assert_eq!(pega(&a, B).code, Some(0));
    let r1 = envia_cli(&a, B, A, "RESPOSTA", "r6d-r", Some("r6d-m"), &["--json"]);
    assert_eq!(r1.code, Some(0), "{r1:?}");
    assert_eq!(json_da_saida(&r1)["closed"], "r6d-m");
    let r2 = envia_cli(&a, B, A, "RESPOSTA", "r6d-r", Some("r6d-m"), &["--json"]);
    assert_eq!(r2.code, Some(0), "{r2:?}");
    let j2 = json_da_saida(&r2);
    assert_eq!(j2["duplicate"], true, "reenvio deve ser duplicata: {j2}");
    assert_eq!(
        j2["closed"], "r6d-m",
        "reenvio repete o closed do primeiro envio: {j2}"
    );
    assert_eq!(
        eventos_do_log(&a, "resulted", "r6d-m").len(),
        1,
        "reenvio nao grava novo resulted"
    );
}

// ---------- R7: reinicio do daemon mantem resultado; releitura do log sem erro ----------

#[test]
fn r7_reinicio_do_daemon_mantem_resultado_e_o_log_relido_sem_erro() {
    let (a, mut srv) = amb_servidor();
    assert_eq!(
        envia_cli(&a, A, B, "PEDIDO", "r7-m", None, &[]).code,
        Some(0)
    );
    assert_eq!(pega(&a, B).code, Some(0));
    let r = envia_cli(&a, B, A, "RESPOSTA", "r7-r", Some("r7-m"), &[]);
    assert_eq!(r.code, Some(0), "{r:?}");
    assert_eq!(srv.para(), Some(0), "SIGTERM deve encerrar limpo");
    let cfg = a.p().join("srv.json");
    let _srv2 = Srv::sobe(&a, &a.state(), &cfg, &[], "reinicio");
    let msg = mensagem(&a, A, "r7-m");
    assert_eq!(
        msg["state"], "resultado",
        "estado sobrevive ao reinicio: {msg}"
    );
    assert_eq!(msg["result"], "done");
    assert_eq!(msg["note"], "auto:r7-r");
    let log = roda(&a, &["log", "--state-dir", s(&a.state())], &[]);
    assert_eq!(
        log.code,
        Some(0),
        "chatd log deve reler o log sem erro: {log:?}"
    );
    let res_ev = eventos_do_log(&a, "resulted", "r7-m");
    assert_eq!(res_ev.len(), 1);
    assert_eq!(res_ev[0]["note"], "auto:r7-r");
}

// ---------- R8: regressao: ack manual depois do automatico segue a regra da v1.4 ----------

#[test]
fn r8_ack_manual_depois_do_automatico_segue_a_regra_da_v14() {
    let (a, _srv) = amb_servidor();
    assert_eq!(
        envia_cli(&a, A, B, "PEDIDO", "r8-m", None, &[]).code,
        Some(0)
    );
    assert_eq!(pega(&a, B).code, Some(0));
    let r = envia_cli(&a, B, A, "RESPOSTA", "r8-r", Some("r8-m"), &[]);
    assert_eq!(r.code, Some(0), "{r:?}");
    let igual = confirma(&a, B, "r8-m", "resultado", Some("done"));
    assert_eq!(
        igual.code,
        Some(0),
        "repetir o mesmo resultado e idempotente: {igual:?}"
    );
    let diferente = confirma(&a, B, "r8-m", "resultado", Some("refused"));
    assert_eq!(
        diferente.code,
        Some(1),
        "resultado diferente e invalid_transition: {diferente:?}"
    );
    let msg = mensagem(&a, A, "r8-m");
    assert_eq!(msg["result"], "done");
    assert_eq!(
        msg["note"], "auto:r8-r",
        "o automatico nao pode ser sobrescrito"
    );
    assert_eq!(eventos_do_log(&a, "resulted", "r8-m").len(), 1);
}

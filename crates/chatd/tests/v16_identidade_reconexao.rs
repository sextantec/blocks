//! v1.6 (docs/spec-v1.6-identidade-e-reconexao.md): identidade explicita e fail-closed (Parte A, I1-I6)
//! e reconexao do cliente com erro de permissao distinto (Parte B, R1-R4). R5 (regressao das suites
//! v1.1 a v1.5 e da auditoria F) nao e um teste novo: e a execucao das suites existentes.
//! Escrito so a partir do contrato e dos modelos tests/common e tests/v15_cliente_resultado.rs; src/ nao foi lido.
//! Caixa preta: so o binario, o socket e os arquivos. Todo diretorio e temporario; HOME e XDG_CONFIG_HOME
//! apontam para dentro dele, e CHATD_CONFIG tambem quando ha arquivo de configuracao (com arquivo
//! ausente o contrato manda saida 2, C8 da v1.5). Nenhum processo real, nenhuma rede, nenhum token real.
//! Devem falhar ate a implementacao existir (e compilar com o binario atual).
//! AMBIGUO: marcado nos testes onde o contrato nao fixa o texto ou o formato lido.
#![allow(dead_code)]
mod common;

use common::*;
use serde_json::{Value, json};
use std::ffi::CString;
use std::fs::{self, File};
use std::io::{BufRead, BufReader, Write};
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::PermissionsExt;
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, mpsc};
use std::thread;
use std::time::{Duration, Instant};

const A: &str = "claude-main";
const B: &str = "codex-web";
const C: &str = "terceiro";
/// Marcador que o arquivo de configuracao carrega; nunca pode aparecer na saida de erro (I6).
const MARCA: &str = "SEGREDO-CONFIG-V16";
/// Aviso literal de I2 (rule 1 da Parte A): uma linha em stderr em cada execucao que encontra o campo.
const AVISO_DEFAULT: &str = "aviso: default_profile ignorado desde a v1.6; use --profile NOME";

/// Variaveis lidas pelo cliente e pelo servidor; removidas antes de cada execucao.
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

/// Grava `conteudo` em `p` com o modo dado (fixado depois, para ignorar a umask).
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

/// Cria um FIFO no caminho dado. Se o cliente abrir esse arquivo como token, bloqueia: o `roda` estoura e o teste falha.
fn fifo(p: &Path) {
    let c = CString::new(p.as_os_str().as_bytes()).unwrap();
    let r = unsafe { libc::mkfifo(c.as_ptr(), 0o600) };
    assert_eq!(r, 0, "mkfifo em {}", p.display());
}

/// Ambiente de qualquer processo do teste: variaveis do cliente removidas, HOME e XDG no temporario,
/// CHATD_CONFIG apontando para o arquivo do temporario quando ele existe.
fn ambiente(cmd: &mut Command, a: &Amb) {
    for v in VARS_LIMPAS {
        cmd.env_remove(v);
    }
    cmd.env("HOME", a.home()).env("XDG_CONFIG_HOME", a.xdg());
    if a.cfg_padrao().exists() {
        cmd.env("CHATD_CONFIG", a.cfg_padrao());
    }
}

/// Roda o chatd ate sair (limite 10 s). `envs` por cima do ambiente limpo. Saida em arquivos.
fn roda(a: &Amb, args: &[&str], envs: &[(&str, &str)]) -> Saida {
    let n = CONTADOR.fetch_add(1, Ordering::SeqCst);
    let out_p = a.p().join(format!("saida-{n}.out"));
    let err_p = a.p().join(format!("saida-{n}.err"));
    let mut cmd = Command::new(BIN);
    cmd.args(args);
    ambiente(&mut cmd, a);
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

/// Origem de `campo` no JSON do whoami: `sources.<campo>` (Esclarecimento 1 da v1.6). Valores: flag, env,
/// profile:NOME, ignored (default_profile) e unset (campo sem origem). Os fallbacks ficam por tolerancia.
fn origem(j: &Value, campo: &str) -> String {
    if let Some(o) = j["sources"][campo].as_str() {
        return o.to_string();
    }
    if let Some(o) = j[campo]["source"].as_str() {
        return o.to_string();
    }
    if let Some(o) = j["source"][campo].as_str() {
        return o.to_string();
    }
    panic!("origem de {campo} ausente em {j}");
}

/// Campo texto de um JSON; falha se ausente.
fn txt(j: &Value, campo: &str) -> String {
    j[campo]
        .as_str()
        .unwrap_or_else(|| panic!("campo {campo} ausente ou nao texto em {j}"))
        .to_string()
}

/// Saida da CLI como JSON (uma linha); falha com a saida crua se nao for JSON.
fn json_da_saida(r: &Saida) -> Value {
    serde_json::from_str(r.stdout.trim())
        .unwrap_or_else(|e| panic!("saida nao e JSON ({e}): {r:?}"))
}

/// Configuracao do cliente sem perfis: so socket e estado.
fn cfg_base(a: &Amb) -> Value {
    json!({
        "socket": a.socket().display().to_string(),
        "state_dir": a.state().display().to_string(),
    })
}

/// Configuracao com `default_profile: jabbur` (-> as=claude-main) e `codex` (-> as=codex-web).
fn cfg_jabbur(a: &Amb) -> Value {
    let mut c = cfg_base(a);
    c["default_profile"] = json!("jabbur");
    c["profiles"] = json!({
        "jabbur": {"as": A, "token_file": a.token(A).display().to_string()},
        "codex": {"as": B, "token_file": a.token(B).display().to_string()}
    });
    c
}

/// Configuracao so com o perfil `jabbur` (-> as=claude-main), sem `default_profile`.
fn cfg_so_jabbur(a: &Amb) -> Value {
    let mut c = cfg_base(a);
    c["profiles"] = json!({
        "jabbur": {"as": A, "token_file": a.token(A).display().to_string()}
    });
    c
}

/// Erro de uso esperado (saida 2) sem conectar: o socket da config e um ouvinte nosso, que nao pode aceitar conexao.
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

/// Emite o token de `nome` no estado de `a` (comando local, sem servidor), gravando em `a.token(nome)` (0600).
fn emite_token(a: &Amb, nome: &str) -> String {
    let out = a.token(nome);
    let st = a.state();
    fs::create_dir_all(&st).unwrap();
    let r = roda(
        a,
        &[
            "token",
            "new",
            nome,
            "--state-dir",
            s(&st),
            "--out",
            s(&out),
        ],
        &[],
    );
    assert_eq!(r.code, Some(0), "token new {nome}: {r:?}");
    fs::read_to_string(&out)
        .expect("ler arquivo do token")
        .trim()
        .to_string()
}

/// Configuracao do servidor: auth obrigatoria (padrao), campainha inerte, tres destinatarios por watch.
fn cfg_servidor() -> Value {
    json!({
        "herdr_bin": "/nonexistent/chatd-herdr-v16",
        "bell_poll_ms": 5000,
        "bell_rearm_s": 600,
        "recipients": {
            "claude-main": {"delivery": "watch"},
            "codex-web": {"delivery": "watch"},
            "terceiro": {"delivery": "watch"}
        }
    })
}

/// Servidor do chatd (processo real, ambiente limpo). Encerra no drop.
struct Srv {
    child: Child,
    out: PathBuf,
    err: PathBuf,
}

impl Srv {
    /// Sobe `serve --state-dir STATE --config CFG`; espera `chatd: ready` (10 s).
    fn sobe(a: &Amb, state: &Path, cfg: &Path, tag: &str) -> Srv {
        let out = a.p().join(format!("serve-{tag}.out"));
        let err = a.p().join(format!("serve-{tag}.err"));
        let mut cmd = Command::new(BIN);
        cmd.args(["serve", "--state-dir", s(state), "--config", s(cfg)]);
        ambiente(&mut cmd, a);
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

/// Servidor no estado `amb/state` e tokens de A, B e C em `amb/<nome>.tok`.
fn amb_servidor() -> (Amb, Srv) {
    let a = Amb::novo();
    let cfg = escreve(&a.p().join("srv.json"), &cfg_servidor().to_string(), 0o600);
    let srv = Srv::sobe(&a, &a.state(), &cfg, "principal");
    for n in [A, B, C] {
        emite_token(&a, n);
    }
    (a, srv)
}

/// `chatd watch --as QUEM` com stderr em arquivo; linhas de stdout lidas por thread. Espera o heartbeat.
struct Vigia {
    child: Child,
    linhas: mpsc::Receiver<String>,
}

impl Vigia {
    fn inicia(a: &Amb, quem: &str, err: &Path) -> Vigia {
        let hb = a.p().join(format!(
            "hb-{quem}-{}.txt",
            CONTADOR.fetch_add(1, Ordering::SeqCst)
        ));
        let sock = a.socket().display().to_string();
        let tok = a.token(quem).display().to_string();
        let mut cmd = Command::new(BIN);
        cmd.args(["watch", "--as", quem, "--heartbeat-file", s(&hb)]);
        ambiente(&mut cmd, a);
        cmd.env("CHATD_SOCKET", sock.as_str())
            .env("CHATD_TOKEN_FILE", tok.as_str());
        let mut child = cmd
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(File::create(err).unwrap())
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
        let v = Vigia { child, linhas: rx };
        assert!(
            espera(Duration::from_secs(5), || hb.exists()),
            "watch de {quem} nao conectou (sem heartbeat)"
        );
        v
    }

    /// Acumula linhas em `acum` ate que cada id de `ids` apareca em alguma linha, ou estoure `prazo`.
    /// AMBIGUO: o formato da linha de mensagem do `watch` nao e fixado no contrato; so se assume o id na linha.
    fn ate_ver(&self, acum: &mut Vec<String>, ids: &[&str], prazo: Duration) -> bool {
        let fim = Instant::now() + prazo;
        loop {
            if ids.iter().all(|i| acum.iter().any(|l| l.contains(i))) {
                return true;
            }
            let restante = fim.saturating_duration_since(Instant::now());
            if restante.is_zero() {
                return false;
            }
            match self.linhas.recv_timeout(restante) {
                Ok(l) => acum.push(l),
                Err(_) => return false,
            }
        }
    }
}

impl Drop for Vigia {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// `send` pela CLI como `de`, com a identidade de `de` pelo ambiente.
fn envia_cli(a: &Amb, de: &str, para: &str, kind: &str, id: &str) -> Saida {
    como(
        a,
        de,
        &[
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
        ],
    )
}

// ---------- daemon falso de socket (Parte B) ----------

/// Daemon falso: ouve no socket dado, depois de `atraso` (antes disso o socket nao existe: connect da ENOENT).
/// Responde `auth` e `message.send`. Com `corta_no_envio`, fecha a conexao ao ler a primeira `message.send`,
/// sem responder. Guarda o id de cada `message.send` recebida (uma por tentativa de envio que chegou).
/// AMBIGUO: o formato exato das respostas do protocolo nao e fixado no contrato; o falso devolve o minimo.
struct Falso {
    envios: Arc<Mutex<Vec<String>>>,
}

fn sobe_falso(socket: &Path, atraso: Duration, corta_no_envio: bool) -> Falso {
    let envios = Arc::new(Mutex::new(Vec::<String>::new()));
    let ouvinte = if atraso.is_zero() {
        Some(UnixListener::bind(socket).expect("bind do falso"))
    } else {
        None
    };
    let (env2, sock) = (Arc::clone(&envios), socket.to_path_buf());
    thread::spawn(move || {
        let ouvinte = match ouvinte {
            Some(o) => o,
            None => {
                thread::sleep(atraso);
                match UnixListener::bind(&sock) {
                    Ok(o) => o,
                    Err(_) => return, // o teste ja terminou e o diretorio foi apagado
                }
            }
        };
        for conn in ouvinte.incoming().map_while(Result::ok) {
            atende(conn, &env2, corta_no_envio);
        }
    });
    Falso { envios }
}

fn atende(conn: UnixStream, envios: &Arc<Mutex<Vec<String>>>, corta: bool) {
    let mut escrita = match conn.try_clone() {
        Ok(c) => c,
        Err(_) => return,
    };
    let leitor = BufReader::new(conn);
    for linha in leitor.lines().map_while(Result::ok) {
        let Ok(req) = serde_json::from_str::<Value>(&linha) else {
            return;
        };
        let id = req["id"].clone();
        let resp = match req["method"].as_str().unwrap_or("") {
            "auth" => json!({"ok": true}),
            "message.send" => {
                let mid = req["params"]["id"].as_str().unwrap_or("?").to_string();
                envios.lock().unwrap().push(mid.clone());
                if corta {
                    return;
                }
                json!({"id": mid, "seq": 1, "duplicate": false, "closed": null})
            }
            _ => json!({}),
        };
        let r = json!({"jsonrpc": "2.0", "id": id, "result": resp});
        if escrita.write_all(format!("{r}\n").as_bytes()).is_err() {
            return;
        }
    }
}

/// Restaura o modo de um diretorio ao sair do escopo (para o TempDir poder apagar).
struct Restaura(PathBuf);

impl Drop for Restaura {
    fn drop(&mut self) {
        let _ = fs::set_permissions(&self.0, fs::Permissions::from_mode(0o755));
    }
}

// =====================================================================
// Parte A: identidade explicita, sem perfil padrao
// =====================================================================

// ---------- I1: default_profile sem variaveis = saida 2, sem conectar e sem abrir token ----------

#[test]
fn i1_default_profile_sem_variaveis_sai_2_sem_conectar_e_sem_abrir_token() {
    let a = Amb::novo();
    // FIFO como arquivo de token: se o cliente abrir, bloqueia e o `roda` estoura (code None).
    let tok = a.p().join("token-fifo.tok");
    fifo(&tok);
    grava_cfg(
        &a,
        &json!({
            "socket": a.socket().display().to_string(),
            "state_dir": a.state().display().to_string(),
            "default_profile": "claude",
            "profiles": {"claude": {"as": A, "token_file": tok.display().to_string()}}
        }),
    );
    // Esclarecimento 12: status nao exige identidade; o fail-closed vale para inbox (e send, ack, show, watch, bind).
    let r = erro_sem_conectar(&a, &["inbox", "--json"], &[]);
    assert!(
        r.stderr.contains("identidade nao definida"),
        "mensagem de identidade ausente: {r:?}"
    );
    assert!(
        r.stderr.contains(AVISO_DEFAULT),
        "o aviso de default_profile deve sair junto: {r:?}"
    );
}

#[test]
fn i1b_send_sem_nenhuma_identidade_explicita_sai_2_sem_conectar() {
    let a = Amb::novo();
    let tok = a.p().join("token-fifo-send.tok");
    fifo(&tok);
    grava_cfg(
        &a,
        &json!({
            "socket": a.socket().display().to_string(),
            "state_dir": a.state().display().to_string(),
            "default_profile": "claude",
            "profiles": {"claude": {"as": A, "token_file": tok.display().to_string()}}
        }),
    );
    let r = erro_sem_conectar(
        &a,
        &[
            "send",
            "--to",
            B,
            "--kind",
            "AVISO",
            "--subject",
            "s",
            "--body",
            "c",
            "--id",
            "i1b-1",
        ],
        &[],
    );
    assert!(r.stderr.contains("identidade nao definida"), "{r:?}");
}

// ---------- I2: --profile explicito funciona e avisa que default_profile foi ignorado ----------

#[test]
fn i2_profile_explicito_funciona_e_avisa_que_default_profile_foi_ignorado() {
    let (a, _srv) = amb_servidor();
    grava_cfg(&a, &cfg_jabbur(&a));
    let r = roda(&a, &["status", "--profile", "jabbur"], &[]);
    assert_eq!(r.code, Some(0), "status com --profile jabbur: {r:?}");
    assert!(r.stderr.contains(AVISO_DEFAULT), "aviso ausente: {r:?}");
}

// ---------- I3: CHATD_AS + CHATD_TOKEN_FILE sem arquivo de configuracao (regressao v1.4) ----------

#[test]
fn i3_as_e_token_file_por_ambiente_sem_arquivo_funcionam() {
    let (a, _srv) = amb_servidor();
    assert!(
        !a.cfg_padrao().exists(),
        "o teste nao pode ter arquivo de configuracao"
    );
    let r = como(&a, A, &["status"]);
    assert_eq!(r.code, Some(0), "{r:?}");
    assert!(
        !r.stderr.contains("identidade nao definida"),
        "identidade do ambiente nao foi aceita: {r:?}"
    );
    let r2 = como(&a, A, &["inbox", "--json"]);
    assert_eq!(r2.code, Some(0), "inbox com identidade do ambiente: {r2:?}");
}

#[test]
fn i3b_identidade_do_ambiente_vence_o_default_profile_do_arquivo() {
    let (a, _srv) = amb_servidor();
    // default_profile aponta para o token de C; o ambiente traz A. A fonte (a) e a unica valida: vale A.
    grava_cfg(
        &a,
        &json!({
            "socket": a.socket().display().to_string(),
            "state_dir": a.state().display().to_string(),
            "default_profile": "jabbur",
            "profiles": {"jabbur": {"as": C, "token_file": a.token(C).display().to_string()}}
        }),
    );
    let w = como(&a, A, &["whoami", "--json"]);
    assert_eq!(w.code, Some(0), "{w:?}");
    let j = json_da_saida(&w);
    assert_eq!(
        txt(&j, "as"),
        A,
        "o ambiente (CHATD_AS) deve vencer o perfil padrao: {j}"
    );
    let r = como(&a, A, &["status"]);
    assert_eq!(r.code, Some(0), "{r:?}");
    assert!(
        r.stderr.contains(AVISO_DEFAULT),
        "o aviso continua valendo: {r:?}"
    );
}

// ---------- I4: whoami --json traz a origem de cada campo e nunca o token ----------

#[test]
fn i4_whoami_json_traz_origem_flag_por_campo_e_nunca_o_token() {
    let a = Amb::novo();
    fs::create_dir_all(a.state()).unwrap();
    let segredo = "TOKEN-DE-TESTE-V16-NAO-PODE-VAZAR";
    escreve(&a.token(A), segredo, 0o600);
    let sock = a.socket().display().to_string();
    let st = a.state().display().to_string();
    let tok = a.token(A).display().to_string();
    let (r, j) = whoami_json(
        &a,
        &[
            "--as",
            A,
            "--socket",
            sock.as_str(),
            "--state-dir",
            st.as_str(),
            "--token-file",
            tok.as_str(),
        ],
        &[],
    );
    assert_eq!(r.code, Some(0), "{r:?}");
    assert!(
        !r.stdout.contains(segredo) && !r.stderr.contains(segredo),
        "o token apareceu no whoami: {r:?}"
    );
    for campo in ["as", "token_file", "socket", "state_dir"] {
        assert_eq!(origem(&j, campo), "flag", "origem de {campo} em {j}");
    }
}

#[test]
fn i4b_whoami_json_marca_env_quando_a_identidade_vem_do_ambiente() {
    let a = Amb::novo();
    fs::create_dir_all(a.state()).unwrap();
    let segredo = "TOKEN-DE-TESTE-V16-AMBIENTE";
    escreve(&a.token(A), segredo, 0o600);
    let sock = a.socket().display().to_string();
    let st = a.state().display().to_string();
    let tok = a.token(A).display().to_string();
    let (r, j) = whoami_json(
        &a,
        &[],
        &[
            ("CHATD_SOCKET", sock.as_str()),
            ("CHATD_STATE_DIR", st.as_str()),
            ("CHATD_AS", A),
            ("CHATD_TOKEN_FILE", tok.as_str()),
        ],
    );
    assert_eq!(r.code, Some(0), "{r:?}");
    assert!(
        !r.stdout.contains(segredo) && !r.stderr.contains(segredo),
        "o token apareceu no whoami: {r:?}"
    );
    for campo in ["as", "token_file", "socket", "state_dir"] {
        assert_eq!(origem(&j, campo), "env", "origem de {campo} em {j}");
    }
}

// ---------- I5: CHATD_PROFILE herdado avisa no whoami e o cliente segue funcionando ----------

#[test]
fn i5_chatd_profile_herdado_avisa_no_whoami_e_o_cliente_segue_funcionando() {
    let (a, _srv) = amb_servidor();
    grava_cfg(&a, &cfg_so_jabbur(&a));
    // AMBIGUO: "identidade jabbur" lido como nome do perfil (CHATD_PROFILE=jabbur); o `as` fica claude-main.
    let (r, j) = whoami_json(&a, &[], &[("CHATD_PROFILE", "jabbur")]);
    assert_eq!(r.code, Some(0), "aviso nao pode virar erro: {r:?}");
    assert!(
        r.stderr
            .contains("aviso: identidade jabbur vem de CHATD_PROFILE herdado do ambiente"),
        "aviso de CHATD_PROFILE herdado ausente: {r:?}"
    );
    assert_eq!(
        txt(&j, "as"),
        A,
        "o perfil herdado ainda fornece a identidade: {j}"
    );
    let st = roda(&a, &["status"], &[("CHATD_PROFILE", "jabbur")]);
    assert_eq!(st.code, Some(0), "status com CHATD_PROFILE herdado: {st:?}");
}

#[test]
fn i5b_profile_explicito_na_linha_nao_gera_o_aviso_de_herdado() {
    let (a, _srv) = amb_servidor();
    grava_cfg(&a, &cfg_so_jabbur(&a));
    let r = roda(
        &a,
        &["whoami", "--profile", "jabbur"],
        &[("CHATD_PROFILE", "jabbur")],
    );
    assert_eq!(r.code, Some(0), "{r:?}");
    assert!(
        !r.stderr.contains("herdado do ambiente"),
        "com --profile na linha nao ha aviso de herdado: {r:?}"
    );
}

// ---------- I6: perfil inexistente e arquivo gravavel por grupo/outros continuam saida 2 ----------

#[test]
fn i6_perfil_inexistente_continua_saida_2_sem_conectar() {
    let a = Amb::novo();
    grava_cfg(&a, &cfg_jabbur(&a));
    let _ = erro_sem_conectar(&a, &["status", "--profile", "nao-existe"], &[]);
}

#[test]
fn i6_arquivo_gravavel_pelo_grupo_continua_saida_2_sem_conectar_nem_ecoar() {
    let a = Amb::novo();
    let mut cfg = cfg_jabbur(&a);
    cfg["state_dir"] = json!(format!("{}/{}", a.state().display(), MARCA));
    escreve(&a.cfg_padrao(), &cfg.to_string(), 0o620);
    let r = erro_sem_conectar(&a, &["status"], &[]);
    assert!(
        !r.stderr.contains(MARCA) && !r.stdout.contains(MARCA),
        "a mensagem de erro nao pode conter o conteudo do arquivo: {r:?}"
    );
}

#[test]
fn i6_arquivo_gravavel_por_outros_continua_saida_2_sem_conectar_nem_ecoar() {
    let a = Amb::novo();
    let mut cfg = cfg_jabbur(&a);
    cfg["state_dir"] = json!(format!("{}/{}", a.state().display(), MARCA));
    escreve(&a.cfg_padrao(), &cfg.to_string(), 0o602);
    let r = erro_sem_conectar(&a, &["status"], &[]);
    assert!(
        !r.stderr.contains(MARCA) && !r.stdout.contains(MARCA),
        "a mensagem de erro nao pode conter o conteudo do arquivo: {r:?}"
    );
}

// ---------- I7: whoami e diagnostico (sai 0, as nulo); status sem identidade sai 2 ----------

#[test]
fn i7_sem_variaveis_com_default_profile_whoami_sai_0_com_as_nulo_e_status_sai_2() {
    let a = Amb::novo();
    grava_cfg(&a, &cfg_jabbur(&a));
    let (r, j) = whoami_json(&a, &[], &[]);
    assert_eq!(r.code, Some(0), "whoami e diagnostico, nao falha sem identidade: {r:?}");
    assert!(j["as"].is_null(), "as deve ser null: {j}");
    assert!(j["profile"].is_null(), "profile null sem perfil escolhido: {j}");
    assert_eq!(origem(&j, "as"), "unset", "sources.as deve ser unset: {j}");
    assert!(
        r.stderr.contains(AVISO_DEFAULT),
        "aviso de default_profile em stderr: {r:?}"
    );
    // status nao exige identidade (Esclarecimento 12); o fail-closed de um comando de caixa e o inbox.
    let _ = erro_sem_conectar(&a, &["inbox", "--json"], &[]);
}

// ---------- I9: status sem identidade conecta sem autenticar (Esclarecimento 12); default_profile nao abre token ----------

#[test]
fn i9_status_sem_identidade_contra_auth_off_sai_0_e_nao_abre_o_token() {
    let a = Amb::novo();
    fs::create_dir_all(a.state()).unwrap();
    let cfg = escreve(&a.p().join("srv-off-i9.json"), &cfg_sem_auth().to_string(), 0o600);
    let _srv = Srv::sobe(&a, &a.state(), &cfg, "auth-off-i9");
    // default_profile com token em FIFO: se o cliente abrir o token, bloqueia e o roda estoura (code None).
    let tok = a.p().join("token-fifo-i9.tok");
    fifo(&tok);
    grava_cfg(
        &a,
        &json!({
            "socket": a.socket().display().to_string(),
            "state_dir": a.state().display().to_string(),
            "default_profile": "jabbur",
            "profiles": {"jabbur": {"as": C, "token_file": tok.display().to_string()}}
        }),
    );
    let sock = a.socket().display().to_string();
    let r = roda(&a, &["status"], &[("CHATD_SOCKET", sock.as_str())]);
    assert_eq!(r.code, Some(0), "status sem identidade contra auth off: {r:?}");
    assert!(r.stderr.contains(AVISO_DEFAULT), "o aviso de default_profile continua: {r:?}");
}

// ---------- I8: identidade explicita (--from) sem token funciona com daemon auth off; token nao e aberto ----------

/// Configuracao do servidor com `auth: off` (suites v1.1 a v1.5 usam esse modo sem token).
fn cfg_sem_auth() -> Value {
    json!({
        "auth": "off",
        "herdr_bin": "/nonexistent/chatd-herdr-v16",
        "bell_poll_ms": 5000,
        "bell_rearm_s": 600,
        "recipients": {
            "claude-main": {"delivery": "watch"},
            "codex-web": {"delivery": "watch"}
        }
    })
}

#[test]
fn i8_identidade_explicita_sem_token_funciona_com_auth_off_e_nao_abre_o_token() {
    let a = Amb::novo();
    fs::create_dir_all(a.state()).unwrap();
    let cfg = escreve(&a.p().join("srv-off.json"), &cfg_sem_auth().to_string(), 0o600);
    let _srv = Srv::sobe(&a, &a.state(), &cfg, "auth-off");
    // default_profile com token em FIFO: se o cliente abrir o token, bloqueia e o roda estoura (code None).
    let tok = a.p().join("token-fifo-i8.tok");
    fifo(&tok);
    grava_cfg(
        &a,
        &json!({
            "socket": a.socket().display().to_string(),
            "state_dir": a.state().display().to_string(),
            "default_profile": "jabbur",
            "profiles": {"jabbur": {"as": C, "token_file": tok.display().to_string()}}
        }),
    );
    let sock = a.socket().display().to_string();
    let envs = [("CHATD_SOCKET", sock.as_str())];
    let r = roda(
        &a,
        &[
            "send", "--from", A, "--to", B, "--kind", "AVISO", "--subject", "s", "--body", "c",
            "--id", "i8-1",
        ],
        &envs,
    );
    assert_eq!(r.code, Some(0), "--from explicito com auth off e sem token: {r:?}");
    assert!(r.stderr.contains(AVISO_DEFAULT), "o aviso continua valendo: {r:?}");
    let i = roda(&a, &["inbox", "--json", "--as", B], &envs);
    assert_eq!(i.code, Some(0), "inbox --as sem token com auth off: {i:?}");
}

// =====================================================================
// Parte B: reconexao e erro de permissao distinto
// =====================================================================

// ---------- R1: daemon que so aceita na 3a tentativa; a mensagem entra uma vez ----------

#[test]
fn r1_daemon_que_aparece_na_terceira_tentativa_recebe_uma_unica_message_send() {
    let a = Amb::novo();
    fs::create_dir_all(a.state()).unwrap();
    emite_token(&a, A);
    // Socket ausente na 1a e na 2a tentativa (ENOENT); passa a ouvir em ~250 ms. As tentativas caem em
    // ~0, ~100 e ~400 ms: a 3a conecta. Margem de ~150 ms de cada lado.
    let falso = sobe_falso(&a.socket(), Duration::from_millis(250), false);
    let r = envia_cli(&a, A, B, "AVISO", "r16-1");
    assert_eq!(
        r.code,
        Some(0),
        "send deveria concluir na 3a tentativa: {r:?}"
    );
    let envios = falso.envios.lock().unwrap().clone();
    assert_eq!(
        envios,
        vec!["r16-1".to_string()],
        "uma unica message.send, sem duplicata"
    );
}

#[test]
fn r1b_queda_depois_do_envio_nao_repete_a_mensagem() {
    let a = Amb::novo();
    fs::create_dir_all(a.state()).unwrap();
    emite_token(&a, A);
    // Daemon que le a message.send e fecha a conexao sem responder. Se o cliente repetisse, a 2a chegaria ao falso.
    // Esclarecimento 3: sai 4 e imprime o id.
    let falso = sobe_falso(&a.socket(), Duration::ZERO, true);
    let r = envia_cli(&a, A, B, "AVISO", "r16-2");
    let envios = falso.envios.lock().unwrap().clone();
    assert_eq!(
        envios,
        vec!["r16-2".to_string()],
        "a requisicao ja enviada nao pode ser repetida"
    );
    assert_eq!(r.code, Some(4), "queda apos o envio: sai 4 sem repetir: {r:?}");
    assert!(
        format!("{}{}", r.stdout, r.stderr).contains("r16-2"),
        "o id da mensagem deve sair na saida (Esclarecimento 3): {r:?}"
    );
}

// ---------- R2: socket inexistente: 4 tentativas, tempo entre 1,2 e 3 s, saida 4 ----------

#[test]
fn r2_socket_inexistente_leva_entre_1_2_e_3_s_e_sai_4() {
    let a = Amb::novo();
    fs::create_dir_all(a.state()).unwrap();
    emite_token(&a, A);
    // Nenhum daemon: connect devolve ENOENT em toda tentativa. Esclarecimento 4: tempo total >= 1,2 s e <= 3 s.
    let t = Instant::now();
    let r = como(&a, A, &["status"]);
    let el = t.elapsed();
    assert_eq!(r.code, Some(4), "daemon indisponivel = saida 4: {r:?}");
    assert!(
        el >= Duration::from_millis(1200),
        "saiu cedo demais ({el:?}): faltaram as esperas de reconexao"
    );
    assert!(el <= Duration::from_secs(3), "esperou demais: {el:?}");
}

#[test]
fn r2b_socket_obsoleto_sem_ouvinte_recusa_a_conexao_e_sai_4() {
    let a = Amb::novo();
    fs::create_dir_all(a.state()).unwrap();
    emite_token(&a, A);
    // Arquivo de socket sem ouvinte (o Rust nao apaga o arquivo ao soltar o UnixListener): connect da ECONNREFUSED.
    {
        let _l = UnixListener::bind(a.socket()).expect("criar socket obsoleto");
    }
    let t = Instant::now();
    let r = como(&a, A, &["status"]);
    let el = t.elapsed();
    assert_eq!(r.code, Some(4), "ECONNREFUSED tambem e daemon fora: {r:?}");
    assert!(el >= Duration::from_millis(1200), "saiu cedo demais: {el:?}");
    assert!(el <= Duration::from_secs(3), "esperou demais: {el:?}");
}

/// Conta as chamadas connect() do cliente ao socket, pelo strace. Esclarecimento 4: "contadas pelo falso";
/// um falso em Rust nao ve tentativas recusadas (ENOENT/ECONNREFUSED), entao a contagem e feita pelo strace.
#[test]
#[ignore = "requer strace e ptrace permitido; conta as 4 chamadas connect (Esclarecimento 4). Rodar com --ignored"]
fn r2c_conta_quatro_connects_com_strace() {
    let a = Amb::novo();
    fs::create_dir_all(a.state()).unwrap();
    emite_token(&a, A);
    let trace = a.p().join("connect.trace");
    let sock = a.socket().display().to_string();
    let tok = a.token(A).display().to_string();
    let mut cmd = Command::new("strace");
    cmd.args(["-f", "-e", "trace=connect", "-o", s(&trace), BIN, "status"]);
    ambiente(&mut cmd, &a);
    cmd.env("CHATD_SOCKET", sock.as_str())
        .env("CHATD_AS", A)
        .env("CHATD_TOKEN_FILE", tok.as_str());
    let _ = cmd
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .expect("executar strace");
    let t = fs::read_to_string(&trace).unwrap_or_default();
    let n = t
        .lines()
        .filter(|l| l.contains(&format!("sun_path=\"{sock}\"")))
        .count();
    assert_eq!(n, 4, "esperava 4 connects (1 + 3 repeticoes): {t}");
}

// ---------- R3: EACCES no connect sai 5, uma tentativa, sem caminho do token ----------

#[test]
fn r3_eacces_no_connect_sai_5_sem_repetir_e_sem_caminho_do_token() {
    let a = Amb::novo();
    fs::create_dir_all(a.state()).unwrap();
    emite_token(&a, A);
    // Diretorio sem permissao de busca: connect em dir/chatd.sock devolve EACCES (simula a negacao do ambiente).
    // Se o processo rodar como root, a permissao nao vale e o teste falha: nao se aplica a root.
    let dir = a.p().join("sem-busca");
    fs::create_dir_all(&dir).unwrap();
    let sock = dir.join("chatd.sock");
    fs::set_permissions(&dir, fs::Permissions::from_mode(0o000)).unwrap();
    let _restaura = Restaura(dir.clone());
    let tok = a.token(A).display().to_string();
    let t = Instant::now();
    let r = roda(
        &a,
        &["status"],
        &[
            ("CHATD_SOCKET", s(&sock)),
            ("CHATD_AS", A),
            ("CHATD_TOKEN_FILE", tok.as_str()),
        ],
    );
    let el = t.elapsed();
    assert_eq!(r.code, Some(5), "permissao negada = saida 5: {r:?}");
    assert!(
        r.stderr.contains("permissao negada ao socket"),
        "mensagem de permissao ausente: {r:?}"
    );
    assert!(
        r.stderr.contains("nao e queda do daemon"),
        "a mensagem deve dizer que nao e queda do daemon: {r:?}"
    );
    assert!(
        !r.stderr.contains("claude-main.tok") && !r.stdout.contains("claude-main.tok"),
        "a mensagem nao pode trazer o caminho do token: {r:?}"
    );
    assert!(
        el < Duration::from_millis(1000),
        "EACCES nao pode repetir (as esperas somam 1,3 s): {el:?}"
    );
}

#[test]
#[ignore = "EPERM nao simulavel: o common nao oferece falso de EPERM e sem seccomp/LD_PRELOAD nao se forca; a regra (sem repeticao, saida 5) esta coberta pelo EACCES do r3"]
fn r3_eperm_no_connect_sai_5_como_eacces() {
    panic!("EPERM nao simulavel sem seccomp/LD_PRELOAD");
}

// ---------- R4: watch reconecta apos reinicio do daemon, sem perder nem repetir ----------

#[test]
fn r4_watch_reconecta_apos_reinicio_do_daemon_sem_perder_nem_repetir() {
    let a = Amb::novo();
    let cfg = escreve(&a.p().join("srv.json"), &cfg_servidor().to_string(), 0o600);
    let mut srv = Srv::sobe(&a, &a.state(), &cfg, "r4-1");
    emite_token(&a, A);
    emite_token(&a, B);
    let err_w = a.p().join("vigia-r4.err");
    let vigia = Vigia::inicia(&a, A, &err_w);
    let antes = ["r16-r4-1", "r16-r4-2", "r16-r4-3"];
    for id in antes {
        let r = envia_cli(&a, B, A, "AVISO", id);
        assert_eq!(r.code, Some(0), "envio antes do reinicio: {r:?}");
    }
    let mut visto: Vec<String> = Vec::new();
    assert!(
        vigia.ate_ver(&mut visto, &antes, Duration::from_secs(10)),
        "watch nao entregou antes do reinicio: {visto:?}"
    );
    assert_eq!(srv.para(), Some(0), "SIGTERM deve encerrar o daemon limpo");
    let _srv2 = Srv::sobe(&a, &a.state(), &cfg, "r4-2");
    let depois = ["r16-r4-4", "r16-r4-5", "r16-r4-6"];
    for id in depois {
        let r = envia_cli(&a, B, A, "AVISO", id);
        assert_eq!(r.code, Some(0), "envio depois do reinicio: {r:?}");
    }
    let todos: Vec<&str> = antes.iter().chain(depois.iter()).copied().collect();
    assert!(
        vigia.ate_ver(&mut visto, &todos, Duration::from_secs(15)),
        "watch nao recebeu tudo apos o reinicio: {visto:?}"
    );
    // Cada id aparece exatamente uma vez, na ordem de envio.
    let posicoes: Vec<usize> = todos
        .iter()
        .map(|id| {
            let p: Vec<usize> = visto
                .iter()
                .enumerate()
                .filter(|(_, l)| l.contains(id))
                .map(|(i, _)| i)
                .collect();
            assert_eq!(p.len(), 1, "{id} deveria aparecer uma vez: {visto:?}");
            p[0]
        })
        .collect();
    assert!(
        posicoes.windows(2).all(|w| w[0] < w[1]),
        "ordem de entrega errada: {visto:?}"
    );
    // seq contiguo no log. AMBIGUO: assume o campo `seq` nas linhas de log.jsonl (o contrato so fala em seq).
    let seqs: Vec<i64> = log_linhas(&a.state())
        .iter()
        .filter_map(|l| serde_json::from_str::<Value>(l).ok())
        .filter_map(|v| v["seq"].as_i64())
        .collect();
    assert!(seqs.len() >= 6, "log com seq insuficiente: {seqs:?}");
    assert!(
        seqs.windows(2).all(|w| w[1] == w[0] + 1),
        "seq do log nao contiguo: {seqs:?}"
    );
    // Esclarecimento 5: uma linha em stderr que comeca com "aviso: reconectando".
    let err = fs::read_to_string(&err_w).unwrap_or_default();
    assert!(
        err.lines().any(|l| l.starts_with("aviso: reconectando")),
        "esperava linha 'aviso: reconectando' em stderr do watch: {err:?}"
    );
}

// ---------- R6: backoff do watch (Esclarecimento 6) ----------

/// Derrubador: aceita e fecha cada conexao logo em seguida, o que faz o `watch` reconectar. Guarda o instante de cada accept.
fn sobe_derrubador(socket: &Path) -> Arc<Mutex<Vec<Instant>>> {
    let ouvinte = UnixListener::bind(socket).expect("bind do derrubador");
    let marcas = Arc::new(Mutex::new(Vec::<Instant>::new()));
    let m2 = Arc::clone(&marcas);
    thread::spawn(move || {
        for conn in ouvinte.incoming().map_while(Result::ok) {
            m2.lock().unwrap().push(Instant::now());
            drop(conn);
        }
    });
    marcas
}

/// Roda `watch --as A` + `extra` por `dur` contra o derrubador. Devolve os intervalos (s) entre accepts seguidos.
/// AMBIGUO: os intervalos medidos incluem a latencia do processo; por isso as tolerancias de 0,05 s e 0,5 s.
fn intervalos_de_reconexao(a: &Amb, extra: &[&str], dur: Duration) -> Vec<f64> {
    fs::create_dir_all(a.state()).unwrap();
    emite_token(a, A);
    let marcas = sobe_derrubador(&a.socket());
    let err_p = a.p().join(format!(
        "watch-r6-{}.err",
        CONTADOR.fetch_add(1, Ordering::SeqCst)
    ));
    let sock = a.socket().display().to_string();
    let tok = a.token(A).display().to_string();
    let mut cmd = Command::new(BIN);
    cmd.args(["watch", "--as", A]).args(extra);
    ambiente(&mut cmd, a);
    cmd.env("CHATD_SOCKET", sock.as_str())
        .env("CHATD_TOKEN_FILE", tok.as_str());
    let mut child = cmd
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(File::create(&err_p).unwrap())
        .spawn()
        .expect("executar watch");
    thread::sleep(dur);
    let _ = child.kill();
    let _ = child.wait();
    let m = marcas.lock().unwrap().clone();
    m.windows(2)
        .map(|w| w[1].duration_since(w[0]).as_secs_f64())
        .collect()
}

#[test]
fn r6a_watch_sem_reconnect_primeiro_retry_ate_1s_e_nenhum_intervalo_acima_de_5s() {
    let a = Amb::novo();
    // Backoff 200 ms a 5 s; 12 s de observacao cobrem o teto.
    let g = intervalos_de_reconexao(&a, &[], Duration::from_secs(12));
    assert!(g.len() >= 3, "o watch deveria reconectar varias vezes: {g:?}");
    assert!(g[0] <= 1.0 + 0.05, "primeiro retry sem --reconnect deve ser <= 1 s: {g:?}");
    assert!(
        g.iter().all(|&x| x <= 5.0 + 0.5),
        "nenhum intervalo acima de 5 s: {g:?}"
    );
}

#[test]
fn r6b_watch_com_reconnect_segue_a_v12_primeiro_1s_depois_2s() {
    let a = Amb::novo();
    let g = intervalos_de_reconexao(&a, &["--reconnect"], Duration::from_secs(8));
    assert!(g.len() >= 2, "o watch com --reconnect deveria reconectar: {g:?}");
    assert!(g[0] >= 1.0 - 0.05, "primeiro retry com --reconnect >= 1 s: {g:?}");
    assert!(g[1] >= 2.0 - 0.05, "segundo retry com --reconnect >= 2 s: {g:?}");
}

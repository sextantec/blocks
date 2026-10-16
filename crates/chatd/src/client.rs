//! Subcomandos do cliente: `send`, `inbox`, `ack`, `status`, `watch` e `log` (spec v1 4 e v1.1 D).

use std::collections::{BTreeMap, HashMap};
use std::fs::{self, File};
use std::io::{self, Read, Write};
use std::ops::Deref;
use std::os::unix::fs::{MetadataExt, OpenOptionsExt, PermissionsExt};
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use serde_json::{Map, Value, json};

use crate::protocol::valid_name;
use crate::{Failure, Opts, parse_opts, usage};

const SOCKET_OPTS: [&str; 3] = ["socket", "state-dir", "token-file"];
const RPC_DEADLINE: Duration = Duration::from_secs(30);
const KEEPALIVE: Duration = Duration::from_secs(15);
const HEARTBEAT: Duration = Duration::from_secs(5);
const BIND_KEEPALIVE: Duration = Duration::from_secs(25);
const DEFAULT_PROFILE_WARNING: &str =
    "aviso: default_profile ignorado desde a v1.6; use --profile NOME";
const INHERITED_JABBUR_WARNING: &str =
    "aviso: identidade jabbur vem de CHATD_PROFILE herdado do ambiente";
const IDENTITY_UNDEFINED: &str =
    "identidade nao definida: informe --profile NOME ou CHATD_AS e CHATD_TOKEN_FILE";

#[derive(Debug, Default, serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct ClientFile {
    socket: Option<String>,
    state_dir: Option<String>,
    default_profile: Option<String>,
    profiles: Option<BTreeMap<String, Profile>>,
}

#[derive(Debug, serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct Profile {
    #[serde(rename = "as")]
    identity: String,
    token_file: String,
}

struct ClientOpts {
    raw: Opts,
    file: ClientFile,
    profile: Option<String>,
    profile_source: &'static str,
}

impl Deref for ClientOpts {
    type Target = Opts;

    fn deref(&self) -> &Self::Target {
        &self.raw
    }
}

impl ClientOpts {
    fn profile(&self) -> Option<&Profile> {
        self.profile
            .as_deref()
            .and_then(|name| self.file.profiles.as_ref()?.get(name))
    }
}

fn client_opts(args: &[String], values: &[&str], bools: &[&str]) -> Result<ClientOpts, Failure> {
    let mut values = values.to_vec();
    values.push("profile");
    let raw = parse_opts(args, &values, bools)?;
    let file = load_client_file()?;
    if file.default_profile.is_some() {
        eprintln!("{DEFAULT_PROFILE_WARNING}");
    }
    let (profile, profile_source) = if let Some(name) = raw.get("profile") {
        (Some(name.to_string()), "flag")
    } else if let Some(name) = env_value("CHATD_PROFILE") {
        (Some(name), "env")
    } else {
        (
            None,
            if file.default_profile.is_some() {
                "ignored"
            } else {
                "unset"
            },
        )
    };
    if let Some(name) = &profile {
        if !valid_name(name) {
            return Err(usage("nome de perfil invalido"));
        }
        if file
            .profiles
            .as_ref()
            .and_then(|profiles| profiles.get(name))
            .is_none()
        {
            return Err(usage(format!(
                "perfil '{name}' nao existe na configuracao do cliente"
            )));
        }
    }
    Ok(ClientOpts {
        raw,
        file,
        profile,
        profile_source,
    })
}

fn load_client_file() -> Result<ClientFile, Failure> {
    let explicit = std::env::var_os("CHATD_CONFIG");
    let explicit_set = explicit.is_some();
    let path = if let Some(path) = explicit.as_ref() {
        if path.is_empty() {
            return Err(usage("CHATD_CONFIG vazio"));
        }
        Some(PathBuf::from(path))
    } else if let Some(root) = std::env::var_os("XDG_CONFIG_HOME").filter(|v| !v.is_empty()) {
        Some(PathBuf::from(root).join("chatd/client.json"))
    } else {
        std::env::var_os("HOME")
            .filter(|v| !v.is_empty())
            .map(|home| PathBuf::from(home).join(".config/chatd/client.json"))
    };
    let Some(path) = path else {
        return Ok(ClientFile::default());
    };
    let metadata = match fs::symlink_metadata(&path) {
        Ok(meta) => meta,
        Err(e) if e.kind() == io::ErrorKind::NotFound && !explicit_set => {
            return Ok(ClientFile::default());
        }
        Err(_) => return Err(usage("nao foi possivel ler a configuracao do cliente")),
    };
    // SAFETY: geteuid does not dereference pointers or change process state.
    let uid = unsafe { libc::geteuid() };
    if !metadata.file_type().is_file()
        || metadata.uid() != uid
        || metadata.permissions().mode() & 0o022 != 0
    {
        return Err(usage(
            "configuracao do cliente deve ser arquivo regular, do usuario e nao gravavel por grupo/outros",
        ));
    }
    let text = fs::read_to_string(&path)
        .map_err(|_| usage("nao foi possivel ler a configuracao do cliente"))?;
    let file: ClientFile =
        serde_json::from_str(&text).map_err(|_| usage("configuracao do cliente invalida"))?;
    validate_client_file(&file)?;
    Ok(file)
}

fn validate_client_file(file: &ClientFile) -> Result<(), Failure> {
    for path in [file.socket.as_deref(), file.state_dir.as_deref()]
        .into_iter()
        .flatten()
    {
        if !valid_config_path(path) {
            return Err(usage("caminho invalido na configuracao do cliente"));
        }
    }
    if let Some(name) = &file.default_profile
        && (!valid_name(name)
            || file
                .profiles
                .as_ref()
                .and_then(|profiles| profiles.get(name))
                .is_none())
    {
        return Err(usage("default_profile inexistente ou invalido"));
    }
    if let Some(profiles) = &file.profiles {
        for (name, profile) in profiles {
            if !valid_name(name)
                || !valid_name(&profile.identity)
                || !valid_config_path(&profile.token_file)
            {
                return Err(usage("perfil invalido na configuracao do cliente"));
            }
        }
    }
    Ok(())
}

fn valid_config_path(value: &str) -> bool {
    !value.is_empty() && !value.contains('\0')
}

fn env_value(name: &str) -> Option<String> {
    std::env::var(name).ok().filter(|value| !value.is_empty())
}

fn expand_home(value: &str) -> PathBuf {
    if let Some(rest) = value.strip_prefix("~/")
        && let Some(home) = std::env::var_os("HOME").filter(|v| !v.is_empty())
    {
        return PathBuf::from(home).join(rest);
    }
    PathBuf::from(value)
}

/// Erro de uma chamada: falha de transporte ou erro devolvido pelo servidor.
pub enum Rpc {
    Io(String),
    Connect {
        kind: io::ErrorKind,
        message: String,
    },
    ConnectDenied,
    Unavailable(String),
    NoResponse(String),
    Server {
        code: String,
        message: String,
    },
    /// `unauthenticated` recebido por um cliente que nao tem token configurado (spec D: saida 2).
    NoToken,
}

fn rpc_fail(e: Rpc) -> Failure {
    rpc_failure(e, false)
}

fn rpc_failure(e: Rpc, for_bind: bool) -> Failure {
    match e {
        Rpc::ConnectDenied => Failure::PermissionDenied(
            "permissao negada ao socket: o ambiente do processo nao permite conectar (sandbox ou permissoes); nao e queda do daemon".into(),
        ),
        Rpc::Unavailable(m) | Rpc::NoResponse(m) => Failure::Unavailable(m),
        Rpc::Connect { message, .. } if for_bind => Failure::Unavailable(message),
        Rpc::Connect { message, .. } => Failure::Run(message),
        Rpc::Io(m) if for_bind => Failure::Unavailable(m),
        Rpc::Io(m) => Failure::Run(m),
        Rpc::Server { code, message } if for_bind && code == "forbidden" => {
            Failure::BindForbidden(format!("{code}: {message}"))
        }
        Rpc::Server { code, message } => Failure::Run(format!("{code}: {message}")),
        Rpc::NoToken => Failure::Bad(NO_TOKEN_MSG.to_string()),
    }
}

const NO_TOKEN_MSG: &str = "token ausente: defina CHATD_TOKEN_FILE ou --token-file";

/// Conexao JSON-RPC por linha. Notificacoes (`method`) sao ignoradas pelas chamadas comuns.
pub struct Session {
    stream: UnixStream,
    buf: Vec<u8>,
    next_id: u64,
    has_token: bool,
}

impl Session {
    fn connect(socket: &Path) -> Result<Session, Rpc> {
        const RETRIES_MS: [u64; 3] = [100, 300, 900];
        for attempt in 0..=RETRIES_MS.len() {
            if attempt > 0 {
                std::thread::sleep(Duration::from_millis(RETRIES_MS[attempt - 1]));
            }
            match Self::connect_once(socket) {
                Ok(session) => return Ok(session),
                Err(Rpc::Connect {
                    kind: io::ErrorKind::NotFound | io::ErrorKind::ConnectionRefused,
                    message,
                }) if attempt < RETRIES_MS.len() => {
                    let _ = message;
                }
                Err(Rpc::Connect {
                    kind: io::ErrorKind::NotFound | io::ErrorKind::ConnectionRefused,
                    message,
                }) => {
                    return Err(Rpc::Unavailable(message));
                }
                Err(error) => return Err(error),
            }
        }
        unreachable!("the final connect attempt returns or exits")
    }

    fn connect_once(socket: &Path) -> Result<Session, Rpc> {
        let stream = UnixStream::connect(socket).map_err(|e| {
            if matches!(e.raw_os_error(), Some(libc::EACCES) | Some(libc::EPERM)) {
                Rpc::ConnectDenied
            } else {
                Rpc::Connect {
                    kind: e.kind(),
                    message: format!("nao conectou em {}: {e}", socket.display()),
                }
            }
        })?;
        let _ = stream.set_read_timeout(Some(Duration::from_secs(1)));
        let _ = stream.set_write_timeout(Some(Duration::from_secs(5)));
        Ok(Session {
            stream,
            buf: Vec::new(),
            next_id: 1,
            has_token: false,
        })
    }

    /// Le um JSON por linha. `Ok(None)` = tempo de leitura esgotado (sem perder bytes parciais).
    fn next_value(&mut self) -> io::Result<Option<Value>> {
        loop {
            if let Some(pos) = self.buf.iter().position(|&b| b == b'\n') {
                let line: Vec<u8> = self.buf.drain(..=pos).collect();
                return serde_json::from_slice(&line[..line.len() - 1])
                    .map(Some)
                    .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e));
            }
            let mut chunk = [0u8; 4096];
            match self.stream.read(&mut chunk) {
                Ok(0) => {
                    return Err(io::Error::new(
                        io::ErrorKind::UnexpectedEof,
                        "conexao fechada",
                    ));
                }
                Ok(n) => self.buf.extend_from_slice(&chunk[..n]),
                Err(e)
                    if matches!(
                        e.kind(),
                        io::ErrorKind::WouldBlock
                            | io::ErrorKind::TimedOut
                            | io::ErrorKind::Interrupted
                    ) =>
                {
                    return Ok(None);
                }
                Err(e) => return Err(e),
            }
        }
    }

    fn write_req(&mut self, id: Value, method: &str, params: Value) -> Result<(), Rpc> {
        let req = json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params});
        let mut bytes = serde_json::to_vec(&req).map_err(|e| Rpc::Io(e.to_string()))?;
        bytes.push(b'\n');
        self.stream
            .write_all(&bytes)
            .map_err(|e| Rpc::Io(format!("falha ao enviar: {e}")))
    }

    fn call(&mut self, method: &str, params: Value) -> Result<Value, Rpc> {
        let id = self.next_id;
        self.next_id += 1;
        self.write_req(json!(id), method, params)?;
        let started = Instant::now();
        loop {
            if started.elapsed() > RPC_DEADLINE {
                return Err(Rpc::NoResponse(
                    "servidor nao respondeu a tempo".to_string(),
                ));
            }
            let v = match self.next_value() {
                Ok(Some(v)) => v,
                Ok(None) => continue,
                Err(e) => return Err(Rpc::NoResponse(format!("falha ao ler resposta: {e}"))),
            };
            if v.get("method").is_some() || v.get("id") != Some(&json!(id)) {
                continue;
            }
            return match v.get("error") {
                Some(err)
                    if !self.has_token
                        && err.get("code").and_then(Value::as_str) == Some("unauthenticated") =>
                {
                    Err(Rpc::NoToken)
                }
                Some(err) => Err(Rpc::Server {
                    code: err
                        .get("code")
                        .and_then(Value::as_str)
                        .unwrap_or("internal_error")
                        .to_string(),
                    message: err
                        .get("message")
                        .and_then(Value::as_str)
                        .unwrap_or("")
                        .to_string(),
                }),
                None => Ok(v.get("result").cloned().unwrap_or(Value::Null)),
            };
        }
    }
}

/// Resolve o socket: --socket, CHATD_SOCKET, depois <state-dir>/chatd.sock (flag ou CHATD_STATE_DIR).
fn socket_path(o: &ClientOpts) -> Result<PathBuf, Failure> {
    if let Some(s) = o.get("socket") {
        return Ok(expand_home(s));
    }
    if let Some(s) = env_value("CHATD_SOCKET") {
        return Ok(expand_home(&s));
    }
    if let Some(s) = o.file.socket.as_deref() {
        return Ok(expand_home(s));
    }
    let dir = o
        .get("state-dir")
        .map(expand_home)
        .or_else(|| env_value("CHATD_STATE_DIR").map(|s| expand_home(&s)))
        .or_else(|| o.file.state_dir.as_deref().map(expand_home));
    match dir {
        Some(d) => Ok(d.join("chatd.sock")),
        None => Err(usage(
            "informe --socket, CHATD_SOCKET, --state-dir ou CHATD_STATE_DIR",
        )),
    }
}

/// Identidade: a flag vence `CHATD_AS`. Nome fora do formato e erro de uso.
fn identity(o: &ClientOpts, flag: &str) -> Result<Option<String>, Failure> {
    let raw = o
        .get(flag)
        .map(str::to_string)
        .or_else(|| env_value("CHATD_AS"))
        .or_else(|| o.profile().map(|p| p.identity.clone()));
    match raw {
        Some(n) if valid_name(&n) => Ok(Some(n)),
        Some(_) => Err(usage("nome de agente invalido")),
        None => Ok(None),
    }
}

/// Nome usado no `auth`: a credencial e a do `CHATD_AS`. `--as`/`--from` diferente dela chega
/// ao servidor, que responde `forbidden` (identidade imposta pela conexao, v1.1 B).
fn auth_name(name: Option<&str>) -> Option<String> {
    std::env::var("CHATD_AS")
        .ok()
        .filter(|s| !s.is_empty())
        .or_else(|| name.map(str::to_string))
}

/// Token: --token-file vence CHATD_TOKEN_FILE. O conteudo nunca e impresso.
fn token(o: &ClientOpts) -> Result<Option<String>, Failure> {
    let path = o
        .get("token-file")
        .map(expand_home)
        .or_else(|| env_value("CHATD_TOKEN_FILE").map(|s| expand_home(&s)))
        .or_else(|| o.profile().map(|p| expand_home(&p.token_file)));
    let Some(path) = path else {
        return Ok(None);
    };
    let text = fs::read_to_string(&path)
        .map_err(|e| Failure::Run(format!("nao li o arquivo de token: {e}")))?;
    let t = text.trim().to_string();
    if t.is_empty() {
        return Err(Failure::Run("arquivo de token vazio".to_string()));
    }
    Ok(Some(t))
}

/// Abre a sessao; com token, faz o `auth` antes de qualquer metodo.
fn open(o: &ClientOpts, name: Option<&str>) -> Result<Session, Failure> {
    let who = auth_name(name);
    if who.is_none() {
        return Err(usage(IDENTITY_UNDEFINED));
    }
    let tok = token(o)?;
    let auth = match (&tok, who.as_deref()) {
        (Some(t), Some(n)) => Some((n, t.as_str())),
        (None, _) => None,
        (Some(_), None) => unreachable!("identity checked above"),
    };
    let socket = socket_path(o)?;
    let mut s = Session::connect(&socket).map_err(rpc_fail)?;
    s.has_token = tok.is_some();
    if let Some((n, t)) = auth
        && let Err(error) = s.call("auth", json!({"v": 1, "name": n, "token": t}))
        && !matches!(&error, Rpc::Server { code, .. } if code == "method_not_found")
    {
        return Err(rpc_fail(error));
    }
    Ok(s)
}

fn open_without_auth(o: &ClientOpts) -> Result<Session, Failure> {
    let socket = socket_path(o)?;
    Session::connect(&socket).map_err(rpc_fail)
}

fn need<'a>(name: &'a Option<String>, _what: &str) -> Result<&'a str, Failure> {
    name.as_deref().ok_or_else(|| usage(IDENTITY_UNDEFINED))
}

/// Escapa caracteres de controle como `\xNN` (spec F.1): nenhum byte de controle chega ao terminal.
pub fn esc(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        if c.is_control() {
            out.push_str(&format!("\\x{:02x}", c as u32));
        } else {
            out.push(c);
        }
    }
    out
}

fn txt(v: &Value) -> String {
    match v {
        Value::String(s) => esc(s),
        Value::Null => "-".to_string(),
        other => other.to_string(),
    }
}

fn random_id() -> Result<String, Failure> {
    let mut b = [0u8; 16];
    File::open("/dev/urandom")
        .and_then(|mut f| f.read_exact(&mut b))
        .map_err(|e| Failure::Run(format!("nao gerei id: {e}")))?;
    Ok(b.iter().map(|x| format!("{x:02x}")).collect())
}

/// Linha de mensagem: `SEQ ID KIND FROM>TO | SUBJECT`, com o corpo recuado em 2 espacos.
fn msg_text(m: &Value) -> String {
    let mut s = format!(
        "{} {} {} {}>{} | {}",
        txt(&m["seq"]),
        txt(&m["id"]),
        txt(&m["kind"]),
        txt(&m["from"]),
        txt(&m["to"]),
        txt(&m["subject"])
    );
    if let Some(body) = m["body"].as_str().filter(|b| !b.is_empty()) {
        for line in body.lines() {
            s.push_str("\n  ");
            s.push_str(&esc(line));
        }
    }
    s
}

pub fn send(args: &[String]) -> Result<(), Failure> {
    let mut vals = SOCKET_OPTS.to_vec();
    vals.extend([
        "from",
        "to",
        "kind",
        "subject",
        "body",
        "body-file",
        "thread",
        "id",
        "ref",
        "due",
    ]);
    let o = client_opts(args, &vals, &["json", "keep-open"])?;
    let from = identity(&o, "from")?;
    let from_name = need(&from, "send")?.to_string();
    let body = match (o.get("body"), o.get("body-file")) {
        (Some(b), None) => b.to_string(),
        (None, Some("-")) => {
            let mut s = String::new();
            io::stdin()
                .read_to_string(&mut s)
                .map_err(|e| Failure::Run(format!("nao li stdin: {e}")))?;
            s
        }
        (None, Some(path)) => {
            let bytes = fs::read(path).map_err(|e| Failure::Run(format!("nao li {path}: {e}")))?;
            String::from_utf8(bytes)
                .map_err(|_| Failure::Run("body-file nao e UTF-8".to_string()))?
        }
        _ => {
            return Err(usage(
                "exatamente um de --body, --body-file ou --body-file -",
            ));
        }
    };
    let mut refs = Vec::new();
    for r in o.all("ref") {
        let Some((t, v)) = r.split_once(':') else {
            return Err(usage("--ref deve ser t:v"));
        };
        refs.push(json!({"t": t, "v": v}));
    }
    let id = match o.get("id") {
        Some(i) => i.to_string(),
        None => random_id()?,
    };
    let mut params = Map::new();
    params.insert("v".into(), json!(1));
    params.insert("id".into(), json!(id));
    params.insert("from".into(), json!(from_name));
    params.insert("to".into(), json!(o.req("to")?));
    params.insert("kind".into(), json!(o.req("kind")?));
    params.insert("subject".into(), json!(o.req("subject")?));
    params.insert("body".into(), json!(body));
    if let Some(t) = o.get("thread") {
        params.insert("thread".into(), json!(t));
    }
    if !refs.is_empty() {
        params.insert("refs".into(), Value::Array(refs));
    }
    if o.flag("keep-open") {
        params.insert("keep_open".into(), json!(true));
    }
    if let Some(due) = o.get("due") {
        params.insert("due_s".into(), json!(parse_due(due)?));
    }
    let mut s = open(&o, Some(from_name.as_str()))?;
    let r = match s.call("message.send", Value::Object(params)) {
        Ok(result) => result,
        Err(Rpc::NoResponse(message)) => {
            return Err(Failure::Unavailable(format!(
                "{message}; id da mensagem para reconciliar: {id}"
            )));
        }
        Err(error) => return Err(rpc_fail(error)),
    };
    if o.flag("json") {
        println!(
            "{}",
            json!({"id": r["id"], "seq": r["seq"], "state": r["state"], "duplicate": r["duplicate"], "closed": r["closed"]})
        );
    } else if r["duplicate"] == json!(true) {
        println!("{} {} dup", txt(&r["id"]), txt(&r["seq"]));
    } else {
        println!("{} {}", txt(&r["id"]), txt(&r["seq"]));
    }
    Ok(())
}

pub fn inbox(args: &[String]) -> Result<(), Failure> {
    let mut vals = SOCKET_OPTS.to_vec();
    vals.extend(["as", "after"]);
    let o = client_opts(args, &vals, &["all", "take", "ids", "json"])?;
    let name = identity(&o, "as")?;
    let name = need(&name, "inbox")?.to_string();
    let take = o.flag("take");
    if take && (o.flag("all") || o.get("after").is_some()) {
        return Err(usage("--take nao combina com --all nem --after"));
    }
    let mut s = open(&o, Some(name.as_str()))?;
    if take {
        loop {
            let r = s
                .call("message.take", json!({"v": 1, "to": name, "limit": 200}))
                .map_err(rpc_fail)?;
            let msgs = r["messages"].as_array().cloned().unwrap_or_default();
            print_msgs(&msgs, &o);
            if msgs.len() < 200 {
                break;
            }
        }
        return Ok(());
    }
    let mut after: u64 = match o.get("after") {
        Some(a) => a.parse().map_err(|_| usage("--after deve ser inteiro"))?,
        None => 0,
    };
    loop {
        let r = s
            .call(
                "message.list",
                json!({"v": 1, "to": name, "after_seq": after, "limit": 200, "unresolved_only": !o.flag("all")}),
            )
            .map_err(rpc_fail)?;
        let msgs = r["messages"].as_array().cloned().unwrap_or_default();
        print_msgs(&msgs, &o);
        match msgs.last().and_then(|m| m["seq"].as_u64()) {
            Some(last) if msgs.len() == 200 => after = last,
            _ => break,
        }
    }
    Ok(())
}

fn print_msgs(msgs: &[Value], o: &Opts) {
    for m in msgs {
        if o.flag("json") {
            println!("{m}");
        } else if o.flag("ids") {
            println!("{}", txt(&m["id"]));
        } else {
            println!("{}", msg_text(m));
        }
    }
}

pub fn ack(args: &[String]) -> Result<(), Failure> {
    let mut vals = SOCKET_OPTS.to_vec();
    vals.extend(["id", "as", "stage", "result", "note", "await"]);
    let o = client_opts(args, &vals, &[])?;
    let name = identity(&o, "as")?;
    let name = need(&name, "ack")?.to_string();
    let stage = o.req("stage")?;
    if stage != "aceito" && stage != "resultado" && stage != "bloqueado" {
        return Err(usage("--stage deve ser aceito, resultado ou bloqueado"));
    }
    if o.get("result").is_some() && stage != "resultado" {
        return Err(usage("--result so vale com --stage resultado"));
    }
    if stage == "bloqueado" {
        let await_class = o.req("await")?;
        if !matches!(
            await_class,
            "permission" | "user_input" | "network" | "identity" | "policy" | "unknown" | "none"
        ) {
            return Err(usage(
                "--await deve ser permission, user_input, network, identity, policy, unknown ou none",
            ));
        }
        if o.get("note").is_some() || o.get("result").is_some() {
            return Err(usage("--stage bloqueado aceita somente --await"));
        }
    } else if o.get("await").is_some() {
        return Err(usage("--await so vale com --stage bloqueado"));
    }
    if matches!(o.get("result"), Some("refused" | "failed")) {
        let note = o
            .get("note")
            .ok_or_else(|| usage("--result refused/failed exige --note"))?;
        if note.chars().count() > 200 {
            return Err(usage("--note deve ter no maximo 200 caracteres"));
        }
    }
    let mut params = Map::new();
    params.insert("v".into(), json!(1));
    params.insert("id".into(), json!(o.req("id")?));
    params.insert("by".into(), json!(name));
    params.insert("stage".into(), json!(stage));
    if let Some(r) = o.get("result") {
        params.insert("result".into(), json!(r));
    }
    if let Some(n) = o.get("note") {
        params.insert("note".into(), json!(n));
    }
    if let Some(a) = o.get("await") {
        params.insert("await".into(), json!(a));
    }
    let mut s = open(&o, Some(name.as_str()))?;
    s.call("message.ack", Value::Object(params))
        .map_err(rpc_fail)?;
    println!("ok");
    Ok(())
}

fn parse_due(s: &str) -> Result<u64, Failure> {
    let (number, unit) = s.split_at(s.len().saturating_sub(1));
    if number.is_empty() || !number.bytes().all(|b| b.is_ascii_digit()) {
        return Err(usage("--due deve ser N(s|m|h) inteiro positivo"));
    }
    let n = number
        .parse::<u64>()
        .map_err(|_| usage("--due fora do intervalo"))?;
    let factor = match unit {
        "s" => 1,
        "m" => 60,
        "h" => 3600,
        _ => return Err(usage("--due deve terminar em s, m ou h")),
    };
    let seconds = n
        .checked_mul(factor)
        .ok_or_else(|| usage("--due fora do intervalo"))?;
    if !(300..=86_400).contains(&seconds) {
        return Err(usage("--due deve ser de 5 min a 24 h"));
    }
    Ok(seconds)
}

pub fn status(args: &[String]) -> Result<(), Failure> {
    let mut vals = SOCKET_OPTS.to_vec();
    vals.push("as");
    let o = client_opts(args, &vals, &["json"])?;
    let name = identity(&o, "as")?;
    let mut s = match name.as_deref() {
        Some(name) => open(&o, Some(name))?,
        None => open_without_auth(&o)?,
    };
    let r = s.call("status", json!({"v": 1})).map_err(rpc_fail)?;
    if o.flag("json") {
        println!("{r}");
        return Ok(());
    }
    println!("head_seq {}", txt(&r["head_seq"]));
    if let Some(recs) = r["recipients"].as_object() {
        for (n, info) in recs {
            println!(
                "{n} pendentes={} aceitas={} campainha={} ultimo_bell_seq={}",
                txt(&info["pending"]),
                txt(&info["accepted"]),
                txt(&info["bell"]),
                txt(&info["last_bell_seq"])
            );
        }
    }
    Ok(())
}

fn bind_rpc_fail(e: Rpc) -> Failure {
    rpc_failure(e, true)
}

pub fn bind(args: &[String]) -> Result<(), Failure> {
    let mut vals = SOCKET_OPTS.to_vec();
    vals.extend(["as", "mux", "pane"]);
    let o = client_opts(args, &vals, &["keepalive"])?;
    let name = identity(&o, "as")?;
    let name = need(&name, "bind")?.to_string();
    if let Some(env_name) = std::env::var("CHATD_AS").ok().filter(|s| !s.is_empty())
        && env_name != name
    {
        return Err(Failure::BindForbidden(format!(
            "forbidden: --as {name} difere de CHATD_AS {env_name}"
        )));
    }
    let tmux_pane = std::env::var("TMUX_PANE").ok().filter(|v| !v.is_empty());
    let herdr_pane = std::env::var("HERDR_PANE_ID")
        .ok()
        .filter(|v| !v.is_empty());
    let mux = match o.get("mux") {
        Some("tmux") => "tmux",
        Some("herdr") => "herdr",
        Some(_) => return Err(usage("--mux deve ser herdr ou tmux")),
        None => match (tmux_pane.is_some(), herdr_pane.is_some()) {
            (true, false) => "tmux",
            (false, true) => "herdr",
            (true, true) => return Err(usage("multiplexador ambiguo; informe --mux")),
            (false, false) => {
                return Err(usage("nao detectei multiplexador; informe --mux e --pane"));
            }
        },
    };
    let pane = o
        .get("pane")
        .map(str::to_string)
        .or_else(|| if mux == "tmux" { tmux_pane } else { herdr_pane })
        .ok_or_else(|| usage(format!("--pane obrigatorio para mux {mux}")))?;
    if !crate::protocol::valid_pane_id(&pane) {
        return Err(usage("pane invalido"));
    }
    let keepalive = o.flag("keepalive");
    let stop = Arc::new(AtomicBool::new(false));
    if keepalive {
        for sig in [signal_hook::consts::SIGINT, signal_hook::consts::SIGTERM] {
            signal_hook::flag::register(sig, Arc::clone(&stop))
                .map_err(|e| Failure::Run(format!("nao registrei sinal: {e}")))?;
        }
    }
    let mut s = match open(&o, Some(&name)) {
        Ok(s) => s,
        Err(Failure::Run(message)) if message.starts_with("nao conectou em ") => {
            return Err(Failure::Unavailable(message));
        }
        Err(error) => return Err(error),
    };
    let params = json!({"v": 1, "mux": mux, "pane": pane});
    let result = s
        .call("agent.bind", params.clone())
        .map_err(bind_rpc_fail)?;
    println!(
        "{} {} {} ttl_s={}",
        txt(&result["name"]),
        txt(&result["mux"]),
        txt(&result["pane"]),
        txt(&result["ttl_s"])
    );
    if !keepalive {
        return Ok(());
    }
    while !sleep_or_stop(BIND_KEEPALIVE, &stop, None) {
        s.call("agent.bind", params.clone())
            .map_err(bind_rpc_fail)?;
    }
    // Ao sair, o daemon detecta o pid ausente e registra `unbound process_gone`.
    Ok(())
}

/// Estado do `watch` entre reconexoes (v1.1 + opcoes da v1.2).
struct WatchState {
    /// Ultimo seq visto (nunca imprime duas vezes).
    last: Option<u64>,
    from_start: bool,
    /// `--resume-file`: cursor gravado depois de cada evento processado (v1.2 A).
    resume_file: Option<PathBuf>,
    /// Ainda falta comparar o `N` do arquivo com o `head_seq` do daemon (v1.2 A.2).
    head_check: bool,
    /// `--heartbeat-file` (v1.2 C).
    heartbeat: Option<PathBuf>,
    /// `--exit-after` (v1.2 B): instante de saida limpa.
    deadline: Option<Instant>,
    /// Espera da proxima tentativa com `--reconnect` (1, 2, 4, 8, 16, 30, 30...).
    backoff: u64,
    /// Espera padrao v1.6, em milissegundos (200 ms ate 5 s).
    quick_backoff_ms: u64,
    receipts: bool,
    seen: HashMap<String, String>,
}

enum WatchErr {
    /// Erro de protocolo ou de autenticacao: nao adianta reconectar.
    Fatal(Failure),
    /// Queda de conexao ou servidor fora: reconecta.
    Retry,
}

fn classify(e: Rpc) -> WatchErr {
    match e {
        Rpc::Io(_) | Rpc::Connect { .. } | Rpc::Unavailable(_) | Rpc::NoResponse(_) => {
            WatchErr::Retry
        }
        Rpc::ConnectDenied => WatchErr::Fatal(rpc_fail(Rpc::ConnectDenied)),
        e => WatchErr::Fatal(rpc_fail(e)),
    }
}

/// Duracao de `--exit-after`: inteiro 1..=86400 seguido de s, m ou h, ate 24 h no total.
pub fn parse_dur(s: &str) -> Result<Duration, Failure> {
    let bad = || usage("--exit-after deve ser 1 a 86400 seguido de s, m ou h (ex.: 30s, 29m, 2h)");
    if !s.is_ascii() || s.len() < 2 {
        return Err(bad());
    }
    let (digits, unit) = s.split_at(s.len() - 1);
    let mult: u64 = match unit {
        "s" => 1,
        "m" => 60,
        "h" => 3600,
        _ => return Err(bad()),
    };
    if !digits.bytes().all(|b| b.is_ascii_digit()) {
        return Err(bad());
    }
    let n: u64 = digits.parse().map_err(|_| bad())?;
    if !(1..=86400).contains(&n) {
        return Err(bad());
    }
    let secs = n * mult;
    if !(1..=86400).contains(&secs) {
        return Err(bad());
    }
    Ok(Duration::from_secs(secs))
}

/// Le o cursor de `--resume-file`: uma linha com inteiro decimal >= 0.
fn read_cursor(path: &Path) -> Result<u64, Failure> {
    let text = fs::read_to_string(path).map_err(|_| usage("--resume-file ilegivel"))?;
    let t = text.trim();
    if t.is_empty() || !t.bytes().all(|b| b.is_ascii_digit()) {
        return Err(usage("--resume-file deve conter um inteiro decimal"));
    }
    t.parse::<u64>()
        .map_err(|_| usage("--resume-file com valor fora da faixa"))
}

/// Grava `content` em `path` de forma atomica: temporario 0600 no mesmo diretorio, fsync, rename.
fn write_atomic(path: &Path, content: &str) -> Result<(), String> {
    let dir = match path.parent() {
        Some(p) if !p.as_os_str().is_empty() => p,
        _ => Path::new("."),
    };
    let name = path
        .file_name()
        .ok_or_else(|| "caminho sem nome de arquivo".to_string())?
        .to_string_lossy()
        .into_owned();
    let tmp = dir.join(format!(".{name}.tmp.{}", std::process::id()));
    let _ = fs::remove_file(&tmp);
    let res = (|| -> io::Result<()> {
        let mut f = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&tmp)?;
        f.write_all(content.as_bytes())?;
        f.sync_all()?;
        drop(f);
        fs::rename(&tmp, path)
    })();
    res.map_err(|e| {
        let _ = fs::remove_file(&tmp);
        format!("nao gravei {}: {e}", path.display())
    })
}

fn now_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

fn sleep_or_stop(d: Duration, stop: &AtomicBool, deadline: Option<Instant>) -> bool {
    let until = Instant::now() + d;
    while Instant::now() < until {
        if stop.load(Ordering::SeqCst) || deadline.is_some_and(|dl| Instant::now() >= dl) {
            return true;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    false
}

pub fn watch(args: &[String]) -> Result<(), Failure> {
    let mut vals = SOCKET_OPTS.to_vec();
    vals.extend(["as", "resume-file", "exit-after", "heartbeat-file"]);
    let o = client_opts(args, &vals, &["from-start", "reconnect", "receipts"])?;
    let name = identity(&o, "as")?;
    let name = need(&name, "watch")?.to_string();
    let socket = socket_path(&o)?;
    let from_start = o.flag("from-start");
    let resume_file = o.get("resume-file").map(PathBuf::from);
    if resume_file.is_some() && from_start {
        return Err(usage("--resume-file nao combina com --from-start"));
    }
    let deadline = match o.get("exit-after") {
        Some(d) => Some(Instant::now() + parse_dur(d)?),
        None => None,
    };
    // Sem token o watch conecta sem `auth` (servidor com `auth: off`); com `auth: required`
    // o servidor responde `unauthenticated` e o cliente sai com a mensagem de token ausente.
    let tok = token(&o)?;
    if let Some(env) = std::env::var("CHATD_AS").ok().filter(|s| !s.is_empty())
        && tok.is_some()
        && env != name
    {
        return Err(Failure::Run(format!(
            "forbidden: --as {name} difere de CHATD_AS {env}"
        )));
    }
    let stop = Arc::new(AtomicBool::new(false));
    for sig in [signal_hook::consts::SIGINT, signal_hook::consts::SIGTERM] {
        signal_hook::flag::register(sig, Arc::clone(&stop))
            .map_err(|e| Failure::Run(format!("nao registrei sinal: {e}")))?;
    }
    let mut st = WatchState {
        last: None,
        from_start,
        resume_file,
        head_check: false,
        heartbeat: o.get("heartbeat-file").map(PathBuf::from),
        deadline,
        backoff: 1,
        quick_backoff_ms: 200,
        receipts: o.flag("receipts"),
        seen: HashMap::new(),
    };
    if let Some(path) = st.resume_file.clone()
        && path.exists()
    {
        st.last = Some(read_cursor(&path)?);
        st.head_check = true;
    }
    let reconnect = o.flag("reconnect");
    loop {
        if stop.load(Ordering::SeqCst) || st.deadline.is_some_and(|d| Instant::now() >= d) {
            return Ok(());
        }
        match watch_once(&socket, &name, tok.as_deref(), &mut st, &stop) {
            Ok(()) => return Ok(()),
            Err(WatchErr::Fatal(f)) => return Err(f),
            Err(WatchErr::Retry) => {
                let wait = if reconnect {
                    let w = st.backoff;
                    eprintln!("chatd: reconectando em {w}s");
                    st.backoff = if w >= 16 { 30 } else { w * 2 };
                    Duration::from_secs(w)
                } else {
                    let ms = st.quick_backoff_ms;
                    eprintln!("aviso: reconectando em {ms}ms");
                    st.quick_backoff_ms = (ms * 2).min(5000);
                    Duration::from_millis(ms)
                };
                if sleep_or_stop(wait, &stop, st.deadline) {
                    return Ok(());
                }
            }
        }
    }
}

/// Grava o cursor depois de processar o evento `seq` (v1.2 A.4).
fn persist_cursor(st: &WatchState, seq: u64) -> Result<(), WatchErr> {
    if let Some(p) = &st.resume_file {
        write_atomic(p, &format!("{seq}\n")).map_err(|e| WatchErr::Fatal(Failure::Run(e)))?;
    }
    Ok(())
}

/// Uma conexao do `watch`: auth, `events.resume` a partir do ultimo seq e impressao ao vivo.
fn watch_once(
    socket: &Path,
    name: &str,
    tok: Option<&str>,
    st: &mut WatchState,
    stop: &AtomicBool,
) -> Result<(), WatchErr> {
    let mut s = Session::connect_once(socket).map_err(classify)?;
    if let Some(t) = tok {
        s.has_token = true;
        s.call("auth", json!({"v": 1, "name": name, "token": t}))
            .map_err(classify)?;
    }
    if st.last.is_none() || st.head_check {
        let r = s.call("status", json!({"v": 1})).map_err(classify)?;
        let head = r["head_seq"].as_u64().unwrap_or(0);
        match st.last {
            None => {
                let start = if st.from_start { 0 } else { head };
                st.last = Some(start);
                if let Some(p) = &st.resume_file {
                    write_atomic(p, &format!("{start}\n"))
                        .map_err(|e| WatchErr::Fatal(Failure::Run(e)))?;
                }
            }
            Some(n) if n > head => {
                return Err(WatchErr::Fatal(usage(
                    "--resume-file com N maior que head_seq do daemon",
                )));
            }
            Some(_) => {}
        }
        st.head_check = false;
    }
    let from = st.last.unwrap_or(0);
    s.call(
        "events.resume",
        json!({"v": 1, "last_seq": from, "receipts": st.receipts}),
    )
    .map_err(classify)?;
    // Conectado e autenticado: zera a espera de --reconnect e registra o heartbeat (v1.2 C).
    st.backoff = 1;
    st.quick_backoff_ms = 200;
    let mut last_hb = Instant::now();
    write_heartbeat(st)?;
    let mut last_ka = Instant::now();
    let mut out = io::stdout();
    loop {
        if stop.load(Ordering::SeqCst) || st.deadline.is_some_and(|d| Instant::now() >= d) {
            return Ok(());
        }
        if last_ka.elapsed() >= KEEPALIVE {
            s.write_req(json!("ka"), "status", json!({"v": 1}))
                .map_err(|_| WatchErr::Retry)?;
            last_ka = Instant::now();
        }
        if last_hb.elapsed() >= HEARTBEAT {
            write_heartbeat(st)?;
            last_hb = Instant::now();
        }
        let v = match s.next_value() {
            Ok(Some(v)) => v,
            Ok(None) => continue,
            Err(_) => return Err(WatchErr::Retry),
        };
        if v.get("method").and_then(Value::as_str) == Some("event") {
            consume_watch_event(&v["params"], name, st, &mut out)?;
        } else if v
            .get("error")
            .and_then(|e| e.get("code"))
            .and_then(Value::as_str)
            == Some("unauthenticated")
        {
            return Err(WatchErr::Fatal(Failure::Run(
                "unauthenticated: autenticacao falhou".to_string(),
            )));
        }
    }
}

fn consume_watch_event(
    ev: &Value,
    name: &str,
    st: &mut WatchState,
    out: &mut impl Write,
) -> Result<(), WatchErr> {
    let Some(seq) = ev["seq"].as_u64() else {
        return Ok(());
    };
    if st.last.is_some_and(|last| seq <= last) {
        return Ok(());
    }
    if ev["type"] == json!("registered") {
        if st.receipts && ev["from"].as_str() == Some(name) {
            st.seen.insert(txt(&ev["id"]), txt(&ev["subject"]));
        }
        if ev["to"].as_str() == Some(name) {
            let line = format!(
                "{} {} {} {}>{} | {}",
                seq,
                txt(&ev["id"]),
                txt(&ev["kind"]),
                txt(&ev["from"]),
                txt(&ev["to"]),
                txt(&ev["subject"])
            );
            let _ = writeln!(out, "{line}");
            let _ = out.flush();
        }
    } else if st.receipts
        && matches!(
            ev["type"].as_str(),
            Some("accepted" | "resulted" | "blocked")
        )
    {
        print_receipt(ev, seq, st, out);
    }
    st.last = Some(seq);
    persist_cursor(st, seq)
}

fn print_receipt(ev: &Value, seq: u64, st: &WatchState, out: &mut impl Write) {
    let stage = if ev["type"] == json!("accepted") {
        "aceito".to_string()
    } else if ev["type"] == json!("blocked") {
        format!("bloqueado:{}", txt(&ev["await"]))
    } else {
        format!("resultado:{}", txt(&ev["result"]))
    };
    let mut line = format!(
        "RECIBO {} {} {} {}",
        seq,
        txt(&ev["id"]),
        stage,
        txt(&ev["receipt_to"])
    );
    if let Some(subject) = st.seen.get(&txt(&ev["id"])) {
        line.push_str(" | ");
        line.push_str(subject);
    }
    let _ = writeln!(out, "{line}");
    let _ = out.flush();
}

pub fn show(args: &[String]) -> Result<(), Failure> {
    let mut vals = SOCKET_OPTS.to_vec();
    vals.extend(["as", "id"]);
    let o = client_opts(args, &vals, &["json"])?;
    let name = identity(&o, "as")?;
    let name = need(&name, "show")?.to_string();
    let mut s = open(&o, Some(&name))?;
    let result = s
        .call("message.get", json!({"v": 1, "id": o.req("id")?}))
        .map_err(rpc_fail)?;
    let message = &result["message"];
    if o.flag("json") {
        println!("{message}");
        return Ok(());
    }
    println!(
        "{} {} {}>{} {} | {}",
        txt(&message["id"]),
        txt(&message["state"]),
        txt(&message["from"]),
        txt(&message["to"]),
        txt(&message["kind"]),
        txt(&message["subject"])
    );
    println!("registrado  {}", txt(&message["registered_at"]));
    if message["accepted_at"].is_string() {
        let mut line = format!("aceito      {}", txt(&message["accepted_at"]));
        if message["state"] == json!("aceito") && message["note"].is_string() {
            line.push_str(&format!(" [{}]", txt(&message["note"])));
        }
        println!("{line}");
    }
    if message["resulted_at"].is_string() {
        let mut line = format!(
            "resultado   {} {}",
            txt(&message["resulted_at"]),
            txt(&message["result"])
        );
        if message["note"].is_string() {
            line.push_str(&format!(" [{}]", txt(&message["note"])));
        }
        println!("{line}");
    }
    Ok(())
}

fn write_heartbeat(st: &WatchState) -> Result<(), WatchErr> {
    if let Some(p) = &st.heartbeat {
        write_atomic(p, &format!("{}\n", now_secs()))
            .map_err(|e| WatchErr::Fatal(Failure::Run(e)))?;
    }
    Ok(())
}

/// Exibe a resolucao local do cliente sem conectar ao daemon ou revelar o token.
pub fn whoami(args: &[String]) -> Result<(), Failure> {
    let mut values = SOCKET_OPTS.to_vec();
    values.push("as");
    let o = client_opts(args, &values, &["json"])?;
    let identity = identity(&o, "as")?;
    let (state_dir, state_source) = if let Some(value) = o.get("state-dir") {
        (Some(expand_home(value)), "flag")
    } else if let Some(value) = env_value("CHATD_STATE_DIR") {
        (Some(expand_home(&value)), "env")
    } else if let Some(value) = o.file.state_dir.as_deref() {
        (Some(expand_home(value)), "config")
    } else {
        (None, "-")
    };
    let (socket, socket_source) = if let Some(value) = o.get("socket") {
        (Some(expand_home(value)), "flag")
    } else if let Some(value) = env_value("CHATD_SOCKET") {
        (Some(expand_home(&value)), "env")
    } else if let Some(value) = o.file.socket.as_deref() {
        (Some(expand_home(value)), "config")
    } else if let Some(dir) = &state_dir {
        (Some(dir.join("chatd.sock")), state_source)
    } else {
        (None, "-")
    };
    let has_socket = socket.is_some();
    let profile_origin = || {
        o.profile
            .as_deref()
            .map(|name| format!("profile:{name}"))
            .unwrap_or_else(|| "unset".to_string())
    };
    let (token_file, token_source) = if let Some(value) = o.get("token-file") {
        (Some(expand_home(value)), "flag".to_string())
    } else if let Some(value) = env_value("CHATD_TOKEN_FILE") {
        (Some(expand_home(&value)), "env".to_string())
    } else if let Some(profile) = o.profile() {
        (Some(expand_home(&profile.token_file)), profile_origin())
    } else {
        (None, "unset".to_string())
    };
    let as_source = if o.get("as").is_some() {
        "flag".to_string()
    } else if env_value("CHATD_AS").is_some() {
        "env".to_string()
    } else if o.profile().is_some() {
        profile_origin()
    } else {
        "unset".to_string()
    };
    if o.profile_source == "env"
        && o.get("profile").is_none()
        && (o.profile.as_deref() == Some("jabbur") || identity.as_deref() == Some("jabbur"))
    {
        eprintln!("{INHERITED_JABBUR_WARNING}");
    }
    let field = |value: Option<String>| value.map(Value::String).unwrap_or(Value::Null);
    let output = json!({
        "as": field(identity),
        "socket": field(socket.map(|p| p.display().to_string())),
        "state_dir": field(state_dir.map(|p| p.display().to_string())),
        "token_file": field(token_file.map(|p| p.display().to_string())),
        "profile": field(o.profile.clone()),
        "sources": {
            "as": as_source, "socket": socket_source, "state_dir": state_source,
            "token_file": token_source, "profile": o.profile_source,
        },
    });
    if o.flag("json") {
        println!("{output}");
    } else {
        for key in ["as", "socket", "state_dir", "token_file", "profile"] {
            let source = output["sources"][key].as_str().unwrap_or("-");
            let value = txt(&output[key]);
            println!("{key}={value} ({source})");
        }
    }
    if !has_socket {
        return Err(Failure::Bad(
            "whoami: socket ausente; informe --socket, CHATD_SOCKET, state_dir ou configuracao"
                .into(),
        ));
    }
    Ok(())
}

/// `chatd log --state-dir D [--tail N]`: le o log direto, sem socket (v1.1 D).
pub fn log(args: &[String]) -> Result<(), Failure> {
    let o = client_opts(args, &["state-dir", "tail"], &[])?;
    let dir = o
        .get("state-dir")
        .map(expand_home)
        .or_else(|| env_value("CHATD_STATE_DIR").map(|s| expand_home(&s)))
        .or_else(|| o.file.state_dir.as_deref().map(expand_home))
        .ok_or_else(|| usage("log exige --state-dir ou CHATD_STATE_DIR"))?;
    let tail: Option<usize> = match o.get("tail") {
        Some(n) => Some(n.parse().map_err(|_| usage("--tail deve ser inteiro"))?),
        None => None,
    };
    let text = fs::read_to_string(dir.join("log.jsonl"))
        .map_err(|e| Failure::Run(format!("nao li o log: {e}")))?;
    let lines: Vec<&str> = text.lines().collect();
    let start = tail.map_or(0, |n| lines.len().saturating_sub(n));
    for l in &lines[start..] {
        if let Ok(v) = serde_json::from_str::<Value>(l) {
            println!("{}", log_line(&v));
        }
    }
    Ok(())
}

fn log_line(v: &Value) -> String {
    let ts_raw = txt(&v["ts"]);
    let ts = chrono::DateTime::parse_from_rfc3339(&ts_raw)
        .map(|d| {
            d.with_timezone(&chrono::Local)
                .format("%Y-%m-%d %H:%M:%S")
                .to_string()
        })
        .unwrap_or(ts_raw);
    let ty = txt(&v["type"]);
    let mut s = String::new();
    if ty == "auth_failed" || ty == "denied" {
        s.push_str("!! ");
    }
    s.push_str(&format!("{} {} {}", txt(&v["seq"]), ts, ty));
    if let Some(id) = v["id"].as_str() {
        s.push(' ');
        s.push_str(&esc(id));
    }
    if let (Some(f), Some(t)) = (v["from"].as_str(), v["to"].as_str()) {
        s.push_str(&format!(" {}>{}", esc(f), esc(t)));
    }
    if let Some(subj) = v["subject"].as_str() {
        s.push_str(&format!(" | {}", esc(subj)));
    }
    match ty.as_str() {
        "bell_failed" => s.push_str(&format!(" reason={}", txt(&v["reason"]))),
        "auth_failed" => s.push_str(&format!(
            " name={} pid={} uid={}",
            txt(&v["name"]),
            txt(&v["peer_pid"]),
            txt(&v["peer_uid"])
        )),
        "denied" => s.push_str(&format!(
            " method={} claimed={} pid={} uid={}",
            txt(&v["method"]),
            txt(&v["claimed"]),
            txt(&v["peer_pid"]),
            txt(&v["peer_uid"])
        )),
        "bound" => s.push_str(&format!(
            " name={} mux={} pane={} pid={}",
            esc(&txt(&v["name"])),
            esc(&txt(&v["mux"])),
            esc(&txt(&v["pane"])),
            txt(&v["peer_pid"])
        )),
        "unbound" => s.push_str(&format!(
            " name={} mux={} pane={} reason={}",
            esc(&txt(&v["name"])),
            esc(&txt(&v["mux"])),
            esc(&txt(&v["pane"])),
            txt(&v["reason"])
        )),
        _ => {}
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_dur_aceita_unidades_e_limites() {
        assert_eq!(parse_dur("1s").ok().unwrap(), Duration::from_secs(1));
        assert_eq!(parse_dur("90s").ok().unwrap(), Duration::from_secs(90));
        assert_eq!(parse_dur("1m").ok().unwrap(), Duration::from_secs(60));
        assert_eq!(parse_dur("29m").ok().unwrap(), Duration::from_secs(29 * 60));
        assert_eq!(parse_dur("2h").ok().unwrap(), Duration::from_secs(7200));
        assert_eq!(parse_dur("24h").ok().unwrap(), Duration::from_secs(86400));
        assert_eq!(
            parse_dur("86400s").ok().unwrap(),
            Duration::from_secs(86400)
        );
    }

    #[test]
    fn write_atomic_cria_0600_e_substitui_sem_sobras() {
        use std::os::unix::fs::PermissionsExt;
        let dir =
            std::env::temp_dir().join(format!("chatd-wa-{}-{}", std::process::id(), now_secs()));
        fs::create_dir_all(&dir).unwrap();
        let p = dir.join("cursor");
        write_atomic(&p, "1\n").unwrap();
        write_atomic(&p, "2\n").unwrap();
        assert_eq!(fs::read_to_string(&p).unwrap(), "2\n");
        assert_eq!(
            fs::metadata(&p).unwrap().permissions().mode() & 0o777,
            0o600
        );
        let names: Vec<_> = fs::read_dir(&dir)
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        assert_eq!(
            names,
            vec!["cursor".to_string()],
            "temporario sobrou: {names:?}"
        );
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn parse_dur_recusa_formas_invalidas() {
        for bad in [
            "", "s", "0s", "0m", "25h", "86401s", "1441m", "abc", "30", "1.5s", "2d", "-1s", "１s",
        ] {
            assert!(parse_dur(bad).is_err(), "deveria recusar {bad:?}");
        }
    }
}

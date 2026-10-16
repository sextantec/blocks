//! Socket Unix: uma thread por conexao, teto de 64, linha ate 1 MiB, ociosidade de 30 s,
//! autenticacao por token (v1.1 B), identidade imposta e `events.resume` com entrega ao vivo.

use std::collections::{HashMap, VecDeque};
use std::fs;
use std::io::{self, ErrorKind, Read, Write};
use std::os::fd::AsRawFd;
use std::os::unix::fs::{FileTypeExt, PermissionsExt};
use std::os::unix::net::{UnixListener, UnixStream};
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use serde_json::{Map, Value, json};

use crate::auth;
use crate::bell;
use crate::binding::{self, Binding, Mux, Registry, TTL};
use crate::blobs::BlobManager;
use crate::config::Config;
use crate::protocol::{self, ApiError, MAX_LINE};
use crate::store::{Store, msg_json};

const MAX_CONNS: usize = 64;
const IDLE: Duration = Duration::from_secs(30);
const READ_TICK: Duration = Duration::from_millis(100);
const WRITE_TIMEOUT: Duration = Duration::from_secs(5);
const ACCEPT_TICK: Duration = Duration::from_millis(20);
/// Trava por pid (v1.1 B): 5 falhas em 60 s trancam novas tentativas por 300 s.
const AUTH_MAX_FAILS: usize = 5;
const AUTH_WINDOW: Duration = Duration::from_secs(60);
const AUTH_LOCK: Duration = Duration::from_secs(300);

/// Teto do mapa de pids (spec F.3).
const PID_CAP: usize = 4096;
static NEXT_CONN: AtomicUsize = AtomicUsize::new(1);

#[derive(Default)]
struct PidAuth {
    fails: VecDeque<Instant>,
    locked_until: Option<Instant>,
    /// Ultimo toque, para escolher o que sai primeiro quando o mapa passa do teto.
    last: Option<Instant>,
}

fn pid_entry(m: &mut HashMap<i32, PidAuth>, pid: i32, now: Instant) -> &mut PidAuth {
    let e = m.entry(pid).or_default();
    e.last = Some(now);
    e
}

/// Poda entradas com janela expirada e sem trava; depois respeita o teto de `PID_CAP`
/// (sem trava e mais antigas saem primeiro; so depois as trancadas mais antigas).
fn prune_pids(m: &mut HashMap<i32, PidAuth>, now: Instant) {
    m.retain(|_, e| {
        e.fails.retain(|f| now.duration_since(*f) < AUTH_WINDOW);
        let locked = matches!(e.locked_until, Some(t) if now < t);
        locked || !e.fails.is_empty()
    });
    if m.len() <= PID_CAP {
        return;
    }
    let mut order: Vec<(bool, Option<Instant>, i32)> = m
        .iter()
        .map(|(p, e)| (e.locked_until.is_some(), e.last, *p))
        .collect();
    order.sort();
    for (_, _, p) in order {
        if m.len() <= PID_CAP {
            break;
        }
        m.remove(&p);
    }
}

pub struct Shared {
    pub store: Mutex<Store>,
    pub blobs: Mutex<BlobManager>,
    pub bindings: Mutex<Registry>,
    pub cfg: Config,
    pub auth_path: PathBuf,
    pub connections: Arc<AtomicUsize>,
    pub started_at: Instant,
    pids: Mutex<HashMap<i32, PidAuth>>,
}

pub fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

/// Estado por conexao: identidade autenticada, falhas de `auth` e cursor de `events.resume`.
struct Conn {
    identity: Option<String>,
    connection: u64,
    failures: usize,
    cursor: Option<u64>,
    receipts: bool,
    peer_uid: u32,
    peer_pid: i32,
    last_watch_renew: Instant,
}

struct ActiveGuard(Arc<AtomicUsize>);

impl Drop for ActiveGuard {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::SeqCst);
    }
}

pub fn run(
    cfg: Config,
    store: Store,
    blobs: BlobManager,
    socket: &Path,
    auth_path: PathBuf,
    shutdown: Arc<AtomicBool>,
) -> Result<(), String> {
    prepare_socket_path(socket)?;
    let listener = UnixListener::bind(socket)
        .map_err(|e| format!("nao consegui escutar em {}: {e}", socket.display()))?;
    if let Err(e) = fs::set_permissions(socket, fs::Permissions::from_mode(0o600)) {
        let _ = fs::remove_file(socket);
        return Err(format!("nao ajustei permissoes do socket: {e}"));
    }
    listener
        .set_nonblocking(true)
        .map_err(|e| format!("nao configurei o socket: {e}"))?;

    let active = Arc::new(AtomicUsize::new(0));
    let shared = Arc::new(Shared {
        store: Mutex::new(store),
        blobs: Mutex::new(blobs),
        bindings: Mutex::new(Registry::default()),
        cfg,
        auth_path,
        connections: Arc::clone(&active),
        started_at: Instant::now(),
        pids: Mutex::new(HashMap::new()),
    });
    let bell_handle = if shared.cfg.recipients.values().any(|d| d.is_bell()) {
        let s = Arc::clone(&shared);
        let f = Arc::clone(&shutdown);
        Some(thread::spawn(move || bell::run(&s, &f)))
    } else {
        None
    };
    let binding_shared = Arc::clone(&shared);
    let binding_shutdown = Arc::clone(&shutdown);
    let binding_handle =
        thread::spawn(move || binding::maintain(&binding_shared, &binding_shutdown));
    let blob_handle = start_blob_janitor(Arc::clone(&shared), Arc::clone(&shutdown));

    println!("chatd: ready");
    let _ = io::stdout().flush();

    // SAFETY: getuid nao recebe argumentos e nao falha.
    let server_uid = unsafe { libc::getuid() };
    let mut handles: Vec<JoinHandle<()>> = Vec::new();
    while !shutdown.load(Ordering::SeqCst) {
        match listener.accept() {
            Ok((mut stream, _)) => {
                // SO_PEERCRED: uid diferente do servidor = conexao fechada sem resposta.
                let Ok((uid, pid)) = peer_cred(&stream) else {
                    continue;
                };
                if uid != server_uid {
                    continue;
                }
                if active.load(Ordering::SeqCst) >= MAX_CONNS {
                    let _ = stream.set_write_timeout(Some(WRITE_TIMEOUT));
                    let err = ApiError::new("too_many_connections", "limite de 64 conexoes");
                    let _ = write_line(&mut stream, &error_reply(Value::Null, &err));
                    continue;
                }
                if stream.set_nonblocking(false).is_err() {
                    continue;
                }
                active.fetch_add(1, Ordering::SeqCst);
                let s = Arc::clone(&shared);
                let f = Arc::clone(&shutdown);
                let guard = ActiveGuard(Arc::clone(&active));
                handles.push(thread::spawn(move || {
                    let _guard = guard;
                    let _ = catch_unwind(AssertUnwindSafe(move || {
                        serve_conn(stream, &s, &f, uid, pid)
                    }));
                }));
            }
            Err(_) => thread::sleep(ACCEPT_TICK),
        }
        handles.retain(|h| !h.is_finished());
    }
    for h in handles {
        let _ = h.join();
    }
    if let Some(h) = bell_handle {
        let _ = h.join();
    }
    let _ = binding_handle.join();
    let _ = blob_handle.join();
    let _ = fs::remove_file(socket);
    Ok(())
}

fn start_blob_janitor(shared: Arc<Shared>, shutdown: Arc<AtomicBool>) -> JoinHandle<()> {
    thread::spawn(move || {
        while !shutdown.load(Ordering::SeqCst) {
            for _ in 0..60 {
                if shutdown.load(Ordering::SeqCst) {
                    break;
                }
                thread::sleep(Duration::from_secs(1));
            }
            if shutdown.load(Ordering::SeqCst) {
                break;
            }
            let mut emit = |kind: &'static str, fields: Vec<(&'static str, Value)>| {
                lock(&shared.store).blob_event(kind, fields)
            };
            lock(&shared.blobs).cleanup_tick(&mut emit);
        }
    })
}

/// uid e pid do outro lado do socket (SO_PEERCRED).
fn peer_cred(stream: &UnixStream) -> io::Result<(u32, i32)> {
    let mut cred = libc::ucred {
        pid: 0,
        uid: 0,
        gid: 0,
    };
    let mut len = std::mem::size_of::<libc::ucred>() as libc::socklen_t;
    // SAFETY: ponteiros validos para uma struct ucred e seu tamanho, descritor do socket aberto.
    let rc = unsafe {
        libc::getsockopt(
            stream.as_raw_fd(),
            libc::SOL_SOCKET,
            libc::SO_PEERCRED,
            (&mut cred as *mut libc::ucred).cast(),
            &mut len,
        )
    };
    if rc != 0 {
        return Err(io::Error::last_os_error());
    }
    Ok((cred.uid, cred.pid))
}

fn prepare_socket_path(path: &Path) -> Result<(), String> {
    match fs::symlink_metadata(path) {
        Ok(m) if m.file_type().is_socket() => fs::remove_file(path)
            .map_err(|e| format!("nao removi socket antigo {}: {e}", path.display())),
        Ok(_) => Err(format!(
            "{} existe e nao e socket; recusei remover",
            path.display()
        )),
        Err(e) if e.kind() == ErrorKind::NotFound => Ok(()),
        Err(e) => Err(format!("nao acessei {}: {e}", path.display())),
    }
}

fn error_reply(id: Value, e: &ApiError) -> Value {
    json!({"jsonrpc": "2.0", "id": id, "error": e.to_error_json()})
}

fn ok_reply(id: Value, result: Value) -> Value {
    json!({"jsonrpc": "2.0", "id": id, "result": result})
}

fn write_line(stream: &mut UnixStream, v: &Value) -> io::Result<()> {
    let mut bytes = serde_json::to_vec(v).map_err(io::Error::other)?;
    bytes.push(b'\n');
    stream.write_all(&bytes)
}

fn serve_conn(mut stream: UnixStream, shared: &Shared, shutdown: &AtomicBool, uid: u32, pid: i32) {
    if stream.set_read_timeout(Some(READ_TICK)).is_err()
        || stream.set_write_timeout(Some(WRITE_TIMEOUT)).is_err()
    {
        return;
    }
    let mut conn = Conn {
        identity: None,
        connection: NEXT_CONN.fetch_add(1, Ordering::Relaxed) as u64,
        failures: 0,
        cursor: None,
        receipts: false,
        peer_uid: uid,
        peer_pid: pid,
        last_watch_renew: Instant::now(),
    };
    let mut buf: Vec<u8> = Vec::new();
    let mut chunk = [0u8; 8192];
    let mut last_activity = Instant::now();
    loop {
        if shutdown.load(Ordering::SeqCst) {
            return;
        }
        while let Some(pos) = buf.iter().position(|&b| b == b'\n') {
            let line: Vec<u8> = buf.drain(..=pos).collect();
            let content = &line[..line.len() - 1];
            if content.len() > MAX_LINE {
                let e = ApiError::new("invalid_request", "linha acima de 1 MiB");
                let _ = write_line(&mut stream, &error_reply(Value::Null, &e));
                return;
            }
            let (reply, close) = handle_line(content, shared, &mut conn);
            if write_line(&mut stream, &reply).is_err() || close {
                return;
            }
        }
        if buf.len() > MAX_LINE {
            let e = ApiError::new("invalid_request", "linha acima de 1 MiB");
            let _ = write_line(&mut stream, &error_reply(Value::Null, &e));
            return;
        }
        if let Some(c) = conn.cursor.as_mut()
            && push_events(
                &mut stream,
                shared,
                c,
                conn.identity.as_deref(),
                conn.receipts,
            )
            .is_err()
        {
            return;
        }
        match stream.read(&mut chunk) {
            Ok(0) => return,
            Ok(n) => {
                buf.extend_from_slice(&chunk[..n]);
                last_activity = Instant::now();
            }
            Err(e)
                if matches!(
                    e.kind(),
                    ErrorKind::WouldBlock | ErrorKind::TimedOut | ErrorKind::Interrupted
                ) => {}
            Err(_) => return,
        }
        if last_activity.elapsed() >= IDLE {
            return;
        }
    }
}

/// Envia as notificacoes de eventos com seq > cursor, na ordem, atualizando o cursor.
/// Com identidade, so eventos visiveis para ela (v1.1 B); o cursor avanca mesmo nos ocultos.
fn push_events(
    stream: &mut UnixStream,
    shared: &Shared,
    cursor: &mut u64,
    who: Option<&str>,
    receipts: bool,
) -> io::Result<()> {
    if shared.cfg.auth_required && who.is_none() {
        return Ok(());
    }
    let batch: Vec<(u64, Option<Value>)> = {
        let st = lock(&shared.store);
        st.events_after(*cursor)
            .into_iter()
            .filter_map(|ev| {
                let seq = ev.get("seq").and_then(Value::as_u64)?;
                let internal = matches!(
                    ev.get("type").and_then(Value::as_str),
                    Some("auth_failed" | "denied" | "bound" | "unbound")
                );
                let blob_event = ev
                    .get("type")
                    .and_then(Value::as_str)
                    .is_some_and(|kind| kind.starts_with("blob_"));
                let output = if internal {
                    None
                } else if receipts {
                    who.and_then(|w| st.receipt_event(&ev, w))
                } else if blob_event {
                    who.filter(|w| st.visible(&ev, w)).map(|_| ev)
                } else if who.is_none_or(|w| st.visible(&ev, w)) {
                    Some(ev)
                } else {
                    None
                };
                Some((seq, output))
            })
            .collect()
    };
    for (seq, ev) in batch {
        if let Some(ev) = ev {
            write_line(
                stream,
                &json!({"jsonrpc": "2.0", "method": "event", "params": ev}),
            )?;
        }
        *cursor = seq;
    }
    Ok(())
}

/// Trata uma linha. Devolve a resposta e se a conexao deve ser fechada depois dela.
fn handle_line(line: &[u8], shared: &Shared, conn: &mut Conn) -> (Value, bool) {
    let req = match protocol::parse_request(line) {
        Ok(r) => r,
        Err((id, e)) => return (error_reply(id, &e), false),
    };
    if shared.cfg.auth_required {
        if req.method == "auth" {
            return auth_step(&req, shared, conn);
        }
        if req.method != "status" && conn.identity.is_none() {
            return (
                error_reply(req.id.clone(), &ApiError::unauthenticated()),
                false,
            );
        }
    }
    let who = conn.identity.clone();
    if req.method == "status"
        && conn.cursor.is_some()
        && conn.last_watch_renew.elapsed() >= Duration::from_secs(30)
        && let Some(name) = who.as_deref()
    {
        lock(&shared.bindings).renew_for_watch(name, Instant::now());
        conn.last_watch_renew = Instant::now();
    }
    match dispatch(&req.method, &req.params, shared, conn) {
        Ok(result) => (ok_reply(req.id.clone(), result), false),
        Err(e) => {
            if e.code == "forbidden" {
                record_denied(
                    shared,
                    conn,
                    &req.method,
                    &req.params,
                    conn.identity.as_deref(),
                );
            }
            (error_reply(req.id.clone(), &e), false)
        }
    }
}

/// Rastro de `forbidden` (spec F.2): so o metodo, o peer e o nome pretendido (se for nome valido).
fn record_denied(
    shared: &Shared,
    conn: &Conn,
    method: &str,
    p: &Map<String, Value>,
    who: Option<&str>,
) {
    let field = match method {
        "message.send" => "from",
        "message.list" | "message.take" => "to",
        "message.ack" => "by",
        "agent.bind" => "name",
        _ => return,
    };
    let claimed = if method == "agent.bind" {
        who.filter(|s| protocol::valid_name(s))
    } else {
        p.get(field)
            .and_then(Value::as_str)
            .filter(|s| protocol::valid_name(s))
    };
    let _ = lock(&shared.store).denied(method, conn.peer_uid, conn.peer_pid, claimed);
}

/// Handshake `auth {v, name, token}` (v1.1 B). Falha: `unauthenticated` identico nas 3 causas.
fn auth_step(req: &protocol::Request, shared: &Shared, conn: &mut Conn) -> (Value, bool) {
    let id = req.id.clone();
    if conn.identity.is_some() {
        let e = ApiError::new("invalid_request", "auth ja feito nesta conexao");
        return (error_reply(id, &e), false);
    }
    if let Err(e) = protocol::check_version(&req.params) {
        return (error_reply(id, &e), false);
    }
    let (name, token) = match protocol::parse_auth(&req.params) {
        Ok(x) => x,
        Err(e) => return (error_reply(id, &e), false),
    };
    let now = Instant::now();
    prune_pids(
        &mut shared.pids.lock().unwrap_or_else(|e| e.into_inner()),
        now,
    );
    if pid_locked(shared, conn.peer_pid, now) {
        return (error_reply(id, &ApiError::unauthenticated()), true);
    }
    let ok = auth::verify(&shared.auth_path, &name, &token).unwrap_or(false);
    if ok {
        // Sucesso limpa o historico de falhas e a trava do pid (spec F.3).
        shared
            .pids
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(&conn.peer_pid);
        conn.identity = Some(name.clone());
        return (ok_reply(id, json!({"v": 1, "name": name})), false);
    }
    let logged = protocol::valid_name(&name).then_some(name.as_str());
    {
        let mut st = lock(&shared.store);
        let _ = st.auth_failed(logged, conn.peer_uid, conn.peer_pid);
    }
    conn.failures += 1;
    let now_locked = pid_failure(shared, conn.peer_pid, now);
    let close = now_locked || conn.failures >= AUTH_MAX_FAILS;
    (error_reply(id, &ApiError::unauthenticated()), close)
}

fn pid_locked(shared: &Shared, pid: i32, now: Instant) -> bool {
    let mut m = shared.pids.lock().unwrap_or_else(|e| e.into_inner());
    let e = pid_entry(&mut m, pid, now);
    match e.locked_until {
        Some(t) if now < t => true,
        Some(_) => {
            e.locked_until = None;
            false
        }
        None => false,
    }
}

/// Registra uma falha do pid; devolve true se ele ficou trancado.
fn pid_failure(shared: &Shared, pid: i32, now: Instant) -> bool {
    let mut m = shared.pids.lock().unwrap_or_else(|e| e.into_inner());
    let e = pid_entry(&mut m, pid, now);
    while let Some(f) = e.fails.front() {
        if now.duration_since(*f) >= AUTH_WINDOW {
            e.fails.pop_front();
        } else {
            break;
        }
    }
    e.fails.push_back(now);
    if e.fails.len() >= AUTH_MAX_FAILS {
        e.fails.clear();
        e.locked_until = Some(now + AUTH_LOCK);
        true
    } else {
        false
    }
}

fn bind_agent(
    params: &Map<String, Value>,
    shared: &Shared,
    who: Option<&str>,
    peer_pid: i32,
) -> Result<Value, ApiError> {
    let Some(name) = who else {
        return Err(ApiError::unauthenticated());
    };
    protocol::check_name(name, "/name")?;
    if !shared.cfg.recipients.contains_key(name) {
        return Err(ApiError::new(
            "unknown_recipient",
            format!("destinatario '{name}' nao configurado"),
        ));
    }
    let params = protocol::parse_bind(params)?;
    let mux = if params.mux == "herdr" {
        Mux::Herdr
    } else {
        Mux::Tmux
    };
    if !binding::verify(&shared.cfg, name, mux, &params.pane, peer_pid) {
        return Err(ApiError::new(
            "forbidden",
            "painel nao comprovado para esta identidade",
        ));
    }
    let now = Instant::now();
    let mut bindings = lock(&shared.bindings);
    if let Some(old) = bindings.get(name).cloned() {
        lock(&shared.store).unbound(name, &old.mux.to_string(), &old.pane, "replaced")?;
        bindings.remove(name);
    }
    lock(&shared.store).bound(name, &mux.to_string(), &params.pane, peer_pid)?;
    bindings.replace(
        name.to_string(),
        Binding {
            mux,
            pane: params.pane.clone(),
            peer_pid,
            expires_at: now + TTL,
        },
    );
    Ok(json!({
        "v": 1,
        "name": name,
        "mux": mux.to_string(),
        "pane": params.pane,
        "ttl_s": TTL.as_secs()
    }))
}

fn dispatch(
    method: &str,
    p: &Map<String, Value>,
    shared: &Shared,
    conn: &mut Conn,
) -> Result<Value, ApiError> {
    let Conn {
        identity,
        connection,
        cursor,
        receipts: conn_receipts,
        peer_pid,
        ..
    } = conn;
    let who = identity.as_deref();
    let connection = *connection;
    let peer_pid = *peer_pid;
    if !protocol::METHODS.contains(&method) {
        return Err(ApiError::new(
            "method_not_found",
            format!("metodo '{method}' desconhecido"),
        ));
    }
    protocol::check_version(p)?;
    if method == "status" {
        protocol::reject_unknown(p, &[])?;
        let st = lock(&shared.store);
        return Ok(st.status_for(
            shared.started_at.elapsed().as_secs(),
            shared.connections.load(Ordering::SeqCst),
        ));
    }
    if method == "mailbox.counts" {
        protocol::reject_unknown(p, &["to"])?;
        let who = who.ok_or_else(ApiError::unauthenticated)?;
        return Ok(lock(&shared.store).mailbox_counts_for(who));
    }
    if method.starts_with("blob.") {
        return dispatch_blob(method, p, shared, who, connection);
    }
    match method {
        "agent.bind" => bind_agent(p, shared, who, peer_pid),
        "message.send" => {
            let mut sp = protocol::parse_send(p)?;
            let from = protocol::resolve(sp.from.clone(), who, "/from")?;
            protocol::check_name(&from, "/from")?;
            protocol::check_name(&sp.to, "/to")?;
            if sp.due_s.is_none() {
                sp.due_s = shared.cfg.due_defaults.get(&sp.kind).copied();
            }
            let (m, dup, closed) = lock(&shared.store).send(&from, &sp)?;
            Ok(json!({
                "v": 1,
                "id": m.id,
                "seq": m.seq,
                "state": m.state.as_str(),
                "duplicate": dup,
                "closed": closed,
                "registered_at": m.registered_at,
            }))
        }
        "message.list" => {
            let lp = protocol::parse_list(p)?;
            let to = protocol::resolve(lp.to.clone(), who, "/to")?;
            protocol::check_name(&to, "/to")?;
            if !shared.cfg.recipients.contains_key(&to) {
                return Err(ApiError::new(
                    "unknown_recipient",
                    format!("destinatario '{to}' nao configurado"),
                ));
            }
            let st = lock(&shared.store);
            let msgs: Vec<Value> = st
                .list(&to, lp.after_seq, lp.limit, lp.unresolved_only)
                .iter()
                .map(msg_json)
                .collect();
            Ok(json!({"v": 1, "messages": msgs, "head_seq": st.head()}))
        }
        "message.take" => {
            let tp = protocol::parse_take(p)?;
            let to = protocol::resolve(tp.to.clone(), who, "/to")?;
            protocol::check_name(&to, "/to")?;
            if !shared.cfg.recipients.contains_key(&to) {
                return Err(ApiError::new(
                    "unknown_recipient",
                    format!("destinatario '{to}' nao configurado"),
                ));
            }
            let mut st = lock(&shared.store);
            let taken = st.take(&to, tp.limit)?;
            let msgs: Vec<Value> = taken.iter().map(msg_json).collect();
            Ok(json!({"v": 1, "messages": msgs, "head_seq": st.head()}))
        }
        "message.ack" => {
            let ap = protocol::parse_ack(p)?;
            if ap.stage == "bloqueado" && (!shared.cfg.auth_required || who.is_none()) {
                return Err(ApiError::unauthenticated());
            }
            if ap.stage == "bloqueado" && !shared.cfg.blocked_receipts {
                return Err(ApiError::new(
                    "feature_disabled",
                    "blocked receipts desativados",
                ));
            }
            if ap.stage == "bloqueado"
                && ap.by.as_deref().is_some_and(|claimed| Some(claimed) != who)
            {
                return Err(ApiError::new("not_found", "mensagem nao encontrada"));
            }
            let by = if ap.stage == "bloqueado" {
                who.expect("bloqueado exige autenticacao").to_string()
            } else {
                protocol::resolve(ap.by.clone(), who, "/by")?
            };
            protocol::check_name(&by, "/by")?;
            let (m, dup) = lock(&shared.store).ack(&by, &ap)?;
            let mut v = msg_json(&m);
            if let Some(o) = v.as_object_mut() {
                o.insert("duplicate".into(), Value::Bool(dup));
            }
            Ok(v)
        }
        "message.get" => {
            let id = protocol::parse_get(p)?;
            let message = who
                .and_then(|name| lock(&shared.store).get_for(&id, name))
                .ok_or_else(|| ApiError::new("unknown_message", "mensagem nao encontrada"))?;
            let head_seq = lock(&shared.store).head();
            Ok(json!({"v": 1, "message": msg_json(&message), "head_seq": head_seq}))
        }
        "events.resume" => {
            if cursor.is_some() {
                return Err(ApiError::new(
                    "invalid_request",
                    "events.resume ja foi chamado nesta conexao",
                ));
            }
            protocol::reject_unknown(p, &["last_seq", "receipts"])?;
            *conn_receipts = protocol::opt_bool(p, "receipts")?.unwrap_or(false);
            let last = protocol::req_u64(p, "last_seq")?;
            let head = lock(&shared.store).head();
            if last > head {
                return Err(ApiError::invalid(
                    "/last_seq",
                    "last_seq maior que head_seq",
                ));
            }
            *cursor = Some(last);
            Ok(json!({"v": 1, "last_seq": last, "head_seq": head}))
        }
        _ => Err(ApiError::new("method_not_found", "metodo desconhecido")),
    }
}

fn dispatch_blob(
    method: &str,
    p: &Map<String, Value>,
    shared: &Shared,
    who: Option<&str>,
    connection: u64,
) -> Result<Value, ApiError> {
    let who = who
        .filter(|_| shared.cfg.auth_required)
        .ok_or_else(ApiError::unauthenticated)?;
    let mut blobs = lock(&shared.blobs);
    match method {
        "blob.put.begin" => {
            let to = protocol::opt_str(p, "to")?.unwrap_or("");
            let known = shared.cfg.recipients.contains_key(to);
            let mut emit = |kind: &'static str, fields: Vec<(&'static str, Value)>| {
                lock(&shared.store).blob_event(kind, fields)
            };
            blobs.begin(who, connection, p, known, &mut emit)
        }
        "blob.put.chunk" => blobs.chunk(who, connection, p),
        "blob.put.end" => {
            let mut emit = |kind: &'static str, fields: Vec<(&'static str, Value)>| {
                lock(&shared.store).blob_event(kind, fields)
            };
            blobs.end(who, connection, p, &mut emit)
        }
        "blob.get" => {
            let mut emit = |kind: &'static str, fields: Vec<(&'static str, Value)>| {
                lock(&shared.store).blob_event(kind, fields)
            };
            blobs.get(who, p, &mut emit)
        }
        "blob.rm" => {
            let mut emit = |kind: &'static str, fields: Vec<(&'static str, Value)>| {
                lock(&shared.store).blob_event(kind, fields)
            };
            blobs.remove(who, p, &mut emit)
        }
        "blob.list" => {
            let mut emit = |kind, fields| lock(&shared.store).blob_event(kind, fields);
            blobs.list(who, p, &mut emit)
        }
        _ => Err(ApiError::new(
            "method_not_found",
            "metodo de blob desconhecido",
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(fail: Option<Instant>, locked_until: Option<Instant>, last: Instant) -> PidAuth {
        PidAuth {
            fails: fail.into_iter().collect(),
            locked_until,
            last: Some(last),
        }
    }

    #[test]
    fn poda_remove_janela_expirada_e_mantem_recente_e_trancado() {
        let base = Instant::now();
        let now = base + Duration::from_secs(1000);
        let mut m = HashMap::new();
        m.insert(1, entry(Some(base), None, base));
        m.insert(2, entry(Some(now - Duration::from_secs(1)), None, now));
        m.insert(3, entry(None, Some(now + Duration::from_secs(100)), base));
        m.insert(4, entry(None, Some(base), base));
        prune_pids(&mut m, now);
        let mut keys: Vec<i32> = m.keys().copied().collect();
        keys.sort();
        assert_eq!(keys, vec![2, 3]);
    }

    #[test]
    fn teto_preserva_trancados_e_descarta_sem_trava_mais_antigos() {
        let base = Instant::now();
        let now = base + Duration::from_secs(1000);
        let extra = 50;
        let mut m = HashMap::new();
        m.insert(-1, entry(None, Some(now + Duration::from_secs(100)), base));
        for i in 0..(PID_CAP as i32 + extra) {
            let last = base + Duration::from_millis(i as u64);
            m.insert(i, entry(Some(now - Duration::from_secs(1)), None, last));
        }
        prune_pids(&mut m, now);
        assert_eq!(m.len(), PID_CAP);
        assert!(
            m.contains_key(&-1),
            "trava ativa nao pode sair antes das sem trava"
        );
        assert!(!m.contains_key(&0), "o mais antigo sem trava sai primeiro");
        assert!(m.contains_key(&(PID_CAP as i32 + extra - 1)));
    }
}

use crate::config::Config;
use serde_json::{Value, json};
use std::{
    collections::{BTreeMap, HashSet, VecDeque},
    fs,
    io::{BufRead, BufReader, Read, Write},
    os::unix::{
        fs::PermissionsExt,
        net::{UnixListener, UnixStream},
        process::CommandExt,
    },
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
        mpsc::{self, Receiver, SyncSender, TrySendError},
    },
    thread,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};
static STOP: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
extern "C" fn stop_signal(_: libc::c_int) {
    STOP.store(true, Ordering::Relaxed);
}

type Failure = (i32, String);
pub fn run(args: Vec<String>) -> Result<(), Failure> {
    if args.first().map(String::as_str) == Some("serve") {
        return serve_cli(&args[1..]);
    }
    cli(&args)
}
fn opt(args: &[String], name: &str) -> Option<String> {
    args.windows(2).find(|w| w[0] == name).map(|w| w[1].clone())
}
fn serve_cli(args: &[String]) -> Result<(), Failure> {
    let cfg = opt(args, "--config").ok_or((
        2,
        "uso: agentd serve --config F [--socket P] [--state-dir D]".into(),
    ))?;
    let c = Config::read(Path::new(&cfg)).map_err(|e| (1, e))?;
    let socket = opt(args, "--socket")
        .or_else(|| opt(args, "--state-dir").map(|p| format!("{p}/agentd.sock")))
        .ok_or((2, "socket ou state-dir obrigatorio".into()))?;
    serve(c, PathBuf::from(socket))
}

#[derive(Clone)]
struct Sub {
    tx: SyncSender<Value>,
    closed: Arc<AtomicBool>,
}
struct Rec {
    state: String,
    marks: Vec<String>,
    evidence: String,
    source: Option<String>,
    last_seen: Option<u64>,
    observed: Option<Instant>,
    observed_at: Option<u64>,
    last_known: Option<String>,
    cwd: Option<String>,
    terminal: Option<String>,
    absent: Option<Instant>,
    idle_key: Option<(String, u64)>,
    idle_first: Option<Instant>,
    idle_n: u64,
}
struct Core {
    cfg: Config,
    epoch: String,
    start: Instant,
    agents: BTreeMap<String, Rec>,
    seq: u64,
    ring: VecDeque<Value>,
    subs: Vec<Sub>,
    source_ok: bool,
    source_last: Option<u64>,
    source_error: Option<String>,
}
impl Core {
    fn wall() -> u64 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as u64
    }
    fn changed(&mut self, id: &str, old: (String, Vec<String>, String, Option<String>)) {
        let a = &self.agents[id];
        if old
            == (
                a.state.clone(),
                a.marks.clone(),
                a.evidence.clone(),
                a.source.clone(),
            )
        {
            return;
        }
        self.seq += 1;
        let e = json!({"v":1,"seq":self.seq,"epoch":self.epoch,"ts":Self::wall(),"type":"state_changed","id":id,"state":a.state,"marks":a.marks,"evidence":a.evidence,"source":a.source,"previous_state":old.0,"previous_marks":old.1});
        self.ring.push_back(e.clone());
        while self.ring.len() > self.cfg.ring {
            self.ring.pop_front();
        }
        self.subs.retain(|s| {
            if s.closed.load(Ordering::Relaxed) {
                return false;
            }
            match s.tx.try_send(e.clone()) {
                Ok(()) => true,
                Err(TrySendError::Full(_)) => {
                    s.closed.store(true, Ordering::Relaxed);
                    false
                }
                Err(TrySendError::Disconnected(_)) => false,
            }
        });
    }
    fn stale(&mut self) {
        let now = Instant::now();
        let ids: Vec<_> = self.agents.keys().cloned().collect();
        for id in ids {
            let old = {
                let a = &self.agents[&id];
                (
                    a.state.clone(),
                    a.marks.clone(),
                    a.evidence.clone(),
                    a.source.clone(),
                )
            };
            let a = self.agents.get_mut(&id).unwrap();
            if a.observed
                .is_some_and(|t| now.duration_since(t).as_millis() as u64 > self.cfg.stale_after_ms)
            {
                if !a.marks.contains(&"stale".into()) {
                    a.marks.push("stale".into());
                }
                if ["idle", "working", "blocked"].contains(&a.state.as_str()) {
                    a.last_known = Some(a.state.clone());
                    a.state = "unknown".into();
                }
            }
            self.changed(&id, old);
        }
    }
    fn agent_json(&self, id: &str) -> Value {
        let a = &self.agents[id];
        let age = a
            .observed
            .map(|t| Instant::now().duration_since(t).as_millis() as u64);
        json!({"v":1,"id":id,"kind":self.cfg.agents[id].kind,"state":a.state,"marks":a.marks,"evidence":a.evidence,"source":a.source,"last_seen":a.last_seen,"observed_at":a.observed_at,"freshness_ms":age,"last_known":a.last_known,"cwd":a.cwd})
    }
    fn snapshot(&mut self) {
        self.stale();
    }
}
fn mark(a: &mut Rec, m: &str) {
    if !a.marks.iter().any(|x| x == m) {
        a.marks.push(m.into());
        a.marks.sort();
    }
}
fn unmark(a: &mut Rec, m: &str) {
    a.marks.retain(|x| x != m)
}
fn serve(cfg: Config, socket: PathBuf) -> Result<(), Failure> {
    STOP.store(false, Ordering::Relaxed);
    unsafe {
        libc::signal(
            libc::SIGTERM,
            stop_signal as *const () as libc::sighandler_t,
        );
        libc::signal(libc::SIGINT, stop_signal as *const () as libc::sighandler_t);
    }
    let parent = socket
        .parent()
        .ok_or((1, "caminho de socket invalido".into()))?;
    fs::create_dir_all(parent).map_err(|e| (1, e.to_string()))?;
    fs::set_permissions(parent, fs::Permissions::from_mode(0o700))
        .map_err(|e| (1, e.to_string()))?;
    if socket.exists() {
        fs::remove_file(&socket).map_err(|e| (1, e.to_string()))?;
    }
    let listener = UnixListener::bind(&socket).map_err(|e| (1, e.to_string()))?;
    fs::set_permissions(&socket, fs::Permissions::from_mode(0o600))
        .map_err(|e| (1, e.to_string()))?;
    listener.set_nonblocking(true).ok();
    let epoch = (SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos() as u64
        ^ (std::process::id() as u64).rotate_left(31))
    .to_string();
    let agents = cfg
        .agents
        .keys()
        .map(|id| {
            (
                id.clone(),
                Rec {
                    state: "unknown".into(),
                    marks: vec!["error".into()],
                    evidence: "none".into(),
                    source: None,
                    last_seen: None,
                    observed: None,
                    observed_at: None,
                    last_known: None,
                    cwd: None,
                    terminal: None,
                    absent: None,
                    idle_key: None,
                    idle_first: None,
                    idle_n: 0,
                },
            )
        })
        .collect();
    let core = Arc::new(Mutex::new(Core {
        cfg: cfg.clone(),
        epoch,
        start: Instant::now(),
        agents,
        seq: 0,
        ring: VecDeque::new(),
        subs: vec![],
        source_ok: false,
        source_last: None,
        source_error: Some("source_spawn_failed".into()),
    }));
    {
        let c = core.clone();
        thread::spawn(move || poller(c));
    }
    {
        let c = core.clone();
        thread::spawn(move || {
            loop {
                thread::sleep(Duration::from_secs(1));
                if STOP.load(Ordering::Relaxed) {
                    break;
                }
                c.lock().unwrap().snapshot();
            }
        });
    }
    eprintln!("agentd: pronto");
    while !STOP.load(Ordering::Relaxed) {
        match listener.accept() {
            Ok((s, _)) => {
                let c = core.clone();
                thread::spawn(move || handle(s, c));
            }
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                thread::sleep(Duration::from_millis(20))
            }
            Err(e) => return Err((1, e.to_string())),
        }
    }
    let _ = fs::remove_file(socket);
    Ok(())
}

#[derive(Clone)]
struct Info {
    pane: String,
    agent: Option<String>,
    status: String,
    cwd: Option<String>,
    terminal: Option<String>,
    seq: Option<u64>,
    launch: bool,
    skipped: bool,
}
struct Snapshot {
    agents: Vec<Info>,
    started: Instant,
    started_wall: u64,
}
fn source(cfg: &Config) -> Result<Snapshot, String> {
    let started = Instant::now();
    let started_wall = Core::wall();
    let mut cmd = Command::new(&cfg.herdr_bin);
    cmd.arg("agent")
        .arg("list")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    unsafe {
        cmd.pre_exec(|| {
            if libc::setpgid(0, 0) == -1 {
                return Err(std::io::Error::last_os_error());
            }
            Ok(())
        });
    }
    let mut child = cmd.spawn().map_err(|_| "source_spawn_failed".to_string())?;
    let pid = child.id() as i32;
    let mut out = child.stdout.take().unwrap();
    let mut err = child.stderr.take().unwrap();
    let (out_tx, out_rx) = mpsc::channel();
    let oh = thread::spawn(move || {
        let mut b = Vec::new();
        let mut chunk = [0u8; 8192];
        loop {
            match out.read(&mut chunk) {
                Ok(0) | Err(_) => break,
                Ok(n) => {
                    if b.len() < 1024 * 1024 + 1 {
                        let keep = (1024 * 1024 + 1 - b.len()).min(n);
                        b.extend_from_slice(&chunk[..keep]);
                    }
                }
            }
        }
        let _ = out_tx.send(b);
    });
    let eh = thread::spawn(move || {
        let mut b = [0u8; 8192];
        let mut total = 0usize;
        loop {
            match err.read(&mut b) {
                Ok(0) | Err(_) => break,
                Ok(n) => {
                    total += n;
                    if total >= 65536 {
                        break;
                    }
                }
            }
        }
    });
    let start = Instant::now();
    let mut status = None;
    let mut output = None;
    let timed_out = loop {
        if status.is_none()
            && let Ok(Some(st)) = child.try_wait()
        {
            status = Some(st.success());
        }
        if output.is_none() {
            output = out_rx.try_recv().ok();
        }
        if status.is_some() && output.is_some() {
            break false;
        }
        if start.elapsed() > Duration::from_millis(cfg.source_timeout_ms) {
            unsafe {
                libc::kill(-pid, libc::SIGKILL);
            }
            let _ = child.kill();
            let _ = child.wait();
            break true;
        }
        thread::sleep(Duration::from_millis(5));
    };
    if output.is_none() {
        output = out_rx.recv_timeout(Duration::from_millis(100)).ok();
    }
    let b = output.unwrap_or_default();
    let _ = oh.join();
    let _ = eh.join();
    if timed_out {
        return Err("source_timeout".into());
    }
    if status == Some(false) {
        return Err("source_exit_nonzero".into());
    }
    if b.len() > 1024 * 1024 {
        return Err("source_too_large".into());
    }
    let v: Value = serde_json::from_slice(&b).map_err(|_| "source_bad_json".to_string())?;
    let arr = v
        .get("result")
        .and_then(|x| x.get("agents"))
        .and_then(Value::as_array)
        .ok_or("source_bad_json")?;
    let mut seen = HashSet::new();
    let mut out = Vec::new();
    for x in arr {
        let pane = x
            .get("pane_id")
            .and_then(Value::as_str)
            .ok_or("source_bad_json")?
            .to_owned();
        if !seen.insert(pane.clone()) {
            return Err("duplicate_pane".into());
        }
        out.push(Info {
            pane,
            agent: x.get("agent").and_then(Value::as_str).map(str::to_owned),
            status: x
                .get("agent_status")
                .and_then(Value::as_str)
                .unwrap_or("")
                .into(),
            cwd: x.get("cwd").and_then(Value::as_str).map(str::to_owned),
            terminal: x
                .get("terminal_id")
                .and_then(Value::as_str)
                .map(str::to_owned),
            seq: x.get("state_change_seq").and_then(Value::as_u64),
            launch: x
                .get("launch_pending")
                .and_then(Value::as_bool)
                .unwrap_or(false),
            skipped: x
                .get("screen_detection_skipped")
                .and_then(Value::as_bool)
                .unwrap_or(false),
        });
    }
    Ok(Snapshot {
        agents: out,
        started,
        started_wall,
    })
}
fn poller(core: Arc<Mutex<Core>>) {
    loop {
        let (cfg, cycle) = {
            let c = core.lock().unwrap();
            (c.cfg.clone(), c.cfg.poll_ms)
        };
        let start = Instant::now();
        let read = source(&cfg);
        {
            let mut c = core.lock().unwrap();
            match read {
                Err(code) => {
                    c.source_ok = false;
                    c.source_error = Some(code);
                    let ids: Vec<_> = c.agents.keys().cloned().collect();
                    for id in ids {
                        let old = {
                            let a = &c.agents[&id];
                            (
                                a.state.clone(),
                                a.marks.clone(),
                                a.evidence.clone(),
                                a.source.clone(),
                            )
                        };
                        {
                            let a = c.agents.get_mut(&id).unwrap();
                            if ["idle", "working", "blocked"].contains(&a.state.as_str()) {
                                a.last_known = Some(a.state.clone());
                            }
                            a.state = "unknown".into();
                            mark(a, "error");
                            a.idle_n = 0;
                            a.idle_first = None;
                            a.idle_key = None;
                            unmark(a, "idle_unconfirmed");
                        }
                        c.changed(&id, old);
                    }
                }
                Ok(snapshot) => {
                    c.source_ok = true;
                    c.source_last = Some(Core::wall());
                    c.source_error = None;
                    let ids: Vec<_> = c.agents.keys().cloned().collect();
                    let now = Instant::now();
                    for id in ids {
                        let ac = c.cfg.agents[&id].clone();
                        let idle_confirm_polls = c.cfg.idle_confirm_polls;
                        let idle_confirm_ms = c.cfg.idle_confirm_ms;
                        let gone_after_s = c.cfg.gone_after_s;
                        let found = snapshot.agents.iter().find(|i| i.pane == ac.pane).cloned();
                        let old = {
                            let a = &c.agents[&id];
                            (
                                a.state.clone(),
                                a.marks.clone(),
                                a.evidence.clone(),
                                a.source.clone(),
                            )
                        };
                        let mut source_error = None;
                        let mut terminal_changed = false;
                        if let Some(i) = found {
                            let a = c.agents.get_mut(&id).unwrap();
                            if i.agent.as_deref() != Some(ac.kind.as_str())
                                && i.agent.as_deref() != Some(id.as_str())
                            {
                                if ["idle", "working", "blocked"].contains(&a.state.as_str()) {
                                    a.last_known = Some(a.state.clone());
                                }
                                a.state = "unknown".into();
                                mark(a, "error");
                                a.idle_n = 0;
                                a.idle_key = None;
                                a.idle_first = None;
                                unmark(a, "idle_unconfirmed");
                                source_error = Some("kind_mismatch".to_string());
                            } else {
                                unmark(a, "error");
                                unmark(a, "stale");
                                a.last_seen = Some(Core::wall());
                                a.observed = Some(snapshot.started);
                                a.observed_at = Some(snapshot.started_wall);
                                a.evidence = "observed".into();
                                a.source = Some("herdr".into());
                                a.cwd = i.cwd.clone();
                                if a.terminal
                                    .as_ref()
                                    .is_some_and(|t| Some(t.as_str()) != i.terminal.as_deref())
                                {
                                    if ["idle", "working", "blocked"].contains(&a.state.as_str()) {
                                        a.last_known = Some(a.state.clone());
                                    }
                                    a.state = "unknown".into();
                                    mark(a, "error");
                                    terminal_changed = true;
                                    a.idle_n = 0;
                                    a.idle_key = None;
                                    a.idle_first = None;
                                }
                                a.terminal = i.terminal.clone();
                                let mapped = match i.status.as_str() {
                                    "working" => Some("working"),
                                    "blocked" => Some("blocked"),
                                    "idle" | "done" => Some("idle"),
                                    "unknown" => Some("unknown"),
                                    _ => {
                                        if ["idle", "working", "blocked"]
                                            .contains(&a.state.as_str())
                                        {
                                            a.last_known = Some(a.state.clone());
                                        }
                                        a.state = "unknown".into();
                                        mark(a, "error");
                                        source_error = Some("status_unmapped".to_string());
                                        None
                                    }
                                };
                                if terminal_changed {
                                    // A reused terminal needs one subsequent valid snapshot before publication.
                                } else if i.launch || i.skipped {
                                    a.state = "unknown".into();
                                    a.idle_n = 0;
                                    unmark(a, "idle_unconfirmed");
                                } else if let Some(st) = mapped {
                                    if st == "idle" {
                                        let key = i.terminal.clone().zip(i.seq);
                                        let same = key.is_some() && a.idle_key == key;
                                        if !same {
                                            a.idle_n = 0;
                                            a.idle_first = Some(now);
                                            a.idle_key = key.clone();
                                        }
                                        a.idle_n += 1;
                                        mark(a, "idle_unconfirmed");
                                        if a.idle_n >= idle_confirm_polls
                                            && key.is_some()
                                            && a.idle_first.is_some_and(|t| {
                                                now.duration_since(t).as_millis() as u64
                                                    >= idle_confirm_ms
                                            })
                                        {
                                            a.state = "idle".into();
                                            unmark(a, "idle_unconfirmed");
                                            a.last_known = None;
                                        }
                                    } else {
                                        a.state = st.into();
                                        a.idle_n = 0;
                                        a.idle_first = None;
                                        a.idle_key = None;
                                        unmark(a, "idle_unconfirmed");
                                        a.last_known = None;
                                    }
                                }
                                a.absent = None;
                            }
                        } else {
                            let a = c.agents.get_mut(&id).unwrap();
                            if ["idle", "working", "blocked"].contains(&a.state.as_str()) {
                                a.last_known = Some(a.state.clone());
                            }
                            a.idle_n = 0;
                            a.idle_first = None;
                            a.idle_key = None;
                            unmark(a, "idle_unconfirmed");
                            mark(a, "error");
                            if a.absent.is_none() {
                                a.absent = Some(now)
                            }
                            a.state = if now.duration_since(a.absent.unwrap()).as_secs()
                                >= gone_after_s
                            {
                                "gone".into()
                            } else {
                                "unknown".into()
                            };
                        }
                        if source_error.is_some() {
                            c.source_error = source_error;
                        }
                        c.changed(&id, old);
                    }
                }
            }
        }
        thread::sleep(Duration::from_millis(cycle).saturating_sub(start.elapsed()));
    }
}
fn reply(id: Value, result: Result<Value, (String, Value)>) -> Value {
    match result {
        Ok(v) => json!({"jsonrpc":"2.0","id":id,"result":v}),
        Err((code, data)) => {
            json!({"jsonrpc":"2.0","id":id,"error":{"code":code,"message":code,"data":data}})
        }
    }
}
fn err(code: &str) -> (String, Value) {
    (code.into(), Value::Null)
}
fn valid_params(p: &Value, allowed: &[&str]) -> Result<(), (String, Value)> {
    let o = p.as_object().ok_or_else(|| err("invalid_envelope"))?;
    for k in o.keys() {
        if !allowed.contains(&k.as_str()) {
            return Err((
                "invalid_envelope".into(),
                json!({"path":format!("params.{k}")}),
            ));
        }
    }
    if o.get("v").and_then(Value::as_u64) != Some(1) {
        return Err(err("unsupported_version"));
    }
    Ok(())
}
fn dispatch(
    req: &Value,
    c: &mut Core,
    sub: &mut Option<(Receiver<Value>, Arc<AtomicBool>)>,
) -> Value {
    let id = req.get("id").cloned().unwrap_or(Value::Null);
    if req.is_array() || !req.is_object() || !req.get("id").is_some() {
        return reply(id, Err(err("invalid_request")));
    }
    let m = req.get("method").and_then(Value::as_str).unwrap_or("");
    let p = req.get("params").cloned().unwrap_or(Value::Null);
    let result: Result<Value, (String, Value)> = match m {
        "agent.get" => {
            if let Err(e) = valid_params(&p, &["v", "id"]) {
                Err(e)
            } else {
                let id = p["id"].as_str().unwrap_or("");
                if !c.agents.contains_key(id) {
                    Err(err("unknown_agent"))
                } else {
                    c.snapshot();
                    Ok(json!({"v":1,"agent":c.agent_json(id),"epoch":c.epoch,"head_seq":c.seq}))
                }
            }
        }
        "agent.list" => {
            if let Err(e) = valid_params(&p, &["v", "kind"]) {
                Err(e)
            } else if p
                .get("kind")
                .is_some_and(|x| !matches!(x.as_str(), Some("claude" | "codex")))
            {
                Err(err("invalid_envelope"))
            } else {
                c.snapshot();
                let a: Vec<_> = c
                    .agents
                    .keys()
                    .filter(|id| {
                        p.get("kind")
                            .is_none_or(|k| k.as_str() == Some(c.cfg.agents[*id].kind.as_str()))
                    })
                    .map(|id| c.agent_json(id))
                    .collect();
                Ok(json!({"v":1,"agents":a,"epoch":c.epoch,"head_seq":c.seq}))
            }
        }
        "status" => {
            if let Err(e) = valid_params(&p, &["v"]) {
                Err(e)
            } else {
                c.snapshot();
                let agents:serde_json::Map<String,Value>=c.agents.iter().map(|(id,a)|(id.clone(),json!({"state":a.state,"marks":a.marks,"source":a.source,"last_seen":a.last_seen}))).collect();
                Ok(
                    json!({"v":1,"epoch":c.epoch,"head_seq":c.seq,"uptime_s":c.start.elapsed().as_secs(),"agents":agents,"sources":{"herdr":{"ok":c.source_ok,"last_ok_at":c.source_last,"last_error":c.source_error}}}),
                )
            }
        }
        "events.resume" => {
            if let Err(e) = valid_params(&p, &["v", "epoch", "last_seq"]) {
                Err(e)
            } else if sub.is_some() {
                Err(err("invalid_request"))
            } else {
                let ep = p["epoch"].as_str().unwrap_or("");
                let last = p["last_seq"].as_u64();
                if last.is_none() {
                    Err(err("invalid_envelope"))
                } else if ep != c.epoch {
                    Err(err("epoch_mismatch"))
                } else {
                    let Some(last) = last else {
                        return reply(id, Err(err("invalid_envelope")));
                    };
                    if last > c.seq {
                        Err(err("invalid_envelope"))
                    } else {
                        let oldest = c
                            .ring
                            .front()
                            .and_then(|e| e["seq"].as_u64())
                            .unwrap_or(c.seq.saturating_add(1));
                        if (c.ring.is_empty() && last != c.seq)
                            || (!c.ring.is_empty() && last.saturating_add(1) < oldest)
                        {
                            Err(("resume_gap".into(), json!({"oldest_seq":oldest})))
                        } else {
                            let replay: Vec<_> = c
                                .ring
                                .iter()
                                .filter(|e| e["seq"].as_u64().unwrap_or(0) > last)
                                .cloned()
                                .collect();
                            let (tx, rx) = mpsc::sync_channel(256);
                            let closed = Arc::new(AtomicBool::new(false));
                            for e in replay {
                                let _ = tx.try_send(e);
                            }
                            c.subs.push(Sub {
                                tx,
                                closed: closed.clone(),
                            });
                            *sub = Some((rx, closed));
                            Ok(json!({"v":1,"epoch":c.epoch,"head_seq":c.seq}))
                        }
                    }
                }
            }
        }
        _ => Err(err("method_not_found")),
    };
    reply(id, result)
}
fn handle(stream: UnixStream, core: Arc<Mutex<Core>>) {
    let send_buffer: libc::c_int = 8192;
    unsafe {
        libc::setsockopt(
            std::os::fd::AsRawFd::as_raw_fd(&stream),
            libc::SOL_SOCKET,
            libc::SO_SNDBUF,
            &send_buffer as *const _ as *const _,
            std::mem::size_of_val(&send_buffer) as libc::socklen_t,
        );
    }
    let mut peer = libc::ucred {
        pid: 0,
        uid: 0,
        gid: 0,
    };
    let mut len = std::mem::size_of::<libc::ucred>() as libc::socklen_t;
    let rc = unsafe {
        libc::getsockopt(
            std::os::fd::AsRawFd::as_raw_fd(&stream),
            libc::SOL_SOCKET,
            libc::SO_PEERCRED,
            &mut peer as *mut _ as *mut _,
            &mut len,
        )
    };
    let forbidden = rc == 0 && peer.uid != unsafe { libc::geteuid() };
    let writer = Arc::new(Mutex::new(stream.try_clone().unwrap()));
    let mut reader = BufReader::new(stream);
    let mut resumed: Option<(Receiver<Value>, Arc<AtomicBool>)> = None;
    let mut resumed_once = false;
    loop {
        let mut line = Vec::new();
        match reader.read_until(b'\n', &mut line) {
            Ok(0) => return,
            Err(_) => return,
            Ok(_) => {}
        }
        if line.len() > 1024 * 1024 {
            let v = reply(Value::Null, Err(err("invalid_request")));
            let _ = writeln!(writer.lock().unwrap(), "{v}");
            return;
        }
        if line.last() == Some(&b'\n') {
            line.pop();
        }
        let parsed = std::str::from_utf8(&line)
            .map_err(|_| ())
            .and_then(|s| serde_json::from_str::<Value>(s).map_err(|_| ()));
        let resp = match parsed {
            Err(_) => reply(Value::Null, Err(err("parse_error"))),
            Ok(ref v) if forbidden => reply(
                v.get("id").cloned().unwrap_or(Value::Null),
                Err(err("forbidden")),
            ),
            Ok(v)
                if resumed_once
                    && v.get("method").and_then(Value::as_str) == Some("events.resume") =>
            {
                reply(
                    v.get("id").cloned().unwrap_or(Value::Null),
                    Err(err("invalid_request")),
                )
            }
            Ok(v) => {
                let mut c = core.lock().unwrap();
                dispatch(&v, &mut c, &mut resumed)
            }
        };
        if writeln!(writer.lock().unwrap(), "{resp}").is_err() {
            return;
        }
        if let Some((rx, closed)) = resumed.take() {
            resumed_once = true;
            let writer = writer.clone();
            let core = core.clone();
            thread::spawn(move || {
                loop {
                    match rx.recv_timeout(Duration::from_millis(50)) {
                        Ok(e) => {
                            let n = json!({"jsonrpc":"2.0","method":"event","params":e});
                            if writeln!(writer.lock().unwrap(), "{n}").is_err() {
                                break;
                            }
                        }
                        Err(mpsc::RecvTimeoutError::Disconnected) => {
                            if closed.load(Ordering::Relaxed) {
                                let n = json!({"jsonrpc":"2.0","method":"closed","params":{"reason":"slow_consumer","last_seq":core.lock().unwrap().seq}});
                                let _ = writeln!(writer.lock().unwrap(), "{n}");
                            }
                            break;
                        }
                        Err(mpsc::RecvTimeoutError::Timeout) if closed.load(Ordering::Relaxed) => {
                            let n = json!({"jsonrpc":"2.0","method":"closed","params":{"reason":"slow_consumer","last_seq":core.lock().unwrap().seq}});
                            let _ = writeln!(writer.lock().unwrap(), "{n}");
                            break;
                        }
                        Err(mpsc::RecvTimeoutError::Timeout) => {}
                    }
                }
            });
        }
    }
}
fn socket_for(args: &[String]) -> Option<String> {
    opt(args, "--socket")
        .or_else(|| std::env::var("AGENTD_SOCKET").ok())
        .or_else(|| opt(args, "--state-dir").map(|p| format!("{p}/agentd.sock")))
        .or_else(|| {
            std::env::var("AGENTD_STATE_DIR")
                .ok()
                .map(|p| format!("{p}/agentd.sock"))
        })
}
fn cli(args: &[String]) -> Result<(), Failure> {
    let cmd = args.first().map(String::as_str).unwrap_or("");
    let json_mode = args.iter().any(|x| x == "--json");
    if !["get", "list", "watch", "health", "status"].contains(&cmd) {
        return Err((2, "uso: agentd get|list|watch|health|status".into()));
    }
    if cmd == "watch" && (args.contains(&"--since".into()) != args.contains(&"--epoch".into())) {
        return Err((2, "--since e --epoch devem aparecer juntos".into()));
    }
    let Some(sock) = socket_for(args) else {
        return output(
            json_mode,
            false,
            "usage",
            "socket nao configurado",
            Value::Null,
            2,
        );
    };
    if cmd == "watch" {
        return watch(&sock, args, json_mode);
    }
    let method = match cmd {
        "get" => "agent.get",
        "list" => "agent.list",
        "health" | "status" => "status",
        _ => unreachable!(),
    };
    let mut p = json!({"v":1});
    if cmd == "get" {
        let id = args
            .get(1)
            .filter(|s| !s.starts_with('-'))
            .ok_or((2, "uso: agentd get ID".into()))?;
        p["id"] = json!(id)
    }
    if cmd == "list"
        && let Some(k) = opt(args, "--kind")
    {
        p["kind"] = json!(k)
    }
    let res = rpc_path(&sock, method, p);
    let Ok(r) = res else {
        return output(
            json_mode,
            false,
            "daemon_unavailable",
            "daemon indisponivel",
            Value::Null,
            4,
        );
    };
    if let Some(e) = r.get("error") {
        let code = e["code"].as_str().unwrap_or("internal_error");
        let exit = if code == "unknown_agent" { 3 } else { 1 };
        return output(
            json_mode,
            false,
            code,
            code,
            e.get("data").cloned().unwrap_or(Value::Null),
            exit,
        );
    }
    let mut data = r["result"].clone();
    let healthy = cmd != "health" || data["sources"]["herdr"]["ok"] == true;
    if cmd == "get" {
        data = data["agent"].clone()
    }
    if cmd == "health" {
        if !healthy {
            return output(
                json_mode,
                false,
                "source_error",
                "fonte herdr falhou",
                data,
                1,
            );
        }
        return output(json_mode, true, "ok", "ok", data, 0);
    }
    if json_mode {
        return output(json_mode, true, "ok", "ok", data, 0);
    }
    if cmd == "status" {
        println!("{}", data);
        return Ok(());
    }
    let agents = if cmd == "get" {
        vec![data]
    } else {
        data["agents"].as_array().cloned().unwrap_or_default()
    };
    for a in agents {
        let age = a["freshness_ms"].as_u64().unwrap_or(0) / 1000;
        let mut state = a["state"].as_str().unwrap_or("unknown").to_string();
        if let Some(m) = a["marks"].as_array() {
            let ms: Vec<_> = m.iter().filter_map(Value::as_str).collect();
            if !ms.is_empty() {
                state.push(',');
                state.push_str(&ms.join(","));
            }
        }
        let esc = |v: &Value| {
            v.as_str()
                .unwrap_or("-")
                .chars()
                .map(|c| if c.is_control() { '�' } else { c })
                .collect::<String>()
        };
        println!(
            "{} {} {} {} idade={}s",
            esc(&a["id"]),
            state,
            esc(&a["evidence"]),
            esc(&a["source"]),
            age
        )
    }
    Ok(())
}
fn output(
    _json_mode: bool,
    ok: bool,
    code: &str,
    message: &str,
    data: Value,
    exit: i32,
) -> Result<(), Failure> {
    println!(
        "{}",
        json!({"schema_version":1,"ok":ok,"code":code,"message":message,"data":data})
    );
    if exit == 0 {
        Ok(())
    } else {
        Err((exit, code.to_string()))
    }
}
fn rpc_path(path: &str, method: &str, params: Value) -> std::io::Result<Value> {
    let mut s = UnixStream::connect(path)?;
    s.set_read_timeout(Some(Duration::from_secs(3)))?;
    writeln!(
        s,
        "{}",
        json!({"jsonrpc":"2.0","id":1,"method":method,"params":params})
    )?;
    let mut r = BufReader::new(s);
    let mut line = String::new();
    r.read_line(&mut line)?;
    Ok(serde_json::from_str(&line).unwrap_or(Value::Null))
}
fn watch(path: &str, args: &[String], json_mode: bool) -> Result<(), Failure> {
    let since = opt(args, "--since")
        .and_then(|x| x.parse::<u64>().ok())
        .unwrap_or(0);
    let epoch = opt(args, "--epoch");
    let mut s = UnixStream::connect(path).map_err(|_| (4, "daemon indisponivel".into()))?;
    writeln!(
        s,
        "{}",
        json!({"jsonrpc":"2.0","id":1,"method":"agent.list","params":{"v":1}})
    )
    .map_err(|e| (4, e.to_string()))?;
    let mut r = BufReader::new(s.try_clone().unwrap());
    let mut l = String::new();
    r.read_line(&mut l).map_err(|e| (4, e.to_string()))?;
    let list: Value = serde_json::from_str(&l).unwrap_or(Value::Null);
    let ep = epoch.unwrap_or_else(|| list["result"]["epoch"].as_str().unwrap_or("").to_string());
    let seq = if args.iter().any(|x| x == "--since") {
        since
    } else {
        list["result"]["head_seq"].as_u64().unwrap_or(0)
    };
    writeln!(s,"{}",json!({"jsonrpc":"2.0","id":2,"method":"events.resume","params":{"v":1,"epoch":ep,"last_seq":seq}})).map_err(|e|(4,e.to_string()))?;
    l.clear();
    r.read_line(&mut l).map_err(|e| (4, e.to_string()))?;
    let v: Value = serde_json::from_str(&l).unwrap_or(Value::Null);
    if v.get("error").is_some() {
        return Err((
            1,
            v["error"]["code"].as_str().unwrap_or("resume_error").into(),
        ));
    }
    loop {
        l.clear();
        match r.read_line(&mut l) {
            Ok(0) => break,
            Err(_) => break,
            Ok(_) => {
                let v: Value = serde_json::from_str(&l).unwrap_or(Value::Null);
                if v["method"] == "event" {
                    let d = v["params"].clone();
                    if json_mode {
                        println!(
                            "{}",
                            json!({"schema_version":1,"ok":true,"code":"ok","message":"ok","data":d})
                        )
                    } else {
                        println!(
                            "{} {} {}",
                            d["seq"],
                            d["id"].as_str().unwrap_or(""),
                            d["state"].as_str().unwrap_or("unknown")
                        );
                    }
                }
            }
        }
    }
    Ok(())
}

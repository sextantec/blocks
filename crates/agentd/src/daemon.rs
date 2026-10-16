use crate::{
    config::{Config, Selector},
    rules::Rules,
    tmux,
};
use serde_json::{Value, json};
use std::{
    collections::{BTreeMap, HashMap, HashSet, VecDeque},
    fs,
    io::{BufRead, BufReader, Write},
    os::unix::{
        fs::PermissionsExt,
        net::{UnixListener, UnixStream},
    },
    path::{Path, PathBuf},
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
    let rules = Rules::load(&c.rules_file).map_err(|e| (1, e))?;
    if c.agents
        .values()
        .any(|a| !rules.kinds.contains_key(&a.kind))
    {
        return Err((1, "kind sem regras".into()));
    }
    let socket = opt(args, "--socket")
        .or_else(|| opt(args, "--state-dir").map(|p| format!("{p}/agentd.sock")))
        .ok_or((2, "socket ou state-dir obrigatorio".into()))?;
    serve(c, rules, PathBuf::from(socket))
}

#[derive(Clone)]
struct Sub {
    tx: SyncSender<Value>,
    closed: Arc<AtomicBool>,
}
struct Rec {
    kind: String,
    origin: String,
    dynamic: bool,
    identity: Option<(String, u32)>,
    gone_since: Option<Instant>,
    state: String,
    marks: Vec<String>,
    evidence: String,
    source: Option<String>,
    last_seen: Option<u64>,
    observed: Option<Instant>,
    observed_at: Option<u64>,
    last_known: Option<String>,
    cwd: Option<String>,
    terminal: Option<(String, u32)>,
    blocked_reason: Option<String>,
    rule: Option<String>,
    input: Option<String>,
    since: Option<u64>,
    absent: Option<Instant>,
    idle_key: Option<(String, u32, u64, String)>,
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
    rules: Rules,
    event_mode: String,
    last_output: BTreeMap<String, Instant>,
    discover_found: usize,
    discover_ignored: usize,
}
impl Core {
    fn changed(
        &mut self,
        id: &str,
        old: (String, Vec<String>, String, Option<String>, Option<String>),
    ) {
        if old.0 != self.agents[id].state {
            self.agents.get_mut(id).unwrap().since = Some(crate::time::wall_ms());
        }
        let a = &self.agents[id];
        if old
            == (
                a.state.clone(),
                a.marks.clone(),
                a.evidence.clone(),
                a.source.clone(),
                a.blocked_reason.clone(),
            )
        {
            return;
        }
        self.seq += 1;
        let e = json!({"v":1,"seq":self.seq,"epoch":self.epoch,"ts":crate::time::wall_ms(),"type":"state_changed","id":id,"origin":a.origin,"state":a.state,"marks":a.marks,"evidence":a.evidence,"source":a.source,"since":a.since,"blocked_reason":a.blocked_reason,"previous_state":old.0,"previous_marks":old.1});
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
                    a.blocked_reason.clone(),
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
        json!({"v":1,"id":id,"kind":a.kind,"origin":a.origin,"state":a.state,"marks":a.marks,"evidence":a.evidence,"source":a.source,"last_seen":a.last_seen,"observed_at":a.observed_at,"freshness_ms":age,"last_known":a.last_known,"cwd":a.cwd,"since":a.since,"blocked_reason":a.blocked_reason,"rule":a.rule,"input":a.input})
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
fn serve(cfg: Config, rules: Rules, socket: PathBuf) -> Result<(), Failure> {
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
                    kind: cfg.agents[id].kind.clone(),
                    origin: "configured".into(),
                    dynamic: false,
                    identity: None,
                    gone_since: None,
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
                    blocked_reason: None,
                    rule: None,
                    input: None,
                    since: None,
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
        rules,
        event_mode: if cfg.events {
            "degraded".into()
        } else {
            "off".into()
        },
        last_output: BTreeMap::new(),
        discover_found: 0,
        discover_ignored: 0,
    }));
    let (ready_tx, ready_rx) = mpsc::sync_channel(1);
    {
        let c = core.clone();
        thread::spawn(move || poller(c, ready_tx));
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
    let _ = ready_rx.recv_timeout(Duration::from_millis(
        cfg.source_timeout_ms.saturating_mul(2).min(5000),
    ));
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

struct EventClient {
    child: std::process::Child,
    _stdin: std::process::ChildStdin,
}
struct Retry {
    attempts: u32,
    after: Instant,
}
fn old(a: &Rec) -> (String, Vec<String>, String, Option<String>, Option<String>) {
    (
        a.state.clone(),
        a.marks.clone(),
        a.evidence.clone(),
        a.source.clone(),
        a.blocked_reason.clone(),
    )
}
fn source_failure(c: &mut Core, code: &str) {
    c.source_ok = false;
    c.source_error = Some(code.into());
    let ids: Vec<_> = c.agents.keys().cloned().collect();
    for id in ids {
        let before = old(&c.agents[&id]);
        let a = c.agents.get_mut(&id).unwrap();
        if ["idle", "working", "blocked"].contains(&a.state.as_str()) {
            a.last_known = Some(a.state.clone());
        }
        a.state = "unknown".into();
        a.blocked_reason = None;
        a.rule = None;
        a.input = None;
        mark(a, "error");
        unmark(a, "unmatched");
        unmark(a, "background");
        a.idle_n = 0;
        a.idle_first = None;
        a.idle_key = None;
        unmark(a, "idle_unconfirmed");
        c.changed(&id, before);
    }
}

fn fresh_rec(kind: &str, origin: &str, dynamic: bool, identity: Option<(String, u32)>) -> Rec {
    Rec {
        kind: kind.into(),
        origin: origin.into(),
        dynamic,
        identity,
        gone_since: None,
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
        blocked_reason: None,
        rule: None,
        input: None,
        since: None,
        absent: None,
        idle_key: None,
        idle_first: None,
        idle_n: 0,
    }
}

fn selector_matches(selector: &Selector, p: &tmux::Pane) -> bool {
    match selector {
        Selector::Pane(id) => &p.id == id,
        Selector::Tag(tag) => &p.tag == tag,
        Selector::Cwd(cwd) => &p.cwd == cwd,
    }
}

fn pane_number(p: &tmux::Pane) -> u64 {
    p.id.strip_prefix('%')
        .and_then(|n| n.parse().ok())
        .unwrap_or(u64::MAX)
}

fn valid_mark(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= 32
        && s.bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_' || b == b'-')
}

fn generated_id(kind: &str, cwd: &str) -> String {
    let last = cwd.rsplit('/').find(|s| !s.is_empty()).unwrap_or("");
    let mut base = String::new();
    let mut dash = false;
    for ch in last.chars().flat_map(char::to_lowercase) {
        let ch = if ch.is_ascii_lowercase() || ch.is_ascii_digit() || ch == '_' || ch == '-' {
            ch
        } else {
            '-'
        };
        if ch == '-' {
            if !dash {
                base.push(ch);
            }
            dash = true;
        } else {
            base.push(ch);
            dash = false;
        }
    }
    let base = base.trim_matches(['-', '_', '.']);
    let prefix = if base.is_empty() {
        kind.to_string()
    } else {
        format!("{kind}-")
    };
    let room = 32usize.saturating_sub(prefix.len());
    format!("{}{}", prefix, &base[..base.len().min(room)])
}

fn unique_id(base: &str, kind: &str, discovered: bool, used: &HashSet<String>) -> String {
    if !used.contains(base) {
        return base.into();
    }
    for n in 2u64.. {
        let suffix = format!("-{n}");
        let prefix = if discovered && base == kind {
            kind.to_string()
        } else if discovered {
            format!("{kind}-")
        } else {
            base.to_string()
        };
        let body = if discovered {
            base.strip_prefix(&prefix).unwrap_or("")
        } else {
            ""
        };
        let room = 32usize.saturating_sub(prefix.len() + suffix.len());
        let candidate = format!("{}{}{}", prefix, &body[..body.len().min(room)], suffix);
        if !used.contains(&candidate) {
            return candidate;
        }
    }
    unreachable!()
}

/// Resolve configured and dynamic identities before any pane is captured.
fn plan_agents(
    c: &mut Core,
    panes: &[tmux::Pane],
) -> (
    HashMap<String, tmux::AgentRead>,
    HashMap<String, tmux::Pane>,
    HashSet<String>,
) {
    let mut reads = HashMap::new();
    let mut targets = HashMap::new();
    let mut active = HashSet::new();
    let mut linked = HashSet::new();
    let mut configured_panes = HashSet::new();
    for (id, ac) in &c.cfg.agents {
        let matches: Vec<_> = panes
            .iter()
            .filter(|p| selector_matches(&ac.selector, p))
            .collect();
        for p in &matches {
            linked.insert(p.id.clone());
            configured_panes.insert(p.id.clone());
        }
        if matches.len() > 1 {
            reads.insert(
                id.clone(),
                tmux::AgentRead {
                    pane: None,
                    screen: None,
                    error: Some("pane_ambiguous".into()),
                },
            );
            continue;
        }
        if let Some(p) = matches.first().filter(|p| {
            !p.dead
                && c.rules
                    .kinds
                    .get(&ac.kind)
                    .is_some_and(|k| k.commands.contains(&p.command))
        }) {
            targets.insert(id.clone(), (*p).clone());
            active.insert(id.clone());
        }
    }

    let mut candidates: Vec<(u8, tmux::Pane, String, String)> = Vec::new();
    if c.cfg.discover {
        for p in panes.iter().filter(|p| !p.dead && !linked.contains(&p.id)) {
            let mark = if valid_mark(&p.tag) && !c.cfg.agents.contains_key(&p.tag) {
                Some(p.tag.as_str())
            } else {
                None
            };
            if mark == Some("off") {
                continue;
            }
            let kinds: Vec<_> = c
                .rules
                .kinds
                .iter()
                .filter(|(_, r)| r.commands.contains(&p.command))
                .map(|(k, _)| k.as_str())
                .collect();
            if kinds.len() != 1 {
                continue;
            }
            let kind = kinds[0].to_string();
            let identity = (p.id.clone(), p.pid);
            let old = c
                .agents
                .iter()
                .find(|(_, a)| a.dynamic && a.identity.as_ref() == Some(&identity));
            let origin = if let Some((_, a)) = old {
                a.origin.clone()
            } else if mark.is_some() {
                "mark".into()
            } else {
                "discovered".into()
            };
            let priority = match origin.as_str() {
                "mark" => 1,
                _ => 2,
            };
            candidates.push((priority, p.clone(), kind, origin));
        }
    }
    candidates.sort_by(|a, b| {
        a.0.cmp(&b.0)
            .then_with(|| pane_number(&a.1).cmp(&pane_number(&b.1)))
    });
    let mut ignored = 0usize;
    let mut found = 0usize;
    for (_, p, kind, origin) in candidates {
        let identity = (p.id.clone(), p.pid);
        let known = c
            .agents
            .iter()
            .find(|(_, a)| a.dynamic && a.identity.as_ref() == Some(&identity))
            .map(|(id, _)| id.clone());
        let id = if let Some(id) = known {
            id
        } else {
            if c.agents.len() >= 32 {
                ignored += 1;
                continue;
            }
            let desired = if origin == "mark" {
                p.tag.clone()
            } else {
                generated_id(&kind, &p.cwd)
            };
            let used: HashSet<String> = c.agents.keys().cloned().collect();
            let id = unique_id(&desired, &kind, origin == "discovered", &used);
            c.agents.insert(
                id.clone(),
                fresh_rec(&kind, &origin, true, Some(identity.clone())),
            );
            id
        };
        if c.agents.len() > 32 && !active.contains(&id) {
            ignored += 1;
            continue;
        }
        targets.insert(id.clone(), p.clone());
        active.insert(id.clone());
        configured_panes.insert(p.id.clone());
        if c.agents[&id].origin == "discovered" {
            found += 1;
        }
    }
    for id in c.agents.keys() {
        reads.entry(id.clone()).or_insert(tmux::AgentRead {
            pane: None,
            screen: None,
            error: None,
        });
    }
    c.discover_found = found;
    c.discover_ignored = ignored;
    (reads, targets, configured_panes)
}

fn poller(core: Arc<Mutex<Core>>, ready: mpsc::SyncSender<()>) {
    let (events_tx, events_rx) = mpsc::channel::<(String, Option<String>)>();
    let mut clients = BTreeMap::<String, EventClient>::new();
    let mut retries = BTreeMap::<String, Retry>::new();
    let mut configured_panes = HashSet::<String>::new();
    let mut dirty_panes = HashSet::<String>::new();
    let mut screen_cache = HashMap::<(String, u32), Vec<crate::rules::TextLine>>::new();
    let mut dirty_at: Option<Instant> = None;
    let mut next_safety = Instant::now();
    let mut last_cycle = Instant::now() - Duration::from_secs(3600);
    let mut first_cycle = true;
    let mut ready_cycles = 0u8;

    loop {
        if STOP.load(Ordering::Relaxed) {
            for (_, mut client) in clients {
                tmux::kill_group(&mut client.child);
            }
            return;
        }
        let cfg = {
            let c = core.lock().unwrap();
            c.cfg.clone()
        };
        let now = Instant::now();

        while let Ok((session, line)) = events_rx.try_recv() {
            match line {
                None => {
                    core.lock().unwrap().event_mode = "degraded".into();
                    if let Some(mut cl) = clients.remove(&session) {
                        tmux::kill_group(&mut cl.child);
                    }
                    schedule_retry(&mut retries, &session);
                }
                Some(line) if line == "__long__" || line == "%exit" => {
                    core.lock().unwrap().event_mode = "degraded".into();
                    if let Some(mut cl) = clients.remove(&session) {
                        tmux::kill_group(&mut cl.child);
                    }
                    schedule_retry(&mut retries, &session);
                }
                Some(line) => {
                    let mut parts = line.splitn(3, ' ');
                    if parts.next() == Some("%output")
                        && let Some(pane) = parts.next().filter(|p| configured_panes.contains(*p))
                    {
                        note_output(
                            &core,
                            pane,
                            &mut dirty_panes,
                            &mut dirty_at,
                            cfg.event_debounce_ms,
                        );
                    }
                }
            }
        }
        for (session, client) in &mut clients {
            if client.child.try_wait().ok().flatten().is_some() {
                let session = session.clone();
                if let Some(mut dead) = clients.remove(&session) {
                    tmux::kill_group(&mut dead.child);
                }
                schedule_retry(&mut retries, &session);
                break;
            }
        }

        let events_enabled = cfg.events;
        let any_clients = !clients.is_empty();
        let mode = core.lock().unwrap().event_mode.clone();
        let poll_due = (!events_enabled || mode != "on")
            && now.duration_since(last_cycle) >= Duration::from_millis(cfg.poll_ms);
        let safety_due = events_enabled && now >= next_safety;
        let debounce_due = dirty_at.is_some_and(|t| now >= t);
        let idle_due = {
            let c = core.lock().unwrap();
            c.agents.values().any(|a| {
                a.marks.contains(&"idle_unconfirmed".into())
                    && a.idle_first.is_some_and(|t| {
                        now.duration_since(t) >= Duration::from_millis(cfg.idle_confirm_ms)
                    })
            })
        };
        if poll_due || safety_due || debounce_due || idle_due {
            dirty_at = None;
            last_cycle = now;
            if safety_due {
                next_safety = now + Duration::from_millis(cfg.safety_poll_ms);
            }
            let capture_all = !events_enabled || mode != "on" || safety_due;
            let mut capture_ids = dirty_panes.clone();
            if !capture_all {
                let c = core.lock().unwrap();
                capture_ids.extend(c.agents.values().filter_map(|a| {
                    if a.marks.contains(&"idle_unconfirmed".into()) {
                        a.terminal.as_ref().map(|p| p.0.clone())
                    } else {
                        None
                    }
                }));
            }
            match tmux::list(&cfg) {
                Err(code) => {
                    let mut c = core.lock().unwrap();
                    source_failure(&mut c, &code);
                }
                Ok((started, started_wall, panes)) => {
                    let (mut reads, targets, current) = {
                        let mut c = core.lock().unwrap();
                        let (reads, targets, current) = plan_agents(&mut c, &panes);
                        (reads, targets, current)
                    };
                    let captured = tmux::capture(
                        &cfg,
                        &targets,
                        (!capture_all).then_some(&capture_ids),
                        &screen_cache,
                        started,
                        started_wall,
                    );
                    let snapshot = match captured {
                        Ok(s) => {
                            reads.extend(s.agents);
                            tmux::Snapshot { agents: reads, ..s }
                        }
                        Err(code) => {
                            let mut c = core.lock().unwrap();
                            source_failure(&mut c, &code);
                            continue;
                        }
                    };
                    let active_pairs: HashSet<(String, u32)> = snapshot
                        .agents
                        .values()
                        .filter_map(|r| r.pane.as_ref().map(|p| (p.id.clone(), p.pid)))
                        .collect();
                    screen_cache.retain(|key, _| active_pairs.contains(key));
                    for read in snapshot.agents.values() {
                        if let (Some(pane), Some(screen)) = (&read.pane, &read.screen) {
                            screen_cache.insert((pane.id.clone(), pane.pid), screen.clone());
                        } else if let Some(pane) = &read.pane {
                            screen_cache.remove(&(pane.id.clone(), pane.pid));
                        }
                    }
                    dirty_panes.clear();
                    configured_panes = current;
                    {
                        let mut c = core.lock().unwrap();
                        c.source_ok = true;
                        c.source_last = Some(snapshot.started_wall);
                        c.source_error = None;
                        let now = Instant::now();
                        let all_ids = c.agents.keys().cloned().collect::<Vec<_>>();
                        for id in all_ids {
                            let kind = c.agents[&id].kind.clone();
                            let read =
                                snapshot
                                    .agents
                                    .get(&id)
                                    .cloned()
                                    .unwrap_or(tmux::AgentRead {
                                        pane: None,
                                        screen: None,
                                        error: Some("source_bad_output".into()),
                                    });
                            let before = old(&c.agents[&id]);
                            let poll_ms = c.cfg.idle_confirm_polls;
                            let confirm_ms = c.cfg.idle_confirm_ms;
                            let gone_after_s = c.cfg.gone_after_s;
                            let event_live = c.cfg.events
                                && !clients.is_empty()
                                && read
                                    .pane
                                    .as_ref()
                                    .is_some_and(|p| clients.contains_key(&p.session));
                            let mut per_agent_error = read.error.clone();
                            if let Some(code) = per_agent_error.as_deref() {
                                apply_error(c.agents.get_mut(&id).unwrap(), code);
                            } else if let Some(pane) = read.pane {
                                if let Some(screen) = read.screen {
                                    let decision = c.rules.evaluate(&kind, &screen, &pane.title);
                                    apply_valid(
                                        &mut c,
                                        &id,
                                        pane,
                                        decision,
                                        snapshot.started,
                                        snapshot.started_wall,
                                        poll_ms,
                                        confirm_ms,
                                        event_live,
                                        now,
                                    );
                                } else {
                                    per_agent_error = Some("capture_failed".into());
                                    apply_error(c.agents.get_mut(&id).unwrap(), "capture_failed");
                                }
                            } else {
                                apply_absent(c.agents.get_mut(&id).unwrap(), now, gone_after_s);
                            }
                            if let Some(code) = per_agent_error {
                                c.source_error = Some(code);
                            }
                            c.changed(&id, before);
                        }
                        let forget = c.cfg.discover_forget_s;
                        let forgotten: Vec<String> = c
                            .agents
                            .iter()
                            .filter_map(|(id, a)| {
                                (a.dynamic
                                    && a.state == "gone"
                                    && a.gone_since
                                        .is_some_and(|t| now.duration_since(t).as_secs() >= forget))
                                .then_some(id.clone())
                            })
                            .collect();
                        for id in forgotten {
                            c.agents.remove(&id);
                        }
                    }
                    if events_enabled {
                        let mut sessions: Vec<String> = panes
                            .iter()
                            .filter(|p| configured_panes.contains(&p.id) && !p.dead)
                            .map(|p| p.session.clone())
                            .collect();
                        sessions.sort_by_key(|s| {
                            s.trim_start_matches('$').parse::<u64>().unwrap_or(u64::MAX)
                        });
                        sessions.dedup();
                        let sessions: Vec<String> = sessions
                            .into_iter()
                            .filter(|s| !clients.contains_key(s))
                            .take(8usize.saturating_sub(clients.len()))
                            .collect();
                        for session in sessions {
                            if retries.get(&session).is_some_and(|r| now < r.after) {
                                continue;
                            }
                            match tmux::spawn_event(&cfg, &session) {
                                Ok((child, input, output, stderr)) => {
                                    tmux::event_reader(session.clone(), output, events_tx.clone());
                                    tmux::discard_stderr(stderr);
                                    clients.insert(
                                        session.clone(),
                                        EventClient {
                                            child,
                                            _stdin: input,
                                        },
                                    );
                                }
                                Err(_) => schedule_retry(&mut retries, &session),
                            }
                        }
                        let wanted: HashSet<String> = panes
                            .iter()
                            .filter(|p| configured_panes.contains(&p.id) && !p.dead)
                            .map(|p| p.session.clone())
                            .collect();
                        let mode = if wanted.is_empty() {
                            "off"
                        } else if wanted.iter().all(|s| clients.contains_key(s)) {
                            "on"
                        } else {
                            "degraded"
                        };
                        core.lock().unwrap().event_mode = mode.into();
                    } else {
                        core.lock().unwrap().event_mode = "off".into();
                    }
                }
            }
            if first_cycle {
                ready_cycles = ready_cycles.saturating_add(1);
                if ready_cycles >= 2 {
                    let _ = ready.send(());
                    first_cycle = false;
                }
            }
            continue;
        }
        let wait = if events_enabled && any_clients {
            let d = dirty_at
                .map(|t| t.saturating_duration_since(now))
                .unwrap_or(
                    Duration::from_millis(cfg.safety_poll_ms)
                        .min(next_safety.saturating_duration_since(now)),
                );
            d.min(Duration::from_millis(50))
                .max(Duration::from_millis(1))
        } else {
            Duration::from_millis(20).min(Duration::from_millis(cfg.poll_ms))
        };
        match events_rx.recv_timeout(wait) {
            Ok((session, line)) => match line {
                None => {
                    core.lock().unwrap().event_mode = "degraded".into();
                    if let Some(mut cl) = clients.remove(&session) {
                        tmux::kill_group(&mut cl.child);
                    }
                    schedule_retry(&mut retries, &session);
                }
                Some(ref s) if s == "__long__" || s == "%exit" => {
                    core.lock().unwrap().event_mode = "degraded".into();
                    if let Some(mut cl) = clients.remove(&session) {
                        tmux::kill_group(&mut cl.child);
                    }
                    schedule_retry(&mut retries, &session);
                }
                Some(line) => {
                    let mut parts = line.splitn(3, ' ');
                    if parts.next() == Some("%output")
                        && let Some(pane) = parts.next().filter(|p| configured_panes.contains(*p))
                    {
                        note_output(
                            &core,
                            pane,
                            &mut dirty_panes,
                            &mut dirty_at,
                            cfg.event_debounce_ms,
                        );
                    }
                }
            },
            Err(mpsc::RecvTimeoutError::Timeout) => {}
            Err(mpsc::RecvTimeoutError::Disconnected) => thread::sleep(Duration::from_millis(50)),
        }
    }
}
fn schedule_retry(retries: &mut BTreeMap<String, Retry>, session: &str) {
    let entry = retries.entry(session.into()).or_insert(Retry {
        attempts: 0,
        after: Instant::now(),
    });
    let secs = 1u64
        .checked_shl(entry.attempts.min(5))
        .unwrap_or(30)
        .min(30);
    entry.attempts = entry.attempts.saturating_add(1);
    entry.after = Instant::now() + Duration::from_secs(secs);
}
fn note_output(
    core: &Arc<Mutex<Core>>,
    pane: &str,
    dirty_panes: &mut HashSet<String>,
    dirty_at: &mut Option<Instant>,
    debounce_ms: u64,
) {
    let now = Instant::now();
    dirty_panes.insert(pane.to_string());
    let mut c = core.lock().unwrap();
    c.last_output.insert(pane.to_string(), now);
    for a in c.agents.values_mut() {
        if a.terminal.as_ref().is_some_and(|p| p.0 == pane)
            && (a.state == "idle" || a.marks.contains(&"idle_unconfirmed".into()))
        {
            a.idle_key = None;
            a.idle_first = None;
            a.idle_n = 0;
            if a.state == "idle" {
                mark(a, "idle_unconfirmed");
            }
        }
    }
    if dirty_at.is_none() {
        *dirty_at = Some(now + Duration::from_millis(debounce_ms));
    }
}
fn apply_error(a: &mut Rec, _code: &str) {
    if ["idle", "working", "blocked"].contains(&a.state.as_str()) {
        a.last_known = Some(a.state.clone());
    }
    a.state = "unknown".into();
    a.blocked_reason = None;
    a.rule = None;
    a.input = None;
    mark(a, "error");
    a.idle_n = 0;
    a.idle_key = None;
    a.idle_first = None;
    unmark(a, "idle_unconfirmed");
}
fn apply_absent(a: &mut Rec, now: Instant, gone_after_s: u64) {
    if ["idle", "working", "blocked"].contains(&a.state.as_str()) {
        a.last_known = Some(a.state.clone());
    }
    a.idle_n = 0;
    a.idle_key = None;
    a.idle_first = None;
    unmark(a, "idle_unconfirmed");
    mark(a, "error");
    if a.absent.is_none() {
        a.absent = Some(now);
    }
    a.state = if now.duration_since(a.absent.unwrap()).as_secs() >= gone_after_s {
        "gone"
    } else {
        "unknown"
    }
    .into();
    if a.state == "gone" && a.gone_since.is_none() {
        a.gone_since = Some(now);
    }
    a.blocked_reason = None;
    a.rule = None;
    a.input = None;
}
#[allow(clippy::too_many_arguments)]
fn apply_valid(
    c: &mut Core,
    id: &str,
    pane: tmux::Pane,
    d: crate::rules::Decision,
    observed: Instant,
    observed_wall: u64,
    polls: u64,
    confirm_ms: u64,
    event_live: bool,
    now: Instant,
) {
    let a = c.agents.get_mut(id).unwrap();
    let pair = (pane.id.clone(), pane.pid);
    let reused = a.terminal.as_ref().is_some_and(|p| p != &pair);
    unmark(a, "error");
    unmark(a, "stale");
    a.last_seen = Some(crate::time::wall_ms());
    a.observed = Some(observed);
    a.observed_at = Some(observed_wall);
    a.evidence = "observed".into();
    a.source = Some("tmux".into());
    a.cwd = None;
    a.absent = None;
    a.gone_since = None;
    if reused {
        if ["idle", "working", "blocked"].contains(&a.state.as_str()) {
            a.last_known = Some(a.state.clone());
        }
        a.terminal = Some(pair);
        a.state = "unknown".into();
        mark(a, "error");
        a.blocked_reason = None;
        a.rule = None;
        a.input = None;
        a.idle_n = 0;
        a.idle_key = None;
        a.idle_first = None;
        unmark(a, "unmatched");
        unmark(a, "background");
        unmark(a, "idle_unconfirmed");
        return;
    }
    a.terminal = Some(pair.clone());
    let marks = d.marks.clone();
    if d.state == "idle" {
        let key = (
            pane.id.clone(),
            pane.pid,
            if event_live { 0 } else { pane.window_activity },
            d.rule.clone().unwrap_or_default(),
        );
        if a.idle_key.as_ref() != Some(&key) {
            a.idle_key = Some(key.clone());
            a.idle_first = Some(now);
            a.idle_n = 0;
        }
        a.idle_n = a.idle_n.saturating_add(1);
        let quiet = event_live
            && a.idle_first
                .is_some_and(|first| c.last_output.get(&pane.id).is_none_or(|t| *t <= first));
        let confirm = a.idle_n >= polls
            && a.idle_first
                .is_some_and(|first| now.duration_since(first).as_millis() as u64 >= confirm_ms)
            && (if event_live {
                quiet
            } else {
                a.idle_key
                    .as_ref()
                    .is_some_and(|k| k.2 == pane.window_activity)
            });
        a.marks = marks;
        if confirm {
            a.state = "idle".into();
            a.blocked_reason = None;
            a.rule = d.rule;
            a.input = d.input;
            a.last_known = None;
            unmark(a, "idle_unconfirmed");
        } else {
            if a.state == "unknown" || a.state == "gone" {
                a.state = "unknown".into();
            }
            mark(a, "idle_unconfirmed");
            a.rule = d.rule;
            a.input = if a.state == "idle" { d.input } else { None };
        }
    } else {
        a.state = d.state;
        a.blocked_reason = d.reason;
        a.rule = d.rule;
        a.input = None;
        a.marks = marks;
        a.last_known = None;
        a.idle_n = 0;
        a.idle_key = None;
        a.idle_first = None;
        unmark(a, "idle_unconfirmed");
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
                .is_some_and(|x| !matches!(x.as_str(), Some("claude" | "codex" | "pi")))
            {
                Err(err("invalid_envelope"))
            } else {
                c.snapshot();
                let a: Vec<_> = c
                    .agents
                    .keys()
                    .filter(|id| {
                        p.get("kind")
                            .is_none_or(|k| k.as_str() == Some(c.agents[*id].kind.as_str()))
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
                let agents:serde_json::Map<String,Value>=c.agents.iter().map(|(id,a)|(id.clone(),json!({"state":a.state,"origin":a.origin,"marks":a.marks,"source":a.source,"last_seen":a.last_seen,"since":a.since,"blocked_reason":a.blocked_reason}))).collect();
                Ok(
                    json!({"v":1,"epoch":c.epoch,"head_seq":c.seq,"uptime_s":c.start.elapsed().as_secs(),"agents":agents,"sources":{"tmux":{"ok":c.source_ok,"last_ok_at":c.source_last,"last_error":c.source_error,"events":c.event_mode,"discover":{"on":c.cfg.discover,"found":c.discover_found,"ignored":c.discover_ignored}}}}),
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
    if cmd == "rules" {
        return rules_check_cli(args, json_mode);
    }
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
    let healthy = cmd != "health" || data["sources"]["tmux"]["ok"] == true;
    if cmd == "get" && !json_mode {
        data = data["agent"].clone()
    }
    if cmd == "health" {
        if !healthy {
            return output(
                json_mode,
                false,
                "source_error",
                "fonte tmux falhou",
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
            "{} {} {} {} idade={}s origin={}",
            esc(&a["id"]),
            state,
            esc(&a["evidence"]),
            esc(&a["source"]),
            age,
            esc(&a["origin"])
        )
    }
    Ok(())
}

fn rules_check_cli(args: &[String], json_mode: bool) -> Result<(), Failure> {
    const USAGE: &str = "uso: agentd rules check --rules ARQ --fixtures DIR [--json]";
    if args.get(1).map(String::as_str) != Some("check") {
        return Err((2, USAGE.into()));
    }
    let (Some(rules_path), Some(fixtures_path)) = (opt(args, "--rules"), opt(args, "--fixtures"))
    else {
        return Err((2, USAGE.into()));
    };
    let rules = Rules::load(Path::new(&rules_path)).map_err(|_| (1, "rules invalidas".into()))?;
    match rules.check(Path::new(&fixtures_path)) {
        Ok(()) => {
            if json_mode {
                println!(
                    "{}",
                    json!({"schema_version":1,"ok":true,"code":"ok","message":"ok","data":{"checked":true}})
                );
            } else {
                println!("rules: ok");
            }
            Ok(())
        }
        Err(failures) => {
            let message = failures.join("; ");
            if json_mode {
                println!(
                    "{}",
                    json!({"schema_version":1,"ok":false,"code":"rules_check_failed","message":message,"data":null})
                );
            }
            Err((1, message))
        }
    }
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

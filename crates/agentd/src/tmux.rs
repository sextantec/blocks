use crate::{
    config::{Config, valid_pane},
    rules::{TextLine, parse_screen},
};
use std::{
    collections::{HashMap, HashSet},
    io::Read,
    os::unix::process::CommandExt,
    process::{Command, Stdio},
    sync::mpsc,
    thread,
    time::{Duration, Instant},
};

const LIST_LIMIT: usize = 1024 * 1024;
const CAPTURE_LIMIT: usize = 256 * 1024;
const FORMAT: &str = "#{pane_id}\t#{pane_pid}\t#{pane_dead}\t#{pane_dead_status}\t#{window_activity}\t#{pane_current_command}\t#{session_id}\t#{@agentd}\t#{s|[^a-zA-Z0-9._/-]|_|:pane_current_path}\t#{pane_title}";

#[derive(Clone, Debug)]
pub struct Pane {
    pub id: String,
    pub pid: u32,
    pub dead: bool,
    pub window_activity: u64,
    pub command: String,
    pub session: String,
    pub tag: String,
    pub cwd: String,
    pub title: String,
}
#[derive(Clone, Debug)]
pub struct AgentRead {
    pub pane: Option<Pane>,
    pub screen: Option<Vec<TextLine>>,
    pub error: Option<String>,
}
pub struct Snapshot {
    pub started: Instant,
    pub started_wall: u64,
    pub agents: std::collections::HashMap<String, AgentRead>,
}

pub fn list(cfg: &Config) -> Result<(Instant, u64, Vec<Pane>), String> {
    let started = Instant::now();
    let started_wall = crate::time::wall_ms();
    let mut args = prefix(cfg);
    args.extend(["list-panes".into(), "-a".into(), "-F".into(), FORMAT.into()]);
    let output = run(cfg, &args, LIST_LIMIT)?;
    if !output.success {
        return Err("source_exit_nonzero".into());
    }
    Ok((started, started_wall, parse_list(&output.stdout)?))
}

pub fn capture(
    cfg: &Config,
    targets: &HashMap<String, Pane>,
    capture_ids: Option<&HashSet<String>>,
    cache: &HashMap<(String, u32), Vec<TextLine>>,
    started: Instant,
    started_wall: u64,
) -> Result<Snapshot, String> {
    let mut agents = std::collections::HashMap::new();
    for (id, pane) in targets {
        let cache_key = (pane.id.clone(), pane.pid);
        if capture_ids.is_some_and(|ids| !ids.contains(&pane.id))
            && let Some(screen) = cache.get(&cache_key)
        {
            agents.insert(
                id.clone(),
                AgentRead {
                    pane: Some(pane.clone()),
                    screen: Some(screen.clone()),
                    error: None,
                },
            );
            continue;
        }
        let mut capargs = prefix(cfg);
        capargs.extend([
            "capture-pane".into(),
            "-p".into(),
            "-e".into(),
            "-J".into(),
            "-t".into(),
            pane.id.clone(),
        ]);
        match run(cfg, &capargs, CAPTURE_LIMIT) {
            Ok(c) if c.success => {
                agents.insert(
                    id.clone(),
                    AgentRead {
                        pane: Some(pane.clone()),
                        screen: Some(parse_screen(&c.stdout)),
                        error: None,
                    },
                );
            }
            Ok(_) => {
                agents.insert(
                    id.clone(),
                    AgentRead {
                        pane: Some(pane.clone()),
                        screen: None,
                        error: Some("capture_failed".into()),
                    },
                );
            }
            Err(code) => {
                agents.insert(
                    id.clone(),
                    AgentRead {
                        pane: Some(pane.clone()),
                        screen: None,
                        error: Some(code),
                    },
                );
            }
        }
    }
    Ok(Snapshot {
        started,
        started_wall,
        agents,
    })
}
fn prefix(cfg: &Config) -> Vec<String> {
    cfg.tmux_socket
        .as_ref()
        .map(|s| vec!["-S".into(), s.clone()])
        .unwrap_or_default()
}
pub fn parse_list(raw: &[u8]) -> Result<Vec<Pane>, String> {
    let text = std::str::from_utf8(raw).map_err(|_| "source_bad_output".to_string())?;
    let mut out = Vec::new();
    let mut seen = HashSet::new();
    for line in text.lines() {
        let cols: Vec<&str> = line.split('\t').collect();
        if cols.len() < 10 || cols[8].is_empty() {
            return Err("source_bad_output".into());
        }
        let id = cols[0];
        let pid = cols[1].parse::<u32>().map_err(|_| "source_bad_output")?;
        let dead = match cols[2] {
            "0" => false,
            "1" => true,
            _ => return Err("source_bad_output".into()),
        };
        let activity = cols[4].parse::<u64>().map_err(|_| "source_bad_output")?;
        let session = cols[6];
        if !valid_pane(id) || !valid_session(session) {
            return Err("source_bad_output".into());
        }
        if !seen.insert(id.to_string()) {
            return Err("duplicate_pane".into());
        }
        out.push(Pane {
            id: id.into(),
            pid,
            dead,
            window_activity: activity,
            command: cols[5].into(),
            session: session.into(),
            tag: cols[7].into(),
            cwd: cols[8].into(),
            title: cols[9..].join("\t"),
        });
    }
    Ok(out)
}
fn valid_session(s: &str) -> bool {
    static SESSION: std::sync::OnceLock<regex::Regex> = std::sync::OnceLock::new();
    SESSION
        .get_or_init(|| regex::Regex::new(r"^\$[0-9]+$").unwrap())
        .is_match(s)
}
struct Output {
    stdout: Vec<u8>,
    success: bool,
}
fn run(cfg: &Config, args: &[String], limit: usize) -> Result<Output, String> {
    let mut cmd = Command::new(&cfg.tmux_bin);
    cmd.args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    unsafe {
        cmd.pre_exec(|| {
            if libc::setpgid(0, 0) == -1 {
                Err(std::io::Error::last_os_error())
            } else {
                Ok(())
            }
        });
    }
    let mut child = cmd.spawn().map_err(|_| "source_spawn_failed".to_string())?;
    let pid = child.id() as i32;
    let mut stdout = child.stdout.take().unwrap();
    let mut stderr = child.stderr.take().unwrap();
    let (tx, rx) = mpsc::channel();
    let out_task = thread::spawn(move || {
        let mut bytes = Vec::new();
        let mut b = [0u8; 8192];
        loop {
            match stdout.read(&mut b) {
                Ok(0) | Err(_) => break,
                Ok(n) => {
                    let rem = limit.saturating_add(1).saturating_sub(bytes.len());
                    bytes.extend_from_slice(&b[..n.min(rem)]);
                }
            }
        }
        let _ = tx.send(bytes);
    });
    let err_task = thread::spawn(move || {
        let mut b = [0u8; 8192];
        let mut n = 0;
        while n < 65536 {
            match stderr.read(&mut b) {
                Ok(0) | Err(_) => break,
                Ok(x) => n += x,
            }
        }
    });
    let start = Instant::now();
    let mut status = None;
    let mut result = None;
    while status.is_none() || result.is_none() {
        if status.is_none() {
            status = child.try_wait().ok().flatten();
        }
        if result.is_none() {
            result = rx.try_recv().ok();
        }
        if start.elapsed() >= Duration::from_millis(cfg.source_timeout_ms) {
            unsafe {
                libc::kill(-pid, libc::SIGKILL);
            }
            let _ = child.kill();
            let _ = child.wait();
            let _ = out_task.join();
            let _ = err_task.join();
            return Err("source_timeout".into());
        }
        thread::sleep(Duration::from_millis(4));
    }
    let bytes = result.unwrap_or_default();
    let _ = out_task.join();
    let _ = err_task.join();
    if bytes.len() > limit {
        return Err("source_too_large".into());
    }
    Ok(Output {
        stdout: bytes,
        success: status.unwrap().success(),
    })
}

pub fn spawn_event(
    cfg: &Config,
    session: &str,
) -> Result<
    (
        std::process::Child,
        std::process::ChildStdin,
        std::process::ChildStdout,
        std::process::ChildStderr,
    ),
    String,
> {
    let mut cmd = Command::new(&cfg.tmux_bin);
    if let Some(socket) = &cfg.tmux_socket {
        cmd.arg("-S").arg(socket);
    }
    cmd.args(["-C", "attach", "-f", "ignore-size", "-t", session])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    unsafe {
        cmd.pre_exec(|| {
            if libc::setpgid(0, 0) == -1 {
                Err(std::io::Error::last_os_error())
            } else {
                Ok(())
            }
        });
    }
    let mut child = cmd.spawn().map_err(|_| "source_spawn_failed".to_string())?;
    let input = child.stdin.take().ok_or("source_spawn_failed")?;
    let output = child.stdout.take().ok_or("source_spawn_failed")?;
    let error = child.stderr.take().ok_or("source_spawn_failed")?;
    Ok((child, input, output, error))
}
pub fn event_reader<R: Read + Send + 'static>(
    session: String,
    output: R,
    tx: mpsc::Sender<(String, Option<String>)>,
) {
    thread::spawn(move || {
        let mut reader = std::io::BufReader::new(output);
        loop {
            let mut line = Vec::new();
            match std::io::BufRead::read_until(&mut reader, b'\n', &mut line) {
                Ok(0) => {
                    let _ = tx.send((session, None));
                    break;
                }
                Ok(n) if n > 1024 * 1024 => {
                    let _ = tx.send((session, Some("__long__".into())));
                    break;
                }
                Ok(_) => {
                    let mut fields = line
                        .split(|b| b.is_ascii_whitespace())
                        .filter(|f| !f.is_empty());
                    let first = fields.next().unwrap_or_default();
                    let message = if first == b"%output" {
                        fields
                            .next()
                            .filter(|p| valid_pane(std::str::from_utf8(p).unwrap_or("")))
                            .map(|pane| {
                                format!("%output {}", std::str::from_utf8(pane).unwrap_or(""))
                            })
                    } else if first == b"%exit" {
                        Some("%exit".into())
                    } else {
                        None
                    };
                    if let Some(message) = message
                        && tx.send((session.clone(), Some(message))).is_err()
                    {
                        break;
                    }
                }
                Err(_) => {
                    let _ = tx.send((session, None));
                    break;
                }
            }
        }
    });
}
pub fn discard_stderr<R: Read + Send + 'static>(mut input: R) {
    thread::spawn(move || {
        let mut b = [0u8; 8192];
        let mut n = 0;
        while n < 65536 {
            match input.read(&mut b) {
                Ok(0) | Err(_) => break,
                Ok(x) => n += x,
            }
        }
    });
}
pub fn kill_group(child: &mut std::process::Child) {
    unsafe {
        libc::kill(-(child.id() as i32), libc::SIGKILL);
    }
    let _ = child.kill();
    let _ = child.wait();
}

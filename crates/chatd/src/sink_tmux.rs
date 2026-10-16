//! Sink tmux do `stale`: comandos fixos, limites de tempo/saida e nomes validados.

use std::collections::HashSet;
use std::io::Read;
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

const OUTPUT_LIMIT: usize = 64 * 1024;
const CALL_LIMIT: Duration = Duration::from_secs(5);
const TOTAL_LIMIT: Duration = Duration::from_secs(10);
const CLIENT_LIMIT: usize = 8;

pub(crate) struct Entry {
    pub recipient: String,
    pub kind: &'static str,
    pub count: usize,
    pub minutes: i64,
}

#[derive(Clone, Copy)]
enum SinkError {
    SpawnFailed,
    Timeout,
    ExitNonzero,
    BadOutput,
}

struct Output {
    stdout: Vec<u8>,
    status_ok: bool,
}

/// Escreve o estado sempre, e entrega somente os avisos novos aos clientes conectados.
/// Retorna a quantidade de nomes de destinatario invalidos ignorados no status.
pub(crate) fn run(
    bin: &str,
    socket: Option<&str>,
    entries: &[Entry],
    new_lines: &[String],
) -> usize {
    let (rows, skipped) = status_rows(entries);
    let status = compact_status(&rows);
    let deadline = Instant::now() + TOTAL_LIMIT;
    let mut warned = HashSet::new();
    if set_status(bin, socket, &status, deadline, &mut warned) && !new_lines.is_empty() {
        notify_clients(bin, socket, new_lines, deadline, &mut warned);
    }
    skipped
}

fn status_rows(entries: &[Entry]) -> (Vec<(&str, &str, usize, i64)>, usize) {
    let mut skipped = 0;
    let mut rows: Vec<_> = entries
        .iter()
        .filter_map(|entry| {
            if crate::protocol::valid_name(&entry.recipient) {
                Some((
                    entry.recipient.as_str(),
                    entry.kind,
                    entry.count,
                    entry.minutes,
                ))
            } else {
                skipped += 1;
                None
            }
        })
        .collect();
    rows.sort_by(|a, b| a.0.cmp(b.0).then_with(|| a.1.cmp(b.1)));
    (rows, skipped)
}

fn set_status(
    bin: &str,
    socket: Option<&str>,
    status: &str,
    deadline: Instant,
    warned: &mut HashSet<&'static str>,
) -> bool {
    match call(
        bin,
        socket,
        &["set-option", "-g", "--", "@chatd_stale", status],
        deadline,
    ) {
        Ok(output) => {
            if !output.status_ok {
                warn(warned, SinkError::ExitNonzero);
            }
            true
        }
        Err(error) => {
            warn(warned, error);
            false
        }
    }
}

fn notify_clients(
    bin: &str,
    socket: Option<&str>,
    lines: &[String],
    deadline: Instant,
    warned: &mut HashSet<&'static str>,
) {
    let clients = match list_clients(bin, socket, deadline) {
        Ok(clients) => clients,
        Err(error) => {
            warn(warned, error);
            return;
        }
    };
    display_alerts(bin, socket, &clients, lines, deadline, warned);
}

fn list_clients(
    bin: &str,
    socket: Option<&str>,
    deadline: Instant,
) -> Result<Vec<String>, SinkError> {
    let output = call(
        bin,
        socket,
        &["list-clients", "-F", "#{client_name}"],
        deadline,
    )?;
    if !output.status_ok {
        return Err(SinkError::ExitNonzero);
    }
    let text = String::from_utf8(output.stdout).map_err(|_| SinkError::BadOutput)?;
    Ok(text
        .lines()
        .filter(|name| valid_client(name))
        .take(CLIENT_LIMIT)
        .map(str::to_owned)
        .collect())
}

fn display_alerts(
    bin: &str,
    socket: Option<&str>,
    clients: &[String],
    lines: &[String],
    deadline: Instant,
    warned: &mut HashSet<&'static str>,
) {
    for line in lines {
        for client in clients {
            let result = call(
                bin,
                socket,
                &[
                    "display-message",
                    "-c",
                    client,
                    "-d",
                    "10000",
                    "-l",
                    "--",
                    line,
                ],
                deadline,
            );
            match result {
                Ok(output) if !output.status_ok => warn(warned, SinkError::ExitNonzero),
                Ok(_) => {}
                Err(error) => {
                    warn(warned, error);
                    return;
                }
            }
        }
    }
}

fn warn(warned: &mut HashSet<&'static str>, error: SinkError) {
    let reason = match error {
        SinkError::SpawnFailed => "spawn_failed",
        SinkError::Timeout => "timeout",
        SinkError::ExitNonzero => "exit_nonzero",
        SinkError::BadOutput => "bad_output",
    };
    if warned.insert(reason) {
        eprintln!("chatd: aviso: sink tmux: {reason}");
    }
}

fn call(
    bin: &str,
    socket: Option<&str>,
    args: &[&str],
    total_deadline: Instant,
) -> Result<Output, SinkError> {
    let call_deadline = (Instant::now() + CALL_LIMIT).min(total_deadline);
    if Instant::now() >= call_deadline {
        return Err(SinkError::Timeout);
    }
    let mut command = Command::new(bin);
    if let Some(socket) = socket {
        command.arg("-S").arg(socket);
    }
    command
        .args(args)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = command.spawn().map_err(|_| SinkError::SpawnFailed)?;
    let stdout = child.stdout.take().ok_or(SinkError::BadOutput)?;
    let stderr = child.stderr.take().ok_or(SinkError::BadOutput)?;
    let bytes_seen = Arc::new(AtomicUsize::new(0));
    let too_large = Arc::new(AtomicBool::new(false));
    let out = Arc::new(Mutex::new(Vec::new()));
    let stdout_reader = drain(
        stdout,
        Arc::clone(&bytes_seen),
        Arc::clone(&too_large),
        Some(Arc::clone(&out)),
    );
    let stderr_reader = drain(
        stderr,
        Arc::clone(&bytes_seen),
        Arc::clone(&too_large),
        None,
    );

    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) if Instant::now() < call_deadline => {
                thread::sleep(Duration::from_millis(10));
            }
            Ok(None) => {
                let _ = child.kill();
                let _ = child.wait();
                let _ = stdout_reader.join();
                let _ = stderr_reader.join();
                return Err(SinkError::Timeout);
            }
            Err(_) => {
                let _ = child.kill();
                let _ = child.wait();
                let _ = stdout_reader.join();
                let _ = stderr_reader.join();
                return Err(SinkError::BadOutput);
            }
        }
    };
    if stdout_reader.join().is_err() || stderr_reader.join().is_err() {
        return Err(SinkError::BadOutput);
    }
    if Instant::now() > call_deadline {
        return Err(SinkError::Timeout);
    }
    if too_large.load(Ordering::Relaxed) {
        return Err(SinkError::BadOutput);
    }
    let stdout = out.lock().map_err(|_| SinkError::BadOutput)?.clone();
    Ok(Output {
        stdout,
        status_ok: status.success(),
    })
}

fn drain<R: Read + Send + 'static>(
    mut reader: R,
    seen: Arc<AtomicUsize>,
    too_large: Arc<AtomicBool>,
    capture: Option<Arc<Mutex<Vec<u8>>>>,
) -> thread::JoinHandle<()> {
    thread::spawn(move || {
        let mut chunk = [0u8; 4096];
        loop {
            match reader.read(&mut chunk) {
                Ok(0) => break,
                Err(_) => {
                    too_large.store(true, Ordering::Relaxed);
                    break;
                }
                Ok(count) => {
                    let before = seen.fetch_add(count, Ordering::Relaxed);
                    if before.saturating_add(count) > OUTPUT_LIMIT {
                        too_large.store(true, Ordering::Relaxed);
                    }
                    if let Some(capture) = &capture {
                        let available = OUTPUT_LIMIT.saturating_sub(before).min(count);
                        if available > 0
                            && let Ok(mut output) = capture.lock()
                        {
                            output.extend_from_slice(&chunk[..available]);
                        }
                    }
                }
            }
        }
    })
}

fn compact_status(rows: &[(&str, &str, usize, i64)]) -> String {
    let entries: Vec<String> = rows
        .iter()
        .map(|(name, kind, count, minutes)| {
            let code = if *kind == "unaccepted" { 'p' } else { 'r' };
            format!("{name} {count}{code} {}m", (*minutes).max(0))
        })
        .collect();
    let joined = entries.join(" | ");
    if joined.len() <= 200 {
        return joined;
    }
    for shown in (0..entries.len()).rev() {
        let omitted = entries.len() - shown;
        let mut value = entries[..shown].join(" | ");
        value.push_str(" +");
        value.push_str(&omitted.to_string());
        if value.len() <= 200 {
            return value;
        }
    }
    String::new()
}

fn valid_client(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 64
        && !name.split('/').any(|part| part == "..")
        && name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"/_.:@-".contains(&b))
}

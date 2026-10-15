//! Campainha (adaptador herdr, spec 8): so toca em `idle`/`done`, texto fixo, sem shell, limite de 5 s.

use std::io::Read;
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};

use crate::config::Delivery;
use crate::server::{Shared, lock};

const CMD_TIMEOUT: Duration = Duration::from_secs(5);
const CMD_OUT_MAX: u64 = 64 * 1024;
const TICK: Duration = Duration::from_millis(50);

pub fn run(shared: &Shared, shutdown: &AtomicBool) {
    let poll = Duration::from_millis(shared.cfg.bell_poll_ms);
    let rearm = Duration::from_secs(shared.cfg.bell_rearm_s);
    while !shutdown.load(Ordering::SeqCst) {
        let due: Vec<(String, String)> = {
            let st = lock(&shared.store);
            st.bell_due(rearm, Instant::now())
                .into_iter()
                .filter_map(|name| match shared.cfg.recipients.get(&name) {
                    Some(Delivery::Bell { pane }) => Some((name.clone(), pane.clone())),
                    _ => None,
                })
                .collect()
        };
        for (name, pane) in due {
            if shutdown.load(Ordering::SeqCst) {
                return;
            }
            ring(shared, &name, &pane);
        }
        let until = Instant::now() + poll;
        while Instant::now() < until && !shutdown.load(Ordering::SeqCst) {
            thread::sleep(TICK);
        }
    }
}

fn ring(shared: &Shared, name: &str, pane: &str) {
    let bin = shared.cfg.herdr_bin.as_str();
    match run_herdr(bin, &["agent", "get", pane]).and_then(|out| agent_status(&out)) {
        Err(reason) => return record_failed(shared, name, reason),
        Ok(s) if s != "idle" && s != "done" => return,
        Ok(_) => {}
    }
    let n = lock(&shared.store).pending_count(name);
    if n == 0 {
        return;
    }
    let text = prompt_text(n);
    match run_herdr(bin, &["agent", "prompt", pane, &text]) {
        Ok(_) => {
            if let Err(e) = lock(&shared.store).bell_sent(name, Instant::now()) {
                eprintln!("chatd: aviso: nao registrei bell_sent: {}", e.message);
            }
        }
        Err(reason) => record_failed(shared, name, reason),
    }
}

fn record_failed(shared: &Shared, name: &str, reason: &str) {
    if let Err(e) = lock(&shared.store).bell_failed(name, reason, Instant::now()) {
        eprintln!("chatd: aviso: nao registrei bell_failed: {}", e.message);
    }
}

/// Texto fixo e literal da v1.1 (contrato C.3): so a contagem. Nunca remetente, assunto ou corpo.
pub fn prompt_text(n: usize) -> String {
    format!("[chat] {n} pendente(s). Rode: chatd inbox --take")
}

fn agent_status(out: &str) -> Result<String, &'static str> {
    let v: serde_json::Value = serde_json::from_str(out).map_err(|_| "invalid_output")?;
    v.pointer("/result/agent/agent_status")
        .and_then(serde_json::Value::as_str)
        .map(str::to_string)
        .ok_or("invalid_output")
}

/// Executa `bin args...` sem shell, com limite de 5 s. Devolve a saida ou um motivo curto.
fn run_herdr(bin: &str, args: &[&str]) -> Result<String, &'static str> {
    let mut child = Command::new(bin)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|_| "spawn_failed")?;
    let stdout = child.stdout.take();
    let (tx, rx) = mpsc::channel::<Vec<u8>>();
    thread::spawn(move || {
        let mut buf = Vec::new();
        if let Some(out) = stdout {
            let _ = out.take(CMD_OUT_MAX).read_to_end(&mut buf);
        }
        let _ = tx.send(buf);
    });
    let deadline = Instant::now() + CMD_TIMEOUT;
    let status = loop {
        match child.try_wait() {
            Ok(Some(st)) => break st,
            Ok(None) => {
                if Instant::now() >= deadline {
                    let _ = child.kill();
                    let _ = child.wait();
                    return Err("timeout");
                }
                thread::sleep(Duration::from_millis(10));
            }
            Err(_) => {
                let _ = child.kill();
                return Err("exit_status");
            }
        }
    };
    let wait_out = deadline
        .saturating_duration_since(Instant::now())
        .max(Duration::from_millis(100));
    let buf = rx.recv_timeout(wait_out).map_err(|_| "timeout")?;
    if !status.success() {
        return Err("exit_status");
    }
    Ok(String::from_utf8_lossy(&buf).into_owned())
}

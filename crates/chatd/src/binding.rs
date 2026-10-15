//! Ligações efêmeras entre uma identidade autenticada e um painel verificado.

use std::collections::HashMap;
use std::fmt;
use std::fs;
use std::io::ErrorKind;
use std::thread;
use std::time::{Duration, Instant};

use serde_json::Value;

use crate::config::Config;

pub const TTL: Duration = Duration::from_secs(120);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mux {
    Herdr,
    Tmux,
}

impl fmt::Display for Mux {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Mux::Herdr => "herdr",
            Mux::Tmux => "tmux",
        })
    }
}

#[derive(Debug, Clone)]
pub struct Binding {
    pub mux: Mux,
    pub pane: String,
    pub peer_pid: i32,
    pub expires_at: Instant,
}

#[derive(Default)]
pub struct Registry {
    entries: HashMap<String, Binding>,
}

impl Registry {
    pub fn get(&self, name: &str) -> Option<&Binding> {
        self.entries.get(name)
    }

    pub fn replace(&mut self, name: String, binding: Binding) -> Option<Binding> {
        self.entries.insert(name, binding)
    }

    pub fn remove(&mut self, name: &str) -> Option<Binding> {
        self.entries.remove(name)
    }

    pub fn renew_for_watch(&mut self, name: &str, now: Instant) -> bool {
        let Some(binding) = self.entries.get_mut(name) else {
            return false;
        };
        if !is_process_alive(binding.peer_pid) {
            return false;
        }
        binding.expires_at = now + TTL;
        true
    }

    /// Retira ligacoes expiradas ou cujo processo chamador deixou de existir.
    pub fn reap(&mut self, now: Instant) -> Vec<(String, Binding, &'static str)> {
        let names: Vec<(String, &'static str)> = self
            .entries
            .iter()
            .filter_map(|(name, binding)| {
                if !is_process_alive(binding.peer_pid) {
                    Some((name.clone(), "process_gone"))
                } else if now >= binding.expires_at {
                    Some((name.clone(), "expired"))
                } else {
                    None
                }
            })
            .collect();
        names
            .into_iter()
            .filter_map(|(name, reason)| self.entries.remove(&name).map(|b| (name, b, reason)))
            .collect()
    }
}

pub fn verify(cfg: &Config, name: &str, mux: Mux, pane: &str, peer_pid: i32) -> bool {
    if peer_pid <= 0 || !is_process_alive(peer_pid) {
        return false;
    }
    let Some(peer_cwd) = process_cwd(peer_pid) else {
        return false;
    };
    match mux {
        Mux::Tmux => tmux_pane_pid(&cfg.tmux_bin, pane)
            .is_some_and(|pane_pid| is_ancestor(pane_pid, peer_pid)),
        Mux::Herdr => herdr_agent(&cfg.herdr_bin, pane).is_some_and(|agent| {
            let Some(agent_type) = agent.pointer("/agent").and_then(Value::as_str) else {
                return false;
            };
            let Some(cwd) = agent.pointer("/cwd").and_then(Value::as_str) else {
                return false;
            };
            let expected = cfg
                .recipients
                .get(name)
                .map(|recipient| recipient.agent.as_str());
            if Some(agent_type) != expected || same_path(cwd, &peer_cwd).is_none() {
                return false;
            }
            // Algumas versoes do herdr podem passar a publicar pane_pid/pid. Se vier,
            // exige ancestralidade. A sondagem atual (`herdr agent get w3:p1`) nao
            // publica pid; nesse caso cwd + tipo e a verificacao disponivel.
            match agent.get("pane_pid").or_else(|| agent.get("pid")) {
                None => true,
                Some(value) => value
                    .as_i64()
                    .or_else(|| value.as_str()?.parse::<i64>().ok())
                    .is_some_and(|pane_pid| pane_pid > 0 && is_ancestor(pane_pid as i32, peer_pid)),
            }
        }),
    }
}

fn tmux_pane_pid(bin: &str, pane: &str) -> Option<i32> {
    let output =
        crate::bell::run_herdr(bin, &["display-message", "-p", "-t", pane, "#{pane_pid}"]).ok()?;
    output.trim().parse::<i32>().ok().filter(|pid| *pid > 0)
}

fn herdr_agent(bin: &str, pane: &str) -> Option<Value> {
    let output = crate::bell::run_herdr(bin, &["agent", "get", pane]).ok()?;
    let value: Value = serde_json::from_str(&output).ok()?;
    value.pointer("/result/agent").cloned()
}

fn process_cwd(pid: i32) -> Option<std::path::PathBuf> {
    fs::canonicalize(format!("/proc/{pid}/cwd")).ok()
}

fn same_path(path: &str, actual: &std::path::Path) -> Option<()> {
    (fs::canonicalize(path).ok()?.as_path() == actual).then_some(())
}

fn is_ancestor(ancestor: i32, pid: i32) -> bool {
    let mut current = pid;
    for _ in 0..256 {
        if current == ancestor {
            return true;
        }
        current = parent_pid(current).unwrap_or(0);
        if current <= 1 {
            return current == ancestor;
        }
    }
    false
}

fn parent_pid(pid: i32) -> Option<i32> {
    let status = fs::read_to_string(format!("/proc/{pid}/status")).ok()?;
    status
        .lines()
        .find_map(|line| line.strip_prefix("PPid:")?.trim().parse::<i32>().ok())
}

pub fn is_process_alive(pid: i32) -> bool {
    if pid <= 0 {
        return false;
    }
    // SAFETY: signal 0 only probes existence/permission and sends no signal.
    let rc = unsafe { libc::kill(pid, 0) };
    if rc == 0 {
        true
    } else {
        io_error_is_permission_denied()
    }
}

pub fn reap_now(shared: &crate::server::Shared, now: Instant) {
    let stale = crate::server::lock(&shared.bindings).reap(now);
    for (name, binding, reason) in stale {
        let _ = crate::server::lock(&shared.store).unbound(
            &name,
            &binding.mux.to_string(),
            &binding.pane,
            reason,
        );
    }
}

pub fn maintain(shared: &crate::server::Shared, shutdown: &std::sync::atomic::AtomicBool) {
    while !shutdown.load(std::sync::atomic::Ordering::SeqCst) {
        reap_now(shared, Instant::now());
        thread::sleep(Duration::from_millis(100));
    }
}

fn io_error_is_permission_denied() -> bool {
    std::io::Error::last_os_error().kind() == ErrorKind::PermissionDenied
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ancestry_includes_process_and_parent() {
        let pid = std::process::id() as i32;
        assert!(is_ancestor(pid, pid));
        assert!(is_ancestor(1, pid));
        assert!(!is_ancestor(i32::MAX, pid));
    }

    #[test]
    fn current_process_is_alive() {
        assert!(is_process_alive(std::process::id() as i32));
        assert!(!is_process_alive(0));
    }
}

//! CLI do chatd (spec v1 4 e v1.1 D). Codigos de saida: 0 sucesso, 1 falha ou erro do servidor, 2 uso.

mod auth;
mod bell;
mod binding;
mod client;
mod config;
mod hash;
mod obligations;
mod protocol;
mod server;
mod stale;
mod store;

use std::collections::BTreeMap;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::AtomicBool;

const USAGE: &str = "uso: chatd serve --state-dir DIR --config FILE [--socket PATH]\n     \
chatd send [--from N] --to N --kind K --subject T (--body T | --body-file F|-) [--thread ID] [--id ID] [--ref t:v]... [--keep-open] [--json]\n     \
chatd inbox [--as N] [--after SEQ] [--all] [--take] [--ids] [--json]\n     \
chatd ack --id ID [--as N] --stage aceito|resultado [--result R] [--note T]\n     \
chatd watch [--as N] [--from-start] [--receipts]\n     \
chatd show --id ID [--as N] [--json]\n     \
chatd bind [--as N] [--mux herdr|tmux] [--pane ID] [--keepalive]\n     \
chatd status [--as N] [--json]\n     \
chatd whoami [--as N] [--profile N] [--json]\n     \
chatd log --state-dir D [--tail N]\n     \
chatd stale [--state-dir D] [--older-than SEC] [--bells N] [--result-older-than SEC] [--notify SINK] [--stamp-file F] [--json]\n     \
chatd token new|revoke NOME --state-dir D | token list --state-dir D\n     \
clientes: --socket, CHATD_SOCKET, --state-dir ou CHATD_STATE_DIR; identidade: --as/--from ou CHATD_AS; token: --token-file ou CHATD_TOKEN_FILE";

pub enum Failure {
    Usage(String),
    Run(String),
    BindForbidden(String),
    Unavailable(String),
    PermissionDenied(String),
    StaleAlerts,
    /// Erro de uso sem a ajuda longa (saida 2): so a linha `chatd: <msg>`.
    Bad(String),
}

pub fn usage(m: impl Into<String>) -> Failure {
    Failure::Usage(m.into())
}

/// Opcoes `--nome valor` / `--nome=valor` (valores repetiveis para `ref`) e flags `--nome`.
pub struct Opts {
    values: BTreeMap<String, Vec<String>>,
    flags: Vec<String>,
}

impl Opts {
    /// Ultimo valor dado para a opcao.
    pub fn get(&self, k: &str) -> Option<&str> {
        self.values
            .get(k)
            .and_then(|v| v.last())
            .map(String::as_str)
    }

    /// Todos os valores dados para a opcao, na ordem.
    pub fn all(&self, k: &str) -> Vec<String> {
        self.values.get(k).cloned().unwrap_or_default()
    }

    pub fn flag(&self, k: &str) -> bool {
        self.flags.iter().any(|f| f == k)
    }

    pub fn req(&self, k: &str) -> Result<&str, Failure> {
        self.get(k).ok_or_else(|| usage(format!("falta --{k}")))
    }
}

pub fn parse_opts(args: &[String], values: &[&str], bools: &[&str]) -> Result<Opts, Failure> {
    let mut o = Opts {
        values: BTreeMap::new(),
        flags: Vec::new(),
    };
    let mut i = 0;
    while i < args.len() {
        let raw = &args[i];
        let Some(body) = raw.strip_prefix("--") else {
            return Err(usage(format!("argumento inesperado '{raw}'")));
        };
        let (name, inline) = match body.split_once('=') {
            Some((n, v)) => (n, Some(v.to_string())),
            None => (body, None),
        };
        if values.contains(&name) {
            let val = match inline {
                Some(v) => v,
                None => {
                    i += 1;
                    args.get(i)
                        .cloned()
                        .ok_or_else(|| usage(format!("--{name} exige valor")))?
                }
            };
            let entry = o.values.entry(name.to_string()).or_default();
            if !entry.is_empty() && name != "ref" {
                return Err(usage(format!("--{name} repetido")));
            }
            entry.push(val);
        } else if bools.contains(&name) && inline.is_none() {
            if o.flag(name) {
                return Err(usage(format!("--{name} repetido")));
            }
            o.flags.push(name.to_string());
        } else {
            return Err(usage(format!("opcao desconhecida '--{name}'")));
        }
        i += 1;
    }
    Ok(o)
}

pub fn env_path(k: &str) -> Option<PathBuf> {
    std::env::var_os(k)
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
}

fn main() {
    // Umask 0077 desde o inicio: log, trava, auth.json e tokens nascem 0600, sem janela 0644.
    // SAFETY: umask so altera a mascara de criacao do processo.
    unsafe {
        libc::umask(0o077);
    }
    let args: Vec<String> = std::env::args().skip(1).collect();
    let code = match run(&args) {
        Ok(()) => 0,
        Err(Failure::Usage(m)) => {
            eprintln!("chatd: uso: {m}\n{USAGE}");
            2
        }
        Err(Failure::Bad(m)) => {
            eprintln!("chatd: {m}");
            2
        }
        Err(Failure::Run(m)) => {
            eprintln!("chatd: {m}");
            1
        }
        Err(Failure::BindForbidden(m)) => {
            eprintln!("chatd: {m}");
            3
        }
        Err(Failure::Unavailable(m)) => {
            eprintln!("chatd: {m}");
            4
        }
        Err(Failure::PermissionDenied(m)) => {
            eprintln!("chatd: {m}");
            5
        }
        Err(Failure::StaleAlerts) => 6,
    };
    std::process::exit(code);
}

fn run(args: &[String]) -> Result<(), Failure> {
    let Some((cmd, rest)) = args.split_first() else {
        return Err(usage("falta o comando"));
    };
    match cmd.as_str() {
        "serve" => serve(rest),
        "send" => client::send(rest),
        "inbox" => client::inbox(rest),
        "ack" => client::ack(rest),
        "watch" => client::watch(rest),
        "show" => client::show(rest),
        "bind" => client::bind(rest),
        "status" => client::status(rest),
        "whoami" => client::whoami(rest),
        "log" => client::log(rest),
        "stale" => stale::run(rest),
        "obligations" => obligations::run(rest),
        "token" => auth::cli(rest),
        "-h" | "--help" => {
            println!("{USAGE}");
            Ok(())
        }
        other => Err(usage(format!("comando desconhecido '{other}'"))),
    }
}

fn serve(args: &[String]) -> Result<(), Failure> {
    let o = parse_opts(args, &["state-dir", "config", "socket"], &[])?;
    let state_dir = o
        .get("state-dir")
        .map(PathBuf::from)
        .or_else(|| env_path("CHATD_STATE_DIR"))
        .ok_or_else(|| usage("serve exige --state-dir ou CHATD_STATE_DIR"))?;
    let config_path = o.req("config")?;
    let socket = o
        .get("socket")
        .map(PathBuf::from)
        .or_else(|| env_path("CHATD_SOCKET"))
        .unwrap_or_else(|| state_dir.join("chatd.sock"));
    let cfg = config::load(Path::new(config_path)).map_err(Failure::Run)?;
    fs::create_dir_all(&state_dir)
        .map_err(|e| Failure::Run(format!("nao criei {}: {e}", state_dir.display())))?;
    fs::set_permissions(&state_dir, fs::Permissions::from_mode(0o700))
        .map_err(|e| Failure::Run(format!("nao ajustei 0700 em {}: {e}", state_dir.display())))?;
    let shutdown = Arc::new(AtomicBool::new(false));
    for sig in [signal_hook::consts::SIGINT, signal_hook::consts::SIGTERM] {
        signal_hook::flag::register(sig, Arc::clone(&shutdown))
            .map_err(|e| Failure::Run(format!("nao registrei sinal: {e}")))?;
    }
    let recipients: BTreeMap<String, bool> = cfg
        .recipients
        .iter()
        .map(|(n, d)| (n.clone(), d.is_bell()))
        .collect();
    let st = store::Store::open(&state_dir, recipients).map_err(Failure::Run)?;
    let auth_path = state_dir.join(auth::AUTH_FILE);
    server::run(cfg, st, &socket, auth_path, shutdown).map_err(Failure::Run)
}

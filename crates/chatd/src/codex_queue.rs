//! Adaptador Codex `queue` da spec v1.11.

use std::collections::HashMap;
use std::fs::{self, OpenOptions};
use std::io::Read;
use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
use std::path::Path;
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use crate::bell::{prompt_text, run_herdr};
use crate::config::Config;

const VERSION_TTL: Duration = Duration::from_secs(600);

static VERSION_CACHE: OnceLock<Mutex<HashMap<String, (Instant, String)>>> = OnceLock::new();

pub(crate) fn send_prompt(
    config: &Config,
    thread_file: &str,
    pending: usize,
) -> Result<(), &'static str> {
    let thread = read_thread_id(thread_file)?;
    codex_version(&config.codex_bin, &config.codex_versions)?;
    let message = prompt_text(pending);
    let args = [
        "queue",
        "--thread",
        thread.as_str(),
        "--message",
        message.as_str(),
    ];
    match run_herdr(&config.codex_bin, &args) {
        Ok(out) if out.starts_with("Queued message ") => Ok(()),
        Ok(_) | Err("exit_status") => Err("queue_failed"),
        Err(reason) => Err(reason),
    }
}

fn read_thread_id(path: &str) -> Result<String, &'static str> {
    let path = Path::new(path);
    let metadata = match fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Err("thread_file_missing");
        }
        Err(_) => return Err("thread_file_invalid"),
    };
    if !metadata.file_type().is_file() {
        return Err("thread_file_invalid");
    }
    let mut options = OpenOptions::new();
    options.read(true).custom_flags(libc::O_NOFOLLOW);
    let file = options.open(path).map_err(|_| "thread_file_invalid")?;
    let metadata = file.metadata().map_err(|_| "thread_file_invalid")?;
    let daemon_uid = fs::metadata("/proc/self")
        .map_err(|_| "thread_file_invalid")?
        .uid();
    if !metadata.file_type().is_file()
        || metadata.uid() != daemon_uid
        || metadata.mode() & 0o022 != 0
        || metadata.len() > 64
    {
        return Err("thread_file_invalid");
    }
    let mut bytes = Vec::with_capacity(metadata.len() as usize);
    file.take(65)
        .read_to_end(&mut bytes)
        .map_err(|_| "thread_file_invalid")?;
    if bytes.len() > 64 {
        return Err("thread_file_invalid");
    }
    if bytes.last() == Some(&b'\n') {
        bytes.pop();
    }
    if !valid_uuid(&bytes) {
        return Err("thread_file_invalid");
    }
    String::from_utf8(bytes).map_err(|_| "thread_file_invalid")
}

fn valid_uuid(bytes: &[u8]) -> bool {
    if bytes.len() != 36 {
        return false;
    }
    bytes.iter().enumerate().all(|(index, byte)| match index {
        8 | 13 | 18 | 23 => *byte == b'-',
        _ => byte.is_ascii_digit() || (b'a'..=b'f').contains(byte),
    })
}

fn codex_version(bin: &str, allowed_versions: &[String]) -> Result<String, &'static str> {
    let cache = VERSION_CACHE.get_or_init(|| Mutex::new(HashMap::new()));
    if let Some((cached_at, version)) = cache
        .lock()
        .map_err(|_| "codex_version_unsupported")?
        .get(bin)
        && cached_at.elapsed() < VERSION_TTL
    {
        return if allowed_versions.iter().any(|allowed| allowed == version) {
            Ok(version.clone())
        } else {
            Err("codex_version_unsupported")
        };
    }
    let output = run_herdr(bin, &["--version"]).map_err(|reason| match reason {
        "spawn_failed" | "timeout" => reason,
        _ => "codex_version_unsupported",
    })?;
    let version = extract_version(&output).ok_or("codex_version_unsupported")?;
    if !allowed_versions.iter().any(|allowed| allowed == &version) {
        return Err("codex_version_unsupported");
    }
    if let Ok(mut cache) = cache.lock() {
        cache.insert(bin.to_string(), (Instant::now(), version.clone()));
    }
    Ok(version)
}

fn extract_version(output: &str) -> Option<String> {
    output.split_whitespace().find_map(|word| {
        let candidate = word.trim_matches(|c: char| !c.is_ascii_digit() && c != '.');
        let mut parts = candidate.split('.');
        let valid = (0..3).all(|_| {
            parts
                .next()
                .is_some_and(|part| !part.is_empty() && part.bytes().all(|b| b.is_ascii_digit()))
        }) && parts.next().is_none();
        valid.then(|| candidate.to_string())
    })
}

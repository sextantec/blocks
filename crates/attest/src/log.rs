//! Log JSONL append-only, com trava e cadeia SHA-256.

use crate::sha256;
use serde::{Deserialize, Serialize};
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Seek, SeekFrom, Write};
use std::os::fd::AsRawFd;
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::thread;
use std::time::{Duration, Instant};

pub const MAX_LINE: usize = 8 * 1024;
pub const MAX_LOG: u64 = 64 * 1024 * 1024;
const LOCK_WAIT: Duration = Duration::from_secs(5);

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Record {
    pub v: u8,
    pub seq: u64,
    pub ts: String,
    pub prev: String,
    pub kind: String,
    pub step: String,
    pub task: String,
    pub tree: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tree_before: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mutated: Option<bool>,
    pub head: Option<String>,
    pub agent: String,
    pub model: String,
    pub effort: Option<String>,
    pub authorized_by: String,
    pub result: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub argv: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub exit: Option<serde_json::Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub duration_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output_sha256: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output_bytes: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
}

#[derive(Clone)]
pub struct Verified {
    pub record: Record,
    pub raw: Vec<u8>,
}

#[derive(Debug)]
pub enum Error {
    Corrupt(String),
    Io(String),
}

impl Error {
    pub fn code(&self) -> i32 {
        match self {
            Self::Corrupt(_) => 4,
            Self::Io(_) => 4,
        }
    }

    pub fn message(&self) -> &str {
        match self {
            Self::Corrupt(message) | Self::Io(message) => message,
        }
    }
}

pub fn read(state_dir: &Path) -> Result<Vec<Verified>, Error> {
    let path = log_path(state_dir);
    let bytes = match fs::symlink_metadata(&path) {
        Ok(meta) if meta.file_type().is_symlink() || !meta.is_file() => {
            return Err(Error::Io("log.jsonl nao e arquivo regular".into()));
        }
        Ok(meta) if meta.len() > MAX_LOG => {
            return Err(Error::Corrupt("log acima de 64 MiB".into()));
        }
        Ok(_) => fs::read(&path).map_err(|e| Error::Io(format!("nao li log.jsonl: {e}")))?,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(Error::Io(format!("nao li log.jsonl: {error}"))),
    };
    verify_bytes(&bytes)
}

pub fn append(state_dir: &Path, mut record: Record) -> Result<u64, Error> {
    ensure_state_dir(state_dir)?;
    let path = log_path(state_dir);
    if let Ok(meta) = fs::symlink_metadata(&path)
        && (meta.file_type().is_symlink() || !meta.is_file())
    {
        return Err(Error::Io("log.jsonl nao e arquivo regular".into()));
    }
    let mut file = OpenOptions::new()
        .read(true)
        .append(true)
        .create(true)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
        .open(&path)
        .map_err(|e| Error::Io(format!("nao abri log.jsonl: {e}")))?;
    file.set_permissions(fs::Permissions::from_mode(0o600))
        .map_err(|e| Error::Io(format!("nao ajustei log.jsonl para 0600: {e}")))?;
    lock(&file)?;
    let size = file
        .metadata()
        .map_err(|e| Error::Io(format!("nao consultei log.jsonl: {e}")))?
        .len();
    if size > MAX_LOG {
        return Err(Error::Corrupt("log acima de 64 MiB".into()));
    }
    file.seek(SeekFrom::Start(0))
        .map_err(|e| Error::Io(format!("nao li log.jsonl: {e}")))?;
    let mut bytes = Vec::with_capacity(size as usize);
    file.read_to_end(&mut bytes)
        .map_err(|e| Error::Io(format!("nao li log.jsonl: {e}")))?;
    let records = verify_bytes(&bytes)?;
    if let Some(last) = records.last() {
        record.seq = last.record.seq.saturating_add(1);
        record.prev = sha256::hex(&last.raw);
    } else {
        record.seq = 1;
        record.prev.clear();
    }
    let mut line = serialize_record(&mut record)?;
    if line.len() + 1 > MAX_LINE {
        return Err(Error::Corrupt("registro acima de 8 KiB".into()));
    }
    if size + line.len() as u64 + 1 + u64::from(!bytes.is_empty() && !bytes.ends_with(b"\n"))
        > MAX_LOG
    {
        return Err(Error::Corrupt("log excederia 64 MiB".into()));
    }
    if !bytes.is_empty() && !bytes.ends_with(b"\n") {
        file.write_all(b"\n")
            .map_err(|e| Error::Io(format!("nao anexo log.jsonl: {e}")))?;
    }
    line.push(b'\n');
    file.write_all(&line)
        .and_then(|()| file.sync_data())
        .map_err(|e| Error::Io(format!("nao anexo log.jsonl: {e}")))?;
    Ok(record.seq)
}

fn serialize_record(record: &mut Record) -> Result<Vec<u8>, Error> {
    let argv = record.argv.take();
    let mut line = serde_json::to_vec(record)
        .map_err(|e| Error::Io(format!("nao serializo registro: {e}")))?;
    record.argv = argv.clone();
    let Some(argv) = argv else {
        return Ok(line);
    };
    if line.pop() != Some(b'}') {
        return Err(Error::Io("registro serializado invalido".into()));
    }
    line.extend_from_slice(b",\"argv\":[");
    for (index, arg) in argv.iter().enumerate() {
        if index != 0 {
            line.push(b',');
        }
        line.push(b'\"');
        for character in arg.chars() {
            let code = character as u32;
            if code <= 0xffff {
                line.extend_from_slice(format!("\\u{code:04x}").as_bytes());
            } else {
                let value = code - 0x10000;
                let high = 0xd800 + (value >> 10);
                let low = 0xdc00 + (value & 0x3ff);
                line.extend_from_slice(format!("\\u{high:04x}\\u{low:04x}").as_bytes());
            }
        }
        line.push(b'\"');
    }
    line.extend_from_slice(b"]}");
    Ok(line)
}

pub fn verify_bytes(bytes: &[u8]) -> Result<Vec<Verified>, Error> {
    if bytes.len() as u64 > MAX_LOG {
        return Err(Error::Corrupt("log acima de 64 MiB".into()));
    }
    let mut records = Vec::new();
    if bytes.is_empty() {
        return Ok(records);
    }
    let mut expected_seq = 1u64;
    let mut previous_hash = String::new();
    let content = bytes.strip_suffix(b"\n").unwrap_or(bytes);
    for (index, raw) in content.split(|byte| *byte == b'\n').enumerate() {
        let line_number = index + 1;
        if raw.is_empty() || raw.len() + 1 > MAX_LINE {
            return Err(Error::Corrupt(format!("linha {line_number} invalida")));
        }
        let record: Record = serde_json::from_slice(raw)
            .map_err(|_| Error::Corrupt(format!("linha {line_number} invalida")))?;
        if record.v != 1 || record.seq != expected_seq {
            return Err(Error::Corrupt(format!("linha {line_number} invalida")));
        }
        if record.prev != previous_hash {
            let changed_line = expected_seq.saturating_sub(1).max(1);
            return Err(Error::Corrupt(format!("linha {changed_line} invalida")));
        }
        previous_hash = sha256::hex(raw);
        records.push(Verified {
            record,
            raw: raw.to_vec(),
        });
        expected_seq = expected_seq.saturating_add(1);
    }
    Ok(records)
}

fn ensure_state_dir(path: &Path) -> Result<(), Error> {
    fs::create_dir_all(path)
        .map_err(|e| Error::Io(format!("nao criei diretorio de estado: {e}")))?;
    let meta = fs::symlink_metadata(path)
        .map_err(|e| Error::Io(format!("nao consultei diretorio de estado: {e}")))?;
    if meta.file_type().is_symlink() || !meta.is_dir() {
        return Err(Error::Io("state-dir nao e diretorio regular".into()));
    }
    fs::set_permissions(path, fs::Permissions::from_mode(0o700))
        .map_err(|e| Error::Io(format!("nao ajustei state-dir para 0700: {e}")))
}

fn lock(file: &File) -> Result<(), Error> {
    let deadline = Instant::now() + LOCK_WAIT;
    loop {
        // SAFETY: file permanece aberto ate o retorno e o lock e associado ao descritor.
        let result = unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) };
        if result == 0 {
            return Ok(());
        }
        let error = std::io::Error::last_os_error();
        if error.kind() != std::io::ErrorKind::WouldBlock || Instant::now() >= deadline {
            return Err(Error::Io("nao obtive a trava do log em 5 s".into()));
        }
        thread::sleep(Duration::from_millis(10));
    }
}

fn log_path(state_dir: &Path) -> PathBuf {
    state_dir.join("log.jsonl")
}

pub fn empty_record() -> Record {
    Record {
        v: 1,
        seq: 0,
        ts: String::new(),
        prev: String::new(),
        kind: String::new(),
        step: String::new(),
        task: String::new(),
        tree: String::new(),
        tree_before: None,
        mutated: None,
        head: None,
        agent: String::new(),
        model: String::new(),
        effort: None,
        authorized_by: String::new(),
        result: String::new(),
        argv: None,
        exit: None,
        duration_ms: None,
        output_sha256: None,
        output_bytes: None,
        detail: None,
    }
}

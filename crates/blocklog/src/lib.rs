mod error;
mod fs_storage;
mod hash;
mod log;

pub use error::{CommitError, OpenError};
pub use fs_storage::{FsStorage, SystemClock};
pub use hash::sha256_hex;
pub use log::Log;

use std::io;

use chrono::{DateTime, Utc};
use serde_json::Value;

#[allow(clippy::len_without_is_empty)]
pub trait LogFile: Send {
    fn read_all(&mut self) -> io::Result<Vec<u8>>;
    fn write_all(&mut self, buf: &[u8]) -> io::Result<()>;
    fn sync_data(&mut self) -> io::Result<()>;
    fn sync_all(&mut self) -> io::Result<()>;
    fn set_len(&mut self, len: u64) -> io::Result<()>;
    fn len(&mut self) -> io::Result<u64>;
}

pub trait Storage: Send {
    fn lock(&mut self) -> Result<(), OpenError>;
    fn open_log(&mut self) -> Result<Box<dyn LogFile>, OpenError>;
}

pub trait Clock: Send {
    fn now(&self) -> DateTime<Utc>;
}

#[derive(Clone, Debug)]
pub struct Options {
    pub line_max: usize,
    pub tx_max: usize,
    pub request_retention_days: u32,
    pub log_max_bytes: u64,
}

impl Default for Options {
    fn default() -> Self {
        Self {
            line_max: 262_144,
            tx_max: 16,
            request_retention_days: 30,
            log_max_bytes: 268_435_456,
        }
    }
}

impl Options {
    pub fn validate(&self) -> Result<(), OpenError> {
        if !(4096..=4_194_304).contains(&self.line_max)
            || !(1..=64).contains(&self.tx_max)
            || !(1..=3650).contains(&self.request_retention_days)
            || !(1_048_576..=68_719_476_736).contains(&self.log_max_bytes)
        {
            return Err(OpenError::BadOptions(
                "opcoes fora das faixas permitidas".to_string(),
            ));
        }
        Ok(())
    }
}

#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Req {
    pub agent: String,
    pub id: String,
    pub h: String,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Commit {
    pub seq: u64,
    pub ts: String,
    pub tx: Vec<Value>,
    pub req: Option<Req>,
    pub res: Option<Value>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Committed {
    pub seq: u64,
    pub replayed: bool,
    pub res: Option<Value>,
}

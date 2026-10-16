use std::{collections::HashMap, io, path::Path};

use chrono::{DateTime, Duration, SecondsFormat, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::{
    Clock, Commit, CommitError, Committed, FsStorage, LogFile, OpenError, Options, Req, Storage,
    SystemClock,
};

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct StoredCommit {
    v: u8,
    seq: u64,
    ts: String,
    tx: Vec<Value>,
    #[serde(default)]
    req: StoredReq,
    #[serde(default)]
    res: StoredValue,
}

#[derive(Default)]
enum StoredValue {
    #[default]
    Missing,
    Present(Value),
}

#[derive(Default)]
enum StoredReq {
    #[default]
    Missing,
    Present(Req),
}

impl<'de> Deserialize<'de> for StoredValue {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        Value::deserialize(deserializer).map(Self::Present)
    }
}

impl<'de> Deserialize<'de> for StoredReq {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        Req::deserialize(deserializer).map(Self::Present)
    }
}

#[derive(Serialize)]
struct EncodedCommit<'a> {
    v: u8,
    seq: u64,
    ts: &'a str,
    tx: &'a [Value],
    #[serde(skip_serializing_if = "Option::is_none")]
    req: Option<&'a Req>,
    #[serde(skip_serializing_if = "Option::is_none")]
    res: Option<&'a Value>,
}

#[derive(Clone)]
struct RequestEntry {
    seq: u64,
    h: String,
    res: Value,
    ts: DateTime<Utc>,
}

pub struct Log {
    _storage: Box<dyn Storage>,
    file: Box<dyn LogFile>,
    clock: Box<dyn Clock>,
    opts: Options,
    head_seq: u64,
    last_ts: Option<DateTime<Utc>>,
    requests: HashMap<(String, String), RequestEntry>,
    broken: bool,
}

impl Log {
    pub fn open(
        mut storage: Box<dyn Storage>,
        clock: Box<dyn Clock>,
        opts: Options,
    ) -> Result<(Log, Vec<Commit>), OpenError> {
        opts.validate()?;
        storage.lock()?;
        let mut file = storage.open_log()?;
        let opened_at = clock.now();
        let retention = Duration::days(i64::from(opts.request_retention_days));
        let records = read_records(file.as_mut(), &opts)?;
        let requests = restore_requests(&records, opened_at, retention)?;
        let commits = records.iter().map(|(commit, _)| commit.clone()).collect();
        let head_seq = records.last().map_or(0, |(commit, _)| commit.seq);
        let last_ts = records.last().map(|(_, ts)| *ts);

        Ok((
            Log {
                _storage: storage,
                file,
                clock,
                opts,
                head_seq,
                last_ts,
                requests,
                broken: false,
            },
            commits,
        ))
    }

    pub fn open_dir(dir: &Path, opts: Options) -> Result<(Log, Vec<Commit>), OpenError> {
        Self::open(Box::new(FsStorage::new(dir)), Box::new(SystemClock), opts)
    }

    pub fn commit(
        &mut self,
        tx: Vec<Value>,
        req: Option<Req>,
        res: Option<Value>,
    ) -> Result<Committed, CommitError> {
        if self.broken {
            return Err(CommitError::Broken);
        }
        validate_commit_input(&self.opts, &tx, req.as_ref(), res.as_ref())?;

        let now = self.clock.now();
        let ts = self.last_ts.map_or(now, |last| now.max(last));
        let timestamp = ts.to_rfc3339_opts(SecondsFormat::Millis, true);
        let ts = parse_timestamp(&timestamp).expect("formatted UTC timestamp is valid");
        let retention = Duration::days(i64::from(self.opts.request_retention_days));
        self.requests.retain(|_, entry| {
            entry
                .ts
                .checked_add_signed(retention)
                .is_none_or(|expiry| expiry > ts)
        });
        if let Some(req) = &req
            && let Some(committed) = self.replay_request(req)?
        {
            return Ok(committed);
        }

        let seq = self
            .head_seq
            .checked_add(1)
            .ok_or_else(|| CommitError::InvalidInput("seq esgotado".into()))?;
        let mut line = encode_commit(seq, &timestamp, &tx, req.as_ref(), res.as_ref())?;
        if line.len() > self.opts.line_max {
            return Err(CommitError::TooLarge);
        }
        self.write_line(&mut line)?;

        self.head_seq = seq;
        self.last_ts = Some(ts);
        if let (Some(req), Some(res)) = (req, res) {
            self.requests.insert(
                (req.agent, req.id),
                RequestEntry {
                    seq,
                    h: req.h,
                    res,
                    ts,
                },
            );
        }
        Ok(Committed {
            seq,
            replayed: false,
            res: None,
        })
    }

    pub fn head_seq(&self) -> u64 {
        self.head_seq
    }

    pub fn is_broken(&self) -> bool {
        self.broken
    }

    fn fail_write<T>(&mut self, error: io::Error) -> Result<T, CommitError> {
        self.broken = true;
        Err(CommitError::Io(error))
    }

    fn replay_request(&mut self, req: &Req) -> Result<Option<Committed>, CommitError> {
        let key = (req.agent.clone(), req.id.clone());
        match self.requests.get(&key) {
            Some(entry) if entry.h != req.h => Err(CommitError::RequestConflict),
            Some(entry) => Ok(Some(Committed {
                seq: entry.seq,
                replayed: true,
                res: Some(entry.res.clone()),
            })),
            None => Ok(None),
        }
    }

    fn write_line(&mut self, line: &mut Vec<u8>) -> Result<(), CommitError> {
        let file_len = match self.file.len() {
            Ok(len) => len,
            Err(error) => return self.fail_write(error),
        };
        if file_len.saturating_add(line.len() as u64).saturating_add(1) > self.opts.log_max_bytes {
            return Err(CommitError::LogFull);
        }
        line.push(b'\n');
        if let Err(error) = self.file.write_all(line) {
            return self.fail_write(error);
        }
        if let Err(error) = self.file.sync_data() {
            return self.fail_write(error);
        }
        Ok(())
    }
}

fn read_records(
    file: &mut dyn LogFile,
    opts: &Options,
) -> Result<Vec<(Commit, DateTime<Utc>)>, OpenError> {
    let bytes = file.read_all()?;
    let complete_len = bytes
        .iter()
        .rposition(|byte| *byte == b'\n')
        .map_or(0, |position| position + 1);
    if complete_len != bytes.len() {
        file.set_len(complete_len as u64)?;
        file.sync_all()?;
    }
    if complete_len == 0 {
        return Ok(Vec::new());
    }

    let complete_lines = &bytes[..complete_len - 1];
    let mut records = Vec::new();
    let mut previous_seq = 0;
    let mut previous_ts = None;
    for (index, line) in complete_lines.split(|byte| *byte == b'\n').enumerate() {
        let record = parse_record(line, index + 1, previous_seq, previous_ts, opts)?;
        previous_seq = record.0.seq;
        previous_ts = Some(record.1);
        records.push(record);
    }
    Ok(records)
}

fn parse_record(
    line: &[u8],
    line_number: usize,
    previous_seq: u64,
    previous_ts: Option<DateTime<Utc>>,
    opts: &Options,
) -> Result<(Commit, DateTime<Utc>), OpenError> {
    if line.len() > opts.line_max {
        return Err(corrupt(line_number, "linha acima de line_max"));
    }
    let stored: StoredCommit = serde_json::from_slice(line)
        .map_err(|error| corrupt(line_number, format!("JSON ou campos invalidos: {error}")))?;
    if stored.v != 1 {
        return Err(corrupt(line_number, "versao diferente de 1"));
    }
    if previous_seq.checked_add(1) != Some(stored.seq) {
        return Err(corrupt(line_number, "seq nao e o anterior mais um"));
    }
    let parsed_ts = parse_timestamp(&stored.ts).map_err(|reason| corrupt(line_number, reason))?;
    if previous_ts.is_some_and(|previous| parsed_ts < previous) {
        return Err(corrupt(line_number, "ts regressivo"));
    }
    validate_tx(&stored.tx, opts.tx_max).map_err(|reason| corrupt(line_number, reason))?;
    let req = match stored.req {
        StoredReq::Missing => None,
        StoredReq::Present(req) => Some(req),
    };
    if let Some(req) = &req {
        validate_req(req).map_err(|reason| corrupt(line_number, reason))?;
    }
    let res = match stored.res {
        StoredValue::Missing => None,
        StoredValue::Present(value) => Some(value),
    };
    if req.is_some() && res.is_none() {
        return Err(corrupt(line_number, "req sem res"));
    }
    Ok((
        Commit {
            seq: stored.seq,
            ts: stored.ts,
            tx: stored.tx,
            req,
            res,
        },
        parsed_ts,
    ))
}

fn restore_requests(
    records: &[(Commit, DateTime<Utc>)],
    opened_at: DateTime<Utc>,
    retention: Duration,
) -> Result<HashMap<(String, String), RequestEntry>, OpenError> {
    let mut requests = HashMap::new();
    let mut seen: HashMap<(String, String), DateTime<Utc>> = HashMap::new();
    for (commit, ts) in records {
        let Some(req) = &commit.req else {
            continue;
        };
        let key = (req.agent.clone(), req.id.clone());
        if let Some(first) = seen.get(&key)
            && first
                .checked_add_signed(retention)
                .is_none_or(|expiry| *ts < expiry)
        {
            return Err(corrupt(
                commit.seq as usize,
                "req repetida antes do fim da retencao",
            ));
        }
        seen.insert(key.clone(), *ts);
        if ts
            .checked_add_signed(retention)
            .is_none_or(|expiry| expiry > opened_at)
        {
            requests.insert(
                key,
                RequestEntry {
                    seq: commit.seq,
                    h: req.h.clone(),
                    res: commit.res.clone().expect("req requires a result"),
                    ts: *ts,
                },
            );
        }
    }
    Ok(requests)
}

fn encode_commit(
    seq: u64,
    ts: &str,
    tx: &[Value],
    req: Option<&Req>,
    res: Option<&Value>,
) -> Result<Vec<u8>, CommitError> {
    serde_json::to_vec(&EncodedCommit {
        v: 1,
        seq,
        ts,
        tx,
        req,
        res,
    })
    .map_err(|error| CommitError::InvalidInput(error.to_string()))
}

fn validate_tx(tx: &[Value], tx_max: usize) -> Result<(), String> {
    if tx.is_empty() {
        return Err("tx vazio".into());
    }
    if tx.len() > tx_max {
        return Err("tx excede tx_max".into());
    }
    if tx.iter().any(|event| !event.is_object()) {
        return Err("cada evento de tx deve ser um objeto JSON".into());
    }
    Ok(())
}

fn validate_commit_input(
    opts: &Options,
    tx: &[Value],
    req: Option<&Req>,
    res: Option<&Value>,
) -> Result<(), CommitError> {
    if tx.is_empty() {
        return Err(CommitError::EmptyTx);
    }
    if tx.len() > opts.tx_max {
        return Err(CommitError::TxTooLarge);
    }
    validate_tx(tx, opts.tx_max).map_err(CommitError::InvalidInput)?;
    if let Some(req) = req {
        validate_req(req).map_err(CommitError::InvalidInput)?;
    }
    if req.is_some() && res.is_none() {
        return Err(CommitError::MissingRes);
    }
    Ok(())
}

fn validate_req(req: &Req) -> Result<(), String> {
    if req.agent.is_empty()
        || req.agent.len() > 32
        || !req
            .agent
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || b"_-".contains(&byte))
    {
        return Err("Req.agent fora do formato".into());
    }
    if req.id.is_empty()
        || req.id.len() > 64
        || !req
            .id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"._-".contains(&byte))
    {
        return Err("Req.id fora do formato".into());
    }
    if req.h.len() != 64
        || !req
            .h
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        return Err("Req.h deve ser SHA-256 hexadecimal minusculo".into());
    }
    Ok(())
}

fn parse_timestamp(value: &str) -> Result<DateTime<Utc>, String> {
    let parsed = DateTime::parse_from_rfc3339(value)
        .map_err(|error| format!("ts nao e RFC 3339: {error}"))?
        .with_timezone(&Utc);
    if parsed.to_rfc3339_opts(SecondsFormat::Millis, true) != value {
        return Err("ts deve ser UTC com milissegundos".into());
    }
    Ok(parsed)
}

fn corrupt(line: usize, reason: impl Into<String>) -> OpenError {
    OpenError::Corrupt {
        line,
        reason: reason.into(),
    }
}

//! Log append-only (`log.jsonl`), dobra dos eventos, recuperacao de partida e trava (spec v1 6 e v1.1).

use std::collections::{BTreeMap, HashMap, HashSet, VecDeque};
use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::os::fd::AsRawFd;
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::Path;
use std::time::{Duration, Instant};

use chrono::{DateTime, Utc};
use serde_json::{Map, Value, json};

use crate::protocol::{self, AckParams, ApiError, Ref, SendParams};

const BELL_FAIL_INTERVAL: Duration = Duration::from_secs(60);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum State {
    Registrado,
    Aceito,
    Resultado,
}

impl State {
    pub fn as_str(self) -> &'static str {
        match self {
            State::Registrado => "registrado",
            State::Aceito => "aceito",
            State::Resultado => "resultado",
        }
    }
}

#[derive(Debug, Clone)]
pub struct Msg {
    pub seq: u64,
    pub id: String,
    pub from: String,
    pub to: String,
    pub kind: String,
    pub subject: String,
    pub body: String,
    pub thread: Option<String>,
    pub refs: Vec<Ref>,
    pub hash: String,
    pub state: State,
    pub registered_at: String,
    pub accepted_at: Option<String>,
    pub resulted_at: Option<String>,
    pub result: Option<String>,
    pub note: Option<String>,
    pub due_s: Option<u64>,
    pub await_class: Option<String>,
    pub since: Option<String>,
}

#[derive(Debug, Clone)]
pub struct BlobRecord {
    pub id: String,
    pub owner: String,
    pub to: String,
    pub size: u64,
    pub name: String,
    pub sha256: String,
    pub created_at: String,
    pub ttl_s: u64,
    pub read_by: HashSet<String>,
}

/// Mensagem no formato do protocolo (spec v1 5.2, com `refs` da v1.1).
pub fn msg_json(m: &Msg) -> Value {
    json!({
        "v": 1,
        "seq": m.seq,
        "id": m.id,
        "from": m.from,
        "to": m.to,
        "kind": m.kind,
        "subject": m.subject,
        "body": m.body,
        "thread": m.thread,
        "refs": protocol::refs_to_json(&m.refs),
        "state": m.state.as_str(),
        "registered_at": m.registered_at,
        "accepted_at": m.accepted_at,
        "resulted_at": m.resulted_at,
        "result": m.result,
        "note": m.note,
        "due_s": m.due_s,
        "await": m.await_class,
        "since": m.since,
    })
}

pub fn now_ts() -> String {
    chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true)
}

fn replay_bell_instant(ts: &str, wall_now: DateTime<Utc>, monotonic_now: Instant) -> Instant {
    let age = DateTime::parse_from_rfc3339(ts)
        .ok()
        .and_then(|event_ts| {
            wall_now
                .signed_duration_since(event_ts.with_timezone(&Utc))
                .to_std()
                .ok()
        })
        .unwrap_or_default();
    monotonic_now.checked_sub(age).unwrap_or(monotonic_now)
}

pub(crate) fn sliding_rate_available(
    times: &mut VecDeque<Instant>,
    now: Instant,
    limit: usize,
    window: Duration,
) -> bool {
    while times
        .front()
        .is_some_and(|t| now.duration_since(*t) >= window)
    {
        times.pop_front();
    }
    times.len() < limit
}

pub(crate) fn charge_sliding_rate(
    times: &mut VecDeque<Instant>,
    now: Instant,
    limit: usize,
    window: Duration,
) -> bool {
    if !sliding_rate_available(times, now, limit, window) {
        return false;
    }
    times.push_back(now);
    true
}

fn internal(msg: String) -> ApiError {
    ApiError::new("internal_error", msg)
}

fn str_field<'a>(obj: &'a Map<String, Value>, key: &str) -> Result<&'a str, String> {
    obj.get(key)
        .and_then(Value::as_str)
        .ok_or_else(|| format!("campo '{key}' ausente ou invalido"))
}

fn valid_blob_id(s: &str) -> bool {
    s.strip_prefix("blob:").is_some_and(|hex| {
        hex.len() == 32
            && hex
                .bytes()
                .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
    })
}

fn valid_blob_name(s: &str) -> bool {
    (1..=64).contains(&s.len())
        && s.bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-'))
}

fn valid_sha256(s: &str) -> bool {
    s.len() == 64
        && s.bytes()
            .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
}

pub struct Store {
    log: File,
    _lock: File,
    events: Vec<Value>,
    msgs: HashMap<String, Msg>,
    /// Destinatario -> tem campainha configurada (delivery bell).
    recipients: BTreeMap<String, bool>,
    last_bell_seq: HashMap<String, u64>,
    /// Campainha tocada e ainda sem resposta do destinatario (so em memoria).
    outstanding: HashMap<String, Instant>,
    bell_fail_at: HashMap<String, Instant>,
    broken: bool,
    receipt_times: HashMap<String, VecDeque<Instant>>,
    pub blobs: HashMap<String, BlobRecord>,
}

fn open_private(path: &Path) -> Result<File, String> {
    let f = OpenOptions::new()
        .read(true)
        .append(true)
        .create(true)
        .mode(0o600)
        .open(path)
        .map_err(|e| format!("nao abri {}: {e}", path.display()))?;
    fs::set_permissions(path, fs::Permissions::from_mode(0o600))
        .map_err(|e| format!("nao ajustei permissoes de {}: {e}", path.display()))?;
    Ok(f)
}

impl Store {
    /// Abre o diretorio de estado: trava exclusiva, releitura do log e recuperacao da cauda.
    pub fn open(state_dir: &Path, recipients: BTreeMap<String, bool>) -> Result<Store, String> {
        let lock = open_private(&state_dir.join("chatd.lock"))?;
        // SAFETY: descritor valido do arquivo de trava que fica aberto durante a vida do Store.
        let rc = unsafe { libc::flock(lock.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) };
        if rc != 0 {
            return Err("outro servidor ja usa este diretorio (trava chatd.lock)".to_string());
        }
        let log_path = state_dir.join("log.jsonl");
        let log = open_private(&log_path)?;
        let data = fs::read(&log_path).map_err(|e| format!("nao li o log: {e}"))?;
        let mut store = Store {
            log,
            _lock: lock,
            events: Vec::new(),
            msgs: HashMap::new(),
            recipients,
            last_bell_seq: HashMap::new(),
            outstanding: HashMap::new(),
            bell_fail_at: HashMap::new(),
            broken: false,
            receipt_times: HashMap::new(),
            blobs: HashMap::new(),
        };
        let mut pos = 0usize;
        let mut cut: Option<usize> = None;
        while pos < data.len() {
            let rest = &data[pos..];
            let Some(nl) = rest.iter().position(|&b| b == b'\n') else {
                cut = Some(pos);
                break;
            };
            let is_last = pos + nl + 1 == data.len();
            match serde_json::from_slice::<Value>(&rest[..nl]) {
                Ok(ev) => {
                    let n = store.events.len() + 1;
                    store
                        .apply(&ev)
                        .map_err(|e| format!("log invalido na linha {n}: {e}"))?;
                }
                Err(e) => {
                    if is_last {
                        cut = Some(pos);
                        break;
                    }
                    let n = store.events.len() + 1;
                    return Err(format!(
                        "log invalido na linha {n}, que nao e a ultima: {e}"
                    ));
                }
            }
            pos += nl + 1;
        }
        if let Some(at) = cut {
            let dropped = data.len() - at;
            let f = OpenOptions::new()
                .write(true)
                .open(&log_path)
                .map_err(|e| format!("nao abri o log para truncar: {e}"))?;
            f.set_len(at as u64)
                .map_err(|e| format!("nao truncou o log: {e}"))?;
            f.sync_all()
                .map_err(|e| format!("nao sincronizou o log: {e}"))?;
            eprintln!("chatd: aviso: ultima linha truncada descartada do log ({dropped} bytes)");
        }
        Ok(store)
    }

    /// Aplica um evento ao estado; valida antes de mutar. Usado na releitura e em cada escrita.
    fn apply(&mut self, ev: &Value) -> Result<(), String> {
        let obj = ev.as_object().ok_or("evento nao e um objeto JSON")?;
        if obj.get("v").and_then(Value::as_u64) != Some(1) {
            return Err("versao do evento diferente de 1".to_string());
        }
        let expected = self.events.len() as u64 + 1;
        let seq = obj
            .get("seq")
            .and_then(Value::as_u64)
            .ok_or("campo 'seq' ausente")?;
        if seq != expected {
            return Err(format!(
                "seq {seq} fora de ordem ou com lacuna (esperado {expected})"
            ));
        }
        let ts = str_field(obj, "ts")?.to_string();
        let note = obj.get("note").and_then(Value::as_str).map(str::to_string);
        match str_field(obj, "type")? {
            "registered" => {
                let id = str_field(obj, "id")?;
                if self.msgs.contains_key(id) {
                    return Err(format!("id '{id}' registrado duas vezes"));
                }
                let from = str_field(obj, "from")?;
                let to = str_field(obj, "to")?;
                let kind = str_field(obj, "kind")?;
                let subject = str_field(obj, "subject")?;
                let body = str_field(obj, "body")?;
                let thread = match obj.get("thread") {
                    None | Some(Value::Null) => None,
                    Some(Value::String(s)) => Some(s.clone()),
                    Some(_) => return Err("campo 'thread' invalido".to_string()),
                };
                let refs = protocol::parse_refs(obj.get("refs")).map_err(|e| e.message)?;
                let hash = protocol::envelope_hash(
                    from,
                    to,
                    kind,
                    subject,
                    body,
                    thread.as_deref(),
                    &refs,
                );
                let msg = Msg {
                    seq,
                    id: id.to_string(),
                    from: from.to_string(),
                    to: to.to_string(),
                    kind: kind.to_string(),
                    subject: subject.to_string(),
                    body: body.to_string(),
                    thread,
                    refs,
                    hash,
                    state: State::Registrado,
                    registered_at: ts,
                    accepted_at: None,
                    resulted_at: None,
                    result: None,
                    note: None,
                    due_s: obj.get("due_s").and_then(Value::as_u64),
                    await_class: None,
                    since: None,
                };
                self.msgs.insert(id.to_string(), msg);
            }
            "accepted" => {
                let id = str_field(obj, "id")?;
                let m = self
                    .msgs
                    .get_mut(id)
                    .ok_or_else(|| format!("accepted para id desconhecido '{id}'"))?;
                if m.state != State::Registrado {
                    return Err(format!(
                        "accepted invalido para '{id}' no estado {}",
                        m.state.as_str()
                    ));
                }
                m.state = State::Aceito;
                m.accepted_at = Some(ts);
                if note.is_some() {
                    m.note = note;
                }
                let to = m.to.clone();
                self.outstanding.remove(&to);
            }
            "resulted" => {
                let id = str_field(obj, "id")?;
                let result = str_field(obj, "result")?;
                if !matches!(result, "done" | "refused" | "failed") {
                    return Err(format!("result invalido '{result}'"));
                }
                let m = self
                    .msgs
                    .get_mut(id)
                    .ok_or_else(|| format!("resulted para id desconhecido '{id}'"))?;
                if m.state == State::Resultado {
                    return Err(format!("resulted repetido para '{id}'"));
                }
                m.state = State::Resultado;
                m.resulted_at = Some(ts);
                m.result = Some(result.to_string());
                m.await_class = None;
                m.since = None;
                if note.is_some() {
                    m.note = note;
                }
                let to = m.to.clone();
                self.outstanding.remove(&to);
            }
            "blocked" => {
                let id = str_field(obj, "id")?;
                let await_class = match obj.get("await") {
                    None | Some(Value::Null) => None,
                    Some(Value::String(v))
                        if [
                            "permission",
                            "user_input",
                            "network",
                            "identity",
                            "policy",
                            "unknown",
                        ]
                        .contains(&v.as_str()) =>
                    {
                        Some(v.clone())
                    }
                    _ => return Err("await de blocked invalido".to_string()),
                };
                let m = self
                    .msgs
                    .get_mut(id)
                    .ok_or_else(|| format!("blocked para id desconhecido '{id}'"))?;
                if m.state != State::Aceito {
                    return Err(format!(
                        "blocked invalido para '{id}' no estado {}",
                        m.state.as_str()
                    ));
                }
                m.since = if await_class.is_some() {
                    Some(
                        obj.get("since")
                            .and_then(Value::as_str)
                            .unwrap_or(&ts)
                            .to_string(),
                    )
                } else {
                    None
                };
                m.await_class = await_class;
            }
            "auth_failed" => {}
            "denied" => {
                str_field(obj, "method")?;
            }
            "bound" => {
                if !protocol::valid_name(str_field(obj, "name")?)
                    || !matches!(str_field(obj, "mux")?, "herdr" | "tmux")
                    || !protocol::valid_pane_id(str_field(obj, "pane")?)
                {
                    return Err("evento bound invalido".to_string());
                }
                obj.get("peer_pid")
                    .and_then(Value::as_i64)
                    .filter(|pid| *pid > 0)
                    .ok_or("campo 'peer_pid' ausente ou invalido")?;
            }
            "unbound" => {
                if !protocol::valid_name(str_field(obj, "name")?)
                    || !matches!(str_field(obj, "mux")?, "herdr" | "tmux")
                    || !protocol::valid_pane_id(str_field(obj, "pane")?)
                {
                    return Err("evento unbound invalido".to_string());
                }
                if !matches!(
                    str_field(obj, "reason")?,
                    "expired" | "process_gone" | "replaced"
                ) {
                    return Err("reason de unbound invalido".to_string());
                }
            }
            "bell_sent" => {
                let r = str_field(obj, "recipient")?.to_string();
                self.last_bell_seq.insert(r.clone(), seq);
                self.outstanding
                    .insert(r, replay_bell_instant(&ts, Utc::now(), Instant::now()));
            }
            "bell_failed" => {
                str_field(obj, "recipient")?;
                str_field(obj, "reason")?;
            }
            kind @ ("blob_put" | "blob_got" | "blob_removed" | "blob_expired") => {
                self.apply_blob_event(kind, obj)?;
            }
            other => return Err(format!("tipo de evento desconhecido '{other}'")),
        }
        self.events.push(ev.clone());
        Ok(())
    }

    fn apply_blob_event(&mut self, kind: &str, obj: &Map<String, Value>) -> Result<(), String> {
        let id = str_field(obj, "blob")?;
        if !valid_blob_id(id) {
            return Err("id de blob invalido".to_string());
        }
        match kind {
            "blob_put" => {
                if self.blobs.contains_key(id) {
                    return Err("id de blob duplicado".to_string());
                }
                let record = BlobRecord {
                    id: id.to_string(),
                    owner: str_field(obj, "owner")?.to_string(),
                    to: str_field(obj, "to")?.to_string(),
                    size: obj
                        .get("size")
                        .and_then(Value::as_u64)
                        .ok_or("size ausente")?,
                    name: str_field(obj, "name")?.to_string(),
                    sha256: str_field(obj, "sha256")?.to_string(),
                    created_at: str_field(obj, "created_at")?.to_string(),
                    ttl_s: obj
                        .get("ttl_s")
                        .and_then(Value::as_u64)
                        .ok_or("ttl_s ausente")?,
                    read_by: HashSet::new(),
                };
                if !protocol::valid_name(&record.owner)
                    || !protocol::valid_name(&record.to)
                    || !valid_blob_name(&record.name)
                    || !valid_sha256(&record.sha256)
                    || !(60..=604_800).contains(&record.ttl_s)
                    || record.size > 8 * 1024 * 1024
                {
                    return Err("evento blob_put invalido".to_string());
                }
                self.blobs.insert(id.to_string(), record);
            }
            "blob_got" => {
                let record = self
                    .blobs
                    .get(id)
                    .ok_or_else(|| "blob_got para id ausente".to_string())?;
                let by = str_field(obj, "by")?;
                let offset = obj
                    .get("offset")
                    .and_then(Value::as_u64)
                    .ok_or("offset ausente")?;
                let length = obj
                    .get("length")
                    .and_then(Value::as_u64)
                    .ok_or("length ausente")?;
                if !protocol::valid_name(by)
                    || (by != record.owner && by != record.to)
                    || str_field(obj, "owner")? != record.owner
                    || str_field(obj, "to")? != record.to
                    || offset
                        .checked_add(length)
                        .is_none_or(|end| end > record.size)
                {
                    return Err("evento blob_got invalido".to_string());
                }
                self.blobs
                    .get_mut(id)
                    .expect("blob record validated")
                    .read_by
                    .insert(by.to_string());
            }
            "blob_removed" | "blob_expired" => {
                let record = self
                    .blobs
                    .get(id)
                    .ok_or_else(|| "remocao para blob ausente".to_string())?;
                let reason = str_field(obj, "reason")?;
                if str_field(obj, "owner")? != record.owner
                    || str_field(obj, "to")? != record.to
                    || (kind == "blob_expired" && reason != "ttl")
                    || (kind == "blob_removed" && !matches!(reason, "requested" | "missing"))
                {
                    return Err("evento de remocao de blob invalido".to_string());
                }
                self.blobs.remove(id);
            }
            _ => return Err("tipo de evento blob invalido".to_string()),
        }
        Ok(())
    }

    /// Grava um evento: aplica, escreve a linha e faz `sync_data` antes de retornar.
    fn append(&mut self, ev: Map<String, Value>) -> Result<(), ApiError> {
        self.append_many(vec![ev])
    }

    /// Grava eventos sequenciais em um bloco e sincroniza uma vez.
    fn append_many(&mut self, events: Vec<Map<String, Value>>) -> Result<(), ApiError> {
        if self.broken {
            return Err(internal(
                "log indisponivel apos falha de escrita".to_string(),
            ));
        }
        let mut bytes = Vec::new();
        for mut ev in events {
            ev.insert("v".into(), json!(1));
            ev.insert("seq".into(), json!(self.events.len() as u64 + 1));
            // apply() pushes the event, so calculate each next sequence from the new head.
            let value = Value::Object(ev);
            self.apply(&value).map_err(internal)?;
            serde_json::to_writer(&mut bytes, &value).map_err(|e| internal(e.to_string()))?;
            bytes.push(b'\n');
        }
        if self
            .log
            .write_all(&bytes)
            .and_then(|()| self.log.sync_data())
            .is_err()
        {
            self.broken = true;
            return Err(internal("falha ao gravar o log".to_string()));
        }
        Ok(())
    }

    fn append_event(
        &mut self,
        kind: &str,
        fields: impl IntoIterator<Item = (&'static str, Value)>,
    ) -> Result<(), ApiError> {
        let mut ev = Map::new();
        ev.insert("type".into(), json!(kind));
        ev.insert("ts".into(), json!(now_ts()));
        for (key, value) in fields {
            ev.insert(key.into(), value);
        }
        self.append(ev)
    }

    pub fn head(&self) -> u64 {
        self.events.len() as u64
    }

    pub fn blob_event(
        &mut self,
        kind: &str,
        fields: impl IntoIterator<Item = (&'static str, Value)>,
    ) -> Result<(), ApiError> {
        self.append_event(kind, fields)
    }

    pub fn mailbox_counts_for(&self, who: &str) -> Value {
        let mut pending = 0u64;
        let mut accepted = 0u64;
        for m in self.msgs.values().filter(|m| m.to == who) {
            match m.state {
                State::Registrado => pending += 1,
                State::Aceito => accepted += 1,
                State::Resultado => {}
            }
        }
        let bell = if !self.recipients.get(who).copied().unwrap_or(false) {
            "none"
        } else if self.outstanding.contains_key(who) {
            "outstanding"
        } else {
            "idle_wait"
        };
        json!({"v": 1, "pending": pending, "accepted": accepted, "bell": bell,
            "last_bell_seq": self.last_bell_seq.get(who).copied()})
    }

    pub fn status_stats(&self, uptime_s: u64, connections: usize) -> Value {
        let mut registered = 0u64;
        let mut accepted = 0u64;
        let mut resulted = 0u64;
        for m in self.msgs.values() {
            match m.state {
                State::Registrado => registered += 1,
                State::Aceito => accepted += 1,
                State::Resultado => resulted += 1,
            }
        }
        let sent = self
            .events
            .iter()
            .filter(|e| e["type"] == "bell_sent")
            .count();
        let failed = self
            .events
            .iter()
            .filter(|e| e["type"] == "bell_failed")
            .count();
        let bytes: u64 = self.blobs.values().map(|b| b.size).sum();
        json!({"v":1,"ok":true,"version":env!("CARGO_PKG_VERSION"),"uptime_s":uptime_s,
            "head_seq":self.head(),"messages":{"total":self.msgs.len(),"registered":registered,"accepted":accepted,"resulted":resulted},
            "bells":{"sent":sent,"failed":failed},"connections":connections,
            "blobs":{"count":self.blobs.len(),"bytes":bytes}})
    }

    /// Eventos com seq > after, em ordem.
    pub fn events_after(&self, after: u64) -> Vec<Value> {
        self.events.iter().skip(after as usize).cloned().collect()
    }

    /// Evento visivel para a identidade `who` (v1.1 B: from, to ou recipient).
    pub fn visible(&self, ev: &Value, who: &str) -> bool {
        let Some(obj) = ev.as_object() else {
            return false;
        };
        match obj.get("type").and_then(Value::as_str) {
            Some("registered") => {
                obj.get("from").and_then(Value::as_str) == Some(who)
                    || obj.get("to").and_then(Value::as_str) == Some(who)
            }
            Some("accepted") | Some("resulted") | Some("blocked") => {
                let id = obj.get("id").and_then(Value::as_str).unwrap_or("");
                self.msgs
                    .get(id)
                    .is_some_and(|m| m.from == who || m.to == who)
            }
            Some("bell_sent") | Some("bell_failed") => {
                obj.get("recipient").and_then(Value::as_str) == Some(who)
            }
            Some("blob_put") | Some("blob_got") | Some("blob_removed") | Some("blob_expired") => {
                obj.get("owner").and_then(Value::as_str) == Some(who)
                    || obj.get("to").and_then(Value::as_str) == Some(who)
            }
            _ => false,
        }
    }

    /// `message.send` com idempotencia por id. `from` e a identidade ja resolvida.
    /// Devolve (mensagem, duplicate, id fechado por fechamento automatico).
    pub fn send(
        &mut self,
        from: &str,
        p: &SendParams,
    ) -> Result<(Msg, bool, Option<String>), ApiError> {
        let hash = protocol::envelope_hash(
            from,
            &p.to,
            &p.kind,
            &p.subject,
            &p.body,
            p.thread.as_deref(),
            &p.refs,
        );
        if let Some(m) = self.msgs.get(&p.id) {
            if m.hash == hash {
                let closed = p.thread.as_ref().and_then(|target| {
                    self.msgs.get(target).and_then(|target_msg| {
                        let auto_note = format!("auto:{}", p.id);
                        (target_msg.state == State::Resultado
                            && target_msg.note.as_deref() == Some(auto_note.as_str()))
                        .then(|| target.clone())
                    })
                });
                return Ok((m.clone(), true, closed));
            }
            return Err(ApiError::new(
                "envelope_mismatch",
                "id ja usado com outro envelope",
            ));
        }
        if !self.recipients.contains_key(&p.to) {
            return Err(ApiError::new(
                "unknown_recipient",
                format!("destinatario '{}' nao configurado", p.to),
            ));
        }
        if let Some(t) = &p.thread
            && !self.msgs.contains_key(t)
        {
            return Err(ApiError::new(
                "unknown_message",
                "thread aponta para mensagem nao registrada",
            ));
        }
        let ts = now_ts();
        let mut ev = Map::new();
        ev.insert("type".into(), json!("registered"));
        ev.insert("ts".into(), json!(ts));
        ev.insert("id".into(), json!(p.id));
        ev.insert("from".into(), json!(from));
        ev.insert("to".into(), json!(p.to));
        ev.insert("kind".into(), json!(p.kind));
        ev.insert("subject".into(), json!(p.subject));
        ev.insert("body".into(), json!(p.body));
        ev.insert("thread".into(), json!(p.thread));
        ev.insert("refs".into(), protocol::refs_to_json(&p.refs));
        ev.insert("state".into(), json!("registrado"));
        ev.insert("registered_at".into(), json!(ts));
        ev.insert("accepted_at".into(), Value::Null);
        ev.insert("resulted_at".into(), Value::Null);
        ev.insert("result".into(), Value::Null);
        ev.insert("note".into(), Value::Null);
        if let Some(due_s) = p.due_s {
            ev.insert("due_s".into(), json!(due_s));
        }
        let target_to_close = if !p.keep_open && p.kind == "RESPOSTA" {
            p.thread.as_ref().and_then(|target_id| {
                self.msgs.get(target_id).and_then(|target| {
                    (matches!(target.state, State::Registrado | State::Aceito)
                        && from == target.to
                        && p.to == target.from)
                        .then(|| target_id.clone())
                })
            })
        } else {
            None
        };
        let mut events = vec![ev];
        if let Some(target_id) = &target_to_close {
            let mut resulted = Map::new();
            resulted.insert("type".into(), json!("resulted"));
            resulted.insert("ts".into(), json!(ts));
            resulted.insert("id".into(), json!(target_id));
            resulted.insert("result".into(), json!("done"));
            resulted.insert("note".into(), json!(format!("auto:{}", p.id)));
            events.push(resulted);
        }
        self.append_many(events)?;
        let m = self
            .msgs
            .get(&p.id)
            .cloned()
            .ok_or_else(|| internal("mensagem nao encontrada apos gravar".to_string()))?;
        Ok((m, false, target_to_close))
    }

    pub fn list(&self, to: &str, after: u64, limit: usize, unresolved_only: bool) -> Vec<Msg> {
        let mut v: Vec<&Msg> = self
            .msgs
            .values()
            .filter(|m| {
                m.to == to && m.seq > after && (!unresolved_only || m.state != State::Resultado)
            })
            .collect();
        v.sort_by_key(|m| m.seq);
        v.into_iter().take(limit).cloned().collect()
    }

    /// `message.get` so revela mensagens enviadas ou recebidas pela identidade.
    pub fn get_for(&self, id: &str, who: &str) -> Option<Msg> {
        self.msgs
            .get(id)
            .filter(|m| m.from == who || m.to == who)
            .cloned()
    }

    /// Evento que pode aparecer em `watch --receipts`: mensagens recebidas e recibos enviados.
    pub fn receipt_event(&self, ev: &Value, who: &str) -> Option<Value> {
        let kind = ev.get("type").and_then(Value::as_str)?;
        if kind == "registered" {
            return (ev.get("from").and_then(Value::as_str) == Some(who)
                || ev.get("to").and_then(Value::as_str) == Some(who))
            .then(|| ev.clone());
        }
        if !matches!(kind, "accepted" | "resulted" | "blocked") {
            return None;
        }
        let id = ev.get("id").and_then(Value::as_str)?;
        let message = self.msgs.get(id).filter(|m| {
            if kind == "blocked" {
                m.from == who || m.to == who
            } else {
                m.from == who && m.to != who
            }
        })?;
        let mut visible = ev.clone();
        if let Some(obj) = visible.as_object_mut() {
            obj.insert("receipt_to".into(), json!(message.to));
        }
        Some(visible)
    }

    /// `message.take` (v1.1 C.1): sob o lock do servidor, marca como `aceito` (note "take") as
    /// mensagens `registrado` de `to`, em ordem de seq, ate `limit`, e devolve-as.
    pub fn take(&mut self, to: &str, limit: usize) -> Result<Vec<Msg>, ApiError> {
        let mut sel: Vec<(u64, String)> = self
            .msgs
            .values()
            .filter(|m| m.to == to && m.state == State::Registrado)
            .map(|m| (m.seq, m.id.clone()))
            .collect();
        sel.sort();
        sel.truncate(limit);
        let ids: Vec<String> = sel.into_iter().map(|(_, id)| id).collect();
        for id in &ids {
            let mut ev = Map::new();
            ev.insert("type".into(), json!("accepted"));
            ev.insert("ts".into(), json!(now_ts()));
            ev.insert("id".into(), json!(id));
            ev.insert("note".into(), json!("take"));
            self.append(ev)?;
        }
        Ok(ids
            .iter()
            .filter_map(|id| self.msgs.get(id).cloned())
            .collect())
    }

    /// `message.ack`. `by` e a identidade ja resolvida. Devolve (mensagem atualizada, duplicate).
    pub fn ack(&mut self, by: &str, p: &AckParams) -> Result<(Msg, bool), ApiError> {
        let m = self
            .msgs
            .get(&p.id)
            .cloned()
            .ok_or_else(|| ApiError::new("unknown_message", "mensagem nao registrada"))?;
        if m.to != by {
            if p.stage == "bloqueado" {
                return Err(ApiError::new("not_found", "mensagem nao encontrada"));
            }
            return Err(ApiError::new(
                "forbidden",
                "so o destinatario confirma a mensagem",
            ));
        }
        if p.stage == "bloqueado" {
            if m.state != State::Aceito {
                return Err(ApiError::new(
                    "invalid_transition",
                    "bloqueado exige mensagem aceita",
                ));
            }
            let next = p.await_class.as_deref().expect("parse_ack exige await");
            if m.await_class.as_deref() == Some(next) {
                return Ok((m, true));
            }
            let count = self
                .events
                .iter()
                .filter(|e| e["type"] == "blocked" && e["id"] == p.id)
                .count();
            if count >= 6 {
                return Err(ApiError::new(
                    "rate_limited",
                    "limite de bloqueios por mensagem",
                ));
            }
            self.check_receipt_rate(by)?;
            let mut ev = Map::new();
            ev.insert("type".into(), json!("blocked"));
            ev.insert("id".into(), json!(p.id));
            ev.insert("by".into(), json!(by));
            let ts = now_ts();
            ev.insert("ts".into(), json!(ts));
            ev.insert("since".into(), json!(ts));
            ev.insert(
                "await".into(),
                if next == "none" {
                    Value::Null
                } else {
                    json!(next)
                },
            );
            ev.insert("note".into(), Value::Null);
            self.append(ev)?;
            let updated = self
                .msgs
                .get(&p.id)
                .cloned()
                .ok_or_else(|| internal("mensagem sumiu apos bloqueio".to_string()))?;
            return Ok((updated, false));
        }
        self.check_receipt_rate(by)?;
        let mut ev = Map::new();
        ev.insert("id".into(), json!(p.id));
        ev.insert("ts".into(), json!(now_ts()));
        ev.insert("note".into(), json!(p.note));
        if p.stage == "aceito" {
            match m.state {
                State::Registrado => {
                    ev.insert("type".into(), json!("accepted"));
                    self.append(ev)?;
                }
                State::Aceito => return Ok((m, true)),
                State::Resultado => {
                    return Err(ApiError::new("invalid_transition", "mensagem ja resolvida"));
                }
            }
        } else {
            let r = p.result.clone().unwrap_or_else(|| "done".to_string());
            if m.state == State::Resultado {
                if m.result.as_deref() == Some(r.as_str()) {
                    return Ok((m, true));
                }
                return Err(ApiError::new(
                    "invalid_transition",
                    "mensagem ja resolvida com outro resultado",
                ));
            }
            ev.insert("type".into(), json!("resulted"));
            ev.insert("result".into(), json!(r));
            self.append(ev)?;
        }
        let m = self
            .msgs
            .get(&p.id)
            .cloned()
            .ok_or_else(|| internal("mensagem sumiu apos ack".to_string()))?;
        Ok((m, false))
    }

    fn check_receipt_rate(&mut self, by: &str) -> Result<(), ApiError> {
        let times = self.receipt_times.entry(by.to_string()).or_default();
        if !charge_sliding_rate(times, Instant::now(), 30, Duration::from_secs(60)) {
            return Err(ApiError::new(
                "rate_limited",
                "limite de recibos por minuto",
            ));
        }
        Ok(())
    }

    /// Falha de autenticacao (v1.1 B): nunca grava o token.
    pub fn auth_failed(
        &mut self,
        name: Option<&str>,
        peer_uid: u32,
        peer_pid: i32,
    ) -> Result<(), ApiError> {
        let mut fields = vec![("peer_uid", json!(peer_uid)), ("peer_pid", json!(peer_pid))];
        if let Some(n) = name {
            fields.push(("name", json!(n)));
        }
        self.append_event("auth_failed", fields)
    }

    /// Rastro de `forbidden` (spec F.2): metodo, peer e o nome pretendido (so se for nome valido).
    /// Nunca grava assunto, corpo ou token.
    pub fn denied(
        &mut self,
        method: &str,
        peer_uid: u32,
        peer_pid: i32,
        claimed: Option<&str>,
    ) -> Result<(), ApiError> {
        let mut fields = vec![
            ("method", json!(method)),
            ("peer_uid", json!(peer_uid)),
            ("peer_pid", json!(peer_pid)),
        ];
        if let Some(c) = claimed {
            fields.push(("claimed", json!(c)));
        }
        self.append_event("denied", fields)
    }

    pub fn pending_count(&self, name: &str) -> usize {
        self.msgs
            .values()
            .filter(|m| m.to == name && m.state == State::Registrado)
            .count()
    }

    /// Destinatarios com campainha, com pendencia, e sem campainha em aberto dentro de `rearm`.
    pub fn bell_due(&self, rearm: Duration, now: Instant) -> Vec<String> {
        self.recipients
            .iter()
            .filter(|(name, bell)| {
                **bell
                    && self.pending_count(name) > 0
                    && self
                        .outstanding
                        .get(*name)
                        .is_none_or(|t| now.duration_since(*t) >= rearm)
            })
            .map(|(name, _)| name.clone())
            .collect()
    }

    /// Registra a campainha tocada (evento `bell_sent` e estado `outstanding`).
    pub fn bell_sent(&mut self, name: &str, now: Instant) -> Result<(), ApiError> {
        self.append_event("bell_sent", [("recipient", json!(name))])?;
        self.outstanding.insert(name.to_string(), now);
        Ok(())
    }

    /// Registra falha da campainha, no maximo uma vez por minuto por destinatario.
    pub fn bell_failed(&mut self, name: &str, reason: &str, now: Instant) -> Result<(), ApiError> {
        if let Some(t) = self.bell_fail_at.get(name)
            && now.duration_since(*t) < BELL_FAIL_INTERVAL
        {
            return Ok(());
        }
        self.bell_fail_at.insert(name.to_string(), now);
        self.append_event(
            "bell_failed",
            [("recipient", json!(name)), ("reason", json!(reason))],
        )
    }

    pub fn bound(
        &mut self,
        name: &str,
        mux: &str,
        pane: &str,
        peer_pid: i32,
    ) -> Result<(), ApiError> {
        self.append_event(
            "bound",
            [
                ("name", json!(name)),
                ("mux", json!(mux)),
                ("pane", json!(pane)),
                ("peer_pid", json!(peer_pid)),
            ],
        )
    }

    pub fn unbound(
        &mut self,
        name: &str,
        mux: &str,
        pane: &str,
        reason: &str,
    ) -> Result<(), ApiError> {
        self.append_event(
            "unbound",
            [
                ("name", json!(name)),
                ("mux", json!(mux)),
                ("pane", json!(pane)),
                ("reason", json!(reason)),
            ],
        )
    }

    /// `status` agora expõe apenas estatísticas agregadas (spec v1.8 B.0).
    pub fn status_for(&self, uptime_s: u64, connections: usize) -> Value {
        self.status_stats(uptime_s, connections)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    pub(super) fn recips() -> BTreeMap<String, bool> {
        [("a".to_string(), false), ("b".to_string(), false)]
            .into_iter()
            .collect()
    }

    fn params(id: &str) -> SendParams {
        SendParams {
            id: id.into(),
            from: None,
            to: "b".into(),
            kind: "AVISO".into(),
            subject: "oi".into(),
            body: "corpo".into(),
            thread: None,
            refs: Vec::new(),
            keep_open: false,
            due_s: None,
        }
    }

    #[test]
    fn recupera_linha_truncada_na_cauda() {
        let dir = tempfile::tempdir().unwrap();
        {
            let mut st = Store::open(dir.path(), recips()).unwrap();
            st.send("a", &params("m1")).unwrap();
        }
        let log = dir.path().join("log.jsonl");
        let mut f = OpenOptions::new().append(true).open(&log).unwrap();
        f.write_all(b"{\"v\":1,\"seq\":2,\"ty").unwrap();
        drop(f);
        let st = Store::open(dir.path(), recips()).unwrap();
        assert_eq!(st.head(), 1);
        let text = fs::read_to_string(&log).unwrap();
        assert!(text.ends_with('\n'));
        assert_eq!(text.lines().count(), 1);
    }

    #[test]
    fn recusa_linha_invalida_no_meio() {
        let dir = tempfile::tempdir().unwrap();
        {
            let mut st = Store::open(dir.path(), recips()).unwrap();
            st.send("a", &params("m1")).unwrap();
        }
        let log = dir.path().join("log.jsonl");
        let mut f = OpenOptions::new().append(true).open(&log).unwrap();
        f.write_all(b"lixo\n{\"v\":1}\n").unwrap();
        drop(f);
        assert!(Store::open(dir.path(), recips()).is_err());
    }

    #[test]
    fn recusa_lacuna_de_seq() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(
            dir.path().join("log.jsonl"),
            "{\"v\":1,\"seq\":2,\"ts\":\"x\",\"type\":\"bell_sent\",\"recipient\":\"b\"}\n",
        )
        .unwrap();
        assert!(Store::open(dir.path(), recips()).is_err());
    }
}

#[cfg(test)]
mod blob_read_replay_tests {
    use super::*;

    #[test]
    fn replay_restores_readers_and_removal_clears_blob_state() {
        let dir = tempfile::tempdir().unwrap();
        let id = format!("blob:{}", "a".repeat(32));
        {
            let mut store = Store::open(dir.path(), tests::recips()).unwrap();
            store
                .blob_event(
                    "blob_put",
                    vec![
                        ("blob", json!(id)),
                        ("owner", json!("a")),
                        ("to", json!("b")),
                        ("size", json!(3)),
                        ("name", json!("x")),
                        ("sha256", json!("0".repeat(64))),
                        ("created_at", json!(now_ts())),
                        ("ttl_s", json!(3600)),
                    ],
                )
                .unwrap();
            store
                .blob_event(
                    "blob_got",
                    vec![
                        ("blob", json!(id)),
                        ("owner", json!("a")),
                        ("to", json!("b")),
                        ("by", json!("a")),
                        ("offset", json!(0)),
                        ("length", json!(3)),
                    ],
                )
                .unwrap();
        }

        let mut store = Store::open(dir.path(), tests::recips()).unwrap();
        assert!(store.blobs[&id].read_by.contains("a"));
        store
            .blob_event(
                "blob_removed",
                vec![
                    ("blob", json!(id)),
                    ("owner", json!("a")),
                    ("to", json!("b")),
                    ("name", json!("x")),
                    ("reason", json!("requested")),
                ],
            )
            .unwrap();
        drop(store);

        let store = Store::open(dir.path(), tests::recips()).unwrap();
        assert!(!store.blobs.contains_key(&id));
    }
}

//! Log append-only (`log.jsonl`), dobra dos eventos, recuperacao de partida e trava (spec v1 6 e v1.1).

use std::collections::{BTreeMap, HashMap};
use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::os::fd::AsRawFd;
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::Path;
use std::time::{Duration, Instant};

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
    })
}

pub fn now_ts() -> String {
    chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true)
}

fn internal(msg: String) -> ApiError {
    ApiError::new("internal_error", msg)
}

fn str_field<'a>(obj: &'a Map<String, Value>, key: &str) -> Result<&'a str, String> {
    obj.get(key)
        .and_then(Value::as_str)
        .ok_or_else(|| format!("campo '{key}' ausente ou invalido"))
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
                if note.is_some() {
                    m.note = note;
                }
                let to = m.to.clone();
                self.outstanding.remove(&to);
            }
            "auth_failed" => {}
            "denied" => {
                str_field(obj, "method")?;
            }
            "bell_sent" => {
                let r = str_field(obj, "recipient")?.to_string();
                self.last_bell_seq.insert(r, seq);
            }
            "bell_failed" => {
                str_field(obj, "recipient")?;
                str_field(obj, "reason")?;
            }
            other => return Err(format!("tipo de evento desconhecido '{other}'")),
        }
        self.events.push(ev.clone());
        Ok(())
    }

    /// Grava um evento: aplica, escreve a linha e faz `sync_data` antes de retornar.
    fn append(&mut self, mut ev: Map<String, Value>) -> Result<(), ApiError> {
        if self.broken {
            return Err(internal(
                "log indisponivel apos falha de escrita".to_string(),
            ));
        }
        ev.insert("v".into(), json!(1));
        ev.insert("seq".into(), json!(self.events.len() as u64 + 1));
        let value = Value::Object(ev);
        self.apply(&value).map_err(internal)?;
        let mut line = serde_json::to_vec(&value).map_err(|e| internal(e.to_string()))?;
        line.push(b'\n');
        if self
            .log
            .write_all(&line)
            .and_then(|()| self.log.sync_data())
            .is_err()
        {
            self.broken = true;
            return Err(internal("falha ao gravar o log".to_string()));
        }
        Ok(())
    }

    pub fn head(&self) -> u64 {
        self.events.len() as u64
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
            Some("accepted") | Some("resulted") => {
                let id = obj.get("id").and_then(Value::as_str).unwrap_or("");
                self.msgs
                    .get(id)
                    .is_some_and(|m| m.from == who || m.to == who)
            }
            Some("bell_sent") | Some("bell_failed") => {
                obj.get("recipient").and_then(Value::as_str) == Some(who)
            }
            _ => false,
        }
    }

    /// `message.send` com idempotencia por id. `from` e a identidade ja resolvida.
    /// Devolve (mensagem, duplicate).
    pub fn send(&mut self, from: &str, p: &SendParams) -> Result<(Msg, bool), ApiError> {
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
                return Ok((m.clone(), true));
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
        self.append(ev)?;
        let m = self
            .msgs
            .get(&p.id)
            .cloned()
            .ok_or_else(|| internal("mensagem nao encontrada apos gravar".to_string()))?;
        Ok((m, false))
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
            return Err(ApiError::new(
                "forbidden",
                "so o destinatario confirma a mensagem",
            ));
        }
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

    /// Falha de autenticacao (v1.1 B): nunca grava o token.
    pub fn auth_failed(
        &mut self,
        name: Option<&str>,
        peer_uid: u32,
        peer_pid: i32,
    ) -> Result<(), ApiError> {
        let mut ev = Map::new();
        ev.insert("type".into(), json!("auth_failed"));
        ev.insert("ts".into(), json!(now_ts()));
        if let Some(n) = name {
            ev.insert("name".into(), json!(n));
        }
        ev.insert("peer_uid".into(), json!(peer_uid));
        ev.insert("peer_pid".into(), json!(peer_pid));
        self.append(ev)
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
        let mut ev = Map::new();
        ev.insert("type".into(), json!("denied"));
        ev.insert("ts".into(), json!(now_ts()));
        ev.insert("method".into(), json!(method));
        ev.insert("peer_uid".into(), json!(peer_uid));
        ev.insert("peer_pid".into(), json!(peer_pid));
        if let Some(c) = claimed {
            ev.insert("claimed".into(), json!(c));
        }
        self.append(ev)
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
        let mut ev = Map::new();
        ev.insert("type".into(), json!("bell_sent"));
        ev.insert("ts".into(), json!(now_ts()));
        ev.insert("recipient".into(), json!(name));
        self.append(ev)?;
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
        let mut ev = Map::new();
        ev.insert("type".into(), json!("bell_failed"));
        ev.insert("ts".into(), json!(now_ts()));
        ev.insert("recipient".into(), json!(name));
        ev.insert("reason".into(), json!(reason));
        self.append(ev)
    }

    /// `status` (spec v1 5.5). Com `who`, so a entrada da identidade (v1.1 B).
    pub fn status_for(&self, who: Option<&str>) -> Value {
        let mut rec = Map::new();
        for (name, has_bell) in &self.recipients {
            if who.is_some_and(|w| w != name) {
                continue;
            }
            let mut pending = 0u64;
            let mut accepted = 0u64;
            for m in self.msgs.values().filter(|m| &m.to == name) {
                match m.state {
                    State::Registrado => pending += 1,
                    State::Aceito => accepted += 1,
                    State::Resultado => {}
                }
            }
            let bell = if !has_bell {
                "none"
            } else if self.outstanding.contains_key(name) {
                "outstanding"
            } else {
                "idle_wait"
            };
            rec.insert(
                name.clone(),
                json!({
                    "pending": pending,
                    "accepted": accepted,
                    "bell": bell,
                    "last_bell_seq": self.last_bell_seq.get(name).copied(),
                }),
            );
        }
        json!({"v": 1, "head_seq": self.head(), "recipients": rec})
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn recips() -> BTreeMap<String, bool> {
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

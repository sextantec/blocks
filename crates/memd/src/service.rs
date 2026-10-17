use std::collections::{BTreeMap, HashMap, HashSet};

use blocklog::{Clock, Commit, CommitError, Log, Req};
use chrono::{DateTime, Duration, NaiveDate, Utc};
use serde::Deserialize;
use serde_json::{Map, Value, json};

use crate::{
    config::Config,
    grants::{Grant, Grants, valid_agent, valid_scope},
    model::{Record, Reference, Source, State},
};

pub struct Service {
    cfg: Config,
    grants: Grants,
    log: Log,
    state: State,
    clock: Box<dyn Clock>,
    effective_now: DateTime<Utc>,
    started: DateTime<Utc>,
    requests: HashMap<(String, String), (String, Value, DateTime<Utc>)>,
    projection_broken: bool,
}

#[derive(Debug)]
struct ApiError {
    code: &'static str,
    message: &'static str,
}

type ApiResult = Result<Value, ApiError>;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Request {
    v: u8,
    id: String,
    token: String,
    method: String,
    params: Map<String, Value>,
}

impl Service {
    pub fn new(
        cfg: Config,
        grants: Grants,
        log: Log,
        replay: Vec<Commit>,
        clock: Box<dyn Clock>,
    ) -> Result<Service, String> {
        cfg.validate()?;
        let started = clock.now();
        let mut service = Service {
            cfg,
            grants,
            log,
            state: State::default(),
            clock,
            effective_now: started,
            started,
            requests: HashMap::new(),
            projection_broken: false,
        };
        for commit in replay {
            let commit_ts = DateTime::parse_from_rfc3339(&commit.ts)
                .map_err(|_| "ts de commit invalido".to_string())?
                .with_timezone(&Utc);
            if commit_ts > service.effective_now {
                service.effective_now = commit_ts;
            }
            for event in &commit.tx {
                service.apply_event(event, commit.seq, &commit.ts)?;
            }
            if let (Some(req), Some(res)) = (commit.req, commit.res) {
                let request_ts = DateTime::parse_from_rfc3339(&commit.ts)
                    .map_err(|_| "ts de request invalido".to_string())?
                    .with_timezone(&Utc);
                service
                    .requests
                    .insert((req.agent, req.id), (req.h, res, request_ts));
            }
        }
        service.validate_state()?;
        Ok(service)
    }

    pub fn handle_line(&mut self, line: &str) -> String {
        self.refresh_clock();
        if line.len() > self.cfg.line_max {
            return encode_response(Value::Null, Err(invalid()));
        }
        let request: Request = match serde_json::from_str(line) {
            Ok(value) => value,
            Err(_) => return encode_response(Value::Null, Err(invalid())),
        };
        let id = Value::String(request.id.clone());
        if request.v != 1 || request.id.is_empty() || request.id.len() > 128 {
            return encode_response(id, Err(invalid()));
        }
        let today = self.effective_now.date_naive();
        let Some(grant) = self.grants.authenticate(&request.token, today).cloned() else {
            return encode_response(id, Err(err("unauthorized", "nao autorizado")));
        };
        let result = self.dispatch(&grant, &request.method, request.params);
        let response = encode_response(id.clone(), result);
        if response.len() > self.cfg.response_max {
            encode_response(id, Err(err("overloaded", "resposta excede o limite")))
        } else {
            response
        }
    }

    pub fn tick(&mut self) {
        if self.is_broken() {
            return;
        }
        self.refresh_clock();
        let now = self.effective_now;
        let ttl = Duration::days(i64::from(self.cfg.proposal_ttl_days));
        let mut expired = Vec::new();
        for records in self.state.records.values() {
            for record in records {
                if record.status != "proposed" {
                    continue;
                }
                let Ok(at) = DateTime::parse_from_rfc3339(&record.proposed_at) else {
                    continue;
                };
                if now.signed_duration_since(at.with_timezone(&Utc)) >= ttl {
                    expired.push((record.id.clone(), record.rev));
                }
            }
        }
        for chunk in expired.chunks(16) {
            let tx: Vec<Value> = chunk
                .iter()
                .map(|(id, rev)| json!({"e":"expire","id":id,"rev":rev,"by":"memd"}))
                .collect();
            let Ok(committed) = self.log.commit(tx.clone(), None, None) else {
                return;
            };
            self.refresh_clock();
            let ts = self
                .effective_now
                .to_rfc3339_opts(chrono::SecondsFormat::Millis, true);
            let previous = self.state.clone();
            for event in &tx {
                if self.apply_event(event, committed.seq, &ts).is_err() {
                    self.state = previous;
                    self.projection_broken = true;
                    return;
                }
            }
        }
    }

    pub fn reload_grants(&mut self, grants: Grants) {
        self.grants = grants;
    }
    pub fn head_seq(&self) -> u64 {
        self.log.head_seq()
    }
    pub fn is_broken(&self) -> bool {
        self.projection_broken || self.log.is_broken()
    }

    fn refresh_clock(&mut self) {
        let now = self.clock.now();
        if now > self.effective_now {
            self.effective_now = now;
        }
        if let Ok(normalized) = DateTime::parse_from_rfc3339(
            &self
                .effective_now
                .to_rfc3339_opts(chrono::SecondsFormat::Millis, true),
        ) {
            self.effective_now = normalized.with_timezone(&Utc);
        }
    }

    fn dispatch(&mut self, grant: &Grant, method: &str, params: Map<String, Value>) -> ApiResult {
        if matches!(method, "propose" | "promote" | "reject" | "retire") && self.is_broken() {
            return Err(err("broken", "log indisponivel"));
        }
        match method {
            "propose" => self.propose(grant, params),
            "promote" => self.promote(grant, params),
            "reject" => self.reject(grant, params),
            "retire" => self.retire(grant, params),
            "get" => self.get(grant, params),
            "query" => self.query(grant, params),
            "view" => self.view(grant, params),
            "whoami" => self.whoami(grant, params),
            "status" => self.status(params),
            _ => Err(invalid()),
        }
    }

    fn propose(&mut self, grant: &Grant, params: Map<String, Value>) -> ApiResult {
        let request_hash = canonical_params_hash("propose", &params);
        let source_missing = !params.contains_key("source");
        let p: ProposeParams = parse_params(params)?;
        valid_request_id(&p.request_id)?;
        if let Some(result) = self.prior_request(grant, &p.request_id, &request_hash)? {
            return Ok(result);
        }
        if source_missing {
            return Err(err("missing_source", "source obrigatorio"));
        }
        let source = p
            .source
            .as_ref()
            .ok_or_else(|| err("missing_source", "source obrigatorio"))?;
        let kind = match (&p.id, p.kind.as_deref()) {
            (None, Some(kind)) => kind.to_string(),
            (None, None) => return Err(invalid()),
            (Some(id), Some(kind)) => {
                let latest = self.state.latest(id).ok_or_else(not_found)?;
                if !grant.can_read(&latest.kind, &latest.scope) {
                    return Err(not_found());
                }
                if latest.kind != kind {
                    return Err(invalid());
                }
                kind.to_string()
            }
            (Some(id), None) => {
                let latest = self.state.latest(id).ok_or_else(not_found)?;
                if !grant.can_read(&latest.kind, &latest.scope) {
                    return Err(not_found());
                }
                latest.kind.clone()
            }
        };
        validate_proposal(&self.cfg, &p, &kind, self.effective_now.date_naive())?;
        if !grant.can_write("propose", &kind, &p.scope) {
            return Err(err("forbidden", "operacao proibida"));
        }
        let open = self
            .state
            .records
            .values()
            .flatten()
            .filter(|r| r.author == grant.agent && r.status == "proposed")
            .count();
        if open >= self.cfg.max_open_proposals || self.state.record_count() >= self.cfg.max_records
        {
            return Err(err("quota", "cota excedida"));
        }
        let (id, rev) = if let Some(id) = &p.id {
            let latest = self.state.latest(id).ok_or_else(not_found)?;
            let active_rev = self.state.active(id).map_or(0, |r| r.rev);
            if p.expected_rev != Some(active_rev) {
                return Err(err("stale_rev", "revisao desatualizada"));
            }
            (
                id.clone(),
                latest
                    .rev
                    .checked_add(1)
                    .ok_or_else(|| err("validation", "revisao esgotada"))?,
            )
        } else {
            if p.expected_rev.is_some() {
                return Err(invalid());
            }
            let next = if kind == "decision" {
                self.state.next_decision.checked_add(1)
            } else {
                self.state.next_pending.checked_add(1)
            }
            .ok_or_else(|| err("validation", "ids esgotados"))?;
            let prefix = if kind == "decision" { 'D' } else { 'P' };
            (format!("{prefix}-{next:03}"), 1)
        };
        let event = json!({
            "e":"propose", "id":id, "rev":rev, "type":kind, "title":p.title,
            "body":p.body, "scope":p.scope, "tags":p.tags, "fields":p.fields,
            "source":source, "refs":p.refs, "supersedes":p.supersedes,
            "review_by":p.review_by, "author":grant.agent
        });
        let seq = self
            .log
            .head_seq()
            .checked_add(1)
            .ok_or_else(|| err("broken", "log indisponivel"))?;
        let result = json!({"id":id,"rev":rev,"status":"proposed","seq":seq});
        self.commit_mutation(grant, &p.request_id, request_hash, &event, result)
    }

    fn promote(&mut self, grant: &Grant, params: Map<String, Value>) -> ApiResult {
        let request_hash = canonical_params_hash("promote", &params);
        let p: PromoteParams = parse_params(params)?;
        valid_request_id(&p.request_id)?;
        if let Some(result) = self.prior_request(grant, &p.request_id, &request_hash)? {
            return Ok(result);
        }
        let record = self.state.get(&p.id, p.rev).ok_or_else(not_found)?.clone();
        if !grant.can_read(&record.kind, &record.scope) {
            return Err(not_found());
        }
        if !grant.can_write("promote", &record.kind, &record.scope) {
            return Err(err("forbidden", "operacao proibida"));
        }
        if record.status != "proposed" {
            return Err(err("conflict", "estado da versao incompativel"));
        }
        if p.expected_seq != record.proposed_seq {
            return Err(err("stale_seq", "sequencia desatualizada"));
        }
        for id in &record.supersedes {
            let target = self.state.active(id).ok_or_else(not_found)?;
            if !grant.can_read(&target.kind, &target.scope) {
                return Err(not_found());
            }
            if !grant.can_write("promote", &target.kind, &target.scope) {
                return Err(not_found());
            }
        }
        let normalized = normalized_title(&record.title);
        let conflict = self
            .state
            .records
            .values()
            .filter_map(|v| v.iter().find(|r| r.status == "active"))
            .any(|other| {
                other.id != record.id
                    && other.kind == record.kind
                    && other.scope == record.scope
                    && normalized_title(&other.title) == normalized
                    && !record.supersedes.contains(&other.id)
            });
        if conflict {
            return Err(err("conflict", "registro ativo conflitante"));
        }
        let mut superseded = record.supersedes.clone();
        if self.state.active(&record.id).is_some() {
            superseded.push(record.id.clone());
        }
        superseded.sort();
        superseded.dedup();
        let event = json!({"e":"promote","id":p.id,"rev":p.rev,"by":grant.agent});
        let seq = self
            .log
            .head_seq()
            .checked_add(1)
            .ok_or_else(|| err("broken", "log indisponivel"))?;
        let result = json!({"id":record.id,"rev":record.rev,"status":"active","superseded":superseded,"seq":seq});
        self.commit_mutation(grant, &p.request_id, request_hash, &event, result)
    }

    fn reject(&mut self, grant: &Grant, params: Map<String, Value>) -> ApiResult {
        let request_hash = canonical_params_hash("reject", &params);
        let p: StateParams = parse_params(params)?;
        self.change_state(grant, p, request_hash, "reject", "proposed", "rejected")
    }

    fn retire(&mut self, grant: &Grant, params: Map<String, Value>) -> ApiResult {
        let request_hash = canonical_params_hash("retire", &params);
        let p: StateParams = parse_params(params)?;
        self.change_state(grant, p, request_hash, "retire", "active", "retired")
    }

    fn change_state(
        &mut self,
        grant: &Grant,
        p: StateParams,
        request_hash: String,
        action: &'static str,
        expected: &str,
        status: &str,
    ) -> ApiResult {
        valid_request_id(&p.request_id)?;
        if let Some(result) = self.prior_request(grant, &p.request_id, &request_hash)? {
            return Ok(result);
        }
        if p.reason.as_deref().is_some_and(|v| v.chars().count() > 200) {
            return Err(invalid());
        }
        if p.reason
            .as_deref()
            .is_some_and(|v| contains_secret(v, &self.cfg.secret_patterns))
        {
            return Err(err("secret_detected", "possivel segredo detectado"));
        }
        let record = self.state.get(&p.id, p.rev).ok_or_else(not_found)?.clone();
        if !grant.can_read(&record.kind, &record.scope) {
            return Err(not_found());
        }
        if !grant.can_write("promote", &record.kind, &record.scope) {
            return Err(err("forbidden", "operacao proibida"));
        }
        if record.status != expected {
            return Err(err("conflict", "estado da versao incompativel"));
        }
        let event = json!({"e":action,"id":p.id,"rev":p.rev,"by":grant.agent,"reason":p.reason});
        let seq = self
            .log
            .head_seq()
            .checked_add(1)
            .ok_or_else(|| err("broken", "log indisponivel"))?;
        let result = json!({"id":record.id,"rev":record.rev,"status":status,"seq":seq});
        self.commit_mutation(grant, &p.request_id, request_hash, &event, result)
    }

    fn get(&self, grant: &Grant, params: Map<String, Value>) -> ApiResult {
        let p: GetParams = parse_params(params)?;
        let record = self.state.selected(&p.id, p.rev).ok_or_else(not_found)?;
        if !grant.can_read(&record.kind, &record.scope) {
            return Err(not_found());
        }
        let mut value = self.record_value(record, grant);
        let object = value
            .as_object_mut()
            .ok_or_else(|| err("broken", "estado invalido"))?;
        object.insert("head_seq".into(), json!(self.log.head_seq()));
        Ok(value)
    }

    fn query(&self, grant: &Grant, params: Map<String, Value>) -> ApiResult {
        let p: QueryParams = parse_params(params)?;
        let limit = p.limit.unwrap_or(20);
        if !(1..=50).contains(&limit) {
            return Err(invalid());
        }
        if p.kind
            .as_deref()
            .is_some_and(|v| v != "decision" && v != "pending")
        {
            return Err(invalid());
        }
        let status = p.status.as_deref().unwrap_or("active");
        if ![
            "proposed",
            "active",
            "superseded",
            "retired",
            "rejected",
            "expired",
        ]
        .contains(&status)
        {
            return Err(invalid());
        }
        if p.scope.as_deref().is_some_and(|v| !valid_scope(v)) {
            return Err(invalid());
        }
        if p.state
            .as_deref()
            .is_some_and(|v| !["open", "blocked", "done"].contains(&v))
        {
            return Err(invalid());
        }
        let explicit_done = p.state.as_deref() == Some("done");
        let term = p.term.as_deref().map(ascii_fold);
        let mut matches: Vec<&Record> = self
            .state
            .records
            .values()
            .flatten()
            .filter(|r| {
                grant.can_read(&r.kind, &r.scope)
                    && r.status == status
                    && p.kind.as_deref().is_none_or(|v| v == r.kind)
                    && p.scope.as_deref().is_none_or(|v| v == r.scope)
                    && p.tag
                        .as_deref()
                        .is_none_or(|v| r.tags.iter().any(|tag| tag == v))
                    && p.author.as_deref().is_none_or(|v| v == r.author)
                    && p.state
                        .as_deref()
                        .is_none_or(|v| field_string(&r.fields, "state") == Some(v))
                    && (explicit_done || field_string(&r.fields, "state") != Some("done"))
                    && term.as_ref().is_none_or(|needle| {
                        ascii_fold(&r.title).contains(needle)
                            || r.tags.iter().any(|tag| ascii_fold(tag).contains(needle))
                    })
                    && p.after
                        .as_deref()
                        .is_none_or(|after| compare_record_id(&r.id, after).is_gt())
            })
            .collect();
        matches.sort_by(|a, b| record_order(a, b));
        let truncated = matches.len() > limit;
        matches.truncate(limit);
        let items: Vec<Value> = matches
            .iter()
            .map(|r| {
                json!({
                    "id":r.id,"rev":r.rev,"type":r.kind,"title":r.title,"status":r.status,
                    "scope":r.scope,"review_by":r.review_by,"unconfirmed":self.unconfirmed(r)
                })
            })
            .collect();
        let next_after = if truncated {
            matches
                .last()
                .map(|r| Value::String(r.id.clone()))
                .unwrap_or(Value::Null)
        } else {
            Value::Null
        };
        Ok(json!({"items":items,"next_after":next_after,"head_seq":self.log.head_seq()}))
    }

    fn view(&self, grant: &Grant, params: Map<String, Value>) -> ApiResult {
        let p: ViewParams = parse_params(params)?;
        if p.name.as_deref().is_some_and(|name| name != "default") {
            return Err(invalid());
        }
        let mut due: Vec<&Record> = self
            .state
            .records
            .values()
            .flatten()
            .filter(|r| {
                r.status == "active"
                    && self.unconfirmed(r)
                    && grant.can_read(&r.kind, &r.scope)
                    && field_string(&r.fields, "state") != Some("done")
            })
            .collect();
        due.sort_by(|a, b| {
            a.review_by
                .cmp(&b.review_by)
                .then_with(|| record_order(a, b))
        });
        due.truncate(self.cfg.view_overdue_max);
        let due_keys: HashSet<(&str, u64)> = due.iter().map(|r| (r.id.as_str(), r.rev)).collect();
        let mut active: Vec<&Record> = self
            .state
            .records
            .values()
            .flatten()
            .filter(|r| {
                r.status == "active"
                    && grant.can_read(&r.kind, &r.scope)
                    && field_string(&r.fields, "state") != Some("done")
                    && !due_keys.contains(&(r.id.as_str(), r.rev))
            })
            .collect();
        active.sort_by(|a, b| record_order(a, b));

        let mut text = format!("intent: {}\n", grant.intent);
        let mut delivered = Vec::new();
        let mut truncated = false;
        let mut next_after = Value::Null;
        for (heading, records) in [("due", due), ("active", active)] {
            let header = format!("{heading}:\n");
            if text.chars().count() + header.chars().count() > self.cfg.view_budget {
                truncated = true;
                break;
            }
            text.push_str(&header);
            for record in records {
                let review = record.review_by.as_deref().unwrap_or("-");
                let line = format!(
                    "{} | {} | {} | {}\n",
                    record.id, record.title, record.scope, review
                );
                if text.chars().count() + line.chars().count() > self.cfg.view_budget {
                    truncated = true;
                    next_after = delivered
                        .last()
                        .map(|id: &String| Value::String(id.clone()))
                        .unwrap_or(Value::Null);
                    break;
                }
                text.push_str(&line);
                delivered.push(record.id.clone());
            }
            if truncated {
                break;
            }
        }
        Ok(
            json!({"text":text,"trust":"data","view_truncated":truncated,"next_after":next_after,"head_seq":self.log.head_seq()}),
        )
    }

    fn whoami(&self, grant: &Grant, params: Map<String, Value>) -> ApiResult {
        if !params.is_empty() {
            return Err(invalid());
        }
        Ok(
            json!({"agent":grant.agent,"ops":grant.ops,"read_scopes":grant.read_scopes,
            "write_scopes":grant.write_scopes,"types":grant.types,"expires":grant.expires,"intent":grant.intent}),
        )
    }

    fn status(&self, params: Map<String, Value>) -> ApiResult {
        if !params.is_empty() {
            return Err(invalid());
        }
        let mut counts = BTreeMap::<String, u64>::new();
        for status in [
            "proposed",
            "active",
            "superseded",
            "retired",
            "rejected",
            "expired",
        ] {
            counts.insert(status.to_string(), 0);
        }
        for record in self.state.records.values().flatten() {
            *counts.entry(record.status.clone()).or_default() += 1;
        }
        let uptime = self
            .clock
            .now()
            .signed_duration_since(self.started)
            .num_seconds()
            .max(0);
        Ok(
            json!({"head_seq":self.log.head_seq(),"counts":counts,"broken":self.is_broken(),"uptime_s":uptime}),
        )
    }

    fn record_value(&self, record: &Record, grant: &Grant) -> Value {
        let mut value = serde_json::to_value(record).unwrap_or_else(|_| json!({}));
        let Some(object) = value.as_object_mut() else {
            return json!({});
        };
        object.remove("proposed_at");
        object.insert("unconfirmed".into(), Value::Bool(self.unconfirmed(record)));
        if let Some(Value::String(id_rev)) = object.get_mut("superseded_by") {
            let id = id_rev.split('@').next().unwrap_or("");
            if !self.id_readable(id, grant) {
                *id_rev = "(oculto)".into();
            }
        }
        if let Some(Value::Array(ids)) = object.get_mut("supersedes") {
            for id in ids {
                if let Some(text) = id.as_str().map(str::to_owned)
                    && !self.id_readable(&text, grant)
                {
                    *id = Value::String("(oculto)".into());
                }
            }
        }
        value
    }

    fn id_readable(&self, id: &str, grant: &Grant) -> bool {
        self.state
            .latest(id)
            .is_some_and(|r| grant.can_read(&r.kind, &r.scope))
    }

    fn unconfirmed(&self, record: &Record) -> bool {
        record.status == "active"
            && record
                .review_by
                .as_deref()
                .and_then(|d| NaiveDate::parse_from_str(d, "%Y-%m-%d").ok())
                .is_some_and(|d| d < self.effective_now.date_naive())
    }

    fn commit_mutation(
        &mut self,
        grant: &Grant,
        request_id: &str,
        hash: String,
        event: &Value,
        result: Value,
    ) -> ApiResult {
        if self.is_broken() {
            return Err(err("broken", "log indisponivel"));
        }
        let req = Req {
            agent: grant.agent.clone(),
            id: request_id.to_string(),
            h: hash.clone(),
        };
        let committed = self
            .log
            .commit(vec![event.clone()], Some(req), Some(result.clone()))
            .map_err(map_commit_error)?;
        if committed.replayed {
            let mut original = committed
                .res
                .ok_or_else(|| err("broken", "resultado ausente"))?;
            self.authorize_saved_result(grant, &original)?;
            if let Some(object) = original.as_object_mut() {
                object.insert("replayed".into(), Value::Bool(true));
            }
            return Ok(original);
        }
        self.refresh_clock();
        let ts = self
            .effective_now
            .to_rfc3339_opts(chrono::SecondsFormat::Millis, true);
        let previous = self.state.clone();
        if self.apply_event(event, committed.seq, &ts).is_err() {
            self.state = previous;
            self.projection_broken = true;
            return Err(err("broken", "estado invalido"));
        }
        self.requests.insert(
            (grant.agent.clone(), request_id.to_string()),
            (hash, result.clone(), self.effective_now),
        );
        Ok(result)
    }

    fn prior_request(
        &self,
        grant: &Grant,
        request_id: &str,
        hash: &str,
    ) -> Result<Option<Value>, ApiError> {
        let Some((old_hash, old_result, accepted_at)) = self
            .requests
            .get(&(grant.agent.clone(), request_id.to_string()))
        else {
            return Ok(None);
        };
        if accepted_at
            .checked_add_signed(Duration::days(30))
            .is_some_and(|expiry| expiry <= self.effective_now)
        {
            return Ok(None);
        }
        if old_hash != hash {
            return Err(err("request_conflict", "request_id reutilizado"));
        }
        self.authorize_saved_result(grant, old_result)?;
        let mut result = old_result.clone();
        if let Some(object) = result.as_object_mut() {
            object.insert("replayed".into(), Value::Bool(true));
        }
        Ok(Some(result))
    }

    fn authorize_saved_result(&self, grant: &Grant, result: &Value) -> Result<(), ApiError> {
        let id = result
            .get("id")
            .and_then(Value::as_str)
            .ok_or_else(|| err("broken", "resultado idempotente invalido"))?;
        let rev = result
            .get("rev")
            .and_then(Value::as_u64)
            .ok_or_else(|| err("broken", "resultado idempotente invalido"))?;
        let record = self.state.get(id, rev).ok_or_else(not_found)?;
        if grant.can_read(&record.kind, &record.scope) {
            Ok(())
        } else {
            Err(not_found())
        }
    }

    fn apply_event(&mut self, event: &Value, seq: u64, ts: &str) -> Result<(), String> {
        let object = event
            .as_object()
            .ok_or_else(|| "evento nao e objeto".to_string())?;
        let action = object
            .get("e")
            .and_then(Value::as_str)
            .ok_or_else(|| "evento sem e".to_string())?;
        match action {
            "propose" => self.apply_propose(event, seq, ts),
            "promote" => self.apply_promote(object, seq, ts),
            "reject" => self.apply_status(object, "proposed", "rejected", seq, ts),
            "retire" => self.apply_status(object, "active", "retired", seq, ts),
            "expire" => self.apply_status(object, "proposed", "expired", seq, ts),
            _ => Err("evento desconhecido".into()),
        }
    }

    fn apply_propose(&mut self, event: &Value, seq: u64, ts: &str) -> Result<(), String> {
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct Event {
            e: String,
            id: String,
            rev: u64,
            #[serde(rename = "type")]
            kind: String,
            title: String,
            body: String,
            scope: String,
            tags: Vec<String>,
            fields: Value,
            source: Source,
            refs: Vec<Reference>,
            supersedes: Vec<String>,
            review_by: Option<String>,
            author: String,
        }
        let e: Event =
            serde_json::from_value(event.clone()).map_err(|v| format!("propose invalido: {v}"))?;
        if e.e != "propose" || self.state.get(&e.id, e.rev).is_some() {
            return Err("propose duplicado".into());
        }
        let event_date = DateTime::parse_from_rfc3339(ts)
            .map_err(|_| "ts de propose invalido".to_string())?
            .date_naive();
        let validation = ProposeParams {
            kind: Some(e.kind.clone()),
            id: if e.rev == 1 { None } else { Some(e.id.clone()) },
            expected_rev: None,
            title: e.title.clone(),
            body: e.body.clone(),
            scope: e.scope.clone(),
            tags: e.tags.clone(),
            fields: e.fields.clone(),
            source: Some(e.source.clone()),
            refs: e.refs.clone(),
            supersedes: e.supersedes.clone(),
            review_by: e.review_by.clone(),
            request_id: "replay".into(),
        };
        validate_proposal(&self.cfg, &validation, &e.kind, event_date)
            .map_err(|_| "propose viola validacao".to_string())?;
        if !valid_agent(&e.author) {
            return Err("author invalido".into());
        }
        let expected = if e.kind == "decision" {
            self.state.next_decision.checked_add(1)
        } else if e.kind == "pending" {
            self.state.next_pending.checked_add(1)
        } else {
            return Err("type invalido".into());
        }
        .ok_or_else(|| "ids esgotados".to_string())?;
        if e.rev == 1 {
            let correct_prefix = (e.kind == "decision" && e.id.starts_with("D-"))
                || (e.kind == "pending" && e.id.starts_with("P-"));
            if !correct_prefix {
                return Err("prefixo de id invalido".into());
            }
            if parse_id_number(&e.id) != Some(expected) {
                return Err("lacuna de id".into());
            }
            if e.kind == "decision" {
                self.state.next_decision = expected;
            } else {
                self.state.next_pending = expected;
            }
        } else if self.state.latest(&e.id).and_then(|r| r.rev.checked_add(1)) != Some(e.rev) {
            return Err("lacuna de revisao".into());
        }
        let record = Record {
            id: e.id.clone(),
            rev: e.rev,
            kind: e.kind,
            title: e.title,
            body: e.body,
            scope: e.scope,
            tags: e.tags,
            fields: e.fields,
            source: e.source,
            refs: e.refs,
            supersedes: e.supersedes,
            status: "proposed".into(),
            author: e.author,
            seq,
            ts: ts.into(),
            promoted_by: None,
            proposed_seq: seq,
            promoted_seq: None,
            superseded_by: None,
            review_by: e.review_by,
            proposed_at: ts.into(),
        };
        self.state.records.entry(e.id).or_default().push(record);
        Ok(())
    }

    fn apply_promote(
        &mut self,
        object: &Map<String, Value>,
        seq: u64,
        ts: &str,
    ) -> Result<(), String> {
        exact_keys(object, &["e", "id", "rev", "by"])?;
        let id = string_field(object, "id")?;
        let rev = u64_field(object, "rev")?;
        let by = string_field(object, "by")?;
        let target = self
            .state
            .get(id, rev)
            .ok_or("promote sem proposta")?
            .clone();
        if target.status != "proposed" {
            return Err("promote de estado invalido".into());
        }
        let marker = format!("{id}@{rev}");
        if let Some(records) = self.state.records.get_mut(id) {
            for record in records.iter_mut().filter(|r| r.status == "active") {
                record.status = "superseded".into();
                record.superseded_by = Some(marker.clone());
                record.seq = seq;
                record.ts = ts.into();
            }
        }
        for other_id in &target.supersedes {
            let Some(other) = self
                .state
                .records
                .get_mut(other_id)
                .and_then(|v| v.iter_mut().find(|r| r.status == "active"))
            else {
                return Err("supersedes sem active".into());
            };
            other.status = "superseded".into();
            other.superseded_by = Some(marker.clone());
            other.seq = seq;
            other.ts = ts.into();
        }
        let record = self
            .state
            .get_mut(id, rev)
            .ok_or("promote perdeu proposta")?;
        record.status = "active".into();
        record.seq = seq;
        record.ts = ts.into();
        record.promoted_by = Some(by.into());
        record.promoted_seq = Some(seq);
        Ok(())
    }

    fn apply_status(
        &mut self,
        object: &Map<String, Value>,
        expected: &str,
        status: &str,
        seq: u64,
        ts: &str,
    ) -> Result<(), String> {
        let allowed = if object.get("e").and_then(Value::as_str) == Some("expire") {
            vec!["e", "id", "rev", "by"]
        } else {
            vec!["e", "id", "rev", "by", "reason"]
        };
        exact_keys(object, &allowed)?;
        let id = string_field(object, "id")?;
        let rev = u64_field(object, "rev")?;
        let record = self.state.get_mut(id, rev).ok_or("mudanca sem registro")?;
        if record.status != expected {
            return Err("estado anterior invalido".into());
        }
        record.status = status.into();
        record.seq = seq;
        record.ts = ts.into();
        Ok(())
    }

    fn validate_state(&self) -> Result<(), String> {
        for records in self.state.records.values() {
            if records.iter().filter(|r| r.status == "active").count() > 1 {
                return Err("mais de uma revisao active".into());
            }
            for record in records {
                for id in &record.supersedes {
                    if !valid_record_id(id) || self.state.latest(id).is_none() {
                        return Err("supersedes aponta para id inexistente".into());
                    }
                }
                if (record.status == "superseded") != record.superseded_by.is_some() {
                    return Err("superseded inconsistente".into());
                }
                if let Some(marker) = &record.superseded_by {
                    let (id, rev) = parse_record_marker(marker)?;
                    let replacement = self
                        .state
                        .get(id, rev)
                        .ok_or_else(|| "superseded_by quebrado".to_string())?;
                    if replacement.promoted_seq.is_none() {
                        return Err("superseded_by aponta para versao nao promovida".into());
                    }
                }
                if record.promoted_seq.is_some() {
                    let marker = format!("{}@{}", record.id, record.rev);
                    for id in &record.supersedes {
                        let consistent = self.state.records.get(id).is_some_and(|versions| {
                            versions
                                .iter()
                                .any(|target| target.superseded_by.as_deref() == Some(&marker))
                        });
                        if !consistent {
                            return Err("supersedes quebrado".into());
                        }
                    }
                }
            }
        }
        Ok(())
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ProposeParams {
    #[serde(rename = "type")]
    kind: Option<String>,
    id: Option<String>,
    expected_rev: Option<u64>,
    title: String,
    body: String,
    scope: String,
    #[serde(default)]
    tags: Vec<String>,
    fields: Value,
    source: Option<Source>,
    #[serde(default)]
    refs: Vec<Reference>,
    #[serde(default)]
    supersedes: Vec<String>,
    review_by: Option<String>,
    request_id: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct PromoteParams {
    id: String,
    rev: u64,
    expected_seq: u64,
    request_id: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct StateParams {
    id: String,
    rev: u64,
    reason: Option<String>,
    request_id: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct GetParams {
    id: String,
    rev: Option<u64>,
}

#[derive(Deserialize, Default)]
#[serde(default, deny_unknown_fields)]
struct QueryParams {
    #[serde(rename = "type")]
    kind: Option<String>,
    status: Option<String>,
    scope: Option<String>,
    tag: Option<String>,
    term: Option<String>,
    author: Option<String>,
    state: Option<String>,
    after: Option<String>,
    limit: Option<usize>,
}

#[derive(Deserialize, Default)]
#[serde(default, deny_unknown_fields)]
struct ViewParams {
    name: Option<String>,
}

fn parse_params<T: for<'de> Deserialize<'de>>(params: Map<String, Value>) -> Result<T, ApiError> {
    serde_json::from_value(Value::Object(params)).map_err(|_| invalid())
}

fn validate_proposal(
    cfg: &Config,
    p: &ProposeParams,
    kind: &str,
    today: NaiveDate,
) -> Result<(), ApiError> {
    if kind != "decision" && kind != "pending" {
        return Err(invalid());
    }
    if p.title.trim().is_empty() || p.title.chars().count() > 120 {
        return Err(invalid());
    }
    if p.body.len() > 4096 {
        return Err(err("body_too_large", "body excede o limite"));
    }
    if !valid_scope(&p.scope) || p.tags.len() > 8 || p.refs.len() > 8 || p.supersedes.len() > 4 {
        return Err(invalid());
    }
    if p.tags.iter().any(|tag| {
        tag.is_empty()
            || tag.len() > 24
            || !tag
                .bytes()
                .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_' || b == b'-')
    }) {
        return Err(invalid());
    }
    if p.supersedes.iter().collect::<HashSet<_>>().len() != p.supersedes.len() {
        return Err(invalid());
    }
    if p.id.as_ref().is_some_and(|id| !valid_record_id(id))
        || p.supersedes.iter().any(|id| !valid_record_id(id))
    {
        return Err(invalid());
    }
    let source = p
        .source
        .as_ref()
        .ok_or_else(|| err("missing_source", "source obrigatorio"))?;
    if !["commit", "doc", "quote", "msg"].contains(&source.kind.as_str())
        || source.r#ref.is_empty()
        || source.r#ref.chars().count() > 200
        || parse_date(&source.at).is_none()
    {
        return Err(invalid());
    }
    if p.refs.iter().any(|r| {
        !["chatd", "git", "doc", "grant"].contains(&r.kind.as_str())
            || r.r#ref.is_empty()
            || r.r#ref.chars().count() > 200
    }) {
        return Err(invalid());
    }
    if let Some(review) = &p.review_by
        && parse_date(review).is_none()
    {
        return Err(invalid());
    }
    let fields = p.fields.as_object().ok_or_else(invalid)?;
    if kind == "decision" {
        if !keys_are(fields, &["criterion"]) {
            return Err(invalid());
        }
        let criterion = fields
            .get("criterion")
            .and_then(Value::as_str)
            .ok_or_else(invalid)?;
        if criterion.chars().count() > 400 {
            return Err(invalid());
        }
    } else {
        if !keys_subset_required(
            fields,
            &["owner", "next_action", "state"],
            &["owner", "next_action", "state", "due"],
        ) {
            return Err(invalid());
        }
        let owner = fields
            .get("owner")
            .and_then(Value::as_str)
            .ok_or_else(invalid)?;
        let next = fields
            .get("next_action")
            .and_then(Value::as_str)
            .ok_or_else(invalid)?;
        let state = fields
            .get("state")
            .and_then(Value::as_str)
            .ok_or_else(invalid)?;
        if !valid_agent(owner)
            || next.chars().count() > 240
            || !["open", "blocked", "done"].contains(&state)
        {
            return Err(invalid());
        }
        if fields
            .get("due")
            .is_some_and(|v| v.as_str().and_then(parse_date).is_none())
        {
            return Err(invalid());
        }
        let review = p
            .review_by
            .as_deref()
            .and_then(parse_date)
            .ok_or_else(invalid)?;
        if review < today {
            return Err(invalid());
        }
    }
    let mut texts = vec![
        p.title.as_str(),
        p.body.as_str(),
        p.scope.as_str(),
        source.r#ref.as_str(),
    ];
    texts.extend(p.tags.iter().map(String::as_str));
    texts.extend(p.refs.iter().map(|r| r.r#ref.as_str()));
    collect_strings(&p.fields, &mut texts);
    if texts
        .into_iter()
        .any(|text| contains_secret(text, &cfg.secret_patterns))
    {
        return Err(err("secret_detected", "possivel segredo detectado"));
    }
    Ok(())
}

fn collect_strings<'a>(value: &'a Value, out: &mut Vec<&'a str>) {
    match value {
        Value::String(s) => out.push(s),
        Value::Array(a) => {
            for v in a {
                collect_strings(v, out)
            }
        }
        Value::Object(o) => {
            for v in o.values() {
                collect_strings(v, out)
            }
        }
        _ => {}
    }
}

fn contains_secret(text: &str, extras: &[String]) -> bool {
    if extras.iter().any(|p| text.contains(p)) {
        return true;
    }
    if text.contains("-----BEGIN") && text.contains("PRIVATE KEY-----") {
        return true;
    }
    token_after(
        text,
        "AKIA",
        16,
        |b| b.is_ascii_uppercase() || b.is_ascii_digit(),
        true,
    ) || token_after(text, "ghp_", 36, |b| b.is_ascii_alphanumeric(), true)
        || token_after(
            text,
            "sk-",
            20,
            |b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-',
            false,
        )
        || ["xoxb-", "xoxa-", "xoxp-", "xoxr-", "xoxs-"]
            .iter()
            .any(|p| {
                token_after(
                    text,
                    p,
                    10,
                    |b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-',
                    false,
                )
            })
        || text
            .split(|c: char| !(c.is_ascii_alphanumeric() || c == '_' || c == '-' || c == '.'))
            .any(is_jwt)
}

fn token_after<F: Fn(u8) -> bool>(
    text: &str,
    prefix: &str,
    n: usize,
    allowed: F,
    exact: bool,
) -> bool {
    let bytes = text.as_bytes();
    let mut start = 0;
    while let Some(pos) = text[start..].find(prefix) {
        let at = start + pos + prefix.len();
        let count = bytes[at..].iter().take_while(|b| allowed(**b)).count();
        if (exact && count == n) || (!exact && count >= n) {
            return true;
        }
        start = at;
    }
    false
}
fn is_jwt(v: &str) -> bool {
    let parts: Vec<&str> = v.split('.').collect();
    parts.len() == 3
        && parts[0].starts_with("eyJ")
        && parts.iter().all(|p| {
            p.len() >= 10
                && p.bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
        })
}

fn valid_request_id(id: &str) -> Result<(), ApiError> {
    if !id.is_empty()
        && id.len() <= 64
        && id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"._-".contains(&b))
    {
        Ok(())
    } else {
        Err(invalid())
    }
}
fn valid_record_id(id: &str) -> bool {
    matches!(id.as_bytes().first(), Some(b'D' | b'P'))
        && id.as_bytes().get(1) == Some(&b'-')
        && id[2..].bytes().all(|b| b.is_ascii_digit())
        && !id[2..].is_empty()
}
fn parse_id_number(id: &str) -> Option<u64> {
    if !valid_record_id(id) {
        None
    } else {
        id[2..].parse().ok()
    }
}
fn parse_record_marker(marker: &str) -> Result<(&str, u64), String> {
    let (id, rev) = marker
        .split_once('@')
        .ok_or_else(|| "superseded_by invalido".to_string())?;
    if !valid_record_id(id) {
        return Err("superseded_by invalido".into());
    }
    let rev = rev
        .parse::<u64>()
        .map_err(|_| "superseded_by invalido".to_string())?;
    if rev == 0 {
        return Err("superseded_by invalido".into());
    }
    Ok((id, rev))
}
fn parse_date(v: &str) -> Option<NaiveDate> {
    NaiveDate::parse_from_str(v, "%Y-%m-%d").ok()
}
fn keys_are(map: &Map<String, Value>, keys: &[&str]) -> bool {
    map.len() == keys.len() && keys.iter().all(|k| map.contains_key(*k))
}
fn keys_subset_required(map: &Map<String, Value>, required: &[&str], allowed: &[&str]) -> bool {
    required.iter().all(|k| map.contains_key(*k))
        && map.keys().all(|k| allowed.contains(&k.as_str()))
}
fn field_string<'a>(v: &'a Value, key: &str) -> Option<&'a str> {
    v.as_object()?.get(key)?.as_str()
}
fn normalized_title(v: &str) -> String {
    ascii_fold(v.trim())
}
fn ascii_fold(v: &str) -> String {
    v.chars()
        .map(|c| {
            if c.is_ascii_uppercase() {
                c.to_ascii_lowercase()
            } else {
                c
            }
        })
        .collect()
}
fn record_order(a: &Record, b: &Record) -> std::cmp::Ordering {
    let ak = if a.kind == "decision" { 0 } else { 1 };
    let bk = if b.kind == "decision" { 0 } else { 1 };
    ak.cmp(&bk)
        .then_with(|| parse_id_number(&a.id).cmp(&parse_id_number(&b.id)))
        .then_with(|| a.rev.cmp(&b.rev))
}
fn compare_record_id(a: &str, b: &str) -> std::cmp::Ordering {
    let ap = a.as_bytes().first();
    let bp = b.as_bytes().first();
    ap.cmp(&bp)
        .then_with(|| parse_id_number(a).cmp(&parse_id_number(b)))
}

fn canonical_params_hash(method: &str, params: &Map<String, Value>) -> String {
    let mut params = params.clone();
    params.remove("request_id");
    let value = json!({"method":method,"params":Value::Object(params)});
    let sorted = canonicalize(&value);
    blocklog::sha256_hex(
        serde_json::to_string(&sorted)
            .unwrap_or_default()
            .as_bytes(),
    )
}
fn canonicalize(v: &Value) -> Value {
    match v {
        Value::Object(m) => {
            let mut out = BTreeMap::new();
            for (k, v) in m {
                out.insert(k.clone(), canonicalize(v));
            }
            serde_json::to_value(out).unwrap_or(Value::Null)
        }
        Value::Array(a) => Value::Array(a.iter().map(canonicalize).collect()),
        _ => v.clone(),
    }
}

fn encode_response(id: Value, result: ApiResult) -> String {
    match result {
        Ok(value) => json!({"v":1,"id":id,"ok":true,"result":value}).to_string(),
        Err(e) => json!({"v":1,"id":id,"ok":false,"error":{"code":e.code,"message":e.message}})
            .to_string(),
    }
}
fn err(code: &'static str, message: &'static str) -> ApiError {
    ApiError { code, message }
}
fn invalid() -> ApiError {
    err("invalid_params", "parametros invalidos")
}
fn not_found() -> ApiError {
    err("not_found", "registro nao encontrado")
}
fn map_commit_error(e: CommitError) -> ApiError {
    match e {
        CommitError::Broken | CommitError::Io(_) => err("broken", "log indisponivel"),
        CommitError::RequestConflict => err("request_conflict", "request_id reutilizado"),
        CommitError::TooLarge
        | CommitError::TxTooLarge
        | CommitError::EmptyTx
        | CommitError::InvalidInput(_)
        | CommitError::MissingRes => err("validation", "mutacao invalida"),
        CommitError::LogFull => err("quota", "log cheio"),
    }
}
fn exact_keys(map: &Map<String, Value>, allowed: &[&str]) -> Result<(), String> {
    if map.len() == allowed.len() && map.keys().all(|k| allowed.contains(&k.as_str())) {
        Ok(())
    } else {
        Err("campos de evento invalidos".into())
    }
}
fn string_field<'a>(m: &'a Map<String, Value>, k: &str) -> Result<&'a str, String> {
    m.get(k)
        .and_then(Value::as_str)
        .ok_or_else(|| format!("campo {k} invalido"))
}
fn u64_field(m: &Map<String, Value>, k: &str) -> Result<u64, String> {
    m.get(k)
        .and_then(Value::as_u64)
        .ok_or_else(|| format!("campo {k} invalido"))
}

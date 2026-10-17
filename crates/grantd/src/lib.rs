//! Grant decision and audit service. The socket and command line are thin
//! adapters; authorization decisions and mutations live here.

use std::{
    collections::{BTreeMap, HashMap, HashSet},
    time::Instant,
};

use blocklog::{Clock, Commit, Log, Req};
use chrono::{DateTime, SecondsFormat, Utc};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};

#[derive(Clone, Debug)]
pub struct Config {
    pub max_pending_requests: usize,
    pub line_max: usize,
    pub response_max: usize,
    pub max_connections: usize,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            max_pending_requests: 5,
            line_max: 65_536,
            response_max: 65_536,
            max_connections: 32,
        }
    }
}

impl Config {
    pub fn from_json(s: &str) -> Result<Self, String> {
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct Raw {
            max_pending_requests: Option<usize>,
            line_max: Option<usize>,
            response_max: Option<usize>,
            max_connections: Option<usize>,
        }
        let r: Raw = serde_json::from_str(s).map_err(|e| format!("invalid config: {e}"))?;
        let d = Self::default();
        let c = Self {
            max_pending_requests: r.max_pending_requests.unwrap_or(d.max_pending_requests),
            line_max: r.line_max.unwrap_or(d.line_max),
            response_max: r.response_max.unwrap_or(d.response_max),
            max_connections: r.max_connections.unwrap_or(d.max_connections),
        };
        if c.max_pending_requests == 0
            || !(1024..=4_194_304).contains(&c.line_max)
            || !(1024..=65_536).contains(&c.response_max)
            || !(1..=256).contains(&c.max_connections)
        {
            return Err("config value out of range".into());
        }
        Ok(c)
    }
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Profile {
    pub agent: String,
    pub token_sha256: String,
    pub role: String,
    #[serde(default)]
    pub can_request: bool,
    #[serde(default)]
    pub can_delegate: bool,
    pub expires: Option<String>,
}

#[derive(Clone, Debug, Default)]
pub struct Grants {
    profiles: HashMap<String, Profile>,
}

impl Grants {
    pub fn from_json(s: &str) -> Result<Self, String> {
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct RawObject {
            profiles: Vec<Profile>,
        }
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct RawGrants {
            grants: Vec<Profile>,
        }
        #[derive(Deserialize)]
        #[serde(untagged)]
        enum Raw {
            List(Vec<Profile>),
            Profiles(RawObject),
            Grants(RawGrants),
        }
        let raw: Raw = serde_json::from_str(s).map_err(|e| format!("invalid grants: {e}"))?;
        let raw_profiles = match raw {
            Raw::List(v) => v,
            Raw::Profiles(o) => o.profiles,
            Raw::Grants(o) => o.grants,
        };
        let mut profiles = HashMap::new();
        let mut hashes = HashSet::new();
        for p in raw_profiles {
            if !valid_agent(&p.agent)
                || !matches!(p.role.as_str(), "owner" | "agent")
                || p.token_sha256.len() != 64
                || !p
                    .token_sha256
                    .bytes()
                    .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
            {
                return Err("invalid grant profile".into());
            }
            if !hashes.insert(p.token_sha256.clone()) {
                return Err("duplicate grant token".into());
            }
            if let Some(exp) = &p.expires
                && parse_expiry(exp).is_none()
            {
                return Err("invalid profile expiry".into());
            }
            if profiles.insert(p.agent.clone(), p).is_some() {
                return Err("duplicate grant profile".into());
            }
        }
        Ok(Self { profiles })
    }
}

pub fn token_sha256_hex(token: &str) -> String {
    blocklog::sha256_hex(token.as_bytes())
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Grant {
    id: String,
    effect: String,
    grantee: String,
    action: String,
    resource: String,
    #[serde(default)]
    constraints: Constraints,
    not_before: String,
    until: Option<String>,
    #[serde(default)]
    permanent: bool,
    #[serde(default)]
    delegable: bool,
    source: Value,
    task_ref: Option<String>,
    parent: Option<String>,
    issuer: String,
    seq: u64,
}
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
struct Constraints {
    max_uses: Option<u64>,
    max_amount: Option<u64>,
    unit: Option<String>,
    window_s: Option<u64>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
struct Request {
    id: String,
    agent: String,
    action: String,
    resource: String,
    until: String,
    source: Option<Value>,
    #[serde(default)]
    status: String,
    seq: u64,
    approved_grant: Option<String>,
    reason: Option<String>,
}
#[derive(Clone, Debug)]
struct Use {
    ts: DateTime<Utc>,
    qty: u64,
    unit: Option<String>,
}
#[derive(Clone)]
struct RequestReplay {
    hash: String,
    result: Value,
}

pub struct Service {
    cfg: Config,
    grants: Grants,
    log: Log,
    clock: Box<dyn Clock>,
    effective_now: DateTime<Utc>,
    started: Instant,
    grants_by_id: BTreeMap<String, Grant>,
    requests: BTreeMap<String, Request>,
    revoked: HashSet<String>,
    uses: HashMap<String, Vec<Use>>,
    request_replays: HashMap<(String, String), RequestReplay>,
    broken: bool,
}

impl Service {
    pub fn new(
        cfg: Config,
        grants: Grants,
        log: Log,
        replay: Vec<Commit>,
        clock: Box<dyn Clock>,
    ) -> Result<Self, String> {
        if cfg.max_pending_requests == 0
            || !(1024..=4_194_304).contains(&cfg.line_max)
            || !(1024..=65_536).contains(&cfg.response_max)
            || !(1..=256).contains(&cfg.max_connections)
        {
            return Err("invalid config".into());
        }
        let initial_now = clock.now();
        let mut s = Self {
            cfg,
            grants,
            log,
            clock,
            effective_now: initial_now,
            started: Instant::now(),
            grants_by_id: BTreeMap::new(),
            requests: BTreeMap::new(),
            revoked: HashSet::new(),
            uses: HashMap::new(),
            request_replays: HashMap::new(),
            broken: false,
        };
        for c in replay {
            if let Some(ts) = parse_time(&c.ts)
                && ts > s.effective_now
            {
                s.effective_now = ts;
            }
            for e in c.tx {
                s.apply_event(&e, c.seq, &c.ts)
                    .map_err(|e| format!("invalid grant log at seq {}: {e}", c.seq))?;
            }
            if let (Some(req), Some(res)) = (c.req, c.res) {
                s.request_replays.insert(
                    (req.agent, req.id),
                    RequestReplay {
                        hash: req.h,
                        result: res,
                    },
                );
            }
        }
        s.validate_projection()?;
        Ok(s)
    }
    pub fn handle_line(&mut self, line: &str) -> String {
        self.refresh_clock();
        let parsed = if line.len() > self.cfg.line_max {
            Err(())
        } else {
            serde_json::from_str::<Value>(line).map_err(|_| ())
        };
        let response = match parsed {
            Err(()) => {
                json!({"v":1,"id":null,"ok":false,"error":{"code":"invalid_params","message":"invalid request"}})
            }
            Ok(v) => self.dispatch(v),
        };
        let mut out=serde_json::to_string(&response).unwrap_or_else(|_|"{\"v\":1,\"id\":null,\"ok\":false,\"error\":{\"code\":\"overloaded\",\"message\":\"response too large\"}}".into());
        if out.len() > self.cfg.response_max {
            out=json!({"v":1,"id":response.get("id").cloned().unwrap_or(Value::Null),"ok":false,"error":{"code":"overloaded","message":"response too large"}}).to_string();
        }
        out
    }
    pub fn reload_grants(&mut self, grants: Grants) {
        self.grants = grants;
    }
    pub fn head_seq(&self) -> u64 {
        self.log.head_seq()
    }
    pub fn is_broken(&self) -> bool {
        self.broken || self.log.is_broken()
    }
    pub fn t_max(&self) -> DateTime<Utc> {
        self.effective_now
    }
    fn refresh_clock(&mut self) {
        let now = self.clock.now();
        if now > self.effective_now {
            self.effective_now = now;
        }
        let normalized = self
            .effective_now
            .to_rfc3339_opts(SecondsFormat::Millis, true);
        if let Some(ts) = parse_time(&normalized) {
            self.effective_now = ts;
        }
    }

    fn dispatch(&mut self, v: Value) -> Value {
        let id = v.get("id").cloned().unwrap_or(Value::Null);
        let result = (|| -> Result<Value, (&'static str, &'static str)> {
            if v.get("v").and_then(Value::as_u64) != Some(1) {
                return Err(("invalid_params", "invalid request"));
            }
            let token = v
                .get("token")
                .and_then(Value::as_str)
                .ok_or(("unauthorized", "unauthorized"))?;
            let method = v
                .get("method")
                .and_then(Value::as_str)
                .ok_or(("invalid_params", "invalid request"))?;
            let p = v
                .get("params")
                .and_then(Value::as_object)
                .ok_or(("invalid_params", "invalid request"))?
                .clone();
            let profile = self.authenticate(token)?;
            let broken = self.is_broken();
            if !broken && let Some(replay) = self.mutation_replay(&profile, method, &p)? {
                return Ok(replay);
            }
            self.method(profile, method, p)
        })();
        match result {
            Ok(r) => json!({"v":1,"id":id,"ok":true,"result":r}),
            Err((code, msg)) => {
                json!({"v":1,"id":id,"ok":false,"error":{"code":code,"message":msg}})
            }
        }
    }
    fn authenticate(&self, token: &str) -> Result<Profile, (&'static str, &'static str)> {
        if !token.starts_with("gt_")
            || token.len() != 46
            || !token[3..]
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"-_".contains(&b))
        {
            return Err(("unauthorized", "unauthorized"));
        }
        let hash = token_sha256_hex(token);
        let now = self.effective_now;
        self.grants
            .profiles
            .values()
            .find(|p| {
                constant_eq(&p.token_sha256, &hash)
                    && p.expires
                        .as_ref()
                        .is_none_or(|x| parse_expiry(x).is_some_and(|t| now < t))
            })
            .cloned()
            .ok_or(("unauthorized", "unauthorized"))
    }
    fn method(
        &mut self,
        pf: Profile,
        m: &str,
        p: Map<String, Value>,
    ) -> Result<Value, (&'static str, &'static str)> {
        let allowed: &[&str] = match m {
            "whoami" | "status" => &[],
            "list" => &["after", "limit"],
            "show" => &["id"],
            "check" | "explain" => &["agent", "action", "resource", "qty", "unit"],
            "use" => &["agent", "action", "resource", "qty", "unit", "request_id"],
            "request" => &[
                "action",
                "resource",
                "until",
                "reason",
                "source",
                "request_id",
            ],
            "grant" | "deny_rule" => &[
                "effect",
                "grantee",
                "action",
                "resource",
                "constraints",
                "not_before",
                "until",
                "permanent",
                "delegable",
                "source",
                "task_ref",
                "request_id",
            ],
            "delegate" => &[
                "parent",
                "grantee",
                "action",
                "resource",
                "constraints",
                "not_before",
                "until",
                "permanent",
                "delegable",
                "source",
                "task_ref",
                "request_id",
            ],
            "revoke" => &["id", "reason", "request_id"],
            "approve_request" => &["id", "req", "until", "request_id"],
            "reject_request" => &["id", "req", "reason", "request_id"],
            _ => &[],
        };
        if p.keys().any(|k| !allowed.contains(&k.as_str())) {
            return Err(("invalid_params", "invalid parameters"));
        }
        match m {
            "whoami" => Ok(
                json!({"agent":pf.agent,"role":pf.role,"can_request":pf.can_request,"can_delegate":pf.can_delegate,"expires":pf.expires}),
            ),
            "status" => Ok(self.status()),
            "check" | "explain" | "use" => self.decide(&pf, m, &p),
            "list" => self.list(&pf, &p),
            "show" => self.show(&pf, &p),
            "request" => self.request(&pf, &p),
            "grant" | "deny_rule" => self.grant(&pf, &p, m == "deny_rule", None),
            "delegate" => self.delegate(&pf, &p),
            "revoke" => self.revoke(&pf, &p),
            "approve_request" => self.approve(&pf, &p),
            "reject_request" => self.reject(&pf, &p),
            _ => Err(("invalid_params", "unknown method")),
        }
    }
    fn mutation_replay(
        &mut self,
        pf: &Profile,
        method: &str,
        p: &Map<String, Value>,
    ) -> Result<Option<Value>, (&'static str, &'static str)> {
        let mutating = matches!(
            method,
            "use"
                | "request"
                | "grant"
                | "deny_rule"
                | "delegate"
                | "revoke"
                | "approve_request"
                | "reject_request"
        );
        if !mutating {
            return Ok(None);
        }
        let authorized = match method {
            "grant" | "deny_rule" | "revoke" | "approve_request" | "reject_request" => {
                pf.role == "owner"
            }
            "request" => pf.can_request,
            "delegate" => pf.can_delegate,
            "use" => p.get("agent").and_then(Value::as_str) == Some(pf.agent.as_str()),
            _ => false,
        };
        if !authorized {
            return Ok(None);
        }
        let Some(id) = p.get("request_id").and_then(Value::as_str) else {
            return Ok(None);
        };
        if id.is_empty()
            || id.len() > 64
            || !id
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"._-".contains(&b))
        {
            return Ok(None);
        }
        let key = (pf.agent.clone(), id.to_owned());
        let Some(old) = self.request_replays.get(&key) else {
            return Ok(None);
        };
        let hash = canonical_hash(method, p);
        if old.hash != hash {
            return Err(("request_conflict", "request conflict"));
        }
        let mut result = old.result.clone();
        if let Some(object) = result.as_object_mut() {
            object.insert("replayed".into(), json!(true));
        }
        Ok(Some(result))
    }
    fn status(&self) -> Value {
        let now = self.effective_now;
        let mut counts = json!({"active":0,"expired":0,"revoked":0,"future":0,"pending":0,"approved":0,"rejected":0});
        for g in self.grants_by_id.values() {
            let st = self.status_of(g, &now);
            if let Some(n) = counts.get_mut(st) {
                *n = json!(n.as_u64().unwrap_or(0) + 1);
            }
        }
        for r in self.requests.values() {
            let status = if r.status == "pending"
                && parse_time(&r.until).is_some_and(|until| until <= now)
            {
                "expired"
            } else {
                &r.status
            };
            if let Some(n) = counts.get_mut(status) {
                *n = json!(n.as_u64().unwrap_or(0) + 1);
            }
        }
        let uptime = self.started.elapsed().as_secs();
        json!({"head_seq":self.head_seq(),"counts":counts,"broken":self.is_broken(),"t_max":self.effective_now.to_rfc3339_opts(SecondsFormat::Millis,true),"uptime_s":uptime})
    }
    fn status_of(&self, g: &Grant, now: &DateTime<Utc>) -> &'static str {
        if self.revoked.contains(&g.id) {
            "revoked"
        } else if g
            .not_before
            .parse::<DateTime<Utc>>()
            .ok()
            .is_some_and(|t| *now < t)
        {
            "future"
        } else if g.permanent
            || g.until
                .as_ref()
                .and_then(|u| parse_time(u))
                .is_some_and(|u| *now < u)
        {
            "active"
        } else {
            "expired"
        }
    }
    fn visible(&self, pf: &Profile, g: &Grant) -> bool {
        pf.role == "owner" || g.grantee == pf.agent || g.grantee == "*"
    }
    fn list(
        &self,
        pf: &Profile,
        p: &Map<String, Value>,
    ) -> Result<Value, (&'static str, &'static str)> {
        let after = match p.get("after") {
            None | Some(Value::Null) => None,
            Some(v) => Some(v.as_str().ok_or(("invalid_params", "invalid parameters"))?),
        };
        let limit = match p.get("limit") {
            None => 20,
            Some(v) => v
                .as_u64()
                .filter(|n| (1..=50).contains(n))
                .ok_or(("invalid_params", "invalid parameters"))? as usize,
        };
        let after_parts = after
            .map(parse_page_cursor)
            .transpose()
            .map_err(|_| ("invalid_params", "invalid parameters"))?;
        let mut items: Vec<(String, bool, Value)> = self
            .grants_by_id
            .values()
            .filter(|g| {
                self.visible(pf, g)
                    && after_parts.is_none_or(|(is_grant, n)| is_grant && id_number(&g.id).is_some_and(|x| x > n))
            })
            .map(|g| {
                (
                    g.id.clone(),
                    true,
                    json!({"id":g.id,"effect":g.effect,"grantee":g.grantee,"action":g.action,"resource":g.resource,"until":if g.permanent {Value::Null} else {g.until.clone().map(Value::String).unwrap_or(Value::Null)},"permanent":g.permanent,"status":self.status_of(g,&self.effective_now),"parent":g.parent}),
                )
            })
            .chain(
                self.requests
                    .values()
                    .filter(|r| {
                        (pf.role == "owner" || r.agent == pf.agent)
                            && after_parts.is_none_or(|(is_grant, n)| is_grant || id_number(&r.id).is_some_and(|x| x > n))
                    })
                    .map(|r| {
                        let status = if r.status == "pending"
                            && parse_time(&r.until).is_some_and(|until| until <= self.effective_now)
                        {
                            "expired"
                        } else {
                            &r.status
                        };
                        (
                            r.id.clone(),
                            false,
                            json!({"id":r.id,"effect":"request","grantee":r.agent,"action":r.action,"resource":r.resource,"until":r.until,"permanent":false,"status":status,"parent":Value::Null}),
                        )
                    }),
            )
            .collect();
        items.sort_by_key(|(id, grant, _)| (!*grant, id_number(id).unwrap_or(u64::MAX)));
        let has_more = items.len() > limit;
        items.truncate(limit);
        let next_after = if has_more {
            items.last().map(|item| item.0.clone())
        } else {
            None
        };
        let grants = items
            .iter()
            .filter(|item| item.1)
            .map(|item| item.2.clone())
            .collect::<Vec<_>>();
        let requests = items
            .iter()
            .filter(|item| !item.1)
            .map(|item| item.2.clone())
            .collect::<Vec<_>>();
        Ok(
            json!({"grants":grants,"requests":requests,"next_after":next_after,"head_seq":self.head_seq()}),
        )
    }
    fn show(
        &self,
        pf: &Profile,
        p: &Map<String, Value>,
    ) -> Result<Value, (&'static str, &'static str)> {
        let id = strp(p, "id")?;
        if let Some(g) = self.grants_by_id.get(id) {
            if !self.visible(pf, g) {
                return Err(("not_found", "not found"));
            }
            return Ok(self.grant_value(g, &self.effective_now));
        }
        if let Some(r) = self.requests.get(id) {
            if pf.role != "owner" && r.agent != pf.agent {
                return Err(("not_found", "not found"));
            }
            let mut value = serde_json::to_value(r).map_err(|_| ("broken", "broken"))?;
            if r.status == "pending"
                && parse_time(&r.until).is_some_and(|until| until <= self.effective_now)
                && let Some(object) = value.as_object_mut()
            {
                object.insert("status".into(), json!("expired"));
            }
            return Ok(value);
        }
        Err(("not_found", "not found"))
    }
    fn grant_value(&self, g: &Grant, now: &DateTime<Utc>) -> Value {
        json!({
            "id":g.id,
            "effect":g.effect,
            "grantee":g.grantee,
            "action":g.action,
            "resource":g.resource,
            "until":if g.permanent {Value::Null} else {g.until.clone().map(Value::String).unwrap_or(Value::Null)},
            "permanent":g.permanent,
            "status":self.status_of(g, now),
            "parent":g.parent,
        })
    }
    fn decide(
        &mut self,
        pf: &Profile,
        m: &str,
        p: &Map<String, Value>,
    ) -> Result<Value, (&'static str, &'static str)> {
        let agent = strp(p, "agent")?;
        if !valid_agent(agent) {
            return Err(("invalid_params", "invalid parameters"));
        }
        if (m == "use" || pf.role != "owner") && agent != pf.agent {
            return Err(("forbidden", "forbidden"));
        }
        let action = strp(p, "action")?;
        let resource = strp(p, "resource")?;
        if !valid_action(action) {
            return Err(("invalid_action", "invalid action"));
        }
        validate_action_resource(action, resource, false)
            .map_err(|_| ("invalid_params", "invalid parameters"))?;
        let qty = p.get("qty").and_then(Value::as_u64);
        if p.contains_key("qty") && qty.is_none_or(|q| q > 9_007_199_254_740_991)
            || p.get("unit").is_some_and(|u| !u.is_string())
        {
            return Err(("invalid_params", "invalid parameters"));
        }
        if self.is_broken() {
            if m == "use" {
                return Err(("denied", "broken"));
            }
            let mut result = json!({"decision":"deny","grant":null,"reason":"broken","until":null,"signed":false});
            if m == "explain" {
                result
                    .as_object_mut()
                    .ok_or(("broken", "broken"))?
                    .insert("matched".into(), json!([]));
            }
            if m == "use" {
                result
                    .as_object_mut()
                    .ok_or(("broken", "broken"))?
                    .insert("replayed".into(), json!(false));
            }
            return Ok(result);
        }
        if m == "use" {
            let rid = strp(p, "request_id")?;
            if !valid_request_id(rid) {
                return Err(("invalid_params", "invalid request id"));
            }
            let key = (pf.agent.clone(), rid.to_owned());
            if let Some(old) = self.request_replays.get(&key) {
                let hash = canonical_hash(m, p);
                if old.hash != hash {
                    return Err(("request_conflict", "request conflict"));
                }
                let mut result = old.result.clone();
                if let Some(object) = result.as_object_mut() {
                    object.insert("replayed".into(), json!(true));
                }
                return Ok(result);
            }
        }
        let now = self.effective_now;
        let matches: Vec<Grant> = self
            .grants_by_id
            .values()
            .filter(|g| {
                self.status_of(g, &now) == "active"
                    && (g.grantee == agent || g.grantee == "*")
                    && g.action == action
                    && resource_matches(&g.resource, resource)
            })
            .cloned()
            .collect();
        if matches.iter().any(|g| {
            g.effect == "allow"
                && g.constraints.max_amount.is_some()
                && g.constraints.unit.as_deref() != p.get("unit").and_then(Value::as_str)
        }) {
            return Err(("invalid_params", "invalid parameters"));
        }
        if m == "use"
            && qty.is_none()
            && matches
                .iter()
                .any(|g| g.effect == "allow" && g.constraints.max_amount.is_some())
        {
            return Err(("invalid_params", "invalid parameters"));
        }
        let deny = matches
            .iter()
            .filter(|g| g.effect == "deny")
            .min_by_key(|g| id_number(&g.id).unwrap_or(u64::MAX));
        let mut reason = "no_grant";
        let mut chosen = None;
        if deny.is_some() {
            reason = "denied_by";
        } else {
            for g in matches.iter().filter(|g| g.effect == "allow") {
                if self.has_room_with_ancestors(
                    g,
                    if g.constraints.max_amount.is_some() {
                        Some(qty.unwrap_or(0))
                    } else {
                        qty
                    },
                    p.get("unit").and_then(Value::as_str),
                    &now,
                ) {
                    if chosen
                        .is_none_or(|old: &Grant| until_order(g, old) == std::cmp::Ordering::Less)
                    {
                        chosen = Some(g);
                    }
                } else {
                    reason = "exhausted";
                }
            }
        }
        let allowed = deny.is_none() && chosen.is_some();
        if m == "use" && allowed {
            let Some(g) = chosen.cloned() else {
                return Err(("broken", "broken"));
            };
            let request_id = Some(strp(p, "request_id")?);
            let payload = canonical_hash(m, p);
            let result = json!({"decision":"allow","grant":g.id,"reason":"ok","until":g.until,"signed":false,"replayed":false});
            let event = json!({"e":"use","agent":agent,"grant":g.id,"action":action,"resource":resource,"qty":qty.unwrap_or(1),"unit":p.get("unit"),"request_id":request_id,"hash":payload});
            let committed = self.commit_events(
                vec![event],
                request_id.map(|id| Req {
                    agent: pf.agent.clone(),
                    id: id.to_string(),
                    h: payload.clone(),
                }),
                Some(result.clone()),
            )?;
            let mut committed_result = committed.res.unwrap_or(result);
            if committed.replayed
                && let Some(object) = committed_result.as_object_mut()
            {
                object.insert("replayed".into(), json!(true));
            }
            return Ok(committed_result);
        }
        if allowed {
            let Some(g) = chosen else {
                return Err(("broken", "broken"));
            };
            let mut base = json!({"decision":"allow","grant":g.id,"reason":"ok","until":g.until,"signed":false});
            if m == "explain" {
                if let Some(object) = base.as_object_mut() {
                    object.insert(
                        "matched".into(),
                        json!(
                            matches
                                .iter()
                                .map(|g| json!({
                                    "id":g.id,
                                    "effect":g.effect,
                                    "active":true,
                                    "why":format!("{} rule matched", g.effect)
                                }))
                                .collect::<Vec<_>>()
                        ),
                    );
                }
                Ok(base)
            } else {
                Ok(base)
            }
        } else {
            let mut base =
                json!({"decision":"deny","grant":null,"reason":reason,"until":null,"signed":false});
            if let Some(denied) = deny {
                base.as_object_mut()
                    .ok_or(("broken", "broken"))?
                    .insert("denied_by".into(), json!(denied.id));
            }
            if m == "explain" {
                if let Some(object) = base.as_object_mut() {
                    object.insert(
                        "matched".into(),
                        json!(
                            matches
                                .iter()
                                .map(|g| json!({
                                    "id":g.id,
                                    "effect":g.effect,
                                    "active":true,
                                    "why":format!("{} rule matched", g.effect)
                                }))
                                .collect::<Vec<_>>()
                        ),
                    );
                }
                Ok(base)
            } else if m == "check" {
                Ok(base)
            } else {
                Err(("denied", "denied"))
            }
        }
    }
    fn has_room(
        &self,
        g: &Grant,
        qty: Option<u64>,
        unit: Option<&str>,
        now: &DateTime<Utc>,
    ) -> bool {
        let us = self.uses.get(&g.id).map(Vec::as_slice).unwrap_or(&[]);
        let start = g
            .constraints
            .window_s
            .map(|w| *now - chrono::Duration::seconds(w as i64))
            .or_else(|| parse_time(&g.not_before))
            .unwrap_or(*now);
        if g.constraints
            .max_uses
            .is_some_and(|max| us.iter().filter(|u| u.ts >= start).count() as u64 >= max)
        {
            return false;
        }
        if let Some(max) = g.constraints.max_amount
            && let Some(q) = qty
        {
            if g.constraints.unit.as_deref() != unit {
                return false;
            }
            let spent = us
                .iter()
                .filter(|u| u.ts >= start && u.unit.as_deref() == unit)
                .map(|u| u.qty)
                .fold(0u64, u64::saturating_add);
            if spent >= max || spent.saturating_add(q) > max {
                return false;
            }
        }
        true
    }
    fn has_room_with_ancestors(
        &self,
        g: &Grant,
        qty: Option<u64>,
        unit: Option<&str>,
        now: &DateTime<Utc>,
    ) -> bool {
        let mut current = Some(g);
        while let Some(grant) = current {
            if !self.has_room(grant, qty, unit, now) {
                return false;
            }
            current = grant
                .parent
                .as_deref()
                .and_then(|id| self.grants_by_id.get(id));
        }
        true
    }
    fn request(
        &mut self,
        pf: &Profile,
        p: &Map<String, Value>,
    ) -> Result<Value, (&'static str, &'static str)> {
        if !pf.can_request {
            return Err(("forbidden", "forbidden"));
        }
        let action = strp(p, "action")?;
        let resource = strp(p, "resource")?;
        if !valid_action(action) {
            return Err(("invalid_action", "invalid action"));
        }
        validate_action_resource(action, resource, true)
            .map_err(|_| ("invalid_params", "invalid parameters"))?;
        let until = strp(p, "until")?;
        if parse_time(until).is_none_or(|t| t <= self.effective_now) {
            return Err(("invalid_params", "invalid parameters"));
        }
        if p.get("reason").is_some_and(|v| !v.is_string()) {
            return Err(("invalid_params", "invalid parameters"));
        }
        let reason = p.get("reason").and_then(Value::as_str).unwrap_or("");
        if reason.chars().count() > 200 {
            return Err(("invalid_params", "invalid parameters"));
        }
        if contains_secret(reason) {
            return Err(("secret_detected", "possible secret detected"));
        }
        let source = p.get("source").cloned();
        if let Some(source) = &source {
            validate_source(source).map_err(|_| ("invalid_params", "invalid parameters"))?;
            if contains_secret(&source.to_string()) {
                return Err(("secret_detected", "possible secret detected"));
            }
        }
        if self
            .requests
            .values()
            .filter(|r| {
                r.agent == pf.agent
                    && r.status == "pending"
                    && parse_time(&r.until).is_some_and(|t| t > self.effective_now)
            })
            .count()
            >= self.cfg.max_pending_requests
        {
            return Err(("quota", "quota"));
        }
        let id = next_id("R", self.requests.keys());
        let mut event = json!({"e":"request","id":id,"agent":pf.agent,"action":action,"resource":resource,"until":until});
        if let Some(source) = source {
            event
                .as_object_mut()
                .ok_or(("broken", "broken"))?
                .insert("source".into(), source);
        }
        if !reason.is_empty() {
            event
                .as_object_mut()
                .ok_or(("broken", "broken"))?
                .insert("reason".into(), json!(reason));
        }
        let res = json!({"id":id,"status":"pending"});
        self.mutate(p, pf, &event, res)
    }
    fn grant(
        &mut self,
        pf: &Profile,
        p: &Map<String, Value>,
        deny: bool,
        parent: Option<&Grant>,
    ) -> Result<Value, (&'static str, &'static str)> {
        if pf.role != "owner" && parent.is_none() {
            return Err(("forbidden", "forbidden"));
        }
        let effect = if deny { "deny" } else { "allow" };
        if p.get("effect")
            .is_some_and(|value| value.as_str() != Some(effect))
        {
            return Err(("invalid_params", "invalid parameters"));
        }
        let grantee = strp(p, "grantee")?;
        if grantee != "*" && !valid_agent(grantee) {
            return Err(("invalid_params", "invalid parameters"));
        }
        let action = strp(p, "action")?;
        let resource = strp(p, "resource")?;
        if !valid_action(action) {
            return Err(("invalid_action", "invalid action"));
        }
        validate_action_resource(action, resource, true)
            .map_err(|_| ("invalid_params", "invalid parameters"))?;
        let mut constraints = parse_constraints(p.get("constraints"), deny)
            .map_err(|_| ("invalid_params", "invalid parameters"))?;
        if let Some(parent) = parent
            && p.get("constraints").is_none()
        {
            constraints = parent.constraints.clone();
        }
        let now = self.effective_now;
        if p.get("not_before").is_some_and(|v| !v.is_string())
            || p.get("until").is_some_and(|v| !v.is_string())
            || p.get("permanent").is_some_and(|v| !v.is_boolean())
            || p.get("delegable").is_some_and(|v| !v.is_boolean())
            || p.get("task_ref")
                .is_some_and(|v| v.as_str().is_none_or(|s| s.chars().count() > 64))
        {
            return Err(("invalid_params", "invalid parameters"));
        }
        if p.get("task_ref")
            .and_then(Value::as_str)
            .is_some_and(contains_secret)
        {
            return Err(("secret_detected", "possible secret detected"));
        }
        let not_before = p.get("not_before").and_then(Value::as_str).unwrap_or("now");
        let not_before = if not_before == "now" {
            now.to_rfc3339_opts(SecondsFormat::Millis, true)
        } else {
            if parse_time(not_before).is_none() {
                return Err(("invalid_params", "invalid parameters"));
            }
            not_before.to_owned()
        };
        let permanent = p.get("permanent").and_then(Value::as_bool).unwrap_or(false);
        if permanent && p.contains_key("until") {
            return Err(("invalid_params", "invalid parameters"));
        }
        if permanent && pf.role != "owner" {
            return Err((
                if parent.is_some() {
                    "exceeds_parent"
                } else {
                    "forbidden"
                },
                if parent.is_some() {
                    "exceeds parent"
                } else {
                    "forbidden"
                },
            ));
        }
        let until = p.get("until").and_then(Value::as_str).map(str::to_owned);
        if until.is_none() && !permanent {
            return Err(("missing_until", "missing until"));
        }
        if let Some(u) = &until
            && parse_time(u).is_none_or(|t| parse_time(&not_before).is_some_and(|n| t <= n))
        {
            return Err(("invalid_params", "invalid parameters"));
        }
        if deny
            && (p.get("delegable").and_then(Value::as_bool).unwrap_or(false)
                || p.get("constraints").is_some())
        {
            return Err(("invalid_params", "invalid parameters"));
        }
        let source = match parent {
            Some(parent) => {
                if let Some(source) = p.get("source") {
                    validate_source(source)
                        .map_err(|_| ("invalid_params", "invalid parameters"))?;
                    if contains_secret(&source.to_string()) {
                        return Err(("secret_detected", "possible secret detected"));
                    }
                }
                json!({"kind":"delegation","ref":parent.id,"at":now.format("%Y-%m-%d").to_string()})
            }
            None => p
                .get("source")
                .cloned()
                .ok_or(("missing_source", "missing source"))?,
        };
        validate_source(&source).map_err(|_| ("invalid_params", "invalid parameters"))?;
        if contains_secret(&source.to_string()) {
            return Err(("secret_detected", "possible secret detected"));
        }
        let delegable = p.get("delegable").and_then(Value::as_bool).unwrap_or(false);
        if parent.is_some() && grantee == "*" {
            return Err(("invalid_params", "invalid parameters"));
        }
        if parent.is_some()
            && (permanent || parent.is_some_and(|g| g.grantee == "*" || g.grantee != pf.agent))
        {
            return Err(("exceeds_parent", "exceeds parent"));
        }
        if parent.is_some() && validate_action_resource(action, resource, false).is_err() {
            return Err(("invalid_params", "invalid parameters"));
        }
        let id = next_grant_id(self.grants_by_id.keys());
        let mut event = json!({"e":"grant","id":id,"effect":effect,"grantee":grantee,"action":action,"resource":resource,"constraints":constraints,"not_before":not_before,"until":until,"permanent":permanent,"delegable":delegable,"source":source,"task_ref":p.get("task_ref"),"parent":parent.map(|g|g.id.as_str()),"issuer":if parent.is_some(){pf.agent.as_str()}else{"owner"}});
        if event.get("task_ref") == Some(&Value::Null) {
            event.as_object_mut().map(|o| o.remove("task_ref"));
        }
        let res = json!({"id":id});
        if let Some(parent) = parent {
            if delegable && self.depth(parent) >= 1 {
                return Err(("invalid_params", "invalid parameters"));
            }
            let fits = self.delegate_fits(parent, &event);
            if !fits {
                return Err(("exceeds_parent", "exceeds parent"));
            }
            if self.children(&parent.id) >= 3 {
                return Err(("quota", "quota"));
            }
        }
        self.mutate(p, pf, &event, res)
    }
    fn delegate(
        &mut self,
        pf: &Profile,
        p: &Map<String, Value>,
    ) -> Result<Value, (&'static str, &'static str)> {
        if self.is_broken() {
            return Err(("broken", "broken"));
        }
        if !pf.can_delegate {
            return Err(("forbidden", "forbidden"));
        }
        let parent_id = strp(p, "parent")?;
        let parent = self
            .grants_by_id
            .get(parent_id)
            .cloned()
            .ok_or(("not_found", "not found"))?;
        if !parent.delegable {
            return Err(("exceeds_parent", "exceeds parent"));
        }
        if parent.effect != "allow"
            || self.status_of(&parent, &self.effective_now) != "active"
            || parent.grantee != pf.agent
        {
            if parent.grantee == "*" {
                return Err(("exceeds_parent", "exceeds parent"));
            }
            return Err(("forbidden", "forbidden"));
        }
        let child_action = strp(p, "action")?;
        let child_resource = strp(p, "resource")?;
        let mut request = Map::new();
        request.insert("agent".into(), json!(pf.agent));
        request.insert("action".into(), json!(child_action));
        request.insert("resource".into(), json!(child_resource));
        if let Some(unit) = parent.constraints.unit.as_deref() {
            request.insert("unit".into(), json!(unit));
            request.insert("qty".into(), json!(0));
        }
        let checked = self.decide(pf, "check", &request)?;
        if checked.get("decision").and_then(Value::as_str) != Some("allow") {
            if checked.get("reason").and_then(Value::as_str) == Some("denied_by") {
                return Err(("forbidden", "forbidden"));
            }
            if checked.get("reason").and_then(Value::as_str) != Some("exhausted") {
                return Err(("exceeds_parent", "exceeds parent"));
            }
        }
        self.grant(pf, p, false, Some(&parent))
    }
    fn delegate_fits(&self, parent: &Grant, event: &Value) -> bool {
        let a = event.get("action").and_then(Value::as_str);
        let r = event.get("resource").and_then(Value::as_str);
        let u = event
            .get("until")
            .and_then(Value::as_str)
            .and_then(parse_time);
        let not_before = event
            .get("not_before")
            .and_then(Value::as_str)
            .and_then(parse_time);
        let pu = parent.until.as_deref().and_then(parse_time);
        let depth = self.depth(parent);
        let delegable = event
            .get("delegable")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        a == Some(parent.action.as_str())
            && r.is_some_and(|x| resource_contained(&parent.resource, x))
            && u.is_some_and(|x| pu.is_none_or(|p| x <= p))
            && not_before.is_some_and(|x| parse_time(&parent.not_before).is_some_and(|p| x >= p))
            && constraints_leq(&parent.constraints, event.get("constraints"))
            && depth < 2
            && (!delegable || depth < 1)
    }
    fn depth(&self, g: &Grant) -> usize {
        let mut n = 0;
        let mut p = g.parent.as_deref();
        while let Some(id) = p {
            n += 1;
            p = self.grants_by_id.get(id).and_then(|x| x.parent.as_deref());
            if n > 2 {
                break;
            }
        }
        n
    }
    fn children(&self, id: &str) -> usize {
        self.grants_by_id
            .values()
            .filter(|g| g.parent.as_deref() == Some(id))
            .count()
    }

    fn validate_projection(&self) -> Result<(), String> {
        let mut grant_numbers: Vec<_> = self
            .grants_by_id
            .keys()
            .map(|id| id_number(id).ok_or_else(|| "invalid grant id".to_owned()))
            .collect::<Result<_, _>>()?;
        grant_numbers.sort_unstable();
        if grant_numbers
            .iter()
            .enumerate()
            .any(|(index, number)| *number != index as u64 + 1)
        {
            return Err("nonsequential grant ids".into());
        }
        for prefix in ["R", "Q"] {
            let mut numbers: Vec<_> = self
                .requests
                .keys()
                .filter(|id| id.starts_with(&format!("{prefix}-")))
                .map(|id| id_number(id).ok_or_else(|| "invalid request id".to_owned()))
                .collect::<Result<_, _>>()?;
            numbers.sort_unstable();
            if numbers
                .iter()
                .enumerate()
                .any(|(index, number)| *number != index as u64 + 1)
            {
                return Err("nonsequential request ids".into());
            }
        }
        if self
            .requests
            .keys()
            .any(|id| !id.starts_with("R-") && !id.starts_with("Q-"))
        {
            return Err("invalid request id".into());
        }
        for g in self.grants_by_id.values() {
            if !g.id.starts_with("G-")
                || id_number(&g.id).is_none_or(|number| g.id != format!("G-{number:03}"))
                || !matches!(g.effect.as_str(), "allow" | "deny")
                || !(g.grantee == "*" || valid_agent(&g.grantee))
                || !valid_action(&g.action)
                || validate_action_resource(&g.action, &g.resource, true).is_err()
                || parse_time(&g.not_before).is_none()
            {
                return Err("invalid grant in log".into());
            }
            let start = parse_time(&g.not_before).ok_or("invalid grant time")?;
            match (&g.until, g.permanent) {
                (Some(_), true) => return Err("permanent grant has until".into()),
                (Some(u), false) if parse_time(u).is_none_or(|t| t <= start) => {
                    return Err("invalid grant expiry".into());
                }
                (None, false) => return Err("grant without until".into()),
                _ => {}
            }
            let invalid_constraints = if g.effect == "deny" {
                g.constraints.max_uses.is_some()
                    || g.constraints.max_amount.is_some()
                    || g.constraints.unit.is_some()
                    || g.constraints.window_s.is_some()
            } else {
                parse_constraints(
                    Some(&serde_json::to_value(&g.constraints).map_err(|_| "bad constraints")?),
                    false,
                )
                .is_err()
            };
            if invalid_constraints || g.task_ref.as_deref().is_some_and(contains_secret) {
                return Err("invalid grant constraints or task reference".into());
            }
            if validate_source(&g.source).is_err()
                || contains_secret(&g.source.to_string())
                || g.effect == "deny" && (g.delegable || g.constraints != Constraints::default())
                || g.parent.is_none() && g.issuer != "owner"
                || g.parent.is_some() && !valid_agent(&g.issuer)
            {
                return Err("invalid grant fields".into());
            }
            if let Some(parent_id) = &g.parent {
                let parent = self
                    .grants_by_id
                    .get(parent_id)
                    .ok_or("missing parent grant")?;
                let child_until = g.until.as_deref().and_then(parse_time);
                let parent_until = parent.until.as_deref().and_then(parse_time);
                let parent_not_before =
                    parse_time(&parent.not_before).ok_or("invalid parent time")?;
                if parent.effect != "allow"
                    || !parent.delegable
                    || parent.action != g.action
                    || g.grantee == "*"
                    || validate_action_resource(&g.action, &g.resource, false).is_err()
                    || !resource_contained(&parent.resource, &g.resource)
                    || g.permanent
                    || child_until.is_none_or(|c| parent_until.is_some_and(|p| c > p))
                    || parse_time(&g.not_before).is_none_or(|n| n < parent_not_before)
                    || !constraints_leq(
                        &parent.constraints,
                        Some(&serde_json::to_value(&g.constraints).map_err(|_| "bad constraints")?),
                    )
                    || self.depth(parent) >= 2
                    || parent.grantee != g.issuer
                {
                    return Err("delegation exceeds parent".into());
                }
                if g.source.get("kind").and_then(Value::as_str) != Some("delegation")
                    || g.source.get("ref").and_then(Value::as_str) != Some(parent_id)
                {
                    return Err("invalid delegation source".into());
                }
                if g.delegable && self.depth(parent) + 1 >= 2 {
                    return Err("delegation depth exceeded".into());
                }
                let mut ancestor = Some(parent_id.as_str());
                while let Some(id) = ancestor {
                    let parent = self.grants_by_id.get(id).ok_or("missing ancestor")?;
                    if self.revoked.contains(id) && !self.revoked.contains(&g.id) {
                        return Err("unrevoked descendant of revoked grant".into());
                    }
                    ancestor = parent.parent.as_deref();
                }
            }
        }
        if self
            .grants_by_id
            .values()
            .any(|g| g.parent.as_ref().is_some_and(|p| self.children(p) > 3))
        {
            return Err("too many delegated grants".into());
        }
        for r in self.requests.values() {
            if id_number(&r.id).is_none_or(|number| {
                let prefix = if r.id.starts_with("Q-") { "Q" } else { "R" };
                r.id != format!("{prefix}-{number:03}")
            }) || !valid_agent(&r.agent)
                || !valid_action(&r.action)
                || validate_action_resource(&r.action, &r.resource, true).is_err()
                || parse_time(&r.until).is_none()
                || r.source.as_ref().is_some_and(|source| {
                    validate_source(source).is_err() || contains_secret(&source.to_string())
                })
                || r.reason.as_deref().is_some_and(contains_secret)
                || !matches!(r.status.as_str(), "pending" | "approved" | "rejected")
            {
                return Err("invalid request in log".into());
            }
            if r.status == "approved" {
                let grant_id = r
                    .approved_grant
                    .as_deref()
                    .ok_or("approved request missing grant")?;
                let grant = self
                    .grants_by_id
                    .get(grant_id)
                    .ok_or("approved request grant missing")?;
                if grant.grantee != r.agent
                    || grant.action != r.action
                    || grant.resource != r.resource
                    || grant.source.get("kind").and_then(Value::as_str) != Some("request")
                    || grant.source.get("ref").and_then(Value::as_str) != Some(r.id.as_str())
                {
                    return Err("approved request grant mismatch".into());
                }
            }
        }
        Ok(())
    }
    fn revoke(
        &mut self,
        pf: &Profile,
        p: &Map<String, Value>,
    ) -> Result<Value, (&'static str, &'static str)> {
        if pf.role != "owner" {
            return Err(("forbidden", "forbidden"));
        }
        let id = strp(p, "id")?;
        if !self.grants_by_id.contains_key(id) {
            return Err(("not_found", "not found"));
        }
        if self.revoked.contains(id) {
            return Err(("conflict", "conflict"));
        }
        if p.get("reason").is_some_and(|v| !v.is_string()) {
            return Err(("invalid_params", "invalid parameters"));
        }
        let reason = p.get("reason").and_then(Value::as_str).unwrap_or("");
        if reason.chars().count() > 200 {
            return Err(("invalid_params", "invalid parameters"));
        }
        if contains_secret(reason) {
            return Err(("secret_detected", "possible secret detected"));
        }
        let mut ids = vec![id.to_owned()];
        let mut i = 0;
        while i < ids.len() {
            let parent = ids[i].clone();
            for g in self.grants_by_id.values() {
                if g.parent.as_deref() == Some(&parent) && !self.revoked.contains(&g.id) {
                    ids.push(g.id.clone());
                }
            }
            i += 1;
        }
        if ids.len() > 13 {
            return Err(("quota", "quota"));
        }
        let by = pf.agent.clone();
        let events: Vec<_> = ids
            .iter()
            .map(|x| json!({"e":"revoke","id":x,"by":by,"reason":reason}))
            .collect();
        self.mutate_events(p, pf, events, json!({"id":id}))
    }
    fn approve(
        &mut self,
        pf: &Profile,
        p: &Map<String, Value>,
    ) -> Result<Value, (&'static str, &'static str)> {
        if pf.role != "owner" {
            return Err(("forbidden", "forbidden"));
        }
        let id = p
            .get("req")
            .or_else(|| p.get("id"))
            .and_then(Value::as_str)
            .filter(|id| !id.is_empty())
            .ok_or(("invalid_params", "invalid parameters"))?;
        let r = self
            .requests
            .get(id)
            .cloned()
            .ok_or(("not_found", "not found"))?;
        if r.status != "pending"
            || parse_time(&r.until).is_none_or(|until| until <= self.effective_now)
        {
            return Err(("conflict", "conflict"));
        }
        if p.get("until").is_some_and(|v| !v.is_string()) {
            return Err(("invalid_params", "invalid parameters"));
        }
        let until = p.get("until").and_then(Value::as_str).unwrap_or(&r.until);
        if parse_time(until)
            .is_none_or(|t| t <= self.effective_now || t > parse_time(&r.until).unwrap_or(t))
        {
            return Err(("invalid_params", "invalid parameters"));
        }
        let source = json!({"kind":"request","ref":r.id,"at":self.effective_now.format("%Y-%m-%d").to_string()});
        let gid = next_grant_id(self.grants_by_id.keys());
        let grant_event = json!({"e":"grant","id":gid,"effect":"allow","grantee":r.agent,"action":r.action,"resource":r.resource,"constraints":Constraints::default(),"not_before":self.effective_now.to_rfc3339_opts(SecondsFormat::Millis,true),"until":until,"permanent":false,"delegable":false,"source":source,"parent":null,"issuer":"owner"});
        let ev = json!({"e":"approve","req":id,"grant":gid,"by":pf.agent});
        let res = json!({"id":gid,"req":id});
        self.mutate_events(p, pf, vec![ev, grant_event], res)
    }
    fn reject(
        &mut self,
        pf: &Profile,
        p: &Map<String, Value>,
    ) -> Result<Value, (&'static str, &'static str)> {
        if pf.role != "owner" {
            return Err(("forbidden", "forbidden"));
        }
        let id = p
            .get("req")
            .or_else(|| p.get("id"))
            .and_then(Value::as_str)
            .filter(|id| !id.is_empty())
            .ok_or(("invalid_params", "invalid parameters"))?;
        let Some(request) = self.requests.get(id) else {
            return Err(("not_found", "not found"));
        };
        if request.status != "pending"
            || parse_time(&request.until).is_none_or(|until| until <= self.effective_now)
        {
            return Err(("conflict", "conflict"));
        }
        if p.get("reason").is_some_and(|v| !v.is_string()) {
            return Err(("invalid_params", "invalid parameters"));
        }
        let reason = p.get("reason").and_then(Value::as_str).unwrap_or("");
        if reason.chars().count() > 200 {
            return Err(("invalid_params", "invalid parameters"));
        }
        if contains_secret(reason) {
            return Err(("secret_detected", "possible secret detected"));
        }
        let ev = json!({"e":"reject_req","req":id,"by":pf.agent,"reason":reason});
        let res = json!({"id":id});
        self.mutate(p, pf, &ev, res)
    }
    fn mutate(
        &mut self,
        p: &Map<String, Value>,
        pf: &Profile,
        event: &Value,
        res: Value,
    ) -> Result<Value, (&'static str, &'static str)> {
        self.mutate_events(p, pf, vec![event.clone()], res)
    }
    fn mutate_events(
        &mut self,
        p: &Map<String, Value>,
        pf: &Profile,
        events: Vec<Value>,
        res: Value,
    ) -> Result<Value, (&'static str, &'static str)> {
        if self.head_seq() >= 20_000 {
            return Err(("quota", "quota"));
        }
        let rid = p
            .get("request_id")
            .and_then(Value::as_str)
            .ok_or(("invalid_params", "invalid request id"))?;
        if !valid_request_id(rid) {
            return Err(("invalid_params", "invalid request id"));
        }
        let method = mutation_method(&events);
        let hash = canonical_hash(method, p);
        let c = self.commit_events(
            events,
            Some(Req {
                agent: pf.agent.clone(),
                id: rid.into(),
                h: hash,
            }),
            Some(res.clone()),
        )?;
        let mut out = c.res.unwrap_or(res);
        if let Some(object) = out.as_object_mut() {
            object.entry("seq").or_insert_with(|| json!(c.seq));
        }
        if c.replayed
            && let Some(o) = out.as_object_mut()
        {
            o.insert("replayed".into(), json!(true));
        }
        Ok(out)
    }
    fn commit_events(
        &mut self,
        events: Vec<Value>,
        req: Option<Req>,
        res: Option<Value>,
    ) -> Result<blocklog::Committed, (&'static str, &'static str)> {
        if self.is_broken() {
            return Err(("broken", "broken"));
        }
        let replay_req = req.clone();
        let saved_res = res.clone();
        match self.log.commit(events.clone(), req, res) {
            Ok(c) => {
                let timestamp = self
                    .effective_now
                    .to_rfc3339_opts(SecondsFormat::Millis, true);
                if !c.replayed {
                    for e in &events {
                        if self.apply_event(e, c.seq, &timestamp).is_err() {
                            self.broken = true;
                            return Err(("broken", "broken"));
                        }
                    }
                }
                if let (Some(req), Some(result)) = (replay_req, c.res.clone().or(saved_res)) {
                    self.request_replays.insert(
                        (req.agent, req.id),
                        RequestReplay {
                            hash: req.h,
                            result,
                        },
                    );
                }
                Ok(c)
            }
            Err(err) => {
                let code = match err {
                    blocklog::CommitError::RequestConflict => "request_conflict",
                    blocklog::CommitError::LogFull => "quota",
                    blocklog::CommitError::TooLarge
                    | blocklog::CommitError::TxTooLarge
                    | blocklog::CommitError::EmptyTx
                    | blocklog::CommitError::InvalidInput(_)
                    | blocklog::CommitError::MissingRes => "invalid_params",
                    _ => "broken",
                };
                if code == "broken" {
                    self.broken = true;
                }
                Err((
                    code,
                    if code == "request_conflict" {
                        "request conflict"
                    } else if code == "quota" {
                        "quota"
                    } else if code == "broken" {
                        "broken"
                    } else {
                        "invalid parameters"
                    },
                ))
            }
        }
    }
    fn apply_event(&mut self, e: &Value, seq: u64, ts: &str) -> Result<(), String> {
        let kind = e
            .get("e")
            .and_then(Value::as_str)
            .ok_or("missing event kind")?;
        let (keys, required): (&[&str], &[&str]) = match kind {
            "grant" => (
                &[
                    "e",
                    "id",
                    "effect",
                    "grantee",
                    "action",
                    "resource",
                    "constraints",
                    "not_before",
                    "until",
                    "permanent",
                    "delegable",
                    "source",
                    "task_ref",
                    "parent",
                    "issuer",
                ],
                &[
                    "e",
                    "id",
                    "effect",
                    "grantee",
                    "action",
                    "resource",
                    "constraints",
                    "not_before",
                    "until",
                    "permanent",
                    "delegable",
                    "source",
                    "parent",
                    "issuer",
                ],
            ),
            "request" => (
                &[
                    "e", "id", "agent", "action", "resource", "until", "source", "task_ref",
                    "reason",
                ],
                &["e", "id", "agent", "action", "resource", "until"],
            ),
            "revoke" => (&["e", "id", "by", "reason"], &["e", "id", "by", "reason"]),
            "use" => (
                &[
                    "e",
                    "agent",
                    "grant",
                    "action",
                    "resource",
                    "qty",
                    "unit",
                    "request_id",
                    "hash",
                ],
                &[
                    "e",
                    "agent",
                    "grant",
                    "action",
                    "resource",
                    "qty",
                    "request_id",
                    "hash",
                ],
            ),
            "approve" => (&["e", "req", "grant", "by"], &["e", "req", "grant", "by"]),
            "reject_req" => (&["e", "req", "by", "reason"], &["e", "req", "by", "reason"]),
            _ => return Err("unknown event kind".into()),
        };
        let object = e.as_object().ok_or("event must be object")?;
        if object.keys().any(|k| !keys.contains(&k.as_str()))
            || required.iter().any(|k| !object.contains_key(*k))
        {
            return Err("invalid event fields".into());
        }
        match kind {
            "grant" => {
                let mut v = e.clone();
                if let Some(o) = v.as_object_mut() {
                    o.remove("e");
                    o.insert("seq".into(), json!(seq));
                }
                let g: Grant = serde_json::from_value(v).map_err(|_| "invalid grant event")?;
                if self.grants_by_id.contains_key(&g.id) {
                    return Err("duplicate grant id".into());
                }
                self.grants_by_id.insert(g.id.clone(), g);
            }
            "request" => {
                let reason = e.get("reason").and_then(Value::as_str);
                if reason
                    .is_some_and(|reason| reason.chars().count() > 200 || contains_secret(reason))
                {
                    return Err("invalid request reason".into());
                }
                let r = Request {
                    id: e["id"].as_str().ok_or("bad request")?.into(),
                    agent: e["agent"].as_str().ok_or("bad agent")?.into(),
                    action: e["action"].as_str().ok_or("bad action")?.into(),
                    resource: e["resource"].as_str().ok_or("bad resource")?.into(),
                    until: e["until"].as_str().ok_or("bad until")?.into(),
                    source: e.get("source").cloned(),
                    reason: e.get("reason").and_then(Value::as_str).map(str::to_owned),
                    status: "pending".into(),
                    seq,
                    approved_grant: None,
                };
                if self.requests.contains_key(&r.id) {
                    return Err("duplicate request id".into());
                }
                self.requests.insert(r.id.clone(), r);
            }
            "revoke" => {
                if e["reason"]
                    .as_str()
                    .is_none_or(|reason| reason.chars().count() > 200 || contains_secret(reason))
                {
                    return Err("secret in revoke reason".into());
                }
                let id = e["id"].as_str().ok_or("bad revoke")?;
                if !self.grants_by_id.contains_key(id) || !self.revoked.insert(id.into()) {
                    return Err("invalid revoke".into());
                }
            }
            "use" => {
                let id = e["grant"].as_str().ok_or("bad use grant")?.to_owned();
                let grant = self
                    .grants_by_id
                    .get(&id)
                    .ok_or("use references missing grant")?;
                if grant.effect != "allow"
                    || grant.action != e["action"].as_str().unwrap_or("")
                    || !resource_matches(&grant.resource, e["resource"].as_str().unwrap_or(""))
                {
                    return Err("use outside grant".into());
                }
                let use_ts = parse_time(ts).ok_or("bad use timestamp")?;
                let qty = e["qty"].as_u64().ok_or("bad use quantity")?;
                if qty > 9_007_199_254_740_991 {
                    return Err("use quantity out of range".into());
                }
                let agent = e["agent"].as_str().ok_or("bad use agent")?;
                let unit = e.get("unit").and_then(Value::as_str);
                if (grant.grantee != "*" && grant.grantee != agent)
                    || self.revoked.contains(&id)
                    || parse_time(&grant.not_before).is_none_or(|start| use_ts < start)
                    || grant
                        .until
                        .as_deref()
                        .and_then(parse_time)
                        .is_some_and(|until| use_ts >= until)
                    || grant.constraints.max_amount.is_some()
                        && grant.constraints.unit.as_deref() != unit
                    || !self.has_room_with_ancestors(grant, Some(qty), unit, &use_ts)
                {
                    return Err("use violates grant constraints".into());
                }
                let usage = Use {
                    ts: use_ts,
                    qty,
                    unit: unit.map(str::to_owned),
                };
                let mut current = Some(id);
                while let Some(grant_id) = current {
                    let Some(grant) = self.grants_by_id.get(&grant_id) else {
                        return Err("missing ancestor grant".into());
                    };
                    self.uses.entry(grant_id).or_default().push(usage.clone());
                    current = grant.parent.clone();
                }
            }
            "approve" => {
                let id = e["req"].as_str().ok_or("bad approve")?;
                let now = parse_time(ts).ok_or("bad approve timestamp")?;
                let r = self
                    .requests
                    .get_mut(id)
                    .filter(|r| {
                        r.status == "pending"
                            && parse_time(&r.until).is_some_and(|until| now < until)
                    })
                    .ok_or("approve missing pending request")?;
                r.status = "approved".into();
                r.approved_grant = e["grant"].as_str().map(str::to_owned);
            }
            "reject_req" => {
                if e["reason"]
                    .as_str()
                    .is_none_or(|reason| reason.chars().count() > 200 || contains_secret(reason))
                {
                    return Err("secret in request reason".into());
                }
                let id = e["req"].as_str().ok_or("bad reject")?;
                let now = parse_time(ts).ok_or("bad reject timestamp")?;
                let r = self
                    .requests
                    .get_mut(id)
                    .filter(|r| {
                        r.status == "pending"
                            && parse_time(&r.until).is_some_and(|until| now < until)
                    })
                    .ok_or("reject missing pending request")?;
                r.status = "rejected".into();
                r.reason = e["reason"].as_str().map(str::to_owned);
            }
            _ => return Err("unknown event".into()),
        }
        Ok(())
    }
}

fn strp<'a>(p: &'a Map<String, Value>, k: &str) -> Result<&'a str, (&'static str, &'static str)> {
    p.get(k)
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
        .ok_or(("invalid_params", "invalid parameters"))
}
fn valid_agent(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= 32
        && s.bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b"_-".contains(&b))
}
fn valid_request_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 64
        && id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"._-".contains(&b))
}
fn valid_action(a: &str) -> bool {
    matches!(
        a,
        "fs.read"
            | "fs.write"
            | "exec"
            | "net"
            | "spend"
            | "git.commit"
            | "git.push"
            | "install"
            | "service.restart"
    )
}
fn parse_time(s: &str) -> Option<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(s)
        .ok()
        .map(|d| d.with_timezone(&Utc))
}
fn parse_expiry(s: &str) -> Option<DateTime<Utc>> {
    chrono::NaiveDate::parse_from_str(s, "%Y-%m-%d")
        .ok()?
        .succ_opt()?
        .and_hms_opt(0, 0, 0)
        .map(|d| d.and_utc())
}
fn constant_eq(a: &str, b: &str) -> bool {
    if a.len() != b.len() {
        return false;
    }
    a.bytes().zip(b.bytes()).fold(0u8, |x, (a, b)| x | (a ^ b)) == 0
}
fn canonical_hash(method: &str, p: &Map<String, Value>) -> String {
    let mut m = p.clone();
    m.remove("request_id");
    let v = json!({"method":method,"params":m});
    blocklog::sha256_hex(v.to_string().as_bytes())
}
fn mutation_method(events: &[Value]) -> &'static str {
    if events
        .iter()
        .any(|e| e.get("e").and_then(Value::as_str) == Some("approve"))
    {
        return "approve_request";
    }
    match events
        .first()
        .and_then(|e| e.get("e"))
        .and_then(Value::as_str)
    {
        Some("request") => "request",
        Some("revoke") => "revoke",
        Some("reject_req") => "reject_request",
        Some("grant") => {
            if events
                .first()
                .and_then(|e| e.get("effect"))
                .and_then(Value::as_str)
                == Some("deny")
            {
                "deny_rule"
            } else if events
                .first()
                .is_some_and(|e| e.get("parent").is_some_and(|p| !p.is_null()))
            {
                "delegate"
            } else {
                "grant"
            }
        }
        _ => "unknown",
    }
}
fn next_id<'a>(prefix: &str, ids: impl Iterator<Item = &'a String>) -> String {
    let max = ids
        .filter_map(|s| s.strip_prefix(&format!("{prefix}-"))?.parse::<u64>().ok())
        .max()
        .unwrap_or(0);
    format!("{prefix}-{:03}", max.saturating_add(1))
}
fn next_grant_id<'a>(ids: impl Iterator<Item = &'a String>) -> String {
    next_id("G", ids)
}
fn id_number(id: &str) -> Option<u64> {
    id.rsplit_once('-')?.1.parse().ok()
}
fn parse_page_cursor(id: &str) -> Result<(bool, u64), ()> {
    let (prefix, number) = id.rsplit_once('-').ok_or(())?;
    let is_grant = prefix == "G";
    if !is_grant && prefix != "R" && prefix != "Q" {
        return Err(());
    }
    Ok((is_grant, number.parse().map_err(|_| ())?))
}
fn until_order(a: &Grant, b: &Grant) -> std::cmp::Ordering {
    match (a.permanent, b.permanent) {
        (true, false) => std::cmp::Ordering::Greater,
        (false, true) => std::cmp::Ordering::Less,
        _ => a
            .until
            .as_deref()
            .and_then(parse_time)
            .cmp(&b.until.as_deref().and_then(parse_time))
            .then_with(|| {
                id_number(&a.id)
                    .unwrap_or(u64::MAX)
                    .cmp(&id_number(&b.id).unwrap_or(u64::MAX))
            }),
    }
}
fn constraints_leq(parent: &Constraints, child: Option<&Value>) -> bool {
    let Some(c) = child.and_then(|v| serde_json::from_value::<Constraints>(v.clone()).ok()) else {
        return false;
    };
    parent
        .max_uses
        .is_none_or(|p| c.max_uses.is_some_and(|x| x <= p))
        && parent
            .max_amount
            .is_none_or(|p| c.max_amount.is_some_and(|x| x <= p) && parent.unit == c.unit)
        && parent.window_s == c.window_s
}
fn parse_constraints(v: Option<&Value>, deny: bool) -> Result<Constraints, ()> {
    if deny && v.is_some() {
        return Err(());
    }
    let c: Constraints = match v {
        Some(v) => serde_json::from_value(v.clone()).map_err(|_| ())?,
        None => Constraints::default(),
    };
    if c.max_uses == Some(0)
        || c.max_amount.is_some_and(|x| x > 9_007_199_254_740_991)
        || c.max_amount.is_some() != c.unit.is_some()
        || c.unit.as_ref().is_some_and(|x| {
            x.is_empty()
                || x.len() > 24
                || !x
                    .bytes()
                    .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b"._-".contains(&b))
        })
        || c.window_s.is_some_and(|x| !(1..=31_536_000).contains(&x))
    {
        return Err(());
    }
    Ok(c)
}
fn validate_source(v: &Value) -> Result<(), ()> {
    let o = v.as_object().ok_or(())?;
    if o.len() != 3
        || !matches!(
            o.get("kind").and_then(Value::as_str),
            Some("quote" | "doc" | "msg" | "commit" | "delegation" | "request")
        )
        || o.get("ref")
            .and_then(Value::as_str)
            .is_none_or(|s| s.is_empty() || s.len() > 200)
        || o.get("at").and_then(Value::as_str).is_none_or(|s| {
            s.len() != 10 || chrono::NaiveDate::parse_from_str(s, "%Y-%m-%d").is_err()
        })
    {
        return Err(());
    }
    Ok(())
}
fn validate_action_resource(a: &str, r: &str, glob: bool) -> Result<(), ()> {
    let valid = match a {
        "fs.read" | "fs.write" | "exec" | "git.commit" | "git.push" => {
            r.starts_with('/')
                && !r.contains("//")
                && !r.split('/').any(|x| x == "." || x == "..")
                && (r == "/" || !r.ends_with('/'))
                && (glob && matches!(a, "fs.read" | "fs.write" | "git.commit" | "git.push")
                    || !r.contains('*'))
                && r.split('/').all(|part| {
                    !part.contains('*')
                        || (glob
                            && matches!(a, "fs.read" | "fs.write" | "git.commit" | "git.push")
                            && matches!(part, "*" | "**"))
                })
        }
        "net" => valid_network_resource(r),
        "spend" => {
            !r.is_empty()
                && r.len() <= 80
                && r.bytes()
                    .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b"._:/-".contains(&b))
        }
        "install" | "service.restart" => {
            !r.is_empty()
                && r.len() <= 64
                && r.bytes()
                    .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b"._-".contains(&b))
        }
        _ => return Err(()),
    };
    if valid { Ok(()) } else { Err(()) }
}
fn resource_matches(pattern: &str, value: &str) -> bool {
    let p: Vec<_> = pattern.split('/').collect();
    let v: Vec<_> = value.split('/').collect();
    fn go(p: &[&str], v: &[&str]) -> bool {
        if p.is_empty() {
            return v.is_empty();
        }
        match p[0] {
            "**" => (0..=v.len()).any(|n| go(&p[1..], &v[n..])),
            "*" => !v.is_empty() && go(&p[1..], &v[1..]),
            x => !v.is_empty() && x == v[0] && go(&p[1..], &v[1..]),
        }
    }
    go(&p, &v)
}
fn resource_contained(parent: &str, child: &str) -> bool {
    let p: Vec<_> = parent.split('/').collect();
    let c: Vec<_> = child.split('/').collect();
    fn closure(pattern: &[&str], mut states: Vec<usize>) -> Vec<usize> {
        states.sort_unstable();
        states.dedup();
        let mut i = 0;
        while i < states.len() {
            let state = states[i];
            if state < pattern.len() && pattern[state] == "**" {
                let next = state + 1;
                if let Err(pos) = states.binary_search(&next) {
                    states.insert(pos, next);
                }
            }
            i += 1;
        }
        states
    }
    fn step(pattern: &[&str], states: &[usize], symbol: Option<&str>) -> Vec<usize> {
        let mut next = Vec::new();
        for state in states {
            if *state == pattern.len() {
                continue;
            }
            match pattern[*state] {
                "**" | "*" => next.push(*state + usize::from(pattern[*state] != "**")),
                literal if symbol == Some(literal) => next.push(*state + 1),
                _ => {}
            }
            if pattern[*state] == "**" {
                next.push(*state);
            }
        }
        closure(pattern, next)
    }

    let mut symbols: Vec<&str> = p
        .iter()
        .chain(c.iter())
        .copied()
        .filter(|part| *part != "*" && *part != "**")
        .collect();
    symbols.sort_unstable();
    symbols.dedup();
    let mut alphabet: Vec<Option<&str>> = symbols.into_iter().map(Some).collect();
    alphabet.push(None);

    let start = (closure(&p, vec![0]), closure(&c, vec![0]));
    let mut pending = vec![start.clone()];
    let mut seen = HashSet::from([start]);
    while let Some((parent_states, child_states)) = pending.pop() {
        if child_states.contains(&c.len()) && !parent_states.contains(&p.len()) {
            return false;
        }
        for symbol in &alphabet {
            let child_next = step(&c, &child_states, *symbol);
            if child_next.is_empty() {
                continue;
            }
            let parent_next = step(&p, &parent_states, *symbol);
            let state = (parent_next, child_next);
            if seen.insert(state.clone()) {
                pending.push(state);
            }
        }
    }
    true
}

fn valid_network_resource(resource: &str) -> bool {
    let (host, port) = match resource.split_once(':') {
        Some((host, port)) if !port.is_empty() && !port.contains(':') => (host, Some(port)),
        Some(_) => return false,
        None => (resource, None),
    };
    if host.is_empty()
        || host.len() > 253
        || !host
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b".-".contains(&b))
    {
        return false;
    }
    port.is_none_or(|p| {
        p.bytes().all(|b| b.is_ascii_digit()) && p.parse::<u16>().is_ok_and(|n| n > 0)
    })
}
fn contains_secret(s: &str) -> bool {
    if s.contains("-----BEGIN") && s.contains("PRIVATE KEY-----") {
        return true;
    }
    token_after(
        s,
        "AKIA",
        16,
        |b| b.is_ascii_uppercase() || b.is_ascii_digit(),
        true,
    ) || token_after(s, "ghp_", 36, |b| b.is_ascii_alphanumeric(), true)
        || token_after(
            s,
            "sk-",
            20,
            |b| b.is_ascii_alphanumeric() || b"_-".contains(&b),
            false,
        )
        || ["xoxb-", "xoxa-", "xoxp-", "xoxr-", "xoxs-"]
            .iter()
            .any(|prefix| {
                token_after(
                    s,
                    prefix,
                    10,
                    |b| b.is_ascii_alphanumeric() || b"_-".contains(&b),
                    false,
                )
            })
        || s.split(|c: char| !(c.is_ascii_alphanumeric() || c == '_' || c == '-' || c == '.'))
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
fn is_jwt(value: &str) -> bool {
    let parts: Vec<_> = value.split('.').collect();
    parts.len() == 3
        && parts[0].starts_with("eyJ")
        && parts.iter().all(|part| {
            part.len() >= 10
                && part
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b"_-".contains(&b))
        })
}

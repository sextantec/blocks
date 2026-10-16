use serde::Deserialize;
use serde::de::{MapAccess, SeqAccess, Visitor};
use serde_json::Value;
use std::{
    collections::{BTreeMap, HashSet},
    fmt, fs,
    path::{Path, PathBuf},
};

#[derive(Clone, Debug)]
pub struct Config {
    pub tmux_bin: String,
    pub tmux_socket: Option<String>,
    pub rules_file: PathBuf,
    pub poll_ms: u64,
    pub events: bool,
    pub event_debounce_ms: u64,
    pub safety_poll_ms: u64,
    pub source_timeout_ms: u64,
    pub stale_after_ms: u64,
    pub gone_after_s: u64,
    pub idle_confirm_polls: u64,
    pub idle_confirm_ms: u64,
    pub ring: usize,
    pub agents: BTreeMap<String, AgentConfig>,
}

struct Limits {
    poll_ms: u64,
    event_debounce_ms: u64,
    safety_poll_ms: u64,
    source_timeout_ms: u64,
    stale_after_ms: u64,
    gone_after_s: u64,
    idle_confirm_polls: u64,
    idle_confirm_ms: u64,
    ring: usize,
}

#[derive(Clone, Debug)]
pub struct AgentConfig {
    pub kind: String,
    pub selector: Selector,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Selector {
    Pane(String),
    Tag(String),
}

#[derive(Debug)]
pub struct Unique(pub Value);
impl<'de> Deserialize<'de> for Unique {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        struct V;
        impl<'de> Visitor<'de> for V {
            type Value = Unique;
            fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
                f.write_str("JSON without duplicate object keys")
            }
            fn visit_bool<E: serde::de::Error>(self, v: bool) -> Result<Self::Value, E> {
                Ok(Unique(Value::Bool(v)))
            }
            fn visit_i64<E: serde::de::Error>(self, v: i64) -> Result<Self::Value, E> {
                Ok(Unique(Value::from(v)))
            }
            fn visit_u64<E: serde::de::Error>(self, v: u64) -> Result<Self::Value, E> {
                Ok(Unique(Value::from(v)))
            }
            fn visit_f64<E: serde::de::Error>(self, v: f64) -> Result<Self::Value, E> {
                Ok(Unique(Value::from(v)))
            }
            fn visit_str<E: serde::de::Error>(self, v: &str) -> Result<Self::Value, E> {
                Ok(Unique(Value::from(v)))
            }
            fn visit_string<E: serde::de::Error>(self, v: String) -> Result<Self::Value, E> {
                Ok(Unique(Value::from(v)))
            }
            fn visit_none<E: serde::de::Error>(self) -> Result<Self::Value, E> {
                Ok(Unique(Value::Null))
            }
            fn visit_unit<E: serde::de::Error>(self) -> Result<Self::Value, E> {
                Ok(Unique(Value::Null))
            }
            fn visit_seq<A: SeqAccess<'de>>(self, mut a: A) -> Result<Self::Value, A::Error> {
                let mut out = Vec::new();
                while let Some(v) = a.next_element::<Unique>()? {
                    out.push(v.0)
                }
                Ok(Unique(Value::Array(out)))
            }
            fn visit_map<A: MapAccess<'de>>(self, mut a: A) -> Result<Self::Value, A::Error> {
                let mut out = serde_json::Map::new();
                while let Some((k, v)) = a.next_entry::<String, Unique>()? {
                    if out.insert(k.clone(), v.0).is_some() {
                        return Err(serde::de::Error::custom(format!("duplicate key: {k}")));
                    }
                }
                Ok(Unique(Value::Object(out)))
            }
        }
        d.deserialize_any(V)
    }
}

pub fn read_unique(path: &Path) -> Result<Value, String> {
    let raw = fs::read(path).map_err(|_| "arquivo invalido".to_string())?;
    let mut de = serde_json::Deserializer::from_slice(&raw);
    let val = Unique::deserialize(&mut de)
        .map_err(|_| "JSON invalido".to_string())?
        .0;
    de.end().map_err(|_| "JSON invalido".to_string())?;
    Ok(val)
}

impl Config {
    pub fn read(path: &Path) -> Result<Self, String> {
        Self::parse(read_unique(path)?)
    }

    fn parse(v: Value) -> Result<Self, String> {
        let o = v.as_object().ok_or("config deve ser objeto")?;
        let allowed = [
            "tmux_bin",
            "tmux_socket",
            "rules_file",
            "poll_ms",
            "events",
            "event_debounce_ms",
            "safety_poll_ms",
            "source_timeout_ms",
            "stale_after_ms",
            "gone_after_s",
            "idle_confirm_polls",
            "idle_confirm_ms",
            "ring",
            "agents",
        ];
        if o.keys().any(|k| !allowed.contains(&k.as_str())) {
            return Err("campo desconhecido na config".into());
        }
        let s = |k: &str, d: &str| -> Result<String, String> {
            Ok(o.get(k)
                .map(|x| {
                    x.as_str()
                        .map(str::to_owned)
                        .ok_or_else(|| format!("{k} invalido"))
                })
                .transpose()?
                .unwrap_or_else(|| d.into()))
        };
        let n = |k: &str, d: u64| -> Result<u64, String> {
            Ok(o.get(k)
                .map(|x| x.as_u64().ok_or_else(|| format!("{k} invalido")))
                .transpose()?
                .unwrap_or(d))
        };
        let b = |k: &str, d: bool| -> Result<bool, String> {
            Ok(o.get(k)
                .map(|x| x.as_bool().ok_or_else(|| format!("{k} invalido")))
                .transpose()?
                .unwrap_or(d))
        };

        let tmux_bin = s("tmux_bin", "tmux")?;
        let tmux_socket = match o.get("tmux_socket") {
            None | Some(Value::Null) => None,
            Some(Value::String(p)) if Path::new(p).is_absolute() => Some(p.clone()),
            _ => return Err("tmux_socket invalido".into()),
        };
        let rules_file = PathBuf::from(s("rules_file", "")?);
        if tmux_bin.is_empty() || rules_file.as_os_str().is_empty() || !rules_file.exists() {
            return Err("tmux_bin ou rules_file invalido".into());
        }
        let poll_ms = n("poll_ms", 2000)?;
        let events = b("events", true)?;
        let event_debounce_ms = n("event_debounce_ms", 150)?;
        let safety_poll_ms = n("safety_poll_ms", 5000)?;
        let source_timeout_ms = n("source_timeout_ms", 3000)?;
        let stale_after_ms = n("stale_after_ms", 30000)?;
        let gone_after_s = n("gone_after_s", 120)?;
        let idle_confirm_polls = n("idle_confirm_polls", 2)?;
        let idle_confirm_ms = n("idle_confirm_ms", 2500)?;
        let ring = n("ring", 4096)? as usize;
        validate_limits(&Limits {
            poll_ms,
            event_debounce_ms,
            safety_poll_ms,
            source_timeout_ms,
            stale_after_ms,
            gone_after_s,
            idle_confirm_polls,
            idle_confirm_ms,
            ring,
        })?;
        let agents = parse_agents(o.get("agents").ok_or("agents invalido")?)?;
        Ok(Self {
            tmux_bin,
            tmux_socket,
            rules_file,
            poll_ms,
            events,
            event_debounce_ms,
            safety_poll_ms,
            source_timeout_ms,
            stale_after_ms,
            gone_after_s,
            idle_confirm_polls,
            idle_confirm_ms,
            ring,
            agents,
        })
    }
}

fn validate_limits(l: &Limits) -> Result<(), String> {
    if !(200..=10000).contains(&l.poll_ms)
        || !(50..=1000).contains(&l.event_debounce_ms)
        || !(1000..=60000).contains(&l.safety_poll_ms)
        || l.safety_poll_ms < l.poll_ms
        || !(200..=30000).contains(&l.source_timeout_ms)
        || l.stale_after_ms <= l.poll_ms.saturating_add(l.source_timeout_ms)
        || !(5..=86400).contains(&l.gone_after_s)
        || !(1..=5).contains(&l.idle_confirm_polls)
        || l.idle_confirm_ms > 30000
        || !(64..=65536).contains(&l.ring)
    {
        return Err("limite da config invalido".into());
    }
    Ok(())
}

fn parse_agents(value: &Value) -> Result<BTreeMap<String, AgentConfig>, String> {
    let ag = value.as_object().ok_or("agents invalido")?;
    if ag.is_empty() || ag.len() > 32 {
        return Err("quantidade de agents invalida".into());
    }
    let mut agents = BTreeMap::new();
    let mut selectors = HashSet::new();
    for (id, a) in ag {
        let (agent, key) = parse_agent(id, a)?;
        if !selectors.insert(key) {
            return Err("pane ou tag repetido".into());
        }
        agents.insert(id.clone(), agent);
    }
    Ok(agents)
}

fn parse_agent(id: &str, value: &Value) -> Result<(AgentConfig, String), String> {
    if id.is_empty()
        || id.len() > 32
        || !id
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_' || b == b'-')
    {
        return Err("id invalido".into());
    }
    let a = value.as_object().ok_or("agent invalido")?;
    if a.keys().any(|k| !["kind", "sources"].contains(&k.as_str())) {
        return Err("campo desconhecido em agent".into());
    }
    let kind = a
        .get("kind")
        .and_then(Value::as_str)
        .filter(|kind| ["claude", "codex", "pi"].contains(kind))
        .ok_or("kind invalido")?;
    let sources = a
        .get("sources")
        .and_then(Value::as_array)
        .filter(|sources| sources.len() == 1)
        .ok_or("sources invalido")?;
    let source = sources[0].as_object().ok_or("source invalida")?;
    if source.get("type").and_then(Value::as_str) != Some("tmux") {
        return Err("source invalida".into());
    }
    let selector = match (source.get("pane"), source.get("tag")) {
        (Some(Value::String(p)), None) if valid_pane(p) => Selector::Pane(p.clone()),
        (None, Some(Value::String(t))) if valid_tag(t) => Selector::Tag(t.clone()),
        _ => return Err("seletor tmux invalido".into()),
    };
    if source
        .keys()
        .any(|k| !["type", "pane", "tag"].contains(&k.as_str()))
    {
        return Err("campo desconhecido em source".into());
    }
    let key = match &selector {
        Selector::Pane(pane) => format!("pane:{pane}"),
        Selector::Tag(tag) => format!("tag:{tag}"),
    };
    Ok((
        AgentConfig {
            kind: kind.into(),
            selector,
        },
        key,
    ))
}

pub(crate) fn valid_pane(p: &str) -> bool {
    static PANE: std::sync::OnceLock<regex::Regex> = std::sync::OnceLock::new();
    PANE.get_or_init(|| regex::Regex::new(r"^%[0-9]+$").unwrap())
        .is_match(p)
}

fn valid_tag(t: &str) -> bool {
    static TAG: std::sync::OnceLock<regex::Regex> = std::sync::OnceLock::new();
    TAG.get_or_init(|| regex::Regex::new(r"^[a-z0-9_-]{1,32}$").unwrap())
        .is_match(t)
}

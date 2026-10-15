use serde::Deserialize;
use serde::de::{MapAccess, SeqAccess, Visitor};
use serde_json::Value;
use std::{
    collections::{BTreeMap, HashSet},
    fmt, fs,
    path::Path,
};

#[derive(Clone, Debug)]
pub struct Config {
    pub herdr_bin: String,
    pub poll_ms: u64,
    pub source_timeout_ms: u64,
    pub stale_after_ms: u64,
    pub gone_after_s: u64,
    pub idle_confirm_polls: u64,
    pub idle_confirm_ms: u64,
    pub ring: usize,
    pub agents: BTreeMap<String, AgentConfig>,
}

#[derive(Clone, Debug)]
pub struct AgentConfig {
    pub kind: String,
    pub pane: String,
}

#[derive(Debug)]
struct Unique(Value);
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

impl Config {
    pub fn read(path: &Path) -> Result<Self, String> {
        let raw = fs::read(path).map_err(|e| format!("config: {e}"))?;
        let mut de = serde_json::Deserializer::from_slice(&raw);
        let val = Unique::deserialize(&mut de)
            .map_err(|_| "config invalida".to_string())?
            .0;
        de.end().map_err(|_| "config invalida".to_string())?;
        Self::parse(val)
    }
    fn parse(v: Value) -> Result<Self, String> {
        let o = v.as_object().ok_or("config deve ser objeto")?;
        let allowed = [
            "herdr_bin",
            "poll_ms",
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
        let herdr_bin = s("herdr_bin", "herdr")?;
        let poll_ms = n("poll_ms", 2000)?;
        let source_timeout_ms = n("source_timeout_ms", 3000)?;
        let stale_after_ms = n("stale_after_ms", 30000)?;
        let gone_after_s = n("gone_after_s", 120)?;
        let idle_confirm_polls = n("idle_confirm_polls", 2)?;
        let idle_confirm_ms = n("idle_confirm_ms", 2500)?;
        let ring = n("ring", 4096)? as usize;
        if !(200..=10000).contains(&poll_ms)
            || !(200..=30000).contains(&source_timeout_ms)
            || stale_after_ms <= poll_ms + source_timeout_ms
            || !(5..=86400).contains(&gone_after_s)
            || !(1..=5).contains(&idle_confirm_polls)
            || idle_confirm_ms > 30000
            || !(64..=65536).contains(&ring)
        {
            return Err("limite da config invalido".into());
        }
        let ag = o
            .get("agents")
            .and_then(Value::as_object)
            .ok_or("agents invalido")?;
        if ag.is_empty() || ag.len() > 32 {
            return Err("quantidade de agents invalida".into());
        }
        let mut agents = BTreeMap::new();
        let mut panes = HashSet::new();
        for (id, a) in ag {
            if id.len() > 32
                || id.is_empty()
                || !id
                    .bytes()
                    .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_' || b == b'-')
            {
                return Err("id invalido".into());
            }
            let a = a.as_object().ok_or("agent invalido")?;
            if a.keys().any(|k| !["kind", "sources"].contains(&k.as_str())) {
                return Err("campo desconhecido em agent".into());
            }
            let kind = a
                .get("kind")
                .and_then(Value::as_str)
                .ok_or("kind invalido")?;
            if kind != "claude" && kind != "codex" {
                return Err("kind invalido".into());
            }
            let src = a
                .get("sources")
                .and_then(Value::as_array)
                .ok_or("sources invalido")?;
            if src.len() != 1 {
                return Err("sources deve conter um item".into());
            }
            let so = src[0].as_object().ok_or("source invalida")?;
            if so.keys().any(|k| !["type", "pane"].contains(&k.as_str()))
                || so.get("type").and_then(Value::as_str) != Some("herdr")
            {
                return Err("source invalida".into());
            }
            let pane = so
                .get("pane")
                .and_then(Value::as_str)
                .ok_or("pane invalido")?
                .to_owned();
            if pane.is_empty() || !panes.insert(pane.clone()) {
                return Err("pane repetido ou invalido".into());
            }
            agents.insert(
                id.clone(),
                AgentConfig {
                    kind: kind.into(),
                    pane,
                },
            );
        }
        Ok(Self {
            herdr_bin,
            poll_ms,
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

//! Relatorio local, sem conteudo livre, de PEDIDO/PERGUNTA aceitos sem resultado.

use crate::{Failure, env_path, parse_opts, usage};
use chrono::{DateTime, Utc};
use serde::Serialize;
use serde_json::Value;
use std::fs::File;
use std::io::{BufRead, BufReader};
use std::path::PathBuf;

#[derive(Serialize)]
struct Item {
    id: String,
    #[serde(skip)]
    to: String,
    kind: String,
    from: String,
    registered_at: String,
    accepted_at: Option<String>,
    due_at: Option<String>,
    overdue: bool,
    #[serde(rename = "await")]
    await_class: Option<String>,
    since: Option<String>,
}

pub fn run(args: &[String]) -> Result<(), Failure> {
    let opts = parse_opts(args, &["state-dir", "as", "stamp-file"], &["json"])?;
    let state = opts
        .get("state-dir")
        .map(PathBuf::from)
        .or_else(|| env_path("CHATD_STATE_DIR"))
        .ok_or_else(|| usage("obligations exige --state-dir ou CHATD_STATE_DIR"))?;
    let file = File::open(state.join("log.jsonl"))
        .map_err(|e| Failure::Unavailable(format!("nao li log.jsonl: {e}")))?;
    let mut messages: std::collections::BTreeMap<String, (Item, bool)> =
        std::collections::BTreeMap::new();
    for line in BufReader::new(file).lines() {
        let line = line.map_err(|e| Failure::Unavailable(format!("falha lendo log: {e}")))?;
        let Ok(ev) = serde_json::from_str::<Value>(&line) else {
            continue;
        };
        let Some(id) = ev["id"].as_str() else {
            continue;
        };
        match ev["type"].as_str() {
            Some("registered") => {
                let (Some(kind), Some(from), Some(registered_at)) =
                    (ev["kind"].as_str(), ev["from"].as_str(), ev["ts"].as_str())
                else {
                    continue;
                };
                if !matches!(kind, "PEDIDO" | "PERGUNTA") {
                    continue;
                }
                let due_at = ev["due_s"].as_u64().and_then(|s| {
                    DateTime::parse_from_rfc3339(registered_at)
                        .ok()
                        .map(|t| (t + chrono::TimeDelta::seconds(s as i64)).to_rfc3339())
                });
                messages.insert(
                    id.to_string(),
                    (
                        Item {
                            id: id.to_string(),
                            to: ev["to"].as_str().unwrap_or("").to_string(),
                            kind: kind.to_string(),
                            from: from.to_string(),
                            registered_at: registered_at.to_string(),
                            accepted_at: None,
                            due_at,
                            overdue: false,
                            await_class: None,
                            since: None,
                        },
                        false,
                    ),
                );
            }
            Some("accepted") => {
                if let Some((item, resulted)) = messages.get_mut(id)
                    && !*resulted
                {
                    item.accepted_at = ev["ts"].as_str().map(str::to_string);
                }
            }
            Some("blocked") => {
                if let Some((item, _)) = messages.get_mut(id) {
                    item.await_class = ev["await"].as_str().map(str::to_string);
                    item.since = ev["ts"].as_str().map(str::to_string);
                }
            }
            Some("resulted") => {
                if let Some((_, resulted)) = messages.get_mut(id) {
                    *resulted = true;
                }
            }
            _ => {}
        }
    }
    let now = Utc::now();
    let filter = opts.get("as");
    let mut items: Vec<Item> = messages
        .into_values()
        .filter_map(|(mut item, resulted)| {
            if resulted || item.accepted_at.is_none() || filter.is_some_and(|f| f != item.to) {
                return None;
            }
            item.overdue = item
                .due_at
                .as_deref()
                .and_then(|d| DateTime::parse_from_rfc3339(d).ok())
                .is_some_and(|d| now >= d);
            Some(item)
        })
        .collect();
    items.sort_by(|a, b| a.registered_at.cmp(&b.registered_at).then(a.id.cmp(&b.id)));
    let stamp = opts
        .get("stamp-file")
        .map(PathBuf::from)
        .unwrap_or_else(|| state.parent().unwrap_or(&state).join("stale.stamp"));
    let scheduler_ok = std::fs::read(&stamp)
        .ok()
        .and_then(|b| serde_json::from_slice::<Value>(&b).ok())
        .and_then(|v| {
            v.get("scheduler_last_run")
                .and_then(Value::as_str)
                .map(str::to_string)
        })
        .and_then(|s| DateTime::parse_from_rfc3339(&s).ok())
        .map(|t| Utc::now().signed_duration_since(t).num_seconds() < 900);
    if opts.flag("json") {
        println!(
            "{}",
            serde_json::json!({"v":1,"scheduler_ok":scheduler_ok,"obligations":items})
        );
    } else {
        for item in items {
            println!(
                "{} {} from={} registered_at={} accepted_at={} due_at={} overdue={} await={} since={}",
                item.id,
                item.kind,
                item.from,
                item.registered_at,
                item.accepted_at.as_deref().unwrap_or("-"),
                item.due_at.as_deref().unwrap_or("-"),
                item.overdue,
                item.await_class.as_deref().unwrap_or("-"),
                item.since.as_deref().unwrap_or("-")
            );
        }
    }
    Ok(())
}

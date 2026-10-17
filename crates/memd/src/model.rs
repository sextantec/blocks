use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Source {
    pub kind: String,
    pub r#ref: String,
    pub at: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Reference {
    pub kind: String,
    pub r#ref: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Record {
    pub id: String,
    pub rev: u64,
    #[serde(rename = "type")]
    pub kind: String,
    pub title: String,
    pub body: String,
    pub scope: String,
    pub tags: Vec<String>,
    pub fields: Value,
    pub source: Source,
    pub refs: Vec<Reference>,
    pub supersedes: Vec<String>,
    pub status: String,
    pub author: String,
    pub seq: u64,
    pub ts: String,
    pub promoted_by: Option<String>,
    pub proposed_seq: u64,
    pub promoted_seq: Option<u64>,
    pub superseded_by: Option<String>,
    pub review_by: Option<String>,
    pub proposed_at: String,
}

#[derive(Clone, Default)]
pub(crate) struct State {
    pub records: BTreeMap<String, Vec<Record>>,
    pub next_decision: u64,
    pub next_pending: u64,
}

impl State {
    pub fn get(&self, id: &str, rev: u64) -> Option<&Record> {
        self.records.get(id)?.iter().find(|r| r.rev == rev)
    }
    pub fn get_mut(&mut self, id: &str, rev: u64) -> Option<&mut Record> {
        self.records.get_mut(id)?.iter_mut().find(|r| r.rev == rev)
    }
    pub fn active(&self, id: &str) -> Option<&Record> {
        self.records.get(id)?.iter().find(|r| r.status == "active")
    }
    pub fn latest(&self, id: &str) -> Option<&Record> {
        self.records.get(id)?.iter().max_by_key(|r| r.rev)
    }
    pub fn selected(&self, id: &str, rev: Option<u64>) -> Option<&Record> {
        match rev {
            Some(rev) => self.get(id, rev),
            None => self.active(id).or_else(|| self.latest(id)),
        }
    }
    pub fn record_count(&self) -> usize {
        self.records.values().map(Vec::len).sum()
    }
}

//! Avaliacao de politicas de passos por tarefa e arvore.

use crate::log::Verified;
use serde::Deserialize;
use serde_json::{Value, json};
use std::collections::{BTreeMap, HashSet};
use std::fs;
use std::path::Path;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Policy {
    steps: Vec<PolicyStep>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct PolicyStep {
    name: String,
    kind: Option<String>,
    independent_of: Option<String>,
}

struct StepResult {
    step: String,
    status: &'static str,
    agent: Option<String>,
    seq: Option<u64>,
    stale_tree: bool,
    author: Option<String>,
    has_independent_target: bool,
}

pub struct Report {
    pub task: String,
    pub tree: String,
    pub complete: bool,
    steps: Vec<StepResult>,
}

impl Report {
    pub fn json(&self) -> Value {
        json!({
            "task": self.task,
            "tree": self.tree,
            "complete": self.complete,
            "steps": self.steps.iter().map(|step| {
                let mut row = json!({
                    "step": step.step,
                    "status": step.status,
                    "agent": step.agent,
                    "seq": step.seq,
                    "stale_tree": step.stale_tree,
                });
                if step.has_independent_target {
                    row["author"] = json!(step.author);
                }
                row
            }).collect::<Vec<_>>(),
        })
    }

    pub fn text(&self) -> String {
        let mut output = format!(
            "task={} tree={} complete={}\n",
            self.task, self.tree, self.complete
        );
        for step in &self.steps {
            output.push_str(&format!(
                "{} {} agent={} seq={}{}",
                step.step,
                step.status,
                step.agent.as_deref().unwrap_or("null"),
                step.seq
                    .map_or_else(|| "null".into(), |seq| seq.to_string()),
                if step.stale_tree { " stale_tree" } else { "" },
            ));
            if step.has_independent_target {
                output.push_str(&format!(
                    " author={}",
                    step.author.as_deref().unwrap_or("null")
                ));
            }
            output.push('\n');
        }
        output
    }
}

pub fn evaluate(
    records: &[Verified],
    policy_path: &Path,
    task: &str,
    tree: &str,
) -> Result<Report, String> {
    let bytes = fs::read(policy_path).map_err(|e| format!("nao li politica: {e}"))?;
    let policy: Policy =
        serde_json::from_slice(&bytes).map_err(|_| "politica invalida".to_string())?;
    validate_policy(&policy)?;
    let latest = latest_for(records, task, tree);
    let mut steps = Vec::with_capacity(policy.steps.len());
    for requirement in &policy.steps {
        steps.push(evaluate_step(records, &latest, task, tree, requirement));
    }
    let complete = steps.iter().all(|step| step.status == "ok");
    Ok(Report {
        task: task.to_string(),
        tree: tree.to_string(),
        complete,
        steps,
    })
}

fn validate_policy(policy: &Policy) -> Result<(), String> {
    let mut names = HashSet::new();
    for step in &policy.steps {
        if !valid_step(&step.name) || !names.insert(&step.name) {
            return Err("politica invalida: passo invalido ou repetido".into());
        }
        if step
            .kind
            .as_deref()
            .is_some_and(|kind| kind != "run" && kind != "note")
        {
            return Err("politica invalida: kind deve ser run ou note".into());
        }
        if step
            .independent_of
            .as_deref()
            .is_some_and(|name| !valid_step(name))
        {
            return Err("politica invalida: independent_of invalido".into());
        }
    }
    Ok(())
}

fn latest_for<'a>(
    records: &'a [Verified],
    task: &str,
    tree: &str,
) -> BTreeMap<&'a str, &'a crate::log::Record> {
    let mut latest = BTreeMap::new();
    for verified in records {
        let record = &verified.record;
        if record.task == task && record.tree == tree {
            let should_replace = latest
                .get(record.step.as_str())
                .is_none_or(|prior: &&crate::log::Record| record.seq > prior.seq);
            if should_replace {
                latest.insert(record.step.as_str(), record);
            }
        }
    }
    latest
}

fn evaluate_step(
    records: &[Verified],
    latest: &BTreeMap<&str, &crate::log::Record>,
    task: &str,
    tree: &str,
    requirement: &PolicyStep,
) -> StepResult {
    let current = latest.get(requirement.name.as_str()).copied();
    let stale_tree = records.iter().any(|verified| {
        let record = &verified.record;
        record.task == task
            && record.step == requirement.name
            && record.tree != tree
            && record.result == "pass"
    });
    let mut status = match current {
        None => "missing",
        Some(record) if record.result != "pass" => "failed",
        Some(record)
            if requirement
                .kind
                .as_deref()
                .is_some_and(|kind| kind != record.kind) =>
        {
            "missing"
        }
        Some(_) => "ok",
    };
    let mut author = None;
    let has_independent_target = requirement.independent_of.is_some();
    if status == "ok"
        && let Some(target) = &requirement.independent_of
    {
        let target_record = latest.get(target.as_str()).copied();
        author = target_record.map(|record| record.agent.clone());
        if target_record.is_none_or(|record| {
            record.result != "pass" || current.is_some_and(|own| own.agent == record.agent)
        }) {
            status = "not_independent";
        }
    }
    StepResult {
        step: requirement.name.clone(),
        status,
        agent: current.map(|record| record.agent.clone()),
        seq: current.map(|record| record.seq),
        stale_tree,
        author,
        has_independent_target,
    }
}

fn valid_step(name: &str) -> bool {
    (1..=32).contains(&name.len())
        && name
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || b"_-".contains(&byte))
}

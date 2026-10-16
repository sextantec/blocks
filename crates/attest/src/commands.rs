//! Dispatch dos subcomandos e apresentacao dos resultados.

use crate::cli::{self, Action};
use crate::log::{self, Record, Verified};
use crate::{policy, run_cmd, tree};
use chrono::Utc;
use serde_json::{Value, json};
use std::env;
use std::ffi::OsString;
use std::io::{self, Write};
use std::path::Path;

pub fn execute() -> i32 {
    let args: Vec<OsString> = env::args_os().skip(1).collect();
    let action = match cli::parse(&args) {
        Ok(action) => action,
        Err(message) => return error(2, &message),
    };
    match action {
        Action::Run(args) => run(args),
        Action::Note(args) => note(args),
        Action::Check(args) => check(args),
        Action::Footer {
            state,
            task,
            tree: requested,
        } => footer(&state, &task, &requested),
        Action::Show {
            state,
            task,
            tree: requested,
        } => show(&state, &task, requested.as_deref()),
        Action::Verify { state } => verify(&state),
    }
}

fn run(args: cli::Run) -> i32 {
    let before = match tree::snapshot(Path::new(".")) {
        Ok(snapshot) => snapshot,
        Err(message) => return error(2, &message),
    };
    let outcome = run_cmd::run(&args);
    let after = match tree::snapshot(Path::new(".")) {
        Ok(snapshot) => snapshot,
        Err(message) => return error(2, &message),
    };
    let mut record = base_record(RecordFields {
        kind: "run",
        step: &args.step,
        task: &args.task,
        tree: &after.tree,
        head: after.head,
        agent: &args.agent,
        model: &args.model,
        effort: args.effort.as_deref(),
        authorized_by: &args.authorized_by,
    });
    record.tree_before = Some(before.tree.clone());
    record.mutated = Some(before.tree != after.tree);
    record.result = if outcome.code == 0 { "pass" } else { "fail" }.into();
    record.argv = Some(outcome.argv);
    record.exit = Some(outcome.exit.map_or(Value::Null, |code| json!(code)));
    record.duration_ms = Some(outcome.duration_ms);
    record.output_sha256 = Some(outcome.output_sha256);
    record.output_bytes = Some(outcome.output_bytes);
    if let Err(err) = log::append(&args.state, record) {
        return error(err.code(), err.message());
    }
    outcome.code
}

fn note(args: cli::Note) -> i32 {
    let snapshot = match tree::snapshot(Path::new(".")) {
        Ok(snapshot) => snapshot,
        Err(message) => return error(2, &message),
    };
    let mut record = base_record(RecordFields {
        kind: "note",
        step: &args.step,
        task: &args.task,
        tree: &snapshot.tree,
        head: snapshot.head,
        agent: &args.agent,
        model: &args.model,
        effort: args.effort.as_deref(),
        authorized_by: &args.authorized_by,
    });
    record.result = args.result;
    record.detail = Some(args.detail);
    match log::append(&args.state, record) {
        Ok(seq) => {
            println!("seq={seq}");
            0
        }
        Err(err) => error(err.code(), err.message()),
    }
}

fn check(args: cli::Check) -> i32 {
    let requested_tree = match resolve_tree(&args.tree) {
        Ok(tree) => tree,
        Err(message) => return error(2, &message),
    };
    let records = match log::read(&args.state) {
        Ok(records) => records,
        Err(err) => return error(err.code(), err.message()),
    };
    let report = match policy::evaluate(&records, &args.policy, &args.task, &requested_tree) {
        Ok(report) => report,
        Err(message) => return error(2, &message),
    };
    if args.json {
        println!("{}", report.json());
    } else {
        print!("{}", report.text());
    }
    if report.complete { 0 } else { 3 }
}

fn footer(state: &Path, task: &str, requested_tree: &str) -> i32 {
    let requested_tree = match resolve_tree(requested_tree) {
        Ok(tree) => tree,
        Err(message) => return error(2, &message),
    };
    let records = match log::read(state) {
        Ok(records) => records,
        Err(err) => return error(err.code(), err.message()),
    };
    let mut passes: Vec<&Record> = records
        .iter()
        .map(|verified| &verified.record)
        .filter(|record| {
            record.task == task && record.tree == requested_tree && record.result == "pass"
        })
        .collect();
    passes.sort_by_key(|record| record.seq);
    if passes.is_empty() {
        return error(3, "nenhum registro pass para task/tree");
    }
    let mut agents: Vec<(String, String, Option<String>)> = Vec::new();
    let mut authorizations = Vec::new();
    for record in passes {
        if !agents.iter().any(|(agent, _, _)| agent == &record.agent) {
            agents.push((
                record.agent.clone(),
                record.model.clone(),
                record.effort.clone(),
            ));
        }
        if !authorizations.contains(&record.authorized_by) {
            authorizations.push(record.authorized_by.clone());
        }
    }
    let agent_line = agents
        .iter()
        .map(|(agent, model, effort)| match effort {
            Some(effort) if !effort.is_empty() => format!("{agent} ({model}, {effort})"),
            _ => format!("{agent} ({model})"),
        })
        .collect::<Vec<_>>()
        .join("; ");
    println!("Agente: {agent_line}");
    println!("Autorizado por: {}", authorizations.join(" | "));
    0
}

fn show(state: &Path, task: &str, requested_tree: Option<&str>) -> i32 {
    let tree = match requested_tree.map(resolve_tree).transpose() {
        Ok(tree) => tree,
        Err(message) => return error(2, &message),
    };
    let records = match log::read(state) {
        Ok(records) => records,
        Err(err) => return error(err.code(), err.message()),
    };
    for verified in records.iter().filter(|verified| {
        verified.record.task == task
            && tree
                .as_ref()
                .is_none_or(|tree| &verified.record.tree == tree)
    }) {
        let mut row = match serde_json::to_value(&verified.record) {
            Ok(value) => value,
            Err(err) => return error(4, &format!("nao serializo registro: {err}")),
        };
        truncate_argv(&mut row);
        println!("{row}");
    }
    0
}

fn verify(state: &Path) -> i32 {
    match log::read(state) {
        Ok(records) => {
            println!("ok registros={}", records.len());
            0
        }
        Err(err) => error(err.code(), err.message()),
    }
}

fn resolve_tree(requested: &str) -> Result<String, String> {
    if requested == "auto" {
        return tree::snapshot(Path::new(".")).map(|snapshot| snapshot.tree);
    }
    if requested.is_empty() || requested.bytes().any(|byte| byte.is_ascii_whitespace()) {
        return Err("tree invalida".into());
    }
    tree::ensure_repository(Path::new("."))?;
    Ok(requested.to_string())
}

struct RecordFields<'a> {
    kind: &'a str,
    step: &'a str,
    task: &'a str,
    tree: &'a str,
    head: Option<String>,
    agent: &'a str,
    model: &'a str,
    effort: Option<&'a str>,
    authorized_by: &'a str,
}

fn base_record(fields: RecordFields<'_>) -> Record {
    let mut record = log::empty_record();
    record.ts = Utc::now().format("%Y-%m-%dT%H:%M:%SZ").to_string();
    record.kind = fields.kind.to_string();
    record.step = fields.step.to_string();
    record.task = fields.task.to_string();
    record.tree = fields.tree.to_string();
    record.head = fields.head;
    record.agent = fields.agent.to_string();
    record.model = fields.model.to_string();
    record.effort = fields.effort.map(str::to_string);
    record.authorized_by = fields.authorized_by.to_string();
    record
}

fn truncate_argv(row: &mut Value) {
    let Some(argv) = row.get("argv").and_then(Value::as_array) else {
        return;
    };
    let rendered = argv
        .iter()
        .map(|arg| arg.as_str().unwrap_or(""))
        .collect::<Vec<_>>()
        .join(" ");
    if rendered.chars().count() <= 200 {
        return;
    }
    let short: String = rendered.chars().take(197).collect();
    row["argv"] = json!([format!("{short}...")]);
}

fn error(code: i32, message: &str) -> i32 {
    let _ = writeln!(io::stderr().lock(), "attest: {message}");
    code
}

#[allow(dead_code)]
fn _record_ref(_: &Verified) {}

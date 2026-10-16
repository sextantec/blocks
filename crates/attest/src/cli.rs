//! Parsinga do contrato CLI do attest.

use std::collections::{HashMap, HashSet};
use std::ffi::{OsStr, OsString};
use std::path::PathBuf;

pub enum Action {
    Run(Run),
    Note(Note),
    Check(Check),
    Footer {
        state: PathBuf,
        task: String,
        tree: String,
    },
    Show {
        state: PathBuf,
        task: String,
        tree: Option<String>,
    },
    Verify {
        state: PathBuf,
    },
}

pub struct Run {
    pub state: PathBuf,
    pub step: String,
    pub task: String,
    pub agent: String,
    pub model: String,
    pub effort: Option<String>,
    pub authorized_by: String,
    pub timeout: u64,
    pub argv: Vec<OsString>,
}

pub struct Note {
    pub state: PathBuf,
    pub step: String,
    pub task: String,
    pub agent: String,
    pub model: String,
    pub effort: Option<String>,
    pub authorized_by: String,
    pub result: String,
    pub detail: String,
}

pub struct Check {
    pub state: PathBuf,
    pub tree: String,
    pub policy: PathBuf,
    pub task: String,
    pub json: bool,
}

pub fn parse(args: &[OsString]) -> Result<Action, String> {
    let (subcommand, rest) = args.split_first().ok_or("falta subcomando")?;
    let subcommand = text(subcommand)?;
    match subcommand.as_str() {
        "run" => parse_run(rest),
        "note" => parse_note(rest),
        "check" => parse_check(rest),
        "footer" => parse_footer(rest),
        "show" => parse_show(rest),
        "verify" => {
            let (opts, flags) = parse_options(rest, &["state-dir"], &[])?;
            no_flags(&flags)?;
            Ok(Action::Verify {
                state: state_dir(&opts)?,
            })
        }
        other => Err(format!("subcomando desconhecido '{other}'")),
    }
}

fn parse_run(args: &[OsString]) -> Result<Action, String> {
    let separator = args
        .iter()
        .position(|arg| arg == OsStr::new("--"))
        .ok_or("run exige -- antes do comando")?;
    let argv = args[separator + 1..].to_vec();
    if argv.is_empty() {
        return Err("run exige comando depois de --".into());
    }
    let (opts, flags) = parse_options(
        &args[..separator],
        &[
            "state-dir",
            "step",
            "task",
            "agent",
            "model",
            "effort",
            "authorized-by",
            "timeout",
        ],
        &[],
    )?;
    no_flags(&flags)?;
    let fields = Metadata::from(&opts)?;
    Ok(Action::Run(Run {
        state: state_dir(&opts)?,
        step: fields.step,
        task: fields.task,
        agent: fields.agent,
        model: fields.model,
        effort: fields.effort,
        authorized_by: fields.authorized_by,
        timeout: parse_timeout(opts.get("timeout"))?,
        argv,
    }))
}

fn parse_note(args: &[OsString]) -> Result<Action, String> {
    let (opts, flags) = parse_options(
        args,
        &[
            "state-dir",
            "step",
            "task",
            "agent",
            "model",
            "effort",
            "authorized-by",
            "result",
            "detail",
        ],
        &[],
    )?;
    no_flags(&flags)?;
    let fields = Metadata::from(&opts)?;
    let result = required(&opts, "result")?;
    if result != "pass" && result != "fail" {
        return Err("--result deve ser pass ou fail".into());
    }
    let detail = required(&opts, "detail")?;
    if detail.chars().count() > 280 || detail.contains(['\n', '\r']) {
        return Err("--detail aceita ate 280 caracteres e nao pode ter quebra de linha".into());
    }
    Ok(Action::Note(Note {
        state: state_dir(&opts)?,
        step: fields.step,
        task: fields.task,
        agent: fields.agent,
        model: fields.model,
        effort: fields.effort,
        authorized_by: fields.authorized_by,
        result,
        detail,
    }))
}

fn parse_check(args: &[OsString]) -> Result<Action, String> {
    let (opts, flags) = parse_options(args, &["state-dir", "tree", "policy", "task"], &["json"])?;
    let tree = required(&opts, "tree")?;
    let policy = PathBuf::from(required(&opts, "policy")?);
    let task = required(&opts, "task")?;
    validate_task(&task)?;
    Ok(Action::Check(Check {
        state: state_dir(&opts)?,
        tree,
        policy,
        task,
        json: flags.contains("json"),
    }))
}

fn parse_footer(args: &[OsString]) -> Result<Action, String> {
    let (opts, flags) = parse_options(args, &["state-dir", "task", "tree"], &[])?;
    no_flags(&flags)?;
    let task = required(&opts, "task")?;
    validate_task(&task)?;
    Ok(Action::Footer {
        state: state_dir(&opts)?,
        task,
        tree: required(&opts, "tree")?,
    })
}

fn parse_show(args: &[OsString]) -> Result<Action, String> {
    let (opts, flags) = parse_options(args, &["state-dir", "task", "tree"], &[])?;
    no_flags(&flags)?;
    let task = required(&opts, "task")?;
    validate_task(&task)?;
    Ok(Action::Show {
        state: state_dir(&opts)?,
        task,
        tree: opts.get("tree").cloned(),
    })
}

struct Metadata {
    step: String,
    task: String,
    agent: String,
    model: String,
    effort: Option<String>,
    authorized_by: String,
}

impl Metadata {
    fn from(opts: &HashMap<String, String>) -> Result<Self, String> {
        let step = required(opts, "step")?;
        let task = required(opts, "task")?;
        let agent = required(opts, "agent")?;
        let model = required(opts, "model")?;
        let authorized_by = required(opts, "authorized-by")?;
        if !matches_pattern(&step, 1, 32, |byte| {
            byte.is_ascii_lowercase() || byte.is_ascii_digit() || b"_-".contains(&byte)
        }) {
            return Err("--step invalido".into());
        }
        validate_task(&task)?;
        if !matches_pattern(&agent, 1, 32, |byte| {
            byte.is_ascii_lowercase() || byte.is_ascii_digit() || b"_-".contains(&byte)
        }) {
            return Err("--agent invalido".into());
        }
        if model.is_empty() {
            return Err("--model nao pode ser vazio".into());
        }
        if authorized_by.chars().count() > 280 || authorized_by.contains(['\n', '\r']) {
            return Err("--authorized-by aceita ate 280 caracteres sem quebras".into());
        }
        Ok(Self {
            step,
            task,
            agent,
            model,
            effort: opts.get("effort").cloned(),
            authorized_by,
        })
    }
}

fn parse_options(
    args: &[OsString],
    value_options: &[&str],
    bool_options: &[&str],
) -> Result<(HashMap<String, String>, HashSet<String>), String> {
    let mut values = HashMap::new();
    let mut flags = HashSet::new();
    let mut index = 0;
    while index < args.len() {
        let raw = text(&args[index])?;
        let name_value = raw
            .strip_prefix("--")
            .ok_or_else(|| format!("argumento inesperado '{raw}'"))?;
        let (name, inline) = name_value
            .split_once('=')
            .map_or((name_value, None), |(name, value)| {
                (name, Some(value.to_string()))
            });
        if bool_options.contains(&name) {
            if inline.is_some() || !flags.insert(name.to_string()) {
                return Err(format!("--{name} repetido ou invalido"));
            }
        } else if value_options.contains(&name) {
            let value = match inline {
                Some(value) => value,
                None => {
                    index += 1;
                    text(
                        args.get(index)
                            .ok_or_else(|| format!("--{name} exige valor"))?,
                    )?
                }
            };
            if values.insert(name.to_string(), value).is_some() {
                return Err(format!("--{name} repetido"));
            }
        } else {
            return Err(format!("opcao desconhecida '--{name}'"));
        }
        index += 1;
    }
    Ok((values, flags))
}

fn state_dir(opts: &HashMap<String, String>) -> Result<PathBuf, String> {
    if let Some(path) = opts.get("state-dir") {
        return Ok(PathBuf::from(path));
    }
    if let Some(path) = std::env::var_os("ATTEST_STATE_DIR") {
        return Ok(PathBuf::from(path));
    }
    let home = std::env::var_os("HOME").ok_or("HOME nao definido")?;
    Ok(PathBuf::from(home).join(".local/state/attest"))
}

fn required(opts: &HashMap<String, String>, name: &str) -> Result<String, String> {
    opts.get(name)
        .cloned()
        .ok_or_else(|| format!("falta --{name}"))
}

fn validate_task(task: &str) -> Result<(), String> {
    if matches_pattern(task, 1, 64, |byte| {
        byte.is_ascii_alphanumeric() || b"._-".contains(&byte)
    }) {
        Ok(())
    } else {
        Err("--task invalido".into())
    }
}

fn matches_pattern(value: &str, min: usize, max: usize, allowed: impl Fn(u8) -> bool) -> bool {
    (min..=max).contains(&value.len()) && value.bytes().all(allowed)
}

fn parse_timeout(raw: Option<&String>) -> Result<u64, String> {
    raw.map_or(Ok(900), |value| {
        value
            .parse::<u64>()
            .ok()
            .filter(|seconds| *seconds > 0)
            .ok_or_else(|| "--timeout deve ser inteiro positivo".into())
    })
}

fn no_flags(flags: &HashSet<String>) -> Result<(), String> {
    if flags.is_empty() {
        Ok(())
    } else {
        Err("flag nao aceita neste subcomando".into())
    }
}

fn text(value: &OsStr) -> Result<String, String> {
    value
        .to_str()
        .map(str::to_owned)
        .ok_or_else(|| "argumento CLI deve ser UTF-8".into())
}

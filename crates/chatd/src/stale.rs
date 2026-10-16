//! Verificador offline de mensagens sem aceite ou resultado.

use crate::{Failure, Opts, parse_opts, usage};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize, de::IgnoredAny};
use std::collections::{BTreeMap, HashMap, HashSet};
use std::fs::{self, File, OpenOptions};
use std::io::{BufRead, BufReader, Write};
use std::os::fd::AsRawFd;
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};

#[derive(Debug, Deserialize)]
struct Event {
    #[serde(rename = "type")]
    event_type: String,
    ts: String,
    seq: u64,
    id: Option<String>,
    #[serde(rename = "from")]
    _from: Option<String>,
    to: Option<String>,
    kind: Option<String>,
    recipient: Option<String>,
    due_s: Option<u64>,
    #[serde(flatten)]
    _ignored: BTreeMap<String, IgnoredAny>,
}

#[derive(Clone)]
struct Message {
    seq: u64,
    id: String,
    to: String,
    kind: String,
    registered: DateTime<Utc>,
    accepted: Option<DateTime<Utc>>,
    resulted: bool,
    due_at: Option<DateTime<Utc>>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct Episode {
    episode_first_seq: u64,
    last_alert_ts: String,
}

#[derive(Default, Serialize, Deserialize)]
struct Stamp {
    #[serde(default)]
    version: u32,
    #[serde(default)]
    scheduler_last_run: Option<String>,
    #[serde(default)]
    unaccepted: BTreeMap<String, Episode>,
    #[serde(default)]
    no_result: BTreeMap<String, Episode>,
    #[serde(default)]
    due_expired: BTreeMap<String, Episode>,
}

#[derive(Serialize)]
struct Alert {
    name: String,
    #[serde(rename = "type")]
    alert_type: &'static str,
    count: usize,
    ids: Vec<String>,
    oldest_min: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    bells: Option<usize>,
    new_alert: bool,
}

struct Options {
    older_than: u64,
    bells: usize,
    result_older_than: u64,
    sinks: Vec<String>,
    tmux_bin: String,
    tmux_socket: Option<String>,
    stamp_path: Option<PathBuf>,
    json: bool,
}

struct LogData {
    messages: HashMap<String, Message>,
    bells: BellEvents,
    skipped: usize,
}

pub fn run(args: &[String]) -> Result<(), Failure> {
    let opts = parse_stale_options(args)?;
    let cli = options(&opts)?;
    let state_dir = opts
        .get("state-dir")
        .map(PathBuf::from)
        .or_else(|| crate::env_path("CHATD_STATE_DIR"))
        .ok_or_else(|| usage("stale exige --state-dir ou CHATD_STATE_DIR"))?;
    let log_path = state_dir.join("log.jsonl");
    let log = File::open(&log_path).map_err(|e| unavailable(&log_path, e))?;
    let data = read_log(log)?;
    let now = Utc::now();
    let stamp_path = cli
        .stamp_path
        .clone()
        .unwrap_or_else(|| state_dir.join("stale.stamp"));
    let _lock = stamp_lock(&stamp_path)?;
    let (mut alerts, active_unaccepted, active_results, active_due) =
        detect_alerts(&data, &cli, now);
    let mut stamp = load_stamp(Some(&stamp_path))?;
    stamp.version = 1;
    stamp.scheduler_last_run = Some(now.to_rfc3339());
    prune_stamp(&mut stamp, &active_unaccepted, &active_results, &active_due);
    mark_new_alerts(&mut alerts, &mut stamp, &data.messages, &cli, now);
    write_stamp(&stamp_path, &stamp)?;
    let roles = state_dir
        .parent()
        .and_then(|p| crate::config::load(&p.join("config.json")).ok())
        .and_then(|c| c.roles);
    if let Some(roles) = roles {
        for alert in &mut alerts {
            if alert.name == roles.orchestrator {
                alert.name = roles.owner.clone();
            }
        }
    }
    let output = render_alerts(&alerts, cli.json)?;
    deliver_sinks(&cli, &alerts, data.skipped, &output)?;
    if alerts.iter().any(|a| a.new_alert) {
        Err(Failure::StaleAlerts)
    } else {
        Ok(())
    }
}

fn deliver_sinks(
    cli: &Options,
    alerts: &[Alert],
    initial_skipped: usize,
    output: &str,
) -> Result<(), Failure> {
    let tmux_entries: Vec<_> = alerts
        .iter()
        .map(|alert| crate::sink_tmux::Entry {
            recipient: alert.name.clone(),
            kind: alert.alert_type,
            count: alert.count,
            minutes: alert.oldest_min,
        })
        .collect();
    let new_lines: Vec<String> = alerts
        .iter()
        .filter(|alert| alert.new_alert)
        .map(format_alert)
        .collect();
    let mut skipped = initial_skipped;
    for sink in &cli.sinks {
        if sink == "tmux" {
            skipped += crate::sink_tmux::run(
                &cli.tmux_bin,
                cli.tmux_socket.as_deref(),
                &tmux_entries,
                &new_lines,
            );
        } else {
            write_sink(sink, output.as_bytes())?;
        }
    }
    if skipped > 0 {
        eprintln!("skipped={skipped}");
    }
    Ok(())
}

fn parse_stale_options(args: &[String]) -> Result<Opts, Failure> {
    parse_opts(
        args,
        &[
            "state-dir",
            "older-than",
            "bells",
            "result-older-than",
            "notify",
            "tmux-bin",
            "tmux-socket",
            "stamp-file",
        ],
        &["json"],
    )
}

fn load_stamp(path: Option<&Path>) -> Result<Stamp, Failure> {
    match path {
        Some(path) => read_stamp(path),
        None => Ok(Stamp::default()),
    }
}

fn options(opts: &Opts) -> Result<Options, Failure> {
    let mut sinks = opts.all("notify");
    if sinks.is_empty() {
        sinks.push("stdout".to_string());
    }
    if sinks.len() > 3 {
        return Err(usage("--notify aceita no maximo tres sinks"));
    }
    let mut unique = HashSet::new();
    for sink in &sinks {
        if !unique.insert(sink) {
            return Err(usage("--notify repetido"));
        }
        if sink != "stdout" && sink != "tmux" && !sink.starts_with("file:") {
            return Err(usage("--notify deve ser stdout, file:CAMINHO ou tmux"));
        }
        if sink == "file:" {
            return Err(usage("--notify file: exige caminho"));
        }
    }
    let has_tmux = sinks.iter().any(|sink| sink == "tmux");
    if !has_tmux && (opts.get("tmux-bin").is_some() || opts.get("tmux-socket").is_some()) {
        return Err(usage("--tmux-bin e --tmux-socket exigem --notify tmux"));
    }
    let tmux_bin = opts.get("tmux-bin").unwrap_or("tmux").to_string();
    if tmux_bin.is_empty() {
        return Err(usage("--tmux-bin exige caminho"));
    }
    let tmux_socket = opts.get("tmux-socket").map(str::to_string);
    if tmux_socket.as_ref().is_some_and(String::is_empty) {
        return Err(usage("--tmux-socket exige caminho"));
    }
    Ok(Options {
        older_than: number(opts, "older-than", 900)?,
        bells: number(opts, "bells", 2)? as usize,
        result_older_than: number(opts, "result-older-than", 3600)?,
        sinks,
        tmux_bin,
        tmux_socket,
        stamp_path: opts.get("stamp-file").map(PathBuf::from),
        json: opts.flag("json"),
    })
}

fn detect_alerts(
    data: &LogData,
    options: &Options,
    now: DateTime<Utc>,
) -> (
    Vec<Alert>,
    HashSet<String>,
    HashSet<String>,
    HashSet<String>,
) {
    let mut due_groups: BTreeMap<&str, Vec<&Message>> = BTreeMap::new();
    let mut pending_groups: BTreeMap<&str, Vec<&Message>> = BTreeMap::new();
    let mut result_groups: BTreeMap<&str, Vec<&Message>> = BTreeMap::new();
    for msg in data.messages.values().filter(|m| !m.resulted) {
        if msg.due_at.is_some_and(|at| now >= at) {
            due_groups.entry(&msg.to).or_default().push(msg);
        } else if msg.accepted.is_none() {
            pending_groups.entry(&msg.to).or_default().push(msg);
        } else if matches!(msg.kind.as_str(), "PEDIDO" | "PERGUNTA")
            && msg
                .accepted
                .is_some_and(|at| age_seconds(now, at) as u64 > options.result_older_than)
        {
            result_groups.entry(&msg.to).or_default().push(msg);
        }
    }
    let mut alerts = Vec::new();
    let mut active_unaccepted = HashSet::new();
    for (name, msgs) in pending_groups {
        if let Some(alert) = unaccepted_alert(name, &msgs, data, options, now) {
            active_unaccepted.insert(name.to_string());
            alerts.push(alert);
        }
    }
    let mut active_results = HashSet::new();
    for (name, msgs) in result_groups {
        let oldest = msgs
            .iter()
            .filter_map(|m| m.accepted.map(|at| (m, at)))
            .min_by_key(|(_, at)| *at);
        if let Some((oldest_msg, at)) = oldest {
            active_results.insert(name.to_string());
            alerts.push(make_alert(
                name,
                "no_result",
                &msgs,
                age_seconds(now, at) / 60,
                None,
            ));
            let _ = oldest_msg;
        }
    }
    let mut active_due = HashSet::new();
    for (name, msgs) in due_groups {
        active_due.insert(name.to_string());
        let oldest = msgs
            .iter()
            .filter_map(|m| m.due_at.map(|at| (m, at)))
            .min_by_key(|(_, at)| *at);
        if let Some((_, at)) = oldest {
            alerts.push(make_alert(
                name,
                "due_expired",
                &msgs,
                age_seconds(now, at) / 60,
                None,
            ));
        }
    }
    (alerts, active_unaccepted, active_results, active_due)
}

fn unaccepted_alert(
    name: &str,
    msgs: &[&Message],
    data: &LogData,
    options: &Options,
    now: DateTime<Utc>,
) -> Option<Alert> {
    let oldest = msgs.iter().min_by_key(|m| m.seq)?;
    let oldest_age = age_seconds(now, oldest.registered);
    let bell_count = data
        .bells
        .get(name)
        .into_iter()
        .flatten()
        .filter(|seq| **seq > oldest.seq)
        .count();
    if oldest_age as u64 <= options.older_than && bell_count < options.bells {
        return None;
    }
    Some(Alert {
        name: name.to_string(),
        alert_type: "unaccepted",
        count: msgs.len(),
        ids: sorted_ids(msgs),
        oldest_min: oldest_age / 60,
        bells: Some(bell_count),
        new_alert: false,
    })
}

fn sorted_ids(msgs: &[&Message]) -> Vec<String> {
    let mut rows = msgs.to_vec();
    rows.sort_by_key(|m| m.seq);
    rows.into_iter().map(|m| m.id.clone()).collect()
}

fn make_alert(
    name: &str,
    alert_type: &'static str,
    msgs: &[&Message],
    oldest_min: i64,
    bells: Option<usize>,
) -> Alert {
    Alert {
        name: name.to_string(),
        alert_type,
        count: msgs.len(),
        ids: sorted_ids(msgs),
        oldest_min,
        bells,
        new_alert: false,
    }
}

fn prune_stamp(
    stamp: &mut Stamp,
    unaccepted: &HashSet<String>,
    results: &HashSet<String>,
    due: &HashSet<String>,
) {
    stamp.unaccepted.retain(|name, _| unaccepted.contains(name));
    stamp.no_result.retain(|name, _| results.contains(name));
    stamp.due_expired.retain(|name, _| due.contains(name));
}

fn mark_new_alerts(
    alerts: &mut [Alert],
    stamp: &mut Stamp,
    messages: &HashMap<String, Message>,
    options: &Options,
    now: DateTime<Utc>,
) {
    for alert in alerts {
        let threshold = match alert.alert_type {
            "unaccepted" => options.older_than,
            "no_result" => options.result_older_than,
            _ => options.older_than,
        };
        let first_seq = episode_first_seq(alert, messages);
        let entries = match alert.alert_type {
            "unaccepted" => &mut stamp.unaccepted,
            "no_result" => &mut stamp.no_result,
            _ => &mut stamp.due_expired,
        };
        let due = entries.get(&alert.name).is_none_or(|episode| {
            episode.episode_first_seq != first_seq
                || DateTime::parse_from_rfc3339(&episode.last_alert_ts)
                    .map(|at| {
                        now.signed_duration_since(at.with_timezone(&Utc))
                            .num_seconds()
                            >= threshold as i64
                    })
                    .unwrap_or(true)
        });
        alert.new_alert = due;
        if due {
            entries.insert(
                alert.name.clone(),
                Episode {
                    episode_first_seq: first_seq,
                    last_alert_ts: now.to_rfc3339(),
                },
            );
        }
    }
}

fn render_alerts(alerts: &[Alert], json: bool) -> Result<String, Failure> {
    let mut output = String::new();
    for alert in alerts {
        let line = if json {
            serde_json::to_string(alert).map_err(|e| Failure::Run(e.to_string()))?
        } else if alert.new_alert {
            format_alert(alert)
        } else {
            continue;
        };
        output.push_str(&line);
        output.push('\n');
    }
    Ok(output)
}

fn format_alert(alert: &Alert) -> String {
    if alert.alert_type == "due_expired" {
        format!(
            "[chat] due_expired: {} {} mensagem(ns): {}",
            alert.name,
            alert.count,
            alert.ids.join(" ")
        )
    } else if alert.alert_type == "unaccepted" {
        format!(
            "[chat] parado: {} {} pendente(s), mais antiga {} min, campainha {} vez(es)",
            alert.name,
            alert.count,
            alert.oldest_min,
            alert.bells.unwrap_or(0)
        )
    } else {
        format!(
            "[chat] sem resultado: {} {} pedido(s), mais antigo {} min",
            alert.name, alert.count, alert.oldest_min
        )
    }
}

fn number(opts: &Opts, name: &str, default: u64) -> Result<u64, Failure> {
    let Some(value) = opts.get(name) else {
        return Ok(default);
    };
    value
        .parse::<u64>()
        .ok()
        .filter(|n| *n > 0 && *n <= i64::MAX as u64)
        .ok_or_else(|| usage(format!("--{name} deve ser inteiro positivo")))
}

type BellEvents = HashMap<String, Vec<u64>>;

fn read_log(file: File) -> Result<LogData, Failure> {
    let mut messages: HashMap<String, Message> = HashMap::new();
    let mut bells: BellEvents = HashMap::new();
    let mut skipped = 0;
    for line in BufReader::new(file).lines() {
        let line = line.map_err(|e| Failure::Unavailable(format!("falha lendo log: {e}")))?;
        if line.trim().is_empty() {
            continue;
        }
        let Some((event, ts)) = parse_log_line(&line) else {
            skipped += 1;
            continue;
        };
        if !apply_event(event, ts, &mut messages, &mut bells) {
            skipped += 1;
        }
    }
    Ok(LogData {
        messages,
        bells,
        skipped,
    })
}

fn parse_log_line(line: &str) -> Option<(Event, DateTime<Utc>)> {
    let event: Event = serde_json::from_str(line).ok()?;
    let ts = DateTime::parse_from_rfc3339(&event.ts)
        .ok()?
        .with_timezone(&Utc);
    Some((event, ts))
}

fn apply_event(
    event: Event,
    ts: DateTime<Utc>,
    messages: &mut HashMap<String, Message>,
    bells: &mut BellEvents,
) -> bool {
    match event.event_type.as_str() {
        "registered" => {
            let (Some(id), Some(to), Some(kind)) = (event.id, event.to, event.kind) else {
                return false;
            };
            messages.insert(
                id.clone(),
                Message {
                    seq: event.seq,
                    id: id.clone(),
                    to,
                    kind,
                    registered: ts,
                    accepted: None,
                    resulted: false,
                    due_at: event
                        .due_s
                        .and_then(|s| ts.checked_add_signed(chrono::TimeDelta::seconds(s as i64))),
                },
            );
            true
        }
        "accepted" => {
            if let Some(message) = event.id.and_then(|id| messages.get_mut(&id)) {
                message.accepted = Some(ts);
                true
            } else {
                false
            }
        }
        "resulted" => {
            if let Some(message) = event.id.and_then(|id| messages.get_mut(&id)) {
                message.resulted = true;
                true
            } else {
                false
            }
        }
        "bell_sent" => {
            if let Some(name) = event.recipient {
                bells.entry(name).or_default().push(event.seq);
                true
            } else {
                false
            }
        }
        _ => true,
    }
}

fn age_seconds(now: DateTime<Utc>, then: DateTime<Utc>) -> i64 {
    now.signed_duration_since(then).num_seconds().max(0)
}

fn episode_first_seq(alert: &Alert, messages: &HashMap<String, Message>) -> u64 {
    messages
        .values()
        .filter(|m| m.to == alert.name)
        .filter(|m| {
            if alert.alert_type == "due_expired" {
                m.due_at.is_some()
            } else if alert.alert_type == "unaccepted" {
                m.accepted.is_none() && !m.resulted
            } else {
                matches!(m.kind.as_str(), "PEDIDO" | "PERGUNTA")
                    && m.accepted.is_some()
                    && !m.resulted
            }
        })
        .map(|m| m.seq)
        .min()
        .unwrap_or(0)
}

fn stamp_lock(path: &Path) -> Result<File, Failure> {
    let parent = path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    fs::create_dir_all(parent)
        .map_err(|e| usage(format!("nao criei diretorio do stamp-file: {e}")))?;
    let lock_path = path.with_extension("lock");
    let file = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .mode(0o600)
        .open(lock_path)
        .map_err(|e| usage(format!("nao abri trava do stamp-file: {e}")))?;
    // SAFETY: o descritor permanece vivo durante a secao critica desta execucao.
    if unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX) } != 0 {
        return Err(usage("nao travei stamp-file"));
    }
    Ok(file)
}

fn read_stamp(path: &Path) -> Result<Stamp, Failure> {
    match fs::symlink_metadata(path) {
        Ok(meta) if meta.file_type().is_symlink() => {
            return Err(usage("--stamp-file nao pode ser link simbolico"));
        }
        Ok(meta) if !meta.is_file() => return Err(usage("--stamp-file deve ser arquivo regular")),
        Ok(_) => {}
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Stamp::default()),
        Err(e) => return Err(usage(format!("nao li --stamp-file: {e}"))),
    }
    let bytes = fs::read(path).map_err(|e| usage(format!("nao li --stamp-file: {e}")))?;
    serde_json::from_slice(&bytes).map_err(|_| usage("--stamp-file invalido"))
}

fn write_stamp(path: &Path, stamp: &Stamp) -> Result<(), Failure> {
    if let Ok(meta) = fs::symlink_metadata(path) {
        if meta.file_type().is_symlink() {
            return Err(usage("--stamp-file nao pode ser link simbolico"));
        }
        if !meta.is_file() {
            return Err(usage("--stamp-file deve ser arquivo regular"));
        }
    }
    let parent = path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    fs::create_dir_all(parent)
        .map_err(|e| usage(format!("nao criei diretorio do stamp-file: {e}")))?;
    let tmp = parent.join(format!(
        ".chatd-stale-{}-{}.tmp",
        std::process::id(),
        Utc::now().timestamp_nanos_opt().unwrap_or_default()
    ));
    let bytes = serde_json::to_vec(stamp).map_err(|e| Failure::Run(e.to_string()))?;
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(&tmp)
        .map_err(|e| usage(format!("nao criei stamp-file temporario: {e}")))?;
    let write_result = file.write_all(&bytes).and_then(|()| file.sync_all());
    if let Err(e) = write_result {
        let _ = fs::remove_file(&tmp);
        return Err(usage(format!("nao gravei stamp-file: {e}")));
    }
    drop(file);
    if let Ok(meta) = fs::symlink_metadata(path)
        && meta.file_type().is_symlink()
    {
        let _ = fs::remove_file(&tmp);
        return Err(usage("--stamp-file nao pode ser link simbolico"));
    }
    fs::rename(&tmp, path).map_err(|e| {
        let _ = fs::remove_file(&tmp);
        usage(format!("nao atualizei stamp-file: {e}"))
    })?;
    fs::set_permissions(path, fs::Permissions::from_mode(0o600))
        .map_err(|e| usage(format!("nao ajustei stamp-file para 0600: {e}")))?;
    Ok(())
}

fn write_sink(sink: &str, bytes: &[u8]) -> Result<(), Failure> {
    if sink == "stdout" {
        use std::io::Write;
        std::io::stdout()
            .lock()
            .write_all(bytes)
            .map_err(|e| Failure::Run(format!("nao escrevi saida: {e}")))
    } else {
        let path = Path::new(sink.strip_prefix("file:").expect("validated sink"));
        let mut file = OpenOptions::new()
            .create(true)
            .append(true)
            .mode(0o600)
            .open(path)
            .map_err(|e| Failure::Run(format!("nao abri sink: {e}")))?;
        file.set_permissions(fs::Permissions::from_mode(0o600))
            .map_err(|e| Failure::Run(format!("nao ajustei sink para 0600: {e}")))?;
        file.write_all(bytes)
            .map_err(|e| Failure::Run(format!("nao escrevi sink: {e}")))
    }
}

fn unavailable(path: &Path, error: std::io::Error) -> Failure {
    Failure::Unavailable(format!(
        "log ausente ou ilegivel em {}: {error}",
        path.display()
    ))
}

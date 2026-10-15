//! Verificador offline de mensagens sem aceite ou resultado.

use crate::{Failure, Opts, parse_opts, usage};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize, de::IgnoredAny};
use std::collections::{BTreeMap, HashMap, HashSet};
use std::fs::{self, File, OpenOptions};
use std::io::{BufRead, BufReader, Write};
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
    #[serde(flatten)]
    _ignored: BTreeMap<String, IgnoredAny>,
}

#[derive(Clone)]
struct Message {
    seq: u64,
    to: String,
    kind: String,
    registered: DateTime<Utc>,
    accepted: Option<DateTime<Utc>>,
    resulted: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct Episode {
    episode_first_seq: u64,
    last_alert_ts: String,
}

#[derive(Default, Serialize, Deserialize)]
struct Stamp {
    #[serde(default)]
    unaccepted: BTreeMap<String, Episode>,
    #[serde(default)]
    no_result: BTreeMap<String, Episode>,
}

#[derive(Serialize)]
struct Alert {
    name: String,
    #[serde(rename = "type")]
    alert_type: &'static str,
    count: usize,
    oldest_min: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    bells: Option<usize>,
    new_alert: bool,
}

struct Options {
    older_than: u64,
    bells: usize,
    result_older_than: u64,
    sink: String,
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
    let sink = cli.sink.as_str();
    let state_dir = opts
        .get("state-dir")
        .map(PathBuf::from)
        .or_else(|| crate::env_path("CHATD_STATE_DIR"))
        .ok_or_else(|| usage("stale exige --state-dir ou CHATD_STATE_DIR"))?;
    let log_path = state_dir.join("log.jsonl");
    let log = File::open(&log_path).map_err(|e| unavailable(&log_path, e))?;
    let data = read_log(log)?;
    if data.skipped > 0 {
        eprintln!("skipped={}", data.skipped);
    }
    let now = Utc::now();
    let (mut alerts, active_unaccepted, active_results) = detect_alerts(&data, &cli, now);
    let mut stamp = load_stamp(cli.stamp_path.as_deref())?;
    prune_stamp(&mut stamp, &active_unaccepted, &active_results);
    mark_new_alerts(&mut alerts, &mut stamp, &data.messages, &cli, now);
    if let Some(path) = cli.stamp_path.as_deref() {
        write_stamp(path, &stamp)?;
    }
    let output = render_alerts(&alerts, cli.json)?;
    write_sink(sink, output.as_bytes())?;
    if alerts.iter().any(|a| a.new_alert) {
        Err(Failure::StaleAlerts)
    } else {
        Ok(())
    }
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
    let sink = opts.get("notify").unwrap_or("stdout");
    if sink == "tmux" {
        return Err(usage("--notify tmux ainda nao e suportado"));
    }
    if sink != "stdout" && !sink.starts_with("file:") {
        return Err(usage("--notify deve ser stdout ou file:CAMINHO"));
    }
    if sink == "file:" {
        return Err(usage("--notify file: exige caminho"));
    }
    Ok(Options {
        older_than: number(opts, "older-than", 900)?,
        bells: number(opts, "bells", 2)? as usize,
        result_older_than: number(opts, "result-older-than", 3600)?,
        sink: sink.to_string(),
        stamp_path: opts.get("stamp-file").map(PathBuf::from),
        json: opts.flag("json"),
    })
}

fn detect_alerts(
    data: &LogData,
    options: &Options,
    now: DateTime<Utc>,
) -> (Vec<Alert>, HashSet<String>, HashSet<String>) {
    let pending = data
        .messages
        .values()
        .filter(|m| m.accepted.is_none() && !m.resulted);
    let mut by_recipient: BTreeMap<&str, Vec<&Message>> = BTreeMap::new();
    for msg in pending {
        by_recipient.entry(&msg.to).or_default().push(msg);
    }

    let mut alerts = Vec::new();
    let mut active_unaccepted = HashSet::new();
    for (name, msgs) in by_recipient {
        if let Some(alert) = unaccepted_alert(name, &msgs, data, options, now) {
            active_unaccepted.insert(name.to_string());
            alerts.push(alert);
        }
    }
    let (result_alerts, active_results) = result_alerts(&data.messages, options, now);
    alerts.extend(result_alerts);
    (alerts, active_unaccepted, active_results)
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
        oldest_min: oldest_age / 60,
        bells: Some(bell_count),
        new_alert: false,
    })
}

fn result_alerts(
    messages: &HashMap<String, Message>,
    options: &Options,
    now: DateTime<Utc>,
) -> (Vec<Alert>, HashSet<String>) {
    let mut groups: BTreeMap<&str, Vec<&Message>> = BTreeMap::new();
    for msg in messages.values() {
        if let Some(accepted) = msg.accepted
            && matches!(msg.kind.as_str(), "PEDIDO" | "PERGUNTA")
            && !msg.resulted
            && age_seconds(now, accepted) as u64 > options.result_older_than
        {
            groups.entry(&msg.to).or_default().push(msg);
        }
    }
    let mut active = HashSet::new();
    let alerts = groups
        .into_iter()
        .filter_map(|(name, msgs)| {
            let oldest = msgs.iter().min_by_key(|m| m.accepted)?;
            let age = age_seconds(now, oldest.accepted?);
            active.insert(name.to_string());
            Some(Alert {
                name: name.to_string(),
                alert_type: "no_result",
                count: msgs.len(),
                oldest_min: age / 60,
                bells: None,
                new_alert: false,
            })
        })
        .collect();
    (alerts, active)
}

fn prune_stamp(stamp: &mut Stamp, unaccepted: &HashSet<String>, results: &HashSet<String>) {
    stamp.unaccepted.retain(|name, _| unaccepted.contains(name));
    stamp.no_result.retain(|name, _| results.contains(name));
}

fn mark_new_alerts(
    alerts: &mut [Alert],
    stamp: &mut Stamp,
    messages: &HashMap<String, Message>,
    options: &Options,
    now: DateTime<Utc>,
) {
    for alert in alerts {
        let threshold = if alert.alert_type == "unaccepted" {
            options.older_than
        } else {
            options.result_older_than
        };
        let first_seq = episode_first_seq(alert, messages);
        let entries = if alert.alert_type == "unaccepted" {
            &mut stamp.unaccepted
        } else {
            &mut stamp.no_result
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
    if alert.alert_type == "unaccepted" {
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
                id,
                Message {
                    seq: event.seq,
                    to,
                    kind,
                    registered: ts,
                    accepted: None,
                    resulted: false,
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
            if alert.alert_type == "unaccepted" {
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

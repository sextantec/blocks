use crate::config::read_unique;
use regex::Regex;
use serde_json::{Value, json};
use std::{
    collections::{BTreeMap, HashSet},
    fs,
    os::unix::fs::PermissionsExt,
    path::Path,
};

#[derive(Clone)]
pub struct Rule {
    pub id: String,
    pub state: String,
    pub reason: Option<String>,
    pub all: Vec<Condition>,
    pub none: Vec<Condition>,
    pub fixtures: Vec<String>,
}
#[derive(Clone)]
pub struct MarkRule {
    pub id: String,
    pub mark: String,
    pub all: Vec<Condition>,
    pub fixtures: Vec<String>,
}
#[derive(Clone)]
pub struct Condition {
    region: Region,
    regex: Regex,
    min: usize,
}
#[derive(Clone)]
enum Region {
    Screen,
    Title,
    Tail(usize),
}
#[derive(Clone)]
pub struct InputRule {
    between: Option<Regex>,
    line: Option<Regex>,
    strip_prefix: Option<Regex>,
}
#[derive(Clone)]
pub struct KindRules {
    pub commands: Vec<String>,
    pub rules: Vec<Rule>,
    pub marks: Vec<MarkRule>,
    input: Option<InputRule>,
}
#[derive(Clone)]
pub struct Rules {
    pub kinds: BTreeMap<String, KindRules>,
    raw: Value,
}
#[derive(Clone, Debug)]
pub struct TextLine {
    pub text: String,
    attrs: Vec<(bool, bool)>,
}
#[derive(Clone, Debug)]
pub struct Decision {
    pub state: String,
    pub reason: Option<String>,
    pub rule: Option<String>,
    pub marks: Vec<String>,
    pub input: Option<String>,
}

impl Rules {
    pub fn load(path: &Path) -> Result<Self, String> {
        let meta = fs::metadata(path).map_err(|_| "rules_file ausente".to_string())?;
        if meta.permissions().mode() & 0o022 != 0 {
            return Err("rules_file gravavel por grupo/outros".into());
        }
        let raw = read_unique(path)?;
        let root = obj(&raw, &["schema", "agents", "golden"], "rules")?;
        if root.get("schema").and_then(Value::as_u64) != Some(1) {
            return Err("schema de rules invalido".into());
        }
        let agents = root
            .get("agents")
            .and_then(Value::as_object)
            .ok_or("agents de rules invalido")?;
        let mut kinds = BTreeMap::new();
        let mut ids = HashSet::new();
        for (kind, value) in agents {
            if !["claude", "codex", "pi"].contains(&kind.as_str()) {
                return Err("kind de rules invalido".into());
            }
            let a = obj(
                value,
                &["tested_versions", "identify", "rules", "marks", "input"],
                "agent rules",
            )?;
            let versions = a
                .get("tested_versions")
                .and_then(Value::as_array)
                .ok_or("tested_versions invalido")?;
            if versions.iter().any(|v| v.as_str().is_none()) {
                return Err("tested_versions invalido".into());
            }
            let ident = obj(
                a.get("identify").ok_or("identify ausente")?,
                &["commands"],
                "identify",
            )?;
            let commands = string_array(ident.get("commands"), "commands")?;
            if commands.is_empty() {
                return Err("commands vazio".into());
            }
            let mut rules = Vec::new();
            for r in array(a.get("rules"), "rules")? {
                let o = obj(
                    r,
                    &["id", "state", "reason", "all", "none", "fixtures"],
                    "rule",
                )?;
                let id = id_string(o.get("id"), &mut ids)?;
                let state = str_field(o, "state")?;
                if !["blocked", "working", "idle"].contains(&state.as_str()) {
                    return Err("state de rule invalido".into());
                }
                let reason = o
                    .get("reason")
                    .map(|v| v.as_str().map(str::to_owned).ok_or("reason invalido"))
                    .transpose()?;
                if (state == "blocked"
                    && !matches!(
                        reason.as_deref(),
                        Some("permission" | "user_input" | "unknown")
                    ))
                    || (state != "blocked" && reason.is_some())
                {
                    return Err("reason invalido para state".into());
                }
                let all = conditions(o.get("all"), true)?;
                let none = conditions(o.get("none"), false)?;
                let fixtures = string_array(o.get("fixtures"), "fixtures")?;
                if fixtures.is_empty() {
                    return Err("fixtures vazio".into());
                }
                rules.push(Rule {
                    id,
                    state,
                    reason,
                    all,
                    none,
                    fixtures,
                });
            }
            let mut marks = Vec::new();
            for r in a
                .get("marks")
                .map(|v| array(Some(v), "marks"))
                .transpose()?
                .unwrap_or(&[])
            {
                let o = obj(r, &["id", "mark", "all", "fixtures"], "mark")?;
                let id = id_string(o.get("id"), &mut ids)?;
                let mark = str_field(o, "mark")?;
                if mark != "background" {
                    return Err("mark invalida".into());
                }
                let all = conditions(o.get("all"), true)?;
                let fixtures = string_array(o.get("fixtures"), "fixtures")?;
                if fixtures.is_empty() {
                    return Err("fixtures vazio".into());
                }
                marks.push(MarkRule {
                    id,
                    mark,
                    all,
                    fixtures,
                });
            }
            let input = a.get("input").map(parse_input).transpose()?;
            kinds.insert(
                kind.clone(),
                KindRules {
                    commands,
                    rules,
                    marks,
                    input,
                },
            );
        }
        if kinds.is_empty() {
            return Err("agents de rules vazio".into());
        }
        let golden = array(root.get("golden"), "golden")?;
        for g in golden {
            let go = obj(g, &["fixture", "kind", "expect"], "golden")?;
            if !kinds.contains_key(str_field(go, "kind")?.as_str()) {
                return Err("kind de golden sem rules".into());
            }
            let e = obj(
                g.get("expect").ok_or("expect ausente")?,
                &["state", "reason", "rule", "input", "marks"],
                "expect",
            )?;
            if !e.contains_key("state") || !e.contains_key("rule") || !e.contains_key("marks") {
                return Err("golden incompleto".into());
            }
        }
        Ok(Self { kinds, raw })
    }

    pub fn evaluate(&self, kind: &str, screen: &[TextLine], title: &str) -> Decision {
        let Some(k) = self.kinds.get(kind) else {
            return Decision {
                state: "unknown".into(),
                reason: None,
                rule: None,
                marks: vec!["unmatched".into()],
                input: None,
            };
        };
        let matching: Vec<&Rule> = k
            .rules
            .iter()
            .filter(|r| {
                r.all.iter().all(|c| c.matches(screen, title))
                    && r.none.iter().all(|c| !c.matches(screen, title))
            })
            .collect();
        let winner = matching
            .iter()
            .find(|r| r.state == "blocked")
            .or_else(|| matching.iter().find(|r| r.state == "working"))
            .or_else(|| matching.iter().find(|r| r.state == "idle"));
        let mut marks = Vec::new();
        if winner.is_none() {
            marks.push("unmatched".into());
        }
        marks.extend(
            k.marks
                .iter()
                .filter(|r| r.all.iter().all(|c| c.matches(screen, title)))
                .map(|r| r.mark.clone()),
        );
        marks.sort();
        marks.dedup();
        let state = winner
            .map(|r| r.state.clone())
            .unwrap_or_else(|| "unknown".into());
        let reason = winner
            .filter(|r| r.state == "blocked")
            .and_then(|r| r.reason.clone());
        let rule = winner.map(|r| r.id.clone());
        let input = (state == "idle")
            .then(|| classify_input(k.input.as_ref(), screen))
            .flatten();
        Decision {
            state,
            reason,
            rule,
            marks,
            input,
        }
    }

    pub fn check(&self, fixture_root: &Path) -> Result<(), Vec<String>> {
        let mut failures = Vec::new();
        let goldens = self.raw["golden"].as_array().cloned().unwrap_or_default();
        let mut cases = BTreeMap::<String, (String, Decision)>::new();
        for g in &goldens {
            let fx = g["fixture"].as_str().unwrap_or("");
            let kind = g["kind"].as_str().unwrap_or("");
            match read_fixture(fixture_root, fx) {
                Ok((screen, title, has_ansi)) => {
                    let got = self.evaluate(kind, &screen, &title);
                    let expect = &g["expect"];
                    let mut diffs = Vec::new();
                    if expect["state"] != got.state {
                        diffs.push(format!(
                            "state esperado {}, obtido {}",
                            expect["state"], got.state
                        ));
                    }
                    if expect.get("rule").is_some_and(|v| v != &json!(got.rule)) {
                        diffs.push(format!(
                            "rule esperado {}, obtido {}",
                            expect["rule"],
                            got.rule.as_deref().unwrap_or("null")
                        ));
                    }
                    if expect
                        .get("reason")
                        .is_some_and(|v| v != &json!(got.reason))
                    {
                        diffs.push("reason divergente".into());
                    }
                    if expect.get("input").is_some()
                        && has_ansi
                        && expect["input"] != json!(got.input)
                    {
                        diffs.push("input divergente".into());
                    }
                    let em: HashSet<String> = expect["marks"]
                        .as_array()
                        .into_iter()
                        .flatten()
                        .filter_map(Value::as_str)
                        .map(str::to_owned)
                        .collect();
                    if em != got.marks.iter().cloned().collect() {
                        diffs.push(format!("marks esperado {em:?}, obtido {:?}", got.marks));
                    }
                    if !diffs.is_empty() {
                        failures.push(format!("{fx}: {}", diffs.join(", ")));
                    }
                    cases.insert(fx.into(), (kind.into(), got));
                }
                Err(()) => failures.push(format!("{fx}: fixture ausente ou ilegivel")),
            }
        }
        for (kind, k) in &self.kinds {
            for r in &k.rules {
                let mut matched = false;
                for fx in &r.fixtures {
                    match read_fixture(fixture_root, fx) {
                        Ok((s, t, _)) => {
                            matched |= r.all.iter().all(|c| c.matches(&s, &t))
                                && r.none.iter().all(|c| !c.matches(&s, &t))
                        }
                        Err(()) => failures.push(format!(
                            "{fx}: fixture ausente ou ilegivel (regra {})",
                            r.id
                        )),
                    }
                }
                if !matched {
                    failures.push(format!("regra {}: nenhuma fixture declarada a casa", r.id));
                }
                if !cases.values().any(|(gold_kind, got)| {
                    gold_kind == kind
                        && got.state != r.state
                        && got.rule.as_deref() != Some(r.id.as_str())
                }) {
                    failures.push(format!("regra {}: nenhuma golden de outro estado", r.id));
                }
            }
            for r in &k.marks {
                let mut matched = false;
                for fx in &r.fixtures {
                    match read_fixture(fixture_root, fx) {
                        Ok((s, t, _)) => matched |= r.all.iter().all(|c| c.matches(&s, &t)),
                        Err(()) => failures.push(format!(
                            "{fx}: fixture ausente ou ilegivel (regra {})",
                            r.id
                        )),
                    }
                }
                if !matched {
                    failures.push(format!("regra {}: nenhuma fixture declarada a casa", r.id));
                }
                if !cases
                    .values()
                    .any(|(gold_kind, got)| gold_kind == kind && !got.marks.contains(&r.mark))
                {
                    failures.push(format!("regra {}: nenhuma golden sem a marca", r.id));
                }
            }
        }
        if failures.is_empty() {
            Ok(())
        } else {
            Err(failures)
        }
    }
}

impl Condition {
    fn matches(&self, screen: &[TextLine], title: &str) -> bool {
        let lines: Vec<&str> = match self.region {
            Region::Screen => screen.iter().map(|l| l.text.as_str()).collect(),
            Region::Title => vec![title],
            Region::Tail(n) => screen
                .iter()
                .filter(|l| !l.text.trim().is_empty())
                .rev()
                .take(n)
                .collect::<Vec<_>>()
                .into_iter()
                .rev()
                .map(|l| l.text.as_str())
                .collect(),
        };
        lines.iter().filter(|l| self.regex.is_match(l)).count() >= self.min
    }
}

pub fn parse_screen(raw: &[u8]) -> Vec<TextLine> {
    String::from_utf8_lossy(raw)
        .split('\n')
        .map(|line| parse_line(line.trim_end_matches('\r')))
        .collect()
}
fn parse_line(line: &str) -> TextLine {
    let mut chars = Vec::new();
    let (mut dim, mut reverse) = (false, false);
    let c: Vec<char> = line.chars().collect();
    let mut i = 0;
    while i < c.len() {
        if c[i] == '\u{1b}' {
            if matches!(c.get(i + 1), Some(']') | Some('P') | Some('^') | Some('_')) {
                let mut end = i + 2;
                while end < c.len() {
                    if c[end] == '\u{7}' || (c[end] == '\u{1b}' && c.get(end + 1) == Some(&'\\')) {
                        end += if c[end] == '\u{1b}' { 2 } else { 1 };
                        break;
                    }
                    end += 1;
                }
                i = end;
                continue;
            }
            if c.get(i + 1) == Some(&'[') {
                let mut end = i + 2;
                while end < c.len() && !c[end].is_ascii_alphabetic() {
                    end += 1;
                }
                if end < c.len() {
                    if c[end] == 'm' {
                        let params = c[i + 2..end].iter().collect::<String>();
                        let nums: Vec<u16> = if params.is_empty() {
                            vec![0]
                        } else {
                            params.split(';').map(|x| x.parse().unwrap_or(0)).collect()
                        };
                        let mut p = 0;
                        while p < nums.len() {
                            match nums[p] {
                                0 => {
                                    dim = false;
                                    reverse = false;
                                }
                                2 => dim = true,
                                22 => dim = false,
                                7 => reverse = true,
                                27 => reverse = false,
                                38 | 48 | 58 => {
                                    p += match nums.get(p + 1) {
                                        Some(5) => 2,
                                        Some(2) => 4,
                                        _ => 0,
                                    };
                                }
                                _ => (),
                            }
                            p += 1;
                        }
                    }
                    i = end + 1;
                    continue;
                }
                break;
            }
            let mut end = i + 1;
            while end < c.len() && !('@'..='~').contains(&c[end]) {
                end += 1;
            }
            i = (end + 1).min(c.len());
            continue;
        }
        let ch = if c[i] == '\u{00a0}' { ' ' } else { c[i] };
        chars.push((ch, dim, reverse));
        i += 1;
    }
    while chars.last().is_some_and(|x| x.0 == ' ') {
        chars.pop();
    }
    TextLine {
        text: chars.iter().map(|x| x.0).collect(),
        attrs: chars.iter().map(|x| (x.1, x.2)).collect(),
    }
}
fn classify_input(rule: Option<&InputRule>, screen: &[TextLine]) -> Option<String> {
    let Some(rule) = rule else {
        return Some("unknown".into());
    };
    let selected: Vec<&TextLine> = if let Some(re) = &rule.between {
        let rails: Vec<usize> = screen
            .iter()
            .enumerate()
            .filter(|(_, l)| re.is_match(&l.text))
            .map(|(i, _)| i)
            .collect();
        if rails.len() < 2 {
            return Some("unknown".into());
        }
        screen[rails[rails.len() - 2] + 1..rails[rails.len() - 1]]
            .iter()
            .collect()
    } else if let Some(re) = &rule.line {
        match screen.iter().rev().find(|l| re.is_match(&l.text)) {
            Some(line) => vec![line],
            None => return Some("unknown".into()),
        }
    } else {
        return Some("unknown".into());
    };
    for line in selected {
        let skip = rule
            .strip_prefix
            .as_ref()
            .and_then(|r| {
                r.find(&line.text)
                    .filter(|m| m.start() == 0)
                    .map(|m| m.end())
            })
            .unwrap_or(0);
        for (idx, ch) in line.text.chars().enumerate().skip(skip) {
            if !ch.is_whitespace() && !line.attrs.get(idx).is_some_and(|(dim, rev)| *dim || *rev) {
                return Some("draft".into());
            }
        }
    }
    Some("empty".into())
}

fn read_fixture(root: &Path, name: &str) -> Result<(Vec<TextLine>, String, bool), ()> {
    if name.is_empty() || name.contains("..") || name.starts_with('/') {
        return Err(());
    }
    let raw = fs::read(root.join(format!("{name}.screen"))).map_err(|_| ())?;
    let ansi = raw.windows(2).any(|x| x == b"\x1b[");
    let title = fs::read_to_string(root.join(format!("{name}.title")))
        .unwrap_or_default()
        .trim_end_matches(['\n', '\r'])
        .to_string();
    Ok((parse_screen(&raw), title, ansi))
}

fn parse_input(v: &Value) -> Result<InputRule, String> {
    let o = obj(v, &["between", "line", "strip_prefix"], "input")?;
    if o.contains_key("between") == o.contains_key("line") {
        return Err("input precisa between ou line".into());
    }
    Ok(InputRule {
        between: o.get("between").map(regex_string).transpose()?,
        line: o.get("line").map(regex_string).transpose()?,
        strip_prefix: o.get("strip_prefix").map(regex_string).transpose()?,
    })
}
fn conditions(v: Option<&Value>, required: bool) -> Result<Vec<Condition>, String> {
    let Some(v) = v else {
        return if required {
            Err("all ausente".into())
        } else {
            Ok(Vec::new())
        };
    };
    let a = v.as_array().ok_or("conditions invalido")?;
    if required && a.is_empty() {
        return Err("all vazio".into());
    }
    a.iter()
        .map(|v| {
            let o = obj(v, &["in", "re", "min"], "condition")?;
            let rs = str_field(o, "in")?;
            let region = if rs == "screen" {
                Region::Screen
            } else if rs == "title" {
                Region::Title
            } else if let Some(n) = rs.strip_prefix("tail:") {
                let n = n.parse::<usize>().map_err(|_| "tail invalido")?;
                if !(1..=64).contains(&n) {
                    return Err("tail invalido".into());
                }
                Region::Tail(n)
            } else {
                return Err("region invalida".into());
            };
            let regex = regex_string(o.get("re").ok_or("regex ausente")?)?;
            let min = o
                .get("min")
                .map(|v| v.as_u64().ok_or("min invalido"))
                .transpose()?
                .unwrap_or(1) as usize;
            if min == 0 {
                return Err("min invalido".into());
            }
            Ok(Condition { region, regex, min })
        })
        .collect()
}
fn regex_string(v: &Value) -> Result<Regex, String> {
    Regex::new(v.as_str().ok_or("regex invalida")?).map_err(|_| "regex invalida".into())
}
fn array<'a>(v: Option<&'a Value>, name: &str) -> Result<&'a [Value], String> {
    v.and_then(Value::as_array)
        .map(Vec::as_slice)
        .ok_or_else(|| format!("{name} invalido"))
}
fn string_array(v: Option<&Value>, name: &str) -> Result<Vec<String>, String> {
    array(v, name)?
        .iter()
        .map(|x| {
            x.as_str()
                .map(str::to_owned)
                .ok_or_else(|| format!("{name} invalido"))
        })
        .collect()
}
fn obj<'a>(
    v: &'a Value,
    allowed: &[&str],
    name: &str,
) -> Result<&'a serde_json::Map<String, Value>, String> {
    let o = v.as_object().ok_or_else(|| format!("{name} invalido"))?;
    if o.keys().any(|k| !allowed.contains(&k.as_str())) {
        return Err(format!("campo desconhecido em {name}"));
    }
    Ok(o)
}
fn str_field(o: &serde_json::Map<String, Value>, k: &str) -> Result<String, String> {
    o.get(k)
        .and_then(Value::as_str)
        .map(str::to_owned)
        .ok_or_else(|| format!("{k} invalido"))
}
fn id_string(v: Option<&Value>, ids: &mut HashSet<String>) -> Result<String, String> {
    let id = v.and_then(Value::as_str).ok_or("id invalido")?;
    if id.is_empty()
        || id.len() > 64
        || !id.bytes().all(|b| {
            b.is_ascii_lowercase() || b.is_ascii_digit() || matches!(b, b'.' | b'_' | b'-')
        })
        || !ids.insert(id.into())
    {
        return Err("id de regra invalido ou repetido".into());
    }
    Ok(id.into())
}

//! Configuracao JSON estrita: campos desconhecidos sao recusados (spec v1 7 e v1.1 A).

use std::collections::BTreeMap;
use std::fs;
use std::path::Path;

use serde::Deserialize;

use crate::protocol::valid_name;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Delivery {
    /// Campainha no painel `pane` (adaptador herdr).
    Bell { pane: String },
    /// Campainha no painel validado mais recentemente por `agent.bind`.
    Auto,
    /// Sem campainha: o agente usa `chatd watch`.
    Watch,
}

#[derive(Debug, Clone)]
pub struct Recipient {
    pub delivery: Delivery,
    pub agent: String,
}

impl Recipient {
    pub fn is_bell(&self) -> bool {
        self.delivery.is_bell()
    }
}

impl Delivery {
    pub fn is_bell(&self) -> bool {
        matches!(self, Delivery::Bell { .. } | Delivery::Auto)
    }
}

#[derive(Debug, Clone)]
pub struct Config {
    pub auth_required: bool,
    pub herdr_bin: String,
    pub tmux_bin: String,
    /// Permite digitar a campainha no tmux; inseguro porque nao verifica se o painel esta ocioso.
    pub bell_tmux_typing: bool,
    pub bell_poll_ms: u64,
    pub bell_rearm_s: u64,
    pub recipients: BTreeMap<String, Recipient>,
    pub blocked_receipts: bool,
    pub due_defaults: BTreeMap<String, u64>,
    pub roles: Option<Roles>,
}

#[derive(Debug, Clone)]
pub struct Roles {
    pub orchestrator: String,
    pub owner: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Raw {
    auth: Option<String>,
    herdr_bin: Option<String>,
    tmux_bin: Option<String>,
    bell_tmux_typing: Option<bool>,
    bell_poll_ms: Option<u64>,
    bell_rearm_s: Option<u64>,
    #[serde(default)]
    recipients: BTreeMap<String, RawRecipient>,
    features: Option<RawFeatures>,
    due_defaults: Option<BTreeMap<String, u64>>,
    roles: Option<RawRoles>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawFeatures {
    blocked_receipts: Option<bool>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawRoles {
    orchestrator: String,
    owner: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawRecipient {
    delivery: Option<String>,
    pane: Option<String>,
    agent: Option<String>,
}

pub fn load(path: &Path) -> Result<Config, String> {
    let text =
        fs::read_to_string(path).map_err(|e| format!("config: nao li {}: {e}", path.display()))?;
    parse(&text)
}

pub fn parse(text: &str) -> Result<Config, String> {
    let raw: Raw = serde_json::from_str(text).map_err(|e| format!("config invalida: {e}"))?;
    let auth_required = match raw.auth.as_deref() {
        None | Some("required") => true,
        Some("off") => false,
        Some(_) => return Err("config: auth deve ser required ou off".to_string()),
    };
    let herdr_bin = raw.herdr_bin.unwrap_or_else(|| "herdr".to_string());
    if herdr_bin.is_empty() || herdr_bin.contains('\0') {
        return Err("config: herdr_bin invalido".to_string());
    }
    let tmux_bin = raw.tmux_bin.unwrap_or_else(|| "tmux".to_string());
    if tmux_bin.is_empty() || tmux_bin.contains('\0') {
        return Err("config: tmux_bin invalido".to_string());
    }
    let bell_poll_ms = raw.bell_poll_ms.unwrap_or(5000);
    if bell_poll_ms < 50 {
        return Err("config: bell_poll_ms deve ser de no minimo 50".to_string());
    }
    let bell_rearm_s = raw.bell_rearm_s.unwrap_or(600);
    if bell_rearm_s == 0 {
        return Err("config: bell_rearm_s deve ser de no minimo 1".to_string());
    }
    let mut recipients = BTreeMap::new();
    for (name, rec) in raw.recipients {
        if !valid_name(&name) {
            return Err(format!("config: nome de destinatario invalido '{name}'"));
        }
        let agent = rec.agent.clone().unwrap_or_else(|| {
            name.split_once('-')
                .map_or(name.as_str(), |(prefix, _)| prefix)
                .to_string()
        });
        if !valid_name(&agent) {
            return Err(format!("config: agent de '{name}' invalido"));
        }
        let delivery = parse_delivery(&name, rec)?;
        recipients.insert(name, Recipient { delivery, agent });
    }
    let blocked_receipts = raw
        .features
        .and_then(|f| f.blocked_receipts)
        .unwrap_or(false);
    let mut due_defaults =
        BTreeMap::from([("PEDIDO".to_string(), 1800), ("PERGUNTA".to_string(), 900)]);
    if let Some(custom) = raw.due_defaults {
        for (kind, seconds) in custom {
            if !matches!(kind.as_str(), "PEDIDO" | "PERGUNTA") || !(300..=86_400).contains(&seconds)
            {
                return Err(format!("config: due_defaults invalido para '{kind}'"));
            }
            due_defaults.insert(kind, seconds);
        }
    }
    let roles = raw
        .roles
        .map(|r| {
            if !valid_name(&r.orchestrator) || !valid_name(&r.owner) {
                Err("config: roles contem nome invalido".to_string())
            } else {
                Ok(Roles {
                    orchestrator: r.orchestrator,
                    owner: r.owner,
                })
            }
        })
        .transpose()?;
    Ok(Config {
        auth_required,
        herdr_bin,
        tmux_bin,
        bell_tmux_typing: raw.bell_tmux_typing.unwrap_or(false),
        bell_poll_ms,
        bell_rearm_s,
        recipients,
        blocked_receipts,
        due_defaults,
        roles,
    })
}

fn parse_delivery(name: &str, rec: RawRecipient) -> Result<Delivery, String> {
    match (rec.delivery.as_deref(), rec.pane) {
        (Some("bell"), Some(pane)) if valid_pane(&pane) => Ok(Delivery::Bell { pane }),
        (Some("bell"), _) => Err(format!(
            "config: '{name}' com delivery bell exige pane valido"
        )),
        (Some("watch"), None) => Ok(Delivery::Watch),
        (Some("watch"), Some(_)) => Err(format!(
            "config: '{name}' com delivery watch nao aceita pane"
        )),
        (Some("auto"), None) => Ok(Delivery::Auto),
        (Some("auto"), Some(_)) => Err(format!(
            "config: '{name}' com delivery auto nao aceita pane"
        )),
        (None, _) => Err(format!("config: '{name}' sem delivery (bell ou watch)")),
        (Some(_), _) => Err(format!(
            "config: delivery de '{name}' deve ser bell, watch ou auto"
        )),
    }
}

/// Pane do herdr: 1 a 64 caracteres de `[A-Za-z0-9:._-]`, sem comecar por `-`.
pub fn valid_pane(s: &str) -> bool {
    crate::protocol::valid_pane_id_with(s, 64, false)
}

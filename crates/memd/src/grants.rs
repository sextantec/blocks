use chrono::NaiveDate;
use serde::Deserialize;

use crate::token_sha256_hex;

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Grant {
    pub agent: String,
    pub token_sha256: String,
    pub ops: Vec<String>,
    pub read_scopes: Vec<String>,
    pub write_scopes: Vec<String>,
    pub types: Vec<String>,
    pub expires: Option<String>,
    pub intent: String,
}

#[derive(Clone, Debug)]
pub struct Grants {
    pub(crate) profiles: Vec<Grant>,
}

#[derive(Deserialize)]
#[serde(untagged)]
enum GrantsFile {
    List(Vec<Grant>),
    Object(GrantsObject),
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct GrantsObject {
    grants: Vec<Grant>,
}

impl Grants {
    pub fn from_json(s: &str) -> Result<Grants, String> {
        let parsed: GrantsFile =
            serde_json::from_str(s).map_err(|e| format!("grants JSON invalido: {e}"))?;
        let profiles = match parsed {
            GrantsFile::List(v) => v,
            GrantsFile::Object(object) => object.grants,
        };
        let mut agents = std::collections::HashSet::new();
        let mut hashes = std::collections::HashSet::new();
        for grant in &profiles {
            validate_grant(grant)?;
            if !agents.insert(&grant.agent) {
                return Err("agent duplicado em grants".into());
            }
            if !hashes.insert(&grant.token_sha256) {
                return Err("token_sha256 duplicado em grants".into());
            }
        }
        Ok(Self { profiles })
    }

    pub(crate) fn authenticate(&self, token: &str, today: NaiveDate) -> Option<&Grant> {
        if !valid_token(token) {
            return None;
        }
        let hash = token_sha256_hex(token);
        self.profiles.iter().find(|grant| {
            constant_time_eq(grant.token_sha256.as_bytes(), hash.as_bytes())
                && grant
                    .expires
                    .as_deref()
                    .and_then(|s| NaiveDate::parse_from_str(s, "%Y-%m-%d").ok())
                    .is_none_or(|expires| today <= expires)
        })
    }
}

impl Grant {
    pub(crate) fn permits_type(&self, kind: &str) -> bool {
        self.types.iter().any(|item| item == kind)
    }

    pub(crate) fn can_read(&self, kind: &str, scope: &str) -> bool {
        self.permits_type(kind) && self.read_scopes.iter().any(|p| scope_matches(p, scope))
    }

    pub(crate) fn can_write(&self, op: &str, kind: &str, scope: &str) -> bool {
        self.ops.iter().any(|item| item == op)
            && self.permits_type(kind)
            && self.write_scopes.iter().any(|p| scope_matches(p, scope))
    }
}

fn validate_grant(grant: &Grant) -> Result<(), String> {
    if !valid_agent(&grant.agent) {
        return Err("agent invalido em grants".into());
    }
    if grant.token_sha256.len() != 64
        || !grant
            .token_sha256
            .bytes()
            .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
    {
        return Err("token_sha256 invalido em grants".into());
    }
    if grant.ops.iter().any(|v| v != "propose" && v != "promote") {
        return Err("op invalida em grants".into());
    }
    if grant
        .types
        .iter()
        .any(|v| v != "decision" && v != "pending")
    {
        return Err("type invalido em grants".into());
    }
    if grant.intent.chars().count() > 400 {
        return Err("intent acima de 400 caracteres".into());
    }
    for pattern in grant.read_scopes.iter().chain(&grant.write_scopes) {
        if pattern != "*" {
            let base = pattern.strip_suffix("/**").unwrap_or(pattern);
            if !valid_scope(base) {
                return Err("padrao de escopo invalido em grants".into());
            }
        }
    }
    if let Some(expires) = &grant.expires {
        NaiveDate::parse_from_str(expires, "%Y-%m-%d")
            .map_err(|_| "expires invalido em grants".to_string())?;
    }
    Ok(())
}

pub(crate) fn scope_matches(pattern: &str, scope: &str) -> bool {
    pattern == "*"
        || pattern == scope
        || pattern.strip_suffix("/**").is_some_and(|base| {
            scope == base || scope.strip_prefix(base).is_some_and(|s| s.starts_with('/'))
        })
}

pub(crate) fn valid_agent(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 32
        && value
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_' || b == b'-')
}

pub(crate) fn valid_scope(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 64
        && value
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b"._/-".contains(&b))
}

fn valid_token(token: &str) -> bool {
    token.len() == 46
        && token.starts_with("mt_")
        && token[3..]
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
}

fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    a.iter().zip(b).fold(0_u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

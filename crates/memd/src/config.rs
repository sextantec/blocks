use serde::Deserialize;

#[derive(Clone, Debug, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Config {
    pub(crate) proposal_ttl_days: u32,
    pub(crate) view_budget: usize,
    pub(crate) view_overdue_max: usize,
    pub(crate) max_open_proposals: usize,
    pub(crate) max_records: usize,
    pub(crate) max_connections: usize,
    pub(crate) secret_patterns: Vec<String>,
    pub(crate) line_max: usize,
    pub(crate) response_max: usize,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            proposal_ttl_days: 14,
            view_budget: 6_000,
            view_overdue_max: 5,
            max_open_proposals: 20,
            max_records: 20_000,
            max_connections: 32,
            secret_patterns: Vec::new(),
            line_max: 262_144,
            response_max: 65_536,
        }
    }
}

impl Config {
    pub fn from_json(s: &str) -> Result<Config, String> {
        let cfg: Self =
            serde_json::from_str(s).map_err(|e| format!("config JSON invalido: {e}"))?;
        cfg.validate()?;
        Ok(cfg)
    }

    pub(crate) fn validate(&self) -> Result<(), String> {
        if !(1..=90).contains(&self.proposal_ttl_days) {
            return Err("proposal_ttl_days fora da faixa 1..=90".into());
        }
        if !(1_000..=20_000).contains(&self.view_budget) {
            return Err("view_budget fora da faixa 1000..=20000".into());
        }
        if !(1..=20).contains(&self.view_overdue_max) {
            return Err("view_overdue_max fora da faixa 1..=20".into());
        }
        if self.max_open_proposals == 0 || self.max_records == 0 || self.max_connections == 0 {
            return Err("cotas devem ser maiores que zero".into());
        }
        if !(4_096..=4_194_304).contains(&self.line_max) {
            return Err("line_max fora da faixa 4096..=4194304".into());
        }
        if !(1_024..=4_194_304).contains(&self.response_max) {
            return Err("response_max fora da faixa 1024..=4194304".into());
        }
        if self.secret_patterns.iter().any(|p| p.is_empty()) {
            return Err("secret_patterns nao aceita string vazia".into());
        }
        Ok(())
    }
}

//! Envelopes, validacao e erros com codigo string (spec v1 5 e v1.1 B e C).

use serde_json::{Map, Value, json};

pub const PROTO_VERSION: u64 = 1;
pub const MAX_LINE: usize = 1_048_576;
pub const MAX_BODY: usize = 16_384;
pub const MAX_NOTE: usize = 1_024;
pub const MAX_SUBJECT: usize = 200;
pub const MAX_REFS: usize = 16;
pub const MAX_REF_V: usize = 512;
pub const TAKE_DEFAULT: usize = 50;
pub const LIST_DEFAULT: u64 = 50;
pub const LIST_MAX: u64 = 200;
pub const KINDS: [&str; 5] = ["AVISO", "PEDIDO", "PERGUNTA", "RESPOSTA", "ACK"];
pub const REF_TYPES: [&str; 6] = ["path", "commit", "msg", "job", "url", "blob"];
/// Metodos disponiveis depois da autenticacao (ou sempre, com `auth: off`).
pub const METHODS: [&str; 15] = [
    "message.send",
    "message.list",
    "message.take",
    "message.ack",
    "events.resume",
    "status",
    "agent.bind",
    "message.get",
    "mailbox.counts",
    "blob.put.begin",
    "blob.put.chunk",
    "blob.put.end",
    "blob.get",
    "blob.rm",
    "blob.list",
];

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{code}: {message}")]
pub struct ApiError {
    pub code: &'static str,
    pub message: String,
    pub path: Option<String>,
}

impl ApiError {
    pub fn new(code: &'static str, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
            path: None,
        }
    }

    pub fn invalid(path: &str, message: impl Into<String>) -> Self {
        Self {
            code: "invalid_envelope",
            message: message.into(),
            path: Some(path.to_string()),
        }
    }

    /// Erro identico para as tres causas de falha de autenticacao (spec v1.1 B).
    pub fn unauthenticated() -> Self {
        Self::new("unauthenticated", "autenticacao falhou")
    }

    pub fn to_error_json(&self) -> Value {
        let mut err = Map::new();
        err.insert("code".into(), Value::from(self.code));
        err.insert("message".into(), Value::from(self.message.clone()));
        if let Some(p) = &self.path {
            let mut data = Map::new();
            data.insert("path".into(), Value::from(p.clone()));
            err.insert("data".into(), Value::Object(data));
        }
        Value::Object(err)
    }
}

pub fn valid_name(s: &str) -> bool {
    (1..=32).contains(&s.len())
        && s.bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_' || b == b'-')
}

/// Caractere de controle (C0, DEL, C1) fora da lista `allow` (spec F.1).
pub fn has_control(s: &str, allow: &[char]) -> bool {
    s.chars().any(|c| c.is_control() && !allow.contains(&c))
}

pub fn valid_id(s: &str) -> bool {
    (1..=128).contains(&s.len())
        && s.bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b':' | b'/' | b'-'))
}

/// Nome de destinatario/remetente validado no caminho `path` do erro.
pub fn check_name(s: &str, path: &str) -> Result<(), ApiError> {
    if valid_name(s) {
        Ok(())
    } else {
        Err(ApiError::invalid(path, "nome invalido"))
    }
}

/// Pane id passado como argumento de subprocesso (sem shell ou controles).
pub fn valid_pane_id(s: &str) -> bool {
    valid_pane_id_with(s, 128, true)
}

pub fn valid_pane_id_with(s: &str, max_len: usize, allow_percent: bool) -> bool {
    (1..=max_len).contains(&s.len())
        && !s.starts_with('-')
        && s.bytes().all(|b| {
            b.is_ascii_alphanumeric()
                || matches!(b, b':' | b'.' | b'_' | b'-')
                || (allow_percent && b == b'%')
        })
}

/// Identidade: com conexao autenticada (`who` = Some), campo ausente vira a identidade e
/// campo diferente e `forbidden`; sem autenticacao o campo e obrigatorio.
pub fn resolve(field: Option<String>, who: Option<&str>, path: &str) -> Result<String, ApiError> {
    match (who, field) {
        (Some(w), None) => Ok(w.to_string()),
        (Some(w), Some(f)) if f == w => Ok(f),
        (Some(_), Some(_)) => Err(ApiError::new(
            "forbidden",
            "identidade diferente da conexao",
        )),
        (None, Some(f)) => Ok(f),
        (None, None) => Err(ApiError::invalid(path, "campo obrigatorio")),
    }
}

/// Requisicao JSON-RPC ja com a forma basica verificada.
pub struct Request {
    pub id: Value,
    pub method: String,
    pub params: Map<String, Value>,
}

/// Le uma linha. Em erro devolve o `id` a ecoar (null se nao houver id valido) e o erro.
pub fn parse_request(line: &[u8]) -> Result<Request, (Value, ApiError)> {
    let value: Value = serde_json::from_slice(line)
        .map_err(|_| (Value::Null, ApiError::new("parse_error", "JSON invalido")))?;
    let Value::Object(mut obj) = value else {
        return Err((
            Value::Null,
            ApiError::new(
                "invalid_request",
                "requisicao deve ser um objeto (lotes nao sao suportados)",
            ),
        ));
    };
    let bad = |m: &str| ApiError::new("invalid_request", m);
    let id = match obj.get("id") {
        Some(v @ (Value::Number(_) | Value::String(_))) => v.clone(),
        _ => return Err((Value::Null, bad("campo id deve ser numero ou texto"))),
    };
    if obj.get("jsonrpc").and_then(Value::as_str) != Some("2.0") {
        return Err((id, bad("campo jsonrpc deve ser \"2.0\"")));
    }
    let method = match obj.remove("method") {
        Some(Value::String(s)) => s,
        _ => return Err((id, bad("campo method ausente ou invalido"))),
    };
    let params = match obj.remove("params") {
        Some(Value::Object(p)) => p,
        _ => return Err((id, bad("campo params deve ser objeto"))),
    };
    Ok(Request { id, method, params })
}

pub fn check_version(p: &Map<String, Value>) -> Result<(), ApiError> {
    match p.get("v") {
        None => Err(ApiError::invalid("/v", "campo v obrigatorio")),
        Some(Value::Number(n)) if n.as_u64() == Some(PROTO_VERSION) => Ok(()),
        Some(Value::Number(_)) => Err(ApiError::new(
            "unsupported_version",
            "versao do protocolo nao suportada",
        )),
        Some(_) => Err(ApiError::invalid("/v", "campo v deve ser inteiro")),
    }
}

pub fn reject_unknown(p: &Map<String, Value>, allowed: &[&str]) -> Result<(), ApiError> {
    for k in p.keys() {
        if k != "v" && !allowed.contains(&k.as_str()) {
            return Err(ApiError::invalid(&format!("/{k}"), "campo desconhecido"));
        }
    }
    Ok(())
}

pub fn req_str<'a>(p: &'a Map<String, Value>, key: &str) -> Result<&'a str, ApiError> {
    match p.get(key) {
        Some(Value::String(s)) => Ok(s),
        _ => Err(ApiError::invalid(
            &format!("/{key}"),
            format!("campo {key} obrigatorio (texto)"),
        )),
    }
}

pub fn opt_str<'a>(p: &'a Map<String, Value>, key: &str) -> Result<Option<&'a str>, ApiError> {
    match p.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(s)) => Ok(Some(s)),
        _ => Err(ApiError::invalid(
            &format!("/{key}"),
            format!("campo {key} deve ser texto"),
        )),
    }
}

pub fn req_u64(p: &Map<String, Value>, key: &str) -> Result<u64, ApiError> {
    p.get(key).and_then(Value::as_u64).ok_or_else(|| {
        ApiError::invalid(
            &format!("/{key}"),
            format!("campo {key} obrigatorio (inteiro)"),
        )
    })
}

pub fn opt_u64(p: &Map<String, Value>, key: &str) -> Result<Option<u64>, ApiError> {
    match p.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(v) => v.as_u64().map(Some).ok_or_else(|| {
            ApiError::invalid(&format!("/{key}"), format!("campo {key} deve ser inteiro"))
        }),
    }
}

pub fn opt_bool(p: &Map<String, Value>, key: &str) -> Result<Option<bool>, ApiError> {
    match p.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::Bool(b)) => Ok(Some(*b)),
        _ => Err(ApiError::invalid(
            &format!("/{key}"),
            format!("campo {key} deve ser booleano"),
        )),
    }
}

/// Referencia estruturada (v1.1 C.2): aponta para algo fora do corpo.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Ref {
    pub t: String,
    pub v: String,
}

/// Valida a lista `refs` (ate 16 itens de `{t, v}`) com caminhos `/refs/N/t` e `/refs/N/v`.
pub fn parse_refs(value: Option<&Value>) -> Result<Vec<Ref>, ApiError> {
    let arr = match value {
        None | Some(Value::Null) => return Ok(Vec::new()),
        Some(Value::Array(a)) => a,
        Some(_) => return Err(ApiError::invalid("/refs", "refs deve ser lista")),
    };
    if arr.len() > MAX_REFS {
        return Err(ApiError::invalid("/refs", "refs acima de 16 itens"));
    }
    let mut out = Vec::with_capacity(arr.len());
    for (i, item) in arr.iter().enumerate() {
        let Value::Object(o) = item else {
            return Err(ApiError::invalid(
                &format!("/refs/{i}"),
                "item de refs deve ser objeto",
            ));
        };
        for k in o.keys() {
            if k != "t" && k != "v" {
                return Err(ApiError::invalid(
                    &format!("/refs/{i}/{k}"),
                    "campo desconhecido",
                ));
            }
        }
        let t = match o.get("t") {
            Some(Value::String(s)) if REF_TYPES.contains(&s.as_str()) => s.clone(),
            _ => {
                return Err(ApiError::invalid(
                    &format!("/refs/{i}/t"),
                    "tipo de ref invalido",
                ));
            }
        };
        let v = match o.get("v") {
            Some(Value::String(s)) if valid_ref_value(&t, s) => s.clone(),
            _ => {
                return Err(ApiError::invalid(
                    &format!("/refs/{i}/v"),
                    "valor de ref invalido",
                ));
            }
        };
        out.push(Ref { t, v });
    }
    Ok(out)
}

fn valid_blob_id(s: &str) -> bool {
    s.strip_prefix("blob:").is_some_and(|hex| {
        hex.len() == 32
            && hex
                .bytes()
                .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
    })
}

fn valid_ref_value(kind: &str, value: &str) -> bool {
    (1..=MAX_REF_V).contains(&value.chars().count())
        && !has_control(value, &[])
        && (kind != "blob" || valid_blob_id(value))
}

pub fn refs_to_json(refs: &[Ref]) -> Value {
    Value::Array(refs.iter().map(|r| json!({"t": r.t, "v": r.v})).collect())
}

pub struct SendParams {
    pub id: String,
    /// Remetente como veio no envelope; a identidade da conexao e resolvida no servidor.
    pub from: Option<String>,
    pub to: String,
    pub kind: String,
    pub subject: String,
    pub body: String,
    pub thread: Option<String>,
    pub refs: Vec<Ref>,
    pub keep_open: bool,
    pub due_s: Option<u64>,
}

pub fn parse_send(p: &Map<String, Value>) -> Result<SendParams, ApiError> {
    reject_unknown(
        p,
        &[
            "id",
            "from",
            "to",
            "kind",
            "subject",
            "body",
            "thread",
            "refs",
            "keep_open",
            "due_s",
        ],
    )?;
    let id = req_str(p, "id")?;
    if !valid_id(id) {
        return Err(ApiError::invalid("/id", "id invalido"));
    }
    let from = opt_str(p, "from")?.map(str::to_string);
    let to = req_str(p, "to")?;
    let kind = req_str(p, "kind")?;
    if !KINDS.contains(&kind) {
        return Err(ApiError::invalid("/kind", "kind invalido"));
    }
    let subject = req_str(p, "subject")?;
    let n = subject.chars().count();
    if !(1..=MAX_SUBJECT).contains(&n) || has_control(subject, &[]) {
        return Err(ApiError::invalid(
            "/subject",
            "subject deve ter de 1 a 200 caracteres, sem controle",
        ));
    }
    let body = req_str(p, "body")?;
    if body.len() > MAX_BODY {
        return Err(ApiError::invalid("/body", "body acima de 16384 bytes"));
    }
    if has_control(body, &['\n', '\t']) {
        return Err(ApiError::invalid(
            "/body",
            "body com controle (so \\n e \\t)",
        ));
    }
    let thread = match opt_str(p, "thread")? {
        None => None,
        Some(t) if valid_id(t) => Some(t.to_string()),
        Some(_) => return Err(ApiError::invalid("/thread", "thread invalido")),
    };
    let refs = parse_refs(p.get("refs"))?;
    Ok(SendParams {
        id: id.to_string(),
        from,
        to: to.to_string(),
        kind: kind.to_string(),
        subject: subject.to_string(),
        body: body.to_string(),
        thread,
        refs,
        keep_open: opt_bool(p, "keep_open")?.unwrap_or(false),
        due_s: match p.get("due_s") {
            None => None,
            Some(v) => Some(
                v.as_u64()
                    .filter(|n| (300..=86_400).contains(n))
                    .ok_or_else(|| ApiError::invalid("/due_s", "due_s fora do intervalo"))?,
            ),
        },
    })
}

pub struct AckParams {
    pub id: String,
    pub by: Option<String>,
    pub stage: String,
    pub result: Option<String>,
    pub note: Option<String>,
    pub await_class: Option<String>,
}

pub fn parse_ack(p: &Map<String, Value>) -> Result<AckParams, ApiError> {
    reject_unknown(p, &["id", "by", "stage", "result", "note", "await"])?;
    let id = req_str(p, "id")?;
    if !valid_id(id) {
        return Err(ApiError::invalid("/id", "id invalido"));
    }
    let by = opt_str(p, "by")?.map(str::to_string);
    let stage = req_str(p, "stage")?;
    if stage != "aceito" && stage != "resultado" && stage != "bloqueado" {
        return Err(ApiError::invalid(
            "/stage",
            "stage deve ser aceito, resultado ou bloqueado",
        ));
    }
    let result = match opt_str(p, "result")? {
        None => None,
        Some(r) => {
            if stage != "resultado" {
                return Err(ApiError::invalid(
                    "/result",
                    "result so vale com stage resultado",
                ));
            }
            if !matches!(r, "done" | "refused" | "failed") {
                return Err(ApiError::invalid(
                    "/result",
                    "result deve ser done, refused ou failed",
                ));
            }
            Some(r.to_string())
        }
    };
    let allow_untrusted_note =
        stage == "resultado" && matches!(result.as_deref(), Some("refused" | "failed"));
    let note = match opt_str(p, "note")? {
        None => None,
        Some(n)
            if n.chars().count() <= MAX_NOTE
                && (allow_untrusted_note || !has_control(n, &['\t'])) =>
        {
            Some(n.to_string())
        }
        Some(_) => {
            return Err(ApiError::invalid(
                "/note",
                "note invalida (ate 1024 caracteres, sem controle exceto tab)",
            ));
        }
    };
    if stage == "bloqueado" && note.is_some() {
        return Err(ApiError::invalid(
            "/note",
            "bloqueado nao aceita nota livre",
        ));
    }
    let await_class = match opt_str(p, "await")? {
        None if stage == "bloqueado" => {
            return Err(ApiError::invalid("/await", "bloqueado exige await"));
        }
        None => None,
        Some("none") if stage == "bloqueado" => Some("none".to_string()),
        Some(v)
            if stage == "bloqueado"
                && [
                    "permission",
                    "user_input",
                    "network",
                    "identity",
                    "policy",
                    "unknown",
                ]
                .contains(&v) =>
        {
            Some(v.to_string())
        }
        Some(_) => {
            return Err(ApiError::invalid(
                "/await",
                "await invalido ou fora de stage bloqueado",
            ));
        }
    };
    Ok(AckParams {
        id: id.to_string(),
        by,
        stage: stage.to_string(),
        result,
        note,
        await_class,
    })
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BindParams {
    pub mux: String,
    pub pane: String,
}

pub fn parse_bind(p: &Map<String, Value>) -> Result<BindParams, ApiError> {
    reject_unknown(p, &["mux", "pane", "name"])?;
    let _ = opt_str(p, "name")?;
    let mux = req_str(p, "mux")?;
    if !matches!(mux, "herdr" | "tmux") {
        return Err(ApiError::invalid("/mux", "mux deve ser herdr ou tmux"));
    }
    let pane = req_str(p, "pane")?;
    if !valid_pane_id(pane) {
        return Err(ApiError::invalid("/pane", "pane invalido"));
    }
    Ok(BindParams {
        mux: mux.to_string(),
        pane: pane.to_string(),
    })
}

pub fn parse_get(p: &Map<String, Value>) -> Result<String, ApiError> {
    reject_unknown(p, &["id"])?;
    let id = req_str(p, "id")?;
    if !valid_id(id) {
        return Err(ApiError::invalid("/id", "id invalido"));
    }
    Ok(id.to_string())
}

pub struct ListParams {
    pub to: Option<String>,
    pub after_seq: u64,
    pub limit: usize,
    pub unresolved_only: bool,
}

pub fn parse_list(p: &Map<String, Value>) -> Result<ListParams, ApiError> {
    reject_unknown(p, &["to", "after_seq", "limit", "unresolved_only"])?;
    let to = opt_str(p, "to")?.map(str::to_string);
    let limit = opt_u64(p, "limit")?.unwrap_or(LIST_DEFAULT);
    if !(1..=LIST_MAX).contains(&limit) {
        return Err(ApiError::invalid(
            "/limit",
            "limit deve estar entre 1 e 200",
        ));
    }
    Ok(ListParams {
        to,
        after_seq: opt_u64(p, "after_seq")?.unwrap_or(0),
        limit: limit as usize,
        unresolved_only: opt_bool(p, "unresolved_only")?.unwrap_or(false),
    })
}

pub struct TakeParams {
    pub to: Option<String>,
    pub limit: usize,
}

pub fn parse_take(p: &Map<String, Value>) -> Result<TakeParams, ApiError> {
    reject_unknown(p, &["to", "limit"])?;
    let to = opt_str(p, "to")?.map(str::to_string);
    let limit = opt_u64(p, "limit")?.unwrap_or(TAKE_DEFAULT as u64);
    if !(1..=LIST_MAX).contains(&limit) {
        return Err(ApiError::invalid(
            "/limit",
            "limit deve estar entre 1 e 200",
        ));
    }
    Ok(TakeParams {
        to,
        limit: limit as usize,
    })
}

/// `auth {v, name, token}`: devolve (name, token) sem validar o formato (isso e do servidor).
pub fn parse_auth(p: &Map<String, Value>) -> Result<(String, String), ApiError> {
    reject_unknown(p, &["name", "token"])?;
    let name = req_str(p, "name")?;
    let token = req_str(p, "token")?;
    Ok((name.to_string(), token.to_string()))
}

/// Hash do envelope canonico de `message.send` (spec v1 5.1, com `refs` na v1.1).
pub fn envelope_hash(
    from: &str,
    to: &str,
    kind: &str,
    subject: &str,
    body: &str,
    thread: Option<&str>,
    refs: &[Ref],
) -> String {
    let mut m = Map::new();
    m.insert("from".into(), Value::from(from));
    m.insert("to".into(), Value::from(to));
    m.insert("kind".into(), Value::from(kind));
    m.insert("subject".into(), Value::from(subject));
    m.insert("body".into(), Value::from(body));
    m.insert("thread".into(), thread.map_or(Value::Null, Value::from));
    m.insert("refs".into(), refs_to_json(refs));
    crate::hash::envelope_hash(&Value::Object(m))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn refs_of(v: Value) -> Result<Vec<Ref>, ApiError> {
        parse_refs(Some(&v))
    }

    #[test]
    fn refs_validas_e_ausentes() {
        assert_eq!(parse_refs(None).unwrap(), Vec::new());
        let ok = refs_of(json!([{"t": "path", "v": "src/a.rs"}, {"t": "url", "v": "https://x/y"}]))
            .unwrap();
        assert_eq!(ok.len(), 2);
        assert_eq!(ok[1].t, "url");
    }

    #[test]
    fn refs_recusadas_com_caminho() {
        let e = refs_of(json!([{"t": "arquivo", "v": "x"}])).unwrap_err();
        assert_eq!(e.path.as_deref(), Some("/refs/0/t"));
        let e = refs_of(json!([{"t": "path", "v": ""}])).unwrap_err();
        assert_eq!(e.path.as_deref(), Some("/refs/0/v"));
        let e = refs_of(json!([{"t": "msg", "v": "a\nb"}])).unwrap_err();
        assert_eq!(e.path.as_deref(), Some("/refs/0/v"));
        let e = refs_of(json!([{"t": "job", "v": "j", "x": 1}])).unwrap_err();
        assert_eq!(e.path.as_deref(), Some("/refs/0/x"));
        let muitas: Vec<Value> = (0..17).map(|_| json!({"t": "job", "v": "j"})).collect();
        let e = refs_of(Value::Array(muitas)).unwrap_err();
        assert_eq!(e.path.as_deref(), Some("/refs"));
    }

    #[test]
    fn refs_limite_de_512_caracteres() {
        let no_limite = "x".repeat(512);
        assert!(refs_of(json!([{"t": "path", "v": no_limite}])).is_ok());
        let acima = "x".repeat(513);
        assert!(refs_of(json!([{"t": "path", "v": acima}])).is_err());
    }

    #[test]
    fn refs_entram_no_hash() {
        let a = envelope_hash("a", "b", "AVISO", "s", "c", None, &[]);
        let r = [Ref {
            t: "path".into(),
            v: "x".into(),
        }];
        let b = envelope_hash("a", "b", "AVISO", "s", "c", None, &r);
        assert_ne!(a, b);
    }

    #[test]
    fn identidade_implicita_e_forbidden() {
        assert_eq!(resolve(None, Some("a"), "/from").unwrap(), "a");
        assert_eq!(resolve(Some("a".into()), Some("a"), "/from").unwrap(), "a");
        assert_eq!(
            resolve(Some("b".into()), Some("a"), "/from")
                .unwrap_err()
                .code,
            "forbidden"
        );
        assert_eq!(
            resolve(None, None, "/from").unwrap_err().path.as_deref(),
            Some("/from")
        );
        assert_eq!(resolve(Some("b".into()), None, "/from").unwrap(), "b");
    }
}

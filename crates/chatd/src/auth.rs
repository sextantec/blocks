//! Tokens por agente (v1.1 B): `auth.json` com SHA-256 do token, emissao, revogacao, listagem
//! e verificacao. O token nunca e gravado em claro nem impresso fora de `token new`.

use std::collections::BTreeMap;
use std::fs::{self, File, OpenOptions};
use std::io::{ErrorKind, Read, Write};
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};

use serde_json::{Value, json};

use crate::protocol::valid_name;
use crate::{Failure, Opts, env_path, parse_opts, usage};

pub const AUTH_FILE: &str = "auth.json";

/// Token: 32 bytes aleatorios em 64 caracteres hexadecimais minusculos.
pub fn valid_token(t: &str) -> bool {
    t.len() == 64
        && t.bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

/// SHA-256 hexadecimal do texto do token (o que fica em `auth.json`).
pub fn token_hash(token: &str) -> String {
    crate::hash::sha256_hex(token.as_bytes())
}

/// Comparacao em tempo constante.
fn ct_eq(a: &[u8], b: &[u8]) -> bool {
    a.len() == b.len() && a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

pub fn random_token() -> Result<String, String> {
    let mut b = [0u8; 32];
    File::open("/dev/urandom")
        .and_then(|mut f| f.read_exact(&mut b))
        .map_err(|e| format!("nao li /dev/urandom: {e}"))?;
    Ok(b.iter().map(|x| format!("{x:02x}")).collect())
}

/// Le `auth.json`. Arquivo ausente = nenhum token. Formato fora do esperado = erro.
pub fn load(path: &Path) -> Result<BTreeMap<String, String>, String> {
    let text = match fs::read_to_string(path) {
        Ok(t) => t,
        Err(e) if e.kind() == ErrorKind::NotFound => return Ok(BTreeMap::new()),
        Err(e) => return Err(format!("nao li auth.json: {e}")),
    };
    let v: Value = serde_json::from_str(&text).map_err(|_| "auth.json invalido".to_string())?;
    let obj = v.as_object().ok_or("auth.json invalido")?;
    if obj.len() != 2 || obj.get("v").and_then(Value::as_u64) != Some(1) {
        return Err("auth.json: versao invalida".to_string());
    }
    let tokens = obj
        .get("tokens")
        .and_then(Value::as_object)
        .ok_or("auth.json: campo tokens invalido")?;
    let mut out = BTreeMap::new();
    for (name, h) in tokens {
        let hs = h.as_str().unwrap_or("");
        if !valid_name(name) || hs.len() != 64 || !hs.bytes().all(|b| b.is_ascii_hexdigit()) {
            return Err("auth.json: entrada invalida".to_string());
        }
        out.insert(name.clone(), hs.to_string());
    }
    Ok(out)
}

/// Substituicao atomica: grava temporario 0600 no mesmo diretorio, fsync, rename.
pub fn store(path: &Path, tokens: &BTreeMap<String, String>) -> Result<(), String> {
    let dir = path.parent().ok_or("caminho de auth.json sem diretorio")?;
    let tmp = dir.join(format!(".auth.json.tmp.{}", std::process::id()));
    let _ = fs::remove_file(&tmp);
    let body = json!({"v": 1, "tokens": tokens}).to_string();
    let res = (|| -> std::io::Result<()> {
        let mut f = OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&tmp)?;
        f.write_all(body.as_bytes())?;
        f.sync_all()?;
        drop(f);
        fs::rename(&tmp, path)?;
        if let Ok(d) = File::open(dir) {
            let _ = d.sync_all();
        }
        Ok(())
    })();
    if let Err(e) = res {
        let _ = fs::remove_file(&tmp);
        return Err(format!("nao gravei auth.json: {e}"));
    }
    Ok(())
}

/// Verifica (name, token) contra `auth.json`, relido a cada chamada (revogacao vale ja).
pub fn verify(path: &Path, name: &str, token: &str) -> Result<bool, String> {
    let tokens = load(path)?;
    let presented = token_hash(token);
    let stored = tokens.get(name).cloned().unwrap_or_else(|| "0".repeat(64));
    let eq = ct_eq(presented.as_bytes(), stored.as_bytes());
    Ok(valid_name(name) && valid_token(token) && tokens.contains_key(name) && eq)
}

/// `chatd token new|revoke|list` (v1.1 B).
pub fn cli(args: &[String]) -> Result<(), Failure> {
    let Some((sub, rest)) = args.split_first() else {
        return Err(usage("token exige new, revoke ou list"));
    };
    match sub.as_str() {
        "new" => new(rest),
        "revoke" => revoke(rest),
        "list" => list(rest),
        other => Err(usage(format!("subcomando de token desconhecido '{other}'"))),
    }
}

fn split_name(rest: &[String]) -> Result<(String, &[String]), Failure> {
    let Some((name, r)) = rest.split_first() else {
        return Err(usage("falta NOME"));
    };
    if name.starts_with("--") || !valid_name(name) {
        return Err(usage("NOME invalido (use [a-z0-9_-], ate 32)"));
    }
    Ok((name.clone(), r))
}

fn auth_path(o: &Opts) -> Result<PathBuf, Failure> {
    let dir = o
        .get("state-dir")
        .map(PathBuf::from)
        .or_else(|| env_path("CHATD_STATE_DIR"))
        .ok_or_else(|| usage("falta --state-dir ou CHATD_STATE_DIR"))?;
    Ok(dir.join(AUTH_FILE))
}

fn run(m: impl Into<String>) -> Failure {
    Failure::Run(m.into())
}

fn new(rest: &[String]) -> Result<(), Failure> {
    let (name, r) = split_name(rest)?;
    let o = parse_opts(r, &["state-dir", "out"], &["rotate"])?;
    let path = auth_path(&o)?;
    let mut tokens = load(&path).map_err(run)?;
    if tokens.contains_key(&name) && !o.flag("rotate") {
        return Err(run("NOME ja tem token; use --rotate para substituir"));
    }
    let out = o.get("out").map(PathBuf::from);
    let token = random_token().map_err(run)?;
    // O arquivo de saida e criado antes de tocar em auth.json, para nao deixar token orfao.
    let out_file = match &out {
        Some(p) => {
            let mut f = OpenOptions::new()
                .write(true)
                .create_new(true)
                .mode(0o600)
                .open(p)
                .map_err(|e| run(format!("nao criei --out ({e}); ja existe?")))?;
            f.write_all(format!("{token}\n").as_bytes())
                .map_err(|e| run(format!("nao gravei --out: {e}")))?;
            Some(p.clone())
        }
        None => None,
    };
    tokens.insert(name.clone(), token_hash(&token));
    if let Err(e) = store(&path, &tokens) {
        if let Some(p) = &out_file {
            let _ = fs::remove_file(p);
        }
        return Err(run(e));
    }
    match out_file {
        Some(_) => println!("ok {name}"),
        None => println!("{token}"),
    }
    Ok(())
}

fn revoke(rest: &[String]) -> Result<(), Failure> {
    let (name, r) = split_name(rest)?;
    let o = parse_opts(r, &["state-dir"], &[])?;
    let path = auth_path(&o)?;
    let mut tokens = load(&path).map_err(run)?;
    if tokens.remove(&name).is_none() {
        return Err(run(format!("'{name}' nao tem token")));
    }
    store(&path, &tokens).map_err(run)?;
    println!("ok {name}");
    Ok(())
}

fn list(rest: &[String]) -> Result<(), Failure> {
    let o = parse_opts(rest, &["state-dir"], &[])?;
    let path = auth_path(&o)?;
    for name in load(&path).map_err(run)?.keys() {
        println!("{name}");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    #[test]
    fn token_tem_64_hex_minusculos() {
        let t = random_token().unwrap();
        assert!(valid_token(&t));
        assert!(!valid_token(&t.to_uppercase()) || !t.chars().any(|c| c.is_ascii_alphabetic()));
        assert!(!valid_token("abc"));
        assert_eq!(token_hash("abc").len(), 64);
    }

    #[test]
    fn auth_json_atomico_0600_e_roundtrip() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(AUTH_FILE);
        assert!(load(&path).unwrap().is_empty());
        let t = random_token().unwrap();
        let mut m = BTreeMap::new();
        m.insert("codex-web".to_string(), token_hash(&t));
        store(&path, &m).unwrap();
        let mode = fs::metadata(&path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600);
        assert_eq!(load(&path).unwrap(), m);
        assert!(verify(&path, "codex-web", &t).unwrap());
        assert!(!verify(&path, "codex-web", &random_token().unwrap()).unwrap());
        assert!(!verify(&path, "outro", &t).unwrap());
        // nenhum temporario sobra no diretorio
        let sobra: Vec<_> = fs::read_dir(dir.path())
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .filter(|n| n.contains("tmp"))
            .collect();
        assert!(sobra.is_empty(), "temporario sobrou: {sobra:?}");
    }

    #[test]
    fn auth_json_com_formato_errado_e_recusado() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(AUTH_FILE);
        fs::write(&path, "{\"v\":1,\"tokens\":{\"a\":\"curto\"}}").unwrap();
        assert!(load(&path).is_err());
        fs::write(&path, "{\"v\":2,\"tokens\":{}}").unwrap();
        assert!(load(&path).is_err());
    }
}

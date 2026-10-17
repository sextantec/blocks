//! Testes do tipo B: o que so o binario prova. Socket Unix real, SIGHUP, arquivos em diretorio temporario,
//! saidas de processo. O binario usa o relogio real: as validades aqui sao relativas ao agora real.
mod common;

use chrono::{SecondsFormat, TimeDelta, Utc};
use common::*;
use grantd::token_sha256_hex;
use serde_json::{Value, json};
use std::fs::{self, File};
use std::io::{BufRead, BufReader, Write};
use std::os::unix::fs::PermissionsExt;
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Output, Stdio};
use std::thread::sleep;
use std::time::{Duration, Instant};

const BIN: &str = env!("CARGO_BIN_EXE_grantd");

/// Instante real (agora + `s` segundos) em RFC 3339 com Z, para `until` de testes do binario.
fn agora_mais(s: i64) -> String {
    (Utc::now() + TimeDelta::seconds(s)).to_rfc3339_opts(SecondsFormat::Secs, true)
}

/// Servidor `grantd serve` em diretorio temporario. Mata o processo ao sair do escopo.
struct Servidor {
    filho: Child,
    sock: PathBuf,
    grants: PathBuf,
    diag: PathBuf,
    _tmp: tempfile::TempDir,
}

impl Drop for Servidor {
    fn drop(&mut self) {
        let _ = self.filho.kill();
        let _ = self.filho.wait();
    }
}

impl Servidor {
    /// Envia SIGHUP ao processo (recarrega `grants.json`).
    fn sighup(&self) {
        // SAFETY: kill(2) so com o pid de um processo filho deste teste e um sinal definido.
        let rc = unsafe { libc::kill(self.filho.id() as i32, libc::SIGHUP) };
        assert_eq!(rc, 0, "kill(SIGHUP) falhou");
    }
}

fn grava_privado(p: &Path, conteudo: &str) {
    fs::write(p, conteudo).unwrap();
    fs::set_permissions(p, fs::Permissions::from_mode(0o600)).unwrap();
}

fn sobe_servidor(grants: &str) -> Servidor {
    let tmp = tempfile::tempdir().unwrap();
    let estado = tmp.path().join("estado");
    fs::create_dir(&estado).unwrap();
    fs::set_permissions(&estado, fs::Permissions::from_mode(0o700)).unwrap();
    let config = tmp.path().join("config.json");
    grava_privado(&config, "{}");
    let grants_path = tmp.path().join("grants.json");
    grava_privado(&grants_path, grants);
    let sock = tmp.path().join("grantd.sock");
    let diag = tmp.path().join("diag.log");
    let erro = File::create(&diag).unwrap();

    let filho = Command::new(BIN)
        .args(["serve", "--state-dir"])
        .arg(&estado)
        .arg("--config")
        .arg(&config)
        .arg("--grants")
        .arg(&grants_path)
        .arg("--socket")
        .arg(&sock)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(erro)
        .spawn()
        .unwrap();
    let s = Servidor {
        filho,
        sock: sock.clone(),
        grants: grants_path,
        diag,
        _tmp: tmp,
    };
    let inicio = Instant::now();
    while !sock.exists() {
        assert!(
            inicio.elapsed() < Duration::from_secs(10),
            "socket nao apareceu"
        );
        sleep(Duration::from_millis(20));
    }
    s
}

/// Conexao persistente ao socket: uma requisicao JSON por linha, uma resposta por linha.
struct Conexao {
    escreve: UnixStream,
    le: BufReader<UnixStream>,
}

impl Conexao {
    fn abre(sock: &Path) -> Conexao {
        let escreve = UnixStream::connect(sock).expect("connect no socket");
        let clone = escreve.try_clone().unwrap();
        Conexao {
            escreve,
            le: BufReader::new(clone),
        }
    }

    /// Envia uma linha e devolve a resposta crua (sem o `\n`).
    fn bruto(&mut self, linha: &str) -> String {
        writeln!(self.escreve, "{linha}").unwrap();
        let mut resposta = String::new();
        let n = self.le.read_line(&mut resposta).unwrap();
        assert!(n > 0, "conexao fechada pelo servidor");
        resposta.trim_end_matches('\n').to_string()
    }

    fn pede(&mut self, token: &str, metodo: &str, params: Value) -> Value {
        let id = novo_id();
        let linha = json!({"v": 1, "id": id, "token": token, "method": metodo, "params": params})
            .to_string();
        let r = self.bruto(&linha);
        let v: Value =
            serde_json::from_str(&r).unwrap_or_else(|e| panic!("resposta nao e JSON ({e}): {r}"));
        assert_eq!(v["id"], json!(id), "id ecoado: {r}");
        v
    }
}

fn concessao_real(grantee: &str, acao: &str, recurso: &str) -> Value {
    com(
        allow(grantee, acao, recurso),
        json!({"until": agora_mais(3600), "request_id": novo_rid()}),
    )
}

/// Repete `check` do agente ate a resposta `ok:false` ou o limite de 5 s; devolve a ultima resposta.
fn espera_negado(c: &mut Conexao, token: &str, params: Value) -> Value {
    let inicio = Instant::now();
    loop {
        let v = c.pede(token, "check", params.clone());
        if v["ok"] == json!(false) || inicio.elapsed() > Duration::from_secs(5) {
            return v;
        }
        sleep(Duration::from_millis(20));
    }
}

// G29 (B): SIGHUP invalida tokens na hora, inclusive em conexao ja aberta: token removido, token com
// `expires` no passado e token trocado (hash diferente) respondem `unauthorized` na mesma conexao.
// Controles: antes do SIGHUP o agente e o dono funcionam nessa conexao; o dono continua valendo depois.
#[test]
fn g29_sighup_invalida_token_em_conexao_aberta() {
    let perfis = perfis_padrao();
    let srv = sobe_servidor(&grants_de(&perfis));
    let mut c = Conexao::abre(&srv.sock);

    let grant = c.pede(&tok_dono(), "grant", concessao_real(A1, "fs.read", "/p/x"));
    resultado(&grant);
    assert_eq!(
        resultado(&c.pede(
            &token_de(A1),
            "check",
            json!({"agent": A1, "action": "fs.read", "resource": "/p/x"})
        ))["decision"],
        json!("allow"),
        "controle antes do SIGHUP"
    );

    // Novo grants.json: a1 removido; a2 com expires no passado; a3 com token trocado.
    let mut novos = perfis_padrao();
    novos.retain(|p| p.agente != A1);
    for p in novos.iter_mut() {
        if p.agente == A2 {
            p.expira = "2000-01-01";
        }
        if p.agente == A3 {
            p.token = token_de("TROCADO");
        }
    }
    grava_privado(&srv.grants, &grants_de(&novos));
    srv.sighup();

    let a1 = espera_negado(
        &mut c,
        &token_de(A1),
        json!({"agent": A1, "action": "fs.read", "resource": "/p/x"}),
    );
    assert_eq!(codigo(&a1), "unauthorized", "a1 removido: {a1}");

    let a2 = c.pede(&token_de(A2), "whoami", json!({}));
    assert_eq!(codigo(&a2), "unauthorized", "a2 expirado: {a2}");
    let a3_antigo = c.pede(&token_de(A3), "whoami", json!({}));
    assert_eq!(
        codigo(&a3_antigo),
        "unauthorized",
        "a3 com token trocado: {a3_antigo}"
    );

    let dono = c.pede(&tok_dono(), "status", json!({}));
    resultado(&dono);
}

// G30 (B): nenhuma API altera perfis. Metodos com cara de gestao de perfil sao desconhecidos
// (invalid_params, 6A) e o
// grants.json fica byte a byte igual; o token de um agente novo continua sem acesso.
// Controle: uma concessao do dono ja existe antes das tentativas e continua valendo depois delas.
#[test]
fn g30_nenhuma_api_altera_perfis() {
    let perfis = perfis_padrao();
    let grants = grants_de(&perfis);
    let srv = sobe_servidor(&grants);
    let antes = fs::read(&srv.grants).unwrap();
    let mut c = Conexao::abre(&srv.sock);
    resultado(&c.pede(&tok_dono(), "grant", concessao_real(A1, "fs.read", "/p/x")));

    let novo_token = token_de("A4NOVO");
    let hash_novo = token_sha256_hex(&novo_token);
    let metodos = [
        "set_profile",
        "add_agent",
        "create_token",
        "rotate_token",
        "update_grants",
        "grants",
        "profile",
        "token",
        "grant_profile",
        "reload_grants",
    ];
    for metodo in metodos {
        let v = c.pede(
            &tok_dono(),
            metodo,
            json!({"agent": "a4", "token_sha256": hash_novo, "role": "owner", "can_request": true}),
        );
        assert_eq!(
            codigo(&v),
            "invalid_params",
            "metodo {metodo} desconhecido: {v}"
        );
    }
    assert_eq!(fs::read(&srv.grants).unwrap(), antes, "grants.json mudou");

    let a4 = c.pede(&novo_token, "whoami", json!({}));
    assert_eq!(codigo(&a4), "unauthorized", "{a4}");
    let a1 = c.pede(
        &token_de(A1),
        "check",
        json!({"agent": A1, "action": "fs.read", "resource": "/p/x"}),
    );
    assert_decisao(&a1, "allow", "ok");
}

// G31 (B): token e `reason` em sentinela nunca aparecem em resposta de `status`, `whoami`, erro, nem no
// log de diagnostico (arquivo de stderr do processo). Controle: a concessao com o motivo e registrada
// (o `revoke` com o motivo funciona), e o status/whoami do dono e do agente respondem ok.
#[test]
fn g31_token_e_reason_nao_vazam_para_diagnostico_status_whoami_ou_erro() {
    let srv = sobe_servidor(&grants_de(&perfis_padrao()));
    let mut c = Conexao::abre(&srv.sock);
    let sentinela_token = token_de("SENTINELATOKEN");
    let sentinela_motivo = "SENTINELA-MOTIVO-G31";

    let g = c.pede(&tok_dono(), "grant", concessao_real(A1, "fs.read", "/p/x"));
    let id_g = id_de(&g);
    let rev = c.pede(
        &tok_dono(),
        "revoke",
        json!({"id": id_g, "reason": sentinela_motivo, "request_id": novo_rid()}),
    );
    resultado(&rev);

    let token_errado = c.bruto(
        &json!({"v": 1, "id": "g31-a", "token": sentinela_token, "method": "whoami", "params": {}})
            .to_string(),
    );
    assert!(token_errado.contains("unauthorized"), "{token_errado}");
    assert!(
        !token_errado.contains("SENTINELATOKEN"),
        "token vazou na resposta de erro: {token_errado}"
    );

    let motivo_em_erro = c.bruto(
        &json!({"v": 1, "id": "g31-b", "token": tok_dono(), "method": "revoke", "params": {"id": "G-999999", "reason": sentinela_motivo, "request_id": "g31-b"}}).to_string(),
    );
    assert!(
        !motivo_em_erro.contains(sentinela_motivo),
        "motivo vazou no erro: {motivo_em_erro}"
    );

    let status = c.pede(&tok_dono(), "status", json!({}));
    let whoami = c.pede(&token_de(A1), "whoami", json!({}));
    resultado(&status);
    let wr = resultado(&whoami);
    let chaves: std::collections::BTreeSet<&str> = wr
        .as_object()
        .expect("whoami e objeto")
        .keys()
        .map(|k| k.as_str())
        .collect();
    assert_eq!(
        chaves,
        ["agent", "role", "can_request", "can_delegate", "expires"]
            .into_iter()
            .collect(),
        "whoami = {{agent, role, can_request, can_delegate, expires}} (5A.13): {whoami}"
    );
    assert_eq!(wr["agent"], json!(A1));
    assert_eq!(wr["role"], json!("agent"));
    for texto in [status.to_string(), whoami.to_string()] {
        assert!(!texto.contains(sentinela_motivo), "{texto}");
        assert!(!texto.contains("gt_"), "token no status/whoami: {texto}");
    }

    let diag = fs::read_to_string(&srv.diag).unwrap();
    assert!(
        !diag.contains("SENTINELATOKEN"),
        "token no log de diagnostico: {diag}"
    );
    assert!(
        !diag.contains(sentinela_motivo),
        "motivo no log de diagnostico: {diag}"
    );
    assert!(!diag.contains("gt_"), "token no log de diagnostico: {diag}");
}

// G32 (B): `validate` offline (6A). Controle: log consistente sai com 0 e imprime `ok t_max=<RFC3339>`
// (com --json: {"ok":true,"t_max":"..."}); o t_max e o ultimo ts do log (relogio 0 s). Cada corrupcao sai
// com 4 (com --json: {"ok":false,"error":"..."} na saida padrao).
#[test]
fn g32_validate_pega_active_sem_until_delegacao_fora_do_pai_e_cascata_incompleta() {
    // Log gerado pela biblioteca (mesmo formato do disco), com um revoke em cascata no fim.
    let d = Disco::novo();
    let r = Relogio::novo();
    let mut s = sobe(&d, &r, &grants_de(&perfis_delegacao()));
    let g1 = id_de(&pede_dono(
        &mut s,
        "grant",
        com(
            allow(A1, "fs.read", "/p/**"),
            json!({"until": em(3600), "delegable": true, "request_id": novo_rid()}),
        ),
    ));
    let g2 = id_de(&pede_como(
        &mut s,
        A1,
        "delegate",
        json!({"parent": g1, "grantee": A2, "action": "fs.read", "resource": "/p/x", "until": em(1800), "request_id": novo_rid()}),
    ));
    resultado(&pede_dono(
        &mut s,
        "revoke",
        json!({"id": g1, "reason": "fim", "request_id": novo_rid()}),
    ));
    let base = d.texto();
    drop(s);

    let valida = |conteudo: &str, como_json: bool| -> Output {
        let t = tempfile::tempdir().unwrap();
        let estado = t.path().join("estado");
        fs::create_dir(&estado).unwrap();
        fs::set_permissions(&estado, fs::Permissions::from_mode(0o700)).unwrap();
        grava_privado(&estado.join("log.jsonl"), conteudo);
        let mut cmd = Command::new(BIN);
        cmd.args(["validate", "--state-dir"]).arg(&estado);
        if como_json {
            cmd.arg("--json");
        }
        let saida = cmd.output().unwrap();
        drop(t);
        saida
    };

    let ok = valida(&base, false);
    assert_eq!(
        ok.status.code(),
        Some(0),
        "controle: log consistente: {ok:?}"
    );
    let texto = String::from_utf8(ok.stdout.clone()).unwrap();
    let t_max = texto
        .trim()
        .strip_prefix("ok t_max=")
        .unwrap_or_else(|| panic!("saida sem `ok t_max=`: {texto}"));
    assert_eq!(
        chrono::DateTime::parse_from_rfc3339(t_max)
            .unwrap()
            .with_timezone(&Utc),
        no(0),
        "t_max = ultimo ts do log: {texto}"
    );

    let ok_json = valida(&base, true);
    assert_eq!(ok_json.status.code(), Some(0), "{ok_json:?}");
    let v: Value = serde_json::from_slice(&ok_json.stdout).expect("--json e JSON");
    assert_eq!(v["ok"], json!(true), "{v}");
    assert_eq!(v["t_max"].as_str(), Some(t_max), "{v}");

    let mut linhas_sem_until: Vec<Value> = base
        .lines()
        .map(|l| serde_json::from_str(l).unwrap())
        .collect();
    for linha in linhas_sem_until.iter_mut() {
        for e in linha["tx"].as_array_mut().unwrap().iter_mut() {
            if e["e"] == json!("grant") && e["id"] == json!(g1) {
                e.as_object_mut().unwrap().remove("until");
            }
        }
    }
    let sem_until: String = linhas_sem_until.iter().map(|l| format!("{l}\n")).collect();

    let mut linhas_alem: Vec<Value> = base
        .lines()
        .map(|l| serde_json::from_str(l).unwrap())
        .collect();
    for linha in linhas_alem.iter_mut() {
        for e in linha["tx"].as_array_mut().unwrap().iter_mut() {
            if e["e"] == json!("grant") && e["id"] == json!(g2) {
                e["until"] = json!(em(7200));
            }
        }
    }
    let alem: String = linhas_alem.iter().map(|l| format!("{l}\n")).collect();

    let mut linhas_incompleta: Vec<Value> = base
        .lines()
        .map(|l| serde_json::from_str(l).unwrap())
        .collect();
    let ultima = linhas_incompleta.last_mut().unwrap();
    let tx = ultima["tx"].as_array_mut().unwrap();
    tx.retain(|e| !(e["e"] == json!("revoke") && e["id"] == json!(g2)));
    let incompleta: String = linhas_incompleta.iter().map(|l| format!("{l}\n")).collect();

    for (nome, conteudo) in [
        ("active sem until", &sem_until),
        ("delegacao alem do pai", &alem),
        ("cascata incompleta", &incompleta),
    ] {
        let simples = valida(conteudo, false);
        assert_eq!(simples.status.code(), Some(4), "{nome}: {simples:?}");
        let jsonf = valida(conteudo, true);
        assert_eq!(jsonf.status.code(), Some(4), "{nome}: {jsonf:?}");
        let v: Value = serde_json::from_slice(&jsonf.stdout).expect("--json de falha e JSON");
        assert_eq!(v["ok"], json!(false), "{nome}: {v}");
        assert!(
            v["error"].as_str().is_some_and(|e| !e.is_empty()),
            "{nome}: {v}"
        );
    }
}

// G34 (B): `check` com o daemon fora (socket inexistente) sai com 5. O check exige --token-file (6A). Controle: com o daemon no ar, o
// mesmo `check` sai com 0 e imprime `allow G-...`; sem concessao, sai com 3 e imprime `deny no_grant`.
#[test]
fn g34_check_com_daemon_fora_sai_5() {
    let t = tempfile::tempdir().unwrap();
    let tok = t.path().join("a1.token");
    grava_privado(&tok, &token_de(A1));
    let sock_morto = t.path().join("nao-existe.sock");
    let fora = Command::new(BIN)
        .args([
            "check",
            "--agent",
            A1,
            "--action",
            "fs.read",
            "--resource",
            "/p/x",
            "--token-file",
            tok.to_str().unwrap(),
            "--socket",
        ])
        .arg(&sock_morto)
        .output()
        .unwrap();
    assert_eq!(fora.status.code(), Some(5), "daemon fora: {fora:?}");

    let srv = sobe_servidor(&grants_de(&perfis_padrao()));
    let mut c = Conexao::abre(&srv.sock);
    let id = id_de(&c.pede(&tok_dono(), "grant", concessao_real(A1, "fs.read", "/p/x")));

    let check = |recurso: &str| -> Output {
        Command::new(BIN)
            .args([
                "check",
                "--agent",
                A1,
                "--action",
                "fs.read",
                "--resource",
                recurso,
                "--token-file",
                tok.to_str().unwrap(),
                "--socket",
            ])
            .arg(&srv.sock)
            .output()
            .unwrap()
    };
    let allow = check("/p/x");
    assert_eq!(allow.status.code(), Some(0), "{allow:?}");
    assert!(
        String::from_utf8_lossy(&allow.stdout).starts_with(&format!("allow {id}")),
        "{allow:?}"
    );
    let deny = check("/q/y");
    assert_eq!(deny.status.code(), Some(3), "{deny:?}");
    assert!(
        String::from_utf8_lossy(&deny.stdout).contains("deny no_grant"),
        "{deny:?}"
    );
}

// Auxiliar (B): `grantd init DIR` cria o estado 0700, sem token em nenhum arquivo, com LEIA-ME.md; e idempotente.
#[test]
fn aux_init_cria_estado_sem_tokens_e_e_idempotente() {
    let t = tempfile::tempdir().unwrap();
    let dir = t.path().join("estado-novo");
    let primeira = Command::new(BIN).args(["init"]).arg(&dir).output().unwrap();
    assert_eq!(primeira.status.code(), Some(0), "{primeira:?}");
    assert_eq!(
        fs::metadata(&dir).unwrap().permissions().mode() & 0o777,
        0o700
    );
    let leia = dir.join("LEIA-ME.md");
    assert!(leia.exists(), "LEIA-ME.md ausente");

    let mut contem_token = false;
    for entrada in fs::read_dir(&dir).unwrap() {
        let p = entrada.unwrap().path();
        if p.is_file() && fs::read_to_string(&p).unwrap_or_default().contains("gt_") {
            contem_token = true;
        }
    }
    assert!(!contem_token, "init nao pode gravar token");

    let antes = fs::read(&leia).unwrap();
    let segunda = Command::new(BIN).args(["init"]).arg(&dir).output().unwrap();
    assert_eq!(
        segunda.status.code(),
        Some(0),
        "init idempotente: {segunda:?}"
    );
    assert_eq!(
        fs::read(&leia).unwrap(),
        antes,
        "init repetido mudou o LEIA-ME.md"
    );
}

// Auxiliar (B): `grantd token-hash` le o token da entrada padrao e imprime o SHA-256 em hex
// (igual a `token_sha256_hex`). A entrada e a stdin (6A).
// Controle: outro token tem outro hash.
#[test]
fn aux_token_hash_bate_com_a_biblioteca() {
    let token = token_de(A1);
    let mut filho = Command::new(BIN)
        .args(["token-hash"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    filho
        .stdin
        .take()
        .unwrap()
        .write_all(token.as_bytes())
        .unwrap();
    let saida = filho.wait_with_output().unwrap();
    assert_eq!(saida.status.code(), Some(0), "{saida:?}");
    let impresso = String::from_utf8(saida.stdout).unwrap();
    assert_eq!(impresso.trim(), token_sha256_hex(&token));
    assert_ne!(impresso.trim(), token_sha256_hex(&token_de(A2)));
}

fn tamanho(p: &Path) -> u64 {
    fs::metadata(p).map(|m| m.len()).unwrap_or(0)
}

/// Espera (ate 5 s) o log de diagnostico crescer alem de `antes`.
fn espera_diag_cresce(p: &Path, antes: u64) {
    let inicio = Instant::now();
    while tamanho(p) <= antes {
        assert!(
            inicio.elapsed() < Duration::from_secs(5),
            "stderr nao registrou o erro de recarga"
        );
        sleep(Duration::from_millis(20));
    }
}

// G50 (B): SIGHUP com `grants.json` invalido (agent duplicado; role invalido) mantem os perfis anteriores
// (5A.13) e registra erro no stderr. Controles: a1 segue autorizado apos cada recarga invalida; um arquivo
// valido posterior (sem a1) e recarregado.
#[test]
fn g50_sighup_com_grants_invalido_mantem_perfis_anteriores() {
    let perfis = perfis_padrao();
    let srv = sobe_servidor(&grants_de(&perfis));
    let mut c = Conexao::abre(&srv.sock);
    resultado(&c.pede(&tok_dono(), "grant", concessao_real(A1, "fs.read", "/p/x")));
    let consulta = json!({"agent": A1, "action": "fs.read", "resource": "/p/x"});
    assert_decisao(
        &c.pede(&token_de(A1), "check", consulta.clone()),
        "allow",
        "ok",
    );

    // 1) agent duplicado.
    let antes = tamanho(&srv.diag);
    let mut dup: Vec<Value> = serde_json::from_str(&grants_de(&perfis)).unwrap();
    dup.push(dup[1].clone());
    grava_privado(&srv.grants, &Value::Array(dup).to_string());
    srv.sighup();
    espera_diag_cresce(&srv.diag, antes);
    assert_decisao(
        &c.pede(&token_de(A1), "check", consulta.clone()),
        "allow",
        "ok",
    );

    // 2) role invalido (a2).
    let antes = tamanho(&srv.diag);
    let mut inv: Vec<Value> = serde_json::from_str(&grants_de(&perfis)).unwrap();
    inv[2]["role"] = json!("root");
    grava_privado(&srv.grants, &Value::Array(inv).to_string());
    srv.sighup();
    espera_diag_cresce(&srv.diag, antes);
    assert_decisao(
        &c.pede(&token_de(A1), "check", consulta.clone()),
        "allow",
        "ok",
    );
    let quem = c.pede(&token_de(A2), "whoami", json!({}));
    assert_eq!(
        resultado(&quem)["agent"],
        json!(A2),
        "perfil anterior de a2: {quem}"
    );

    // 3) arquivo valido sem a1: e recarregado.
    let sem_a1: Vec<Value> = serde_json::from_str(&grants_de(
        &perfis
            .iter()
            .filter(|p| p.agente != A1)
            .map(|p| Perfil {
                agente: p.agente,
                papel: p.papel,
                pode_pedir: p.pode_pedir,
                pode_delegar: p.pode_delegar,
                expira: p.expira,
                token: p.token.clone(),
            })
            .collect::<Vec<_>>(),
    ))
    .unwrap();
    grava_privado(&srv.grants, &Value::Array(sem_a1).to_string());
    srv.sighup();
    let negado = espera_negado(&mut c, &token_de(A1), consulta);
    assert_eq!(codigo(&negado), "unauthorized", "{negado}");
}

/// Disputa: `n` conexoes concorrentes, cada uma com um `use` de `agente` e `request_id` novo.
fn disputa(
    sock: &Path,
    agente: &'static str,
    acao: &'static str,
    recurso: &'static str,
    n: usize,
) -> Vec<Value> {
    std::thread::scope(|sc| {
        let handles: Vec<_> = (0..n)
            .map(|_| {
                sc.spawn(move || {
                    let mut c = Conexao::abre(sock);
                    c.pede(
                        &token_de(agente),
                        "use",
                        json!({"agent": agente, "action": acao, "resource": recurso, "request_id": novo_rid()}),
                    )
                })
            })
            .collect();
        handles
            .into_iter()
            .map(|h| h.join().expect("thread"))
            .collect()
    })
}

// G51 (B): N clientes concorrentes disputando 1 unidade (max_uses 1): exatamente 1 `allow` (ok, replayed
// false) e os demais `denied`. Controle: com max_uses 8, 8 clientes concorrentes obtem exatamente 8 `allow`
// e um 9o cliente, depois, recebe `denied`.
#[test]
fn g51_clientes_concorrentes_disputam_uma_unidade() {
    let perfis = perfis_padrao();
    let srv = sobe_servidor(&grants_de(&perfis));
    let mut c = Conexao::abre(&srv.sock);

    let um = com(
        concessao_real(A1, "fs.read", "/p/x"),
        json!({"constraints": {"max_uses": 1}}),
    );
    resultado(&c.pede(&tok_dono(), "grant", um));
    let oito = com(
        concessao_real(A2, "fs.read", "/q/x"),
        json!({"constraints": {"max_uses": 8}}),
    );
    resultado(&c.pede(&tok_dono(), "grant", oito));

    let rodada = disputa(&srv.sock, A1, "fs.read", "/p/x", 8);
    let permitidos = rodada.iter().filter(|v| v["ok"] == json!(true)).count();
    assert_eq!(permitidos, 1, "exatamente 1 allow: {rodada:?}");
    for v in rodada.iter().filter(|v| v["ok"] == json!(false)) {
        assert_eq!(codigo(v), "denied", "{v}");
    }
    let vencedor = rodada.iter().find(|v| v["ok"] == json!(true)).unwrap();
    assert_eq!(resultado(vencedor)["replayed"], json!(false), "{vencedor}");

    let rodada2 = disputa(&srv.sock, A2, "fs.read", "/q/x", 8);
    let permitidos2 = rodada2.iter().filter(|v| v["ok"] == json!(true)).count();
    assert_eq!(
        permitidos2, 8,
        "controle: 8 allow com max_uses 8: {rodada2:?}"
    );
    let nono = c.pede(
        &token_de(A2),
        "use",
        json!({"agent": A2, "action": "fs.read", "resource": "/q/x", "request_id": novo_rid()}),
    );
    assert_eq!(codigo(&nono), "denied", "{nono}");
}

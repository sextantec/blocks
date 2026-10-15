#![allow(dead_code)]
//! Infra de teste de caixa preta do chatd. Fala so com o binario (CARGO_BIN_EXE_chatd),
//! com o socket (JSON-RPC por linha) e com os arquivos descritos na spec. Nao importa a crate.

use serde_json::{Value, json};
use std::fs;
use std::io::{BufRead, BufReader, ErrorKind, Read, Write};
use std::os::unix::fs::PermissionsExt;
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};
use tempfile::TempDir;

pub const BIN: &str = env!("CARGO_BIN_EXE_chatd");
pub const PRONTO: &str = "chatd: ready";

pub fn tmp() -> TempDir {
    tempfile::tempdir().expect("diretorio temporario")
}

/// Repete `cond` ate dar verdadeiro ou estourar `d`.
pub fn espera(d: Duration, mut cond: impl FnMut() -> bool) -> bool {
    let fim = Instant::now() + d;
    loop {
        if cond() {
            return true;
        }
        if Instant::now() >= fim {
            return false;
        }
        thread::sleep(Duration::from_millis(20));
    }
}

pub fn s(p: &Path) -> &str {
    p.to_str().expect("caminho UTF-8")
}

// ---------- configuracao ----------

/// Configuracao com o formato da spec (secao 7). `herdr` e o caminho do herdr falso.
pub fn config(herdr: &Path, poll_ms: u64, rearm_s: u64) -> Value {
    json!({
        "auth": "off",
        "herdr_bin": s(herdr),
        "bell_poll_ms": poll_ms,
        "bell_rearm_s": rearm_s,
        "recipients": {
            "codex-web": {"delivery": "bell", "pane": "w2:p1"},
            "claude-main": {"delivery": "watch"}
        }
    })
}

/// Config v1.1 com auth obrigatoria (padrao): sem a chave `auth`. Inclui `terceiro` (watch) para testes de isolamento.
pub fn config_req_json(herdr: &Path, poll_ms: u64, rearm_s: u64) -> Value {
    json!({
        "herdr_bin": s(herdr),
        "bell_poll_ms": poll_ms,
        "bell_rearm_s": rearm_s,
        "recipients": {
            "codex-web": {"delivery": "bell", "pane": "w2:p1"},
            "claude-main": {"delivery": "watch"},
            "terceiro": {"delivery": "watch"}
        }
    })
}

/// Diretorio com config de auth obrigatoria e herdr inexistente.
pub fn config_req(dir: &Path) -> PathBuf {
    escreve_config(
        dir,
        &config_req_json(Path::new("/nonexistent/chatd-herdr-x"), 5000, 600),
    )
}

pub fn escreve_config(dir: &Path, cfg: &Value) -> PathBuf {
    let p = dir.join("config.json");
    fs::write(&p, cfg.to_string()).expect("escrever config");
    p
}

/// Config para quem nao testa campainha: herdr inexistente e poll lento.
pub fn config_padrao(dir: &Path) -> PathBuf {
    escreve_config(
        dir,
        &config(Path::new("/nonexistent/chatd-herdr-x"), 5000, 600),
    )
}

// ---------- processos ----------

#[derive(Debug)]
pub struct Saida {
    pub code: Option<i32>,
    pub stdout: String,
    pub stderr: String,
}

pub struct Servidor {
    child: Child,
    pub state: PathBuf,
    pub socket: PathBuf,
    err: Arc<Mutex<String>>,
}

impl Servidor {
    /// Linhas de stderr recebidas ate agora.
    pub fn stderr(&self) -> String {
        self.err.lock().unwrap().clone()
    }

    /// Envia um sinal e espera ate 5 s. Devolve o codigo de saida, se saiu.
    pub fn sinal(&mut self, sig: libc::c_int) -> Option<i32> {
        unsafe { libc::kill(self.child.id() as libc::pid_t, sig) };
        self.espera_saida(Duration::from_secs(5))
    }

    /// Encerramento limpo: SIGTERM.
    pub fn para(&mut self) -> Option<i32> {
        self.sinal(libc::SIGTERM)
    }

    fn espera_saida(&mut self, d: Duration) -> Option<i32> {
        let fim = Instant::now() + d;
        loop {
            if let Ok(Some(st)) = self.child.try_wait() {
                return st.code();
            }
            if Instant::now() >= fim {
                return None;
            }
            thread::sleep(Duration::from_millis(20));
        }
    }
}

impl Drop for Servidor {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// Sobe `chatd` com `args`. Espera a linha `chatd: ready` (10 s). Se o processo sair antes, devolve a saida.
pub fn tenta(
    args: &[&str],
    envs: &[(&str, &str)],
    state: &Path,
    socket: &Path,
) -> Result<Servidor, Saida> {
    let mut child = Command::new(BIN)
        .args(args)
        .env_remove("CHATD_SOCKET")
        .env_remove("CHATD_STATE_DIR")
        .envs(envs.iter().copied())
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("executar o binario chatd");
    let out = child.stdout.take().unwrap();
    let errp = child.stderr.take().unwrap();
    let (tx, rx) = mpsc::channel::<String>();
    thread::spawn(move || {
        for l in BufReader::new(out).lines().map_while(Result::ok) {
            let _ = tx.send(l);
        }
    });
    let err = Arc::new(Mutex::new(String::new()));
    let err2 = Arc::clone(&err);
    thread::spawn(move || {
        for l in BufReader::new(errp).lines().map_while(Result::ok) {
            let mut g = err2.lock().unwrap();
            g.push_str(&l);
            g.push('\n');
        }
    });
    let fim = Instant::now() + Duration::from_secs(10);
    let mut vistos: Vec<String> = Vec::new();
    let mut pronto = false;
    loop {
        let restante = fim.saturating_duration_since(Instant::now());
        match rx.recv_timeout(restante) {
            Ok(l) => {
                let e_pronto = l == PRONTO;
                vistos.push(l);
                if e_pronto {
                    pronto = true;
                    break;
                }
            }
            Err(_) => break,
        }
    }
    if pronto {
        return Ok(Servidor {
            child,
            state: state.to_path_buf(),
            socket: socket.to_path_buf(),
            err,
        });
    }
    let _ = child.kill();
    let code = child.wait().ok().and_then(|st| st.code());
    thread::sleep(Duration::from_millis(50));
    let stderr = err.lock().unwrap().clone();
    Err(Saida {
        code,
        stdout: vistos.join("\n"),
        stderr,
    })
}

/// Sobe `chatd serve --state-dir STATE --config CFG` com socket padrao.
pub fn sobe(state: &Path, cfg: &Path) -> Servidor {
    let sock = state.join("chatd.sock");
    tenta(
        &["serve", "--state-dir", s(state), "--config", s(cfg)],
        &[],
        state,
        &sock,
    )
    .unwrap_or_else(|e| panic!("servidor nao ficou pronto: {e:?}"))
}

/// Diretorio temporario com config padrao e servidor no subdiretorio `state`.
pub fn sobe_padrao() -> (TempDir, Servidor) {
    let d = tmp();
    let cfg = config_padrao(d.path());
    let srv = sobe(&d.path().join("state"), &cfg);
    (d, srv)
}

/// Roda o binario ate sair (limite 10 s), com stdin opcional. Ambiente CHATD_* limpo.
pub fn cli(args: &[&str]) -> Saida {
    cli_env(args, &[], None)
}

pub fn cli_env(args: &[&str], envs: &[(&str, &str)], entrada: Option<&str>) -> Saida {
    let mut child = Command::new(BIN)
        .args(args)
        .env_remove("CHATD_SOCKET")
        .env_remove("CHATD_STATE_DIR")
        .envs(envs.iter().copied())
        .stdin(if entrada.is_some() {
            Stdio::piped()
        } else {
            Stdio::null()
        })
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("executar chatd");
    if let Some(t) = entrada {
        let mut si = child.stdin.take().unwrap();
        let _ = si.write_all(t.as_bytes());
    }
    let mut so = child.stdout.take().unwrap();
    let mut se = child.stderr.take().unwrap();
    let h1 = thread::spawn(move || {
        let mut b = String::new();
        let _ = so.read_to_string(&mut b);
        b
    });
    let h2 = thread::spawn(move || {
        let mut b = String::new();
        let _ = se.read_to_string(&mut b);
        b
    });
    let fim = Instant::now() + Duration::from_secs(10);
    let code = loop {
        if let Ok(Some(st)) = child.try_wait() {
            break st.code();
        }
        if Instant::now() >= fim {
            let _ = child.kill();
            let _ = child.wait();
            break None;
        }
        thread::sleep(Duration::from_millis(10));
    };
    Saida {
        code,
        stdout: h1.join().unwrap(),
        stderr: h2.join().unwrap(),
    }
}

// ---------- socket ----------

pub struct Conexao {
    leitor: BufReader<UnixStream>,
    escritor: UnixStream,
    prox: u64,
    /// Eventos (campo `params` das notificacoes) recebidos mas ainda nao consumidos.
    pub eventos_pendentes: Vec<Value>,
    /// True se alguma notificacao chegou antes da resposta da ultima chamada.
    pub notif_antes_da_resposta: bool,
}

impl Conexao {
    pub fn abre(socket: &Path) -> Conexao {
        let st = UnixStream::connect(socket)
            .unwrap_or_else(|e| panic!("conectar em {}: {e}", socket.display()));
        let escritor = st.try_clone().expect("clonar socket");
        Conexao {
            leitor: BufReader::new(st),
            escritor,
            prox: 1,
            eventos_pendentes: Vec::new(),
            notif_antes_da_resposta: false,
        }
    }

    pub fn envia_bruto(&mut self, linha: &str) {
        self.escritor
            .write_all(linha.as_bytes())
            .expect("escrever no socket");
        self.escritor
            .write_all(b"\n")
            .expect("escrever quebra de linha");
    }

    /// Proxima linha JSON do servidor; None em prazo esgotado ou fim da conexao.
    pub fn le(&mut self, prazo: Duration) -> Option<Value> {
        self.leitor
            .get_ref()
            .set_read_timeout(Some(prazo.max(Duration::from_millis(1))))
            .expect("definir timeout");
        let mut linha = String::new();
        match self.leitor.read_line(&mut linha) {
            Ok(0) | Err(_) => None,
            Ok(_) => Some(
                serde_json::from_str(linha.trim_end())
                    .unwrap_or_else(|e| panic!("linha do servidor nao e JSON: {linha:?}: {e}")),
            ),
        }
    }

    /// True se o servidor fechou a conexao dentro do prazo (False se so deu timeout).
    pub fn fechada_em(&mut self, prazo: Duration) -> bool {
        self.leitor
            .get_ref()
            .set_read_timeout(Some(prazo))
            .expect("definir timeout");
        let mut b = [0u8; 1];
        match self.leitor.read(&mut b) {
            Ok(0) => true,
            Ok(_) => false,
            Err(e) => !matches!(e.kind(), ErrorKind::WouldBlock | ErrorKind::TimedOut),
        }
    }

    /// Envia uma requisicao e devolve a resposta (result ou error). Notificacoes recebidas no meio vao para `eventos_pendentes`.
    pub fn chama(&mut self, metodo: &str, params: Value) -> Value {
        let id = self.prox;
        self.prox += 1;
        let req = json!({"jsonrpc": "2.0", "id": id, "method": metodo, "params": params});
        self.envia_bruto(&req.to_string());
        self.espera_resposta(metodo)
    }

    fn espera_resposta(&mut self, ctx: &str) -> Value {
        self.notif_antes_da_resposta = false;
        loop {
            let v = self
                .le(Duration::from_secs(10))
                .unwrap_or_else(|| panic!("sem resposta a {ctx}"));
            if v.get("method").is_some() {
                assert_eq!(
                    v["method"], "event",
                    "notificacao com metodo inesperado: {v}"
                );
                self.notif_antes_da_resposta = true;
                self.eventos_pendentes.push(v["params"].clone());
                continue;
            }
            return v;
        }
    }

    /// Proximo evento (params da notificacao `event`), ou None se nao chegar no prazo / conexao fechada.
    pub fn evento(&mut self, prazo: Duration) -> Option<Value> {
        if !self.eventos_pendentes.is_empty() {
            return Some(self.eventos_pendentes.remove(0));
        }
        let v = self.le(prazo)?;
        assert_eq!(v["method"], "event", "resposta sem requisicao: {v}");
        Some(v["params"].clone())
    }

    /// Coleta eventos ate `pare` dar verdadeiro para um deles, ou ate o prazo.
    pub fn eventos_ate(
        &mut self,
        prazo: Duration,
        mut pare: impl FnMut(&Value) -> bool,
    ) -> Vec<Value> {
        let fim = Instant::now() + prazo;
        let mut v = Vec::new();
        while Instant::now() < fim {
            let restante = fim.saturating_duration_since(Instant::now());
            match self.evento(restante) {
                Some(e) => {
                    let fin = pare(&e);
                    v.push(e);
                    if fin {
                        break;
                    }
                }
                None => break,
            }
        }
        v
    }
}

pub fn rpc(socket: &Path, metodo: &str, params: Value) -> Value {
    Conexao::abre(socket).chama(metodo, params)
}

/// `result` da resposta; falha se houver erro.
pub fn res(r: &Value) -> Value {
    r.get("result")
        .cloned()
        .unwrap_or_else(|| panic!("esperava result, veio {r}"))
}

pub fn is_err(r: &Value) -> bool {
    r.get("error").is_some()
}

/// `error.code`, que precisa ser string. Falha se nao houver erro.
pub fn cod(r: &Value) -> String {
    match r.get("error").and_then(|e| e.get("code")) {
        Some(Value::String(c)) => c.clone(),
        outro => panic!("esperava error.code string, veio {outro:?} em {r}"),
    }
}

/// `error.data.path`.
pub fn caminho(r: &Value) -> String {
    r["error"]["data"]["path"]
        .as_str()
        .unwrap_or_else(|| panic!("sem error.data.path em {r}"))
        .to_string()
}

pub fn msg(id: &str, de: &str, para: &str, kind: &str, subject: &str, body: &str) -> Value {
    json!({"v": 1, "id": id, "from": de, "to": para, "kind": kind, "subject": subject, "body": body})
}

pub fn envia(socket: &Path, params: Value) -> Value {
    rpc(socket, "message.send", params)
}

/// Envia AVISO de claude-main para `para`; devolve o result (falha se houver erro).
pub fn envia_ok(socket: &Path, id: &str, para: &str, corpo: &str) -> Value {
    res(&envia(
        socket,
        msg(id, "claude-main", para, "AVISO", "assunto", corpo),
    ))
}

pub fn seq(v: &Value) -> i64 {
    v["seq"]
        .as_i64()
        .unwrap_or_else(|| panic!("sem seq em {v}"))
}

pub fn ack_json(socket: &Path, params: Value) -> Value {
    rpc(socket, "message.ack", params)
}

pub fn ack(socket: &Path, id: &str, by: &str, stage: &str) -> Value {
    ack_json(socket, json!({"v": 1, "id": id, "by": by, "stage": stage}))
}

pub fn ack_ok(socket: &Path, id: &str, by: &str, stage: &str) -> Value {
    res(&ack(socket, id, by, stage))
}

/// Mensagens de `message.list` para `para` (limite 200).
pub fn lista(socket: &Path, para: &str) -> Vec<Value> {
    res(&rpc(
        socket,
        "message.list",
        json!({"v": 1, "to": para, "limit": 200}),
    ))["messages"]
        .as_array()
        .cloned()
        .expect("messages e array")
}

pub fn status(socket: &Path) -> Value {
    res(&rpc(socket, "status", json!({"v": 1})))
}

/// Campo `bell` do destinatario no `status`.
pub fn bell(socket: &Path, nome: &str) -> String {
    status(socket)["recipients"][nome]["bell"]
        .as_str()
        .unwrap_or("<ausente>")
        .to_string()
}

// ---------- log ----------

pub fn log_bytes(state: &Path) -> Vec<u8> {
    fs::read(state.join("log.jsonl")).expect("ler log.jsonl")
}

pub fn log_linhas(state: &Path) -> Vec<String> {
    String::from_utf8(log_bytes(state))
        .expect("log em UTF-8")
        .lines()
        .map(String::from)
        .collect()
}

// ---------- herdr falso ----------

const SCRIPT: &str = r#"#!/bin/sh
D="$(dirname "$0")"
if [ "$1" = "agent" ] && [ "$2" = "get" ]; then
  if [ -f "$D/trava.flag" ]; then exec sleep 30; fi
  if [ -f "$D/get_falha.flag" ]; then exit 1; fi
  E="$(cat "$D/estado.txt" 2>/dev/null)"
  printf '{"id":"cli:agent:get","result":{"agent":{"pane_id":"%s","agent_status":"%s"},"type":"agent_info"}}\n' "$3" "$E"
  exit 0
fi
if [ "$1" = "agent" ] && [ "$2" = "prompt" ]; then
  { for a in "$@"; do printf '%s\t' "$a"; done; printf '\n'; } >> "$D/calls.log"
  if [ -f "$D/falha.flag" ]; then exit 1; fi
  exit 0
fi
exit 2
"#;

/// Herdr falso: script shell que grava o argv de cada `agent prompt` em calls.log.
/// Estado lido de estado.txt; flags: falha.flag (prompt falha), get_falha.flag (get falha), trava.flag (get dorme 30 s).
pub struct Herdr {
    pub dir: PathBuf,
    pub bin: PathBuf,
}

impl Herdr {
    pub fn novo(dir: &Path) -> Herdr {
        let bin = dir.join("herdr-falso.sh");
        fs::write(&bin, SCRIPT).expect("escrever herdr falso");
        fs::set_permissions(&bin, fs::Permissions::from_mode(0o755)).expect("chmod herdr falso");
        let h = Herdr {
            dir: dir.to_path_buf(),
            bin,
        };
        h.estado("working");
        h
    }

    pub fn estado(&self, e: &str) {
        fs::write(self.dir.join("estado.txt"), e).expect("escrever estado");
    }

    pub fn flag(&self, nome: &str, ligado: bool) {
        let p = self.dir.join(nome);
        if ligado {
            fs::write(&p, "").expect("criar flag");
        } else {
            let _ = fs::remove_file(&p);
        }
    }

    /// Argv de cada chamada registrada, sem o nome do programa.
    pub fn chamadas(&self) -> Vec<Vec<String>> {
        let t = fs::read_to_string(self.dir.join("calls.log")).unwrap_or_default();
        t.lines()
            .map(|l| {
                let mut v: Vec<String> = l.split('\t').map(String::from).collect();
                v.pop();
                v
            })
            .collect()
    }

    /// Texto (ultimo argumento) de cada `agent prompt`.
    pub fn textos(&self) -> Vec<String> {
        self.chamadas()
            .into_iter()
            .map(|mut c| c.pop().unwrap_or_default())
            .collect()
    }

    pub fn espera_chamadas(&self, n: usize, d: Duration) -> bool {
        espera(d, || self.chamadas().len() >= n)
    }
}

/// Servidor com campainha apontada para o herdr falso, no pane w2:p1 de codex-web.
pub fn amb_campainha(poll_ms: u64, rearm_s: u64) -> (TempDir, Servidor, Herdr) {
    let d = tmp();
    let h = Herdr::novo(d.path());
    let cfg = escreve_config(d.path(), &config(&h.bin, poll_ms, rearm_s));
    let srv = sobe(&d.path().join("state"), &cfg);
    (d, srv, h)
}

// ---------- v1.1: autenticacao e CLI com token ----------

/// Emite um token com `chatd token new NOME --state-dir STATE --out OUT` e devolve o token (trim).
pub fn token_new(state: &Path, name: &str, out: &Path) -> String {
    let r = cli(&[
        "token",
        "new",
        name,
        "--state-dir",
        s(state),
        "--out",
        s(out),
    ]);
    assert_eq!(r.code, Some(0), "token new: {r:?}");
    fs::read_to_string(out)
        .expect("ler arquivo do token")
        .trim()
        .to_string()
}

/// Conexao ja autenticada como `name` (espera o handshake dar certo).
pub fn conexao_auth(socket: &Path, name: &str, token: &str) -> Conexao {
    let mut c = Conexao::abre(socket);
    let r = c.chama("auth", json!({"v": 1, "name": name, "token": token}));
    assert!(!is_err(&r), "auth de {name} falhou: {r}");
    c
}

/// Erro `unauthenticated` (codigo) de uma tentativa de auth.
pub fn auth_tenta(socket: &Path, name: &str, token: &str) -> Value {
    Conexao::abre(socket).chama("auth", json!({"v": 1, "name": name, "token": token}))
}

/// Roda o binario com ambiente de cliente: token file, nome e socket (flag/env conforme o teste).
pub fn cli_com(args: &[&str], envs: &[(&str, &str)]) -> Saida {
    cli_env(args, envs, None)
}

/// Chaves de auth.json lidas do disco (so para checar o formato).
pub fn auth_json(state: &Path) -> Value {
    serde_json::from_str(&fs::read_to_string(state.join("auth.json")).expect("ler auth.json"))
        .expect("auth.json e JSON")
}

pub fn modo_de(p: &Path) -> u32 {
    fs::metadata(p).expect("metadata").permissions().mode() & 0o777
}

/// Conta eventos de um tipo no log.jsonl.
pub fn eventos_tipo(state: &Path, tipo: &str) -> Vec<Value> {
    log_linhas(state)
        .iter()
        .map(|l| serde_json::from_str::<Value>(l).expect("linha do log"))
        .filter(|v| v["type"] == tipo)
        .collect()
}

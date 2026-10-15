#![allow(dead_code)]
//! Infra de teste de caixa preta do agentd. Fala so com o binario (CARGO_BIN_EXE_agentd),
//! com o socket (JSON-RPC por linha) e com o herdr FALSO (script sh gravado em temporario).
//! Escrito so a partir de docs/spec.md (rev3); nao importa a crate. Nenhum herdr real, nenhuma rede.

use serde_json::{Value, json};
use std::fs;
use std::io::{BufRead, BufReader, ErrorKind, Read, Write};
use std::os::unix::fs::PermissionsExt;
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};
use tempfile::TempDir;

pub const BIN: &str = env!("CARGO_BIN_EXE_agentd");

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

// ---------- configuracao (spec secao 7) ----------

/// Configuracao de teste: herdr falso, poll/stale/gone/confirmacao pequenos. Um agente `claude` (pane w1:p1)
/// e um `codex-blocks` (pane w3:p1). `extra` e mesclado por cima (para campos de teste).
pub fn config(herdr: &Path, poll_ms: u64) -> Value {
    json!({
        "herdr_bin": s(herdr),
        "poll_ms": poll_ms,
        "source_timeout_ms": 400,
        "stale_after_ms": poll_ms * 3 + 400 + 200,
        "gone_after_s": 5,
        "idle_confirm_polls": 2,
        "idle_confirm_ms": 0,
        "ring": 4096,
        "agents": {
            "claude": {"kind": "claude", "sources": [{"type": "herdr", "pane": "w1:p1"}]},
            "codex-blocks": {"kind": "codex", "sources": [{"type": "herdr", "pane": "w3:p1"}]}
        }
    })
}

pub fn escreve_config(dir: &Path, cfg: &Value) -> PathBuf {
    let p = dir.join("config.json");
    fs::write(&p, cfg.to_string()).expect("escrever config");
    p
}

/// Config com `herdr_bin` apontando para o herdr falso e poll rapido.
pub fn config_com(dir: &Path, h: &Herdr, poll_ms: u64, ajuste: impl FnOnce(&mut Value)) -> PathBuf {
    let mut c = config(&h.bin, poll_ms);
    ajuste(&mut c);
    escreve_config(dir, &c)
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

    /// Encerramento limpo: SIGTERM, espera ate 5 s. Devolve o codigo de saida, se saiu.
    pub fn para(&mut self) -> Option<i32> {
        self.sinal(libc::SIGTERM)
    }

    pub fn viva(&mut self) -> bool {
        matches!(self.child.try_wait(), Ok(None))
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

/// Sobe `agentd serve --config CFG --state-dir STATE` (socket padrao = STATE/agentd.sock).
/// AMBIGUO: a spec nao diz qual linha de prontidao o `serve` imprime; o teste espera o socket aceitar conexao.
pub fn sobe(state: &Path, cfg: &Path) -> Servidor {
    sobe_com(state, cfg, &[])
}

pub fn sobe_com(state: &Path, cfg: &Path, extra: &[&str]) -> Servidor {
    let mut args = vec!["serve", "--config", s(cfg), "--state-dir", s(state)];
    args.extend_from_slice(extra);
    let mut child = Command::new(BIN)
        .args(&args)
        .env_remove("AGENTD_SOCKET")
        .env_remove("AGENTD_STATE_DIR")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("executar o binario agentd");
    let out = child.stdout.take().unwrap();
    let errp = child.stderr.take().unwrap();
    thread::spawn(move || {
        for _ in BufReader::new(out).lines().map_while(Result::ok) {}
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
    let sock = state.join("agentd.sock");
    let mut srv = Servidor {
        child,
        state: state.to_path_buf(),
        socket: sock.clone(),
        err,
    };
    // Espera o socket aceitar conexao (10 s). Se o processo sair antes, falha ja com a saida.
    let fim = Instant::now() + Duration::from_secs(10);
    loop {
        if UnixStream::connect(&sock).is_ok() {
            break;
        }
        if !srv.viva() {
            panic!("agentd saiu antes de aceitar conexao; stderr: {}", srv.stderr());
        }
        if Instant::now() >= fim {
            panic!("agentd nao aceitou conexao em 10 s; stderr: {}", srv.stderr());
        }
        thread::sleep(Duration::from_millis(20));
    }
    srv
}

/// Diretorio temporario com config de teste, herdr falso e servidor em `state/`.
pub fn amb(poll_ms: u64) -> (TempDir, Servidor, Herdr) {
    let d = tmp();
    let h = Herdr::novo(d.path());
    let cfg = config_com(d.path(), &h, poll_ms, |_| {});
    let srv = sobe(&d.path().join("state"), &cfg);
    (d, srv, h)
}

/// Roda o binario ate sair (limite 10 s), ambiente limpo, HOME e XDG_CONFIG_HOME no temporario.
pub fn cli(home: &Path, args: &[&str], envs: &[(&str, &str)]) -> Saida {
    let mut child = Command::new(BIN)
        .args(args)
        .env_remove("AGENTD_SOCKET")
        .env_remove("AGENTD_STATE_DIR")
        .env("HOME", home)
        .env("XDG_CONFIG_HOME", home.join("xdg"))
        .envs(envs.iter().copied())
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("executar agentd");
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

// ---------- socket (JSON-RPC por linha; spec secao 5) ----------

pub struct Conexao {
    leitor: BufReader<UnixStream>,
    escritor: UnixStream,
    prox: u64,
    /// Notificacoes `event` (params) recebidas e ainda nao consumidas.
    pub pendentes: Vec<Value>,
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
            pendentes: Vec::new(),
        }
    }

    /// Escreve bytes crus no socket (sem LF automatico).
    pub fn bruto(&mut self, bytes: &[u8]) {
        let _ = self.escritor.write_all(bytes);
    }

    /// Fecha so a escrita (o servidor ve EOF depois do que ja foi enviado).
    pub fn fecha_escrita(&mut self) {
        let _ = self.escritor.shutdown(std::net::Shutdown::Write);
    }

    pub fn envia_bruto(&mut self, linha: &str) {
        self.bruto(linha.as_bytes());
        self.bruto(b"\n");
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

    /// Envia uma requisicao com `id` e devolve a primeira resposta (com `id`). Notificacoes vao para `pendentes`.
    pub fn chama(&mut self, metodo: &str, params: Value) -> Value {
        let id = self.prox;
        self.prox += 1;
        // AMBIGUO: a spec nao diz se o pedido leva "jsonrpc":"2.0"; enviamos (convencao do chatd).
        let req = json!({"jsonrpc": "2.0", "id": id, "method": metodo, "params": params});
        self.envia_bruto(&req.to_string());
        self.espera_resposta()
    }

    fn espera_resposta(&mut self) -> Value {
        loop {
            let v = self
                .le(Duration::from_secs(10))
                .expect("sem resposta do agentd em 10 s");
            if v.get("method") == Some(&json!("event")) {
                self.pendentes.push(v["params"].clone());
                continue;
            }
            return v;
        }
    }

    /// Proximo evento (params), ou None se nao chegar no prazo / conexao fechada.
    pub fn evento(&mut self, prazo: Duration) -> Option<Value> {
        if !self.pendentes.is_empty() {
            return Some(self.pendentes.remove(0));
        }
        let v = self.le(prazo)?;
        assert_eq!(v["method"], "event", "notificacao inesperada: {v}");
        Some(v["params"].clone())
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

/// Envelope completo de `agent.get`: `{v, agent: Agente, epoch, head_seq}` (spec secao 5).
pub fn agente_envelope(socket: &Path, id: &str) -> Value {
    res(&rpc(socket, "agent.get", json!({"v": 1, "id": id})))
}

/// Objeto `Agente` de `agent.get` (campo `agent` do envelope).
pub fn agente(socket: &Path, id: &str) -> Value {
    agente_envelope(socket, id)["agent"].clone()
}

pub fn lista(socket: &Path) -> Vec<Value> {
    res(&rpc(socket, "agent.list", json!({"v": 1})))["agents"]
        .as_array()
        .cloned()
        .expect("agents e array")
}

pub fn status(socket: &Path) -> Value {
    res(&rpc(socket, "status", json!({"v": 1})))
}

/// Espera ate `cond(agente)` ser verdadeiro, lendo agent.get a cada 20 ms.
pub fn espera_agente(socket: &Path, id: &str, d: Duration, mut cond: impl FnMut(&Value) -> bool) -> Value {
    let fim = Instant::now() + d;
    loop {
        let a = agente(socket, id);
        if cond(&a) || Instant::now() >= fim {
            return a;
        }
        thread::sleep(Duration::from_millis(20));
    }
}

// ---------- herdr falso ----------

/// Script sh: registra o argv de cada chamada em calls.log, e obedece flags na propria pasta:
/// trava.flag (dorme 30 s), saida.flag (sai 1 com stderr sentinela), lixo.flag (stdout nao-JSON),
/// grande.flag (stdout de 1 MiB + 1 byte), dup.flag (ver resposta), stderr.flag (imprime sentinela no stderr),
/// filho.flag (deixa um descendente vivo). Sem flag: imprime resposta.json.
const SCRIPT: &str = r#"#!/bin/sh
D="$(dirname "$0")"
{ for a in "$@"; do printf '%s\t' "$a"; done; printf '\n'; } >> "$D/calls.log"
if [ -f "$D/trava.flag" ]; then exec sleep 30; fi
if [ -f "$D/filho.flag" ]; then sleep 30 & fi
if [ -f "$D/stderr.flag" ]; then echo "SENTINELA-STDERR-AGENTD" >&2; fi
if [ -f "$D/saida.flag" ]; then echo "SENTINELA-STDERR-AGENTD erro" >&2; exit 1; fi
if [ -f "$D/lixo.flag" ]; then echo "nao e json SENTINELA-LIXO"; exit 0; fi
if [ -f "$D/grande.flag" ]; then head -c 1048577 /dev/zero | tr '\0' 'a'; echo; exit 0; fi
N=$(cat "$D/n.txt" 2>/dev/null || echo 0)
P="$D/seq/$(printf '%04d' "$N")"
if [ -f "$P" ]; then
  echo $((N+1)) > "$D/n.txt"
  if grep -q '^__ERR__' "$P"; then echo "erro" >&2; exit 1; fi
  if grep -q '^__LIXO__' "$P"; then echo "lixo"; exit 0; fi
  if grep -q '^__TRAVA__' "$P"; then exec sleep 30; fi
  cat "$P"; exit 0
fi
cat "$D/resposta.json"
exit 0
"#;

/// Uma leitura da fila (ver `Herdr::fila`): envelope valido, falha de processo, lixo ou travamento.
pub enum Leitura {
    Ok(Vec<Value>),
    Erro,
    Lixo,
    Trava,
}

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
        h.resposta(&[]);
        h
    }

    /// Grava o envelope que o herdr falso imprime: `{"id":..,"result":{"agents":[...]}}`.
    pub fn resposta(&self, agentes: &[Value]) {
        let env = json!({"id": "cli:agent:list", "result": {"agents": agentes}});
        fs::write(self.dir.join("resposta.json"), env.to_string()).expect("escrever resposta");
    }

    /// Fila de leituras: cada chamada do herdr falso consome uma. Depois da fila, vale `resposta.json`
    /// (gravado aqui com `depois`). Chamar ANTES de subir o servidor, ou com o servidor parado.
    pub fn fila(&self, leituras: &[Leitura], depois: &[Value]) {
        let dir = self.dir.join("seq");
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).expect("criar fila");
        for (i, l) in leituras.iter().enumerate() {
            let conteudo = match l {
                Leitura::Ok(ag) => {
                    json!({"id": "cli:agent:list", "result": {"agents": ag}}).to_string()
                }
                Leitura::Erro => "__ERR__".to_string(),
                Leitura::Lixo => "__LIXO__".to_string(),
                Leitura::Trava => "__TRAVA__".to_string(),
            };
            fs::write(dir.join(format!("{:04}", i)), conteudo).expect("gravar leitura");
        }
        let _ = fs::write(self.dir.join("n.txt"), "0");
        self.resposta(depois);
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

    pub fn espera_chamadas(&self, n: usize, d: Duration) -> bool {
        espera(d, || self.chamadas().len() >= n)
    }
}

/// AgentInfo do herdr (spec secao 6) com os campos lidos; `extra` mescla campos adicionais.
pub fn info(pane: &str, agent: &str, status: &str, terminal: &str, seq: u64) -> Value {
    json!({
        "pane_id": pane,
        "agent": agent,
        "agent_status": status,
        "cwd": "/tmp/projeto",
        "state_change_seq": seq,
        "completion_seq": 0,
        "revision": 1,
        "terminal_id": terminal,
        "launch_pending": false,
        "screen_detection_skipped": false,
    })
}

/// Lista de um agente em `pane`: `[info]`, so o painel esperado.
pub fn painel(pane: &str, kind: &str, status: &str, seq: u64) -> Vec<Value> {
    vec![info(pane, kind, status, "t-1", seq)]
}

/// Linha de log (log.jsonl do state) nao e usada: o agentd nao persiste (spec 2). Os helpers ficam so no socket.

/// Grava a config com `claude` e `codex-blocks` apontando para o herdr falso; para testes de `kind`.
pub fn sem_fonte(v: &mut Value) {
    v["agents"] = json!({});
}

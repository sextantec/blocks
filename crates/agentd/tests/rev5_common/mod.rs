#![allow(dead_code)]
//! Harness rev5 (caixa preta) do agentd. Sobe o binario (`CARGO_BIN_EXE_agentd`) com `tmux_bin`
//! apontando para um tmux FALSO (`fake_tmux.py`, cenario num diretorio temporario), fala com o socket
//! (JSON por linha) e com a CLI. Nunca executa tmux, herdr, chatd ou agente reais; nada de rede.
//! Escrito a partir de `docs/spec-rev5.md` (secoes 4, 4A, 5, 6, 7, 8, 9, 11 e Anexos A e B).
//! Nao importa a crate e nao le `src/`.

use serde_json::{Map, Value, json};
use std::collections::BTreeSet;
use std::fs;
use std::io::{BufRead, BufReader, ErrorKind, Read, Write};
use std::os::unix::fs::PermissionsExt;
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use tempfile::TempDir;

pub const BIN: &str = env!("CARGO_BIN_EXE_agentd");

pub fn raiz_crate() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

pub fn fixtures() -> PathBuf {
    raiz_crate().join("rules/fixtures")
}

pub fn regras_padrao() -> PathBuf {
    raiz_crate().join("rules/default.json")
}

fn fake_py() -> PathBuf {
    raiz_crate().join("tests/rev5_common/fake_tmux.py")
}

pub fn s(p: &Path) -> &str {
    p.to_str().expect("caminho UTF-8")
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

pub fn ms_unix() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

// ---------- tmux falso: cenario ----------

/// Um painel como o tmux o listaria (campos da spec secao 4).
#[derive(Clone, Debug)]
pub struct Painel {
    pub id: String,
    pub pid: String,
    pub morto: bool,
    pub cmd: String,
    pub sessao: String,
    pub tag: Option<String>,
    pub titulo: String,
    pub ativ: u64,
    /// `pane_current_path` cru (o falso sanitiza com o modificador s| do formato, como o tmux).
    pub cwd: String,
}

impl Painel {
    /// Painel vivo, sessao `$1`, pid 4242, `window_activity` 1700000000, sem tag, titulo vazio.
    pub fn new(id: &str, cmd: &str) -> Painel {
        Painel {
            id: id.to_string(),
            pid: "4242".to_string(),
            morto: false,
            cmd: cmd.to_string(),
            sessao: "$1".to_string(),
            tag: None,
            titulo: String::new(),
            ativ: 1_700_000_000,
            cwd: "/work/proj".to_string(),
        }
    }
    pub fn tag(mut self, t: &str) -> Painel {
        self.tag = Some(t.to_string());
        self
    }
    pub fn sessao(mut self, s: &str) -> Painel {
        self.sessao = s.to_string();
        self
    }
    pub fn titulo(mut self, t: &str) -> Painel {
        self.titulo = t.to_string();
        self
    }
    pub fn pid(mut self, p: &str) -> Painel {
        self.pid = p.to_string();
        self
    }
    pub fn morto(mut self, m: bool) -> Painel {
        self.morto = m;
        self
    }
    pub fn cwd(mut self, c: &str) -> Painel {
        self.cwd = c.to_string();
        self
    }
    pub fn ativ(mut self, a: u64) -> Painel {
        self.ativ = a;
        self
    }
    pub fn json(&self) -> Value {
        json!({
            "pane_id": self.id,
            "pane_pid": self.pid,
            "pane_dead": if self.morto { "1" } else { "0" },
            "pane_dead_status": if self.morto { "0" } else { "" },
            "window_activity": self.ativ.to_string(),
            "pane_current_command": self.cmd,
            "session_id": self.sessao,
            "@agentd": self.tag.clone().unwrap_or_default(),
            "pane_current_path": self.cwd,
            "pane_title": self.titulo,
        })
    }
}

/// Um cenario do tmux falso: diretorio temporario com `cenario.json`, o wrapper `tmux` e o log.
pub struct Mundo {
    pub raiz: TempDir,
    pub cen: PathBuf,
    pub tmux: PathBuf,
    estado: Value,
    n_tela: u32,
}

impl Mundo {
    pub fn novo() -> Mundo {
        Mundo::novo_em("tmux")
    }

    /// Como `novo`, mas o wrapper recebe o nome `nome` (usado para testar caminhos hostis).
    pub fn novo_em(nome: &str) -> Mundo {
        let raiz = tempfile::tempdir().expect("diretorio temporario");
        let cen = raiz.path().join("cenario");
        fs::create_dir_all(cen.join("telas")).expect("criar cenario");
        let tmux = raiz.path().join(nome);
        if let Some(pai) = tmux.parent() {
            fs::create_dir_all(pai).expect("criar pasta do wrapper");
        }
        // python3 por caminho absoluto: o teste de PATH controlado (spec 15.4) nao pode depender do PATH.
        let wrapper = format!(
            "#!/bin/sh\nexec '{}' -I '{}' '{}' \"$@\"\n",
            python3_abs(),
            fake_py().display(),
            cen.display()
        );
        fs::write(&tmux, wrapper).expect("gravar wrapper do tmux falso");
        fs::set_permissions(&tmux, fs::Permissions::from_mode(0o755)).expect("chmod");
        let mut m = Mundo {
            raiz,
            cen,
            tmux,
            estado: json!({"paineis": [], "capturas": {}, "listar": {"modo": "ok"}}),
            n_tela: 0,
        };
        m.salva();
        m
    }

    fn salva(&mut self) {
        let tmp = self.cen.join("cenario.json.tmp");
        fs::write(&tmp, self.estado.to_string()).expect("gravar cenario");
        fs::rename(&tmp, self.cen.join("cenario.json")).expect("trocar cenario");
    }

    /// Define a lista de paineis que `list-panes` devolve.
    pub fn paineis(&mut self, ps: &[Painel]) {
        self.estado["paineis"] = Value::Array(ps.iter().map(Painel::json).collect());
        self.salva();
    }

    /// Define a tela que `capture-pane` devolve para `pane` (bytes crus, com escapes se houver).
    pub fn tela(&mut self, pane: &str, bytes: &[u8]) {
        self.n_tela += 1;
        let arq = self.cen.join("telas").join(format!("t{}.bin", self.n_tela));
        fs::write(&arq, bytes).expect("gravar tela");
        self.estado["capturas"][pane] = json!({"modo": "ok", "arquivo": arq.display().to_string()});
        self.salva();
    }

    /// Tela de uma fixture real (`rules/fixtures/KIND/NOME.screen`).
    pub fn tela_fixture(&mut self, pane: &str, kind: &str, nome: &str) {
        let bytes = tela_fixture(kind, nome);
        self.tela(pane, &bytes);
    }

    pub fn captura_falha(&mut self, pane: &str, falha: bool) {
        if falha {
            self.estado["capturas"][pane] = json!({"modo": "falha"});
        }
        self.salva();
    }

    /// Ajusta um campo de topo do cenario (`listar`, `stderr`, `anexo_falha`...).
    pub fn ajusta(&mut self, chave: &str, valor: Value) {
        self.estado[chave] = valor;
        self.salva();
    }

    pub fn listar(&mut self, modo: &str) {
        self.estado["listar"] = json!({"modo": modo});
        self.salva();
    }

    pub fn listar_com(&mut self, modo: &str, extra: Value) {
        let mut l = json!({"modo": modo});
        for (k, v) in extra.as_object().expect("objeto") {
            l[k] = v.clone();
        }
        self.estado["listar"] = l;
        self.salva();
    }

    /// Escreve uma linha no feed de eventos da sessao (`$N` ou `N`); o processo de anexo a repassa.
    pub fn feed(&self, sessao: &str, linha: &str) {
        let n = sessao.trim_start_matches('$');
        let p = self.cen.join(format!("eventos-{n}.txt"));
        let mut f = fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&p)
            .expect("abrir feed");
        f.write_all(format!("{linha}\n").as_bytes())
            .expect("escrever feed");
    }

    /// Todos os registros do log (uma linha JSON por registro).
    pub fn registros(&self) -> Vec<Value> {
        let t = fs::read_to_string(self.cen.join("chamadas.jsonl")).unwrap_or_default();
        t.lines()
            .filter(|l| !l.trim().is_empty())
            .map(|l| serde_json::from_str(l).expect("registro JSON"))
            .collect()
    }

    fn argvs(&self, ev: &str) -> Vec<Vec<String>> {
        self.registros()
            .iter()
            .filter(|r| r["ev"] == ev)
            .map(|r| {
                r["argv"]
                    .as_array()
                    .expect("argv")
                    .iter()
                    .map(|x| x.as_str().unwrap_or("").to_string())
                    .collect()
            })
            .collect()
    }

    /// Argv de cada chamada registrada (sem o `-S SOCKET`, quando houver).
    pub fn chamadas(&self) -> Vec<Vec<String>> {
        self.argvs("chamada")
    }

    /// Chamadas fora da lista fechada (o falso as recusa e registra).
    pub fn violacoes(&self) -> Vec<Vec<String>> {
        self.argvs("violacao")
    }

    /// Quantas vezes duas leituras (list-panes/capture-pane) estiveram em voo ao mesmo tempo.
    pub fn sobreposicoes(&self) -> usize {
        self.registros()
            .iter()
            .filter(|r| r["ev"] == "sobreposicao")
            .count()
    }

    /// Alvos `%N` de cada `capture-pane` chamado.
    pub fn capturas(&self) -> Vec<String> {
        self.chamadas()
            .iter()
            .filter(|a| a.first().map(|x| x == "capture-pane").unwrap_or(false))
            .filter_map(|a| a.last().cloned())
            .collect()
    }

    /// Numero de `list-panes` chamados.
    pub fn listagens(&self) -> usize {
        self.chamadas()
            .iter()
            .filter(|a| a.iter().any(|x| x == "list-panes"))
            .count()
    }

    /// Processos de anexo (`-C attach`) iniciados, em ordem.
    pub fn anexos(&self) -> Vec<Vec<String>> {
        self.chamadas()
            .into_iter()
            .filter(|a| a.iter().any(|x| x == "-C"))
            .collect()
    }

    /// Total de bytes que chegaram aos stdins dos processos de anexo.
    pub fn bytes_stdin(&self) -> u64 {
        self.registros()
            .iter()
            .filter(|r| r["ev"] == "stdin")
            .map(|r| r["n"].as_u64().unwrap_or(0))
            .sum()
    }
}

// ---------- fixtures e goldens ----------

pub fn tela_fixture(kind: &str, nome: &str) -> Vec<u8> {
    let p = fixtures().join(kind).join(format!("{nome}.screen"));
    fs::read(&p).unwrap_or_else(|e| panic!("ler fixture {}: {e}", p.display()))
}

pub fn titulo_fixture(kind: &str, nome: &str) -> String {
    let p = fixtures().join(kind).join(format!("{nome}.title"));
    match fs::read_to_string(&p) {
        Ok(t) => t.trim_end_matches(['\n', '\r']).to_string(),
        Err(_) => String::new(),
    }
}

/// Entradas `golden` de `rules/default.json`.
pub fn goldens() -> Vec<Value> {
    let t = fs::read_to_string(regras_padrao()).expect("ler default.json");
    let v: Value = serde_json::from_str(&t).expect("default.json e JSON");
    v["golden"].as_array().expect("golden e array").clone()
}

/// Id do agente, tag e comando em primeiro plano para cada `kind`.
pub fn agente_do_kind(kind: &str) -> (&'static str, &'static str, &'static str) {
    match kind {
        "claude" => ("claude", "claude", "claude"),
        "codex" => ("codex-blocks", "codex", "codex"),
        "pi" => ("pi", "pi", "pi"),
        outro => panic!("kind desconhecido no teste: {outro}"),
    }
}

// ---------- configuracao (spec rev5 secao 7) ----------

pub fn copia_regras(para: &Path, de: &Path) {
    fs::copy(de, para).expect("copiar regras");
    fs::set_permissions(para, fs::Permissions::from_mode(0o644)).expect("chmod regras");
}

/// Configuracao base: tmux falso, eventos desligados, poll pequeno, confirmacao de idle imediata.
/// `agentes`: (id, kind, tag). Campos podem ser trocados por `ajuste`.
pub fn cfg(m: &Mundo, regras: &Path, poll_ms: u64, agentes: &[(&str, &str, &str)]) -> Value {
    let mut ags = Map::new();
    for (id, kind, tag) in agentes {
        ags.insert(
            (*id).to_string(),
            json!({"kind": kind, "sources": [{"type": "tmux", "tag": tag}]}),
        );
    }
    // Spec secao 7: stale_after_ms > poll_ms + source_timeout_ms (e <= 300000) e safety_poll_ms >= poll_ms.
    // Os valores abaixo respeitam isso para qualquer poll_ms de 200 a 10000; testes que querem outro
    // stale ou outro safety sobrescrevem pelo `ajuste`.
    const TIMEOUT_MS: u64 = 400;
    let stale_ms = std::cmp::max(3000, poll_ms + TIMEOUT_MS + 2000);
    let safety_ms = std::cmp::max(5000, poll_ms);
    json!({
        "tmux_bin": s(&m.tmux),
        "tmux_socket": null,
        "rules_file": s(regras),
        "poll_ms": poll_ms,
        "events": false,
        "event_debounce_ms": 150,
        "safety_poll_ms": safety_ms,
        "source_timeout_ms": TIMEOUT_MS,
        "stale_after_ms": stale_ms,
        "gone_after_s": 5,
        "idle_confirm_polls": 2,
        "idle_confirm_ms": 0,
        "ring": 4096,
        "agents": Value::Object(ags)
    })
}

/// Agentes padrao dos testes de estado: claude (tag claude), codex-blocks (tag codex-blocks), pi (tag pi).
pub fn agentes_todos() -> Vec<(&'static str, &'static str, &'static str)> {
    vec![
        ("claude", "claude", "claude"),
        ("codex-blocks", "codex", "codex-blocks"),
        ("pi", "pi", "pi"),
    ]
}

/// Troca a entrada `id` de `c["agents"]` pela fonte `{type: tmux, cwd}` (spec rev5.3 secao 16).
pub fn agente_cwd(c: &mut Value, id: &str, kind: &str, cwd: &str) {
    c["agents"][id] = json!({"kind": kind, "sources": [{"type": "tmux", "cwd": cwd}]});
}

pub fn escreve_cfg(dir: &Path, c: &Value) -> PathBuf {
    let p = dir.join("config.json");
    fs::write(&p, c.to_string()).expect("gravar config");
    p
}

/// Sobe o daemon com `cfg(...)` mais `ajuste`, regras copiadas para o temporario do cenario.
pub fn inicia(
    m: &Mundo,
    poll_ms: u64,
    agentes: &[(&str, &str, &str)],
    ajuste: impl FnOnce(&mut Value),
) -> Servidor {
    let regras = m.raiz.path().join("regras.json");
    copia_regras(&regras, &regras_padrao());
    let mut c = cfg(m, &regras, poll_ms, agentes);
    ajuste(&mut c);
    let cfgp = escreve_cfg(m.raiz.path(), &c);
    sobe(&estado_dir(m), &cfgp)
}

pub fn estado_dir(m: &Mundo) -> PathBuf {
    let d = m.raiz.path().join("estado");
    fs::create_dir_all(&d).expect("criar estado");
    fs::set_permissions(&d, fs::Permissions::from_mode(0o700)).expect("chmod estado");
    d
}

// ---------- processos ----------

#[derive(Debug)]
pub struct Saida {
    pub code: Option<i32>,
    pub stdout: String,
    pub stderr: String,
}

/// Roda o binario ate sair; estoura em `limite` (code None). Ambiente com HOME temporario.
pub fn roda(home: &Path, cwd: Option<&Path>, args: &[&str], limite: Duration) -> Saida {
    let mut cmd = Command::new(BIN);
    cmd.args(args)
        .env_remove("AGENTD_SOCKET")
        .env_remove("AGENTD_STATE_DIR")
        .env("HOME", home)
        .env("XDG_CONFIG_HOME", home.join("xdg"))
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    if let Some(d) = cwd {
        cmd.current_dir(d);
    }
    let mut child = cmd.spawn().expect("executar agentd");
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
    let fim = Instant::now() + limite;
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

pub struct Servidor {
    child: Child,
    pub state: PathBuf,
    pub socket: PathBuf,
    err: Arc<Mutex<String>>,
}

impl Servidor {
    /// Tudo o que o daemon escreveu em stderr ate agora.
    pub fn stderr(&self) -> String {
        self.err.lock().unwrap().clone()
    }

    /// Envia SIGTERM e espera ate 5 s. Devolve o codigo de saida, se saiu.
    pub fn para(&mut self) -> Option<i32> {
        unsafe { libc::kill(self.child.id() as libc::pid_t, libc::SIGTERM) };
        self.espera_saida(Duration::from_secs(5))
    }

    pub fn viva(&mut self) -> bool {
        matches!(self.child.try_wait(), Ok(None))
    }

    pub fn espera_saida(&mut self, d: Duration) -> Option<i32> {
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

/// Sobe `agentd serve --config CFG --state-dir STATE` e espera o socket aceitar conexao (10 s).
pub fn sobe(state: &Path, cfg: &Path) -> Servidor {
    sobe_em(None, state, cfg)
}

pub fn sobe_em(cwd: Option<&Path>, state: &Path, cfg: &Path) -> Servidor {
    sobe_com(cwd, state, cfg, &[])
}

/// Como `sobe_em`, com variaveis de ambiente extras (ex.: PATH controlado, spec 15.4).
pub fn sobe_com(cwd: Option<&Path>, state: &Path, cfg: &Path, envs: &[(&str, &str)]) -> Servidor {
    let mut cmd = Command::new(BIN);
    cmd.args(["serve", "--config", s(cfg), "--state-dir", s(state)])
        .env_remove("AGENTD_SOCKET")
        .env_remove("AGENTD_STATE_DIR")
        .envs(envs.iter().copied())
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    if let Some(d) = cwd {
        cmd.current_dir(d);
    }
    let mut child = cmd.spawn().expect("executar agentd");
    let out = child.stdout.take().unwrap();
    let errp = child.stderr.take().unwrap();
    thread::spawn(
        move || {
            for _ in BufReader::new(out).lines().map_while(Result::ok) {}
        },
    );
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
    let fim = Instant::now() + Duration::from_secs(10);
    loop {
        if UnixStream::connect(&sock).is_ok() {
            break;
        }
        if !srv.viva() {
            // Espera a thread leitora de stderr esvaziar a saida antes de relatar.
            thread::sleep(Duration::from_millis(200));
            panic!(
                "agentd saiu antes de aceitar conexao; stderr: {}",
                srv.stderr().trim()
            );
        }
        if Instant::now() >= fim {
            panic!(
                "agentd nao aceitou conexao em 10 s; stderr: {}",
                srv.stderr()
            );
        }
        thread::sleep(Duration::from_millis(20));
    }
    srv
}

// ---------- socket (JSON por linha; spec rev4 secao 5) ----------

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

    pub fn bruto(&mut self, bytes: &[u8]) {
        let _ = self.escritor.write_all(bytes);
    }

    pub fn envia_bruto(&mut self, linha: &str) {
        self.bruto(linha.as_bytes());
        self.bruto(b"\n");
    }

    /// Proxima linha JSON do servidor; None em prazo esgotado ou conexao fechada.
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

    /// True se o servidor fechou a conexao dentro do prazo.
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

    /// Envia requisicao com `id` e devolve a primeira resposta com `id`; notificacoes vao para `pendentes`.
    pub fn chama(&mut self, metodo: &str, params: Value) -> Value {
        let id = self.prox;
        self.prox += 1;
        let req = json!({"jsonrpc": "2.0", "id": id, "method": metodo, "params": params});
        self.envia_bruto(&req.to_string());
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

    /// Proximo evento (params), ou None se nao chegar no prazo.
    pub fn evento(&mut self, prazo: Duration) -> Option<Value> {
        if !self.pendentes.is_empty() {
            return Some(self.pendentes.remove(0));
        }
        let v = self.le(prazo)?;
        if v.get("method") != Some(&json!("event")) {
            return None;
        }
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

/// `error.code` (string). Falha se nao houver erro.
pub fn cod(r: &Value) -> String {
    match r.get("error").and_then(|e| e.get("code")) {
        Some(Value::String(c)) => c.clone(),
        outro => panic!("esperava error.code string, veio {outro:?} em {r}"),
    }
}

/// Objeto `Agente` de `agent.get` (campo `agent` do envelope). Falha se o id nao existir.
pub fn agente(socket: &Path, id: &str) -> Value {
    res(&rpc(socket, "agent.get", json!({"v": 1, "id": id})))["agent"].clone()
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

/// `status.sources.tmux` (ok, last_ok_at, last_error, e `events` no modo de eventos).
pub fn fonte_tmux(socket: &Path) -> Value {
    status(socket)["sources"]["tmux"].clone()
}

/// Marcas do agente como conjunto ordenado.
pub fn marcas(a: &Value) -> BTreeSet<String> {
    a["marks"]
        .as_array()
        .map(|v| {
            v.iter()
                .filter_map(|x| x.as_str().map(String::from))
                .collect()
        })
        .unwrap_or_default()
}

pub fn marcas_de(v: &[&str]) -> BTreeSet<String> {
    v.iter().map(|x| x.to_string()).collect()
}

/// Espera ate `cond(agente)` ser verdadeiro (poll de 20 ms) ou o prazo; devolve o ultimo visto.
pub fn espera_agente(
    socket: &Path,
    id: &str,
    d: Duration,
    mut cond: impl FnMut(&Value) -> bool,
) -> Value {
    let fim = Instant::now() + d;
    loop {
        let a = agente(socket, id);
        if cond(&a) || Instant::now() >= fim {
            return a;
        }
        thread::sleep(Duration::from_millis(20));
    }
}

/// Todos os eventos retidos (desde `last_seq` 0) mais os que chegarem em 300 ms.
pub fn eventos_todos(socket: &Path) -> Vec<Value> {
    eventos_desde(socket, 0)
}

pub fn eventos_desde(socket: &Path, last_seq: u64) -> Vec<Value> {
    let st = status(socket);
    let epoch = st["epoch"].as_str().expect("epoch string").to_string();
    let mut c = Conexao::abre(socket);
    let r = c.chama(
        "events.resume",
        json!({"v": 1, "epoch": epoch, "last_seq": last_seq}),
    );
    if is_err(&r) {
        panic!("events.resume falhou: {r}");
    }
    let mut out = Vec::new();
    while let Some(e) = c.evento(Duration::from_millis(300)) {
        out.push(e);
    }
    out
}

/// Espera ate o estado publicado ser `estado` (ou o prazo). Devolve o ultimo `Agente` visto.
pub fn espera_estado(socket: &Path, id: &str, estado: &str, d: Duration) -> Value {
    espera_agente(socket, id, d, |a| a["state"] == estado)
}

/// Espera o `status.sources.tmux.ok` ser `ok`.
pub fn espera_fonte(socket: &Path, ok: bool, d: Duration) -> bool {
    espera(d, || fonte_tmux(socket)["ok"] == ok)
}

/// CLI sobre o socket: `agentd SUBCOMANDO ARGS... --socket S` (convencao do harness rev4).
pub fn cli(home: &Path, socket: &Path, args: &[&str]) -> Saida {
    let mut a: Vec<&str> = args.to_vec();
    a.push("--socket");
    a.push(s(socket));
    roda(home, None, &a, Duration::from_secs(10))
}

/// Une stdout e stderr de uma saida (para checagens de privacidade).
pub fn todo_texto(s: &Saida) -> String {
    format!("{}\n{}", s.stdout, s.stderr)
}

/// Linha de regras (a regra JSON) pelo id, do arquivo padrao.
pub fn regra_padrao(id: &str) -> Value {
    let t = fs::read_to_string(regras_padrao()).expect("ler regras");
    let v: Value = serde_json::from_str(&t).expect("JSON");
    for kind in ["claude", "codex", "pi"] {
        if let Some(rs) = v["agents"][kind]["rules"].as_array() {
            for r in rs {
                if r["id"] == id {
                    return r.clone();
                }
            }
        }
    }
    panic!("regra {id} nao existe no default.json");
}

/// Caminho absoluto do python3 encontrado no PATH do teste.
pub fn python3_abs() -> String {
    let path = std::env::var_os("PATH").unwrap_or_default();
    for dir in std::env::split_paths(&path) {
        let p = dir.join("python3");
        if p.is_file() {
            return p.display().to_string();
        }
    }
    panic!("python3 nao encontrado no PATH do teste");
}

/// Sentinela unica, para checar vazamento (spec R5).
pub fn sentinela(tag: &str) -> String {
    format!("SENTINELA-{tag}-{}", std::process::id())
}

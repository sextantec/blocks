//! v1.3 (docs/spec-v1.3-bind.md): `agent.bind`, `delivery: auto`, verificacao de ancestralidade,
//! campainha so no painel ligado, TTL, `bound`/`unbound`, e o caso real com `tmux -S`.
//! Integracao: so o binario e o socket; multiplexador FALSO (scripts) salvo o teste `t1_*`.
//! Devem falhar ate a implementacao chegar.
mod common;

use common::*;
use serde_json::{Value, json};
use std::fs;
use std::io::{BufRead, BufReader};
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};
use tempfile::TempDir;

const CODEX: &str = "codex-web";
const CLAUDE: &str = "claude-main";
/// Texto fixo da campainha (v1.1, secao C.3).
const TEXTO: &str = "[chat] 1 pendente(s). Rode: chatd inbox --take";

/// Tmux falso: `display-message -p -t PANE '#{pane_pid}'` le `pane_PANE`; `send-keys` vai para digita.log.
const TMUX_SH: &str = r#"#!/bin/sh
D="$(dirname "$0")"
echo "$*" >> "$D/calls_tmux.log"
case "$1" in
  display-message)
    if [ -f "$D/pane_$4" ]; then cat "$D/pane_$4"; exit 0; fi
    exit 1 ;;
  send-keys)
    echo "send-keys $*" >> "$D/digita.log"
    exit 0 ;;
esac
exit 2
"#;

/// Herdr falso: todo painel responde `idle`; `agent prompt PANE TEXTO` vai para digita.log.
const HERDR_SH: &str = r#"#!/bin/sh
D="$(dirname "$0")"
echo "$*" >> "$D/calls_herdr.log"
if [ "$1" = "agent" ] && [ "$2" = "get" ]; then
  printf '{"id":"cli:agent:get","result":{"agent":{"pane_id":"%s","agent_status":"idle"},"type":"agent_info"}}\n' "$3"
  exit 0
fi
if [ "$1" = "agent" ] && [ "$2" = "prompt" ]; then
  echo "prompt $3 $4" >> "$D/digita.log"
  exit 0
fi
exit 2
"#;

fn getppid() -> i32 {
    unsafe { libc::getppid() }
}

/// Multiplexador falso: scripts e o painel de cada pane (pid que o script devolve).
struct Mux {
    dir: PathBuf,
    tmux: PathBuf,
    herdr: PathBuf,
}

impl Mux {
    fn novo(dir: &Path) -> Mux {
        let tmux = dir.join("tmux-falso.sh");
        let herdr = dir.join("herdr-falso.sh");
        fs::write(&tmux, TMUX_SH).unwrap();
        fs::set_permissions(&tmux, fs::Permissions::from_mode(0o755)).unwrap();
        fs::write(&herdr, HERDR_SH).unwrap();
        fs::set_permissions(&herdr, fs::Permissions::from_mode(0o755)).unwrap();
        Mux {
            dir: dir.to_path_buf(),
            tmux,
            herdr,
        }
    }

    /// Declara que o painel `pane` tem `pid` como pid do painel (tmux `pane_pid`).
    fn painel_pid(&self, pane: &str, pid: i32) {
        fs::write(self.dir.join(format!("pane_{pane}")), pid.to_string()).unwrap();
    }

    fn digita(&self) -> Vec<String> {
        fs::read_to_string(self.dir.join("digita.log"))
            .unwrap_or_default()
            .lines()
            .map(String::from)
            .collect()
    }
}

/// Config v1.3: auth obrigatoria; codex com `delivery` dado; claude so watch; binarios falsos.
fn cfg_v13(dir: &Path, mux: &Mux, codex: Value) -> PathBuf {
    escreve_config(
        dir,
        &json!({
            "herdr_bin": s(&mux.herdr),
            "tmux_bin": s(&mux.tmux),
            "bell_poll_ms": 50,
            "bell_rearm_s": 600,
            "recipients": {
                CODEX: codex,
                CLAUDE: {"delivery": "watch"}
            }
        }),
    )
}

struct Amb {
    _d: TempDir,
    srv: Servidor,
    mux: Mux,
    tok_codex: String,
    tok_claude: String,
    tokfile_codex: PathBuf,
}

fn amb(codex: Value) -> Amb {
    let d = tmp();
    let mux = Mux::novo(d.path());
    let cfg = cfg_v13(d.path(), &mux, codex);
    let srv = sobe(&d.path().join("state"), &cfg);
    let tok_codex = token_new(&srv.state, CODEX, &d.path().join("codex.tok"));
    let tok_claude = token_new(&srv.state, CLAUDE, &d.path().join("claude.tok"));
    let tokfile_codex = d.path().join("codex.tok");
    Amb {
        _d: d,
        srv,
        mux,
        tok_codex,
        tok_claude,
        tokfile_codex,
    }
}

impl Amb {
    fn sock(&self) -> PathBuf {
        self.srv.socket.clone()
    }
    fn codex(&self) -> Conexao {
        conexao_auth(&self.srv.socket, CODEX, &self.tok_codex)
    }
    fn claude(&self) -> Conexao {
        conexao_auth(&self.srv.socket, CLAUDE, &self.tok_claude)
    }
    /// `agent.bind` pela conexao autenticada do codex.
    fn bind(&self, mux: &str, pane: &str) -> Value {
        let mut c = self.codex();
        c.chama("agent.bind", json!({"v": 1, "mux": mux, "pane": pane}))
    }
    /// Mensagem do claude para o codex (pela conexao autenticada do claude).
    fn manda_para_codex(&self, id: &str) {
        let mut c = self.claude();
        let r = c.chama(
            "message.send",
            json!({"v": 1, "id": id, "to": CODEX, "kind": "AVISO", "subject": "assunto", "body": "corpo"}),
        );
        assert!(!is_err(&r), "envio falhou: {r}");
    }
    fn eventos(&self, tipo: &str) -> Vec<Value> {
        eventos_tipo(&self.srv.state, tipo)
    }
    /// Espera a campainha digitar TEXTO no painel `pane` (tmux ou herdr falsos).
    fn digitou(&self, pane: &str, d: Duration) -> bool {
        espera(d, || {
            self.mux
                .digita()
                .iter()
                .any(|l| l.contains(pane) && l.contains(TEXTO))
        })
    }
}

/// Ligacao tmux do codex no painel `pane`, com o proprio processo de teste como ancestral (verificacao passa).
fn liga_no_proprio_ancestral(a: &Amb, pane: &str) {
    a.mux.painel_pid(pane, getppid());
    let r = a.bind("tmux", pane);
    assert!(!is_err(&r), "bind do painel {pane} deveria ser aceito: {r}");
}

/// `chatd bind` como processo filho (mantem a ligacao com --keepalive).
fn bind_filho(a: &Amb, pane: &str, extra: &[&str]) -> Child {
    let sock = a.sock();
    let mut args = vec!["bind", "--as", CODEX, "--mux", "tmux", "--pane", pane];
    args.extend_from_slice(extra);
    Command::new(BIN)
        .args(&args)
        .env_remove("CHATD_SOCKET")
        .env_remove("CHATD_STATE_DIR")
        .env_remove("CHATD_AS")
        .env_remove("CHATD_TOKEN_FILE")
        .env("CHATD_SOCKET", s(&sock))
        .env("CHATD_TOKEN_FILE", s(&a.tokfile_codex))
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("executar chatd bind")
}

/// Evento de `tipo` cujo texto (qualquer campo) contem `valor`.
fn tem_valor(evs: &[Value], valor: &str) -> bool {
    evs.iter().any(|e| e.to_string().contains(valor))
}

/// `chatd watch` com stdout e stderr lidos por threads.
struct Watch {
    child: Child,
    out: mpsc::Receiver<String>,
}

impl Watch {
    fn inicia(args: &[&str], envs: &[(&str, &str)]) -> Watch {
        let mut child = Command::new(BIN)
            .arg("watch")
            .args(args)
            .env_remove("CHATD_SOCKET")
            .env_remove("CHATD_STATE_DIR")
            .env_remove("CHATD_AS")
            .env_remove("CHATD_TOKEN_FILE")
            .envs(envs.iter().copied())
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .expect("executar watch");
        let out = child.stdout.take().unwrap();
        let (tx, rx) = mpsc::channel();
        thread::spawn(move || {
            for l in BufReader::new(out).lines().map_while(Result::ok) {
                if tx.send(l).is_err() {
                    break;
                }
            }
        });
        Watch { child, out: rx }
    }

    fn linhas_por(&self, d: Duration) -> Vec<String> {
        let fim = Instant::now() + d;
        let mut v = Vec::new();
        while Instant::now() < fim {
            match self.out.recv_timeout(fim - Instant::now()) {
                Ok(l) => v.push(l),
                Err(_) => break,
            }
        }
        v
    }
}

impl Drop for Watch {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

// ---------- A: autenticacao e identidade ----------

#[test]
fn a1_agent_bind_sem_autenticacao_e_unauthenticated() {
    let a = amb(json!({"delivery": "auto"}));
    a.mux.painel_pid("%1", getppid());
    let r = rpc(
        &a.sock(),
        "agent.bind",
        json!({"v": 1, "mux": "tmux", "pane": "%1"}),
    );
    assert_eq!(cod(&r), "unauthenticated", "{r}");
    assert!(
        a.eventos("bound").is_empty(),
        "nada pode ser ligado sem autenticacao"
    );
}

#[test]
fn a2_nome_da_ligacao_e_a_identidade_da_conexao() {
    // o campo `name` nao existe no contrato: o nome vem so da conexao autenticada (codex)
    let a = amb(json!({"delivery": "auto"}));
    a.mux.painel_pid("%1", getppid());
    let mut c = a.codex();
    let r = c.chama(
        "agent.bind",
        json!({"v": 1, "mux": "tmux", "pane": "%1", "name": CLAUDE}),
    );
    let v = res(&r);
    assert_eq!(v["name"], CODEX, "{v}");
    assert_eq!(v["mux"], "tmux");
    assert_eq!(v["pane"], "%1");
    assert_eq!(v["ttl_s"], 120, "{v}");
    let b = a.eventos("bound");
    assert_eq!(b.len(), 1, "{b:?}");
    assert_eq!(b[0]["name"], CODEX);
}

// ---------- B: verificacao ----------

#[test]
fn b1_painel_de_outro_processo_e_forbidden_e_denied() {
    let a = amb(json!({"delivery": "auto"}));
    // processo que nao e ancestral do chamador
    let mut alheio = Command::new("sleep").arg("60").spawn().unwrap();
    a.mux.painel_pid("%9", alheio.id() as i32);
    let r = a.bind("tmux", "%9");
    assert_eq!(cod(&r), "forbidden", "{r}");
    assert!(a.eventos("bound").is_empty(), "nada pode ser ligado");
    let negs = a.eventos("denied");
    assert!(
        negs.iter().any(|e| e["method"] == "agent.bind"),
        "denied com method agent.bind: {negs:?}"
    );
    let _ = alheio.kill();
    let _ = alheio.wait();
}

#[test]
fn b2_cli_bind_com_painel_alheio_sai_3() {
    let a = amb(json!({"delivery": "auto"}));
    let mut alheio = Command::new("sleep").arg("60").spawn().unwrap();
    a.mux.painel_pid("%9", alheio.id() as i32);
    let sock = a.sock();
    let tok = s(&a.tokfile_codex).to_string();
    let sock_s = s(&sock).to_string();
    let r = cli_com(
        &["bind", "--as", CODEX, "--mux", "tmux", "--pane", "%9"],
        &[
            ("CHATD_SOCKET", sock_s.as_str()),
            ("CHATD_TOKEN_FILE", tok.as_str()),
        ],
    );
    assert_eq!(
        r.code,
        Some(3),
        "verificacao recusada deve sair com 3: {r:?}"
    );
    let _ = alheio.kill();
    let _ = alheio.wait();
}

// ---------- C: campainha com ligacao ----------

#[test]
fn c1_ligacao_valida_habilita_campainha_no_painel_verificado() {
    let a = amb(json!({"delivery": "auto"}));
    liga_no_proprio_ancestral(&a, "%1");
    a.manda_para_codex("c1-1");
    assert!(
        a.digitou("%1", Duration::from_secs(5)),
        "campainha nao digitou no painel ligado: {:?}",
        a.mux.digita()
    );
}

#[test]
fn c2_sem_ligacao_delivery_auto_gera_bell_failed_unbound_e_nao_digita() {
    let a = amb(json!({"delivery": "auto"}));
    a.manda_para_codex("c2-1");
    thread::sleep(Duration::from_secs(3));
    assert!(
        a.mux.digita().is_empty(),
        "sem ligacao nada pode ser digitado: {:?}",
        a.mux.digita()
    );
    let falhas = a.eventos("bell_failed");
    let unbound: Vec<&Value> = falhas
        .iter()
        .filter(|e| e.to_string().contains("unbound"))
        .collect();
    assert_eq!(
        unbound.len(),
        1,
        "um bell_failed unbound por minuto: {falhas:?}"
    );
}

#[test]
fn c3_campainha_digita_so_no_painel_ligado_nunca_no_id_do_arquivo() {
    // delivery auto nao tem pane: qualquer digitacao precisa vir da ligacao verificada
    let a = amb(json!({"delivery": "auto"}));
    liga_no_proprio_ancestral(&a, "%1");
    a.manda_para_codex("c3-1");
    assert!(a.digitou("%1", Duration::from_secs(5)));
    let todas = a.mux.digita();
    assert!(!todas.is_empty());
    for l in &todas {
        assert!(l.contains("%1"), "digitacao fora do painel ligado: {l}");
        assert!(!l.contains("w2:p1"), "id de arquivo usado: {l}");
    }
}

// ---------- D: TTL e processo ----------

#[test]
#[ignore = "AMBIGUO: espera o TTL de 120 s (roda com --ignored)"]
fn d1_ttl_expira_em_120s_e_a_campainha_para() {
    let a = amb(json!({"delivery": "auto"}));
    liga_no_proprio_ancestral(&a, "%1");
    thread::sleep(Duration::from_secs(125));
    assert!(
        tem_valor(&a.eventos("unbound"), "expired"),
        "{:?}",
        a.eventos("unbound")
    );
    a.manda_para_codex("d1-1");
    thread::sleep(Duration::from_secs(2));
    assert!(
        a.mux.digita().iter().all(|l| !l.contains(TEXTO)),
        "ligacao expirada nao pode digitar"
    );
}

#[test]
#[ignore = "AMBIGUO: exige passar do TTL de 120 s com watch ativo (roda com --ignored)"]
fn d2_watch_ativo_renova_a_ligacao() {
    let a = amb(json!({"delivery": "auto"}));
    liga_no_proprio_ancestral(&a, "%1");
    let tok = s(&a.tokfile_codex).to_string();
    let sock = s(&a.sock()).to_string();
    let _w = Watch::inicia(
        &["--as", CODEX],
        &[
            ("CHATD_TOKEN_FILE", tok.as_str()),
            ("CHATD_SOCKET", sock.as_str()),
        ],
    );
    thread::sleep(Duration::from_secs(125));
    assert!(
        !tem_valor(&a.eventos("unbound"), "expired"),
        "watch ativo deve renovar o TTL"
    );
    a.manda_para_codex("d2-1");
    assert!(a.digitou("%1", Duration::from_secs(5)));
}

#[test]
fn d3_processo_morto_invalida_a_ligacao_na_hora() {
    let a = amb(json!({"delivery": "auto"}));
    a.mux.painel_pid("%1", getppid());
    let mut ch = bind_filho(&a, "%1", &["--keepalive"]);
    assert!(
        espera(Duration::from_secs(5), || !a.eventos("bound").is_empty()),
        "bound nao registrado"
    );
    let peer = a.eventos("bound")[0]["peer_pid"]
        .as_i64()
        .expect("peer_pid");
    assert_eq!(
        peer,
        i64::from(ch.id()),
        "peer_pid deve ser o pid do chatd bind"
    );
    unsafe { libc::kill(ch.id() as i32, libc::SIGKILL) };
    let _ = ch.wait();
    assert!(
        espera(Duration::from_secs(5), || tem_valor(
            &a.eventos("unbound"),
            "process_gone"
        )),
        "processo morto deve gerar unbound process_gone: {:?}",
        a.eventos("unbound")
    );
}

// ---------- E: pane fixo (v1.1) e prevalencia do bind ----------

#[test]
fn e1_bell_com_pane_fixo_funciona_sem_bind() {
    let a = amb(json!({"delivery": "bell", "pane": "w2:p1"}));
    a.manda_para_codex("e1-1");
    assert!(
        a.digitou("w2:p1", Duration::from_secs(5)),
        "pane fixo sem bind deve tocar: {:?}",
        a.mux.digita()
    );
}

#[test]
fn e2_com_bind_ativo_a_ligacao_prevalece_sobre_o_pane_do_arquivo() {
    let a = amb(json!({"delivery": "bell", "pane": "w2:p1"}));
    liga_no_proprio_ancestral(&a, "%1");
    a.manda_para_codex("e2-1");
    assert!(
        a.digitou("%1", Duration::from_secs(5)),
        "a ligacao deve prevalecer: {:?}",
        a.mux.digita()
    );
    thread::sleep(Duration::from_millis(300));
    assert!(
        a.mux.digita().iter().all(|l| !l.contains("w2:p1")),
        "pane do arquivo nao pode ser usado: {:?}",
        a.mux.digita()
    );
}

// ---------- F: log, watch e substituicao ----------

#[test]
fn f1_bound_e_unbound_sem_segredos_e_fora_do_watch() {
    let a = amb(json!({"delivery": "auto"}));
    liga_no_proprio_ancestral(&a, "%1");
    let b = a.eventos("bound");
    assert_eq!(b.len(), 1, "{b:?}");
    assert_eq!(b[0]["name"], CODEX);
    assert_eq!(b[0]["mux"], "tmux");
    assert_eq!(b[0]["pane"], "%1");
    assert!(b[0]["peer_pid"].is_number(), "{}", b[0]);
    let tok = s(&a.tokfile_codex).to_string();
    let sock = s(&a.sock()).to_string();
    let w = Watch::inicia(
        &["--as", CODEX],
        &[
            ("CHATD_TOKEN_FILE", tok.as_str()),
            ("CHATD_SOCKET", sock.as_str()),
        ],
    );
    thread::sleep(Duration::from_millis(700));
    a.manda_para_codex("f1-1");
    let linhas = w.linhas_por(Duration::from_secs(2));
    assert!(linhas.iter().any(|l| l.contains(" f1-1 ")), "{linhas:?}");
    assert!(
        linhas.iter().all(|l| !l.contains("bound")),
        "bound/unbound nao pode aparecer no watch: {linhas:?}"
    );
    let log = fs::read(a.srv.state.join("log.jsonl")).unwrap();
    assert!(
        !String::from_utf8_lossy(&log).contains(&a.tok_codex),
        "token no log"
    );
    let r = cli(&["log", "--state-dir", s(&a.srv.state)]);
    assert!(
        r.stdout.contains("bound"),
        "chatd log deve mostrar bound: {:?}",
        r.stdout
    );
}

#[test]
fn f2_uma_ligacao_por_nome_substitui_a_anterior() {
    let a = amb(json!({"delivery": "auto"}));
    liga_no_proprio_ancestral(&a, "%1");
    a.mux.painel_pid("%2", getppid());
    let r = a.bind("tmux", "%2");
    assert!(!is_err(&r), "{r}");
    let un = a.eventos("unbound");
    assert!(tem_valor(&un, "replaced"), "{un:?}");
    a.manda_para_codex("f2-1");
    assert!(
        a.digitou("%2", Duration::from_secs(5)),
        "{:?}",
        a.mux.digita()
    );
    thread::sleep(Duration::from_millis(300));
    assert!(
        a.mux.digita().iter().all(|l| !l.contains("%1")),
        "painel antigo nao pode receber: {:?}",
        a.mux.digita()
    );
}

#[test]
fn f3_cli_keepalive_sai_0_em_sigterm_e_desfaz_a_ligacao() {
    // AMBIGUO: a spec nao fixa o motivo do `unbound` por desligamento explicito (so expired|process_gone|replaced).
    let a = amb(json!({"delivery": "auto"}));
    a.mux.painel_pid("%1", getppid());
    let mut ch = bind_filho(&a, "%1", &["--keepalive"]);
    assert!(espera(Duration::from_secs(5), || !a
        .eventos("bound")
        .is_empty()));
    unsafe { libc::kill(ch.id() as i32, libc::SIGTERM) };
    let code = {
        let fim = Instant::now() + Duration::from_secs(5);
        loop {
            if let Ok(Some(st)) = ch.try_wait() {
                break st.code();
            }
            if Instant::now() >= fim {
                break None;
            }
            thread::sleep(Duration::from_millis(20));
        }
    };
    assert_eq!(code, Some(0), "SIGTERM deve sair com 0");
    assert!(
        espera(Duration::from_secs(3), || !a.eventos("unbound").is_empty()),
        "desligar deve registrar unbound"
    );
}

// ---------- X: codigos de saida do bind ----------

#[test]
fn x1_cli_sem_mux_detectavel_sai_2() {
    // AMBIGUO: "ambiguo" nao esta definido em detalhe; testamos so o caso sem nenhum multiplexador no ambiente.
    let a = amb(json!({"delivery": "auto"}));
    let sock = s(&a.sock()).to_string();
    let tok = s(&a.tokfile_codex).to_string();
    let r = Command::new(BIN)
        .args(["bind", "--as", CODEX, "--pane", "%1"])
        .env_remove("TMUX_PANE")
        .env_remove("TMUX")
        .env_remove("CHATD_SOCKET")
        .env_remove("CHATD_TOKEN_FILE")
        .env("CHATD_SOCKET", &sock)
        .env("CHATD_TOKEN_FILE", &tok)
        .output()
        .expect("executar bind");
    assert_eq!(r.status.code(), Some(2), "{r:?}");
}

#[test]
fn x2_daemon_indisponivel_sai_4() {
    let d = tmp();
    let sock = d.path().join("nao-existe.sock");
    let tok = d.path().join("t.tok");
    fs::write(&tok, "0".repeat(64)).unwrap();
    fs::set_permissions(&tok, fs::Permissions::from_mode(0o600)).unwrap();
    let r = cli_com(
        &["bind", "--as", CODEX, "--mux", "tmux", "--pane", "%1"],
        &[("CHATD_SOCKET", s(&sock)), ("CHATD_TOKEN_FILE", s(&tok))],
    );
    assert_eq!(r.code, Some(4), "daemon ausente deve sair com 4: {r:?}");
}

// ---------- T: tmux real (-S em socket temporario) ----------

#[test]
fn t1_tmux_real_bind_pelo_painel_e_campainha_digita_no_painel() {
    // Usa o tmux real do ambiente, em servidor isolado (-S), sem tocar sessoes do usuario.
    let tmux_ok = Command::new("tmux")
        .arg("-V")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false);
    if !tmux_ok {
        eprintln!("tmux ausente: teste pulado");
        return;
    }
    let d = tmp();
    let tsock = d.path().join("tmux.sock");
    let wrapper = d.path().join("tmux-real.sh");
    fs::write(
        &wrapper,
        format!("#!/bin/sh\nexec tmux -S '{}' \"$@\"\n", s(&tsock)),
    )
    .unwrap();
    fs::set_permissions(&wrapper, fs::Permissions::from_mode(0o755)).unwrap();
    let mux = Mux::novo(d.path());
    let cfg = escreve_config(
        d.path(),
        &json!({
            "herdr_bin": s(&mux.herdr),
            "tmux_bin": s(&wrapper),
            "bell_poll_ms": 50,
            "bell_rearm_s": 600,
            "recipients": {CODEX: {"delivery": "auto"}, CLAUDE: {"delivery": "watch"}}
        }),
    );
    let st = d.path().join("state");
    let srv = sobe(&st, &cfg);
    let _tok_codex = token_new(&st, CODEX, &d.path().join("codex.tok"));
    let tok_claude = token_new(&st, CLAUDE, &d.path().join("claude.tok"));
    let tokfile = d.path().join("codex.tok");

    // painel do tmux: o script faz o bind com o proprio TMUX_PANE e fica como pai do chatd
    let pane_sh = d.path().join("painel.sh");
    fs::write(
        &pane_sh,
        format!(
            "#!/bin/sh\n'{bin}' bind --as {CODEX} --mux tmux --pane \"$TMUX_PANE\" --keepalive\nsleep 300\n",
            bin = BIN
        ),
    )
    .unwrap();
    fs::set_permissions(&pane_sh, fs::Permissions::from_mode(0o755)).unwrap();
    let mut tmux_cmd = Command::new(&wrapper);
    let status = tmux_cmd
        .args(["new-session", "-d", "-s", "blk", "-x", "200", "-y", "50"])
        .arg(s(&pane_sh))
        .env("CHATD_SOCKET", s(&srv.socket))
        .env("CHATD_TOKEN_FILE", s(&tokfile))
        .status()
        .expect("tmux new-session");
    assert!(status.success(), "tmux new-session falhou");

    let bound = espera(Duration::from_secs(10), || {
        !eventos_tipo(&st, "bound").is_empty()
    });
    assert!(bound, "o bind dentro do painel real nao registrou `bound`");

    // pane id real do painel
    let out = Command::new(&wrapper)
        .args(["list-panes", "-a", "-F", "#{pane_id}"])
        .output()
        .expect("list-panes");
    let pane = String::from_utf8_lossy(&out.stdout)
        .lines()
        .next()
        .unwrap_or("")
        .trim()
        .to_string();
    assert!(pane.starts_with('%'), "pane id: {pane:?}");

    let mut claude = conexao_auth(&srv.socket, CLAUDE, &tok_claude);
    let r = claude.chama(
        "message.send",
        json!({"v": 1, "id": "t1-1", "to": CODEX, "kind": "AVISO", "subject": "s", "body": "b"}),
    );
    assert!(!is_err(&r), "{r}");

    let mut visto = false;
    let fim = Instant::now() + Duration::from_secs(10);
    while Instant::now() < fim {
        let cap = Command::new(&wrapper)
            .args(["capture-pane", "-p", "-t", &pane])
            .output()
            .expect("capture-pane");
        if String::from_utf8_lossy(&cap.stdout).contains("[chat] 1 pendente(s)") {
            visto = true;
            break;
        }
        thread::sleep(Duration::from_millis(200));
    }
    let _ = Command::new(&wrapper).arg("kill-server").status();
    drop(srv);
    assert!(visto, "a campainha nao digitou no painel tmux real");
}

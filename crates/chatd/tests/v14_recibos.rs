//! v1.4 (docs/spec-v1.4-recibos.md): `bind` no herdr com tipo de agente (Z1-Z5) e recibos visiveis
//! ao remetente (`watch --receipts`, `show`, `message.get`; A-H).
//! Integracao: so o binario e o socket; o herdr e um script FALSO. Sem tmux, sem rede, sem sessoes reais.
//! Devem falhar ate a implementacao chegar (e compilar com o binario atual).
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

const A: &str = "claude-main";
const B: &str = "codex-web";
const C: &str = "terceiro";
const SUBJ: &str = "assunto";
/// Texto fixo da campainha (v1.1, secao C.3).
const TEXTO: &str = "[chat] 1 pendente(s). Rode: chatd inbox --take";
const TMUX_INEXISTENTE: &str = "/nonexistent/chatd-tmux-v14";

/// Herdr falso: `agent get PANE` devolve tipo e cwd do painel (arquivos `tipo.txt` e `cwd.txt` ao lado do script);
/// `agent prompt PANE TEXTO` vai para digita.log. O tipo sai em `result.agent.agent` e o cwd em `result.agent.cwd`:
/// leitura nossa de "herdr agent get -> agent" (o contrato nao fixa o formato completo do herdr).
const HERDR_SH: &str = r#"#!/bin/sh
D="$(dirname "$0")"
echo "$*" >> "$D/calls_herdr.log"
if [ "$1" = "agent" ] && [ "$2" = "get" ]; then
  printf '{"id":"cli:agent:get","result":{"agent":{"pane_id":"%s","agent":"%s","cwd":"%s","agent_status":"idle"},"type":"agent_info"}}\n' "$3" "$(cat "$D/tipo.txt")" "$(cat "$D/cwd.txt")"
  exit 0
fi
if [ "$1" = "agent" ] && [ "$2" = "prompt" ]; then
  echo "prompt $3 $4" >> "$D/digita.log"
  exit 0
fi
exit 2
"#;

fn herdr_fake(dir: &Path) -> PathBuf {
    let p = dir.join("herdr-falso.sh");
    fs::write(&p, HERDR_SH).unwrap();
    fs::set_permissions(&p, fs::Permissions::from_mode(0o755)).unwrap();
    p
}

/// Config v1.4: herdr falso, tmux inexistente, campainha lenta (so Z usa campainha).
fn cfg_com(herdr: &Path, recipients: Value) -> Value {
    json!({
        "herdr_bin": s(herdr),
        "tmux_bin": TMUX_INEXISTENTE,
        "bell_poll_ms": 50,
        "bell_rearm_s": 600,
        "recipients": recipients
    })
}

/// Servidor com auth obrigatoria, diretorio temporario proprio e um token por nome.
struct Amb {
    d: TempDir,
    srv: Servidor,
    cfg: PathBuf,
}

impl Amb {
    fn novo(d: TempDir, cfg_json: Value, nomes: &[&str]) -> Amb {
        let cfg = escreve_config(d.path(), &cfg_json);
        let srv = sobe(&d.path().join("state"), &cfg);
        for &n in nomes {
            token_new(
                &d.path().join("state"),
                n,
                &d.path().join(format!("{n}.tok")),
            );
        }
        Amb { d, srv, cfg }
    }

    fn st(&self) -> PathBuf {
        self.d.path().join("state")
    }

    fn sock(&self) -> PathBuf {
        self.srv.socket.clone()
    }

    fn tok_path(&self, nome: &str) -> PathBuf {
        self.d.path().join(format!("{nome}.tok"))
    }

    fn conexao(&self, nome: &str) -> Conexao {
        let tok = fs::read_to_string(self.tok_path(nome))
            .expect("ler token")
            .trim()
            .to_string();
        conexao_auth(&self.srv.socket, nome, &tok)
    }

    /// Roda o CLI como `nome` (CHATD_TOKEN_FILE do nome, `--as` explicito no fim).
    fn cli_de(&self, nome: &str, args: &[&str]) -> Saida {
        let sock = s(&self.srv.socket).to_string();
        let tok = s(&self.tok_path(nome)).to_string();
        let mut v: Vec<&str> = args.to_vec();
        v.push("--as");
        v.push(nome);
        cli_env(
            &v,
            &[
                ("CHATD_SOCKET", sock.as_str()),
                ("CHATD_TOKEN_FILE", tok.as_str()),
            ],
            None,
        )
    }

    fn watch(&self, nome: &str, args: &[&str]) -> Watch {
        let sock = s(&self.srv.socket).to_string();
        let tok = s(&self.tok_path(nome)).to_string();
        let mut v: Vec<&str> = vec!["--as", nome];
        v.extend_from_slice(args);
        Watch::inicia(
            &v,
            &[
                ("CHATD_SOCKET", sock.as_str()),
                ("CHATD_TOKEN_FILE", tok.as_str()),
            ],
        )
    }

    /// Mensagem de `de` para `para` pela conexao autenticada de `de`.
    fn envia_de(&self, de: &str, id: &str, para: &str, subj: &str, corpo: &str) -> Value {
        let mut c = self.conexao(de);
        let r = c.chama(
            "message.send",
            json!({"v": 1, "id": id, "to": para, "kind": "AVISO", "subject": subj, "body": corpo}),
        );
        assert!(!is_err(&r), "envio {id} falhou: {r}");
        r
    }

    /// `inbox --take` como `nome`.
    fn take(&self, nome: &str) -> Saida {
        self.cli_de(nome, &["inbox", "--take"])
    }

    /// `ack` por CLI como `nome`; exige saida 0.
    fn ack(&self, nome: &str, id: &str, estagio: &str, resultado: Option<&str>) -> Saida {
        let mut v = vec!["ack", "--id", id, "--stage", estagio];
        if let Some(r) = resultado {
            v.push("--result");
            v.push(r);
            if r == "refused" || r == "failed" {
                v.push("--note");
                v.push("nota do teste");
            }
        }
        let r = self.cli_de(nome, &v);
        assert_eq!(r.code, Some(0), "ack {id} {estagio}: {r:?}");
        r
    }

    fn show(&self, nome: &str, id: &str, json_: bool) -> Saida {
        let mut v = vec!["show", "--id", id];
        if json_ {
            v.push("--json");
        }
        self.cli_de(nome, &v)
    }

    /// Seq do evento `tipo` (accepted, resulted...) de `id`, visto por `nome`.
    fn seq_de(&self, nome: &str, tipo: &str, id: &str) -> i64 {
        let mut c = self.conexao(nome);
        res(&c.chama("events.resume", json!({"v": 1, "last_seq": 0})));
        let evs = c.eventos_ate(Duration::from_secs(3), |e| {
            e["type"] == tipo && e["id"] == id
        });
        evs.iter()
            .find(|e| e["type"] == tipo && e["id"] == id)
            .unwrap_or_else(|| panic!("evento {tipo} de {id} ausente para {nome}: {evs:?}"))["seq"]
            .as_i64()
            .expect("seq numerico")
    }

    fn eventos(&self, tipo: &str) -> Vec<Value> {
        eventos_tipo(&self.st(), tipo)
    }

    fn bind_herdr(&self, nome: &str, pane: &str) -> Value {
        let mut c = self.conexao(nome);
        c.chama("agent.bind", json!({"v": 1, "mux": "herdr", "pane": pane}))
    }

    fn digita(&self) -> Vec<String> {
        fs::read_to_string(self.d.path().join("digita.log"))
            .unwrap_or_default()
            .lines()
            .map(String::from)
            .collect()
    }

    fn digitou(&self, pane: &str, d: Duration) -> bool {
        espera(d, || {
            self.digita()
                .iter()
                .any(|l| l.contains(pane) && l.contains(TEXTO))
        })
    }

    fn para(&mut self) {
        assert!(self.srv.para().is_some(), "daemon nao saiu com SIGTERM");
    }

    fn sobe_de_novo(&mut self) {
        let st = self.st();
        self.srv = sobe(&st, &self.cfg);
    }

    /// Troca marcas no log.jsonl (daemon parado), como a auditoria F: simula logs antigos com controles.
    fn troca_no_log(&self, pares: &[(&str, &str)]) {
        let p = self.st().join("log.jsonl");
        let mut t = fs::read_to_string(&p).expect("ler log.jsonl");
        for &(de, para) in pares {
            assert!(t.contains(de), "marca {de} ausente no log");
            t = t.replace(de, para);
        }
        fs::write(&p, t).expect("reescrever log.jsonl");
    }
}

/// Tres identidades em watch (A = claude-main, B = codex-web, C = terceiro), herdr falso inerte.
fn amb_padrao() -> Amb {
    let d = tmp();
    let herdr = herdr_fake(d.path());
    let cfg = cfg_com(
        &herdr,
        json!({
            "claude-main": {"delivery": "watch"},
            "codex-web": {"delivery": "watch"},
            "terceiro": {"delivery": "watch"}
        }),
    );
    Amb::novo(d, cfg, &[A, B, C])
}

/// Ambiente de Z: painel herdr com `tipo_painel` e cwd igual ao do processo de teste.
fn amb_z(recipients: Value, nomes: &[&str], tipo_painel: &str) -> Amb {
    let d = tmp();
    let herdr = herdr_fake(d.path());
    fs::write(d.path().join("tipo.txt"), tipo_painel).unwrap();
    let cwd = std::env::current_dir().expect("cwd do teste");
    fs::write(d.path().join("cwd.txt"), s(&cwd)).unwrap();
    let cfg = cfg_com(&herdr, recipients);
    Amb::novo(d, cfg, nomes)
}

/// `chatd watch` com stdout lido por thread.
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

    /// Todas as linhas que chegarem em `d` (ou ate o processo fechar a saida).
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

    /// Le ate ter `n` linhas RECIBO (ou estourar `d`), depois mais 800 ms para pegar as extras.
    fn recibos(&self, n: usize, d: Duration) -> Vec<String> {
        let fim = Instant::now() + d;
        let mut v: Vec<String> = Vec::new();
        while Instant::now() < fim && conta_recibo(&v) < n {
            match self.out.recv_timeout(fim - Instant::now()) {
                Ok(l) => v.push(l),
                Err(_) => break,
            }
        }
        v.extend(self.linhas_por(Duration::from_millis(800)));
        v
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

impl Drop for Watch {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn conta_recibo(v: &[String]) -> usize {
    v.iter().filter(|l| l.starts_with("RECIBO ")).count()
}

fn recibos_de(v: &[String]) -> Vec<String> {
    v.iter()
        .filter(|l| l.starts_with("RECIBO "))
        .cloned()
        .collect()
}

/// Par (seq, linha esperada) de um recibo: `RECIBO SEQ ID ESTAGIO TO`.
fn recibo(seq: i64, id: &str, estagio: &str, para: &str) -> (i64, String) {
    (seq, format!("RECIBO {seq} {id} {estagio} {para}"))
}

/// Linhas esperadas em ordem de seq.
fn esperados(mut v: Vec<(i64, String)>) -> Vec<String> {
    v.sort();
    v.into_iter().map(|(_, b)| b).collect()
}

/// Uma linha RECIBO confere com a base ou com a base seguida de ` | ASSUNTO` (a segunda forma so
/// quando o processo ja viu o `registered`; ver contrato, Parte B, regra 2).
fn confere_recibo(linha: &str, base: &str) -> bool {
    linha == base || linha == format!("{} | {}", base, SUBJ)
}

/// As linhas RECIBO de `linhas`, em ordem, batem uma a uma com `esperados` (e nao ha sobra).
fn assert_recibos(linhas: &[String], esp: &[String]) {
    let rec = recibos_de(linhas);
    assert_eq!(
        rec.len(),
        esp.len(),
        "numero de linhas RECIBO: {rec:?} (esperado {esp:?})"
    );
    for (l, b) in rec.iter().zip(esp) {
        assert!(confere_recibo(l, b), "linha {l:?} nao bate com {b:?}");
    }
}

/// Sem byte de controle cru (C0 exceto `\n`, DEL e C1).
fn sem_controles(t: &str) -> bool {
    !t.chars().any(|c| {
        let n = c as u32;
        (n < 0x20 && c != '\n') || (0x7f..=0x9f).contains(&n)
    })
}

// ---------- Z: correcao do bind no herdr (tipo do agente, v1.4 secao 0) ----------

#[test]
fn z1_nome_codex_blocks_com_painel_codex_e_ligado() {
    // Z1: agent ausente deriva "codex" de "codex-blocks"; painel codex, cwd igual: ligado e campainha digita.
    let a = amb_z(
        json!({
            "claude-main": {"delivery": "watch"},
            "codex-blocks": {"delivery": "auto"}
        }),
        &[A, "codex-blocks"],
        "codex",
    );
    let r = a.bind_herdr("codex-blocks", "w3:p1");
    assert!(!is_err(&r), "bind deveria ser aceito: {r}");
    assert!(
        a.eventos("bound")
            .iter()
            .any(|e| e["name"] == "codex-blocks"),
        "evento bound ausente"
    );
    a.envia_de(A, "z1-1", "codex-blocks", SUBJ, "corpo");
    assert!(
        a.digitou("w3:p1", Duration::from_secs(5)),
        "campainha nao digitou no painel ligado: {:?}",
        a.digita()
    );
}

#[test]
fn z2_painel_claude_para_codex_blocks_e_forbidden_e_denied() {
    // Z2: tipo do painel (claude) diferente do esperado (codex): recusado.
    let a = amb_z(
        json!({
            "claude-main": {"delivery": "watch"},
            "codex-blocks": {"delivery": "auto"}
        }),
        &[A, "codex-blocks"],
        "claude",
    );
    let r = a.bind_herdr("codex-blocks", "w3:p1");
    assert_eq!(cod(&r), "forbidden", "{r}");
    assert!(a.eventos("bound").is_empty(), "nada pode ser ligado");
    assert!(
        a.eventos("denied")
            .iter()
            .any(|e| e["method"] == "agent.bind"),
        "denied com method agent.bind ausente"
    );
}

#[test]
fn z3_agent_explicito_codex_para_custom_bot_e_ligado() {
    // Z3: `agent` explicito ("codex") vale para um nome sem prefixo de tipo ("custom-bot").
    let a = amb_z(
        json!({
            "claude-main": {"delivery": "watch"},
            "custom-bot": {"delivery": "auto", "agent": "codex"}
        }),
        &[A, "custom-bot"],
        "codex",
    );
    let r = a.bind_herdr("custom-bot", "w3:p1");
    assert!(!is_err(&r), "bind deveria ser aceito: {r}");
    assert!(
        a.eventos("bound").iter().any(|e| e["name"] == "custom-bot"),
        "evento bound ausente"
    );
}

#[test]
fn z4_agent_invalido_chatd_nao_sobe() {
    // Z4: `agent` fora de [a-z0-9_-]{1,32} e erro de configuracao: saida 1, sem "ready".
    let d = tmp();
    let herdr = herdr_fake(d.path());
    let cfg = escreve_config(
        d.path(),
        &cfg_com(
            &herdr,
            json!({"codex-blocks": {"delivery": "auto", "agent": "Co dex"}}),
        ),
    );
    let state = d.path().join("state");
    let sock = state.join("chatd.sock");
    match tenta(
        &["serve", "--state-dir", s(&state), "--config", s(&cfg)],
        &[],
        &state,
        &sock,
    ) {
        Ok(_) => panic!("chatd subiu com agent invalido"),
        Err(sai) => assert_eq!(sai.code, Some(1), "{sai:?}"),
    }
}

#[test]
fn z5_nome_claude_com_painel_claude_e_ligado_por_padrao() {
    // Z5: sem `agent`, "claude" deriva "claude" (trecho antes do primeiro "-", aqui o nome inteiro).
    let a = amb_z(
        json!({
            "claude-main": {"delivery": "watch"},
            "claude": {"delivery": "auto"}
        }),
        &[A, "claude"],
        "claude",
    );
    let r = a.bind_herdr("claude", "w3:p1");
    assert!(!is_err(&r), "bind deveria ser aceito: {r}");
    assert!(
        a.eventos("bound").iter().any(|e| e["name"] == "claude"),
        "evento bound ausente"
    );
}

// ---------- A: watch sem --receipts nao muda ----------

#[test]
fn a_watch_sem_receipts_nao_imprime_recibo() {
    // Sem --receipts: aceito e resultado acontecem e o watch so imprime as mensagens para a identidade.
    let a = amb_padrao();
    let w = a.watch(A, &[]);
    thread::sleep(Duration::from_millis(700));
    a.envia_de(B, "a-2", A, SUBJ, "para o claude");
    a.envia_de(A, "a-1", B, SUBJ, "para o codex");
    let t = a.take(B);
    assert_eq!(t.code, Some(0), "{t:?}");
    a.ack(B, "a-1", "resultado", Some("done"));
    let linhas = w.linhas_por(Duration::from_secs(2));
    assert!(
        recibos_de(&linhas).is_empty(),
        "sem --receipts nao pode haver RECIBO: {linhas:?}"
    );
    assert!(
        linhas.iter().any(|l| l.contains(" a-2 ")),
        "a mensagem para o claude deve sair como antes: {linhas:?}"
    );
}

// ---------- B: aceito e resultado done, uma linha por recibo, em ordem ----------

#[test]
fn b_watch_receipts_entrega_aceito_e_resultado_done_em_ordem() {
    // A linha pode ou nao levar ` | assunto` (regra 2: so se o processo ja viu o registered); confere_recibo aceita as duas.
    let a = amb_padrao();
    let w = a.watch(A, &["--receipts"]);
    thread::sleep(Duration::from_millis(700));
    a.envia_de(A, "b-1", B, SUBJ, "corpo");
    let t = a.take(B);
    assert_eq!(t.code, Some(0), "{t:?}");
    a.ack(B, "b-1", "resultado", Some("done"));
    let linhas = w.recibos(2, Duration::from_secs(5));
    let esp = esperados(vec![
        recibo(a.seq_de(A, "accepted", "b-1"), "b-1", "aceito", B),
        recibo(a.seq_de(A, "resulted", "b-1"), "b-1", "resultado:done", B),
    ]);
    assert_recibos(&linhas, &esp);
}

// ---------- C: refused e failed ----------

#[test]
fn c_resultado_refused_e_failed_aparecem_com_o_resultado() {
    let a = amb_padrao();
    let w = a.watch(A, &["--receipts"]);
    thread::sleep(Duration::from_millis(700));
    a.envia_de(A, "c-1", B, SUBJ, "um");
    a.envia_de(A, "c-2", B, SUBJ, "dois");
    let t = a.take(B);
    assert_eq!(t.code, Some(0), "{t:?}");
    a.ack(B, "c-1", "resultado", Some("refused"));
    a.ack(B, "c-2", "resultado", Some("failed"));
    let linhas = w.recibos(4, Duration::from_secs(5));
    let esp = esperados(vec![
        recibo(a.seq_de(A, "accepted", "c-1"), "c-1", "aceito", B),
        recibo(a.seq_de(A, "accepted", "c-2"), "c-2", "aceito", B),
        recibo(
            a.seq_de(A, "resulted", "c-1"),
            "c-1",
            "resultado:refused",
            B,
        ),
        recibo(a.seq_de(A, "resulted", "c-2"), "c-2", "resultado:failed", B),
    ]);
    assert_recibos(&linhas, &esp);
}

// ---------- D: isolamento entre identidades ----------

#[test]
fn d_recibos_alheios_nao_chegam_e_destinatario_nao_recebe_o_proprio_aceite() {
    let a = amb_padrao();
    let wa = a.watch(A, &["--receipts"]);
    let wb = a.watch(B, &["--receipts"]);
    thread::sleep(Duration::from_millis(700));
    // A -> B: A deve ver os recibos; B so o registered (o proprio aceite nao volta a ele).
    a.envia_de(A, "d-1", B, SUBJ, "de A para B");
    let t1 = a.take(B);
    assert_eq!(t1.code, Some(0), "{t1:?}");
    a.ack(B, "d-1", "resultado", Some("done"));
    // B -> C: entre B e C; A nao pode ver nada disso, B (remetente) ve os seus.
    a.envia_de(B, "d-2", C, SUBJ, "de B para C");
    let t2 = a.take(C);
    assert_eq!(t2.code, Some(0), "{t2:?}");
    a.ack(C, "d-2", "resultado", None);

    let la = wa.recibos(2, Duration::from_secs(5));
    let esp_a = esperados(vec![
        recibo(a.seq_de(A, "accepted", "d-1"), "d-1", "aceito", B),
        recibo(a.seq_de(A, "resulted", "d-1"), "d-1", "resultado:done", B),
    ]);
    assert_recibos(&la, &esp_a);

    let lb = wb.recibos(2, Duration::from_secs(5));
    let esp_b = esperados(vec![
        recibo(a.seq_de(B, "accepted", "d-2"), "d-2", "aceito", C),
        recibo(a.seq_de(B, "resulted", "d-2"), "d-2", "resultado:done", C),
    ]);
    assert_recibos(&lb, &esp_b);
    // A linha `registered` de d-1 e devida a B (contrato, regra 2); so o RECIBO nao.
    assert!(
        lb.iter()
            .all(|l| !(l.starts_with("RECIBO ") && l.contains(" d-1 "))),
        "B (destinatario de d-1) nao pode receber RECIBO de d-1: {lb:?}"
    );
}

// ---------- E: --resume-file, --exit-after e --reconnect ----------

#[test]
fn e_resume_file_entrega_uma_vez_e_reconnect_nao_duplica_com_reinicio() {
    let mut a = amb_padrao();
    let arq = a.d.path().join("cur.txt");
    let arq_s = s(&arq);

    // Execucao 1: arquivo ausente comeca no head; sai sozinha em 2 s.
    let mut w1 = a.watch(
        A,
        &["--receipts", "--resume-file", arq_s, "--exit-after", "2s"],
    );
    assert_eq!(
        w1.espera_saida(Duration::from_secs(8)),
        Some(0),
        "exit-after"
    );
    assert!(
        arq.exists(),
        "resume-file deve existir depois da 1a execucao"
    );

    // Entre as execucoes: aceito e resultado acontecem com o watch parado.
    a.envia_de(A, "e-1", B, SUBJ, "entre execucoes");
    let t = a.take(B);
    assert_eq!(t.code, Some(0), "{t:?}");
    a.ack(B, "e-1", "resultado", Some("done"));

    // Execucao 2: entrega os dois recibos, uma vez.
    let mut w2 = a.watch(
        A,
        &["--receipts", "--resume-file", arq_s, "--exit-after", "2s"],
    );
    assert_eq!(
        w2.espera_saida(Duration::from_secs(8)),
        Some(0),
        "exit-after 2"
    );
    let l2 = w2.linhas_por(Duration::from_millis(300));
    let esp = esperados(vec![
        recibo(a.seq_de(A, "accepted", "e-1"), "e-1", "aceito", B),
        recibo(a.seq_de(A, "resulted", "e-1"), "e-1", "resultado:done", B),
    ]);
    assert_recibos(&l2, &esp);

    // Execucao 3 sem novidades: nenhum recibo repetido.
    let mut w3 = a.watch(
        A,
        &["--receipts", "--resume-file", arq_s, "--exit-after", "2s"],
    );
    assert_eq!(
        w3.espera_saida(Duration::from_secs(8)),
        Some(0),
        "exit-after 3"
    );
    let l3 = w3.linhas_por(Duration::from_millis(300));
    assert!(recibos_de(&l3).is_empty(), "recibo repetido: {l3:?}");

    // --reconnect: o daemon reinicia entre o aceito e o resultado; cada recibo sai uma vez.
    let w4 = a.watch(A, &["--receipts", "--reconnect"]);
    thread::sleep(Duration::from_millis(700));
    a.envia_de(A, "e-2", B, SUBJ, "com reinicio");
    let t = a.take(B);
    assert_eq!(t.code, Some(0), "{t:?}");
    let mut todas = w4.recibos(1, Duration::from_secs(5));
    a.para();
    thread::sleep(Duration::from_millis(300));
    a.sobe_de_novo();
    thread::sleep(Duration::from_millis(2500));
    a.ack(B, "e-2", "resultado", Some("done"));
    todas.extend(w4.recibos(1, Duration::from_secs(8)));
    let esp4 = esperados(vec![
        recibo(a.seq_de(A, "accepted", "e-2"), "e-2", "aceito", B),
        recibo(a.seq_de(A, "resulted", "e-2"), "e-2", "resultado:done", B),
    ]);
    assert_recibos(&todas, &esp4);
}

// ---------- F: show e message.get ----------

#[test]
fn f_show_mostra_estagios_e_alheia_e_inexistente_saem_iguais() {
    let a = amb_padrao();
    a.envia_de(A, "f-1", B, SUBJ, "corpo-secreto-f1");

    // Antes do take: so o estagio registrado.
    let pre = a.show(A, "f-1", false);
    assert_eq!(pre.code, Some(0), "{pre:?}");
    let l_pre: Vec<&str> = pre.stdout.lines().collect();
    assert_eq!(l_pre.len(), 2, "{:?}", pre.stdout);
    assert_eq!(l_pre[0], format!("f-1 registrado {A}>{B} AVISO | {SUBJ}"));
    assert!(
        !pre.stdout.contains("corpo-secreto-f1"),
        "corpo nao sai sem --json"
    );

    let t = a.take(B);
    assert_eq!(t.code, Some(0), "{t:?}");
    a.ack(B, "f-1", "resultado", Some("done"));

    let mut ca = a.conexao(A);
    let m = res(&ca.chama("message.get", json!({"v": 1, "id": "f-1"})))["message"].clone();
    let estado = m["state"].as_str().expect("state");
    let reg = m["registered_at"].as_str().expect("registered_at");
    let acc = m["accepted_at"].as_str().expect("accepted_at");
    let resu = m["resulted_at"].as_str().expect("resulted_at");

    // Remetente: os tres estagios, com horarios.
    let ra = a.show(A, "f-1", false);
    assert_eq!(ra.code, Some(0), "{ra:?}");
    let la: Vec<&str> = ra.stdout.lines().collect();
    assert_eq!(la.len(), 4, "{:?}", ra.stdout);
    assert_eq!(la[0], format!("f-1 {estado} {A}>{B} AVISO | {SUBJ}"));
    let t1: Vec<&str> = la[1].split_whitespace().collect();
    assert_eq!((t1[0], t1[1]), ("registrado", reg), "{:?}", la[1]);
    let t2: Vec<&str> = la[2].split_whitespace().collect();
    assert_eq!((t2[0], t2[1]), ("aceito", acc), "{:?}", la[2]);
    let t3: Vec<&str> = la[3].split_whitespace().collect();
    assert_eq!(
        (t3[0], t3[1], t3[2]),
        ("resultado", resu, "done"),
        "{:?}",
        la[3]
    );
    assert!(
        !ra.stdout.contains("corpo-secreto-f1"),
        "corpo nao sai sem --json"
    );

    // Destinatario: mesma saida.
    let rb = a.show(B, "f-1", false);
    assert_eq!(rb.code, Some(0), "{rb:?}");
    assert_eq!(rb.stdout, ra.stdout, "destinatario ve a mesma saida");

    // Alheia e inexistente: mesma saida (so o id muda), saida 1, e unknown_message no protocolo.
    let alheia = a.show(C, "f-1", false);
    let inex = a.show(C, "f-nao-existe", false);
    assert_eq!(alheia.code, Some(1), "{alheia:?}");
    assert_eq!(inex.code, Some(1), "{inex:?}");
    assert_eq!(
        alheia.stdout.replace("f-1", "<ID>"),
        inex.stdout.replace("f-nao-existe", "<ID>"),
        "stdout diferente"
    );
    assert_eq!(
        alheia.stderr.replace("f-1", "<ID>"),
        inex.stderr.replace("f-nao-existe", "<ID>"),
        "stderr diferente"
    );
    let mut cc = a.conexao(C);
    let r = cc.chama("message.get", json!({"v": 1, "id": "f-1"}));
    assert_eq!(cod(&r), "unknown_message", "{r}");

    // --json bate com message.list da identidade destinataria.
    let j = a.show(B, "f-1", true);
    assert_eq!(j.code, Some(0), "{j:?}");
    assert_eq!(
        j.stdout.lines().count(),
        1,
        "--json e uma linha: {:?}",
        j.stdout
    );
    let vj: Value = serde_json::from_str(j.stdout.trim()).expect("show --json e JSON");
    let mut cb = a.conexao(B);
    let lista = res(&cb.chama("message.list", json!({"v": 1, "to": B, "limit": 200})));
    let item = lista["messages"]
        .as_array()
        .expect("messages")
        .iter()
        .find(|x| x["id"] == "f-1")
        .expect("f-1 em message.list");
    assert_eq!(&vj, item, "show --json deve bater com message.list");
}

// ---------- G: controles escapados em watch --receipts e show ----------

#[test]
fn g_controles_nao_chegam_crus_em_watch_receipts_nem_em_show() {
    // Controles so entram pelo log antigo (send e ack recusam): marcas trocadas com daemon parado.
    let mut a = amb_padrao();
    a.envia_de(A, "g-1", B, "SUBJMARK", "BODYMARK");
    let t = a.take(B);
    assert_eq!(t.code, Some(0), "{t:?}");
    let r = a.cli_de(
        B,
        &[
            "ack",
            "--id",
            "g-1",
            "--stage",
            "resultado",
            "--note",
            "NOTEMARK",
        ],
    );
    assert_eq!(r.code, Some(0), "{r:?}");
    a.para();
    a.troca_no_log(&[
        ("SUBJMARK", r"a\u001b[1AZ"),
        ("BODYMARK", r"b\u0007Z"),
        ("NOTEMARK", r"n\u009bZ"),
    ]);
    a.sobe_de_novo();

    // watch --receipts (destinatario, --from-start para pegar o registered antigo).
    let w = a.watch(B, &["--receipts", "--from-start"]);
    let linhas = w.linhas_por(Duration::from_secs(2));
    assert!(
        linhas.iter().any(|l| l.contains(" g-1 ")),
        "o registered de g-1 devia sair: {linhas:?}"
    );
    for l in &linhas {
        assert!(sem_controles(l), "controle cru em watch: {l:?}");
    }

    // show (texto) do remetente e do destinatario.
    for quem in [A, B] {
        let sh = a.show(quem, "g-1", false);
        assert_eq!(sh.code, Some(0), "{sh:?}");
        assert!(
            sem_controles(&sh.stdout),
            "controle cru em show ({quem}): {:?}",
            sh.stdout
        );
        assert!(
            sem_controles(&sh.stderr),
            "controle cru no stderr de show ({quem})"
        );
    }
}

// ---------- H: sem regressao no message.get ----------

#[test]
fn h_message_get_sem_auth_e_unauthenticated_e_campo_extra_e_invalid_envelope() {
    let a = amb_padrao();
    a.envia_de(A, "h-1", B, SUBJ, "corpo");
    let sem = rpc(&a.sock(), "message.get", json!({"v": 1, "id": "h-1"}));
    assert_eq!(cod(&sem), "unauthenticated", "{sem}");
    let mut cb = a.conexao(B);
    let extra = cb.chama("message.get", json!({"v": 1, "id": "h-1", "zz_extra": 1}));
    assert_eq!(cod(&extra), "invalid_envelope", "{extra}");
}

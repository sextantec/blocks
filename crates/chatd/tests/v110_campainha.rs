//! v1.10 (docs/spec-v1.10-seguranca-da-campainha.md): contrato V1 a V13 da campainha.
//! Caixa preta: daemon real em diretorio temporario; `herdr` e `tmux` FALSOS (scripts shell) que registram
//! o argv de cada chamada. Sem multiplexador real, sem agente real, sem rede. O codigo de src/ nao foi lido.
//! Mapa teste -> item do spec e ambiguidades: crates/chatd/tests/V110-TESTES.md.
//! Testes negativos (nao toca / nao chama) tem controle positivo no mesmo cenario, para nao passarem por defeito.
mod common;

use chrono::{SecondsFormat, TimeDelta, Utc};
use common::*;
use serde_json::{Value, json};
use std::collections::BTreeSet;
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::thread;
use std::time::Duration;
use tempfile::TempDir;

const CODEX: &str = "codex-web";
const CLAUDE: &str = "claude-main";
const PANE_CODEX: &str = "w2:p1";
const PANE_CLAUDE: &str = "w2:p2";
/// Texto fixo da campainha (v1.1, C.3).
const TEXTO: &str = "[chat] 1 pendente(s). Rode: chatd inbox --take";
/// Motivo novo da v1.10 (texto livre em `reason` de `bell_failed`).
const MOTIVO_TMUX: &str = "tmux_typing_disabled";
/// Tipos de evento que ja existiam antes da v1.10. AMBIGUO: lista montada dos docs e dos testes existentes
/// (`bound`, `unbound`, `denied`, `auth_failed`, `auth_ok`, `blocked`, `unaccepted`, `due_expired`), nao do codigo.
const TIPOS_EXISTENTES: &[&str] = &[
    "registered",
    "accepted",
    "resulted",
    "bell_sent",
    "bell_failed",
    "bound",
    "unbound",
    "denied",
    "auth_failed",
    "auth_ok",
    "blocked",
    "unaccepted",
    "due_expired",
];

/// Tmux falso: registra o argv (uma linha por chamada, campos separados por TAB). `display-message` le `pane_PANE`.
const TMUX_SH: &str = r#"#!/bin/sh
D="$(dirname "$0")"
{ for a in "$@"; do printf '%s\t' "$a"; done; printf '\n'; } >> "$D/calls_tmux.log"
case "$1" in
  display-message)
    if [ -f "$D/pane_$4" ]; then cat "$D/pane_$4"; exit 0; fi
    exit 1 ;;
  send-keys)
    exit 0 ;;
esac
exit 2
"#;

fn getppid() -> i32 {
    unsafe { libc::getppid() }
}

fn vec_s(v: &[&str]) -> Vec<String> {
    v.iter().map(|x| x.to_string()).collect()
}

fn ha(seg: i64) -> String {
    (Utc::now() - TimeDelta::seconds(seg)).to_rfc3339_opts(SecondsFormat::Millis, true)
}

fn no_futuro(seg: i64) -> String {
    (Utc::now() + TimeDelta::seconds(seg)).to_rfc3339_opts(SecondsFormat::Millis, true)
}

/// Tmux falso e seu registro de chamadas.
struct Tmux {
    dir: PathBuf,
    bin: PathBuf,
}

impl Tmux {
    fn novo(dir: &Path) -> Tmux {
        let bin = dir.join("tmux-falso.sh");
        fs::write(&bin, TMUX_SH).unwrap();
        fs::set_permissions(&bin, fs::Permissions::from_mode(0o755)).unwrap();
        Tmux {
            dir: dir.to_path_buf(),
            bin,
        }
    }

    /// Declara que o painel `pane` tem `pid` como pid do painel (tmux `pane_pid`).
    fn painel_pid(&self, pane: &str, pid: i32) {
        fs::write(self.dir.join(format!("pane_{pane}")), pid.to_string()).unwrap();
    }

    /// Apaga o registro de chamadas (usado depois do bind, para contar so o que a campainha fizer).
    fn limpa(&self) {
        let _ = fs::remove_file(self.dir.join("calls_tmux.log"));
    }

    /// Argv de cada chamada registrada, sem o programa.
    fn chamadas(&self) -> Vec<Vec<String>> {
        fs::read_to_string(self.dir.join("calls_tmux.log"))
            .unwrap_or_default()
            .lines()
            .map(|l| {
                let mut v: Vec<String> = l.split('\t').map(String::from).collect();
                v.pop();
                v
            })
            .collect()
    }

    /// So as chamadas `send-keys`.
    fn envios(&self) -> Vec<Vec<String>> {
        self.chamadas()
            .into_iter()
            .filter(|c| c.first().map(String::as_str) == Some("send-keys"))
            .collect()
    }
}

/// Quantas vezes o herdr falso recebeu `agent prompt` para o painel `pane`.
fn prompts(h: &Herdr, pane: &str) -> usize {
    h.chamadas()
        .iter()
        .filter(|c| c.len() >= 4 && c[0] == "agent" && c[1] == "prompt" && c[2] == pane)
        .count()
}

/// Textos (ultimo argumento) dos `agent prompt` para o painel `pane`.
fn textos_para(h: &Herdr, pane: &str) -> Vec<String> {
    h.chamadas()
        .iter()
        .filter(|c| c.len() >= 4 && c[0] == "agent" && c[1] == "prompt" && c[2] == pane)
        .map(|c| c[3].clone())
        .collect()
}

/// Config com os binarios falsos, poll de 50 ms e rearme de 600 s. `extra` acrescenta chaves.
fn cfg(dir: &Path, herdr: &Path, tmux: &Path, recipients: Value, extra: Value) -> PathBuf {
    let mut c = json!({
        "herdr_bin": s(herdr),
        "tmux_bin": s(tmux),
        "bell_poll_ms": 50,
        "bell_rearm_s": 600,
        "recipients": recipients
    });
    if let (Some(base), Value::Object(ex)) = (c.as_object_mut(), extra) {
        for (k, v) in ex {
            base.insert(k, v);
        }
    }
    escreve_config(dir, &c)
}

/// Destinatarios com campainha por herdr (codex no painel w2:p1, claude no w2:p2).
fn rec_bell() -> Value {
    json!({
        CODEX: {"delivery": "bell", "pane": PANE_CODEX},
        CLAUDE: {"delivery": "bell", "pane": PANE_CLAUDE}
    })
}

/// Um daemon real com seus falsos. Os tokens so existem com auth ligada.
struct Cenario {
    _d: TempDir,
    srv: Servidor,
    tm: Tmux,
    hd: Herdr,
    auth: bool,
    tok_codex: String,
    tok_claude: String,
}

impl Cenario {
    /// Sobe o daemon (auth ligada ou `"auth": "off"`), com `estado` inicial do herdr falso.
    fn sobe(auth: bool, estado: &str, recipients: Value, mut extra: Value) -> Cenario {
        let d = tmp();
        let tm = Tmux::novo(d.path());
        let hd = Herdr::novo(d.path());
        hd.estado(estado);
        if !auth {
            extra
                .as_object_mut()
                .unwrap()
                .insert("auth".into(), json!("off"));
        }
        let c = cfg(d.path(), &hd.bin, &tm.bin, recipients, extra);
        let srv = sobe(&d.path().join("state"), &c);
        let (tok_codex, tok_claude) = if auth {
            (
                token_new(&srv.state, CODEX, &d.path().join("codex.tok")),
                token_new(&srv.state, CLAUDE, &d.path().join("claude.tok")),
            )
        } else {
            (String::new(), String::new())
        };
        Cenario {
            _d: d,
            srv,
            tm,
            hd,
            auth,
            tok_codex,
            tok_claude,
        }
    }

    /// `agent.bind` do codex no painel tmux `pane`, com o proprio processo de teste como ancestral.
    fn liga_tmux(&self, pane: &str) {
        assert!(self.auth, "bind exige auth");
        self.tm.painel_pid(pane, getppid());
        let mut c = conexao_auth(&self.srv.socket, CODEX, &self.tok_codex);
        let r = c.chama("agent.bind", json!({"v": 1, "mux": "tmux", "pane": pane}));
        assert!(!is_err(&r), "bind do painel {pane} deveria ser aceito: {r}");
    }

    /// AVISO de claude-main para `para`.
    fn manda(&self, id: &str, para: &str) {
        if self.auth {
            let mut c = conexao_auth(&self.srv.socket, CLAUDE, &self.tok_claude);
            let r = c.chama(
                "message.send",
                msg(id, CLAUDE, para, "AVISO", "assunto", "corpo"),
            );
            assert!(!is_err(&r), "envio {id} falhou: {r}");
        } else {
            envia_ok(&self.srv.socket, id, para, "corpo");
        }
    }

    fn tipo(&self, tipo: &str) -> Vec<Value> {
        eventos_tipo(&self.srv.state, tipo)
    }
}

/// Escritor do log.jsonl como o daemon o escreve (v, seq crescente a partir de 1, ts, type e campos).
/// Os campos de `registered` seguem o modelo de v18_stale_resultado.rs.
struct Diario {
    arq: PathBuf,
    seq: u64,
}

impl Diario {
    fn novo(state: &Path) -> Diario {
        fs::create_dir_all(state).unwrap();
        let arq = state.join("log.jsonl");
        fs::write(&arq, "").unwrap();
        Diario { arq, seq: 0 }
    }

    fn ev(&mut self, tipo: &str, ts: &str, campos: Value) {
        self.seq += 1;
        let mut linha = json!({"v": 1, "seq": self.seq, "ts": ts, "type": tipo});
        if let (Some(base), Value::Object(ex)) = (linha.as_object_mut(), campos) {
            for (k, v) in ex {
                base.insert(k, v);
            }
        }
        let mut f = OpenOptions::new().append(true).open(&self.arq).unwrap();
        writeln!(f, "{linha}").unwrap();
    }

    /// Mensagem `id` de `de` para `para`, registrada e ainda sem `accepted` (pendente).
    fn registra(&mut self, id: &str, de: &str, para: &str, ts: &str) {
        self.ev(
            "registered",
            ts,
            json!({
                "id": id, "from": de, "to": para, "kind": "AVISO",
                "subject": "assunto", "body": "corpo", "thread": null,
                "state": "registered", "refs": [], "note": null,
                "registered_at": ts, "accepted_at": null, "resulted_at": null, "result": null
            }),
        );
    }

    fn aceita(&mut self, id: &str, ts: &str) {
        self.ev("accepted", ts, json!({"id": id, "note": null}));
    }

    fn campainha(&mut self, dest: &str, ts: &str) {
        self.ev("bell_sent", ts, json!({"recipient": dest}));
    }
}

/// Daemon com log pre-gravado (antes de subir), herdr falso com `estado` e auth desligada.
fn cenario_pre(estado: &str, escreve: impl FnOnce(&mut Diario)) -> Cenario {
    let d = tmp();
    let tm = Tmux::novo(d.path());
    let hd = Herdr::novo(d.path());
    hd.estado(estado);
    let state = d.path().join("state");
    let mut dia = Diario::novo(&state);
    escreve(&mut dia);
    let c = cfg(
        d.path(),
        &hd.bin,
        &tm.bin,
        rec_bell(),
        json!({"auth": "off"}),
    );
    let srv = sobe(&state, &c);
    Cenario {
        _d: d,
        srv,
        tm,
        hd,
        auth: false,
        tok_codex: String::new(),
        tok_claude: String::new(),
    }
}

/// Cenario tmux V1 a V3: auth ligada, codex com `delivery: bell` e `pane` fixo (o bind prevalece).
fn cenario_tmux(extra: Value) -> Cenario {
    Cenario::sobe(
        true,
        "idle",
        json!({
            CODEX: {"delivery": "bell", "pane": PANE_CODEX},
            CLAUDE: {"delivery": "watch"}
        }),
        extra,
    )
}

// ---------- V1 a V3: campainha tmux ----------

#[test]
fn v01_tmux_sem_chave_nao_digita_e_registra_bell_failed_do_motivo_novo() {
    // V1 (negativo): chave ausente. Controle: V3 (mesmo cenario com a chave true digita).
    // AMBIGUO: o spec proibe send-keys e "nenhum comando"; nao diz se a campainha pode rodar outro subcomando
    // (ex. display-message) antes de recusar. Aqui qualquer chamada ao tmux depois do bind conta como erro.
    let a = cenario_tmux(json!({}));
    a.liga_tmux("%1");
    a.tm.limpa();
    a.manda("v01-1", CODEX);
    thread::sleep(Duration::from_secs(3));
    assert!(
        a.tm.chamadas().is_empty(),
        "tmux nao pode receber nenhum comando com a chave ausente: {:?}",
        a.tm.chamadas()
    );
    let falhas = a.tipo("bell_failed");
    let motivo: Vec<&Value> = falhas
        .iter()
        .filter(|e| e["reason"] == MOTIVO_TMUX)
        .collect();
    assert_eq!(
        motivo.len(),
        1,
        "um bell_failed {MOTIVO_TMUX} por minuto (3 s de rodadas): {falhas:?}"
    );
    assert_eq!(motivo[0]["recipient"], CODEX);
    let cl = cli(&["log", "--state-dir", s(&a.srv.state)]);
    assert!(
        cl.stdout.contains("bell_failed") && cl.stdout.contains(MOTIVO_TMUX),
        "chatd log deve mostrar o bell_failed: {}",
        cl.stdout
    );
}

#[test]
fn v02_tmux_com_chave_false_explicita_nao_digita() {
    // V2 (negativo): mesmo que V1 com a chave false escrita. Controle: V3.
    let a = cenario_tmux(json!({"bell_tmux_typing": false}));
    a.liga_tmux("%1");
    a.tm.limpa();
    a.manda("v02-1", CODEX);
    thread::sleep(Duration::from_secs(3));
    assert!(
        a.tm.chamadas().is_empty(),
        "com false nenhum comando tmux: {:?}",
        a.tm.chamadas()
    );
    let motivo = a
        .tipo("bell_failed")
        .iter()
        .filter(|e| e["reason"] == MOTIVO_TMUX)
        .count();
    assert_eq!(motivo, 1, "bell_failed {MOTIVO_TMUX} esperado");
}

#[test]
fn v03_tmux_com_chave_true_digita_como_na_v13_uma_vez() {
    // V3 (controle positivo de V1/V2): com true, send-keys -l TEXTO e send-keys Enter, uma vez cada.
    let a = cenario_tmux(json!({"bell_tmux_typing": true}));
    a.liga_tmux("%1");
    a.tm.limpa();
    a.manda("v03-1", CODEX);
    assert!(
        espera(Duration::from_secs(5), || a.tm.envios().len() >= 2),
        "a campainha tmux com true nao digitou: {:?}",
        a.tm.chamadas()
    );
    thread::sleep(Duration::from_secs(2));
    let envios = a.tm.envios();
    let texto = vec_s(&["send-keys", "-t", "%1", "-l", TEXTO]);
    let enter = vec_s(&["send-keys", "-t", "%1", "Enter"]);
    let n_texto = envios.iter().filter(|c| **c == texto).count();
    let n_enter = envios.iter().filter(|c| **c == enter).count();
    assert_eq!(n_texto, 1, "send-keys -l TEXTO uma vez: {envios:?}");
    assert_eq!(n_enter, 1, "send-keys Enter uma vez: {envios:?}");
    assert_eq!(envios.len(), 2, "nenhum send-keys alem destes: {envios:?}");
    let enviados = a.tipo("bell_sent");
    assert_eq!(
        enviados.iter().filter(|e| e["recipient"] == CODEX).count(),
        1,
        "bell_sent do destinatario: {enviados:?}"
    );
}

// ---------- V4: validacao da chave ----------

#[test]
fn v04_valor_ou_chave_invalida_recusa_a_config_e_booleano_sobe() {
    // V4 (negativo): "sim" e chave com outro nome sao recusados (saida != 0, sem travar).
    // Controle: true booleano sobe normalmente.
    // AMBIGUO: o spec manda "saida de erro como as outras chaves invalidas", sem codigo fixo; aceita-se qualquer != 0.
    let invalidas = [
        json!({"bell_tmux_typing": "sim"}),
        json!({"bell_tmux_type": true}),
    ];
    for extra in invalidas {
        let d = tmp();
        let h = Herdr::novo(d.path());
        let t = Tmux::novo(d.path());
        let c = cfg(
            d.path(),
            &h.bin,
            &t.bin,
            json!({CODEX: {"delivery": "bell", "pane": PANE_CODEX}}),
            extra.clone(),
        );
        let st = d.path().join("state");
        match tenta(
            &["serve", "--state-dir", s(&st), "--config", s(&c)],
            &[],
            &st,
            &st.join("chatd.sock"),
        ) {
            Ok(_srv) => panic!("config com {extra} deveria ser recusada"),
            Err(e) => assert!(
                e.code.is_some() && e.code != Some(0),
                "recusa deve sair com codigo de erro, veio {e:?} para {extra}"
            ),
        }
    }
    let d = tmp();
    let h = Herdr::novo(d.path());
    let t = Tmux::novo(d.path());
    let c = cfg(
        d.path(),
        &h.bin,
        &t.bin,
        json!({CODEX: {"delivery": "bell", "pane": PANE_CODEX}}),
        json!({"bell_tmux_typing": true}),
    );
    let st = d.path().join("state");
    let ok = tenta(
        &["serve", "--state-dir", s(&st), "--config", s(&c)],
        &[],
        &st,
        &st.join("chatd.sock"),
    );
    assert!(
        ok.is_ok(),
        "o booleano true deve subir o daemon: {:?}",
        ok.err()
    );
}

// ---------- V5: herdr inalterado ----------

#[test]
fn v05_herdr_working_nao_recebe_prompt_e_idle_recebe() {
    // V5 (regressao v1.1/v1.3): working nao recebe `agent prompt`; idle recebe uma vez com o texto fixo.
    let a = Cenario::sobe(
        false,
        "working",
        json!({CODEX: {"delivery": "bell", "pane": PANE_CODEX}, CLAUDE: {"delivery": "watch"}}),
        json!({}),
    );
    a.manda("v05-1", CODEX);
    thread::sleep(Duration::from_millis(1500));
    assert_eq!(
        prompts(&a.hd, PANE_CODEX),
        0,
        "working nao pode receber prompt: {:?}",
        a.hd.chamadas()
    );
    a.hd.estado("idle");
    assert!(
        espera(Duration::from_secs(5), || prompts(&a.hd, PANE_CODEX) >= 1),
        "idle deveria receber prompt: {:?}",
        a.hd.chamadas()
    );
    thread::sleep(Duration::from_secs(1));
    assert_eq!(prompts(&a.hd, PANE_CODEX), 1, "uma campainha por rearme");
    assert_eq!(textos_para(&a.hd, PANE_CODEX), vec![TEXTO.to_string()]);
}

// ---------- V6: nenhum tipo de evento novo ----------

fn tipos(state: &Path) -> BTreeSet<String> {
    log_linhas(state)
        .iter()
        .map(|l| serde_json::from_str::<Value>(l).expect("linha do log"))
        .filter_map(|v| v["type"].as_str().map(String::from))
        .collect()
}

#[test]
fn v06_nenhum_tipo_de_evento_novo_nas_rodadas_v1_a_v5() {
    // V6: tipos do log nas rodadas com tmux desligado e com herdr; so os existentes.
    // Controle: o conjunto contem bell_failed e bell_sent, que ja existiam (a leitura nao e vazia).
    let a = cenario_tmux(json!({}));
    a.liga_tmux("%1");
    a.manda("v06-1", CODEX);
    assert!(
        espera(Duration::from_secs(5), || !a.tipo("bell_failed").is_empty()),
        "rodada tmux desligado sem bell_failed"
    );
    let mut vistos = tipos(&a.srv.state);

    let b = Cenario::sobe(
        false,
        "idle",
        json!({CODEX: {"delivery": "bell", "pane": PANE_CODEX}}),
        json!({}),
    );
    b.manda("v06-2", CODEX);
    assert!(
        espera(Duration::from_secs(5), || !b.tipo("bell_sent").is_empty()),
        "rodada herdr sem bell_sent"
    );
    vistos.extend(tipos(&b.srv.state));

    let novos: Vec<&String> = vistos
        .iter()
        .filter(|t| !TIPOS_EXISTENTES.contains(&t.as_str()))
        .collect();
    assert!(novos.is_empty(), "tipos de evento novos: {novos:?}");
    assert!(vistos.contains("bell_failed") && vistos.contains("bell_sent"));
}

// ---------- V7 a V13: rearme durante o reinicio (log pre-gravado) ----------

#[test]
fn v07_bell_sent_de_10s_do_claude_nao_toca_ao_iniciar() {
    // V7 (negativo): claude tocado ha 10 s, com rearme de 600 s. Controle: codex com bell_sent de 700 s toca.
    let a = cenario_pre("idle", |dia| {
        dia.registra("v07-c", CODEX, CLAUDE, &ha(30));
        dia.campainha(CLAUDE, &ha(10));
        dia.registra("v07-x", CLAUDE, CODEX, &ha(30));
        dia.campainha(CODEX, &ha(700));
    });
    assert!(
        espera(Duration::from_secs(5), || prompts(&a.hd, PANE_CODEX) >= 1),
        "controle: codex com bell_sent de 700 s deveria tocar: {:?}",
        a.hd.chamadas()
    );
    thread::sleep(Duration::from_secs(2));
    assert_eq!(
        prompts(&a.hd, PANE_CLAUDE),
        0,
        "claude tocado ha 10 s nao pode tocar de novo: {:?}",
        a.hd.chamadas()
    );
}

#[test]
fn v08_bell_sent_de_700s_do_claude_toca_em_um_ciclo() {
    // V8 (positivo, par de V7): claude tocado ha 700 s (>= 600 s) toca uma vez.
    let a = cenario_pre("idle", |dia| {
        dia.registra("v08-1", CODEX, CLAUDE, &ha(30));
        dia.campainha(CLAUDE, &ha(700));
    });
    assert!(
        espera(Duration::from_secs(5), || prompts(&a.hd, PANE_CLAUDE) >= 1),
        "bell_sent de 700 s deveria liberar a campainha: {:?}",
        a.hd.chamadas()
    );
    thread::sleep(Duration::from_secs(2));
    assert_eq!(prompts(&a.hd, PANE_CLAUDE), 1, "uma vez so");
    assert_eq!(textos_para(&a.hd, PANE_CLAUDE), vec![TEXTO.to_string()]);
}

#[test]
fn v09_bell_sent_com_ts_no_futuro_nao_toca() {
    // V9 (negativo): ts 1 h no futuro = idade 0. Controle: codex com bell_sent de 700 s toca.
    let a = cenario_pre("idle", |dia| {
        dia.registra("v09-c", CODEX, CLAUDE, &ha(30));
        dia.registra("v09-x", CLAUDE, CODEX, &ha(30));
        dia.campainha(CLAUDE, &no_futuro(3600));
        dia.campainha(CODEX, &ha(700));
    });
    assert!(
        espera(Duration::from_secs(5), || prompts(&a.hd, PANE_CODEX) >= 1),
        "controle: codex deveria tocar: {:?}",
        a.hd.chamadas()
    );
    thread::sleep(Duration::from_secs(2));
    assert_eq!(
        prompts(&a.hd, PANE_CLAUDE),
        0,
        "ts no futuro deve contar como idade 0: {:?}",
        a.hd.chamadas()
    );
}

#[test]
fn v10_bell_sent_com_ts_ilegivel_nao_toca_e_o_daemon_sobe() {
    // V10 (negativo): ts ilegivel = idade 0; o daemon tem de subir (sobe() falha se nao subir).
    // Controle: codex com bell_sent de 700 s toca.
    // AMBIGUO: o spec nao diz se um ts ilegivel torna o log ilegivel para outras leituras; aqui so se exige subir e nao tocar.
    let a = cenario_pre("idle", |dia| {
        dia.registra("v10-c", CODEX, CLAUDE, &ha(30));
        dia.registra("v10-x", CLAUDE, CODEX, &ha(30));
        dia.campainha(CLAUDE, "nao-e-data");
        dia.campainha(CODEX, &ha(700));
    });
    assert!(
        espera(Duration::from_secs(5), || prompts(&a.hd, PANE_CODEX) >= 1),
        "controle: codex deveria tocar: {:?}",
        a.hd.chamadas()
    );
    thread::sleep(Duration::from_secs(2));
    assert_eq!(
        prompts(&a.hd, PANE_CLAUDE),
        0,
        "ts ilegivel deve falhar para o lado seguro: {:?}",
        a.hd.chamadas()
    );
}

#[test]
fn v11_accepted_da_pendencia_apaga_o_rearme_e_a_nova_toca() {
    // V11 (positivo): claude tocado ha 10 s; a pendente foi aceita; chegou uma nova: toca no ciclo.
    let a = cenario_pre("idle", |dia| {
        dia.registra("v11-1", CODEX, CLAUDE, &ha(30));
        dia.campainha(CLAUDE, &ha(10));
        dia.aceita("v11-1", &ha(9));
        dia.registra("v11-2", CODEX, CLAUDE, &ha(8));
    });
    assert!(
        espera(Duration::from_secs(5), || prompts(&a.hd, PANE_CLAUDE) >= 1),
        "accepted deveria ter apagado o rearme: {:?}",
        a.hd.chamadas()
    );
    thread::sleep(Duration::from_secs(1));
    assert_eq!(prompts(&a.hd, PANE_CLAUDE), 1, "uma campainha so");
}

#[test]
fn v12_so_o_destinatario_com_bell_sent_recente_fica_quieto() {
    // V12 (negativo para claude, positivo para codex): codex sem bell_sent toca; claude tocado ha 10 s nao.
    let a = cenario_pre("idle", |dia| {
        dia.registra("v12-c", CLAUDE, CODEX, &ha(30));
        dia.registra("v12-l", CODEX, CLAUDE, &ha(30));
        dia.campainha(CLAUDE, &ha(10));
    });
    assert!(
        espera(Duration::from_secs(5), || prompts(&a.hd, PANE_CODEX) >= 1),
        "codex sem bell_sent deveria tocar: {:?}",
        a.hd.chamadas()
    );
    thread::sleep(Duration::from_secs(2));
    assert_eq!(
        prompts(&a.hd, PANE_CLAUDE),
        0,
        "claude com bell_sent de 10 s nao pode tocar: {:?}",
        a.hd.chamadas()
    );
}

#[test]
fn v13_replay_nao_acrescenta_eventos_ao_log() {
    // V13 (negativo): herdr working, so replay; o log fica igual. Controle: com idle, o mesmo log
    // ganha bell_sent (prova que o log e gravado ao vivo e que o teste nao esta vazio).
    let d = tmp();
    let h = Herdr::novo(d.path());
    let t = Tmux::novo(d.path());
    h.estado("working");
    let state = d.path().join("state");
    let mut dia = Diario::novo(&state);
    dia.registra("v13-0", CLAUDE, CODEX, &ha(60));
    dia.aceita("v13-0", &ha(50));
    dia.registra("v13-1", CLAUDE, CODEX, &ha(40));
    dia.campainha(CLAUDE, &ha(20));
    let antes = log_bytes(&state);
    let c = cfg(
        d.path(),
        &h.bin,
        &t.bin,
        json!({CODEX: {"delivery": "bell", "pane": PANE_CODEX}, CLAUDE: {"delivery": "watch"}}),
        json!({"auth": "off"}),
    );
    let mut srv = sobe(&state, &c);
    thread::sleep(Duration::from_millis(1500));
    assert!(srv.para().is_some(), "daemon nao saiu com SIGTERM");
    drop(srv);
    assert_eq!(
        log_bytes(&state),
        antes,
        "o replay nao pode acrescentar nem alterar linhas do log"
    );

    h.estado("idle");
    let _srv2 = sobe(&state, &c);
    assert!(
        espera(Duration::from_secs(5), || eventos_tipo(&state, "bell_sent")
            .iter()
            .any(|e| e["recipient"] == CODEX)),
        "controle: com idle a gravacao ao vivo deve acrescentar bell_sent"
    );
    assert_ne!(
        log_bytes(&state),
        antes,
        "controle: o log deve crescer ao vivo"
    );
}

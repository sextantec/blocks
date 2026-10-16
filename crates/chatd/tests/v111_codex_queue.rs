//! v1.11 (crates/chatd/docs/spec-v1.11-canal-codex-queue.md): contrato Q1 a Q17 do canal `codex-queue`.
//! Q18 e extra: cobre a razao `spawn_failed` (secao 3 do spec, passo 4), que nao tem item Q proprio.
//! Caixa preta: daemon real em diretorio temporario; `codex`, `herdr` e `tmux` FALSOS (scripts shell) que
//! registram o argv de cada chamada. Sem codex, tmux ou herdr reais, sem agente real, sem rede.
//! O codigo de src/ nao foi lido. Mapa teste -> item do spec e ambiguidades: crates/chatd/tests/V111-TESTES.md.
//! Testes negativos (nao executa / nao chama) tem controle positivo no mesmo cenario, para nao passarem por defeito.
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
use std::time::{Duration, Instant};
use tempfile::TempDir;

const RECIP: &str = "codex-blocks";
const CLAUDE: &str = "claude-main";
const CODEX_WEB: &str = "codex-web";
const PANE_CODEX: &str = "w2:p1";
/// Texto fixo da campainha (so a contagem; spec 3.3, passo 3).
const TEXTO: &str = "[chat] 1 pendente(s). Rode: chatd inbox --take";
/// Versao aceita nos testes (`codex_versions`).
const VERSAO: &str = "0.163.0";
/// Tipos de evento que ja existiam antes da v1.11. AMBIGUO: lista copiada de v110_campainha.rs (que a montou dos
/// docs e dos testes), nao do codigo. Se o daemon tiver outro tipo antigo (ex. de stale), o Q13 acusa falso positivo.
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

/// Codex falso. `--version` registra o argv em calls_version.log e imprime `codex-cli <versao.txt>`.
/// `queue` registra o argv em calls_queue.log e age pelo arquivo `modo_<UUID>` (o UUID e o `$3`):
/// `falha` sai 1 com texto em stderr; `dorme` dorme 8 s (exec, para o timeout de 5 s matar o proprio sleep);
/// `silencio` sai 0 sem imprimir. Sem arquivo de modo = caso feliz (imprime `Queued message ...`).
const CODEX_SH: &str = r##"#!/bin/sh
D="$(dirname "$0")"
reg() {
  f="$1"; shift
  { for a in "$@"; do printf '%s\t' "$a"; done; printf '\n'; } >> "$D/$f"
}
case "$1" in
  --version)
    reg calls_version.log "$@"
    if [ -f "$D/dorme_version" ]; then exec sleep 8; fi
    printf 'codex-cli %s\n' "$(cat "$D/versao.txt" 2>/dev/null)"
    exit 0 ;;
  queue)
    reg calls_queue.log "$@"
    case "$(cat "$D/modo_$3" 2>/dev/null)" in
      falha) echo "falso: falha simulada" >&2; exit 1 ;;
      dorme) exec sleep 8 ;;
      silencio) exit 0 ;;
    esac
    printf 'Queued message 01a14603-ff3c-72e1-a55b-b8389c529719 for thread %s.\n' "$3"
    exit 0 ;;
esac
reg calls_outro.log "$@"
exit 2
"##;

/// Herdr falso: registra todo argv em calls_herdr.log; `agent get` responde `idle` (para a campainha do codex-web).
const HERDR_SH: &str = r##"#!/bin/sh
D="$(dirname "$0")"
{ for a in "$@"; do printf '%s\t' "$a"; done; printf '\n'; } >> "$D/calls_herdr.log"
if [ "$1" = "agent" ] && [ "$2" = "get" ]; then
  printf '{"id":"cli:agent:get","result":{"agent":{"pane_id":"%s","agent_status":"idle"},"type":"agent_info"}}\n' "$3"
  exit 0
fi
exit 0
"##;

/// Tmux falso: registra todo argv em calls_tmux.log e sai 1 (nao ha painel tmux nos testes).
const TMUX_SH: &str = r##"#!/bin/sh
D="$(dirname "$0")"
{ for a in "$@"; do printf '%s\t' "$a"; done; printf '\n'; } >> "$D/calls_tmux.log"
exit 1
"##;

// ---------- auxiliares ----------

/// UUID minusculo valido, distinto para cada `n`. Tem letras hex, para que o caso "maiusculo" de Q3 seja diferente.
fn uuid(n: u32) -> String {
    format!("{n:08x}-abcd-4ef0-8123-456789abcdef")
}

/// argv esperado de `codex queue --thread U --message TEXTO`.
fn argv_queue(u: &str, texto: &str) -> Vec<String> {
    ["queue", "--thread", u, "--message", texto]
        .iter()
        .map(|x| x.to_string())
        .collect()
}

fn ha(seg: i64) -> String {
    (Utc::now() - TimeDelta::seconds(seg)).to_rfc3339_opts(SecondsFormat::Millis, true)
}

/// Argv de cada chamada registrada em `arq` (dentro de `dir`), sem o programa.
fn chamadas(dir: &Path, arq: &str) -> Vec<Vec<String>> {
    fs::read_to_string(dir.join(arq))
        .unwrap_or_default()
        .lines()
        .map(|l| {
            let mut v: Vec<String> = l.split('\t').map(String::from).collect();
            v.pop();
            v
        })
        .collect()
}

fn escreve_script(dir: &Path, arq: &str, corpo: &str) -> PathBuf {
    let p = dir.join(arq);
    fs::write(&p, corpo).expect("escrever script falso");
    fs::set_permissions(&p, fs::Permissions::from_mode(0o755)).expect("chmod script falso");
    p
}

/// Arquivo do thread com conteudo e modo dados.
fn arq_thread(dir: &Path, nome: &str, conteudo: &[u8], modo: u32) -> PathBuf {
    let p = dir.join(nome);
    fs::write(&p, conteudo).expect("escrever thread_file");
    fs::set_permissions(&p, fs::Permissions::from_mode(modo)).expect("chmod thread_file");
    p
}

/// thread_file valido: UUID minusculo, uma linha, modo 0600.
fn thread_ok(dir: &Path, nome: &str, u: &str) -> PathBuf {
    arq_thread(dir, nome, format!("{u}\n").as_bytes(), 0o600)
}

/// Destinatario `codex-queue` apontando para `thread`.
fn recp(thread: &Path) -> Value {
    json!({"delivery": "codex-queue", "thread_file": s(thread)})
}

struct Codex {
    dir: PathBuf,
    bin: PathBuf,
}

impl Codex {
    /// Codex falso `arq` dentro de `dir`, versao `VERSAO`.
    fn em(dir: &Path, arq: &str) -> Codex {
        let bin = escreve_script(dir, arq, CODEX_SH);
        fs::write(dir.join("versao.txt"), VERSAO).expect("versao do codex falso");
        Codex {
            dir: dir.to_path_buf(),
            bin,
        }
    }

    fn versao(&self, v: &str) {
        fs::write(self.dir.join("versao.txt"), v).expect("trocar versao do codex falso");
    }

    /// Modo do `queue` para o thread `u` (falha, dorme, silencio).
    fn modo(&self, u: &str, modo: &str) {
        fs::write(self.dir.join(format!("modo_{u}")), modo).expect("modo do codex falso");
    }

    /// Remove o modo do thread `u`: os proximos `queue` dele voltam ao caso feliz.
    fn modo_limpa(&self, u: &str) {
        let _ = fs::remove_file(self.dir.join(format!("modo_{u}")));
    }

    /// Liga ou desliga o sono do `--version` (dorme 8 s; o daemon corta em 5 s).
    fn versao_dorme(&self, ligado: bool) {
        let p = self.dir.join("dorme_version");
        if ligado {
            fs::write(&p, "").expect("ligar sono do --version");
        } else {
            let _ = fs::remove_file(&p);
        }
    }

    fn queue(&self) -> Vec<Vec<String>> {
        chamadas(&self.dir, "calls_queue.log")
    }

    fn versoes(&self) -> Vec<Vec<String>> {
        chamadas(&self.dir, "calls_version.log")
    }

    /// Quantas vezes `queue` foi chamado com `--thread u`.
    fn queue_de(&self, u: &str) -> usize {
        self.queue()
            .iter()
            .filter(|c| c.get(2).map(String::as_str) == Some(u))
            .count()
    }
}

struct Falsos {
    codex: Codex,
    herdr: PathBuf,
    tmux: PathBuf,
}

fn falsos(dir: &Path) -> Falsos {
    Falsos {
        codex: Codex::em(dir, "codex-falso.sh"),
        herdr: escreve_script(dir, "herdr-falso.sh", HERDR_SH),
        tmux: escreve_script(dir, "tmux-falso.sh", TMUX_SH),
    }
}

/// Config base com os falsos, sem auth, poll de 50 ms e o rearme dado. Os destinatarios vem em `recipients`.
fn base(f: &Falsos, recipients: Value, rearm_s: u64) -> Value {
    json!({
        "auth": "off",
        "herdr_bin": s(&f.herdr),
        "tmux_bin": s(&f.tmux),
        "codex_bin": s(&f.codex.bin),
        "codex_versions": [VERSAO],
        "bell_poll_ms": 50,
        "bell_rearm_s": rearm_s,
        "recipients": recipients
    })
}

/// Grava a config em `d` e sobe o daemon com state em `d/state`.
fn sobe_cfg(d: &TempDir, cfg: &Value) -> Servidor {
    let c = escreve_config(d.path(), cfg);
    sobe(&d.path().join("state"), &c)
}

/// AVISO de claude-main para `para` (corpo fixo).
fn manda(srv: &Servidor, id: &str, para: &str) {
    envia_ok(&srv.socket, id, para, "corpo");
}

/// bell_failed do destinatario `rec`.
fn falhas(state: &Path, rec: &str) -> Vec<Value> {
    eventos_tipo(state, "bell_failed")
        .into_iter()
        .filter(|e| e["recipient"] == rec)
        .collect()
}

/// bell_sent do destinatario `rec`.
fn enviados(state: &Path, rec: &str) -> usize {
    eventos_tipo(state, "bell_sent")
        .iter()
        .filter(|e| e["recipient"] == rec)
        .count()
}

fn espera_razao(state: &Path, rec: &str, razao: &str, d: Duration) -> bool {
    espera(d, || {
        falhas(state, rec).iter().any(|e| e["reason"] == razao)
    })
}

/// Quantas vezes o herdr falso recebeu `agent prompt` para o painel `pane`.
fn prompts_herdr(dir: &Path, pane: &str) -> usize {
    chamadas(dir, "calls_herdr.log")
        .iter()
        .filter(|c| c.len() >= 4 && c[1] == "prompt" && c[2] == pane)
        .count()
}

/// Tipos (campo `type`) presentes no log.jsonl.
fn tipos(state: &Path) -> BTreeSet<String> {
    log_linhas(state)
        .iter()
        .map(|l| serde_json::from_str::<Value>(l).expect("linha do log"))
        .filter_map(|v| v["type"].as_str().map(String::from))
        .collect()
}

/// Escritor do log.jsonl como o daemon o escreve (copiado de v110_campainha.rs).
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

// ---------- Q1: caminho feliz ----------

#[test]
fn q1_caminho_feliz_chama_queue_uma_vez_com_argv_exato() {
    let d = tmp();
    let f = falsos(d.path());
    let ok = thread_ok(d.path(), "thread-ok", &uuid(1));
    let srv = sobe_cfg(&d, &base(&f, json!({RECIP: recp(&ok)}), 600));
    manda(&srv, "q1-1", RECIP);
    assert!(
        espera(Duration::from_secs(5), || f.codex.queue().len() >= 1),
        "queue nao foi chamado; versoes: {:?}",
        f.codex.versoes()
    );
    thread::sleep(Duration::from_secs(2));
    assert_eq!(
        f.codex.queue(),
        vec![argv_queue(&uuid(1), TEXTO)],
        "uma unica chamada, com argv exato"
    );
    assert!(
        !f.codex.versoes().is_empty(),
        "a checagem de versao deve rodar antes do queue"
    );
    assert!(
        f.codex
            .versoes()
            .iter()
            .all(|c| *c == vec!["--version".to_string()]),
        "chamadas de versao: {:?}",
        f.codex.versoes()
    );
    assert_eq!(enviados(&srv.state, RECIP), 1, "bell_sent do destinatario");
    assert!(
        falhas(&srv.state, RECIP).is_empty(),
        "sem bell_failed no caso feliz: {:?}",
        falhas(&srv.state, RECIP)
    );
}

// ---------- Q2: thread_file ausente ----------

#[test]
fn q2_thread_file_ausente_nao_chama_version_nem_queue_com_controle() {
    // Negativo (E1): thread_file inexistente (destinatario "ruim"). Antes de o destinatario valido receber
    // mensagem, nem --version nem queue podem ter rodado. Controle: o valido, enviado depois, roda --version e queue.
    let d = tmp();
    let f = falsos(d.path());
    let ok = thread_ok(d.path(), "thread-ok", &uuid(1));
    let ausente = d.path().join("nao-existe");
    let srv = sobe_cfg(
        &d,
        &base(&f, json!({RECIP: recp(&ok), "ruim": recp(&ausente)}), 600),
    );
    manda(&srv, "q2-2", "ruim");
    assert!(
        espera_razao(
            &srv.state,
            "ruim",
            "thread_file_missing",
            Duration::from_secs(5)
        ),
        "bell_failed thread_file_missing esperado: {:?}",
        falhas(&srv.state, "ruim")
    );
    assert!(
        f.codex.versoes().is_empty(),
        "E1: --version nao pode rodar com thread_file ausente: {:?}",
        f.codex.versoes()
    );
    assert!(
        f.codex.queue().is_empty(),
        "queue nao pode rodar: {:?}",
        f.codex.queue()
    );
    manda(&srv, "q2-1", RECIP);
    assert!(
        espera(Duration::from_secs(5), || f.codex.queue_de(&uuid(1)) >= 1),
        "controle: o destinatario com arquivo valido deveria chamar queue"
    );
    assert!(
        !f.codex.versoes().is_empty(),
        "controle: o destinatario valido consulta a versao"
    );
    thread::sleep(Duration::from_secs(2));
    assert_eq!(
        f.codex.queue().len(),
        1,
        "so o destinatario com arquivo valido chama queue: {:?}",
        f.codex.queue()
    );
    assert_eq!(
        enviados(&srv.state, "ruim"),
        0,
        "sem bell_sent para o arquivo ausente"
    );
}

// ---------- Q3: conteudo invalido ----------

#[test]
fn q3_conteudo_invalido_nao_chama_version_nem_queue_com_controle() {
    // Negativos (E1, E2): vazio, --help, 65 bytes, duas linhas, duas quebras finais, CRLF, UUID maiusculo, espaco.
    // Controles: UUID com um unico \n final (E2) e UUID sem quebra final (leitura literal do spec).
    // Os invalidos sao recusados ANTES de os validos receberem mensagem (E1: --version nao pode ter rodado).
    let d = tmp();
    let f = falsos(d.path());
    let ok = thread_ok(d.path(), "thread-ok", &uuid(1));
    let sem_nl = arq_thread(d.path(), "sem-nl", uuid(5).as_bytes(), 0o600);
    let ruins: Vec<Vec<u8>> = vec![
        b"".to_vec(),
        b"--help\n".to_vec(),
        format!("{}\n{}", uuid(2), " ".repeat(28)).into_bytes(), // 36 + 1 + 28 = 65 bytes
        format!("{0}\n{0}\n", uuid(3)).into_bytes(),
        format!("{}\n\n", uuid(7)).into_bytes(), // duas quebras finais (E2)
        format!("{}\r\n", uuid(8)).into_bytes(), // CRLF (E2)
        format!("{}\n", uuid(4).to_uppercase()).into_bytes(),
        format!(" {}\n", uuid(6)).into_bytes(),
    ];
    let mut recs = serde_json::Map::new();
    recs.insert(RECIP.into(), recp(&ok));
    recs.insert("sem-nl".into(), recp(&sem_nl));
    for (i, conteudo) in ruins.iter().enumerate() {
        let p = arq_thread(d.path(), &format!("ruim-{i}"), conteudo, 0o600);
        recs.insert(format!("ruim-{i}"), recp(&p));
    }
    let srv = sobe_cfg(&d, &base(&f, Value::Object(recs), 600));
    for i in 0..ruins.len() {
        manda(&srv, &format!("q3-r{i}"), &format!("ruim-{i}"));
    }
    for i in 0..ruins.len() {
        assert!(
            espera_razao(
                &srv.state,
                &format!("ruim-{i}"),
                "thread_file_invalid",
                Duration::from_secs(5)
            ),
            "ruim-{i} ({:?}): thread_file_invalid esperado: {:?}",
            String::from_utf8_lossy(&ruins[i]),
            falhas(&srv.state, &format!("ruim-{i}"))
        );
    }
    assert!(
        f.codex.versoes().is_empty() && f.codex.queue().is_empty(),
        "E1: nenhum arquivo invalido pode rodar --version nem queue: versoes {:?}, queue {:?}",
        f.codex.versoes(),
        f.codex.queue()
    );
    manda(&srv, "q3-0", RECIP);
    manda(&srv, "q3-1", "sem-nl");
    assert!(
        espera(Duration::from_secs(5), || f.codex.queue_de(&uuid(1)) >= 1
            && f.codex.queue_de(&uuid(5)) >= 1),
        "controle: os dois arquivos validos deveriam chamar queue; queue: {:?}",
        f.codex.queue()
    );
    thread::sleep(Duration::from_secs(2));
    let mut chamados: Vec<String> = f
        .codex
        .queue()
        .iter()
        .map(|c| c.get(2).cloned().unwrap_or_default())
        .collect();
    chamados.sort();
    let mut esperados = vec![uuid(1), uuid(5)];
    esperados.sort();
    assert_eq!(
        chamados,
        esperados,
        "so os dois arquivos validos chamam queue, uma vez cada: {:?}",
        f.codex.queue()
    );
}

// ---------- Q4: symlink e gravavel por grupo/outros ----------

#[test]
fn q4_symlink_e_gravavel_por_grupo_ou_outros_nao_chama_version_nem_queue_com_controle() {
    // Negativos (E1): symlink (para um arquivo valido), modo 0620 (grupo grava), modo 0602 (outros gravam).
    // Os invalidos sao recusados antes de os validos receberem mensagem: --version nao pode ter rodado.
    // Controles: 0600 (UUID 1) e 0644 (UUID 14; leitura literal de "sem escrita para grupo/outros").
    let d = tmp();
    let f = falsos(d.path());
    let ok = thread_ok(d.path(), "thread-ok", &uuid(1));
    let alvo = thread_ok(d.path(), "alvo", &uuid(11));
    let sym = d.path().join("link");
    std::os::unix::fs::symlink(&alvo, &sym).expect("criar symlink");
    let gw = arq_thread(
        d.path(),
        "grupo",
        format!("{}\n", uuid(12)).as_bytes(),
        0o620,
    );
    let ow = arq_thread(
        d.path(),
        "outros",
        format!("{}\n", uuid(13)).as_bytes(),
        0o602,
    );
    let lr = arq_thread(
        d.path(),
        "leitura",
        format!("{}\n", uuid(14)).as_bytes(),
        0o644,
    );
    let srv = sobe_cfg(
        &d,
        &base(
            &f,
            json!({
                RECIP: recp(&ok),
                "symlink": recp(&sym),
                "grupo": recp(&gw),
                "outros": recp(&ow),
                "leitura": recp(&lr)
            }),
            600,
        ),
    );
    for rec in ["symlink", "grupo", "outros"] {
        manda(&srv, &format!("q4-{rec}"), rec);
    }
    for rec in ["symlink", "grupo", "outros"] {
        assert!(
            espera_razao(
                &srv.state,
                rec,
                "thread_file_invalid",
                Duration::from_secs(5)
            ),
            "{rec}: thread_file_invalid esperado: {:?}",
            falhas(&srv.state, rec)
        );
    }
    assert!(
        f.codex.versoes().is_empty() && f.codex.queue().is_empty(),
        "E1: arquivos invalidos nao podem rodar --version nem queue: versoes {:?}, queue {:?}",
        f.codex.versoes(),
        f.codex.queue()
    );
    manda(&srv, "q4-ok", RECIP);
    manda(&srv, "q4-leitura", "leitura");
    assert!(
        espera(Duration::from_secs(5), || f.codex.queue_de(&uuid(1)) >= 1
            && f.codex.queue_de(&uuid(14)) >= 1),
        "controle: 0600 e 0644 validos deveriam chamar queue; queue: {:?}",
        f.codex.queue()
    );
    thread::sleep(Duration::from_secs(2));
    assert_eq!(
        f.codex.queue_de(&uuid(11)),
        0,
        "symlink: o alvo nao pode ser usado"
    );
    assert_eq!(
        f.codex.queue_de(&uuid(12)),
        0,
        "modo 0620 (grupo grava) nao pode ser usado"
    );
    assert_eq!(
        f.codex.queue_de(&uuid(13)),
        0,
        "modo 0602 (outros gravam) nao pode ser usado"
    );
    assert_eq!(f.codex.queue_de(&uuid(1)), 1, "controle 0600 chama uma vez");
    assert_eq!(f.codex.queue_de(&uuid(14)), 1, "0644 chama uma vez");
}

// ---------- Q5: versao fora de codex_versions ----------

#[test]
fn q5_versao_fora_da_lista_nao_chama_queue_com_controle() {
    // Negativo: codex 0.164.0 fora de ["0.163.0"]. Controle: mesma config com 0.163.0 chama queue.
    // O controle roda em OUTRO daemon: o cache de 10 min (spec 3.2) impede trocar a versao no mesmo daemon.
    let d = tmp();
    let f = falsos(d.path());
    f.codex.versao("0.164.0");
    let ok = thread_ok(d.path(), "thread-ok", &uuid(1));
    let srv = sobe_cfg(&d, &base(&f, json!({RECIP: recp(&ok)}), 600));
    manda(&srv, "q5-1", RECIP);
    assert!(
        espera_razao(
            &srv.state,
            RECIP,
            "codex_version_unsupported",
            Duration::from_secs(5)
        ),
        "bell_failed codex_version_unsupported esperado: {:?}",
        falhas(&srv.state, RECIP)
    );
    thread::sleep(Duration::from_secs(2));
    assert!(
        f.codex.queue().is_empty(),
        "versao fora da lista nao pode chamar queue: {:?}",
        f.codex.queue()
    );
    assert!(
        !f.codex.versoes().is_empty(),
        "a versao deve ter sido consultada"
    );

    let d2 = tmp();
    let f2 = falsos(d2.path());
    let ok2 = thread_ok(d2.path(), "thread-ok", &uuid(1));
    let srv2 = sobe_cfg(&d2, &base(&f2, json!({RECIP: recp(&ok2)}), 600));
    manda(&srv2, "q5-2", RECIP);
    assert!(
        espera(Duration::from_secs(5), || f2.codex.queue_de(&uuid(1)) >= 1),
        "controle: versao 0.163.0 (na lista) deveria chamar queue"
    );
    drop(srv);
}

// ---------- Q6: queue sai com 1 ----------

#[test]
fn q6_queue_que_sai_com_1_gera_queue_failed_com_controle() {
    // Negativo: `queue` do thread UUID 2 sai 1. Controle: thread UUID 1 chama e vira bell_sent.
    let d = tmp();
    let f = falsos(d.path());
    let ok = thread_ok(d.path(), "thread-ok", &uuid(1));
    let falha = thread_ok(d.path(), "thread-falha", &uuid(2));
    f.codex.modo(&uuid(2), "falha");
    let srv = sobe_cfg(
        &d,
        &base(&f, json!({RECIP: recp(&ok), "falha": recp(&falha)}), 600),
    );
    manda(&srv, "q6-1", RECIP);
    manda(&srv, "q6-2", "falha");
    assert!(
        espera(Duration::from_secs(5), || f.codex.queue_de(&uuid(1)) >= 1),
        "controle: queue com thread 1 deveria rodar"
    );
    assert!(
        espera_razao(&srv.state, "falha", "queue_failed", Duration::from_secs(5)),
        "bell_failed queue_failed esperado: {:?}",
        falhas(&srv.state, "falha")
    );
    assert!(
        f.codex.queue_de(&uuid(2)) >= 1,
        "o queue do thread 2 deve ter sido chamado"
    );
    assert_eq!(
        enviados(&srv.state, "falha"),
        0,
        "falha nao pode virar bell_sent"
    );
    assert_eq!(
        enviados(&srv.state, RECIP),
        1,
        "controle: bell_sent do destinatario feliz"
    );
}

// ---------- Q7: timeout sem travar o daemon ----------

#[test]
fn q7_queue_que_dorme_gera_timeout_sem_travar_o_daemon() {
    // Negativo: queue do thread 2 dorme 8 s (limite 5 s). Controles: status responde durante o sono e o
    // destinatario feliz (thread 1) vira bell_sent. Sem limite inferior de tempo no spec (ver V111-TESTES.md).
    let d = tmp();
    let f = falsos(d.path());
    let ok = thread_ok(d.path(), "thread-ok", &uuid(1));
    let lento = thread_ok(d.path(), "thread-lento", &uuid(2));
    f.codex.modo(&uuid(2), "dorme");
    let srv = sobe_cfg(
        &d,
        &base(&f, json!({RECIP: recp(&ok), "lento": recp(&lento)}), 600),
    );
    manda(&srv, "q7-1", "lento");
    manda(&srv, "q7-2", RECIP);
    thread::sleep(Duration::from_secs(1));
    let t = Instant::now();
    let st = status(&srv.socket);
    let dt = t.elapsed();
    assert!(
        dt < Duration::from_secs(3),
        "status levou {dt:?} enquanto o queue dormia: daemon travado ({st})"
    );
    assert!(
        espera_razao(&srv.state, "lento", "timeout", Duration::from_secs(15)),
        "bell_failed timeout esperado: {:?}",
        falhas(&srv.state, "lento")
    );
    assert!(
        espera(Duration::from_secs(15), || enviados(&srv.state, RECIP) >= 1),
        "controle: o destinatario feliz deveria virar bell_sent"
    );
    assert_eq!(
        enviados(&srv.state, "lento"),
        0,
        "timeout nao pode virar bell_sent"
    );
    let st2 = status(&srv.socket);
    assert_eq!(st2["ok"], true, "status continua respondendo: {st2}");
}

// ---------- Q8: saida 0 sem Queued message ----------

#[test]
fn q8_saida_0_sem_queued_message_gera_queue_failed_com_controle() {
    // Negativo: queue do thread 2 sai 0 sem imprimir. Controle: thread 1 imprime e vira bell_sent.
    let d = tmp();
    let f = falsos(d.path());
    let ok = thread_ok(d.path(), "thread-ok", &uuid(1));
    let silencio = thread_ok(d.path(), "thread-silencio", &uuid(2));
    f.codex.modo(&uuid(2), "silencio");
    let srv = sobe_cfg(
        &d,
        &base(
            &f,
            json!({RECIP: recp(&ok), "silencio": recp(&silencio)}),
            600,
        ),
    );
    manda(&srv, "q8-1", RECIP);
    manda(&srv, "q8-2", "silencio");
    assert!(
        espera_razao(
            &srv.state,
            "silencio",
            "queue_failed",
            Duration::from_secs(5)
        ),
        "bell_failed queue_failed esperado para saida 0 sem texto: {:?}",
        falhas(&srv.state, "silencio")
    );
    assert_eq!(
        enviados(&srv.state, "silencio"),
        0,
        "saida 0 sem texto nao e sucesso"
    );
    assert!(
        espera(Duration::from_secs(5), || enviados(&srv.state, RECIP) >= 1),
        "controle: a saida com `Queued message` deveria ser bell_sent"
    );
}

// ---------- Q9: rearme ----------

#[test]
fn q9_segundo_ciclo_dentro_do_rearme_nao_chama_de_novo() {
    // Negativo: rearme de 600 s, pendencia aberta; apos o primeiro toque, 3 s depois continua 1 chamada.
    // Controle: o primeiro toque acontece (espera) no mesmo cenario. Q9b mostra que o rearme e a causa.
    let d = tmp();
    let f = falsos(d.path());
    let ok = thread_ok(d.path(), "thread-ok", &uuid(1));
    let srv = sobe_cfg(&d, &base(&f, json!({RECIP: recp(&ok)}), 600));
    manda(&srv, "q9-1", RECIP);
    assert!(
        espera(Duration::from_secs(5), || f.codex.queue_de(&uuid(1)) >= 1),
        "controle: o primeiro toque deveria acontecer"
    );
    thread::sleep(Duration::from_secs(3));
    assert_eq!(
        f.codex.queue_de(&uuid(1)),
        1,
        "dentro do rearme de 600 s nao pode haver segundo queue: {:?}",
        f.codex.queue()
    );
}

#[test]
fn q9b_com_rearme_vencido_o_ciclo_volta_e_chama_de_novo() {
    // Par positivo de Q9: rearme de 2 s, mesma pendencia aberta; depois do rearme deve chamar de novo.
    let d = tmp();
    let f = falsos(d.path());
    let ok = thread_ok(d.path(), "thread-ok", &uuid(1));
    let srv = sobe_cfg(&d, &base(&f, json!({RECIP: recp(&ok)}), 2));
    manda(&srv, "q9b-1", RECIP);
    assert!(
        espera(Duration::from_secs(10), || f.codex.queue_de(&uuid(1)) >= 2),
        "rearme vencido com a pendencia aberta deveria chamar de novo: {:?}",
        f.codex.queue()
    );
}

// ---------- Q10: accepted apaga o rearme ----------

#[test]
fn q10_accepted_da_pendencia_apaga_o_rearme_com_controle() {
    // Positivo: A tem bell_sent ha 10 s, mas a pendencia foi aceita; chegou nova -> chama.
    // Negativo: B tem bell_sent ha 10 s, sem accepted -> nao chama. Os dois no mesmo daemon.
    let d = tmp();
    let f = falsos(d.path());
    let ta = thread_ok(d.path(), "ta", &uuid(1));
    let tb = thread_ok(d.path(), "tb", &uuid(2));
    let state = d.path().join("state");
    let mut dia = Diario::novo(&state);
    dia.registra("q10-1", CLAUDE, "a", &ha(30));
    dia.campainha("a", &ha(10));
    dia.aceita("q10-1", &ha(9));
    dia.registra("q10-2", CLAUDE, "a", &ha(8));
    dia.registra("q10-3", CLAUDE, "b", &ha(30));
    dia.campainha("b", &ha(10));
    dia.registra("q10-4", CLAUDE, "b", &ha(8));
    let c = escreve_config(
        d.path(),
        &base(&f, json!({"a": recp(&ta), "b": recp(&tb)}), 600),
    );
    let _srv = sobe(&state, &c);
    assert!(
        espera(Duration::from_secs(5), || f.codex.queue_de(&uuid(1)) >= 1),
        "accepted deveria ter apagado o rearme de A: {:?}",
        f.codex.queue()
    );
    thread::sleep(Duration::from_secs(2));
    assert_eq!(
        f.codex.queue_de(&uuid(2)),
        0,
        "B sem accepted continua no rearme de 600 s"
    );
}

// ---------- Q11: configuracao recusada ----------

#[test]
fn q11_configuracao_invalida_do_codex_queue_e_recusada_com_controle() {
    // Cada caso e recusado (saida com codigo != 0, sem ficar pronto). Formato da saida de erro: AMBIGUO (V111-TESTES.md).
    // Controle: a config valida sobe; e a config antiga, sem as chaves novas, tambem sobe.
    let d0 = tmp();
    let f0 = falsos(d0.path());
    let t = thread_ok(d0.path(), "t", &uuid(1));
    let abs = s(&t).to_string();
    let rec = |r: Value| base(&f0, json!({RECIP: r}), 600);

    let mut sem_versoes = rec(recp(&t));
    sem_versoes
        .as_object_mut()
        .unwrap()
        .remove("codex_versions");
    let mut versoes_vazias = rec(recp(&t));
    versoes_vazias
        .as_object_mut()
        .unwrap()
        .insert("codex_versions".into(), json!([]));
    let mut chave_extra = rec(recp(&t));
    chave_extra
        .as_object_mut()
        .unwrap()
        .insert("chave_inventada".into(), json!(1));
    let mut bin_vazio = rec(recp(&t));
    bin_vazio
        .as_object_mut()
        .unwrap()
        .insert("codex_bin".into(), json!(""));

    let casos: Vec<(&str, Value)> = vec![
        ("sem thread_file", rec(json!({"delivery": "codex-queue"}))),
        (
            "com pane",
            rec(json!({"delivery": "codex-queue", "thread_file": abs.clone(), "pane": PANE_CODEX})),
        ),
        (
            "thread_file relativo",
            rec(json!({"delivery": "codex-queue", "thread_file": "relativo/thread"})),
        ),
        ("sem codex_versions", sem_versoes),
        ("codex_versions vazio", versoes_vazias),
        (
            "thread_file com delivery watch",
            rec(json!({"delivery": "watch", "thread_file": abs.clone()})),
        ),
        (
            "thread_file com delivery bell",
            rec(json!({"delivery": "bell", "pane": PANE_CODEX, "thread_file": abs.clone()})),
        ),
        ("chave global desconhecida", chave_extra),
        ("codex_bin vazio", bin_vazio),
    ];
    for (nome, cfg) in &casos {
        let d = tmp();
        let c = escreve_config(d.path(), cfg);
        let st = d.path().join("state");
        match tenta(
            &["serve", "--state-dir", s(&st), "--config", s(&c)],
            &[],
            &st,
            &st.join("chatd.sock"),
        ) {
            Ok(_srv) => panic!("caso '{nome}' deveria ser recusado"),
            Err(e) => assert!(
                e.code.is_some() && e.code != Some(0),
                "caso '{nome}': recusa com codigo de erro, veio {e:?}"
            ),
        }
    }

    // Controle positivo: config valida sobe e aceita o codex-queue.
    let d = tmp();
    let f = falsos(d.path());
    let t = thread_ok(d.path(), "t", &uuid(1));
    let c = escreve_config(d.path(), &base(&f, json!({RECIP: recp(&t)}), 600));
    let st = d.path().join("state");
    let ok = tenta(
        &["serve", "--state-dir", s(&st), "--config", s(&c)],
        &[],
        &st,
        &st.join("chatd.sock"),
    );
    assert!(
        ok.is_ok(),
        "controle: config valida com codex-queue deveria subir: {:?}",
        ok.err()
    );
    drop(ok);

    // Controle 2: config antiga (sem codex_bin nem codex_versions) continua subindo.
    let d = tmp();
    let f = falsos(d.path());
    let mut legado = base(&f, json!({"x": {"delivery": "watch"}}), 600);
    legado.as_object_mut().unwrap().remove("codex_versions");
    legado.as_object_mut().unwrap().remove("codex_bin");
    let c = escreve_config(d.path(), &legado);
    let st = d.path().join("state");
    let ok = tenta(
        &["serve", "--state-dir", s(&st), "--config", s(&c)],
        &[],
        &st,
        &st.join("chatd.sock"),
    );
    assert!(
        ok.is_ok(),
        "controle: config sem chaves do codex-queue deve subir: {:?}",
        ok.err()
    );
}

// ---------- Q12: herdr e tmux nao recebem chamada do codex-queue ----------

#[test]
fn q12_herdr_e_tmux_falsos_nao_recebem_chamada_do_codex_queue() {
    // Negativo: herdr so pode receber chamadas do painel do codex-web; tmux nenhuma (em nenhuma falha).
    // Controles: queue feliz, falha de queue (queue_failed) e agent prompt do codex-web (herdr) no mesmo daemon.
    // Controle do tmux: AUSENTE (nao ha bind tmux sem auth; o teste so prova que tmux nao e chamado).
    let d = tmp();
    let f = falsos(d.path());
    let ok = thread_ok(d.path(), "thread-ok", &uuid(1));
    let falha = thread_ok(d.path(), "thread-falha", &uuid(2));
    f.codex.modo(&uuid(2), "falha");
    let srv = sobe_cfg(
        &d,
        &base(
            &f,
            json!({
                RECIP: recp(&ok),
                "falha": recp(&falha),
                CODEX_WEB: {"delivery": "bell", "pane": PANE_CODEX}
            }),
            600,
        ),
    );
    manda(&srv, "q12-1", RECIP);
    manda(&srv, "q12-2", "falha");
    manda(&srv, "q12-3", CODEX_WEB);
    assert!(
        espera(Duration::from_secs(5), || f.codex.queue_de(&uuid(1)) >= 1),
        "controle: queue do codex-queue deveria rodar"
    );
    assert!(
        espera(Duration::from_secs(5), || prompts_herdr(
            d.path(),
            PANE_CODEX
        ) >= 1),
        "controle: a campainha herdr do codex-web deveria chamar agent prompt: {:?}",
        chamadas(d.path(), "calls_herdr.log")
    );
    assert!(
        espera_razao(&srv.state, "falha", "queue_failed", Duration::from_secs(5)),
        "controle: a falha de queue deveria gerar queue_failed"
    );
    thread::sleep(Duration::from_secs(1));
    let herdr = chamadas(d.path(), "calls_herdr.log");
    assert!(
        herdr
            .iter()
            .all(|c| c.get(2).map(String::as_str) == Some(PANE_CODEX)),
        "o herdr so pode receber chamadas do painel {PANE_CODEX}: {herdr:?}"
    );
    assert!(
        chamadas(d.path(), "calls_tmux.log").is_empty(),
        "o tmux falso nao pode receber nenhuma chamada: {:?}",
        chamadas(d.path(), "calls_tmux.log")
    );
}

// ---------- Q13: nenhum tipo de evento novo ----------

#[test]
fn q13_nenhum_tipo_de_evento_novo_no_log() {
    // Negativo: os tipos do log com sucesso, falha de queue, arquivo ausente e saida silenciosa.
    // Controle: o conjunto tem bell_sent e bell_failed (nao esta vazio).
    let d = tmp();
    let f = falsos(d.path());
    let ok = thread_ok(d.path(), "thread-ok", &uuid(1));
    let falha = thread_ok(d.path(), "thread-falha", &uuid(2));
    let silencio = thread_ok(d.path(), "thread-silencio", &uuid(4));
    f.codex.modo(&uuid(2), "falha");
    f.codex.modo(&uuid(4), "silencio");
    let srv = sobe_cfg(
        &d,
        &base(
            &f,
            json!({
                RECIP: recp(&ok),
                "falha": recp(&falha),
                "ausente": recp(&d.path().join("nao-existe")),
                "silencio": recp(&silencio)
            }),
            600,
        ),
    );
    for (i, r) in [RECIP, "falha", "ausente", "silencio"].iter().enumerate() {
        manda(&srv, &format!("q13-{i}"), r);
    }
    let state = srv.state.clone();
    assert!(
        espera(Duration::from_secs(10), || f.codex.queue_de(&uuid(1)) >= 1
            && !falhas(&state, "falha").is_empty()
            && !falhas(&state, "ausente").is_empty()
            && !falhas(&state, "silencio").is_empty()),
        "controle: os quatro desfechos deveriam aparecer no log"
    );
    thread::sleep(Duration::from_secs(1));
    let tps = tipos(&state);
    let novos: Vec<&String> = tps
        .iter()
        .filter(|t| !TIPOS_EXISTENTES.contains(&t.as_str()))
        .collect();
    assert!(novos.is_empty(), "tipos de evento novos no log: {novos:?}");
    assert!(
        tps.contains("bell_sent") && tps.contains("bell_failed"),
        "controle: o log deve ter bell_sent e bell_failed: {tps:?}"
    );
}

// ---------- Q14: texto so com a contagem ----------

#[test]
fn q14_texto_enviado_so_tem_a_contagem_com_controle() {
    // Negativo: assunto e corpo com aspas, $(), crase, ponto e virgula nao aparecem no argv nem no log do codex.
    // Controle: o argv e exatamente TEXTO e o queue roda uma vez.
    let d = tmp();
    let f = falsos(d.path());
    let ok = thread_ok(d.path(), "thread-ok", &uuid(1));
    let srv = sobe_cfg(&d, &base(&f, json!({RECIP: recp(&ok)}), 600));
    let assunto = r##"assunto "aspas" 'simples' $(touch X) ; rm -rf x && `id` | $HOME"##;
    let corpo = "linha 1\nlinha 2 $(id) \"quoted\" ; touch Y";
    let m = msg("q14-1", CLAUDE, RECIP, "AVISO", assunto, corpo);
    res(&envia(&srv.socket, m));
    assert!(
        espera(Duration::from_secs(5), || f.codex.queue_de(&uuid(1)) >= 1),
        "controle: queue deveria rodar"
    );
    thread::sleep(Duration::from_secs(1));
    assert_eq!(
        f.codex.queue(),
        vec![argv_queue(&uuid(1), TEXTO)],
        "o argv deve ser exatamente o texto fixo da contagem"
    );
    let bruto = fs::read_to_string(f.codex.dir.join("calls_queue.log")).unwrap_or_default();
    for pedaco in [
        "aspas",
        "touch",
        "linha",
        "quoted",
        "claude-main",
        "assunto",
    ] {
        assert!(
            !bruto.contains(pedaco),
            "'{pedaco}' vazou para o codex: {bruto:?}"
        );
    }
}

// ---------- Q15: codex_bin com espacos e metacaracteres ----------

#[test]
fn q15_codex_bin_com_espacos_e_metacaracteres_executa_como_um_argv() {
    // Negativo implicito: se o daemon passasse por shell, o caminho quebraria e queue nao rodaria.
    // Controle: o mesmo caminho executa com argv exato. Sem marcador de efeito colateral: o cwd do daemon nao e
    // controlado pelo teste (ver V111-TESTES.md).
    let d = tmp();
    let pasta = d.path().join("pasta com espaco");
    fs::create_dir_all(&pasta).unwrap();
    let codex = Codex::em(&pasta, "codex falso ; $(false) & 'x'");
    let f = Falsos {
        codex,
        herdr: escreve_script(d.path(), "herdr-falso.sh", HERDR_SH),
        tmux: escreve_script(d.path(), "tmux-falso.sh", TMUX_SH),
    };
    let ok = thread_ok(d.path(), "thread-ok", &uuid(1));
    let srv = sobe_cfg(&d, &base(&f, json!({RECIP: recp(&ok)}), 600));
    manda(&srv, "q15-1", RECIP);
    assert!(
        espera(Duration::from_secs(5), || f.codex.queue_de(&uuid(1)) >= 1),
        "codex_bin com espacos e metacaracteres deveria ser executado como um argv: versoes {:?}",
        f.codex.versoes()
    );
    thread::sleep(Duration::from_secs(1));
    assert_eq!(
        f.codex.queue(),
        vec![argv_queue(&uuid(1), TEXTO)],
        "uma chamada, com argv exato"
    );
    assert_eq!(enviados(&srv.state, RECIP), 1, "bell_sent do destinatario");
}

// ---------- Q16: reinicio dentro do rearme ----------

#[test]
fn q16_reinicio_dentro_do_rearme_nao_toca_de_novo() {
    // Tecnica de V7 (log pre-gravado): A tocado ha 10 s (rearme 600 s) nao toca; B tocado ha 700 s toca (controle).
    let d = tmp();
    let f = falsos(d.path());
    let ta = thread_ok(d.path(), "ta", &uuid(1));
    let tb = thread_ok(d.path(), "tb", &uuid(2));
    let state = d.path().join("state");
    let mut dia = Diario::novo(&state);
    dia.registra("q16-a", CLAUDE, "a", &ha(30));
    dia.campainha("a", &ha(10));
    dia.registra("q16-b", CLAUDE, "b", &ha(30));
    dia.campainha("b", &ha(700));
    let c = escreve_config(
        d.path(),
        &base(&f, json!({"a": recp(&ta), "b": recp(&tb)}), 600),
    );
    let _srv = sobe(&state, &c);
    assert!(
        espera(Duration::from_secs(5), || f.codex.queue_de(&uuid(2)) >= 1),
        "controle: B com bell_sent de 700 s deveria tocar"
    );
    thread::sleep(Duration::from_secs(2));
    assert_eq!(
        f.codex.queue_de(&uuid(1)),
        0,
        "A tocado ha 10 s nao pode tocar ao iniciar: {:?}",
        f.codex.queue()
    );
}

#[test]
fn q16b_reinicio_vivo_dentro_do_rearme_nao_toca_de_novo() {
    // Reinicio de verdade: toca A, para o daemon, sobe de novo; A nao toca (rearme 600 s).
    // Controle: depois do reinicio, uma mensagem nova para B toca.
    let d = tmp();
    let f = falsos(d.path());
    let ta = thread_ok(d.path(), "ta", &uuid(1));
    let tb = thread_ok(d.path(), "tb", &uuid(2));
    let cfg = escreve_config(
        d.path(),
        &base(&f, json!({RECIP: recp(&ta), "outro": recp(&tb)}), 600),
    );
    let state = d.path().join("state");
    let mut srv = sobe(&state, &cfg);
    manda(&srv, "q16b-1", RECIP);
    assert!(
        espera(Duration::from_secs(5), || f.codex.queue_de(&uuid(1)) >= 1),
        "primeiro toque de A deveria acontecer"
    );
    assert!(srv.para().is_some(), "daemon nao saiu com SIGTERM");
    drop(srv);
    let srv2 = sobe(&state, &cfg);
    manda(&srv2, "q16b-2", "outro");
    assert!(
        espera(Duration::from_secs(5), || f.codex.queue_de(&uuid(2)) >= 1),
        "controle: depois do reinicio, B com mensagem nova deveria tocar"
    );
    thread::sleep(Duration::from_secs(2));
    assert_eq!(
        f.codex.queue_de(&uuid(1)),
        1,
        "reiniciar dentro do rearme nao pode tocar A de novo"
    );
}

// ---------- Q17: thread_file relido a cada toque ----------

#[test]
fn q17_thread_file_e_relido_a_cada_toque() {
    // Rearme de 2 s para ter um segundo toque sem novas mensagens. Depois de trocar o conteudo do arquivo,
    // o toque seguinte usa o UUID novo, e nenhum toque depois da troca usa o antigo.
    let d = tmp();
    let f = falsos(d.path());
    let arq = thread_ok(d.path(), "thr", &uuid(1));
    let srv = sobe_cfg(&d, &base(&f, json!({RECIP: recp(&arq)}), 2));
    manda(&srv, "q17-1", RECIP);
    assert!(
        espera(Duration::from_secs(5), || f.codex.queue_de(&uuid(1)) >= 1),
        "controle: o primeiro toque deve usar o UUID 1"
    );
    fs::write(&arq, format!("{}\n", uuid(2))).expect("trocar conteudo do thread_file");
    fs::set_permissions(&arq, fs::Permissions::from_mode(0o600)).expect("chmod");
    assert!(
        espera(Duration::from_secs(10), || f.codex.queue_de(&uuid(2)) >= 1),
        "o toque seguinte deve usar o UUID novo do arquivo: {:?}",
        f.codex.queue()
    );
    let calls = f.codex.queue();
    let primeiro_2 = calls
        .iter()
        .position(|c| c.get(2).map(String::as_str) == Some(uuid(2).as_str()))
        .expect("ha um toque com UUID 2");
    assert!(
        calls[primeiro_2..]
            .iter()
            .all(|c| c.get(2).map(String::as_str) != Some(uuid(1).as_str())),
        "depois da troca nenhum toque pode usar o UUID antigo: {calls:?}"
    );
    assert_eq!(
        calls[primeiro_2],
        argv_queue(&uuid(2), TEXTO),
        "argv do toque novo"
    );
}

// ---------- Q18 (extra): spawn_failed ----------

#[test]
fn q18_binario_que_deixa_de_ser_executavel_gera_spawn_failed_com_controle() {
    // Extra (spec 3.4, passo 4: "nao conseguiu executar -> spawn_failed"). Depende do cache da versao (10 min):
    // tirado o bit de execucao depois do primeiro toque, o queue nao consegue executar. Controle: o primeiro toque.
    let d = tmp();
    let f = falsos(d.path());
    let ok = thread_ok(d.path(), "thread-ok", &uuid(1));
    let spawn = thread_ok(d.path(), "thread-spawn", &uuid(2));
    let srv = sobe_cfg(
        &d,
        &base(&f, json!({RECIP: recp(&ok), "spawn": recp(&spawn)}), 600),
    );
    manda(&srv, "q18-1", RECIP);
    assert!(
        espera(Duration::from_secs(5), || f.codex.queue_de(&uuid(1)) >= 1),
        "controle: o primeiro toque deve executar"
    );
    fs::set_permissions(&f.codex.bin, fs::Permissions::from_mode(0o644)).expect("tirar exec");
    manda(&srv, "q18-2", "spawn");
    assert!(
        espera_razao(&srv.state, "spawn", "spawn_failed", Duration::from_secs(10)),
        "bell_failed spawn_failed esperado: {:?}",
        falhas(&srv.state, "spawn")
    );
    assert_eq!(
        f.codex.queue_de(&uuid(2)),
        0,
        "sem exec, nao ha chamada de queue"
    );
    assert_eq!(
        enviados(&srv.state, "spawn"),
        0,
        "spawn_failed nao e sucesso"
    );
}

// ---------- Q19: nova tentativa depois de queue_failed (E4) ----------

#[test]
fn q19_depois_de_queue_failed_nova_tentativa_so_apos_60_s() {
    // E4. Negativo: no maximo a chamada inicial em 55 s depois da primeira vista (o limite de 60 s conta da falha).
    // Positivo: ha nova chamada depois de 60 s. Controle: o destinatario feliz roda no mesmo daemon.
    // Sem relogio acelerado: o teste leva cerca de 85 s. A falha ocorre logo depois da primeira chamada,
    // entao a janela de 55 s fica dentro do limite de 60 s.
    let d = tmp();
    let f = falsos(d.path());
    let ok = thread_ok(d.path(), "thread-ok", &uuid(1));
    let falha = thread_ok(d.path(), "thread-falha", &uuid(2));
    f.codex.modo(&uuid(2), "falha");
    let srv = sobe_cfg(
        &d,
        &base(&f, json!({RECIP: recp(&ok), "falha": recp(&falha)}), 600),
    );
    manda(&srv, "q19-1", RECIP);
    manda(&srv, "q19-2", "falha");
    assert!(
        espera(Duration::from_secs(5), || f.codex.queue_de(&uuid(2)) >= 1),
        "queue do thread 2 deveria rodar"
    );
    let t0 = Instant::now();
    assert!(
        espera_razao(&srv.state, "falha", "queue_failed", Duration::from_secs(5)),
        "queue_failed esperado: {:?}",
        falhas(&srv.state, "falha")
    );
    assert!(
        espera(Duration::from_secs(5), || f.codex.queue_de(&uuid(1)) >= 1),
        "controle: o destinatario feliz deveria rodar queue"
    );
    thread::sleep(Duration::from_secs(55).saturating_sub(t0.elapsed()));
    let antes = f.codex.queue_de(&uuid(2));
    assert!(
        antes <= 1,
        "E4: nova tentativa antes de 60 s depois de queue_failed: {antes} chamadas: {:?}",
        f.codex.queue()
    );
    assert!(
        espera(Duration::from_secs(30), || f.codex.queue_de(&uuid(2)) >= 2),
        "E4: depois de 60 s deve haver nova tentativa: {:?}",
        f.codex.queue()
    );
    assert_eq!(enviados(&srv.state, "falha"), 0, "falha nao vira bell_sent");
}

// ---------- Q20: nova tentativa depois de timeout (E4) ----------

#[test]
fn q20_depois_de_timeout_nova_tentativa_so_apos_bell_rearm_s() {
    // E4. Rearme de 4 s (pequeno). Negativo: depois do timeout observado, nenhuma chamada nova nos 3 s seguintes.
    // Positivo: depois do rearme, a nova tentativa acontece (o modo do thread vira caso feliz e vira bell_sent).
    // Controle: o destinatario feliz no mesmo daemon. Medido a partir do bell_failed (E4: "apos timeout").
    let d = tmp();
    let f = falsos(d.path());
    let ok = thread_ok(d.path(), "thread-ok", &uuid(1));
    let lento = thread_ok(d.path(), "thread-lento", &uuid(2));
    f.codex.modo(&uuid(2), "dorme");
    let srv = sobe_cfg(
        &d,
        &base(&f, json!({RECIP: recp(&ok), "lento": recp(&lento)}), 4),
    );
    manda(&srv, "q20-1", "lento");
    manda(&srv, "q20-2", RECIP);
    assert!(
        espera_razao(&srv.state, "lento", "timeout", Duration::from_secs(15)),
        "bell_failed timeout esperado: {:?}",
        falhas(&srv.state, "lento")
    );
    let t_obs = Instant::now();
    assert!(
        espera(Duration::from_secs(5), || f.codex.queue_de(&uuid(1)) >= 1),
        "controle: o destinatario feliz deveria rodar queue"
    );
    assert_eq!(
        f.codex.queue_de(&uuid(2)),
        1,
        "antes do timeout observado so ha a chamada inicial"
    );
    // Proxima tentativa vai dar certo (caso feliz), para o controle positivo.
    f.codex.modo_limpa(&uuid(2));
    thread::sleep(Duration::from_secs(3).saturating_sub(t_obs.elapsed()));
    assert_eq!(
        f.codex.queue_de(&uuid(2)),
        1,
        "E4: nova tentativa antes de bell_rearm_s (4 s) depois do timeout: {:?}",
        f.codex.queue()
    );
    assert!(
        espera(Duration::from_secs(10), || enviados(&srv.state, "lento")
            >= 1),
        "E4: depois do rearme a nova tentativa deve acontecer e dar bell_sent: {:?}",
        f.codex.queue()
    );
    assert!(
        f.codex.queue_de(&uuid(2)) >= 2,
        "a nova tentativa chamou queue"
    );
}

// ---------- Q21: falha de thread_file nao executa nem --version (E1 a) ----------

#[test]
fn q21_falha_de_thread_file_nao_executa_nem_version() {
    // E1 (a). Tres kinds de falha no mesmo daemon: ausente, conteudo "--help" e symlink para arquivo valido.
    // Nenhum deles pode rodar --version nem queue. Controle: o destinatario valido, enviado depois, roda os dois.
    let d = tmp();
    let f = falsos(d.path());
    let ok = thread_ok(d.path(), "thread-ok", &uuid(1));
    let ausente = d.path().join("nao-existe");
    let ruim = arq_thread(d.path(), "ruim", b"--help\n", 0o600);
    let alvo = thread_ok(d.path(), "alvo", &uuid(11));
    let sym = d.path().join("link");
    std::os::unix::fs::symlink(&alvo, &sym).expect("criar symlink");
    let srv = sobe_cfg(
        &d,
        &base(
            &f,
            json!({
                RECIP: recp(&ok),
                "ausente": recp(&ausente),
                "ruim": recp(&ruim),
                "link": recp(&sym)
            }),
            600,
        ),
    );
    manda(&srv, "q21-1", "ausente");
    manda(&srv, "q21-2", "ruim");
    manda(&srv, "q21-3", "link");
    assert!(espera_razao(
        &srv.state,
        "ausente",
        "thread_file_missing",
        Duration::from_secs(5)
    ));
    assert!(espera_razao(
        &srv.state,
        "ruim",
        "thread_file_invalid",
        Duration::from_secs(5)
    ));
    assert!(espera_razao(
        &srv.state,
        "link",
        "thread_file_invalid",
        Duration::from_secs(5)
    ));
    assert!(
        f.codex.versoes().is_empty(),
        "E1: --version nao pode rodar: {:?}",
        f.codex.versoes()
    );
    assert!(
        f.codex.queue().is_empty(),
        "E1: queue nao pode rodar: {:?}",
        f.codex.queue()
    );
    manda(&srv, "q21-ok", RECIP);
    assert!(
        espera(Duration::from_secs(5), || f.codex.queue_de(&uuid(1)) >= 1),
        "controle: o destinatario valido deveria rodar queue"
    );
    assert!(
        !f.codex.versoes().is_empty(),
        "controle: o destinatario valido deveria consultar a versao"
    );
}

// ---------- Q22 (E5): --version que nao executa vira spawn_failed ----------

#[test]
fn q22_version_que_nao_executa_gera_spawn_failed_com_controle() {
    // E5. codex_bin sem bit de execucao desde o inicio: --version nao executa -> spawn_failed, queue nao roda.
    // Cache so guarda sucesso: com o bit de volta, outro destinatario roda --version e queue (controle).
    let d = tmp();
    let f = falsos(d.path());
    fs::set_permissions(&f.codex.bin, fs::Permissions::from_mode(0o644)).expect("tirar exec");
    let ok = thread_ok(d.path(), "thread-ok", &uuid(1));
    let outro = thread_ok(d.path(), "thread-outro", &uuid(2));
    let srv = sobe_cfg(
        &d,
        &base(&f, json!({RECIP: recp(&ok), "outro": recp(&outro)}), 600),
    );
    manda(&srv, "q22-1", RECIP);
    assert!(
        espera_razao(&srv.state, RECIP, "spawn_failed", Duration::from_secs(10)),
        "E5: bell_failed spawn_failed esperado para --version que nao executa: {:?}",
        falhas(&srv.state, RECIP)
    );
    assert!(
        f.codex.versoes().is_empty() && f.codex.queue().is_empty(),
        "nada executou: versoes {:?}, queue {:?}",
        f.codex.versoes(),
        f.codex.queue()
    );
    assert_eq!(enviados(&srv.state, RECIP), 0, "spawn_failed nao e sucesso");
    fs::set_permissions(&f.codex.bin, fs::Permissions::from_mode(0o755)).expect("devolver exec");
    manda(&srv, "q22-2", "outro");
    assert!(
        espera(Duration::from_secs(10), || f.codex.queue_de(&uuid(2)) >= 1),
        "controle: falha de --version nao pode ficar em cache; o outro destinatario deveria rodar"
    );
}

// ---------- Q23 (E5): --version que dorme mais de 5 s vira timeout ----------

#[test]
fn q23_version_que_dorme_gera_timeout_e_nao_chama_queue_com_controle() {
    // E5. `--version` dorme 8 s (o daemon corta em 5 s). Negativos: a razao e `timeout` (nao
    // codex_version_unsupported), `queue` nao roda, e o daemon continua respondendo `status` durante o sono.
    // Controle: no mesmo daemon, com o `--version` de volta ao normal, o destinatario "outro" roda --version
    // e queue e vira bell_sent (falha de versao nao fica em cache).
    let d = tmp();
    let f = falsos(d.path());
    f.codex.versao_dorme(true);
    let ok = thread_ok(d.path(), "thread-ok", &uuid(1));
    let outro = thread_ok(d.path(), "thread-outro", &uuid(2));
    let srv = sobe_cfg(
        &d,
        &base(&f, json!({RECIP: recp(&ok), "outro": recp(&outro)}), 600),
    );
    manda(&srv, "q23-1", RECIP);
    thread::sleep(Duration::from_secs(1));
    let t = Instant::now();
    let st = status(&srv.socket);
    let dt = t.elapsed();
    assert!(
        dt < Duration::from_secs(3),
        "status levou {dt:?} enquanto o --version dormia: daemon travado ({st})"
    );
    assert!(
        espera_razao(&srv.state, RECIP, "timeout", Duration::from_secs(15)),
        "E5: bell_failed com razao timeout esperado para --version que dorme: {:?}",
        falhas(&srv.state, RECIP)
    );
    assert!(
        !falhas(&srv.state, RECIP)
            .iter()
            .any(|e| e["reason"] == "codex_version_unsupported"),
        "E5: timeout nao pode virar codex_version_unsupported: {:?}",
        falhas(&srv.state, RECIP)
    );
    assert!(
        f.codex.queue().is_empty(),
        "E5: queue nao pode rodar quando o --version falha: {:?}",
        f.codex.queue()
    );
    assert_eq!(enviados(&srv.state, RECIP), 0, "timeout nao e sucesso");
    f.codex.versao_dorme(false);
    manda(&srv, "q23-2", "outro");
    assert!(
        espera(Duration::from_secs(10), || f.codex.queue_de(&uuid(2)) >= 1),
        "controle: com o --version de volta ao normal, o destinatario 'outro' deveria rodar queue: {:?}",
        f.codex.queue()
    );
    assert!(
        espera(Duration::from_secs(5), || enviados(&srv.state, "outro")
            >= 1),
        "controle: o destinatario 'outro' deveria virar bell_sent: {:?}",
        falhas(&srv.state, "outro")
    );
}

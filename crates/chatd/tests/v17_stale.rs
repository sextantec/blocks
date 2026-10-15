//! v1.7 (docs/spec-v1.7-stale.md): `chatd stale`, verificador de mensagens paradas (S1 a S12).
//! Escrito so a partir do contrato e dos modelos tests/common e tests/v16_identidade_reconexao.rs; src/ nao foi lido.
//! Caixa preta: so o binario e os arquivos. Cada teste tem diretorio temporario proprio; HOME e XDG_CONFIG_HOME
//! apontam para dentro dele. Nenhum processo real, nenhuma rede, nenhum token real, nenhum acesso ao estado real.
//! O log.jsonl e montado aqui, com `ts` calculados em relacao ao relogio atual.
//! Devem falhar ate a implementacao existir (`chatd stale` ainda nao existe: erro de uso, saida 2).
//! AMBIGUO: marcado nos testes onde o contrato nao fixa o texto, o formato ou o codigo de saida lido.
#![allow(dead_code)]
mod common;

use chrono::{SecondsFormat, TimeDelta, Utc};
use common::*;
use serde_json::{Value, json};
use std::ffi::CString;
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::{PermissionsExt, symlink};
use std::os::unix::net::UnixListener;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::thread;
use std::time::{Duration, Instant};

const A: &str = "claude-main";
const C: &str = "terceiro";
const B: &str = "codex-web";
/// Marcas unicas de assunto e corpo (S6): nunca podem aparecer em saida, stderr nem stamp-file.
const MARCA_ASSUNTO: &str = "ASSUNTO-SECRETO-V17-S6";
const MARCA_CORPO: &str = "CORPO-SECRETO-V17-S6";

/// Variaveis lidas pelo cliente; removidas antes de cada execucao.
const VARS_LIMPAS: &[&str] = &[
    "CHATD_SOCKET",
    "CHATD_STATE_DIR",
    "CHATD_AS",
    "CHATD_TOKEN_FILE",
    "CHATD_PROFILE",
    "CHATD_CONFIG",
    "HERDR_PANE_ID",
];

static CONTADOR: AtomicUsize = AtomicUsize::new(0);

// ---------- infraestrutura ----------

/// Diretorio temporario com `home/` e `xdg/` (HOME e XDG_CONFIG_HOME apontam para la).
struct Amb {
    d: tempfile::TempDir,
}

impl Amb {
    fn novo() -> Amb {
        let d = tmp();
        fs::create_dir_all(d.path().join("home")).unwrap();
        fs::create_dir_all(d.path().join("xdg")).unwrap();
        Amb { d }
    }
    fn p(&self) -> &Path {
        self.d.path()
    }
    fn home(&self) -> PathBuf {
        self.p().join("home")
    }
    fn xdg(&self) -> PathBuf {
        self.p().join("xdg")
    }
    /// Estado de teste (dentro do temporario, nunca o estado real).
    fn state(&self) -> PathBuf {
        self.p().join("state")
    }
    fn stamp(&self) -> PathBuf {
        self.p().join("stamp.json")
    }
    fn token(&self, nome: &str) -> PathBuf {
        self.p().join(format!("{nome}.tok"))
    }
}

/// Grava `conteudo` em `p` com o modo dado (fixado depois, para ignorar a umask).
fn escreve(p: &Path, conteudo: &str, modo: u32) -> PathBuf {
    fs::create_dir_all(p.parent().unwrap()).unwrap();
    fs::write(p, conteudo).unwrap();
    fs::set_permissions(p, fs::Permissions::from_mode(modo)).unwrap();
    p.to_path_buf()
}

/// Cria um FIFO no caminho dado. Se o chatd abrir esse arquivo, bloqueia: o `roda` estoura (code None).
fn fifo(p: &Path) {
    fs::create_dir_all(p.parent().unwrap()).unwrap();
    let c = CString::new(p.as_os_str().as_bytes()).unwrap();
    let r = unsafe { libc::mkfifo(c.as_ptr(), 0o600) };
    assert_eq!(r, 0, "mkfifo em {}", p.display());
}

/// Instante `min` minutos antes de agora, em RFC 3339 UTC com milissegundos (formato do log).
fn ha_min(min: i64) -> String {
    ha_seg(min * 60)
}

fn ha_seg(seg: i64) -> String {
    (Utc::now() - TimeDelta::seconds(seg)).to_rfc3339_opts(SecondsFormat::Millis, true)
}

/// Campos de um evento `registered` (o contrato lista estes campos; o log os traz no nivel de cima da linha).
fn campos_registro(id: &str, de: &str, para: &str, assunto: &str, corpo: &str, ts: &str) -> Value {
    json!({
        "id": id, "from": de, "to": para, "kind": "AVISO",
        "subject": assunto, "body": corpo, "thread": null,
        "state": "registered", "refs": [], "note": null,
        "registered_at": ts, "accepted_at": null, "resulted_at": null, "result": null
    })
}

/// Monta o log.jsonl como o daemon o escreveria: uma linha JSON por evento, com `v`, `seq` (crescente, a partir
/// de 1) e `ts`. AMBIGUO: o contrato lista os campos de cada tipo; aqui eles ficam no nivel de cima da linha.
struct Diario {
    arq: PathBuf,
    seq: u64,
}

impl Diario {
    /// Cria `state/log.jsonl` vazio (cria o diretorio se preciso).
    fn novo(state: &Path) -> Diario {
        fs::create_dir_all(state).unwrap();
        let arq = state.join("log.jsonl");
        fs::write(&arq, "").unwrap();
        Diario { arq, seq: 0 }
    }

    fn evento(&mut self, tipo: &str, ts: &str, campos: Value) -> &mut Self {
        self.seq += 1;
        let mut linha = json!({"v": 1, "seq": self.seq, "ts": ts, "type": tipo});
        if let (Some(base), Value::Object(extra)) = (linha.as_object_mut(), campos) {
            for (k, v) in extra {
                base.insert(k, v);
            }
        }
        self.cru(&linha.to_string())
    }

    /// Acrescenta uma linha crua (para simular linha corrompida), sem `seq`.
    fn cru(&mut self, texto: &str) -> &mut Self {
        let mut f = OpenOptions::new()
            .append(true)
            .create(true)
            .open(&self.arq)
            .unwrap();
        writeln!(f, "{texto}").unwrap();
        self
    }

    fn registro(
        &mut self,
        id: &str,
        de: &str,
        para: &str,
        assunto: &str,
        corpo: &str,
        ts: &str,
    ) -> &mut Self {
        self.evento(
            "registered",
            ts,
            campos_registro(id, de, para, assunto, corpo, ts),
        )
    }

    fn aceite(&mut self, id: &str, ts: &str) -> &mut Self {
        self.evento("accepted", ts, json!({"id": id, "note": null}))
    }

    fn resultado(&mut self, id: &str, ts: &str) -> &mut Self {
        self.evento("resulted", ts, json!({"id": id, "note": null, "result": "ok"}))
    }

    fn campainha(&mut self, dest: &str, ts: &str) -> &mut Self {
        self.evento("bell_sent", ts, json!({"recipient": dest}))
    }
}

/// Roda o chatd ate sair (limite 10 s). `envs` por cima do ambiente limpo. Saida em arquivos.
fn roda(a: &Amb, args: &[&str], envs: &[(&str, &str)]) -> Saida {
    let n = CONTADOR.fetch_add(1, Ordering::SeqCst);
    let out_p = a.p().join(format!("saida-{n}.out"));
    let err_p = a.p().join(format!("saida-{n}.err"));
    let mut cmd = Command::new(BIN);
    cmd.args(args);
    for v in VARS_LIMPAS {
        cmd.env_remove(v);
    }
    cmd.env("HOME", a.home()).env("XDG_CONFIG_HOME", a.xdg());
    cmd.envs(envs.iter().copied());
    let mut child = cmd
        .stdin(Stdio::null())
        .stdout(fs::File::create(&out_p).unwrap())
        .stderr(fs::File::create(&err_p).unwrap())
        .spawn()
        .expect("executar chatd");
    let fim = Instant::now() + Duration::from_secs(10);
    let code = loop {
        if let Some(st) = child.try_wait().expect("try_wait") {
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
        stdout: fs::read_to_string(&out_p).unwrap_or_default(),
        stderr: fs::read_to_string(&err_p).unwrap_or_default(),
    }
}

/// `chatd stale --state-dir STATE` mais os argumentos extras.
fn stale(a: &Amb, state: &Path, extra: &[&str]) -> Saida {
    let st = s(state).to_string();
    let mut args: Vec<&str> = vec!["stale", "--state-dir", st.as_str()];
    args.extend_from_slice(extra);
    roda(a, &args, &[])
}

/// Linhas `[chat] parado:` de uma saida.
fn paradas(saida: &str) -> Vec<String> {
    saida
        .lines()
        .filter(|l| l.starts_with("[chat] parado:"))
        .map(String::from)
        .collect()
}

#[derive(Debug)]
struct Parado {
    nome: String,
    pendentes: u64,
    mais_antiga_min: u64,
    campainha: u64,
}

/// Numero imediatamente antes de `rotulo` (ex.: "1 pendente(s)" -> 1).
fn numero_antes(t: &str, rotulo: &str) -> Option<u64> {
    let i = t.find(rotulo)?;
    t[..i].split_whitespace().last()?.parse().ok()
}

/// Primeiro numero depois de `rotulo` (ex.: "mais antiga 16 min" -> 16).
fn numero_apos(t: &str, rotulo: &str) -> Option<u64> {
    let i = t.find(rotulo)? + rotulo.len();
    t[i..]
        .chars()
        .skip_while(|c| !c.is_ascii_digit())
        .take_while(|c| c.is_ascii_digit())
        .collect::<String>()
        .parse()
        .ok()
}

/// Le `[chat] parado: NOME N pendente(s), mais antiga M min, campainha K vez(es)`.
/// AMBIGUO: o contrato fixa o texto com N, M e K como marcas; aqui cada numero e lido pelo rotulo
/// vizinho, sem exigir os parenteses nem a virgula.
fn le_parado(l: &str) -> Parado {
    let resto = l
        .strip_prefix("[chat] parado: ")
        .unwrap_or_else(|| panic!("prefixo da linha de parado errado: {l:?}"));
    let (nome, resto) = resto
        .split_once(' ')
        .unwrap_or_else(|| panic!("linha sem NOME: {l:?}"));
    Parado {
        nome: nome.to_string(),
        pendentes: numero_antes(resto, "pendente")
            .unwrap_or_else(|| panic!("N (pendentes) ausente: {l:?}")),
        mais_antiga_min: numero_apos(resto, "mais antiga")
            .unwrap_or_else(|| panic!("M (mais antiga) ausente: {l:?}")),
        campainha: numero_apos(resto, "campainha")
            .unwrap_or_else(|| panic!("K (campainha) ausente: {l:?}")),
    }
}

// =====================================================================
// S1 a S12
// =====================================================================

// ---------- S1: sem pendencia: saida 0, nenhuma linha ----------

#[test]
fn s1_sem_pendencia_sai_0_e_nao_escreve_linha() {
    let a = Amb::novo();
    let st = a.state();
    let mut d = Diario::novo(&st);
    // Registrada e aceita; registrada e resultada. Nada fica pendente.
    d.registro("s1-1", A, C, "assunto", "corpo", &ha_min(30));
    d.aceite("s1-1", &ha_min(29));
    d.registro("s1-2", A, C, "assunto", "corpo", &ha_min(20));
    d.resultado("s1-2", &ha_min(19));
    let r = stale(&a, &st, &[]);
    assert_eq!(r.code, Some(0), "sem pendencia = saida 0: {r:?}");
    assert!(r.stdout.trim().is_empty(), "sem pendencia nao ha linha: {r:?}");
}

#[test]
fn s1b_log_vazio_sai_0() {
    let a = Amb::novo();
    let st = a.state();
    Diario::novo(&st);
    let r = stale(&a, &st, &[]);
    assert_eq!(r.code, Some(0), "log vazio = saida 0: {r:?}");
    assert!(r.stdout.trim().is_empty(), "{r:?}");
}

// ---------- S2: limite de idade (--older-than, padrao 900 s) ----------

#[test]
fn s2_pendente_de_14_min_com_limite_padrao_900_sai_0() {
    let a = Amb::novo();
    let st = a.state();
    let mut d = Diario::novo(&st);
    d.registro("s2-1", A, C, "assunto", "corpo", &ha_min(14));
    let r = stale(&a, &st, &[]);
    assert_eq!(r.code, Some(0), "14 min < 900 s nao esta parado: {r:?}");
    assert!(paradas(&r.stdout).is_empty(), "{r:?}");
}

#[test]
fn s2b_pendente_de_16_min_sai_6_com_linha_nome_n_m_k() {
    let a = Amb::novo();
    let st = a.state();
    let mut d = Diario::novo(&st);
    d.registro("s2-2", A, C, "assunto", "corpo", &ha_min(16));
    let r = stale(&a, &st, &[]);
    assert_eq!(r.code, Some(6), "16 min > 900 s: parado = saida 6: {r:?}");
    let ls = paradas(&r.stdout);
    assert_eq!(ls.len(), 1, "uma linha por destinatario parado: {r:?}");
    let p = le_parado(&ls[0]);
    assert_eq!(p.nome, C, "{p:?}");
    assert_eq!(p.pendentes, 1, "{p:?}");
    // AMBIGUO: M em minutos inteiros, arredondado ou truncado; 16 min reais aceitam 15 a 17.
    assert!((15..=17).contains(&p.mais_antiga_min), "M fora de 15..=17: {p:?}");
    assert_eq!(p.campainha, 0, "{p:?}");
}

#[test]
fn s2c_limite_explicito_altera_o_corte() {
    let a = Amb::novo();
    let st = a.state();
    let mut d = Diario::novo(&st);
    d.registro("s2-3", A, C, "assunto", "corpo", &ha_min(14));
    // 14 min > 600 s: parado com limite menor.
    let r1 = stale(&a, &st, &["--older-than", "600"]);
    assert_eq!(r1.code, Some(6), "14 min > 600 s: {r1:?}");
    // 16 min < 1200 s: nao parado com limite maior.
    let a2 = Amb::novo();
    let st2 = a2.state();
    let mut d2 = Diario::novo(&st2);
    d2.registro("s2-4", A, C, "assunto", "corpo", &ha_min(16));
    let r2 = stale(&a2, &st2, &["--older-than", "1200"]);
    assert_eq!(r2.code, Some(0), "16 min < 1200 s nao esta parado: {r2:?}");
    assert!(paradas(&r2.stdout).is_empty(), "{r2:?}");
}

// ---------- S3: campainhas sem aceite (--bells, padrao 2) ----------

#[test]
fn s3_duas_campainhas_sem_aceite_com_pendente_recente_sai_6() {
    let a = Amb::novo();
    let st = a.state();
    let mut d = Diario::novo(&st);
    d.registro("s3-1", A, C, "assunto", "corpo", &ha_min(2));
    d.campainha(C, &ha_seg(90));
    d.campainha(C, &ha_seg(60));
    let r = stale(&a, &st, &[]);
    assert_eq!(r.code, Some(6), "2 campainhas sem aceite (padrao 2) = parado: {r:?}");
    let ls = paradas(&r.stdout);
    assert_eq!(ls.len(), 1, "{r:?}");
    let p = le_parado(&ls[0]);
    assert_eq!(p.nome, C, "{p:?}");
    assert_eq!(p.pendentes, 1, "{p:?}");
    assert_eq!(p.campainha, 2, "K deve ser 2: {p:?}");
}

#[test]
fn s3b_uma_campainha_sem_aceite_nao_basta_com_limite_padrao() {
    let a = Amb::novo();
    let st = a.state();
    let mut d = Diario::novo(&st);
    d.registro("s3-2", A, C, "assunto", "corpo", &ha_min(2));
    d.campainha(C, &ha_seg(60));
    let r = stale(&a, &st, &[]);
    assert_eq!(r.code, Some(0), "1 campainha < 2 e idade 2 min: {r:?}");
    assert!(paradas(&r.stdout).is_empty(), "{r:?}");
}

#[test]
fn s3c_limite_explicito_de_campainhas_3_com_2_sai_0() {
    let a = Amb::novo();
    let st = a.state();
    let mut d = Diario::novo(&st);
    d.registro("s3-3", A, C, "assunto", "corpo", &ha_min(2));
    d.campainha(C, &ha_seg(90));
    d.campainha(C, &ha_seg(60));
    let r = stale(&a, &st, &["--bells", "3"]);
    assert_eq!(r.code, Some(0), "2 campainhas < --bells 3: {r:?}");
    assert!(paradas(&r.stdout).is_empty(), "{r:?}");
}

#[test]
fn s3d_campainhas_antes_do_registro_nao_contam() {
    let a = Amb::novo();
    let st = a.state();
    let mut d = Diario::novo(&st);
    // Duas campainhas para C ANTES de a mensagem existir: nao sao "sem aceite" da mensagem.
    d.campainha(C, &ha_min(10));
    d.campainha(C, &ha_min(9));
    d.registro("s3-4", A, C, "assunto", "corpo", &ha_min(2));
    let r = stale(&a, &st, &[]);
    assert_eq!(r.code, Some(0), "campainhas anteriores ao registro nao contam: {r:?}");
    assert!(paradas(&r.stdout).is_empty(), "{r:?}");
}

#[test]
fn s3e_campainha_de_outro_destinatario_nao_conta_para_c() {
    let a = Amb::novo();
    let st = a.state();
    let mut d = Diario::novo(&st);
    d.registro("s3-5", A, C, "assunto", "corpo", &ha_min(2));
    // Duas campainhas, mas para A (remetente), nao para o destinatario C.
    d.campainha(A, &ha_seg(90));
    d.campainha(A, &ha_seg(60));
    let r = stale(&a, &st, &[]);
    assert_eq!(r.code, Some(0), "campainha de outro destinatario nao conta: {r:?}");
    assert!(paradas(&r.stdout).is_empty(), "{r:?}");
}

// ---------- S4: aceite e resultado depois do registro encerram a pendencia ----------

#[test]
fn s4_accepted_depois_do_registro_nao_conta_como_parado() {
    let a = Amb::novo();
    let st = a.state();
    let mut d = Diario::novo(&st);
    d.registro("s4-1", A, C, "assunto", "corpo", &ha_min(16));
    d.aceite("s4-1", &ha_min(15));
    let r = stale(&a, &st, &[]);
    assert_eq!(r.code, Some(0), "aceita = nao pendente: {r:?}");
    assert!(paradas(&r.stdout).is_empty(), "{r:?}");
}

#[test]
fn s4b_resulted_depois_do_registro_tambem_encerra() {
    let a = Amb::novo();
    let st = a.state();
    let mut d = Diario::novo(&st);
    d.registro("s4-2", A, C, "assunto", "corpo", &ha_min(16));
    d.resultado("s4-2", &ha_min(15));
    let r = stale(&a, &st, &[]);
    assert_eq!(r.code, Some(0), "resultada = nao pendente: {r:?}");
    assert!(paradas(&r.stdout).is_empty(), "{r:?}");
}

#[test]
fn s4c_campainhas_seguidas_de_aceite_nao_deixam_parado() {
    let a = Amb::novo();
    let st = a.state();
    let mut d = Diario::novo(&st);
    d.registro("s4-3", A, C, "assunto", "corpo", &ha_min(16));
    d.campainha(C, &ha_min(10));
    d.campainha(C, &ha_min(9));
    d.aceite("s4-3", &ha_min(5));
    let r = stale(&a, &st, &[]);
    assert_eq!(r.code, Some(0), "aceite depois das campainhas encerra o episodio: {r:?}");
    assert!(paradas(&r.stdout).is_empty(), "{r:?}");
}

#[test]
fn s4d_aceite_de_outra_mensagem_nao_encerra_a_pendente() {
    let a = Amb::novo();
    let st = a.state();
    let mut d = Diario::novo(&st);
    d.registro("s4-4a", A, C, "assunto", "corpo", &ha_min(16));
    d.registro("s4-4b", A, C, "assunto", "corpo", &ha_min(15));
    d.aceite("s4-4b", &ha_min(14));
    let r = stale(&a, &st, &[]);
    assert_eq!(r.code, Some(6), "s4-4a segue pendente de 16 min: {r:?}");
    let ls = paradas(&r.stdout);
    assert_eq!(ls.len(), 1, "{r:?}");
    let p = le_parado(&ls[0]);
    assert_eq!(p.pendentes, 1, "so s4-4a esta pendente: {p:?}");
}

// ---------- S5: um destinatario parado por linha; o outro nao aparece ----------

#[test]
fn s5_so_o_destinatario_parado_aparece() {
    let a = Amb::novo();
    let st = a.state();
    let mut d = Diario::novo(&st);
    d.registro("s5-1", A, C, "assunto", "corpo", &ha_min(16));
    d.registro("s5-2", C, A, "assunto", "corpo", &ha_min(2));
    let r = stale(&a, &st, &[]);
    assert_eq!(r.code, Some(6), "{r:?}");
    let ls = paradas(&r.stdout);
    assert_eq!(ls.len(), 1, "so o terceiro esta parado: {r:?}");
    assert_eq!(le_parado(&ls[0]).nome, C, "{r:?}");
    assert!(!r.stdout.contains(&format!("parado: {A} ")), "{r:?}");
}

#[test]
fn s5b_dois_destinatarios_parados_geram_duas_linhas() {
    let a = Amb::novo();
    let st = a.state();
    let mut d = Diario::novo(&st);
    d.registro("s5-3", A, C, "assunto", "corpo", &ha_min(16));
    d.registro("s5-4", C, A, "assunto", "corpo", &ha_min(17));
    let r = stale(&a, &st, &[]);
    assert_eq!(r.code, Some(6), "{r:?}");
    let ls = paradas(&r.stdout);
    assert_eq!(ls.len(), 2, "uma linha por destinatario parado: {r:?}");
    let nomes: Vec<String> = ls.iter().map(|l| le_parado(l).nome).collect();
    assert!(nomes.contains(&C.to_string()) && nomes.contains(&A.to_string()), "{nomes:?}");
}

// ---------- S6: assunto e corpo nunca saem (stdout, stderr, stamp-file, --json, sink de arquivo) ----------

#[test]
fn s6_assunto_e_corpo_nunca_aparecem_em_stdout_stderr_nem_stamp() {
    let a = Amb::novo();
    let st = a.state();
    let mut d = Diario::novo(&st);
    d.registro("s6-1", A, C, MARCA_ASSUNTO, MARCA_CORPO, &ha_min(16));
    // Sanidade: as marcas estao no log; sem isso a ausencia na saida nao provaria nada.
    let log = fs::read_to_string(a.state().join("log.jsonl")).unwrap();
    assert!(log.contains(MARCA_ASSUNTO) && log.contains(MARCA_CORPO), "marcas fora do log");
    let stamp = a.stamp();
    let sp = s(&stamp).to_string();
    let r = stale(&a, &st, &["--stamp-file", sp.as_str()]);
    assert_eq!(r.code, Some(6), "a mensagem esta parada: {r:?}");
    assert!(!paradas(&r.stdout).is_empty(), "a execucao precisa ter produzido a linha: {r:?}");
    let sf = fs::read_to_string(&stamp).expect("o stamp-file deve ser criado pelo verificador");
    assert!(!r.stdout.contains(MARCA_ASSUNTO) && !r.stdout.contains(MARCA_CORPO), "vazou em stdout: {r:?}");
    assert!(!r.stderr.contains(MARCA_ASSUNTO) && !r.stderr.contains(MARCA_CORPO), "vazou em stderr: {r:?}");
    assert!(!sf.contains(MARCA_ASSUNTO) && !sf.contains(MARCA_CORPO), "vazou no stamp-file: {sf}");
}

#[test]
fn s6b_assunto_e_corpo_nao_aparecem_com_json() {
    let a = Amb::novo();
    let st = a.state();
    let mut d = Diario::novo(&st);
    d.registro("s6-2", A, C, MARCA_ASSUNTO, MARCA_CORPO, &ha_min(16));
    let r = stale(&a, &st, &["--json"]);
    assert_eq!(r.code, Some(6), "{r:?}");
    assert!(!r.stdout.contains(MARCA_ASSUNTO) && !r.stdout.contains(MARCA_CORPO), "vazou com --json: {r:?}");
    assert!(!r.stderr.contains(MARCA_ASSUNTO) && !r.stderr.contains(MARCA_CORPO), "{r:?}");
}

#[test]
fn s6c_assunto_e_corpo_nao_aparecem_no_sink_de_arquivo() {
    let a = Amb::novo();
    let st = a.state();
    let mut d = Diario::novo(&st);
    d.registro("s6-3", A, C, MARCA_ASSUNTO, MARCA_CORPO, &ha_min(16));
    let alertas = a.p().join("alertas.log");
    let notify = format!("file:{}", s(&alertas));
    let r = stale(&a, &st, &["--notify", notify.as_str()]);
    assert_eq!(r.code, Some(6), "{r:?}");
    let conteudo = fs::read_to_string(&alertas).expect("o sink file deve receber a linha");
    assert!(!conteudo.trim().is_empty(), "linha de alerta ausente no arquivo");
    assert!(!conteudo.contains(MARCA_ASSUNTO) && !conteudo.contains(MARCA_CORPO), "vazou no arquivo: {conteudo}");
}

// ---------- S7: um aviso por episodio; episodio novo avisa de novo ----------

#[test]
fn s7_mesmo_episodio_nao_repete_e_episodio_novo_avisa() {
    let a = Amb::novo();
    let st = a.state();
    let mut d = Diario::novo(&st);
    d.registro("s7-1", A, C, "assunto", "corpo", &ha_min(40));
    let stamp = a.stamp();
    let sp = s(&stamp).to_string();
    let extra = ["--stamp-file", sp.as_str()];

    // Primeira execucao do episodio: avisa.
    let r1 = stale(&a, &st, &extra);
    assert_eq!(r1.code, Some(6), "{r1:?}");
    assert_eq!(paradas(&r1.stdout).len(), 1, "{r1:?}");

    // Segunda execucao, mesmo episodio (a mensagem segue pendente): nao repete a linha.
    // AMBIGUO: o codigo desta execucao deduplicada nao e fixado (0 ou 6); o que se exige e nao repetir.
    let r2 = stale(&a, &st, &extra);
    assert!(matches!(r2.code, Some(0) | Some(6)), "{r2:?}");
    assert!(paradas(&r2.stdout).is_empty(), "o mesmo episodio nao pode repetir o aviso: {r2:?}");

    // Fim do episodio: a mensagem e aceita e nada mais fica pendente.
    d.aceite("s7-1", &ha_min(30));
    let r3 = stale(&a, &st, &extra);
    assert_eq!(r3.code, Some(0), "sem pendencia o episodio acaba: {r3:?}");
    assert!(paradas(&r3.stdout).is_empty(), "{r3:?}");

    // Episodio novo: nova mensagem pendente ha 16 min. Avisa de novo.
    d.registro("s7-2", A, C, "assunto", "corpo", &ha_min(16));
    let r4 = stale(&a, &st, &extra);
    assert_eq!(r4.code, Some(6), "episodio novo deve avisar: {r4:?}");
    assert_eq!(paradas(&r4.stdout).len(), 1, "{r4:?}");
}

// ---------- S8: linha corrompida conta em skipped e nao derruba a verificacao ----------

#[test]
fn s8_linha_corrompida_conta_em_skipped_e_o_resto_e_avaliado() {
    let a = Amb::novo();
    let st = a.state();
    let mut d = Diario::novo(&st);
    d.registro("s8-1", A, C, "assunto", "corpo", &ha_min(16));
    d.cru("{\"v\":1,\"seq\":2,\"ts\":");
    let r = stale(&a, &st, &[]);
    assert_eq!(r.code, Some(6), "a linha valida continua valendo: {r:?}");
    assert_eq!(paradas(&r.stdout).len(), 1, "{r:?}");
    assert!(r.stderr.contains("skipped"), "contagem skipped ausente em stderr: {r:?}");
    // AMBIGUO: o contrato so diz `skipped` em stderr; o numero e lido logo apos o rotulo.
    assert_eq!(numero_apos(&r.stderr, "skipped"), Some(1), "skipped deve ser 1: {r:?}");
}

#[test]
fn s8b_campo_desconhecido_no_log_e_ignorado() {
    let a = Amb::novo();
    let st = a.state();
    let mut d = Diario::novo(&st);
    let mut campos = campos_registro("s8-2", A, C, "assunto", "corpo", &ha_min(16));
    campos["campo_do_futuro"] = json!({"x": [1, 2]});
    d.evento("registered", &ha_min(16), campos);
    let r = stale(&a, &st, &[]);
    assert_eq!(r.code, Some(6), "campo desconhecido nao e erro: {r:?}");
    assert_eq!(paradas(&r.stdout).len(), 1, "{r:?}");
    let skipped = if r.stderr.contains("skipped") {
        numero_apos(&r.stderr, "skipped").unwrap_or(0)
    } else {
        0
    };
    assert_eq!(skipped, 0, "campo desconhecido nao conta como linha invalida: {r:?}");
}

#[test]
fn s8c_conteudo_de_linha_corrompida_nao_ecoa() {
    // Variante de seguranca: a linha invalida traz assunto e corpo; eles nao podem sair em nenhuma saida.
    let a = Amb::novo();
    let st = a.state();
    let mut d = Diario::novo(&st);
    d.registro("s8-3", A, C, "assunto", "corpo", &ha_min(16));
    d.cru(&format!(
        "{{\"type\":\"registered\",\"subject\":\"{MARCA_ASSUNTO}\",\"body\":\"{MARCA_CORPO}\""
    ));
    let r = stale(&a, &st, &[]);
    assert_eq!(r.code, Some(6), "{r:?}");
    assert!(!r.stdout.contains(MARCA_ASSUNTO) && !r.stdout.contains(MARCA_CORPO), "{r:?}");
    assert!(!r.stderr.contains(MARCA_ASSUNTO) && !r.stderr.contains(MARCA_CORPO), "{r:?}");
}

// ---------- S9: sink tmux e reservado: saida 2, sem executar nada ----------

#[test]
fn s9_notify_tmux_sai_2_e_nao_executa_tmux() {
    let a = Amb::novo();
    let st = a.state();
    let mut d = Diario::novo(&st);
    d.registro("s9-1", A, C, "assunto", "corpo", &ha_min(16));
    // tmux falso no PATH: se o verificador o executar, cria a marca.
    let bin = a.p().join("bin");
    fs::create_dir_all(&bin).unwrap();
    let marca = a.p().join("tmux-foi-chamado");
    let script = format!("#!/bin/sh\n: > '{}'\n", s(&marca));
    let falso = escreve(&bin.join("tmux"), &script, 0o755);
    assert!(falso.exists());
    let path = bin.display().to_string();
    let st_s = s(&st).to_string();
    // Controle: sem o sink tmux o mesmo estado e verificado com saida 6. Sem isso, saida 2 poderia ser so erro de uso.
    let ctl = roda(&a, &["stale", "--state-dir", st_s.as_str()], &[]);
    assert_eq!(ctl.code, Some(6), "controle sem --notify tmux deve dar 6: {ctl:?}");
    let r = roda(
        &a,
        &["stale", "--state-dir", st_s.as_str(), "--notify", "tmux"],
        &[("PATH", path.as_str())],
    );
    assert_eq!(r.code, Some(2), "sink tmux reservado = saida 2: {r:?}");
    assert!(!marca.exists(), "o verificador executou tmux");
}

#[test]
fn s9b_sink_fora_da_lista_sai_2() {
    // O contrato enumera os sinks (stdout e file:CAMINHO). Qualquer outro nome e erro de uso.
    let a = Amb::novo();
    let st = a.state();
    let mut d = Diario::novo(&st);
    d.registro("s9-2", A, C, "assunto", "corpo", &ha_min(16));
    let ctl = stale(&a, &st, &[]);
    assert_eq!(ctl.code, Some(6), "controle sem --notify deve dar 6: {ctl:?}");
    let r = stale(&a, &st, &["--notify", "exec:/bin/true"]);
    assert_eq!(r.code, Some(2), "sink fora da lista = saida 2: {r:?}");
}

// ---------- S10: log ausente: saida 4 e nada e criado ----------

#[test]
fn s10_log_ausente_sai_4_e_nada_e_criado() {
    let a = Amb::novo();
    let st = a.p().join("estado-inexistente");
    let stamp = a.stamp();
    let sp = s(&stamp).to_string();
    let r = stale(&a, &st, &["--stamp-file", sp.as_str()]);
    assert_eq!(r.code, Some(4), "log ausente = saida 4: {r:?}");
    assert!(
        !st.exists() || fs::read_dir(&st).unwrap().next().is_none(),
        "o verificador nao pode criar nada no estado"
    );
    assert!(!stamp.exists(), "o stamp-file nao pode ser criado sem log");
}

#[test]
fn s10b_log_ilegivel_sai_4() {
    // Um diretorio no lugar do log.jsonl: a leitura falha (vale tambem para root, que ignora permissao).
    let a = Amb::novo();
    let st = a.state();
    fs::create_dir_all(st.join("log.jsonl")).unwrap();
    let r = stale(&a, &st, &[]);
    assert_eq!(r.code, Some(4), "log ilegivel = saida 4: {r:?}");
}

// ---------- S11: nenhuma conexao ao socket e nenhum token aberto ----------

#[test]
fn s11_nao_conecta_ao_socket_do_daemon() {
    let a = Amb::novo();
    let st = a.state();
    let mut d = Diario::novo(&st);
    d.registro("s11-1", A, C, "assunto", "corpo", &ha_min(16));
    // Daemon falso: registra qualquer conexao que chegue ao socket dado por CHATD_SOCKET.
    let sock = a.p().join("daemon-falso.sock");
    let ouvinte = UnixListener::bind(&sock).expect("ouvinte falso");
    let sock_s = s(&sock).to_string();
    let tok = escreve(&a.token(A), "TOKEN-FALSO-V17-NAO-E-REAL", 0o600);
    let tok_s = s(&tok).to_string();
    let st_s = s(&st).to_string();
    let r = roda(
        &a,
        &["stale", "--state-dir", st_s.as_str()],
        &[
            ("CHATD_SOCKET", sock_s.as_str()),
            ("CHATD_AS", A),
            ("CHATD_TOKEN_FILE", tok_s.as_str()),
        ],
    );
    // A execucao precisa chegar ao fim (senao o teste nao prova nada).
    assert_eq!(r.code, Some(6), "{r:?}");
    ouvinte.set_nonblocking(true).unwrap();
    let conectou = ouvinte.accept().is_ok();
    assert!(!conectou, "o stale conectou ao socket do daemon: {r:?}");
}

#[test]
fn s11b_nao_abre_arquivo_de_token() {
    let a = Amb::novo();
    let st = a.state();
    let mut d = Diario::novo(&st);
    d.registro("s11-2", A, C, "assunto", "corpo", &ha_min(16));
    // FIFOs como token em todos os lugares plausiveis: se o stale abrir qualquer um, bloqueia e o roda estoura.
    let tok_env = a.token(A);
    fifo(&tok_env);
    fifo(&a.home().join(".local/state/chatd/claude-main.token"));
    fifo(&a.home().join(".local/state/chatd/terceiro.token"));
    fifo(&st.join("claude-main.token"));
    fifo(&st.join("terceiro.token"));
    let tok_s = s(&tok_env).to_string();
    let st_s = s(&st).to_string();
    let r = roda(
        &a,
        &["stale", "--state-dir", st_s.as_str()],
        &[
            ("CHATD_SOCKET", "/nonexistent/chatd-stale-v17.sock"),
            ("CHATD_AS", A),
            ("CHATD_TOKEN_FILE", tok_s.as_str()),
        ],
    );
    assert_eq!(
        r.code,
        Some(6),
        "bloqueou ou nao chegou ao fim (um FIFO de token foi aberto?): {r:?}"
    );
}

// ---------- S12: stamp-file criado com 0600 e sem seguir link simbolico ----------

#[test]
fn s12_stamp_criado_com_modo_0600() {
    let a = Amb::novo();
    let st = a.state();
    let mut d = Diario::novo(&st);
    d.registro("s12-1", A, C, "assunto", "corpo", &ha_min(16));
    let stamp = a.stamp();
    let sp = s(&stamp).to_string();
    let r = stale(&a, &st, &["--stamp-file", sp.as_str()]);
    assert_eq!(r.code, Some(6), "{r:?}");
    assert!(stamp.exists(), "stamp-file nao criado");
    assert_eq!(modo_de(&stamp), 0o600, "stamp-file deve ser 0600");
}

#[test]
fn s12b_nao_segue_link_simbolico_no_stamp() {
    let a = Amb::novo();
    let st = a.state();
    let mut d = Diario::novo(&st);
    d.registro("s12-2", A, C, "assunto", "corpo", &ha_min(16));
    let alvo = escreve(&a.p().join("alvo.txt"), "ALVO-INTACTO-V17", 0o600);
    let stamp = a.stamp();
    symlink(&alvo, &stamp).unwrap();
    // Controle: com stamp-file em caminho comum a execucao da 6, para o teste nao passar por erro de uso.
    let ctl_stamp = a.p().join("stamp-controle.json");
    let csp = s(&ctl_stamp).to_string();
    let ctl = stale(&a, &st, &["--stamp-file", csp.as_str()]);
    assert_eq!(ctl.code, Some(6), "controle sem link deve dar 6: {ctl:?}");
    let sp = s(&stamp).to_string();
    let r = stale(&a, &st, &["--stamp-file", sp.as_str()]);
    // AMBIGUO: o codigo de saida nesse caso (recusar ou seguir sem gravar no alvo) nao e fixado.
    assert!(r.code.is_some(), "a execucao nao terminou: {r:?}");
    assert_eq!(
        fs::read_to_string(&alvo).unwrap(),
        "ALVO-INTACTO-V17",
        "o stamp gravou pelo link simbolico no alvo"
    );
}

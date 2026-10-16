//! v1.8, Parte A (docs/spec-v1.8-stale-e-anexos.md): regra 4 (v1.7.1, "aceita sem resultado") e S13 a S22.
//! Escrito so a partir do contrato e dos modelos tests/common e tests/v17_stale.rs; src/ nao foi lido.
//! Caixa preta: so o binario e os arquivos. Cada teste tem diretorio temporario proprio; HOME, XDG_CONFIG_HOME e
//! XDG_STATE_HOME apontam para dentro dele. Variaveis CHATD_*, HERDR_* e TMUX* sao removidas do filho.
//! Nenhum processo real, nenhuma rede, nenhum token real, nenhum acesso a ~/.local/state/chatd nem a ~/.config.
//! O log.jsonl e montado aqui, com `ts` relativos ao relogio atual.
//! Devem falhar ate a implementacao existir (`--result-older-than` ainda nao existe: erro de uso, saida 2).
//! AMBIGUO: marcado onde o contrato nao fixa a base da idade, o texto, o envelope do --json ou o arredondamento.
#![allow(dead_code)]
mod common;

use chrono::{SecondsFormat, TimeDelta, Utc};
use common::*;
use serde_json::{Value, json};
use std::ffi::CString;
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::PermissionsExt;
use std::os::unix::net::UnixListener;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::thread;
use std::time::{Duration, Instant};

const A: &str = "claude-main";
const C: &str = "terceiro";
const B: &str = "codex-web";
/// Marcas unicas (S21): nunca podem aparecer em saida, stderr, stamp-file nem sink de arquivo.
const MARCA_ASSUNTO: &str = "ASSUNTO-SECRETO-V18-S21";
const MARCA_CORPO: &str = "CORPO-SECRETO-V18-S21";
const MARCA_NOTA: &str = "NOTA-SECRETA-V18-S21";

/// Prefixos de variaveis do ambiente que nao podem chegar ao filho.
const PREFIXOS_LIMPOS: &[&str] = &["CHATD", "HERDR", "TMUX"];

static CONTADOR: AtomicUsize = AtomicUsize::new(0);

// ---------- infraestrutura ----------

/// Diretorio temporario com `home/`, `xdg/` e `xdg-state/` (HOME, XDG_CONFIG_HOME, XDG_STATE_HOME).
struct Amb {
    d: tempfile::TempDir,
}

impl Amb {
    fn novo() -> Amb {
        let d = tmp();
        fs::create_dir_all(d.path().join("home")).unwrap();
        fs::create_dir_all(d.path().join("xdg")).unwrap();
        fs::create_dir_all(d.path().join("xdg-state")).unwrap();
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
    fn xdg_state(&self) -> PathBuf {
        self.p().join("xdg-state")
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

/// Campos de um evento `registered` (o log os traz no nivel de cima da linha). `kind` e trocado pelo chamador.
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

    /// Registro de mensagem do tipo `kind` (AVISO, PEDIDO, PERGUNTA, RESPOSTA, ACK).
    fn registro(
        &mut self,
        kind: &str,
        id: &str,
        de: &str,
        para: &str,
        assunto: &str,
        corpo: &str,
        ts: &str,
    ) -> &mut Self {
        let mut campos = campos_registro(id, de, para, assunto, corpo, ts);
        campos["kind"] = json!(kind);
        self.evento("registered", ts, campos)
    }

    fn aceite(&mut self, id: &str, ts: &str) -> &mut Self {
        self.evento("accepted", ts, json!({"id": id, "note": null}))
    }

    fn aceite_com_nota(&mut self, id: &str, ts: &str, nota: &str) -> &mut Self {
        self.evento("accepted", ts, json!({"id": id, "note": nota}))
    }

    fn resultado(&mut self, id: &str, ts: &str) -> &mut Self {
        self.resultado_com(id, ts, "ok")
    }

    fn resultado_com(&mut self, id: &str, ts: &str, result: &str) -> &mut Self {
        self.evento(
            "resulted",
            ts,
            json!({"id": id, "note": null, "result": result}),
        )
    }
}

/// Roda o chatd ate sair (limite 10 s). `envs` por cima do ambiente limpo. Saida em arquivos.
fn roda(a: &Amb, args: &[&str], envs: &[(&str, &str)]) -> Saida {
    let n = CONTADOR.fetch_add(1, Ordering::SeqCst);
    let out_p = a.p().join(format!("saida-{n}.out"));
    let err_p = a.p().join(format!("saida-{n}.err"));
    let mut cmd = Command::new(BIN);
    cmd.args(args);
    for (k, _) in std::env::vars_os() {
        if let Some(ks) = k.to_str() {
            if PREFIXOS_LIMPOS.iter().any(|p| ks.starts_with(p)) {
                cmd.env_remove(&k);
            }
        }
    }
    cmd.env("HOME", a.home())
        .env("XDG_CONFIG_HOME", a.xdg())
        .env("XDG_STATE_HOME", a.xdg_state());
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

/// Todas as linhas `[chat]` de uma saida.
fn chat_linhas(saida: &str) -> Vec<String> {
    saida
        .lines()
        .filter(|l| l.starts_with("[chat]"))
        .map(String::from)
        .collect()
}

/// Linhas `[chat] parado:` de uma saida.
fn paradas(saida: &str) -> Vec<String> {
    saida
        .lines()
        .filter(|l| l.starts_with("[chat] parado:"))
        .map(String::from)
        .collect()
}

/// Linhas `[chat] sem resultado:` de uma saida.
fn sem_resultado(saida: &str) -> Vec<String> {
    saida
        .lines()
        .filter(|l| l.starts_with("[chat] sem resultado:"))
        .map(String::from)
        .collect()
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

#[derive(Debug)]
struct Parado {
    nome: String,
    pendentes: u64,
    mais_antiga_min: u64,
    campainha: u64,
}

/// Le `[chat] parado: NOME N pendente(s), mais antiga M min, campainha K vez(es)`.
/// AMBIGUO: o contrato fixa o texto com N, M e K como marcas; cada numero e lido pelo rotulo vizinho.
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

#[derive(Debug)]
struct SemRes {
    nome: String,
    pedidos: u64,
    mais_antigo_min: u64,
}

/// Le `[chat] sem resultado: NOME N pedido(s), mais antigo M min`.
/// AMBIGUO: o contrato fixa o texto com N e M como marcas; cada numero e lido pelo rotulo vizinho.
fn le_sem_resultado(l: &str) -> SemRes {
    let resto = l
        .strip_prefix("[chat] sem resultado: ")
        .unwrap_or_else(|| panic!("prefixo da linha de sem resultado errado: {l:?}"));
    let (nome, resto) = resto
        .split_once(' ')
        .unwrap_or_else(|| panic!("linha sem NOME: {l:?}"));
    SemRes {
        nome: nome.to_string(),
        pedidos: numero_antes(resto, "pedido")
            .unwrap_or_else(|| panic!("N (pedidos) ausente: {l:?}")),
        mais_antigo_min: numero_apos(resto, "mais antigo")
            .unwrap_or_else(|| panic!("M (mais antigo) ausente: {l:?}")),
    }
}

/// Objetos do `--json`. AMBIGUO: o contrato nao fixa o envelope (array, ou um objeto por linha).
fn objetos_json(saida: &str) -> Vec<Value> {
    let t = saida.trim();
    if let Ok(v) = serde_json::from_str::<Value>(t) {
        return match v {
            Value::Array(a) => a,
            outro => vec![outro],
        };
    }
    t.lines()
        .filter(|l| !l.trim().is_empty())
        .map(|l| {
            serde_json::from_str::<Value>(l)
                .unwrap_or_else(|e| panic!("linha do --json nao e JSON ({e}): {l:?}"))
        })
        .collect()
}

/// Campo que diz o tipo do alerta. AMBIGUO: a spec diz "`kind` ou `type` de alerta"; aceita-se qualquer um.
fn campo_tipo(o: &Value) -> Option<String> {
    o.get("kind")
        .or_else(|| o.get("type"))
        .and_then(|v| v.as_str())
        .map(String::from)
}

/// Verdadeiro se alguma marca sentinela aparece no texto.
fn tem_marca(t: &str) -> bool {
    [MARCA_ASSUNTO, MARCA_CORPO, MARCA_NOTA]
        .iter()
        .any(|m| t.contains(m))
}

/// Log com marcas em assunto, corpo e nota/resultado. s21-1 e PEDIDO aceito ha 61 min sem resultado (alerta);
/// s21-2 e PEDIDO aceito e resultado, sem alerta. Usado pelos testes S21.
fn log_com_marcas(st: &Path) {
    let mut d = Diario::novo(st);
    d.registro(
        "PEDIDO",
        "s21-1",
        A,
        C,
        MARCA_ASSUNTO,
        MARCA_CORPO,
        &ha_min(61),
    );
    d.aceite_com_nota("s21-1", &ha_min(61), MARCA_NOTA);
    d.registro(
        "PEDIDO",
        "s21-2",
        A,
        C,
        MARCA_ASSUNTO,
        MARCA_CORPO,
        &ha_min(61),
    );
    d.aceite("s21-2", &ha_min(61));
    d.resultado_com("s21-2", &ha_min(61), MARCA_NOTA);
}

// =====================================================================
// Regra 4 (v1.7.1): aceita sem resultado. S13 a S18.
// =====================================================================

// ---------- S13: PEDIDO aceito sem resultado; limite padrao --result-older-than 3600 s ----------

#[test]
fn s13a_pedido_aceito_ha_59_min_sem_resultado_nao_avisa() {
    let a = Amb::novo();
    let st = a.state();
    let mut d = Diario::novo(&st);
    d.registro("PEDIDO", "s13-1", A, C, "assunto", "corpo", &ha_min(59));
    d.aceite("s13-1", &ha_min(59));
    let r = stale(&a, &st, &[]);
    assert_eq!(r.code, Some(0), "59 min < 3600 s nao esta parado: {r:?}");
    assert!(
        r.stdout.trim().is_empty(),
        "nada a avisar, nenhuma linha: {r:?}"
    );
}

#[test]
fn s13b_pedido_aceito_ha_61_min_sem_resultado_sai_6_com_linha() {
    let a = Amb::novo();
    let st = a.state();
    let mut d = Diario::novo(&st);
    d.registro("PEDIDO", "s13-2", A, C, "assunto", "corpo", &ha_min(61));
    d.aceite("s13-2", &ha_min(61));
    let r = stale(&a, &st, &[]);
    assert_eq!(
        r.code,
        Some(6),
        "61 min > 3600 s: parado por resultado = saida 6: {r:?}"
    );
    let ls = sem_resultado(&r.stdout);
    assert_eq!(
        ls.len(),
        1,
        "uma linha `sem resultado` por destinatario e tipo: {r:?}"
    );
    let p = le_sem_resultado(&ls[0]);
    assert_eq!(p.nome, C, "{p:?}");
    assert_eq!(p.pedidos, 1, "{p:?}");
    // AMBIGUO: M em minutos inteiros, arredondado ou truncado; 61 min reais aceitam 60 a 62.
    assert!(
        (60..=62).contains(&p.mais_antigo_min),
        "M fora de 60..=62: {p:?}"
    );
    assert!(
        paradas(&r.stdout).is_empty(),
        "sem pendencia de aceite, nao ha linha `parado`: {r:?}"
    );
}

#[test]
fn s13c_idade_do_resultado_conta_do_aceite_e_nao_do_registro() {
    // AMBIGUO: a regra 4 diz "com accepted ... ha mais de --result-older-than"; a leitura adotada e a idade a partir
    // do `accepted` (a S13 diz "aceito ha 59 min"). Se a implementacao contar do `registered`, este teste falha.
    let a = Amb::novo();
    let st = a.state();
    let mut d = Diario::novo(&st);
    d.registro("PEDIDO", "s13-3", A, C, "assunto", "corpo", &ha_min(180));
    d.aceite("s13-3", &ha_min(59));
    let r = stale(&a, &st, &[]);
    assert_eq!(
        r.code,
        Some(0),
        "aceito ha 59 min: nada, mesmo registrado ha 3 h: {r:?}"
    );
    assert!(r.stdout.trim().is_empty(), "{r:?}");
}

// ---------- S14: AVISO e RESPOSTA nao pedem resultado ----------

#[test]
fn s14a_aviso_e_resposta_aceitos_ha_3_h_sem_resultado_nao_avisam() {
    let a = Amb::novo();
    let st = a.state();
    let mut d = Diario::novo(&st);
    d.registro("AVISO", "s14-1", A, C, "assunto", "corpo", &ha_min(180));
    d.aceite("s14-1", &ha_min(180));
    d.registro("RESPOSTA", "s14-2", A, C, "assunto", "corpo", &ha_min(180));
    d.aceite("s14-2", &ha_min(180));
    let r = stale(&a, &st, &[]);
    assert_eq!(
        r.code,
        Some(0),
        "AVISO e RESPOSTA nao pedem resultado: {r:?}"
    );
    assert!(r.stdout.trim().is_empty(), "nada a avisar: {r:?}");
}

#[test]
fn s14b_aviso_e_resposta_nao_entram_na_contagem_de_pedidos() {
    // Controle: o mesmo log com um PEDIDO aceito ha 61 min. So o PEDIDO conta: N = 1.
    let a = Amb::novo();
    let st = a.state();
    let mut d = Diario::novo(&st);
    d.registro("AVISO", "s14-3", A, C, "assunto", "corpo", &ha_min(180));
    d.aceite("s14-3", &ha_min(180));
    d.registro("RESPOSTA", "s14-4", A, C, "assunto", "corpo", &ha_min(180));
    d.aceite("s14-4", &ha_min(180));
    d.registro("PEDIDO", "s14-5", A, C, "assunto", "corpo", &ha_min(61));
    d.aceite("s14-5", &ha_min(61));
    let r = stale(&a, &st, &[]);
    assert_eq!(r.code, Some(6), "{r:?}");
    let ls = sem_resultado(&r.stdout);
    assert_eq!(ls.len(), 1, "{r:?}");
    assert_eq!(
        le_sem_resultado(&ls[0]).pedidos,
        1,
        "so o PEDIDO conta: {r:?}"
    );
}

// ---------- S15: PERGUNTA conta como PEDIDO ----------

#[test]
fn s15_pergunta_aceita_ha_61_min_sem_resultado_conta_como_pedido() {
    let a = Amb::novo();
    let st = a.state();
    let mut d = Diario::novo(&st);
    d.registro("PERGUNTA", "s15-1", A, C, "assunto", "corpo", &ha_min(61));
    d.aceite("s15-1", &ha_min(61));
    let r = stale(&a, &st, &[]);
    assert_eq!(r.code, Some(6), "PERGUNTA sem resultado ha 61 min: {r:?}");
    let ls = sem_resultado(&r.stdout);
    assert_eq!(ls.len(), 1, "{r:?}");
    let p = le_sem_resultado(&ls[0]);
    assert_eq!(p.nome, C, "{p:?}");
    assert_eq!(p.pedidos, 1, "PERGUNTA entra na contagem N: {p:?}");
    // AMBIGUO: M em minutos inteiros, arredondado ou truncado; 61 min reais aceitam 60 a 62.
    assert!(
        (60..=62).contains(&p.mais_antigo_min),
        "M fora de 60..=62: {p:?}"
    );
}

// ---------- S16: resulted encerra o episodio; nova mensagem reavisa; stamp-file ----------

#[test]
fn s16_resulted_encerra_o_episodio_e_nova_mensagem_reavisa() {
    // Limite de resultado de 2 s para o teste ser curto (ha um sleep de 3 s para a segunda mensagem envelhecer).
    let a = Amb::novo();
    let st = a.state();
    let mut d = Diario::novo(&st);
    d.registro("PEDIDO", "s16-1", A, C, "assunto", "corpo", &ha_seg(12));
    d.aceite("s16-1", &ha_seg(10));
    let stamp = a.stamp();
    let sp = s(&stamp).to_string();
    let extra = ["--result-older-than", "2", "--stamp-file", sp.as_str()];

    // Primeira execucao do episodio: avisa e cria o stamp-file (0600).
    let r1 = stale(&a, &st, &extra);
    assert_eq!(r1.code, Some(6), "{r1:?}");
    let ls = sem_resultado(&r1.stdout);
    assert_eq!(ls.len(), 1, "{r1:?}");
    assert_eq!(le_sem_resultado(&ls[0]).pedidos, 1, "{r1:?}");
    assert!(
        stamp.exists(),
        "stamp-file deve existir apos o primeiro aviso"
    );
    assert_eq!(modo_de(&stamp), 0o600, "stamp-file deve ser 0600");

    // Mesmo episodio, ainda sem resultado: nao reavisa (linha deduplicada omitida, nada novo, saida 0).
    let r2 = stale(&a, &st, &extra);
    assert_eq!(r2.code, Some(0), "sem aviso novo a saida e 0: {r2:?}");
    assert!(sem_resultado(&r2.stdout).is_empty(), "deduplicado: {r2:?}");

    // O resultado chega: o episodio acaba e nada fica em condicao.
    d.resultado("s16-1", &ha_seg(0));
    let r3 = stale(&a, &st, &extra);
    assert_eq!(r3.code, Some(0), "resulted encerra o episodio: {r3:?}");
    assert!(r3.stdout.trim().is_empty(), "{r3:?}");

    // Nova mensagem, mesmo destinatario, aceita ha 3 s e sem resultado: reavisa, uma vez.
    d.registro("PEDIDO", "s16-2", A, C, "assunto", "corpo", &ha_seg(0));
    d.aceite("s16-2", &ha_seg(0));
    thread::sleep(Duration::from_secs(3));
    let r4 = stale(&a, &st, &extra);
    assert_eq!(r4.code, Some(6), "episodio novo deve reavisar: {r4:?}");
    assert_eq!(sem_resultado(&r4.stdout).len(), 1, "{r4:?}");

    // Repeticao do novo episodio: deduplicada.
    let r5 = stale(&a, &st, &extra);
    assert_eq!(r5.code, Some(0), "{r5:?}");
    assert!(sem_resultado(&r5.stdout).is_empty(), "{r5:?}");
}

// ---------- S17: os dois tipos no mesmo destinatario geram duas linhas; dedup independente ----------

#[test]
fn s17_dois_tipos_no_mesmo_destinatario_geram_duas_linhas_com_dedup_independente() {
    let a = Amb::novo();
    let st = a.state();
    let mut d = Diario::novo(&st);
    // Tipo aceite: AVISO pendente ha 16 min (sem aceite).
    d.registro("AVISO", "s17-1", A, C, "assunto", "corpo", &ha_min(16));
    // Tipo resultado: PEDIDO aceito ha 61 min sem resultado.
    d.registro("PEDIDO", "s17-2", A, C, "assunto", "corpo", &ha_min(61));
    d.aceite("s17-2", &ha_min(61));
    let stamp = a.stamp();
    let sp = s(&stamp).to_string();
    let extra = ["--stamp-file", sp.as_str()];

    let r1 = stale(&a, &st, &extra);
    assert_eq!(r1.code, Some(6), "{r1:?}");
    assert_eq!(
        chat_linhas(&r1.stdout).len(),
        2,
        "duas linhas, uma por tipo: {r1:?}"
    );
    let parados_1 = paradas(&r1.stdout);
    let sem_1 = sem_resultado(&r1.stdout);
    assert_eq!(parados_1.len(), 1, "{r1:?}");
    assert_eq!(sem_1.len(), 1, "{r1:?}");
    assert_eq!(le_parado(&parados_1[0]).nome, C, "{r1:?}");
    assert_eq!(le_sem_resultado(&sem_1[0]).nome, C, "{r1:?}");

    // Repeticao: os dois tipos deduplicados.
    let r2 = stale(&a, &st, &extra);
    assert_eq!(r2.code, Some(0), "nada novo: {r2:?}");
    assert!(chat_linhas(&r2.stdout).is_empty(), "{r2:?}");

    // Aceite da pendente: acaba o episodio de aceite; o de resultado segue deduplicado (nao volta a avisar).
    d.aceite("s17-1", &ha_seg(0));
    let r3 = stale(&a, &st, &extra);
    assert_eq!(r3.code, Some(0), "{r3:?}");
    assert!(chat_linhas(&r3.stdout).is_empty(), "{r3:?}");
}

// ---------- S18: --result-older-than invalido: saida 2 ----------

#[test]
fn s18_result_older_than_invalido_sai_2() {
    let a = Amb::novo();
    let st = a.state();
    let mut d = Diario::novo(&st);
    d.registro("PEDIDO", "s18-1", A, C, "assunto", "corpo", &ha_min(61));
    d.aceite("s18-1", &ha_min(61));
    // Controle: com um valor valido o mesmo estado da 6. Sem isso, a saida 2 poderia ser so erro de uso geral.
    let ctl = stale(&a, &st, &["--result-older-than", "3600"]);
    assert_eq!(
        ctl.code,
        Some(6),
        "controle com valor valido deve dar 6: {ctl:?}"
    );
    for v in ["0", "-5", "abc"] {
        let opt = format!("--result-older-than={v}");
        let r = stale(&a, &st, &[opt.as_str()]);
        assert_eq!(
            r.code,
            Some(2),
            "--result-older-than={v} deve sair 2: {r:?}"
        );
    }
}

// =====================================================================
// Saida, deduplicacao e privacidade com o tipo de resultado. S19 a S22.
// =====================================================================

// ---------- S19: --json, um objeto por destinatario e tipo; repeticao com new_alert false ----------

#[test]
fn s19_json_um_objeto_por_destinatario_e_tipo_e_repeticao_sem_alerta_novo() {
    let a = Amb::novo();
    let st = a.state();
    let mut d = Diario::novo(&st);
    d.registro("AVISO", "s19-1", A, C, "assunto", "corpo", &ha_min(16));
    d.registro("PEDIDO", "s19-2", A, C, "assunto", "corpo", &ha_min(61));
    d.aceite("s19-2", &ha_min(61));
    let stamp = a.stamp();
    let sp = s(&stamp).to_string();
    let extra = ["--json", "--stamp-file", sp.as_str()];

    let r1 = stale(&a, &st, &extra);
    assert_eq!(r1.code, Some(6), "{r1:?}");
    let objs = objetos_json(&r1.stdout);
    assert_eq!(
        objs.len(),
        2,
        "um objeto por destinatario e tipo parado: {r1:?}"
    );
    for o in &objs {
        assert_eq!(
            o.get("new_alert"),
            Some(&json!(true)),
            "primeiro aviso: new_alert true: {o}"
        );
        assert!(
            o.to_string().contains(&format!("\"{C}\"")),
            "destinatario ausente no objeto: {o}"
        );
    }
    let tipos: Vec<Option<String>> = objs.iter().map(campo_tipo).collect();
    assert!(
        tipos.iter().all(|t| t.is_some()),
        "AMBIGUO: campo de tipo (kind ou type) ausente: {objs:?}"
    );
    assert_ne!(
        tipos[0], tipos[1],
        "os dois objetos devem ter tipos diferentes: {objs:?}"
    );

    // Repeticao do mesmo episodio: os objetos continuam (inclusive os ja avisados), new_alert false, saida 0.
    let r2 = stale(&a, &st, &extra);
    assert_eq!(
        r2.code,
        Some(0),
        "repeticao sem aviso novo = saida 0: {r2:?}"
    );
    let objs2 = objetos_json(&r2.stdout);
    assert_eq!(objs2.len(), 2, "objetos inclusive os ja avisados: {r2:?}");
    assert!(
        objs2
            .iter()
            .all(|o| o.get("new_alert") == Some(&json!(false))),
        "repeticao: new_alert false em todos: {objs2:?}"
    );
}

// ---------- S20: pendente (sem aceite) e aceita sem resultado geram linhas distintas; contagens separadas ----------

#[test]
fn s20_pendente_e_aceita_sem_resultado_geram_linhas_distintas_sem_misturar_contagens() {
    let a = Amb::novo();
    let st = a.state();
    let mut d = Diario::novo(&st);
    // Pendente: sem aceite, ha 16 min.
    d.registro("AVISO", "s20-1", A, C, "assunto", "corpo", &ha_min(16));
    // Aceita sem resultado: PEDIDO aceito ha 61 min. Nao e pendente.
    d.registro("PEDIDO", "s20-2", A, C, "assunto", "corpo", &ha_min(61));
    d.aceite("s20-2", &ha_min(61));
    let r = stale(&a, &st, &[]);
    assert_eq!(r.code, Some(6), "{r:?}");
    assert_eq!(
        chat_linhas(&r.stdout).len(),
        2,
        "duas linhas distintas: {r:?}"
    );
    let parados_l = paradas(&r.stdout);
    let sem_l = sem_resultado(&r.stdout);
    assert_eq!(parados_l.len(), 1, "{r:?}");
    assert_eq!(sem_l.len(), 1, "{r:?}");
    assert_eq!(
        le_parado(&parados_l[0]).pendentes,
        1,
        "o PEDIDO aceito nao conta como pendente: {r:?}"
    );
    assert_eq!(
        le_sem_resultado(&sem_l[0]).pedidos,
        1,
        "a pendente nao conta como pedido: {r:?}"
    );
}

// ---------- S21: marcas em assunto, corpo e nota nunca saem (stdout, stderr, stamp, --json, sink de arquivo) ----------

#[test]
fn s21a_marcas_nao_aparecem_em_stdout_stderr_nem_stamp() {
    let a = Amb::novo();
    let st = a.state();
    log_com_marcas(&st);
    // Sanidade: as marcas estao no log; sem isso a ausencia na saida nao provaria nada.
    let log = fs::read_to_string(st.join("log.jsonl")).unwrap();
    assert!(
        log.contains(MARCA_ASSUNTO) && log.contains(MARCA_CORPO) && log.contains(MARCA_NOTA),
        "marcas fora do log"
    );
    let stamp = a.stamp();
    let sp = s(&stamp).to_string();
    let r = stale(&a, &st, &["--stamp-file", sp.as_str()]);
    assert_eq!(r.code, Some(6), "{r:?}");
    assert_eq!(
        sem_resultado(&r.stdout).len(),
        1,
        "a execucao precisa ter produzido a linha: {r:?}"
    );
    let sf = fs::read_to_string(&stamp).expect("o stamp-file deve ser criado pelo verificador");
    assert!(!tem_marca(&r.stdout), "vazou em stdout: {r:?}");
    assert!(!tem_marca(&r.stderr), "vazou em stderr: {r:?}");
    assert!(!tem_marca(&sf), "vazou no stamp-file: {sf}");
}

#[test]
fn s21b_marcas_nao_aparecem_com_json() {
    let a = Amb::novo();
    let st = a.state();
    log_com_marcas(&st);
    let stamp = a.stamp();
    let sp = s(&stamp).to_string();
    let r = stale(&a, &st, &["--json", "--stamp-file", sp.as_str()]);
    assert_eq!(r.code, Some(6), "{r:?}");
    assert!(
        !objetos_json(&r.stdout).is_empty(),
        "a saida --json precisa ter objetos: {r:?}"
    );
    assert!(!tem_marca(&r.stdout), "vazou com --json: {r:?}");
    assert!(!tem_marca(&r.stderr), "vazou em stderr com --json: {r:?}");
    let sf = fs::read_to_string(&stamp).expect("stamp-file");
    assert!(!tem_marca(&sf), "vazou no stamp-file com --json: {sf}");
}

#[test]
fn s21c_marcas_nao_aparecem_no_sink_de_arquivo() {
    let a = Amb::novo();
    let st = a.state();
    log_com_marcas(&st);
    let alertas = a.p().join("alertas.log");
    let notify = format!("file:{}", s(&alertas));
    let r = stale(&a, &st, &["--notify", notify.as_str()]);
    assert_eq!(r.code, Some(6), "{r:?}");
    let conteudo = fs::read_to_string(&alertas).expect("o sink file deve receber a linha");
    assert!(
        !conteudo.trim().is_empty(),
        "linha de alerta ausente no arquivo"
    );
    assert!(!tem_marca(&conteudo), "vazou no arquivo: {conteudo}");
}

// ---------- S22: nenhuma conexao ao socket e nenhum token aberto no caminho de resultado ----------

#[test]
fn s22_nao_conecta_ao_socket_e_nao_abre_token_com_pendente_e_resultado() {
    let a = Amb::novo();
    let st = a.state();
    let mut d = Diario::novo(&st);
    // Caminho de aceite (pendente) e caminho de resultado (PEDIDO aceito ha 61 min) no mesmo log.
    d.registro("AVISO", "s22-1", A, C, "assunto", "corpo", &ha_min(16));
    d.registro("PEDIDO", "s22-2", A, B, "assunto", "corpo", &ha_min(61));
    d.aceite("s22-2", &ha_min(61));
    // Daemon falso: qualquer conexao que chegue ao socket dado por CHATD_SOCKET fica registrada no listener.
    let sock = a.p().join("daemon-falso.sock");
    let ouvinte = UnixListener::bind(&sock).expect("ouvinte falso");
    let sock_s = s(&sock).to_string();
    // FIFOs como token em todos os lugares plausiveis (inclusive o apontado por CHATD_TOKEN_FILE): se o stale
    // abrir qualquer um deles, bloqueia e o roda estoura (code None).
    let tok_env = a.token(A);
    fifo(&tok_env);
    fifo(&a.home().join(".local/state/chatd/claude-main.token"));
    fifo(&a.home().join(".local/state/chatd/terceiro.token"));
    fifo(&a.xdg_state().join("chatd/claude-main.token"));
    fifo(&st.join("claude-main.token"));
    fifo(&st.join("terceiro.token"));
    let tok_s = s(&tok_env).to_string();
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
    // A execucao precisa chegar ao fim e produzir as duas linhas (senao o teste nao prova nada).
    assert_eq!(
        r.code,
        Some(6),
        "bloqueou ou nao chegou ao fim (um FIFO de token foi aberto?): {r:?}"
    );
    assert_eq!(
        paradas(&r.stdout).len(),
        1,
        "caminho de aceite nao rodou: {r:?}"
    );
    assert_eq!(
        sem_resultado(&r.stdout).len(),
        1,
        "caminho de resultado nao rodou: {r:?}"
    );
    ouvinte.set_nonblocking(true).unwrap();
    assert!(
        ouvinte.accept().is_err(),
        "o stale conectou ao socket do daemon: {r:?}"
    );
}

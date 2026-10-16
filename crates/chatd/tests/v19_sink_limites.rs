//! v1.9, Parte A (docs/spec-v1.9-sink-tmux.md): limites do sink `tmux` do `chatd stale`, L1 a L7 (rodada de mutacao).
//! Escrito a partir da Parte A e dos esclarecimentos da spec, e dos helpers de tests/v19_sink_tmux.rs (copiados:
//! nao ha modulo comum). src/ nao foi lido. Caixa preta: so o binario, o log, os arquivos e um `tmux` FALSO
//! (script Python 3 -I) que registra cada chamada. Cada teste tem diretorio temporario proprio; HOME e XDG
//! apontam para dentro dele. Nenhum tmux real, nenhum agente real, nenhuma rede.
//! Cada teste tem controle positivo: o cenario valido roda antes de provar o limite.
#![allow(dead_code)]
mod common;

use chrono::{SecondsFormat, TimeDelta, Utc};
use common::*;
use serde_json::{Value, json};
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::thread;
use std::time::{Duration, Instant};

/// Prefixos de variaveis removidas do filho: CHATD_* (identidade e config), HERDR_* e TMUX* (multiplexador).
const PREFIXOS_LIMPOS: &[&str] = &["CHATD", "HERDR", "TMUX"];
/// Inicio de todo aviso de falha do sink tmux (spec, regra 4).
const AVISO_SINK: &str = "chatd: aviso: sink tmux:";

static CONTADOR: AtomicUsize = AtomicUsize::new(0);

// ---------- infraestrutura ----------

/// Diretorio temporario com `home/`, `xdg/` e `xdg-state/`.
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
    fn state(&self) -> PathBuf {
        self.p().join("state")
    }
    /// Stamp por episodio: stamp novo = aviso novo; mesmo stamp = mesmo episodio.
    fn stamp(&self, nome: &str) -> PathBuf {
        self.p().join(format!("stamp-{nome}.json"))
    }
}

/// Variaveis do ambiente do filho: CHATD_*, HERDR_* e TMUX* removidas; HOME e XDG no temporario.
fn ambiente(cmd: &mut Command, a: &Amb) {
    for (k, _) in std::env::vars_os() {
        if k.to_str()
            .is_some_and(|ks| PREFIXOS_LIMPOS.iter().any(|p| ks.starts_with(p)))
        {
            cmd.env_remove(&k);
        }
    }
    cmd.env("HOME", a.home())
        .env("XDG_CONFIG_HOME", a.xdg())
        .env("XDG_STATE_HOME", a.xdg_state());
}

/// Roda o chatd ate sair (limite 15 s). Saida em arquivos dentro do temporario.
fn roda(a: &Amb, args: &[&str]) -> Saida {
    let n = CONTADOR.fetch_add(1, Ordering::SeqCst);
    let out_p = a.p().join(format!("saida-{n}.out"));
    let err_p = a.p().join(format!("saida-{n}.err"));
    let mut cmd = Command::new(BIN);
    cmd.args(args);
    ambiente(&mut cmd, a);
    let mut child = cmd
        .stdin(Stdio::null())
        .stdout(fs::File::create(&out_p).unwrap())
        .stderr(fs::File::create(&err_p).unwrap())
        .spawn()
        .expect("executar chatd");
    let fim = Instant::now() + Duration::from_secs(15);
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

/// `chatd stale --state-dir STATE` mais `extra`.
fn stale(a: &Amb, extra: &[&str]) -> Saida {
    let st = s(&a.state()).to_string();
    let mut args: Vec<&str> = vec!["stale", "--state-dir", st.as_str()];
    args.extend_from_slice(extra);
    roda(a, &args)
}

/// Uma execucao com `--notify tmux --tmux-bin FALSO --stamp-file STAMP` mais `extra`.
fn corre(a: &Amb, f: &Falso, stamp: &str, extra: &[&str]) -> Saida {
    let st = a.stamp(stamp);
    let st_s = s(&st).to_string();
    let bin = f.bin_s();
    let mut v: Vec<&str> = vec![
        "--notify",
        "tmux",
        "--tmux-bin",
        bin.as_str(),
        "--stamp-file",
        st_s.as_str(),
    ];
    v.extend_from_slice(extra);
    stale(a, &v)
}

/// Instante `min` minutos antes de agora, RFC 3339 UTC com milissegundos (formato do log).
fn ha_min(min: i64) -> String {
    (Utc::now() - TimeDelta::seconds(min * 60)).to_rfc3339_opts(SecondsFormat::Millis, true)
}

// ---------- o tmux falso ----------

/// Script Python 3 -I que registra cada chamada (argv em JSON, uma por linha) e responde conforme `modo.txt`:
/// `ok` (padrao); `trava_set` (set-option dorme 30 s, uma so chamada); `lento` (cada chamada dorme 4 s e sai 0);
/// `falha_display` (display-message sai 1; as demais saem 0). `list-clients` imprime `clientes.txt`.
const FALSO_PY: &str = r#"import json, os, sys, time
d = os.path.dirname(os.path.abspath(__file__))
args = sys.argv[1:]
with open(os.path.join(d, "calls.jsonl"), "a") as f:
    f.write(json.dumps(args) + "\n")
def le(nome, padrao):
    p = os.path.join(d, nome)
    return open(p).read() if os.path.exists(p) else padrao
modo = le("modo.txt", "ok").strip()
sub = args[2:] if args[:1] == ["-S"] else args
nome = sub[0] if sub else ""
if modo == "trava_set" and nome == "set-option":
    time.sleep(30)
    sys.exit(0)
if modo == "lento":
    time.sleep(4)
if modo == "falha_display" and nome == "display-message":
    sys.exit(1)
if nome == "list-clients":
    data = le("clientes.txt", "")
    try:
        sys.stdout.write(data)
        sys.stdout.flush()
    except BrokenPipeError:
        os._exit(0)
sys.exit(0)
"#;

/// Falso `tmux` de um teste: diretorio proprio com o wrapper, o script e os arquivos de controle.
struct Falso {
    dir: PathBuf,
    bin: PathBuf,
}

impl Falso {
    fn novo(a: &Amb) -> Falso {
        let dir = a
            .p()
            .join(format!("falso-{}", CONTADOR.fetch_add(1, Ordering::SeqCst)));
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("falso.py"), FALSO_PY).unwrap();
        let bin = dir.join("tmux");
        fs::write(
            &bin,
            format!(
                "#!/bin/sh\nexec python3 -I '{}/falso.py' \"$@\"\n",
                dir.display()
            ),
        )
        .unwrap();
        fs::set_permissions(&bin, fs::Permissions::from_mode(0o700)).unwrap();
        Falso { dir, bin }
    }

    fn modo(&self, m: &str) {
        fs::write(self.dir.join("modo.txt"), m).unwrap();
    }

    fn clientes(&self, nomes: &[&str]) {
        fs::write(self.dir.join("clientes.txt"), nomes.join("\n") + "\n").unwrap();
    }

    fn bin_s(&self) -> String {
        s(&self.bin).to_string()
    }

    /// Chamadas registradas, cada uma como o argv (sem o nome do programa).
    fn chamadas(&self) -> Vec<Vec<String>> {
        fs::read_to_string(self.dir.join("calls.jsonl"))
            .unwrap_or_default()
            .lines()
            .map(|l| serde_json::from_str::<Vec<String>>(l).expect("linha do falso"))
            .collect()
    }

    /// Subcomando de cada chamada (pula o `-S P` opcional).
    fn subcomandos(&self) -> Vec<String> {
        self.chamadas()
            .into_iter()
            .map(|c| {
                let k = if c.first().map(String::as_str) == Some("-S") {
                    2
                } else {
                    0
                };
                c.get(k).cloned().unwrap_or_default()
            })
            .collect()
    }

    /// Alvo (`-c CLIENTE`) de cada `display-message`, na ordem das chamadas.
    fn alvos_display(&self) -> Vec<String> {
        let mut v = Vec::new();
        for c in self.chamadas() {
            let k = if c.first().map(String::as_str) == Some("-S") {
                2
            } else {
                0
            };
            if c.get(k).map(String::as_str) != Some("display-message") {
                continue;
            }
            if let Some(i) = c.iter().position(|x| x == "-c") {
                v.push(c[i + 1].clone());
            }
        }
        v
    }
}

fn n_display(f: &Falso) -> usize {
    f.subcomandos()
        .iter()
        .filter(|x| x.as_str() == "display-message")
        .count()
}

// ---------- log de cenario (so os campos que o stale le) ----------

struct Log {
    arq: PathBuf,
    seq: u64,
}

impl Log {
    fn novo(state: &Path) -> Log {
        fs::create_dir_all(state).unwrap();
        let arq = state.join("log.jsonl");
        fs::write(&arq, "").unwrap();
        Log { arq, seq: 0 }
    }

    fn linha(&mut self, tipo: &str, ts: &str, campos: Value) {
        self.seq += 1;
        let mut base = json!({"v": 1, "seq": self.seq, "ts": ts, "type": tipo});
        if let (Some(b), Value::Object(extra)) = (base.as_object_mut(), campos) {
            for (k, v) in extra {
                b.insert(k, v);
            }
        }
        let mut f = OpenOptions::new()
            .append(true)
            .create(true)
            .open(&self.arq)
            .unwrap();
        writeln!(f, "{base}").unwrap();
    }

    /// Acrescenta um `registered` pendente (sem `accepted`) para `para`, com a idade `min` em minutos.
    fn pendente(&mut self, id: &str, para: &str, kind: &str, min: i64) {
        let ts = ha_min(min);
        self.linha(
            "registered",
            &ts,
            json!({
                "id": id, "from": "codex-web", "to": para, "kind": kind,
                "subject": "assunto", "body": "corpo", "thread": null,
                "state": "registered", "refs": [], "note": null,
                "registered_at": ts, "accepted_at": null, "resulted_at": null, "result": null
            }),
        );
    }
}

/// Cenario do K2: `claude` com 7 pendentes, o mais antigo ha 329 min.
fn cenario_claude(a: &Amb) {
    let mut l = Log::novo(&a.state());
    for i in 0..7 {
        l.pendente(format!("l-claude-{i}").as_str(), "claude", "AVISO", 329);
    }
}

/// Um pendente por destinatario `(nome, minutos)`, todos AVISO sem aceite.
fn cenario_grupos(a: &Amb, grupos: &[(&str, i64)]) {
    let mut l = Log::novo(&a.state());
    for (nome, min) in grupos {
        l.pendente(format!("{nome}-0").as_str(), nome, "AVISO", *min);
    }
}

/// Valor do `set-option -g @chatd_stale -- VALOR` do falso (o unico `set-option` da execucao).
fn valor_do_set_option(f: &Falso) -> Option<String> {
    f.chamadas()
        .into_iter()
        .find(|c| c.first().map(String::as_str) == Some("set-option"))
        .map(|c| c.last().cloned().unwrap_or_default())
}

// =====================================================================
// L1: timeout por chamada (5 s)
// =====================================================================

#[test]
fn l1_chamada_que_trava_termina_em_menos_de_8_s_com_aviso_timeout() {
    let a = Amb::novo();
    cenario_claude(&a);
    // Controle: o mesmo cenario com tmux normal sai 6 e nao tem aviso.
    let f_ok = Falso::novo(&a);
    f_ok.clientes(&["/dev/pts/1"]);
    let c = corre(&a, &f_ok, "l1-ctrl", &[]);
    assert_eq!(c.code, Some(6), "controle: {c:?}");
    assert!(!c.stderr.contains(AVISO_SINK), "controle sem aviso: {c:?}");

    // A primeira chamada (set-option) trava por 30 s.
    let f = Falso::novo(&a);
    f.clientes(&["/dev/pts/1"]);
    f.modo("trava_set");
    let t = Instant::now();
    let r = corre(&a, &f, "l1", &[]);
    let el = t.elapsed();
    assert!(
        el < Duration::from_secs(8),
        "menos de 8 s (limite 5 s por chamada): {el:?}"
    );
    assert!(
        r.stderr.contains("chatd: aviso: sink tmux: timeout"),
        "aviso timeout: {r:?}"
    );
    assert_eq!(r.code, Some(6), "codigo de saida inalterado: {r:?}");
    assert_eq!(
        f.subcomandos(),
        vec!["set-option"],
        "timeout pula o resto (esclarecimento 2): {:?}",
        f.chamadas()
    );
}

// =====================================================================
// L2: timeout total (10 s)
// =====================================================================

#[test]
fn l2_tmux_lento_termina_em_menos_de_13_s_com_aviso_timeout() {
    let a = Amb::novo();
    cenario_claude(&a);
    let nomes = ["/dev/pts/1", "/dev/pts/2", "/dev/pts/3"];
    // Controle: sem lentidao, 3 clientes = set-option + list-clients + 3 display-message = 5 chamadas, saida 6.
    let f_ok = Falso::novo(&a);
    f_ok.clientes(&nomes);
    let c = corre(&a, &f_ok, "l2-ctrl", &[]);
    assert_eq!(c.code, Some(6), "controle: {c:?}");
    assert_eq!(
        f_ok.chamadas().len(),
        5,
        "controle: cinco chamadas: {:?}",
        f_ok.chamadas()
    );
    assert!(!c.stderr.contains(AVISO_SINK), "controle sem aviso: {c:?}");

    // Cada chamada dorme 4 s: sem limite total seriam 20 s.
    let f = Falso::novo(&a);
    f.clientes(&nomes);
    f.modo("lento");
    let t = Instant::now();
    let r = corre(&a, &f, "l2", &[]);
    let el = t.elapsed();
    assert!(
        el < Duration::from_secs(13),
        "menos de 13 s (limite total 10 s): {el:?}"
    );
    assert!(
        r.stderr.contains("chatd: aviso: sink tmux: timeout"),
        "aviso timeout: {r:?}"
    );
    assert_eq!(r.code, Some(6), "codigo de saida inalterado: {r:?}");
}

// =====================================================================
// L3: saida do list-clients acima de 64 KiB
// =====================================================================

#[test]
fn l3_list_clients_acima_de_64_kib_avisa_bad_output_sem_display_message() {
    let a = Amb::novo();
    cenario_claude(&a);
    // Controle: saida pequena, um cliente, recebe display-message.
    let f_ok = Falso::novo(&a);
    f_ok.clientes(&["/dev/pts/1"]);
    let c = corre(&a, &f_ok, "l3-ctrl", &[]);
    assert_eq!(c.code, Some(6), "controle: {c:?}");
    assert_eq!(
        n_display(&f_ok),
        1,
        "controle: saida pequena tem display-message: {:?}",
        f_ok.chamadas()
    );

    // 12 000 linhas de 11 bytes = 132 000 bytes, acima de 64 KiB (e de 100 KiB).
    let grande = vec!["/dev/pts/1"; 12_000];
    let f = Falso::novo(&a);
    f.clientes(&grande);
    let r = corre(&a, &f, "l3", &[]);
    assert!(
        r.stderr.contains("chatd: aviso: sink tmux: bad_output"),
        "aviso bad_output: {r:?}"
    );
    assert_eq!(
        n_display(&f),
        0,
        "nenhum display-message com saida acima do limite: {:?}",
        f.subcomandos()
    );
    assert_eq!(r.code, c.code, "codigo de saida inalterado: {r:?}");
}

// =====================================================================
// L4: corte do @chatd_stale em 200 caracteres
// =====================================================================

/// Nome de destinatario de exatamente `n` caracteres (`[a-z0-9_-]`), comecando por `dest-II-`.
fn nome_com_len(i: usize, n: usize) -> String {
    let base = format!("dest-{i:02}-");
    assert!(n >= base.len(), "nome curto demais");
    format!("{base}{}", "x".repeat(n - base.len()))
}

/// Texto esperado: uma entrada `NOME 1p 20m` por nome, separadas por ` | `.
fn entradas(nomes: &[String]) -> String {
    nomes
        .iter()
        .map(|n| format!("{n} 1p 20m"))
        .collect::<Vec<_>>()
        .join(" | ")
}

/// Roda o `stale` com um pendente de 20 min por nome; devolve o valor do `set-option` e a saida.
fn valor_para(nomes: &[String]) -> (Option<String>, Saida) {
    let a = Amb::novo();
    let grupos: Vec<(&str, i64)> = nomes.iter().map(|n| (n.as_str(), 20i64)).collect();
    cenario_grupos(&a, &grupos);
    let f = Falso::novo(&a);
    f.clientes(&[]);
    let r = corre(&a, &f, "l4", &[]);
    (valor_do_set_option(&f), r)
}

#[test]
fn l4_texto_de_200_inteiro_sem_mais_k_e_201_corta_com_mais_k() {
    // Controle: duas entradas curtas saem inteiras, sem ` +K`.
    let curto = vec![nome_com_len(1, 19), nome_com_len(2, 19)];
    let (v, r) = valor_para(&curto);
    assert_eq!(r.code, Some(6), "controle: {r:?}");
    assert_eq!(
        v.as_deref(),
        Some(entradas(&curto).as_str()),
        "controle: duas entradas inteiras"
    );

    // Exatamente 200: 7 entradas de 26 caracteres = 7 x 26 + 6 x 3 (separadores) = 200.
    let duzentos: Vec<String> = (1..=7).map(|i| nome_com_len(i, 19)).collect();
    let texto = entradas(&duzentos);
    assert_eq!(texto.len(), 200, "conta do cenario de 200");
    let (v, r) = valor_para(&duzentos);
    assert_eq!(r.code, Some(6), "{r:?}");
    let v = v.expect("set-option com o valor");
    assert_eq!(v.len(), 200, "200 caracteres: {v}");
    assert_eq!(v, texto, "texto inteiro, sem corte");
    assert!(!v.contains(" +"), "sem ` +K`: {v}");

    // 201: 6 entradas de 31 caracteres = 6 x 31 + 5 x 3 = 201. Corta a ultima: 5 entradas + ` +1`.
    let duzentos_um: Vec<String> = (1..=6).map(|i| nome_com_len(i, 24)).collect();
    assert_eq!(entradas(&duzentos_um).len(), 201, "conta do cenario de 201");
    let (v, r) = valor_para(&duzentos_um);
    assert_eq!(r.code, Some(6), "{r:?}");
    let v = v.expect("set-option com o valor");
    assert!(
        v.len() <= 200,
        "no maximo 200 caracteres: {} ({v})",
        v.len()
    );
    assert!(v.ends_with(" +1"), "termina em ` +K` com K = 1: {v}");
    // AMBIGUO (greedy): o contrato manda cortar em fronteira de entrada; com 200 de limite, cabem 5 entradas.
    let esperado = format!("{} +1", entradas(&duzentos_um[..5]));
    assert_eq!(v, esperado, "cinco entradas inteiras e +1");
}

// =====================================================================
// L5: list-clients so com aviso novo
// =====================================================================

#[test]
fn l5_segunda_execucao_sem_aviso_novo_so_chama_set_option() {
    let a = Amb::novo();
    cenario_claude(&a);
    let f = Falso::novo(&a);
    f.clientes(&["/dev/pts/1"]);
    let r1 = corre(&a, &f, "l5", &[]);
    assert_eq!(r1.code, Some(6), "primeira execucao: {r1:?}");
    assert!(
        f.subcomandos().iter().any(|x| x.as_str() == "list-clients"),
        "primeira execucao com aviso novo chama list-clients: {:?}",
        f.chamadas()
    );
    let depois = f.chamadas().len();
    let r2 = corre(&a, &f, "l5", &[]);
    assert_eq!(r2.code, Some(0), "segunda execucao, mesmo episodio: {r2:?}");
    let novas: Vec<String> = f.subcomandos().into_iter().skip(depois).collect();
    assert_eq!(
        novas,
        vec!["set-option"],
        "sem aviso novo, so set-option (sem list-clients): {:?}",
        f.chamadas()
    );
}

// =====================================================================
// L6: um aviso por motivo
// =====================================================================

#[test]
fn l6_display_message_com_codigo_1_gera_uma_linha_de_aviso_so() {
    let a = Amb::novo();
    cenario_claude(&a);
    let nomes = ["/dev/pts/1", "/dev/pts/2", "/dev/pts/3"];
    // Controle: tmux normal, tres clientes: tres display-message, saida 6, nenhum aviso.
    let f_ok = Falso::novo(&a);
    f_ok.clientes(&nomes);
    let c = corre(&a, &f_ok, "l6-ctrl", &[]);
    assert_eq!(c.code, Some(6), "controle: {c:?}");
    assert_eq!(n_display(&f_ok), 3, "controle: tres display-message");
    assert!(!c.stderr.contains(AVISO_SINK), "controle sem aviso: {c:?}");

    // Os display-message saem com codigo 1.
    let f = Falso::novo(&a);
    f.clientes(&nomes);
    f.modo("falha_display");
    let r = corre(&a, &f, "l6", &[]);
    let linhas: Vec<&str> = r
        .stderr
        .lines()
        .filter(|l| l.contains(AVISO_SINK))
        .collect();
    assert_eq!(
        linhas,
        vec!["chatd: aviso: sink tmux: exit_nonzero"],
        "exatamente uma linha de aviso: {r:?}"
    );
    assert_eq!(
        n_display(&f),
        3,
        "uma falha nao impede as demais chamadas (esclarecimento 2): {:?}",
        f.chamadas()
    );
    assert_eq!(r.code, c.code, "codigo de saida inalterado: {r:?}");
}

// =====================================================================
// L7: tamanho do nome de cliente (64 aceito, 65 ignorado)
// =====================================================================

#[test]
fn l7_nome_de_cliente_de_64_recebe_e_de_65_e_ignorado() {
    let a = Amb::novo();
    cenario_claude(&a);
    // Controle: nome normal recebe display-message.
    let normal = "/dev/pts/1".to_string();
    let f_ok = Falso::novo(&a);
    f_ok.clientes(&[normal.as_str()]);
    let c = corre(&a, &f_ok, "l7-ctrl", &[]);
    assert_eq!(c.code, Some(6), "controle: {c:?}");
    assert_eq!(
        f_ok.alvos_display(),
        vec![normal.clone()],
        "controle: {:?}",
        f_ok.chamadas()
    );

    // 64 caracteres (`/dev/pts/` + 55) e 65 (`/dev/pts/` + 56); ambos com caracteres validos.
    let nome64 = format!("/dev/pts/{}", "a".repeat(55));
    let nome65 = format!("/dev/pts/{}", "a".repeat(56));
    assert_eq!(nome64.len(), 64, "conta do nome de 64");
    assert_eq!(nome65.len(), 65, "conta do nome de 65");
    let f = Falso::novo(&a);
    f.clientes(&[nome64.as_str(), nome65.as_str()]);
    let r = corre(&a, &f, "l7", &[]);
    assert_eq!(r.code, Some(6), "{r:?}");
    assert_eq!(
        f.alvos_display(),
        vec![nome64.clone()],
        "64 recebe display-message; 65 e ignorado: {:?}",
        f.chamadas()
    );
}

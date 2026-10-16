//! v1.9, Parte A (docs/spec-v1.9-sink-tmux.md): sink `tmux` do `chatd stale`, contrato K1 a K14.
//! Escrito so a partir do contrato e dos modelos tests/common, tests/v17_stale.rs e tests/v18_stale_resultado.rs;
//! src/ nao foi lido. Caixa preta: so o binario, o log, os arquivos e um `tmux` FALSO (script Python 3 -I) que
//! registra cada chamada num arquivo JSON Lines. Cada teste tem diretorio temporario proprio; HOME, XDG_CONFIG_HOME
//! e XDG_STATE_HOME apontam para dentro dele; CHATD_*, HERDR_* e TMUX* sao removidas do filho. Nenhum tmux real,
//! nenhum agente real, nenhuma rede, nenhum acesso a ~/.local nem a ~/.config.
//! A Parte B da spec (portao do agentd na campainha tmux) NAO e testada aqui: e proposta.
//! Cada teste negativo tem controle (o mesmo cenario sem o defeito, ou um teste irmao que prova o caminho
//! positivo); onde o controle e tambem o comportamento novo, o teste falha hoje com a mensagem que diz isso.
//! AMBIGUO: marcado onde o contrato nao fixa texto, ordem ou consequencia. Ver tests/V19-TESTES.md.
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
/// Sentinela de assunto e corpo (K7): nunca pode aparecer num argumento do tmux.
const MARCA_K7: &str = "SEGREDO-K7-V19";
/// Linha de texto fixo que o alerta de parado produz (v1.8 regra 6), com a campainha (K vez(es)).
const LINHA_PARADO: &str = "[chat] parado: claude 7 pendente(s), mais antiga 329 min";
/// Subcomandos do tmux permitidos pelo contrato (K12).
const PERMITIDOS: &[&str] = &["set-option", "list-clients", "display-message"];

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
    /// Diretorio de estado de teste (dentro do temporario): onde fica o `log.jsonl`.
    fn state(&self) -> PathBuf {
        self.p().join("state")
    }
    /// Stamp novo por execucao (evita que a deduplicacao de uma rodada mude o codigo de outra).
    fn stamp(&self, nome: &str) -> PathBuf {
        self.p().join(format!("stamp-{nome}.json"))
    }
    fn arquivo(&self, nome: &str) -> PathBuf {
        self.p().join(nome)
    }
}

/// Variaveis do ambiente de qualquer processo do teste: CHATD_*, HERDR_* e TMUX* removidas; HOME e XDG no temporario.
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
    roda_com(a, args, &[])
}

/// Como `roda`, com variaveis extras por cima do ambiente limpo (ex.: PATH com um falso na frente).
fn roda_com(a: &Amb, args: &[&str], envs: &[(&str, &str)]) -> Saida {
    let n = CONTADOR.fetch_add(1, Ordering::SeqCst);
    let out_p = a.p().join(format!("saida-{n}.out"));
    let err_p = a.p().join(format!("saida-{n}.err"));
    let mut cmd = Command::new(BIN);
    cmd.args(args);
    ambiente(&mut cmd, a);
    cmd.envs(envs.iter().copied());
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

/// Instante `min` minutos antes de agora, RFC 3339 UTC com milissegundos (formato do log).
fn ha_min(min: i64) -> String {
    (Utc::now() - TimeDelta::seconds(min * 60)).to_rfc3339_opts(SecondsFormat::Millis, true)
}

// ---------- o tmux falso ----------

/// Script Python 3 -I que registra cada chamada (argv em JSON, uma por linha) e responde conforme `modo.txt`:
/// `ok` (padrao), `falha` (sai 1), `trava` (dorme 30 s). `list-clients` imprime os nomes de `clientes.txt`.
/// O `chatd` chama `tmux` por um wrapper sh, `<dir>/tmux`, que faz `exec python3 -I falso.py`.
const FALSO_PY: &str = r#"import json, os, sys, time
d = os.path.dirname(os.path.abspath(__file__))
args = sys.argv[1:]
with open(os.path.join(d, "calls.jsonl"), "a") as f:
    f.write(json.dumps(args) + "\n")
def le(nome, padrao):
    p = os.path.join(d, nome)
    return open(p).read() if os.path.exists(p) else padrao
modo = le("modo.txt", "ok").strip()
if modo == "falha":
    sys.exit(1)
if modo == "trava":
    time.sleep(30)
    sys.exit(0)
sub = args[2:] if args[:1] == ["-S"] else args
if sub[:1] == ["list-clients"]:
    sys.stdout.write(le("clientes.txt", ""))
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

    /// Chamadas com o subcomando (sem o `-S P` opcional).
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
}

// ---------- log de cenario (so os campos que o stale le) ----------

/// Escreve `state/log.jsonl` com `registered` (e `accepted`, se `aceito`). Cada entrada: (id, de, para, kind,
/// assunto, corpo, minutos atras). Os `registered` carregam `subject` e `body`, como o daemon grava.
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
    #[allow(clippy::too_many_arguments)]
    fn pendente(
        &mut self,
        id: &str,
        de: &str,
        para: &str,
        kind: &str,
        assunto: &str,
        corpo: &str,
        min: i64,
    ) {
        let ts = ha_min(min);
        self.linha(
            "registered",
            &ts,
            json!({
                "id": id, "from": de, "to": para, "kind": kind,
                "subject": assunto, "body": corpo, "thread": null,
                "state": "registered", "refs": [], "note": null,
                "registered_at": ts, "accepted_at": null, "resulted_at": null, "result": null
            }),
        );
    }
}

/// Cenario do K2: `claude` com 7 pendentes, o mais antigo ha 329 min (todos com a mesma idade para o teste).
fn cenario_claude(a: &Amb) {
    let mut l = Log::novo(&a.state());
    for i in 0..7 {
        l.pendente(
            format!("k-claude-{i}").as_str(),
            "codex-web",
            "claude",
            "AVISO",
            "assunto",
            "corpo",
            329,
        );
    }
}

/// Cenario com `(nome, pendentes, minutos)` por destinatario, todos AVISO sem aceite.
fn cenario_varios(a: &Amb, grupos: &[(&str, usize, i64)], assunto: &str, corpo: &str) {
    let mut l = Log::novo(&a.state());
    for (nome, n, min) in grupos {
        for i in 0..*n {
            l.pendente(
                format!("{nome}-{i}").as_str(),
                "codex-web",
                nome,
                "AVISO",
                assunto,
                corpo,
                *min,
            );
        }
    }
}

/// Linhas de alerta `[chat] parado:` da saida de texto.
fn paradas(saida: &str) -> Vec<String> {
    saida
        .lines()
        .filter(|l| l.starts_with("[chat] parado:"))
        .map(String::from)
        .collect()
}

/// Valor do `set-option -g @chatd_stale -- VALOR` do falso (o unico `set-option` da execucao).
fn valor_do_set_option(f: &Falso) -> Option<String> {
    f.chamadas()
        .into_iter()
        .find(|c| c.first().map(String::as_str) == Some("set-option"))
        .map(|c| c.last().cloned().unwrap_or_default())
}

// =====================================================================
// K1-K4: valor do @chatd_stale, display-message por cliente, ordem, mesmo episodio
// =====================================================================

#[test]
fn k01_nada_parado_grava_valor_vazio_sem_display_message_e_sai_0() {
    let a = Amb::novo();
    let mut l = Log::novo(&a.state());
    // Mensagem ja aceita e resolvida: nao ha pendencia.
    l.pendente(
        "k1-ok",
        "codex-web",
        "claude",
        "AVISO",
        "assunto",
        "corpo",
        2,
    );
    let f = Falso::novo(&a);
    let st = a.stamp("k1");
    let r = stale(
        &a,
        &[
            "--notify",
            "tmux",
            "--tmux-bin",
            f.bin_s().as_str(),
            "--stamp-file",
            s(&st),
        ],
    );
    assert_eq!(r.code, Some(0), "sem parado a saida e 0: {r:?}");
    assert_eq!(
        valor_do_set_option(&f).as_deref(),
        Some(""),
        "set-option com valor vazio: {:?}",
        f.chamadas()
    );
    assert!(
        !f.subcomandos().iter().any(|c| c == "display-message"),
        "nenhum display-message sem aviso novo: {:?}",
        f.subcomandos()
    );
}

#[test]
fn k02_um_parado_grava_valor_exato_e_um_display_message_por_cliente() {
    let a = Amb::novo();
    cenario_claude(&a);
    let f = Falso::novo(&a);
    f.clientes(&["/dev/pts/1"]);
    let st = a.stamp("k2");
    let r = stale(
        &a,
        &[
            "--notify",
            "tmux",
            "--tmux-bin",
            f.bin_s().as_str(),
            "--stamp-file",
            s(&st),
        ],
    );
    assert_eq!(r.code, Some(6), "aviso novo: saida 6: {r:?}");
    let linha = paradas(&r.stdout);
    assert_eq!(linha.len(), 1, "uma linha de alerta: {}", r.stdout);
    assert!(
        linha[0].starts_with(LINHA_PARADO),
        "linha de texto fixo: {}",
        linha[0]
    );
    // Valor exato do @chatd_stale: NOME Np Mm.
    assert_eq!(
        valor_do_set_option(&f).as_deref(),
        Some("claude 7p 329m"),
        "{:?}",
        f.chamadas()
    );
    // Ordem: set-option, list-clients, display-message por cliente.
    let c = f.chamadas();
    assert_eq!(
        c[0],
        vec!["set-option", "-g", "@chatd_stale", "--", "claude 7p 329m"],
        "argv do set-option"
    );
    assert_eq!(
        c[1],
        vec!["list-clients", "-F", "#{client_name}"],
        "argv do list-clients"
    );
    assert_eq!(
        c[2],
        vec![
            "display-message",
            "-c",
            "/dev/pts/1",
            "-d",
            "10000",
            "-l",
            "--",
            linha[0].as_str()
        ],
        "um display-message com a linha exata (-l literal, sem expansao)"
    );
    assert_eq!(c.len(), 3, "nenhuma chamada extra: {c:?}");
}

#[test]
fn k03_segunda_execucao_no_mesmo_episodio_regrava_valor_sem_display_message() {
    let a = Amb::novo();
    cenario_claude(&a);
    let f = Falso::novo(&a);
    f.clientes(&["/dev/pts/1"]);
    let st = a.stamp("k3");
    let st_s = s(&st).to_string();
    let bin = f.bin_s();
    let args = [
        "--notify",
        "tmux",
        "--tmux-bin",
        bin.as_str(),
        "--stamp-file",
        st_s.as_str(),
    ];
    let primeira = stale(&a, &args);
    assert_eq!(primeira.code, Some(6), "{primeira:?}");
    let depois_da_primeira = f.chamadas().len();
    let segunda = stale(&a, &args);
    assert_eq!(
        segunda.code,
        Some(0),
        "mesmo episodio: saida 0: {segunda:?}"
    );
    let novas: Vec<Vec<String>> = f.chamadas().into_iter().skip(depois_da_primeira).collect();
    let set: Vec<&Vec<String>> = novas.iter().filter(|c| c[0] == "set-option").collect();
    assert_eq!(
        set.len(),
        1,
        "o @chatd_stale e regravado na segunda execucao: {novas:?}"
    );
    assert_eq!(set[0].last().map(String::as_str), Some("claude 7p 329m"));
    assert!(
        !novas.iter().any(|c| c[0] == "display-message"),
        "o display-message sai uma vez por episodio: {novas:?}"
    );
}

#[test]
fn k04_dois_parados_ordenados_por_nome_separados_por_barra() {
    let a = Amb::novo();
    cenario_varios(
        &a,
        &[("codex", 3, 20), ("claude", 7, 329)],
        "assunto",
        "corpo",
    );
    let f = Falso::novo(&a);
    f.clientes(&[]);
    let st = a.stamp("k4");
    let r = stale(
        &a,
        &[
            "--notify",
            "tmux",
            "--tmux-bin",
            f.bin_s().as_str(),
            "--stamp-file",
            s(&st),
        ],
    );
    assert_eq!(r.code, Some(6), "{r:?}");
    assert_eq!(
        valor_do_set_option(&f).as_deref(),
        Some("claude 7p 329m | codex 3p 20m"),
        "ordem por nome e separador ` | `"
    );
}

// =====================================================================
// K5-K6: limite de 200 caracteres com ` +K`; nomes de cliente validados e no maximo 8
// =====================================================================

#[test]
fn k05_texto_acima_de_200_corta_em_fronteira_e_termina_em_mais_k() {
    let a = Amb::novo();
    let nomes: Vec<String> = (1..=12)
        .map(|i| format!("agente-{i:02}-xxxxxxxxxxxxxxxxxxxx"))
        .collect();
    let grupos: Vec<(&str, usize, i64)> =
        nomes.iter().map(|n| (n.as_str(), 1usize, 20i64)).collect();
    cenario_varios(&a, &grupos, "assunto", "corpo");
    let f = Falso::novo(&a);
    f.clientes(&[]);
    let st = a.stamp("k5");
    let r = stale(
        &a,
        &[
            "--notify",
            "tmux",
            "--tmux-bin",
            f.bin_s().as_str(),
            "--stamp-file",
            s(&st),
        ],
    );
    assert_eq!(r.code, Some(6), "{r:?}");
    let v = valor_do_set_option(&f).expect("set-option com o valor");
    assert!(
        v.len() <= 200,
        "no maximo 200 caracteres: {} ({v})",
        v.len()
    );
    // Cauda ` +K`, com K > 0; as entradas mostradas sao completas e na ordem dos nomes.
    let (corpo, k) = v.rsplit_once(" +").expect("termina em ` +K`");
    let k: usize = k.parse().expect("K numerico");
    assert!(k > 0, "algo foi omitido: {v}");
    let mostradas: Vec<&str> = corpo.split(" | ").collect();
    assert_eq!(
        mostradas.len() + k,
        nomes.len(),
        "mostradas + omitidas = total: {v}"
    );
    for (i, m) in mostradas.iter().enumerate() {
        assert!(
            m.starts_with(&nomes[i]),
            "entrada {i} inteira e na ordem: {m}"
        );
        assert!(m.ends_with(" 1p 20m"), "entrada completa: {m}");
    }
}

#[test]
fn k06a_no_maximo_oito_clientes_com_nomes_validos() {
    let a = Amb::novo();
    cenario_claude(&a);
    let f = Falso::novo(&a);
    let nomes: Vec<String> = (1..=9).map(|i| format!("/dev/pts/{i}")).collect();
    let refs: Vec<&str> = nomes.iter().map(String::as_str).collect();
    f.clientes(&refs);
    let st = a.stamp("k6a");
    let r = stale(
        &a,
        &[
            "--notify",
            "tmux",
            "--tmux-bin",
            f.bin_s().as_str(),
            "--stamp-file",
            s(&st),
        ],
    );
    assert_eq!(r.code, Some(6), "{r:?}");
    let display = f
        .chamadas()
        .into_iter()
        .filter(|c| c[0] == "display-message")
        .count();
    assert_eq!(display, 8, "nove clientes validos = oito display-message");
}

#[test]
fn k06b_nomes_de_cliente_invalidos_sao_ignorados() {
    let a = Amb::novo();
    cenario_claude(&a);
    let f = Falso::novo(&a);
    f.clientes(&["/dev/pts/1", "nao valido;rm -rf", "../../etc", "/dev/pts/2"]);
    let st = a.stamp("k6b");
    let r = stale(
        &a,
        &[
            "--notify",
            "tmux",
            "--tmux-bin",
            f.bin_s().as_str(),
            "--stamp-file",
            s(&st),
        ],
    );
    assert_eq!(r.code, Some(6), "{r:?}");
    let alvos: Vec<String> = f
        .chamadas()
        .into_iter()
        .filter(|c| c[0] == "display-message")
        .map(|c| c[2].clone())
        .collect();
    assert_eq!(
        alvos,
        vec!["/dev/pts/1".to_string(), "/dev/pts/2".to_string()],
        "so os nomes validos"
    );
}

// =====================================================================
// K7: assunto e corpo com `#{`, `#(` e `;` nunca aparecem em argumento algum
// =====================================================================

#[test]
fn k07_assunto_e_corpo_nunca_chegam_a_nenhum_argumento_do_tmux() {
    let a = Amb::novo();
    let assunto = format!("assunto #{{pane_id}} #(rm -rf /) ; {MARCA_K7}");
    let corpo = format!("corpo #(id) ; {MARCA_K7} #{{session}}");
    cenario_varios(&a, &[("claude", 7, 329)], &assunto, &corpo);
    let f = Falso::novo(&a);
    f.clientes(&["/dev/pts/1"]);
    let st = a.stamp("k7");
    let r = stale(
        &a,
        &[
            "--notify",
            "tmux",
            "--tmux-bin",
            f.bin_s().as_str(),
            "--stamp-file",
            s(&st),
        ],
    );
    assert_eq!(
        r.code,
        Some(6),
        "o cenario precisa de aviso novo para o teste valer: {r:?}"
    );
    let chamadas = f.chamadas();
    // Controle: as chamadas existem (sem elas a varredura seria vazia e o teste nao provaria nada).
    assert!(
        chamadas.len() >= 2,
        "o falso deve ter recebido chamadas: {chamadas:?}"
    );
    for c in &chamadas {
        for arg in c {
            assert!(
                !arg.contains("#{") && !arg.contains("#(") && !arg.contains(';'),
                "argumento proibido: {arg}"
            );
            assert!(
                !arg.contains(MARCA_K7),
                "a sentinela vazou para o tmux: {arg}"
            );
        }
    }
}

// =====================================================================
// K8-K10: falha do sink nao muda o codigo nem perde o aviso; sink file: continua gravando
// =====================================================================

/// Controle de K8-K10: o mesmo cenario, so com `file:` (sem o defeito do tmux). Devolve (codigo, linhas do arquivo).
fn controle_so_arquivo(a: &Amb, nome: &str) -> (Option<i32>, String) {
    cenario_claude(a);
    let arq = a.arquivo(format!("ctrl-{nome}.log").as_str());
    let f_s = s(&arq).to_string();
    let st = a.stamp(format!("ctrl-{nome}").as_str());
    let r = stale(
        a,
        &[
            "--notify",
            format!("file:{f_s}").as_str(),
            "--stamp-file",
            s(&st),
        ],
    );
    (r.code, fs::read_to_string(&arq).unwrap_or_default())
}

#[test]
fn k08_tmux_inexistente_avisa_spawn_failed_sem_mudar_o_codigo() {
    let a = Amb::novo();
    let (codigo_controle, _) = controle_so_arquivo(&a, "k8");
    cenario_claude(&a);
    let arq = a.arquivo("alertas-k8.log");
    let f_s = s(&arq).to_string();
    let st = a.stamp("k8");
    let inexistente = a.p().join("nao-existe").join("tmux");
    let inex_s = s(&inexistente).to_string();
    let r = stale(
        &a,
        &[
            "--notify",
            format!("file:{f_s}").as_str(),
            "--notify",
            "tmux",
            "--tmux-bin",
            inex_s.as_str(),
            "--stamp-file",
            s(&st),
        ],
    );
    assert_eq!(
        r.code, codigo_controle,
        "o codigo nao muda por causa do tmux (controle): {r:?}"
    );
    assert!(
        r.stderr.contains("chatd: aviso: sink tmux: spawn_failed"),
        "aviso de spawn_failed em stderr: {r:?}"
    );
    assert!(
        fs::read_to_string(&arq)
            .unwrap_or_default()
            .contains("[chat] parado: claude"),
        "o sink file: gravou o aviso mesmo com o tmux quebrado"
    );
}

#[test]
fn k09_tmux_que_sai_nao_zero_avisa_exit_nonzero_sem_mudar_o_codigo() {
    let a = Amb::novo();
    let (codigo_controle, _) = controle_so_arquivo(&a, "k9");
    cenario_claude(&a);
    let f = Falso::novo(&a);
    f.modo("falha");
    let arq = a.arquivo("alertas-k9.log");
    let f_s = s(&arq).to_string();
    let st = a.stamp("k9");
    let r = stale(
        &a,
        &[
            "--notify",
            format!("file:{f_s}").as_str(),
            "--notify",
            "tmux",
            "--tmux-bin",
            f.bin_s().as_str(),
            "--stamp-file",
            s(&st),
        ],
    );
    assert_eq!(r.code, codigo_controle, "{r:?}");
    assert!(
        r.stderr.contains("chatd: aviso: sink tmux: exit_nonzero"),
        "{r:?}"
    );
    assert!(
        fs::read_to_string(&arq)
            .unwrap_or_default()
            .contains("[chat] parado: claude")
    );
}

#[test]
fn k10_tmux_que_trava_avisa_timeout_e_a_execucao_termina_em_menos_de_12_s() {
    // Tempo: cada chamada tem 5 s e o total 10 s; o falso dorme 30 s. Esperado: uns 5 s de execucao.
    let a = Amb::novo();
    let (codigo_controle, _) = controle_so_arquivo(&a, "k10");
    cenario_claude(&a);
    let f = Falso::novo(&a);
    f.modo("trava");
    let arq = a.arquivo("alertas-k10.log");
    let f_s = s(&arq).to_string();
    let st = a.stamp("k10");
    let t = Instant::now();
    let r = stale(
        &a,
        &[
            "--notify",
            format!("file:{f_s}").as_str(),
            "--notify",
            "tmux",
            "--tmux-bin",
            f.bin_s().as_str(),
            "--stamp-file",
            s(&st),
        ],
    );
    let el = t.elapsed();
    assert!(
        el < Duration::from_secs(12),
        "o teste pode levar ate ~12 s por causa do limite de 10 s total: {el:?}"
    );
    assert_eq!(r.code, codigo_controle, "{r:?}");
    assert!(
        r.stderr.contains("chatd: aviso: sink tmux: timeout"),
        "{r:?}"
    );
    assert!(
        fs::read_to_string(&arq)
            .unwrap_or_default()
            .contains("[chat] parado: claude")
    );
}

// =====================================================================
// K11: sinks combinados; limite de quatro, repeticao e opcoes sem o sink tmux
// =====================================================================

#[test]
fn k11a_file_e_tmux_agem_juntos() {
    let a = Amb::novo();
    cenario_claude(&a);
    let f = Falso::novo(&a);
    f.clientes(&["/dev/pts/1"]);
    let arq = a.arquivo("alertas-k11a.log");
    let f_s = s(&arq).to_string();
    let st = a.stamp("k11a");
    let r = stale(
        &a,
        &[
            "--notify",
            format!("file:{f_s}").as_str(),
            "--notify",
            "tmux",
            "--tmux-bin",
            f.bin_s().as_str(),
            "--stamp-file",
            s(&st),
        ],
    );
    assert_eq!(r.code, Some(6), "{r:?}");
    assert!(
        fs::read_to_string(&arq)
            .unwrap_or_default()
            .contains("[chat] parado: claude"),
        "o file: agiu"
    );
    assert!(
        f.subcomandos().iter().any(|c| c == "set-option"),
        "o tmux agiu: {:?}",
        f.subcomandos()
    );
}

#[test]
fn k11b_quatro_sinks_saem_2() {
    let a = Amb::novo();
    cenario_claude(&a);
    let f = Falso::novo(&a);
    let (x, y) = (
        s(&a.arquivo("x.log")).to_string(),
        s(&a.arquivo("y.log")).to_string(),
    );
    // Controle: tres sinks distintos (stdout e dois file:) sao validos: saida 6, nao 2.
    let st_c = a.stamp("k11b-ctrl");
    let fx = format!("file:{x}");
    let fy = format!("file:{y}");
    let ctrl = stale(
        &a,
        &[
            "--notify",
            fx.as_str(),
            "--notify",
            fy.as_str(),
            "--notify",
            "stdout",
            "--stamp-file",
            s(&st_c),
        ],
    );
    assert_eq!(
        ctrl.code,
        Some(6),
        "controle: tres sinks distintos sao validos: {ctrl:?}"
    );
    cenario_claude(&a);
    let st = a.stamp("k11b");
    let r = stale(
        &a,
        &[
            "--notify",
            format!("file:{x}").as_str(),
            "--notify",
            format!("file:{y}").as_str(),
            "--notify",
            "stdout",
            "--notify",
            "tmux",
            "--tmux-bin",
            f.bin_s().as_str(),
            "--stamp-file",
            s(&st),
        ],
    );
    assert_eq!(r.code, Some(2), "quarto sink = saida 2: {r:?}");
    assert!(
        f.chamadas().is_empty(),
        "nada e chamado quando o uso esta errado"
    );
}

#[test]
fn k11c_sink_repetido_sai_2() {
    let a = Amb::novo();
    cenario_claude(&a);
    let f = Falso::novo(&a);
    // Controle: dois sinks distintos sao validos (saida 6); so a repeticao do mesmo sink e saida 2.
    let (x, y) = (
        s(&a.arquivo("cx.log")).to_string(),
        s(&a.arquivo("cy.log")).to_string(),
    );
    let fx = format!("file:{x}");
    let fy = format!("file:{y}");
    let st_c = a.stamp("k11c-ctrl");
    let ctrl = stale(
        &a,
        &[
            "--notify",
            fx.as_str(),
            "--notify",
            fy.as_str(),
            "--stamp-file",
            s(&st_c),
        ],
    );
    assert_eq!(
        ctrl.code,
        Some(6),
        "controle: dois sinks distintos sao validos: {ctrl:?}"
    );
    let st = a.stamp("k11c");
    let r = stale(
        &a,
        &[
            "--notify",
            "tmux",
            "--notify",
            "tmux",
            "--tmux-bin",
            f.bin_s().as_str(),
            "--stamp-file",
            s(&st),
        ],
    );
    assert_eq!(r.code, Some(2), "sink repetido = saida 2: {r:?}");
    assert!(
        f.chamadas().is_empty(),
        "nada e chamado quando o uso esta errado"
    );
}

#[test]
fn k11d_tmux_bin_sem_o_sink_tmux_sai_2() {
    let a = Amb::novo();
    cenario_claude(&a);
    let f = Falso::novo(&a);
    let arq = a.arquivo("alertas-k11d.log");
    let f_s = s(&arq).to_string();
    // Controle: so `file:` (sem --tmux-bin) sai 6.
    let st_c = a.stamp("k11d-ctrl");
    let fa = format!("file:{f_s}");
    let ctrl = stale(&a, &["--notify", fa.as_str(), "--stamp-file", s(&st_c)]);
    assert_eq!(
        ctrl.code,
        Some(6),
        "controle: file: sozinho sai 6: {ctrl:?}"
    );
    let st = a.stamp("k11d");
    let r = stale(
        &a,
        &[
            "--notify",
            format!("file:{f_s}").as_str(),
            "--tmux-bin",
            f.bin_s().as_str(),
            "--stamp-file",
            s(&st),
        ],
    );
    assert_eq!(
        r.code,
        Some(2),
        "--tmux-bin sem o sink tmux = saida 2: {r:?}"
    );
    assert!(f.chamadas().is_empty(), "nada e chamado");
}

#[test]
fn k11e_tmux_socket_sem_o_sink_tmux_sai_2() {
    let a = Amb::novo();
    cenario_claude(&a);
    let arq = a.arquivo("alertas-k11e.log");
    let f_s = s(&arq).to_string();
    let st = a.stamp("k11e");
    let r = stale(
        &a,
        &[
            "--notify",
            format!("file:{f_s}").as_str(),
            "--tmux-socket",
            "/tmp/nao-usar.sock",
            "--stamp-file",
            s(&st),
        ],
    );
    assert_eq!(
        r.code,
        Some(2),
        "--tmux-socket sem o sink tmux = saida 2: {r:?}"
    );
}

// =====================================================================
// K12: propriedade: o falso so recebe set-option, list-clients e display-message
// =====================================================================

#[test]
fn k12_so_os_tres_subcomandos_permitidos_sao_chamados() {
    let a = Amb::novo();
    cenario_claude(&a);
    let f = Falso::novo(&a);
    f.clientes(&["/dev/pts/1", "/dev/pts/2"]);
    let st = a.stamp("k12");
    let r = stale(
        &a,
        &[
            "--notify",
            "tmux",
            "--tmux-bin",
            f.bin_s().as_str(),
            "--stamp-file",
            s(&st),
        ],
    );
    assert_eq!(r.code, Some(6), "{r:?}");
    let subs = f.subcomandos();
    // Controle: o caminho positivo chegou a rodar (senao a propriedade e vacua).
    assert!(subs.iter().any(|c| c == "display-message"), "{subs:?}");
    for c in &subs {
        assert!(
            PERMITIDOS.contains(&c.as_str()),
            "subcomando nao permitido: {c}"
        );
    }
}

// =====================================================================
// K13: --tmux-socket P: toda chamada leva `-S P` antes do subcomando
// =====================================================================

#[test]
fn k13_tmux_socket_vai_como_dash_s_antes_do_subcomando() {
    let a = Amb::novo();
    cenario_claude(&a);
    let f = Falso::novo(&a);
    f.clientes(&["/dev/pts/1"]);
    let sock = s(&a.p().join("tmux-proprio.sock")).to_string();
    let st = a.stamp("k13");
    let r = stale(
        &a,
        &[
            "--notify",
            "tmux",
            "--tmux-bin",
            f.bin_s().as_str(),
            "--tmux-socket",
            sock.as_str(),
            "--stamp-file",
            s(&st),
        ],
    );
    assert_eq!(r.code, Some(6), "{r:?}");
    let c = f.chamadas();
    assert!(!c.is_empty(), "o falso recebeu chamadas: {c:?}");
    for chamada in &c {
        assert!(
            chamada.len() >= 3 && chamada[0] == "-S" && chamada[1] == sock,
            "toda chamada leva -S P antes do subcomando: {chamada:?}"
        );
        assert!(PERMITIDOS.contains(&chamada[2].as_str()), "{chamada:?}");
    }
}

// =====================================================================
// K14: regressao: sem --notify tmux o stale nao muda nada (S1 a S11 e v1.8)
// =====================================================================

#[test]
fn k14a_sem_pendencia_sai_0_sem_linha_e_nao_toca_o_falso() {
    let a = Amb::novo();
    let mut l = Log::novo(&a.state());
    l.pendente(
        "k14a",
        "codex-web",
        "claude",
        "AVISO",
        "assunto",
        "corpo",
        2,
    );
    let f = Falso::novo(&a);
    let st = a.stamp("k14a");
    let caminho = format!(
        "{}:{}",
        f.dir.display(),
        std::env::var("PATH").unwrap_or_default()
    );
    let st_s = s(&st).to_string();
    let r = roda_com(
        &a,
        &[
            "stale",
            "--state-dir",
            s(&a.state()),
            "--stamp-file",
            st_s.as_str(),
        ],
        &[("PATH", caminho.as_str())],
    );
    assert_eq!(r.code, Some(0), "{r:?}");
    assert!(paradas(&r.stdout).is_empty(), "nenhuma linha: {}", r.stdout);
    assert!(
        f.chamadas().is_empty(),
        "sem --notify tmux nenhum tmux no PATH e chamado"
    );
}

#[test]
fn k14b_parado_sem_tmux_sai_6_com_a_linha_de_texto_fixo() {
    let a = Amb::novo();
    cenario_claude(&a);
    let f = Falso::novo(&a);
    let st = a.stamp("k14b");
    let caminho = format!(
        "{}:{}",
        f.dir.display(),
        std::env::var("PATH").unwrap_or_default()
    );
    let st_s = s(&st).to_string();
    let r = roda_com(
        &a,
        &[
            "stale",
            "--state-dir",
            s(&a.state()),
            "--stamp-file",
            st_s.as_str(),
        ],
        &[("PATH", caminho.as_str())],
    );
    assert_eq!(r.code, Some(6), "{r:?}");
    let linha = paradas(&r.stdout);
    assert_eq!(linha.len(), 1, "{}", r.stdout);
    assert!(linha[0].starts_with(LINHA_PARADO), "{}", linha[0]);
    assert!(
        f.chamadas().is_empty(),
        "sem --notify tmux nenhum tmux no PATH e chamado"
    );
}

#[test]
fn k14c_sem_stamp_novo_a_segunda_execucao_com_file_continua_sai_0() {
    // Regressao da deduplicacao (C12 da v1.7/v1.8): mesmo episodio, segunda execucao nao repete a linha nem sai 6.
    let a = Amb::novo();
    cenario_claude(&a);
    let arq = a.arquivo("alertas-k14c.log");
    let f_s = s(&arq).to_string();
    let st = a.stamp("k14c");
    let st_s = s(&st).to_string();
    let fa = format!("file:{f_s}");
    let args = ["--notify", fa.as_str(), "--stamp-file", st_s.as_str()];
    let r1 = stale(&a, &args);
    assert_eq!(r1.code, Some(6), "{r1:?}");
    let r2 = stale(&a, &args);
    assert_eq!(
        r2.code,
        Some(0),
        "segunda execucao no mesmo episodio sai 0: {r2:?}"
    );
    let linhas = fs::read_to_string(&arq).unwrap_or_default();
    assert_eq!(
        linhas.matches("[chat] parado: claude").count(),
        1,
        "a linha nao se repete no arquivo: {linhas}"
    );
}

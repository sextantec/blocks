//! attest v0: testes de contrato A1..A21 de docs/attest-spec-v0.md.
//! Escritos so a partir do contrato e do modelo de estilo tests/v19_sink_tmux.rs; src/ nao foi lido.
//! Caixa preta: so o binario, os arquivos de estado e repositorios git temporarios.
//! Cada teste tem diretorio temporario proprio com repo/ (git), home/, xdg/ e os estados (state/, ...).
//! HOME, XDG_* e GIT_CONFIG_GLOBAL apontam para dentro do temporario; nenhum acesso ao repositorio real,
//! a ~/.local, ~/.config ou ~/.claude. Sem rede, sem agente real.
//! Controle positivo: cada negativo vem depois de um positivo com o mesmo comando/estado, que hoje ja falha.
//! Marcados AMBIGUO: pontos que o spec nao fixa (formato de saida, texto, ordem, campo).

use serde_json::{Value, json};
use std::fs;
use std::io::{Read, Write};
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::thread;
use std::time::{Duration, Instant};

const BIN: &str = env!("CARGO_BIN_EXE_attest");
/// Limite de espera de cada chamada (rede de seguranca do teste; o timeout real e o do proprio attest).
const LIMITE: Duration = Duration::from_secs(20);
const AUT: &str = "Jabbur (dono), 2026-10-16, \"teste independente\"";
const SHA_VAZIO: &str = "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855";
const SHA_ABC: &str = "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad";
const SHA_MIL_A: &str = "cdc76e5c9914fb9281a1c7e284d73e67f1809a48a497200e046d39ccc7112cd0";
/// Politica do exemplo do spec (secao "Politica").
const POLITICA_SPEC: &str = r#"{"steps": [{"name": "gate", "kind": "run"}, {"name": "tests", "independent_of": "author"}, {"name": "review", "independent_of": "author"}]}"#;
const POLITICA_GATE: &str = r#"{"steps": [{"name": "gate", "kind": "run"}]}"#;

static CONTADOR: AtomicUsize = AtomicUsize::new(0);

// ---------- infraestrutura ----------

/// Diretorio temporario com home/, xdg/, xdg-state/ e repo/ (repositorio git com um commit).
struct Amb {
    _d: tempfile::TempDir,
    raiz: PathBuf,
}

impl Amb {
    fn repo(&self) -> PathBuf {
        self.raiz.join("repo")
    }
    /// Diretorio de estado dentro do temporario (ainda nao criado: quem cria e o attest).
    fn estado(&self, nome: &str) -> PathBuf {
        self.raiz.join(nome)
    }
}

struct Saida {
    code: Option<i32>,
    out: Vec<u8>,
    err: Vec<u8>,
}

impl Saida {
    fn out_s(&self) -> String {
        String::from_utf8_lossy(&self.out).into_owned()
    }
    fn err_s(&self) -> String {
        String::from_utf8_lossy(&self.err).into_owned()
    }
    fn tudo(&self) -> String {
        format!("{}{}", self.out_s(), self.err_s())
    }
}

fn novo_amb() -> Amb {
    let d = tempfile::tempdir().expect("diretorio temporario");
    let raiz = d.path().to_path_buf();
    let amb = Amb { _d: d, raiz };
    for p in ["home", "repo", "xdg", "xdg-state"] {
        fs::create_dir_all(amb.raiz.join(p)).expect("criar subdiretorio");
    }
    let repo = amb.repo();
    git(&amb, &repo, &["init", "-q"]);
    git(
        &amb,
        &repo,
        &["config", "user.email", "teste@exemplo.invalid"],
    );
    git(&amb, &repo, &["config", "user.name", "teste"]);
    git(&amb, &repo, &["config", "commit.gpgsign", "false"]);
    fs::write(repo.join("a.txt"), "um\n").expect("escrever a.txt");
    fs::write(repo.join(".gitignore"), "*.log\n").expect("escrever .gitignore");
    git(&amb, &repo, &["add", "-A"]);
    git(&amb, &repo, &["commit", "-q", "-m", "inicial"]);
    amb
}

/// Ambiente isolado: HOME e XDG no temporario, git sem configuracao global.
fn ambiente(cmd: &mut Command, amb: &Amb) {
    cmd.env("HOME", amb.raiz.join("home"))
        .env("XDG_CONFIG_HOME", amb.raiz.join("xdg"))
        .env("XDG_STATE_HOME", amb.raiz.join("xdg-state"))
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env_remove("ATTEST_STATE_DIR");
}

/// Executa e captura saida; mata o processo se passar de LIMITE (o codigo fica None).
fn executa(mut cmd: Command) -> Saida {
    cmd.stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut filho = cmd.spawn().expect("executar comando");
    let mut out = filho.stdout.take().expect("stdout do filho");
    let mut err = filho.stderr.take().expect("stderr do filho");
    let th_out = thread::spawn(move || {
        let mut v = Vec::new();
        let _ = out.read_to_end(&mut v);
        v
    });
    let th_err = thread::spawn(move || {
        let mut v = Vec::new();
        let _ = err.read_to_end(&mut v);
        v
    });
    let fim = Instant::now() + LIMITE;
    let code = loop {
        if let Some(st) = filho.try_wait().expect("try_wait") {
            break st.code();
        }
        if Instant::now() >= fim {
            let _ = filho.kill();
            let _ = filho.wait();
            break None;
        }
        thread::sleep(Duration::from_millis(10));
    };
    Saida {
        code,
        out: th_out.join().expect("thread stdout"),
        err: th_err.join().expect("thread stderr"),
    }
}

/// Chama o attest. `args[0]` e o subcomando; `--state-dir` entra logo depois dele. AMBIGUO: posicao da flag.
fn attest(amb: &Amb, cwd: &Path, estado: &Path, args: &[&str]) -> Saida {
    let (sub, resto) = args.split_first().expect("subcomando");
    let mut cmd = Command::new(BIN);
    cmd.current_dir(cwd)
        .arg(sub)
        .arg("--state-dir")
        .arg(estado)
        .args(resto);
    ambiente(&mut cmd, amb);
    executa(cmd)
}

/// git dentro de um repositorio temporario; falha o teste se o git do teste falhar.
fn git(amb: &Amb, cwd: &Path, args: &[&str]) -> String {
    let mut cmd = Command::new("git");
    cmd.current_dir(cwd).args(args);
    ambiente(&mut cmd, amb);
    let s = executa(cmd);
    assert_eq!(s.code, Some(0), "git {args:?} falhou: {}", s.err_s());
    s.out_s().trim().to_string()
}

/// SHA-256 de referencia (sha256sum do sistema), para conferir output_sha256 em bytes conhecidos.
fn sha256_sistema(bytes: &[u8]) -> String {
    let mut filho = Command::new("sha256sum")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .expect("sha256sum");
    filho
        .stdin
        .take()
        .expect("stdin do sha256sum")
        .write_all(bytes)
        .expect("escrever no sha256sum");
    let s = filho.wait_with_output().expect("sha256sum");
    String::from_utf8_lossy(&s.stdout)
        .split_whitespace()
        .next()
        .unwrap_or("")
        .to_string()
}

fn cod(s: &Saida, esperado: i32, contexto: &str) {
    assert_eq!(
        s.code,
        Some(esperado),
        "{contexto}\n  stdout: {}\n  stderr: {}",
        s.out_s(),
        s.err_s()
    );
}

/// Registros do log.jsonl (vazio se o arquivo nao existe).
fn registros(estado: &Path) -> Vec<Value> {
    match fs::read_to_string(estado.join("log.jsonl")) {
        Ok(t) => t
            .lines()
            .map(|l| {
                serde_json::from_str(l)
                    .unwrap_or_else(|e| panic!("linha de registro invalida ({e}): {l}"))
            })
            .collect(),
        Err(_) => Vec::new(),
    }
}

fn ultimo(estado: &Path) -> Value {
    registros(estado)
        .last()
        .cloned()
        .expect("deveria haver um registro")
}

/// `attest run` com o padrao de campos (modelo fixo, sem effort).
fn grava_run(
    amb: &Amb,
    estado: &Path,
    passo: &str,
    tarefa: &str,
    agente: &str,
    cmd: &[&str],
) -> Saida {
    let mut args = vec![
        "run",
        "--step",
        passo,
        "--task",
        tarefa,
        "--agent",
        agente,
        "--model",
        "modelo-teste",
        "--authorized-by",
        AUT,
        "--",
    ];
    args.extend_from_slice(cmd);
    attest(amb, &amb.repo(), estado, &args)
}

/// `attest note` com o padrao de campos.
fn grava_note(
    amb: &Amb,
    estado: &Path,
    passo: &str,
    tarefa: &str,
    agente: &str,
    resultado: &str,
    detalhe: &str,
) -> Saida {
    attest(
        amb,
        &amb.repo(),
        estado,
        &[
            "note",
            "--step",
            passo,
            "--task",
            tarefa,
            "--agent",
            agente,
            "--model",
            "modelo-teste",
            "--authorized-by",
            AUT,
            "--result",
            resultado,
            "--detail",
            detalhe,
        ],
    )
}

fn check(amb: &Amb, estado: &Path, politica: &Path, tarefa: &str) -> Saida {
    let p = politica.to_str().expect("caminho utf-8");
    attest(
        amb,
        &amb.repo(),
        estado,
        &[
            "check", "--tree", "auto", "--policy", p, "--task", tarefa, "--json",
        ],
    )
}

fn politica(amb: &Amb, conteudo: &str) -> PathBuf {
    let n = CONTADOR.fetch_add(1, Ordering::SeqCst);
    let p = amb.raiz.join(format!("politica-{n}.json"));
    fs::write(&p, conteudo).expect("escrever politica");
    p
}

/// Lista de passos do JSON de `check --json`. AMBIGUO: o spec nao fixa o envelope (array ou objeto).
fn passos(s: &Saida) -> Vec<Value> {
    let v: Value = serde_json::from_slice(&s.out)
        .unwrap_or_else(|e| panic!("check --json nao devolveu JSON ({e}): {}", s.out_s()));
    let lista = match &v {
        Value::Array(a) => Some(a.clone()),
        Value::Object(m) => m.values().find_map(|x| x.as_array().cloned()),
        _ => None,
    };
    lista.unwrap_or_else(|| panic!("JSON sem lista de passos: {}", s.out_s()))
}

fn status_do(lista: &[Value], passo: &str) -> Option<String> {
    lista
        .iter()
        .find(|p| p["step"] == passo)
        .and_then(|p| p["status"].as_str().map(str::to_string))
}

/// Verdadeiro se `n` aparece como token isolado em `s`.
fn tem_token(s: &str, n: &str) -> bool {
    s.split(|c: char| !c.is_ascii_alphanumeric())
        .any(|t| t == n)
}

/// Estado com os registros do cenario do A11/A12/A20: gate (run), author, tests e review.
/// `sem` lista os passos a NAO gravar; `agente_testes` e o agente do passo `tests`.
fn monta(amb: &Amb, nome: &str, sem: &[&str], agente_testes: &str) -> PathBuf {
    let st = amb.estado(nome);
    if !sem.contains(&"gate") {
        cod(
            &grava_run(amb, &st, "gate", "T1", "a1", &["printf", "ok\n"]),
            0,
            "monta: gate",
        );
    }
    if !sem.contains(&"author") {
        cod(
            &grava_note(amb, &st, "author", "T1", "autor1", "pass", "autor"),
            0,
            "monta: author",
        );
    }
    if !sem.contains(&"tests") {
        cod(
            &grava_note(amb, &st, "tests", "T1", agente_testes, "pass", "testes"),
            0,
            "monta: tests",
        );
    }
    if !sem.contains(&"review") {
        cod(
            &grava_note(amb, &st, "review", "T1", "revisor3", "pass", "revisao"),
            0,
            "monta: review",
        );
    }
    st
}

// ---------- testes A1..A21 ----------

/// A1: run com comando que sai 0 grava registro pass; a saida nao vai ao log.
#[test]
fn a01_run_pass_grava_registro_sem_saida_no_log() {
    let amb = novo_amb();
    let st = amb.estado("state");
    let saida_cmd = "saida-secreta-A1\n";
    let s = grava_run(&amb, &st, "gate", "T1", "a1", &["printf", saida_cmd]);
    cod(&s, 0, "A1: run com comando que sai 0 deve sair 0");
    assert!(
        s.out_s().contains("saida-secreta-A1"),
        "A1: a saida do comando deve passar ao terminal"
    );
    let r = registros(&st);
    assert_eq!(r.len(), 1, "A1: deve haver exatamente um registro");
    let x = &r[0];
    assert_eq!(x["v"], json!(1));
    assert_eq!(x["seq"], json!(1));
    assert_eq!(x["prev"], json!(""));
    assert_eq!(x["kind"], json!("run"));
    assert_eq!(x["step"], json!("gate"));
    assert_eq!(x["task"], json!("T1"));
    assert_eq!(x["agent"], json!("a1"));
    assert_eq!(x["model"], json!("modelo-teste"));
    assert_eq!(x["authorized_by"], json!(AUT));
    assert_eq!(x["result"], json!("pass"));
    assert_eq!(x["exit"], json!(0));
    assert_eq!(x["argv"], json!(["printf", saida_cmd]));
    assert_eq!(x["output_bytes"], json!(saida_cmd.len()));
    assert_eq!(
        x["output_sha256"],
        json!(sha256_sistema(saida_cmd.as_bytes()))
    );
    let arvore = git(&amb, &amb.repo(), &["rev-parse", "HEAD^{tree}"]);
    let cabeca = git(&amb, &amb.repo(), &["rev-parse", "HEAD"]);
    assert_eq!(
        x["tree"],
        json!(arvore),
        "A1: tree = arvore de trabalho (repo limpo)"
    );
    assert_eq!(x["head"], json!(cabeca), "A1: head = HEAD");
    // AMBIGUO: formato exato de ts (o spec diz so ISO 8601 UTC); aqui so se confere que e texto com 'T'.
    assert!(
        x["ts"].as_str().is_some_and(|t| t.contains('T')),
        "A1: ts deve ser ISO 8601"
    );
    let log = fs::read_to_string(st.join("log.jsonl")).expect("log.jsonl");
    assert!(
        !log.contains("saida-secreta-A1"),
        "A1: a saida do comando nunca pode aparecer no log"
    );
}

/// A2: comando que sai 3 grava fail com exit 3, e o attest sai 3.
#[test]
fn a02_run_fail_sai_com_codigo_do_comando() {
    let amb = novo_amb();
    let st = amb.estado("state");
    cod(
        &grava_run(&amb, &st, "gate", "T1", "a1", &["sh", "-c", "exit 0"]),
        0,
        "A2 controle: comando que sai 0 deve sair 0",
    );
    let s = grava_run(&amb, &st, "gate", "T1", "a1", &["sh", "-c", "exit 3"]);
    cod(&s, 3, "A2: comando que sai 3 deve fazer o attest sair 3");
    let r = registros(&st);
    assert_eq!(r.len(), 2, "A2: os dois runs devem estar gravados");
    assert_eq!(r[1]["result"], json!("fail"));
    assert_eq!(r[1]["exit"], json!(3));
}

/// A3: comando inexistente grava fail com exit null e sai 127.
#[test]
fn a03_comando_inexistente_grava_fail_exit_null_sai_127() {
    let amb = novo_amb();
    let st = amb.estado("state");
    cod(
        &grava_run(&amb, &st, "gate", "T1", "a1", &["printf", "ok\n"]),
        0,
        "A3 controle: comando existente deve sair 0",
    );
    let s = grava_run(&amb, &st, "gate", "T1", "a1", &["nao-existe-attest-a03"]);
    // AMBIGUO: "saida 127" lido como codigo de saida do attest.
    cod(
        &s,
        127,
        "A3: comando inexistente deve fazer o attest sair 127",
    );
    let r = registros(&st);
    assert_eq!(
        r.len(),
        2,
        "A3: o registro do comando inexistente deve ser gravado"
    );
    assert_eq!(r[1]["result"], json!("fail"));
    assert_eq!(r[1]["exit"], json!(null));
}

/// A4: timeout grava fail e sai 124.
#[test]
fn a04_timeout_grava_fail_e_sai_124() {
    let amb = novo_amb();
    let st = amb.estado("state");
    let s = attest(
        &amb,
        &amb.repo(),
        &st,
        &[
            "run",
            "--step",
            "gate",
            "--task",
            "T1",
            "--agent",
            "a1",
            "--model",
            "modelo-teste",
            "--authorized-by",
            AUT,
            "--timeout",
            "1",
            "--",
            "printf",
            "ok\n",
        ],
    );
    cod(
        &s,
        0,
        "A4 controle: --timeout 1 com comando rapido deve sair 0",
    );
    let inicio = Instant::now();
    let s = attest(
        &amb,
        &amb.repo(),
        &st,
        &[
            "run",
            "--step",
            "gate",
            "--task",
            "T1",
            "--agent",
            "a1",
            "--model",
            "modelo-teste",
            "--authorized-by",
            AUT,
            "--timeout",
            "1",
            "--",
            "sleep",
            "5",
        ],
    );
    let decorrido = inicio.elapsed();
    cod(&s, 124, "A4: timeout deve sair 124");
    assert!(
        decorrido < Duration::from_secs(4),
        "A4: o timeout de 1 s nao foi aplicado (levou {decorrido:?})"
    );
    let r = registros(&st);
    assert_eq!(r.len(), 2, "A4: o registro do timeout deve ser gravado");
    assert_eq!(r[1]["result"], json!("fail"));
    // AMBIGUO: o spec nao diz se `exit` vale null ou 124 quando ha timeout; nao se afirma aqui.
}

/// A5: `--` e obrigatorio; sem comando tambem e uso (2).
#[test]
fn a05_separador_obrigatorio_e_comando_obrigatorio() {
    let amb = novo_amb();
    let st = amb.estado("state");
    cod(
        &grava_run(&amb, &st, "gate", "T1", "a1", &["printf", "ok\n"]),
        0,
        "A5 controle: com -- o run deve sair 0",
    );
    let base = [
        "run",
        "--step",
        "gate",
        "--task",
        "T1",
        "--agent",
        "a1",
        "--model",
        "modelo-teste",
        "--authorized-by",
        AUT,
    ];
    let mut sem_separador = base.to_vec();
    sem_separador.extend_from_slice(&["printf", "ok\n"]);
    let s = attest(&amb, &amb.repo(), &st, &sem_separador);
    cod(&s, 2, "A5: sem -- o attest deve sair 2");
    let mut sem_comando = base.to_vec();
    sem_comando.push("--");
    let s = attest(&amb, &amb.repo(), &st, &sem_comando);
    cod(&s, 2, "A5: sem comando depois de -- deve sair 2");
    assert_eq!(
        registros(&st).len(),
        1,
        "A5: nenhum registro novo nos casos de uso"
    );
}

/// A6: metacaracteres chegam literais ao comando (sem shell).
#[test]
fn a06_metacaracteres_chegam_literais() {
    let amb = novo_amb();
    let st = amb.estado("state");
    cod(
        &grava_run(&amb, &st, "gate", "T1", "a1", &["printf", "%s\n", "ok"]),
        0,
        "A6 controle: argumento simples deve sair 0",
    );
    let s = grava_run(
        &amb,
        &st,
        "gate",
        "T1",
        "a1",
        &["printf", "%s|%s\n", "a;b", "$(touch INJETADO)"],
    );
    cod(&s, 0, "A6: o comando com metacaracteres deve sair 0");
    assert_eq!(
        s.out_s(),
        "a;b|$(touch INJETADO)\n",
        "A6: os metacaracteres devem chegar literais ao printf"
    );
    assert!(
        !amb.repo().join("INJETADO").exists(),
        "A6: nenhum shell pode ter executado $(...)"
    );
    assert_eq!(
        ultimo(&st)["argv"],
        json!(["printf", "%s|%s\n", "a;b", "$(touch INJETADO)"])
    );
}

/// A7: tree muda com arquivo rastreado ou nao ignorado; nao muda com o .gitignored; repo intacto.
#[test]
fn a07_tree_ignora_gitignore_e_nao_toca_repositorio() {
    let amb = novo_amb();
    let st = amb.estado("state");
    let repo = amb.repo();
    // Controle: repositorio limpo grava um registro com tree.
    cod(
        &grava_run(&amb, &st, "gate", "T1", "a1", &["printf", "ok\n"]),
        0,
        "A7 controle: run no repositorio limpo deve sair 0",
    );
    let t1 = ultimo(&st)["tree"].clone();
    assert!(t1.is_string(), "A7: registro sem tree");

    // Arquivo ignorado pelo .gitignore: a tree nao muda; status e indice ficam intactos.
    fs::write(repo.join("ignorado.log"), "segredo ignorado\n").unwrap();
    let status1 = git(&amb, &repo, &["status", "--porcelain"]);
    let idx1 = fs::read(repo.join(".git/index")).unwrap();
    cod(
        &grava_run(&amb, &st, "gate", "T1", "a1", &["printf", "ok\n"]),
        0,
        "A7: run com arquivo ignorado deve sair 0",
    );
    let idx2 = fs::read(repo.join(".git/index")).unwrap();
    assert_eq!(idx1, idx2, "A7: o indice real nao pode mudar");
    assert_eq!(
        git(&amb, &repo, &["status", "--porcelain"]),
        status1,
        "A7: git status nao pode mudar"
    );
    assert_eq!(
        ultimo(&st)["tree"],
        t1,
        "A7: arquivo .gitignored nao muda a tree"
    );

    // Arquivo novo nao ignorado: a tree muda; o indice real segue intacto.
    fs::write(repo.join("novo.txt"), "novo\n").unwrap();
    let status2 = git(&amb, &repo, &["status", "--porcelain"]);
    let idx3 = fs::read(repo.join(".git/index")).unwrap();
    cod(
        &grava_run(&amb, &st, "gate", "T1", "a1", &["printf", "ok\n"]),
        0,
        "A7: run com arquivo novo deve sair 0",
    );
    let idx4 = fs::read(repo.join(".git/index")).unwrap();
    assert_eq!(
        idx3, idx4,
        "A7: o indice real nao pode mudar com arquivo novo"
    );
    assert_eq!(
        git(&amb, &repo, &["status", "--porcelain"]),
        status2,
        "A7: git status nao pode mudar com arquivo novo"
    );
    let t3 = ultimo(&st)["tree"].clone();
    assert_ne!(t3, t1, "A7: arquivo novo nao ignorado deve mudar a tree");

    // Arquivo rastreado alterado: a tree muda de novo.
    fs::write(repo.join("a.txt"), "dois\n").unwrap();
    cod(
        &grava_run(&amb, &st, "gate", "T1", "a1", &["printf", "ok\n"]),
        0,
        "A7: run com a.txt alterado deve sair 0",
    );
    assert_ne!(
        ultimo(&st)["tree"],
        t3,
        "A7: arquivo rastreado alterado deve mudar a tree"
    );
}

/// A8: note valido grava; detail acima de 280 ou com quebra de linha sai 2.
#[test]
fn a08_note_detail_limite_e_quebra_de_linha() {
    let amb = novo_amb();
    let st = amb.estado("state");
    // AMBIGUO: o limite de 280 e contado em caracteres ou bytes; usa-se so ASCII aqui.
    let limite = "a".repeat(280);
    cod(
        &grava_note(&amb, &st, "author", "T1", "a1", "pass", &limite),
        0,
        "A8 controle: note com detail de 280 caracteres deve sair 0",
    );
    let r = registros(&st);
    assert_eq!(r.len(), 1);
    assert_eq!(r[0]["kind"], json!("note"));
    assert_eq!(r[0]["step"], json!("author"));
    assert_eq!(r[0]["result"], json!("pass"));
    assert_eq!(r[0]["detail"], json!(limite));
    let antes = registros(&st).len();
    cod(
        &grava_note(&amb, &st, "author", "T1", "a1", "pass", &"b".repeat(281)),
        2,
        "A8: detail de 281 caracteres deve sair 2",
    );
    cod(
        &grava_note(&amb, &st, "author", "T1", "a1", "pass", "linha1\nlinha2"),
        2,
        "A8: detail com quebra de linha deve sair 2",
    );
    assert_eq!(
        registros(&st).len(),
        antes,
        "A8: nenhum registro novo nos casos recusados"
    );
}

/// A9: nomes invalidos (step, task, agent) saem 2; nomes validos (com os limites) passam.
#[test]
fn a09_nomes_validos_e_invalidos() {
    let amb = novo_amb();
    let st = amb.estado("state");
    let passo_32 = "a".repeat(32);
    let tarefa_64 = "T".repeat(64);
    cod(
        &grava_note(&amb, &st, "gate", "T-1.a_b", "agente_1", "pass", "x"),
        0,
        "A9 controle: nomes validos devem sair 0",
    );
    cod(
        &grava_note(&amb, &st, &passo_32, &tarefa_64, "a", "pass", "x"),
        0,
        "A9 controle: step de 32 e task de 64 caracteres devem sair 0",
    );
    let antes = registros(&st).len();
    let invalidos: Vec<(String, String, String)> = vec![
        ("Gate".into(), "T1".into(), "a1".into()),
        ("".into(), "T1".into(), "a1".into()),
        ("a".repeat(33), "T1".into(), "a1".into()),
        ("gate passo".into(), "T1".into(), "a1".into()),
        ("gate".into(), "T 1".into(), "a1".into()),
        ("gate".into(), "T".repeat(65), "a1".into()),
        ("gate".into(), "".into(), "a1".into()),
        ("gate".into(), "T1".into(), "Agente".into()),
        ("gate".into(), "T1".into(), "a.1".into()),
        ("gate".into(), "T1".into(), "a".repeat(33)),
    ];
    for (p, t, a) in &invalidos {
        cod(
            &grava_note(&amb, &st, p, t, a, "pass", "x"),
            2,
            &format!("A9: nome invalido (step={p:?}, task={t:?}, agent={a:?}) deve sair 2"),
        );
    }
    assert_eq!(
        registros(&st).len(),
        antes,
        "A9: nenhum registro nos nomes invalidos"
    );
}

/// A10: cadeia: verify 0 em log integro; editar, remover ou trocar linha = 4 e informa a linha.
#[test]
fn a10_cadeia_detecta_edicao_remocao_e_troca() {
    let amb = novo_amb();
    let st = amb.estado("state");
    for d in ["um", "dois", "tres"] {
        cod(
            &grava_note(&amb, &st, "author", "T1", "a1", "pass", d),
            0,
            "A10: note para montar a cadeia",
        );
    }
    let original = fs::read_to_string(st.join("log.jsonl")).expect("log.jsonl");
    assert_eq!(original.lines().count(), 3, "A10: tres registros");
    let verifica = || attest(&amb, &amb.repo(), &st, &["verify"]);
    cod(
        &verifica(),
        0,
        "A10 controle: log integro deve passar no verify",
    );

    // a) editar a linha do meio (seq 2): o registro ainda e JSON valido, mas o hash da cadeia muda.
    let editado = original.replacen("dois", "DOIS", 1);
    assert_ne!(editado, original);
    fs::write(st.join("log.jsonl"), &editado).unwrap();
    let s = verifica();
    cod(&s, 4, "A10: linha editada deve sair 4");
    assert!(
        tem_token(&s.tudo(), "2"),
        "A10: a saida deve informar a linha 2: {}",
        s.tudo()
    );

    // b) remover a linha do meio.
    let mut linhas: Vec<&str> = original.lines().collect();
    linhas.remove(1);
    fs::write(st.join("log.jsonl"), format!("{}\n", linhas.join("\n"))).unwrap();
    let s = verifica();
    cod(&s, 4, "A10: linha removida no meio deve sair 4");
    assert!(
        tem_token(&s.tudo(), "2"),
        "A10: a saida deve informar a linha 2 (apos remocao): {}",
        s.tudo()
    );

    // c) trocar duas linhas de lugar.
    let mut linhas: Vec<&str> = original.lines().collect();
    linhas.swap(0, 1);
    fs::write(st.join("log.jsonl"), format!("{}\n", linhas.join("\n"))).unwrap();
    // AMBIGUO: qual linha o verify aponta na troca nao e fixado; so o codigo 4 e afirmado.
    cod(&verifica(), 4, "A10: linhas trocadas devem sair 4");
}

/// A11: check completo = 0; sem qualquer passo = 3 com o passo ausente como missing.
#[test]
fn a11_check_completo_e_passo_removido() {
    let amb = novo_amb();
    let pol = politica(&amb, POLITICA_SPEC);
    let cheio = monta(&amb, "cheio", &[], "tester2");
    cod(
        &check(&amb, &cheio, &pol, "T1"),
        0,
        "A11 controle: registros completos devem sair 0",
    );

    for ausente in ["gate", "tests", "review"] {
        let st = monta(&amb, &format!("sem-{ausente}"), &[ausente], "tester2");
        let s = check(&amb, &st, &pol, "T1");
        cod(&s, 3, &format!("A11: sem o passo {ausente} deve sair 3"));
        assert_eq!(
            status_do(&passos(&s), ausente).as_deref(),
            Some("missing"),
            "A11: o passo {ausente} deve estar missing"
        );
    }
    // AMBIGUO: sem `author`, o spec so diz que `tests` exige `author` ok; o passo reportado nao e fixado.
    let st = monta(&amb, "sem-author", &["author"], "tester2");
    let s = check(&amb, &st, &pol, "T1");
    cod(&s, 3, "A11: sem o passo author deve sair 3");
    assert_ne!(
        status_do(&passos(&s), "tests").as_deref(),
        Some("ok"),
        "A11: sem author, tests nao pode ficar ok"
    );
}

/// A12: tests pelo mesmo agente do author = not_independent e saida 3.
#[test]
fn a12_tests_mesmo_agente_do_author_nao_independente() {
    let amb = novo_amb();
    let pol = politica(&amb, POLITICA_SPEC);
    let outro = monta(&amb, "outro", &[], "tester2");
    cod(
        &check(&amb, &outro, &pol, "T1"),
        0,
        "A12 controle: tests de outro agente deve sair 0",
    );
    let mesmo = monta(&amb, "mesmo", &[], "autor1");
    let s = check(&amb, &mesmo, &pol, "T1");
    cod(&s, 3, "A12: tests do mesmo agente do author deve sair 3");
    assert_eq!(
        status_do(&passos(&s), "tests").as_deref(),
        Some("not_independent"),
        "A12: tests deve estar not_independent"
    );
    // AMBIGUO: o formato de como os dois agentes aparecem na saida nao e fixado; so se confere o nome.
    assert!(
        s.out_s().contains("autor1"),
        "A12: a saida deve citar o agente"
    );
}

/// A13: depois do gate pass, alterar um arquivo deixa o gate missing (stale_tree).
#[test]
fn a13_arvore_mudou_gate_fica_stale() {
    let amb = novo_amb();
    let repo = amb.repo();
    let st = amb.estado("state");
    let pol = politica(&amb, POLITICA_GATE);
    cod(
        &grava_run(&amb, &st, "gate", "T1", "a1", &["printf", "ok\n"]),
        0,
        "A13: gate pass",
    );
    let s = check(&amb, &st, &pol, "T1");
    cod(&s, 0, "A13 controle: gate na mesma arvore deve sair 0");
    assert_eq!(status_do(&passos(&s), "gate").as_deref(), Some("ok"));

    fs::write(repo.join("a.txt"), "dois\n").unwrap();
    let s = check(&amb, &st, &pol, "T1");
    cod(&s, 3, "A13: arvore alterada deve fazer o check sair 3");
    assert_eq!(
        status_do(&passos(&s), "gate").as_deref(),
        Some("missing"),
        "A13: gate deve ficar missing na arvore nova"
    );
    // AMBIGUO: `stale_tree` pode ser um status, um campo ou um texto; so se exige a palavra na saida.
    assert!(
        s.out_s().contains("stale_tree"),
        "A13: a saida deve mencionar stale_tree"
    );
}

/// A14: fail mais novo derruba o pass antigo do mesmo passo e arvore.
#[test]
fn a14_fail_mais_novo_derruba_pass_antigo() {
    let amb = novo_amb();
    let st = amb.estado("state");
    let pol = politica(&amb, POLITICA_GATE);
    cod(
        &grava_run(&amb, &st, "gate", "T1", "a1", &["printf", "ok\n"]),
        0,
        "A14: gate pass",
    );
    cod(
        &check(&amb, &st, &pol, "T1"),
        0,
        "A14 controle: so o pass antigo deve deixar o check em 0",
    );
    cod(
        &grava_run(&amb, &st, "gate", "T1", "a1", &["sh", "-c", "exit 1"]),
        1,
        "A14: o run com falha deve sair 1",
    );
    let s = check(&amb, &st, &pol, "T1");
    cod(&s, 3, "A14: fail mais novo deve fazer o check sair 3");
    assert_eq!(
        status_do(&passos(&s), "gate").as_deref(),
        Some("failed"),
        "A14: gate deve estar failed"
    );
}

/// A15: footer imprime Agente e Autorizado por dos pass; sem pass sai 3 e nao imprime.
#[test]
fn a15_footer_imprime_agentes_e_autorizacao() {
    let amb = novo_amb();
    let repo = amb.repo();
    let st = amb.estado("state");
    cod(
        &grava_run(&amb, &st, "gate", "T1", "a1", &["printf", "ok\n"]),
        0,
        "A15: gate pass",
    );
    cod(
        &grava_note(&amb, &st, "author", "T1", "autor1", "pass", "autor"),
        0,
        "A15: author",
    );
    cod(
        &grava_note(&amb, &st, "tests", "T1", "tester2", "pass", "testes"),
        0,
        "A15: tests",
    );
    let s = attest(
        &amb,
        &repo,
        &st,
        &["footer", "--task", "T1", "--tree", "auto"],
    );
    cod(&s, 0, "A15: footer com pass deve sair 0");
    let linhas = s.out_s();
    // AMBIGUO: formato exato das linhas (ordem dos agentes, separadores); so o prefixo e os nomes.
    let agente = linhas
        .lines()
        .find(|l| l.starts_with("Agente:"))
        .unwrap_or("");
    assert!(
        agente.contains("a1") && agente.contains("autor1") && agente.contains("tester2"),
        "A15: linha Agente: deve citar os agentes: {linhas}"
    );
    let aut = linhas
        .lines()
        .find(|l| l.starts_with("Autorizado por:"))
        .unwrap_or("");
    assert!(
        aut.contains("Jabbur (dono)"),
        "A15: linha Autorizado por: deve citar a autorizacao: {linhas}"
    );

    let s = attest(
        &amb,
        &repo,
        &st,
        &["footer", "--task", "T-vazio", "--tree", "auto"],
    );
    cod(&s, 3, "A15: footer sem pass deve sair 3");
    assert!(
        !s.out_s().contains("Agente:") && !s.out_s().contains("Autorizado por:"),
        "A15: sem pass nao pode imprimir as linhas do rodape"
    );
}

/// A16: quatro `run` em paralelo: todos saem 0, seq sem buracos e cadeia valida.
#[test]
fn a16_runs_em_paralelo_mantem_seq_e_cadeia() {
    let amb = novo_amb();
    let st = amb.estado("state");
    let repo = amb.repo();
    let saidas: Vec<Saida> = thread::scope(|sc| {
        let hs: Vec<_> = (0..4)
            .map(|i| {
                let amb = &amb;
                let st = &st;
                let repo = &repo;
                sc.spawn(move || {
                    let tarefa = format!("T{i}");
                    attest(
                        amb,
                        repo,
                        st,
                        &[
                            "run",
                            "--step",
                            "gate",
                            "--task",
                            &tarefa,
                            "--agent",
                            "a1",
                            "--model",
                            "modelo-teste",
                            "--authorized-by",
                            AUT,
                            "--",
                            "printf",
                            "ok\n",
                        ],
                    )
                })
            })
            .collect();
        hs.into_iter().map(|h| h.join().expect("thread")).collect()
    });
    for (i, s) in saidas.iter().enumerate() {
        cod(s, 0, &format!("A16: run paralelo {i} deve sair 0"));
    }
    cod(
        &attest(&amb, &repo, &st, &["verify"]),
        0,
        "A16: a cadeia deve estar integra apos os runs paralelos",
    );
    let r = registros(&st);
    assert_eq!(r.len(), 4, "A16: quatro registros");
    let seqs: Vec<Value> = r.iter().map(|x| x["seq"].clone()).collect();
    assert_eq!(
        seqs,
        vec![json!(1), json!(2), json!(3), json!(4)],
        "A16: seq sem buracos"
    );
    let mut tarefas: Vec<String> = r.iter().map(|x| x["task"].to_string()).collect();
    tarefas.sort();
    tarefas.dedup();
    assert_eq!(
        tarefas.len(),
        4,
        "A16: cada run paralelo deve ter gravado o seu registro"
    );
}

/// A17: diretorio 0700 e log 0600.
#[test]
fn a17_permissoes_do_estado() {
    let amb = novo_amb();
    let st = amb.estado("state");
    cod(
        &grava_run(&amb, &st, "gate", "T1", "a1", &["printf", "ok\n"]),
        0,
        "A17: run deve criar o estado e sair 0",
    );
    let modo_dir = fs::metadata(&st)
        .expect("diretorio de estado")
        .permissions()
        .mode()
        & 0o777;
    let modo_log = fs::metadata(st.join("log.jsonl"))
        .expect("log.jsonl")
        .permissions()
        .mode()
        & 0o777;
    assert_eq!(modo_dir, 0o700, "A17: diretorio de estado deve ser 0700");
    assert_eq!(modo_log, 0o600, "A17: log.jsonl deve ser 0600");
}

/// A18: politica invalida (JSON ruim), passo repetido ou campo desconhecido sai 2.
#[test]
fn a18_politica_invalida_sai_2() {
    let amb = novo_amb();
    let st = amb.estado("state");
    cod(
        &grava_run(&amb, &st, "gate", "T1", "a1", &["printf", "ok\n"]),
        0,
        "A18: gate pass",
    );
    let valida = politica(&amb, POLITICA_GATE);
    cod(
        &check(&amb, &st, &valida, "T1"),
        0,
        "A18 controle: politica valida deve sair 0",
    );
    let invalidas = [
        "{nao e json",
        r#"{"steps": [{"name": "gate", "kind": "run"}, {"name": "gate", "kind": "run"}]}"#,
        r#"{"steps": [{"name": "gate", "kind": "run", "extra": 1}]}"#,
        // AMBIGUO: campo desconhecido no nivel de topo; o spec diz so "campo desconhecido".
        r#"{"steps": [{"name": "gate", "kind": "run"}], "extra": 1}"#,
    ];
    for c in invalidas {
        let p = politica(&amb, c);
        cod(
            &check(&amb, &st, &p, "T1"),
            2,
            &format!("A18: politica invalida {c:?} deve sair 2"),
        );
    }
}

/// A19: fora de repositorio git, run/note/check saem 2 e nao gravam.
#[test]
fn a19_fora_de_repositorio_sai_2() {
    let amb = novo_amb();
    let st = amb.estado("state");
    cod(
        &grava_run(&amb, &st, "gate", "T1", "a1", &["printf", "ok\n"]),
        0,
        "A19 controle: dentro do repositorio o run deve sair 0",
    );
    let fora = amb.raiz.join("fora");
    fs::create_dir_all(&fora).unwrap();
    let mut pre = Command::new("git");
    pre.current_dir(&fora)
        .args(["rev-parse", "--is-inside-work-tree"]);
    ambiente(&mut pre, &amb);
    assert_ne!(
        executa(pre).code,
        Some(0),
        "pre-condicao: a pasta temporaria nao pode estar dentro de um repositorio git"
    );
    let st_fora = amb.estado("state-fora");
    let s = attest(
        &amb,
        &fora,
        &st_fora,
        &[
            "run",
            "--step",
            "gate",
            "--task",
            "T1",
            "--agent",
            "a1",
            "--model",
            "m",
            "--authorized-by",
            AUT,
            "--",
            "printf",
            "ok\n",
        ],
    );
    cod(&s, 2, "A19: run fora de git deve sair 2");
    let s = attest(
        &amb,
        &fora,
        &st_fora,
        &[
            "note",
            "--step",
            "gate",
            "--task",
            "T1",
            "--agent",
            "a1",
            "--model",
            "m",
            "--authorized-by",
            AUT,
            "--result",
            "pass",
            "--detail",
            "x",
        ],
    );
    cod(&s, 2, "A19: note fora de git deve sair 2");
    let pol = politica(&amb, POLITICA_GATE);
    let s = attest(
        &amb,
        &fora,
        &st_fora,
        &[
            "check",
            "--tree",
            "auto",
            "--policy",
            pol.to_str().unwrap(),
            "--task",
            "T1",
            "--json",
        ],
    );
    cod(&s, 2, "A19: check fora de git deve sair 2");
    assert!(
        !st_fora.join("log.jsonl").exists(),
        "A19: nenhum registro pode ser gravado fora de git"
    );
}

/// A19b: sem `git` no PATH, check sai 2 (o tree nao pode ser calculado). AMBIGUO: o attest usa o git pelo PATH.
#[test]
fn a19b_sem_git_no_path_sai_2() {
    let amb = novo_amb();
    let st = amb.estado("state");
    cod(
        &grava_run(&amb, &st, "gate", "T1", "a1", &["printf", "ok\n"]),
        0,
        "A19b: gate pass",
    );
    let pol = politica(&amb, POLITICA_GATE);
    cod(
        &check(&amb, &st, &pol, "T1"),
        0,
        "A19b controle: com git no PATH o check deve sair 0",
    );
    let vazio = amb.raiz.join("path-vazio");
    fs::create_dir_all(&vazio).unwrap();
    let mut cmd = Command::new(BIN);
    cmd.current_dir(amb.repo())
        .arg("check")
        .arg("--state-dir")
        .arg(&st)
        .args(["--tree", "auto", "--policy"])
        .arg(&pol)
        .args(["--task", "T1", "--json"])
        .env("PATH", &vazio);
    ambiente(&mut cmd, &amb);
    cod(
        &executa(cmd),
        2,
        "A19b: sem git no PATH o check deve sair 2",
    );
}

/// A20: `check --json` traz step, status, agent e seq em cada passo da politica.
#[test]
fn a20_check_json_tem_campos() {
    let amb = novo_amb();
    let pol = politica(&amb, POLITICA_SPEC);
    let st = monta(&amb, "cheio", &[], "tester2");
    let s = check(&amb, &st, &pol, "T1");
    cod(&s, 0, "A20 controle: registros completos devem sair 0");
    let lista = passos(&s);
    for passo in ["gate", "tests", "review"] {
        let e = lista
            .iter()
            .find(|p| p["step"] == passo)
            .unwrap_or_else(|| panic!("A20: passo {passo} ausente do JSON: {}", s.out_s()));
        assert!(e["step"].is_string(), "A20: {passo} sem step");
        assert_eq!(e["status"], json!("ok"), "A20: {passo} deve estar ok");
        assert!(e["agent"].is_string(), "A20: {passo} sem agent");
        assert!(e["seq"].is_number(), "A20: {passo} sem seq");
    }
}

/// A21: SHA-256 do attest bate com os vetores conhecidos (via output_sha256 de comandos).
#[test]
fn a21_sha256_vetores_conhecidos() {
    let amb = novo_amb();
    let st = amb.estado("state");
    let casos: Vec<(Vec<&str>, &str, usize)> = vec![
        (vec!["printf", ""], SHA_VAZIO, 0),
        (vec!["printf", "abc"], SHA_ABC, 3),
        (
            vec!["sh", "-c", "head -c 1000000 /dev/zero | tr '\\0' a"],
            SHA_MIL_A,
            1_000_000,
        ),
    ];
    for (i, (cmd, sha, bytes)) in casos.iter().enumerate() {
        cod(
            &grava_run(&amb, &st, "hash", "T1", "a1", cmd),
            0,
            &format!("A21: comando de vetor {i} deve sair 0"),
        );
        let x = registros(&st).pop().expect("registro");
        assert_eq!(
            x["output_sha256"],
            json!(sha),
            "A21: vetor {i} com SHA-256 errado"
        );
        assert_eq!(
            x["output_bytes"],
            json!(bytes),
            "A21: vetor {i} com tamanho errado"
        );
    }
}

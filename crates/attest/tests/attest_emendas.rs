//! attest v0: testes das emendas A22 e A23 de docs/attest-spec-v0.md (itens 18 e 19, "Emendas apos a consulta ao Astra").
//! Escritos so a partir do contrato e dos helpers de tests/attest_v0.rs; src/ nao foi lido.
//! Caixa preta: so o binario, os arquivos de estado e repositorios git temporarios.
//! HOME, XDG_* e GIT_CONFIG_GLOBAL apontam para dentro do temporario; sem rede, sem agente real.
//! Controle positivo: cada afirmacao negativa vem depois de um controle que mostra que a medida discrimina.
//! `sh` aparece apenas como PROGRAMA do comando em teste (`sh -c 'echo x >> a.txt'`).

use serde_json::{Value, json};
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::thread;
use std::time::{Duration, Instant};

const BIN: &str = env!("CARGO_BIN_EXE_attest");
/// Limite de espera de cada chamada (rede de seguranca do teste; o timeout real e o do proprio attest).
const LIMITE: Duration = Duration::from_secs(20);
const AUT: &str = "Jabbur (dono), 2026-10-16, \"teste independente\"";
const POLITICA_GATE: &str = r#"{"steps": [{"name": "gate", "kind": "run"}]}"#;
/// Conteudo unico do arquivo nao ignorado: nao pode aparecer em nenhum objeto do repositorio real.
const SEGREDO: &str = "SEGREDO-UNICO-A22-7f3a9c";

static CONTADOR: AtomicUsize = AtomicUsize::new(0);

// ---------- infraestrutura (copiada de attest_v0.rs) ----------

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

/// Chama o attest. `args[0]` e o subcomando; `--state-dir` entra logo depois dele.
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

fn politica(amb: &Amb, conteudo: &str) -> PathBuf {
    let n = CONTADOR.fetch_add(1, Ordering::SeqCst);
    let p = amb.raiz.join(format!("politica-{n}.json"));
    fs::write(&p, conteudo).expect("escrever politica");
    p
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

/// Lista de passos do JSON de `check --json` (o envelope e objeto com a lista, ou a propria lista).
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

/// Contagem de objetos do repositorio: (count, in-pack) de `git count-objects -v`.
fn contagem(amb: &Amb, repo: &Path) -> (i64, i64) {
    let t = git(amb, repo, &["count-objects", "-v"]);
    let campo = |nome: &str| -> i64 {
        t.lines()
            .find_map(|l| l.strip_prefix(nome))
            .map(|v| v.trim().parse().expect("campo numerico"))
            .unwrap_or_else(|| panic!("count-objects sem o campo {nome}: {t}"))
    };
    (campo("count:"), campo("in-pack:"))
}

/// Lista de ids de todos os objetos do repositorio (sem conteudo).
fn ids_objetos(amb: &Amb, repo: &Path) -> String {
    git(
        amb,
        repo,
        &["cat-file", "--batch-all-objects", "--batch-check"],
    )
}

/// Verdadeiro se o marcador aparece no conteudo (decodificado) de algum objeto do repositorio.
fn objetos_contem(amb: &Amb, repo: &Path, marcador: &str) -> bool {
    let mut cmd = Command::new("git");
    cmd.current_dir(repo)
        .args(["cat-file", "--batch-all-objects", "--batch"]);
    ambiente(&mut cmd, amb);
    let s = executa(cmd);
    assert_eq!(s.code, Some(0), "cat-file falhou: {}", s.err_s());
    s.out
        .windows(marcador.len())
        .any(|w| w == marcador.as_bytes())
}

// ---------- A22 e A23 ----------

/// A22: nenhum objeto e gravado no .git real por run, note, check ou footer, mesmo com arquivo nao ignorado novo.
#[test]
fn a22_nao_grava_objetos_no_git_real() {
    // Controle 1: git de verdade grava objeto, e count-objects enxerga a mudanca.
    let ctrl = novo_amb();
    let ctrl_repo = ctrl.repo();
    let (c0, _) = contagem(&ctrl, &ctrl_repo);
    fs::write(ctrl_repo.join("controle.txt"), "controle\n").unwrap();
    git(&ctrl, &ctrl_repo, &["hash-object", "-w", "controle.txt"]);
    let (c1, _) = contagem(&ctrl, &ctrl_repo);
    assert!(
        c1 > c0,
        "controle: git hash-object -w deveria aumentar count-objects ({c0} -> {c1})"
    );
    // Controle 2: conteudo commitado aparece em objeto (a busca por conteudo discrimina).
    fs::write(ctrl_repo.join("marcador.txt"), format!("{SEGREDO}\n")).unwrap();
    git(&ctrl, &ctrl_repo, &["add", "marcador.txt"]);
    git(&ctrl, &ctrl_repo, &["commit", "-q", "-m", "marcador"]);
    assert!(
        objetos_contem(&ctrl, &ctrl_repo, SEGREDO),
        "controle: conteudo commitado deveria aparecer em algum objeto"
    );

    // Repositorio real do teste: arquivo nao ignorado novo com conteudo unico.
    let amb = novo_amb();
    let repo = amb.repo();
    let st = amb.estado("state");
    fs::write(repo.join("segredo.env"), format!("{SEGREDO}\n")).unwrap();
    let base = contagem(&amb, &repo);
    let ids0 = ids_objetos(&amb, &repo);
    assert!(
        !objetos_contem(&amb, &repo, SEGREDO),
        "A22: conteudo do segredo.env ja estava em objeto antes do attest"
    );

    cod(
        &grava_run(&amb, &st, "gate", "T1", "a1", &["printf", "ok\n"]),
        0,
        "A22: run com arquivo novo nao ignorado deve sair 0",
    );
    let t1 = ultimo(&st)["tree"].clone();
    assert!(t1.is_string(), "A22: registro run sem tree");
    assert_eq!(
        contagem(&amb, &repo),
        base,
        "A22: run gravou objeto no .git real"
    );

    cod(
        &grava_note(&amb, &st, "author", "T1", "autor1", "pass", "autor"),
        0,
        "A22: note deve sair 0",
    );
    assert_eq!(
        contagem(&amb, &repo),
        base,
        "A22: note gravou objeto no .git real"
    );

    cod(
        &check(&amb, &st, &politica(&amb, POLITICA_GATE), "T1"),
        0,
        "A22: check deve sair 0 (gate pass na arvore atual)",
    );
    assert_eq!(
        contagem(&amb, &repo),
        base,
        "A22: check gravou objeto no .git real"
    );

    cod(
        &attest(
            &amb,
            &repo,
            &st,
            &["footer", "--task", "T1", "--tree", "auto"],
        ),
        0,
        "A22: footer deve sair 0 (ha registro pass)",
    );
    assert_eq!(
        contagem(&amb, &repo),
        base,
        "A22: footer gravou objeto no .git real"
    );

    // Controle da arvore: o arquivo nao ignorado entra no calculo do tree mesmo sem objeto gravado.
    fs::write(repo.join("segredo.env"), format!("{SEGREDO}\nmudou\n")).unwrap();
    cod(
        &grava_run(&amb, &st, "gate", "T1", "a1", &["printf", "ok\n"]),
        0,
        "A22: run apos alterar segredo.env deve sair 0",
    );
    let t2 = ultimo(&st)["tree"].clone();
    assert!(t2.is_string(), "A22: registro run sem tree (2)");
    assert_ne!(
        t2, t1,
        "A22: alterar o arquivo nao ignorado deve mudar o tree registrado"
    );
    assert_eq!(
        contagem(&amb, &repo),
        base,
        "A22: a segunda rodada gravou objeto no .git real"
    );

    // Nenhum objeto novo e nenhum objeto com o conteudo unico no .git real.
    assert_eq!(
        ids_objetos(&amb, &repo),
        ids0,
        "A22: a lista de objetos do .git real mudou"
    );
    assert!(
        !objetos_contem(&amb, &repo, SEGREDO),
        "A22: o conteudo unico de segredo.env apareceu em objeto do .git real"
    );
}

/// A23: run registra tree_before e tree (depois), com mutated; check e footer usam tree; note grava so tree.
#[test]
fn a23_tree_antes_e_depois_e_mutated() {
    let amb = novo_amb();
    let repo = amb.repo();
    let st = amb.estado("state");

    // (b) comando que nao altera a arvore: tree_before == tree e mutated false.
    cod(
        &grava_run(&amb, &st, "gate", "T1", "a1", &["printf", "ok\n"]),
        0,
        "A23 (b): run que nao altera deve sair 0",
    );
    let r0 = ultimo(&st);
    assert!(
        r0["tree_before"].is_string(),
        "A23 (b): run sem tree_before: {r0}"
    );
    assert!(r0["tree"].is_string(), "A23 (b): run sem tree: {r0}");
    assert_eq!(
        r0["tree_before"], r0["tree"],
        "A23 (b): comando que nao altera deve gravar tree_before == tree"
    );
    assert_eq!(
        r0["mutated"],
        json!(false),
        "A23 (b): mutated deve ser false: {r0}"
    );
    let t0 = r0["tree"].as_str().expect("tree texto").to_string();

    // (a) comando que altera a.txt: tree_before (antes) = t0, tree (depois) != t0, mutated true.
    cod(
        &grava_run(
            &amb,
            &st,
            "gate",
            "T1",
            "a1",
            &["sh", "-c", "echo x >> a.txt"],
        ),
        0,
        "A23 (a): run que altera deve sair 0 (o comando sai 0)",
    );
    let r1 = ultimo(&st);
    assert!(
        r1["tree_before"].is_string(),
        "A23 (a): run sem tree_before: {r1}"
    );
    assert!(r1["tree"].is_string(), "A23 (a): run sem tree: {r1}");
    assert_eq!(
        r1["result"], "pass",
        "A23 (a): resultado deve ser pass: {r1}"
    );
    assert_eq!(
        r1["tree_before"].as_str(),
        Some(t0.as_str()),
        "A23 (a): tree_before deve ser a arvore antes do comando (t0)"
    );
    assert_ne!(
        r1["tree_before"], r1["tree"],
        "A23 (a): comando que altera deve gravar tree_before != tree"
    );
    assert_eq!(
        r1["mutated"],
        json!(true),
        "A23 (a): mutated deve ser true: {r1}"
    );
    let t1 = r1["tree"].as_str().expect("tree texto").to_string();

    // check usa tree (depois): a arvore atual e t1 e o gate pass gravado com tree = t1 conta como ok.
    // Se o check usasse tree_before (t0 no registro), o gate ficaria missing.
    let c = check(&amb, &st, &politica(&amb, POLITICA_GATE), "T1");
    cod(
        &c,
        0,
        "A23 (a): check --tree auto deve usar tree (depois) e dar ok",
    );
    assert_eq!(
        status_do(&passos(&c), "gate"),
        Some("ok".to_string()),
        "A23 (a): gate deve estar ok na arvore atual"
    );
    // Controle: a arvore mudou de fato (o registro de (b), t0, ja nao corresponde a arvore atual).
    assert_ne!(
        t0, t1,
        "A23: a arvore deveria ter mudado com o comando que altera"
    );

    // (c) note grava tree (= arvore atual, t1) e nao grava tree_before (nem mutated).
    cod(
        &grava_note(&amb, &st, "author", "T1", "autor1", "pass", "autor"),
        0,
        "A23 (c): note deve sair 0",
    );
    let n = ultimo(&st);
    assert_eq!(
        n["kind"], "note",
        "A23 (c): ultimo registro deve ser note: {n}"
    );
    assert_eq!(
        n["tree"].as_str(),
        Some(t1.as_str()),
        "A23 (c): note deve gravar tree da arvore atual"
    );
    assert!(
        n.get("tree_before").is_none(),
        "A23 (c): note nao deve gravar tree_before: {n}"
    );
    assert!(
        n.get("mutated").is_none(),
        "A23 (c): note nao deve gravar mutated (spec: note grava so tree): {n}"
    );

    // (d) conferir os campos no log.jsonl, linha a linha.
    let regs = registros(&st);
    assert_eq!(
        regs.len(),
        3,
        "A23 (d): deveria haver 3 registros: {regs:?}"
    );
    for (i, r) in regs.iter().enumerate() {
        assert_eq!(r["v"], json!(1), "A23 (d): v do registro {}", i + 1);
        assert_eq!(
            r["seq"],
            json!(i as i64 + 1),
            "A23 (d): seq do registro {}",
            i + 1
        );
    }
    for r in regs.iter().filter(|r| r["kind"] == "run") {
        let antes = r["tree_before"].as_str().expect("run com tree_before");
        let depois = r["tree"].as_str().expect("run com tree");
        assert_eq!(
            r["mutated"],
            json!(antes != depois),
            "A23 (d): mutated deve ser (tree_before != tree) no registro {}: {r}",
            r["seq"]
        );
    }
    assert_eq!(
        regs[0]["mutated"],
        json!(false),
        "A23 (d): registro 1 sem mutacao"
    );
    assert_eq!(
        regs[1]["mutated"],
        json!(true),
        "A23 (d): registro 2 com mutacao"
    );
    assert_eq!(regs[2]["kind"], "note", "A23 (d): registro 3 deve ser note");

    // O comando que altera deixou a alteracao na copia de trabalho (o attest nao desfaz nada).
    let diff = git(&amb, &repo, &["status", "--porcelain"]);
    assert!(
        diff.contains("a.txt"),
        "A23: a alteracao do comando deveria continuar na copia de trabalho: {diff}"
    );
}

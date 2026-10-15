//! Instalador `crates/chatd/scripts/instala.sh`: regras 1 a 11 e testes I1 a I11 de docs/spec-instalador.md.
//! Escrito so a partir do contrato; o script nao foi lido. Ele roda com `bash` e um PATH que aponta
//! primeiro para um diretorio temporario com FALSOS `systemctl` e `cargo`; o `git` e o real, num
//! repositorio temporario com um commit. O `chatd` falso e um ELF pequeno compilado com `rustc` na
//! hora: o script confere sha256 de /proc/PID/exe, e um script bash daria /bin/bash como executavel.
//! Todo diretorio e temporario; HOME, CHATD_HOME e TMPDIR apontam para dentro dele. Nada de systemctl
//! real, de rede ou de arquivo fora do temporario.
#![allow(dead_code)]

use std::collections::BTreeMap;
use std::fs::{self, File};
use std::io::Write;
use std::os::unix::fs::{PermissionsExt, symlink};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::thread;
use std::time::{Duration, Instant};
use tempfile::TempDir;

const SEGREDO: &str = "SEGREDO-INSTALA-V1";
const CONFIG_REAL: &str = "{\"token\": \"SEGREDO-INSTALA-V1\"}\n";
const SERVICO: &str = "chatd.service";
static CONTADOR: AtomicUsize = AtomicUsize::new(0);

/// Fonte do `chatd` falso. `serve` imprime `chatd: ready` e fica vivo ate SIGTERM (ou imprime `iniciando`
/// e nunca `ready`, na variante MUDO). Cada chamada e registrada em FAKE_CHATD_LOG; o pid de cada `serve`
/// vai para FAKE_CHATD_PIDS (limpeza); o conteudo do `--config` que existir vai para FAKE_CHATD_SEEN.
const FONTE_CHATD: &str = r#"
use std::io::Write;
use std::time::Duration;

const VERSAO: &str = "__VERSAO__";
const MUDO: bool = __MUDO__;

fn anexa(caminho: &str, texto: &[u8]) {
    if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(caminho) {
        let _ = f.write_all(texto);
    }
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if let Ok(log) = std::env::var("FAKE_CHATD_LOG") {
        anexa(&log, format!("{} {}\n", VERSAO, args.join(" ")).as_bytes());
    }
    if args.is_empty() {
        println!("uso: chatd <comando> [opcoes]");
        return;
    }
    match args[0].as_str() {
        "serve" => {
            if let Ok(pids) = std::env::var("FAKE_CHATD_PIDS") {
                anexa(&pids, format!("{}\n", std::process::id()).as_bytes());
            }
            if let Some(i) = args.iter().position(|a| a == "--config") {
                if let (Some(c), Ok(seen)) = (args.get(i + 1), std::env::var("FAKE_CHATD_SEEN")) {
                    if let Ok(conteudo) = std::fs::read(c) {
                        anexa(&seen, &conteudo);
                    }
                }
            }
            if MUDO {
                println!("iniciando");
            } else {
                println!("chatd: ready");
            }
            let _ = std::io::stdout().flush();
            loop {
                std::thread::sleep(Duration::from_millis(100));
            }
        }
        "log" => println!("[{}] registro de teste", VERSAO),
        _ => {
            eprintln!("comando desconhecido: {}", args[0]);
            std::process::exit(2);
        }
    }
}
"#;

/// FALSO systemctl: registra a chamada; `restart` sobe o `$FAKE_EXEC` como daemon do servico (pid em
/// `$FAKE_SVC_DIR/NOME.pid`, contagem em `NOME.restarts`). `FAKE_FALHA_PRIMEIRO_RESTART` faz o primeiro
/// restart nao subir o processo. `FAKE_SEM_UNIT` marca um nome como inexistente.
const SYSTEMCTL_FALSO: &str = r##"#!/bin/bash
echo "systemctl $*" >> "$FAKE_SVC_LOG"
dir="$FAKE_SVC_DIR"
acao=""
for arg in "$@"; do
  case "$arg" in
    -*) ;;
    *) acao="$arg"; break ;;
  esac
done
nome="${@: -1}"
pidf="$dir/$nome.pid"
rf="$dir/$nome.restarts"
existe() { [ "$nome" != "${FAKE_SEM_UNIT:-}" ]; }
pid_vivo() {
  local p
  p=$(cat "$pidf" 2>/dev/null)
  p=${p:-0}
  [ "$p" -gt 0 ] 2>/dev/null || return 1
  [ -r "/proc/$p/stat" ] || return 1
  if grep -q '^[0-9]* ([^)]*) Z' "/proc/$p/stat"; then return 1; fi
  return 0
}
case "$acao" in
  restart)
    if ! existe; then
      echo "Failed to restart $nome: Unit $nome not found." >&2
      exit 5
    fi
    if pid_vivo; then kill -TERM "$(cat "$pidf")" 2>/dev/null; fi
    echo 0 > "$pidf"
    n=$(( $(cat "$rf" 2>/dev/null || echo 0) + 1 ))
    echo "$n" > "$rf"
    if [ "$n" -eq 1 ] && [ -n "${FAKE_FALHA_PRIMEIRO_RESTART:-}" ]; then exit 0; fi
    "$FAKE_EXEC" serve --state-dir "$FAKE_UNIT_STATE" --config "$FAKE_UNIT_CONFIG" --socket "$FAKE_UNIT_SOCK" >/dev/null 2>&1 </dev/null &
    novo=$!
    for _ in $(seq 100); do
      [ "$(readlink "/proc/$novo/exe" 2>/dev/null)" = "$(readlink -f "$FAKE_EXEC")" ] && break
      sleep 0.02
    done
    echo "$novo" > "$pidf"
    exit 0 ;;
  is-active)
    if existe && pid_vivo; then echo active; exit 0; fi
    echo inactive
    exit 3 ;;
  show)
    case "$*" in
      *MainPID*) if pid_vivo; then cat "$pidf"; else echo 0; fi ;;
      *ActiveState*) if pid_vivo; then echo active; else echo inactive; fi ;;
      *LoadState*) if existe; then echo loaded; else echo not-found; fi ;;
    esac
    exit 0 ;;
  *)
    if ! existe; then echo "Unit $nome not found." >&2; exit 1; fi
    exit 0 ;;
esac
"##;

/// FALSO cargo: registra a chamada (com o cwd) e copia `$FAKE_CARGO_SRC` para `$FAKE_CARGO_DEST`.
const CARGO_FALSO: &str = r##"#!/bin/bash
echo "cargo $* cwd=$PWD" >> "$FAKE_CARGO_LOG"
mkdir -p "$(dirname "$FAKE_CARGO_DEST")" || exit 101
cp "$FAKE_CARGO_SRC" "$FAKE_CARGO_DEST" && chmod 0755 "$FAKE_CARGO_DEST"
"##;

#[derive(Debug)]
struct Saida {
    code: Option<i32>,
    stdout: String,
    stderr: String,
}

impl Saida {
    fn tudo(&self) -> String {
        format!("{}{}", self.stdout, self.stderr)
    }
}

/// O script real, so para checar que existe (nunca lido aqui). Falha com mensagem clara se ausente.
fn script_real() -> PathBuf {
    let p = Path::new(env!("CARGO_MANIFEST_DIR")).join("scripts/instala.sh");
    assert!(
        p.is_file(),
        "script ausente: {} (a implementacao ainda nao existe; estes testes falham ate la)",
        p.display()
    );
    p
}

fn s(p: &Path) -> &str {
    p.to_str().expect("caminho UTF-8")
}

fn modo(p: &Path) -> u32 {
    fs::metadata(p).unwrap().permissions().mode() & 0o7777
}

fn escreve_exec(p: &Path, conteudo: &str) {
    fs::write(p, conteudo).unwrap();
    fs::set_permissions(p, fs::Permissions::from_mode(0o755)).unwrap();
}

fn copia_exec(origem: &Path, destino: &Path) {
    fs::create_dir_all(destino.parent().unwrap()).unwrap();
    fs::copy(origem, destino).unwrap();
    fs::set_permissions(destino, fs::Permissions::from_mode(0o755)).unwrap();
}

/// Compila um `chatd` falso (ELF real) em `base/bins/NOME/chatd`.
fn compila(base: &Path, nome: &str, versao: &str, mudo: bool) -> PathBuf {
    let dir = base.join("bins").join(nome);
    fs::create_dir_all(&dir).unwrap();
    let fonte = dir.join("chatd.rs");
    let codigo = FONTE_CHATD
        .replace("__VERSAO__", versao)
        .replace("__MUDO__", if mudo { "true" } else { "false" });
    fs::write(&fonte, codigo).unwrap();
    let bin = dir.join("chatd");
    let rustc = std::env::var("RUSTC").unwrap_or_else(|_| "rustc".to_string());
    let out = Command::new(rustc)
        .args(["--edition", "2021", "-o"])
        .arg(&bin)
        .arg(&fonte)
        .output()
        .expect("rustc");
    assert!(
        out.status.success(),
        "rustc do chatd falso: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    bin
}

/// Conteudo de um diretorio: arquivo -> (bytes, modo); symlink -> (alvo, 0o120000).
fn arvore(dir: &Path) -> BTreeMap<String, (Vec<u8>, u32)> {
    fn anda(base: &Path, atual: &Path, m: &mut BTreeMap<String, (Vec<u8>, u32)>) {
        let Ok(rd) = fs::read_dir(atual) else {
            return;
        };
        for e in rd.flatten() {
            let p = e.path();
            let rel = p.strip_prefix(base).unwrap().to_string_lossy().into_owned();
            let md = fs::symlink_metadata(&p).unwrap();
            if md.file_type().is_symlink() {
                let alvo = fs::read_link(&p).unwrap().to_string_lossy().into_owned();
                m.insert(rel, (alvo.into_bytes(), 0o120000));
            } else if md.is_dir() {
                anda(base, &p, m);
            } else {
                m.insert(
                    rel,
                    (fs::read(&p).unwrap(), md.permissions().mode() & 0o7777),
                );
            }
        }
    }
    let mut m = BTreeMap::new();
    anda(dir, dir, &mut m);
    m
}

/// Nome no padrao `chatd.AAAAMMDDTHHMMSSZ.bak` (regra 7).
fn e_backup_padrao(nome: &str) -> bool {
    let Some(meio) = nome
        .strip_prefix("chatd.")
        .and_then(|r| r.strip_suffix(".bak"))
    else {
        return false;
    };
    let b = meio.as_bytes();
    b.len() == 16
        && b[8] == b'T'
        && b[15] == b'Z'
        && b.iter()
            .enumerate()
            .all(|(i, c)| i == 8 || i == 15 || c.is_ascii_digit())
}

fn backups_em(dir: &Path) -> Vec<String> {
    let mut v: Vec<String> = fs::read_dir(dir)
        .map(|rd| {
            rd.flatten()
                .map(|e| e.file_name().to_string_lossy().into_owned())
                .filter(|n| e_backup_padrao(n))
                .collect()
        })
        .unwrap_or_default();
    v.sort();
    v
}

/// Ambiente temporario: repo git, fakes no PATH, prefixo de instalacao, CHATD_HOME, unidade do servico.
struct Amb {
    tmp: TempDir,
    path_orig: String,
    raiz: PathBuf,
    home: PathBuf,
    chatd_home: PathBuf,
    tmpdir: PathBuf,
    fakebin: PathBuf,
    svc: PathBuf,
    unidade: PathBuf,
    prefix: PathBuf,
    velho: PathBuf,
    novo: PathBuf,
    mudo: PathBuf,
    filhos: Vec<Child>,
}

impl Amb {
    fn novo() -> Amb {
        script_real();
        let tmp = tempfile::Builder::new()
            .prefix("instala-")
            .tempdir()
            .expect("tempdir");
        let b = tmp.path().to_path_buf();
        let velho = compila(&b, "velho", "v1", false);
        let novo = compila(&b, "novo", "v2", false);
        let mudo = compila(&b, "mudo", "v3", true);
        let a = Amb {
            raiz: b.join("repo"),
            home: b.join("home"),
            chatd_home: b.join("home/chatd"),
            tmpdir: b.join("tmpdir"),
            fakebin: b.join("fakebin"),
            svc: b.join("svc"),
            unidade: b.join("unidade"),
            prefix: b.join("prefix/bin"),
            velho,
            novo,
            mudo,
            path_orig: std::env::var("PATH").unwrap_or_default(),
            filhos: Vec::new(),
            tmp,
        };
        let state = a.chatd_home.join("state");
        for d in [
            &a.raiz, &a.home, &state, &a.tmpdir, &a.fakebin, &a.svc, &a.unidade,
        ] {
            fs::create_dir_all(d).unwrap();
        }
        fs::write(a.chatd_home.join("config.json"), CONFIG_REAL).unwrap();
        fs::write(
            a.chatd_home.join("tokens.json"),
            format!("{{\"jabbur\": \"{SEGREDO}\"}}\n"),
        )
        .unwrap();
        fs::write(state.join("estado.json"), "{\"seq\": 7}\n").unwrap();
        escreve_exec(&a.fakebin.join("systemctl"), SYSTEMCTL_FALSO);
        escreve_exec(&a.fakebin.join("cargo"), CARGO_FALSO);
        a
    }

    fn arq(&self, nome: &str) -> PathBuf {
        self.tmp.path().join(nome)
    }

    fn cargo_dest(&self) -> PathBuf {
        self.raiz.join("target/release/chatd")
    }

    fn cargo_log(&self) -> String {
        fs::read_to_string(self.arq("cargo.log")).unwrap_or_default()
    }

    fn chatd_log(&self) -> String {
        fs::read_to_string(self.arq("chatd.log")).unwrap_or_default()
    }

    fn git(&self, args: &[&str]) -> String {
        let out = Command::new("git")
            .args(args)
            .current_dir(&self.raiz)
            .env_clear()
            .env("PATH", &self.path_orig)
            .env("HOME", &self.home)
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("LC_ALL", "C")
            .output()
            .expect("git");
        assert!(
            out.status.success(),
            "git {args:?}: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        String::from_utf8_lossy(&out.stdout).trim().to_string()
    }

    fn head(&self) -> String {
        self.git(&["rev-parse", "HEAD"])
    }

    /// Repositorio git temporario com um commit. O script fica como symlink rastreado em
    /// crates/chatd/scripts/instala.sh, para que a raiz detectada a partir dele seja este repo.
    fn repo_git(&self) {
        let scripts = self.raiz.join("crates/chatd/scripts");
        fs::create_dir_all(&scripts).unwrap();
        symlink(script_real(), scripts.join("instala.sh")).unwrap();
        fs::write(self.raiz.join("README.md"), "teste\n").unwrap();
        fs::write(self.raiz.join(".gitignore"), "target/\n").unwrap();
        self.git(&["init", "-q"]);
        self.git(&["add", "-A"]);
        self.git(&[
            "-c",
            "user.name=teste",
            "-c",
            "user.email=teste@exemplo.invalid",
            "commit",
            "-q",
            "-m",
            "base",
        ]);
    }

    /// Coloca um binario como o instalado em `prefix/chatd`.
    fn instala_bin(&self, origem: &Path) {
        copia_exec(origem, &self.prefix.join("chatd"));
    }

    /// Sobe um daemon `serve` (ja rodando antes do script) e registra o pid como MainPID da unidade.
    fn sobe_servico(&mut self, unidade: &str, bin: &Path) {
        let filho = Command::new(bin)
            .args([
                "serve",
                "--state-dir",
                s(&self.unidade.join("state")),
                "--config",
                s(&self.unidade.join("config.json")),
                "--socket",
                s(&self.unidade.join("s.sock")),
            ])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .expect("sobe servico");
        fs::write(
            self.svc.join(format!("{unidade}.pid")),
            filho.id().to_string(),
        )
        .unwrap();
        self.filhos.push(filho);
    }

    fn pid_servico(&self, unidade: &str) -> u32 {
        fs::read_to_string(self.svc.join(format!("{unidade}.pid")))
            .unwrap()
            .trim()
            .parse()
            .unwrap()
    }

    /// Bytes do binario que o processo do servico esta rodando (/proc/PID/exe).
    fn exe_servico(&self, unidade: &str) -> Vec<u8> {
        let pid = self.pid_servico(unidade);
        fs::read(format!("/proc/{pid}/exe")).unwrap_or_else(|e| panic!("/proc/{pid}/exe: {e}"))
    }

    fn reinicios(&self, unidade: &str) -> usize {
        fs::read_to_string(self.svc.join(format!("{unidade}.restarts")))
            .ok()
            .and_then(|t| t.trim().parse().ok())
            .unwrap_or(0)
    }

    fn prefix_bytes(&self) -> Vec<u8> {
        fs::read(self.prefix.join("chatd")).unwrap()
    }

    fn backups(&self) -> Vec<String> {
        backups_em(&self.prefix)
    }

    fn roda(&self, args: &[&str]) -> Saida {
        self.roda_com(args, &[], None)
    }

    /// Roda `bash instala.sh ARGS` com o ambiente controlado. Saida vai para arquivos (nao para pipes),
    /// para que um processo orfao nao segure a execucao.
    fn roda_com(&self, args: &[&str], extra: &[(&str, &str)], entrada: Option<&str>) -> Saida {
        let script = self.raiz.join("crates/chatd/scripts/instala.sh");
        let n = CONTADOR.fetch_add(1, Ordering::SeqCst);
        let sout = self.arq(&format!("out-{n}.txt"));
        let serr = self.arq(&format!("err-{n}.txt"));
        let mut cmd = Command::new("bash");
        cmd.arg(&script)
            .args(args)
            .current_dir(&self.raiz)
            .env_clear()
            .env(
                "PATH",
                format!("{}:{}", self.fakebin.display(), self.path_orig),
            )
            .env("HOME", &self.home)
            .env("CHATD_HOME", &self.chatd_home)
            .env("XDG_CONFIG_HOME", self.home.join(".config"))
            .env("XDG_STATE_HOME", self.home.join(".local/state"))
            .env("TMPDIR", &self.tmpdir)
            .env("LC_ALL", "C")
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("FAKE_SVC_DIR", &self.svc)
            .env("FAKE_SVC_LOG", self.arq("svc.log"))
            .env("FAKE_CARGO_LOG", self.arq("cargo.log"))
            .env("FAKE_CARGO_DEST", self.cargo_dest())
            .env("FAKE_CARGO_SRC", &self.novo)
            .env("FAKE_EXEC", self.prefix.join("chatd"))
            .env("FAKE_UNIT_STATE", self.unidade.join("state"))
            .env("FAKE_UNIT_CONFIG", self.unidade.join("config.json"))
            .env("FAKE_UNIT_SOCK", self.unidade.join("s.sock"))
            .env("FAKE_CHATD_LOG", self.arq("chatd.log"))
            .env("FAKE_CHATD_PIDS", self.arq("pids.txt"))
            .env("FAKE_CHATD_SEEN", self.arq("seen.bin"))
            .stdin(if entrada.is_some() {
                Stdio::piped()
            } else {
                Stdio::null()
            })
            .stdout(File::create(&sout).expect("arquivo de saida"))
            .stderr(File::create(&serr).expect("arquivo de erro"));
        for (k, v) in extra {
            cmd.env(k, v);
        }
        let mut filho = cmd.spawn().expect("bash");
        if let Some(texto) = entrada {
            if let Some(mut bruto) = filho.stdin.take() {
                let _ = bruto.write_all(texto.as_bytes());
            }
        }
        let inicio = Instant::now();
        let status = loop {
            if let Some(st) = filho.try_wait().expect("try_wait") {
                break st;
            }
            if inicio.elapsed() > Duration::from_secs(120) {
                let _ = filho.kill();
                panic!("instala.sh passou de 120 s");
            }
            thread::sleep(Duration::from_millis(50));
        };
        Saida {
            code: status.code(),
            stdout: fs::read_to_string(&sout).unwrap_or_default(),
            stderr: fs::read_to_string(&serr).unwrap_or_default(),
        }
    }
}

impl Drop for Amb {
    fn drop(&mut self) {
        for f in self.filhos.iter_mut() {
            let _ = f.kill();
            let _ = f.wait();
        }
        let mut pids: Vec<i32> = Vec::new();
        if let Ok(t) = fs::read_to_string(self.arq("pids.txt")) {
            pids.extend(t.lines().filter_map(|l| l.trim().parse::<i32>().ok()));
        }
        if let Ok(rd) = fs::read_dir(&self.svc) {
            for e in rd.flatten() {
                if let Ok(t) = fs::read_to_string(e.path()) {
                    pids.extend(t.trim().parse::<i32>().ok());
                }
            }
        }
        for p in pids {
            if p <= 0 {
                continue;
            }
            // So mata processo que roda binario deste temporario (um pid antigo pode ter sido reusado).
            let exe = fs::read_link(format!("/proc/{p}/exe")).unwrap_or_default();
            if exe.starts_with(self.tmp.path()) {
                unsafe {
                    libc::kill(p, libc::SIGKILL);
                }
            }
        }
    }
}

// ---------- Interface e pre-condicoes ----------

#[test]
fn i1_dry_run_nao_cria_nem_altera_nada_e_imprime_plano() {
    let mut a = Amb::novo();
    a.repo_git();
    let velho = a.velho.clone();
    a.instala_bin(&velho);
    a.sobe_servico(SERVICO, &velho);
    let antes_prefix = arvore(&a.prefix);
    let antes_home = arvore(&a.chatd_home);
    let r = a.roda(&["--dry-run", "--prefix", s(&a.prefix), "--wait", "1"]);
    assert_eq!(r.code, Some(0), "saida: {r:?}");
    assert!(!r.stdout.trim().is_empty(), "plano vazio: {r:?}");
    assert!(
        r.stdout.contains(s(&a.prefix)),
        "plano nao menciona o --prefix: {}",
        r.stdout
    );
    assert_eq!(
        arvore(&a.prefix),
        antes_prefix,
        "--dry-run alterou o --prefix"
    );
    assert_eq!(
        arvore(&a.chatd_home),
        antes_home,
        "--dry-run alterou CHATD_HOME"
    );
    assert_eq!(
        a.cargo_log(),
        "",
        "--dry-run nao pode chamar cargo (nem build)"
    );
    assert_eq!(
        a.reinicios(SERVICO),
        0,
        "--dry-run nao pode reiniciar o servico"
    );
}

#[test]
fn i2_instalacao_feliz_troca_o_binario_guarda_backup_e_reinicia_uma_vez() {
    let mut a = Amb::novo();
    a.repo_git();
    let head = a.head();
    let velho = a.velho.clone();
    let novo = a.novo.clone();
    a.instala_bin(&velho);
    a.sobe_servico(SERVICO, &velho);
    let bytes_velho = fs::read(&velho).unwrap();
    let bytes_novo = fs::read(&novo).unwrap();
    let r = a.roda(&["--yes", "--prefix", s(&a.prefix), "--wait", "1"]);
    assert_eq!(r.code, Some(0), "saida: {r:?}");
    assert!(
        r.tudo().contains(&head[..7]),
        "relatorio sem o commit {}: {}",
        &head[..7],
        r.tudo()
    );
    assert_eq!(
        a.prefix_bytes(),
        bytes_novo,
        "binario instalado nao e o novo"
    );
    assert_eq!(modo(&a.prefix.join("chatd")), 0o755, "modo do binario novo");
    let bk = a.backups();
    assert_eq!(bk.len(), 1, "backups: {bk:?}");
    let caminho = a.prefix.join(&bk[0]);
    assert_eq!(
        fs::read(&caminho).unwrap(),
        bytes_velho,
        "backup nao e o binario antigo byte a byte"
    );
    assert_eq!(modo(&caminho), 0o755, "modo do backup");
    assert_eq!(a.reinicios(SERVICO), 1, "servico deve reiniciar uma vez");
    assert_eq!(
        a.exe_servico(SERVICO),
        bytes_novo,
        "o processo do servico nao roda o binario novo"
    );
    let sobras: Vec<String> = fs::read_dir(&a.prefix)
        .unwrap()
        .flatten()
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .filter(|n| n.starts_with("chatd.new."))
        .collect();
    assert!(
        sobras.is_empty(),
        "temporario chatd.new.PID sobrou: {sobras:?}"
    );
    assert_eq!(
        fs::read_dir(&a.tmpdir).unwrap().count(),
        0,
        "o temporario do teste de fumaca nao foi removido"
    );
}

#[test]
fn i3_mesmo_sha_ja_instalado_sem_backup_nem_reinicio() {
    let mut a = Amb::novo();
    a.repo_git();
    let novo = a.novo.clone();
    a.instala_bin(&novo);
    a.sobe_servico(SERVICO, &novo);
    let r = a.roda(&["--yes", "--prefix", s(&a.prefix), "--wait", "1"]);
    assert_eq!(r.code, Some(0), "saida: {r:?}");
    assert!(
        r.tudo().contains("instalado"),
        "sem 'ja instalado': {}",
        r.tudo()
    );
    assert!(a.backups().is_empty(), "nao deveria criar backup");
    assert_eq!(a.reinicios(SERVICO), 0, "nao deveria reiniciar");
    assert_eq!(a.prefix_bytes(), fs::read(&novo).unwrap());
}

#[test]
fn i3b_processo_desatualizado_com_binario_igual_so_reinicia() {
    let mut a = Amb::novo();
    a.repo_git();
    let velho = a.velho.clone();
    let novo = a.novo.clone();
    a.instala_bin(&novo);
    a.sobe_servico(SERVICO, &velho);
    let bytes_novo = fs::read(&novo).unwrap();
    let r = a.roda(&["--yes", "--prefix", s(&a.prefix), "--wait", "1"]);
    assert_eq!(r.code, Some(0), "saida: {r:?}");
    assert!(
        a.backups().is_empty(),
        "processo desatualizado nao pode copiar nem criar backup"
    );
    assert_eq!(a.prefix_bytes(), bytes_novo, "o binario instalado mudou");
    assert_eq!(a.reinicios(SERVICO), 1, "deveria reiniciar uma vez");
    assert_eq!(
        a.exe_servico(SERVICO),
        bytes_novo,
        "servico nao passou ao binario novo"
    );
}

#[test]
fn i4_servico_nao_fica_ativo_reverte_binario_e_reinicia_de_novo() {
    let mut a = Amb::novo();
    a.repo_git();
    let velho = a.velho.clone();
    let bytes_velho = fs::read(&velho).unwrap();
    a.instala_bin(&velho);
    a.sobe_servico(SERVICO, &velho);
    let r = a.roda_com(
        &["--yes", "--prefix", s(&a.prefix), "--wait", "1"],
        &[("FAKE_FALHA_PRIMEIRO_RESTART", "1")],
        None,
    );
    assert_eq!(r.code, Some(1), "saida: {r:?}");
    assert_eq!(
        a.prefix_bytes(),
        bytes_velho,
        "o binario antigo nao voltou byte a byte"
    );
    assert_eq!(
        a.reinicios(SERVICO),
        2,
        "esperava o restart que falhou e o restart da reversao"
    );
    assert_eq!(
        a.exe_servico(SERVICO),
        bytes_velho,
        "o servico nao voltou a rodar o binario antigo"
    );
    assert!(
        r.tudo()
            .contains("journalctl --user -u chatd.service -n 30 --no-pager"),
        "faltou o comando de diagnostico: {}",
        r.tudo()
    );
}

#[test]
fn i5_arvore_suja_recusa_sem_allow_dirty_e_aceita_com_ele() {
    let mut a = Amb::novo();
    a.repo_git();
    let velho = a.velho.clone();
    let novo = a.novo.clone();
    a.instala_bin(&velho);
    a.sobe_servico(SERVICO, &velho);
    fs::write(a.raiz.join("README.md"), "teste\nalterado\n").unwrap();
    let r1 = a.roda(&["--yes", "--prefix", s(&a.prefix), "--wait", "1"]);
    assert_eq!(r1.code, Some(2), "arvore suja deveria recusar: {r1:?}");
    assert_eq!(
        a.prefix_bytes(),
        fs::read(&velho).unwrap(),
        "recusa alterou o binario"
    );
    assert_eq!(a.reinicios(SERVICO), 0, "recusa reiniciou o servico");
    assert!(a.backups().is_empty(), "recusa criou backup");
    let r2 = a.roda(&[
        "--yes",
        "--allow-dirty",
        "--prefix",
        s(&a.prefix),
        "--wait",
        "1",
    ]);
    assert_eq!(r2.code, Some(0), "--allow-dirty deveria aceitar: {r2:?}");
    assert_eq!(a.prefix_bytes(), fs::read(&novo).unwrap());
}

#[test]
fn i6_keep_poda_os_mais_antigos_e_so_os_do_padrao() {
    let mut a = Amb::novo();
    a.repo_git();
    let velho = a.velho.clone();
    a.instala_bin(&velho);
    a.sobe_servico(SERVICO, &velho);
    for (nome, texto) in [
        ("chatd.20000101T000000Z.bak", "antigo-1"),
        ("chatd.20000102T000000Z.bak", "antigo-2"),
        ("chatd.20000103T000000Z.bak", "antigo-3"),
    ] {
        fs::write(a.prefix.join(nome), texto).unwrap();
    }
    fs::write(a.prefix.join("chatd.notas.txt"), "nao e backup").unwrap();
    fs::write(a.prefix.join("notas.bak"), "outro").unwrap();
    let r = a.roda(&[
        "--yes",
        "--keep",
        "2",
        "--prefix",
        s(&a.prefix),
        "--wait",
        "1",
    ]);
    assert_eq!(r.code, Some(0), "saida: {r:?}");
    let bk = a.backups();
    assert_eq!(bk.len(), 2, "--keep 2 deve deixar 2 backups: {bk:?}");
    let tem = |n: &str| bk.iter().any(|x| x == n);
    assert!(
        !tem("chatd.20000101T000000Z.bak"),
        "backup mais antigo nao foi podado"
    );
    assert!(
        !tem("chatd.20000102T000000Z.bak"),
        "segundo backup mais antigo nao foi podado"
    );
    assert!(
        tem("chatd.20000103T000000Z.bak"),
        "o backup mais novo antigo sumiu: {bk:?}"
    );
    let novo_bk = bk
        .iter()
        .find(|n| !n.starts_with("chatd.2000"))
        .expect("backup desta execucao");
    assert_eq!(
        fs::read(a.prefix.join(novo_bk)).unwrap(),
        fs::read(&velho).unwrap(),
        "backup novo nao e o binario antigo"
    );
    assert_eq!(
        fs::read_to_string(a.prefix.join("chatd.notas.txt")).unwrap(),
        "nao e backup"
    );
    assert_eq!(
        fs::read_to_string(a.prefix.join("notas.bak")).unwrap(),
        "outro"
    );
}

#[test]
fn i7_chatd_home_intocado_token_fora_da_saida_e_fumaca_com_copia_da_config() {
    let mut a = Amb::novo();
    a.repo_git();
    let velho = a.velho.clone();
    a.instala_bin(&velho);
    a.sobe_servico(SERVICO, &velho);
    let antes = arvore(&a.chatd_home);
    let config_bytes = fs::read(a.chatd_home.join("config.json")).unwrap();
    let r = a.roda(&["--yes", "--prefix", s(&a.prefix), "--wait", "1"]);
    assert_eq!(r.code, Some(0), "saida: {r:?}");
    assert_eq!(arvore(&a.chatd_home), antes, "CHATD_HOME foi alterado");
    assert!(
        !r.tudo().contains(SEGREDO),
        "o token apareceu na saida: {}",
        r.tudo()
    );
    assert_eq!(
        fs::read(a.arq("seen.bin")).unwrap_or_default(),
        config_bytes,
        "o teste de fumaca nao subiu com a copia da config.json real"
    );
    let cfg_real = s(&a.chatd_home.join("config.json")).to_string();
    let home_str = s(&a.chatd_home).to_string();
    let mut viu_serve = false;
    for linha in a.chatd_log().lines() {
        let v: Vec<&str> = linha.split_whitespace().collect();
        if v.get(1) != Some(&"serve") {
            continue;
        }
        viu_serve = true;
        if let Some(i) = v.iter().position(|x| *x == "--config") {
            assert_ne!(
                v.get(i + 1).copied(),
                Some(cfg_real.as_str()),
                "serve usou o config.json do CHATD_HOME: {linha}"
            );
        }
        if let Some(i) = v.iter().position(|x| *x == "--state-dir") {
            assert!(
                !v.get(i + 1)
                    .copied()
                    .unwrap_or("")
                    .starts_with(home_str.as_str()),
                "serve usou o estado do CHATD_HOME: {linha}"
            );
        }
    }
    assert!(viu_serve, "nenhum serve foi chamado: {:?}", a.chatd_log());
}

#[test]
fn i8_no_restart_nao_chama_restart_e_sai_3() {
    let mut a = Amb::novo();
    a.repo_git();
    let velho = a.velho.clone();
    let novo = a.novo.clone();
    a.instala_bin(&velho);
    a.sobe_servico(SERVICO, &velho);
    let r = a.roda(&[
        "--yes",
        "--no-restart",
        "--prefix",
        s(&a.prefix),
        "--wait",
        "1",
    ]);
    assert_eq!(r.code, Some(3), "saida: {r:?}");
    assert_eq!(a.reinicios(SERVICO), 0, "--no-restart chamou restart");
    assert_eq!(
        a.prefix_bytes(),
        fs::read(&novo).unwrap(),
        "o binario deveria estar instalado"
    );
    let bk = a.backups();
    assert_eq!(bk.len(), 1, "backup: {bk:?}");
    assert_eq!(
        fs::read(a.prefix.join(&bk[0])).unwrap(),
        fs::read(&velho).unwrap()
    );
}

#[test]
fn i9_prefix_e_service_customizados_sem_tocar_o_padrao() {
    let a = Amb::novo();
    a.repo_git();
    let velho = a.velho.clone();
    let novo = a.novo.clone();
    let custom = a.tmp.path().join("custom/bin");
    copia_exec(&velho, &custom.join("chatd"));
    let exec = custom.join("chatd");
    let r = a.roda_com(
        &[
            "--yes",
            "--prefix",
            s(&custom),
            "--service",
            "chatd-teste.service",
            "--wait",
            "1",
        ],
        &[("FAKE_EXEC", s(&exec))],
        None,
    );
    assert_eq!(r.code, Some(0), "saida: {r:?}");
    assert_eq!(
        fs::read(&exec).unwrap(),
        fs::read(&novo).unwrap(),
        "--prefix customizado"
    );
    assert_eq!(
        backups_em(&custom).len(),
        1,
        "backup no --prefix customizado"
    );
    assert_eq!(
        a.reinicios("chatd-teste.service"),
        1,
        "--service customizado"
    );
    assert_eq!(
        a.reinicios(SERVICO),
        0,
        "o servico padrao nao deveria ser tocado"
    );
    assert!(
        !a.home.join(".local").exists(),
        "o prefixo padrao HOME/.local/bin foi usado"
    );
    assert_eq!(
        a.exe_servico("chatd-teste.service"),
        fs::read(&novo).unwrap()
    );
}

#[test]
fn i10_fumaca_que_nao_escreve_chatd_ready_sai_2_sem_alterar() {
    let mut a = Amb::novo();
    a.repo_git();
    let velho = a.velho.clone();
    let mudo = a.mudo.clone();
    a.instala_bin(&velho);
    a.sobe_servico(SERVICO, &velho);
    let r = a.roda_com(
        &["--yes", "--prefix", s(&a.prefix), "--wait", "1"],
        &[("FAKE_CARGO_SRC", s(&mudo))],
        None,
    );
    assert_eq!(r.code, Some(2), "fumaca falha deveria sair 2: {r:?}");
    assert_eq!(
        a.prefix_bytes(),
        fs::read(&velho).unwrap(),
        "fumaca falha alterou o binario"
    );
    assert!(a.backups().is_empty(), "fumaca falha criou backup");
    assert_eq!(a.reinicios(SERVICO), 0, "fumaca falha reiniciou o servico");
}

#[test]
fn i11_bash_n_passa_e_shellcheck_sem_avisos_quando_existir() {
    let script = script_real();
    let sintaxe = Command::new("bash")
        .arg("-n")
        .arg(&script)
        .output()
        .expect("bash -n");
    assert!(
        sintaxe.status.success(),
        "bash -n falhou: {}",
        String::from_utf8_lossy(&sintaxe.stderr)
    );
    match Command::new("shellcheck").arg(&script).output() {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            eprintln!("shellcheck ausente: etapa pulada");
        }
        Err(e) => panic!("shellcheck: {e}"),
        Ok(o) => assert!(
            o.status.success() && o.stdout.is_empty() && o.stderr.is_empty(),
            "shellcheck: {}{}",
            String::from_utf8_lossy(&o.stdout),
            String::from_utf8_lossy(&o.stderr)
        ),
    }
}

// ---------- Regras sem teste I-numerado (regras 2, 3, 4 e 10) ----------

#[test]
fn regra2_servico_inexistente_sai_2_sem_alterar_nada() {
    let mut a = Amb::novo();
    a.repo_git();
    let velho = a.velho.clone();
    a.instala_bin(&velho);
    a.sobe_servico(SERVICO, &velho);
    let r = a.roda_com(
        &["--yes", "--prefix", s(&a.prefix), "--wait", "1"],
        &[("FAKE_SEM_UNIT", SERVICO)],
        None,
    );
    assert_eq!(r.code, Some(2), "servico inexistente deveria sair 2: {r:?}");
    assert_eq!(a.prefix_bytes(), fs::read(&velho).unwrap());
    assert_eq!(a.reinicios(SERVICO), 0);
    assert!(a.backups().is_empty());
}

#[test]
fn regra3_sem_confirmacao_padrao_nao_altera_nada() {
    let mut a = Amb::novo();
    a.repo_git();
    let velho = a.velho.clone();
    a.instala_bin(&velho);
    a.sobe_servico(SERVICO, &velho);
    let _ = a.roda_com(&["--prefix", s(&a.prefix), "--wait", "1"], &[], Some("n\n"));
    assert_eq!(
        a.prefix_bytes(),
        fs::read(&velho).unwrap(),
        "resposta 'n' alterou o binario"
    );
    assert_eq!(a.reinicios(SERVICO), 0, "resposta 'n' reiniciou o servico");
    assert!(a.backups().is_empty(), "resposta 'n' criou backup");
}

#[test]
fn regra4_build_roda_na_raiz_do_repo_e_repassa_offline() {
    let mut a = Amb::novo();
    a.repo_git();
    let velho = a.velho.clone();
    a.instala_bin(&velho);
    a.sobe_servico(SERVICO, &velho);
    let r = a.roda(&[
        "--yes",
        "--offline",
        "--prefix",
        s(&a.prefix),
        "--wait",
        "1",
    ]);
    assert_eq!(r.code, Some(0), "saida: {r:?}");
    let log = a.cargo_log();
    let linha = log.lines().next().unwrap_or_default().to_string();
    assert!(
        linha.contains("build") && linha.contains("--release") && linha.contains("-p chatd"),
        "cargo sem 'build --release -p chatd': {log:?}"
    );
    assert!(
        linha.contains("--offline"),
        "--offline nao foi repassado: {linha}"
    );
    let raiz = fs::canonicalize(&a.raiz).unwrap();
    assert!(
        linha.contains(&format!("cwd={}", raiz.display())),
        "cargo nao rodou na raiz do repositorio: {linha}"
    );
}

#[test]
fn regra4_no_build_usa_o_binario_ja_compilado() {
    let mut a = Amb::novo();
    a.repo_git();
    let velho = a.velho.clone();
    let novo = a.novo.clone();
    a.instala_bin(&velho);
    a.sobe_servico(SERVICO, &velho);
    copia_exec(&novo, &a.cargo_dest());
    let r = a.roda(&[
        "--yes",
        "--no-build",
        "--prefix",
        s(&a.prefix),
        "--wait",
        "1",
    ]);
    assert_eq!(r.code, Some(0), "saida: {r:?}");
    assert_eq!(a.cargo_log(), "", "--no-build chamou cargo");
    assert_eq!(a.prefix_bytes(), fs::read(&novo).unwrap());
    assert_eq!(a.reinicios(SERVICO), 1);
}

#[test]
fn regra10_relatorio_traz_backup_e_comando_de_reversao() {
    let mut a = Amb::novo();
    a.repo_git();
    let velho = a.velho.clone();
    a.instala_bin(&velho);
    a.sobe_servico(SERVICO, &velho);
    let r = a.roda(&["--yes", "--prefix", s(&a.prefix), "--wait", "1"]);
    assert_eq!(r.code, Some(0), "saida: {r:?}");
    let bk = a.backups();
    assert_eq!(bk.len(), 1, "backups: {bk:?}");
    assert!(
        r.tudo().contains(&bk[0]),
        "relatorio sem o caminho do backup: {}",
        r.tudo()
    );
    assert!(
        r.tudo().contains("mv -f"),
        "relatorio sem 'mv -f': {}",
        r.tudo()
    );
    assert!(
        r.tudo().contains("systemctl --user restart chatd.service"),
        "relatorio sem o restart da reversao: {}",
        r.tudo()
    );
}

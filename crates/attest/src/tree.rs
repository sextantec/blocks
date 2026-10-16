//! Captura `HEAD` e a arvore de trabalho sem tocar no indice real.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

static TEMP_ID: AtomicU64 = AtomicU64::new(0);

pub struct Snapshot {
    pub tree: String,
    pub head: Option<String>,
}

struct TempGit {
    dir: PathBuf,
    index: PathBuf,
    objects: PathBuf,
    real_objects: PathBuf,
}

impl TempGit {
    fn create(repo: &Path, real_objects: &Path) -> Result<Self, String> {
        let base = std::env::temp_dir();
        let base_canon = fs::canonicalize(&base).map_err(|e| format!("tempdir: {e}"))?;
        let repo_canon = fs::canonicalize(repo).map_err(|e| format!("repositorio: {e}"))?;
        if base_canon.starts_with(&repo_canon) {
            return Err("diretorio temporario fica dentro do repositorio".into());
        }
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        for _ in 0..8 {
            let id = TEMP_ID.fetch_add(1, Ordering::Relaxed);
            let dir = base.join(format!("attest-git-{}-{stamp}-{id}", std::process::id()));
            match fs::create_dir(&dir) {
                Ok(()) => {
                    if let Err(error) = fs::set_permissions(&dir, fs::Permissions::from_mode(0o700))
                    {
                        let _ = fs::remove_dir(&dir);
                        return Err(format!("permissoes do indice temporario: {error}"));
                    }
                    let objects = dir.join("objects");
                    let setup = fs::create_dir(&objects).and_then(|()| {
                        fs::set_permissions(&objects, fs::Permissions::from_mode(0o700))
                    });
                    if let Err(error) = setup {
                        let _ = fs::remove_dir_all(&dir);
                        return Err(format!("objetos temporarios: {error}"));
                    }
                    return Ok(Self {
                        index: dir.join("index"),
                        objects,
                        real_objects: real_objects.to_path_buf(),
                        dir,
                    });
                }
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
                Err(error) => return Err(format!("indice temporario: {error}")),
            }
        }
        Err("nao criei diretorio temporario unico".into())
    }
}

impl Drop for TempGit {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.dir);
    }
}

pub fn snapshot(cwd: &Path) -> Result<Snapshot, String> {
    let root = repository_root(cwd)?;
    let head = git_text(cwd, &[], &["rev-parse", "HEAD"])
        .ok()
        .filter(|value| !value.is_empty());
    let real_objects = real_objects_dir(cwd)?;
    let temp = TempGit::create(&root, &real_objects)?;
    let head_tree = head.is_some();
    let init = if head_tree {
        git_with_temp(cwd, &temp, &["read-tree", "HEAD"])?
    } else {
        git_with_temp(cwd, &temp, &["read-tree", "--empty"])?
    };
    if !init.status.success() {
        return Err("git read-tree falhou".into());
    }
    let added = git_with_temp(cwd, &temp, &["add", "-A"])?;
    if !added.status.success() {
        return Err("git add -A falhou".into());
    }
    let tree = git_with_temp(cwd, &temp, &["write-tree"])?;
    if !tree.status.success() {
        return Err("git write-tree falhou".into());
    }
    let tree = String::from_utf8_lossy(&tree.stdout).trim().to_string();
    if tree.is_empty() {
        return Err("git write-tree nao devolveu tree".into());
    }
    Ok(Snapshot { tree, head })
}

pub fn ensure_repository(cwd: &Path) -> Result<PathBuf, String> {
    repository_root(cwd)
}

fn repository_root(cwd: &Path) -> Result<PathBuf, String> {
    let output = git(cwd, &[], &["rev-parse", "--show-toplevel"])
        .map_err(|_| "diretorio fora de um repositorio git".to_string())?;
    if !output.status.success() {
        return Err("diretorio fora de um repositorio git".into());
    }
    let root = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if root.is_empty() {
        return Err("git nao identificou o repositorio".into());
    }
    Ok(PathBuf::from(root))
}

fn git_text(cwd: &Path, env: &[(&str, &Path)], args: &[&str]) -> Result<String, String> {
    let output = git(cwd, env, args)?;
    if !output.status.success() {
        return Err("git rev-parse falhou".into());
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

fn real_objects_dir(cwd: &Path) -> Result<PathBuf, String> {
    let path = git_text(cwd, &[], &["rev-parse", "--git-path", "objects"])?;
    let path = PathBuf::from(path);
    let path = if path.is_absolute() {
        path
    } else {
        cwd.join(path)
    };
    fs::canonicalize(path).map_err(|e| format!("diretorio real de objetos indisponivel: {e}"))
}

fn git_with_temp(cwd: &Path, temp: &TempGit, args: &[&str]) -> Result<Output, String> {
    let alternates = temp.real_objects.as_os_str();
    let objects = temp.objects.as_path();
    let index = temp.index.as_path();
    git_env(
        cwd,
        &[("GIT_INDEX_FILE", index), ("GIT_OBJECT_DIRECTORY", objects)],
        args,
        Some(("GIT_ALTERNATE_OBJECT_DIRECTORIES", alternates)),
    )
}

fn git(cwd: &Path, env: &[(&str, &Path)], args: &[&str]) -> Result<Output, String> {
    git_env(cwd, env, args, None)
}

fn git_env(
    cwd: &Path,
    env: &[(&str, &Path)],
    args: &[&str],
    extra: Option<(&str, &std::ffi::OsStr)>,
) -> Result<Output, String> {
    let mut command = Command::new("git");
    command
        .current_dir(cwd)
        .args(args)
        .stderr(std::process::Stdio::null());
    for (key, value) in env {
        command.env(key, value);
    }
    if let Some((key, value)) = extra {
        command.env(key, value);
    }
    command
        .output()
        .map_err(|e| format!("git indisponivel: {e}"))
}

use std::os::unix::fs::PermissionsExt;

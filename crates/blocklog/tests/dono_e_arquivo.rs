//! Dono, trava e permissoes com arquivo real (spec secao 2 e secao 4 passo 1; B23-B26).
//! Usa Log::open_dir (FsStorage + SystemClock) em diretorio temporario.
mod common;

use blocklog::{Log, OpenError, Options};
use common::*;
use std::fs;
use std::os::unix::fs::MetadataExt;
use std::os::unix::fs::symlink;

/// Cria `log.jsonl` vazio com o modo dado dentro de um estado 0700.
fn estado_com_log(modo_log: u32) -> (tempfile::TempDir, std::path::PathBuf) {
    let (t, st) = estado_novo();
    let f = st.join("log.jsonl");
    fs::write(&f, b"").unwrap();
    fixa_modo(&f, modo_log);
    (t, st)
}

// ---------------------------------------------------------------- B23

#[test]
fn b23_segundo_open_no_mesmo_diretorio_e_already_running() {
    let (_t, st) = estado_novo();
    let (log1, c) = Log::open_dir(&st, Options::default()).expect("primeira abertura");
    assert!(c.is_empty());

    let e = erro_abre_dir(&st, Options::default());
    assert!(matches!(e, OpenError::AlreadyRunning), "{e:?}");

    // controle: com o log anterior fechado, a abertura passa
    drop(log1);
    let (log2, _) = Log::open_dir(&st, Options::default()).expect("controle: lock solto");
    assert_eq!(log2.head_seq(), 0);
}

// ---------------------------------------------------------------- B24

#[test]
fn b24_modo_errado_symlink_ou_nao_regular_recusa_insecure_permissions() {
    // controle: diretorio 0700 e log 0600 abrem
    let (_t, st) = estado_com_log(0o600);
    let (log, _) = Log::open_dir(&st, Options::default()).expect("controle: 0700/0600 abre");
    drop(log);

    // diretorio com bit de grupo, depois com bit de outros
    for modo_dir in [0o750u32, 0o701] {
        let (_t, st) = estado_novo();
        fixa_modo(&st, modo_dir);
        let e = erro_abre_dir(&st, Options::default());
        assert!(
            matches!(e, OpenError::InsecurePermissions(_)),
            "dir {modo_dir:o}: {e:?}"
        );
    }

    // log com bit de grupo, depois com bit de outros
    for modo_log in [0o640u32, 0o604] {
        let (_t, st) = estado_com_log(modo_log);
        let e = erro_abre_dir(&st, Options::default());
        assert!(
            matches!(e, OpenError::InsecurePermissions(_)),
            "log {modo_log:o}: {e:?}"
        );
    }

    // log e symlink para arquivo 0600 (abertura com O_NOFOLLOW)
    let (_t, st) = estado_novo();
    fs::write(st.join("alvo"), b"").unwrap();
    fixa_modo(&st.join("alvo"), 0o600);
    symlink(st.join("alvo"), st.join("log.jsonl")).unwrap();
    let e = erro_abre_dir(&st, Options::default());
    assert!(
        matches!(e, OpenError::InsecurePermissions(_)),
        "symlink: {e:?}"
    );

    // log que nao e arquivo regular (um diretorio)
    let (_t, st) = estado_novo();
    fs::create_dir(st.join("log.jsonl")).unwrap();
    let e = erro_abre_dir(&st, Options::default());
    assert!(
        matches!(e, OpenError::InsecurePermissions(_)),
        "nao regular: {e:?}"
    );
}

// ---------------------------------------------------------------- B25

#[test]
fn b25_arquivos_criados_com_0600_e_diretorio_0700() {
    // O fsync do diretorio depois de criar log.jsonl nao e observavel por API publica
    // (nao ha como ver syscalls sem strace). Este teste cobre so os modos e o conteudo inicial.
    let (_t, st) = estado_novo();
    let (log, c) = Log::open_dir(&st, Options::default()).expect("criacao");
    assert!(c.is_empty());
    assert_eq!(modo(&st.join("log.jsonl")), 0o600, "log.jsonl criado 0600");
    assert_eq!(modo(&st.join("LOCK")), 0o600, "LOCK criado 0600");
    assert_eq!(modo(&st), 0o700, "diretorio segue 0700");
    assert!(
        fs::read(st.join("log.jsonl")).unwrap().is_empty(),
        "arquivo novo comeca vazio"
    );
    drop(log);
}

// ---------------------------------------------------------------- B26 (so root)

#[test]
#[ignore = "exige root: troca o dono de log.jsonl com chown; rodar: sudo -E cargo test -p blocklog --test dono_e_arquivo -- --ignored b26"]
fn b26_uid_errado_e_recusado() {
    assert!(
        euid_root(),
        "b26 so roda como root (euid 0); rode com --ignored sob sudo"
    );
    let (_t, st) = estado_novo();
    let (log, _) = Log::open_dir(&st, Options::default()).expect("primeira abertura");
    drop(log);

    // uid do processo: dono do diretorio criado por este processo
    let meu_uid = fs::metadata(&st).unwrap().uid();
    let arquivo = st.join("log.jsonl");

    std::os::unix::fs::chown(&arquivo, Some(65534), None).expect("chown para outro uid");
    let e = erro_abre_dir(&st, Options::default());
    assert!(matches!(e, OpenError::InsecurePermissions(_)), "{e:?}");

    // controle: com o dono de volta ao uid do processo, abre
    std::os::unix::fs::chown(&arquivo, Some(meu_uid), None).expect("chown de volta");
    Log::open_dir(&st, Options::default()).expect("controle: dono correto abre");
}

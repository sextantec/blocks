//! Fakes em memoria (Storage, LogFile, Clock) e helpers dos testes caixa-preta do blocklog.
//! Usa so a API publica da spec (secao 8). Os fakes guardam o "disco" em Arc<Mutex<..>>,
//! para que duas aberturas do mesmo `Disco` simulem reabrir o mesmo diretorio.
#![allow(dead_code, unused_imports)]

use blocklog::{Clock, Commit, CommitError, Log, LogFile, OpenError, Options, Req, Storage};
use chrono::{DateTime, Utc};
use serde_json::{Value, json};
use std::fs;
use std::io;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard};

pub const TS_000: &str = "2026-10-16T22:00:00.000Z";
pub const TS_100: &str = "2026-10-16T22:00:00.100Z";
pub const TS_200: &str = "2026-10-16T22:00:00.200Z";
pub const TS_300: &str = "2026-10-16T22:00:00.300Z";
pub const TS_500: &str = "2026-10-16T22:00:00.500Z";
pub const TS_900: &str = "2026-10-16T22:00:00.900Z";
/// Relogio padrao dos testes de escrita: ts do primeiro commit.
pub const TS_123: &str = "2026-10-16T22:00:00.123Z";
/// Instante usado ao reabrir (depois de todos os ts gravados nos testes de recuperacao).
pub const TS_ABERTURA: &str = "2026-10-16T22:00:01.000Z";
pub const OK: &str = r#"{"ok":true}"#;
pub const EV_UM: &str = r#"{"k":"um"}"#;
pub const EV_DOIS: &str = r#"{"k":"dois"}"#;

/// SHA-256 de teste: 64 hex minusculos determinados por `n` (nao usa sha256_hex).
pub fn hash(n: u64) -> String {
    format!("{n:064x}")
}

pub fn ev(texto: &str) -> Value {
    json!({ "k": texto })
}

pub fn ok_json() -> Value {
    json!({ "ok": true })
}

pub fn rq(agente: &str, id: &str, h: &str) -> Req {
    Req {
        agent: agente.to_string(),
        id: id.to_string(),
        h: h.to_string(),
    }
}

/// Linha no formato da secao 3, montada a mao (req e res sempre presentes).
/// `tx` e o texto JSON da lista de eventos, sem os colchetes.
pub fn linha(seq: u64, ts: &str, tx: &str, agente: &str, id: &str, h: &str, res: &str) -> String {
    format!(
        r#"{{"v":1,"seq":{seq},"ts":"{ts}","tx":[{tx}],"req":{{"agent":"{agente}","id":"{id}","h":"{h}"}},"res":{res}}}"#
    )
}

/// Linha valida de seq `n` com um evento `{"k":"<texto>"}`, req (a, r<n>, hash(n)) e res {"ok":true}.
pub fn lin(seq: u64, ts: &str, texto: &str) -> String {
    linha(
        seq,
        ts,
        &format!(r#"{{"k":"{texto}"}}"#),
        "a",
        &format!("r{seq}"),
        &hash(seq),
        OK,
    )
}

/// Commit que `lin(seq, ts, texto)` deve produzir ao ser lido de volta.
pub fn esperado(seq: u64, ts: &str, texto: &str) -> Commit {
    Commit {
        seq,
        ts: ts.to_string(),
        tx: vec![ev(texto)],
        req: Some(rq("a", &format!("r{seq}"), &hash(seq))),
        res: Some(ok_json()),
    }
}

/// Bytes de um log: cada linha seguida de `\n`.
pub fn arq<S: AsRef<str>>(linhas: &[S]) -> Vec<u8> {
    linhas
        .iter()
        .flat_map(|l| format!("{}\n", l.as_ref()).into_bytes())
        .collect()
}

pub fn com_chave_extra(l: &str) -> String {
    format!("{},\"x\":1}}", &l[..l.len() - 1])
}

pub fn instante(ts: &str) -> DateTime<Utc> {
    DateTime::parse_from_rfc3339(ts)
        .unwrap()
        .with_timezone(&Utc)
}

// ---------------------------------------------------------------- fakes de E/S

/// Falha injetada na proxima operacao de escrita ou de sync.
#[derive(Clone, Copy, Debug)]
pub enum Falha {
    /// EIO (raw os error 5). Nada e gravado em write_all.
    Eio,
    /// ENOSPC (raw os error 28). Nada e gravado em write_all.
    Enospc,
    /// Grava so os primeiros n bytes do buffer e devolve erro.
    EscritaCurta(usize),
}

/// Operacoes registradas pelo fake, na ordem em que ocorreram.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Op {
    Read,
    /// write_all com um buffer de n bytes (conta a tentativa, mesmo se falhou).
    Write(usize),
    SyncData,
    SyncAll,
    SetLen(u64),
}

#[derive(Default)]
struct Estado {
    bytes: Vec<u8>,
    trancado: bool,
    ops: Vec<Op>,
    falha_escrita: Option<Falha>,
    falha_sync: Option<Falha>,
}

/// O "disco": conteudo do log.jsonl, estado do flock, historico de operacoes e falhas armadas.
#[derive(Clone, Default)]
pub struct Disco(Arc<Mutex<Estado>>);

impl Disco {
    pub fn novo() -> Disco {
        Disco::default()
    }

    pub fn com(bytes: &[u8]) -> Disco {
        let d = Disco::novo();
        d.semear(bytes);
        d
    }

    fn st(&self) -> MutexGuard<'_, Estado> {
        self.0.lock().unwrap()
    }

    /// Substitui o conteudo do arquivo sem registrar operacao (preparo do cenario).
    pub fn semear(&self, bytes: &[u8]) {
        self.st().bytes = bytes.to_vec();
    }

    pub fn bytes(&self) -> Vec<u8> {
        self.st().bytes.clone()
    }

    /// Marca a posicao atual do historico; use com `ops_desde`.
    pub fn marca(&self) -> usize {
        self.st().ops.len()
    }

    pub fn ops_desde(&self, marca: usize) -> Vec<Op> {
        self.st().ops[marca..].to_vec()
    }

    pub fn ops(&self) -> Vec<Op> {
        self.ops_desde(0)
    }

    /// Falha aplicada na proxima write_all (uma vez).
    pub fn arma_escrita(&self, f: Falha) {
        self.st().falha_escrita = Some(f);
    }

    /// Falha aplicada no proximo sync_data (uma vez). Nesse caso o buffer ja foi gravado.
    pub fn arma_sync(&self, f: Falha) {
        self.st().falha_sync = Some(f);
    }
}

fn erro_io(f: Falha) -> io::Error {
    match f {
        Falha::Eio => io::Error::from_raw_os_error(5),
        Falha::Enospc => io::Error::from_raw_os_error(28),
        Falha::EscritaCurta(_) => io::Error::other("escrita curta"),
    }
}

/// Trava exclusiva simulada: segura enquanto o LogFile (ou o Storage, se nunca abriu) viver.
struct Trava(Disco);

impl Drop for Trava {
    fn drop(&mut self) {
        self.0.st().trancado = false;
    }
}

pub struct ArmazenamentoFalso {
    disco: Disco,
    trava: Option<Trava>,
}

impl ArmazenamentoFalso {
    pub fn novo(disco: &Disco) -> ArmazenamentoFalso {
        ArmazenamentoFalso {
            disco: disco.clone(),
            trava: None,
        }
    }
}

impl Storage for ArmazenamentoFalso {
    fn lock(&mut self) -> Result<(), OpenError> {
        {
            let mut e = self.disco.st();
            if e.trancado {
                return Err(OpenError::AlreadyRunning);
            }
            e.trancado = true;
        }
        self.trava = Some(Trava(self.disco.clone()));
        Ok(())
    }

    fn open_log(&mut self) -> Result<Box<dyn LogFile>, OpenError> {
        Ok(Box::new(ArquivoFalso {
            disco: self.disco.clone(),
            _trava: self.trava.take(),
        }))
    }
}

pub struct ArquivoFalso {
    disco: Disco,
    _trava: Option<Trava>,
}

impl LogFile for ArquivoFalso {
    fn read_all(&mut self) -> io::Result<Vec<u8>> {
        let mut e = self.disco.st();
        e.ops.push(Op::Read);
        Ok(e.bytes.clone())
    }

    fn write_all(&mut self, buf: &[u8]) -> io::Result<()> {
        let mut e = self.disco.st();
        e.ops.push(Op::Write(buf.len()));
        match e.falha_escrita.take() {
            None => {
                e.bytes.extend_from_slice(buf);
                Ok(())
            }
            Some(Falha::EscritaCurta(n)) => {
                let k = n.min(buf.len());
                let prefixo = buf[..k].to_vec();
                e.bytes.extend_from_slice(&prefixo);
                Err(erro_io(Falha::EscritaCurta(n)))
            }
            Some(f) => Err(erro_io(f)),
        }
    }

    fn sync_data(&mut self) -> io::Result<()> {
        let mut e = self.disco.st();
        e.ops.push(Op::SyncData);
        match e.falha_sync.take() {
            None => Ok(()),
            Some(f) => Err(erro_io(f)),
        }
    }

    fn sync_all(&mut self) -> io::Result<()> {
        self.disco.st().ops.push(Op::SyncAll);
        Ok(())
    }

    fn set_len(&mut self, len: u64) -> io::Result<()> {
        let mut e = self.disco.st();
        e.ops.push(Op::SetLen(len));
        if len > e.bytes.len() as u64 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "set_len alem do fim",
            ));
        }
        e.bytes.truncate(len as usize);
        Ok(())
    }

    fn len(&mut self) -> io::Result<u64> {
        Ok(self.disco.st().bytes.len() as u64)
    }
}

/// Relogio controlavel; `poe` ajusta o instante (inclusive para tras).
#[derive(Clone)]
pub struct RelogioFalso(Arc<Mutex<DateTime<Utc>>>);

impl RelogioFalso {
    pub fn em(ts: &str) -> RelogioFalso {
        RelogioFalso(Arc::new(Mutex::new(instante(ts))))
    }

    pub fn poe(&self, ts: &str) {
        *self.0.lock().unwrap() = instante(ts);
    }
}

impl Clock for RelogioFalso {
    fn now(&self) -> DateTime<Utc> {
        *self.0.lock().unwrap()
    }
}

// ---------------------------------------------------------------- aberturas e resultados

pub fn abre(d: &Disco, r: &RelogioFalso, o: Options) -> Result<(Log, Vec<Commit>), OpenError> {
    Log::open(
        Box::new(ArmazenamentoFalso::novo(d)),
        Box::new(r.clone()),
        o,
    )
}

pub fn abre_ok(d: &Disco, r: &RelogioFalso, o: Options) -> (Log, Vec<Commit>) {
    match abre(d, r, o) {
        Ok(x) => x,
        Err(e) => panic!("open deveria passar: {e:?}"),
    }
}

pub fn erro_abre(d: &Disco, r: &RelogioFalso, o: Options) -> OpenError {
    match abre(d, r, o) {
        Ok(_) => panic!("open deveria recusar"),
        Err(e) => e,
    }
}

/// Linha (1-based) de um OpenError::Corrupt; outro erro e falha do teste.
pub fn linha_corrupta(e: OpenError) -> usize {
    match e {
        OpenError::Corrupt { line, .. } => line,
        outro => panic!("esperava Corrupt, veio {outro:?}"),
    }
}

pub fn ok<T>(r: Result<T, CommitError>) -> T {
    match r {
        Ok(x) => x,
        Err(e) => panic!("commit deveria passar: {e:?}"),
    }
}

pub fn erro<T>(r: Result<T, CommitError>) -> CommitError {
    match r {
        Ok(_) => panic!("commit deveria falhar"),
        Err(e) => e,
    }
}

// ---------------------------------------------------------------- arquivo real

/// Diretorio de estado real (modo 0700) dentro de um tempdir. Guarde o TempDir para mante-lo vivo.
pub fn estado_novo() -> (tempfile::TempDir, PathBuf) {
    let t = tempfile::tempdir().unwrap();
    let p = t.path().join("estado");
    fs::create_dir(&p).unwrap();
    fixa_modo(&p, 0o700);
    (t, p)
}

pub fn fixa_modo(p: &Path, modo: u32) {
    fs::set_permissions(p, fs::Permissions::from_mode(modo)).unwrap();
}

pub fn modo(p: &Path) -> u32 {
    fs::metadata(p).unwrap().permissions().mode() & 0o777
}

pub fn erro_abre_dir(dir: &Path, o: Options) -> OpenError {
    match Log::open_dir(dir, o) {
        Ok(_) => panic!("open_dir deveria recusar"),
        Err(e) => e,
    }
}

/// True se o processo roda com euid 0 (le /proc/self/status, sem libc).
pub fn euid_root() -> bool {
    fs::read_to_string("/proc/self/status")
        .unwrap()
        .lines()
        .find(|l| l.starts_with("Uid:"))
        .map(|l| l.split_whitespace().nth(2) == Some("0"))
        .unwrap_or(false)
}

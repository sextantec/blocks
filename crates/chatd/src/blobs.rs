//! Armazenamento de anexos v1.8 Parte B.
//!
//! O mutex deste modulo nao e a trava global do Store: hashing, arquivos e uploads
//! ficam isolados do caminho de `message.send`. Nomes no disco sao sempre ids opacos.

use std::collections::{HashMap, VecDeque};
use std::ffi::CString;
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Seek, SeekFrom, Write};
use std::os::fd::{AsRawFd, FromRawFd};
use std::os::unix::fs::{MetadataExt, OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use serde_json::{Map, Value, json};

use crate::hash::Sha256;
use crate::protocol::{self, ApiError};
use crate::store::{BlobRecord, Store, charge_sliding_rate, now_ts, sliding_rate_available};

pub const CHUNK_MAX: usize = 512 * 1024;
pub const BLOB_MAX: u64 = 8 * 1024 * 1024;
const OWNER_MAX_BYTES: u64 = 64 * 1024 * 1024;
const GLOBAL_MAX_BYTES: u64 = 256 * 1024 * 1024;
const OWNER_MAX_COUNT: usize = 32;
const UPLOAD_MAX_GLOBAL: usize = 8;
const UPLOAD_MAX_CHUNKS: u64 = 20;
const PUT_RATE: usize = 30;
const PUT_WINDOW: Duration = Duration::from_secs(60);
const GET_RATE: usize = 600;
const GET_WINDOW: Duration = Duration::from_secs(60);
const UPLOAD_IDLE: Duration = Duration::from_secs(120);
const UPLOAD_LIFETIME: Duration = Duration::from_secs(600);
const TTL_DEFAULT: u64 = 86_400;
const TTL_MAX: u64 = 604_800;
const DISK_RESERVE: u64 = 512 * 1024 * 1024;

fn owner_count_exceeded(count: usize) -> bool {
    count >= OWNER_MAX_COUNT
}

#[derive(Clone)]
struct ActiveBlob {
    record: BlobRecord,
    expires: Instant,
}

struct Upload {
    id: String,
    owner: String,
    to: String,
    size: u64,
    name: String,
    ttl_s: u64,
    temp_name: String,
    file: File,
    digest: Sha256,
    tail: Vec<u8>,
    received: u64,
    next_chunk: u64,
    started: Instant,
    touched: Instant,
    connection: u64,
}

pub struct BlobManager {
    root: File,
    path: PathBuf,
    active: HashMap<String, ActiveBlob>,
    uploads: HashMap<String, Upload>,
    rate: HashMap<String, VecDeque<Instant>>,
    get_rate: HashMap<String, VecDeque<Instant>>,
}

fn api(code: &'static str, message: &str) -> ApiError {
    ApiError::new(code, message)
}

fn c_name(s: &str) -> Result<CString, ApiError> {
    CString::new(s).map_err(|_| ApiError::invalid("/blob", "nome invalido"))
}

fn random_hex() -> Result<String, ApiError> {
    let mut bytes = [0u8; 16];
    let mut done = 0;
    while done < bytes.len() {
        // SAFETY: the destination is a valid writable buffer of the supplied length.
        let n =
            unsafe { libc::getrandom(bytes[done..].as_mut_ptr().cast(), bytes.len() - done, 0) };
        if n < 0 {
            return Err(api("internal_error", "nao gerei identificador aleatorio"));
        }
        if n == 0 {
            return Err(api("internal_error", "gerador aleatorio sem progresso"));
        }
        done += n as usize;
    }
    Ok(bytes.iter().map(|b| format!("{b:02x}")).collect())
}

fn is_hex(s: &str, len: usize) -> bool {
    s.len() == len
        && s.bytes()
            .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
}

fn blob_id(s: &str) -> bool {
    s.strip_prefix("blob:").is_some_and(|h| is_hex(h, 32))
}

fn valid_name(s: &str) -> bool {
    (1..=64).contains(&s.len())
        && s.bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-'))
}

fn valid_digest(s: &str) -> bool {
    is_hex(s, 64)
}

fn req_string<'a>(p: &'a Map<String, Value>, key: &str) -> Result<&'a str, ApiError> {
    protocol::req_str(p, key)
}

fn reject_unknown(p: &Map<String, Value>, allowed: &[&str]) -> Result<(), ApiError> {
    protocol::reject_unknown(p, allowed)
}

fn set_dir_mode(path: &Path) -> Result<(), String> {
    let meta =
        fs::symlink_metadata(path).map_err(|e| format!("nao acessei diretorio blobs: {e}"))?;
    // SAFETY: geteuid has no pointer arguments and only reads the current uid.
    let uid = unsafe { libc::geteuid() };
    if !meta.file_type().is_dir() || meta.uid() != uid || meta.permissions().mode() & 0o777 != 0o700
    {
        return Err("blobs/ deve ser diretorio real, do usuario e modo 0700".into());
    }
    Ok(())
}

fn open_dir(path: &Path) -> Result<File, String> {
    OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC)
        .open(path)
        .map_err(|e| format!("nao abri diretorio blobs com seguranca: {e}"))
}

fn openat_dir(root: &File, name: &str, create: bool) -> Result<Option<File>, ApiError> {
    let name_c = c_name(name)?;
    if create {
        // SAFETY: root is a live directory fd and name is a NUL-terminated single component.
        let rc = unsafe { libc::mkdirat(root.as_raw_fd(), name_c.as_ptr(), 0o700) };
        if rc != 0 {
            let e = std::io::Error::last_os_error();
            if e.kind() != std::io::ErrorKind::AlreadyExists {
                return Err(api("disk_full", "nao criei diretorio do blob"));
            }
        }
    }
    // SAFETY: openat receives a valid directory fd and checked single-component name.
    let fd = unsafe {
        libc::openat(
            root.as_raw_fd(),
            name_c.as_ptr(),
            libc::O_RDONLY | libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC,
        )
    };
    if fd < 0 {
        let e = std::io::Error::last_os_error();
        if e.kind() == std::io::ErrorKind::NotFound && !create {
            return Ok(None);
        }
        return Err(api(
            "internal_error",
            "diretorio do blob inseguro ou indisponivel",
        ));
    }
    // SAFETY: openat returned a newly owned file descriptor.
    let dir = unsafe { File::from_raw_fd(fd) };
    let mut st = std::mem::MaybeUninit::<libc::stat>::uninit();
    // SAFETY: fstat writes a stat value to the provided aligned allocation.
    if unsafe { libc::fstat(fd, st.as_mut_ptr()) } != 0 {
        return Err(api("internal_error", "nao validei diretorio do blob"));
    }
    // SAFETY: fstat succeeded.
    let st = unsafe { st.assume_init() };
    // SAFETY: geteuid only reads process credentials.
    let uid = unsafe { libc::geteuid() };
    if st.st_uid != uid
        || st.st_mode & 0o777 != 0o700
        || (st.st_mode & libc::S_IFMT) != libc::S_IFDIR
    {
        return Err(api(
            "internal_error",
            "diretorio do blob deve ser 0700 do usuario",
        ));
    }
    Ok(Some(dir))
}

fn openat_file(dir: &File, name: &str, flags: i32, mode: u32) -> Result<File, ApiError> {
    let name = c_name(name)?;
    // SAFETY: dir is a live directory fd; O_NOFOLLOW is added by every call site.
    let fd = unsafe {
        libc::openat(
            dir.as_raw_fd(),
            name.as_ptr(),
            flags | libc::O_NOFOLLOW | libc::O_CLOEXEC,
            mode,
        )
    };
    if fd < 0 {
        return Err(api("not_found", "blob nao encontrado"));
    }
    // SAFETY: openat returned a new descriptor owned by this File.
    let file = unsafe { File::from_raw_fd(fd) };
    let mut st = std::mem::MaybeUninit::<libc::stat>::uninit();
    // SAFETY: fstat writes to the provided valid stat allocation.
    if unsafe { libc::fstat(fd, st.as_mut_ptr()) } != 0 {
        return Err(api("not_found", "blob nao encontrado"));
    }
    // SAFETY: fstat succeeded.
    let st = unsafe { st.assume_init() };
    // SAFETY: geteuid only reads process credentials.
    let uid = unsafe { libc::geteuid() };
    if st.st_uid != uid
        || (st.st_mode & libc::S_IFMT) != libc::S_IFREG
        || st.st_mode & 0o777 != 0o600
    {
        return Err(api("not_found", "blob nao encontrado"));
    }
    Ok(file)
}

fn fsync(file: &File) -> Result<(), ApiError> {
    file.sync_all()
        .map_err(|_| api("disk_full", "falha de durabilidade do blob"))
}

fn expiry(record: &BlobRecord) -> Instant {
    let now = Instant::now();
    let created = chrono::DateTime::parse_from_rfc3339(&record.created_at)
        .map(|t| t.with_timezone(&chrono::Utc));
    let remaining = created.map_or(Duration::ZERO, |created| {
        let ttl_exp = created + chrono::Duration::seconds(record.ttl_s.min(TTL_MAX) as i64);
        let max_exp = created + chrono::Duration::seconds(TTL_MAX as i64);
        let end = ttl_exp.min(max_exp);
        (end - chrono::Utc::now())
            .to_std()
            .unwrap_or(Duration::ZERO)
    });
    now + remaining
}

pub(crate) type EventSink<'a> =
    dyn FnMut(&'static str, Vec<(&'static str, Value)>) -> Result<(), ApiError> + 'a;

fn put_fields(record: &BlobRecord) -> Vec<(&'static str, Value)> {
    vec![
        ("blob", json!(record.id)),
        ("owner", json!(record.owner)),
        ("to", json!(record.to)),
        ("size", json!(record.size)),
        ("name", json!(record.name)),
        ("sha256", json!(record.sha256)),
        ("created_at", json!(record.created_at)),
        ("ttl_s", json!(record.ttl_s)),
    ]
}

impl BlobManager {
    pub fn open(state_dir: &Path, records: Vec<BlobRecord>) -> Result<Self, String> {
        let path = state_dir.join("blobs");
        match fs::symlink_metadata(&path) {
            Ok(_) => set_dir_mode(&path)?,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                fs::create_dir(&path).map_err(|e| format!("nao criei blobs/: {e}"))?;
                fs::set_permissions(&path, fs::Permissions::from_mode(0o700))
                    .map_err(|e| format!("nao ajustei blobs/ para 0700: {e}"))?;
            }
            Err(e) => return Err(format!("nao acessei blobs/: {e}")),
        }
        let root = open_dir(&path)?;
        let mut this = Self {
            root,
            path,
            active: HashMap::new(),
            uploads: HashMap::new(),
            rate: HashMap::new(),
            get_rate: HashMap::new(),
        };
        for record in records {
            let expires = expiry(&record);
            this.active
                .insert(record.id.clone(), ActiveBlob { record, expires });
        }
        Ok(this)
    }

    pub fn reconcile(&mut self, store: &mut Store) -> Result<(), String> {
        let mut seen = HashMap::<String, bool>::new();
        let entries = fs::read_dir(&self.path).map_err(|e| format!("nao li blobs/: {e}"))?;
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().to_string();
            if let Some(hex) = name.strip_prefix("tmp-") {
                if is_hex(hex, hex.len()) && !hex.is_empty() {
                    let c = CString::new(name.as_str()).unwrap();
                    // SAFETY: root is a held directory fd; unlinkat does not follow symlinks.
                    unsafe {
                        libc::unlinkat(self.root.as_raw_fd(), c.as_ptr(), 0);
                    }
                }
                continue;
            }
            if name.len() != 2 || !is_hex(&name, 2) {
                continue;
            }
            let meta = fs::symlink_metadata(entry.path())
                .map_err(|e| format!("nao validei shard: {e}"))?;
            if !meta.file_type().is_dir()
                || meta.uid() != unsafe { libc::geteuid() }
                || meta.permissions().mode() & 0o777 != 0o700
            {
                return Err("blobs/<aa> deve ser diretorio real, do usuario e modo 0700".into());
            }
            let shard = openat_dir(&self.root, &name, false)
                .map_err(|e| e.message)?
                .ok_or_else(|| "shard desapareceu".to_string())?;
            for child in fs::read_dir(entry.path())
                .map_err(|e| format!("nao li shard: {e}"))?
                .flatten()
            {
                let filename = child.file_name().to_string_lossy().to_string();
                if !is_hex(&filename, 32) {
                    continue;
                }
                let id = format!("blob:{filename}");
                let expected = self.active.contains_key(&id);
                if expected {
                    let opened = openat_file(&shard, &filename, libc::O_RDONLY, 0);
                    if opened.is_ok() {
                        seen.insert(id, true);
                    }
                } else {
                    let c = CString::new(filename.as_str()).unwrap();
                    // SAFETY: shard is held open and unlinkat never follows a symlink.
                    unsafe {
                        libc::unlinkat(shard.as_raw_fd(), c.as_ptr(), 0);
                    }
                }
            }
        }
        let records: Vec<BlobRecord> = self.active.values().map(|b| b.record.clone()).collect();
        for record in records {
            let mut emit = |kind: &'static str, fields: Vec<(&'static str, Value)>| {
                store.blob_event(kind, fields)
            };
            if record_expires(&record) {
                self.remove_durable(&mut emit, &record, "ttl", true)
                    .map_err(|e| e.message)?;
            } else if !seen.contains_key(&record.id) {
                self.remove_durable(&mut emit, &record, "missing", false)
                    .map_err(|e| e.message)?;
            }
        }
        fsync(&self.root).map_err(|e| e.message)?;
        Ok(())
    }

    fn shard(&self, id: &str, create: bool) -> Result<Option<File>, ApiError> {
        let h = id
            .strip_prefix("blob:")
            .ok_or_else(|| ApiError::invalid("/blob", "id invalido"))?;
        if !is_hex(h, 32) {
            return Err(ApiError::invalid("/blob", "id invalido"));
        }
        openat_dir(&self.root, &h[..2], create)
    }

    fn remove_durable(
        &mut self,
        emit: &mut EventSink<'_>,
        record: &BlobRecord,
        reason: &str,
        expired: bool,
    ) -> Result<(), ApiError> {
        let fields = vec![
            ("blob", json!(record.id)),
            ("owner", json!(record.owner)),
            ("to", json!(record.to)),
            ("name", json!(record.name)),
            ("reason", json!(reason)),
        ];
        emit(
            if expired {
                "blob_expired"
            } else {
                "blob_removed"
            },
            fields,
        )?;
        self.active.remove(&record.id);
        if let Some(shard) = self.shard(&record.id, false)? {
            let filename = &record.id[5..];
            let c = c_name(filename)?;
            // SAFETY: shard is held open, unlinkat never follows symlinks.
            if unsafe { libc::unlinkat(shard.as_raw_fd(), c.as_ptr(), 0) } != 0 {
                let e = std::io::Error::last_os_error();
                if e.kind() != std::io::ErrorKind::NotFound {
                    return Err(api("internal_error", "falha ao remover blob"));
                }
            }
            fsync(&shard)?;
        }
        Ok(())
    }

    fn expire_due(&mut self, emit: &mut EventSink<'_>) -> Result<(), ApiError> {
        let now = Instant::now();
        let due: Vec<BlobRecord> = self
            .active
            .values()
            .filter(|b| b.expires <= now)
            .map(|b| b.record.clone())
            .collect();
        for record in due {
            self.remove_durable(emit, &record, "ttl", true)?;
        }
        let stale: Vec<String> = self
            .uploads
            .values()
            .filter(|u| {
                now.duration_since(u.touched) >= UPLOAD_IDLE
                    || now.duration_since(u.started) >= UPLOAD_LIFETIME
            })
            .map(|u| u.id.clone())
            .collect();
        for id in stale {
            self.abort(&id);
        }
        Ok(())
    }

    fn record_first_read(
        &mut self,
        who: &str,
        record: &BlobRecord,
        offset: u64,
        length: u64,
        emit: &mut EventSink<'_>,
    ) -> Result<(), ApiError> {
        if record.read_by.contains(who) {
            return Ok(());
        }
        emit(
            "blob_got",
            vec![
                ("blob", json!(record.id)),
                ("owner", json!(record.owner)),
                ("to", json!(record.to)),
                ("by", json!(who)),
                ("offset", json!(offset)),
                ("length", json!(length)),
            ],
        )?;
        self.active
            .get_mut(&record.id)
            .expect("active blob checked")
            .record
            .read_by
            .insert(who.to_string());
        Ok(())
    }

    fn abort(&mut self, id: &str) {
        if let Some(upload) = self.uploads.remove(id) {
            let c = CString::new(upload.temp_name).expect("generated temporary name");
            // SAFETY: root fd is held open and unlinkat cannot follow symlinks.
            unsafe {
                libc::unlinkat(self.root.as_raw_fd(), c.as_ptr(), 0);
            }
        }
    }

    pub fn begin(
        &mut self,
        who: &str,
        connection: u64,
        p: &Map<String, Value>,
        known: bool,
        emit: &mut EventSink<'_>,
    ) -> Result<Value, ApiError> {
        reject_unknown(p, &["size", "to", "name", "ttl_s"])?;
        let size = protocol::req_u64(p, "size")?;
        let to = req_string(p, "to")?;
        protocol::check_name(to, "/to")?;
        if !known {
            return Err(api("unknown_recipient", "destinatario desconhecido"));
        }
        if size > BLOB_MAX {
            return Err(api("too_large", "blob acima de 8 MiB"));
        }
        let name = protocol::opt_str(p, "name")?.unwrap_or("anexo");
        if !valid_name(name) {
            return Err(ApiError::invalid(
                "/name",
                "name deve conter 1 a 64 caracteres ASCII validos",
            ));
        }
        let ttl = protocol::opt_u64(p, "ttl_s")?.unwrap_or(TTL_DEFAULT);
        if !(60..=TTL_MAX).contains(&ttl) {
            return Err(ApiError::invalid("/ttl_s", "ttl_s fora de 60 a 604800"));
        }
        self.expire_due(emit)?;
        let now = Instant::now();
        let rate = self.rate.entry(who.to_string()).or_default();
        if !sliding_rate_available(rate, now, PUT_RATE, PUT_WINDOW) {
            return Err(api("rate_limited", "limite de put.begin excedido"));
        }
        if self.uploads.values().any(|u| u.owner == who) {
            return Err(api("upload_busy", "ja existe upload desta identidade"));
        }
        if self.uploads.len() >= UPLOAD_MAX_GLOBAL {
            return Err(api("rate_limited", "limite global de uploads simultaneos"));
        }
        let owner_bytes: u64 = self
            .active
            .values()
            .filter(|b| b.record.owner == who)
            .map(|b| b.record.size)
            .sum::<u64>()
            + self
                .uploads
                .values()
                .filter(|u| u.owner == who)
                .map(|u| u.size)
                .sum::<u64>();
        let total_bytes: u64 = self.active.values().map(|b| b.record.size).sum::<u64>()
            + self.uploads.values().map(|u| u.size).sum::<u64>();
        let owner_count = self
            .active
            .values()
            .filter(|b| b.record.owner == who)
            .count()
            + self.uploads.values().filter(|u| u.owner == who).count();
        if owner_bytes.saturating_add(size) > OWNER_MAX_BYTES
            || owner_count_exceeded(owner_count)
            || total_bytes.saturating_add(size) > GLOBAL_MAX_BYTES
        {
            return Err(api("quota_exceeded", "cota de anexos excedida"));
        }
        if available_bytes(&self.root).is_none_or(|free| free < size.saturating_add(DISK_RESERVE)) {
            return Err(api("disk_full", "espaco livre insuficiente para anexo"));
        }
        let temp_name = format!("tmp-{}", random_hex()?);
        let id = format!("up:{}", random_hex()?);
        let file = openat_file(
            &self.root,
            &temp_name,
            libc::O_RDWR | libc::O_CREAT | libc::O_EXCL,
            0o600,
        )
        .map_err(|_| api("disk_full", "nao criei temporario de anexo"))?;
        self.uploads.insert(
            id.clone(),
            Upload {
                id: id.clone(),
                owner: who.to_string(),
                to: to.to_string(),
                size,
                name: name.to_string(),
                ttl_s: ttl,
                temp_name,
                file,
                digest: Sha256::default(),
                tail: Vec::new(),
                received: 0,
                next_chunk: 0,
                started: now,
                touched: now,
                connection,
            },
        );
        self.rate.entry(who.to_string()).or_default().push_back(now);
        Ok(json!({"v":1,"upload_id":id,"chunk_max":CHUNK_MAX}))
    }

    pub fn chunk(
        &mut self,
        who: &str,
        connection: u64,
        p: &Map<String, Value>,
    ) -> Result<Value, ApiError> {
        reject_unknown(p, &["upload_id", "n", "data"])?;
        let id = req_string(p, "upload_id")?;
        let n = protocol::req_u64(p, "n")?;
        let encoded = req_string(p, "data")?;
        let Some(u) = self.uploads.get(id) else {
            return Err(api("not_found", "upload nao encontrado"));
        };
        if u.owner != who || u.connection != connection {
            return Err(api("not_found", "upload nao encontrado"));
        }
        let data = match base64_decode(encoded) {
            Some(d) if d.len() <= CHUNK_MAX => d,
            _ => {
                self.abort(id);
                return Err(ApiError::invalid(
                    "/data",
                    "data base64 invalido ou acima do limite",
                ));
            }
        };
        let u = self.uploads.get_mut(id).expect("upload checked");
        if n != u.next_chunk
            || n >= UPLOAD_MAX_CHUNKS
            || u.received.saturating_add(data.len() as u64) > u.size
        {
            self.abort(id);
            return Err(ApiError::invalid(
                "/n",
                "pedaco fora de ordem ou tamanho excedido; sessao abortada",
            ));
        }
        let mut scan = std::mem::take(&mut u.tail);
        scan.extend_from_slice(&data);
        if has_secret(&scan) {
            self.abort(id);
            return Err(api("secret_suspected", "padrao sensivel detectado"));
        }
        let keep = scan.len().min(64);
        u.tail = scan[scan.len() - keep..].to_vec();
        if u.file.write_all(&data).is_err() {
            self.abort(id);
            return Err(api("disk_full", "falha ao gravar pedaco do anexo"));
        }
        u.digest.update(&data);
        u.received += data.len() as u64;
        u.next_chunk += 1;
        u.touched = Instant::now();
        Ok(json!({"v":1,"received":u.received,"next":u.next_chunk}))
    }

    pub fn end(
        &mut self,
        who: &str,
        connection: u64,
        p: &Map<String, Value>,
        emit: &mut EventSink<'_>,
    ) -> Result<Value, ApiError> {
        reject_unknown(p, &["upload_id", "sha256"])?;
        let id = req_string(p, "upload_id")?;
        let expected = req_string(p, "sha256")?;
        let Some(u) = self.uploads.get(id) else {
            return Err(api("not_found", "upload nao encontrado"));
        };
        if u.owner != who || u.connection != connection {
            return Err(api("not_found", "upload nao encontrado"));
        }
        if !valid_digest(expected) {
            self.abort(id);
            return Err(ApiError::invalid("/sha256", "sha256 invalido"));
        }
        if u.received != u.size || (u.size == 0 && u.next_chunk != 0) {
            self.abort(id);
            return Err(api(
                "invalid_envelope",
                "tamanho recebido diferente do declarado",
            ));
        }
        if has_secret(&u.tail) {
            self.abort(id);
            return Err(api("secret_suspected", "padrao sensivel detectado"));
        }
        let actual = u.digest.clone().finalize_hex();
        if actual != expected {
            self.abort(id);
            return Err(api("hash_mismatch", "sha256 divergente"));
        }
        let id_hex = random_hex()?;
        let blob = format!("blob:{id_hex}");
        let shard = self.shard(&blob, true)?.expect("created shard");
        let (from, sync_result) = {
            let upload = self.uploads.get_mut(id).expect("upload checked");
            let from = c_name(&upload.temp_name)?;
            let result = upload
                .file
                .flush()
                .map_err(|_| api("disk_full", "falha ao sincronizar anexo"))
                .and_then(|()| fsync(&upload.file));
            (from, result)
        };
        if let Err(error) = sync_result {
            self.abort(id);
            return Err(error);
        }
        let to = c_name(&id_hex)?;
        // SAFETY: root and shard descriptors are held open; both names are generated components.
        if unsafe {
            libc::renameat2(
                self.root.as_raw_fd(),
                from.as_ptr(),
                shard.as_raw_fd(),
                to.as_ptr(),
                libc::RENAME_NOREPLACE,
            )
        } != 0
        {
            self.abort(id);
            return Err(api("disk_full", "falha ao finalizar arquivo de anexo"));
        }
        if let Err(error) = fsync(&shard) {
            let c = CString::new(id_hex).expect("generated id");
            // SAFETY: shard descriptor is live and unlinkat does not follow symlinks.
            unsafe {
                libc::unlinkat(shard.as_raw_fd(), c.as_ptr(), 0);
            }
            let _ = shard.sync_all();
            self.abort(id);
            return Err(error);
        }
        let upload = self.uploads.remove(id).expect("upload checked");
        let created_at = now_ts();
        let record = BlobRecord {
            id: blob.clone(),
            owner: upload.owner,
            to: upload.to,
            size: upload.size,
            name: upload.name,
            sha256: actual,
            created_at,
            ttl_s: upload.ttl_s,
            read_by: Default::default(),
        };
        if let Err(e) = emit("blob_put", put_fields(&record)) {
            let c = CString::new(id_hex).expect("generated id");
            // SAFETY: shard descriptor is live and unlinkat does not follow symlinks.
            unsafe {
                libc::unlinkat(shard.as_raw_fd(), c.as_ptr(), 0);
            }
            let _ = shard.sync_all();
            return Err(e);
        }
        let expires = expiry(&record);
        self.active.insert(
            blob.clone(),
            ActiveBlob {
                record: record.clone(),
                expires,
            },
        );
        let expires_at = (chrono::DateTime::parse_from_rfc3339(&record.created_at)
            .expect("generated timestamp")
            .with_timezone(&chrono::Utc)
            + chrono::Duration::seconds(record.ttl_s as i64))
        .to_rfc3339_opts(chrono::SecondsFormat::Millis, true);
        Ok(json!({"v":1,"blob":blob,"size":record.size,"expires_at":expires_at}))
    }

    pub fn get(
        &mut self,
        who: &str,
        p: &Map<String, Value>,
        emit: &mut EventSink<'_>,
    ) -> Result<Value, ApiError> {
        reject_unknown(p, &["blob", "offset", "length"])?;
        let id = req_string(p, "blob")?;
        if !blob_id(id) {
            return Err(ApiError::invalid("/blob", "id de blob invalido"));
        }
        let offset = protocol::opt_u64(p, "offset")?.unwrap_or(0);
        let requested_length = protocol::opt_u64(p, "length")?;
        if requested_length.is_some_and(|length| length > CHUNK_MAX as u64)
            || requested_length.is_some_and(|length| offset.checked_add(length).is_none())
        {
            return Err(ApiError::invalid("/length", "intervalo de blob invalido"));
        }
        if !charge_sliding_rate(
            self.get_rate.entry(who.to_string()).or_default(),
            Instant::now(),
            GET_RATE,
            GET_WINDOW,
        ) {
            return Err(api("rate_limited", "limite de blob.get excedido"));
        }
        self.expire_due(emit)?;
        let Some(active) = self.active.get(id) else {
            return Err(api("not_found", "blob nao encontrado"));
        };
        if active.record.owner != who && active.record.to != who {
            return Err(api("not_found", "blob nao encontrado"));
        }
        let record = active.record.clone();
        let length = requested_length
            .unwrap_or_else(|| (record.size.saturating_sub(offset)).min(CHUNK_MAX as u64));
        if offset > record.size
            || length > CHUNK_MAX as u64
            || offset
                .checked_add(length)
                .is_none_or(|end| end > record.size)
        {
            return Err(ApiError::invalid("/length", "intervalo de blob invalido"));
        }
        let shard = self
            .shard(id, false)?
            .ok_or_else(|| api("not_found", "blob nao encontrado"))?;
        let mut file = openat_file(&shard, &id[5..], libc::O_RDONLY, 0)
            .map_err(|_| api("not_found", "blob nao encontrado"))?;
        file.seek(SeekFrom::Start(offset))
            .map_err(|_| api("not_found", "blob nao encontrado"))?;
        let mut data = vec![0; length as usize];
        file.read_exact(&mut data)
            .map_err(|_| api("not_found", "blob nao encontrado"))?;
        self.record_first_read(who, &record, offset, length, emit)?;
        Ok(json!({"v":1,"size":record.size,"offset":offset,"data":base64_encode(&data)}))
    }

    pub fn remove(
        &mut self,
        who: &str,
        p: &Map<String, Value>,
        emit: &mut EventSink<'_>,
    ) -> Result<Value, ApiError> {
        reject_unknown(p, &["blob"])?;
        let id = req_string(p, "blob")?;
        if !blob_id(id) {
            return Err(ApiError::invalid("/blob", "id de blob invalido"));
        }
        self.expire_due(emit)?;
        let Some(active) = self.active.get(id) else {
            return Err(api("not_found", "blob nao encontrado"));
        };
        if active.record.owner != who && active.record.to != who {
            return Err(api("not_found", "blob nao encontrado"));
        }
        let record = active.record.clone();
        self.remove_durable(emit, &record, "requested", false)?;
        Ok(json!({"v":1,"removed":true}))
    }

    pub fn list(
        &mut self,
        who: &str,
        p: &Map<String, Value>,
        emit: &mut EventSink<'_>,
    ) -> Result<Value, ApiError> {
        reject_unknown(p, &["after"])?;
        self.expire_due(emit)?;
        let after = protocol::opt_str(p, "after")?.unwrap_or("");
        if !after.is_empty() && !blob_id(after) {
            return Err(ApiError::invalid("/after", "cursor de blob invalido"));
        }
        let now = Instant::now();
        let mut all: Vec<&ActiveBlob> = self
            .active
            .values()
            .filter(|b| {
                (b.record.owner == who || b.record.to == who) && b.record.id.as_str() > after
            })
            .collect();
        all.sort_by(|a, b| a.record.id.cmp(&b.record.id));
        let more = all.len() > 50;
        all.truncate(50);
        let items: Vec<Value> = all.iter().map(|b| json!({
            "blob":b.record.id,"owner":b.record.owner,"to":b.record.to,"size":b.record.size,"name":b.record.name,
            "sha256":b.record.sha256,"expires_in":b.expires.saturating_duration_since(now).as_secs()
        })).collect();
        let next = if more {
            items
                .last()
                .and_then(|v| v["blob"].as_str())
                .map(str::to_string)
        } else {
            None
        };
        Ok(json!({"v":1,"blobs":items,"next":next}))
    }

    pub fn cleanup_tick(&mut self, emit: &mut EventSink<'_>) {
        let _ = self.expire_due(emit);
    }
}

fn record_expires(record: &BlobRecord) -> bool {
    expiry(record) <= Instant::now()
}

fn available_bytes(root: &File) -> Option<u64> {
    let mut st = std::mem::MaybeUninit::<libc::statvfs>::uninit();
    // SAFETY: fstatvfs writes a statvfs value into a valid allocation.
    if unsafe { libc::fstatvfs(root.as_raw_fd(), st.as_mut_ptr()) } != 0 {
        return None;
    }
    // SAFETY: fstatvfs succeeded.
    let st = unsafe { st.assume_init() };
    Some(st.f_bavail.saturating_mul(st.f_frsize))
}

fn has_secret(data: &[u8]) -> bool {
    [b"-----BEGIN".as_slice(), b"sk-", b"ghp_", b"refresh_token"]
        .iter()
        .any(|needle| data.windows(needle.len()).any(|w| w == *needle))
}

pub fn denied_filename(name: &str) -> bool {
    let lower = name.to_ascii_lowercase();
    matches!(
        lower.as_str(),
        ".env" | ".credentials.json" | "auth.json" | "config.toml" | ".netrc"
    ) || lower.ends_with(".token")
        || lower.ends_with(".pem")
        || lower.ends_with(".p12")
        || lower.ends_with(".key")
        || lower.starts_with("id_")
}

pub fn base64_encode(data: &[u8]) -> String {
    const T: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(data.len().div_ceil(3) * 4);
    for c in data.chunks(3) {
        let b = [c[0], *c.get(1).unwrap_or(&0), *c.get(2).unwrap_or(&0)];
        let n = ((b[0] as u32) << 16) | ((b[1] as u32) << 8) | b[2] as u32;
        for k in 0..4 {
            if k <= c.len() {
                out.push(T[((n >> (18 - 6 * k)) & 63) as usize] as char);
            } else {
                out.push('=');
            }
        }
    }
    out
}

pub fn base64_decode(data: &str) -> Option<Vec<u8>> {
    if !data.len().is_multiple_of(4) {
        return None;
    }
    let mut out = Vec::with_capacity(data.len() / 4 * 3);
    for (idx, block) in data.as_bytes().as_chunks::<4>().0.iter().enumerate() {
        let last = idx + 1 == data.len() / 4;
        let a = b64_value(block[0])? as u32;
        let b = b64_value(block[1])? as u32;
        let c = if block[2] == b'=' {
            if !last || block[3] != b'=' {
                return None;
            }
            0
        } else {
            b64_value(block[2])? as u32
        };
        let d = if block[3] == b'=' {
            if !last {
                return None;
            }
            0
        } else {
            b64_value(block[3])? as u32
        };
        if block[2] == b'=' && (b & 0x0f) != 0
            || block[3] == b'=' && block[2] != b'=' && (c & 0x03) != 0
        {
            return None;
        }
        let n = (a << 18) | (b << 12) | (c << 6) | d;
        out.push((n >> 16) as u8);
        if block[2] != b'=' {
            out.push((n >> 8) as u8);
        }
        if block[3] != b'=' {
            out.push(n as u8);
        }
    }
    Some(out)
}

fn b64_value(b: u8) -> Option<u8> {
    match b {
        b'A'..=b'Z' => Some(b - b'A'),
        b'a'..=b'z' => Some(b - b'a' + 26),
        b'0'..=b'9' => Some(b - b'0' + 52),
        b'+' => Some(62),
        b'/' => Some(63),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn base64_roundtrip_and_rejects_noncanonical_padding() {
        for v in [b"".as_slice(), b"a", b"ab", b"abc", b"binary\0bytes"] {
            assert_eq!(base64_decode(&base64_encode(v)).as_deref(), Some(v));
        }
        assert!(base64_decode("Zh==").is_none());
    }

    #[test]
    fn cota_de_contagem_por_dono_e_trinta_e_dois() {
        let dir = tempfile::tempdir().unwrap();
        let records: Vec<BlobRecord> = (0..OWNER_MAX_COUNT)
            .map(|i| BlobRecord {
                id: format!("blob:{i:032x}"),
                owner: "a".into(),
                to: "b".into(),
                size: 0,
                name: "x".into(),
                sha256: "0".repeat(64),
                created_at: now_ts(),
                ttl_s: TTL_DEFAULT,
                read_by: Default::default(),
            })
            .collect();
        assert_eq!(records.len(), OWNER_MAX_COUNT);
        let mut manager = BlobManager::open(dir.path(), Vec::new()).unwrap();
        for record in records {
            manager.active.insert(
                record.id.clone(),
                ActiveBlob {
                    expires: Instant::now() + Duration::from_secs(TTL_DEFAULT),
                    record,
                },
            );
        }
        let mut store = Store::open(dir.path(), Default::default()).unwrap();
        let params = json!({"size":0,"to":"b"}).as_object().unwrap().clone();
        let mut emit = |kind, fields| store.blob_event(kind, fields);
        let err = manager.begin("a", 1, &params, true, &mut emit).unwrap_err();
        assert_eq!(err.code, "quota_exceeded");
    }
}

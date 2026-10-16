use std::{
    fs::{self, File, OpenOptions, Permissions},
    io::{self, Read, Seek, SeekFrom, Write},
    os::unix::fs::{MetadataExt, OpenOptionsExt, PermissionsExt},
    path::{Path, PathBuf},
};

use chrono::{DateTime, Utc};

use crate::{Clock, LogFile, OpenError, Storage};

pub struct FsStorage {
    dir: PathBuf,
    lock_file: Option<File>,
}

impl FsStorage {
    pub fn new(dir: &Path) -> FsStorage {
        Self {
            dir: dir.to_path_buf(),
            lock_file: None,
        }
    }

    fn validate_dir(&self) -> Result<(), OpenError> {
        let metadata = fs::symlink_metadata(&self.dir)?;
        if !metadata.is_dir() || metadata.file_type().is_symlink() {
            return Err(OpenError::InsecurePermissions(
                "diretorio ausente, nao regular ou symlink".into(),
            ));
        }
        if metadata.mode() & 0o777 != 0o700 {
            return Err(OpenError::InsecurePermissions(
                "diretorio permite acesso a grupo/outros".into(),
            ));
        }
        Ok(())
    }

    fn process_uid() -> Result<u32, OpenError> {
        Ok(fs::metadata("/proc/self")?.uid())
    }

    fn validate_file(file: &File, label: &str) -> Result<(), OpenError> {
        let metadata = file.metadata()?;
        let uid = Self::process_uid()?;
        if !metadata.is_file() || metadata.mode() & 0o077 != 0 || metadata.uid() != uid {
            return Err(OpenError::InsecurePermissions(format!(
                "{label} nao e arquivo privado do uid do processo"
            )));
        }
        Ok(())
    }

    fn open_private_file(&self, name: &str) -> Result<(File, bool), OpenError> {
        let path = self.dir.join(name);
        self.validate_existing_file(&path, name)?;
        let flags = libc::O_CLOEXEC | libc::O_NOFOLLOW;
        let mut create = OpenOptions::new();
        create
            .read(true)
            .write(true)
            .create_new(true)
            .append(name == "log.jsonl")
            .mode(0o600)
            .custom_flags(flags);

        match create.open(&path) {
            Ok(file) => {
                file.set_permissions(Permissions::from_mode(0o600))?;
                Self::validate_file(&file, name)?;
                Ok((file, true))
            }
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {
                let mut open = OpenOptions::new();
                open.read(true)
                    .write(true)
                    .append(name == "log.jsonl")
                    .custom_flags(flags);
                let file = open.open(&path).map_err(map_open_error)?;
                Self::validate_file(&file, name)?;
                Ok((file, false))
            }
            Err(error) => Err(map_open_error(error)),
        }
    }

    fn validate_existing_file(&self, path: &Path, label: &str) -> Result<(), OpenError> {
        let metadata = match fs::symlink_metadata(path) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
            Err(error) => return Err(OpenError::Io(error)),
        };
        let uid = Self::process_uid()?;
        if !metadata.is_file()
            || metadata.file_type().is_symlink()
            || metadata.mode() & 0o777 != 0o600
            || metadata.uid() != uid
        {
            return Err(OpenError::InsecurePermissions(format!(
                "{label} nao e arquivo regular 0600 do uid do processo"
            )));
        }
        Ok(())
    }
}

impl Storage for FsStorage {
    fn lock(&mut self) -> Result<(), OpenError> {
        self.validate_dir()?;
        let (file, _) = self.open_private_file("LOCK")?;
        let result = unsafe {
            libc::flock(
                std::os::fd::AsRawFd::as_raw_fd(&file),
                libc::LOCK_EX | libc::LOCK_NB,
            )
        };
        if result != 0 {
            let error = io::Error::last_os_error();
            if error.kind() == io::ErrorKind::WouldBlock {
                return Err(OpenError::AlreadyRunning);
            }
            return Err(OpenError::Io(error));
        }
        self.lock_file = Some(file);
        Ok(())
    }

    fn open_log(&mut self) -> Result<Box<dyn LogFile>, OpenError> {
        self.validate_dir()?;
        if self.lock_file.is_none() {
            return Err(OpenError::Io(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "lock deve ser adquirido antes de abrir o log",
            )));
        }
        let (file, created) = self.open_private_file("log.jsonl")?;
        if created {
            File::open(&self.dir)?.sync_all()?;
        }
        Ok(Box::new(FsLogFile { file }))
    }
}

struct FsLogFile {
    file: File,
}

impl LogFile for FsLogFile {
    fn read_all(&mut self) -> io::Result<Vec<u8>> {
        self.file.seek(SeekFrom::Start(0))?;
        let mut bytes = Vec::new();
        self.file.read_to_end(&mut bytes)?;
        Ok(bytes)
    }

    fn write_all(&mut self, buf: &[u8]) -> io::Result<()> {
        self.file.write_all(buf)
    }

    fn sync_data(&mut self) -> io::Result<()> {
        self.file.sync_data()
    }

    fn sync_all(&mut self) -> io::Result<()> {
        self.file.sync_all()
    }

    fn set_len(&mut self, len: u64) -> io::Result<()> {
        self.file.set_len(len)
    }

    fn len(&mut self) -> io::Result<u64> {
        Ok(self.file.metadata()?.len())
    }
}

pub struct SystemClock;

impl Clock for SystemClock {
    fn now(&self) -> DateTime<Utc> {
        Utc::now()
    }
}

fn map_open_error(error: io::Error) -> OpenError {
    match error.raw_os_error() {
        Some(libc::ELOOP) | Some(libc::EISDIR) => {
            OpenError::InsecurePermissions("caminho e symlink ou diretorio".into())
        }
        _ => OpenError::Io(error),
    }
}

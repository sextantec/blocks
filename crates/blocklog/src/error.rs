use std::{error::Error, fmt, io};

#[derive(Debug)]
pub enum OpenError {
    AlreadyRunning,
    InsecurePermissions(String),
    Corrupt { line: usize, reason: String },
    BadOptions(String),
    Io(io::Error),
}

impl fmt::Display for OpenError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::AlreadyRunning => f.write_str("ja existe outro dono do log"),
            Self::InsecurePermissions(reason) => write!(f, "permissoes inseguras: {reason}"),
            Self::Corrupt { line, reason } => write!(f, "log corrompido na linha {line}: {reason}"),
            Self::BadOptions(reason) => write!(f, "opcoes invalidas: {reason}"),
            Self::Io(error) => write!(f, "erro de E/S: {error}"),
        }
    }
}

impl Error for OpenError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Io(error) => Some(error),
            _ => None,
        }
    }
}

impl From<io::Error> for OpenError {
    fn from(error: io::Error) -> Self {
        Self::Io(error)
    }
}

#[derive(Debug)]
pub enum CommitError {
    Broken,
    TooLarge,
    TxTooLarge,
    EmptyTx,
    InvalidInput(String),
    LogFull,
    RequestConflict,
    MissingRes,
    Io(io::Error),
}

impl fmt::Display for CommitError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Broken => f.write_str("log indisponivel apos falha de escrita"),
            Self::TooLarge => f.write_str("linha acima do limite"),
            Self::TxTooLarge => f.write_str("transacao acima do limite"),
            Self::EmptyTx => f.write_str("transacao vazia"),
            Self::InvalidInput(reason) => write!(f, "entrada invalida: {reason}"),
            Self::LogFull => f.write_str("log atingiu o limite"),
            Self::RequestConflict => f.write_str("request_id reutilizado com carga diferente"),
            Self::MissingRes => f.write_str("req exige res"),
            Self::Io(error) => write!(f, "erro de E/S: {error}"),
        }
    }
}

impl Error for CommitError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Io(error) => Some(error),
            _ => None,
        }
    }
}

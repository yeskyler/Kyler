use std::fmt;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ErrorKind {
    InvalidInput,
    Transport,
    Api,
    Protocol,
}
#[derive(Debug, Clone)]
pub struct Error {
    pub kind: ErrorKind,
    pub code: String,
    pub message: String,
    pub status: Option<u16>,
}
pub type Result<T> = std::result::Result<T, Error>;
impl Error {
    pub fn new(kind: ErrorKind, code: &str, message: impl Into<String>) -> Self {
        Self {
            kind,
            code: code.into(),
            message: message.into(),
            status: None,
        }
    }
    pub fn transport() -> Self {
        Self::new(
            ErrorKind::Transport,
            "connection_failed",
            "Cannot reach Kyler server. Start `kyler serve` or check --server and authentication settings.",
        )
    }
    pub fn protocol(message: &str) -> Self {
        Self::new(ErrorKind::Protocol, "invalid_response", message)
    }
    pub fn invalid(message: &str) -> Self {
        Self::new(ErrorKind::InvalidInput, "invalid_input", message)
    }
}
impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {}", self.code, self.message)
    }
}
impl std::error::Error for Error {}

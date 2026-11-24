#[derive(Clone)]
pub struct Binding {
    pub chat: String,
    pub token: String,
    pub workspace: String,
    pub session: String,
}

pub(crate) fn pairing_digest(code: &str) -> Vec<u8> {
    use sha2::{Digest, Sha256};

    Sha256::digest(code.as_bytes()).to_vec()
}

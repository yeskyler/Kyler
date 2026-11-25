use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IncomingAttachment {
    pub name: String,
    pub url: String,
    pub content_type: String,
    pub size: Option<u64>,
}

pub struct Incoming {
    pub update: String,
    pub chat: String,
    pub text: String,
    pub attachments: Vec<IncomingAttachment>,
}

/// Immutable session artifact reference, never a workspace path.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OutgoingFile {
    pub session: String,
    pub artifact: String,
}

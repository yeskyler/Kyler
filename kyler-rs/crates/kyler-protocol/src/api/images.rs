//! Bounded inline image inputs for Chat Completions-compatible backends.
use std::fmt::{Debug, Formatter};

use serde::{Deserialize, Serialize};

pub const MAX_IMAGES: usize = 4;
pub const MAX_IMAGE_BYTES: usize = 5 * 1024 * 1024;
pub const MAX_IMAGE_REQUEST_BYTES: usize = 29 * 1024 * 1024;

#[derive(Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ImageInput {
    pub name: String,
    /// Base64-encoded file bytes. The server detects the format from the bytes.
    pub data: String,
}
impl Debug for ImageInput {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ImageInput")
            .field("name", &self.name)
            .field("encoded_bytes", &self.data.len())
            .finish()
    }
}

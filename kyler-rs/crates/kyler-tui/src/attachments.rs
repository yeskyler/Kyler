//! Local attachment drafts. Temporary files live through admission retries.
use std::{
    fs::File,
    io::{Cursor, Read, Write},
    path::{Path, PathBuf},
    sync::Arc,
};

use anyhow::{Context, Result, ensure};

use kyler_protocol::api::attachments::MAX_ATTACHMENT_BYTES;

#[derive(Clone, Debug)]
pub struct LocalAttachment {
    pub path: PathBuf,
    pub name: String,
    // Drop removes the private temporary file after the draft/retry is released.
    _temporary: Arc<tempfile::TempPath>,
}
impl LocalAttachment {
    pub fn read_bytes(&self) -> Result<Vec<u8>> {
        let file = File::open(&self.path).context("Cannot read the temporary attachment.")?;
        let mut bytes = Vec::new();
        file.take(MAX_ATTACHMENT_BYTES as u64 + 1)
            .read_to_end(&mut bytes)?;
        ensure!(
            bytes.len() <= MAX_ATTACHMENT_BYTES,
            "Attachment exceeds 20 MiB"
        );
        Ok(bytes)
    }
}
pub enum Paste {
    Image(LocalAttachment),
    Text(String),
}

fn store(name: String, bytes: Vec<u8>) -> Result<LocalAttachment> {
    ensure!(
        name.len() <= 255 && !name.chars().any(char::is_control),
        "Invalid image filename."
    );
    ensure!(
        bytes.len() <= MAX_ATTACHMENT_BYTES,
        "Images must be at most 20 MiB."
    );
    let mut file = tempfile::Builder::new()
        .prefix("kyler-image-")
        .suffix(".png")
        .tempfile()?;
    file.write_all(&bytes)?;
    let path = file.into_temp_path();
    Ok(LocalAttachment {
        path: path.to_path_buf(),
        name,
        _temporary: Arc::new(path),
    })
}
pub fn from_path(path: &Path) -> Result<LocalAttachment> {
    let file = File::open(path).context("Cannot open the image file.")?;
    ensure!(
        file.metadata()?.is_file(),
        "Attachment must be a regular file."
    );
    let mut bytes = Vec::new();
    file.take(MAX_ATTACHMENT_BYTES as u64 + 1)
        .read_to_end(&mut bytes)?;
    let name = path
        .file_name()
        .context("Missing image filename.")?
        .to_string_lossy()
        .into_owned();
    store(name, bytes)
}
pub fn from_rgba(width: usize, height: usize, bytes: &[u8]) -> Result<LocalAttachment> {
    ensure!(
        width > 0
            && height > 0
            && width <= 8192
            && height <= 8192
            && width
                .checked_mul(height)
                .is_some_and(|n| n <= 32 * 1024 * 1024),
        "Clipboard image is too large."
    );
    ensure!(
        width.checked_mul(height).and_then(|n| n.checked_mul(4)) == Some(bytes.len()),
        "Invalid clipboard image."
    );
    let buffer = image::RgbaImage::from_raw(width as u32, height as u32, bytes.to_vec())
        .context("Invalid clipboard image.")?;
    let image = image::DynamicImage::ImageRgba8(buffer).thumbnail(2048, 2048);
    let mut png = Cursor::new(Vec::new());
    image.write_to(&mut png, image::ImageFormat::Png)?;
    store("screenshot.png".into(), png.into_inner())
}
pub fn clipboard() -> Result<Paste> {
    let mut clipboard = arboard::Clipboard::new().context("Cannot access the system clipboard.")?;
    if let Ok(image) = clipboard.get_image() {
        return from_rgba(image.width, image.height, &image.bytes).map(Paste::Image);
    }
    let text = clipboard
        .get_text()
        .context("Clipboard contains no supported image or text.")?;
    if let Some(path) = pasted_path(&text) {
        return from_path(&path).map(Paste::Image);
    }
    Ok(Paste::Text(text))
}
pub fn pasted_path(text: &str) -> Option<PathBuf> {
    let text = text.trim().trim_matches(['\"', '\'']).replace("\\ ", " ");
    let path = if let Some(relative) = text.strip_prefix("~/") {
        PathBuf::from(std::env::var_os("HOME")?).join(relative)
    } else {
        PathBuf::from(text)
    };
    let extension = path.extension()?.to_str()?.to_ascii_lowercase();
    (path.is_absolute()
        && matches!(
            extension.as_str(),
            "png"
                | "jpg"
                | "jpeg"
                | "webp"
                | "pdf"
                | "docx"
                | "xlsx"
                | "csv"
                | "tsv"
                | "txt"
                | "md"
                | "json"
                | "rs"
                | "py"
                | "js"
                | "toml"
        ))
    .then_some(path)
}

#[cfg(test)]
mod tests {
    use super::{from_path, from_rgba, pasted_path};

    use std::path::PathBuf;

    #[test]
    fn screenshot_and_file_share_the_same_payload_and_temporary_lifetime() {
        let screenshot = from_rgba(1, 1, &[255, 0, 0, 255]).unwrap();
        let path = screenshot.path.clone();
        let original = screenshot.read_bytes().unwrap();
        let file = from_path(&path).unwrap();
        assert_eq!(original, file.read_bytes().unwrap());
        let retained = screenshot.clone();
        drop(screenshot);
        assert!(path.exists());
        drop(retained);
        assert!(!path.exists());
        assert!(from_rgba(1, 1, &[]).is_err());
        assert!(from_rgba(8193, 1, &[]).is_err());
    }
    #[test]
    fn detects_image_paths_without_consuming_regular_text() {
        assert_eq!(
            pasted_path("'/tmp/image one.png'"),
            Some(PathBuf::from("/tmp/image one.png"))
        );
        assert_eq!(
            pasted_path("/tmp/image\\ one.png"),
            Some(PathBuf::from("/tmp/image one.png"))
        );
        assert!(pasted_path("describe the picture").is_none());
        assert!(pasted_path("https://example.com/image.png").is_none());
    }
}

//! Private session-scoped blobs. Models and clients never select server paths.
use super::document::{Extracted, detect, extract};
use crate::{attachments::image::UserImage, environment::ExecutionEnvironment};

use std::{
    fs,
    io::Write,
    path::{Path, PathBuf},
};

use anyhow::{Context, Result, ensure};
use base64::{Engine, engine::general_purpose::STANDARD};
use sha2::{Digest, Sha256};
use uuid::Uuid;

use kyler_protocol::api::{
    attachments::{
        AttachmentContent, AttachmentInfo, AttachmentKind, MAX_ATTACHMENT_BYTES,
        ReadAttachmentRequest,
    },
    images::ImageInput,
};

#[derive(Clone)]
pub struct AttachmentStore {
    root: PathBuf,
}
impl AttachmentStore {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }
    fn session(&self, session: &str) -> Result<PathBuf> {
        ensure!(
            Uuid::parse_str(session)?.to_string() == session,
            "Invalid session ID"
        );
        Ok(self.root.join(session))
    }
    fn location(&self, session: &str, id: &str) -> Result<PathBuf> {
        ensure!(
            Uuid::parse_str(id)?.to_string() == id,
            "Invalid attachment ID"
        );
        let path = self.session(session)?.join(id);
        ensure!(
            fs::symlink_metadata(&path)?.is_dir(),
            "Attachment is unavailable"
        );
        Ok(path)
    }
    pub fn get(&self, session: &str, id: &str) -> Result<AttachmentInfo> {
        let info: AttachmentInfo = serde_json::from_slice(&fs::read(
            self.location(session, id)?.join("metadata.json"),
        )?)?;
        ensure!(info.id == id, "Attachment identity mismatch");
        Ok(info)
    }
    pub fn list(&self, session: &str) -> Result<Vec<AttachmentInfo>> {
        let root = self.session(session)?;
        if !root.exists() {
            return Ok(Vec::new());
        }
        let mut items = Vec::new();
        for entry in fs::read_dir(root)? {
            let entry = entry?;
            if let Ok(info) = self.get(session, &entry.file_name().to_string_lossy()) {
                items.push(info);
            }
        }
        items.sort_by_key(|item| item.created_at);
        Ok(items)
    }
    pub fn put(&self, session: &str, name: &str, bytes: &[u8]) -> Result<AttachmentInfo> {
        ensure!(
            !name.is_empty()
                && name.len() <= 255
                && !name.chars().any(char::is_control)
                && !name.contains(['/', '\\']),
            "Invalid attachment name"
        );
        ensure!(
            !bytes.is_empty() && bytes.len() <= MAX_ATTACHMENT_BYTES,
            "Attachment must contain 1 byte to 20 MiB"
        );
        let kind = detect(name, bytes)?;
        if kind == AttachmentKind::Image {
            crate::attachments::image::prepare_images(&[ImageInput {
                name: name.into(),
                data: STANDARD.encode(bytes),
            }])?;
        }
        let root = self.session(session)?;
        let digest = format!("{:x}", Sha256::digest(bytes));
        if let Some(existing) = self
            .list(session)?
            .into_iter()
            .find(|item| item.sha256 == digest && item.name == name)
        {
            return Ok(existing);
        }
        fs::create_dir_all(&root)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;

            fs::set_permissions(&root, fs::Permissions::from_mode(0o700))?;
        }
        ensure!(
            self.list(session)?.len() < 128,
            "Session attachment limit reached"
        );
        let directory = tempfile::Builder::new()
            .prefix(".upload-")
            .tempdir_in(&root)?;
        let info = AttachmentInfo {
            id: Uuid::new_v4().to_string(),
            name: name.into(),
            kind,
            bytes: bytes.len(),
            sha256: format!("{:x}", Sha256::digest(bytes)),
            created_at: chrono::Utc::now().timestamp_millis(),
        };
        private_write(&directory.path().join("blob"), bytes)?;
        private_write(
            &directory.path().join("metadata.json"),
            &serde_json::to_vec(&info)?,
        )?;
        fs::rename(directory.path(), root.join(&info.id))?;
        Ok(info)
    }
    pub fn download(&self, session: &str, id: &str) -> Result<(AttachmentInfo, Vec<u8>)> {
        let info = self.get(session, id)?;
        let bytes = if info.kind == AttachmentKind::Image {
            let image = self.image(session, id)?;
            STANDARD.decode(image.data_url.split_once(',').context("Invalid image")?.1)?
        } else {
            fs::read(self.location(session, id)?.join("blob"))?
        };
        if info.kind != AttachmentKind::Image {
            ensure!(
                format!("{:x}", Sha256::digest(&bytes)) == info.sha256,
                "Attachment integrity check failed"
            );
        }
        Ok((info, bytes))
    }
    pub fn image(&self, session: &str, id: &str) -> Result<UserImage> {
        let info = self.get(session, id)?;
        ensure!(
            info.kind == AttachmentKind::Image,
            "Attachment is not an image"
        );
        let bytes = fs::read(self.location(session, id)?.join("blob"))?;
        ensure!(
            format!("{:x}", Sha256::digest(&bytes)) == info.sha256,
            "Attachment integrity check failed"
        );
        Ok(crate::attachments::image::prepare_images(&[ImageInput {
            name: info.name,
            data: STANDARD.encode(bytes),
        }])?
        .remove(0))
    }
    pub async fn read_isolated(
        &self,
        session: &str,
        args: &ReadAttachmentRequest,
        environment: &ExecutionEnvironment,
    ) -> Result<AttachmentContent> {
        let info = self.get(session, &args.attachment_id)?;
        let raw = fs::read(self.location(session, &info.id)?.join("blob"))?;
        ensure!(
            format!("{:x}", Sha256::digest(&raw)) == info.sha256,
            "Attachment integrity check failed"
        );
        let cache = Self::new(environment.attachment_cache_directory());
        let temporary = cache.put(session, &info.name, &raw)?;
        let mut request = args.clone();
        request.attachment_id = temporary.id;
        let output = environment
            .read_only()
            .run_tool(
                "read_attachment_document",
                &serde_json::json!({"root":cache.root,"session":session,"request":request})
                    .to_string(),
            )
            .await?;
        let mut content: AttachmentContent = serde_json::from_str(&output)?;
        content.info = info;
        Ok(content)
    }
    pub fn read(&self, session: &str, args: &ReadAttachmentRequest) -> Result<AttachmentContent> {
        self.read_document(session, args, true)
    }
    pub(crate) fn read_uncached(
        &self,
        session: &str,
        args: &ReadAttachmentRequest,
    ) -> Result<AttachmentContent> {
        self.read_document(session, args, false)
    }
    fn read_document(
        &self,
        session: &str,
        args: &ReadAttachmentRequest,
        cache: bool,
    ) -> Result<AttachmentContent> {
        let info = self.get(session, &args.attachment_id)?;
        ensure!(
            info.kind != AttachmentKind::Image,
            "Images are provided directly to vision-capable models; use inspect_attachment for metadata"
        );
        let location = self.location(session, &info.id)?;
        let bytes = fs::read(location.join("blob"))?;
        ensure!(
            format!("{:x}", Sha256::digest(&bytes)) == info.sha256,
            "Attachment integrity check failed"
        );
        // Parsing is lazy and cached. The cache never becomes model instructions.
        let extracted: Extracted = if location.join("extracted.json").is_file() {
            serde_json::from_slice(&fs::read(location.join("extracted.json"))?)?
        } else {
            let extracted = extract(&info, &bytes)?;
            ensure!(
                extracted.units.len() <= 2000
                    && extracted
                        .units
                        .iter()
                        .map(|(_, text)| text.len())
                        .sum::<usize>()
                        <= 8 * 1024 * 1024,
                "Extracted document exceeds limits"
            );
            if cache {
                let mut tmp = tempfile::NamedTempFile::new_in(&location)?;
                serde_json::to_writer(&mut tmp, &extracted)?;
                tmp.flush()?;
                tmp.persist(location.join("extracted.json"))
                    .map_err(|error| error.error)?;
            }
            extracted
        };
        let (unit_name, text) = extracted
            .units
            .get(args.unit)
            .context("Document unit/page/sheet is out of range")?;
        let lines: Vec<_> = text.lines().collect();
        ensure!(args.offset <= lines.len(), "Line offset is out of range");
        let limit = args.limit.unwrap_or(100).clamp(1, 500);
        let mut content = lines
            .iter()
            .skip(args.offset)
            .take(limit)
            .map(|s| *s)
            .collect::<Vec<_>>()
            .join("\n");
        let cap = 32000;
        let clipped = content.len() > cap;
        if clipped {
            let mut end = cap;
            while !content.is_char_boundary(end) {
                end -= 1;
            }
            content.truncate(end);
        }
        Ok(AttachmentContent {
            info,
            unit: args.unit,
            units: extracted.units.len(),
            unit_name: unit_name.clone(),
            offset: args.offset,
            total_lines: lines.len(),
            content,
            truncated: clipped || args.offset + limit < lines.len(),
        })
    }
    pub fn delete(&self, session: &str, id: &str) -> Result<()> {
        fs::remove_dir_all(self.location(session, id)?)?;
        Ok(())
    }
    pub fn delete_session(&self, session: &str) -> Result<()> {
        let path = self.session(session)?;
        if path.exists() {
            fs::remove_dir_all(path)?;
        }
        Ok(())
    }
}
fn private_write(path: &Path, bytes: &[u8]) -> Result<()> {
    let mut options = fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;

        options.mode(0o600);
    }
    let mut file = options.open(path)?;
    file.write_all(bytes)?;
    file.sync_all()?;
    Ok(())
}
#[cfg(test)]
mod tests {
    use super::{super::document::validate_sheet, AttachmentStore};

    use std::io::Write;

    use uuid::Uuid;

    use kyler_protocol::api::attachments::ReadAttachmentRequest;

    #[test]
    fn attachments_are_scoped_deduplicated_bounded_and_lazy() {
        let root = tempfile::tempdir().unwrap();
        let store = AttachmentStore::new(root.path());
        let session = Uuid::new_v4().to_string();
        let other = Uuid::new_v4().to_string();
        let info = store
            .put(&session, "notes.txt", b"one\ntwo\nthree")
            .unwrap();
        assert_eq!(
            store
                .put(&session, "notes.txt", b"one\ntwo\nthree")
                .unwrap()
                .id,
            info.id
        );
        assert!(store.get(&other, &info.id).is_err());
        assert!(store.get("../", &info.id).is_err());
        assert!(store.put(&session, "../secret", b"text").is_err());
        let read = store
            .read(
                &session,
                &ReadAttachmentRequest {
                    attachment_id: info.id.clone(),
                    offset: 1,
                    limit: Some(1),
                    ..Default::default()
                },
            )
            .unwrap();
        assert_eq!(read.content, "two");
        assert!(read.truncated);
        assert_eq!(read.total_lines, 3);
        store.delete(&session, &info.id).unwrap();
        assert!(store.get(&session, &info.id).is_err());
    }
    #[test]
    fn sparse_or_implicit_spreadsheet_ranges_cannot_trigger_huge_allocations() {
        assert!(validate_sheet("<x:worksheet xmlns:x='urn:test'><x:row r='1'><x:c r='A1'/><x:c r='XFD1048576'/></x:row></x:worksheet>").is_err());
        let xml = format!(
            "<worksheet><row r='100000'>{}</row></worksheet>",
            "<c/>".repeat(11)
        );
        assert!(validate_sheet(&xml).is_err());
    }
    #[test]
    fn docx_is_parsed_as_data_and_archive_expansion_is_bounded() {
        use zip::write::SimpleFileOptions;

        let mut archive = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
        archive
            .start_file("word/document.xml", SimpleFileOptions::default())
            .unwrap();
        archive.write_all(b"<w:document xmlns:w=\"urn:test\"><w:p><w:r><w:t>Hello &amp; world</w:t><w:tab/><w:t>column</w:t></w:r></w:p></w:document>").unwrap();
        let bytes = archive.finish().unwrap().into_inner();
        let root = tempfile::tempdir().unwrap();
        let store = AttachmentStore::new(root.path());
        let session = Uuid::new_v4().to_string();
        let info = store.put(&session, "report.docx", &bytes).unwrap();
        let read = store
            .read(
                &session,
                &ReadAttachmentRequest {
                    attachment_id: info.id,
                    ..Default::default()
                },
            )
            .unwrap();
        assert_eq!(read.content, "Hello & world\tcolumn");
    }
}

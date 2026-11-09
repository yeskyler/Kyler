//! Bounded discovery of local presentation assets, never a directory crawl.
use std::path::Path;

use quick_xml::{Reader, events::Event};

pub(super) fn references(name: &str, bytes: &[u8]) -> Vec<String> {
    if bytes.len() > 2 * 1024 * 1024 {
        return Vec::new();
    }
    let extension = Path::new(name)
        .extension()
        .and_then(|s| s.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase();
    if !matches!(
        extension.as_str(),
        "html" | "htm" | "css" | "js" | "mjs" | "md" | "markdown"
    ) {
        return Vec::new();
    }
    let Ok(text) = std::str::from_utf8(bytes) else {
        return Vec::new();
    };
    let mut references = Vec::new();
    if matches!(extension.as_str(), "md" | "markdown") {
        use pulldown_cmark::{Event as MarkdownEvent, Options, Parser, Tag};
        return Parser::new_ext(text, Options::all())
            .filter_map(|event| match event {
                MarkdownEvent::Start(Tag::Link { dest_url, .. } | Tag::Image { dest_url, .. }) => {
                    Some(dest_url.into_string())
                }
                _ => None,
            })
            .filter(|reference| {
                !reference.is_empty()
                    && reference.len() <= 4096
                    && !reference.starts_with(['/', '#', '?'])
                    && !reference.contains([':', '\\'])
            })
            .take(24)
            .collect();
    }

    if matches!(extension.as_str(), "html" | "htm") {
        let mut reader = Reader::from_str(text);
        reader.config_mut().check_end_names = false;
        reader.config_mut().allow_unmatched_ends = true;
        loop {
            match reader.read_event() {
                Ok(Event::Start(tag) | Event::Empty(tag)) => {
                    for attribute in tag.attributes().with_checks(false).flatten() {
                        if matches!(attribute.key.as_ref(), b"src" | b"href" | b"poster") {
                            if let Ok(value) = attribute.decode_and_unescape_value(reader.decoder())
                            {
                                references.push(value.into_owned());
                            }
                        }
                    }
                }
                Ok(Event::Eof) | Err(_) => break,
                _ => {}
            }
        }
    }
    // CSS url() and literal JS module/data references. Dynamic URLs require a self-contained artifact.
    for tail in text.split("url(").skip(1) {
        if let Some((reference, _)) = tail.split_once(')') {
            references.push(reference.trim().trim_matches(['\'', '"']).to_owned());
        }
    }
    if matches!(extension.as_str(), "js" | "mjs" | "html" | "htm") {
        let mut chars = text.chars();
        while let Some(quote) = chars.next() {
            if !matches!(quote, '\'' | '"') {
                continue;
            }
            let mut value = String::new();
            for ch in chars.by_ref().take(4097) {
                if ch == quote {
                    break;
                }
                value.push(ch);
            }
            if asset_reference(&value) {
                references.push(value);
            }
            if references.len() >= 128 {
                break;
            }
        }
    }
    references
        .into_iter()
        .filter(|s| asset_reference(s))
        .take(24)
        .collect()
}
fn asset_reference(reference: &str) -> bool {
    if reference.is_empty()
        || reference.len() > 4096
        || reference.starts_with(['/', '#', '?'])
        || reference.contains([':', '\\'])
    {
        return false;
    }
    let reference = reference.split(['?', '#']).next().unwrap_or_default();
    matches!(
        Path::new(reference)
            .extension()
            .and_then(|s| s.to_str())
            .unwrap_or_default()
            .to_ascii_lowercase()
            .as_str(),
        "js" | "mjs"
            | "css"
            | "json"
            | "csv"
            | "png"
            | "jpg"
            | "jpeg"
            | "webp"
            | "gif"
            | "avif"
            | "svg"
            | "woff"
            | "woff2"
            | "ttf"
            | "otf"
            | "mp3"
            | "wav"
            | "mp4"
            | "webm"
            | "ogg"
    )
}
pub(super) fn resolve(owner: &str, reference: &str) -> Option<String> {
    let root = url::Url::parse("https://artifact.invalid/").ok()?;
    let base = root.join(owner).ok()?;
    let target = base.join(reference).ok()?;
    if target.origin() != root.origin() {
        return None;
    }
    let path = target.path();
    Some(if owner.starts_with('/') {
        path.to_owned()
    } else {
        path.trim_start_matches('/').to_owned()
    })
}

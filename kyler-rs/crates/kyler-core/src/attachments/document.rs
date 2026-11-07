//! Document format validation and bounded text extraction.
use std::{
    io::{Cursor, Read},
    path::Path,
};

use anyhow::{Context, Result, ensure};
use calamine::{Reader, Xlsx};
use serde::{Deserialize, Serialize};

use kyler_protocol::api::attachments::{AttachmentInfo, AttachmentKind};

#[derive(Serialize, Deserialize)]
pub(super) struct Extracted {
    pub(super) units: Vec<(String, String)>,
}
pub(super) fn detect(name: &str, bytes: &[u8]) -> Result<AttachmentKind> {
    if ::image::guess_format(bytes).is_ok() {
        return Ok(AttachmentKind::Image);
    }
    if bytes.starts_with(b"%PDF-") {
        return Ok(AttachmentKind::Pdf);
    }
    let extension = Path::new(name)
        .extension()
        .and_then(|v| v.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    if bytes.starts_with(b"PK") {
        let mut archive = zip::ZipArchive::new(Cursor::new(bytes))?;
        let mut expanded = 0u64;
        for i in 0..archive.len() {
            expanded = expanded
                .checked_add(archive.by_index(i)?.size())
                .context("Archive too large")?;
        }
        ensure!(
            expanded <= 64 * 1024 * 1024 && archive.len() <= 10000,
            "Document archive exceeds extraction limits"
        );
        if archive.by_name("word/document.xml").is_ok() {
            return Ok(AttachmentKind::Document);
        }
        if archive.by_name("xl/workbook.xml").is_ok() {
            for i in 0..archive.len() {
                let mut file = archive.by_index(i)?;
                if file.name().starts_with("xl/worksheets/") && file.name().ends_with(".xml") {
                    ensure!(
                        file.size() <= 16 * 1024 * 1024,
                        "Worksheet XML exceeds limits"
                    );
                    let mut xml = String::new();
                    file.read_to_string(&mut xml)?;
                    validate_sheet(&xml)?;
                }
            }
            return Ok(AttachmentKind::Spreadsheet);
        }
        anyhow::bail!("Unsupported document archive");
    }
    ensure!(
        std::str::from_utf8(bytes).is_ok() && !bytes.contains(&0),
        "Unsupported binary attachment"
    );
    Ok(if matches!(extension.as_str(), "csv" | "tsv") {
        AttachmentKind::Spreadsheet
    } else {
        AttachmentKind::Text
    })
}
pub(super) fn validate_sheet(xml: &str) -> Result<()> {
    let mut reader = quick_xml::Reader::from_str(xml);
    let mut max_row = 1u64;
    let mut max_col = 1u64;
    let mut count = 0u64;
    let mut current_col = 0u64;
    loop {
        match reader.read_event()? {
            quick_xml::events::Event::Start(element) | quick_xml::events::Event::Empty(element)
                if element.local_name().as_ref() == b"row" =>
            {
                current_col = 0;
                for attribute in element.attributes() {
                    let attribute = attribute?;
                    if attribute.key.as_ref() == b"r" {
                        let row = std::str::from_utf8(&attribute.value)?.parse::<u64>()?;
                        max_row = max_row.max(row);
                        ensure!(
                            max_row <= 100000 && max_row * max_col <= 1_000_000,
                            "Worksheet row limit exceeded"
                        );
                    }
                }
            }
            quick_xml::events::Event::Start(element) | quick_xml::events::Event::Empty(element)
                if element.local_name().as_ref() == b"c" =>
            {
                count += 1;
                current_col += 1;
                max_col = max_col.max(current_col);
                ensure!(
                    max_col <= 1000 && max_row * max_col <= 1_000_000,
                    "Worksheet dimensions exceed safe extraction limits"
                );
                ensure!(count <= 1_000_000, "Worksheet cell limit exceeded");
                for attribute in element.attributes() {
                    let attribute = attribute?;
                    if attribute.key.as_ref() == b"r" {
                        let reference = std::str::from_utf8(&attribute.value)?;
                        let mut col = 0u64;
                        let mut row = 0u64;
                        for ch in reference.chars() {
                            if ch.is_ascii_alphabetic() {
                                col = col
                                    .checked_mul(26)
                                    .and_then(|v| {
                                        v.checked_add(
                                            ch.to_ascii_uppercase() as u64 - 'A' as u64 + 1,
                                        )
                                    })
                                    .context("Invalid cell reference")?;
                            } else if ch.is_ascii_digit() {
                                row = row
                                    .checked_mul(10)
                                    .and_then(|v| v.checked_add(ch as u64 - '0' as u64))
                                    .context("Invalid cell reference")?;
                            } else {
                                anyhow::bail!("Invalid cell reference");
                            }
                        }
                        current_col = col;
                        max_row = max_row.max(row);
                        max_col = max_col.max(col);
                        ensure!(
                            max_row <= 100000 && max_col <= 1000 && max_row * max_col <= 1_000_000,
                            "Worksheet dimensions exceed safe extraction limits"
                        );
                    }
                }
            }
            quick_xml::events::Event::Eof => break,
            _ => {}
        }
    }
    Ok(())
}
pub(super) fn extract(info: &AttachmentInfo, bytes: &[u8]) -> Result<Extracted> {
    let units = match info.kind {
        AttachmentKind::Text => vec![("Text".into(), String::from_utf8(bytes.to_vec())?)],
        AttachmentKind::Pdf => pdf_extract::extract_text_from_mem_by_pages(bytes)?
            .into_iter()
            .enumerate()
            .map(|(i, text)| (format!("Page {}", i + 1), text))
            .collect(),
        AttachmentKind::Document => {
            let mut archive = zip::ZipArchive::new(Cursor::new(bytes))?;
            let mut xml = String::new();
            archive
                .by_name("word/document.xml")?
                .take(16 * 1024 * 1024)
                .read_to_string(&mut xml)?;
            let mut reader = quick_xml::Reader::from_str(&xml);
            let mut text = String::new();
            loop {
                match reader.read_event()? {
                    quick_xml::events::Event::Text(value) => {
                        text.push_str(&quick_xml::escape::unescape(&value.decode()?)?)
                    }
                    quick_xml::events::Event::GeneralRef(value) => {
                        if let Some(character) = value.resolve_char_ref()? {
                            text.push(character);
                        } else {
                            let name = value.decode()?;
                            let encoded = format!("&{name};");
                            text.push_str(&quick_xml::escape::unescape(&encoded)?);
                        }
                    }
                    quick_xml::events::Event::End(value) if value.name().as_ref() == b"w:p" => {
                        text.push('\n')
                    }
                    quick_xml::events::Event::Eof => break,
                    _ => {}
                }
            }
            vec![("Document".into(), text)]
        }
        AttachmentKind::Spreadsheet if bytes.starts_with(b"PK") => {
            let mut workbook: Xlsx<_> = calamine::open_workbook_from_rs(Cursor::new(bytes))?;
            let mut units = Vec::new();
            ensure!(
                workbook.sheet_names().len() <= 100,
                "Workbook has too many sheets"
            );
            for name in workbook.sheet_names().to_vec() {
                let range = workbook.worksheet_range(&name)?;
                let mut text = String::new();
                for row in range.rows().take(100000) {
                    text.push_str(
                        &row.iter()
                            .map(ToString::to_string)
                            .collect::<Vec<_>>()
                            .join("\t"),
                    );
                    text.push('\n');
                    ensure!(
                        text.len() <= 8 * 1024 * 1024,
                        "Sheet exceeds extraction limit"
                    );
                }
                units.push((name, text));
            }
            units
        }
        AttachmentKind::Spreadsheet => vec![("Table".into(), String::from_utf8(bytes.to_vec())?)],
        AttachmentKind::Image => anyhow::bail!("Image text extraction requires an OCR tool"),
    };
    ensure!(
        !units.is_empty() && units.iter().any(|(_, text)| !text.trim().is_empty()),
        "Document has no extractable text; scanned documents require OCR"
    );
    Ok(Extracted { units })
}

//! Bounded, read-only Word ingestion. Never unpack paths or follow links.
use std::fs::File;
use std::io::{Read, Seek};
use std::path::Path;

use anyhow::{ensure, Context, Result};
use quick_xml::{events::Event, name::ResolveResult, reader::NsReader};

const MAX_FILE: u64 = 20 * 1024 * 1024;
const MAX_EXPANDED: u64 = 64 * 1024 * 1024;
const MAX_XML: u64 = 8 * 1024 * 1024;
const MAX_TEXT: usize = 2 * 1024 * 1024;
const WORD_NS: &[u8] = b"http://schemas.openxmlformats.org/wordprocessingml/2006/main";
const STRICT_WORD_NS: &[u8] = b"http://purl.oclc.org/ooxml/wordprocessingml/main";
const CONTENT_NS: &[u8] = b"http://schemas.openxmlformats.org/package/2006/content-types";
const WORD_TYPE: &str =
    "application/vnd.openxmlformats-officedocument.wordprocessingml.document.main+xml";

fn entry<R: Read + Seek>(
    archive: &mut zip::ZipArchive<R>,
    name: &str,
    max: u64,
) -> Result<Vec<u8>> {
    let mut file = archive
        .by_name(name)
        .with_context(|| format!("Word package is missing {name}"))?;
    ensure!(
        !file.encrypted(),
        "Encrypted Word documents are not supported"
    );
    ensure!(
        file.size() <= max,
        "Word XML part {name} exceeds its size limit"
    );
    let mut bytes = Vec::new();
    (&mut file).take(max + 1).read_to_end(&mut bytes)?;
    ensure!(
        bytes.len() as u64 <= max,
        "Word XML part {name} exceeds its size limit"
    );
    Ok(bytes)
}

fn has_namespace(ns: &ResolveResult<'_>, expected: &[u8]) -> bool {
    matches!(ns, ResolveResult::Bound(value) if value.as_ref() == expected)
}

fn content_type(bytes: &[u8]) -> Result<()> {
    let mut reader = NsReader::from_reader(bytes);
    let mut buf = Vec::new();
    let mut depth = 0_usize;
    let mut root = false;
    let mut found = false;
    loop {
        let (ns, event) = reader.read_resolved_event_into(&mut buf)?;
        match event {
            Event::Start(ref e) | Event::Empty(ref e) => {
                let local = e.local_name();
                ensure!(
                    has_namespace(&ns, CONTENT_NS),
                    "Invalid Word content-types namespace"
                );
                if depth == 0 {
                    ensure!(
                        !root && local.as_ref() == b"Types",
                        "Invalid Word content-types root"
                    );
                    root = true;
                }
                if local.as_ref() == b"Override" {
                    let mut part = false;
                    let mut kind = false;
                    for attr in e.attributes() {
                        let attr = attr?;
                        let value = attr.normalized_value(quick_xml::XmlVersion::Implicit1_0)?;
                        part |= attr.key.as_ref() == b"PartName" && value == "/word/document.xml";
                        kind |= attr.key.as_ref() == b"ContentType" && value == WORD_TYPE;
                    }
                    found |= part && kind;
                }
                if matches!(event, Event::Start(_)) {
                    depth += 1;
                }
                ensure!(depth <= 128, "Word content-types XML is too deeply nested");
            }
            Event::End(_) => {
                depth = depth
                    .checked_sub(1)
                    .context("Invalid Word content-types XML")?;
            }
            Event::DocType(_) => anyhow::bail!("Word XML document types are not allowed"),
            Event::Text(e) if depth == 0 => ensure!(
                e.decode()?.trim().is_empty(),
                "Text outside the Word content-types root"
            ),
            Event::Eof => break,
            _ => {}
        }
        buf.clear();
    }
    ensure!(
        root && depth == 0 && found,
        "Not a supported .docx Word document (invalid or macro-enabled package)"
    );
    Ok(())
}

fn body_text(bytes: &[u8]) -> Result<String> {
    let mut reader = NsReader::from_reader(bytes);
    let mut buf = Vec::new();
    let mut text = String::new();
    let mut depth = 0_usize;
    let mut root = false;
    let mut body_depth = None;
    let mut body_seen = false;
    let mut text_depth = None;
    let mut deleted_depth = None;
    loop {
        let (ns, event) = reader.read_resolved_event_into(&mut buf)?;
        let word = has_namespace(&ns, WORD_NS) || has_namespace(&ns, STRICT_WORD_NS);
        match event {
            Event::Start(ref e) | Event::Empty(ref e) => {
                let local = e.local_name();
                let name = local.as_ref();
                if depth == 0 {
                    ensure!(
                        !root && word && name == b"document",
                        "Invalid Word document root"
                    );
                    root = true;
                }
                if word && name == b"body" {
                    ensure!(!body_seen && depth == 1, "Invalid Word body");
                    body_seen = true;
                    body_depth = Some(depth + 1);
                }
                if word && body_depth.is_some() && deleted_depth.is_none() {
                    match name {
                        b"del" | b"moveFrom" => deleted_depth = Some(depth + 1),
                        b"t" => text_depth = Some(depth + 1),
                        b"tab" => text.push('\t'),
                        b"br" | b"cr" => text.push('\n'),
                        b"noBreakHyphen" => text.push('\u{2011}'),
                        b"softHyphen" => text.push('\u{ad}'),
                        _ => {}
                    }
                }
                if matches!(event, Event::Start(_)) {
                    depth += 1;
                } else {
                    if text_depth == Some(depth + 1) {
                        text_depth = None;
                    }
                    if deleted_depth == Some(depth + 1) {
                        deleted_depth = None;
                    }
                    if body_depth == Some(depth + 1) {
                        body_depth = None;
                    }
                }
                ensure!(depth <= 256, "Word XML is too deeply nested");
            }
            Event::End(e) => {
                if word && body_depth.is_some() && deleted_depth.is_none() {
                    match e.local_name().as_ref() {
                        b"p" | b"tr" => text.push('\n'),
                        b"tc" => text.push('\t'),
                        _ => {}
                    }
                }
                if text_depth == Some(depth) {
                    text_depth = None;
                }
                if deleted_depth == Some(depth) {
                    deleted_depth = None;
                }
                if body_depth == Some(depth) {
                    body_depth = None;
                }
                depth = depth.checked_sub(1).context("Invalid Word XML nesting")?;
            }
            Event::Text(e) if text_depth.is_some() && deleted_depth.is_none() => {
                text.push_str(&e.decode()?)
            }
            Event::CData(e) if text_depth.is_some() && deleted_depth.is_none() => {
                text.push_str(&e.decode()?)
            }
            Event::GeneralRef(e) if text_depth.is_some() && deleted_depth.is_none() => {
                if let Some(value) = e.resolve_char_ref()? {
                    text.push(value);
                } else {
                    text.push(match e.decode()?.as_ref() {
                        "amp" => '&',
                        "lt" => '<',
                        "gt" => '>',
                        "quot" => '"',
                        "apos" => '\'',
                        _ => anyhow::bail!("Unsupported Word XML entity"),
                    });
                }
            }
            Event::DocType(_) => anyhow::bail!("Word XML document types are not allowed"),
            Event::Text(e) if depth == 0 => ensure!(
                e.decode()?.trim().is_empty(),
                "Text outside the Word document root"
            ),
            Event::Eof => break,
            _ => {}
        }
        ensure!(
            text.len() <= MAX_TEXT,
            "Word extracted text exceeds the 2 MiB limit"
        );
        buf.clear();
    }
    ensure!(
        root && body_seen && depth == 0,
        "Incomplete Word document XML"
    );
    ensure!(
        !text.trim().is_empty(),
        "Word document has no readable body text; embedded images are not OCRed"
    );
    Ok(text)
}

pub(super) fn read_docx(path: &Path) -> Result<String> {
    let file = File::open(path)?;
    ensure!(
        file.metadata()?.len() <= MAX_FILE,
        "Word file exceeds the 20 MiB limit"
    );
    let mut archive = zip::ZipArchive::new(file).context("Invalid .docx ZIP package")?;
    ensure!(
        archive.len() <= 4096,
        "Word package exceeds the 4096-entry limit"
    );
    let mut expanded = 0_u64;
    let mut names = std::collections::HashSet::new();
    for index in 0..archive.len() {
        let file = archive.by_index(index)?;
        ensure!(
            !file.encrypted(),
            "Encrypted Word documents are not supported"
        );
        ensure!(
            names.insert(file.name().to_string()),
            "Word package contains duplicate ZIP entries"
        );
        expanded = expanded
            .checked_add(file.size())
            .context("Word expanded size overflow")?;
        ensure!(
            expanded <= MAX_EXPANDED,
            "Word package exceeds the 64 MiB expanded limit"
        );
    }
    content_type(&entry(&mut archive, "[Content_Types].xml", 256 * 1024)?)?;
    let document = entry(&mut archive, "word/document.xml", MAX_XML)?;
    body_text(&document)
}

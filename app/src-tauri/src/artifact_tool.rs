use async_trait::async_trait;
use calamine::{Reader, Xlsx};
use cap_std::{ambient_authority, fs::Dir};
use quick_xml::{events::Event as XmlEvent, Reader as XmlReader};
use serde_json::{json, Value};
use socai_core::agent::{Tool, ToolContext, ToolResult};
use std::io::{BufReader, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};

const MAX_OOXML_ARCHIVE_ENTRIES: usize = 4_096;
const MAX_OOXML_UNCOMPRESSED_BYTES: u64 = 64 * 1024 * 1024;
const DOCX_MAIN_CONTENT_TYPE: &str =
    "application/vnd.openxmlformats-officedocument.wordprocessingml.document.main+xml";
const DOCM_MAIN_CONTENT_TYPE: &str = "application/vnd.ms-word.document.macroEnabled.main+xml";
const PPTX_MAIN_CONTENT_TYPE: &str =
    "application/vnd.openxmlformats-officedocument.presentationml.presentation.main+xml";
const PPTM_MAIN_CONTENT_TYPE: &str =
    "application/vnd.ms-powerpoint.presentation.macroEnabled.main+xml";
const XLSX_MAIN_CONTENT_TYPE: &str =
    "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet.main+xml";
const XLSM_MAIN_CONTENT_TYPE: &str = "application/vnd.ms-excel.sheet.macroEnabled.main+xml";

/// Publish a completed file from this conversation as a desktop download card.
/// The source stays in place; the user-facing copy lives in this turn's
/// `outputs/` directory.
pub(crate) struct PublishArtifactTool {
    conversation_dir: Option<PathBuf>,
}

impl PublishArtifactTool {
    pub(crate) fn new(conversation_dir: Option<PathBuf>) -> Self {
        Self { conversation_dir }
    }
}

#[async_trait]
impl Tool for PublishArtifactTool {
    fn available_in_local_delivery(&self) -> bool {
        true
    }

    fn name(&self) -> &str {
        "publish_artifact"
    }

    fn description(&self) -> &str {
        "Publish a completed user-requested file so the desktop app displays a \
         download card. Call this after creating and verifying the file. Pass \
         its path relative to the current run directory or an absolute path \
         inside the current conversation, including an earlier turn. The \
         source must be a regular file. Publishing never overwrites an existing \
         output. Do not tell the user a file is downloadable until this tool \
         succeeds."
    }

    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "path": {
                    "type": "string",
                    "description": "Completed file path, relative to the current run directory or absolute within the current conversation."
                }
            },
            "required": ["path"],
            "additionalProperties": false
        })
    }

    fn always_available(&self) -> bool {
        true
    }

    async fn call(&self, input: Value, ctx: &ToolContext) -> anyhow::Result<ToolResult> {
        let raw_path = input
            .get("path")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|path| !path.is_empty())
            .ok_or_else(|| anyhow::anyhow!("publish_artifact requires a non-empty `path`"))?;

        let run_root = ctx.run_dir.canonicalize().map_err(|error| {
            anyhow::anyhow!(
                "could not resolve current run directory {}: {error}",
                ctx.run_dir.display()
            )
        })?;
        let requested = resolve_source_path(raw_path, &ctx.run_dir);
        let requested_metadata = std::fs::symlink_metadata(&requested).map_err(|error| {
            anyhow::anyhow!(
                "could not inspect artifact source {}: {error}",
                requested.display()
            )
        })?;
        if requested_metadata.file_type().is_symlink() || !requested_metadata.is_file() {
            anyhow::bail!(
                "artifact source must be a regular file and cannot be a symbolic link: {}",
                requested.display()
            );
        }

        let source = requested.canonicalize().map_err(|error| {
            anyhow::anyhow!(
                "could not resolve artifact source {}: {error}",
                requested.display()
            )
        })?;
        let conversation_root = self
            .conversation_dir
            .as_deref()
            .map(Path::canonicalize)
            .transpose()
            .map_err(|error| {
                anyhow::anyhow!("could not resolve conversation directory: {error}")
            })?;
        let source_is_allowed = source.starts_with(&run_root)
            || conversation_root
                .as_deref()
                .is_some_and(|root| source.starts_with(root));
        if !source_is_allowed {
            anyhow::bail!(
                "artifact source is outside the current conversation: {}",
                requested.display()
            );
        }

        let mut source_file = open_read_only_no_follow(&source).map_err(|error| {
            anyhow::anyhow!(
                "could not open artifact source {}: {error}",
                source.display()
            )
        })?;
        let source_metadata = source_file.metadata().map_err(|error| {
            anyhow::anyhow!(
                "could not inspect open artifact source {}: {error}",
                source.display()
            )
        })?;
        if !source_metadata.is_file() {
            anyhow::bail!(
                "artifact source is no longer a regular file: {}",
                source.display()
            );
        }
        let opened_identity = same_file::Handle::from_file(source_file.try_clone()?)?;
        let current_identity = same_file::Handle::from_path(&source)?;
        if opened_identity != current_identity || requested.canonicalize()? != source {
            anyhow::bail!(
                "artifact source changed while publishing: {}",
                requested.display()
            );
        }

        validate_artifact_format(&source, &mut source_file)?;

        let (run_dir, outputs, outputs_dir) = prepare_outputs_directory(&run_root)?;
        let (published, published_file) = if source.starts_with(&outputs) {
            let relative = source.strip_prefix(&outputs)?;
            let destination_file = outputs_dir
                .open(relative)
                .map_err(|error| {
                    anyhow::anyhow!(
                        "could not open published artifact {}: {error}",
                        source.display()
                    )
                })?
                .into_std();
            let source_identity = same_file::Handle::from_file(source_file.try_clone()?)?;
            let destination_identity = same_file::Handle::from_file(destination_file.try_clone()?)?;
            if source_identity != destination_identity {
                anyhow::bail!(
                    "artifact source changed while validating its output: {}",
                    source.display()
                );
            }
            validate_published_destination(
                &outputs,
                &outputs_dir,
                relative,
                &source,
                &destination_file,
            )?;
            (source, destination_file)
        } else {
            let file_name = source
                .file_name()
                .ok_or_else(|| anyhow::anyhow!("artifact source has no filename"))?;
            let relative = Path::new(file_name);
            let destination = outputs.join(file_name);
            match outputs_dir.symlink_metadata(file_name) {
                Ok(_) => {
                    anyhow::bail!(
                        "an artifact named `{}` is already published for this turn",
                        file_name.to_string_lossy()
                    );
                }
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(error) => return Err(error.into()),
            }

            let mut staged = tempfile::NamedTempFile::new_in(&run_root)?;
            std::io::copy(&mut source_file, staged.as_file_mut()).map_err(|error| {
                anyhow::anyhow!("could not stage artifact {}: {error}", source.display())
            })?;
            staged.as_file_mut().flush()?;
            let staged_name = staged
                .path()
                .strip_prefix(&run_root)
                .map_err(|_| anyhow::anyhow!("staged artifact escaped the current run"))?;
            if staged_name.parent() != Some(Path::new("")) {
                anyhow::bail!("staged artifact is not directly inside the current run");
            }
            validate_directory_handle(&run_root, &run_dir, "current run")?;
            let staged_file = run_dir
                .open(staged_name)
                .map_err(|error| anyhow::anyhow!("could not reopen staged artifact: {error}"))?
                .into_std();
            let staged_identity = same_file::Handle::from_file(staged.as_file().try_clone()?)?;
            let reopened_staged_identity = same_file::Handle::from_file(staged_file.try_clone()?)?;
            if staged_identity != reopened_staged_identity {
                anyhow::bail!("staged artifact changed before publication");
            }

            validate_directory_handle(&outputs, &outputs_dir, "artifact output")?;
            if let Err(error) = run_dir.hard_link(staged_name, &outputs_dir, file_name) {
                if error.kind() == std::io::ErrorKind::AlreadyExists {
                    anyhow::bail!(
                        "an artifact named `{}` is already published for this turn",
                        file_name.to_string_lossy()
                    );
                }
                return Err(anyhow::anyhow!(
                    "could not publish artifact to {}: {error}",
                    destination.display()
                ));
            }
            let destination_file = match outputs_dir.open(file_name) {
                Ok(file) => file.into_std(),
                Err(error) => {
                    remove_published_destination(&outputs_dir, relative, &staged_file);
                    return Err(anyhow::anyhow!(
                        "could not reopen published artifact {}: {error}",
                        destination.display()
                    ));
                }
            };
            let destination_identity = same_file::Handle::from_file(destination_file.try_clone()?)?;
            if staged_identity != destination_identity {
                remove_published_destination(&outputs_dir, relative, &staged_file);
                anyhow::bail!(
                    "published artifact changed while writing: {}",
                    destination.display()
                );
            }
            if let Err(error) = validate_published_destination(
                &outputs,
                &outputs_dir,
                relative,
                &destination,
                &destination_file,
            ) {
                remove_published_destination(&outputs_dir, relative, &staged_file);
                return Err(error);
            }
            (destination, destination_file)
        };

        let metadata = published_file.metadata()?;
        let name = published
            .file_name()
            .and_then(|value| value.to_str())
            .ok_or_else(|| anyhow::anyhow!("published artifact has no valid filename"))?;
        let relative_path = ctx.register_artifact(
            &published,
            name,
            "deliverable",
            &format!("User-downloadable file {name}"),
            json!({
                "category": "deliverable",
                "name": name,
                "size_bytes": metadata.len(),
            }),
            None,
            "publish_artifact",
        );

        Ok(ToolResult::text(serde_json::to_string_pretty(&json!({
            "ok": true,
            "artifact": {
                "name": name,
                "path": relative_path,
                "size_bytes": metadata.len(),
            },
            "message": "Artifact published. The desktop app will display a download card."
        }))?))
    }
}

fn resolve_source_path(raw_path: &str, run_dir: &Path) -> PathBuf {
    let path = PathBuf::from(raw_path);
    if path.is_absolute() {
        path
    } else {
        run_dir.join(path)
    }
}

fn validate_artifact_format(path: &Path, file: &mut std::fs::File) -> anyhow::Result<()> {
    let extension = path
        .extension()
        .and_then(|value| value.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase();
    let validation = match extension.as_str() {
        "xlsx" => validate_spreadsheet(file, XLSX_MAIN_CONTENT_TYPE),
        "xlsm" => validate_spreadsheet(file, XLSM_MAIN_CONTENT_TYPE),
        "docx" => validate_ooxml_archive(
            file,
            "Word document",
            "word/document.xml",
            "document",
            DOCX_MAIN_CONTENT_TYPE,
        ),
        "docm" => validate_ooxml_archive(
            file,
            "Word document",
            "word/document.xml",
            "document",
            DOCM_MAIN_CONTENT_TYPE,
        ),
        "pptx" => validate_ooxml_archive(
            file,
            "PowerPoint presentation",
            "ppt/presentation.xml",
            "presentation",
            PPTX_MAIN_CONTENT_TYPE,
        ),
        "pptm" => validate_ooxml_archive(
            file,
            "PowerPoint presentation",
            "ppt/presentation.xml",
            "presentation",
            PPTM_MAIN_CONTENT_TYPE,
        ),
        "pdf" => validate_pdf(file),
        _ => Ok(()),
    };
    file.seek(SeekFrom::Start(0))?;
    validation
}

fn validate_spreadsheet(file: &mut std::fs::File, main_content_type: &str) -> anyhow::Result<()> {
    crate::commands::validate_spreadsheet_archive(file)
        .map_err(|error| anyhow::anyhow!("artifact extension/content mismatch: {error}"))?;
    file.seek(SeekFrom::Start(0))?;
    {
        let mut archive = zip::ZipArchive::new(&mut *file).map_err(|error| {
            anyhow::anyhow!("artifact extension/content mismatch: invalid Excel workbook: {error}")
        })?;
        validate_ooxml_content_type(
            &mut archive,
            "Excel workbook",
            "xl/workbook.xml",
            main_content_type,
        )?;
    }
    file.seek(SeekFrom::Start(0))?;
    let mut workbook = Xlsx::new(&mut *file).map_err(|error| {
        anyhow::anyhow!(
            "artifact extension/content mismatch: could not open Excel workbook: {error}"
        )
    })?;
    if workbook.sheet_names().is_empty() {
        anyhow::bail!("artifact extension/content mismatch: Excel workbook has no worksheets");
    }
    workbook
        .worksheet_range_at(0)
        .ok_or_else(|| anyhow::anyhow!("Excel workbook has no readable worksheet"))?
        .map_err(|error| {
            anyhow::anyhow!(
                "artifact extension/content mismatch: could not read Excel worksheet: {error}"
            )
        })?;
    Ok(())
}

fn validate_ooxml_archive(
    file: &mut std::fs::File,
    kind: &str,
    main_entry: &str,
    main_root: &str,
    main_content_type: &str,
) -> anyhow::Result<()> {
    let mut archive = zip::ZipArchive::new(&mut *file).map_err(|error| {
        anyhow::anyhow!("artifact extension/content mismatch: invalid {kind}: {error}")
    })?;
    if archive.len() > MAX_OOXML_ARCHIVE_ENTRIES {
        anyhow::bail!(
            "artifact extension/content mismatch: {kind} exceeds the {MAX_OOXML_ARCHIVE_ENTRIES} entry limit"
        );
    }

    let mut expanded_bytes = 0_u64;
    let mut has_content_types = false;
    let mut has_package_relationships = false;
    let mut has_main_entry = false;
    for index in 0..archive.len() {
        let entry = archive
            .by_index(index)
            .map_err(|error| anyhow::anyhow!("could not inspect {kind}: {error}"))?;
        has_content_types |= entry.name() == "[Content_Types].xml";
        has_package_relationships |= entry.name() == "_rels/.rels";
        has_main_entry |= entry.name() == main_entry;
        expanded_bytes = expanded_bytes
            .checked_add(entry.size())
            .ok_or_else(|| anyhow::anyhow!("{kind} expanded size overflowed"))?;
        if expanded_bytes > MAX_OOXML_UNCOMPRESSED_BYTES {
            anyhow::bail!(
                "artifact extension/content mismatch: {kind} exceeds the {} MB expanded size limit",
                MAX_OOXML_UNCOMPRESSED_BYTES / (1024 * 1024)
            );
        }
    }
    if !has_content_types || !has_package_relationships || !has_main_entry {
        anyhow::bail!(
            "artifact extension/content mismatch: {kind} is missing required OOXML entries"
        );
    }
    validate_ooxml_content_type(&mut archive, kind, main_entry, main_content_type)?;
    validate_ooxml_relationship(&mut archive, kind, main_entry)?;
    validate_ooxml_main_document(&mut archive, kind, main_entry, main_root)?;
    Ok(())
}

fn validate_ooxml_content_type<R: Read + Seek>(
    archive: &mut zip::ZipArchive<R>,
    kind: &str,
    main_entry: &str,
    main_content_type: &str,
) -> anyhow::Result<()> {
    let entry = archive
        .by_name("[Content_Types].xml")
        .map_err(|error| anyhow::anyhow!("could not open {kind} content types: {error}"))?;
    let mut xml = XmlReader::from_reader(BufReader::new(entry));
    let mut buffer = Vec::new();
    let expected_part = format!("/{main_entry}");
    let mut found = false;
    let mut depth = 0_usize;
    let mut root_seen = false;
    loop {
        buffer.clear();
        match xml.read_event_into(&mut buffer) {
            Ok(XmlEvent::Start(event)) => {
                validate_ooxml_root(
                    &event,
                    kind,
                    "content types",
                    b"Types",
                    depth,
                    &mut root_seen,
                )?;
                found |= ooxml_element_has_attributes(
                    &event,
                    b"Override",
                    &[
                        (b"PartName", expected_part.as_str()),
                        (b"ContentType", main_content_type),
                    ],
                    kind,
                )?;
                depth = depth.checked_add(1).ok_or_else(|| {
                    anyhow::anyhow!("{kind} content types XML is too deeply nested")
                })?;
            }
            Ok(XmlEvent::Empty(event)) => {
                validate_ooxml_root(
                    &event,
                    kind,
                    "content types",
                    b"Types",
                    depth,
                    &mut root_seen,
                )?;
                found |= ooxml_element_has_attributes(
                    &event,
                    b"Override",
                    &[
                        (b"PartName", expected_part.as_str()),
                        (b"ContentType", main_content_type),
                    ],
                    kind,
                )?;
            }
            Ok(XmlEvent::End(_)) => {
                depth = depth.checked_sub(1).ok_or_else(|| {
                    anyhow::anyhow!(
                        "artifact extension/content mismatch: invalid {kind} content types XML"
                    )
                })?;
            }
            Ok(XmlEvent::Eof) => {
                if !root_seen || depth != 0 {
                    anyhow::bail!(
                        "artifact extension/content mismatch: incomplete {kind} content types XML"
                    );
                }
                break;
            }
            Ok(_) => {}
            Err(error) => anyhow::bail!(
                "artifact extension/content mismatch: invalid {kind} content types XML: {error}"
            ),
        }
    }
    if !found {
        anyhow::bail!(
            "artifact extension/content mismatch: {kind} main content type is missing or invalid"
        );
    }
    Ok(())
}

fn validate_ooxml_relationship<R: Read + Seek>(
    archive: &mut zip::ZipArchive<R>,
    kind: &str,
    main_entry: &str,
) -> anyhow::Result<()> {
    let entry = archive
        .by_name("_rels/.rels")
        .map_err(|error| anyhow::anyhow!("could not open {kind} relationships: {error}"))?;
    let mut xml = XmlReader::from_reader(BufReader::new(entry));
    let mut buffer = Vec::new();
    let mut found = false;
    let mut depth = 0_usize;
    let mut root_seen = false;
    loop {
        buffer.clear();
        match xml.read_event_into(&mut buffer) {
            Ok(XmlEvent::Start(event)) => {
                validate_ooxml_root(
                    &event,
                    kind,
                    "relationships",
                    b"Relationships",
                    depth,
                    &mut root_seen,
                )?;
                found |= ooxml_relationship_targets(&event, main_entry, kind)?;
                depth = depth.checked_add(1).ok_or_else(|| {
                    anyhow::anyhow!("{kind} relationships XML is too deeply nested")
                })?;
            }
            Ok(XmlEvent::Empty(event)) => {
                validate_ooxml_root(
                    &event,
                    kind,
                    "relationships",
                    b"Relationships",
                    depth,
                    &mut root_seen,
                )?;
                found |= ooxml_relationship_targets(&event, main_entry, kind)?;
            }
            Ok(XmlEvent::End(_)) => {
                depth = depth.checked_sub(1).ok_or_else(|| {
                    anyhow::anyhow!(
                        "artifact extension/content mismatch: invalid {kind} relationships XML"
                    )
                })?;
            }
            Ok(XmlEvent::Eof) => {
                if !root_seen || depth != 0 {
                    anyhow::bail!(
                        "artifact extension/content mismatch: incomplete {kind} relationships XML"
                    );
                }
                break;
            }
            Ok(_) => {}
            Err(error) => anyhow::bail!(
                "artifact extension/content mismatch: invalid {kind} relationships XML: {error}"
            ),
        }
    }
    if !found {
        anyhow::bail!(
            "artifact extension/content mismatch: {kind} package does not target its main document"
        );
    }
    Ok(())
}

fn validate_ooxml_main_document<R: Read + Seek>(
    archive: &mut zip::ZipArchive<R>,
    kind: &str,
    main_entry: &str,
    main_root: &str,
) -> anyhow::Result<()> {
    let entry = archive
        .by_name(main_entry)
        .map_err(|error| anyhow::anyhow!("could not open {kind} main document: {error}"))?;
    let mut xml = XmlReader::from_reader(BufReader::new(entry));
    let mut buffer = Vec::new();
    let mut depth = 0_usize;
    let mut root_seen = false;
    loop {
        buffer.clear();
        match xml.read_event_into(&mut buffer) {
            Ok(XmlEvent::Start(event)) => {
                validate_ooxml_root(
                    &event,
                    kind,
                    "main document",
                    main_root.as_bytes(),
                    depth,
                    &mut root_seen,
                )?;
                depth = depth.checked_add(1).ok_or_else(|| {
                    anyhow::anyhow!("{kind} main document XML is too deeply nested")
                })?;
            }
            Ok(XmlEvent::Empty(event)) => {
                validate_ooxml_root(
                    &event,
                    kind,
                    "main document",
                    main_root.as_bytes(),
                    depth,
                    &mut root_seen,
                )?;
            }
            Ok(XmlEvent::End(_)) => {
                depth = depth.checked_sub(1).ok_or_else(|| {
                    anyhow::anyhow!(
                        "artifact extension/content mismatch: invalid {kind} main document XML"
                    )
                })?;
            }
            Ok(XmlEvent::Eof) => {
                if !root_seen || depth != 0 {
                    anyhow::bail!(
                        "artifact extension/content mismatch: incomplete {kind} main document XML"
                    );
                }
                return Ok(());
            }
            Ok(_) => {}
            Err(error) => anyhow::bail!(
                "artifact extension/content mismatch: invalid {kind} main document XML: {error}"
            ),
        }
    }
}

fn validate_ooxml_root(
    event: &quick_xml::events::BytesStart<'_>,
    kind: &str,
    section: &str,
    expected_root: &[u8],
    depth: usize,
    root_seen: &mut bool,
) -> anyhow::Result<()> {
    if depth != 0 {
        return Ok(());
    }
    if *root_seen || event.local_name().as_ref() != expected_root {
        anyhow::bail!("artifact extension/content mismatch: invalid {kind} {section} root");
    }
    *root_seen = true;
    Ok(())
}

fn ooxml_element_has_attributes(
    event: &quick_xml::events::BytesStart<'_>,
    element: &[u8],
    expected: &[(&[u8], &str)],
    kind: &str,
) -> anyhow::Result<bool> {
    if event.local_name().as_ref() != element {
        return Ok(false);
    }
    let mut matches = vec![false; expected.len()];
    for attribute in event.attributes().with_checks(false) {
        let attribute =
            attribute.map_err(|error| anyhow::anyhow!("invalid {kind} XML attributes: {error}"))?;
        for (index, (name, value)) in expected.iter().enumerate() {
            if attribute.key.local_name().as_ref() == *name
                && attribute.value.as_ref() == value.as_bytes()
            {
                matches[index] = true;
            }
        }
    }
    Ok(matches.into_iter().all(|matched| matched))
}

fn ooxml_relationship_targets(
    event: &quick_xml::events::BytesStart<'_>,
    main_entry: &str,
    kind: &str,
) -> anyhow::Result<bool> {
    if event.local_name().as_ref() != b"Relationship" {
        return Ok(false);
    }
    let mut relationship_type = None;
    let mut target = None;
    for attribute in event.attributes().with_checks(false) {
        let attribute = attribute
            .map_err(|error| anyhow::anyhow!("invalid {kind} relationship attributes: {error}"))?;
        match attribute.key.local_name().as_ref() {
            b"Type" => relationship_type = Some(attribute.value.into_owned()),
            b"Target" => target = Some(attribute.value.into_owned()),
            _ => {}
        }
    }
    Ok(relationship_type
        .as_deref()
        .is_some_and(|value| value.ends_with(b"/officeDocument"))
        && target.as_deref().is_some_and(|value| {
            value.strip_prefix(b"/").unwrap_or(value) == main_entry.as_bytes()
        }))
}

fn validate_pdf(file: &mut std::fs::File) -> anyhow::Result<()> {
    let len = file.metadata()?.len();
    if len < 8 {
        anyhow::bail!("artifact extension/content mismatch: invalid PDF header");
    }
    let mut header = [0_u8; 5];
    file.read_exact(&mut header)?;
    if &header != b"%PDF-" {
        anyhow::bail!("artifact extension/content mismatch: invalid PDF header");
    }
    let tail_len = len.min(1_024) as usize;
    file.seek(SeekFrom::End(-(tail_len as i64)))?;
    let mut tail = vec![0_u8; tail_len];
    file.read_exact(&mut tail)?;
    if !tail.windows(5).any(|window| window == b"%%EOF") {
        anyhow::bail!("artifact extension/content mismatch: PDF end marker is missing");
    }
    Ok(())
}

fn prepare_outputs_directory(run_root: &Path) -> anyhow::Result<(Dir, PathBuf, Dir)> {
    let run_dir = Dir::open_ambient_dir(run_root, ambient_authority())?;
    validate_directory_handle(run_root, &run_dir, "current run")?;
    let outputs = run_root.join("outputs");
    match run_dir.symlink_metadata("outputs") {
        Ok(metadata) if metadata.file_type().is_symlink() || !metadata.is_dir() => {
            anyhow::bail!(
                "artifact output path must be a regular directory: {}",
                outputs.display()
            );
        }
        Ok(_) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            run_dir.create_dir("outputs")?;
        }
        Err(error) => return Err(error.into()),
    }
    let outputs_dir = run_dir.open_dir("outputs")?;
    validate_directory_handle(&outputs, &outputs_dir, "artifact output")?;
    Ok((run_dir, outputs, outputs_dir))
}

fn validate_directory_handle(path: &Path, directory: &Dir, label: &str) -> anyhow::Result<()> {
    let metadata = std::fs::symlink_metadata(path)?;
    let opened_metadata = directory.metadata(".")?;
    if metadata.file_type().is_symlink()
        || !metadata.is_dir()
        || !opened_metadata.is_dir()
        || path.canonicalize()? != path
    {
        anyhow::bail!(
            "{label} directory changed while publishing: {}",
            path.display()
        );
    }
    let opened_identity = same_file::Handle::from_file(directory.try_clone()?.into_std_file())?;
    let current_identity = same_file::Handle::from_path(path)?;
    if opened_identity != current_identity {
        anyhow::bail!(
            "{label} directory changed while publishing: {}",
            path.display()
        );
    }
    Ok(())
}

fn validate_published_destination(
    outputs: &Path,
    outputs_dir: &Dir,
    relative: &Path,
    destination: &Path,
    destination_file: &std::fs::File,
) -> anyhow::Result<()> {
    validate_directory_handle(outputs, outputs_dir, "artifact output")?;
    let relative_metadata = outputs_dir.symlink_metadata(relative)?;
    let ambient_metadata = std::fs::symlink_metadata(destination)?;
    if relative_metadata.file_type().is_symlink()
        || !relative_metadata.is_file()
        || ambient_metadata.file_type().is_symlink()
        || !ambient_metadata.is_file()
    {
        anyhow::bail!(
            "published artifact path changed after writing: {}",
            destination.display()
        );
    }
    let canonical = destination.canonicalize()?;
    let opened_identity = same_file::Handle::from_file(destination_file.try_clone()?)?;
    let relative_identity = same_file::Handle::from_file(outputs_dir.open(relative)?.into_std())?;
    let current_identity = same_file::Handle::from_path(&canonical)?;
    if !canonical.starts_with(outputs)
        || opened_identity != relative_identity
        || opened_identity != current_identity
    {
        anyhow::bail!(
            "published artifact escaped or changed after writing: {}",
            destination.display()
        );
    }
    Ok(())
}

fn remove_published_destination(outputs_dir: &Dir, relative: &Path, expected_file: &std::fs::File) {
    let Ok(current_file) = outputs_dir.open(relative) else {
        return;
    };
    let Ok(expected_clone) = expected_file.try_clone() else {
        return;
    };
    let Ok(expected_identity) = same_file::Handle::from_file(expected_clone) else {
        return;
    };
    let Ok(current_identity) = same_file::Handle::from_file(current_file.into_std()) else {
        return;
    };
    if expected_identity == current_identity {
        let _ = outputs_dir.remove_file(relative);
    }
}

fn open_read_only_no_follow(path: &Path) -> std::io::Result<std::fs::File> {
    let mut options = std::fs::OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NOFOLLOW);
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        const FILE_FLAG_OPEN_REPARSE_POINT: u32 = 0x0020_0000;
        options.custom_flags(FILE_FLAG_OPEN_REPARSE_POINT);
    }
    options.open(path)
}

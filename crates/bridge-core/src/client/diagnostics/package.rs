use super::*;

pub(super) fn collect_optional_file(
    entries: &mut Vec<(String, String)>,
    archive_name: &str,
    path: Option<PathBuf>,
    warnings: &mut Vec<String>,
) {
    let Some(path) = path else {
        warnings.push(format!("{archive_name}: unavailable"));
        return;
    };
    match read_text_excerpt(&path) {
        Ok(contents) => push_text_entry(entries, archive_name, contents, warnings),
        Err(error) => warnings.push(format!("{archive_name}: unavailable ({error})")),
    }
}

pub(super) fn read_text_excerpt(path: &Path) -> io::Result<String> {
    let file = File::open(path)?;
    let mut bytes = Vec::with_capacity(MAX_PACKAGE_ENTRY_BYTES.min(64 * 1024));
    file.take(u64::try_from(MAX_PACKAGE_ENTRY_BYTES + 1).expect("entry bound fits u64"))
        .read_to_end(&mut bytes)?;
    let text = String::from_utf8_lossy(&bytes);
    Ok(tail_bounded(&text, MAX_PACKAGE_ENTRY_BYTES).0)
}

pub(super) fn push_text_entry(
    entries: &mut Vec<(String, String)>,
    archive_name: &str,
    contents: String,
    warnings: &mut Vec<String>,
) {
    if entries.len() >= MAX_PACKAGE_FILES {
        warnings.push(format!(
            "{archive_name}: omitted because the package file limit was reached"
        ));
        return;
    }
    let redacted = crate::diagnostics::redact_text(&contents);
    let (bounded, truncated) = tail_bounded(&redacted, MAX_PACKAGE_ENTRY_BYTES);
    if truncated {
        warnings.push(format!(
            "{archive_name}: truncated to the most recent {MAX_PACKAGE_ENTRY_BYTES} bytes"
        ));
    }
    entries.push((archive_name.to_owned(), bounded));
}

pub(super) fn tail_bounded(value: &str, max_bytes: usize) -> (String, bool) {
    if value.len() <= max_bytes {
        return (value.to_owned(), false);
    }
    let mut start = value.len() - max_bytes;
    while !value.is_char_boundary(start) {
        start += 1;
    }
    (value[start..].to_owned(), true)
}

pub(super) fn enforce_total_bound(entries: &mut Vec<(String, String)>, warnings: &mut Vec<String>) {
    let mut remaining = MAX_PACKAGE_TOTAL_BYTES;
    entries.retain_mut(|(name, contents)| {
        if remaining == 0 {
            warnings.push(format!(
                "{name}: omitted because the package size limit was reached"
            ));
            return false;
        }
        if contents.len() > remaining {
            let (bounded, _) = tail_bounded(contents, remaining);
            *contents = bounded;
            warnings.push(format!("{name}: truncated by the package size limit"));
        }
        remaining = remaining.saturating_sub(contents.len());
        true
    });
}

pub(super) fn package_manifest(entries: &[(String, String)], warnings: &[String]) -> String {
    let mut manifest = format!(
        "Tractor Beam Troubleshooting Package\ngenerated_at: {}\nfiles:\n",
        format_evidence_timestamp(unix_seconds().saturating_mul(1_000))
    );
    for (name, contents) in entries {
        manifest.push_str(&format!("- {name}: {} bytes\n", contents.len()));
    }
    manifest.push_str("warnings:\n");
    if warnings.is_empty() {
        manifest.push_str("- none\n");
    } else {
        for warning in warnings {
            manifest.push_str("- ");
            manifest.push_str(warning);
            manifest.push('\n');
        }
    }
    manifest
}

pub(super) fn temporary_package_path(path: &Path) -> PathBuf {
    let name = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("tractor-beam-troubleshooting.zip");
    path.with_file_name(format!(".{name}.{}.tmp", std::process::id()))
}

pub(super) fn write_package(path: &Path, entries: &[(String, String)]) -> io::Result<()> {
    let file = OpenOptions::new().write(true).create_new(true).open(path)?;
    let mut archive = ZipWriter::new(file);
    let options = SimpleFileOptions::default().compression_method(CompressionMethod::Deflated);
    for (name, contents) in entries {
        archive
            .start_file(name, options)
            .map_err(io::Error::other)?;
        archive.write_all(contents.as_bytes())?;
    }
    let file: File = archive.finish().map_err(io::Error::other)?;
    file.sync_all()
}

pub(super) fn format_evidence_timestamp(timestamp_ms: u64) -> String {
    use chrono::{DateTime, FixedOffset, Utc};

    let timestamp_ms = i64::try_from(timestamp_ms).unwrap_or(i64::MAX);
    DateTime::<Utc>::from_timestamp_millis(timestamp_ms).map_or_else(
        || "invalid-timestamp".to_owned(),
        |timestamp| {
            timestamp
                .with_timezone(&FixedOffset::east_opt(0).expect("UTC offset is valid"))
                .to_rfc3339_opts(chrono::SecondsFormat::Millis, true)
        },
    )
}

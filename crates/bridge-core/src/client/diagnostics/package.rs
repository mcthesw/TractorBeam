use std::io::{BufRead, BufReader, Seek as _, SeekFrom, Write};

use atomic_write_file::AtomicWriteFile;

use super::*;

const MAX_LOG_RECORD_BYTES: usize = 64 * 1024;

#[derive(Debug)]
pub(super) struct PackageSource {
    pub(super) archive_name: String,
    pub(super) path: Option<PathBuf>,
    pub(super) tail_bytes: Option<u64>,
}

#[derive(Debug)]
struct EntryOutcome {
    name: String,
    status: String,
}

pub(super) fn write_diagnostics_bundle(
    path: &Path,
    summary: &str,
    sources: &[PackageSource],
) -> io::Result<()> {
    let file = AtomicWriteFile::open(path)?;
    let mut archive = ZipWriter::new(file);
    let options = SimpleFileOptions::default().compression_method(CompressionMethod::Deflated);
    let mut outcomes = Vec::with_capacity(sources.len().saturating_add(1));

    let redacted_summary = crate::diagnostics::redact_text(summary);
    archive
        .start_file("summary.txt", options)
        .map_err(io::Error::other)?;
    archive.write_all(redacted_summary.as_bytes())?;
    outcomes.push(EntryOutcome {
        name: "summary.txt".to_owned(),
        status: format!("included bytes={}", redacted_summary.len()),
    });

    for source in sources {
        outcomes.push(write_source(&mut archive, options, source));
    }

    let manifest = crate::diagnostics::redact_text(&package_manifest(&outcomes));
    archive
        .start_file("manifest.txt", options)
        .map_err(io::Error::other)?;
    archive.write_all(manifest.as_bytes())?;

    let file = archive.finish().map_err(io::Error::other)?;
    file.commit()
}

fn write_source(
    archive: &mut ZipWriter<AtomicWriteFile>,
    options: SimpleFileOptions,
    source: &PackageSource,
) -> EntryOutcome {
    let Some(path) = source.path.as_ref() else {
        return EntryOutcome {
            name: source.archive_name.clone(),
            status: "unavailable source_not_found".to_owned(),
        };
    };
    let mut file = match File::open(path) {
        Ok(file) => file,
        Err(error) => {
            return EntryOutcome {
                name: source.archive_name.clone(),
                status: format!("unavailable path={} error={error}", path.display()),
            };
        }
    };
    let source_truncated = match source.tail_bytes {
        Some(limit) => match seek_to_tail(&mut file, limit) {
            Ok(truncated) => truncated,
            Err(error) => {
                return EntryOutcome {
                    name: source.archive_name.clone(),
                    status: format!("unreadable path={} error={error}", path.display()),
                };
            }
        },
        None => false,
    };
    if let Err(error) = archive.start_file(&source.archive_name, options) {
        return EntryOutcome {
            name: source.archive_name.clone(),
            status: format!("zip_entry_failed error={error}"),
        };
    }
    match stream_redacted(BufReader::new(file), archive) {
        Ok((bytes, truncated_records)) => EntryOutcome {
            name: source.archive_name.clone(),
            status: format!(
                "included bytes={bytes} source_truncated={source_truncated} truncated_records={truncated_records}"
            ),
        },
        Err(error) => EntryOutcome {
            name: source.archive_name.clone(),
            status: format!(
                "partial path={} source_truncated={source_truncated} error={error}",
                path.display()
            ),
        },
    }
}

fn seek_to_tail(file: &mut File, limit: u64) -> io::Result<bool> {
    let length = file.metadata()?.len();
    if length <= limit {
        return Ok(false);
    }
    file.seek(SeekFrom::Start(length.saturating_sub(limit)))?;
    let mut reader = BufReader::new(&mut *file);
    skip_to_record_end(&mut reader)?;
    let position = reader.stream_position()?;
    drop(reader);
    file.seek(SeekFrom::Start(position))?;
    Ok(true)
}

fn stream_redacted(mut reader: impl BufRead, writer: &mut impl Write) -> io::Result<(u64, u64)> {
    let mut record = Vec::with_capacity(1024);
    let mut bytes_written = 0_u64;
    let mut truncated_records = 0_u64;
    while let Some(truncated) = read_bounded_record(&mut reader, &mut record)? {
        let text = String::from_utf8_lossy(&record);
        let redacted = crate::diagnostics::redact_text(&text);
        writer.write_all(redacted.as_bytes())?;
        bytes_written = bytes_written.saturating_add(redacted.len() as u64);
        if truncated {
            const MARKER: &[u8] = b"\n[record truncated]\n";
            writer.write_all(MARKER)?;
            bytes_written = bytes_written.saturating_add(MARKER.len() as u64);
            truncated_records = truncated_records.saturating_add(1);
        }
    }
    Ok((bytes_written, truncated_records))
}

fn read_bounded_record(
    reader: &mut impl BufRead,
    record: &mut Vec<u8>,
) -> io::Result<Option<bool>> {
    record.clear();
    loop {
        let available = reader.fill_buf()?;
        if available.is_empty() {
            return Ok((!record.is_empty()).then_some(false));
        }
        let newline = available.iter().position(|byte| *byte == b'\n');
        let available_record = newline.map_or(available.len(), |position| position + 1);
        let remaining = MAX_LOG_RECORD_BYTES.saturating_sub(record.len());
        let copied = available_record.min(remaining);
        record.extend_from_slice(&available[..copied]);
        reader.consume(available_record);
        if available_record > remaining {
            if newline.is_none() {
                skip_to_record_end(reader)?;
            }
            return Ok(Some(true));
        }
        if newline.is_some() {
            return Ok(Some(false));
        }
    }
}

fn skip_to_record_end(reader: &mut impl BufRead) -> io::Result<()> {
    loop {
        let available = reader.fill_buf()?;
        if available.is_empty() {
            return Ok(());
        }
        if let Some(position) = available.iter().position(|byte| *byte == b'\n') {
            reader.consume(position + 1);
            return Ok(());
        }
        let length = available.len();
        reader.consume(length);
    }
}

fn package_manifest(outcomes: &[EntryOutcome]) -> String {
    let mut manifest = format!(
        "Tractor Beam Diagnostics Bundle\ngenerated_at: {}\nfiles:\n",
        format_evidence_timestamp(unix_seconds().saturating_mul(1_000))
    );
    for outcome in outcomes {
        manifest.push_str("- ");
        manifest.push_str(&outcome.name);
        manifest.push_str(": ");
        manifest.push_str(&outcome.status);
        manifest.push('\n');
    }
    manifest
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bounded_record_reader_does_not_grow_for_a_malformed_long_line() {
        let input = vec![b'x'; MAX_LOG_RECORD_BYTES.saturating_mul(2)];
        let mut reader = BufReader::new(input.as_slice());
        let mut record = Vec::new();

        assert_eq!(
            read_bounded_record(&mut reader, &mut record).unwrap(),
            Some(true)
        );
        assert_eq!(record.len(), MAX_LOG_RECORD_BYTES);
        assert_eq!(read_bounded_record(&mut reader, &mut record).unwrap(), None);
    }
}

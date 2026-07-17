use std::{
    fs, io,
    path::{Path, PathBuf},
    sync::OnceLock,
};

use tracing_appender::non_blocking::WorkerGuard;
use tracing_subscriber::{EnvFilter, fmt, prelude::*};
use tractor_beam_core::{
    ClientLogSink, ClientSessionLogContext, LogLevel, bundle_config_path, emit_client_log_event,
};

static PROCESS_LOG_GUARD: OnceLock<Result<WorkerGuard, String>> = OnceLock::new();

#[derive(Debug)]
pub(crate) struct ClientLogFiles {
    root: PathBuf,
    client_dir: PathBuf,
    warnings: Vec<String>,
}

#[derive(Debug)]
struct LocalDailyAppender {
    directory: PathBuf,
    active_date: String,
    file: fs::File,
}

impl LocalDailyAppender {
    fn new(directory: &Path) -> io::Result<Self> {
        fs::create_dir_all(directory)?;
        let active_date = local_date();
        let file = open_daily_log(directory, &active_date)?;
        tractor_beam_core::diagnostics::prune_daily_logs(directory)?;
        Ok(Self {
            directory: directory.to_path_buf(),
            active_date,
            file,
        })
    }

    fn rotate_if_needed(&mut self) -> io::Result<()> {
        let date = local_date();
        if date == self.active_date {
            return Ok(());
        }
        self.file = open_daily_log(&self.directory, &date)?;
        self.active_date = date;
        tractor_beam_core::diagnostics::prune_daily_logs(&self.directory)
    }
}

impl io::Write for LocalDailyAppender {
    fn write(&mut self, buffer: &[u8]) -> io::Result<usize> {
        self.rotate_if_needed()?;
        self.file.write(buffer)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.file.flush()
    }
}

impl ClientLogFiles {
    pub(crate) fn new() -> Self {
        let root = bundle_log_root();
        let client_dir = root.join("client");
        let mut warnings = Vec::new();
        if let Err(error) = fs::create_dir_all(&client_dir) {
            warnings.push(format!(
                "Could not create Client log directory {}: {error}",
                client_dir.display()
            ));
        }
        if let Some(error) = init_process_tracing(&client_dir) {
            warnings.push(error);
        }
        Self {
            root,
            client_dir,
            warnings,
        }
    }

    pub(crate) fn open_default_directory() -> io::Result<PathBuf> {
        let log_files = Self::new();
        fs::create_dir_all(&log_files.root)?;
        open::that_detached(&log_files.root)?;
        Ok(log_files.root)
    }
}

impl ClientLogSink for ClientLogFiles {
    fn root(&self) -> Option<PathBuf> {
        Some(self.root.clone())
    }

    fn warnings(&self) -> Vec<String> {
        self.warnings.clone()
    }

    fn log_files(&self) -> Vec<PathBuf> {
        tractor_beam_core::diagnostics::daily_log_files(&self.client_dir)
    }

    fn emit(&self, context: Option<&ClientSessionLogContext>, level: LogLevel, message: &str) {
        emit_client_log_event(context, level, message);
    }
}

fn init_process_tracing(directory: &Path) -> Option<String> {
    PROCESS_LOG_GUARD
        .get_or_init(|| {
            let appender = LocalDailyAppender::new(directory).map_err(|error| error.to_string())?;
            let (writer, guard) = tracing_appender::non_blocking(appender);
            let layer = fmt::layer()
                .with_ansi(false)
                .with_target(false)
                .with_writer(writer);
            let filter =
                EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));
            let subscriber = tracing_subscriber::registry().with(filter).with(layer);
            tracing::subscriber::set_global_default(subscriber)
                .map_err(|error| error.to_string())?;
            Ok(guard)
        })
        .as_ref()
        .err()
        .cloned()
}

fn local_date() -> String {
    chrono::Local::now().format("%Y-%m-%d").to_string()
}

fn open_daily_log(directory: &Path, date: &str) -> io::Result<fs::File> {
    fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(directory.join(format!("{date}.log")))
}

fn bundle_log_root() -> PathBuf {
    bundle_config_path()
        .and_then(|path| path.parent().map(Path::to_path_buf))
        .unwrap_or_else(|| std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")))
        .join("logs")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn daily_log_discovery_ignores_unrelated_files_and_keeps_newest_ten() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path();
        for day in 1..=12 {
            fs::write(root.join(format!("2026-07-{day:02}.log")), "log").unwrap();
        }
        fs::write(root.join("bridge-client.log"), "legacy").unwrap();
        fs::write(root.join("notes.txt"), "keep").unwrap();

        let files = tractor_beam_core::diagnostics::daily_log_files(root);

        assert_eq!(files.len(), 10);
        assert_eq!(files[0].file_name().unwrap(), "2026-07-12.log");
        assert_eq!(files[9].file_name().unwrap(), "2026-07-03.log");
    }
}

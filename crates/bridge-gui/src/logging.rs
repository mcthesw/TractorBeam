use std::{
    error::Error,
    fmt::Display,
    fs, io,
    path::{Path, PathBuf},
    sync::Mutex,
};

use tracing_appender::non_blocking::WorkerGuard;
use tracing_subscriber::{EnvFilter, fmt, prelude::*};
use tractor_beam_core::{
    ClientLogSink, ClientSessionLogContext, LogLevel, bundle_config_path, emit_client_log_event,
};

static PROCESS_LOG_GUARD: Mutex<Option<WorkerGuard>> = Mutex::new(None);

#[derive(Debug)]
pub(crate) struct ClientLogInitError(io::Error);

impl Display for ClientLogInitError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        Display::fmt(&self.0, formatter)
    }
}

impl Error for ClientLogInitError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        Some(&self.0)
    }
}

impl From<io::Error> for ClientLogInitError {
    fn from(error: io::Error) -> Self {
        Self(error)
    }
}

#[derive(Debug)]
pub(crate) struct ClientLogFiles {
    root: PathBuf,
    client_dir: PathBuf,
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
    pub(crate) fn new() -> Result<Self, ClientLogInitError> {
        let root = bundle_log_root();
        let client_dir = root.join("client");
        fs::create_dir_all(&client_dir)
            .map_err(|error| {
                io::Error::new(
                    error.kind(),
                    format!(
                        "Could not create Client log directory {}: {error}",
                        client_dir.display()
                    ),
                )
            })
            .map_err(ClientLogInitError::from)?;
        init_process_tracing(&client_dir).map_err(|error| {
            ClientLogInitError::from(io::Error::new(
                error.kind(),
                format!("Could not initialize Client logging: {error}"),
            ))
        })?;
        Ok(Self { root, client_dir })
    }

    pub(crate) fn open_default_directory() -> io::Result<PathBuf> {
        let root = bundle_log_root();
        fs::create_dir_all(&root)?;
        open::that_detached(&root)?;
        Ok(root)
    }
}

impl ClientLogSink for ClientLogFiles {
    fn root(&self) -> Option<PathBuf> {
        Some(self.root.clone())
    }

    fn warnings(&self) -> Vec<String> {
        Vec::new()
    }

    fn log_files(&self) -> Vec<PathBuf> {
        tractor_beam_core::diagnostics::daily_log_files(&self.client_dir)
    }

    fn emit(&self, context: Option<&ClientSessionLogContext>, level: LogLevel, message: &str) {
        emit_client_log_event(context, level, message);
    }
}

fn init_process_tracing(directory: &Path) -> io::Result<()> {
    let mut process_log_guard = PROCESS_LOG_GUARD
        .lock()
        .map_err(|_| io::Error::other("Client log guard is poisoned"))?;
    if process_log_guard.is_some() {
        return Ok(());
    }

    let appender = LocalDailyAppender::new(directory)?;
    let (writer, guard) = tracing_appender::non_blocking(appender);
    let layer = fmt::layer()
        .with_ansi(false)
        .with_target(false)
        .with_writer(writer);
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));
    let subscriber = tracing_subscriber::registry().with(filter).with(layer);
    tracing::subscriber::set_global_default(subscriber).map_err(io::Error::other)?;
    *process_log_guard = Some(guard);
    Ok(())
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

use std::{
    fs, io,
    path::{Path, PathBuf},
    sync::OnceLock,
};

use basement_bridge_core::{
    ClientLogSink, ClientSessionLog, ClientSessionLogContext, LogLevel, PRODUCT_NAME,
    bundle_config_path, emit_client_log_event,
};
use directories::ProjectDirs;
use tracing::Dispatch;
use tracing_appender::{non_blocking::WorkerGuard, rolling};
use tracing_subscriber::{EnvFilter, fmt, prelude::*};

const SESSION_RETAIN_COUNT: usize = 10;

static PROCESS_LOG_GUARD: OnceLock<Option<WorkerGuard>> = OnceLock::new();

#[derive(Debug)]
pub(crate) struct ClientLogFiles {
    root: PathBuf,
    sessions_dir: PathBuf,
    process_log: PathBuf,
    warnings: Vec<String>,
}

#[derive(Debug)]
struct FileClientSessionLog {
    session_id: String,
    context: ClientSessionLogContext,
    dispatch: Dispatch,
    _guard: WorkerGuard,
}

impl ClientLogFiles {
    pub(crate) fn new() -> Self {
        let mut warnings = Vec::new();
        let root = choose_log_root(&mut warnings);
        let sessions_dir = root.join("sessions");
        let process_log = root.join("bridge-client.log");
        init_process_tracing(&root);
        Self {
            root,
            sessions_dir,
            process_log,
            warnings,
        }
    }
}

impl ClientLogSink for ClientLogFiles {
    fn root(&self) -> Option<PathBuf> {
        Some(self.root.clone())
    }

    fn warnings(&self) -> Vec<String> {
        self.warnings.clone()
    }

    fn process_log_path(&self) -> Option<PathBuf> {
        Some(self.process_log.clone())
    }

    fn recent_session_logs(&self) -> Vec<PathBuf> {
        session_log_files(&self.sessions_dir)
            .into_iter()
            .take(SESSION_RETAIN_COUNT)
            .collect()
    }

    fn start_session(
        &self,
        context: ClientSessionLogContext,
    ) -> io::Result<Box<dyn ClientSessionLog>> {
        fs::create_dir_all(&self.sessions_dir)?;
        prune_session_logs(&self.sessions_dir);
        let session_id = format!("{}-{}", unix_seconds(), std::process::id());
        let file_name = format!("session-{session_id}.log");
        let appender = rolling::never(&self.sessions_dir, file_name);
        let (writer, guard) = tracing_appender::non_blocking(appender);
        let subscriber = tracing_subscriber::registry().with(
            fmt::layer()
                .with_ansi(false)
                .with_target(false)
                .with_writer(writer),
        );
        Ok(Box::new(FileClientSessionLog {
            session_id,
            context,
            dispatch: Dispatch::new(subscriber),
            _guard: guard,
        }))
    }

    fn emit(&self, context: Option<&ClientSessionLogContext>, level: LogLevel, message: &str) {
        emit_client_log_event(context, None, level, message);
    }
}

impl ClientSessionLog for FileClientSessionLog {
    fn session_id(&self) -> &str {
        &self.session_id
    }

    fn context(&self) -> &ClientSessionLogContext {
        &self.context
    }

    fn emit(&self, level: LogLevel, message: &str) {
        tracing::dispatcher::with_default(&self.dispatch, || {
            emit_client_log_event(Some(&self.context), Some(&self.session_id), level, message);
        });
    }
}

fn init_process_tracing(root: &Path) {
    PROCESS_LOG_GUARD.get_or_init(|| {
        let appender = rolling::never(root, "bridge-client.log");
        let (writer, guard) = tracing_appender::non_blocking(appender);
        let layer = fmt::layer()
            .with_ansi(false)
            .with_target(false)
            .with_writer(writer);
        let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));
        let subscriber = tracing_subscriber::registry().with(filter).with(layer);
        match tracing::subscriber::set_global_default(subscriber) {
            Ok(()) => Some(guard),
            Err(_) => None,
        }
    });
}

fn choose_log_root(warnings: &mut Vec<String>) -> PathBuf {
    if let Some(bundle_dir) = bundle_directory() {
        let bundle_logs = bundle_dir.join("logs");
        if prepare_writable_dir(&bundle_logs).is_ok() {
            return bundle_logs;
        }
        warnings.push(format!(
            "Bundle log directory is not writable; using app data logs instead: {}",
            bundle_logs.display()
        ));
    }
    let fallback = app_data_log_root();
    let _ = fs::create_dir_all(&fallback);
    fallback
}

fn bundle_directory() -> Option<PathBuf> {
    bundle_config_path().and_then(|path| path.parent().map(Path::to_path_buf))
}

fn prepare_writable_dir(path: &Path) -> io::Result<()> {
    fs::create_dir_all(path)?;
    let probe = path.join(".write-test");
    fs::write(&probe, b"ok")?;
    fs::remove_file(probe)?;
    Ok(())
}

fn app_data_log_root() -> PathBuf {
    ProjectDirs::from("io.github", "mcthesw", PRODUCT_NAME)
        .map(|project| project.data_local_dir().join("logs"))
        .unwrap_or_else(|| std::env::temp_dir().join(PRODUCT_NAME).join("logs"))
}

fn prune_session_logs(directory: &Path) {
    for path in session_log_files(directory)
        .into_iter()
        .skip(SESSION_RETAIN_COUNT)
    {
        let _ = fs::remove_file(path);
    }
}

fn session_log_files(directory: &Path) -> Vec<PathBuf> {
    let mut files = fs::read_dir(directory)
        .into_iter()
        .flat_map(|entries| entries.filter_map(Result::ok))
        .map(|entry| entry.path())
        .filter(|path| path.extension().is_some_and(|extension| extension == "log"))
        .collect::<Vec<_>>();
    files.sort_by(|left, right| {
        file_sort_key(right)
            .cmp(&file_sort_key(left))
            .then_with(|| right.cmp(left))
    });
    files
}

fn file_sort_key(path: &Path) -> String {
    path.file_name()
        .and_then(|name| name.to_str())
        .unwrap_or_default()
        .to_owned()
}

fn unix_seconds() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |duration| duration.as_secs())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn retention_keeps_recent_session_logs() {
        let root =
            std::env::temp_dir().join(format!("basement-bridge-log-retention-{}", unix_seconds()));
        let sessions = root.join("sessions");
        fs::create_dir_all(&sessions).unwrap();
        for index in 0..12 {
            fs::write(
                sessions.join(format!("session-20260610-{index:02}.log")),
                "log",
            )
            .unwrap();
        }

        prune_session_logs(&sessions);
        let files = session_log_files(&sessions);

        assert_eq!(files.len(), 10);
        assert!(
            files
                .iter()
                .all(|path| !path.to_string_lossy().contains("-00.log"))
        );
        let _ = fs::remove_dir_all(root);
    }
}

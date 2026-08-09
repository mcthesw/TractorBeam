use std::sync::Mutex;

pub(crate) static TRACING_SUBSCRIBER_LOCK: Mutex<()> = Mutex::new(());

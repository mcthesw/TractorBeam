use std::io;

use thiserror::Error;

#[derive(Debug, Error)]
pub enum InjectorError {
    #[error("Isaac process was not found")]
    ProcessNotFound,
    #[error("Native Hook DLL was not found near the Bridge Client")]
    MissingNativeHook,
    #[error("Injector helper was not found near the Bridge Client")]
    MissingInjector,
    #[error("Native Hook injection is not supported on this platform yet")]
    UnsupportedPlatform,
    #[error("{0}")]
    Io(#[from] io::Error),
    #[error("Native Hook injection failed: {0}")]
    Injection(String),
}

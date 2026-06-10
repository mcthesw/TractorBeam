use std::{fmt, io};

use thiserror::Error;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum InjectionStep {
    ResolvePaths,
    HelperProcess,
    OpenProcess,
    AllocateRemoteMemory,
    WriteDllPath,
    ResolveLoadLibrary,
    CreateRemoteThread,
    WaitForRemoteThread,
    ReadRemoteThreadExit,
}

impl fmt::Display for InjectionStep {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let step = match self {
            Self::ResolvePaths => "resolve paths",
            Self::HelperProcess => "injector helper",
            Self::OpenProcess => "open Isaac process",
            Self::AllocateRemoteMemory => "allocate remote memory",
            Self::WriteDllPath => "write DLL path",
            Self::ResolveLoadLibrary => "resolve LoadLibraryW",
            Self::CreateRemoteThread => "create remote thread",
            Self::WaitForRemoteThread => "wait for remote thread",
            Self::ReadRemoteThreadExit => "read remote thread exit",
        };
        formatter.write_str(step)
    }
}

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
    #[error("Native Hook injection failed at {step}: {source}")]
    StepIo {
        step: InjectionStep,
        #[source]
        source: io::Error,
    },
    #[error("Native Hook injection failed at {step}: {message}")]
    Injection {
        step: InjectionStep,
        message: String,
    },
}

impl InjectorError {
    pub fn step_io(step: InjectionStep, source: io::Error) -> Self {
        Self::StepIo { step, source }
    }

    pub fn injection(step: InjectionStep, message: impl Into<String>) -> Self {
        Self::Injection {
            step,
            message: message.into(),
        }
    }

    #[must_use]
    pub fn is_access_denied(&self) -> bool {
        match self {
            Self::Io(error) | Self::StepIo { source: error, .. } => {
                error.raw_os_error() == Some(5) || error.kind() == io::ErrorKind::PermissionDenied
            }
            Self::Injection { message, .. } => {
                let lower = message.to_ascii_lowercase();
                lower.contains("access is denied") || lower.contains("access denied")
            }
            _ => false,
        }
    }
}

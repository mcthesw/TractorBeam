use std::{fmt, io};

use thiserror::Error;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum InjectionStep {
    ResolvePaths,
    HelperProcess,
    InspectModules,
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
            Self::InspectModules => "inspect loaded modules",
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
    #[error("Admin permission was cancelled")]
    AdminPermissionCancelled,
    #[error("Native Hook is already loaded in the Isaac process")]
    NativeHookAlreadyLoaded,
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
    #[error("Native Hook elevated retry failed after access denied: {message}")]
    ElevatedRetryFailed { message: String },
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

    pub fn elevated_retry_failed(message: impl Into<String>) -> Self {
        Self::ElevatedRetryFailed {
            message: message.into(),
        }
    }

    #[must_use]
    pub fn is_access_denied(&self) -> bool {
        match self {
            Self::AdminPermissionCancelled | Self::ElevatedRetryFailed { .. } => true,
            Self::Io(error) | Self::StepIo { source: error, .. } => {
                error.raw_os_error() == Some(5) || error.kind() == io::ErrorKind::PermissionDenied
            }
            Self::Injection { message, .. } => {
                let lower = message.to_ascii_lowercase();
                lower.contains("access is denied")
                    || lower.contains("access denied")
                    || lower.contains("os error 5")
                    || message.contains("拒绝访问")
            }
            _ => false,
        }
    }

    #[must_use]
    pub fn is_admin_permission_cancelled(&self) -> bool {
        matches!(self, Self::AdminPermissionCancelled)
    }

    #[must_use]
    pub fn is_native_hook_already_loaded(&self) -> bool {
        matches!(self, Self::NativeHookAlreadyLoaded)
            || self
                .to_string()
                .contains("Native Hook is already loaded in the Isaac process")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classifies_helper_message_access_denied_variants() {
        for message in [
            "injector helper exited with exit code: 1: access denied",
            "injector helper exited with exit code: 1: Access is denied.",
            "injector helper exited with exit code: 1: open Isaac process: os error 5",
            "injector helper exited with exit code: 1: open Isaac process: 拒绝访问。 (os error 5)",
        ] {
            assert!(
                InjectorError::injection(InjectionStep::HelperProcess, message).is_access_denied(),
                "{message}"
            );
        }
    }

    #[test]
    fn classifies_raw_os_error_five_as_access_denied() {
        let error =
            InjectorError::step_io(InjectionStep::OpenProcess, io::Error::from_raw_os_error(5));

        assert!(error.is_access_denied());
    }

    #[test]
    fn admin_permission_cancelled_is_explicit() {
        let error = InjectorError::AdminPermissionCancelled;

        assert!(error.is_admin_permission_cancelled());
        assert!(error.is_access_denied());
        assert_eq!(error.to_string(), "Admin permission was cancelled");
    }

    #[test]
    fn classifies_stale_hook_reported_by_injector_helper() {
        let error = InjectorError::injection(
            InjectionStep::HelperProcess,
            "Native Hook is already loaded in the Isaac process",
        );

        assert!(error.is_native_hook_already_loaded());
    }
}

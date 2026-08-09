use std::{fs, io, path::Path};

use crate::{InjectionStep, InjectorError};

const REPORT_MAGIC: &str = "tractor-beam-injector-result-v1";

pub fn write_failure_report(path: &Path, error: &InjectorError) -> io::Result<()> {
    let kind = match error {
        InjectorError::NativeHookAlreadyLoaded => "native_hook_already_loaded",
        InjectorError::InjectionCancelled => "injection_cancelled",
        InjectorError::ProcessNotFound => "process_not_found",
        InjectorError::UnsupportedPlatform => "unsupported_platform",
        error if error.is_access_denied() => "access_denied",
        _ => "injection_failed",
    };
    let message = error.to_string().replace(['\r', '\n'], " ");
    fs::write(path, format!("{REPORT_MAGIC}\n{kind}\n{message}\n"))
}

pub fn read_failure_report(path: &Path) -> io::Result<Option<InjectorError>> {
    let contents = fs::read_to_string(path)?;
    Ok(parse_failure_report(&contents))
}

fn parse_failure_report(contents: &str) -> Option<InjectorError> {
    let mut lines = contents.lines();
    if lines.next()? != REPORT_MAGIC {
        return None;
    }
    let kind = lines.next()?;
    let message = lines.collect::<Vec<_>>().join(" ");
    Some(match kind {
        "native_hook_already_loaded" => InjectorError::NativeHookAlreadyLoaded,
        "injection_cancelled" => InjectorError::InjectionCancelled,
        "process_not_found" => InjectorError::ProcessNotFound,
        "unsupported_platform" => InjectorError::UnsupportedPlatform,
        "access_denied" | "injection_failed" => InjectorError::injection(
            InjectionStep::HelperProcess,
            format!("elevated injector helper: {message}"),
        ),
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stale_hook_report_preserves_typed_error() {
        let report =
            format!("{REPORT_MAGIC}\nnative_hook_already_loaded\nNative Hook is already loaded\n");

        assert!(matches!(
            parse_failure_report(&report),
            Some(InjectorError::NativeHookAlreadyLoaded)
        ));
    }

    #[test]
    fn access_denied_report_preserves_message_and_category() {
        let report = format!(
            "{REPORT_MAGIC}\naccess_denied\nNative Hook injection failed at inspect loaded modules: Access is denied. (os error 5)\n"
        );

        let error = parse_failure_report(&report).expect("valid report");

        assert!(error.is_access_denied());
        assert!(error.to_string().contains("inspect loaded modules"));
    }

    #[test]
    fn rejects_unknown_report_versions_and_kinds() {
        assert!(parse_failure_report("other\naccess_denied\nmessage").is_none());
        assert!(parse_failure_report(&format!("{REPORT_MAGIC}\nother\nmessage")).is_none());
    }
}

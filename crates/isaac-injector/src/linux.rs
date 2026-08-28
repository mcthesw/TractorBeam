//! Proton runs Isaac inside pressure-vessel, so the host cannot use the Windows
//! remote-thread injector. The game install is bind-mounted into the container,
//! which makes a game-directory `winmm.dll` proxy a viable process-local entry.
//!
//! Deployment is session-scoped and reversible. The proxy forwards the complete
//! WinMM API to Proton's builtin DLL, while Proton's registry override is limited
//! to `isaac-ng.exe`. Tractor Beam never modifies Steam API DLLs, Steam userdata,
//! or the prefix Documents tree.

use std::{
    fs::{self, File, OpenOptions},
    io::{self, Write},
    path::{Path, PathBuf},
    process::{Command, Output},
    thread,
    time::Duration,
};

use crate::{InjectionStep, InjectorError};

const ISAAC_APP_ID: &str = "250900";
const ISAAC_WINDOWS_EXE: &str = "isaac-ng.exe";
const SIDECAR_DLL_NAME: &str = "winmm.dll";
const FORWARD_DLL_NAMES: [&str; 2] = ["winmm_orig.dll", "_winmm_orig.dll"];
const SIDECAR_MARKER_NAME: &str = "tractor-beam-winmm.sidecar";
const SIDECAR_MARKER_HEADER: &str = "tractor-beam-winmm-sidecar-v2";
const REGISTRY_KEY: &str = r"HKCU\Software\Wine\AppDefaults\isaac-ng.exe\DllOverrides";
const REGISTRY_VALUE: &str = "winmm";
const NATIVE_OVERRIDE: &str = "native,builtin";
const MAPS_RETRY_INTERVAL: Duration = Duration::from_millis(100);
const MAPS_RETRY_ATTEMPTS: u32 = 100;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ProtonWinmmSidecar {
    pub isaac_dir: PathBuf,
    pub dll: PathBuf,
    pub forward_dlls: [PathBuf; 2],
    pub marker: PathBuf,
}

impl ProtonWinmmSidecar {
    #[must_use]
    pub fn runtime_path(&self) -> PathBuf {
        self.isaac_dir
            .join("logs")
            .join("hook")
            .join("hook-runtime.txt")
    }

    fn from_isaac_dir(isaac_dir: &Path) -> Self {
        Self {
            isaac_dir: isaac_dir.to_path_buf(),
            dll: isaac_dir.join(SIDECAR_DLL_NAME),
            forward_dlls: FORWARD_DLL_NAMES.map(|name| isaac_dir.join(name)),
            marker: isaac_dir.join(SIDECAR_MARKER_NAME),
        }
    }

    fn managed_files(&self) -> impl Iterator<Item = &Path> {
        std::iter::once(self.dll.as_path()).chain(self.forward_dlls.iter().map(PathBuf::as_path))
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct OriginalOverride {
    key_existed: bool,
    value: Option<String>,
}

#[derive(Clone, Debug)]
struct ProtonRuntime {
    proton: PathBuf,
    compat_data: PathBuf,
    steam_root: PathBuf,
    builtin_winmm: PathBuf,
}

impl ProtonRuntime {
    fn discover(isaac_dir: &Path) -> Result<Self, InjectorError> {
        let steamapps = isaac_dir
            .parent()
            .and_then(Path::parent)
            .filter(|path| path.file_name().is_some_and(|name| name == "steamapps"))
            .ok_or_else(|| {
                injection_error(format!(
                    "Isaac install is not inside a Steam library: {}",
                    isaac_dir.display()
                ))
            })?;
        let compat_data = steamapps.join("compatdata").join(ISAAC_APP_ID);
        let config_info_path = compat_data.join("config_info");
        let config_info = fs::read_to_string(&config_info_path)
            .map_err(|error| step_io_with_path(error, &config_info_path))?;

        let proton_root = config_info
            .lines()
            .map(Path::new)
            .filter(|path| path.is_absolute())
            .find_map(|path| {
                path.ancestors()
                    .find(|ancestor| ancestor.join("proton").is_file())
                    .map(Path::to_path_buf)
            })
            .ok_or_else(|| {
                injection_error(format!(
                    "could not resolve Proton from {}",
                    config_info_path.display()
                ))
            })?;
        let proton = proton_root.join("proton");
        let steam_root = config_info
            .lines()
            .find_map(|line| {
                let path = PathBuf::from(line);
                (path.is_absolute() && path.join("steamapps").is_dir()).then_some(path)
            })
            .or_else(|| steamapps.parent().map(Path::to_path_buf))
            .ok_or_else(|| injection_error("could not resolve the Steam installation root"))?;
        let builtin_winmm = [
            proton_root
                .join("files")
                .join("lib")
                .join("wine")
                .join("i386-windows")
                .join("winmm.dll"),
            proton_root
                .join("files")
                .join("lib64")
                .join("wine")
                .join("i386-windows")
                .join("winmm.dll"),
        ]
        .into_iter()
        .find(|path| path.is_file())
        .ok_or_else(|| {
            injection_error(format!(
                "Proton builtin 32-bit winmm.dll was not found under {}",
                proton_root.display()
            ))
        })?;

        Ok(Self {
            proton,
            compat_data,
            steam_root,
            builtin_winmm,
        })
    }

    fn read_override(&self) -> Result<OriginalOverride, InjectorError> {
        let key_output = self.run_registry(["query", REGISTRY_KEY])?;
        let key_existed = key_output.status.success();
        if !key_existed && !is_registry_not_found(&key_output) {
            return Err(registry_command_error("query override key", &key_output));
        }

        let value_output = self.run_registry(["query", REGISTRY_KEY, "/v", REGISTRY_VALUE])?;
        let value = if value_output.status.success() {
            Some(parse_registry_string(&value_output).ok_or_else(|| {
                injection_error("Proton returned an unreadable winmm DLL override")
            })?)
        } else if is_registry_not_found(&value_output) {
            None
        } else {
            return Err(registry_command_error(
                "query winmm override",
                &value_output,
            ));
        };
        Ok(OriginalOverride { key_existed, value })
    }

    fn set_override(&self, value: &str) -> Result<(), InjectorError> {
        let output = self.run_registry([
            "add",
            REGISTRY_KEY,
            "/v",
            REGISTRY_VALUE,
            "/t",
            "REG_SZ",
            "/d",
            value,
            "/f",
        ])?;
        if output.status.success() {
            Ok(())
        } else {
            Err(registry_command_error("set winmm override", &output))
        }
    }

    fn restore_override(&self, original: &OriginalOverride) -> Result<(), InjectorError> {
        if let Some(value) = &original.value {
            return self.set_override(value);
        }

        let output = self.run_registry(["delete", REGISTRY_KEY, "/v", REGISTRY_VALUE, "/f"])?;
        if !output.status.success() && !is_registry_not_found(&output) {
            return Err(registry_command_error("remove winmm override", &output));
        }
        if !original.key_existed {
            self.remove_empty_override_key()?;
        }
        Ok(())
    }

    fn remove_empty_override_key(&self) -> Result<(), InjectorError> {
        let query = self.run_registry(["query", REGISTRY_KEY])?;
        if !query.status.success() {
            return if is_registry_not_found(&query) {
                Ok(())
            } else {
                Err(registry_command_error("inspect override key", &query))
            };
        }
        if registry_output_has_values(&query) {
            return Ok(());
        }
        let delete = self.run_registry(["delete", REGISTRY_KEY, "/f"])?;
        if delete.status.success() || is_registry_not_found(&delete) {
            Ok(())
        } else {
            Err(registry_command_error("remove empty override key", &delete))
        }
    }

    fn run_registry<const N: usize>(&self, args: [&str; N]) -> Result<Output, InjectorError> {
        Command::new(&self.proton)
            .arg("runinprefix")
            .arg("reg.exe")
            .args(args)
            .env("STEAM_COMPAT_DATA_PATH", &self.compat_data)
            .env("STEAM_COMPAT_CLIENT_INSTALL_PATH", &self.steam_root)
            .env("SteamAppId", ISAAC_APP_ID)
            .env("SteamGameId", ISAAC_APP_ID)
            .output()
            .map_err(|error| step_io_with_path(error, &self.proton))
    }
}

pub fn deploy_proton_winmm_sidecar(
    hook: &Path,
    isaac_dir: &Path,
) -> Result<ProtonWinmmSidecar, InjectorError> {
    if !hook.is_file() {
        return Err(InjectorError::MissingNativeHook);
    }
    if !isaac_dir.join(ISAAC_WINDOWS_EXE).is_file() {
        return Err(injection_error(format!(
            "Isaac Proton install was not found at {}",
            isaac_dir.display()
        )));
    }

    let runtime = ProtonRuntime::discover(isaac_dir)?;
    let sidecar = ProtonWinmmSidecar::from_isaac_dir(isaac_dir);
    if sidecar.marker.exists() {
        if read_marker(&sidecar.marker).is_none() {
            return Err(injection_error(format!(
                "refusing to use unrecognized sidecar marker {}",
                sidecar.marker.display()
            )));
        }
        remove_proton_winmm_sidecar_with_runtime(&sidecar, &runtime)?;
    }
    for path in sidecar.managed_files() {
        if path.exists() {
            return Err(injection_error(format!(
                "refusing to overwrite existing {} that was not created by Tractor Beam",
                path.display()
            )));
        }
    }

    remove_deployment_temps(&sidecar);
    let original = runtime.read_override()?;
    write_marker_atomically(&sidecar.marker, &original)?;

    let deploy_result = (|| {
        for destination in &sidecar.forward_dlls {
            copy_atomically(&runtime.builtin_winmm, destination)?;
        }
        copy_atomically(hook, &sidecar.dll)?;
        runtime.set_override(NATIVE_OVERRIDE)
    })();
    if let Err(error) = deploy_result {
        let rollback = remove_proton_winmm_sidecar_with_runtime(&sidecar, &runtime);
        return match rollback {
            Ok(()) => Err(error),
            Err(rollback_error) => Err(injection_error(format!(
                "{error}; rollback also failed: {rollback_error}"
            ))),
        };
    }

    Ok(sidecar)
}

pub fn remove_proton_winmm_sidecar(sidecar: &ProtonWinmmSidecar) -> Result<(), InjectorError> {
    let Some(_) = read_marker(&sidecar.marker) else {
        return Ok(());
    };
    let runtime = ProtonRuntime::discover(&sidecar.isaac_dir)?;
    remove_proton_winmm_sidecar_with_runtime(sidecar, &runtime)
}

pub fn recover_stale_proton_winmm_sidecar(isaac_dir: &Path) -> Result<bool, InjectorError> {
    let sidecar = ProtonWinmmSidecar::from_isaac_dir(isaac_dir);
    if !sidecar.marker.exists() {
        return Ok(false);
    }
    if read_marker(&sidecar.marker).is_none() {
        return Err(injection_error(format!(
            "refusing to use unrecognized sidecar marker {}",
            sidecar.marker.display()
        )));
    }
    let runtime = ProtonRuntime::discover(isaac_dir)?;
    remove_proton_winmm_sidecar_with_runtime(&sidecar, &runtime)?;
    Ok(true)
}

fn remove_proton_winmm_sidecar_with_runtime(
    sidecar: &ProtonWinmmSidecar,
    runtime: &ProtonRuntime,
) -> Result<(), InjectorError> {
    let Some(original) = read_marker(&sidecar.marker) else {
        return Ok(());
    };
    runtime.restore_override(&original)?;
    for path in sidecar.managed_files() {
        remove_if_exists(path)?;
    }
    remove_deployment_temps(sidecar);
    remove_if_exists(&sidecar.runtime_path())?;
    remove_if_exists(&sidecar.marker)?;
    let hook_logs = sidecar.isaac_dir.join("logs").join("hook");
    let _ = fs::remove_dir(&hook_logs);
    let _ = fs::remove_dir(sidecar.isaac_dir.join("logs"));
    Ok(())
}

pub fn verify_proton_sidecar_loaded(pid: u32, _hook: &Path) -> Result<(), InjectorError> {
    for _ in 0..MAPS_RETRY_ATTEMPTS {
        match process_maps_contain_sidecar(pid) {
            Ok(true) => return Ok(()),
            Ok(false) => thread::sleep(MAPS_RETRY_INTERVAL),
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                thread::sleep(MAPS_RETRY_INTERVAL);
            }
            Err(error) => {
                return Err(InjectorError::step_io(InjectionStep::InspectModules, error));
            }
        }
    }
    Err(InjectorError::injection(
        InjectionStep::InspectModules,
        format!("Proton Native Hook sidecar was not loaded in process {pid}"),
    ))
}

fn write_marker_atomically(
    marker: &Path,
    original: &OriginalOverride,
) -> Result<(), InjectorError> {
    let contents = format!(
        "{SIDECAR_MARKER_HEADER}\nkey_existed={}\noriginal_override={}\n",
        u8::from(original.key_existed),
        original
            .value
            .as_deref()
            .map(hex_encode)
            .unwrap_or_else(|| "-".to_owned())
    );
    write_atomically(marker, contents.as_bytes())
}

fn read_marker(path: &Path) -> Option<OriginalOverride> {
    let contents = fs::read_to_string(path).ok()?;
    let mut lines = contents.lines();
    if lines.next()? != SIDECAR_MARKER_HEADER {
        return None;
    }
    let key_existed = match lines.next()?.strip_prefix("key_existed=")? {
        "0" => false,
        "1" => true,
        _ => return None,
    };
    let encoded = lines.next()?.strip_prefix("original_override=")?;
    let value = if encoded == "-" {
        None
    } else {
        Some(String::from_utf8(hex_decode(encoded)?).ok()?)
    };
    (lines.next().is_none()).then_some(OriginalOverride { key_existed, value })
}

fn copy_atomically(source: &Path, destination: &Path) -> Result<(), InjectorError> {
    let temporary = deployment_temp(destination);
    remove_file_quietly(&temporary);
    let mut input = File::open(source).map_err(|error| step_io_with_path(error, source))?;
    let mut output = OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(&temporary)
        .map_err(|error| step_io_with_path(error, &temporary))?;
    io::copy(&mut input, &mut output).map_err(|error| step_io_with_path(error, &temporary))?;
    output
        .sync_all()
        .map_err(|error| step_io_with_path(error, &temporary))?;
    fs::rename(&temporary, destination).map_err(|error| step_io_with_path(error, destination))
}

fn write_atomically(destination: &Path, contents: &[u8]) -> Result<(), InjectorError> {
    let temporary = deployment_temp(destination);
    remove_file_quietly(&temporary);
    let mut output = OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(&temporary)
        .map_err(|error| step_io_with_path(error, &temporary))?;
    output
        .write_all(contents)
        .map_err(|error| step_io_with_path(error, &temporary))?;
    output
        .sync_all()
        .map_err(|error| step_io_with_path(error, &temporary))?;
    fs::rename(&temporary, destination).map_err(|error| step_io_with_path(error, destination))
}

fn deployment_temp(path: &Path) -> PathBuf {
    let name = path
        .file_name()
        .map_or_else(|| "sidecar".into(), |name| name.to_string_lossy());
    path.with_file_name(format!(".{name}.tractor-beam-tmp"))
}

fn remove_deployment_temps(sidecar: &ProtonWinmmSidecar) {
    remove_file_quietly(&deployment_temp(&sidecar.marker));
    for path in sidecar.managed_files() {
        remove_file_quietly(&deployment_temp(path));
    }
}

fn remove_file_quietly(path: &Path) {
    let _ = fs::remove_file(path);
}

fn remove_if_exists(path: &Path) -> Result<(), InjectorError> {
    match fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(step_io_with_path(error, path)),
    }
}

fn process_maps_contain_sidecar(pid: u32) -> io::Result<bool> {
    let maps = fs::read_to_string(format!("/proc/{pid}/maps"))?;
    Ok(maps.lines().any(is_sidecar_map_line))
}

fn is_sidecar_map_line(line: &str) -> bool {
    let lower = line.to_ascii_lowercase();
    if !lower.contains("winmm.dll") {
        return false;
    }
    !lower.contains("system32") && !lower.contains("/lib/wine/") && !lower.contains("\\lib\\wine\\")
}

fn parse_registry_string(output: &Output) -> Option<String> {
    String::from_utf8_lossy(&output.stdout)
        .lines()
        .find_map(|line| {
            let (_, value) = line.split_once("REG_SZ")?;
            Some(value.trim().to_owned())
        })
}

fn registry_output_has_values(output: &Output) -> bool {
    String::from_utf8_lossy(&output.stdout)
        .lines()
        .any(|line| line.contains("REG_"))
}

fn is_registry_not_found(output: &Output) -> bool {
    output.status.code() == Some(1)
        && [&output.stdout, &output.stderr].into_iter().any(|bytes| {
            let text = String::from_utf8_lossy(bytes).to_ascii_lowercase();
            text.contains("unable to find")
                || text.contains("cannot find")
                || text.contains("not found")
                || text.contains("找不到")
        })
}

fn registry_command_error(action: &str, output: &Output) -> InjectorError {
    let details = String::from_utf8_lossy(&output.stderr);
    let details = details.trim();
    injection_error(if details.is_empty() {
        format!("Proton could not {action} (exit status {})", output.status)
    } else {
        format!("Proton could not {action}: {details}")
    })
}

fn step_io_with_path(error: io::Error, path: &Path) -> InjectorError {
    InjectorError::injection(
        InjectionStep::DeploySidecar,
        format!("{}: {error}", path.display()),
    )
}

fn injection_error(message: impl Into<String>) -> InjectorError {
    InjectorError::injection(InjectionStep::DeploySidecar, message)
}

fn hex_encode(value: &str) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut encoded = String::with_capacity(value.len() * 2);
    for byte in value.bytes() {
        encoded.push(HEX[(byte >> 4) as usize] as char);
        encoded.push(HEX[(byte & 0x0f) as usize] as char);
    }
    encoded
}

fn hex_decode(value: &str) -> Option<Vec<u8>> {
    if !value.len().is_multiple_of(2) {
        return None;
    }
    value
        .as_bytes()
        .chunks_exact(2)
        .map(|digits| Some((hex_digit(digits[0])? << 4) | hex_digit(digits[1])?))
        .collect()
}

fn hex_digit(value: u8) -> Option<u8> {
    match value {
        b'0'..=b'9' => Some(value - b'0'),
        b'a'..=b'f' => Some(value - b'a' + 10),
        b'A'..=b'F' => Some(value - b'A' + 10),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::process::ExitStatusExt;

    #[test]
    fn marker_round_trips_original_override() {
        let directory = tempfile::tempdir().expect("create temp dir");
        let marker = directory.path().join(SIDECAR_MARKER_NAME);
        let original = OriginalOverride {
            key_existed: true,
            value: Some("builtin,native".to_owned()),
        };

        write_marker_atomically(&marker, &original).expect("write marker");

        assert_eq!(read_marker(&marker), Some(original));
    }

    #[test]
    fn sidecar_map_line_ignores_system_and_forward_dlls() {
        assert!(is_sidecar_map_line(
            "00400000-00401000 r-xp 00000000 00:00 0  /home/user/.steam/steamapps/common/The Binding of Isaac Rebirth/winmm.dll"
        ));
        assert!(is_sidecar_map_line(
            r"00400000-00401000 r-xp 00000000 00:00 0  Z:\home\user\.steam\steamapps\common\The Binding of Isaac Rebirth\winmm.dll"
        ));
        assert!(!is_sidecar_map_line(
            r"7f000000-7f001000 r-xp 00000000 00:00 0  C:\windows\system32\winmm.dll"
        ));
        assert!(!is_sidecar_map_line("/usr/lib/wine/i386-windows/winmm.dll"));
        assert!(!is_sidecar_map_line(
            "/home/user/.steam/steamapps/common/The Binding of Isaac Rebirth/_winmm_orig.dll"
        ));
    }

    #[test]
    fn refuses_to_parse_foreign_or_partial_marker() {
        let directory = tempfile::tempdir().expect("create temp dir");
        let marker = directory.path().join(SIDECAR_MARKER_NAME);
        fs::write(&marker, b"another-tool\n").expect("write foreign marker");
        assert_eq!(read_marker(&marker), None);

        fs::write(&marker, format!("{SIDECAR_MARKER_HEADER}\nkey_existed=1\n"))
            .expect("write partial marker");
        assert_eq!(read_marker(&marker), None);
    }

    #[test]
    fn distinguishes_missing_registry_values_from_command_failures() {
        let missing = Output {
            status: std::process::ExitStatus::from_raw(1 << 8),
            stdout: Vec::new(),
            stderr: b"reg: Unable to find the specified registry key".to_vec(),
        };
        let failed = Output {
            status: std::process::ExitStatus::from_raw(1 << 8),
            stdout: Vec::new(),
            stderr: b"reg: Invalid syntax".to_vec(),
        };

        assert!(is_registry_not_found(&missing));
        assert!(!is_registry_not_found(&failed));
    }
}

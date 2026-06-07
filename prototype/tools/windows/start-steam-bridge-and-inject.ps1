param(
    [ValidateSet("mirror", "replace")]
    [string]$Mode = "replace",
    [switch]$NoSteamFallback,
    [int]$GameId = 250900,
    [string]$Dll = "",
    [string]$Injector = "",
    [int]$TimeoutSeconds = 30
)

$ErrorActionPreference = "Stop"

$toolsRoot = Split-Path -Parent $PSScriptRoot
$prototypeRoot = Split-Path -Parent $toolsRoot
$nativeBuild = Join-Path $prototypeRoot "native-hook\build\x86-clang-rel"
if (-not $Dll) {
    $Dll = Join-Path $nativeBuild "isaac_eos_probe.dll"
}
if (-not $Injector) {
    $Injector = Join-Path $nativeBuild "eos_probe_injector.exe"
}

$logDir = Join-Path $env:USERPROFILE "Documents\My Games\Binding of Isaac Repentance+\online_logs"
New-Item -ItemType Directory -Force -Path $logDir | Out-Null
$configPath = Join-Path $logDir "isaac_bridge_config.txt"
$fallbackToSteam = if ($NoSteamFallback) { "0" } else { "1" }
@(
    "mode=$Mode",
    "fallback_to_steam=$fallbackToSteam",
    "sidecar=127.0.0.1:25900",
    "bind=127.0.0.1:25901"
) | Set-Content -LiteralPath $configPath -Encoding ascii

$env:ISAAC_BRIDGE_MODE = $Mode
if ($NoSteamFallback) {
    $env:ISAAC_BRIDGE_NO_STEAM_FALLBACK = "1"
} else {
    Remove-Item Env:\ISAAC_BRIDGE_NO_STEAM_FALLBACK -ErrorAction SilentlyContinue
}

$launcher = Join-Path $PSScriptRoot "start-steam-and-inject.ps1"

powershell -NoProfile -ExecutionPolicy Bypass -File $launcher `
    -GameId $GameId `
    -Dll $Dll `
    -Injector $Injector `
    -TimeoutSeconds $TimeoutSeconds

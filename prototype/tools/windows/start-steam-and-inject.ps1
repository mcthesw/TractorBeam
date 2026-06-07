param(
    [int]$GameId = 250900,
    [string]$Dll = ".\build\x86-clang\src\eos_probe\isaac_eos_probe.dll",
    [string]$Injector = ".\build\x86-clang\src\eos_probe\eos_probe_injector.exe",
    [int]$TimeoutSeconds = 30
)

$ErrorActionPreference = "Stop"

$dllPath = (Resolve-Path -LiteralPath $Dll).Path
$injectorPath = (Resolve-Path -LiteralPath $Injector).Path

Start-Process "steam://rungameid/$GameId"

$deadline = (Get-Date).AddSeconds($TimeoutSeconds)
$process = $null
while ((Get-Date) -lt $deadline) {
    $process = Get-CimInstance Win32_Process -Filter "name = 'isaac-ng.exe'" |
        Select-Object -First 1
    if ($process) {
        break
    }
    Start-Sleep -Milliseconds 250
}

if (-not $process) {
    throw "Timed out waiting for isaac-ng.exe."
}

& $injectorPath --pid $process.ProcessId --dll $dllPath

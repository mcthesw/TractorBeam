$ErrorActionPreference = "Stop"

$processes = Get-CimInstance Win32_Process -Filter "name = 'isaac-ng.exe'" |
    Select-Object ProcessId, ExecutablePath, CommandLine

if (-not $processes) {
    Write-Host "isaac-ng.exe is not running."
    exit 1
}

$processes | Format-Table -AutoSize

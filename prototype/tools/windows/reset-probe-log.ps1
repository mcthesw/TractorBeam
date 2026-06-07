param(
    [string]$Log = "$env:USERPROFILE\Documents\My Games\Binding of Isaac Repentance+\online_logs\eos_probe.jsonl",
    [switch]$Archive
)

$ErrorActionPreference = "Stop"

if (-not (Test-Path -LiteralPath $Log)) {
    Write-Host "No probe log found: $Log"
    exit 0
}

if ($Archive) {
    $directory = Split-Path -Parent $Log
    $stamp = Get-Date -Format "yyyyMMdd-HHmmss"
    $archivePath = Join-Path $directory "eos_probe.$stamp.jsonl"
    Move-Item -LiteralPath $Log -Destination $archivePath
    Write-Host "Archived probe log to $archivePath"
} else {
    Remove-Item -LiteralPath $Log
    Write-Host "Removed probe log: $Log"
}

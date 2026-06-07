param(
    [Parameter(Mandatory = $true)]
    [string]$Room,
    [Parameter(Mandatory = $true)]
    [string]$SteamId,
    [Parameter(Mandatory = $true)]
    [string]$Relay
)

$ErrorActionPreference = "Stop"

$toolsRoot = Split-Path -Parent $PSScriptRoot
$sidecar = Join-Path $toolsRoot "python\bridge_sidecar.py"

python $sidecar --room $Room --steam-id $SteamId --relay $Relay

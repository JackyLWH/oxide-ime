# 開發建置和測試共用發行版本及檔名。
function Get-ImeRelease([string]$Version, [switch]$PrivateDiscord) {
    $project = Split-Path -Parent $PSScriptRoot
    $outputs = Join-Path $project 'outputs'
    $binaries = if ($PrivateDiscord) { Join-Path $project 'work\private-discord\outputs' } else { $outputs }
    $suffix = if ($PrivateDiscord) { '-Private' } else { '' }
    if (-not $Version) { $Version = [IO.File]::ReadAllText((Join-Path $project 'release-version.txt')).Trim() }
    if ($Version -cnotmatch '^v[1-9][0-9]*\.[0-9]+(?:\.[0-9]+)?$') {
        throw "無效的發行版本：$Version"
    }
    $number = $Version.Substring(1)
    if (($number.Split('.')).Count -eq 2) { $number += '.0' }
    [pscustomobject]@{
        Version = $Version
        Number = $number
        Project = $project
        Outputs = $outputs
        Binaries = $binaries
        Dll = Join-Path $binaries "oxide_ime_$Version.dll"
        DllX86 = Join-Path $binaries "oxide_ime_$($Version)_x86.dll"
        Broker = Join-Path $binaries "oxide_ime_broker_$Version.exe"
        Maintenance = Join-Path $binaries "oxide_ime_maintenance_$Version.exe"
        Setup = Join-Path $outputs "OxideIme-Setup-$Version$suffix.exe"
    }
}

function Get-ImeProjectSuffix([string]$Project) {
    $path = [IO.Path]::GetFullPath($Project).TrimEnd('\').ToLowerInvariant()
    $digest = [Security.Cryptography.SHA256]::HashData([Text.Encoding]::UTF8.GetBytes($path))
    [BitConverter]::ToString($digest, 0, 8).Replace('-', '')
}

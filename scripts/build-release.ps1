param(
    [string]$Version,
    [switch]$Offline,
    [switch]$PrivateDiscord
)
$ErrorActionPreference = 'Stop'
. (Join-Path $PSScriptRoot 'release-info.ps1')
$release = Get-ImeRelease $Version -PrivateDiscord:$PrivateDiscord
$target = Join-Path $release.Project $(if ($PrivateDiscord) { 'work\target-private-discord' } else { 'work\target-release' })
$artifacts = [ordered]@{
    'x86_64-pc-windows-msvc\release\oxide_ime.dll' = $release.Dll
    'i686-pc-windows-msvc\release\oxide_ime.dll' = $release.DllX86
    'x86_64-pc-windows-msvc\release\oxide-ime-broker.exe' = $release.Broker
    'x86_64-pc-windows-msvc\release\oxide-ime-maintenance.exe' = $release.Maintenance
}
$cargoArgs = @('build', '--release', '--locked', '--no-default-features', '--target-dir', $target)
if ($PrivateDiscord) { $cargoArgs += @('--features', 'private-discord') }
if ($Offline) { $cargoArgs += '--offline' }
# 成品的 Rust panic 路徑使用固定前綴。單獨指定編譯旗標時仍保留靜態 CRT。
$rustFlags = @('-C', 'target-feature=+crt-static')
$profileDirectory = [Environment]::GetFolderPath('UserProfile')
$prefixes = @(@($profileDirectory, '/build-user'), @($release.Project, '/oxide-ime'))
foreach ($mapping in $prefixes) {
    $original, $replacement = $mapping
    foreach ($prefix in @($original, $original.Replace('\','/'), "\\?\$original") | Select-Object -Unique) {
        $rustFlags += '--remap-path-prefix', "$prefix=$replacement"
    }
}
$activeVersion = $env:RUST_IME_RELEASE
$activeFlags = $env:CARGO_ENCODED_RUSTFLAGS
$env:RUST_IME_RELEASE = $release.Version
$env:CARGO_ENCODED_RUSTFLAGS = $rustFlags -join [char]31
Push-Location -LiteralPath $release.Project
try {
    & cargo @cargoArgs --target x86_64-pc-windows-msvc --lib --bins
    if ($LASTEXITCODE -ne 0) { throw 'x64 版本建置失敗。' }
    & cargo @cargoArgs --target i686-pc-windows-msvc --lib
    if ($LASTEXITCODE -ne 0) { throw 'x86 版本建置失敗。' }
} finally {
    Pop-Location
    $env:RUST_IME_RELEASE = $activeVersion
    $env:CARGO_ENCODED_RUSTFLAGS = $activeFlags
}
New-Item -ItemType Directory -Path $release.Binaries -Force | Out-Null
foreach ($source in $artifacts.Keys) {
    Copy-Item -LiteralPath (Join-Path $target $source) -Destination $artifacts[$source] -Force
}
$artifacts.Values

param(
    [string]$Version,
    [switch]$Offline,
    [switch]$PrivateDiscord
)
$ErrorActionPreference = 'Stop'
. (Join-Path $PSScriptRoot 'release-info.ps1')
$release = Get-ImeRelease $Version -PrivateDiscord:$PrivateDiscord
$compiler = (Get-Command ISCC.exe -ErrorAction Stop).Source
$setupName = [IO.Path]::GetFileNameWithoutExtension($release.Setup)
$work = Join-Path $release.Project "work\installer-$setupName"
$licenses = Join-Path $release.Project 'licenses'
$buildTemplates = Join-Path $release.Project 'assets\templates\build'
$dictionary = Join-Path $release.Project 'assets\templates\cangjie3.dict.yaml'
$header = Get-Content -LiteralPath $dictionary -TotalCount 1
if ($header -cnotmatch '^# Cangjie3-Plus，來源修訂 [0-9a-f]{40}$' -or
    -not ([IO.File]::ReadAllText((Join-Path $licenses 'Cangjie3-Plus-MIT.txt')).StartsWith('MIT License'))) {
    throw '公用倉頡範本及授權必須取自 Cangjie3-Plus。'
}
$commands = @(
    '---', '...',
    "__rime_clipboard_raw__`tzv", "__rime_clipboard_s2t__`tzv",
    "__rime_time__`tztime", "__rime_date__`tzdate"
)
$custom = [IO.File]::ReadAllLines((Join-Path $release.Project 'assets\templates\cangjie3_custom.dict.yaml'))
if (($custom -join "`n") -cne ($commands -join "`n")) {
    throw '公開倉頡附加詞庫的範本必須只含四個指令標記。'
}
& (Join-Path $PSScriptRoot 'build-release.ps1') -Version $release.Version -Offline:$Offline -PrivateDiscord:$PrivateDiscord
New-Item -Path $work -ItemType Directory -Force | Out-Null
$compilerArgs = @(
    "/DRelease=$($release.Version)", "/DAppNumber=$($release.Number)",
    "/DBinaries=$($release.Binaries)", "/DLicenses=$licenses", "/DBuildTemplates=$buildTemplates",
    "/DOutputFolder=$work", "/DSetupFileName=$setupName",
    (Join-Path $PSScriptRoot 'setup.iss')
)
& $compiler @compilerArgs
if ($LASTEXITCODE -ne 0) { throw '安裝程式編譯失敗。' }
New-Item -ItemType Directory -Path (Split-Path -Parent $release.Setup) -Force | Out-Null
Move-Item -LiteralPath (Join-Path $work (Split-Path -Leaf $release.Setup)) -Destination $release.Setup -Force
$release.Setup

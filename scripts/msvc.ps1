function Get-ImeMsvcEnvironment {
    param([ValidateSet('x64', 'x86')][string]$Architecture = 'x64')
    $vswhere = Join-Path ${env:ProgramFiles(x86)} 'Microsoft Visual Studio\Installer\vswhere.exe'
    if (-not (Test-Path -LiteralPath $vswhere -PathType Leaf)) { throw '找不到 Visual Studio 安裝查詢工具。' }
    $vs = & $vswhere -latest -products '*' -requires Microsoft.VisualStudio.Component.VC.Tools.x86.x64 -property installationPath
    if ($LASTEXITCODE -ne 0 -or -not $vs) { throw "找不到 MSVC $Architecture 建置工具。" }
    $batch = if ($Architecture -eq 'x86') { 'vcvars32.bat' } else { 'vcvars64.bat' }
    $vcvars = Join-Path $vs "VC\Auxiliary\Build\$batch"
    if (-not (Test-Path -LiteralPath $vcvars -PathType Leaf)) { throw "找不到 $batch。" }
    $vcvars
}

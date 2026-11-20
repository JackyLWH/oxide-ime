param(
    [string]$Dll,
    [ValidateSet('x64', 'x86')][string]$Architecture = 'x64',
    [string]$LongText,
    [switch]$FixtureOnly
)
$ErrorActionPreference = 'Stop'
$root = Split-Path $PSScriptRoot -Parent
. (Join-Path $root 'scripts\release-info.ps1')
$release = Get-ImeRelease
if (-not $Dll) { $Dll = if ($Architecture -eq 'x86') { $release.DllX86 } else { $release.Dll } }
$dllPath = (Resolve-Path -LiteralPath $Dll).Path
if (-not $FixtureOnly) {
    $class = 'SOFTWARE\Classes\CLSID\{4BD0700E-5DD1-4310-A9D2-F3D579D54730}'
    $view = if ($Architecture -eq 'x86') { 'Registry32' } else { 'Registry64' }
    $base = [Microsoft.Win32.RegistryKey]::OpenBaseKey('LocalMachine', $view)
    try {
        $key = $base.OpenSubKey("$class\InprocServer32")
        if ($key) { try { $registered = $key.GetValue('') } finally { $key.Dispose() } }
    } finally { $base.Dispose() }
    if (-not $registered -or [IO.Path]::GetFullPath($registered) -ne $dllPath) {
        throw "$Architecture 測試 DLL 與目前註冊的輸入法不同。"
    }
}
. (Join-Path $root 'scripts\msvc.ps1')
$vcvars = Get-ImeMsvcEnvironment -Architecture $Architecture

$testWork = Join-Path $root "work\tsf-xaml-test\$Architecture"
$bin = Join-Path $testWork 'bin'
$fixture = Join-Path $testWork 'fixture'
New-Item -ItemType Directory -Force -Path $bin, (Join-Path $fixture 'outputs'), (Join-Path $fixture 'data\dictionaries') | Out-Null
$exe = Join-Path $bin 'tsf_xaml_test.exe'
$obj = Join-Path $bin 'tsf_xaml_test.obj'
$source = Join-Path $PSScriptRoot 'tsf_xaml_test.cpp'
$manifest = Join-Path $PSScriptRoot 'tsf_xaml_test.manifest'
$command = 'call "{0}" >nul && cl.exe /nologo /std:c++20 /utf-8 /EHsc /MD /DUNICODE /D_UNICODE /I"!WindowsSdkDir!Include\!WindowsSDKVersion!cppwinrt" /Fo"{1}" "{2}" /Fe:"{3}" /link /MANIFEST:EMBED /MANIFESTINPUT:"{4}" user32.lib runtimeobject.lib windowsapp.lib ole32.lib uuid.lib' -f $vcvars, $obj, $source, $exe, $manifest
& $env:ComSpec /d /v:on /c $command
if ($LASTEXITCODE -ne 0) { throw '無法建置 XAML 測試宿主。' }

if ($Architecture -eq 'x86') {
    $registrationExe = Join-Path $testWork 'registration_x86_test.exe'
    $registrationObj = Join-Path $testWork 'registration_x86_test.obj'
    $registrationSource = Join-Path $PSScriptRoot 'registration_x86_test.cpp'
    $registrationCommand = 'call "{0}" >nul && cl.exe /nologo /std:c++20 /utf-8 /EHsc /MD /Fo"{1}" "{2}" /Fe:"{3}" /link advapi32.lib' -f $vcvars, $registrationObj, $registrationSource, $registrationExe
    & $env:ComSpec /d /v:on /c $registrationCommand
    if ($LASTEXITCODE -ne 0) { throw '無法建置 x86 登錄測試宿主。' }
    & $registrationExe $dllPath
    if ($LASTEXITCODE -ne 0) { throw '32 位元 COM 獨立登錄測試失敗。' }
}

$fixtureDll = Join-Path $fixture 'outputs\rust_ime_fixture.dll'
Copy-Item -LiteralPath $dllPath -Destination $fixtureDll -Force
$utf8 = [Text.UTF8Encoding]::new($false)
$dictionaries = Join-Path $fixture 'data\dictionaries'
[IO.File]::WriteAllText((Join-Path $dictionaries 'cangjie3.dict.yaml'), "---`n...`n日`ta`n曰`ta`n髮`ts`n", $utf8)
[IO.File]::WriteAllText((Join-Path $dictionaries 'jyut6ping3.chars.dict.csv.json'),
    '{"測":{"ngo5":"常用"},"我":{"ngo5":"預設"}}', $utf8)
$t2s = Join-Path $dictionaries 't2s'
New-Item -ItemType Directory -Force -Path $t2s | Out-Null
$tables = Join-Path $root 'assets\templates\t2s'
$characters = [IO.File]::ReadAllText((Join-Path $tables 'TSCharacters.txt'))
[IO.File]::WriteAllText((Join-Path $t2s 'TSCharacters.txt'), $characters.Replace("髮`t发", "髮`t測"), $utf8)
Copy-Item -LiteralPath (Join-Path $tables 'TSPhrases.txt') -Destination $t2s -Force
$custom = "---`n...`n第一行。\n第二行🙂。\n最後一行。`tz`n"
$argsList = @('--fixture', $fixtureDll)
if ($LongText) {
    $LongText = (Resolve-Path -LiteralPath $LongText).Path
    $source = [IO.File]::ReadAllText($LongText).Replace("`r`n", "`n").TrimEnd([char]10)
    $custom += $source.Replace('\','\\').Replace("`n", '\n') + "`tzs`n"
    $argsList += $LongText
}
[IO.File]::WriteAllText((Join-Path $fixture 'data\dictionaries\cangjie3_custom.dict.yaml'), $custom, $utf8)
if (-not $FixtureOnly) {
    & $exe --registered $dllPath
    if ($LASTEXITCODE -ne 0) { throw '已註冊的輸入法未通過沉浸式載入測試。' }
}
& $exe @argsList
if ($LASTEXITCODE -ne 0) { throw '沉浸式 XAML 輸入測試失敗。' }

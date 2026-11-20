$ErrorActionPreference = 'Stop'
$root = Split-Path -Parent (Split-Path -Parent $PSScriptRoot)
. (Join-Path $root 'scripts\msvc.ps1')
$vc = Get-ImeMsvcEnvironment
$work = (New-Item -ItemType Directory -Force -Path (Join-Path $root 'work\verification\lol-entry-probe')).FullName
$outputs = (New-Item -ItemType Directory -Force -Path (Join-Path $root 'outputs')).FullName
$source = Join-Path $PSScriptRoot 'lol_entry_probe.cpp'
$exe = Join-Path $work 'OxideIme-LoL-EntryProbe-v3.exe'
$command = 'call "{0}" >nul && cl.exe /nologo /std:c++20 /utf-8 /O2 /EHsc /MT /W4 /WX /Fo"{1}\lol_entry_probe.obj" "{2}" /Fe:"{3}" /link advapi32.lib' -f $vc, $work, $source, $exe
& $env:ComSpec /d /c $command
if ($LASTEXITCODE -ne 0) { throw '無法建置 LoL 入口頁面診斷工具。' }
Copy-Item -LiteralPath $exe -Destination $outputs -Force -PassThru

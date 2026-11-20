#requires -RunAsAdministrator
param([string]$Version)
$ErrorActionPreference = 'Stop'
. (Join-Path $PSScriptRoot 'release-info.ps1')
$release = Get-ImeRelease $Version
$tool = Start-Process -FilePath $release.Maintenance -ArgumentList '--register' -WindowStyle Hidden -Wait -PassThru
if ($tool.ExitCode -ne 0) { throw '開發版 TSF 登錄失敗。請確認本機目前的 Oxide IME 登錄指向此專案。' }
& (Join-Path $PSScriptRoot 'sandbox-broker.ps1') -Version $release.Version
Write-Host "已登錄開發版 $($release.Version)。重新開啟應用程式後按 Win+Space 選 Oxide IME。"

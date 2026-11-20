#requires -RunAsAdministrator
$ErrorActionPreference = 'Stop'
. (Join-Path $PSScriptRoot 'release-info.ps1')
$release = Get-ImeRelease
$tool = Start-Process -FilePath $release.Maintenance -ArgumentList '--unregister' -WindowStyle Hidden -Wait -PassThru
if ($tool.ExitCode -ne 0) { throw '開發版 TSF 解除登錄失敗。' }
& (Join-Path $PSScriptRoot 'sandbox-broker.ps1') -Action Disable
Write-Host '已解除此專案的 Oxide IME 開發版登錄。'

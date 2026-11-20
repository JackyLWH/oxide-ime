#requires -RunAsAdministrator
param([string]$Version)
$ErrorActionPreference = 'Stop'
# 開發版及升級共用 Maintenance 的驗證和登錄流程。
& (Join-Path $PSScriptRoot 'register.ps1') -Version $Version

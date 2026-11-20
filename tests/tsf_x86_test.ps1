param(
    [string]$Dll,
    [string]$LongText,
    [switch]$FixtureOnly
)
# 保留既有的 32 位元測試入口，與 x64 使用同一套 XAML 測試。
& (Join-Path $PSScriptRoot 'tsf_xaml_test.ps1') -Architecture x86 @PSBoundParameters

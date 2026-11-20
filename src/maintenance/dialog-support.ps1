# 維護視窗共用的 WinForms 介面及設定儲存。
Add-Type -AssemblyName System.Windows.Forms
Add-Type -AssemblyName System.Drawing
$data = $env:OXIDE_IME_DATA
$setting = Join-Path $data 'setting'
$identity = [Security.Principal.WindowsIdentity]::GetCurrent()
$principal = [Security.Principal.WindowsPrincipal]::new($identity)
$elevated = $principal.IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)

function New-Dialog([string]$Title, [int]$Width, [int]$Height) {
    $form = [Windows.Forms.Form]::new()
    $form.Text = $Title
    $form.TopMost = $true
    $form.StartPosition = 'CenterScreen'
    $form.FormBorderStyle = 'FixedDialog'
    $form.MaximizeBox = $false
    $form.MinimizeBox = $false
    $form.Font = [Drawing.Font]::new('Microsoft JhengHei UI', 10)
    $form.ClientSize = [Drawing.Size]::new($Width, $Height)
    $form.AutoScaleMode = 'Dpi'
    $form
}

function New-Button([string]$Text, [int]$X, [int]$Y, [int]$Width = 82) {
    $button = [Windows.Forms.Button]::new()
    $button.Text = $Text
    $button.SetBounds($X, $Y, $Width, 30)
    $button
}

function New-DialogOwner {
    $owner = New-Dialog 'Oxide IME' 1 1
    $owner.ShowInTaskbar = $false
    $owner.Opacity = 0
    $owner.Show()
    $owner.Activate()
    $owner
}

function Show-Result([string]$Message, [switch]$ErrorIcon, [switch]$Confirm) {
    $owner = New-DialogOwner
    try {
        if ($Confirm) {
            return [Windows.Forms.MessageBox]::Show($owner, $Message, 'Oxide IME', 'YesNo', 'Warning', 'Button2') -eq 'Yes'
        }
        $icon = if ($ErrorIcon) { 'Error' } else { 'Information' }
        [void][Windows.Forms.MessageBox]::Show($owner, $Message, 'Oxide IME', 'OK', $icon)
    } finally { $owner.Dispose() }
}

function Require-User {
    if ($elevated) { throw '請從一般權限的應用程式修改使用者資料。' }
}

function Save-Setting([string]$Path, [string]$Value) {
    Require-User
    [void][IO.Directory]::CreateDirectory([IO.Path]::GetDirectoryName($Path))
    $temporary = $Path + '.' + [guid]::NewGuid().ToString('N') + '.tmp'
    try {
        [IO.File]::WriteAllText($temporary, $Value, [Text.UTF8Encoding]::new($false))
        if ([IO.File]::Exists($Path)) { [IO.File]::Replace($temporary, $Path, [System.Management.Automation.Language.NullString]::Value) }
        else { [IO.File]::Move($temporary, $Path) }
    } finally { [IO.File]::Delete($temporary) }
}

# 安裝版以受保護的 bin 界定登錄及程序歸屬。
$bin = $env:OXIDE_IME_BIN
$root = $env:OXIDE_IME_ROOT
$exe = $env:OXIDE_IME_EXE
$release = $env:OXIDE_IME_RELEASE
$installed = $env:OXIDE_IME_INSTALLED -eq '1'
$exe64 = Join-Path ([Environment]::SystemDirectory) 'regsvr32.exe'
$exe32 = Join-Path ([Environment]::GetFolderPath('Windows')) 'SysWOW64\regsvr32.exe'
$runName = 'OxideIme'
$dll64 = Join-Path $bin "oxide_ime_$release.dll"
$dll32 = Join-Path $bin "oxide_ime_$($release)_x86.dll"
$class = 'SOFTWARE\Classes\CLSID\{4BD0700E-5DD1-4310-A9D2-F3D579D54730}'
$serverKey = "$class\InprocServer32"
$profileKey = 'SOFTWARE\Microsoft\CTF\TIP\{4BD0700E-5DD1-4310-A9D2-F3D579D54730}\LanguageProfile\0x00000404\{21A75529-20A1-4CF2-B7D8-1BB94267E1C9}'
$pattern = 'v[1-9][0-9]*\.[0-9]+(?:\.[0-9]+)?'
if ($installed -and $root -ine (Join-Path ([Environment]::GetFolderPath('ProgramFiles')) 'OxideIME')) {
    throw '找不到受保護的 Oxide IME 安裝目錄。'
}

function Test-Owned([string]$Path, [string]$NamePattern) {
    $Path -and [IO.Path]::IsPathRooted($Path) -and
        [IO.Path]::GetDirectoryName([IO.Path]::GetFullPath($Path)) -ieq $bin -and
        [IO.Path]::GetFileName($Path) -match $NamePattern
}

function Assert-OrdinaryFile([string]$Path) {
    $file = Get-Item -LiteralPath $Path -Force
    if ($file.Attributes -band [IO.FileAttributes]::ReparsePoint) {
        throw "程式路徑含有重新解析點。$Path"
    }
}

function Invoke-ComServer([string]$Server, [string]$Dll, [switch]$Remove) {
    Assert-OrdinaryFile $Dll
    $arguments = @('/s')
    if ($Remove) { $arguments += '/u' }
    $arguments += ('"{0}"' -f $Dll)
    $process = Start-Process -FilePath $Server -ArgumentList $arguments -WindowStyle Hidden -Wait -PassThru
    if ($process.ExitCode -ne 0) {
        throw "無法更新 TSF 登錄（$([IO.Path]::GetFileName($Dll))，結束碼 $($process.ExitCode)）。"
    }
}

function Read-Server($View) {
    $key = $View.OpenSubKey($serverKey)
    if ($key) { try { $key.GetValue('') } finally { $key.Dispose() } }
}

function Set-Server($View, [string]$Dll) {
    $key = $View.OpenSubKey($serverKey, $true)
    if (-not $key) { throw '找不到既有 COM 登錄。' }
    try { $key.SetValue('', $Dll, 'String') } finally { $key.Dispose() }
}

function Set-ProfileIcon($View, [string]$Dll) {
    $key = $View.OpenSubKey($profileKey, $true)
    if (-not $key) { throw '找不到既有 TSF 語言設定檔。' }
    try { $key.SetValue('IconFile', $Dll, 'String') } finally { $key.Dispose() }
}

function Allow-SandboxRead([string]$Path) {
    Assert-OrdinaryFile $Path
    $acl = Get-Acl -LiteralPath $Path
    foreach ($sid in 'S-1-15-2-1', 'S-1-15-2-2') {
        $rule = [Security.AccessControl.FileSystemAccessRule]::new(
            [Security.Principal.SecurityIdentifier]::new($sid), 'ReadAndExecute', 'Allow')
        $acl.SetAccessRule($rule)
    }
    Set-Acl -LiteralPath $Path -AclObject $acl
}

function Stop-OwnedProcesses {
    $parent = [int]$env:OXIDE_IME_PARENT_PID
    foreach ($process in Get-Process -Name 'oxide_ime_broker_v*', 'oxide_ime_maintenance_v*' -ErrorAction SilentlyContinue) {
        if ($process.Id -ne $parent -and
            (Test-Owned $process.Path "^oxide_ime_(?:broker|maintenance)_$pattern\.exe$")) {
            try {
                $process.Kill()
                if (-not $process.WaitForExit(4000)) { throw '程序未能停止。' }
            } catch { if (-not $process.HasExited) { throw } }
        }
    }
}

Assert-OrdinaryFile $root
Assert-OrdinaryFile $bin
Assert-OrdinaryFile $exe

$view64 = [Microsoft.Win32.RegistryKey]::OpenBaseKey('LocalMachine', 'Registry64')
$view32 = [Microsoft.Win32.RegistryKey]::OpenBaseKey('LocalMachine', 'Registry32')
$login = $view64.OpenSubKey('SOFTWARE\Microsoft\Windows\CurrentVersion\Run', $true)
try {
    $old64 = Read-Server $view64
    $old32 = Read-Server $view32
    $oldRun = if ($installed) { $login.GetValue($runName) }
    if (($old64 -and -not (Test-Owned $old64 "^oxide_ime_$pattern\.dll$")) -or
        ($old32 -and -not (Test-Owned $old32 "^oxide_ime_$($pattern)_x86\.dll$"))) {
        throw 'Windows 正登記另一套 Oxide IME。請先在該安裝解除登錄。'
    }
    if ($oldRun -and ($oldRun -notmatch '^"(.+)" --session$' -or
        -not (Test-Owned $Matches[1] "^oxide_ime_maintenance_$pattern\.exe$"))) {
        throw '同名登入啟動項目由其他目錄持有。'
    }
    if ($env:OXIDE_IME_ACTION -eq '--prepare-upgrade') {
        Stop-OwnedProcesses
        return
    }
    $eventProvider = Get-WinEvent -ListProvider 'OxideIme' -ErrorAction SilentlyContinue
    $resource = if ($eventProvider) { [Environment]::ExpandEnvironmentVariables($eventProvider.ResourceFilePath) }
    if ($resource -and -not (Test-Owned $resource "^oxide_ime_maintenance_$pattern\.exe$")) {
        throw 'Oxide IME 事件通道由另一個安裝登記。'
    }
    $manifest = $env:OXIDE_IME_MANIFEST
    if ($env:OXIDE_IME_ACTION -eq '--unregister') {
        if ($old32) { Invoke-ComServer $exe32 $old32 -Remove }
        if ($old64) { Invoke-ComServer $exe64 $old64 -Remove }
        if ($oldRun) { $login.DeleteValue($runName) }
        Stop-OwnedProcesses
        if ($eventProvider) { Invoke-OxideEventUtility @('um', $manifest) }
        return
    }

    if ($installed) { Allow-SandboxRead $bin }
    Allow-SandboxRead $dll64
    Allow-SandboxRead $dll32
    if (-not $installed) {
        # EventLog 服務需要讀取開發樹內的訊息資源。
        $sid = [Security.Principal.NTAccount]::new('NT SERVICE', 'EventLog').Translate(
            [Security.Principal.SecurityIdentifier]).Value
        & (Join-Path ([Environment]::SystemDirectory) 'icacls.exe') $exe /grant ('*{0}:(RX)' -f $sid) | Out-Null
        if ($LASTEXITCODE -ne 0) { throw '無法授予 Windows 事件記錄服務讀取資源的權限。' }
    }
    $changed64 = $false
    $changed32 = $false
    $changedEvents = $false
    try {
        if ($old64) {
            $changed64 = $true
            Set-Server $view64 $dll64
            Set-ProfileIcon $view64 $dll64
        } else {
            Invoke-ComServer $exe64 $dll64
            $changed64 = $true
        }
        if ($old32) {
            $changed32 = $true
            Set-Server $view32 $dll32
        } else {
            Invoke-ComServer $exe32 $dll32
            $changed32 = $true
        }
        $detail = $eventProvider -and (Get-WinEvent -ListLog $diagnostic -ErrorAction SilentlyContinue).IsEnabled
        Invoke-OxideEventUtility @('im', $manifest, "/rf:$exe", "/mf:$exe")
        $changedEvents = $true
        if ($detail -and -not (Get-WinEvent -ListLog $diagnostic).IsEnabled) {
            Invoke-OxideEventUtility @('sl', $diagnostic, '/e:true')
        }
        if ($installed) { $login.SetValue($runName, ('"{0}" --session' -f $exe), 'String') }
    } catch {
        $failure = $_
        $errors = [Collections.Generic.List[string]]::new()
        try {
            if ($changedEvents) {
                if ($resource) { Invoke-OxideEventUtility @('im', $manifest, "/rf:$resource", "/mf:$resource") }
                else { Invoke-OxideEventUtility @('um', $manifest) }
            }
        } catch { $errors.Add($_.Exception.Message) }
        try {
            if ($changed32) {
                if ($old32) { Set-Server $view32 $old32 } else { Invoke-ComServer $exe32 $dll32 -Remove }
            }
        } catch { $errors.Add($_.Exception.Message) }
        try {
            if ($changed64) {
                if ($old64) { Set-Server $view64 $old64; Set-ProfileIcon $view64 $old64 }
                else { Invoke-ComServer $exe64 $dll64 -Remove }
            }
        } catch { $errors.Add($_.Exception.Message) }
        if ($errors.Count) { throw "$($failure.Exception.Message) 回復登錄失敗。$($errors -join '。')" }
        throw $failure
    }
} finally {
    if ($login) { $login.Dispose() }
    $view32.Dispose()
    $view64.Dispose()
}

#requires -RunAsAdministrator
param(
    [ValidateSet('Enable', 'Disable')]
    [string]$Action = 'Enable',
    [string]$Version
)
$ErrorActionPreference = 'Stop'
. (Join-Path $PSScriptRoot 'release-info.ps1')
$release = Get-ImeRelease $Version
$project = [IO.Path]::GetFullPath($release.Project).TrimEnd('\')
$outputs = $release.Outputs
$identity = [Security.Principal.WindowsIdentity]::GetCurrent()
$taskName = 'OxideIme-Development-' + (Get-ImeProjectSuffix $project)
$oldTask = Get-ScheduledTask -TaskName $taskName -ErrorAction SilentlyContinue
$oldWasRunning = $oldTask -and $oldTask.State -eq 'Running'
$oldXml = $null
if ($oldTask) {
    $actions = @($oldTask.Actions)
    $expectedRoot = [IO.Path]::GetFullPath($outputs) + [IO.Path]::DirectorySeparatorChar
    if ($actions.Count -ne 1 -or -not $actions[0].Execute -or
        -not [IO.Path]::GetFullPath($actions[0].Execute).StartsWith($expectedRoot, [StringComparison]::OrdinalIgnoreCase) -or
        [IO.Path]::GetFileName($actions[0].Execute) -notmatch '^oxide_ime_broker_v[1-9][0-9]*\.[0-9]+(?:\.[0-9]+)?\.exe$' -or
        $actions[0].Arguments) {
        throw '同名排程不屬於此專案，未更改。'
    }
    $oldXml = Export-ScheduledTask -TaskName $taskName
}
function Wait-BrokerStopped([string]$Executable) {
    $name = [IO.Path]::GetFileNameWithoutExtension($Executable)
    for ($attempt = 0; $attempt -lt 40; $attempt++) {
        $running = @(Get-Process -Name $name -ErrorAction SilentlyContinue | Where-Object { $_.Path -eq $Executable })
        if ($running.Count -eq 0) { return $true }
        Start-Sleep -Milliseconds 100
    }
    return $false
}

if ($Action -eq 'Disable') {
    if ($oldTask) {
        try {
            if ($oldWasRunning) {
                Stop-ScheduledTask -TaskName $taskName
                if (-not (Wait-BrokerStopped $actions[0].Execute)) { throw '舊字庫程序未能停止。' }
            }
            Unregister-ScheduledTask -TaskName $taskName -Confirm:$false
        } catch {
            $failure = $_
            try {
                if (-not (Get-ScheduledTask -TaskName $taskName -ErrorAction SilentlyContinue)) {
                    Register-ScheduledTask -TaskName $taskName -Xml $oldXml -Force | Out-Null
                }
                if ($oldWasRunning) { Start-ScheduledTask -TaskName $taskName }
            } catch {
                throw "停用失敗：$($failure.Exception.Message)；還原舊排程失敗：$($_.Exception.Message)"
            }
            throw $failure
        }
        Write-Host '已停止並移除此專案的沙盒字庫服務排程。'
    }
    return
}

$broker = $release.Broker
if (-not (Test-Path -LiteralPath $broker -PathType Leaf)) { throw "缺少所需檔案：$broker" }

$registered = $false
function Wait-Broker([string]$Executable) {
    for ($attempt = 0; $attempt -lt 20; $attempt++) {
        $probe = Start-Process -FilePath $Executable -ArgumentList '--check' -WindowStyle Hidden -Wait -PassThru
        if ($probe.ExitCode -eq 0) { return $true }
        Start-Sleep -Milliseconds 250
    }
    return $false
}
try {
    if ($oldWasRunning) {
        Stop-ScheduledTask -TaskName $taskName
        if (-not (Wait-BrokerStopped $actions[0].Execute)) { throw '舊字庫程序未能停止。' }
    }
    $taskAction = New-ScheduledTaskAction -Execute $broker
    $trigger = New-ScheduledTaskTrigger -AtLogOn -User $identity.Name
    $principal = New-ScheduledTaskPrincipal -UserId $identity.User.Value -LogonType Interactive -RunLevel Limited
    $settings = New-ScheduledTaskSettingsSet -AllowStartIfOnBatteries -DontStopIfGoingOnBatteries `
        -ExecutionTimeLimit (New-TimeSpan -Seconds 0) -MultipleInstances IgnoreNew `
        -RestartCount 3 -RestartInterval (New-TimeSpan -Minutes 1) -StartWhenAvailable
    $registered = $true
    Register-ScheduledTask -TaskName $taskName -Action $taskAction -Trigger $trigger `
        -Principal $principal -Settings $settings -Description 'Oxide IME 本機沙盒查字服務' -Force | Out-Null
    Start-ScheduledTask -TaskName $taskName
    if (-not (Wait-Broker $broker)) { throw '字庫服務未能啟動或回應。' }
} catch {
    $failure = $_
    $rollbackErrors = [Collections.Generic.List[string]]::new()
    if ($registered) {
        try {
            $task = Get-ScheduledTask -TaskName $taskName -ErrorAction SilentlyContinue
            if ($task -and $task.State -eq 'Running') {
                Stop-ScheduledTask -TaskName $taskName
                if (-not (Wait-BrokerStopped $task.Actions[0].Execute)) { throw '新字庫程序未能停止。' }
            }
            if ($task) { Unregister-ScheduledTask -TaskName $taskName -Confirm:$false }
        } catch { $rollbackErrors.Add("移除新排程：$($_.Exception.Message)") }
    }
    if ($oldXml) {
        try {
            Register-ScheduledTask -TaskName $taskName -Xml $oldXml -Force | Out-Null
            if ($oldWasRunning) {
                Start-ScheduledTask -TaskName $taskName
                if (-not (Wait-Broker $actions[0].Execute)) { throw '舊服務未回應。' }
            }
        } catch { $rollbackErrors.Add("還原舊排程：$($_.Exception.Message)") }
    }
    if ($rollbackErrors.Count) { throw "設定失敗：$($failure.Exception.Message)；回復失敗：$($rollbackErrors -join '；')" }
    throw $failure
}
Write-Host '已設定沙盒 DLL 讀取權限及登入後啟動的字庫服務。'

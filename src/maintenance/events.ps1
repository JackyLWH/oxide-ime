$ErrorActionPreference = 'Stop'
$env:PSModulePath = Join-Path $PSHOME 'Modules'
$wevtutil = Join-Path ([Environment]::SystemDirectory) 'wevtutil.exe'
$operational = 'OxideIme/Operational'
$diagnostic = 'OxideIme/Diagnostic'

function Invoke-OxideEventUtility([string[]]$Arguments, [switch]$Elevate) {
    if ($Elevate) {
        $process = Start-Process -FilePath $wevtutil -ArgumentList $Arguments -Verb RunAs -WindowStyle Hidden -Wait -PassThru
        $status = $process.ExitCode
    } else {
        & $wevtutil @Arguments
        $status = $LASTEXITCODE
    }
    if ($status -ne 0) {
        throw "Windows 事件操作失敗（$($Arguments[0])，結束碼 $status）。"
    }
}

function Read-OxideEvents([int]$Hours, [int]$Limit) {
    if (-not (Get-WinEvent -ListLog $operational -ErrorAction SilentlyContinue)) {
        throw 'Oxide IME 事件通道尚未註冊。'
    }
    $filter = @{ LogName = @($operational, $diagnostic); StartTime = (Get-Date).AddHours(-$Hours) }
    try { $events = Get-WinEvent -FilterHashtable $filter -MaxEvents $Limit }
    catch {
        if ($_.FullyQualifiedErrorId -like 'NoMatchingEventsFound,*') { return }
        throw
    }
    foreach ($record in $events) {
        $xml = [xml]$record.ToXml()
        $fields = @{}
        foreach ($field in $xml.Event.EventData.Data) {
            $fields[$field.Name] = $field.'#text'
        }
        [ordered]@{
            time_utc = $record.TimeCreated.ToUniversalTime().ToString('o')
            channel = $record.LogName
            id = $record.Id
            record = $record.RecordId
            pid = $record.ProcessId
            tid = $record.ThreadId
            release = $fields.Release
            host = $fields.Host
            component = $fields.Component
            operation = $fields.Operation
            details = $fields.Details | ConvertFrom-Json
        } | ConvertTo-Json -Depth 4 -Compress
    }
}

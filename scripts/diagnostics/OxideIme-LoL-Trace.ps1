#requires -Version 5.1
#requires -RunAsAdministrator
param(
    [ValidateRange(5, 600)][int]$Seconds = 600,
    [string]$OutputDirectory = $PSScriptRoot
)
$ErrorActionPreference = 'Stop'
$project = Join-Path $PSScriptRoot '..\..'
if (-not $PSBoundParameters.ContainsKey('OutputDirectory') -and
    (Test-Path -LiteralPath (Join-Path $project 'release-version.txt') -PathType Leaf)) {
    $OutputDirectory = Join-Path $project 'work\diagnostics\lol'
}

$kernel = [guid]'22fb2cd6-0e7b-422b-a0c7-2fad1fd0e716'
$com = [guid]'bf406804-6afa-46e7-8a48-6c357e1d6d61'
$imeProviders = [guid[]]@('e3736b92-a9a2-4e78-b611-4ea74ca506ea','9d7644e6-4f13-49a1-91cf-2f64c80c24b7')
$clsid = '4bd0700e-5dd1-4310-a9d2-f3d579d54730'
$gameImage = '^(?:League of Legends|LeagueClient(?:Ux(?:Render)?)?|RiotClient(?:Services|Ux(?:Render)?)?|Riot Client)\.exe$'
$imeImage = '(?:oxide|rust)_ime(?:_(?:stage\d{1,3}|v[1-9]\d*\.\d+(?:\.\d+)?)(?:_x86)?|_current)?\.dll'
$gamePids = [Collections.Generic.HashSet[int]]::new()
$counts = @{}
$events = [Collections.Generic.List[object]]::new()
$utf8 = [Text.UTF8Encoding]::new($false)
$traceName = 'OxideIme-LoL-' + [guid]::NewGuid().ToString('N')
$scratch = [IO.Directory]::CreateDirectory((Join-Path ([IO.Path]::GetTempPath()) $traceName)).FullName
$providerPath = Join-Path $scratch 'providers.txt'
$etl = Join-Path $scratch 'trace.etl'
$logman = Join-Path $env:WINDIR 'System32\logman.exe'
$out = [IO.Directory]::CreateDirectory([IO.Path]::GetFullPath($OutputDirectory)).FullName
$destination = Join-Path $out ("OxideIme-LoL-{0:yyyyMMdd-HHmmss}-{1}.jsonl" -f (Get-Date),$PID)

function Invoke-Logman([string[]]$Options) {
    $messages = & $logman @Options 2>&1
    if ($LASTEXITCODE -ne 0) { throw ("追蹤命令失敗。結束碼 {0}。{1}" -f $LASTEXITCODE, ($messages -join ' ')) }
}

function Event-Fields([xml]$Xml) {
    $fields = @{}
    foreach ($field in $Xml.Event.EventData.Data) {
        if ($field.Name) { $fields[$field.Name] = $field.'#text' }
    }
    $fields
}

function Image-Leaf([string]$Path) {
    [IO.Path]::GetFileName($Path)
}

$observed = @(Get-Process -Name 'League of Legends','LeagueClient','LeagueClientUx','LeagueClientUxRender','RiotClientServices','RiotClientUx','RiotClientUxRender','Riot Client' -ErrorAction SilentlyContinue)
foreach ($process in $observed) { [void]$gamePids.Add($process.Id) }
[IO.File]::WriteAllLines($providerPath, @(
    "$($kernel.ToString('B')) 0x50 4",
    "$($com.ToString('B')) 0x2000000000000000 4",
    "$($imeProviders[0].ToString('B')) 0xFFFFFFFFFFFFFFFF 5",
    "$($imeProviders[1].ToString('B')) 0xFFFFFFFFFFFFFFFF 5"
), [Text.Encoding]::ASCII)

$started = [DateTimeOffset]::UtcNow
$running = $false
$collected = $false
try {
    try {
        Invoke-Logman @('start', $traceName, '-ets', '-o', $etl, '-f', 'bincirc', '-max', '64',
            '-bs', '64', '-nb', '16', '64', '-pf', $providerPath)
        $running = $true
        Write-Host '追蹤已開始。請重現問題，完成後回到此視窗按 Enter。'
        Write-Host ("追蹤最多持續 {0} 秒。" -f $Seconds)
        $stopAt = [DateTimeOffset]::UtcNow.AddSeconds($Seconds)
        while ([DateTimeOffset]::UtcNow -lt $stopAt) {
            if (-not [Console]::IsInputRedirected -and [Console]::KeyAvailable -and
                [Console]::ReadKey($true).Key -eq [ConsoleKey]::Enter) { break }
            Start-Sleep -Milliseconds 200
        }
    } finally {
        if ($running) { Invoke-Logman @('stop', $traceName, '-ets') }
    }
    $finished = [DateTimeOffset]::UtcNow
    Write-Host '追蹤已停止，正在整理日誌。'

    Get-WinEvent -Path $etl -Oldest | ForEach-Object {
        $event = $_
        $provider = [string]$event.ProviderId
        $counts[$provider] = 1 + [int]$counts[$provider]
        if ($event.ProviderId -eq $kernel -and $event.Id -in 1,2,5,6) {
            $fields = Event-Fields ([xml]$event.ToXml())
            $name = Image-Leaf ([string]$fields.ImageName)
            $processEvent = $event.Id -in 1,2
            if (($processEvent -and $name -match $gameImage) -or ($event.Id -in 5,6 -and $name -match "^$imeImage$")) {
                $value = [string]$fields.ProcessID
                $targetPid = if ($value -match '^0x') { [Convert]::ToInt32($value.Substring(2), 16) }
                elseif ($value) { [int]$value } else { $event.ProcessId }
                if ($processEvent) { [void]$gamePids.Add($targetPid) }
                $events.Add([ordered]@{
                    utc = $event.TimeCreated.ToUniversalTime().ToString('o')
                    source = 'kernel'; id = $event.Id; pid = $targetPid; tid = $event.ThreadId
                    image = $name; exit_code = $fields.ExitCode; image_base = $fields.ImageBase
                    image_size = $fields.ImageSize
                })
            }
        } elseif ($event.ProviderId -in $imeProviders) {
            $fields = Event-Fields ([xml]$event.ToXml())
            $hostName = [string]$fields.Host
            if ($hostName -match $gameImage) {
                [void]$gamePids.Add($event.ProcessId)
                $events.Add([ordered]@{
                    utc = $event.TimeCreated.ToUniversalTime().ToString('o')
                    source = 'ime'; id = $event.Id; pid = $event.ProcessId; tid = $event.ThreadId
                    host = $hostName; release = $fields.Release
                    component = $fields.Component; operation = $fields.Operation
                    details = $fields.Details | ConvertFrom-Json
                })
            }
        } elseif ($event.ProviderId -eq $com) {
            $raw = $event.ToXml()
            if ($raw -match "$clsid|\b$imeImage\b") {
                $fields = Event-Fields ([xml]$raw)
                $numbers = @{}
                foreach ($field in $fields.Keys | Where-Object { $_ -match '^(?:clsid|iid|classid|result|status|hr|hresult|error|returncode|context|clsctx)$' }) {
                    if ($fields[$field] -match '^(?:-?\d+|0x[0-9a-f]+|\{?[0-9a-f-]{36}\}?)$') {
                        $numbers[$field] = $fields[$field]
                    }
                }
                $events.Add([ordered]@{
                    utc = $event.TimeCreated.ToUniversalTime().ToString('o')
                    source = 'com'; id = $event.Id; pid = $event.ProcessId; tid = $event.ThreadId
                    details = $numbers
                })
            }
        }
    }

    $writer = [IO.StreamWriter]::new($destination, $false, $utf8)
    try {
        $windows = Get-ItemProperty -LiteralPath 'HKLM:\SOFTWARE\Microsoft\Windows NT\CurrentVersion'
        $writer.WriteLine(([ordered]@{
            source = 'trace'; format = 1; started_utc = $started.ToString('o'); finished_utc = $finished.ToString('o')
            windows_build = "$($windows.CurrentBuild).$($windows.UBR)"
            process_ids = @($gamePids); provider_event_counts = $counts
        } | ConvertTo-Json -Depth 4 -Compress))
        foreach ($row in $events) {
            if ($gamePids.Contains([int]$row.pid)) {
                $writer.WriteLine(($row | ConvertTo-Json -Depth 4 -Compress))
            }
        }
    } finally { $writer.Dispose() }
    $collected = $true
    Write-Host "完成。請交回 $destination"
} finally {
    if ($collected -or -not $running) {
        [IO.File]::Delete($providerPath)
        [IO.File]::Delete($etl)
        [IO.Directory]::Delete($scratch, $false)
    } else {
        Write-Warning "未能完成篩選。原始追蹤留在本機的 $etl"
    }
}

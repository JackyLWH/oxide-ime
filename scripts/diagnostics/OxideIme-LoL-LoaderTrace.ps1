#requires -Version 5.1
#requires -RunAsAdministrator
[CmdletBinding(DefaultParameterSetName = 'Trace')]
param(
    [Parameter(ParameterSetName = 'Trace')]
    [ValidateRange(5, 120)][int]$Seconds = 90,
    [Parameter(Mandatory, ParameterSetName = 'History')]
    [DateTimeOffset]$At,
    [Parameter(ParameterSetName = 'History')]
    [ValidateRange(1, 120)][int]$WindowSeconds = 30,
    [Parameter(ParameterSetName = 'History')]
    [ValidateRange(1, 2147483647)][int]$GameProcessId,
    [string]$OutputDirectory = $PSScriptRoot
)
$ErrorActionPreference = 'Stop'
$project = Join-Path $PSScriptRoot '..\..'
if (-not $PSBoundParameters.ContainsKey('OutputDirectory') -and
    (Test-Path -LiteralPath (Join-Path $project 'release-version.txt') -PathType Leaf)) {
    $OutputDirectory = Join-Path $project 'work\diagnostics\lol'
}

$historical = $PSCmdlet.ParameterSetName -eq 'History'
if (-not $historical -and -not ('OxideImeMemoryTrace' -as [type])) {
    Add-Type -TypeDefinition @'
using System;
using System.ComponentModel;
using System.Runtime.InteropServices;
[StructLayout(LayoutKind.Sequential)]
internal struct OxideImeWnodeHeader
{
    public uint BufferSize, ProviderId;
    public ulong HistoricalContext;
    public long TimeStamp;
    public Guid Guid;
    public uint ClientContext, Flags;
}

[StructLayout(LayoutKind.Sequential)]
internal struct OxideImeTraceProperties
{
    public OxideImeWnodeHeader Wnode;
    public uint BufferSize, MinimumBuffers, MaximumBuffers, MaximumFileSize, LogFileMode, FlushTimer,
                EnableFlags, AgeLimit, NumberOfBuffers, FreeBuffers, EventsLost, BuffersWritten,
                LogBuffersLost, RealTimeBuffersLost;
    public IntPtr LoggerThreadId;
    public uint LogFileNameOffset, LoggerNameOffset;
}

public static class OxideImeMemoryTrace
{
    private const uint TraceSystemLogger = 0x02000000;
    private const uint TraceFileCircular = 0x00000002;
    private const uint TraceMemoryPageFaults = 0x00001000;
    private const uint TraceNoSysConfig = 0x10000000;

    [DllImport("advapi32.dll", CharSet = CharSet.Unicode)]
    private static extern uint StartTrace(out ulong handle, string name, IntPtr properties);

    [DllImport("advapi32.dll", CharSet = CharSet.Unicode)]
    private static extern uint ControlTrace(ulong handle, string name, IntPtr properties, uint control);

    public static ulong Start(string name, string path)
    {
        OxideImeTraceProperties data = new OxideImeTraceProperties();
        data.Wnode.Guid = Guid.NewGuid();
        data.Wnode.ClientContext = 1;
        data.Wnode.Flags = 0x00020000; // WNODE_FLAG_TRACED_GUID
        data.BufferSize = 64;
        data.MinimumBuffers = 8;
        data.MaximumBuffers = 64;
        data.MaximumFileSize = 160;
        data.LogFileMode = TraceSystemLogger | TraceFileCircular;
        data.EnableFlags = TraceMemoryPageFaults | TraceNoSysConfig;
        data.FlushTimer = 1;
        int size = Marshal.SizeOf(typeof(OxideImeTraceProperties));
        data.LoggerNameOffset = (uint)size;
        data.LogFileNameOffset = (uint)(size + 2 * (name.Length + 1));
        data.Wnode.BufferSize = data.LogFileNameOffset + (uint)(2 * (path.Length + 1));
        IntPtr buffer = Marshal.AllocHGlobal((int)data.Wnode.BufferSize);
        try
        {
            Marshal.StructureToPtr(data, buffer, false);
            Marshal.Copy((name + '\0').ToCharArray(), 0, IntPtr.Add(buffer, (int)data.LoggerNameOffset), name.Length + 1);
            Marshal.Copy((path + '\0').ToCharArray(), 0, IntPtr.Add(buffer, (int)data.LogFileNameOffset), path.Length + 1);
            ulong handle;
            uint code = StartTrace(out handle, name, buffer);
            if (code != 0) throw new Win32Exception((int)code);
            return handle;
        }
        finally { Marshal.FreeHGlobal(buffer); }
    }

    public static uint[] Stop(ulong handle, string name)
    {
        OxideImeTraceProperties data = new OxideImeTraceProperties();
        data.Wnode.BufferSize = (uint)Marshal.SizeOf(typeof(OxideImeTraceProperties)) + 4096;
        data.LoggerNameOffset = (uint)Marshal.SizeOf(typeof(OxideImeTraceProperties));
        data.LogFileNameOffset = data.LoggerNameOffset + 1024;
        IntPtr buffer = Marshal.AllocHGlobal((int)data.Wnode.BufferSize);
        try
        {
            Marshal.StructureToPtr(data, buffer, false);
            uint code = ControlTrace(handle, name, buffer, 1); // EVENT_TRACE_CONTROL_STOP
            if (code != 0) throw new Win32Exception((int)code);
            data = (OxideImeTraceProperties)Marshal.PtrToStructure(buffer, typeof(OxideImeTraceProperties));
            return new uint[] { data.EventsLost, data.LogBuffersLost };
        }
        finally { Marshal.FreeHGlobal(buffer); }
    }
}
'@
}

$kernel = [guid]'22fb2cd6-0e7b-422b-a0c7-2fad1fd0e716'
$com = [guid]'bf406804-6afa-46e7-8a48-6c357e1d6d61'
$loader = [guid]'b059b83f-d946-4b13-87ca-4292839dc2f2'
$pageFaultProvider = [guid]'3d6fa8d3-fe05-11d0-9dda-00c04fd7ba7c'
$imeProviders = [guid[]]@('e3736b92-a9a2-4e78-b611-4ea74ca506ea','9d7644e6-4f13-49a1-91cf-2f64c80c24b7')
$clsid = '4bd0700e-5dd1-4310-a9d2-f3d579d54730'
$gameImage = '^(?:League of Legends|LeagueClient(?:Ux(?:Render)?)?|RiotClient(?:Services|Ux(?:Render)?)?|Riot Client)\.exe$'
$imeImage = '(?:oxide|rust)_ime(?:_(?:stage\d{1,3}|v[1-9]\d*\.\d+(?:\.\d+)?)(?:_x86)?|_current)?\.dll'
$gamePids = [Collections.Generic.HashSet[int]]::new()
$counts = @{}
$events = [Collections.Generic.List[object]]::new()
$utf8 = [Text.UTF8Encoding]::new($false)
$out = [IO.Directory]::CreateDirectory([IO.Path]::GetFullPath($OutputDirectory)).FullName

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

function Read-ProtectionEvents([datetime]$Start, [datetime]$End) {
    $channels = [ordered]@{
        'Microsoft-Windows-CodeIntegrity/Operational' = @(3004,3033,3034,3076,3077,3089)
        'Microsoft-Windows-Security-Mitigations/KernelMode' = @(1,2,5,6,7,8,11,12)
        'Microsoft-Windows-Security-Mitigations/UserMode' = @(1,2,5,6,7,8,11,12,13,14,15,16,17,18,19,20,21,22,23,24)
        'Microsoft-Windows-WER-Diag/Operational' = @(5)
    }
    $sysmonChannel = 'Microsoft-Windows-Sysmon/Operational'
    $imageFields = @('FileNameBuffer','File Name','ImageName','TargetModName','CallReturnModName')
    $hostFields = @('ProcessNameBuffer','Process Name','ProcessPath','AppPath')
    if ($historical) {
        $channels['Microsoft-Windows-Security-Mitigations/KernelMode'] = 1..24
        $channels['Microsoft-Windows-Security-Mitigations/UserMode'] = 1..24
        $channels['Application'] = @(1000,1001)
        $channels[$sysmonChannel] = @(7,10)
        $imageFields += @('ImageLoaded','ModuleName','ModulePath','FaultingModuleName','P4')
        $hostFields += @('Image','TargetImage','AppName','FaultingApplicationName','P1')
    }
    $status = [ordered]@{}
    foreach ($channel in $channels.Keys) {
        try {
            $log = Get-WinEvent -ListLog $channel -ErrorAction Stop
            $status[$channel] = [ordered]@{ enabled = $log.IsEnabled; read = $false; matches = 0 }
            $query = @{ LogName = $channel; Id = $channels[$channel]; StartTime = $Start; EndTime = $End }
            if ($channel -eq 'Application') {
                $query.ProviderName = @('Application Error','Windows Error Reporting')
            }
            $selected = [Collections.Generic.List[object]]::new()
            $signed = [Collections.Generic.List[object]]::new()
            $activities = [Collections.Generic.HashSet[guid]]::new()
            try {
                Get-WinEvent -FilterHashtable $query -ErrorAction Stop | ForEach-Object {
                    $event = $_
                    $fields = Event-Fields ([xml]$event.ToXml())
                    $images = @($imageFields |
                        ForEach-Object { Image-Leaf ([string]$fields[$_]) } | Where-Object { $_ -match "^$imeImage$" } |
                        Select-Object -Unique)
                    $hosts = @($hostFields |
                        ForEach-Object { Image-Leaf ([string]$fields[$_]) } | Where-Object { $_ -match $gameImage } |
                        Select-Object -Unique)
                    $signature = $channel -eq 'Microsoft-Windows-CodeIntegrity/Operational' -and $event.Id -eq 3089
                    $pidMatch = $false
                    if ($GameProcessId -and $channel -ne $sysmonChannel) {
                        $pidMatch = $GameProcessId -in @('TargetProcessId','FaultingProcessId','ProcessId' |
                            ForEach-Object { Parse-Address ([string]$fields[$_]) })
                    }
                    $matchesTarget = $images.Count -or $hosts.Count -or $pidMatch
                    if ($channel -eq $sysmonChannel -and $event.Id -eq 7) {
                        $matchesTarget = $images.Count -and $hosts.Count
                    }
                    if ($matchesTarget -or $signature) {
                        $details = [ordered]@{}
                        foreach ($key in 'Status','RequestedPolicy','ValidatedPolicy','Requested Signing Level',
                            'Validated Signing Level','RequiredSignatureLevel','SignatureLevel','TotalSignatureCount',
                            'VerificationError','ProcessId','CallingProcessId','TargetThreadId','CallingThreadId',
                            'TargetProtect','TargetState','TargetType','TargetModOffset','ExceptionCode',
                            'FaultingOffset','GrantedAccess','TargetProcessId','SourceProcessId') {
                            if ($fields[$key] -match '^(?:-?\d+|0x[0-9a-fA-F]+|[0-9a-fA-F]{8})$') {
                                $details[$key] = $fields[$key]
                            }
                        }
                        if ($channel -eq $sysmonChannel -and $event.Id -eq 10) {
                            $callerImage = Image-Leaf ([string]$fields['SourceImage'])
                            if ($callerImage -match '\.exe$') { $details.CallerImage = $callerImage }
                        }
                        $row = [ordered]@{
                            utc = $event.TimeCreated.ToUniversalTime().ToString('o')
                            source = 'protection'; channel = $channel; id = $event.Id; record_id = $event.RecordId
                            activity_id = $event.ActivityId; images = $images; hosts = $hosts; details = $details
                        }
                        if ($matchesTarget) {
                            $selected.Add($row)
                            if ($event.ActivityId -and $event.ActivityId -ne [guid]::Empty) {
                                [void]$activities.Add($event.ActivityId)
                            }
                        } else { $signed.Add($row) }
                    }
                }
            } catch {
                if ($_.FullyQualifiedErrorId -notlike 'NoMatchingEventsFound,*') { throw }
            }
            foreach ($row in $signed) {
                if ($row.activity_id -and $activities.Contains($row.activity_id)) { $selected.Add($row) }
            }
            $status[$channel].read = $true
            $status[$channel].matches = $selected.Count
            foreach ($row in $selected) { $events.Add($row) }
        } catch {
            if (-not $status.Contains($channel)) { $status[$channel] = [ordered]@{ enabled = $null; read = $false } }
            $status[$channel].error = ($_.FullyQualifiedErrorId -split ',', 2)[0]
        }
    }
    $status
}

function Parse-Address([string]$Value) {
    if ($Value -match '^0x') { return [Convert]::ToUInt64($Value.Substring(2), 16) }
    if ($Value -match '^\d+$') { return [UInt64]$Value }
    return [UInt64]0
}

if ($historical) {
    $start = $At.UtcDateTime.AddSeconds(-$WindowSeconds)
    $end = $At.UtcDateTime.AddSeconds($WindowSeconds)
    $protectionLogs = Read-ProtectionEvents $start $end
    $destination = Join-Path $out ("OxideIme-LoL-Windows-{0:yyyyMMdd-HHmmss}-{1}.jsonl" -f (Get-Date),$PID)
    $writer = [IO.StreamWriter]::new($destination, $false, $utf8)
    try {
        $writer.WriteLine(([ordered]@{
            source = 'windows_event_history'; format = 1; fault_utc = $At.UtcDateTime.ToString('o')
            started_utc = $start.ToString('o'); finished_utc = $end.ToString('o')
            game_pid = $GameProcessId; protection_logs = $protectionLogs
        } | ConvertTo-Json -Compress -Depth 8))
        foreach ($row in ($events | Sort-Object { $_['utc'] })) {
            $writer.WriteLine(($row | ConvertTo-Json -Compress -Depth 8))
        }
    } finally { $writer.Dispose() }
    Write-Output $destination
    return
}

$traceName = 'OxideIme-LoL-' + [guid]::NewGuid().ToString('N')
$scratch = [IO.Directory]::CreateDirectory((Join-Path ([IO.Path]::GetTempPath()) $traceName)).FullName
$providerPath = Join-Path $scratch 'providers.txt'
$etl = Join-Path $scratch 'trace.etl'
$faultEtl = Join-Path $scratch 'page-fault.etl'
$logman = Join-Path $env:WINDIR 'System32\logman.exe'
$destination = Join-Path $out ("OxideIme-LoL-{0:yyyyMMdd-HHmmss}-{1}.jsonl" -f (Get-Date),$PID)

function Find-ImeFaults {
    $mapped = @{}
    $lifetimes = [Collections.Generic.List[object]]::new()
    foreach ($row in ($events | Sort-Object { $_['utc'] })) {
        if ($row.source -ne 'kernel' -or $row.id -notin 5,6 -or -not $gamePids.Contains([int]$row.pid)) { continue }
        $identity = "$($row.pid):$($row.image_base)"
        if ($row.id -eq 5) {
            $image = [pscustomobject]@{
                pid = $row.pid; image = $row.image; base = (Parse-Address $row.image_base); size = (Parse-Address $row.image_size)
                begin = [DateTimeOffset]::Parse($row.utc, [Globalization.CultureInfo]::InvariantCulture)
                end = $finished
            }
            $mapped[$identity] = $image
            $lifetimes.Add($image)
        } elseif ($mapped.ContainsKey($identity)) {
            $mapped[$identity].end = [DateTimeOffset]::Parse($row.utc, [Globalization.CultureInfo]::InvariantCulture)
            $mapped.Remove($identity)
        }
    }

    $query = "*[System[Provider[@Guid='$($pageFaultProvider.ToString('B'))'] and Opcode=15]]"
    try {
        Get-WinEvent -Path $faultEtl -Oldest -FilterXPath $query | ForEach-Object {
            $event = $_
            if ($gamePids.Contains([int]$event.ProcessId)) {
                $when = $event.TimeCreated.ToUniversalTime()
                $xml = [xml]$event.ToXml()
                $fields = Event-Fields $xml
                $address = Parse-Address $fields.VirtualAddress
                $instruction = Parse-Address $fields.ProgramCounter
                # 有些 Windows 版本沒有登錄傳統 ETW 事件的欄位名稱。64 位元事件依次存放兩個指標。
                if (-not $address -and -not $instruction) {
                    $payload = [string]$xml.Event.ProcessingErrorData.EventPayload
                    if ($payload -match '^[0-9a-fA-F]{32}$') {
                        $bytes = [byte[]]@(for ($index = 0; $index -lt 32; $index += 2) {
                            [Convert]::ToByte($payload.Substring($index, 2), 16)
                        })
                        $address = [BitConverter]::ToUInt64($bytes, 0)
                        $instruction = [BitConverter]::ToUInt64($bytes, 8)
                    }
                }
                foreach ($image in $lifetimes) {
                    if ($image.pid -ne $event.ProcessId -or $when -lt $image.begin.UtcDateTime -or $when -gt $image.end.UtcDateTime) { continue }
                    $limit = $image.base + $image.size
                    $insideIp = $instruction -ge $image.base -and $instruction -lt $limit
                    $insideAddress = $address -ge $image.base -and $address -lt $limit
                    if ($insideIp -or $insideAddress) {
                        $events.Add([ordered]@{
                            utc = $when.ToString('o'); source = 'memory'; id = 15
                            pid = $event.ProcessId; tid = $event.ThreadId; image = $image.image
                            instruction_rva = $(if ($insideIp) { '0x{0:x}' -f ($instruction - $image.base) } else { $null })
                            accessed_rva = $(if ($insideAddress) { '0x{0:x}' -f ($address - $image.base) } else { $null })
                            same_address = $instruction -eq $address
                        })
                    }
                }
            }
        }
    } catch {
        if ($_.FullyQualifiedErrorId -notlike 'NoMatchingEventsFound,*') { throw }
    }
}

function Registered-ImeImage {
    try {
        $key = Get-Item -LiteralPath 'Registry::HKEY_LOCAL_MACHINE\SOFTWARE\Classes\CLSID\{4BD0700E-5DD1-4310-A9D2-F3D579D54730}\InprocServer32'
        $registered = [Environment]::ExpandEnvironmentVariables([string]$key.GetValue(''))
        $name = Image-Leaf $registered
        if ($name -notmatch "^$imeImage$") { return $null }
        $file = [IO.File]::OpenRead($registered)
        $reader = [IO.BinaryReader]::new($file)
        try {
            if ($reader.ReadUInt16() -ne 0x5a4d) { return $null }
            $file.Position = 0x3c
            $pe = $reader.ReadUInt32()
            $file.Position = $pe
            if ($reader.ReadUInt32() -ne 0x4550) { return $null }
            $file.Position = $pe + 40
            $entry = $reader.ReadUInt32()
        } finally { $reader.Dispose() }
        [ordered]@{
            image = $name; entry_rva = '0x{0:x}' -f $entry
            sha256 = (Get-FileHash -LiteralPath $registered -Algorithm SHA256).Hash
        }
    } catch { return $null }
}


$observed = @(Get-Process -Name 'League of Legends','LeagueClient','LeagueClientUx','LeagueClientUxRender','RiotClientServices','RiotClientUx','RiotClientUxRender','Riot Client' -ErrorAction SilentlyContinue)
foreach ($process in $observed) { [void]$gamePids.Add($process.Id) }
[IO.File]::WriteAllLines($providerPath, @(
    "$($kernel.ToString('B')) 0x50 4",
    "$($com.ToString('B')) 0x2000000000000000 4",
    "$($loader.ToString('B')) 0xFFFFFFFFFFFFFFFF 5",
    "$($imeProviders[0].ToString('B')) 0xFFFFFFFFFFFFFFFF 5",
    "$($imeProviders[1].ToString('B')) 0xFFFFFFFFFFFFFFFF 5"
), [Text.Encoding]::ASCII)

$started = [DateTimeOffset]::UtcNow
$running = $false
$faultHandle = [UInt64]0
$faultLoss = @()
$collected = $false
try {
    try {
        Invoke-Logman @('start', $traceName, '-ets', '-o', $etl, '-f', 'bincirc', '-max', '64',
            '-bs', '64', '-nb', '16', '64', '-pf', $providerPath)
        $running = $true
        $faultHandle = [OxideImeMemoryTrace]::Start("$traceName-Memory", $faultEtl)
        Write-Host '追蹤已開始。請重現問題，完成後回到此視窗按 Enter。'
        Write-Host ("追蹤最多持續 {0} 秒。" -f $Seconds)
        $stopAt = [DateTimeOffset]::UtcNow.AddSeconds($Seconds)
        while ([DateTimeOffset]::UtcNow -lt $stopAt) {
            if (-not [Console]::IsInputRedirected -and [Console]::KeyAvailable -and
                [Console]::ReadKey($true).Key -eq [ConsoleKey]::Enter) { break }
            Start-Sleep -Milliseconds 200
        }
    } finally {
        try {
            if ($faultHandle) { $faultLoss = [OxideImeMemoryTrace]::Stop($faultHandle, "$traceName-Memory") }
        } finally {
            if ($running) { Invoke-Logman @('stop', $traceName, '-ets') }
        }
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
        } elseif ($event.ProviderId -eq $loader) {
            $fields = Event-Fields ([xml]$event.ToXml())
            $names = @(foreach ($field in 'FileName','ImportDllName','DLLName','ExportModule','ProcessImagePath') {
                if ($fields[$field]) { Image-Leaf ([string]$fields[$field]) }
            })
            if ($gamePids.Contains([int]$event.ProcessId) -or ($names -match "^$imeImage$")) {
                $events.Add([ordered]@{
                    utc = $event.TimeCreated.ToUniversalTime().ToString('o')
                    source = 'loader'; id = $event.Id; pid = $event.ProcessId; tid = $event.ThreadId
                    images = $names; failure_reason = $fields.FailureReason
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
    Find-ImeFaults
    $protectionLogs = Read-ProtectionEvents $started.UtcDateTime $finished.UtcDateTime
    $pageQuery = "*[System[Provider[@Guid='$($pageFaultProvider.ToString('B'))']]]"
    $firstFault = Get-WinEvent -Path $faultEtl -Oldest -MaxEvents 1 -FilterXPath $pageQuery -ErrorAction SilentlyContinue

    $writer = [IO.StreamWriter]::new($destination, $false, $utf8)
    try {
        $windows = Get-ItemProperty -LiteralPath 'HKLM:\SOFTWARE\Microsoft\Windows NT\CurrentVersion'
        $writer.WriteLine(([ordered]@{
            source = 'trace'; format = 3; started_utc = $started.ToString('o'); finished_utc = $finished.ToString('o')
            windows_build = "$($windows.CurrentBuild).$($windows.UBR)"
            process_ids = @($gamePids); provider_event_counts = $counts
            registered_ime = (Registered-ImeImage)
            protection_logs = $protectionLogs
            fault_events_lost = $faultLoss[0]; fault_buffers_lost = $faultLoss[1]
            first_fault_event_utc = $(if ($firstFault) { $firstFault.TimeCreated.ToUniversalTime().ToString('o') } else { $null })
        } | ConvertTo-Json -Depth 4 -Compress))
        foreach ($row in ($events | Sort-Object { $_['utc'] })) {
            if ($row.source -eq 'protection' -or $gamePids.Contains([int]$row.pid)) {
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
        [IO.File]::Delete($faultEtl)
        [IO.Directory]::Delete($scratch, $false)
    } else {
        Write-Warning "未能完成篩選。原始追蹤留在本機的 $scratch"
    }
}

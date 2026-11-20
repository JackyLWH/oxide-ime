param(
    [ValidateSet('Read', 'Enable', 'Disable', 'Clear')]
    [string]$Action = 'Read',
    [ValidateRange(1, 720)][int]$Hours = 24,
    [ValidateRange(1, 10000)][int]$Limit = 250
)
. (Join-Path (Split-Path $PSScriptRoot -Parent) 'src\maintenance\events.ps1')
switch ($Action) {
    Enable  { Invoke-OxideEventUtility @('sl', $diagnostic, '/e:true') }
    Disable { Invoke-OxideEventUtility @('sl', $diagnostic, '/e:false') }
    Clear {
        foreach ($channel in $operational, $diagnostic) {
            Invoke-OxideEventUtility @('cl', $channel)
        }
    }
    Read { Read-OxideEvents $Hours $Limit }
}

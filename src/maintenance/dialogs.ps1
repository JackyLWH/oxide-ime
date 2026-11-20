# 維護程式的公用操作。共用 WinForms 設定由 dialog-support.ps1 提供。
$exe = $env:OXIDE_IME_EXE
$root = $env:OXIDE_IME_ROOT
$policy = Join-Path $setting 'update-policy.txt'
$interval = Join-Path $setting 'update-interval.txt'
$programs = Join-Path $setting 'restart-programs.json'
$groups = [ordered]@{
    opencc = 'OpenCC 繁轉簡字詞表'
    s2t = 'OpenCC／同文堂 簡轉繁詞組表'
    jyutping = 'Rime Cantonese 粵拼及 CanCLID 讀音'
    ranking = '香港常用字及字頻'
    cangjie = 'Cangjie3-Plus 公用倉頡字庫'
}
$days = [ordered]@{
    '7' = '每 7 天'
    '1' = '每天'
    '14' = '每 14 天'
    '30' = '每 30 天'
    '0' = '只手動檢查'
}

function Selected-Updates {
    if ([IO.File]::Exists($policy)) {
        [IO.File]::ReadAllLines($policy) | ForEach-Object { $_.Trim() } | Where-Object { $_ -cin $groups.Keys }
    } elseif ($env:OXIDE_IME_INSTALLED -ne '1') {
        'opencc', 's2t', 'jyutping', 'ranking'
    } else { $groups.Keys }
}

function Show-UpdateSettings {
    Require-User
    $form = New-Dialog 'Oxide IME 定期更新設定' 400 222
    try {
        $selected = @(Selected-Updates)
        $choices = [Windows.Forms.CheckedListBox]::new()
        $choices.CheckOnClick = $true
        $choices.SetBounds(16, 16, 368, 112)
        foreach ($key in $groups.Keys) { [void]$choices.Items.Add($groups[$key], $key -cin $selected) }
        $period = [Windows.Forms.ComboBox]::new()
        $period.DropDownStyle = 'DropDownList'
        $period.SetBounds(16, 137, 368, 28)
        $values = @($days.Keys)
        $period.Items.AddRange(@($days.Values))
        $stored = if ([IO.File]::Exists($interval)) { [IO.File]::ReadAllText($interval).Trim() }
        $period.SelectedIndex = [Math]::Max(0, [array]::IndexOf($values, $stored))
        $save = New-Button '儲存' 210 178
        $save.DialogResult = 'OK'
        $cancel = New-Button '取消' 302 178
        $cancel.DialogResult = 'Cancel'
        $form.Controls.AddRange(@($choices, $period, $save, $cancel))
        $form.AcceptButton = $save
        $form.CancelButton = $cancel
        if ($form.ShowDialog() -eq 'OK') {
            $keys = @($groups.Keys)
            $chosen = @($choices.CheckedIndices | ForEach-Object { $keys[$_] })
            Save-Setting $interval ($values[$period.SelectedIndex] + [char]10)
            Save-Setting $policy ($chosen -join [char]10)
        }
    } finally { $form.Dispose() }
}

function Show-RestartSettings {
    Require-User
    if ($env:OXIDE_IME_INSTALLED -ne '1') { throw '指定程式重啟功能由正式版的登入維護程序提供。' }
    $form = New-Dialog 'Oxide IME 指定程式重啟設定' 640 340
    try {
        $list = [Windows.Forms.ListView]::new()
        $list.View = 'Details'
        $list.FullRowSelect = $true
        $list.MultiSelect = $false
        $list.HideSelection = $false
        $list.SetBounds(16, 16, 608, 217)
        [void]$list.Columns.Add('執行檔路徑或名稱', 470)
        [void]$list.Columns.Add('前景等待（秒）', 125)
        $rulesJson = if ([IO.File]::Exists($programs)) {
            Get-Content -LiteralPath $programs -Raw -Encoding UTF8
        } else {
            $env:OXIDE_IME_RESTART_DEFAULT
        }
        foreach ($rule in ($rulesJson | ConvertFrom-Json)) {
            if ($rule.path -and $null -ne $rule.delay_ms) {
                [void]$list.Items.Add([Windows.Forms.ListViewItem]::new([string[]]@($rule.path, ([decimal]$rule.delay_ms / 1000))))
            }
        }
        $add = New-Button '新增程式…' 16 246 111
        $remove = New-Button '移除' 136 246
        $label = [Windows.Forms.Label]::new()
        $label.Text = '前景等待（秒）'
        $label.SetBounds(242, 250, 120, 24)
        $delay = [Windows.Forms.NumericUpDown]::new()
        $delay.SetBounds(367, 246, 91, 30)
        $delay.DecimalPlaces = 2
        $delay.Increment = [decimal]0.25
        $delay.Maximum = 600
        $delay.Value = [decimal]1.5
        $delay.Enabled = $false
        $save = New-Button '儲存' 448 296
        $save.DialogResult = 'OK'
        $cancel = New-Button '取消' 542 296
        $cancel.DialogResult = 'Cancel'
        $list.Add_SelectedIndexChanged({
            $delay.Enabled = $list.SelectedItems.Count -gt 0
            if ($delay.Enabled) {
                $delay.Value = [decimal]::Parse($list.SelectedItems[0].SubItems[1].Text, [Globalization.CultureInfo]::InvariantCulture)
            }
        })
        $delay.Add_ValueChanged({
            if ($list.SelectedItems.Count) {
                $list.SelectedItems[0].SubItems[1].Text = $delay.Value.ToString([Globalization.CultureInfo]::InvariantCulture)
            }
        })
        $add.Add_Click({
            $picker = [Windows.Forms.OpenFileDialog]::new()
            try {
                $picker.Title = '選擇開啟後要重啟 Oxide IME 的程式'
                $picker.Filter = 'Windows 執行檔 (*.exe)|*.exe'
                $picker.CheckFileExists = $true
                if ($picker.ShowDialog($form) -ne 'OK') { return }
                $path = [IO.Path]::GetFullPath($picker.FileName)
                $item = $list.Items | Where-Object { $_.Text -ieq $path } | Select-Object -First 1
                if (-not $item -and $list.Items.Count -lt 128) {
                    $item = $list.Items.Add([Windows.Forms.ListViewItem]::new([string[]]@($path, '1.5')))
                }
                if ($item) { $item.Selected = $true; $item.EnsureVisible() }
            } finally { $picker.Dispose() }
        })
        $remove.Add_Click({
            if ($list.SelectedItems.Count) { $list.Items.Remove($list.SelectedItems[0]) }
        })
        $form.Controls.AddRange(@($list, $add, $remove, $label, $delay, $save, $cancel))
        $form.AcceptButton = $save
        $form.CancelButton = $cancel
        $form.Add_Shown({
            if ($list.Items.Count) { $list.Items[0].Selected = $true }
        })
        if ($form.ShowDialog() -eq 'OK') {
            if ($list.SelectedItems.Count) {
                $list.SelectedItems[0].SubItems[1].Text = $delay.Value.ToString([Globalization.CultureInfo]::InvariantCulture)
            }
            $rules = @(foreach ($item in $list.Items) {
                [ordered]@{
                    path = $item.Text
                    delay_ms = [int]([decimal]::Parse($item.SubItems[1].Text, [Globalization.CultureInfo]::InvariantCulture) * 1000)
                }
            })
            Save-Setting $programs (ConvertTo-Json -InputObject $rules -Depth 4)
            Start-Process -FilePath $exe -ArgumentList '--session' -WindowStyle Hidden
        }
    } finally { $form.Dispose() }
}

switch ($env:OXIDE_IME_ACTION) {
    ChooseData {
        Require-User
        $picker = [Windows.Forms.FolderBrowserDialog]::new()
        $owner = New-DialogOwner
        try {
            $picker.Description = '選擇直接存放 Oxide IME 詞庫和設定的資料夾'
            $picker.SelectedPath = $data
            if ($picker.ShowDialog($owner) -eq 'OK') {
                $path = [IO.Path]::GetFullPath($picker.SelectedPath)
                if ($path -ieq $root -or $path.StartsWith("$root\", [StringComparison]::OrdinalIgnoreCase)) {
                    throw '請將使用者資料放在程式安裝目錄以外。'
                }
                [Console]::WriteLine($path)
            }
        } finally { $owner.Dispose(); $picker.Dispose() }
    }
    OpenData {
        if (-not [IO.Directory]::Exists($data)) {
            Require-User
            [void][IO.Directory]::CreateDirectory($data)
        }
        Invoke-Item -LiteralPath $data
    }
    OpenProgram { Invoke-Item -LiteralPath $root }
    UpdateSettings { Show-UpdateSettings }
    RestartSettings { Show-RestartSettings }
    CheckUpdates {
        Require-User
        if (-not @(Selected-Updates).Count) {
            Show-Result '請先在定期更新設定選擇要檢查的公用資料。'
        } else {
            $report = Join-Path $setting 'last-update.txt'
            $start = [datetime]::UtcNow
            $run = Start-Process -FilePath $exe -ArgumentList '--update-public' -WindowStyle Hidden -Wait -PassThru
            if ([IO.File]::Exists($report) -and [IO.File]::GetLastWriteTimeUtc($report) -ge $start) {
                $result = ([IO.File]::ReadAllLines($report) | Select-Object -Skip 1) -join [Environment]::NewLine
                Show-Result $result -ErrorIcon:($run.ExitCode -ne 0)
            } else { throw '未能完成檢查。請稍後重試。' }
        }
    }
    ExportLogs {
        Require-User
        $folder = Join-Path $data 'logs'
        [void][IO.Directory]::CreateDirectory($folder)
        $picker = [Windows.Forms.SaveFileDialog]::new()
        $owner = New-DialogOwner
        try {
            $picker.Title = '匯出 Oxide IME 日誌'
            $picker.Filter = 'JSON Lines 日誌 (*.jsonl)|*.jsonl'
            $picker.InitialDirectory = $folder
            $picker.FileName = 'OxideIme-' + (Get-Date -Format yyyyMMdd-HHmmss) + '.jsonl'
            if ($picker.ShowDialog($owner) -eq 'OK') {
                $lines = @(Read-OxideEvents 168 10000)
                [IO.File]::WriteAllLines($picker.FileName, [string[]]$lines, [Text.UTF8Encoding]::new($false))
                Show-Result "已匯出 $($lines.Count) 筆事件。"
            }
        } finally { $owner.Dispose(); $picker.Dispose() }
    }
    Diagnostics {
        $enable = -not (Get-WinEvent -ListLog $diagnostic).IsEnabled
        $value = if ($enable) { '/e:true' } else { '/e:false' }
        Invoke-OxideEventUtility @('sl', $diagnostic, $value) -Elevate:(-not $elevated)
        if ($enable) { Show-Result '已開啟詳細日誌。' } else { Show-Result '已關閉詳細日誌。' }
    }
    ClearLogs {
        if (Show-Result '確定清空事件檢視器的 Oxide IME 操作及詳細日誌？清空後無法復原。' -Confirm) {
            foreach ($channel in $operational, $diagnostic) {
                Invoke-OxideEventUtility @('cl', $channel) -Elevate:(-not $elevated)
            }
            Show-Result '已清空 Oxide IME 的操作及詳細日誌。'
        }
    }
    About {
        Show-Result (@("Oxide IME $env:OXIDE_IME_LOADED ($env:OXIDE_IME_ARCH)",
            'Windows TSF 倉頡三代、速成、筆劃及粵拼輸入法', "資料目錄　$data") -join [Environment]::NewLine)
    }
}

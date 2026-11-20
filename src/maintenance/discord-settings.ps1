# 私用版在目前資料目錄設定簡轉繁通知。
Require-User
$path = Join-Path $setting 'discord-webhook.txt'
$form = New-Dialog 'Oxide IME Discord 通知' 460 130
try {
    $label = [Windows.Forms.Label]::new()
    $label.Text = '貼上完整 Discord Webhook 網址。留空並儲存會停用通知。'
    $label.SetBounds(16, 15, 428, 25)
    $url = [Windows.Forms.TextBox]::new()
    $url.SetBounds(16, 45, 428, 25)
    $url.UseSystemPasswordChar = $true
    $url.MaxLength = 512
    $stored = [IO.FileInfo]::new($path)
    if ($stored.Exists -and $stored.Length -le 512) {
        $url.Text = [IO.File]::ReadAllText($path).Trim()
    }
    $save = New-Button '儲存' 270 83
    $cancel = New-Button '取消' 362 83
    $cancel.DialogResult = 'Cancel'
    $save.Add_Click({
        $value = $url.Text.Trim()
        if ($value -and $value -cnotmatch '\Ahttps://discord\.com/api/webhooks/[0-9]{1,20}/[A-Za-z0-9_-]{1,200}\z') {
            [void][Windows.Forms.MessageBox]::Show($form, '請輸入完整的 Discord Webhook 網址。', 'Oxide IME', 'OK', 'Error')
            return
        }
        try {
            Save-Setting $path $value
            $form.DialogResult = 'OK'
        } catch {
            [void][Windows.Forms.MessageBox]::Show($form, '無法儲存 Discord 設定。請檢查資料目錄的寫入權限。', 'Oxide IME', 'OK', 'Error')
        }
    })
    $form.Controls.AddRange(@($label, $url, $save, $cancel))
    $form.AcceptButton = $save
    $form.CancelButton = $cancel
    $form.ActiveControl = $url
    $url.SelectAll()
    [void]$form.ShowDialog()
} finally { $form.Dispose() }

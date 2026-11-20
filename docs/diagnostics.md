# Oxide IME 事件記錄及問題排查

Windows 安裝程式及開發註冊腳本會登錄兩個 ETW 事件記錄通道。每個通道可循環保留最多 4 MiB。輸入法透過 Windows API 發送本機事件，適用於不同程序內載入的 x64／x86 DLL、字庫服務與排程更新程序。可在事件檢視器的「應用程式及服務記錄」開啟，也可用下方指令讀取。

| 通道 | 預設 | 記錄範圍 |
| --- | --- | --- |
| `OxideIme/Operational` | 開啟 | COM 登錄、TSF 啟用、共用輸入狀態變更、字庫載入與異常、服務啟動、資料更新及主要錯誤。 |
| `OxideIme/Diagnostic` | 關閉 | 編輯鎖、提交次序、UTF-16 寫入及範圍讀回、候選介面、輸入焦點、字庫服務請求及 Windows 錯誤碼。 |

stage49 起使用以上通道。已儲存的舊版事件仍可在事件檢視器的原通道查看。

每筆事件含 UTC 時間、版本、宿主程式檔名、程序及執行緒識別碼、模組、操作及數值欄位。詳細記錄會統計文字長度、換行數及行尾句號數。讀回比對只限本次寫入的範圍，最多 32,768 個 UTF-16 碼元。記錄不含按鍵或字碼內容、候選文字、私人字典文字、剪貼簿內容、文件標題、網址或完整路徑。事件留在本機，由 Windows 管理查看權限。

## 讀取及重現

在開發專案根目錄開啟 PowerShell。每行輸出一筆 JSON，較新的事件在前。開發測試的記錄可寫入 `work/diagnostics/`。正式安裝後可從工作列的 **中／英 → 匯出日誌…** 取得相同的記錄。

```powershell
.\scripts\diagnostics.ps1 -Hours 2 -Limit 300
.\scripts\diagnostics.ps1 -Hours 1 -Limit 1000 | ConvertFrom-Json |
    Where-Object { $_.component -eq 'edit' } | Format-List
```

如需詳細資訊，在系統管理員 PowerShell 依次執行。

1. 執行 `.\scripts\diagnostics.ps1 -Action Enable`。
2. 在出問題的程式只重現一次，記下大約時間及所用程式。
3. 執行 `.\scripts\diagnostics.ps1 -Action Disable` 停止收集，然後用讀取指令查看已儲存的詳細事件。

工作列的 **中／英 → 清空 Oxide IME 日誌…** 可清空兩個通道。請先匯出需要保留的事件。也可在系統管理員 PowerShell 執行 `.\scripts\diagnostics.ps1 -Action Clear`。

相同 PID、TID 及 `edit_id` 可對照 `set_text`／`insert_at_selection`、`readback` 與編輯工作階段結果。`request_hr` 是請求編輯工作階段的 HRESULT，`session_hr` 是工作階段工作結果。兩者均為無號數值，`0` 代表成功。`readback.exact=1` 代表 TSF 寫入範圍當時與輸入法的 UTF-16 完全一致，宿主讀取並更新自己的文件模型可能在工作階段釋放後發生。

字庫服務的 `phase` 依次為參數、連線、送出請求、讀取回應標頭及讀取回應內容（0 至 4）。字庫 `invalid_table.table` 為主碼表（0）或附加詞庫（1）。表格行號從 1 開始，標頭結束標記缺失時以 0 表示。可用事件時間對照使用者資料目錄的 `setting/last-update.txt`，查看公開來源、修訂及下載細節。

若工作列的「中／英」與實際輸入不同，可對照 `tsf/mode_change`、`language_bar/mode_notify` 和 `language_bar/mode_refresh`。stage51 將中英變更透過 TSF 全域通知交予其他執行緒。各指示器以字庫服務的共用狀態更新工作列。詳見 [Microsoft TSF Compartments](https://learn.microsoft.com/en-us/windows/win32/tsf/compartments)。

本機一般 AppContainer 可以寫入事件。更受限的 LPAC 測試宿主在註冊 ETW 時收到 Windows 錯誤 5。這類宿主可用詳細通道中的字庫服務事件及 `client_pid` 追查查詢。

## 組字時出現 0x8000FFFF

2026 年 11 月 19 日 09:59:16（台灣時間）Chrome 的 `edit/preedit` 同時記下 `request_hr=0x8000FFFF`（`E_UNEXPECTED`）及 `session_hr=0x00040300`（`TF_S_ASYNC`）。同步組字請求在取得文字編輯權限前失敗。記錄可定位到 TSF 請求，宿主內部的觸發條件仍待確認。

stage50 對這類無法同步取得編輯鎖的組字要求排程一次非同步重試。原要求即使延遲回呼也會忽略，切走後的舊回呼亦會作廢。以同一 `edit_id` 對照 `preedit` 的 `callback_ran`、`async` 與 `preedit_retry_complete`，可確認重試有否寫入組字。候選視窗隨宿主視窗被銷毀時，下次顯示會重建。詳見 [Microsoft TSF 編輯工作階段](https://learn.microsoft.com/en-us/windows/win32/api/msctf/nf-msctf-itfcontext-requesteditsession)。

## 字庫服務重啟失敗

stage52 曾在手動重啟時留下舊服務。新程序建立命名管道收到 Windows 錯誤 5，舊程序其後仍收到查詢。可按程序識別碼對照 `broker_pipe/win32_error`、`broker/start_failed` 和 `broker/request`。 [Microsoft CreateNamedPipeW](https://learn.microsoft.com/en-us/windows/win32/api/namedpipeapi/nf-namedpipeapi-createnamedpipew)

自 stage53 起，新程序通知同一帳戶和登入工作階段的舊服務停止。可查閱舊程序的 `broker/stop_requested`。重啟指令會核對命名管道的回應確實來自新程序，然後再停用及重新啟用 TSF。

## LoL 顯示及重載

在 LoL 已實測用 **中／英 → 重新啟動輸入法** 恢復輸入。新版先備妥字庫服務，再立即停用、嘗試釋放和重新啟用 TSF。Windows 在停用一刻可能短暫顯示另一個輸入法。

正式版可在 **中／英 → 指定程式重啟設定…** 新增或移除規則，也可調整每個程式的前景等待時間。未儲存過規則時，預設為 `League of Legends.exe` 在前景持續 1.5 秒。登入後的維護程序會重新讀取設定。符合規則的新遊戲程序在前景達到指定時間，且正在使用 Oxide IME 時，便會重啟一次。`Alt+Tab` 回到同一對戰程序時會沿用原狀態。可在事件檢視器核對 `profile/restart_deactivate`、`profile/restart_release` 和 `profile/restart_activate`。

此前追蹤依序錄得對戰程序載入已登錄的 DLL、DLL 的 PE 入口指令發生存取違規，以及 DLL 隨即卸載。當時 TSF 尚未啟用輸入服務。stage52、stage53 的監察以 Windows ETW 比對這組事件。 [ETW 載入及卸載事件](https://learn.microsoft.com/en-us/windows/win32/etw/image-load) · [ETW 存取違規事件](https://learn.microsoft.com/en-us/windows/win32/etw/pagefault-typegroup1)

要調查首次載入，請從系統管理員 PowerShell 執行進階追蹤腳本，再進入練習模式開啟聊天。可加 `-Seconds 120` 延長追蹤，或以 `-OutputDirectory` 指定儲存位置。出現問題後可回到追蹤視窗按 Enter 結束。腳本會查閱同一段時間的 Code Integrity、Security Mitigations 及 WER 記錄，輸出匹配的檔名、事件編號及數值欄位。`protection_logs` 表示各通道能否讀取及匹配數，`source=protection` 是匹配事件。零筆代表在該段時間的可讀通道中未找到相符事件。 [LoL 進階追蹤工具](../scripts/diagnostics/OxideIme-LoL-LoaderTrace.ps1) · [Windows 防護事件](https://learn.microsoft.com/en-us/defender-endpoint/exploit-protection#review-exploit-protection-events-in-windows-event-viewer)

### 首次載入的入口頁面

Stage49 與 stage52 的實體機追蹤均在各自的 PE 入口出現存取違規。可用 `OxideIme-LoL-EntryProbe-v3.exe` 在同一段追蹤取樣已登錄 x64 DLL 入口頁面的狀態。工具只執行 Windows 事件追蹤及頁面屬性查詢。Windows 拒絕查詢時會記錄錯誤碼。

1. 將工具放在有寫入權限的資料夾，從該處開啟系統管理員 PowerShell。選取 Oxide IME 後執行下列命令。

   ```powershell
   .\OxideIme-LoL-EntryProbe-v3.exe 120 | Tee-Object -FilePath .\OxideIme-LoL-Entry-v3.jsonl
   ```

2. 看到 `"kind":"ready"` 後進入練習模式，首次開啟聊天並嘗試打字。等候 `"kind":"done"` 後，保留 `OxideIme-LoL-Entry-v3.jsonl`。

工具即時輸出載入、故障及卸載事件，並定期寫出已取得的頁面樣本。v3 在首次載入之前較頻繁要求 ETW 送達事件。用 `observed_filetime` 和 `load` 的 `filetime` 可量度延遲。`progress` 表示外層計時仍在進行。時間到後，通常數秒內會出現 `done`。若取樣程序卡住，外層程序會停止該次取樣並輸出 `forced_stop: true`。這種情況仍可分析此前已輸出的記錄。

以 `attempt` 對照載入、頁面取樣、故障及卸載的 `filetime`。`page` 每列收合連續相同的取樣結果。取樣時間應在載入和卸載之間，並核對 `same_allocation` 是否為 `true`，以及 `type` 是否為 `16777216`（`MEM_IMAGE`）。`protect` 的 `32` 代表可執行、可讀，`4` 代表可讀寫而不可執行，`2` 代表唯讀。取樣間隔設為 15 毫秒，實際最大間隔記在 `max_gap_100ns`。只在故障兩側都取得足夠接近的同一映射樣本時，才以它們判斷當時可能的頁面狀態。 [Windows 頁面屬性查詢](https://learn.microsoft.com/en-us/windows/win32/api/memoryapi/nf-memoryapi-virtualqueryex) · [Windows 記憶體保護常數](https://learn.microsoft.com/en-us/windows/win32/memory/memory-protection-constants)

可從專案根目錄執行 `.\scripts\diagnostics\build-entry-probe.ps1` 重建工具。需要現有的 x64 MSVC 建置工具和 Windows SDK。

LoL 追蹤顯示，遊戲要求自行繪製輸入法介面。stage50 把中英、繁簡、粵拼／倉頡等狀態文字經標準閱讀提示介面交予宿主。遊戲的顯示效果由宿主決定。詳見 [Microsoft TSF UI-less 模式](https://learn.microsoft.com/en-us/windows/win32/tsf/uiless-mode-overview)。

#### 實體機 v3 追蹤結果

2026 年 11 月 19 日 18:24:29（台灣時間）的追蹤正常完成，沒有 ETW 事件或緩衝遺失。登錄 DLL 的入口 RVA 是 `0x1BFF20`，與 stage53 成品相符。

| 首次載入後 | 觀察結果 |
| --- | --- |
| 230.39 毫秒 | 最後一次取樣為 `PAGE_EXECUTE_READ`。 |
| 245.81 毫秒 | 同一個映射頁面首次取樣為 `PAGE_READWRITE`。 |
| 247.81 毫秒 | 在 PE 入口發生存取違規。ETW 的指令位址與違規位址均符合入口。 |
| 338.86 毫秒 | 卸載前最後一次取樣仍為 `PAGE_READWRITE`。 |
| 346.96 毫秒 | DLL 卸載。 |

故障前後的頁面均已提交且屬於原有 DLL 映射。這組時間和位址符合 Windows 嘗試執行不可執行頁面時的存取違規。同一遊戲程序約 17.46 秒後重載，隨後約 39.32 秒內的 2,290 筆頁面樣本均為可執行、可讀。 [ETW 存取違規事件](https://learn.microsoft.com/en-us/windows/win32/etw/pagefault-typegroup1) · [Windows 執行存取違規](https://learn.microsoft.com/en-us/shows/inside/access-violation-c0000005-execute)

stage53 檔案的入口位於可執行、可讀的 `.text`，入口所在的 4 KiB 頁面沒有 PE 基底重定位目標。以本機獨立 Windows 測試程序載入成品三次均成功，映射中的入口取樣均為可執行、可讀。此前 stage49 與 stage52 的追蹤在 Windows Code Integrity、Security Mitigations 及 WER 通道沒有相關事件。權限變更發起者及有否涉及簽署規則仍待查證。 [Microsoft PE 格式](https://learn.microsoft.com/en-us/windows/win32/debug/pe-format)

同一場的 `_r3dlog.txt` 於 18:24:28.671 開始記錄。首次入口違規約在開啟日誌後 0.783 秒，早於渲染器初始化記錄（1.031 秒）及聊天工作階段載入記錄（2.492 秒）。三份對戰日誌沒有記錄 DLL 頁面權限變更；`_netlog.txt` 及 `_netstats.csv` 記載的是對戰網絡與統計資料。可按 18:24:29（UTC+8）對照同一場的 Vanguard 記錄。 [Riot Windows 日誌位置](https://support.riotgames.com/en-us/riot/performance/network-system-and-logs-windows#vanguard-logs)

收到的 `vgc_2026-11-19_18-21-58.log` 最後修改於 18:43:12。故障時間介乎檔名所示時間與最後修改時間。檔案為二進位格式，未能核對個別事件。可向 Riot 支援提供原始 `vgc` 日誌及 v3 追蹤，請對照 18:24:29.454（UTC+8）的 DLL 入口違規。 [Riot 日誌收集及提交指引](https://support.riotgames.com/en-us/riot/performance/gathering-logs-riot-repair-tool-rrt)

要回查本次故障，可在實測電腦以系統管理員 PowerShell 執行下列指令。從放置新版 `OxideIme-LoL-LoaderTrace.ps1` 的資料夾執行即可。

```powershell
.\OxideIme-LoL-LoaderTrace.ps1 -At '2026-11-19T18:24:29.454+08:00' -GameProcessId 5352
```

`-At` 讀取指定時間前後各 30 秒已寫入的 Windows 事件，產生 `OxideIme-LoL-Windows-*.jsonl`。回查涵蓋 Code Integrity、Security Mitigations、WER、應用程式錯誤及電腦原有的 Sysmon。Sysmon 事件 10 記錄哪個程序開啟了遊戲程序，可提供後續檢查的線索。各通道的 `read` 與 `matches` 分別表示能否讀取及有幾筆相關事件。歷史記錄可能已輪替，零筆只能用於描述查詢結果。 [Windows 防護事件](https://learn.microsoft.com/en-us/defender-endpoint/exploit-protection#review-exploit-protection-events-in-windows-event-viewer) · [Microsoft Sysmon 事件](https://learn.microsoft.com/en-us/sysinternals/downloads/sysmon)

## 多行候選的句號與游標

使用回報的 873 字、7 行候選測試。stage34 的原生 Chromium 多行富文字欄逐字與原文一致。在同版本 Chromium 的 ProseMirror 測試欄，`compositionend` 仍回報原有的五個換行前句號，編輯器的文件模型則把它們移到最後一行。模型結果與回報的錯位完全一致。

相容性修正在第一個 TSF 編輯工作階段清除並結束字根組字，再以 `InsertTextAtSelection` 整段插入原本的選字內容，並指定 `TF_IAS_NO_DEFAULT_COMPOSITION`。這讓 Chromium 以普通文字插入傳遞多行文字。每次選字只提交一次完整內容。補充平面字元、原有換行及標點皆沿用原字串。測試亦覆蓋宿主拒絕寫入時還原原先選取的文字。

使用 Chromium 157 重現另一個狀況。候選全文和後方原文均正確，ProseMirror 的文件模型和 DOM 游標卻停在預期位置之前 6 格。沒有使用段落節點的富文字欄和一般多行文字欄位置正確。插入前保留開頭及 15 字後文，插入和替換選取範圍均可重現。

六格與候選的六個換行吻合。Chromium 用提交字串的 UTF-16 長度計算插入點，富文字段落在該位置映射可能佔用額外的分隔位置。將 LF 換成 CRLF 的對照測試會讓 ProseMirror 的游標回到末端，但同時令普通文字欄多走六格。 [Chromium 插入點計算](https://chromium.googlesource.com/chromium/src/+/cad6126a9d04a7cab94a776d74721933f36cf01e/third_party/blink/renderer/core/editing/ime/input_method_controller.cc#897)

v1.0 保留整段 TSF 寫入和宿主接受的 TSF 插入點核對。Chromium 忽略部分輸入法在組字完結後的純選取變更，因此另由 Maintenance 用 Windows UI Automation 比對游標附近的候選末段和原有後文。仍屬同一個焦點且位置吻合時才移動插入點。詳細日誌的 `edit/chromium_caret_checked` 只記錄數值。`result` 由 0 至 4 依次表示無法建立介面、未找到焦點、未找到文字介面、未找到單一插入點、文字位置未吻合。5 表示焦點或選取已改動，6 表示宿主拒絕，7 表示宿主接受選取更新。 [Chromium TSF 文字儲存](https://chromium.googlesource.com/chromium/src/+/HEAD/ui/base/ime/win/tsf_text_store.cc) [Windows UI Automation 文字模型](https://learn.microsoft.com/en-us/windows/win32/winauto/uiauto-understandingtheuiautomationtextobjectmodel)

重用 stage38 的 Chromium 測試頁，以同一個 `zs` 詞條比對文件模型及 DOM。v1.0 的 Maintenance 將 ProseMirror 的插入點由 872 移到預期的 878，直接插入和替換選取範圍都通過。普通文字框及原生富文字欄的位置維持正確。另以 x64 和 x86 Windows TSF 接駁隔離 XAML 宿主，驗證原文、句號、換行、後文及插入點。

介面參考可查閱 [Microsoft TSF 的插入旗標](https://learn.microsoft.com/en-us/windows/win32/tsf/tf-ias--constants)、[Chromium 的 TSF 文字儲存實作](https://chromium.googlesource.com/chromium/src/+/HEAD/ui/base/ime/win/tsf_text_store.cc) 及 [ProseMirror 的 DOM 變更處理](https://github.com/ProseMirror/prosemirror-view/blob/master/src/domchange.ts)。

## 剪貼簿長文

stage40 在 `z` 產生剪貼簿候選時便讀取並轉換文字，續輸入 `v` 時重用快照。詳細通道的 `command/clipboard` 分別記下 `read_ms` 和 `conversion_ms`，兩者合計達 250 毫秒時也會寫入一般通道。

Notepad++ 8.9.9.1 的 Scintilla 在 TSF 編輯工作階段釋放後，逐字處理 Windows 送出的 IME 結果。以 26,955 個 Unicode 字元、70 個換行的合成文字測試，stage39 在啟用自動換行時花了約 152 秒處理該結果。[Notepad++ 的 Scintilla 實作](https://github.com/notepad-plus-plus/notepad-plus-plus/blob/v8.9.9.1/scintilla/win32/ScintillaWin.cxx#L1522-L1534)

長文批量寫入適用於頂層視窗類別為 `Notepad++`、當前執行緒焦點為 `Scintilla` 的編輯區，且須符合 UTF-8、視窗式 IME、單一一般選取範圍等條件。至少 1,024 個 UTF-8 位元組的剪貼簿及自訂字庫候選會先結束 TSF 組字，再由 `SCI_REPLACESEL` 寫入或取代選取範圍。其他宿主及編輯情況使用 TSF。詳細通道的 `edit/scintilla_replace` 記下批量寫入耗時、長度及長度檢查結果。批量寫入或 TSF 編輯工作階段耗時達 250 毫秒時，也可在一般通道查閱 `elapsed_ms`。[Scintilla 編輯介面](https://scintilla.org/ScintillaDoc.html#SCI_REPLACESEL)

同一份合成文字覆蓋原文插入及轉換後取代選取文字。實測批量寫入約 10 至 25 毫秒，全文逐位元組相符，兩種操作均可用一次 `Ctrl+Z` 還原。合成剪貼簿的測試亦確認 `z` 首次轉換約 20 至 31 毫秒，`v` 沿用同份快照。

stage41 移除先前在文件末尾插入時額外指定捲動位置的指令。選字後游標位於插入文字的結尾，畫面隨 Scintilla 本身的捲動規則調整。同一份 71 行合成文字在開啟自動換行時完整寫入，畫面停留在文字中段。[Notepad++ 的原生貼上命令](https://github.com/notepad-plus-plus/notepad-plus-plus/blob/v8.9.9.1/scintilla/src/Editor.cxx#L6361-L6367) · [批量替換命令](https://github.com/notepad-plus-plus/notepad-plus-plus/blob/v8.9.9.1/scintilla/src/Editor.cxx#L6482-L6494)

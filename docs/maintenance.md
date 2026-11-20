# 維護 Oxide IME

| 位置 | 內容 |
| --- | --- |
| `src/` | 倉頡字庫、轉換和粵拼引擎，Windows TSF、候選窗、字庫服務及維護程式。 |
| `src/engine/` | 候選查詢、F4 選項與輸入引擎測試。 |
| `src/tsf/` | DLL 的 COM 匯出、Windows 輸入服務及原生介面。 |
| `src/maintenance/` | 登入及重啟協調、使用者資料初始化、事件通道和維護介面。Windows PowerShell 原始碼在建置時內嵌入維護程式。 |
| `scripts/` | 開發建置、註冊及診斷腳本。`setup.iss` 管理安裝、升級及解除安裝。 |
| `assets/` | 編譯用圖示、COM 匯出定義、事件資訊清單和初始字庫範本。重建來源位於 `assets/templates/build/`。 |
| `data/` | 開發用資料，初始結構等同 `%LOCALAPPDATA%\OxideIME\data\`。 |
| `tests/` | Windows、AppContainer 及 RIME 的隔離測試和樣本。 |
| `licenses/` | 安裝程式原樣加入 `C:\Program Files\OxideIME\licenses\` 的授權與來源說明。 |
| `outputs/` | 正式發佈的 DLL、字庫服務、開發用維護程式及安裝包。 |
| `work/` | 編譯快取、上游快照、回歸樣本及驗證原始輸出。診斷原始檔集中在 `work/diagnostics/`。 |

## 建置

版本由 `release-version.txt` 指定。需要 x64／x86 Rust MSVC 目標、MSVC 和含有 `rc.exe` 和 `mc.exe` 的 Windows SDK。安裝程式使用 Inno Setup 7.1 或以上，可用 `scoop install extras/inno-setup` 安裝。需要離線使用已取得的 Cargo 相依套件時加 `-Offline`。

~~~powershell
pwsh -NoProfile -File .\scripts\build-release.ps1 -Offline
pwsh -NoProfile -File .\scripts\build-installer.ps1 -Offline
~~~

第二個命令將四個發行元件、授權及 `assets/templates/build/` 的四個重建來源檔封裝成 `OxideIme-Setup-v<版本>.exe`。安裝期間會將重建來源解壓至暫存目錄，初次初始化或從舊版升級時補到登入者的 `data/dictionaries/build/`。其他 Windows 帳戶可各自執行安裝程式取得這些來源。Maintenance 更新字庫時，讀取使用者目錄的副本並比對 `.initialized`。其餘公用字庫初始範本仍由 Maintenance 內嵌。

公開安裝包使用預設建置。自用版可另存帶有 Discord 通知的安裝包。

~~~powershell
pwsh -NoProfile -File .\scripts\build-installer.ps1 -Offline -PrivateDiscord
~~~

自用安裝包檔名以 `-Private.exe` 結尾，使用獨立編譯快取。通知中的主機名稱預設取自 Windows 電腦名稱，自用建置前可設定 `OXIDE_IME_DISCORD_HOST` 環境變數更改顯示名稱。在自用版的 **中／英 → 輸入 Discord Webhook 網址…** 貼上完整網址。設定以 UTF-8 純文字儲存在資料目錄的 `setting/discord-webhook.txt`，留空並儲存會停用通知。通知僅包含新識別到的公開詞組和選詞結果。

## 重建字庫範本

- 倉頡沿用 Maintenance 的 `--stage-cangjie <work/快照目錄>`，從 Cangjie3-Plus 的同一 Git 提交取得字庫及 MIT 授權。將核對過的字典放入 `assets/templates/cangjie3.dict.yaml`。
- 粵拼用 Maintenance 的 `--rebuild-jyutping <char.csv> <jyut6ping3.words.dict.yaml> <新輸出檔案>`。用同一批來源執行 `Documents/tools` 中的 CSV 轉換及補音工具，核對兩份輸出，再替換範本。使用者更新會下載 CanCLID 原始字音與 Rime Cantonese 詞庫，再執行同一套重建流程。
- 同文堂的詞組表使用 `Documents/tongwentang/tools/dictionary.py build --output <work/新輸出檔案>`，從 OpenCC 詞組來源重建。可用該工具的 `check --dictionary <範本路徑>` 驗證。手改過的單字表與自訂 JSON 使用同文堂原檔。定期更新會以 OpenCC 官方 `STPhrases.txt` 和 `data/dictionaries/build/s2t-rules/` 的同文堂規則重建詞組。同文堂的指定選詞優先。其餘詞組如有兩個候選，採用 OpenCC 第一個候選，再套用同文堂的字形規則。三個或更多候選仍需在 `STPhrasesMultiOverrideHK.json` 指定選詞。這組更新僅發佈詞組 JSON。
- 繁轉簡兩份 OpenCC TXT 及常用字排名可各按原有公用資料更新管道另行更新。常用字更新讀取 `data/dictionaries/build/cuhk-rank.txt`。發佈前確認範本是公用版本，並同步核對 `licenses/`。
- 正式安裝在使用者資料目錄的 `.initialized` 保存 SHA-256。更新只替換符合基準的檔案。安裝程式以 `--initialize <更新選項> <暫存範本目錄>` 交給 Maintenance 設定登入者的資料。

## PowerShell 腳本

| 腳本 | 使用位置 |
| --- | --- |
| `release-info.ps1` | 統一解析組建及測試用的版本與檔名。 |
| `build-release.ps1` | 建置 x64／x86 DLL、Broker 和 Maintenance。 |
| `build-installer.ps1` | 核對公用倉頡來源、建置附初始資料的發行元件，再編譯 `setup.iss`。 |
| `build-icon.ps1` | 由原始 PNG 重建 ICO。 |
| `msvc.ps1` | 尋找 MSVC 建置環境，供 XAML 與入口探針測試。 |
| `register.ps1`、`update-dll.ps1`、`unregister.ps1` | 管理員從開發目錄更新或移除 TSF 登錄，委由 Maintenance 執行。 |
| `sandbox-broker.ps1` | 開發版以一般使用者權限執行沙盒字庫服務。正式版由登入維護程序啟動 Broker。 |
| `diagnostics.ps1`、`src/maintenance/events.ps1` | 從開發目錄讀取或管理事件檢視器通道，共用維護程式的事件讀取邏輯。 |
| `src/maintenance/dialogs.ps1` | 選擇資料目錄、更新項目、匯出日誌及設定指定程式的視窗。 |
| `src/maintenance/registration.ps1` | 安裝、升級及解除安裝的 Windows 系統登錄和事件通道設定。 |
| `tests/tsf_xaml_test.ps1`、`tests/tsf_x86_test.ps1` | x64／x86 XAML 宿主測試與 x86 獨立 COM 登錄測試。 |
| `scripts/diagnostics/` 中的兩個 LoL 追蹤腳本及 `build-entry-probe.ps1` | LoL 入口及系統事件診斷，留作開發排查。 |

原本的獨立 LoL 復原執行檔、安裝管理腳本和公用更新排程腳本已由 Maintenance 接管。Inno Setup 執行受保護的 Maintenance 做 x64／x86 註冊及清理。使用者資料初始化和背景更新由登入者以一般權限執行。自訂資料路徑用 `HKCU\Software\OxideIME\DataDirectory` 定位。從開發目錄執行時，讀取的詞庫、更新設定和套用的公用更新都位於專案的 `data/`。

## 驗證

~~~powershell
cargo test --locked --offline --lib
cargo test --locked --offline --target i686-pc-windows-msvc --lib
cargo clippy --locked --offline --all-targets -- -D warnings
cargo clippy --locked --offline --target i686-pc-windows-msvc --all-targets -- -D warnings
pwsh -NoProfile -File .\tests\tsf_xaml_test.ps1 -FixtureOnly
pwsh -NoProfile -File .\tests\tsf_x86_test.ps1 -FixtureOnly
~~~

從開發產物使用隔離字庫測試上屏、滑鼠及 TSF 自繪候選。

~~~powershell
. .\scripts\release-info.ps1
$release = Get-ImeRelease
python -B .\tests\tsf_real_store_test.py $release.Dll
pwsh -NoProfile -File .\tests\tsf_xaml_test.ps1 -FixtureOnly -LongText .\work\regression\zs.txt
python -B .\tests\tsf_privacy_test.py $release.Dll
python -B .\tests\tsf_mouse_test.py $release.Dll
python -B .\tests\tsf_uiless_activation_test.py $release.Dll
~~~

AppContainer 驗證使用 `integration-fixtures` 組建。此測試功能產生合成剪貼簿和專用接口，僅供隔離測試。

~~~powershell
cargo build --release --offline --features integration-fixtures --target-dir work/fixture-target
python -B .\tests\appcontainer_test.py .\work\fixture-target\release\oxide_ime.dll .\work\fixture-target\release\oxide-ime-broker.exe
~~~

另可加 `--registered-dll` 核對目前 Windows 登記的相同版本。32 位元 LPAC 可用 x86 DLL、x64 Broker 及對應的 x86 Python 進行。開發主機以外的完整安裝、重裝及解除安裝可按 [安裝說明](installation.md) 驗證。

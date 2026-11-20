#define AppName "Oxide IME"
#define DLL "oxide_ime_" + Release + ".dll"
#define DLLx86 "oxide_ime_" + Release + "_x86.dll"
#define Broker "oxide_ime_broker_" + Release + ".exe"
#define Maintenance "oxide_ime_maintenance_" + Release + ".exe"
#ifndef SetupFileName
#define SetupFileName "OxideIme-Setup-" + Release
#endif

[Setup]
AppId={{4F7BC212-59FE-424E-80CF-B13713B28EDA}
AppName={#AppName}
AppVersion={#Release}
VersionInfoVersion={#AppNumber}.0
DefaultDirName={autopf}\OxideIME
UsePreviousAppDir=no
DisableDirPage=yes
DisableProgramGroupPage=yes
PrivilegesRequired=admin
SetupArchitecture=x64
ArchitecturesAllowed=x64os
ArchitecturesInstallIn64BitMode=x64os
MinVersion=10.0.19041
CloseApplications=no
RestartApplications=no
SetupIconFile=..\assets\oxide-ime.ico
UninstallDisplayIcon={app}\bin\{#DLL}
WizardStyle=modern
OutputDir={#OutputFolder}
OutputBaseFilename={#SetupFileName}
Compression=lzma2
SolidCompression=yes
SetupLogging=yes
ArchiveExtraction=full

[Languages]
Name: "zhTW"; MessagesFile: "compiler:Languages\ChineseTraditional.isl"

[Files]
Source: "{#Binaries}\{#DLL}"; DestDir: "{app}\bin"; Flags: ignoreversion restartreplace uninsrestartdelete
Source: "{#Binaries}\{#DLLx86}"; DestDir: "{app}\bin"; Flags: ignoreversion restartreplace uninsrestartdelete
Source: "{#Binaries}\{#Broker}"; DestDir: "{app}\bin"; Flags: ignoreversion restartreplace uninsrestartdelete
Source: "{#Binaries}\{#Maintenance}"; DestDir: "{app}\bin"; Flags: ignoreversion restartreplace uninsrestartdelete
Source: "{#Licenses}\*"; DestDir: "{app}\licenses"; Flags: ignoreversion recursesubdirs
Source: "{#BuildTemplates}\cuhk-rank.txt"; DestDir: "{tmp}\oxide-seed\build"; Flags: deleteafterinstall
Source: "{#BuildTemplates}\s2t-rules\STPhrasesMultiOverrideHK.json"; DestDir: "{tmp}\oxide-seed\build\s2t-rules"; Flags: deleteafterinstall
Source: "{#BuildTemplates}\s2t-rules\uncommon-fonts.json"; DestDir: "{tmp}\oxide-seed\build\s2t-rules"; Flags: deleteafterinstall
Source: "{#BuildTemplates}\s2t-rules\exclusions.json"; DestDir: "{tmp}\oxide-seed\build\s2t-rules"; Flags: deleteafterinstall
Source: "{tmp}\texgyretermes-regular.otf"; DestDir: "{autofonts}"; FontInstall: "TeXGyreTermes-Regular (OpenType)"; Flags: external skipifsourcedoesntexist onlyifdoesntexist uninsneveruninstall fontisnttruetype; Check: InstallTermes
Source: "{tmp}\TW-Kai.ttf"; DestDir: "{autofonts}"; FontInstall: "全字庫正楷體"; Flags: external skipifsourcedoesntexist onlyifdoesntexist uninsneveruninstall; Check: InstallKai

[Messages]
FinishedLabel=已完成 Oxide IME 安裝。重新開啟要輸入的程式，按 Win+Space 選 Oxide IME。如未列出，請到 Windows 繁體中文語言選項新增鍵盤。

[Code]
const
  ClassKey = 'SOFTWARE\Classes\CLSID\{4BD0700E-5DD1-4310-A9D2-F3D579D54730}\InprocServer32';
  RunKey = 'SOFTWARE\Microsoft\Windows\CurrentVersion\Run';
  TermesUrl = 'https://mirrors.mit.edu/CTAN/fonts/tex-gyre/opentype/texgyretermes-regular.otf';
  TermesHash = 'cc3fe7c707b81428d23d54df3eadd9228a2bf6a4d43125d94df56f5f63134659';
  KaiUrl = 'https://data.cns11643.gov.tw/opendata/Fonts_Kai.zip';
  KaiHash = 'c602f1518f496252184ffd38f3cc8dc0a2d50fef4815e9340edda71dd501920f';

var
  UpdatesPage, FontsPage, DataPage: TInputOptionWizardPage;
  DownloadPage: TOutputProgressWizardPage;
  Keys: TArrayOfString;
  TermesIndex, KaiIndex: Integer;
  TermesReady, KaiReady: Boolean;
  InstallError: String;
  PreviousMaintenance: String;
  Upgrade, RestartForCleanup: Boolean;

function HasTermes: Boolean;
begin
  Result := FontExists('TeX Gyre Termes') or FontExists('TeXGyreTermes');
end;

function HasKai: Boolean;
begin
  Result := FontExists('TW-Kai') or FontExists('全字庫正楷體');
end;

procedure InitializeWizard;
var
  I: Integer;
  OldPath: String;
begin
  Upgrade := RegQueryStringValue(HKLM64, ClassKey, '', OldPath) and
    (CompareText(ExtractFileDir(OldPath), ExpandConstant('{autopf}\OxideIME\bin')) = 0);
  DataPage := CreateInputOptionPage(wpWelcome, '使用者資料目錄',
    '選擇資料目錄，詞庫與設定會分放在 dictionaries 和 setting',
    '預設儲存於 %LOCALAPPDATA%\OxideIME\data。勾選後可另選資料夾。',
    False, False);
  DataPage.Add('選擇其他資料目錄');
  SetArrayLength(Keys, 5);
  Keys[0] := 'opencc';
  Keys[1] := 's2t';
  Keys[2] := 'jyutping';
  Keys[3] := 'ranking';
  Keys[4] := 'cangjie';
  UpdatesPage := CreateInputOptionPage(DataPage.ID, '公用資料更新',
    '選擇要自動更新的資料',
    '取消勾選的項目會保留現有版本。所有自訂倉頡檔案由使用者自行管理。',
    False, False);
  UpdatesPage.Add('OpenCC 繁轉簡字詞表');
  UpdatesPage.Add('OpenCC／同文堂 簡轉繁詞組表');
  UpdatesPage.Add('Rime Cantonese 粵拼及 CanCLID 讀音');
  UpdatesPage.Add('香港常用字及字頻');
  UpdatesPage.Add('Cangjie3-Plus 公用倉頡字庫');
  for I := 0 to GetArrayLength(Keys) - 1 do UpdatesPage.Values[I] := True;

  FontsPage := CreateInputOptionPage(UpdatesPage.ID, '選用字型',
    '按需要下載並安裝缺少的字型',
    '略過時會使用 Windows 後備字型。',
    False, False);
  TermesIndex := -1;
  KaiIndex := -1;
  if not HasTermes then
    TermesIndex := FontsPage.Add('TeX Gyre Termes（CTAN）');
  if not HasKai then
    KaiIndex := FontsPage.Add('TW-Kai 全字庫正楷體（數位發展部）');
  DownloadPage := CreateOutputProgressPage('下載字型', '正在從官方來源下載選用字型');
end;

function ShouldSkipPage(PageID: Integer): Boolean;
begin
  Result := ((PageID = FontsPage.ID) and (TermesIndex = -1) and (KaiIndex = -1)) or
    (Upgrade and ((PageID = DataPage.ID) or (PageID = UpdatesPage.ID)));
end;

function InstallTermes: Boolean;
begin
  Result := TermesReady;
end;

function InstallKai: Boolean;
begin
  Result := KaiReady;
end;

function DownloadProgress(const Url, FileName: String; const Progress, Total: Int64): Boolean;
begin
  DownloadPage.SetProgress(Progress div 1024, Total div 1024);
  Result := True;
end;

procedure RequestFonts;
var
  FontPath: String;
begin
  if TermesIndex >= 0 then
    if FontsPage.Values[TermesIndex] and not HasTermes then begin
      DownloadPage.SetText('TeX Gyre Termes', 'CTAN');
      DownloadPage.Show;
      try
        try
          DownloadTemporaryFile(TermesUrl, 'texgyretermes-regular.otf', TermesHash, @DownloadProgress);
          TermesReady := True;
        except
          Log(GetExceptionMessage);
          SuppressibleMsgBox('TeX Gyre Termes 暫時下載不到。安裝會繼續並使用後備字型。',
            mbInformation, MB_OK, IDOK);
        end;
      finally
        DownloadPage.Hide;
      end;
    end;
  if KaiIndex >= 0 then
    if FontsPage.Values[KaiIndex] and not HasKai then begin
      DownloadPage.SetText('TW-Kai 全字庫正楷體', '數位發展部');
      DownloadPage.Show;
      try
        try
          DownloadTemporaryFile(KaiUrl, 'Fonts_Kai.zip', KaiHash, @DownloadProgress);
          ForceDirectories(ExpandConstant('{tmp}\kai'));
          ExtractArchive(ExpandConstant('{tmp}\Fonts_Kai.zip'), ExpandConstant('{tmp}\kai'), '', False, nil);
          FontPath := ExpandConstant('{tmp}\kai\TW-Kai.ttf');
          if not FileExists(FontPath) then
            FontPath := ExpandConstant('{tmp}\kai\TW-Kai-98_1.ttf');
          if not CopyFile(FontPath, ExpandConstant('{tmp}\TW-Kai.ttf'), True) then
            RaiseException('找不到全字庫正楷主字型。');
          KaiReady := True;
        except
          Log(GetExceptionMessage);
          SuppressibleMsgBox('全字庫正楷體暫時下載不到。安裝會繼續並使用後備字型。',
            mbInformation, MB_OK, IDOK);
        end;
      finally
        DownloadPage.Hide;
      end;
    end;
end;

function SelectedUpdates: String;
var
  I: Integer;
begin
  Result := '';
  for I := 0 to GetArrayLength(Keys) - 1 do
    if UpdatesPage.Values[I] then begin
      if Result <> '' then Result := Result + ',';
      Result := Result + Keys[I];
    end;
  if Result = '' then Result := '-';
end;

procedure ManageInstall(const Exe, Arguments: String);
var
  Code, I: Integer;
  Output: TExecOutput;
  ErrorText: String;
begin
  if not ExecAndCaptureOutputWithNativeSysDir(Exe, Arguments,
    ExpandConstant('{app}'), SW_HIDE, ewWaitUntilTerminated, Code, Output) then
    RaiseException('無法啟動輸入法安裝工具。');
  for I := 0 to GetArrayLength(Output.StdOut) - 1 do begin
    Log(Output.StdOut[I]);
    if Output.StdOut[I] = 'OXIDE_IME_RESTART_REQUIRED' then RestartForCleanup := True;
  end;
  if Code <> 0 then begin
    ErrorText := '';
    for I := 0 to GetArrayLength(Output.StdErr) - 1 do begin
      Log(Output.StdErr[I]);
      if ErrorText = '' then ErrorText := Output.StdErr[I];
    end;
    RaiseException('輸入法安裝操作未完成。' + #13#10 + ErrorText);
  end;
end;

function PrepareToInstall(var NeedsRestart: Boolean): String;
var
  AppDir, BinDir, Path: String;
  EndQuote: Integer;
  Found: TFindRec;
begin
  Result := '';
  PreviousMaintenance := '';
  AppDir := ExpandConstant('{app}');
  BinDir := AppDir + '\bin';
  if CompareText(AppDir, ExpandConstant('{autopf}\OxideIME')) <> 0 then begin
    Result := '請使用 Program Files 下的 OxideIME 安裝目錄。';
    exit;
  end;
  if RegQueryStringValue(HKLM64, ClassKey, '', Path) then
    if CompareText(ExtractFileDir(Path), BinDir) <> 0 then begin
      Result := 'Windows 已登記另一套 Oxide IME。請先移除該安裝或解除登錄。';
      exit;
    end;
  if FindFirst(AppDir + '\*', Found) then
    try
      repeat
        if (Found.Name <> '.') and (Found.Name <> '..') then
          if (Found.Attributes and $400 <> 0) or
            ((CompareText(Found.Name, 'bin') <> 0) and (CompareText(Found.Name, 'licenses') <> 0) and
             (CompareText(Found.Name, 'templates') <> 0) and
             (CompareText(Found.Name, 'unins000.exe') <> 0) and (CompareText(Found.Name, 'unins000.dat') <> 0)) then begin
            Result := '正式安裝目錄含有其他檔案。請先移除原有安裝或整理該目錄。';
            exit;
          end;
      until not FindNext(Found);
    finally
      FindClose(Found);
    end;
  if RegQueryStringValue(HKLM64, RunKey, 'OxideIme', Path) then begin
    if Copy(Path, 1, 1) = '"' then Path := Copy(Path, 2, Length(Path) - 1) else Path := '';
    EndQuote := Pos('"', Path);
    PreviousMaintenance := Copy(Path, 1, EndQuote - 1);
    if (CompareText(ExtractFileDir(PreviousMaintenance), BinDir) <> 0) or
      (CompareText(Copy(ExtractFileName(PreviousMaintenance), 1, 23), 'oxide_ime_maintenance_v') <> 0) or
      (Copy(Path, EndQuote, Length(Path)) <> '" --session') then begin
      Result := '同名登入啟動項目由其他程式持有。';
      exit;
    end;
  end;
  if not WizardSilent then RequestFonts;
  if FileExists(PreviousMaintenance) then
    try
      ManageInstall(PreviousMaintenance, '--prepare-upgrade');
    except
      Result := GetExceptionMessage;
    end;
end;

procedure CurStepChanged(CurStep: TSetupStep);
var
  Code: Integer;
  Exe, InitArgs: String;
begin
  if CurStep = ssPostInstall then begin
    Exe := ExpandConstant('{app}\bin\{#Maintenance}');
    try
      ManageInstall(Exe, '--register');
    except
      InstallError := GetExceptionMessage;
      if FileExists(PreviousMaintenance) then
        ExecAsOriginalUser(PreviousMaintenance, '--session', ExpandConstant('{app}'), SW_HIDE, ewNoWait, Code);
      SuppressibleMsgBox(InstallError, mbError, MB_OK, IDOK);
      exit;
    end;
    InitArgs := '--initialize ' + SelectedUpdates + ' "' + ExpandConstant('{tmp}\oxide-seed') + '"';
    if (not WizardSilent) and (not Upgrade) and DataPage.Values[0] then
      InitArgs := InitArgs + ' --select-data';
    if not ExecAsOriginalUser(Exe, InitArgs, ExpandConstant('{app}'), SW_HIDE,
      ewWaitUntilTerminated, Code) or (Code <> 0) then
        SuppressibleMsgBox('未能初始化目前登入者的資料。請以平常使用輸入法的帳戶重新執行安裝程式。', mbInformation, MB_OK, IDOK);
    if not ExecAsOriginalUser(Exe, '--session',
      ExpandConstant('{app}'), SW_HIDE, ewNoWait, Code) then
      Log('字庫服務會在下一次登入時啟動。');
  end;
end;

function NeedRestart: Boolean;
begin
  Result := RestartForCleanup;
end;

function GetCustomSetupExitCode: Integer;
begin
  Result := 0;
  if InstallError <> '' then Result := 10;
end;

procedure CurPageChanged(PageID: Integer);
begin
  if (PageID = wpFinished) and (InstallError <> '') then
    WizardForm.FinishedLabel.Caption := InstallError;
end;

procedure CurUninstallStepChanged(CurUninstallStep: TUninstallStep);
begin
  if CurUninstallStep = usUninstall then
    ManageInstall(ExpandConstant('{app}\bin\{#Maintenance}'), '--unregister');
end;

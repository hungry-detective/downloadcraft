; Download Craft's Windows installer (Inno Setup 6).
; Built by CI from the portable package's folder:
;   iscc /DAppVersion=0.9.16 /DSourceDir=<unzipped portable folder> /O<output dir> installer\downloadcraft.iss
; Installs for the current user (no administrator rights needed), so the
; app's one-click updates can replace its own files, as the portable copy does.

#ifndef AppVersion
  #define AppVersion "0.0.0"
#endif
#ifndef SourceDir
  #error SourceDir must point at the unzipped portable package
#endif

[Setup]
AppId={{6F1E2B7C-3D5A-4C8E-9B21-D0C0FA17C2A1}
AppName=Download Craft
AppVersion={#AppVersion}
AppVerName=Download Craft {#AppVersion}
AppPublisher=Download Craft
AppPublisherURL=https://github.com/hungry-detective/downloadcraft
AppSupportURL=https://github.com/hungry-detective/downloadcraft/issues
DefaultDirName={localappdata}\Programs\Download Craft
DefaultGroupName=Download Craft
DisableProgramGroupPage=yes
PrivilegesRequired=lowest
PrivilegesRequiredOverridesAllowed=dialog
OutputBaseFilename=DownloadCraft-{#AppVersion}-windows-x64-setup
SetupIconFile=..\assets\icon.ico
UninstallDisplayIcon={app}\download_craft.exe
Compression=lzma2/max
SolidCompression=yes
WizardStyle=modern
ArchitecturesAllowed=x64compatible
ArchitecturesInstallIn64BitMode=x64compatible
CloseApplications=yes
RestartApplications=no
LicenseFile={#SourceDir}\LICENSE

[Languages]
Name: "english"; MessagesFile: "compiler:Default.isl"

[Tasks]
Name: "desktopicon"; Description: "{cm:CreateDesktopIcon}"; GroupDescription: "{cm:AdditionalIcons}"
Name: "autostart"; Description: "Start Download Craft when I sign in to Windows"; GroupDescription: "Options:"; Flags: unchecked

[Files]
Source: "{#SourceDir}\*"; DestDir: "{app}"; Flags: ignoreversion recursesubdirs createallsubdirs

[Icons]
Name: "{group}\Download Craft"; Filename: "{app}\download_craft.exe"
Name: "{group}\Uninstall Download Craft"; Filename: "{uninstallexe}"
Name: "{userdesktop}\Download Craft"; Filename: "{app}\download_craft.exe"; Tasks: desktopicon

[Registry]
Root: HKCU; Subkey: "Software\Microsoft\Windows\CurrentVersion\Run"; ValueType: string; ValueName: "Download Craft"; ValueData: """{app}\download_craft.exe"" --startup"; Flags: uninsdeletevalue; Tasks: autostart

[Run]
Filename: "{app}\download_craft.exe"; Description: "{cm:LaunchProgram,Download Craft}"; Flags: nowait postinstall skipifsilent

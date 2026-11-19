; Inno Setup script for Thermo Companion.
; Build:  ISCC /DAppVersion=0.1.0 installer\thermo-companion.iss   (or installer\build.ps1)

#ifndef AppVersion
  #define AppVersion "0.1.0"
#endif
#define AppExe "thermo-companion.exe"

[Setup]
AppId={{B4E0A6C1-3D52-4F8B-9A17-6C2E5D8F0A39}
AppName=Thermo Companion
AppVersion={#AppVersion}
AppPublisher=miketsak.gr
AppPublisherURL=https://miketsak.gr
AppSupportURL=https://github.com/MikeTsak/Thermo-Companion
AppUpdatesURL=https://github.com/MikeTsak/Thermo-Companion/releases
DefaultDirName={autopf}\Thermo Companion
DefaultGroupName=Thermo Companion
DisableProgramGroupPage=yes
; Per-user install by default: no admin prompt. The user can still pick "all users".
PrivilegesRequired=lowest
PrivilegesRequiredOverridesAllowed=dialog
OutputDir=..\dist
OutputBaseFilename=ThermoCompanion-Setup-{#AppVersion}
SetupIconFile=..\assets\icon.ico
UninstallDisplayIcon={app}\icon.ico
UninstallDisplayName=Thermo Companion
Compression=lzma2
SolidCompression=yes
WizardStyle=modern
ArchitecturesAllowed=x64compatible
ArchitecturesInstallIn64BitMode=x64compatible
; Same name the app uses for its single-instance mutex: setup/uninstall ask you to close it.
AppMutex=ThermoCompanionSingleton,ThermoCompanionDashboard
LicenseFile=..\LICENSE

[Tasks]
Name: "startup"; Description: "Start Thermo Companion when Windows starts"; GroupDescription: "Options:"; Flags: unchecked
Name: "desktopicon"; Description: "Create a desktop shortcut"; GroupDescription: "Options:"; Flags: unchecked
Name: "contextmenu"; Description: "Add ""Thermo Print"" to file right-click menu"; GroupDescription: "Options:"

[Files]
Source: "..\target\release\{#AppExe}"; DestDir: "{app}"; Flags: ignoreversion
Source: "..\assets\icon.ico"; DestDir: "{app}"; Flags: ignoreversion
Source: "..\README.md"; DestDir: "{app}"; Flags: ignoreversion
Source: "..\LICENSE"; DestDir: "{app}"; Flags: ignoreversion
Source: "..\extension\*"; DestDir: "{app}\extension"; Flags: ignoreversion recursesubdirs createallsubdirs

[Icons]
Name: "{autoprograms}\Thermo Companion"; Filename: "{app}\{#AppExe}"; IconFilename: "{app}\icon.ico"
Name: "{autodesktop}\Thermo Companion"; Filename: "{app}\{#AppExe}"; IconFilename: "{app}\icon.ico"; Tasks: desktopicon
Name: "{userappdata}\Microsoft\Windows\SendTo\Thermo Print"; Filename: "{app}\{#AppExe}"; Parameters: "--print-file"; IconFilename: "{app}\icon.ico"; Tasks: contextmenu

[Registry]
Root: HKA; Subkey: "Software\Microsoft\Windows\CurrentVersion\Run"; ValueType: string; ValueName: "ThermoCompanion"; ValueData: """{app}\{#AppExe}"""; Flags: uninsdeletevalue; Tasks: startup
Root: HKA; Subkey: "Software\Classes\*\shell\ThermoPrint"; ValueType: string; ValueData: "Thermo Print"; Flags: uninsdeletekey; Tasks: contextmenu
Root: HKA; Subkey: "Software\Classes\*\shell\ThermoPrint"; ValueType: string; ValueName: "Icon"; ValueData: """{app}\icon.ico"""; Tasks: contextmenu
Root: HKA; Subkey: "Software\Classes\*\shell\ThermoPrint\command"; ValueType: string; ValueData: """{app}\{#AppExe}"" --print-file ""%1"""; Tasks: contextmenu
Root: HKA; Subkey: "Software\Classes\Directory\shell\ThermoPrint"; ValueType: string; ValueData: "Thermo Print"; Flags: uninsdeletekey; Tasks: contextmenu
Root: HKA; Subkey: "Software\Classes\Directory\shell\ThermoPrint"; ValueType: string; ValueName: "Icon"; ValueData: """{app}\icon.ico"""; Tasks: contextmenu
Root: HKA; Subkey: "Software\Classes\Directory\shell\ThermoPrint\command"; ValueType: string; ValueData: """{app}\{#AppExe}"" --print-file ""%1"""; Tasks: contextmenu

[Run]
Filename: "{app}\{#AppExe}"; Parameters: "--autostart"; Description: "Start Pip now"; Flags: nowait postinstall skipifsilent; Tasks: startup
Filename: "{app}\{#AppExe}"; Description: "Start Pip now"; Flags: nowait postinstall skipifsilent; Tasks: not startup

[UninstallRun]
Filename: "{sys}\taskkill.exe"; Parameters: "/im {#AppExe} /f"; Flags: runhidden; RunOnceId: "StopThermoCompanion"

; Script d'installation officiel — MCP Manager
[Setup]
AppId={{A2EE16A2-5315-4B01-93B3-925D738B0C10}
AppName=MCP Manager
AppVersion=1.0.1
AppPublisher=MCP Manager
DefaultDirName={autopf}\MCP Manager
DefaultGroupName=MCP Manager
OutputDir=..\dist
OutputBaseFilename=MCP-Manager-Setup
SetupIconFile=..\flutter\windows\runner\resources\app_icon.ico
UninstallDisplayIcon={app}\app_icon.ico
ArchitecturesInstallIn64BitMode=x64
CloseApplications=force
RestartApplications=no
PrivilegesRequired=admin
ChangesAssociations=yes

[Files]
Source: "..\dist\MCP-Manager\*"; DestDir: "{app}"; Flags: recursesubdirs ignoreversion

[Icons]
Name: "{group}\MCP Manager"; Filename: "{app}\MCP-Manager.exe"; IconFilename: "{app}\app_icon.ico"
Name: "{autodesktop}\MCP Manager"; Filename: "{app}\MCP-Manager.exe"; IconFilename: "{app}\app_icon.ico"

[Run]
Filename: "{app}\MCP-Manager.exe"; Description: "Lancer MCP Manager"; Flags: nowait postinstall skipifsilent

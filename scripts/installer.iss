; Exemple facultatif — compiler après scripts/build.ps1 avec Inno Setup 6
[Setup]
AppId={{A2EE16A2-5315-4B01-93B3-925D738B0C10}
AppName=MCP Hub
AppVersion=1.0.0
DefaultDirName={autopf}\MCP Hub
DefaultGroupName=MCP Hub
OutputDir=..\dist
OutputBaseFilename=MCP-Hub-Setup
ArchitecturesInstallIn64BitMode=x64

[Files]
Source: "..\dist\MCP-Hub\*"; DestDir: "{app}"; Flags: recursesubdirs ignoreversion

[Icons]
Name: "{group}\MCP Hub"; Filename: "{app}\mcp_hub.exe"
Name: "{autodesktop}\MCP Hub"; Filename: "{app}\mcp_hub.exe"

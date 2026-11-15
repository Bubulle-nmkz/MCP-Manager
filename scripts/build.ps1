$ErrorActionPreference = 'Stop'
$Root = Split-Path -Parent $PSScriptRoot
$Flutter = Join-Path $Root 'flutter'
$Dist = Join-Path $Root 'dist\MCP-Manager'
Write-Host '[1/5] Vérification des prérequis Windows…'
foreach ($tool in @('cargo','flutter')) { if (-not (Get-Command $tool -ErrorAction SilentlyContinue)) { throw "Commande requise introuvable: $tool" } }
Write-Host '[2/5] Tests Rust…'
cargo test --manifest-path (Join-Path $Root 'Cargo.toml') -p mcp_hub_core
if ($LASTEXITCODE -ne 0) { throw 'Échec cargo test' }
cargo build --manifest-path (Join-Path $Root 'Cargo.toml') --release -p mcp_hub_core
if ($LASTEXITCODE -ne 0) { throw 'Échec cargo build' }
Write-Host '[3/5] Préparation Flutter Windows…'
Push-Location $Flutter
try {
  if (-not (Test-Path (Join-Path $Flutter 'windows\CMakeLists.txt'))) { flutter create --platforms=windows --project-name mcp_hub .; if ($LASTEXITCODE -ne 0) { throw 'Échec flutter create' } }
  flutter pub get; if ($LASTEXITCODE -ne 0) { throw 'Échec flutter pub get' }
  Write-Host '[4/5] Analyse et build Flutter Windows…'
  flutter analyze; if ($LASTEXITCODE -ne 0) { throw 'Échec flutter analyze' }
  flutter build windows --release; if ($LASTEXITCODE -ne 0) { throw 'Échec flutter build' }
} finally { Pop-Location }
$ExeDir = Join-Path $Flutter 'build\windows\x64\runner\Release'
if (-not (Test-Path $ExeDir)) { $ExeDir = Join-Path $Flutter 'build\windows\runner\Release' }
if (-not (Test-Path $ExeDir)) { throw "Dossier Flutter Release introuvable: $ExeDir" }
if (Test-Path $Dist) {
  try {
    Remove-Item $Dist -Recurse -Force -ErrorAction Stop
  } catch {
    Write-Host 'Fermeture forcée des processus résiduels...'
    Get-Process -Name 'mcp_hub','MCP-Manager','mcp-hub-core' -ErrorAction SilentlyContinue | Stop-Process -Force
    Start-Sleep -Milliseconds 800
    Remove-Item $Dist -Recurse -Force -ErrorAction SilentlyContinue
  }
}
New-Item -ItemType Directory -Force -Path $Dist | Out-Null
Copy-Item (Join-Path $ExeDir '*') $Dist -Recurse -Force
$FlutterExe = Join-Path $Dist 'mcp_hub.exe'
$TargetExe = Join-Path $Dist 'MCP-Manager.exe'
if (Test-Path $FlutterExe) {
  Copy-Item $FlutterExe $TargetExe -Force
}
Copy-Item (Join-Path $Root 'target\release\mcp_hub_core.exe') (Join-Path $Dist 'mcp-hub-core.exe') -Force
Copy-Item (Join-Path $Root 'README.md') $Dist -Force
Write-Host "[5/5] Distribution prête: $Dist"

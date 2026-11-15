$ErrorActionPreference = 'Stop'
$Root = Split-Path -Parent $PSScriptRoot
$Flutter = Join-Path $Root 'flutter'
$Dist = Join-Path $Root 'dist\MCP-Manager'
$LogoPng = 'C:\Users\natha\Documents\MCP\Logo MCP manager.png'
if (-not (Test-Path $LogoPng)) {
  $LogoPng = Join-Path $Flutter 'assets\mcp-manager.png'
}
$AssetsPng = Join-Path $Flutter 'assets\mcp-manager.png'
$AssetsIco = Join-Path $Flutter 'assets\mcp-manager.ico'
$RunnerIco = Join-Path $Flutter 'windows\runner\resources\app_icon.ico'

Write-Host '[0/6] Fermeture des instances en cours de MCP Manager…'
Get-Process -Name 'mcp_hub','MCP-Manager','mcp-hub-core','mcp_hub_core' -ErrorAction SilentlyContinue | Stop-Process -Force -ErrorAction SilentlyContinue
Start-Sleep -Milliseconds 600

Write-Host '[1/6] Vérification des icônes (.ico et .png)…'
if ((Test-Path $LogoPng) -and ($LogoPng -ne $AssetsPng)) {
  Copy-Item $LogoPng $AssetsPng -Force
}

Add-Type -AssemblyName System.Drawing
function Convert-PngToMultiIco([string]$sourcePng, [string]$destIco) {
  if (-not (Test-Path $sourcePng)) {
    Write-Warning "Fichier source PNG introuvable : $sourcePng"
    return
  }
  $srcImg = [System.Drawing.Image]::FromFile($sourcePng)
  $sizes = @(16, 24, 32, 48, 64, 128, 256)
  $pngStreams = New-Object System.Collections.Generic.List[byte[]]

  foreach ($sz in $sizes) {
    $bmp = New-Object System.Drawing.Bitmap($sz, $sz, [System.Drawing.Imaging.PixelFormat]::Format32bppArgb)
    $g = [System.Drawing.Graphics]::FromImage($bmp)
    $g.InterpolationMode = [System.Drawing.Drawing2D.InterpolationMode]::HighQualityBicubic
    $g.SmoothingMode = [System.Drawing.Drawing2D.SmoothingMode]::HighQuality
    $g.PixelOffsetMode = [System.Drawing.Drawing2D.PixelOffsetMode]::HighQuality
    $g.Clear([System.Drawing.Color]::Transparent)
    $g.DrawImage($srcImg, 0, 0, $sz, $sz)
    $g.Dispose()

    $ms = New-Object System.IO.MemoryStream
    $bmp.Save($ms, [System.Drawing.Imaging.ImageFormat]::Png)
    $pngStreams.Add($ms.ToArray())
    $ms.Dispose()
    $bmp.Dispose()
  }
  $srcImg.Dispose()

  $fs = [System.IO.File]::Create($destIco)
  $bw = New-Object System.IO.BinaryWriter($fs)
  $bw.Write([uint16]0)
  $bw.Write([uint16]1)
  $bw.Write([uint16]$sizes.Count)

  $offset = 6 + (16 * $sizes.Count)
  for ($i = 0; $i -lt $sizes.Count; $i++) {
    $sz = $sizes[$i]
    $data = $pngStreams[$i]
    $wByte = if ($sz -ge 256) { [byte]0 } else { [byte]$sz }
    $bw.Write($wByte)
    $bw.Write($wByte)
    $bw.Write([byte]0)
    $bw.Write([byte]0)
    $bw.Write([uint16]1)
    $bw.Write([uint16]32)
    $bw.Write([uint32]$data.Length)
    $bw.Write([uint32]$offset)
    $offset += $data.Length
  }
  for ($i = 0; $i -lt $sizes.Count; $i++) {
    $bw.Write($pngStreams[$i])
  }
  $bw.Flush()
  $bw.Close()
  $fs.Close()
}

if (Test-Path $AssetsPng) {
  Convert-PngToMultiIco $AssetsPng $AssetsIco
  Convert-PngToMultiIco $AssetsPng $RunnerIco
} elseif (-not (Test-Path $RunnerIco)) {
  Write-Warning "Aucune icône source trouvée."
}

# Forcer CMake/Visual Studio à recompiler les ressources Windows (.rc) avec la nouvelle icône
$RcFile = Join-Path $Flutter 'windows\runner\Runner.rc'
if (Test-Path $RcFile) {
  (Get-Item $RcFile).LastWriteTime = Get-Date
}
$BuildRunnerDir = Join-Path $Flutter 'build\windows\x64\runner'
if (Test-Path $BuildRunnerDir) {
  Remove-Item $BuildRunnerDir -Recurse -Force -ErrorAction SilentlyContinue
}

Write-Host '[2/6] Vérification des prérequis Windows…'
foreach ($tool in @('cargo','flutter')) { if (-not (Get-Command $tool -ErrorAction SilentlyContinue)) { throw "Commande requise introuvable: $tool" } }

Write-Host '[3/6] Tests et build Rust…'
cargo test --manifest-path (Join-Path $Root 'Cargo.toml') -p mcp_hub_core
if ($LASTEXITCODE -ne 0) { throw 'Échec cargo test' }
cargo build --manifest-path (Join-Path $Root 'Cargo.toml') --release -p mcp_hub_core
if ($LASTEXITCODE -ne 0) { throw 'Échec cargo build' }

Write-Host '[4/6] Analyse et build Flutter Windows…'
Push-Location $Flutter
try {
  flutter pub get; if ($LASTEXITCODE -ne 0) { throw 'Échec flutter pub get' }
  flutter analyze --no-fatal-infos --no-fatal-warnings
  if ($LASTEXITCODE -ne 0) { throw 'Échec flutter analyze' }
  flutter build windows --release; if ($LASTEXITCODE -ne 0) { throw 'Échec flutter build' }
} finally { Pop-Location }

$ExeDir = Join-Path $Flutter 'build\windows\x64\runner\Release'
if (-not (Test-Path $ExeDir)) { $ExeDir = Join-Path $Flutter 'build\windows\runner\Release' }
if (-not (Test-Path $ExeDir)) { throw "Dossier Flutter Release introuvable: $ExeDir" }

if (Test-Path $Dist) {
  Remove-Item $Dist -Recurse -Force -ErrorAction SilentlyContinue
}
New-Item -ItemType Directory -Force -Path $Dist | Out-Null
Copy-Item (Join-Path $ExeDir '*') $Dist -Recurse -Force
$FlutterExe = Join-Path $Dist 'mcp_hub.exe'
$TargetExe = Join-Path $Dist 'MCP-Manager.exe'
if (Test-Path $FlutterExe) {
  Copy-Item $FlutterExe $TargetExe -Force
}
Copy-Item $RunnerIco (Join-Path $Dist 'app_icon.ico') -Force
Copy-Item (Join-Path $Root 'target\release\mcp_hub_core.exe') (Join-Path $Dist 'mcp-hub-core.exe') -Force
Copy-Item (Join-Path $Root 'README.md') $Dist -Force
Write-Host "[5/6] Distribution prête avec le nouveau logo partout : $Dist"

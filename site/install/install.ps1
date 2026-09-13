#
# Astra CLI one-line installer (Windows PowerShell 5.1+).
#
#   irm https://astracode.topodrive.top/install/install.ps1 | iex
#
# Override the target directory with:
#   $env:ASTRA_INSTALL_DIR = "D:\astra"
$ErrorActionPreference = "Stop"

$InstallDir = if ($env:ASTRA_INSTALL_DIR) { $env:ASTRA_INSTALL_DIR } else { Join-Path $env:LOCALAPPDATA "Programs\astra" }
$Repo = if ($env:ASTRA_REPO) { $env:ASTRA_REPO } else { "kevenhu001-cyber/astra-code-v3" }

$arch = $env:PROCESSOR_ARCHITECTURE
if ($arch -eq "x86" -and $env:PROCESSOR_ARCHITEW6432) {
  $arch = $env:PROCESSOR_ARCHITEW6432
}
switch ($arch) {
  "AMD64" { $targetArch = "x86_64" }
  "ARM64" { $targetArch = "aarch64" }
  default { throw "Unsupported architecture: $arch" }
}

# Fetch latest release version from GitHub API
Write-Host "Fetching latest release info..."
$releaseInfo = Invoke-RestMethod -Uri "https://api.github.com/repos/$Repo/releases/latest"
$tag = $releaseInfo.tag_name
if (-not $tag) { throw "Failed to fetch latest release version" }
Write-Host "Latest release: $tag"

$asset = "astra-$tag-$targetArch-pc-windows-msvc.zip"
$ghBaseUrl = "https://github.com/$Repo/releases/download/$tag"
# GitHub serves release assets from release-assets.githubusercontent.com, which
# is intermittently unreachable from some networks and surfaces as a 0-byte EOF
# in Invoke-WebRequest. ghproxy.net fronts the same asset without that hop and is
# used as an automatic fallback.
$proxyBaseUrl = "https://ghproxy.net/https://github.com/$Repo/releases/download/$tag"
$tmp = Join-Path $env:TEMP ("astra-install-" + [guid]::NewGuid().ToString("N"))
New-Item -ItemType Directory -Path $tmp | Out-Null

# Download from a URL with retries. Returns $true on success.
function Get-FileWithRetry {
  param(
    [string]$Uri,
    [string]$OutFile,
    [int]$MaxAttempts = 4,
    [int]$TimeoutSec = 180
  )
  for ($i = 1; $i -le $MaxAttempts; $i++) {
    try {
      Invoke-WebRequest -UseBasicParsing -Uri $Uri -OutFile $OutFile `
        -TimeoutSec $TimeoutSec -MaximumRetryCount 3 -RetryIntervalSec 2
      if ((Test-Path $OutFile) -and ((Get-Item $OutFile).Length -gt 0)) {
        return $true
      }
      Write-Warning "Attempt $i`: downloaded file is empty, retrying..."
    } catch {
      Write-Warning "Attempt $i failed: $_"
    }
    if (Test-Path $OutFile) { Remove-Item -Force $OutFile -ErrorAction SilentlyContinue }
    Start-Sleep -Seconds 1
  }
  return $false
}

try {
  # Try GitHub directly first; fall back to the proxy if it fails (e.g. EOF).
  $downloaded = $false
  Write-Host "Downloading $asset ..."
  if (Get-FileWithRetry -Uri "$ghBaseUrl/$asset" -OutFile (Join-Path $tmp "astra.zip")) {
    Write-Host "Downloaded from GitHub releases."
    $downloaded = $true
  } else {
    Write-Host "GitHub download failed, retrying via proxy (ghproxy.net)..."
    if (Get-FileWithRetry -Uri "$proxyBaseUrl/$asset" -OutFile (Join-Path $tmp "astra.zip")) {
      Write-Host "Downloaded via proxy."
      $downloaded = $true
    }
  }
  if (-not $downloaded) {
    throw "Failed to download $asset from all sources"
  }

  # Extract zip — contents may be nested under astra/ subdirectory
  $extractDir = Join-Path $tmp "extract"
  Expand-Archive -Path (Join-Path $tmp "astra.zip") -DestinationPath $extractDir -Force

  # Verify the download against the published SHA-256 checksum when available.
  # The checksum is a small text file fetched from GitHub; if unreachable we
  # warn but do not block installation.
  $checksumOk = $false
  if (Get-FileWithRetry -Uri "$ghBaseUrl/$asset.sha256" -OutFile (Join-Path $tmp "astra.zip.sha256") -MaxAttempts 2 -TimeoutSec 30) {
    $expected = (Get-Content (Join-Path $tmp "astra.zip.sha256") -Raw).Split(' ')[0].Trim()
    $actual = (Get-FileHash (Join-Path $tmp "astra.zip") -Algorithm SHA256).Hash.ToLower()
    $checksumOk = ($expected -and $expected.ToLower() -eq $actual)
    if (-not $checksumOk) {
      Write-Warning "Checksum mismatch; the downloaded file may be corrupted."
    }
  }
  if (-not $checksumOk) {
    Write-Warning "Checksum verification skipped or failed; proceeding without verification."
  }

  # Find the astra.exe inside the archive.
  $exe = Get-ChildItem -Path $extractDir -Filter "astra.exe" -Recurse | Select-Object -First 1
  if (-not $exe) {
    throw "Executable not found in the archive"
  }

  # Install: copy exe to InstallDir as astra.exe
  New-Item -ItemType Directory -Path $InstallDir -Force | Out-Null
  Copy-Item $exe.FullName (Join-Path $InstallDir "astra.exe") -Force

  $astra = Join-Path $InstallDir "astra.exe"
  $userPath = [Environment]::GetEnvironmentVariable("Path", "User")
  $parts = if ($userPath) { $userPath -split ";" } else { @() }
  $pathUpdated = $false
  if ($parts -notcontains $InstallDir) {
    $newPath = if ($userPath) { "$InstallDir;$userPath" } else { $InstallDir }
    [Environment]::SetEnvironmentVariable("Path", $newPath, "User")
    $pathUpdated = $true
    Write-Host "Added $InstallDir to your user PATH (persistent)."
  }

  $currentParts = $env:Path -split ";"
  if ($currentParts -notcontains $InstallDir) {
    $env:Path = "$InstallDir;$env:Path"
    Write-Host "Updated PATH for this terminal."
  }

  if ($pathUpdated) {
    Write-Host ""
    Write-Host "VS Code: quit VS Code completely and reopen it, then open a new terminal."
    Write-Host "Other apps: restart them so they pick up the new PATH."
    Write-Host "This terminal already works: 'astra version' below."
  }

  & $astra version
  Write-Host "Astra installed: $InstallDir\astra.exe"
} finally {
  Remove-Item -Recurse -Force $tmp
}

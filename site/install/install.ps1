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
$baseUrl = "https://github.com/$Repo/releases/download/$tag"
$tmp = Join-Path $env:TEMP ("astra-install-" + [guid]::NewGuid().ToString("N"))
New-Item -ItemType Directory -Path $tmp | Out-Null

try {
  Write-Host "Downloading $asset ..."
  Invoke-WebRequest -UseBasicParsing -Uri "$baseUrl/$asset" -OutFile (Join-Path $tmp "astra.zip")

  # Extract zip — contents may be nested under astra/ subdirectory
  $extractDir = Join-Path $tmp "extract"
  Expand-Archive -Path (Join-Path $tmp "astra.zip") -DestinationPath $extractDir -Force

  # Verify the download against the published SHA-256 checksum.
  Invoke-WebRequest -UseBasicParsing -Uri "$baseUrl/$asset.sha256" -OutFile (Join-Path $tmp "astra.zip.sha256")
  $expected = (Get-Content (Join-Path $tmp "astra.zip.sha256") -Raw).Split(' ')[0].Trim()
  $actual = (Get-FileHash (Join-Path $tmp "astra.zip") -Algorithm SHA256).Hash.ToLower()
  if ($expected.ToLower() -ne $actual) {
    throw "Checksum verification failed for $asset"
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

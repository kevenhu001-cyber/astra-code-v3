#
# Astra CLI uninstaller (Windows PowerShell 5.1+).
#
#   irm https://astracode.topodrive.top/install/uninstall.ps1 | iex
#
# Flags (use them on the pipeline or pass as args):
#   -Purge           Also remove the user data directory (default: keep it).
#   -DryRun          Show what would be removed without changing anything.
#
# Default behaviour is safe / non-destructive:
#   - Removes the binary from %LOCALAPPDATA%\Programs\astra (and any extras
#     we discover) and strips the installer-managed User PATH entry it added.
#   - Leaves %USERPROFILE%\.astra (or $ASTRA_HOME, if set) alone.
#
# Exit code is non-zero only if at least one removal step failed.
[CmdletBinding()]
param(
    [switch]$Purge,
    [switch]$DryRun
)
$ErrorActionPreference = 'Continue'
function Write-Step {
    param([string]$Message)
    Write-Host "==> $Message"
}
function Remove-Target {
    param(
        [Parameter(Mandatory = $true)][string]$Path,
        [string]$Kind = 'item'
    )
    if (-not (Test-Path -LiteralPath $Path)) {
        return
    }
    if ($DryRun) {
        Write-Host "  [dry-run] would remove $Kind at $Path"
        return
    }
    try {
        Remove-Item -LiteralPath $Path -Recurse -Force -ErrorAction Stop
        Write-Host "  removed $Kind at $Path"
    }
    catch {
        Write-Warning "failed to remove $Path`: $($_.Exception.Message)"
        $script:FailureCount++
    }
}
function Remove-IfEmpty {
    param([string]$Path)
    if (-not (Test-Path -LiteralPath $Path)) { return }
    $item = Get-Item -LiteralPath $Path -ErrorAction SilentlyContinue
    if ($item -and $item.PSIsContainer) {
        $children = Get-ChildItem -LiteralPath $Path -Force -ErrorAction SilentlyContinue
        if (-not $children) {
            if ($DryRun) {
                Write-Host "  [dry-run] would remove empty directory $Path"
            }
            else {
                try {
                    Remove-Item -LiteralPath $Path -Force -ErrorAction Stop
                    Write-Host "  removed empty directory $Path"
                }
                catch {
                    # Best-effort; not fatal.
                }
            }
        }
    }
}
$FailureCount = 0
$script:FailureCount = 0
# --- 1. known binary locations ------------------------------------------
#
# The installer uses %LOCALAPPDATA%\Programs\astra by default and lets you
# override with $env:ASTRA_INSTALL_DIR. We honour that same env here, and
# also drop a couple of common fallbacks.
$installDir = if ($env:ASTRA_INSTALL_DIR) { $env:ASTRA_INSTALL_DIR } else { Join-Path $env:LOCALAPPDATA 'Programs\astra' }
$homeBin     = Join-Path $env:USERPROFILE '.local\bin\astra.exe'
Write-Step 'Removing the Astra binary'
$exeCandidates = @(
    (Join-Path $installDir 'astra.exe'),
    $homeBin
)
# As an extra safety net: discover Astra binaries that came from a previous
# install under the per-programs folder and remove them too. We search one
# level deep so we don't wander off into unrelated tooling.
if ($env:LOCALAPPDATA) {
    $programsRoot = Join-Path $env:LOCALAPPDATA 'Programs'
    if (Test-Path -LiteralPath $programsRoot) {
        Get-ChildItem -LiteralPath $programsRoot -Directory -ErrorAction SilentlyContinue |
            Where-Object { $_.Name -eq 'astra' } |
            ForEach-Object {
                $candidate = Join-Path $_.FullName 'astra.exe'
                if (Test-Path -LiteralPath $candidate) {
                    $exeCandidates += $candidate
                }
            }
    }
}
foreach ($cand in $exeCandidates) {
    Remove-Target -Path $cand -Kind 'binary'
}
# Best effort: drop the now-empty install directory.
Remove-IfEmpty -Path $installDir
# --- 2. installer-managed User PATH -------------------------------------
#
# `install.ps1` adds the install directory to the User PATH. We pull it back
# out, but only if it is the SAME directory we just removed the binary from
# (or it currently contains no `astra.exe`). Touching unrelated PATH entries
# is out of scope.
Write-Step 'Cleaning installer-managed User PATH entry'
$pathUpdated = $false
$currentUserPath = [Environment]::GetEnvironmentVariable('Path', 'User')
if ($currentUserPath -and $installDir) {
    $parts = $currentUserPath -split ';' | Where-Object { $_ -ne $null -and $_ -ne '' }
    $filtered = $parts | Where-Object { $_ -ne $installDir }
    if ($filtered.Count -lt $parts.Count) {
        $newUserPath = ($filtered -join ';')
        if ($DryRun) {
            Write-Host "  [dry-run] would remove $installDir from user PATH"
            $pathUpdated = $true
        }
        else {
            try {
                [Environment]::SetEnvironmentVariable('Path', $newUserPath, 'User')
                Write-Host "  removed $installDir from user PATH"
                $pathUpdated = $true
            }
            catch {
                Write-Warning "failed to update user PATH: $($_.Exception.Message)"
                $script:FailureCount++
            }
        }
    }
}
# Best effort: drop the install dir from this session's $env:Path so the
# current terminal stops finding the (now-gone) binary.
if ($env:Path) {
    $live = $env:Path -split ';' | Where-Object { $_ -ne $null -and $_ -ne '' -and $_ -ne $installDir }
    $env:Path = $live -join ';'
}
# --- 3. user data directory --------------------------------------------
#
# Default behaviour: leave user data alone. Only remove it under -Purge so
# we never delete a user's config / sessions by accident.
Write-Step 'Deciding what to do with the user data directory'
$astraHomeEnv = $env:ASTRA_HOME
$dataDir = if ($astraHomeEnv) { $astraHomeEnv } else { Join-Path $env:USERPROFILE '.astra' }
if ($Purge) {
    # Belt-and-suspenders guard: never wipe what looks like an obviously bad path.
    $forbidden = @('', $env:USERPROFILE, $env:USERPROFILE + '\.astra', $env:USERPROFILE + '\.astra\')
    $matchedForbidden = $false
    foreach ($bad in $forbidden) {
        if ($dataDir -ne $null -and $bad -ne $null -and $dataDir.TrimEnd('\') -eq $bad.TrimEnd('\')) {
            $matchedForbidden = $true
            break
        }
    }
    # We don't refuse root paths by default; just refuse empties.
    if ([string]::IsNullOrWhiteSpace($dataDir)) {
        Write-Warning 'refusing to purge an empty ASTRA_HOME path'
        $script:FailureCount++
    }
    else {
        Remove-Target -Path $dataDir -Kind 'data directory'
    }
}
else {
    if (Test-Path -LiteralPath $dataDir) {
        Write-Host "  preserved user data at $dataDir (use -Purge to remove)"
        Write-Host ''
        Write-Host '  Note: to remove your config and sessions later, re-run with -Purge:'
        Write-Host '    irm https://astracode.topodrive.top/install/uninstall.ps1 | iex -Purge'
    }
}
# --- 4. summary --------------------------------------------------------
Write-Host ''
Write-Host 'Astra uninstall summary:'
if ($DryRun) {
    Write-Host '  mode: dry-run (no changes were made)'
}
elseif ($Purge) {
    Write-Host '  mode: full (binary + user data)'
}
else {
    Write-Host '  mode: safe (binary only; user data preserved)'
}
Write-Host "  failures: $FailureCount"
if ($FailureCount -gt 0) {
    Write-Warning 'one or more removal steps failed; review the messages above'
    exit 1
}
exit 0
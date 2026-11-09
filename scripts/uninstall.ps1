# apkscope uninstaller for Windows (PowerShell).
#
# Removes apkscope.exe and the install directory created by scripts/install.ps1,
# and drops the directory from your user PATH.
#
# Usage:
#   ./scripts/uninstall.ps1

$ErrorActionPreference = 'Stop'

# --- output helpers (kept in sync with install.ps1; each script stays ----------
# self-contained because it may be piped straight into iex)
$interactive = -not [Console]::IsOutputRedirected
$script:Plain = -not $interactive -or [bool]$env:NO_COLOR -or $env:TERM -eq 'dumb'
$script:Tick  = if ($interactive -and $env:TERM -ne 'dumb' -and ($env:WT_SESSION -or [Console]::OutputEncoding.CodePage -eq 65001)) { [char]0x2713 } else { '+' }

function Write-Title($text) {
    if ($script:Plain) { Write-Host "  $text" } else { Write-Host "  $text" -ForegroundColor White }
}
function Write-Ok($msg) {
    Write-Host '  ' -NoNewline
    if ($script:Plain) { Write-Host "$($script:Tick) $msg" }
    else { Write-Host $script:Tick -ForegroundColor Green -NoNewline; Write-Host " $msg" }
}
function Write-Note($msg) {
    if ($script:Plain) { Write-Host "note: $msg" }
    else { Write-Host 'note: ' -ForegroundColor DarkGray -NoNewline; Write-Host $msg }
}

$Dest = Join-Path $env:LOCALAPPDATA 'Programs\apkscope'

Write-Host ''
Write-Title 'apkscope uninstaller'
Write-Host ''

if (Test-Path $Dest) {
    Remove-Item -Recurse -Force -ErrorAction SilentlyContinue $Dest
    Write-Ok "removed $Dest"
} else {
    Write-Note "$Dest was not present (already uninstalled?)"
}

# install.ps1 added $Dest to the user PATH; drop it back out if it is still there.
$userPath = [Environment]::GetEnvironmentVariable('Path', 'User')
if ($userPath -and ($userPath -split ';' -contains $Dest)) {
    $parts = $userPath -split ';' | Where-Object { $_ -and ($_ -ne $Dest) }
    [Environment]::SetEnvironmentVariable('Path', ($parts -join ';'), 'User')
    Write-Ok "removed $Dest from your user PATH (restart terminals to pick it up)"
}

Write-Host ''
Write-Host '  Done.'
Write-Host ''

# apkscope installer for Windows (PowerShell).
#
# Downloads apkscope.exe from GitHub Releases, verifies its SHA-256 and
# installs it to %LOCALAPPDATA%\Programs\apkscope (added to your user PATH).
#
# Usage:
#   ./scripts/install.ps1                 # latest version
#   ./scripts/install.ps1 -Version 0.9.1  # specific version
#
# Environment overrides (for forks / mirrors):
#   $env:APKSCOPE_REPO = 'suffixing/apkscope'
#   $env:APKSCOPE_BASE_URL = 'https://github.com'
param(
    [string]$Version = ""
)

$ErrorActionPreference = 'Stop'
# Invoke-WebRequest's progress bar slows the download down and clutters the
# output; we print our own status lines instead.
$ProgressPreference = 'SilentlyContinue'

$Repo  = if ($env:APKSCOPE_REPO) { $env:APKSCOPE_REPO } else { 'suffixing/apkscope' }
$Base  = if ($env:APKSCOPE_BASE_URL) { $env:APKSCOPE_BASE_URL } else { 'https://github.com' }
$Dest  = Join-Path $env:LOCALAPPDATA 'Programs\apkscope'
# The release asset is named for the platform (apkscope_windows_x86_64.exe)
# so it is self-describing on the Releases page, but once installed it
# should be callable as `apkscope` from any terminal — so we rename on
# install to a clean binary name.
$Asset         = 'apkscope_windows_x86_64.exe'
$InstalledName = 'apkscope.exe'

# --- output helpers ---------------------------------------------------------------
# Colors go through the console API (Write-Host), never ANSI escapes, so
# redirected output stays clean; NO_COLOR (no-color.org) still disables them.
# Unicode ticks are only used under Windows Terminal or UTF-8 consoles, where
# they are known to render; elsewhere the ASCII fallback keeps things readable.
$interactive = -not [Console]::IsOutputRedirected
$script:Plain = -not $interactive -or [bool]$env:NO_COLOR -or $env:TERM -eq 'dumb'
$script:Tick  = if ($interactive -and $env:TERM -ne 'dumb' -and ($env:WT_SESSION -or [Console]::OutputEncoding.CodePage -eq 65001)) { [char]0x2713 } else { '+' }

function Write-Title($text) {
    if ($script:Plain) { Write-Host "  $text" } else { Write-Host "  $text" -ForegroundColor White }
}
function Write-Info($label, $value) {
    $padded = '{0,-11} ' -f $label
    Write-Host '  ' -NoNewline
    if ($script:Plain) { Write-Host $padded -NoNewline } else { Write-Host $padded -ForegroundColor DarkGray -NoNewline }
    Write-Host $value
}
function Write-Step($msg) { Write-Host "  $msg ..." }
function Write-Ok($msg) {
    Write-Host '  ' -NoNewline
    if ($script:Plain) { Write-Host "$($script:Tick) $msg" }
    else { Write-Host $script:Tick -ForegroundColor Green -NoNewline; Write-Host " $msg" }
}
function Warn($msg) {
    if ($script:Plain) { Write-Host "warning: $msg" }
    else { Write-Host 'warning: ' -ForegroundColor Yellow -NoNewline; Write-Host $msg }
}
function Die($msg) {
    if ($script:Plain) { Write-Host "error: $msg" }
    else { Write-Host 'error: ' -ForegroundColor Red -NoNewline; Write-Host $msg }
    exit 1
}

# The Windows build is x64; Windows on ARM runs it under emulation.
if ($Version -ne '') {
    if ($Version -notlike 'v*') { $Version = "v$Version" }
    $BaseUrl = "$Base/$Repo/releases/download/$Version"
    $VersionLabel = $Version
} else {
    $BaseUrl = "$Base/$Repo/releases/latest/download"
    $VersionLabel = 'latest'
}

Write-Host ''
Write-Title 'apkscope installer'
Write-Host ''
Write-Info 'platform' 'windows x64'
Write-Info 'version' $VersionLabel
Write-Info 'destination' $Dest
Write-Host ''

$tmp = New-Item -ItemType Directory -Path (Join-Path ([System.IO.Path]::GetTempPath()) "apkscope-install-$(Get-Random)")
try {
    Write-Step "downloading $Asset"
    try {
        Invoke-WebRequest -UseBasicParsing -Uri "$BaseUrl/$Asset" -OutFile (Join-Path $tmp $Asset)
    } catch {
        Die "Could not download $Asset (no release for this platform yet? Check $Base/$Repo/releases).`n$($_.Exception.Message)"
    }

    # sanity: GitHub returns an HTML page (not a 4xx) when the asset is missing
    # from a release; reject anything that is clearly not the binary we asked for.
    $downloaded = Join-Path $tmp $Asset
    $bytes = (Get-Item $downloaded).Length
    if ($bytes -lt 1MB) {
        Die "Downloaded $Asset is only $bytes bytes - looks like an error page, not the binary. Check $Base/$Repo/releases."
    }
    $mb = ($bytes / 1MB).ToString('0.0', [System.Globalization.CultureInfo]::InvariantCulture)
    Write-Ok "downloaded ($mb MB)"

    # verify checksum when checksums.txt is published alongside the binary.
    # Only the download + expected-hash extraction is wrapped in try/catch:
    # if checksums.txt is unavailable we proceed with a warning (best-effort).
    # The actual hash comparison is OUTSIDE the try, so a mismatch fails
    # the install rather than being swallowed by the catch.
    $want = $null
    try {
        Invoke-WebRequest -UseBasicParsing -Uri "$BaseUrl/checksums.txt" -OutFile (Join-Path $tmp 'checksums.txt')
        # anchor to end-of-line so a future asset named e.g. "$Asset.sig" can't
        # match the wrong hash line
        $pattern = "$([regex]::Escape($Asset))$"
        $want = (Select-String -Path (Join-Path $tmp 'checksums.txt') -Pattern $pattern |
                 Select-Object -First 1).Line -split '\s+' | Select-Object -First 1
    } catch {
        Warn 'checksums.txt not available, skipping verification'
    }
    if ($want) {
        $got = (Get-FileHash -Algorithm SHA256 -Path $downloaded).Hash.ToLower()
        if ($got -ne $want.ToLower()) {
            Die "SHA-256 mismatch for ${Asset}: expected $want, got $got"
        }
        Write-Ok 'checksum verified (sha256)'
    }

    New-Item -ItemType Directory -Force -Path $Dest | Out-Null
    Move-Item -Force -Path $downloaded -Destination (Join-Path $Dest $InstalledName)
    Write-Ok "installed $(Join-Path $Dest $InstalledName)"

    # add to the user PATH if it is not there yet
    $nowHint = ''
    $userPath = [Environment]::GetEnvironmentVariable('Path', 'User')
    if ($userPath -notlike "*$Dest*") {
        [Environment]::SetEnvironmentVariable('Path', "$userPath;$Dest", 'User')
        Write-Ok 'PATH updated for future terminals'
        $nowHint = 'open a new terminal, then run apkscope'
    } else {
        Write-Ok 'PATH already includes the install directory'
        $nowHint = 'run apkscope'
    }
} finally {
    Remove-Item -Recurse -Force -ErrorAction SilentlyContinue $tmp
}

Write-Host ''
& (Join-Path $Dest $InstalledName) --version
Write-Host ''
Write-Info 'use it now' $nowHint
Write-Info 'learn more' 'apkscope --help'
Write-Host ''

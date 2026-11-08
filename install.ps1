# apkscope installer for Windows (PowerShell).
#
# Downloads apkscope.exe from GitHub Releases, verifies its SHA-256 and
# installs it to %LOCALAPPDATA%\Programs\apkscope (added to your user PATH).
#
# Usage:
#   ./install.ps1                 # latest version
#   ./install.ps1 -Version 0.9.1  # specific version
#
# Environment overrides (for forks / mirrors):
#   $env:APKSCOPE_REPO = 'suffixing/apkscope'
#   $env:APKSCOPE_BASE_URL = 'https://github.com'
param(
    [string]$Version = ""
)

$ErrorActionPreference = 'Stop'

$Repo  = if ($env:APKSCOPE_REPO) { $env:APKSCOPE_REPO } else { 'suffixing/apkscope' }
$Base  = if ($env:APKSCOPE_BASE_URL) { $env:APKSCOPE_BASE_URL } else { 'https://github.com' }
$Dest  = Join-Path $env:LOCALAPPDATA 'Programs\apkscope'
$Asset = 'apkscope_windows_x86_64.exe'

# The Windows build is x64; Windows on ARM runs it under emulation.
if ($Version -ne '') {
    if ($Version -notlike 'v*') { $Version = "v$Version" }
    $BaseUrl = "$Base/$Repo/releases/download/$Version"
    $What = "apkscope $Version"
} else {
    $BaseUrl = "$Base/$Repo/releases/latest/download"
    $What = 'apkscope (latest)'
}

Write-Host "Downloading $What for Windows ..."
$tmp = New-Item -ItemType Directory -Path (Join-Path ([System.IO.Path]::GetTempPath()) "apkscope-install-$(Get-Random)")
try {
    try {
        Invoke-WebRequest -UseBasicParsing -Uri "$BaseUrl/$Asset" -OutFile (Join-Path $tmp $Asset)
    } catch {
        throw "Could not download $Asset (no release for this platform yet? Check $Base/$Repo/releases).`n$($_.Exception.Message)"
    }

    # verify checksum when checksums.txt is published alongside the binary
    try {
        Invoke-WebRequest -UseBasicParsing -Uri "$BaseUrl/checksums.txt" -OutFile (Join-Path $tmp 'checksums.txt')
        $want = (Select-String -Path (Join-Path $tmp 'checksums.txt') -Pattern ([regex]::Escape($Asset)) |
                 Select-Object -First 1).Line -split '\s+' | Select-Object -First 1
        if ($want) {
            $got = (Get-FileHash -Algorithm SHA256 -Path (Join-Path $tmp $Asset)).Hash.ToLower()
            if ($got -ne $want.ToLower()) {
                throw "SHA-256 mismatch for ${Asset}: expected $want, got $got"
            }
            Write-Host "Checksum OK ($($want.ToLower()))"
        }
    } catch {
        Write-Warning "checksums.txt not available, skipping verification"
    }

    New-Item -ItemType Directory -Force -Path $Dest | Out-Null
    Move-Item -Force -Path (Join-Path $tmp $Asset) -Destination (Join-Path $Dest $Asset)

    # add to the user PATH if it is not there yet
    $userPath = [Environment]::GetEnvironmentVariable('Path', 'User')
    if ($userPath -notlike "*$Dest*") {
        [Environment]::SetEnvironmentVariable('Path', "$userPath;$Dest", 'User')
        Write-Host "Added $Dest to your user PATH (restart terminals to pick it up)."
    }

    Write-Host "Installed: $(Join-Path $Dest $Asset)"
    & (Join-Path $Dest $Asset) --version
} finally {
    Remove-Item -Recurse -Force -ErrorAction SilentlyContinue $tmp
}

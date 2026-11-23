# apkscope PR-preview installer for Windows (PowerShell).
#
# Downloads the CI build of a pull request (the workflow artifacts ci.yml
# uploads for every PR), verifies the SHA-256 digest GitHub reports for the
# artifact, and installs it SIDE BY SIDE as `apkscope-pr-<N>.exe` — the real
# install is never touched, and the preview never goes through
# `apkscope upgrade`: per #52, the updater stays limited to signed-off
# stable and canary releases.
#
# Usage:
#   ./scripts/preview-pr.ps1 -Pr 53                # the latest build of PR #53
#   ./scripts/preview-pr.ps1 -Pr 53 -Commit 1a2b3c4
#
# Needs a GitHub login: the artifact download API requires authentication
# even on public repos (GitHub policy). `gh` (logged in) or $env:GITHUB_TOKEN
# is enough; without either, the script prints the Checks-tab link where any
# logged-in browser can download the same zip by hand, and -Zip installs
# from that file.
#
# Environment overrides (for forks / mirrors):
#   $env:APKSCOPE_REPO = 'suffixing/apkscope'
#   $env:APKSCOPE_PREVIEW_ZIP = 'C:\path\to\apkscope-windows-x64.zip'
#   $env:GITHUB_TOKEN
param(
    [Parameter(Mandatory = $true)][string]$Pr,
    [string]$Commit = "",
    [string]$Zip = ""
)

$ErrorActionPreference = 'Stop'
# Invoke-WebRequest's progress bar slows the download down and clutters the
# output; we print our own status lines instead.
$ProgressPreference = 'SilentlyContinue'

$Repo = if ($env:APKSCOPE_REPO) { $env:APKSCOPE_REPO } else { 'suffixing/apkscope' }
$Api  = if ($env:GITHUB_API_URL) { $env:GITHUB_API_URL } else { 'https://api.github.com' }
$Api  = "$Api/repos/$Repo"
$Dest = Join-Path $env:LOCALAPPDATA 'Programs\apkscope'
# The release asset names are self-describing on the Releases page but the
# installed binary should be callable as `apkscope` — here the preview gets
# its own side-by-side name, so the real install is never at risk.
$InstalledName = "apkscope-pr-$Pr.exe"
$Asset = 'apkscope-windows-x64'   # the CI matrix builds Windows x64 only

if (-not $Zip -and $env:APKSCOPE_PREVIEW_ZIP) { $Zip = $env:APKSCOPE_PREVIEW_ZIP }

# --- output helpers (same vocabulary as install.ps1) --------------------------------
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
    # throw, not exit: under `irm ... | iex` exit closes the user's PowerShell window
    throw 'apkscope PR-preview install aborted'
}

# --- GitHub API access ----------------------------------------------------------------
# Listing endpoints work anonymously within IP quota; the artifact download
# needs a login (gh, or $env:GITHUB_TOKEN). Note: `2>$null` on a native
# command can raise under $ErrorActionPreference='Stop' on Windows PowerShell
# 5.1, so every gh call sits in a try/catch with an Invoke-WebRequest
# fallback behind it.
function Get-ApiJson($Path) {
    if ($script:HaveGh) {
        try {
            $out = & gh api "repos/$Repo/$Path" 2>$null
            if ($LASTEXITCODE -eq 0 -and $out) {
                return (($out -join "`n") | ConvertFrom-Json)
            }
        } catch { }
    }
    $headers = @{ Accept = 'application/vnd.github+json' }
    if ($env:GITHUB_TOKEN) { $headers['Authorization'] = "Bearer $($env:GITHUB_TOKEN)" }
    try {
        $resp = Invoke-WebRequest -UseBasicParsing -Uri "$Api/$Path" -Headers $headers
        return ($resp.Content | ConvertFrom-Json)
    } catch {
        return $null
    }
}
function Save-ArtifactZip($ArtifactId, $OutFile) {
    # Invoke-WebRequest -OutFile is binary-safe on every PowerShell; `gh api >`
    # redirection is not on Windows PowerShell 5.1, which re-encodes a native
    # command's stdout as text. So gh only contributes its credential here,
    # never its stdout: the token is read once and used as a header. The
    # request redirects to a signed Azure blob; .NET and PowerShell strip the
    # Authorization header on cross-host redirects (and the blob would refuse
    # a stray one), so the signed URL is fetched anonymously as intended.
    $token = $env:GITHUB_TOKEN
    if (-not $token -and $script:HaveGh) {
        try {
            $token = & gh auth token 2>$null
            if ($LASTEXITCODE -ne 0 -or -not $token) { $token = $null }
        } catch { $token = $null }
    }
    if (-not $token) { return $false }
    try {
        Invoke-WebRequest -UseBasicParsing -Uri "$Api/actions/artifacts/$ArtifactId/zip" `
            -Headers @{ Authorization = "Bearer $token" } -OutFile $OutFile
        return $true
    } catch { return $false }
}
# gh present and logged in? (its own credential flow, no token handling here)
$script:HaveGh = $false
try {
    if (Get-Command gh -ErrorAction SilentlyContinue) {
        & gh auth status *> $null
        if ($LASTEXITCODE -eq 0) { $script:HaveGh = $true }
    }
} catch { }

# --- resolve the PR and its CI run ------------------------------------------------------
# The preview is the ci run of the PR's head commit; pull_request-event runs
# live in the base repo even for fork PRs, which is exactly why their
# artifacts are downloadable by anyone with a GitHub login.
#
# NB: PowerShell variable names are CASE-INSENSITIVE, so the result below must
# NOT be named $pr: that would overwrite the [string]$Pr parameter, whose type
# constraint stringifies the JSON object to '@{...}' (killing .head.sha).
# Also, '?' is a legal variable-name character, so the error message uses
# ${Repo}? - a bare "$Repo?" would look up a variable named 'Repo?' (empty).
Write-Step "resolving PR #$Pr"
$pull = Get-ApiJson "pulls/$Pr"
if (-not $pull -or -not $pull.head.sha) { Die "Could not read PR #$Pr - is it open on github.com/${Repo}?" }
$headSha = $pull.head.sha
$shortSha = $headSha.Substring(0, 7)

# a specific commit was requested: accept it as a full sha or a short prefix
if ($Commit -ne '') {
    if (-not $headSha.StartsWith($Commit, [System.StringComparison]::OrdinalIgnoreCase)) {
        Die "PR #$Pr's latest build is $shortSha, not $Commit - drop -Commit, or update it after pushing."
    }
}

$runs = Get-ApiJson "actions/runs?event=pull_request&head_sha=$headSha"
$run = if ($runs) { $runs.workflow_runs | Where-Object { $_.name -eq 'ci' } | Select-Object -First 1 } else { $null }
if (-not $run) { Die "No ci run found for $shortSha - the PR's checks may not have finished; open https://github.com/$Repo/pull/$Pr/checks" }
$runUrl = $run.html_url

Write-Host ''
Write-Title 'apkscope PR preview'
Write-Host ''
Write-Info 'PR'          "#$Pr (https://github.com/$Repo/pull/$Pr)"
Write-Info 'commit'      $shortSha
Write-Info 'run'         $runUrl
Write-Info 'artifact'    $Asset
Write-Info 'destination' (Join-Path $Dest $InstalledName)
Write-Host ''

$tmp = New-Item -ItemType Directory -Path (Join-Path ([System.IO.Path]::GetTempPath()) "apkscope-preview-$(Get-Random)")
try {
    # --- get the artifact zip ------------------------------------------------------------
    $wantDigest = $null
    if ($Zip -ne '') {
        # a zip downloaded by hand from the PR's Checks tab (no login on this machine)
        if (-not (Test-Path $Zip)) { Die "The -Zip path '$Zip' is not a file." }
        Write-Step "using $Zip"
        Copy-Item $Zip (Join-Path $tmp 'preview.zip')
    } else {
        $arts = Get-ApiJson "actions/runs/$($run.id)/artifacts?per_page=100"
        $artifact = if ($arts) { $arts.artifacts | Where-Object { $_.name -eq $Asset } | Select-Object -First 1 } else { $null }
        if (-not $artifact) {
            if ($arts) {
                Write-Host 'warning: available artifacts on this run:'
                $arts.artifacts | ForEach-Object { Write-Host "    $($_.name)" }
            }
            Die "Artifact '$Asset' not found on run $($run.id)."
        }
        $wantDigest = ($artifact.digest -replace '^sha256:', '')

        Write-Step ("downloading {0} ({1:N1} MB)" -f $Asset, ($artifact.size_in_bytes / 1MB))
        $zipPath = Join-Path $tmp 'preview.zip'
        if (-not (Save-ArtifactZip $artifact.id $zipPath)) {
            $help = @"
error: the artifact download needs a GitHub login
  - gh auth login   (then rerun), or
  - set `$env:GITHUB_TOKEN, or
  - download the zip by hand (logged-in browser): $runUrl
    Checks tab -> $Asset, then:
    ./scripts/preview-pr.ps1 -Pr $Pr -Zip C:\path\to\$Asset.zip
"@
            Write-Host $help -ForegroundColor Red
            throw 'apkscope PR-preview install aborted'
        }
    }
    $zipPath = Join-Path $tmp 'preview.zip'
    $bytes = (Get-Item $zipPath).Length
    if ($bytes -lt 100KB) { Die "Downloaded artifact is only $bytes bytes - not a zip. Re-run, or fetch it from $runUrl." }
    $mb = ($bytes / 1MB).ToString('0.0', [System.Globalization.CultureInfo]::InvariantCulture)
    Write-Ok "downloaded ($mb MB)"

    # --- verify the artifact digest --------------------------------------------------------
    # GitHub reports the SHA-256 of the artifact zip itself; verify what we
    # received against it, like install.ps1 verifies checksums.txt.
    if ($wantDigest) {
        $got = (Get-FileHash -Algorithm SHA256 -Path $zipPath).Hash.ToLower()
        if ($got -ne $wantDigest.ToLower()) {
            Die "SHA-256 mismatch for ${Asset}: expected $wantDigest, got $got"
        }
        Write-Ok 'digest verified (sha256, GitHub''s artifact digest)'
    } else {
        Warn 'hand-provided zip: digest not verified (fetch it from the run''s Checks tab, not from anywhere else)'
    }

    # --- unpack + install side-by-side -----------------------------------------------------
    # The artifact zip holds the bare binary (apkscope.exe).
    Write-Step 'unpacking'
    Expand-Archive -Path $zipPath -DestinationPath (Join-Path $tmp 'unpacked') -Force
    $bin = Get-ChildItem -Path (Join-Path $tmp 'unpacked') -Filter 'apkscope*.exe' | Select-Object -First 1
    if (-not $bin) { $bin = Get-ChildItem -Path (Join-Path $tmp 'unpacked') -Filter 'apkscope*' | Select-Object -First 1 }
    if (-not $bin) { Die 'The artifact zip did not contain a binary.' }

    Write-Step 'installing side-by-side'
    New-Item -ItemType Directory -Force -Path $Dest | Out-Null
    Copy-Item -Force $bin.FullName (Join-Path $Dest $InstalledName)
    Write-Ok "installed $(Join-Path $Dest $InstalledName)"

    $userPath = [Environment]::GetEnvironmentVariable('Path', 'User')
    if ($userPath -notlike "*$Dest*") {
        Write-Ok 'PATH updated for future terminals'
        [Environment]::SetEnvironmentVariable('Path', "$userPath;$Dest", 'User')
    } else {
        Write-Ok 'PATH already includes the install directory'
    }
} finally {
    Remove-Item -Recurse -Force -ErrorAction SilentlyContinue $tmp
}

Write-Host ''
& (Join-Path $Dest $InstalledName) --version
Write-Host ''
Write-Host 'note: this preview is UNREVIEWED PR code, not a release. It replaced nothing:' -ForegroundColor DarkGray
Write-Host "note: the real apkscope is untouched; remove the preview with: Remove-Item '$(Join-Path $Dest $InstalledName)'" -ForegroundColor DarkGray
Write-Host ''
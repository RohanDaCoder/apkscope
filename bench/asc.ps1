# usage: bench\asc.ps1 <rust_ASC.exe> <apkscope.exe> [runs] [-Legacy]
# Same queries through rust_ASC and apkscope's stateless commands; median wall time and peak
# memory of `runs` runs each, as a Markdown table. APKs and classes are listed below.
param([string]$Asc, [string]$Ours, [int]$Runs = 5, [switch]$Legacy)

# 10 popular apps, official builds (publisher-signed, MD5-checked), found through
# https://apkscope.vercel.app/ and downloaded from Aptoide's pool into D:\apks\bench10
$apks = @(
    @{ name = "TikTok";   path = "D:\apks\bench10\com.zhiliaoapp.musically.apk";    class = "com.ss.android.ugc.aweme.app.host.AwemeHostApplication" },
    @{ name = "Facebook"; path = "D:\apks\bench10\com.facebook.katana.apk";          class = "com.facebook.katana.app.FacebookApplication" },
    @{ name = "YouTube";  path = "D:\apks\bench10\com.google.android.youtube.apk";   class = "com.google.android.apps.youtube.app.YouTubeTikTokRoot_Application" },
    @{ name = "WhatsApp"; path = "D:\apks\bench10\com.whatsapp.apk";                 class = "com.whatsapp.AppShell" },
    @{ name = "Telegram"; path = "D:\apks\bench10\org.telegram.messenger.web.apk";   class = "org.telegram.messenger.ApplicationLoaderImpl" },
    @{ name = "Snapchat"; path = "D:\apks\bench10\com.snapchat.android.apk";         class = "com.snapchat.android.shell.AppShell" },
    @{ name = "Netflix";  path = "D:\apks\bench10\com.netflix.mediaclient.apk";      class = "com.netflix.mediaclient.NetflixApplication" },
    @{ name = "Firefox";  path = "D:\apks\bench10\org.mozilla.firefox.apk";          class = "org.mozilla.fenix.FenixApplication" },
    @{ name = "Signal";   path = "D:\apks\bench10\org.thoughtcrime.securesms.apk";   class = "org.thoughtcrime.securesms.ApplicationContext" },
    @{ name = "Duolingo"; path = "D:\apks\bench10\com.duolingo.apk";                 class = "com.duolingo.DuoApp" }
)

# the earlier set (2026-10-30 results): -Legacy
if ($Legacy) { $apks = @(
    @{ name = "Zestle";  path = "D:\apks\Zestle_v0.6.8-prealpha.apk"; class = "com.google.gson.Gson" },
    @{ name = "Unciv";   path = "D:\apks\Unciv.apk";                  class = "com.unciv.UncivGame" },
    @{ name = "Capital"; path = "D:\apks\base.apk";                   class = "okhttp3.OkHttpClient" },
    @{ name = "TikTok";  path = "D:\APKS\tiktok-47-2-41.apk";         class = "com.ss.android.ugc.aweme.app.api.RequestLog" },
    # F-Droid builds, https://f-droid.org/repo/<file>
    @{ name = "KeePassDX";   path = "D:\apks\fdroid\com.kunzisoft.keepass.libre_45600.apk";  class = "com.kunzisoft.keepass.activities.stylish.StylishActivity" },
    @{ name = "Nextcloud";   path = "D:\apks\fdroid\com.nextcloud.client_350000190.apk";     class = "com.nextcloud.client.device.PowerManagementService" },
    @{ name = "AntennaPod";  path = "D:\apks\fdroid\de.danoeh.antennapod_3120395.apk";       class = "de.danoeh.antennapod.activity.MainActivity" },
    @{ name = "Element";     path = "D:\apks\fdroid\im.vector.app_40106624.apk";             class = "im.vector.app.Hilt_VectorApplication" },
    @{ name = "OsmAnd";      path = "D:\apks\fdroid\net.osmand.plus_541003.apk";             class = "net.osmand.plus.routing.RouteService" },
    @{ name = "Thunderbird"; path = "D:\apks\fdroid\net.thunderbird.android_33.apk";         class = "net.thunderbird.app.common.MainActivity" },
    @{ name = "Fennec";      path = "D:\apks\fdroid\org.mozilla.fennec_fdroid_1580220.apk";  class = "org.mozilla.fenix.FenixApplication" },
    @{ name = "NewPipe";     path = "D:\apks\fdroid\org.schabi.newpipe_1015.apk";            class = "org.schabi.newpipe.ExitActivity" },
    @{ name = "VLC";         path = "D:\apks\fdroid\org.videolan.vlc_13070208.apk";          class = "org.videolan.vlc.IDebugLogService" },
    @{ name = "Wikipedia";   path = "D:\apks\fdroid\org.wikipedia_50614.apk";                class = "org.wikipedia.activity.BaseActivity" }
) }

# peak memory read from the OS once the process has exited (no polling: a sleep loop rounds
# every time up to Windows' ~15 ms timer)
Add-Type @"
using System; using System.Runtime.InteropServices;
public static class Mem {
  [StructLayout(LayoutKind.Sequential)] public struct C { public uint cb, faults; public UIntPtr peakWs, ws, a, b, c, d, pagefile, peakPagefile; }
  [DllImport("psapi.dll")] static extern bool GetProcessMemoryInfo(IntPtr h, out C c, uint cb);
  public static ulong Peak(IntPtr h) { C c; GetProcessMemoryInfo(h, out c, (uint)Marshal.SizeOf(typeof(C))); return (ulong)c.peakWs; }
}
"@

function Measure-Run($exe, [string[]]$argv) {
    $si = New-Object Diagnostics.ProcessStartInfo $exe
    $si.Arguments = ($argv | ForEach-Object { '"' + $_ + '"' }) -join ' '
    $si.UseShellExecute = $false
    $si.RedirectStandardOutput = $true
    $si.RedirectStandardError = $true
    $sw = [Diagnostics.Stopwatch]::StartNew()
    $p = [Diagnostics.Process]::Start($si)
    $o = $p.StandardOutput.ReadToEndAsync(); $e = $p.StandardError.ReadToEndAsync()
    $p.WaitForExit()
    $ms = $sw.Elapsed.TotalMilliseconds
    $lines = ($o.Result -split "`n").Count
    $r = @{ ms = $ms; mb = [math]::Round([Mem]::Peak($p.Handle) / 1MB); cpu = $p.TotalProcessorTime.TotalMilliseconds; exit = $p.ExitCode; lines = $lines }
    $p.Dispose()
    $r
}

# both tools take turns, so a slow moment on the laptop hits each of them alike
function Pair($argvA, $argvB) {
    $ra = @(); $rb = @()
    1..$Runs | ForEach-Object { $ra += Measure-Run $Asc $argvA; $rb += Measure-Run $Ours $argvB }
    foreach ($r in @($ra, $rb)) {
        $m = ($r | Sort-Object { $_.ms })[[int][math]::Floor($Runs / 2)]
        $m.mb = ($r | ForEach-Object { $_.mb } | Measure-Object -Maximum).Maximum
        $m
    }
}

"| APK | Query | rust_ASC | apkscope | Speed-up |"
"|---|---|---:|---:|---:|"
foreach ($a in $apks) {
    $p = $a.path
    $tests = @(
        @("String xref ``https://``", @("findrefs", $p, "string", "https://"), @("refs", $p, "string", "https://")),
        @("Method xref ``openConnection``", @("findrefs", $p, "method", "openConnection"), @("refs", $p, "method", "openConnection")),
        @("Decompile one class", @("getclass", $p, $a.class), @("decompile", $p, $a.class)),
        @("List classes", @("listclass", $p), @("classes", $p)),
        @("List methods", @("listmethod", $p), @("methods", $p)),
        @("List strings ``-f http``", @("liststring", "-f", "http", $p), @("strings", $p, "--filter", "http")),
        @("Decode manifest", @("getmanifest", $p), @("manifest", $p))
    )
    foreach ($t in $tests) {
        $x, $y = Pair $t[1] $t[2]
        $ratio = if ($y.ms -gt 0) { "{0:N1}x" -f ($x.ms / $y.ms) } else { "-" }
        $xs = if ($x.exit -eq 0) { "{0:N0} ms / {1:N0} MB" -f $x.ms, $x.mb } else { "failed (exit {0})" -f $x.exit }
        $ys = if ($y.exit -eq 0) { "{0:N0} ms / {1:N0} MB" -f $y.ms, $y.mb } else { "failed (exit {0})" -f $y.exit }
        "| $($a.name) | $($t[0]) | $xs | $ys | $ratio |"
    }
}

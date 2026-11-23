//! `install`: put an APK or a split bundle on a device with the right splits and ABI.
//! A bundle's config splits are matched to the device: the one ABI split for the ABI chosen, and
//! every density and language split (harmless when unused). The ABI is the first one in the
//! device's list that the app ships code for, passed to adb as `--abi`, so an arm64-only app
//! on an x86_64 emulator with ARM translation gets arm64 as its primary ABI instead of failing
//! to find its libraries. The result is read back from the package manager.

use std::io::Read;
use std::process::{Command, Stdio};

fn adb(device: Option<&str>, args: &[&str]) -> Result<String, String> {
    let mut c = Command::new("adb");
    if let Some(d) = device {
        c.args(["-s", d]);
    }
    let out = c
        .args(args)
        .stdin(Stdio::null())
        .output()
        .map_err(|e| format!("cannot run adb ({e}); install Android platform-tools"))?;
    let text = String::from_utf8_lossy(&out.stdout).trim().to_string();
    if !out.status.success() || text.contains("Failure [") {
        let err = String::from_utf8_lossy(&out.stderr).trim().to_string();
        return Err(format!("adb {}: {text} {err}", args.first().unwrap_or(&"")));
    }
    Ok(text)
}

/// ABI of a config split's name (`config.arm64_v8a.apk` -> `arm64-v8a`), if it is one.
pub fn split_abi(name: &str) -> Option<&'static str> {
    let base = name.rsplit('/').next().unwrap_or(name);
    let tag = base.strip_prefix("config.")?.strip_suffix(".apk")?;
    Some(match tag {
        "arm64_v8a" => "arm64-v8a",
        "armeabi_v7a" => "armeabi-v7a",
        "armeabi" => "armeabi",
        "x86_64" => "x86_64",
        "x86" => "x86",
        _ => return None,
    })
}

/// ABIs an APK carries native code for, from its `lib/<abi>/` entries.
fn apk_abis(data: &[u8]) -> Vec<String> {
    let Ok(z) = zip::ZipArchive::new(std::io::Cursor::new(data)) else {
        return Vec::new();
    };
    let mut abis: Vec<String> = z
        .file_names()
        .filter_map(|n| n.ok())
        .filter_map(|n| {
            n.strip_prefix("lib/")?
                .split_once('/')
                .map(|(a, _)| a.to_string())
        })
        .collect();
    abis.sort();
    abis.dedup();
    abis
}

/// The ABI to install with: the first of the device's ABIs (in its order of preference) that the
/// app has code for; `None` when the app has no native code (any ABI works).
pub fn pick_abi(device_abis: &[&str], app_abis: &[String]) -> Result<Option<String>, String> {
    if app_abis.is_empty() {
        return Ok(None);
    }
    device_abis
        .iter()
        .find(|d| app_abis.iter().any(|a| a == *d))
        .map(|d| Some(d.to_string()))
        .ok_or_else(|| {
            format!(
                "the app ships native code for {} only and the device runs {}: it cannot run there (an x86_64 emulator needs ARM translation, which adds arm64-v8a to ro.product.cpu.abilist)",
                app_abis.join(", "),
                device_abis.join(", ")
            )
        })
}

pub fn install(path: &str, device: Option<&str>, abi: Option<&str>) -> Result<String, String> {
    let mut o = Vec::new();
    let list = adb(device, &["shell", "getprop", "ro.product.cpu.abilist"])?;
    let device_abis: Vec<&str> = list
        .split(',')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .collect();
    o.push(format!("device ABIs: {}", device_abis.join(", ")));
    let ext = std::path::Path::new(path)
        .extension()
        .map(|e| e.to_string_lossy().to_lowercase())
        .unwrap_or_default();
    // the APKs to install, as (name, bytes)
    let mut apks: Vec<(String, Vec<u8>)> = Vec::new();
    if matches!(ext.as_str(), "apks" | "xapk" | "apkm") {
        let f = std::fs::File::open(path).map_err(|e| format!("{path}: {e}"))?;
        let mut z = zip::ZipArchive::new(f).map_err(|e| format!("{path}: {e}"))?;
        for i in 0..z.len() {
            let mut e = z.by_index(i).map_err(|e| e.to_string())?;
            let name = e.name().map_err(|e| e.to_string())?.into_owned();
            if !name.ends_with(".apk") {
                continue;
            }
            let mut b = Vec::with_capacity(e.size() as usize);
            e.read_to_end(&mut b).map_err(|e| format!("{name}: {e}"))?;
            apks.push((name, b));
        }
    } else {
        apks.push((
            path.to_string(),
            std::fs::read(path).map_err(|e| format!("{path}: {e}"))?,
        ));
    }
    if apks.is_empty() {
        return Err(format!("{path}: no APK inside"));
    }
    // native code per ABI: ABI splits by name, plus lib/ folders inside any APK
    let mut app_abis: Vec<String> = apks
        .iter()
        .filter_map(|(n, _)| split_abi(n).map(str::to_string))
        .chain(apks.iter().flat_map(|(_, b)| apk_abis(b)))
        .collect();
    app_abis.sort();
    app_abis.dedup();
    o.push(format!(
        "app native code: {}",
        if app_abis.is_empty() {
            "none".to_string()
        } else {
            app_abis.join(", ")
        }
    ));
    let chosen = match abi {
        Some(a) if a != "auto" => Some(a.to_string()),
        _ => pick_abi(&device_abis, &app_abis)?,
    };
    if let Some(a) = &chosen
        && device_abis.first().is_some_and(|d| d != a)
    {
        o.push(format!(
            "note: {a} is not the device's own ABI ({}): the app runs through translation",
            device_abis[0]
        ));
    }
    // one ABI split (the chosen one), all others kept
    apks.retain(|(n, _)| split_abi(n).is_none_or(|s| Some(s) == chosen.as_deref()));
    let dir = std::env::temp_dir().join(format!("apkscope-install-{}", std::process::id()));
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let mut files = Vec::new();
    for (i, (n, b)) in apks.iter().enumerate() {
        let base = n.rsplit(['/', '\\']).next().unwrap_or(n);
        let p = dir.join(format!("{i}-{base}"));
        std::fs::write(&p, b).map_err(|e| e.to_string())?;
        files.push(p.display().to_string());
    }
    let mut args: Vec<&str> = vec![
        if files.len() > 1 {
            "install-multiple"
        } else {
            "install"
        },
        "-r",
    ];
    if let Some(a) = &chosen {
        args.extend(["--abi", a]);
    }
    args.extend(files.iter().map(String::as_str));
    let res = adb(device, &args);
    let _ = std::fs::remove_dir_all(&dir);
    res?;
    let names: Vec<&str> = apks
        .iter()
        .map(|(n, _)| n.rsplit(['/', '\\']).next().unwrap_or(n))
        .collect();
    o.push(format!("installed: {}", names.join(", ")));
    // what the package manager made of it
    let pkg = crate::quick::package(path);
    if !pkg.is_empty()
        && let Ok(dump) = adb(device, &["shell", "dumpsys", "package", &pkg])
        && let Some(l) = dump.lines().find(|l| l.contains("primaryCpuAbi="))
    {
        o.push(format!("{pkg}: {}", l.trim()));
    }
    Ok(o.join("\n"))
}

#[cfg(test)]
#[path = "../tests/apkscope/install.rs"]
mod tests;

//! Getting frida-server onto a device, offline: what the device and this machine are, which
//! frida-server file matches them (named, with its release URL, for the user to fetch), and
//! pushing and starting a file the user downloaded. apkscope itself downloads nothing.

use std::process::{Command, Stdio};

/// Where frida-server goes on the device.
pub const REMOTE: &str = "/data/local/tmp/frida-server";

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
    if !out.status.success() {
        let err = String::from_utf8_lossy(&out.stderr).trim().to_string();
        return Err(format!(
            "adb {}: {}",
            args.join(" "),
            if err.is_empty() { &text } else { &err }
        ));
    }
    Ok(text)
}

/// `adb shell` as root: directly when adbd runs as root (emulators, `adb root`), else through
/// `su -c`, else through `su 0` (the two forms rooted builds take).
fn root_shell(device: Option<&str>, cmd: &str) -> Result<String, String> {
    if adb(device, &["shell", "id", "-u"]).is_ok_and(|u| u == "0") {
        return adb(device, &["shell", cmd]);
    }
    adb(device, &["shell", &format!("su -c '{cmd}'")])
        .or_else(|_| adb(device, &["shell", &format!("su 0 sh -c '{cmd}'")]))
        .map_err(|e| format!("no root shell on the device (needed to run frida-server): {e}"))
}

/// frida's name for a device ABI.
pub fn frida_arch(abi: &str) -> Option<&'static str> {
    Some(match abi {
        "arm64-v8a" => "arm64",
        "armeabi-v7a" | "armeabi" => "arm",
        "x86_64" => "x86_64",
        "x86" => "x86",
        _ => return None,
    })
}

/// The frida-server file for a frida version and device ABI, and its release URL.
pub fn server_file(version: &str, abi: &str) -> Option<(String, String)> {
    let arch = frida_arch(abi)?;
    let file = format!("frida-server-{version}-android-{arch}.xz");
    let url = format!("https://github.com/frida/frida/releases/download/{version}/{file}");
    Some((file, url))
}

fn host_frida() -> Option<String> {
    let out = Command::new("frida")
        .arg("--version")
        .stdin(Stdio::null())
        .output()
        .ok()?;
    out.status
        .success()
        .then(|| String::from_utf8_lossy(&out.stdout).trim().to_string())
        .filter(|v| !v.is_empty())
}

/// What is there and what is missing, as lines; with `push`, the file is pushed, started and
/// checked first.
pub fn setup(device: Option<&str>, push: Option<&str>) -> Result<String, String> {
    let mut o = Vec::new();
    let abi = adb(device, &["shell", "getprop", "ro.product.cpu.abi"])?;
    let abis = adb(device, &["shell", "getprop", "ro.product.cpu.abilist"]).unwrap_or_default();
    let sdk = adb(device, &["shell", "getprop", "ro.build.version.sdk"]).unwrap_or_default();
    o.push(format!("device: {abi} (all ABIs: {abis}), SDK {sdk}"));
    let host = host_frida();
    match &host {
        Some(v) => o.push(format!("this machine: frida {v}")),
        None => o.push("this machine: no frida; install it with: pip install frida-tools".into()),
    }
    if let Some(path) = push {
        if path.ends_with(".xz") {
            return Err(format!(
                "{path} is still compressed: unpack it first (xz -d {path}, or 7-Zip on Windows) and pass the unpacked file"
            ));
        }
        if !std::path::Path::new(path).is_file() {
            return Err(format!("{path}: no such file"));
        }
        adb(device, &["push", path, REMOTE])?;
        root_shell(device, &format!("chmod 755 {REMOTE}"))?;
        // a running one keeps the port: stop it, then start the pushed file detached
        let _ = root_shell(device, "pkill -f frida-server");
        root_shell(device, &format!("{REMOTE} -D >/dev/null 2>&1 &"))?;
        std::thread::sleep(std::time::Duration::from_millis(800));
        o.push(format!("pushed {path} to {REMOTE} and started it"));
    }
    let on_device = root_shell(device, &format!("{REMOTE} --version"))
        .ok()
        .filter(|v| !v.is_empty() && !v.contains("not found"));
    let running = root_shell(device, "pidof frida-server").is_ok_and(|p| !p.trim().is_empty());
    match &on_device {
        Some(v) => o.push(format!(
            "device: frida-server {v} at {REMOTE}, {}",
            if running { "running" } else { "not running" }
        )),
        None => o.push(format!("device: no frida-server at {REMOTE}")),
    }
    let matches = matches!((&host, &on_device), (Some(h), Some(d)) if h == d);
    if matches && running {
        o.push("ready: frida_start (or the viewer's Live tab) can spawn the app".into());
        return Ok(o.join("\n"));
    }
    // what to fetch: the version this machine's frida speaks, for the device's ABI
    match (
        &host,
        server_file(host.as_deref().unwrap_or("<frida --version>"), &abi),
    ) {
        (_, None) => o.push(format!("frida-server has no build for ABI {abi}")),
        (_, Some((file, url))) => {
            if on_device.is_some() && !matches {
                o.push("versions differ: frida needs the same version on both sides".into());
            }
            o.push(format!("download: {file}\n  {url}"));
            o.push(
                "then unpack it (xz -d, or 7-Zip) and run: apkscope frida-server --push <unpacked file>"
                    .into(),
            );
        }
    }
    if abi == "x86_64" || abi == "x86" {
        o.push(format!(
            "note: this device is {abi}. An app that ships arm64-only code runs here only through ARM translation; install it with apkscope install --abi arm64-v8a, or use an arm64 image"
        ));
    }
    Ok(o.join("\n"))
}

#[cfg(test)]
#[path = "../tests/apkscope/frida_setup.rs"]
mod tests;

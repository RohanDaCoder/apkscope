// Embed the icon in the Windows executable (Explorer, taskbar, Alt-Tab).
fn main() {
    println!("cargo:rerun-if-changed=assets/apkscope.rc");
    println!("cargo:rerun-if-changed=assets/apkscope.ico");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        let _ = embed_resource::compile("assets/apkscope.rc", embed_resource::NONE);
        // The viewer's DLLs load only when the viewer opens: loading them at startup cost every
        // CLI and MCP call ~60 ms (75 ms vs 12 ms for `--help`).
        if std::env::var("CARGO_FEATURE_GUI").is_ok() && std::env::var("CARGO_CFG_TARGET_ENV").as_deref() == Ok("msvc") {
            for dll in ["gdi32", "shell32", "ole32", "oleaut32", "opengl32", "dwmapi", "imm32", "uxtheme", "shlwapi", "uiautomationcore"] {
                println!("cargo:rustc-link-arg-bins=/DELAYLOAD:{dll}.dll");
            }
            println!("cargo:rustc-link-arg-bins=delayimp.lib");
        }
    }
}

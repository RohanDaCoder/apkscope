//! dump, disasm, export, gui, and the file-association / no-subcommand fallbacks.

use std::io::Write as _;
use std::process::ExitCode;

use crate::cli::{self, DisasmArgs, DumpArgs, ExportArgs, GuiArgs};
use crate::export;
use crate::live;
use crate::quick;
use crate::session;

pub fn dump(a: &DumpArgs) -> Result<(), String> {
    let attach = a.attach;
    let device = a.device.as_deref();
    let wait = a.wait;
    let more = a.scripts_for;
    let target = if std::path::Path::new(&a.app).is_file() {
        Some(quick::package(&a.app))
            .filter(|p| !p.is_empty())
            .ok_or("no package name in the APK's manifest")?
    } else {
        a.app.clone()
    };
    eprintln!(
        "dumping {target}: waiting {wait} s for it to unpack, then {more} s more for scripts. Use the app meanwhile."
    );
    let files = live::dump(
        &target,
        attach,
        device,
        std::path::Path::new(&a.out),
        wait,
        more,
    )?;
    if !files.iter().any(|f| f.starts_with("dex/")) {
        eprintln!("no DEX found in memory");
    }
    cli::emit_list(&files)
}

pub fn disasm(a: &DisasmArgs) -> Result<(), String> {
    disasm_run(&a.apk, a.class.as_deref())
}

fn disasm_run(path: &str, class: Option<&str>) -> Result<(), String> {
    let mut out = std::io::BufWriter::new(std::io::stdout().lock());
    let mut found = false;
    cli::for_each_dex(path, |_, dex| {
        for c in 0..dex.class_count {
            let name = crate::dex::java_name(&dex.class_descriptor(c)?);
            if class.is_some_and(|want| want != name) {
                continue;
            }
            found = true;
            for m in dex.methods(c)? {
                let _ = writeln!(
                    out,
                    "\n.method {:#x} {}",
                    m.access,
                    dex.method_ref(m.method_idx)?
                );
                let Some(code) = m.code else { continue };
                let _ = writeln!(out, "    .registers {} (ins {})", code.registers, code.ins);
                for i in apkscope_core::bytecode::decode(code.insns)? {
                    let _ = writeln!(
                        out,
                        "    {:04x}: {}",
                        i.off,
                        apkscope_core::dex::render(dex, &i)?
                    );
                }
            }
            if class.is_some() {
                return Ok(true);
            }
        }
        Ok(false)
    })?;
    let _ = out.flush();
    match class {
        Some(c) if !found => Err(format!("class {c} not found")),
        _ => Ok(()),
    }
}

pub fn export_cmd(a: &ExportArgs, deobf: bool) -> Result<(), String> {
    export_run(&a.apk, &a.out, deobf)
}

fn export_run(path: &str, out: &str, deobf: bool) -> Result<(), String> {
    let start = std::time::Instant::now();
    let mut apk = export::open(&session::prepare(path)?)?;
    let root = std::path::Path::new(out);
    let main_dir = root.join("app/src/main");
    let (n, failed) = export::sources(&apk, &main_dir.join("java"), deobf);
    eprintln!(
        "sources: {n} classes ({failed} failed) in {:.1}s",
        start.elapsed().as_secs_f64()
    );
    let t = std::time::Instant::now();
    let (st, manifest) = export::resources(&mut apk, &main_dir)?;
    export::gradle(root, &manifest.unwrap_or_default())?;
    eprintln!(
        "resources: {} values files, {} res files ({} binary XML decoded), {} assets/libs in {:.1}s",
        st.values,
        st.files,
        st.xml_decoded,
        st.other,
        t.elapsed().as_secs_f64()
    );
    eprintln!(
        "done in {:.1}s -> {}",
        start.elapsed().as_secs_f64(),
        root.display()
    );
    Ok(())
}

pub fn gui_cmd(a: &GuiArgs, deobf: bool) -> Result<(), String> {
    crate::gui::run(a.apk.as_deref(), deobf)
}

/// A lone argument that is not a known subcommand: a file associated with apkscope
/// (double-click) opens the viewer; anything else is an unrecognized command.
pub fn external(args: &[String], deobf: bool) -> ExitCode {
    if let [path] = args
        && session::INPUTS
            .iter()
            .any(|e| path.to_lowercase().ends_with(&format!(".{e}")))
    {
        cli::hide_console();
        return super::finish(crate::gui::run(Some(path), deobf));
    }
    cli::usage()
}

/// No subcommand given: the viewer opens (gui build) or the no-viewer note + usage prints.
pub fn no_command(_deobf: bool) -> ExitCode {
    #[cfg(feature = "gui")]
    {
        cli::hide_console();
        super::finish(crate::gui::run(None, _deobf))
    }
    #[cfg(not(feature = "gui"))]
    {
        crate::gui::no_viewer_note();
        eprintln!();
        cli::usage()
    }
}

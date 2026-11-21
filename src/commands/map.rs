//! JADX / Enigma / ProGuard mapping import/export.

use crate::cli::{self, MapArgs};
use crate::re;

pub fn map(a: &MapArgs) -> Result<(), String> {
    let mut n = re::load_notes(&a.apk)?;
    match a.dir.as_str() {
        "export" => {
            let st = cli::load(&a.apk, false)?;
            std::fs::write(
                &a.file,
                re::export_mapping(&st.dexes, &n, re::MappingFormat::from_file(&a.file)),
            )
            .map_err(|e| e.to_string())?;
            eprintln!("wrote {}", a.file);
        }
        "import" => {
            let text = std::fs::read_to_string(&a.file).map_err(|e| format!("{}: {e}", a.file))?;
            let k = re::import_mapping(&text, &mut n);
            re::save_notes(&a.apk, &n)?;
            eprintln!(
                "{k} names imported into {}",
                re::project_file(&a.apk).display()
            );
        }
        _ => return Err("usage: apkscope map <apk> export|import <file>".into()),
    }
    Ok(())
}

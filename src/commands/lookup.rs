//! `consts` and `returns`: constants read straight from the DEX tables (see `crate::lookup`).

use crate::cli::{self, ConstsArgs, ReturnsArgs};
use crate::dex::Dex;
use crate::lookup::{self, NameFilter, Row, ValueFilter};

/// Every DEX of the APK, whole, with the deobfuscator's names when it is on.
pub fn load_dexes(apk: &str, deobf: bool) -> Result<Vec<Dex<'static>>, String> {
    let mut apk = crate::export::Apk::new(crate::input::Input::open(apk)?)?;
    let raw: &'static [Vec<u8>] = Box::leak(
        apk.dex()?
            .into_iter()
            .map(|(_, b)| b)
            .collect::<Vec<_>>()
            .into_boxed_slice(),
    );
    let dexes = raw
        .iter()
        .map(|b| Dex::parse(b).map_err(|e| e.to_string()))
        .collect::<Result<Vec<_>, _>>()?;
    if deobf {
        crate::deobf::init(&dexes);
    }
    Ok(dexes)
}

fn emit_rows(rows: &[Row]) -> Result<(), String> {
    if cli::out_opts().json {
        let o = cli::out_opts();
        let page: Vec<_> = rows
            .iter()
            .skip(o.offset)
            .take(o.limit)
            .map(Row::json)
            .collect();
        if page.len() < rows.len() {
            crate::note!("{} of {} shown (--offset/--limit)", page.len(), rows.len());
        }
        return cli::emit(&format!("{}\n", serde_json::Value::Array(page)));
    }
    cli::emit_list(&rows.iter().map(Row::line).collect::<Vec<_>>())
}

pub fn consts(a: &ConstsArgs, deobf: bool) -> Result<(), String> {
    let t = std::time::Instant::now();
    let dexes = load_dexes(&a.apk, deobf)?;
    let names = NameFilter::new(&a.classes)?;
    let member = a
        .name
        .as_deref()
        .map(|n| {
            regex::RegexBuilder::new(n)
                .case_insensitive(true)
                .build()
                .map_err(|e| e.to_string())
        })
        .transpose()?;
    let rows = lookup::consts(
        &dexes,
        &names,
        &ValueFilter::new(a.value.as_deref()),
        member.as_ref(),
    );
    emit_rows(&rows)?;
    crate::note!(
        "{} constants in {:.0} ms",
        rows.len(),
        t.elapsed().as_secs_f64() * 1000.0
    );
    Ok(())
}

pub fn returns(a: &ReturnsArgs, deobf: bool) -> Result<(), String> {
    let t = std::time::Instant::now();
    let dexes = load_dexes(&a.apk, deobf)?;
    let names = NameFilter::new(&a.r#in)?;
    let impls = a.r#impl.as_deref().map(|i| lookup::implementors(&dexes, i));
    let keep = |desc: &str, class: &str| {
        names.accepts(desc, class) && impls.as_ref().is_none_or(|s| s.contains(desc))
    };
    let rows = lookup::returns(
        &dexes,
        a.method.as_deref(),
        keep,
        &ValueFilter::new(a.value.as_deref()),
    );
    emit_rows(&rows)?;
    crate::note!(
        "{} methods in {:.0} ms",
        rows.len(),
        t.elapsed().as_secs_f64() * 1000.0
    );
    Ok(())
}

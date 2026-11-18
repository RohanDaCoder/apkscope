//! The DEX reader of the core crate, with class names as the user sees them.

pub use apkscope_core::dex::*;

/// `La/b/C;` -> `a.b.C`, under its renamed or deobfuscated name.
pub fn java_name(desc: &str) -> String {
    let desc = crate::deobf::class(desc).unwrap_or(desc);
    desc.strip_prefix('L')
        .and_then(|s| s.strip_suffix(';'))
        .unwrap_or(desc)
        .replace('/', ".")
}

#[cfg(test)]
#[path = "../tests/apkscope/dex.rs"]
pub(crate) mod tests;

//! `apkscope fingerprint` — known-SDK / framework / tracker fingerprinting.
//!
//! Thin handler matching `commands/flutter.rs`: parse the args, call into the
//! `fingerprint` module, emit through the global `--json` / `-o` plumbing. All logic
//! lives in `crate::fingerprint`; this file only resolves the optional APK argument.

use crate::cli::{self, FingerprintArgs};
use crate::fingerprint;

pub fn fingerprint_cmd(a: &FingerprintArgs) -> Result<(), String> {
    let Some(apk) = a.apk.as_deref() else {
        if a.list {
            return cli::emit_list(&fingerprint::list_catalog());
        }
        return Err("fingerprint needs an APK (or --list to print the catalogs)".into());
    };
    fingerprint::fingerprint(a, apk)
}

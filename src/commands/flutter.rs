//! Flutter, scan, decrypt commands.

use crate::cli::{self, DecryptArgs, FlutterArgs, ScanArgs};
use crate::quick;
use crate::re;
use crate::session;

pub fn flutter(a: &FlutterArgs) -> Result<(), String> {
    cli::emit_list(&quick::flutter(&a.apk, a.abi.as_deref(), a.strings, a.api)?)
}

pub fn scan(a: &ScanArgs, deobf: bool) -> Result<(), String> {
    cli::run_big_stack(|| cli::load(&a.apk, deobf).and_then(|st| cli::emit(&re::scan(&st))))
}

pub fn decrypt(a: &DecryptArgs) -> Result<(), String> {
    cli::load(&a.apk, false).and_then(|st| {
        cli::emit_list(
            &re::decrypt_all(&st.dexes)
                .into_iter()
                .map(|(at, f, p)| format!("{at}  {f}  =>  {}", session::json_str(&p)))
                .collect::<Vec<_>>(),
        )
    })
}

//! Reverse-engineering commands: callers, callgraph, access, frida.

use crate::cli;
use crate::cli::{CallgraphArgs, FridaArgs, ReconArgs};
use crate::re;

pub fn callers(a: &ReconArgs) -> Result<(), String> {
    cli::run_big_stack(|| {
        let st = cli::load(&a.apk, false)?;
        let name = a.member.as_deref();
        cli::emit_list(
            &re::callers(&st.dexes, &a.class, name)
                .into_iter()
                .map(|(f, t)| format!("{f}  ->  {t}"))
                .collect::<Vec<_>>(),
        )
    })
}

pub fn callgraph(a: &CallgraphArgs) -> Result<(), String> {
    cli::run_big_stack(|| {
        let st = cli::load(&a.apk, false)?;
        let name = a.member.as_deref();
        cli::emit(&re::call_graph(&st.dexes, &a.class, name, a.depth))
    })
}

pub fn access(a: &ReconArgs) -> Result<(), String> {
    cli::run_big_stack(|| {
        let st = cli::load(&a.apk, false)?;
        let field = a
            .member
            .as_deref()
            .ok_or("usage: apkscope access <apk> <Class> <field>")?;
        cli::emit_list(
            &re::field_access(&st.dexes, &a.class, field)
                .into_iter()
                .map(|(m, f, w)| format!("{}  {m}  ({f})", if w { "write" } else { "read " }))
                .collect::<Vec<_>>(),
        )
    })
}

pub fn frida(a: &FridaArgs) -> Result<(), String> {
    let st = cli::load(&a.apk, false)?;
    cli::emit(&re::frida(
        &st.dexes,
        &a.class,
        a.member.as_deref(),
        a.live,
    )?)
}

//! Unity (il2cpp) command.

use crate::cli::{self, Il2cppArgs};
use crate::session;

pub fn il2cpp(a: &Il2cppArgs) -> Result<(), String> {
    if a.inputs.is_empty() {
        return Err("usage: apkscope il2cpp <apk|xapk>... [--uses TEXT] [--method NAME]".into());
    }
    let g = crate::il2cpp::load(&a.inputs)?;
    let name = |addr: u64| match g.method_at(addr) {
        Some((m, 0)) => format!("0x{:x}  {}  {}", m.addr, m.image, m.name),
        Some((m, n)) => format!(
            "0x{:x}  {}  {}  (+{n} share this code)",
            m.addr, m.image, m.name
        ),
        None => format!("0x{addr:x}  ?"),
    };
    let mut out = Vec::new();
    if let Some(text) = &a.uses {
        let mut hits: std::collections::BTreeMap<usize, std::collections::BTreeSet<u64>> =
            Default::default();
        for (pc, k) in g.literal_uses() {
            if g.literals[k].contains(text) {
                hits.entry(k)
                    .or_default()
                    .insert(g.method_at(pc).map_or(pc, |(m, _)| m.addr));
            }
        }
        for (k, at) in hits {
            out.push(session::json_str(&g.literals[k]));
            out.extend(at.into_iter().map(|a| format!("  {}", name(a))));
        }
    } else if let Some(f) = &a.method {
        for m in g.methods.iter().filter(|m| m.name.contains(f)) {
            out.push(format!("{}  {}  0x{:x}", m.image, m.name, m.addr));
            let (lits, calls) = g.scan(m.addr, g.end_of(m.addr));
            out.extend(lits.iter().map(|&(pc, k)| {
                format!("  0x{pc:x}  string  {}", session::json_str(&g.literals[k]))
            }));
            // direct calls to known method starts; shared bodies show one of the names
            out.extend(
                calls
                    .iter()
                    .filter(|&&(_, t)| g.method_at(t).is_some_and(|(c, _)| c.addr == t))
                    .map(|&(pc, t)| format!("  0x{pc:x}  call    {}", name(t))),
            );
        }
    } else {
        out = g
            .methods
            .iter()
            .map(|m| format!("0x{:x}  {}  {}", m.addr, m.image, m.name))
            .collect();
    }
    cli::emit_list(&out)
}

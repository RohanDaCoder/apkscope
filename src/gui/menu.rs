//! The right-click menu of a code line: open, find usages, hook, rename, comment.

use super::*;

/// Right-click menu for `doc.menu_at`; returns the chosen action.
pub(super) fn line_menu(
    ui: &mut egui::Ui,
    doc: &Doc,
    text: &str,
    st: Option<&State>,
) -> Option<Act> {
    let (at, col) = doc.menu_at?;
    let mut out = None;
    let mut pick = |ui: &mut egui::Ui, label: &str, a: Act| {
        if ui.button(label).clicked() {
            out = Some(a);
        }
    };
    let st = st?;
    if let Open::Report(_) = &doc.open {
        let link = doc
            .links
            .as_ref()
            .and_then(|l| l.get(at).cloned().flatten());
        match link {
            Some(l) => {
                if l.then.is_some() {
                    pick(
                        ui,
                        "Go to the call",
                        Act::GoTo(Link {
                            method: None,
                            ..l.clone()
                        }),
                    );
                }
                if let Some(m) = &l.method {
                    pick(
                        ui,
                        "Open this method",
                        Act::GoTo(Link {
                            at: m.clone(),
                            then: None,
                            method: None,
                        }),
                    );
                    if let Some((c, name)) = m.split_once("->").map(|(c, r)| {
                        (c.to_string(), r.split('(').next().unwrap_or("").to_string())
                    }) {
                        pick(
                            ui,
                            "Its callers",
                            Act::Callers(c.clone(), Some(name.clone())),
                        );
                        pick(ui, "Its call graph", Act::Graph(c, Some(name)));
                    }
                }
            }
            None => {
                ui.label("Click a line with a method to jump there");
            }
        }
        return out;
    }
    let Open::Class(top) = &doc.open else {
        ui.label("Nothing to do here");
        return None;
    };
    if doc.view == View::Smali {
        ui.label("Switch to Java for code actions");
        return None;
    }
    let line = text.lines().nth(at).unwrap_or("");
    // the word under the cursor: a class, a call or a name
    let decl = member_at(text, at);
    if let Some((word, call, is_new, recv)) = word_at(text, at, col) {
        let is_decl_name = decl
            .as_ref()
            .is_some_and(|(c, m)| m.as_ref().map_or(c.last() == Some(&word), |m| m.1 == word));
        if !is_decl_name {
            let class = if word.starts_with(|c: char| c.is_uppercase()) && !call || is_new {
                resolve_class(st, text, top, &word)
            } else {
                None
            };
            if let Some(cls) = &class {
                ui.label(RichText::new(format!("class {word}")).strong());
                pick(
                    ui,
                    "Open class   (Ctrl+click)",
                    Act::OpenClass(cls.clone(), None),
                );
                if let Some(d) = desc_of(st, cls) {
                    pick(
                        ui,
                        &format!("Where {word} is created (new {word})"),
                        Act::Callers(d.clone(), Some("<init>".into())),
                    );
                    pick(
                        ui,
                        &format!("Callers of any {word} method"),
                        Act::Callers(d, None),
                    );
                }
                pick(ui, "Find usages", Act::Usages(word.clone()));
                ui.separator();
            } else if call {
                ui.label(RichText::new(format!("{word}()")).strong());
                let owner = match recv.as_deref() {
                    None | Some("this" | "super") => owner_desc(st, top, &classes_around(text, at)),
                    Some(r) => resolve_class(st, text, top, r).and_then(|c| desc_of(st, &c)),
                };
                let real = owner.as_ref().and_then(|o| {
                    crate::re::real_member(&st.dexes, o, &word, true).map(|n| (o.clone(), n))
                });
                if let Some(a) = definition(st, top, text, at, col) {
                    pick(ui, "Go to definition   (Ctrl+click)", a);
                }
                if let Some((o, n)) = real {
                    pick(ui, "Callers", Act::Callers(o.clone(), Some(n.clone())));
                    pick(ui, "Call graph", Act::Graph(o.clone(), Some(n.clone())));
                    pick(
                        ui,
                        "Hook live (Frida)",
                        Act::LiveHook(o.clone(), Some(n.clone())),
                    );
                    pick(ui, "Frida hook (copy)", Act::Frida(o, Some(n)));
                }
                pick(ui, "Find usages", Act::Usages(word.clone()));
                ui.separator();
            } else if !KEYWORDS.contains(&word.as_str()) {
                ui.label(RichText::new(word.as_str()).strong());
                let owner = owner_desc(st, top, &classes_around(text, at));
                if let Some(o) =
                    owner.filter(|o| crate::re::real_member(&st.dexes, o, &word, false).is_some())
                {
                    let n = crate::re::real_member(&st.dexes, &o, &word, false).unwrap_or_default();
                    pick(ui, "Reads & writes of this field", Act::Access(o, n));
                }
                pick(ui, "Find usages in the app", Act::Usages(word.clone()));
                ui.label(RichText::new("Click a name to mark it everywhere in this file").small());
                ui.separator();
            }
        }
    }
    let Some((chain, member)) = decl else {
        if out.is_none() && word_at(text, at, col).is_none() {
            ui.label("Right-click a name, or a class, method or field declaration");
        }
        return out;
    };
    let owner = owner_desc(st, top, &chain)?;
    let real = member.as_ref().and_then(|(m, n)| {
        crate::re::real_member(&st.dexes, &owner, n, *m).map(|r| (*m, r, n.clone()))
    });
    match &real {
        Some((true, name, shown)) => {
            ui.label(RichText::new(format!("method {shown}()")).strong());
            pick(
                ui,
                "Callers",
                Act::Callers(owner.clone(), Some(name.clone())),
            );
            pick(
                ui,
                "Call graph",
                Act::Graph(owner.clone(), Some(name.clone())),
            );
            pick(
                ui,
                if line.contains("WARNING") {
                    "Show this method in Smali"
                } else {
                    "Show Smali"
                },
                Act::Smali(top.clone(), name.clone()),
            );
            pick(
                ui,
                "Hook live (Frida)",
                Act::LiveHook(owner.clone(), Some(name.clone())),
            );
            pick(
                ui,
                "Frida hook (copy)",
                Act::Frida(owner.clone(), Some(name.clone())),
            );
            ui.separator();
            pick(
                ui,
                "Rename…",
                Act::Note(
                    format!("{owner}->{name}()"),
                    false,
                    format!("Rename method {shown}"),
                ),
            );
            pick(
                ui,
                "Comment…",
                Act::Note(
                    format!("{owner}->{name}()"),
                    true,
                    format!("Comment on {shown}()"),
                ),
            );
        }
        Some((false, name, shown)) => {
            ui.label(RichText::new(format!("field {shown}")).strong());
            pick(
                ui,
                "Reads & writes",
                Act::Access(owner.clone(), name.clone()),
            );
            ui.separator();
            pick(
                ui,
                "Rename…",
                Act::Note(
                    format!("{owner}->{name}"),
                    false,
                    format!("Rename field {shown}"),
                ),
            );
            pick(
                ui,
                "Comment…",
                Act::Note(
                    format!("{owner}->{name}"),
                    true,
                    format!("Comment on {shown}"),
                ),
            );
        }
        None if member.is_some() => {
            ui.label("Not found in the DEX (synthetic or inlined code)");
        }
        None => {
            let shown = chain.last().cloned().unwrap_or_default();
            ui.label(RichText::new(format!("class {shown}")).strong());
            pick(
                ui,
                &format!("Where {shown} is created"),
                Act::Callers(owner.clone(), Some("<init>".into())),
            );
            pick(
                ui,
                "Callers (any method)",
                Act::Callers(owner.clone(), None),
            );
            pick(
                ui,
                "Hook all methods live (Frida)",
                Act::LiveHook(owner.clone(), None),
            );
            pick(
                ui,
                "Frida hooks (all methods, copy)",
                Act::Frida(owner.clone(), None),
            );
            ui.separator();
            pick(
                ui,
                "Rename…",
                Act::Note(
                    owner.clone(),
                    false,
                    format!("Rename class {shown} (a short name keeps the package)"),
                ),
            );
            pick(
                ui,
                "Comment…",
                Act::Note(owner.clone(), true, format!("Comment on class {shown}")),
            );
        }
    }
    out
}

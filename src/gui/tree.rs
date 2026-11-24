//! The class tree (packages folded into nodes) and the resource tree.

use super::*;

/// Package tree of the class list.
#[derive(Default)]
pub(super) struct Tree {
    pub(super) dirs: BTreeMap<String, Tree>,
    /// (short name, full name)
    pub(super) leaves: Vec<(String, String)>,
}

pub(super) fn tree<'a>(classes: impl Iterator<Item = &'a String>) -> Tree {
    let mut root = Tree::default();
    for full in classes {
        let mut node = &mut root;
        let mut parts: Vec<&str> = full.split('.').collect();
        let leaf = parts.pop().unwrap_or(full);
        for p in parts {
            node = node.dirs.entry(p.to_string()).or_default();
        }
        node.leaves.push((leaf.to_string(), full.clone()));
    }
    root
}

/// Resource paths grouped by folder: `res/layout/main.xml` -> "res/layout" -> "main.xml".
pub(super) fn res_groups(st: &State) -> BTreeMap<String, Vec<String>> {
    let mut g: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for p in st.texts().keys().chain(st.files().keys()) {
        let dir = p.rsplit_once('/').map_or("", |d| d.0);
        g.entry(dir.to_string()).or_default().push(p.clone());
    }
    g
}

pub(super) fn node(
    ui: &mut egui::Ui,
    t: &Tree,
    prefix: &str,
    doc: &Option<Open>,
    clicked: &mut Option<String>,
) {
    for (name, sub) in &t.dirs {
        let full = if prefix.is_empty() {
            name.clone()
        } else {
            format!("{prefix}.{name}")
        };
        egui::CollapsingHeader::new(name.as_str())
            .id_salt(&full)
            .show(ui, |ui| node(ui, sub, &full, doc, clicked));
    }
    for (short, full) in &t.leaves {
        if ui
            .selectable_label(doc.as_ref() == Some(&Open::Class(full.clone())), short)
            .clicked()
        {
            *clicked = Some(full.clone());
        }
    }
}

//! The side panel: classes, resources, natives and the outline.

use super::*;

impl App {
    pub(super) fn side_panel(&mut self, ui: &mut egui::Ui) {
        let p = self.set.theme.palette();
        ui.horizontal(|ui| {
            for (s, n) in [
                (Side::Classes, "Classes"),
                (Side::Resources, "Resources"),
                (Side::Strings, "Strings"),
                (Side::Native, "Native"),
                (Side::Files, "Files"),
            ] {
                let sel = self.side == s;
                let t = RichText::new(n)
                    .strong()
                    .color(if sel { p.accent } else { p.dim });
                if ui.add(egui::Button::new(t).frame(false)).clicked() {
                    self.side = s;
                }
            }
        });
        ui.add_space(2.0);
        let Some(st) = self.st else {
            ui.label(RichText::new("No APK open").color(p.dim));
            return;
        };
        if self.side == Side::Files {
            let files = self.files.get_or_insert_with(|| st.files_list());
            let cur = self.docs.get(self.active).map(|d| d.open.clone());
            ui.horizontal(|ui| {
                ui.label(RichText::new(format!("{} files", files.len())).color(p.dim));
                if ui
                    .button("Extract all…")
                    .on_hover_text("Write every file of the APK to a folder")
                    .clicked()
                    && let Some(dir) = rfd::FileDialog::new().pick_folder()
                {
                    let tx = self.tx.clone();
                    spawn(&self.tx, "save", move || {
                        let r = st
                            .extract_all(&dir)
                            .map(|n| format!("Extracted {n} files to {}", dir.display()));
                        let _ = tx.send(Msg::Saved(r));
                    });
                }
                if ui
                    .button("Decode resources…")
                    .on_hover_text(
                        "Write the manifest and res/ as editable files, like `apkscope res`",
                    )
                    .clicked()
                    && let Some(dir) = rfd::FileDialog::new().pick_folder()
                {
                    let (tx, file) = (self.tx.clone(), st.file());
                    spawn(&self.tx, "save", move || {
                        let r = (|| -> Result<String, String> {
                            let mut apk = apkscope_core::apk::open(&file)?;
                            let files = apkscope_core::resources::decode(&mut apk)?;
                            for (path, data) in &files {
                                let to = std::path::Path::new(&dir).join(path);
                                if let Some(parent) = to.parent() {
                                    std::fs::create_dir_all(parent)
                                        .map_err(|e| format!("{}: {e}", parent.display()))?;
                                }
                                std::fs::write(&to, data)
                                    .map_err(|e| format!("{}: {e}", to.display()))?;
                            }
                            Ok(format!(
                                "Decoded {} resource file(s) to {}",
                                files.len(),
                                dir.display()
                            ))
                        })();
                        let _ = tx.send(Msg::Saved(r));
                    });
                }
            });
            let mut verify = false;
            ui.horizontal(|ui| {
                if ui
                    .button("Add file…")
                    .on_hover_text("Put a new file into the APK when you build")
                    .clicked()
                    && let Some(f) = rfd::FileDialog::new().pick_file() {
                        match std::fs::read(&f) {
                            Ok(b) => {
                                let name = f
                                    .file_name()
                                    .map_or("file".into(), |n| n.to_string_lossy().into_owned());
                                self.adding = Some((format!("assets/{name}"), b));
                            }
                            Err(e) => {
                                self.toast = Some((
                                    format!("Can't read {}: {e}", f.display()),
                                    std::time::Instant::now(),
                                ))
                            }
                        }
                    }
                if ui
                    .button("Resources folder…")
                    .on_hover_text(
                        "Apply a folder of edited resources (from Decode resources) on build, like `build --res`",
                    )
                    .clicked()
                    && let Some(dir) = rfd::FileDialog::new().pick_folder() {
                        self.res_dir = Some(dir.display().to_string());
                        self.toast = Some((
                            "Resources folder staged — use Build & sign to apply it".into(),
                            std::time::Instant::now(),
                        ));
                    }
                let n = self.edits.len() + self.smali_edits.len() + self.res_dir.is_some() as usize;
                let build = ui.add_enabled(
                    n > 0,
                    egui::Button::new(RichText::new(format!("Build & sign ({n})")).strong())
                        .fill(p.accent.gamma_multiply(0.55)),
                );
                if build
                    .on_hover_text(
                        "Write a new APK with your changes, zipaligned and signed (v2 + v3)",
                    )
                    .clicked()
                {
                    self.build = Some(BuildDlg {
                        own_key: false,
                        key: String::new(),
                        cert: String::new(),
                    });
                }
                verify = ui
                    .button("Verify signature")
                    .on_hover_text("Check this APK's v2/v3 signatures")
                    .clicked();
            });
            if verify {
                let (tx, file) = (self.tx.clone(), st.file());
                spawn(&self.tx, "save", move || {
                    let r = apksign::verify(&file).map(|l| l.join(", "));
                    let _ = tx.send(Msg::Saved(
                        r.map(|l| format!("Signature: {l}")).map_err(String::from),
                    ));
                });
            }
            let mut clear_res = false;
            if let Some(dir) = self.res_dir.clone() {
                ui.horizontal(|ui| {
                    if ui.small_button("clear").clicked() {
                        clear_res = true;
                    }
                    ui.label(RichText::new(format!("resources: {dir}")).color(p.accent));
                });
            }
            if clear_res {
                self.res_dir = None;
            }
            if !self.edits.is_empty() {
                let mut undo = None;
                egui::CollapsingHeader::new(
                    RichText::new(format!("Changes ({})", self.edits.len()))
                        .strong()
                        .color(p.accent),
                )
                .id_salt("changes")
                .default_open(true)
                .show(ui, |ui| {
                    for (name, new) in &self.edits {
                        ui.horizontal(|ui| {
                            let what = match new {
                                Some(b) if files.iter().any(|f| &f.0 == name) => {
                                    format!("replace  {name}  ({})", mb(b.len() as u64))
                                }
                                Some(b) => format!("add  {name}  ({})", mb(b.len() as u64)),
                                None => format!("delete  {name}"),
                            };
                            if ui.small_button("undo").clicked() {
                                undo = Some(name.clone());
                            }
                            ui.label(what);
                        });
                    }
                });
                if let Some(n) = undo {
                    self.edits.remove(&n);
                }
            }
            edit(ui, &mut self.filter, "Filter files…", f32::INFINITY);
            ui.separator();
            let filter = self.filter.to_lowercase();
            let mut go = None;
            egui::ScrollArea::vertical()
                .auto_shrink(false)
                .show(ui, |ui| {
                    let mut dirs: BTreeMap<&str, Vec<&(String, u64, u64)>> = BTreeMap::new();
                    for f in files
                        .iter()
                        .filter(|f| filter.is_empty() || f.0.to_lowercase().contains(&filter))
                    {
                        dirs.entry(f.0.rsplit_once('/').map_or("", |x| x.0))
                            .or_default()
                            .push(f);
                    }
                    for (dir, list) in dirs {
                        let mut row = |ui: &mut egui::Ui, f: &(String, u64, u64)| {
                            let name = f.0.rsplit('/').next().unwrap_or(&f.0);
                            let open = if f.0.ends_with(".so") {
                                Open::Native(f.0.clone())
                            } else {
                                Open::File(f.0.clone())
                            };
                            // pending changes show in the list too
                            let mark = match self.edits.get(&f.0) {
                                Some(Some(_)) => "  (replaced)",
                                Some(None) => "  (deleted)",
                                None => "",
                            };
                            if ui
                                .selectable_label(
                                    cur.as_ref() == Some(&open),
                                    format!("{name}   {}{mark}", mb(f.1)),
                                )
                                .on_hover_text(format!(
                                    "{}\n{} bytes, {} compressed",
                                    f.0, f.1, f.2
                                ))
                                .clicked()
                            {
                                go = Some(open);
                            }
                        };
                        if dir.is_empty() {
                            list.into_iter().for_each(|f| row(ui, f));
                        } else {
                            egui::CollapsingHeader::new(format!("{dir}  ({})", list.len()))
                                .id_salt(("files", dir))
                                .default_open(!filter.is_empty())
                                .show(ui, |ui| list.into_iter().for_each(|f| row(ui, f)));
                        }
                    }
                });
            if let Some(o) = go {
                self.open(o, None, None);
            }
            return;
        }
        if self.side == Side::Native {
            let libs = self.natives.get_or_insert_with(|| st.native_libs());
            let cur = self.docs.get(self.active).map(|d| d.open.clone());
            ui.label(RichText::new("Native libraries (.so) per CPU architecture. Open one for its JNI methods, exports, dependencies and strings.").small().color(p.dim));
            ui.separator();
            if libs.is_empty() {
                ui.label(RichText::new("This APK has no native libraries.").color(p.dim));
                return;
            }
            let mut go = None;
            egui::ScrollArea::vertical()
                .auto_shrink(false)
                .show(ui, |ui| {
                    let mut abis: Vec<&str> = libs
                        .iter()
                        .map(|l| l.0.split('/').nth(1).unwrap_or(""))
                        .collect();
                    abis.dedup();
                    for abi in abis {
                        let mine: Vec<&(String, u64)> = libs
                            .iter()
                            .filter(|l| l.0.split('/').nth(1) == Some(abi))
                            .collect();
                        let total: u64 = mine.iter().map(|l| l.1).sum();
                        egui::CollapsingHeader::new(
                            RichText::new(format!("{abi}  ({}, {})", mine.len(), mb(total)))
                                .strong(),
                        )
                        .id_salt(("abi", abi))
                        .default_open(abi == "arm64-v8a")
                        .show(ui, |ui| {
                            for (path, size) in mine {
                                let name = path.rsplit('/').next().unwrap_or(path);
                                if ui
                                    .selectable_label(
                                        cur.as_ref() == Some(&Open::Native(path.clone())),
                                        format!("{name}   {}", mb(*size)),
                                    )
                                    .clicked()
                                {
                                    go = Some(path.clone());
                                }
                            }
                        });
                    }
                });
            if let Some(path) = go {
                self.open(Open::Native(path), None, None);
            }
            return;
        }
        if self.side == Side::Strings {
            edit(ui, &mut self.strings_q, "Filter strings…", f32::INFINITY);
            // list everything at first, then refilter as the user types (stale replies are dropped)
            // the index adds usage counts: list again once it's ready
            let key = format!("{}\0{}", self.strings_q, st.index.get().is_some());
            if self.strings_for.as_deref() != Some(key.as_str()) {
                self.strings_for = Some(key.clone());
                let (tx, q) = (self.tx.clone(), self.strings_q.clone());
                spawn(&self.tx, "strings", move || {
                    let v = st.strings(&q, session::LIMIT);
                    let _ = tx.send(Msg::Strings(key, v));
                });
            }
            let more = if self.strings.len() >= session::LIMIT {
                format!("first {}", session::LIMIT)
            } else {
                self.strings.len().to_string()
            };
            ui.label(
                RichText::new(format!(
                    "{more} strings{}. Click one to find where it's used.",
                    if st.index.get().is_some() {
                        " (number = classes using it)"
                    } else {
                        ""
                    }
                ))
                .small()
                .color(p.dim),
            );
            ui.separator();
            let mut go = None;
            let row = ui.text_style_height(&egui::TextStyle::Body) + 6.0;
            egui::ScrollArea::vertical().auto_shrink(false).show_rows(
                ui,
                row,
                self.strings.len(),
                |ui, rows| {
                    for (s, n) in &self.strings[rows] {
                        let shown: String = s.chars().take(120).collect();
                        let label = if st.index.get().is_some() {
                            format!("{n:>4}   {shown}")
                        } else {
                            shown
                        };
                        let r = ui
                            .add(
                                egui::Label::new(label)
                                    .truncate()
                                    .sense(egui::Sense::click()),
                            )
                            .on_hover_cursor(egui::CursorIcon::PointingHand);
                        if r.on_hover_text(s).clicked() {
                            go = Some(s.clone());
                        }
                    }
                },
            );
            if let Some(s) = go {
                self.usages_of(&s);
            }
            return;
        }
        let hint = if self.side == Side::Classes {
            "Filter classes…"
        } else {
            "Filter resources…"
        };
        edit(ui, &mut self.filter, hint, f32::INFINITY);
        if self.side == Side::Resources
            && ui
                .button("Edit strings…")
                .on_hover_text("Change string resources (app name, texts, URLs in strings.xml)")
                .clicked()
        {
            let base = match self.edits.get("resources.arsc") {
                Some(Some(b)) => Some(b.clone()),
                _ => st.read_entry("resources.arsc"),
            };
            match base.map(|b| arsc::strings(&b).map(|all| (b, all))) {
                Some(Ok((base, all))) => {
                    self.strings_dlg = Some(StringsDlg {
                        base,
                        all,
                        config: String::new(),
                        filter: String::new(),
                        changes: Default::default(),
                    })
                }
                Some(Err(e)) => {
                    self.toast = Some((
                        format!("Can't edit strings: {e}"),
                        std::time::Instant::now(),
                    ))
                }
                None => {
                    self.toast = Some((
                        "This APK has no resources.arsc".into(),
                        std::time::Instant::now(),
                    ))
                }
            }
        }
        if self.side == Side::Classes {
            ui.checkbox(&mut self.show_libs, format!("Show libraries ({})", self.counts[2])).on_hover_text("androidx, Kotlin, Google Play services, OkHttp, ad SDKs... They stay searchable either way.");
        }
        ui.separator();
        let cur = self.docs.get(self.active).map(|d| d.open.clone());
        let filter = self.filter.to_lowercase();
        let mut go = None;
        egui::ScrollArea::vertical()
            .auto_shrink(false)
            .show(ui, |ui| match self.side {
                Side::Classes if filter.len() >= 2 => {
                    let shown = st.classes.iter().zip(&st.origin).filter(|(c, o)| {
                        (self.show_libs || **o != Origin::Library)
                            && c.0.to_lowercase().contains(&filter)
                    });
                    for ((n, _, _), _) in shown.take(session::LIMIT) {
                        if ui
                            .selectable_label(cur.as_ref() == Some(&Open::Class(n.clone())), n)
                            .clicked()
                        {
                            go = Some(Open::Class(n.clone()));
                        }
                    }
                }
                Side::Classes => {
                    if ui
                        .selectable_label(
                            cur.as_ref() == Some(&Open::Overview),
                            RichText::new("Overview").strong(),
                        )
                        .clicked()
                    {
                        go = Some(Open::Overview);
                    }
                    let mut clicked = None;
                    let names = [
                        format!("App code · {}", st.package),
                        "Other code (unknown or obfuscated packages)".to_string(),
                        "Libraries & SDKs".to_string(),
                    ];
                    for (k, name) in names.iter().enumerate() {
                        if self.counts[k] == 0 || (k == 2 && !self.show_libs) {
                            continue;
                        }
                        let head = RichText::new(format!("{name}  ({})", self.counts[k]))
                            .strong()
                            .color(if k == 0 { p.accent } else { p.text });
                        egui::CollapsingHeader::new(head)
                            .id_salt(("origin", k))
                            .default_open(k < 2)
                            .show(ui, |ui| node(ui, &self.trees[k], "", &cur, &mut clicked));
                    }
                    if self.counts[2] > 0 && !self.show_libs {
                        ui.label(
                            RichText::new(format!("{} library classes hidden", self.counts[2]))
                                .small()
                                .color(p.dim),
                        );
                    }
                    if clicked.is_some() {
                        go = clicked.map(Open::Class);
                    }
                }
                _ => {
                    for (dir, paths) in &self.res {
                        let shown: Vec<&String> = paths
                            .iter()
                            .filter(|p| filter.is_empty() || p.to_lowercase().contains(&filter))
                            .collect();
                        if shown.is_empty() {
                            continue;
                        }
                        let mut leaf = |ui: &mut egui::Ui, p: &String| {
                            let name = p.rsplit('/').next().unwrap_or(p);
                            if ui
                                .selectable_label(cur.as_ref() == Some(&Open::Res(p.clone())), name)
                                .clicked()
                            {
                                go = Some(Open::Res(p.clone()));
                            }
                        };
                        if dir.is_empty() {
                            shown.into_iter().for_each(|p| leaf(ui, p));
                        } else {
                            egui::CollapsingHeader::new(format!("{dir}  ({})", shown.len()))
                                .id_salt(dir)
                                .default_open(!filter.is_empty())
                                .show(ui, |ui| shown.into_iter().for_each(|p| leaf(ui, p)));
                        }
                    }
                }
            });
        if let Some(o) = go {
            self.open(o, None, None);
        }
    }
}

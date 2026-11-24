//! Windows over the main view: patching and building, export, the start screen and settings.

use super::*;

impl App {
    /// "Add file", "Edit strings" and "Build & sign" windows.
    pub(super) fn edit_windows(&mut self, ctx: &egui::Context) {
        let p = self.set.theme.palette();
        if let Some(dlg) = &mut self.strings_dlg {
            let (mut open, mut apply) = (true, false);
            egui::Window::new("Edit strings")
                .collapsible(false)
                .default_size([720.0, 520.0])
                .open(&mut open)
                .show(ctx, |ui| {
                    let mut configs: Vec<&str> =
                        dlg.all.iter().map(|r| r.config.as_str()).collect();
                    configs.sort();
                    configs.dedup();
                    ui.horizontal(|ui| {
                        egui::ComboBox::from_id_salt("str-config")
                            .selected_text(if dlg.config.is_empty() {
                                "default"
                            } else {
                                &dlg.config
                            })
                            .show_ui(ui, |ui| {
                                for c in &configs {
                                    ui.selectable_value(
                                        &mut dlg.config,
                                        c.to_string(),
                                        if c.is_empty() { "default" } else { c },
                                    );
                                }
                            });
                        edit(ui, &mut dlg.filter, "Filter by name or text…", 260.0);
                        ui.label(
                            RichText::new(format!("{} changed", dlg.changes.len())).color(p.dim),
                        );
                    });
                    ui.separator();
                    let f = dlg.filter.to_lowercase();
                    let rows: Vec<usize> = dlg
                        .all
                        .iter()
                        .enumerate()
                        .filter(|(_, r)| {
                            r.config == dlg.config
                                && (f.is_empty()
                                    || r.name.to_lowercase().contains(&f)
                                    || r.value.to_lowercase().contains(&f))
                        })
                        .map(|x| x.0)
                        .collect();
                    let h = ui.text_style_height(&egui::TextStyle::Body) + 10.0;
                    egui::ScrollArea::vertical()
                        .max_height(400.0)
                        .auto_shrink(false)
                        .show_rows(ui, h, rows.len(), |ui, range| {
                            for &k in &rows[range] {
                                let r = &dlg.all[k];
                                let key = (r.config.clone(), r.name.clone());
                                let mut text = dlg
                                    .changes
                                    .get(&key)
                                    .cloned()
                                    .unwrap_or_else(|| r.value.clone());
                                ui.horizontal(|ui| {
                                    let changed = dlg.changes.contains_key(&key);
                                    ui.allocate_ui_with_layout(
                                        egui::vec2(220.0, h - 6.0),
                                        egui::Layout::left_to_right(egui::Align::Center),
                                        |ui| {
                                            ui.set_width(220.0);
                                            ui.add(
                                                egui::Label::new(
                                                    RichText::new(&r.name).monospace().color(
                                                        if changed { p.accent } else { p.text },
                                                    ),
                                                )
                                                .truncate(),
                                            )
                                            .on_hover_text(&r.name);
                                        },
                                    );
                                    if ui
                                        .add(
                                            egui::TextEdit::singleline(&mut text)
                                                .desired_width(f32::INFINITY),
                                        )
                                        .changed()
                                    {
                                        if text == r.value {
                                            dlg.changes.remove(&key);
                                        } else {
                                            dlg.changes.insert(key.clone(), text.clone());
                                        }
                                    }
                                });
                            }
                        });
                    ui.separator();
                    apply = ui
                        .add_enabled(
                            !dlg.changes.is_empty(),
                            egui::Button::new(RichText::new("Apply").strong())
                                .fill(p.accent.gamma_multiply(0.55)),
                        )
                        .clicked();
                });
            if apply {
                let dlg = self.strings_dlg.take().unwrap();
                match arsc::set_strings(&dlg.base, &dlg.changes) {
                    Ok(bytes) => {
                        self.edits.insert("resources.arsc".into(), Some(bytes));
                        self.toast(format!(
                            "{} string(s) will change when you build (Files tab)",
                            dlg.changes.len()
                        ));
                    }
                    Err(e) => self.toast(format!("Strings not applied: {e}")),
                }
            } else if !open {
                self.strings_dlg = None;
            }
        }
        if let Some((path, data)) = &mut self.adding {
            let (mut open, mut add) = (true, false);
            egui::Window::new("Add file")
                .collapsible(false)
                .resizable(false)
                .open(&mut open)
                .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
                .show(ctx, |ui| {
                    ui.label(format!(
                        "{} to add. Path inside the APK:",
                        mb(data.len() as u64)
                    ));
                    ui.add(egui::TextEdit::singleline(path).desired_width(360.0));
                    ui.label(
                        RichText::new(
                            "assets/ is safe for any file; lib/<abi>/ for native libraries.",
                        )
                        .small()
                        .color(p.dim),
                    );
                    add = ui
                        .add_enabled(
                            !path.trim().is_empty() && !path.contains(".."),
                            egui::Button::new("Add"),
                        )
                        .clicked();
                });
            if add {
                let (path, data) = self.adding.take().unwrap();
                let path = path.trim().trim_start_matches('/').replace('\\', "/");
                self.toast(format!("{path} will be added when you build"));
                self.edits.insert(path, Some(data));
            } else if !open {
                self.adding = None;
            }
        }
        let Some(dlg) = &mut self.build else { return };
        let Some(st) = self.st else { return };
        let (mut open, mut go) = (true, false);
        egui::Window::new("Build & sign").collapsible(false).resizable(false).open(&mut open).anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0]).show(ctx, |ui| {
            let smali_n = self.smali_edits.len();
            let smali_note = if smali_n > 0 { format!(" (incl. {smali_n} edited smali class(es), assembled on build)") } else { String::new() };
            ui.label(format!("{} change(s){smali_note}. The new APK is zipaligned and signed with APK Signature Scheme v2 + v3{}.", self.edits.len() + smali_n, match st.min_sdk() {
                Some(m) if m >= 24 => String::new(),
                Some(m) => format!(", and v1 (JAR) for Android 5-6 (minSdk {m})"),
                None => ", and v1 (JAR): the app sets no minSdk".into(),
            }));
            if let Some(dir) = &self.res_dir {
                ui.label(format!("Applying edited resources from {dir}."));
            }
            if let Some(dir) = &self.smali_dir {
                ui.label(format!("Applying the changed classes of the smali folder {dir}."));
            }
            ui.add_space(6.0);
            ui.radio_value(&mut dlg.own_key, false, "apkscope's key (created on first use)");
            ui.radio_value(&mut dlg.own_key, true, "My own key");
            if dlg.own_key {
                for (label, field, ext) in [("Private key (PKCS#8 .pk8 / .pem)", &mut dlg.key, ["pk8", "pem", "der"]), ("Certificate (.crt / .pem / .der)", &mut dlg.cert, ["crt", "pem", "der"])] {
                    ui.horizontal(|ui| {
                        ui.label(label);
                        if ui.button("Choose…").clicked()
                            && let Some(f) = rfd::FileDialog::new().add_filter(label, &ext).pick_file() {
                                *field = f.display().to_string();
                            }
                    });
                    if !field.is_empty() {
                        ui.label(RichText::new(field.as_str()).small().color(p.dim));
                    }
                }
            }
            ui.add_space(6.0);
            let warn = Color32::from_rgb(230, 160, 60);
            ui.colored_label(warn, "A different signature means it won't install as an update over the original app: uninstall that first.");
            ui.add_space(6.0);
            let ready = !dlg.own_key || (!dlg.key.is_empty() && !dlg.cert.is_empty());
            go = ui.add_enabled(ready, egui::Button::new(RichText::new("Build…").strong()).fill(p.accent.gamma_multiply(0.55))).clicked();
        });
        if go {
            let stem = std::path::Path::new(&st.path)
                .file_stem()
                .map_or("app".into(), |s| s.to_string_lossy().into_owned());
            if let Some(out) = rfd::FileDialog::new()
                .set_file_name(format!("{stem}-apkscope.apk"))
                .add_filter("APK", &["apk"])
                .save_file()
            {
                let own = dlg.own_key.then(|| (dlg.key.clone(), dlg.cert.clone()));
                let (tx, mut edits, file) = (self.tx.clone(), self.edits.clone(), st.file());
                let mut smali_srcs = self.smali_edits.clone();
                let (res_dir, smali_dir) = (self.res_dir.clone(), self.smali_dir.clone());
                spawn(&self.tx, "save", move || {
                    let r = (|| {
                        let key = match own {
                            Some((k, c)) => {
                                apksign::load(std::path::Path::new(&k), std::path::Path::new(&c))?
                            }
                            None => apksign::default_key()?.0,
                        };
                        if let Some(dir) = &res_dir {
                            // everything Decode resources wrote: a values file that is gone has no values
                            let files = crate::read_tree(std::path::Path::new(dir))?;
                            let mut apk = apkscope_core::apk::open(&file)?;
                            edits.extend(apkscope_core::resources::edits(&mut apk, &files, true)?);
                        }
                        if let Some(dir) = smali_dir {
                            smali_srcs.extend(crate::commands::edit::smali_sources(&file, &[dir])?);
                        }
                        if !smali_srcs.is_empty() {
                            smali::inject(&file, &smali_srcs, &mut edits)?;
                        }
                        let size = crate::export::write_apk(&file, &edits, &out, Some(&key))?;
                        let check = apksign::verify(&out.display().to_string())?;
                        Ok(format!(
                            "Built {} ({}). {}",
                            out.display(),
                            mb(size),
                            check.join(", ")
                        ))
                    })();
                    let _ = tx.send(Msg::Saved(r));
                });
                self.toast("Building…");
            }
            self.build = None;
        } else if !open {
            self.build = None;
        }
    }

    pub(super) fn export_window(&mut self, ctx: &egui::Context) {
        let Some(dlg) = &mut self.export else { return };
        let mut open = true;
        let mut save = false;
        egui::Window::new("Export results")
            .collapsible(false)
            .resizable(false)
            .open(&mut open)
            .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
            .show(ctx, |ui| {
                ui.label(format!("{} hits from {}", self.hits.len(), self.hits_title));
                ui.add_space(4.0);
                ui.horizontal(|ui| {
                    ui.label("Format");
                    ui.radio_value(&mut dlg.fmt, Format::Text, "Text");
                    ui.radio_value(&mut dlg.fmt, Format::Csv, "CSV");
                    ui.radio_value(&mut dlg.fmt, Format::Json, "JSON");
                });
                ui.checkbox(&mut dlg.method, "Whole enclosing method for each hit");
                ui.add_enabled_ui(!dlg.method, |ui| {
                    ui.add(
                        egui::Slider::new(&mut dlg.ctx, 0..=30)
                            .text("lines of context around each hit"),
                    );
                });
                ui.add_space(6.0);
                save = ui.button("Save…").clicked();
            });
        if save {
            let (fmt, ctxl, method) = (dlg.fmt, dlg.ctx, dlg.method);
            let ext = match fmt {
                Format::Text => "txt",
                Format::Csv => "csv",
                Format::Json => "json",
            };
            if let Some(file) = rfd::FileDialog::new()
                .set_file_name(format!("apkscope-results.{ext}"))
                .add_filter(ext, &[ext])
                .save_file()
            {
                let (tx, hits, st) = (self.tx.clone(), self.hits.clone(), self.st);
                spawn(&self.tx, "save", move || {
                    let Some(st) = st else { return };
                    let out = session::export(&hits, ctxl, method, fmt, |t| st.target_text(t));
                    let r = std::fs::write(&file, out)
                        .map(|_| format!("Saved {} hits to {}", hits.len(), file.display()))
                        .map_err(|e| e.to_string());
                    let _ = tx.send(Msg::Saved(r));
                });
                self.set.export_ctx = ctxl;
                self.set.export_method = method;
                self.set.save();
            }
            self.export = None;
        } else if !open {
            self.export = None;
        }
    }

    /// Before an APK is open: logo, a drop zone and what apkscope accepts.
    pub(super) fn start_screen(&mut self, ui: &mut egui::Ui, p: &Palette) {
        let logo = self.logo.get_or_insert_with(|| {
            let img = egui::ColorImage::from_rgba_unmultiplied(
                [64, 64],
                include_bytes!("../../assets/icon64.rgba"),
            );
            ui.ctx()
                .load_texture("logo", img, egui::TextureOptions::LINEAR)
        });
        let dragging = ui.ctx().input(|i| !i.raw.hovered_files.is_empty());
        let loading = !self.path.is_empty() && self.err.is_none();
        let mut pick = false;
        ui.vertical_centered(|ui| {
            ui.add_space((ui.available_height() * 0.12).max(10.0));
            ui.add(egui::Image::new(&*logo).fit_to_exact_size(egui::vec2(96.0, 96.0)));
            ui.add_space(6.0);
            ui.label(RichText::new("apkscope").size(30.0).strong().color(p.text));
            ui.label(RichText::new("Android decompiler and API recon").size(15.0).color(p.dim));
            ui.label(RichText::new(concat!("v", env!("CARGO_PKG_VERSION"))).size(12.0).color(p.dim.gamma_multiply(0.8)));
            ui.add_space(24.0);
            let stroke = if dragging { Stroke::new(2.5_f32, p.accent) } else { Stroke::new(1.5_f32, p.dim.gamma_multiply(0.6)) };
            let fill = if dragging { p.accent.gamma_multiply(0.12) } else { p.panel };
            egui::Frame::new().fill(fill).stroke(stroke).corner_radius(CornerRadius::same(16)).inner_margin(Margin::symmetric(60, 36)).show(ui, |ui| {
                ui.set_width(520.0_f32.min(ui.available_width()));
                ui.vertical_centered(|ui| {
                    if loading {
                        ui.spinner();
                        ui.label(RichText::new(format!("Opening {}…", self.path)).size(15.0));
                    } else {
                        ui.label(RichText::new(if dragging { "Release to open" } else { "Drop an APK here" }).size(22.0).strong().color(p.text));
                        ui.add_space(6.0);
                        ui.label(RichText::new("Also opens split bundles (.apks, .xapk, .apkm) and .dex files").color(p.dim));
                        ui.add_space(14.0);
                        pick = ui.add(egui::Button::new(RichText::new("Open file…").size(15.0).strong()).fill(p.accent.gamma_multiply(0.55)).min_size(egui::vec2(160.0, 34.0))).clicked();
                    }
                });
            });
            if let Some(e) = &self.err {
                ui.add_space(12.0);
                ui.colored_label(Color32::from_rgb(230, 80, 80), format!("Couldn't open it: {e}"));
            }
            ui.add_space(22.0);
            ui.label(RichText::new("Decompile to Java  ·  find URLs, API calls and keys  ·  inspect native libraries  ·  export a Gradle project").color(p.dim));
        });
        if pick
            && let Some(f) = rfd::FileDialog::new()
                .add_filter("APK, split bundle or DEX", &session::INPUTS)
                .pick_file()
        {
            self.err = None;
            self.start_load(f.display().to_string());
        }
    }

    pub(super) fn settings_window(&mut self, ctx: &egui::Context) {
        if !self.show_settings {
            return;
        }
        let mut open = true;
        let before = self.set.clone();
        let st = self.st;
        egui::Window::new("Settings")
            .collapsible(false)
            .resizable(false)
            .open(&mut open)
            .default_width(420.0)
            .show(ctx, |ui| {
                let s = &mut self.set;
                ui.heading("Appearance");
                egui::Grid::new("app")
                    .num_columns(2)
                    .spacing([16.0, 8.0])
                    .show(ui, |ui| {
                        ui.label("Theme");
                        egui::ComboBox::from_id_salt("theme")
                            .selected_text(s.theme.name())
                            .show_ui(ui, |ui| {
                                for t in Theme::ALL {
                                    ui.selectable_value(&mut s.theme, t, t.name());
                                }
                            });
                        ui.end_row();
                        ui.label("Code font size");
                        ui.add(egui::Slider::new(&mut s.code_font, 10.0..=22.0).step_by(0.5));
                        ui.end_row();
                        ui.label("Interface scale");
                        ui.add(egui::Slider::new(&mut s.ui_scale, 0.8..=1.6).step_by(0.05));
                        ui.end_row();
                    });
                ui.separator();
                ui.heading("Decompiler");
                ui.checkbox(
                    &mut s.deobf,
                    "Rename obfuscated classes, methods and fields",
                )
                .on_hover_text("Applies to the next APK you open");
                ui.horizontal(|ui| {
                    ui.label("Default export context");
                    ui.add(egui::Slider::new(&mut s.export_ctx, 0..=30).text("lines"));
                });
                ui.separator();
                ui.heading("Performance");
                let cores = std::thread::available_parallelism().map_or(4, |n| n.get());
                ui.horizontal(|ui| {
                    ui.label("Worker threads");
                    ui.add(
                        egui::Slider::new(&mut s.threads, 0..=cores * 2).custom_formatter(
                            |v, _| {
                                if v == 0.0 {
                                    format!("auto ({cores})")
                                } else {
                                    format!("{v}")
                                }
                            },
                        ),
                    );
                });
                ui.checkbox(
                    &mut s.use_cache,
                    "Cache decompiled code on disk (fast repeat searches)",
                );
                ui.separator();
                ui.heading("Cache & usage");
                let size = *self.cache_size.get_or_insert_with(cache::total_size);
                egui::Grid::new("usage")
                    .num_columns(2)
                    .spacing([16.0, 6.0])
                    .show(ui, |ui| {
                        ui.label("Memory in use");
                        ui.label(session::memory_used().map_or("?".into(), mb));
                        ui.end_row();
                        ui.label("Disk cache (all APKs)");
                        ui.label(mb(size));
                        ui.end_row();
                        if let Some(st) = st {
                            ui.label("This APK");
                            let idx = if st.index.get().is_some() {
                                "indexed".to_string()
                            } else {
                                format!(
                                    "indexing {} / {}",
                                    st.indexed.load(Ordering::Relaxed),
                                    st.classes.len()
                                )
                            };
                            ui.label(format!(
                                "{idx}, {}",
                                if st.cached() {
                                    "code cached"
                                } else {
                                    "code not cached yet"
                                }
                            ));
                            ui.end_row();
                        }
                        ui.label("Cache folder");
                        ui.label(RichText::new(cache::root().display().to_string()).small());
                        ui.end_row();
                    });
                ui.horizontal(|ui| {
                    if ui.button("Clear cache").clicked() {
                        let _ = cache::clear_all();
                        if let Some(st) = st {
                            st.drop_cache();
                        }
                        self.cache_size = None;
                    }
                    if ui.button("Open folder").clicked() {
                        let _ = std::fs::create_dir_all(cache::root());
                        #[cfg(windows)]
                        let _ = std::process::Command::new("explorer")
                            .arg(cache::root())
                            .spawn();
                    }
                });
            });
        if self.set.theme != before.theme || self.set.ui_scale != before.ui_scale {
            self.styled = None;
        }
        if self.set.code_font != before.code_font || self.set.theme != before.theme {
            self.docs.iter_mut().for_each(|d| d.job = None);
        }
        session::THREADS.store(self.set.threads, Ordering::Relaxed);
        session::CACHE_WRITE.store(self.set.use_cache, Ordering::Relaxed);
        if !open {
            self.show_settings = false;
            self.cache_size = None;
            self.set.save();
        }
    }
}

//! Search: starting and cancelling it, and the results panel.

use super::*;

impl App {
    pub(super) fn run_search(&mut self) {
        let Some(st) = self.st else { return };
        if self.query.is_empty() && !self.scope.query_optional() {
            return;
        }
        // `a.b.*, impl:a.b.Proto`: the --in / --impl filters of the command line
        let (mut within, mut implements) = (Vec::new(), None);
        for part in self
            .within
            .split(',')
            .map(str::trim)
            .filter(|p| !p.is_empty())
        {
            match part.strip_prefix("impl:") {
                Some(t) => implements = Some(t.trim().to_string()),
                None => within.push(part.to_string()),
            }
        }
        let only = match crate::lookup::class_mask(st, &within, implements.as_deref()) {
            Ok(o) => o,
            Err(e) => {
                self.search_err = Some(e);
                return;
            }
        };
        let m = match Matcher::new(&self.query, self.scope, self.use_regex, self.case) {
            Ok(mut m) => {
                m.app_only = self.app_only;
                m.only = only;
                m
            }
            Err(e) => {
                self.search_err = Some(e);
                return;
            }
        };
        self.cancel_search();
        self.search_id += 1;
        self.search_err = None;
        self.folded.clear();
        self.hits_filter.clear();
        self.hits.clear();
        self.hits_title = if self.query.is_empty() {
            self.scope.label().to_string()
        } else {
            format!("{}: \"{}\"", self.scope.label(), self.query)
        };
        if self.app_only {
            self.hits_title += " (app code only)";
        }
        if !self.within.trim().is_empty() {
            self.hits_title += &format!(" in {}", self.within.trim());
        }
        self.search_started = Some(std::time::Instant::now());
        let prog = Arc::new(Progress::default());
        self.prog = Some(prog.clone());
        let (tx, id) = (self.tx.clone(), self.search_id);
        spawn(&self.tx, "search", move || {
            let (mut hits, hidden) = session::search_stats(st, &m, &prog);
            if let Some(only) = &m.only {
                hits.retain(|h| crate::lookup::class_id(st, &h.0).is_some_and(|k| only[k]));
            }
            let _ = tx.send(Msg::Hits(id, hits, hidden));
        });
    }

    /// Stop the running search and give the UI back now; a class stuck decompiling finishes
    /// on its own and its late result is dropped (stale search id).
    pub(super) fn cancel_search(&mut self) {
        if let Some(p) = self.prog.take() {
            p.cancel.store(true, Ordering::Relaxed);
            self.search_id += 1;
            self.hits_secs = self
                .search_started
                .map_or(0.0, |t| t.elapsed().as_secs_f32());
            self.hits_title += " (cancelled)";
        }
    }

    /// Usages of an exact name or string, as from the Strings tab.
    pub(super) fn usages_of(&mut self, s: &str) {
        self.scope = Scope::Usages;
        self.query = s.to_string();
        self.use_regex = false;
        self.case = true;
        self.run_search();
    }

    pub(super) fn results_panel(&mut self, ui: &mut egui::Ui) {
        let p = self.set.theme.palette();
        let mut go = None;
        let mut cancel = false;
        ui.horizontal(|ui| match (&self.prog, self.st) {
            (Some(pr), Some(_)) => {
                ui.spinner();
                let (done, total) = (
                    pr.done.load(Ordering::Relaxed),
                    pr.total.load(Ordering::Relaxed),
                );
                ui.label(format!("Searching {}…", self.hits_title));
                if total > 0 {
                    ui.add(
                        egui::ProgressBar::new(done as f32 / total as f32)
                            .desired_width(180.0)
                            .text(format!("{done} / {total} classes")),
                    );
                }
                cancel = ui.button("Cancel").on_hover_text("Esc").clicked();
            }
            _ => {
                let more = if self.hits.len() >= session::LIMIT {
                    format!(" (first {})", session::LIMIT)
                } else {
                    String::new()
                };
                ui.label(RichText::new(&self.hits_title).strong());
                ui.label(
                    RichText::new(format!(
                        "{} results{more} in {:.1} s",
                        self.hits.len(),
                        self.hits_secs
                    ))
                    .color(p.dim),
                );
                if ui
                    .add_enabled(!self.hits.is_empty(), egui::Button::new("Export…"))
                    .on_hover_text(
                        "Save every hit with its surrounding code to .txt, .csv or .json",
                    )
                    .clicked()
                {
                    self.export = Some(ExportDlg {
                        fmt: Format::Text,
                        ctx: self.set.export_ctx,
                        method: self.set.export_method,
                    });
                }
                if ui
                    .add_enabled(!self.hits.is_empty(), egui::Button::new("Copy"))
                    .clicked()
                {
                    let all: String = self
                        .hits
                        .iter()
                        .map(|(t, ln, text)| {
                            if *ln == 0 {
                                format!("{t}\n")
                            } else {
                                format!("{t}:{ln}\t{text}\n")
                            }
                        })
                        .collect();
                    ui.ctx().copy_text(all);
                    self.toast(format!("Copied {} results", self.hits.len()));
                }
                if ui.button("Close").clicked() {
                    self.hits.clear();
                    self.hits_title.clear();
                }
            }
        });
        if cancel {
            self.cancel_search();
        }
        ui.label(
            RichText::new(self.scope.note())
                .small()
                .italics()
                .color(p.dim),
        );
        ui.separator();
        let size = self.set.code_font - 0.5;
        let row = ui.fonts(|f| f.row_height(&FontId::monospace(size))) + 4.0;
        // hits grouped by class (they arrive sorted by class): a foldable header, then its lines
        let flt = self.hits_filter.to_lowercase();
        let keep = |h: &Hit| {
            flt.is_empty() || h.0.to_lowercase().contains(&flt) || h.2.to_lowercase().contains(&flt)
        };
        let mut groups: Vec<(&str, Vec<usize>)> = Vec::new();
        for (k, h) in self.hits.iter().enumerate().filter(|(_, h)| keep(h)) {
            match groups.last_mut() {
                Some(g) if g.0 == h.0 => g.1.push(k),
                _ => groups.push((&h.0, vec![k])),
            }
        }
        ui.horizontal(|ui| {
            edit(ui, &mut self.hits_filter, "Filter results…", 220.0);
            ui.label(RichText::new(format!("{} classes", groups.len())).color(p.dim));
            if ui.small_button("Fold all").clicked() {
                self.folded = groups.iter().map(|g| g.0.to_string()).collect();
            }
            if ui.small_button("Unfold all").clicked() {
                self.folded.clear();
            }
        });
        ui.separator();
        // rows: (hit index, is header)
        let mut rows: Vec<(usize, bool)> = Vec::new();
        for (c, ks) in &groups {
            rows.push((ks[0], true));
            if !self.folded.contains(*c) {
                rows.extend(
                    ks.iter()
                        .filter(|&&k| !self.hits[k].2.is_empty())
                        .map(|&k| (k, false)),
                );
            }
        }
        let counts: std::collections::HashMap<&str, usize> = groups
            .iter()
            .map(|g| {
                (
                    g.0,
                    g.1.iter().filter(|&&k| !self.hits[k].2.is_empty()).count(),
                )
            })
            .collect();
        // the query, to mark inside each line
        let needle = (!self.use_regex && !self.query.is_empty()).then(|| {
            if self.case {
                self.query.clone()
            } else {
                self.query.to_lowercase()
            }
        });
        let re = self
            .use_regex
            .then(|| {
                regex::RegexBuilder::new(&self.query)
                    .case_insensitive(!self.case)
                    .build()
                    .ok()
            })
            .flatten();
        let mut fold = None;
        let mono = FontId::monospace(size);
        egui::ScrollArea::both()
            .auto_shrink(false)
            .show_rows(ui, row, rows.len(), |ui, range| {
                for &(k, header) in &rows[range] {
                    let (t, ln, text) = &self.hits[k];
                    let mut job = LayoutJob::default();
                    if header {
                        let n = counts.get(t.as_str()).copied().unwrap_or(0);
                        let arrow = if n == 0 {
                            "  "
                        } else if self.folded.contains(t) {
                            "▶ "
                        } else {
                            "▼ "
                        };
                        job.append(arrow, 0.0, TextFormat::simple(mono.clone(), p.dim));
                        let (pkg, name) = t.rsplit_once(['.', '/']).unwrap_or(("", t.as_str()));
                        job.append(name, 0.0, TextFormat::simple(mono.clone(), p.accent));
                        if !pkg.is_empty() {
                            job.append(
                                &format!("  {pkg}"),
                                0.0,
                                TextFormat::simple(mono.clone(), p.dim),
                            );
                        }
                        if n > 0 {
                            job.append(
                                &format!("  ({n})"),
                                0.0,
                                TextFormat::simple(mono.clone(), p.dim),
                            );
                        }
                        let r = ui
                            .add(egui::Label::new(job).sense(egui::Sense::click()).extend())
                            .on_hover_cursor(egui::CursorIcon::PointingHand);
                        if r.clicked() {
                            if n == 0 {
                                go = Some((t.clone(), 0))
                            } else {
                                fold = Some(t.clone())
                            }
                        }
                        if r.double_clicked() {
                            go = Some((t.clone(), 0));
                        }
                        continue;
                    }
                    let lnt = if *ln > 0 {
                        ln.to_string()
                    } else {
                        String::new()
                    };
                    job.append(
                        &format!("    {lnt:>5}  "),
                        0.0,
                        TextFormat::simple(mono.clone(), p.dim),
                    );
                    let line = text.trim();
                    // spans of the query in this line
                    let spans: Vec<(usize, usize)> = match (&needle, &re) {
                        (Some(nd), _) => {
                            let hay = if self.case {
                                line.to_string()
                            } else {
                                line.to_lowercase()
                            };
                            // lowercasing can change byte lengths; only mark when it didn't
                            if hay.len() == line.len() {
                                hay.match_indices(nd.as_str())
                                    .map(|(i, m)| (i, i + m.len()))
                                    .collect()
                            } else {
                                vec![]
                            }
                        }
                        (_, Some(re)) => re.find_iter(line).map(|m| (m.start(), m.end())).collect(),
                        _ => vec![],
                    };
                    let mut at = 0;
                    for (s0, e0) in spans {
                        job.append(&line[at..s0], 0.0, TextFormat::simple(mono.clone(), p.text));
                        let mut f = TextFormat::simple(mono.clone(), p.text);
                        f.background = p.accent.gamma_multiply(0.38);
                        job.append(&line[s0..e0], 0.0, f);
                        at = e0;
                    }
                    job.append(&line[at..], 0.0, TextFormat::simple(mono.clone(), p.text));
                    if ui
                        .add(egui::Label::new(job).sense(egui::Sense::click()).extend())
                        .on_hover_cursor(egui::CursorIcon::PointingHand)
                        .on_hover_text("Open at this line")
                        .clicked()
                    {
                        go = Some((t.clone(), *ln));
                    }
                }
            });
        if let Some(c) = fold
            && !self.folded.remove(&c)
        {
            self.folded.insert(c);
        }
        if let Some((t, ln)) = go {
            let o = match self.st {
                Some(s) if s.is_native(&t) => Open::Native(t),
                Some(s) if s.is_res(&t) => Open::Res(t),
                _ => Open::Class(t),
            };
            self.push_nav();
            self.open(o, Some(View::Java), (ln > 0).then_some(ln));
            // mark the searched name in the opened code
            let q = self.query.clone();
            if let Some(d) = self.docs.get_mut(self.active)
                && !self.use_regex
                && !q.is_empty()
                && q.chars()
                    .all(|c| c.is_alphanumeric() || c == '_' || c == '$')
            {
                d.sel = Some(q);
                d.job = None;
            }
        }
    }
}

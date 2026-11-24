//! `apkscope gui`: native window (egui). The APK loads on a background thread, then the
//! reference index builds in the background; classes are decompiled when opened.

use std::collections::BTreeMap;
use std::sync::atomic::Ordering;
use std::sync::{Arc, mpsc};

use eframe::egui::{
    self, Color32, CornerRadius, FontId, Margin, RichText, Stroke, TextFormat, text::LayoutJob,
};

use crate::{apksign, arsc, cache, rebuild, smali};

mod frame;
mod highlight;
mod live_ui;
mod menu;
mod nav;
mod search_ui;
mod side;
mod theme;
mod tree;
mod windows;
use crate::re::{Line, Link};
use crate::session::{self, Format, Hit, Matcher, Origin, Progress, Scope, State};
use highlight::*;
use menu::*;
use nav::*;
use theme::*;
use tree::*;

// ---------- settings ----------

// ---------- trees ----------

// ---------- documents ----------

#[derive(Clone, Copy, PartialEq)]
enum View {
    Java,
    Smali,
}

#[derive(Clone, PartialEq)]
enum Open {
    Overview,
    Class(String),
    Res(String),
    Native(String),
    File(String),
    /// generated text: callers, call graph, Frida script, security scan ...
    Report(String),
}

impl Open {
    fn name(&self) -> &str {
        match self {
            Open::Overview => "Overview",
            Open::Class(c) | Open::Res(c) | Open::Native(c) | Open::File(c) | Open::Report(c) => c,
        }
    }

    fn short(&self) -> &str {
        let n = self.name();
        let sep = match self {
            Open::Class(_) => '.',
            Open::Report(_) => return n,
            _ => '/',
        };
        n.rsplit_once(sep).map_or(n, |x| x.1)
    }
}

/// What a laid-out text was made for (colour, font size, marked line, highlighted word):
/// the layout is redone only when one of them changes.
type JobKey = (Color32, u32, Option<usize>, Option<String>);

struct Doc {
    open: Open,
    view: View,
    text: Option<Arc<String>>,
    /// per line of a report: where clicking it leads
    links: Option<Arc<Vec<Option<Link>>>>,
    /// 1-based line to scroll to and mark
    mark: Option<usize>,
    scroll_pending: bool,
    /// highlighted text and what it was built for: (background, font size x10, marked line, selected word)
    job: Option<(JobKey, LayoutJob)>,
    /// identifier clicked in the code, marked everywhere in the file
    sel: Option<String>,
    find: String,
    /// manifest text being edited, when in edit mode
    editing: Option<String>,
    /// where to jump once the document's text arrives
    find_on_load: Option<Target>,
    /// line and column (0-based) the context menu was opened on
    menu_at: Option<(usize, usize)>,
    /// line actions opened from the keyboard (Shift+F10), shown in a window
    kb_menu: bool,
}

impl Doc {
    fn new(open: Open) -> Doc {
        Doc {
            open,
            view: View::Java,
            text: None,
            links: None,
            mark: None,
            scroll_pending: false,
            job: None,
            sel: None,
            find: String::new(),
            editing: None,
            find_on_load: None,
            menu_at: None,
            kb_menu: false,
        }
    }
}

/// A place inside a class's Java: a method declaration (shown name, parameter count), then
/// optionally the first line after it that contains `then` (a call, a field use).
#[derive(Clone)]
struct Target {
    decl: Option<(String, Option<usize>)>,
    then: Option<String>,
}

/// "Rename" / "Comment" dialog
struct NoteDlg {
    /// project-file key: `La/b;`, `La/b;->field`, `La/b;->method()`
    key: String,
    comment: bool,
    label: String,
    text: String,
}

/// What a click or right-click in the code asked for.
enum Act {
    Callers(String, Option<String>),
    Graph(String, Option<String>),
    Frida(String, Option<String>),
    /// hook in the running Frida session
    LiveHook(String, Option<String>),
    Access(String, String),
    Smali(String, String),
    Note(String, bool, String),
    GoTo(Link),
    /// top-level class (Java name as listed), and where in it
    OpenClass(String, Option<Target>),
    /// 1-based line of the active tab
    Line(usize),
    Usages(String),
}

enum Msg {
    Loaded(Result<Box<State>, String>),
    Text(Open, View, String),
    Report(Open, Vec<Line>),
    Hits(u64, Vec<Hit>, usize),
    /// (query, matching strings)
    Strings(String, Vec<(String, usize)>),
    Saved(Result<String, String>),
    /// a background job panicked: (what it was doing, why)
    Failed(&'static str, String),
}

#[derive(Clone, Copy, PartialEq)]
enum Side {
    Classes,
    Resources,
    Strings,
    Native,
    Files,
}

/// "Edit strings" window
struct StringsDlg {
    /// the resources.arsc the edits apply to (with any pending changes)
    base: Vec<u8>,
    all: Vec<arsc::StringRes>,
    config: String,
    filter: String,
    /// (config, name) -> new text
    changes: std::collections::BTreeMap<(String, String), String>,
}

/// "Build & sign" dialog
struct BuildDlg {
    /// sign with the user's own key instead of apkscope's
    own_key: bool,
    key: String,
    cert: String,
}

struct ExportDlg {
    fmt: Format,
    ctx: usize,
    method: bool,
}

pub struct App {
    path: String,
    set: Settings,
    st: Option<&'static State>,
    err: Option<String>,
    /// class trees for App, Other and Library code
    trees: [Tree; 3],
    counts: [usize; 3],
    show_libs: bool,
    app_only: bool,
    /// search filter: packages / classes (`a.b.*`, comma-separated) and `impl:Type`
    within: String,
    res: BTreeMap<String, Vec<String>>,
    tx: mpsc::Sender<Msg>,
    rx: mpsc::Receiver<Msg>,
    docs: Vec<Doc>,
    active: usize,
    side: Side,
    filter: String,
    strings_q: String,
    /// query the shown strings were listed for
    strings_for: Option<String>,
    strings: Vec<(String, usize)>,
    natives: Option<Vec<(String, u64)>>,
    files: Option<Vec<(String, u64, u64)>>,
    /// pending changes to the APK: name -> new content, or None to delete
    edits: rebuild::Edits,
    /// edited Smali class sources, assembled into DEX at build time
    smali_edits: Vec<String>,
    /// a folder of edited resources (the `apkscope res` layout), applied at build time
    res_dir: Option<String>,
    /// "Add file" dialog: (path inside the APK, content)
    adding: Option<(String, Vec<u8>)>,
    build: Option<BuildDlg>,
    strings_dlg: Option<StringsDlg>,
    query: String,
    scope: Scope,
    use_regex: bool,
    case: bool,
    hits: Vec<Hit>,
    hits_title: String,
    hits_secs: f32,
    search_started: Option<std::time::Instant>,
    search_err: Option<String>,
    search_id: u64,
    prog: Option<Arc<Progress>>,
    focus_search: bool,
    show_settings: bool,
    export: Option<ExportDlg>,
    toast: Option<(String, std::time::Instant)>,
    cache_size: Option<u64>,
    styled: Option<(Theme, u32)>,
    note_dlg: Option<NoteDlg>,
    /// navigation history: (tab, view, marked line)
    back: Vec<(Open, View, Option<usize>)>,
    fwd: Vec<(Open, View, Option<usize>)>,
    /// search results: classes folded away, and a filter over the hits
    folded: std::collections::HashSet<String>,
    hits_filter: String,
    titled: bool,
    logo: Option<egui::TextureHandle>,
    live: live_ui::LiveUi,
}

/// One-line text box whose hint is also its name for screen readers.
fn edit(ui: &mut egui::Ui, buf: &mut String, hint: &str, width: f32) -> egui::Response {
    let r = ui.add(
        egui::TextEdit::singleline(buf)
            .hint_text(hint)
            .desired_width(width),
    );
    r.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::TextEdit, true, hint));
    r
}

/// Run `f` in the background. If it panics, the UI hears about it instead of waiting forever.
fn spawn(tx: &mpsc::Sender<Msg>, what: &'static str, f: impl FnOnce() + Send + 'static) {
    let mine = tx.clone();
    // decompiling recurses per nesting level
    let ok = std::thread::Builder::new()
        .stack_size(256 << 20)
        .spawn(move || {
            let tx = mine;
            if let Err(e) = std::panic::catch_unwind(std::panic::AssertUnwindSafe(f)) {
                let why = e
                    .downcast_ref::<&str>()
                    .map(|s| s.to_string())
                    .or_else(|| e.downcast_ref::<String>().cloned())
                    .unwrap_or_else(|| "unknown error".into());
                let _ = tx.send(Msg::Failed(what, why));
            }
        });
    if let Err(e) = ok {
        let _ = tx.send(Msg::Failed(what, e.to_string()));
    }
}

impl App {
    fn new(path: Option<String>, deobf_flag: bool) -> Self {
        let (tx, rx) = mpsc::channel();
        let mut set = Settings::load();
        // --no-deobf on the command line wins over the setting
        set.deobf &= deobf_flag;
        session::THREADS.store(set.threads, Ordering::Relaxed);
        let mut app = App {
            path: String::new(),
            set,
            st: None,
            err: None,
            trees: Default::default(),
            counts: [0; 3],
            show_libs: false,
            app_only: false,
            within: String::new(),
            res: BTreeMap::new(),
            tx,
            rx,
            docs: vec![],
            active: 0,
            side: Side::Classes,
            filter: String::new(),
            strings_q: String::new(),
            strings_for: None,
            strings: vec![],
            natives: None,
            files: None,
            edits: rebuild::Edits::new(),
            smali_edits: vec![],
            res_dir: None,
            adding: None,
            build: None,
            strings_dlg: None,
            query: String::new(),
            scope: Scope::Code,
            use_regex: false,
            case: false,
            hits: vec![],
            hits_title: String::new(),
            hits_secs: 0.0,
            search_started: None,
            search_err: None,
            search_id: 0,
            prog: None,
            focus_search: false,
            show_settings: false,
            export: None,
            toast: None,
            cache_size: None,
            styled: None,
            note_dlg: None,
            back: vec![],
            fwd: vec![],
            folded: Default::default(),
            hits_filter: String::new(),
            titled: false,
            logo: None,
            live: Default::default(),
        };
        if let Some(p) = path {
            app.start_load(p);
        }
        app
    }

    fn toast(&mut self, s: impl Into<String>) {
        self.toast = Some((s.into(), std::time::Instant::now()));
    }

    fn start_load(&mut self, path: String) {
        // the name tables are process-wide, so a second APK gets its own window
        if !self.path.is_empty() {
            if let Ok(exe) = std::env::current_exe() {
                let mut cmd = std::process::Command::new(exe);
                cmd.arg("gui").arg(&path);
                if !self.set.deobf {
                    cmd.arg("--no-deobf");
                }
                let _ = cmd.spawn();
            }
            return;
        }
        self.path = path.clone();
        let (tx, deobf) = (self.tx.clone(), self.set.deobf);
        spawn(&self.tx, "load", move || {
            let _ = tx.send(Msg::Loaded(session::load(&path, deobf).map(Box::new)));
        });
    }

    /// Show `what` in its tab (opening one if needed), optionally scrolled to a line.
    fn open(&mut self, what: Open, view: Option<View>, mark: Option<usize>) {
        let Some(st) = self.st else { return };
        let k = match self.docs.iter().position(|d| d.open == what) {
            Some(k) => k,
            None => {
                self.docs.push(Doc::new(what.clone()));
                self.docs.len() - 1
            }
        };
        self.active = k;
        let d = &mut self.docs[k];
        if mark.is_some() {
            d.mark = mark;
            d.scroll_pending = true;
            d.job = None;
        }
        let view = view.unwrap_or(d.view);
        if d.text.is_some() && d.view == view {
            return;
        }
        d.view = view;
        d.text = None;
        d.job = None;
        let tx = self.tx.clone();
        spawn(&self.tx, "open", move || {
            let text = match &what {
                Open::Overview => Some(session::overview(st)),
                Open::Class(c) => st.class_text(c, view == View::Smali),
                Open::Res(p) => st.resource(p),
                Open::Native(p) => Some(st.native_report(p)),
                Open::File(p) => Some(st.file_view(p)),
                Open::Report(_) => None,
            };
            let _ = tx.send(Msg::Text(
                what,
                view,
                text.unwrap_or_else(|| "// not found".into()),
            ));
        });
    }

    /// Open a generated report in its own tab, computed off the UI thread. Lines with a link
    /// jump to the code when clicked.
    fn report(
        &mut self,
        title: String,
        f: impl FnOnce(&'static State) -> Vec<Line> + Send + 'static,
    ) {
        let Some(st) = self.st else { return };
        self.push_nav();
        let open = Open::Report(title);
        self.docs.retain(|d| d.open != open);
        self.docs.push(Doc::new(open.clone()));
        self.active = self.docs.len() - 1;
        let tx = self.tx.clone();
        spawn(&self.tx, "report", move || {
            let _ = tx.send(Msg::Report(open, f(st)));
        });
    }

    /// Remember where we are before jumping somewhere else (Back = Alt+Left).
    fn push_nav(&mut self) {
        if let Some(d) = self.docs.get(self.active) {
            self.back.push((d.open.clone(), d.view, d.mark));
            self.fwd.clear();
            if self.back.len() > 100 {
                self.back.remove(0);
            }
        }
    }

    fn nav(&mut self, forward: bool) {
        let (from, to) = if forward {
            (&mut self.fwd, &mut self.back)
        } else {
            (&mut self.back, &mut self.fwd)
        };
        let Some((o, v, m)) = from.pop() else { return };
        if let Some(d) = self.docs.get(self.active) {
            to.push((d.open.clone(), d.view, d.mark));
        }
        if matches!(o, Open::Report(_)) && !self.docs.iter().any(|d| d.open == o) {
            return; // closed report: nothing to go back to
        }
        self.open(o, Some(v), m);
    }

    /// Open a link's method, at the call or field use it points to.
    fn go_to(&mut self, l: &Link) {
        let Some(st) = self.st else { return };
        let Some((desc, rest)) = l.at.split_once("->") else {
            return;
        };
        let top = desc
            .find('$')
            .map_or(desc.to_string(), |k| format!("{};", &desc[..k]));
        let Some(n) = st
            .classes
            .iter()
            .find(|x| st.dexes[x.1].class_descriptor(x.2).ok().as_deref() == Some(top.as_str()))
            .map(|x| x.0.clone())
        else {
            self.toast(format!(
                "{} is not in this APK",
                crate::dex::java_name(desc)
            ));
            return;
        };
        let name = rest.split('(').next().unwrap_or("");
        let shown = match name {
            "<init>" => crate::dex::java_name(desc)
                .rsplit(['.', '$'])
                .next()
                .unwrap_or("")
                .to_string(),
            n => crate::deobf::method(desc, n).unwrap_or_else(|| n.to_string()),
        };
        let nargs = rest
            .split_once('(')
            .map(|x| crate::re::split_params(x.1.split(')').next().unwrap_or("")).len());
        let decl = (name != "<clinit>").then_some((shown, nargs));
        self.push_nav();
        self.open(Open::Class(n), Some(View::Java), None);
        self.find_in_active(Target {
            decl,
            then: l.then.clone(),
        });
    }

    /// Jump to a target in the active tab now, or once its text arrives.
    fn find_in_active(&mut self, t: Target) {
        let Some(d) = self.docs.get_mut(self.active) else {
            return;
        };
        match &d.text {
            Some(text) => {
                if let Some(l) = locate(text, &t) {
                    d.mark = Some(l);
                    d.scroll_pending = true;
                    d.job = None;
                }
            }
            None => d.find_on_load = Some(t),
        }
    }

    /// Store a rename or comment in the project file and redraw the open classes with it.
    fn save_note(&mut self, key: &str, comment: bool, text: &str) {
        let mut n = crate::deobf::notes();
        crate::re::apply_note(&mut n, key, comment, text);
        self.store_notes(n, None);
    }

    /// Save names and comments to the project file, then redraw the open classes with them.
    /// `done` replaces the "Saved to" toast.
    fn store_notes(&mut self, n: crate::deobf::Notes, done: Option<String>) {
        let Some(st) = self.st else { return };
        if let Err(e) = crate::re::save_notes(&st.path, &n) {
            self.toast(format!("Not saved: {e}"));
            return;
        }
        crate::deobf::set_notes(n);
        // cached code has the old names; and new code must not go under the old cache key
        st.drop_cache();
        session::CACHE_WRITE.store(false, Ordering::Relaxed);
        let active = self.active;
        let reopen: Vec<(Open, View)> = self
            .docs
            .iter_mut()
            .filter(|d| matches!(d.open, Open::Class(_)))
            .map(|d| {
                d.text = None;
                (d.open.clone(), d.view)
            })
            .collect();
        for (o, v) in reopen {
            self.open(o, Some(v), None);
        }
        self.active = active.min(self.docs.len().saturating_sub(1));
        self.toast(done.unwrap_or_else(|| {
            format!("Saved to {}", crate::re::project_file(&st.path).display())
        }));
    }

    /// Run one CLI command in the background and show what it printed as a report, so
    /// every command the CLI has is one menu item away without a second implementation.
    fn cli_report(&mut self, title: &str, args: Vec<std::ffi::OsString>) {
        self.report(title.into(), move |_| {
            let out = std::env::current_exe().and_then(|exe| {
                let mut cmd = std::process::Command::new(exe);
                cmd.args(&args).stdin(std::process::Stdio::null());
                #[cfg(windows)]
                {
                    use std::os::windows::process::CommandExt;
                    cmd.creation_flags(0x0800_0000); // CREATE_NO_WINDOW: no console flash
                }
                cmd.output()
            });
            let text = match out {
                Ok(o) => format!(
                    "{}{}",
                    String::from_utf8_lossy(&o.stdout),
                    String::from_utf8_lossy(&o.stderr)
                ),
                Err(e) => format!("error: {e}"),
            };
            text.lines().map(|l| (l.to_string(), None)).collect()
        });
    }

    /// Carry out a context-menu choice.
    fn act(&mut self, a: Act, ctx: &egui::Context) {
        let Some(st) = self.st else { return };
        let short = |c: &str, m: &Option<String>| {
            format!(
                "{}{}",
                crate::dex::java_name(c).rsplit('.').next().unwrap_or(""),
                m.as_ref().map(|m| format!(".{m}")).unwrap_or_default()
            )
        };
        match a {
            Act::Callers(c, m) => {
                let title = if m.as_deref() == Some("<init>") {
                    format!("new {}", short(&c, &None))
                } else {
                    format!("Callers {}", short(&c, &m))
                };
                self.report(title, move |st| {
                    crate::re::callers_report(&st.dexes, &c, m.as_deref())
                })
            }
            Act::Graph(c, m) => self.report(format!("Graph {}", short(&c, &m)), move |st| {
                crate::re::graph_report(&st.dexes, &c, m.as_deref(), 3)
            }),
            Act::Access(c, f) => self.report(
                format!("Access {}", short(&c, &Some(f.clone()))),
                move |st| crate::re::access_report(&st.dexes, &c, &f),
            ),
            Act::Frida(c, m) => match crate::re::frida(&st.dexes, &c, m.as_deref(), false) {
                Ok(js) => {
                    ctx.copy_text(js.clone());
                    self.report(format!("Frida {}", short(&c, &m)), move |_| {
                        js.lines().map(|l| (l.to_string(), None)).collect()
                    });
                    self.toast("Frida script copied to the clipboard");
                }
                Err(e) => self.toast(e),
            },
            Act::LiveHook(c, m) => {
                let r = self.live.hook(st, &c, m.as_deref());
                self.toast(r.unwrap_or_else(|e| e));
            }
            Act::Smali(class_name, method) => {
                self.push_nav();
                self.open(Open::Class(class_name), Some(View::Smali), None);
                self.find_in_active(Target {
                    decl: None,
                    then: Some(format!(" {method}(")),
                });
            }
            Act::OpenClass(name, target) => {
                self.push_nav();
                self.open(Open::Class(name), Some(View::Java), None);
                if let Some(t) = target {
                    self.find_in_active(t);
                }
            }
            Act::Line(l) => {
                self.push_nav();
                if let Some(d) = self.docs.get_mut(self.active) {
                    d.mark = Some(l);
                    d.scroll_pending = true;
                    d.job = None;
                }
            }
            Act::Usages(w) => self.usages_of(&w),
            Act::Note(key, comment, label) => {
                let n = crate::deobf::notes();
                let class = !key.contains("->");
                let cur = if comment {
                    n.comments.get(&key)
                } else if class {
                    n.classes.get(&key)
                } else {
                    n.members.get(&key)
                };
                let text = cur
                    .map(|v| {
                        if !comment && class {
                            crate::dex::java_name(v)
                        } else {
                            v.clone()
                        }
                    })
                    .unwrap_or_default();
                self.note_dlg = Some(NoteDlg {
                    key,
                    comment,
                    label,
                    text,
                });
            }
            Act::GoTo(l) => self.go_to(&l),
        }
    }

    fn note_window(&mut self, ctx: &egui::Context) {
        let Some(dlg) = &mut self.note_dlg else {
            return;
        };
        let mut done = None;
        let mut open = true;
        egui::Window::new(if dlg.comment { "Comment" } else { "Rename" }).collapsible(false).open(&mut open).show(ctx, |ui| {
            ui.label(&dlg.label);
            let r = if dlg.comment {
                ui.add(egui::TextEdit::multiline(&mut dlg.text).desired_rows(4).desired_width(420.0))
            } else {
                edit(ui, &mut dlg.text, "new name", 320.0)
            };
            ui.label(RichText::new("Saved next to the APK in <apk>.apkscope.json. `apkscope map` exports it for JADX / Enigma.").small());
            ui.horizontal(|ui| {
                let enter = !dlg.comment && r.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
                if ui.button("Save").clicked() || enter {
                    done = Some((dlg.key.clone(), dlg.comment, dlg.text.trim().to_string()));
                }
                if ui.button("Remove").on_hover_text("Back to the original name / no comment").clicked() {
                    done = Some((dlg.key.clone(), dlg.comment, String::new()));
                }
            });
        });
        if let Some((k, c, t)) = done {
            self.note_dlg = None;
            self.save_note(&k, c, &t);
        } else if !open {
            self.note_dlg = None;
        }
    }

    fn close(&mut self, k: usize) {
        self.docs.remove(k);
        if self.active >= self.docs.len() {
            self.active = self.docs.len().saturating_sub(1);
        }
    }

    fn poll(&mut self) {
        while let Ok(m) = self.rx.try_recv() {
            match m {
                Msg::Loaded(Ok(st)) => {
                    let st: &'static State = Box::leak(st);
                    for (k, o) in [Origin::App, Origin::Other, Origin::Library]
                        .into_iter()
                        .enumerate()
                    {
                        let mine: Vec<&String> = st
                            .classes
                            .iter()
                            .zip(&st.origin)
                            .filter(|x| *x.1 == o)
                            .map(|x| &x.0.0)
                            .collect();
                        self.counts[k] = mine.len();
                        self.trees[k] = tree(mine.into_iter());
                    }
                    self.res = res_groups(st);
                    self.strings_for = None;
                    self.strings.clear();
                    self.st = Some(st);
                    spawn(&self.tx, "index", move || st.build_index());
                    self.open(Open::Overview, None, None);
                }
                Msg::Loaded(Err(e)) => {
                    // nothing is loaded, so the next file opens here instead of in a new window
                    self.err = Some(e);
                    self.path.clear();
                }
                Msg::Text(open, view, text) => {
                    if let Some(d) = self
                        .docs
                        .iter_mut()
                        .find(|d| d.open == open && d.view == view)
                    {
                        if let Some(l) = d.find_on_load.take().and_then(|t| locate(&text, &t)) {
                            d.mark = Some(l);
                            d.scroll_pending = true;
                        }
                        d.text = Some(Arc::new(text));
                        d.job = None;
                    }
                }
                Msg::Report(open, lines) => {
                    if let Some(d) = self.docs.iter_mut().find(|d| d.open == open) {
                        let text: String = lines.iter().map(|l| format!("{}\n", l.0)).collect();
                        d.links = Some(Arc::new(lines.into_iter().map(|l| l.1).collect()));
                        d.text = Some(Arc::new(text));
                        d.job = None;
                    }
                }
                Msg::Hits(id, mut h, hidden) if id == self.search_id => {
                    self.prog = None;
                    if hidden > 0 {
                        self.hits_title.push_str(&format!(
                            " ({hidden} library class{} hidden)",
                            if hidden == 1 { "" } else { "es" }
                        ));
                    }
                    if self.scope == Scope::Urls {
                        self.hits_title
                            .push_str(&format!(" — {} hosts", session::host_count(&h)));
                    }
                    // the labelled entropy tier of `secrets`: rows under their class, each
                    // row named so it cannot be mistaken for a pattern hit
                    if self.scope == Scope::Secrets
                        && let Some(st) = self.st
                    {
                        let extra: Vec<Hit> = session::entropy_secrets(st)
                            .into_iter()
                            .map(|c| {
                                (
                                    c.class,
                                    0,
                                    format!(
                                        "possible secret (entropy): {} = \"{}\"",
                                        c.field, c.value
                                    ),
                                )
                            })
                            .collect();
                        if !extra.is_empty() {
                            self.hits_title
                                .push_str(&format!(" (+{} entropy)", extra.len()));
                        }
                        h.extend(extra);
                    }
                    self.hits = h;
                    // stable: lines stay in order within a class
                    self.hits.sort_by(|a, b| a.0.cmp(&b.0));
                    self.hits_secs = self
                        .search_started
                        .map_or(0.0, |t| t.elapsed().as_secs_f32());
                }
                Msg::Hits(..) => {}
                Msg::Strings(q, v) if self.strings_for.as_deref() == Some(q.as_str()) => {
                    self.strings = v
                }
                Msg::Strings(..) => {}
                Msg::Saved(Ok(s)) => self.toast(s),
                Msg::Saved(Err(e)) => self.toast(format!("Export failed: {e}")),
                Msg::Failed(what, why) => {
                    let msg = format!("Something went wrong ({what}): {why}");
                    match what {
                        "load" => {
                            self.err = Some(msg.clone());
                            self.path.clear();
                        }
                        "search" => {
                            self.prog = None;
                            self.search_err = Some(msg.clone());
                        }
                        // tabs still waiting for text would spin forever
                        "open" | "report" => {
                            for d in self.docs.iter_mut().filter(|d| d.text.is_none()) {
                                d.text = Some(Arc::new(format!("// {msg}\n")));
                                d.job = None;
                            }
                        }
                        _ => {}
                    }
                    self.toast(msg);
                }
            }
        }
    }

    fn style(&mut self, ctx: &egui::Context) {
        let key = (self.set.theme, (self.set.ui_scale * 100.0) as u32);
        if self.styled == Some(key) {
            return;
        }
        self.styled = Some(key);
        let p = self.set.theme.palette();
        let mut v = if self.set.theme.dark() {
            egui::Visuals::dark()
        } else {
            egui::Visuals::light()
        };
        v.panel_fill = p.panel;
        v.window_fill = p.panel;
        v.extreme_bg_color = p.bg;
        v.faint_bg_color = p.bg;
        v.override_text_color = Some(p.text);
        v.hyperlink_color = p.accent;
        v.selection.bg_fill = p.accent.gamma_multiply(0.45);
        v.selection.stroke = Stroke::new(1.0_f32, p.accent);
        v.window_corner_radius = CornerRadius::same(10);
        v.menu_corner_radius = CornerRadius::same(8);
        for w in [
            &mut v.widgets.inactive,
            &mut v.widgets.hovered,
            &mut v.widgets.active,
            &mut v.widgets.open,
            &mut v.widgets.noninteractive,
        ] {
            w.corner_radius = CornerRadius::same(6);
        }
        // a hairline so text fields stand out from the panel they sit on
        v.widgets.inactive.bg_stroke = Stroke::new(1.0_f32, p.dim.gamma_multiply(0.35));
        v.widgets.hovered.bg_stroke = Stroke::new(1.0_f32, p.accent.gamma_multiply(0.6));
        v.widgets.active.bg_fill = p.accent.gamma_multiply(0.6);
        ctx.set_visuals(v);
        ctx.style_mut(|s| {
            s.spacing.item_spacing = egui::vec2(8.0, 5.0);
            s.spacing.button_padding = egui::vec2(10.0, 4.0);
            s.spacing.interact_size.y = 24.0;
        });
        ctx.set_zoom_factor(self.set.ui_scale);
    }
}

pub fn run(path: Option<&str>, deobf: bool) -> Result<(), String> {
    // opengl32 is delay-loaded (see build.rs) and the GL loader finds it with GetModuleHandle
    #[cfg(windows)]
    {
        unsafe extern "system" {
            fn LoadLibraryA(name: *const u8) -> *mut u8;
        }
        unsafe { LoadLibraryA(c"opengl32.dll".as_ptr().cast()) };
    }
    let icon = egui::IconData {
        rgba: include_bytes!("../assets/icon64.rgba").to_vec(),
        width: 64,
        height: 64,
    };
    let viewport = egui::ViewportBuilder::default()
        .with_inner_size([1100.0, 700.0])
        .with_maximized(true)
        .with_title("apkscope")
        .with_icon(Arc::new(icon))
        .with_drag_and_drop(true);
    let opts = eframe::NativeOptions {
        viewport,
        ..Default::default()
    };
    let path = path.map(str::to_string);
    eframe::run_native(
        "apkscope",
        opts,
        Box::new(move |_| Ok(Box::new(App::new(path, deobf)))),
    )
    .map_err(|e| e.to_string())
}

#[cfg(test)]
#[path = "../tests/apkscope/gui.rs"]
mod tests;

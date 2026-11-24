//! `search res` inside every resource XML (layouts, menus, xml/, drawables...), not only the
//! values files and the manifest decoded at startup: `@id/login_button` ->
//! `res/layout/activity_login.xml:37`. Files are decoded on all cores, only when searched.

use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};

use crate::session::{Hit, Matcher, Progress, State, threads};

pub fn xml_hits(st: &State, m: &Matcher, prog: &Progress) -> Vec<Hit> {
    let list: Vec<&String> = st
        .files()
        .keys()
        .filter(|p| p.ends_with(".xml") && !st.texts().contains_key(*p))
        .collect();
    let next = AtomicUsize::new(0);
    let found = Mutex::new(Vec::new());
    std::thread::scope(|s| {
        for _ in 0..threads().min(list.len()).max(1) {
            s.spawn(|| {
                let mut mine = Vec::new();
                loop {
                    let k = next.fetch_add(1, Ordering::Relaxed);
                    let Some(p) = list.get(k) else { break };
                    if prog.cancel.load(Ordering::Relaxed) {
                        break;
                    }
                    let Some(text) = st.resource(p) else { continue };
                    for (ln, l) in text.lines().enumerate() {
                        if m.line(l) {
                            mine.push(((*p).clone(), ln + 1, l.trim().to_string()));
                        }
                    }
                }
                found.lock().unwrap().extend(mine);
            });
        }
    });
    let mut hits = found.into_inner().unwrap();
    hits.sort();
    hits
}

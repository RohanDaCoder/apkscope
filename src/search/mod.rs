//! What both query paths share: the input plumbing (`prepare`, `threads`), origin
//! classification (`Origin`, `native_lib_meaning`), the search model (`Scope`, `Matcher`,
//! `Hit`, `Progress`) and the hit formatting (`Format`, `excerpt`, `json_str`,
//! `csv_field`). The stateless path (`quick`) and the indexed driver (`indexed`) both
//! build on this seam; `session` opens the APK around it.

mod api;
mod indexed;
mod secrets;

pub use api::api_sheet;
#[cfg(any(test, feature = "gui"))]
pub use indexed::host_count;
pub use indexed::{search, search_stats, search_summary};
pub use secrets::{Const, entropy_bits, entropy_secrets, looks_like_secret, string_constants};

use std::fmt::Write as _;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

use crate::bytecode;
use crate::cache;
use crate::dex::Dex;

/// Worker threads for indexing and searching (0 = all cores).
pub static THREADS: AtomicUsize = AtomicUsize::new(0);

/// Whether a full scan may write the decompiled-code cache.
pub static CACHE_WRITE: AtomicBool = AtomicBool::new(true);

pub fn threads() -> usize {
    match THREADS.load(Ordering::Relaxed) {
        0 => std::thread::available_parallelism().map_or(4, |n| n.get()),
        n => n,
    }
}

/// Where a top-level class comes from.
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Origin {
    /// the app's own package
    App,
    /// unknown or obfuscated packages: may well be app code
    Other,
    /// a known SDK or library
    Library,
}

/// Package prefixes of common Android SDKs and libraries (Java names, dot-terminated).
pub(crate) const LIBRARIES: &[&str] = &[
    "android.",
    "androidx.",
    "com.android.",
    "dalvik.",
    "java.",
    "javax.",
    "kotlin.",
    "kotlinx.",
    "org.jetbrains.",
    "org.intellij.",
    "com.google.",
    "com.squareup.",
    "okhttp3.",
    "okio.",
    "retrofit2.",
    "io.reactivex.",
    "rx.",
    "io.grpc.",
    "com.facebook.",
    "com.bumptech.",
    "dagger.",
    "hilt_aggregated_deps.",
    "com.fasterxml.",
    "org.json.",
    "org.apache.",
    "org.slf4j.",
    "ch.qos.",
    "com.airbnb.",
    "io.sentry.",
    "com.crashlytics.",
    "io.fabric.",
    "com.adjust.",
    "com.appsflyer.",
    "com.unity3d.",
    "com.applovin.",
    "com.ironsource.",
    "com.mopub.",
    "com.chartboost.",
    "com.vungle.",
    "com.inmobi.",
    "com.amazon.",
    "com.microsoft.",
    "org.chromium.",
    "io.flutter.",
    "com.facebook.react.",
    "org.greenrobot.",
    "net.sqlcipher.",
    "org.bouncycastle.",
    "org.conscrypt.",
    "com.tencent.",
    "com.huawei.",
    "io.branch.",
    "com.onesignal.",
    "com.revenuecat.",
    "io.realm.",
    "org.koin.",
    "coil.",
    "com.github.",
    "io.ktor.",
    "org.reactivestreams.",
    "com.jakewharton.",
    "timber.",
    "io.opentelemetry.",
    "com.braze.",
    "com.mixpanel.",
    "com.segment.",
    "com.stripe.",
    "com.paypal.",
    "com.yandex.",
    "com.bytedance.sdk.",
];

/// Origin of a class by its (displayed) Java name. App code is the manifest package's
/// organization (`com.unciv.app` -> `com.unciv`), since apps often keep code beside the
/// launcher package; known libraries are checked first.
pub fn origin(name: &str, package: &str) -> Origin {
    if LIBRARIES.iter().any(|p| name.starts_with(p)) {
        return Origin::Library;
    }
    let parts: Vec<&str> = package.split('.').collect();
    let root = if parts.len() >= 3 {
        parts[..2].join(".")
    } else {
        package.to_string()
    };
    if !root.is_empty() && name.starts_with(&root) && name[root.len()..].starts_with('.') {
        Origin::App
    } else {
        Origin::Other
    }
}

/// One-line meanings of well-known native libraries, shared by `info` and `flutter`. Small
/// and factual on purpose: one line per library, nothing for unknowns.
pub fn native_lib_meaning(path: &str) -> Option<&'static str> {
    let name = path.rsplit('/').next()?.to_ascii_lowercase();
    Some(match name.as_str() {
        "libpdfium.so" | "libmupdf.so" | "libmupdf_java.so" => "in-app PDF rendering",
        "libapp.so" => "Flutter Dart AOT snapshot",
        "libflutter.so" => "Flutter engine",
        "libunity.so" => "Unity engine",
        "libil2cpp.so" => "Unity IL2CPP (C# compiled to native code)",
        "libhermes.so" => "React Native Hermes JS engine",
        "libjsc.so" => "React Native JavaScriptCore engine",
        "libc++_shared.so" => "NDK C++ runtime",
        "libjiagu.so" => "packer: 360 Jiagu",
        "libnesec.so" => "packer: NetEase",
        "libsecshell.so" | "libsecexe.so" => "packer: Bangcle",
        "libmobisec.so" => "packer: Tencent Legu",
        "libtprt.so" => "Tencent X5 WebView",
        _ if name.starts_with("libjiagu_") => "packer: 360 Jiagu",
        _ if name.starts_with("libdexhelper") => "packer: SecNeo",
        _ if name.starts_with("libshella") => "packer: SecNeo",
        _ => return None,
    })
}

/// What apkscope opens, for file dialogs and the start screen.
pub const INPUTS: [&str; 6] = ["apk", "jar", "apks", "xapk", "apkm", "dex"];

/// Turn any supported input into an APK path: a `.jar` of DEX files (Android's framework JARs)
/// reads as one, split bundles (`.apks`, `.xapk`, `.apkm`) give their `base.apk`, a bare `.dex`
/// is wrapped in a minimal APK. Both land in the cache folder.
pub fn prepare(path: &str) -> Result<String, String> {
    use std::io::{Read, Write};
    let ext = std::path::Path::new(path)
        .extension()
        .map(|e| e.to_string_lossy().to_lowercase())
        .unwrap_or_default();
    let stem = std::path::Path::new(path)
        .file_stem()
        .map_or("input".into(), |s| s.to_string_lossy().into_owned());
    let dir = cache::root().join("inputs");
    match ext.as_str() {
        "apk" | "jar" => Ok(path.to_string()),
        "apks" | "xapk" | "apkm" => {
            // all three are plain ZIPs of split APKs; `.apkm` was encrypted before 2021,
            // but APKMirror now serves it as a regular ZIP, same as `.apks`/`.xapk`.
            let f = std::fs::File::open(path).map_err(|e| format!("{path}: {e}"))?;
            let mut zip = zip::ZipArchive::new(f).map_err(|e| format!("{path}: {e}"))?;
            let names: Vec<String> = zip
                .file_names()
                .filter_map(|n| n.ok().map(|n| n.into_owned()))
                .filter(|n| n.ends_with(".apk"))
                .collect();
            // the base split holds the code; fall back to the largest APK inside
            let base = names
                .iter()
                .find(|n| n.rsplit('/').next() == Some("base.apk"))
                .cloned()
                .or_else(|| {
                    names
                        .iter()
                        .max_by_key(|n| zip.by_name(n).map(|e| e.size()).unwrap_or(0))
                        .cloned()
                });
            let base = base.ok_or("no APK inside this bundle")?;
            let size = zip.by_name(&base).map_err(|e| e.to_string())?.size();
            let out = dir.join(format!("{stem}-{size:x}-base.apk"));
            if !out.exists() {
                std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
                let mut data = Vec::new();
                zip.by_name(&base)
                    .map_err(|e| e.to_string())?
                    .read_to_end(&mut data)
                    .map_err(|e| e.to_string())?;
                std::fs::write(&out, data).map_err(|e| e.to_string())?;
            }
            Ok(out.display().to_string())
        }
        "dex" => {
            let data = std::fs::read(path).map_err(|e| format!("{path}: {e}"))?;
            Dex::parse(&data).map_err(|e| format!("{path}: {e}"))?;
            let out = dir.join(format!("{stem}-{:x}.apk", data.len()));
            std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
            let mut w =
                zip::ZipWriter::new(std::fs::File::create(&out).map_err(|e| e.to_string())?);
            let opts = zip::write::SimpleFileOptions::default()
                .compression_method(zip::CompressionMethod::Stored);
            w.start_file("classes.dex", opts)
                .map_err(|e| e.to_string())?;
            w.write_all(&data).map_err(|e| e.to_string())?;
            w.finish().map_err(|e| e.to_string())?;
            Ok(out.display().to_string())
        }
        _ => Err(format!(
            "apkscope opens {}; not .{ext}",
            INPUTS.map(|e| format!(".{e}")).join(", ")
        )),
    }
}

pub const LIMIT: usize = 5000;

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Scope {
    Class,
    Code,
    String,
    Usages,
    Res,
    Urls,
    Http,
    Secrets,
    Api,
}

impl Scope {
    #[cfg(feature = "gui")]
    pub const ALL: [Scope; 9] = [
        Scope::Code,
        Scope::String,
        Scope::Usages,
        Scope::Class,
        Scope::Res,
        Scope::Urls,
        Scope::Http,
        Scope::Secrets,
        Scope::Api,
    ];

    #[cfg(feature = "gui")]
    pub fn label(self) -> &'static str {
        match self {
            Scope::Class => "Class names",
            Scope::Code => "Code",
            Scope::String => "String literals",
            Scope::Usages => "Usages (xref)",
            Scope::Res => "Resources",
            Scope::Urls => "URLs & endpoints",
            Scope::Http => "HTTP / API calls",
            Scope::Secrets => "Keys & secrets",
            Scope::Api => "HTTP API (Retrofit)",
        }
    }

    pub fn parse(s: &str) -> Option<Scope> {
        Some(match s {
            "class" => Scope::Class,
            "code" => Scope::Code,
            "string" => Scope::String,
            "usages" | "xref" => Scope::Usages,
            "res" => Scope::Res,
            "urls" => Scope::Urls,
            "http" => Scope::Http,
            "secrets" => Scope::Secrets,
            "api" => Scope::Api,
            _ => return None,
        })
    }

    /// API scopes list everything they find; the query only narrows them
    pub fn query_optional(self) -> bool {
        matches!(
            self,
            Scope::Urls | Scope::Http | Scope::Secrets | Scope::Api
        )
    }

    /// scopes that read decompiled classes
    pub fn decompiles(self) -> bool {
        !matches!(self, Scope::Class | Scope::Res)
    }

    /// What the scope can miss: shown next to the results.
    pub fn note(self) -> &'static str {
        match self {
            Scope::Code => {
                "Searches decompiled Java. Names are shown after deobfuscation, so search for what you see in the viewer."
            }
            Scope::String => {
                "Matches string literals as stored in the app. Strings built at runtime, encrypted or loaded from native code won't appear."
            }
            Scope::Usages => {
                "Finds classes that reference a method, field, class or string with this name, then the lines that mention it. Calls made via reflection, JNI or dynamic loading are not visible."
            }
            Scope::Class => "Matches class names (deobfuscated).",
            Scope::Res => {
                "Searches decoded XML resources and the manifest, plus resource file names."
            }
            Scope::Urls => {
                "Heuristic: finds URL-like string literals. Endpoints built at runtime, encrypted, downloaded from a config, or kept in native libraries won't be found, and some hits are documentation or ad URLs. App code only by default; --all includes library code."
            }
            Scope::Http => {
                "Heuristic: finds common networking APIs (Retrofit, OkHttp, HttpURLConnection). Apps using other libraries, their own wrappers, native code or obfuscated networking may not show up, and library code shows up too. App code only by default; --all includes library code."
            }
            Scope::Secrets => {
                "Heuristic: finds well-known key formats and key-like names, plus a labelled lower-confidence entropy tier of the app's own string constants. A hit is not proof of a leaked secret (many keys are meant to be public), and keys that are encrypted, split or fetched at runtime won't be found. App code only by default; --all includes library code."
            }
            Scope::Api => {
                "Retrofit-annotated interfaces only: verb, path, parameters and base URL read from the annotations. Generic return types are erased in the DEX, so response models and call-site arguments are inferred from the interface's callers and labelled as such. Dynamic paths and non-Retrofit HTTP calls don't show up (see urls / http)."
            }
        }
    }
}

pub(crate) const URL_RE: &str = r#""((?:https?|wss?)://[^"\s]+|/(?:api|v\d+|rest|graphql|auth|oauth|user|users|login)\b[^"\s]*)""#;

const HTTP_RE: &str = r"@(GET|POST|PUT|DELETE|PATCH|HEAD|OPTIONS|HTTP|Headers?|Query|QueryMap|Path|Body|Field|FieldMap|FormUrlEncoded|Multipart|Part|Url)\b|\bRequest\.Builder|\.url\(|\.newCall\(|HttpURLConnection|openConnection\(|\.addHeader\(|\.header\(|Retrofit\.Builder|\.baseUrl\(|setRequestMethod\(|HttpUrl\.(parse|get)\(|WebSocket";

pub(crate) const SECRET_RE: &str = r#"AIza[0-9A-Za-z_\-]{35}|AKIA[0-9A-Z]{16}|(sk|pk|rk)_(live|test)_[0-9A-Za-z]{16,}|gh[pousr]_[0-9A-Za-z]{36}|xox[baprs]-[0-9A-Za-z\-]{10,}|eyJ[0-9A-Za-z_\-]{10,}\.[0-9A-Za-z_\-]{10,}|(?i)"(authorization|bearer ?|x-api-key|api[_-]?key|x-auth-token|client[_-]?secret|access[_-]?token)"|(?i)\b\w*(api_?key|secret|token|password|passwd|client_?id|private_?key|access_?key|auth_?header|sign_?key|signing_?salt|hmac_?key|hash_?key|encryption_?key|license_?key|merchant_?id|partner_?code)\w*\s*=\s*"[^"]{6,}"|(?i)name="[^"]*(api_?key|secret|token|client_?id|app_?id|private_?key|access_?key|auth_?header|sign_?key|signing_?salt|hmac_?key|hash_?key|encryption_?key|license_?key|merchant_?id|partner_?code)[^"]*">[^<\s]{6,}<"#;

/// What one search looks for: an optional user query plus the scope's own pattern.
pub struct Matcher {
    pub scope: Scope,
    /// skip classes from known libraries
    pub app_only: bool,
    query: Option<regex::Regex>,
    builtin: Option<regex::Regex>,
}

impl Matcher {
    pub fn new(q: &str, scope: Scope, use_regex: bool, case: bool) -> Result<Matcher, String> {
        let query = if q.is_empty() {
            None
        } else {
            let pat = if use_regex {
                q.to_string()
            } else {
                regex::escape(q)
            };
            Some(
                regex::RegexBuilder::new(&pat)
                    .case_insensitive(!case)
                    .build()
                    .map_err(|e| e.to_string())?,
            )
        };
        let builtin = match scope {
            Scope::Urls => Some(URL_RE),
            Scope::Http => Some(HTTP_RE),
            Scope::Secrets => Some(SECRET_RE),
            _ => None,
        }
        .map(|p| regex::Regex::new(p).unwrap());
        Ok(Matcher {
            scope,
            app_only: false,
            query,
            builtin,
        })
    }

    fn query_hits(&self, s: &str) -> bool {
        self.query.as_ref().is_none_or(|q| q.is_match(s))
    }

    pub fn line(&self, l: &str) -> bool {
        if self.builtin.is_some() && l.trim_start().starts_with("import ") {
            return false;
        }
        match (&self.builtin, self.scope) {
            // XML namespaces and spec URLs are identifiers, not endpoints
            (Some(b), Scope::Urls) => {
                const NOISE: [&str; 6] = [
                    "schemas.android.com",
                    "www.w3.org",
                    "ns.adobe.com",
                    "www.apache.org/licenses",
                    "xmlpull.org",
                    "schemas.xmlsoap.org",
                ];
                if !b
                    .captures_iter(l)
                    .any(|c| !NOISE.iter().any(|n| c[1].contains(n)))
                {
                    return false;
                }
            }
            (Some(b), _) if !b.is_match(l) => return false,
            _ => {}
        }
        match (&self.query, self.scope) {
            (None, _) => true,
            // only inside a string literal
            (Some(q), Scope::String) => l.split('"').skip(1).step_by(2).any(|lit| q.is_match(lit)),
            (Some(q), _) => q.is_match(l),
        }
    }
}

/// (target: class or resource path, line, text)
pub type Hit = (String, usize, String);

/// Shared with the caller: how far a search got, and a flag to stop it.
#[derive(Default)]
pub struct Progress {
    pub done: AtomicUsize,
    pub total: AtomicUsize,
    pub cancel: AtomicBool,
}

#[derive(Clone, Copy, PartialEq)]
pub enum Format {
    Text,
    Csv,
    Json,
}

/// Lines `[from, to)` around a hit: `ctx` lines either side, or the whole enclosing method.
pub fn excerpt(lines: &[&str], at: usize, ctx: usize, method: bool) -> (usize, usize) {
    let indent = |l: &str| l.len() - l.trim_start().len();
    if method {
        const NOT_METHOD: [&str; 9] = [
            "if ",
            "for ",
            "while ",
            "switch ",
            "try ",
            "catch ",
            "} catch",
            "else",
            "synchronized ",
        ];
        let hit_ind = indent(lines.get(at).copied().unwrap_or(""));
        let head = (0..=at.min(lines.len().saturating_sub(1)))
            .rev()
            .find(|&k| {
                let l = lines[k];
                let t = l.trim();
                indent(l) < hit_ind.max(1)
                    && t.ends_with('{')
                    && t.contains('(')
                    && !NOT_METHOD.iter().any(|n| t.starts_with(n))
            });
        if let Some(h) = head {
            let ind = indent(lines[h]);
            let end = (h + 1..lines.len())
                .find(|&k| indent(lines[k]) == ind && lines[k].trim() == "}")
                .map_or(lines.len(), |e| e + 1);
            return (h, end);
        }
    }
    (at.saturating_sub(ctx), (at + ctx + 1).min(lines.len()))
}

pub fn json_str(s: &str) -> String {
    let mut o = String::with_capacity(s.len() + 2);
    o.push('"');
    for c in s.chars() {
        match c {
            '"' => o += "\\\"",
            '\\' => o += "\\\\",
            '\n' => o += "\\n",
            '\r' => o += "\\r",
            '\t' => o += "\\t",
            c if (c as u32) < 0x20 => {
                let _ = write!(o, "\\u{:04x}", c as u32);
            }
            c => o.push(c),
        }
    }
    o.push('"');
    o
}

pub fn csv_field(s: &str) -> String {
    format!("\"{}\"", s.replace('"', "\"\""))
}

/// `name="value"` of the first `name=` in `s`.
pub fn attr(s: &str, name: &str) -> Option<String> {
    let k = s.find(&format!("{name}=\""))? + name.len() + 2;
    Some(s[k..].split('"').next()?.to_string())
}

/// Smali of class `c` and its nested classes, outer first.
pub fn smali(dx: &Dex, c: usize) -> String {
    let mut out = String::new();
    let mut stack = vec![c];
    while let Some(x) = stack.pop() {
        let _ = writeln!(
            out,
            ".class {}
",
            dx.class_descriptor(x).unwrap_or_default()
        );
        for m in dx.methods(x).unwrap_or_default() {
            let _ = writeln!(
                out,
                ".method {:#x} {}",
                m.access,
                dx.method_ref(m.method_idx).unwrap_or_default()
            );
            if let Some(code) = &m.code {
                let _ = writeln!(out, "    .registers {}", code.registers);
                for i in bytecode::decode(code.insns).unwrap_or_default() {
                    let _ = writeln!(
                        out,
                        "    {:04x}: {}",
                        i.off,
                        crate::render(dx, &i).unwrap_or_else(|e| e.to_string())
                    );
                }
            }
            out += ".end method

";
        }
        stack.extend(dx.nesting().kids[x].iter().rev());
    }
    out
}

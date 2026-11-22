//! Named string constants and the secrets entropy tier: every static `String` field
//! with a stored value, and the token-shaped, high-entropy constants of the app's own
//! code that no known pattern matched.

use crate::dex::{self, Dex};
use crate::session::State;

use super::{Origin, SECRET_RE};

/// One static string constant as stored in the DEX: `Class#field = "value"`.
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Debug)]
pub struct Const {
    /// class, Java name (`a.b.C` or a nested `a.b.C$D`)
    pub class: String,
    pub field: String,
    pub value: String,
}

/// Every static `String` field with a stored initial value, sorted and de-duplicated. The
/// stored values line up with the static-field prefix of each class's field list, so this is
/// the exact `BuildConfig.X = "..."` set — read from the DEX tables, not from decompiled
/// text. Shared by the secrets entropy tier, `search config` and the constants diff.
pub fn string_constants(dexes: &[Dex]) -> Vec<Const> {
    let mut out = Vec::new();
    for dx in dexes {
        for c in 0..dx.class_count {
            let Ok(desc) = dx.class_descriptor(c) else {
                continue;
            };
            let Ok(data) = dx.class_data(c) else { continue };
            let vals = dx.static_values(c).unwrap_or_default();
            for (k, (f, acc)) in data.fields.iter().enumerate() {
                // static fields come first; the stored values match that prefix of the list
                if acc & 0x8 == 0 {
                    break;
                }
                if let (Some(dex::Value::Str(s)), Ok((_, _, name))) =
                    (vals.get(k), dx.field_parts(*f))
                    && let Ok(value) = dx.string(*s)
                {
                    out.push(Const {
                        class: dex::java_name(&desc),
                        field: name,
                        value,
                    });
                }
            }
        }
    }
    out.sort();
    out.dedup();
    out
}

/// Shannon entropy of `s`, in bits per character.
pub fn entropy_bits(s: &str) -> f64 {
    let mut counts = [0u32; 256];
    let mut n = 0u64;
    for b in s.as_bytes() {
        counts[*b as usize] += 1;
        n += 1;
    }
    if n == 0 {
        return 0.0;
    }
    counts
        .into_iter()
        .filter(|c| *c > 0)
        .map(|c| {
            let p = c as f64 / n as f64;
            -p * p.log2()
        })
        .sum()
}

/// A string that looks like a first-party key while no pattern named it: token-shaped —
/// letters, digits and `+/=_-.:` only, which is what drops regexes, alphabets-in-prose and
/// multi-line shaders — 24–200 characters long, high-entropy, mixing character classes,
/// and not shaped like a URL or path. Long ascending or descending runs (`abcdef`,
/// `0123456789`) are alphabet fragments, not key material, and are rejected too.
pub fn looks_like_secret(v: &str) -> bool {
    (24..=200).contains(&v.chars().count())
        && v.bytes().all(|b| {
            b.is_ascii_alphanumeric() || matches!(b, b'+' | b'/' | b'=' | b'_' | b'-' | b'.' | b':')
        })
        && entropy_bits(v) >= 4.0
        && v.chars().any(|c| c.is_ascii_lowercase())
        && v.chars().any(|c| c.is_ascii_uppercase())
        && v.chars().any(|c| c.is_ascii_digit())
        && !v.contains("://")
        && !v.starts_with('/')
        && !sequential_run(v)
}

/// Whether `v` contains a run of 5 or more consecutive code points in either direction
/// (`abcdef`, `0123456789`, `XWVUT`): digit-sequence or alphabet fragments, which real
/// key material does not contain.
fn sequential_run(v: &str) -> bool {
    let mut run = 1u32;
    let mut prev = 0u32;
    for (i, c) in v.chars().map(|c| c as u32).enumerate() {
        if i > 0 {
            run = if c == prev + 1 || c + 1 == prev {
                run + 1
            } else {
                1
            };
            if run >= 5 {
                return true;
            }
        }
        prev = c;
    }
    false
}

/// The entropy tier of `search secrets`: static string constants of the app's own code
/// (library classes excluded) that look like keys while no known pattern matched them.
/// A separate, lower-confidence list — capped, sorted, and never mixed with the pattern
/// hits; values the patterns already report on their `name = "value"` line are left out.
pub fn entropy_secrets(st: &State) -> Vec<Const> {
    let known = regex::Regex::new(SECRET_RE).unwrap();
    let mut out = Vec::new();
    for c in string_constants(&st.dexes) {
        if st.origin_of(&c.class) == Origin::Library || !looks_like_secret(&c.value) {
            continue;
        }
        if known.is_match(&format!(r#"{} = "{}""#, c.field, c.value)) {
            continue;
        }
        out.push(c);
        if out.len() >= 50 {
            break;
        }
    }
    out
}

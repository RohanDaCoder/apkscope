//! The body a `<return>` leaves behind: [`stub`] writes a method body that returns one
//! constant, typed by the method's return descriptor — the narrowest `const` instruction
//! that holds the value, `.locals` for the registers it uses, and the matching `return`
//! instruction.

/// The body of a method stubbed to `return <value>`, typed by the method's return descriptor:
/// the header line and `.end method` are the caller's, this is what goes between. `.locals`
/// counts what the stub needs — wide values take a register pair.
pub fn stub(ret: &str, value: Option<&str>, at: &str) -> Result<String, String> {
    let err = |m: String| Err(format!("{at}: {m}"));
    // (instructions, locals)
    let body: (Vec<String>, usize) = match ret {
        "V" => match value {
            None => (vec!["return-void".into()], 0),
            Some(v) => return err(format!("a void method returns nothing (value={v:?})")),
        },
        "Z" => {
            let lit = match value {
                None | Some("false") => 0,
                Some("true") => 1,
                Some(v) => return err(format!("a boolean returns true or false, not {v:?}")),
            };
            (vec![format!("const/4 v0, 0x{lit}"), "return v0".into()], 1)
        }
        "B" | "S" | "C" | "I" => {
            let (lo, hi, what) = match ret {
                "B" => (-128i64, 127, "a byte"),
                "S" => (-0x8000, 0x7fff, "a short"),
                "C" => (0, 0xffff, "a char"),
                _ => (i32::MIN as i64, i32::MAX as i64, "an int"),
            };
            // no value: the type's zero, like `return 0`
            let v = match value {
                None => 0,
                Some(v) => number(v).map_err(|e| format!("{at}: {e}"))?,
            };
            if v < lo || v > hi {
                return err(format!("{what} holds {lo} to {hi}, not {v}"));
            }
            (vec![i32_const(v as i32), "return v0".into()], 1)
        }
        "J" => {
            let v = match value {
                None => 0,
                Some(v) => number(v).map_err(|e| format!("{at}: {e}"))?,
            };
            (vec![wide_const(v), "return-wide v0".into()], 2)
        }
        "F" => {
            let bits = f32_bits(value)?;
            (
                vec![
                    match bits {
                        0 => "const/4 v0, 0x0".into(),
                        v => format!("const v0, 0x{v:08x}"),
                    },
                    "return v0".into(),
                ],
                1,
            )
        }
        "D" => {
            let bits = f64_bits(value)?;
            (
                vec![
                    match bits {
                        0 => "const-wide/16 v0, 0x0".into(),
                        v => format!("const-wide v0, 0x{v:016x}"),
                    },
                    "return-wide v0".into(),
                ],
                2,
            )
        }
        t if t.starts_with('L') || t.starts_with('[') => match value {
            None | Some("null") => (vec!["const/4 v0, 0x0".into(), "return-object v0".into()], 1),
            Some(v) => {
                return err(format!(
                    "{t} is a reference type: value can only be null, not {v:?}"
                ));
            }
        },
        t => return err(format!("{t} is not a return type")),
    };
    let (lines, locals) = body;
    let mut out = format!("    .locals {locals}\n");
    for l in lines {
        out.push_str("    ");
        out.push_str(&l);
        out.push('\n');
    }
    Ok(out)
}

/// `value` as the bits of a float: a decimal number, or 0x hex as raw bits. Absent: 0.0.
fn f32_bits(value: Option<&str>) -> Result<u32, String> {
    Ok(match value {
        None => 0,
        Some(v) => match v.starts_with("0x") {
            true => number(v)? as u32,
            false => v
                .parse::<f32>()
                .map_err(|e| format!("{v:?} is not a float: {e}"))?
                .to_bits(),
        },
    })
}

/// `value` as the bits of a double: a decimal number, or 0x hex as raw bits. Absent: 0.0.
fn f64_bits(value: Option<&str>) -> Result<u64, String> {
    Ok(match value {
        None => 0,
        Some(v) => match v.starts_with("0x") {
            true => number(v)? as u64,
            false => v
                .parse::<f64>()
                .map_err(|e| format!("{v:?} is not a double: {e}"))?
                .to_bits(),
        },
    })
}

/// A smali-style integer: decimal or `0x` hex, optional leading minus.
fn number(v: &str) -> Result<i64, String> {
    let (neg, v) = match v.strip_prefix('-') {
        Some(r) => (true, r),
        None => (false, v),
    };
    let n = match v.strip_prefix("0x").or_else(|| v.strip_prefix("0X")) {
        Some(h) => u64::from_str_radix(h, 16).map_err(|_| format!("{v:?} is not a number"))?,
        None => v
            .parse::<u64>()
            .map_err(|_| format!("{v:?} is not a number"))?,
    };
    Ok(match neg {
        true => (n as i64).wrapping_neg(),
        false => n as i64,
    })
}

/// The narrowest `const` that holds an int.
fn i32_const(v: i32) -> String {
    match v {
        -8..=7 => format!("const/4 v0, {}", lit(v as i64)),
        -0x8000..=0x7fff => format!("const/16 v0, {}", lit(v as i64)),
        _ => format!("const v0, {}", lit(v as i64)),
    }
}

/// The narrowest `const-wide` that holds a long.
fn wide_const(v: i64) -> String {
    match v {
        -0x8000..=0x7fff => format!("const-wide/16 v0, {}", lit(v)),
        _ => format!("const-wide v0, {}", lit(v)),
    }
}

/// A literal as smali writes it: `0x2a`, or `-0x472f5021`.
fn lit(v: i64) -> String {
    match v {
        0.. => format!("0x{v:x}"),
        _ => format!("-0x{:x}", v.unsigned_abs()),
    }
}

#[cfg(test)]
#[path = "../../tests/apkscope/patch_stub.rs"]
mod tests;

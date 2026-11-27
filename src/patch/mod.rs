//! XML patch bundles: one file that describes every edit an APK needs.
//!
//! A bundle is what `apkscope patch` applies. It says which app it is for, and holds named
//! patches, picked from a list, each a list of edits: `return` stubs a method to return a
//! constant, `replace` swaps smali text inside a method — a piece of a line, or a block of
//! lines — `body` writes a whole new method body in smali, and `any` holds alternatives of
//! which the first that lands is applied, so one patch can follow an app across versions.
//! `string`, `manifest` and `entry` edit resources, the manifest and the zip itself. The XML
//! is read by the same parser the text resource forms use ([`apkscope_core::markup`]); the
//! code edits are applied to the class's own smali and re-assembled from it, so a bundle
//! edits readable smali text and the assembler checks what it becomes — never raw DEX hex.
//!
//! The parts, one file each: [`bundle`] reads the XML into [`Bundle`] (format and parse
//! checks), [`target`] holds what a bundle says it is for and checks it against the APK,
//! [`smali`] finds and edits method bodies in a class's smali text, and [`stub`] writes the
//! body a `<return>` leaves behind. Everything flows through the re-exports below, so callers
//! outside see the one `crate::patch` surface and no `use` line changes when the parts move.

mod bundle;
mod smali;
mod stub;
mod target;

pub use self::bundle::{Bundle, Op, Patch, parse};
pub use self::smali::{At, bodies, replaces, returns};
pub use self::target::{Target, manifest_id};

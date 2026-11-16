//! `resources.arsc` as a model: [`read`] it, change it, and [`write`] it back. Writing what was
//! read gives the same file, so an edit changes only what it touches.
//!
//! Format: `frameworks/base/libs/androidfw/include/androidfw/ResourceTypes.h` in AOSP.

pub mod config;
mod edit;
pub mod read;
pub mod write;

pub use read::read;
pub use write::write;

#[derive(Clone, Debug, PartialEq)]
pub struct Table {
    /// The global pool: string values, and the names of style spans.
    pub strings: StringPool,
    pub packages: Vec<Package>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct StringPool {
    pub utf8: bool,
    pub sorted: bool,
    pub strings: Vec<String>,
    /// Style spans of the first strings, in order: one list per styled string.
    pub styles: Vec<Vec<Span>>,
}

/// Markup over a string's characters `first..=last` (UTF-16 units), named by a string in the
/// same pool: `b`, `i`, `font;size=12`, ...
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Span {
    pub name: u32,
    pub first: u32,
    pub last: u32,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Package {
    /// `0x7f` for an app, `0x01` for the framework.
    pub id: u32,
    pub name: String,
    pub last_public_type: u32,
    pub last_public_key: u32,
    pub type_id_offset: u32,
    /// Type names: `attr`, `drawable`, `string`, ...; type `n` is `types[n - 1]`.
    pub types: StringPool,
    /// Entry names.
    pub keys: StringPool,
    /// The package's chunks in file order.
    pub chunks: Vec<Chunk>,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Chunk {
    Spec(Spec),
    Type(Type),
    /// Kept as read: the library, overlayable and staged-alias chunks.
    Other(Vec<u8>),
}

/// Which configurations each entry of a type varies by, and whether it is public.
#[derive(Clone, Debug, PartialEq)]
pub struct Spec {
    pub id: u8,
    pub res0: u8,
    /// Since Android 13, the number of the type's [`Type`] chunks.
    pub res1: u16,
    pub flags: Vec<u32>,
}

/// A type's entries for one configuration.
#[derive(Clone, Debug, PartialEq)]
pub struct Type {
    pub id: u8,
    /// [`Type::SPARSE`], [`Type::OFFSET16`]: how the entries are indexed.
    pub flags: u8,
    pub reserved: u16,
    /// `ResTable_config`, as read.
    pub config: Vec<u8>,
    /// By entry index; a sparse type has only the entries that are there.
    pub entries: Vec<Option<Entry>>,
}

impl Type {
    pub const SPARSE: u8 = 0x01;
    pub const OFFSET16: u8 = 0x02;
}

#[derive(Clone, Debug, PartialEq)]
pub struct Entry {
    /// [`Entry::COMPLEX`], [`Entry::PUBLIC`], [`Entry::WEAK`], [`Entry::COMPACT`].
    pub flags: u16,
    /// Index into the package's [`Package::keys`].
    pub key: u32,
    pub value: EntryValue,
}

impl Entry {
    pub const COMPLEX: u16 = 0x01;
    pub const PUBLIC: u16 = 0x02;
    pub const WEAK: u16 = 0x04;
    /// Stored in 8 bytes: the key in the size field, the value's type in the flags' high byte.
    pub const COMPACT: u16 = 0x08;
}

#[derive(Clone, Debug, PartialEq)]
pub enum EntryValue {
    Simple(Value),
    /// A style, array, plural or attribute: its parent (0 for none) and its items by name.
    Complex {
        parent: u32,
        map: Vec<(u32, Value)>,
    },
}

/// `Res_value`: a type and its data.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Value {
    pub ty: u8,
    pub data: u32,
}

impl Value {
    pub const NULL: u8 = 0x00;
    pub const REFERENCE: u8 = 0x01;
    pub const ATTRIBUTE: u8 = 0x02;
    /// An index into the table's global [`Table::strings`].
    pub const STRING: u8 = 0x03;
    pub const FLOAT: u8 = 0x04;
    pub const DIMENSION: u8 = 0x05;
    pub const FRACTION: u8 = 0x06;
    pub const DYNAMIC_REFERENCE: u8 = 0x07;
    pub const DYNAMIC_ATTRIBUTE: u8 = 0x08;
    pub const INT_DEC: u8 = 0x10;
    pub const INT_HEX: u8 = 0x11;
    /// 0 or `0xffffffff`.
    pub const INT_BOOLEAN: u8 = 0x12;
    pub const INT_COLOR_ARGB8: u8 = 0x1c;
    pub const INT_COLOR_RGB8: u8 = 0x1d;
    pub const INT_COLOR_ARGB4: u8 = 0x1e;
    pub const INT_COLOR_RGB4: u8 = 0x1f;
}

#[cfg(test)]
#[path = "../../../tests/apkscope-core/table.rs"]
mod tests;

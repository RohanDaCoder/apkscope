//! Binary XML (a compiled `AndroidManifest.xml`, layout, drawable, ...) as a model: [`read`]
//! it, change it, and [`write`] it back. Writing what was read gives the same file.
//!
//! Format: `ResXMLTree` in `frameworks/base/libs/androidfw/include/androidfw/ResourceTypes.h`.

pub mod read;
pub mod text;
pub mod write;

pub use read::read;
pub use write::write;

use crate::table::{StringPool, Value};

/// No string, in a node's `comment` or an attribute's `raw` value.
pub const NONE: u32 = u32::MAX;

#[derive(Clone, Debug, PartialEq)]
pub struct Document {
    /// Every name, namespace and string value the document uses.
    pub strings: StringPool,
    /// The resource ID of each of the first strings, for attribute names: `android:layout_width`
    /// is named by both its string and `0x010100f4`.
    pub ids: Vec<u32>,
    /// The document's nodes, in order.
    pub nodes: Vec<Node>,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Node {
    StartNamespace(Namespace),
    EndNamespace(Namespace),
    Start(Start),
    End(End),
    Text(Text),
    /// Kept as read.
    Other(Vec<u8>),
}

/// Where a node was in the source, and the comment before it ([`NONE`] for none).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Source {
    pub line: u32,
    pub comment: u32,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Namespace {
    pub source: Source,
    pub prefix: u32,
    pub uri: u32,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Start {
    pub source: Source,
    /// The namespace URI ([`NONE`] for none) and name.
    pub ns: u32,
    pub name: u32,
    /// One more than the index of the `id`, `class` and `style` attributes, 0 if there is none.
    pub id_index: u16,
    pub class_index: u16,
    pub style_index: u16,
    pub attrs: Vec<Attribute>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Attribute {
    pub ns: u32,
    pub name: u32,
    /// The value as written, for a string ([`NONE`] otherwise).
    pub raw: u32,
    pub value: Value,
}

#[derive(Clone, Debug, PartialEq)]
pub struct End {
    pub source: Source,
    pub ns: u32,
    pub name: u32,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Text {
    pub source: Source,
    pub data: u32,
    pub value: Value,
    /// The typed value's size field: 8, or 0 where an older tool left the value empty.
    pub value_size: u16,
}

#[cfg(test)]
#[path = "../../../tests/apkscope-core/xml.rs"]
mod tests;

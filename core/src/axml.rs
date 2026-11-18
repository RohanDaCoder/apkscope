//! Text XML -> Android binary XML (the format of a compiled AndroidManifest.xml), so an edited
//! manifest can go back into an APK. With the original file, whatever the text leaves as it
//! was stays exactly as it is there, so `versionName="1"` stays a string.

use crate::Result;
use crate::res::Names;
use crate::table::StringPool;
use crate::xml::{self, Document};

pub fn encode(text: &str, original: Option<&[u8]>, names: &Names) -> Result<Vec<u8>> {
    let original = match original {
        Some(bytes) => xml::read(bytes)?,
        None => Document {
            strings: StringPool {
                utf8: true,
                sorted: false,
                strings: vec![],
                styles: vec![],
            },
            ids: vec![],
            nodes: vec![],
        },
    };
    Ok(xml::write(&xml::text::apply(&original, text, names)?))
}

#[cfg(test)]
#[path = "../../tests/apkscope-core/axml.rs"]
mod tests;

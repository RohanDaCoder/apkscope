//! An open APK's files, laid out as apktool lays them out: `AndroidManifest.xml` and binary
//! XML under `res/` as text, the resource table's values as values XML (`res/values/strings.xml`),
//! a file resource at the path its type, name and configuration give it
//! (`res/layout/main.xml`, which may be `res/Wb5.xml` in an APK whose resource files were
//! renamed), every other entry as it is. A file is decoded when it is read, and one that is
//! put changes only what it is: a values file the values of its types in its configuration,
//! a new file under `res/` adds its resource.

use crate::apk::{Apk, is_dex};
use crate::res::{self, Names};
use crate::table::{self, EntryValue, Table, Value, config};
use crate::{Error, Result, rebuild, values, xml};
use std::collections::{BTreeMap, BTreeSet, HashSet};
use std::io::{Read, Seek};

const MANIFEST: &str = "AndroidManifest.xml";
const TABLE: &str = "resources.arsc";
/// The IDs of the table's resources, and those new ones are to get.
const PUBLIC: &str = "res/values/public.xml";

/// The resource table of an open APK and what is read from it, with the edits that wait for the
/// table to be complete.
pub(crate) struct Resources {
    /// The table as the edits left it, if the APK has one.
    table: Option<Table>,
    /// Whether an edit reached the table.
    changed: bool,
    /// The manifest's `minSdkVersion`.
    min_sdk: u16,
    /// What the table names, until an edit reaches it.
    view: Option<View>,
    /// XML put as text, by entry: compiled when the APK is written, when the table names
    /// everything the edits added.
    texts: BTreeMap<String, String>,
    /// `res/values/public.xml` as it was put.
    public: Option<String>,
}

struct View {
    names: Names,
    /// A file resource's entry in the APK, by its path here.
    files: BTreeMap<String, String>,
    /// The values files, by path, each with the chunks of the table it is made of.
    values: BTreeMap<String, Vec<usize>>,
}

fn no_table() -> Error {
    Error::EntryNotFound(TABLE.into())
}

fn empty_pool() -> table::StringPool {
    table::StringPool {
        utf8: true,
        sorted: false,
        strings: vec![],
        styles: vec![],
    }
}

/// The path of a values file under `res/`: `values-night/colors.xml`.
fn values_file(path: &str) -> Option<&str> {
    path.strip_prefix("res/")
        .filter(|p| p.starts_with("values"))
}

impl Resources {
    fn view(&self) -> &View {
        self.view.as_ref().expect("read before it is asked for")
    }

    /// The table, to change.
    fn table_mut(&mut self) -> Result<&mut Table> {
        (self.changed, self.view) = (true, None);
        self.table.as_mut().ok_or_else(no_table)
    }

    fn read_view(&mut self) {
        let none = Table {
            strings: empty_pool(),
            packages: vec![],
        };
        let table = self.table.as_ref().unwrap_or(&none);
        let names = Names::new(table);
        let layout = res::layout(table, &names);
        let under = |path: String| format!("res/{path}");
        let files = layout.files.into_iter();
        let values = layout.values.into_iter();
        self.view = Some(View {
            files: files.map(|(path, entry)| (under(path), entry)).collect(),
            values: values.map(|(path, chunks)| (under(path), chunks)).collect(),
            names,
        });
    }

    /// The values file at `path`, as text.
    fn values(&self, path: &str) -> Option<String> {
        let (table, view) = (self.table.as_ref()?, self.view());
        match path {
            PUBLIC => res::public(table, &view.names),
            _ => Some(res::values_file(table, &view.names, view.values.get(path)?)),
        }
    }

    /// Where a file at `path`, `res/<type>[-<qualifiers>]/<name>.<ext>`, puts its resource in
    /// the app package: its type, its name and the configuration its directory names.
    fn file_resource(&self, path: &str) -> Result<(String, String, Vec<u8>)> {
        let bad = || Error::resources(format!("{path}: not res/<type>/<file>"));
        let (dir, file) = path
            .strip_prefix("res/")
            .and_then(|p| p.split_once('/'))
            .filter(|(_, file)| !file.contains('/'))
            .ok_or_else(bad)?;
        let (ty, qualifiers) = dir.split_once('-').unwrap_or((dir, ""));
        let name = file
            .split('.')
            .next()
            .filter(|n| !n.is_empty())
            .ok_or_else(bad)?;
        let p = self.table.as_ref().and_then(Table::app);
        let config = p
            .ok_or_else(no_table)?
            .config(qualifiers, self.min_sdk)
            .map_err(|e| e.at(path))?;
        Ok((ty.to_owned(), name.to_owned(), config))
    }

    /// Make the new file at `path` a resource. The entry it goes in, named as aapt2 names it,
    /// by the configuration it is in; whether it is XML aapt2 compiles, which is all but
    /// `raw/`; and the entry of the file the resource had there, which it no longer has.
    fn add_file(&mut self, path: &str) -> Result<(String, bool, Option<String>)> {
        let (ty, name, config) = self.file_resource(path)?;
        let file = path.rsplit('/').next().expect("a file name");
        let entry = match config::name(&config).as_str() {
            "" => format!("res/{ty}/{file}"),
            q => format!("res/{ty}-{q}/{file}"),
        };
        let t = self.table_mut()?;
        let value = EntryValue::Simple(Value {
            ty: Value::STRING,
            data: t.strings.intern(&entry),
        });
        let strings = t.strings.strings.clone();
        let p = t.app_mut().expect("the app package");
        let mut replaced = None;
        match p.find(&ty, &name) {
            Some(id) => {
                if let Some(table::Entry {
                    value: EntryValue::Simple(old),
                    ..
                }) = p.get(id, &config)
                    && old.ty == Value::STRING
                {
                    replaced = strings.get(old.data as usize).cloned();
                }
                p.set(id, &config, value)?;
            }
            None => {
                p.add(&ty, &name, &config, value)?;
            }
        }
        let replaced = replaced.filter(|old| old.starts_with("res/") && *old != entry);
        Ok((entry, file.ends_with(".xml") && ty != "raw", replaced))
    }

    /// Take from the table the resource the file at `path` is.
    fn remove_file(&mut self, path: &str) -> Result<()> {
        let (ty, name, config) = self.file_resource(path)?;
        let p = self.table_mut()?.app_mut().expect("the app package");
        if let Some(id) = p.find(&ty, &name) {
            p.remove(id, &config);
        }
        Ok(())
    }
}

/// The manifest's `minSdkVersion`: 0 if it names none, and for a preview's codename the
/// version aapt2 gives that, 10000.
fn min_sdk<R: Read + Seek>(apk: &mut Apk<R>) -> u16 {
    const MIN_SDK_VERSION: u32 = 0x0101_020c;
    let Some(doc) = apk.read(MANIFEST).and_then(|m| xml::read(&m).ok()) else {
        return 0;
    };
    doc.nodes
        .iter()
        .filter_map(|n| match n {
            xml::Node::Start(s) => Some(s),
            _ => None,
        })
        .filter(|s| {
            doc.strings
                .strings
                .get(s.name as usize)
                .is_some_and(|n| n == "uses-sdk")
        })
        .flat_map(|s| &s.attrs)
        .find(|a| doc.ids.get(a.name as usize) == Some(&MIN_SDK_VERSION))
        .map_or(0, |a| match a.value.ty {
            table::Value::INT_DEC | table::Value::INT_HEX => a.value.data as u16,
            _ => 10000,
        })
}

impl<R: Read + Seek> Apk<R> {
    /// The APK's resources, read if they are not yet.
    fn resources(&mut self) -> Result<&mut Resources> {
        if self.resources.is_none() {
            let table = self.read(TABLE).map(|d| table::read(&d)).transpose()?;
            self.resources = Some(Resources {
                table,
                changed: false,
                min_sdk: min_sdk(self),
                view: None,
                texts: BTreeMap::new(),
                public: None,
            });
        }
        Ok(self.resources.as_mut().expect("read above"))
    }

    /// The APK's resources, with what the table names as the edits left it.
    fn viewed(&mut self) -> Result<&mut Resources> {
        let resources = self.resources()?;
        if resources.view.is_none() {
            resources.read_view();
        }
        Ok(resources)
    }

    /// The paths of the APK's files, as the edits left them: every entry but the DEX files and
    /// the resource table, a file resource at the path the table gives it, and the table's
    /// values files.
    pub fn files(&mut self) -> Result<Vec<String>> {
        self.viewed()?;
        let resources = self.resources.as_ref().expect("read above");
        let view = resources.view();
        let there = |name: &str| resources.texts.contains_key(name) || self.has(name);
        let named: HashSet<&str> = view.files.values().map(String::as_str).collect();
        let entries = self
            .zip
            .file_names()
            .flatten()
            .map(|name| name.into_owned());
        let added = self.entries.keys().chain(resources.texts.keys()).cloned();
        let mut files: BTreeSet<String> = entries
            .chain(added)
            .filter(|name| !name.ends_with('/') && !is_dex(name) && name != TABLE)
            .filter(|name| !named.contains(name.as_str()) && there(name))
            .collect();
        let resources = view.files.iter().filter(|(_, entry)| there(entry));
        files.extend(resources.map(|(path, _)| path.clone()));
        files.extend(view.values.keys().cloned());
        if !view.values.is_empty() || !view.files.is_empty() {
            files.insert(PUBLIC.to_owned());
        }
        Ok(files.into_iter().collect())
    }

    /// The file at `path`, as the edits left it: binary XML and values as text.
    pub fn file(&mut self, path: &str) -> Result<Option<Vec<u8>>> {
        let resources = self.viewed()?;
        if values_file(path).is_some() {
            let text = match (&resources.public, path) {
                (Some(public), PUBLIC) => Some(public.clone()),
                _ => resources.values(path),
            };
            return Ok(text.map(String::into_bytes));
        }
        let entry = resources.view().files.get(path).cloned();
        let entry = entry.unwrap_or_else(|| path.to_owned());
        if let Some(text) = resources.texts.get(&entry) {
            return Ok(Some(text.clone().into_bytes()));
        }
        let Some(data) = self.read(&entry) else {
            return Ok(None);
        };
        Ok(Some(match xml::read(&data) {
            Ok(doc) => xml::text::decode(&doc, &self.viewed()?.view().names).into_bytes(),
            Err(_) => data,
        }))
    }

    /// Put `data` at `path`, in place of a file there. A file as [`Apk::file`] gives it changes
    /// nothing. A values file says everything its configuration has of the types in it; text
    /// in place of binary XML is compiled as that was, when the APK is written; a new file
    /// under `res/` adds its resource, compiled from its text where aapt2 would compile it,
    /// and `res/values/public.xml` gives the resources added after it their IDs.
    pub fn put(&mut self, path: &str, data: Vec<u8>) -> Result<()> {
        if path != PUBLIC && self.file(path)?.as_deref() == Some(&data[..]) {
            return Ok(());
        }
        let text = |data: Vec<u8>| {
            String::from_utf8(data).map_err(|_| Error::xml(format!("{path} is not UTF-8")))
        };
        let resources = self.viewed()?;
        if let Some(values) = values_file(path) {
            let text = text(data)?;
            if path == PUBLIC {
                resources.public = Some(text);
                return Ok(());
            }
            let mut files = BTreeMap::from([(values.to_owned(), text)]);
            if let Some(public) = &resources.public {
                files.insert("values/public.xml".to_owned(), public.clone());
            }
            let min_sdk = resources.min_sdk;
            return values::apply(resources.table_mut()?, &files, min_sdk);
        }
        let entry = resources.view().files.get(path).cloned();
        let (entry, compiled) = match entry {
            Some(entry) => (entry, None),
            None if self.has(path) || !path.starts_with("res/") => (path.to_owned(), None),
            None => {
                let (entry, compiled, replaced) = self.resources()?.add_file(path)?;
                if let Some(replaced) = replaced {
                    self.resources()?.texts.remove(&replaced);
                    self.entries.insert(replaced, None);
                }
                (entry, Some(compiled))
            }
        };
        let texts = &self.resources()?.texts;
        let compiled = match compiled {
            Some(compiled) => compiled,
            None if texts.contains_key(&entry) => true,
            None => self.read(&entry).is_some_and(|d| xml::read(&d).is_ok()),
        };
        match compiled {
            true => {
                self.resources()?.texts.insert(entry, text(data)?);
            }
            false => {
                self.entries.insert(entry, Some(data));
            }
        }
        Ok(())
    }

    /// Remove the file at `path`. A values file that is gone has no values, and a resource
    /// file that is gone has no resource there.
    pub fn remove(&mut self, path: &str) -> Result<()> {
        let resources = self.viewed()?;
        if let Some(values) = values_file(path) {
            if resources.view().values.contains_key(path) {
                let files = BTreeMap::from([(values.to_owned(), "<resources />".to_owned())]);
                let min_sdk = resources.min_sdk;
                values::apply(resources.table_mut()?, &files, min_sdk)?;
            }
            return Ok(());
        }
        let entry = match resources.view().files.get(path).cloned() {
            Some(entry) => {
                resources.remove_file(path)?;
                entry
            }
            None => path.to_owned(),
        };
        resources.texts.remove(&entry);
        self.entries.insert(entry, None);
        Ok(())
    }

    /// Set the resource the values element `<kind name="name">` defines, with `body` in it, in
    /// the configuration the qualifiers `qualifiers` name: [`values::set`] on the APK's table.
    /// Whether the resource had a value there.
    pub fn set_value(
        &mut self,
        kind: &str,
        name: &str,
        qualifiers: &str,
        body: values::Body<'_>,
    ) -> Result<bool> {
        let resources = self.resources()?;
        let min_sdk = resources.min_sdk;
        values::set(
            resources.table_mut()?,
            kind,
            name,
            qualifiers,
            body,
            min_sdk,
        )
    }

    /// Put into the APK's entries what the edits of its resources come to: binary XML compiled
    /// from the text they gave, and the table if they changed it.
    pub(crate) fn write_resources(&mut self) -> Result<()> {
        let Some(resources) = &mut self.resources else {
            return Ok(());
        };
        for (entry, text) in std::mem::take(&mut resources.texts) {
            let original = match self.read(&entry) {
                Some(bytes) => xml::read(&bytes)?,
                None => xml::Document {
                    strings: empty_pool(),
                    ids: vec![],
                    nodes: vec![],
                },
            };
            let names = &self.viewed()?.view().names;
            let doc = xml::text::apply(&original, &text, names).map_err(|e| e.at(&entry))?;
            self.entries.insert(entry, Some(xml::write(&doc)));
        }
        let resources = self.resources.as_mut().expect("there above");
        if resources.changed
            && let Some(table) = &resources.table
        {
            self.entries
                .insert(TABLE.to_owned(), Some(table::write(table)));
            resources.changed = false;
        }
        Ok(())
    }
}

/// The APK's resources as files by path: `AndroidManifest.xml` and everything under `res/`.
pub fn decode<R: Read + Seek>(apk: &mut Apk<R>) -> Result<BTreeMap<String, Vec<u8>>> {
    let mut files = BTreeMap::new();
    for path in apk.files()? {
        if (path == MANIFEST || path.starts_with("res/"))
            && let Some(data) = apk.file(&path)?
        {
            files.insert(path, data);
        }
    }
    Ok(files)
}

/// How the APK's entries change for `files`, laid out as [`decode`] lays them out, to be its
/// resources: each is [`Apk::put`], and with `complete`, `files` are all of them, so a file
/// under `res/` they don't have is removed.
pub fn edits<R: Read + Seek>(
    apk: &mut Apk<R>,
    files: &BTreeMap<String, Vec<u8>>,
    complete: bool,
) -> Result<rebuild::Edits> {
    let had = match complete {
        true => apk.files()?,
        false => vec![],
    };
    // the IDs first, then the files, which values may name
    let order = |path: &str| (path != PUBLIC, values_file(path).is_some());
    let mut paths: Vec<&String> = files.keys().collect();
    paths.sort_by_key(|path| order(path));
    for path in paths {
        apk.put(path, files[path].clone())?;
    }
    for path in had {
        if path.starts_with("res/") && !files.contains_key(&path) {
            apk.remove(&path)?;
        }
    }
    Ok(apk.edits()?.clone())
}

#[cfg(test)]
#[path = "../../tests/apkscope-core/resources.rs"]
mod tests;

//! The error every fallible function of `apkscope-core` returns. Its text is the message the
//! `apkscope` command prints.

use std::{fmt, io, path::PathBuf};

#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum Error {
    /// Reading or writing a file failed.
    #[error("{}", path.display())]
    Io {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
    /// Writing a rebuilt APK to where it goes failed.
    #[error("cannot write the APK")]
    Write(#[source] io::Error),
    /// A ZIP archive, or an entry in it, could not be read.
    #[error(transparent)]
    Zip(#[from] zip::result::ZipError),
    /// What went wrong in a file or a ZIP entry, `what`: the path or the entry name.
    #[error("{what}")]
    In {
        what: String,
        #[source]
        source: Box<Error>,
    },
    /// The APK defines no class by this name.
    #[error("class {0} not found")]
    ClassNotFound(String),
    /// The APK has no entry by this name.
    #[error("{0} is not in the APK")]
    EntryNotFound(String),
    /// Smali that does not parse.
    #[error("line {line}: {message}")]
    Smali { line: usize, message: String },
    /// A signing key that is not an unencrypted PKCS#8 RSA key.
    #[error("{}: not an unencrypted PKCS#8 RSA key", path.display())]
    Key {
        path: PathBuf,
        #[source]
        source: rsa::pkcs8::Error,
    },
    /// A keystore that cannot be read: wrong password, unsupported format or cipher.
    #[error("keystore: {0}")]
    Keystore(String),
    /// A certificate that is not X.509.
    #[error("{}: not an X.509 certificate", path.display())]
    Certificate { path: PathBuf },
    /// A malformed DEX file.
    #[error("invalid DEX: {0}")]
    Dex(String),
    /// Classes the DEX format cannot hold as written: a branch too far, a register too high.
    #[error("{0}")]
    Assemble(String),
    /// A malformed `resources.arsc` or binary XML file, or a string resource that isn't there.
    #[error("invalid resources: {0}")]
    Resources(String),
    /// XML text that cannot be encoded as binary XML.
    #[error("{0}")]
    Xml(String),
    /// A malformed or failing APK signature.
    #[error("{0}")]
    Signature(String),
    /// Generating an RSA key failed.
    #[error(transparent)]
    Rsa(#[from] rsa::Error),
    /// An entry compressed with a method other than stored or deflated, which a rebuild cannot
    /// copy.
    #[error("{entry}: unsupported compression {method}")]
    UnsupportedCompression { entry: String, method: String },
    /// A rebuild that would need ZIP64, which Android cannot read.
    #[error("more than 65535 files: Android can't read ZIP64 APKs")]
    TooManyEntries,
    /// An entry, or the APK itself, too large for a ZIP without ZIP64.
    #[error("{0} is larger than 4 GB")]
    TooLarge(String),
}

pub type Result<T, E = Error> = std::result::Result<T, E>;

impl Error {
    pub(crate) fn io(path: impl Into<PathBuf>) -> impl FnOnce(io::Error) -> Error {
        let path = path.into();
        move |source| Error::Io { path, source }
    }

    pub(crate) fn dex(m: impl Into<String>) -> Error {
        Error::Dex(m.into())
    }

    pub(crate) fn assemble(m: impl Into<String>) -> Error {
        Error::Assemble(m.into())
    }

    pub(crate) fn resources(m: impl Into<String>) -> Error {
        Error::Resources(m.into())
    }

    pub(crate) fn xml(m: impl Into<String>) -> Error {
        Error::Xml(m.into())
    }

    pub(crate) fn signature(m: impl Into<String>) -> Error {
        Error::Signature(m.into())
    }

    /// The same error, as one in `what`: a file's path or an entry's name. An I/O error
    /// names its file already.
    pub(crate) fn at(self, what: impl fmt::Display) -> Error {
        match self {
            Error::Io { .. } => self,
            source => Error::In {
                what: what.to_string(),
                source: Box::new(source),
            },
        }
    }

    /// The message and those of the errors that caused it: `app.apk: classes2.dex: bad type id 7`.
    pub fn report(&self) -> String {
        let mut out = self.to_string();
        let mut cause = std::error::Error::source(self);
        while let Some(e) = cause {
            let text = e.to_string();
            // some sources' messages already include their own cause
            if !out.ends_with(&text) {
                out = format!("{out}: {text}");
            }
            cause = e.source();
        }
        out
    }
}

/// For callers that report errors as text, as the `apkscope` command does: [`Error::report`].
impl From<Error> for String {
    fn from(e: Error) -> Self {
        e.report()
    }
}

#[cfg(test)]
#[path = "../../tests/apkscope-core/error.rs"]
mod tests;

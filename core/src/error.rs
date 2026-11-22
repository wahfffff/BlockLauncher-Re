//! Error types shared by the whole core.
//!
//! Every failure that can reach a user carries enough context to say *which*
//! file or field was wrong. A bare "invalid APK" tells the person nothing, and
//! the point of this crate is that the UI never has to guess.

use std::fmt;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Error {
    /// The file is not a ZIP at all, or its directory is unreadable.
    NotZip(String),
    /// The ZIP is fine but the entry we need is missing.
    MissingEntry(String),
    /// The manifest is present but not a well-formed binary XML document.
    BadManifest(String),
    /// The manifest parsed, but a field the launcher needs is absent or absurd.
    UnusableManifest(String),
    /// A store operation that expected something to exist did not find it.
    NotFound(String),
    /// A store operation that expected something to be absent found it.
    AlreadyExists(String),
    /// A native mod package did not describe itself correctly.
    BadMod(String),
    /// A texture pack is not something the game could use.
    BadPack(String),
    /// A ModPE script failed before it could define anything.
    BadScript(String),
    /// A shared object is not an ELF file this reader can trust.
    BadElf(String),
    /// Reading or writing a file on the device failed.
    Io(String),
    /// The persisted index could not be read back.
    Corrupt(String),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::NotZip(s) => write!(f, "not a zip archive: {s}"),
            Error::MissingEntry(s) => write!(f, "archive has no {s}"),
            Error::BadManifest(s) => write!(f, "malformed AndroidManifest.xml: {s}"),
            Error::UnusableManifest(s) => write!(f, "AndroidManifest.xml is unusable: {s}"),
            Error::NotFound(s) => write!(f, "not found: {s}"),
            Error::AlreadyExists(s) => write!(f, "already exists: {s}"),
            Error::BadMod(s) => write!(f, "bad native mod: {s}"),
            Error::BadPack(s) => write!(f, "bad texture pack: {s}"),
            Error::BadScript(s) => write!(f, "script error: {s}"),
            Error::BadElf(s) => write!(f, "unreadable shared object: {s}"),
            Error::Io(s) => write!(f, "io error: {s}"),
            Error::Corrupt(s) => write!(f, "corrupt index: {s}"),
        }
    }
}

impl std::error::Error for Error {}

impl From<std::io::Error> for Error {
    fn from(e: std::io::Error) -> Self {
        Error::Io(e.to_string())
    }
}

pub type Result<T> = std::result::Result<T, Error>;

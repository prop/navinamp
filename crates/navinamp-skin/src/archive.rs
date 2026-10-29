//! `.wal` archive loading: a ZIP with a case-insensitive name index.
//!
//! Real skins are inconsistent about entry-name casing (`Skin.XML`,
//! `bitmapS/logo.png`, ...), so every lookup is normalized to lowercase while
//! the original names are preserved for [`SkinArchive::file_names`].

use std::collections::BTreeMap;
use std::io::{Cursor, Read};
use std::path::Path;

use crate::error::SkinError;

/// Canonical name of the skin manifest inside a `.wal` archive.
pub const SKIN_XML: &str = "skin.xml";

/// A loaded `.wal` skin archive.
///
/// The raw ZIP bytes are kept in memory (skins are a few MB at most) and a
/// ZIP reader is opened per [`read_file`](Self::read_file) call, which keeps
/// the type `Send + Sync` and the API borrow-free.
#[derive(Debug)]
pub struct SkinArchive {
    raw: Vec<u8>,
    /// lowercase entry name -> original entry name
    index: BTreeMap<String, String>,
}

impl SkinArchive {
    /// Opens a `.wal` file from disk.
    pub fn open(path: &Path) -> Result<Self, SkinError> {
        let raw = std::fs::read(path)?;
        Self::from_bytes(raw)
    }

    /// Builds an archive from raw `.wal` bytes (useful for IPC and tests).
    pub fn from_bytes(raw: Vec<u8>) -> Result<Self, SkinError> {
        let mut zip = zip::ZipArchive::new(Cursor::new(raw.as_slice()))?;
        let mut index = BTreeMap::new();
        for i in 0..zip.len() {
            let entry = zip.by_index(i)?;
            let name = entry.name()?.into_owned();
            // Do not let directory entries shadow files; last file wins.
            index.insert(name.to_lowercase(), name);
        }
        Ok(Self { raw, index })
    }

    /// All entry names, in their original casing, sorted.
    pub fn file_names(&self) -> Vec<String> {
        self.index.values().cloned().collect()
    }

    /// Case-insensitive existence check.
    pub fn contains(&self, name: &str) -> bool {
        self.index.contains_key(&name.to_lowercase())
    }

    /// Reads an entry by name, case-insensitively.
    pub fn read_file(&self, name: &str) -> Result<Vec<u8>, SkinError> {
        let actual = self
            .index
            .get(&name.to_lowercase())
            .ok_or_else(|| SkinError::FileNotFound(name.to_owned()))?;
        let mut zip = zip::ZipArchive::new(Cursor::new(self.raw.as_slice()))?;
        let mut entry = zip.by_name(actual)?;
        let mut out = Vec::new();
        entry.read_to_end(&mut out)?;
        Ok(out)
    }
}

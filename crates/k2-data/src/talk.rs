//! dialog.tlk string lookup.

use std::path::Path;

use rakata_formats::tlk::Tlk;

use crate::gff::LocString;
use crate::{DataError, Result, parse_err};

/// The install's talk table.
pub struct Talk {
    tlk: Tlk,
}

impl Talk {
    /// Load `dialog.tlk` from the install root.
    pub fn load(install: &k2_res::Install) -> Result<Self> {
        let path = std::fs::read_dir(&install.root)
            .map_err(k2_res::ResError::from)?
            .flatten()
            .map(|e| e.path())
            .find(|p| p.file_name().and_then(|n| n.to_str()).is_some_and(|n| n.eq_ignore_ascii_case("dialog.tlk")))
            .ok_or_else(|| DataError::Missing("dialog.tlk".into()))?;
        Self::from_path(&path)
    }

    pub fn from_path(path: &Path) -> Result<Self> {
        let bytes = std::fs::read(path).map_err(k2_res::ResError::from)?;
        let tlk = rakata_formats::read_tlk_from_bytes(&bytes).map_err(|e| parse_err(path.display(), e))?;
        Ok(Self { tlk })
    }

    pub fn len(&self) -> usize {
        self.tlk.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.tlk.entries.is_empty()
    }

    /// Text for a StrRef, None when out of range.
    pub fn get(&self, strref: u32) -> Option<&str> {
        self.tlk.entries.get(strref as usize).map(|e| e.text.as_str())
    }

    /// Voice-over resref for a StrRef, if any.
    pub fn sound(&self, strref: u32) -> Option<String> {
        let e = self.tlk.entries.get(strref as usize)?;
        let s = e.voiceover.to_string().to_ascii_lowercase();
        (!s.is_empty()).then_some(s)
    }

    /// Resolve a CExoLocString: the TLK entry when referenced, else the first
    /// embedded substring (English male/female first).
    pub fn resolve(&self, s: &LocString) -> String {
        if let Some(text) = s.strref.and_then(|r| self.get(r))
            && !text.is_empty() {
                return text.to_owned();
            }
        s.substrings
            .iter()
            .find(|(id, _)| *id <= 1)
            .or_else(|| s.substrings.first())
            .map(|(_, t)| t.clone())
            .unwrap_or_default()
    }
}

use std::collections::HashMap;

use fontdue::{Font, FontSettings};
use serde::{Deserialize, Serialize};
use tracing::info;

use crate::{
    RDBView,
    defaults::default_fonts,
    utils::NorenError,
};

use super::DatabaseEntry;

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct FontInfo {
    pub name: String,
    #[serde(default)]
    pub collection_index: u32,
}

impl FontInfo {
    pub fn new(name: String) -> Self {
        Self {
            name,
            collection_index: 0,
        }
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct HostFont {
    pub info: FontInfo,
    pub data: Vec<u8>,
}

impl HostFont {
    pub fn new(name: String, data: Vec<u8>) -> Self {
        Self {
            info: FontInfo::new(name),
            data,
        }
    }

    pub fn new_with_index(name: String, collection_index: u32, data: Vec<u8>) -> Self {
        Self {
            info: FontInfo {
                name,
                collection_index,
            },
            data,
        }
    }

    pub fn info(&self) -> &FontInfo {
        &self.info
    }

    pub fn data(&self) -> &[u8] {
        &self.data
    }
}

pub struct LoadedFont {
    pub name: String,
    pub font: Font,
}

#[derive(Default)]
pub struct FontDB {
    data: Option<RDBView>,
    defaults: HashMap<String, HostFont>,
}

impl FontDB {
    /// Loads fonts from the given `.rdb` file path, if it exists.
    pub fn new(module_path: &str) -> Self {
        let data = match RDBView::load(module_path) {
            Ok(d) => Some(d),
            Err(_) => None,
        };

        Self {
            data,
            defaults: default_fonts()
                .into_iter()
                .map(|font| (font.info.name.clone(), font))
                .collect(),
        }
    }

    fn build_font(&self, host: HostFont) -> Result<LoadedFont, NorenError> {
        let settings = FontSettings {
            collection_index: host.info.collection_index,
            ..Default::default()
        };
        let font = Font::from_bytes(host.data, settings).map_err(|_| NorenError::DataFailure())?;
        Ok(LoadedFont {
            name: host.info.name,
            font,
        })
    }

    /// Fetches a font by entry name.
    pub fn fetch_font(&mut self, entry: DatabaseEntry<'_>) -> Result<LoadedFont, NorenError> {
        if let Some(rdb) = &mut self.data {
            if let Ok(font) = rdb.fetch::<HostFont>(entry) {
                info!(resource = "font", entry = %entry, source = "rdb");
                return self.build_font(font);
            }
        }

        if let Some(font) = self.defaults.get(entry) {
            info!(resource = "font", entry = %entry, source = "default");
            return self.build_font(font.clone());
        }

        Err(NorenError::DataFailure())
    }

    /// Lists all font entries available in the backing database.
    pub fn enumerate_entries(&self) -> Vec<String> {
        let mut entries: Vec<String> = self
            .data
            .as_ref()
            .map(|rdb| rdb.entries().into_iter().map(|meta| meta.name).collect())
            .unwrap_or_default();
        for entry in self.defaults.keys() {
            if !entries.iter().any(|existing| existing == entry) {
                entries.push(entry.clone());
            }
        }
        entries
    }
}

#[cfg(test)]
mod tests {
    use super::{FontDB, HostFont};
    use crate::{defaults::DEFAULT_FONT_ENTRY, utils::rdbfile::RDBFile};

    const ENTRY: &str = "fonts/test";

    #[test]
    fn fetch_font() {
        let data = include_bytes!("../../sample/sample_pre/fonts/DejaVuSans.ttf").to_vec();
        let font = HostFont::new(ENTRY.to_string(), data);
        let mut file = RDBFile::new();
        file.add(ENTRY, &font).expect("add font");

        let tmp = std::env::temp_dir().join("font_module.rdb");
        file.save(&tmp).expect("save font file");

        let mut db = FontDB::new(tmp.to_str().unwrap());
        let loaded = db.fetch_font(ENTRY).expect("load font");

        assert_eq!(loaded.name, ENTRY);
    }

    #[test]
    fn default_font_available_without_file() {
        let mut db = FontDB::new("missing_fonts.rdb");
        let loaded = db
            .fetch_font(DEFAULT_FONT_ENTRY)
            .expect("load default font");
        assert_eq!(loaded.name, DEFAULT_FONT_ENTRY);
    }
}

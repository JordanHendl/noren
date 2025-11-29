use serde::{Deserialize, Serialize};

use super::DatabaseEntry;
use crate::{RDBView, utils::NorenError};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AudioFormat {
    Ogg,
    Wav,
    Mp3,
    Flac,
    Unknown,
}

impl Default for AudioFormat {
    fn default() -> Self {
        Self::Unknown
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct AudioClip {
    pub name: String,
    #[serde(default)]
    pub format: AudioFormat,
    #[serde(default)]
    pub data: Vec<u8>,
}

impl AudioClip {
    pub fn new(name: String, format: AudioFormat, data: Vec<u8>) -> Self {
        Self { name, format, data }
    }
}

#[derive(Default)]
pub struct AudioDB {
    data: Option<RDBView>,
}

impl AudioDB {
    /// Loads audio clips from the given `.rdb` file path, if it exists.
    pub fn new(module_path: &str) -> Self {
        let data = match RDBView::load(module_path) {
            Ok(d) => Some(d),
            Err(_) => None,
        };

        Self { data }
    }

    /// Fetches an audio clip by entry name.
    pub fn fetch_clip(&mut self, entry: DatabaseEntry<'_>) -> Result<AudioClip, NorenError> {
        if let Some(rdb) = &mut self.data {
            return Ok(rdb.fetch::<AudioClip>(entry)?);
        }

        Err(NorenError::DataFailure())
    }

    /// Lists all audio clip entries available in the backing database.
    pub fn enumerate_entries(&self) -> Vec<String> {
        self.data
            .as_ref()
            .map(|rdb| rdb.entries().into_iter().map(|meta| meta.name).collect())
            .unwrap_or_default()
    }
}

#[cfg(test)]
mod tests {
    use super::{AudioClip, AudioDB, AudioFormat};
    use crate::utils::rdbfile::RDBFile;

    const ENTRY: &str = "audio/test";

    #[test]
    fn fetch_audio_clip() {
        let clip = AudioClip::new(ENTRY.to_string(), AudioFormat::Wav, vec![0, 1, 2, 3]);
        let mut file = RDBFile::new();
        file.add(ENTRY, &clip).expect("add clip");

        let tmp = std::env::temp_dir().join("audio_module.rdb");
        file.save(&tmp).expect("save audio file");

        let mut db = AudioDB::new(tmp.to_str().unwrap());
        let loaded = db.fetch_clip(ENTRY).expect("load audio clip");

        assert_eq!(loaded, clip);
    }
}

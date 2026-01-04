use serde::{Deserialize, Serialize};
use std::collections::HashMap;

use super::DatabaseEntry;
use crate::{
    RDBView,
    defaults::default_sounds,
    utils::NorenError,
};

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

#[derive(Debug, Clone, Default)]
pub struct SoundTrack {
    pub name: String,
    pub format: AudioFormat,
    data: Vec<u8>,
    cursor: usize,
}

impl SoundTrack {
    pub fn new(clip: AudioClip) -> Self {
        Self {
            name: clip.name,
            format: clip.format,
            data: clip.data,
            cursor: 0,
        }
    }

    pub fn is_finished(&self) -> bool {
        self.cursor >= self.data.len()
    }

    pub fn reset(&mut self) {
        self.cursor = 0;
    }

    pub fn read_chunk(&mut self, size: usize) -> Option<Vec<u8>> {
        if self.is_finished() {
            return None;
        }

        let end = (self.cursor + size).min(self.data.len());
        let chunk = self.data[self.cursor..end].to_vec();
        self.cursor = end;
        Some(chunk)
    }
}

#[derive(Default)]
pub struct AudioDB {
    data: Option<RDBView>,
    defaults: HashMap<String, AudioClip>,
}

impl AudioDB {
    /// Loads audio clips from the given `.rdb` file path, if it exists.
    pub fn new(module_path: &str) -> Self {
        let data = match RDBView::load(module_path) {
            Ok(d) => Some(d),
            Err(_) => None,
        };

        Self {
            data,
            defaults: default_sounds()
                .into_iter()
                .map(|clip| (clip.name.clone(), clip))
                .collect(),
        }
    }

    /// Fetches an audio clip by entry name.
    pub fn fetch_clip(&mut self, entry: DatabaseEntry<'_>) -> Result<AudioClip, NorenError> {
        if let Some(rdb) = &mut self.data {
            if let Ok(clip) = rdb.fetch::<AudioClip>(entry) {
                return Ok(clip);
            }
        }

        self.defaults
            .get(entry)
            .cloned()
            .ok_or(NorenError::DataFailure())
    }

    /// Fetches a fully loaded sound clip by entry name.
    pub fn fetch_sound_clip(&mut self, entry: DatabaseEntry<'_>) -> Result<AudioClip, NorenError> {
        self.fetch_clip(entry)
    }

    /// Fetches a sound track by entry name for streaming use.
    pub fn fetch_sound_track(
        &mut self,
        entry: DatabaseEntry<'_>,
    ) -> Result<SoundTrack, NorenError> {
        let clip = self.fetch_clip(entry)?;
        Ok(SoundTrack::new(clip))
    }

    /// Lists all audio clip entries available in the backing database.
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

    #[test]
    fn default_sound_available_without_file() {
        let mut db = AudioDB::new("missing_audio.rdb");
        for entry in crate::defaults::DEFAULT_SOUND_ENTRIES {
            let clip = db
                .fetch_sound_clip(entry)
                .unwrap_or_else(|_| panic!("load default sound {entry}"));
            assert_eq!(clip.name, entry);
        }
    }
}

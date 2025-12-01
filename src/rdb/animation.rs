use serde::{Deserialize, Serialize};

use super::DatabaseEntry;
use crate::{RDBView, utils::NorenError};

#[repr(C)]
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct AnimationClip {
    pub name: String,
    #[serde(default)]
    pub duration_seconds: f32,
    #[serde(default)]
    pub data: Vec<u8>,
}

impl AnimationClip {
    pub fn new(name: impl Into<String>, duration_seconds: f32, data: Vec<u8>) -> Self {
        Self {
            name: name.into(),
            duration_seconds,
            data,
        }
    }
}

#[derive(Default)]
pub struct AnimationDB {
    data: Option<RDBView>,
}

impl AnimationDB {
    /// Loads animation clips from the given `.rdb` file path, if present.
    pub fn new(module_path: &str) -> Self {
        let data = match RDBView::load(module_path) {
            Ok(d) => Some(d),
            Err(_) => None,
        };

        Self { data }
    }

    /// Fetches an animation clip by entry name.
    pub fn fetch_animation(
        &mut self,
        entry: DatabaseEntry<'_>,
    ) -> Result<AnimationClip, NorenError> {
        if let Some(rdb) = &mut self.data {
            return Ok(rdb.fetch::<AnimationClip>(entry)?);
        }

        Err(NorenError::DataFailure())
    }

    /// Lists animation entries available in the backing database.
    pub fn enumerate_entries(&self) -> Vec<String> {
        self.data
            .as_ref()
            .map(|rdb| rdb.entries().into_iter().map(|meta| meta.name).collect())
            .unwrap_or_default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::utils::rdbfile::RDBFile;

    const ENTRY: &str = "animations/test";

    #[test]
    fn fetch_animation_clip() {
        let clip = AnimationClip::new("wave", 1.5, vec![5, 6, 7, 8]);
        let mut file = RDBFile::new();
        file.add(ENTRY, &clip).expect("add animation");

        let tmp = std::env::temp_dir().join("animations.rdb");
        file.save(&tmp).expect("save animation file");

        let mut db = AnimationDB::new(tmp.to_str().unwrap());
        let loaded = db.fetch_animation(ENTRY).expect("load animation");

        assert_eq!(loaded, clip);
    }
}

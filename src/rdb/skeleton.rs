use serde::{Deserialize, Serialize};

use super::DatabaseEntry;
use crate::{RDBView, utils::NorenError};

#[repr(C)]
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct Skeleton {
    pub name: String,
    #[serde(default)]
    pub joints: Vec<Joint>,
    #[serde(default)]
    pub root: Option<usize>,
    /// Reserved for future binary payloads.
    #[serde(default)]
    pub data: Vec<u8>,
}

impl Skeleton {
    pub fn new(name: impl Into<String>, joints: Vec<Joint>, root: Option<usize>) -> Self {
        Self {
            name: name.into(),
            joints,
            root,
            data: Vec::new(),
        }
    }
}

#[repr(C)]
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Joint {
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub parent: Option<usize>,
    #[serde(default)]
    pub children: Vec<usize>,
    #[serde(default)]
    pub inverse_bind_matrix: [[f32; 4]; 4],
    #[serde(default)]
    pub translation: [f32; 3],
    #[serde(default)]
    pub rotation: [f32; 4],
    #[serde(default)]
    pub scale: [f32; 3],
}

impl Default for Joint {
    fn default() -> Self {
        Self {
            name: None,
            parent: None,
            children: Vec::new(),
            inverse_bind_matrix: [
                [1.0, 0.0, 0.0, 0.0],
                [0.0, 1.0, 0.0, 0.0],
                [0.0, 0.0, 1.0, 0.0],
                [0.0, 0.0, 0.0, 1.0],
            ],
            translation: [0.0, 0.0, 0.0],
            rotation: [0.0, 0.0, 0.0, 1.0],
            scale: [1.0, 1.0, 1.0],
        }
    }
}

#[derive(Default)]
pub struct SkeletonDB {
    data: Option<RDBView>,
}

impl SkeletonDB {
    /// Loads skeleton assets from the given `.rdb` file path, if present.
    pub fn new(module_path: &str) -> Self {
        let data = match RDBView::load(module_path) {
            Ok(d) => Some(d),
            Err(_) => None,
        };

        Self { data }
    }

    /// Fetches a skeleton asset by entry name.
    pub fn fetch_skeleton(&mut self, entry: DatabaseEntry<'_>) -> Result<Skeleton, NorenError> {
        if let Some(rdb) = &mut self.data {
            return Ok(rdb.fetch::<Skeleton>(entry)?);
        }

        Err(NorenError::DataFailure())
    }

    /// Lists skeleton entries available in the backing database.
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

    const ENTRY: &str = "skeletons/test";

    #[test]
    fn fetch_skeleton_asset() {
        let skeleton = Skeleton::new("test", vec![Joint::default()], Some(0));
        let mut file = RDBFile::new();
        file.add(ENTRY, &skeleton).expect("add skeleton");

        let tmp = std::env::temp_dir().join("skeletons.rdb");
        file.save(&tmp).expect("save skeleton file");

        let mut db = SkeletonDB::new(tmp.to_str().unwrap());
        let loaded = db.fetch_skeleton(ENTRY).expect("load skeleton");

        assert_eq!(loaded, skeleton);
    }
}

use serde::{Deserialize, Serialize};

use super::DatabaseEntry;
use crate::{RDBView, utils::NorenError};

#[repr(C)]
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct Skeleton {
    pub name: String,
    #[serde(default)]
    pub data: Vec<u8>,
}

impl Skeleton {
    pub fn new(name: impl Into<String>, data: Vec<u8>) -> Self {
        Self {
            name: name.into(),
            data,
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
        let skeleton = Skeleton::new("test", vec![1, 2, 3, 4]);
        let mut file = RDBFile::new();
        file.add(ENTRY, &skeleton).expect("add skeleton");

        let tmp = std::env::temp_dir().join("skeletons.rdb");
        file.save(&tmp).expect("save skeleton file");

        let mut db = SkeletonDB::new(tmp.to_str().unwrap());
        let loaded = db.fetch_skeleton(ENTRY).expect("load skeleton");

        assert_eq!(loaded, skeleton);
    }
}

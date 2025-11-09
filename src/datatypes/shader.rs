use serde::{Deserialize, Serialize};

use super::DatabaseEntry;
use crate::{RDBView, utils::NorenError};

const SPIRV_MAGIC_WORD: u32 = 0x0723_0203;

#[repr(C)]
#[derive(Clone, Debug, Serialize, Deserialize, Default)]
pub struct ShaderModule {
    words: Vec<u32>,
}

impl ShaderModule {
    pub fn from_words(words: Vec<u32>) -> Self {
        Self { words }
    }

    pub fn words(&self) -> &[u32] {
        &self.words
    }

    pub fn is_spirv(&self) -> bool {
        matches!(self.words.first(), Some(&word) if word == SPIRV_MAGIC_WORD)
    }
}

#[derive(Default)]
pub struct ShaderDB {
    data: Option<RDBView>,
}

impl ShaderDB {
    pub fn new(module_path: &str) -> Self {
        let data = match RDBView::load(module_path) {
            Ok(d) => Some(d),
            Err(_) => None,
        };

        Self { data }
    }

    pub fn fetch_module(&mut self, entry: DatabaseEntry) -> Result<ShaderModule, NorenError> {
        if let Some(rdb) = &mut self.data {
            let module = rdb.fetch::<ShaderModule>(entry)?;
            if module.is_spirv() {
                return Ok(module);
            }

            return Err(NorenError::DataFailure());
        }

        Err(NorenError::DataFailure())
    }
}

#[cfg(test)]
mod tests {
    use super::{SPIRV_MAGIC_WORD, ShaderDB, ShaderModule};
    use crate::utils::rdbfile::RDBFile;

    const ENTRY: &str = "shader/test";

    #[test]
    fn fetch_valid_spirv_module() {
        let mut file = RDBFile::new();
        let module = ShaderModule::from_words(vec![SPIRV_MAGIC_WORD, 1, 2, 3]);
        file.add(ENTRY, &module).expect("add module");

        let tmp = std::env::temp_dir().join("shader_module.rdb");
        file.save(&tmp).expect("save module file");

        let mut db = ShaderDB::new(tmp.to_str().unwrap());
        let loaded = db.fetch_module(ENTRY).expect("load shader module");

        assert_eq!(loaded.words(), module.words());
    }
}

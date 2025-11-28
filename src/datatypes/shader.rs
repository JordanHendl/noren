use serde::{Deserialize, Serialize};

use super::DatabaseEntry;
use crate::{RDBView, utils::NorenError};

const SPIRV_MAGIC_WORD: u32 = 0x0723_0203;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ShaderModule {
    artifact: bento::CompilationResult,
}

impl ShaderModule {
    pub fn from_compilation(artifact: bento::CompilationResult) -> Self {
        Self { artifact }
    }

    /// Creates a shader module from raw SPIR-V words.
    pub fn from_words(words: Vec<u32>) -> Self {
        Self {
            artifact: bento::CompilationResult {
                name: None,
                file: None,
                lang: bento::ShaderLang::Glsl,
                stage: dashi::ShaderType::Compute,
                variables: Vec::new(),
                spirv: words,
                metadata: todo!(),
            },
        }
    }

    pub fn artifact(&self) -> &bento::CompilationResult {
        &self.artifact
    }

    /// Returns the raw SPIR-V words backing the module.
    pub fn words(&self) -> &[u32] {
        &self.artifact.spirv
    }

    /// Checks whether the module data is valid SPIR-V.
    pub fn is_spirv(&self) -> bool {
        matches!(self.words().first(), Some(&word) if word == SPIRV_MAGIC_WORD)
    }
}

impl Default for ShaderModule {
    fn default() -> Self {
        Self::from_words(Vec::new())
    }
}

#[derive(Default)]
pub struct ShaderDB {
    data: Option<RDBView>,
}

impl ShaderDB {
    /// Loads shader modules from the given `.rdb` file path, if it exists.
    pub fn new(module_path: &str) -> Self {
        let data = match RDBView::load(module_path) {
            Ok(d) => Some(d),
            Err(_) => None,
        };

        Self { data }
    }

    /// Fetches a shader module by entry name, ensuring it contains SPIR-V data.
    pub fn fetch_module(&mut self, entry: DatabaseEntry<'_>) -> Result<ShaderModule, NorenError> {
        if let Some(rdb) = &mut self.data {
            let module = rdb.fetch::<ShaderModule>(entry)?;
            if module.is_spirv() {
                return Ok(module);
            }

            return Err(NorenError::DataFailure());
        }

        Err(NorenError::DataFailure())
    }

    /// Lists all shader modules available in the backing database.
    pub fn enumerate_entries(&self) -> Vec<String> {
        self.data
            .as_ref()
            .map(|rdb| rdb.entries().into_iter().map(|meta| meta.name).collect())
            .unwrap_or_default()
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

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

fn default_geometry_path() -> String {
    "geometry.rdb".to_string()
}

fn default_imagery_path() -> String {
    "imagery.rdb".to_string()
}

fn default_model_path() -> String {
    "models.json".to_string()
}

////////////////////////////////
/// This struct defines the structure of the database.
/// It is not needed, and if data is missing, it will default to values for data lookups.
///
/// Raw data (geometry, imagery, etc.) is found in '*.rdb' files inside the database. These are
/// mapped and data is looked up at runtime when fetched.
///
/// Complex data (models, materials) are loaded from json configuration, where they are described with what
/// primitives they use (mutliple meshes, ref geometry a/b/c with textures d/e/f, etc).
////////////////////////////////

#[derive(Debug, Serialize, Deserialize)]
pub struct DatabaseLayoutFile {
    #[serde(default = "default_geometry_path")]
    pub geometry: String,
    #[serde(default = "default_imagery_path")]
    pub imagery: String,
    #[serde(default = "default_model_path")]
    pub models: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ModelLayoutFile {
    #[serde(default)]
    pub textures: HashMap<String, TextureLayout>,
    #[serde(default)]
    pub materials: HashMap<String, MaterialLayout>,
    #[serde(default)]
    pub meshes: HashMap<String, MeshLayout>,
    #[serde(default)]
    pub models: HashMap<String, ModelLayout>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct TextureLayout {
    /// Database entry for the texture image.
    #[serde(default)]
    pub image: String,
    #[serde(default)]
    pub name: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct MaterialLayout {
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub textures: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct MeshLayout {
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub geometry: String,
    #[serde(default)]
    pub material: Option<String>,
    #[serde(default)]
    pub textures: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ModelLayout {
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub meshes: Vec<String>,
}

impl Default for DatabaseLayoutFile {
    fn default() -> Self {
        Self {
            geometry: default_geometry_path(),
            imagery: default_imagery_path(),
            models: default_model_path(),
        }
    }
}

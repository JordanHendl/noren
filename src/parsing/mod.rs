use std::collections::HashMap;

use crate::furikake_state::FurikakeState;
use dashi::{AttachmentDescription, SubpassDependency, Viewport};
use serde::{Deserialize, Serialize};
use serde_json::Value;

fn default_geometry_path() -> String {
    "geometry.rdb".to_string()
}

fn default_imagery_path() -> String {
    "imagery.rdb".to_string()
}

fn default_model_path() -> String {
    "models.json".to_string()
}

fn default_material_path() -> String {
    "materials.json".to_string()
}

fn default_render_pass_path() -> String {
    "render_passes.json".to_string()
}

fn default_shader_path() -> String {
    "shaders.rdb".to_string()
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
    #[serde(default = "default_material_path")]
    pub materials: String,
    #[serde(default = "default_render_pass_path")]
    pub render_passes: String,
    #[serde(default = "default_shader_path")]
    pub shaders: String,
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
    #[serde(default)]
    pub shaders: HashMap<String, GraphicsShaderLayout>,
    #[serde(default)]
    pub render_passes: HashMap<String, RenderPassLayout>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct RenderPassLayoutFile {
    #[serde(default)]
    pub render_passes: HashMap<String, RenderPassLayout>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct RenderPassLayout {
    #[serde(default)]
    pub debug_name: Option<String>,
    #[serde(default)]
    pub viewport: Viewport,
    #[serde(default)]
    pub subpasses: Vec<RenderSubpassLayout>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct RenderSubpassLayout {
    #[serde(default)]
    pub color_attachments: Vec<AttachmentDescription>,
    #[serde(default)]
    pub depth_stencil_attachment: Option<AttachmentDescription>,
    #[serde(default)]
    pub subpass_dependencies: Vec<SubpassDependency>,
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
    #[serde(default)]
    pub shader: Option<String>,
    #[serde(default)]
    pub metadata: MaterialMetadata,
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

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct MaterialMetadata {
    #[serde(default)]
    pub bindings: Vec<MaterialBinding>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct MaterialBinding {
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub defaults: Value,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct GraphicsShaderLayout {
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub vertex: Option<String>,
    #[serde(default)]
    pub fragment: Option<String>,
    #[serde(default)]
    pub geometry: Option<String>,
    #[serde(default, rename = "tessellation_control")]
    pub tessellation_control: Option<String>,
    #[serde(default, rename = "tessellation_evaluation")]
    pub tessellation_evaluation: Option<String>,
    #[serde(default)]
    pub subpass: u8,
    #[serde(default)]
    pub render_pass: Option<String>,
    #[serde(default)]
    pub furikake_state: FurikakeState,
}

impl Default for DatabaseLayoutFile {
    fn default() -> Self {
        Self {
            geometry: default_geometry_path(),
            imagery: default_imagery_path(),
            models: default_model_path(),
            materials: default_material_path(),
            render_passes: default_render_pass_path(),
            shaders: default_shader_path(),
        }
    }
}

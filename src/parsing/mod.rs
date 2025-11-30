use std::collections::HashMap;

use crate::furikake_state::FurikakeState;
use dashi::Format;
use serde::{Deserialize, Serialize};
use serde_json::Value;

fn default_geometry_path() -> String {
    "geometry.rdb".to_string()
}

fn default_imagery_path() -> String {
    "imagery.rdb".to_string()
}

fn default_audio_path() -> String {
    "audio.rdb".to_string()
}

fn default_texture_path() -> String {
    "textures.json".to_string()
}

fn default_model_path() -> String {
    "models.json".to_string()
}

fn default_material_path() -> String {
    "materials.json".to_string()
}

fn default_mesh_path() -> String {
    "meshes.json".to_string()
}

fn default_shader_layout_path() -> String {
    "shaders.json".to_string()
}

fn default_shader_module_path() -> String {
    "shaders.rdb".to_string()
}

#[derive(Debug, Serialize, Deserialize)]
pub struct DatabaseLayoutFile {
    #[serde(default = "default_geometry_path")]
    pub geometry: String,
    #[serde(default = "default_imagery_path")]
    pub imagery: String,
    #[serde(default = "default_audio_path")]
    pub audio: String,
    #[serde(default = "default_texture_path")]
    pub textures: String,
    #[serde(default = "default_material_path")]
    pub materials: String,
    #[serde(default = "default_mesh_path")]
    pub meshes: String,
    #[serde(default = "default_model_path")]
    pub models: String,
    #[serde(default = "default_shader_layout_path")]
    pub shader_layouts: String,
    #[serde(default = "default_shader_module_path")]
    pub shaders: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct TextureLayoutFile {
    #[serde(default)]
    pub textures: HashMap<String, TextureLayout>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct MaterialLayoutFile {
    #[serde(default)]
    pub materials: HashMap<String, MaterialLayout>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct MeshLayoutFile {
    #[serde(default)]
    pub meshes: HashMap<String, MeshLayout>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ModelLayoutFile {
    #[serde(default)]
    pub models: HashMap<String, ModelLayout>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ShaderLayoutFile {
    #[serde(default)]
    pub shaders: HashMap<String, GraphicsShaderLayout>,
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
    pub color_formats: Vec<Format>,
    #[serde(default)]
    pub depth_format: Option<Format>,
    #[serde(default)]
    pub furikake_state: FurikakeState,
}

#[derive(Debug, Clone, Default)]
pub struct MetaLayout {
    pub textures: HashMap<String, TextureLayout>,
    pub materials: HashMap<String, MaterialLayout>,
    pub meshes: HashMap<String, MeshLayout>,
    pub models: HashMap<String, ModelLayout>,
    pub shaders: HashMap<String, GraphicsShaderLayout>,
}

impl MetaLayout {
    pub fn is_empty(&self) -> bool {
        self.textures.is_empty()
            && self.materials.is_empty()
            && self.meshes.is_empty()
            && self.models.is_empty()
            && self.shaders.is_empty()
    }
}

impl Default for DatabaseLayoutFile {
    fn default() -> Self {
        Self {
            geometry: default_geometry_path(),
            imagery: default_imagery_path(),
            audio: default_audio_path(),
            textures: default_texture_path(),
            materials: default_material_path(),
            meshes: default_mesh_path(),
            models: default_model_path(),
            shader_layouts: default_shader_layout_path(),
            shaders: default_shader_module_path(),
        }
    }
}

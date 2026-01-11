use std::collections::HashMap;

use crate::furikake_state::FurikakeState;
use dashi::Format;
use serde::{Deserialize, Serialize};

fn default_geometry_path() -> String {
    "geometry.rdb".to_string()
}

fn default_imagery_path() -> String {
    "imagery.rdb".to_string()
}

fn default_audio_path() -> String {
    "audio.rdb".to_string()
}

fn default_font_path() -> String {
    "fonts.rdb".to_string()
}

fn default_texture_path() -> String {
    "textures.json".to_string()
}

fn default_atlas_path() -> String {
    "atlases.json".to_string()
}

fn default_skeleton_path() -> String {
    "skeletons.rdb".to_string()
}

fn default_animation_path() -> String {
    "animations.rdb".to_string()
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
    #[serde(default = "default_font_path")]
    pub fonts: String,
    #[serde(default = "default_skeleton_path")]
    pub skeletons: String,
    #[serde(default = "default_animation_path")]
    pub animations: String,
    #[serde(default = "default_texture_path")]
    pub textures: String,
    #[serde(default = "default_atlas_path")]
    pub atlases: String,
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
pub struct TextureAtlasLayoutFile {
    #[serde(default)]
    pub atlases: HashMap<String, TextureAtlasLayout>,
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
    #[serde(default)]
    pub compute_shaders: HashMap<String, ComputeShaderLayout>,
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
pub struct TextureAtlasLayout {
    /// Database entry for the atlas image.
    #[serde(default)]
    pub image: String,
    #[serde(default)]
    pub name: Option<String>,
    /// Default sprite size for sprites in the atlas.
    #[serde(default)]
    pub sprite_size: [u32; 2],
    /// Named sprite regions within the atlas image.
    #[serde(default)]
    pub sprites: HashMap<String, AtlasSprite>,
    /// Named animation clips keyed by animation name.
    #[serde(default)]
    pub animations: HashMap<String, AtlasAnimation>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct AtlasSprite {
    /// Pixel-space origin of the sprite region within the atlas.
    #[serde(default)]
    pub origin: [u32; 2],
    /// Pixel-space size override for the sprite region.
    #[serde(default)]
    pub size: Option<[u32; 2]>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct AtlasAnimation {
    /// Ordered frames for the animation.
    #[serde(default)]
    pub frames: Vec<AtlasFrame>,
    /// Whether the animation should loop when played.
    #[serde(default)]
    pub looped: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct AtlasFrame {
    /// Sprite name to display for this frame.
    #[serde(default)]
    pub sprite: String,
    /// Frame duration in milliseconds.
    #[serde(default)]
    pub duration_ms: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum MaterialType {
    #[default]
    Textured,
    EmissiveOnly,
    VertexColor,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct MaterialLayout {
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub render_mask: u16,
    #[serde(default)]
    pub material_type: MaterialType,
    #[serde(default)]
    pub texture_lookups: MaterialTextureLookups,
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
pub struct MaterialTextureLookups {
    #[serde(default)]
    pub base_color: Option<String>,
    #[serde(default)]
    pub normal: Option<String>,
    #[serde(default)]
    pub metallic_roughness: Option<String>,
    #[serde(default)]
    pub occlusion: Option<String>,
    #[serde(default)]
    pub emissive: Option<String>,
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

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ComputeShaderLayout {
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub entry: Option<String>,
    #[serde(default)]
    pub furikake_state: FurikakeState,
}

#[derive(Debug, Clone, Default)]
pub struct MetaLayout {
    pub textures: HashMap<String, TextureLayout>,
    pub atlases: HashMap<String, TextureAtlasLayout>,
    pub materials: HashMap<String, MaterialLayout>,
    pub meshes: HashMap<String, MeshLayout>,
    pub models: HashMap<String, ModelLayout>,
    pub shaders: HashMap<String, GraphicsShaderLayout>,
    pub compute_shaders: HashMap<String, ComputeShaderLayout>,
}

impl MetaLayout {
    pub fn is_empty(&self) -> bool {
        self.textures.is_empty()
            && self.atlases.is_empty()
            && self.materials.is_empty()
            && self.meshes.is_empty()
            && self.models.is_empty()
            && self.shaders.is_empty()
            && self.compute_shaders.is_empty()
    }
}

impl Default for DatabaseLayoutFile {
    fn default() -> Self {
        Self {
            geometry: default_geometry_path(),
            imagery: default_imagery_path(),
            audio: default_audio_path(),
            fonts: default_font_path(),
            skeletons: default_skeleton_path(),
            animations: default_animation_path(),
            textures: default_texture_path(),
            atlases: default_atlas_path(),
            materials: default_material_path(),
            meshes: default_mesh_path(),
            models: default_model_path(),
            shader_layouts: default_shader_layout_path(),
            shaders: default_shader_module_path(),
        }
    }
}

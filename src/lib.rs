mod furikake_state;
pub mod meta;
pub mod parsing;
pub mod rdb;
mod utils;
use std::{collections::HashMap, io::ErrorKind, ptr::NonNull};

use dashi::Context;
use furikake::{
    BindlessState,
    reservations::{
        bindless_materials::ReservedBindlessMaterials, bindless_textures::ReservedBindlessTextures,
    },
    types::Material as FurikakeMaterial,
};
pub use furikake_state::FurikakeState;
use meta::*;
use parsing::*;
use rdb::*;
use serde::de::DeserializeOwned;
use utils::*;

pub use parsing::DatabaseLayoutFile;
pub use utils::error::{NorenError, RdbErr};
pub use utils::rdbfile::{RDBEntryMeta, RDBFile, RDBView, type_tag_for};

pub struct DBInfo<'a> {
    pub base_dir: &'a str,
    pub layout_file: Option<&'a str>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ShaderValidationError {
    pub shader: String,
    pub issues: Vec<String>,
    pub materials: Vec<String>,
    pub models: Vec<String>,
}

struct FurikakeBindings {
    state: NonNull<BindlessState>,
    textures: HashMap<String, u16>,
    materials: HashMap<String, dashi::Handle<FurikakeMaterial>>,
}

impl FurikakeBindings {
    fn new(state: &mut BindlessState) -> Self {
        Self {
            state: NonNull::from(state),
            textures: HashMap::new(),
            materials: HashMap::new(),
        }
    }

    fn state_mut(&mut self) -> &mut BindlessState {
        unsafe { self.state.as_mut() }
    }
}

pub struct DB {
    geometry: GeometryDB,
    imagery: ImageDB,
    audio: AudioDB,
    skeletons: SkeletonDB,
    animations: AnimationDB,
    shaders: ShaderDB,
    ctx: Option<NonNull<dashi::Context>>,
    meta_layout: Option<MetaLayout>,
    graphics_pipeline_layouts: HashMap<String, dashi::Handle<dashi::GraphicsPipelineLayout>>,
    graphics_pipelines: HashMap<String, dashi::Handle<dashi::GraphicsPipeline>>,
    compute_pipeline_layouts: HashMap<String, dashi::Handle<dashi::ComputePipelineLayout>>,
    compute_pipelines: HashMap<String, dashi::Handle<dashi::ComputePipeline>>,
    furikake: Option<FurikakeBindings>,
}

fn read_database_layout(layout_file: Option<&str>) -> Result<DatabaseLayoutFile, NorenError> {
    let layout: DatabaseLayoutFile = match layout_file {
        Some(f) => serde_json::from_str(&std::fs::read_to_string(f.to_string())?)?,
        None => Default::default(),
    };
    Ok(layout)
}

fn load_json_file<T: DeserializeOwned + Default>(path: &str) -> Result<Option<T>, NorenError> {
    match std::fs::read_to_string(path) {
        Ok(raw) if raw.trim().is_empty() => Ok(None),
        Ok(raw) => Ok(Some(serde_json::from_str::<T>(&raw)?)),
        Err(err) if err.kind() == ErrorKind::NotFound => Ok(None),
        Err(err) => Err(err.into()),
    }
}

fn load_meta_layout(
    base_dir: &str,
    layout: &DatabaseLayoutFile,
) -> Result<Option<MetaLayout>, NorenError> {
    let textures =
        load_json_file::<TextureLayoutFile>(&format!("{}/{}", base_dir, layout.textures))?;
    let materials =
        load_json_file::<MaterialLayoutFile>(&format!("{}/{}", base_dir, layout.materials))?;
    let meshes = load_json_file::<MeshLayoutFile>(&format!("{}/{}", base_dir, layout.meshes))?;
    let models = load_json_file::<ModelLayoutFile>(&format!("{}/{}", base_dir, layout.models))?;
    let shader_layouts =
        load_json_file::<ShaderLayoutFile>(&format!("{}/{}", base_dir, layout.shader_layouts))?;
    let mut meta_layout = MetaLayout::default();
    if let Some(file) = textures {
        meta_layout.textures = file.textures;
    }
    if let Some(file) = materials {
        meta_layout.materials = file.materials;
    }
    if let Some(file) = meshes {
        meta_layout.meshes = file.meshes;
    }
    if let Some(file) = models {
        meta_layout.models = file.models;
    }
    if let Some(file) = shader_layouts {
        meta_layout.shaders = file.shaders;
        meta_layout.compute_shaders = file.compute_shaders;
    }

    if meta_layout.is_empty() {
        Ok(None)
    } else {
        Ok(Some(meta_layout))
    }
}

/// Validates that shader, material, and attachment-format references in the layout are consistent.
pub fn validate_database_layout(
    base_dir: &str,
    layout_file: Option<&str>,
) -> Result<(), NorenError> {
    let layout = read_database_layout(layout_file)?;
    let Some(meta_layout) = load_meta_layout(base_dir, &layout)? else {
        return Ok(());
    };

    let shader_modules = ShaderDB::new(&format!("{}/{}", base_dir, layout.shaders));
    let shader_db_ref = shader_modules.has_data().then_some(&shader_modules);

    validate_meta_layout(&meta_layout, shader_db_ref)
}

////////////////////////////////////////////////
/// NorenDB (Noran Database)
/// * Provides readonly access to reading and loading data
///   from a Noren Generated Database.
///
/// * Handles access to Host(CPU) and Device(GPU) data.
/// ** CPU Data is read from the mapped memory when requested.
/// ** GPU Data is GPU-Ready (OK for usage/uploads), cached and refcounted when requested, and will unload on a timer when all refs
///    are released. The timer is so that if data is quickly unreffed/fetched, it will handle
///    gracefully. This timer is configurable.
////////////////////////////////////////////////

impl DB {
    pub fn new(info: &DBInfo) -> Result<Self, NorenError> {
        Self::new_with_ctx(info, None)
    }

    /// Creates a database handle that can load assets using the provided configuration.
    pub fn new_with_ctx(info: &DBInfo, ctx: Option<*mut Context>) -> Result<Self, NorenError> {
        let layout = read_database_layout(info.layout_file)?;

        let geometry = GeometryDB::new(ctx, &format!("{}/{}", info.base_dir, layout.geometry));
        let imagery = ImageDB::new(ctx, &format!("{}/{}", info.base_dir, layout.imagery));
        let audio = AudioDB::new(&format!("{}/{}", info.base_dir, layout.audio));
        let skeletons = SkeletonDB::new(&format!("{}/{}", info.base_dir, layout.skeletons));
        let animations = AnimationDB::new(&format!("{}/{}", info.base_dir, layout.animations));
        let shaders = ShaderDB::new(&format!("{}/{}", info.base_dir, layout.shaders));
        let meta_layout = load_meta_layout(info.base_dir, &layout)?;

        if let Some(layout) = meta_layout.as_ref() {
            let shader_db_ref = shaders.has_data().then_some(&shaders);
            validate_meta_layout(layout, shader_db_ref)?;
        }

        Ok(Self {
            geometry,
            imagery,
            audio,
            skeletons,
            animations,
            shaders,
            ctx: None,
            meta_layout,
            graphics_pipeline_layouts: HashMap::new(),
            graphics_pipelines: HashMap::new(),
            compute_pipeline_layouts: HashMap::new(),
            compute_pipelines: HashMap::new(),
            furikake: None,
        })
    }

    /// Registers a dashi context for GPU-backed resource requests.
    pub fn import_dashi_context(&mut self, ctx: &mut dashi::Context) {
        let ctx = NonNull::from(ctx);
        self.ctx = Some(ctx);
        self.geometry.import_ctx(ctx);
        self.imagery.import_ctx(ctx);
    }

    fn ctx_mut(&mut self) -> Result<&mut dashi::Context, NorenError> {
        self.ctx
            .as_mut()
            .map(|ctx| unsafe { ctx.as_mut() })
            .ok_or(NorenError::DashiContext())
    }

    /// Returns an immutable reference to the geometry database.
    pub fn geometry(&self) -> &GeometryDB {
        &self.geometry
    }

    /// Returns a mutable reference to the geometry database.
    pub fn geometry_mut(&mut self) -> &mut GeometryDB {
        &mut self.geometry
    }

    /// Returns an immutable reference to the imagery database.
    pub fn imagery(&self) -> &ImageDB {
        &self.imagery
    }

    /// Returns a mutable reference to the imagery database.
    pub fn imagery_mut(&mut self) -> &mut ImageDB {
        &mut self.imagery
    }

    /// Returns an immutable reference to the audio database.
    pub fn audio(&self) -> &AudioDB {
        &self.audio
    }

    /// Returns a mutable reference to the audio database.
    pub fn audio_mut(&mut self) -> &mut AudioDB {
        &mut self.audio
    }

    /// Returns an immutable reference to the skeleton database.
    pub fn skeletons(&self) -> &SkeletonDB {
        &self.skeletons
    }

    /// Returns a mutable reference to the skeleton database.
    pub fn skeletons_mut(&mut self) -> &mut SkeletonDB {
        &mut self.skeletons
    }

    /// Returns an immutable reference to the animation database.
    pub fn animations(&self) -> &AnimationDB {
        &self.animations
    }

    /// Returns a mutable reference to the animation database.
    pub fn animations_mut(&mut self) -> &mut AnimationDB {
        &mut self.animations
    }

    /// Returns an immutable reference to the shader database.
    pub fn shaders(&self) -> &ShaderDB {
        &self.shaders
    }

    /// Returns a mutable reference to the shader database.
    pub fn shaders_mut(&mut self) -> &mut ShaderDB {
        &mut self.shaders
    }

    /// Placeholder for accessing font assets.
    pub fn font(&self) -> &FontDB {
        todo!()
    }

    /// Enumerates all geometry entries available in the backing database.
    pub fn enumerate_geometry(&self) -> Vec<String> {
        self.geometry.enumerate_entries()
    }

    /// Enumerates all imagery entries available in the backing database.
    pub fn enumerate_images(&self) -> Vec<String> {
        self.imagery.enumerate_entries()
    }

    /// Enumerates all audio clips available in the backing database.
    pub fn enumerate_audio_clips(&self) -> Vec<String> {
        self.audio.enumerate_entries()
    }

    /// Enumerates skeleton assets available in the backing database.
    pub fn enumerate_skeletons(&self) -> Vec<String> {
        self.skeletons.enumerate_entries()
    }

    /// Enumerates animation clips available in the backing database.
    pub fn enumerate_animations(&self) -> Vec<String> {
        self.animations.enumerate_entries()
    }

    /// Enumerates shader module entries available for program creation.
    pub fn enumerate_shader_modules(&self) -> Vec<String> {
        self.shaders.enumerate_entries()
    }

    /// Enumerates logical texture definitions declared in the model layout.
    pub fn enumerate_textures(&self) -> Vec<String> {
        self.meta_layout
            .as_ref()
            .map(|layout| layout.textures.keys().cloned().collect())
            .unwrap_or_default()
    }

    /// Enumerates material definitions declared in the model layout.
    pub fn enumerate_materials(&self) -> Vec<String> {
        self.meta_layout
            .as_ref()
            .map(|layout| layout.materials.keys().cloned().collect())
            .unwrap_or_default()
    }

    /// Enumerates mesh definitions declared in the model layout.
    pub fn enumerate_meshes(&self) -> Vec<String> {
        self.meta_layout
            .as_ref()
            .map(|layout| layout.meshes.keys().cloned().collect())
            .unwrap_or_default()
    }

    /// Enumerates model definitions declared in the model layout.
    pub fn enumerate_models(&self) -> Vec<String> {
        self.meta_layout
            .as_ref()
            .map(|layout| layout.models.keys().cloned().collect())
            .unwrap_or_default()
    }

    /// Enumerates graphics shader definitions declared in the model layout.
    pub fn enumerate_shaders(&self) -> Vec<String> {
        self.meta_layout
            .as_ref()
            .map(|layout| layout.shaders.keys().cloned().collect())
            .unwrap_or_default()
    }

    /// Registers an existing bindless furikake state for asset imports.
    pub fn import_furikake_state(&mut self, state: &mut BindlessState) {
        self.furikake = Some(FurikakeBindings::new(state));
    }

    /// Builds a CPU-side model composed of host geometry, textures, and materials.
    pub fn fetch_model(&mut self, entry: DatabaseEntry<'_>) -> Result<HostModel, NorenError> {
        self.assemble_model(
            entry,
            |geometry_db, entry| geometry_db.fetch_raw_geometry(entry),
            |imagery_db, entry| imagery_db.fetch_raw_image(entry),
            |name, image| HostTexture { name, image },
            |name, textures, material| HostMaterial {
                name,
                textures,
                material,
            },
            |name, geometry, textures, material| HostMesh {
                name,
                geometry,
                textures,
                material,
            },
            |name, meshes| HostModel { name, meshes },
        )
    }

    /// Loads a GPU-ready model with device buffers, textures, and shaders.
    pub fn fetch_gpu_model(&mut self, entry: DatabaseEntry<'_>) -> Result<DeviceModel, NorenError> {
        self.assemble_model(
            entry,
            |geometry_db, entry| geometry_db.fetch_gpu_geometry(entry),
            |imagery_db, entry| imagery_db.fetch_gpu_image(entry),
            |_, image| DeviceTexture::new(image),
            |_, textures, material| DeviceMaterial::new(textures, material),
            |_, geometry, textures, material| DeviceMesh::new(geometry, textures, material),
            |name, meshes| DeviceModel { name, meshes },
        )
    }

    /// Builds a CPU-side mesh using the provided material entry instead of the layout default.
    pub fn fetch_mesh_with_material(
        &mut self,
        mesh_entry: DatabaseEntry<'_>,
        material_entry: &str,
    ) -> Result<HostMesh, NorenError> {
        self.assemble_mesh(
            mesh_entry,
            Some(material_entry),
            &mut |geometry_db, entry| geometry_db.fetch_raw_geometry(entry),
            &mut |imagery_db, entry| imagery_db.fetch_raw_image(entry),
            &mut |name, image| HostTexture { name, image },
            &mut |name, textures, material| HostMaterial {
                name,
                textures,
                material,
            },
            &mut |name, geometry, textures, material| HostMesh {
                name,
                geometry,
                textures,
                material,
            },
        )?
        .ok_or_else(NorenError::LookupFailure)
    }

    /// Builds a GPU-ready mesh using the provided material entry instead of the layout default.
    pub fn fetch_gpu_mesh_with_material(
        &mut self,
        mesh_entry: DatabaseEntry<'_>,
        material_entry: &str,
    ) -> Result<DeviceMesh, NorenError> {
        self.assemble_mesh(
            mesh_entry,
            Some(material_entry),
            &mut |geometry_db, entry| geometry_db.fetch_gpu_geometry(entry),
            &mut |imagery_db, entry| imagery_db.fetch_gpu_image(entry),
            &mut |_, image| DeviceTexture::new(image),
            &mut |_, textures, material| DeviceMaterial::new(textures, material),
            &mut |_, geometry, textures, material| DeviceMesh::new(geometry, textures, material),
        )?
        .ok_or_else(NorenError::LookupFailure)
    }

    /// Builds a CPU-side material with host images and furikake definitions.
    ///
    /// Returns the material alongside an optional furikake bindless handle when a
    /// bindless state has been imported.
    pub fn fetch_host_material(
        &mut self,
        entry: &str,
    ) -> Result<(HostMaterial, Option<dashi::Handle<FurikakeMaterial>>), NorenError> {
        let layout = self
            .meta_layout
            .as_ref()
            .ok_or_else(|| NorenError::LookupFailure())?;

        let (name, textures, material) = build_material_components(
            layout,
            &mut self.imagery,
            entry,
            &mut |name, image| HostTexture { name, image },
            &mut |imagery, tex_entry| imagery.fetch_raw_image(tex_entry),
        )?
        .ok_or_else(NorenError::LookupFailure)?;

        let furikake_handle = self.ensure_furikake_material(entry).transpose()?;

        Ok((
            HostMaterial {
                name,
                textures,
                material,
            },
            furikake_handle,
        ))
    }

    /// Builds a GPU-ready material with device textures and furikake definitions.
    ///
    /// Returns the material alongside an optional furikake bindless handle when a
    /// bindless state has been imported.
    pub fn fetch_device_material(
        &mut self,
        entry: &str,
    ) -> Result<(DeviceMaterial, Option<dashi::Handle<FurikakeMaterial>>), NorenError> {
        let layout = self
            .meta_layout
            .as_ref()
            .ok_or_else(|| NorenError::LookupFailure())?;

        let (_name, textures, material) = build_material_components(
            layout,
            &mut self.imagery,
            entry,
            &mut |_, image| DeviceTexture::new(image),
            &mut |imagery, tex_entry| imagery.fetch_gpu_image(tex_entry),
        )?
        .ok_or_else(NorenError::LookupFailure)?;

        let furikake_handle = self.ensure_furikake_material(entry).transpose()?;

        Ok((DeviceMaterial::new(textures, material), furikake_handle))
    }

    /// Fetches a graphics shader definition.
    pub fn fetch_graphics_shader(
        &mut self,
        entry: DatabaseEntry<'_>,
    ) -> Result<GraphicsShader, NorenError> {
        let shader_layout = {
            let layout = self
                .meta_layout
                .as_ref()
                .ok_or_else(|| NorenError::LookupFailure())?;
            layout
                .shaders
                .get(entry)
                .cloned()
                .ok_or_else(|| NorenError::LookupFailure())?
        };

        let shader = Self::load_graphics_shader(&mut self.shaders, entry, &shader_layout)?
            .ok_or_else(NorenError::LookupFailure)?;

        furikake_state::validate_furikake_state(&shader, shader.furikake_state)?;

        Ok(shader)
    }

    fn furikake_bindings_mut(&mut self) -> Result<&mut FurikakeBindings, NorenError> {
        self.furikake
            .as_mut()
            .ok_or_else(|| NorenError::FurikakeError("furikake state not imported".to_string()))
    }

    fn ensure_furikake_texture(
        &mut self,
        entry: DatabaseEntry<'_>,
    ) -> Option<Result<u16, NorenError>> {
        let Some(_) = self.furikake else {
            return None;
        };

        if let Some(id) = self
            .furikake
            .as_ref()
            .and_then(|bindings| bindings.textures.get(entry))
        {
            return Some(Ok(*id));
        }

        let device_image = match self.imagery.fetch_gpu_image(entry) {
            Ok(image) => image,
            Err(err) => return Some(Err(err)),
        };
        let view = dashi::ImageView {
            img: device_image.img,
            ..Default::default()
        };

        let mut inserted_id = None;
        {
            let bindings = match self.furikake_bindings_mut() {
                Ok(bindings) => bindings,
                Err(err) => return Some(Err(err)),
            };
            let result = bindings
                .state_mut()
                .reserved_mut::<ReservedBindlessTextures, _>(
                    "meshi_bindless_textures",
                    |textures| {
                        inserted_id = Some(textures.add_texture(view));
                    },
                );

            if let Err(err) = result {
                return Some(Err(err.into()));
            }

            let id = match inserted_id {
                Some(id) => id,
                None => {
                    return Some(Err(NorenError::FurikakeError(
                        "failed to allocate furikake texture slot".to_string(),
                    )));
                }
            };

            bindings.textures.insert(entry.to_string(), id);
            Some(Ok(id))
        }
    }

    fn ensure_furikake_material(
        &mut self,
        entry: &str,
    ) -> Option<Result<dashi::Handle<FurikakeMaterial>, NorenError>> {
        let Some(_) = self.furikake else {
            return None;
        };

        if let Some(handle) = self
            .furikake
            .as_ref()
            .and_then(|bindings| bindings.materials.get(entry))
        {
            return Some(Ok(*handle));
        }

        let layout = match self.meta_layout.as_ref() {
            Some(layout) => layout,
            None => return Some(Err(NorenError::LookupFailure())),
        };

        let Some(material_def) = layout.materials.get(entry) else {
            return Some(Err(NorenError::LookupFailure()));
        };

        let mut furikake_material = FurikakeMaterial {
            render_mask: material_def.render_mask,
            ..Default::default()
        };

        let mut texture_ids: HashMap<String, u16> = HashMap::new();
        let mut texture_entries: Vec<(MaterialTextureSlot, String, String)> = Vec::new();

        for (slot, texture_key) in material_texture_lookups(&material_def.texture_lookups) {
            let Some(tex_key) = texture_key else { continue };

            let tex_def = match layout.textures.get(tex_key) {
                Some(tex_def) => tex_def,
                None => {
                    return Some(Err(NorenError::InvalidMaterial(format!(
                        "Material '{entry}' references missing texture '{tex_key}'",
                    ))));
                }
            };

            if tex_def.image.is_empty() {
                return Some(Err(NorenError::InvalidMaterial(format!(
                    "Material '{entry}' references texture '{tex_key}' without an image",
                ))));
            }

            texture_entries.push((slot, tex_key.to_string(), tex_def.image.clone()));
        }

        for (slot, tex_key, image_entry) in texture_entries {
            let id = if let Some(id) = texture_ids.get(&tex_key) {
                *id
            } else {
                let id = match self.ensure_furikake_texture(image_entry.as_str()) {
                    Some(Ok(id)) => id,
                    Some(Err(err)) => return Some(Err(err)),
                    None => return None,
                };
                texture_ids.insert(tex_key.clone(), id);
                id
            };

            match slot {
                MaterialTextureSlot::BaseColor => furikake_material.base_color_texture_id = id,
                MaterialTextureSlot::Normal => furikake_material.normal_texture_id = id,
                MaterialTextureSlot::MetallicRoughness => {
                    furikake_material.metallic_roughness_texture_id = id
                }
                MaterialTextureSlot::Occlusion => furikake_material.occlusion_texture_id = id,
                MaterialTextureSlot::Emissive => furikake_material.emissive_texture_id = id,
            }
        }

        let mut handle = None;
        {
            let bindings = match self.furikake_bindings_mut() {
                Ok(bindings) => bindings,
                Err(err) => return Some(Err(err)),
            };
            let result = bindings
                .state_mut()
                .reserved_mut::<ReservedBindlessMaterials, _>(
                    "meshi_bindless_materials",
                    |materials| {
                        let material_handle = materials.add_material();
                        *materials.material_mut(material_handle) = furikake_material;
                        handle = Some(material_handle);
                    },
                );

            if let Err(err) = result {
                return Some(Err(err.into()));
            }

            let handle = match handle {
                Some(handle) => handle,
                None => {
                    return Some(Err(NorenError::FurikakeError(
                        "failed to allocate furikake material slot".to_string(),
                    )));
                }
            };

            bindings.materials.insert(entry.to_string(), handle);
            Some(Ok(handle))
        }
    }
}

fn append_texture_bindings<Texture, Image, MakeTexture, FetchImage>(
    output: &mut Vec<Texture>,
    keys: &[String],
    layout: &MetaLayout,
    imagery: &mut ImageDB,
    make_texture: &mut MakeTexture,
    fetch_image: &mut FetchImage,
) -> Result<(), NorenError>
where
    MakeTexture: FnMut(String, Image) -> Texture,
    FetchImage: FnMut(&mut ImageDB, DatabaseEntry<'_>) -> Result<Image, NorenError>,
{
    for tex_key in keys {
        if let Some(tex_def) = layout.textures.get(tex_key) {
            if tex_def.image.is_empty() {
                continue;
            }
            let image = fetch_image(imagery, tex_def.image.as_str())?;
            let name = tex_def.name.clone().unwrap_or_else(|| tex_key.clone());
            output.push(make_texture(name, image));
        }
    }
    Ok(())
}

fn validate_material_links(layout: &MetaLayout) -> Result<(), NorenError> {
    for (material_key, material) in &layout.materials {
        for (_, texture_key) in material_texture_lookups(&material.texture_lookups) {
            let Some(texture_key) = texture_key else {
                continue;
            };
            if !layout.textures.contains_key(texture_key) {
                return Err(NorenError::InvalidMaterial(format!(
                    "Material '{material_key}' references missing texture '{texture_key}'",
                )));
            }
        }
    }

    Ok(())
}

fn validate_mesh_links(layout: &MetaLayout) -> Result<(), NorenError> {
    for (mesh_key, mesh) in &layout.meshes {
        for texture_key in &mesh.textures {
            if !layout.textures.contains_key(texture_key) {
                return Err(NorenError::InvalidModel(format!(
                    "Mesh '{mesh_key}' references missing texture '{texture_key}'",
                )));
            }
        }

        if let Some(material_key) = &mesh.material {
            if !layout.materials.contains_key(material_key) {
                return Err(NorenError::InvalidMaterial(format!(
                    "Mesh '{mesh_key}' references missing material '{material_key}'",
                )));
            }
        }
    }

    Ok(())
}

fn validate_model_links(layout: &MetaLayout) -> Result<(), NorenError> {
    for (model_key, model) in &layout.models {
        for mesh_key in &model.meshes {
            if !layout.meshes.contains_key(mesh_key) {
                return Err(NorenError::InvalidModel(format!(
                    "Model '{model_key}' references missing mesh '{mesh_key}'",
                )));
            }
        }
    }

    Ok(())
}

fn validate_meta_layout(
    layout: &MetaLayout,
    shader_modules: Option<&ShaderDB>,
) -> Result<(), NorenError> {
    validate_material_links(layout)?;
    validate_mesh_links(layout)?;
    validate_model_links(layout)?;
    validate_shader_layouts(layout, shader_modules)
}

fn build_material_components<Texture, Image, MakeTexture, FetchImage>(
    layout: &MetaLayout,
    imagery: &mut ImageDB,
    material_key: &str,
    make_texture: &mut MakeTexture,
    fetch_image: &mut FetchImage,
) -> Result<Option<(String, Vec<Texture>, FurikakeMaterial)>, NorenError>
where
    MakeTexture: FnMut(String, Image) -> Texture,
    FetchImage: FnMut(&mut ImageDB, DatabaseEntry<'_>) -> Result<Image, NorenError>,
{
    let Some(material_def) = layout.materials.get(material_key) else {
        return Ok(None);
    };

    let mut textures = Vec::new();
    let mut furikake_material = FurikakeMaterial {
        render_mask: material_def.render_mask,
        ..Default::default()
    };

    let mut lookup_indices: HashMap<String, u16> = HashMap::new();

    for (slot, texture_key) in material_texture_lookups(&material_def.texture_lookups) {
        let Some(tex_key) = texture_key else {
            continue;
        };

        let tex_def = layout.textures.get(tex_key).ok_or_else(|| {
            NorenError::InvalidMaterial(format!(
                "Material '{material_key}' references missing texture '{tex_key}'",
            ))
        })?;

        if tex_def.image.is_empty() {
            return Err(NorenError::InvalidMaterial(format!(
                "Material '{material_key}' references texture '{tex_key}' without an image",
            )));
        }

        let id = if let Some(id) = lookup_indices.get(tex_key) {
            *id
        } else {
            let image = fetch_image(imagery, tex_def.image.as_str())?;
            let name = tex_def.name.clone().unwrap_or_else(|| tex_key.to_string());
            let id = textures.len() as u16;
            textures.push(make_texture(name, image));
            lookup_indices.insert(tex_key.to_string(), id);
            id
        };

        match slot {
            MaterialTextureSlot::BaseColor => furikake_material.base_color_texture_id = id,
            MaterialTextureSlot::Normal => furikake_material.normal_texture_id = id,
            MaterialTextureSlot::MetallicRoughness => {
                furikake_material.metallic_roughness_texture_id = id
            }
            MaterialTextureSlot::Occlusion => furikake_material.occlusion_texture_id = id,
            MaterialTextureSlot::Emissive => furikake_material.emissive_texture_id = id,
        }
    }

    let name = material_def
        .name
        .clone()
        .unwrap_or_else(|| material_key.to_string());

    Ok(Some((name, textures, furikake_material)))
}

#[derive(Copy, Clone)]
enum MaterialTextureSlot {
    BaseColor,
    Normal,
    MetallicRoughness,
    Occlusion,
    Emissive,
}

fn material_texture_lookups<'a>(
    lookups: &'a MaterialTextureLookups,
) -> [(MaterialTextureSlot, Option<&'a str>); 5] {
    [
        (
            MaterialTextureSlot::BaseColor,
            lookups.base_color.as_deref(),
        ),
        (MaterialTextureSlot::Normal, lookups.normal.as_deref()),
        (
            MaterialTextureSlot::MetallicRoughness,
            lookups.metallic_roughness.as_deref(),
        ),
        (MaterialTextureSlot::Occlusion, lookups.occlusion.as_deref()),
        (MaterialTextureSlot::Emissive, lookups.emissive.as_deref()),
    ]
}

fn validate_shader_layouts(
    layout: &MetaLayout,
    shader_modules: Option<&ShaderDB>,
) -> Result<(), NorenError> {
    use std::collections::HashSet;

    let available_modules: Option<HashSet<String>> =
        shader_modules.map(|db| db.enumerate_entries().into_iter().collect());

    let mut errors = Vec::new();

    for (shader_key, shader_layout) in &layout.shaders {
        let mut issues = Vec::new();
        if shader_layout.vertex.is_none()
            && shader_layout.fragment.is_none()
            && shader_layout.geometry.is_none()
            && shader_layout.tessellation_control.is_none()
            && shader_layout.tessellation_evaluation.is_none()
        {
            issues.push("no shader stages specified".to_string());
        }

        if let Some(modules) = &available_modules {
            let stages = [
                ("vertex", shader_layout.vertex.as_deref()),
                ("fragment", shader_layout.fragment.as_deref()),
                ("geometry", shader_layout.geometry.as_deref()),
                (
                    "tessellation_control",
                    shader_layout.tessellation_control.as_deref(),
                ),
                (
                    "tessellation_evaluation",
                    shader_layout.tessellation_evaluation.as_deref(),
                ),
            ];

            for (stage, entry) in stages {
                if let Some(entry) = entry {
                    if entry.is_empty() {
                        issues.push(format!("{stage} shader entry is empty"));
                    } else if !modules.contains(entry) {
                        issues.push(format!(
                            "{stage} shader module '{}' is missing from shader modules",
                            entry
                        ));
                    }
                }
            }
        }

        if shader_layout.color_formats.is_empty() && shader_layout.depth_format.is_none() {
            issues.push("no attachment formats specified".to_string());
        }

        if !issues.is_empty() {
            errors.push(ShaderValidationError {
                shader: shader_key.clone(),
                issues,
                materials: Vec::new(),
                models: Vec::new(),
            });
        }
    }

    if let Some(modules) = &available_modules {
        for (shader_key, compute_layout) in &layout.compute_shaders {
            let mut issues = Vec::new();

            let Some(entry) = compute_layout.entry.as_deref() else {
                issues.push("no compute shader entry specified".to_string());
                let diag = ShaderValidationError {
                    shader: shader_key.clone(),
                    issues,
                    materials: Vec::new(),
                    models: Vec::new(),
                };
                errors.push(diag);
                continue;
            };

            if entry.is_empty() {
                issues.push("compute shader entry is empty".to_string());
            } else if !modules.contains(entry) {
                issues.push(format!(
                    "compute shader module '{}' is missing from shader modules",
                    entry
                ));
            }

            if !issues.is_empty() {
                errors.push(ShaderValidationError {
                    shader: shader_key.clone(),
                    issues,
                    materials: Vec::new(),
                    models: Vec::new(),
                });
            }
        }
    }

    if errors.is_empty() {
        Ok(())
    } else {
        Err(NorenError::InvalidShaderLayout(errors))
    }
}

impl DB {
    fn assemble_mesh<
        Mesh,
        Material,
        Texture,
        Geometry,
        Image,
        FetchGeometry,
        FetchImage,
        MakeTexture,
        MakeMaterial,
        MakeMesh,
    >(
        &mut self,
        mesh_key: &str,
        material_override: Option<&str>,
        fetch_geometry: &mut FetchGeometry,
        fetch_image: &mut FetchImage,
        make_texture: &mut MakeTexture,
        make_material: &mut MakeMaterial,
        make_mesh: &mut MakeMesh,
    ) -> Result<Option<Mesh>, NorenError>
    where
        FetchGeometry: FnMut(&mut GeometryDB, DatabaseEntry<'_>) -> Result<Geometry, NorenError>,
        FetchImage: FnMut(&mut ImageDB, DatabaseEntry<'_>) -> Result<Image, NorenError>,
        MakeTexture: FnMut(String, Image) -> Texture,
        MakeMaterial: FnMut(String, Vec<Texture>, FurikakeMaterial) -> Material,
        MakeMesh: FnMut(String, Geometry, Vec<Texture>, Option<Material>) -> Mesh,
    {
        let layout = self
            .meta_layout
            .as_ref()
            .ok_or_else(|| NorenError::LookupFailure())?;

        let mesh_def = match layout.meshes.get(mesh_key) {
            Some(mesh) => mesh,
            None => return Ok(None),
        };

        if mesh_def.geometry.is_empty() {
            return Ok(None);
        }

        let geometry = fetch_geometry(&mut self.geometry, mesh_def.geometry.as_str())?;
        let mesh_name = mesh_def
            .name
            .clone()
            .unwrap_or_else(|| mesh_key.to_string());

        let mut mesh_textures = Vec::new();
        append_texture_bindings(
            &mut mesh_textures,
            &mesh_def.textures,
            layout,
            &mut self.imagery,
            make_texture,
            fetch_image,
        )?;

        let material_key = material_override.or_else(|| mesh_def.material.as_deref());
        let material = if let Some(material_key) = material_key {
            match build_material_components(
                layout,
                &mut self.imagery,
                material_key,
                make_texture,
                fetch_image,
            )? {
                Some((name, textures, material)) => Some(make_material(name, textures, material)),
                None if material_override.is_some() => return Err(NorenError::LookupFailure()),
                None => None,
            }
        } else {
            None
        };

        Ok(Some(make_mesh(
            mesh_name,
            geometry,
            mesh_textures,
            material,
        )))
    }

    fn assemble_model<
        Model,
        Mesh,
        Material,
        Texture,
        Geometry,
        Image,
        FetchGeometry,
        FetchImage,
        MakeTexture,
        MakeMaterial,
        MakeMesh,
        MakeModel,
    >(
        &mut self,
        entry: DatabaseEntry<'_>,
        mut fetch_geometry: FetchGeometry,
        mut fetch_image: FetchImage,
        mut make_texture: MakeTexture,
        mut make_material: MakeMaterial,
        mut make_mesh: MakeMesh,
        mut make_model: MakeModel,
    ) -> Result<Model, NorenError>
    where
        FetchGeometry: FnMut(&mut GeometryDB, DatabaseEntry<'_>) -> Result<Geometry, NorenError>,
        FetchImage: FnMut(&mut ImageDB, DatabaseEntry<'_>) -> Result<Image, NorenError>,
        MakeTexture: FnMut(String, Image) -> Texture,
        MakeMaterial: FnMut(String, Vec<Texture>, FurikakeMaterial) -> Material,
        MakeMesh: FnMut(String, Geometry, Vec<Texture>, Option<Material>) -> Mesh,
        MakeModel: FnMut(String, Vec<Mesh>) -> Model,
    {
        let (model_name, mesh_keys) = {
            let layout = self
                .meta_layout
                .as_ref()
                .ok_or_else(|| NorenError::LookupFailure())?;

            let model = layout
                .models
                .get(entry)
                .ok_or_else(|| NorenError::LookupFailure())?;

            (
                model.name.clone().unwrap_or_else(|| entry.to_string()),
                model.meshes.clone(),
            )
        };
        let mut meshes = Vec::new();

        for mesh_key in &mesh_keys {
            if let Some(mesh) = self.assemble_mesh(
                mesh_key,
                None,
                &mut fetch_geometry,
                &mut fetch_image,
                &mut make_texture,
                &mut make_material,
                &mut make_mesh,
            )? {
                meshes.push(mesh);
            }
        }

        Ok(make_model(model_name, meshes))
    }

    pub(crate) fn load_graphics_shader(
        shaders: &mut ShaderDB,
        shader_key: &str,
        layout: &GraphicsShaderLayout,
    ) -> Result<Option<GraphicsShader>, NorenError> {
        let mut shader = GraphicsShader::new(
            layout
                .name
                .clone()
                .unwrap_or_else(|| shader_key.to_string()),
        );
        shader.furikake_state = layout.furikake_state;

        let mut has_stage = false;

        if let Some(stage) = Self::load_optional_shader_stage(shaders, layout.vertex.as_deref())? {
            shader.vertex = Some(stage);
            has_stage = true;
        }

        if let Some(stage) = Self::load_optional_shader_stage(shaders, layout.fragment.as_deref())?
        {
            shader.fragment = Some(stage);
            has_stage = true;
        }

        if let Some(stage) = Self::load_optional_shader_stage(shaders, layout.geometry.as_deref())?
        {
            shader.geometry = Some(stage);
            has_stage = true;
        }

        if let Some(stage) =
            Self::load_optional_shader_stage(shaders, layout.tessellation_control.as_deref())?
        {
            shader.tessellation_control = Some(stage);
            has_stage = true;
        }

        if let Some(stage) =
            Self::load_optional_shader_stage(shaders, layout.tessellation_evaluation.as_deref())?
        {
            shader.tessellation_evaluation = Some(stage);
            has_stage = true;
        }

        if has_stage {
            Ok(Some(shader))
        } else {
            Ok(None)
        }
    }

    pub(crate) fn load_optional_shader_stage(
        shaders: &mut ShaderDB,
        entry: Option<&str>,
    ) -> Result<Option<ShaderStage>, NorenError> {
        match entry {
            Some(name) if !name.is_empty() => {
                let stage = Self::load_shader_stage(shaders, name)?;
                Ok(Some(stage))
            }
            _ => Ok(None),
        }
    }

    pub(crate) fn load_shader_stage(
        shaders: &mut ShaderDB,
        entry: &str,
    ) -> Result<ShaderStage, NorenError> {
        let module = Self::fetch_shader_module(shaders, entry)?;
        Ok(ShaderStage::new(entry.to_string(), module))
    }

    pub(crate) fn fetch_shader_module(
        shaders: &mut ShaderDB,
        entry: &str,
    ) -> Result<ShaderModule, NorenError> {
        shaders.fetch_module(entry)
    }

    fn vertex_description() -> dashi::VertexDescriptionInfo<'static> {
        const ENTRIES: [dashi::VertexEntryInfo; 5] = [
            dashi::VertexEntryInfo {
                format: dashi::ShaderPrimitiveType::Vec3,
                location: 0,
                offset: 0,
            },
            dashi::VertexEntryInfo {
                format: dashi::ShaderPrimitiveType::Vec3,
                location: 1,
                offset: 12,
            },
            dashi::VertexEntryInfo {
                format: dashi::ShaderPrimitiveType::Vec4,
                location: 2,
                offset: 24,
            },
            dashi::VertexEntryInfo {
                format: dashi::ShaderPrimitiveType::Vec2,
                location: 3,
                offset: 40,
            },
            dashi::VertexEntryInfo {
                format: dashi::ShaderPrimitiveType::Vec4,
                location: 4,
                offset: 48,
            },
        ];

        dashi::VertexDescriptionInfo {
            entries: &ENTRIES,
            stride: std::mem::size_of::<crate::rdb::primitives::Vertex>(),
            rate: dashi::VertexRate::Vertex,
        }
    }

    fn graphics_pipeline_inputs(
        &mut self,
        shader_key: &str,
    ) -> Result<GraphicsPipelineInputs, NorenError> {
        let shader_layout = self
            .meta_layout
            .as_ref()
            .ok_or_else(|| NorenError::LookupFailure())?
            .shaders
            .get(shader_key)
            .cloned()
            .ok_or_else(|| NorenError::LookupFailure())?;

        if shader_layout.geometry.is_some()
            || shader_layout.tessellation_control.is_some()
            || shader_layout.tessellation_evaluation.is_some()
        {
            return Err(NorenError::InvalidShaderState(
                "unsupported graphics shader stage specified".to_string(),
            ));
        }

        let shader = Self::load_graphics_shader(&mut self.shaders, shader_key, &shader_layout)?
            .ok_or_else(|| {
                NorenError::InvalidShaderState(format!(
                    "graphics shader '{shader_key}' does not declare any stages"
                ))
            })?;

        let ctx = self.ctx_mut()?;

        crate::meta::graphics_pipeline_inputs(ctx, shader_key, &shader_layout, shader)
    }

    fn build_graphics_pipeline_layout(
        &mut self,
        shader_key: &str,
        inputs: &GraphicsPipelineInputs,
    ) -> Result<dashi::Handle<dashi::GraphicsPipelineLayout>, NorenError> {
        if let Some(layout) = self.graphics_pipeline_layouts.get(shader_key) {
            return Ok(*layout);
        }

        let vertex_info = Self::vertex_description();
        let mut shader_infos: Vec<dashi::PipelineShaderInfo<'_>> = Vec::new();

        if let Some(stage) = inputs.shader.vertex.as_ref() {
            shader_infos.push(dashi::PipelineShaderInfo {
                stage: stage.module.artifact().stage,
                spirv: stage.module.words(),
                specialization: &[],
            });
        }

        if let Some(stage) = inputs.shader.fragment.as_ref() {
            shader_infos.push(dashi::PipelineShaderInfo {
                stage: stage.module.artifact().stage,
                spirv: stage.module.words(),
                specialization: &[],
            });
        }

        let layout_info = dashi::GraphicsPipelineLayoutInfo {
            debug_name: inputs.debug_name.as_str(),
            vertex_info,
            bg_layouts: inputs.layouts.bg_layouts,
            bt_layouts: inputs.layouts.bt_layouts,
            shaders: &shader_infos,
            details: Default::default(),
        };

        let handle = {
            let ctx = self.ctx_mut()?;
            ctx.make_graphics_pipeline_layout(&layout_info)
                .map_err(|_| NorenError::UploadFailure())?
        };

        self.graphics_pipeline_layouts
            .insert(shader_key.to_string(), handle);

        Ok(handle)
    }

    fn compute_pipeline_inputs(
        &mut self,
        shader_key: &str,
    ) -> Result<ComputePipelineInputs, NorenError> {
        let shader_layout = self
            .meta_layout
            .as_ref()
            .ok_or_else(|| NorenError::LookupFailure())?
            .compute_shaders
            .get(shader_key)
            .cloned()
            .ok_or_else(|| NorenError::LookupFailure())?;

        let entry = shader_layout
            .entry
            .as_deref()
            .ok_or_else(|| NorenError::LookupFailure())?;

        let stage = Self::load_shader_stage(&mut self.shaders, entry)?;

        let ctx = self.ctx_mut()?;

        crate::meta::compute_pipeline_inputs(ctx, shader_key, &shader_layout, stage)
    }

    fn build_compute_pipeline_layout(
        &mut self,
        shader_key: &str,
        inputs: &ComputePipelineInputs,
    ) -> Result<dashi::Handle<dashi::ComputePipelineLayout>, NorenError> {
        if let Some(layout) = self.compute_pipeline_layouts.get(shader_key) {
            return Ok(*layout);
        }

        let layout_info = dashi::ComputePipelineLayoutInfo {
            bg_layouts: inputs.layouts.bg_layouts,
            bt_layouts: inputs.layouts.bt_layouts,
            shader: &dashi::PipelineShaderInfo {
                stage: inputs.stage.module.artifact().stage,
                spirv: inputs.stage.module.words(),
                specialization: &[],
            },
        };

        let handle = {
            let ctx = self.ctx_mut()?;
            ctx.make_compute_pipeline_layout(&layout_info)
                .map_err(|_| NorenError::UploadFailure())?
        };

        self.compute_pipeline_layouts
            .insert(shader_key.to_string(), handle);

        Ok(handle)
    }

    /// Builds or retrieves a cached graphics pipeline layout for the shader key.
    pub fn make_pipeline_layout(
        &mut self,
        shader_key: &str,
    ) -> Result<dashi::Handle<dashi::GraphicsPipelineLayout>, NorenError> {
        if let Some(layout) = self.graphics_pipeline_layouts.get(shader_key) {
            return Ok(*layout);
        }

        let inputs = self.graphics_pipeline_inputs(shader_key)?;
        self.build_graphics_pipeline_layout(shader_key, &inputs)
    }

    /// Builds or retrieves a cached graphics pipeline for the shader key.
    pub fn make_graphics_pipeline(
        &mut self,
        shader_key: &str,
    ) -> Result<dashi::Handle<dashi::GraphicsPipeline>, NorenError> {
        if let Some(pipeline) = self.graphics_pipelines.get(shader_key) {
            return Ok(*pipeline);
        }

        let inputs = self.graphics_pipeline_inputs(shader_key)?;
        let pipeline_layout = self.build_graphics_pipeline_layout(shader_key, &inputs)?;

        let pipeline_info = dashi::GraphicsPipelineInfo {
            debug_name: inputs.debug_name.as_str(),
            layout: pipeline_layout,
            attachment_formats: inputs.color_formats,
            depth_format: inputs.depth_format,
            subpass_samples: inputs.subpass_samples,
            subpass_id: 0,
        };

        let handle = {
            let ctx = self.ctx_mut()?;
            ctx.make_graphics_pipeline(&pipeline_info)
                .map_err(|_| NorenError::UploadFailure())?
        };

        self.graphics_pipelines
            .insert(shader_key.to_string(), handle);

        Ok(handle)
    }

    /// Builds or retrieves a cached compute pipeline layout for the shader key.
    pub fn make_compute_pipeline_layout(
        &mut self,
        shader_key: &str,
    ) -> Result<dashi::Handle<dashi::ComputePipelineLayout>, NorenError> {
        if let Some(layout) = self.compute_pipeline_layouts.get(shader_key) {
            return Ok(*layout);
        }

        let inputs = self.compute_pipeline_inputs(shader_key)?;
        self.build_compute_pipeline_layout(shader_key, &inputs)
    }

    /// Builds or retrieves a cached compute pipeline for the shader key.
    pub fn make_compute_pipeline(
        &mut self,
        shader_key: &str,
    ) -> Result<dashi::Handle<dashi::ComputePipeline>, NorenError> {
        if let Some(pipeline) = self.compute_pipelines.get(shader_key) {
            return Ok(*pipeline);
        }

        let inputs = self.compute_pipeline_inputs(shader_key)?;
        let pipeline_layout = self.build_compute_pipeline_layout(shader_key, &inputs)?;

        let pipeline_info = dashi::ComputePipelineInfo {
            debug_name: inputs.debug_name.as_str(),
            layout: pipeline_layout,
        };

        let handle = {
            let ctx = self.ctx_mut()?;
            ctx.make_compute_pipeline(&pipeline_info)
                .map_err(|_| NorenError::UploadFailure())?
        };

        self.compute_pipelines
            .insert(shader_key.to_string(), handle);

        Ok(handle)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parsing::{
        ComputeShaderLayout, GraphicsShaderLayout, MaterialLayout, MaterialLayoutFile,
        MaterialTextureLookups, MeshLayout, MeshLayoutFile, ModelLayout, ModelLayoutFile,
        ShaderLayoutFile, TextureLayout, TextureLayoutFile,
    };
    use crate::rdb::{
        ShaderModule,
        geometry::HostGeometry,
        imagery::{HostImage, ImageInfo},
        primitives::Vertex,
    };
    use crate::utils::rdbfile::RDBFile;
    use std::fs::File;
    use tempfile::tempdir;

    const MODEL_ENTRY: DatabaseEntry<'static> = "model/simple";
    const GEOMETRY_ENTRY: &str = "geom/simple_mesh";
    const IMAGE_ENTRY: &str = "imagery/sample_texture";
    const MATERIAL_ENTRY: &str = "material/simple";
    const ALT_MATERIAL_ENTRY: &str = "material/alternative";
    const MESH_ENTRY: &str = "mesh/simple_mesh";
    const MESH_MISSING_GEOMETRY: &str = "mesh/no_geometry";
    const MESH_TEXTURE_ENTRY: &str = "texture/mesh_texture";
    const SHADER_PROGRAM_ENTRY: &str = "shader/program";
    const SHADER_VERTEX_MODULE: &str = "shader/program.vert";
    const SHADER_FRAGMENT_MODULE: &str = "shader/program.frag";
    const SHADER_MISSING_ENTRY: &str = "shader/missing";

    fn sample_vertex(x: f32) -> Vertex {
        Vertex {
            position: [x, 0.0, 0.0],
            normal: [0.0, 1.0, 0.0],
            tangent: [1.0, 0.0, 0.0, 1.0],
            uv: [0.0, 0.0],
            color: [1.0, 1.0, 1.0, 1.0],
        }
    }

    #[test]
    fn load_sample_model_definition() -> Result<(), NorenError> {
        let tmp = tempdir().expect("create temp dir");
        let base_dir = tmp.path();

        let textures_dst = base_dir.join("textures.json");
        let materials_dst = base_dir.join("materials.json");
        let meshes_dst = base_dir.join("meshes.json");
        let models_dst = base_dir.join("models.json");
        let shaders_dst = base_dir.join("shaders.json");
        let mut textures = TextureLayoutFile::default();
        let mut materials = MaterialLayoutFile::default();
        let mut meshes = MeshLayoutFile::default();
        let mut models = ModelLayoutFile::default();
        let mut shaders = ShaderLayoutFile::default();

        textures.textures.insert(
            MESH_TEXTURE_ENTRY.to_string(),
            TextureLayout {
                image: IMAGE_ENTRY.to_string(),
                name: None,
            },
        );

        materials.materials.insert(
            MATERIAL_ENTRY.to_string(),
            MaterialLayout {
                name: None,
                render_mask: 3,
                texture_lookups: MaterialTextureLookups {
                    base_color: Some(MESH_TEXTURE_ENTRY.to_string()),
                    normal: None,
                    metallic_roughness: None,
                    occlusion: None,
                    emissive: None,
                },
            },
        );

        materials.materials.insert(
            ALT_MATERIAL_ENTRY.to_string(),
            MaterialLayout {
                name: Some("Override Material".to_string()),
                render_mask: 1,
                texture_lookups: MaterialTextureLookups {
                    base_color: Some(MESH_TEXTURE_ENTRY.to_string()),
                    normal: Some(MESH_TEXTURE_ENTRY.to_string()),
                    ..Default::default()
                },
            },
        );

        shaders.shaders.insert(
            SHADER_PROGRAM_ENTRY.to_string(),
            GraphicsShaderLayout {
                name: None,
                vertex: Some(SHADER_VERTEX_MODULE.to_string()),
                fragment: Some(SHADER_FRAGMENT_MODULE.to_string()),
                geometry: None,
                tessellation_control: None,
                tessellation_evaluation: None,
                color_formats: vec![dashi::Format::RGBA8],
                depth_format: None,
                furikake_state: FurikakeState::None,
            },
        );

        meshes.meshes.insert(
            MESH_ENTRY.to_string(),
            MeshLayout {
                name: Some("Simple Mesh".to_string()),
                geometry: GEOMETRY_ENTRY.to_string(),
                material: Some(MATERIAL_ENTRY.to_string()),
                textures: vec![MESH_TEXTURE_ENTRY.to_string()],
            },
        );

        meshes.meshes.insert(
            MESH_MISSING_GEOMETRY.to_string(),
            MeshLayout {
                name: Some("No Geometry".to_string()),
                geometry: String::new(),
                material: None,
                textures: vec![MESH_TEXTURE_ENTRY.to_string()],
            },
        );

        models.models.insert(
            MODEL_ENTRY.to_string(),
            ModelLayout {
                name: None,
                meshes: vec![MESH_ENTRY.to_string(), MESH_MISSING_GEOMETRY.to_string()],
            },
        );

        serde_json::to_writer(File::create(&textures_dst)?, &textures)?;
        serde_json::to_writer(File::create(&materials_dst)?, &materials)?;
        serde_json::to_writer(File::create(&meshes_dst)?, &meshes)?;
        serde_json::to_writer(File::create(&models_dst)?, &models)?;
        serde_json::to_writer(File::create(&shaders_dst)?, &shaders)?;

        let parsed_shader_file: ShaderLayoutFile =
            serde_json::from_reader(File::open(&shaders_dst)?)?;
        let parsed_shader = parsed_shader_file
            .shaders
            .get(SHADER_PROGRAM_ENTRY)
            .expect("shader entry");
        assert!(parsed_shader.depth_format.is_none());

        let mut geom_rdb = RDBFile::new();
        let geom = HostGeometry {
            vertices: vec![sample_vertex(0.0), sample_vertex(1.0), sample_vertex(2.0)],
            indices: Some(vec![0, 1, 2]),
            lods: Vec::new(),
        };
        geom_rdb.add(GEOMETRY_ENTRY, &geom)?;
        geom_rdb.save(base_dir.join("geometry.rdb"))?;

        let image_info = ImageInfo {
            name: IMAGE_ENTRY.to_string(),
            dim: [1, 1, 1],
            layers: 1,
            format: dashi::Format::RGBA8,
            mip_levels: 1,
        };
        let host_image = HostImage {
            info: image_info,
            data: vec![255, 255, 255, 255],
        };
        let mut img_rdb = RDBFile::new();
        img_rdb.add(IMAGE_ENTRY, &host_image)?;
        img_rdb.save(base_dir.join("imagery.rdb"))?;

        let mut shader_rdb = RDBFile::new();
        shader_rdb.add(
            SHADER_VERTEX_MODULE,
            &ShaderModule::from_words(vec![0x0723_0203, 1, 2, 3]),
        )?;
        shader_rdb.add(
            SHADER_FRAGMENT_MODULE,
            &ShaderModule::from_words(vec![0x0723_0203, 4, 5, 6]),
        )?;
        shader_rdb.save(base_dir.join("shaders.rdb"))?;

        let mut ctx =
            dashi::Context::headless(&Default::default()).expect("create headless context");

        let db_info = DBInfo {
            base_dir: base_dir.to_str().expect("base dir to str"),
            layout_file: None,
        };

        let mut db = DB::new(&db_info)?;
        db.import_dashi_context(&mut ctx);
        let host_model = db.fetch_model(MODEL_ENTRY)?;
        assert_eq!(host_model.name, MODEL_ENTRY);
        assert_eq!(host_model.meshes.len(), 1);
        let mesh = &host_model.meshes[0];
        assert_eq!(mesh.name, "Simple Mesh");
        assert_eq!(mesh.geometry.vertices.len(), 3);
        assert_eq!(mesh.textures.len(), 1);
        assert_eq!(mesh.textures[0].name, MESH_TEXTURE_ENTRY);
        assert!(mesh.material.is_some());
        let mat = mesh.material.as_ref().unwrap();
        assert_eq!(mat.name, MATERIAL_ENTRY);
        assert_eq!(mat.textures.len(), 1);
        assert_eq!(mat.textures[0].name, MESH_TEXTURE_ENTRY);
        assert_eq!(mat.material.render_mask, 3);
        assert_eq!(mat.material.base_color_texture_id, 0);

        let overridden_mesh = db.fetch_mesh_with_material(MESH_ENTRY, ALT_MATERIAL_ENTRY)?;
        assert_eq!(overridden_mesh.name, "Simple Mesh");
        let override_mat = overridden_mesh.material.as_ref().unwrap();
        assert_eq!(override_mat.name, "Override Material");
        assert_eq!(override_mat.textures.len(), 1);
        assert_eq!(override_mat.textures[0].name, MESH_TEXTURE_ENTRY);
        assert_eq!(override_mat.material.render_mask, 1);
        assert_eq!(override_mat.material.base_color_texture_id, 0);
        assert_eq!(override_mat.material.normal_texture_id, 0);

        let device_override = db.fetch_gpu_mesh_with_material(MESH_ENTRY, ALT_MATERIAL_ENTRY)?;
        assert_eq!(device_override.textures.len(), 1);
        let device_override_mat = device_override.material.as_ref().unwrap();
        assert_eq!(device_override_mat.textures.len(), 1);
        assert_eq!(device_override_mat.material.render_mask, 1);
        assert_eq!(device_override_mat.material.base_color_texture_id, 0);

        let fetched_shader = db.fetch_graphics_shader(SHADER_PROGRAM_ENTRY)?;
        assert_eq!(fetched_shader.name, SHADER_PROGRAM_ENTRY);
        assert!(fetched_shader.vertex.is_some());
        assert!(fetched_shader.fragment.is_some());

        assert!(matches!(
            db.fetch_graphics_shader(SHADER_MISSING_ENTRY),
            Err(NorenError::LookupFailure())
        ));

        let device_model = db.fetch_gpu_model(MODEL_ENTRY)?;
        assert_eq!(device_model.name, MODEL_ENTRY);
        assert_eq!(device_model.meshes.len(), 1);
        let device_mesh = &device_model.meshes[0];
        assert!(device_mesh.material.is_some());
        let device_mat = device_mesh.material.as_ref().unwrap();
        assert_eq!(device_mesh.textures.len(), 1);
        assert_eq!(device_mat.textures.len(), 1);
        assert_eq!(device_mat.material.render_mask, 3);

        Ok(())
    }

    #[test]
    fn validate_compute_shader_references() -> Result<(), NorenError> {
        let base = tempdir()?;
        let mut shaders = ShaderLayoutFile::default();
        shaders.compute_shaders.insert(
            "shader/compute".into(),
            ComputeShaderLayout {
                name: Some("Compute".into()),
                entry: Some("shader/compute.comp".into()),
                furikake_state: FurikakeState::None,
            },
        );

        std::fs::write(
            base.path().join("shaders.json"),
            serde_json::to_vec(&shaders).unwrap(),
        )?;

        let shader_rdb = RDBFile::new();
        shader_rdb.save(base.path().join("shaders.rdb"))?;

        let error = validate_database_layout(base.path().to_str().unwrap(), None)
            .expect_err("validation should fail for missing compute module");

        match error {
            NorenError::InvalidShaderLayout(errors) => {
                assert_eq!(errors.len(), 1);
                let diag = &errors[0];
                assert_eq!(diag.shader, "shader/compute");
                assert!(
                    diag.issues
                        .iter()
                        .any(|issue| issue.contains("compute shader module"))
                );
            }
            other => panic!("unexpected error: {other:?}"),
        }

        Ok(())
    }

    #[test]
    fn validate_layouts_with_shader_stage() -> Result<(), NorenError> {
        let tmp = tempdir().unwrap();
        let base = tmp.path();

        let mut meshes = MeshLayoutFile::default();
        meshes.meshes.insert(
            "mesh".into(),
            MeshLayout {
                material: Some("mat".into()),
                ..Default::default()
            },
        );
        let mut models = ModelLayoutFile::default();
        models.models.insert(
            "model".into(),
            ModelLayout {
                meshes: vec!["mesh".into()],
                ..Default::default()
            },
        );
        let mut materials = MaterialLayoutFile::default();
        materials
            .materials
            .insert("mat".into(), MaterialLayout::default());
        let mut shaders = ShaderLayoutFile::default();
        shaders.shaders.insert(
            "shader".into(),
            GraphicsShaderLayout {
                vertex: Some("shader.vert".into()),
                color_formats: vec![dashi::Format::RGBA8],
                ..Default::default()
            },
        );

        std::fs::write(base.join("meshes.json"), serde_json::to_vec(&meshes)?)?;
        std::fs::write(base.join("models.json"), serde_json::to_vec(&models)?)?;
        std::fs::write(base.join("materials.json"), serde_json::to_vec(&materials)?)?;
        std::fs::write(base.join("shaders.json"), serde_json::to_vec(&shaders)?)?;

        validate_database_layout(base.to_str().unwrap(), None)
    }

    #[test]
    fn validation_reports_incompatible_shader() -> Result<(), NorenError> {
        let tmp = tempdir().unwrap();
        let base = tmp.path();

        let mut meshes = MeshLayoutFile::default();
        meshes.meshes.insert(
            "mesh".into(),
            MeshLayout {
                material: Some("mat".into()),
                ..Default::default()
            },
        );
        let mut models = ModelLayoutFile::default();
        models.models.insert(
            "model".into(),
            ModelLayout {
                meshes: vec!["mesh".into()],
                ..Default::default()
            },
        );
        let mut materials = MaterialLayoutFile::default();
        materials
            .materials
            .insert("mat".into(), MaterialLayout::default());
        let mut shaders = ShaderLayoutFile::default();
        shaders
            .shaders
            .insert("shader".into(), GraphicsShaderLayout::default());

        std::fs::write(
            base.join("meshes.json"),
            serde_json::to_vec(&meshes).unwrap(),
        )?;
        std::fs::write(
            base.join("models.json"),
            serde_json::to_vec(&models).unwrap(),
        )?;
        std::fs::write(
            base.join("materials.json"),
            serde_json::to_vec(&materials).unwrap(),
        )?;
        std::fs::write(
            base.join("shaders.json"),
            serde_json::to_vec(&shaders).unwrap(),
        )?;

        let error = validate_database_layout(base.to_str().unwrap(), None)
            .expect_err("validation should fail");

        match error {
            NorenError::InvalidShaderLayout(errors) => {
                assert_eq!(errors.len(), 1);
                let diag = &errors[0];
                assert_eq!(diag.shader, "shader");
                assert!(
                    diag.issues
                        .iter()
                        .any(|issue| issue.contains("no shader stages"))
                );
                assert!(diag.materials.is_empty());
                assert!(diag.models.is_empty());
            }
            other => panic!("unexpected error: {other:?}"),
        }

        Ok(())
    }

    #[test]
    fn mesh_override_missing_material_fails() -> Result<(), NorenError> {
        let tmp = tempdir().unwrap();
        let base = tmp.path();

        let mut textures = TextureLayoutFile::default();
        textures.textures.insert(
            MESH_TEXTURE_ENTRY.to_string(),
            TextureLayout {
                image: IMAGE_ENTRY.to_string(),
                name: None,
            },
        );

        let mut materials = MaterialLayoutFile::default();
        materials.materials.insert(
            MATERIAL_ENTRY.to_string(),
            MaterialLayout {
                name: Some("Base Material".to_string()),
                render_mask: 0,
                texture_lookups: MaterialTextureLookups {
                    base_color: Some(MESH_TEXTURE_ENTRY.to_string()),
                    ..Default::default()
                },
            },
        );

        let mut meshes = MeshLayoutFile::default();
        meshes.meshes.insert(
            MESH_ENTRY.to_string(),
            MeshLayout {
                name: Some("Simple Mesh".to_string()),
                geometry: GEOMETRY_ENTRY.to_string(),
                material: Some(MATERIAL_ENTRY.to_string()),
                textures: vec![],
            },
        );

        serde_json::to_writer(File::create(base.join("textures.json"))?, &textures)?;
        serde_json::to_writer(File::create(base.join("materials.json"))?, &materials)?;
        serde_json::to_writer(File::create(base.join("meshes.json"))?, &meshes)?;

        let mut geom_rdb = RDBFile::new();
        let geom = HostGeometry {
            vertices: vec![sample_vertex(0.0), sample_vertex(1.0), sample_vertex(2.0)],
            indices: Some(vec![0, 1, 2]),
            lods: Vec::new(),
        };
        geom_rdb.add(GEOMETRY_ENTRY, &geom)?;
        geom_rdb.save(base.join("geometry.rdb"))?;

        let image_info = ImageInfo {
            name: IMAGE_ENTRY.to_string(),
            dim: [1, 1, 1],
            layers: 1,
            format: dashi::Format::RGBA8,
            mip_levels: 1,
        };
        let host_image = HostImage {
            info: image_info,
            data: vec![255, 255, 255, 255],
        };
        let mut img_rdb = RDBFile::new();
        img_rdb.add(IMAGE_ENTRY, &host_image)?;
        img_rdb.save(base.join("imagery.rdb"))?;

        let shader_rdb = RDBFile::new();
        shader_rdb.save(base.join("shaders.rdb"))?;

        let mut ctx =
            dashi::Context::headless(&Default::default()).expect("create headless context");
        let db_info = DBInfo {
            base_dir: base.to_str().unwrap(),
            layout_file: None,
        };

        let mut db = DB::new_with_ctx(&db_info, Some(&mut ctx))?;
        let host_err = db
            .fetch_mesh_with_material(MESH_ENTRY, "material/missing")
            .expect_err("missing material override should fail");
        assert!(matches!(host_err, NorenError::LookupFailure()));

        let device_err = db
            .fetch_gpu_mesh_with_material(MESH_ENTRY, "material/missing")
            .expect_err("missing material override should fail");
        assert!(matches!(device_err, NorenError::LookupFailure()));

        Ok(())
    }

    #[test]
    fn mesh_override_without_geometry_fails() -> Result<(), NorenError> {
        let tmp = tempdir().unwrap();
        let base = tmp.path();

        let mut meshes = MeshLayoutFile::default();
        meshes.meshes.insert(
            MESH_MISSING_GEOMETRY.to_string(),
            MeshLayout {
                name: Some("No Geometry".to_string()),
                geometry: String::new(),
                material: Some(MATERIAL_ENTRY.to_string()),
                textures: vec![MESH_TEXTURE_ENTRY.to_string()],
            },
        );

        let mut materials = MaterialLayoutFile::default();
        materials.materials.insert(
            MATERIAL_ENTRY.to_string(),
            MaterialLayout {
                name: None,
                texture_lookups: MaterialTextureLookups {
                    base_color: Some(MESH_TEXTURE_ENTRY.to_string()),
                    ..Default::default()
                },
                render_mask: 0,
            },
        );

        let mut textures = TextureLayoutFile::default();
        textures.textures.insert(
            MESH_TEXTURE_ENTRY.to_string(),
            TextureLayout {
                image: IMAGE_ENTRY.to_string(),
                name: None,
            },
        );

        serde_json::to_writer(File::create(base.join("textures.json"))?, &textures)?;
        serde_json::to_writer(File::create(base.join("materials.json"))?, &materials)?;
        serde_json::to_writer(File::create(base.join("meshes.json"))?, &meshes)?;

        let geom_rdb = RDBFile::new();
        geom_rdb.save(base.join("geometry.rdb"))?;

        let image_info = ImageInfo {
            name: IMAGE_ENTRY.to_string(),
            dim: [1, 1, 1],
            layers: 1,
            format: dashi::Format::RGBA8,
            mip_levels: 1,
        };
        let host_image = HostImage {
            info: image_info,
            data: vec![255, 255, 255, 255],
        };
        let mut img_rdb = RDBFile::new();
        img_rdb.add(IMAGE_ENTRY, &host_image)?;
        img_rdb.save(base.join("imagery.rdb"))?;

        let shader_rdb = RDBFile::new();
        shader_rdb.save(base.join("shaders.rdb"))?;

        let mut ctx =
            dashi::Context::headless(&Default::default()).expect("create headless context");
        let db_info = DBInfo {
            base_dir: base.to_str().unwrap(),
            layout_file: None,
        };

        let mut db = DB::new_with_ctx(&db_info, Some(&mut ctx))?;
        let host_err = db
            .fetch_mesh_with_material(MESH_MISSING_GEOMETRY, MATERIAL_ENTRY)
            .expect_err("mesh with empty geometry should fail");
        assert!(matches!(host_err, NorenError::LookupFailure()));

        let device_err = db
            .fetch_gpu_mesh_with_material(MESH_MISSING_GEOMETRY, MATERIAL_ENTRY)
            .expect_err("mesh with empty geometry should fail");
        assert!(matches!(device_err, NorenError::LookupFailure()));

        Ok(())
    }
}

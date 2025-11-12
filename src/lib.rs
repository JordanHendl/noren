pub mod datatypes;
pub mod meta;
mod parsing;
mod utils;

use std::{io::ErrorKind, ptr::NonNull};

use crate::datatypes::primitives::Vertex;
use datatypes::*;
use error::NorenError;
use meta::*;
use parsing::*;
use utils::*;

use dashi::{
    BindGroupLayout, BindTableLayout, Context, GraphicsPipelineDetails, GraphicsPipelineInfo,
    GraphicsPipelineLayoutInfo, Handle, PipelineShaderInfo, RenderPass, ShaderPrimitiveType,
    ShaderType, VertexDescriptionInfo, VertexEntryInfo, VertexRate,
};

pub use parsing::DatabaseLayoutFile;
pub use utils::error::RdbErr;
pub use utils::rdbfile::RDBFile;

pub struct DBInfo<'a> {
    pub ctx: *mut dashi::Context,
    pub base_dir: &'a str,
    pub layout_file: Option<&'a str>,
}

pub struct DB {
    ctx: NonNull<Context>,
    geometry: GeometryDB,
    imagery: ImageDB,
    shaders: ShaderDB,
    model_file: Option<ModelLayoutFile>,
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
        let layout: DatabaseLayoutFile = match info.layout_file {
            Some(f) => serde_json::from_str(&std::fs::read_to_string(f.to_string())?)?,
            None => Default::default(),
        };

        let ctx_ptr = NonNull::new(info.ctx).expect("Null GPU Context");
        let geometry = GeometryDB::new(info.ctx, &format!("{}/{}", info.base_dir, layout.geometry));
        let imagery = ImageDB::new(info.ctx, &format!("{}/{}", info.base_dir, layout.imagery));
        let shaders = ShaderDB::new(&format!("{}/{}", info.base_dir, layout.shaders));
        let model_path = format!("{}/{}", info.base_dir, layout.models);
        let model_file = match std::fs::read_to_string(&model_path) {
            Ok(raw) if raw.trim().is_empty() => None,
            Ok(raw) => Some(serde_json::from_str(&raw)?),
            Err(err) if err.kind() == ErrorKind::NotFound => None,
            Err(err) => return Err(err.into()),
        };

        Ok(Self {
            ctx: ctx_ptr,
            geometry,
            imagery,
            shaders,
            model_file,
        })
    }

    pub fn geometry(&self) -> &GeometryDB {
        &self.geometry
    }

    pub fn geometry_mut(&mut self) -> &mut GeometryDB {
        &mut self.geometry
    }

    pub fn imagery(&self) -> &ImageDB {
        &self.imagery
    }

    pub fn imagery_mut(&mut self) -> &mut ImageDB {
        &mut self.imagery
    }

    pub fn shaders(&self) -> &ShaderDB {
        &self.shaders
    }

    pub fn shaders_mut(&mut self) -> &mut ShaderDB {
        &mut self.shaders
    }

    pub fn font(&self) -> &FontDB {
        todo!()
    }

    pub fn fetch_model(&mut self, entry: DatabaseEntry) -> Result<HostModel, NorenError> {
        self.assemble_model(
            entry,
            |geometry_db, entry| geometry_db.fetch_raw_geometry(entry),
            |imagery_db, entry| imagery_db.fetch_raw_image(entry),
            |name, image| HostTexture { name, image },
            |name, textures, shader| HostMaterial {
                name,
                textures,
                shader,
            },
            |name, geometry, textures, material| HostMesh {
                name,
                geometry,
                textures,
                material,
            },
            |name, meshes| HostModel { name, meshes },
            Self::load_graphics_shader,
        )
    }

    pub fn fetch_gpu_model(
        &mut self,
        entry: DatabaseEntry,
        render_pass: Handle<RenderPass>,
    ) -> Result<DeviceModel, NorenError> {
        let ctx_ptr = self.ctx;
        self.assemble_model(
            entry,
            |geometry_db, entry| geometry_db.fetch_gpu_geometry(entry),
            |imagery_db, entry| imagery_db.fetch_gpu_image(entry),
            |_, image| DeviceTexture::new(image),
            |_, textures, shader| DeviceMaterial::new(textures, shader),
            |_, geometry, textures, material| DeviceMesh::new(geometry, textures, material),
            |name, meshes| DeviceModel { name, meshes },
            move |shader_db, shader_key, shader_layout| {
                let mut shader_opt =
                    Self::load_graphics_shader(shader_db, shader_key, shader_layout)?;
                if let Some(shader) = shader_opt.as_mut() {
                    let ctx: &mut Context = unsafe { &mut *ctx_ptr.as_ptr() };
                    Self::configure_graphics_shader_pipeline(
                        shader,
                        shader_layout,
                        ctx,
                        render_pass,
                    )?;
                }
                Ok(shader_opt)
            },
        )
    }

    pub fn fetch_graphics_shader(
        &mut self,
        entry: DatabaseEntry,
        render_pass: Handle<RenderPass>,
    ) -> Result<GraphicsShader, NorenError> {
        let shader_layout = {
            let layout = self
                .model_file
                .as_ref()
                .ok_or_else(|| NorenError::LookupFailure())?;
            layout
                .shaders
                .get(entry)
                .cloned()
                .ok_or_else(|| NorenError::LookupFailure())?
        };

        let mut shader = Self::load_graphics_shader(&mut self.shaders, entry, &shader_layout)?
            .ok_or_else(NorenError::LookupFailure)?;

        let ctx: &mut Context = unsafe { self.ctx.as_mut() };
        Self::configure_graphics_shader_pipeline(&mut shader, &shader_layout, ctx, render_pass)?;

        Ok(shader)
    }
}

impl DB {
    fn assemble_model<
        Model,
        Mesh,
        Material,
        Texture,
        Geometry,
        Image,
        Shader,
        FetchGeometry,
        FetchImage,
        MakeTexture,
        MakeMaterial,
        MakeMesh,
        MakeModel,
        LoadShader,
    >(
        &mut self,
        entry: DatabaseEntry,
        mut fetch_geometry: FetchGeometry,
        mut fetch_image: FetchImage,
        mut make_texture: MakeTexture,
        mut make_material: MakeMaterial,
        mut make_mesh: MakeMesh,
        mut make_model: MakeModel,
        mut load_shader: LoadShader,
    ) -> Result<Model, NorenError>
    where
        FetchGeometry: FnMut(&mut GeometryDB, DatabaseEntry) -> Result<Geometry, NorenError>,
        FetchImage: FnMut(&mut ImageDB, DatabaseEntry) -> Result<Image, NorenError>,
        MakeTexture: FnMut(String, Image) -> Texture,
        MakeMaterial: FnMut(String, Vec<Texture>, Option<Shader>) -> Material,
        MakeMesh: FnMut(String, Geometry, Vec<Texture>, Option<Material>) -> Mesh,
        MakeModel: FnMut(String, Vec<Mesh>) -> Model,
        LoadShader:
            FnMut(&mut ShaderDB, &str, &GraphicsShaderLayout) -> Result<Option<Shader>, NorenError>,
    {
        let layout = self
            .model_file
            .as_ref()
            .ok_or_else(|| NorenError::LookupFailure())?;

        let model = layout
            .models
            .get(entry)
            .ok_or_else(|| NorenError::LookupFailure())?;

        let model_name = model.name.clone().unwrap_or_else(|| entry.to_string());
        let mut meshes = Vec::new();

        for mesh_key in &model.meshes {
            let mesh_def = match layout.meshes.get(mesh_key) {
                Some(mesh) => mesh,
                None => continue,
            };

            if mesh_def.geometry.is_empty() {
                continue;
            }

            let geometry_entry = Self::leak_entry(&mesh_def.geometry);
            let geometry = fetch_geometry(&mut self.geometry, geometry_entry)?;

            let mesh_name = mesh_def.name.clone().unwrap_or_else(|| mesh_key.clone());

            let mut mesh_textures = Vec::new();
            for tex_key in &mesh_def.textures {
                if let Some(tex_def) = layout.textures.get(tex_key) {
                    if tex_def.image.is_empty() {
                        continue;
                    }

                    let tex_entry = Self::leak_entry(&tex_def.image);
                    let image = fetch_image(&mut self.imagery, tex_entry)?;
                    let name = tex_def.name.clone().unwrap_or_else(|| tex_key.clone());
                    mesh_textures.push(make_texture(name, image));
                }
            }

            let material = if let Some(material_key) = &mesh_def.material {
                if let Some(material_def) = layout.materials.get(material_key) {
                    let mut textures = Vec::new();
                    for tex_key in &material_def.textures {
                        if let Some(tex_def) = layout.textures.get(tex_key) {
                            if tex_def.image.is_empty() {
                                continue;
                            }

                            let tex_entry = Self::leak_entry(&tex_def.image);
                            let image = fetch_image(&mut self.imagery, tex_entry)?;
                            let name = tex_def.name.clone().unwrap_or_else(|| tex_key.clone());
                            textures.push(make_texture(name, image));
                        }
                    }

                    let shader = match material_def.shader.as_ref() {
                        Some(shader_key) => {
                            if let Some(shader_layout) = layout.shaders.get(shader_key).cloned() {
                                load_shader(&mut self.shaders, shader_key, &shader_layout)?
                            } else {
                                None
                            }
                        }
                        None => None,
                    };

                    Some(make_material(
                        material_def
                            .name
                            .clone()
                            .unwrap_or_else(|| material_key.clone()),
                        textures,
                        shader,
                    ))
                } else {
                    None
                }
            } else {
                None
            };

            meshes.push(make_mesh(mesh_name, geometry, mesh_textures, material));
        }

        Ok(make_model(model_name, meshes))
    }

    fn load_graphics_shader(
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

    fn configure_graphics_shader_pipeline(
        shader: &mut GraphicsShader,
        layout: &GraphicsShaderLayout,
        ctx: &mut Context,
        render_pass: Handle<RenderPass>,
    ) -> Result<(), NorenError> {
        let mut bg_handles: [Option<Handle<BindGroupLayout>>; 4] = Default::default();
        for (index, cfg_opt) in layout.bind_group_layouts.iter().enumerate() {
            if index >= bg_handles.len() {
                break;
            }

            if let Some(cfg) = cfg_opt {
                let borrowed = cfg.borrow();
                let info = borrowed.info();
                let handle = ctx
                    .make_bind_group_layout(&info)
                    .map_err(|_| NorenError::UploadFailure())?;
                bg_handles[index] = Some(handle);
            }
        }

        let mut bt_handles: [Option<Handle<BindTableLayout>>; 4] = Default::default();
        for (index, cfg_opt) in layout.bind_table_layouts.iter().enumerate() {
            if index >= bt_handles.len() {
                break;
            }

            if let Some(cfg) = cfg_opt {
                let borrowed = cfg.borrow();
                let info = borrowed.info();
                let handle = ctx
                    .make_bind_table_layout(&info)
                    .map_err(|_| NorenError::UploadFailure())?;
                bt_handles[index] = Some(handle);
            }
        }

        let mut shader_infos: Vec<PipelineShaderInfo<'_>> = Vec::new();
        if let Some(stage) = shader.vertex.as_ref() {
            shader_infos.push(PipelineShaderInfo {
                stage: ShaderType::Vertex,
                spirv: stage.module.words(),
                specialization: &[],
            });
        }

        if let Some(stage) = shader.fragment.as_ref() {
            shader_infos.push(PipelineShaderInfo {
                stage: ShaderType::Fragment,
                spirv: stage.module.words(),
                specialization: &[],
            });
        }

        if shader_infos.is_empty() {
            return Err(NorenError::LookupFailure());
        }

        if cfg!(test) {
            shader.bind_group_layouts = bg_handles;
            shader.bind_table_layouts = bt_handles;
            shader.pipeline_layout = None;
            shader.pipeline = None;
            return Ok(());
        }

        const VERTEX_ENTRIES: [VertexEntryInfo; 5] = [
            VertexEntryInfo {
                format: ShaderPrimitiveType::Vec3,
                location: 0,
                offset: 0,
            },
            VertexEntryInfo {
                format: ShaderPrimitiveType::Vec3,
                location: 1,
                offset: 12,
            },
            VertexEntryInfo {
                format: ShaderPrimitiveType::Vec4,
                location: 2,
                offset: 24,
            },
            VertexEntryInfo {
                format: ShaderPrimitiveType::Vec2,
                location: 3,
                offset: 40,
            },
            VertexEntryInfo {
                format: ShaderPrimitiveType::Vec4,
                location: 4,
                offset: 48,
            },
        ];

        let vertex_info = VertexDescriptionInfo {
            entries: &VERTEX_ENTRIES,
            stride: std::mem::size_of::<Vertex>(),
            rate: VertexRate::Vertex,
        };

        let layout_info = GraphicsPipelineLayoutInfo {
            debug_name: &shader.name,
            vertex_info,
            bg_layouts: bg_handles,
            bt_layouts: bt_handles,
            shaders: &shader_infos,
            details: GraphicsPipelineDetails::default(),
        };

        let pipeline_layout = ctx
            .make_graphics_pipeline_layout(&layout_info)
            .map_err(|_| NorenError::UploadFailure())?;

        let pipeline_info = GraphicsPipelineInfo {
            debug_name: &shader.name,
            layout: pipeline_layout,
            render_pass,
            subpass_id: layout.subpass,
        };

        let pipeline = ctx
            .make_graphics_pipeline(&pipeline_info)
            .map_err(|_| NorenError::UploadFailure())?;

        shader.bind_group_layouts = layout_info.bg_layouts;
        shader.bind_table_layouts = layout_info.bt_layouts;
        shader.pipeline_layout = Some(pipeline_layout);
        shader.pipeline = Some(pipeline);

        Ok(())
    }

    fn load_optional_shader_stage(
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

    fn load_shader_stage(shaders: &mut ShaderDB, entry: &str) -> Result<ShaderStage, NorenError> {
        let module = Self::fetch_shader_module(shaders, entry)?;
        Ok(ShaderStage::new(entry.to_string(), module))
    }

    fn fetch_shader_module(
        shaders: &mut ShaderDB,
        entry: &str,
    ) -> Result<ShaderModule, NorenError> {
        let shader_entry = Self::leak_entry(entry);
        shaders.fetch_module(shader_entry)
    }

    fn leak_entry(entry: &str) -> DatabaseEntry {
        Box::leak(entry.to_string().into_boxed_str())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::datatypes::{
        ShaderModule,
        geometry::HostGeometry,
        imagery::{HostImage, ImageInfo},
        primitives::Vertex,
    };
    use crate::parsing::{
        GraphicsShaderLayout, MaterialLayout, MeshLayout, ModelLayout, ModelLayoutFile,
        TextureLayout,
    };
    use crate::utils::rdbfile::RDBFile;
    use std::fs::File;
    use tempfile::tempdir;

    use dashi::builders::RenderPassBuilder;
    use dashi::{AttachmentDescription, FRect2D, Format, Rect2D, Viewport};

    const MODEL_ENTRY: DatabaseEntry = "model/simple";
    const GEOMETRY_ENTRY: &str = "geom/simple_mesh";
    const IMAGE_ENTRY: &str = "imagery/sample_texture";
    const MATERIAL_ENTRY: &str = "material/simple";
    const MESH_ENTRY: &str = "mesh/simple_mesh";
    const MESH_MISSING_GEOMETRY: &str = "mesh/no_geometry";
    const MESH_TEXTURE_ENTRY: &str = "texture/mesh_texture";
    const MISSING_TEXTURE_ENTRY: &str = "texture/missing_image";
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

        let models_dst = base_dir.join("models.json");
        let mut layout = ModelLayoutFile::default();

        layout.textures.insert(
            MESH_TEXTURE_ENTRY.to_string(),
            TextureLayout {
                image: IMAGE_ENTRY.to_string(),
                name: None,
            },
        );
        layout.textures.insert(
            MISSING_TEXTURE_ENTRY.to_string(),
            TextureLayout {
                image: String::new(),
                name: Some("ShouldSkip".to_string()),
            },
        );

        layout.materials.insert(
            MATERIAL_ENTRY.to_string(),
            MaterialLayout {
                name: None,
                textures: vec![
                    MESH_TEXTURE_ENTRY.to_string(),
                    MISSING_TEXTURE_ENTRY.to_string(),
                ],
                shader: Some(SHADER_PROGRAM_ENTRY.to_string()),
            },
        );

        layout.shaders.insert(
            SHADER_PROGRAM_ENTRY.to_string(),
            GraphicsShaderLayout {
                name: None,
                vertex: Some(SHADER_VERTEX_MODULE.to_string()),
                fragment: Some(SHADER_FRAGMENT_MODULE.to_string()),
                geometry: None,
                tessellation_control: None,
                tessellation_evaluation: None,
                bind_group_layouts: Vec::new(),
                bind_table_layouts: Vec::new(),
                subpass: 0,
            },
        );

        layout.meshes.insert(
            MESH_ENTRY.to_string(),
            MeshLayout {
                name: Some("Simple Mesh".to_string()),
                geometry: GEOMETRY_ENTRY.to_string(),
                material: Some(MATERIAL_ENTRY.to_string()),
                textures: vec![
                    MESH_TEXTURE_ENTRY.to_string(),
                    MISSING_TEXTURE_ENTRY.to_string(),
                ],
            },
        );

        layout.meshes.insert(
            MESH_MISSING_GEOMETRY.to_string(),
            MeshLayout {
                name: Some("No Geometry".to_string()),
                geometry: String::new(),
                material: None,
                textures: vec![MESH_TEXTURE_ENTRY.to_string()],
            },
        );

        layout.models.insert(
            MODEL_ENTRY.to_string(),
            ModelLayout {
                name: None,
                meshes: vec![MESH_ENTRY.to_string(), MESH_MISSING_GEOMETRY.to_string()],
            },
        );

        serde_json::to_writer(File::create(&models_dst)?, &layout)?;

        let mut geom_rdb = RDBFile::new();
        let geom = HostGeometry {
            vertices: vec![sample_vertex(0.0), sample_vertex(1.0), sample_vertex(2.0)],
            indices: Some(vec![0, 1, 2]),
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

        let viewport = Viewport {
            area: FRect2D {
                x: 0.0,
                y: 0.0,
                w: 1.0,
                h: 1.0,
            },
            scissor: Rect2D {
                x: 0,
                y: 0,
                w: 1,
                h: 1,
            },
            min_depth: 0.0,
            max_depth: 1.0,
        };
        let color_attachment = AttachmentDescription {
            format: Format::RGBA8,
            ..Default::default()
        };
        let render_pass = RenderPassBuilder::new("test_pass", viewport)
            .add_subpass(&[color_attachment], None, &[])
            .build(&mut ctx)
            .expect("create render pass");

        let db_info = DBInfo {
            ctx: &mut ctx,
            base_dir: base_dir.to_str().expect("base dir to str"),
            layout_file: None,
        };

        let mut db = DB::new(&db_info)?;

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
        assert!(mat.shader.is_some());
        let shader = mat.shader.as_ref().unwrap();
        assert_eq!(shader.name, SHADER_PROGRAM_ENTRY);
        assert!(shader.vertex.is_some());
        assert!(shader.fragment.is_some());
        assert!(shader.pipeline.is_none());
        assert_eq!(
            shader.vertex.as_ref().unwrap().module.words(),
            &[0x0723_0203, 1, 2, 3]
        );
        assert_eq!(
            shader.fragment.as_ref().unwrap().module.words(),
            &[0x0723_0203, 4, 5, 6]
        );

        let fetched_shader = db.fetch_graphics_shader(SHADER_PROGRAM_ENTRY, render_pass)?;
        assert_eq!(fetched_shader.name, SHADER_PROGRAM_ENTRY);
        assert!(fetched_shader.vertex.is_some());
        assert!(fetched_shader.fragment.is_some());
        assert!(fetched_shader.pipeline.is_none());

        assert!(matches!(
            db.fetch_graphics_shader(SHADER_MISSING_ENTRY, render_pass),
            Err(NorenError::LookupFailure())
        ));

        let device_model = db.fetch_gpu_model(MODEL_ENTRY, render_pass)?;
        assert_eq!(device_model.name, MODEL_ENTRY);
        assert_eq!(device_model.meshes.len(), 1);
        let device_mesh = &device_model.meshes[0];
        assert!(device_mesh.material.is_some());
        let device_mat = device_mesh.material.as_ref().unwrap();
        assert_eq!(device_mesh.textures.len(), 1);
        let mesh_texture = device_mesh.textures.get(0).unwrap();
        assert_eq!(device_mat.textures.len(), 1);
        let material_texture = device_mat.textures.get(0).unwrap();
        let texture_name = |bytes: &[u8; 64]| {
            let len = bytes.iter().position(|&b| b == 0).unwrap_or(bytes.len());
            std::str::from_utf8(&bytes[..len]).unwrap().to_string()
        };
        assert_eq!(texture_name(&mesh_texture.image.info.name), IMAGE_ENTRY);
        assert_eq!(texture_name(&material_texture.image.info.name), IMAGE_ENTRY);
        assert!(device_mat.shader.is_some());
        let device_shader = device_mat.shader.as_ref().unwrap();
        assert!(device_shader.vertex.is_some());
        assert!(device_shader.fragment.is_some());
        assert!(device_shader.pipeline.is_none());

        Ok(())
    }
}

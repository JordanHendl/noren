//! Run with `cargo run --example material_render [material/key]` to fetch a
//! material from the bundled database and render a simple quad with it.
//! If no argument is provided the demo defaults to `material/quad`.
//! Materials that require bind groups or bind tables are rejected so the
//! example can stay minimal.

#[path = "../common/mod.rs"]
mod common;

use common::{SAMPLE_GEOMETRY_ENTRY, display::blit_image_to_display, init_context, open_sample_db};
use dashi::builders::{BindGroupBuilder, BindTableBuilder};
use dashi::driver::command::{BeginDrawing, DrawIndexed};
use dashi::gpu::{self, CommandStream};
use dashi::{
    BindGroupVariableType, BufferInfo, BufferUsage, BufferView, ClearValue, CommandQueueInfo2,
    DynamicAllocator, DynamicAllocatorInfo, DynamicBuffer, FRect2D, Handle, Image, ImageInfo,
    ImageView, IndexedResource, MemoryVisibility, Rect2D, SamplerInfo, ShaderResource, SubmitInfo,
    Viewport,
};
use bytemuck::{Pod, Zeroable, bytes_of, cast_slice};
use glam::Mat4;
use noren::meta::model::DeviceMaterial;
use noren::parsing::{DatabaseLayoutFile, GraphicsShaderLayout, ModelLayoutFile};
use std::error::Error;
use std::path::PathBuf;

const FALLBACK_DIM: u32 = 2;

fn main() {
    if let Err(err) = run() {
        eprintln!("error: {err}");
        std::process::exit(1);
    }
}

fn run() -> Result<(), Box<dyn Error>> {
    let material_entry = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "material/quad".to_string());

    let mut ctx = match init_context() {
        Ok(ctx) => ctx,
        Err(err) => {
            eprintln!("Skipping example – unable to create GPU context: {err}");
            return Ok(());
        }
    };

    let mut db = open_sample_db(&mut ctx)?;
    let layout = load_material_layout()?;

    let (host_geometry, device_geometry) = {
        let geometry = db.geometry_mut();
        (
            geometry.fetch_raw_geometry(SAMPLE_GEOMETRY_ENTRY)?,
            geometry.fetch_gpu_geometry(SAMPLE_GEOMETRY_ENTRY)?,
        )
    };

    let material_layout = layout
        .materials
        .get(&material_entry)
        .ok_or_else(|| format!("material '{material_entry}' not found in layout"))?;

    let shader_key = material_layout
        .shader
        .as_deref()
        .ok_or_else(|| format!("material '{material_entry}' does not reference a shader"))?;

    let shader_layout = layout
        .shaders
        .get(shader_key)
        .ok_or_else(|| format!("shader layout for '{shader_key}' missing"))?;

    let device_material = db.fetch_device_material(&material_entry)?;
    println!(
        "Fetched material '{}' with {} texture(s)",
        material_entry,
        device_material.textures.len()
    );

    let shader = device_material
        .shader
        .as_ref()
        .ok_or_else(|| "material has no associated shader")?;

    let pipeline = shader
        .pipeline
        .ok_or_else(|| "material shader has no pipeline handle")?;

    let framebuffer = ctx.make_image(&ImageInfo {
        debug_name: "material_render_fb",
        dim: [800, 600, 1],
        format: dashi::Format::RGBA8,
        ..Default::default()
    })?;

    let material_bindings = build_material_bindings(
        &mut ctx,
        shader,
        shader_layout,
        &device_material,
        shader_key,
        &material_entry,
    )?;

    render_quad_with_material(
        &mut ctx,
        &host_geometry,
        device_geometry,
        &material_bindings,
        pipeline,
        framebuffer,
    )?;

    println!("Displaying material render (800x600)");
    blit_image_to_display(&mut ctx, framebuffer, [800, 600], "material_render")?;
    Ok(())
}

fn render_quad_with_material(
    ctx: &mut gpu::Context,
    host_geometry: &noren::datatypes::geometry::HostGeometry,
    device_geometry: noren::datatypes::geometry::DeviceGeometry,
    bindings: &MaterialBindings,
    pipeline: Handle<dashi::GraphicsPipeline>,
    framebuffer: Handle<Image>,
) -> Result<(), dashi::GPUError> {
    let mut ring = ctx.make_command_ring(&CommandQueueInfo2 {
        debug_name: "material_render_ring",
        ..Default::default()
    })?;

    ring.record(|list| {
        let mut stream = CommandStream::new().begin();
        let viewport = Viewport {
            area: FRect2D {
                w: 800.0,
                h: 600.0,
                ..Default::default()
            },
            scissor: Rect2D {
                w: 800,
                h: 600,
                ..Default::default()
            },
            ..Default::default()
        };

        let mut draw = stream.begin_drawing(&BeginDrawing {
            viewport,
            pipeline,
            color_attachments: [
                Some(ImageView {
                    img: framebuffer,
                    ..Default::default()
                }),
                None,
                None,
                None,
            ],
            depth_attachment: None,
            clear_values: [
                Some(ClearValue::Color([0.02, 0.02, 0.05, 1.0])),
                None,
                None,
                None,
            ],
        });

        draw.draw_indexed(&DrawIndexed {
            vertices: device_geometry.vertices,
            indices: device_geometry.indices,
            bind_groups: bindings.bind_groups,
            bind_tables: bindings.bind_tables,
            index_count: host_geometry
                .indices
                .as_ref()
                .map(|idx| idx.len() as u32)
                .unwrap_or_default(),
            ..Default::default()
        });

        stream = draw.stop_drawing();
        stream.end().append(list);
    })?;

    ring.submit(&SubmitInfo::default())?;
    ring.wait_all()?;

    Ok(())
}

struct MaterialBindings {
    bind_groups: [Option<Handle<dashi::BindGroup>>; 4],
    bind_tables: [Option<Handle<dashi::BindTable>>; 4],
    buffers: Vec<Handle<dashi::Buffer>>,
    images: Vec<Handle<Image>>,
    samplers: Vec<Handle<dashi::Sampler>>,
    dynamic_allocator: Option<DynamicAllocator>,
    dynamic_buffers: Vec<DynamicBuffer>,
}

impl Default for MaterialBindings {
    fn default() -> Self {
        Self {
            bind_groups: Default::default(),
            bind_tables: Default::default(),
            buffers: Vec::new(),
            images: Vec::new(),
            samplers: Vec::new(),
            dynamic_allocator: None,
            dynamic_buffers: Vec::new(),
        }
    }
}

fn build_material_bindings(
    ctx: &mut gpu::Context,
    shader: &noren::meta::model::GraphicsShader,
    shader_layout: &GraphicsShaderLayout,
    material: &DeviceMaterial,
    shader_key: &str,
    material_entry: &str,
) -> Result<MaterialBindings, Box<dyn Error>> {
    match shader_key {
        "shader/multi_bind" => build_multi_bind_bindings(ctx, shader, shader_layout, material),
        "shader/bind_table" => build_bind_table_material_bindings(ctx, shader, shader_layout, material),
        _ => {
            if shader_layout
                .bind_group_layouts
                .iter()
                .flatten()
                .next()
                .is_some()
                || shader_layout
                    .bind_table_layouts
                    .iter()
                    .flatten()
                    .next()
                    .is_some()
            {
                Err(format!(
                    "Material '{material_entry}' uses bindings but the example only knows how to configure multi-bind layouts"
                )
                .into())
            } else {
                Ok(MaterialBindings::default())
            }
        }
    }
}

fn build_multi_bind_bindings(
    ctx: &mut gpu::Context,
    shader: &noren::meta::model::GraphicsShader,
    shader_layout: &GraphicsShaderLayout,
    material: &DeviceMaterial,
) -> Result<MaterialBindings, Box<dyn Error>> {
    let mut bindings = MaterialBindings::default();

    let sampler = ctx.make_sampler(&SamplerInfo::default())?;
    bindings.samplers.push(sampler);

    let textures: Vec<Handle<Image>> = material
        .textures
        .as_slice()
        .iter()
        .map(|tex| tex.image.img)
        .collect();

    let fallback = make_fallback_texture(ctx)?;
    bindings.images.push(fallback);

    if let Some(layout) = shader.bind_group_layouts.get(0).and_then(|opt| *opt) {
        let camera = CameraData {
            view_proj: Mat4::IDENTITY.to_cols_array_2d(),
            eye: [0.0, 0.0, 3.0],
            exposure: 1.0,
        };
        let buffer = ctx.make_buffer(&BufferInfo {
            debug_name: "material_camera",
            byte_size: std::mem::size_of::<CameraData>() as u32,
            visibility: MemoryVisibility::CpuAndGpu,
            usage: BufferUsage::UNIFORM,
            initial_data: Some(bytes_of(&camera)),
        })?;
        bindings.buffers.push(buffer);

        let group = BindGroupBuilder::new("material_camera_group")
            .layout(layout)
            .set(0)
            .binding(0, ShaderResource::ConstBuffer(BufferView::new(buffer)))
            .build(ctx)?;
        bindings.bind_groups[0] = Some(group);
    }

    if let Some(layout) = shader.bind_group_layouts.get(1).and_then(|opt| *opt) {
        let transforms = InstanceTransforms::default();
        let lights = Lights::default();

        let transform_buffer = ctx.make_buffer(&BufferInfo {
            debug_name: "material_instances",
            byte_size: std::mem::size_of::<InstanceTransforms>() as u32,
            visibility: MemoryVisibility::CpuAndGpu,
            usage: BufferUsage::STORAGE,
            initial_data: Some(bytes_of(&transforms)),
        })?;

        let lights_buffer = ctx.make_buffer(&BufferInfo {
            debug_name: "material_lights",
            byte_size: std::mem::size_of::<Lights>() as u32,
            visibility: MemoryVisibility::CpuAndGpu,
            usage: BufferUsage::STORAGE,
            initial_data: Some(bytes_of(&lights)),
        })?;

        bindings.buffers.push(transform_buffer);
        bindings.buffers.push(lights_buffer);

        let group = BindGroupBuilder::new("material_scene_data")
            .layout(layout)
            .set(1)
            .binding(
                0,
                ShaderResource::ConstBuffer(BufferView::new(transform_buffer)),
            )
            .binding(1, ShaderResource::ConstBuffer(BufferView::new(lights_buffer)))
            .build(ctx)?;
        bindings.bind_groups[1] = Some(group);
    }

    if let Some(layout) = shader.bind_group_layouts.get(2).and_then(|opt| *opt) {
        let mut builder = BindGroupBuilder::new("material_layers");
        builder = builder.layout(layout).set(2);

        let mut slot = 0;
        if let Some(cfg) = shader_layout.bind_group_layouts.get(2).and_then(|c| c.as_ref()) {
            let borrowed = cfg.borrow();
            let info = borrowed.info();
            for shader_info in info.shaders {
                for variable in shader_info.variables {
                    if variable.var_type == BindGroupVariableType::SampledImage {
                        let count = variable.count.max(1);
                        for _ in 0..count {
                            let tex = textures
                                .get(slot)
                                .copied()
                                .unwrap_or(fallback);
                            builder = builder.binding(
                                variable.binding,
                                ShaderResource::SampledImage(
                                    ImageView {
                                        img: tex,
                                        ..Default::default()
                                    },
                                    sampler,
                                ),
                            );
                            slot += 1;
                        }
                    }
                }
            }
        }

        bindings.bind_groups[2] = Some(builder.build(ctx)?);
    }

    if let Some(layout) = shader.bind_table_layouts.get(0).and_then(|opt| *opt) {
        let mut binding_sets: Vec<(u32, Vec<IndexedResource>)> = Vec::new();

        if let Some(cfg) = shader_layout.bind_table_layouts.get(0).and_then(|c| c.as_ref()) {
            let borrowed = cfg.borrow();
            let info = borrowed.info();
            for shader_info in info.shaders {
                for variable in shader_info.variables {
                    if variable.var_type == BindGroupVariableType::SampledImage {
                        let count = variable.count.max(1) as usize;
                        let resources: Vec<IndexedResource> = (0..count)
                            .map(|element| IndexedResource {
                                slot: element as u32,
                                resource: ShaderResource::SampledImage(
                                    ImageView {
                                        img: textures
                                            .get(element)
                                            .copied()
                                            .unwrap_or(fallback),
                                        ..Default::default()
                                    },
                                    sampler,
                                ),
                            })
                            .collect();
                        binding_sets.push((variable.binding, resources));
                    }
                }
            }
        }

        let mut builder = BindTableBuilder::new("material_bindless");
        builder = builder.layout(layout).set(0);
        for (binding, resources) in &binding_sets {
            builder = builder.binding(*binding, resources);
        }

        bindings.bind_tables[0] = Some(builder.build(ctx)?);
    }

    Ok(bindings)
}

fn build_bind_table_material_bindings(
    ctx: &mut gpu::Context,
    shader: &noren::meta::model::GraphicsShader,
    shader_layout: &GraphicsShaderLayout,
    material: &DeviceMaterial,
) -> Result<MaterialBindings, Box<dyn Error>> {
    let mut bindings = MaterialBindings::default();

    let sampler = ctx.make_sampler(&SamplerInfo::default())?;
    bindings.samplers.push(sampler);

    let fallback = make_fallback_texture(ctx)?;
    bindings.images.push(fallback);

    let textures: Vec<Handle<Image>> = material
        .textures
        .as_slice()
        .iter()
        .map(|tex| tex.image.img)
        .collect();

    if let Some(layout) = shader.bind_group_layouts.get(0).and_then(|opt| *opt) {
        let mut allocator = ctx.make_dynamic_allocator(&DynamicAllocatorInfo {
            debug_name: "material_dynamic_uniforms",
            usage: BufferUsage::UNIFORM,
            ..Default::default()
        })?;
        let mut settings = allocator.bump().ok_or("allocate dynamic settings buffer")?;
        settings.slice::<PostSettings>()[0] = PostSettings::default();

        let exposure = ExposureData {
            exposure: 2.0,
            gamma: 2.2,
            _padding: [0.0; 2],
        };

        let exposure_buffer = ctx.make_buffer(&BufferInfo {
            debug_name: "material_exposure",
            byte_size: std::mem::size_of::<ExposureData>() as u32,
            visibility: MemoryVisibility::CpuAndGpu,
            usage: BufferUsage::UNIFORM,
            initial_data: Some(bytes_of(&exposure)),
        })?;

        let group = BindGroupBuilder::new("material_post_settings")
            .layout(layout)
            .set(0)
            .binding(0, ShaderResource::Dynamic(allocator.state()))
            .binding(
                1,
                ShaderResource::ConstBuffer(BufferView::new(exposure_buffer)),
            )
            .build(ctx)?;

        bindings.dynamic_buffers.push(settings);
        bindings.dynamic_allocator = Some(allocator);
        bindings.buffers.push(exposure_buffer);
        bindings.bind_groups[0] = Some(group);
    }

    if let Some(layout) = shader.bind_table_layouts.get(3).and_then(|opt| *opt) {
        let mut binding_sets: Vec<(u32, Vec<IndexedResource>)> = Vec::new();

        let indices = [0u32, 1u32];
        let indices_buffer = ctx.make_buffer(&BufferInfo {
            debug_name: "material_indices",
            byte_size: (indices.len() * std::mem::size_of::<u32>()) as u32,
            visibility: MemoryVisibility::CpuAndGpu,
            usage: BufferUsage::STORAGE,
            initial_data: Some(cast_slice(&indices)),
        })?;
        bindings.buffers.push(indices_buffer);

        if let Some(cfg) = shader_layout.bind_table_layouts.get(3).and_then(|c| c.as_ref()) {
            let borrowed = cfg.borrow();
            let info = borrowed.info();
            for shader_info in info.shaders {
                for variable in shader_info.variables {
                    match variable.var_type {
                        BindGroupVariableType::SampledImage => {
                            let count = variable.count.max(1) as usize;
                            let resources: Vec<IndexedResource> = (0..count)
                                .map(|element| IndexedResource {
                                    slot: element as u32,
                                    resource: ShaderResource::SampledImage(
                                        ImageView {
                                            img: textures
                                                .get(element)
                                                .copied()
                                                .unwrap_or(fallback),
                                            ..Default::default()
                                        },
                                        sampler,
                                    ),
                                })
                                .collect();
                            binding_sets.push((variable.binding, resources));
                        }
                        BindGroupVariableType::Storage => {
                            let resources = vec![IndexedResource {
                                slot: 0,
                                resource: ShaderResource::ConstBuffer(BufferView::new(
                                    indices_buffer,
                                )),
                            }];
                            binding_sets.push((variable.binding, resources));
                        }
                        _ => {}
                    }
                }
            }
        }

        let indices = [0u32, 1u32];
        let indices_buffer = ctx.make_buffer(&BufferInfo {
            debug_name: "material_indices",
            byte_size: (indices.len() * std::mem::size_of::<u32>()) as u32,
            visibility: MemoryVisibility::CpuAndGpu,
            usage: BufferUsage::STORAGE,
            initial_data: Some(cast_slice(&indices)),
        })?;
        bindings.buffers.push(indices_buffer);

        let mut builder = BindTableBuilder::new("material_bind_table");
        builder = builder.layout(layout).set(3);
        for (binding, resources) in &binding_sets {
            builder = builder.binding(*binding, resources);
        }

        bindings.bind_tables[3] = Some(builder.build(ctx)?);
    }

    Ok(bindings)
}

fn make_fallback_texture(ctx: &mut gpu::Context) -> Result<Handle<Image>, Box<dyn Error>> {
    let pixels = vec![255u8; (FALLBACK_DIM * FALLBACK_DIM * 4) as usize];
    let info = ImageInfo {
        debug_name: "material_fallback_tex",
        dim: [FALLBACK_DIM, FALLBACK_DIM, 1],
        format: dashi::Format::RGBA8,
        initial_data: Some(&pixels),
        ..Default::default()
    };

    Ok(ctx.make_image(&info)?)
}

fn load_material_layout() -> Result<ModelLayoutFile, Box<dyn Error>> {
    let base_dir: PathBuf = common::sample_db_path();
    let layout_path = base_dir.join("layout.json");
    let layout: DatabaseLayoutFile = serde_json::from_str(&std::fs::read_to_string(layout_path)?)?;

    let materials_path = base_dir.join(layout.materials);
    let materials = std::fs::read_to_string(&materials_path)?;
    let mut material_layout: ModelLayoutFile = serde_json::from_str(&materials)?;

    if !material_layout.render_passes.is_empty() {
        return Ok(material_layout);
    }

    if let Ok(render_passes) = std::fs::read_to_string(base_dir.join(layout.render_passes)) {
        let render_layout: ModelLayoutFile = serde_json::from_str(&render_passes)?;
        material_layout
            .render_passes
            .extend(render_layout.render_passes);
    }

    Ok(material_layout)
}

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct CameraData {
    view_proj: [[f32; 4]; 4],
    eye: [f32; 3],
    exposure: f32,
}

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct InstanceTransforms {
    models: [[[f32; 4]; 4]; 1],
}

impl Default for InstanceTransforms {
    fn default() -> Self {
        Self {
            models: [[
                [1.0, 0.0, 0.0, 0.0],
                [0.0, 1.0, 0.0, 0.0],
                [0.0, 0.0, 1.0, 0.0],
                [0.0, 0.0, 0.0, 1.0],
            ]; 1],
        }
    }
}

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct Lights {
    positions: [[f32; 4]; 16],
    colors: [[f32; 4]; 16],
}

impl Default for Lights {
    fn default() -> Self {
        let mut lights = Self {
            positions: [[0.0; 4]; 16],
            colors: [[0.0; 4]; 16],
        };

        lights.positions[0] = [2.0, 2.0, 2.0, 1.0];
        lights.colors[0] = [1.0, 0.9, 0.8, 1.0];
        lights
    }
}

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable, Default)]
struct PostSettings {
    exposure: f32,
    gamma: f32,
    _padding: [f32; 2],
}

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct ExposureData {
    exposure: f32,
    gamma: f32,
    _padding: [f32; 2],
}

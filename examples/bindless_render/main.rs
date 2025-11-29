//! Run with `cargo run --example bindless_render` to build a bindless texture
//! table from assets in the sample database.

#[path = "../common/mod.rs"]
mod common;

use bytemuck::{Pod, Zeroable, bytes_of};
use common::{
    SAMPLE_GEOMETRY_ENTRY, SAMPLE_TEXTURE_ENTRY, display::blit_image_to_display, init_context,
    open_sample_db,
};
use dashi::builders::{BindGroupBuilder, BindTableBuilder};
use dashi::driver::command::{BeginDrawing, DrawIndexed};
use dashi::gpu::{self, CommandStream};
use dashi::{
    BindGroupLayout, BindTableLayout, BufferInfo, BufferUsage, BufferView, ClearValue,
    CommandQueueInfo2, DynamicAllocatorInfo, Handle, Image, ImageInfo, ImageView, IndexedResource,
    MemoryVisibility, Rect2D, SamplerInfo, ShaderResource, SubmitInfo, Viewport,
};
use noren::render_graph::RenderGraphRequest;
use std::error::Error;

fn main() {
    if let Err(err) = run() {
        eprintln!("error: {err}");
        std::process::exit(1);
    }
}

fn run() -> Result<(), Box<dyn Error>> {
    let mut ctx = match init_context() {
        Ok(ctx) => ctx,
        Err(err) => {
            eprintln!("Skipping example – unable to create GPU context: {err}");
            return Ok(());
        }
    };

    let mut db = open_sample_db(&mut ctx)?;

    let render_graph = db.create_render_graph(RenderGraphRequest {
        shaders: vec!["shader/bind_table".to_string()],
    })?;
    let pipeline_binding = render_graph
        .pipelines
        .get("shader/bind_table")
        .ok_or_else(|| "Missing 'shader/bind_table' pipeline")?;

    let (bind_group_set, bind_group_layout) = first_layout(pipeline_binding.bind_group_layouts)
        .ok_or_else(|| "Bind group layout for shader/bind_table is missing")?;
    let (bind_table_set, bind_table_layout) = first_layout(pipeline_binding.bind_table_layouts)
        .ok_or_else(|| "Bind table layout for shader/bind_table is missing")?;

    let textures = {
        let imagery = db.imagery_mut();
        load_bindless_textures(&mut ctx, imagery)?
    };
    let indices_buffer = make_indices_buffer(&mut ctx)?;
    let sampler = ctx.make_sampler(&SamplerInfo::default())?;
    let exposure_buffer = make_exposure_buffer(&mut ctx)?;

    let mut allocator = ctx.make_dynamic_allocator(&DynamicAllocatorInfo {
        debug_name: "bind_table_settings_allocator",
        usage: BufferUsage::UNIFORM,
        ..Default::default()
    })?;
    let settings_buffer = write_settings(&mut allocator);

    let bind_group = make_bind_group(
        &mut ctx,
        bind_group_layout,
        bind_group_set as u32,
        allocator.state(),
        exposure_buffer,
    )?;

    let bind_table = make_bind_table(
        &mut ctx,
        bind_table_layout,
        bind_table_set as u32,
        sampler,
        &textures,
        indices_buffer,
    )?;

    let (host_geometry, device_geometry) = {
        let geometry = db.geometry_mut();
        (
            geometry.fetch_raw_geometry(SAMPLE_GEOMETRY_ENTRY)?,
            geometry.fetch_gpu_geometry(SAMPLE_GEOMETRY_ENTRY)?,
        )
    };

    let framebuffer = ctx.make_image(&ImageInfo {
        debug_name: "bindless_render_fb",
        dim: [800, 600, 1],
        format: dashi::Format::RGBA8,
        ..Default::default()
    })?;

    render_bindless(
        &mut ctx,
        device_geometry,
        &host_geometry,
        pipeline_binding.pipeline,
        framebuffer,
        bind_group_set,
        bind_group,
        settings_buffer,
        bind_table_set,
        bind_table,
    )?;

    println!("Displaying bindless render (800x600)");
    blit_image_to_display(&mut ctx, framebuffer, [800, 600], "bindless_render")?;
    Ok(())
}

fn load_bindless_textures(
    ctx: &mut gpu::Context,
    imagery: &mut noren::rdb::ImageDB,
) -> Result<[Handle<Image>; 2], Box<dyn Error>> {
    let albedo = imagery.fetch_raw_image(SAMPLE_TEXTURE_ENTRY)?;
    let bloom = imagery.fetch_raw_image("imagery/peppers")?;

    let mut albedo_info = albedo.info().dashi();
    albedo_info.debug_name = "bindless_albedo";
    albedo_info.initial_data = Some(albedo.data());

    let mut bloom_info = bloom.info().dashi();
    bloom_info.debug_name = "bindless_bloom";
    bloom_info.initial_data = Some(bloom.data());

    let albedo_tex = ctx.make_image(&albedo_info)?;
    let bloom_tex = ctx.make_image(&bloom_info)?;

    Ok([albedo_tex, bloom_tex])
}

fn make_indices_buffer(ctx: &mut gpu::Context) -> Result<Handle<dashi::Buffer>, Box<dyn Error>> {
    let indices = BindlessIndices {
        albedo_index: 0,
        bloom_index: 1,
    };

    let buffer = ctx.make_buffer(&BufferInfo {
        debug_name: "bindless_indices",
        byte_size: std::mem::size_of::<BindlessIndices>() as u32,
        visibility: MemoryVisibility::CpuAndGpu,
        usage: BufferUsage::STORAGE,
        initial_data: Some(bytes_of(&indices)),
    })?;

    Ok(buffer)
}

fn make_exposure_buffer(ctx: &mut gpu::Context) -> Result<Handle<dashi::Buffer>, Box<dyn Error>> {
    let exposure = ExposureData {
        exposure: 2.4,
        gamma: 2.2,
        _padding: [0.0; 2],
    };

    let buffer = ctx.make_buffer(&BufferInfo {
        debug_name: "bindless_exposure",
        byte_size: std::mem::size_of::<ExposureData>() as u32,
        visibility: MemoryVisibility::CpuAndGpu,
        usage: BufferUsage::UNIFORM,
        initial_data: Some(bytes_of(&exposure)),
    })?;

    Ok(buffer)
}

fn write_settings(allocator: &mut dashi::DynamicAllocator) -> dashi::DynamicBuffer {
    let mut settings = allocator
        .bump()
        .expect("allocate dynamic uniform for settings");
    settings.slice::<Settings>()[0] = Settings::default();
    settings
}

fn make_bind_group(
    ctx: &mut gpu::Context,
    layout: Handle<BindGroupLayout>,
    set: u32,
    allocator_state: dashi::DynamicAllocatorState,
    exposure_buffer: Handle<dashi::Buffer>,
) -> Result<Handle<dashi::BindGroup>, Box<dyn Error>> {
    let group = BindGroupBuilder::new("bind_table_group")
        .layout(layout)
        .set(set)
        .binding(0, ShaderResource::Dynamic(allocator_state))
        .binding(
            1,
            ShaderResource::ConstBuffer(BufferView::new(exposure_buffer)),
        )
        .build(ctx)?;

    Ok(group)
}

fn make_bind_table(
    ctx: &mut gpu::Context,
    layout: Handle<BindTableLayout>,
    set: u32,
    sampler: Handle<dashi::Sampler>,
    textures: &[Handle<Image>; 2],
    indices: Handle<dashi::Buffer>,
) -> Result<Handle<dashi::BindTable>, Box<dyn Error>> {
    let bindings = [
        IndexedResource {
            slot: 0,
            resource: ShaderResource::SampledImage(
                ImageView {
                    img: textures[0],
                    ..Default::default()
                },
                sampler,
            ),
        },
        IndexedResource {
            slot: 1,
            resource: ShaderResource::SampledImage(
                ImageView {
                    img: textures[1],
                    ..Default::default()
                },
                sampler,
            ),
        },
    ];

    let table = BindTableBuilder::new("bindless_table")
        .layout(layout)
        .set(set)
        .binding(0, &bindings)
        .binding(
            1,
            &[IndexedResource {
                slot: 0,
                resource: ShaderResource::StorageBuffer(indices),
            }],
        )
        .build(ctx)?;

    Ok(table)
}

fn render_bindless(
    ctx: &mut gpu::Context,
    device_geometry: noren::rdb::geometry::DeviceGeometry,
    host_geometry: &noren::rdb::geometry::HostGeometry,
    pipeline: Handle<dashi::GraphicsPipeline>,
    framebuffer: Handle<Image>,
    bind_group_set: usize,
    bind_group: Handle<dashi::BindGroup>,
    settings_buffer: dashi::DynamicBuffer,
    bind_table_set: usize,
    bind_table: Handle<dashi::BindTable>,
) -> Result<(), dashi::GPUError> {
    let mut ring = ctx.make_command_ring(&CommandQueueInfo2 {
        debug_name: "bindless_render_ring",
        ..Default::default()
    })?;

    ring.record(|list| {
        let mut stream = CommandStream::new().begin();
        let viewport = Viewport {
            area: dashi::FRect2D {
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
                Some(ClearValue::Color([0.02, 0.02, 0.04, 1.0])),
                None,
                None,
                None,
            ],
        });

        let mut bind_groups = [None; 4];
        let mut dynamic_buffers = [None; 4];
        let mut bind_tables = [None; 4];

        bind_groups[bind_group_set] = Some(bind_group);
        dynamic_buffers[bind_group_set] = Some(settings_buffer);
        bind_tables[bind_table_set] = Some(bind_table);

        draw.draw_indexed(&DrawIndexed {
            vertices: device_geometry.vertices,
            indices: device_geometry.indices,
            index_count: host_geometry
                .indices
                .as_ref()
                .map(|idx| idx.len() as u32)
                .unwrap_or_default(),
            bind_groups,
            bind_tables,
            dynamic_buffers,
            ..Default::default()
        });

        stream = draw.stop_drawing();
        stream.end().append(list);
    })?;

    ring.submit(&SubmitInfo::default())?;
    ring.wait_all()?;

    Ok(())
}

fn first_layout<T: Copy>(layouts: [Option<T>; 4]) -> Option<(usize, T)> {
    layouts
        .iter()
        .copied()
        .enumerate()
        .find_map(|(idx, layout)| layout.map(|layout| (idx, layout)))
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Pod, Zeroable)]
struct BindlessIndices {
    albedo_index: u32,
    bloom_index: u32,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Pod, Zeroable)]
struct ExposureData {
    exposure: f32,
    gamma: f32,
    _padding: [f32; 2],
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Pod, Zeroable)]
struct Settings {
    jittered_proj: [[f32; 4]; 4],
    film_grain_seed: [f32; 2],
    vignette_strength: f32,
    _padding: f32,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            jittered_proj: glam::Mat4::IDENTITY.to_cols_array_2d(),
            film_grain_seed: [0.37, 0.13],
            vignette_strength: 1.5,
            _padding: 0.0,
        }
    }
}

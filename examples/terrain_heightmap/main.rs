//! Renders the full sample terrain database as geometry.
//!
//! This example stitches together all terrain chunks in the sample database
//! and renders the resulting mesh using a simple dashi pipeline.

use std::error::Error;

use bytemuck::{Pod, Zeroable};
use dashi::driver::command::{BeginDrawing, BlitImage, DrawIndexed};
use dashi::*;
use inline_spirv::inline_spirv;
use noren::{
    RDBView, RdbErr,
    rdb::terrain::{TerrainChunk, TerrainProjectSettings, parse_chunk_coord_key},
};
use winit::event::{ElementState, Event, KeyboardInput, VirtualKeyCode, WindowEvent};
use winit::event_loop::ControlFlow;
use winit::platform::run_return::EventLoopExtRunReturn;

#[path = "../common/mod.rs"]
mod common;

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct TerrainVertex {
    position: [f32; 3],
    color: [f32; 3],
}

#[derive(Debug)]
struct RdbViewError(String);

impl std::fmt::Display for RdbViewError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl Error for RdbViewError {}

fn main() {
    if let Err(err) = run() {
        eprintln!("error: {err}");
        std::process::exit(1);
    }
}

fn run() -> Result<(), Box<dyn Error>> {
    let mut ctx = common::init_context()?;

    let terrain_path = common::sample_db_path().join("terrain.rdb");
    let mut view = load_rdb_view(&terrain_path)?;
    let entries = view.entries();

    let (settings_entry, project_key) = find_project_settings(&entries)?
        .ok_or("missing terrain project settings entry")?;
    let settings: TerrainProjectSettings = view
        .fetch(&settings_entry)
        .map_err(|err| rdb_view_error(err))?;

    let mut chunks = Vec::new();
    let mut min_height = f32::INFINITY;
    let mut max_height = f32::NEG_INFINITY;

    for entry in &entries {
        let Some(coord_key) = entry.name.strip_prefix("terrain/chunk_") else {
            continue;
        };
        if parse_chunk_coord_key(coord_key).is_none() {
            continue;
        }

        let chunk: TerrainChunk = view.fetch(&entry.name).map_err(|err| rdb_view_error(err))?;
        for height in &chunk.heights {
            min_height = min_height.min(*height);
            max_height = max_height.max(*height);
        }
        chunks.push(chunk);
    }

    if chunks.is_empty() {
        return Err("no terrain chunks found in sample database".into());
    }

    let (vertices, indices) = build_terrain_mesh(&chunks, &settings, min_height, max_height);

    let vertex_bytes = bytemuck::cast_slice(&vertices);
    let index_bytes = bytemuck::cast_slice(&indices);

    let vertex_buffer = ctx.make_buffer(&BufferInfo {
        debug_name: "terrain_vertices",
        byte_size: vertex_bytes.len() as u32,
        visibility: MemoryVisibility::Gpu,
        usage: BufferUsage::VERTEX,
        initial_data: Some(vertex_bytes),
    })?;
    let index_buffer = ctx.make_buffer(&BufferInfo {
        debug_name: "terrain_indices",
        byte_size: index_bytes.len() as u32,
        visibility: MemoryVisibility::Gpu,
        usage: BufferUsage::INDEX,
        initial_data: Some(index_bytes),
    })?;

    const WIDTH: u32 = 1280;
    const HEIGHT: u32 = 720;

    let fb = ctx.make_image(&ImageInfo {
        debug_name: "terrain_framebuffer",
        dim: [WIDTH, HEIGHT, 1],
        format: Format::RGBA8,
        mip_levels: 1,
        initial_data: None,
        ..Default::default()
    })?;
    let fb_view = ImageView { img: fb, ..Default::default() };

    let pipeline_layout = ctx.make_graphics_pipeline_layout(&GraphicsPipelineLayoutInfo {
        vertex_info: VertexDescriptionInfo {
            entries: &[
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
            ],
            stride: std::mem::size_of::<TerrainVertex>(),
            rate: VertexRate::Vertex,
        },
        bt_layouts: [None, None, None, None],
        shaders: &[
            PipelineShaderInfo {
                stage: ShaderType::Vertex,
                spirv: inline_spirv!(
                    r#"
#version 450
layout(location = 0) in vec3 in_pos;
layout(location = 1) in vec3 in_color;
layout(location = 0) out vec3 v_color;
void main() {
    v_color = in_color;
    gl_Position = vec4(in_pos, 1.0);
}
"#,
                    vert
                ),
                specialization: &[],
            },
            PipelineShaderInfo {
                stage: ShaderType::Fragment,
                spirv: inline_spirv!(
                    r#"
#version 450
layout(location = 0) in vec3 v_color;
layout(location = 0) out vec4 out_color;
void main() { out_color = vec4(v_color, 1.0); }
"#,
                    frag
                ),
                specialization: &[],
            },
        ],
        details: Default::default(),
        debug_name: "terrain_pipeline_layout",
    })?;

    let render_pass = ctx.make_render_pass(&RenderPassInfo {
        viewport: Viewport {
            area: FRect2D {
                w: WIDTH as f32,
                h: HEIGHT as f32,
                ..Default::default()
            },
            scissor: Rect2D {
                w: WIDTH,
                h: HEIGHT,
                ..Default::default()
            },
            ..Default::default()
        },
        subpasses: &[SubpassDescription {
            color_attachments: &[AttachmentDescription::default()],
            depth_stencil_attachment: None,
            subpass_dependencies: &[],
        }],
        debug_name: "terrain_render_pass",
    })?;

    let subpass_info = ctx
        .render_pass_subpass_info(render_pass, 0)
        .ok_or("missing render pass subpass info")?;
    let graphics_pipeline = ctx.make_graphics_pipeline(&GraphicsPipelineInfo {
        layout: pipeline_layout,
        attachment_formats: subpass_info.color_formats,
        depth_format: subpass_info.depth_format,
        subpass_samples: subpass_info.samples,
        debug_name: "terrain_pipeline",
        ..Default::default()
    })?;

    let mut display = ctx.make_display(&DisplayInfo {
        window: WindowInfo {
            title: format!("Sample Terrain ({project_key})"),
            size: [WIDTH, HEIGHT],
            resizable: false,
        },
        vsync: true,
        ..Default::default()
    })?;
    let mut ring = ctx.make_command_ring(&CommandQueueInfo2 {
        debug_name: "terrain_cmd",
        ..Default::default()
    })?;
    let render_sems = ctx.make_semaphores(3)?;

    loop {
        let mut should_exit = false;
        {
            let event_loop = display.winit_event_loop();
            event_loop.run_return(|event, _target, control_flow| {
                *control_flow = ControlFlow::Exit;
                if let Event::WindowEvent { event, .. } = event {
                    match event {
                        WindowEvent::CloseRequested => should_exit = true,
                        WindowEvent::KeyboardInput {
                            input:
                                KeyboardInput {
                                    virtual_keycode: Some(VirtualKeyCode::Escape),
                                    state: ElementState::Pressed,
                                    ..
                                },
                            ..
                        } => should_exit = true,
                        _ => {}
                    }
                }
            });
        }

        if should_exit {
            break;
        }

        let (img, sem, _idx, _good) = ctx.acquire_new_image(&mut display)?;
        let frame_slot = ring.current_index();

        ring.record(|list| {
            let stream = CommandStream::new().begin();
            let draw = stream.begin_drawing(&BeginDrawing {
                viewport: Viewport {
                    area: FRect2D {
                        w: WIDTH as f32,
                        h: HEIGHT as f32,
                        ..Default::default()
                    },
                    scissor: Rect2D {
                        w: WIDTH,
                        h: HEIGHT,
                        ..Default::default()
                    },
                    ..Default::default()
                },
                render_pass,
                pipeline: graphics_pipeline,
                color_attachments: [Some(fb_view), None, None, None, None, None, None, None],
                depth_attachment: None,
                clear_values: [
                    Some(ClearValue::Color([0.07, 0.1, 0.12, 1.0])),
                    None,
                    None,
                    None,
                    None,
                    None,
                    None,
                    None,
                ],
                ..Default::default()
            });

            let draw = draw.draw_indexed(&DrawIndexed {
                vertices: vertex_buffer,
                indices: index_buffer,
                bind_tables: Default::default(),
                dynamic_buffers: Default::default(),
                instance_count: 1,
                first_instance: 0,
                index_count: indices.len() as u32,
            });

            let stream = draw
                .stop_drawing()
                .blit_images(&BlitImage {
                    src: fb,
                    dst: img.img,
                    filter: Filter::Linear,
                    ..Default::default()
                })
                .prepare_for_presentation(img.img);

            stream.end().append(list).unwrap();
        })?;

        ring.submit(&SubmitInfo {
            wait_sems: &[sem],
            signal_sems: &[render_sems[frame_slot]],
            ..Default::default()
        })?;

        ctx.present_display(&display, &[render_sems[frame_slot]])?;
    }

    Ok(())
}

fn load_rdb_view(path: &std::path::Path) -> Result<RDBView, Box<dyn Error>> {
    RDBView::load(path).map_err(rdb_view_error)
}

fn rdb_view_error(err: RdbErr) -> Box<dyn Error> {
    Box::new(RdbViewError(err.to_string()))
}

fn build_terrain_mesh(
    chunks: &[TerrainChunk],
    settings: &TerrainProjectSettings,
    min_height: f32,
    max_height: f32,
) -> (Vec<TerrainVertex>, Vec<u32>) {
    let world_min = settings.world_bounds_min;
    let world_max = settings.world_bounds_max;
    let world_size_x = (world_max[0] - world_min[0]).max(1.0);
    let world_size_y = (world_max[1] - world_min[1]).max(1.0);
    let height_range = (max_height - min_height).max(1.0);

    let mut vertices = Vec::new();
    let mut indices = Vec::new();

    for chunk in chunks {
        let grid_width = chunk.tiles_per_chunk[0];
        let grid_height = chunk.tiles_per_chunk[1];
        let base_index = vertices.len() as u32;

        for sample_y in 0..=grid_height {
            for sample_x in 0..=grid_width {
                let height = chunk.height_sample(sample_x, sample_y).unwrap_or(0.0);
                let world_x = chunk.origin[0] + sample_x as f32 * chunk.tile_size;
                let world_y = chunk.origin[1] + sample_y as f32 * chunk.tile_size;
                let norm_x = ((world_x - world_min[0]) / world_size_x) * 2.0 - 1.0;
                let norm_y = ((world_y - world_min[1]) / world_size_y) * 2.0 - 1.0;
                let norm_z = ((height - min_height) / height_range) * 2.0 - 1.0;
                let t = ((height - min_height) / height_range).clamp(0.0, 1.0);
                let color = [
                    0.1 + 0.4 * t,
                    0.3 + 0.5 * t,
                    0.2 + 0.2 * t,
                ];

                vertices.push(TerrainVertex {
                    position: [norm_x, norm_y, norm_z],
                    color,
                });
            }
        }

        let row_stride = grid_width + 1;
        for y in 0..grid_height {
            for x in 0..grid_width {
                let i0 = base_index + y * row_stride + x;
                let i1 = i0 + 1;
                let i2 = i0 + row_stride;
                let i3 = i2 + 1;
                indices.extend_from_slice(&[i0, i2, i1, i1, i2, i3]);
            }
        }
    }

    (vertices, indices)
}

fn find_project_settings(
    entries: &[noren::RDBEntryMeta],
) -> Result<Option<(String, String)>, Box<dyn Error>> {
    for entry in entries {
        if let Some(rest) = entry.name.strip_prefix("terrain/project/") {
            if let Some(project_key) = rest.strip_suffix("/settings") {
                return Ok(Some((entry.name.clone(), project_key.to_string())));
            }
        }
    }

    Ok(None)
}

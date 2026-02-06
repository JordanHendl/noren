//! Renders the full sample terrain database as geometry.
//!
//! This example stitches together all terrain chunks in the sample database
//! and renders the resulting mesh using a simple dashi pipeline.

use std::error::Error;

use bytemuck::{Pod, Zeroable};
use dashi::builders::{BindTableBuilder, BindTableLayoutBuilder};
use dashi::driver::command::{BeginDrawing, BlitImage, DrawIndexed};
use dashi::*;
use inline_spirv::inline_spirv;
use noren::{
    rdb::terrain::{TerrainChunkArtifact, TerrainProjectSettings, parse_chunk_artifact_entry},
    NorenError,
};
use winit::event::{
    ElementState,
    Event,
    KeyboardInput,
    MouseScrollDelta,
    VirtualKeyCode,
    WindowEvent,
};
use winit::event_loop::ControlFlow;
use winit::platform::run_return::EventLoopExtRunReturn;

#[path = "../common/mod.rs"]
mod common;

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct TerrainVertex {
    position: [f32; 3],
    normal: [f32; 3],
    color: [f32; 3],
}

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct CameraUniform {
    view: [[f32; 4]; 4],
}

fn main() {
    if let Err(err) = run() {
        eprintln!("error: {err}");
        std::process::exit(1);
    }
}

fn run() -> Result<(), Box<dyn Error>> {
    let mut ctx = common::init_context()?;

    let mut db = common::open_sample_db(&mut ctx)?;
    let entries = db.terrain().enumerate_entries();
    let project_key = "iceland";
    let settings_entry = project_settings_entry(project_key);
    if !entries.iter().any(|entry| entry == &settings_entry) {
        return Err(format!(
            "missing terrain project settings entry: {settings_entry}"
        )
        .into());
    }
    let settings: TerrainProjectSettings = db
        .terrain_mut()
        .fetch_project_settings(&settings_entry)
        .map_err(|err| terrain_db_error(err))?;

    let mut artifacts = Vec::new();

    for entry in &entries {
        let Some(key) = parse_chunk_artifact_entry(entry) else {
            continue;
        };
        if key.project_key != project_key || key.lod != 0 {
            continue;
        }

        let artifact: TerrainChunkArtifact = db
            .terrain_mut()
            .fetch_chunk_artifact(entry)
            .map_err(|err| terrain_db_error(err))?;
        artifacts.push(artifact);
    }

    if artifacts.is_empty() {
        return Err("no terrain chunk artifacts found in sample database".into());
    }

    let (vertices, indices) = build_terrain_mesh(&artifacts, &settings);
    let vertex_bytes = bytemuck::cast_slice(&vertices);
    let index_bytes = bytemuck::cast_slice(&indices);

    let vertex_buffer = ctx.make_buffer(&BufferInfo {
        debug_name: "terrain_vertices",
        byte_size: vertex_bytes.len() as u32,
        visibility: MemoryVisibility::CpuAndGpu,
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

    let camera_layout = BindTableLayoutBuilder::new("terrain_camera_layout")
        .shader(ShaderInfo {
            shader_type: ShaderType::Vertex,
            variables: &[BindTableVariable {
                var_type: BindTableVariableType::Uniform,
                binding: 0,
                count: 1,
            }],
        })
        .build(&mut ctx)?;

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
                VertexEntryInfo {
                    format: ShaderPrimitiveType::Vec3,
                    location: 2,
                    offset: 24,
                },
            ],
            stride: std::mem::size_of::<TerrainVertex>(),
            rate: VertexRate::Vertex,
        },
        bt_layouts: [Some(camera_layout), None, None, None],
        shaders: &[
            PipelineShaderInfo {
                stage: ShaderType::Vertex,
                spirv: inline_spirv!(
                    r#"
#version 450
layout(location = 0) in vec3 in_pos;
layout(location = 1) in vec3 in_normal;
layout(location = 2) in vec3 in_color;
layout(set = 0, binding = 0) uniform Camera {
    mat4 view;
} camera;
layout(location = 0) out vec3 v_normal;
layout(location = 1) out vec3 v_color;
void main() {
    v_normal = in_normal;
    v_color = in_color;
    gl_Position = camera.view * vec4(in_pos, 1.0);
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
layout(location = 0) in vec3 v_normal;
layout(location = 1) in vec3 v_color;
layout(location = 0) out vec4 out_color;
void main() {
    vec3 normal = normalize(v_normal);
    vec3 light = normalize(vec3(0.4, 0.6, 1.0));
    float shade = max(dot(normal, light), 0.1);
    out_color = vec4(v_color * shade, 1.0);
}
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

    let mut zoom = 1.0f32;
    let mut zoom_offset = [0.0f32, 0.0f32];
    let mut cursor_pos = [WIDTH as f32 * 0.5, HEIGHT as f32 * 0.5];

    let camera_uniform = CameraUniform {
        view: make_view_matrix(zoom, zoom_offset),
    };
    let camera_buffer = ctx.make_buffer(&BufferInfo {
        debug_name: "terrain_camera_uniform",
        byte_size: std::mem::size_of::<CameraUniform>() as u32,
        visibility: MemoryVisibility::CpuAndGpu,
        usage: BufferUsage::UNIFORM,
        initial_data: Some(bytemuck::bytes_of(&camera_uniform)),
    })?;
    let camera_binding = [IndexedResource {
        resource: ShaderResource::Buffer(BufferView {
            handle: camera_buffer,
            offset: 0,
            size: std::mem::size_of::<CameraUniform>() as u64,
        }),
        slot: 0,
    }];
    let camera_table = BindTableBuilder::new("terrain_camera_table")
        .layout(camera_layout)
        .binding(0, &camera_binding)
        .build(&mut ctx)?;

    loop {
        let mut should_exit = false;
        let mut zoom_delta = 0.0f32;
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
                        WindowEvent::CursorMoved { position, .. } => {
                            cursor_pos = [position.x as f32, position.y as f32];
                        }
                        WindowEvent::MouseWheel { delta, .. } => {
                            zoom_delta = match delta {
                                MouseScrollDelta::LineDelta(_, y) => zoom_delta + y,
                                MouseScrollDelta::PixelDelta(pos) => {
                                    zoom_delta + (pos.y as f32 / 100.0)
                                }
                            };
                        }
                        _ => {}
                    }
                }
            });
        }

        if should_exit {
            break;
        }

        if zoom_delta.abs() > f32::EPSILON {
            let old_zoom = zoom;
            let zoom_step = (1.0 + zoom_delta * 0.1).clamp(0.1, 4.0);
            let new_zoom = (zoom * zoom_step).clamp(0.2, 6.0);
            let cursor_ndc = [
                (cursor_pos[0] / WIDTH as f32) * 2.0 - 1.0,
                1.0 - (cursor_pos[1] / HEIGHT as f32) * 2.0,
            ];
            let world = [
                cursor_ndc[0] / old_zoom - zoom_offset[0],
                cursor_ndc[1] / old_zoom - zoom_offset[1],
            ];
            zoom_offset = [
                cursor_ndc[0] / new_zoom - world[0],
                cursor_ndc[1] / new_zoom - world[1],
            ];
            zoom = new_zoom;
            let camera_uniform = CameraUniform {
                view: make_view_matrix(zoom, zoom_offset),
            };
            write_camera_buffer(&mut ctx, camera_buffer, &camera_uniform)?;
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
                bind_tables: [Some(camera_table), None, None, None],
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

fn terrain_db_error(err: NorenError) -> Box<dyn Error> {
    Box::new(err)
}

fn build_terrain_mesh(
    artifacts: &[TerrainChunkArtifact],
    settings: &TerrainProjectSettings,
) -> (Vec<TerrainVertex>, Vec<u32>) {
    let world_min = settings.world_bounds_min;
    let world_max = settings.world_bounds_max;
    let world_size_x = (world_max[0] - world_min[0]).max(1.0);
    let world_size_y = (world_max[1] - world_min[1]).max(1.0);
    let mut min_height = f32::INFINITY;
    let mut max_height = f32::NEG_INFINITY;
    for artifact in artifacts {
        for vertex in &artifact.vertices {
            min_height = min_height.min(vertex.position[2]);
            max_height = max_height.max(vertex.position[2]);
        }
    }
    let height_range = (max_height - min_height).max(1.0);
    let norm_scale_x = 2.0 / world_size_x;
    let norm_scale_y = 2.0 / world_size_y;
    let norm_scale_z = 2.0 / height_range;
    let inv_scale_x = 1.0 / norm_scale_x.max(0.001);
    let inv_scale_y = 1.0 / norm_scale_y.max(0.001);
    let inv_scale_z = 1.0 / norm_scale_z.max(0.001);

    let mut vertices = Vec::new();
    let mut indices = Vec::new();

    for artifact in artifacts {
        let base_index = vertices.len() as u32;
        for vertex in &artifact.vertices {
            let world_x = vertex.position[0];
            let world_y = vertex.position[1];
            let height = vertex.position[2];
            let norm_x = ((world_x - world_min[0]) / world_size_x) * 2.0 - 1.0;
            let norm_y = ((world_y - world_min[1]) / world_size_y) * 2.0 - 1.0;
            let norm_z = ((height - min_height) / height_range) * 2.0 - 1.0;
            let normal = normalize_vec3([
                vertex.normal[0] * inv_scale_x,
                vertex.normal[1] * inv_scale_y,
                vertex.normal[2] * inv_scale_z,
            ]);
            let t = ((height - min_height) / height_range).clamp(0.0, 1.0);
            let color = [
                0.1 + 0.4 * t,
                0.3 + 0.5 * t,
                0.2 + 0.2 * t,
            ];

            vertices.push(TerrainVertex {
                position: [norm_x, norm_y, norm_z],
                normal,
                color,
            });
        }

        indices.extend(
            artifact
                .indices
                .iter()
                .map(|index| base_index + *index),
        );
    }

    (vertices, indices)
}

fn normalize_vec3(value: [f32; 3]) -> [f32; 3] {
    let mut x = value[0];
    let mut y = value[1];
    let mut z = value[2];
    let length = (x * x + y * y + z * z).sqrt().max(0.001);
    x /= length;
    y /= length;
    z /= length;
    [x, y, z]
}

fn make_view_matrix(zoom: f32, offset: [f32; 2]) -> [[f32; 4]; 4] {
    [
        [zoom, 0.0, 0.0, 0.0],
        [0.0, zoom, 0.0, 0.0],
        [0.0, 0.0, 1.0, 0.0],
        [zoom * offset[0], zoom * offset[1], 0.0, 1.0],
    ]
}

fn write_camera_buffer(
    ctx: &mut Context,
    buffer: Handle<Buffer>,
    uniform: &CameraUniform,
) -> Result<(), Box<dyn Error>> {
    let bytes = bytemuck::bytes_of(uniform);
    let mut view = BufferView::new(buffer);
    view.offset = 0;
    view.size = bytes.len() as u64;

    let mapped = ctx
        .map_buffer_mut::<u8>(view)
        .map_err(|err| Box::new(err) as Box<dyn Error>)?;

    if mapped.len() < bytes.len() {
        return Err("mapped buffer too small for camera upload".into());
    }

    mapped[..bytes.len()].copy_from_slice(bytes);
    ctx.flush_buffer(BufferView::new(buffer))
        .and_then(|_| ctx.unmap_buffer(buffer))
        .map_err(|err| Box::new(err) as Box<dyn Error>)?;
    Ok(())
}

fn project_settings_entry(project_key: &str) -> String {
    format!("terrain/project/{project_key}/settings")
}

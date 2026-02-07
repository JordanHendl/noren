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
    NorenError,
    rdb::terrain::{
        TerrainCameraInfo, TerrainChunkArtifact, TerrainFrustum, TerrainProjectSettings,
        chunk_artifact_entry, chunk_coord_key, lod_key,
    },
};
use winit::event::{
    ElementState, Event, KeyboardInput, MouseScrollDelta, VirtualKeyCode, WindowEvent,
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
    projection: [[f32; 4]; 4],
}

#[derive(Clone, Copy)]
struct TerrainChunkDraw {
    vertex_buffer: Handle<Buffer>,
    index_buffer: Handle<Buffer>,
    index_count: u32,
}

struct TerrainNormalization {
    world_min: [f32; 3],
    world_size_x: f32,
    world_size_z: f32,
    min_height: f32,
    height_range: f32,
    inv_scale_x: f32,
    inv_scale_y: f32,
    inv_scale_z: f32,
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
    let project_key = "iceland";
    let settings_entry = project_settings_entry(project_key);
    let settings: TerrainProjectSettings = db
        .terrain_mut()
        .fetch_project_settings(&settings_entry)
        .map_err(|err| terrain_db_error(err))?;
    let normalization = build_terrain_normalization(&settings);

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
    let fb_view = ImageView {
        img: fb,
        ..Default::default()
    };

    let depth_image = ctx.make_image(&ImageInfo {
        debug_name: "terrain_depth_buffer",
        dim: [WIDTH, HEIGHT, 1],
        format: Format::D24S8,
        mip_levels: 1,
        initial_data: None,
        ..Default::default()
    })?;
    let depth_view = ImageView {
        img: depth_image,
        aspect: AspectMask::DepthStencil,
        ..Default::default()
    };

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
    mat4 projection;
} camera;
layout(location = 0) out vec3 v_normal;
layout(location = 1) out vec3 v_color;
void main() {
    v_normal = in_normal;
    v_color = in_color;
    gl_Position = camera.projection * camera.view * vec4(in_pos, 1.0);
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
        details: GraphicsPipelineDetails {
            depth_test: Some(DepthInfo {
                should_test: true,
                should_write: true,
            }),
            ..Default::default()
        },
        debug_name: "terrain_pipeline_layout",
    })?;

    let depth_attachment = AttachmentDescription {
        format: Format::D24S8,
        ..Default::default()
    };
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
            depth_stencil_attachment: Some(&depth_attachment),
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
    let mut yaw = 0.0f32;
    let mut pitch = 0.0f32;
    let mut cursor_pos = [WIDTH as f32 * 0.5, HEIGHT as f32 * 0.5];
    let projection = make_projection_matrix(-10000.0, 10000.0);

    let camera_uniform = CameraUniform {
        view: make_view_matrix(zoom, zoom_offset, yaw, pitch),
        projection,
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

    let mut chunk_draws = Vec::new();
    let mut chunk_cache = std::collections::HashMap::new();
    let mut camera_info = build_camera_info(zoom, zoom_offset, yaw, &settings, &normalization);
    let mut visible_set: std::collections::HashSet<String> = std::collections::HashSet::new();
    let mut needs_refresh = true;

    loop {
        let mut should_exit = false;
        let mut zoom_delta = 0.0f32;
        let mut rotate_delta = 0.0f32;
        let mut pitch_delta = 0.0f32;
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
                        WindowEvent::KeyboardInput {
                            input:
                                KeyboardInput {
                                    virtual_keycode: Some(keycode),
                                    state: ElementState::Pressed,
                                    ..
                                },
                            ..
                        } => {
                            if matches!(keycode, VirtualKeyCode::Q | VirtualKeyCode::A) {
                                rotate_delta -= 1.0;
                            }
                            if matches!(keycode, VirtualKeyCode::E | VirtualKeyCode::D) {
                                rotate_delta += 1.0;
                            }
                            if matches!(keycode, VirtualKeyCode::W | VirtualKeyCode::Up) {
                                pitch_delta += 1.0;
                            }
                            if matches!(keycode, VirtualKeyCode::S | VirtualKeyCode::Down) {
                                pitch_delta -= 1.0;
                            }
                        }
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
            let world = camera_norm_from_ndc(cursor_ndc, old_zoom, zoom_offset, yaw);
            let rotated = rotate_2d(world, yaw);
            zoom_offset = [
                cursor_ndc[0] / new_zoom - rotated[0],
                cursor_ndc[1] / new_zoom - rotated[1],
            ];
            zoom = new_zoom;
            let camera_uniform = CameraUniform {
                view: make_view_matrix(zoom, zoom_offset, yaw, pitch),
                projection,
            };
            write_camera_buffer(&mut ctx, camera_buffer, &camera_uniform)?;
            camera_info = build_camera_info(zoom, zoom_offset, yaw, &settings, &normalization);
            needs_refresh = true;
        }

        if rotate_delta.abs() > f32::EPSILON {
            yaw = (yaw + rotate_delta * 0.05) % std::f32::consts::TAU;
            let camera_uniform = CameraUniform {
                view: make_view_matrix(zoom, zoom_offset, yaw, pitch),
                projection,
            };
            write_camera_buffer(&mut ctx, camera_buffer, &camera_uniform)?;
            camera_info = build_camera_info(zoom, zoom_offset, yaw, &settings, &normalization);
            needs_refresh = true;
        }

        if pitch_delta.abs() > f32::EPSILON {
            pitch = (pitch + pitch_delta * 0.05).clamp(-1.1, 1.1);
            let camera_uniform = CameraUniform {
                view: make_view_matrix(zoom, zoom_offset, yaw, pitch),
                projection,
            };
            write_camera_buffer(&mut ctx, camera_buffer, &camera_uniform)?;
        }

        if needs_refresh {
            let (next_draws, next_set) = refresh_visible_chunks(
                &mut ctx,
                &mut db,
                project_key,
                &settings,
                &camera_info,
                &normalization,
                &mut chunk_cache,
            )?;
            if next_set != visible_set {
                visible_set = next_set;
                chunk_draws = next_draws;
            }
            needs_refresh = false;
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
                depth_attachment: Some(depth_view),
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
                depth_clear: Some(ClearValue::DepthStencil {
                    depth: 1.0,
                    stencil: 0,
                }),
                ..Default::default()
            });

            let mut draw = draw;
            for chunk in &chunk_draws {
                draw = draw.draw_indexed(&DrawIndexed {
                    vertices: chunk.vertex_buffer,
                    indices: chunk.index_buffer,
                    bind_tables: [Some(camera_table), None, None, None],
                    dynamic_buffers: Default::default(),
                    instance_count: 1,
                    first_instance: 0,
                    index_count: chunk.index_count,
                });
            }

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

fn build_terrain_normalization(settings: &TerrainProjectSettings) -> TerrainNormalization {
    let world_min = settings.world_bounds_min;
    let world_max = settings.world_bounds_max;
    let world_size_x = (world_max[0] - world_min[0]).max(1.0);
    let world_size_z = (world_max[2] - world_min[2]).max(1.0);
    let min_height = world_min[1];
    let max_height = world_max[1];
    let height_range = (max_height - min_height).max(1.0);
    let norm_scale_x = 2.0 / world_size_x;
    let norm_scale_z = 2.0 / world_size_z;
    let norm_scale_y = 2.0 / height_range;
    let inv_scale_x = 1.0 / norm_scale_x.max(0.001);
    let inv_scale_z = 1.0 / norm_scale_z.max(0.001);
    let inv_scale_y = 1.0 / norm_scale_y.max(0.001);

    TerrainNormalization {
        world_min,
        world_size_x,
        world_size_z,
        min_height,
        height_range,
        inv_scale_x,
        inv_scale_y,
        inv_scale_z,
    }
}

fn build_chunk_draw(
    ctx: &mut Context,
    artifact: &TerrainChunkArtifact,
    normalization: &TerrainNormalization,
) -> Result<TerrainChunkDraw, Box<dyn Error>> {
    let vertices = build_chunk_vertices(artifact, normalization);
    let vertex_bytes = bytemuck::cast_slice(&vertices);
    let vertex_buffer = ctx.make_buffer(&BufferInfo {
        debug_name: "terrain_chunk_vertices",
        byte_size: vertex_bytes.len() as u32,
        visibility: MemoryVisibility::CpuAndGpu,
        usage: BufferUsage::VERTEX,
        initial_data: Some(vertex_bytes),
    })?;

    let indices = build_chunk_indices(artifact);
    let index_bytes = bytemuck::cast_slice(indices.as_slice());
    let index_buffer = ctx.make_buffer(&BufferInfo {
        debug_name: "terrain_chunk_indices",
        byte_size: index_bytes.len() as u32,
        visibility: MemoryVisibility::Gpu,
        usage: BufferUsage::INDEX,
        initial_data: Some(index_bytes),
    })?;

    Ok(TerrainChunkDraw {
        vertex_buffer,
        index_buffer,
        index_count: indices.len() as u32,
    })
}

fn build_chunk_vertices(
    artifact: &TerrainChunkArtifact,
    normalization: &TerrainNormalization,
) -> Vec<TerrainVertex> {
    let TerrainNormalization {
        world_min,
        world_size_x,
        world_size_z,
        min_height,
        height_range,
        inv_scale_x,
        inv_scale_y,
        inv_scale_z,
    } = *normalization;

    let grid_x = artifact.grid_size[0];
    let grid_y = artifact.grid_size[1];
    let spacing = artifact.sample_spacing.max(0.0001);
    let chunk_size_x = spacing * grid_x.saturating_sub(1) as f32;
    let chunk_size_z = spacing * grid_y.saturating_sub(1) as f32;
    let origin_x = world_min[0] + artifact.chunk_coords[0] as f32 * chunk_size_x;
    let origin_z = world_min[2] + artifact.chunk_coords[1] as f32 * chunk_size_z;

    let mut vertices = Vec::with_capacity(artifact.heights.len());
    for y in 0..grid_y {
        for x in 0..grid_x {
            let idx = (y * grid_x + x) as usize;
            let height = artifact.heights.get(idx).copied().unwrap_or_default();
            let world_x = origin_x + x as f32 * spacing;
            let world_z = origin_z + y as f32 * spacing;
            let normal = artifact
                .normals
                .get(idx)
                .copied()
                .unwrap_or([0.0, 1.0, 0.0]);
            let norm_x = ((world_x - world_min[0]) / world_size_x) * 2.0 - 1.0;
            let norm_z = ((world_z - world_min[2]) / world_size_z) * 2.0 - 1.0;
            let norm_y = ((height - min_height) / height_range) * 2.0 - 1.0;
            let normal = normalize_vec3([
                normal[0] * inv_scale_x,
                normal[1] * inv_scale_y,
                normal[2] * inv_scale_z,
            ]);
            let t = ((height - min_height) / height_range).clamp(0.0, 1.0);
            let color = [0.1 + 0.4 * t, 0.3 + 0.5 * t, 0.2 + 0.2 * t];

            vertices.push(TerrainVertex {
                position: [norm_x, norm_y, norm_z],
                normal,
                color,
            });
        }
    }

    vertices
}

fn build_chunk_indices(artifact: &TerrainChunkArtifact) -> Vec<u32> {
    let grid_x = artifact.grid_size[0];
    let grid_y = artifact.grid_size[1];
    let sample_count = (grid_x * grid_y) as usize;
    let mut indices =
        Vec::with_capacity((grid_x.saturating_sub(1) * grid_y.saturating_sub(1) * 6) as usize);
    let has_holes = artifact.hole_masks.len() == sample_count;
    for y in 0..grid_y.saturating_sub(1) {
        for x in 0..grid_x.saturating_sub(1) {
            let base = y * grid_x + x;
            let i0 = base;
            let i1 = base + 1;
            let i2 = base + grid_x;
            let i3 = i2 + 1;
            if has_holes
                && (artifact.hole_masks[i0 as usize] != 0
                    || artifact.hole_masks[i1 as usize] != 0
                    || artifact.hole_masks[i2 as usize] != 0
                    || artifact.hole_masks[i3 as usize] != 0)
            {
                continue;
            }
            indices.extend_from_slice(&[i0, i2, i1, i1, i2, i3]);
        }
    }
    indices
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

fn make_view_matrix(zoom: f32, offset: [f32; 2], yaw: f32, pitch: f32) -> [[f32; 4]; 4] {
    let cos_yaw = yaw.cos();
    let sin_yaw = yaw.sin();
    let cos_pitch = pitch.cos();
    let sin_pitch = pitch.sin();
    let right = [cos_yaw, 0.0, -sin_yaw];
    let forward = [sin_yaw, 0.0, cos_yaw];
    let forward_pitch = [forward[0] * cos_pitch, sin_pitch, forward[2] * cos_pitch];
    let up_pitch = [-forward[0] * sin_pitch, cos_pitch, -forward[2] * sin_pitch];
    [
        [zoom * right[0], zoom * right[1], zoom * right[2], 0.0],
        [
            zoom * forward_pitch[0],
            zoom * forward_pitch[1],
            zoom * forward_pitch[2],
            0.0,
        ],
        [-up_pitch[0], -up_pitch[1], -up_pitch[2], 0.0],
        [zoom * offset[0], 0.0, zoom * offset[1], 1.0],
    ]
}

fn make_projection_matrix(near_plane: f32, far_plane: f32) -> [[f32; 4]; 4] {
    let left = -1.0;
    let right = 1.0;
    let bottom = -1.0;
    let top = 1.0;
    let rl = (right - left);
    let tb = (top - bottom);
    let fn_plane = (far_plane - near_plane);

    [
        [2.0 / rl, 0.0, 0.0, 0.0],
        [0.0, 2.0 / tb, 0.0, 0.0],
        [0.0, 0.0, 1.0 / fn_plane, 0.0],
        [
            -(right + left) / rl,
            -(top + bottom) / tb,
            -near_plane / fn_plane,
            1.0,
        ],
    ]
}

fn build_camera_info(
    zoom: f32,
    offset: [f32; 2],
    yaw: f32,
    settings: &TerrainProjectSettings,
    normalization: &TerrainNormalization,
) -> TerrainCameraInfo {
    let frustum = build_camera_frustum(zoom, offset, yaw, normalization);
    let center_xz = camera_world_point([0.0, 0.0], zoom, offset, yaw, normalization);
    let center_y = (settings.world_bounds_min[1] + settings.world_bounds_max[1]) * 0.5;
    let position = [center_xz[0], center_y, center_xz[1]];
    let max_dist = frustum
        .iter()
        .map(|corner| {
            let dx = corner[0] - position[0];
            let dz = corner[1] - position[2];
            (dx * dx + dz * dz).sqrt()
        })
        .fold(0.0, f32::max);

    TerrainCameraInfo {
        frustum,
        position,
        curve: 1.0,
        falloff: 0.35,
        max_dist: max_dist.max(1.0),
    }
}

fn build_camera_frustum(
    zoom: f32,
    offset: [f32; 2],
    yaw: f32,
    normalization: &TerrainNormalization,
) -> TerrainFrustum {
    [
        camera_world_point([-1.0, -1.0], zoom, offset, yaw, normalization),
        camera_world_point([1.0, -1.0], zoom, offset, yaw, normalization),
        camera_world_point([1.0, 1.0], zoom, offset, yaw, normalization),
        camera_world_point([-1.0, 1.0], zoom, offset, yaw, normalization),
    ]
}

fn camera_world_point(
    ndc: [f32; 2],
    zoom: f32,
    offset: [f32; 2],
    yaw: f32,
    normalization: &TerrainNormalization,
) -> [f32; 2] {
    let [norm_x, norm_y] = camera_norm_from_ndc(ndc, zoom, offset, yaw);
    let world_x = (norm_x + 1.0) * 0.5 * normalization.world_size_x + normalization.world_min[0];
    let world_z = (norm_y + 1.0) * 0.5 * normalization.world_size_z + normalization.world_min[2];
    [world_x, world_z]
}

fn refresh_visible_chunks(
    ctx: &mut Context,
    db: &mut noren::DB,
    project_key: &str,
    settings: &TerrainProjectSettings,
    camera: &TerrainCameraInfo,
    normalization: &TerrainNormalization,
    cache: &mut std::collections::HashMap<String, TerrainChunkDraw>,
) -> Result<(Vec<TerrainChunkDraw>, std::collections::HashSet<String>), Box<dyn Error>> {
    let artifacts = db
        .fetch_terrain_chunks_for_camera(settings, project_key, camera)
        .map_err(|err| terrain_db_error(err))?;

    let mut entries = std::collections::HashSet::with_capacity(artifacts.len());
    let mut draws = Vec::with_capacity(artifacts.len());
    for artifact in artifacts {
        let coord_key = chunk_coord_key(artifact.chunk_coords[0], artifact.chunk_coords[1]);
        let entry = chunk_artifact_entry(project_key, &coord_key, &lod_key(artifact.lod));
        entries.insert(entry.clone());

        if let Some(cached) = cache.get(&entry) {
            draws.push(*cached);
            continue;
        }

        let draw = build_chunk_draw(ctx, &artifact, normalization)?;
        cache.insert(entry, draw);
        draws.push(draw);
    }

    Ok((draws, entries))
}

fn camera_norm_from_ndc(ndc: [f32; 2], zoom: f32, offset: [f32; 2], yaw: f32) -> [f32; 2] {
    let camera = [ndc[0] / zoom - offset[0], ndc[1] / zoom - offset[1]];
    rotate_2d(camera, -yaw)
}

fn rotate_2d(value: [f32; 2], yaw: f32) -> [f32; 2] {
    let cos_yaw = yaw.cos();
    let sin_yaw = yaw.sin();
    [
        value[0] * cos_yaw - value[1] * sin_yaw,
        value[0] * sin_yaw + value[1] * cos_yaw,
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

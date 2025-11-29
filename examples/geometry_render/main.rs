//! Run with `cargo run --example geometry_render [primitive]` to load a default
//! primitive from the database and upload it to GPU buffers.

#[path = "../common/mod.rs"]
mod common;

use common::{
    DEFAULT_GEOMETRY_PRIMITIVES, default_geometry_entry_from_args, display::blit_image_to_display,
    init_context, open_sample_db,
};
use dashi::driver::command::{BeginDrawing, Draw, DrawIndexed};
use dashi::gpu::{self, CommandStream};
use dashi::{
    ClearValue, CommandQueueInfo2, FRect2D, Handle, Image, ImageInfo, ImageView, Rect2D,
    SubmitInfo, Viewport,
};
use noren::render_graph::RenderGraphRequest;
use std::{env, error::Error};

const FRAMEBUFFER_DIM: [u32; 2] = [800, 600];

fn main() {
    if let Err(err) = run() {
        eprintln!("error: {err}");
        std::process::exit(1);
    }
}

fn run() -> Result<(), Box<dyn Error>> {
    let geometry_entry = default_geometry_entry_from_args(env::args().nth(1));
    println!(
        "Rendering geometry entry '{geometry_entry}'. Default primitives: {}",
        DEFAULT_GEOMETRY_PRIMITIVES.join(", ")
    );

    let mut ctx = match init_context() {
        Ok(ctx) => ctx,
        Err(err) => {
            eprintln!("Skipping example – unable to create GPU context: {err}");
            return Ok(());
        }
    };

    let mut db = open_sample_db(&mut ctx)?;
    let geom_db = db.geometry_mut();

    let host_geometry = geom_db.fetch_raw_geometry(&geometry_entry)?;
    println!(
        "Host geometry '{}' contains {} vertices and {} indices",
        geometry_entry,
        host_geometry.vertices.len(),
        host_geometry.indices.as_ref().map_or(0, |idx| idx.len())
    );

    let device_geometry = geom_db.fetch_gpu_geometry(&geometry_entry)?;
    println!(
        "Uploaded GPU geometry with buffers {:?} / {:?}",
        device_geometry.vertices, device_geometry.indices
    );

    let render_graph = db.create_render_graph(RenderGraphRequest {
        shaders: vec!["shader/default".to_string()],
    })?;

    let pipeline = render_graph
        .pipelines
        .get("shader/default")
        .cloned()
        .ok_or_else(|| {
            std::io::Error::new(
                std::io::ErrorKind::Other,
                "Missing default shader pipeline in render graph",
            )
        })?;

    let framebuffer = ctx.make_image(&ImageInfo {
        debug_name: "geometry_render_fb",
        dim: [FRAMEBUFFER_DIM[0], FRAMEBUFFER_DIM[1], 1],
        format: dashi::Format::RGBA8,
        ..Default::default()
    })?;

    render_geometry(
        &mut ctx,
        &host_geometry,
        device_geometry,
        pipeline.pipeline,
        framebuffer,
    )?;

    println!(
        "Displaying GPU rasterized geometry ({}x{})",
        FRAMEBUFFER_DIM[0], FRAMEBUFFER_DIM[1]
    );
    blit_image_to_display(&mut ctx, framebuffer, FRAMEBUFFER_DIM, "geometry_render")?;

    Ok(())
}

fn render_geometry(
    ctx: &mut gpu::Context,
    host_geometry: &noren::rdb::geometry::HostGeometry,
    device_geometry: noren::rdb::geometry::DeviceGeometry,
    pipeline: Handle<dashi::GraphicsPipeline>,
    framebuffer: Handle<Image>,
) -> Result<(), dashi::GPUError> {
    let mut ring = ctx.make_command_ring(&CommandQueueInfo2 {
        debug_name: "geometry_render_ring",
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
                Some(ClearValue::Color([0.05, 0.05, 0.1, 1.0])),
                None,
                None,
                None,
            ],
        });

        if device_geometry.indices.valid() {
            draw.draw_indexed(&DrawIndexed {
                vertices: device_geometry.vertices,
                indices: device_geometry.indices,
                index_count: host_geometry
                    .indices
                    .as_ref()
                    .map(|idx| idx.len() as u32)
                    .unwrap_or_default(),
                ..Default::default()
            });
        } else {
            draw.draw(&Draw {
                vertices: device_geometry.vertices,
                bind_groups: Default::default(),
                bind_tables: Default::default(),
                dynamic_buffers: Default::default(),
                instance_count: 1,
                count: host_geometry.vertices.len() as u32,
            });
        }

        stream = draw.stop_drawing();
        stream.end().append(list);
    })?;

    ring.submit(&SubmitInfo::default())?;
    ring.wait_all()?;

    Ok(())
}

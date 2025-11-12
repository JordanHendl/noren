//! Run with `cargo run --example model_render` to load the quad model definition
//! and upload its geometry and textures to GPU resources.

#[path = "../common/mod.rs"]
mod common;

use common::{SAMPLE_MODEL_ENTRY, init_context, open_sample_db};
use dashi::builders::RenderPassBuilder;
use dashi::{AttachmentDescription, FRect2D, Format, Rect2D, Viewport};
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
    let render_pass = RenderPassBuilder::new("model_render", viewport)
        .add_subpass(&[color_attachment], None, &[])
        .build(&mut ctx)?;

    let host_model = db.fetch_model(SAMPLE_MODEL_ENTRY)?;
    println!(
        "Host model '{}' contains {} mesh(es)",
        host_model.name,
        host_model.meshes.len()
    );
    for mesh in &host_model.meshes {
        println!(
            " - Mesh '{}' has {} vertices and {} material texture(s)",
            mesh.name,
            mesh.geometry.vertices.len(),
            mesh.material.as_ref().map_or(0, |mat| mat.textures.len())
        );
    }

    let device_model = db.fetch_gpu_model(SAMPLE_MODEL_ENTRY, render_pass)?;
    for mesh in &device_model.meshes {
        println!(
            "Uploaded mesh with vertex buffer {:?} and index buffer {:?}",
            mesh.geometry.vertices, mesh.geometry.indices
        );
        if let Some(material) = &mesh.material {
            for texture in &material.textures {
                let gpu_name = {
                    let bytes = &texture.image.info.name;
                    let len = bytes.iter().position(|&b| b == 0).unwrap_or(bytes.len());
                    std::str::from_utf8(&bytes[..len]).unwrap_or("")
                };
                println!(
                    "   - Texture '{}' uploaded as {:?}",
                    gpu_name, texture.image
                );
            }
        }
    }

    // Should render to a display, with a camera.
    todo!();

    Ok(())
}

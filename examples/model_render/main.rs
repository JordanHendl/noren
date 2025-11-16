//! Run with `cargo run --example model_render` to load the quad model definition
//! and upload its geometry and textures to GPU resources.

#[path = "../common/mod.rs"]
mod common;

use common::{SAMPLE_MODEL_ENTRY, init_context, open_sample_db};
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

    let device_model = db.fetch_gpu_model(SAMPLE_MODEL_ENTRY)?;
    for mesh in &device_model.meshes {
        println!(
            "Uploaded mesh with vertex buffer {:?} and index buffer {:?}",
            mesh.geometry.vertices, mesh.geometry.indices
        );
    }

    let graph = db.create_render_graph(RenderGraphRequest {
        shaders: vec!["shader/default".to_string()],
    })?;

    if let Some(binding) = graph.pipelines.get("shader/default") {
        println!(
            "Prepared graphics pipeline {:?} with layout {:?}",
            binding.pipeline, binding.pipeline_layout
        );
    }

    if let Some(pass) = graph.render_passes.get("render_pass/default") {
        println!("Render pass handle: {:?}", pass);
    }

    // Should render to a display, with a camera.
    todo!();

    Ok(())
}

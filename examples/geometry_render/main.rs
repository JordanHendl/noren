//! Run with `cargo run --example geometry_render` to load geometry from the
//! database and upload it to GPU buffers.

#[path = "../common/mod.rs"]
mod common;

use common::{SAMPLE_GEOMETRY_ENTRY, init_headless_context, open_sample_db};
use std::error::Error;

fn main() {
    if let Err(err) = run() {
        eprintln!("error: {err}");
        std::process::exit(1);
    }
}

fn run() -> Result<(), Box<dyn Error>> {
    let mut ctx = match init_headless_context() {
        Ok(ctx) => ctx,
        Err(err) => {
            eprintln!("Skipping example – unable to create GPU context: {err}");
            return Ok(());
        }
    };

    let mut db = open_sample_db(&mut ctx)?;
    let geom_db = db.geometry_mut();

    let host_geometry = geom_db.fetch_raw_geometry(SAMPLE_GEOMETRY_ENTRY)?;
    println!(
        "Host geometry '{}' contains {} vertices and {} indices",
        SAMPLE_GEOMETRY_ENTRY,
        host_geometry.vertices.len(),
        host_geometry.indices.as_ref().map_or(0, |idx| idx.len())
    );

    let device_geometry = geom_db.fetch_gpu_geometry(SAMPLE_GEOMETRY_ENTRY)?;
    println!(
        "Uploaded GPU geometry with buffers {:?} / {:?}",
        device_geometry.vertices, device_geometry.indices
    );

    Ok(())
}

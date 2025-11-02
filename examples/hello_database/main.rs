//! Run with `cargo run --example hello_database` to verify that the sample
//! database can be opened and queried.

#[path = "../common/mod.rs"]
mod common;

use common::{SAMPLE_GEOMETRY_ENTRY, SAMPLE_TEXTURE_ENTRY, init_headless_context, open_sample_db};
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

    let geometry = db
        .geometry_mut()
        .fetch_raw_geometry(SAMPLE_GEOMETRY_ENTRY)?;
    println!(
        "Loaded geometry '{}' with {} vertices",
        SAMPLE_GEOMETRY_ENTRY,
        geometry.vertices.len()
    );

    let image = db.imagery_mut().fetch_raw_image(SAMPLE_TEXTURE_ENTRY)?;
    println!(
        "Loaded texture '{}' with dimensions {}x{}",
        SAMPLE_TEXTURE_ENTRY,
        image.info().dim[0],
        image.info().dim[1]
    );

    Ok(())
}

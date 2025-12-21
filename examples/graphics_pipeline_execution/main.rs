//! Builds a graphics pipeline for the sample database shader metadata.
//!
//! This example demonstrates how callers can use the database to construct
//! pipeline layouts and pipelines before recording their own render passes.

use std::error::Error;

#[path = "../common/mod.rs"]
mod common;

fn main() {
    if let Err(err) = run() {
        eprintln!("error: {err}");
        std::process::exit(1);
    }
}

fn run() -> Result<(), Box<dyn Error>> {
    // Build a headless context suitable for offline pipeline creation. When the
    // host lacks a compatible device, the error is propagated to the caller so
    // the example can exit cleanly.
    let mut ctx = dashi::Context::headless(&Default::default())?;

    // Locate the bundled database metadata and shader modules.
    let base_dir = common::sample_db_path();
    let layout_path = base_dir.join("layout.json");

    let base_dir_str = base_dir.to_str().expect("base dir utf-8");
    let layout_str = layout_path.to_str().expect("layout utf-8");

    let info = noren::DBInfo {
        base_dir: base_dir_str,
        layout_file: Some(layout_str),
        pooled_geometry_uploads: false,
    };

    let mut db = noren::DB::new(&info)?;
    db.import_dashi_context(&mut ctx);
    // Create a pipeline layout for the default quad shader and immediately
    // build a graphics pipeline from it.
    let layout = db.make_pipeline_layout("shader/default")?;
    let pipeline = db.make_graphics_pipeline("shader/default")?;

    println!(
        "Created layout {:?} and pipeline {:?} for shader/default",
        layout, pipeline
    );

    Ok(())
}

//! Run with `cargo run --example image_blit` to record a simple blit command
//! that copies a database image into a fresh framebuffer.

#[path = "../common/mod.rs"]
mod common;

use common::{SAMPLE_TEXTURE_ENTRY, display::blit_image_to_display, init_context, open_sample_db};
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
    let imagery = db.imagery_mut();

    let source_image = imagery.fetch_gpu_image(SAMPLE_TEXTURE_ENTRY)?;
    let dims = source_image.info.dim;
    println!(
        "Blitting image '{}' ({}x{})",
        SAMPLE_TEXTURE_ENTRY, dims[0], dims[1]
    );

    blit_image_to_display(&mut ctx, source_image.img, [dims[0], dims[1]], "image_blit")?;

    Ok(())
}

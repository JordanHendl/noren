//! Run with `cargo run --example image_blit` to record a simple blit command
//! that copies a database image into a fresh framebuffer.

#[path = "../common/mod.rs"]
mod common;

use common::{SAMPLE_TEXTURE_ENTRY, init_context, open_sample_db};
use dashi::driver::command::BlitImage;
use dashi::gpu::CommandStream;
use dashi::gpu::driver::state::SubresourceRange;
use dashi::{Filter, Rect2D};
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

    let source_image = imagery.fetch_raw_image(SAMPLE_TEXTURE_ENTRY)?;
    let mut source_info = source_image.info().dashi();
    source_info.debug_name = SAMPLE_TEXTURE_ENTRY;
    source_info.initial_data = Some(source_image.data());
    let source_handle = ctx.make_image(&source_info)?;

    let mut target_info = source_info;
    target_info.debug_name = "image_blit/target";
    target_info.initial_data = None;
    let target_handle = ctx.make_image(&target_info)?;

    let dims = source_image.info().dim;
    let full_rect = Rect2D {
        x: 0,
        y: 0,
        w: dims[0],
        h: dims[1],
    };

    let mut stream = CommandStream::new().begin();
    stream.blit_images(&BlitImage {
        src: source_handle,
        dst: target_handle,
        src_range: SubresourceRange::default(),
        dst_range: SubresourceRange::default(),
        filter: Filter::Linear,
        src_region: full_rect,
        dst_region: full_rect,
    });
    let _commands = stream.end();

    println!(
        "Recorded blit commands for '{}' ({}x{})",
        SAMPLE_TEXTURE_ENTRY, dims[0], dims[1]
    );

    // Should blit to a display.
    todo!();

    Ok(())
}

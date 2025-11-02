//! Run with `cargo run --example bindless_render` to build a bindless texture
//! table from assets in the sample database.

#[path = "../common/mod.rs"]
mod common;

use common::{SAMPLE_TEXTURE_ENTRY, init_context, open_sample_db};
use dashi::builders::{BindTableBuilder, BindTableLayoutBuilder};
use dashi::{
    BindGroupVariable, BindGroupVariableType, ImageView, IndexedResource, SamplerInfo, ShaderInfo,
    ShaderResource, ShaderType,
};
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

    let texture = imagery.fetch_raw_image(SAMPLE_TEXTURE_ENTRY)?;
    let mut image_info = texture.info().dashi();
    image_info.debug_name = SAMPLE_TEXTURE_ENTRY;
    image_info.initial_data = Some(texture.data());
    let gpu_texture = ctx.make_image(&image_info)?;

    let sampler = ctx.make_sampler(&SamplerInfo::default())?;
    let shader_info = ShaderInfo {
        shader_type: ShaderType::Fragment,
        variables: &[BindGroupVariable {
            var_type: BindGroupVariableType::SampledImage,
            binding: 0,
            count: 1,
        }],
    };

    let layout = BindTableLayoutBuilder::new("bindless_layout")
        .shader(shader_info)
        .build(&mut ctx)?;

    let resources = [IndexedResource {
        slot: 0,
        resource: ShaderResource::SampledImage(
            ImageView {
                img: gpu_texture,
                ..Default::default()
            },
            sampler,
        ),
    }];

    let table = BindTableBuilder::new("bindless_table")
        .layout(layout)
        .binding(0, &resources)
        .build(&mut ctx)?;

    println!(
        "Created bindless table {:?} containing texture '{}'",
        table, SAMPLE_TEXTURE_ENTRY
    );
    
    // Should render to a display, with a camera.
    todo!();
    Ok(())
}

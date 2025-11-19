//! Run with `cargo run --example pipeline_layouts` to see the bind layouts
//! prepared alongside a graphics pipeline pulled from the sample render graph.

#[path = "../common/mod.rs"]
mod common;

use common::{init_context, open_sample_db};
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
    // A render graph is built from shader keys present in the sample layout file.
    // List every shader you intend to draw with so the factory can assemble
    // matching render passes, pipelines, and the bind layouts they rely on.
    let graph = db.create_render_graph(RenderGraphRequest {
        shaders: vec!["shader/default".to_string()],
    })?;

    if let Some(binding) = graph.pipelines.get("shader/default") {
        println!(
            "Prepared graphics pipeline {:?} with layout {:?}",
            binding.pipeline, binding.pipeline_layout
        );
        println!("Use these layouts when creating the bind groups/tables you will bind before drawing with this pipeline:");

        for (set, layout) in binding.bind_group_layouts.iter().enumerate() {
            match layout {
                Some(layout) => {
                    println!(
                        "Bind group layout [{}] handle {:?} (use when creating resources for this set)",
                        set, layout
                    );
                }
                None => println!("Bind group layout [{}] unused by this shader", set),
            }
        }

        for (table, layout) in binding.bind_table_layouts.iter().enumerate() {
            match layout {
                Some(layout) => {
                    println!(
                        "Bind table layout [{}] handle {:?} (pass to BindTableBuilder when filling that table)",
                        table, layout
                    );
                }
                None => println!("Bind table layout [{}] unused by this shader", table),
            }
        }
    } else {
        println!("No pipeline named 'shader/default' found in render graph");
    }

    if let Some(pass) = graph.render_passes.get("render_pass/default") {
        println!("Render pass handle: {:?}", pass);
        println!(
            "Begin this render pass on a command list, bind the pipeline/layout above,\n \
             then supply vertex/index buffers and draw calls as usual."
        );
    }

    Ok(())
}

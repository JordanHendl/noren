//! Loads a model from the sample database.
//!
//! Rendering responsibilities (render passes, pipeline layouts, and pipelines)
//! are left to callers now that the database layer only loads assets.

use std::error::Error;

fn main() {
    if let Err(err) = run() {
        eprintln!("error: {err}");
        std::process::exit(1);
    }
}

fn run() -> Result<(), Box<dyn Error>> {
    println!(
        "Model loading no longer builds pipelines; please create your own render pass and pipeline to draw the data."
    );
    Ok(())
}

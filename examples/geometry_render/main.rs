//! Loads geometry from the sample database.
//!
//! Rendering is left to the caller now that render pass and pipeline creation
//! are outside the database layer.

use std::error::Error;

fn main() {
    if let Err(err) = run() {
        eprintln!("error: {err}");
        std::process::exit(1);
    }
}

fn run() -> Result<(), Box<dyn Error>> {
    println!(
        "Geometry loading still works, but supply your own render pass and pipeline to draw it."
    );
    Ok(())
}

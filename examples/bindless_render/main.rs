//! Run with `cargo run --example bindless_render` to build a bindless texture
//! table from assets in the sample database.

#[path = "../common/mod.rs"]
mod common;

use std::error::Error;

fn main() {
    if let Err(err) = run() {
        eprintln!("error: {err}");
        std::process::exit(1);
    }
}

fn run() -> Result<(), Box<dyn Error>> {
    println!(
        "Bindless rendering now requires you to supply render passes and pipelines; the database loader no longer builds them."
    );

    Ok(())
}

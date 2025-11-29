//! Prints database pipeline layout information for shader metadata.

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
        "Pipeline layout inspection now requires caller-managed render passes; the database no longer builds render graphs."
    );

    Ok(())
}

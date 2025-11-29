//! Renders a material from the sample database.
//!
//! With render pass and pipeline construction removed from the database layer,
//! this example now only documents that responsibility.

use std::error::Error;

fn main() {
    if let Err(err) = run() {
        eprintln!("error: {err}");
        std::process::exit(1);
    }
}

fn run() -> Result<(), Box<dyn Error>> {
    println!(
        "Material rendering now requires user-managed render passes, layouts, and pipelines. Supply those handles before drawing."
    );
    Ok(())
}

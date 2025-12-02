//! Load the bundled skeleton and animation clip and print basic details.
//!
//! Run with `cargo run --example skeleton_animation_load` after generating the
//! sample database in `sample/db`.

#[path = "../common/mod.rs"]
mod common;

use common::{SAMPLE_ANIMATION_ENTRY, SAMPLE_SKELETON_ENTRY, init_context, open_sample_db};
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

    let skeleton = db.skeletons_mut().fetch_skeleton(SAMPLE_SKELETON_ENTRY)?;
    println!(
        "Loaded skeleton '{}' with {} joints (root: {:?})",
        SAMPLE_SKELETON_ENTRY,
        skeleton.joints.len(),
        skeleton.root
    );

    let animation = db
        .animations_mut()
        .fetch_animation(SAMPLE_ANIMATION_ENTRY)?;
    println!(
        "Loaded animation '{}' lasting {:.2} seconds with {} channels",
        SAMPLE_ANIMATION_ENTRY,
        animation.duration_seconds,
        animation.channels.len()
    );

    Ok(())
}

use std::{
    error::Error,
    fs,
    path::{Path, PathBuf},
};

use dashi::{ContextInfo, gpu};
use image::RgbaImage;
use noren::{DB, DBInfo, datatypes::DatabaseEntry};

/// Absolute path to the bundled sample database directory.
pub fn sample_db_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("sample/db")
}

/// Entry name for the quad geometry in the bundled database.
#[allow(dead_code)]
pub const SAMPLE_GEOMETRY_ENTRY: DatabaseEntry = "geometry/quad";

/// Entry name for a representative texture in the bundled database.
#[allow(dead_code)]
pub const SAMPLE_TEXTURE_ENTRY: DatabaseEntry = "imagery/tulips";

/// Entry name for the bundled quad model definition.
#[allow(dead_code)]
pub const SAMPLE_MODEL_ENTRY: DatabaseEntry = "model/quad";

/// Relative directory used for example artifacts.
pub const EXAMPLE_OUTPUT_DIR: &str = "target/example_outputs";

/// Convenience wrapper that creates a headless GPU context suitable for the
/// examples. When the host system lacks a Vulkan device the failure is
/// propagated so that callers can gracefully skip the demo.
pub fn init_context() -> Result<gpu::Context, dashi::GPUError> {
    gpu::Context::new(&ContextInfo::default())
}

/// Ensures the standard output directory for an example exists and returns the
/// full path to an artifact within it.
pub fn prepare_example_artifact(example: &str, artifact: &str) -> Result<PathBuf, Box<dyn Error>> {
    let base = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join(EXAMPLE_OUTPUT_DIR)
        .join(example);
    fs::create_dir_all(&base)?;
    Ok(base.join(artifact))
}

/// Writes a text artifact into the standard example output directory.
pub fn write_text_artifact(
    example: &str,
    artifact: &str,
    contents: &str,
) -> Result<PathBuf, Box<dyn Error>> {
    let path = prepare_example_artifact(example, artifact)?;
    fs::write(&path, contents)?;
    Ok(path)
}

/// Writes an RGBA image artifact into the standard example output directory.
pub fn write_image_artifact(
    example: &str,
    artifact: &str,
    image: &RgbaImage,
) -> Result<PathBuf, Box<dyn Error>> {
    let path = prepare_example_artifact(example, artifact)?;
    image.save(&path)?;
    Ok(path)
}

/// Open the bundled sample database against the provided GPU context.
pub fn open_sample_db(ctx: &mut gpu::Context) -> Result<DB, Box<dyn Error>> {
    let base_dir = sample_db_path();
    let layout_path = base_dir.join("layout.json");

    let base_dir_str = path_to_string(&base_dir)?;
    let layout_str = path_to_string(&layout_path)?;

    let info = DBInfo {
        ctx,
        base_dir: &base_dir_str,
        layout_file: Some(&layout_str),
    };

    Ok(DB::new(&info)?)
}

fn path_to_string(path: &Path) -> Result<String, Box<dyn Error>> {
    path.to_str()
        .map(|s| s.to_owned())
        .ok_or_else(|| format!("path '{}' is not valid UTF-8", path.display()).into())
}

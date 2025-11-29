use std::{
    error::Error,
    path::{Path, PathBuf},
};

use dashi::{ContextInfo, gpu};
use noren::{DB, DBInfo, rdb::DatabaseEntry};

pub mod display;

pub const DEFAULT_GEOMETRY_PRIMITIVES: [&str; 6] = [
    "geometry/sphere",
    "geometry/cube",
    "geometry/quad",
    "geometry/plane",
    "geometry/cylinder",
    "geometry/cone",
];

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

/// Convenience wrapper that creates a headless GPU context suitable for the
/// examples. When the host system lacks a Vulkan device the failure is
/// propagated so that callers can gracefully skip the demo.
pub fn init_context() -> Result<gpu::Context, dashi::GPUError> {
    gpu::Context::new(&ContextInfo::default())
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

/// Resolve an input argument into a geometry database entry name.
///
/// When no value is provided the default "geometry/sphere" is used. Arguments
/// without the `geometry/` prefix are automatically expanded.
pub fn default_geometry_entry_from_args(arg: Option<String>) -> String {
    let requested = arg.unwrap_or_else(|| "sphere".to_string());
    let entry = if requested.starts_with("geometry/") {
        requested
    } else {
        format!("geometry/{requested}")
    };

    if !DEFAULT_GEOMETRY_PRIMITIVES
        .iter()
        .any(|primitive| *primitive == entry)
    {
        eprintln!(
            "Provided primitive '{entry}' is not in the default set: {}",
            DEFAULT_GEOMETRY_PRIMITIVES.join(", ")
        );
    }

    entry
}

fn path_to_string(path: &Path) -> Result<String, Box<dyn Error>> {
    path.to_str()
        .map(|s| s.to_owned())
        .ok_or_else(|| format!("path '{}' is not valid UTF-8", path.display()).into())
}

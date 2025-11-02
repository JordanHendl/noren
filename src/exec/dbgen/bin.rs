use std::{
    fs::{self, File},
    io::BufReader,
    path::{Path, PathBuf},
};

use image::DynamicImage;
use noren::{
    DatabaseLayoutFile, RDBFile, RdbErr,
    datatypes::{HostGeometry, HostImage, ImageInfo, primitives::Vertex},
};
use serde::{Deserialize, Serialize};

fn main() {
    let mut args = std::env::args();
    let _ = args.next();

    let Some(input) = args.next() else {
        eprintln!("Usage: noren_dbgen <staging-build.json>");
        std::process::exit(1);
    };

    if args.next().is_some() {
        eprintln!("Usage: noren_dbgen <staging-build.json>");
        std::process::exit(1);
    }

    if let Err(err) = run_from_path(Path::new(&input)) {
        eprintln!("error: {err}");
        std::process::exit(1);
    }
}

fn run_from_path(input: &Path) -> Result<(), BuildError> {
    let file = File::open(input)?;
    let reader = BufReader::new(file);
    let spec: BuildSpec = serde_json::from_reader(reader)?;

    let base_dir = input
        .parent()
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."));

    let BuildSpec {
        output,
        imagery,
        geometry,
        models,
    } = spec;

    let output_dir = resolve_path(&base_dir, &output.directory);
    fs::create_dir_all(&output_dir)?;

    let geometry_path = resolve_string_path(&output_dir, &output.layout.geometry);
    let imagery_path = resolve_string_path(&output_dir, &output.layout.imagery);
    let models_path = resolve_string_path(&output_dir, &output.layout.models);
    let layout_path = resolve_path(&output_dir, &output.layout_file);

    build_geometry(&base_dir, &geometry_path, &geometry)?;
    build_imagery(&base_dir, &imagery_path, &imagery)?;

    if let Some(parent) = models_path.parent() {
        fs::create_dir_all(parent)?;
    }
    let models_file = File::create(&models_path)?;
    serde_json::to_writer_pretty(models_file, &ModelFile { models })?;

    if let Some(parent) = layout_path.parent() {
        fs::create_dir_all(parent)?;
    }
    let layout_file = File::create(&layout_path)?;
    serde_json::to_writer_pretty(layout_file, &output.layout)?;

    Ok(())
}

fn build_geometry(
    base_dir: &Path,
    output: &Path,
    entries: &[GeometryEntry],
) -> Result<(), BuildError> {
    if let Some(parent) = output.parent() {
        fs::create_dir_all(parent)?;
    }

    let mut rdb = RDBFile::new();

    for entry in entries {
        let host = load_geometry(base_dir, entry)?;
        rdb.add(&entry.entry, &host).map_err(BuildError::from)?;
    }

    rdb.save(output).map_err(BuildError::from)?;
    Ok(())
}

fn load_geometry(base_dir: &Path, entry: &GeometryEntry) -> Result<HostGeometry, BuildError> {
    let path = resolve_path(base_dir, &entry.file);
    let (doc, buffers, _) = gltf::import(path)?;

    let mesh = if let Some(ref mesh_name) = entry.mesh {
        doc.meshes()
            .find(|m| m.name().map(|n| n == mesh_name).unwrap_or(false))
            .ok_or_else(|| BuildError::message(format!("mesh '{}' not found", mesh_name)))?
    } else {
        doc.meshes()
            .next()
            .ok_or_else(|| BuildError::message("geometry file did not contain any meshes"))?
    };

    let primitive_index = entry.primitive.unwrap_or(0);
    let primitive = mesh
        .primitives()
        .nth(primitive_index)
        .ok_or_else(|| BuildError::message(format!("primitive {} not found", primitive_index)))?;

    let reader = primitive.reader(|buffer| Some(&buffers[buffer.index()].0[..]));

    let positions: Vec<[f32; 3]> = reader
        .read_positions()
        .ok_or_else(|| BuildError::message("mesh is missing POSITION attribute"))?
        .collect();

    let vertex_count = positions.len();

    let normals: Vec<[f32; 3]> = reader
        .read_normals()
        .map(|iter| iter.collect())
        .unwrap_or_else(|| vec![[0.0, 0.0, 1.0]; vertex_count]);

    let tangents: Vec<[f32; 4]> = reader
        .read_tangents()
        .map(|iter| iter.collect())
        .unwrap_or_else(|| vec![[1.0, 0.0, 0.0, 1.0]; vertex_count]);

    let tex_coords: Vec<[f32; 2]> = reader
        .read_tex_coords(0)
        .map(|iter| iter.into_f32().collect())
        .unwrap_or_else(|| vec![[0.0, 0.0]; vertex_count]);

    let colors: Vec<[f32; 4]> = reader
        .read_colors(0)
        .map(|iter| iter.into_rgba_f32().collect())
        .unwrap_or_else(|| vec![[1.0, 1.0, 1.0, 1.0]; vertex_count]);

    let indices = reader
        .read_indices()
        .map(|iter| iter.into_u32().collect::<Vec<u32>>());

    let vertices: Vec<Vertex> = (0..vertex_count)
        .map(|idx| Vertex {
            position: positions[idx],
            normal: normals.get(idx).copied().unwrap_or([0.0, 0.0, 1.0]),
            tangent: tangents.get(idx).copied().unwrap_or([1.0, 0.0, 0.0, 1.0]),
            uv: tex_coords.get(idx).copied().unwrap_or([0.0, 0.0]),
            color: colors.get(idx).copied().unwrap_or([1.0, 1.0, 1.0, 1.0]),
        })
        .collect();

    Ok(HostGeometry { vertices, indices })
}

fn build_imagery(base_dir: &Path, output: &Path, entries: &[ImageEntry]) -> Result<(), BuildError> {
    if let Some(parent) = output.parent() {
        fs::create_dir_all(parent)?;
    }

    let mut rdb = RDBFile::new();

    for entry in entries {
        let image = load_image(base_dir, entry)?;
        rdb.add(&entry.entry, &image).map_err(BuildError::from)?;
    }

    rdb.save(output).map_err(BuildError::from)?;
    Ok(())
}

fn load_image(base_dir: &Path, entry: &ImageEntry) -> Result<HostImage, BuildError> {
    let path = resolve_path(base_dir, &entry.file);
    let image = image::open(&path)?;
    let rgba = to_rgba(image);
    let (width, height) = rgba.dimensions();
    let data = rgba.into_raw();

    let info = ImageInfo {
        name: entry.entry.clone(),
        dim: [width, height, 1],
        layers: entry.layers,
        format: entry.format,
        mip_levels: entry.mip_levels,
    };

    Ok(HostImage::new(info, data))
}

fn to_rgba(image: DynamicImage) -> image::RgbaImage {
    match image {
        DynamicImage::ImageRgba8(img) => img,
        other => other.to_rgba8(),
    }
}

fn resolve_path(base: &Path, value: &Path) -> PathBuf {
    if value.is_absolute() {
        value.to_path_buf()
    } else {
        base.join(value)
    }
}

fn resolve_string_path(base: &Path, value: &str) -> PathBuf {
    let path = Path::new(value);
    resolve_path(base, path)
}

#[derive(Debug, Deserialize, Serialize)]
struct BuildSpec {
    #[serde(default)]
    output: OutputSpec,
    #[serde(default)]
    imagery: Vec<ImageEntry>,
    #[serde(default)]
    geometry: Vec<GeometryEntry>,
    #[serde(default)]
    models: Vec<ModelEntry>,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(default)]
struct OutputSpec {
    directory: PathBuf,
    layout_file: PathBuf,
    layout: DatabaseLayoutFile,
}

impl Default for OutputSpec {
    fn default() -> Self {
        Self {
            directory: PathBuf::from("db"),
            layout_file: PathBuf::from("layout.json"),
            layout: DatabaseLayoutFile::default(),
        }
    }
}

#[derive(Debug, Deserialize, Serialize)]
struct GeometryEntry {
    entry: String,
    file: PathBuf,
    #[serde(default)]
    mesh: Option<String>,
    #[serde(default)]
    primitive: Option<usize>,
}

#[derive(Debug, Deserialize, Serialize)]
struct ImageEntry {
    entry: String,
    file: PathBuf,
    #[serde(default = "default_layers")]
    layers: u32,
    #[serde(default = "default_format")]
    format: dashi::Format,
    #[serde(default = "default_mip_levels")]
    mip_levels: u32,
}

fn default_layers() -> u32 {
    1
}

fn default_mip_levels() -> u32 {
    1
}

fn default_format() -> dashi::Format {
    dashi::Format::RGBA8
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct ModelEntry {
    name: String,
    geometry: String,
    #[serde(default)]
    textures: Vec<String>,
}

#[derive(Serialize)]
struct ModelFile {
    models: Vec<ModelEntry>,
}

#[derive(Debug)]
enum BuildError {
    Io(std::io::Error),
    Json(serde_json::Error),
    Image(image::ImageError),
    Gltf(gltf::Error),
    Rdb(RdbErr),
    Message(String),
}

impl BuildError {
    fn message<T: Into<String>>(msg: T) -> Self {
        BuildError::Message(msg.into())
    }
}

impl std::fmt::Display for BuildError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            BuildError::Io(err) => write!(f, "I/O error: {err}"),
            BuildError::Json(err) => write!(f, "JSON error: {err}"),
            BuildError::Image(err) => write!(f, "image decode error: {err}"),
            BuildError::Gltf(err) => write!(f, "glTF error: {err}"),
            BuildError::Rdb(err) => write!(f, "RDB error: {err}"),
            BuildError::Message(msg) => write!(f, "{msg}"),
        }
    }
}

impl std::error::Error for BuildError {}

impl From<std::io::Error> for BuildError {
    fn from(value: std::io::Error) -> Self {
        BuildError::Io(value)
    }
}

impl From<serde_json::Error> for BuildError {
    fn from(value: serde_json::Error) -> Self {
        BuildError::Json(value)
    }
}

impl From<image::ImageError> for BuildError {
    fn from(value: image::ImageError) -> Self {
        BuildError::Image(value)
    }
}

impl From<gltf::Error> for BuildError {
    fn from(value: gltf::Error) -> Self {
        BuildError::Gltf(value)
    }
}

impl From<RdbErr> for BuildError {
    fn from(value: RdbErr) -> Self {
        BuildError::Rdb(value)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rand::{Rng, distributions::Alphanumeric};
    use std::io::Read;

    #[test]
    fn builds_sample_database() {
        let tmp_root = temp_dir();
        fs::create_dir_all(tmp_root.join("sample_pre/imagery")).unwrap();
        fs::create_dir_all(tmp_root.join("sample_pre/gltf")).unwrap();

        copy_fixture(
            "sample/sample_pre/imagery/tulips.png",
            tmp_root.join("sample_pre/imagery/tulips.png"),
        );
        copy_fixture(
            "sample/sample_pre/imagery/peppers.png",
            tmp_root.join("sample_pre/imagery/peppers.png"),
        );
        copy_fixture(
            "sample/sample_pre/gltf/quad.gltf",
            tmp_root.join("sample_pre/gltf/quad.gltf"),
        );

        let build_spec = BuildSpec {
            output: OutputSpec {
                directory: PathBuf::from("../db"),
                layout_file: PathBuf::from("layout.json"),
                layout: DatabaseLayoutFile {
                    geometry: "geometry.rdb".into(),
                    imagery: "imagery.rdb".into(),
                    models: "models.json".into(),
                },
            },
            imagery: vec![
                ImageEntry {
                    entry: "imagery/tulips".into(),
                    file: PathBuf::from("imagery/tulips.png"),
                    layers: 1,
                    format: dashi::Format::RGBA8,
                    mip_levels: 1,
                },
                ImageEntry {
                    entry: "imagery/peppers".into(),
                    file: PathBuf::from("imagery/peppers.png"),
                    layers: 1,
                    format: dashi::Format::RGBA8,
                    mip_levels: 1,
                },
            ],
            geometry: vec![GeometryEntry {
                entry: "geometry/quad".into(),
                file: PathBuf::from("gltf/quad.gltf"),
                mesh: Some("Quad".into()),
                primitive: Some(0),
            }],
            models: vec![ModelEntry {
                name: "quad".into(),
                geometry: "geometry/quad".into(),
                textures: vec!["imagery/tulips".into()],
            }],
        };

        let build_path = tmp_root.join("sample_pre/norenbuild.json");
        let file = File::create(&build_path).unwrap();
        serde_json::to_writer_pretty(file, &build_spec).unwrap();

        run_from_path(&build_path).unwrap();

        let output_dir = tmp_root.join("db");
        assert!(output_dir.join("geometry.rdb").exists());
        assert!(output_dir.join("imagery.rdb").exists());
        assert!(output_dir.join("models.json").exists());
        assert!(output_dir.join("layout.json").exists());

        let mut layout_text = String::new();
        File::open(output_dir.join("layout.json"))
            .unwrap()
            .read_to_string(&mut layout_text)
            .unwrap();
        let layout: DatabaseLayoutFile = serde_json::from_str(&layout_text).unwrap();
        assert_eq!(layout.geometry, "geometry.rdb");
        assert_eq!(layout.imagery, "imagery.rdb");
        assert_eq!(layout.models, "models.json");

        let mut geom = RDBFile::load(output_dir.join("geometry.rdb")).unwrap();
        let host_geom = geom.fetch::<HostGeometry>("geometry/quad").unwrap();
        assert_eq!(host_geom.vertices.len(), 4);
        assert_eq!(host_geom.indices.as_ref().map(|i| i.len()), Some(6));

        let mut images = RDBFile::load(output_dir.join("imagery.rdb")).unwrap();
        let tulips = images.fetch::<HostImage>("imagery/tulips").unwrap();
        assert_eq!(tulips.info().dim[2], 1);
        assert_eq!(tulips.info().layers, 1);
        assert_eq!(tulips.info().format, dashi::Format::RGBA8);
    }

    fn temp_dir() -> PathBuf {
        let mut rng = rand::thread_rng();
        let id: String = (0..12).map(|_| rng.sample(Alphanumeric) as char).collect();
        let dir = std::env::temp_dir().join(format!("noren_dbgen_{id}"));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn copy_fixture(src: &str, dst: PathBuf) {
        if let Some(parent) = dst.parent() {
            fs::create_dir_all(parent).unwrap();
        }
        fs::copy(src, dst).unwrap();
    }
}

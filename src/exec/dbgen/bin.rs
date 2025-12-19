use std::{
    collections::{HashMap, HashSet},
    f32::consts::PI,
    fs::{self, File},
    io::BufReader,
    path::{Path, PathBuf},
    str::FromStr,
};

use bento::{
    BentoError, Compiler as BentoCompiler, OptimizationLevel, Request as BentoRequest, ShaderLang,
};
use gltf::animation::util::ReadOutputs;
use image::DynamicImage;
use noren::{
    DatabaseLayoutFile, NorenError, RDBFile, RdbErr,
    parsing::{
        MaterialLayout, MaterialLayoutFile, MaterialTextureLookups, MeshLayout, MeshLayoutFile,
        ModelLayout, ModelLayoutFile, TextureLayout, TextureLayoutFile,
    },
    rdb::{
        AnimationChannel, AnimationClip, AnimationInterpolation, AnimationOutput, AnimationSampler,
        AnimationTargetPath, AudioClip, AudioFormat, GeometryLayer, HostGeometry, HostImage,
        ImageInfo, Joint, ShaderModule, Skeleton, primitives::Vertex,
    },
    validate_database_layout,
};
use serde::{Deserialize, Serialize};

const DEFAULT_IMAGE_ENTRY: &str = "imagery/default";
const DEFAULT_TEXTURE_ENTRY: &str = "texture/default";
const DEFAULT_MATERIAL_ENTRY: &str = "material/default";
const DEFAULT_GEOMETRY_ENTRIES: [&str; 6] = [
    "geometry/sphere",
    "geometry/cube",
    "geometry/quad",
    "geometry/plane",
    "geometry/cylinder",
    "geometry/cone",
];

#[derive(Clone, Default)]
struct Logger {
    verbose: bool,
    sink: Option<std::sync::Arc<std::sync::Mutex<Vec<String>>>>,
}

impl Logger {
    fn stderr(verbose: bool) -> Self {
        Self {
            verbose,
            sink: None,
        }
    }

    #[cfg(test)]
    fn with_sink(verbose: bool, sink: std::sync::Arc<std::sync::Mutex<Vec<String>>>) -> Self {
        Self {
            verbose,
            sink: Some(sink),
        }
    }

    fn log(&self, message: impl AsRef<str>) {
        if self.verbose {
            if let Some(sink) = &self.sink {
                if let Ok(mut guard) = sink.lock() {
                    guard.push(message.as_ref().to_string());
                    return;
                }
            }

            eprintln!("{}", message.as_ref());
        }
    }
}

fn main() {
    let mut args = std::env::args();
    let program = args.next().unwrap_or_else(|| "dbgen".to_string());

    let cli = match parse_command(&program, args) {
        Ok(cmd) => cmd,
        Err(err) => {
            eprintln!("{err}");
            print_usage(&program);
            std::process::exit(1);
        }
    };

    let logger = Logger::stderr(cli.verbose);

    let result = match cli.command {
        Command::Build { append, spec } => {
            run_from_path(&spec, append, &logger, cli.write_binaries)
        }
        Command::Validate(args) => run_validation(&args, &logger),
        Command::AppendGeometry(args) => append_geometry(&args, &logger, cli.write_binaries),
        Command::AppendSkeleton(args) => append_skeleton(&args, &logger, cli.write_binaries),
        Command::AppendAnimation(args) => append_animation(&args, &logger, cli.write_binaries),
        Command::AppendImagery(args) => append_imagery(&args, &logger, cli.write_binaries),
        Command::AppendAudio(args) => append_audio(&args, &logger, cli.write_binaries),
        Command::AppendShader(args) => append_shader(&args, &logger, cli.write_binaries),
    };

    if let Err(err) = result {
        eprintln!("error: {err}");
        std::process::exit(1);
    }
}

#[derive(Debug)]
struct Cli {
    command: Command,
    verbose: bool,
    write_binaries: bool,
}

fn parse_command(program: &str, args: impl Iterator<Item = String>) -> Result<Cli, String> {
    let mut verbose = false;
    let mut write_binaries = true;
    let mut args = args.peekable();

    while let Some(arg) = args.next() {
        match arg.as_str() {
            "-h" | "--help" => {
                print_usage(program);
                std::process::exit(0);
            }
            "-v" | "--verbose" => {
                verbose = true;
            }
            "--layouts-only" => {
                write_binaries = false;
            }
            other if other.starts_with('-') => {
                return Err(format!("unexpected flag: {other}"));
            }
            other => {
                let command = match other {
                    "--append" => {
                        let Some(spec) = args.next() else {
                            return Err("--append requires a build specification path".into());
                        };
                        Command::Build {
                            append: true,
                            spec: PathBuf::from(spec),
                        }
                    }
                    "validate" => parse_validate_command(args)?,
                    "append" => parse_append_command(args)?,
                    path => Command::Build {
                        append: false,
                        spec: PathBuf::from(path),
                    },
                };

                return Ok(Cli {
                    command,
                    verbose,
                    write_binaries,
                });
            }
        }
    }

    Err(format!("missing arguments\n\nSee '{program} --help'"))
}

fn parse_validate_command(mut args: impl Iterator<Item = String>) -> Result<Command, String> {
    let mut spec: Option<PathBuf> = None;
    let mut base: Option<PathBuf> = None;

    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--base" => {
                base = Some(PathBuf::from(next_value("--base", &mut args)?));
            }
            other => {
                if spec.is_none() {
                    spec = Some(PathBuf::from(other));
                } else {
                    return Err(format!("unexpected argument to validate: {other}"));
                }
            }
        }
    }

    let spec = spec.ok_or_else(|| "validate requires a database layout file".to_string())?;
    Ok(Command::Validate(ValidateArgs { spec, base }))
}

fn parse_append_command(mut args: impl Iterator<Item = String>) -> Result<Command, String> {
    let Some(kind) = args.next() else {
        return Err("append requires a resource type (geometry, imagery, audio, shader)".into());
    };

    match kind.as_str() {
        "geometry" => parse_geometry_append(args).map(Command::AppendGeometry),
        "skeleton" => parse_skeleton_append(args).map(Command::AppendSkeleton),
        "animation" => parse_animation_append(args).map(Command::AppendAnimation),
        "imagery" => parse_imagery_append(args).map(Command::AppendImagery),
        "audio" => parse_audio_append(args).map(Command::AppendAudio),
        "shader" => parse_shader_append(args).map(Command::AppendShader),
        other => Err(format!("unknown append resource type: {other}")),
    }
}

fn parse_geometry_append(
    mut args: impl Iterator<Item = String>,
) -> Result<GeometryAppendArgs, String> {
    let mut rdb: Option<PathBuf> = None;
    let mut entry = None;
    let mut file = None;
    let mut mesh = None;
    let mut primitive = None;

    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--rdb" => {
                let value = next_value("--rdb", &mut args)?;
                rdb = Some(PathBuf::from(value));
            }
            "--entry" => {
                entry = Some(next_value("--entry", &mut args)?);
            }
            "--gltf" => {
                file = Some(next_value("--gltf", &mut args)?);
            }
            "--mesh" => {
                mesh = Some(next_value("--mesh", &mut args)?);
            }
            "--primitive" => {
                let value = next_value("--primitive", &mut args)?;
                let parsed = value
                    .parse::<usize>()
                    .map_err(|_| format!("--primitive expects an integer, received '{value}'"))?;
                primitive = Some(parsed);
            }
            other => return Err(format!("unexpected argument to append geometry: {other}")),
        }
    }

    Ok(GeometryAppendArgs {
        rdb: rdb.ok_or_else(|| "--rdb is required".to_string())?,
        entry: GeometryEntry {
            entry: entry.ok_or_else(|| "--entry is required".to_string())?,
            file: PathBuf::from(file.ok_or_else(|| "--gltf is required".to_string())?),
            mesh,
            primitive,
            lods: Vec::new(),
        },
    })
}

fn parse_skeleton_append(
    mut args: impl Iterator<Item = String>,
) -> Result<SkeletonAppendArgs, String> {
    let mut rdb: Option<PathBuf> = None;
    let mut entry = None;
    let mut file = None;
    let mut skin = None;

    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--rdb" => {
                let value = next_value("--rdb", &mut args)?;
                rdb = Some(PathBuf::from(value));
            }
            "--entry" => {
                entry = Some(next_value("--entry", &mut args)?);
            }
            "--gltf" => {
                file = Some(next_value("--gltf", &mut args)?);
            }
            "--skin" => {
                skin = Some(next_value("--skin", &mut args)?);
            }
            other => return Err(format!("unexpected argument to append skeleton: {other}")),
        }
    }

    Ok(SkeletonAppendArgs {
        rdb: rdb.ok_or_else(|| "--rdb is required".to_string())?,
        entry: SkeletonEntry {
            entry: entry.ok_or_else(|| "--entry is required".to_string())?,
            file: PathBuf::from(file.ok_or_else(|| "--gltf is required".to_string())?),
            skin,
        },
    })
}

fn parse_animation_append(
    mut args: impl Iterator<Item = String>,
) -> Result<AnimationAppendArgs, String> {
    let mut rdb: Option<PathBuf> = None;
    let mut entry = None;
    let mut file = None;
    let mut animation = None;

    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--rdb" => {
                let value = next_value("--rdb", &mut args)?;
                rdb = Some(PathBuf::from(value));
            }
            "--entry" => {
                entry = Some(next_value("--entry", &mut args)?);
            }
            "--gltf" => {
                file = Some(next_value("--gltf", &mut args)?);
            }
            "--animation" => {
                animation = Some(next_value("--animation", &mut args)?);
            }
            other => return Err(format!("unexpected argument to append animation: {other}")),
        }
    }

    Ok(AnimationAppendArgs {
        rdb: rdb.ok_or_else(|| "--rdb is required".to_string())?,
        entry: AnimationEntry {
            entry: entry.ok_or_else(|| "--entry is required".to_string())?,
            file: PathBuf::from(file.ok_or_else(|| "--gltf is required".to_string())?),
            animation,
        },
    })
}

fn parse_imagery_append(mut args: impl Iterator<Item = String>) -> Result<ImageAppendArgs, String> {
    let mut rdb: Option<PathBuf> = None;
    let mut entry = None;
    let mut file = None;
    let mut layers = None;
    let mut format = None;
    let mut mip_levels = None;

    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--rdb" => {
                let value = next_value("--rdb", &mut args)?;
                rdb = Some(PathBuf::from(value));
            }
            "--entry" => {
                entry = Some(next_value("--entry", &mut args)?);
            }
            "--image" => {
                file = Some(next_value("--image", &mut args)?);
            }
            "--layers" => {
                let value = next_value("--layers", &mut args)?;
                let parsed = value
                    .parse::<u32>()
                    .map_err(|_| format!("--layers expects an integer, received '{value}'"))?;
                layers = Some(parsed);
            }
            "--format" => {
                let value = next_value("--format", &mut args)?;
                let parsed = parse_image_format(&value)
                    .ok_or_else(|| format!("unknown image format '{value}'"))?;
                format = Some(parsed);
            }
            "--mip-levels" => {
                let value = next_value("--mip-levels", &mut args)?;
                let parsed = value
                    .parse::<u32>()
                    .map_err(|_| format!("--mip-levels expects an integer, received '{value}'"))?;
                mip_levels = Some(parsed);
            }
            other => return Err(format!("unexpected argument to append imagery: {other}")),
        }
    }

    Ok(ImageAppendArgs {
        rdb: rdb.ok_or_else(|| "--rdb is required".to_string())?,
        entry: ImageEntry {
            entry: entry.ok_or_else(|| "--entry is required".to_string())?,
            file: PathBuf::from(file.ok_or_else(|| "--image is required".to_string())?),
            layers: layers.unwrap_or_else(default_layers),
            format: format.unwrap_or_else(default_format),
            mip_levels: mip_levels.unwrap_or_else(default_mip_levels),
        },
    })
}

fn parse_audio_append(mut args: impl Iterator<Item = String>) -> Result<AudioAppendArgs, String> {
    let mut rdb: Option<PathBuf> = None;
    let mut entry = None;
    let mut file = None;
    let mut format = None;

    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--rdb" => {
                let value = next_value("--rdb", &mut args)?;
                rdb = Some(PathBuf::from(value));
            }
            "--entry" => {
                entry = Some(next_value("--entry", &mut args)?);
            }
            "--audio" => {
                file = Some(next_value("--audio", &mut args)?);
            }
            "--format" => {
                let value = next_value("--format", &mut args)?;
                format = Some(
                    parse_audio_format(&value)
                        .ok_or_else(|| format!("unknown audio format '{value}'"))?,
                );
            }
            other => return Err(format!("unexpected argument to append audio: {other}")),
        }
    }

    Ok(AudioAppendArgs {
        rdb: rdb.ok_or_else(|| "--rdb is required".to_string())?,
        entry: AudioEntry {
            entry: entry.ok_or_else(|| "--entry is required".to_string())?,
            file: PathBuf::from(file.ok_or_else(|| "--audio is required".to_string())?),
            format,
        },
    })
}

fn parse_shader_append(mut args: impl Iterator<Item = String>) -> Result<ShaderAppendArgs, String> {
    let mut rdb: Option<PathBuf> = None;
    let mut entry = None;
    let mut file = None;
    let mut stage = None;

    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--rdb" => {
                let value = next_value("--rdb", &mut args)?;
                rdb = Some(PathBuf::from(value));
            }
            "--entry" => {
                entry = Some(next_value("--entry", &mut args)?);
            }
            "--shader" => {
                file = Some(next_value("--shader", &mut args)?);
            }
            "--stage" => {
                let value = next_value("--stage", &mut args)?;
                stage = Some(
                    ShaderStageKind::from_str(&value)
                        .map_err(|_| format!("unknown shader stage '{value}'"))?,
                );
            }
            other => return Err(format!("unexpected argument to append shader: {other}")),
        }
    }

    Ok(ShaderAppendArgs {
        rdb: rdb.ok_or_else(|| "--rdb is required".to_string())?,
        entry: ShaderEntry {
            entry: entry.ok_or_else(|| "--entry is required".to_string())?,
            stage: stage.ok_or_else(|| "--stage is required".to_string())?,
            file: PathBuf::from(file.ok_or_else(|| "--shader is required".to_string())?),
        },
    })
}

fn next_value(flag: &str, args: &mut impl Iterator<Item = String>) -> Result<String, String> {
    args.next()
        .ok_or_else(|| format!("{flag} requires a value"))
}

#[derive(Debug)]
enum Command {
    Build { append: bool, spec: PathBuf },
    Validate(ValidateArgs),
    AppendGeometry(GeometryAppendArgs),
    AppendSkeleton(SkeletonAppendArgs),
    AppendAnimation(AnimationAppendArgs),
    AppendImagery(ImageAppendArgs),
    AppendAudio(AudioAppendArgs),
    AppendShader(ShaderAppendArgs),
}

#[derive(Debug)]
struct ValidateArgs {
    spec: PathBuf,
    base: Option<PathBuf>,
}

#[derive(Debug)]
struct GeometryAppendArgs {
    rdb: PathBuf,
    entry: GeometryEntry,
}

#[derive(Debug)]
struct SkeletonAppendArgs {
    rdb: PathBuf,
    entry: SkeletonEntry,
}

#[derive(Debug)]
struct AnimationAppendArgs {
    rdb: PathBuf,
    entry: AnimationEntry,
}

#[derive(Debug)]
struct ImageAppendArgs {
    rdb: PathBuf,
    entry: ImageEntry,
}

#[derive(Debug)]
struct AudioAppendArgs {
    rdb: PathBuf,
    entry: AudioEntry,
}

#[derive(Debug)]
struct ShaderAppendArgs {
    rdb: PathBuf,
    entry: ShaderEntry,
}

fn run_from_path(
    input: &Path,
    append: bool,
    logger: &Logger,
    write_binaries: bool,
) -> Result<(), BuildError> {
    logger.log(format!("building from spec: {}", input.display()));
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
        audio,
        skeletons,
        animations,
        geometry,
        shaders,
        models,
    } = spec;

    let output_dir = resolve_path(&base_dir, &output.directory);
    fs::create_dir_all(&output_dir)?;

    let geometry_path = resolve_string_path(&output_dir, &output.layout.geometry);
    let imagery_path = resolve_string_path(&output_dir, &output.layout.imagery);
    let audio_path = resolve_string_path(&output_dir, &output.layout.audio);
    let skeletons_path = resolve_string_path(&output_dir, &output.layout.skeletons);
    let animations_path = resolve_string_path(&output_dir, &output.layout.animations);
    let textures_path = resolve_string_path(&output_dir, &output.layout.textures);
    let materials_path = resolve_string_path(&output_dir, &output.layout.materials);
    let meshes_path = resolve_string_path(&output_dir, &output.layout.meshes);
    let models_path = resolve_string_path(&output_dir, &output.layout.models);
    let shaders_path = resolve_string_path(&output_dir, &output.layout.shaders);
    let layout_path = resolve_path(&output_dir, &output.layout_file);

    build_geometry(
        &base_dir,
        &geometry_path,
        &geometry,
        append,
        write_binaries,
        logger,
    )?;
    build_imagery(
        &base_dir,
        &imagery_path,
        &imagery,
        append,
        write_binaries,
        logger,
    )?;
    build_audio(
        &base_dir,
        &audio_path,
        &audio,
        append,
        write_binaries,
        logger,
    )?;
    build_skeletons(
        &base_dir,
        &skeletons_path,
        &skeletons,
        append,
        write_binaries,
        logger,
    )?;
    build_animations(
        &base_dir,
        &animations_path,
        &animations,
        append,
        write_binaries,
        logger,
    )?;
    build_shaders(
        &base_dir,
        &shaders_path,
        &shaders,
        append,
        write_binaries,
        logger,
    )?;

    let model_layouts = build_model_layout(&models);

    if let Some(parent) = materials_path.parent() {
        fs::create_dir_all(parent)?;
    }
    let materials_file = File::create(&materials_path)?;
    serde_json::to_writer_pretty(materials_file, &model_layouts.materials)?;

    if let Some(parent) = textures_path.parent() {
        fs::create_dir_all(parent)?;
    }
    let textures_file = File::create(&textures_path)?;
    serde_json::to_writer_pretty(textures_file, &model_layouts.textures)?;

    if let Some(parent) = meshes_path.parent() {
        fs::create_dir_all(parent)?;
    }
    let meshes_file = File::create(&meshes_path)?;
    serde_json::to_writer_pretty(meshes_file, &model_layouts.meshes)?;

    if let Some(parent) = models_path.parent() {
        fs::create_dir_all(parent)?;
    }
    let models_file = File::create(&models_path)?;
    serde_json::to_writer_pretty(models_file, &model_layouts.models)?;

    if let Some(parent) = layout_path.parent() {
        fs::create_dir_all(parent)?;
    }
    let layout_file = File::create(&layout_path)?;
    serde_json::to_writer_pretty(layout_file, &output.layout)?;

    Ok(())
}

fn run_validation(args: &ValidateArgs, logger: &Logger) -> Result<(), BuildError> {
    let base_dir = args
        .base
        .clone()
        .or_else(|| args.spec.parent().map(PathBuf::from))
        .unwrap_or_else(|| PathBuf::from("."));

    let base_str = base_dir
        .to_str()
        .ok_or_else(|| BuildError::message("base directory is not valid UTF-8"))?;
    let spec_str = args
        .spec
        .to_str()
        .ok_or_else(|| BuildError::message("layout path is not valid UTF-8"))?;

    logger.log(format!(
        "validating layout {spec_str} against base {base_str}"
    ));
    validate_database_layout(base_str, Some(spec_str)).map_err(BuildError::from)
}

fn build_geometry(
    base_dir: &Path,
    output: &Path,
    entries: &[GeometryEntry],
    append: bool,
    write_binaries: bool,
    logger: &Logger,
) -> Result<(), BuildError> {
    if let Some(parent) = output.parent() {
        fs::create_dir_all(parent)?;
    }

    let mut rdb = if write_binaries || append {
        load_rdb(output, append)?
    } else {
        RDBFile::new()
    };

    let mut existing_entries: HashSet<String> =
        rdb.entries().into_iter().map(|meta| meta.name).collect();

    for entry in entries {
        logger.log(format!(
            "geometry: loading {} from {}",
            entry.entry,
            resolve_path(base_dir, &entry.file).display()
        ));
        let host = load_geometry(base_dir, entry)?;
        rdb.add(&entry.entry, &host).map_err(BuildError::from)?;
        existing_entries.insert(entry.entry.clone());
    }

    inject_default_geometry(&mut rdb, &mut existing_entries, logger)?;

    if write_binaries {
        logger.log(format!("geometry: writing {}", output.display()));
        rdb.save(output).map_err(BuildError::from)?;
    } else {
        logger.log("geometry: skipping binary output (--layouts-only)");
    }
    Ok(())
}

fn append_geometry(
    args: &GeometryAppendArgs,
    logger: &Logger,
    write_binaries: bool,
) -> Result<(), BuildError> {
    if let Some(parent) = args.rdb.parent() {
        fs::create_dir_all(parent)?;
    }

    let mut rdb = if write_binaries {
        load_rdb(&args.rdb, true)?
    } else {
        RDBFile::new()
    };
    logger.log(format!("append geometry: {}", args.entry.entry));
    let geometry = load_geometry(Path::new("."), &args.entry)?;
    let entry_name = args.entry.entry.clone();
    rdb.add(&entry_name, &geometry).map_err(BuildError::from)?;
    if write_binaries {
        logger.log(format!("append geometry: writing {}", args.rdb.display()));
        rdb.save(&args.rdb).map_err(BuildError::from)?;
    } else {
        logger.log("append geometry: skipping binary output (--layouts-only)");
    }
    Ok(())
}

fn load_geometry(base_dir: &Path, entry: &GeometryEntry) -> Result<HostGeometry, BuildError> {
    let base = load_geometry_layer(
        base_dir,
        &entry.file,
        entry.mesh.as_deref(),
        entry.primitive,
    )?;

    let lods = entry
        .lods
        .iter()
        .map(|lod| GeometryLoadSource::from_entries(entry, lod))
        .map(|source| load_geometry_layer(base_dir, source.file, source.mesh, source.primitive))
        .collect::<Result<Vec<_>, _>>()?;

    Ok(HostGeometry {
        vertices: base.vertices,
        indices: base.indices,
        lods,
    })
}

struct GeometryLoadSource<'a> {
    file: &'a PathBuf,
    mesh: Option<&'a str>,
    primitive: Option<usize>,
}

impl<'a> GeometryLoadSource<'a> {
    fn from_entries(base: &'a GeometryEntry, lod: &'a GeometryLodEntry) -> Self {
        Self {
            file: lod.file.as_ref().unwrap_or(&base.file),
            mesh: lod.mesh.as_deref().or_else(|| base.mesh.as_deref()),
            primitive: lod.primitive.or(base.primitive),
        }
    }
}

fn load_geometry_layer(
    base_dir: &Path,
    file: &Path,
    mesh_name: Option<&str>,
    primitive_index: Option<usize>,
) -> Result<GeometryLayer, BuildError> {
    let path = resolve_path(base_dir, file);
    let (doc, buffers, _) = gltf::import(path)?;

    let mesh = if let Some(mesh_name) = mesh_name {
        doc.meshes()
            .find(|m| m.name().map(|n| n == mesh_name).unwrap_or(false))
            .ok_or_else(|| BuildError::message(format!("mesh '{}' not found", mesh_name)))?
    } else {
        doc.meshes()
            .next()
            .ok_or_else(|| BuildError::message("geometry file did not contain any meshes"))?
    };

    let primitive_index = primitive_index.unwrap_or(0);
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

    Ok(GeometryLayer { vertices, indices })
}

fn inject_default_geometry(
    rdb: &mut RDBFile,
    existing_entries: &mut HashSet<String>,
    logger: &Logger,
) -> Result<(), BuildError> {
    for (name, geometry) in default_primitives() {
        if existing_entries.insert(name.clone()) {
            logger.log(format!("geometry: injecting {name}"));
            rdb.add(&name, &geometry).map_err(BuildError::from)?;
        }
    }

    Ok(())
}

fn default_primitives() -> Vec<(String, HostGeometry)> {
    let [sphere, cube, quad, plane, cylinder, cone] = DEFAULT_GEOMETRY_ENTRIES;

    vec![
        (sphere.into(), make_sphere_geometry(0.5, 32, 16)),
        (cube.into(), make_cube_geometry(0.5)),
        (quad.into(), make_quad_geometry()),
        (plane.into(), make_plane_geometry()),
        (cylinder.into(), make_cylinder_geometry(0.5, 1.0, 32)),
        (cone.into(), make_cone_geometry(0.5, 1.0, 32)),
    ]
}

fn default_image() -> HostImage {
    let info = ImageInfo {
        name: DEFAULT_IMAGE_ENTRY.into(),
        dim: [1, 1, 1],
        layers: 1,
        format: dashi::Format::RGBA8,
        mip_levels: 1,
    };

    HostImage::new(info, vec![255, 255, 255, 255])
}

fn inject_default_imagery(rdb: &mut RDBFile, logger: &Logger) -> Result<(), BuildError> {
    let has_default = rdb
        .entries()
        .iter()
        .any(|meta| meta.name == DEFAULT_IMAGE_ENTRY);

    if !has_default {
        logger.log(format!("imagery: injecting {DEFAULT_IMAGE_ENTRY}"));
        rdb.add(DEFAULT_IMAGE_ENTRY, &default_image())
            .map_err(BuildError::from)?;
    }

    Ok(())
}

fn make_vertex(position: [f32; 3], normal: [f32; 3], uv: [f32; 2]) -> Vertex {
    Vertex {
        position,
        normal,
        tangent: [1.0, 0.0, 0.0, 1.0],
        uv,
        color: [1.0, 1.0, 1.0, 1.0],
    }
}

fn make_quad_geometry() -> HostGeometry {
    let vertices = vec![
        make_vertex([-0.5, -0.5, 0.0], [0.0, 0.0, 1.0], [0.0, 0.0]),
        make_vertex([0.5, -0.5, 0.0], [0.0, 0.0, 1.0], [1.0, 0.0]),
        make_vertex([0.5, 0.5, 0.0], [0.0, 0.0, 1.0], [1.0, 1.0]),
        make_vertex([-0.5, 0.5, 0.0], [0.0, 0.0, 1.0], [0.0, 1.0]),
    ];

    let indices = vec![0, 1, 2, 2, 3, 0];

    HostGeometry {
        vertices,
        indices: Some(indices),
        ..Default::default()
    }
}

fn make_plane_geometry() -> HostGeometry {
    let vertices = vec![
        make_vertex([-0.5, 0.0, -0.5], [0.0, 1.0, 0.0], [0.0, 0.0]),
        make_vertex([0.5, 0.0, -0.5], [0.0, 1.0, 0.0], [1.0, 0.0]),
        make_vertex([0.5, 0.0, 0.5], [0.0, 1.0, 0.0], [1.0, 1.0]),
        make_vertex([-0.5, 0.0, 0.5], [0.0, 1.0, 0.0], [0.0, 1.0]),
    ];

    let indices = vec![0, 1, 2, 2, 3, 0];

    HostGeometry {
        vertices,
        indices: Some(indices),
        ..Default::default()
    }
}

fn make_cube_geometry(half_extent: f32) -> HostGeometry {
    let positions = [
        (
            [-half_extent, -half_extent, half_extent],
            [0.0, 0.0, 1.0],
            [0.0, 0.0],
        ),
        (
            [half_extent, -half_extent, half_extent],
            [0.0, 0.0, 1.0],
            [1.0, 0.0],
        ),
        (
            [half_extent, half_extent, half_extent],
            [0.0, 0.0, 1.0],
            [1.0, 1.0],
        ),
        (
            [-half_extent, half_extent, half_extent],
            [0.0, 0.0, 1.0],
            [0.0, 1.0],
        ),
        (
            [-half_extent, -half_extent, -half_extent],
            [0.0, 0.0, -1.0],
            [1.0, 0.0],
        ),
        (
            [-half_extent, half_extent, -half_extent],
            [0.0, 0.0, -1.0],
            [1.0, 1.0],
        ),
        (
            [half_extent, half_extent, -half_extent],
            [0.0, 0.0, -1.0],
            [0.0, 1.0],
        ),
        (
            [half_extent, -half_extent, -half_extent],
            [0.0, 0.0, -1.0],
            [0.0, 0.0],
        ),
        (
            [-half_extent, half_extent, -half_extent],
            [0.0, 1.0, 0.0],
            [0.0, 1.0],
        ),
        (
            [-half_extent, half_extent, half_extent],
            [0.0, 1.0, 0.0],
            [0.0, 0.0],
        ),
        (
            [half_extent, half_extent, half_extent],
            [0.0, 1.0, 0.0],
            [1.0, 0.0],
        ),
        (
            [half_extent, half_extent, -half_extent],
            [0.0, 1.0, 0.0],
            [1.0, 1.0],
        ),
        (
            [-half_extent, -half_extent, -half_extent],
            [0.0, -1.0, 0.0],
            [0.0, 0.0],
        ),
        (
            [half_extent, -half_extent, -half_extent],
            [0.0, -1.0, 0.0],
            [1.0, 0.0],
        ),
        (
            [half_extent, -half_extent, half_extent],
            [0.0, -1.0, 0.0],
            [1.0, 1.0],
        ),
        (
            [-half_extent, -half_extent, half_extent],
            [0.0, -1.0, 0.0],
            [0.0, 1.0],
        ),
        (
            [half_extent, -half_extent, -half_extent],
            [1.0, 0.0, 0.0],
            [0.0, 0.0],
        ),
        (
            [half_extent, half_extent, -half_extent],
            [1.0, 0.0, 0.0],
            [0.0, 1.0],
        ),
        (
            [half_extent, half_extent, half_extent],
            [1.0, 0.0, 0.0],
            [1.0, 1.0],
        ),
        (
            [half_extent, -half_extent, half_extent],
            [1.0, 0.0, 0.0],
            [1.0, 0.0],
        ),
        (
            [-half_extent, -half_extent, -half_extent],
            [-1.0, 0.0, 0.0],
            [1.0, 0.0],
        ),
        (
            [-half_extent, -half_extent, half_extent],
            [-1.0, 0.0, 0.0],
            [0.0, 0.0],
        ),
        (
            [-half_extent, half_extent, half_extent],
            [-1.0, 0.0, 0.0],
            [0.0, 1.0],
        ),
        (
            [-half_extent, half_extent, -half_extent],
            [-1.0, 0.0, 0.0],
            [1.0, 1.0],
        ),
    ];

    let vertices = positions
        .iter()
        .copied()
        .map(|(position, normal, uv)| make_vertex(position, normal, uv))
        .collect();

    let indices = vec![
        0, 1, 2, 2, 3, 0, // Front
        4, 5, 6, 6, 7, 4, // Back
        8, 9, 10, 10, 11, 8, // Top
        12, 13, 14, 14, 15, 12, // Bottom
        16, 17, 18, 18, 19, 16, // Right
        20, 21, 22, 22, 23, 20, // Left
    ];

    HostGeometry {
        vertices,
        indices: Some(indices),
        ..Default::default()
    }
}

fn make_sphere_geometry(radius: f32, slices: u32, stacks: u32) -> HostGeometry {
    let mut vertices = Vec::new();
    let mut indices = Vec::new();

    for stack in 0..=stacks {
        let v = stack as f32 / stacks as f32;
        let phi = v * PI;
        let y = radius * phi.cos();
        let ring_radius = radius * phi.sin();

        for slice in 0..=slices {
            let u = slice as f32 / slices as f32;
            let theta = u * PI * 2.0;
            let x = ring_radius * theta.cos();
            let z = ring_radius * theta.sin();
            let normal = if radius != 0.0 {
                let len = (x * x + y * y + z * z).sqrt();
                [x / len, y / len, z / len]
            } else {
                [0.0, 1.0, 0.0]
            };

            vertices.push(make_vertex([x, y, z], normal, [u, 1.0 - v]));
        }
    }

    let ring = slices + 1;
    for stack in 0..stacks {
        for slice in 0..slices {
            let a = stack * ring + slice;
            let b = a + ring;
            let c = b + 1;
            let d = a + 1;

            indices.extend_from_slice(&[a, b, c, c, d, a]);
        }
    }

    HostGeometry {
        vertices,
        indices: Some(indices.into_iter().map(|i| i as u32).collect()),
        ..Default::default()
    }
}

fn make_cylinder_geometry(radius: f32, height: f32, segments: u32) -> HostGeometry {
    let mut vertices = Vec::new();
    let mut indices: Vec<u32> = Vec::new();
    let half_height = height * 0.5;

    for y in [-half_height, half_height] {
        for i in 0..segments {
            let frac = i as f32 / segments as f32;
            let theta = frac * 2.0 * PI;
            let (s, c) = theta.sin_cos();
            let x = c * radius;
            let z = s * radius;
            vertices.push(make_vertex(
                [x, y, z],
                [c, 0.0, s],
                [frac, (y + half_height) / height],
            ));
        }
    }

    for i in 0..segments {
        let next = (i + 1) % segments;
        let top = segments + i;
        let top_next = segments + next;
        indices.extend_from_slice(&[
            i as u32,
            next as u32,
            top_next as u32,
            top_next as u32,
            top as u32,
            i as u32,
        ]);
    }

    let top_center_index = vertices.len() as u32;
    vertices.push(make_vertex(
        [0.0, half_height, 0.0],
        [0.0, 1.0, 0.0],
        [0.5, 0.5],
    ));
    for i in 0..segments {
        let next = (i + 1) % segments;
        let top = segments + i;
        let top_next = segments + next;
        indices.extend_from_slice(&[top_center_index, top_next as u32, top as u32]);
    }

    let bottom_center_index = vertices.len() as u32;
    vertices.push(make_vertex(
        [0.0, -half_height, 0.0],
        [0.0, -1.0, 0.0],
        [0.5, 0.5],
    ));
    for i in 0..segments {
        let next = (i + 1) % segments;
        indices.extend_from_slice(&[bottom_center_index, i as u32, next as u32]);
    }

    HostGeometry {
        vertices,
        indices: Some(indices),
        ..Default::default()
    }
}

fn make_cone_geometry(radius: f32, height: f32, segments: u32) -> HostGeometry {
    let mut vertices = Vec::new();
    let mut indices: Vec<u32> = Vec::new();
    let half_height = height * 0.5;
    let slope = radius / height;

    let apex_index = 0u32;
    vertices.push(make_vertex(
        [0.0, half_height, 0.0],
        [0.0, 1.0, 0.0],
        [0.5, 1.0],
    ));

    for i in 0..segments {
        let frac = i as f32 / segments as f32;
        let theta = frac * 2.0 * PI;
        let (s, c) = theta.sin_cos();
        let x = c * radius;
        let z = s * radius;
        let normal = [c, slope, s];
        let len = (normal[0] * normal[0] + normal[1] * normal[1] + normal[2] * normal[2]).sqrt();
        let normal = [normal[0] / len, normal[1] / len, normal[2] / len];
        vertices.push(make_vertex([x, -half_height, z], normal, [frac, 0.0]));
    }

    for i in 0..segments {
        let next = (i + 1) % segments;
        indices.extend_from_slice(&[apex_index, (i + 1) as u32, (next + 1) as u32]);
    }

    let base_center_index = vertices.len() as u32;
    vertices.push(make_vertex(
        [0.0, -half_height, 0.0],
        [0.0, -1.0, 0.0],
        [0.5, 0.5],
    ));
    for i in 0..segments {
        let next = (i + 1) % segments;
        indices.extend_from_slice(&[base_center_index, (next + 1) as u32, (i + 1) as u32]);
    }

    HostGeometry {
        vertices,
        indices: Some(indices),
        ..Default::default()
    }
}

fn append_imagery(
    args: &ImageAppendArgs,
    logger: &Logger,
    write_binaries: bool,
) -> Result<(), BuildError> {
    if let Some(parent) = args.rdb.parent() {
        fs::create_dir_all(parent)?;
    }

    let mut rdb = if write_binaries {
        load_rdb(&args.rdb, true)?
    } else {
        RDBFile::new()
    };
    logger.log(format!("append imagery: {}", args.entry.entry));
    let image = load_image(Path::new("."), &args.entry)?;
    let entry_name = args.entry.entry.clone();
    rdb.add(&entry_name, &image).map_err(BuildError::from)?;
    inject_default_imagery(&mut rdb, logger)?;
    if write_binaries {
        logger.log(format!("append imagery: writing {}", args.rdb.display()));
        rdb.save(&args.rdb).map_err(BuildError::from)?;
    } else {
        logger.log("append imagery: skipping binary output (--layouts-only)");
    }
    Ok(())
}

fn build_imagery(
    base_dir: &Path,
    output: &Path,
    entries: &[ImageEntry],
    append: bool,
    write_binaries: bool,
    logger: &Logger,
) -> Result<(), BuildError> {
    if let Some(parent) = output.parent() {
        fs::create_dir_all(parent)?;
    }

    let mut rdb = if write_binaries || append {
        load_rdb(output, append)?
    } else {
        RDBFile::new()
    };

    for entry in entries {
        logger.log(format!(
            "imagery: loading {} from {}",
            entry.entry,
            resolve_path(base_dir, &entry.file).display()
        ));
        let image = load_image(base_dir, entry)?;
        rdb.add(&entry.entry, &image).map_err(BuildError::from)?;
    }

    inject_default_imagery(&mut rdb, logger)?;

    if write_binaries {
        logger.log(format!("imagery: writing {}", output.display()));
        rdb.save(output).map_err(BuildError::from)?;
    } else {
        logger.log("imagery: skipping binary output (--layouts-only)");
    }
    Ok(())
}

fn append_audio(
    args: &AudioAppendArgs,
    logger: &Logger,
    write_binaries: bool,
) -> Result<(), BuildError> {
    if let Some(parent) = args.rdb.parent() {
        fs::create_dir_all(parent)?;
    }

    let mut rdb = if write_binaries {
        load_rdb(&args.rdb, true)?
    } else {
        RDBFile::new()
    };

    logger.log(format!("append audio: {}", args.entry.entry));
    let clip = load_audio(Path::new("."), &args.entry)?;
    let entry_name = args.entry.entry.clone();
    rdb.add(&entry_name, &clip).map_err(BuildError::from)?;

    if write_binaries {
        logger.log(format!("append audio: writing {}", args.rdb.display()));
        rdb.save(&args.rdb).map_err(BuildError::from)?;
    } else {
        logger.log("append audio: skipping binary output (--layouts-only)");
    }

    Ok(())
}

fn append_skeleton(
    args: &SkeletonAppendArgs,
    logger: &Logger,
    write_binaries: bool,
) -> Result<(), BuildError> {
    if let Some(parent) = args.rdb.parent() {
        fs::create_dir_all(parent)?;
    }

    let mut rdb = if write_binaries {
        load_rdb(&args.rdb, true)?
    } else {
        RDBFile::new()
    };

    logger.log(format!("append skeleton: {}", args.entry.entry));
    let skeleton = load_skeleton(Path::new("."), &args.entry)?;
    let entry_name = args.entry.entry.clone();
    rdb.add(&entry_name, &skeleton).map_err(BuildError::from)?;

    if write_binaries {
        logger.log(format!("append skeleton: writing {}", args.rdb.display()));
        rdb.save(&args.rdb).map_err(BuildError::from)?;
    } else {
        logger.log("append skeleton: skipping binary output (--layouts-only)");
    }

    Ok(())
}

fn append_animation(
    args: &AnimationAppendArgs,
    logger: &Logger,
    write_binaries: bool,
) -> Result<(), BuildError> {
    if let Some(parent) = args.rdb.parent() {
        fs::create_dir_all(parent)?;
    }

    let mut rdb = if write_binaries {
        load_rdb(&args.rdb, true)?
    } else {
        RDBFile::new()
    };

    logger.log(format!("append animation: {}", args.entry.entry));
    let clip = load_animation(Path::new("."), &args.entry)?;
    let entry_name = args.entry.entry.clone();
    rdb.add(&entry_name, &clip).map_err(BuildError::from)?;

    if write_binaries {
        logger.log(format!("append animation: writing {}", args.rdb.display()));
        rdb.save(&args.rdb).map_err(BuildError::from)?;
    } else {
        logger.log("append animation: skipping binary output (--layouts-only)");
    }

    Ok(())
}

fn build_audio(
    base_dir: &Path,
    output: &Path,
    entries: &[AudioEntry],
    append: bool,
    write_binaries: bool,
    logger: &Logger,
) -> Result<(), BuildError> {
    if let Some(parent) = output.parent() {
        fs::create_dir_all(parent)?;
    }

    let mut rdb = if write_binaries || append {
        load_rdb(output, append)?
    } else {
        RDBFile::new()
    };

    for entry in entries {
        logger.log(format!(
            "audio: loading {} from {}",
            entry.entry,
            resolve_path(base_dir, &entry.file).display()
        ));
        let clip = load_audio(base_dir, entry)?;
        rdb.add(&entry.entry, &clip).map_err(BuildError::from)?;
    }

    if write_binaries {
        logger.log(format!("audio: writing {}", output.display()));
        rdb.save(output).map_err(BuildError::from)?;
    } else {
        logger.log("audio: skipping binary output (--layouts-only)");
    }
    Ok(())
}

fn build_skeletons(
    base_dir: &Path,
    output: &Path,
    entries: &[SkeletonEntry],
    append: bool,
    write_binaries: bool,
    logger: &Logger,
) -> Result<(), BuildError> {
    if let Some(parent) = output.parent() {
        fs::create_dir_all(parent)?;
    }

    let mut rdb = if write_binaries || append {
        load_rdb(output, append)?
    } else {
        RDBFile::new()
    };

    for entry in entries {
        logger.log(format!(
            "skeleton: loading {} from {}",
            entry.entry,
            resolve_path(base_dir, &entry.file).display()
        ));
        let skeleton = load_skeleton(base_dir, entry)?;
        rdb.add(&entry.entry, &skeleton).map_err(BuildError::from)?;
    }

    if write_binaries {
        logger.log(format!("skeleton: writing {}", output.display()));
        rdb.save(output).map_err(BuildError::from)?;
    } else {
        logger.log("skeleton: skipping binary output (--layouts-only)");
    }
    Ok(())
}

fn build_animations(
    base_dir: &Path,
    output: &Path,
    entries: &[AnimationEntry],
    append: bool,
    write_binaries: bool,
    logger: &Logger,
) -> Result<(), BuildError> {
    if let Some(parent) = output.parent() {
        fs::create_dir_all(parent)?;
    }

    let mut rdb = if write_binaries || append {
        load_rdb(output, append)?
    } else {
        RDBFile::new()
    };

    for entry in entries {
        logger.log(format!(
            "animation: loading {} from {}",
            entry.entry,
            resolve_path(base_dir, &entry.file).display()
        ));
        let clip = load_animation(base_dir, entry)?;
        rdb.add(&entry.entry, &clip).map_err(BuildError::from)?;
    }

    if write_binaries {
        logger.log(format!("animation: writing {}", output.display()));
        rdb.save(output).map_err(BuildError::from)?;
    } else {
        logger.log("animation: skipping binary output (--layouts-only)");
    }
    Ok(())
}

fn append_shader(
    args: &ShaderAppendArgs,
    logger: &Logger,
    write_binaries: bool,
) -> Result<(), BuildError> {
    if let Some(parent) = args.rdb.parent() {
        fs::create_dir_all(parent)?;
    }

    let mut rdb = if write_binaries {
        load_rdb(&args.rdb, true)?
    } else {
        RDBFile::new()
    };
    let compiler = BentoCompiler::new()?;
    logger.log(format!("append shader: {}", args.entry.entry));
    let module = compile_shader(&compiler, Path::new("."), &args.entry)?;
    let entry_name = args.entry.entry.clone();
    rdb.add(&entry_name, &module).map_err(BuildError::from)?;
    if write_binaries {
        logger.log(format!("append shader: writing {}", args.rdb.display()));
        rdb.save(&args.rdb).map_err(BuildError::from)?;
    } else {
        logger.log("append shader: skipping binary output (--layouts-only)");
    }
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

fn infer_audio_format(path: &Path, override_format: Option<AudioFormat>) -> AudioFormat {
    if let Some(format) = override_format {
        return format;
    }

    path.extension()
        .and_then(|ext| ext.to_str())
        .and_then(parse_audio_format)
        .unwrap_or_default()
}

fn load_audio(base_dir: &Path, entry: &AudioEntry) -> Result<AudioClip, BuildError> {
    let path = resolve_path(base_dir, &entry.file);
    let data = fs::read(&path)?;
    let format = infer_audio_format(&entry.file, entry.format.clone());

    Ok(AudioClip::new(entry.entry.clone(), format, data))
}

fn load_skeleton(base_dir: &Path, entry: &SkeletonEntry) -> Result<Skeleton, BuildError> {
    let path = resolve_path(base_dir, &entry.file);
    let (doc, buffers, _) = gltf::import(path)?;

    let skin = if let Some(ref skin_name) = entry.skin {
        doc.skins()
            .find(|s| s.name().map(|n| n == skin_name).unwrap_or(false))
            .ok_or_else(|| BuildError::message(format!("skin '{skin_name}' not found")))?
    } else {
        doc.skins()
            .next()
            .ok_or_else(|| BuildError::message("geometry file did not contain any skins"))?
    };

    let joints: Vec<_> = skin.joints().collect();
    if joints.is_empty() {
        return Err(BuildError::message("skin does not contain any joints"));
    }

    let mut node_to_joint: HashMap<usize, usize> = HashMap::new();
    for (idx, joint) in joints.iter().enumerate() {
        node_to_joint.insert(joint.index(), idx);
    }

    let mut parents: Vec<Option<usize>> = vec![None; joints.len()];
    let mut children_per_joint: Vec<Vec<usize>> = vec![Vec::new(); joints.len()];
    for (idx, joint) in joints.iter().enumerate() {
        for child in joint.children() {
            if let Some(child_idx) = node_to_joint.get(&child.index()) {
                children_per_joint[idx].push(*child_idx);
                parents[*child_idx] = Some(idx);
            }
        }
    }

    let reader = skin.reader(|buffer| Some(&buffers[buffer.index()].0[..]));
    let inverse_bind_matrices: Vec<[[f32; 4]; 4]> = reader
        .read_inverse_bind_matrices()
        .map(|iter| iter.collect())
        .unwrap_or_else(|| vec![identity_matrix(); joints.len()]);

    let mut parsed_joints = Vec::new();
    for (idx, joint) in joints.iter().enumerate() {
        let (translation, rotation, scale) = joint.transform().decomposed();
        let inverse_bind_matrix = inverse_bind_matrices
            .get(idx)
            .copied()
            .unwrap_or_else(identity_matrix);

        parsed_joints.push(Joint {
            name: joint.name().map(|n| n.to_string()),
            parent: parents[idx],
            children: children_per_joint[idx].clone(),
            inverse_bind_matrix,
            translation,
            rotation,
            scale,
        });
    }

    let root = skin
        .skeleton()
        .and_then(|node| node_to_joint.get(&node.index()).copied())
        .or_else(|| parents.iter().position(|p| p.is_none()));
    let name = skin
        .name()
        .map(|n| n.to_string())
        .unwrap_or_else(|| entry.entry.clone());

    Ok(Skeleton {
        name,
        joints: parsed_joints,
        root,
        data: Vec::new(),
    })
}

fn load_animation(base_dir: &Path, entry: &AnimationEntry) -> Result<AnimationClip, BuildError> {
    let path = resolve_path(base_dir, &entry.file);
    let (doc, buffers, _) = gltf::import(path)?;

    let animation = if let Some(ref name) = entry.animation {
        doc.animations()
            .find(|anim| anim.name().map(|n| n == name).unwrap_or(false))
            .ok_or_else(|| BuildError::message(format!("animation '{name}' not found")))?
    } else {
        doc.animations()
            .next()
            .ok_or_else(|| BuildError::message("geometry file did not contain any animations"))?
    };

    let sampler_count = animation.samplers().count();
    let mut samplers: Vec<Option<AnimationSampler>> = vec![None; sampler_count];
    let mut channels = Vec::new();

    for channel in animation.channels() {
        let sampler = channel.sampler();
        let sampler_index = sampler.index();

        if samplers[sampler_index].is_none() {
            let reader = channel.reader(|buffer| Some(&buffers[buffer.index()].0[..]));
            let input: Vec<f32> = reader
                .read_inputs()
                .ok_or_else(|| BuildError::message("animation is missing input keyframes"))?
                .collect();

            let output = reader
                .read_outputs()
                .ok_or_else(|| BuildError::message("animation is missing output values"))?;

            let output = match output {
                ReadOutputs::Translations(values) => {
                    AnimationOutput::Translations(values.collect::<Vec<[f32; 3]>>())
                }
                ReadOutputs::Rotations(values) => {
                    AnimationOutput::Rotations(values.into_f32().collect::<Vec<[f32; 4]>>())
                }
                ReadOutputs::Scales(values) => {
                    AnimationOutput::Scales(values.collect::<Vec<[f32; 3]>>())
                }
                ReadOutputs::MorphTargetWeights(values) => {
                    AnimationOutput::Weights(values.into_f32().collect::<Vec<f32>>())
                }
            };

            let interpolation = match sampler.interpolation() {
                gltf::animation::Interpolation::Linear => AnimationInterpolation::Linear,
                gltf::animation::Interpolation::Step => AnimationInterpolation::Step,
                gltf::animation::Interpolation::CubicSpline => AnimationInterpolation::CubicSpline,
            };

            samplers[sampler_index] = Some(AnimationSampler {
                interpolation,
                input,
                output,
            });
        }

        let target = channel.target();
        let target_path = match target.property() {
            gltf::animation::Property::Translation => AnimationTargetPath::Translation,
            gltf::animation::Property::Rotation => AnimationTargetPath::Rotation,
            gltf::animation::Property::Scale => AnimationTargetPath::Scale,
            gltf::animation::Property::MorphTargetWeights => AnimationTargetPath::Weights,
        };

        channels.push(AnimationChannel {
            sampler_index,
            target_node: target.node().index(),
            target_path,
        });
    }

    let samplers: Vec<AnimationSampler> = samplers
        .into_iter()
        .enumerate()
        .map(|(idx, sampler)| {
            sampler.ok_or_else(|| {
                BuildError::message(format!(
                    "animation sampler {idx} was not referenced by a channel"
                ))
            })
        })
        .collect::<Result<_, _>>()?;

    let duration_seconds = samplers
        .iter()
        .flat_map(|sampler| sampler.input.iter().copied())
        .fold(0.0, f32::max);
    let name = animation
        .name()
        .map(|n| n.to_string())
        .unwrap_or_else(|| entry.entry.clone());

    Ok(AnimationClip {
        name,
        duration_seconds,
        samplers,
        channels,
        data: Vec::new(),
    })
}

fn identity_matrix() -> [[f32; 4]; 4] {
    [
        [1.0, 0.0, 0.0, 0.0],
        [0.0, 1.0, 0.0, 0.0],
        [0.0, 0.0, 1.0, 0.0],
        [0.0, 0.0, 0.0, 1.0],
    ]
}

fn to_rgba(image: DynamicImage) -> image::RgbaImage {
    match image {
        DynamicImage::ImageRgba8(img) => img,
        other => other.to_rgba8(),
    }
}

fn build_shaders(
    base_dir: &Path,
    output: &Path,
    entries: &[ShaderEntry],
    append: bool,
    write_binaries: bool,
    logger: &Logger,
) -> Result<(), BuildError> {
    if let Some(parent) = output.parent() {
        fs::create_dir_all(parent)?;
    }

    let mut rdb = if write_binaries || append {
        load_rdb(output, append)?
    } else {
        RDBFile::new()
    };

    let compiler = BentoCompiler::new()?;

    for entry in entries {
        logger.log(format!(
            "shader: compiling {} from {}",
            entry.entry,
            resolve_path(base_dir, &entry.file).display()
        ));
        let module = compile_shader(&compiler, base_dir, entry)?;
        rdb.add(&entry.entry, &module).map_err(BuildError::from)?;
    }

    if write_binaries {
        logger.log(format!("shader: writing {}", output.display()));
        rdb.save(output).map_err(BuildError::from)?;
    } else {
        logger.log("shader: skipping binary output (--layouts-only)");
    }
    Ok(())
}

fn load_rdb(path: &Path, append: bool) -> Result<RDBFile, BuildError> {
    if append && path.exists() {
        let mut rdb = RDBFile::load(path).map_err(BuildError::from)?;
        rdb.unmap();
        Ok(rdb)
    } else {
        Ok(RDBFile::new())
    }
}

fn compile_shader(
    compiler: &BentoCompiler,
    base_dir: &Path,
    entry: &ShaderEntry,
) -> Result<ShaderModule, BuildError> {
    let path = resolve_path(base_dir, &entry.file);
    let request = BentoRequest {
        name: Some(entry.entry.clone()),
        lang: ShaderLang::Glsl,
        stage: entry.stage.to_shader_type()?,
        optimization: OptimizationLevel::Performance,
        debug_symbols: false,
        defines: HashMap::new(),
    };

    let path_str = path
        .to_str()
        .ok_or_else(|| BuildError::message("shader path contains invalid UTF-8"))?;

    let artifact = compiler.compile_from_file(path_str, &request)?;

    Ok(ShaderModule::from_compilation(artifact))
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

fn print_usage(program: &str) {
    eprintln!("Usage:");
    eprintln!("  {program} <staging-build.json>");
    eprintln!("  {program} --append <staging-build.json>");
    eprintln!("  {program} validate <layout.json> [--base <db root>]");
    eprintln!(
        "  {program} append geometry --rdb <geometry.rdb> --entry <name> --gltf <file> [--mesh <name>] [--primitive <index>]"
    );
    eprintln!(
        "  {program} append skeleton --rdb <skeletons.rdb> --entry <name> --gltf <file> [--skin <name>]"
    );
    eprintln!(
        "  {program} append animation --rdb <animations.rdb> --entry <name> --gltf <file> [--animation <name>]"
    );
    eprintln!(
        "  {program} append imagery --rdb <imagery.rdb> --entry <name> --image <file> [--layers <count>] [--mip-levels <count>] [--format <format>]"
    );
    eprintln!(
        "  {program} append audio --rdb <audio.rdb> --entry <name> --audio <file> [--format <format>]"
    );
    eprintln!(
        "  {program} append shader --rdb <shaders.rdb> --entry <name> --stage <stage> --shader <file>"
    );
    eprintln!("");
    eprintln!("Options:");
    eprintln!("  --append        Append new entries to existing RDB files when using a JSON spec");
    eprintln!("  --layouts-only  Build JSON layout assets without writing RDB binaries");
    eprintln!("  -v, --verbose   Emit detailed progress output");
    eprintln!("  -h, --help      Show this help message");
    eprintln!("");
    eprintln!("Formats:");
    eprintln!("  r8uint, r8sint, rgb8, bgra8, rgba8, rgba8unorm, rgba32f, bgra8unorm, d24s8");
    eprintln!("");
    eprintln!("Stages:");
    eprintln!("  vertex, fragment, compute");
}

#[derive(Debug, Deserialize, Serialize)]
struct BuildSpec {
    #[serde(default)]
    output: OutputSpec,
    #[serde(default)]
    imagery: Vec<ImageEntry>,
    #[serde(default)]
    audio: Vec<AudioEntry>,
    #[serde(default)]
    skeletons: Vec<SkeletonEntry>,
    #[serde(default)]
    animations: Vec<AnimationEntry>,
    #[serde(default)]
    geometry: Vec<GeometryEntry>,
    #[serde(default)]
    shaders: Vec<ShaderEntry>,
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
    #[serde(default)]
    lods: Vec<GeometryLodEntry>,
}

#[derive(Debug, Deserialize, Serialize, Default)]
struct GeometryLodEntry {
    #[serde(default)]
    file: Option<PathBuf>,
    #[serde(default)]
    mesh: Option<String>,
    #[serde(default)]
    primitive: Option<usize>,
}

#[derive(Debug, Deserialize, Serialize)]
struct SkeletonEntry {
    entry: String,
    file: PathBuf,
    #[serde(default)]
    skin: Option<String>,
}

#[derive(Debug, Deserialize, Serialize)]
struct AnimationEntry {
    entry: String,
    file: PathBuf,
    #[serde(default)]
    animation: Option<String>,
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

fn parse_image_format(value: &str) -> Option<dashi::Format> {
    match value.to_ascii_lowercase().as_str() {
        "r8uint" => Some(dashi::Format::R8Uint),
        "r8sint" => Some(dashi::Format::R8Sint),
        "rgb8" => Some(dashi::Format::RGB8),
        "bgra8" => Some(dashi::Format::BGRA8),
        "rgba8" => Some(dashi::Format::RGBA8),
        "rgba8unorm" | "rgba8_unorm" => Some(dashi::Format::RGBA8Unorm),
        "rgba32f" | "rgba32_float" | "rgba32float" => Some(dashi::Format::RGBA32F),
        "bgra8unorm" | "bgra8_unorm" => Some(dashi::Format::BGRA8Unorm),
        "d24s8" => Some(dashi::Format::D24S8),
        _ => None,
    }
}

#[derive(Debug, Deserialize, Serialize)]
struct AudioEntry {
    entry: String,
    file: PathBuf,
    #[serde(default)]
    format: Option<AudioFormat>,
}

fn parse_audio_format(value: &str) -> Option<AudioFormat> {
    match value.to_ascii_lowercase().as_str() {
        "ogg" | "oga" => Some(AudioFormat::Ogg),
        "wav" => Some(AudioFormat::Wav),
        "mp3" => Some(AudioFormat::Mp3),
        "flac" => Some(AudioFormat::Flac),
        _ => None,
    }
}

#[derive(Debug, Deserialize, Serialize)]
struct ShaderEntry {
    entry: String,
    stage: ShaderStageKind,
    file: PathBuf,
}

#[derive(Debug, Copy, Clone, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
enum ShaderStageKind {
    Vertex,
    Fragment,
    Geometry,
    TessellationControl,
    TessellationEvaluation,
    Compute,
}

impl ShaderStageKind {
    fn to_shader_type(self) -> Result<dashi::ShaderType, BuildError> {
        match self {
            ShaderStageKind::Vertex => Ok(dashi::ShaderType::Vertex),
            ShaderStageKind::Fragment => Ok(dashi::ShaderType::Fragment),
            ShaderStageKind::Compute => Ok(dashi::ShaderType::Compute),
            other => Err(BuildError::message(format!(
                "shader stage '{other:?}' is not supported by Bento compilation"
            ))),
        }
    }
}

impl FromStr for ShaderStageKind {
    type Err = ();

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_ascii_lowercase().as_str() {
            "vertex" => Ok(Self::Vertex),
            "fragment" | "pixel" => Ok(Self::Fragment),
            "geometry" => Ok(Self::Geometry),
            "tessellation_control" | "tess_control" | "hull" => Ok(Self::TessellationControl),
            "tessellation_evaluation" | "tess_evaluation" | "domain" => {
                Ok(Self::TessellationEvaluation)
            }
            "compute" => Ok(Self::Compute),
            _ => Err(()),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct ModelEntry {
    name: String,
    geometry: String,
    #[serde(default)]
    textures: Vec<String>,
}

struct GeneratedModelLayouts {
    textures: TextureLayoutFile,
    materials: MaterialLayoutFile,
    meshes: MeshLayoutFile,
    models: ModelLayoutFile,
}

fn build_model_layout(entries: &[ModelEntry]) -> GeneratedModelLayouts {
    let mut textures = TextureLayoutFile::default();
    let materials = MaterialLayoutFile::default();
    let mut meshes = MeshLayoutFile::default();
    let mut models = ModelLayoutFile::default();

    for model in entries {
        let model_key = normalize_entry_name(&model.name, "model/", true);
        let mesh_key = normalize_entry_name(&model.name, "mesh/", true);

        let mut mesh_textures = Vec::new();
        for texture in &model.textures {
            let texture_key = normalize_entry_name(texture, "texture/", false);
            mesh_textures.push(texture_key.clone());

            textures
                .textures
                .entry(texture_key.clone())
                .or_insert_with(|| TextureLayout {
                    image: texture.clone(),
                    name: None,
                });
        }

        meshes.meshes.insert(
            mesh_key.clone(),
            MeshLayout {
                name: Some(model.name.clone()),
                geometry: model.geometry.clone(),
                material: None,
                textures: mesh_textures,
            },
        );

        models.models.insert(
            model_key,
            ModelLayout {
                name: Some(model.name.clone()),
                meshes: vec![mesh_key],
            },
        );
    }

    let mut layouts = GeneratedModelLayouts {
        textures,
        materials,
        meshes,
        models,
    };

    ensure_default_assets(&mut layouts);

    layouts
}

fn ensure_default_assets(layouts: &mut GeneratedModelLayouts) {
    layouts
        .textures
        .textures
        .entry(DEFAULT_TEXTURE_ENTRY.into())
        .or_insert(TextureLayout {
            image: DEFAULT_IMAGE_ENTRY.into(),
            name: Some("Default Texture".into()),
        });

    layouts
        .materials
        .materials
        .entry(DEFAULT_MATERIAL_ENTRY.into())
        .or_insert(MaterialLayout {
            name: Some("Default Material".into()),
            render_mask: 0,
            texture_lookups: MaterialTextureLookups {
                base_color: Some(DEFAULT_TEXTURE_ENTRY.into()),
                ..Default::default()
            },
        });

    for geometry in DEFAULT_GEOMETRY_ENTRIES {
        let mesh_name = geometry.trim_start_matches("geometry/");
        let mesh_key = format!("mesh/{mesh_name}");
        let model_key = format!("model/{mesh_name}");

        layouts
            .meshes
            .meshes
            .entry(mesh_key.clone())
            .or_insert(MeshLayout {
                name: Some(mesh_name.to_string()),
                geometry: geometry.to_string(),
                material: Some(DEFAULT_MATERIAL_ENTRY.into()),
                textures: vec![DEFAULT_TEXTURE_ENTRY.into()],
            });

        layouts
            .models
            .models
            .entry(model_key)
            .or_insert(ModelLayout {
                name: Some(mesh_name.to_string()),
                meshes: vec![mesh_key],
            });
    }
}

fn normalize_entry_name(entry: &str, prefix: &str, allow_existing_prefix: bool) -> String {
    if entry.starts_with(prefix) || (allow_existing_prefix && entry.contains('/')) {
        entry.to_string()
    } else {
        format!("{prefix}{entry}")
    }
}

#[derive(Debug)]
enum BuildError {
    Io(std::io::Error),
    Json(serde_json::Error),
    Image(image::ImageError),
    Gltf(gltf::Error),
    Rdb(RdbErr),
    Shader(BentoError),
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
            BuildError::Shader(err) => write!(f, "shader compile error: {err}"),
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

impl From<NorenError> for BuildError {
    fn from(value: NorenError) -> Self {
        BuildError::Message(value.to_string())
    }
}

impl From<BentoError> for BuildError {
    fn from(value: BentoError) -> Self {
        BuildError::Shader(value)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rand::{Rng, distributions::Alphanumeric};
    use std::{
        io::Read,
        sync::{Arc, Mutex},
    };

    #[test]
    fn builds_sample_database() {
        let tmp_root = temp_dir();
        fs::create_dir_all(tmp_root.join("sample_pre/imagery")).unwrap();
        fs::create_dir_all(tmp_root.join("sample_pre/gltf")).unwrap();
        fs::create_dir_all(tmp_root.join("sample_pre/shaders")).unwrap();

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
        copy_fixture(
            "sample/sample_pre/shaders/quad.vert",
            tmp_root.join("sample_pre/shaders/quad.vert"),
        );
        copy_fixture(
            "sample/sample_pre/shaders/quad.frag",
            tmp_root.join("sample_pre/shaders/quad.frag"),
        );

        let build_spec = BuildSpec {
            output: OutputSpec {
                directory: PathBuf::from("../db"),
                layout_file: PathBuf::from("layout.json"),
                layout: DatabaseLayoutFile {
                    geometry: "geometry.rdb".into(),
                    imagery: "imagery.rdb".into(),
                    audio: "audio.rdb".into(),
                    skeletons: "skeletons.rdb".into(),
                    animations: "animations.rdb".into(),
                    materials: "materials.json".into(),
                    textures: "textures.json".into(),
                    meshes: "meshes.json".into(),
                    models: "models.json".into(),
                    shader_layouts: "shaders.json".into(),
                    shaders: "shaders.rdb".into(),
                },
            },
            skeletons: Vec::new(),
            animations: Vec::new(),
            audio: Vec::new(),
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
                lods: Vec::new(),
            }],
            shaders: vec![
                ShaderEntry {
                    entry: "shader/quad.vert".into(),
                    stage: ShaderStageKind::Vertex,
                    file: PathBuf::from("shaders/quad.vert"),
                },
                ShaderEntry {
                    entry: "shader/quad.frag".into(),
                    stage: ShaderStageKind::Fragment,
                    file: PathBuf::from("shaders/quad.frag"),
                },
            ],
            models: vec![ModelEntry {
                name: "quad".into(),
                geometry: "geometry/quad".into(),
                textures: vec!["imagery/tulips".into()],
            }],
        };

        let build_path = tmp_root.join("sample_pre/norenbuild.json");
        let file = File::create(&build_path).unwrap();
        serde_json::to_writer_pretty(file, &build_spec).unwrap();

        let logger = Logger::default();
        run_from_path(&build_path, false, &logger, true).unwrap();

        let output_dir = tmp_root.join("db");
        assert!(output_dir.join("geometry.rdb").exists());
        assert!(output_dir.join("imagery.rdb").exists());
        assert!(output_dir.join("skeletons.rdb").exists());
        assert!(output_dir.join("animations.rdb").exists());
        assert!(output_dir.join("materials.json").exists());
        assert!(output_dir.join("textures.json").exists());
        assert!(output_dir.join("meshes.json").exists());
        assert!(output_dir.join("models.json").exists());
        assert!(output_dir.join("shaders.rdb").exists());
        assert!(output_dir.join("layout.json").exists());

        let model_layout: ModelLayoutFile =
            serde_json::from_reader(File::open(output_dir.join("models.json")).unwrap()).unwrap();
        let mesh_layout: MeshLayoutFile =
            serde_json::from_reader(File::open(output_dir.join("meshes.json")).unwrap()).unwrap();
        let material_layout: MaterialLayoutFile =
            serde_json::from_reader(File::open(output_dir.join("materials.json")).unwrap())
                .unwrap();
        let texture_layout: TextureLayoutFile =
            serde_json::from_reader(File::open(output_dir.join("textures.json")).unwrap()).unwrap();

        let model = model_layout
            .models
            .get("model/quad")
            .expect("model entry exists");
        assert_eq!(model.meshes, vec!["mesh/quad"]);

        let mesh = mesh_layout
            .meshes
            .get("mesh/quad")
            .expect("mesh entry exists");
        assert_eq!(mesh.geometry, "geometry/quad");
        assert_eq!(mesh.textures, vec!["texture/imagery/tulips"]);

        let texture = texture_layout
            .textures
            .get("texture/imagery/tulips")
            .expect("texture entry exists");
        assert_eq!(texture.image, "imagery/tulips");

        let default_material = material_layout
            .materials
            .get(DEFAULT_MATERIAL_ENTRY)
            .expect("default material");
        assert_eq!(
            default_material.texture_lookups.base_color.as_deref(),
            Some(DEFAULT_TEXTURE_ENTRY)
        );

        let default_texture = texture_layout
            .textures
            .get(DEFAULT_TEXTURE_ENTRY)
            .expect("default texture entry");
        assert_eq!(default_texture.image, DEFAULT_IMAGE_ENTRY);

        let sphere_model = model_layout
            .models
            .get("model/sphere")
            .expect("default model");
        assert_eq!(sphere_model.meshes, vec!["mesh/sphere"]);

        let sphere_mesh = mesh_layout.meshes.get("mesh/sphere").expect("default mesh");
        assert_eq!(sphere_mesh.geometry, "geometry/sphere");
        assert_eq!(
            sphere_mesh.material.as_deref(),
            Some(DEFAULT_MATERIAL_ENTRY)
        );
        assert_eq!(
            sphere_mesh.textures,
            vec![DEFAULT_TEXTURE_ENTRY.to_string()]
        );

        let mut layout_text = String::new();
        File::open(output_dir.join("layout.json"))
            .unwrap()
            .read_to_string(&mut layout_text)
            .unwrap();
        let layout: DatabaseLayoutFile = serde_json::from_str(&layout_text).unwrap();
        assert_eq!(layout.geometry, "geometry.rdb");
        assert_eq!(layout.imagery, "imagery.rdb");
        assert_eq!(layout.skeletons, "skeletons.rdb");
        assert_eq!(layout.animations, "animations.rdb");
        assert_eq!(layout.textures, "textures.json");
        assert_eq!(layout.meshes, "meshes.json");
        assert_eq!(layout.models, "models.json");
        assert_eq!(layout.shader_layouts, "shaders.json");
        assert_eq!(layout.shaders, "shaders.rdb");

        let mut geom = RDBFile::load(output_dir.join("geometry.rdb")).unwrap();
        let host_geom = geom.fetch::<HostGeometry>("geometry/quad").unwrap();
        assert_eq!(host_geom.vertices.len(), 4);
        assert_eq!(host_geom.indices.as_ref().map(|i| i.len()), Some(6));

        let mut images = RDBFile::load(output_dir.join("imagery.rdb")).unwrap();
        let tulips = images.fetch::<HostImage>("imagery/tulips").unwrap();
        assert_eq!(tulips.info().dim[2], 1);
        assert_eq!(tulips.info().layers, 1);
        assert_eq!(tulips.info().format, dashi::Format::RGBA8);

        let mut shaders = RDBFile::load(output_dir.join("shaders.rdb")).unwrap();
        let vert = shaders.fetch::<ShaderModule>("shader/quad.vert").unwrap();
        assert!(vert.is_spirv());
        let frag = shaders.fetch::<ShaderModule>("shader/quad.frag").unwrap();
        assert!(frag.is_spirv());
    }

    #[test]
    fn builds_layouts_without_binaries() {
        let tmp_root = temp_dir();
        fs::create_dir_all(tmp_root.join("sample_pre/imagery")).unwrap();
        fs::create_dir_all(tmp_root.join("sample_pre/gltf")).unwrap();

        copy_fixture(
            "sample/sample_pre/imagery/tulips.png",
            tmp_root.join("sample_pre/imagery/tulips.png"),
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
                    audio: "audio.rdb".into(),
                    skeletons: "skeletons.rdb".into(),
                    animations: "animations.rdb".into(),
                    materials: "materials.json".into(),
                    textures: "textures.json".into(),
                    meshes: "meshes.json".into(),
                    models: "models.json".into(),
                    shader_layouts: "shaders.json".into(),
                    shaders: "shaders.rdb".into(),
                },
            },
            skeletons: Vec::new(),
            animations: Vec::new(),
            audio: Vec::new(),
            imagery: vec![ImageEntry {
                entry: "imagery/tulips".into(),
                file: PathBuf::from("imagery/tulips.png"),
                layers: 1,
                format: dashi::Format::RGBA8,
                mip_levels: 1,
            }],
            geometry: vec![GeometryEntry {
                entry: "geometry/quad".into(),
                file: PathBuf::from("gltf/quad.gltf"),
                mesh: Some("Quad".into()),
                primitive: Some(0),
                lods: Vec::new(),
            }],
            shaders: Vec::new(),
            models: vec![ModelEntry {
                name: "quad".into(),
                geometry: "geometry/quad".into(),
                textures: vec!["imagery/tulips".into()],
            }],
        };

        let build_path = tmp_root.join("sample_pre/norenbuild.json");
        let file = File::create(&build_path).unwrap();
        serde_json::to_writer_pretty(file, &build_spec).unwrap();

        let logger = Logger::default();
        run_from_path(&build_path, false, &logger, false).unwrap();

        let output_dir = tmp_root.join("db");
        assert!(!output_dir.join("geometry.rdb").exists());
        assert!(!output_dir.join("imagery.rdb").exists());
        assert!(!output_dir.join("skeletons.rdb").exists());
        assert!(!output_dir.join("animations.rdb").exists());
        assert!(!output_dir.join("shaders.rdb").exists());
        assert!(output_dir.join("materials.json").exists());
        assert!(output_dir.join("textures.json").exists());
        assert!(output_dir.join("meshes.json").exists());
        assert!(output_dir.join("models.json").exists());
        assert!(output_dir.join("layout.json").exists());

        let model_layout: ModelLayoutFile =
            serde_json::from_reader(File::open(output_dir.join("models.json")).unwrap()).unwrap();
        let mesh_layout: MeshLayoutFile =
            serde_json::from_reader(File::open(output_dir.join("meshes.json")).unwrap()).unwrap();
        let material_layout: MaterialLayoutFile =
            serde_json::from_reader(File::open(output_dir.join("materials.json")).unwrap())
                .unwrap();
        let texture_layout: TextureLayoutFile =
            serde_json::from_reader(File::open(output_dir.join("textures.json")).unwrap()).unwrap();

        let model = model_layout
            .models
            .get("model/quad")
            .expect("model entry exists");
        assert_eq!(model.meshes, vec!["mesh/quad"]);

        let mesh = mesh_layout
            .meshes
            .get("mesh/quad")
            .expect("mesh entry exists");
        assert_eq!(mesh.geometry, "geometry/quad");
        assert_eq!(mesh.textures, vec!["texture/imagery/tulips"]);

        let texture = texture_layout
            .textures
            .get("texture/imagery/tulips")
            .expect("texture entry exists");
        assert_eq!(texture.image, "imagery/tulips");

        let default_material = material_layout
            .materials
            .get(DEFAULT_MATERIAL_ENTRY)
            .expect("default material");
        assert_eq!(
            default_material.texture_lookups.base_color.as_deref(),
            Some(DEFAULT_TEXTURE_ENTRY)
        );

        let mut layout_text = String::new();
        File::open(output_dir.join("layout.json"))
            .unwrap()
            .read_to_string(&mut layout_text)
            .unwrap();
        let layout: DatabaseLayoutFile = serde_json::from_str(&layout_text).unwrap();
        assert_eq!(layout.skeletons, "skeletons.rdb");
        assert_eq!(layout.animations, "animations.rdb");
    }

    #[test]
    fn verbose_logger_records_progress() {
        let tmp_root = temp_dir();
        fs::create_dir_all(tmp_root.join("sample_pre/gltf")).unwrap();

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
                    audio: "audio.rdb".into(),
                    skeletons: "skeletons.rdb".into(),
                    animations: "animations.rdb".into(),
                    textures: "textures.json".into(),
                    materials: "materials.json".into(),
                    meshes: "meshes.json".into(),
                    models: "models.json".into(),
                    shader_layouts: "shaders.json".into(),
                    shaders: "shaders.rdb".into(),
                },
            },
            skeletons: Vec::new(),
            animations: Vec::new(),
            audio: Vec::new(),
            imagery: Vec::new(),
            geometry: vec![GeometryEntry {
                entry: "geometry/quad".into(),
                file: PathBuf::from("gltf/quad.gltf"),
                mesh: Some("Quad".into()),
                primitive: Some(0),
                lods: Vec::new(),
            }],
            shaders: Vec::new(),
            models: Vec::new(),
        };

        let build_path = tmp_root.join("sample_pre/norenbuild.json");
        let file = File::create(&build_path).unwrap();
        serde_json::to_writer_pretty(file, &build_spec).unwrap();

        let sink = Arc::new(Mutex::new(Vec::new()));
        let logger = Logger::with_sink(true, sink.clone());
        run_from_path(&build_path, false, &logger, false).unwrap();

        let logs = sink.lock().unwrap();
        assert!(logs.iter().any(|msg| msg.contains("building from spec")));
        assert!(
            logs.iter()
                .any(|msg| msg.contains("geometry: loading geometry/quad"))
        );
        assert!(
            logs.iter()
                .any(|msg| msg.contains("geometry: skipping binary output"))
        );
    }

    #[test]
    fn builds_model_layout_with_prefixed_entries() {
        let layout = build_model_layout(&[ModelEntry {
            name: "sample".into(),
            geometry: "geometry/sample".into(),
            textures: vec!["imagery/sample".into()],
        }]);

        let model = layout.models.models.get("model/sample").expect("model key");
        assert_eq!(model.meshes, vec!["mesh/sample"]);

        let mesh = layout.meshes.meshes.get("mesh/sample").expect("mesh key");
        assert_eq!(mesh.geometry, "geometry/sample");
        assert_eq!(mesh.textures, vec!["texture/imagery/sample"]);

        let texture = layout
            .textures
            .textures
            .get("texture/imagery/sample")
            .expect("texture key");
        assert_eq!(texture.image, "imagery/sample");

        layout
            .materials
            .materials
            .get(DEFAULT_MATERIAL_ENTRY)
            .expect("default material present");

        layout
            .models
            .models
            .get("model/sphere")
            .expect("default model present");
    }

    fn temp_dir() -> PathBuf {
        let mut rng = rand::thread_rng();
        let id: String = (0..12).map(|_| rng.sample(Alphanumeric) as char).collect();
        let dir = std::env::temp_dir().join(format!("dbgen_{id}"));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn appends_to_existing_rdb_files() {
        let tmp_root = temp_dir();
        fs::create_dir_all(tmp_root.join("sample_pre/imagery")).unwrap();
        fs::create_dir_all(tmp_root.join("sample_pre/gltf")).unwrap();

        copy_fixture(
            "sample/sample_pre/imagery/tulips.png",
            tmp_root.join("sample_pre/imagery/tulips.png"),
        );
        copy_fixture(
            "sample/sample_pre/gltf/quad.gltf",
            tmp_root.join("sample_pre/gltf/quad.gltf"),
        );

        let initial_spec = BuildSpec {
            output: OutputSpec {
                directory: PathBuf::from("../db"),
                layout_file: PathBuf::from("layout.json"),
                layout: DatabaseLayoutFile {
                    geometry: "geometry.rdb".into(),
                    imagery: "imagery.rdb".into(),
                    audio: "audio.rdb".into(),
                    skeletons: "skeletons.rdb".into(),
                    animations: "animations.rdb".into(),
                    materials: "materials.json".into(),
                    textures: "textures.json".into(),
                    meshes: "meshes.json".into(),
                    models: "models.json".into(),
                    shader_layouts: "shaders.json".into(),
                    shaders: "shaders.rdb".into(),
                },
            },
            skeletons: Vec::new(),
            animations: Vec::new(),
            audio: Vec::new(),
            imagery: vec![ImageEntry {
                entry: "imagery/tulips".into(),
                file: PathBuf::from("imagery/tulips.png"),
                layers: 1,
                format: dashi::Format::RGBA8,
                mip_levels: 1,
            }],
            geometry: vec![GeometryEntry {
                entry: "geometry/quad".into(),
                file: PathBuf::from("gltf/quad.gltf"),
                mesh: Some("Quad".into()),
                primitive: Some(0),
                lods: Vec::new(),
            }],
            shaders: Vec::new(),
            models: Vec::new(),
        };

        let build_path = tmp_root.join("sample_pre/norenbuild.json");
        let file = File::create(&build_path).unwrap();
        serde_json::to_writer_pretty(file, &initial_spec).unwrap();
        let logger = Logger::default();
        run_from_path(&build_path, false, &logger, true).unwrap();

        let append_spec = BuildSpec {
            output: OutputSpec {
                directory: PathBuf::from("../db"),
                layout_file: PathBuf::from("layout.json"),
                layout: DatabaseLayoutFile {
                    geometry: "geometry.rdb".into(),
                    imagery: "imagery.rdb".into(),
                    audio: "audio.rdb".into(),
                    skeletons: "skeletons.rdb".into(),
                    animations: "animations.rdb".into(),
                    textures: "textures.json".into(),
                    materials: "materials.json".into(),
                    meshes: "meshes.json".into(),
                    models: "models.json".into(),
                    shader_layouts: "shaders.json".into(),
                    shaders: "shaders.rdb".into(),
                },
            },
            skeletons: Vec::new(),
            animations: Vec::new(),
            audio: Vec::new(),
            imagery: vec![],
            geometry: vec![GeometryEntry {
                entry: "geometry/quad_copy".into(),
                file: PathBuf::from("gltf/quad.gltf"),
                mesh: Some("Quad".into()),
                primitive: Some(0),
                lods: Vec::new(),
            }],
            shaders: Vec::new(),
            models: Vec::new(),
        };

        let append_path = tmp_root.join("sample_pre/append.json");
        let file = File::create(&append_path).unwrap();
        serde_json::to_writer_pretty(file, &append_spec).unwrap();
        run_from_path(&append_path, true, &logger, true).unwrap();

        let geometry_path = tmp_root.join("db/geometry.rdb");
        let mut rdb = RDBFile::load(&geometry_path).unwrap();
        let original = rdb.fetch::<HostGeometry>("geometry/quad").unwrap();
        assert_eq!(original.vertices.len(), 4);

        let copy = rdb.fetch::<HostGeometry>("geometry/quad_copy").unwrap();
        assert_eq!(copy.vertices.len(), 4);
    }

    #[test]
    fn appends_geometry_without_spec_file() {
        let tmp_root = temp_dir();
        fs::create_dir_all(tmp_root.join("sample_pre/gltf")).unwrap();

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
                    audio: "audio.rdb".into(),
                    skeletons: "skeletons.rdb".into(),
                    animations: "animations.rdb".into(),
                    textures: "textures.json".into(),
                    materials: "materials.json".into(),
                    meshes: "meshes.json".into(),
                    models: "models.json".into(),
                    shader_layouts: "shaders.json".into(),
                    shaders: "shaders.rdb".into(),
                },
            },
            skeletons: Vec::new(),
            animations: Vec::new(),
            audio: Vec::new(),
            imagery: Vec::new(),
            geometry: vec![GeometryEntry {
                entry: "geometry/original".into(),
                file: PathBuf::from("gltf/quad.gltf"),
                mesh: Some("Quad".into()),
                primitive: Some(0),
                lods: Vec::new(),
            }],
            shaders: Vec::new(),
            models: Vec::new(),
        };

        let build_path = tmp_root.join("sample_pre/norenbuild.json");
        let file = File::create(&build_path).unwrap();
        serde_json::to_writer_pretty(file, &build_spec).unwrap();
        let logger = Logger::default();
        run_from_path(&build_path, false, &logger, true).unwrap();

        let geometry_path = tmp_root.join("db/geometry.rdb");
        append_geometry(
            &GeometryAppendArgs {
                rdb: geometry_path.clone(),
                entry: GeometryEntry {
                    entry: "geometry/appended".into(),
                    file: tmp_root.join("sample_pre/gltf/quad.gltf"),
                    mesh: Some("Quad".into()),
                    primitive: Some(0),
                    lods: Vec::new(),
                },
            },
            &logger,
            true,
        )
        .unwrap();

        let mut rdb = RDBFile::load(&geometry_path).unwrap();
        let original = rdb.fetch::<HostGeometry>("geometry/original").unwrap();
        assert_eq!(original.vertices.len(), 4);
        let appended = rdb.fetch::<HostGeometry>("geometry/appended").unwrap();
        assert_eq!(appended.vertices.len(), 4);
    }

    fn copy_fixture(src: &str, dst: PathBuf) {
        if let Some(parent) = dst.parent() {
            fs::create_dir_all(parent).unwrap();
        }
        fs::copy(src, dst).unwrap();
    }
}

use std::{
    collections::{BTreeMap, HashMap, HashSet},
    fs::{self, File},
    io::{BufReader, Write},
    path::{Path, PathBuf},
    str::FromStr,
};

use crate::{
    DatabaseLayoutFile, NorenError, RDBEntryMeta, RDBFile, RdbErr,
    defaults::{
        DEFAULT_IMAGE_ENTRY, default_fonts, default_image, default_primitives, default_sounds,
        ensure_default_assets,
    },
    parsing::{
        FontBounds, FontGlyph, FontMetrics, MaterialLayoutFile, MeshLayout, MeshLayoutFile,
        ModelLayout, ModelLayoutFile, MsdfFontLayout, MsdfFontLayoutFile, SdfFontLayout,
        SdfFontLayoutFile, TextureAtlasLayoutFile, TextureLayout, TextureLayoutFile,
    },
    rdb::{
        AnimationChannel, AnimationClip, AnimationInterpolation, AnimationOutput, AnimationSampler,
        AnimationTargetPath, AudioClip, AudioFormat, GeometryLayer, HostCubemap, HostFont,
        HostGeometry, HostImage, ImageInfo, Joint, ShaderModule, Skeleton, index_vertices,
        primitives::Vertex,
        terrain::{
            TERRAIN_MUTATION_LAYER_PREFIX, TERRAIN_MUTATION_OP_PREFIX, TerrainChunk,
            TerrainGeneratorDefinition, TerrainMutationLayer, TerrainMutationOp,
            TerrainProjectSettings, TerrainTile, chunk_artifact_entry, chunk_coord_key,
            generator_entry, lod_key, mutation_layer_entry, mutation_op_entry,
            project_settings_entry,
        },
    },
    terrain::build_heightmap_chunk_artifact,
    validate_database_layout,
};
use bento::{
    BentoError, Compiler as BentoCompiler, OptimizationLevel, Request as BentoRequest, ShaderLang,
};
use fontdue::{Font, FontSettings};
use gltf::{animation::util::ReadOutputs, image::Format};
use image::DynamicImage;
use serde::{Deserialize, Serialize};

#[derive(Clone, Default)]
pub struct Logger {
    verbose: bool,
    sink: Option<std::sync::Arc<std::sync::Mutex<Vec<String>>>>,
}

struct FontAtlasOutput {
    images: Vec<(String, HostImage)>,
    msdf_layouts: HashMap<String, MsdfFontLayout>,
    sdf_layouts: HashMap<String, SdfFontLayout>,
}

#[derive(Clone)]
struct RasterizedGlyph {
    unicode: u32,
    metrics: fontdue::Metrics,
    bitmap: Vec<u8>,
}

impl Logger {
    pub fn stderr(verbose: bool) -> Self {
        Self {
            verbose,
            sink: None,
        }
    }

    pub fn disabled() -> Self {
        Self::default()
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

struct ProgressBar {
    label: String,
    total: usize,
    width: usize,
    last_len: usize,
}

impl ProgressBar {
    fn new(label: impl Into<String>, total: usize) -> Self {
        Self {
            label: label.into(),
            total,
            width: 30,
            last_len: 0,
        }
    }

    fn render(&mut self, current: usize) {
        let total = self.total.max(1);
        let current = current.min(total);
        let ratio = current as f32 / total as f32;
        let filled = (ratio * self.width as f32).round() as usize;
        let filled = filled.min(self.width);
        let empty = self.width - filled;
        let percent = (ratio * 100.0).round() as u32;
        let bar = format!("[{}{}]", "#".repeat(filled), "-".repeat(empty));
        let message = format!(
            "{} {} {:>3}% ({}/{})",
            self.label, bar, percent, current, total
        );
        let padding = if message.len() < self.last_len {
            " ".repeat(self.last_len - message.len())
        } else {
            String::new()
        };
        self.last_len = message.len();
        eprint!("\r{message}{padding}");
        let _ = std::io::stderr().flush();
    }

    fn finish(&mut self) {
        self.render(self.total);
        eprintln!();
    }
}

fn print_stage(message: impl AsRef<str>) {
    eprintln!("terrain: {}", message.as_ref());
}

#[derive(Clone, Copy, Debug)]
pub struct BuildOptions {
    pub append: bool,
    pub write_binaries: bool,
}

impl Default for BuildOptions {
    fn default() -> Self {
        Self {
            append: false,
            write_binaries: true,
        }
    }
}

pub fn run_cli() -> Result<(), ()> {
    let mut args = std::env::args();
    let program = args.next().unwrap_or_else(|| "dbgen".to_string());

    let cli = match parse_command(&program, args) {
        Ok(cmd) => cmd,
        Err(err) => {
            eprintln!("{err}");
            print_usage(&program);
            return Err(());
        }
    };

    let logger = Logger::stderr(cli.verbose);
    let options = BuildOptions {
        append: false,
        write_binaries: cli.write_binaries,
    };

    let result = match cli.command {
        Command::Build { append, spec } => {
            build_from_path(&spec, BuildOptions { append, ..options }, &logger)
        }
        Command::Validate(args) => run_validation(&args, &logger),
        Command::AppendGeometry(args) => append_geometry(&args, &logger, cli.write_binaries),
        Command::AppendSkeleton(args) => append_skeleton(&args, &logger, cli.write_binaries),
        Command::AppendAnimation(args) => append_animation(&args, &logger, cli.write_binaries),
        Command::AppendImagery(args) => append_imagery(&args, &logger, cli.write_binaries),
        Command::AppendCubemap(args) => append_cubemap(&args, &logger, cli.write_binaries),
        Command::AppendAudio(args) => append_audio(&args, &logger, cli.write_binaries),
        Command::AppendFont(args) => append_font(&args, &logger, cli.write_binaries),
        Command::AppendShader(args) => append_shader(&args, &logger, cli.write_binaries),
        Command::Terrain(cmd) => match cmd {
            TerrainCommand::Init(args) => init_terrain_project(&args, &logger, cli.write_binaries),
            TerrainCommand::Export(args) => export_terrain_project(&args, &logger),
            TerrainCommand::Import(args) => {
                import_terrain_project(&args, &logger, cli.write_binaries)
            }
            TerrainCommand::Heightmap(args) => {
                import_terrain_heightmap(&args, &logger, cli.write_binaries)
            }
        },
    };

    if let Err(err) = result {
        eprintln!("error: {err}");
        return Err(());
    }

    Ok(())
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
                    "terrain" => parse_terrain_command(args)?,
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
        return Err(
            "append requires a resource type (geometry, imagery, cubemap, audio, font, shader)"
                .into(),
        );
    };

    match kind.as_str() {
        "geometry" => parse_geometry_append(args).map(Command::AppendGeometry),
        "skeleton" => parse_skeleton_append(args).map(Command::AppendSkeleton),
        "animation" => parse_animation_append(args).map(Command::AppendAnimation),
        "imagery" => parse_imagery_append(args).map(Command::AppendImagery),
        "cubemap" => parse_cubemap_append(args).map(Command::AppendCubemap),
        "audio" => parse_audio_append(args).map(Command::AppendAudio),
        "font" => parse_font_append(args).map(Command::AppendFont),
        "shader" => parse_shader_append(args).map(Command::AppendShader),
        other => Err(format!("unknown append resource type: {other}")),
    }
}

fn parse_terrain_command(mut args: impl Iterator<Item = String>) -> Result<Command, String> {
    let Some(kind) = args.next() else {
        return Err("terrain requires a subcommand (init, export, import, heightmap)".into());
    };

    match kind.as_str() {
        "init" => parse_terrain_init(args).map(|args| Command::Terrain(TerrainCommand::Init(args))),
        "export" => {
            parse_terrain_export(args).map(|args| Command::Terrain(TerrainCommand::Export(args)))
        }
        "import" => {
            parse_terrain_import(args).map(|args| Command::Terrain(TerrainCommand::Import(args)))
        }
        "heightmap" => parse_terrain_heightmap(args)
            .map(|args| Command::Terrain(TerrainCommand::Heightmap(args))),
        other => Err(format!("unknown terrain subcommand: {other}")),
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

fn parse_cubemap_append(
    mut args: impl Iterator<Item = String>,
) -> Result<CubemapAppendArgs, String> {
    let mut rdb: Option<PathBuf> = None;
    let mut entry = None;
    let mut pos_x = None;
    let mut neg_x = None;
    let mut pos_y = None;
    let mut neg_y = None;
    let mut pos_z = None;
    let mut neg_z = None;
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
            "--pos-x" => {
                pos_x = Some(next_value("--pos-x", &mut args)?);
            }
            "--neg-x" => {
                neg_x = Some(next_value("--neg-x", &mut args)?);
            }
            "--pos-y" => {
                pos_y = Some(next_value("--pos-y", &mut args)?);
            }
            "--neg-y" => {
                neg_y = Some(next_value("--neg-y", &mut args)?);
            }
            "--pos-z" => {
                pos_z = Some(next_value("--pos-z", &mut args)?);
            }
            "--neg-z" => {
                neg_z = Some(next_value("--neg-z", &mut args)?);
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
            other => return Err(format!("unexpected argument to append cubemap: {other}")),
        }
    }

    Ok(CubemapAppendArgs {
        rdb: rdb.ok_or_else(|| "--rdb is required".to_string())?,
        entry: CubemapEntry {
            entry: entry.ok_or_else(|| "--entry is required".to_string())?,
            pos_x: PathBuf::from(pos_x.ok_or_else(|| "--pos-x is required".to_string())?),
            neg_x: PathBuf::from(neg_x.ok_or_else(|| "--neg-x is required".to_string())?),
            pos_y: PathBuf::from(pos_y.ok_or_else(|| "--pos-y is required".to_string())?),
            neg_y: PathBuf::from(neg_y.ok_or_else(|| "--neg-y is required".to_string())?),
            pos_z: PathBuf::from(pos_z.ok_or_else(|| "--pos-z is required".to_string())?),
            neg_z: PathBuf::from(neg_z.ok_or_else(|| "--neg-z is required".to_string())?),
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

fn parse_font_append(mut args: impl Iterator<Item = String>) -> Result<FontAppendArgs, String> {
    let mut rdb: Option<PathBuf> = None;
    let mut entry = None;
    let mut file = None;
    let mut collection_index = None;

    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--rdb" => {
                let value = next_value("--rdb", &mut args)?;
                rdb = Some(PathBuf::from(value));
            }
            "--entry" => {
                entry = Some(next_value("--entry", &mut args)?);
            }
            "--font" => {
                file = Some(next_value("--font", &mut args)?);
            }
            "--collection-index" => {
                let value = next_value("--collection-index", &mut args)?;
                collection_index = Some(value.parse::<u32>().map_err(|_| {
                    format!("--collection-index expects an integer, received '{value}'")
                })?);
            }
            other => return Err(format!("unexpected argument to append font: {other}")),
        }
    }

    Ok(FontAppendArgs {
        rdb: rdb.ok_or_else(|| "--rdb is required".to_string())?,
        entry: FontEntry {
            entry: entry.ok_or_else(|| "--entry is required".to_string())?,
            file: PathBuf::from(file.ok_or_else(|| "--font is required".to_string())?),
            collection_index: collection_index.unwrap_or_default(),
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

fn parse_terrain_init(mut args: impl Iterator<Item = String>) -> Result<TerrainInitArgs, String> {
    let mut rdb: Option<PathBuf> = None;
    let mut project_key: Option<String> = None;
    let mut name: Option<String> = None;
    let mut seed: Option<u64> = None;

    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--rdb" => {
                rdb = Some(PathBuf::from(next_value("--rdb", &mut args)?));
            }
            "--project" => {
                project_key = Some(next_value("--project", &mut args)?);
            }
            "--name" => {
                name = Some(next_value("--name", &mut args)?);
            }
            "--seed" => {
                let value = next_value("--seed", &mut args)?;
                seed = Some(
                    value
                        .parse::<u64>()
                        .map_err(|_| "--seed expects a positive integer".to_string())?,
                );
            }
            other => return Err(format!("unexpected argument to terrain init: {other}")),
        }
    }

    Ok(TerrainInitArgs {
        rdb: rdb.ok_or_else(|| "--rdb is required".to_string())?,
        project_key: project_key.ok_or_else(|| "--project is required".to_string())?,
        name,
        seed,
    })
}

fn parse_terrain_export(
    mut args: impl Iterator<Item = String>,
) -> Result<TerrainExportArgs, String> {
    let mut rdb: Option<PathBuf> = None;
    let mut project_key: Option<String> = None;
    let mut output: Option<PathBuf> = None;

    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--rdb" => {
                rdb = Some(PathBuf::from(next_value("--rdb", &mut args)?));
            }
            "--project" => {
                project_key = Some(next_value("--project", &mut args)?);
            }
            "--out" => {
                output = Some(PathBuf::from(next_value("--out", &mut args)?));
            }
            other => return Err(format!("unexpected argument to terrain export: {other}")),
        }
    }

    Ok(TerrainExportArgs {
        rdb: rdb.ok_or_else(|| "--rdb is required".to_string())?,
        project_key: project_key.ok_or_else(|| "--project is required".to_string())?,
        output: output.ok_or_else(|| "--out is required".to_string())?,
    })
}

fn parse_terrain_import(
    mut args: impl Iterator<Item = String>,
) -> Result<TerrainImportArgs, String> {
    let mut rdb: Option<PathBuf> = None;
    let mut input: Option<PathBuf> = None;
    let mut project_key: Option<String> = None;

    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--rdb" => {
                rdb = Some(PathBuf::from(next_value("--rdb", &mut args)?));
            }
            "--input" => {
                input = Some(PathBuf::from(next_value("--input", &mut args)?));
            }
            "--project" => {
                project_key = Some(next_value("--project", &mut args)?);
            }
            other => return Err(format!("unexpected argument to terrain import: {other}")),
        }
    }

    Ok(TerrainImportArgs {
        rdb: rdb.ok_or_else(|| "--rdb is required".to_string())?,
        input: input.ok_or_else(|| "--input is required".to_string())?,
        project_key,
    })
}

fn parse_terrain_heightmap(
    mut args: impl Iterator<Item = String>,
) -> Result<TerrainHeightmapArgs, String> {
    let mut rdb = None;
    let mut heightmap = None;
    let mut project_key = None;
    let mut name = None;
    let mut tile_size = 1.0f32;
    let mut tiles_per_chunk = 32u32;
    let mut max_lod = 0u8;
    let mut detail = 1u32;
    let mut height_min = 0.0f32;
    let mut height_max = 256.0f32;

    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--rdb" => {
                rdb = Some(PathBuf::from(next_value("--rdb", &mut args)?));
            }
            "--heightmap" => {
                heightmap = Some(PathBuf::from(next_value("--heightmap", &mut args)?));
            }
            "--project" => {
                project_key = Some(next_value("--project", &mut args)?);
            }
            "--name" => {
                name = Some(next_value("--name", &mut args)?);
            }
            "--tile-size" => {
                tile_size = next_value("--tile-size", &mut args)?
                    .parse()
                    .map_err(|_| "--tile-size must be a number".to_string())?;
            }
            "--tiles-per-chunk" => {
                tiles_per_chunk = next_value("--tiles-per-chunk", &mut args)?
                    .parse()
                    .map_err(|_| "--tiles-per-chunk must be an integer".to_string())?;
            }
            "--max-lod" => {
                max_lod = next_value("--max-lod", &mut args)?
                    .parse()
                    .map_err(|_| "--max-lod must be an integer".to_string())?;
            }
            "--detail" => {
                detail = next_value("--detail", &mut args)?
                    .parse()
                    .map_err(|_| "--detail must be an integer".to_string())?;
            }
            "--height-min" => {
                height_min = next_value("--height-min", &mut args)?
                    .parse()
                    .map_err(|_| "--height-min must be a number".to_string())?;
            }
            "--height-max" => {
                height_max = next_value("--height-max", &mut args)?
                    .parse()
                    .map_err(|_| "--height-max must be a number".to_string())?;
            }
            other => return Err(format!("unexpected argument to terrain heightmap: {other}")),
        }
    }

    Ok(TerrainHeightmapArgs {
        rdb: rdb.ok_or_else(|| "--rdb is required".to_string())?,
        heightmap: heightmap.ok_or_else(|| "--heightmap is required".to_string())?,
        project_key: project_key.ok_or_else(|| "--project is required".to_string())?,
        name,
        tile_size,
        tiles_per_chunk,
        max_lod,
        detail,
        height_min,
        height_max,
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
    AppendCubemap(CubemapAppendArgs),
    AppendAudio(AudioAppendArgs),
    AppendFont(FontAppendArgs),
    AppendShader(ShaderAppendArgs),
    Terrain(TerrainCommand),
}

#[derive(Debug)]
enum TerrainCommand {
    Init(TerrainInitArgs),
    Export(TerrainExportArgs),
    Import(TerrainImportArgs),
    Heightmap(TerrainHeightmapArgs),
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
struct CubemapAppendArgs {
    rdb: PathBuf,
    entry: CubemapEntry,
}

#[derive(Debug)]
struct AudioAppendArgs {
    rdb: PathBuf,
    entry: AudioEntry,
}

#[derive(Debug)]
struct FontAppendArgs {
    rdb: PathBuf,
    entry: FontEntry,
}

#[derive(Debug)]
struct ShaderAppendArgs {
    rdb: PathBuf,
    entry: ShaderEntry,
}

#[derive(Debug)]
struct TerrainInitArgs {
    rdb: PathBuf,
    project_key: String,
    name: Option<String>,
    seed: Option<u64>,
}

#[derive(Debug)]
struct TerrainExportArgs {
    rdb: PathBuf,
    project_key: String,
    output: PathBuf,
}

#[derive(Debug)]
struct TerrainImportArgs {
    rdb: PathBuf,
    input: PathBuf,
    project_key: Option<String>,
}

#[derive(Debug)]
struct TerrainHeightmapArgs {
    rdb: PathBuf,
    heightmap: PathBuf,
    project_key: String,
    name: Option<String>,
    tile_size: f32,
    tiles_per_chunk: u32,
    max_lod: u8,
    detail: u32,
    height_min: f32,
    height_max: f32,
}

pub fn build_from_path(
    input: impl AsRef<Path>,
    options: BuildOptions,
    logger: &Logger,
) -> Result<(), BuildError> {
    let input = input.as_ref();
    logger.log(format!("building from spec: {}", input.display()));
    let file = File::open(input)?;
    let reader = BufReader::new(file);
    let spec: BuildSpec = serde_json::from_reader(reader)?;

    let base_dir = input
        .parent()
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."));

    build_from_spec(&base_dir, spec, options, logger)
}

pub fn build_from_spec(
    base_dir: impl AsRef<Path>,
    spec: BuildSpec,
    options: BuildOptions,
    logger: &Logger,
) -> Result<(), BuildError> {
    let base_dir = base_dir.as_ref();
    let BuildSpec {
        output,
        imagery,
        audio,
        fonts,
        skeletons,
        animations,
        geometry,
        shaders,
        models,
    } = spec;

    let output_dir = resolve_path(base_dir, &output.directory);
    fs::create_dir_all(&output_dir)?;

    let geometry_path = resolve_string_path(&output_dir, &output.layout.geometry);
    let imagery_path = resolve_string_path(&output_dir, &output.layout.imagery);
    let audio_path = resolve_string_path(&output_dir, &output.layout.audio);
    let fonts_path = resolve_string_path(&output_dir, &output.layout.fonts);
    let msdf_fonts_path = resolve_string_path(&output_dir, &output.layout.msdf_fonts);
    let sdf_fonts_path = resolve_string_path(&output_dir, &output.layout.sdf_fonts);
    let skeletons_path = resolve_string_path(&output_dir, &output.layout.skeletons);
    let animations_path = resolve_string_path(&output_dir, &output.layout.animations);
    let textures_path = resolve_string_path(&output_dir, &output.layout.textures);
    let atlases_path = resolve_string_path(&output_dir, &output.layout.atlases);
    let materials_path = resolve_string_path(&output_dir, &output.layout.materials);
    let meshes_path = resolve_string_path(&output_dir, &output.layout.meshes);
    let models_path = resolve_string_path(&output_dir, &output.layout.models);
    let shaders_path = resolve_string_path(&output_dir, &output.layout.shaders);
    let layout_path = resolve_path(&output_dir, &output.layout_file);

    let font_atlases = build_font_atlases(base_dir, &fonts, logger)?;

    build_geometry(
        base_dir,
        &geometry_path,
        &geometry,
        options.append,
        options.write_binaries,
        logger,
    )?;
    let gltf_sources = gather_gltf_sources(base_dir, &geometry, &skeletons, &animations);
    build_imagery(
        base_dir,
        &imagery_path,
        &imagery,
        &gltf_sources,
        &font_atlases.images,
        options.append,
        options.write_binaries,
        logger,
    )?;
    build_audio(
        base_dir,
        &audio_path,
        &audio,
        options.append,
        options.write_binaries,
        logger,
    )?;
    build_fonts(
        base_dir,
        &fonts_path,
        &fonts,
        options.append,
        options.write_binaries,
        logger,
    )?;
    build_skeletons(
        base_dir,
        &skeletons_path,
        &skeletons,
        options.append,
        options.write_binaries,
        logger,
    )?;
    build_animations(
        base_dir,
        &animations_path,
        &animations,
        options.append,
        options.write_binaries,
        logger,
    )?;
    build_shaders(
        base_dir,
        &shaders_path,
        &shaders,
        options.append,
        options.write_binaries,
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

    if let Some(parent) = atlases_path.parent() {
        fs::create_dir_all(parent)?;
    }
    let atlases_file = File::create(&atlases_path)?;
    serde_json::to_writer_pretty(atlases_file, &TextureAtlasLayoutFile::default())?;

    if let Some(parent) = msdf_fonts_path.parent() {
        fs::create_dir_all(parent)?;
    }
    let msdf_fonts_file = File::create(&msdf_fonts_path)?;
    serde_json::to_writer_pretty(
        msdf_fonts_file,
        &MsdfFontLayoutFile {
            fonts: font_atlases.msdf_layouts,
        },
    )?;

    if let Some(parent) = sdf_fonts_path.parent() {
        fs::create_dir_all(parent)?;
    }
    let sdf_fonts_file = File::create(&sdf_fonts_path)?;
    serde_json::to_writer_pretty(
        sdf_fonts_file,
        &SdfFontLayoutFile {
            fonts: font_atlases.sdf_layouts,
        },
    )?;

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
        vertex_count: base.vertex_count,
        index_count: base.index_count,
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

    let emissive_factor = primitive.material().emissive_factor();
    let default_color = [
        emissive_factor[0],
        emissive_factor[1],
        emissive_factor[2],
        1.0,
    ];
    let colors: Vec<[f32; 4]> = reader
        .read_colors(0)
        .map(|iter| iter.into_rgba_f32().collect())
        .unwrap_or_else(|| vec![default_color; vertex_count]);
    let joints: Vec<[u32; 4]> = reader
        .read_joints(0)
        .map(|iter| iter.into_u16().map(|joint| joint.map(u32::from)).collect())
        .unwrap_or_else(|| vec![[0; 4]; vertex_count]);
    let weights: Vec<[f32; 4]> = reader
        .read_weights(0)
        .map(|iter| iter.into_f32().collect())
        .unwrap_or_else(|| vec![[0.0; 4]; vertex_count]);

    let vertices: Vec<Vertex> = (0..vertex_count)
        .map(|idx| Vertex {
            position: positions[idx],
            normal: normals.get(idx).copied().unwrap_or([0.0, 0.0, 1.0]),
            tangent: tangents.get(idx).copied().unwrap_or([1.0, 0.0, 0.0, 1.0]),
            uv: tex_coords.get(idx).copied().unwrap_or([0.0, 0.0]),
            color: colors.get(idx).copied().unwrap_or(default_color),
            joint_indices: joints.get(idx).copied().unwrap_or([0; 4]),
            joint_weights: weights.get(idx).copied().unwrap_or([0.0; 4]),
        })
        .collect();

    let indices = reader
        .read_indices()
        .map(|iter| iter.into_u32().collect::<Vec<u32>>());

    let (vertices, indices) = match indices {
        Some(indices) => (vertices, indices),
        None => index_vertices(vertices),
    };

    Ok(GeometryLayer {
        vertex_count: vertices.len().try_into().unwrap_or(u32::MAX),
        index_count: Some(indices.len().try_into().unwrap_or(u32::MAX)),
        vertices,
        indices: Some(indices),
    })
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

fn append_cubemap(
    args: &CubemapAppendArgs,
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

    logger.log(format!("append cubemap: {}", args.entry.entry));
    let cubemap = load_cubemap(Path::new("."), &args.entry)?;
    let entry_name = args.entry.entry.clone();
    rdb.add(&entry_name, &cubemap).map_err(BuildError::from)?;
    inject_default_imagery(&mut rdb, logger)?;
    if write_binaries {
        logger.log(format!("append cubemap: writing {}", args.rdb.display()));
        rdb.save(&args.rdb).map_err(BuildError::from)?;
    } else {
        logger.log("append cubemap: skipping binary output (--layouts-only)");
    }
    Ok(())
}

fn build_imagery(
    base_dir: &Path,
    output: &Path,
    entries: &[ImageEntry],
    gltf_sources: &[PathBuf],
    extra_images: &[(String, HostImage)],
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

    let mut seen_entries: HashSet<String> =
        rdb.entries().into_iter().map(|meta| meta.name).collect();
    for entry in entries {
        logger.log(format!(
            "imagery: loading {} from {}",
            entry.entry,
            resolve_path(base_dir, &entry.file).display()
        ));
        let image = load_image(base_dir, entry)?;
        rdb.add(&entry.entry, &image).map_err(BuildError::from)?;
        seen_entries.insert(entry.entry.clone());
    }

    for gltf_source in gltf_sources {
        match load_gltf_images(base_dir, gltf_source, &mut seen_entries) {
            Ok(gltf_images) => {
                for (entry_name, image) in gltf_images {
                    logger.log(format!(
                        "imagery: loading {} from {}",
                        entry_name,
                        resolve_path(base_dir, gltf_source).display()
                    ));
                    rdb.add(&entry_name, &image).map_err(BuildError::from)?;
                }
            }
            Err(err) => {
                logger.log(format!(
                    "imagery: skipping embedded glTF images from {} ({err})",
                    resolve_path(base_dir, gltf_source).display()
                ));
            }
        }
    }

    inject_default_imagery(&mut rdb, logger)?;
    for (entry, image) in extra_images {
        if seen_entries.contains(entry) {
            continue;
        }
        logger.log(format!("imagery: adding generated {entry}"));
        rdb.add(entry, image).map_err(BuildError::from)?;
        seen_entries.insert(entry.clone());
    }

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

fn append_font(
    args: &FontAppendArgs,
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

    logger.log(format!("append font: {}", args.entry.entry));
    let font = load_font(Path::new("."), &args.entry)?;
    let entry_name = args.entry.entry.clone();
    rdb.add(&entry_name, &font).map_err(BuildError::from)?;

    if write_binaries {
        logger.log(format!("append font: writing {}", args.rdb.display()));
        rdb.save(&args.rdb).map_err(BuildError::from)?;
    } else {
        logger.log("append font: skipping binary output (--layouts-only)");
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

    let mut seen_entries: HashSet<String> =
        rdb.entries().into_iter().map(|meta| meta.name).collect();
    for entry in entries {
        seen_entries.insert(entry.entry.clone());
    }

    for clip in default_sounds() {
        if seen_entries.contains(&clip.name) {
            continue;
        }
        logger.log(format!("audio: adding default {}", clip.name));
        rdb.add(&clip.name, &clip).map_err(BuildError::from)?;
    }

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

fn build_fonts(
    base_dir: &Path,
    output: &Path,
    entries: &[FontEntry],
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

    let mut seen_entries: HashSet<String> =
        rdb.entries().into_iter().map(|meta| meta.name).collect();
    for entry in entries {
        seen_entries.insert(entry.entry.clone());
    }

    for font in default_fonts() {
        if seen_entries.contains(&font.info.name) {
            continue;
        }
        logger.log(format!("font: adding default {}", font.info.name));
        rdb.add(&font.info.name, &font).map_err(BuildError::from)?;
    }

    for entry in entries {
        logger.log(format!(
            "font: loading {} from {}",
            entry.entry,
            resolve_path(base_dir, &entry.file).display()
        ));
        let font = load_font(base_dir, entry)?;
        rdb.add(&entry.entry, &font).map_err(BuildError::from)?;
    }

    if write_binaries {
        logger.log(format!("font: writing {}", output.display()));
        rdb.save(output).map_err(BuildError::from)?;
    } else {
        logger.log("font: skipping binary output (--layouts-only)");
    }
    Ok(())
}

fn build_font_atlases(
    base_dir: &Path,
    entries: &[FontEntry],
    logger: &Logger,
) -> Result<FontAtlasOutput, BuildError> {
    let mut seen_entries = HashSet::new();
    for entry in entries {
        seen_entries.insert(entry.entry.clone());
    }

    let mut fonts = Vec::new();
    for font in default_fonts() {
        if seen_entries.contains(&font.info.name) {
            continue;
        }
        fonts.push(font);
    }

    for entry in entries {
        logger.log(format!(
            "font: loading {} from {}",
            entry.entry,
            resolve_path(base_dir, &entry.file).display()
        ));
        fonts.push(load_font(base_dir, entry)?);
    }

    let mut images = Vec::new();
    let mut msdf_layouts = HashMap::new();
    let mut sdf_layouts = HashMap::new();

    for font in fonts {
        let settings = FontSettings {
            collection_index: font.info.collection_index,
            ..FontSettings::default()
        };
        let parsed_font = Font::from_bytes(font.data.clone(), settings).map_err(|err| {
            BuildError::message(format!("font parse error for {}: {err}", font.info.name))
        })?;
        let image_entry = font_atlas_image_entry(&font.info.name);
        let (image, glyphs, metrics) = generate_font_atlas(&parsed_font, &image_entry, 16.0, 2)?;
        let display_name = parsed_font
            .name()
            .map(str::to_string)
            .unwrap_or_else(|| font.info.name.clone());
        let leaf = font_leaf_name(&font.info.name);

        images.push((image_entry.clone(), image));

        msdf_layouts.insert(
            format!("msdf_fonts/{leaf}"),
            MsdfFontLayout {
                image: image_entry.clone(),
                name: Some(format!("{display_name} MSDF Font")),
                font: Some(font.info.name.clone()),
                size: 16.0,
                distance_range: 4.0,
                angle_threshold: 3.0,
                metrics: metrics.clone(),
                glyphs: glyphs.clone(),
            },
        );
        sdf_layouts.insert(
            format!("sdf_fonts/{leaf}"),
            SdfFontLayout {
                image: image_entry,
                name: Some(format!("{display_name} SDF Font")),
                font: Some(font.info.name.clone()),
                size: 16.0,
                distance_range: 4.0,
                metrics,
                glyphs,
            },
        );
    }

    Ok(FontAtlasOutput {
        images,
        msdf_layouts,
        sdf_layouts,
    })
}

fn font_leaf_name(entry: &str) -> String {
    entry
        .rsplit('/')
        .next()
        .unwrap_or(entry)
        .trim()
        .replace(' ', "_")
}

fn font_atlas_image_entry(entry: &str) -> String {
    let leaf = font_leaf_name(entry);
    format!("imagery/fonts/{leaf}")
}

fn generate_font_atlas(
    font: &Font,
    image_entry: &str,
    size: f32,
    padding: u32,
) -> Result<(HostImage, Vec<FontGlyph>, FontMetrics), BuildError> {
    let mut glyphs: Vec<RasterizedGlyph> = font
        .chars()
        .iter()
        .map(|(character, glyph_index)| {
            let (metrics, bitmap) = font.rasterize_indexed(glyph_index.get(), size);
            RasterizedGlyph {
                unicode: *character as u32,
                metrics,
                bitmap,
            }
        })
        .collect();
    if glyphs.is_empty() {
        glyphs = (32u32..=126u32)
            .filter_map(|codepoint| {
                let ch = char::from_u32(codepoint)?;
                let (metrics, bitmap) = font.rasterize(ch, size);
                Some(RasterizedGlyph {
                    unicode: codepoint,
                    metrics,
                    bitmap,
                })
            })
            .collect();
    }
    glyphs.sort_by_key(|glyph| glyph.unicode);

    let placements = pack_glyphs(&glyphs, padding)?;
    let atlas_dim = placements
        .values()
        .fold(1u32, |acc, placement| acc.max(placement.atlas_dim));
    let mut pixels = vec![0u8; (atlas_dim * atlas_dim * 4) as usize];

    for (index, glyph) in glyphs.iter().enumerate() {
        if let Some(placement) = placements.get(&index) {
            let width = glyph.metrics.width as u32;
            let height = glyph.metrics.height as u32;
            for row in 0..height {
                let src_start = (row * width) as usize;
                let src_end = src_start + width as usize;
                let src_row = &glyph.bitmap[src_start..src_end];
                let dest_y = placement.y + row;
                let dest_x = placement.x;
                let dest_start = ((dest_y * atlas_dim + dest_x) * 4) as usize;
                for (offset, alpha) in src_row.iter().enumerate() {
                    let dest = dest_start + offset * 4;
                    pixels[dest] = 255;
                    pixels[dest + 1] = 255;
                    pixels[dest + 2] = 255;
                    pixels[dest + 3] = *alpha;
                }
            }
        }
    }

    let info = ImageInfo {
        name: image_entry.to_string(),
        dim: [atlas_dim, atlas_dim, 1],
        layers: 1,
        format: dashi::Format::RGBA8,
        mip_levels: 1,
    };
    let image = HostImage::new(info, pixels);

    let line_metrics = font.horizontal_line_metrics(size);
    let metrics = FontMetrics {
        em_size: size,
        line_height: line_metrics
            .map(|metrics| metrics.new_line_size)
            .unwrap_or(size),
        ascender: line_metrics
            .map(|metrics| metrics.ascent)
            .unwrap_or_default(),
        descender: line_metrics
            .map(|metrics| metrics.descent)
            .unwrap_or_default(),
        underline_y: 0.0,
        underline_thickness: 0.0,
    };

    let layout_glyphs = glyphs
        .iter()
        .enumerate()
        .map(|(index, glyph)| FontGlyph {
            unicode: glyph.unicode,
            advance: glyph.metrics.advance_width,
            plane_bounds: glyph_plane_bounds(&glyph.metrics),
            atlas_bounds: placements.get(&index).map(|placement| FontBounds {
                left: placement.x as f32,
                bottom: placement.y as f32,
                right: (placement.x + placement.width) as f32,
                top: (placement.y + placement.height) as f32,
            }),
        })
        .collect();

    Ok((image, layout_glyphs, metrics))
}

fn glyph_plane_bounds(metrics: &fontdue::Metrics) -> Option<FontBounds> {
    if metrics.width == 0 || metrics.height == 0 {
        return None;
    }
    let left = metrics.xmin as f32;
    let bottom = metrics.ymin as f32;
    Some(FontBounds {
        left,
        bottom,
        right: left + metrics.width as f32,
        top: bottom + metrics.height as f32,
    })
}

struct GlyphPlacement {
    x: u32,
    y: u32,
    width: u32,
    height: u32,
    atlas_dim: u32,
}

fn pack_glyphs(
    glyphs: &[RasterizedGlyph],
    padding: u32,
) -> Result<HashMap<usize, GlyphPlacement>, BuildError> {
    let mut sortable: Vec<(usize, u32, u32)> = glyphs
        .iter()
        .enumerate()
        .filter_map(|(index, glyph)| {
            let width = glyph.metrics.width as u32;
            let height = glyph.metrics.height as u32;
            if width == 0 || height == 0 {
                None
            } else {
                Some((index, width, height))
            }
        })
        .collect();
    sortable.sort_by(|a, b| b.2.cmp(&a.2).then_with(|| b.1.cmp(&a.1)));

    let mut atlas_dim = 256u32;
    loop {
        if let Some(placements) = try_pack(atlas_dim, &sortable, padding) {
            return Ok(placements);
        }
        atlas_dim = atlas_dim.saturating_mul(2);
        if atlas_dim > 16384 {
            return Err(BuildError::message(
                "font atlas exceeded maximum size when packing glyphs",
            ));
        }
    }
}

fn try_pack(
    atlas_dim: u32,
    glyphs: &[(usize, u32, u32)],
    padding: u32,
) -> Option<HashMap<usize, GlyphPlacement>> {
    let mut placements = HashMap::new();
    let mut cursor_x = padding;
    let mut cursor_y = padding;
    let mut row_height = 0u32;

    for (index, width, height) in glyphs {
        if *width + padding * 2 > atlas_dim || *height + padding * 2 > atlas_dim {
            return None;
        }
        if cursor_x + width + padding > atlas_dim {
            cursor_x = padding;
            cursor_y = cursor_y.saturating_add(row_height + padding);
            row_height = 0;
        }

        if cursor_y + height + padding > atlas_dim {
            return None;
        }

        placements.insert(
            *index,
            GlyphPlacement {
                x: cursor_x,
                y: cursor_y,
                width: *width,
                height: *height,
                atlas_dim,
            },
        );
        cursor_x = cursor_x.saturating_add(width + padding);
        row_height = row_height.max(*height);
    }

    Some(placements)
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

#[derive(Debug, Serialize, Deserialize)]
struct TerrainProjectExport {
    project_key: String,
    settings: TerrainProjectSettings,
    generator: TerrainGeneratorDefinition,
    mutation_layers: Vec<TerrainMutationLayer>,
}

fn init_terrain_project(
    args: &TerrainInitArgs,
    logger: &Logger,
    write_binaries: bool,
) -> Result<(), BuildError> {
    print_stage("stage 1/3 - preparing project settings");
    if let Some(parent) = args.rdb.parent() {
        fs::create_dir_all(parent)?;
    }

    let mut rdb = if write_binaries {
        load_rdb(&args.rdb, true)?
    } else {
        RDBFile::new()
    };

    let settings_key = project_settings_entry(&args.project_key);
    if rdb.entries().iter().any(|entry| entry.name == settings_key) {
        return Err(BuildError::message(format!(
            "terrain project '{}' already exists",
            args.project_key
        )));
    }

    let mut settings = TerrainProjectSettings::default();
    if let Some(name) = &args.name {
        settings.name = name.clone();
    }
    if let Some(seed) = args.seed {
        settings.seed = seed;
    }

    print_stage("stage 2/3 - writing project entries");
    let generator = TerrainGeneratorDefinition::default();
    settings.active_generator_version = generator.version;

    let layer = TerrainMutationLayer::new("layer-1", "Layer 1", 0);
    settings.active_mutation_version = layer.version;

    rdb.add(&settings_key, &settings)
        .map_err(BuildError::from)?;
    rdb.add(
        &generator_entry(&args.project_key, generator.version),
        &generator,
    )
    .map_err(BuildError::from)?;
    rdb.add(
        &mutation_layer_entry(&args.project_key, &layer.layer_id, layer.version),
        &layer,
    )
    .map_err(BuildError::from)?;

    if write_binaries {
        print_stage("stage 3/3 - writing output");
        logger.log(format!("terrain: writing {}", args.rdb.display()));
        rdb.save(&args.rdb).map_err(BuildError::from)?;
    } else {
        print_stage("stage 3/3 - skipping binary output (--layouts-only)");
        logger.log("terrain: skipping binary output (--layouts-only)");
    }
    Ok(())
}

fn export_terrain_project(args: &TerrainExportArgs, logger: &Logger) -> Result<(), BuildError> {
    let mut rdb = RDBFile::load(&args.rdb).map_err(BuildError::from)?;
    rdb.unmap();
    let settings = rdb
        .fetch::<TerrainProjectSettings>(&project_settings_entry(&args.project_key))
        .map_err(BuildError::from)?;
    let generator = rdb
        .fetch::<TerrainGeneratorDefinition>(&generator_entry(
            &args.project_key,
            settings.active_generator_version,
        ))
        .map_err(BuildError::from)?;
    let entries = rdb.entries();
    let mutation_layers = collect_terrain_layers(
        &entries,
        &mut rdb,
        &args.project_key,
        settings.active_mutation_version,
    )?;

    let payload = TerrainProjectExport {
        project_key: args.project_key.clone(),
        settings,
        generator,
        mutation_layers,
    };

    if let Some(parent) = args.output.parent() {
        fs::create_dir_all(parent)?;
    }
    let file = File::create(&args.output)?;
    serde_json::to_writer_pretty(file, &payload)?;
    logger.log(format!(
        "terrain: exported project '{}' to {}",
        args.project_key,
        args.output.display()
    ));
    Ok(())
}

fn import_terrain_project(
    args: &TerrainImportArgs,
    logger: &Logger,
    write_binaries: bool,
) -> Result<(), BuildError> {
    print_stage("stage 1/3 - reading project file");
    if let Some(parent) = args.rdb.parent() {
        fs::create_dir_all(parent)?;
    }

    let mut rdb = if write_binaries {
        load_rdb(&args.rdb, true)?
    } else {
        RDBFile::new()
    };

    let file = File::open(&args.input)?;
    let reader = BufReader::new(file);
    let mut payload: TerrainProjectExport = serde_json::from_reader(reader)?;
    if let Some(project_key) = &args.project_key {
        payload.project_key = project_key.clone();
    }

    print_stage("stage 2/3 - writing project entries");
    rdb.add(
        &project_settings_entry(&payload.project_key),
        &payload.settings,
    )
    .map_err(BuildError::from)?;
    rdb.add(
        &generator_entry(&payload.project_key, payload.generator.version),
        &payload.generator,
    )
    .map_err(BuildError::from)?;
    for layer in &payload.mutation_layers {
        rdb.add(
            &mutation_layer_entry(&payload.project_key, &layer.layer_id, layer.version),
            layer,
        )
        .map_err(BuildError::from)?;
        for op in &layer.ops {
            rdb.add(
                &mutation_op_entry(
                    &payload.project_key,
                    &layer.layer_id,
                    layer.version,
                    op.order,
                    op.event_id,
                ),
                op,
            )
            .map_err(BuildError::from)?;
        }
    }

    if write_binaries {
        print_stage("stage 3/3 - writing output");
        logger.log(format!("terrain: writing {}", args.rdb.display()));
        rdb.save(&args.rdb).map_err(BuildError::from)?;
    } else {
        print_stage("stage 3/3 - skipping binary output (--layouts-only)");
        logger.log("terrain: skipping binary output (--layouts-only)");
    }
    Ok(())
}

fn import_terrain_heightmap(
    args: &TerrainHeightmapArgs,
    logger: &Logger,
    write_binaries: bool,
) -> Result<(), BuildError> {
    print_stage("stage 1/3 - loading heightmap");
    if let Some(parent) = args.rdb.parent() {
        fs::create_dir_all(parent)?;
    }

    let mut rdb = if write_binaries {
        load_rdb(&args.rdb, true)?
    } else {
        RDBFile::new()
    };

    if args.tiles_per_chunk == 0 {
        return Err(BuildError::message(
            "--tiles-per-chunk must be greater than 0",
        ));
    }
    if args.detail == 0 {
        return Err(BuildError::message("--detail must be greater than 0"));
    }
    if args.tile_size <= 0.0 {
        return Err(BuildError::message("--tile-size must be greater than 0"));
    }
    if args.height_max <= args.height_min {
        return Err(BuildError::message(
            "--height-max must be greater than --height-min",
        ));
    }

    let settings_key = project_settings_entry(&args.project_key);
    if rdb.entries().iter().any(|entry| entry.name == settings_key) {
        return Err(BuildError::message(format!(
            "terrain project '{}' already exists",
            args.project_key
        )));
    }

    let heightmap = image::open(&args.heightmap)?;
    let heightmap = heightmap.to_luma16();
    let (width, height) = heightmap.dimensions();
    if width < 2 || height < 2 {
        return Err(BuildError::message("heightmap must be at least 2x2 pixels"));
    }

    let tiles_per_chunk = args.tiles_per_chunk;
    let tiles_x = width.saturating_sub(1).div_ceil(args.detail);
    let tiles_y = height.saturating_sub(1).div_ceil(args.detail);
    let chunk_count_x = (tiles_x + tiles_per_chunk - 1) / tiles_per_chunk;
    let chunk_count_y = (tiles_y + tiles_per_chunk - 1) / tiles_per_chunk;

    let mut settings = TerrainProjectSettings::default();
    settings.name = args
        .name
        .clone()
        .unwrap_or_else(|| format!("{} Heightmap", args.project_key));
    settings.tile_size = args.tile_size;
    settings.tiles_per_chunk = [tiles_per_chunk, tiles_per_chunk];
    settings.world_bounds_min = [0.0, args.height_min, 0.0];
    settings.world_bounds_max = [
        tiles_x as f32 * args.tile_size,
        args.height_max,
        tiles_y as f32 * args.tile_size,
    ];
    settings.lod_policy.max_lod = args.max_lod;

    let generator = TerrainGeneratorDefinition::default();
    settings.active_generator_version = generator.version;

    let layer = TerrainMutationLayer::new("layer-1", "Layer 1", 0);
    settings.active_mutation_version = layer.version;

    print_stage("stage 2/3 - generating terrain chunks");
    rdb.add(&settings_key, &settings)
        .map_err(BuildError::from)?;
    rdb.add(
        &generator_entry(&args.project_key, generator.version),
        &generator,
    )
    .map_err(BuildError::from)?;
    rdb.add(
        &mutation_layer_entry(&args.project_key, &layer.layer_id, layer.version),
        &layer,
    )
    .map_err(BuildError::from)?;

    let chunk_sample_width = tiles_per_chunk + 1;
    let chunk_sample_len = (chunk_sample_width * chunk_sample_width) as usize;
    let tile_len = (tiles_per_chunk * tiles_per_chunk) as usize;
    let height_scale = args.height_max - args.height_min;

    logger.log(format!(
        "terrain: heightmap {} ({}x{}), detail {} px/tile, chunks {}x{}",
        args.heightmap.display(),
        width,
        height,
        args.detail,
        chunk_count_x,
        chunk_count_y
    ));

    let total_chunks = (chunk_count_x * chunk_count_y) as usize;
    let mut progress = ProgressBar::new("terrain: chunks", total_chunks);
    let mut processed_chunks = 0usize;

    for chunk_y in 0..chunk_count_y {
        for chunk_x in 0..chunk_count_x {
            let origin = [
                chunk_x as f32 * tiles_per_chunk as f32 * args.tile_size,
                chunk_y as f32 * tiles_per_chunk as f32 * args.tile_size,
            ];
            let tiles = vec![TerrainTile::default(); tile_len];
            let mut heights = Vec::with_capacity(chunk_sample_len);
            let mut min_height = f32::MAX;
            let mut max_height = f32::MIN;
            for sample_y in 0..=tiles_per_chunk {
                let tile_y = chunk_y * tiles_per_chunk + sample_y;
                let global_y = (tile_y * args.detail).min(height - 1);
                for sample_x in 0..=tiles_per_chunk {
                    let tile_x = chunk_x * tiles_per_chunk + sample_x;
                    let global_x = (tile_x * args.detail).min(width - 1);
                    let value = heightmap.get_pixel(global_x, global_y).0[0];
                    let normalized = value as f32 / u16::MAX as f32;
                    let height_sample = args.height_min + normalized * height_scale;
                    min_height = min_height.min(height_sample);
                    max_height = max_height.max(height_sample);
                    heights.push(height_sample);
                }
            }
            if heights.is_empty() {
                min_height = args.height_min;
                max_height = args.height_min;
            }
            let chunk_size_x = tiles_per_chunk as f32 * args.tile_size;
            let chunk_size_z = tiles_per_chunk as f32 * args.tile_size;
            let bounds_min = [origin[0], min_height, origin[1]];
            let bounds_max = [
                origin[0] + chunk_size_x,
                max_height,
                origin[1] + chunk_size_z,
            ];

            let valid_tiles_x = tiles_x
                .saturating_sub(chunk_x * tiles_per_chunk)
                .min(tiles_per_chunk);
            let valid_tiles_y = tiles_y
                .saturating_sub(chunk_y * tiles_per_chunk)
                .min(tiles_per_chunk);
            let chunk = TerrainChunk {
                chunk_coords: [chunk_x as i32, chunk_y as i32],
                origin,
                tile_size: args.tile_size,
                tiles_per_chunk: [tiles_per_chunk, tiles_per_chunk],
                tiles,
                heights,
                bounds_min,
                bounds_max,
            };
            let entry = format!("terrain/chunk_{chunk_x}_{chunk_y}");
            rdb.add(&entry, &chunk).map_err(BuildError::from)?;

            let coord_key = chunk_coord_key(chunk_x as i32, chunk_y as i32);
            let valid_tiles = [valid_tiles_x, valid_tiles_y];
            for lod in 0..=settings.lod_policy.max_lod {
                let artifact = build_heightmap_chunk_artifact(
                    &settings,
                    &generator,
                    std::slice::from_ref(&layer),
                    &args.project_key,
                    &chunk,
                    lod,
                    valid_tiles,
                );
                let artifact_entry =
                    chunk_artifact_entry(&args.project_key, &coord_key, &lod_key(lod));
                rdb.add(&artifact_entry, &artifact)
                    .map_err(BuildError::from)?;
            }

            processed_chunks += 1;
            progress.render(processed_chunks);
        }
    }
    if total_chunks > 0 {
        progress.finish();
    }

    if write_binaries {
        print_stage("stage 3/3 - writing output");
        logger.log(format!("terrain: writing {}", args.rdb.display()));
        rdb.save(&args.rdb).map_err(BuildError::from)?;
    } else {
        print_stage("stage 3/3 - skipping binary output (--layouts-only)");
        logger.log("terrain: skipping binary output (--layouts-only)");
    }
    Ok(())
}

fn collect_terrain_layers(
    entries: &[RDBEntryMeta],
    rdb: &mut RDBFile,
    project_key: &str,
    active_version: u32,
) -> Result<Vec<TerrainMutationLayer>, BuildError> {
    let prefix = format!("{TERRAIN_MUTATION_LAYER_PREFIX}/{project_key}/");
    let mut layer_versions: BTreeMap<String, u32> = BTreeMap::new();
    for entry in entries {
        if let Some((layer_id, version)) = parse_mutation_layer_entry(&entry.name, &prefix) {
            if version > active_version {
                continue;
            }
            let current = layer_versions.entry(layer_id).or_insert(version);
            if version > *current {
                *current = version;
            }
        }
    }

    let mut layers = Vec::new();
    for (layer_id, version) in layer_versions {
        let entry = mutation_layer_entry(project_key, &layer_id, version);
        let mut layer = rdb
            .fetch::<TerrainMutationLayer>(&entry)
            .map_err(BuildError::from)?;
        let op_events = collect_terrain_ops(entries, rdb, project_key, &layer_id, active_version)?;
        if !op_events.is_empty() {
            layer.ops = op_events;
        }
        layers.push(layer);
    }
    layers.sort_by(|a, b| {
        a.order
            .cmp(&b.order)
            .then_with(|| a.layer_id.cmp(&b.layer_id))
    });
    Ok(layers)
}

fn collect_terrain_ops(
    entries: &[RDBEntryMeta],
    rdb: &mut RDBFile,
    project_key: &str,
    layer_id: &str,
    active_version: u32,
) -> Result<Vec<TerrainMutationOp>, BuildError> {
    let prefix = format!("{TERRAIN_MUTATION_OP_PREFIX}/{project_key}/{layer_id}/");
    let mut latest: BTreeMap<String, TerrainMutationOp> = BTreeMap::new();
    for entry in entries {
        if let Some((version, order, event_id)) = parse_mutation_op_entry(&entry.name, &prefix) {
            if version > active_version {
                continue;
            }
            let mut op = rdb
                .fetch::<TerrainMutationOp>(&entry.name)
                .map_err(BuildError::from)?;
            op.order = order;
            op.event_id = event_id;
            latest
                .entry(op.op_id.clone())
                .and_modify(|current| {
                    if op.event_id > current.event_id {
                        *current = op.clone();
                    }
                })
                .or_insert(op);
        }
    }
    let mut ops: Vec<TerrainMutationOp> = latest.into_values().collect();
    ops.sort_by(|a, b| a.order.cmp(&b.order).then_with(|| a.op_id.cmp(&b.op_id)));
    Ok(ops)
}

fn parse_mutation_op_entry(name: &str, prefix: &str) -> Option<(u32, u32, u32)> {
    let remainder = name.strip_prefix(prefix)?;
    let mut parts = remainder.split('/');
    let version_part = parts.next()?;
    let order_part = parts.next()?;
    let event_part = parts.next()?;
    let version = version_part.strip_prefix('v')?.parse().ok()?;
    let order = order_part.strip_prefix('o')?.parse().ok()?;
    let event_id = event_part.strip_prefix('e')?.parse().ok()?;
    Some((version, order, event_id))
}

fn parse_mutation_layer_entry(name: &str, prefix: &str) -> Option<(String, u32)> {
    let remainder = name.strip_prefix(prefix)?;
    let mut parts = remainder.split('/');
    let layer_id = parts.next()?.to_string();
    let version_part = parts.next()?;
    let version = version_part.strip_prefix('v')?.parse().ok()?;
    Some((layer_id, version))
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

fn load_cubemap(base_dir: &Path, entry: &CubemapEntry) -> Result<HostCubemap, BuildError> {
    let face_paths = [
        &entry.pos_x,
        &entry.neg_x,
        &entry.pos_y,
        &entry.neg_y,
        &entry.pos_z,
        &entry.neg_z,
    ];

    let mut faces = Vec::with_capacity(6);
    let mut dimensions = None;

    for face_path in face_paths {
        let path = resolve_path(base_dir, face_path);
        let image = image::open(&path)?;
        let rgba = to_rgba(image);
        let (width, height) = rgba.dimensions();
        if let Some((expected_width, expected_height)) = dimensions {
            if width != expected_width || height != expected_height {
                return Err(BuildError::message(format!(
                    "cubemap face dimensions mismatch: expected {expected_width}x{expected_height}, got {width}x{height}"
                )));
            }
        } else {
            dimensions = Some((width, height));
        }
        faces.push(rgba.into_raw());
    }

    let (width, height) = dimensions.unwrap_or((0, 0));
    let info = ImageInfo {
        name: entry.entry.clone(),
        dim: [width, height, 1],
        layers: 6,
        format: entry.format,
        mip_levels: entry.mip_levels,
    };

    let faces: [Vec<u8>; 6] = faces
        .try_into()
        .map_err(|_| BuildError::message("cubemap must have exactly 6 faces"))?;
    HostCubemap::from_faces(info, faces).map_err(BuildError::from)
}

fn gather_gltf_sources(
    base_dir: &Path,
    geometry: &[GeometryEntry],
    skeletons: &[SkeletonEntry],
    animations: &[AnimationEntry],
) -> Vec<PathBuf> {
    let mut sources = HashSet::new();

    for entry in geometry {
        sources.insert(resolve_path(base_dir, &entry.file));
        for lod in &entry.lods {
            if let Some(file) = &lod.file {
                sources.insert(resolve_path(base_dir, file));
            }
        }
    }

    for entry in skeletons {
        sources.insert(resolve_path(base_dir, &entry.file));
    }

    for entry in animations {
        sources.insert(resolve_path(base_dir, &entry.file));
    }

    sources.into_iter().collect()
}

fn load_gltf_images(
    base_dir: &Path,
    file: &Path,
    seen_entries: &mut HashSet<String>,
) -> Result<Vec<(String, HostImage)>, BuildError> {
    let path = resolve_path(base_dir, file);
    let (doc, _, images) = gltf::import(path)?;
    let mut entries = Vec::with_capacity(images.len());
    let file_slug = file
        .file_stem()
        .and_then(|stem| stem.to_str())
        .map(slugify)
        .filter(|slug| !slug.is_empty())
        .unwrap_or_else(|| "gltf".to_string());
    let prefix = format!("imagery/{file_slug}");

    for (index, image) in images.iter().enumerate() {
        let image_name = doc.images().nth(index).and_then(|image| image.name());
        let image_slug = image_name
            .map(slugify)
            .filter(|slug| !slug.is_empty())
            .unwrap_or_else(|| format!("image_{index}"));
        let base_entry = format!("{prefix}/{image_slug}");
        let entry = unique_entry_name(base_entry, seen_entries);
        let data = rgba_from_gltf_image(image)?;
        let info = ImageInfo {
            name: entry.clone(),
            dim: [image.width, image.height, 1],
            layers: 1,
            format: dashi::Format::RGBA8,
            mip_levels: 1,
        };
        entries.push((entry, HostImage::new(info, data)));
    }

    Ok(entries)
}

fn unique_entry_name(base: String, seen_entries: &mut HashSet<String>) -> String {
    let mut entry = base.clone();
    let mut suffix = 1;
    while seen_entries.contains(&entry) {
        entry = format!("{base}_{suffix}");
        suffix += 1;
    }
    seen_entries.insert(entry.clone());
    entry
}

fn rgba_from_gltf_image(image: &gltf::image::Data) -> Result<Vec<u8>, BuildError> {
    match image.format {
        Format::R8G8B8A8 => Ok(image.pixels.clone()),
        Format::R8G8B8 => Ok(image
            .pixels
            .chunks_exact(3)
            .flat_map(|chunk| [chunk[0], chunk[1], chunk[2], 255])
            .collect()),
        Format::R8G8 => Ok(image
            .pixels
            .chunks_exact(2)
            .flat_map(|chunk| [chunk[0], chunk[1], 0, 255])
            .collect()),
        Format::R8 => Ok(image
            .pixels
            .iter()
            .flat_map(|value| [*value, *value, *value, 255])
            .collect()),
        _ => Err(BuildError::message(format!(
            "unsupported glTF image format {:?}",
            image.format
        ))),
    }
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

fn load_font(base_dir: &Path, entry: &FontEntry) -> Result<HostFont, BuildError> {
    let path = resolve_path(base_dir, &entry.file);
    let data = fs::read(&path)?;
    let settings = FontSettings {
        collection_index: entry.collection_index,
        ..Default::default()
    };
    Font::from_bytes(data.clone(), settings).map_err(|err| {
        BuildError::message(format!("font parse error for {}: {err}", path.display()))
    })?;

    Ok(HostFont::new_with_index(
        entry.entry.clone(),
        entry.collection_index,
        data,
    ))
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
    let node_to_joint = doc
        .skins()
        .next()
        .map(|skin| {
            skin.joints()
                .enumerate()
                .map(|(idx, joint)| (joint.index(), idx))
                .collect::<HashMap<_, _>>()
        })
        .unwrap_or_default();

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

        let target_node = node_to_joint
            .get(&target.node().index())
            .copied()
            .unwrap_or_else(|| target.node().index());

        channels.push(AnimationChannel {
            sampler_index,
            target_node,
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

fn slugify(value: &str) -> String {
    let mut out = String::new();
    let mut prev_sep = false;
    for ch in value.chars() {
        let lower = ch.to_ascii_lowercase();
        if lower.is_ascii_alphanumeric() {
            out.push(lower);
            prev_sep = false;
        } else if !prev_sep {
            out.push('_');
            prev_sep = true;
        }
    }
    while out.ends_with('_') {
        out.pop();
    }
    if out.is_empty() {
        "texture".to_string()
    } else {
        out
    }
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
        "  {program} append cubemap --rdb <imagery.rdb> --entry <name> --pos-x <file> --neg-x <file> --pos-y <file> --neg-y <file> --pos-z <file> --neg-z <file> [--mip-levels <count>] [--format <format>]"
    );
    eprintln!(
        "  {program} append audio --rdb <audio.rdb> --entry <name> --audio <file> [--format <format>]"
    );
    eprintln!(
        "  {program} append font --rdb <fonts.rdb> --entry <name> --font <file> [--collection-index <index>]"
    );
    eprintln!(
        "  {program} append shader --rdb <shaders.rdb> --entry <name> --stage <stage> --shader <file>"
    );
    eprintln!(
        "  {program} terrain init --rdb <terrain.rdb> --project <key> [--name <name>] [--seed <seed>]"
    );
    eprintln!("  {program} terrain export --rdb <terrain.rdb> --project <key> --out <file>");
    eprintln!("  {program} terrain import --rdb <terrain.rdb> --input <file> [--project <key>]");
    eprintln!(
        "  {program} terrain heightmap --rdb <terrain.rdb> --project <key> --heightmap <file> [--name <name>] [--tile-size <size>] [--tiles-per-chunk <count>] [--detail <pixels>] [--max-lod <index>] [--height-min <value>] [--height-max <value>]"
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
pub struct BuildSpec {
    #[serde(default)]
    pub output: OutputSpec,
    #[serde(default)]
    pub imagery: Vec<ImageEntry>,
    #[serde(default)]
    pub audio: Vec<AudioEntry>,
    #[serde(default)]
    pub fonts: Vec<FontEntry>,
    #[serde(default)]
    pub skeletons: Vec<SkeletonEntry>,
    #[serde(default)]
    pub animations: Vec<AnimationEntry>,
    #[serde(default)]
    pub geometry: Vec<GeometryEntry>,
    #[serde(default)]
    pub shaders: Vec<ShaderEntry>,
    #[serde(default)]
    pub models: Vec<ModelEntry>,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(default)]
pub struct OutputSpec {
    pub directory: PathBuf,
    pub layout_file: PathBuf,
    pub layout: DatabaseLayoutFile,
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
pub struct GeometryEntry {
    pub entry: String,
    pub file: PathBuf,
    #[serde(default)]
    pub mesh: Option<String>,
    #[serde(default)]
    pub primitive: Option<usize>,
    #[serde(default)]
    pub lods: Vec<GeometryLodEntry>,
}

#[derive(Debug, Deserialize, Serialize, Default)]
pub struct GeometryLodEntry {
    #[serde(default)]
    pub file: Option<PathBuf>,
    #[serde(default)]
    pub mesh: Option<String>,
    #[serde(default)]
    pub primitive: Option<usize>,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct SkeletonEntry {
    pub entry: String,
    pub file: PathBuf,
    #[serde(default)]
    pub skin: Option<String>,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct AnimationEntry {
    pub entry: String,
    pub file: PathBuf,
    #[serde(default)]
    pub animation: Option<String>,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct ImageEntry {
    pub entry: String,
    pub file: PathBuf,
    #[serde(default = "default_layers")]
    pub layers: u32,
    #[serde(default = "default_format")]
    pub format: dashi::Format,
    #[serde(default = "default_mip_levels")]
    pub mip_levels: u32,
}

#[derive(Debug)]
struct CubemapEntry {
    entry: String,
    pos_x: PathBuf,
    neg_x: PathBuf,
    pos_y: PathBuf,
    neg_y: PathBuf,
    pos_z: PathBuf,
    neg_z: PathBuf,
    format: dashi::Format,
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
pub struct AudioEntry {
    pub entry: String,
    pub file: PathBuf,
    #[serde(default)]
    pub format: Option<AudioFormat>,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct FontEntry {
    pub entry: String,
    pub file: PathBuf,
    #[serde(default)]
    pub collection_index: u32,
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
pub struct ShaderEntry {
    pub entry: String,
    pub stage: ShaderStageKind,
    pub file: PathBuf,
}

#[derive(Debug, Copy, Clone, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ShaderStageKind {
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
pub struct ModelEntry {
    pub name: String,
    pub geometry: String,
    #[serde(default)]
    pub textures: Vec<String>,
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

    ensure_default_assets(
        &mut layouts.textures.textures,
        &mut layouts.materials.materials,
        &mut layouts.meshes.meshes,
        &mut layouts.models.models,
    );

    layouts
}

fn normalize_entry_name(entry: &str, prefix: &str, allow_existing_prefix: bool) -> String {
    if entry.starts_with(prefix) || (allow_existing_prefix && entry.contains('/')) {
        entry.to_string()
    } else {
        format!("{prefix}{entry}")
    }
}

#[derive(Debug)]
pub enum BuildError {
    Io(std::io::Error),
    Json(serde_json::Error),
    Image(image::ImageError),
    Gltf(gltf::Error),
    Rdb(RdbErr),
    Shader(BentoError),
    Message(String),
}

impl BuildError {
    pub fn message<T: Into<String>>(msg: T) -> Self {
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
    use noren::defaults::{DEFAULT_IMAGE_ENTRY, DEFAULT_MATERIAL_ENTRY, DEFAULT_TEXTURE_ENTRY};
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
                    fonts: "fonts.rdb".into(),
                    msdf_fonts: "msdf_fonts.json".into(),
                    sdf_fonts: "sdf_fonts.json".into(),
                    skeletons: "skeletons.rdb".into(),
                    animations: "animations.rdb".into(),
                    terrain: "terrain.rdb".into(),
                    materials: "materials.json".into(),
                    textures: "textures.json".into(),
                    atlases: "atlases.json".into(),
                    meshes: "meshes.json".into(),
                    models: "models.json".into(),
                    shader_layouts: "shaders.json".into(),
                    shaders: "shaders.rdb".into(),
                },
            },
            skeletons: Vec::new(),
            animations: Vec::new(),
            audio: Vec::new(),
            fonts: Vec::new(),
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
        build_from_path(
            &build_path,
            BuildOptions {
                append: false,
                write_binaries: true,
            },
            &logger,
        )
        .unwrap();

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
        assert_eq!(layout.fonts, "fonts.rdb");
        assert_eq!(layout.msdf_fonts, "msdf_fonts.json");
        assert_eq!(layout.sdf_fonts, "sdf_fonts.json");
        assert_eq!(layout.skeletons, "skeletons.rdb");
        assert_eq!(layout.animations, "animations.rdb");
        assert_eq!(layout.terrain, "terrain.rdb");
        assert_eq!(layout.textures, "textures.json");
        assert_eq!(layout.atlases, "atlases.json");
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
                    fonts: "fonts.rdb".into(),
                    msdf_fonts: "msdf_fonts.json".into(),
                    sdf_fonts: "sdf_fonts.json".into(),
                    skeletons: "skeletons.rdb".into(),
                    animations: "animations.rdb".into(),
                    terrain: "terrain.rdb".into(),
                    materials: "materials.json".into(),
                    textures: "textures.json".into(),
                    atlases: "atlases.json".into(),
                    meshes: "meshes.json".into(),
                    models: "models.json".into(),
                    shader_layouts: "shaders.json".into(),
                    shaders: "shaders.rdb".into(),
                },
            },
            skeletons: Vec::new(),
            animations: Vec::new(),
            audio: Vec::new(),
            fonts: Vec::new(),
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
        build_from_path(
            &build_path,
            BuildOptions {
                append: false,
                write_binaries: false,
            },
            &logger,
        )
        .unwrap();

        let output_dir = tmp_root.join("db");
        assert!(!output_dir.join("geometry.rdb").exists());
        assert!(!output_dir.join("imagery.rdb").exists());
        assert!(!output_dir.join("skeletons.rdb").exists());
        assert!(!output_dir.join("animations.rdb").exists());
        assert!(!output_dir.join("shaders.rdb").exists());
        assert!(output_dir.join("materials.json").exists());
        assert!(output_dir.join("textures.json").exists());
        assert!(output_dir.join("msdf_fonts.json").exists());
        assert!(output_dir.join("sdf_fonts.json").exists());
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
        assert_eq!(layout.fonts, "fonts.rdb");
        assert_eq!(layout.msdf_fonts, "msdf_fonts.json");
        assert_eq!(layout.sdf_fonts, "sdf_fonts.json");
        assert_eq!(layout.skeletons, "skeletons.rdb");
        assert_eq!(layout.animations, "animations.rdb");
        assert_eq!(layout.terrain, "terrain.rdb");
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
                    fonts: "fonts.rdb".into(),
                    msdf_fonts: "msdf_fonts.json".into(),
                    sdf_fonts: "sdf_fonts.json".into(),
                    skeletons: "skeletons.rdb".into(),
                    animations: "animations.rdb".into(),
                    terrain: "terrain.rdb".into(),
                    textures: "textures.json".into(),
                    atlases: "atlases.json".into(),
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
            fonts: Vec::new(),
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
        build_from_path(
            &build_path,
            BuildOptions {
                append: false,
                write_binaries: false,
            },
            &logger,
        )
        .unwrap();

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
                    fonts: "fonts.rdb".into(),
                    msdf_fonts: "msdf_fonts.json".into(),
                    sdf_fonts: "sdf_fonts.json".into(),
                    skeletons: "skeletons.rdb".into(),
                    animations: "animations.rdb".into(),
                    terrain: "terrain.rdb".into(),
                    materials: "materials.json".into(),
                    textures: "textures.json".into(),
                    atlases: "atlases.json".into(),
                    meshes: "meshes.json".into(),
                    models: "models.json".into(),
                    shader_layouts: "shaders.json".into(),
                    shaders: "shaders.rdb".into(),
                },
            },
            skeletons: Vec::new(),
            animations: Vec::new(),
            audio: Vec::new(),
            fonts: Vec::new(),
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
        build_from_path(
            &build_path,
            BuildOptions {
                append: false,
                write_binaries: true,
            },
            &logger,
        )
        .unwrap();

        let append_spec = BuildSpec {
            output: OutputSpec {
                directory: PathBuf::from("../db"),
                layout_file: PathBuf::from("layout.json"),
                layout: DatabaseLayoutFile {
                    geometry: "geometry.rdb".into(),
                    imagery: "imagery.rdb".into(),
                    audio: "audio.rdb".into(),
                    fonts: "fonts.rdb".into(),
                    msdf_fonts: "msdf_fonts.json".into(),
                    sdf_fonts: "sdf_fonts.json".into(),
                    skeletons: "skeletons.rdb".into(),
                    animations: "animations.rdb".into(),
                    terrain: "terrain.rdb".into(),
                    textures: "textures.json".into(),
                    atlases: "atlases.json".into(),
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
            fonts: Vec::new(),
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
        build_from_path(
            &append_path,
            BuildOptions {
                append: true,
                write_binaries: true,
            },
            &logger,
        )
        .unwrap();

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
                    fonts: "fonts.rdb".into(),
                    msdf_fonts: "msdf_fonts.json".into(),
                    sdf_fonts: "sdf_fonts.json".into(),
                    skeletons: "skeletons.rdb".into(),
                    animations: "animations.rdb".into(),
                    terrain: "terrain.rdb".into(),
                    textures: "textures.json".into(),
                    atlases: "atlases.json".into(),
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
            fonts: Vec::new(),
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
        build_from_path(
            &build_path,
            BuildOptions {
                append: false,
                write_binaries: true,
            },
            &logger,
        )
        .unwrap();

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

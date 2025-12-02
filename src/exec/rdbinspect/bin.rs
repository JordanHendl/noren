use std::{any::type_name, env, fs, path::PathBuf, process, sync::OnceLock};

use bincode::deserialize;
use noren::{
    RDBView,
    rdb::{HostGeometry, HostImage, ShaderModule},
    type_tag_for,
};
use serde::de::DeserializeOwned;

fn main() {
    if let Err(err) = run() {
        eprintln!("{err}");
        process::exit(1);
    }
}

fn run() -> Result<(), String> {
    let mut args = env::args();
    let program = args.next().unwrap_or_else(|| "rdbinspect".to_string());

    let rest: Vec<String> = args.collect();
    if rest.is_empty() {
        print_usage(&program);
        return Err("missing RDB file path".to_string());
    }

    let mut path: Option<PathBuf> = None;
    let mut entry_to_dump: Option<String> = None;
    let mut hex_limit: usize = 256;
    let mut show_hex = true;

    let mut iter = rest.into_iter();
    while let Some(arg) = iter.next() {
        match arg.as_str() {
            "-h" | "--help" => {
                print_usage(&program);
                return Ok(());
            }
            "--entry" => {
                let value = iter
                    .next()
                    .ok_or_else(|| "--entry requires an entry name".to_string())?;
                entry_to_dump = Some(value);
            }
            "--limit" => {
                let value = iter
                    .next()
                    .ok_or_else(|| "--limit requires a byte count".to_string())?;
                hex_limit = value
                    .parse::<usize>()
                    .map_err(|_| "--limit expects a positive integer".to_string())?;
            }
            "--no-hex" => {
                show_hex = false;
            }
            _ => {
                if path.is_none() {
                    path = Some(PathBuf::from(arg));
                } else {
                    print_usage(&program);
                    return Err(format!("unexpected argument: {arg}"));
                }
            }
        }
    }

    let Some(path) = path else {
        print_usage(&program);
        return Err("missing RDB file path".to_string());
    };

    let view =
        RDBView::load(&path).map_err(|err| format!("failed to open {}: {err}", path.display()))?;
    let entries = view.entries();

    println!("File: {}", path.display());
    if let Ok(meta) = fs::metadata(&path) {
        println!("Size: {} bytes", meta.len());
    }
    println!("Entries: {}", entries.len());

    if entries.is_empty() {
        println!("(no entries)");
    } else {
        let name_width = entries
            .iter()
            .map(|entry| entry.name.len())
            .max()
            .unwrap_or(4)
            .clamp(4, 48);
        let type_width = entries
            .iter()
            .map(|entry| type_label(entry.type_tag).len())
            .max()
            .unwrap_or(4)
            .clamp(4, 32);
        println!(
            "\n{:<name_width$}  {:<type_width$}  {:^12}  {:>12}  {:>12}",
            "Name",
            "Type",
            "Type (hex)",
            "Offset",
            "Length",
            name_width = name_width,
            type_width = type_width
        );
        println!(
            "{:-<name_width$}  {:-<type_width$}  {:-^12}  {:-<12}  {:-<12}",
            "",
            "",
            "",
            "",
            "",
            name_width = name_width,
            type_width = type_width
        );

        for entry in &entries {
            let label = type_label(entry.type_tag);
            println!(
                "{:<name_width$}  {:<type_width$}  {:#010X}  {:>12}  {:>12}",
                truncated_name(&entry.name, name_width),
                label,
                entry.type_tag,
                entry.offset,
                entry.len,
                name_width = name_width,
                type_width = type_width
            );
        }

        let total_bytes: u64 = entries.iter().map(|entry| entry.len).sum();
        println!("\nTotal payload bytes: {total_bytes}");
    }

    if let Some(name) = entry_to_dump {
        let meta = entries
            .iter()
            .find(|entry| entry.name == name)
            .cloned()
            .ok_or_else(|| format!("entry '{name}' not found"))?;

        println!("\nEntry: {}", meta.name);
        let entry_type_label = type_label(meta.type_tag);
        println!("  Type: {entry_type_label}");
        println!("  Type (hex): {:#010X}", meta.type_tag);
        println!("  Offset: {}", meta.offset);
        println!("  Length: {} bytes", meta.len);

        let bytes = view
            .entry_bytes(&meta.name)
            .map_err(|err| format!("unable to read entry '{}': {err}", meta.name))?;

        if let Some(known) = known_type(meta.type_tag) {
            println!("\nDeserialized as {}:", known.display_name());
            match (known.describe)(bytes) {
                Ok(text) => println!("{text}"),
                Err(err) => println!("(failed to decode {}: {err})", known.display_name()),
            }
        }

        if show_hex {
            println!("\nHex dump (showing up to {hex_limit} bytes):");
            hexdump(bytes, hex_limit);
        }
    }

    Ok(())
}

fn print_usage(program: &str) {
    println!("Usage: {program} <RDB_FILE> [--entry <NAME>] [--limit <BYTES>] [--no-hex]");
    println!("\nOptions:");
    println!("  --entry <NAME>   Inspect a specific entry and display its metadata");
    println!("  --limit <BYTES>  Limit the number of bytes shown in the hex dump (default 256)");
    println!("  --no-hex         Skip the hex dump when inspecting an entry");
    println!("  -h, --help       Show this help message");
}

fn ascii_type(tag: u32) -> Option<String> {
    let bytes = tag.to_le_bytes();
    if bytes.iter().all(|b| matches!(b, 0x20..=0x7E)) {
        Some(String::from_utf8_lossy(&bytes).into_owned())
    } else {
        None
    }
}

fn known_types() -> &'static [KnownType] {
    static KNOWN: OnceLock<Vec<KnownType>> = OnceLock::new();
    KNOWN.get_or_init(|| {
        vec![
            KnownType::with::<HostGeometry>(describe_geometry),
            KnownType::with::<HostImage>(describe_image),
            KnownType::with::<ShaderModule>(describe_shader),
        ]
    })
}

fn known_type(tag: u32) -> Option<&'static KnownType> {
    known_types().iter().find(|ty| ty.tag == tag)
}

fn type_label(tag: u32) -> String {
    if let Some(known) = known_type(tag) {
        return known.display_name().to_string();
    }

    ascii_type(tag).unwrap_or_else(|| "-".to_string())
}

struct KnownType {
    name: &'static str,
    tag: u32,
    describe: Box<dyn Fn(&[u8]) -> Result<String, String> + Send + Sync>,
}

impl KnownType {
    fn new<T>() -> Self
    where
        T: DeserializeOwned + std::fmt::Debug + 'static,
    {
        Self {
            name: type_name::<T>(),
            tag: type_tag_for::<T>(),
            describe: Box::new(|bytes| {
                deserialize::<T>(bytes)
                    .map(|value| format!("{value:#?}"))
                    .map_err(|err| err.to_string())
            }),
        }
    }

    fn with<T>(describe: fn(&T) -> String) -> Self
    where
        T: DeserializeOwned + std::fmt::Debug + 'static,
    {
        Self {
            name: type_name::<T>(),
            tag: type_tag_for::<T>(),
            describe: Box::new(move |bytes| {
                let value: T = deserialize(bytes).map_err(|err| err.to_string())?;
                Ok(describe(&value))
            }),
        }
    }

    fn display_name(&self) -> &str {
        self.name
            .rsplit_once("::")
            .map(|(_, tail)| tail)
            .unwrap_or(self.name)
    }
}

fn describe_geometry(geometry: &HostGeometry) -> String {
    let mut description = describe_geometry_layer(
        "Base",
        geometry.vertices.len(),
        geometry.indices.as_ref(),
        "  ",
    );

    if !geometry.lods.is_empty() {
        description.push_str(&format!("\n  LODs: {}", geometry.lods.len()));
        for (idx, lod) in geometry.lods.iter().enumerate() {
            description.push('\n');
            description.push_str(&describe_geometry_layer(
                &format!("LOD {idx}"),
                lod.vertices.len(),
                lod.indices.as_ref(),
                "    ",
            ));
        }
    }

    description
}

fn describe_geometry_layer(
    name: &str,
    vertex_count: usize,
    indices: Option<&Vec<u32>>,
    indent: &str,
) -> String {
    let (index_summary, triangle_hint) = match indices {
        Some(list) if !list.is_empty() => {
            let triangle_count = list.len() / 3;
            (
                format!(
                    "{} indices ({} bytes)",
                    list.len(),
                    list.len() * std::mem::size_of::<u32>()
                ),
                format!("\n{indent}  ~ Estimated triangles: {triangle_count}"),
            )
        }
        Some(_) => ("0 indices (empty buffer)".to_string(), String::new()),
        None => ("None (non-indexed draw)".to_string(), String::new()),
    };

    format!(
        "{indent}{name}:\n{indent}  Vertices: {vertex_count}\n{indent}  Indices: {index_summary}{triangle_hint}",
    )
}

fn describe_image(image: &HostImage) -> String {
    let info = &image.info;
    let dimensions = format!("{} x {} x {}", info.dim[0], info.dim[1], info.dim[2]);
    let bytes = image.data.len();

    format!(
        "  Dimensions: {dimensions}\n  Layers: {}\n  Format: {:?}\n  Mip levels: {}\n  Data size: {bytes} bytes",
        info.layers, info.format, info.mip_levels
    )
}

fn describe_shader(module: &ShaderModule) -> String {
    let artifact = module.artifact();
    let name = artifact
        .name
        .as_deref()
        .or(artifact.file.as_deref())
        .unwrap_or("(unnamed)");

    let binding_lines: Vec<String> = artifact
        .variables
        .iter()
        .map(|var| format!("    - {}: {:?}", var.name, var.kind))
        .collect();

    let bindings = if binding_lines.is_empty() {
        "  Bindings: (none)".to_string()
    } else {
        format!("  Bindings:\n{}", binding_lines.join("\n"))
    };

    format!(
        "  Name: {name}\n  Language: {:?}\n  Stage: {:?}\n  SPIR-V words: {}\n{}",
        artifact.lang,
        artifact.stage,
        artifact.spirv.len(),
        bindings
    )
}

fn truncated_name(name: &str, width: usize) -> String {
    if name.len() <= width {
        name.to_string()
    } else if width <= 1 {
        "…".to_string()
    } else {
        let mut truncated = name.chars().take(width - 1).collect::<String>();
        truncated.push('…');
        truncated
    }
}

fn hexdump(data: &[u8], limit: usize) {
    let max = data.len().min(limit);
    let mut offset = 0usize;

    while offset < max {
        let end = (offset + 16).min(max);
        let chunk = &data[offset..end];
        print!("{:08X}: ", offset);

        for i in 0..16 {
            if i < chunk.len() {
                print!("{:02X} ", chunk[i]);
            } else {
                print!("   ");
            }
        }

        print!(" |");
        for &byte in chunk {
            let ch = if (0x20..=0x7E).contains(&byte) {
                byte as char
            } else {
                '.'
            };
            print!("{ch}");
        }
        println!("|");

        offset += 16;
    }

    if max < data.len() {
        println!("... truncated ({} additional bytes)", data.len() - max);
    }
}

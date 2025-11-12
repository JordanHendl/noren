use std::{env, fs, path::PathBuf, process};

use noren::RDBView;

fn main() {
    if let Err(err) = run() {
        eprintln!("{err}");
        process::exit(1);
    }
}

fn run() -> Result<(), String> {
    let mut args = env::args();
    let program = args
        .next()
        .unwrap_or_else(|| "noren_rdbinspect".to_string());

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
        println!(
            "\n{:<name_width$}  {:<6}  {:^12}  {:>12}  {:>12}",
            "Name",
            "Type",
            "Type (hex)",
            "Offset",
            "Length",
            name_width = name_width
        );
        println!(
            "{:-<name_width$}  {:-<6}  {:-^12}  {:-<12}  {:-<12}",
            "",
            "",
            "",
            "",
            "",
            name_width = name_width
        );

        for entry in &entries {
            let ascii = ascii_type(entry.type_tag).unwrap_or_else(|| "-".to_string());
            println!(
                "{:<name_width$}  {:<6}  {:#010X}  {:>12}  {:>12}",
                truncated_name(&entry.name, name_width),
                ascii,
                entry.type_tag,
                entry.offset,
                entry.len,
                name_width = name_width
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
        println!(
            "  Type: {}",
            ascii_type(meta.type_tag).unwrap_or_else(|| "-".to_string())
        );
        println!("  Type (hex): {:#010X}", meta.type_tag);
        println!("  Offset: {}", meta.offset);
        println!("  Length: {} bytes", meta.len);

        if show_hex {
            let bytes = view
                .entry_bytes(&meta.name)
                .map_err(|err| format!("unable to read entry '{}': {err}", meta.name))?;
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

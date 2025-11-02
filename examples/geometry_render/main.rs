//! Run with `cargo run --example geometry_render` to load geometry from the
//! database and upload it to GPU buffers.

#[path = "../common/mod.rs"]
mod common;

use common::{SAMPLE_GEOMETRY_ENTRY, init_context, open_sample_db, write_image_artifact};
use std::error::Error;
use std::f32;

use image::{ImageBuffer, Rgba, RgbaImage};
use noren::datatypes::{geometry::HostGeometry, primitives::Vertex};

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
    let geom_db = db.geometry_mut();

    let host_geometry = geom_db.fetch_raw_geometry(SAMPLE_GEOMETRY_ENTRY)?;
    println!(
        "Host geometry '{}' contains {} vertices and {} indices",
        SAMPLE_GEOMETRY_ENTRY,
        host_geometry.vertices.len(),
        host_geometry.indices.as_ref().map_or(0, |idx| idx.len())
    );

    let device_geometry = geom_db.fetch_gpu_geometry(SAMPLE_GEOMETRY_ENTRY)?;
    println!(
        "Uploaded GPU geometry with buffers {:?} / {:?}",
        device_geometry.vertices, device_geometry.indices
    );

    let preview = rasterize_geometry(&host_geometry)?;
    let output_path = write_image_artifact("geometry_render", "preview.png", &preview)?;
    println!("Wrote headless preview to {}", output_path.display());

    Ok(())
}

fn rasterize_geometry(geometry: &HostGeometry) -> Result<RgbaImage, Box<dyn Error>> {
    const SIZE: u32 = 512;
    let mut image = ImageBuffer::from_pixel(SIZE, SIZE, Rgba([20, 20, 28, 255]));

    if geometry.vertices.is_empty() {
        return Ok(image);
    }

    let mut min = [f32::MAX; 2];
    let mut max = [f32::MIN; 2];
    for vertex in &geometry.vertices {
        update_bounds(&mut min, &mut max, vertex.position);
    }

    if (max[0] - min[0]).abs() < f32::EPSILON {
        min[0] -= 0.5;
        max[0] += 0.5;
    }
    if (max[1] - min[1]).abs() < f32::EPSILON {
        min[1] -= 0.5;
        max[1] += 0.5;
    }

    let indices: Vec<u32> = match &geometry.indices {
        Some(idx) if !idx.is_empty() => idx.clone(),
        _ => (0..geometry.vertices.len() as u32).collect(),
    };

    for triangle in indices.chunks(3) {
        if triangle.len() < 3 {
            continue;
        }

        let vertices = [
            &geometry.vertices[triangle[0] as usize],
            &geometry.vertices[triangle[1] as usize],
            &geometry.vertices[triangle[2] as usize],
        ];

        draw_triangle(&mut image, &vertices, min, max);
    }

    Ok(image)
}

fn update_bounds(min: &mut [f32; 2], max: &mut [f32; 2], position: [f32; 3]) {
    min[0] = min[0].min(position[0]);
    min[1] = min[1].min(position[1]);
    max[0] = max[0].max(position[0]);
    max[1] = max[1].max(position[1]);
}

fn draw_triangle(image: &mut RgbaImage, vertices: &[&Vertex; 3], min: [f32; 2], max: [f32; 2]) {
    let points = vertices
        .iter()
        .map(|v| project_to_pixel(image.width(), image.height(), v.position, min, max))
        .collect::<Vec<_>>();

    let colors = vertices
        .iter()
        .map(|v| {
            let rgba = v.color;
            [
                (rgba[0].clamp(0.0, 1.0) * 255.0) as u8,
                (rgba[1].clamp(0.0, 1.0) * 255.0) as u8,
                (rgba[2].clamp(0.0, 1.0) * 255.0) as u8,
                255,
            ]
        })
        .collect::<Vec<_>>();

    let min_x = points
        .iter()
        .map(|&(x, _)| x)
        .fold(i32::MAX, |a, b| a.min(b))
        .clamp(0, image.width() as i32 - 1);
    let max_x = points
        .iter()
        .map(|&(x, _)| x)
        .fold(i32::MIN, |a, b| a.max(b))
        .clamp(0, image.width() as i32 - 1);
    let min_y = points
        .iter()
        .map(|&(_, y)| y)
        .fold(i32::MAX, |a, b| a.min(b))
        .clamp(0, image.height() as i32 - 1);
    let max_y = points
        .iter()
        .map(|&(_, y)| y)
        .fold(i32::MIN, |a, b| a.max(b))
        .clamp(0, image.height() as i32 - 1);

    if min_x > max_x || min_y > max_y {
        return;
    }

    let area = edge(points[0], points[1], points[2]) as f32;
    if area.abs() < f32::EPSILON {
        return;
    }

    for y in min_y..=max_y {
        for x in min_x..=max_x {
            let p = (x as f32 + 0.5, y as f32 + 0.5);
            let w0 = edge(points[1], points[2], p) as f32 / area;
            let w1 = edge(points[2], points[0], p) as f32 / area;
            let w2 = edge(points[0], points[1], p) as f32 / area;

            if w0 < 0.0 || w1 < 0.0 || w2 < 0.0 {
                continue;
            }

            let mut color = [0.0f32; 3];
            for i in 0..3 {
                color[0] += colors[i][0] as f32 * weights(i, w0, w1, w2);
                color[1] += colors[i][1] as f32 * weights(i, w0, w1, w2);
                color[2] += colors[i][2] as f32 * weights(i, w0, w1, w2);
            }

            image.put_pixel(
                x as u32,
                y as u32,
                Rgba([
                    color[0].clamp(0.0, 255.0) as u8,
                    color[1].clamp(0.0, 255.0) as u8,
                    color[2].clamp(0.0, 255.0) as u8,
                    255,
                ]),
            );
        }
    }
}

fn weights(index: usize, w0: f32, w1: f32, w2: f32) -> f32 {
    match index {
        0 => w0,
        1 => w1,
        _ => w2,
    }
}

fn project_to_pixel(
    width: u32,
    height: u32,
    position: [f32; 3],
    min: [f32; 2],
    max: [f32; 2],
) -> (i32, i32) {
    let nx = (position[0] - min[0]) / (max[0] - min[0]);
    let ny = (position[1] - min[1]) / (max[1] - min[1]);
    let px = (nx.clamp(0.0, 1.0) * (width as f32 - 1.0)).round() as i32;
    let py = ((1.0 - ny.clamp(0.0, 1.0)) * (height as f32 - 1.0)).round() as i32;
    (px, py)
}

fn edge(a: (i32, i32), b: (i32, i32), c: (f32, f32)) -> f32 {
    let ax = a.0 as f32;
    let ay = a.1 as f32;
    let bx = b.0 as f32;
    let by = b.1 as f32;
    (c.0 - ax) * (by - ay) - (c.1 - ay) * (bx - ax)
}

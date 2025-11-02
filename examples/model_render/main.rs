//! Run with `cargo run --example model_render` to load the quad model definition
//! and upload its geometry and textures to GPU resources.

#[path = "../common/mod.rs"]
mod common;

use common::{
    SAMPLE_MODEL_ENTRY, init_context, open_sample_db, write_image_artifact, write_text_artifact,
};
use std::error::Error;
use std::fmt::Write;

use image::{ImageBuffer, Rgba, RgbaImage};
use noren::datatypes::{geometry::HostGeometry, imagery::HostImage, primitives::Vertex};
use noren::meta::model::{HostMesh, HostModel};

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

    let host_model = db.fetch_model(SAMPLE_MODEL_ENTRY)?;
    println!(
        "Host model '{}' contains {} mesh(es)",
        host_model.name,
        host_model.meshes.len()
    );
    for mesh in &host_model.meshes {
        println!(
            " - Mesh '{}' has {} vertices and {} material texture(s)",
            mesh.name,
            mesh.geometry.vertices.len(),
            mesh.material.as_ref().map_or(0, |mat| mat.textures.len())
        );
    }

    let device_model = db.fetch_gpu_model(SAMPLE_MODEL_ENTRY)?;
    for mesh in &device_model.meshes {
        println!(
            "Uploaded mesh '{}' with vertex buffer {:?} and index buffer {:?}",
            mesh.name, mesh.geometry.vertices, mesh.geometry.indices
        );
        if let Some(material) = &mesh.material {
            for texture in &material.textures {
                println!(
                    "   - Texture '{}' uploaded as {:?}",
                    texture.name, texture.image
                );
            }
        }
    }

    let mut summary = String::new();
    writeln!(
        &mut summary,
        "Model '{}' contains {} mesh(es)",
        host_model.name,
        host_model.meshes.len()
    )?;
    for mesh in &host_model.meshes {
        summarize_mesh(&mut summary, mesh)?;
    }
    let summary_path = write_text_artifact("model_render", "model.txt", &summary)?;
    println!("Wrote model summary to {}", summary_path.display());

    let preview = render_model_preview(&host_model)?;
    let image_path = write_image_artifact("model_render", "preview.png", &preview)?;
    println!("Rendered model preview to {}", image_path.display());

    Ok(())
}

fn summarize_mesh(output: &mut String, mesh: &HostMesh) -> Result<(), Box<dyn Error>> {
    writeln!(
        output,
        "- Mesh '{}' has {} vertices and {} indices",
        mesh.name,
        mesh.geometry.vertices.len(),
        mesh.geometry.indices.as_ref().map_or(0, |idx| idx.len())
    )?;

    if let Some(material) = &mesh.material {
        writeln!(output, "  Material '{}':", material.name)?;
        for texture in &material.textures {
            writeln!(output, "    - Texture '{}'", texture.name)?;
        }
    }

    if !mesh.textures.is_empty() {
        writeln!(output, "  Embedded textures:")?;
        for texture in &mesh.textures {
            writeln!(output, "    - Texture '{}'", texture.name)?;
        }
    }

    Ok(())
}

fn render_model_preview(model: &HostModel) -> Result<RgbaImage, Box<dyn Error>> {
    let mesh = model
        .meshes
        .first()
        .ok_or_else(|| "model contains no meshes".into())?;
    let geometry = &mesh.geometry;
    if geometry.vertices.is_empty() {
        return Ok(ImageBuffer::from_pixel(512, 512, Rgba([0, 0, 0, 255])));
    }

    let texture = mesh
        .material
        .as_ref()
        .and_then(|mat| mat.textures.first())
        .or_else(|| mesh.textures.first())
        .ok_or_else(|| "mesh has no textures".into())?;
    let texture_image = host_image_to_rgba(&texture.image)?;

    rasterize_textured_geometry(geometry, &texture_image)
}

fn host_image_to_rgba(image: &HostImage) -> Result<RgbaImage, Box<dyn Error>> {
    let dims = image.info().dim;
    RgbaImage::from_raw(dims[0], dims[1], image.data().to_vec())
        .ok_or_else(|| "invalid texture dimensions".into())
}

fn rasterize_textured_geometry(
    geometry: &HostGeometry,
    texture: &RgbaImage,
) -> Result<RgbaImage, Box<dyn Error>> {
    const SIZE: u32 = 512;
    let mut image = ImageBuffer::from_pixel(SIZE, SIZE, Rgba([12, 12, 18, 255]));

    let mut min = [f32::MAX; 2];
    let mut max = [f32::MIN; 2];
    for vertex in &geometry.vertices {
        min[0] = min[0].min(vertex.position[0]);
        min[1] = min[1].min(vertex.position[1]);
        max[0] = max[0].max(vertex.position[0]);
        max[1] = max[1].max(vertex.position[1]);
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

        draw_textured_triangle(&mut image, texture, &vertices, min, max);
    }

    Ok(image)
}

fn draw_textured_triangle(
    output: &mut RgbaImage,
    texture: &RgbaImage,
    vertices: &[&Vertex; 3],
    min: [f32; 2],
    max: [f32; 2],
) {
    let points = vertices
        .iter()
        .map(|v| project_to_pixel(output.width(), output.height(), v.position, min, max))
        .collect::<Vec<_>>();

    let mut vertex_colors = [[0.0f32; 4]; 3];
    let mut vertex_uvs = [[0.0f32; 2]; 3];
    for (idx, vertex) in vertices.iter().enumerate() {
        vertex_colors[idx] = [
            vertex.color[0].clamp(0.0, 1.0),
            vertex.color[1].clamp(0.0, 1.0),
            vertex.color[2].clamp(0.0, 1.0),
            vertex.color[3].clamp(0.0, 1.0),
        ];
        vertex_uvs[idx] = vertex.uv;
    }

    let min_x = points
        .iter()
        .map(|&(x, _)| x)
        .fold(i32::MAX, |a, b| a.min(b))
        .clamp(0, output.width() as i32 - 1);
    let max_x = points
        .iter()
        .map(|&(x, _)| x)
        .fold(i32::MIN, |a, b| a.max(b))
        .clamp(0, output.width() as i32 - 1);
    let min_y = points
        .iter()
        .map(|&(_, y)| y)
        .fold(i32::MAX, |a, b| a.min(b))
        .clamp(0, output.height() as i32 - 1);
    let max_y = points
        .iter()
        .map(|&(_, y)| y)
        .fold(i32::MIN, |a, b| a.max(b))
        .clamp(0, output.height() as i32 - 1);

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

            let uv = [
                vertex_uvs[0][0] * w0 + vertex_uvs[1][0] * w1 + vertex_uvs[2][0] * w2,
                vertex_uvs[0][1] * w0 + vertex_uvs[1][1] * w1 + vertex_uvs[2][1] * w2,
            ];
            let sampled = sample_texture(texture, uv);

            let vertex_color = [
                vertex_colors[0][0] * w0 + vertex_colors[1][0] * w1 + vertex_colors[2][0] * w2,
                vertex_colors[0][1] * w0 + vertex_colors[1][1] * w1 + vertex_colors[2][1] * w2,
                vertex_colors[0][2] * w0 + vertex_colors[1][2] * w1 + vertex_colors[2][2] * w2,
                vertex_colors[0][3] * w0 + vertex_colors[1][3] * w1 + vertex_colors[2][3] * w2,
            ];

            let color = blend_color(sampled, vertex_color);
            output.put_pixel(x as u32, y as u32, color);
        }
    }
}

fn blend_color(sampled: [u8; 4], vertex_color: [f32; 4]) -> Rgba<u8> {
    let mut rgba = [0u8; 4];
    for channel in 0..3 {
        let tex = sampled[channel] as f32 / 255.0;
        let vert = vertex_color[channel].clamp(0.0, 1.0);
        rgba[channel] = (tex * vert * 255.0).clamp(0.0, 255.0) as u8;
    }
    rgba[3] = (sampled[3] as f32 * vertex_color[3].clamp(0.0, 1.0)).clamp(0.0, 255.0) as u8;
    Rgba(rgba)
}

fn sample_texture(texture: &RgbaImage, uv: [f32; 2]) -> [u8; 4] {
    let width = texture.width();
    let height = texture.height();

    let u = uv[0].fract();
    let v = uv[1].fract();
    let u = if u < 0.0 { u + 1.0 } else { u };
    let v = if v < 0.0 { v + 1.0 } else { v };

    let x = (u.clamp(0.0, 1.0) * (width as f32 - 1.0)).round() as u32;
    let y = ((1.0 - v.clamp(0.0, 1.0)) * (height as f32 - 1.0)).round() as u32;

    texture.get_pixel(x, y).0
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

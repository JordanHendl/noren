use std::f32::consts::PI;

use crate::{
    parsing::{
        MaterialLayout, MaterialTextureLookups, MeshLayout, MetaLayout, ModelLayout, TextureLayout,
    },
    rdb::{AudioClip, AudioFormat, HostGeometry, HostImage, ImageInfo, primitives::Vertex},
};

pub const DEFAULT_IMAGE_ENTRY: &str = "imagery/default";
pub const DEFAULT_TEXTURE_ENTRY: &str = "texture/default";
pub const DEFAULT_MATERIAL_ENTRY: &str = "material/default";
pub const DEFAULT_SOUND_ENTRY: &str = "audio/default";
pub const DEFAULT_GEOMETRY_ENTRIES: [&str; 6] = [
    "geometry/sphere",
    "geometry/cube",
    "geometry/quad",
    "geometry/plane",
    "geometry/cylinder",
    "geometry/cone",
];

pub fn default_image() -> HostImage {
    let info = ImageInfo {
        name: DEFAULT_IMAGE_ENTRY.into(),
        dim: [1, 1, 1],
        layers: 1,
        format: dashi::Format::RGBA8,
        mip_levels: 1,
    };

    HostImage::new(info, vec![255, 255, 255, 255])
}

pub fn default_sound() -> AudioClip {
    let data = include_bytes!("../sample/sample_pre/audio/beep.wav").to_vec();
    AudioClip::new(DEFAULT_SOUND_ENTRY.to_string(), AudioFormat::Wav, data)
}

pub fn default_primitives() -> Vec<(String, HostGeometry)> {
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

pub fn inject_default_layouts(meta: &mut MetaLayout) {
    ensure_default_assets(
        &mut meta.textures,
        &mut meta.materials,
        &mut meta.meshes,
        &mut meta.models,
    );
}

pub fn ensure_default_assets(
    textures: &mut std::collections::HashMap<String, TextureLayout>,
    materials: &mut std::collections::HashMap<String, MaterialLayout>,
    meshes: &mut std::collections::HashMap<String, MeshLayout>,
    models: &mut std::collections::HashMap<String, ModelLayout>,
) {
    textures
        .entry(DEFAULT_TEXTURE_ENTRY.into())
        .or_insert(TextureLayout {
            image: DEFAULT_IMAGE_ENTRY.into(),
            name: Some("Default Texture".into()),
        });

    materials
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

        meshes.entry(mesh_key.clone()).or_insert(MeshLayout {
            name: Some(mesh_name.to_string()),
            geometry: geometry.to_string(),
            material: Some(DEFAULT_MATERIAL_ENTRY.into()),
            textures: vec![DEFAULT_TEXTURE_ENTRY.into()],
        });

        models.entry(model_key).or_insert(ModelLayout {
            name: Some(mesh_name.to_string()),
            meshes: vec![mesh_key],
        });
    }
}

fn make_vertex(position: [f32; 3], normal: [f32; 3], uv: [f32; 2]) -> Vertex {
    Vertex {
        position,
        normal,
        tangent: [1.0, 0.0, 0.0, 1.0],
        uv,
        color: [1.0, 1.0, 1.0, 1.0],
        joint_indices: [0; 4],
        joint_weights: [0.0; 4],
    }
}

fn make_geometry(vertices: Vec<Vertex>, indices: Option<Vec<u32>>) -> HostGeometry {
    HostGeometry {
        vertices,
        indices,
        ..Default::default()
    }
    .with_counts()
}

fn make_quad_geometry() -> HostGeometry {
    let vertices = vec![
        make_vertex([-0.5, -0.5, 0.0], [0.0, 0.0, 1.0], [0.0, 0.0]),
        make_vertex([0.5, -0.5, 0.0], [0.0, 0.0, 1.0], [1.0, 0.0]),
        make_vertex([0.5, 0.5, 0.0], [0.0, 0.0, 1.0], [1.0, 1.0]),
        make_vertex([-0.5, 0.5, 0.0], [0.0, 0.0, 1.0], [0.0, 1.0]),
    ];

    let indices = vec![0, 1, 2, 2, 3, 0];

    make_geometry(vertices, Some(indices))
}

fn make_plane_geometry() -> HostGeometry {
    let vertices = vec![
        make_vertex([-0.5, 0.0, -0.5], [0.0, 1.0, 0.0], [0.0, 0.0]),
        make_vertex([0.5, 0.0, -0.5], [0.0, 1.0, 0.0], [1.0, 0.0]),
        make_vertex([0.5, 0.0, 0.5], [0.0, 1.0, 0.0], [1.0, 1.0]),
        make_vertex([-0.5, 0.0, 0.5], [0.0, 1.0, 0.0], [0.0, 1.0]),
    ];

    let indices = vec![0, 1, 2, 2, 3, 0];

    make_geometry(vertices, Some(indices))
}

fn make_cube_geometry(half_extent: f32) -> HostGeometry {
    let positions = [
        (
            [half_extent, half_extent, half_extent],
            [0.0, 0.0, 1.0],
            [1.0, 1.0],
        ),
        (
            [half_extent, -half_extent, half_extent],
            [0.0, 0.0, 1.0],
            [1.0, 0.0],
        ),
        (
            [-half_extent, -half_extent, half_extent],
            [0.0, 0.0, 1.0],
            [0.0, 0.0],
        ),
        (
            [-half_extent, half_extent, half_extent],
            [0.0, 0.0, 1.0],
            [0.0, 1.0],
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
            [half_extent, half_extent, half_extent],
            [1.0, 0.0, 0.0],
            [1.0, 0.0],
        ),
        (
            [half_extent, -half_extent, half_extent],
            [1.0, 0.0, 0.0],
            [1.0, 1.0],
        ),
        (
            [half_extent, -half_extent, -half_extent],
            [1.0, 0.0, 0.0],
            [0.0, 1.0],
        ),
        (
            [half_extent, half_extent, -half_extent],
            [1.0, 0.0, 0.0],
            [0.0, 0.0],
        ),
        (
            [-half_extent, half_extent, half_extent],
            [-1.0, 0.0, 0.0],
            [0.0, 0.0],
        ),
        (
            [-half_extent, -half_extent, half_extent],
            [-1.0, 0.0, 0.0],
            [0.0, 1.0],
        ),
        (
            [-half_extent, -half_extent, -half_extent],
            [-1.0, 0.0, 0.0],
            [1.0, 1.0],
        ),
        (
            [-half_extent, half_extent, -half_extent],
            [-1.0, 0.0, 0.0],
            [1.0, 0.0],
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

    make_geometry(vertices, Some(indices))
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

    make_geometry(
        vertices,
        Some(indices.into_iter().map(|i| i as u32).collect()),
    )
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
        let bottom = i;
        let bottom_next = next;
        indices.extend_from_slice(&[bottom_center_index, bottom as u32, bottom_next as u32]);
    }

    make_geometry(vertices, Some(indices))
}

fn make_cone_geometry(radius: f32, height: f32, segments: u32) -> HostGeometry {
    let mut vertices = Vec::new();
    let mut indices: Vec<u32> = Vec::new();
    let half_height = height * 0.5;

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

        vertices.push(make_vertex(
            [x, -half_height, z],
            [c, radius / height, s],
            [frac, 0.0],
        ));
    }

    for i in 0..segments {
        let next = (i + 1) % segments;
        indices.extend_from_slice(&[0, (i + 1) as u32, (next + 1) as u32]);
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

    make_geometry(vertices, Some(indices))
}

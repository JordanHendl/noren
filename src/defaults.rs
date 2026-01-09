use std::f32::consts::PI;

use gltf::animation::util::ReadOutputs;

use crate::{
    parsing::{
        MaterialLayout, MaterialTextureLookups, MeshLayout, MetaLayout, ModelLayout, TextureLayout,
    },
    rdb::{
        AnimationChannel, AnimationClip, AnimationInterpolation, AnimationOutput, AnimationSampler,
        AnimationTargetPath, AudioClip, AudioFormat, HostGeometry, HostImage, ImageInfo, Joint,
        Skeleton, index_vertices, primitives::Vertex,
    },
};

pub const DEFAULT_IMAGE_ENTRY: &str = "imagery/default";
pub const DEFAULT_TEXTURE_ENTRY: &str = "texture/default";
pub const DEFAULT_MATERIAL_ENTRY: &str = "material/default";
pub const DEFAULT_SOUND_ENTRY: &str = "audio/beep";
pub const DEFAULT_SOUND_ENTRIES: [&str; 2] = ["audio/beep", "audio/tone"];
pub const DEFAULT_SKELETON_ENTRY: &str = "skeletons/fox";
pub const DEFAULT_ANIMATION_ENTRY: &str = "animations/fox";
pub const DEFAULT_GEOMETRY_ENTRIES: [&str; 7] = [
    "geometry/sphere",
    "geometry/cube",
    "geometry/quad",
    "geometry/plane",
    "geometry/cylinder",
    "geometry/cone",
    "geometry/fox",
];
const WITCH_GEOMETRY_PREFIX: &str = "geometry/witch";
const WITCH_MATERIAL_PREFIX: &str = "material/witch";
const WITCH_MESH_PREFIX: &str = "mesh/witch";
const WITCH_MODEL_ENTRY: &str = "model/witch";
const FOX_MATERIAL_ENTRY: &str = "material/fox";

#[derive(Copy, Clone)]
struct WitchPrimitiveDef {
    mesh_index: usize,
    primitive_index: usize,
    mesh_name: &'static str,
    material_index: usize,
}

const WITCH_MATERIAL_NAMES: [&str; 13] = [
    "nails",
    "Material",
    "legs",
    "Material.006",
    "face txt",
    "neck/ skin shading",
    "Material.007",
    "tights",
    "sweater",
    "Material.009",
    "skin",
    "hat",
    "Material.004",
];

const WITCH_PRIMITIVES: [WitchPrimitiveDef; 18] = [
    WitchPrimitiveDef {
        mesh_index: 0,
        primitive_index: 0,
        mesh_name: "Cube",
        material_index: 0,
    },
    WitchPrimitiveDef {
        mesh_index: 0,
        primitive_index: 1,
        mesh_name: "Cube",
        material_index: 1,
    },
    WitchPrimitiveDef {
        mesh_index: 0,
        primitive_index: 2,
        mesh_name: "Cube",
        material_index: 2,
    },
    WitchPrimitiveDef {
        mesh_index: 0,
        primitive_index: 3,
        mesh_name: "Cube",
        material_index: 3,
    },
    WitchPrimitiveDef {
        mesh_index: 0,
        primitive_index: 4,
        mesh_name: "Cube",
        material_index: 4,
    },
    WitchPrimitiveDef {
        mesh_index: 0,
        primitive_index: 5,
        mesh_name: "Cube",
        material_index: 5,
    },
    WitchPrimitiveDef {
        mesh_index: 0,
        primitive_index: 6,
        mesh_name: "Cube",
        material_index: 6,
    },
    WitchPrimitiveDef {
        mesh_index: 0,
        primitive_index: 7,
        mesh_name: "Cube",
        material_index: 7,
    },
    WitchPrimitiveDef {
        mesh_index: 1,
        primitive_index: 0,
        mesh_name: "Cube.014",
        material_index: 8,
    },
    WitchPrimitiveDef {
        mesh_index: 1,
        primitive_index: 1,
        mesh_name: "Cube.014",
        material_index: 3,
    },
    WitchPrimitiveDef {
        mesh_index: 1,
        primitive_index: 2,
        mesh_name: "Cube.014",
        material_index: 9,
    },
    WitchPrimitiveDef {
        mesh_index: 1,
        primitive_index: 3,
        mesh_name: "Cube.014",
        material_index: 10,
    },
    WitchPrimitiveDef {
        mesh_index: 1,
        primitive_index: 4,
        mesh_name: "Cube.014",
        material_index: 6,
    },
    WitchPrimitiveDef {
        mesh_index: 2,
        primitive_index: 0,
        mesh_name: "Cube.003",
        material_index: 1,
    },
    WitchPrimitiveDef {
        mesh_index: 3,
        primitive_index: 0,
        mesh_name: "Circle.004",
        material_index: 11,
    },
    WitchPrimitiveDef {
        mesh_index: 3,
        primitive_index: 1,
        mesh_name: "Circle.004",
        material_index: 6,
    },
    WitchPrimitiveDef {
        mesh_index: 3,
        primitive_index: 2,
        mesh_name: "Circle.004",
        material_index: 9,
    },
    WitchPrimitiveDef {
        mesh_index: 3,
        primitive_index: 3,
        mesh_name: "Circle.004",
        material_index: 12,
    },
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

pub fn default_sounds() -> Vec<AudioClip> {
    vec![
        AudioClip::new(
            DEFAULT_SOUND_ENTRIES[0].to_string(),
            AudioFormat::Wav,
            include_bytes!("../sample/sample_pre/audio/beep.wav").to_vec(),
        ),
        AudioClip::new(
            DEFAULT_SOUND_ENTRIES[1].to_string(),
            AudioFormat::Wav,
            include_bytes!("../sample/sample_pre/audio/tone.wav").to_vec(),
        ),
    ]
}

pub fn default_primitives() -> Vec<(String, HostGeometry)> {
    let [sphere, cube, quad, plane, cylinder, cone, fox] = DEFAULT_GEOMETRY_ENTRIES;

    let mut entries = vec![
        (sphere.into(), make_sphere_geometry(0.5, 32, 16)),
        (cube.into(), make_cube_geometry(0.5)),
        (quad.into(), make_quad_geometry()),
        (plane.into(), make_plane_geometry()),
        (cylinder.into(), make_cylinder_geometry(0.5, 1.0, 32)),
        (cone.into(), make_cone_geometry(0.5, 1.0, 32)),
        (fox.into(), load_default_fox_geometry()),
    ];

    entries.extend(load_default_witch_geometries());

    entries
}

pub fn default_skeletons() -> Vec<(String, Skeleton)> {
    vec![(DEFAULT_SKELETON_ENTRY.to_string(), load_default_fox_skeleton())]
}

pub fn default_animations() -> Vec<(String, AnimationClip)> {
    load_default_fox_animations()
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
        let (material, textures) = if geometry == "geometry/fox" {
            (
                Some(FOX_MATERIAL_ENTRY.to_string()),
                Vec::new(),
            )
        } else {
            (
                Some(DEFAULT_MATERIAL_ENTRY.into()),
                vec![DEFAULT_TEXTURE_ENTRY.into()],
            )
        };

        meshes.entry(mesh_key.clone()).or_insert(MeshLayout {
            name: Some(mesh_name.to_string()),
            geometry: geometry.to_string(),
            material,
            textures,
        });

        models.entry(model_key).or_insert(ModelLayout {
            name: Some(mesh_name.to_string()),
            meshes: vec![mesh_key],
        });
    }

    materials
        .entry(FOX_MATERIAL_ENTRY.into())
        .or_insert(MaterialLayout {
            name: Some("fox_material".into()),
            render_mask: 0,
            texture_lookups: MaterialTextureLookups::default(),
        });

    let mut witch_meshes = Vec::new();
    for primitive in WITCH_PRIMITIVES {
        let mesh_slug = slugify(primitive.mesh_name);
        let geometry_key =
            format!("{WITCH_GEOMETRY_PREFIX}/{mesh_slug}/{}", primitive.primitive_index);
        let mesh_key = format!("{WITCH_MESH_PREFIX}/{mesh_slug}/{}", primitive.primitive_index);
        let material_name = WITCH_MATERIAL_NAMES[primitive.material_index];
        let material_key = format!("{WITCH_MATERIAL_PREFIX}/{}", slugify(material_name));

        materials
            .entry(material_key.clone())
            .or_insert(MaterialLayout {
                name: Some(material_name.to_string()),
                render_mask: 0,
                texture_lookups: MaterialTextureLookups::default(),
            });

        meshes.entry(mesh_key.clone()).or_insert(MeshLayout {
            name: Some(format!("Witch {mesh_slug} {}", primitive.primitive_index)),
            geometry: geometry_key,
            material: Some(material_key),
            textures: Vec::new(),
        });

        witch_meshes.push(mesh_key);
    }

    models.entry(WITCH_MODEL_ENTRY.into()).or_insert(ModelLayout {
        name: Some("witch".into()),
        meshes: witch_meshes,
    });
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

fn load_default_fox_geometry() -> HostGeometry {
    let (doc, buffers, _) =
        gltf::import_slice(include_bytes!("../sample/sample_pre/gltf/Fox.glb"))
            .expect("load embedded fox glb");
    let mesh = doc
        .meshes()
        .next()
        .expect("embedded fox glb missing meshes");
    let primitive = mesh
        .primitives()
        .next()
        .expect("embedded fox glb missing primitives");
    load_geometry_from_primitive(primitive, &buffers)
}

fn load_default_witch_geometries() -> Vec<(String, HostGeometry)> {
    let (doc, buffers, _) =
        gltf::import_slice(include_bytes!("../sample/sample_pre/gltf/Witch.glb"))
            .expect("load embedded witch glb");
    let meshes: Vec<_> = doc.meshes().collect();
    let mut geometries = Vec::with_capacity(WITCH_PRIMITIVES.len());

    for primitive in WITCH_PRIMITIVES {
        let mesh = meshes
            .get(primitive.mesh_index)
            .expect("embedded witch glb missing mesh");
        let gltf_primitive = mesh
            .primitives()
            .nth(primitive.primitive_index)
            .expect("embedded witch glb missing primitive");
        let geometry = load_geometry_from_primitive(gltf_primitive, &buffers);
        let mesh_slug = slugify(primitive.mesh_name);
        let entry = format!(
            "{WITCH_GEOMETRY_PREFIX}/{mesh_slug}/{}",
            primitive.primitive_index
        );
        geometries.push((entry, geometry));
    }

    geometries
}

fn load_geometry_from_primitive(
    primitive: gltf::Primitive,
    buffers: &[gltf::buffer::Data],
) -> HostGeometry {
    let reader = primitive.reader(|buffer| Some(&buffers[buffer.index()].0[..]));

    let positions: Vec<[f32; 3]> = reader
        .read_positions()
        .expect("embedded fox glb missing POSITION attribute")
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
    let joints: Vec<[u32; 4]> = reader
        .read_joints(0)
        .map(|iter| {
            iter.into_u16()
                .map(|joint| joint.map(u32::from))
                .collect()
        })
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
            color: colors.get(idx).copied().unwrap_or([1.0, 1.0, 1.0, 1.0]),
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

    HostGeometry {
        vertices,
        indices: Some(indices),
        ..Default::default()
    }
    .with_counts()
}

fn load_default_fox_skeleton() -> Skeleton {
    let (doc, buffers, _) =
        gltf::import_slice(include_bytes!("../sample/sample_pre/gltf/Fox.glb"))
            .expect("load embedded fox glb");
    let skin = doc
        .skins()
        .next()
        .expect("embedded fox glb missing skins");

    let joints: Vec<_> = skin.joints().collect();
    assert!(!joints.is_empty(), "embedded fox glb skin has no joints");

    let mut node_to_joint = std::collections::HashMap::new();
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
        .unwrap_or_else(|| DEFAULT_SKELETON_ENTRY.to_string());

    Skeleton {
        name,
        joints: parsed_joints,
        root,
        data: Vec::new(),
    }
}

fn load_default_fox_animations() -> Vec<(String, AnimationClip)> {
    let (doc, buffers, _) =
        gltf::import_slice(include_bytes!("../sample/sample_pre/gltf/Fox.glb"))
            .expect("load embedded fox glb");
    let node_to_joint = doc
        .skins()
        .next()
        .map(|skin| {
            skin.joints()
                .enumerate()
                .map(|(idx, joint)| (joint.index(), idx))
                .collect::<std::collections::HashMap<_, _>>()
        })
        .unwrap_or_default();
    let mut entries = Vec::new();

    for (index, animation) in doc.animations().enumerate() {
        let clip_name = animation
            .name()
            .map(|n| n.to_string())
            .unwrap_or_else(|| format!("fox_animation_{index}"));
        let entry_key = animation
            .name()
            .map(|name| format!("animations/fox/{}", slugify(name)))
            .unwrap_or_else(|| format!("animations/fox/{index}"));
        let clip = build_animation_clip(animation, &buffers, &node_to_joint, &clip_name);
        entries.push((entry_key, clip));
    }

    if let Some((_, clip)) = entries.first() {
        entries.insert(
            0,
            (DEFAULT_ANIMATION_ENTRY.to_string(), clip.clone()),
        );
    }

    entries
}

fn build_animation_clip(
    animation: gltf::Animation,
    buffers: &[gltf::buffer::Data],
    node_to_joint: &std::collections::HashMap<usize, usize>,
    name: &str,
) -> AnimationClip {
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
                .expect("embedded fox glb missing animation input keyframes")
                .collect();

            let output = reader
                .read_outputs()
                .expect("embedded fox glb missing animation output values");

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
        .map(|sampler| {
            sampler.expect("embedded fox glb animation sampler referenced by channel")
        })
        .collect();

    let duration_seconds = samplers
        .iter()
        .flat_map(|sampler| sampler.input.iter().copied())
        .fold(0.0, f32::max);

    AnimationClip {
        name: name.to_string(),
        duration_seconds,
        samplers,
        channels,
        data: Vec::new(),
    }
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
        "material".to_string()
    } else {
        out
    }
}

fn identity_matrix() -> [[f32; 4]; 4] {
    [
        [1.0, 0.0, 0.0, 0.0],
        [0.0, 1.0, 0.0, 0.0],
        [0.0, 0.0, 1.0, 0.0],
        [0.0, 0.0, 0.0, 1.0],
    ]
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

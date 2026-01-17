use std::collections::BTreeMap;

use serde::Serialize;

use crate::{
    RDBEntryMeta, RDBFile, RdbErr,
    rdb::{
        primitives::Vertex,
        terrain::{
            TERRAIN_DIRTY_GENERATOR, TERRAIN_DIRTY_MUTATION, TERRAIN_DIRTY_SETTINGS,
            TerrainChunkArtifact, TerrainChunkDependencyHashes, TerrainChunkLodHash,
            TerrainChunkState, TerrainDirtyReason, TerrainGeneratorDefinition,
            TerrainMutationLayer, TerrainProjectSettings, TerrainVertexLayout,
            chunk_artifact_entry, chunk_coord_key, chunk_state_entry, generator_entry, lod_key,
            mutation_layer_entry, project_settings_entry,
        },
    },
};

#[derive(Clone, Copy, Debug)]
pub struct TerrainChunkBuildRequest {
    pub chunk_coords: [i32; 2],
    pub lod: u8,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct TerrainBuildReport {
    pub built_chunks: usize,
    pub skipped_chunks: usize,
    pub updated_states: usize,
}

#[derive(Clone, Debug, Serialize)]
struct TerrainChunkHashInput<'a> {
    settings: TerrainProjectSettingsHashInput<'a>,
    generator: &'a TerrainGeneratorDefinition,
    mutation_layers: &'a [TerrainMutationLayer],
    chunk_coords: [i32; 2],
    lod: u8,
}

#[derive(Clone, Debug, Serialize)]
struct TerrainProjectSettingsHashInput<'a> {
    name: &'a str,
    seed: u64,
    tile_size: f32,
    tiles_per_chunk: [u32; 2],
    world_bounds_min: [f32; 3],
    world_bounds_max: [f32; 3],
    lod_policy: &'a crate::rdb::terrain::TerrainLodPolicy,
    generator_graph_id: &'a str,
    vertex_layout: &'a TerrainVertexLayout,
}

#[derive(Clone, Debug, Serialize)]
struct TerrainMutationHashInput<'a> {
    mutation_layers: &'a [TerrainMutationLayer],
}

pub fn build_terrain_chunks(
    rdb: &mut RDBFile,
    project_key: &str,
    requests: &[TerrainChunkBuildRequest],
) -> Result<TerrainBuildReport, RdbErr> {
    if requests.is_empty() {
        return Ok(TerrainBuildReport::default());
    }

    let settings = rdb.fetch::<TerrainProjectSettings>(&project_settings_entry(project_key))?;
    let generator = rdb.fetch::<TerrainGeneratorDefinition>(&generator_entry(
        project_key,
        settings.active_generator_version,
    ))?;
    let mutation_layers = collect_active_mutation_layers(
        rdb.entries(),
        rdb,
        project_key,
        settings.active_mutation_version,
    )?;

    let settings_hash = hash_serialize(&settings_hash_input(&settings));
    let generator_hash = hash_serialize(&generator);

    let mut report = TerrainBuildReport::default();

    for request in requests {
        let coord_key = chunk_coord_key(request.chunk_coords[0], request.chunk_coords[1]);
        let state_key = chunk_state_entry(project_key, &coord_key);
        let artifact_key = chunk_artifact_entry(project_key, &coord_key, &lod_key(request.lod));

        let relevant_layers = relevant_mutation_layers(&mutation_layers, request.chunk_coords);
        let mutation_hash = hash_serialize(&TerrainMutationHashInput {
            mutation_layers: &relevant_layers,
        });
        let content_hash = hash_serialize(&TerrainChunkHashInput {
            settings: settings_hash_input(&settings),
            generator: &generator,
            mutation_layers: &relevant_layers,
            chunk_coords: request.chunk_coords,
            lod: request.lod,
        });

        let existing_state = rdb.fetch::<TerrainChunkState>(&state_key).ok();
        let mut dirty_flags = 0;
        let mut dirty_reasons = Vec::new();
        if let Some(state) = &existing_state {
            if state.dependency_hashes.settings_hash != settings_hash {
                dirty_flags |= TERRAIN_DIRTY_SETTINGS;
                dirty_reasons.push(TerrainDirtyReason::SettingsChanged);
            }
            if state.dependency_hashes.generator_hash != generator_hash {
                dirty_flags |= TERRAIN_DIRTY_GENERATOR;
                dirty_reasons.push(TerrainDirtyReason::GeneratorChanged);
            }
            if state.dependency_hashes.mutation_hash != mutation_hash {
                dirty_flags |= TERRAIN_DIRTY_MUTATION;
                dirty_reasons.push(TerrainDirtyReason::MutationChanged);
            }
        } else {
            dirty_flags = TERRAIN_DIRTY_SETTINGS | TERRAIN_DIRTY_GENERATOR | TERRAIN_DIRTY_MUTATION;
            dirty_reasons = vec![
                TerrainDirtyReason::SettingsChanged,
                TerrainDirtyReason::GeneratorChanged,
                TerrainDirtyReason::MutationChanged,
            ];
        }

        let existing_hash = existing_state
            .as_ref()
            .and_then(|state| {
                state
                    .last_built_hashes
                    .iter()
                    .find(|h| h.lod == request.lod)
            })
            .map(|h| h.hash);
        if existing_hash == Some(content_hash) && dirty_flags == 0 {
            report.skipped_chunks += 1;
            continue;
        }

        let artifact = build_chunk_artifact(
            project_key,
            &settings,
            &generator,
            &relevant_layers,
            request.chunk_coords,
            request.lod,
            content_hash,
        );
        rdb.upsert(&artifact_key, &artifact)?;

        let mut last_built_hashes = existing_state
            .as_ref()
            .map(|state| state.last_built_hashes.clone())
            .unwrap_or_default();
        upsert_lod_hash(&mut last_built_hashes, request.lod, content_hash);

        let state = TerrainChunkState {
            project_key: project_key.to_string(),
            chunk_coords: request.chunk_coords,
            dirty_flags,
            dirty_reasons,
            generator_version: settings.active_generator_version,
            mutation_version: settings.active_mutation_version,
            last_built_hashes,
            dependency_hashes: TerrainChunkDependencyHashes {
                settings_hash,
                generator_hash,
                mutation_hash,
            },
        };
        rdb.upsert(&state_key, &state)?;

        report.built_chunks += 1;
        report.updated_states += 1;
    }

    Ok(report)
}

fn collect_active_mutation_layers(
    entries: Vec<RDBEntryMeta>,
    rdb: &mut RDBFile,
    project_key: &str,
    active_version: u32,
) -> Result<Vec<TerrainMutationLayer>, RdbErr> {
    let prefix = format!("terrain/mutation_layer/{project_key}/");
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
        if let Ok(layer) = rdb.fetch::<TerrainMutationLayer>(&entry) {
            layers.push(layer);
        }
    }
    layers.sort_by(|a, b| {
        a.order
            .cmp(&b.order)
            .then_with(|| a.layer_id.cmp(&b.layer_id))
    });
    Ok(layers)
}

fn parse_mutation_layer_entry(name: &str, prefix: &str) -> Option<(String, u32)> {
    let remainder = name.strip_prefix(prefix)?;
    let mut parts = remainder.split('/');
    let layer_id = parts.next()?.to_string();
    let version_part = parts.next()?;
    let version = version_part.strip_prefix('v')?.parse().ok()?;
    Some((layer_id, version))
}

fn relevant_mutation_layers(
    layers: &[TerrainMutationLayer],
    chunk_coords: [i32; 2],
) -> Vec<TerrainMutationLayer> {
    layers
        .iter()
        .filter(|layer| layer_affects_chunk(layer, chunk_coords))
        .cloned()
        .collect()
}

fn layer_affects_chunk(layer: &TerrainMutationLayer, chunk_coords: [i32; 2]) -> bool {
    match &layer.affected_chunks {
        Some(list) if !list.is_empty() => list.iter().any(|coords| *coords == chunk_coords),
        _ => true,
    }
}

fn upsert_lod_hash(hashes: &mut Vec<TerrainChunkLodHash>, lod: u8, hash: u64) {
    if let Some(entry) = hashes.iter_mut().find(|entry| entry.lod == lod) {
        entry.hash = hash;
    } else {
        hashes.push(TerrainChunkLodHash { lod, hash });
    }
    hashes.sort_by_key(|entry| entry.lod);
}

fn build_chunk_artifact(
    project_key: &str,
    settings: &TerrainProjectSettings,
    generator: &TerrainGeneratorDefinition,
    mutation_layers: &[TerrainMutationLayer],
    chunk_coords: [i32; 2],
    lod: u8,
    content_hash: u64,
) -> TerrainChunkArtifact {
    let (vertices, indices, bounds_min, bounds_max) =
        generate_chunk_geometry(settings, generator, mutation_layers, chunk_coords, lod);
    TerrainChunkArtifact {
        project_key: project_key.to_string(),
        chunk_coords,
        lod,
        bounds_min,
        bounds_max,
        vertex_layout: settings.vertex_layout.clone(),
        vertices,
        indices,
        material_ids: None,
        material_weights: None,
        content_hash,
        mesh_entry: "geometry/terrain_chunk".to_string(),
    }
}

fn generate_chunk_geometry(
    settings: &TerrainProjectSettings,
    generator: &TerrainGeneratorDefinition,
    mutation_layers: &[TerrainMutationLayer],
    chunk_coords: [i32; 2],
    lod: u8,
) -> (Vec<Vertex>, Vec<u32>, [f32; 3], [f32; 3]) {
    let step = 1_u32.checked_shl(lod as u32).unwrap_or(1).max(1);
    let tiles_x = settings.tiles_per_chunk[0];
    let tiles_y = settings.tiles_per_chunk[1];
    let grid_x = tiles_x / step + 1;
    let grid_y = tiles_y / step + 1;

    let origin_x = chunk_coords[0] as f32 * tiles_x as f32 * settings.tile_size;
    let origin_y = chunk_coords[1] as f32 * tiles_y as f32 * settings.tile_size;

    let mut heights = vec![0.0_f32; (grid_x * grid_y) as usize];
    for y in 0..grid_y {
        for x in 0..grid_x {
            let world_x = origin_x + x as f32 * step as f32 * settings.tile_size;
            let world_y = origin_y + y as f32 * step as f32 * settings.tile_size;
            let idx = (y * grid_x + x) as usize;
            heights[idx] = sample_height(settings, generator, mutation_layers, world_x, world_y);
        }
    }

    let mut vertices = Vec::with_capacity((grid_x * grid_y) as usize);
    let mut min_bounds = [f32::MAX; 3];
    let mut max_bounds = [f32::MIN; 3];
    for y in 0..grid_y {
        for x in 0..grid_x {
            let world_x = origin_x + x as f32 * step as f32 * settings.tile_size;
            let world_y = origin_y + y as f32 * step as f32 * settings.tile_size;
            let height = heights[(y * grid_x + x) as usize];
            let normal = estimate_normal(grid_x, grid_y, x, y, &heights, settings, step);
            let position = [world_x, world_y, height];
            let uv = [
                x as f32 / (grid_x.saturating_sub(1).max(1)) as f32,
                y as f32 / (grid_y.saturating_sub(1).max(1)) as f32,
            ];
            update_bounds(&mut min_bounds, &mut max_bounds, &position);
            vertices.push(Vertex {
                position,
                normal,
                tangent: [1.0, 0.0, 0.0, 1.0],
                uv,
                color: [1.0, 1.0, 1.0, 1.0],
                joint_indices: [0; 4],
                joint_weights: [0.0; 4],
            });
        }
    }

    let mut indices = Vec::new();
    for y in 0..grid_y.saturating_sub(1) {
        for x in 0..grid_x.saturating_sub(1) {
            let i0 = (y * grid_x + x) as u32;
            let i1 = (y * grid_x + x + 1) as u32;
            let i2 = ((y + 1) * grid_x + x) as u32;
            let i3 = ((y + 1) * grid_x + x + 1) as u32;
            indices.extend_from_slice(&[i0, i2, i1, i1, i2, i3]);
        }
    }

    match settings.vertex_layout {
        TerrainVertexLayout::Standard => (vertices, indices, min_bounds, max_bounds),
    }
}

fn update_bounds(min_bounds: &mut [f32; 3], max_bounds: &mut [f32; 3], position: &[f32; 3]) {
    for i in 0..3 {
        min_bounds[i] = min_bounds[i].min(position[i]);
        max_bounds[i] = max_bounds[i].max(position[i]);
    }
}

fn estimate_normal(
    grid_x: u32,
    grid_y: u32,
    x: u32,
    y: u32,
    heights: &[f32],
    settings: &TerrainProjectSettings,
    step: u32,
) -> [f32; 3] {
    let left = x.saturating_sub(1);
    let right = (x + 1).min(grid_x - 1);
    let down = y.saturating_sub(1);
    let up = (y + 1).min(grid_y - 1);

    let h_l = heights[(y * grid_x + left) as usize];
    let h_r = heights[(y * grid_x + right) as usize];
    let h_d = heights[(down * grid_x + x) as usize];
    let h_u = heights[(up * grid_x + x) as usize];

    let scale = settings.tile_size * step as f32;
    let dx = (h_l - h_r) / scale.max(0.001);
    let dy = (h_d - h_u) / scale.max(0.001);

    let mut nx = dx;
    let mut ny = dy;
    let mut nz = 1.0;
    let length = (nx * nx + ny * ny + nz * nz).sqrt().max(0.001);
    nx /= length;
    ny /= length;
    nz /= length;
    [nx, ny, nz]
}

fn sample_height(
    settings: &TerrainProjectSettings,
    generator: &TerrainGeneratorDefinition,
    mutation_layers: &[TerrainMutationLayer],
    world_x: f32,
    world_y: f32,
) -> f32 {
    let seed = settings.seed as f32;
    let freq = generator.frequency.max(0.0001);
    let base =
        ((world_x * freq + seed).sin() + (world_y * freq + seed).cos()) * generator.amplitude;
    let mut height = base;
    for layer in mutation_layers {
        for op in &layer.ops {
            if !op.enabled || op.weight == 0.0 {
                continue;
            }
            let op_hash = fnv1a64(op.op_id.as_bytes());
            let offset = ((op_hash as f32 / u64::MAX as f32) - 0.5) * op.weight;
            height += offset * generator.amplitude * 0.25;
        }
    }
    height
}

fn hash_serialize<T: Serialize>(value: &T) -> u64 {
    let bytes = bincode::serialize(value).unwrap_or_default();
    fnv1a64(&bytes)
}

fn fnv1a64(bytes: &[u8]) -> u64 {
    let mut hash = 0xcbf29ce484222325_u64;
    for byte in bytes {
        hash ^= *byte as u64;
        hash = hash.wrapping_mul(0x100000001b3);
    }
    hash
}

fn settings_hash_input(settings: &TerrainProjectSettings) -> TerrainProjectSettingsHashInput<'_> {
    TerrainProjectSettingsHashInput {
        name: &settings.name,
        seed: settings.seed,
        tile_size: settings.tile_size,
        tiles_per_chunk: settings.tiles_per_chunk,
        world_bounds_min: settings.world_bounds_min,
        world_bounds_max: settings.world_bounds_max,
        lod_policy: &settings.lod_policy,
        generator_graph_id: &settings.generator_graph_id,
        vertex_layout: &settings.vertex_layout,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rdb::terrain::{TerrainMutationLayer, TerrainMutationOp, TerrainProjectSettings};

    fn seed_rdb(project_key: &str) -> RDBFile {
        let mut rdb = RDBFile::new();
        let settings = TerrainProjectSettings::default();
        let generator = TerrainGeneratorDefinition::default();
        let layer = TerrainMutationLayer::new("layer-a", "Layer A", 0)
            .with_op(TerrainMutationOp::new("raise"));
        rdb.add(&project_settings_entry(project_key), &settings)
            .expect("settings");
        rdb.add(
            &generator_entry(project_key, settings.active_generator_version),
            &generator,
        )
        .expect("generator");
        rdb.add(
            &mutation_layer_entry(
                project_key,
                &layer.layer_id,
                settings.active_mutation_version,
            ),
            &layer,
        )
        .expect("mutation");
        rdb
    }

    #[test]
    fn rebuild_skips_when_hash_matches() {
        let mut rdb = seed_rdb("sample");
        let request = TerrainChunkBuildRequest {
            chunk_coords: [0, 0],
            lod: 0,
        };
        let report = build_terrain_chunks(&mut rdb, "sample", &[request]).expect("build");
        assert_eq!(report.built_chunks, 1);

        let report_again = build_terrain_chunks(&mut rdb, "sample", &[request]).expect("build");
        assert_eq!(report_again.built_chunks, 0);
        assert_eq!(report_again.skipped_chunks, 1);
    }

    #[test]
    fn mutation_changes_only_dirties_affected_chunks() {
        let project_key = "sample";
        let mut rdb = RDBFile::new();
        let mut settings = TerrainProjectSettings::default();
        let generator = TerrainGeneratorDefinition::default();

        let mut layer = TerrainMutationLayer::new("layer-a", "Layer A", 0)
            .with_op(TerrainMutationOp::new("raise"));
        layer.affected_chunks = Some(vec![[0, 0]]);

        rdb.add(&project_settings_entry(project_key), &settings)
            .expect("settings");
        rdb.add(
            &generator_entry(project_key, settings.active_generator_version),
            &generator,
        )
        .expect("generator");
        rdb.add(
            &mutation_layer_entry(
                project_key,
                &layer.layer_id,
                settings.active_mutation_version,
            ),
            &layer,
        )
        .expect("mutation");

        let requests = [
            TerrainChunkBuildRequest {
                chunk_coords: [0, 0],
                lod: 0,
            },
            TerrainChunkBuildRequest {
                chunk_coords: [1, 0],
                lod: 0,
            },
        ];
        let report = build_terrain_chunks(&mut rdb, project_key, &requests).expect("build");
        assert_eq!(report.built_chunks, 2);

        let coord_key_00 = chunk_coord_key(0, 0);
        let coord_key_10 = chunk_coord_key(1, 0);
        let artifact_key_00 = chunk_artifact_entry(project_key, &coord_key_00, "lod0");
        let artifact_key_10 = chunk_artifact_entry(project_key, &coord_key_10, "lod0");
        let artifact_00_before = rdb
            .fetch::<TerrainChunkArtifact>(&artifact_key_00)
            .expect("artifact");
        let artifact_10_before = rdb
            .fetch::<TerrainChunkArtifact>(&artifact_key_10)
            .expect("artifact");

        let mut updated_layer = layer.clone();
        updated_layer.ops[0].weight = 2.0;
        settings.active_mutation_version += 1;
        rdb.upsert(&project_settings_entry(project_key), &settings)
            .expect("settings");
        rdb.add(
            &mutation_layer_entry(
                project_key,
                &updated_layer.layer_id,
                settings.active_mutation_version,
            ),
            &updated_layer,
        )
        .expect("mutation");

        let report = build_terrain_chunks(&mut rdb, project_key, &requests).expect("build");
        assert_eq!(report.built_chunks, 1);
        assert_eq!(report.skipped_chunks, 1);

        let artifact_00_after = rdb
            .fetch::<TerrainChunkArtifact>(&artifact_key_00)
            .expect("artifact");
        let artifact_10_after = rdb
            .fetch::<TerrainChunkArtifact>(&artifact_key_10)
            .expect("artifact");
        assert_ne!(
            artifact_00_before.content_hash,
            artifact_00_after.content_hash
        );
        assert_eq!(
            artifact_10_before.content_hash,
            artifact_10_after.content_hash
        );
    }
}

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
            TerrainMaterialBlendMode, TerrainMaterialRule, TerrainMutationLayer, TerrainMutationOp,
            TerrainMutationOpKind, TerrainMutationParams, TerrainProjectSettings,
            TerrainVertexLayout, chunk_artifact_entry, chunk_coord_key, chunk_state_entry,
            generator_entry, lod_key, mutation_layer_entry, project_settings_entry,
        },
    },
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
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

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TerrainChunkBuildPhase {
    FieldEval,
    SurfaceExtraction,
    Optimize,
    Write,
}

impl TerrainChunkBuildPhase {
    pub fn label(self) -> &'static str {
        match self {
            Self::FieldEval => "field eval",
            Self::SurfaceExtraction => "surface extraction",
            Self::Optimize => "optimize",
            Self::Write => "write",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TerrainChunkBuildStatus {
    Built,
    Skipped,
    Cancelled,
}

#[derive(Clone, Debug)]
pub struct TerrainChunkBuildOutcome {
    pub status: TerrainChunkBuildStatus,
    pub artifact: Option<TerrainChunkArtifact>,
    pub state: Option<TerrainChunkState>,
}

#[derive(Clone, Debug)]
pub struct TerrainBuildContext {
    pub settings: TerrainProjectSettings,
    pub generator: TerrainGeneratorDefinition,
    pub mutation_layers: Vec<TerrainMutationLayer>,
    pub settings_hash: u64,
    pub generator_hash: u64,
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

    let context = prepare_terrain_build_context(rdb, project_key)?;
    let mut report = TerrainBuildReport::default();

    for request in requests {
        let outcome = build_terrain_chunk_with_context(
            rdb,
            project_key,
            &context,
            *request,
            |_| {},
            || false,
        )?;
        match outcome.status {
            TerrainChunkBuildStatus::Built => {
                let coord_key = chunk_coord_key(request.chunk_coords[0], request.chunk_coords[1]);
                let state_key = chunk_state_entry(project_key, &coord_key);
                let artifact_key =
                    chunk_artifact_entry(project_key, &coord_key, &lod_key(request.lod));
                if let Some(artifact) = &outcome.artifact {
                    rdb.upsert(&artifact_key, artifact)?;
                }
                if let Some(state) = &outcome.state {
                    rdb.upsert(&state_key, state)?;
                    report.updated_states += 1;
                }
                report.built_chunks += 1;
            }
            TerrainChunkBuildStatus::Skipped => {
                report.skipped_chunks += 1;
            }
            TerrainChunkBuildStatus::Cancelled => {}
        }
    }

    Ok(report)
}

pub fn prepare_terrain_build_context(
    rdb: &mut RDBFile,
    project_key: &str,
) -> Result<TerrainBuildContext, RdbErr> {
    let settings = rdb.fetch::<TerrainProjectSettings>(&project_settings_entry(project_key))?;
    let generator = rdb.fetch::<TerrainGeneratorDefinition>(&generator_entry(
        project_key,
        settings.active_generator_version,
    ))?;
    let entries = rdb.entries();
    let mutation_layers = collect_active_mutation_layers(
        &entries,
        rdb,
        project_key,
        settings.active_mutation_version,
    )?;
    let settings_hash = hash_serialize(&settings_hash_input(&settings));
    let generator_hash = hash_serialize(&generator);

    Ok(TerrainBuildContext {
        settings,
        generator,
        mutation_layers,
        settings_hash,
        generator_hash,
    })
}

pub fn build_terrain_chunk_with_context(
    rdb: &mut RDBFile,
    project_key: &str,
    context: &TerrainBuildContext,
    request: TerrainChunkBuildRequest,
    mut phase_callback: impl FnMut(TerrainChunkBuildPhase),
    mut should_cancel: impl FnMut() -> bool,
) -> Result<TerrainChunkBuildOutcome, RdbErr> {
    let coord_key = chunk_coord_key(request.chunk_coords[0], request.chunk_coords[1]);
    let state_key = chunk_state_entry(project_key, &coord_key);

    let relevant_layers = relevant_mutation_layers(&context.mutation_layers, request.chunk_coords);
    let mutation_hash = hash_serialize(&TerrainMutationHashInput {
        mutation_layers: &relevant_layers,
    });
    let content_hash = hash_serialize(&TerrainChunkHashInput {
        settings: settings_hash_input(&context.settings),
        generator: &context.generator,
        mutation_layers: &relevant_layers,
        chunk_coords: request.chunk_coords,
        lod: request.lod,
    });

    let existing_state = rdb.fetch::<TerrainChunkState>(&state_key).ok();
    let mut dirty_flags = 0;
    let mut dirty_reasons = Vec::new();
    if let Some(state) = &existing_state {
        if state.dependency_hashes.settings_hash != context.settings_hash {
            dirty_flags |= TERRAIN_DIRTY_SETTINGS;
            dirty_reasons.push(TerrainDirtyReason::SettingsChanged);
        }
        if state.dependency_hashes.generator_hash != context.generator_hash {
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
        return Ok(TerrainChunkBuildOutcome {
            status: TerrainChunkBuildStatus::Skipped,
            artifact: None,
            state: None,
        });
    }

    let geometry = generate_chunk_geometry_phased(
        &context.settings,
        &context.generator,
        &relevant_layers,
        request.chunk_coords,
        request.lod,
        &mut phase_callback,
        &mut should_cancel,
    );
    let Some((vertices, indices, bounds_min, bounds_max)) = geometry else {
        return Ok(TerrainChunkBuildOutcome {
            status: TerrainChunkBuildStatus::Cancelled,
            artifact: None,
            state: None,
        });
    };

    let (material_ids, material_weights) = assign_chunk_materials(
        &context.settings,
        &context.generator,
        &relevant_layers,
        &vertices,
    );

    let artifact = TerrainChunkArtifact {
        project_key: project_key.to_string(),
        chunk_coords: request.chunk_coords,
        lod: request.lod,
        bounds_min,
        bounds_max,
        vertex_layout: context.settings.vertex_layout.clone(),
        vertices,
        indices,
        material_ids,
        material_weights,
        content_hash,
        mesh_entry: "geometry/terrain_chunk".to_string(),
    };

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
        generator_version: context.settings.active_generator_version,
        mutation_version: context.settings.active_mutation_version,
        last_built_hashes,
        dependency_hashes: TerrainChunkDependencyHashes {
            settings_hash: context.settings_hash,
            generator_hash: context.generator_hash,
            mutation_hash,
        },
    };

    Ok(TerrainChunkBuildOutcome {
        status: TerrainChunkBuildStatus::Built,
        artifact: Some(artifact),
        state: Some(state),
    })
}

fn collect_active_mutation_layers(
    entries: &[RDBEntryMeta],
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
            let mut layer = layer;
            let op_events =
                collect_mutation_ops(entries, rdb, project_key, &layer_id, active_version)?;
            if !op_events.is_empty() {
                layer.ops = op_events;
            }
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

fn collect_mutation_ops(
    entries: &[RDBEntryMeta],
    rdb: &mut RDBFile,
    project_key: &str,
    layer_id: &str,
    active_version: u32,
) -> Result<Vec<TerrainMutationOp>, RdbErr> {
    let prefix = format!("terrain/mutation_op/{project_key}/{layer_id}/");
    let mut latest: BTreeMap<String, TerrainMutationOp> = BTreeMap::new();
    for entry in entries {
        if let Some((version, order, event_id)) = parse_mutation_op_entry(&entry.name, &prefix) {
            if version > active_version {
                continue;
            }
            let mut op = rdb.fetch::<TerrainMutationOp>(&entry.name)?;
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

struct ChunkFieldSamples {
    grid_x: u32,
    grid_y: u32,
    step: u32,
    origin_x: f32,
    origin_y: f32,
    heights: Vec<f32>,
}

fn generate_chunk_geometry_phased(
    settings: &TerrainProjectSettings,
    generator: &TerrainGeneratorDefinition,
    mutation_layers: &[TerrainMutationLayer],
    chunk_coords: [i32; 2],
    lod: u8,
    phase_callback: &mut impl FnMut(TerrainChunkBuildPhase),
    should_cancel: &mut impl FnMut() -> bool,
) -> Option<(Vec<Vertex>, Vec<u32>, [f32; 3], [f32; 3])> {
    phase_callback(TerrainChunkBuildPhase::FieldEval);
    let field = evaluate_chunk_field(settings, generator, mutation_layers, chunk_coords, lod);
    if should_cancel() {
        return None;
    }

    phase_callback(TerrainChunkBuildPhase::SurfaceExtraction);
    let (mut vertices, mut indices, bounds_min, bounds_max) =
        extract_chunk_surface(settings, &field);
    if should_cancel() {
        return None;
    }

    phase_callback(TerrainChunkBuildPhase::Optimize);
    vertices.shrink_to_fit();
    indices.shrink_to_fit();
    if should_cancel() {
        return None;
    }

    Some((vertices, indices, bounds_min, bounds_max))
}

fn evaluate_chunk_field(
    settings: &TerrainProjectSettings,
    generator: &TerrainGeneratorDefinition,
    mutation_layers: &[TerrainMutationLayer],
    chunk_coords: [i32; 2],
    lod: u8,
) -> ChunkFieldSamples {
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

    ChunkFieldSamples {
        grid_x,
        grid_y,
        step,
        origin_x,
        origin_y,
        heights,
    }
}

fn extract_chunk_surface(
    settings: &TerrainProjectSettings,
    field: &ChunkFieldSamples,
) -> (Vec<Vertex>, Vec<u32>, [f32; 3], [f32; 3]) {
    let ChunkFieldSamples {
        grid_x,
        grid_y,
        step,
        origin_x,
        origin_y,
        heights,
    } = field;
    let grid_x = *grid_x;
    let grid_y = *grid_y;
    let step = *step;

    let mut vertices = Vec::with_capacity((grid_x * grid_y) as usize);
    let mut min_bounds = [f32::MAX; 3];
    let mut max_bounds = [f32::MIN; 3];
    for y in 0..grid_y {
        for x in 0..grid_x {
            let world_x = origin_x + x as f32 * step as f32 * settings.tile_size;
            let world_y = origin_y + y as f32 * step as f32 * settings.tile_size;
            let height = heights[(y * grid_x + x) as usize];
            let normal = estimate_normal(grid_x, grid_y, x, y, heights, settings, step);
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

    add_chunk_skirts(
        settings,
        field,
        &mut vertices,
        &mut indices,
        &mut min_bounds,
    );

    match settings.vertex_layout {
        TerrainVertexLayout::Standard => (vertices, indices, min_bounds, max_bounds),
    }
}

#[derive(Clone, Copy)]
struct MaterialSample {
    ids: [u32; 4],
    weights: [f32; 4],
}

fn assign_chunk_materials(
    settings: &TerrainProjectSettings,
    generator: &TerrainGeneratorDefinition,
    mutation_layers: &[TerrainMutationLayer],
    vertices: &[Vertex],
) -> (Option<Vec<u32>>, Option<Vec<[f32; 4]>>) {
    if vertices.is_empty() {
        return (None, None);
    }

    let mut material_ids = Vec::with_capacity(vertices.len() * 4);
    let mut material_weights = Vec::with_capacity(vertices.len());

    for vertex in vertices {
        let position = vertex.position;
        let normal = vertex.normal;
        let mut sample =
            evaluate_material_rules(settings, generator, position, normal, mutation_layers);
        normalize_material_sample(&mut sample);
        material_ids.extend_from_slice(&sample.ids);
        material_weights.push(sample.weights);
    }

    (Some(material_ids), Some(material_weights))
}

fn evaluate_material_rules(
    settings: &TerrainProjectSettings,
    generator: &TerrainGeneratorDefinition,
    position: [f32; 3],
    normal: [f32; 3],
    mutation_layers: &[TerrainMutationLayer],
) -> MaterialSample {
    let slope = (1.0 - normal[2].abs().clamp(0.0, 1.0)).clamp(0.0, 1.0);
    let height = position[2];
    let biome = biome_value(settings, generator, position[0], position[1]);

    let mut weighted = Vec::new();
    for rule in &generator.material_rules {
        let weight = rule_weight(rule, height, slope, biome);
        if weight > 0.0 {
            weighted.push((rule.material_id, weight));
        }
    }

    let mut sample = MaterialSample {
        ids: [0; 4],
        weights: [0.0; 4],
    };

    if weighted.is_empty() {
        sample.ids[0] = 0;
        sample.weights[0] = 1.0;
    } else {
        weighted.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
        pack_material_weights(&mut sample, &weighted);
    }

    apply_material_paint(mutation_layers, position, &mut sample);
    sample
}

fn rule_weight(rule: &TerrainMaterialRule, height: f32, slope: f32, biome: f32) -> f32 {
    let height_weight = range_weight(height, rule.height_range, rule.blend);
    let slope_weight = range_weight(slope, rule.slope_range, rule.blend);
    let biome_weight = range_weight(biome, rule.biome_range, rule.blend);
    let weight = height_weight.min(slope_weight).min(biome_weight);
    (weight * rule.weight).clamp(0.0, 1.0)
}

fn range_weight(value: f32, range: [f32; 2], blend: f32) -> f32 {
    let min = range[0].min(range[1]);
    let max = range[0].max(range[1]);
    if value < min || value > max {
        return 0.0;
    }
    let blend = blend.clamp(0.0, 1.0);
    let start = min + (max - min) * blend;
    let end = max - (max - min) * blend;
    if value >= start && value <= end {
        return 1.0;
    }
    if value < start {
        return ((value - min) / (start - min).max(0.0001)).clamp(0.0, 1.0);
    }
    ((max - value) / (max - end).max(0.0001)).clamp(0.0, 1.0)
}

fn pack_material_weights(sample: &mut MaterialSample, weights: &[(u32, f32)]) {
    let mut aggregated: Vec<(u32, f32)> = Vec::new();
    for (id, weight) in weights {
        if let Some(entry) = aggregated.iter_mut().find(|(existing, _)| existing == id) {
            entry.1 += *weight;
        } else {
            aggregated.push((*id, *weight));
        }
    }
    aggregated.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
    for i in 0..4 {
        if let Some((id, weight)) = aggregated.get(i) {
            sample.ids[i] = *id;
            sample.weights[i] = *weight;
        } else {
            sample.ids[i] = 0;
            sample.weights[i] = 0.0;
        }
    }
}

fn apply_material_paint(
    mutation_layers: &[TerrainMutationLayer],
    position: [f32; 3],
    sample: &mut MaterialSample,
) {
    let world_x = position[0];
    let world_y = position[1];
    for layer in mutation_layers {
        for op in &layer.ops {
            if !op.enabled || op.strength == 0.0 {
                continue;
            }
            let (center, material_id, blend_mode) = match op.params {
                TerrainMutationParams::MaterialPaint {
                    center,
                    material_id,
                    blend_mode,
                } => (center, material_id, blend_mode),
                _ => continue,
            };
            let influence = radial_falloff(center, world_x, world_y, op.radius, op.falloff);
            if influence <= 0.0 {
                continue;
            }
            let mut blend = (op.strength * influence).clamp(0.0, 1.0);
            if blend_mode == TerrainMaterialBlendMode::Overwrite {
                blend = 1.0;
            }
            blend_material(sample, material_id, blend);
        }
    }
}

fn blend_material(sample: &mut MaterialSample, material_id: u32, blend: f32) {
    if blend <= 0.0 {
        return;
    }
    for weight in &mut sample.weights {
        *weight *= 1.0 - blend;
    }
    if let Some(idx) = sample.ids.iter().position(|id| *id == material_id) {
        sample.weights[idx] += blend;
    } else {
        let (min_idx, _) = sample
            .weights
            .iter()
            .enumerate()
            .min_by(|a, b| a.1.partial_cmp(b.1).unwrap_or(std::cmp::Ordering::Equal))
            .unwrap_or((0, &0.0));
        sample.ids[min_idx] = material_id;
        sample.weights[min_idx] = blend;
    }
    normalize_material_sample(sample);
}

fn normalize_material_sample(sample: &mut MaterialSample) {
    let total: f32 = sample.weights.iter().sum();
    if total <= 0.0001 {
        sample.weights = [0.0; 4];
        sample.ids = [0; 4];
        sample.ids[0] = 0;
        sample.weights[0] = 1.0;
        return;
    }
    for weight in &mut sample.weights {
        *weight /= total;
    }
    let mut entries: Vec<(u32, f32)> = sample
        .ids
        .iter()
        .copied()
        .zip(sample.weights.iter().copied())
        .collect();
    entries.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
    for (i, (id, weight)) in entries.iter().enumerate() {
        sample.ids[i] = *id;
        sample.weights[i] = *weight;
    }
}

fn add_chunk_skirts(
    settings: &TerrainProjectSettings,
    field: &ChunkFieldSamples,
    vertices: &mut Vec<Vertex>,
    indices: &mut Vec<u32>,
    min_bounds: &mut [f32; 3],
) {
    if field.grid_x < 2 || field.grid_y < 2 {
        return;
    }
    let base_drop = (settings.tile_size * field.step as f32 * 2.0).max(1.0);
    let desired_z = min_bounds[2] - base_drop;
    let skirt_z = desired_z.min(settings.world_bounds_min[2]);
    let skirt_depth = min_bounds[2] - skirt_z;
    if skirt_depth <= 0.0 {
        return;
    }

    let grid_x = field.grid_x;
    let grid_y = field.grid_y;

    let mut edge_indices = Vec::new();
    edge_indices.extend((0..grid_x).map(|x| (x) as u32));
    append_skirt_edge(vertices, indices, &edge_indices, skirt_depth);

    edge_indices.clear();
    edge_indices.extend((0..grid_x).map(|x| ((grid_y - 1) * grid_x + x) as u32));
    append_skirt_edge(vertices, indices, &edge_indices, skirt_depth);

    edge_indices.clear();
    edge_indices.extend((0..grid_y).map(|y| (y * grid_x) as u32));
    append_skirt_edge(vertices, indices, &edge_indices, skirt_depth);

    edge_indices.clear();
    edge_indices.extend((0..grid_y).map(|y| (y * grid_x + (grid_x - 1)) as u32));
    append_skirt_edge(vertices, indices, &edge_indices, skirt_depth);

    min_bounds[2] = min_bounds[2].min(skirt_z);
}

fn append_skirt_edge(
    vertices: &mut Vec<Vertex>,
    indices: &mut Vec<u32>,
    edge: &[u32],
    skirt_depth: f32,
) {
    if edge.len() < 2 {
        return;
    }
    let start_index = vertices.len() as u32;
    for &idx in edge {
        if let Some(src) = vertices.get(idx as usize).cloned() {
            let mut v = src;
            v.position[2] -= skirt_depth;
            v.normal = [0.0, 0.0, -1.0];
            vertices.push(v);
        }
    }
    for i in 0..edge.len() - 1 {
        let top0 = edge[i];
        let top1 = edge[i + 1];
        let skirt0 = start_index + i as u32;
        let skirt1 = start_index + i as u32 + 1;
        indices.extend_from_slice(&[top0, top1, skirt1, top0, skirt1, skirt0]);
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
            if !op.enabled || op.strength == 0.0 {
                continue;
            }
            let influence = match op.params {
                TerrainMutationParams::Sphere { center }
                | TerrainMutationParams::Smooth { center } => {
                    radial_falloff(center, world_x, world_y, op.radius, op.falloff)
                }
                TerrainMutationParams::Capsule { start, end } => {
                    capsule_falloff(start, end, [world_x, world_y], op.radius, op.falloff)
                }
                TerrainMutationParams::MaterialPaint {
                    center,
                    material_id: _,
                    blend_mode: _,
                } => {
                    radial_falloff(center, world_x, world_y, op.radius, op.falloff)
                }
            };
            if influence <= 0.0 {
                continue;
            }
            match op.kind {
                TerrainMutationOpKind::SphereAdd | TerrainMutationOpKind::CapsuleAdd => {
                    height += op.strength * influence;
                }
                TerrainMutationOpKind::SphereSub | TerrainMutationOpKind::CapsuleSub => {
                    height -= op.strength * influence;
                }
                TerrainMutationOpKind::Smooth => {
                    let blend = (op.strength * influence).clamp(0.0, 1.0);
                    height = height + (base - height) * blend;
                }
                TerrainMutationOpKind::MaterialPaint => {
                    // Height unaffected.
                }
            }
        }
    }
    height
}

fn biome_value(
    settings: &TerrainProjectSettings,
    generator: &TerrainGeneratorDefinition,
    world_x: f32,
    world_y: f32,
) -> f32 {
    let freq = generator.biome_frequency.max(0.0001);
    let xi = (world_x * freq).floor() as i64;
    let yi = (world_y * freq).floor() as i64;
    let mut bytes = Vec::with_capacity(24);
    bytes.extend_from_slice(&settings.seed.to_le_bytes());
    bytes.extend_from_slice(&xi.to_le_bytes());
    bytes.extend_from_slice(&yi.to_le_bytes());
    let hash = fnv1a64(&bytes);
    (hash as f64 / u64::MAX as f64) as f32
}

fn radial_falloff(center: [f32; 3], world_x: f32, world_y: f32, radius: f32, falloff: f32) -> f32 {
    let dx = world_x - center[0];
    let dy = world_y - center[1];
    falloff_weight((dx * dx + dy * dy).sqrt(), radius, falloff)
}

fn capsule_falloff(
    start: [f32; 3],
    end: [f32; 3],
    point: [f32; 2],
    radius: f32,
    falloff: f32,
) -> f32 {
    let vx = end[0] - start[0];
    let vy = end[1] - start[1];
    let wx = point[0] - start[0];
    let wy = point[1] - start[1];
    let len_sq = vx * vx + vy * vy;
    let t = if len_sq > 0.0 {
        (wx * vx + wy * vy) / len_sq
    } else {
        0.0
    };
    let t = t.clamp(0.0, 1.0);
    let closest = [start[0] + vx * t, start[1] + vy * t];
    let dx = point[0] - closest[0];
    let dy = point[1] - closest[1];
    falloff_weight((dx * dx + dy * dy).sqrt(), radius, falloff)
}

fn falloff_weight(distance: f32, radius: f32, falloff: f32) -> f32 {
    if radius <= 0.0 {
        return 0.0;
    }
    if distance >= radius {
        return 0.0;
    }
    let t = (distance / radius).clamp(0.0, 1.0);
    let hardness = (1.0 - falloff).clamp(0.0, 1.0);
    if t <= hardness {
        1.0
    } else {
        let ft = (t - hardness) / (1.0 - hardness).max(0.0001);
        1.0 - ft
    }
}

pub fn sample_height_with_mutations(
    settings: &TerrainProjectSettings,
    generator: &TerrainGeneratorDefinition,
    mutation_layers: &[TerrainMutationLayer],
    world_x: f32,
    world_y: f32,
) -> f32 {
    sample_height(settings, generator, mutation_layers, world_x, world_y)
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
    use crate::rdb::terrain::{
        TerrainMutationLayer, TerrainMutationOp, TerrainMutationOpKind, TerrainMutationParams,
        TerrainProjectSettings, mutation_op_entry,
    };

    fn seed_rdb(project_key: &str) -> RDBFile {
        let mut rdb = RDBFile::new();
        let settings = TerrainProjectSettings::default();
        let generator = TerrainGeneratorDefinition::default();
        let layer = TerrainMutationLayer::new("layer-a", "Layer A", 0);
        let op = TerrainMutationOp {
            op_id: "raise".to_string(),
            layer_id: layer.layer_id.clone(),
            enabled: true,
            order: 0,
            kind: TerrainMutationOpKind::SphereAdd,
            params: TerrainMutationParams::Sphere {
                center: [8.0, 8.0, 0.0],
            },
            radius: 4.0,
            strength: 2.0,
            falloff: 0.5,
            event_id: 1,
            timestamp: 1,
            author: None,
        };
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
        rdb.add(
            &mutation_op_entry(
                project_key,
                &layer.layer_id,
                settings.active_mutation_version,
                op.order,
                op.event_id,
            ),
            &op,
        )
        .expect("mutation op");
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
        let settings = TerrainProjectSettings::default();
        let generator = TerrainGeneratorDefinition::default();

        let layer = TerrainMutationLayer::new("layer-a", "Layer A", 0);
        let op = TerrainMutationOp {
            op_id: "raise".to_string(),
            layer_id: layer.layer_id.clone(),
            enabled: true,
            order: 0,
            kind: TerrainMutationOpKind::SphereAdd,
            params: TerrainMutationParams::Sphere {
                center: [8.0, 8.0, 0.0],
            },
            radius: 4.0,
            strength: 2.0,
            falloff: 0.5,
            event_id: 1,
            timestamp: 1,
            author: None,
        };
        let mut layer = layer;
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
        rdb.add(
            &mutation_op_entry(
                project_key,
                &layer.layer_id,
                settings.active_mutation_version,
                op.order,
                op.event_id,
            ),
            &op,
        )
        .expect("mutation op");

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

        let updated_op = TerrainMutationOp {
            strength: 4.0,
            event_id: 2,
            timestamp: 2,
            ..op.clone()
        };
        rdb.add(
            &mutation_op_entry(
                project_key,
                &layer.layer_id,
                settings.active_mutation_version,
                updated_op.order,
                updated_op.event_id,
            ),
            &updated_op,
        )
        .expect("mutation op");

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

    #[test]
    fn mutation_replay_is_deterministic_by_order() {
        let project_key = "sample";
        let settings = TerrainProjectSettings::default();
        let generator = TerrainGeneratorDefinition::default();
        let layer = TerrainMutationLayer::new("layer-a", "Layer A", 0);

        let op_a = TerrainMutationOp {
            op_id: "a".to_string(),
            layer_id: layer.layer_id.clone(),
            enabled: true,
            order: 1,
            kind: TerrainMutationOpKind::SphereAdd,
            params: TerrainMutationParams::Sphere {
                center: [4.0, 4.0, 0.0],
            },
            radius: 3.0,
            strength: 1.0,
            falloff: 0.4,
            event_id: 1,
            timestamp: 1,
            author: None,
        };
        let op_b = TerrainMutationOp {
            op_id: "b".to_string(),
            order: 0,
            ..op_a.clone()
        };

        let mut rdb = RDBFile::new();
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
        rdb.add(
            &mutation_op_entry(
                project_key,
                &layer.layer_id,
                settings.active_mutation_version,
                op_a.order,
                op_a.event_id,
            ),
            &op_a,
        )
        .expect("mutation op");
        rdb.add(
            &mutation_op_entry(
                project_key,
                &layer.layer_id,
                settings.active_mutation_version,
                op_b.order,
                op_b.event_id,
            ),
            &op_b,
        )
        .expect("mutation op");

        let request = TerrainChunkBuildRequest {
            chunk_coords: [0, 0],
            lod: 0,
        };
        let report = build_terrain_chunks(&mut rdb, project_key, &[request]).expect("build");
        assert_eq!(report.built_chunks, 1);
        let coord_key = chunk_coord_key(0, 0);
        let artifact_key = chunk_artifact_entry(project_key, &coord_key, "lod0");
        let artifact_a = rdb
            .fetch::<TerrainChunkArtifact>(&artifact_key)
            .expect("artifact");

        let mut rdb_alt = RDBFile::new();
        rdb_alt
            .add(&project_settings_entry(project_key), &settings)
            .expect("settings");
        rdb_alt
            .add(
                &generator_entry(project_key, settings.active_generator_version),
                &generator,
            )
            .expect("generator");
        rdb_alt
            .add(
                &mutation_layer_entry(
                    project_key,
                    &layer.layer_id,
                    settings.active_mutation_version,
                ),
                &layer,
            )
            .expect("mutation");
        rdb_alt
            .add(
                &mutation_op_entry(
                    project_key,
                    &layer.layer_id,
                    settings.active_mutation_version,
                    op_b.order,
                    op_b.event_id,
                ),
                &op_b,
            )
            .expect("mutation op");
        rdb_alt
            .add(
                &mutation_op_entry(
                    project_key,
                    &layer.layer_id,
                    settings.active_mutation_version,
                    op_a.order,
                    op_a.event_id,
                ),
                &op_a,
            )
            .expect("mutation op");

        let report_alt =
            build_terrain_chunks(&mut rdb_alt, project_key, &[request]).expect("build");
        assert_eq!(report_alt.built_chunks, 1);
        let artifact_b = rdb_alt
            .fetch::<TerrainChunkArtifact>(&artifact_key)
            .expect("artifact");
        assert_eq!(artifact_a.content_hash, artifact_b.content_hash);
    }

    #[test]
    fn deterministic_hash_matches_golden() {
        let mut rdb = seed_rdb("sample");
        let request = TerrainChunkBuildRequest {
            chunk_coords: [0, 0],
            lod: 0,
        };
        let report = build_terrain_chunks(&mut rdb, "sample", &[request]).expect("build");
        assert_eq!(report.built_chunks, 1);
        let coord_key = chunk_coord_key(0, 0);
        let artifact_key = chunk_artifact_entry("sample", &coord_key, "lod0");
        let artifact = rdb
            .fetch::<TerrainChunkArtifact>(&artifact_key)
            .expect("artifact");
        assert_eq!(artifact.content_hash, 0xA19A48D25039A6F8);
    }

    #[test]
    fn material_paint_rebuilds_only_affected_chunk() {
        let project_key = "sample";
        let mut rdb = RDBFile::new();
        let settings = TerrainProjectSettings::default();
        let generator = TerrainGeneratorDefinition::default();

        let mut layer = TerrainMutationLayer::new("layer-a", "Layer A", 0);
        layer.affected_chunks = Some(vec![[0, 0]]);
        let op = TerrainMutationOp {
            op_id: "paint".to_string(),
            layer_id: layer.layer_id.clone(),
            enabled: true,
            order: 0,
            kind: TerrainMutationOpKind::MaterialPaint,
            params: TerrainMutationParams::MaterialPaint {
                center: [8.0, 8.0, 0.0],
                material_id: 2,
                blend_mode: crate::rdb::terrain::TerrainMaterialBlendMode::Blend,
            },
            radius: 6.0,
            strength: 0.8,
            falloff: 0.4,
            event_id: 1,
            timestamp: 1,
            author: None,
        };

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
        rdb.add(
            &mutation_op_entry(
                project_key,
                &layer.layer_id,
                settings.active_mutation_version,
                op.order,
                op.event_id,
            ),
            &op,
        )
        .expect("mutation op");

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

        let updated_op = TerrainMutationOp {
            params: TerrainMutationParams::MaterialPaint {
                center: [8.0, 8.0, 0.0],
                material_id: 3,
                blend_mode: crate::rdb::terrain::TerrainMaterialBlendMode::Blend,
            },
            event_id: 2,
            timestamp: 2,
            ..op
        };
        rdb.add(
            &mutation_op_entry(
                project_key,
                &layer.layer_id,
                settings.active_mutation_version,
                updated_op.order,
                updated_op.event_id,
            ),
            &updated_op,
        )
        .expect("mutation op");

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

    #[test]
    fn cancelled_build_does_not_commit_artifacts() {
        let mut rdb = seed_rdb("sample");
        let context = prepare_terrain_build_context(&mut rdb, "sample").expect("context");
        let request = TerrainChunkBuildRequest {
            chunk_coords: [0, 0],
            lod: 0,
        };
        let cancel = std::cell::Cell::new(false);
        let outcome = build_terrain_chunk_with_context(
            &mut rdb,
            "sample",
            &context,
            request,
            |phase| {
                if phase == TerrainChunkBuildPhase::FieldEval {
                    cancel.set(true);
                }
            },
            || cancel.replace(false),
        )
        .expect("build");
        assert_eq!(outcome.status, TerrainChunkBuildStatus::Cancelled);
        assert!(outcome.artifact.is_none());
        assert!(outcome.state.is_none());

        let coord_key = chunk_coord_key(0, 0);
        let artifact_key = chunk_artifact_entry("sample", &coord_key, "lod0");
        assert!(rdb.fetch::<TerrainChunkArtifact>(&artifact_key).is_err());
    }
}

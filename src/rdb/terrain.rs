use serde::{Deserialize, Serialize};
use tracing::info;

use super::{DatabaseEntry, primitives::Vertex};
use crate::{RDBView, error::NorenError};

/// RDB schema for terrain data.
///
/// ## Entry naming conventions
/// - Project settings: `terrain/project/{project_key}/settings`
/// - Generator definition (versioned): `terrain/generator/{project_key}/v{version}`
/// - Mutation layer (versioned): `terrain/mutation_layer/{project_key}/{layer_id}/v{version}`
/// - Chunk artifact: `terrain/chunk_artifact/{project_key}/{chunk_coord}/{lod_key}`
/// - Chunk state: `terrain/chunk_state/{project_key}/{chunk_coord}`
///
/// ## Migration strategy
/// Existing terrain entries under `terrain/project/{project_key}/settings`,
/// `terrain/project/{project_key}/generator`, and
/// `terrain/project/{project_key}/mutation_layers` should be copied into the
/// new versioned entries above. Consumers should then set
/// `active_generator_version` and `active_mutation_version` in the project
/// settings and write the new entries alongside existing data. Once clients are
/// updated to use the new entries, legacy keys can be removed.
pub const TERRAIN_PROJECT_PREFIX: &str = "terrain/project";
pub const TERRAIN_GENERATOR_PREFIX: &str = "terrain/generator";
pub const TERRAIN_MUTATION_LAYER_PREFIX: &str = "terrain/mutation_layer";
pub const TERRAIN_CHUNK_ARTIFACT_PREFIX: &str = "terrain/chunk_artifact";
pub const TERRAIN_CHUNK_STATE_PREFIX: &str = "terrain/chunk_state";

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub enum TerrainVertexLayout {
    Standard,
}

impl Default for TerrainVertexLayout {
    fn default() -> Self {
        Self::Standard
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct TerrainLodPolicy {
    /// Maximum LOD index (0 is the highest detail).
    pub max_lod: u8,
    /// World-space distances at which to transition to the next LOD.
    pub distance_bands: Vec<f32>,
}

impl Default for TerrainLodPolicy {
    fn default() -> Self {
        Self {
            max_lod: 0,
            distance_bands: vec![256.0, 512.0, 1024.0],
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct TerrainProjectSettings {
    pub name: String,
    pub seed: u64,
    /// Tile size in world units.
    pub tile_size: f32,
    /// Tile grid dimensions per chunk.
    pub tiles_per_chunk: [u32; 2],
    /// World bounds minimum (x, y, z).
    pub world_bounds_min: [f32; 3],
    /// World bounds maximum (x, y, z).
    pub world_bounds_max: [f32; 3],
    pub lod_policy: TerrainLodPolicy,
    pub generator_graph_id: String,
    #[serde(default)]
    pub vertex_layout: TerrainVertexLayout,
    /// The active generator version stored under `terrain/generator`.
    pub active_generator_version: u32,
    /// The active mutation layer version stored under `terrain/mutation_layer`.
    pub active_mutation_version: u32,
}

impl Default for TerrainProjectSettings {
    fn default() -> Self {
        Self {
            name: "New Terrain Project".to_string(),
            seed: 1337,
            tile_size: 1.0,
            tiles_per_chunk: [32, 32],
            world_bounds_min: [0.0, 0.0, 0.0],
            world_bounds_max: [1024.0, 1024.0, 256.0],
            lod_policy: TerrainLodPolicy::default(),
            generator_graph_id: "default".to_string(),
            vertex_layout: TerrainVertexLayout::Standard,
            active_generator_version: 1,
            active_mutation_version: 1,
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct TerrainGeneratorDefinition {
    pub version: u32,
    pub graph_id: String,
    pub algorithm: String,
    pub frequency: f32,
    pub amplitude: f32,
}

impl Default for TerrainGeneratorDefinition {
    fn default() -> Self {
        Self {
            version: 1,
            graph_id: "default".to_string(),
            algorithm: "ridge-noise".to_string(),
            frequency: 0.02,
            amplitude: 64.0,
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct TerrainMutationOp {
    pub op_id: String,
    pub enabled: bool,
    pub weight: f32,
}

impl TerrainMutationOp {
    pub fn new(op_id: impl Into<String>) -> Self {
        Self {
            op_id: op_id.into(),
            enabled: true,
            weight: 1.0,
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct TerrainMutationLayer {
    pub layer_id: String,
    pub name: String,
    pub order: u32,
    pub version: u32,
    /// Ordered mutation operations for the layer.
    pub ops: Vec<TerrainMutationOp>,
    /// Optional list of chunk coordinates this layer affects. When omitted, the
    /// layer applies to every chunk.
    #[serde(default)]
    pub affected_chunks: Option<Vec<[i32; 2]>>,
}

impl TerrainMutationLayer {
    pub fn new(layer_id: impl Into<String>, name: impl Into<String>, order: u32) -> Self {
        Self {
            layer_id: layer_id.into(),
            name: name.into(),
            order,
            version: 1,
            ops: Vec::new(),
            affected_chunks: None,
        }
    }

    pub fn with_op(mut self, op: TerrainMutationOp) -> Self {
        self.ops.push(op);
        self
    }
}

impl Default for TerrainMutationLayer {
    fn default() -> Self {
        Self::new("layer-1", "Layer 1", 0).with_op(TerrainMutationOp::new("default-op"))
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct TerrainChunkArtifact {
    pub project_key: String,
    pub chunk_coords: [i32; 2],
    pub lod: u8,
    pub bounds_min: [f32; 3],
    pub bounds_max: [f32; 3],
    #[serde(default)]
    pub vertex_layout: TerrainVertexLayout,
    #[serde(default)]
    pub vertices: Vec<Vertex>,
    #[serde(default)]
    pub indices: Vec<u32>,
    #[serde(default)]
    pub material_ids: Option<Vec<u32>>,
    #[serde(default)]
    pub material_weights: Option<Vec<[f32; 4]>>,
    pub content_hash: u64,
    #[serde(default)]
    pub mesh_entry: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct TerrainChunkLodHash {
    pub lod: u8,
    pub hash: u64,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct TerrainChunkDependencyHashes {
    #[serde(default)]
    pub settings_hash: u64,
    pub generator_hash: u64,
    pub mutation_hash: u64,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub enum TerrainDirtyReason {
    SettingsChanged,
    GeneratorChanged,
    MutationChanged,
}

pub const TERRAIN_DIRTY_SETTINGS: u32 = 1 << 0;
pub const TERRAIN_DIRTY_GENERATOR: u32 = 1 << 1;
pub const TERRAIN_DIRTY_MUTATION: u32 = 1 << 2;

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct TerrainChunkState {
    pub project_key: String,
    pub chunk_coords: [i32; 2],
    pub dirty_flags: u32,
    #[serde(default)]
    pub dirty_reasons: Vec<TerrainDirtyReason>,
    pub generator_version: u32,
    pub mutation_version: u32,
    pub last_built_hashes: Vec<TerrainChunkLodHash>,
    pub dependency_hashes: TerrainChunkDependencyHashes,
}

pub fn project_settings_entry(project_key: &str) -> String {
    format!("{TERRAIN_PROJECT_PREFIX}/{project_key}/settings")
}

pub fn generator_entry(project_key: &str, version: u32) -> String {
    format!("{TERRAIN_GENERATOR_PREFIX}/{project_key}/v{version}")
}

pub fn mutation_layer_entry(project_key: &str, layer_id: &str, version: u32) -> String {
    format!("{TERRAIN_MUTATION_LAYER_PREFIX}/{project_key}/{layer_id}/v{version}")
}

pub fn chunk_coord_key(x: i32, y: i32) -> String {
    format!("{x}_{y}")
}

pub fn lod_key(lod: u8) -> String {
    format!("lod{lod}")
}

pub fn chunk_artifact_entry(project_key: &str, coord_key: &str, lod_key: &str) -> String {
    format!("{TERRAIN_CHUNK_ARTIFACT_PREFIX}/{project_key}/{coord_key}/{lod_key}")
}

pub fn chunk_state_entry(project_key: &str, coord_key: &str) -> String {
    format!("{TERRAIN_CHUNK_STATE_PREFIX}/{project_key}/{coord_key}")
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, Default, PartialEq)]
pub struct TerrainTile {
    pub tile_id: u32,
    pub flags: u32,
}

#[derive(Clone, Debug, Serialize, Deserialize, Default)]
pub struct TerrainChunk {
    /// Grid coordinates of the chunk in chunk space.
    pub chunk_coords: [i32; 2],
    /// World-space origin (x, y) for the chunk.
    pub origin: [f32; 2],
    /// Size of each tile in world units.
    pub tile_size: f32,
    /// Tile dimensions (width, height) for this chunk.
    pub tiles_per_chunk: [u32; 2],
    /// Tile metadata for each tile in the chunk, row-major.
    pub tiles: Vec<TerrainTile>,
    /// Height samples stored in a (width + 1) x (height + 1) grid.
    pub heights: Vec<f32>,
    /// Geometry entry name for the chunk mesh.
    pub mesh_entry: String,
}

impl TerrainChunk {
    pub fn tile_index(&self, tile_x: i32, tile_y: i32) -> Option<usize> {
        if tile_x < 0 || tile_y < 0 {
            return None;
        }

        let tile_x = tile_x as u32;
        let tile_y = tile_y as u32;
        if tile_x >= self.tiles_per_chunk[0] || tile_y >= self.tiles_per_chunk[1] {
            return None;
        }

        let idx = tile_y
            .checked_mul(self.tiles_per_chunk[0])?
            .checked_add(tile_x)?;
        if idx as usize >= self.tiles.len() {
            return None;
        }

        Some(idx as usize)
    }

    pub fn tile_at(&self, tile_x: i32, tile_y: i32) -> Option<&TerrainTile> {
        let idx = self.tile_index(tile_x, tile_y)?;
        self.tiles.get(idx)
    }

    pub fn tile_at_world(&self, world_x: f32, world_y: f32) -> Option<&TerrainTile> {
        let (tile_x, tile_y) = self.tile_coords_from_world(world_x, world_y)?;
        self.tile_at(tile_x as i32, tile_y as i32)
    }

    pub fn tile_coords_from_world(&self, world_x: f32, world_y: f32) -> Option<(u32, u32)> {
        if self.tile_size <= 0.0 {
            return None;
        }

        let local_x = (world_x - self.origin[0]) / self.tile_size;
        let local_y = (world_y - self.origin[1]) / self.tile_size;
        if local_x < 0.0 || local_y < 0.0 {
            return None;
        }

        let tile_x = local_x.floor() as i32;
        let tile_y = local_y.floor() as i32;
        if tile_x < 0 || tile_y < 0 {
            return None;
        }

        let tile_x = tile_x as u32;
        let tile_y = tile_y as u32;
        if tile_x >= self.tiles_per_chunk[0] || tile_y >= self.tiles_per_chunk[1] {
            return None;
        }

        Some((tile_x, tile_y))
    }

    pub fn height_at_world(&self, world_x: f32, world_y: f32) -> Option<f32> {
        if self.tile_size <= 0.0 {
            return None;
        }

        let local_x = world_x - self.origin[0];
        let local_y = world_y - self.origin[1];
        self.height_at_local(local_x, local_y)
    }

    pub fn height_at_local(&self, local_x: f32, local_y: f32) -> Option<f32> {
        if self.tile_size <= 0.0 {
            return None;
        }

        let grid_x = local_x / self.tile_size;
        let grid_y = local_y / self.tile_size;

        if grid_x < 0.0 || grid_y < 0.0 {
            return None;
        }

        let grid_width = self.tiles_per_chunk[0].checked_add(1)?;
        let grid_height = self.tiles_per_chunk[1].checked_add(1)?;
        if grid_width == 0 || grid_height == 0 {
            return None;
        }

        let max_x = (grid_width - 1) as f32;
        let max_y = (grid_height - 1) as f32;
        if grid_x > max_x || grid_y > max_y {
            return None;
        }

        let x0 = grid_x.floor() as u32;
        let y0 = grid_y.floor() as u32;
        let x1 = (x0 + 1).min(grid_width - 1);
        let y1 = (y0 + 1).min(grid_height - 1);

        let h00 = self.height_sample(x0, y0)?;
        let h10 = self.height_sample(x1, y0)?;
        let h01 = self.height_sample(x0, y1)?;
        let h11 = self.height_sample(x1, y1)?;

        let tx = grid_x - x0 as f32;
        let ty = grid_y - y0 as f32;

        let hx0 = h00 + (h10 - h00) * tx;
        let hx1 = h01 + (h11 - h01) * tx;

        Some(hx0 + (hx1 - hx0) * ty)
    }

    pub fn height_sample(&self, sample_x: u32, sample_y: u32) -> Option<f32> {
        let grid_width = self.tiles_per_chunk[0].checked_add(1)?;
        let grid_height = self.tiles_per_chunk[1].checked_add(1)?;
        if sample_x >= grid_width || sample_y >= grid_height {
            return None;
        }

        let idx = sample_y.checked_mul(grid_width)?.checked_add(sample_x)? as usize;
        self.heights.get(idx).copied()
    }
}

pub struct TerrainDB {
    data: Option<RDBView>,
}

impl TerrainDB {
    pub fn new(module_path: &str) -> Self {
        let data = match RDBView::load(module_path) {
            Ok(d) => Some(d),
            Err(_) => None,
        };

        Self { data }
    }

    pub fn fetch_chunk(&mut self, entry: DatabaseEntry<'_>) -> Result<TerrainChunk, NorenError> {
        if let Some(rdb) = &mut self.data {
            if let Ok(chunk) = rdb.fetch::<TerrainChunk>(entry) {
                info!(resource = "terrain", entry = %entry, source = "rdb");
                return Ok(chunk);
            }
        }

        Err(NorenError::DataFailure())
    }

    pub fn enumerate_entries(&self) -> Vec<String> {
        self.data
            .as_ref()
            .map(|rdb| rdb.entries().into_iter().map(|meta| meta.name).collect())
            .unwrap_or_default()
    }

    pub fn has_data(&self) -> bool {
        self.data.is_some()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::utils::rdbfile::RDBFile;
    use tempfile::tempdir;

    fn sample_chunk() -> TerrainChunk {
        TerrainChunk {
            chunk_coords: [0, 0],
            origin: [0.0, 0.0],
            tile_size: 1.0,
            tiles_per_chunk: [2, 2],
            tiles: vec![
                TerrainTile {
                    tile_id: 1,
                    flags: 0,
                },
                TerrainTile {
                    tile_id: 2,
                    flags: 0,
                },
                TerrainTile {
                    tile_id: 3,
                    flags: 0,
                },
                TerrainTile {
                    tile_id: 4,
                    flags: 0,
                },
            ],
            heights: vec![0.0, 1.0, 2.0, 1.0, 2.0, 3.0, 2.0, 3.0, 4.0],
            mesh_entry: "geometry/terrain_chunk".to_string(),
        }
    }

    #[test]
    fn tile_lookup_works() {
        let chunk = sample_chunk();
        assert_eq!(chunk.tile_at(0, 0).unwrap().tile_id, 1);
        assert_eq!(chunk.tile_at(1, 1).unwrap().tile_id, 4);
        assert!(chunk.tile_at(2, 2).is_none());
        assert!(chunk.tile_at_world(1.2, 0.2).is_some());
    }

    #[test]
    fn height_interpolation() {
        let chunk = sample_chunk();
        let h = chunk.height_at_world(0.5, 0.5).unwrap();
        assert!(h > 0.0 && h < 2.0);
        assert_eq!(chunk.height_at_world(-1.0, 0.0), None);
    }

    #[test]
    fn terrain_db_reads_chunks() -> Result<(), NorenError> {
        let temp = tempdir().expect("temp dir");
        let path = temp.path().join("terrain.rdb");
        let mut file = RDBFile::new();
        file.add("terrain/chunk_0_0", &sample_chunk())?;
        file.save(&path)?;

        let mut db = TerrainDB::new(path.to_str().unwrap());
        let chunk = db.fetch_chunk("terrain/chunk_0_0")?;
        assert_eq!(chunk.mesh_entry, "geometry/terrain_chunk");
        Ok(())
    }

    #[test]
    fn mutation_layers_round_trip_with_versions() -> Result<(), NorenError> {
        let mut rdb = RDBFile::new();
        let project_key = "sample_project";

        let mut settings = TerrainProjectSettings::default();
        settings.name = "Sample Project".to_string();

        let generator = TerrainGeneratorDefinition::default();
        settings.active_generator_version = generator.version;

        let layer_id = "base-layer";
        let layer_v1 =
            TerrainMutationLayer::new(layer_id, "Base", 0).with_op(TerrainMutationOp::new("raise"));
        let mut layer_v2 = layer_v1.clone();
        layer_v2.version = 2;
        layer_v2.ops.push(TerrainMutationOp::new("erode"));
        settings.active_mutation_version = layer_v2.version;

        rdb.add(&project_settings_entry(project_key), &settings)?;
        rdb.add(&generator_entry(project_key, generator.version), &generator)?;
        rdb.add(
            &mutation_layer_entry(project_key, layer_id, layer_v1.version),
            &layer_v1,
        )?;
        rdb.add(
            &mutation_layer_entry(project_key, layer_id, layer_v2.version),
            &layer_v2,
        )?;

        let settings_back =
            rdb.fetch::<TerrainProjectSettings>(&project_settings_entry(project_key))?;
        assert_eq!(settings_back.active_mutation_version, 2);
        let layer_back =
            rdb.fetch::<TerrainMutationLayer>(&mutation_layer_entry(project_key, layer_id, 2))?;
        assert_eq!(layer_back.version, 2);
        assert_eq!(layer_back.ops.len(), 2);
        assert_eq!(layer_back.ops[0].op_id, "raise");
        assert_eq!(layer_back.ops[1].op_id, "erode");
        Ok(())
    }
}

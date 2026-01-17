use serde::{Deserialize, Serialize};
use tracing::info;

use super::DatabaseEntry;
use crate::{RDBView, error::NorenError};

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
}

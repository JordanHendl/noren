# Terrain RDB Layout and Format

This document describes the RDB binary layout and the terrain-specific entry
schema so downstream tooling can read and render terrain assets consistently.

## RDB file format

The terrain database uses the generic RDB container format implemented in
`src/utils/rdbfile.rs` (version 1). The file is a single binary with:

1. **Header** (fixed size, little-endian fields)
   - `magic`: 4 bytes, ASCII `RDB0`
   - `version`: `u16` (currently `1`)
   - `reserved`: `u16` (unused)
   - `entry_count`: `u32` number of entries in the file
2. **Entry table** (`entry_count` entries, fixed-size records)
   - `type_tag`: `u32` FNV-1a hash of the Rust type name
   - `offset`: `u64` byte offset into the data section
   - `len`: `u64` length in bytes of the serialized payload
   - `name`: `[u8; 64]` null-terminated UTF-8 entry key (63 bytes max)
3. **Data section** (concatenated bincode payloads for each entry)

### Serialization

Entry payloads are serialized with `bincode` using Serde `Serialize`/`Deserialize`.
Consumers should deserialize entries into the corresponding Rust-equivalent
schemas (or reimplement the schemas in another language) using the same field
order and types.

### Type tags

The `type_tag` is an FNV-1a 64-bit hash of the fully qualified Rust type name,
truncated to `u32`. This tag is used to validate that the entry payload is read
as the expected type.

## Terrain entry layout

All terrain entries share the prefix `terrain/` with the following keys:

| Entry type | Entry name format |
| --- | --- |
| Project settings | `terrain/project/{project_key}/settings` |
| Generator definition (versioned) | `terrain/generator/{project_key}/v{version}` |
| Mutation layer (versioned) | `terrain/mutation_layer/{project_key}/{layer_id}/v{version}` |
| Mutation op (append-only) | `terrain/mutation_op/{project_key}/{layer_id}/v{version}/o{order}/e{event}` |
| Chunk artifact | `terrain/chunk_artifact/{project_key}/{chunk_coord}/{lod_key}` |
| Chunk state | `terrain/chunk_state/{project_key}/{chunk_coord}` |
| Raw chunk samples | `terrain/chunk_{x}_{y}` |

### Project settings (`TerrainProjectSettings`)

Defines global settings for a terrain project.

- `name`: user-facing project name
- `seed`: generator seed
- `tile_size`: world-space units per tile
- `tiles_per_chunk`: `[u32; 2]` tiles in x/y per chunk
- `world_bounds_min` / `world_bounds_max`: `[f32; 3]` world-space bounds
- `lod_policy`: `TerrainLodPolicy` (max LOD and distance bands)
- `generator_graph_id`: generator graph identifier
- `vertex_layout`: currently `Standard`
- `active_generator_version`: active generator version to read from
  `terrain/generator/{project_key}/v{version}`
- `active_mutation_version`: active mutation layer version to read from
  `terrain/mutation_layer/{project_key}/{layer_id}/v{version}`

### Generator definition (`TerrainGeneratorDefinition`)

Defines the base noise or procedural generator used before mutations.

- `version`: generator version number
- `graph_id`: generator graph identifier
- `algorithm`: generator algorithm identifier (e.g. `ridge-noise`, `fbm`)
- `frequency`, `amplitude`: scalar tuning parameters
- `biome_frequency`: frequency for biome selection
- `material_rules`: list of `TerrainMaterialRule`
  - Each rule defines a material id, height range, slope range, biome range,
    blend factor, and weight.

### Mutation layer (`TerrainMutationLayer`)

Layered edits applied on top of the generator output.

- `layer_id`, `name`, `order`, `version`
- `ops`: ordered list of `TerrainMutationOp` (brush operations)
- `affected_chunks`: optional list of chunk coordinates affected by the layer;
  if omitted, the layer applies to all chunks.

### Mutation op (`TerrainMutationOp`)

Brush operations are stored both inside layers and as append-only entries to
support change tracking. Fields include:

- `op_id`, `layer_id`, `enabled`, `order`
- `kind`: `SphereAdd`, `SphereSub`, `CapsuleAdd`, `CapsuleSub`, `Smooth`,
  `MaterialPaint`
- `params`: shape parameters (`Sphere`, `Capsule`, `Smooth`, or `MaterialPaint`)
  with optional `blend_mode` for material paints
- `radius`, `strength`, `falloff`
- `event_id`: append-only event identifier
- `timestamp`: event timestamp
- `author`: optional author string

### Chunk artifact (`TerrainChunkArtifact`)

Mesh data generated for a specific project chunk and LOD.

- `project_key`, `chunk_coords` (`[i32; 2]`), `lod`
- `bounds_min`, `bounds_max`: world-space bounds for the mesh
- `vertex_layout`: currently `Standard`
- `vertices`: list of `Vertex` records (position, normal, tangent, UV, color,
  joint indices, joint weights)
- `indices`: triangle index buffer (`u32`)
- `material_ids`: optional material id per vertex
- `material_weights`: optional material weight per vertex (`[f32; 4]`)
- `content_hash`: content checksum for dependency tracking
- `mesh_entry`: optional geometry entry name for downstream asset lookup

### Chunk state (`TerrainChunkState`)

Tracks build metadata and dependencies for a chunk.

- `project_key`, `chunk_coords`
- `dirty_flags`: bitset of `TERRAIN_DIRTY_*` values
- `dirty_reasons`: list of `TerrainDirtyReason` values
- `generator_version`, `mutation_version`
- `last_built_hashes`: list of `TerrainChunkLodHash` per LOD
- `dependency_hashes`: `TerrainChunkDependencyHashes` with settings, generator,
  and mutation hashes

### Raw chunk samples (`TerrainChunk`)

Legacy/raw chunk data stored as `terrain/chunk_{x}_{y}`.

- `chunk_coords`: chunk-space `[i32; 2]`
- `origin`: world-space origin `[f32; 2]`
- `tile_size`, `tiles_per_chunk`
- `tiles`: `TerrainTile` array (tile id + flags), row-major
- `heights`: height samples stored in a `(width + 1) x (height + 1)` grid
- `mesh_entry`: geometry entry name for rendering

This directory is a staging database, and shows how a 'staging db' gets built into a fully fledged database.

To do this, run `dbgen sample_pre/norenbuild.json` and it'll create the output `db` which is usable with samples. Use `dbgen --append sample_pre/norenbuild.json` if you only want to add new entries to existing `.rdb` files without rebuilding them from scratch.

Meta data now lives in per-entity files under both `sample_pre/` and `db/`:

- `textures.json` – texture entries that reference imagery in `imagery.rdb`
- `materials.json` – material bindings and metadata (bindless layers, camera defaults, etc.)
- `meshes.json` – mesh entries that point to geometry and optional materials
- `models.json` – logical models that bundle together meshes
- `shaders.json` – shader layout metadata (including attachment formats) that references compiled modules in `shaders.rdb`

Every graphics shader in `shaders.json` must declare the attachment formats it targets (via `color_formats` and optional `depth_format`). These values inform pipeline creation without requiring a render pass to be present in the database.

When you just need to add a single resource, you can skip `norenbuild.json` entirely and write straight into an `.rdb`:

```
dbgen append geometry --rdb db/geometry.rdb --entry geometry/new_quad --gltf sample_pre/gltf/quad.gltf --mesh Quad
dbgen append imagery --rdb db/imagery.rdb --entry imagery/peppers --image sample_pre/imagery/peppers.png --format rgba8
dbgen append shader --rdb db/shaders.rdb --entry shader/quad.frag --stage fragment --shader sample_pre/shaders/quad.frag
```


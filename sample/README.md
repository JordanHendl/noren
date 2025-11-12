This directory is a staging database, and shows how a 'staging db' gets built into a fully fledged database.

To do this, run `dbgen sample_pre/norenbuild.json` and it'll create the output `db` which is usable with samples. Use `dbgen --append sample_pre/norenbuild.json` if you only want to add new entries to existing `.rdb` files without rebuilding them from scratch.

Render passes are authored in `db/render_passes.json`, while `db/models.json` continues to describe textures, materials, meshes, and shaders. Each shader references the render pass it expects via the `render_pass` field. These definitions mirror the data consumed by `dashi::builders::RenderPassBuilder` so the sample can construct a compatible render pass at runtime.

When you just need to add a single resource, you can skip `norenbuild.json` entirely and write straight into an `.rdb`:

```
dbgen append geometry --rdb db/geometry.rdb --entry geometry/new_quad --gltf sample_pre/gltf/quad.gltf --mesh Quad
dbgen append imagery --rdb db/imagery.rdb --entry imagery/peppers --image sample_pre/imagery/peppers.png --format rgba8
dbgen append shader --rdb db/shaders.rdb --entry shader/quad.frag --stage fragment --shader sample_pre/shaders/quad.frag
```


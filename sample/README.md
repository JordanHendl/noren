This directory is a staging database, and shows how a 'staging db' gets built into a fully fledged database.

To do this, run 'noren_dbgen sample_pre/norenbuild.json' and it'll create the output 'db' which is usable with samples.

Render passes are authored in `db/render_passes.json`, while `db/models.json` continues to describe textures, materials, meshes, and shaders. Each shader references the render pass it expects via the `render_pass` field. These definitions mirror the data consumed by `dashi::builders::RenderPassBuilder` so the sample can construct a compatible render pass at runtime.

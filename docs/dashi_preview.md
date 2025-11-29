# Dashi-backed material preview feasibility

## What the editor does today
- `MaterialPreviewPanel` owns a `PreviewRenderer` that ray-marches the selected mesh into an `egui::ColorImage` by hand, so it never talks to Dashi at all. The panel simply re-uploads that CPU buffer whenever the preview changes and draws it as a regular egui image.【F:src/material_editor/preview.rs†L20-L141】
- `MaterialEditorApp` only keeps the on-disk `MaterialEditorProjectState` plus UI bookkeeping; there is no GPU context or runtime cache anywhere in the editor state today.【F:src/material_editor/ui.rs†L19-L140】
- The `material_editor` binary bootstraps eframe with either the glow or wgpu renderer, but it never asks Dashi for a device/context. That means the preview can run on systems without Vulkan and never has to synchronize with Dashi's command queues.【F:src/exec/material_editor/bin.rs†L1-L52】

## What would be needed to render with Dashi
1. **Create and own a Dashi GPU context inside the editor**
   - Dashi code elsewhere in the repo expects a live `dashi::gpu::Context` (or the lower-level `dashi::Context`) that is created once and shared with loaders. Today only the runtime/CLI paths do that (see the example helper that builds a headless context via `gpu::Context::new`).【F:examples/common/mod.rs†L6-L48】
   - The editor would need to attempt the same during startup, surface failures to the user (because Vulkan devices are optional), and plumb a mutable reference to that context through `MaterialEditorApp` so a preview renderer can submit work. Without that structural change none of the preview code can call into Dashi.

2. **Translate editor assets into Dashi resources**
   - The runtime `DB` type already knows how to load meshes, textures, materials, and shader modules from the database layout, but it leaves render pass construction and pipeline setup to the caller.【F:src/lib.rs†L24-L320】
   - The editor graph (`MaterialEditorProjectState`) only contains deserialized JSON—it never resolves shader binaries or GPU buffers.【F:src/material_editor/project.rs†L16-L146】 To drive a Dashi preview you would either need to embed a slimmed-down variant of `DB` that can read from the project's `db/` folders, or teach the editor project graph to lazily fetch the same RDB entries (geometry, imagery, compiled shaders) and reuse the existing cache types (`GeometryDB`, `ImageDB`, `ShaderDB`).
   - Canonical preview meshes (sphere/quad) would need to exist as Dashi vertex/index buffers. Either generate them procedurally each session or ship them inside a tiny RDB so that the same uploading path can be reused.

3. **Render to an off-screen Dashi image and share it with egui**
   - Egui expects preview textures to arrive as CPU `ColorImage`s (see `update_texture`), because the widget tree is rendered by eframe's glow/wgpu backend.【F:src/material_editor/preview.rs†L124-L141】 A Dashi render pass would therefore need an off-screen color attachment, a command submission, and a read-back step (copy to a host-visible buffer, then map/convert to RGBA) each frame you want to refresh.
   - Alternatively, egui's `TextureId::User` path could be used, but that would require writing a custom `egui::Painter` integration so that Dashi-owned textures can be sampled directly inside the GUI renderer. That is significantly more invasive than the current CPU upload approach and would have to bridge two graphics APIs inside the same window.

4. **Preview shader/pipeline compatibility checks**
   - The software renderer can get away with sampling whichever texture happens to exist and falling back to a constant color. A Dashi preview would still need a render pass and pipeline layout to sample textures with the GPU, but those pieces now have to be assembled by the preview code rather than the database layer.【F:src/lib.rs†L24-L320】
   - Missing shader stages or incompatible bindings should surface as validation errors instead of silently falling back to a default look. Extra UI affordances (e.g., per-material pipeline compilation status) would help users debug why the preview failed.

## Practical hurdles
- **Platform coverage**: eframe/glow currently lets the editor run anywhere an OpenGL context exists. Pulling in Dashi implies Vulkan (or whatever backend Dashi targets) must be available; otherwise the preview has to gracefully degrade back to the software path.
- **Startup/runtime cost**: Creating a Dashi context, streaming project textures into GPU memory, and compiling pipelines all incur non-trivial cost compared to the instantaneous CPU rasterizer. Caching and background loading would need to be added to avoid freezing the UI whenever the selected material changes.
- **Testing**: CI machines or contributor laptops without the necessary GPU extensions would be unable to run the preview tests unless we keep the software fallback and guard Dashi usage behind feature flags or runtime toggles.

In short, rendering the preview with Dashi is feasible, but it requires architectural changes: initialize and route a GPU context through the editor, reuse (or refactor) the runtime loaders so materials can be uploaded to Dashi resources, and add an interop layer that copies the off-screen result back into egui textures. Until those pieces are in place the current CPU rasterizer remains the simplest cross-platform option.

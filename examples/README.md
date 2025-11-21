# [Examples]

Examples of using 'noren' to render visual data (through dashi).

For these examples, they all reference the sample database in the root source directory ('{root}/samples/db'). See the README.md there to see how it is generated.

Available examples:

- `pipeline_layouts`: builds the default render graph (requesting the `shader/default` graphics shader) and prints the bind group and bind table layout handles that accompany the graphics pipeline so you know exactly which layouts to feed into your bind builders before issuing draw calls.
- `material_render`: pulls a material by database key (defaults to `material/quad`) and renders the bundled quad geometry using its shader pipeline.


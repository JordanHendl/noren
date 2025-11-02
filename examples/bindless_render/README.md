# [bindless_render]

Builds a bindless texture table from database assets. Running
`cargo run --example bindless_render` writes two artifacts:

* `target/example_outputs/bindless_render/table.txt` – summary of the created
  table, layout, and GPU handles.
* `target/example_outputs/bindless_render/texture.png` – copy of the uploaded
  tulip texture.

Inspecting these files verifies that the bindless resources were populated.

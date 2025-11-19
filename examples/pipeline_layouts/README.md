# pipeline_layouts

Demonstrates how to pull bind group and bind table layout handles from a `PipelineBinding` built with the bundled sample database.

The sample builds a render graph by requesting the `shader/default` entry from the sample layout file. That request causes the
pipeline factory to assemble the render pass, graphics pipeline, and the bind layouts declared by that shader. When you run the
example it prints the layout handles you should pass into your bind-group and bind-table builders before binding the pipeline
on a command list.

Rendering is not driven here—the example only introspects the layouts. To draw, begin the reported render pass on a command
list, bind the printed pipeline/layout pair, attach your bind groups/tables, then supply vertex/index buffers and draw calls.

Run with `cargo run --example pipeline_layouts`.

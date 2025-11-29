# [model_render]

Simple app to fetch default primitive geometry from the database and render it
to the display. Run with `cargo run --example model_render [primitive]` to
select a primitive (defaults to `sphere`). Supported primitive arguments are
`sphere`, `cube`, `quad`, `plane`, `cylinder`, and `cone` (the `geometry/`
prefix is optional).

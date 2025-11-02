# [image_blit]

Loads an image from the database, records a GPU blit, and dumps the source
pixels to disk. The resulting PNG is written to
`target/example_outputs/image_blit/blit.png`.

Run `cargo run --example image_blit` and inspect the PNG to confirm that it
matches the tulip texture in the sample database.

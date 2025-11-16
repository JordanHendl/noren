# Noren

Noren is an asset database toolkit for rendering applications. It contains a
runtime crate that can read ready-only database files as well as tooling for
building those databases from staging assets. The project is built in Rust and
targets `wgpu`/`glow` back ends through `eframe` and the `dashi` rendering
framework.

## Features

- **Database-backed runtime** – `src/lib.rs` exposes the `DB` type that lazily
  maps geometry, imagery, shaders, and render passes from disk into GPU ready
  resources.
- **Authoring tooling** – The `dbgen`, `rdbinspect`, and `material_editor`
  binaries under `src/exec` allow you to compile, inspect, and edit database
  assets.
- **Sample content** – The `sample/` directory demonstrates how staging assets
  are compiled into `.rdb` files and how the runtime consumes them.
- **Examples** – The `examples/` folder contains rendering samples that showcase
  how to integrate Noren in small applications.

## Getting started

1. Install the [Rust toolchain](https://www.rust-lang.org/tools/install).
2. Clone this repository and enter the project directory.
3. Build the workspace:

   ```bash
   cargo build --release
   ```

## Working with sample data

The `sample/` directory contains a set of staging assets along with a
`norenbuild.json` recipe that describes the database layout. You can generate a
database in-place with:

```bash
cargo run --bin dbgen -- sample/sample_pre/norenbuild.json
```

The command produces a `sample/db/` folder with `geometry.rdb`, `imagery.rdb`,
`shaders.rdb`, `materials.json`, and `render_passes.json`. Use the `--append`
flag to incrementally add new entries without rebuilding from scratch, or call
`dbgen append` to inject a single resource (geometry, imagery, or shader) into an
existing `.rdb` file.

## Running examples

Each subdirectory under `examples/` is a self-contained binary that demonstrates
a specific area of the API. You can run them with `cargo run --example
<name>`. For example, to load the hello database sample:

```bash
cargo run --example hello_database
```

## Testing

Run the unit test suite with:

```bash
cargo test
```

## License

This repository does not currently declare an explicit license. Please contact
the maintainers before using it in production.

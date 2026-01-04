# Noren

Noren is an asset database toolkit for rendering applications. It contains a
runtime crate that can read ready-only database files as well as tooling for
building those databases from staging assets. The project is built in Rust and
targets `wgpu`/`glow` back ends through `eframe` and the `dashi` rendering
framework.

## Features

- **Database-backed runtime** – `src/lib.rs` exposes the `DB` type that lazily
  maps geometry, imagery, shaders, and shader metadata (including attachment
  formats) from disk into GPU ready resources.
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
`skeletons.rdb`, `animations.rdb`, `audio.rdb`, `shaders.rdb`, and metadata JSON files
(`materials.json`, `textures.json`, `meshes.json`, `models.json`,
`shaders.json`). Use the `--append` flag to incrementally add new entries
without rebuilding from scratch or the `--layouts-only` flag to refresh the JSON
layouts without touching the binary `.rdb` payloads. You can also call `dbgen
append` to inject a single resource (geometry, imagery, skeleton, animation, or
shader) into an existing database file.

### Default audio samples

Noren ships with two default audio samples that are always built into the audio
database and always available at runtime, even if the audio database is empty or
missing:

- `audio/beep` (`sample/sample_pre/audio/beep.wav`)
- `audio/tone` (`sample/sample_pre/audio/tone.wav`)

These samples are embedded into the runtime as fallbacks and are also injected
into `audio.rdb` whenever `dbgen` builds or appends audio content.

### Loading animated assets

The staging area now ships with `sample/sample_pre/gltf/SimpleSkin.gltf`, a
minimal skinned mesh with a rotation animation. After rebuilding the sample
database, you can verify that runtime loading works with:

```bash
cargo run --example skeleton_animation_load
```

The example prints the joint count and animation clip duration for the bundled
assets while reusing the generated database.

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

## Packaging installers

Platform-specific packages are available for the two tooling binaries (`dbgen`
and `rdbinspect`). The scripts auto-detect the host and emit the matching
format:

- **Debian / Ubuntu** – `.deb` generated via `bash dist/package.sh` (requires
  `fakeroot` and `dpkg-deb`).
- **Red Hat / Fedora** – `.rpm` generated via `bash dist/package.sh` (requires
  `rpmbuild`).
- **Windows** – self-extracting installer generated via PowerShell:

  ```pwsh
  pwsh -File dist/package.ps1
  ```

  The Windows script uses `7z.exe` when available to produce
  `noren-tools-installer.exe`; otherwise it falls back to a `.zip` archive. Both
  outputs include an `install.bat` helper that copies the binaries into
  `%ProgramFiles%\NorenTools\bin`.

If the platform is not detected (for example, macOS), `bash dist/package.sh`
defaults to a `tar.gz` payload containing the two binaries under
`usr/local/bin`.

## License

This repository does not currently declare an explicit license. Please contact
the maintainers before using it in production.

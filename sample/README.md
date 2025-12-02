# Sample database staging assets

This directory shows how a staging database is compiled into the ready-to-load
artifacts under `sample/db/`.

## Building the sample database

Generate (or refresh) the database from the root of the repository with:

```
cargo run --bin dbgen -- sample_pre/norenbuild.json
```

The recipe now emits geometry, imagery, skeletons, animations, shaders, and
supporting metadata. When you only need to update the JSON layout files without
rewriting the binary `.rdb` payloads, pass `--layouts-only`:

```
cargo run --bin dbgen -- --layouts-only sample_pre/norenbuild.json
```

Use `--append` for incremental rebuilds, or `dbgen append` to write an
individual asset directly into an `.rdb`:

```
dbgen append geometry --rdb db/geometry.rdb --entry geometry/new_quad --gltf sample_pre/gltf/quad.gltf --mesh Quad
dbgen append skeleton --rdb db/skeletons.rdb --entry skeletons/simple_skin --gltf sample_pre/gltf/SimpleSkin.gltf
dbgen append animation --rdb db/animations.rdb --entry animations/simple_skin --gltf sample_pre/gltf/SimpleSkin.gltf
dbgen append imagery --rdb db/imagery.rdb --entry imagery/peppers --image sample_pre/imagery/peppers.png --format rgba8
dbgen append shader --rdb db/shaders.rdb --entry shader/quad.frag --stage fragment --shader sample_pre/shaders/quad.frag
```

## Animated sample content

`sample_pre/gltf/SimpleSkin.gltf` contains a two-joint skeleton with a simple
rotation animation. The `norenbuild.json` recipe compiles it into
`skeletons.rdb` and `animations.rdb`, and also adds a `geometry/simple_skin`
entry plus a matching `model/simple_skin` for quick inspection.

After building the database, run the example application to confirm the runtime
can read the skinned assets:

```
cargo run --example skeleton_animation_load
```

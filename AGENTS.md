# Repository agent instructions

- After making changes that affect database assets or sample content, run:
  `cargo run --bin dbgen -- --layouts-only sample/sample_pre/norenbuild.json`
  to refresh the JSON layout outputs without regenerating binary `.rdb` files.
- Use `-v`/`--verbose` with dbgen when troubleshooting to capture progress logs.

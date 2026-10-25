# 0008 Project file format and migrations

`.cantor` files are a JSON envelope `{format: "cantor-project", version, project}`,
pretty-printed by default and optionally zstd-compressed (detected on load by
the zstd magic number, so the extension does not change). Saves are atomic
(temporary file, then rename).

Version 1 is the first format, so `MIGRATIONS` is empty. The migration
machinery (`from_bytes_with`) takes the chain as a parameter and is tested
with a synthetic v1 → v2 → v3 chain, plus the failure cases: a file newer
than the build, a missing step, a non-Cantor JSON file. Migrations operate on
`serde_json::Value` so old shapes need no Rust types.

Loading canonicalizes (with an info diagnostic if anything moved) and
validates; invariant violations are a load error, not a silent repair.

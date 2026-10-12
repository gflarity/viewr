# Attribution

`viewr` is licensed under the Apache License, Version 2.0 (see `LICENSE-APACHE`).

## Vendored LightCraft code

The TIFF reader and the NEF embedded-preview extractor are taken from
[LightCraft](https://github.com/storytold/lightcraft),
Copyright (c) 2026 ArtCraft Team and the LightCraft contributors.
LightCraft is licensed under the MIT License or the Apache License,
Version 2.0, at your option; `viewr` uses the Apache-2.0 option, and each
vendored file carries a header saying so.

| File in this repo | Source in LightCraft | Changes from upstream |
|---|---|---|
| `src/tiff/mod.rs` | `crates/tiff/src/lib.rs` | Attribution header; module doc retitled; `writer` gated to `cfg(test)` (fixtures only) |
| `src/tiff/reader.rs` | `crates/tiff/src/reader.rs` | Attribution header; `crate::` paths rewritten to `crate::tiff::` |
| `src/tiff/tags.rs` | `crates/tiff/src/tags.rs` | Attribution header only |
| `src/tiff/image.rs` | `crates/tiff/src/image.rs` | Attribution header; `crate::` paths rewritten to `crate::tiff::` |
| `src/tiff/makernote.rs` | `crates/tiff/src/makernote.rs` | Attribution header; `crate::` paths rewritten to `crate::tiff::` |
| `src/tiff/writer.rs` | `crates/tiff/src/writer.rs` | Attribution header; `crate::` paths rewritten to `crate::tiff::` |
| `src/nef.rs` | `crates/raw/src/preview.rs` | Attribution header; trimmed to the TIFF/NEF embedded-JPEG path (Fujifilm/Canon CR3/ORF/Sony/vendor-specific branches, JPEG XL previews, `bounded_preview`, and colour-space helpers removed); tests adapted to the kept subset |

Everything else in `src/` is original to `viewr`.

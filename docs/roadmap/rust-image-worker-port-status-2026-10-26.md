# IMG-01…03: PORT — Rust image pipeline in TOR

Source: NEwBOO image-worker @17125ef758a791e9630505c14d24f7fe65aed228.
Feature G: crates/ingest/src/services/image_pipeline. No Python callback or HTTP worker.

Implemented in this PR:
- Pure Rust: JPEG/PNG/WebP format verification and strict container checks; dimensions ≤5000px/24MP; source ≤8MiB before decompression.
- Reused bounded image::ImageReader + decoder, fast_image_resize Lanczos3 crop-to-fill and original cascading downscale, WebP lossless encoder.
- Two cover variants: 380x570 main, 150x220 card, with no storage keys controlled by the input.
- Corrected misleading quality setting: explicitly fixed lossless encoder, versioned recipe.
- Single-threaded/synchronous recipe, to be invoked by G's future bounded spawn_blocking/PG worker (no Rayon fanout, no unbounded async decode).
- Rust integration tests use actual JPEG/PNG/WebP bytes generated at test time and assert decoded output/crop/alpha; negative tests cover MIME, size, metadata, animation, trailing data, dimensions.
- Dedicated changed-path workflow "Rust G image pipeline"; general TOR feature/layout and contract checks remain required.

Not implemented by this slice: B ownership/cover-slot commands, S3 upload/presign, PG job queue/leases/retries, image variants attachment/GC, browser/real S3. Therefore S025 is PARTIAL, G-05 core only; G-07 and G-09 remain open. No real publisher traffic or secret credentials.

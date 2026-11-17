# G-05d — 55-book-cover WebP q85 pilot (18 November 2026)

**Decision:** promising enough for an explicitly enabled *new-book-cover canary*, but
**not** evidence to rewrite historical immutable covers or turn on a production
default automatically. This is an offline quality and size experiment, not
production telemetry.

## Corpus and reproducibility

- 55 distinct book-cover designs (selected from 62 locally available cover
  images; overlapping design variants excluded). All input files are RGB PNG
  **1024 × 1536**. Not a random sample of live TOR book covers.
- Each source was resized via Pillow Lanczos center-fit to TOR's 380 × 570
  `main` and 150 × 220 `card` outputs.
- Compared Pillow/libwebp lossless (method 4) with WebP q85 (method 4).
  The **110 q85 byte lengths** were independently checked using native
  libwebp `WebPEncodeRGB` at quality 85 and matched **110/110**.
- This validates the libwebp encoding behavior for those resized pixel inputs.
  It does **not** establish byte identity with TOR's Rust `image` lossless
  encoder or its `fast_image_resize` transform. Run
  `cargo run -p tor-ingest --example cover_codec_bench` on the same corpus in a
  suitable Rust environment before adopting these values as production forecasts.
- Inputs, previews and per-file fingerprints are intentionally **not committed**
  to the repository.

## Results — 55 designs / 110 output images

| Metric | Main 380 × 570 | Card 150 × 220 |
| --- | ---: | ---: |
| Average lossless WebP | 339.09 KiB | 60.08 KiB |
| Average lossy WebP q85 | 71.94 KiB | 15.27 KiB |
| Aggregate byte saving | **78.79%** | **74.59%** |
| Worst saving | 75.12% | 71.08% |
| 10th / median / 90th percentile saving | 76.84 / 78.73 / 81.37% | 72.86 / 74.66 / 76.52% |
| Median SSIM vs lossless decode | 0.9522 | 0.9588 |
| Minimum SSIM | 0.8284 | 0.8139 |
| Mean encode time, lossless *in this Pillow pilot* | 156.4 ms | 52.6 ms |
| Mean encode time, q85 *in this Pillow pilot* | 31.8 ms | 6.7 ms |

All 110 q85 outputs were smaller than their corresponding lossless outputs.
Total average per-book size: **399.17 KiB → 87.21 KiB** for both variants
(78.15% fewer bytes).

For an assumed 100,000 covers with these *same input characteristics*, private
storage for one `main` + one `card` would be approximately
**38.07 GiB → 8.32 GiB**, a saving of **29.75 GiB**.
This excludes originals, historical versions, additional public CDN origin
copies and actual browser/CDN cache behavior. Do not label it a current TOR
storage saving or forecast it as a randomly sampled population estimate.

At 50 fetched cards per catalogue screen, the hypothetical transfer shrinks
from about 2.93 MiB to 0.75 MiB. Browser caching, lazy-loading and response
headers determine whether 50 actual downloads take place.

## Quality review and limits

The eight lowest-SSIM cards were reviewed as paired 150 × 220 images shown
at 2× enlargement. Titles remained legible in those examples, while fine
textures, glow and small decorative details lost fidelity.

Two art-heavy images produced visibly lower SSIM (approx. 0.814 and 0.858
for their cards). Lower SSIM is **not** proof that the title became unreadable,
and title readability is not proved by SSIM either. Keep checking microtype,
non-Latin text and covers with small author names.

Critical limits:
- all sources have the same aspect ratio, size, RGB mode and PNG format;
- transparent artwork, previously lossy JPEG/WebP sources and widely different
  source dimensions were not represented in the 55-cover corpus;
- production Rust worker concurrency, CPU budget and memory use were **not**
  measured here;
- no blind multi-reviewer assessment was conducted.

## Safe next step

1. Run the *shipping Rust* `cover_codec_bench` on this or a similarly diverse
   set of 50–100 book covers; examine individual negative savings and card
   text legibility, particularly at actual physical display size.
2. On an isolated/test environment only, set
   `TOR_COVER_LOSSY_WEBP_ENABLED=1`. This stamps the v2 recipe on **new**
   author cover jobs; queued v1 jobs retain lossless encoding.
3. Confirm owner upload → source verification → worker → moderation →
   S3/CDN/public-browser journey; verify correct `image/webp` and VP8 bytes.
   Integration CI tests already exercise this route.
4. Compare bounded worker CPU/memory, processing latency, and fallback rate to
   baseline before changing the default on a public server.
5. Abort new q85 ingestion by setting
   `TOR_COVER_LOSSY_WEBP_ENABLED=0`. This does **not** revert already
   completed/published v2 jobs; historical cover jobs remain reproducible.
6. Keep old-cover conversion a separate project. It requires true S3 source
   presence, new immutable keys and a moderation/CAS/CDN promotion path.

**No VPS, S3, production settings or migrations were touched by this pilot.**

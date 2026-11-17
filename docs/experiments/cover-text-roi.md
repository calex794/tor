# Text-aware cover compression: offline experiment (2026-11-17)

Status: **experiment only**. No production feature flag, no replacement of the current lossless-WebP G-05 recipe, and no changes to S3, PostgreSQL or browser contracts.

## Goal and baseline

Current TOR `crates/ingest/src/services/image_pipeline/orchestrator.rs` generates two variants, 380 x 570 and 150 x 220, using lossless WebP. We are testing whether recognizing title/text zones and simplifying *only the background* improves title fidelity at an equal AVIF byte budget.

This is **pseudo-ROI preprocessing**, not AV1 quantizer-region control. The final AVIF encoder still makes independent bit-allocation decisions and may damage text.

## Run locally

Environment: Python 3, Pillow with AVIF support, NumPy, scikit-image, Tesseract executable with `rus` and `eng` languages installed. These are experimental offline dependencies, not TOR runtime requirements.

```sh
python3 -m pip install Pillow numpy scikit-image
tesseract --list-langs
python3 tools/cover_roi_experiment.py /path/to/cover.png --out /tmp/cover-roi --sizes 380x570 150x220 --budgets 10 15 20 --blur-radii 0.8 1.4 2.2
PYTHONPATH=tools python3 -m unittest discover -s tools -p test_cover_roi_experiment.py -v
```

Artifacts for each size/budget: AVIF variants, ROI mask, side-by-side visual gallery, `measurements.csv`, `selected.json`, and selected AVIF. Do not commit real user cover images or output images to the repository.

## Decision guard

At identical byte budget, select pseudo-ROI only when:

- the measured SSIM in the detected text zone improves by at least 0.003;
- background SSIM decreases by at most 0.025;
- whole-frame SSIM decreases by at most 0.025.

Otherwise select the uniform AVIF. Empty or missing text regions also select uniform AVIF. These are **prototype thresholds**, not validated product acceptance limits.

## Local observations (single book cover; do not generalize)

The real-world source was the user's book cover "Я просто инвестирую…", resized to 380 x 570. At a 15-KiB target:

| Measurement | Uniform AVIF | Selective prefilter (radius 0.8) |
|---|---:|---:|
| Actual file bytes | 14,846 | 14,796 |
| Text SSIM | 0.934612 | 0.941420 |
| Background SSIM | 0.935617 | 0.915363 |
| Full-frame SSIM | 0.935414 | 0.920642 |

This demonstrates a **tradeoff**, not an overall-quality improvement. OCR detects ten candidate text boxes on this source; approximately 20% of the 380x570 image is protected after padding. Local tests: 7/7 pass. A separate 150x220 trial and one TRIZ teaching cover also ran; representativeness is insufficient.

## Rejected shortcut

A local FFmpeg `addroi` + `libaom-av1` / AVIF smoke test emitted identical bytes with and without `addroi`. This does **not** demonstrate that AV1/AVIF ROI is impossible; it demonstrates that this specific pipeline did not apply ROI. A future codec-level ROI implementation needs an encoder proven to honor per-region quantization, with byte-level and decoded-image tests.

## Risks and next gate

- OCR can miss artistic fonts and hallucinate text in drawings. Enforce limits and try diverse scripts.
- Background blurring can visibly damage faces, patterns, or edges; ROI-SSIM alone is insufficient for ship decisions.
- SSIM text-region improvement is not the same as unchanged spelling. Add manual legibility and character-level OCR evaluation.
- The current experiment runs OCR on the uploaded source, before center-crop; production must also secure decoder limits, worker CPU and cancellation.
- JPEG, WebP lossy and uniform AVIF should remain baselines; evaluate at *matched bytes* on at least 50-100 diverse covers.
- Do not change the accepted `RECIPE_VERSION` or write to S3 until the experiment passes its quality gate and a separate production PR is reviewed.

The only intended next change is to extend the offline benchmark with OCR accuracy and a validated codec-level ROI backend, or reject ROI and choose uniform lossy encoding if the improvement is not worth the background cost.

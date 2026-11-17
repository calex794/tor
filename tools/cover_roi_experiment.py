#!/usr/bin/env python3
"""Offline cover-text ROI experiment. No calls to the TOR storage or database.

Requires Pillow, NumPy, scikit-image, and Tesseract (rus+eng language data).
Example:
  python tools/cover_roi_experiment.py cover.png --out /tmp/roi --budgets 10 15 20

This is NOT codec-level ROI: detected text is protected during a selective
background prefilter; an AVIF encoder still makes the final bit allocation.
"""
from __future__ import annotations

import argparse
import csv
import io
import json
import math
import subprocess
from pathlib import Path

import numpy as np
from PIL import Image, ImageDraw, ImageFilter
from skimage.metrics import structural_similarity


def text_boxes(path: Path, lang: str, confidence: float = 65) -> list[tuple[int, int, int, int]]:
    command = ["tesseract", str(path), "stdout", "-l", lang, "--psm", "11", "tsv"]
    result = subprocess.run(command, text=True, capture_output=True, check=True, timeout=30)
    boxes = []
    for item in csv.DictReader(io.StringIO(result.stdout), delimiter="\t"):
        token = (item.get("text") or "").strip()
        try:
            score = float(item["conf"])
            x, y, w, h = (int(item[k]) for k in ("left", "top", "width", "height"))
        except (ValueError, KeyError, TypeError):
            continue
        letters = [ch for ch in token if ch.isalpha()]
        if score < confidence or not letters or (len(letters) == 1 and not (score > 85 and "А" <= letters[0].upper() <= "Я")):
            continue
        if w > 0 and h > 0:
            boxes.append((x, y, x + w, y + h))
    return boxes


def resize_cover(source: Image.Image, size: tuple[int, int]) -> tuple[Image.Image, tuple[int, int, int, int]]:
    w, h = source.size
    out_w, out_h = size
    crop_ratio = out_w / out_h
    source_ratio = w / h
    if source_ratio > crop_ratio:
        crop_h = h
        crop_w = round(h * crop_ratio)
    else:
        crop_w = w
        crop_h = round(w / crop_ratio)
    x0, y0 = (w - crop_w) // 2, (h - crop_h) // 2
    crop = (x0, y0, x0 + crop_w, y0 + crop_h)
    return source.crop(crop).convert("RGB").resize(size, Image.Resampling.LANCZOS), crop


def make_mask(boxes: list[tuple[int, int, int, int]], crop: tuple[int, int, int, int],
              size: tuple[int, int], padding: int = 3) -> Image.Image:
    mask = Image.new("L", size, 0)
    draw = ImageDraw.Draw(mask)
    cx, cy, cr, cb = crop
    sx, sy = size[0] / (cr - cx), size[1] / (cb - cy)
    for x0, y0, x1, y1 in boxes:
        left = max(0, math.floor((x0 - cx) * sx) - padding)
        top = max(0, math.floor((y0 - cy) * sy) - padding)
        right = min(size[0] - 1, math.ceil((x1 - cx) * sx) + padding)
        bottom = min(size[1] - 1, math.ceil((y1 - cy) * sy) + padding)
        if right > left and bottom > top:
            draw.rectangle((left, top, right, bottom), fill=255)
    return mask


def selective_prefilter(image: Image.Image, mask: Image.Image, radius: float) -> Image.Image:
    if radius <= 0:
        return image.copy()
    blurred = image.filter(ImageFilter.GaussianBlur(radius))
    feathered = mask.filter(ImageFilter.GaussianBlur(1.2))
    return Image.composite(image, blurred, feathered)


def encode_budget(image: Image.Image, budget: int, speed: int = 6) -> tuple[bytes, int]:
    """Choose highest AVIF quality not exceeding byte budget; near quality nonmonotonicity is handled."""
    cache = {}

    def encode(q: int) -> bytes:
        if q not in cache:
            buf = io.BytesIO()
            image.save(buf, format="AVIF", quality=q, speed=speed)
            cache[q] = buf.getvalue()
        return cache[q]

    lo, hi = 0, 100
    while lo <= hi:
        middle = (lo + hi) // 2
        if len(encode(middle)) <= budget:
            lo = middle + 1
        else:
            hi = middle - 1
    candidates = set(range(max(0, hi - 4), min(100, hi + 5) + 1))
    candidates.update((0, 100))
    valid = [(q, encode(q)) for q in candidates if len(encode(q)) <= budget]
    if valid:
        q, data = max(valid, key=lambda p: (p[0], len(p[1])))
        return data, q
    raise ValueError(f"AVIF cannot fit within {budget} bytes even at minimum quality")


def distortion(reference: Image.Image, encoded: bytes, mask: Image.Image) -> tuple[float, float, float]:
    decoded = Image.open(io.BytesIO(encoded)).convert("RGB")
    original = np.asarray(reference, dtype=np.uint8)
    candidate = np.asarray(decoded, dtype=np.uint8)
    _, quality_map = structural_similarity(original, candidate, channel_axis=2, data_range=255, full=True)
    score = quality_map.mean(axis=2)
    protected = np.asarray(mask) > 127
    if protected.any() and (~protected).any():
        return float(score.mean()), float(score[protected].mean()), float(score[~protected].mean())
    return float(score.mean()), float("nan"), float(score.mean())


def visualize(image: Image.Image, mask: Image.Image, originals: list[Image.Image],
              labels: list[str], out: Path) -> None:
    w, h = image.size
    canvas = Image.new("RGB", (w * len(originals), h + 40), "white")
    draw = ImageDraw.Draw(canvas)
    for i, (im, label) in enumerate(zip(originals, labels)):
        canvas.paste(im, (i * w, 0))
        draw.text((i * w + 8, h + 10), label, fill="black")
    canvas.save(out)
    overlay = Image.new("RGBA", image.size, (255, 35, 50, 0))
    overlay.putalpha(mask.point(lambda p: round(p * 0.45)))
    Image.alpha_composite(image.convert("RGBA"), overlay).convert("RGB").save(out.with_name(out.stem + "_roi.png"))


def run(args: argparse.Namespace) -> list[dict]:
    args.out.mkdir(parents=True, exist_ok=True)
    source = Image.open(args.input)
    boxes = text_boxes(args.input, args.lang, args.confidence)
    rows = []
    for width, height in args.sizes:
        reference, crop = resize_cover(source, (width, height))
        mask = make_mask(boxes, crop, (width, height), args.padding)
        name = f"{width}x{height}"
        reference.save(args.out / f"{name}_reference.png")
        mask.save(args.out / f"{name}_mask.png")
        protected_share = np.asarray(mask).mean() / 255
        for budget_kib in args.budgets:
            budget_bytes = round(budget_kib * 1024)
            candidates = [("uniform", 0.0, reference)]
            if boxes and protected_share < args.max_roi_share:
                for radius in args.blur_radii:
                    candidates.append(("selective", radius, selective_prefilter(reference, mask, radius)))
            gallery, gallery_labels = [], []
            for kind, radius, candidate in candidates:
                encoded, quality = encode_budget(candidate, budget_bytes, args.speed)
                score, text_score, background_score = distortion(reference, encoded, mask)
                result_name = f"{name}_{budget_kib:g}k_{kind}_blur{radius:g}.avif"
                (args.out / result_name).write_bytes(encoded)
                gallery.append(Image.open(io.BytesIO(encoded)).convert("RGB"))
                gallery_labels.append(f"{kind} r={radius:g} q={quality}")
                rows.append({
                    "image": args.input.name, "size": name, "budget_kib": budget_kib,
                    "mode": kind, "blur_radius": radius, "quality": quality,
                    "bytes": len(encoded), "roi_share": round(protected_share, 4),
                    "text_boxes": len(boxes), "ssim_full": round(score, 6),
                    "ssim_text": round(text_score, 6),
                    "ssim_background": round(background_score, 6),
                    "filename": result_name,
                })
            visualize(reference, mask, gallery, gallery_labels, args.out / f"{name}_{budget_kib:g}k_gallery.png")
    (args.out / "detected_boxes.json").write_text(json.dumps(boxes, ensure_ascii=False, indent=2))
    with (args.out / "measurements.csv").open("w", encoding="utf-8", newline="") as f:
        writer = csv.DictWriter(f, fieldnames=list(rows[0]))
        writer.writeheader()
        writer.writerows(rows)
    selections = []
    for size in sorted({r["size"] for r in rows}):
        for budget in args.budgets:
            batch = [r for r in rows if r["size"] == size and r["budget_kib"] == budget]
            baseline = next(r for r in batch if r["mode"] == "uniform")
            selected = choose_candidate(batch, args.min_text_gain, args.max_background_loss, args.max_full_loss)
            selected_file = args.out / f"{size}_{budget:g}k_selected.avif"
            selected_file.write_bytes((args.out / selected["filename"]).read_bytes())
            selections.append({
                "size": size, "budget_kib": budget, "selected": selected["mode"],
                "blur_radius": selected["blur_radius"], "file": selected_file.name,
                "text_ssim_gain": round(selected["ssim_text"] - baseline["ssim_text"], 6)
                    if not math.isnan(baseline["ssim_text"]) else 0,
                "background_ssim_loss": round(baseline["ssim_background"] - selected["ssim_background"], 6),
                "full_ssim_loss": round(baseline["ssim_full"] - selected["ssim_full"], 6),
            })
    (args.out / "selected.json").write_text(json.dumps(selections, ensure_ascii=False, indent=2))
    return rows


def choose_candidate(batch: list[dict], min_text_gain: float = 0.003,
                     max_background_loss: float = 0.025,
                     max_full_loss: float = 0.025) -> dict:
    """Conservative gate: a text improvement must not hide large background losses."""
    baseline = next(row for row in batch if row["mode"] == "uniform")
    if math.isnan(baseline["ssim_text"]):
        return baseline
    choices = [
        row for row in batch if row["mode"] == "selective"
        and row["ssim_text"] >= baseline["ssim_text"] + min_text_gain
        and row["ssim_background"] >= baseline["ssim_background"] - max_background_loss
        and row["ssim_full"] >= baseline["ssim_full"] - max_full_loss
    ]
    return max(choices, key=lambda row: row["ssim_text"], default=baseline)


def parse_size(s: str) -> tuple[int, int]:
    left, right = s.lower().split("x")
    return int(left), int(right)


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("input", type=Path)
    parser.add_argument("--out", type=Path, required=True)
    parser.add_argument("--sizes", type=parse_size, nargs="+", default=[(380, 570), (150, 220)])
    parser.add_argument("--budgets", type=float, nargs="+", default=[10, 15, 20])
    parser.add_argument("--blur-radii", type=float, nargs="+", default=[0.8, 1.4, 2.2])
    parser.add_argument("--lang", default="rus+eng")
    parser.add_argument("--confidence", type=float, default=65)
    parser.add_argument("--padding", type=int, default=3)
    parser.add_argument("--max-roi-share", type=float, default=0.45)
    parser.add_argument("--speed", type=int, default=6)
    parser.add_argument("--min-text-gain", type=float, default=0.003)
    parser.add_argument("--max-background-loss", type=float, default=0.025)
    parser.add_argument("--max-full-loss", type=float, default=0.025)
    args = parser.parse_args()
    rows = run(args)
    for size in sorted({r["size"] for r in rows}):
        for budget in args.budgets:
            batch = [r for r in rows if r["size"] == size and r["budget_kib"] == budget]
            baseline = next(r for r in batch if r["mode"] == "uniform")
            best = choose_candidate(batch, args.min_text_gain, args.max_background_loss, args.max_full_loss)
            print(f"{size} {budget:g} KiB: uniform SSIM_text={baseline['ssim_text']:.4f} q={baseline['quality']}; "
                  f"best SSIM_text={best['ssim_text']:.4f} {best['mode']} blur={best['blur_radius']} "
                  f"q={best['quality']} (full SSIM {best['ssim_full']:.4f}, bytes {best['bytes']})")


if __name__ == "__main__":
    main()

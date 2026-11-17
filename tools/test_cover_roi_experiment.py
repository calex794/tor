#!/usr/bin/env python3
"""Unit tests for offline cover ROI probe (no network / S3 / PostgreSQL)."""
import math
import unittest

from PIL import Image

from cover_roi_experiment import (
    choose_candidate,
    encode_budget,
    make_mask,
    resize_cover,
    selective_prefilter,
)


class CoverRoiExperimentTests(unittest.TestCase):
    def test_resize_crop_and_mask_mapping(self):
        source = Image.new("RGB", (200, 300), (20, 30, 40))
        resized, crop = resize_cover(source, (100, 150))
        self.assertEqual(resized.size, (100, 150))
        mask = make_mask([(20, 30, 60, 75)], crop, resized.size, padding=0)
        self.assertEqual(mask.getpixel((20, 25)), 255)
        self.assertEqual(mask.getpixel((80, 125)), 0)

    def test_mask_clips_to_image(self):
        mask = make_mask([(-100, -10, 50, 50), (150, 200, 1000, 1000)],
                         (0, 0, 200, 300), (100, 150))
        self.assertEqual(mask.getpixel((0, 0)), 255)
        self.assertEqual(mask.getpixel((99, 149)), 255)

    def test_prefilter_preserves_text_center_and_smooths_background(self):
        image = Image.new("RGB", (60, 60))
        pixels = image.load()
        for y in range(60):
            for x in range(60):
                pixels[x, y] = (255, 255, 255) if (x + y) % 2 else (0, 0, 0)
        mask = make_mask([(24, 24, 36, 36)], (0, 0, 60, 60), (60, 60), padding=0)
        filtered = selective_prefilter(image, mask, 1.4)
        self.assertEqual(filtered.getpixel((30, 30)), image.getpixel((30, 30)))
        self.assertNotEqual(filtered.getpixel((2, 2)), image.getpixel((2, 2)))

    def test_selection_accepts_small_gain_without_background_collapse(self):
        baseline = {"mode": "uniform", "ssim_text": 0.9, "ssim_background": 0.93, "ssim_full": 0.92}
        improved = {"mode": "selective", "ssim_text": 0.91, "ssim_background": 0.915, "ssim_full": 0.91}
        self.assertIs(choose_candidate([baseline, improved]), improved)

    def test_selection_rejects_excessive_background_loss(self):
        baseline = {"mode": "uniform", "ssim_text": 0.9, "ssim_background": 0.93, "ssim_full": 0.92}
        overblurred = {"mode": "selective", "ssim_text": 0.99, "ssim_background": 0.8, "ssim_full": 0.83}
        self.assertIs(choose_candidate([baseline, overblurred]), baseline)

    def test_no_text_roi_falls_back_to_baseline(self):
        baseline = {"mode": "uniform", "ssim_text": math.nan, "ssim_background": 0.9, "ssim_full": 0.9}
        self.assertIs(choose_candidate([baseline]), baseline)

    def test_avif_size_budget_is_enforced(self):
        image = Image.new("RGB", (64, 64), (80, 90, 100))
        try:
            data, quality = encode_budget(image, 1500)
        except (KeyError, OSError, ValueError) as exc:
            self.skipTest(f"AVIF encoder not available: {exc}")
        self.assertLessEqual(len(data), 1500)
        self.assertGreaterEqual(quality, 0)
        self.assertLessEqual(quality, 100)


if __name__ == "__main__":
    unittest.main()

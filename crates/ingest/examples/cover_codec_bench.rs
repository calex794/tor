//! Read-only, offline G-05 benchmark of the ACTUAL TOR Rust cover recipes.
 //!
 //! Run with a flat directory of legitimately obtained source images:
 //!   cargo run -p tor-ingest --example cover_codec_bench -- /path/to/covers 100 > /tmp/covers.csv
 //! Optional third argument: local preview output directory.
 //!
 //! This tool makes no network/S3/PG calls and never changes input files.
 //! It prints only truncated SHA-256 IDs, not source paths or book titles.

use image::ImageFormat;
use sha2::{Digest, Sha256};
use std::{
    collections::HashSet,
    env, error::Error, fs,
    io::{self, Write},
    path::{Path, PathBuf},
    time::Instant,
};
use tor_ingest::services::image_pipeline::{
    process_cover_with_recipe, ImageVariant, LEGACY_RECIPE_VERSION, MAX_SOURCE_BYTES,
    RECIPE_VERSION,
};

const MAX_SAMPLES: usize = 500;

fn mime_for(path: &Path) -> Option<&'static str> {
    match path.extension()?.to_str()?.to_ascii_lowercase().as_str() {
        "png" => Some("image/png"),
        "jpeg" | "jpg" => Some("image/jpeg"),
        "webp" => Some("image/webp"),
        _ => None,
    }
}

fn decoded_psnr(reference: &[u8], candidate: &[u8]) -> Result<f64, Box<dyn Error>> {
    let old = image::load_from_memory_with_format(reference, ImageFormat::WebP)?.to_rgba8();
    let new = image::load_from_memory_with_format(candidate, ImageFormat::WebP)?.to_rgba8();
    if old.dimensions() != new.dimensions() {
        return Err(io::Error::other("decoded WebP dimensions changed").into());
    }
    Ok(psnr_bytes(old.as_raw(), new.as_raw()))
}

fn psnr_bytes(reference: &[u8], candidate: &[u8]) -> f64 {
    if reference.len() != candidate.len() || reference.is_empty() {
        return f64::NAN;
    }
    let squared_error: f64 = reference
        .iter()
        .zip(candidate)
        .map(|(&a, &b)| {
            let d = f64::from(a) - f64::from(b);
            d * d
        })
        .sum();
    if squared_error == 0.0 {
        return f64::INFINITY;
    }
    let mse = squared_error / reference.len() as f64;
    10.0 * (255.0 * 255.0 / mse).log10()
}

fn variant<'a>(values: &'a [ImageVariant], name: &str) -> io::Result<&'a ImageVariant> {
    values
        .iter()
        .find(|value| value.name == name)
        .ok_or_else(|| io::Error::other(format!("missing variant {name}")))
}

fn compare_one(
    id: &str,
    original: &[u8],
    mime: &str,
    preview_dir: Option<&Path>,
) -> Result<(u64, u64), Box<dyn Error>> {
    let before = Instant::now();
    let old = process_cover_with_recipe(original, mime, LEGACY_RECIPE_VERSION)?;
    let old_ms = before.elapsed().as_secs_f64() * 1000.0;

    let before = Instant::now();
    let new = process_cover_with_recipe(original, mime, RECIPE_VERSION)?;
    let new_ms = before.elapsed().as_secs_f64() * 1000.0;

    if old.len() != 2 || new.len() != 2 {
        return Err(io::Error::other("both recipes must produce two variants").into());
    }

    // Build both rows before printing. Any decode or preview failure should
    // skip an entire source instead of leaving a partial CSV source pair.
    let mut rows = Vec::with_capacity(2);
    let mut old_total = 0_u64;
    let mut new_total = 0_u64;
    for kind in ["main", "card"] {
        let legacy = variant(&old, kind)?;
        let lossy = variant(&new, kind)?;
        if (legacy.width, legacy.height) != (lossy.width, lossy.height) {
            return Err(io::Error::other("variant dimensions changed between recipes").into());
        }
        let psnr = decoded_psnr(&legacy.bytes, &lossy.bytes)?;
        let a = legacy.bytes.len() as u64;
        let b = lossy.bytes.len() as u64;
        old_total += a;
        new_total += b;
        if let Some(out) = preview_dir {
            fs::write(out.join(format!("{id}_{kind}_v1.webp")), &legacy.bytes)?;
            fs::write(out.join(format!("{id}_{kind}_v2.webp")), &lossy.bytes)?;
        }
        rows.push(format!(
            "{id},{kind},{a},{b},{:.2},{old_ms:.2},{new_ms:.2},{psnr:.2},{}",
            100.0 * (1.0 - b as f64 / a as f64),
            lossy.encoding,
        ));
    }
    for row in rows {
        println!("{row}");
    }
    Ok((old_total, new_total))
}

fn main() -> Result<(), Box<dyn Error>> {
    let args: Vec<String> = env::args().collect();
    if !(2..=4).contains(&args.len()) {
        eprintln!(
            "usage: {} <source-image-directory> [max-files=100] [local-preview-directory]",
            args[0]
        );
        return Err(io::Error::other("invalid arguments").into());
    }
    let root = Path::new(&args[1]);
    if !root.is_dir() {
        return Err(io::Error::other("source-image-directory must exist").into());
    }
    let limit = args.get(2).map_or(Ok(100), |value| value.parse::<usize>())?;
    if !(1..=MAX_SAMPLES).contains(&limit) {
        return Err(io::Error::other("max-files must be 1..=500").into());
    }
    let preview = args.get(3).map(PathBuf::from);
    if let Some(out) = &preview {
        fs::create_dir_all(out)?;
    }

    // Explicitly no recursion: a single reviewable corpus directory.
    let mut paths = Vec::new();
    for entry in fs::read_dir(root)? {
        let entry = entry?;
        if entry.file_type()?.is_file() && mime_for(&entry.path()).is_some() {
            paths.push(entry.path());
        }
    }
    paths.sort();

    println!(
        "source_sha12,variant,legacy_bytes,lossy_bytes,saving_percent,\
         legacy_pipeline_ms,lossy_pipeline_ms,psnr_vs_lossless_db,v2_encoding"
    );
    let mut seen = HashSet::new();
    let mut processed = 0_usize;
    let mut skipped = 0_usize;
    let mut old_bytes = 0_u64;
    let mut new_bytes = 0_u64;
    for path in paths {
        if processed >= limit {
            break;
        }
        let Some(mime) = mime_for(&path) else {
            continue;
        };
        let metadata = fs::metadata(&path)?;
        if metadata.len() == 0 || metadata.len() > MAX_SOURCE_BYTES as u64 {
            skipped += 1;
            continue;
        }
        let original = fs::read(&path)?;
        let sha256: [u8; 32] = Sha256::digest(&original).into();
        if !seen.insert(sha256) {
            skipped += 1;
            continue;
        }
        let id: String = sha256[..6].iter().map(|byte| format!("{byte:02x}")).collect();
        match compare_one(&id, &original, mime, preview.as_deref()) {
            Ok((old, new)) => {
                old_bytes += old;
                new_bytes += new;
                processed += 1;
            }
            Err(problem) => {
                skipped += 1;
                eprintln!("skip source {id}: {problem}");
            }
        }
    }
    io::stdout().flush()?;
    if processed == 0 {
        return Err(io::Error::other("no decodable, unique cover images were processed").into());
    }
    let percent = 100.0 * (1.0 - new_bytes as f64 / old_bytes as f64);
    eprintln!(
        "source_count={processed} skipped_or_duplicate={skipped} \
         legacy_bytes={old_bytes} lossy_bytes={new_bytes} saving_percent={percent:.2}"
    );
    if processed < 50 {
        eprintln!("WARNING: fewer than 50 unique covers; do not extrapolate to the TOR catalog");
    }
    eprintln!("PSNR is a pixel metric, not proof of text legibility; review the previews manually");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn matching_pixels_have_infinite_psnr() {
        assert!(psnr_bytes(&[20, 40, 60, 255], &[20, 40, 60, 255]).is_infinite());
    }

    #[test]
    fn distortion_decreases_psnr_and_invalid_lengths_fail() {
        let reference = [10, 20, 30, 255];
        let small = [11, 20, 30, 255];
        let large = [100, 20, 30, 255];
        assert!(psnr_bytes(&reference, &small) > psnr_bytes(&reference, &large));
        assert!(psnr_bytes(&[], &[]).is_nan());
        assert!(psnr_bytes(&reference, &[10, 20]).is_nan());
    }

    #[test]
    fn supported_mime_is_extension_specific() {
        assert_eq!(mime_for(Path::new("a.JPG")), Some("image/jpeg"));
        assert_eq!(mime_for(Path::new("a.png")), Some("image/png"));
        assert_eq!(mime_for(Path::new("a.webp")), Some("image/webp"));
        assert_eq!(mime_for(Path::new("a.txt")), None);
    }
}

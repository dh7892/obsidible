use anyhow::{Context, Result, bail};
use std::path::{Path, PathBuf};
use tokio::process::Command;
use tracing::debug;

/// Convert a PDF file to a series of PNG images, one per page.
///
/// Uses `pdftoppm` from the poppler-utils package.
/// Returns a sorted list of generated image paths.
pub async fn convert(pdf_path: &Path, output_dir: &Path, dpi: u32) -> Result<Vec<PathBuf>> {
    if !pdf_path.exists() {
        bail!("PDF file not found: {}", pdf_path.display());
    }

    std::fs::create_dir_all(output_dir)
        .with_context(|| format!("Failed to create output dir: {}", output_dir.display()))?;

    let output_prefix = output_dir.join("page");

    debug!(
        "Converting PDF to images at {} DPI: {} -> {}/page-*.png",
        dpi,
        pdf_path.display(),
        output_dir.display()
    );

    let output = Command::new("pdftoppm")
        .args([
            "-png",
            "-r",
            &dpi.to_string(),
            &pdf_path.to_string_lossy(),
            &output_prefix.to_string_lossy(),
        ])
        .output()
        .await
        .context("Failed to execute pdftoppm. Is poppler-utils installed?")?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        bail!("pdftoppm failed: {}", stderr.trim());
    }

    // Collect generated images, sorted by name (page order)
    let mut images: Vec<PathBuf> = Vec::new();
    for entry in std::fs::read_dir(output_dir)? {
        let entry = entry?;
        let path = entry.path();
        if path.extension().is_some_and(|e| e == "png")
            && path
                .file_name()
                .is_some_and(|n| n.to_string_lossy().starts_with("page-"))
        {
            images.push(path);
        }
    }

    images.sort();

    if images.is_empty() {
        bail!("pdftoppm produced no images from {}", pdf_path.display());
    }

    debug!("Generated {} page images", images.len());
    Ok(images)
}

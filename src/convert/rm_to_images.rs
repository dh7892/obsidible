use anyhow::{Context, Result, bail};
use std::path::{Path, PathBuf};
use tokio::process::Command;
use tracing::debug;

/// Convert a single .rm file to SVG using the `rmc` tool.
///
/// Returns the path to the generated SVG file.
pub async fn rm_to_svg(rm_path: &Path, output_dir: &Path) -> Result<PathBuf> {
    let stem = rm_path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("page");

    let svg_path = output_dir.join(format!("{stem}.svg"));

    debug!(
        "Converting .rm to SVG: {} -> {}",
        rm_path.display(),
        svg_path.display()
    );

    let output = Command::new("rmc")
        .args([
            "-t",
            "svg",
            "-o",
            &svg_path.to_string_lossy(),
            &rm_path.to_string_lossy(),
        ])
        .output()
        .await
        .context("Failed to execute rmc. Is it installed? (pip install rmc)")?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        bail!("rmc failed for {}: {}", rm_path.display(), stderr.trim());
    }

    if !svg_path.exists() {
        bail!("rmc succeeded but SVG not found at {}", svg_path.display());
    }

    Ok(svg_path)
}

/// Convert an SVG file to PNG using `rsvg-convert`.
///
/// Returns the path to the generated PNG file.
pub async fn svg_to_png(svg_path: &Path, output_dir: &Path, dpi: u32) -> Result<PathBuf> {
    let stem = svg_path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("page");

    let png_path = output_dir.join(format!("{stem}.png"));

    debug!(
        "Converting SVG to PNG: {} -> {}",
        svg_path.display(),
        png_path.display()
    );

    let output = Command::new("rsvg-convert")
        .args([
            "-d",
            &dpi.to_string(),
            "-p",
            &dpi.to_string(),
            "-o",
            &png_path.to_string_lossy(),
            &svg_path.to_string_lossy(),
        ])
        .output()
        .await
        .context("Failed to execute rsvg-convert. Is librsvg installed?")?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        bail!(
            "rsvg-convert failed for {}: {}",
            svg_path.display(),
            stderr.trim()
        );
    }

    if !png_path.exists() {
        bail!(
            "rsvg-convert succeeded but PNG not found at {}",
            png_path.display()
        );
    }

    Ok(png_path)
}

/// Convert a list of .rm files to PNG images.
///
/// Pipeline: .rm -> SVG (via rmc) -> PNG (via rsvg-convert)
///
/// Returns a list of PNG paths in the same order as the input .rm files.
pub async fn convert_rm_files(
    rm_files: &[PathBuf],
    output_dir: &Path,
    dpi: u32,
) -> Result<Vec<PathBuf>> {
    std::fs::create_dir_all(output_dir)
        .with_context(|| format!("Failed to create output dir: {}", output_dir.display()))?;

    let svg_dir = output_dir.join("svg");
    std::fs::create_dir_all(&svg_dir)?;

    let mut png_paths = Vec::new();

    for (i, rm_path) in rm_files.iter().enumerate() {
        debug!("Processing page {}/{}", i + 1, rm_files.len());

        // .rm -> SVG
        let svg_path = rm_to_svg(rm_path, &svg_dir).await?;

        // SVG -> PNG
        let png_path = svg_to_png(&svg_path, output_dir, dpi).await?;

        png_paths.push(png_path);
    }

    if png_paths.is_empty() {
        bail!("No pages were converted");
    }

    debug!("Converted {} pages to PNG", png_paths.len());
    Ok(png_paths)
}

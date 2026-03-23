use anyhow::{Context, Result, bail};
use serde::Serialize;
use std::path::{Path, PathBuf};
use tempfile::TempDir;
use tracing::{debug, info};

use crate::convert::{composite, pdf_info, pdf_to_images, rm_to_images, rmdoc};
use crate::remarkable;

/// JSON output from the pull command
#[derive(Debug, Serialize)]
pub struct PullOutput {
    pub document_name: String,
    pub file_type: String,
    pub pages: Vec<String>,
    pub has_annotations: bool,
}

/// Run the pull pipeline for a single document.
///
/// Pipeline:
///   1. Download .rmdoc via rmapi get
///   2. Extract zip to get .rm stroke files and embedded PDF (if any)
///   3. Branch by file type:
///      A. Notebook: .rm -> SVG -> PNG
///      B. PDF with annotations: render PDF pages + composite .rm annotations
///      C. PDF without annotations: render PDF pages directly
///      D. EPUB: extract text (future)
///   4. Print JSON output with page image paths
pub async fn run(rm_path: &str, output_dir: &str, dpi: u32) -> Result<()> {
    let output_dir = PathBuf::from(output_dir);
    std::fs::create_dir_all(&output_dir)
        .with_context(|| format!("Failed to create output dir: {}", output_dir.display()))?;

    // Extract the document name from the rm_path
    let doc_name = rm_path.rsplit('/').next().unwrap_or(rm_path).to_string();

    info!("Pulling document: {}", doc_name);

    // Step 1: Download the .rmdoc archive
    let temp_dir = TempDir::new().context("Failed to create temp directory")?;
    let rmdoc_path = remarkable::get(rm_path, temp_dir.path())
        .await
        .with_context(|| format!("Failed to download '{}' from reMarkable", rm_path))?;

    info!("Downloaded: {}", rmdoc_path.display());

    // Step 2: Extract the .rmdoc archive
    let extract_dir = temp_dir.path().join("extracted");
    let extracted = rmdoc::extract(&rmdoc_path, &extract_dir)
        .with_context(|| format!("Failed to extract rmdoc: {}", rmdoc_path.display()))?;

    info!(
        "Extracted: {} pages, type: {}",
        extracted.metadata.pages.len(),
        extracted.metadata.file_type
    );

    // Step 3: Branch by file type
    let (page_images, has_annotations) = match extracted.metadata.file_type.as_str() {
        "pdf" => process_pdf(&extracted, &output_dir, dpi).await?,
        "epub" => {
            bail!("EPUB support is not yet implemented. Use 'notebook' or 'pdf' documents.");
        }
        // "notebook" or empty string (default)
        _ => process_notebook(&extracted, &output_dir, dpi).await?,
    };

    // Step 4: Output JSON
    let output = PullOutput {
        document_name: doc_name,
        file_type: extracted.metadata.file_type.clone(),
        pages: page_images
            .iter()
            .map(|p| p.to_string_lossy().to_string())
            .collect(),
        has_annotations,
    };

    println!("{}", serde_json::to_string_pretty(&output)?);

    Ok(())
}

/// Process a pure handwriting notebook: .rm -> SVG -> PNG for each page.
async fn process_notebook(
    extracted: &rmdoc::ExtractedRmdoc,
    output_dir: &Path,
    dpi: u32,
) -> Result<(Vec<PathBuf>, bool)> {
    if extracted.rm_files.is_empty() {
        bail!("Notebook has no pages with stroke data");
    }

    info!(
        "Processing notebook with {} annotated pages",
        extracted.rm_files.len()
    );

    let images = rm_to_images::convert_rm_files(&extracted.rm_files, output_dir, dpi).await?;

    // Rename to sequential page numbers for consistent output
    let mut final_images = Vec::new();
    for (i, img) in images.iter().enumerate() {
        let dest = output_dir.join(format!("page-{:03}.png", i + 1));
        if img != &dest {
            std::fs::rename(img, &dest).with_context(|| {
                format!("Failed to rename {} to {}", img.display(), dest.display())
            })?;
        }
        final_images.push(dest);
    }

    Ok((final_images, true))
}

/// Process a PDF document: render base pages + composite annotations where present.
async fn process_pdf(
    extracted: &rmdoc::ExtractedRmdoc,
    output_dir: &Path,
    dpi: u32,
) -> Result<(Vec<PathBuf>, bool)> {
    let pdf_path = extracted
        .pdf_path
        .as_ref()
        .context("PDF document type but no embedded PDF found in rmdoc archive")?;

    let has_annotations = !extracted.rm_files_by_page.is_empty();

    info!(
        "Processing PDF with {} pages, {} have annotations",
        extracted.metadata.pages.len(),
        extracted.rm_files_by_page.len()
    );

    // Render all PDF pages to images
    let base_dir = output_dir.join("base");
    let base_images = pdf_to_images::convert(pdf_path, &base_dir, dpi).await?;

    debug!("Rendered {} base PDF page images", base_images.len());

    if !has_annotations {
        // No annotations -- just copy base images to output with consistent naming
        let mut final_images = Vec::new();
        for (i, img) in base_images.iter().enumerate() {
            let dest = output_dir.join(format!("page-{:03}.png", i + 1));
            std::fs::copy(img, &dest).with_context(|| {
                format!("Failed to copy {} to {}", img.display(), dest.display())
            })?;
            final_images.push(dest);
        }
        // Clean up base dir
        let _ = std::fs::remove_dir_all(&base_dir);
        return Ok((final_images, false));
    }

    // Get PDF page dimensions for coordinate mapping
    let page_size = pdf_info::get_page_size(pdf_path).await?;
    debug!(
        "PDF page size: {:.1} x {:.1} pts",
        page_size.width_pt, page_size.height_pt
    );

    // Process each page: composite annotations where they exist
    let composite_dir = output_dir.join("composite");
    std::fs::create_dir_all(&composite_dir)?;

    let mut final_images = Vec::new();

    for (i, page_uuid) in extracted.metadata.pages.iter().enumerate() {
        let page_num = i + 1;
        let output_path = output_dir.join(format!("page-{:03}.png", page_num));

        // Check if this page has annotations
        if let Some(rm_path) = extracted.rm_files_by_page.get(page_uuid) {
            // Find the corresponding base image
            if i < base_images.len() {
                info!("Compositing annotations onto page {}", page_num);
                composite::composite_annotations(
                    rm_path,
                    &base_images[i],
                    &output_path,
                    &page_size,
                    dpi,
                )
                .await?;
            } else {
                debug!(
                    "Page {} has annotations but no base PDF page (page count mismatch)",
                    page_num
                );
                // Render annotations standalone as fallback
                let svg_dir = composite_dir.join("svg");
                std::fs::create_dir_all(&svg_dir)?;
                let svg_path = rm_to_images::rm_to_svg(rm_path, &svg_dir).await?;
                let png_path = rm_to_images::svg_to_png(&svg_path, &composite_dir, dpi).await?;
                if png_path != output_path {
                    std::fs::rename(&png_path, &output_path)?;
                }
            }
        } else {
            // No annotations on this page -- use the base image directly
            if i < base_images.len() {
                std::fs::copy(&base_images[i], &output_path).with_context(|| {
                    format!(
                        "Failed to copy base page {} to {}",
                        base_images[i].display(),
                        output_path.display()
                    )
                })?;
            } else {
                debug!("Page {} has no base image and no annotations", page_num);
                continue;
            }
        }

        final_images.push(output_path);
    }

    // Clean up temporary directories
    let _ = std::fs::remove_dir_all(&base_dir);
    let _ = std::fs::remove_dir_all(&composite_dir);

    Ok((final_images, has_annotations))
}

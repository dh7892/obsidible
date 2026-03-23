use anyhow::{Context, Result, bail};
use serde::Serialize;
use std::path::{Path, PathBuf};
use tracing::{debug, info};

use crate::PushFormat;
use crate::convert::md_to_pdf;
use crate::remarkable;

/// JSON output from the push command
#[derive(Debug, Serialize)]
pub struct PushOutput {
    pub uploaded: String,
    pub source: String,
    pub pages: u32,
}

/// Run the push pipeline for a single file.
///
/// Pipeline:
///   1. Read the input file (.md or .pdf)
///   2. Convert markdown to PDF if needed (with format-specific Typst settings)
///   3. Upload to reMarkable (delete existing file with same name first)
///   4. Print JSON output
pub async fn run(local_path: &str, rm_destination: &str, format: &PushFormat) -> Result<()> {
    let local_path = PathBuf::from(local_path);

    if !local_path.exists() {
        bail!("File not found: {}", local_path.display());
    }

    let extension = local_path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_lowercase();

    let file_name = local_path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("document");

    info!("Pushing: {} -> {}", local_path.display(), rm_destination);

    // Step 1: Determine the PDF to upload
    let (pdf_path, is_temp) = match extension.as_str() {
        "pdf" => {
            // Direct PDF upload -- no conversion needed
            info!("Uploading PDF directly");
            (local_path.clone(), false)
        }
        "md" => {
            // Convert markdown to PDF
            info!("Converting markdown to PDF with format: {:?}", format);
            let temp_dir = tempfile::tempdir().context("Failed to create temp directory")?;
            let pdf = md_to_pdf::convert(&local_path, temp_dir.path(), format).await?;
            // Move out of temp dir so it persists
            let final_pdf = local_path.with_extension("pdf");
            std::fs::copy(&pdf, &final_pdf)
                .with_context(|| format!("Failed to copy PDF to {}", final_pdf.display()))?;
            (final_pdf, true)
        }
        other => {
            bail!(
                "Unsupported file type '.{}'. Supported: .md (markdown), .pdf",
                other
            );
        }
    };

    // Count pages in the PDF
    let page_count = count_pdf_pages(&pdf_path).await.unwrap_or(1);

    // Step 2: Ensure the destination directory exists on reMarkable
    remarkable::ensure_dir(rm_destination).await?;

    // Step 3: Delete existing file with the same name if it exists
    let rm_file_path = format!("{}/{}", rm_destination.trim_end_matches('/'), file_name);
    match delete_if_exists(&rm_file_path).await {
        Ok(true) => info!("Deleted existing file: {}", rm_file_path),
        Ok(false) => debug!("No existing file to delete: {}", rm_file_path),
        Err(e) => debug!("Could not check/delete existing file: {}", e),
    }

    // Step 4: Upload to reMarkable
    remarkable::put(&pdf_path, rm_destination).await?;

    // Clean up temporary PDF if we created one
    if is_temp {
        let _ = std::fs::remove_file(&pdf_path);
    }

    // Step 5: Output JSON
    let output = PushOutput {
        uploaded: rm_file_path,
        source: local_path.to_string_lossy().to_string(),
        pages: page_count,
    };

    println!("{}", serde_json::to_string_pretty(&output)?);

    Ok(())
}

/// Count the number of pages in a PDF using pdfinfo.
async fn count_pdf_pages(pdf_path: &Path) -> Result<u32> {
    let output = tokio::process::Command::new("pdfinfo")
        .arg(pdf_path.to_string_lossy().as_ref())
        .output()
        .await
        .context("Failed to execute pdfinfo")?;

    if !output.status.success() {
        bail!("pdfinfo failed");
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    for line in stdout.lines() {
        let line = line.trim();
        if line.starts_with("Pages:") {
            let count_str = line.strip_prefix("Pages:").unwrap().trim();
            return count_str
                .parse()
                .context("Failed to parse page count from pdfinfo");
        }
    }

    bail!("No 'Pages:' line found in pdfinfo output");
}

/// Try to delete a file on the reMarkable if it exists.
/// Returns Ok(true) if deleted, Ok(false) if it didn't exist.
async fn delete_if_exists(rm_path: &str) -> Result<bool> {
    // We can't easily check if a specific file exists without listing the parent,
    // so we just try to delete and handle the error gracefully.
    match remarkable::rm(rm_path).await {
        Ok(()) => Ok(true),
        Err(_) => Ok(false),
    }
}

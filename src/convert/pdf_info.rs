use anyhow::{Context, Result, bail};
use std::path::Path;
use tokio::process::Command;
use tracing::debug;

/// PDF page dimensions in points (72 DPI)
#[derive(Debug, Clone)]
pub struct PdfPageSize {
    pub width_pt: f64,
    pub height_pt: f64,
}

/// Get the page size of a PDF in points using `pdfinfo`.
///
/// Returns the page dimensions from the first page.
pub async fn get_page_size(pdf_path: &Path) -> Result<PdfPageSize> {
    debug!("Getting PDF page size: {}", pdf_path.display());

    let output = Command::new("pdfinfo")
        .arg(pdf_path.to_string_lossy().as_ref())
        .output()
        .await
        .context("Failed to execute pdfinfo. Is poppler-utils installed?")?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        bail!(
            "pdfinfo failed for {}: {}",
            pdf_path.display(),
            stderr.trim()
        );
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    parse_page_size(&stdout).with_context(|| {
        format!(
            "Failed to parse page size from pdfinfo output for {}",
            pdf_path.display()
        )
    })
}

/// Parse the "Page size:" line from pdfinfo output.
///
/// Expected format: "Page size:      595.28 x 841.89 pts (A4)"
fn parse_page_size(pdfinfo_output: &str) -> Result<PdfPageSize> {
    for line in pdfinfo_output.lines() {
        let line = line.trim();
        if line.starts_with("Page size:") {
            let rest = line.strip_prefix("Page size:").unwrap().trim();
            // Parse "595.28 x 841.89 pts (A4)" or "612 x 792 pts (letter)"
            let parts: Vec<&str> = rest.split_whitespace().collect();
            if parts.len() >= 3 && parts[1] == "x" {
                let width: f64 = parts[0].parse().context("Failed to parse page width")?;
                let height: f64 = parts[2].parse().context("Failed to parse page height")?;
                return Ok(PdfPageSize {
                    width_pt: width,
                    height_pt: height,
                });
            }
        }
    }
    bail!("No 'Page size:' line found in pdfinfo output");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_a4() {
        let output = "Page size:      595.28 x 841.89 pts (A4)\n";
        let size = parse_page_size(output).unwrap();
        assert!((size.width_pt - 595.28).abs() < 0.01);
        assert!((size.height_pt - 841.89).abs() < 0.01);
    }

    #[test]
    fn test_parse_letter() {
        let output =
            "Title:          doc\nPage size:      612 x 792 pts (letter)\nPages:          3\n";
        let size = parse_page_size(output).unwrap();
        assert!((size.width_pt - 612.0).abs() < 0.01);
        assert!((size.height_pt - 792.0).abs() < 0.01);
    }
}

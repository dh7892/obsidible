use anyhow::{Context, Result, bail};
use serde::Deserialize;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use tracing::debug;

/// Metadata from the .content file inside an rmdoc archive
#[derive(Debug, Deserialize)]
pub struct ContentMetadata {
    /// List of page UUIDs in order
    #[serde(default)]
    pub pages: Vec<String>,
    /// Number of pages
    #[serde(rename = "pageCount", default)]
    pub page_count: i64,
    /// File type: "notebook", "pdf", "epub"
    #[serde(rename = "fileType", default)]
    pub file_type: String,
}

/// Result of extracting an rmdoc archive
pub struct ExtractedRmdoc {
    /// The document UUID
    pub doc_id: String,
    /// Content metadata
    pub metadata: ContentMetadata,
    /// Paths to .rm files in page order (only for pages that have .rm files)
    pub rm_files: Vec<PathBuf>,
    /// Map of page UUID -> .rm file path (for pages that have annotations)
    pub rm_files_by_page: HashMap<String, PathBuf>,
    /// Path to the embedded PDF, if this is a PDF-type document
    pub pdf_path: Option<PathBuf>,
    /// Root extraction directory
    pub extract_dir: PathBuf,
}

/// Extract an .rmdoc file (zip archive) and return structured info about its contents.
///
/// The rmdoc format contains:
/// - `<uuid>.content` -- JSON metadata with page list
/// - `<uuid>.metadata` -- Document metadata
/// - `<uuid>/<page-uuid>.rm` -- Raw stroke data per page
/// - `<uuid>.pdf` -- Embedded PDF (for PDF-type documents)
pub fn extract(rmdoc_path: &Path, output_dir: &Path) -> Result<ExtractedRmdoc> {
    if !rmdoc_path.exists() {
        bail!("rmdoc file not found: {}", rmdoc_path.display());
    }

    std::fs::create_dir_all(output_dir)
        .with_context(|| format!("Failed to create output dir: {}", output_dir.display()))?;

    debug!("Extracting rmdoc: {}", rmdoc_path.display());

    // Unzip the archive
    let file = std::fs::File::open(rmdoc_path)
        .with_context(|| format!("Failed to open rmdoc: {}", rmdoc_path.display()))?;
    let mut archive = zip::ZipArchive::new(file)
        .with_context(|| format!("Failed to read rmdoc as zip: {}", rmdoc_path.display()))?;

    archive
        .extract(output_dir)
        .with_context(|| format!("Failed to extract rmdoc to: {}", output_dir.display()))?;

    // Find the .content file to get the document UUID and page list
    let content_file = find_content_file(output_dir)?;
    let doc_id = content_file
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("unknown")
        .to_string();

    debug!("Document ID: {}", doc_id);

    // Parse the content metadata
    let content_json = std::fs::read_to_string(&content_file)
        .with_context(|| format!("Failed to read content file: {}", content_file.display()))?;
    let metadata: ContentMetadata = serde_json::from_str(&content_json)
        .with_context(|| format!("Failed to parse content file: {}", content_file.display()))?;

    debug!(
        "Document has {} pages, type: {}",
        metadata.pages.len(),
        metadata.file_type
    );

    // Build the list of .rm file paths in page order
    let rm_dir = output_dir.join(&doc_id);
    let mut rm_files = Vec::new();
    let mut rm_files_by_page = HashMap::new();

    if metadata.pages.is_empty() {
        // Some notebooks have an empty pages array in .content.
        // Fall back to scanning the UUID subdirectory for .rm files directly.
        debug!("Pages array is empty, scanning directory for .rm files");
        if rm_dir.exists() {
            let mut found: Vec<PathBuf> = std::fs::read_dir(&rm_dir)?
                .filter_map(|e| e.ok())
                .map(|e| e.path())
                .filter(|p| p.extension().is_some_and(|e| e == "rm"))
                .collect();
            found.sort(); // Sort by filename for consistent ordering
            for rm_path in found {
                let page_uuid = rm_path
                    .file_stem()
                    .and_then(|s| s.to_str())
                    .unwrap_or("unknown")
                    .to_string();
                rm_files_by_page.insert(page_uuid, rm_path.clone());
                rm_files.push(rm_path);
            }
            debug!("Found {} .rm files by directory scan", rm_files.len());
        }
    } else {
        for page_uuid in &metadata.pages {
            let rm_path = rm_dir.join(format!("{page_uuid}.rm"));
            if rm_path.exists() {
                rm_files.push(rm_path.clone());
                rm_files_by_page.insert(page_uuid.clone(), rm_path);
            } else {
                debug!("No .rm file for page {page_uuid} (no annotations on this page)");
            }
        }
    }

    // Look for an embedded PDF
    let pdf_path = find_embedded_pdf(output_dir, &doc_id);
    if let Some(ref p) = pdf_path {
        debug!("Found embedded PDF: {}", p.display());
    }

    Ok(ExtractedRmdoc {
        doc_id,
        metadata,
        rm_files,
        rm_files_by_page,
        pdf_path,
        extract_dir: output_dir.to_path_buf(),
    })
}

/// Find the .content file in the extracted directory
fn find_content_file(dir: &Path) -> Result<PathBuf> {
    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        let path = entry.path();
        if path.extension().is_some_and(|e| e == "content") && path.is_file() {
            return Ok(path);
        }
    }
    bail!(
        "No .content file found in extracted rmdoc at {}",
        dir.display()
    );
}

/// Find an embedded PDF in the extracted directory.
/// The PDF is stored as `<doc-uuid>.pdf` at the top level.
fn find_embedded_pdf(dir: &Path, doc_id: &str) -> Option<PathBuf> {
    // Try the expected path first
    let expected = dir.join(format!("{doc_id}.pdf"));
    if expected.exists() {
        return Some(expected);
    }

    // Fall back to searching for any PDF at the top level
    if let Ok(entries) = std::fs::read_dir(dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().is_some_and(|e| e == "pdf") && path.is_file() {
                return Some(path);
            }
        }
    }

    None
}

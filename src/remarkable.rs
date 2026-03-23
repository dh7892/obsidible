use anyhow::{Context, Result, bail};
use serde::Deserialize;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use tokio::process::Command;
use tracing::{debug, info};

/// A file or directory entry from rmapi
#[derive(Debug, Clone, Deserialize)]
pub struct RmEntry {
    /// Unique ID (UUID)
    pub id: String,
    /// Display name
    pub name: String,
    /// "CollectionType" for dirs, "DocumentType" for files
    #[serde(rename = "type")]
    pub entry_type: String,
    /// Version number
    pub version: i64,
    /// Last modified timestamp (RFC3339)
    #[serde(rename = "modifiedClient")]
    pub modified_client: String,
    /// Current page index
    #[serde(rename = "currentPage", default)]
    pub current_page: i64,
    /// Parent UUID
    #[serde(default)]
    pub parent: String,
}

impl RmEntry {
    pub fn is_directory(&self) -> bool {
        self.entry_type == "CollectionType"
    }

    pub fn is_document(&self) -> bool {
        self.entry_type == "DocumentType"
    }
}

/// Run an rmapi command and return stdout
async fn run_rmapi(args: &[&str]) -> Result<String> {
    debug!("Running: rmapi {}", args.join(" "));

    let output = Command::new("rmapi")
        .args(args)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .await
        .context("Failed to execute rmapi. Is it installed?")?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let stdout = String::from_utf8_lossy(&output.stdout);
        bail!(
            "rmapi {} failed (exit {}):\nstdout: {}\nstderr: {}",
            args.join(" "),
            output.status,
            stdout.trim(),
            stderr.trim()
        );
    }

    Ok(String::from_utf8_lossy(&output.stdout).to_string())
}

/// Run rmapi interactive auth setup
pub async fn auth() -> Result<()> {
    info!("Starting reMarkable cloud authentication...");
    println!("This will open the rmapi authentication flow.");
    println!("You'll need a one-time code from https://my.remarkable.com/connect/desktop");
    println!();

    // Run rmapi interactively for auth
    let status = Command::new("rmapi")
        .arg("version")
        .status()
        .await
        .context("Failed to execute rmapi. Is it installed?")?;

    if status.success() {
        // rmapi is working, check if we can list files (proves auth)
        match run_rmapi(&["-ni", "ls", "/"]).await {
            Ok(_) => {
                println!("Already authenticated with reMarkable cloud.");
                return Ok(());
            }
            Err(_) => {
                println!("Authentication required. Running rmapi...");
            }
        }
    }

    // Run rmapi interactively so user can enter their code
    let status = Command::new("rmapi")
        .arg("ls")
        .arg("/")
        .stdin(Stdio::inherit())
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit())
        .status()
        .await
        .context("Failed to run rmapi for authentication")?;

    if status.success() {
        println!("\nAuthentication successful!");
    } else {
        bail!("Authentication failed. Please try again.");
    }

    Ok(())
}

/// Check if rmapi is authenticated
pub async fn is_authenticated() -> bool {
    run_rmapi(&["-ni", "ls", "/"]).await.is_ok()
}

/// List entries in a reMarkable directory
pub async fn ls(rm_path: &str) -> Result<Vec<RmEntry>> {
    let output = run_rmapi(&["-ni", "-json", "ls", rm_path]).await?;

    // rmapi -json ls returns a JSON array
    let entries: Vec<RmEntry> = serde_json::from_str(output.trim())
        .with_context(|| format!("Failed to parse rmapi ls output for {rm_path}"))?;

    Ok(entries)
}

/// Recursively list all documents under a path
pub async fn ls_recursive(rm_path: &str) -> Result<Vec<(String, RmEntry)>> {
    let mut results = Vec::new();
    let mut stack = vec![rm_path.to_string()];

    while let Some(current_path) = stack.pop() {
        let entries = ls(&current_path).await?;
        for entry in entries {
            let full_path = if current_path == "/" {
                format!("/{}", entry.name)
            } else {
                format!("{}/{}", current_path, entry.name)
            };

            if entry.is_directory() {
                stack.push(full_path);
            } else {
                results.push((full_path, entry));
            }
        }
    }

    Ok(results)
}

/// Download a document as .rmdoc (zip archive)
pub async fn get(rm_path: &str, output_dir: &Path) -> Result<PathBuf> {
    let output_dir_str = output_dir.to_string_lossy();
    debug!("Downloading {} to {}", rm_path, output_dir_str);

    // rmapi get downloads to current directory, so we set the working dir
    std::fs::create_dir_all(output_dir)
        .with_context(|| format!("Failed to create output dir: {}", output_dir.display()))?;

    let output = Command::new("rmapi")
        .args(["-ni", "get", rm_path])
        .current_dir(output_dir)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .await
        .context("Failed to execute rmapi get")?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        bail!("rmapi get failed for {}: {}", rm_path, stderr.trim());
    }

    // The file is saved as <name>.rmdoc in the output directory
    let name = rm_path.rsplit('/').next().unwrap_or(rm_path);
    let rmdoc_path = output_dir.join(format!("{name}.rmdoc"));

    if !rmdoc_path.exists() {
        // Sometimes rmapi uses slightly different naming, search for any .rmdoc
        for entry in std::fs::read_dir(output_dir)? {
            let entry = entry?;
            if entry.path().extension().is_some_and(|e| e == "rmdoc") {
                return Ok(entry.path());
            }
        }
        bail!(
            "rmapi get succeeded but no .rmdoc file found in {}",
            output_dir.display()
        );
    }

    Ok(rmdoc_path)
}

/// Download a document with annotations rendered as PDF
/// Returns path to the annotations PDF
pub async fn geta(rm_path: &str, output_dir: &Path, all_pages: bool) -> Result<PathBuf> {
    let output_dir_str = output_dir.to_string_lossy();
    debug!(
        "Downloading with annotations: {} to {}",
        rm_path, output_dir_str
    );

    std::fs::create_dir_all(output_dir)
        .with_context(|| format!("Failed to create output dir: {}", output_dir.display()))?;

    let mut args = vec!["-ni", "geta"];
    if all_pages {
        args.push("-a");
    }
    args.push(rm_path);

    let output = Command::new("rmapi")
        .args(&args)
        .current_dir(output_dir)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .await
        .context("Failed to execute rmapi geta")?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        bail!("rmapi geta failed for {}: {}", rm_path, stderr.trim());
    }

    // geta creates <name>.zip and <name>-annotations.pdf
    let name = rm_path.rsplit('/').next().unwrap_or(rm_path);

    // Look for the annotations PDF
    let annotations_pdf = output_dir.join(format!("{name}-annotations.pdf"));
    if annotations_pdf.exists() {
        return Ok(annotations_pdf);
    }

    // Fall back to searching for any PDF
    for entry in std::fs::read_dir(output_dir)? {
        let entry = entry?;
        let path = entry.path();
        if path.extension().is_some_and(|e| e == "pdf") {
            return Ok(path);
        }
    }

    bail!(
        "rmapi geta succeeded but no annotation PDF found in {}",
        output_dir.display()
    );
}

/// Upload a file (PDF/EPUB) to reMarkable
pub async fn put(local_path: &Path, rm_folder: &str) -> Result<()> {
    let local_str = local_path.to_string_lossy();
    debug!("Uploading {} to {}", local_str, rm_folder);

    let output = Command::new("rmapi")
        .args(["-ni", "put", &local_str, rm_folder])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .await
        .context("Failed to execute rmapi put")?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        bail!(
            "rmapi put failed for {} -> {}: {}",
            local_str,
            rm_folder,
            stderr.trim()
        );
    }

    info!("Uploaded {} to {}", local_path.display(), rm_folder);
    Ok(())
}

/// Create a directory on reMarkable
pub async fn mkdir(rm_path: &str) -> Result<()> {
    debug!("Creating directory: {}", rm_path);
    run_rmapi(&["-ni", "mkdir", rm_path]).await?;
    Ok(())
}

/// Delete a file or empty directory on reMarkable
pub async fn rm(rm_path: &str) -> Result<()> {
    debug!("Deleting: {}", rm_path);
    run_rmapi(&["-ni", "rm", rm_path]).await?;
    info!("Deleted {} from reMarkable", rm_path);
    Ok(())
}

/// Move/rename a file on reMarkable
pub async fn mv(src: &str, dst: &str) -> Result<()> {
    debug!("Moving: {} -> {}", src, dst);
    run_rmapi(&["-ni", "mv", src, dst]).await?;
    info!("Moved {} to {}", src, dst);
    Ok(())
}

/// Ensure a directory exists on reMarkable, creating it if necessary
pub async fn ensure_dir(rm_path: &str) -> Result<()> {
    match ls(rm_path).await {
        Ok(_) => Ok(()),
        Err(_) => {
            // Directory doesn't exist, create it
            // Need to ensure parent exists first
            if let Some(parent) = rm_path.rsplit_once('/') {
                if !parent.0.is_empty() {
                    // Box the recursive future to avoid infinite type size
                    Box::pin(ensure_dir(parent.0)).await?;
                }
            }
            mkdir(rm_path).await
        }
    }
}

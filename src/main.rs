// Allow dead code during development -- public API surface is larger than current usage
#![allow(dead_code)]

mod convert;
mod pull;
mod push;
mod remarkable;

use anyhow::Result;
use clap::{Parser, Subcommand, ValueEnum};

#[derive(Parser)]
#[command(name = "obsidible")]
#[command(about = "Convert and transport documents between Obsidian and reMarkable")]
#[command(version)]
struct Cli {
    /// Enable verbose output
    #[arg(short, long)]
    verbose: bool,

    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Download a document from the reMarkable, convert to PNG images
    Pull {
        /// Path on the reMarkable (e.g. "/Tasks" or "/Quick Notes/Meeting")
        rm_path: String,

        /// Output directory for converted images
        #[arg(long, default_value = "/tmp/rm-work")]
        output_dir: String,

        /// Render DPI for image output
        #[arg(long, default_value_t = 200)]
        dpi: u32,
    },

    /// Convert a local file to PDF and upload to the reMarkable
    Push {
        /// Local file path (.md or .pdf)
        local_path: String,

        /// Destination folder on the reMarkable (e.g. "/Briefings")
        rm_destination: String,

        /// Document format/layout preset
        #[arg(long, default_value = "default")]
        format: PushFormat,
    },

    /// Set up reMarkable cloud authentication (runs rmapi interactive auth)
    Auth,
}

#[derive(Debug, Clone, ValueEnum)]
pub enum PushFormat {
    /// 11pt, A4, 2cm margins, justified (standard)
    Default,
    /// 12pt, no justification, 2.5cm margins for annotation space
    Recipe,
    /// 11pt, scannable layout, clear headings, bullet points
    Briefing,
    /// 12pt, checkbox grid layout with empty rows for handwritten additions
    Tasks,
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();

    // Set up tracing based on verbosity
    let builder = tracing_subscriber::fmt().with_target(false);
    if cli.verbose {
        builder.with_max_level(tracing::Level::DEBUG).init();
    } else {
        builder.with_max_level(tracing::Level::WARN).init();
    }

    match cli.command {
        Commands::Auth => {
            remarkable::auth().await?;
        }
        Commands::Pull {
            rm_path,
            output_dir,
            dpi,
        } => {
            pull::run(&rm_path, &output_dir, dpi).await?;
        }
        Commands::Push {
            local_path,
            rm_destination,
            format,
        } => {
            push::run(&local_path, &rm_destination, &format).await?;
        }
    }

    Ok(())
}

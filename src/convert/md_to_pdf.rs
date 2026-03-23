use anyhow::{Context, Result, bail};
use std::path::{Path, PathBuf};
use tempfile::TempDir;
use tokio::process::Command;
use tracing::debug;

use crate::PushFormat;

/// Convert a markdown file to PDF using typst with format-specific settings.
pub async fn convert(md_path: &Path, output_dir: &Path, format: &PushFormat) -> Result<PathBuf> {
    let stem = md_path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("document");

    let md_content = std::fs::read_to_string(md_path)
        .with_context(|| format!("Failed to read markdown file: {}", md_path.display()))?;

    let pdf_path = output_dir.join(format!("{stem}.pdf"));

    // Create a temporary typst source file
    let temp_dir = TempDir::new().context("Failed to create temp directory")?;
    let typ_path = temp_dir.path().join("document.typ");

    // Generate typst markup from markdown with format-specific settings
    let typst_content = match format {
        PushFormat::Tasks => markdown_to_typst_tasks(&md_content),
        _ => markdown_to_typst(&md_content, format),
    };

    std::fs::write(&typ_path, &typst_content)
        .with_context(|| format!("Failed to write typst file: {}", typ_path.display()))?;

    debug!(
        "Compiling typst ({:?}): {} -> {}",
        format,
        typ_path.display(),
        pdf_path.display()
    );

    // Compile with typst
    let output = Command::new("typst")
        .args([
            "compile",
            &typ_path.to_string_lossy(),
            &pdf_path.to_string_lossy(),
        ])
        .output()
        .await
        .context("Failed to execute typst. Is it installed?")?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        bail!("typst compile failed: {}", stderr.trim());
    }

    if !pdf_path.exists() {
        bail!(
            "typst compile succeeded but PDF not found at {}",
            pdf_path.display()
        );
    }

    Ok(pdf_path)
}

/// Generate the Typst page/text/par setup block for a given format.
fn format_preamble(format: &PushFormat) -> String {
    match format {
        PushFormat::Default => r#"#set page(
  paper: "a4",
  margin: (x: 2cm, y: 2cm),
)
#set text(
  font: "New Computer Modern",
  size: 11pt,
)
#set par(
  justify: true,
  leading: 0.65em,
)
#set heading(numbering: none)

"#
        .to_string(),
        PushFormat::Recipe => r#"#set page(
  paper: "a4",
  margin: (x: 2.5cm, y: 2.5cm),
)
#set text(
  font: "New Computer Modern",
  size: 12pt,
)
#set par(
  justify: false,
  leading: 0.75em,
)
#set heading(numbering: none)

"#
        .to_string(),
        PushFormat::Briefing => r#"#set page(
  paper: "a4",
  margin: (x: 2cm, y: 2cm),
)
#set text(
  font: "New Computer Modern",
  size: 11pt,
)
#set par(
  justify: false,
  leading: 0.7em,
)
#set heading(numbering: none)
#show heading.where(level: 1): set text(size: 16pt)
#show heading.where(level: 2): set text(size: 13pt)

"#
        .to_string(),
        PushFormat::Tasks => {
            // Tasks format has its own preamble in markdown_to_typst_tasks
            String::new()
        }
    }
}

/// Convert markdown content to typst markup with format-specific settings.
fn markdown_to_typst(md: &str, format: &PushFormat) -> String {
    let mut output = format_preamble(format);

    let mut in_code_block = false;
    let mut code_block_content = String::new();

    for line in md.lines() {
        // Handle fenced code blocks
        if line.starts_with("```") {
            if in_code_block {
                output.push_str("```\n\n");
                in_code_block = false;
                code_block_content.clear();
                continue;
            } else {
                in_code_block = true;
                let lang = line.trim_start_matches('`').trim();
                if lang.is_empty() {
                    output.push_str("```\n");
                } else {
                    output.push_str(&format!("```{lang}\n"));
                }
                continue;
            }
        }

        if in_code_block {
            output.push_str(line);
            output.push('\n');
            continue;
        }

        // Handle headings
        if line.starts_with("# ") {
            let text = escape_typst(&line[2..]);
            output.push_str(&format!("= {text}\n\n"));
            continue;
        }
        if line.starts_with("## ") {
            let text = escape_typst(&line[3..]);
            output.push_str(&format!("== {text}\n\n"));
            continue;
        }
        if line.starts_with("### ") {
            let text = escape_typst(&line[4..]);
            output.push_str(&format!("=== {text}\n\n"));
            continue;
        }
        if line.starts_with("#### ") {
            let text = escape_typst(&line[5..]);
            output.push_str(&format!("==== {text}\n\n"));
            continue;
        }

        // Handle horizontal rules
        if line.trim() == "---" || line.trim() == "***" || line.trim() == "___" {
            output.push_str("#line(length: 100%)\n\n");
            continue;
        }

        // Handle unordered list items
        if line.starts_with("- ") || line.starts_with("* ") {
            let text = convert_inline_formatting(&line[2..]);
            output.push_str(&format!("- {text}\n"));
            continue;
        }

        // Handle ordered list items
        if let Some(rest) = strip_ordered_list_prefix(line) {
            let text = convert_inline_formatting(rest);
            output.push_str(&format!("+ {text}\n"));
            continue;
        }

        // Handle blockquotes
        if line.starts_with("> ") {
            let text = convert_inline_formatting(&line[2..]);
            output.push_str(&format!("#quote[{text}]\n\n"));
            continue;
        }

        // Handle empty lines
        if line.trim().is_empty() {
            output.push('\n');
            continue;
        }

        // Regular paragraph text
        let text = convert_inline_formatting(line);
        output.push_str(&text);
        output.push('\n');
    }

    output
}

/// Convert task-list markdown to Typst with checkbox grid rendering.
///
/// Expected input format:
/// ```
/// # Section Name
/// - [ ] Task description
/// - [x] Completed task description
///   - [ ] Sub-task
/// ```
///
/// Renders as:
/// - Section headings
/// - Checkbox grid with empty box (unchecked) or X'd box (checked)
/// - Strikethrough on completed tasks
/// - Empty rows at the bottom under "New Tasks" heading
fn markdown_to_typst_tasks(md: &str) -> String {
    let mut output = String::new();

    // Task format preamble
    output.push_str(
        r#"#set page(
  paper: "a4",
  margin: (x: 2cm, y: 2cm),
)
#set text(
  font: "New Computer Modern",
  size: 12pt,
)
#set par(
  justify: false,
  leading: 0.7em,
)
#set heading(numbering: none)

// Checkbox helper functions
#let checkbox-empty = box(
  width: 0.9em,
  height: 0.9em,
  stroke: 0.8pt + black,
  radius: 1pt,
  inset: 0pt,
)

#let checkbox-checked = box(
  width: 0.9em,
  height: 0.9em,
  stroke: 0.8pt + black,
  radius: 1pt,
  inset: 0pt,
  align(center + horizon, text(size: 0.7em, weight: "bold")[X])
)

#let task(checked, body) = {
  grid(
    columns: (1.5em, 1fr),
    column-gutter: 0.5em,
    row-gutter: 0.4em,
    align(center + top, if checked { checkbox-checked } else { checkbox-empty }),
    if checked { strike(body) } else { body },
  )
}

#let subtask(checked, body) = {
  grid(
    columns: (1.5em, 1.5em, 1fr),
    column-gutter: 0.5em,
    row-gutter: 0.4em,
    [],
    align(center + top, if checked { checkbox-checked } else { checkbox-empty }),
    if checked { strike(body) } else { body },
  )
}

#let empty-row = {
  grid(
    columns: (1.5em, 1fr),
    column-gutter: 0.5em,
    row-gutter: 0.4em,
    align(center + top, checkbox-empty),
    line(length: 100%, stroke: 0.3pt + luma(200)),
  )
}

"#,
    );

    for line in md.lines() {
        let trimmed = line.trim();

        // Handle headings
        if trimmed.starts_with("# ") {
            let text = escape_typst(&trimmed[2..]);
            output.push_str(&format!("= {text}\n\n"));
            continue;
        }
        if trimmed.starts_with("## ") {
            let text = escape_typst(&trimmed[3..]);
            output.push_str(&format!("== {text}\n\n"));
            continue;
        }
        if trimmed.starts_with("### ") {
            let text = escape_typst(&trimmed[4..]);
            output.push_str(&format!("=== {text}\n\n"));
            continue;
        }

        // Handle checkbox items
        // Detect indentation for sub-tasks
        let indent_level = line.len() - line.trim_start().len();
        let is_sub = indent_level >= 2;

        if trimmed.starts_with("- [x] ") || trimmed.starts_with("- [X] ") {
            let text = escape_typst(&trimmed[6..]);
            let text = convert_inline_formatting_typst(&text);
            if is_sub {
                output.push_str(&format!("#subtask(true)[{text}]\n"));
            } else {
                output.push_str(&format!("#task(true)[{text}]\n"));
            }
            continue;
        }
        if trimmed.starts_with("- [ ] ") {
            let text = escape_typst(&trimmed[6..]);
            let text = convert_inline_formatting_typst(&text);
            if is_sub {
                output.push_str(&format!("#subtask(false)[{text}]\n"));
            } else {
                output.push_str(&format!("#task(false)[{text}]\n"));
            }
            continue;
        }

        // Handle regular list items (treat as unchecked tasks)
        if trimmed.starts_with("- ") {
            let text = escape_typst(&trimmed[2..]);
            let text = convert_inline_formatting_typst(&text);
            if is_sub {
                output.push_str(&format!("#subtask(false)[{text}]\n"));
            } else {
                output.push_str(&format!("#task(false)[{text}]\n"));
            }
            continue;
        }

        // Handle empty lines
        if trimmed.is_empty() {
            output.push_str("#v(0.3em)\n");
            continue;
        }

        // Regular text
        let text = convert_inline_formatting(&trimmed);
        output.push_str(&text);
        output.push('\n');
    }

    // Add empty rows for handwritten additions
    output.push_str("\n#v(1em)\n");
    output.push_str("== New Tasks\n\n");
    for _ in 0..8 {
        output.push_str("#empty-row\n");
        output.push_str("#v(0.5em)\n");
    }

    output
}

/// Minimal inline formatting for content inside Typst content blocks [...].
/// Bold and italic only -- links are not useful in checkbox labels.
fn convert_inline_formatting_typst(text: &str) -> String {
    let mut result = text.to_string();
    result = convert_paired_marker(&result, "**", "*");
    result = convert_paired_marker(&result, "*", "_");
    result
}

/// Strip an ordered list prefix like "1. ", "2. ", etc.
fn strip_ordered_list_prefix(line: &str) -> Option<&str> {
    let trimmed = line.trim_start();
    let digit_end = trimmed.find(|c: char| !c.is_ascii_digit())?;
    if digit_end > 0 && trimmed[digit_end..].starts_with(". ") {
        Some(&trimmed[digit_end + 2..])
    } else {
        None
    }
}

/// Escape characters that have special meaning in typst
fn escape_typst(text: &str) -> String {
    text.replace('\\', "\\\\")
        .replace('#', "\\#")
        .replace('$', "\\$")
        .replace('@', "\\@")
}

/// Convert inline markdown formatting to typst
fn convert_inline_formatting(text: &str) -> String {
    let mut result = escape_typst(text);

    // Bold: **text** -> *text*
    result = convert_paired_marker(&result, "**", "*");

    // Italic: *text* or _text_ -> _text_
    result = convert_paired_marker(&result, "*", "_");

    // Links: [text](url) -> #link("url")[text]
    result = convert_links(&result);

    result
}

/// Convert paired markdown markers to typst equivalents
fn convert_paired_marker(text: &str, md_marker: &str, typst_marker: &str) -> String {
    let parts: Vec<&str> = text.split(md_marker).collect();
    if parts.len() < 3 {
        return text.to_string();
    }

    let mut result = String::new();
    let mut inside = false;
    for (i, part) in parts.iter().enumerate() {
        if i > 0 {
            result.push_str(typst_marker);
        }
        result.push_str(part);
        if i > 0 || parts.len() % 2 == 1 {
            inside = !inside;
        }
    }
    result
}

/// Convert markdown links to typst links
fn convert_links(text: &str) -> String {
    let mut result = String::new();
    let mut remaining = text;

    while let Some(start) = remaining.find('[') {
        result.push_str(&remaining[..start]);
        remaining = &remaining[start..];

        if let Some(mid) = remaining.find("](") {
            let link_text = &remaining[1..mid];
            let after_paren = &remaining[mid + 2..];
            if let Some(end) = after_paren.find(')') {
                let url = &after_paren[..end];
                result.push_str(&format!("#link(\"{url}\")[{link_text}]"));
                remaining = &after_paren[end + 1..];
                continue;
            }
        }

        result.push('[');
        remaining = &remaining[1..];
    }

    result.push_str(remaining);
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_strip_ordered_list() {
        assert_eq!(strip_ordered_list_prefix("1. First"), Some("First"));
        assert_eq!(strip_ordered_list_prefix("12. Twelfth"), Some("Twelfth"));
        assert_eq!(strip_ordered_list_prefix("Not a list"), None);
        assert_eq!(strip_ordered_list_prefix("- Unordered"), None);
    }

    #[test]
    fn test_markdown_to_typst_headings() {
        let md = "# Title\n## Section\n### Sub\n";
        let result = markdown_to_typst(md, &PushFormat::Default);
        assert!(result.contains("= Title"));
        assert!(result.contains("== Section"));
        assert!(result.contains("=== Sub"));
    }

    #[test]
    fn test_tasks_format() {
        let md = "# Work\n- [ ] Do the thing\n- [x] Already done\n  - [ ] Sub-task\n";
        let result = markdown_to_typst_tasks(md);
        assert!(result.contains("#task(false)[Do the thing]"));
        assert!(result.contains("#task(true)[Already done]"));
        assert!(result.contains("#subtask(false)[Sub-task]"));
        assert!(result.contains("== New Tasks"));
        assert!(result.contains("#empty-row"));
    }

    #[test]
    fn test_format_preambles() {
        let default = format_preamble(&PushFormat::Default);
        assert!(default.contains("margin: (x: 2cm, y: 2cm)"));
        assert!(default.contains("size: 11pt"));
        assert!(default.contains("justify: true"));

        let recipe = format_preamble(&PushFormat::Recipe);
        assert!(recipe.contains("margin: (x: 2.5cm, y: 2.5cm)"));
        assert!(recipe.contains("size: 12pt"));
        assert!(recipe.contains("justify: false"));

        let briefing = format_preamble(&PushFormat::Briefing);
        assert!(briefing.contains("justify: false"));
        assert!(briefing.contains("size: 11pt"));
    }
}

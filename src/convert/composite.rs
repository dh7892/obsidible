use anyhow::{Context, Result, bail};
use std::path::{Path, PathBuf};
use tokio::process::Command;
use tracing::debug;

use super::pdf_info::PdfPageSize;

/// Composite handwritten annotations from an .rm file onto a base PDF page image.
///
/// Pipeline:
/// 1. Convert .rm -> SVG via `rmc -t svg`
/// 2. Parse SVG and transform coordinates from rmc's centered point-space to pixel-space
/// 3. Write a new SVG at the target image dimensions
/// 4. Render the transformed SVG to PNG via `rsvg-convert`
/// 5. Composite the annotation PNG onto the base page image via ImageMagick
///
/// Returns the path to the composited output image.
pub async fn composite_annotations(
    rm_path: &Path,
    base_image: &Path,
    output_path: &Path,
    page_size: &PdfPageSize,
    dpi: u32,
) -> Result<PathBuf> {
    let work_dir = output_path.parent().unwrap_or(Path::new("/tmp"));

    let stem = rm_path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("annotation");

    // Step 1: Convert .rm to SVG using rmc
    let raw_svg_path = work_dir.join(format!("{stem}-raw.svg"));
    debug!(
        "Converting .rm to SVG: {} -> {}",
        rm_path.display(),
        raw_svg_path.display()
    );

    let rmc_output = Command::new("rmc")
        .args([
            "-t",
            "svg",
            "-o",
            &raw_svg_path.to_string_lossy(),
            &rm_path.to_string_lossy(),
        ])
        .output()
        .await
        .context("Failed to execute rmc. Is it installed? (pip install rmc)")?;

    if !rmc_output.status.success() {
        let stderr = String::from_utf8_lossy(&rmc_output.stderr);
        bail!("rmc failed for {}: {}", rm_path.display(), stderr.trim());
    }

    if !raw_svg_path.exists() {
        bail!(
            "rmc succeeded but SVG not found at {}",
            raw_svg_path.display()
        );
    }

    // Step 2: Read and transform the SVG
    let raw_svg = std::fs::read_to_string(&raw_svg_path)
        .with_context(|| format!("Failed to read SVG: {}", raw_svg_path.display()))?;

    let scale = dpi as f64 / 72.0;
    let img_width = (page_size.width_pt * scale).round() as u32;
    let img_height = (page_size.height_pt * scale).round() as u32;
    let x_offset = page_size.width_pt / 2.0;

    let transformed_svg = transform_svg(&raw_svg, x_offset, scale, img_width, img_height)?;

    // Step 3: Write the transformed SVG
    let transformed_svg_path = work_dir.join(format!("{stem}-transformed.svg"));
    std::fs::write(&transformed_svg_path, &transformed_svg).with_context(|| {
        format!(
            "Failed to write transformed SVG: {}",
            transformed_svg_path.display()
        )
    })?;

    // Step 4: Render the transformed SVG to PNG
    let annotation_png = work_dir.join(format!("{stem}-annotation.png"));
    debug!(
        "Rendering annotation SVG to PNG: {}",
        annotation_png.display()
    );

    let rsvg_output = Command::new("rsvg-convert")
        .args([
            "--width",
            &img_width.to_string(),
            "--height",
            &img_height.to_string(),
            "-o",
            &annotation_png.to_string_lossy(),
            &transformed_svg_path.to_string_lossy(),
        ])
        .output()
        .await
        .context("Failed to execute rsvg-convert. Is librsvg installed?")?;

    if !rsvg_output.status.success() {
        let stderr = String::from_utf8_lossy(&rsvg_output.stderr);
        bail!("rsvg-convert failed: {}", stderr.trim());
    }

    // Step 5: Composite annotation onto base image using ImageMagick
    debug!(
        "Compositing: {} + {} -> {}",
        base_image.display(),
        annotation_png.display(),
        output_path.display()
    );

    let magick_output = Command::new("magick")
        .args([
            "composite",
            &annotation_png.to_string_lossy(),
            &base_image.to_string_lossy(),
            &output_path.to_string_lossy(),
        ])
        .output()
        .await
        .context("Failed to execute magick (ImageMagick). Is ImageMagick installed?")?;

    if !magick_output.status.success() {
        let stderr = String::from_utf8_lossy(&magick_output.stderr);
        bail!("magick composite failed: {}", stderr.trim());
    }

    if !output_path.exists() {
        bail!(
            "magick composite succeeded but output not found at {}",
            output_path.display()
        );
    }

    Ok(output_path.to_path_buf())
}

/// Transform an rmc-generated SVG from its centered point-space coordinate system
/// to pixel-space coordinates matching the rendered PDF page image.
///
/// The rmc SVG uses coordinates in 72 DPI point-space with x centered at 0:
///   x range: [-page_width_pt/2, +page_width_pt/2]
///   y range: [0, page_height_pt]
///
/// We transform to pixel coordinates:
///   pixel_x = (svg_x + page_width_pt/2) * (dpi/72)
///   pixel_y = svg_y * (dpi/72)
fn transform_svg(
    raw_svg: &str,
    x_offset: f64,
    scale: f64,
    img_width: u32,
    img_height: u32,
) -> Result<String> {
    let mut output = String::new();

    // Write a new SVG header with the target pixel dimensions
    output.push_str(&format!(
        r#"<svg xmlns="http://www.w3.org/2000/svg" width="{img_width}" height="{img_height}" viewBox="0 0 {img_width} {img_height}">"#,
    ));
    output.push('\n');

    // Process each line of the SVG
    for line in raw_svg.lines() {
        let trimmed = line.trim();

        // Skip the original SVG header and closing tag
        if trimmed.starts_with("<svg")
            || trimmed.starts_with("</svg")
            || trimmed.starts_with("<?xml")
        {
            continue;
        }

        // Transform polyline elements
        if trimmed.contains("<polyline") || trimmed.contains("<polygon") {
            if let Some(transformed) = transform_polyline_element(trimmed, x_offset, scale) {
                output.push_str(&transformed);
                output.push('\n');
            }
            continue;
        }

        // Transform line elements
        if trimmed.contains("<line") {
            if let Some(transformed) = transform_line_element(trimmed, x_offset, scale) {
                output.push_str(&transformed);
                output.push('\n');
            }
            continue;
        }

        // Transform circle elements (dots)
        if trimmed.contains("<circle") {
            if let Some(transformed) = transform_circle_element(trimmed, x_offset, scale) {
                output.push_str(&transformed);
                output.push('\n');
            }
            continue;
        }

        // Transform path elements
        if trimmed.contains("<path") {
            if let Some(transformed) = transform_path_element(trimmed, x_offset, scale) {
                output.push_str(&transformed);
                output.push('\n');
            }
            continue;
        }

        // Pass through other elements (like style, defs, g, etc.)
        output.push_str(trimmed);
        output.push('\n');
    }

    output.push_str("</svg>\n");
    Ok(output)
}

/// Extract the value of a named attribute from an SVG element string.
fn extract_attr<'a>(element: &'a str, attr: &str) -> Option<&'a str> {
    let search = format!("{attr}=\"");
    let start = element.find(&search)? + search.len();
    let end = start + element[start..].find('"')?;
    Some(&element[start..end])
}

/// Replace the value of a named attribute in an SVG element string.
fn replace_attr(element: &str, attr: &str, new_value: &str) -> String {
    let search = format!("{attr}=\"");
    if let Some(start) = element.find(&search) {
        let val_start = start + search.len();
        if let Some(end_offset) = element[val_start..].find('"') {
            let end = val_start + end_offset;
            return format!("{}{}{}", &element[..val_start], new_value, &element[end..]);
        }
    }
    element.to_string()
}

/// Transform a polyline or polygon element's points attribute.
fn transform_polyline_element(element: &str, x_offset: f64, scale: f64) -> Option<String> {
    let points_str = extract_attr(element, "points")?;
    let transformed_points = transform_points(points_str, x_offset, scale);
    let mut result = replace_attr(element, "points", &transformed_points);

    // Scale stroke-width if present
    result = scale_stroke_width(&result, scale);

    Some(result)
}

/// Transform a line element's coordinates.
fn transform_line_element(element: &str, x_offset: f64, scale: f64) -> Option<String> {
    let mut result = element.to_string();

    if let Some(x1) = extract_attr(element, "x1") {
        if let Ok(v) = x1.parse::<f64>() {
            result = replace_attr(&result, "x1", &format!("{:.2}", (v + x_offset) * scale));
        }
    }
    if let Some(y1) = extract_attr(element, "y1") {
        if let Ok(v) = y1.parse::<f64>() {
            result = replace_attr(&result, "y1", &format!("{:.2}", v * scale));
        }
    }
    if let Some(x2) = extract_attr(element, "x2") {
        if let Ok(v) = x2.parse::<f64>() {
            result = replace_attr(&result, "x2", &format!("{:.2}", (v + x_offset) * scale));
        }
    }
    if let Some(y2) = extract_attr(element, "y2") {
        if let Ok(v) = y2.parse::<f64>() {
            result = replace_attr(&result, "y2", &format!("{:.2}", v * scale));
        }
    }

    result = scale_stroke_width(&result, scale);
    Some(result)
}

/// Transform a circle element's center coordinates and radius.
fn transform_circle_element(element: &str, x_offset: f64, scale: f64) -> Option<String> {
    let mut result = element.to_string();

    if let Some(cx) = extract_attr(element, "cx") {
        if let Ok(v) = cx.parse::<f64>() {
            result = replace_attr(&result, "cx", &format!("{:.2}", (v + x_offset) * scale));
        }
    }
    if let Some(cy) = extract_attr(element, "cy") {
        if let Ok(v) = cy.parse::<f64>() {
            result = replace_attr(&result, "cy", &format!("{:.2}", v * scale));
        }
    }
    if let Some(r) = extract_attr(element, "r") {
        if let Ok(v) = r.parse::<f64>() {
            result = replace_attr(&result, "r", &format!("{:.2}", v * scale));
        }
    }

    result = scale_stroke_width(&result, scale);
    Some(result)
}

/// Transform a path element's d attribute.
/// This handles SVG path commands (M, L, C, Q, etc.) by transforming their coordinates.
fn transform_path_element(element: &str, x_offset: f64, scale: f64) -> Option<String> {
    let d = extract_attr(element, "d")?;
    let transformed_d = transform_path_data(d, x_offset, scale);
    let mut result = replace_attr(element, "d", &transformed_d);
    result = scale_stroke_width(&result, scale);
    Some(result)
}

/// Transform SVG path data (the d attribute).
/// Handles absolute commands: M, L, C, Q, S, T, A, H, V, Z
/// and relative commands: m, l, c, q, s, t, a, h, v, z
fn transform_path_data(d: &str, x_offset: f64, scale: f64) -> String {
    let mut result = String::new();
    let mut chars = d.chars().peekable();
    let mut current_cmd = ' ';

    while chars.peek().is_some() {
        // Skip whitespace and commas
        while chars.peek().is_some_and(|c| c.is_whitespace() || *c == ',') {
            result.push(chars.next().unwrap());
        }

        // Check for a command letter
        if chars.peek().is_some_and(|c| c.is_ascii_alphabetic()) {
            current_cmd = chars.next().unwrap();
            result.push(current_cmd);
            continue;
        }

        if chars.peek().is_none() {
            break;
        }

        // Read numbers based on the current command
        match current_cmd {
            // Absolute move/line: pairs of (x, y)
            'M' | 'L' | 'T' => {
                if let Some(x) = read_number(&mut chars) {
                    skip_separator(&mut chars, &mut result);
                    if let Some(y) = read_number(&mut chars) {
                        result.push_str(&format!("{:.2}", (x + x_offset) * scale));
                        result.push(',');
                        result.push_str(&format!("{:.2}", y * scale));
                    }
                }
            }
            // Relative move/line: pairs of (dx, dy) -- only scale, no offset
            'm' | 'l' | 't' => {
                if let Some(dx) = read_number(&mut chars) {
                    skip_separator(&mut chars, &mut result);
                    if let Some(dy) = read_number(&mut chars) {
                        result.push_str(&format!("{:.2}", dx * scale));
                        result.push(',');
                        result.push_str(&format!("{:.2}", dy * scale));
                    }
                }
            }
            // Absolute cubic bezier: (x1,y1, x2,y2, x,y)
            'C' => {
                for i in 0..3 {
                    if i > 0 {
                        result.push(' ');
                    }
                    if let Some(x) = read_number(&mut chars) {
                        skip_separator(&mut chars, &mut result);
                        if let Some(y) = read_number(&mut chars) {
                            result.push_str(&format!("{:.2}", (x + x_offset) * scale));
                            result.push(',');
                            result.push_str(&format!("{:.2}", y * scale));
                        }
                    }
                    skip_separator(&mut chars, &mut result);
                }
            }
            // Relative cubic bezier
            'c' => {
                for i in 0..3 {
                    if i > 0 {
                        result.push(' ');
                    }
                    if let Some(dx) = read_number(&mut chars) {
                        skip_separator(&mut chars, &mut result);
                        if let Some(dy) = read_number(&mut chars) {
                            result.push_str(&format!("{:.2}", dx * scale));
                            result.push(',');
                            result.push_str(&format!("{:.2}", dy * scale));
                        }
                    }
                    skip_separator(&mut chars, &mut result);
                }
            }
            // Absolute quadratic bezier: (x1,y1, x,y)
            'Q' | 'S' => {
                for i in 0..2 {
                    if i > 0 {
                        result.push(' ');
                    }
                    if let Some(x) = read_number(&mut chars) {
                        skip_separator(&mut chars, &mut result);
                        if let Some(y) = read_number(&mut chars) {
                            result.push_str(&format!("{:.2}", (x + x_offset) * scale));
                            result.push(',');
                            result.push_str(&format!("{:.2}", y * scale));
                        }
                    }
                    skip_separator(&mut chars, &mut result);
                }
            }
            // Relative quadratic bezier
            'q' | 's' => {
                for i in 0..2 {
                    if i > 0 {
                        result.push(' ');
                    }
                    if let Some(dx) = read_number(&mut chars) {
                        skip_separator(&mut chars, &mut result);
                        if let Some(dy) = read_number(&mut chars) {
                            result.push_str(&format!("{:.2}", dx * scale));
                            result.push(',');
                            result.push_str(&format!("{:.2}", dy * scale));
                        }
                    }
                    skip_separator(&mut chars, &mut result);
                }
            }
            // Absolute horizontal line
            'H' => {
                if let Some(x) = read_number(&mut chars) {
                    result.push_str(&format!("{:.2}", (x + x_offset) * scale));
                }
            }
            // Relative horizontal line
            'h' => {
                if let Some(dx) = read_number(&mut chars) {
                    result.push_str(&format!("{:.2}", dx * scale));
                }
            }
            // Absolute vertical line
            'V' => {
                if let Some(y) = read_number(&mut chars) {
                    result.push_str(&format!("{:.2}", y * scale));
                }
            }
            // Relative vertical line
            'v' => {
                if let Some(dy) = read_number(&mut chars) {
                    result.push_str(&format!("{:.2}", dy * scale));
                }
            }
            // Close path -- no coordinates
            'Z' | 'z' => {}
            // For unrecognised commands, try to pass through numbers
            _ => {
                if let Some(n) = read_number(&mut chars) {
                    result.push_str(&format!("{:.2}", n * scale));
                }
            }
        }
    }

    result
}

/// Read a floating point number from a char iterator.
fn read_number(chars: &mut std::iter::Peekable<std::str::Chars>) -> Option<f64> {
    // Skip whitespace and commas
    while chars.peek().is_some_and(|c| c.is_whitespace() || *c == ',') {
        chars.next();
    }

    let mut num_str = String::new();

    // Optional sign
    if chars.peek().is_some_and(|c| *c == '-' || *c == '+') {
        num_str.push(chars.next().unwrap());
    }

    // Integer part
    while chars.peek().is_some_and(|c| c.is_ascii_digit()) {
        num_str.push(chars.next().unwrap());
    }

    // Decimal part
    if chars.peek() == Some(&'.') {
        num_str.push(chars.next().unwrap());
        while chars.peek().is_some_and(|c| c.is_ascii_digit()) {
            num_str.push(chars.next().unwrap());
        }
    }

    // Exponent
    if chars.peek().is_some_and(|c| *c == 'e' || *c == 'E') {
        num_str.push(chars.next().unwrap());
        if chars.peek().is_some_and(|c| *c == '-' || *c == '+') {
            num_str.push(chars.next().unwrap());
        }
        while chars.peek().is_some_and(|c| c.is_ascii_digit()) {
            num_str.push(chars.next().unwrap());
        }
    }

    if num_str.is_empty() || num_str == "-" || num_str == "+" {
        return None;
    }

    num_str.parse().ok()
}

/// Skip whitespace and commas between coordinates, don't add them to result.
fn skip_separator(chars: &mut std::iter::Peekable<std::str::Chars>, _result: &mut String) {
    while chars.peek().is_some_and(|c| c.is_whitespace() || *c == ',') {
        chars.next();
    }
}

/// Transform a space-separated list of "x,y" coordinate pairs.
fn transform_points(points_str: &str, x_offset: f64, scale: f64) -> String {
    points_str
        .split_whitespace()
        .filter_map(|pair| {
            let parts: Vec<&str> = pair.split(',').collect();
            if parts.len() == 2 {
                let x: f64 = parts[0].parse().ok()?;
                let y: f64 = parts[1].parse().ok()?;
                Some(format!("{:.2},{:.2}", (x + x_offset) * scale, y * scale))
            } else {
                None
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// Scale the stroke-width attribute by the given factor.
fn scale_stroke_width(element: &str, scale: f64) -> String {
    if let Some(sw) = extract_attr(element, "stroke-width") {
        if let Ok(w) = sw.parse::<f64>() {
            return replace_attr(element, "stroke-width", &format!("{:.2}", w * scale));
        }
    }

    // Also check for stroke-width in a style attribute
    if let Some(style) = extract_attr(element, "style") {
        if let Some(sw_start) = style.find("stroke-width:") {
            let after = &style[sw_start + "stroke-width:".len()..];
            let after = after.trim_start();
            // Read the numeric value
            let end = after
                .find(|c: char| !c.is_ascii_digit() && c != '.')
                .unwrap_or(after.len());
            if let Ok(w) = after[..end].parse::<f64>() {
                let new_style = format!(
                    "{}stroke-width:{:.2}{}",
                    &style[..sw_start],
                    w * scale,
                    &after[end..]
                );
                return replace_attr(element, "style", &new_style);
            }
        }
    }

    element.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_transform_points() {
        // A4 width = 595.28pt, so x_offset = 297.64
        // At 200 DPI, scale = 200/72 = 2.778
        let points = "-100.0,50.0 0.0,100.0 100.0,150.0";
        let result = transform_points(points, 297.64, 200.0 / 72.0);

        let pairs: Vec<&str> = result.split_whitespace().collect();
        assert_eq!(pairs.len(), 3);

        // First point: (-100 + 297.64) * 2.778 = 549.01
        let p0: Vec<&str> = pairs[0].split(',').collect();
        let x0: f64 = p0[0].parse().unwrap();
        assert!((x0 - 549.01).abs() < 1.0);
    }

    #[test]
    fn test_extract_attr() {
        let elem = r#"<polyline points="1,2 3,4" stroke="black" stroke-width="0.5"/>"#;
        assert_eq!(extract_attr(elem, "points"), Some("1,2 3,4"));
        assert_eq!(extract_attr(elem, "stroke"), Some("black"));
        assert_eq!(extract_attr(elem, "stroke-width"), Some("0.5"));
        assert_eq!(extract_attr(elem, "fill"), None);
    }

    #[test]
    fn test_replace_attr() {
        let elem = r#"<polyline points="1,2" stroke-width="0.5"/>"#;
        let result = replace_attr(elem, "stroke-width", "1.39");
        assert!(result.contains(r#"stroke-width="1.39""#));
    }
}

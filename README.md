# obsidible

CLI tool for converting and transporting documents between [Obsidian](https://obsidian.md/) vaults and [reMarkable](https://remarkable.com/) tablets.

Designed to be driven by an AI coding assistant (e.g. an OpenCode/Claude Code skill), handling the deterministic conversion pipeline so the assistant can focus on interpretation and vault management.

## Commands

### `obsidible pull`

Download a document from the reMarkable, convert it to PNG images ready for LLM interpretation.

```
obsidible pull <RM_PATH> [--output-dir /tmp/rm-work] [--dpi 200]
```

The pipeline handles:
- Downloading via `rmapi`
- Extracting the `.rmdoc` archive
- Detecting file type (notebook / PDF / EPUB)
- **Notebooks**: converting `.rm` stroke files to SVG then PNG
- **PDFs with annotations**: rendering base pages, transforming annotation coordinates from reMarkable point-space to pixel-space, compositing handwritten strokes onto PDF pages via ImageMagick
- **PDFs without annotations**: rendering pages directly

Output is JSON to stdout:

```json
{
  "document_name": "Quick sheets",
  "file_type": "notebook",
  "pages": ["/tmp/rm-work/page-001.png", "/tmp/rm-work/page-002.png"],
  "has_annotations": true
}
```

### `obsidible push`

Convert a local file to PDF and upload it to the reMarkable.

```
obsidible push <LOCAL_PATH> <RM_DESTINATION> [--format default]
```

Accepts `.md` (converted to PDF via Typst) or `.pdf` (uploaded directly). Deletes any existing file with the same name at the destination before uploading.

Format presets:

| Format | Description |
|--------|-------------|
| `default` | 11pt, A4, 2cm margins, justified |
| `recipe` | 12pt, 2.5cm margins, no justification, space for annotations |
| `briefing` | 11pt, scannable layout, larger headings |
| `tasks` | 12pt, checkbox grid with empty rows for handwritten additions |

The `tasks` format expects markdown checkbox syntax:

```markdown
# Section
- [ ] Unchecked task
- [x] Completed task
  - [ ] Sub-task
```

Output is JSON to stdout:

```json
{
  "uploaded": "/Briefings/Briefing 2026-03-23",
  "source": "/tmp/rm-work/briefing.md",
  "pages": 1
}
```

### `obsidible auth`

Run `rmapi` interactive authentication.

## Installation

Requires [Nix](https://nixos.org/) with flakes enabled.

```bash
# One-off install
nix profile install github:dh7892/obsidible

# Or add to a flake-based system config (nix-darwin / home-manager)
# See flake.nix for the package definition
```

The Nix package wraps the binary with all runtime dependencies on PATH -- no need to install them separately.

## Runtime dependencies

All bundled by the Nix flake:

- [rmapi](https://github.com/ddvk/rmapi) (v0.0.29+) -- reMarkable cloud API
- [rmc](https://github.com/ricklupton/rmc) -- `.rm` stroke file to SVG conversion
- [rsvg-convert](https://wiki.gnome.org/Projects/LibRsvg) (librsvg) -- SVG to PNG
- [typst](https://typst.app/) -- markdown to PDF compilation
- [pdftoppm / pdfinfo](https://poppler.freedesktop.org/) (poppler-utils) -- PDF rendering and metadata
- [magick](https://imagemagick.org/) (ImageMagick) -- image compositing

## Development

```bash
# Enter the dev shell (provides Rust toolchain + all runtime deps)
nix develop

# Build
cargo build

# Run tests
cargo test

# Run directly
cargo run -- pull "/Quick sheets" --output-dir /tmp/rm-work
```

## How it works

### PDF annotation compositing

When a reMarkable document is a PDF with handwritten annotations, the `.rm` stroke data needs to be overlaid onto the rendered PDF pages. The `rmc` tool converts strokes to SVG using a coordinate system in PDF points (72 DPI) with the x-axis centered on the page. The mapping to image pixel coordinates is:

```
pixel_x = (svg_x + page_width_pt / 2) * (render_dpi / 72)
pixel_y = svg_y * (render_dpi / 72)
```

The tool parses the SVG elements, applies this transform, renders to PNG, then composites onto the base PDF page image.

## License

MIT

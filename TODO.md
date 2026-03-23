# Obsidible -- Planned CLI Commands

Two new standalone CLI subcommands that handle the full conversion pipelines for pulling and pushing content between the reMarkable and Obsidian. These commands handle the file conversion, image compositing, and transport steps -- the LLM interpretation and vault filing remain in the `/rem` skill (driven by the AI assistant).

---

## Command 1: `obsidible pull`

**Purpose:** Download a document from the reMarkable, convert it to one or more PNG images ready for LLM interpretation, and output the image path(s).

### Current state

The building blocks already exist in `src/convert/`:
- `rmdoc.rs` -- extracts `.rmdoc` zip archives, parses `.content` JSON metadata
- `rm_to_images.rs` -- converts `.rm` stroke files to PNG via `rmc` (SVG) then `rsvg-convert` (PNG)
- `pdf_to_images.rs` -- renders PDF pages to PNG via `pdftoppm`

**What's missing:** compositing handwritten annotations onto PDF pages. Currently `rm_to_images.rs` renders `.rm` files as standalone images on a blank canvas. When the document is a PDF (`fileType: "pdf"` in `.content`), the annotations need to be overlaid onto the rendered PDF pages.

### Detailed pipeline

```
obsidible pull <rm_path> [--output-dir /tmp/rm-work] [--dpi 200]
```

1. **Download** the document from the reMarkable cloud:
   ```
   rmapi get "<rm_path>"  ->  <name>.rmdoc
   ```

2. **Extract** the `.rmdoc` (zip archive). Parse the `.content` JSON to get:
   - `fileType`: "notebook", "pdf", or "epub"
   - `pages`: ordered list of page UUIDs
   - The document UUID (from the `.content` filename)

3. **Branch by file type:**

   **A. Notebook (pure handwriting):**
   - For each page UUID, find `<doc-uuid>/<page-uuid>.rm`
   - Convert: `.rm` -> SVG (via `rmc -t svg`) -> PNG (via `rsvg-convert`)
   - Output the PNG files

   **B. PDF with annotations (this is the new part):**
   - Render the base PDF to per-page images: `pdftoppm -png -r 200 <uuid>.pdf base`
   - For each page, check if a corresponding `.rm` file exists in `<doc-uuid>/`
   - If an `.rm` file exists, **composite the annotations onto the PDF page image** (see coordinate mapping section below)
   - If no `.rm` file for a page, use the base PDF image as-is
   - Output the composited images

   **C. PDF without annotations:**
   - Render pages with `pdftoppm` and output directly

   **D. EPUB:**
   - Extract HTML text from the zip
   - Output as a text file (or rendered images if needed)

4. **Print** the output image paths (one per line) to stdout as JSON, so the calling tool can read them.

### PDF annotation compositing -- coordinate mapping

This is the critical piece discovered through experimentation. The `rmc` tool converts `.rm` stroke data to SVG using a coordinate system that is **already in PDF points (72 DPI)** with the x-axis centered on the page.

**The mapping from rmc SVG coordinates to PDF coordinates is:**

```
pdf_x = svg_x + (page_width_pt / 2)
pdf_y = svg_y
```

Where `page_width_pt` is the PDF page width in points (e.g. 595.28 for A4). Get this from `pdfinfo` or by reading the PDF directly.

**To convert to image pixel coordinates at a given render DPI:**

```
img_x = (svg_x + page_width_pt / 2) * (render_dpi / 72)
img_y = svg_y * (render_dpi / 72)
```

For example, at 200 DPI on an A4 page:
```
img_x = (svg_x + 297.64) * 2.778
img_y = svg_y * 2.778
```

**Why this works:** The `rmc` source code (`exporters/svg.py`) defines:
```python
SCREEN_WIDTH = 1404      # reMarkable screen pixels
SCREEN_HEIGHT = 1872
SCREEN_DPI = 226
SCALE = 72.0 / SCREEN_DPI  # = 0.31858
```

It multiplies all raw screen-pixel coordinates by `SCALE` (72/226), converting them from 226 DPI screen space to 72 DPI point space. The x-axis is centered at 0 (range: -page_width_pt/2 to +page_width_pt/2). The y-axis starts at 0 at the top of the page.

**Implementation approach:**

1. Run `rmc -t svg` on the `.rm` file to get the SVG with polyline stroke data
2. Parse the SVG to extract all `<polyline points="...">` elements (and their style attributes like stroke color, width, opacity)
3. Transform every point using the formula above
4. Write a new SVG at the image pixel dimensions (matching the `pdftoppm` output size)
5. Render the transformed SVG to PNG with `rsvg-convert`
6. Composite onto the base PDF page image using ImageMagick: `magick base.png annotation.png -composite combined.png`

### Output format

```json
{
  "document_name": "Tasks",
  "file_type": "pdf",
  "pages": [
    "/tmp/rm-work/page-001.png",
    "/tmp/rm-work/page-002.png"
  ],
  "has_annotations": true
}
```

---

## Command 2: `obsidible push`

**Purpose:** Convert markdown content to a clean PDF and upload it to the reMarkable.

### Current state

The building blocks exist:
- `md_to_pdf.rs` -- converts markdown to Typst markup, compiles to PDF
- `remarkable.rs` -- wraps `rmapi` for upload, directory creation, deletion

### Detailed pipeline

```
obsidible push <local_path> <rm_destination> [--format recipe|briefing|tasks|default]
```

1. **Read** the input file. It can be:
   - A markdown file (`.md`) -- convert to PDF
   - An already-compiled PDF (`.pdf`) -- upload directly

2. **Convert markdown to PDF** (if needed):
   - Parse the markdown and convert to Typst markup (existing `markdown_to_typst()`)
   - Apply format-specific Typst settings:
     - **default**: 11pt, A4, 2cm margins, justified
     - **recipe**: 12pt, no justification, generous margins (2.5cm) for annotation space, 1-2 pages max
     - **briefing**: 11pt, scannable layout, clear headings, bullet points
     - **tasks**: 12pt, checkbox grid layout with empty rows at the bottom for handwritten additions (see task format below)
   - Compile with `typst compile`

3. **Upload** to the reMarkable:
   - Ensure the destination directory exists: `rmapi mkdir <parent_dirs>`
   - If a file with the same name already exists at the destination, delete it first: `rmapi rm "<rm_path>"`
   - Upload: `rmapi put <local.pdf> <rm_destination>`

4. **Print** result to stdout:
   ```json
   {
     "uploaded": "/Tasks",
     "source": "/tmp/rm-work/Tasks.pdf",
     "pages": 1
   }
   ```

### Task list format

When `--format tasks` is specified, the input should be a simple text format (one task per line):

```
# Section Name
- [ ] Task description
- [x] Completed task description
```

The Typst output should render this as:
- Section headings as `== Section Name`
- Unchecked tasks as a grid row with an empty checkbox box (0.9em square, 0.8pt stroke) and the task text
- Checked tasks as a grid row with an X'd checkbox and strikethrough text
- Sub-items (indented with spaces/tabs) rendered as nested grids
- Empty checkbox rows at the bottom under a "New Tasks" heading for handwritten additions
- Generous margins (2cm) so Dave can write in them

### Note on rmapi version

The `rmapi` binary must be version 0.0.29+ from the `ddvk/rmapi` fork (not the archived `juruen/rmapi`). The old version fails with HTTP 202 errors on upload because the reMarkable cloud API now returns 202 (Accepted) for async uploads, which the old version treats as an error.

---

## Integration with OpenCode/Claude Code skills

The `/rem` skill at `~/.claude/skills/rem/SKILL.md` should be updated to use these commands instead of manually running the conversion pipeline. Specifically:

### Pull workflow changes

Replace the manual steps in sections 2-4b (download, extract, convert, composite) with:

```bash
obsidible pull "<rm_path>" --output-dir /tmp/rm-work --dpi 200
```

The command handles everything: downloading via `rmapi`, extracting the `.rmdoc`, detecting the file type, converting notebooks to images, rendering PDFs, and compositing handwritten annotations onto PDF pages.

The AI assistant then reads the output images (from the JSON stdout) for LLM interpretation, vault filing, and post-actions -- which remain manual/interactive steps.

### Push workflow changes

Replace the manual Typst file generation and compilation with:

```bash
obsidible push /tmp/rm-work/content.md "/Briefings" --format briefing
```

Or for task sync:

```bash
obsidible push /tmp/rm-work/tasks.md "/Tasks" --format tasks
```

The AI assistant still gathers the content (e.g. querying `obsidian tasks todo vault=work`), writes it to a temporary markdown file in the expected format, then calls `obsidible push` to handle conversion and upload.

### Sync tasks workflow changes

The sync workflow in the `/rem` skill combines both commands:

1. `obsidible pull "/Tasks"` -- pull and render the annotated task list
2. AI reads the images, identifies checked/new tasks, updates the vault
3. AI queries `obsidian tasks todo vault=work` for the current task list
4. AI writes the task list to a temp markdown file
5. `obsidible push /tmp/tasks.md "/" --format tasks` -- generate PDF and upload

---

## Shared considerations

### Error handling
- All external tool invocations (`rmapi`, `rmc`, `rsvg-convert`, `typst`, `pdftoppm`, `magick`) should have clear error messages indicating which tool failed and whether it's installed
- `rmapi` auth failures should suggest running `rmapi` interactively to re-authenticate

### Dependencies (external CLI tools)
- `rmapi` (v0.0.29+ from ddvk/rmapi) -- reMarkable cloud API
- `rmc` (Python, pip install rmc) -- `.rm` stroke file conversion
- `rsvg-convert` (librsvg) -- SVG to PNG
- `typst` -- Typst compiler for PDF generation
- `pdftoppm` (poppler-utils) -- PDF page rendering
- `magick` (ImageMagick) -- image compositing
- `pdfinfo` (poppler-utils) -- PDF metadata (page dimensions)

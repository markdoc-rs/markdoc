use crate::schema::default_nodes;
use crate::types::*;
use std::collections::HashMap;

pub fn default_tags() -> HashMap<String, Schema> {
    let mut tags = HashMap::new();

    // ── if ──────────────────────────────────────────────────────────────
    // The predicate lives in `Node.expressions["primary"]` (see parser),
    // so the schema tracks `primary` rather than the older `condition` key.
    tags.insert(
        "if".to_string(),
        Schema {
            render: None,
            children: None,
            attributes: Some({
                let mut attrs = HashMap::new();
                attrs.insert(
                    "primary".to_string(),
                    SchemaAttribute {
                        attr_type: Some(vec![
                            ValidationType::Boolean,
                            ValidationType::String,
                            ValidationType::Number,
                            ValidationType::Object,
                            ValidationType::Array,
                        ]),
                        render: Some(SchemaRender::Bool(false)),
                        default: None,
                        required: false,
                        description: Some("Predicate to evaluate".to_string()),
                    },
                );
                attrs
            }),
            self_closing: false,
            inline: false,
            description: Some("Conditional rendering tag".to_string()),
        },
    );

    // ── else ────────────────────────────────────────────────────────────
    // Self-closing branch separator inside a `{% if %}` body. Optional
    // primary expression turns it into an else-if; without one it is
    // the unconditional fallback.
    //
    //   {% if $a %} A {% else $b /%} B {% else /%} C {% /if %}
    //
    // The conditional evaluator splits the if's children at every
    // `{% else %}` and picks the first branch whose predicate is truthy.
    tags.insert(
        "else".to_string(),
        Schema {
            render: None,
            children: None,
            attributes: Some({
                let mut attrs = HashMap::new();
                attrs.insert(
                    "primary".to_string(),
                    SchemaAttribute {
                        attr_type: Some(vec![
                            ValidationType::Boolean,
                            ValidationType::String,
                            ValidationType::Number,
                            ValidationType::Object,
                            ValidationType::Array,
                        ]),
                        render: Some(SchemaRender::Bool(false)),
                        default: None,
                        required: false,
                        description: Some(
                            "Optional else-if predicate; bare `{% else /%}` is the fallback."
                                .into(),
                        ),
                    },
                );
                attrs
            }),
            self_closing: true,
            inline: false,
            description: Some("Branch separator inside `{% if %}`".to_string()),
        },
    );

    // ── partial ─────────────────────────────────────────────────────────
    tags.insert(
        "partial".to_string(),
        Schema {
            render: None,
            children: None,
            attributes: Some({
                let mut attrs = HashMap::new();
                attrs.insert(
                    "file".to_string(),
                    SchemaAttribute {
                        attr_type: Some(vec![ValidationType::String]),
                        render: Some(SchemaRender::Bool(false)),
                        default: None,
                        required: true,
                        description: Some("Path to partial file".to_string()),
                    },
                );
                attrs
            }),
            self_closing: true,
            inline: false,
            description: Some("Include a partial file".to_string()),
        },
    );

    // ── table ──────────────────────────────────────────────────────────
    // Used both as a list-syntax table (`{% table %} * a * b --- …`) and as
    // a styling wrapper around a pipe table. PDF honours attributes such as
    // `column_weights="1 2"` so consecutive tables can share column widths.
    tags.insert(
        "table".to_string(),
        Schema {
            render: Some("table".to_string()),
            children: None,
            attributes: Some({
                let mut attrs = HashMap::new();
                attrs.insert(
                    "column_weights".to_string(),
                    SchemaAttribute {
                        attr_type: Some(vec![ValidationType::String]),
                        render: None,
                        default: None,
                        required: false,
                        description: Some(
                            "Space/comma-separated relative column widths, e.g. \"1 2\" or \"3 1\""
                                .to_string(),
                        ),
                    },
                );
                attrs.insert(
                    "borders".to_string(),
                    SchemaAttribute {
                        attr_type: Some(vec![ValidationType::String]),
                        render: None,
                        default: None,
                        required: false,
                        description: Some("grid | horizontal | none".to_string()),
                    },
                );
                attrs.insert(
                    "fillable".to_string(),
                    SchemaAttribute {
                        attr_type: Some(vec![ValidationType::Boolean, ValidationType::String]),
                        render: None,
                        default: None,
                        required: false,
                        description: Some(
                            "Render the table's blank cells as fields that can be filled in \
                             and printed"
                                .to_string(),
                        ),
                    },
                );
                attrs.insert(
                    "header_column".to_string(),
                    SchemaAttribute {
                        attr_type: Some(vec![ValidationType::Boolean, ValidationType::String]),
                        render: None,
                        default: None,
                        required: false,
                        description: Some("When true, treat column 0 as row headers".to_string()),
                    },
                );
                attrs.insert(
                    "stripe".to_string(),
                    SchemaAttribute {
                        attr_type: Some(vec![ValidationType::String]),
                        render: None,
                        default: None,
                        required: false,
                        description: Some(
                            "Zebra stripe colour, or \"none\" to disable".to_string(),
                        ),
                    },
                );
                attrs.insert(
                    "cell_padding".to_string(),
                    SchemaAttribute {
                        attr_type: Some(vec![ValidationType::Number, ValidationType::String]),
                        render: None,
                        default: None,
                        required: false,
                        description: Some("Per-table cell padding in points".to_string()),
                    },
                );
                attrs.insert(
                    "header_background".to_string(),
                    SchemaAttribute {
                        attr_type: Some(vec![ValidationType::String]),
                        render: None,
                        default: None,
                        required: false,
                        description: Some("Header row fill colour (CSS colour)".to_string()),
                    },
                );
                attrs.insert(
                    "border_color".to_string(),
                    SchemaAttribute {
                        attr_type: Some(vec![ValidationType::String]),
                        render: None,
                        default: None,
                        required: false,
                        description: Some("Table border colour (CSS colour)".to_string()),
                    },
                );
                attrs.insert(
                    "edge_color".to_string(),
                    SchemaAttribute {
                        attr_type: Some(vec![ValidationType::String]),
                        render: None,
                        default: None,
                        required: false,
                        description: Some("Outer edge colour (CSS colour)".to_string()),
                    },
                );
                attrs
            }),
            self_closing: false,
            inline: false,
            description: Some("Table (list-syntax or pipe-table styling wrapper)".to_string()),
        },
    );

    // ── callout ────────────────────────────────────────────────────────
    // Block callout with a `type` discriminator. Default `type` is `note`
    // so authors can write `{% callout %}...{% /callout %}` without
    // specifying severity. The validator accepts any string for `type`;
    // renderers decide how to map types to visual treatments.
    tags.insert(
        "callout".to_string(),
        Schema {
            render: None,
            children: None,
            attributes: Some({
                let mut attrs = HashMap::new();
                attrs.insert(
                    "type".to_string(),
                    SchemaAttribute {
                        attr_type: Some(vec![ValidationType::String]),
                        render: None,
                        default: Some(Scalar::String("note".to_string())),
                        required: false,
                        description: Some(
                            "Severity: note, info, warning, caution, danger, success, notice"
                                .to_string(),
                        ),
                    },
                );
                attrs
            }),
            self_closing: false,
            inline: false,
            description: Some("Callout block".to_string()),
        },
    );

    // ── tag (anchor declaration) ────────────────────────────────────────
    // Declares a named anchor used by `{% tagref %}`. The Flux spec lets
    // the id appear under either `id=` or `tag=`; both are optional at
    // the schema level (cross-reference resolution validates that at
    // least one is present).
    tags.insert(
        "tag".to_string(),
        Schema {
            render: None,
            children: None,
            attributes: Some({
                let mut attrs = HashMap::new();
                attrs.insert(
                    "id".to_string(),
                    SchemaAttribute {
                        attr_type: Some(vec![ValidationType::String]),
                        render: None,
                        default: None,
                        required: false,
                        description: Some("Anchor id".to_string()),
                    },
                );
                attrs.insert(
                    "tag".to_string(),
                    SchemaAttribute {
                        attr_type: Some(vec![ValidationType::String]),
                        render: None,
                        default: None,
                        required: false,
                        description: Some("Anchor id (alternate spelling)".to_string()),
                    },
                );
                attrs
            }),
            self_closing: true,
            inline: true,
            description: Some("Named anchor declaration".to_string()),
        },
    );

    // ── media ──────────────────────────────────────────────────────────
    // Structural reference to a media asset: image (bitmap/vector),
    // animated image, 3D model, audio, or video. The `src` URI carries
    // any transformation parameters (size, focus, effects) and is opaque
    // to markdoc — Arca handles all media processing/transformation;
    // renderers fetch ready-to-embed bytes from Arca using the URI as-is.
    //
    // Authors typically write self-closing form:
    //   {% media src="arca://abc-123?w=400&focus=0.5,0.3" alt="Front panel" /%}
    // Block form is also accepted, with body content treated as a caption:
    //   {% media src="..." %}**Figure 1**: front-panel layout{% /media %}
    let media_schema = Schema {
        render: None,
        children: None,
        attributes: Some({
            let mut attrs = HashMap::new();
            attrs.insert(
                    "src".to_string(),
                    SchemaAttribute {
                        attr_type: Some(vec![ValidationType::String]),
                        render: None,
                        default: None,
                        // Optional because `id` is an alternative locator; a
                        // media reference must carry one of `src` or `id`.
                        required: false,
                        description: Some(
                            "Media URI (alternative to `id`). May carry transformation params interpreted by Arca."
                                .to_string(),
                        ),
                    },
                );
            attrs.insert(
                    "id".to_string(),
                    SchemaAttribute {
                        attr_type: Some(vec![ValidationType::String]),
                        render: None,
                        default: None,
                        required: false,
                        description: Some(
                            "Arca asset id (alternative to `src`). Scriptor rewrites it to a concrete `src` before render; locally, markdoc-pdf resolves it to a file named `<id>.<ext>` under the assets root."
                                .into(),
                        ),
                    },
                );
            attrs.insert(
                "alt".to_string(),
                SchemaAttribute {
                    attr_type: Some(vec![ValidationType::String]),
                    render: None,
                    default: None,
                    required: false,
                    description: Some(
                        "Accessibility / fallback text describing the media".to_string(),
                    ),
                },
            );
            attrs.insert(
                "title".to_string(),
                SchemaAttribute {
                    attr_type: Some(vec![ValidationType::String]),
                    render: None,
                    default: None,
                    required: false,
                    description: Some(
                        "Advisory title (the HTML `title` / tooltip); markdown `![alt](url \"title\")` sets it. Renderers may use it as a caption / alt fallback."
                            .to_string(),
                    ),
                },
            );
            attrs.insert(
                "caption".to_string(),
                SchemaAttribute {
                    attr_type: Some(vec![ValidationType::String]),
                    render: None,
                    default: None,
                    required: false,
                    description: Some("Visible caption rendered alongside the media".into()),
                },
            );
            attrs.insert(
                "kind".to_string(),
                SchemaAttribute {
                    attr_type: Some(vec![ValidationType::String]),
                    render: None,
                    default: None,
                    required: false,
                    description: Some(
                        "Optional type hint: image | vector | animation | video | audio | model"
                            .into(),
                    ),
                },
            );
            attrs.insert(
                    "side".to_string(),
                    SchemaAttribute {
                        attr_type: Some(vec![ValidationType::String]),
                        render: None,
                        default: None,
                        required: false,
                        description: Some(
                            "When placed inside a `{% float %}`, the side to float this image to: left or right"
                                .into(),
                        ),
                    },
                );
            attrs.insert(
                    "size".to_string(),
                    SchemaAttribute {
                        attr_type: Some(vec![ValidationType::String]),
                        render: None,
                        default: None,
                        required: false,
                        description: Some(
                            "Display size preset: small (45%) | medium (75%) | large (100%, default) of the available width".into(),
                        ),
                    },
                );
            attrs.insert(
                    "width".to_string(),
                    SchemaAttribute {
                        attr_type: Some(vec![ValidationType::Number, ValidationType::String]),
                        render: None,
                        default: None,
                        required: false,
                        description: Some(
                            "Explicit width for a floated image (`{% float %}`) — a fraction \u{2264} 1 of the column, a length, or a \"NN%\" string. For general image sizing use `size`."
                                .into(),
                        ),
                    },
                );
            attrs
        }),
        self_closing: true,
        inline: false,
        description: Some(
            "Media reference (image, video, audio, 3D model, animation, etc.)".into(),
        ),
    };
    // `img` is an alias so `{% img … /%}` and markdown images share the media
    // contract (both dispatch to the same renderer path).
    tags.insert("img".to_string(), {
        let mut s = media_schema.clone();
        s.description = Some("Image reference (alias of `media`)".to_string());
        s
    });
    tags.insert("media".to_string(), media_schema);

    // ── tagref (anchor reference) ───────────────────────────────────────
    tags.insert(
        "tagref".to_string(),
        Schema {
            render: None,
            children: None,
            attributes: Some({
                let mut attrs = HashMap::new();
                attrs.insert(
                    "id".to_string(),
                    SchemaAttribute {
                        attr_type: Some(vec![ValidationType::String]),
                        render: None,
                        default: None,
                        required: false,
                        description: Some("Target anchor id".to_string()),
                    },
                );
                attrs.insert(
                    "tag".to_string(),
                    SchemaAttribute {
                        attr_type: Some(vec![ValidationType::String]),
                        render: None,
                        default: None,
                        required: false,
                        description: Some("Target anchor id (alternate spelling)".to_string()),
                    },
                );
                attrs.insert(
                    "doc".to_string(),
                    SchemaAttribute {
                        attr_type: Some(vec![ValidationType::String]),
                        render: None,
                        default: None,
                        required: false,
                        description: Some(
                            "Cross-document target — id of the Adeptus document that owns the anchor. Adeptus rewrites these into resolved links before publish; renderers that see one in raw source treat it as an unresolved placeholder."
                                .into(),
                        ),
                    },
                );
                attrs.insert(
                    "text".to_string(),
                    SchemaAttribute {
                        attr_type: Some(vec![ValidationType::String]),
                        render: None,
                        default: None,
                        required: false,
                        description: Some(
                            "Visible link text. When omitted, the target section heading is used."
                                .into(),
                        ),
                    },
                );
                attrs
            }),
            self_closing: true,
            inline: true,
            description: Some("Named anchor reference".to_string()),
        },
    );

    // ── columns ─────────────────────────────────────────────────────────
    // Side-by-side layout: each child (a list item or a blank-line
    // separated block) becomes one column. A pure layout primitive —
    // renderers map it to their medium (parley table in PDF, flexbox/grid
    // on the web). No attribute is required.
    tags.insert(
        "columns".to_string(),
        Schema {
            render: None,
            children: None,
            attributes: Some({
                let mut attrs = HashMap::new();
                attrs.insert(
                    "widths".to_string(),
                    SchemaAttribute {
                        attr_type: Some(vec![ValidationType::String, ValidationType::Array]),
                        render: None,
                        default: None,
                        required: false,
                        description: Some(
                            "Relative column widths, e.g. \"2 1\" or [2, 1]; equal if omitted"
                                .to_string(),
                        ),
                    },
                );
                attrs.insert(
                    "gap".to_string(),
                    SchemaAttribute {
                        attr_type: Some(vec![ValidationType::Number, ValidationType::String]),
                        render: None,
                        default: None,
                        required: false,
                        description: Some(
                            "Space between columns, in points (default 16)".to_string(),
                        ),
                    },
                );
                attrs.insert(
                    "align".to_string(),
                    SchemaAttribute {
                        attr_type: Some(vec![ValidationType::String]),
                        render: None,
                        default: None,
                        required: false,
                        description: Some(
                            "Horizontal alignment of each column's content: left (default) | center | right"
                                .to_string(),
                        ),
                    },
                );
                attrs.insert(
                    "background".to_string(),
                    SchemaAttribute {
                        attr_type: Some(vec![ValidationType::String]),
                        render: None,
                        default: None,
                        required: false,
                        description: Some(
                            "Panel fill behind the columns — any CSS colour, e.g. \"#f9f9f9\""
                                .to_string(),
                        ),
                    },
                );
                attrs
            }),
            self_closing: false,
            inline: false,
            description: Some("Side-by-side columns".to_string()),
        },
    );

    // ── grid ────────────────────────────────────────────────────────────
    // A responsive grid: cells (list items, or blank-line-separated blocks)
    // reflow into as many equal columns as fit at `min` width, wrapping into
    // rows — a pure layout primitive renderers map to their medium (CSS
    // `repeat(auto-fill, minmax(min, 1fr))` on the web, a wrapped table in
    // PDF). Use `{% columns %}` instead when you want a fixed single row.
    tags.insert(
        "grid".to_string(),
        Schema {
            render: None,
            children: None,
            attributes: Some({
                let mut attrs = HashMap::new();
                attrs.insert(
                    "min".to_string(),
                    SchemaAttribute {
                        attr_type: Some(vec![ValidationType::Number, ValidationType::String]),
                        render: None,
                        default: None,
                        required: false,
                        description: Some(
                            "Minimum column width in points; the grid fits as many equal columns as the width allows (default 120)"
                                .to_string(),
                        ),
                    },
                );
                attrs.insert(
                    "gap".to_string(),
                    SchemaAttribute {
                        attr_type: Some(vec![ValidationType::Number, ValidationType::String]),
                        render: None,
                        default: None,
                        required: false,
                        description: Some(
                            "Space between cells, in points (default 16)".to_string(),
                        ),
                    },
                );
                attrs.insert(
                    "align".to_string(),
                    SchemaAttribute {
                        attr_type: Some(vec![ValidationType::String]),
                        render: None,
                        default: None,
                        required: false,
                        description: Some(
                            "Horizontal alignment of each cell's content: left (default) | center | right"
                                .to_string(),
                        ),
                    },
                );
                attrs.insert(
                    "background".to_string(),
                    SchemaAttribute {
                        attr_type: Some(vec![ValidationType::String]),
                        render: None,
                        default: None,
                        required: false,
                        description: Some(
                            "Panel fill behind the grid — any CSS colour, e.g. \"#f9f9f9\""
                                .to_string(),
                        ),
                    },
                );
                attrs
            }),
            self_closing: false,
            inline: false,
            description: Some("Responsive grid — cells reflow into as many equal columns as fit".to_string()),
        },
    );

    // ── swatch ──────────────────────────────────────────────────────────
    // A block colour bar / chip — solid (`color`) or a linear gradient
    // (`gradient`). The block-level sibling of the inline `{% color %}`;
    // renderers realise it as a filled box (CSS background on the web).
    // Handy for legends, status keys, and indicator bars.
    tags.insert(
        "swatch".to_string(),
        Schema {
            render: None,
            children: None,
            attributes: Some({
                let mut attrs = HashMap::new();
                attrs.insert(
                    "color".to_string(),
                    SchemaAttribute {
                        attr_type: Some(vec![ValidationType::String]),
                        render: None,
                        default: None,
                        required: false,
                        description: Some(
                            "Solid fill — any CSS colour, e.g. \"#ff0000\"".to_string(),
                        ),
                    },
                );
                attrs.insert(
                    "gradient".to_string(),
                    SchemaAttribute {
                        attr_type: Some(vec![ValidationType::String]),
                        render: None,
                        default: None,
                        required: false,
                        description: Some(
                            "Linear gradient fill: \"[Ndeg,] stop, stop, …\" — each stop a CSS colour (or transparent) with an optional NN% position; wins over color"
                                .to_string(),
                        ),
                    },
                );
                attrs.insert(
                    "height".to_string(),
                    SchemaAttribute {
                        attr_type: Some(vec![ValidationType::Number, ValidationType::String]),
                        render: None,
                        default: None,
                        required: false,
                        description: Some("Bar height in points (default 6)".to_string()),
                    },
                );
                attrs.insert(
                    "radius".to_string(),
                    SchemaAttribute {
                        attr_type: Some(vec![ValidationType::Number, ValidationType::String]),
                        render: None,
                        default: None,
                        required: false,
                        description: Some("Corner radius in points (default 2)".to_string()),
                    },
                );
                attrs
            }),
            self_closing: true,
            inline: false,
            description: Some("Colour swatch / bar — solid or linear-gradient fill".to_string()),
        },
    );

    // ── pagebreak ───────────────────────────────────────────────────────
    // Force the following content onto a new PDF page. Self-closing;
    // no attributes. Web renderers drop this tag as a no-op.
    tags.insert(
        "pagebreak".to_string(),
        Schema {
            render: None,
            children: None,
            attributes: None,
            self_closing: true,
            inline: false,
            description: Some(
                "Force a PDF page break; no-op in non-paginated renderers".to_string(),
            ),
        },
    );

    // ── imagegrid / gridimage ───────────────────────────────────────────
    // Side-by-side illustrated callouts (MiR manuals). `{% imagegrid %}`
    // wraps one or more self-closing `{% gridimage id=… headline=…
    // bodytext=… /%}` cells; the PDF renderer paints them as equal
    // columns on a light grey panel (mirrors `.imagegrid` / `.gridimage`
    // in `public/globals.css`).
    tags.insert(
        "imagegrid".to_string(),
        Schema {
            render: None,
            children: None,
            attributes: None,
            self_closing: false,
            inline: false,
            description: Some("Row of gridimage cells on a light grey background".to_string()),
        },
    );
    tags.insert(
        "gridimage".to_string(),
        Schema {
            render: None,
            children: None,
            attributes: Some({
                let mut attrs = HashMap::new();
                attrs.insert(
                    "id".to_string(),
                    SchemaAttribute {
                        attr_type: Some(vec![ValidationType::String]),
                        render: None,
                        default: None,
                        required: true,
                        description: Some("Asset id for the illustration".to_string()),
                    },
                );
                attrs.insert(
                    "alt".to_string(),
                    SchemaAttribute {
                        attr_type: Some(vec![ValidationType::String]),
                        render: None,
                        default: Some(Scalar::String(String::new())),
                        required: false,
                        description: Some("Accessibility text for the illustration".to_string()),
                    },
                );
                attrs.insert(
                    "headline".to_string(),
                    SchemaAttribute {
                        attr_type: Some(vec![ValidationType::String]),
                        render: None,
                        default: Some(Scalar::String(String::new())),
                        required: false,
                        description: Some("Bold title under the illustration".to_string()),
                    },
                );
                attrs.insert(
                    "bodytext".to_string(),
                    SchemaAttribute {
                        attr_type: Some(vec![ValidationType::String]),
                        render: None,
                        default: Some(Scalar::String(String::new())),
                        required: false,
                        description: Some("Body copy under the headline".to_string()),
                    },
                );
                attrs
            }),
            self_closing: true,
            inline: false,
            description: Some(
                "One imagegrid cell: illustration + headline + body text".to_string(),
            ),
        },
    );

    // ── chip ────────────────────────────────────────────────────────────
    // Inline colour chip / dot — the inline sibling of the block
    // `{% swatch %}`. A small filled mark (circle by default, square via
    // `shape`) tinted with `color`, flowing within running text. Solid
    // only; renderers realise it as an inline styled span on the web.
    tags.insert(
        "chip".to_string(),
        Schema {
            render: None,
            children: None,
            attributes: Some({
                let mut attrs = HashMap::new();
                attrs.insert(
                    "color".to_string(),
                    SchemaAttribute {
                        attr_type: Some(vec![ValidationType::String]),
                        render: None,
                        default: None,
                        required: false,
                        description: Some(
                            "Chip fill — any CSS colour, e.g. \"#ff0000\"".to_string(),
                        ),
                    },
                );
                attrs.insert(
                    "shape".to_string(),
                    SchemaAttribute {
                        attr_type: Some(vec![ValidationType::String]),
                        render: None,
                        default: None,
                        required: false,
                        description: Some("Chip shape: circle (default) | square".to_string()),
                    },
                );
                attrs
            }),
            self_closing: true,
            inline: true,
            description: Some("Inline colour chip / dot".to_string()),
        },
    );

    // ── ref ─────────────────────────────────────────────────────────────
    // Inline cross-document reference / mention. Points at another
    // publication by its immutable `document` number (or Adeptus `uuid`);
    // Adeptus resolves it to the target's title + URL at render time. A
    // structural tag — a renderer without Adeptus (markdoc-pdf) shows a
    // placeholder link with the reference.
    tags.insert(
        "ref".to_string(),
        Schema {
            render: None,
            children: None,
            attributes: Some({
                let mut attrs = HashMap::new();
                attrs.insert(
                    "document".to_string(),
                    SchemaAttribute {
                        attr_type: Some(vec![ValidationType::String]),
                        render: None,
                        default: None,
                        required: false,
                        description: Some("Target publication's document number".to_string()),
                    },
                );
                attrs.insert(
                    "uuid".to_string(),
                    SchemaAttribute {
                        attr_type: Some(vec![ValidationType::String]),
                        render: None,
                        default: None,
                        required: false,
                        description: Some(
                            "Target's Adeptus UUID (alternative to `document`)".to_string(),
                        ),
                    },
                );
                attrs.insert(
                    "label".to_string(),
                    SchemaAttribute {
                        attr_type: Some(vec![ValidationType::String]),
                        render: None,
                        default: None,
                        required: false,
                        description: Some(
                            "Optional visible link text (defaults to the target title)".to_string(),
                        ),
                    },
                );
                attrs
            }),
            self_closing: true,
            inline: true,
            description: Some("Inline cross-document reference".to_string()),
        },
    );

    // ── document-history ─────────────────────────────────────────────────
    // Renders the frontmatter `documentHistory` (version / date / description
    // entries) as a table where the tag sits. Block, self-closing; the
    // renderer reads the frontmatter and draws the table. `title` overrides
    // the heading (default "Document history"; an empty string omits it).
    tags.insert(
        "document-history".to_string(),
        Schema {
            render: None,
            children: None,
            attributes: Some({
                let mut attrs = HashMap::new();
                attrs.insert(
                    "title".to_string(),
                    SchemaAttribute {
                        attr_type: Some(vec![ValidationType::String]),
                        render: None,
                        default: None,
                        required: false,
                        description: Some(
                            "Heading above the table (default \"Document history\"; empty omits it)"
                                .to_string(),
                        ),
                    },
                );
                attrs
            }),
            self_closing: true,
            inline: false,
            description: Some("Document revision-history table from frontmatter".to_string()),
        },
    );

    // ── qr ──────────────────────────────────────────────────────────────
    // A QR code generated from `value` (any string — a URL, a document
    // number, arbitrary text). A structural, output-agnostic tag: PDF draws
    // the matrix, the web can render an <img> / <svg>. The other attributes
    // are presentation hints renderers may honour.
    tags.insert(
        "qr".to_string(),
        Schema {
            render: None,
            children: None,
            attributes: Some({
                let mut attrs = HashMap::new();
                attrs.insert(
                    "value".to_string(),
                    SchemaAttribute {
                        attr_type: Some(vec![ValidationType::String, ValidationType::Number]),
                        render: None,
                        default: None,
                        required: true,
                        description: Some(
                            "The data to encode — a URL, document number, or any text".to_string(),
                        ),
                    },
                );
                attrs.insert(
                    "size".to_string(),
                    SchemaAttribute {
                        attr_type: Some(vec![ValidationType::Number, ValidationType::String]),
                        render: None,
                        default: None,
                        required: false,
                        description: Some(
                            "Rendered side length in points (default 72)".to_string(),
                        ),
                    },
                );
                attrs.insert(
                    "ecl".to_string(),
                    SchemaAttribute {
                        attr_type: Some(vec![ValidationType::String]),
                        render: None,
                        default: None,
                        required: false,
                        description: Some(
                            "Error-correction level: low | medium (default) | quartile | high"
                                .to_string(),
                        ),
                    },
                );
                attrs.insert(
                    "align".to_string(),
                    SchemaAttribute {
                        attr_type: Some(vec![ValidationType::String]),
                        render: None,
                        default: None,
                        required: false,
                        description: Some(
                            "Horizontal placement: left (default) | center | right".to_string(),
                        ),
                    },
                );
                attrs.insert(
                    "color".to_string(),
                    SchemaAttribute {
                        attr_type: Some(vec![ValidationType::String]),
                        render: None,
                        default: None,
                        required: false,
                        description: Some(
                            "Module (foreground) colour, any CSS colour (default black)"
                                .to_string(),
                        ),
                    },
                );
                attrs.insert(
                    "background".to_string(),
                    SchemaAttribute {
                        attr_type: Some(vec![ValidationType::String]),
                        render: None,
                        default: None,
                        required: false,
                        description: Some(
                            "Field (background) colour, any CSS colour (default white)".to_string(),
                        ),
                    },
                );
                attrs.insert(
                    "quiet_zone".to_string(),
                    SchemaAttribute {
                        attr_type: Some(vec![ValidationType::Number, ValidationType::String]),
                        render: None,
                        default: None,
                        required: false,
                        description: Some(
                            "Margin around the code, in modules (default 4)".to_string(),
                        ),
                    },
                );
                attrs
            }),
            self_closing: true,
            inline: false,
            description: Some("QR code generated from a value".to_string()),
        },
    );

    // ── gate ────────────────────────────────────────────────────────────
    // A free-reading cut-off (a newspaper-style metered wall). Everything
    // BEFORE the gate is the free preview served to everyone; everything
    // AFTER it is gated. Adeptus serves the below-gate section only to a
    // viewer who holds the required `access_level` (a value from the
    // document's accessLevel vocabulary) — readers without it receive only
    // the content above the gate. A digital-rendering marker: markdoc-pdf
    // strips it (a printed PDF always carries the whole document).
    tags.insert(
        "gate".to_string(),
        Schema {
            render: None,
            children: None,
            attributes: Some({
                let mut attrs = HashMap::new();
                attrs.insert(
                    "access_level".to_string(),
                    SchemaAttribute {
                        attr_type: Some(vec![ValidationType::String]),
                        render: None,
                        default: None,
                        required: false,
                        description: Some(
                            "Access level required to read below the gate (from the document's accessLevel vocabulary); omit for the platform's default gated tier".to_string(),
                        ),
                    },
                );
                attrs
            }),
            self_closing: true,
            inline: false,
            description: Some(
                "Free-reading cut-off; content below is gated by accessLevel".to_string(),
            ),
        },
    );

    // ── ad ──────────────────────────────────────────────────────────────
    // An ad slot: a location where a digital renderer may inject an
    // advertisement, carrying targeting metadata. Relevancy layers on the
    // document's frontmatter `tags` / `products`, refined per slot by
    // `categories` / `keywords`. A digital-rendering marker: markdoc-pdf
    // strips it (no ads in print).
    tags.insert(
        "ad".to_string(),
        Schema {
            render: None,
            children: None,
            attributes: Some({
                let mut attrs = HashMap::new();
                for (name, desc) in [
                    ("slot", "Stable slot name, for fill tracking / analytics"),
                    (
                        "format",
                        "Ad format / size hint, e.g. leaderboard | inline | square",
                    ),
                    ("categories", "Comma-separated ad categories for targeting"),
                    (
                        "keywords",
                        "Extra relevancy keywords, beyond the document's tags / products",
                    ),
                ] {
                    attrs.insert(
                        name.to_string(),
                        SchemaAttribute {
                            attr_type: Some(vec![ValidationType::String]),
                            render: None,
                            default: None,
                            required: false,
                            description: Some(desc.to_string()),
                        },
                    );
                }
                attrs
            }),
            self_closing: true,
            inline: false,
            description: Some("Ad slot with targeting metadata (digital renderers)".to_string()),
        },
    );

    // ── float ───────────────────────────────────────────────────────────
    // Float an image to one side with content wrapping around it. With
    // inline `{% media side=… /%}` markers in the body it becomes a
    // multi-image "magazine" wrap (several floats anchored where they
    // appear in the prose). Units are interpreted by each renderer
    // (points in PDF, CSS length on the web); page-break behaviour is a
    // renderer concern, not part of this contract.
    tags.insert(
        "float".to_string(),
        Schema {
            render: None,
            children: None,
            attributes: Some({
                let mut attrs = HashMap::new();
                attrs.insert(
                    "side".to_string(),
                    SchemaAttribute {
                        attr_type: Some(vec![ValidationType::String]),
                        render: None,
                        default: None,
                        required: false,
                        description: Some(
                            "Which side the image floats to: left (default) or right".to_string(),
                        ),
                    },
                );
                attrs.insert(
                    "width".to_string(),
                    SchemaAttribute {
                        attr_type: Some(vec![ValidationType::Number, ValidationType::String]),
                        render: None,
                        default: None,
                        required: false,
                        description: Some(
                            "Image width — a fraction \u{2264} 1 of the column, a length, or a \"NN%\" string (default 40%)"
                                .to_string(),
                        ),
                    },
                );
                attrs.insert(
                    "gap".to_string(),
                    SchemaAttribute {
                        attr_type: Some(vec![ValidationType::Number, ValidationType::String]),
                        render: None,
                        default: None,
                        required: false,
                        description: Some(
                            "Space between the image and the wrapped content (default 14)"
                                .to_string(),
                        ),
                    },
                );
                attrs
            }),
            self_closing: false,
            inline: false,
            description: Some("Float an image with content wrapping around it".to_string()),
        },
    );

    // ── color / c ───────────────────────────────────────────────────────
    // Inline coloured text span. `c` is a shorthand alias.
    let color_schema = |alias: bool| Schema {
        render: None,
        children: None,
        attributes: Some({
            let mut attrs = HashMap::new();
            attrs.insert(
                "value".to_string(),
                SchemaAttribute {
                    attr_type: Some(vec![ValidationType::String]),
                    render: None,
                    default: None,
                    required: false,
                    description: Some(
                        "Colour: a #rgb / #rrggbb hex value or a named colour".to_string(),
                    ),
                },
            );
            attrs
        }),
        self_closing: false,
        inline: true,
        description: Some(if alias {
            "Inline coloured text span (alias of `color`)".to_string()
        } else {
            "Inline coloured text span".to_string()
        }),
    };
    tags.insert("color".to_string(), color_schema(false));
    tags.insert("c".to_string(), color_schema(true));

    // ── list ────────────────────────────────────────────────────────────
    // A list with a custom marker style, wrapping ordinary list items.
    tags.insert(
        "list".to_string(),
        Schema {
            render: None,
            children: None,
            attributes: Some({
                let mut attrs = HashMap::new();
                attrs.insert(
                    "type".to_string(),
                    SchemaAttribute {
                        attr_type: Some(vec![ValidationType::String]),
                        render: None,
                        default: None,
                        required: false,
                        description: Some("Marker style: checkmark | dash | none".to_string()),
                    },
                );
                attrs
            }),
            self_closing: false,
            inline: false,
            description: Some("List with a custom marker style".to_string()),
        },
    );

    // ── caption ─────────────────────────────────────────────────────────
    // Caption text for an adjacent figure or table. Placed above (default)
    // or below its target; the media tag's own `caption` attribute is the
    // inline alternative for a single asset.
    tags.insert(
        "caption".to_string(),
        Schema {
            render: None,
            children: None,
            attributes: Some({
                let mut attrs = HashMap::new();
                attrs.insert(
                    "position".to_string(),
                    SchemaAttribute {
                        attr_type: Some(vec![ValidationType::String]),
                        render: None,
                        default: None,
                        required: false,
                        description: Some(
                            "Placement relative to the figure/table: above (default) or below"
                                .to_string(),
                        ),
                    },
                );
                attrs.insert(
                    "color".to_string(),
                    SchemaAttribute {
                        attr_type: Some(vec![ValidationType::String]),
                        render: None,
                        default: None,
                        required: false,
                        description: Some("Colour for the caption text".to_string()),
                    },
                );
                attrs
            }),
            self_closing: false,
            inline: false,
            description: Some("Caption for an adjacent figure or table".to_string()),
        },
    );

    // ── input ───────────────────────────────────────────────────────────
    // A form input field. Attributes mirror the HTML input attributes so
    // they translate across renderers: the web maps them to a native
    // `<input>` with full constraint validation; PDF draws a print form box
    // (label + ruled field) since PDF/A forbids the JavaScript that dynamic
    // field validation would need. Author-time validation here only checks
    // the tag itself (attribute types); the field's own required/min/max
    // rules are enforced when a user fills the rendered form, not in markdoc.
    tags.insert(
        "input".to_string(),
        Schema {
            render: None,
            children: None,
            attributes: Some({
                let mut attrs = HashMap::new();
                attrs.insert(
                    "name".to_string(),
                    SchemaAttribute {
                        attr_type: Some(vec![ValidationType::String]),
                        render: None,
                        default: None,
                        required: true,
                        description: Some("Field identifier".to_string()),
                    },
                );
                attrs.insert(
                    "type".to_string(),
                    SchemaAttribute {
                        attr_type: Some(vec![ValidationType::String]),
                        render: None,
                        default: None,
                        required: false,
                        description: Some(
                            "Input type: text (default) | number | email | tel | url | date | checkbox"
                                .to_string(),
                        ),
                    },
                );
                attrs.insert(
                    "label".to_string(),
                    SchemaAttribute {
                        attr_type: Some(vec![ValidationType::String]),
                        render: None,
                        default: None,
                        required: false,
                        description: Some("Human-readable label shown beside the field".to_string()),
                    },
                );
                attrs.insert(
                    "required".to_string(),
                    SchemaAttribute {
                        attr_type: Some(vec![ValidationType::Boolean]),
                        render: None,
                        default: None,
                        required: false,
                        description: Some("Whether a value must be supplied".to_string()),
                    },
                );
                attrs.insert(
                    "value".to_string(),
                    SchemaAttribute {
                        attr_type: Some(vec![ValidationType::String, ValidationType::Number]),
                        render: None,
                        default: None,
                        required: false,
                        description: Some("Default / pre-filled value".to_string()),
                    },
                );
                attrs.insert(
                    "placeholder".to_string(),
                    SchemaAttribute {
                        attr_type: Some(vec![ValidationType::String]),
                        render: None,
                        default: None,
                        required: false,
                        description: Some("Placeholder hint shown in an empty field".to_string()),
                    },
                );
                attrs.insert(
                    "min".to_string(),
                    SchemaAttribute {
                        attr_type: Some(vec![ValidationType::Number, ValidationType::String]),
                        render: None,
                        default: None,
                        required: false,
                        description: Some("Minimum value (number / date types)".to_string()),
                    },
                );
                attrs.insert(
                    "max".to_string(),
                    SchemaAttribute {
                        attr_type: Some(vec![ValidationType::Number, ValidationType::String]),
                        render: None,
                        default: None,
                        required: false,
                        description: Some("Maximum value (number / date types)".to_string()),
                    },
                );
                attrs.insert(
                    "minlength".to_string(),
                    SchemaAttribute {
                        attr_type: Some(vec![ValidationType::Number]),
                        render: None,
                        default: None,
                        required: false,
                        description: Some("Minimum length in characters (text types)".to_string()),
                    },
                );
                attrs.insert(
                    "maxlength".to_string(),
                    SchemaAttribute {
                        attr_type: Some(vec![ValidationType::Number]),
                        render: None,
                        default: None,
                        required: false,
                        description: Some("Maximum length in characters (text types)".to_string()),
                    },
                );
                attrs.insert(
                    "pattern".to_string(),
                    SchemaAttribute {
                        attr_type: Some(vec![ValidationType::String]),
                        render: None,
                        default: None,
                        required: false,
                        description: Some("Regular expression the value must match".to_string()),
                    },
                );
                attrs
            }),
            self_closing: true,
            inline: false,
            description: Some(
                "Form input field (HTML input on the web; a print form box in PDF)".to_string(),
            ),
        },
    );

    tags
}

/// Hand-written `Default` for `Config` so a default-constructed config
/// already knows about the standard markdown nodes and built-in tags.
/// Lives in this module (rather than `types.rs`) to avoid an import
/// cycle: `tags.rs` → `types.rs` → `tags.rs`.
impl Default for Config {
    fn default() -> Self {
        Self {
            nodes: default_nodes(),
            tags: default_tags(),
            variables: HashMap::new(),
            functions: HashMap::new(),
            partials: HashMap::new(),
        }
    }
}

pub fn truthy(value: &Scalar) -> bool {
    match value {
        Scalar::Null => false,
        Scalar::Boolean(b) => *b,
        Scalar::Number(n) => *n != 0.0,
        Scalar::String(s) => !s.is_empty(),
        Scalar::Array(a) => !a.is_empty(),
        Scalar::Object(o) => !o.is_empty(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parser::parse;
    use crate::transformer::transform;
    use crate::types::RenderableTreeNode;

    fn find_tag<'a>(node: &'a RenderableTreeNode, name: &str) -> Option<&'a Tag> {
        match node {
            RenderableTreeNode::Tag(t) if t.name == name => Some(t),
            RenderableTreeNode::Tag(t) => t.children.iter().find_map(|c| find_tag(c, name)),
            _ => None,
        }
    }

    #[test]
    fn default_config_has_callout_tag_registered() {
        let cfg = Config::default();
        assert!(cfg.tags.contains_key("callout"));
        assert!(cfg.tags.contains_key("tag"));
        assert!(cfg.tags.contains_key("tagref"));
        // Standard markdown nodes are also registered now.
        assert!(cfg.nodes.contains_key("heading"));
        assert!(cfg.nodes.contains_key("paragraph"));
    }

    #[test]
    fn callout_without_explicit_type_gets_default_note() {
        let src = "{% callout %}body{% /callout %}";
        let doc = parse(src, None).unwrap();
        let rendered = transform(&doc, &Config::default()).unwrap();
        let callout = find_tag(&rendered, "callout").expect("callout in tree");
        assert_eq!(
            callout.attributes.get("type"),
            Some(&Scalar::String("note".to_string())),
            "expected default type=note, attrs were {:?}",
            callout.attributes
        );
    }

    #[test]
    fn callout_explicit_type_overrides_default() {
        let src = r#"{% callout type="warning" %}body{% /callout %}"#;
        let doc = parse(src, None).unwrap();
        let rendered = transform(&doc, &Config::default()).unwrap();
        let callout = find_tag(&rendered, "callout").unwrap();
        assert_eq!(
            callout.attributes.get("type"),
            Some(&Scalar::String("warning".to_string()))
        );
    }

    #[test]
    fn heading_level_attribute_is_dropped_under_default_config() {
        // The heading schema marks `level` as render: false. With the
        // populated default config, the renderable tree should not carry
        // level=N as an HTML attribute (it survives only as the element
        // name h1..h6).
        let src = "# Hello";
        let doc = parse(src, None).unwrap();
        let rendered = transform(&doc, &Config::default()).unwrap();
        let h1 = find_tag(&rendered, "h1").expect("h1 in tree");
        assert!(
            !h1.attributes.contains_key("level"),
            "level should be dropped, attrs were {:?}",
            h1.attributes
        );
    }

    #[test]
    fn truthy_classifies_scalars_as_expected() {
        assert!(!truthy(&Scalar::Null));
        assert!(!truthy(&Scalar::Boolean(false)));
        assert!(truthy(&Scalar::Boolean(true)));
        assert!(!truthy(&Scalar::Number(0.0)));
        assert!(truthy(&Scalar::Number(1.0)));
        assert!(!truthy(&Scalar::String(String::new())));
        assert!(truthy(&Scalar::String("x".into())));
        assert!(!truthy(&Scalar::Array(vec![])));
        assert!(truthy(&Scalar::Array(vec![Scalar::Null])));
    }

    #[test]
    fn default_config_has_media_tag_registered() {
        let cfg = Config::default();
        let media = cfg.tags.get("media").expect("media tag registered");
        let attrs = media.attributes.as_ref().unwrap();
        // `src` is optional now — a reference may instead carry `id`.
        assert!(!attrs.get("src").unwrap().required);
        assert!(!attrs.get("id").unwrap().required);
        // `img` is registered as an alias of `media`.
        assert!(cfg.tags.contains_key("img"));
        assert!(!attrs.get("alt").unwrap().required);
        assert!(!attrs.get("title").unwrap().required);
        assert!(!attrs.get("caption").unwrap().required);
        assert!(!attrs.get("kind").unwrap().required);
        // Gained for the `{% float %}` anchored/magazine mode.
        assert!(!attrs.get("side").unwrap().required);
        assert!(!attrs.get("width").unwrap().required);
        assert!(!attrs.get("size").unwrap().required);
    }

    #[test]
    fn default_config_has_layout_tags_registered() {
        let cfg = Config::default();
        for name in [
            "columns",
            "grid",
            "swatch",
            "chip",
            "qr",
            "float",
            "color",
            "c",
            "list",
            "caption",
            "pagebreak",
        ] {
            assert!(cfg.tags.contains_key(name), "{name} tag registered");
        }
        assert!(!cfg.tags["pagebreak"].inline);
        assert!(cfg.tags["pagebreak"].self_closing);
        // Float declares side/width/gap, all optional.
        let fattrs = cfg.tags["float"].attributes.as_ref().unwrap();
        for a in ["side", "width", "gap"] {
            assert!(!fattrs.get(a).unwrap().required, "float.{a} optional");
        }
        // columns and grid are block containers, color is an inline span.
        assert!(!cfg.tags["columns"].inline);
        assert!(!cfg.tags["grid"].inline);
        assert!(cfg.tags["color"].inline);
        // columns carries widths plus the align / background cosmetics.
        let cattrs = cfg.tags["columns"].attributes.as_ref().unwrap();
        for a in ["widths", "align", "background"] {
            assert!(cattrs.contains_key(a), "columns.{a} declared");
        }
        // grid carries the reflow `min` plus align / background.
        let gattrs = cfg.tags["grid"].attributes.as_ref().unwrap();
        for a in ["min", "gap", "align", "background"] {
            assert!(gattrs.contains_key(a), "grid.{a} declared");
            assert!(!gattrs.get(a).unwrap().required, "grid.{a} optional");
        }
        // swatch is a self-closing block with colour / gradient fill knobs.
        assert!(!cfg.tags["swatch"].inline);
        assert!(cfg.tags["swatch"].self_closing);
        let sattrs = cfg.tags["swatch"].attributes.as_ref().unwrap();
        for a in ["color", "gradient", "height", "radius"] {
            assert!(sattrs.contains_key(a), "swatch.{a} declared");
            assert!(!sattrs.get(a).unwrap().required, "swatch.{a} optional");
        }
        // chip is the inline, self-closing sibling with colour / shape.
        assert!(cfg.tags["chip"].inline);
        assert!(cfg.tags["chip"].self_closing);
        let chattrs = cfg.tags["chip"].attributes.as_ref().unwrap();
        for a in ["color", "shape"] {
            assert!(chattrs.contains_key(a), "chip.{a} declared");
            assert!(!chattrs.get(a).unwrap().required, "chip.{a} optional");
        }
        // qr is a self-closing block; `value` is required, the rest optional.
        assert!(!cfg.tags["qr"].inline);
        assert!(cfg.tags["qr"].self_closing);
        let qattrs = cfg.tags["qr"].attributes.as_ref().unwrap();
        assert!(qattrs["value"].required, "qr.value required");
        for a in ["size", "ecl", "align", "color", "background", "quiet_zone"] {
            assert!(qattrs.contains_key(a), "qr.{a} declared");
            assert!(!qattrs.get(a).unwrap().required, "qr.{a} optional");
        }
        // gate / ad are self-closing digital-rendering markers; attrs all optional.
        for name in ["gate", "ad"] {
            assert!(!cfg.tags[name].inline, "{name} is a block tag");
            assert!(cfg.tags[name].self_closing, "{name} self-closing");
        }
        let gattrs = cfg.tags["gate"].attributes.as_ref().unwrap();
        assert!(
            !gattrs["access_level"].required,
            "gate.access_level optional"
        );
        let adattrs = cfg.tags["ad"].attributes.as_ref().unwrap();
        for a in ["slot", "format", "categories", "keywords"] {
            assert!(adattrs.contains_key(a), "ad.{a} declared");
            assert!(!adattrs.get(a).unwrap().required, "ad.{a} optional");
        }
    }

    #[test]
    fn layout_tags_validate_without_error() {
        // A representative document using the layout tags must not raise any
        // error-level validation issues against the default config.
        let src = "\
{% columns widths=\"2 1\" gap=16 align=\"center\" background=\"#f9f9f9\" %}\n* a\n* b\n{% /columns %}\n\n\
{% grid min=120 gap=16 align=\"center\" %}\n* a\n* b\n* c\n{% /grid %}\n\n\
{% swatch gradient=\"90deg, #fd7e14, #ffffff, #fd7e14\" height=5 /%}\n\n\
{% qr value=\"HELLO-600123\" size=64 ecl=\"quartile\" /%}\n\n\
{% float side=\"left\" width=120 %}\n{% media src=\"x.png\" /%}\n\ntext\n{% /float %}\n\n\
Inline {% color value=\"#c026d3\" %}pink{% /color %} and a {% chip color=\"#ff0000\" /%} chip.\n";
        let doc = parse(src, None).unwrap();
        let errors = crate::validator::validate(&doc, &Config::default());
        let hard: Vec<_> = errors
            .iter()
            .filter(|e| matches!(e.level, crate::types::ValidationLevel::Error))
            .collect();
        assert!(hard.is_empty(), "unexpected validation errors: {hard:?}");
    }

    #[test]
    fn input_tag_schema_and_validation() {
        let cfg = Config::default();
        let input = cfg.tags.get("input").expect("input tag registered");
        let attrs = input.attributes.as_ref().unwrap();
        assert!(attrs["name"].required, "name is required");
        for a in [
            "type",
            "required",
            "min",
            "max",
            "minlength",
            "maxlength",
            "pattern",
        ] {
            assert!(!attrs[a].required, "input.{a} optional");
        }
        assert!(input.self_closing);

        // A well-formed field validates clean.
        let ok = parse(
            "{% input name=\"qty\" type=\"number\" required=true min=1 max=100 maxlength=3 /%}\n",
            None,
        )
        .unwrap();
        assert!(
            crate::validator::validate(&ok, &cfg)
                .iter()
                .all(|e| !matches!(e.level, crate::types::ValidationLevel::Error)),
            "valid input should not error"
        );

        // Missing the required `name` is flagged.
        let bad = parse("{% input type=\"text\" /%}\n", None).unwrap();
        assert!(
            crate::validator::validate(&bad, &cfg)
                .iter()
                .any(|e| e.id == "missing-attribute"),
            "missing name should be flagged"
        );
    }

    #[test]
    fn media_tag_with_complex_uri_parses_intact() {
        // The src URI carries Arca transformation params (size, focus,
        // effects) verbatim. markdoc must not interpret or mutate them.
        let src = r#"{% media src="arca://abc-123?w=400&focus=0.5,0.3&blur-edge=8" alt="Front panel" kind="image" /%}"#;
        let doc = parse(src, None).unwrap();
        let rendered = transform(&doc, &Config::default()).unwrap();
        let media = find_tag(&rendered, "media").expect("media in tree");
        assert_eq!(
            media.attributes.get("src"),
            Some(&Scalar::String(
                "arca://abc-123?w=400&focus=0.5,0.3&blur-edge=8".into()
            ))
        );
        assert_eq!(
            media.attributes.get("alt"),
            Some(&Scalar::String("Front panel".into()))
        );
        assert_eq!(
            media.attributes.get("kind"),
            Some(&Scalar::String("image".into()))
        );
    }

    #[test]
    fn media_accepts_id_instead_of_src() {
        // `src` is optional now — a reference may instead carry `id` — so
        // neither is flagged as missing. Holds for the `img` alias too.
        use crate::validator::validate;
        for src in [
            "{% media id=\"0a02bb82-1a68-4c5c-883f-406361e1235e\" /%}",
            "{% img id=\"0a02bb82-1a68-4c5c-883f-406361e1235e\" /%}",
        ] {
            let doc = parse(src, None).unwrap();
            let errors = validate(&doc, &Config::default());
            assert!(
                !errors.iter().any(|e| e.id == "missing-attribute"),
                "{src} should validate without a missing-attribute error, got: {errors:?}"
            );
        }
    }

    #[test]
    fn media_works_for_non_image_kinds() {
        // The same tag is used for all media types — only `kind` differs.
        for kind in &["image", "vector", "animation", "video", "audio", "model"] {
            let src = format!(
                r#"{{% media src="arca://x" kind="{kind}" /%}}"#,
                kind = kind
            );
            let doc = parse(&src, None).unwrap();
            let rendered = transform(&doc, &Config::default()).unwrap();
            let media = find_tag(&rendered, "media")
                .unwrap_or_else(|| panic!("media not found for kind={kind}"));
            assert_eq!(
                media.attributes.get("kind"),
                Some(&Scalar::String((*kind).into())),
                "kind={kind}"
            );
        }
    }
}

# Blueprint

Decodes PDFs and extracts structured data for automated forms conversion. 

## Usage

```
# Analyze document structure
cargo run -- input/AAAI_019_DE.pdf

# Render with labeled field groups (blue overlays)
cargo run -- input/AAAI_019_DE.pdf --render-labelled

# Render plain document (no annotations)
cargo run -- input/AAAI_019_DE.pdf --render-plain

# Render with field annotations (red overlays)
cargo run -- input/AAAI_019_DE.pdf --render-annotated
```
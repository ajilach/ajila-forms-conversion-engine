# Blueprint

Decodes PDFs and extracts structured data for automated forms conversion. 

## Usage

```
# Basic analysis (processes the PDF, no file output)
cargo run -- input/AAAI_019_DE.pdf

# Export structured JSON
cargo run -- input/AAAI_019_DE.pdf --structured

# Export standalone HTML
cargo run -- input/AAAI_019_DE.pdf --html

# Export AEM Adaptive Forms package (XFA PDFs only)
cargo run -- input/AAAI_019_DE.pdf --aem

# Render images (modes: plain, labelled, annotated; repeatable)
cargo run -- input/AAAI_019_DE.pdf --render plain --render labelled

# Custom render scale (default 1.5)
cargo run -- input/AAAI_019_DE.pdf --render plain --scale 2.0

# Multilingual merge (pass multiple language variants)
cargo run -- input/AAAI_019_DE.pdf input/AAAI_019_EN.pdf --structured --html

# Dump raw XFA XML and exit
cargo run -- input/AAAI_019_DE.pdf --dump-xfa

# Suppress verbose output
cargo run -- input/AAAI_019_DE.pdf --structured -q
```

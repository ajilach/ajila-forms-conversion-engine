# Context System Usage Examples

## Basic Usage

### 1. Process document with default language (German)

```bash
blueprint input/AAAI_019_DE.pdf --structured
```

Output in `AAAI_019_DE_merged.json`:
```json
{
  "context": {
    "language": "de"
  },
  "content": [...]
}
```

### 2. Process document with specific language

```bash
blueprint document.pdf --language en --structured
```

Output:
```json
{
  "context": {
    "language": "en"
  },
  "content": [...]
}
```

### 3. Enable single module

```bash
blueprint document.pdf --language de --module ubs --structured
```

Output:
```json
{
  "context": {
    "language": "de",
    "modules": {
      "enabled_modules": ["ubs"]
    }
  },
  "content": [...]
}
```

### 4. Enable multiple modules

```bash
blueprint document.pdf --language fr --module ubs --module custom --module validation --structured
```

Output:
```json
{
  "context": {
    "language": "fr",
    "modules": {
      "enabled_modules": ["ubs", "custom", "validation"]
    }
  },
  "content": [...]
}
```

## Combined with Rendering

### 5. Generate structured JSON and annotated image

```bash
blueprint document.pdf --render annotated --structured --language de
```

Generates:
- `document_default.annotated.png` - Annotated image
- `document_default.json` - Structured JSON with context
- `document_merged.json` - Merged structured JSON

### 6. Generate multiple render modes with modules

```bash
blueprint document.pdf \
  --render plain \
  --render labelled \
  --render annotated \
  --structured \
  --html \
  --language en \
  --module ubs \
  --scale 2.0
```

Generates:
- PNG images for each render mode at 2x scale
- Structured JSON with context (language: en, modules: [ubs])
- HTML output

## Quiet Mode

### 7. Process without verbose output

```bash
blueprint document.pdf --structured --language de --module ubs --quiet
```

Only errors and final results are shown.

## Integration Examples

### Python Integration

```python
import json
import subprocess

# Run blueprint and capture output
subprocess.run([
    "blueprint", 
    "document.pdf",
    "--structured",
    "--language", "de",
    "--module", "ubs",
    "--quiet"
])

# Load the result
with open("document_merged.json") as f:
    envelope = json.load(f)
    
# Access context
language = envelope["context"]["language"]
modules = envelope["context"]["modules"]["enabled_modules"]

# Access content
for node in envelope["content"]:
    if node["type"] == "heading":
        print(f"Heading: {node['content'][0]['content']}")
```

### Node.js Integration

```javascript
const { execSync } = require('child_process');
const fs = require('fs');

// Run blueprint
execSync('blueprint document.pdf --structured --language en --module ubs --quiet');

// Load result
const envelope = JSON.parse(fs.readFileSync('document_merged.json', 'utf8'));

// Access context
const { language, modules } = envelope.context;
console.log(`Language: ${language}`);
console.log(`Modules: ${modules.enabled_modules.join(', ')}`);

// Process content
envelope.content.forEach(node => {
  if (node.type === 'field') {
    console.log(`Field: ${node.name}`);
  }
});
```

## Module Development

### Creating a Custom Module

While the current implementation stores enabled module names in the context, future modules can enrich the context with custom data:

```rust
// Example: Future module implementation
use crate::context::{Context, ModuleData};
use std::collections::HashMap;

pub struct CustomModule;

impl CustomModule {
    pub fn enrich_context(&self, context: &mut Context, doc: &Document) {
        let mut stats = HashMap::new();
        
        // Collect statistics
        stats.insert("field_count".to_string(), 
            serde_json::Value::Number(doc.field_count().into()));
        stats.insert("heading_count".to_string(),
            serde_json::Value::Number(doc.heading_count().into()));
            
        // Add to context
        context.set_module_data("custom_stats", ModuleData::Object(stats));
    }
}
```

This would produce:
```json
{
  "context": {
    "language": "de",
    "modules": {
      "enabled_modules": ["custom"],
      "custom_stats": {
        "field_count": 42,
        "heading_count": 15
      }
    }
  },
  "content": [...]
}
```

## Testing Context Output

### Verify context in JSON output

```bash
# Generate output
blueprint document.pdf --structured --language de --module ubs --quiet

# Check context
jq '.context' document_merged.json

# Output:
# {
#   "language": "de",
#   "modules": {
#     "enabled_modules": ["ubs"]
#   }
# }

# Check content exists
jq '.content | length' document_merged.json
# Output: 42
```

### Extract specific information

```bash
# Get language
jq -r '.context.language' document_merged.json

# Get enabled modules
jq -r '.context.modules.enabled_modules[]' document_merged.json

# Get all headings
jq '.content[] | select(.type == "heading")' document_merged.json
```

## Common Patterns

### Pattern 1: Language-specific processing

```bash
# German documents
blueprint *.pdf --language de --structured

# English documents
blueprint *.pdf --language en --structured

# French documents
blueprint *.pdf --language fr --structured
```

### Pattern 2: Bank-specific processing

```bash
# UBS documents
blueprint ubs_forms/*.pdf --module ubs --structured --quiet

# Custom validation
blueprint forms/*.pdf --module custom --module validation --structured
```

### Pattern 3: Batch processing with different configs

```bash
#!/bin/bash

for doc in forms/*.pdf; do
    name=$(basename "$doc" .pdf)
    
    # Detect language from filename
    if [[ $name == *"_DE.pdf" ]]; then
        lang="de"
    elif [[ $name == *"_EN.pdf" ]]; then
        lang="en"
    else
        lang="de"  # default
    fi
    
    # Process with appropriate language
    blueprint "$doc" \
        --language "$lang" \
        --module ubs \
        --structured \
        --quiet
        
    echo "Processed: $doc (language: $lang)"
done
```

## Troubleshooting

### Check if context is properly included

```bash
# Should show "context" and "content" keys
jq 'keys' document_merged.json

# Should show language
jq '.context.language' document_merged.json

# Should show modules if any were enabled
jq '.context.modules' document_merged.json
```

### Validate JSON structure

```bash
# Validate JSON syntax
jq empty document_merged.json && echo "Valid JSON" || echo "Invalid JSON"

# Count nodes in content
jq '.content | length' document_merged.json

# List all node types
jq '.content[].type' document_merged.json | sort | uniq -c
```

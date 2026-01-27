//! Font Manager Module - XFA Font Resolution and Loading
//!
//! This module handles font resolution according to the XFA specification.
//! Per XFA spec section 17 (Template Reference - font element):
//! - typeface: The name of the typeface. Default is "Courier"
//! - size: Font size as measurement. Default is 10pt
//! - weight: "normal" or "bold". Default is "normal"
//! - posture: "normal" or "italic". Default is "normal"
//!
//! Font Resolution Strategy (per XFA spec section 28, "Font Mapping"):
//! 1. Try to find exact match for typeface + weight + posture
//! 2. If not found, check embedded fonts from PDF
//! 3. If not found, try common aliases (e.g., "Helvetica" -> "Arial")
//! 4. If not found, use genericFamily fallback (serif, sansSerif, monospace, etc.)
//! 5. If still not found, return error (configurable) or use system fallback
//!
//! Per XFA spec section 28 (Font Mapping in LiveCycle Forms ES3):
//! "When the XFA processor is unable to supply the requested font it substitutes
//! whatever font is available that it considers the best match."
//!
//! The genericFamily attribute values (from CSS2):
//! - sansSerif: Absence of serifs (default)
//! - serif: Presence of serifs on characters
//! - cursive: Presence of joining strokes (handwritten look)
//! - fantasy: Decorative fonts
//! - monospace: Fixed-width fonts

use crate::xfa::{Font, FontWeight, FontPosture, GenericFamily};
use ab_glyph::FontRef;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::OnceLock;
use thiserror::Error;

/// Font-related errors
#[derive(Debug, Error)]
pub enum FontError {
    /// The requested font was not found and no suitable fallback exists
    #[error("Font not found: '{typeface}' (weight: {weight:?}, posture: {posture:?}). Tried aliases: {tried_aliases:?}. Consider embedding the font in the PDF or specifying a genericFamily fallback.")]
    FontNotFound {
        typeface: String,
        weight: FontWeight,
        posture: FontPosture,
        tried_aliases: Vec<String>,
    },
    
    /// No fallback font available on the system
    #[error("No fallback font available. Searched paths: {searched_paths:?}")]
    NoFallbackAvailable {
        searched_paths: Vec<String>,
    },
    
    /// Failed to read font file
    #[error("Failed to read font file '{path}': {reason}")]
    FontFileReadError {
        path: String,
        reason: String,
    },
    
    /// Failed to parse font data
    #[error("Failed to parse font data: {reason}")]
    FontParseError {
        reason: String,
    },
    
    /// Lock error when accessing font manager
    #[error("Font manager lock error: {0}")]
    LockError(String),
    
    /// Embedded font data is invalid
    #[error("Invalid embedded font data for '{name}': {reason}")]
    InvalidEmbeddedFont {
        name: String,
        reason: String,
    },
}

/// Extension trait to add fallback_typefaces to GenericFamily
pub trait GenericFamilyExt {
    /// Get fallback font families for this generic family
    /// Returns a list of common typeface names to try
    fn fallback_typefaces(&self) -> Vec<&'static str>;
}

impl GenericFamilyExt for GenericFamily {
    fn fallback_typefaces(&self) -> Vec<&'static str> {
        match self {
            // Prefer Arial first since it has explicit bold/italic files on most systems
            GenericFamily::SansSerif => vec![
                "arial", "helvetica", "helvetica neue", 
                "liberation sans", "dejavu sans", "verdana"
            ],
            GenericFamily::Serif => vec![
                "times new roman", "times", "georgia",
                "liberation serif", "dejavu serif"
            ],
            GenericFamily::Monospace => vec![
                "courier new", "courier", "consolas",
                "liberation mono", "dejavu sans mono"
            ],
            GenericFamily::Cursive => vec![
                "comic sans ms", "brush script mt", "lucida handwriting"
            ],
            GenericFamily::Fantasy => vec![
                "impact", "papyrus", "copperplate"
            ],
        }
    }
}

/// Embedded font data from a PDF file
#[derive(Debug, Clone)]
pub struct EmbeddedFont {
    /// Font name as referenced in the PDF
    pub name: String,
    /// The raw font data (TTF/OTF)
    pub data: Vec<u8>,
    /// Font weight if known
    pub weight: FontWeight,
    /// Font posture if known  
    pub posture: FontPosture,
    /// Generic family hint
    pub generic_family: Option<GenericFamily>,
}

/// Font variant key for caching
#[derive(Debug, Clone, Hash, PartialEq, Eq)]
pub struct FontVariant {
    pub family: String,
    pub weight: FontWeight,
    pub posture: FontPosture,
}

impl FontVariant {
    pub fn new(family: &str, weight: FontWeight, posture: FontPosture) -> Self {
        FontVariant {
            family: family.to_lowercase(),
            weight,
            posture,
        }
    }
    
    pub fn from_xfa_font(font: &Font) -> Self {
        FontVariant {
            family: font.typeface.to_lowercase(),
            weight: font.weight,
            posture: font.posture,
        }
    }
}

/// Font file information
#[derive(Debug, Clone)]
struct FontFile {
    path: PathBuf,
    family: String,
    weight: FontWeight,
    posture: FontPosture,
}

/// Configuration for font resolution behavior
#[derive(Debug, Clone)]
pub struct FontConfig {
    /// If true, return an error when a font is not found (instead of using fallback)
    pub strict_mode: bool,
    /// Generic family to use for fallback when typeface not found
    pub default_generic_family: GenericFamily,
}

impl Default for FontConfig {
    fn default() -> Self {
        FontConfig {
            strict_mode: false,
            default_generic_family: GenericFamily::SansSerif,
        }
    }
}

/// Font manager that handles font resolution and loading
/// 
/// Per XFA spec, font resolution follows this priority:
/// 1. Embedded fonts from PDF (registered via `register_embedded_font`)
/// 2. Exact typeface match from system fonts
/// 3. Aliases (e.g., "Helvetica" -> "Arial")
/// 4. Generic family fallback (serif, sansSerif, monospace, etc.)
/// 5. System default fallback
pub struct FontManager {
    /// Known font files by variant
    font_files: HashMap<FontVariant, FontFile>,
    /// Loaded fonts (static lifetime for ab_glyph)
    loaded_fonts: HashMap<FontVariant, &'static [u8]>,
    /// Font family aliases (e.g., "Helvetica" -> ["Arial", "Helvetica Neue"])
    aliases: HashMap<String, Vec<String>>,
    /// Default fallback font data
    fallback_font_data: Option<&'static [u8]>,
    /// Embedded fonts from PDF files (per XFA spec: "PDF file may carry the required fonts")
    embedded_fonts: HashMap<String, EmbeddedFont>,
    /// Loaded embedded font data (static lifetime)
    loaded_embedded: HashMap<String, &'static [u8]>,
    /// Configuration
    config: FontConfig,
}

impl FontManager {
    /// Create a new font manager and scan for system fonts
    pub fn new() -> Self {
        Self::with_config(FontConfig::default())
    }
    
    /// Create a new font manager with custom configuration
    pub fn with_config(config: FontConfig) -> Self {
        let mut manager = FontManager {
            font_files: HashMap::new(),
            loaded_fonts: HashMap::new(),
            aliases: Self::build_aliases(),
            fallback_font_data: None,
            embedded_fonts: HashMap::new(),
            loaded_embedded: HashMap::new(),
            config,
        };
        manager.scan_system_fonts();
        manager
    }
    
    /// Enable or disable strict mode
    /// In strict mode, returns an error when a font is not found instead of using fallback
    pub fn set_strict_mode(&mut self, strict: bool) {
        self.config.strict_mode = strict;
    }
    
    /// Register an embedded font from a PDF file
    /// Per XFA spec section 28: "When an XFA form is packaged inside a PDF file
    /// the PDF file may carry the required fonts along with the form."
    pub fn register_embedded_font(&mut self, font: EmbeddedFont) -> Result<(), FontError> {
        // Validate that we can parse the font data
        FontRef::try_from_slice(&font.data)
            .map_err(|e| FontError::InvalidEmbeddedFont {
                name: font.name.clone(),
                reason: e.to_string(),
            })?;
        
        let name_lower = font.name.to_lowercase();
        self.embedded_fonts.insert(name_lower, font);
        Ok(())
    }
    
    /// Register multiple embedded fonts from a PDF
    pub fn register_embedded_fonts(&mut self, fonts: Vec<EmbeddedFont>) -> Result<(), FontError> {
        for font in fonts {
            self.register_embedded_font(font)?;
        }
        Ok(())
    }
    
    /// Check if an embedded font with this name exists
    pub fn has_embedded_font(&self, name: &str) -> bool {
        self.embedded_fonts.contains_key(&name.to_lowercase())
    }
    
    /// Get list of registered embedded fonts
    pub fn embedded_font_names(&self) -> Vec<&str> {
        self.embedded_fonts.values().map(|f| f.name.as_str()).collect()
    }
    
    /// Build font family aliases map
    /// Per XFA spec: when requested font is unavailable, substitute best match
    fn build_aliases() -> HashMap<String, Vec<String>> {
        let mut aliases = HashMap::new();
        
        // Sans-serif family aliases
        aliases.insert("helvetica".to_string(), vec![
            "arial".to_string(),
            "helvetica neue".to_string(),
            "liberation sans".to_string(),
            "dejavu sans".to_string(),
        ]);
        aliases.insert("arial".to_string(), vec![
            "helvetica".to_string(),
            "helvetica neue".to_string(),
            "liberation sans".to_string(),
            "dejavu sans".to_string(),
        ]);
        
        // Serif family aliases  
        aliases.insert("times".to_string(), vec![
            "times new roman".to_string(),
            "liberation serif".to_string(),
            "dejavu serif".to_string(),
        ]);
        aliases.insert("times new roman".to_string(), vec![
            "times".to_string(),
            "liberation serif".to_string(),
            "dejavu serif".to_string(),
        ]);
        
        // Monospace family aliases (XFA default is Courier)
        aliases.insert("courier".to_string(), vec![
            "courier new".to_string(),
            "liberation mono".to_string(),
            "dejavu sans mono".to_string(),
        ]);
        aliases.insert("courier new".to_string(), vec![
            "courier".to_string(),
            "liberation mono".to_string(),
            "dejavu sans mono".to_string(),
        ]);
        
        // Note: Per XFA spec (Chapter 28), font substitution should use:
        // 1. equate elements from XFA config packet (not implemented yet)
        // 2. Same typeface with different weight/posture (handled in resolution code)
        // 3. Locale's typeFaces fallback fonts (not implemented yet)
        // 4. genericFamily fallback (implemented)
        //
        // The aliases below are minimal cross-platform mappings for common font name variations,
        // NOT arbitrary substitutions like "Frutiger" -> "Arial"
        
        aliases
    }
    
    /// Scan system font directories and register available fonts
    fn scan_system_fonts(&mut self) {
        // macOS font directories
        let macos_dirs = [
            "/System/Library/Fonts",
            "/System/Library/Fonts/Supplemental",
            "/Library/Fonts",
        ];
        
        // User font directory (macOS)
        let home_dir = std::env::var("HOME").unwrap_or_default();
        let user_fonts_dir = format!("{}/Library/Fonts", home_dir);
        
        // Linux font directories
        let linux_dirs = [
            "/usr/share/fonts/truetype",
            "/usr/share/fonts/TTF",
            "/usr/local/share/fonts",
        ];
        
        // Windows font directory
        let windows_dirs = [
            "C:\\Windows\\Fonts",
        ];
        
        // Combine all directories
        let all_dirs: Vec<&str> = if cfg!(target_os = "macos") {
            macos_dirs.to_vec()
        } else if cfg!(target_os = "linux") {
            linux_dirs.to_vec()
        } else if cfg!(target_os = "windows") {
            windows_dirs.to_vec()
        } else {
            // Try all on unknown OS
            macos_dirs.iter().chain(linux_dirs.iter()).chain(windows_dirs.iter()).copied().collect()
        };
        
        for dir in all_dirs {
            self.scan_font_directory(dir);
        }
        
        // Also scan user fonts directory on macOS
        if cfg!(target_os = "macos") && !user_fonts_dir.is_empty() {
            self.scan_font_directory(&user_fonts_dir);
        }
        
        // Register specific known font files for common families
        self.register_common_fonts();
    }
    
    /// Scan a font directory recursively
    fn scan_font_directory(&mut self, dir: &str) {
        let path = PathBuf::from(dir);
        if !path.exists() {
            return;
        }
        
        if let Ok(entries) = std::fs::read_dir(&path) {
            for entry in entries.flatten() {
                let file_path = entry.path();
                if file_path.is_file() {
                    if let Some(ext) = file_path.extension() {
                        let ext_lower = ext.to_string_lossy().to_lowercase();
                        if ext_lower == "ttf" || ext_lower == "otf" || ext_lower == "ttc" {
                            self.try_register_font_file(&file_path);
                        }
                    }
                } else if file_path.is_dir() {
                    // Recurse into subdirectories
                    self.scan_font_directory(&file_path.to_string_lossy());
                }
            }
        }
    }
    
    /// Try to register a font file by parsing its name
    fn try_register_font_file(&mut self, path: &PathBuf) {
        let file_name = path.file_stem()
            .map(|s| s.to_string_lossy().to_string())
            .unwrap_or_default();
        
        let file_name_lower = file_name.to_lowercase();
        
        // Parse weight and posture from filename
        let (family, weight, posture) = Self::parse_font_filename(&file_name_lower);
        
        if !family.is_empty() {
            let variant = FontVariant::new(&family, weight, posture);
            let font_file = FontFile {
                path: path.clone(),
                family: family.clone(),
                weight,
                posture,
            };
            
            // Only insert if we don't already have this variant
            self.font_files.entry(variant).or_insert(font_file);
        }
    }
    
    /// Parse font filename to extract family, weight, and posture
    fn parse_font_filename(filename: &str) -> (String, FontWeight, FontPosture) {
        let mut weight = FontWeight::Normal;
        let mut posture = FontPosture::Normal;
        
        // Check for weight indicators
        let is_bold = filename.contains("bold") || filename.contains("-b") || filename.ends_with("b");
        if is_bold {
            weight = FontWeight::Bold;
        }
        
        // Check for posture indicators
        let is_italic = filename.contains("italic") || filename.contains("oblique") 
            || filename.contains("-i") || filename.ends_with("i")
            || filename.ends_with("it");
        if is_italic {
            posture = FontPosture::Italic;
        }
        
        // Extract family name by removing weight/posture suffixes
        let mut family = filename.to_string();
        for suffix in &["bold italic", "bolditalic", "bold", "italic", "oblique", 
                        " bold", " italic", "-bold", "-italic", "-regular", "regular",
                        "-b", "-i", " b", " i"] {
            family = family.replace(suffix, "");
        }
        
        // Clean up family name
        family = family.trim().replace(['_', '-'], " ");
        
        (family, weight, posture)
    }
    
    /// Register commonly used fonts with explicit paths
    fn register_common_fonts(&mut self) {
        // Define common font mappings
        let common_fonts = [
            // Helvetica variants (macOS)
            ("/System/Library/Fonts/Helvetica.ttc", "helvetica", FontWeight::Normal, FontPosture::Normal),
            
            // Arial variants (cross-platform)
            ("/System/Library/Fonts/Supplemental/Arial.ttf", "arial", FontWeight::Normal, FontPosture::Normal),
            ("/System/Library/Fonts/Supplemental/Arial Bold.ttf", "arial", FontWeight::Bold, FontPosture::Normal),
            ("/System/Library/Fonts/Supplemental/Arial Italic.ttf", "arial", FontWeight::Normal, FontPosture::Italic),
            ("/System/Library/Fonts/Supplemental/Arial Bold Italic.ttf", "arial", FontWeight::Bold, FontPosture::Italic),
            
            // Courier variants (XFA default)
            ("/System/Library/Fonts/Courier.ttc", "courier", FontWeight::Normal, FontPosture::Normal),
            ("/System/Library/Fonts/Supplemental/Courier New.ttf", "courier new", FontWeight::Normal, FontPosture::Normal),
            ("/System/Library/Fonts/Supplemental/Courier New Bold.ttf", "courier new", FontWeight::Bold, FontPosture::Normal),
            ("/System/Library/Fonts/Supplemental/Courier New Italic.ttf", "courier new", FontWeight::Normal, FontPosture::Italic),
            ("/System/Library/Fonts/Supplemental/Courier New Bold Italic.ttf", "courier new", FontWeight::Bold, FontPosture::Italic),
            
            // Times variants
            ("/System/Library/Fonts/Supplemental/Times New Roman.ttf", "times new roman", FontWeight::Normal, FontPosture::Normal),
            ("/System/Library/Fonts/Supplemental/Times New Roman Bold.ttf", "times new roman", FontWeight::Bold, FontPosture::Normal),
            ("/System/Library/Fonts/Supplemental/Times New Roman Italic.ttf", "times new roman", FontWeight::Normal, FontPosture::Italic),
            ("/System/Library/Fonts/Supplemental/Times New Roman Bold Italic.ttf", "times new roman", FontWeight::Bold, FontPosture::Italic),
            
            // DejaVu (Linux fallback)
            ("/usr/share/fonts/truetype/dejavu/DejaVuSans.ttf", "dejavu sans", FontWeight::Normal, FontPosture::Normal),
            ("/usr/share/fonts/truetype/dejavu/DejaVuSans-Bold.ttf", "dejavu sans", FontWeight::Bold, FontPosture::Normal),
            ("/usr/share/fonts/truetype/dejavu/DejaVuSans-Oblique.ttf", "dejavu sans", FontWeight::Normal, FontPosture::Italic),
            ("/usr/share/fonts/truetype/dejavu/DejaVuSans-BoldOblique.ttf", "dejavu sans", FontWeight::Bold, FontPosture::Italic),
            ("/usr/share/fonts/truetype/dejavu/DejaVuSansMono.ttf", "dejavu sans mono", FontWeight::Normal, FontPosture::Normal),
            ("/usr/share/fonts/truetype/dejavu/DejaVuSansMono-Bold.ttf", "dejavu sans mono", FontWeight::Bold, FontPosture::Normal),
            
            // Windows fonts
            ("C:\\Windows\\Fonts\\arial.ttf", "arial", FontWeight::Normal, FontPosture::Normal),
            ("C:\\Windows\\Fonts\\arialbd.ttf", "arial", FontWeight::Bold, FontPosture::Normal),
            ("C:\\Windows\\Fonts\\ariali.ttf", "arial", FontWeight::Normal, FontPosture::Italic),
            ("C:\\Windows\\Fonts\\arialbi.ttf", "arial", FontWeight::Bold, FontPosture::Italic),
            ("C:\\Windows\\Fonts\\cour.ttf", "courier new", FontWeight::Normal, FontPosture::Normal),
            ("C:\\Windows\\Fonts\\courbd.ttf", "courier new", FontWeight::Bold, FontPosture::Normal),
            ("C:\\Windows\\Fonts\\couri.ttf", "courier new", FontWeight::Normal, FontPosture::Italic),
            ("C:\\Windows\\Fonts\\courbi.ttf", "courier new", FontWeight::Bold, FontPosture::Italic),
            ("C:\\Windows\\Fonts\\times.ttf", "times new roman", FontWeight::Normal, FontPosture::Normal),
            ("C:\\Windows\\Fonts\\timesbd.ttf", "times new roman", FontWeight::Bold, FontPosture::Normal),
            ("C:\\Windows\\Fonts\\timesi.ttf", "times new roman", FontWeight::Normal, FontPosture::Italic),
            ("C:\\Windows\\Fonts\\timesbi.ttf", "times new roman", FontWeight::Bold, FontPosture::Italic),
        ];
        
        for (path, family, weight, posture) in common_fonts {
            let path_buf = PathBuf::from(path);
            if path_buf.exists() {
                let variant = FontVariant::new(family, weight, posture);
                let font_file = FontFile {
                    path: path_buf,
                    family: family.to_string(),
                    weight,
                    posture,
                };
                self.font_files.insert(variant, font_file);
            }
        }
    }
    
    /// Get font data for a specific variant
    /// Returns static font data that can be used with ab_glyph
    /// 
    /// Resolution order per XFA spec:
    /// 1. Embedded fonts from PDF
    /// 2. Exact system font match
    /// 3. Aliases
    /// 4. Normal weight/posture variant
    /// 5. Generic family fallback (if set)
    /// 6. System fallback (if not in strict mode)
    pub fn get_font_data(&mut self, variant: &FontVariant) -> Result<&'static [u8], FontError> {
        self.get_font_data_with_generic(variant, None)
    }
    
    /// Get font data with optional generic family fallback
    pub fn get_font_data_with_generic(
        &mut self, 
        variant: &FontVariant,
        generic_family: Option<GenericFamily>,
    ) -> Result<&'static [u8], FontError> {
        let mut tried_aliases = Vec::new();
        
        // Check if already loaded
        if let Some(data) = self.loaded_fonts.get(variant) {
            return Ok(*data);
        }
        
        // 1. Try embedded fonts first (per XFA spec: PDF embedded fonts take priority)
        let family_lower = variant.family.to_lowercase();
        if let Some(embedded) = self.embedded_fonts.get(&family_lower) {
            // Check if already loaded as static
            if let Some(data) = self.loaded_embedded.get(&family_lower) {
                return Ok(*data);
            }
            // Load and cache embedded font
            let static_data: &'static [u8] = Box::leak(embedded.data.clone().into_boxed_slice());
            self.loaded_embedded.insert(family_lower.clone(), static_data);
            self.loaded_fonts.insert(variant.clone(), static_data);
            return Ok(static_data);
        }
        
        // 2. Try to find system font file with exact match
        if let Some(font_file) = self.font_files.get(variant) {
            return self.load_font_file(&font_file.path.clone(), variant.clone());
        }
        
        // 3. Try original font with different weight/posture (per XFA spec step 2:
        //    "ignore the weight and posture attributes and use any available font that matches")
        // This is higher priority than aliases - better to use the right typeface with wrong weight
        // than a different typeface
        if variant.weight != FontWeight::Normal || variant.posture != FontPosture::Normal {
            let normal_variant = FontVariant::new(&variant.family, FontWeight::Normal, FontPosture::Normal);
            if let Some(font_file) = self.font_files.get(&normal_variant) {
                return self.load_font_file(&font_file.path.clone(), variant.clone());
            }
        }
        
        // 4. Try aliases - first pass: preserve weight and posture
        if let Some(aliases) = self.aliases.get(&variant.family).cloned() {
            for alias in &aliases {
                tried_aliases.push(alias.clone());
                
                // Check embedded fonts for alias
                if let Some(embedded) = self.embedded_fonts.get(alias) {
                    if let Some(data) = self.loaded_embedded.get(alias) {
                        return Ok(*data);
                    }
                    let static_data: &'static [u8] = Box::leak(embedded.data.clone().into_boxed_slice());
                    self.loaded_embedded.insert(alias.clone(), static_data);
                    self.loaded_fonts.insert(variant.clone(), static_data);
                    return Ok(static_data);
                }
                
                // Check system fonts for alias with same weight/posture
                let alias_variant = FontVariant::new(alias, variant.weight, variant.posture);
                if let Some(font_file) = self.font_files.get(&alias_variant) {
                    return self.load_font_file(&font_file.path.clone(), variant.clone());
                }
            }
        }
        
        // 5. Try aliases with normal weight if we were looking for bold/italic
        if (variant.weight != FontWeight::Normal || variant.posture != FontPosture::Normal)
            && let Some(aliases) = self.aliases.get(&variant.family).cloned() {
                for alias in &aliases {
                    let alias_normal = FontVariant::new(alias, FontWeight::Normal, FontPosture::Normal);
                    if let Some(font_file) = self.font_files.get(&alias_normal) {
                        return self.load_font_file(&font_file.path.clone(), variant.clone());
                    }
                }
            }
        
        // 6. Try generic family fallback - preserve weight and posture
        let generic = generic_family.unwrap_or(self.config.default_generic_family);
        for fallback_typeface in generic.fallback_typefaces() {
            if fallback_typeface.to_lowercase() == variant.family {
                continue; // Already tried this one
            }
            if !tried_aliases.contains(&fallback_typeface.to_string()) {
                tried_aliases.push(fallback_typeface.to_string());
            }
            
            // Try with requested weight/posture
            let fallback_variant = FontVariant::new(fallback_typeface, variant.weight, variant.posture);
            if let Some(font_file) = self.font_files.get(&fallback_variant) {
                return self.load_font_file(&font_file.path.clone(), variant.clone());
            }
        }
        
        // 6. Try generic family with normal variant as last resort
        if variant.weight != FontWeight::Normal || variant.posture != FontPosture::Normal {
            for fallback_typeface in generic.fallback_typefaces() {
                if fallback_typeface.to_lowercase() == variant.family {
                    continue;
                }
                let fallback_normal = FontVariant::new(fallback_typeface, FontWeight::Normal, FontPosture::Normal);
                if let Some(font_file) = self.font_files.get(&fallback_normal) {
                    return self.load_font_file(&font_file.path.clone(), variant.clone());
                }
            }
        }
        
        // 7. In strict mode, return error. Otherwise use system fallback.
        if self.config.strict_mode {
            return Err(FontError::FontNotFound {
                typeface: variant.family.clone(),
                weight: variant.weight,
                posture: variant.posture,
                tried_aliases,
            });
        }
        
        // Use fallback font
        self.get_fallback_font()
    }
    
    /// Load a font file and cache it
    fn load_font_file(&mut self, path: &PathBuf, variant: FontVariant) -> Result<&'static [u8], FontError> {
        let font_data = std::fs::read(path)
            .map_err(|e| FontError::FontFileReadError {
                path: path.to_string_lossy().to_string(),
                reason: e.to_string(),
            })?;
        
        // Leak the data to get 'static lifetime (necessary for ab_glyph)
        let static_data: &'static [u8] = Box::leak(font_data.into_boxed_slice());
        
        self.loaded_fonts.insert(variant, static_data);
        Ok(static_data)
    }
    
    /// Get fallback font data
    fn get_fallback_font(&mut self) -> Result<&'static [u8], FontError> {
        if let Some(data) = self.fallback_font_data {
            return Ok(data);
        }
        
        // Try common fallback fonts in order of preference
        let fallback_paths = [
            // macOS
            "/System/Library/Fonts/Helvetica.ttc",
            "/System/Library/Fonts/Supplemental/Arial.ttf",
            "/System/Library/Fonts/Geneva.ttf",
            // Linux
            "/usr/share/fonts/truetype/dejavu/DejaVuSans.ttf",
            "/usr/share/fonts/TTF/DejaVuSans.ttf",
            "/usr/share/fonts/truetype/liberation/LiberationSans-Regular.ttf",
            // Windows
            "C:\\Windows\\Fonts\\arial.ttf",
            "C:\\Windows\\Fonts\\segoeui.ttf",
        ];
        
        for path in &fallback_paths {
            if let Ok(font_data) = std::fs::read(path) {
                let static_data: &'static [u8] = Box::leak(font_data.into_boxed_slice());
                self.fallback_font_data = Some(static_data);
                return Ok(static_data);
            }
        }
        
        Err(FontError::NoFallbackAvailable {
            searched_paths: fallback_paths.iter().map(|s| s.to_string()).collect(),
        })
    }
    
    /// Get a FontRef for a specific XFA font style
    pub fn get_font(&mut self, xfa_font: &Font) -> Result<FontRef<'static>, FontError> {
        let variant = FontVariant::from_xfa_font(xfa_font);
        let data = self.get_font_data_with_generic(&variant, xfa_font.generic_family)?;
        
        FontRef::try_from_slice(data)
            .map_err(|e| FontError::FontParseError {
                reason: e.to_string(),
            })
    }
    
    /// Get a FontRef using default XFA font settings
    pub fn get_default_font(&mut self) -> Result<FontRef<'static>, FontError> {
        let default_font = Font::default();
        self.get_font(&default_font)
    }
    
    /// Check if a specific font variant is available
    pub fn has_font(&self, family: &str, weight: FontWeight, posture: FontPosture) -> bool {
        let variant = FontVariant::new(family, weight, posture);
        self.font_files.contains_key(&variant)
    }
    
    /// Get list of available font families
    pub fn available_families(&self) -> Vec<String> {
        let mut families: Vec<String> = self.font_files.values()
            .map(|f| f.family.clone())
            .collect();
        families.sort();
        families.dedup();
        families
    }
}

impl Default for FontManager {
    fn default() -> Self {
        Self::new()
    }
}

/// Global font manager instance (lazy initialized)
static GLOBAL_FONT_MANAGER: OnceLock<std::sync::Mutex<FontManager>> = OnceLock::new();

/// Get the global font manager
pub fn get_font_manager() -> &'static std::sync::Mutex<FontManager> {
    GLOBAL_FONT_MANAGER.get_or_init(|| std::sync::Mutex::new(FontManager::new()))
}

/// Convenience function to get a font for an XFA font style
pub fn get_font_for_style(xfa_font: &Font) -> Result<FontRef<'static>, FontError> {
    let manager = get_font_manager();
    let mut manager = manager.lock().map_err(|e| FontError::LockError(e.to_string()))?;
    manager.get_font(xfa_font)
}

/// Convenience function to get the default fallback font
pub fn get_fallback_font() -> Result<FontRef<'static>, FontError> {
    let manager = get_font_manager();
    let mut manager = manager.lock().map_err(|e| FontError::LockError(e.to_string()))?;
    manager.get_default_font()
}

/// Register an embedded font globally (for use with PDFs that contain embedded fonts)
pub fn register_embedded_font_global(font: EmbeddedFont) -> Result<(), FontError> {
    let manager = get_font_manager();
    let mut manager = manager.lock().map_err(|e| FontError::LockError(e.to_string()))?;
    manager.register_embedded_font(font)
}

/// Enable strict mode globally (returns errors instead of fallback)
pub fn set_strict_mode_global(strict: bool) -> Result<(), FontError> {
    let manager = get_font_manager();
    let mut manager = manager.lock().map_err(|e| FontError::LockError(e.to_string()))?;
    manager.set_strict_mode(strict);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    
    #[test]
    fn test_font_variant_creation() {
        let variant = FontVariant::new("Arial", FontWeight::Bold, FontPosture::Italic);
        assert_eq!(variant.family, "arial");
        assert_eq!(variant.weight, FontWeight::Bold);
        assert_eq!(variant.posture, FontPosture::Italic);
    }
    
    #[test]
    fn test_font_manager_creation() {
        let manager = FontManager::new();
        // Should have at least some fonts registered on any system
        let families = manager.available_families();
        println!("Available font families: {:?}", families);
    }
    
    #[test]
    fn test_fallback_font() {
        let mut manager = FontManager::new();
        let result = manager.get_fallback_font();
        assert!(result.is_ok(), "Should be able to load fallback font");
    }
    
    #[test]
    fn test_default_xfa_font() {
        let mut manager = FontManager::new();
        let font = Font::default();
        // Courier is the XFA default, should fallback to Courier New or similar
        let result = manager.get_font(&font);
        if result.is_err() {
            // Fallback should still work
            let fallback = manager.get_fallback_font();
            assert!(fallback.is_ok(), "Fallback font should work");
        }
    }
    
    #[test]
    fn test_parse_font_filename() {
        let (family, weight, posture) = FontManager::parse_font_filename("arial bold italic");
        assert_eq!(family, "arial");
        assert_eq!(weight, FontWeight::Bold);
        assert_eq!(posture, FontPosture::Italic);
        
        let (family, weight, posture) = FontManager::parse_font_filename("times new roman");
        assert_eq!(family, "times new roman");
        assert_eq!(weight, FontWeight::Normal);
        assert_eq!(posture, FontPosture::Normal);
    }
    
    #[test]
    fn test_generic_family_parsing() {
        assert_eq!("serif".parse::<GenericFamily>().unwrap(), GenericFamily::Serif);
        assert_eq!("sansSerif".parse::<GenericFamily>().unwrap(), GenericFamily::SansSerif);
        assert_eq!("sans-serif".parse::<GenericFamily>().unwrap(), GenericFamily::SansSerif);
        assert_eq!("monospace".parse::<GenericFamily>().unwrap(), GenericFamily::Monospace);
        assert_eq!("cursive".parse::<GenericFamily>().unwrap(), GenericFamily::Cursive);
        assert_eq!("fantasy".parse::<GenericFamily>().unwrap(), GenericFamily::Fantasy);
        // Unknown defaults to SansSerif
        assert_eq!("unknown".parse::<GenericFamily>().unwrap(), GenericFamily::SansSerif);
    }
    
    #[test]
    fn test_generic_family_fallbacks() {
        let sans = GenericFamily::SansSerif;
        let fallbacks = sans.fallback_typefaces();
        assert!(fallbacks.contains(&"arial"));
        assert!(fallbacks.contains(&"helvetica"));
        
        let mono = GenericFamily::Monospace;
        let fallbacks = mono.fallback_typefaces();
        assert!(fallbacks.contains(&"courier"));
    }
    
    #[test]
    fn test_strict_mode_error() {
        // Create a manager with strict mode but clear the font files to ensure nothing is found
        let mut manager = FontManager {
            font_files: HashMap::new(),
            loaded_fonts: HashMap::new(),
            aliases: HashMap::new(),
            fallback_font_data: None,
            embedded_fonts: HashMap::new(),
            loaded_embedded: HashMap::new(),
            config: FontConfig {
                strict_mode: true,
                default_generic_family: GenericFamily::SansSerif,
            },
        };
        
        // Try to get a font that definitely doesn't exist (with empty font_files, nothing will be found)
        let variant = FontVariant::new("NonExistentFontXYZ123", FontWeight::Normal, FontPosture::Normal);
        let result = manager.get_font_data(&variant);
        
        assert!(result.is_err(), "Should return error in strict mode when font not found");
        if let Err(FontError::FontNotFound { typeface, .. }) = result {
            assert_eq!(typeface, "nonexistentfontxyz123");
        } else {
            panic!("Expected FontNotFound error, got {:?}", result);
        }
    }
    
    #[test]
    fn test_embedded_font_registration() {
        let mut manager = FontManager::new();
        
        // Create a minimal valid TTF (this is a placeholder - in practice you'd use real font data)
        // For testing, we just verify the API works
        let font = EmbeddedFont {
            name: "TestFont".to_string(),
            data: vec![], // Empty data will fail validation
            weight: FontWeight::Normal,
            posture: FontPosture::Normal,
            generic_family: Some(GenericFamily::SansSerif),
        };
        
        // Should fail due to invalid font data
        let result = manager.register_embedded_font(font);
        assert!(result.is_err());
    }
}

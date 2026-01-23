//! Font Manager Module - XFA Font Resolution and Loading
//!
//! This module handles font resolution according to the XFA specification.
//! Per XFA spec section 17 (Template Reference - font element):
//! - typeface: The name of the typeface. Default is "Courier"
//! - size: Font size as measurement. Default is 10pt
//! - weight: "normal" or "bold". Default is "normal"
//! - posture: "normal" or "italic". Default is "normal"
//!
//! Font Resolution Strategy:
//! 1. Try to find exact match for typeface + weight + posture
//! 2. If not found, try common aliases (e.g., "Helvetica" -> "Arial")
//! 3. If still not found, use fallback font (DejaVu Sans or system default)

use crate::xfa::{Font, FontWeight, FontPosture};
use ab_glyph::FontRef;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::OnceLock;

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

/// Font manager that handles font resolution and loading
pub struct FontManager {
    /// Known font files by variant
    font_files: HashMap<FontVariant, FontFile>,
    /// Loaded fonts (static lifetime for ab_glyph)
    loaded_fonts: HashMap<FontVariant, &'static [u8]>,
    /// Font family aliases (e.g., "Helvetica" -> ["Arial", "Helvetica Neue"])
    aliases: HashMap<String, Vec<String>>,
    /// Default fallback font data
    fallback_font_data: Option<&'static [u8]>,
}

impl FontManager {
    /// Create a new font manager and scan for system fonts
    pub fn new() -> Self {
        let mut manager = FontManager {
            font_files: HashMap::new(),
            loaded_fonts: HashMap::new(),
            aliases: Self::build_aliases(),
            fallback_font_data: None,
        };
        manager.scan_system_fonts();
        manager
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
        
        // Myriad aliases
        aliases.insert("myriad pro".to_string(), vec![
            "myriad".to_string(),
            "helvetica".to_string(),
            "arial".to_string(),
        ]);
        
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
    pub fn get_font_data(&mut self, variant: &FontVariant) -> Result<&'static [u8], String> {
        // Check if already loaded
        if let Some(data) = self.loaded_fonts.get(variant) {
            return Ok(*data);
        }
        
        // Try to find font file
        if let Some(font_file) = self.font_files.get(variant) {
            return self.load_font_file(&font_file.path.clone(), variant.clone());
        }
        
        // Try aliases
        if let Some(aliases) = self.aliases.get(&variant.family).cloned() {
            for alias in aliases {
                let alias_variant = FontVariant::new(&alias, variant.weight, variant.posture);
                if let Some(font_file) = self.font_files.get(&alias_variant) {
                    return self.load_font_file(&font_file.path.clone(), variant.clone());
                }
            }
        }
        
        // Try with normal weight/posture if bold/italic not found
        if variant.weight != FontWeight::Normal || variant.posture != FontPosture::Normal {
            let normal_variant = FontVariant::new(&variant.family, FontWeight::Normal, FontPosture::Normal);
            if let Ok(data) = self.get_font_data(&normal_variant) {
                return Ok(data);
            }
        }
        
        // Use fallback font
        self.get_fallback_font()
    }
    
    /// Load a font file and cache it
    fn load_font_file(&mut self, path: &PathBuf, variant: FontVariant) -> Result<&'static [u8], String> {
        let font_data = std::fs::read(path)
            .map_err(|e| format!("Failed to read font file {:?}: {}", path, e))?;
        
        // Leak the data to get 'static lifetime (necessary for ab_glyph)
        let static_data: &'static [u8] = Box::leak(font_data.into_boxed_slice());
        
        self.loaded_fonts.insert(variant, static_data);
        Ok(static_data)
    }
    
    /// Get fallback font data
    fn get_fallback_font(&mut self) -> Result<&'static [u8], String> {
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
        
        for path in fallback_paths {
            if let Ok(font_data) = std::fs::read(path) {
                let static_data: &'static [u8] = Box::leak(font_data.into_boxed_slice());
                self.fallback_font_data = Some(static_data);
                return Ok(static_data);
            }
        }
        
        Err("No fallback font available".to_string())
    }
    
    /// Get a FontRef for a specific XFA font style
    pub fn get_font(&mut self, xfa_font: &Font) -> Result<FontRef<'static>, String> {
        let variant = FontVariant::from_xfa_font(xfa_font);
        let data = self.get_font_data(&variant)?;
        
        FontRef::try_from_slice(data)
            .map_err(|e| format!("Failed to parse font: {}", e))
    }
    
    /// Get a FontRef using default XFA font settings
    pub fn get_default_font(&mut self) -> Result<FontRef<'static>, String> {
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
pub fn get_font_for_style(xfa_font: &Font) -> Result<FontRef<'static>, String> {
    let manager = get_font_manager();
    let mut manager = manager.lock().map_err(|e| format!("Lock error: {}", e))?;
    manager.get_font(xfa_font)
}

/// Convenience function to get the default fallback font
pub fn get_fallback_font() -> Result<FontRef<'static>, String> {
    let manager = get_font_manager();
    let mut manager = manager.lock().map_err(|e| format!("Lock error: {}", e))?;
    manager.get_default_font()
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
}

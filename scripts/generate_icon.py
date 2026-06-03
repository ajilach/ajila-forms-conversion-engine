"""
Generate the Forms Conversion Engine app icon.

Uses the exact same Lucide "file-cog" glyph as the in-app logo
(app/assets/app-icon.svg), rendered in the Ajila palette:
  navy #062544 (badge) · white file · gold #dc9e26 (cog).

Outputs icon.png / icon.ico / icon.icns into app/icons/ for the
Dioxus desktop bundle ([bundle] icon in Dioxus.toml).
"""
import cairosvg
from PIL import Image
import io
import os

PROJECT_ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
APP_DIR = os.path.join(PROJECT_ROOT, "app")

# Same glyph as app/assets/app-icon.svg (Lucide "file-cog").
SVG = """<?xml version="1.0" encoding="UTF-8"?>
<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 48 48" width="1024" height="1024">
  <rect x="0" y="0" width="48" height="48" rx="11" fill="#062544"/>
  <g transform="translate(10 10) scale(1.1667)" fill="none" stroke-width="2"
     stroke-linecap="round" stroke-linejoin="round">
    <g stroke="#ffffff">
      <path d="M15 8a1 1 0 0 1-1-1V2a2.4 2.4 0 0 1 1.704.706l3.588 3.588A2.4 2.4 0 0 1 20 8z"/>
      <path d="M20 8v12a2 2 0 0 1-2 2h-4.182"/>
      <path d="M4 10.592V4a2 2 0 0 1 2-2h8"/>
    </g>
    <g stroke="#dc9e26">
      <path d="m3.305 19.53.923-.382"/>
      <path d="m4.228 16.852-.924-.383"/>
      <path d="m5.852 15.228-.383-.923"/>
      <path d="m5.852 20.772-.383.924"/>
      <path d="m8.148 15.228.383-.923"/>
      <path d="m8.53 21.696-.382-.924"/>
      <path d="m9.773 16.852.922-.383"/>
      <path d="m9.773 19.148.922.383"/>
      <circle cx="7" cy="18" r="3"/>
    </g>
  </g>
</svg>
"""

png_data = cairosvg.svg2png(bytestring=SVG.encode(), output_width=1024, output_height=1024)

icons_dir = os.path.join(APP_DIR, "icons")
os.makedirs(icons_dir, exist_ok=True)

icon_png_path = os.path.join(icons_dir, "icon.png")
with open(icon_png_path, "wb") as f:
    f.write(png_data)

img = Image.open(io.BytesIO(png_data))
ico_sizes = [(16, 16), (32, 32), (48, 48), (64, 64), (128, 128), (256, 256)]
img.save(os.path.join(icons_dir, "icon.ico"), format="ICO", sizes=ico_sizes)

icns_img = Image.open(io.BytesIO(png_data)).convert("RGBA")
try:
    icns_img.save(os.path.join(icons_dir, "icon.icns"), format="ICNS")
except Exception as exc:  # pragma: no cover - environment dependent
    print(f"Warning: could not write icon.icns: {exc}")

print(f"Icons generated in: {icons_dir}/")

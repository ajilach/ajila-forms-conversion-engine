#!/usr/bin/env python3
from fontTools.ttLib import TTFont

font_path = "/Users/fboesiger/Library/Fonts/ftl____.ttf"
font = TTFont(font_path)

# Check head table for unitsPerEm
head = font['head']
print(f"unitsPerEm: {head.unitsPerEm}")

# Check OS/2 table
os2 = font['OS/2']
print(f"OS/2 weight class: {os2.usWeightClass}")
print(f"OS/2 width class: {os2.usWidthClass}")
print(f"OS/2 panose: {os2.panose}")
print(f"OS/2 ascender: {os2.sTypoAscender}")
print(f"OS/2 descender: {os2.sTypoDescender}")

# Check name table
name = font['name']
for record in name.names:
    if record.nameID in [1, 2, 4, 6]:  # family, style, full name, postscript
        try:
            print(f"name {record.nameID} (platform={record.platformID}): {record.toUnicode()}")
        except:
            pass

# Check key glyph advances
hmtx = font['hmtx']
cmap = font.getBestCmap()

chars = {'D': 68, 'i': 105, 'e': 101, 'B': 66, 'a': 97, 'r': 114, 'b': 98, 
         't': 116, 'u': 117, 'n': 110, 'g': 103, 'space': 32, 's': 115}

print(f"\nGlyph advances (in font units, unitsPerEm={head.unitsPerEm}):")
for name_str, code in sorted(chars.items(), key=lambda x: x[1]):
    if code in cmap:
        glyph_name = cmap[code]
        advance, lsb = hmtx[glyph_name]
        pt8 = advance * 8 / head.unitsPerEm
        print(f"  {name_str:8s} (U+{code:04X}): advance={advance:4d}/{head.unitsPerEm}em = {pt8:.3f}pt at 8pt")

# Compute "Die" width
die_chars = [68, 105, 101]  # D, i, e
die_advance = sum(hmtx[cmap[c]][0] for c in die_chars)
die_pt8 = die_advance * 8 / head.unitsPerEm
print(f'\n"Die" advance: {die_advance}/{head.unitsPerEm}em = {die_pt8:.3f}pt at 8pt')

# PDF comparison: Die = 11.11pt, our font = ?
print(f'PDF "Die" = 11.112pt')
print(f'Ratio: {die_pt8 / 11.112:.4f}')

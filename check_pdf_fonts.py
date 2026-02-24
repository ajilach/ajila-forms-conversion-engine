#!/usr/bin/env python3
import fitz
import re

doc = fitz.open("input/AAAI_019_DE.pdf")
obj = doc.xref_object(7, compressed=False)

wm = re.search(r"/Widths\s*\[([^\]]+)\]", obj)
if not wm:
    print("No widths match")
    print(obj[:500])
    exit(1)

w = [int(x) for x in wm.group(1).split()]
print(f"Width array length: {len(w)}")

# Print widths for key characters
chars = [
    (32, "space"), (46, "."), (65, "A"), (66, "B"), (68, "D"),
    (97, "a"), (98, "b"), (100, "d"), (101, "e"), (102, "f"),
    (103, "g"), (105, "i"), (110, "n"), (114, "r"), (115, "s"),
    (116, "t"), (117, "u"), (171, "laquo"), (187, "raquo"),
    (223, "eszett"), (228, "a-uml"), (246, "o-uml"), (252, "u-uml"),
]

print("\nPDF embedded Frutiger 45 Light widths (per 1000 em units):")
for code, name in chars:
    pt8 = w[code] * 8 / 1000
    print(f"  char {code:3d} ({name:8s}): {w[code]:4d}/1000em = {pt8:.3f}pt at 8pt")

# Compute word widths for "Die Bearbeitung..."
text = "Die Bearbeitung der seitens der Vertragsbank"
words = text.split()
for word in words:
    total = 0
    for ch in word:
        code = ord(ch)
        if code < len(w):
            total += w[code]
    pt = total * 8 / 1000
    print(f'  "{word}": {pt:.2f}pt (PDF)')

print(f"\nspace at 8pt = {w[32]*8/1000:.3f}pt")

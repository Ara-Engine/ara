"""Independent HarfBuzz glyph reference for the installed Segoe color emoji font."""
import json
import pathlib
import sys

ROOT = pathlib.Path(__file__).resolve().parent
sys.path.insert(0, str(ROOT / ".vendor/python"))
import uharfbuzz as hb

face = hb.Face(pathlib.Path("C:/Windows/Fonts/seguiemj.ttf").read_bytes())
font = hb.Font(face)
results = []
for value in ["👩🏽‍🚀", "👨‍👩‍👧‍👦", "🏳️‍🌈", "😀"]:
    buffer = hb.Buffer()
    buffer.add_str(value)
    buffer.guess_segment_properties()
    hb.shape(font, buffer)
    results.append({"text": value, "glyphs": [i.codepoint for i in buffer.glyph_infos],
                    "advances_font_units": [p.x_advance for p in buffer.glyph_positions]})
output = {"uharfbuzz": hb.__version__, "harfbuzz": hb.version_string(), "sequences": results}
(ROOT / "evidence/harfbuzz-reference.json").write_text(json.dumps(output, ensure_ascii=False, indent=2), encoding="utf-8")
print(json.dumps(output, ensure_ascii=True))

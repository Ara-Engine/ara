"""Create labelled review contact sheets from the story suite's actual PNG evidence.

Run after cargo test -p ara-ui-test: python crates/ara-ui-test/examples/review_gallery.py
Requires Pillow only for this optional review aid; the Rust suite has no Python dependency.
"""
import json
from pathlib import Path
from PIL import Image, ImageDraw, ImageFont

root = Path(__file__).resolve().parent.parent
book = json.loads((root / "examples/catalogue.stories").read_text(encoding="utf-8"))
stories = list(book["records"].values())
font = ImageFont.truetype(str(root.parent / "ara-ui-text/fonts/NotoSans.ttf"), 13)
cell_w, cell_h = 250, 230
def render_sheet(rows, page, suffix):
    sheet = Image.new("RGB", (cell_w * 6, cell_h * len(rows)), "#27272a")
    draw = ImageDraw.Draw(sheet)
    for row, story in enumerate(rows):
        for column, view in enumerate(story["views"]):
            source = root / f"evidence/{story['name']}-{column}{suffix}.png"
            image = Image.open(source).convert("RGB")
            image.thumbnail((cell_w - 8, cell_h - 40), Image.Resampling.LANCZOS)
            x, y = column * cell_w, row * cell_h
            sheet.paste(image, (x + 4, y + 36))
            label = f"{story['name']} / {view['locale']} / {view.get('theme', 'dark')}"
            draw.text((x + 4, y + 3), label, font=font, fill="white")
            draw.text((x + 4, y + 19), f"{view['width']}x{view['height']} @ {view['dpi']} DPI", font=font, fill="#a1a1aa")
    target = root / f"evidence/gallery{suffix}-{page // 10}.png"
    sheet.save(target)
    print(target)


interactive = [story for story in stories if any(
    step.get("kind") in {"activate", "text", "advance"} for step in story.get("steps", [])
)]
for suffix, selection in [("", stories), ("-after", interactive)]:
    for page in range(0, len(selection), 10):
        render_sheet(selection[page:page + 10], page, suffix)

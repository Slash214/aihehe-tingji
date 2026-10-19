"""Generate the application mark and native icon from vector primitives."""
from pathlib import Path
from PIL import Image, ImageDraw

root = Path(__file__).parent / "assets"
root.mkdir(exist_ok=True)
im = Image.new("RGBA", (256, 256))
mask = Image.new("L", im.size)
ImageDraw.Draw(mask).rounded_rectangle((0, 0, 255, 255), radius=58, fill=255)
for y in range(256):
    for x in range(256):
        t = (x + y) / 510
        im.putpixel((x, y), (int(184 - 66*t), int(159 - 66*t), int(248 - 57*t), mask.getpixel((x, y))))
draw = ImageDraw.Draw(im)
for x, height in [(60, 48), (94, 95), (128, 146), (162, 95), (196, 48)]:
    draw.rounded_rectangle((x-9, 128-height/2, x+9, 128+height/2), radius=9, fill=(250, 245, 255, 245))
im.save(root / "app.ico", sizes=[(16,16),(24,24),(32,32),(48,48),(64,64),(128,128),(256,256)])
im.save(root / "app.png")
(root / "icon.rgba").write_bytes(im.resize((64,64), Image.Resampling.LANCZOS).tobytes())

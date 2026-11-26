from PIL import Image, ImageDraw, ImageFont, ImageFilter
import random, math, os

os.makedirs("samples/assets", exist_ok=True)
os.makedirs("assets", exist_ok=True)
W, H = 1200, 620
img = Image.new("RGB", (W, H), "#f4efe6")
pix = img.load()
for y in range(H):
    t = y / (H - 1)
    r = int(246 - 18 * t)
    g = int(240 - 22 * t)
    b = int(229 - 24 * t)
    for x in range(W):
        pix[x, y] = (r, g, b)

d = ImageDraw.Draw(img)
# warm translucent shapes
blob = Image.new("RGBA", (W, H), (0, 0, 0, 0))
bd = ImageDraw.Draw(blob)
bd.ellipse((-120, 280, 620, 900), fill=(193, 101, 58, 38))
bd.ellipse((640, -260, 1380, 380), fill=(46, 107, 110, 30))
bd.ellipse((760, 360, 1240, 780), fill=(210, 158, 90, 34))
blob = blob.filter(ImageFilter.GaussianBlur(18))
img = Image.alpha_composite(img.convert("RGBA"), blob).convert("RGB")
d = ImageDraw.Draw(img)

# fine paper grain
rng = random.Random(7)
for _ in range(26000):
    x = rng.randrange(W)
    y = rng.randrange(H)
    c = rng.randrange(150, 240)
    d.point((x, y), fill=(c, c - 4, c - 9))

# thin frame and rule
d.rectangle((38, 38, W - 38, H - 38), outline=(120, 100, 80), width=1)
d.line((38, H - 160, W - 38, H - 160), fill=(120, 100, 80), width=1)

bold = ImageFont.truetype("C:/Windows/Fonts/segoeuib.ttf", 94)
small = ImageFont.truetype("C:/Windows/Fonts/segoeui.ttf", 27)
mono = ImageFont.truetype("C:/Windows/Fonts/Consola.ttf", 22)
d.text((84, 92), "Rusk", font=bold, fill=(43, 40, 35))
d.text((88, 214), "The source is the document.", font=small, fill=(120, 93, 72))
d.text((88, H - 123), "MARKDOWN  /  NATIVE  /  SOURCE-FIRST", font=mono, fill=(125, 108, 88))
d.text((W - 210, H - 123), "0.1", font=mono, fill=(125, 108, 88))
img.save("samples/assets/cover.png", optimize=True)

# App icon
icon = Image.new("RGBA", (256, 256), (0, 0, 0, 0))
di = ImageDraw.Draw(icon)
di.rounded_rectangle((8, 8, 248, 248), radius=58, fill=(177, 88, 50, 255))
di.rounded_rectangle((8, 8, 248, 248), radius=58, outline=(140, 64, 34, 255), width=4)
font = ImageFont.truetype("C:/Windows/Fonts/segoeuib.ttf", 150)
bbox = di.textbbox((0, 0), "R", font=font)
di.text(((256 - (bbox[2] - bbox[0])) / 2 - bbox[0], (256 - (bbox[3] - bbox[1])) / 2 - bbox[1] - 8), "R", font=font, fill=(255, 250, 244, 255))
icon.save("assets/icon.png")
print("done")

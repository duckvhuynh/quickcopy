"""Builds the .ico files embedded in the exe (assets/quickcopy.rc) from the
1024px PNG masters in assets/.

    python tools/icons.py

Each master is cropped to its drawn pixels, centered on a square with a small
margin, and saved with every size Windows asks for (title bar, taskbar,
Explorer, the notification area at 100-200% scale).
"""

from pathlib import Path

from PIL import Image

ASSETS = Path(__file__).resolve().parent.parent / "assets"

APP_SIZES = [16, 20, 24, 32, 40, 48, 64, 96, 128, 256]
TRAY_SIZES = [16, 20, 24, 32, 40, 48, 64]


def squared(img: Image.Image, margin: float) -> Image.Image:
    img = img.convert("RGBA")
    box = img.getchannel("A").point(lambda a: 255 if a > 8 else 0).getbbox()
    if box:
        img = img.crop(box)
    side = round(max(img.size) * (1 + 2 * margin))
    out = Image.new("RGBA", (side, side), (0, 0, 0, 0))
    out.paste(img, ((side - img.width) // 2, (side - img.height) // 2))
    return out


def build(src: str, dst: str, sizes: list[int], margin: float) -> None:
    master = squared(Image.open(ASSETS / src), margin)
    frames = [master.resize((s, s), Image.LANCZOS) for s in sizes]
    frames[-1].save(ASSETS / dst, format="ICO", sizes=[(s, s) for s in sizes], append_images=frames[:-1])
    print(f"{dst}: {sizes}")


if __name__ == "__main__":
    build("icon.png", "icon.ico", APP_SIZES, 0.02)
    build("tray-light.png", "tray-white.ico", TRAY_SIZES, 0.04)
    build("tray-dark.png", "tray-black.ico", TRAY_SIZES, 0.04)

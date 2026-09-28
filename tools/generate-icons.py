"""Export the SVG master: python -m pip install resvg-py Pillow."""
from pathlib import Path

from PIL import Image
import resvg_py


assets = Path(__file__).resolve().parents[1] / "assets"
png = resvg_py.svg_to_bytes(svg_string=(assets / "tshell.svg").read_text(), width=256, height=256)
(assets / "tshell.png").write_bytes(png)
with Image.open(assets / "tshell.png") as image:
    image.save(assets / "tshell.ico", sizes=[(size, size) for size in (16, 20, 24, 32, 40, 48, 64, 128, 256)])

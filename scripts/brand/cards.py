"""Render the share images from the site's own fonts, icon and dashboard
screenshots, with headless Chrome:

    python3 scripts/brand/cards.py

writes marketing/images/og.png (1200x630) and the social images in
social/media/ (<name>-16x9.png at 1600x900, <name>-square.png at 1200x1200).
The menu-bar art is cut from the existing menubar-16x9.png, since the icon
itself hasn't changed. Run it after gen.py or after recapturing the
dashboard screenshots (marketing/images/tab-*.webp).
"""

import http.server
import shutil
import subprocess
import tempfile
import threading
from functools import partial
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
SITE = ROOT / "marketing"
MEDIA = ROOT / "social" / "media"
CHROME = "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome"

CSS = """
@font-face{font-family:Geist;src:url(/fonts/Geist-Regular.ttf);font-weight:400}
@font-face{font-family:Geist;src:url(/fonts/Geist-Medium.ttf);font-weight:500}
@font-face{font-family:GeistMono;src:url(/fonts/GeistMono-Regular.ttf)}
html,body{margin:0;overflow:hidden;background:#0b0b0b;font-family:Geist;color:#f2f2f2}
body{position:relative;background-image:linear-gradient(#ffffff07 1px,transparent 1px),
  linear-gradient(90deg,#ffffff07 1px,transparent 1px);background-size:48px 48px}
.glow{position:absolute;inset:0;background:radial-gradient(55% 65% at 75% 40%,rgba(79,207,125,.09),transparent 70%)}
.text{position:absolute;display:flex;flex-direction:column;gap:22px}
.brand{display:flex;align-items:center;gap:14px;font-weight:500}
h1{margin:0;font-weight:500;letter-spacing:-1.4px;line-height:1.06}
p{margin:0;color:#a8a8a8;line-height:1.4}
.url{font-family:GeistMono;color:#8d8d8d}
.art{position:absolute;overflow:hidden}
.art img{display:block;width:100%;border-radius:6px}
.fade{-webkit-mask-image:linear-gradient(to bottom,#000 85%,transparent)}
"""

# name: (headline, line, art image under marketing/)
CARDS = {
    "menubar": ("Menu bar", "Download and upload rates, the last 24 samples, and how full the disk is.", "_menubar.png"),
    "overview": ("Overview", "The last two minutes of traffic, the current rates, storage and keep awake.", "images/tab-overview.webp"),
    "network": ("Speed test", "Download and upload speed, and latency when idle and under load.", "images/tab-network.webp"),
    "storage": ("Disk usage", "Used space on the startup disk, kept for up to 30 days.", "images/tab-storage.webp"),
}


def page(w, h, title, line, art, *, sizes, text_box, art_box, fade):
    brand_px, h1_px, p_px, url_px, icon_px = sizes
    return f"""<!doctype html><html><head><style>{CSS}
html,body{{width:{w}px;height:{h}px}}
.brand{{font-size:{brand_px}px}} .brand img{{width:{icon_px}px;height:{icon_px}px}}
h1{{font-size:{h1_px}px}} p{{font-size:{p_px}px}} .url{{font-size:{url_px}px}}
</style></head><body><div class="glow"></div>
<div class="text" style="{text_box}"><div class="brand"><img src="/images/icon.webp">Utilify</div>
<h1>{title}</h1><p>{line}</p><div class="url">utilify.imran.codes</div></div>
<div class="art{' fade' if fade else ''}" style="{art_box}"><img src="/{art}"></div></body></html>"""


def shoot(port, html, w, h, out):
    tmp = SITE / "_card.html"
    tmp.write_text(html)
    try:
        subprocess.run(
            [CHROME, "--headless=new", "--disable-gpu", "--hide-scrollbars",
             f"--window-size={w},{h}", "--virtual-time-budget=2000",
             f"--screenshot={out}", f"http://localhost:{port}/_card.html"],
            check=True, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL,
        )
    finally:
        tmp.unlink(missing_ok=True)
    print(out.relative_to(ROOT))


class Quiet(http.server.SimpleHTTPRequestHandler):
    def log_message(self, *args):
        pass


def main():
    handler = partial(Quiet, directory=str(SITE))
    server = http.server.ThreadingHTTPServer(("127.0.0.1", 0), handler)
    port = server.server_address[1]
    threading.Thread(target=server.serve_forever, daemon=True).start()

    # The menu-bar art, cut from the last render (sips: height width).
    menubar = SITE / "_menubar.png"
    with tempfile.TemporaryDirectory() as d:
        src = Path(d) / "src.png"
        shutil.copy(MEDIA / "menubar-16x9.png", src)
        subprocess.run(["sips", "-c", "140", "720", "--cropOffset", "380", "760", str(src),
                        "--out", str(menubar)], check=True, stdout=subprocess.DEVNULL)
    try:
        shoot(port, page(
            1200, 630, "Menu-bar utilities for macOS",
            "Network and storage at a glance, keep awake, and fan control. Free and open source.",
            "images/tab-overview.webp", sizes=(22, 52, 22, 17, 28),
            text_box="left:96px;top:0;bottom:0;width:560px;justify-content:center",
            art_box="right:96px;top:48px;width:320px;height:582px", fade=True,
        ), 1200, 630, SITE / "images" / "og.png")
        for name, (title, line, art) in CARDS.items():
            bar = art == "_menubar.png"
            shoot(port, page(
                1600, 900, title, line, art, sizes=(28, 64, 26, 22, 36),
                text_box="left:128px;top:0;bottom:0;width:520px;justify-content:center",
                art_box="left:770px;top:380px;width:720px" if bar
                else "right:146px;top:36px;width:412px;height:828px", fade=not bar,
            ), 1600, 900, MEDIA / f"{name}-16x9.png")
            shoot(port, page(
                1200, 1200, title, line, art, sizes=(28, 64, 26, 22, 36),
                text_box="left:96px;top:96px;width:900px",
                art_box="left:240px;top:620px;width:720px" if bar
                else "left:355px;top:436px;width:490px;height:764px", fade=not bar,
            ), 1200, 1200, MEDIA / f"{name}-square.png")
    finally:
        menubar.unlink(missing_ok=True)
        server.shutdown()


if __name__ == "__main__":
    main()

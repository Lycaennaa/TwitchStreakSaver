#!/usr/bin/env python3
"""Generate S1 (Dark Glass Shield Crest) icon assets for macOS, Windows, tray, and web extensions."""

from __future__ import annotations

import math
import shutil
import struct
import subprocess
import tempfile
from pathlib import Path

from PIL import Image

ROOT = Path(__file__).resolve().parents[1]
ASSETS_DIR = ROOT / "assets" / "icons"
MACOS_DIR = ROOT / "packaging" / "macos"
WINDOWS_DIR = ROOT / "packaging" / "windows"
EXT_ICONS_DIR = ROOT / "web-extension" / "icons"

SHIELD_PATH = (
    "M 512 198 "
    "C 608 232, 706 250, 782 262 "
    "C 796 264, 804 276, 804 290 "
    "L 804 476 "
    "C 804 654, 680 778, 526 832 "
    "C 517 835, 507 835, 498 832 "
    "C 344 778, 220 654, 220 476 "
    "L 220 290 "
    "C 220 276, 228 264, 242 262 "
    "C 318 250, 416 232, 512 198 Z"
)

B2_FLAME = (
    "M 556 208 "
    "C 554 282, 646 334, 690 434 "
    "C 728 520, 718 634, 652 708 "
    "C 600 766, 502 790, 420 764 "
    "C 328 734, 280 644, 292 542 "
    "C 300 474, 336 414, 372 366 "
    "C 374 414, 394 442, 416 452 "
    "C 408 376, 448 304, 502 254 "
    "C 524 234, 542 220, 556 208 Z"
)

PALETTE = {
    "active": {
        "tile": ("#9E4BFF", "#6C25DC", "#340C75"),
        "flame": ("#FFE259", "#FF9017", "#EB471E"),
        "symbol": ("#FFFFFF", "#FFF6E5"),
        "shield_dark": ("#3B1178", "#22074D"),
        "shield_rim": ("#F5EAFF", "#C492FF"),
    },
    "paused": {
        "tile": ("#67568C", "#473968", "#281D40"),
        "flame": ("#D6C2A3", "#B59C7D", "#8F7860"),
        "symbol": ("#F5F2FA", "#DCD5E8"),
        "shield_dark": ("#34284E", "#201833"),
        "shield_rim": ("#CFC3E6", "#9787B8"),
    },
    "disconnected": {
        "tile": ("#5A5D6B", "#3E414D", "#24262F"),
        "flame": ("#C6C9D4", "#9DA1B0", "#767B8A"),
        "symbol": ("#EAECEF", "#D2D5DE"),
        "shield_dark": ("#333642", "#20222A"),
        "shield_rim": ("#C5C9D4", "#8D92A1"),
    },
}


def squircle_path(x: float, y: float, size: float, radius: float) -> str:
    x1 = x + size
    y1 = y + size
    k = radius * 0.44
    return (
        f"M {x + radius:.1f} {y:.1f} "
        f"L {x1 - radius:.1f} {y:.1f} "
        f"C {x1 - k:.1f} {y:.1f} {x1:.1f} {y + k:.1f} {x1:.1f} {y + radius:.1f} "
        f"L {x1:.1f} {y1 - radius:.1f} "
        f"C {x1:.1f} {y1 - k:.1f} {x1 - k:.1f} {y1:.1f} {x1 - radius:.1f} {y1:.1f} "
        f"L {x + radius:.1f} {y1:.1f} "
        f"C {x + k:.1f} {y1:.1f} {x:.1f} {y1 - k:.1f} {x:.1f} {y1 - radius:.1f} "
        f"L {x:.1f} {y + radius:.1f} "
        f"C {x:.1f} {y + k:.1f} {x + k:.1f} {y:.1f} {x + radius:.1f} {y:.1f} Z"
    )


def play_path(cx: float, cy: float, scale: float) -> str:
    pts = [(-64.0, -90.0), (96.0, 0.0), (-64.0, 90.0)]
    radius = 24.0
    segs: list[str] = []
    for i in range(3):
        prev_pt = pts[(i + 2) % 3]
        curr_pt = pts[i]
        next_pt = pts[(i + 1) % 3]
        d1 = math.hypot(prev_pt[0] - curr_pt[0], prev_pt[1] - curr_pt[1])
        d2 = math.hypot(next_pt[0] - curr_pt[0], next_pt[1] - curr_pt[1])
        p1 = (
            curr_pt[0] + (prev_pt[0] - curr_pt[0]) * (radius / d1),
            curr_pt[1] + (prev_pt[1] - curr_pt[1]) * (radius / d1),
        )
        p2 = (
            curr_pt[0] + (next_pt[0] - curr_pt[0]) * (radius / d2),
            curr_pt[1] + (next_pt[1] - curr_pt[1]) * (radius / d2),
        )
        cmd = "M" if i == 0 else "L"
        segs.append(
            f"{cmd} {cx + p1[0] * scale:.1f} {cy + p1[1] * scale:.1f} "
            f"Q {cx + curr_pt[0] * scale:.1f} {cy + curr_pt[1] * scale:.1f} "
            f"{cx + p2[0] * scale:.1f} {cy + p2[1] * scale:.1f}"
        )
    return " ".join(segs) + " Z"


def pause_bars(cx: float, cy: float, scale: float) -> str:
    w = 44.0 * scale
    h = 154.0 * scale
    gap = 36.0 * scale
    rx = 18.0 * scale
    x1 = cx - gap / 2.0 - w
    x2 = cx + gap / 2.0
    y = cy - h / 2.0
    return (
        f'<rect x="{x1:.1f}" y="{y:.1f}" width="{w:.1f}" height="{h:.1f}" rx="{rx:.1f}" fill="url(#sym)" />'
        f'<rect x="{x2:.1f}" y="{y:.1f}" width="{w:.1f}" height="{h:.1f}" rx="{rx:.1f}" fill="url(#sym)" />'
    )


def build_svg(state: str, *, full_bleed: bool, simplified: bool) -> str:
    colors = PALETTE[state]
    tile = (24, 24, 976, 224) if full_bleed else (100, 100, 824, 186)
    tile_path = squircle_path(*tile)
    bevel_path = squircle_path(tile[0] + 6, tile[1] + 6, tile[2] - 12, tile[3] - 4)

    if simplified:
        outer_scale = "translate(512 512) scale(1.16) translate(-512 -512)"
    elif full_bleed:
        outer_scale = "translate(512 512) scale(1.06) translate(-512 -512)"
    else:
        outer_scale = "translate(512 512) scale(0.93) translate(-512 -512)"

    sym_scale = 1.04 * (1.20 if simplified else 1.0)
    if state == "paused":
        sym_markup = pause_bars(512.0, 582.0, sym_scale * (1.04 if simplified else 1.0))
    else:
        sym_x = 512.0 + (10.0 * sym_scale if state == "active" else 0.0)
        sym_filter = ' filter="url(#sym_shadow)"' if not simplified else ""
        sym_markup = f'<path d="{play_path(sym_x, 582.0, sym_scale)}" fill="url(#sym)"{sym_filter} />'

    dot_pos = 792 if full_bleed else 756
    offline_badge = ""
    if state == "disconnected":
        outer_r = 156 if simplified else 114
        inner_r = 110 if simplified else 80
        offline_badge = (
            f'  <circle cx="{dot_pos}" cy="{dot_pos}" r="{outer_r}" fill="#1E2028" />\n'
            f'  <circle cx="{dot_pos}" cy="{dot_pos}" r="{inner_r}" fill="#F43F5E" />\n'
        )

    filters = ""
    if not simplified:
        filters = """    <filter id="tile_shadow" x="-14%" y="-10%" width="128%" height="132%">
      <feDropShadow dx="0" dy="22" stdDeviation="24" flood-color="#0B0418" flood-opacity="0.44" />
    </filter>
    <filter id="shield_shadow" x="-20%" y="-20%" width="140%" height="145%">
      <feDropShadow dx="0" dy="18" stdDeviation="22" flood-color="#170436" flood-opacity="0.48" />
    </filter>
    <filter id="sym_shadow" x="-30%" y="-30%" width="160%" height="160%">
      <feDropShadow dx="0" dy="8" stdDeviation="10" flood-color="#8A2400" flood-opacity="0.34" />
    </filter>
"""

    tile_filter = ' filter="url(#tile_shadow)"' if (not simplified and not full_bleed) else ""
    shield_filter = ' filter="url(#shield_shadow)"' if not simplified else ""
    top_shine_opacity = "0.28" if state == "active" else "0.16"
    bevel_markup = (
        f'    <path d="{bevel_path}" fill="none" stroke="url(#bevel)" stroke-width="10" />\n'
        if not simplified
        else ""
    )
    shield_gleam = (
        f'    <path d="{SHIELD_PATH}" fill="url(#shield_gleam)" />\n'
        if not simplified
        else ""
    )
    flame_core = (
        f'      <path d="{B2_FLAME}" fill="url(#flame_core)" />\n'
        if (not simplified and state == "active")
        else ""
    )
    rim_width = 42 if simplified else 30

    return f"""<svg xmlns="http://www.w3.org/2000/svg" width="1024" height="1024" viewBox="0 0 1024 1024">
  <defs>
    <linearGradient id="tile" x1="16%" y1="6%" x2="86%" y2="96%">
      <stop offset="0%" stop-color="{colors['tile'][0]}" />
      <stop offset="52%" stop-color="{colors['tile'][1]}" />
      <stop offset="100%" stop-color="{colors['tile'][2]}" />
    </linearGradient>
    <radialGradient id="topshine" cx="50%" cy="14%" r="62%">
      <stop offset="0%" stop-color="#FFFFFF" stop-opacity="{top_shine_opacity}" />
      <stop offset="100%" stop-color="#FFFFFF" stop-opacity="0" />
    </radialGradient>
    <linearGradient id="bevel" x1="0%" y1="0%" x2="0%" y2="100%">
      <stop offset="0%" stop-color="#FFFFFF" stop-opacity="0.34" />
      <stop offset="45%" stop-color="#FFFFFF" stop-opacity="0.06" />
      <stop offset="100%" stop-color="#000000" stop-opacity="0.28" />
    </linearGradient>
    <linearGradient id="shield_dark" x1="30%" y1="10%" x2="70%" y2="96%">
      <stop offset="0%" stop-color="{colors['shield_dark'][0]}" />
      <stop offset="100%" stop-color="{colors['shield_dark'][1]}" />
    </linearGradient>
    <linearGradient id="shield_rim" x1="20%" y1="0%" x2="80%" y2="100%">
      <stop offset="0%" stop-color="{colors['shield_rim'][0]}" />
      <stop offset="100%" stop-color="{colors['shield_rim'][1]}" />
    </linearGradient>
    <linearGradient id="shield_gleam" x1="0%" y1="0%" x2="100%" y2="100%">
      <stop offset="0%" stop-color="#FFFFFF" stop-opacity="0.16" />
      <stop offset="48%" stop-color="#FFFFFF" stop-opacity="0.02" />
      <stop offset="100%" stop-color="#FFFFFF" stop-opacity="0" />
    </linearGradient>
    <linearGradient id="flame" x1="45%" y1="12%" x2="55%" y2="96%">
      <stop offset="0%" stop-color="{colors['flame'][0]}" />
      <stop offset="54%" stop-color="{colors['flame'][1]}" />
      <stop offset="100%" stop-color="{colors['flame'][2]}" />
    </linearGradient>
    <radialGradient id="flame_core" cx="48%" cy="32%" r="54%">
      <stop offset="0%" stop-color="#FFFFFF" stop-opacity="0.36" />
      <stop offset="100%" stop-color="#FFFFFF" stop-opacity="0" />
    </radialGradient>
    <linearGradient id="sym" x1="20%" y1="10%" x2="80%" y2="92%">
      <stop offset="0%" stop-color="{colors['symbol'][0]}" />
      <stop offset="100%" stop-color="{colors['symbol'][1]}" />
    </linearGradient>
{filters}    <clipPath id="tile_clip">
      <path d="{tile_path}" />
    </clipPath>
  </defs>
  <path d="{tile_path}" fill="url(#tile)"{tile_filter} />
  <g clip-path="url(#tile_clip)">
    <rect width="1024" height="1024" fill="url(#topshine)" />
{bevel_markup}  </g>
  <g transform="{outer_scale}"{shield_filter}>
    <path d="{SHIELD_PATH}" fill="url(#shield_dark)" stroke="url(#shield_rim)" stroke-width="{rim_width}" stroke-linejoin="round" />
{shield_gleam}    <g transform="translate(512 508) scale(0.76) translate(-512 -508)">
      <path d="{B2_FLAME}" fill="url(#flame)" />
{flame_core}      {sym_markup}
    </g>
  </g>
{offline_badge}</svg>
"""


def render_png(svg_path: Path, png_path: Path, size: int) -> None:
    rsvg = shutil.which("rsvg-convert")
    if rsvg is None:
        raise SystemExit("rsvg-convert is required to rasterize SVG icons.")
    png_path.parent.mkdir(parents=True, exist_ok=True)
    subprocess.run(
        [rsvg, "-w", str(size), "-h", str(size), str(svg_path), "-o", str(png_path)],
        check=True,
    )


def write_rgba(png_path: Path, rgba_path: Path, size: int) -> None:
    with Image.open(png_path) as image:
        rgba = image.convert("RGBA")
        if rgba.size != (size, size):
            raise ValueError(f"Expected {size}x{size} image for {png_path}, got {rgba.size}")
        rgba_path.write_bytes(rgba.tobytes())


def main() -> None:
    ASSETS_DIR.mkdir(parents=True, exist_ok=True)
    MACOS_DIR.mkdir(parents=True, exist_ok=True)
    WINDOWS_DIR.mkdir(parents=True, exist_ok=True)
    EXT_ICONS_DIR.mkdir(parents=True, exist_ok=True)

    svgs = {
        "macos": ASSETS_DIR / "app-icon-macos.svg",
        "active": ASSETS_DIR / "app-icon-active.svg",
        "active_16": ASSETS_DIR / "app-icon-active-16.svg",
        "paused_16": ASSETS_DIR / "app-icon-paused-16.svg",
        "disconnected_16": ASSETS_DIR / "app-icon-disconnected-16.svg",
    }
    svgs["macos"].write_text(build_svg("active", full_bleed=False, simplified=False), encoding="utf-8")
    svgs["active"].write_text(build_svg("active", full_bleed=True, simplified=False), encoding="utf-8")
    svgs["active_16"].write_text(build_svg("active", full_bleed=True, simplified=True), encoding="utf-8")
    svgs["paused_16"].write_text(build_svg("paused", full_bleed=True, simplified=True), encoding="utf-8")
    svgs["disconnected_16"].write_text(build_svg("disconnected", full_bleed=True, simplified=True), encoding="utf-8")

    # Browser extension PNG icons
    render_png(svgs["active_16"], EXT_ICONS_DIR / "icon-16.png", 16)
    render_png(svgs["active_16"], EXT_ICONS_DIR / "icon-32.png", 32)
    render_png(svgs["active"], EXT_ICONS_DIR / "icon-48.png", 48)
    render_png(svgs["active"], EXT_ICONS_DIR / "icon-128.png", 128)
    render_png(svgs["paused_16"], EXT_ICONS_DIR / "icon-paused-16.png", 16)
    render_png(svgs["paused_16"], EXT_ICONS_DIR / "icon-paused-32.png", 32)
    render_png(svgs["disconnected_16"], EXT_ICONS_DIR / "icon-disconnected-16.png", 16)
    render_png(svgs["disconnected_16"], EXT_ICONS_DIR / "icon-disconnected-32.png", 32)

    with tempfile.TemporaryDirectory(prefix="tss-icons-") as tmp_str:
        tmp = Path(tmp_str)

        # Raw RGBA assets embedded in Rust binary (window icon + 2x Retina tray icons)
        window_png = tmp / "window-64.png"
        render_png(svgs["active"], window_png, 64)
        write_rgba(window_png, ASSETS_DIR / "window-icon-64.rgba", 64)

        for state, svg_key in (
            ("active", "active_16"),
            ("paused", "paused_16"),
            ("disconnected", "disconnected_16"),
        ):
            tray_png = tmp / f"tray-{state}-32.png"
            render_png(svgs[svg_key], tray_png, 32)
            write_rgba(tray_png, ASSETS_DIR / f"tray-{state}-32.rgba", 32)

        # Windows multi-resolution .ico
        ico_sizes = [16, 24, 32, 48, 64, 128, 256]
        ico_frames: list[Image.Image] = []
        for size in ico_sizes:
            source = svgs["active_16"] if size <= 24 else svgs["active"]
            frame_png = tmp / f"win-{size}.png"
            render_png(source, frame_png, size)
            with Image.open(frame_png) as img:
                ico_frames.append(img.convert("RGBA").copy())
        ico_frames[-1].save(
            WINDOWS_DIR / "app.ico",
            format="ICO",
            append_images=ico_frames[:-1],
            sizes=[(s, s) for s in ico_sizes],
        )
        (WINDOWS_DIR / "app.rc").write_text('1 ICON "app.ico"\n', encoding="utf-8")

        # macOS .icns (standard Apple PNG-packed chunks; cross-platform and validated by iconutil/sips)
        icns_entries = [
            (b"icp4", 16, svgs["active_16"]),
            (b"ic11", 32, svgs["macos"]),
            (b"icp5", 32, svgs["macos"]),
            (b"ic12", 64, svgs["macos"]),
            (b"ic07", 128, svgs["macos"]),
            (b"ic13", 256, svgs["macos"]),
            (b"ic08", 256, svgs["macos"]),
            (b"ic14", 512, svgs["macos"]),
            (b"ic09", 512, svgs["macos"]),
            (b"ic10", 1024, svgs["macos"]),
        ]
        icns_chunks: list[bytes] = []
        for tag, size, source in icns_entries:
            chunk_png = tmp / f"icns-{tag.decode('ascii')}-{size}.png"
            render_png(source, chunk_png, size)
            data = chunk_png.read_bytes()
            icns_chunks.append(tag + struct.pack(">I", len(data) + 8) + data)
        icns_body = b"".join(icns_chunks)
        (MACOS_DIR / "AppIcon.icns").write_bytes(
            b"icns" + struct.pack(">I", len(icns_body) + 8) + icns_body
        )

    print("Generated all S1 icon assets.")


if __name__ == "__main__":
    main()

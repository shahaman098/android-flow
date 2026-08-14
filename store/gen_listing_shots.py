#!/usr/bin/env python3
"""Play listing screenshots that match Sprout Hub + bubble UI (no emulator)."""

from pathlib import Path

from PIL import Image, ImageDraw, ImageFilter, ImageFont

ROOT = Path(__file__).resolve().parent
OUT = ROOT / "screenshots"
ICON = ROOT / "icon-512.png"

TEAL = (45, 212, 191, 255)
TEAL_DEEP = (15, 118, 110, 255)
TEAL_BUBBLE = (13, 148, 136, 255)
INK = (7, 16, 24, 255)
PANEL = (15, 28, 46, 204)
LINE = (255, 255, 255, 51)
MIST = (148, 163, 184, 255)
SNOW = (248, 250, 252, 255)
AMBER = (251, 191, 36, 255)
STOP_RED = (127, 29, 29, 255)

FONT_REG = "/System/Library/Fonts/Supplemental/Arial.ttf"
FONT_BOLD = "/System/Library/Fonts/Supplemental/Arial Bold.ttf"
FONT_UNI = "/Library/Fonts/Arial Unicode.ttf"


def font(size, bold=False, uni=False):
    path = FONT_UNI if uni else (FONT_BOLD if bold else FONT_REG)
    return ImageFont.truetype(path, size)


def rr(draw, box, r, fill=None, outline=None, width=1):
    draw.rounded_rectangle(box, radius=r, fill=fill, outline=outline, width=width)


def text_w(draw, text, fnt):
    return draw.textbbox((0, 0), text, font=fnt)[2]


def wrap(draw, text, fnt, max_w):
    words = text.split()
    lines, cur = [], ""
    for w in words:
        trial = (cur + " " + w).strip()
        if text_w(draw, trial, fnt) <= max_w:
            cur = trial
        else:
            if cur:
                lines.append(cur)
            cur = w
    if cur:
        lines.append(cur)
    return lines


def status_bar(draw, w, t="18:39"):
    fnt = font(28, bold=True)
    draw.text((40, 22), t, font=fnt, fill=SNOW)
    # battery / signal dots
    x = w - 48
    draw.rounded_rectangle((x - 52, 28, x - 8, 50), 4, outline=SNOW, width=2)
    draw.rectangle((x - 22, 32, x - 12, 46), fill=TEAL)
    draw.ellipse((x - 8, 34, x - 4, 44), fill=SNOW)


def glow_bg(w, h):
    img = Image.new("RGBA", (w, h), INK)
    overlay = Image.new("RGBA", (w, h), (0, 0, 0, 0))
    d = ImageDraw.Draw(overlay)
    d.ellipse((w - 420, -80, w + 80, 420), fill=(15, 118, 110, 90))
    d.ellipse((-160, h - 380, 280, h + 80), fill=(3, 105, 161, 70))
    overlay = overlay.filter(ImageFilter.GaussianBlur(70))
    return Image.alpha_composite(img, overlay)


def draw_mic(draw, cx, cy, s=22, fill=(255, 255, 255, 255)):
    # capsule
    draw.rounded_rectangle((cx - s * 0.28, cy - s * 0.55, cx + s * 0.28, cy + s * 0.15), s * 0.28, fill=fill)
    draw.arc((cx - s * 0.55, cy - s * 0.25, cx + s * 0.55, cy + s * 0.55), 0, 180, fill=fill, width=max(3, s // 8))
    draw.line((cx, cy + s * 0.55, cx, cy + s * 0.85), fill=fill, width=max(3, s // 8))
    draw.line((cx - s * 0.28, cy + s * 0.85, cx + s * 0.28, cy + s * 0.85), fill=fill, width=max(3, s // 8))


def card(base, box, r=36):
    layer = Image.new("RGBA", base.size, (0, 0, 0, 0))
    d = ImageDraw.Draw(layer)
    rr(d, box, r, fill=PANEL, outline=LINE, width=2)
    return Image.alpha_composite(base, layer)


def check_row(draw, x, y, w, label, done=True):
    rr(draw, (x, y, x + w, y + 78), 22, fill=(255, 255, 255, 20))
    draw.ellipse((x + 18, y + 22, x + 54, y + 58), fill=TEAL_DEEP)
    fnt = font(28)
    draw.text((x + 72, y + 22), label, font=font(30, bold=True), fill=SNOW)
    if done:
        # check
        cx, cy = x + w - 40, y + 39
        draw.ellipse((cx - 16, cy - 16, cx + 16, cy + 16), fill=TEAL)
        draw.line((cx - 8, cy, cx - 2, cy + 7), fill=INK, width=3)
        draw.line((cx - 2, cy + 7, cx + 9, cy - 7), fill=INK, width=3)
    else:
        fnt_s = font(24, bold=True)
        tw = text_w(draw, "Enable", fnt_s)
        draw.text((x + w - 28 - tw, y + 24), "Enable", font=fnt_s, fill=AMBER)
    return y + 90


def hub(w=1080, h=1920, bubble_on=False):
    img = glow_bg(w, h)
    d = ImageDraw.Draw(img)
    status_bar(d, w)
    x, y = 66, 96
    inner = w - 132

    d.text((x, y), "Sprout", font=font(92, bold=True), fill=SNOW)
    y += 118
    tag = "Speak into any app. Hold the bubble to dictate. Tap for vibe prompts and grammar."
    for line in wrap(d, tag, font(34), inner):
        d.text((x, y), line, font=font(34), fill=MIST)
        y += 44
    y += 28

    # Get ready
    img = card(img, (x, y, x + inner, y + 470), 42)
    d = ImageDraw.Draw(img)
    d.text((x + 40, y + 32), "Get ready", font=font(42, bold=True), fill=SNOW)
    ry = y + 98
    for label, done in [
        ("Microphone", True),
        ("Floating bubble", True),
        ("Accessibility insert", True),
        ("Your flow-api", True),
    ]:
        ry = check_row(d, x + 28, ry, inner - 56, label, done)

    y += 494
    # Connection
    conn_h = 430
    img = card(img, (x, y, x + inner, y + conn_h), 42)
    d = ImageDraw.Draw(img)
    d.text((x + 40, y + 32), "Connection", font=font(42, bold=True), fill=SNOW)
    sub = "You must paste your own FLOW_API_URL and FLOW_API_KEY."
    sy = y + 88
    for line in wrap(d, sub, font(26), inner - 80):
        d.text((x + 40, sy), line, font=font(26), fill=MIST)
        sy += 34

    def field(fy, label, value):
        rr(d, (x + 36, fy, x + inner - 36, fy + 88), 22, fill=(0, 0, 0, 34), outline=TEAL, width=2)
        d.text((x + 56, fy + 8), label, font=font(20), fill=TEAL)
        d.text((x + 56, fy + 40), value, font=font(28), fill=SNOW)

    field(y + 168, "FLOW_API_URL", "https://flow-api-….run.app")
    field(y + 272, "FLOW_API_KEY", "••••••••••••••••")

    y += conn_h + 24
    # Bubble card
    bh = 280
    img = card(img, (x, y, x + inner, y + bh), 42)
    d = ImageDraw.Draw(img)
    d.text((x + 40, y + 32), "Bubble", font=font(42, bold=True), fill=SNOW)
    hint = "Hold bubble → dictate · Tap → vibe / grammar · Drag to move"
    hy = y + 92
    for line in wrap(d, hint, font(28), inner - 80):
        d.text((x + 40, hy), line, font=font(28), fill=MIST)
        hy += 38
    btn_y = y + bh - 90
    fill = STOP_RED if bubble_on else TEAL
    fg = SNOW if bubble_on else INK
    rr(d, (x + 36, btn_y, x + inner - 36, btn_y + 70), 24, fill=fill)
    label = "Stop bubble" if bubble_on else "Launch floating bubble"
    fnt = font(32, bold=True)
    tw = text_w(d, label, fnt)
    d.text((x + (inner - tw) / 2, btn_y + 18), label, font=fnt, fill=fg)
    return img.convert("RGB")


def notes_bg(w, h, body_lines, listening=False, panel=False):
    img = Image.new("RGBA", (w, h), (245, 246, 248, 255))
    d = ImageDraw.Draw(img)
    # light status
    d.rectangle((0, 0, w, 72), fill=(255, 255, 255, 255))
    d.text((40, 22), "18:39", font=font(28, bold=True), fill=(30, 41, 59, 255))
    d.text((w / 2 - 40, 22), "Notes", font=font(30, bold=True), fill=(15, 23, 42, 255))

    d.text((56, 110), "Meeting notes", font=font(52, bold=True), fill=(15, 23, 42, 255))
    d.text((56, 178), "Today", font=font(26), fill=(100, 116, 139, 255))
    y = 240
    body_f = font(36)
    for line in body_lines:
        d.text((56, y), line, font=body_f, fill=(30, 41, 59, 255))
        y += 52

    # keyboard suggestion bar
    d.rectangle((0, h - 86, w, h), fill=(226, 232, 240, 255))
    d.text((40, h - 58), "Aa", font=font(28), fill=(71, 85, 105, 255))

    # floating bubble on the right
    bx = w - 170
    by = 420 if not panel else 560
    if panel:
        pw, ph = 280, 210
        px, py = bx + 58 - pw, by - ph - 16
        rr(d, (px, py, px + pw, py + ph), 28, fill=(15, 23, 42, 240))
        d.text((px + 22, py + 16), "Ready — hold to dictate" if not listening else "Listening…", font=font(20), fill=SNOW)
        for i, lab in enumerate(["Vibe prompt", "Fix grammar", "Open Hub"]):
            yy = py + 56 + i * 48
            rr(d, (px + 16, yy, px + pw - 16, yy + 42), 12, fill=(13, 148, 136, 255))
            d.text((px + 32, yy + 8), lab, font=font(22, bold=True), fill=INK)

    r = 58
    bubble_fill = (239, 68, 68, 255) if listening else TEAL_BUBBLE
    d.ellipse((bx, by, bx + r * 2, by + r * 2), fill=bubble_fill)
    # soft shadow
    shadow = Image.new("RGBA", img.size, (0, 0, 0, 0))
    sd = ImageDraw.Draw(shadow)
    sd.ellipse((bx + 6, by + 10, bx + r * 2 + 6, by + r * 2 + 10), fill=(0, 0, 0, 50))
    shadow = shadow.filter(ImageFilter.GaussianBlur(8))
    img = Image.alpha_composite(img, shadow)
    d = ImageDraw.Draw(img)
    d.ellipse((bx, by, bx + r * 2, by + r * 2), fill=bubble_fill)
    draw_mic(d, bx + r, by + r, s=28)
    if listening:
        d.ellipse((bx - 8, by - 8, bx + r * 2 + 8, by + r * 2 + 8), outline=(239, 68, 68, 120), width=4)
    return img.convert("RGB")


def tablet_notes(w, h, listening=False, panel=True):
    img = Image.new("RGBA", (w, h), (248, 250, 252, 255))
    d = ImageDraw.Draw(img)
    # sidebar
    d.rectangle((0, 0, int(w * 0.28), h), fill=(15, 28, 46, 255))
    d.text((36, 40), "Sprout", font=font(36, bold=True), fill=SNOW)
    d.text((36, 92), "Notes", font=font(22), fill=MIST)
    for i, title in enumerate(["Meeting notes", "Vibe prompt", "Inbox"]):
        yy = 160 + i * 72
        if i == 0:
            rr(d, (20, yy, int(w * 0.28) - 20, yy + 56), 16, fill=TEAL_DEEP)
        d.text((40, yy + 14), title, font=font(26), fill=SNOW)

    mx = int(w * 0.28) + 48
    d.text((mx, 48), "Meeting notes", font=font(48, bold=True), fill=(15, 23, 42, 255))
    d.text((mx, 112), "Today · English", font=font(24), fill=(100, 116, 139, 255))
    body = [
        "Ship Sprout internal test today.",
        "Hold the bubble to dictate into any field.",
        "Tap for a vibe prompt or grammar cleanup.",
        "",
        "Languages: English, हिन्दी, नेपाली",
    ]
    y = 180
    uni = font(34, uni=True)
    for line in body:
        d.text((mx, y), line, font=uni, fill=(30, 41, 59, 255))
        y += 52

    bx = w - 180
    by = int(h * 0.38)
    if panel:
        pw, ph = 300, 220
        px, py = bx - pw + 100, by - ph - 20
        rr(d, (px, py, px + pw, py + ph), 28, fill=(15, 23, 42, 245))
        d.text((px + 22, py + 16), "Listening…" if listening else "Ready — hold to dictate", font=font(22), fill=SNOW)
        for i, lab in enumerate(["Vibe prompt", "Fix grammar", "Open Hub"]):
            yy = py + 62 + i * 48
            rr(d, (px + 16, yy, px + pw - 16, yy + 42), 12, fill=(13, 148, 136, 255))
            d.text((px + 32, yy + 8), lab, font=font(22, bold=True), fill=INK)

    r = 62
    fill = (239, 68, 68, 255) if listening else TEAL_BUBBLE
    d.ellipse((bx, by, bx + r * 2, by + r * 2), fill=fill)
    draw_mic(d, bx + r, by + r, s=30)
    return img.convert("RGB")


def save(img, name):
    OUT.mkdir(exist_ok=True)
    path = OUT / name
    img.save(path, "PNG", optimize=True)
    print(path.name, img.size, path.stat().st_size)
    return path


def main():
    hub_phone = hub(1080, 1920, bubble_on=False)
    save(hub_phone, "phone-01-hub.png")
    notes = notes_bg(
        1080,
        1920,
        [
            "Ship the internal test today.",
            "Hold the bubble to dictate.",
            "Tap for vibe prompt or grammar.",
        ],
        listening=True,
        panel=False,
    )
    save(notes, "phone-02-notes-bubble.png")
    vibe = notes_bg(
        1080,
        1920,
        [
            "Turn this into a concise standup",
            "update for the team.",
        ],
        listening=False,
        panel=True,
    )
    save(vibe, "phone-03-vibe-panel.png")

    # 7-inch tablet 16:9 landscape
    save(tablet_notes(1920, 1080, listening=False, panel=True), "tablet7-01-notes.png")
    save(tablet_notes(1920, 1080, listening=True, panel=True), "tablet7-02-listening.png")

    # 10-inch tablet 16:9 landscape
    save(tablet_notes(2560, 1440, listening=False, panel=True), "tablet10-01-notes.png")
    save(tablet_notes(2560, 1440, listening=True, panel=True), "tablet10-02-listening.png")


if __name__ == "__main__":
    main()

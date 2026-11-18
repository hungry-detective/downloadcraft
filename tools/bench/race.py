"""The README's compact speed picture: one row of four panels (one per
test), Download Craft against IDM, light and dark.

    python race.py ../../docs/images        (reads results.json)
"""
import json, os, sys

HERE = os.path.dirname(os.path.abspath(__file__))
DEST = sys.argv[1]
R = json.load(open(os.path.join(HERE, os.environ.get("RESULTS", "results.json"))))
THEMES = {
    "light": dict(card="#f6f8fa", bd="#d0d7de", txt="#1f2328", dim="#59636e", dc="#16a34a", idm="#94a3b8"),
    "dark": dict(card="#161b22", bd="#30363d", txt="#e6edf3", dim="#9198a1", dc="#22c55e", idm="#5b6573"),
}
FONT = "font-family=\"-apple-system,BlinkMacSystemFont,'Segoe UI',Helvetica,Arial,sans-serif\""
TESTS = [("big", "1 GB file"), ("drops", "Connections cut"), ("capped", "Capped server"), ("small", "1 MB file")]


def fmt(s):
    return f"{s:.1f} s" if s >= 1 else f"{s * 1000:.0f} ms"


def svg(t):
    W, H, gap, pad = 760, 176, 12, 4
    cw = (W - pad * 2 - gap * 3) / 4
    out = [f'<svg xmlns="http://www.w3.org/2000/svg" width="{W}" height="{H}" viewBox="0 0 {W} {H}" {FONT}>']
    for i, (key, label) in enumerate(TESTS):
        r = R[key]
        dc, idm = r["dc_median"], r["idm_median"]
        x = pad + i * (cw + gap)
        out.append(f'<rect x="{x:.1f}" y="4" width="{cw:.1f}" height="{H - 8}" rx="12" fill="{t["card"]}" stroke="{t["bd"]}"/>')
        out.append(f'<text x="{x + 16:.1f}" y="30" font-size="13" fill="{t["dim"]}">{label}</text>')
        out.append(f'<text x="{x + 16:.1f}" y="64" font-size="28" font-weight="700" fill="{t["dc"]}">{idm / dc:.1f}×</text>')
        out.append(f'<text x="{x + 16:.1f}" y="84" font-size="12" fill="{t["dim"]}">faster than IDM</text>')
        top = max(dc, idm)
        bw = cw - 32
        for j, (name, v, c) in enumerate((("Download Craft", dc, t["dc"]), ("IDM 6.43", idm, t["idm"]))):
            y = 104 + j * 30
            w = max(4, bw * v / top)
            out.append(f'<rect x="{x + 16:.1f}" y="{y}" width="{w:.1f}" height="8" rx="4" fill="{c}"/>')
            out.append(f'<text x="{x + 16:.1f}" y="{y + 22}" font-size="11.5" fill="{t["txt"]}">{name}</text>')
            out.append(f'<text x="{x + 16 + bw:.1f}" y="{y + 22}" font-size="11.5" text-anchor="end" fill="{t["txt"]}">{fmt(v)}</text>')
    out.append("</svg>")
    return "\n".join(out)


for name, t in THEMES.items():
    open(os.path.join(DEST, f"race-{name}.svg"), "w", encoding="utf-8").write(svg(t))
print("ok")

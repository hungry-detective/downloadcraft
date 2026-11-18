"""Benchmark chart and "why switch" cards as SVG, light and dark."""
import json, os, sys

HERE = os.path.dirname(os.path.abspath(__file__))
DEST = sys.argv[1]
R = json.load(open(os.path.join(HERE, os.environ.get("RESULTS", "results.json"))))
os.makedirs(DEST, exist_ok=True)

THEMES = {
    "light": dict(bg="#ffffff", card="#f6f8fa", bd="#d0d7de", txt="#1f2328", dim="#59636e", dc="#16a34a", idm="#c4cad1", idmc="#3b82f6", acc="#0969da"),
    "dark": dict(bg="#0d1117", card="#161b22", bd="#30363d", txt="#e6edf3", dim="#9198a1", dc="#22c55e", idm="#4a5058", idmc="#4493f8", acc="#4493f8"),
}
FONT = "font-family=\"-apple-system,BlinkMacSystemFont,'Segoe UI',Helvetica,Arial,sans-serif\""
ORDER = [("big", "1 GB file"), ("capped", "300 MB, 4 MB/s per connection"), ("drops", "300 MB, connection cut every 20 MB"), ("small", "1 MB file")]


def esc(s):
    return s.replace("&", "&amp;").replace("<", "&lt;")


def fmt(s):
    return f"{s:.1f} s" if s >= 1 else f"{s * 1000:.0f} ms"


def chart(t):
    """Three bars per test: Download Craft, IDM, one connection."""
    W, row_h, top = 760, 104, 70
    H = top + row_h * len(ORDER) + 40
    series = [("dc_median", "Download Craft", t["dc"]), ("idm_median", "IDM 6.43", t["idmc"]), ("one_median", "One connection", t["idm"])]
    out = [f'<svg xmlns="http://www.w3.org/2000/svg" width="{W}" height="{H}" viewBox="0 0 {W} {H}" {FONT}>',
           f'<rect width="{W}" height="{H}" rx="12" fill="{t["bg"]}" stroke="{t["bd"]}"/>',
           f'<text x="24" y="34" font-size="18" font-weight="600" fill="{t["txt"]}">Time to finish (shorter is better)</text>']
    lx = W - 24
    for key, name, c in reversed(series):
        w = 18 + len(name) * 7.2
        lx -= w + 12
        out.append(f'<rect x="{lx:.0f}" y="22" width="12" height="12" rx="3" fill="{c}"/><text x="{lx + 18:.0f}" y="33" font-size="13" fill="{t["txt"]}">{name}</text>')
    x0, bar_w = 24, W - 24 - 250
    for i, (key, label) in enumerate(ORDER):
        r = R[key]
        vals = [(r.get(k), c) for k, _, c in series]
        y = top + i * row_h
        out.append(f'<text x="{x0}" y="{y + 12}" font-size="13" fill="{t["dim"]}">{esc(label)}</text>')
        top_v = max(v for v, _ in vals if v)
        for j, (v, c) in enumerate(vals):
            by = y + 20 + j * 24
            if v is None:
                out.append(f'<text x="{x0}" y="{by + 14}" font-size="12" fill="{t["dim"]}">not measured</text>')
                continue
            w = max(4, bar_w * v / top_v)
            out.append(f'<rect x="{x0}" y="{by}" width="{w:.1f}" height="18" rx="4" fill="{c}"/>')
            out.append(f'<text x="{x0 + w + 8:.1f}" y="{by + 14}" font-size="13" fill="{t["txt"]}">{fmt(v)}</text>')
        dc, idm = r.get("dc_median"), r.get("idm_median")
        if dc and idm:
            ratio = idm / dc
            word = f"{ratio:.1f}× faster than IDM" if ratio >= 1.05 else ("same as IDM" if ratio > 0.95 else f"IDM {1 / ratio:.1f}× faster")
            out.append(f'<text x="{W - 24}" y="{y + 56}" font-size="14" font-weight="600" text-anchor="end" fill="{t["dc"] if ratio >= 1.05 else t["dim"]}">{word}</text>')
    out.append(f'<text x="24" y="{H - 16}" font-size="11.5" fill="{t["dim"]}">Same PC and local server, 8 connections each, middle of 3 runs; timed from the first request until the file is whole.</text>')
    out.append("</svg>")
    return "\n".join(out)


def icon(kind, cx, cy, t):
    c = t["acc"]
    if kind == "bolt":
        return f'<path d="M{cx+4} {cy-22} L{cx-12} {cy+3} L{cx-1} {cy+3} L{cx-5} {cy+22} L{cx+12} {cy-4} L{cx+1} {cy-4} Z" fill="{t["dc"]}"/>'
    if kind == "gift":
        return (f'<rect x="{cx-17}" y="{cy-6}" width="34" height="24" rx="3" fill="none" stroke="{c}" stroke-width="3"/>'
                f'<rect x="{cx-20}" y="{cy-14}" width="40" height="9" rx="2" fill="none" stroke="{c}" stroke-width="3"/>'
                f'<line x1="{cx}" y1="{cy-14}" x2="{cx}" y2="{cy+18}" stroke="{c}" stroke-width="3"/>'
                f'<path d="M{cx} {cy-14} C{cx-14} {cy-28} {cx-20} {cy-14} {cx} {cy-14} C{cx+20} {cy-14} {cx+14} {cy-28} {cx} {cy-14}" fill="none" stroke="{c}" stroke-width="3"/>')
    if kind == "magnet":
        return (f'<path d="M{cx-16} {cy-18} V{cy+2} A16 16 0 0 0 {cx+16} {cy+2} V{cy-18}" fill="none" stroke="{c}" stroke-width="9"/>'
                f'<rect x="{cx-20.5}" y="{cy-22}" width="9" height="8" fill="{t["dim"]}"/><rect x="{cx+11.5}" y="{cy-22}" width="9" height="8" fill="{t["dim"]}"/>')
    if kind == "globe":
        return (f'<circle cx="{cx}" cy="{cy}" r="19" fill="none" stroke="{c}" stroke-width="3"/>'
                f'<ellipse cx="{cx}" cy="{cy}" rx="8" ry="19" fill="none" stroke="{c}" stroke-width="2.5"/>'
                f'<line x1="{cx-19}" y1="{cy}" x2="{cx+19}" y2="{cy}" stroke="{c}" stroke-width="2.5"/>')
    return ""


def cards(t, best):
    W, H = 760, 190
    items = [("bolt", f"{wins} of 4", "Faster than IDM", "in every speed test"),
             ("gift", "100%", "Free forever", "no licence, no trial, no ads"),
             ("magnet", "Torrent", "Built in", "magnet links and .torrent files"),
             ("globe", "10", "Languages", "switch live, no restart")]
    cw = (W - 24 * 2 - 12 * 3) / 4
    out = [f'<svg xmlns="http://www.w3.org/2000/svg" width="{W}" height="{H}" viewBox="0 0 {W} {H}" {FONT}>']
    for i, (k, big, title, sub) in enumerate(items):
        x = 24 + i * (cw + 12)
        cx = x + cw / 2
        out.append(f'<rect x="{x:.1f}" y="4" width="{cw:.1f}" height="{H - 8}" rx="12" fill="{t["card"]}" stroke="{t["bd"]}"/>')
        out.append(icon(k, cx, 40, t))
        out.append(f'<text x="{cx:.1f}" y="104" font-size="30" font-weight="700" text-anchor="middle" fill="{t["txt"]}">{esc(big)}</text>')
        out.append(f'<text x="{cx:.1f}" y="134" font-size="14" font-weight="600" text-anchor="middle" fill="{t["txt"]}">{esc(title)}</text>')
        out.append(f'<text x="{cx:.1f}" y="156" font-size="11.5" text-anchor="middle" fill="{t["dim"]}">{esc(sub)}</text>')
    out.append("</svg>")
    return "\n".join(out)


best = R["big"]["idm_median"] / R["big"]["dc_median"]
wins = sum(1 for k, _ in ORDER if R[k]["dc_median"] < R[k]["idm_median"])
for name, t in THEMES.items():
    open(os.path.join(DEST, f"bench-{name}.svg"), "w", encoding="utf-8").write(chart(t))
    open(os.path.join(DEST, f"why-{name}.svg"), "w", encoding="utf-8").write(cards(t, best))
print("best", round(best, 2))

#!/usr/bin/env python3
"""Compare the Rust parity runner output with the TypeScript reference runner output.

usage: compare.py <ts-outdir> <rs-outdir>
"""
import base64, json, os, sys
from PIL import Image

ts_dir, rs_dir = sys.argv[1], sys.argv[2]
ts = json.load(open(os.path.join(ts_dir, 'results.json')))
rs = json.load(open(os.path.join(rs_dir, 'results.json')))

def avg_rgba(h):
    h = base64.b64decode(h)
    header = h[0] | (h[1] << 8) | (h[2] << 16)
    l = (header & 63) / 63; p = ((header >> 6) & 63) / 31.5 - 1; q = ((header >> 12) & 63) / 31.5 - 1
    b = l - 2 / 3 * p; r = (3 * l - b + q) / 2; g = r - q
    return [max(0, min(1, x)) for x in (r, g, b)]

def load(p):
    im = Image.open(p)
    im.load()
    if im.mode in ('RGBA', 'LA', 'P'):
        im = im.convert('RGBA')
        bg = Image.new('RGBA', im.size, (128, 128, 128, 255))
        bg.alpha_composite(im)
        im = bg
    return im.convert('RGB')

def mad(a, b):
    """mean abs difference (0..255) after scaling both to 64 px on the long edge"""
    w, h = a.size
    s = 64 / max(w, h)
    size = (max(1, round(w * s)), max(1, round(h * s)))
    a = a.resize(size, Image.LANCZOS); b = b.resize(size, Image.LANCZOS)
    pa, pb = a.tobytes(), b.tobytes()
    return sum(abs(x - y) for x, y in zip(pa, pb)) / len(pa)

rows = []
issues = []
hash_equal = hash_total = 0
avg_diffs = []
mads = {}
size_ratio = {}
for t, r in zip(ts, rs):
    assert t['abs'] == r['abs']
    name = os.path.basename(t['abs'])
    tm, rm = t.get('meta', {}), r.get('meta', {})
    if 'error' not in tm:
        for k in ('width', 'height', 'pages', 'orientation'):
            if tm.get(k) != rm.get(k):
                issues.append(f"{t['id']} {name}: meta {k} ts={tm.get(k)} rs={rm.get(k)}")
    tb, rb = t.get('base'), r.get('base')
    if tb and rb:
        if abs(tb['width'] - rb['width']) > 1 or abs(tb['height'] - rb['height']) > 1:
            issues.append(f"{t['id']} {name}: base size ts={tb['width']}x{tb['height']} rs={rb['width']}x{rb['height']}")
        if tb.get('thumbhash') and rb.get('thumbhash'):
            ta, ra = avg_rgba(tb['thumbhash']), avg_rgba(rb['thumbhash'])
            avg_diffs.append((max(abs(x - y) for x, y in zip(ta, ra)), name))
    elif tb and not rb:
        issues.append(f"{t['id']} {name}: rust failed: {r.get('baseErr')}")
    elif rb and not tb:
        issues.append(f"{t['id']} {name}: (TS failed: {t.get('baseErr', '')[:60]}) rust ok {rb['width']}x{rb['height']}")
    if 'hashOfTsInputEqual' in r:
        hash_total += 1
        hash_equal += 1 if r['hashOfTsInputEqual'] else 0
        if not r['hashOfTsInputEqual']:
            issues.append(f"{t['id']} {name}: thumbhash port differs on the same input")
    for out in sorted(os.listdir(ts_dir)):
        if not out.startswith(t['id'] + '-') or out.endswith('.rgba'):
            continue
        variant = out[4:]
        rp = os.path.join(rs_dir, out)
        if not os.path.exists(rp):
            issues.append(f"{t['id']} {name}: rust has no {variant}")
            continue
        a, b = load(os.path.join(ts_dir, out)), load(rp)
        if abs(a.size[0] - b.size[0]) > 1 or abs(a.size[1] - b.size[1]) > 1:
            issues.append(f"{t['id']} {name}: {variant} size ts={a.size} rs={b.size}")
            continue
        m = mad(a, b)
        kind = variant.split('.')[0].split('-')[0] if not variant.startswith('display') else variant.rsplit('.', 1)[0]
        mads.setdefault(kind, []).append((m, name))
        size_ratio.setdefault(kind, []).append(os.path.getsize(rp) / os.path.getsize(os.path.join(ts_dir, out)))

print(f"files: {len(ts)}")
print(f"thumbhash port on identical RGBA input: {hash_equal}/{hash_total} byte-identical")
if avg_diffs:
    avg_diffs.sort()
    print(f"thumbhash average colour, end to end: max |Δ| {avg_diffs[-1][0]:.4f} ({avg_diffs[-1][1]}), mean {sum(d for d, _ in avg_diffs) / len(avg_diffs):.4f} (0..1 scale, quantisation step 1/63 ≈ 0.016)")
for k, v in sorted(mads.items()):
    v.sort()
    sr = size_ratio[k]
    print(f"{k:14s} n={len(v):3d}  MAD mean {sum(m for m, _ in v) / len(v):5.2f}  max {v[-1][0]:5.2f} ({v[-1][1]})  bytes rust/ts median {sorted(sr)[len(sr) // 2]:.2f}")
print("issues:")
for i in issues:
    print("  " + i)

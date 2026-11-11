"""Rasterizes frames dumped by `examples/render_preview.rs` into PNGs (and a GIF).

Software triangle rasterizer for egui meshes: premultiplied-alpha blending,
bilinear texture sampling, clip rects. Slow but exact enough for a design
preview. Usage: python3 tools/rasterize.py <dir> [background.png]
"""
import glob, os, struct, sys
import numpy as np
from PIL import Image

def read_frame(path, textures):
    data = open(path, 'rb').read(); o = 0
    def take(fmt):
        nonlocal o
        v = struct.unpack_from(fmt, data, o); o += struct.calcsize(fmt); return v
    (ntex,) = take('<I')
    for _ in range(ntex):
        key, x, y, w, h = take('<QiiII')
        px = np.frombuffer(data, np.uint8, w * h * 4, o).reshape(h, w, 4).astype(np.float32) / 255; o += w * h * 4
        if x < 0:
            textures[key] = px.copy()
        else:
            textures[key][y:y + h, x:x + w] = px
    (nmesh,) = take('<I'); (ppp,) = take('<f')
    meshes = []
    for _ in range(nmesh):
        (key,) = take('<Q'); clip = take('<4f'); (nv,) = take('<I')
        rec = np.frombuffer(data, np.dtype([('p', '<f4', 2), ('uv', '<f4', 2), ('c', 'u1', 4)]), nv, o); o += nv * 20
        (ni,) = take('<I'); idx = np.frombuffer(data, '<u4', ni, o); o += ni * 4
        meshes.append((key, clip, rec, idx))
    return ppp, meshes

def sample(tex, u, v):
    h, w, _ = tex.shape
    x = np.clip(u * w - 0.5, 0, w - 1); y = np.clip(v * h - 0.5, 0, h - 1)
    x0 = np.floor(x).astype(int); y0 = np.floor(y).astype(int)
    x1 = np.minimum(x0 + 1, w - 1); y1 = np.minimum(y0 + 1, h - 1)
    fx = (x - x0)[:, None]; fy = (y - y0)[:, None]
    return (tex[y0, x0] * (1 - fx) * (1 - fy) + tex[y0, x1] * fx * (1 - fy)
            + tex[y1, x0] * (1 - fx) * fy + tex[y1, x1] * fx * fy)

def raster(img, ppp, meshes, textures):
    H, W, _ = img.shape
    for key, clip, rec, idx in meshes:
        tex = textures.get(key)
        cx0, cy0, cx1, cy1 = [int(round(c * ppp)) for c in clip]
        cx0, cy0 = max(cx0, 0), max(cy0, 0); cx1, cy1 = min(cx1, W), min(cy1, H)
        pos = rec['p'] * ppp; uv = rec['uv']; col = rec['c'].astype(np.float32) / 255
        for t in range(0, len(idx), 3):
            a, b, c = idx[t:t + 3]
            P = pos[[a, b, c]]
            x0 = max(int(np.floor(P[:, 0].min())), cx0); x1 = min(int(np.ceil(P[:, 0].max())) + 1, cx1)
            y0 = max(int(np.floor(P[:, 1].min())), cy0); y1 = min(int(np.ceil(P[:, 1].max())) + 1, cy1)
            if x0 >= x1 or y0 >= y1: continue
            ys, xs = np.mgrid[y0:y1, x0:x1]; px = xs.ravel() + 0.5; py = ys.ravel() + 0.5
            d = (P[1, 1] - P[2, 1]) * (P[0, 0] - P[2, 0]) + (P[2, 0] - P[1, 0]) * (P[0, 1] - P[2, 1])
            if abs(d) < 1e-9: continue
            l0 = ((P[1, 1] - P[2, 1]) * (px - P[2, 0]) + (P[2, 0] - P[1, 0]) * (py - P[2, 1])) / d
            l1 = ((P[2, 1] - P[0, 1]) * (px - P[2, 0]) + (P[0, 0] - P[2, 0]) * (py - P[2, 1])) / d
            l2 = 1 - l0 - l1
            m = (l0 >= -1e-4) & (l1 >= -1e-4) & (l2 >= -1e-4)
            if not m.any(): continue
            L = np.stack([l0[m], l1[m], l2[m]], 1)
            color = L @ col[[a, b, c]]
            if tex is not None:
                U = L @ uv[[a, b, c]]
                color = color * sample(tex, U[:, 0], U[:, 1])
            yy = ys.ravel()[m]; xx = xs.ravel()[m]
            dst = img[yy, xx]
            img[yy, xx] = color[:, :3] + dst * (1 - color[:, 3:4])  # premultiplied "over"

def main():
    d = sys.argv[1]
    bg = Image.open(sys.argv[2]).convert('RGB').resize((1920, 1080)) if len(sys.argv) > 2 else None
    textures = {}; frames = []
    for path in sorted(glob.glob(os.path.join(d, 'frame*.bin'))):
        ppp, meshes = read_frame(path, textures)
        if bg is not None:
            img = np.asarray(bg, np.float32) / 255
        else:  # turf-ish gradient
            y = np.linspace(0, 1, 1080)[:, None, None]
            img = np.broadcast_to(np.array([0.33, 0.45, 0.27]) * (0.75 + 0.35 * y), (1080, 1920, 3)).copy()
        img = img.astype(np.float32)
        raster(img, ppp, meshes, textures)
        out = Image.fromarray((np.clip(img, 0, 1) * 255).astype(np.uint8))
        out.save(path.replace('.bin', '.png')); frames.append(out)
    if frames:
        crop = (0, 760, 1920, 1080)
        small = [f.crop(crop).resize((960, 160), Image.LANCZOS) for f in frames]
        small[0].save(os.path.join(d, 'swap.gif'), save_all=True, append_images=small[1:], duration=33, loop=0)
    print(len(frames), 'frames')

if __name__ == '__main__':
    main()

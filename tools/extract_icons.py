"""Extracts character icons from your own Umamusume install for the leaderboard.

Writes into the plugin's image folder:
  <chara id>.png   default icon of each character (chr_icon_1001 -> 1001.png)
  <card id>.png    icon of each card's race outfit (card 100101 -> 100101.png)

Only assets the game has already downloaded can be exported; run it again
after a game update to pick up new characters and outfits.

Format notes (from UmamusumeExplorer, github.com/MarshmallowAndroid/UmamusumeExplorer):
- `meta` is an SQLite3MultipleCiphers (ChaCha20) database listing every asset:
  table `a`, columns n (name), h (hash = file name under dat/), e (key).
- Each asset file is a Unity bundle XOR-encrypted after its first 256 bytes,
  with a key stream derived from `e`.
- `master/master.mdb` (plain SQLite) has card -> race outfit in card_rarity_data.

Requirements: pip install apsw-sqlite3mc UnityPy "attrs>=23"
Usage:
  python tools/extract_icons.py --out "<game>\\hachimi\\uma-esport-spectator\\chara"
  python tools/extract_icons.py --data <data dir> --out <dir>   # non-default install
"""
import argparse
import io
import os
import re
import shutil
import struct
import sys
import tempfile

import apsw
import UnityPy

META_KEYS = [
    "c753a5e8f5f78294f7fef57df4a14ffbf9a896cea1d4e09947e0d904e7fde8eaf0",
    "9c2bab97bcf8c0c4f1a9ea7881a213f6c9ebf9d8d4c6a8e43ce5a259bde7e9fd",
    "8908b6dca7b6db99dda0",
]
BUNDLE_BASE_KEYS = bytes([0x53, 0x2B, 0x46, 0x31, 0xE4, 0xA7, 0xB9, 0x47, 0x3E, 0x7C, 0xFB])
HEADER_SIZE = 256

CHARA_ICON = re.compile(r"^chara/chr(\d{4})/chr_icon_(\d{4})$")
DRESS_ICON = re.compile(r"^chara/chr(\d{4})/chr_icon_(\d{4})_(\d{6})_(\d{2})$")


def default_data_dirs():
    local_low = os.path.join(os.path.dirname(os.environ.get("LOCALAPPDATA", "")), "LocalLow", "Cygames")
    return [os.path.join(local_low, name) for name in ("Umamusume", "umamusume", "UmamusumePrettyDerby_Jpn")]


def is_data_dir(path):
    return os.path.isfile(os.path.join(path, "meta")) and os.path.isdir(os.path.join(path, "dat"))


def open_meta(meta_path):
    # Work on a copy: the game may hold the file open, and we never want to touch it.
    tmp = os.path.join(tempfile.mkdtemp(), "meta")
    shutil.copyfile(meta_path, tmp)
    with open(tmp, "rb") as f:
        plain = f.read(16).startswith(b"SQLite format 3")
    for key in [None] if plain else META_KEYS:
        try:
            conn = apsw.Connection(tmp, flags=apsw.SQLITE_OPEN_READONLY)
            if key:
                conn.execute("pragma cipher='chacha20'")
                conn.execute(f"pragma hexkey='{key}'")
            conn.execute("select count(*) from a").fetchone()
            return conn
        except apsw.Error:
            continue
    sys.exit("Cannot open the meta database with any known key (game update changed it?).")


def decrypt(data, key):
    if key == 0:
        return data
    kb = struct.pack("<q", key)
    stream = bytes(b ^ kb[j] for b in BUNDLE_BASE_KEYS for j in range(8))
    # Key stream aligned to file offset 0, applied from HEADER_SIZE on.
    body = data[HEADER_SIZE:]
    start = HEADER_SIZE % len(stream)
    rotated = stream[start:] + stream[:start]
    reps = len(body) // len(rotated) + 1
    mask = (rotated * reps)[: len(body)]
    xored = (int.from_bytes(body, "little") ^ int.from_bytes(mask, "little")).to_bytes(len(body), "little")
    return data[:HEADER_SIZE] + xored


def load_icon(data_dir, hash_name, key):
    path = os.path.join(data_dir, "dat", hash_name[:2], hash_name)
    if not os.path.isfile(path):
        return None
    with open(path, "rb") as f:
        env = UnityPy.load(io.BytesIO(decrypt(f.read(), key)))
    for obj in env.objects:
        if obj.type.name == "Texture2D":
            return obj.read().image
    return None


def card_dresses(data_dir):
    """card id -> race outfit id (from the highest rarity, the card's own outfit)."""
    master = os.path.join(data_dir, "master", "master.mdb")
    if not os.path.isfile(master):
        return {}
    conn = apsw.Connection(master, flags=apsw.SQLITE_OPEN_READONLY)
    rows = conn.execute(
        "select card_id, race_dress_id from card_rarity_data r "
        "where rarity = (select max(rarity) from card_rarity_data where card_id = r.card_id)"
    )
    return {card: dress for card, dress in rows}


def main():
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("--data", help="game data folder (has meta, dat/, master/)")
    ap.add_argument("--out", required=True, help="output folder (the plugin's chara folder)")
    ap.add_argument("--force", action="store_true", help="overwrite existing PNGs")
    args = ap.parse_args()

    data_dir = args.data or next((d for d in default_data_dirs() if is_data_dir(d)), None)
    if not data_dir or not is_data_dir(data_dir):
        sys.exit("Game data folder not found; pass --data (the folder with meta and dat).")
    os.makedirs(args.out, exist_ok=True)

    meta = open_meta(os.path.join(data_dir, "meta"))
    entries = {n: (h, e) for n, h, e in meta.execute("select n, h, e from a where n like 'chara/chr%/chr_icon_%'")}

    jobs = []  # (output name, [candidate asset names])
    for name in entries:
        m = CHARA_ICON.match(name)
        if m and m.group(1) == m.group(2):
            jobs.append((f"{m.group(1)}.png", [name]))
    for card, dress in sorted(card_dresses(data_dir).items()):
        chara = str(card)[:4]
        candidates = [f"chara/chr{chara}/chr_icon_{chara}_{dress:06d}_{plate}" for plate in ("01", "02")]
        jobs.append((f"{card}.png", candidates))

    written = skipped = missing = 0
    for out_name, candidates in jobs:
        out_path = os.path.join(args.out, out_name)
        if os.path.exists(out_path) and not args.force:
            skipped += 1
            continue
        image = None
        for asset in candidates:
            if asset in entries:
                image = load_icon(data_dir, *entries[asset])
                if image is not None:
                    break
        if image is None:
            missing += 1
            continue
        image.save(out_path)
        written += 1
    print(f"{written} icons written, {skipped} already there, {missing} not downloaded by the game -> {args.out}")


if __name__ == "__main__":
    main()

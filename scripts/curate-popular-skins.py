#!/usr/bin/env python3
"""Regenerate src-tauri/resources/popular-skins.json.

Curates 20 classic .wsz skins from the Internet Archive's `winampskins`
collection (the dataset behind the Winamp Skin Museum) and records size,
SHA-256, author and attribution for each. The archives are NOT committed:
the app downloads them only when the user asks.

Usage: python3 scripts/curate-popular-skins.py [--out PATH]

Requires only the Python 3 standard library and network access.
"""

import argparse
import hashlib
import json
import os
import sys
import tempfile
import time
import urllib.parse
import urllib.request

COLLECTION = "winampskins"
SOURCE_URL = "https://archive.org/details/winampskins"
LICENSE_NOTE = (
    "No license is stated on the archive.org items; authors retain all rights. "
    "Skin files are downloaded only at the user's request."
)

# (archive identifier, source file name, destination id, display name, author)
CURATED = [
    ("winampskin_nightcrawler_by_t_k", "nightcrawler_by_t_k.wsz", "nightcrawler", "Nightcrawler", "t-k"),
    ("winampskin_zlomp", "zlomp.wsz", "zlomp", "Zlomp", "herod"),
    ("winampskin_Black_SlanXP2_Update", "Black_SlanXP2_Update.wsz", "black-slanxp2", "Black SlanXP2", None),
    ("winampskin_Expensive_Hi-Fi_1_2", "ExpensiveHi-Fi.wsz", "expensive-hi-fi", "Expensive Hi-Fi", None),
    ("winampskin_Nucleo_NLog_v2G", "Nucleo_NLog_v102_.wsz", "nucleo-nlog-v2g", "Nucleo NLog v2G", None),
    ("winampskins_Frutiger_Aero", "Frutiger Aero.wsz", "frutiger-aero", "Frutiger Aero", None),
    ("winampskin_Cyanian", "Cyanian.wsz", "cyanian", "Cyanian", None),
    ("winampskins_AMPER", "AMPER.wsz", "amper", "AMPER", None),
    ("winampskin_Nightshade", "Nightshade.wsz", "nightshade", "Nightshade", None),
    ("winampskin_TopazAmp_v5", "TopazAmp_v5.wsz", "topazamp-v5", "TopazAmp v5", None),
    ("winampskin_elaysia", "elaysia.wsz", "elaysia", "Elaysia", "x4-deejay"),
    ("winampskins_LimeAmp_3", "LimeAmp.wsz", "limeamp-3", "LimeAmp 3", None),
    ("winampskin_winstepamp", "winstepamp.wsz", "winstepamp", "Winstep Amp", "dangeruss"),
    ("winampskin_Major_Tom_v5", "Major_Tom_v5.wsz", "major-tom-v5", "Major Tom v5", None),
    ("winampskin_CHROMEngine_Indigo_v5", "CHROMEngine_Indigo_v5.wsz", "chromengine-indigo-v5", "CHROMEngine Indigo v5", None),
    ("winampskin_Rebel_Amp_2k", "Rebel_Amp_2k.wsz", "rebel-amp-2k", "Rebel Amp 2k", None),
    ("winampskin_Void_dark", "Void_dark.wsz", "void-dark", "Void (dark)", None),
    ("winampskin_Classic_Ebony", "Classic_Ebony.wsz", "classic-ebony", "Classic Ebony", None),
    ("winampskin_Moonlight_night", "Moonlight_night.wsz", "moonlight-night", "Moonlight Night", None),
    ("winampskin_yboris_minimal_winamp_by_yboris", "yboris_minimal_winamp_by_yboris.wsz", "yboris-minimal", "yboris minimal", "yboris"),
]


def fetch(url: str, dest: str, attempts: int = 3) -> bytes:
    for attempt in range(1, attempts + 1):
        try:
            with urllib.request.urlopen(url, timeout=45) as response:
                data = response.read()
            with open(dest, "wb") as out:
                out.write(data)
            return data
        except Exception as exc:  # noqa: BLE001 - retried
            if attempt == attempts:
                raise SystemExit(f"cannot download {url}: {exc}")
            time.sleep(attempt)
    raise AssertionError("unreachable")


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument(
        "--out",
        default=os.path.join("src-tauri", "resources", "popular-skins.json"),
    )
    args = parser.parse_args()

    skins = []
    with tempfile.TemporaryDirectory() as tmp:
        for identifier, file_name, skin_id, name, author in CURATED:
            url = (
                f"https://archive.org/download/{identifier}/"
                f"{urllib.parse.quote(file_name)}"
            )
            dest = os.path.join(tmp, skin_id + ".wsz")
            data = fetch(url, dest)
            skins.append(
                {
                    "id": skin_id,
                    "name": name,
                    "author": author,
                    "fileName": skin_id + ".wsz",
                    "url": url,
                    "size": len(data),
                    "sha256": hashlib.sha256(data).hexdigest(),
                    "license": "not stated on archive.org",
                    "sourceUrl": f"https://archive.org/details/{identifier}",
                }
            )
            print(f"{skin_id:24} {len(data):>7} bytes  {hashlib.sha256(data).hexdigest()[:12]}")
            time.sleep(0.3)

    pack = {
        "version": 1,
        "curatedAt": time.strftime("%Y-%m-%d"),
        "source": SOURCE_URL,
        "note": (
            "Downloaded on request from the Internet Archive's Winamp Skin Museum "
            "collection; authors retain all rights."
        ),
        "licenseNote": LICENSE_NOTE,
        "skins": skins,
    }
    os.makedirs(os.path.dirname(args.out), exist_ok=True)
    with open(args.out, "w") as out:
        json.dump(pack, out, indent=2)
        out.write("\n")
    print(f"wrote {args.out} ({len(skins)} skins, {sum(s['size'] for s in skins)} bytes)")


if __name__ == "__main__":
    main()

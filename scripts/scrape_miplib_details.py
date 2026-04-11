#!/usr/bin/env python3
"""Scrape per-instance details from miplib.zib.de.

For each instance name, fetches
https://miplib.zib.de/instance_details_<name>.html and writes a JSON file
to cache/miplib-details/<name>.json with the parsed fields. Already-cached
instances are skipped unless --force is passed.

Usage:
    python scripts/scrape_miplib_details.py                     # all instances from miplib-metadata.json
    python scripts/scrape_miplib_details.py 30n20b8 neos-1456979  # specific instances
    python scripts/scrape_miplib_details.py --force 30n20b8     # re-scrape
"""

import argparse
import json
import os
import sys
import time
import urllib.error
import urllib.request

try:
    from bs4 import BeautifulSoup
except ImportError:
    sys.exit("This script requires beautifulsoup4. Install with: pip install beautifulsoup4")

BASE = "https://miplib.zib.de/instance_details_{}.html"
ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
CACHE_DIR = os.path.join(ROOT, "cache", "miplib-details")
METADATA_URL = "https://raw.githubusercontent.com/mmghannam/mipviz-instances/main/miplib-metadata.json"
USER_AGENT = "mipviz-scraper/0.1 (+https://github.com/mmghannam/mipviz)"
DELAY_SECONDS = 1.0


def fetch(url):
    req = urllib.request.Request(url, headers={"User-Agent": USER_AGENT})
    with urllib.request.urlopen(req, timeout=30) as resp:
        return resp.read().decode("utf-8", errors="replace")


def text(node):
    return " ".join(node.get_text(" ", strip=True).split()) if node else None


def parse_header_table(soup):
    """Parse the top table with Submitter/Vars/Constraints/.../MPS File and the description paragraph below."""
    h1 = soup.find("h1")
    if not h1:
        return {}
    table = h1.find_next("table")
    if not table:
        return {}
    headers = [text(th) for th in table.find("thead").find_all("th")]
    body = table.find("tbody")
    if not body:
        return {}
    cells = body.find("tr").find_all("td")
    row = {}
    for h, td in zip(headers, cells):
        row[h] = text(td)
    mps_a = cells[-1].find("a") if cells else None
    if mps_a and mps_a.get("href"):
        href = mps_a["href"]
        if href.startswith("WebData"):
            href = "https://miplib.zib.de/" + href
        row["MPS File URL"] = href

    out = {
        "submitter": row.get("Submitter"),
        "variables": row.get("Variables"),
        "constraints": row.get("Constraints"),
        "density": row.get("Density"),
        "status": row.get("Status"),
        "group": row.get("Group") if row.get("Group") not in (None, "–", "-") else None,
        "objective": row.get("Objective"),
        "mps_file_url": row.get("MPS File URL"),
    }

    # Description paragraph immediately after the header table.
    desc_p = table.find_next("p")
    if desc_p:
        out["description"] = text(desc_p)
    return out


def _parse_orig_presolved_table(table):
    """Parse a 3-col (label, Original, Presolved) table into {label: {original, presolved}}."""
    out = {}
    body = table.find("tbody")
    if not body:
        return out
    for tr in body.find_all("tr"):
        cells = tr.find_all("td")
        if len(cells) < 3:
            continue
        label = text(cells[0])
        if not label:
            continue
        out[label] = {
            "original": text(cells[1]) or None,
            "presolved": text(cells[2]) or None,
        }
    return out


def parse_statistics(soup):
    """Parse the two tables under the 'Instance Statistics' section.

    Returns {"size": {...}, "constraints": {...}} where each sub-dict maps a
    MIPLIB stat label (e.g. 'Variables', 'Set Partitioning') to
    {'original': str, 'presolved': str}.
    """
    section = soup.find("div", id="instance-statistics")
    if not section:
        return {}
    out = {}
    for table in section.find_all("table"):
        caption = table.find("caption")
        caption_text = text(caption) or ""
        parsed = _parse_orig_presolved_table(table)
        if "Size" in caption_text:
            out["size"] = parsed
        elif "Constraint" in caption_text:
            out["constraints"] = parsed
    return out


def parse_solutions(soup):
    section = soup.find("div", id="best-known-solutions")
    if not section:
        return []
    table = section.find("table")
    if not table:
        return []
    headers = [text(th) for th in table.find("thead").find_all("th")]
    rows = []
    for tr in table.find("tbody").find_all("tr"):
        cells = tr.find_all("td")
        row = {}
        for h, td in zip(headers, cells):
            row[h.lower().replace(".", "").replace(" ", "_")] = text(td)
        link = cells[0].find("a") if cells else None
        if link and link.get("href"):
            href = link["href"]
            row["download_url"] = (
                href if href.startswith("http") else "https://miplib.zib.de/" + href
            )
        rows.append(row)
    return rows


def parse_reference(soup):
    section = soup.find("div", id="reference")
    if not section:
        return None
    code = section.find("code")
    if not code:
        return None
    raw = code.get_text()
    return raw.strip() or None


def parse_instance(name, html):
    soup = BeautifulSoup(html, "html.parser")
    data = {"name": name}
    data.update(parse_header_table(soup))
    data["statistics"] = parse_statistics(soup)
    data["solutions"] = parse_solutions(soup)
    data["reference_bibtex"] = parse_reference(soup)
    return data


def scrape(name, force=False):
    out_path = os.path.join(CACHE_DIR, name + ".json")
    if os.path.exists(out_path) and not force:
        return "cached"
    url = BASE.format(name)
    try:
        html = fetch(url)
    except urllib.error.HTTPError as e:
        return f"http-error {e.code}"
    except urllib.error.URLError as e:
        return f"url-error {e.reason}"
    data = parse_instance(name, html)
    os.makedirs(CACHE_DIR, exist_ok=True)
    with open(out_path, "w") as f:
        json.dump(data, f, indent=2, ensure_ascii=False)
    return "ok"


def load_all_names():
    try:
        html = fetch(METADATA_URL)
    except Exception as e:
        sys.exit(f"Failed to load instance list from {METADATA_URL}: {e}")
    meta = json.loads(html)
    return [m["name"] for m in meta if m.get("name")]


def main():
    ap = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    ap.add_argument("names", nargs="*", help="Instance names (default: all from miplib-metadata.json)")
    ap.add_argument("--force", action="store_true", help="Re-scrape even if cached")
    ap.add_argument("--delay", type=float, default=DELAY_SECONDS, help="Delay between requests (seconds)")
    args = ap.parse_args()

    names = args.names or load_all_names()
    print(f"Scraping {len(names)} instance(s) into {CACHE_DIR}")
    os.makedirs(CACHE_DIR, exist_ok=True)

    counts = {"ok": 0, "cached": 0, "error": 0}
    for i, name in enumerate(names, 1):
        result = scrape(name, force=args.force)
        if result == "ok":
            counts["ok"] += 1
        elif result == "cached":
            counts["cached"] += 1
        else:
            counts["error"] += 1
        print(f"[{i}/{len(names)}] {name}: {result}")
        if result == "ok" and i < len(names):
            time.sleep(args.delay)

    print(f"\nDone. ok={counts['ok']} cached={counts['cached']} error={counts['error']}")


if __name__ == "__main__":
    main()

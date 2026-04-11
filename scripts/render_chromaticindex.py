#!/usr/bin/env python3
"""Render a chromaticindex* MIPLIB instance's graph and its optimal edge
coloring from the MIPLIB-hosted .sol.gz file.

Variable names in the solution follow `x<i>_<j>_<c> 1` (edge (i,j) has
colour c) and `c<k> 1` (colour k is used), so the graph and colouring
are reconstructible without touching the MPS file.

Outputs two PNGs to cache/chromaticindex/<name>/:
    graph.png     — uncoloured edges
    solution.png  — edges coloured by their assigned class

Usage:
    python scripts/render_chromaticindex.py chromaticindex32-8
"""

import argparse
import gzip
import os
import re
import sys
import urllib.request

import matplotlib
matplotlib.use("Agg")
import matplotlib.pyplot as plt
import networkx as nx

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
OUT_BASE = os.path.join(ROOT, "cache", "chromaticindex")
SOL_URL = "https://miplib.zib.de/downloads/solutions/{name}/1/{name}.sol.gz"

COLOUR_PALETTE = [
    "#e6194b", "#3cb44b", "#4363d8", "#f58231",
    "#911eb4", "#46f0f0", "#f032e6", "#bcf60c",
    "#fabebe", "#008080", "#e6beff", "#9a6324",
]


def fetch_solution(name, cache_dir):
    os.makedirs(cache_dir, exist_ok=True)
    path = os.path.join(cache_dir, f"{name}.sol.gz")
    if not os.path.exists(path):
        url = SOL_URL.format(name=name)
        print(f"Fetching {url}", file=sys.stderr)
        urllib.request.urlretrieve(url, path)
    return path


def parse_solution(sol_path):
    edges = []  # list of (u, v, colour)
    obj = None
    with gzip.open(sol_path, "rt") as f:
        for line in f:
            line = line.strip()
            if not line:
                continue
            if line.startswith("=obj="):
                obj = float(line.split()[1])
                continue
            name, val = line.split()
            val = float(val)
            if val != 1.0:
                continue
            m = re.match(r"x(\d+)_(\d+)_(\d+)$", name)
            if m:
                i, j, c = (int(x) for x in m.groups())
                edges.append((i, j, c))
    return obj, edges


def build_graph(edges):
    G = nx.Graph()
    for u, v, c in edges:
        G.add_edge(u, v, colour=c)
    return G


def draw(G, layout, path, title, coloured=False):
    fig, ax = plt.subplots(figsize=(10, 10), dpi=150)
    ax.set_facecolor("white")

    node_size = max(8, min(40, 4000 / len(G.nodes)))
    node_colour = "#1e222b"

    if coloured:
        edge_colours = [
            COLOUR_PALETTE[d["colour"] % len(COLOUR_PALETTE)]
            for _, _, d in G.edges(data=True)
        ]
        edge_width = 1.3
    else:
        edge_colours = ["#5a6170"]
        edge_width = 0.8

    nx.draw_networkx_edges(
        G, pos=layout, ax=ax,
        edge_color=edge_colours, width=edge_width, alpha=0.85,
    )
    nx.draw_networkx_nodes(
        G, pos=layout, ax=ax,
        node_size=node_size, node_color=node_colour,
        linewidths=0, edgecolors="none",
    )

    ax.set_title(title, fontsize=13, color="#374151", pad=14)
    ax.set_axis_off()
    ax.margins(0.02)
    fig.tight_layout()
    fig.savefig(path, dpi=150, bbox_inches="tight", facecolor="white")
    plt.close(fig)
    print(f"Wrote {path}", file=sys.stderr)


def main():
    ap = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    ap.add_argument("name", help="Instance name, e.g. chromaticindex32-8")
    ap.add_argument("--seed", type=int, default=42, help="Layout seed")
    args = ap.parse_args()

    out_dir = os.path.join(OUT_BASE, args.name)
    os.makedirs(out_dir, exist_ok=True)

    sol_path = fetch_solution(args.name, out_dir)
    obj, edges = parse_solution(sol_path)
    if not edges:
        sys.exit("No edge variables found in solution file")

    G = build_graph(edges)
    n, m = G.number_of_nodes(), G.number_of_edges()
    degs = dict(G.degree())
    max_deg = max(degs.values())
    colours_used = sorted({d["colour"] for _, _, d in G.edges(data=True)})
    print(f"{args.name}: {n} nodes, {m} edges, max degree {max_deg}, "
          f"χ'={int(obj)}, colours={colours_used}", file=sys.stderr)

    print("Computing layout (this may take a few seconds)…", file=sys.stderr)
    layout = nx.spring_layout(G, seed=args.seed, iterations=200)

    draw(G, layout,
         os.path.join(out_dir, "graph.png"),
         f"{args.name}: {n} vertices, {m} edges (max degree {max_deg})",
         coloured=False)
    draw(G, layout,
         os.path.join(out_dir, "solution.png"),
         f"{args.name}: optimal edge colouring with χ' = {int(obj)}",
         coloured=True)


if __name__ == "__main__":
    main()

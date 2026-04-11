#!/usr/bin/env python3
"""Render a chromaticindex* MIPLIB instance's graph and its optimal edge
coloring from the MIPLIB-hosted .sol.gz file.

Variable names in the solution follow `x<i>_<j>_<c> 1` (edge (i,j) has
colour c) and `c<k> 1` (colour k is used), so the graph and colouring
are reconstructible without touching the MPS file.

Default output is a compact graph.json that the mipviz frontend
renders interactively with Sigma.js. Use --png to additionally emit
static graph.png / solution.png via matplotlib.

Outputs to cache/chromaticindex/<name>/:
    graph.json    — nodes (with layout positions) + coloured edges
    graph.png     — (with --png) uncoloured edges
    solution.png  — (with --png) edges coloured by their assigned class

Usage:
    python scripts/render_chromaticindex.py chromaticindex32-8
    python scripts/render_chromaticindex.py chromaticindex32-8 --png
"""

import argparse
import gzip
import json
import os
import re
import sys
import urllib.request

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


NOTE_TEMPLATE = """# {name}

The instance encodes an edge-colouring problem on a graph with
**{n:,} vertices and {m:,} edges** (maximum degree {d}). The optimal
value is χ′ = {k} — i.e. {k} colour classes are enough to edge-colour
this graph — recovered from MIPLIB's best-known solution file.

Drag the graph to pan and scroll to zoom.

```mipviz-graph
graph.json
```

## Source and reproducibility

The graph and its colouring come from MIPLIB's best-known solution
file
([`{name}.sol.gz`](https://miplib.zib.de/downloads/solutions/{name}/1/{name}.sol.gz)).
Variable names in that file follow `x<i>_<j>_<c> 1` ("edge (i,j) has
colour c"), so the graph and its colouring are reconstructible without
touching the MPS file. The conversion to `graph.json` is done by
[`scripts/render_chromaticindex.py`](https://github.com/mmghannam/mipviz/blob/main/scripts/render_chromaticindex.py)
in the mipviz repo; the interactive view is powered by Sigma.js.

See the group note on `chromaticindex` for the origin paper and
further reading.
"""


def write_note_markdown(name, G, obj, out_path, force=False):
    if os.path.exists(out_path) and not force:
        print(f"Skipping existing {out_path} (use --force to overwrite)",
              file=sys.stderr)
        return
    degs = dict(G.degree())
    md = NOTE_TEMPLATE.format(
        name=name,
        n=G.number_of_nodes(),
        m=G.number_of_edges(),
        d=max(degs.values()) if degs else 0,
        k=int(obj) if obj is not None else "?",
    )
    with open(out_path, "w") as f:
        f.write(md)
    print(f"Wrote {out_path}", file=sys.stderr)


def write_graph_json(G, layout, obj, out_path, meta_extra=None):
    """Emit a compact JSON the mipviz frontend can render with Sigma.

    Positions are pre-computed here (via spring_layout) so the browser
    doesn't need to run its own force-directed layout.
    """
    # Sigma's camera auto-fit works much better on larger coordinate
    # spans (the existing conflict.js uses order-100 coordinates), so
    # stretch the [-1, 1]-ish spring_layout output by a fixed factor.
    POS_SCALE = 100.0
    nodes = []
    for node in sorted(G.nodes()):
        x, y = layout[node]
        nodes.append({
            "id": str(node),
            "x": float(x) * POS_SCALE,
            "y": float(y) * POS_SCALE,
        })
    edges = []
    for u, v, d in G.edges(data=True):
        edges.append({
            "source": str(u),
            "target": str(v),
            "colour": int(d["colour"]),
        })
    colours_used = sorted({d["colour"] for _, _, d in G.edges(data=True)})
    degs = dict(G.degree())
    out = {
        "meta": {
            "nodes": G.number_of_nodes(),
            "edges": G.number_of_edges(),
            "max_degree": max(degs.values()) if degs else 0,
            "min_degree": min(degs.values()) if degs else 0,
            "num_colours": len(colours_used),
            "chromatic_index": int(obj) if obj is not None else None,
            **(meta_extra or {}),
        },
        "nodes": nodes,
        "edges": edges,
    }
    with open(out_path, "w") as f:
        json.dump(out, f, separators=(",", ":"))
    print(f"Wrote {out_path}", file=sys.stderr)


def draw(G, layout, path, title, coloured=False):
    import matplotlib
    matplotlib.use("Agg")
    import matplotlib.pyplot as plt

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
    ap.add_argument("--iterations", type=int, default=200,
                    help="spring_layout iterations (lower is faster)")
    ap.add_argument("--png", action="store_true",
                    help="Also emit graph.png / solution.png via matplotlib")
    ap.add_argument("--note", action="store_true",
                    help="Also emit a markdown note stub (<name>.md)")
    ap.add_argument("--force", action="store_true",
                    help="Overwrite existing note markdown if present")
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

    print(f"Computing layout ({args.iterations} iterations)…", file=sys.stderr)
    layout = nx.spring_layout(G, seed=args.seed, iterations=args.iterations)

    write_graph_json(
        G, layout, obj,
        os.path.join(out_dir, "graph.json"),
        meta_extra={"name": args.name},
    )

    if args.note:
        write_note_markdown(
            args.name, G, obj,
            os.path.join(out_dir, f"{args.name}.md"),
            force=args.force,
        )

    if args.png:
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

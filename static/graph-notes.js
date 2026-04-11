// Interactive graph renderer for instance notes.
//
// Scans a rendered note body for fenced code blocks tagged
// `mipviz-graph` and replaces each with a Sigma.js canvas that loads
// the referenced graph.json and renders nodes/edges with Sigma.
//
// Usage from app.js:
//     import('./graph-notes.js?v=1').then(m => m.renderGraphBlocks(bodyEl, imageBase));
//
// graph.json shape expected:
//     {
//       meta: {...},
//       nodes: [{id, x, y}],
//       edges: [{source, target, colour}]  // colour is an integer class
//     }

import Graph from 'https://esm.sh/graphology@0.25.4';
import Sigma from 'https://esm.sh/sigma@3.0.2';

// Same palette used by scripts/render_chromaticindex.py for parity.
const COLOUR_PALETTE = [
    '#e6194b', '#3cb44b', '#4363d8', '#f58231',
    '#911eb4', '#46f0f0', '#f032e6', '#bcf60c',
    '#fabebe', '#008080', '#e6beff', '#9a6324',
];

const activeRenderers = new WeakMap();

function colourFor(idx) {
    return COLOUR_PALETTE[idx % COLOUR_PALETTE.length];
}

function buildGraph(data) {
    const g = new Graph({ type: 'undirected', multi: false });
    for (const n of data.nodes) {
        g.addNode(n.id, {
            x: n.x,
            y: n.y,
            size: 2,
            color: '#5a6170',
            label: n.id,
        });
    }
    for (const e of data.edges) {
        const key = `${e.source}--${e.target}`;
        if (g.hasEdge(e.source, e.target)) continue;
        g.addEdgeWithKey(key, e.source, e.target, {
            size: 2,
            color: colourFor(e.colour),
            colourClass: e.colour,
        });
    }
    return g;
}

function buildLegend(meta, data) {
    const classes = [...new Set(data.edges.map(e => e.colour))].sort((a, b) => a - b);
    const parts = classes.map(c =>
        `<span class="mpv-graph-legend-item"><span class="mpv-graph-swatch" style="background:${colourFor(c)}"></span>colour&nbsp;${c}</span>`
    );
    const metaBits = [];
    if (meta.nodes != null) metaBits.push(`${meta.nodes} vertices`);
    if (meta.edges != null) metaBits.push(`${meta.edges} edges`);
    if (meta.max_degree != null) metaBits.push(`max degree ${meta.max_degree}`);
    if (meta.chromatic_index != null) metaBits.push(`χ′ = ${meta.chromatic_index}`);
    return (
        `<div class="mpv-graph-meta">${metaBits.join(' · ')}</div>` +
        `<div class="mpv-graph-legend">${parts.join('')}</div>`
    );
}

function makeContainer(data) {
    const wrap = document.createElement('div');
    wrap.className = 'mpv-graph';
    wrap.innerHTML =
        buildLegend(data.meta || {}, data) +
        '<div class="mpv-graph-canvas"></div>' +
        '<div class="mpv-graph-hint">drag to pan · scroll to zoom · hover edges to see colour class</div>';
    return wrap;
}

function attachTooltip(renderer, wrap) {
    const tip = document.createElement('div');
    tip.className = 'mpv-graph-tooltip';
    tip.style.display = 'none';
    wrap.appendChild(tip);

    renderer.on('enterEdge', ({ edge }) => {
        const g = renderer.getGraph();
        const attrs = g.getEdgeAttributes(edge);
        tip.innerHTML = `edge ${g.source(edge)}–${g.target(edge)} · colour ${attrs.colourClass}`;
        tip.style.display = 'block';
    });
    renderer.on('leaveEdge', () => { tip.style.display = 'none'; });
    renderer.on('moveBody', () => { tip.style.display = 'none'; });
    wrap.querySelector('.mpv-graph-canvas').addEventListener('mousemove', (e) => {
        if (tip.style.display !== 'block') return;
        const rect = wrap.getBoundingClientRect();
        tip.style.left = (e.clientX - rect.left + 10) + 'px';
        tip.style.top = (e.clientY - rect.top + 10) + 'px';
    });
}

async function renderOne(codeEl, imageBase) {
    const rawRef = (codeEl.textContent || '').trim();
    if (!rawRef) return;
    const cleanRef = rawRef.replace(/^\.\//, '');
    const url = /^https?:/i.test(cleanRef) ? cleanRef : imageBase + cleanRef;

    const pre = codeEl.closest('pre');
    if (!pre) return;

    // Placeholder while loading
    const loading = document.createElement('div');
    loading.className = 'mpv-graph-loading';
    loading.textContent = 'loading interactive graph…';
    pre.replaceWith(loading);

    let data;
    try {
        const r = await fetch(url);
        if (!r.ok) throw new Error(`${r.status}`);
        data = await r.json();
    } catch (err) {
        loading.className = 'mpv-graph-error';
        loading.textContent = 'could not load graph (' + err.message + ')';
        return;
    }

    const wrap = makeContainer(data);
    loading.replaceWith(wrap);

    const graph = buildGraph(data);
    const canvas = wrap.querySelector('.mpv-graph-canvas');
    const renderer = new Sigma(graph, canvas, {
        renderLabels: false,
        renderEdgeLabels: false,
        enableEdgeEvents: true,
        defaultNodeColor: '#5a6170',
        minCameraRatio: 0.05,
        maxCameraRatio: 5,
    });
    activeRenderers.set(wrap, renderer);
    attachTooltip(renderer, wrap);

    // Observe removal to free WebGL resources
    const obs = new MutationObserver(() => {
        if (!document.body.contains(wrap)) {
            renderer.kill();
            obs.disconnect();
        }
    });
    obs.observe(document.body, { childList: true, subtree: true });
}

export function renderGraphBlocks(containerEl, imageBase) {
    if (!containerEl) return;
    const blocks = containerEl.querySelectorAll('pre > code.language-mipviz-graph');
    blocks.forEach(code => { renderOne(code, imageBase); });
}

import Graph from 'https://esm.sh/graphology@0.25.4';
import Sigma from 'https://esm.sh/sigma@3.0.2';
import { EdgeArrowProgram } from 'https://esm.sh/sigma@3.0.2/rendering';
import forceAtlas2 from 'https://esm.sh/graphology-layout-forceatlas2@0.10.1';

// ── Extract instance name from URL ─────────────────────────────────
const instanceName = location.hash.startsWith('#')
    ? decodeURIComponent(location.hash.slice(1))
    : null;
if (!instanceName) throw new Error('No instance name in URL hash');
const solverParam = new URLSearchParams(location.search).get('solver') || 'highs';

function normalizeSCIPData(scip) {
    // Convert CliquesImplicationsResponse to CliqueResponse format
    const variables = {};
    const cliques = scip.cliques.map(c => {
        return c.members.map(m => {
            const col = m.var_index != null ? m.var_index : m.col;
            const val = m.value != null ? m.value : m.val;
            if (!(col in variables)) {
                variables[col] = { name: m.var_name || ('x' + col) };
            }
            return { col, val };
        });
    });
    return { cliques, variables, num_cliques: scip.num_cliques };
}

// ── Theme: handled by theme.js, hook for label color updates ──────
window.onThemeChange = function (isLight) {
    if (renderer) {
        renderer.setSetting('labelColor', { color: isLight ? '#27272a' : '#e4e4e7' });
        renderer.refresh();
    }
};

// ── Colors ─────────────────────────────────────────────────────────
const COLOR_POS = '#2563eb';
const COLOR_NEG = '#ea580c';
const COLOR_TRUE = '#16a34a';
const COLOR_FALSE = '#71717a';
const COLOR_FRESH = '#facc15';
const COLOR_EDGE = '#2a2a30';
const COLOR_EDGE_HOVER = '#5a5a6a';
const COLOR_BRIDGE = '#ef4444';
const COLOR_ARTIC = '#facc15';
const COLOR_IMPL = '#a855f7';
const NODE_SIZE = 5;

const COMM_PALETTE = [
    '#2563eb', '#16a34a', '#ea580c', '#a855f7', '#ec4899',
    '#06b6d4', '#eab308', '#f97316', '#6366f1', '#14b8a6',
];

// ── State ──────────────────────────────────────────────────────────
let cliqueData = null;
let graph = null;
let renderer = null;

let hoveredNode = null;
let hoveredNeighbors = new Set();
let draggedNode = null;
let isDragging = false;

// Overlay toggles
let showCommunities = false;
let showBridges = false;
let showImplications = false;
let highlightedClique = -1;
let cliqueNodeSet = new Set();

// Structural data
let communities = null;    // Map<nodeKey, communityId>
let numCommunities = 0;
let bridgeSet = new Set();
let articulationSet = new Set();
let implEdges = null;      // Array of { from, to } (node keys)

// Propagation state
let propActive = false;
let propRoots = [];
let propSteps = [];
let propCurrentStep = -1;
let nodeState = new Map();  // nodeKey → 'true'|'false'
let propConflict = false;
let cliquesOf = null;       // Map<nodeKey, cliqueIndex[]>

// ── Fetch data ─────────────────────────────────────────────────────
document.title = instanceName + ' — Conflict Graph';

API.ensureReady().then(() => {
    return fetch('https://media.githubusercontent.com/media/mmghannam/mipviz-instances/main/instances/' + encodeURIComponent(instanceName) + '.mps.gz');
}).then(res => {
    if (!res.ok) throw new Error('Instance not found');
    return res.blob();
}).then(blob => {
    const file = new File([blob], instanceName + '.mps.gz', { type: 'application/gzip' });
    if (solverParam === 'scip') {
        return API.getCliquesImplications(file, 'scip').then(normalizeSCIPData);
    }
    return API.getCliques(file);
}).then(data => {
    cliqueData = data;
    if (data.num_cliques === 0) {
        document.getElementById('conflict-loading').innerHTML =
            '<div class="matrix-loading-text">No cliques found after presolve.</div>';
        return;
    }
    graph = buildGraph(data);
    computeImplications(data);
    detectCommunities();
    detectBridgesAndArticulations();
    initRenderer();
}).catch(err => {
    console.error('Conflict graph load error:', err);
    document.getElementById('conflict-loading').innerHTML =
        '<div class="matrix-loading-text" style="color:var(--orange)">Failed to load: ' + err.message + '</div>';
});

// ── Build graph from cliques ───────────────────────────────────────
function nodeKey(col, val) {
    return col + ':' + (val ? '1' : '0');
}

function buildGraph(data) {
    const g = new Graph();
    const edgeSeen = new Set();

    for (const clique of data.cliques) {
        // Add nodes
        for (const entry of clique) {
            const key = nodeKey(entry.col, entry.val);
            if (!g.hasNode(key)) {
                const v = data.variables[entry.col];
                g.addNode(key, {
                    col: entry.col,
                    val: entry.val,
                    label: v ? v.name : 'x' + entry.col,
                    size: NODE_SIZE,
                    color: entry.val ? COLOR_POS : COLOR_NEG,
                    x: Math.random() * 100 - 50,
                    y: Math.random() * 100 - 50,
                });
            }
        }
        // Add edges between all pairs in clique
        for (let i = 0; i < clique.length; i++) {
            const ki = nodeKey(clique[i].col, clique[i].val);
            for (let j = i + 1; j < clique.length; j++) {
                const kj = nodeKey(clique[j].col, clique[j].val);
                const ek = ki < kj ? ki + '|' + kj : kj + '|' + ki;
                if (!edgeSeen.has(ek)) {
                    edgeSeen.add(ek);
                    g.addUndirectedEdgeWithKey(ek, ki, kj, {
                        color: COLOR_EDGE,
                        implication: false,
                    });
                }
            }
        }
    }
    return g;
}

// ── Implication edges from cliques ─────────────────────────────────
function computeImplications(data) {
    implEdges = [];
    const edgeSeen = new Set();

    for (const clique of data.cliques) {
        for (let i = 0; i < clique.length; i++) {
            const fromKey = nodeKey(clique[i].col, clique[i].val);
            if (!graph.hasNode(fromKey)) continue;

            for (let j = 0; j < clique.length; j++) {
                if (i === j) continue;
                const compVal = !clique[j].val;
                const toKey = nodeKey(clique[j].col, compVal);

                // Ensure complement node exists
                if (!graph.hasNode(toKey)) {
                    const v = data.variables[clique[j].col];
                    graph.addNode(toKey, {
                        col: clique[j].col,
                        val: compVal,
                        label: v ? v.name : 'x' + clique[j].col,
                        size: NODE_SIZE,
                        color: compVal ? COLOR_POS : COLOR_NEG,
                        x: Math.random() * 100 - 50,
                        y: Math.random() * 100 - 50,
                    });
                }

                if (fromKey === toKey) continue;
                const ek = fromKey + '>' + toKey;
                if (edgeSeen.has(ek)) continue;
                edgeSeen.add(ek);
                implEdges.push({ from: fromKey, to: toKey });

                // Add to graph as hidden directed edge
                if (!graph.hasDirectedEdge(fromKey, toKey)) {
                    graph.addDirectedEdgeWithKey('impl:' + ek, fromKey, toKey, {
                        color: COLOR_IMPL,
                        implication: true,
                        type: 'arrow',
                        size: 2,
                    });
                }
            }
        }
    }
    console.log('Implications:', implEdges.length, 'edges, nodes:', graph.order);
}

// ── Community detection (label propagation) ─────────────────────────
function detectCommunities() {
    const n = graph.order;
    communities = new Map();
    const nodeKeys = graph.nodes();

    // Initialize each node to its own community
    nodeKeys.forEach((key, i) => communities.set(key, i));

    // Label propagation iterations
    const order = [...nodeKeys];
    let changed = true;
    for (let iter = 0; iter < 20 && changed; iter++) {
        changed = false;
        // Shuffle
        for (let i = order.length - 1; i > 0; i--) {
            const j = Math.floor(Math.random() * (i + 1));
            [order[i], order[j]] = [order[j], order[i]];
        }
        for (const key of order) {
            const neighbors = graph.neighbors(key);
            if (neighbors.length === 0) continue;
            const counts = {};
            let bestLabel = communities.get(key), bestCount = 0;
            for (const nb of neighbors) {
                const l = communities.get(nb);
                counts[l] = (counts[l] || 0) + 1;
                if (counts[l] > bestCount) { bestCount = counts[l]; bestLabel = l; }
            }
            if (bestLabel !== communities.get(key)) {
                communities.set(key, bestLabel);
                changed = true;
            }
        }
    }

    // Remap to sequential ids sorted by size
    const labelMap = {};
    numCommunities = 0;
    const commSizes = [];
    for (const key of nodeKeys) {
        const l = communities.get(key);
        if (!(l in labelMap)) { labelMap[l] = numCommunities++; commSizes.push(0); }
        communities.set(key, labelMap[l]);
        commSizes[labelMap[l]]++;
    }
    const sizeOrder = Array.from({ length: numCommunities }, (_, i) => i)
        .sort((a, b) => commSizes[b] - commSizes[a]);
    const remap = new Array(numCommunities);
    sizeOrder.forEach((c, i) => remap[c] = i);
    for (const key of nodeKeys) {
        communities.set(key, remap[communities.get(key)]);
    }
}

// ── Bridge & articulation point detection (Tarjan's) ────────────────
function detectBridgesAndArticulations() {
    bridgeSet = new Set();
    articulationSet = new Set();

    const nodeKeys = graph.nodes();
    const n = nodeKeys.length;
    const keyToIdx = new Map();
    nodeKeys.forEach((k, i) => keyToIdx.set(k, i));

    // Build adjacency list (undirected conflict edges only)
    const adj = Array.from({ length: n }, () => []);
    let edgeIdx = 0;
    const edgeList = [];
    graph.forEachUndirectedEdge((edge, attrs, source, target) => {
        const si = keyToIdx.get(source), ti = keyToIdx.get(target);
        const ei = edgeIdx++;
        adj[si].push({ to: ti, ei, source, target });
        adj[ti].push({ to: si, ei, source, target });
        edgeList.push({ source, target });
    });

    const disc = new Int32Array(n).fill(-1);
    const low = new Int32Array(n);
    let timer = 0;

    for (let start = 0; start < n; start++) {
        if (disc[start] !== -1) continue;
        const stack = [[start, -1, 0]];
        disc[start] = low[start] = timer++;
        const rootChildren = {};

        while (stack.length > 0) {
            const top = stack[stack.length - 1];
            const u = top[0], parentEi = top[1];
            const adjList = adj[u];

            if (top[2] < adjList.length) {
                const edge = adjList[top[2]++];
                const v = edge.to, ei = edge.ei;
                if (ei === parentEi) continue;

                if (disc[v] === -1) {
                    disc[v] = low[v] = timer++;
                    stack.push([v, ei, 0]);
                } else {
                    low[u] = Math.min(low[u], disc[v]);
                }
            } else {
                stack.pop();
                if (stack.length > 0) {
                    const parent = stack[stack.length - 1];
                    const pu = parent[0];
                    low[pu] = Math.min(low[pu], low[u]);

                    // Bridge check
                    if (low[u] > disc[pu]) {
                        const e = edgeList[parentEi];
                        const bk = e.source < e.target ? e.source + '|' + e.target : e.target + '|' + e.source;
                        bridgeSet.add(bk);
                    }

                    // Articulation point check
                    const isRoot = stack.length === 1;
                    if (isRoot) {
                        if (!rootChildren[pu]) rootChildren[pu] = 0;
                        rootChildren[pu]++;
                        if (rootChildren[pu] >= 2) articulationSet.add(nodeKeys[pu]);
                    } else {
                        if (low[u] >= disc[pu]) articulationSet.add(nodeKeys[pu]);
                    }
                }
            }
        }
    }
}

// ── Build clique-to-node lookup ──────────────────────────────────────
function buildCliqueLookup() {
    cliquesOf = new Map();
    graph.forEachNode(key => cliquesOf.set(key, []));
    for (let ci = 0; ci < cliqueData.cliques.length; ci++) {
        const clique = cliqueData.cliques[ci];
        for (const entry of clique) {
            const key = nodeKey(entry.col, entry.val);
            if (cliquesOf.has(key)) cliquesOf.get(key).push(ci);
        }
    }
}

// ── BCP propagation ────────────────────────────────────────────────
function bcpFromLiterals(startNodes, tmpState) {
    let currentTrue = startNodes.slice();
    const initFalse = [];
    for (const sn of startNodes) {
        const attrs = graph.getNodeAttributes(sn);
        const ck = nodeKey(attrs.col, !attrs.val);
        if (graph.hasNode(ck) && tmpState.get(ck) === undefined) {
            tmpState.set(ck, 'false');
            initFalse.push(ck);
        }
    }
    propSteps.push({ setTrue: startNodes.slice(), setFalse: initFalse, conflict: false, isChoice: true });

    while (currentTrue.length > 0) {
        const stepFalse = [], nextTrue = [];
        let conflict = false;
        for (const ni of currentTrue) {
            const cliqueIds = cliquesOf.get(ni) || [];
            for (const ci of cliqueIds) {
                const clique = cliqueData.cliques[ci];
                for (const entry of clique) {
                    const key = nodeKey(entry.col, entry.val);
                    if (!graph.hasNode(key) || key === ni) continue;
                    if (tmpState.get(key) === 'true') { conflict = true; continue; }
                    if (tmpState.get(key) === 'false') continue;
                    tmpState.set(key, 'false');
                    stepFalse.push(key);
                    const compKey = nodeKey(entry.col, !entry.val);
                    if (graph.hasNode(compKey) && tmpState.get(compKey) === undefined) {
                        tmpState.set(compKey, 'true');
                        nextTrue.push(compKey);
                    } else if (graph.hasNode(compKey) && tmpState.get(compKey) === 'false') {
                        conflict = true;
                    }
                }
            }
        }
        if (stepFalse.length > 0 || nextTrue.length > 0 || conflict) {
            propSteps.push({ setTrue: nextTrue, setFalse: stepFalse, conflict, isChoice: false });
        }
        currentTrue = nextTrue;
    }
}

function recomputeAllSteps() {
    if (!cliquesOf) buildCliqueLookup();
    propSteps = [];
    const tmpState = new Map();
    for (const ni of propRoots) {
        if (tmpState.has(ni)) continue;
        tmpState.set(ni, 'true');
        bcpFromLiterals([ni], tmpState);
    }
}

function applyStepsUpTo(step) {
    nodeState = new Map();
    propConflict = false;
    for (let s = 0; s <= step && s < propSteps.length; s++) {
        const st = propSteps[s];
        for (const k of st.setTrue) nodeState.set(k, 'true');
        for (const k of st.setFalse) nodeState.set(k, 'false');
        if (st.conflict) propConflict = true;
    }
}

function propagateNode(nk) {
    if (!cliquesOf) buildCliqueLookup();
    const rootIdx = propRoots.indexOf(nk);
    if (rootIdx !== -1) {
        propRoots.splice(rootIdx, 1);
        if (propRoots.length === 0) { resetPropagation(); return; }
        recomputeAllSteps();
        propCurrentStep = propSteps.length - 1;
        applyStepsUpTo(propCurrentStep);
        updateStats();
        renderer.refresh();
        return;
    }
    if (propActive && nodeState.has(nk)) return;
    propRoots.push(nk);
    recomputeAllSteps();
    propActive = true;
    propCurrentStep = propSteps.length - 1;
    applyStepsUpTo(propCurrentStep);
    updateStats();
    renderer.refresh();
}

function stepForward() {
    if (!propActive || propCurrentStep >= propSteps.length - 1) return;
    propCurrentStep++;
    applyStepsUpTo(propCurrentStep);
    updateStats();
    renderer.refresh();
}

function stepBack() {
    if (!propActive || propCurrentStep <= 0) return;
    propCurrentStep--;
    applyStepsUpTo(propCurrentStep);
    updateStats();
    renderer.refresh();
}

function resetPropagation() {
    propActive = false;
    propRoots = [];
    propConflict = false;
    propSteps = [];
    propCurrentStep = -1;
    nodeState = new Map();
    updateStats();
    if (renderer) renderer.refresh();
}

// ── Initialize renderer ────────────────────────────────────────────
function initRenderer() {
    document.getElementById('conflict-loading').style.display = 'none';
    const container = document.getElementById('conflict-container');
    container.style.display = '';

    const sigmaContainer = document.getElementById('sigma-container');

    // Fresh set for current propagation step
    function getFreshSet() {
        const fresh = new Set();
        if (propActive && propCurrentStep >= 0 && propCurrentStep < propSteps.length) {
            const cur = propSteps[propCurrentStep];
            for (const k of cur.setTrue) fresh.add(k);
            for (const k of cur.setFalse) fresh.add(k);
        }
        return fresh;
    }

    const isLight = document.documentElement.classList.contains('light');

    renderer = new Sigma(graph, sigmaContainer, {
        allowInvalidContainer: true,
        renderLabels: true,
        labelRenderedSizeThreshold: 6,
        labelFont: '"JetBrains Mono", monospace',
        labelSize: 10,
        labelColor: { color: isLight ? '#27272a' : '#e4e4e7' },
        defaultEdgeType: 'line',
        defaultNodeType: 'circle',
        minCameraRatio: 0.01,
        maxCameraRatio: 50,
        edgeProgramClasses: {
            arrow: EdgeArrowProgram,
        },

        nodeReducer: (node, data) => {
            const res = { ...data };
            const freshSet = getFreshSet();
            const isFresh = freshSet.has(node);
            const isRoot = propRoots.includes(node);
            const isInClique = highlightedClique >= 0 && cliqueNodeSet.has(node);
            const isArtic = showBridges && articulationSet.has(node);

            // Base color
            if (showCommunities && communities && !propActive) {
                res.color = COMM_PALETTE[communities.get(node) % COMM_PALETTE.length];
            }

            // Propagation coloring
            if (propActive) {
                const st = nodeState.get(node);
                if (st === 'true') {
                    res.color = COLOR_TRUE;
                } else if (st === 'false') {
                    res.color = COLOR_FALSE;
                    if (!hoveredNode || (node !== hoveredNode && !hoveredNeighbors.has(node))) {
                        res.color = COLOR_FALSE;
                    }
                }
                if (isFresh) res.size = NODE_SIZE * 1.6;
            }

            // Clique highlighting
            if (highlightedClique >= 0 && !propActive) {
                if (isInClique) {
                    res.size = NODE_SIZE * 1.4;
                    res.zIndex = 1;
                } else if (node !== hoveredNode) {
                    res.color = '#333340';
                    res.label = '';
                }
            }

            // Hover dimming
            if (hoveredNode && !propActive && highlightedClique < 0) {
                if (node !== hoveredNode && !hoveredNeighbors.has(node)) {
                    res.color = '#333340';
                    res.label = '';
                }
            }

            if (hoveredNode && node === hoveredNode) {
                res.highlighted = true;
                res.size = NODE_SIZE * 1.5;
                res.zIndex = 2;
            }

            // Visual cues for special nodes (size + highlight)
            if (propActive && isRoot) {
                res.highlighted = true;
                res.size = NODE_SIZE * 1.8;
            }
            if (isArtic && !propActive) {
                res.highlighted = true;
            }

            return res;
        },

        edgeReducer: (edge, data) => {
            const res = { ...data };
            const isImpl = data.implication;

            // Hide implication edges when toggle is off
            if (isImpl && !showImplications) {
                res.hidden = true;
                return res;
            }

            // Hide non-implication when only showing implications
            if (!isImpl) {
                const source = graph.source(edge);
                const target = graph.target(edge);
                const ek = source < target ? source + '|' + target : target + '|' + source;
                const isBridge = showBridges && bridgeSet.has(ek);
                const isCliqueEdge = highlightedClique >= 0 && cliqueNodeSet.has(source) && cliqueNodeSet.has(target);
                const isHov = hoveredNode && (source === hoveredNode || target === hoveredNode);

                if (isBridge) {
                    res.color = COLOR_BRIDGE;
                    res.size = 2.5;
                }
                if (isCliqueEdge) {
                    res.color = COLOR_IMPL;
                    res.size = 2;
                }

                // Propagation dimming
                if (propActive) {
                    const ss = nodeState.get(source);
                    const ts = nodeState.get(target);
                    if (ss === 'false' || ts === 'false') {
                        res.hidden = !isHov;
                    }
                } else if (highlightedClique >= 0 && !isCliqueEdge && !isHov) {
                    res.hidden = true;
                } else if (hoveredNode && !isHov) {
                    res.hidden = true;
                }

                if (isHov) {
                    res.color = COLOR_EDGE_HOVER;
                    res.size = 1.5;
                }
            }

            // Implication edge: dim when not relevant
            if (isImpl && showImplications) {
                const source = graph.source(edge);
                const target = graph.target(edge);
                const isHov = hoveredNode && (source === hoveredNode || target === hoveredNode);

                if (propActive) {
                    const fs = nodeState.get(source);
                    const ts = nodeState.get(target);
                    if (fs === 'false' || ts === 'false') res.hidden = true;
                } else if (hoveredNode && !isHov) {
                    res.hidden = true;
                }
            }

            return res;
        },
    });

    // ── FA2 layout (animated, synchronous) ───────────────────────
    const fa2Settings = {
        ...forceAtlas2.inferSettings(graph),
        gravity: 1,
        barnesHutOptimize: graph.order > 500,
    };
    let layoutIterations = 0;
    const maxLayoutIterations = 600;
    let layoutRunning = true;

    function layoutLoop() {
        if (!layoutRunning || layoutIterations >= maxLayoutIterations) return;
        // Run a few iterations per frame for speed
        const batch = Math.min(5, maxLayoutIterations - layoutIterations);
        forceAtlas2.assign(graph, { iterations: batch, settings: fa2Settings });
        layoutIterations += batch;
        requestAnimationFrame(layoutLoop);
    }
    requestAnimationFrame(layoutLoop);

    // ── Events ─────────────────────────────────────────────────────
    renderer.on('enterNode', ({ node }) => {
        hoveredNode = node;
        hoveredNeighbors = new Set(graph.neighbors(node));
        showTooltipForNode(node);
        renderer.refresh();
    });

    renderer.on('leaveNode', () => {
        hoveredNode = null;
        hoveredNeighbors = new Set();
        hideTooltip();
        renderer.refresh();
    });

    renderer.on('clickNode', ({ node, event }) => {
        propagateNode(node);
    });

    renderer.on('clickStage', () => {
        if (propActive) resetPropagation();
    });

    // Node drag
    renderer.on('downNode', (e) => {
        isDragging = true;
        draggedNode = e.node;
        graph.setNodeAttribute(draggedNode, 'highlighted', true);
        renderer.getCamera().disable();
    });

    renderer.getMouseCaptor().on('mousemovebody', (e) => {
        if (!isDragging || !draggedNode) return;
        const pos = renderer.viewportToGraph(e);
        graph.setNodeAttribute(draggedNode, 'x', pos.x);
        graph.setNodeAttribute(draggedNode, 'y', pos.y);
    });

    renderer.getMouseCaptor().on('mouseup', () => {
        if (draggedNode) {
            graph.removeNodeAttribute(draggedNode, 'highlighted');
            draggedNode = null;
        }
        isDragging = false;
        renderer.getCamera().enable();
    });

    // Track mouse position for tooltip
    renderer.getMouseCaptor().on('mousemovebody', (e) => {
        if (hoveredNode) {
            const tooltip = document.getElementById('conflict-tooltip');
            tooltip.style.left = (e.original.clientX + 15) + 'px';
            tooltip.style.top = (e.original.clientY - 10) + 'px';
        }
    });

    // Keyboard
    document.addEventListener('keydown', (e) => {
        if (e.key === 'Escape' && propActive) resetPropagation();
        else if (e.key === 'ArrowRight' && propActive) { e.preventDefault(); stepForward(); }
        else if (e.key === 'ArrowLeft' && propActive) { e.preventDefault(); stepBack(); }
    });

    // Build UI
    buildCliquePanel();
    setupCliquePanelToggle();
    updateStats();
}

function setupCliquePanelToggle() {
    const panel = document.getElementById('clique-panel');
    const container = document.getElementById('conflict-container');
    const toggle = document.getElementById('clique-panel-toggle');
    if (!toggle) return;
    toggle.addEventListener('click', () => {
        const collapsed = panel.classList.toggle('collapsed');
        container.classList.toggle('clique-panel-collapsed', collapsed);
        toggle.setAttribute('aria-label', collapsed ? 'Expand cliques panel' : 'Collapse cliques panel');
        toggle.setAttribute('title', collapsed ? 'Expand' : 'Collapse');
        // Sigma needs to know the container size hasn't actually changed,
        // but a refresh ensures any label re-rendering is consistent.
        if (renderer) renderer.refresh();
    });
}

// ── Clique panel ───────────────────────────────────────────────────
function buildCliquePanel() {
    const listEl = document.getElementById('clique-list');
    let html = '';
    for (let ci = 0; ci < cliqueData.cliques.length; ci++) {
        const clique = cliqueData.cliques[ci];
        const literals = clique.map(entry => {
            const v = cliqueData.variables[entry.col];
            const name = v ? v.name : 'x' + entry.col;
            const cls = entry.val ? 'clique-literal-pos' : 'clique-literal-neg';
            const sign = entry.val ? '' : '\u00ac';
            return '<span class="' + cls + '">' + sign + name + '</span>';
        });
        html += '<div class="clique-item" data-ci="' + ci + '">' +
            '<span class="clique-size">[' + clique.length + ']</span> ' +
            literals.join(', ') + '</div>';
    }
    listEl.innerHTML = html;

    listEl.addEventListener('click', (e) => {
        const item = e.target.closest('.clique-item');
        if (!item) return;
        const ci = parseInt(item.getAttribute('data-ci'));
        selectClique(ci);
    });
}

function selectClique(ci) {
    if (highlightedClique === ci) {
        highlightedClique = -1;
        cliqueNodeSet = new Set();
    } else {
        highlightedClique = ci;
        cliqueNodeSet = new Set();
        const clique = cliqueData.cliques[ci];
        for (const entry of clique) {
            const key = nodeKey(entry.col, entry.val);
            if (graph.hasNode(key)) cliqueNodeSet.add(key);
        }
    }

    const items = document.querySelectorAll('.clique-item');
    for (const item of items) {
        item.classList.toggle('active', parseInt(item.getAttribute('data-ci')) === highlightedClique);
    }
    renderer.refresh();
}

// ── Stats bar ──────────────────────────────────────────────────────
function updateStats() {
    const statsEl = document.getElementById('conflict-stats');
    const undirectedEdgeCount = graph.undirectedSize;

    let base = '<span class="conflict-stat">' + graph.order + ' nodes</span>' +
        '<span class="conflict-stat">' + undirectedEdgeCount + ' edges</span>' +
        '<span class="conflict-stat">' + cliqueData.num_cliques + ' cliques</span>' +
        '<span class="conflict-stat legend-pos">x=1</span>' +
        '<span class="conflict-stat legend-neg">x=0</span>';

    if (propActive) {
        let nTrue = 0, nFalse = 0, nFree = 0;
        graph.forEachNode(key => {
            const s = nodeState.get(key);
            if (s === 'true') nTrue++;
            else if (s === 'false') nFalse++;
            else nFree++;
        });
        base += '<span class="conflict-stat" style="color:var(--green)">fixed: ' + nTrue + '</span>';
        base += '<span class="conflict-stat" style="color:var(--text-muted)">eliminated: ' + nFalse + '</span>';
        base += '<span class="conflict-stat">free: ' + nFree + '</span>';
        base += '<span class="conflict-stat">step ' + (propCurrentStep + 1) + ' / ' + propSteps.length + '</span>';
        if (propConflict) base += '<span class="conflict-stat" style="color:#ef4444">CONFLICT</span>';
        base += '<span class="conflict-stat" style="opacity:0.6">\u2190 \u2192 step \u00b7 Esc reset</span>';
    }

    const nBridges = bridgeSet.size;
    const nArtic = articulationSet.size;
    base += '<span class="conflict-stat conflict-toggle' + (showCommunities ? ' active' : '') + '" id="toggle-communities">' +
        'Communities' + (communities ? ' (' + numCommunities + ')' : '') + '</span>';
    base += '<span class="conflict-stat conflict-toggle' + (showBridges ? ' active' : '') + '" id="toggle-bridges">' +
        'Bridges (' + nBridges + ') / Artic. (' + nArtic + ')</span>';
    base += '<span class="conflict-stat conflict-toggle' + (showImplications ? ' active' : '') + '" id="toggle-implications">' +
        'Implications' + (implEdges ? ' (' + implEdges.length + ')' : '') + '</span>';

    statsEl.innerHTML = base;

    // Wire up toggle buttons
    document.getElementById('toggle-communities')?.addEventListener('click', () => {
        showCommunities = !showCommunities;
        updateStats();
        renderer.refresh();
    });
    document.getElementById('toggle-bridges')?.addEventListener('click', () => {
        showBridges = !showBridges;
        updateStats();
        renderer.refresh();
    });
    document.getElementById('toggle-implications')?.addEventListener('click', () => {
        showImplications = !showImplications;
        updateStats();
        renderer.refresh();
    });
}

// ── Tooltip ────────────────────────────────────────────────────────
function showTooltipForNode(nk) {
    const tooltip = document.getElementById('conflict-tooltip');
    const attrs = graph.getNodeAttributes(nk);
    const v = cliqueData.variables[attrs.col];
    if (!v) { hideTooltip(); return; }

    const literal = attrs.val ? '= 1' : '= 0';
    const typeStr = v.var_type.charAt(0).toUpperCase() + v.var_type.slice(1);
    const lb = v.lower !== null ? v.lower : '-inf';
    const ub = v.upper !== null ? v.upper : '+inf';
    const edgeCount = graph.degree(nk);

    let html =
        '<div class="ct-name">' + v.name + ' <span class="ct-literal">' + literal + '</span></div>' +
        '<div class="ct-row">Type: ' + typeStr + '</div>' +
        '<div class="ct-row">Bounds: [' + lb + ', ' + ub + ']</div>' +
        '<div class="ct-row">Obj: ' + v.obj + '</div>' +
        '<div class="ct-row">Edges: ' + edgeCount + '</div>';

    if (showCommunities && communities) {
        html += '<div class="ct-row">Community: ' + communities.get(nk) + '</div>';
    }
    if (showBridges && articulationSet.has(nk)) {
        html += '<div class="ct-row" style="color:#facc15">Articulation point</div>';
    }

    if (propActive && nodeState.has(nk)) {
        const st = nodeState.get(nk);
        const stateLabel = st === 'true' ? 'fixed TRUE' : 'fixed FALSE';
        const stateColor = st === 'true' ? 'var(--green)' : 'var(--text-muted)';
        let fixedAt = -1;
        for (let s = 0; s <= propCurrentStep && s < propSteps.length; s++) {
            const step = propSteps[s];
            if (step.setTrue.includes(nk) || step.setFalse.includes(nk)) { fixedAt = s; break; }
        }
        html += '<div class="ct-row" style="color:' + stateColor + ';margin-top:2px">' + stateLabel;
        if (fixedAt >= 0) html += ' (step ' + (fixedAt + 1) + ')';
        html += '</div>';
    }

    tooltip.innerHTML = html;
    tooltip.style.display = '';
}

function hideTooltip() {
    document.getElementById('conflict-tooltip').style.display = 'none';
}

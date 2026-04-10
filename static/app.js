const BATCH_SIZE = 500; // v51

// Toast notification
var toastEl = null;
var toastTimer = null;
function showToast(msg, duration) {
    if (!toastEl) {
        toastEl = document.createElement('div');
        toastEl.className = 'toast';
        document.body.appendChild(toastEl);
    }
    toastEl.textContent = msg;
    toastEl.classList.remove('toast-fade');
    toastEl.classList.add('toast-visible');
    clearTimeout(toastTimer);
    toastTimer = setTimeout(function() {
        toastEl.classList.add('toast-fade');
    }, duration || (msg.toLowerCase().includes('error') ? 4000 : 1500));
}
const INF_THRESHOLD = 1e20;
let modelData = null;
let originalModelData = null;
let isPresolved = false;
let currentSolver = 'scip';
let currentInstanceName = null;
let benchmarkInstances = null; // Set of instance keys in benchmark data, loaded lazily
function loadBenchmarkIndex() {
    if (benchmarkInstances) return benchmarkInstances;
    benchmarkInstances = fetch('./benchmark-12threads-details.json')
        .then(r => r.json())
        .then(data => { benchmarkInstances = new Set(Object.keys(data)); return benchmarkInstances; })
        .catch(() => { benchmarkInstances = new Set(); return benchmarkInstances; });
    return benchmarkInstances;
}
let currentUploadFile = null;
let constraintsShown = 0;
let mathMode = false;
let activeTypeFilter = null;
let activeVarFilter = null;
let activeComponentFilter = null; // { index, rowSet, colSet } or null
let activeConstraintVarFilter = null; // constraint index or null
let lpSolution = null; // { col_values, objective_value, status } or null

const dropZone = document.getElementById('drop-zone');
const fileInput = document.getElementById('file-input');
const uploadSection = document.getElementById('upload-section');
const heroSection = document.getElementById('hero-section');
const uploadStatus = document.getElementById('upload-status');
const resultsSection = document.getElementById('results-section');
const modelNameEl = document.getElementById('model-name');
const statsGrid = document.getElementById('stats-grid');
const constraintsList = document.getElementById('constraints-list');
const constraintsCount = document.getElementById('constraints-count');
const showMoreBtn = document.getElementById('show-more-btn');
const newUploadBtn = document.getElementById('new-upload-btn');
const downloadBtn = document.getElementById('download-btn');
const mathToggle = document.getElementById('math-toggle');
const presolveBtn = document.getElementById('presolve-btn');
const cliquesPanel = document.getElementById('cliques-panel');
const symmetryPanel = document.getElementById('symmetry-panel');
const solveLpBtn = document.getElementById('solve-lp-btn');
const solveMipBtn = document.getElementById('solve-mip-btn');
const filterPill = document.getElementById('active-filter-pill');

function updateFilterPill() {
    if (!filterPill) return;
    var parts = [];
    if (activeTypeFilter) parts.push('Type: <strong>' + escapeHtml(activeTypeFilter) + '</strong>');
    if (activeVarFilter) {
        var vName = modelData && modelData.variables[activeVarFilter] ? modelData.variables[activeVarFilter].name : 'x' + activeVarFilter;
        parts.push('Variable: <strong>' + escapeHtml(vName) + '</strong>');
    }
    if (activeComponentFilter) parts.push('Component: <strong>' + activeComponentFilter.stat.index + '</strong>');
    if (activeConstraintVarFilter != null) {
        var cName = modelData && modelData.constraints[activeConstraintVarFilter] ? modelData.constraints[activeConstraintVarFilter].name : '#' + activeConstraintVarFilter;
        parts.push('Constraint: <strong>' + escapeHtml(cName) + '</strong>');
    }
    if (parts.length === 0) {
        filterPill.classList.add('hidden');
        updateHashState();
        return;
    }
    filterPill.innerHTML = '<span class="filter-label">Filtered by</span> ' + parts.join(' + ') +
        ' <button class="filter-clear" onclick="clearAllFilters()">Clear all</button>';
    filterPill.classList.remove('hidden');
    updateHashState();
}

function clearAllFilters() {
    if (activeTypeFilter) {
        activeTypeFilter = null;
        document.querySelectorAll('.type-tag').forEach(function(t) { t.classList.remove('active'); });
    }
    if (activeVarFilter) {
        activeVarFilter = null;
        document.querySelectorAll('.var-hover').forEach(function(s) { s.classList.remove('var-highlight-persist'); });
    }
    if (activeComponentFilter) {
        clearComponentFilter();
    }
    if (typeof activeConstraintVarFilter !== 'undefined' && activeConstraintVarFilter != null) {
        clearConstraintVarFilter();
    }
    applyFilters();
    updateFilterPill();
}

// --- URL hash state for filters ---

function updateHashState() {
    if (!currentInstanceName) return;
    var hash = '#instance=' + encodeURIComponent(currentInstanceName);
    if (activeTypeFilter) hash += '&type=' + encodeURIComponent(activeTypeFilter);
    if (activeVarFilter) hash += '&var=' + encodeURIComponent(activeVarFilter);
    history.replaceState(null, '', hash);
}

function parseHashParams() {
    var hash = location.hash.slice(1);
    var params = {};
    hash.split('&').forEach(function(part) {
        var eq = part.indexOf('=');
        if (eq > 0) {
            params[part.slice(0, eq)] = decodeURIComponent(part.slice(eq + 1));
        }
    });
    return params;
}

// --- Recents (localStorage) ---
// Each entry: { name, source: 'instance' | 'upload', fileName? }
// Uploaded file content stored separately as 'mipviz_file:<fileName>'
const RECENTS_KEY = 'mipviz_recents';
const MAX_RECENTS = 12;
const MAX_UPLOAD_CACHE_SIZE = 2 * 1024 * 1024; // 2MB

function getRecents() {
    try {
        var raw = JSON.parse(localStorage.getItem(RECENTS_KEY)) || [];
        // Migrate old string entries
        return raw.map(r => typeof r === 'string' ? { name: r, source: 'instance' } : r);
    } catch { return []; }
}

function addRecent(name, source, fileName) {
    source = source || 'instance';
    var recents = getRecents().filter(r => r.name !== name);
    var entry = { name: name, source: source };
    if (fileName) entry.fileName = fileName;
    recents.unshift(entry);
    if (recents.length > MAX_RECENTS) {
        // Clean up cached files for evicted entries
        var evicted = recents.slice(MAX_RECENTS);
        evicted.forEach(function (r) {
            if (r.source === 'upload' && r.fileName) {
                localStorage.removeItem('mipviz_file:' + r.fileName);
            }
        });
        recents.length = MAX_RECENTS;
    }
    localStorage.setItem(RECENTS_KEY, JSON.stringify(recents));
    renderRecents();
}

function cacheUploadedFile(file, text) {
    if (text.length > MAX_UPLOAD_CACHE_SIZE) return;
    try {
        localStorage.setItem('mipviz_file:' + file.name, text);
    } catch (e) {
        // localStorage full — silently skip
    }
}

function removeRecent(name) {
    var recents = getRecents();
    var removed = recents.find(r => r.name === name);
    recents = recents.filter(r => r.name !== name);
    if (removed && removed.source === 'upload' && removed.fileName) {
        localStorage.removeItem('mipviz_file:' + removed.fileName);
    }
    localStorage.setItem(RECENTS_KEY, JSON.stringify(recents));
    renderRecents();
}

async function loadRecentUpload(entry) {
    var text = localStorage.getItem('mipviz_file:' + entry.fileName);
    if (!text) {
        removeRecent(entry.name);
        return;
    }
    setStatus('Loading ' + entry.name + '…', 'loading');
    uploadSection.classList.add('hidden');
    heroSection.classList.add('hidden');
    try {
        var file = new File([text], entry.fileName, { type: 'text/plain' });
        modelData = await API.parseModel(file);
        currentUploadFile = file;
        currentInstanceName = null;
        originalModelData = null;
        isPresolved = false;
        presolveBtn.textContent = 'Presolve';
        presolveBtn.classList.remove('active');
        downloadBtn.classList.add('hidden');
        addRecent(entry.name, 'upload', entry.fileName);
        showResults();
    } catch (err) {
        setStatus('Error: ' + err.message, 'error');
        uploadSection.classList.remove('hidden');
        heroSection.classList.remove('hidden');
    }
}

function renderRecents() {
    var recents = getRecents();
    var container = document.getElementById('recents');
    var list = document.getElementById('recents-list');
    if (!container || !list) return;
    if (recents.length === 0) { container.classList.add('hidden'); return; }
    container.classList.remove('hidden');
    list.innerHTML = recents.map(function (r) {
        var icon = r.source === 'upload' ? '\u2191 ' : ''; // ↑ for uploads
        if (r.source === 'upload') {
            return '<a class="recent-item recent-upload" href="#" data-name="' + r.name.replace(/"/g, '&quot;') + '">' +
                '<span class="recent-name">' + icon + r.name + '</span>' +
                '<span class="recent-remove" data-name="' + r.name.replace(/"/g, '&quot;') + '" title="Remove">&times;</span>' +
            '</a>';
        }
        return '<a class="recent-item" href="#instance=' + encodeURIComponent(r.name) + '">' +
            '<span class="recent-name">' + r.name + '</span>' +
            '<span class="recent-remove" data-name="' + r.name.replace(/"/g, '&quot;') + '" title="Remove">&times;</span>' +
        '</a>';
    }).join('');
    list.querySelectorAll('.recent-remove').forEach(btn => {
        btn.addEventListener('click', function (e) {
            e.preventDefault();
            e.stopPropagation();
            removeRecent(this.getAttribute('data-name'));
        });
    });
    list.querySelectorAll('.recent-upload').forEach(a => {
        a.addEventListener('click', function (e) {
            e.preventDefault();
            var name = this.getAttribute('data-name');
            var entry = getRecents().find(r => r.name === name);
            if (entry) loadRecentUpload(entry);
        });
    });
}

renderRecents();

// --- Solve MIP with modal log ---
let solveWorker = null;
let mipSolution = null;

function doSolveMip() {
    var solver = currentSolver;
    if (!currentUploadFile) { showToast('No model file loaded'); return; }

    const modal = document.getElementById('solve-modal');
    const log = document.getElementById('solve-log');
    const title = document.getElementById('solve-modal-title');
    const footer = document.getElementById('solve-modal-footer');
    const statusEl = document.getElementById('solve-modal-status');
    const applyBtn = document.getElementById('solve-modal-apply');

    log.textContent = '';
    title.textContent = 'Solving ' + (modelData ? modelData.name : 'MIP') + '...';
    footer.classList.add('hidden');
    document.getElementById('solve-modal-tabs').classList.add('hidden');
    document.getElementById('solve-stats-body').classList.add('hidden');
    log.classList.remove('hidden');
    // Reset tab state
    document.querySelectorAll('.solve-tab').forEach(function(t) { t.classList.toggle('active', t.dataset.tab === 'log'); });
    modal.classList.remove('hidden');
    mipSolution = null;

    // Terminate previous worker if any
    if (solveWorker) solveWorker.terminate();

    solveWorker = new Worker('./solve-worker.js');

    solveWorker.onmessage = function(e) {
        const msg = e.data;
        if (msg.type === 'ready') {
            // Worker loaded, send the file
            currentUploadFile.arrayBuffer().then(function(buffer) {
                // Decompress if needed
                if (currentUploadFile.name.endsWith('.gz')) {
                    const ds = new DecompressionStream('gzip');
                    new Response(new Blob([buffer]).stream().pipeThrough(ds)).arrayBuffer().then(function(decompressed) {
                        solveWorker.postMessage({ type: 'solve', fileBytes: decompressed, fileName: currentUploadFile.name, solver: solver });
                    });
                } else {
                    solveWorker.postMessage({ type: 'solve', fileBytes: buffer, fileName: currentUploadFile.name, solver: solver });
                }
            });
        } else if (msg.type === 'log') {
            log.textContent += msg.line + '\n';
            log.scrollTop = log.scrollHeight;
        } else if (msg.type === 'done') {
            mipSolution = msg.result;
            title.textContent = 'Solve Complete';
            footer.classList.remove('hidden');
            var obj = mipSolution.objective_value != null ? formatNum(mipSolution.objective_value) : '?';
            statusEl.textContent = mipSolution.status + ' — obj: ' + obj;
            // Show tabs if SCIP statistics available
            var tabs = document.getElementById('solve-modal-tabs');
            var statsBody = document.getElementById('solve-stats-body');
            if (mipSolution.stats) {
                statsBody.textContent = mipSolution.stats;
                tabs.classList.remove('hidden');
            } else {
                tabs.classList.add('hidden');
            }
            solveWorker.terminate();
            solveWorker = null;
        } else if (msg.type === 'error') {
            log.textContent += '\nError: ' + msg.message + '\n';
            title.textContent = 'Solve Failed';
        }
    };
}

solveMipBtn.addEventListener('click', () => doSolveMip());

document.querySelectorAll('.solve-tab').forEach(function(btn) {
    btn.addEventListener('click', function() {
        document.querySelectorAll('.solve-tab').forEach(function(t) { t.classList.remove('active'); });
        btn.classList.add('active');
        var showLog = btn.dataset.tab === 'log';
        document.getElementById('solve-log').classList.toggle('hidden', !showLog);
        document.getElementById('solve-stats-body').classList.toggle('hidden', showLog);
    });
});

document.getElementById('solve-modal-close').addEventListener('click', function() {
    document.getElementById('solve-modal').classList.add('hidden');
    if (solveWorker) { solveWorker.terminate(); solveWorker = null; }
});

document.getElementById('solve-modal-apply').addEventListener('click', function() {
    if (!mipSolution || !mipSolution.col_values) return;
    // Apply as LP solution to show values on variables
    lpSolution = mipSolution;
    document.getElementById('solve-modal').classList.add('hidden');
    solveLpBtn.textContent = 'Hide MIP (obj: ' + formatNum(mipSolution.objective_value) + ')';
    solveLpBtn.classList.add('active');
    renderVariablesInit();
});

// --- Solve visible constraints ---
document.getElementById('solve-visible-btn').addEventListener('click', async function() {
    if (!currentUploadFile || !modelData) return;
    var btn = this;

    // Get visible constraint indices (respecting type/var/component filters)
    var allIndices = filteredConIndices
        ? filteredConIndices.slice()
        : Array.from({ length: modelData.constraints.length }, function(_, i) { return i; });

    // Apply type and var filters
    var visible = allIndices.filter(function(i) {
        var c = modelData.constraints[i];
        var tags = c._tags || [];
        if (activeTypeFilter && !tags.includes(activeTypeFilter)) return false;
        if (activeVarFilter && !c.terms.some(function(t) { return String(t.var_index) === activeVarFilter; })) return false;
        return true;
    });

    if (visible.length === 0) { showToast('No visible constraints'); return; }

    btn.textContent = 'Solving (' + visible.length + ')…';
    btn.disabled = true;
    try {
        var result = await API.solveConstraintSubset(currentUploadFile, visible, true);
        lpSolution = result;
        solveLpBtn.textContent = 'Hide LP (obj: ' + formatNum(result.objective_value) + ', ' + visible.length + ' cons)';
        solveLpBtn.classList.add('active');
        renderVariablesInit();
    } catch (err) {
        showToast('Solve error: ' + err.message);
    } finally {
        btn.textContent = 'Solve visible LP';
        btn.disabled = false;
    }
});

solveLpBtn.addEventListener('click', async () => {
    if (!currentUploadFile) {
        alert('No model file loaded');
        return;
    }
    if (lpSolution) {
        // Toggle off
        lpSolution = null;
        solveLpBtn.textContent = 'Solve LP';
        solveLpBtn.classList.remove('active');
        renderVariablesInit();
        return;
    }
    solveLpBtn.textContent = 'Solving…';
    solveLpBtn.disabled = true;
    try {
        lpSolution = await API.solveRootLp(currentUploadFile, isPresolved);
        solveLpBtn.textContent = 'Hide LP (obj: ' + formatNum(lpSolution.objective_value) + ')';
        solveLpBtn.classList.add('active');
        renderVariablesInit();
    } catch (err) {
        showToast('LP solve error: ' + err.message);
        solveLpBtn.textContent = 'Solve LP';
    } finally {
        solveLpBtn.disabled = false;
    }
});

async function doPresolve() {
    var solver = currentSolver;

    if (isPresolved) {
        modelData = originalModelData;
        originalModelData = null;
        isPresolved = false;
        presolveBtn.textContent = 'Presolve';
        presolveBtn.classList.remove('active');
        showResults();
        // Re-trigger with new solver
    }

    presolveBtn.textContent = 'Presolving…';
    presolveBtn.disabled = true;

    try {
        if (!currentUploadFile) {
            throw new Error('No model loaded');
        }
        let result = await API.presolveModel(currentUploadFile, solver);

        originalModelData = modelData;
        modelData = result;
        isPresolved = true;
        presolveBtn.textContent = 'Original';
        presolveBtn.classList.add('active');
        showResults();
        fetchCliquesImplications(solver);
        if (solver === 'scip') {
            fetchSymmetry();
        } else {
            symmetryPanel.classList.add('hidden');
        }
    } catch (err) {
        showToast('Presolve error: ' + err.message);
        presolveBtn.textContent = 'Presolve';
    } finally {
        presolveBtn.disabled = false;
    }
}

presolveBtn.addEventListener('click', () => {
    if (isPresolved) {
        modelData = originalModelData;
        originalModelData = null;
        isPresolved = false;
        presolveBtn.textContent = 'Presolve';
        presolveBtn.classList.remove('active');
        symmetryPanel.classList.add('hidden');
        showResults();
        return;
    }
    doPresolve();
});

// --- Solver picker ---
document.querySelectorAll('.solver-option').forEach(btn => {
    btn.addEventListener('click', () => {
        currentSolver = btn.dataset.solver;
        document.querySelectorAll('.solver-option').forEach(b => b.classList.remove('active'));
        btn.classList.add('active');
    });
});

// --- Cliques & Implications ---

async function fetchCliquesImplications(solver) {
    cliquesPanel.classList.remove('hidden');
    cliquesPanel.innerHTML = '<div class="cliques-header"><h3>Cliques &amp; Implications</h3></div>' +
        '<p class="cliques-loading">Loading from ' + escapeHtml(solver.toUpperCase()) + '…</p>';

    try {
        if (!currentUploadFile) throw new Error('No model loaded');
        var data = await API.getCliquesImplications(currentUploadFile, solver);
        renderCliquesImplications(data);
    } catch (err) {
        cliquesPanel.innerHTML = '<div class="cliques-header"><h3>Cliques &amp; Implications</h3>' +
            '<button class="cliques-close" onclick="cliquesPanel.classList.add(\'hidden\')">&times;</button></div>' +
            '<p class="cliques-error">Error: ' + escapeHtml(err.message) + '</p>';
    }
}

function cliqueVarTooltip(m) {
    if (!modelData || !modelData.variables || m.var_index == null) return escapeAttr(m.var_name);
    var v = modelData.variables[m.var_index];
    if (!v) return escapeAttr(m.var_name);
    var lo = v.lower === null || v.lower < -INF_THRESHOLD ? '-∞' : formatNum(v.lower);
    var hi = v.upper === null || v.upper > INF_THRESHOLD ? '∞' : formatNum(v.upper);
    return escapeAttr('x' + m.var_index + ' (' + v.name + ') ∈ [' + lo + ', ' + hi + '] · ' + v.type + ' · obj: ' + formatNum(v.obj));
}

function cliqueVarClass(m) {
    if (!modelData || !modelData.variables || m.var_index == null) return 'clique-var';
    var v = modelData.variables[m.var_index];
    if (!v) return 'clique-var';
    return 'var-' + v.type;
}

function implVarTooltip(varIndex, varName) {
    if (!modelData || !modelData.variables || varIndex == null) return escapeAttr(varName);
    var v = modelData.variables[varIndex];
    if (!v) return escapeAttr(varName);
    var lo = v.lower === null || v.lower < -INF_THRESHOLD ? '-∞' : formatNum(v.lower);
    var hi = v.upper === null || v.upper > INF_THRESHOLD ? '∞' : formatNum(v.upper);
    return escapeAttr('x' + varIndex + ' (' + v.name + ') ∈ [' + lo + ', ' + hi + '] · ' + v.type + ' · obj: ' + formatNum(v.obj));
}

function implVarClass(varIndex) {
    if (!modelData || !modelData.variables || varIndex == null) return 'impl-var';
    var v = modelData.variables[varIndex];
    if (!v) return 'impl-var';
    return 'var-' + v.type;
}

function renderCliquesImplications(data) {
    var html = '<div class="cliques-header">' +
        '<h3>Cliques &amp; Implications <span class="cliques-solver-tag">' + escapeHtml(data.solver.toUpperCase()) + '</span></h3>' +
        '<button class="cliques-close" onclick="cliquesPanel.classList.add(\'hidden\')">&times;</button>' +
        '</div>';

    html += '<div class="cliques-summary">';
    html += '<div class="cliques-stat"><span class="cliques-stat-value">' + data.num_cliques + '</span><span class="cliques-stat-label">cliques</span></div>';
    html += '<div class="cliques-stat"><span class="cliques-stat-value">' + data.num_implications + '</span><span class="cliques-stat-label">implications</span></div>';
    if (data.cliques.length > 0) {
        var sizes = data.cliques.map(function(c) { return c.members.length; });
        var maxSize = Math.max.apply(null, sizes);
        var avgSize = (sizes.reduce(function(a, b) { return a + b; }, 0) / sizes.length).toFixed(1);
        var eqCount = data.cliques.filter(function(c) { return c.is_equation; }).length;
        html += '<div class="cliques-stat"><span class="cliques-stat-value">' + maxSize + '</span><span class="cliques-stat-label">max clique size</span></div>';
        html += '<div class="cliques-stat"><span class="cliques-stat-value">' + avgSize + '</span><span class="cliques-stat-label">avg clique size</span></div>';
        if (eqCount > 0) {
            html += '<div class="cliques-stat"><span class="cliques-stat-value">' + eqCount + '</span><span class="cliques-stat-label">equation cliques</span></div>';
        }
    }
    html += '</div>';

    // Clique size distribution
    if (data.cliques.length > 0) {
        var sizeDist = {};
        data.cliques.forEach(function(c) {
            var s = c.members.length;
            sizeDist[s] = (sizeDist[s] || 0) + 1;
        });
        var sizeKeys = Object.keys(sizeDist).map(Number).sort(function(a, b) { return a - b; });
        html += '<div class="cliques-distribution">';
        html += '<span class="cliques-dist-label">Size distribution:</span>';
        sizeKeys.forEach(function(s) {
            html += '<span class="cliques-dist-item"><span class="cliques-dist-size">' + s + '</span><span class="cliques-dist-count">' + sizeDist[s] + '</span></span>';
        });
        html += '</div>';
    }

    // Cliques list (collapsible)
    if (data.cliques.length > 0) {
        html += '<details class="cliques-detail-section">';
        html += '<summary><h4>Cliques (' + data.cliques.length + ')</h4></summary>';
        html += '<div class="cliques-list">';
        for (var i = 0; i < data.cliques.length; i++) {
            var c = data.cliques[i];
            var tag = c.is_equation ? '<span class="clique-eq-tag">= 1</span>' : '<span class="clique-leq-tag">&le; 1</span>';
            html += '<div class="clique-item">';
            html += '<span class="clique-id">#' + c.id + '</span>';
            html += '<span class="clique-members">';
            c.members.forEach(function(m, j) {
                if (j > 0) html += ' <span class="op">+</span> ';
                var varTip = cliqueVarTooltip(m);
                var varClass = cliqueVarClass(m);
                var negated = !m.value;
                html += '<span class="' + varClass + ' var-hover' + (negated ? ' var-negated' : '') + '" data-var="' + m.var_index + '" data-tip="' + varTip + '">x' + m.var_index + '</span>';
            });
            html += '</span>' + tag + '</div>';
        }
        html += '</div></details>';
    }

    // Implications list (collapsible)
    if (data.implications.length > 0) {
        html += '<details class="cliques-detail-section">';
        html += '<summary><h4>Implications (' + data.implications.length + ')</h4></summary>';
        html += '<div class="implications-list">';
        for (var i = 0; i < data.implications.length; i++) {
            var imp = data.implications[i];
            var fromVal = imp.from_value ? '1' : '0';
            var arrow = imp.bound_type === 'lower' ? '&ge;' : '&le;';
            var fromTip = implVarTooltip(imp.from_var_index, imp.from_var_name);
            var toTip = implVarTooltip(imp.to_var_index, imp.to_var_name);
            var fromClass = implVarClass(imp.from_var_index);
            var toClass = implVarClass(imp.to_var_index);
            html += '<div class="implication-item">';
            html += '<span class="' + fromClass + ' var-hover" data-var="' + imp.from_var_index + '" data-tip="' + fromTip + '">x' + imp.from_var_index + '</span>';
            html += ' <span class="impl-from-val">= ' + fromVal + '</span>';
            html += ' <span class="impl-arrow">&rarr;</span> ';
            html += '<span class="' + toClass + ' var-hover" data-var="' + imp.to_var_index + '" data-tip="' + toTip + '">x' + imp.to_var_index + '</span>';
            html += ' <span class="impl-to-bound">' + arrow + ' ' + formatImplVal(imp.bound_value) + '</span>';
            html += '</div>';
        }
        html += '</div></details>';
    }

    if (data.num_cliques === 0 && data.num_implications === 0) {
        html += '<p class="cliques-empty">No cliques or implications found during presolve.</p>';
    }

    cliquesPanel.innerHTML = html;
}

// --- Symmetry ---

async function fetchSymmetry() {
    symmetryPanel.classList.remove('hidden');
    symmetryPanel.innerHTML = '<div class="cliques-header"><h3>Symmetry</h3></div>' +
        '<p class="cliques-loading">Detecting symmetry with SCIP…</p>';

    try {
        if (!currentUploadFile) throw new Error('No model loaded');
        var data = await API.getSymmetry(currentUploadFile);
        renderSymmetry(data);
    } catch (err) {
        symmetryPanel.innerHTML = '<div class="cliques-header"><h3>Symmetry</h3>' +
            '<button class="cliques-close" onclick="symmetryPanel.classList.add(\'hidden\')">&times;</button></div>' +
            '<p class="cliques-error">Error: ' + escapeHtml(err.message) + '</p>';
    }
}

function renderSymmetry(data) {
    var html = '<div class="cliques-header">' +
        '<h3>Symmetry <span class="cliques-solver-tag">SCIP</span></h3>' +
        '<button class="cliques-close" onclick="symmetryPanel.classList.add(\'hidden\')">&times;</button>' +
        '</div>';

    if (data.num_generators === 0) {
        html += '<p class="cliques-empty">No symmetry detected.</p>';
        symmetryPanel.innerHTML = html;
        return;
    }

    html += '<div class="cliques-summary">';
    html += '<div class="cliques-stat"><span class="cliques-stat-value">' + data.num_generators + '</span><span class="cliques-stat-label">generators</span></div>';
    html += '<div class="cliques-stat"><span class="cliques-stat-value">' + data.num_permvars + '</span><span class="cliques-stat-label">affected vars</span></div>';
    html += '<div class="cliques-stat"><span class="cliques-stat-value">' + data.num_components + '</span><span class="cliques-stat-label">components</span></div>';

    var groupSize = data.log10_group_size;
    if (groupSize > 0) {
        var displaySize;
        if (groupSize <= 6) {
            displaySize = Math.round(Math.pow(10, groupSize)).toLocaleString();
        } else {
            displaySize = '10<sup>' + groupSize.toFixed(1) + '</sup>';
        }
        html += '<div class="cliques-stat"><span class="cliques-stat-value">' + displaySize + '</span><span class="cliques-stat-label">group size</span></div>';
    }

    if (data.bin_var_affected) {
        html += '<div class="cliques-stat"><span class="cliques-stat-value">yes</span><span class="cliques-stat-label">binary vars affected</span></div>';
    }
    html += '</div>';

    // Component details
    if (data.components.length > 0) {
        html += '<details class="cliques-detail-section">';
        html += '<summary><h4>Components (' + data.components.length + ')</h4></summary>';
        html += '<div class="cliques-list">';
        for (var i = 0; i < data.components.length; i++) {
            var comp = data.components[i];
            html += '<div class="clique-item">';
            html += '<span class="clique-id">#' + comp.id + '</span>';
            html += '<span class="cliques-stat-label" style="margin-right:8px">' + comp.var_indices.length + ' vars</span>';
            html += '<span class="clique-members">';
            var maxShow = 20;
            for (var j = 0; j < Math.min(comp.var_names.length, maxShow); j++) {
                if (j > 0) html += ', ';
                html += '<span class="var-binary var-hover" data-tip="' + escapeAttr(comp.var_names[j]) + '">' + escapeHtml(comp.var_names[j]) + '</span>';
            }
            if (comp.var_names.length > maxShow) {
                html += ' <span class="cliques-stat-label">… +' + (comp.var_names.length - maxShow) + ' more</span>';
            }
            html += '</span></div>';
        }
        html += '</div></details>';
    }

    symmetryPanel.innerHTML = html;
}

function formatImplVal(v) {
    if (Number.isInteger(v)) return v.toString();
    return v.toPrecision(6);
}

if (mathToggle) mathToggle.addEventListener('click', () => {
    if (!modelData) return;
    mathMode = !mathMode;
    mathToggle.textContent = mathMode ? 'Code' : 'LaTeX';
    renderObjective();
    renderVariablesInit();
    renderConstraintsInit();
});

// Drag and drop
dropZone.addEventListener('dragover', (e) => {
    e.preventDefault();
    dropZone.classList.add('drag-over');
});

dropZone.addEventListener('dragleave', () => {
    dropZone.classList.remove('drag-over');
});

dropZone.addEventListener('drop', (e) => {
    e.preventDefault();
    dropZone.classList.remove('drag-over');
    const file = e.dataTransfer.files[0];
    if (file) uploadFile(file);
});

dropZone.addEventListener('click', (e) => {
    // Avoid double-triggering when clicking the file input/label directly
    if (e.target === fileInput || e.target.closest('.file-label')) return;
    fileInput.click();
});

fileInput.addEventListener('change', () => {
    const file = fileInput.files[0];
    if (file) uploadFile(file);
});

async function loadInstanceFromUrl(name) {
    const url = MIPVIZ_INSTANCES_LFS + 'instances/' + encodeURIComponent(name) + '.mps.gz';
    showLoadingSkeleton();
    modelNameEl.textContent = name;

    try {
        const response = await fetch(url);
        if (!response.ok) throw new Error('Instance not found: ' + name);
        const blob = await response.blob();
        const file = new File([blob], name + '.mps.gz', { type: 'application/gzip' });
        currentUploadFile = file;
        currentInstanceName = name;
        modelData = await API.parseModel(file);
        originalModelData = null;
        isPresolved = false;
        presolveBtn.textContent = 'Presolve';
        presolveBtn.classList.remove('active');
        downloadBtn.classList.add('hidden');
        var matrixBtn = document.getElementById('matrix-btn');
        if (matrixBtn) {
            matrixBtn.href = './matrix.html#' + encodeURIComponent(name);
            matrixBtn.classList.remove('hidden');
        }
        var conflictBtn = document.getElementById('conflict-btn');
        if (conflictBtn) {
            conflictBtn.href = './conflict.html?solver=' + encodeURIComponent(currentSolver) + '#' + encodeURIComponent(name);
            conflictBtn.classList.remove('hidden');
        }
        var benchmarkBtn = document.getElementById('benchmark-btn');
        if (benchmarkBtn) {
            benchmarkBtn.classList.add('hidden');
            var instKey = name + '.mps.gz';
            Promise.resolve(loadBenchmarkIndex()).then(function(set) {
                if (set.has(instKey)) {
                    benchmarkBtn.href = './benchmark-instance.html?instance=' + encodeURIComponent(instKey);
                    benchmarkBtn.classList.remove('hidden');
                }
            });
        }
        document.title = name + ' — mipviz';
        history.replaceState(null, '', '#instance=' + encodeURIComponent(name));
        addRecent(name);
        await showResults();
    } catch (err) {
        setStatus('Error: ' + err.message, 'error');
        uploadSection.classList.remove('hidden');
        heroSection.classList.remove('hidden');
    }
}

if (newUploadBtn) newUploadBtn.addEventListener('click', () => {
    resultsSection.classList.add('hidden');
    uploadSection.classList.remove('hidden'); heroSection.classList.remove('hidden');
    uploadStatus.classList.add('hidden');
    fileInput.value = '';
    history.pushState(null, '', '/');
    document.title = 'mipviz';
});

showMoreBtn.addEventListener('click', renderMoreConstraints);

async function uploadFile(file) {
    setStatus('Parsing ' + file.name + '…', 'loading');

    try {
        // Read text for caching (before parseModel consumes the file)
        var buffer = await file.arrayBuffer();
        var text;
        if (file.name.endsWith('.gz')) {
            var ds = new DecompressionStream('gzip');
            var stream = new Blob([buffer]).stream().pipeThrough(ds);
            text = await new Response(stream).text();
        } else {
            text = new TextDecoder().decode(buffer);
        }
        // Reconstruct file for parsing (original may be .gz, pass decompressed as .mps)
        var parseFile = file.name.endsWith('.gz')
            ? new File([text], file.name.replace(/\.gz$/, ''), { type: 'text/plain' })
            : new File([text], file.name, { type: 'text/plain' });
        modelData = await API.parseModel(parseFile);
        currentUploadFile = parseFile;
        currentInstanceName = null;
        originalModelData = null;
        isPresolved = false;
        presolveBtn.textContent = 'Presolve';
        presolveBtn.classList.remove('active');
        downloadBtn.classList.add('hidden');
        var benchmarkBtn2 = document.getElementById('benchmark-btn');
        if (benchmarkBtn2) benchmarkBtn2.classList.add('hidden');
        // Cache the decompressed text for recents
        cacheUploadedFile(parseFile, text);
        addRecent(modelData.name, 'upload', parseFile.name);
        showResults();
    } catch (err) {
        setStatus('Error: ' + err.message, 'error');
    }
}

function setStatus(msg, type) {
    uploadStatus.textContent = msg;
    uploadStatus.className = 'upload-status ' + (type || '');
    uploadStatus.classList.remove('hidden');
}

function showLoadingSkeleton() {
    uploadSection.classList.add('hidden');
    heroSection.classList.add('hidden');
    resultsSection.classList.remove('hidden');
    modelNameEl.textContent = '';
    statsGrid.innerHTML = Array(6).fill(
        '<div class="stat-card skeleton"><span class="stat-value">&nbsp;</span><span class="stat-label">&nbsp;</span></div>'
    ).join('');
}

function showResults() {
    uploadSection.classList.add('hidden'); heroSection.classList.add('hidden');
    uploadStatus.classList.add('hidden');
    resultsSection.classList.remove('hidden');

    activeVarFilter = null;
    activeComponentFilter = null;
    lpSolution = null;
    solveLpBtn.textContent = 'Solve LP';
    solveLpBtn.classList.remove('active');
    cliquesPanel.classList.add('hidden');
    symmetryPanel.classList.add('hidden');
    if (typeof resetLagrangianPanel === 'function') resetLagrangianPanel();

    document.getElementById('loading-details').classList.add('hidden');

    // Phase 1: instant — name, stats (no iteration needed)
    modelNameEl.textContent = modelData.name;
    const existingTime = document.querySelector('.parse-time');
    if (existingTime) existingTime.remove();
    if (modelData.parse_time_ms != null) {
        const timeEl = document.createElement('span');
        timeEl.className = 'parse-time';
        const ms = modelData.parse_time_ms;
        timeEl.textContent = ms >= 1000 ? `${(ms / 1000).toFixed(1)}s` : `${ms}ms`;
        modelNameEl.after(timeEl);
    }
    renderStats(modelData.stats);

    // Phase 2: deferred — heavier work, yields to let the browser paint
    // Returns a promise that resolves when all rendering is done
    return new Promise(function(resolve) {
        requestAnimationFrame(function() {
            classifyConstraints();
            renderStats(modelData.stats); // re-render with constraint type tags
            renderRanges();
            requestAnimationFrame(function() {
                renderSparsityPlot();
                requestAnimationFrame(function() {
                    renderObjective();
                    renderVariablesInit();
                    renderConstraintsInit();
                    resolve();
                });
            });
        });
    });
}

function classifyConstraint(c) {
    const tags = [];
    const n = c.terms.length;
    if (n === 0) { return ['empty']; }

    // Variable composition
    const hasCont = c.terms.some(t => t.var_type === 'continuous');
    const hasBin = c.terms.some(t => t.var_type === 'binary');
    const hasInt = c.terms.some(t => t.var_type === 'integer');
    const allCont = c.terms.every(t => t.var_type === 'continuous');
    const allBinary = c.terms.every(t => t.var_type === 'binary');

    if (allCont) tags.push('continuous');
    else if (allBinary) tags.push('pure binary');
    else if (hasInt) tags.push('mixed integer');
    else if (hasBin && hasCont) tags.push('mixed binary');

    // Structure tags
    if (n === 1) { tags.push('bound'); return tags; }
    if (n === 2 && hasBin && (hasCont || hasInt)) { tags.push('variable bound'); return tags; }

    const lowInf = c.lower === null || c.lower < -INF_THRESHOLD;
    const upInf = c.upper === null || c.upper > INF_THRESHOLD;
    const isEq = !lowInf && !upInf && Math.abs(c.lower - c.upper) < 1e-10;
    const isLeq = lowInf && !upInf;
    const isGeq = !lowInf && upInf;

    const allCoeffOne = c.terms.every(t => Math.abs(t.coeff - 1) < 1e-10);
    const allUnitCoeff = c.terms.every(t => Math.abs(Math.abs(t.coeff) - 1) < 1e-10);
    const rhsOne = isEq ? Math.abs(c.lower - 1) < 1e-10
                 : isLeq ? Math.abs(c.upper - 1) < 1e-10
                 : isGeq ? Math.abs(c.lower - 1) < 1e-10
                 : false;

    if (allBinary && allCoeffOne && rhsOne) {
        if (isEq) tags.push('set partitioning');
        else if (isLeq) tags.push('set packing');
        else if (isGeq) tags.push('set covering');
    } else if (allBinary && allUnitCoeff) {
        tags.push('cardinality');
    } else if (allBinary && (isLeq || isGeq)) {
        tags.push('knapsack');
    } else if (allBinary && isEq) {
        tags.push('bin. knapsack eq.');
    } else if (!hasCont && hasInt && (isLeq || isGeq)) {
        tags.push('integer knapsack');
    }

    if (isEq && tags.indexOf('set partitioning') === -1 && tags.indexOf('bin. knapsack eq.') === -1) tags.push('equality');

    return tags;
}

function classifyConstraints() {
    const counts = {};
    for (const c of modelData.constraints) {
        c._tags = classifyConstraint(c);
        for (const t of c._tags) {
            counts[t] = (counts[t] || 0) + 1;
        }
    }
    modelData._constraintTypes = counts;
    activeTypeFilter = null;
}

function applyFilters() {
    constraintsList.querySelectorAll('.constraint-row').forEach(row => {
        const typeOk = !activeTypeFilter || (row.dataset.tags || '').split(',').includes(activeTypeFilter);
        const varOk = !activeVarFilter || row.querySelector('.var-hover[data-var="' + activeVarFilter + '"]');
        if (typeOk && varOk) {
            row.classList.remove('filtered-out');
        } else {
            row.classList.add('filtered-out');
        }
    });
    updateFilterPill();
}

function renderStats(stats) {
    const orig = isPresolved && originalModelData ? originalModelData.stats : null;

    const items = [
        { label: 'Variables', value: stats.num_vars, origValue: orig ? orig.num_vars : null, cls: '' },
        { label: 'Constraints', value: stats.num_constraints, origValue: orig ? orig.num_constraints : null, cls: '' },
        { label: 'Nonzeros', value: stats.num_nonzeros, origValue: orig ? orig.num_nonzeros : null, cls: '' },
        { label: 'Continuous', value: stats.num_continuous, origValue: orig ? orig.num_continuous : null, cls: 'accent-blue' },
        { label: 'Integer', value: stats.num_integer, origValue: orig ? orig.num_integer : null, cls: 'accent-green' },
        { label: 'Binary', value: stats.num_binary, origValue: orig ? orig.num_binary : null, cls: 'accent-orange' },
    ];

    statsGrid.innerHTML = items
        .map((s) => {
            let deltaHtml = '';
            if (s.origValue !== null) {
                if (s.origValue !== s.value) {
                    const diff = s.value - s.origValue;
                    const pct = s.origValue > 0 ? Math.round((diff / s.origValue) * 100) : 0;
                    const sign = diff > 0 ? '+' : '';
                    const cls = diff < 0 ? 'delta-down' : 'delta-up';
                    deltaHtml = '<span class="stat-delta ' + cls + '">' + sign + formatNumber(diff) + ' (' + sign + pct + '%)</span>';
                } else {
                    deltaHtml = '<span class="stat-delta delta-none">no change</span>';
                }
            }
            return `
        <div class="stat-card ${s.cls}">
            <span class="stat-value">${formatNumber(s.value)}</span>${deltaHtml}
            <span class="stat-label">${s.label}</span>
        </div>`;
        })
        .join('');


    // Constraint type breakdown
    const types = modelData._constraintTypes || {};
    const typeOrder = ['set partitioning', 'set packing', 'set covering', 'cardinality',
                       'knapsack', 'bin. knapsack eq.', 'integer knapsack', 'equality',
                       'variable bound', 'bound',
                       'continuous', 'pure binary', 'mixed binary', 'mixed integer',
                       'empty'];
    const typesEl = document.getElementById('constraint-types');
    const present = typeOrder.filter(t => types[t]);
    typesEl.innerHTML = present.length
        ? '<span class="type-label">Constraint types <a href="./definitions.html" class="definitions-help" target="_blank" title="Constraint type definitions">?</a></span>' +
          present.map(t =>
            '<span class="type-tag" data-type="' + t + '"><span class="type-count">' + types[t] + '</span> ' + t + '</span>'
          ).join('')
        : '';

    // Click to filter
    typesEl.querySelectorAll('.type-tag').forEach(tag => {
        tag.addEventListener('click', () => {
            const type = tag.dataset.type;
            if (activeTypeFilter === type) {
                activeTypeFilter = null;
                tag.classList.remove('active');
                showToast('Filter cleared');
            } else {
                activeTypeFilter = type;
                showToast('Filtering by ' + type);
                typesEl.querySelectorAll('.type-tag').forEach(t => t.classList.remove('active'));
                tag.classList.add('active');
            }
            applyFilters();
            // Open constraints section if collapsed
            const constraintsDetails = constraintsList.closest('details');
            if (constraintsDetails && !constraintsDetails.open) {
                constraintsDetails.open = true;
            }
        });
    });
}

// Ranges
function renderRanges() {
    const rangesEl = document.getElementById('model-ranges');
    const vars = modelData.variables;
    const cons = modelData.constraints;
    const stats = modelData.stats;

    // Sparsity / density
    const density = (stats.num_vars > 0 && stats.num_constraints > 0)
        ? stats.num_nonzeros / (stats.num_vars * stats.num_constraints)
        : 0;

    // Objective coefficients (non-zero)
    let objMin = Infinity, objMax = -Infinity;
    let objNnz = 0;
    for (const v of vars) {
        if (Math.abs(v.obj) > 1e-10) {
            objNnz++;
            if (v.obj < objMin) objMin = v.obj;
            if (v.obj > objMax) objMax = v.obj;
        }
    }

    // Constraint coefficients (non-zero)
    let consMin = Infinity, consMax = -Infinity;
    for (const c of cons) {
        for (const t of c.terms) {
            if (Math.abs(t.coeff) > 1e-10) {
                if (t.coeff < consMin) consMin = t.coeff;
                if (t.coeff > consMax) consMax = t.coeff;
            }
        }
    }

    // Variable bounds (finite only)
    let lbMin = Infinity, lbMax = -Infinity;
    let ubMin = Infinity, ubMax = -Infinity;
    let freeVars = 0;
    for (const v of vars) {
        const linf = v.lower === null || v.lower < -1e20;
        const uinf = v.upper === null || v.upper > 1e20;
        if (linf && uinf) { freeVars++; continue; }
        if (!linf) {
            if (v.lower < lbMin) lbMin = v.lower;
            if (v.lower > lbMax) lbMax = v.lower;
        }
        if (!uinf) {
            if (v.upper < ubMin) ubMin = v.upper;
            if (v.upper > ubMax) ubMax = v.upper;
        }
    }

    // RHS values (finite only)
    let rhsMin = Infinity, rhsMax = -Infinity;
    for (const c of cons) {
        const linf = c.lower === null || c.lower < -1e20;
        const uinf = c.upper === null || c.upper > 1e20;
        if (!linf) {
            if (c.lower < rhsMin) rhsMin = c.lower;
            if (c.lower > rhsMax) rhsMax = c.lower;
        }
        if (!uinf) {
            if (c.upper < rhsMin) rhsMin = c.upper;
            if (c.upper > rhsMax) rhsMax = c.upper;
        }
    }

    function fmt(v) {
        if (!isFinite(v)) return '–';
        if (Number.isInteger(v)) return formatNumber(v);
        return v.toFixed(4).replace(/0+$/, '').replace(/\.$/, '');
    }

    function rangeStr(lo, hi) {
        if (!isFinite(lo) && !isFinite(hi)) return '–';
        if (Math.abs(lo - hi) < 1e-10) return fmt(lo);
        return '[' + fmt(lo) + ', ' + fmt(hi) + ']';
    }

    var rows = [
        { label: 'Density', value: (density * 100).toFixed(2) + '%' },
        { label: 'Obj. coefficients', value: objNnz > 0 ? rangeStr(objMin, objMax) : '–' },
        { label: 'Constraint coefficients', value: isFinite(consMin) ? rangeStr(consMin, consMax) : '–' },
        { label: 'RHS values', value: isFinite(rhsMin) ? rangeStr(rhsMin, rhsMax) : '–' },
        { label: 'Lower bounds', value: isFinite(lbMin) ? rangeStr(lbMin, lbMax) : '–' },
        { label: 'Upper bounds', value: isFinite(ubMin) ? rangeStr(ubMin, ubMax) : '–' },
    ];

    rangesEl.innerHTML = '<span class="type-label">Ranges</span>' +
        rows.map(function (r) {
            return '<span class="range-item"><span class="range-label">' + r.label + '</span> <span class="range-value">' + r.value + '</span></span>';
        }).join('');
}

// Objective
function renderObjective() {
    const objEl = document.getElementById('objective');
    const terms = modelData.variables
        .map((v, i) => ({ var_index: i, var_name: v.name, var_type: v.var_type, coeff: v.obj }))
        .filter(t => Math.abs(t.coeff) > 1e-10);

    const sense = modelData.obj_sense === 'maximize' ? 'max' : 'min';
    const offset = modelData.obj_offset || 0;
    const hasOffset = Math.abs(offset) > 1e-10;

    // Update objective header with sense
    const objHeader = objEl.closest('.collapsible').querySelector('h3');
    objHeader.textContent = 'Objective (' + sense + ')';

    if (mathMode) {
        const senseLatex = sense === 'max' ? '\\max' : '\\min';
        const offsetLatex = hasOffset ? formatNum(offset) + (terms.length > 0 ? ' + ' : '') : '';
        const termsLatex = terms.length === 0 && !hasOffset ? '0' : formatTermsLatex(terms);
        renderKatex(objEl, senseLatex + ' \\quad ' + offsetLatex + termsLatex, true);
    } else {
        let html = '';
        if (hasOffset) {
            html += '<span class="coeff">' + formatNum(offset) + '</span>';
            if (terms.length > 0) html += ' <span class="op">' + (terms[0].coeff >= 0 ? '+' : '') + '</span> ';
        }
        if (terms.length === 0 && !hasOffset) {
            html += '<span class="coeff">0</span>';
        } else {
            html += formatTermsCompressed(terms);
        }
        objEl.innerHTML = html;
    }
}

function formatTermsCompressed(terms) {
    if (terms.length === 0) return '<span class="coeff">0</span>';

    // Group all terms with the same coefficient and type
    var groupMap = {};
    var groupOrder = [];
    for (var i = 0; i < terms.length; i++) {
        var t = terms[i];
        var key = '' + t.coeff;
        if (groupMap[key] === undefined) {
            groupMap[key] = groupOrder.length;
            groupOrder.push({ coeff: t.coeff, terms: [t] });
        } else {
            groupOrder[groupMap[key]].terms.push(t);
        }
    }
    var groups = groupOrder;

    var html = '';
    var termIdx = 0;
    for (var gi = 0; gi < groups.length; gi++) {
        var g = groups[gi];
        var coeff = g.coeff;
        var absCoeff = Math.abs(coeff);
        var sign = coeff < 0 ? '\u2212' : '+';

        if (termIdx === 0) {
            if (coeff < 0) html += '<span class="op">\u2212</span>';
        } else {
            html += ' <span class="op">' + sign + '</span> ';
        }

        if (g.terms.length === 1) {
            // Single term — render normally
            var t = g.terms[0];
            var varClass = 'var-' + t.var_type;
            if (Math.abs(absCoeff - 1) > 1e-10) {
                html += '<span class="coeff">' + formatNum(absCoeff) + '</span>';
            }
            var v = modelData.variables[t.var_index];
            var lo = v.lower === null || v.lower < -INF_THRESHOLD ? '-\u221E' : formatNum(v.lower);
            var hi = v.upper === null || v.upper > INF_THRESHOLD ? '\u221E' : formatNum(v.upper);
            var tooltip = escapeAttr('x' + t.var_index + ' (' + v.name + ') \u2208 [' + lo + ', ' + hi + '] \u00b7 ' + t.var_type + ' \u00b7 obj: ' + formatNum(v.obj));
            html += '<span class="' + varClass + ' var-hover" data-var="' + t.var_index + '" data-tip="' + tooltip + '">x' + t.var_index + '</span>';
        } else {
            // Group — render as compressed summation
            var count = g.terms.length;
            var groupId = 'obj-group-' + gi;

            if (Math.abs(absCoeff - 1) > 1e-10) {
                html += '<span class="coeff">' + formatNum(absCoeff) + '</span>';
            }
            // Count by type for colored breakdown
            var typeCounts = {};
            for (var ti2 = 0; ti2 < g.terms.length; ti2++) {
                var tt = g.terms[ti2].var_type;
                typeCounts[tt] = (typeCounts[tt] || 0) + 1;
            }
            var breakdownHtml = '';
            var typeKeys = ['binary', 'integer', 'continuous'];
            var first = true;
            for (var tk = 0; tk < typeKeys.length; tk++) {
                if (!typeCounts[typeKeys[tk]]) continue;
                if (!first) breakdownHtml += ' <span class="op">+</span> ';
                breakdownHtml += '<span class="var-' + typeKeys[tk] + '">' + typeCounts[typeKeys[tk]] + '</span>';
                first = false;
            }

            html += '<span class="obj-term-group" id="' + groupId + '">' +
                '<span class="obj-group-summary" title="Click to expand">' +
                    '<span class="obj-sigma">\u03A3</span>(' + breakdownHtml + ')' +
                '</span>' +
                '<span class="obj-group-expanded hidden"><span class="obj-group-collapse" title="Click to collapse">(';
            // Render individual terms inside
            for (var ti = 0; ti < g.terms.length; ti++) {
                var gt = g.terms[ti];
                var gv = modelData.variables[gt.var_index];
                var glo = gv.lower === null || gv.lower < -INF_THRESHOLD ? '-\u221E' : formatNum(gv.lower);
                var ghi = gv.upper === null || gv.upper > INF_THRESHOLD ? '\u221E' : formatNum(gv.upper);
                var gtip = escapeAttr('x' + gt.var_index + ' (' + gv.name + ') \u2208 [' + glo + ', ' + ghi + '] \u00b7 ' + gt.var_type + ' \u00b7 obj: ' + formatNum(gv.obj));
                if (ti > 0) html += ' <span class="op">+</span> ';
                html += '<span class="var-' + gt.var_type + ' var-hover" data-var="' + gt.var_index + '" data-tip="' + gtip + '">x' + gt.var_index + '</span>';
            }
            html += ')</span></span></span>';
        }
        termIdx++;
    }
    return html;
}

// Toggle obj term groups
document.addEventListener('click', function(e) {
    // Collapse button inside expanded group
    var collapse = e.target.closest('.obj-group-collapse');
    if (collapse) {
        e.stopPropagation();
        var group = collapse.closest('.obj-term-group');
        group.querySelector('.obj-group-expanded').classList.add('hidden');
        group.querySelector('.obj-group-summary').classList.remove('hidden');
        return;
    }
    // Expand via summary click
    var group = e.target.closest('.obj-term-group');
    if (!group) return;
    if (e.target.closest('.var-hover')) return;
    var summary = group.querySelector('.obj-group-summary');
    var expanded = group.querySelector('.obj-group-expanded');
    if (!summary || !expanded) return;
    if (expanded.classList.contains('hidden')) {
        expanded.classList.remove('hidden');
        summary.classList.add('hidden');
    }
});

// Variables
let varsShown = 0;
let filteredVarIndices = null; // null = show all, array = filtered indices
let maxObjCoeff = 0;
let globalVarMin = 0;
let globalVarMax = 1;
let varSortKey = 'id';
let varSortAsc = true;
let varsCompressed = true;
let compressedGroups = null;
const VARS_BATCH = 500;
const variablesList = document.getElementById('variables-list');
const showMoreVarsBtn = document.getElementById('show-more-vars-btn');

showMoreVarsBtn.addEventListener('click', renderMoreVariables);

// Sort bar click handling
(function() {
    var bar = document.getElementById('variables-sort-bar');
    if (!bar) return;
    bar.addEventListener('click', function(e) {
        var btn = e.target.closest('.sort-btn');
        if (!btn) return;
        if (btn.id === 'vars-compress-btn') {
            varsCompressed = !varsCompressed;
            btn.classList.toggle('active', varsCompressed);
            if (modelData) renderVariablesInit();
            return;
        }
        var key = btn.dataset.sort;
        if (key === varSortKey) {
            varSortAsc = !varSortAsc;
        } else {
            varSortKey = key;
            varSortAsc = true;
        }
        bar.querySelectorAll('.sort-btn').forEach(function(b) { b.classList.remove('active', 'desc'); });
        btn.classList.add('active');
        if (!varSortAsc) btn.classList.add('desc');
        if (modelData) renderVariablesInit();
    });
})();

function isFractional(i) {
    if (!lpSolution || !lpSolution.col_values || i >= lpSolution.col_values.length) return false;
    var v = modelData.variables[i];
    if (v.var_type !== 'integer' && v.var_type !== 'binary') return false;
    return Math.abs(lpSolution.col_values[i] - Math.round(lpSolution.col_values[i])) > 1e-6;
}

function renderVariablesInit() {
    variablesList.innerHTML = '';
    varsShown = 0;
    if (activeConstraintVarFilter !== null) {
        const c = modelData.constraints[activeConstraintVarFilter];
        filteredVarIndices = c.terms.map(t => t.var_index).sort((a, b) => a - b);
    } else if (activeComponentFilter) {
        filteredVarIndices = Array.from(activeComponentFilter.colSet).sort(function(a, b) { return a - b; });
    } else {
        filteredVarIndices = null;
    }

    // When LP solution is shown, sort fractional variables first
    if (lpSolution && lpSolution.col_values) {
        var indices = filteredVarIndices
            ? filteredVarIndices.slice()
            : Array.from({ length: modelData.variables.length }, function(_, i) { return i; });
        indices.sort(function(a, b) {
            var fa = isFractional(a) ? 0 : 1;
            var fb = isFractional(b) ? 0 : 1;
            return fa - fb || a - b;
        });
        filteredVarIndices = indices;
    }

    const total = filteredVarIndices ? filteredVarIndices.length : modelData.variables.length;
    var countText = total.toLocaleString() + ' total';
    if (lpSolution && lpSolution.col_values) {
        var allIndices = filteredVarIndices || Array.from({ length: modelData.variables.length }, function(_, i) { return i; });
        var nFrac = allIndices.filter(isFractional).length;
        if (nFrac > 0) countText += ' \u00b7 ' + nFrac + ' fractional';
    }
    document.getElementById('variables-count').textContent = countText;

    // Apply user-selected sort
    if (varSortKey !== 'id' || !varSortAsc) {
        var sortIndices = filteredVarIndices
            ? filteredVarIndices.slice()
            : Array.from({ length: modelData.variables.length }, function(_, i) { return i; });
        var vars = modelData.variables;
        var dir = varSortAsc ? 1 : -1;
        var INF = INF_THRESHOLD;
        var sortVal = function(idx) {
            var v = vars[idx];
            switch (varSortKey) {
                case 'id': return idx;
                case 'name': return v.name.toLowerCase();
                case 'lb': return (v.lower === null || v.lower < -INF) ? -Infinity : v.lower;
                case 'ub': return (v.upper === null || v.upper > INF) ? Infinity : v.upper;
                case 'range': {
                    var lo = (v.lower === null || v.lower < -INF) ? -Infinity : v.lower;
                    var hi = (v.upper === null || v.upper > INF) ? Infinity : v.upper;
                    if (!isFinite(lo) || !isFinite(hi)) return Infinity;
                    return hi - lo;
                }
                case 'obj': return Math.abs(v.obj);
                default: return idx;
            }
        };
        sortIndices.sort(function(a, b) {
            var va = sortVal(a), vb = sortVal(b);
            if (va < vb) return -dir;
            if (va > vb) return dir;
            return a - b;
        });
        filteredVarIndices = sortIndices;
    }

    // Compute max |obj| and global bounds for shared number line
    maxObjCoeff = 0;
    globalVarMin = Infinity;
    globalVarMax = -Infinity;
    for (let k = 0; k < modelData.variables.length; k++) {
        const vk = modelData.variables[k];
        const a = Math.abs(vk.obj);
        if (a > maxObjCoeff) maxObjCoeff = a;
        const loOk = !(vk.lower === null || vk.lower < -INF_THRESHOLD);
        const hiOk = !(vk.upper === null || vk.upper > INF_THRESHOLD);
        if (loOk && vk.lower < globalVarMin) globalVarMin = vk.lower;
        if (hiOk && vk.upper > globalVarMax) globalVarMax = vk.upper;
    }
    if (!isFinite(globalVarMin)) globalVarMin = 0;
    if (!isFinite(globalVarMax)) globalVarMax = 1;
    if (globalVarMax - globalVarMin < 1e-10) globalVarMax = globalVarMin + 1;
    // Add 5% padding on each side so infinite-bound bars have visible arrow space
    var pad = (globalVarMax - globalVarMin) * 0.05;
    globalVarMin -= pad;
    globalVarMax += pad;

    // Build compressed groups if needed
    if (varsCompressed) {
        var indices = filteredVarIndices || Array.from({ length: modelData.variables.length }, function(_, i) { return i; });
        var groupMap = {};
        compressedGroups = [];
        for (var gi = 0; gi < indices.length; gi++) {
            var idx = indices[gi];
            var vg = modelData.variables[idx];
            var loRaw = (vg.lower === null || vg.lower < -INF_THRESHOLD) ? '-inf' : vg.lower;
            var hiRaw = (vg.upper === null || vg.upper > INF_THRESHOLD) ? 'inf' : vg.upper;
            var key = vg.var_type + '|' + loRaw + '|' + hiRaw + '|' + vg.obj;
            if (groupMap[key] === undefined) {
                groupMap[key] = compressedGroups.length;
                compressedGroups.push({ indices: [idx], type: vg.var_type, lower: vg.lower, upper: vg.upper, obj: vg.obj });
            } else {
                compressedGroups[groupMap[key]].indices.push(idx);
            }
        }
    } else {
        compressedGroups = null;
    }

    // Render number line axis header
    renderVarAxis();

    renderMoreVariables();
}

function renderVarAxis() {
    var existing = document.getElementById('var-axis');
    if (existing) existing.remove();
    var axis = document.createElement('div');
    axis.id = 'var-axis';
    axis.className = 'variable-row var-axis-row';

    // Generate ~5 nice tick marks with minimum spacing
    var range = globalVarMax - globalVarMin;
    var ticks = [];
    var step = Math.pow(10, Math.floor(Math.log10(range)));
    if (range / step < 3) step /= 2;
    if (range / step > 8) step *= 2;
    var first = Math.ceil(globalVarMin / step) * step;
    for (var t = first; t <= globalVarMax + step * 0.01; t += step) {
        var pct = (t - globalVarMin) / range * 100;
        if (pct >= 2 && pct <= 98) {
            // Skip if too close to previous tick (< 12% apart)
            if (ticks.length === 0 || pct - ticks[ticks.length - 1].pct >= 12) {
                ticks.push({ val: t, pct: pct });
            }
        }
    }

    var ticksHtml = '';
    for (var ti = 0; ti < ticks.length; ti++) {
        ticksHtml += '<div class="var-axis-tick" style="left:' + ticks[ti].pct.toFixed(2) + '%">' +
            '<div class="var-axis-tick-line"></div>' +
            '<span class="var-axis-tick-label">' + formatNum(ticks[ti].val) + '</span>' +
        '</div>';
    }

    // Pad left to match variable rows (name + original name columns)
    axis.innerHTML =
        '<span class="variable-label-area"></span>' +
        '<div class="variable-bar-track var-axis-track">' +
            '<div class="var-axis-line"></div>' +
            ticksHtml +
        '</div>' +
        '<span class="variable-obj"></span>';

    variablesList.parentNode.insertBefore(axis, variablesList);
}

function buildVarBarHtml(v, varClass, barH, loFinite, hiFinite, lo, hi, loLabel, hiLabel, markerHtml) {
    var range = globalVarMax - globalVarMin;
    var typeColor = v.var_type === 'continuous' ? 'var(--orange)' : v.var_type === 'integer' ? 'var(--green)' : 'var(--accent)';

    // Position on the shared number line
    var leftPct = loFinite ? ((lo - globalVarMin) / range * 100) : 0;
    var rightPct = hiFinite ? ((globalVarMax - hi) / range * 100) : 0;
    var barWidthPct = 100 - leftPct - rightPct;
    if (barWidthPct < 0.5) barWidthPct = 0.5; // minimum visible width

    // Arrows for infinite bounds
    var arrowH = barH + 4;
    var arrowW = Math.round(arrowH * 0.8);
    var leftCap = loFinite ? '' :
        '<svg class="variable-bar-arrow" style="left:-' + arrowW + 'px" width="' + arrowW + '" height="' + arrowH + '">' +
        '<polygon points="' + arrowW + ',0 ' + arrowW + ',' + arrowH + ' 0,' + (arrowH / 2) + '" fill="' + typeColor + '"/></svg>';
    var rightCap = hiFinite ? '' :
        '<svg class="variable-bar-arrow" style="right:-' + arrowW + 'px" width="' + arrowW + '" height="' + arrowH + '">' +
        '<polygon points="0,0 0,' + arrowH + ' ' + arrowW + ',' + (arrowH / 2) + '" fill="' + typeColor + '"/></svg>';

    var tip = '[' + loLabel + ', ' + hiLabel + ']';

    return '<div class="variable-bar-track">' +
            '<div class="variable-bar ' + varClass + ' var-bar-tip" data-tip="' + escapeAttr(tip) + '" style="height:' + barH + 'px;left:' + leftPct.toFixed(2) + '%;width:' + barWidthPct.toFixed(2) + '%">' +
                leftCap +
                markerHtml +
                rightCap +
            '</div>' +
        '</div>';
}

function varBarParams(v) {
    var loFinite = !(v.lower === null || v.lower < -INF_THRESHOLD);
    var hiFinite = !(v.upper === null || v.upper > INF_THRESHOLD);
    var lo = loFinite ? v.lower : null;
    var hi = hiFinite ? v.upper : null;
    var loLabel = loFinite ? formatNum(lo) : '-∞';
    var hiLabel = hiFinite ? formatNum(hi) : '∞';
    var barH = maxObjCoeff > 1e-10 && Math.abs(v.obj) > 1e-10
        ? Math.max(2, Math.round(2 + 6 * Math.abs(v.obj) / maxObjCoeff))
        : 2;
    return { loFinite: loFinite, hiFinite: hiFinite, lo: lo, hi: hi, loLabel: loLabel, hiLabel: hiLabel, barH: barH };
}

function renderMoreVariables() {
    if (varsCompressed && compressedGroups) {
        renderMoreVariablesCompressed();
        return;
    }
    const vars = modelData.variables;
    const indices = filteredVarIndices;
    const totalCount = indices ? indices.length : vars.length;
    const end = Math.min(varsShown + VARS_BATCH, totalCount);
    const fragment = document.createDocumentFragment();

    for (let j = varsShown; j < end; j++) {
        const i = indices ? indices[j] : j;
        const v = vars[i];
        const row = document.createElement('div');
        row.className = 'variable-row';

        if (mathMode) {
            const lo = v.lower === null || v.lower < -INF_THRESHOLD ? '-\\infty' : formatNum(v.lower);
            const hi = v.upper === null || v.upper > INF_THRESHOLD ? '\\infty' : formatNum(v.upper);
            const vname = 'x_{' + i + '}';
            const typeSet = v.var_type === 'binary' ? '\\{0,1\\}' : v.var_type === 'integer' ? '\\mathbb{Z}' : '\\mathbb{R}';
            const latex = vname + ' \\in [' + lo + ',\\, ' + hi + '] \\subset ' + typeSet;
            renderKatex(row, latex, false);
            const origSpan = document.createElement('span');
            origSpan.className = 'variable-original-name';
            origSpan.textContent = v.name;
            row.appendChild(origSpan);
        } else {
            const varClass = 'var-' + v.var_type;
            const bp = varBarParams(v);

            // LP marker
            let markerHtml = '';
            let lpValHtml = '';
            if (lpSolution && lpSolution.col_values && i < lpSolution.col_values.length) {
                const val = lpSolution.col_values[i];
                const isInt = v.var_type === 'integer' || v.var_type === 'binary';
                const frac = isInt && Math.abs(val - Math.round(val)) > 1e-6;
                lpValHtml = '<span class="variable-lp-val' + (frac ? ' fractional' : '') + '">' + formatNum(val) + '</span>';
                if (bp.loFinite && bp.hiFinite && bp.hi - bp.lo > 1e-10) {
                    const pct = Math.max(0, Math.min(100, (val - bp.lo) / (bp.hi - bp.lo) * 100));
                    markerHtml = '<div class="variable-bar-marker' + (frac ? ' fractional' : '') + '" style="left:' + pct.toFixed(1) + '%"></div>';
                }
            }

            var varTip = escapeAttr('x' + i + ' (' + v.name + ') ∈ [' + bp.loLabel + ', ' + bp.hiLabel + '] · ' + v.var_type + ' · obj: ' + formatNum(v.obj));

            row.innerHTML =
                '<span class="variable-label-area">' +
                    '<span class="variable-name var-link var-hover ' + varClass + '" data-var="' + i + '" data-tip="' + varTip + '">x' + i + '</span>' +
                    '<span class="variable-original-name">' + escapeHtml(v.name) + '</span>' +
                '</span>' +
                buildVarBarHtml(v, varClass, bp.barH, bp.loFinite, bp.hiFinite, bp.lo, bp.hi, bp.loLabel, bp.hiLabel, markerHtml) +
                lpValHtml +
                (Math.abs(v.obj) > 1e-10 ? '<span class="variable-obj">' + formatNum(v.obj) + '</span>' : '');
        }

        fragment.appendChild(row);
    }
    variablesList.appendChild(fragment);
    varsShown = end;

    if (varsShown >= totalCount) {
        showMoreVarsBtn.classList.add('hidden');
    } else {
        showMoreVarsBtn.classList.remove('hidden');
        const remaining = totalCount - varsShown;
        showMoreVarsBtn.textContent = 'Show ' + Math.min(VARS_BATCH, remaining) + ' more of ' + remaining + ' remaining';
    }
}

function renderMoreVariablesCompressed() {
    const groups = compressedGroups;
    const vars = modelData.variables;
    const totalCount = groups.length;
    const end = Math.min(varsShown + VARS_BATCH, totalCount);
    const fragment = document.createDocumentFragment();

    for (let j = varsShown; j < end; j++) {
        const g = groups[j];
        const v = { var_type: g.type, lower: g.lower, upper: g.upper, obj: g.obj };
        const varClass = 'var-' + g.type;
        const bp = varBarParams(v);
        const ids = g.indices;

        const wrapper = document.createElement('div');
        wrapper.className = 'variable-group';

        // Header row
        const row = document.createElement('div');
        row.className = 'variable-row variable-group-header' + (ids.length > 1 ? ' expandable' : '');

        let label;
        if (ids.length === 1) {
            label = '<span class="variable-label-area">' +
                '<span class="variable-name var-link ' + varClass + '" data-var="' + ids[0] + '">x' + ids[0] + '</span>' +
                '<span class="variable-original-name">' + escapeHtml(vars[ids[0]].name) + '</span>' +
                '</span>';
        } else {
            label = '<span class="variable-label-area">' +
                '<span class="variable-group-toggle"></span>' +
                '<span class="variable-name ' + varClass + '">' + ids.length + ' vars</span>' +
                '</span>';
        }

        row.innerHTML = label +
            buildVarBarHtml(v, varClass, bp.barH, bp.loFinite, bp.hiFinite, bp.lo, bp.hi, bp.loLabel, bp.hiLabel, '') +
            (Math.abs(g.obj) > 1e-10 ? '<span class="variable-obj">' + formatNum(g.obj) + '</span>' : '');

        wrapper.appendChild(row);

        // Expandable children container (hidden by default)
        if (ids.length > 1) {
            const children = document.createElement('div');
            children.className = 'variable-group-children hidden';
            wrapper.appendChild(children);

            row.addEventListener('click', (function(childrenEl, idsArr) {
                return function(e) {
                    if (e.target.closest('.var-link')) return; // don't toggle when clicking a var link
                    var isOpen = !childrenEl.classList.contains('hidden');
                    if (isOpen) {
                        childrenEl.classList.add('hidden');
                        this.classList.remove('expanded');
                    } else {
                        // Populate on first open
                        if (childrenEl.children.length === 0) {
                            var frag = document.createDocumentFragment();
                            for (var ci = 0; ci < idsArr.length; ci++) {
                                var idx = idsArr[ci];
                                var cv = vars[idx];
                                var cClass = 'var-' + cv.var_type;
                                var cbp = varBarParams(cv);
                                var cMarker = '';
                                var cLpVal = '';
                                if (lpSolution && lpSolution.col_values && idx < lpSolution.col_values.length) {
                                    var val = lpSolution.col_values[idx];
                                    var isInt = cv.var_type === 'integer' || cv.var_type === 'binary';
                                    var frac = isInt && Math.abs(val - Math.round(val)) > 1e-6;
                                    cLpVal = '<span class="variable-lp-val' + (frac ? ' fractional' : '') + '">' + formatNum(val) + '</span>';
                                    if (cbp.loFinite && cbp.hiFinite && cbp.hi - cbp.lo > 1e-10) {
                                        var pct = Math.max(0, Math.min(100, (val - cbp.lo) / (cbp.hi - cbp.lo) * 100));
                                        cMarker = '<div class="variable-bar-marker' + (frac ? ' fractional' : '') + '" style="left:' + pct.toFixed(1) + '%"></div>';
                                    }
                                }
                                var cRow = document.createElement('div');
                                cRow.className = 'variable-row variable-group-child';
                                var cTip = escapeAttr('x' + idx + ' (' + cv.name + ') ∈ [' + cbp.loLabel + ', ' + cbp.hiLabel + '] · ' + cv.var_type + ' · obj: ' + formatNum(cv.obj));
                                cRow.innerHTML =
                                    '<span class="variable-label-area">' +
                                        '<span class="variable-name var-link var-hover ' + cClass + '" data-var="' + idx + '" data-tip="' + cTip + '">x' + idx + '</span>' +
                                        '<span class="variable-original-name">' + escapeHtml(cv.name) + '</span>' +
                                    '</span>' +
                                    buildVarBarHtml(cv, cClass, cbp.barH, cbp.loFinite, cbp.hiFinite, cbp.lo, cbp.hi, cbp.loLabel, cbp.hiLabel, cMarker) +
                                    cLpVal +
                                    (Math.abs(cv.obj) > 1e-10 ? '<span class="variable-obj">' + formatNum(cv.obj) + '</span>' : '');
                                frag.appendChild(cRow);
                            }
                            childrenEl.appendChild(frag);
                        }
                        childrenEl.classList.remove('hidden');
                        this.classList.add('expanded');
                    }
                };
            })(children, ids));
        }

        fragment.appendChild(wrapper);
    }
    variablesList.appendChild(fragment);
    varsShown = end;

    if (varsShown >= totalCount) {
        showMoreVarsBtn.classList.add('hidden');
    } else {
        showMoreVarsBtn.classList.remove('hidden');
        const remaining = totalCount - varsShown;
        showMoreVarsBtn.textContent = 'Show ' + Math.min(VARS_BATCH, remaining) + ' more of ' + remaining + ' remaining';
    }
}

// Constraints
let filteredConIndices = null; // null = show all, array = filtered indices

function renderConstraintsInit() {
    constraintsList.innerHTML = '';
    constraintsShown = 0;
    if (activeConstraintVarFilter !== null) {
        const c = modelData.constraints[activeConstraintVarFilter];
        const varSet = new Set(c.terms.map(t => t.var_index));
        const conIndices = [];
        modelData.constraints.forEach((con, i) => {
            if (con.terms.some(t => varSet.has(t.var_index))) conIndices.push(i);
        });
        filteredConIndices = conIndices;
    } else {
        filteredConIndices = activeComponentFilter
            ? Array.from(activeComponentFilter.rowSet).sort(function(a, b) { return a - b; })
            : null;
    }
    const total = filteredConIndices ? filteredConIndices.length : modelData.constraints.length;
    constraintsCount.textContent = total.toLocaleString() + ' total';

    renderMoreConstraints();
}

function renderMoreConstraints() {
    const constraints = modelData.constraints;
    const indices = filteredConIndices;
    const totalCount = indices ? indices.length : constraints.length;
    const end = Math.min(constraintsShown + BATCH_SIZE, totalCount);

    const fragment = document.createDocumentFragment();
    for (let j = constraintsShown; j < end; j++) {
        const i = indices ? indices[j] : j;
        fragment.appendChild(createConstraintRow(constraints[i], i));
    }
    constraintsList.appendChild(fragment);
    constraintsShown = end;

    if (constraintsShown >= totalCount) {
        showMoreBtn.classList.add('hidden');
    } else {
        showMoreBtn.classList.remove('hidden');
        const remaining = totalCount - constraintsShown;
        showMoreBtn.textContent = 'Show ' + Math.min(BATCH_SIZE, remaining) + ' more of ' + remaining + ' remaining';
    }
}

function createConstraintRow(constraint, idx) {
    const row = document.createElement('div');
    row.className = 'constraint-row';
    const tags = constraint._tags || [];
    row.dataset.tags = tags.join(',');
    const typeOk = !activeTypeFilter || tags.includes(activeTypeFilter);
    const varOk = !activeVarFilter || constraint.terms.some(t => String(t.var_index) === activeVarFilter);
    if (!typeOk || !varOk) {
        row.classList.add('filtered-out');
    }
    row.style.animationDelay = Math.min(idx - (constraintsShown), 30) * 3 + 'ms';

    const name = document.createElement('span');
    name.className = 'constraint-name constraint-link';
    name.textContent = constraint.name;
    name.dataset.constraintIdx = idx;
    name.addEventListener('click', function () {
        setConstraintVarFilter(idx);
    });

    const tagsContainer = document.createElement('span');
    tagsContainer.className = 'constraint-tags';
    for (const t of tags) {
        const tag = document.createElement('span');
        tag.className = 'constraint-type-tag';
        tag.dataset.type = t;
        tag.textContent = t;
        tagsContainer.appendChild(tag);
    }

    const expr = document.createElement('span');
    expr.className = 'constraint-expr';

    if (mathMode) {
        renderKatex(expr, formatConstraintLatex(constraint), false);
    } else {
        expr.innerHTML = formatConstraint(constraint);
    }

    // Single-constraint solve button
    const solveBtn = document.createElement('button');
    solveBtn.className = 'constraint-solve-btn';
    solveBtn.title = 'Solve relaxation with only this constraint';
    solveBtn.textContent = '▶';
    solveBtn.dataset.constraintIdx = idx;
    solveBtn.addEventListener('click', async function(e) {
        e.stopPropagation();
        if (!currentUploadFile) return;
        solveBtn.textContent = '…';
        solveBtn.disabled = true;
        try {
            const result = await API.solveConstraintSubset(currentUploadFile, [idx], true);
            const mipResult = await API.solveConstraintSubset(currentUploadFile, [idx], false);
            solveBtn.textContent = '▶';

            // Show results as a table under the constraint
            let tableEl = row.querySelector('.constraint-solve-table');
            if (tableEl) tableEl.remove();
            tableEl = document.createElement('div');
            tableEl.className = 'constraint-solve-table';

            var lpStatus = result.status || '';
            var mipStatus = mipResult.status || '';
            var lpOk = lpStatus === 'Optimal';
            var mipOk = mipStatus === 'Optimal';

            var header = '<tr><th>Variable</th><th>Type</th>' +
                (lpOk ? '<th>LP value</th>' : '') +
                (mipOk ? '<th>MIP value</th>' : '') + '</tr>';

            // Sort: nonzero values first, then by variable name
            var termsSorted = constraint.terms.slice().sort(function(a, b) {
                var aLp = lpOk ? Math.abs(result.col_values[a.var_index]) : 0;
                var bLp = lpOk ? Math.abs(result.col_values[b.var_index]) : 0;
                var aMip = mipOk ? Math.abs(mipResult.col_values[a.var_index]) : 0;
                var bMip = mipOk ? Math.abs(mipResult.col_values[b.var_index]) : 0;
                var aNz = (aLp > 1e-10 || aMip > 1e-10) ? 0 : 1;
                var bNz = (bLp > 1e-10 || bMip > 1e-10) ? 0 : 1;
                return aNz - bNz;
            });

            var rows = termsSorted.map(function(t) {
                var lpVal = lpOk ? result.col_values[t.var_index] : 0;
                var mipVal = mipOk ? mipResult.col_values[t.var_index] : 0;
                var isZero = Math.abs(lpVal) < 1e-10 && Math.abs(mipVal) < 1e-10;
                if (isZero) return '';
                var isInt = t.var_type === 'integer' || t.var_type === 'binary';
                var lpFrac = isInt && Math.abs(lpVal - Math.round(lpVal)) > 1e-6;
                return '<tr>' +
                    '<td class="var-' + t.var_type + '">' + escapeHtml(t.var_name) + '</td>' +
                    '<td>' + t.var_type + '</td>' +
                    (lpOk ? '<td class="' + (lpFrac ? 'fractional' : '') + '">' + formatNum(lpVal) + '</td>' : '') +
                    (mipOk ? '<td>' + formatNum(mipVal) + '</td>' : '') +
                    '</tr>';
            }).join('');

            var nZero = termsSorted.filter(function(t) {
                var lpVal = lpOk ? Math.abs(result.col_values[t.var_index]) : 0;
                var mipVal = mipOk ? Math.abs(mipResult.col_values[t.var_index]) : 0;
                return lpVal < 1e-10 && mipVal < 1e-10;
            }).length;
            if (nZero > 0) {
                rows += '<tr class="zero-summary"><td colspan="4">' + nZero + ' variables at zero</td></tr>';
            }

            tableEl.innerHTML = '<div class="constraint-solve-summary">' +
                'LP: <strong>' + (lpOk ? formatNum(result.objective_value) : lpStatus) + '</strong>' +
                ' &nbsp; MIP: <strong>' + (mipOk ? formatNum(mipResult.objective_value) : mipStatus) + '</strong></div>' +
                (lpOk || mipOk ? '<table>' + header + rows + '</table>' : '');
            row.appendChild(tableEl);
        } catch (err) {
            solveBtn.textContent = '!';
            solveBtn.title = 'Error: ' + err.message;
        } finally {
            solveBtn.disabled = false;
        }
    });

    // Filter-by-constraint button
    const filterBtn = document.createElement('button');
    filterBtn.className = 'constraint-filter-btn';
    filterBtn.title = 'Show only variables in this constraint';
    filterBtn.innerHTML = '<svg width="10" height="10" viewBox="0 0 16 16" fill="currentColor"><path d="M1 2h14l-5 6v5l-4 2V8z"/></svg>';
    if (activeConstraintVarFilter === idx) filterBtn.classList.add('active');
    filterBtn.addEventListener('click', function(e) {
        e.stopPropagation();
        setConstraintVarFilter(idx);
    });

    row.appendChild(name);
    row.appendChild(solveBtn);
    row.appendChild(filterBtn);
    row.appendChild(tagsContainer);
    row.appendChild(expr);
    return row;
}

function formatConstraint(c) {
    const terms = c.terms;
    const lhsHtml = formatTerms(terms);

    const lowInf = c.lower === null || c.lower < -INF_THRESHOLD;
    const upInf = c.upper === null || c.upper > INF_THRESHOLD;

    if (lowInf && upInf) {
        // Free row
        return lhsHtml + ' <span class="relation">free</span>';
    }
    if (!lowInf && !upInf && Math.abs(c.lower - c.upper) < 1e-10) {
        // Equality
        return lhsHtml + ' <span class="relation">=</span> <span class="bound-val">' + formatNum(c.lower) + '</span>';
    }
    if (lowInf) {
        // <= upper
        return lhsHtml + ' <span class="relation">&le;</span> <span class="bound-val">' + formatNum(c.upper) + '</span>';
    }
    if (upInf) {
        // >= lower
        return lhsHtml + ' <span class="relation">&ge;</span> <span class="bound-val">' + formatNum(c.lower) + '</span>';
    }
    // Ranged
    return '<span class="bound-val">' + formatNum(c.lower) + '</span> <span class="relation">&le;</span> ' +
           lhsHtml + ' <span class="relation">&le;</span> <span class="bound-val">' + formatNum(c.upper) + '</span>';
}

function formatTerms(terms) {
    if (terms.length === 0) return '<span class="coeff">0</span>';

    let html = '';
    for (let i = 0; i < terms.length; i++) {
        const t = terms[i];
        const coeff = t.coeff;
        const absCoeff = Math.abs(coeff);
        const sign = coeff < 0 ? '−' : '+';
        const varClass = 'var-' + t.var_type;

        if (i === 0) {
            // First term: show minus sign attached, no plus
            if (coeff < 0) {
                html += '<span class="op">−</span>';
            }
        } else {
            html += ' <span class="op">' + sign + '</span> ';
        }

        if (Math.abs(absCoeff - 1) > 1e-10) {
            html += '<span class="coeff">' + formatNum(absCoeff) + '</span>';
        }

        const v = modelData.variables[t.var_index];
        const lo = v.lower === null || v.lower < -INF_THRESHOLD ? '-∞' : formatNum(v.lower);
        const hi = v.upper === null || v.upper > INF_THRESHOLD ? '∞' : formatNum(v.upper);
        const tooltip = escapeAttr('x' + t.var_index + ' (' + v.name + ') ∈ [' + lo + ', ' + hi + '] · ' + t.var_type + ' · obj: ' + formatNum(v.obj));
        html += '<span class="' + varClass + ' var-hover" data-var="' + t.var_index + '" data-tip="' + tooltip + '">x' + t.var_index + '</span>';
    }
    return html;
}

function formatNum(n) {
    if (Number.isInteger(n) || Math.abs(n - Math.round(n)) < 1e-10) {
        return Math.round(n).toString();
    }
    // Up to 6 significant digits, trim trailing zeros
    return parseFloat(n.toPrecision(6)).toString();
}

function formatNumber(n) {
    return n.toLocaleString();
}

// LaTeX rendering
function renderKatex(el, latex, displayMode) {
    try {
        katex.render(latex, el, { displayMode: displayMode, throwOnError: false });
    } catch (e) {
        el.textContent = latex;
    }
}

function latexVarName(name) {
    // Try to split name into letters + digits for subscript: x1 -> x_{1}
    const m = name.match(/^([a-zA-Z]+)(\d+)$/);
    if (m) return m[1] + '_{' + m[2] + '}';
    // Escape underscores for LaTeX
    return '\\text{' + name.replace(/_/g, '\\_') + '}';
}

function formatTermsLatex(terms) {
    if (terms.length === 0) return '0';
    let s = '';
    for (let i = 0; i < terms.length; i++) {
        const t = terms[i];
        const coeff = t.coeff;
        const absCoeff = Math.abs(coeff);
        const vname = 'x_{' + t.var_index + '}';

        if (i === 0) {
            if (coeff < 0) s += '-';
        } else {
            s += coeff < 0 ? ' - ' : ' + ';
        }

        if (Math.abs(absCoeff - 1) > 1e-10) {
            s += formatNum(absCoeff);
        }
        s += vname;
    }
    return s;
}

function formatConstraintLatex(c) {
    const lhs = formatTermsLatex(c.terms);
    const lowInf = c.lower === null || c.lower < -INF_THRESHOLD;
    const upInf = c.upper === null || c.upper > INF_THRESHOLD;

    if (lowInf && upInf) return lhs + ' \\quad \\text{free}';
    if (!lowInf && !upInf && Math.abs(c.lower - c.upper) < 1e-10)
        return lhs + ' = ' + formatNum(c.lower);
    if (lowInf) return lhs + ' \\leq ' + formatNum(c.upper);
    if (upInf) return lhs + ' \\geq ' + formatNum(c.lower);
    return formatNum(c.lower) + ' \\leq ' + lhs + ' \\leq ' + formatNum(c.upper);
}

function escapeHtml(s) {
    const div = document.createElement('div');
    div.textContent = s;
    return div.innerHTML;
}

function escapeAttr(s) {
    return s.replace(/&/g, '&amp;').replace(/"/g, '&quot;').replace(/</g, '&lt;').replace(/>/g, '&gt;');
}

// Variable highlighting
document.addEventListener('mouseover', (e) => {
    const el = e.target.closest('.var-hover[data-var]');
    if (!el) return;
    const varIdx = el.dataset.var;
    if (activeVarFilter === varIdx) return;
    document.querySelectorAll('.var-hover[data-var="' + varIdx + '"]').forEach(s => {
        s.classList.add('var-highlight-hover');
    });
});

document.addEventListener('mouseout', (e) => {
    const el = e.target.closest('.var-hover[data-var]');
    if (!el) return;
    const varIdx = el.dataset.var;
    document.querySelectorAll('.var-hover[data-var="' + varIdx + '"]').forEach(s => {
        s.classList.remove('var-highlight-hover');
    });
});

document.addEventListener('click', (e) => {
    const el = e.target.closest('.var-hover[data-var]') || e.target.closest('.var-link[data-var]');
    if (!el) return;
    const varIdx = el.dataset.var;
    if (activeVarFilter === varIdx) {
        // Clear filter
        activeVarFilter = null;
        document.querySelectorAll('.var-hover').forEach(s => s.classList.remove('var-highlight-persist'));
        showToast('Filter cleared');
    } else {
        // Set filter
        activeVarFilter = varIdx;
        showToast('Filtering by x' + varIdx);
        document.querySelectorAll('.var-hover').forEach(s => {
            s.classList.remove('var-highlight-hover');
            if (s.dataset.var === varIdx) {
                s.classList.add('var-highlight-persist');
            } else {
                s.classList.remove('var-highlight-persist');
            }
        });
    }
    applyFilters();
    // Open constraints section if collapsed
    const constraintsDetails = constraintsList.closest('details');
    if (constraintsDetails && !constraintsDetails.open) {
        constraintsDetails.open = true;
    }
});

// Union-Find for connected components
function findComponents(constraints, numRows, numCols) {
    const n = numRows + numCols;
    const parent = new Int32Array(n);
    const rank = new Uint8Array(n);
    for (let i = 0; i < n; i++) parent[i] = i;

    function find(x) {
        while (parent[x] !== x) { parent[x] = parent[parent[x]]; x = parent[x]; }
        return x;
    }
    function unite(a, b) {
        a = find(a); b = find(b);
        if (a === b) return;
        if (rank[a] < rank[b]) { const t = a; a = b; b = t; }
        parent[b] = a;
        if (rank[a] === rank[b]) rank[a]++;
    }

    for (let r = 0; r < numRows; r++) {
        const terms = constraints[r].terms;
        for (let t = 0; t < terms.length; t++) {
            unite(r, numRows + terms[t].var_index);
        }
    }

    // Group rows and cols by component
    const compMap = new Map();
    for (let r = 0; r < numRows; r++) {
        const root = find(r);
        if (!compMap.has(root)) compMap.set(root, { rows: [], cols: [] });
        compMap.get(root).rows.push(r);
    }
    for (let c = 0; c < numCols; c++) {
        const root = find(numRows + c);
        if (!compMap.has(root)) compMap.set(root, { rows: [], cols: [] });
        compMap.get(root).cols.push(c);
    }

    // Sort components by size (largest first)
    const components = Array.from(compMap.values())
        .filter(comp => comp.rows.length > 0 || comp.cols.length > 0)
        .sort((a, b) => (b.rows.length + b.cols.length) - (a.rows.length + a.cols.length));

    // Build permutations: row order and col order
    const rowOrder = [];
    const colOrder = [];
    for (const comp of components) {
        for (const r of comp.rows) rowOrder.push(r);
        for (const c of comp.cols) colOrder.push(c);
    }

    // Inverse map: new col index for each original col
    const colNewIndex = new Int32Array(numCols);
    for (let i = 0; i < colOrder.length; i++) colNewIndex[colOrder[i]] = i;

    return { components, rowOrder, colOrder, colNewIndex };
}

// Build rich stats for each component
function buildComponentStats(components, constraints, variables) {
    return components.map(function(comp, idx) {
        let nz = 0;
        const conTags = {};
        for (const r of comp.rows) {
            nz += constraints[r].terms.length;
            const tags = constraints[r]._tags || [];
            for (const t of tags) conTags[t] = (conTags[t] || 0) + 1;
        }
        let nCont = 0, nInt = 0, nBin = 0;
        for (const c of comp.cols) {
            const vt = variables[c].var_type;
            if (vt === 'continuous') nCont++;
            else if (vt === 'integer') nInt++;
            else if (vt === 'binary') nBin++;
        }
        return {
            index: idx + 1,
            rows: comp.rows.length,
            cols: comp.cols.length,
            nz: nz,
            nCont: nCont,
            nInt: nInt,
            nBin: nBin,
            conTags: conTags
        };
    });
}

// Shared: cumulative row boundaries for component hover lookup
// Returns array of { cumRows, cumCols } (end boundaries, exclusive)
function buildCompBoundaries(components) {
    const bounds = [];
    let cumR = 0, cumC = 0;
    for (const comp of components) {
        cumR += comp.rows.length;
        cumC += comp.cols.length;
        bounds.push({ cumRows: cumR, cumCols: cumC });
    }
    return bounds;
}

function compIndexFromPixel(py, scaleR, boundaries) {
    const row = py * scaleR;
    for (let i = 0; i < boundaries.length; i++) {
        if (row < boundaries[i].cumRows) return i;
    }
    return boundaries.length - 1;
}

// Shared: render sparsity to a canvas, return metadata for hover
function renderSparsityToCanvas(canvas, maxPx) {
    const constraints = modelData.constraints;
    const variables = modelData.variables;
    const numRows = constraints.length;
    const numCols = variables.length;
    if (numRows === 0 || numCols === 0) { canvas.width = 0; canvas.height = 0; return null; }

    const { components, rowOrder, colOrder, colNewIndex } = findComponents(constraints, numRows, numCols);
    const numComponents = components.length;

    const scaleR = Math.max(1, Math.ceil(numRows / maxPx));
    const scaleC = Math.max(1, Math.ceil(numCols / maxPx));
    const canvasH = Math.ceil(numRows / scaleR);
    const canvasW = Math.ceil(numCols / scaleC);

    canvas.width = canvasW;
    canvas.height = canvasH;

    const density = new Uint16Array(canvasH * canvasW);
    for (let newR = 0; newR < numRows; newR++) {
        const origR = rowOrder[newR];
        const terms = constraints[origR].terms;
        const gr = Math.floor(newR / scaleR);
        for (let t = 0; t < terms.length; t++) {
            const newC = colNewIndex[terms[t].var_index];
            const gc = Math.floor(newC / scaleC);
            const idx = gr * canvasW + gc;
            if (density[idx] < 65535) density[idx]++;
        }
    }

    let maxDensity = 0;
    for (let i = 0; i < density.length; i++) {
        if (density[i] > maxDensity) maxDensity = density[i];
    }

    const ctx = canvas.getContext('2d');
    const imgData = ctx.createImageData(canvasW, canvasH);
    const data = imgData.data;
    for (let i = 0; i < density.length; i++) {
        const d = density[i];
        if (d > 0) {
            const alpha = maxDensity === 1 ? 220 : Math.round(60 + 195 * (d / maxDensity));
            const off = i * 4;
            data[off] = 37; data[off + 1] = 99; data[off + 2] = 235; data[off + 3] = alpha;
        }
    }
    ctx.putImageData(imgData, 0, 0);

    if (numComponents > 1) {
        ctx.strokeStyle = 'rgba(245, 158, 11, 0.45)';
        ctx.lineWidth = 1;
        let cumRows = 0, cumCols = 0;
        for (let i = 0; i < numComponents - 1; i++) {
            cumRows += components[i].rows.length;
            cumCols += components[i].cols.length;
            const y = Math.floor(cumRows / scaleR);
            const x = Math.floor(cumCols / scaleC);
            ctx.beginPath(); ctx.moveTo(0, y + 0.5); ctx.lineTo(canvasW, y + 0.5); ctx.stroke();
            ctx.beginPath(); ctx.moveTo(x + 0.5, 0); ctx.lineTo(x + 0.5, canvasH); ctx.stroke();
        }
    }

    const compStats = buildComponentStats(components, constraints, variables);
    const boundaries = buildCompBoundaries(components);

    return { numRows, numCols, scaleR, scaleC, canvasH, canvasW, numComponents, compStats, boundaries, components };
}

// Sparsity tooltip element
const sparsityTip = document.createElement('div');
sparsityTip.className = 'sparsity-tooltip';
document.body.appendChild(sparsityTip);

function buildCompTooltipHtml(stat) {
    let html = '<div class="stt-header">Component ' + stat.index + '</div>';
    html += '<div class="stt-row"><span class="stt-label">Size</span><span>' + formatNumber(stat.rows) + ' \u00d7 ' + formatNumber(stat.cols) + '</span></div>';
    html += '<div class="stt-row"><span class="stt-label">Nonzeros</span><span>' + formatNumber(stat.nz) + '</span></div>';
    // Variable types
    const vtParts = [];
    if (stat.nCont) vtParts.push('<span class="stt-var continuous">' + formatNumber(stat.nCont) + ' cont</span>');
    if (stat.nInt) vtParts.push('<span class="stt-var integer">' + formatNumber(stat.nInt) + ' int</span>');
    if (stat.nBin) vtParts.push('<span class="stt-var binary">' + formatNumber(stat.nBin) + ' bin</span>');
    if (vtParts.length) html += '<div class="stt-row"><span class="stt-label">Variables</span><span>' + vtParts.join(' ') + '</span></div>';
    // Constraint tags
    const tagEntries = Object.entries(stat.conTags).sort(function(a, b) { return b[1] - a[1]; });
    if (tagEntries.length) {
        html += '<div class="stt-tags">';
        for (const [tag, count] of tagEntries) {
            html += '<span class="stt-tag">' + count + ' ' + tag + '</span>';
        }
        html += '</div>';
    }
    html += '<div class="stt-hint">Click to filter</div>';
    return html;
}

function attachSparsityHover(canvas, metaGetter) {
    canvas.addEventListener('mousemove', function(e) {
        const meta = metaGetter();
        if (!meta || meta.numComponents <= 1) { sparsityTip.classList.remove('visible'); return; }
        const rect = canvas.getBoundingClientRect();
        const px = (e.clientX - rect.left) / rect.width * meta.canvasW;
        const py = (e.clientY - rect.top) / rect.height * meta.canvasH;
        if (px < 0 || py < 0 || px >= meta.canvasW || py >= meta.canvasH) { sparsityTip.classList.remove('visible'); return; }
        const ci = compIndexFromPixel(Math.floor(py), meta.scaleR, meta.boundaries);
        const stat = meta.compStats[ci];
        sparsityTip.innerHTML = buildCompTooltipHtml(stat);
        sparsityTip.classList.add('visible');
        // Position tooltip
        const tipW = sparsityTip.offsetWidth;
        const tipH = sparsityTip.offsetHeight;
        let tx = e.clientX + 12;
        let ty = e.clientY + 12;
        if (tx + tipW > window.innerWidth - 8) tx = e.clientX - tipW - 12;
        if (ty + tipH > window.innerHeight - 8) ty = e.clientY - tipH - 12;
        sparsityTip.style.left = tx + 'px';
        sparsityTip.style.top = ty + 'px';
    });
    canvas.addEventListener('mouseleave', function() {
        sparsityTip.classList.remove('visible');
    });
}

// Store latest render metadata for hover
var sparsityMeta = null;
var sparsityFullMeta = null;

// Sparsity plot (inline)
function renderSparsityPlot() {
    const canvas = document.getElementById('sparsity-canvas');
    const info = document.getElementById('sparsity-info');
    const compInfo = document.getElementById('components-info');

    const numRows = modelData.constraints.length;
    const numCols = modelData.variables.length;
    if (numRows === 0 || numCols === 0) {
        canvas.width = 0; canvas.height = 0;
        info.textContent = '';
        if (compInfo) compInfo.innerHTML = '';
        sparsityMeta = null;
        return;
    }

    sparsityMeta = renderSparsityToCanvas(canvas, 500);
    const m = sparsityMeta;

    let text = numRows + ' \u00d7 ' + numCols;
    if (m.scaleR > 1 || m.scaleC > 1) {
        text += ' (aggregated ' + m.scaleR + '\u00d7' + m.scaleC + ')';
    }
    info.textContent = text;

    // Component statistics
    if (compInfo) {
        if (m.numComponents === 1) {
            compInfo.innerHTML = '<span class="comp-summary">1 connected component (fully connected)</span>';
        } else {
            let html = '<span class="comp-summary">' + m.numComponents + ' disconnected components</span>';
            html += '<div class="comp-table-wrap"><table class="comp-table"><thead><tr><th>#</th><th>Constraints</th><th>Variables</th><th>Nonzeros</th></tr></thead><tbody>';
            for (const stat of m.compStats) {
                html += '<tr><td>' + stat.index + '</td><td>' + formatNumber(stat.rows) + '</td><td>' + formatNumber(stat.cols) + '</td><td>' + formatNumber(stat.nz) + '</td></tr>';
            }
            html += '</tbody></table></div>';
            compInfo.innerHTML = html;
        }
    }
}

// Attach hover to inline canvas (once)
attachSparsityHover(document.getElementById('sparsity-canvas'), function() { return sparsityMeta; });

// Component filter activation
function setComponentFilter(compIndex, meta) {
    if (!meta || compIndex < 0 || compIndex >= meta.components.length) return;
    const comp = meta.components[compIndex];
    const stat = meta.compStats[compIndex];
    // Toggle off if same component
    if (activeComponentFilter && activeComponentFilter.index === stat.index) {
        clearComponentFilter();
        showToast('Component filter cleared');
        return;
    }
    showToast('Filtering by component ' + (compIndex + 1));
    activeComponentFilter = {
        index: stat.index,
        rowSet: new Set(comp.rows),
        colSet: new Set(comp.cols),
        stat: stat
    };
    renderComponentBanner();
    updateFilterPill();
    renderVariablesInit();
    renderConstraintsInit();
    renderStats(modelData.stats);
    // Open both sections
    var cd = constraintsList.closest('details'); if (cd && !cd.open) cd.open = true;
    var vd = variablesList.closest('details'); if (vd && !vd.open) vd.open = true;
}

function clearComponentFilter() {
    activeComponentFilter = null;
    renderComponentBanner();
    updateFilterPill();
    renderVariablesInit();
    renderConstraintsInit();
    renderStats(modelData.stats);
}

function renderComponentBanner() {
    var banner = document.getElementById('component-filter-banner');
    if (!activeComponentFilter) {
        if (banner) banner.classList.add('hidden');
        return;
    }
    if (!banner) return;
    const s = activeComponentFilter.stat;
    banner.innerHTML = 'Showing <strong>Component ' + s.index + '</strong> &mdash; ' +
        formatNumber(s.rows) + ' constraints, ' + formatNumber(s.cols) + ' variables, ' +
        formatNumber(s.nz) + ' nonzeros ' +
        '<button class="comp-banner-clear" onclick="clearComponentFilter()">&times; Clear</button>';
    banner.classList.remove('hidden');
}

function setConstraintVarFilter(conIdx) {
    if (activeConstraintVarFilter === conIdx) {
        clearConstraintVarFilter();
        showToast('Constraint filter cleared');
        return;
    }
    activeConstraintVarFilter = conIdx;
    showToast('Filtering by constraint ' + modelData.constraints[conIdx].name);
    const c = modelData.constraints[conIdx];
    const varSet = new Set(c.terms.map(t => t.var_index));
    filteredVarIndices = Array.from(varSet).sort((a, b) => a - b);
    // Filter constraints to those sharing any of these variables
    const conIndices = [];
    modelData.constraints.forEach((con, i) => {
        if (con.terms.some(t => varSet.has(t.var_index))) conIndices.push(i);
    });
    filteredConIndices = conIndices;
    renderConstraintVarFilterBanner(c, conIdx, varSet.size, conIndices.length);
    updateFilterPill();
    renderVariablesInit();
    renderConstraintsInit();
    var vd = variablesList.closest('details'); if (vd && !vd.open) vd.open = true;
}

function clearConstraintVarFilter() {
    activeConstraintVarFilter = null;
    var banner = document.getElementById('constraint-var-filter-banner');
    if (banner) banner.classList.add('hidden');
    filteredVarIndices = null;
    filteredConIndices = null;
    updateFilterPill();
    renderVariablesInit();
    renderConstraintsInit();
}

function renderConstraintVarFilterBanner(c, idx, nVars, nCons) {
    var banner = document.getElementById('constraint-var-filter-banner');
    if (!banner) return;
    banner.innerHTML = 'Filtering by <strong>' + escapeHtml(c.name) + '</strong> &mdash; ' +
        nVars + ' variables, ' + nCons + ' constraints ' +
        '<button class="comp-banner-clear" onclick="clearConstraintVarFilter()">&times; Clear</button>';
    banner.classList.remove('hidden');
}

function getCompIndexFromEvent(e, canvas, meta) {
    if (!meta || meta.numComponents <= 1) return -1;
    const rect = canvas.getBoundingClientRect();
    const py = (e.clientY - rect.top) / rect.height * meta.canvasH;
    if (py < 0 || py >= meta.canvasH) return -1;
    return compIndexFromPixel(Math.floor(py), meta.scaleR, meta.boundaries);
}

// Sparsity fullscreen modal with pan/zoom
(function() {
    var modal = document.createElement('div');
    modal.className = 'sparsity-modal';
    modal.innerHTML = '<div class="sparsity-modal-backdrop"></div>' +
        '<canvas id="sparsity-canvas-full"></canvas>' +
        '<button class="sparsity-modal-close">&times;</button>';
    document.body.appendChild(modal);

    var canvas = document.getElementById('sparsity-canvas-full');
    var ctx = canvas.getContext('2d');
    var offscreen = null; // OffscreenCanvas or regular canvas holding the rendered image
    var imgW = 0, imgH = 0;
    var zoom = 1, panX = 0, panY = 0;
    var MIN_ZOOM = 0.1, MAX_ZOOM = 500;
    var dragging = false, dragStartX = 0, dragStartY = 0, dragPanX = 0, dragPanY = 0;
    var dragMoved = false;
    var isOpen = false;

    function closeModal() {
        modal.classList.remove('open');
        isOpen = false;
        sparsityTip.classList.remove('visible');
    }
    modal.querySelector('.sparsity-modal-backdrop').addEventListener('click', closeModal);
    modal.querySelector('.sparsity-modal-close').addEventListener('click', closeModal);

    function resizeCanvas() {
        canvas.width = window.innerWidth;
        canvas.height = window.innerHeight;
    }

    function fitToScreen() {
        if (!imgW || !imgH) return;
        var cw = canvas.width, ch = canvas.height;
        var pad = 40;
        zoom = Math.min((cw - pad * 2) / imgW, (ch - pad * 2) / imgH, MAX_ZOOM);
        zoom = Math.max(zoom, MIN_ZOOM);
        panX = (cw - imgW * zoom) / 2;
        panY = (ch - imgH * zoom) / 2;
    }

    function fmtCoeff(v) {
        if (Number.isInteger(v)) return String(v);
        var s = v.toPrecision(4);
        // Remove trailing zeros after decimal
        if (s.indexOf('.') !== -1) s = s.replace(/\.?0+$/, '');
        return s;
    }

    function redraw() {
        if (!offscreen || !isOpen) return;
        var cw = canvas.width, ch = canvas.height;
        ctx.clearRect(0, 0, cw, ch);
        var isLight = document.documentElement.classList.contains('light');

        // Check if we're zoomed past aggregation and should render at real resolution
        // Each image pixel covers scaleR real rows. If zoom > scaleR, one real row
        // is more than 1 screen pixel, so we can resolve individual cells.
        var realCellScreenH = zoom / fullScaleR;
        var realCellScreenW = zoom / fullScaleC;
        var useDetail = sparseRows && realCellScreenH >= 3 && realCellScreenW >= 3;

        function drawMatrixBorder() {
            var numR = sparsityFullMeta.numRows;
            var numC = sparsityFullMeta.numCols;
            var x0 = panX;
            var y0 = panY;
            var x1 = panX + (numC / fullScaleC) * zoom;
            var y1 = panY + (numR / fullScaleR) * zoom;
            ctx.strokeStyle = isLight ? 'rgba(0,0,0,0.25)' : 'rgba(255,255,255,0.35)';
            ctx.lineWidth = 2;
            ctx.strokeRect(x0, y0, x1 - x0, y1 - y0);
        }

        if (!useDetail) {
            // Standard: draw scaled offscreen image
            ctx.imageSmoothingEnabled = zoom < 1;
            ctx.save();
            ctx.translate(panX, panY);
            ctx.scale(zoom, zoom);
            ctx.drawImage(offscreen, 0, 0);
            ctx.restore();
            drawMatrixBorder();
            return;
        }

        // Detail mode: render visible real cells directly
        var numRealRows = sparsityFullMeta.numRows;
        var numRealCols = sparsityFullMeta.numCols;

        // Visible area in real row/col coordinates
        // Screen (sx, sy) → real coords: realR = (sy - panY) / zoom * scaleR, realC = (sx - panX) / zoom * scaleC
        var rr0 = Math.max(0, Math.floor((-panY) / zoom * fullScaleR));
        var rr1 = Math.min(numRealRows, Math.ceil((ch - panY) / zoom * fullScaleR));
        var rc0 = Math.max(0, Math.floor((-panX) / zoom * fullScaleC));
        var rc1 = Math.min(numRealCols, Math.ceil((cw - panX) / zoom * fullScaleC));

        // Safety cap
        if ((rr1 - rr0) > 4000 || (rc1 - rc0) > 4000) {
            // Fallback to offscreen
            ctx.imageSmoothingEnabled = zoom < 1;
            ctx.save();
            ctx.translate(panX, panY);
            ctx.scale(zoom, zoom);
            ctx.drawImage(offscreen, 0, 0);
            ctx.restore();
            drawMatrixBorder();
            return;
        }

        // Coordinate helpers: real row r → screen y
        // Image pixel for real row r is r/scaleR. Screen y = panY + (r/scaleR) * zoom
        // Simplify: screen y = panY + r * zoom / scaleR = panY + r * realCellScreenH
        var cellW = realCellScreenW;
        var cellH = realCellScreenH;
        function scrX(c) { return panX + c * cellW; }
        function scrY(r) { return panY + r * cellH; }

        // Draw nonzero cells as filled rectangles
        var blueR = 37, blueG = 99, blueB = 235;
        for (var r = rr0; r < rr1; r++) {
            var entries = sparseRows[r];
            if (!entries) continue;
            var sy = scrY(r);
            for (var e = 0; e < entries.length; e++) {
                var ec = entries[e].col;
                if (ec >= rc0 && ec < rc1) {
                    ctx.fillStyle = 'rgba(' + blueR + ',' + blueG + ',' + blueB + ',0.55)';
                    ctx.fillRect(scrX(ec), sy, cellW, cellH);
                }
            }
        }

        // Component separator lines
        if (sparsityFullMeta.numComponents > 1) {
            ctx.strokeStyle = 'rgba(245, 158, 11, 0.45)';
            ctx.lineWidth = 1;
            var bounds = sparsityFullMeta.boundaries;
            for (var i = 0; i < bounds.length - 1; i++) {
                var sy = scrY(bounds[i].cumRows);
                var sx = scrX(bounds[i].cumCols);
                ctx.beginPath(); ctx.moveTo(scrX(rc0), sy); ctx.lineTo(scrX(rc1), sy); ctx.stroke();
                ctx.beginPath(); ctx.moveTo(sx, scrY(rr0)); ctx.lineTo(sx, scrY(rr1)); ctx.stroke();
            }
        }

        // Grid lines — fade in from 3px, thicken with zoom
        if (cellH >= 3 && cellW >= 3) {
            var minCell = Math.min(cellH, cellW);
            var gridAlpha = Math.min(1, (minCell - 3) / 20);
            var gridWidth = Math.max(0.25, Math.min(1, minCell / 40));
            ctx.strokeStyle = isLight
                ? 'rgba(180,180,180,' + (gridAlpha * 0.3).toFixed(3) + ')'
                : 'rgba(200,200,200,' + (gridAlpha * 0.15).toFixed(3) + ')';
            ctx.lineWidth = gridWidth;
            for (var r = rr0; r <= rr1; r++) {
                var y = scrY(r);
                ctx.beginPath(); ctx.moveTo(scrX(rc0), y); ctx.lineTo(scrX(rc1), y); ctx.stroke();
            }
            for (var c = rc0; c <= rc1; c++) {
                var x = scrX(c);
                ctx.beginPath(); ctx.moveTo(x, scrY(rr0)); ctx.lineTo(x, scrY(rr1)); ctx.stroke();
            }
        }

        // Coefficient numbers when cells are at least 20px
        if (cellH >= 20 && cellW >= 20) {
            var fontSize = Math.max(8, Math.min(cellH * 0.55, 14));
            ctx.font = fontSize + 'px JetBrains Mono, monospace';
            ctx.textAlign = 'center';
            ctx.textBaseline = 'middle';
            ctx.fillStyle = isLight ? 'rgba(0,0,0,0.85)' : 'rgba(232,234,237,0.9)';
            for (var r = rr0; r < rr1; r++) {
                var entries = sparseRows[r];
                if (!entries) continue;
                var cy = scrY(r) + cellH / 2;
                for (var e = 0; e < entries.length; e++) {
                    var ec = entries[e].col;
                    if (ec >= rc0 && ec < rc1) {
                        ctx.fillText(fmtCoeff(entries[e].coeff), scrX(ec) + cellW / 2, cy);
                    }
                }
            }
        }

        // Row/col labels when cells are at least 40px
        if (cellH >= 40 && cellW >= 40) {
            var labelFontSize = Math.max(6, Math.min(cellH * 0.35, 11));
            ctx.font = labelFontSize + 'px JetBrains Mono, monospace';
            ctx.fillStyle = isLight ? 'rgba(0,0,0,0.5)' : 'rgba(200,200,200,0.5)';
            // Row labels
            ctx.textAlign = 'right';
            ctx.textBaseline = 'middle';
            var labelX = scrX(rc0) - 4;
            for (var r = rr0; r < rr1; r++) {
                ctx.fillText(reorderedRowNames[r], labelX, scrY(r) + cellH / 2);
            }
            // Col labels (rotated)
            ctx.textAlign = 'left';
            var labelY = scrY(rr0) - 4;
            for (var c = rc0; c < rc1; c++) {
                ctx.save();
                ctx.translate(scrX(c) + cellW / 2, labelY);
                ctx.rotate(-Math.PI / 2);
                ctx.fillText(reorderedColNames[c], 0, 0);
                ctx.restore();
            }
        }

        drawMatrixBorder();
    }

    var fullScaleR = 1, fullScaleC = 1;
    // Sparse coefficient data in reordered coords: sparseRows[newRow] = [{col, coeff}, ...]
    var sparseRows = null;
    // Row/col names in reordered order
    var reorderedRowNames = null, reorderedColNames = null;

    function buildOffscreen() {
        var constraints = modelData.constraints;
        var variables = modelData.variables;
        var numRows = constraints.length;
        var numCols = variables.length;
        if (numRows === 0 || numCols === 0) { offscreen = null; sparseRows = null; return; }

        var result = findComponents(constraints, numRows, numCols);
        var components = result.components;
        var rowOrder = result.rowOrder;
        var colOrder = result.colOrder;
        var colNewIndex = result.colNewIndex;
        var numComponents = components.length;

        // Cap at 4000px per dimension to avoid blowing up memory
        var MAX_IMG = 4000;
        fullScaleR = Math.max(1, Math.ceil(numRows / MAX_IMG));
        fullScaleC = Math.max(1, Math.ceil(numCols / MAX_IMG));
        imgH = Math.ceil(numRows / fullScaleR);
        imgW = Math.ceil(numCols / fullScaleC);

        var buf = document.createElement('canvas');
        buf.width = imgW;
        buf.height = imgH;
        var bctx = buf.getContext('2d');

        // Build sparse row data in reordered coordinates (always, for zoom detail)
        sparseRows = new Array(numRows);
        reorderedRowNames = new Array(numRows);
        reorderedColNames = new Array(numCols);
        for (var r = 0; r < numRows; r++) {
            var origR = rowOrder[r];
            reorderedRowNames[r] = constraints[origR].name;
            var terms = constraints[origR].terms;
            var entries = [];
            for (var t = 0; t < terms.length; t++) {
                entries.push({ col: colNewIndex[terms[t].var_index], coeff: terms[t].coeff });
            }
            sparseRows[r] = entries;
        }
        for (var c = 0; c < numCols; c++) {
            reorderedColNames[c] = variables[colOrder[c]].name;
        }

        var density = new Uint16Array(imgH * imgW);
        for (var newR = 0; newR < numRows; newR++) {
            var origR = rowOrder[newR];
            var terms = constraints[origR].terms;
            var gr = Math.floor(newR / fullScaleR);
            for (var t = 0; t < terms.length; t++) {
                var newC = colNewIndex[terms[t].var_index];
                var gc = Math.floor(newC / fullScaleC);
                var idx = gr * imgW + gc;
                if (density[idx] < 65535) density[idx]++;
            }
        }

        var maxDensity = 0;
        for (var i = 0; i < density.length; i++) {
            if (density[i] > maxDensity) maxDensity = density[i];
        }

        var imgData = bctx.createImageData(imgW, imgH);
        var data = imgData.data;
        for (var i = 0; i < density.length; i++) {
            var d = density[i];
            if (d > 0) {
                var alpha = maxDensity === 1 ? 220 : Math.round(60 + 195 * (d / maxDensity));
                var off = i * 4;
                data[off] = 37; data[off + 1] = 99; data[off + 2] = 235; data[off + 3] = alpha;
            }
        }
        bctx.putImageData(imgData, 0, 0);

        // Separator lines
        if (numComponents > 1) {
            bctx.strokeStyle = 'rgba(245, 158, 11, 0.45)';
            bctx.lineWidth = 1;
            var cumRows = 0, cumCols = 0;
            for (var i = 0; i < numComponents - 1; i++) {
                cumRows += components[i].rows.length;
                cumCols += components[i].cols.length;
                var y = Math.floor(cumRows / fullScaleR);
                var x = Math.floor(cumCols / fullScaleC);
                bctx.beginPath(); bctx.moveTo(0, y + 0.5); bctx.lineTo(imgW, y + 0.5); bctx.stroke();
                bctx.beginPath(); bctx.moveTo(x + 0.5, 0); bctx.lineTo(x + 0.5, imgH); bctx.stroke();
            }
        }

        offscreen = buf;

        // Build meta for hover/click
        var compStats = buildComponentStats(components, constraints, variables);
        var boundaries = buildCompBoundaries(components);
        sparsityFullMeta = {
            numRows: numRows, numCols: numCols,
            scaleR: fullScaleR, scaleC: fullScaleC,
            canvasH: imgH, canvasW: imgW,
            numComponents: numComponents,
            compStats: compStats,
            boundaries: boundaries,
            components: components
        };
    }

    // Mouse → image coordinates
    function mouseToImg(e) {
        var rect = canvas.getBoundingClientRect();
        return {
            x: (e.clientX - rect.left - panX) / zoom,
            y: (e.clientY - rect.top - panY) / zoom
        };
    }

    function compIndexFromImg(iy) {
        if (!sparsityFullMeta || sparsityFullMeta.numComponents <= 1) return -1;
        return compIndexFromPixel(Math.floor(iy), fullScaleR, sparsityFullMeta.boundaries);
    }

    // Open modal
    document.getElementById('sparsity-canvas').addEventListener('click', function() {
        if (!modelData) return;
        isOpen = true;
        modal.classList.add('open');
        resizeCanvas();
        buildOffscreen();
        fitToScreen();
        redraw();
    });

    // Zoom
    canvas.addEventListener('wheel', function(e) {
        e.preventDefault();
        var factor = e.deltaY > 0 ? 0.9 : 1.1;
        var rect = canvas.getBoundingClientRect();
        var mx = e.clientX - rect.left;
        var my = e.clientY - rect.top;
        var newZoom = Math.max(MIN_ZOOM, Math.min(MAX_ZOOM, zoom * factor));
        panX = mx - (mx - panX) * (newZoom / zoom);
        panY = my - (my - panY) * (newZoom / zoom);
        zoom = newZoom;
        redraw();
    }, { passive: false });

    // Pan
    canvas.addEventListener('mousedown', function(e) {
        dragging = true;
        dragMoved = false;
        dragStartX = e.clientX;
        dragStartY = e.clientY;
        dragPanX = panX;
        dragPanY = panY;
        canvas.style.cursor = 'grabbing';
    });
    window.addEventListener('mousemove', function(e) {
        if (!dragging) return;
        var dx = e.clientX - dragStartX;
        var dy = e.clientY - dragStartY;
        if (Math.abs(dx) > 3 || Math.abs(dy) > 3) dragMoved = true;
        panX = dragPanX + dx;
        panY = dragPanY + dy;
        redraw();
    });
    window.addEventListener('mouseup', function() {
        dragging = false;
        if (isOpen) canvas.style.cursor = 'grab';
    });

    // Hover
    canvas.addEventListener('mousemove', function(e) {
        if (dragging || !isOpen) return;
        var meta = sparsityFullMeta;
        if (!meta || meta.numComponents <= 1) { sparsityTip.classList.remove('visible'); return; }
        var p = mouseToImg(e);
        if (p.x < 0 || p.y < 0 || p.x >= imgW || p.y >= imgH) { sparsityTip.classList.remove('visible'); return; }
        var ci = compIndexFromImg(p.y);
        if (ci < 0) { sparsityTip.classList.remove('visible'); return; }
        sparsityTip.innerHTML = buildCompTooltipHtml(meta.compStats[ci]);
        sparsityTip.classList.add('visible');
        var tipW = sparsityTip.offsetWidth, tipH = sparsityTip.offsetHeight;
        var tx = e.clientX + 12, ty = e.clientY + 12;
        if (tx + tipW > window.innerWidth - 8) tx = e.clientX - tipW - 12;
        if (ty + tipH > window.innerHeight - 8) ty = e.clientY - tipH - 12;
        sparsityTip.style.left = tx + 'px';
        sparsityTip.style.top = ty + 'px';
    });
    canvas.addEventListener('mouseleave', function() { sparsityTip.classList.remove('visible'); });

    // Click to filter component
    canvas.addEventListener('click', function(e) {
        if (dragMoved) return;
        var meta = sparsityFullMeta;
        if (!meta || meta.numComponents <= 1) return;
        var p = mouseToImg(e);
        if (p.x < 0 || p.y < 0 || p.x >= imgW || p.y >= imgH) return;
        var ci = compIndexFromImg(p.y);
        if (ci < 0) return;
        setComponentFilter(ci, meta);
        closeModal();
    });

    // Keyboard
    document.addEventListener('keydown', function(e) {
        if (!isOpen) return;
        if (e.key === 'Escape') closeModal();
        if (e.key === 'f' || e.key === 'F') { fitToScreen(); redraw(); }
    });

    // Resize
    window.addEventListener('resize', function() {
        if (!isOpen) return;
        resizeCanvas();
        redraw();
    });

    canvas.style.cursor = 'grab';
})();

// Tooltip
const tip = document.createElement('div');
tip.className = 'var-tooltip';
document.body.appendChild(tip);

document.addEventListener('mouseover', (e) => {
    const el = e.target.closest('.var-hover') || e.target.closest('.var-bar-tip');
    if (!el || !el.dataset.tip) return;
    tip.textContent = el.dataset.tip;
    tip.classList.add('visible');
    const rect = el.getBoundingClientRect();
    tip.style.left = rect.left + rect.width / 2 + 'px';
    tip.style.top = rect.top - 6 + 'px';
});

document.addEventListener('mouseout', (e) => {
    if (e.target.closest('.var-hover') || e.target.closest('.var-bar-tip')) {
        tip.classList.remove('visible');
    }
});

// Theme toggle handled by theme.js

// Handle back/forward navigation
window.addEventListener('popstate', (e) => {
    if (e.state && e.state.instance) {
        loadInstanceFromUrl(e.state.instance);
    } else {
        resultsSection.classList.add('hidden');
        uploadSection.classList.remove('hidden'); heroSection.classList.remove('hidden');

        uploadStatus.classList.add('hidden');
        document.title = 'mipviz';
    }
});

// MIPVIZ_INSTANCES_BASE and MIPVIZ_INSTANCES_LFS are defined in config.js

// Apply filters from URL hash params after model is loaded and rendered
function applyHashFilters() {
    var params = parseHashParams();
    if (!params.type && !params.var) return;
    if (params.type && modelData) {
        activeTypeFilter = params.type;
        var tag = document.querySelector('.type-tag[data-type="' + CSS.escape(params.type) + '"]');
        if (tag) tag.classList.add('active');
    }
    if (params.var && modelData) {
        activeVarFilter = params.var;
        document.querySelectorAll('.var-hover').forEach(function(s) {
            if (s.dataset.var === params.var) s.classList.add('var-highlight-persist');
        });
    }
    applyFilters();
    var constraintsDetails = constraintsList.closest('details');
    if (constraintsDetails && !constraintsDetails.open) constraintsDetails.open = true;
}

// Load instance from URL on page load
(function() {
    var params = parseHashParams();
    if (params.instance) {
        loadInstanceFromUrl(params.instance).then(function() {
            applyHashFilters();
        });
    }
    // Handle hash changes while on the same page (e.g. nav search)
    window.addEventListener('hashchange', function() {
        var p = parseHashParams();
        if (p.instance) {
            loadInstanceFromUrl(p.instance).then(function() {
                applyHashFilters();
            });
        }
    });
})();

// "Try a random instance"
(function() {
    var smallInstances = null;
    var btn = document.getElementById('try-random-btn');
    if (!btn) return;

    var statsUrl = MIPVIZ_INSTANCES_BASE + 'instance-stats.json';
    fetch(statsUrl).then(function(r) { return r.json(); }).then(function(stats) {
        smallInstances = stats
            .filter(function(s) { return s.num_constraints <= 5000 && s.num_vars <= 5000; })
            .map(function(s) { return s.name; });
    }).catch(function() {});

    function loadRandom() {
        if (!smallInstances || smallInstances.length === 0) return;
        var name = smallInstances[Math.floor(Math.random() * smallInstances.length)];
        loadInstanceFromUrl(name);
    }

    btn.addEventListener('click', function(e) {
        e.preventDefault();
        loadRandom();
    });

    document.addEventListener('keydown', function(e) {
        if (e.key === 'r' && !e.ctrlKey && !e.metaKey && !e.altKey) {
            var tag = document.activeElement && document.activeElement.tagName;
            if (tag === 'INPUT' || tag === 'TEXTAREA' || tag === 'SELECT') return;
            loadRandom();
        }
    });
})();

// Keyboard navigation: sections then rows
(function() {
    var mode = 'sections'; // 'sections' or 'rows'
    var sectionIdx = -1;
    var rowIdx = -1;

    function getSections() {
        var results = document.getElementById('results-section');
        if (!results || results.classList.contains('hidden')) return [];
        return Array.from(results.querySelectorAll('details.collapsible'));
    }

    function getRows(section) {
        var list = section.querySelector('.constraints-list, .variables-list');
        if (!list) return [];
        return Array.from(list.querySelectorAll('.constraint-row:not(.filtered-out), .variable-row'));
    }

    function clearAll() {
        document.querySelectorAll('.kb-section-selected').forEach(function(el) { el.classList.remove('kb-section-selected'); });
        document.querySelectorAll('.kb-selected').forEach(function(el) { el.classList.remove('kb-selected'); });
    }

    function selectSection(idx) {
        var sections = getSections();
        if (sections.length === 0) return;
        clearAll();
        sectionIdx = Math.max(0, Math.min(idx, sections.length - 1));
        rowIdx = -1;
        mode = 'sections';
        var s = sections[sectionIdx];
        s.querySelector('summary').classList.add('kb-section-selected');
        s.querySelector('summary').scrollIntoView({ block: 'nearest', behavior: 'smooth' });
    }

    function selectRow(idx) {
        var sections = getSections();
        if (sectionIdx < 0 || sectionIdx >= sections.length) return;
        var rows = getRows(sections[sectionIdx]);
        if (rows.length === 0) return;
        document.querySelectorAll('.kb-selected').forEach(function(el) { el.classList.remove('kb-selected'); });
        rowIdx = Math.max(0, Math.min(idx, rows.length - 1));
        rows[rowIdx].classList.add('kb-selected');
        rows[rowIdx].scrollIntoView({ block: 'nearest', behavior: 'smooth' });
    }

    document.addEventListener('keydown', function(e) {
        if (!modelData) return;
        var tag = document.activeElement && document.activeElement.tagName;
        if (tag === 'INPUT' || tag === 'TEXTAREA' || tag === 'SELECT') return;
        if (e.ctrlKey || e.metaKey || e.altKey) return;

        var sections = getSections();
        if (sections.length === 0) return;

        if (mode === 'sections') {
            if (e.key === 'ArrowDown' || e.key === 'j') {
                e.preventDefault();
                selectSection(sectionIdx + 1);
            } else if (e.key === 'ArrowUp' || e.key === 'k') {
                e.preventDefault();
                selectSection(sectionIdx - 1);
            } else if (e.key === 'Enter') {
                e.preventDefault();
                if (sectionIdx >= 0 && sectionIdx < sections.length) {
                    var s = sections[sectionIdx];
                    s.open = !s.open;
                    if (s.open) {
                        // Drill into rows if there are any
                        var rows = getRows(s);
                        if (rows.length > 0) {
                            mode = 'rows';
                            selectRow(0);
                        }
                    }
                }
            } else if (e.key === 'Escape') {
                clearAll();
                sectionIdx = -1;
                mode = 'sections';
            }
        } else if (mode === 'rows') {
            if (e.key === 'ArrowDown' || e.key === 'j') {
                e.preventDefault();
                selectRow(rowIdx + 1);
            } else if (e.key === 'ArrowUp' || e.key === 'k') {
                e.preventDefault();
                if (rowIdx <= 0) {
                    // Back to section level
                    document.querySelectorAll('.kb-selected').forEach(function(el) { el.classList.remove('kb-selected'); });
                    mode = 'sections';
                    rowIdx = -1;
                    selectSection(sectionIdx);
                } else {
                    selectRow(rowIdx - 1);
                }
            } else if (e.key === 'Enter') {
                e.preventDefault();
                var sections2 = getSections();
                if (sectionIdx >= 0 && sectionIdx < sections2.length) {
                    var rows = getRows(sections2[sectionIdx]);
                    if (rowIdx >= 0 && rowIdx < rows.length) {
                        var link = rows[rowIdx].querySelector('.var-link, .var-hover');
                        if (link) link.click();
                    }
                }
            } else if (e.key === 'Escape') {
                // Back to section level
                document.querySelectorAll('.kb-selected').forEach(function(el) { el.classList.remove('kb-selected'); });
                mode = 'sections';
                rowIdx = -1;
                selectSection(sectionIdx);
            }
        }
    });
})();

// Keyboard shortcut: I to open instance bank
document.addEventListener('keydown', function(e) {
    if (e.key === 'i' && !e.ctrlKey && !e.metaKey && !e.altKey) {
        var tag = document.activeElement && document.activeElement.tagName;
        if (tag === 'INPUT' || tag === 'TEXTAREA' || tag === 'SELECT') return;
        window.location.href = './instances.html';
    }
});


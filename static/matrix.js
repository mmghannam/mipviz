(function () {
    'use strict';

    // ── Extract instance name from URL ─────────────────────────────────
    var instanceName;
    if (location.hash.startsWith('#')) {
        instanceName = decodeURIComponent(location.hash.slice(1));
    }
    if (!instanceName) return;

    // ── Theme: handled by theme.js, hook to redraw on change ──────────
    window.onThemeChange = function () { if (typeof redraw === 'function') redraw(); };

    // ── Constants ──────────────────────────────────────────────────────
    var CELL_W = 72;
    var CELL_H = 28;
    var NAME_H = CELL_H;            // variable names row
    var HEADER_H = CELL_H;          // objective row
    var BOUNDS_H = CELL_H;          // upper/lower bounds rows
    var ROW_LABEL_W = 80;           // left gutter for constraint names
    var RHS_W = 80;                 // right gutter for row bounds (upper + lower)
    var FONT_SIZE = 11;
    var MIN_ZOOM = 0.002;
    var MAX_ZOOM = 4;

    // ── State ──────────────────────────────────────────────────────────
    var modelData = null;
    var reductions = [];
    var currentStep = 0;
    var grayedRows = {};            // row index -> true
    var grayedCols = {};            // col index -> true
    var activeRows = [];            // ordered list of visible row indices
    var activeCols = [];            // ordered list of visible col indices
    var flashRows = {};             // row index -> true (just affected)
    var flashCols = {};             // col index -> true (just affected)
    var flashAlpha = 0;             // animation alpha (1 -> 0)
    var flashAnimId = null;
    var pendingDir = 0;             // 0 none, 1 forward preview, -1 backward preview
    var pendingRows = {};           // row index -> true (highlighted, awaiting commit)
    var pendingCols = {};           // col index -> true (highlighted, awaiting commit)
    var playing = false;
    var playTimerId = null;
    var zoom = 1;
    var panX = 0, panY = 0;
    var dragging = false, dragStartX = 0, dragStartY = 0, dragPanX = 0, dragPanY = 0;

    // Sparse matrix: sparseMatrix[row][col] = coeff
    var sparseMatrix = {};

    var canvas = document.getElementById('matrix-canvas');
    var ctx = canvas.getContext('2d');
    var container = document.getElementById('matrix-container');

    var minimapCanvas = document.getElementById('minimap-canvas');
    var minimapCtx = minimapCanvas.getContext('2d');
    var minimapViewport = document.getElementById('minimap-viewport');

    // ── Derived layout values (set after data loads) ───────────────────
    var numRows = 0, numCols = 0;
    var totalW = 0, totalH = 0;     // total virtual size of the matrix area

    // ── Fetch data ─────────────────────────────────────────────────────
    document.title = instanceName + ' — Matrix Explorer';

    var dataPromise = API.ensureReady().then(function () {
        return fetch('https://media.githubusercontent.com/media/mmghannam/mipviz-instances/main/instances/' + encodeURIComponent(instanceName) + '.mps.gz');
    }).then(function (res) {
        if (!res.ok) throw new Error('Instance not found');
        return res.blob();
    }).then(function (blob) {
        var file = new File([blob], instanceName + '.mps.gz', { type: 'application/gzip' });
        return Promise.all([
            API.parseModel(file),
            API.getReductions(file).catch(function () { return { reductions: [] }; })
        ]);
    });

    dataPromise
        .then(function (data) {
            modelData = data[0];
            reductions = data[1].reductions || [];
            console.log('Loaded', reductions.length, 'reductions');
            init();
        })
        .catch(function (err) {
            console.error('Matrix load error:', err);
            document.getElementById('matrix-loading').innerHTML =
                '<div class="matrix-loading-text" style="color:var(--orange)">Failed to load: ' + err.message + '</div>';
        });

    // ── Initialization ─────────────────────────────────────────────────
    function init() {
        numRows = modelData.constraints.length;
        numCols = modelData.variables.length;

        // Build sparse matrix
        for (var r = 0; r < numRows; r++) {
            var terms = modelData.constraints[r].terms;
            for (var t = 0; t < terms.length; t++) {
                if (!sparseMatrix[r]) sparseMatrix[r] = {};
                sparseMatrix[r][terms[t].var_index] = terms[t].coeff;
            }
        }

        rebuildActive();
        recalcLayout();

        document.getElementById('matrix-loading').style.display = 'none';
        container.style.display = 'block';

        if (reductions.length > 0) {
            document.getElementById('stepper-bar').style.display = 'flex';
            var slider = document.getElementById('step-slider');
            slider.max = reductions.length;
            slider.value = 0;
        }

        // Defer fit until browser has laid out the container
        requestAnimationFrame(function () {
            resizeCanvas();
            fitToScreen();
            setupEvents();
            if (reductions.length > 0) {
                updateStepperUI();
            }
            redraw();
            drawMinimap();
        });
    }

    function fitToScreen() {
        var cw = container.clientWidth;
        var stepperBar = document.getElementById('stepper-bar');
        var stepperH = (stepperBar && stepperBar.offsetHeight) || 0;
        var ch = container.clientHeight - stepperH;
        var zx = cw / totalW;
        var zy = ch / totalH;
        zoom = Math.min(zx, zy, 1);
        zoom = Math.max(zoom, MIN_ZOOM);
        panX = (cw - totalW * zoom) / 2;
        panY = (ch - totalH * zoom) / 2;
    }

    function resizeCanvas() {
        var dpr = window.devicePixelRatio || 1;
        var cw = container.clientWidth;
        var ch = container.clientHeight;
        canvas.width = cw * dpr;
        canvas.height = ch * dpr;
        canvas.style.width = cw + 'px';
        canvas.style.height = ch + 'px';
        ctx.setTransform(dpr, 0, 0, dpr, 0, 0);
    }

    // ── Drawing ────────────────────────────────────────────────────────
    function redraw() {
        if (!modelData) return;
        var cw = canvas.clientWidth;
        var ch = canvas.clientHeight;
        ctx.clearRect(0, 0, cw, ch);

        ctx.save();
        ctx.translate(panX, panY);
        ctx.scale(zoom, zoom);

        var isLight = document.documentElement.classList.contains('light');

        // Viewport bounds in matrix-space
        var vx0 = -panX / zoom;
        var vy0 = -panY / zoom;
        var vx1 = vx0 + cw / zoom;
        var vy1 = vy0 + ch / zoom;

        // Layout: RHS_W (row lower) | ROW_LABEL_W | matrix cols | RHS_W (row upper)
        var LEFT_W = RHS_W + ROW_LABEL_W;
        var nActiveCols = activeCols.length;
        var nActiveRows = activeRows.length;
        var colStart = Math.max(0, Math.floor((vx0 - LEFT_W) / CELL_W));
        var colEnd = Math.min(nActiveCols, Math.ceil((vx1 - LEFT_W) / CELL_W));

        // Layout: NAME_H | BOUNDS_H | HEADER_H | matrix | BOUNDS_H
        var matrixTop = NAME_H + BOUNDS_H + HEADER_H;
        var rowStart = Math.max(0, Math.floor((vy0 - matrixTop) / CELL_H));
        var rowEnd = Math.min(nActiveRows, Math.ceil((vy1 - matrixTop) / CELL_H));

        var showNumbers = zoom >= 0.25;
        var showAbbrev = zoom >= 0.12;
        var fontSize = Math.max(8, Math.min(FONT_SIZE, FONT_SIZE * zoom * 1.2));

        var matrixRight = LEFT_W + nActiveCols * CELL_W;

        // ── Grid lines (skip when too dense) ─────────────────────────
        var cellScreenH = CELL_H * zoom;
        var cellScreenW = CELL_W * zoom;
        if (cellScreenH >= 4 && cellScreenW >= 4) {
            ctx.strokeStyle = isLight ? 'rgba(0,0,0,0.06)' : 'rgba(255,255,255,0.04)';
            ctx.lineWidth = 0.5 / zoom;

            for (var ri = rowStart; ri <= rowEnd; ri++) {
                var y = matrixTop + ri * CELL_H;
                ctx.beginPath();
                ctx.moveTo(LEFT_W, y);
                ctx.lineTo(matrixRight, y);
                ctx.stroke();
            }
            for (var ci = colStart; ci <= colEnd; ci++) {
                var x = LEFT_W + ci * CELL_W;
                ctx.beginPath();
                ctx.moveTo(x, matrixTop);
                ctx.lineTo(x, matrixTop + nActiveRows * CELL_H);
                ctx.stroke();
            }
        }

        // ── Separator lines ───────────────────────────────────────────
        ctx.strokeStyle = isLight ? 'rgba(0,0,0,0.15)' : 'rgba(255,255,255,0.12)';
        ctx.lineWidth = 1 / zoom;

        drawHLine(0, NAME_H, totalW);
        drawHLine(0, NAME_H + BOUNDS_H, totalW);
        drawHLine(0, matrixTop, totalW);
        drawHLine(0, matrixTop + nActiveRows * CELL_H, totalW);
        drawVLine(ROW_LABEL_W, matrixTop, nActiveRows * CELL_H);
        drawVLine(LEFT_W, 0, totalH);
        drawVLine(matrixRight, 0, totalH);

        // Build col position lookup: original col index -> screen position index
        var colPosMap = {};
        for (var ci = 0; ci < nActiveCols; ci++) {
            colPosMap[activeCols[ci]] = ci;
        }

        // ── Variable names row ────────────────────────────────────────
        if (vy0 < NAME_H) {
            for (var ci = colStart; ci < colEnd; ci++) {
                var x = LEFT_W + ci * CELL_W;
                var v = modelData.variables[activeCols[ci]];
                if (showNumbers) {
                    drawCellText(x, 0, CELL_W, NAME_H, v.name,
                        varColor(v.var_type, 0.5), fontSize * 0.9);
                }
            }
        }

        // ── Upper bounds row ──────────────────────────────────────────
        if (vy0 < NAME_H + BOUNDS_H && vy1 > NAME_H) {
            for (var ci = colStart; ci < colEnd; ci++) {
                var x = LEFT_W + ci * CELL_W;
                var v = modelData.variables[activeCols[ci]];
                if (showNumbers) {
                    drawCellText(x, NAME_H, CELL_W, BOUNDS_H,
                        '≤ ' + (isInf(v.upper) ? '∞' : fmtNum(v.upper)),
                        varColor(v.var_type, 0.7), fontSize);
                }
            }
        }

        // ── Objective row ─────────────────────────────────────────────
        if (vy0 < matrixTop && vy1 > NAME_H + BOUNDS_H) {
            if (vx0 < LEFT_W) {
                drawCellText(0, NAME_H + BOUNDS_H, ROW_LABEL_W, HEADER_H,
                    modelData.obj_sense === 'minimize' ? 'min' : 'max',
                    isLight ? 'rgba(0,0,0,0.5)' : 'rgba(255,255,255,0.5)', fontSize, 'right');
            }
            for (var ci = colStart; ci < colEnd; ci++) {
                var x = LEFT_W + ci * CELL_W;
                var v = modelData.variables[activeCols[ci]];
                if (v.obj !== 0) {
                    drawCoeffCell(x, NAME_H + BOUNDS_H, v.obj, false, isLight, showNumbers, showAbbrev, fontSize);
                }
            }
        }

        // ── Constraint matrix ─────────────────────────────────────────
        for (var ri = rowStart; ri < rowEnd; ri++) {
            var y = matrixTop + ri * CELL_H;
            var origRow = activeRows[ri];
            var con = modelData.constraints[origRow];

            // Row label
            if (vx0 < ROW_LABEL_W) {
                drawCellText(0, y, ROW_LABEL_W, CELL_H, con.name,
                    isLight ? 'rgba(0,0,0,0.5)' : 'rgba(255,255,255,0.5)',
                    fontSize, 'right');
            }

            // Row lower bound
            if (vx0 < LEFT_W && showNumbers) {
                var rhsColor = isLight ? 'rgba(0,0,0,0.5)' : 'rgba(255,255,255,0.5)';
                drawCellText(ROW_LABEL_W, y, RHS_W, CELL_H,
                    (isInf(con.lower) ? '−∞' : fmtNum(con.lower)) + ' ≤',
                    rhsColor, fontSize);
            }

            // Row upper bound
            if (vx1 > matrixRight && showNumbers) {
                var rhsColor = isLight ? 'rgba(0,0,0,0.5)' : 'rgba(255,255,255,0.5)';
                drawCellText(matrixRight, y, RHS_W, CELL_H,
                    '≤ ' + (isInf(con.upper) ? '∞' : fmtNum(con.upper)),
                    rhsColor, fontSize);
            }

            // Coefficients
            var rowData = sparseMatrix[origRow];
            if (rowData) {
                var cols = Object.keys(rowData);
                for (var k = 0; k < cols.length; k++) {
                    var origCol = parseInt(cols[k]);
                    var ci = colPosMap[origCol];
                    if (ci === undefined) continue;  // column eliminated
                    if (ci < colStart || ci >= colEnd) continue;
                    var x = LEFT_W + ci * CELL_W;
                    drawCoeffCell(x, y, rowData[origCol], false, isLight, showNumbers, showAbbrev, fontSize);
                }
            }
        }

        // ── Lower bounds row ──────────────────────────────────────────
        var lbY = matrixTop + nActiveRows * CELL_H;
        if (vy1 > lbY) {
            for (var ci = colStart; ci < colEnd; ci++) {
                var x = LEFT_W + ci * CELL_W;
                var v = modelData.variables[activeCols[ci]];
                if (showNumbers) {
                    drawCellText(x, lbY, CELL_W, BOUNDS_H,
                        '≥ ' + (isInf(v.lower) ? '−∞' : fmtNum(v.lower)),
                        varColor(v.var_type, 0.7), fontSize);
                }
            }
        }

        // ── Pending preview overlay — highlight rows/cols about to be removed ──
        var pendingRowKeys = Object.keys(pendingRows);
        var pendingColKeys = Object.keys(pendingCols);
        if (pendingRowKeys.length > 0 || pendingColKeys.length > 0) {
            var pendFill = isLight
                ? 'rgba(234, 88, 12, 0.28)'
                : 'rgba(234, 88, 12, 0.32)';
            var pendStroke = isLight
                ? 'rgba(234, 88, 12, 0.95)'
                : 'rgba(234, 88, 12, 0.9)';
            ctx.lineWidth = Math.max(1, 2 / zoom);

            // Highlight pending rows across the full width
            for (var pri = 0; pri < pendingRowKeys.length; pri++) {
                var origRow = parseInt(pendingRowKeys[pri]);
                var rowPos = -1;
                for (var ai = 0; ai < activeRows.length; ai++) {
                    if (activeRows[ai] === origRow) { rowPos = ai; break; }
                }
                if (rowPos < 0) continue;
                var py = matrixTop + rowPos * CELL_H;
                ctx.fillStyle = pendFill;
                ctx.fillRect(0, py, totalW, CELL_H);
                ctx.strokeStyle = pendStroke;
                ctx.beginPath();
                ctx.moveTo(0, py); ctx.lineTo(totalW, py);
                ctx.moveTo(0, py + CELL_H); ctx.lineTo(totalW, py + CELL_H);
                ctx.stroke();
            }

            // Highlight pending columns across the full height
            for (var pci = 0; pci < pendingColKeys.length; pci++) {
                var origCol = parseInt(pendingColKeys[pci]);
                var colPos = colPosMap[origCol];
                if (colPos === undefined) continue;
                var px = LEFT_W + colPos * CELL_W;
                ctx.fillStyle = pendFill;
                ctx.fillRect(px, 0, CELL_W, totalH);
                ctx.strokeStyle = pendStroke;
                ctx.beginPath();
                ctx.moveTo(px, 0); ctx.lineTo(px, totalH);
                ctx.moveTo(px + CELL_W, 0); ctx.lineTo(px + CELL_W, totalH);
                ctx.stroke();
            }
        }

        // ── Flash overlay — highlight at the border where row/col was removed ──
        if (flashAlpha > 0) {
            var minFlashPx = 6;
            var flashThickH = Math.max(CELL_H, minFlashPx / zoom);
            var flashThickW = Math.max(CELL_W, minFlashPx / zoom);

            var flashFill = isLight
                ? 'rgba(234, 88, 12,' + (flashAlpha * 0.35) + ')'
                : 'rgba(234, 88, 12,' + (flashAlpha * 0.3) + ')';
            var flashLine = isLight
                ? 'rgba(234, 88, 12,' + (flashAlpha * 0.8) + ')'
                : 'rgba(234, 88, 12,' + (flashAlpha * 0.7) + ')';

            // Flash at the bottom edge of matrix (where rows were just removed)
            var flashRowKeys = Object.keys(flashRows);
            if (flashRowKeys.length > 0) {
                var fy = matrixTop + nActiveRows * CELL_H;
                ctx.fillStyle = flashFill;
                ctx.fillRect(LEFT_W, fy - flashThickH / 2, nActiveCols * CELL_W, flashThickH);
                ctx.strokeStyle = flashLine;
                ctx.lineWidth = Math.max(1, 2 / zoom);
                ctx.beginPath();
                ctx.moveTo(LEFT_W, fy); ctx.lineTo(matrixRight, fy);
                ctx.stroke();
            }

            // Flash at the right edge of matrix (where cols were just removed)
            var flashColKeys = Object.keys(flashCols);
            if (flashColKeys.length > 0) {
                var fx = LEFT_W + nActiveCols * CELL_W;
                ctx.fillStyle = flashFill;
                ctx.fillRect(fx - flashThickW / 2, matrixTop, flashThickW, nActiveRows * CELL_H);
                ctx.strokeStyle = flashLine;
                ctx.lineWidth = Math.max(1, 2 / zoom);
                ctx.beginPath();
                ctx.moveTo(fx, matrixTop); ctx.lineTo(fx, matrixTop + nActiveRows * CELL_H);
                ctx.stroke();
            }
        }

        ctx.restore();
        updateMinimap();
    }

    function drawCoeffCell(x, y, coeff, grayed, isLight, showNumbers, showAbbrev, fontSize) {
        if (showNumbers) {
            var alpha = grayed ? 0.15 : 1;
            var color = isLight
                ? 'rgba(0,0,0,' + alpha + ')'
                : 'rgba(232,234,237,' + alpha + ')';
            drawCellText(x, y, CELL_W, CELL_H, fmtNum(coeff), color, fontSize);
        } else if (showAbbrev) {
            // Draw a dot for nonzero
            if (grayed) {
                ctx.fillStyle = isLight ? 'rgba(160,160,160,0.25)' : 'rgba(90,97,112,0.3)';
            } else {
                ctx.fillStyle = 'rgba(37,99,235,0.5)';
            }
            var dotR = Math.max(1, CELL_W * 0.15);
            ctx.beginPath();
            ctx.arc(x + CELL_W / 2, y + CELL_H / 2, dotR, 0, Math.PI * 2);
            ctx.fill();
        } else {
            // Very zoomed out — tiny pixels
            if (grayed) {
                ctx.fillStyle = isLight ? 'rgba(160,160,160,0.15)' : 'rgba(90,97,112,0.2)';
            } else {
                ctx.fillStyle = 'rgba(37,99,235,0.35)';
            }
            ctx.fillRect(x + 2, y + 2, CELL_W - 4, CELL_H - 4);
        }
    }

    function drawCellText(x, y, w, h, text, color, fontSize, align) {
        ctx.font = fontSize + 'px ' + "'JetBrains Mono', monospace";
        ctx.fillStyle = color;
        ctx.textAlign = align || 'center';
        ctx.textBaseline = 'middle';
        var tx = align === 'right' ? x + w - 6 : align === 'left' ? x + 6 : x + w / 2;
        ctx.fillText(text, tx, y + h / 2);
    }

    function drawHLine(x, y, w) {
        ctx.beginPath();
        ctx.moveTo(x, y);
        ctx.lineTo(x + w, y);
        ctx.stroke();
    }

    function drawVLine(x, y, h) {
        ctx.beginPath();
        ctx.moveTo(x, y);
        ctx.lineTo(x, y + h);
        ctx.stroke();
    }

    // ── Number formatting ──────────────────────────────────────────────
    function isInf(v) {
        return v == null || Math.abs(v) >= 1e19;
    }

    function fmtNum(v) {
        if (v == null) return '';
        if (v === 0) return '0';
        var abs = Math.abs(v);
        if (abs >= 1e6 || (abs < 0.01 && abs > 0)) {
            var s = v.toExponential(1);
            return s.replace('+', '');
        }
        if (Number.isInteger(v)) return v.toString();
        // up to 3 decimal places
        var s = v.toFixed(3).replace(/0+$/, '').replace(/\.$/, '');
        return s;
    }

    function formatRhs(con) {
        if (con.lower != null && con.upper != null) {
            if (con.lower === con.upper) return '= ' + fmtNum(con.lower);
            return fmtNum(con.lower) + '…' + fmtNum(con.upper);
        }
        if (con.upper != null) return '≤ ' + fmtNum(con.upper);
        if (con.lower != null) return '≥ ' + fmtNum(con.lower);
        return '';
    }

    function varColor(vtype, alpha) {
        if (alpha === undefined) alpha = 1;
        switch (vtype) {
            case 'binary': return 'rgba(234,88,12,' + alpha + ')';
            case 'integer': return 'rgba(22,163,74,' + alpha + ')';
            default: return 'rgba(37,99,235,' + alpha + ')';
        }
    }

    // ── Minimap ────────────────────────────────────────────────────────
    var MINIMAP_W = 160;
    var MINIMAP_H = 120;

    function drawMinimap() {
        if (!modelData) return;
        var mmEl = document.getElementById('matrix-minimap');
        mmEl.style.display = 'block';

        var dpr = window.devicePixelRatio || 1;
        minimapCanvas.width = MINIMAP_W * dpr;
        minimapCanvas.height = MINIMAP_H * dpr;
        minimapCanvas.style.width = MINIMAP_W + 'px';
        minimapCanvas.style.height = MINIMAP_H + 'px';
        minimapCtx.setTransform(dpr, 0, 0, dpr, 0, 0);

        var isLight = document.documentElement.classList.contains('light');
        minimapCtx.fillStyle = isLight ? 'rgba(0,0,0,0.03)' : 'rgba(255,255,255,0.03)';
        minimapCtx.fillRect(0, 0, MINIMAP_W, MINIMAP_H);

        var sx = MINIMAP_W / totalW;
        var sy = MINIMAP_H / totalH;
        var matrixTop = NAME_H + BOUNDS_H + HEADER_H;

        // Build col position lookup for minimap
        var colPosMap = {};
        for (var ci = 0; ci < activeCols.length; ci++) {
            colPosMap[activeCols[ci]] = ci;
        }

        // Draw nonzero dots (only active rows/cols)
        var LEFT_W_mm = RHS_W + ROW_LABEL_W;
        for (var ri = 0; ri < activeRows.length; ri++) {
            var origRow = activeRows[ri];
            var rowData = sparseMatrix[origRow];
            if (!rowData) continue;
            var my = (matrixTop + ri * CELL_H + CELL_H / 2) * sy;
            var cols = Object.keys(rowData);
            for (var k = 0; k < cols.length; k++) {
                var origCol = parseInt(cols[k]);
                var ci = colPosMap[origCol];
                if (ci === undefined) continue;
                var mx = (LEFT_W_mm + ci * CELL_W + CELL_W / 2) * sx;
                minimapCtx.fillStyle = isLight ? 'rgba(37,99,235,0.5)' : 'rgba(37,99,235,0.6)';
                minimapCtx.fillRect(mx, my, Math.max(1, sx * CELL_W * 0.8), Math.max(1, sy * CELL_H * 0.8));
            }
        }

        // Flash indicators on minimap
        if (flashAlpha > 0) {
            var mmMatrixLeft = LEFT_W_mm * sx;
            var mmMatrixW = activeCols.length * CELL_W * sx;
            var mmMatrixTop = matrixTop * sy;
            var mmMatrixH = activeRows.length * CELL_H * sy;

            minimapCtx.strokeStyle = 'rgba(234, 88, 12,' + (flashAlpha * 0.9) + ')';
            minimapCtx.lineWidth = 2;

            if (Object.keys(flashRows).length > 0) {
                var fy = mmMatrixTop + mmMatrixH;
                minimapCtx.beginPath();
                minimapCtx.moveTo(mmMatrixLeft, fy);
                minimapCtx.lineTo(mmMatrixLeft + mmMatrixW, fy);
                minimapCtx.stroke();
            }

            if (Object.keys(flashCols).length > 0) {
                var fx = mmMatrixLeft + mmMatrixW;
                minimapCtx.beginPath();
                minimapCtx.moveTo(fx, mmMatrixTop);
                minimapCtx.lineTo(fx, mmMatrixTop + mmMatrixH);
                minimapCtx.stroke();
            }
        }
    }

    function updateMinimap() {
        // Update viewport indicator
        var cw = canvas.clientWidth;
        var ch = canvas.clientHeight;
        var sx = MINIMAP_W / totalW;
        var sy = MINIMAP_H / totalH;

        var vx = (-panX / zoom) * sx;
        var vy = (-panY / zoom) * sy;
        var vw = (cw / zoom) * sx;
        var vh = (ch / zoom) * sy;

        minimapViewport.style.left = Math.max(0, vx) + 'px';
        minimapViewport.style.top = Math.max(0, vy) + 'px';
        minimapViewport.style.width = Math.min(MINIMAP_W, vw) + 'px';
        minimapViewport.style.height = Math.min(MINIMAP_H, vh) + 'px';
    }

    // ── Events ─────────────────────────────────────────────────────────
    function setupEvents() {
        // Zoom
        canvas.addEventListener('wheel', function (e) {
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
        canvas.addEventListener('mousedown', function (e) {
            dragging = true;
            dragStartX = e.clientX;
            dragStartY = e.clientY;
            dragPanX = panX;
            dragPanY = panY;
            canvas.style.cursor = 'grabbing';
        });
        window.addEventListener('mousemove', function (e) {
            if (!dragging) return;
            panX = dragPanX + (e.clientX - dragStartX);
            panY = dragPanY + (e.clientY - dragStartY);
            redraw();
        });
        window.addEventListener('mouseup', function () {
            dragging = false;
            canvas.style.cursor = 'grab';
        });

        canvas.style.cursor = 'grab';

        // Resize
        window.addEventListener('resize', function () {
            resizeCanvas();
            redraw();
        });

        // Hover info
        canvas.addEventListener('mousemove', function (e) {
            if (dragging) return;
            var rect = canvas.getBoundingClientRect();
            var mx = (e.clientX - rect.left - panX) / zoom;
            var my = (e.clientY - rect.top - panY) / zoom;
            updateHoverInfo(mx, my);
        });

        // Stepper
        document.getElementById('step-prev').addEventListener('click', function () { step(-1); });
        document.getElementById('step-next').addEventListener('click', function () { step(1); });
        document.getElementById('step-reset').addEventListener('click', function () { stopPlay(); resetSteps(); });
        document.getElementById('step-play').addEventListener('click', function () { togglePlay(false); });
        document.getElementById('step-fast').addEventListener('click', function () { togglePlay(true); });

        var slider = document.getElementById('step-slider');
        slider.addEventListener('input', function () {
            jumpToStep(parseInt(slider.value));
        });

        document.getElementById('reduction-panel-close').addEventListener('click', function () {
            document.getElementById('reduction-panel').style.display = 'none';
        });

        // Keyboard
        window.addEventListener('keydown', function (e) {
            if (e.target.tagName === 'INPUT') return;
            if (e.key === 'ArrowLeft') { e.preventDefault(); step(-1); }
            if (e.key === 'ArrowRight') { e.preventDefault(); step(1); }
            if (e.key === 'Escape') {
                if (pendingDir !== 0) {
                    clearPending();
                    updateStepperUI();
                    redraw();
                    drawMinimap();
                }
            }
            if (e.key === 'r' || e.key === 'R') resetSteps();
            if (e.key === 'f' || e.key === 'F') { fitToScreen(); redraw(); }
        });

        // Minimap click-to-navigate
        minimapCanvas.addEventListener('click', function (e) {
            var rect = minimapCanvas.getBoundingClientRect();
            var mx = e.clientX - rect.left;
            var my = e.clientY - rect.top;
            var sx = MINIMAP_W / totalW;
            var sy = MINIMAP_H / totalH;
            // Center viewport on clicked point
            var targetX = mx / sx;
            var targetY = my / sy;
            panX = canvas.clientWidth / 2 - targetX * zoom;
            panY = canvas.clientHeight / 2 - targetY * zoom;
            redraw();
        });
    }

    function updateHoverInfo(mx, my) {
        var infoEl = document.getElementById('matrix-info');
        var matrixTop = NAME_H + BOUNDS_H + HEADER_H;

        // Check if hovering over a matrix cell
        var ci = Math.floor((mx - (RHS_W + ROW_LABEL_W)) / CELL_W);
        var ri = Math.floor((my - matrixTop) / CELL_H);

        if (ci >= 0 && ci < activeCols.length && ri >= 0 && ri < activeRows.length) {
            var origCol = activeCols[ci];
            var origRow = activeRows[ri];
            var v = modelData.variables[origCol];
            var con = modelData.constraints[origRow];
            var coeff = (sparseMatrix[origRow] && sparseMatrix[origRow][origCol]) || 0;
            infoEl.textContent = con.name + ' × ' + v.name + ' = ' + fmtNum(coeff);
            infoEl.style.display = 'block';
        } else {
            infoEl.style.display = 'none';
        }
    }

    // ── Stepper ────────────────────────────────────────────────────────
    function step(dir) {
        if (reductions.length === 0) return;
        if (pendingDir === dir) {
            // Second press in same direction — commit the change
            clearPending();
            commitStep(dir);
            return;
        }
        if (pendingDir !== 0) {
            // Switching direction — clear current preview, then preview the other way
            clearPending();
        }
        previewStep(dir);
    }

    function previewStep(dir) {
        var idx;
        if (dir === 1) {
            if (currentStep >= reductions.length) return;
            idx = currentStep; // the next reduction to apply
        } else {
            if (currentStep === 0) return;
            idx = currentStep - 1; // the last applied reduction (will be undone)
        }
        pendingDir = dir;
        pendingRows = {};
        pendingCols = {};
        getAffected(reductions[idx], pendingRows, pendingCols);
        updateStepperUI();
        redraw();
        drawMinimap();
    }

    function clearPending() {
        if (pendingDir === 0) return;
        pendingDir = 0;
        pendingRows = {};
        pendingCols = {};
    }

    function commitStep(dir) {
        var newStep = currentStep + dir;
        if (newStep < 0 || newStep > reductions.length) return;
        currentStep = newStep;
        rebuildGrayed();
        fitToScreen();
        updateStepperUI();
        triggerFlash();
        drawMinimap();
    }

    function jumpToStep(s) {
        clearPending();
        currentStep = s;
        rebuildGrayed();
        fitToScreen();
        updateStepperUI();
        triggerFlash();
        drawMinimap();
    }

    function resetSteps() {
        clearPending();
        currentStep = 0;
        flashRows = {};
        flashCols = {};
        flashAlpha = 0;
        if (flashAnimId) { cancelAnimationFrame(flashAnimId); flashAnimId = null; }
        rebuildGrayed();
        fitToScreen();
        updateStepperUI();
        redraw();
        drawMinimap();
    }

    var playFast = false;

    function togglePlay(fast) {
        if (playing) {
            stopPlay();
        } else {
            playFast = !!fast;
            startPlay();
        }
    }

    function startPlay() {
        clearPending();
        if (currentStep >= reductions.length) {
            resetSteps();
        }
        playing = true;
        var btn = playFast ? 'step-fast' : 'step-play';
        document.getElementById(btn).innerHTML = '&#9646;&#9646;';
        document.getElementById(btn).title = 'Pause';
        playTick();
    }

    function stopPlay() {
        playing = false;
        if (playTimerId) { clearTimeout(playTimerId); playTimerId = null; }
        document.getElementById('step-play').innerHTML = '&#9654;';
        document.getElementById('step-play').title = 'Auto-play reductions';
        document.getElementById('step-fast').innerHTML = '&#9654;&#9654;';
        document.getElementById('step-fast').title = 'Fast-forward reductions';
        playFast = false;
        drawMinimap();
    }

    function playTick() {
        if (!playing || currentStep >= reductions.length) {
            stopPlay();
            return;
        }
        if (playFast) {
            // Apply a batch of steps per tick with flash on the last one
            var batch = Math.max(1, Math.ceil(reductions.length / 60));
            for (var i = 0; i < batch && currentStep < reductions.length; i++) {
                currentStep++;
            }
            rebuildGrayed();
            fitToScreen();
            // Flash the last applied reduction
            flashRows = {};
            flashCols = {};
            if (currentStep > 0) {
                getAffected(reductions[currentStep - 1], flashRows, flashCols);
            }
            flashAlpha = 0.7;
            updateStepperUI();
            redraw();
            playTimerId = setTimeout(playTick, 50);
        } else {
            step(1);
            playTimerId = setTimeout(playTick, 400);
        }
    }

    function triggerFlash() {
        flashRows = {};
        flashCols = {};
        if (currentStep > 0) {
            var r = reductions[currentStep - 1];
            getAffected(r, flashRows, flashCols);
        }
        flashAlpha = 1;
        if (flashAnimId) cancelAnimationFrame(flashAnimId);
        animateFlash();
    }

    function animateFlash() {
        flashAlpha *= 0.96;
        if (flashAlpha < 0.01) {
            flashAlpha = 0;
            flashRows = {};
            flashCols = {};
            redraw();
            drawMinimap();
            return;
        }
        redraw();
        drawMinimap();
        flashAnimId = requestAnimationFrame(animateFlash);
    }

    function getAffected(r, rows, cols) {
        var type = r.reduction_type;
        if (type === 'FixedCol' || type === 'FreeColSubstitution' ||
            type === 'SlackColSubstitution' || type === 'DuplicateColumn' ||
            type === 'ForcingColumn') {
            if (r.col >= 0) cols[r.col] = true;
        }
        if (type === 'RedundantRow' || type === 'SingletonRow' ||
            type === 'ForcingRow' || type === 'ForcingColumnRemovedRow' ||
            type === 'DuplicateRow') {
            if (r.row >= 0) rows[r.row] = true;
        }
        if (type === 'DoubletonEquation') {
            if (r.row >= 0) rows[r.row] = true;
            if (r.col >= 0) cols[r.col] = true;
        }
    }

    function rebuildGrayed() {
        grayedRows = {};
        grayedCols = {};
        for (var i = 0; i < currentStep; i++) {
            var r = reductions[i];
            applyReductionGray(r);
        }
        rebuildActive();
    }

    function rebuildActive() {
        activeRows = [];
        activeCols = [];
        for (var r = 0; r < numRows; r++) {
            if (!grayedRows[r]) activeRows.push(r);
        }
        for (var c = 0; c < numCols; c++) {
            if (!grayedCols[c]) activeCols.push(c);
        }
        recalcLayout();
    }

    function recalcLayout() {
        totalW = RHS_W + ROW_LABEL_W + activeCols.length * CELL_W + RHS_W;
        totalH = NAME_H + BOUNDS_H + HEADER_H + activeRows.length * CELL_H + BOUNDS_H;
    }

    function applyReductionGray(r) {
        var type = r.reduction_type;
        if (type === 'FixedCol' || type === 'FreeColSubstitution' ||
            type === 'SlackColSubstitution' || type === 'DuplicateColumn' ||
            type === 'ForcingColumn') {
            if (r.col >= 0) grayedCols[r.col] = true;
        }
        if (type === 'RedundantRow' || type === 'SingletonRow' ||
            type === 'ForcingRow' || type === 'ForcingColumnRemovedRow' ||
            type === 'DuplicateRow') {
            if (r.row >= 0) grayedRows[r.row] = true;
        }
        if (type === 'DoubletonEquation') {
            // Removes the row and substitutes a column
            if (r.row >= 0) grayedRows[r.row] = true;
            if (r.col >= 0) grayedCols[r.col] = true;
        }
    }

    function updateStepperUI() {
        document.getElementById('step-label').textContent =
            'Step ' + currentStep + ' / ' + reductions.length;

        document.getElementById('step-prev').disabled = currentStep === 0;
        document.getElementById('step-next').disabled = currentStep >= reductions.length;
        document.getElementById('step-slider').value = currentStep;

        var infoEl = document.getElementById('step-info');
        if (pendingDir !== 0) {
            var pidx = pendingDir === 1 ? currentStep : currentStep - 1;
            var pr = reductions[pidx];
            var arrow = pendingDir === 1 ? '→' : '←';
            var verb = pendingDir === 1 ? 'apply' : 'undo';
            infoEl.innerHTML = '<span class="step-type">' + pr.reduction_type + '</span> ' +
                '<span class="step-source">[' + pr.source + ']</span> ' +
                '<span class="step-desc">' + pr.description + '</span> ' +
                '<span class="step-pending">— press ' + arrow + ' again to ' + verb + ' (Esc to cancel)</span>';
        } else if (currentStep > 0) {
            var r = reductions[currentStep - 1];
            infoEl.innerHTML = '<span class="step-type">' + r.reduction_type + '</span> ' +
                '<span class="step-source">[' + r.source + ']</span> ' +
                '<span class="step-desc">' + r.description + '</span>';
        } else {
            infoEl.textContent = 'Original matrix — use → to step through reductions';
        }

        // Update stats
        var nAR = activeRows.length;
        var nAC = activeCols.length;
        var activeNz = 0;
        for (var ri = 0; ri < nAR; ri++) {
            var rowData = sparseMatrix[activeRows[ri]];
            if (!rowData) continue;
            var cols = Object.keys(rowData);
            for (var ci = 0; ci < cols.length; ci++) {
                if (!grayedCols[parseInt(cols[ci])]) activeNz++;
            }
        }
        var statsText = nAR + ' rows × ' + nAC + ' cols, ' + activeNz + ' nz';
        if (currentStep > 0) {
            statsText += ' (−' + (numRows - nAR) + ' rows, −' + (numCols - nAC) + ' cols)';
        }
        document.getElementById('step-label').textContent =
            'Step ' + currentStep + ' / ' + reductions.length + '  ·  ' + statsText;

        updateReductionPanel();
    }

    function updateReductionPanel() {
        var panel = document.getElementById('reduction-panel');
        var body = document.getElementById('reduction-panel-body');

        if (currentStep === 0 || playing) {
            panel.style.display = 'none';
            return;
        }

        panel.style.display = '';
        var r = reductions[currentStep - 1];
        var html = '';

        // Type & source
        html += '<div class="rdp-section">';
        html += '<div class="rdp-label">Type</div>';
        html += '<div class="rdp-value"><span class="rdp-type">' + r.reduction_type + '</span></div>';
        html += '</div>';

        html += '<div class="rdp-section">';
        html += '<div class="rdp-label">Found by</div>';
        html += '<div class="rdp-value"><span class="rdp-source">' + r.source + '</span></div>';
        html += '</div>';

        if (r.value !== 0) {
            html += '<div class="rdp-section">';
            html += '<div class="rdp-label">Value</div>';
            html += '<div class="rdp-value">' + fmtNum(r.value) + '</div>';
            html += '</div>';
        }

        html += '<hr class="rdp-divider">';

        // Affected row details
        if (r.row >= 0 && r.row < numRows) {
            var con = modelData.constraints[r.row];
            html += '<div class="rdp-section">';
            html += '<div class="rdp-label">Affected Row <span class="rdp-tag rdp-tag-row">' + con.name + '</span></div>';
            html += '<div class="rdp-value">';
            html += '<div class="rdp-stats-row"><span>Nonzeros</span><span class="rdp-stats-val">' +
                (con.terms ? con.terms.length : 0) + '</span></div>';
            html += '<div class="rdp-stats-row"><span>Bounds</span><span class="rdp-stats-val">' +
                formatRhs(con) + '</span></div>';
            html += '</div>';

            // Show constraint terms
            if (con.terms && con.terms.length > 0 && con.terms.length <= 30) {
                html += '<div class="rdp-terms">';
                for (var i = 0; i < con.terms.length; i++) {
                    var t = con.terms[i];
                    if (i > 0 && t.coeff >= 0) html += ' + ';
                    else if (i > 0) html += ' ';
                    html += '<span class="rdp-term-coeff">' + fmtNum(t.coeff) + '</span>';
                    html += '<span class="rdp-term-var rdp-tag-var ' + t.var_type + '"> ' + t.var_name + '</span>';
                }
                html += ' ' + formatRhs(con);
                html += '</div>';
            } else if (con.terms && con.terms.length > 30) {
                html += '<div class="rdp-terms" style="color:var(--text-muted)">' +
                    con.terms.length + ' terms (too many to display)</div>';
            }
            html += '</div>';
        }

        // Affected column details
        if (r.col >= 0 && r.col < numCols) {
            var v = modelData.variables[r.col];
            html += '<div class="rdp-section">';
            html += '<div class="rdp-label">Affected Column <span class="rdp-tag rdp-tag-col">' + v.name + '</span></div>';
            html += '<div class="rdp-value">';
            html += '<div class="rdp-stats-row"><span>Type</span><span class="rdp-stats-val rdp-tag-var ' +
                v.var_type + '">' + v.var_type + '</span></div>';
            html += '<div class="rdp-stats-row"><span>Bounds</span><span class="rdp-stats-val">' +
                (v.lower != null ? fmtNum(v.lower) : '-∞') + ' ≤ ' + v.name + ' ≤ ' +
                (v.upper != null ? fmtNum(v.upper) : '∞') + '</span></div>';
            html += '<div class="rdp-stats-row"><span>Objective</span><span class="rdp-stats-val">' +
                fmtNum(v.obj) + '</span></div>';

            // Count nonzeros in this column
            var colNz = 0;
            for (var ri = 0; ri < numRows; ri++) {
                if (sparseMatrix[ri] && sparseMatrix[ri][r.col] !== undefined) colNz++;
            }
            html += '<div class="rdp-stats-row"><span>Nonzeros</span><span class="rdp-stats-val">' +
                colNz + '</span></div>';
            html += '</div></div>';
        }

        // Explanation
        html += '<hr class="rdp-divider">';
        html += '<div class="rdp-section">';
        html += '<div class="rdp-label">Explanation</div>';
        html += '<div class="rdp-value" style="color:var(--text-secondary);line-height:1.7">' +
            reductionExplanation(r) + '</div>';
        html += '</div>';

        body.innerHTML = html;
    }

    function reductionExplanation(r) {
        var type = r.reduction_type;
        var row = r.row >= 0 && r.row < numRows ? modelData.constraints[r.row].name : '';
        var col = r.col >= 0 && r.col < numCols ? modelData.variables[r.col].name : '';

        switch (type) {
            case 'RedundantRow':
                return 'Row ' + row + ' is redundant — it is implied by variable bounds or other constraints and can be removed.';
            case 'SingletonRow':
                return 'Row ' + row + ' has a single nonzero. This directly determines a variable bound or fixes a variable.';
            case 'FixedCol':
                return 'Variable ' + col + ' is fixed to ' + fmtNum(r.value) + '. Its column can be removed and bounds adjusted.';
            case 'ForcingRow':
                return 'Row ' + row + ' forces all its variables to one of their bounds. The constraint and implied fixings are applied.';
            case 'ForcingColumn':
                return 'Column ' + col + ' is forcing — the variable can be fixed based on its cost and constraint structure.';
            case 'ForcingColumnRemovedRow':
                return 'A forcing column caused row ' + row + ' to become redundant and it is removed.';
            case 'DoubletonEquation':
                return 'Row ' + row + ' is an equality with two variables. Variable ' + col + ' is substituted out, eliminating both the row and column.';
            case 'FreeColSubstitution':
                return 'Variable ' + col + ' is free (unbounded) and appears in an equation, allowing substitution.';
            case 'SlackColSubstitution':
                return 'Variable ' + col + ' acts as a slack and is substituted out.';
            case 'DuplicateRow':
                return 'Row ' + row + ' is a duplicate of another constraint and is removed.';
            case 'DuplicateColumn':
                return 'Column ' + col + ' is a duplicate of another variable and is merged.';
            case 'LinearTransform':
                return 'A linear transformation is applied to column ' + col + '.';
            case 'EqualityRowAddition':
                return 'An equality row is added to row ' + row + ' to reduce fill-in.';
            default:
                return r.description;
        }
    }

})();

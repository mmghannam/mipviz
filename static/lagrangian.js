// Lagrangian Relaxation Visualization
(function () {
    const INF = 1e20;
    const SAMPLES = 500;
    const COLORS = ['#60a5fa', '#34d399', '#f472b6', '#fbbf24', '#a78bfa', '#fb923c', '#22d3ee', '#f87171'];

    let panel, canvas, ctx, pillsEl, infoEl;
    let selected = []; // [{idx, name, color, points, optLambda, optL, bps, isEq}]
    let colorIdx = 0;
    let varMap = null;

    function createPanel() {
        panel = document.createElement('div');
        panel.className = 'lagrangian-panel';
        panel.innerHTML =
            '<div class="lr-header">' +
                '<div>' +
                    '<div class="lr-title">Lagrangian Relaxation <span class="lr-help" id="lr-help-btn">?</span></div>' +
                    '<div class="lr-subtitle">Click constraint names to add/remove</div>' +
                '</div>' +
                '<button class="lr-close">&times;</button>' +
            '</div>' +
            '<div class="lr-help-popover" id="lr-help-popover" style="display:none">' +
                '<p>For a constraint <code>aᵀx ≤ b</code>, the Lagrangian relaxation dualizes it with multiplier <code>λ ≥ 0</code>:</p>' +
                '<p class="lr-help-formula"><code>L(λ) = min { cᵀx + λ(aᵀx − b) : x ∈ bounds }</code></p>' +
                '<p>This decomposes into per-variable minimizations.</p>' +
                '<ul>' +
                    '<li><strong>Piecewise-linear</strong> — L(λ) is concave and piecewise-linear</li>' +
                    '<li><strong>Peak = tightest bound</strong> — the maximum gives the best lower bound from this constraint</li>' +
                    '<li><strong>Breakpoints</strong> — occur where a variable switches between its bounds</li>' +
                '</ul>' +
            '</div>' +
            '<div class="lr-chart-wrap"><canvas id="lr-canvas"></canvas></div>' +
            '<div class="lr-info" id="lr-info"></div>' +
            '<div class="lr-pills" id="lr-pills"></div>' +
            '<div class="lr-actions"><button class="lr-clear-btn" id="lr-clear-btn">Clear all</button></div>';
        document.body.appendChild(panel);
        canvas = document.getElementById('lr-canvas');
        ctx = canvas.getContext('2d');
        pillsEl = document.getElementById('lr-pills');
        infoEl = document.getElementById('lr-info');

        panel.querySelector('.lr-close').addEventListener('click', function () {
            panel.classList.remove('open');
            document.body.classList.remove('lr-panel-open');
        });

        // Help popover toggle
        var helpBtn = document.getElementById('lr-help-btn');
        var helpPopover = document.getElementById('lr-help-popover');
        helpBtn.addEventListener('click', function (e) {
            e.stopPropagation();
            var isOpen = helpPopover.style.display === 'block';
            helpPopover.style.display = isOpen ? 'none' : 'block';
        });
        document.addEventListener('click', function (e) {
            if (!helpPopover.contains(e.target) && e.target !== helpBtn) {
                helpPopover.style.display = 'none';
            }
        });

        document.getElementById('lr-clear-btn').addEventListener('click', function () {
            selected = [];
            colorIdx = 0;
            panel.classList.remove('open');
            document.body.classList.remove('lr-panel-open');
            pillsEl.innerHTML = '';
            infoEl.innerHTML = '';
            updateNameHighlights();
        });

        // Handle mouse move for crosshair tooltip
        canvas.addEventListener('mousemove', onCanvasHover);
        canvas.addEventListener('mouseleave', function () {
            hoverLambda = null;
            drawChart();
        });
    }

    var hoverLambda = null;

    function ensureVarMap() {
        if (varMap) return;
        varMap = new Map();
        for (var i = 0; i < modelData.variables.length; i++) {
            var v = modelData.variables[i];
            varMap.set(v.name, {
                obj: v.obj,
                lower: (v.lower === null || v.lower < -INF) ? null : v.lower,
                upper: (v.upper === null || v.upper > INF) ? null : v.upper,
            });
        }
    }

    function analyzeConstraint(idx) {
        var c = modelData.constraints[idx];
        ensureVarMap();

        var lowInf = c.lower === null || c.lower < -INF;
        var upInf = c.upper === null || c.upper > INF;
        var isEq = !lowInf && !upInf && Math.abs(c.lower - c.upper) < 1e-10;
        var isLeq = lowInf && !upInf;

        var sign, rhs;
        if (isLeq) { sign = 1; rhs = c.upper; }
        else if (!lowInf && upInf) { sign = -1; rhs = -c.lower; } // ≥ → negate
        else if (isEq) { sign = 1; rhs = c.upper; }
        else { sign = 1; rhs = c.upper; } // ranged: relax upper

        // Precompute constant from variables NOT in constraint
        var termSet = new Set();
        for (var i = 0; i < c.terms.length; i++) termSet.add(c.terms[i].var_name);

        var constant = 0;
        for (var i = 0; i < modelData.variables.length; i++) {
            var vname = modelData.variables[i].name;
            if (termSet.has(vname)) continue;
            var vd = varMap.get(vname);
            if (vd.obj >= 0) {
                if (vd.lower === null) return null;
                constant += vd.obj * vd.lower;
            } else {
                if (vd.upper === null) return null;
                constant += vd.obj * vd.upper;
            }
        }

        // L(λ) for this constraint (only constraint terms vary with λ)
        function computeL(lambda) {
            var val = constant - lambda * rhs;
            for (var j = 0; j < c.terms.length; j++) {
                var t = c.terms[j];
                var v = varMap.get(t.var_name);
                var effCoeff = v.obj + lambda * sign * t.coeff;
                if (effCoeff >= 0) {
                    if (v.lower === null) return -Infinity;
                    val += effCoeff * v.lower;
                } else {
                    if (v.upper === null) return -Infinity;
                    val += effCoeff * v.upper;
                }
            }
            return val;
        }

        // Breakpoints
        var bps = [];
        for (var i = 0; i < c.terms.length; i++) {
            var t = c.terms[i];
            var v = varMap.get(t.var_name);
            var a = sign * t.coeff;
            if (Math.abs(a) < 1e-12) continue;
            var lam = -v.obj / a;
            if (lam > 1e-9) bps.push({ varName: t.var_name, lambda: lam });
        }
        bps.sort(function (a, b) { return a.lambda - b.lambda; });

        // Lambda range
        var maxLam;
        if (bps.length > 0) {
            maxLam = bps[bps.length - 1].lambda * 1.5;
        } else {
            maxLam = 10;
        }
        if (maxLam < 1) maxLam = 1;
        var minLam = isEq ? -maxLam : 0;

        // Sample and include breakpoint neighborhoods
        var lambdaSet = new Set();
        for (var i = 0; i <= SAMPLES; i++) {
            lambdaSet.add(+(minLam + (maxLam - minLam) * i / SAMPLES).toFixed(6));
        }
        for (var i = 0; i < bps.length; i++) {
            var l = bps[i].lambda;
            if (l >= minLam && l <= maxLam) {
                lambdaSet.add(+(l - 0.0001).toFixed(6));
                lambdaSet.add(+l.toFixed(6));
                lambdaSet.add(+(l + 0.0001).toFixed(6));
            }
        }

        var lambdas = Array.from(lambdaSet).sort(function (a, b) { return a - b; });
        var points = [];
        var optLambda = minLam, optL = -Infinity;
        for (var i = 0; i < lambdas.length; i++) {
            var lam = lambdas[i];
            var L = computeL(lam);
            if (L === -Infinity) continue;
            points.push({ lambda: lam, L: L });
            if (L > optL) { optL = L; optLambda = lam; }
        }

        if (points.length === 0) return null;

        return { points: points, optLambda: optLambda, optL: optL, bps: bps, isEq: isEq };
    }

    // Public: toggle a constraint
    window.toggleLagrangianRow = function (idx) {
        if (!panel) createPanel();
        ensureVarMap();

        // Check if already selected
        var existingIdx = -1;
        for (var i = 0; i < selected.length; i++) {
            if (selected[i].idx === idx) { existingIdx = i; break; }
        }

        if (existingIdx >= 0) {
            selected.splice(existingIdx, 1);
            if (selected.length === 0) {
                panel.classList.remove('open');
                document.body.classList.remove('lr-panel-open');
                updateNameHighlights();
                return;
            }
        } else {
            var result = analyzeConstraint(idx);
            if (!result) return; // unbounded
            var color = COLORS[colorIdx % COLORS.length];
            colorIdx++;
            selected.push({
                idx: idx,
                name: modelData.constraints[idx].name,
                color: color,
                points: result.points,
                optLambda: result.optLambda,
                optL: result.optL,
                bps: result.bps,
                isEq: result.isEq,
            });
        }

        panel.classList.add('open');
        document.body.classList.add('lr-panel-open');
        renderPills();
        drawChart();
        renderInfo();
        updateNameHighlights();
    };

    // Public: add all visible (non-filtered) constraints
    window.addAllVisibleToLagrangian = function () {
        if (!panel) createPanel();
        ensureVarMap();

        var rows = document.querySelectorAll('.constraint-row:not(.filtered-out)');
        var added = 0;
        rows.forEach(function (row) {
            var nameEl = row.querySelector('.constraint-name');
            if (!nameEl) return;
            var idx = parseInt(nameEl.dataset.constraintIdx);
            if (isNaN(idx)) return;
            // Skip if already selected
            var already = false;
            for (var i = 0; i < selected.length; i++) {
                if (selected[i].idx === idx) { already = true; break; }
            }
            if (already) return;

            var result = analyzeConstraint(idx);
            if (!result) return;
            var color = COLORS[colorIdx % COLORS.length];
            colorIdx++;
            selected.push({
                idx: idx,
                name: modelData.constraints[idx].name,
                color: color,
                points: result.points,
                optLambda: result.optLambda,
                optL: result.optL,
                bps: result.bps,
                isEq: result.isEq,
            });
            added++;
        });

        if (selected.length > 0) {
            panel.classList.add('open');
            document.body.classList.add('lr-panel-open');
            renderPills();
            drawChart();
            renderInfo();
            updateNameHighlights();
        }
    };

    function updateNameHighlights() {
        var names = document.querySelectorAll('.constraint-name');
        var selectedIdxs = new Set(selected.map(function (s) { return s.idx; }));
        names.forEach(function (el) {
            var idx = parseInt(el.dataset.constraintIdx);
            if (!isNaN(idx) && selectedIdxs.has(idx)) {
                var entry = selected.find(function (s) { return s.idx === idx; });
                el.classList.add('lr-active');
                el.style.borderColor = entry ? entry.color : '';
            } else {
                el.classList.remove('lr-active');
                el.style.borderColor = '';
            }
        });
    }

    function renderPills() {
        pillsEl.innerHTML = '';
        for (var i = 0; i < selected.length; i++) {
            (function (s) {
                var pill = document.createElement('span');
                pill.className = 'lr-pill';
                pill.style.borderColor = s.color;
                pill.style.color = s.color;
                pill.innerHTML = s.name + ' <span class="lr-pill-x">&times;</span>';
                pill.addEventListener('click', function () { toggleLagrangianRow(s.idx); });
                pillsEl.appendChild(pill);
            })(selected[i]);
        }
    }

    function renderInfo() {
        var html = '';
        for (var i = 0; i < selected.length; i++) {
            var s = selected[i];
            html += '<div class="lr-info-row">' +
                '<span class="lr-info-name" style="color:' + s.color + '">' + s.name + '</span>' +
                '<span class="lr-info-val">\u03BB* = ' + s.optLambda.toFixed(4) + '</span>' +
                '<span class="lr-info-val">L* = ' + s.optL.toFixed(2) + '</span>' +
                '</div>';
        }
        infoEl.innerHTML = html;
    }

    function onCanvasHover(e) {
        if (!selected.length) return;
        var rect = canvas.getBoundingClientRect();
        var mx = e.clientX - rect.left;
        var dpr = window.devicePixelRatio || 1;

        // Chart margins (must match drawChart)
        var margin = { top: 24, right: 20, bottom: 36, left: 56 };
        var w = canvas.width / dpr;
        var plotW = w - margin.left - margin.right;

        var frac = (mx - margin.left) / plotW;
        if (frac < 0 || frac > 1) { hoverLambda = null; drawChart(); return; }

        // Get global lambda range
        var range = getGlobalRange();
        hoverLambda = range.minLam + frac * (range.maxLam - range.minLam);
        drawChart();
    }

    function getGlobalRange() {
        var minLam = Infinity, maxLam = -Infinity;
        var minL = Infinity, maxL = -Infinity;
        for (var i = 0; i < selected.length; i++) {
            var pts = selected[i].points;
            for (var j = 0; j < pts.length; j++) {
                if (pts[j].lambda < minLam) minLam = pts[j].lambda;
                if (pts[j].lambda > maxLam) maxLam = pts[j].lambda;
                if (pts[j].L < minL) minL = pts[j].L;
                if (pts[j].L > maxL) maxL = pts[j].L;
            }
        }
        var yPad = (maxL - minL) * 0.1 || 1;
        return { minLam: minLam, maxLam: maxLam, minL: minL - yPad, maxL: maxL + yPad };
    }

    function drawChart() {
        if (!selected.length) return;

        var dpr = window.devicePixelRatio || 1;
        var dispW = canvas.parentElement.clientWidth;
        var dispH = 260;
        canvas.style.width = dispW + 'px';
        canvas.style.height = dispH + 'px';
        canvas.width = dispW * dpr;
        canvas.height = dispH * dpr;
        ctx.setTransform(dpr, 0, 0, dpr, 0, 0);

        var styles = getComputedStyle(document.documentElement);
        var textMuted = styles.getPropertyValue('--text-muted').trim();
        var border = styles.getPropertyValue('--border-subtle').trim();
        var bgCard = styles.getPropertyValue('--bg-card').trim();

        var margin = { top: 24, right: 20, bottom: 36, left: 56 };
        var w = dispW, h = dispH;
        var plotW = w - margin.left - margin.right;
        var plotH = h - margin.top - margin.bottom;

        var range = getGlobalRange();
        var minLam = range.minLam, maxLam = range.maxLam;
        var minL = range.minL, maxL = range.maxL;

        function xPos(lam) { return margin.left + (lam - minLam) / (maxLam - minLam) * plotW; }
        function yPos(L) { return margin.top + (1 - (L - minL) / (maxL - minL)) * plotH; }

        // Clear
        ctx.clearRect(0, 0, w, h);

        // Grid
        ctx.strokeStyle = border;
        ctx.lineWidth = 0.5;
        var yTicks = niceScale(minL, maxL, 5);
        for (var i = 0; i < yTicks.length; i++) {
            var y = yPos(yTicks[i]);
            ctx.beginPath(); ctx.moveTo(margin.left, y); ctx.lineTo(w - margin.right, y); ctx.stroke();
        }
        var xTicks = niceScale(minLam, maxLam, 6);
        for (var i = 0; i < xTicks.length; i++) {
            var x = xPos(xTicks[i]);
            ctx.beginPath(); ctx.moveTo(x, margin.top); ctx.lineTo(x, h - margin.bottom); ctx.stroke();
        }

        // Axis labels
        ctx.font = '10px "JetBrains Mono", monospace';
        ctx.fillStyle = textMuted;
        ctx.textAlign = 'right';
        ctx.textBaseline = 'middle';
        for (var i = 0; i < yTicks.length; i++) {
            ctx.fillText(formatTickVal(yTicks[i]), margin.left - 6, yPos(yTicks[i]));
        }
        ctx.textAlign = 'center';
        ctx.textBaseline = 'top';
        for (var i = 0; i < xTicks.length; i++) {
            ctx.fillText(formatTickVal(xTicks[i]), xPos(xTicks[i]), h - margin.bottom + 6);
        }

        // X axis label
        ctx.fillStyle = textMuted;
        ctx.textAlign = 'center';
        ctx.fillText('\u03BB', w / 2, h - 6);

        // Y axis label
        ctx.save();
        ctx.translate(12, h / 2);
        ctx.rotate(-Math.PI / 2);
        ctx.textAlign = 'center';
        ctx.fillText('L(\u03BB)', 0, 0);
        ctx.restore();

        // Draw curves
        for (var s = 0; s < selected.length; s++) {
            var sel = selected[s];
            var pts = sel.points;
            if (pts.length < 2) continue;

            ctx.strokeStyle = sel.color;
            ctx.lineWidth = 2;
            ctx.beginPath();
            ctx.moveTo(xPos(pts[0].lambda), yPos(pts[0].L));
            for (var i = 1; i < pts.length; i++) {
                ctx.lineTo(xPos(pts[i].lambda), yPos(pts[i].L));
            }
            ctx.stroke();

            // Optimal marker
            ctx.fillStyle = sel.color;
            ctx.beginPath();
            ctx.arc(xPos(sel.optLambda), yPos(sel.optL), 4, 0, Math.PI * 2);
            ctx.fill();

            // Breakpoint markers
            ctx.fillStyle = sel.color;
            ctx.globalAlpha = 0.4;
            for (var i = 0; i < sel.bps.length; i++) {
                var bp = sel.bps[i];
                if (bp.lambda >= minLam && bp.lambda <= maxLam) {
                    var bpL = interpolateL(sel.points, bp.lambda);
                    if (bpL !== null) {
                        ctx.beginPath();
                        ctx.arc(xPos(bp.lambda), yPos(bpL), 2.5, 0, Math.PI * 2);
                        ctx.fill();
                    }
                }
            }
            ctx.globalAlpha = 1;
        }

        // Best bound line (max of all opt L*)
        var bestL = -Infinity;
        for (var i = 0; i < selected.length; i++) {
            if (selected[i].optL > bestL) bestL = selected[i].optL;
        }
        if (selected.length > 1) {
            ctx.setLineDash([5, 4]);
            ctx.strokeStyle = '#fbbf24';
            ctx.lineWidth = 1;
            ctx.globalAlpha = 0.5;
            var by = yPos(bestL);
            ctx.beginPath(); ctx.moveTo(margin.left, by); ctx.lineTo(w - margin.right, by); ctx.stroke();
            ctx.setLineDash([]);
            ctx.globalAlpha = 1;

            // Label
            ctx.font = '9px "JetBrains Mono", monospace';
            ctx.fillStyle = '#fbbf24';
            ctx.textAlign = 'left';
            ctx.textBaseline = 'bottom';
            ctx.fillText('best L* = ' + bestL.toFixed(2), margin.left + 4, by - 3);
        }

        // Hover crosshair
        if (hoverLambda !== null && hoverLambda >= minLam && hoverLambda <= maxLam) {
            var hx = xPos(hoverLambda);

            // Vertical line
            ctx.setLineDash([3, 3]);
            ctx.strokeStyle = textMuted;
            ctx.lineWidth = 0.5;
            ctx.beginPath(); ctx.moveTo(hx, margin.top); ctx.lineTo(hx, h - margin.bottom); ctx.stroke();
            ctx.setLineDash([]);

            // Values at hover
            var tooltipLines = ['\u03BB = ' + hoverLambda.toFixed(3)];
            for (var s = 0; s < selected.length; s++) {
                var val = interpolateL(selected[s].points, hoverLambda);
                if (val !== null) {
                    tooltipLines.push(selected[s].name + ': ' + val.toFixed(2));

                    // Dot on curve
                    ctx.fillStyle = selected[s].color;
                    ctx.beginPath();
                    ctx.arc(hx, yPos(val), 3, 0, Math.PI * 2);
                    ctx.fill();
                }
            }

            // Tooltip box
            ctx.font = '10px "JetBrains Mono", monospace';
            var tooltipW = 0;
            for (var i = 0; i < tooltipLines.length; i++) {
                var tw = ctx.measureText(tooltipLines[i]).width;
                if (tw > tooltipW) tooltipW = tw;
            }
            tooltipW += 16;
            var tooltipH = tooltipLines.length * 16 + 8;
            var tx = hx + 10;
            if (tx + tooltipW > w - margin.right) tx = hx - tooltipW - 10;
            var ty = margin.top + 8;

            ctx.fillStyle = bgCard;
            ctx.strokeStyle = border;
            ctx.lineWidth = 1;
            ctx.beginPath();
            ctx.roundRect(tx, ty, tooltipW, tooltipH, 4);
            ctx.fill(); ctx.stroke();

            ctx.textAlign = 'left';
            ctx.textBaseline = 'top';
            for (var i = 0; i < tooltipLines.length; i++) {
                ctx.fillStyle = i === 0 ? textMuted : (selected[i - 1] ? selected[i - 1].color : textMuted);
                ctx.fillText(tooltipLines[i], tx + 8, ty + 4 + i * 16);
            }
        }
    }

    function interpolateL(points, lambda) {
        if (points.length === 0) return null;
        if (lambda <= points[0].lambda) return points[0].L;
        if (lambda >= points[points.length - 1].lambda) return points[points.length - 1].L;
        for (var i = 1; i < points.length; i++) {
            if (points[i].lambda >= lambda) {
                var p0 = points[i - 1], p1 = points[i];
                var t = (lambda - p0.lambda) / (p1.lambda - p0.lambda);
                return p0.L + t * (p1.L - p0.L);
            }
        }
        return null;
    }

    function niceScale(min, max, targetTicks) {
        var range = max - min;
        if (range <= 0) return [min];
        var rough = range / targetTicks;
        var mag = Math.pow(10, Math.floor(Math.log10(rough)));
        var residual = rough / mag;
        var nice;
        if (residual <= 1.5) nice = mag;
        else if (residual <= 3) nice = 2 * mag;
        else if (residual <= 7) nice = 5 * mag;
        else nice = 10 * mag;

        var ticks = [];
        var start = Math.ceil(min / nice) * nice;
        for (var v = start; v <= max + nice * 0.01; v += nice) {
            ticks.push(+v.toFixed(10));
        }
        return ticks;
    }

    function formatTickVal(v) {
        if (Math.abs(v) >= 1000) return v.toExponential(0);
        if (Math.abs(v) < 0.01 && v !== 0) return v.toExponential(1);
        if (Number.isInteger(v)) return v.toString();
        return v.toFixed(2);
    }

    // Reset when model changes
    window.resetLagrangianPanel = function () {
        selected = [];
        colorIdx = 0;
        varMap = null;
        document.body.classList.remove('lr-panel-open');
        if (panel) {
            panel.classList.remove('open');
            pillsEl.innerHTML = '';
            infoEl.innerHTML = '';
        }
    };
})();

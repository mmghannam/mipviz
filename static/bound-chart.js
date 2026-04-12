// Bound Convergence Chart — dual/primal bound development over time for all solvers
(function () {
    'use strict';

    // Solver colors — distinct, accessible palette
    var SOLVER_COLORS = {
        scip_spx:  '#60a5fa', // blue
        scipc_cpx: '#a78bfa', // purple
        highs:     '#34d399', // green
        copt:      '#fb923c', // orange
        optverse:  '#f472b6', // pink
    };

    var canvas, ctx, wrap, legendEl, toggleBtn;
    var chartData = null; // { solverKey: [{t, dual, primal}] }
    var hiddenSolvers = new Set();
    var logScale = true;
    var hoverX = null;
    var bestObj = null;
    var objSense = 'minimize';

    // ── Log parsers per solver ──────────────────────────────────────────

    function parseSCIP(text) {
        // Format: " 182s|     1 |     0 | ... | 1.510000e+02 | 3.530000e+02 | ..."
        // Also handles heuristic prefix like "d 131s|..."
        var lines = text.split('\n');
        var points = [];
        var re = /^\s*[a-z*]?\s*(\d+(?:\.\d+)?)s\|.*\|\s*([\d.eE+\-]+)\s*\|\s*([\d.eE+\-]+|--)\s*\|/;
        for (var i = 0; i < lines.length; i++) {
            var m = lines[i].match(re);
            if (!m) continue;
            var t = parseFloat(m[1]);
            var dual = parseFloat(m[2]);
            var primal = m[3] === '--' ? null : parseFloat(m[3]);
            if (isNaN(t) || isNaN(dual)) continue;
            if (primal !== null && isNaN(primal)) primal = null;
            points.push({ t: t, dual: dual, primal: primal });
        }
        return points;
    }

    function parseHiGHS(text) {
        // HiGHS B&B table ends each data line with time like "138.7s"
        // Columns: Src Proc InQueue | Leaves Expl% | BestBound BestSol Gap | Cuts InLp Confl | LpIters Time
        // Gap can be "inf" (no solution) or "0.00%" etc.
        var lines = text.split('\n');
        var points = [];
        // Match any line ending with a time value like "138.7s"
        // and containing the B&B columns — we key off: Expl% ... BestBound ... BestSol/inf ... Time
        var re = /^\s*[A-Z ]?\s*\d+\s+\d+\s+\d+\s+[\d.]+%\s+([\d.eE+\-]+)\s+([\d.eE+\-]+|inf)\s+(?:[\d.]+%|inf)\s+.*?([\d.]+)s\s*$/;
        for (var i = 0; i < lines.length; i++) {
            var m = lines[i].match(re);
            if (!m) continue;
            var t = parseFloat(m[3]);
            var dual = parseFloat(m[1]);
            var primal = m[2] === 'inf' ? null : parseFloat(m[2]);
            if (isNaN(t) || isNaN(dual)) continue;
            if (primal !== null && isNaN(primal)) primal = null;
            points.push({ t: t, dual: dual, primal: primal });
        }
        return points;
    }

    function parseCOPT(text) {
        // Format: "         0         1      --       0  1.510000e+02            --     Inf  0.67s"
        // or: "H        0         1      --     174  1.510000e+02  9.060000e+02  83.33%  0.71s"
        // Columns: Nodes Active LPit/n IntInf BestBound BestSolution Gap Time
        var lines = text.split('\n');
        var points = [];
        var re = /^\s*[H ]?\s*\d+\s+\d+\s+\S+\s+\d+\s+([\d.eE+\-]+)\s+([\d.eE+\-]+|--)\s+(?:[\d.]+%|Inf)\s+([\d.]+)s/;
        for (var i = 0; i < lines.length; i++) {
            var m = lines[i].match(re);
            if (!m) continue;
            var t = parseFloat(m[3]);
            var dual = parseFloat(m[1]);
            var primal = m[2] === '--' ? null : parseFloat(m[2]);
            if (isNaN(t) || isNaN(dual)) continue;
            if (primal !== null && isNaN(primal)) primal = null;
            points.push({ t: t, dual: dual, primal: primal });
        }
        return points;
    }

    function parseOptverse(text) {
        // Format: " H   1.3s         0          0       --   1.235086e+02   3.530000e+02   65.01%"
        // or: "     0.5s         0          0       --   0.000000e+00        --          --"
        var lines = text.split('\n');
        var points = [];
        var re = /^\s*[H ]?\s*([\d.]+)s\s+\d+\s+\d+\s+\S+\s+([\d.eE+\-]+)\s+([\d.eE+\-]+|--)\s/;
        for (var i = 0; i < lines.length; i++) {
            var m = lines[i].match(re);
            if (!m) continue;
            var t = parseFloat(m[1]);
            var dual = parseFloat(m[2]);
            var primal = m[3] === '--' ? null : parseFloat(m[3]);
            if (isNaN(t) || isNaN(dual)) continue;
            if (primal !== null && isNaN(primal)) primal = null;
            points.push({ t: t, dual: dual, primal: primal });
        }
        return points;
    }

    var PARSERS = {
        scip_spx: parseSCIP,
        scipc_cpx: parseSCIP,
        highs: parseHiGHS,
        copt: parseCOPT,
        optverse: parseOptverse,
    };

    // ── Deduplicate & clean points ──────────────────────────────────────

    function dedup(points) {
        if (points.length === 0) return points;
        var out = [points[0]];
        for (var i = 1; i < points.length; i++) {
            var p = points[i], prev = out[out.length - 1];
            if (p.t === prev.t && p.dual === prev.dual && p.primal === prev.primal) continue;
            out.push(p);
        }
        return out;
    }

    // ── Public API ──────────────────────────────────────────────────────

    window.initBoundChart = function (containerEl, stats) {
        bestObj = stats && stats.best_obj != null ? stats.best_obj : null;
        objSense = stats && stats.obj_sense === 'maximize' ? 'maximize' : 'minimize';
        chartData = {};

        wrap = document.createElement('div');
        wrap.className = 'bound-chart-section';
        wrap.innerHTML =
            '<div class="bound-chart-header">' +
                '<h3 class="bound-chart-title">Bound Convergence</h3>' +
                '<button class="bound-chart-toggle active" id="bound-chart-toggle">Log scale</button>' +
            '</div>' +
            '<div class="bound-chart-legend" id="bound-chart-legend"></div>' +
            '<div class="bound-chart-wrap"><canvas id="bound-chart-canvas"></canvas></div>';
        containerEl.appendChild(wrap);

        canvas = document.getElementById('bound-chart-canvas');
        ctx = canvas.getContext('2d');
        legendEl = document.getElementById('bound-chart-legend');
        toggleBtn = document.getElementById('bound-chart-toggle');

        toggleBtn.addEventListener('click', function () {
            logScale = !logScale;
            toggleBtn.classList.toggle('active', logScale);
            drawChart();
        });

        canvas.addEventListener('mousemove', onHover);
        canvas.addEventListener('mouseleave', function () {
            hoverX = null;
            drawChart();
        });

        window.addEventListener('resize', function () {
            if (chartData && Object.keys(chartData).length) drawChart();
        });
    };

    window.addBoundSeries = function (solver, logText) {
        if (!chartData) return;
        var parser = PARSERS[solver];
        if (!parser) return;
        var points = dedup(parser(logText));
        if (points.length === 0) return;
        chartData[solver] = points;
        buildLegend();
        drawChart();
    };

    // ── Legend ───────────────────────────────────────────────────────────

    function buildLegend() {
        if (!legendEl) return;
        var LABELS = window.SOLVER_LABELS || {};
        legendEl.innerHTML = '';
        var solvers = Object.keys(chartData);
        for (var i = 0; i < solvers.length; i++) {
            (function (solver) {
                var color = SOLVER_COLORS[solver] || '#888';
                var label = LABELS[solver] || solver;
                var isHidden = hiddenSolvers.has(solver);
                var item = document.createElement('span');
                item.className = 'bound-legend-item bound-legend-toggle' + (isHidden ? ' bound-legend-hidden' : '');
                item.innerHTML =
                    '<span class="bound-legend-swatch" style="background:' + color + '"></span>' +
                    '<span class="bound-legend-line-solid" style="background:' + color + '"></span>' +
                    '<span class="bound-legend-label">' + label + '</span>';
                item.addEventListener('click', function () {
                    if (hiddenSolvers.has(solver)) hiddenSolvers.delete(solver);
                    else hiddenSolvers.add(solver);
                    buildLegend();
                    drawChart();
                });
                legendEl.appendChild(item);
            })(solvers[i]);
        }
        var html = '';
        html +=
            '<span class="bound-legend-item bound-legend-key">' +
                '<span class="bound-legend-line-solid" style="background:var(--text-muted)"></span>' +
                '<span class="bound-legend-label" style="opacity:0.6">solid = primal</span>' +
            '</span>' +
            '<span class="bound-legend-item bound-legend-key">' +
                '<span class="bound-legend-line-dashed" style="background:var(--text-muted)"></span>' +
                '<span class="bound-legend-label" style="opacity:0.6">dashed = dual</span>' +
            '</span>';
        if (bestObj !== null) {
            html +=
                '<span class="bound-legend-item bound-legend-key">' +
                    '<span class="bound-legend-line-dotted"></span>' +
                    '<span class="bound-legend-label" style="opacity:0.6">optimal = ' + fmtNum(bestObj) + '</span>' +
                '</span>';
        }
        legendEl.insertAdjacentHTML('beforeend', html);
    }

    // ── Hover ───────────────────────────────────────────────────────────

    function onHover(e) {
        var rect = canvas.getBoundingClientRect();
        hoverX = e.clientX - rect.left;
        drawChart();
    }

    // ── Drawing ─────────────────────────────────────────────────────────

    function drawChart() {
        if (!canvas || !chartData) return;
        var allSolvers = Object.keys(chartData);
        if (allSolvers.length === 0) return;
        var solvers = allSolvers.filter(function (s) { return !hiddenSolvers.has(s); });
        if (solvers.length === 0) { ctx.clearRect(0, 0, canvas.width, canvas.height); return; }

        var dpr = window.devicePixelRatio || 1;
        var dispW = canvas.parentElement.clientWidth;
        var dispH = 340;
        canvas.style.width = dispW + 'px';
        canvas.style.height = dispH + 'px';
        canvas.width = dispW * dpr;
        canvas.height = dispH * dpr;
        ctx.setTransform(dpr, 0, 0, dpr, 0, 0);

        var styles = getComputedStyle(document.documentElement);
        var textMuted = styles.getPropertyValue('--text-muted').trim();
        var textSecondary = styles.getPropertyValue('--text-secondary').trim();
        var borderSubtle = styles.getPropertyValue('--border-subtle').trim();
        var bgCard = styles.getPropertyValue('--bg-card').trim();

        var margin = { top: 20, right: 24, bottom: 52, left: 64 };
        var w = dispW, h = dispH;
        var plotW = w - margin.left - margin.right;
        var plotH = h - margin.top - margin.bottom;

        // Compute data ranges
        var tMin = Infinity, tMax = -Infinity;
        var vMin = Infinity, vMax = -Infinity;
        for (var si = 0; si < solvers.length; si++) {
            var pts = chartData[solvers[si]];
            for (var j = 0; j < pts.length; j++) {
                var p = pts[j];
                if (p.t < tMin) tMin = p.t;
                if (p.t > tMax) tMax = p.t;
                if (p.dual < vMin) vMin = p.dual;
                if (p.dual > vMax) vMax = p.dual;
                if (p.primal !== null) {
                    if (p.primal < vMin) vMin = p.primal;
                    if (p.primal > vMax) vMax = p.primal;
                }
            }
        }
        if (bestObj !== null) {
            if (bestObj < vMin) vMin = bestObj;
            if (bestObj > vMax) vMax = bestObj;
        }

        // Smart Y-axis clamping: focus on the convergence region.
        // Use the final dual/primal values to find the "settled" range,
        // then extend moderately to show early convergence trajectory.
        if (bestObj !== null) {
            // Find the range between worst final dual and best known obj
            var worstDual = Infinity;
            for (var si = 0; si < solvers.length; si++) {
                var pts = chartData[solvers[si]];
                if (pts.length > 0) {
                    var d0 = pts[0].dual;
                    if (d0 < worstDual) worstDual = d0;
                }
            }
            var coreSpan = Math.abs(bestObj - worstDual) || Math.abs(bestObj) || 1;
            if (objSense === 'minimize') {
                vMin = Math.min(vMin, worstDual - coreSpan * 0.1);
                vMax = bestObj + coreSpan * 0.8;
            } else {
                vMax = Math.max(vMax, worstDual + coreSpan * 0.1);
                vMin = bestObj - coreSpan * 0.8;
            }
        }

        if (tMin === tMax) tMax = tMin + 1;
        var vPad = (vMax - vMin) * 0.08 || 1;
        vMin -= vPad;
        vMax += vPad;

        // For log scale, ensure tMin > 0
        var tMinLog = logScale ? Math.max(tMin, 0.1) : tMin;
        if (logScale && tMinLog >= tMax) tMinLog = tMax / 10;

        function xPos(t) {
            if (logScale) {
                var lt = Math.log10(Math.max(t, tMinLog));
                var l0 = Math.log10(tMinLog);
                var l1 = Math.log10(Math.max(tMax, tMinLog + 0.1));
                return margin.left + (lt - l0) / (l1 - l0) * plotW;
            }
            return margin.left + (t - tMin) / (tMax - tMin) * plotW;
        }
        function yPos(v) {
            var clamped = Math.max(vMin, Math.min(vMax, v));
            return margin.top + (1 - (clamped - vMin) / (vMax - vMin)) * plotH;
        }
        function xToTime(px) {
            var frac = (px - margin.left) / plotW;
            if (frac < 0 || frac > 1) return null;
            if (logScale) {
                var l0 = Math.log10(tMinLog);
                var l1 = Math.log10(Math.max(tMax, tMinLog + 0.1));
                return Math.pow(10, l0 + frac * (l1 - l0));
            }
            return tMin + frac * (tMax - tMin);
        }

        // Clear
        ctx.clearRect(0, 0, w, h);

        // Grid lines
        ctx.strokeStyle = borderSubtle;
        ctx.lineWidth = 0.5;
        var yTicks = niceScale(vMin, vMax, 6);
        for (var i = 0; i < yTicks.length; i++) {
            var y = yPos(yTicks[i]);
            ctx.beginPath(); ctx.moveTo(margin.left, y); ctx.lineTo(w - margin.right, y); ctx.stroke();
        }
        var xTicks = logScale ? logTicks(tMinLog, tMax) : niceScale(tMin, tMax, 7);
        // Always show the max time at the right edge (avoid overlap with last tick)
        var lastTick = xTicks.length > 0 ? xTicks[xTicks.length - 1] : -Infinity;
        if (tMax > lastTick * 1.02) xTicks.push(tMax);
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
            ctx.fillText(fmtNum(yTicks[i]), margin.left - 6, yPos(yTicks[i]));
        }
        ctx.textAlign = 'center';
        ctx.textBaseline = 'top';
        for (var i = 0; i < xTicks.length; i++) {
            ctx.fillText(fmtTime(xTicks[i]), xPos(xTicks[i]), h - margin.bottom + 6);
        }

        // Axis titles
        ctx.fillStyle = textMuted;
        ctx.textAlign = 'center';
        ctx.fillText('time' + (logScale ? ' (log)' : ''), margin.left + plotW / 2, h - 4);
        ctx.save();
        ctx.translate(12, margin.top + plotH / 2);
        ctx.rotate(-Math.PI / 2);
        ctx.textAlign = 'center';
        ctx.fillText('objective', 0, 0);
        ctx.restore();

        // Best known objective line
        if (bestObj !== null && bestObj >= vMin && bestObj <= vMax) {
            var optY = yPos(bestObj);
            ctx.setLineDash([2, 3]);
            ctx.strokeStyle = '#fbbf24';
            ctx.lineWidth = 1;
            ctx.globalAlpha = 0.7;
            ctx.beginPath(); ctx.moveTo(margin.left, optY); ctx.lineTo(w - margin.right, optY); ctx.stroke();
            ctx.setLineDash([]);
            ctx.globalAlpha = 1;
        }

        // Draw solver curves — filled gap band + thin lines
        for (var si = 0; si < solvers.length; si++) {
            var solver = solvers[si];
            var pts = chartData[solver];
            var color = SOLVER_COLORS[solver] || '#888';
            if (pts.length === 0) continue;

            // Build step arrays for dual and primal so we can fill between them
            // Each entry: {t, dual, primal} where primal carries forward last known value
            var steps = [];
            var lastPrimal = null;
            for (var j = 0; j < pts.length; j++) {
                if (pts[j].primal !== null) lastPrimal = pts[j].primal;
                steps.push({ t: pts[j].t, dual: pts[j].dual, primal: lastPrimal });
            }

            // Fill the gap region between primal and dual bounds
            // Build paired arrays of steps that have both dual and primal
            var pairedSteps = [];
            for (var j = 0; j < steps.length; j++) {
                if (steps[j].primal !== null) pairedSteps.push(steps[j]);
            }
            if (pairedSteps.length > 0) {
                ctx.fillStyle = color;
                ctx.globalAlpha = 0.10;
                ctx.beginPath();
                // Forward path along primal (step function)
                ctx.moveTo(xPos(pairedSteps[0].t), yPos(pairedSteps[0].primal));
                for (var j = 1; j < pairedSteps.length; j++) {
                    ctx.lineTo(xPos(pairedSteps[j].t), yPos(pairedSteps[j - 1].primal));
                    ctx.lineTo(xPos(pairedSteps[j].t), yPos(pairedSteps[j].primal));
                }
                // Reverse path along dual (step function)
                var last = pairedSteps[pairedSteps.length - 1];
                ctx.lineTo(xPos(last.t), yPos(last.dual));
                for (var j = pairedSteps.length - 2; j >= 0; j--) {
                    ctx.lineTo(xPos(pairedSteps[j + 1].t), yPos(pairedSteps[j].dual));
                    ctx.lineTo(xPos(pairedSteps[j].t), yPos(pairedSteps[j].dual));
                }
                ctx.closePath();
                ctx.fill();
                ctx.globalAlpha = 1;
            }

            // Dual bound — dashed line
            ctx.strokeStyle = color;
            ctx.lineWidth = 1.5;
            ctx.setLineDash([6, 4]);
            ctx.beginPath();
            ctx.moveTo(xPos(pts[0].t), yPos(pts[0].dual));
            for (var j = 1; j < pts.length; j++) {
                var x = xPos(pts[j].t);
                ctx.lineTo(x, yPos(pts[j - 1].dual));
                ctx.lineTo(x, yPos(pts[j].dual));
            }
            ctx.stroke();
            ctx.setLineDash([]);

            // Primal bound — thin solid line
            ctx.strokeStyle = color;
            ctx.lineWidth = 1.5;
            ctx.beginPath();
            var started = false;
            var lastPrimalY = null;
            for (var j = 0; j < pts.length; j++) {
                if (pts[j].primal === null) continue;
                var x = xPos(pts[j].t);
                var y = yPos(pts[j].primal);
                if (!started) { ctx.moveTo(x, y); started = true; }
                else {
                    ctx.lineTo(x, lastPrimalY);
                    ctx.lineTo(x, y);
                }
                lastPrimalY = y;
            }
            ctx.stroke();
        }

        // Hover crosshair + tooltip
        if (hoverX !== null) {
            var hoverTime = xToTime(hoverX);
            if (hoverTime !== null) {
                var hx = xPos(hoverTime);
                // Vertical line
                ctx.setLineDash([3, 3]);
                ctx.strokeStyle = textMuted;
                ctx.lineWidth = 0.5;
                ctx.beginPath(); ctx.moveTo(hx, margin.top); ctx.lineTo(hx, h - margin.bottom); ctx.stroke();
                ctx.setLineDash([]);

                // Collect values at hoverTime
                var LABELS = window.SOLVER_LABELS || {};
                var tooltipLines = ['t = ' + fmtTime(hoverTime)];
                var tooltipColors = [textMuted];
                for (var si = 0; si < solvers.length; si++) {
                    var solver = solvers[si];
                    var pts = chartData[solver];
                    var color = SOLVER_COLORS[solver] || '#888';
                    var val = interpolateStep(pts, hoverTime);
                    if (!val) continue;
                    var label = LABELS[solver] || solver;
                    var dualStr = fmtNum(val.dual);
                    var primalStr = val.primal !== null ? fmtNum(val.primal) : '--';
                    tooltipLines.push(label + ': ' + dualStr + ' / ' + primalStr);
                    tooltipColors.push(color);

                    // Dots on curves
                    ctx.fillStyle = color;
                    ctx.beginPath();
                    ctx.arc(hx, yPos(val.dual), 3, 0, Math.PI * 2);
                    ctx.fill();
                    if (val.primal !== null) {
                        ctx.beginPath();
                        ctx.arc(hx, yPos(val.primal), 3, 0, Math.PI * 2);
                        ctx.fill();
                    }
                }

                // Tooltip box
                ctx.font = '10px "JetBrains Mono", monospace';
                var tw = 0;
                for (var i = 0; i < tooltipLines.length; i++) {
                    var m = ctx.measureText(tooltipLines[i]).width;
                    if (m > tw) tw = m;
                }
                tw += 16;
                var th = tooltipLines.length * 16 + 8;
                var tx = hx + 12;
                if (tx + tw > w - margin.right) tx = hx - tw - 12;
                var ty = margin.top + 8;

                ctx.fillStyle = bgCard;
                ctx.strokeStyle = borderSubtle;
                ctx.lineWidth = 1;
                ctx.globalAlpha = 0.92;
                ctx.beginPath();
                ctx.roundRect(tx, ty, tw, th, 4);
                ctx.fill(); ctx.stroke();
                ctx.globalAlpha = 1;

                ctx.textAlign = 'left';
                ctx.textBaseline = 'top';
                for (var i = 0; i < tooltipLines.length; i++) {
                    ctx.fillStyle = tooltipColors[i];
                    ctx.fillText(tooltipLines[i], tx + 8, ty + 4 + i * 16);
                }
            }
        }

        // Plot border
        ctx.strokeStyle = borderSubtle;
        ctx.lineWidth = 1;
        ctx.strokeRect(margin.left, margin.top, plotW, plotH);
    }

    // ── Step interpolation: find dual/primal at a given time ────────────

    function interpolateStep(points, t) {
        if (!points || points.length === 0) return null;
        if (t < points[0].t) return null;
        var dual = points[0].dual;
        var primal = points[0].primal;
        for (var i = 1; i < points.length; i++) {
            if (points[i].t > t) break;
            dual = points[i].dual;
            if (points[i].primal !== null) primal = points[i].primal;
        }
        return { dual: dual, primal: primal };
    }

    // ── Formatting helpers ──────────────────────────────────────────────

    function fmtNum(v) {
        if (Math.abs(v) >= 1e6) return v.toExponential(2);
        if (Math.abs(v) >= 1000) return v.toFixed(0);
        if (Math.abs(v) >= 1) return v.toFixed(2);
        if (Math.abs(v) >= 0.01) return v.toFixed(4);
        if (v === 0) return '0';
        return v.toExponential(2);
    }

    function fmtTime(t) {
        if (t >= 3600) return (t / 3600).toFixed(1) + 'h';
        if (t >= 60) return (t / 60).toFixed(1) + 'm';
        return t.toFixed(1) + 's';
    }

    // ── Scale helpers ───────────────────────────────────────────────────

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

    function logTicks(min, max) {
        var ticks = [];
        var logMin = Math.floor(Math.log10(Math.max(min, 0.01)));
        var logMax = Math.ceil(Math.log10(Math.max(max, 1)));
        for (var p = logMin; p <= logMax; p++) {
            var v = Math.pow(10, p);
            if (v >= min * 0.99 && v <= max * 1.01) ticks.push(v);
            // Add intermediate ticks for wider ranges
            if (logMax - logMin <= 3) {
                var v2 = 2 * Math.pow(10, p);
                var v5 = 5 * Math.pow(10, p);
                if (v2 >= min * 0.99 && v2 <= max * 1.01) ticks.push(v2);
                if (v5 >= min * 0.99 && v5 <= max * 1.01) ticks.push(v5);
            }
        }
        ticks.sort(function (a, b) { return a - b; });
        return ticks;
    }
})();

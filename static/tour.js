(function() {
    var btn = document.getElementById('tour-btn');
    var btnResults = document.getElementById('tour-btn-results');
    if (btn) btn.addEventListener('click', startTour);
    if (btnResults) btnResults.addEventListener('click', startTour);
    if (!btn && !btnResults) return;

    document.addEventListener('keydown', function(e) {
        if (e.key === 't' && !e.ctrlKey && !e.metaKey && !e.altKey) {
            var tag = document.activeElement && document.activeElement.tagName;
            if (tag === 'INPUT' || tag === 'TEXTAREA' || tag === 'SELECT') return;
            startTour();
        }
    });

    function startTour() {
        var resultsVisible = !document.getElementById('results-section').classList.contains('hidden');

        var tour = new Shepherd.Tour({
            useModalOverlay: true,
            defaultStepOptions: {
                scrollTo: { behavior: 'smooth', block: 'center' },
                cancelIcon: { enabled: true },
                classes: 'mipviz-tour-step'
            }
        });

        if (!resultsVisible) {
            // Homepage tour
            tour.addStep({
                id: 'welcome',
                text: 'Welcome to <strong>mipviz</strong>! This tool lets you explore the structure of Mixed-Integer Programming instances. Let\'s take a quick look around.',
                buttons: [{ text: 'Next', action: tour.next }]
            });

            tour.addStep({
                id: 'upload',
                attachTo: { element: '#drop-zone', on: 'bottom' },
                text: 'Drop an <strong>.mps</strong> or <strong>.lp</strong> file here to load a model. Gzipped files are supported too.',
                buttons: [
                    { text: 'Back', action: tour.back, secondary: true },
                    { text: 'Next', action: tour.next }
                ]
            });

            tour.addStep({
                id: 'random',
                attachTo: { element: '#try-random-btn', on: 'bottom' },
                text: 'No file handy? Load a random instance from MIPLIB to explore.',
                buttons: [
                    { text: 'Back', action: tour.back, secondary: true },
                    { text: 'Next', action: tour.next }
                ]
            });

            tour.addStep({
                id: 'search',
                attachTo: { element: '#nav-search', on: 'bottom' },
                text: 'Search for specific MIPLIB instances by name. Press <kbd>s</kbd> to focus this field anytime.',
                buttons: [
                    { text: 'Back', action: tour.back, secondary: true },
                    { text: 'Next', action: tour.next }
                ]
            });

            tour.addStep({
                id: 'instance-bank',
                attachTo: { element: 'a[href="./instances.html"]', on: 'bottom' },
                text: 'Browse the full collection of MIPLIB instances with filtering and metadata.',
                buttons: [
                    { text: 'Back', action: tour.back, secondary: true },
                    { text: 'Next', action: tour.next }
                ]
            });

            tour.addStep({
                id: 'end-homepage',
                text: 'That\'s the overview! Load an instance to see the full analysis tour, or explore on your own.',
                buttons: [
                    { text: 'Back', action: tour.back, secondary: true },
                    { text: 'Load an instance', action: function() { tour.complete(); loadRandomAndTour(); } },
                    { text: 'Done', action: tour.complete }
                ]
            });
        } else {
            addResultsSteps(tour);
        }

        tour.start();
    }

    function loadRandomAndTour() {
        // Load flugpl — a small, well-structured instance good for demos
        if (typeof loadInstanceFromUrl === 'function') {
            loadInstanceFromUrl('flugpl');
        }
        // Wait for model to load, then start results tour
        var check = setInterval(function() {
            if (!document.getElementById('results-section').classList.contains('hidden')) {
                clearInterval(check);
                setTimeout(function() { startResultsTour(); }, 400);
            }
        }, 300);
        // Timeout after 15s
        setTimeout(function() { clearInterval(check); }, 15000);
    }

    function startResultsTour() {
        var tour = new Shepherd.Tour({
            useModalOverlay: true,
            defaultStepOptions: {
                scrollTo: { behavior: 'smooth', block: 'center' },
                cancelIcon: { enabled: true },
                classes: 'mipviz-tour-step'
            }
        });
        addResultsSteps(tour);
        tour.start();
    }

    function addResultsSteps(tour) {
        tour.addStep({
            id: 'stats',
            attachTo: { element: '#stats-grid', on: 'bottom' },
            text: 'Key statistics at a glance: number of variables, constraints, nonzeros, and variable types.',
            buttons: [{ text: 'Next', action: tour.next }]
        });

        tour.addStep({
            id: 'legend',
            attachTo: { element: '.legend', on: 'bottom' },
            text: 'Variables are color-coded by type throughout the tool: <span style="color:var(--orange)">continuous</span>, <span style="color:var(--green)">integer</span>, and <span style="color:var(--accent)">binary</span>.',
            buttons: [
                { text: 'Back', action: tour.back, secondary: true },
                { text: 'Next', action: tour.next }
            ]
        });

        tour.addStep({
            id: 'constraint-types',
            attachTo: { element: '#constraint-types', on: 'bottom' },
            text: 'Constraints are classified by structure (set packing, knapsack, etc.). Click a tag to filter the constraint list. The <strong>?</strong> button links to definitions.',
            buttons: [
                { text: 'Back', action: tour.back, secondary: true },
                { text: 'Next', action: tour.next }
            ]
        });

        tour.addStep({
            id: 'ranges',
            attachTo: { element: '#model-ranges', on: 'bottom' },
            text: 'Coefficient ranges show the spread of values in bounds, the objective, the constraint matrix, and the RHS.',
            buttons: [
                { text: 'Back', action: tour.back, secondary: true },
                { text: 'Next', action: tour.next }
            ]
        });

        tour.addStep({
            id: 'sparsity',
            attachTo: { element: '#sparsity-canvas', on: 'bottom' },
            text: 'The sparsity pattern shows where nonzeros appear in the constraint matrix. Connected components are detected automatically.',
            beforeShowPromise: function() {
                var details = document.querySelector('#sparsity-canvas').closest('details');
                if (details && !details.open) details.open = true;
                return Promise.resolve();
            },
            buttons: [
                { text: 'Back', action: tour.back, secondary: true },
                { text: 'Next', action: tour.next }
            ]
        });

        tour.addStep({
            id: 'objective',
            attachTo: { element: '#objective', on: 'bottom' },
            text: 'The objective function with color-coded variables. Terms with the same coefficient are compressed into <strong>\u03A3</strong> groups showing the type breakdown — click a group to expand it.',
            beforeShowPromise: function() {
                var details = document.querySelector('#objective').closest('details');
                if (details && !details.open) details.open = true;
                return Promise.resolve();
            },
            buttons: [
                { text: 'Back', action: tour.back, secondary: true },
                { text: 'Next', action: tour.next }
            ]
        });

        tour.addStep({
            id: 'var-axis',
            attachTo: { element: '#var-axis', on: 'bottom' },
            text: 'Variables are displayed on a shared <strong>number line</strong>. The axis shows the global bounds range across all variables.',
            beforeShowPromise: function() {
                var details = document.querySelector('#variables-list').closest('details');
                if (details && !details.open) details.open = true;
                return Promise.resolve();
            },
            buttons: [
                { text: 'Back', action: tour.back, secondary: true },
                { text: 'Next', action: tour.next }
            ]
        });

        tour.addStep({
            id: 'variables',
            attachTo: { element: '#variables-list', on: 'top' },
            text: 'Each variable is a bar on the number line. Bar <strong>position and length</strong> encode bounds, <strong>color</strong> encodes type, and <strong>thickness</strong> encodes the objective coefficient. Arrows indicate infinite bounds. Hover a bar to see its bounds, or hover a name for full details.',
            buttons: [
                { text: 'Back', action: tour.back, secondary: true },
                { text: 'Next', action: tour.next }
            ]
        });

        tour.addStep({
            id: 'var-toolbar',
            attachTo: { element: '#variables-sort-bar', on: 'bottom' },
            text: '<strong>Compress</strong> groups variables with identical bounds, type, and obj coefficient — click a group to expand. <strong>Sort</strong> by id, name, lower/upper bound, range, or objective coefficient. Click a sort button twice to reverse.',
            buttons: [
                { text: 'Back', action: tour.back, secondary: true },
                { text: 'Next', action: tour.next }
            ]
        });

        tour.addStep({
            id: 'var-click',
            attachTo: { element: '#variables-list', on: 'top' },
            text: 'Click a variable name (<strong>x0</strong>, <strong>x1</strong>, ...) to filter the constraints list to only those containing that variable — the same as clicking a variable in any constraint.',
            buttons: [
                { text: 'Back', action: tour.back, secondary: true },
                { text: 'Next', action: tour.next }
            ]
        });

        tour.addStep({
            id: 'constraints',
            attachTo: { element: '#constraints-list', on: 'top' },
            text: 'All constraints with color-coded variables. Hover a variable to highlight it everywhere. Click to filter. Each constraint shows its type tags.',
            beforeShowPromise: function() {
                var details = document.querySelector('#constraints-list').closest('details');
                if (details && !details.open) details.open = true;
                return Promise.resolve();
            },
            buttons: [
                { text: 'Back', action: tour.back, secondary: true },
                { text: 'Next', action: tour.next }
            ]
        });

        tour.addStep({
            id: 'solver-picker',
            attachTo: { element: '#solver-picker', on: 'bottom' },
            text: 'Choose between <strong>SCIP</strong> and <strong>HiGHS</strong> solvers. Both run entirely in your browser via WebAssembly.',
            buttons: [
                { text: 'Back', action: tour.back, secondary: true },
                { text: 'Next', action: tour.next }
            ]
        });

        tour.addStep({
            id: 'solve-buttons',
            attachTo: { element: '#solve-lp-btn', on: 'bottom' },
            text: '<strong>Solve LP</strong> computes the LP relaxation. Solution values appear as markers on the variable bars, with <span style="color:#f59e0b">fractional</span> values highlighted.',
            buttons: [
                { text: 'Back', action: tour.back, secondary: true },
                { text: 'Next', action: tour.next }
            ]
        });

        tour.addStep({
            id: 'matrix',
            attachTo: { element: '#matrix-btn', on: 'bottom' },
            text: '<strong>Matrix</strong> opens a detailed view of the constraint matrix. It also shows an <strong>animated visualization of presolve</strong> transformations step by step — you can watch rows and columns get eliminated in real time.',
            beforeShowPromise: function() {
                var el = document.getElementById('matrix-btn');
                return el && !el.classList.contains('hidden') ? Promise.resolve() : Promise.resolve();
            },
            buttons: [
                { text: 'Back', action: tour.back, secondary: true },
                { text: 'Next', action: tour.next }
            ]
        });

        tour.addStep({
            id: 'toolbar',
            attachTo: { element: '#presolve-btn', on: 'bottom' },
            text: '<strong>Presolve</strong> simplifies the model and updates all views. <strong>Solve MIP</strong> runs a full solve. All solvers run entirely in your browser via WebAssembly.',
            buttons: [
                { text: 'Back', action: tour.back, secondary: true },
                { text: 'Done', action: tour.complete }
            ]
        });
    }
})();

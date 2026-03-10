// Shared theme handler. Cycles: auto → light → dark → auto.
// "auto" means follow system preference (no localStorage entry).
(function () {
    var media = window.matchMedia('(prefers-color-scheme: light)');

    function getMode() {
        var saved = localStorage.getItem('theme');
        if (saved === 'light' || saved === 'dark') return saved;
        return 'auto';
    }

    function applyMode(mode) {
        var isLight = mode === 'light' || (mode === 'auto' && media.matches);
        document.documentElement.classList.toggle('light', isLight);
        document.documentElement.dataset.themeMode = mode;
        if (typeof window.onThemeChange === 'function') {
            window.onThemeChange(isLight);
        }
    }

    // Apply on load
    applyMode(getMode());

    // React to system changes when in auto mode
    media.addEventListener('change', function () {
        if (getMode() === 'auto') applyMode('auto');
    });

    // Wire up toggle button (cycles through auto → light → dark → auto)
    document.addEventListener('DOMContentLoaded', function () {
        var btn = document.getElementById('theme-toggle');
        if (!btn) return;
        btn.addEventListener('click', function () {
            var current = getMode();
            var next = current === 'auto' ? 'light' : current === 'light' ? 'dark' : 'auto';
            if (next === 'auto') {
                localStorage.removeItem('theme');
            } else {
                localStorage.setItem('theme', next);
            }
            applyMode(next);
        });
    });
})();

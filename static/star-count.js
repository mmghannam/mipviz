// Populate the header "Star" button with the live GitHub star count.
(function () {
    function init() {
        var el = document.getElementById('star-count');
        if (!el) return;
        fetch('https://api.github.com/repos/mmghannam/mipviz')
            .then(function (r) { return r.ok ? r.json() : null; })
            .then(function (d) {
                if (!d || typeof d.stargazers_count !== 'number') return;
                var n = d.stargazers_count;
                el.textContent = n >= 1000 ? (n / 1000).toFixed(1).replace(/\.0$/, '') + 'k' : String(n);
            })
            .catch(function () {});
    }
    if (document.readyState === 'loading') {
        document.addEventListener('DOMContentLoaded', init);
    } else {
        init();
    }
})();

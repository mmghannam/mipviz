(function() {
    const search = document.getElementById('guide-search');
    const entries = Array.from(document.querySelectorAll('.guide-entry'));
    const status = document.getElementById('guide-status');
    function filter() {
        const words = search.value.toLowerCase().trim().split(/\s+/).filter(Boolean);
        let count = 0;
        entries.forEach(function(entry) {
            const text = (entry.textContent + ' ' + entry.dataset.keywords).toLowerCase();
            const matches = words.every(word => text.includes(word));
            entry.hidden = !matches;
            if (matches) count++;
            if (words.length) entry.open = matches;
            else entry.open = false;
        });
        document.querySelectorAll('.guide-section').forEach(function(section) {
            section.hidden = !Array.from(section.querySelectorAll('.guide-entry')).some(entry => !entry.hidden);
        });
        status.textContent = words.length ? count + ' matching feature' + (count === 1 ? '' : 's') : entries.length + ' features · click a title to learn more';
        document.getElementById('guide-empty').hidden = count !== 0;
    }
    search.addEventListener('input', filter);
    filter();
    const target = document.getElementById(location.hash.slice(1));
    if (target && target.classList.contains('guide-entry')) target.open = true;
})();

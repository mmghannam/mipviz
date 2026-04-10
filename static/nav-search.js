// Navbar instance search — shared across all pages
(function() {
    const searchInput = document.getElementById('nav-search');
    const dropdown = document.getElementById('nav-search-dropdown');
    if (!searchInput || !dropdown) return;

    let instanceNames = [];
    let selectedIndex = -1;
    const MAX_SHOWN = 20;

    const statsUrl = MIPVIZ_INSTANCES_BASE + 'instance-stats.json';

    fetch(statsUrl)
        .then(res => res.json())
        .then(stats => { instanceNames = stats.map(s => s.name); })
        .catch(() => {});

    searchInput.addEventListener('input', () => {
        const q = searchInput.value.toLowerCase();
        selectedIndex = -1;
        if (!q) {
            dropdown.innerHTML = '';
            dropdown.classList.remove('visible');
            return;
        }
        const filtered = instanceNames.filter(n => n.toLowerCase().includes(q));
        renderDropdown(filtered);
    });

    searchInput.addEventListener('keydown', (e) => {
        const items = dropdown.querySelectorAll('.nav-search-item[data-name]');
        if (!items.length) return;

        if (e.key === 'ArrowDown') {
            e.preventDefault();
            selectedIndex = Math.min(selectedIndex + 1, items.length - 1);
            updateSelection(items);
        } else if (e.key === 'ArrowUp') {
            e.preventDefault();
            selectedIndex = Math.max(selectedIndex - 1, 0);
            updateSelection(items);
        } else if (e.key === 'Enter') {
            e.preventDefault();
            const idx = selectedIndex >= 0 ? selectedIndex : 0;
            goToInstance(items[idx].dataset.name);
        }
    });

    searchInput.addEventListener('focus', () => {
        if (searchInput.value) {
            const q = searchInput.value.toLowerCase();
            const filtered = instanceNames.filter(n => n.toLowerCase().includes(q));
            renderDropdown(filtered);
        }
    });

    document.addEventListener('click', (e) => {
        if (!e.target.closest('.nav-search-wrap')) {
            dropdown.innerHTML = '';
            dropdown.classList.remove('visible');
        }
    });

    document.addEventListener('keydown', (e) => {
        if (e.key === 's' && !e.ctrlKey && !e.metaKey && !e.altKey
            && document.activeElement.tagName !== 'INPUT'
            && document.activeElement.tagName !== 'TEXTAREA') {
            e.preventDefault();
            searchInput.focus();
        } else if (e.key === 'Escape' && document.activeElement === searchInput) {
            searchInput.blur();
            dropdown.innerHTML = '';
            dropdown.classList.remove('visible');
        }
    });

    function goToInstance(name) {
        searchInput.value = '';
        searchInput.blur();
        dropdown.innerHTML = '';
        dropdown.classList.remove('visible');
        selectedIndex = -1;
        var hash = '#instance=' + encodeURIComponent(name);
        if (window.location.hash === hash) {
            // Same hash — hashchange won't fire, dispatch manually
            window.dispatchEvent(new HashChangeEvent('hashchange'));
        } else {
            window.location.hash = hash;
        }
    }

    function updateSelection(items) {
        items.forEach((el, i) => el.classList.toggle('selected', i === selectedIndex));
        if (selectedIndex >= 0) items[selectedIndex].scrollIntoView({ block: 'nearest' });
    }

    function renderDropdown(names) {
        const shown = names.slice(0, MAX_SHOWN);
        dropdown.innerHTML = '';
        if (!shown.length) {
            dropdown.classList.remove('visible');
            return;
        }
        for (const name of shown) {
            const div = document.createElement('div');
            div.className = 'nav-search-item';
            div.dataset.name = name;
            div.textContent = name;
            div.addEventListener('click', () => goToInstance(name));
            dropdown.appendChild(div);
        }
        if (names.length > MAX_SHOWN) {
            const more = document.createElement('div');
            more.className = 'nav-search-item';
            more.style.color = 'var(--text-muted)';
            more.style.cursor = 'default';
            more.textContent = `… and ${names.length - MAX_SHOWN} more`;
            dropdown.appendChild(more);
        }
        dropdown.classList.add('visible');
    }
})();

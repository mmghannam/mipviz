// dirpicker.js — Local directory picker + bundled instance bank for WASM mode
// Uses the File System Access API (Chromium browsers) for local dirs

const DirPicker = (() => {
    let dirHandle = null;
    let files = []; // Array of { name, handle }

    async function openDirectory() {
        dirHandle = await window.showDirectoryPicker({ mode: 'read' });
        files = [];
        for await (const [filename, handle] of dirHandle) {
            if (handle.kind !== 'file') continue;
            const lower = filename.toLowerCase();
            if (lower.endsWith('.mps') || lower.endsWith('.mps.gz')) {
                const displayName = filename
                    .replace(/\.mps\.gz$/i, '')
                    .replace(/\.mps$/i, '');
                files.push({ name: displayName, filename, handle });
            }
        }
        files.sort((a, b) => a.name.localeCompare(b.name));
        return files;
    }

    async function getFile(name) {
        const entry = files.find(f => f.name === name);
        if (!entry) throw new Error('File not found: ' + name);
        return entry.handle.getFile();
    }

    function isSupported() {
        return typeof window.showDirectoryPicker === 'function';
    }

    return { openDirectory, getFile, isSupported };
})();

// dirpicker.js is kept minimal — instance bank is now a separate page (instances.html)
// and directory picker functionality is no longer needed since we bundle instances.

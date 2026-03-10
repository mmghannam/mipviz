mergeInto(LibraryManager.library, {
    js_on_log: function(ptr, len) {
        var msg = UTF8ToString(ptr, len);
        if (typeof postMessage === 'function') {
            postMessage({type: 'log', line: msg});
        }
    }
});

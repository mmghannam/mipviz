// Web Worker for MIP solving — runs HiGHS in background thread
// Log lines stream back via postMessage({type: 'log', line: ...})

importScripts('mipviz_wasm.js');

let Module = null;

MipVizWasm().then(mod => {
    Module = mod;
    postMessage({ type: 'ready' });
}).catch(err => {
    postMessage({ type: 'error', message: 'Failed to load WASM: ' + err });
});

onmessage = function(e) {
    if (e.data.type !== 'solve') return;

    const { fileBytes, fileName, solver } = e.data;

    // Write file to Emscripten FS
    const path = '/tmp/solve.mps';
    Module.FS.writeFile(path, new Uint8Array(fileBytes));

    // Pass path to WASM
    const pathBytes = new TextEncoder().encode(path);
    const pathPtr = Module._mipviz_alloc(pathBytes.length);
    Module.HEAPU8.set(pathBytes, pathPtr);

    const solveFn = solver === 'scip' ? Module._mipviz_solve_mip_scip : Module._mipviz_solve_mip;
    const status = solveFn(pathPtr, pathBytes.length);
    Module._mipviz_free(pathPtr, pathBytes.length);

    // Read result
    const resultPtr = Module._mipviz_result_ptr();
    const resultLen = Module._mipviz_result_len();
    const resultStr = Module.UTF8ToString(resultPtr, resultLen);
    Module._mipviz_free_result();

    let result = {};
    try { result = JSON.parse(resultStr); } catch (_) {}

    postMessage({ type: 'done', status, result });
};

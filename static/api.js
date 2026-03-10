// api.js — WASM API layer

const API = (() => {
    let Module = null;
    let wasmReady = false;
    let wasmReadyPromise = MipVizWasm().then(mod => {
        Module = mod;
        wasmReady = true;
    });

    async function ensureReady() {
        if (!wasmReady) await wasmReadyPromise;
    }

    // Helper: pass string to WASM, return pointer + length
    function writeString(str) {
        const bytes = new TextEncoder().encode(str);
        const ptr = Module._mipviz_alloc(bytes.length);
        Module.HEAPU8.set(bytes, ptr);
        return { ptr, len: bytes.length };
    }

    function freeStr(s) {
        Module._mipviz_free(s.ptr, s.len);
    }

    // Read result JSON from WASM static buffer
    function readResult() {
        const ptr = Module._mipviz_result_ptr();
        const len = Module._mipviz_result_len();
        const str = Module.UTF8ToString(ptr, len);
        Module._mipviz_free_result();
        return JSON.parse(str);
    }

    // Write file bytes to Emscripten virtual FS and return the path
    function writeToVFS(bytes, filename) {
        const path = '/tmp/' + filename;
        Module.FS.writeFile(path, new Uint8Array(bytes));
        return path;
    }

    // Decompress gzipped ArrayBuffer using DecompressionStream
    async function decompressGzip(buffer) {
        const ds = new DecompressionStream('gzip');
        const stream = new Blob([buffer]).stream().pipeThrough(ds);
        return new Response(stream).text();
    }

    // --- Public API ---

    async function parseModel(file) {
        await ensureReady();
        const buffer = await file.arrayBuffer();
        let text;
        if (file.name.endsWith('.gz')) {
            text = await decompressGzip(buffer);
        } else {
            text = new TextDecoder().decode(buffer);
        }

        const data = writeString(text);
        const name = writeString(file.name);
        const status = Module._mipviz_parse_model(data.ptr, data.len, name.ptr, name.len);
        freeStr(data);
        freeStr(name);

        const result = readResult();
        if (status !== 0) throw new Error(result.error || 'Parse failed');
        return result;
    }

    async function presolveModel(file, solver = 'scip') {
        await ensureReady();
        let buffer = await file.arrayBuffer();
        if (file.name.endsWith('.gz')) {
            const text = await decompressGzip(buffer);
            buffer = new TextEncoder().encode(text).buffer;
        }
        const path = writeToVFS(buffer, 'input.mps');

        const pathStr = writeString(path);
        const name = writeString(file.name);
        const presolveFn = solver === 'scip'
            ? Module._mipviz_presolve_model_scip
            : Module._mipviz_presolve_model;
        const status = presolveFn(pathStr.ptr, pathStr.len, name.ptr, name.len);
        freeStr(pathStr);
        freeStr(name);

        const result = readResult();
        if (status !== 0) throw new Error(result.error || 'Presolve failed');
        return result;
    }

    async function getReductions(file) {
        await ensureReady();
        let buffer = await file.arrayBuffer();
        if (file.name.endsWith('.gz')) {
            const text = await decompressGzip(buffer);
            buffer = new TextEncoder().encode(text).buffer;
        }
        const path = writeToVFS(buffer, 'input.mps');

        const pathStr = writeString(path);
        const status = Module._mipviz_get_reductions(pathStr.ptr, pathStr.len);
        freeStr(pathStr);

        const result = readResult();
        if (status !== 0) throw new Error(result.error || 'Reductions failed');
        return result;
    }

    async function solveRootLp(file, presolved) {
        await ensureReady();
        let buffer = await file.arrayBuffer();
        if (file.name.endsWith('.gz')) {
            const text = await decompressGzip(buffer);
            buffer = new TextEncoder().encode(text).buffer;
        }
        const path = writeToVFS(buffer, 'input.mps');

        const pathStr = writeString(path);
        const status = Module._mipviz_solve_root_lp(pathStr.ptr, pathStr.len, presolved ? 1 : 0);
        freeStr(pathStr);

        const result = readResult();
        if (status !== 0) throw new Error(result.error || 'LP solve failed');
        return result;
    }

    async function solveConstraintSubset(file, indices, lpMode) {
        await ensureReady();
        let buffer = await file.arrayBuffer();
        if (file.name.endsWith('.gz')) {
            const text = await decompressGzip(buffer);
            buffer = new TextEncoder().encode(text).buffer;
        }
        const path = writeToVFS(buffer, 'input.mps');

        const pathStr = writeString(path);
        const indicesJson = writeString(JSON.stringify(indices));
        const status = Module._mipviz_solve_constraint_subset(
            pathStr.ptr, pathStr.len, indicesJson.ptr, indicesJson.len, lpMode ? 1 : 0
        );
        freeStr(pathStr);
        freeStr(indicesJson);

        const result = readResult();
        if (status !== 0) throw new Error(result.error || 'Constraint subset solve failed');
        return result;
    }

    async function getCliques(file) {
        await ensureReady();
        let buffer = await file.arrayBuffer();
        if (file.name.endsWith('.gz')) {
            const text = await decompressGzip(buffer);
            buffer = new TextEncoder().encode(text).buffer;
        }
        const path = writeToVFS(buffer, 'input.mps');

        const pathStr = writeString(path);
        const status = Module._mipviz_get_cliques(pathStr.ptr, pathStr.len);
        freeStr(pathStr);

        const result = readResult();
        if (status !== 0) throw new Error(result.error || 'Get cliques failed');
        return result;
    }

    async function getCliquesImplications(file, solver = 'scip') {
        await ensureReady();
        let buffer = await file.arrayBuffer();
        if (file.name.endsWith('.gz')) {
            const text = await decompressGzip(buffer);
            buffer = new TextEncoder().encode(text).buffer;
        }
        const path = writeToVFS(buffer, 'input.mps');

        const pathStr = writeString(path);
        const fn_ = solver === 'scip'
            ? Module._mipviz_get_cliques_scip
            : Module._mipviz_get_cliques_highs;
        if (typeof fn_ !== 'function') {
            freeStr(pathStr);
            throw new Error('WASM rebuild required — cliques/implications not available in current binary');
        }
        const status = fn_(pathStr.ptr, pathStr.len);
        freeStr(pathStr);

        const result = readResult();
        if (status !== 0) throw new Error(result.error || 'Cliques extraction failed');
        return result;
    }

    async function getSymmetry(file) {
        await ensureReady();
        let buffer = await file.arrayBuffer();
        if (file.name.endsWith('.gz')) {
            const text = await decompressGzip(buffer);
            buffer = new TextEncoder().encode(text).buffer;
        }
        const path = writeToVFS(buffer, 'input.mps');

        const pathStr = writeString(path);
        const status = Module._mipviz_get_symmetry_scip(pathStr.ptr, pathStr.len);
        freeStr(pathStr);

        const result = readResult();
        if (status !== 0) throw new Error(result.error || 'Symmetry extraction failed');
        return result;
    }

    return {
        ensureReady,
        parseModel,
        presolveModel,
        getReductions,
        getCliques,
        solveRootLp,
        solveConstraintSubset,
        getCliquesImplications,
        getSymmetry,
    };
})();

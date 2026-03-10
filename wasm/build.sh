#!/bin/bash
set -euo pipefail

# Source emsdk to ensure EMSDK env var is set (needed by lio-highs build.rs)
source "${EMSDK:-$HOME/emsdk}/emsdk_env.sh" 2>/dev/null || true

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
ROOT_DIR="$(dirname "$SCRIPT_DIR")"
TARGET_DIR="$ROOT_DIR/target/wasm32-unknown-emscripten/release"

echo "Building Rust staticlib..."
cargo build --target wasm32-unknown-emscripten --release -p mipviz-wasm

HIGHS_LIB="$TARGET_DIR/build/lio-highs-*/out/lib/libhighs.a"
SCIP_LIB_DIR=$(echo $TARGET_DIR/build/scip-sys-*/out/lib)
WASM_LIB="$TARGET_DIR/libmipviz_wasm.a"

echo "SCIP lib dir: $SCIP_LIB_DIR"
ls "$SCIP_LIB_DIR"/ 2>/dev/null || echo "Warning: SCIP lib dir not found"

echo "Linking with emcc..."
mkdir -p "$SCRIPT_DIR/dist"
emcc \
    $WASM_LIB \
    $HIGHS_LIB \
    "$SCIP_LIB_DIR"/libscip.a \
    "$SCIP_LIB_DIR"/libsoplex*.a \
    -fwasm-exceptions \
    -O3 \
    -o "$SCRIPT_DIR/dist/mipviz_wasm.js" \
    -sMODULARIZE=1 \
    -sEXPORT_NAME=MipVizWasm \
    -sALLOW_MEMORY_GROWTH=1 \
    -sEXPORTED_FUNCTIONS='["_mipviz_alloc","_mipviz_free","_mipviz_parse_model","_mipviz_parse_model_scip","_mipviz_presolve_model","_mipviz_presolve_model_scip","_mipviz_get_reductions","_mipviz_get_cliques","_mipviz_get_cliques_highs","_mipviz_get_cliques_scip","_mipviz_get_symmetry_scip","_mipviz_solve_root_lp","_mipviz_solve_mip","_mipviz_solve_mip_scip","_mipviz_solve_constraint_subset","_mipviz_result_ptr","_mipviz_result_len","_mipviz_free_result","_malloc","_free"]' \
    -sEXPORTED_RUNTIME_METHODS='["ccall","cwrap","FS","UTF8ToString","stringToUTF8","lengthBytesUTF8","HEAPU8"]' \
    -sFORCE_FILESYSTEM=1 \
    --js-library "$SCRIPT_DIR/src/mipviz_log.js" \
    --no-entry

# Copy to static dir for serving
cp "$SCRIPT_DIR/dist/mipviz_wasm.js" "$ROOT_DIR/static/"
cp "$SCRIPT_DIR/dist/mipviz_wasm.wasm" "$ROOT_DIR/static/"

echo "Built: dist/mipviz_wasm.js + dist/mipviz_wasm.wasm"
echo "Copied to: static/mipviz_wasm.{js,wasm}"

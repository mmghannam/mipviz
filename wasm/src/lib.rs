use std::os::raw::{c_char, c_int, c_void};
use std::sync::Mutex;

unsafe extern "C" {
    fn js_on_log(ptr: *const u8, len: usize);

}

unsafe extern "C" fn log_callback(
    _callback_type: c_int,
    message: *const c_char,
    _data_out: *const lio_highs::ffi::HighsCallbackDataOut,
    _data_in: *mut lio_highs::ffi::HighsCallbackDataIn,
    _user_data: *mut c_void,
) {
    if !message.is_null() {
        let msg = unsafe { std::ffi::CStr::from_ptr(message) }.to_bytes();
        unsafe { js_on_log(msg.as_ptr(), msg.len()) };
    }
}

// --- Memory management ---

#[unsafe(no_mangle)]
pub extern "C" fn mipviz_alloc(size: usize) -> *mut u8 {
    let mut buf = Vec::<u8>::with_capacity(size);
    let ptr = buf.as_mut_ptr();
    std::mem::forget(buf);
    ptr
}

#[unsafe(no_mangle)]
pub extern "C" fn mipviz_free(ptr: *mut u8, size: usize) {
    unsafe {
        drop(Vec::from_raw_parts(ptr, 0, size));
    }
}

// --- Result passing ---

static LAST_RESULT: Mutex<Option<Vec<u8>>> = Mutex::new(None);

fn set_result(json: String) {
    *LAST_RESULT.lock().unwrap() = Some(json.into_bytes());
}

fn set_error(msg: String) {
    set_result(format!("{{\"error\":{}}}", serde_json::to_string(&msg).unwrap()));
}

#[unsafe(no_mangle)]
pub extern "C" fn mipviz_result_ptr() -> *const u8 {
    LAST_RESULT
        .lock()
        .unwrap()
        .as_ref()
        .map_or(std::ptr::null(), |v| v.as_ptr())
}

#[unsafe(no_mangle)]
pub extern "C" fn mipviz_result_len() -> usize {
    LAST_RESULT
        .lock()
        .unwrap()
        .as_ref()
        .map_or(0, |v| v.len())
}

#[unsafe(no_mangle)]
pub extern "C" fn mipviz_free_result() {
    *LAST_RESULT.lock().unwrap() = None;
}

// --- Helper to read pointer+len as &str ---

unsafe fn read_str<'a>(ptr: *const u8, len: usize) -> &'a str {
    if ptr.is_null() || len == 0 {
        return "";
    }
    unsafe { std::str::from_utf8_unchecked(std::slice::from_raw_parts(ptr, len)) }
}

// Parse "key = value" / "key value" lines from the textarea. "#" starts a
// comment. Returns an iterator of owned (key, value) pairs, one per line.
fn parse_solver_params(text: &str) -> Vec<(String, String)> {
    let mut out = Vec::new();
    for raw in text.lines() {
        let line = match raw.split('#').next() {
            Some(s) => s.trim(),
            None => continue,
        };
        if line.is_empty() {
            continue;
        }
        let (k, v) = if let Some(eq) = line.find('=') {
            (line[..eq].trim(), line[eq + 1..].trim())
        } else if let Some(ws) = line.find(char::is_whitespace) {
            (line[..ws].trim(), line[ws..].trim())
        } else {
            (line, "")
        };
        if k.is_empty() {
            continue;
        }
        out.push((k.to_string(), v.to_string()));
    }
    out
}

// Apply `key = value` lines to a raw HiGHS instance using the generic
// Highs_setOptionValue setter (HiGHS parses the string per the registered
// option type). Returns an error listing the first bad key.
unsafe fn apply_highs_params(highs: *mut std::os::raw::c_void, text: &str) -> Result<(), String> {
    use lio_highs::ffi::Highs_setOptionValue;
    use std::ffi::CString;
    for (k, v) in parse_solver_params(text) {
        let c_key = CString::new(k.as_bytes()).map_err(|_| format!("invalid option name: {}", k))?;
        let c_val = CString::new(v.as_bytes()).map_err(|_| format!("invalid option value for {}: {:?}", k, v))?;
        let status = unsafe { Highs_setOptionValue(highs, c_key.as_ptr(), c_val.as_ptr()) };
        if status != 0 {
            return Err(format!("HiGHS rejected parameter '{}' = '{}' (status {})", k, v, status));
        }
    }
    Ok(())
}

// Write params to a VFS temp file and call SCIPreadParams so SCIP does the
// per-type dispatch itself.
unsafe fn apply_scip_params(scip: *mut scip_sys::SCIP, text: &str) -> Result<(), String> {
    use std::ffi::CString;
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return Ok(());
    }
    let path = "/tmp/mipviz_scip_params.set";
    std::fs::write(path, text).map_err(|e| format!("failed to stage SCIP params: {}", e))?;
    let c_path = CString::new(path).unwrap();
    let ret = unsafe { scip_sys::SCIPreadParams(scip, c_path.as_ptr()) };
    let _ = std::fs::remove_file(path);
    if ret != scip_sys::SCIP_Retcode_SCIP_OKAY {
        return Err(format!("SCIP rejected parameters (SCIPreadParams code {})", ret));
    }
    Ok(())
}

// --- Parse model (numnom, pure in-memory, no filesystem needed) ---

/// Parse MPS text from memory. JS should decompress .mps.gz before calling this.
/// Returns 0 on success, 1 on error. Result via mipviz_result_ptr/len.
#[unsafe(no_mangle)]
pub extern "C" fn mipviz_parse_model(
    data_ptr: *const u8,
    data_len: usize,
    name_ptr: *const u8,
    name_len: usize,
) -> i32 {
    let text = unsafe { read_str(data_ptr, data_len) };
    let name = unsafe { read_str(name_ptr, name_len) };

    match mipviz::extract_model_data_from_str(text, name) {
        Ok(resp) => {
            set_result(serde_json::to_string(&resp).unwrap());
            0
        }
        Err(e) => {
            set_error(e);
            1
        }
    }
}

// --- Presolve model (HiGHS via Emscripten FS) ---

/// Presolve a model with HiGHS. The file must already be written to the
/// Emscripten virtual FS by JS (via Module.FS.writeFile). Pass the path.
/// Returns 0 on success, 1 on error.
#[unsafe(no_mangle)]
pub extern "C" fn mipviz_presolve_model(
    path_ptr: *const u8,
    path_len: usize,
    name_ptr: *const u8,
    name_len: usize,
) -> i32 {
    let path = unsafe { read_str(path_ptr, path_len) };
    let name = unsafe { read_str(name_ptr, name_len) };

    match mipviz::extract_presolved_model_data(path, name) {
        Ok(resp) => {
            set_result(serde_json::to_string(&resp).unwrap());
            0
        }
        Err(e) => {
            set_error(e);
            1
        }
    }
}

// --- Presolve model with SCIP (via Emscripten FS) ---

/// Presolve a model with SCIP. The file must already be written to the
/// Emscripten virtual FS by JS (via Module.FS.writeFile). Pass the path.
/// Returns 0 on success, 1 on error.
#[unsafe(no_mangle)]
pub extern "C" fn mipviz_presolve_model_scip(
    path_ptr: *const u8,
    path_len: usize,
    name_ptr: *const u8,
    name_len: usize,
) -> i32 {
    let path = unsafe { read_str(path_ptr, path_len) };
    let name = unsafe { read_str(name_ptr, name_len) };

    match mipviz::extract_presolved_model_data_scip(path, name) {
        Ok(resp) => {
            set_result(serde_json::to_string(&resp).unwrap());
            0
        }
        Err(e) => {
            set_error(e);
            1
        }
    }
}

// --- Parse model with SCIP (supports LP format via Emscripten FS) ---

/// Parse a model with SCIP (supports .lp, .mps, etc.). File must be on Emscripten FS.
/// Returns 0 on success, 1 on error.
#[unsafe(no_mangle)]
pub extern "C" fn mipviz_parse_model_scip(
    path_ptr: *const u8,
    path_len: usize,
    name_ptr: *const u8,
    name_len: usize,
) -> i32 {
    let path = unsafe { read_str(path_ptr, path_len) };
    let name = unsafe { read_str(name_ptr, name_len) };

    match mipviz::extract_model_data(path, name) {
        Ok(resp) => {
            set_result(serde_json::to_string(&resp).unwrap());
            0
        }
        Err(e) => {
            set_error(e);
            1
        }
    }
}

// --- Get presolve reductions (HiGHS via Emscripten FS) ---

/// Get presolve reduction details. File must be on Emscripten FS.
/// Returns 0 on success, 1 on error.
#[unsafe(no_mangle)]
pub extern "C" fn mipviz_get_reductions(
    path_ptr: *const u8,
    path_len: usize,
) -> i32 {
    let path = unsafe { read_str(path_ptr, path_len) };

    match mipviz::extract_reductions(path) {
        Ok(resp) => {
            set_result(serde_json::to_string(&resp).unwrap());
            0
        }
        Err(e) => {
            set_error(e);
            1
        }
    }
}

// --- Get presolve reductions (cub via Emscripten FS) ---

/// Get presolve reduction details from cub. File must be on Emscripten FS.
/// Returns 0 on success, 1 on error.
#[unsafe(no_mangle)]
pub extern "C" fn mipviz_get_reductions_cub(
    path_ptr: *const u8,
    path_len: usize,
) -> i32 {
    let path = unsafe { read_str(path_ptr, path_len) };

    match mipviz::extract_reductions_cub(path) {
        Ok(resp) => {
            set_result(serde_json::to_string(&resp).unwrap());
            0
        }
        Err(e) => {
            set_error(e);
            1
        }
    }
}

// --- Solve constraint subset relaxation ---

/// Solve relaxation keeping only specified constraints. File must be on Emscripten FS.
/// indices_ptr/indices_len point to a JSON array of constraint indices.
/// lp_mode: 0 = MIP, 1 = LP relaxation
/// Returns 0 on success, 1 on error.
#[unsafe(no_mangle)]
pub extern "C" fn mipviz_solve_constraint_subset(
    path_ptr: *const u8,
    path_len: usize,
    indices_ptr: *const u8,
    indices_len: usize,
    lp_mode: i32,
    params_ptr: *const u8,
    params_len: usize,
) -> i32 {
    let path = unsafe { read_str(path_ptr, path_len) };
    let indices_json = unsafe { read_str(indices_ptr, indices_len) };
    let params = unsafe { read_str(params_ptr, params_len) };

    let indices: Vec<usize> = match serde_json::from_str(indices_json) {
        Ok(v) => v,
        Err(e) => {
            set_error(format!("Invalid indices JSON: {}", e));
            return 1;
        }
    };

    match mipviz::solve_constraint_subset(path, &indices, lp_mode != 0, params) {
        Ok(resp) => {
            set_result(serde_json::to_string(&resp).unwrap());
            0
        }
        Err(e) => {
            set_error(e);
            1
        }
    }
}

// --- Get cliques (HiGHS via Emscripten FS) ---

/// Get clique data from HiGHS MIP presolve. File must be on Emscripten FS.
/// Returns 0 on success, 1 on error.
#[unsafe(no_mangle)]
pub extern "C" fn mipviz_get_cliques(
    path_ptr: *const u8,
    path_len: usize,
) -> i32 {
    let path = unsafe { read_str(path_ptr, path_len) };

    match mipviz::extract_cliques(path) {
        Ok(resp) => {
            set_result(serde_json::to_string(&resp).unwrap());
            0
        }
        Err(e) => {
            set_error(e);
            1
        }
    }
}

// --- Extract cliques & implications (HiGHS via Emscripten FS) ---

/// Extract cliques and implications with HiGHS. File must be on Emscripten FS.
/// Returns 0 on success, 1 on error.
#[unsafe(no_mangle)]
pub extern "C" fn mipviz_get_cliques_highs(
    path_ptr: *const u8,
    path_len: usize,
) -> i32 {
    let path = unsafe { read_str(path_ptr, path_len) };

    match mipviz::extract_cliques_implications_highs(path) {
        Ok(resp) => {
            set_result(serde_json::to_string(&resp).unwrap());
            0
        }
        Err(e) => {
            set_error(e);
            1
        }
    }
}

// --- Extract cliques & implications (SCIP via Emscripten FS) ---

/// Extract cliques and implications with SCIP. File must be on Emscripten FS.
/// Returns 0 on success, 1 on error.
#[unsafe(no_mangle)]
pub extern "C" fn mipviz_get_cliques_scip(
    path_ptr: *const u8,
    path_len: usize,
) -> i32 {
    let path = unsafe { read_str(path_ptr, path_len) };

    match mipviz::extract_cliques_implications_scip(path) {
        Ok(resp) => {
            set_result(serde_json::to_string(&resp).unwrap());
            0
        }
        Err(e) => {
            set_error(e);
            1
        }
    }
}

// --- Extract symmetry (SCIP via Emscripten FS) ---

/// Extract symmetry information with SCIP. File must be on Emscripten FS.
/// Returns 0 on success, 1 on error.
#[unsafe(no_mangle)]
pub extern "C" fn mipviz_get_symmetry_scip(
    path_ptr: *const u8,
    path_len: usize,
) -> i32 {
    let path = unsafe { read_str(path_ptr, path_len) };

    match mipviz::extract_symmetry_scip(path) {
        Ok(resp) => {
            set_result(serde_json::to_string(&resp).unwrap());
            0
        }
        Err(e) => {
            set_error(e);
            1
        }
    }
}

// --- Solve MIP (HiGHS via raw FFI with logging callback) ---

/// Solve the MIP. File must be on Emscripten FS.
/// Log lines are sent via js_on_log callback.
/// Returns HiGHS status (0 = success). Result (obj value) via mipviz_result_ptr.
#[unsafe(no_mangle)]
pub extern "C" fn mipviz_solve_mip(
    path_ptr: *const u8,
    path_len: usize,
    params_ptr: *const u8,
    params_len: usize,
) -> i32 {
    use lio_highs::ffi::*;
    use std::ffi::CString;

    let path = unsafe { read_str(path_ptr, path_len) };
    let params = unsafe { read_str(params_ptr, params_len) };
    let c_path = CString::new(path).unwrap();

    unsafe {
        let highs = Highs_create();
        Highs_setCallback(highs, Some(log_callback), std::ptr::null_mut());
        Highs_startCallback(highs, kHighsCallbackLogging);

        if let Err(e) = apply_highs_params(highs, params) {
            set_error(e);
            Highs_destroy(highs);
            return 1;
        }

        let status = Highs_readModel(highs, c_path.as_ptr());
        if status != 0 {
            set_error(format!("Failed to read model (status {})", status));
            Highs_destroy(highs);
            return status;
        }

        let status = Highs_run(highs);
        let obj = Highs_getObjectiveValue(highs);
        let model_status = Highs_getModelStatus(highs);

        let num_cols = Highs_getNumCol(highs) as usize;
        let mut col_values = vec![0.0f64; num_cols];
        Highs_getSolution(
            highs,
            col_values.as_mut_ptr(),
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            std::ptr::null_mut(),
        );

        Highs_destroy(highs);

        let status_str = match model_status {
            7 => "Optimal",
            8 => "Infeasible",
            9 => "Unbounded",
            10 => "ObjectiveBound",
            11 => "ObjectiveTarget",
            13 => "TimeLimit",
            14 => "IterationLimit",
            _ => "Unknown",
        };

        let resp = serde_json::json!({
            "status": status_str,
            "objective_value": obj,
            "col_values": col_values,
        });
        set_result(resp.to_string());
        status
    }
}

// --- Extract HiGHS root cuts ---
//
// Registers `kHighsCallbackMipGetCutPool` and runs the MIP solver. HiGHS
// fires the callback once after the first root separation round (see
// HighsMipSolverData.cpp). We snapshot the cut pool, then set
// `user_interrupt = 1` so the solve returns cleanly without doing B&B.
//
// HiGHS internal presolve is disabled so the cut indices reference the
// original column space of the model the user loaded.

struct RootCutPool {
    num_col: i32,
    num_cut: i32,
    starts: Vec<i32>,   // length num_cut + 1
    indices: Vec<i32>,  // length nnz
    values: Vec<f64>,   // length nnz
    lower: Vec<f64>,    // length num_cut
    upper: Vec<f64>,    // length num_cut
}

static ROOT_CUT_POOL: Mutex<Option<RootCutPool>> = Mutex::new(None);

unsafe extern "C" fn root_cut_callback(
    callback_type: c_int,
    message: *const c_char,
    data_out: *const lio_highs::ffi::HighsCallbackDataOut,
    data_in: *mut lio_highs::ffi::HighsCallbackDataIn,
    _user_data: *mut c_void,
) {
    use lio_highs::ffi::*;
    if callback_type == kHighsCallbackLogging as c_int {
        if !message.is_null() {
            let msg = unsafe { std::ffi::CStr::from_ptr(message) }.to_bytes();
            unsafe { js_on_log(msg.as_ptr(), msg.len()) };
        }
        return;
    }
    if callback_type != kHighsCallbackMipGetCutPool as c_int || data_out.is_null() {
        return;
    }
    let d = unsafe { &*data_out };
    let num_cut = d.cutpool_num_cut as usize;
    if num_cut > 0 {
        let nnz = d.cutpool_num_nz as usize;
        let starts = unsafe { std::slice::from_raw_parts(d.cutpool_start, num_cut + 1) }.to_vec();
        let indices = unsafe { std::slice::from_raw_parts(d.cutpool_index, nnz) }.to_vec();
        let values = unsafe { std::slice::from_raw_parts(d.cutpool_value, nnz) }.to_vec();
        let lower = unsafe { std::slice::from_raw_parts(d.cutpool_lower, num_cut) }.to_vec();
        let upper = unsafe { std::slice::from_raw_parts(d.cutpool_upper, num_cut) }.to_vec();
        *ROOT_CUT_POOL.lock().unwrap() = Some(RootCutPool {
            num_col: d.cutpool_num_col,
            num_cut: d.cutpool_num_cut,
            starts,
            indices,
            values,
            lower,
            upper,
        });
    }
    if !data_in.is_null() {
        unsafe { (*data_in).user_interrupt = 1 };
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn mipviz_get_root_cuts(
    path_ptr: *const u8,
    path_len: usize,
    params_ptr: *const u8,
    params_len: usize,
) -> i32 {
    use lio_highs::ffi::*;
    use std::ffi::CString;

    let path = unsafe { read_str(path_ptr, path_len) };
    let params = unsafe { read_str(params_ptr, params_len) };
    let c_path = CString::new(path).unwrap();

    *ROOT_CUT_POOL.lock().unwrap() = None;

    unsafe {
        let highs = Highs_create();
        Highs_setCallback(highs, Some(root_cut_callback), std::ptr::null_mut());
        Highs_startCallback(highs, kHighsCallbackLogging);
        Highs_startCallback(highs, kHighsCallbackMipGetCutPool);

        // Cut indices come back in whichever column space HiGHS solves on.
        // Disable internal presolve so they match the loaded model.
        let off_key = CString::new("presolve").unwrap();
        let off_val = CString::new("off").unwrap();
        Highs_setOptionValue(highs, off_key.as_ptr(), off_val.as_ptr());

        if let Err(e) = apply_highs_params(highs, params) {
            set_error(e);
            Highs_destroy(highs);
            return 1;
        }

        let status = Highs_readModel(highs, c_path.as_ptr());
        if status != 0 {
            set_error(format!("Failed to read model (status {})", status));
            Highs_destroy(highs);
            return status;
        }

        let _ = Highs_run(highs);
        Highs_destroy(highs);
    }

    let pool = ROOT_CUT_POOL.lock().unwrap().take();
    let pool = match pool {
        Some(p) => p,
        None => {
            set_result(
                serde_json::json!({ "num_cuts": 0, "num_cols": 0, "cuts": [] }).to_string(),
            );
            return 0;
        }
    };

    let num_cut = pool.num_cut as usize;
    let mut cuts = Vec::with_capacity(num_cut);
    for i in 0..num_cut {
        let lo = pool.starts[i] as usize;
        let hi = pool.starts[i + 1] as usize;
        let coeffs: Vec<serde_json::Value> = (lo..hi)
            .map(|k| serde_json::json!([pool.indices[k], pool.values[k]]))
            .collect();
        let to_finite = |v: f64| {
            if v.is_finite() && v.abs() < 1e30 {
                serde_json::json!(v)
            } else {
                serde_json::Value::Null
            }
        };
        cuts.push(serde_json::json!({
            "lower": to_finite(pool.lower[i]),
            "upper": to_finite(pool.upper[i]),
            "coeffs": coeffs,
        }));
    }

    let resp = serde_json::json!({
        "num_cuts": pool.num_cut,
        "num_cols": pool.num_col,
        "cuts": cuts,
    });
    set_result(resp.to_string());
    0
}

// --- Solve MIP (SCIP via raw FFI with logging) ---

static SCIP_LOG_BUF: Mutex<Vec<u8>> = Mutex::new(Vec::new());
static SCIP_STATS_BUF: Mutex<Option<Vec<u8>>> = Mutex::new(None);

unsafe extern "C" fn scip_message_callback(
    _messagehdlr: *mut scip_sys::SCIP_MESSAGEHDLR,
    _file: *mut scip_sys::FILE,
    msg: *const c_char,
) {
    if msg.is_null() {
        return;
    }
    let bytes = unsafe { std::ffi::CStr::from_ptr(msg) }.to_bytes();

    // If stats capture is active, write there instead of log
    if let Some(ref mut stats) = *SCIP_STATS_BUF.lock().unwrap() {
        stats.extend_from_slice(bytes);
        return;
    }

    let mut buf = SCIP_LOG_BUF.lock().unwrap();
    for &b in bytes {
        if b == b'\n' {
            if !buf.is_empty() {
                unsafe { js_on_log(buf.as_ptr(), buf.len()) };
                buf.clear();
            }
        } else {
            buf.push(b);
        }
    }
}

/// Solve the MIP with SCIP. File must be on Emscripten FS.
/// Log lines are sent via js_on_log callback.
/// Returns 0 on success, 1 on error. Result via mipviz_result_ptr.
#[unsafe(no_mangle)]
pub extern "C" fn mipviz_solve_mip_scip(
    path_ptr: *const u8,
    path_len: usize,
    params_ptr: *const u8,
    params_len: usize,
) -> i32 {
    use std::ffi::CString;

    let path = unsafe { read_str(path_ptr, path_len) };
    let params = unsafe { read_str(params_ptr, params_len) };
    let c_path = CString::new(path).unwrap();

    unsafe {
        let mut scip: *mut scip_sys::SCIP = std::ptr::null_mut();
        scip_sys::SCIPcreate(&mut scip);
        scip_sys::SCIPincludeDefaultPlugins(scip);

        // Set up message handler to forward logs to JS
        let mut messagehdlr: *mut scip_sys::SCIP_MESSAGEHDLR = std::ptr::null_mut();
        scip_sys::SCIPmessagehdlrCreate(
            &mut messagehdlr,
            0, // not buffered
            std::ptr::null(),
            0, // not quiet
            Some(scip_message_callback),
            Some(scip_message_callback),
            Some(scip_message_callback),
            None, // no free callback
            std::ptr::null_mut(),
        );
        scip_sys::SCIPsetMessagehdlr(scip, messagehdlr);
        scip_sys::SCIPmessagehdlrRelease(&mut messagehdlr);

        if let Err(e) = apply_scip_params(scip, params) {
            set_error(e);
            scip_sys::SCIPfree(&mut scip);
            return 1;
        }

        let ret = scip_sys::SCIPreadProb(scip, c_path.as_ptr(), std::ptr::null());
        if ret != scip_sys::SCIP_Retcode_SCIP_OKAY {
            set_error(format!("SCIP failed to read problem (code {})", ret));
            scip_sys::SCIPfree(&mut scip);
            return 1;
        }

        let ret = scip_sys::SCIPsolve(scip);
        if ret != scip_sys::SCIP_Retcode_SCIP_OKAY {
            set_error(format!("SCIP solve failed (code {})", ret));
            scip_sys::SCIPfree(&mut scip);
            return 1;
        }

        let status = scip_sys::SCIPgetStatus(scip);
        let status_str = match status {
            scip_sys::SCIP_Status_SCIP_STATUS_OPTIMAL => "Optimal",
            scip_sys::SCIP_Status_SCIP_STATUS_INFEASIBLE => "Infeasible",
            scip_sys::SCIP_Status_SCIP_STATUS_UNBOUNDED => "Unbounded",
            scip_sys::SCIP_Status_SCIP_STATUS_INFORUNBD => "InfOrUnbounded",
            scip_sys::SCIP_Status_SCIP_STATUS_TIMELIMIT => "TimeLimit",
            scip_sys::SCIP_Status_SCIP_STATUS_MEMLIMIT => "MemLimit",
            scip_sys::SCIP_Status_SCIP_STATUS_NODELIMIT => "NodeLimit",
            scip_sys::SCIP_Status_SCIP_STATUS_GAPLIMIT => "GapLimit",
            scip_sys::SCIP_Status_SCIP_STATUS_SOLLIMIT => "SolLimit",
            _ => "Unknown",
        };

        let best_sol = scip_sys::SCIPgetBestSol(scip);
        let obj = if !best_sol.is_null() {
            scip_sys::SCIPgetSolOrigObj(scip, best_sol)
        } else {
            f64::INFINITY
        };

        // Extract solution values
        let num_vars = scip_sys::SCIPgetNOrigVars(scip) as usize;
        let vars_ptr = scip_sys::SCIPgetOrigVars(scip);
        let vars = std::slice::from_raw_parts(vars_ptr, num_vars);

        let col_values: Vec<f64> = if !best_sol.is_null() {
            vars.iter().map(|&var| scip_sys::SCIPgetSolVal(scip, best_sol, var)).collect()
        } else {
            vec![0.0; num_vars]
        };

        // Flush any remaining log buffer
        {
            let mut buf = SCIP_LOG_BUF.lock().unwrap();
            if !buf.is_empty() {
                js_on_log(buf.as_ptr(), buf.len());
                buf.clear();
            }
        }

        // Capture SCIP statistics into buffer (diverts message callback away from log)
        *SCIP_STATS_BUF.lock().unwrap() = Some(Vec::new());
        scip_sys::SCIPprintStatistics(scip, std::ptr::null_mut());
        let stats_text = {
            let mut guard = SCIP_STATS_BUF.lock().unwrap();
            let bytes = guard.take().unwrap_or_default();
            String::from_utf8_lossy(&bytes).into_owned()
        };

        scip_sys::SCIPfree(&mut scip);

        let resp = serde_json::json!({
            "status": status_str,
            "objective_value": obj,
            "col_values": col_values,
            "stats": stats_text,
        });
        set_result(resp.to_string());
        0
    }
}

// --- Extract SCIP root cuts ---
//
// Registers an event handler on SCIP_EVENTTYPE_NODEFOCUSED. When the focus
// node has depth > 0, root processing is fully done — we drain LP rows of
// origin SEPA into a static buffer, then SCIPinterruptSolve. Presolve is
// disabled so column indices match the loaded model.

const SCIP_EVENTTYPE_NODEFOCUSED: u64 = 0x000040000;
const SCIP_EVENTTYPE_NODEFEASIBLE: u64 = 0x000080000;
const SCIP_EVENTTYPE_NODEINFEASIBLE: u64 = 0x000100000;
const SCIP_EVENTTYPE_NODEBRANCHED: u64 = 0x000200000;
const SCIP_EVENTTYPE_NODESOLVED: u64 =
    SCIP_EVENTTYPE_NODEFEASIBLE | SCIP_EVENTTYPE_NODEINFEASIBLE | SCIP_EVENTTYPE_NODEBRANCHED;
const SCIP_ROWORIGINTYPE_SEPA: u32 = 3;

struct ScipRootCut {
    lower: f64,
    upper: f64,
    coeffs: Vec<(i32, f64)>, // (probindex, coef)
    separator: String,       // e.g. "gomory", "mir", "" if no separator pointer
    name: String,            // SCIP's row name
    rank: i32,               // Chvátal-Gomory-style rank
    is_local: bool,
}

struct ScipRootCuts {
    num_cols: i32,
    infinity: f64,
    cuts: Vec<ScipRootCut>,
}

static SCIP_ROOT_CUTS: Mutex<Option<ScipRootCuts>> = Mutex::new(None);

unsafe extern "C" fn scip_root_cut_eventexec(
    scip: *mut scip_sys::SCIP,
    _eventhdlr: *mut scip_sys::SCIP_EVENTHDLR,
    _event: *mut scip_sys::SCIP_EVENT,
    _eventdata: *mut scip_sys::SCIP_EVENTDATA,
) -> scip_sys::SCIP_RETCODE {
    unsafe {
        // Capture once. The first NODEFOCUSED on the root fires before any
        // LP exists (nrows == 0), so gate on having LP rows. Later events —
        // NODEFOCUSED on a child (depth > 0, root just branched) or
        // NODESOLVED on root — see the post-separation LP.
        let node = scip_sys::SCIPgetCurrentNode(scip);
        if node.is_null() {
            return scip_sys::SCIP_Retcode_SCIP_OKAY;
        }
        if SCIP_ROOT_CUTS.lock().unwrap().is_some() {
            return scip_sys::SCIP_Retcode_SCIP_OKAY;
        }

        let num_cols = scip_sys::SCIPgetNVars(scip);
        let mut rows_ptr: *mut *mut scip_sys::SCIP_ROW = std::ptr::null_mut();
        let mut nrows: i32 = 0;
        let ret = scip_sys::SCIPgetLPRowsData(scip, &mut rows_ptr, &mut nrows);
        if ret != scip_sys::SCIP_Retcode_SCIP_OKAY || nrows == 0 {
            return scip_sys::SCIP_Retcode_SCIP_OKAY;
        }
        let rows = std::slice::from_raw_parts(rows_ptr, nrows as usize);

        let mut cuts: Vec<ScipRootCut> = Vec::new();
        for &row in rows {
            if scip_sys::SCIProwGetOrigintype(row) != SCIP_ROWORIGINTYPE_SEPA {
                continue;
            }
            let nnz = scip_sys::SCIProwGetNNonz(row);
            let cols = std::slice::from_raw_parts(scip_sys::SCIProwGetCols(row), nnz as usize);
            let vals = std::slice::from_raw_parts(scip_sys::SCIProwGetVals(row), nnz as usize);
            let constant = scip_sys::SCIProwGetConstant(row);
            let lhs = scip_sys::SCIProwGetLhs(row) - constant;
            let rhs = scip_sys::SCIProwGetRhs(row) - constant;

            let mut coeffs = Vec::with_capacity(nnz as usize);
            for i in 0..nnz as usize {
                let var = scip_sys::SCIPcolGetVar(cols[i]);
                if var.is_null() {
                    continue;
                }
                let pi = scip_sys::SCIPvarGetProbindex(var);
                if pi < 0 {
                    continue;
                }
                coeffs.push((pi, vals[i]));
            }

            let read_cstr = |p: *const std::os::raw::c_char| -> String {
                if p.is_null() { return String::new(); }
                std::ffi::CStr::from_ptr(p).to_string_lossy().into_owned()
            };
            let sepa_ptr = scip_sys::SCIProwGetOriginSepa(row);
            let separator = if sepa_ptr.is_null() {
                String::new()
            } else {
                read_cstr(scip_sys::SCIPsepaGetName(sepa_ptr))
            };
            let name = read_cstr(scip_sys::SCIProwGetName(row));
            let rank = scip_sys::SCIProwGetRank(row);
            let is_local = scip_sys::SCIProwIsLocal(row) != 0;

            cuts.push(ScipRootCut {
                lower: lhs, upper: rhs, coeffs,
                separator, name, rank, is_local,
            });
        }

        let infinity = scip_sys::SCIPinfinity(scip);
        *SCIP_ROOT_CUTS.lock().unwrap() = Some(ScipRootCuts { num_cols, infinity, cuts });
        let _ = scip_sys::SCIPinterruptSolve(scip);
    }
    scip_sys::SCIP_Retcode_SCIP_OKAY
}

#[unsafe(no_mangle)]
pub extern "C" fn mipviz_get_root_cuts_scip(
    path_ptr: *const u8,
    path_len: usize,
    params_ptr: *const u8,
    params_len: usize,
) -> i32 {
    use std::ffi::CString;

    let path = unsafe { read_str(path_ptr, path_len) };
    let params = unsafe { read_str(params_ptr, params_len) };
    let c_path = CString::new(path).unwrap();

    *SCIP_ROOT_CUTS.lock().unwrap() = None;

    unsafe {
        let mut scip: *mut scip_sys::SCIP = std::ptr::null_mut();
        scip_sys::SCIPcreate(&mut scip);
        scip_sys::SCIPincludeDefaultPlugins(scip);

        // Forward SCIP logs to JS
        let mut messagehdlr: *mut scip_sys::SCIP_MESSAGEHDLR = std::ptr::null_mut();
        scip_sys::SCIPmessagehdlrCreate(
            &mut messagehdlr, 0, std::ptr::null(), 0,
            Some(scip_message_callback), Some(scip_message_callback), Some(scip_message_callback),
            None, std::ptr::null_mut(),
        );
        scip_sys::SCIPsetMessagehdlr(scip, messagehdlr);
        scip_sys::SCIPmessagehdlrRelease(&mut messagehdlr);

        // Disable presolve so column indices match the loaded model.
        let key = CString::new("presolving/maxrounds").unwrap();
        scip_sys::SCIPsetIntParam(scip, key.as_ptr(), 0);
        let key = CString::new("presolving/maxrestarts").unwrap();
        scip_sys::SCIPsetIntParam(scip, key.as_ptr(), 0);

        if let Err(e) = apply_scip_params(scip, params) {
            set_error(e);
            scip_sys::SCIPfree(&mut scip);
            return 1;
        }

        let ret = scip_sys::SCIPreadProb(scip, c_path.as_ptr(), std::ptr::null());
        if ret != scip_sys::SCIP_Retcode_SCIP_OKAY {
            set_error(format!("SCIP failed to read problem (code {})", ret));
            scip_sys::SCIPfree(&mut scip);
            return 1;
        }

        // Register the root-cut event handler. We need it included before
        // SCIPsolve transforms the problem so SCIPcatchEvent has somewhere
        // to attach. Use SCIPincludeEventhdlrBasic + SCIPcatchEvent.
        let hdlr_name = CString::new("rootcuts").unwrap();
        let hdlr_desc = CString::new("captures root cuts then interrupts").unwrap();
        let mut eventhdlr: *mut scip_sys::SCIP_EVENTHDLR = std::ptr::null_mut();
        scip_sys::SCIPincludeEventhdlrBasic(
            scip, &mut eventhdlr, hdlr_name.as_ptr(), hdlr_desc.as_ptr(),
            Some(scip_root_cut_eventexec), std::ptr::null_mut(),
        );
        // SCIPcatchEvent requires SCIP to be at least in TRANSFORMING; it's
        // legal here since SCIPreadProb leaves us in PROBLEM and we'll
        // immediately enter SCIPsolve. Defer catch via initsol callback —
        // simpler: catch right before solve via SCIPtransformProb then catch.
        scip_sys::SCIPtransformProb(scip);
        scip_sys::SCIPcatchEvent(
            scip, SCIP_EVENTTYPE_NODEFOCUSED | SCIP_EVENTTYPE_NODESOLVED, eventhdlr,
            std::ptr::null_mut(), std::ptr::null_mut(),
        );

        let _ = scip_sys::SCIPsolve(scip);

        // Flush any remaining log buffer
        {
            let mut buf = SCIP_LOG_BUF.lock().unwrap();
            if !buf.is_empty() {
                js_on_log(buf.as_ptr(), buf.len());
                buf.clear();
            }
        }

        scip_sys::SCIPfree(&mut scip);
    }

    let captured = SCIP_ROOT_CUTS.lock().unwrap().take();
    let captured = match captured {
        Some(v) => v,
        None => {
            set_result(
                serde_json::json!({ "num_cuts": 0, "num_cols": 0, "cuts": [] }).to_string(),
            );
            return 0;
        }
    };

    let inf = captured.infinity;
    let to_finite = |v: f64| {
        if v.is_finite() && v.abs() < inf { serde_json::json!(v) } else { serde_json::Value::Null }
    };
    let cuts_json: Vec<serde_json::Value> = captured.cuts.iter().map(|c| {
        let coeffs: Vec<serde_json::Value> = c.coeffs.iter()
            .map(|&(i, v)| serde_json::json!([i, v])).collect();
        serde_json::json!({
            "lower": to_finite(c.lower),
            "upper": to_finite(c.upper),
            "coeffs": coeffs,
            "separator": if c.separator.is_empty() { serde_json::Value::Null } else { serde_json::json!(c.separator) },
            "name": if c.name.is_empty() { serde_json::Value::Null } else { serde_json::json!(c.name) },
            "rank": c.rank,
            "is_local": c.is_local,
        })
    }).collect();

    let resp = serde_json::json!({
        "num_cuts": captured.cuts.len(),
        "num_cols": captured.num_cols,
        "cuts": cuts_json,
    });
    set_result(resp.to_string());
    0
}

// --- Solve root LP (HiGHS via Emscripten FS) ---

/// Solve the LP relaxation. File must be on Emscripten FS.
/// presolved: 0 = original, 1 = presolve first then solve presolved LP
/// solver_ptr/solver_len: "highs" or "scip" — which presolver's variable
/// ordering to match (only used when presolved=1).
/// Returns 0 on success, 1 on error.
#[unsafe(no_mangle)]
pub extern "C" fn mipviz_solve_root_lp(
    path_ptr: *const u8,
    path_len: usize,
    presolved: i32,
    solver_ptr: *const u8,
    solver_len: usize,
    params_ptr: *const u8,
    params_len: usize,
) -> i32 {
    let path = unsafe { read_str(path_ptr, path_len) };
    let solver = unsafe { read_str(solver_ptr, solver_len) };
    let params = unsafe { read_str(params_ptr, params_len) };

    match mipviz::solve_root_lp(path, presolved != 0, solver, params) {
        Ok(resp) => {
            set_result(serde_json::to_string(&resp).unwrap());
            0
        }
        Err(e) => {
            set_error(e);
            1
        }
    }
}

/// Solve the LP relaxation after injecting extra rows (e.g. derived cuts).
/// `rows_json` is a JSON array of `{lower, upper, coeffs: [[col, coef], …]}`.
/// `presolved` + `solver` select the variable-index space.
/// Returns 0 on success, 1 on error.
#[unsafe(no_mangle)]
pub extern "C" fn mipviz_solve_lp_with_extra_rows(
    path_ptr: *const u8,
    path_len: usize,
    presolved: i32,
    solver_ptr: *const u8,
    solver_len: usize,
    rows_json_ptr: *const u8,
    rows_json_len: usize,
    params_ptr: *const u8,
    params_len: usize,
) -> i32 {
    let path = unsafe { read_str(path_ptr, path_len) };
    let solver = unsafe { read_str(solver_ptr, solver_len) };
    let rows_json = unsafe { read_str(rows_json_ptr, rows_json_len) };
    let params = unsafe { read_str(params_ptr, params_len) };
    let rows: Vec<mipviz::ExtraRow> = match serde_json::from_str(rows_json) {
        Ok(r) => r,
        Err(e) => {
            set_error(format!("Failed to parse extra rows JSON: {}", e));
            return 1;
        }
    };
    match mipviz::solve_lp_with_extra_rows(path, presolved != 0, solver, &rows, params) {
        Ok(resp) => {
            set_result(serde_json::to_string(&resp).unwrap());
            0
        }
        Err(e) => {
            set_error(e);
            1
        }
    }
}

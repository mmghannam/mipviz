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
    unsafe { std::str::from_utf8_unchecked(std::slice::from_raw_parts(ptr, len)) }
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
) -> i32 {
    let path = unsafe { read_str(path_ptr, path_len) };
    let indices_json = unsafe { read_str(indices_ptr, indices_len) };

    let indices: Vec<usize> = match serde_json::from_str(indices_json) {
        Ok(v) => v,
        Err(e) => {
            set_error(format!("Invalid indices JSON: {}", e));
            return 1;
        }
    };

    match mipviz::solve_constraint_subset(path, &indices, lp_mode != 0) {
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
) -> i32 {
    use lio_highs::ffi::*;
    use std::ffi::CString;

    let path = unsafe { read_str(path_ptr, path_len) };
    let c_path = CString::new(path).unwrap();

    unsafe {
        let highs = Highs_create();
        Highs_setCallback(highs, Some(log_callback), std::ptr::null_mut());
        Highs_startCallback(highs, kHighsCallbackLogging);

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
) -> i32 {
    use std::ffi::CString;

    let path = unsafe { read_str(path_ptr, path_len) };
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

// --- Solve root LP (HiGHS via Emscripten FS) ---

/// Solve the LP relaxation. File must be on Emscripten FS.
/// presolved: 0 = original, 1 = presolve first then solve presolved LP
/// Returns 0 on success, 1 on error.
#[unsafe(no_mangle)]
pub extern "C" fn mipviz_solve_root_lp(
    path_ptr: *const u8,
    path_len: usize,
    presolved: i32,
) -> i32 {
    let path = unsafe { read_str(path_ptr, path_len) };

    match mipviz::solve_root_lp(path, presolved != 0) {
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

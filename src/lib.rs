pub mod model;

use model::{
    Clique, CliqueEntry, CliqueResponse, CliqueVariable, CliquesImplicationsResponse, Constraint,
    ImplicationEntry, ModelResponse, PresolveReductionItem, ReductionsResponse, Stats,
    SymmetryComponent, SymmetryResponse, Term, VarType, Variable, finite_or_none,
};
use std::ffi::{CStr, CString};
use std::os::raw::c_int;

fn build_model_response(
    variables: Vec<Variable>,
    constraints: Vec<Constraint>,
    num_nz: usize,
    obj_sense: String,
    obj_offset: f64,
    model_name: String,
) -> ModelResponse {
    let num_binary = variables
        .iter()
        .filter(|v| v.var_type == VarType::Binary)
        .count();
    let num_integer = variables
        .iter()
        .filter(|v| v.var_type == VarType::Integer)
        .count();
    let num_continuous = variables
        .iter()
        .filter(|v| v.var_type == VarType::Continuous)
        .count();

    let stats = Stats {
        num_vars: variables.len(),
        num_constraints: constraints.len(),
        num_nonzeros: num_nz,
        num_continuous,
        num_integer,
        num_binary,
    };

    ModelResponse {
        name: model_name,
        obj_sense,
        obj_offset,
        stats,
        variables,
        constraints,
        parse_time_ms: None,
    }
}

fn strip_mps_suffix(name: &str) -> String {
    name.strip_suffix(".mps.gz")
        .or_else(|| name.strip_suffix(".lp.gz"))
        .or_else(|| name.strip_suffix(".mps"))
        .or_else(|| name.strip_suffix(".lp"))
        .unwrap_or(name)
        .to_string()
}

fn build_model_response_from_numnom(
    m: numnom::Model,
    original_name: &str,
) -> Result<ModelResponse, String> {
    let num_cols = m.num_col as usize;
    let num_rows = m.num_row as usize;

    let mut variables = Vec::with_capacity(num_cols);
    for i in 0..num_cols {
        let var_type = match m.col_integrality[i] {
            numnom::VarType::Integer | numnom::VarType::SemiInteger
                if m.col_lower[i] == 0.0 && m.col_upper[i] == 1.0 =>
            {
                VarType::Binary
            }
            numnom::VarType::Integer | numnom::VarType::SemiInteger => VarType::Integer,
            _ => VarType::Continuous,
        };
        variables.push(Variable {
            name: m.col_names[i].clone(),
            var_type,
            lower: finite_or_none(m.col_lower[i]),
            upper: finite_or_none(m.col_upper[i]),
            obj: m.col_cost[i],
        });
    }

    // Convert CSC matrix to row-wise constraints
    let mut row_terms: Vec<Vec<Term>> = vec![Vec::new(); num_rows];
    for col in 0..num_cols {
        let start = m.a_matrix.start[col] as usize;
        let end = m.a_matrix.start[col + 1] as usize;
        for idx in start..end {
            let row = m.a_matrix.index[idx] as usize;
            let coeff = m.a_matrix.value[idx];
            let v = &variables[col];
            row_terms[row].push(Term {
                var_index: col,
                var_name: v.name.clone(),
                var_type: v.var_type,
                coeff,
            });
        }
    }

    let num_nz = m.a_matrix.value.len();
    let mut constraints = Vec::with_capacity(num_rows);
    for i in 0..num_rows {
        constraints.push(Constraint {
            name: m.row_names[i].clone(),
            lower: finite_or_none(m.row_lower[i]),
            upper: finite_or_none(m.row_upper[i]),
            terms: std::mem::take(&mut row_terms[i]),
        });
    }

    let obj_sense = if m.obj_sense_minimize {
        "minimize".to_string()
    } else {
        "maximize".to_string()
    };

    let model_name = strip_mps_suffix(original_name);

    Ok(build_model_response(
        variables, constraints, num_nz, obj_sense, m.obj_offset, model_name,
    ))
}

fn extract_model_data_numnom(path: &str, original_name: &str) -> Result<ModelResponse, String> {
    let m = numnom::parse_mps_file(path)?;
    build_model_response_from_numnom(m, original_name)
}

/// Parse MPS data from a string (for WASM in-memory parsing).
pub fn extract_model_data_from_str(mps_text: &str, original_name: &str) -> Result<ModelResponse, String> {
    let m = numnom::parse_mps_str(mps_text)?;
    build_model_response_from_numnom(m, original_name)
}

pub fn extract_model_data(path: &str, original_name: &str) -> Result<ModelResponse, String> {
    let lower = original_name.to_ascii_lowercase();
    if lower.ends_with(".mps") || lower.ends_with(".mps.gz") {
        return extract_model_data_numnom(path, original_name);
    }

    extract_model_data_scip(path, original_name)
}

fn extract_model_data_scip(path: &str, original_name: &str) -> Result<ModelResponse, String> {
    unsafe {
        let mut scip: *mut scip_sys::SCIP = std::ptr::null_mut();
        scip_sys::SCIPcreate(&mut scip);
        scip_sys::SCIPincludeDefaultPlugins(scip);
        scip_sys::SCIPsetIntParam(
            scip,
            CString::new("display/verblevel").unwrap().as_ptr(),
            0,
        );

        let c_path = CString::new(path).map_err(|e| format!("Invalid path: {}", e))?;
        let ret = scip_sys::SCIPreadProb(scip, c_path.as_ptr(), std::ptr::null());
        if ret != scip_sys::SCIP_Retcode_SCIP_OKAY {
            scip_sys::SCIPfree(&mut scip);
            return Err(format!("SCIP failed to read problem (code {})", ret));
        }

        let num_vars = scip_sys::SCIPgetNOrigVars(scip) as usize;
        let vars_ptr = scip_sys::SCIPgetOrigVars(scip);
        let num_conss = scip_sys::SCIPgetNOrigConss(scip) as usize;
        let conss_ptr = scip_sys::SCIPgetOrigConss(scip);

        let scip_vars = std::slice::from_raw_parts(vars_ptr, num_vars);
        let mut var_ptr_to_idx: std::collections::HashMap<*mut scip_sys::SCIP_VAR, usize> =
            std::collections::HashMap::with_capacity(num_vars);

        let mut variables = Vec::with_capacity(num_vars);
        for (i, &var) in scip_vars.iter().enumerate() {
            var_ptr_to_idx.insert(var, i);

            let name = CStr::from_ptr(scip_sys::SCIPvarGetName(var))
                .to_string_lossy()
                .into_owned();
            let lb = scip_sys::SCIPvarGetLbOriginal(var);
            let ub = scip_sys::SCIPvarGetUbOriginal(var);
            let obj = scip_sys::SCIPvarGetObj(var);
            let vtype = scip_sys::SCIPvarGetType(var);

            let var_type = match vtype {
                scip_sys::SCIP_Vartype_SCIP_VARTYPE_BINARY => VarType::Binary,
                scip_sys::SCIP_Vartype_SCIP_VARTYPE_INTEGER
                | scip_sys::SCIP_Vartype_SCIP_VARTYPE_IMPLINT => VarType::Integer,
                _ => VarType::Continuous,
            };

            variables.push(Variable {
                name,
                var_type,
                lower: finite_or_none(lb),
                upper: finite_or_none(ub),
                obj,
            });
        }

        let scip_conss = std::slice::from_raw_parts(conss_ptr, num_conss);
        let mut constraints = Vec::new();
        let mut num_nz = 0usize;

        for &cons in scip_conss {
            // Use generic constraint API that works across all handler types
            let mut nvars_int: c_int = 0;
            let mut success: u32 = 0;
            scip_sys::SCIPgetConsNVars(scip, cons, &mut nvars_int, &mut success);
            if success == 0 || nvars_int <= 0 {
                continue;
            }
            let nvars = nvars_int as usize;

            let mut cvars_buf: Vec<*mut scip_sys::SCIP_VAR> = vec![std::ptr::null_mut(); nvars];
            success = 0;
            scip_sys::SCIPgetConsVars(scip, cons, cvars_buf.as_mut_ptr(), nvars_int, &mut success);
            if success == 0 {
                continue;
            }

            let mut cvals_buf: Vec<f64> = vec![0.0; nvars];
            success = 0;
            scip_sys::SCIPgetConsVals(scip, cons, cvals_buf.as_mut_ptr(), nvars_int, &mut success);
            if success == 0 {
                continue;
            }

            success = 0;
            let lhs = scip_sys::SCIPconsGetLhs(scip, cons, &mut success);
            let lhs = if success != 0 { Some(lhs) } else { None };

            success = 0;
            let rhs = scip_sys::SCIPconsGetRhs(scip, cons, &mut success);
            let rhs = if success != 0 { Some(rhs) } else { None };

            let cons_name = CStr::from_ptr(scip_sys::SCIPconsGetName(cons))
                .to_string_lossy()
                .into_owned();

            let mut terms = Vec::with_capacity(nvars);
            for j in 0..nvars {
                if let Some(&var_idx) = var_ptr_to_idx.get(&cvars_buf[j]) {
                    let v = &variables[var_idx];
                    terms.push(Term {
                        var_index: var_idx,
                        var_name: v.name.clone(),
                        var_type: v.var_type,
                        coeff: cvals_buf[j],
                    });
                }
            }

            num_nz += terms.len();
            constraints.push(Constraint {
                name: cons_name,
                lower: lhs.and_then(|v| finite_or_none(v)),
                upper: rhs.and_then(|v| finite_or_none(v)),
                terms,
            });
        }

        let obj_sense = match scip_sys::SCIPgetObjsense(scip) {
            scip_sys::SCIP_Objsense_SCIP_OBJSENSE_MINIMIZE => "minimize".to_string(),
            _ => "maximize".to_string(),
        };
        let obj_offset = scip_sys::SCIPgetOrigObjoffset(scip);

        scip_sys::SCIPfree(&mut scip);

        let model_name = strip_mps_suffix(original_name);

        Ok(build_model_response(
            variables, constraints, num_nz, obj_sense, obj_offset, model_name,
        ))
    }
}

pub fn extract_presolved_model_data_scip(
    path: &str,
    original_name: &str,
) -> Result<ModelResponse, String> {
    unsafe {
        let mut scip: *mut scip_sys::SCIP = std::ptr::null_mut();
        scip_sys::SCIPcreate(&mut scip);
        scip_sys::SCIPincludeDefaultPlugins(scip);
        scip_sys::SCIPsetIntParam(
            scip,
            CString::new("display/verblevel").unwrap().as_ptr(),
            0,
        );

        let c_path = CString::new(path).map_err(|e| format!("Invalid path: {}", e))?;
        let ret = scip_sys::SCIPreadProb(scip, c_path.as_ptr(), std::ptr::null());
        if ret != scip_sys::SCIP_Retcode_SCIP_OKAY {
            scip_sys::SCIPfree(&mut scip);
            return Err(format!("SCIP failed to read problem (code {})", ret));
        }

        let ret = scip_sys::SCIPpresolve(scip);
        if ret != scip_sys::SCIP_Retcode_SCIP_OKAY {
            scip_sys::SCIPfree(&mut scip);
            return Err(format!("SCIP presolve failed (code {})", ret));
        }

        let num_vars = scip_sys::SCIPgetNVars(scip) as usize;
        let vars_ptr = scip_sys::SCIPgetVars(scip);
        let num_conss = scip_sys::SCIPgetNConss(scip) as usize;
        let conss_ptr = scip_sys::SCIPgetConss(scip);

        let scip_vars = std::slice::from_raw_parts(vars_ptr, num_vars);
        let mut var_ptr_to_idx: std::collections::HashMap<*mut scip_sys::SCIP_VAR, usize> =
            std::collections::HashMap::with_capacity(num_vars);

        let mut variables = Vec::with_capacity(num_vars);
        for (i, &var) in scip_vars.iter().enumerate() {
            var_ptr_to_idx.insert(var, i);

            let name = CStr::from_ptr(scip_sys::SCIPvarGetName(var))
                .to_string_lossy()
                .into_owned();
            let lb = scip_sys::SCIPvarGetLbGlobal(var);
            let ub = scip_sys::SCIPvarGetUbGlobal(var);
            let obj = scip_sys::SCIPvarGetObj(var);
            let vtype = scip_sys::SCIPvarGetType(var);

            let var_type = match vtype {
                scip_sys::SCIP_Vartype_SCIP_VARTYPE_BINARY => VarType::Binary,
                scip_sys::SCIP_Vartype_SCIP_VARTYPE_INTEGER
                | scip_sys::SCIP_Vartype_SCIP_VARTYPE_IMPLINT => VarType::Integer,
                _ => VarType::Continuous,
            };

            variables.push(Variable {
                name,
                var_type,
                lower: finite_or_none(lb),
                upper: finite_or_none(ub),
                obj,
            });
        }

        let scip_conss = std::slice::from_raw_parts(conss_ptr, num_conss);
        let mut constraints = Vec::new();
        let mut num_nz = 0usize;

        for &cons in scip_conss {
            // Use generic constraint API that works across all handler types
            let mut nvars_int: c_int = 0;
            let mut success: u32 = 0;
            scip_sys::SCIPgetConsNVars(scip, cons, &mut nvars_int, &mut success);
            if success == 0 || nvars_int <= 0 {
                continue;
            }
            let nvars = nvars_int as usize;

            let mut cvars_buf: Vec<*mut scip_sys::SCIP_VAR> = vec![std::ptr::null_mut(); nvars];
            success = 0;
            scip_sys::SCIPgetConsVars(scip, cons, cvars_buf.as_mut_ptr(), nvars_int, &mut success);
            if success == 0 {
                continue;
            }

            let mut cvals_buf: Vec<f64> = vec![0.0; nvars];
            success = 0;
            scip_sys::SCIPgetConsVals(scip, cons, cvals_buf.as_mut_ptr(), nvars_int, &mut success);
            if success == 0 {
                continue;
            }

            success = 0;
            let lhs = scip_sys::SCIPconsGetLhs(scip, cons, &mut success);
            let lhs = if success != 0 { Some(lhs) } else { None };

            success = 0;
            let rhs = scip_sys::SCIPconsGetRhs(scip, cons, &mut success);
            let rhs = if success != 0 { Some(rhs) } else { None };

            let cons_name = CStr::from_ptr(scip_sys::SCIPconsGetName(cons))
                .to_string_lossy()
                .into_owned();

            let mut terms = Vec::with_capacity(nvars);
            for j in 0..nvars {
                if let Some(&var_idx) = var_ptr_to_idx.get(&cvars_buf[j]) {
                    let v = &variables[var_idx];
                    terms.push(Term {
                        var_index: var_idx,
                        var_name: v.name.clone(),
                        var_type: v.var_type,
                        coeff: cvals_buf[j],
                    });
                }
            }

            num_nz += terms.len();
            constraints.push(Constraint {
                name: cons_name,
                lower: lhs.and_then(|v| finite_or_none(v)),
                upper: rhs.and_then(|v| finite_or_none(v)),
                terms,
            });
        }

        let obj_sense = match scip_sys::SCIPgetObjsense(scip) {
            scip_sys::SCIP_Objsense_SCIP_OBJSENSE_MINIMIZE => "minimize".to_string(),
            _ => "maximize".to_string(),
        };
        let obj_offset = scip_sys::SCIPgetTransObjoffset(scip);

        scip_sys::SCIPfree(&mut scip);

        let model_name = format!("{} (presolved by SCIP)", strip_mps_suffix(original_name));

        Ok(build_model_response(
            variables, constraints, num_nz, obj_sense, obj_offset, model_name,
        ))
    }
}

pub fn extract_presolved_model_data(
    path: &str,
    original_name: &str,
) -> Result<ModelResponse, String> {
    use lio_highs::{ColProblem, LikeModel, Model, VarType as HighsVarType};

    let mut model = Model::new::<ColProblem>(ColProblem::default());
    model.make_quiet();
    model.read(path);
    model.presolve();

    let (
        num_cols,
        num_rows,
        num_nz,
        sense,
        offset,
        col_cost,
        col_lower,
        col_upper,
        row_lower,
        row_upper,
        row_data,
        integrality,
    ) = model.get_presolved_row_lp();

    // Try to get presolved column names; fall back to original names or generated names
    let col_names: Vec<String> = (0..num_cols)
        .map(|i| {
            model.get_col_name(i)
                .unwrap_or_else(|_| format!("x{}", i))
        })
        .collect();

    let mut variables = Vec::with_capacity(num_cols);
    for i in 0..num_cols {
        let var_type = match integrality[i] {
            HighsVarType::Integer if col_lower[i] == 0.0 && col_upper[i] == 1.0 => VarType::Binary,
            HighsVarType::Integer | HighsVarType::ImplicitInteger | HighsVarType::SemiInteger => {
                VarType::Integer
            }
            _ => VarType::Continuous,
        };
        variables.push(Variable {
            name: col_names[i].clone(),
            var_type,
            lower: finite_or_none(col_lower[i]),
            upper: finite_or_none(col_upper[i]),
            obj: col_cost[i],
        });
    }

    let mut constraints = Vec::with_capacity(num_rows);
    for i in 0..num_rows {
        let terms: Vec<Term> = row_data[i]
            .iter()
            .map(|&(col_idx, coeff)| {
                let v = &variables[col_idx];
                Term {
                    var_index: col_idx,
                    var_name: v.name.clone(),
                    var_type: v.var_type,
                    coeff,
                }
            })
            .collect();

        constraints.push(Constraint {
            name: format!("R{}", i),
            lower: finite_or_none(row_lower[i]),
            upper: finite_or_none(row_upper[i]),
            terms,
        });
    }

    let obj_sense = match sense {
        lio_highs::Sense::Minimise => "minimize".to_string(),
        lio_highs::Sense::Maximise => "maximize".to_string(),
    };

    let model_name = format!(
        "{} (presolved by HiGHS)",
        strip_mps_suffix(original_name)
    );

    Ok(build_model_response(
        variables, constraints, num_nz, obj_sense, offset, model_name,
    ))
}

pub fn extract_cliques(path: &str) -> Result<CliqueResponse, String> {
    use lio_highs::{ColProblem, LikeModel, Model, VarType as HighsVarType};

    let mut model = Model::new::<ColProblem>(ColProblem::default());
    model.make_quiet();
    model.read(path);
    model.presolve();

    if !model.has_cliques() {
        return Ok(CliqueResponse {
            cliques: Vec::new(),
            variables: Vec::new(),
            num_cliques: 0,
        });
    }

    let raw_cliques = model
        .get_cliques()
        .map_err(|e| format!("Failed to get cliques: {:?}", e))?;

    let (
        num_cols, _, _, _, _, col_cost, col_lower, col_upper, _, _, _, integrality,
    ) = model.get_presolved_row_lp();

    let col_names: Vec<String> = (0..num_cols)
        .map(|i| {
            model
                .get_col_name(i)
                .unwrap_or_else(|_| format!("x{}", i))
        })
        .collect();

    let variables: Vec<CliqueVariable> = (0..num_cols)
        .map(|i| {
            let var_type = match integrality[i] {
                HighsVarType::Integer if col_lower[i] == 0.0 && col_upper[i] == 1.0 => {
                    VarType::Binary
                }
                HighsVarType::Integer
                | HighsVarType::ImplicitInteger
                | HighsVarType::SemiInteger => VarType::Integer,
                _ => VarType::Continuous,
            };
            CliqueVariable {
                name: col_names[i].clone(),
                var_type,
                lower: finite_or_none(col_lower[i]),
                upper: finite_or_none(col_upper[i]),
                obj: col_cost[i],
            }
        })
        .collect();

    let num_cliques = raw_cliques.len();
    let cliques: Vec<Vec<CliqueEntry>> = raw_cliques
        .into_iter()
        .map(|c| {
            c.into_iter()
                .map(|(col, val)| CliqueEntry {
                    col: col as usize,
                    val,
                    var_name: None,
                    var_index: None,
                    value: None,
                })
                .collect()
        })
        .collect();

    Ok(CliqueResponse {
        cliques,
        variables,
        num_cliques,
    })
}

pub fn solve_root_lp(path: &str, presolved: bool) -> Result<model::LpSolutionResponse, String> {
    use lio_highs::{ColProblem, LikeModel, Model};

    let mut model = Model::new::<ColProblem>(ColProblem::default());
    model.make_quiet();
    model.read(path);

    if !presolved {
        // Relax all integer/binary variables to continuous and solve
        let num_cols = model.num_cols();
        for i in 0..num_cols {
            let _ = model.change_col_integrality(i, false);
        }

        let solved = model
            .try_solve()
            .map_err(|(status, _)| format!("LP solve failed: {:?}", status))?;

        let status = format!("{:?}", solved.status());
        let obj = solved.obj_val();
        let solution = solved.get_solution();

        Ok(model::LpSolutionResponse {
            status,
            objective_value: obj,
            col_values: solution.columns().to_vec(),
            row_values: solution.rows().to_vec(),
            dual_values: solution.dual_rows().to_vec(),
        })
    } else {
        // Presolve, then build a fresh LP from the presolved data and solve it
        model.presolve();

        let (
            num_cols, num_rows, _num_nz, sense, offset,
            col_cost, col_lower, col_upper,
            row_lower, row_upper, row_data,
            _integrality,
        ) = model.get_presolved_row_lp();

        // Build a new continuous LP model from presolved data
        let mut lp = Model::new::<ColProblem>(ColProblem::default());
        lp.make_quiet();
        lp.set_sense(sense);

        // Add columns (all continuous)
        for i in 0..num_cols {
            lp.add_col(col_cost[i], col_lower[i]..=col_upper[i], std::iter::empty::<(usize, f64)>());
        }

        // Add rows with coefficients
        for i in 0..num_rows {
            lp.add_row(row_lower[i]..=row_upper[i], row_data[i].iter().copied());
        }

        let solved = lp
            .try_solve()
            .map_err(|(status, _)| format!("Presolved LP solve failed: {:?}", status))?;

        let status = format!("{:?}", solved.status());
        let obj = solved.obj_val() + offset;
        let solution = solved.get_solution();

        Ok(model::LpSolutionResponse {
            status,
            objective_value: obj,
            col_values: solution.columns().to_vec(),
            row_values: solution.rows().to_vec(),
            dual_values: solution.dual_rows().to_vec(),
        })
    }
}

/// Solve the relaxation keeping only the given constraints (+ variable bounds).
/// If `lp_mode` is true, relax integrality first.
pub fn solve_constraint_subset(
    path: &str,
    keep_indices: &[usize],
    lp_mode: bool,
) -> Result<model::LpSolutionResponse, String> {
    use lio_highs::{ColProblem, LikeModel, Model};

    let mut model = Model::new::<ColProblem>(ColProblem::default());
    model.make_quiet();
    model.read(path);

    let num_rows = model.num_rows();
    let keep_set: std::collections::HashSet<usize> = keep_indices.iter().copied().collect();

    // Collect variable indices that appear in the kept constraints
    let (_, _, _, _, _, _, _, _, _, _, row_data, _) = model.get_row_lp();
    let mut used_cols: std::collections::HashSet<usize> = std::collections::HashSet::new();
    for &idx in keep_indices {
        if idx < row_data.len() {
            for &(col, _) in &row_data[idx] {
                used_cols.insert(col);
            }
        }
    }

    // Delete all rows not in the keep set
    let rows_to_delete: Vec<usize> = (0..num_rows)
        .filter(|r| !keep_set.contains(r))
        .collect();
    model.del_rows(rows_to_delete);

    // Fix variables not in kept constraints to zero (set bounds to [0,0])
    // This prevents unboundedness from free variables outside the constraint set
    let num_cols = model.num_cols();
    for i in 0..num_cols {
        if !used_cols.contains(&i) {
            let _ = model.change_col_bounds(i, 0.0..=0.0);
        }
    }

    // Disable presolve — for small subproblems, HiGHS presolve can reduce
    // the problem to trivial and not populate solution values correctly.
    model.set_option("presolve", "off");

    if lp_mode {
        let num_cols = model.num_cols();
        for i in 0..num_cols {
            let _ = model.change_col_integrality(i, false);
        }
    }

    let solved = model
        .try_solve()
        .map_err(|(status, _)| format!("Solve failed: {:?}", status))?;

    let status = format!("{:?}", solved.status());
    let obj = solved.obj_val();
    let solution = solved.get_solution();

    Ok(model::LpSolutionResponse {
        status,
        objective_value: obj,
        col_values: solution.columns().to_vec(),
        row_values: solution.rows().to_vec(),
        dual_values: solution.dual_rows().to_vec(),
    })
}

pub fn extract_reductions(path: &str) -> Result<ReductionsResponse, String> {
    use lio_highs::{ColProblem, Model};

    let mut model = Model::new::<ColProblem>(ColProblem::default());
    model.make_quiet();
    model.read(path);
    model.presolve();

    let reductions = model
        .get_presolve_reductions()
        .map_err(|e| format!("Failed to get reductions: {:?}", e))?;

    let items = reductions
        .iter()
        .map(|r| PresolveReductionItem {
            reduction_type: r.reduction_type.to_string(),
            col: r.col,
            row: r.row,
            value: r.value,
            source: r.source.to_string(),
            description: r.to_string(),
        })
        .collect();

    Ok(ReductionsResponse { reductions: items })
}

pub fn extract_cliques_implications_highs(path: &str) -> Result<CliquesImplicationsResponse, String> {
    use lio_highs::{ColProblem, ImplicationBoundType, Model};

    let mut model = Model::new::<ColProblem>(ColProblem::default());
    model.make_quiet();
    model.read(path);
    model.presolve();

    let (num_cols, _, _, _, _, _, col_lower, col_upper, _, _, _, integrality) =
        model.get_presolved_row_lp();

    let col_names: Vec<String> = (0..num_cols).map(|i| format!("x{}", i)).collect();

    let is_binary: Vec<bool> = (0..num_cols)
        .map(|i| {
            matches!(integrality[i], lio_highs::VarType::Integer)
                && col_lower[i] == 0.0
                && col_upper[i] == 1.0
        })
        .collect();

    // Extract cliques
    let mut cliques = Vec::new();
    if model.has_cliques() {
        if let Ok(raw_cliques) = model.get_cliques() {
            for (id, members) in raw_cliques.into_iter().enumerate() {
                let entries: Vec<CliqueEntry> = members
                    .iter()
                    .map(|&(col, val)| CliqueEntry {
                        col,
                        val,
                        var_name: Some(col_names
                            .get(col)
                            .cloned()
                            .unwrap_or_else(|| format!("x{}", col))),
                        var_index: Some(col),
                        value: Some(val),
                    })
                    .collect();
                cliques.push(Clique {
                    id,
                    is_equation: false,
                    members: entries,
                });
            }
        }
    }

    // Extract implications
    let mut implications = Vec::new();
    if model.has_implications() {
        for col in 0..num_cols {
            if !is_binary[col] {
                continue;
            }
            for val in [false, true] {
                if let Ok(impls) = model.get_implications(col, val) {
                    for imp in impls {
                        implications.push(ImplicationEntry {
                            from_var_name: col_names[col].clone(),
                            from_var_index: col,
                            from_value: val,
                            to_var_name: col_names
                                .get(imp.column)
                                .cloned()
                                .unwrap_or_else(|| format!("x{}", imp.column)),
                            to_var_index: imp.column,
                            bound_type: match imp.bound_type {
                                ImplicationBoundType::Lower => "lower".to_string(),
                                ImplicationBoundType::Upper => "upper".to_string(),
                            },
                            bound_value: imp.bound_value,
                        });
                    }
                }
            }
        }
    }

    Ok(CliquesImplicationsResponse {
        solver: "highs".to_string(),
        num_cliques: cliques.len(),
        num_implications: implications.len(),
        cliques,
        implications,
    })
}

pub fn extract_cliques_implications_scip(path: &str) -> Result<CliquesImplicationsResponse, String> {
    unsafe {
        let mut scip: *mut scip_sys::SCIP = std::ptr::null_mut();
        scip_sys::SCIPcreate(&mut scip);
        scip_sys::SCIPincludeDefaultPlugins(scip);
        scip_sys::SCIPsetIntParam(
            scip,
            CString::new("display/verblevel").unwrap().as_ptr(),
            0,
        );

        let c_path = CString::new(path).map_err(|e| format!("Invalid path: {}", e))?;
        let ret = scip_sys::SCIPreadProb(scip, c_path.as_ptr(), std::ptr::null());
        if ret != scip_sys::SCIP_Retcode_SCIP_OKAY {
            scip_sys::SCIPfree(&mut scip);
            return Err(format!("SCIP failed to read problem (code {})", ret));
        }

        let ret = scip_sys::SCIPpresolve(scip);
        if ret != scip_sys::SCIP_Retcode_SCIP_OKAY {
            scip_sys::SCIPfree(&mut scip);
            return Err(format!("SCIP presolve failed (code {})", ret));
        }

        // Build var pointer -> (index, name) map for transformed vars
        let num_vars = scip_sys::SCIPgetNVars(scip) as usize;
        let vars_ptr = scip_sys::SCIPgetVars(scip);
        let scip_vars = std::slice::from_raw_parts(vars_ptr, num_vars);

        let mut var_ptr_to_idx: std::collections::HashMap<*mut scip_sys::SCIP_VAR, usize> =
            std::collections::HashMap::with_capacity(num_vars);
        let mut var_names: Vec<String> = Vec::with_capacity(num_vars);

        for (i, &var) in scip_vars.iter().enumerate() {
            var_ptr_to_idx.insert(var, i);
            let name = CStr::from_ptr(scip_sys::SCIPvarGetName(var))
                .to_string_lossy()
                .into_owned();
            var_names.push(name);
        }

        // Extract cliques from clique table
        let num_cliques = scip_sys::SCIPgetNCliques(scip) as usize;
        let cliques_ptr = scip_sys::SCIPgetCliques(scip);
        let scip_cliques = if num_cliques > 0 && !cliques_ptr.is_null() {
            std::slice::from_raw_parts(cliques_ptr, num_cliques)
        } else {
            &[]
        };

        let mut cliques = Vec::with_capacity(num_cliques);
        for (id, &clq) in scip_cliques.iter().enumerate() {
            let nvars = scip_sys::SCIPcliqueGetNVars(clq) as usize;
            let cvars = scip_sys::SCIPcliqueGetVars(clq);
            let cvals = scip_sys::SCIPcliqueGetValues(clq);
            let is_eq = scip_sys::SCIPcliqueIsEquation(clq) != 0;

            let cvars_slice = std::slice::from_raw_parts(cvars, nvars);
            let cvals_slice = std::slice::from_raw_parts(cvals, nvars);

            let mut members = Vec::with_capacity(nvars);
            for j in 0..nvars {
                if let Some(&idx) = var_ptr_to_idx.get(&cvars_slice[j]) {
                    members.push(CliqueEntry {
                        col: idx,
                        val: cvals_slice[j] != 0,
                        var_name: Some(var_names[idx].clone()),
                        var_index: Some(idx),
                        value: Some(cvals_slice[j] != 0),
                    });
                }
            }

            cliques.push(Clique {
                id,
                is_equation: is_eq,
                members,
            });
        }

        // Extract implications
        let total_implications = scip_sys::SCIPgetNImplications(scip) as usize;
        let mut implications = Vec::new();

        for (i, &var) in scip_vars.iter().enumerate() {
            let vtype = scip_sys::SCIPvarGetType(var);
            if vtype != scip_sys::SCIP_Vartype_SCIP_VARTYPE_BINARY {
                continue;
            }

            for fix in [0u32, 1u32] {
                let nimps = scip_sys::SCIPvarGetNImpls(var, fix) as usize;
                if nimps == 0 {
                    continue;
                }
                let imp_vars = scip_sys::SCIPvarGetImplVars(var, fix);
                let imp_types = scip_sys::SCIPvarGetImplTypes(var, fix);
                let imp_bounds = scip_sys::SCIPvarGetImplBounds(var, fix);
                if imp_vars.is_null() || imp_types.is_null() || imp_bounds.is_null() {
                    continue;
                }

                let imp_vars_slice = std::slice::from_raw_parts(imp_vars, nimps);
                let imp_types_slice: &[u32] =
                    std::slice::from_raw_parts(imp_types as *const u32, nimps);
                let imp_bounds_slice = std::slice::from_raw_parts(imp_bounds, nimps);

                for j in 0..nimps {
                    if let Some(&to_idx) = var_ptr_to_idx.get(&imp_vars_slice[j]) {
                        let bt =
                            if imp_types_slice[j] == scip_sys::SCIP_BoundType_SCIP_BOUNDTYPE_LOWER
                            {
                                "lower"
                            } else {
                                "upper"
                            };
                        implications.push(ImplicationEntry {
                            from_var_name: var_names[i].clone(),
                            from_var_index: i,
                            from_value: fix != 0,
                            to_var_name: var_names[to_idx].clone(),
                            to_var_index: to_idx,
                            bound_type: bt.to_string(),
                            bound_value: imp_bounds_slice[j],
                        });
                    }
                }
            }
        }

        scip_sys::SCIPfree(&mut scip);

        Ok(CliquesImplicationsResponse {
            solver: "scip".to_string(),
            num_cliques: cliques.len(),
            num_implications: if implications.is_empty() {
                total_implications
            } else {
                implications.len()
            },
            cliques,
            implications,
        })
    }
}

pub fn extract_symmetry_scip(path: &str) -> Result<SymmetryResponse, String> {
    unsafe {
        let mut scip: *mut scip_sys::SCIP = std::ptr::null_mut();
        scip_sys::SCIPcreate(&mut scip);
        scip_sys::SCIPincludeDefaultPlugins(scip);
        scip_sys::SCIPsetIntParam(
            scip,
            CString::new("display/verblevel").unwrap().as_ptr(),
            0,
        );

        let c_path = CString::new(path).map_err(|e| format!("Invalid path: {}", e))?;
        let ret = scip_sys::SCIPreadProb(scip, c_path.as_ptr(), std::ptr::null());
        if ret != scip_sys::SCIP_Retcode_SCIP_OKAY {
            scip_sys::SCIPfree(&mut scip);
            return Err(format!("SCIP failed to read problem (code {})", ret));
        }

        let ret = scip_sys::SCIPpresolve(scip);
        if ret != scip_sys::SCIP_Retcode_SCIP_OKAY {
            scip_sys::SCIPfree(&mut scip);
            return Err(format!("SCIP presolve failed (code {})", ret));
        }

        let mut npermvars: c_int = 0;
        let mut permvars: *mut *mut scip_sys::SCIP_VAR = std::ptr::null_mut();
        let mut nperms: c_int = 0;
        let mut perms: *mut *mut c_int = std::ptr::null_mut();
        let mut log10groupsize: f64 = 0.0;
        let mut binvaraffected: u32 = 0;
        let mut components: *mut c_int = std::ptr::null_mut();
        let mut componentbegins: *mut c_int = std::ptr::null_mut();
        let mut vartocomponent: *mut c_int = std::ptr::null_mut();
        let mut ncomponents: c_int = 0;

        let ret = scip_sys::SCIPgetSymmetry(
            scip,
            &mut npermvars,
            &mut permvars,
            std::ptr::null_mut(), // permvarmap - not needed
            &mut nperms,
            &mut perms,
            std::ptr::null_mut(), // permstrans - not needed
            &mut log10groupsize,
            &mut binvaraffected,
            &mut components,
            &mut componentbegins,
            &mut vartocomponent,
            &mut ncomponents,
        );

        if ret != scip_sys::SCIP_Retcode_SCIP_OKAY {
            scip_sys::SCIPfree(&mut scip);
            return Err(format!("SCIPgetSymmetry failed (code {})", ret));
        }

        // Build var name lookup for permvars
        let permvar_names: Vec<String> = if npermvars > 0 && !permvars.is_null() {
            let pv = std::slice::from_raw_parts(permvars, npermvars as usize);
            pv.iter()
                .map(|&var| {
                    CStr::from_ptr(scip_sys::SCIPvarGetName(var))
                        .to_string_lossy()
                        .into_owned()
                })
                .collect()
        } else {
            Vec::new()
        };

        // Extract component structure
        let mut sym_components = Vec::new();
        if ncomponents > 0 && !components.is_null() && !componentbegins.is_null() {
            let comp_begins =
                std::slice::from_raw_parts(componentbegins, (ncomponents + 1) as usize);
            let comp_generators =
                std::slice::from_raw_parts(components, comp_begins[ncomponents as usize] as usize);

            // For each component, collect the affected variable indices
            // by looking at which permvars are moved by the generators in this component
            for c in 0..ncomponents as usize {
                let gen_start = comp_begins[c] as usize;
                let gen_end = comp_begins[c + 1] as usize;

                let mut affected_vars: std::collections::BTreeSet<usize> =
                    std::collections::BTreeSet::new();

                if nperms > 0 && !perms.is_null() {
                    let perms_slice = std::slice::from_raw_parts(perms, nperms as usize);
                    for g in gen_start..gen_end {
                        let gen_idx = comp_generators[g] as usize;
                        if gen_idx < nperms as usize {
                            let perm =
                                std::slice::from_raw_parts(perms_slice[gen_idx], npermvars as usize);
                            for i in 0..npermvars as usize {
                                if perm[i] != i as c_int {
                                    affected_vars.insert(i);
                                }
                            }
                        }
                    }
                }

                let var_indices: Vec<usize> = affected_vars.into_iter().collect();
                let var_names: Vec<String> = var_indices
                    .iter()
                    .map(|&i| {
                        permvar_names
                            .get(i)
                            .cloned()
                            .unwrap_or_else(|| format!("x{}", i))
                    })
                    .collect();

                sym_components.push(SymmetryComponent {
                    id: c,
                    var_indices,
                    var_names,
                });
            }
        }

        let resp = SymmetryResponse {
            num_generators: nperms,
            num_permvars: npermvars,
            num_components: ncomponents,
            log10_group_size: log10groupsize,
            bin_var_affected: binvaraffected != 0,
            components: sym_components,
        };

        scip_sys::SCIPfree(&mut scip);
        Ok(resp)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_mps_from_str() {
        let mps = "NAME          test
ROWS
 N  OBJ
 L  C1
 G  C2
COLUMNS
    x1  OBJ  1.0  C1  1.0
    x1  C2  2.0
    MARKER  'MARKER'  'INTORG'
    x2  OBJ  2.0  C1  3.0
    x2  C2  1.0
    MARKER  'MARKER'  'INTEND'
RHS
    RHS  C1  10.0  C2  5.0
BOUNDS
 UP  BND  x1  100.0
 UP  BND  x2  1.0
ENDATA
";
        let resp = extract_model_data_from_str(mps, "test_model.mps").unwrap();
        assert_eq!(resp.name, "test_model");
        assert_eq!(resp.obj_sense, "minimize");
        assert_eq!(resp.stats.num_vars, 2);
        assert_eq!(resp.stats.num_constraints, 2);
        assert_eq!(resp.stats.num_nonzeros, 4);
        let x1 = resp.variables.iter().find(|v| v.name == "x1").unwrap();
        let x2 = resp.variables.iter().find(|v| v.name == "x2").unwrap();
        assert_eq!(x1.var_type, VarType::Continuous);
        assert_eq!(x2.var_type, VarType::Binary);
    }

    #[test]
    fn test_presolved_has_col_names() {
        let path = "static/instances/dcmulti.mps.gz";
        if !std::path::Path::new(path).exists() {
            eprintln!("Skipping test: {} not found", path);
            return;
        }
        let resp = extract_presolved_model_data(path, "dcmulti.mps.gz").unwrap();
        eprintln!("Presolved: {} vars, first 5 names: {:?}",
            resp.variables.len(),
            resp.variables.iter().take(5).map(|v| &v.name).collect::<Vec<_>>());
        // dcmulti has names like D111, not x0/x1
        assert!(resp.variables.iter().any(|v| v.name.starts_with('D')),
            "Expected original variable names like D111 in presolved model");
    }

    #[test]
    fn test_single_constraint_solve() {
        // Test with a simple set partitioning: x1 + x2 + x3 = 1, all binary
        let mps = "NAME          test
ROWS
 N  OBJ
 E  SP1
COLUMNS
    MARKER  'MARKER'  'INTORG'
    x1  OBJ  1.0  SP1  1.0
    x2  OBJ  2.0  SP1  1.0
    x3  OBJ  3.0  SP1  1.0
    MARKER  'MARKER'  'INTEND'
RHS
    RHS  SP1  1.0
BOUNDS
 UP  BND  x1  1.0
 UP  BND  x2  1.0
 UP  BND  x3  1.0
ENDATA
";
        let path = "/tmp/test_single_cons.mps";
        std::fs::write(path, mps).unwrap();

        let result = solve_constraint_subset(path, &[0], true).unwrap();
        eprintln!("Status: {}", result.status);
        eprintln!("Obj: {}", result.objective_value);
        eprintln!("Col values: {:?}", result.col_values);
        assert_eq!(result.status, "Optimal");
        // x1 should be 1 (cheapest), x2=x3=0
        assert!((result.objective_value - 1.0).abs() < 1e-6, "obj should be 1, got {}", result.objective_value);
        assert!((result.col_values[0] - 1.0).abs() < 1e-6, "x1 should be 1, got {}", result.col_values[0]);
    }

    #[test]
    fn test_single_constraint_real_instance() {
        let path = "static/instances/markshare_4_0.mps.gz";
        if !std::path::Path::new(path).exists() {
            eprintln!("Skipping: {} not found", path);
            return;
        }
        // Parse to find an equality constraint
        let model_data = extract_model_data(path, "markshare_4_0.mps.gz").unwrap();
        let eq_idx = model_data.constraints.iter().position(|c| {
            c.lower.is_some() && c.upper.is_some()
                && (c.lower.unwrap() - c.upper.unwrap()).abs() < 1e-10
                && !c.terms.is_empty()
        });
        if let Some(idx) = eq_idx {
            let c = &model_data.constraints[idx];
            eprintln!("Testing constraint {} (idx {}): {} terms, rhs={:?}",
                c.name, idx, c.terms.len(), c.lower);
            let result = solve_constraint_subset(path, &[idx], true).unwrap();
            eprintln!("Status: {}, Obj: {}", result.status, result.objective_value);
            let nz: Vec<_> = c.terms.iter()
                .filter(|t| result.col_values[t.var_index].abs() > 1e-10)
                .map(|t| format!("{}={}", t.var_name, result.col_values[t.var_index]))
                .collect();
            eprintln!("Nonzero vars in constraint: {:?}", nz);
            // For an equality with RHS != 0, at least one variable must be nonzero
            if c.lower.unwrap().abs() > 1e-10 {
                assert!(!nz.is_empty(), "Equality constraint with nonzero RHS should have nonzero solution values");
            }
        }
    }

    #[test]
    fn test_numnom_vs_highs_row_order() {
        use lio_highs::{ColProblem, LikeModel, Model};

        let path = "static/instances/markshare_4_0.mps.gz";
        if !std::path::Path::new(path).exists() {
            eprintln!("Skipping: {} not found", path);
            return;
        }

        // numnom row names (what frontend shows)
        let numnom_data = extract_model_data(path, "markshare_4_0.mps.gz").unwrap();
        let numnom_names: Vec<&str> = numnom_data.constraints.iter().map(|c| c.name.as_str()).collect();

        // HiGHS row order (what solve_constraint_subset uses)
        let mut model = Model::new::<ColProblem>(ColProblem::default());
        model.make_quiet();
        model.read(path);
        let (_, num_rows, _, _, _, _, _, _, row_lower, row_upper, row_data, _) = model.get_row_lp();

        eprintln!("numnom rows: {}, HiGHS rows: {}", numnom_names.len(), num_rows);
        eprintln!("numnom first 5: {:?}", &numnom_names[..5.min(numnom_names.len())]);
        eprintln!("numnom row 0 terms: {}", numnom_data.constraints[0].terms.len());
        eprintln!("HiGHS row 0 terms: {}", row_data[0].len());
        eprintln!("numnom row 0 RHS: {:?}/{:?}", numnom_data.constraints[0].lower, numnom_data.constraints[0].upper);
        eprintln!("HiGHS row 0 bounds: {}/{}", row_lower[0], row_upper[0]);
    }

    #[test]
    fn test_scip_parse_mps() {
        let mps = r#"NAME          test
ROWS
 N  OBJ
 L  C1
 G  C2
COLUMNS
    x1  OBJ  1.0  C1  1.0
    x1  C2  2.0
    MARKER  'MARKER'  'INTORG'
    x2  OBJ  2.0  C1  3.0
    x2  C2  1.0
    MARKER  'MARKER'  'INTEND'
RHS
    RHS  C1  10.0  C2  5.0
BOUNDS
 UP  BND  x1  100.0
 UP  BND  x2  1.0
ENDATA
"#;
        let path = "/tmp/test_scip_parse.mps";
        std::fs::write(path, mps).unwrap();

        let resp = extract_model_data(path, "test_model.mps").unwrap();
        assert_eq!(resp.name, "test_model");
        assert_eq!(resp.obj_sense, "minimize");
        assert_eq!(resp.stats.num_vars, 2);
        assert_eq!(resp.stats.num_constraints, 2);
        assert_eq!(resp.stats.num_nonzeros, 4);
        let x1 = resp.variables.iter().find(|v| v.name == "x1").unwrap();
        let x2 = resp.variables.iter().find(|v| v.name == "x2").unwrap();
        assert_eq!(x1.var_type, VarType::Continuous);
        assert_eq!(x2.var_type, VarType::Binary);
    }

}

use std::{
    any::Any,
    collections::BTreeMap,
    fs,
    io::{BufRead, BufReader, BufWriter, Write},
    panic::{AssertUnwindSafe, catch_unwind},
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::{Arc, Mutex, mpsc::Sender},
    thread,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use lio_highs::{ColProblem, ImplicationBoundType, LikeModel, Sense};

use crate::{
    ConstraintRow, Model, Term, Variable, VariableKind, classify_constraint,
    components::ComponentAnalysis,
};

const SOLVE_TIME_LIMIT_SECONDS: f64 = 60.0;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SolverBackend {
    Highs,
    Scip,
}

impl SolverBackend {
    pub fn label(self) -> &'static str {
        match self {
            Self::Highs => "HiGHS",
            Self::Scip => "SCIP",
        }
    }

    pub fn next(self) -> Self {
        match self {
            Self::Highs => Self::Scip,
            Self::Scip => Self::Highs,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SolveMode {
    Relaxation,
    Mip,
}

impl SolveMode {
    pub fn label(self) -> &'static str {
        match self {
            Self::Relaxation => "LP relaxation",
            Self::Mip => "MIP",
        }
    }
}

pub struct PresolveReport {
    pub backend: SolverBackend,
    pub model: Model,
    pub reductions: usize,
    pub reduction_types: Vec<(String, usize)>,
    pub reduction_items: Vec<ReductionItem>,
    pub elapsed: Duration,
}

/// A single presolve reduction from the HiGHS postsolve stack, recorded in the
/// order it was applied so the matrix stepper can replay them one at a time.
#[derive(Debug, Clone, PartialEq)]
pub struct ReductionItem {
    pub reduction_type: String,
    /// Affected column in the original model (-1 if N/A).
    pub col: i32,
    /// Affected row in the original model (-1 if N/A).
    pub row: i32,
    /// Key numeric value (fix value, scale, rhs, …).
    pub value: f64,
    /// The presolve rule that produced this reduction.
    pub source: String,
    /// Human-readable description.
    pub description: String,
}

pub struct SolveReport {
    pub backend: SolverBackend,
    pub mode: SolveMode,
    pub status: String,
    pub objective: f64,
    pub iterations: i32,
    pub values: Vec<f64>,
    pub elapsed: Duration,
}

/// One clique (a set of pairwise-incompatible binary assignments) found during
/// presolve. Members are `(presolved column index, value)`.
#[derive(Debug, Clone)]
pub struct Clique {
    pub id: usize,
    pub is_equation: bool,
    pub members: Vec<(usize, bool)>,
}

/// One implication `(from_col = from_value) ⇒ (to_col bound)` discovered by
/// presolve, in presolved column space.
#[derive(Debug, Clone)]
pub struct Implication {
    pub from_col: usize,
    pub from_value: bool,
    pub to_col: usize,
    pub bound_type: String,
    pub bound_value: f64,
}

/// Combined presolve-time analysis (cliques, implications, symmetry) produced
/// by a single in-process HiGHS run.
#[derive(Debug, Clone)]
pub struct AnalysisReport {
    pub cliques: Vec<Clique>,
    pub num_implications: usize,
    pub implications: Vec<Implication>,
    pub symmetry_generators: usize,
    pub symmetry_perm_vars: usize,
    pub symmetry_components: Vec<Vec<usize>>,
    pub symmetry_binary_affected: bool,
    pub symmetry_log10_group_size: f64,
    /// Presolved column names (`x{index}`), matching the presolved model.
    pub variable_names: Vec<String>,
    /// Original variable names, matching the symmetry components (which are
    /// in original-model column space).
    pub symmetry_names: Vec<String>,
    pub elapsed: Duration,
}

/// Root-node cut statistics parsed from a solver's MIP log.
#[derive(Debug, Clone)]
pub struct RootCutStats {
    pub backend: SolverBackend,
    /// Cumulative cuts added at the root node.
    pub total_cuts: usize,
    /// Separation rounds performed at the root.
    pub separation_rounds: usize,
    /// Cuts retained in the root LP.
    pub cuts_in_lp: usize,
    /// Final number of LP rows at the root.
    pub lp_rows: usize,
}

pub enum SolverEvent {
    Log(String),
    PresolveFinished(Result<PresolveReport, String>),
    SolveFinished(Result<SolveReport, String>),
    ReductionsFinished(Result<Vec<ReductionItem>, String>),
    AnalysisFinished(Result<AnalysisReport, String>),
    RootCutsFinished(Result<RootCutStats, String>),
}

struct ChannelLogger;

static LOGGER: ChannelLogger = ChannelLogger;
static LOG_SENDER: Mutex<Option<Sender<SolverEvent>>> = Mutex::new(None);
/// Optional capture hook that receives every log line, used to collect a copy
/// of a HiGHS run's output for parsing (e.g. root-cut stats).
static LOG_CAPTURE: Mutex<Option<Box<dyn FnMut(String) + Send>>> = Mutex::new(None);

impl log::Log for ChannelLogger {
    fn enabled(&self, _metadata: &log::Metadata<'_>) -> bool {
        true
    }

    fn log(&self, record: &log::Record<'_>) {
        let message = record.args().to_string();
        let Ok(guard) = LOG_SENDER.lock() else {
            return;
        };
        let sender = guard.as_ref();
        let mut capture = LOG_CAPTURE.lock().unwrap_or_else(|poison| poison.into_inner());
        for line in message.lines().filter(|line| !line.trim().is_empty()) {
            let line = line.strip_prefix("HiGHS: ").unwrap_or(line).to_owned();
            if let Some(sender) = sender {
                let _ = sender.send(SolverEvent::Log(line.clone()));
            }
            if let Some(hook) = capture.as_mut() {
                hook(line);
            }
        }
    }

    fn flush(&self) {}
}

pub fn init_logging() {
    let _ = log::set_logger(&LOGGER);
    log::set_max_level(log::LevelFilter::Trace);
}

pub fn start_highs_presolve(model: Arc<Model>, params: String, sender: Sender<SolverEvent>) {
    thread::spawn(move || {
        install_sender(sender.clone());
        let result = catch_unwind(AssertUnwindSafe(|| run_presolve(&model, &params)))
            .map_err(panic_message)
            .and_then(|result| result);
        clear_sender();
        let _ = sender.send(SolverEvent::PresolveFinished(result));
    });
}

pub fn start_scip_presolve(
    model: Arc<Model>,
    source_path: String,
    params: String,
    sender: Sender<SolverEvent>,
) {
    thread::spawn(move || {
        let result = catch_unwind(AssertUnwindSafe(|| {
            run_scip_presolve(&model, &source_path, &params, &sender)
        }))
        .map_err(panic_message)
        .and_then(|result| result);
        let _ = sender.send(SolverEvent::PresolveFinished(result));
    });
}

pub fn start_highs_solve(model: Arc<Model>, mode: SolveMode, params: String, sender: Sender<SolverEvent>) {
    thread::spawn(move || {
        install_sender(sender.clone());
        let result = catch_unwind(AssertUnwindSafe(|| run_solve(&model, mode, &params)))
            .map_err(panic_message)
            .and_then(|result| result);
        clear_sender();
        let _ = sender.send(SolverEvent::SolveFinished(result));
    });
}

pub fn start_scip_solve(
    model: Arc<Model>,
    mode: SolveMode,
    params: String,
    sender: Sender<SolverEvent>,
) {
    thread::spawn(move || {
        let result = catch_unwind(AssertUnwindSafe(|| {
            run_scip_solve(&model, mode, &params, &sender)
        }))
        .map_err(panic_message)
        .and_then(|result| result);
        let _ = sender.send(SolverEvent::SolveFinished(result));
    });
}

/// Run HiGHS presolve in the background purely to capture the ordered reduction
/// stack for the matrix stepper. Always works regardless of which backend did
/// the main presolve.
pub fn start_highs_reductions(model: Arc<Model>, sender: Sender<SolverEvent>) {
    thread::spawn(move || {
        let result = catch_unwind(AssertUnwindSafe(|| run_highs_reductions(&model)))
            .map_err(panic_message)
            .and_then(|result| result);
        let _ = sender.send(SolverEvent::ReductionsFinished(result));
    });
}

fn run_highs_reductions(source: &Model) -> Result<Vec<ReductionItem>, String> {
    let mut model = build_highs_model(source, false)?;
    model.presolve();
    let items = model
        .get_presolve_reductions()
        .map_err(|error| format!("failed to read presolve reductions: {error:?}"))?;
    Ok(items
        .into_iter()
        .map(|item| ReductionItem {
            reduction_type: item.reduction_type.to_string(),
            col: item.col,
            row: item.row,
            value: item.value,
            source: item.source.to_string(),
            description: item.to_string(),
        })
        .collect())
}

/// Combined cliques + implications + symmetry analysis via in-process HiGHS.
pub fn start_highs_analysis(model: Arc<Model>, sender: Sender<SolverEvent>) {
    thread::spawn(move || {
        install_sender(sender.clone());
        let result = catch_unwind(AssertUnwindSafe(|| run_highs_analysis(&model)))
            .map_err(panic_message)
            .and_then(|result| result);
        clear_sender();
        let _ = sender.send(SolverEvent::AnalysisFinished(result));
    });
}

fn run_highs_analysis(source: &Model) -> Result<AnalysisReport, String> {
    let started = Instant::now();
    let mut model = build_highs_model(source, false)?;
    model.presolve();

    let (num_cols, _, _, _, _, _, col_lower, col_upper, _, _, _, integrality) =
        model.get_presolved_row_lp();
    let variable_names: Vec<String> = (0..num_cols)
        .map(|index| format!("x{index}"))
        .collect();

    // ── Cliques (from set-packing/partitioning rows) ──────────────
    // HiGHS's clique table is not populated by this build's presolve, so we
    // extract cliques directly: a row of all-binary unit-coefficient
    // variables with RHS ≤ 1 (or = 1) makes its support set a clique.
    let cliques = extract_row_cliques(source);
    let symmetry_names = source.variables.iter().map(|v| v.name.clone()).collect();

    // ── Implications ───────────────────────────────────────────────
    let mut implications = Vec::new();
    let mut is_binary = vec![false; num_cols];
    for index in 0..num_cols {
        is_binary[index] = matches!(integrality[index], lio_highs::VarType::Integer)
            && col_lower[index] == 0.0
            && col_upper[index] == 1.0;
    }
    if model.has_implications() {
        for col in 0..num_cols {
            if !is_binary[col] {
                continue;
            }
            for value in [false, true] {
                if let Ok(impls) = model.get_implications(col, value) {
                    for imp in impls {
                        implications.push(Implication {
                            from_col: col,
                            from_value: value,
                            to_col: imp.column as usize,
                            bound_type: match imp.bound_type {
                                ImplicationBoundType::Lower => "lower".to_owned(),
                                ImplicationBoundType::Upper => "upper".to_owned(),
                            },
                            bound_value: imp.bound_value,
                        });
                    }
                }
            }
        }
    }

    // ── Symmetry (structural: identical columns) ───────────────────
    // HiGHS's C symmetry API is unreliable in this build, so we detect the
    // dominant class of symmetry directly: groups of variables with identical
    // coefficients, bounds, objective, and integrality are interchangeable.
    let (symmetry_generators, symmetry_perm_vars, symmetry_components,
         symmetry_binary_affected, symmetry_log10_group_size) =
        detect_twin_symmetry(source);

    Ok(AnalysisReport {
        cliques,
        num_implications: implications.len(),
        implications,
        symmetry_generators,
        symmetry_perm_vars,
        symmetry_components,
        symmetry_binary_affected,
        symmetry_log10_group_size,
        variable_names,
        symmetry_names,
        elapsed: started.elapsed(),
    })
}

/// Detect structural twin symmetry: variables that are indistinguishable (same
/// coefficient pattern across every constraint, same bounds, same objective,
/// same integrality) form a symmetry group — swapping any two of them leaves
/// the model unchanged. Each group contributes `size - 1` adjacent-transposition
/// generators and `log10(size!)` to the group size.
fn detect_twin_symmetry(source: &Model) -> (usize, usize, Vec<Vec<usize>>, bool, f64) {
    let mut signatures = BTreeMap::<Vec<u64>, Vec<usize>>::new();
    for (index, variable) in source.variables.iter().enumerate() {
        let mut terms = Vec::new();
        for (row_index, row) in source.constraints.iter().enumerate() {
            for term in &row.terms {
                if term.column == index {
                    terms.push((row_index, term.coefficient));
                }
            }
        }
        terms.sort_by(|left, right| left.0.cmp(&right.0));
        let mut signature = Vec::with_capacity(terms.len() * 2 + 8);
        signature.push(match variable.kind {
            VariableKind::Continuous => 0.0f64,
            VariableKind::Integer => 1.0f64,
            VariableKind::Binary => 2.0f64,
        }
        .to_bits());
        signature.push(variable.lower.to_bits());
        signature.push(variable.upper.to_bits());
        signature.push(variable.objective.to_bits());
        for (row, coefficient) in terms {
            signature.push(row as u64);
            signature.push(coefficient.to_bits());
        }
        signatures.entry(signature).or_default().push(index);
    }

    let mut components = signatures
        .into_values()
        .filter(|group| group.len() >= 2)
        .collect::<Vec<_>>();
    components.sort_by_key(|group| std::cmp::Reverse(group.len()));

    let perm_vars = components.iter().map(Vec::len).sum();
    let generators = components.iter().map(|group| group.len() - 1).sum();
    let log10_group_size = components
        .iter()
        .map(|group| log10_factorial(group.len()))
        .sum();
    let binary_affected = components.iter().any(|group| {
        group
            .iter()
            .any(|&index| source.variables[index].kind == VariableKind::Binary)
    });
    (generators, perm_vars, components, binary_affected, log10_group_size)
}

/// Extract cliques from set-packing / set-partitioning rows: a row whose
/// variables are all binary with unit coefficients and RHS `≤ 1` (or `= 1`)
/// means no two of its variables can both be 1, so its support set is a
/// clique. Results are in original-model column space.
fn extract_row_cliques(source: &Model) -> Vec<Clique> {
    const EPSILON: f64 = 1e-10;
    let mut seen = std::collections::HashSet::<Vec<usize>>::new();
    let mut cliques = Vec::new();
    for row in &source.constraints {
        if row.terms.len() < 2 {
            continue;
        }
        let mut support = Vec::new();
        let mut all_binary = true;
        for term in &row.terms {
            if source.variables[term.column].kind != VariableKind::Binary
                || (term.coefficient.abs() - 1.0).abs() > EPSILON
            {
                all_binary = false;
                break;
            }
            support.push(term.column);
        }
        if !all_binary {
            continue;
        }
        support.sort_unstable();
        support.dedup();
        let is_equation =
            row.lower.is_finite() && row.upper.is_finite() && (row.lower - row.upper).abs() <= EPSILON;
        let upper_one = row.upper.is_finite() && (row.upper - 1.0).abs() <= EPSILON;
        let lower_one = row.lower.is_finite() && (row.lower - 1.0).abs() <= EPSILON;
        let packing = !row.lower.is_finite() && upper_one;
        let partitioning = is_equation && lower_one;
        if !(packing || partitioning) {
            continue;
        }
        if support.len() < 2 {
            continue;
        }
        if seen.insert(support.clone()) {
            cliques.push(Clique {
                id: cliques.len(),
                is_equation: partitioning,
                members: support.into_iter().map(|col| (col, true)).collect(),
            });
        }
    }
    cliques.sort_by_key(|clique| std::cmp::Reverse(clique.members.len()));
    cliques
}

fn log10_factorial(n: usize) -> f64 {
    let mut total = 0.0f64;
    for value in 2..=n {
        total += (value as f64).log10();
    }
    total
}


fn install_sender(sender: Sender<SolverEvent>) {
    if let Ok(mut slot) = LOG_SENDER.lock() {
        *slot = Some(sender);
    }
}

fn clear_sender() {
    if let Ok(mut slot) = LOG_SENDER.lock() {
        *slot = None;
    }
}

fn install_capture<F>(hook: F)
where
    F: FnMut(String) + Send + 'static,
{
    if let Ok(mut slot) = LOG_CAPTURE.lock() {
        *slot = Some(Box::new(hook));
    }
}

fn clear_capture() {
    if let Ok(mut slot) = LOG_CAPTURE.lock() {
        *slot = None;
    }
}

fn panic_message(payload: Box<dyn Any + Send>) -> String {
    if let Some(message) = payload.downcast_ref::<String>() {
        message.clone()
    } else if let Some(message) = payload.downcast_ref::<&str>() {
        (*message).to_owned()
    } else {
        "HiGHS worker panicked".to_owned()
    }
}

fn run_presolve(source: &Model, params: &str) -> Result<PresolveReport, String> {
    let started = Instant::now();
    let mut model = build_highs_model(source, false)?;
    apply_highs_options(&mut model, params)?;
    model.presolve();
    let (
        columns,
        rows,
        nonzeros,
        sense,
        objective_offset,
        objective,
        column_lower,
        column_upper,
        row_lower,
        row_upper,
        row_data,
        integrality,
    ) = model.get_presolved_row_lp();
    let reductions = model.num_presolve_reductions();
    let mut type_counts = BTreeMap::<String, usize>::new();
    let mut reduction_items = Vec::new();
    if let Ok(items) = model.get_presolve_reductions() {
        for item in items {
            *type_counts
                .entry(item.reduction_type.to_string())
                .or_default() += 1;
            reduction_items.push(ReductionItem {
                reduction_type: item.reduction_type.to_string(),
                col: item.col,
                row: item.row,
                value: item.value,
                source: item.source.to_string(),
                description: item.to_string(),
            });
        }
    }

    let variables = (0..columns)
        .map(|index| {
            let kind = match integrality[index] {
                lio_highs::VarType::Integer | lio_highs::VarType::SemiInteger
                    if column_lower[index] == 0.0 && column_upper[index] == 1.0 =>
                {
                    VariableKind::Binary
                }
                lio_highs::VarType::Integer
                | lio_highs::VarType::ImplicitInteger
                | lio_highs::VarType::SemiInteger => VariableKind::Integer,
                lio_highs::VarType::Continuous => VariableKind::Continuous,
            };
            Variable {
                name: format!("x{index}"),
                kind,
                lower: column_lower[index],
                upper: column_upper[index],
                objective: objective[index],
            }
        })
        .collect::<Vec<_>>();
    let mut constraints = (0..rows)
        .map(|index| ConstraintRow {
            name: format!("r{index}"),
            lower: row_lower[index],
            upper: row_upper[index],
            terms: row_data[index]
                .iter()
                .map(|(column, coefficient)| Term {
                    column: *column,
                    coefficient: *coefficient,
                })
                .collect(),
            tags: Vec::new(),
        })
        .collect::<Vec<_>>();
    for row in &mut constraints {
        row.tags = classify_constraint(row, &variables);
    }
    let components = ComponentAnalysis::analyze(&constraints, &variables);
    let presolved_model = Model {
        name: format!("{} [presolved: HiGHS]", source.name),
        minimize: sense == Sense::Minimise,
        objective_offset,
        variables,
        constraints,
        nonzeros,
        components,
    };

    Ok(PresolveReport {
        backend: SolverBackend::Highs,
        model: presolved_model,
        reductions,
        reduction_types: type_counts.into_iter().collect(),
        reduction_items,
        elapsed: started.elapsed(),
    })
}

fn run_scip_presolve(
    source: &Model,
    source_path: &str,
    params: &str,
    sender: &Sender<SolverEvent>,
) -> Result<PresolveReport, String> {
    let started = Instant::now();
    let output_path = temporary_scip_path("mps");
    let result = (|| {
        let input = scip_quoted_path(Path::new(source_path))?;
        let output = scip_quoted_path(&output_path)?;
        let mut commands = scip_param_commands(params);
        commands.extend([
            format!("read {input}"),
            "presolve".to_owned(),
            format!("write transproblem {output}"),
            "quit".to_owned(),
        ]);
        let lines = run_scip_commands(&commands, sender)?;
        if !output_path.is_file() {
            return Err("SCIP did not export a transformed problem".to_owned());
        }

        let mut model = Model::load(
            output_path
                .to_str()
                .ok_or_else(|| "temporary SCIP path is not valid UTF-8".to_owned())?,
        )?;
        model.name = format!("{} [presolved: SCIP]", source.name);
        let reduction_types = parse_scip_reductions(&lines);
        let reductions = reduction_types.iter().map(|(_, count)| count).sum();
        Ok(PresolveReport {
            backend: SolverBackend::Scip,
            model,
            reductions,
            reduction_types,
            reduction_items: Vec::new(),
            elapsed: started.elapsed(),
        })
    })();
    let _ = fs::remove_file(output_path);
    result
}

fn run_scip_commands(
    commands: &[String],
    sender: &Sender<SolverEvent>,
) -> Result<Vec<String>, String> {
    let mut command = Command::new("scip");
    for item in commands {
        command.arg("-c").arg(item);
    }
    let mut child = command
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|error| {
            if error.kind() == std::io::ErrorKind::NotFound {
                "SCIP executable not found on PATH; install SCIP to use this solver".to_owned()
            } else {
                format!("could not start SCIP: {error}")
            }
        })?;

    let stderr = child
        .stderr
        .take()
        .ok_or_else(|| "could not capture SCIP stderr".to_owned())?;
    let error_sender = sender.clone();
    let error_reader = thread::spawn(move || {
        for line in BufReader::new(stderr).lines().map_while(Result::ok) {
            if !line.trim().is_empty() {
                let _ = error_sender.send(SolverEvent::Log(line));
            }
        }
    });

    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| "could not capture SCIP stdout".to_owned())?;
    let mut lines = Vec::new();
    for line in BufReader::new(stdout).lines() {
        let line = line.map_err(|error| format!("could not read SCIP output: {error}"))?;
        if !line.trim().is_empty() {
            let _ = sender.send(SolverEvent::Log(line.clone()));
            lines.push(line);
        }
    }
    let status = child
        .wait()
        .map_err(|error| format!("could not wait for SCIP: {error}"))?;
    let _ = error_reader.join();
    if status.success() {
        Ok(lines)
    } else {
        Err(format!("SCIP exited with {status}"))
    }
}

fn temporary_scip_path(extension: &str) -> PathBuf {
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    std::env::temp_dir().join(format!(
        "mipviz-scip-{}-{nonce}.{extension}",
        std::process::id()
    ))
}

fn scip_quoted_path(path: &Path) -> Result<String, String> {
    let value = path
        .to_str()
        .ok_or_else(|| "SCIP requires UTF-8 file paths".to_owned())?;
    if value.contains(['\n', '\r', '"']) {
        return Err("SCIP file path contains unsupported characters".to_owned());
    }
    Ok(format!("\"{value}\""))
}

fn parse_scip_reductions(lines: &[String]) -> Vec<(String, usize)> {
    lines
        .iter()
        .rev()
        .find(|line| line.contains("deleted vars") && line.contains("tightened bounds"))
        .map(|line| {
            line.split(',')
                .filter_map(|part| {
                    let part = part.trim();
                    let (count, label) = part.split_once(' ')?;
                    let count = count.parse::<usize>().ok()?;
                    (count > 0).then(|| (label.to_owned(), count))
                })
                .collect()
        })
        .unwrap_or_default()
}

fn run_scip_solve(
    source: &Model,
    mode: SolveMode,
    params: &str,
    sender: &Sender<SolverEvent>,
) -> Result<SolveReport, String> {
    let started = Instant::now();
    let problem_path = temporary_scip_path("lp");
    let solution_path = temporary_scip_path("sol");
    let result = (|| {
        write_scip_lp(source, &problem_path, mode)?;
        let problem = scip_quoted_path(&problem_path)?;
        let solution = scip_quoted_path(&solution_path)?;
        let mut commands = scip_param_commands(params);
        commands.extend([
            format!("read {problem}"),
            format!("set limits time {SOLVE_TIME_LIMIT_SECONDS}"),
            "optimize".to_owned(),
            format!("write solution {solution}"),
            "quit".to_owned(),
        ]);
        let lines = run_scip_commands(&commands, sender)?;
        let status = parse_scip_status(&lines);
        let nodes = parse_scip_nodes(&lines);
        let (objective, values) = if solution_path.is_file() {
            parse_scip_solution(&solution_path, source.variables.len())?
        } else {
            (f64::NAN, vec![0.0; source.variables.len()])
        };
        Ok(SolveReport {
            backend: SolverBackend::Scip,
            mode,
            status,
            objective: objective + source.objective_offset,
            iterations: nodes,
            values,
            elapsed: started.elapsed(),
        })
    })();
    let _ = fs::remove_file(problem_path);
    let _ = fs::remove_file(solution_path);
    result
}

fn write_scip_lp(source: &Model, path: &Path, mode: SolveMode) -> Result<(), String> {
    let file = fs::File::create(path)
        .map_err(|error| format!("could not create temporary SCIP model: {error}"))?;
    let mut writer = BufWriter::new(file);
    write!(
        writer,
        "{}\n obj:",
        if source.minimize {
            "Minimize"
        } else {
            "Maximize"
        }
    )
    .map_err(write_error)?;
    write_lp_expression(
        &mut writer,
        source
            .variables
            .iter()
            .enumerate()
            .map(|(index, variable)| (index, variable.objective)),
    )?;
    writeln!(writer, "\nSubject To").map_err(write_error)?;
    for (index, row) in source.constraints.iter().enumerate() {
        let lower = finite(row.lower);
        let upper = finite(row.upper);
        if let (Some(lower), Some(upper)) = (lower, upper)
            && (lower - upper).abs() <= 1e-10
        {
            write!(writer, " r{index}: ").map_err(write_error)?;
            write_lp_expression(
                &mut writer,
                row.terms.iter().map(|term| (term.column, term.coefficient)),
            )?;
            writeln!(writer, " = {upper}").map_err(write_error)?;
            continue;
        }
        if let Some(upper) = upper {
            write!(writer, " r{index}u: ").map_err(write_error)?;
            write_lp_expression(
                &mut writer,
                row.terms.iter().map(|term| (term.column, term.coefficient)),
            )?;
            writeln!(writer, " <= {upper}").map_err(write_error)?;
        }
        if let Some(lower) = lower {
            write!(writer, " r{index}l: ").map_err(write_error)?;
            write_lp_expression(
                &mut writer,
                row.terms.iter().map(|term| (term.column, term.coefficient)),
            )?;
            writeln!(writer, " >= {lower}").map_err(write_error)?;
        }
    }

    writeln!(writer, "Bounds").map_err(write_error)?;
    for (index, variable) in source.variables.iter().enumerate() {
        match (finite(variable.lower), finite(variable.upper)) {
            (Some(lower), Some(upper)) if (lower - upper).abs() <= 1e-10 => {
                writeln!(writer, " x{index} = {lower}").map_err(write_error)?;
            }
            (Some(lower), Some(upper)) => {
                writeln!(writer, " {lower} <= x{index} <= {upper}").map_err(write_error)?;
            }
            (Some(lower), None) => {
                writeln!(writer, " x{index} >= {lower}").map_err(write_error)?;
            }
            (None, Some(upper)) => {
                writeln!(writer, " x{index} <= {upper}").map_err(write_error)?;
            }
            (None, None) => {
                writeln!(writer, " x{index} free").map_err(write_error)?;
            }
        }
    }
    if mode == SolveMode::Mip {
        let binaries = source
            .variables
            .iter()
            .enumerate()
            .filter(|(_, variable)| variable.kind == VariableKind::Binary)
            .map(|(index, _)| index)
            .collect::<Vec<_>>();
        if !binaries.is_empty() {
            writeln!(writer, "Binary").map_err(write_error)?;
            for index in binaries {
                writeln!(writer, " x{index}").map_err(write_error)?;
            }
        }
        let integers = source
            .variables
            .iter()
            .enumerate()
            .filter(|(_, variable)| variable.kind == VariableKind::Integer)
            .map(|(index, _)| index)
            .collect::<Vec<_>>();
        if !integers.is_empty() {
            writeln!(writer, "General").map_err(write_error)?;
            for index in integers {
                writeln!(writer, " x{index}").map_err(write_error)?;
            }
        }
    }
    writeln!(writer, "End").map_err(write_error)?;
    writer
        .flush()
        .map_err(|error| format!("could not finish temporary SCIP model: {error}"))
}

fn write_lp_expression<I>(writer: &mut impl Write, terms: I) -> Result<(), String>
where
    I: IntoIterator<Item = (usize, f64)>,
{
    let mut wrote_term = false;
    for (index, coefficient) in terms {
        if coefficient.abs() <= 1e-15 {
            continue;
        }
        if wrote_term {
            write!(writer, " {coefficient:+} x{index}").map_err(write_error)?;
        } else {
            write!(writer, " {coefficient} x{index}").map_err(write_error)?;
            wrote_term = true;
        }
    }
    if !wrote_term {
        write!(writer, " 0").map_err(write_error)?;
    }
    Ok(())
}

fn write_error(error: std::io::Error) -> String {
    format!("could not write temporary SCIP model: {error}")
}

fn parse_scip_solution(path: &Path, variable_count: usize) -> Result<(f64, Vec<f64>), String> {
    let file =
        fs::File::open(path).map_err(|error| format!("could not open SCIP solution: {error}"))?;
    let mut objective = f64::NAN;
    let mut values = vec![0.0; variable_count];
    for line in BufReader::new(file).lines() {
        let line = line.map_err(|error| format!("could not read SCIP solution: {error}"))?;
        if let Some(value) = line.strip_prefix("objective value:") {
            objective = value
                .trim()
                .parse()
                .map_err(|_| format!("invalid SCIP objective value: {value}"))?;
            continue;
        }
        let mut fields = line.split_whitespace();
        let Some(name) = fields.next() else {
            continue;
        };
        let Some(index) = name
            .strip_prefix('x')
            .and_then(|value| value.parse::<usize>().ok())
        else {
            continue;
        };
        let Some(value) = fields.next().and_then(|value| value.parse::<f64>().ok()) else {
            continue;
        };
        if index < values.len() {
            values[index] = value;
        }
    }
    Ok((objective, values))
}

fn parse_scip_status(lines: &[String]) -> String {
    let status = lines
        .iter()
        .rev()
        .find(|line| line.contains("SCIP Status"))
        .map(|line| line.to_ascii_lowercase())
        .unwrap_or_default();
    if status.contains("optimal solution") {
        "Optimal"
    } else if status.contains("time limit") {
        "TimeLimit"
    } else if status.contains("infeasible") {
        "Infeasible"
    } else if status.contains("unbounded") {
        "Unbounded"
    } else if status.is_empty() {
        "Unknown"
    } else {
        "Interrupted"
    }
    .to_owned()
}

fn parse_scip_nodes(lines: &[String]) -> i32 {
    lines
        .iter()
        .rev()
        .find_map(|line| {
            line.strip_prefix("Solving Nodes")?
                .split_once(':')?
                .1
                .trim()
                .parse()
                .ok()
        })
        .unwrap_or(0)
}

fn run_solve(source: &Model, mode: SolveMode, params: &str) -> Result<SolveReport, String> {
    let started = Instant::now();
    let mut model = build_highs_model(source, mode == SolveMode::Relaxation)?;
    apply_highs_options(&mut model, params)?;
    model.set_option("time_limit", SOLVE_TIME_LIMIT_SECONDS);
    let solved = model
        .try_solve()
        .map_err(|(status, _)| format!("HiGHS failed with {status:?}"))?;
    let status = format!("{:?}", solved.status());
    let objective = solved.obj_val() + source.objective_offset;
    let iterations = solved.get_iteration_count();
    let values = solved.get_solution().columns().to_vec();

    Ok(SolveReport {
        backend: SolverBackend::Highs,
        mode,
        status,
        objective,
        iterations,
        values,
        elapsed: started.elapsed(),
    })
}

fn build_highs_model(source: &Model, relax: bool) -> Result<lio_highs::Model, String> {
    let mut problem = ColProblem::default();
    for row in &source.constraints {
        add_row(&mut problem, row.lower, row.upper);
    }

    let mut column_terms = vec![Vec::<(usize, f64)>::new(); source.variables.len()];
    for (row_index, row) in source.constraints.iter().enumerate() {
        for term in &row.terms {
            column_terms[term.column].push((row_index, term.coefficient));
        }
    }

    for (index, variable) in source.variables.iter().enumerate() {
        let integer = !relax && variable.kind != VariableKind::Continuous;
        add_column(
            &mut problem,
            variable.objective,
            variable.lower,
            variable.upper,
            &column_terms[index],
            integer,
        );
    }

    let sense = if source.minimize {
        Sense::Minimise
    } else {
        Sense::Maximise
    };
    problem
        .try_optimise(sense)
        .map_err(|status| format!("could not create HiGHS model: {status:?}"))
}

fn add_row(problem: &mut ColProblem, lower: f64, upper: f64) {
    let lower = finite(lower).unwrap_or(f64::NEG_INFINITY);
    let upper = finite(upper).unwrap_or(f64::INFINITY);
    problem.add_row(lower..=upper);
}

fn add_column(
    problem: &mut ColProblem,
    objective: f64,
    lower: f64,
    upper: f64,
    terms: &[(usize, f64)],
    integer: bool,
) {
    let lower = finite(lower).unwrap_or(f64::NEG_INFINITY);
    let upper = finite(upper).unwrap_or(f64::INFINITY);
    problem.add_column_with_integrality(objective, lower..=upper, terms, integer);
}

fn finite(value: f64) -> Option<f64> {
    (value.is_finite() && value.abs() < 1e20).then_some(value)
}

/// Parse `key = value` / `key value` lines from the parameters text. `#` starts
/// a comment; blank lines are skipped.
pub fn parse_param_lines(text: &str) -> Vec<(String, String)> {
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

/// Apply user-supplied HiGHS options. We don't get direct access to the raw
/// pointer (`HighsOptionValue` is sealed), so we guess a type from the value
/// string and try typed setters in order. `set_option` panics on a wrong-type
/// rejection, so each attempt is wrapped in `catch_unwind`.
pub fn apply_highs_options(model: &mut lio_highs::Model, text: &str) -> Result<(), String> {
    if text.trim().is_empty() {
        return Ok(());
    }
    let prev_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(|_| {}));

    enum Typed {
        Bool(bool),
        Int(i32),
        Float(f64),
        Str,
    }
    let mut failed: Vec<String> = Vec::new();
    for (k, v) in parse_param_lines(text) {
        let lower = v.to_ascii_lowercase();
        let order: Vec<Typed> =
            if matches!(lower.as_str(), "true" | "false" | "on" | "off" | "yes" | "no") {
                vec![
                    Typed::Bool(matches!(lower.as_str(), "true" | "on" | "yes")),
                    Typed::Str,
                ]
            } else if let Ok(n) = v.parse::<i32>() {
                vec![Typed::Int(n), Typed::Float(n as f64), Typed::Str]
            } else if let Ok(f) = v.parse::<f64>() {
                vec![Typed::Float(f), Typed::Str]
            } else {
                vec![Typed::Str]
            };

        let mut accepted = false;
        for typed in order {
            let attempt = match typed {
                Typed::Bool(b) => catch_unwind(AssertUnwindSafe(|| {
                    model.set_option(k.as_str(), b);
                })),
                Typed::Int(n) => catch_unwind(AssertUnwindSafe(|| {
                    model.set_option(k.as_str(), n);
                })),
                Typed::Float(f) => catch_unwind(AssertUnwindSafe(|| {
                    model.set_option(k.as_str(), f);
                })),
                Typed::Str => catch_unwind(AssertUnwindSafe(|| {
                    model.set_option(k.as_str(), v.as_str());
                })),
            };
            if attempt.is_ok() {
                accepted = true;
                break;
            }
        }
        if !accepted {
            failed.push(k);
        }
    }
    std::panic::set_hook(prev_hook);
    if failed.is_empty() {
        Ok(())
    } else {
        Err(format!("unknown HiGHS option(s): {}", failed.join(", ")))
    }
}

/// Convert a parameters string into SCIP `set` commands for the binary.
pub fn scip_param_commands(text: &str) -> Vec<String> {
    parse_param_lines(text)
        .into_iter()
        .map(|(k, v)| format!("set {k} {v}"))
        .collect()
}

/// Extract root-node cut statistics from a HiGHS MIP log. HiGHS prints a
/// `Cuts` column in its branch-and-bound table; root rows are the ones whose
/// node index is 0. `InLp` is the number of cuts kept in the LP.
pub fn parse_highs_root_cuts(lines: &[String]) -> RootCutStats {
    let mut total_cuts = 0usize;
    let mut cuts_in_lp = 0usize;
    let mut rounds = 0usize;
    let mut saw_header = false;
    for line in lines {
        let trimmed = line.trim_start();
        if trimmed.starts_with("Src") && trimmed.contains("Cuts") {
            saw_header = true;
            continue;
        }
        if !saw_header {
            continue;
        }
        // Root rows: start with node index 0 (allow a Src tag like `L`/`S`).
        let body = trimmed
            .strip_prefix('L')
            .or_else(|| trimmed.strip_prefix('S'))
            .unwrap_or(trimmed)
            .trim_start();
        let mut fields = body.split_whitespace();
        let Some(node) = fields.next().and_then(|value| value.parse::<usize>().ok()) else {
            continue;
        };
        if node != 0 {
            break;
        }
        // Fields: Proc InQueue Leaves Expl. BestBound BestSol Gap | Cuts InLp Confl. | LpIters Time
        let fields = body.split_whitespace().collect::<Vec<_>>();
        if fields.len() < 9 {
            continue;
        }
        let cuts = fields[7].parse::<usize>().unwrap_or(0);
        let inlp = fields[8].parse::<usize>().unwrap_or(0);
        total_cuts = total_cuts.max(cuts);
        cuts_in_lp = cuts_in_lp.max(inlp);
        rounds += 1;
    }
    RootCutStats {
        backend: SolverBackend::Highs,
        total_cuts,
        separation_rounds: rounds,
        cuts_in_lp,
        lp_rows: 0,
    }
}

/// Extract root-node cut statistics from a SCIP MIP log. SCIP's table has a
/// `cuts` column (cumulative cuts) and a `sepa` column (separation rounds).
/// Column layout (split on `|`): 0 time · 1 node · 2 left · 3 LPiter ·
/// 4 LPit/n · 5 mem/heur · 6 mdpt · 7 vars · 8 cons · 9 rows · 10 cuts ·
/// 11 sepa · 12 confs · 13 strbr · 14+ bounds.
pub fn parse_scip_root_cuts(lines: &[String]) -> RootCutStats {
    let mut total_cuts = 0usize;
    let mut rounds = 0usize;
    let mut lp_rows = 0usize;
    for line in lines {
        let trimmed = line.trim();
        let fields = trimmed.split('|').collect::<Vec<_>>();
        if fields.len() < 14 {
            continue;
        }
        // The first field is a time like `0.0s`. Skip header/summary lines.
        let time = fields[0].trim();
        if !time.ends_with('s') || !time.contains('.') {
            continue;
        }
        let node = fields[1].trim();
        if node != "1" && node != "0" {
            continue;
        }
        let cuts = fields[10].trim().parse::<usize>().unwrap_or(0);
        let sepa = fields[11].trim().parse::<usize>().unwrap_or(0);
        let rows = fields[9].trim().parse::<usize>().unwrap_or(0);
        total_cuts = total_cuts.max(cuts);
        rounds = rounds.max(sepa);
        lp_rows = lp_rows.max(rows);
    }
    RootCutStats {
        backend: SolverBackend::Scip,
        total_cuts,
        separation_rounds: rounds,
        cuts_in_lp: 0,
        lp_rows,
    }
}

/// Parse root-cut stats from any solver log, dispatching on the backend.
pub fn parse_root_cuts(lines: &[String], backend: SolverBackend) -> RootCutStats {
    match backend {
        SolverBackend::Highs => parse_highs_root_cuts(lines),
        SolverBackend::Scip => parse_scip_root_cuts(lines),
    }
}

/// Run a dedicated short MIP solve (on a background thread) purely to capture
/// root-node cut statistics from the solver log.
pub fn start_root_cuts(
    model: Arc<Model>,
    source_path: String,
    params: String,
    sender: Sender<SolverEvent>,
) {
    let backend = if source_path.is_empty() {
        SolverBackend::Highs
    } else {
        SolverBackend::Scip
    };
    thread::spawn(move || {
        let result = catch_unwind(AssertUnwindSafe(|| match backend {
            SolverBackend::Scip => {
                let problem_path = temporary_scip_path("lp");
                let outcome = write_scip_lp(&model, &problem_path, SolveMode::Mip)
                    .and_then(|_| {
                        let problem = scip_quoted_path(&problem_path)?;
                        let mut commands = scip_param_commands(&params);
                        commands.extend([
                            format!("read {problem}"),
                            format!("set limits time {SOLVE_TIME_LIMIT_SECONDS}"),
                            "optimize".to_owned(),
                            "quit".to_owned(),
                        ]);
                        run_scip_commands(&commands, &sender)
                    });
                let _ = fs::remove_file(problem_path);
                let lines = outcome?;
                Ok(parse_root_cuts(&lines, backend))
            }
            SolverBackend::Highs => {
                // Relay HiGHS log lines to the caller while collecting a copy
                // to parse. The global log sender is global, so we install a
                // relay that forwards to the caller and records locally.
                let collected = Arc::new(Mutex::new(Vec::<String>::new()));
                let forwarded = collected.clone();
                let relay = Sender::clone(&sender);
                install_capture(Box::new(move |line: String| {
                    let _ = relay.send(SolverEvent::Log(line.clone()));
                    forwarded.lock().unwrap().push(line);
                }));
                let outcome = (|| {
                    let mut model_ = build_highs_model(&model, false)?;
                    apply_highs_options(&mut model_, &params)?;
                    model_.set_option("time_limit", SOLVE_TIME_LIMIT_SECONDS);
                    let _ = model_.try_solve();
                    Ok::<(), String>(())
                })();
                clear_capture();
                outcome?;
                let lines = collected.lock().unwrap().clone();
                Ok(parse_root_cuts(&lines, backend))
            }
        }))
        .map_err(panic_message)
        .and_then(|result| result);
        let _ = sender.send(SolverEvent::RootCutsFinished(result));
    });
}


#[cfg(test)]
mod tests {
    use super::*;
    use crate::{ConstraintRow, Term, Variable};

    fn tiny_model() -> Model {
        let variables = vec![
            Variable {
                name: "x".to_owned(),
                kind: VariableKind::Binary,
                lower: 0.0,
                upper: 1.0,
                objective: 1.0,
            },
            Variable {
                name: "y".to_owned(),
                kind: VariableKind::Continuous,
                lower: 0.0,
                upper: f64::INFINITY,
                objective: 2.0,
            },
        ];
        let constraints = vec![ConstraintRow {
            name: "demand".to_owned(),
            lower: 1.0,
            upper: f64::INFINITY,
            terms: vec![
                Term {
                    column: 0,
                    coefficient: 1.0,
                },
                Term {
                    column: 1,
                    coefficient: 1.0,
                },
            ],
            tags: Vec::new(),
        }];
        let components = ComponentAnalysis::analyze(&constraints, &variables);
        Model {
            name: "tiny".to_owned(),
            minimize: true,
            objective_offset: 0.0,
            variables,
            constraints,
            nonzeros: 2,
            components,
        }
    }

    #[test]
    fn solves_lp_and_mip() {
        for mode in [SolveMode::Relaxation, SolveMode::Mip] {
            let report = run_solve(&tiny_model(), mode, "").expect("HiGHS solve should succeed");
            assert_eq!(report.status, "Optimal");
            assert!((report.objective - 1.0).abs() < 1e-7);
            assert!((report.values[0] - 1.0).abs() < 1e-7);
        }
    }

    #[test]
    fn presolves_model() {
        let report = run_presolve(&tiny_model(), "").expect("HiGHS presolve should succeed");
        assert_eq!(report.backend, SolverBackend::Highs);
        assert!(report.model.variables.len() <= 2);
        assert!(report.model.constraints.len() <= 1);
        assert!(report.model.nonzeros <= 2);
    }

    #[test]
    fn parses_scip_reduction_summary() {
        let lines = vec![
            "presolving:".to_owned(),
            " 2 deleted vars, 3 deleted constraints, 0 added constraints, 7 tightened bounds, 0 added holes, 1 changed sides, 0 changed coefficients".to_owned(),
        ];
        assert_eq!(
            parse_scip_reductions(&lines),
            vec![
                ("deleted vars".to_owned(), 2),
                ("deleted constraints".to_owned(), 3),
                ("tightened bounds".to_owned(), 7),
                ("changed sides".to_owned(), 1),
            ]
        );
    }

    #[test]
    fn writes_integrality_only_for_scip_mip() {
        for (mode, expects_binary_section) in
            [(SolveMode::Relaxation, false), (SolveMode::Mip, true)]
        {
            let path = temporary_scip_path("lp");
            write_scip_lp(&tiny_model(), &path, mode).expect("LP export should succeed");
            let contents = fs::read_to_string(&path).expect("LP export should be readable");
            let _ = fs::remove_file(path);
            assert_eq!(contents.contains("\nBinary\n"), expects_binary_section);
            assert!(contents.contains("Subject To"));
            assert!(contents.contains("1 x0 +2 x1"));
        }
    }

    #[test]
    fn parses_scip_status_and_nodes() {
        let lines = vec![
            "SCIP Status        : problem is solved [optimal solution found]".to_owned(),
            "Solving Nodes      : 42".to_owned(),
        ];
        assert_eq!(parse_scip_status(&lines), "Optimal");
        assert_eq!(parse_scip_nodes(&lines), 42);
    }

    #[test]
    fn parses_param_lines() {
        let text = "# comment\ntime_limit = 10\npresolve off\n\nmaxrounds=0 # trailing";
        assert_eq!(
            parse_param_lines(text),
            vec![
                ("time_limit".to_owned(), "10".to_owned()),
                ("presolve".to_owned(), "off".to_owned()),
                ("maxrounds".to_owned(), "0".to_owned()),
            ]
        );
    }

    #[test]
    fn scip_param_commands_build_set_lines() {
        assert_eq!(
            scip_param_commands("limits/time = 10\npresolve/maxrounds = 0"),
            vec![
                "set limits/time 10".to_owned(),
                "set presolve/maxrounds 0".to_owned(),
            ]
        );
    }

    #[test]
    fn parses_scip_root_cuts_from_log_rows() {
        let lines = vec![
            "time | node | left | LP iter | ... | cuts | sepa | ...".to_owned(),
            "  0.0s|     1 |     0 |   598 |     - |    18M |   0 |2013 | 233 | 261 |  28 |  1 |   0 |   0 | 3.04e+03 | 6.79e+03 | 123%| unknown".to_owned(),
            "  0.0s|     1 |     0 |   666 |     - |    19M |   0 |2013 | 233 | 287 |  54 |  2 |   0 |   0 | 3.08e+03 | 6.79e+03 | 120%| unknown".to_owned(),
        ];
        let stats = parse_scip_root_cuts(&lines);
        assert_eq!(stats.backend, SolverBackend::Scip);
        assert_eq!(stats.total_cuts, 54);
        assert_eq!(stats.separation_rounds, 2);
        assert_eq!(stats.lp_rows, 287);
    }

    #[test]
    fn parses_highs_root_cuts_from_log_rows() {
        let lines = vec![
            "Src  Proc. InQueue |  Leaves   Expl. | BestBound       BestSol              Gap |   Cuts   InLp Confl. | LpIters     Time".to_owned(),
            "          0       0         0   0.00%   2879.065687     7482.05998        61.52%        0      0      0       322     0.0s".to_owned(),
            "  L       0       0         0   0.00%   3221.312531     3757.499994       14.27%     4717    346      2      4199     1.3s".to_owned(),
        ];
        let stats = parse_highs_root_cuts(&lines);
        assert_eq!(stats.backend, SolverBackend::Highs);
        assert_eq!(stats.total_cuts, 4717);
        assert_eq!(stats.cuts_in_lp, 346);
    }

    #[test]
    fn detects_symmetry_and_cliques_on_symmetric_model() {
        // Two interchangeable binary variables with a set-packing row:
        // x0 + x1 + x2 <= 1, all binary, same objective.
        let variables = (0..3)
            .map(|index| Variable {
                name: format!("x{index}"),
                kind: VariableKind::Binary,
                lower: 0.0,
                upper: 1.0,
                objective: 1.0,
            })
            .collect::<Vec<_>>();
        let constraints = vec![ConstraintRow {
            name: "packing".to_owned(),
            lower: f64::NEG_INFINITY,
            upper: 1.0,
            terms: (0..3)
                .map(|column| Term {
                    column,
                    coefficient: 1.0,
                })
                .collect(),
            tags: Vec::new(),
        }];
        let components = ComponentAnalysis::analyze(&constraints, &variables);
        let model = Model {
            name: "symmetric".to_owned(),
            minimize: true,
            objective_offset: 0.0,
            variables,
            constraints,
            nonzeros: 3,
            components,
        };
        let report = run_highs_analysis(&model).expect("analysis should succeed");
        // x0 + x1 + x2 <= 1 yields a clique on the three binaries.
        assert!(
            report.cliques.iter().any(|clique| clique.members.len() == 3),
            "expected a 3-member clique, got {:?}",
            report.cliques.iter().map(|c| c.members.len()).collect::<Vec<_>>()
        );
        // All three columns are interchangeable twins → one symmetry component.
        assert!(
            report.symmetry_components.iter().any(|group| group.len() == 3),
            "expected a 3-variable symmetry group, got {:?}",
            report.symmetry_components
        );
        assert!(report.symmetry_generators > 0);
    }
}

#[cfg(test)]
mod integration {
    use super::*;
    use crate::Model;

    #[test]
    #[ignore]
    fn real_instance_analysis() {
        init_logging();
        let path = std::env::var("MIPVIZ_TEST_INSTANCE")
            .unwrap_or_else(|_| "/Users/ghannam/instances/10teams.mps.gz".to_owned());
        let model = Model::load(&path).unwrap();
        let report = run_highs_analysis(&model).unwrap();
        println!("cliques: {} max {} impl: {}",
            report.cliques.len(),
            report.cliques.iter().map(|c| c.members.len()).max().unwrap_or(0),
            report.num_implications);
        println!("sym: {} gens, {} permvars, {} components",
            report.symmetry_generators, report.symmetry_perm_vars, report.symmetry_components.len());
        if let Some(big) = report.symmetry_components.first() {
            let names: Vec<_> = big.iter().take(6).map(|&c| report.symmetry_names[c].clone()).collect();
            println!("biggest group size {} sample {:?}", big.len(), names);
        }
        // reductions
        let reds = run_highs_reductions(&model).unwrap();
        println!("reductions: {} ({} rows hidden by 50% rule)",
            reds.len(),
            reds.iter().filter(|r| matches!(r.reduction_type.as_str(), "FixedCol"|"RedundantRow"|"SingletonRow"|"DoubletonEquation"|"ForcingRow"|"ForcingColumn"|"DuplicateRow"|"DuplicateColumn"|"SlackColSubstitution"|"FreeColSubstitution"|"ForcingColumnRemovedRow")).count());
        // root cuts via HiGHS MIP log
        let mut m = build_highs_model(&model, false).unwrap();
        m.set_option("time_limit", 5.0);
        let logs = Arc::new(Mutex::new(Vec::<String>::new()));
        let logs_capture = logs.clone();
        let (tx, _rx) = std::sync::mpsc::channel::<SolverEvent>();
        install_capture(Box::new(move |line| logs_capture.lock().unwrap().push(line)));
        install_sender(tx);
        let _ = m.try_solve();
        clear_sender();
        clear_capture();
        let stats = parse_highs_root_cuts(&logs.lock().unwrap());
        println!("highs root cuts: {} total, {} in LP (log lines {})", stats.total_cuts, stats.cuts_in_lp, logs.lock().unwrap().len());

        // SCIP root cuts from a real optimize run (requires `scip` on PATH).
        if let Ok(lines) = run_scip_commands(
            &[
                "read \"/Users/ghannam/instances/markshare_4_0.mps.gz\"".to_owned(),
                "set limits time 5".to_owned(),
                "optimize".to_owned(),
                "quit".to_owned(),
            ],
            &std::sync::mpsc::channel::<SolverEvent>().0,
        ) {
            let stats = parse_scip_root_cuts(&lines);
            println!("scip root cuts: {} total, {} rounds, {} rows", stats.total_cuts, stats.separation_rounds, stats.lp_rows);
        } else {
            println!("scip not available — skipping");
        }
    }
}

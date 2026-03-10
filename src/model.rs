use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelResponse {
    pub name: String,
    pub obj_sense: String,
    pub obj_offset: f64,
    pub stats: Stats,
    pub variables: Vec<Variable>,
    pub constraints: Vec<Constraint>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub parse_time_ms: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Stats {
    pub num_vars: usize,
    pub num_constraints: usize,
    pub num_nonzeros: usize,
    pub num_continuous: usize,
    pub num_integer: usize,
    pub num_binary: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Variable {
    pub name: String,
    pub var_type: VarType,
    pub lower: Option<f64>,
    pub upper: Option<f64>,
    pub obj: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Constraint {
    pub name: String,
    pub lower: Option<f64>,
    pub upper: Option<f64>,
    pub terms: Vec<Term>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Term {
    pub var_index: usize,
    pub var_name: String,
    pub var_type: VarType,
    pub coeff: f64,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum VarType {
    Continuous,
    Integer,
    Binary,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PresolveReductionItem {
    pub reduction_type: String,
    pub col: i32,
    pub row: i32,
    pub value: f64,
    pub source: String,
    pub description: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReductionsResponse {
    pub reductions: Vec<PresolveReductionItem>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LpSolutionResponse {
    pub status: String,
    pub objective_value: f64,
    pub col_values: Vec<f64>,
    pub row_values: Vec<f64>,
    pub dual_values: Vec<f64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CliqueEntry {
    pub col: usize,
    pub val: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub var_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub var_index: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub value: Option<bool>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CliqueVariable {
    pub name: String,
    pub var_type: VarType,
    pub lower: Option<f64>,
    pub upper: Option<f64>,
    pub obj: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CliqueResponse {
    pub cliques: Vec<Vec<CliqueEntry>>,
    pub variables: Vec<CliqueVariable>,
    pub num_cliques: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Clique {
    pub id: usize,
    pub is_equation: bool,
    pub members: Vec<CliqueEntry>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ImplicationEntry {
    pub from_var_name: String,
    pub from_var_index: usize,
    pub from_value: bool,
    pub to_var_name: String,
    pub to_var_index: usize,
    pub bound_type: String,
    pub bound_value: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CliquesImplicationsResponse {
    pub solver: String,
    pub num_cliques: usize,
    pub num_implications: usize,
    pub cliques: Vec<Clique>,
    pub implications: Vec<ImplicationEntry>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SymmetryResponse {
    pub num_generators: i32,
    pub num_permvars: i32,
    pub num_components: i32,
    pub log10_group_size: f64,
    pub bin_var_affected: bool,
    pub components: Vec<SymmetryComponent>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SymmetryComponent {
    pub id: usize,
    pub var_indices: Vec<usize>,
    pub var_names: Vec<String>,
}

/// Convert f64 to Option<f64>, mapping infinities and near-infinities to None.
/// SCIP uses 1e20 as its default infinity, so treat |v| >= 1e20 as infinite.
pub fn finite_or_none(v: f64) -> Option<f64> {
    if v.is_finite() && v.abs() < 1e20 { Some(v) } else { None }
}

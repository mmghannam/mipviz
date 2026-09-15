use std::{
    collections::BTreeSet,
    env,
    error::Error,
    io::{self, stdout},
    path::Path,
    sync::{
        Arc,
        mpsc::{self, Receiver, Sender},
    },
    time::Duration,
};

use crossterm::{
    event::{
        self, DisableMouseCapture, EnableMouseCapture, Event, KeyCode, KeyEvent, KeyEventKind,
        KeyModifiers, MouseEvent, MouseEventKind,
    },
    execute,
    terminal::{EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode},
};
use ratatui::{
    Frame, Terminal,
    backend::CrosstermBackend,
    layout::{Alignment, Constraint as LayoutConstraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span, Text},
    widgets::{Block, Borders, Cell, Clear, Gauge, Paragraph, Row, Table, TableState, Tabs, Wrap},
};

mod components;
mod solver;

use components::ComponentAnalysis;
use solver::{
    AnalysisReport, PresolveReport, ReductionItem, RootCutStats, SolveMode, SolveReport,
    SolverBackend, SolverEvent,
};

const ACCENT: Color = Color::Rgb(91, 155, 213);
const GREEN: Color = Color::Rgb(92, 184, 92);
const ORANGE: Color = Color::Rgb(240, 145, 60);
const MUTED: Color = Color::Rgb(125, 135, 145);

fn main() {
    if let Err(error) = run() {
        eprintln!("mipviz: {error}");
        std::process::exit(1);
    }
}

fn run() -> Result<(), Box<dyn Error>> {
    let path = match env::args().nth(1) {
        Some(arg) if arg == "-h" || arg == "--help" => {
            print_help();
            return Ok(());
        }
        Some(arg) => arg,
        None => {
            print_help();
            return Err("missing model path".into());
        }
    };

    if !Path::new(&path).is_file() {
        return Err(format!("file not found: {path}").into());
    }
    let lower = path.to_ascii_lowercase();
    if !(lower.ends_with(".mps") || lower.ends_with(".mps.gz")) {
        return Err("this first milestone supports .mps and .mps.gz files".into());
    }

    eprintln!("Loading {path} …");
    let model = Model::load(&path)?;
    solver::init_logging();
    let mut app = App::new(model, path);

    let mut terminal = TerminalGuard::enter()?;
    loop {
        app.drain_solver_events();
        terminal.terminal.draw(|frame| app.draw(frame))?;
        if event::poll(Duration::from_millis(150))? {
            match event::read()? {
                Event::Key(key) if key.kind == KeyEventKind::Press && app.handle_key(key) => break,
                Event::Mouse(mouse) => app.handle_mouse(mouse),
                _ => {}
            }
        }
    }
    Ok(())
}

fn print_help() {
    println!(
        "mipviz-tui — explore a MIP model in your terminal\n\n\
         Usage: mipviz <MODEL.mps[.gz]>\n\n\
         Keys: 1-7 switch views · b choose solver · p presolve · l solve LP · m solve MIP ·\n\
         r analysis/root-cuts · = solver params · q quit"
    );
}

struct TerminalGuard {
    terminal: Terminal<CrosstermBackend<io::Stdout>>,
}

impl TerminalGuard {
    fn enter() -> io::Result<Self> {
        enable_raw_mode()?;
        let mut output = stdout();
        if let Err(error) = execute!(output, EnterAlternateScreen, EnableMouseCapture) {
            let _ = disable_raw_mode();
            return Err(error);
        }
        let terminal = match Terminal::new(CrosstermBackend::new(output)) {
            Ok(terminal) => terminal,
            Err(error) => {
                let _ = disable_raw_mode();
                let _ = execute!(stdout(), LeaveAlternateScreen);
                return Err(error);
            }
        };
        Ok(Self { terminal })
    }
}

impl Drop for TerminalGuard {
    fn drop(&mut self) {
        let _ = disable_raw_mode();
        let _ = execute!(
            self.terminal.backend_mut(),
            LeaveAlternateScreen,
            DisableMouseCapture
        );
        let _ = self.terminal.show_cursor();
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum VariableKind {
    Continuous,
    Integer,
    Binary,
}

impl VariableKind {
    fn label(self) -> &'static str {
        match self {
            Self::Continuous => "cont",
            Self::Integer => "int",
            Self::Binary => "bin",
        }
    }

    fn color(self) -> Color {
        match self {
            Self::Continuous => ACCENT,
            Self::Integer => ORANGE,
            Self::Binary => GREEN,
        }
    }
}

struct Variable {
    name: String,
    kind: VariableKind,
    lower: f64,
    upper: f64,
    objective: f64,
}

struct Term {
    column: usize,
    coefficient: f64,
}

struct ConstraintRow {
    name: String,
    lower: f64,
    upper: f64,
    terms: Vec<Term>,
    tags: Vec<ConstraintTag>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
enum ConstraintTag {
    Empty,
    Continuous,
    PureBinary,
    MixedInteger,
    MixedBinary,
    BigM,
    Bound,
    Precedence,
    VariableBound,
    SetPartitioning,
    SetPacking,
    SetCovering,
    Cardinality,
    Knapsack,
    BinaryKnapsackEquation,
    IntegerKnapsack,
    Equality,
}

impl ConstraintTag {
    const ALL: [Self; 17] = [
        Self::SetPartitioning,
        Self::SetPacking,
        Self::SetCovering,
        Self::Precedence,
        Self::Cardinality,
        Self::Knapsack,
        Self::BinaryKnapsackEquation,
        Self::IntegerKnapsack,
        Self::VariableBound,
        Self::Bound,
        Self::Equality,
        Self::Continuous,
        Self::PureBinary,
        Self::MixedBinary,
        Self::MixedInteger,
        Self::BigM,
        Self::Empty,
    ];

    fn label(self) -> &'static str {
        match self {
            Self::Empty => "empty",
            Self::Continuous => "continuous",
            Self::PureBinary => "pure binary",
            Self::MixedInteger => "mixed integer",
            Self::MixedBinary => "mixed binary",
            Self::BigM => "big-M",
            Self::Bound => "bound",
            Self::Precedence => "precedence",
            Self::VariableBound => "variable bound",
            Self::SetPartitioning => "set partitioning",
            Self::SetPacking => "set packing",
            Self::SetCovering => "set covering",
            Self::Cardinality => "cardinality",
            Self::Knapsack => "knapsack",
            Self::BinaryKnapsackEquation => "bin. knapsack eq.",
            Self::IntegerKnapsack => "integer knapsack",
            Self::Equality => "equality",
        }
    }

    fn color(self) -> Color {
        match self {
            Self::SetPartitioning | Self::Equality => Color::Magenta,
            Self::SetPacking | Self::PureBinary => Color::Blue,
            Self::SetCovering | Self::MixedInteger => Color::Green,
            Self::Precedence => Color::LightGreen,
            Self::Cardinality => Color::Yellow,
            Self::Knapsack => Color::Red,
            Self::BinaryKnapsackEquation | Self::BigM => Color::LightMagenta,
            Self::IntegerKnapsack => ORANGE,
            Self::VariableBound | Self::MixedBinary => Color::Cyan,
            Self::Bound | Self::Empty => MUTED,
            Self::Continuous => Color::LightRed,
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum NonzeroOperator {
    LessOrEqual,
    Equal,
    GreaterOrEqual,
}

impl NonzeroOperator {
    fn symbol(self) -> &'static str {
        match self {
            Self::LessOrEqual => "≤",
            Self::Equal => "=",
            Self::GreaterOrEqual => "≥",
        }
    }

    fn previous(self) -> Self {
        match self {
            Self::LessOrEqual => Self::GreaterOrEqual,
            Self::Equal => Self::LessOrEqual,
            Self::GreaterOrEqual => Self::Equal,
        }
    }

    fn next(self) -> Self {
        match self {
            Self::LessOrEqual => Self::Equal,
            Self::Equal => Self::GreaterOrEqual,
            Self::GreaterOrEqual => Self::LessOrEqual,
        }
    }

    fn matches(self, actual: usize, expected: usize) -> bool {
        match self {
            Self::LessOrEqual => actual <= expected,
            Self::Equal => actual == expected,
            Self::GreaterOrEqual => actual >= expected,
        }
    }
}

struct Model {
    name: String,
    minimize: bool,
    objective_offset: f64,
    variables: Vec<Variable>,
    constraints: Vec<ConstraintRow>,
    nonzeros: usize,
    components: ComponentAnalysis,
}

impl Model {
    fn load(path: &str) -> Result<Self, String> {
        let parsed = numnom::parse_mps_file(path)?;
        let column_count = parsed.num_col as usize;
        let row_count = parsed.num_row as usize;

        let variables = (0..column_count)
            .map(|index| {
                let kind = match parsed.col_integrality[index] {
                    numnom::VarType::Integer | numnom::VarType::SemiInteger
                        if parsed.col_lower[index] == 0.0 && parsed.col_upper[index] == 1.0 =>
                    {
                        VariableKind::Binary
                    }
                    numnom::VarType::Integer | numnom::VarType::SemiInteger => {
                        VariableKind::Integer
                    }
                    _ => VariableKind::Continuous,
                };
                Variable {
                    name: parsed.col_names[index].clone(),
                    kind,
                    lower: parsed.col_lower[index],
                    upper: parsed.col_upper[index],
                    objective: parsed.col_cost[index],
                }
            })
            .collect::<Vec<_>>();

        let mut row_terms: Vec<Vec<Term>> = (0..row_count).map(|_| Vec::new()).collect();
        for column in 0..column_count {
            let start = parsed.a_matrix.start[column] as usize;
            let end = parsed.a_matrix.start[column + 1] as usize;
            for entry in start..end {
                row_terms[parsed.a_matrix.index[entry] as usize].push(Term {
                    column,
                    coefficient: parsed.a_matrix.value[entry],
                });
            }
        }

        let mut constraints = (0..row_count)
            .map(|index| ConstraintRow {
                name: parsed.row_names[index].clone(),
                lower: parsed.row_lower[index],
                upper: parsed.row_upper[index],
                terms: std::mem::take(&mut row_terms[index]),
                tags: Vec::new(),
            })
            .collect::<Vec<_>>();
        for row in &mut constraints {
            row.tags = classify_constraint(row, &variables);
        }

        let filename = Path::new(path)
            .file_name()
            .and_then(|value| value.to_str())
            .unwrap_or(path);
        let name = filename
            .strip_suffix(".mps.gz")
            .or_else(|| filename.strip_suffix(".mps"))
            .unwrap_or(filename)
            .to_owned();

        let components = ComponentAnalysis::analyze(&constraints, &variables);
        Ok(Self {
            name,
            minimize: parsed.obj_sense_minimize,
            objective_offset: parsed.obj_offset,
            variables,
            constraints,
            nonzeros: parsed.a_matrix.value.len(),
            components,
        })
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum View {
    Overview,
    Variables,
    Constraints,
    Matrix,
    Presolve,
    Solve,
    Analysis,
}

impl View {
    const ALL: [Self; 7] = [
        Self::Overview,
        Self::Variables,
        Self::Constraints,
        Self::Matrix,
        Self::Presolve,
        Self::Solve,
        Self::Analysis,
    ];

    fn index(self) -> usize {
        match self {
            Self::Overview => 0,
            Self::Variables => 1,
            Self::Constraints => 2,
            Self::Matrix => 3,
            Self::Presolve => 4,
            Self::Solve => 5,
            Self::Analysis => 6,
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum SolverTask {
    Idle,
    Presolving(SolverBackend),
    Solving(SolverBackend, SolveMode),
    Analyzing(SolverBackend),
    LoadingReductions,
    RootCuts(SolverBackend),
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum ModelSource {
    Original,
    Presolved,
}

#[derive(Clone, Copy)]
struct MatrixViewport {
    zoom: usize,
    row: usize,
    column: usize,
    component: Option<usize>,
}

struct App {
    model: Arc<Model>,
    model_source: ModelSource,
    solver_backend: SolverBackend,
    path: String,
    view: View,
    selected: usize,
    search: String,
    editing_search: bool,
    filter_open: bool,
    filter_cursor: usize,
    constraint_detail_open: bool,
    component_filter_open: bool,
    component_cursor: usize,
    active_component: Option<usize>,
    active_variable_filters: BTreeSet<usize>,
    variable_filters_all: bool,
    active_constraint_types: BTreeSet<ConstraintTag>,
    constraint_types_all: bool,
    nonzero_filter: Option<usize>,
    nonzero_operator: NonzeroOperator,
    editing_nonzero: bool,
    nonzero_input: String,
    matrix_zoom: usize,
    matrix_row: usize,
    matrix_column: usize,
    solver_tx: Sender<SolverEvent>,
    solver_rx: Receiver<SolverEvent>,
    solver_task: SolverTask,
    presolve_error: Option<String>,
    solve_error: Option<String>,
    presolve_report: Option<PresolveReport>,
    solve_report: Option<SolveReport>,
    solution_applied: bool,
    solver_logs: Vec<String>,
    log_scroll: usize,
    params_text: String,
    params_open: bool,
    params_cursor: usize,
    editing_param: bool,
    analysis_report: Option<AnalysisReport>,
    analysis_error: Option<String>,
    analysis_tab: usize,
    analysis_selected: usize,
    reductions: Option<Vec<ReductionItem>>,
    reductions_error: Option<String>,
    stepper_open: bool,
    step_index: usize,
    root_cuts: Option<RootCutStats>,
    root_cuts_error: Option<String>,
    mouse_position: Option<(u16, u16)>,
}

impl App {
    fn new(model: Model, path: String) -> Self {
        let (solver_tx, solver_rx) = mpsc::channel();
        Self {
            model: Arc::new(model),
            model_source: ModelSource::Original,
            solver_backend: SolverBackend::Highs,
            path,
            view: View::Overview,
            selected: 0,
            search: String::new(),
            editing_search: false,
            filter_open: false,
            filter_cursor: 0,
            constraint_detail_open: false,
            component_filter_open: false,
            component_cursor: 0,
            active_component: None,
            active_variable_filters: BTreeSet::new(),
            variable_filters_all: false,
            active_constraint_types: BTreeSet::new(),
            constraint_types_all: false,
            nonzero_filter: None,
            nonzero_operator: NonzeroOperator::LessOrEqual,
            editing_nonzero: false,
            nonzero_input: String::new(),
            matrix_zoom: 1,
            matrix_row: 0,
            matrix_column: 0,
            solver_tx,
            solver_rx,
            solver_task: SolverTask::Idle,
            presolve_error: None,
            solve_error: None,
            presolve_report: None,
            solve_report: None,
            solution_applied: false,
            solver_logs: Vec::new(),
            log_scroll: 0,
            params_text: String::new(),
            params_open: false,
            params_cursor: 0,
            editing_param: false,
            analysis_report: None,
            analysis_error: None,
            analysis_tab: 0,
            analysis_selected: 0,
            reductions: None,
            reductions_error: None,
            stepper_open: false,
            step_index: 0,
            root_cuts: None,
            root_cuts_error: None,
            mouse_position: None,
        }
    }

    fn draw(&mut self, frame: &mut Frame) {
        let area = frame.area();
        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                LayoutConstraint::Length(3),
                LayoutConstraint::Min(6),
                LayoutConstraint::Length(1),
            ])
            .split(area);

        self.draw_header(frame, chunks[0]);
        match self.view {
            View::Overview => self.draw_overview(frame, chunks[1]),
            View::Variables => self.draw_variables(frame, chunks[1]),
            View::Constraints => self.draw_constraints(frame, chunks[1]),
            View::Matrix => self.draw_matrix(frame, chunks[1]),
            View::Presolve => self.draw_presolve(frame, chunks[1]),
            View::Solve => self.draw_solve(frame, chunks[1]),
            View::Analysis => self.draw_analysis(frame, chunks[1]),
        }
        if self.filter_open {
            self.draw_filter_popup(frame, chunks[1]);
        }
        if self.component_filter_open {
            self.draw_component_filter_popup(frame, chunks[1]);
        }
        if self.params_open {
            self.draw_params_popup(frame, chunks[1]);
        }
        if self.view == View::Matrix {
            self.draw_matrix_tooltip(frame);
        }
        self.draw_status(frame, chunks[2]);
    }

    fn draw_header(&self, frame: &mut Frame, area: Rect) {
        let titles = [
            "1 Overview",
            "2 Vars",
            "3 Rows",
            "4 Matrix",
            "5 Presolve",
            "6 Solve",
            "7 Analysis",
        ]
        .into_iter()
        .map(Line::from)
        .collect::<Vec<_>>();
        let component = self
            .active_component
            .map(|index| format!(" · component {}", index + 1))
            .unwrap_or_default();
        let variables = if self.active_variable_filters.is_empty() {
            String::new()
        } else {
            format!(
                " · {} selected var{} ({})",
                self.active_variable_filters.len(),
                if self.active_variable_filters.len() == 1 {
                    ""
                } else {
                    "s"
                },
                if self.variable_filters_all {
                    "ALL"
                } else {
                    "ANY"
                }
            )
        };
        let solution = if self.solution_applied {
            self.solve_report
                .as_ref()
                .map(|report| {
                    format!(
                        " · solution: {} {}",
                        report.backend.label(),
                        report.mode.label()
                    )
                })
                .unwrap_or_default()
        } else {
            String::new()
        };
        let tabs = Tabs::new(titles)
            .select(self.view.index())
            .block(
                Block::default()
                    .title(format!(
                        " mipviz · {} · solver: {}{}{}{} ",
                        self.active_model().name,
                        self.solver_backend.label(),
                        component,
                        variables,
                        solution
                    ))
                    .borders(Borders::ALL),
            )
            .highlight_style(Style::default().fg(ACCENT).add_modifier(Modifier::BOLD))
            .divider(" │ ");
        frame.render_widget(tabs, area);
    }

    fn active_model(&self) -> &Model {
        match self.model_source {
            ModelSource::Original => &self.model,
            ModelSource::Presolved => self
                .presolve_report
                .as_ref()
                .map(|report| &report.model)
                .unwrap_or(&self.model),
        }
    }

    fn set_model_source(&mut self, source: ModelSource) {
        if source == ModelSource::Presolved && self.presolve_report.is_none() {
            return;
        }
        self.model_source = source;
        if source == ModelSource::Presolved {
            self.solution_applied = false;
        }
        self.selected = 0;
        self.search.clear();
        self.editing_search = false;
        self.filter_open = false;
        self.component_filter_open = false;
        self.active_component = None;
        self.active_variable_filters.clear();
        self.component_cursor = 0;
        self.matrix_zoom = 1;
        self.matrix_row = 0;
        self.matrix_column = 0;
    }

    fn draw_overview(&self, frame: &mut Frame, area: Rect) {
        let model = self.active_model();
        let columns = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([
                LayoutConstraint::Percentage(48),
                LayoutConstraint::Percentage(52),
            ])
            .split(area);

        let binaries = model
            .variables
            .iter()
            .filter(|variable| variable.kind == VariableKind::Binary)
            .count();
        let integers = model
            .variables
            .iter()
            .filter(|variable| variable.kind == VariableKind::Integer)
            .count();
        let continuous = model.variables.len() - binaries - integers;
        let density = if model.variables.is_empty() || model.constraints.is_empty() {
            0.0
        } else {
            model.nonzeros as f64 / model.variables.len() as f64 / model.constraints.len() as f64
        };

        let summary = Text::from(vec![
            Line::from(vec![muted("Path       "), Span::raw(&self.path)]),
            Line::from(vec![
                muted("Sense      "),
                Span::raw(if model.minimize {
                    "minimize"
                } else {
                    "maximize"
                }),
            ]),
            Line::from(vec![
                muted("Variables  "),
                Span::raw(format_count(model.variables.len())),
            ]),
            Line::from(vec![
                muted("Constraints"),
                Span::raw(format!("  {}", format_count(model.constraints.len()))),
            ]),
            Line::from(vec![
                muted("Nonzeros   "),
                Span::raw(format_count(model.nonzeros)),
            ]),
            Line::from(vec![
                muted("Density    "),
                Span::raw(format!("{:.4}%", density * 100.0)),
            ]),
            Line::from(""),
            Line::from(vec![
                Span::styled("● ", Style::default().fg(GREEN)),
                Span::raw(format!("{binaries} binary")),
            ]),
            Line::from(vec![
                Span::styled("● ", Style::default().fg(ORANGE)),
                Span::raw(format!("{integers} integer")),
            ]),
            Line::from(vec![
                Span::styled("● ", Style::default().fg(ACCENT)),
                Span::raw(format!("{continuous} continuous")),
            ]),
            Line::from(""),
            Line::from(vec![
                muted("Components "),
                Span::styled(
                    format_count(model.components.components.len()),
                    Style::default().fg(component_color(0)),
                ),
                Span::raw(if model.components.components.len() == 1 {
                    " connected block"
                } else {
                    " disconnected blocks"
                }),
            ]),
        ]);
        frame.render_widget(
            Paragraph::new(summary)
                .block(Block::default().title(" Model ").borders(Borders::ALL))
                .wrap(Wrap { trim: false }),
            columns[0],
        );

        let right = Layout::default()
            .direction(Direction::Vertical)
            .constraints([LayoutConstraint::Length(3), LayoutConstraint::Min(7)])
            .split(columns[1]);
        let integer_ratio = if model.variables.is_empty() {
            0.0
        } else {
            (binaries + integers) as f64 / model.variables.len() as f64
        };
        frame.render_widget(
            Gauge::default()
                .block(
                    Block::default()
                        .title(" Integrality ")
                        .borders(Borders::ALL),
                )
                .gauge_style(Style::default().fg(GREEN))
                .ratio(integer_ratio)
                .label(format!("{:.1}% discrete", integer_ratio * 100.0)),
            right[0],
        );

        let ranges = Self::range_rows(model);
        let table = Table::new(
            ranges,
            [
                LayoutConstraint::Length(12),
                LayoutConstraint::Percentage(42),
                LayoutConstraint::Percentage(42),
            ],
        )
        .header(
            Row::new(["Range", "Minimum", "Maximum"])
                .style(Style::default().fg(MUTED).add_modifier(Modifier::BOLD)),
        )
        .block(
            Block::default()
                .title(" Coefficient ranges ")
                .borders(Borders::ALL),
        );
        frame.render_widget(table, right[1]);
    }

    fn range_rows(model: &Model) -> Vec<Row<'static>> {
        let matrix = model
            .constraints
            .iter()
            .flat_map(|row| row.terms.iter().map(|term| term.coefficient.abs()))
            .filter(|value| *value > 0.0);
        let objective = model
            .variables
            .iter()
            .map(|variable| variable.objective.abs())
            .filter(|value| *value > 0.0);
        let bounds = model
            .variables
            .iter()
            .flat_map(|variable| [variable.lower, variable.upper]);
        let rhs = model
            .constraints
            .iter()
            .flat_map(|row| [row.lower, row.upper]);
        [
            range_row("Matrix", matrix),
            range_row("Objective", objective),
            range_row("Bounds", bounds),
            range_row("RHS", rhs),
            Row::new([
                Cell::from("Obj offset"),
                Cell::from(format_number(model.objective_offset)),
                Cell::from(""),
            ]),
        ]
        .into_iter()
        .collect()
    }

    fn draw_variables(&mut self, frame: &mut Frame, area: Rect) {
        let indices = self.filtered_variable_indices();
        self.selected = clamp_selection(self.selected, indices.len());
        let model = self.active_model();
        let applied_report = if self.solution_applied {
            self.solve_report.as_ref()
        } else {
            None
        };
        let mut state =
            TableState::default().with_selected((!indices.is_empty()).then_some(self.selected));
        let rows = indices
            .iter()
            .map(|&index| {
                let variable = &model.variables[index];
                let filtered = self.active_variable_filters.contains(&index);
                let mut cells = vec![
                    Cell::from(index.to_string()),
                    Cell::from(variable.name.clone()).style(if filtered {
                        Style::default().fg(ACCENT).add_modifier(Modifier::BOLD)
                    } else {
                        Style::default()
                    }),
                    Cell::from(variable.kind.label())
                        .style(Style::default().fg(variable.kind.color())),
                    Cell::from(format_bound(variable.lower)),
                    Cell::from(format_bound(variable.upper)),
                    Cell::from(format_number(variable.objective)),
                ];
                let fractional = applied_report
                    .and_then(|report| report.values.get(index))
                    .is_some_and(|value| is_fractional(variable, *value));
                if let Some(value) = applied_report.and_then(|report| report.values.get(index)) {
                    let style = if fractional {
                        Style::default().fg(ORANGE).add_modifier(Modifier::BOLD)
                    } else {
                        Style::default().fg(GREEN)
                    };
                    cells.push(Cell::from(format_number(*value)).style(style));
                }
                let row = Row::new(cells);
                if fractional {
                    row.style(Style::default().bg(Color::Rgb(55, 42, 30)))
                } else if filtered {
                    row.style(Style::default().bg(Color::Rgb(24, 43, 58)))
                } else {
                    row
                }
            })
            .collect::<Vec<_>>();
        let fractional_count = applied_report.map_or(0, |report| {
            indices
                .iter()
                .filter(|&&index| {
                    report
                        .values
                        .get(index)
                        .is_some_and(|value| is_fractional(&model.variables[index], *value))
                })
                .count()
        });
        let mut title = if let Some(component) = self.active_component {
            format!(
                " Variables · component {} · {}/{}{} ",
                component + 1,
                indices.len(),
                model.variables.len(),
                if self.search.is_empty() {
                    String::new()
                } else {
                    format!(" · /{}", self.search)
                }
            )
        } else {
            list_title(
                " Variables ",
                indices.len(),
                model.variables.len(),
                &self.search,
                self.editing_search,
            )
        };
        if applied_report.is_some() {
            title = format!(
                "{} · {} fractional ",
                title.trim_end(),
                format_count(fractional_count)
            );
        }
        if !self.active_variable_filters.is_empty() {
            title = format!(
                "{} · {} selected ({}) ",
                title.trim_end(),
                self.active_variable_filters.len(),
                if self.variable_filters_all {
                    "ALL"
                } else {
                    "ANY"
                }
            );
        }
        let mut header = vec!["#", "Name", "Type", "Lower", "Upper", "Objective"];
        let mut widths = vec![
            LayoutConstraint::Length(7),
            LayoutConstraint::Percentage(38),
            LayoutConstraint::Length(7),
            LayoutConstraint::Length(13),
            LayoutConstraint::Length(13),
            LayoutConstraint::Length(13),
        ];
        if applied_report.is_some() {
            header.push("Value");
            widths = vec![
                LayoutConstraint::Length(6),
                LayoutConstraint::Min(12),
                LayoutConstraint::Length(6),
                LayoutConstraint::Length(10),
                LayoutConstraint::Length(10),
                LayoutConstraint::Length(10),
                LayoutConstraint::Length(11),
            ];
        }
        let table = Table::new(rows, widths)
            .header(Row::new(header).style(Style::default().fg(MUTED).add_modifier(Modifier::BOLD)))
            .row_highlight_style(Style::default().bg(Color::Rgb(38, 49, 60)).fg(Color::White))
            .highlight_symbol("› ")
            .block(Block::default().title(title).borders(Borders::ALL));
        frame.render_stateful_widget(table, area, &mut state);
    }

    fn draw_constraints(&mut self, frame: &mut Frame, area: Rect) {
        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([LayoutConstraint::Length(3), LayoutConstraint::Min(5)])
            .split(area);
        let indices = self.filtered_constraint_indices();
        self.selected = clamp_selection(self.selected, indices.len());
        self.draw_constraint_filter_bar(frame, chunks[0], indices.len());
        let (table_area, detail_area) = if !self.constraint_detail_open {
            (chunks[1], None)
        } else if chunks[1].width >= 120 {
            let columns = Layout::default()
                .direction(Direction::Horizontal)
                .constraints([
                    LayoutConstraint::Percentage(60),
                    LayoutConstraint::Percentage(40),
                ])
                .split(chunks[1]);
            (columns[0], Some(columns[1]))
        } else {
            let rows = Layout::default()
                .direction(Direction::Vertical)
                .constraints([LayoutConstraint::Min(6), LayoutConstraint::Length(7)])
                .split(chunks[1]);
            (rows[0], Some(rows[1]))
        };
        let model = self.active_model();
        let mut state =
            TableState::default().with_selected((!indices.is_empty()).then_some(self.selected));
        let rows = indices.iter().map(|&index| {
            let row = &model.constraints[index];
            Row::new(vec![
                Cell::from(index.to_string()),
                Cell::from(row.name.clone()),
                Cell::from(constraint_expression_line(model, row, 16)),
                Cell::from(constraint_types_line(row)),
                Cell::from(row.terms.len().to_string()),
            ])
        });
        let has_structured_filter = !self.active_constraint_types.is_empty()
            || self.nonzero_filter.is_some()
            || !self.active_variable_filters.is_empty()
            || self.active_component.is_some();
        let title = if has_structured_filter {
            format!(
                " Constraints · {}/{}{} ",
                indices.len(),
                model.constraints.len(),
                if self.search.is_empty() {
                    String::new()
                } else {
                    format!(" · /{}", self.search)
                }
            )
        } else {
            list_title(
                " Constraints ",
                indices.len(),
                model.constraints.len(),
                &self.search,
                self.editing_search,
            )
        };
        let table = Table::new(
            rows,
            [
                LayoutConstraint::Length(6),
                LayoutConstraint::Percentage(18),
                LayoutConstraint::Min(24),
                LayoutConstraint::Percentage(20),
                LayoutConstraint::Length(6),
            ],
        )
        .header(
            Row::new(["#", "Name", "Constraint expression", "Types", "NZ"])
                .style(Style::default().fg(MUTED).add_modifier(Modifier::BOLD)),
        )
        .row_highlight_style(Style::default().bg(Color::Rgb(38, 49, 60)).fg(Color::White))
        .highlight_symbol("› ")
        .block(Block::default().title(title).borders(Borders::ALL));
        frame.render_stateful_widget(table, table_area, &mut state);

        if let Some(detail_area) = detail_area {
            let detail = indices
                .get(self.selected)
                .map(|&index| constraint_detail(model, index))
                .unwrap_or_else(|| Text::from("No matching constraints"));
            frame.render_widget(
                Paragraph::new(detail)
                    .block(
                        Block::default()
                            .title(" Selected row · d close ")
                            .borders(Borders::ALL),
                    )
                    .wrap(Wrap { trim: false }),
                detail_area,
            );
        }
    }

    fn draw_constraint_filter_bar(&self, frame: &mut Frame, area: Rect, matches: usize) {
        let model = self.active_model();
        let mut spans = vec![Span::styled(
            " f filters ",
            Style::default().fg(Color::Black).bg(ACCENT),
        )];
        if !self.search.is_empty() {
            spans.push(Span::raw(format!("  name contains ‘{}’", self.search)));
        }
        if !self.active_constraint_types.is_empty() {
            spans.push(Span::raw(format!(
                "  types {} ",
                if self.constraint_types_all {
                    "ALL"
                } else {
                    "ANY"
                }
            )));
            for tag in &self.active_constraint_types {
                spans.push(Span::styled(
                    format!(" {} ", tag.label()),
                    Style::default().fg(tag.color()),
                ));
            }
        }
        if let Some(nonzeros) = self.nonzero_filter {
            spans.push(Span::raw(format!(
                "  nz {} {nonzeros}",
                self.nonzero_operator.symbol()
            )));
        }
        if !self.active_variable_filters.is_empty() {
            spans.push(Span::raw(format!(
                "  vars {} ",
                if self.variable_filters_all {
                    "ALL"
                } else {
                    "ANY"
                }
            )));
            for (position, &index) in self.active_variable_filters.iter().take(3).enumerate() {
                if position > 0 {
                    spans.push(Span::raw(", "));
                }
                let label = model
                    .variables
                    .get(index)
                    .map(|variable| variable.name.as_str())
                    .unwrap_or("?");
                spans.push(Span::styled(label.to_owned(), Style::default().fg(ACCENT)));
            }
            if self.active_variable_filters.len() > 3 {
                spans.push(Span::raw(format!(
                    " +{}",
                    self.active_variable_filters.len() - 3
                )));
            }
        }
        if let Some(component) = self.active_component {
            spans.push(Span::styled(
                format!("  component {}", component + 1),
                Style::default().fg(component_color(component)),
            ));
        }
        if self.search.is_empty()
            && self.active_constraint_types.is_empty()
            && self.nonzero_filter.is_none()
            && self.active_variable_filters.is_empty()
            && self.active_component.is_none()
        {
            spans.push(Span::styled("  none active", Style::default().fg(MUTED)));
        }
        spans.push(Span::styled(
            format!("  {matches}/{} rows", model.constraints.len()),
            Style::default().fg(GREEN),
        ));
        frame.render_widget(
            Paragraph::new(Line::from(spans))
                .block(Block::default().borders(Borders::ALL))
                .wrap(Wrap { trim: true }),
            area,
        );
    }

    fn draw_filter_popup(&self, frame: &mut Frame, area: Rect) {
        let model = self.active_model();
        let popup = centered_rect(area, 68, 23);
        frame.render_widget(Clear, popup);
        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([LayoutConstraint::Min(8), LayoutConstraint::Length(4)])
            .split(popup);

        let rows = ConstraintTag::ALL.into_iter().map(|tag| {
            let count = model
                .constraints
                .iter()
                .filter(|row| row.tags.contains(&tag))
                .count();
            let checked = if self.active_constraint_types.contains(&tag) {
                "[×]"
            } else {
                "[ ]"
            };
            Row::new([
                Cell::from(checked),
                Cell::from(tag.label()).style(Style::default().fg(tag.color())),
                Cell::from(format_count(count)),
            ])
        });
        let nz_value = if self.editing_nonzero {
            format!("{}█", self.nonzero_input)
        } else {
            self.nonzero_filter
                .map(format_count)
                .unwrap_or_else(|| "any".to_owned())
        };
        let title = format!(
            " Constraint filters · types {} · nz {} {} · vars {} {} ",
            if self.constraint_types_all {
                "ALL"
            } else {
                "ANY"
            },
            self.nonzero_operator.symbol(),
            nz_value,
            if self.variable_filters_all {
                "ALL"
            } else {
                "ANY"
            },
            self.active_variable_filters.len()
        );
        let mut state = TableState::default().with_selected(Some(self.filter_cursor));
        let table = Table::new(
            rows,
            [
                LayoutConstraint::Length(5),
                LayoutConstraint::Percentage(68),
                LayoutConstraint::Percentage(25),
            ],
        )
        .header(
            Row::new(["", "Type", "Rows"])
                .style(Style::default().fg(MUTED).add_modifier(Modifier::BOLD)),
        )
        .row_highlight_style(Style::default().bg(Color::Rgb(38, 49, 60)).fg(Color::White))
        .highlight_symbol("› ")
        .block(Block::default().title(title).borders(Borders::ALL));
        frame.render_stateful_widget(table, chunks[0], &mut state);

        let help = if self.editing_nonzero {
            Text::from(vec![
                Line::from("Type a nonzero count"),
                Line::from("Enter or Esc finishes editing"),
            ])
        } else {
            Text::from(vec![
                Line::from("↑↓ select · Space toggle · o switches ANY/ALL"),
                Line::from("n edit nz · [ ] operator · vars: view 2 + Space · x clear all"),
            ])
        };
        frame.render_widget(
            Paragraph::new(help)
                .alignment(Alignment::Center)
                .style(Style::default().fg(MUTED))
                .block(Block::default().borders(Borders::ALL)),
            chunks[1],
        );
    }

    fn draw_component_filter_popup(&self, frame: &mut Frame, area: Rect) {
        let model = self.active_model();
        let height = (model.components.components.len() as u16 + 6).min(24);
        let popup = centered_rect(area, 72, height.max(8));
        frame.render_widget(Clear, popup);
        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([LayoutConstraint::Min(3), LayoutConstraint::Length(3)])
            .split(popup);
        let rows = model
            .components
            .components
            .iter()
            .enumerate()
            .map(|(index, component)| {
                Row::new([
                    component_cell(index),
                    Cell::from(format_count(component.rows.len())),
                    Cell::from(format_count(component.columns.len())),
                    Cell::from(format_count(component.nonzeros)),
                    Cell::from(format!(
                        "{}/{}/{}",
                        component.continuous, component.integer, component.binary
                    )),
                ])
            });
        let mut state = TableState::default().with_selected(
            (!model.components.components.is_empty()).then_some(self.component_cursor),
        );
        let table = Table::new(
            rows,
            [
                LayoutConstraint::Length(7),
                LayoutConstraint::Length(9),
                LayoutConstraint::Length(9),
                LayoutConstraint::Length(10),
                LayoutConstraint::Min(12),
            ],
        )
        .header(
            Row::new(["Block", "Rows", "Columns", "Nonzeros", "C/I/B"])
                .style(Style::default().fg(MUTED).add_modifier(Modifier::BOLD)),
        )
        .row_highlight_style(Style::default().bg(Color::Rgb(38, 49, 60)).fg(Color::White))
        .highlight_symbol("› ")
        .block(
            Block::default()
                .title(" Component filter ")
                .borders(Borders::ALL),
        );
        frame.render_stateful_widget(table, chunks[0], &mut state);
        frame.render_widget(
            Paragraph::new("↑↓ select · Enter apply · x clear · Esc cancel")
                .alignment(Alignment::Center)
                .style(Style::default().fg(MUTED))
                .block(Block::default().borders(Borders::ALL)),
            chunks[1],
        );
    }

    fn draw_params_popup(&self, frame: &mut Frame, area: Rect) {
        let lines: Vec<&str> = self.params_text.lines().collect();
        let height = (lines.len() as u16 + 7).clamp(8, 24);
        let popup = centered_rect(area, 72, height);
        frame.render_widget(Clear, popup);
        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([LayoutConstraint::Min(3), LayoutConstraint::Length(4)])
            .split(popup);
        let mut state =
            TableState::default().with_selected((!lines.is_empty()).then_some(self.params_cursor));
        let rows = lines.iter().enumerate().map(|(index, line)| {
            let editing = self.editing_param && index == self.params_cursor;
            Row::new([Cell::from(if editing {
                format!("{line}█")
            } else {
                (*line).to_owned()
            })])
        });
        let table = Table::new(rows, [LayoutConstraint::Percentage(100)])
            .header(
                Row::new([format!(
                    " {} parameters ",
                    self.solver_backend.label()
                )])
                .style(Style::default().fg(MUTED).add_modifier(Modifier::BOLD)),
            )
            .row_highlight_style(Style::default().bg(Color::Rgb(38, 49, 60)).fg(Color::White))
            .highlight_symbol("› ")
            .block(
                Block::default()
                    .title(
                        " Solver parameters · name = value per line (# comments) ",
                    )
                    .borders(Borders::ALL),
            );
        frame.render_stateful_widget(table, chunks[0], &mut state);
        let help = if self.editing_param {
            "Editing line — Enter/Esc finishes".to_owned()
        } else {
            format!(
                "{} · ↑↓ select · Enter edit · a append · d delete · Esc close",
                if self.params_text.trim().is_empty() {
                    "no parameters set"
                } else {
                    "parameters active"
                }
            )
        };
        frame.render_widget(
            Paragraph::new(help)
                .alignment(Alignment::Center)
                .style(Style::default().fg(MUTED))
                .block(Block::default().borders(Borders::ALL)),
            chunks[1],
        );
    }

    fn draw_matrix_tooltip(&self, frame: &mut Frame) {
        let Some((mouse_x, mouse_y)) = self.mouse_position else {
            return;
        };
        let model = self.active_model();
        let area = frame.area();
        // Replicate the matrix view layout: 3-line header on top, 1-line
        // status at the bottom, and a 35-wide right column when wide enough.
        let content = Rect::new(area.x, area.y + 3, area.width, area.height.saturating_sub(4));
        let panel = if content.width >= 76 {
            Rect::new(content.x, content.y, content.width - 35, content.height)
        } else {
            content
        };
        let inner = Rect::new(
            panel.x + 1,
            panel.y + 1,
            panel.width.saturating_sub(2),
            panel.height.saturating_sub(2),
        );
        if mouse_x < inner.x
            || mouse_y < inner.y
            || mouse_x >= inner.x + inner.width
            || mouse_y >= inner.y + inner.height
        {
            return;
        }
        let (hidden_rows, hidden_columns) = self.stepper_hidden();
        let Some((row, column, coefficient)) = matrix_cell_at(
            model,
            (mouse_x - inner.x) as usize,
            (mouse_y - inner.y) as usize,
            inner.width as usize,
            inner.height as usize,
            MatrixViewport {
                zoom: self.matrix_zoom,
                row: self.matrix_row,
                column: self.matrix_column,
                component: self.active_component,
            },
            &hidden_rows,
            &hidden_columns,
        ) else {
            return;
        };
        let variable = &model.variables[column];
        let row_name = &model.constraints[row].name;
        let mut spans = vec![
            Span::styled(
                row_name.clone(),
                Style::default().fg(MUTED).add_modifier(Modifier::BOLD),
            ),
            Span::raw(" × "),
            Span::styled(
                variable.name.clone(),
                Style::default().fg(variable.kind.color()),
            ),
            Span::styled(
                format!(" = {}", format_number(coefficient)),
                Style::default().fg(ACCENT),
            ),
        ];
        if let Some(values) = self.matrix_lp_values()
            && let Some(value) = values.get(column)
        {
            let fractional = is_fractional(variable, *value);
            spans.push(Span::raw("  LP: "));
            spans.push(Span::styled(
                format_number(*value),
                Style::default().fg(if fractional { ORANGE } else { GREEN }),
            ));
            if fractional {
                spans.push(Span::styled(
                    " (fractional)",
                    Style::default().fg(ORANGE),
                ));
            }
        }
        let width = spans.iter().map(|span| span.content.len() as u16).sum::<u16>() + 6;
        let height = 3u16;
        let x = (mouse_x + 2).min(area.width.saturating_sub(width));
        let y = mouse_y.saturating_sub(height + 1);
        frame.render_widget(Clear, Rect::new(x, y, width, height));
        frame.render_widget(
            Paragraph::new(Line::from(spans))
                .block(
                    Block::default()
                        .borders(Borders::ALL)
                        .style(Style::default().fg(ACCENT)),
                )
                .style(Style::default()),
            Rect::new(x, y, width, height),
        );
    }

    fn draw_matrix(&mut self, frame: &mut Frame, area: Rect) {
        let model = self.active_model();
        let (hidden_rows, hidden_columns) = self.stepper_hidden();
        let lp_values = self.matrix_lp_values();
        let (rows, columns, nonzeros) = self
            .active_component
            .and_then(|index| model.components.components.get(index))
            .map(|component| {
                (
                    component.rows.len(),
                    component.columns.len(),
                    component.nonzeros,
                )
            })
            .unwrap_or((
                model.constraints.len(),
                model.variables.len(),
                model.nonzeros,
            ));
        let stepping = self.stepper_open && self.reductions.is_some();
        let mut title = format!(
            " {} sparsity{} · {}×{} · {} nz · zoom {}× ",
            if self.model_source == ModelSource::Presolved {
                "Presolved"
            } else {
                "Original"
            },
            self.active_component
                .map(|index| format!(" · component {}", index + 1))
                .unwrap_or_default(),
            format_count(rows),
            format_count(columns),
            format_count(nonzeros),
            self.matrix_zoom
        );
        if stepping {
            let total = self.reductions.as_ref().map_or(0, Vec::len);
            let item = self
                .reductions
                .as_ref()
                .and_then(|items| items.get(self.step_index.saturating_sub(1)));
            match item {
                Some(item) => title = format!("{title} step {}/{} · {} ", self.step_index, total, item.description),
                None => title = format!("{title} step 0/{total} · original matrix "),
            }
        }
        let viewport = MatrixViewport {
            zoom: self.matrix_zoom,
            row: self.matrix_row,
            column: self.matrix_column,
            component: self.active_component,
        };
        if area.width >= 76 {
            let columns = Layout::default()
                .direction(Direction::Horizontal)
                .constraints([LayoutConstraint::Min(30), LayoutConstraint::Length(35)])
                .split(area);
            let right = Layout::default()
                .direction(Direction::Vertical)
                .constraints([LayoutConstraint::Length(11), LayoutConstraint::Min(4)])
                .split(columns[1]);
            Self::draw_matrix_panel(
                frame,
                columns[0],
                model,
                title,
                viewport,
                &hidden_rows,
                &hidden_columns,
                lp_values,
            );
            Self::draw_minimap(
                frame,
                right[0],
                model,
                viewport,
                &hidden_rows,
                &hidden_columns,
            );
            Self::draw_components(
                frame,
                right[1],
                model,
                " Connected components ",
                self.active_component,
            );
        } else {
            Self::draw_matrix_panel(
                frame,
                area,
                model,
                title,
                viewport,
                &hidden_rows,
                &hidden_columns,
                lp_values,
            );
        }
    }

    fn draw_matrix_panel(
        frame: &mut Frame,
        area: Rect,
        model: &Model,
        title: String,
        viewport: MatrixViewport,
        hidden_rows: &BTreeSet<usize>,
        hidden_columns: &BTreeSet<usize>,
        lp_values: Option<&[f64]>,
    ) {
        let block = Block::default().title(title).borders(Borders::ALL);
        let inner = block.inner(area);
        frame.render_widget(block, area);
        if inner.width > 0 && inner.height > 0 {
            frame.render_widget(
                Paragraph::new(matrix_braille(
                    model,
                    inner.width as usize,
                    inner.height as usize,
                    viewport.zoom,
                    viewport.row,
                    viewport.column,
                    viewport.component,
                    hidden_rows,
                    hidden_columns,
                    lp_values,
                )),
                inner,
            );
        }
    }

    fn draw_minimap(
        frame: &mut Frame,
        area: Rect,
        model: &Model,
        viewport: MatrixViewport,
        hidden_rows: &BTreeSet<usize>,
        hidden_columns: &BTreeSet<usize>,
    ) {
        let block = Block::default().title(" Minimap ").borders(Borders::ALL);
        let inner = block.inner(area);
        frame.render_widget(block, area);
        if inner.width < 3 || inner.height < 3 {
            return;
        }
        let text = matrix_minimap(
            model,
            inner.width as usize,
            inner.height as usize,
            viewport,
            hidden_rows,
            hidden_columns,
        );
        frame.render_widget(Paragraph::new(text), inner);
    }

    fn draw_components(
        frame: &mut Frame,
        area: Rect,
        model: &Model,
        title: &str,
        selected: Option<usize>,
    ) {
        let block = Block::default()
            .title(format!(
                "{}· {} ",
                title,
                format_count(model.components.components.len())
            ))
            .borders(Borders::ALL);
        let header_style = Style::default().fg(MUTED).add_modifier(Modifier::BOLD);
        if area.width >= 44 {
            let rows = model
                .components
                .components
                .iter()
                .enumerate()
                .map(|(index, component)| {
                    let row = Row::new([
                        component_cell(index),
                        Cell::from(format_count(component.rows.len())),
                        Cell::from(format_count(component.columns.len())),
                        Cell::from(format_count(component.nonzeros)),
                        Cell::from(format!(
                            "{}/{}/{}",
                            component.continuous, component.integer, component.binary
                        )),
                    ]);
                    component_row_style(row, selected == Some(index))
                });
            frame.render_widget(
                Table::new(
                    rows,
                    [
                        LayoutConstraint::Length(5),
                        LayoutConstraint::Length(5),
                        LayoutConstraint::Length(6),
                        LayoutConstraint::Length(7),
                        LayoutConstraint::Min(9),
                    ],
                )
                .header(Row::new(["#", "Rows", "Cols", "NZ", "C/I/B"]).style(header_style))
                .block(block),
                area,
            );
        } else {
            let rows = model
                .components
                .components
                .iter()
                .enumerate()
                .map(|(index, component)| {
                    let row = Row::new([
                        component_cell(index),
                        Cell::from(format_count(component.rows.len())),
                        Cell::from(format_count(component.columns.len())),
                        Cell::from(format_count(component.nonzeros)),
                    ]);
                    component_row_style(row, selected == Some(index))
                });
            frame.render_widget(
                Table::new(
                    rows,
                    [
                        LayoutConstraint::Length(5),
                        LayoutConstraint::Length(6),
                        LayoutConstraint::Length(7),
                        LayoutConstraint::Min(5),
                    ],
                )
                .header(Row::new(["#", "Rows", "Cols", "NZ"]).style(header_style))
                .block(block),
                area,
            );
        }
    }

    fn draw_presolve(&self, frame: &mut Frame, area: Rect) {
        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([LayoutConstraint::Length(9), LayoutConstraint::Min(5)])
            .split(area);

        let (title, body, style) = if let SolverTask::Presolving(backend) = self.solver_task {
            (
                format!(" {} presolve · running ", backend.label()),
                Text::from(vec![
                    Line::from("Reducing the model on a background thread…"),
                    Line::from(""),
                    Line::from(format!(
                        "Original: {} columns · {} rows · {} nonzeros",
                        format_count(self.model.variables.len()),
                        format_count(self.model.constraints.len()),
                        format_count(self.model.nonzeros)
                    )),
                ]),
                Style::default().fg(ORANGE),
            )
        } else if let Some(error) = &self.presolve_error {
            (
                format!(" {} presolve · error ", self.solver_backend.label()),
                Text::from(vec![
                    Line::from(error.clone()),
                    Line::from("Press b to change backend or p to retry."),
                ]),
                Style::default().fg(Color::Red),
            )
        } else if let Some(report) = &self.presolve_report {
            let presolved = &report.model;
            let removed_columns = self
                .model
                .variables
                .len()
                .saturating_sub(presolved.variables.len());
            let removed_rows = self
                .model
                .constraints
                .len()
                .saturating_sub(presolved.constraints.len());
            let removed_nonzeros = self.model.nonzeros.saturating_sub(presolved.nonzeros);
            (
                format!(" {} presolve · complete ", report.backend.label()),
                Text::from(vec![
                    comparison_line(
                        "Columns ",
                        self.model.variables.len(),
                        presolved.variables.len(),
                        removed_columns,
                    ),
                    comparison_line(
                        "Rows    ",
                        self.model.constraints.len(),
                        presolved.constraints.len(),
                        removed_rows,
                    ),
                    comparison_line(
                        "Nonzeros",
                        self.model.nonzeros,
                        presolved.nonzeros,
                        removed_nonzeros,
                    ),
                    Line::from(format!(
                        "{} recorded reductions · {}",
                        format_count(report.reductions),
                        format_duration(report.elapsed)
                    )),
                    Line::from(format!(
                        "{} connected components · press a to load in views 1–4",
                        format_count(presolved.components.components.len())
                    )),
                ]),
                Style::default().fg(GREEN),
            )
        } else {
            (
                format!(" {} presolve ", self.solver_backend.label()),
                Text::from(vec![
                    Line::from(format!(
                        "Press p to presolve this model with {}.",
                        self.solver_backend.label()
                    )),
                    Line::from(""),
                    Line::from("Press b to switch between HiGHS and SCIP."),
                ]),
                Style::default().fg(ACCENT),
            )
        };
        frame.render_widget(
            Paragraph::new(body)
                .style(style)
                .block(Block::default().title(title).borders(Borders::ALL)),
            chunks[0],
        );

        if let Some(report) = &self.presolve_report
            && self.solver_task == SolverTask::Idle
        {
            let lower = Layout::default()
                .direction(Direction::Horizontal)
                .constraints([
                    LayoutConstraint::Percentage(66),
                    LayoutConstraint::Percentage(34),
                ])
                .split(chunks[1]);
            Self::draw_matrix_panel(
                frame,
                lower[0],
                &report.model,
                " Presolved sparsity · component ordered ".to_owned(),
                MatrixViewport {
                    zoom: 1,
                    row: 0,
                    column: 0,
                    component: None,
                },
                &BTreeSet::new(),
                &BTreeSet::new(),
                None,
            );
            let right = Layout::default()
                .direction(Direction::Vertical)
                .constraints([
                    LayoutConstraint::Percentage(62),
                    LayoutConstraint::Percentage(38),
                ])
                .split(lower[1]);
            Self::draw_components(
                frame,
                right[0],
                &report.model,
                " Presolved components ",
                None,
            );
            let mut reductions = report.reduction_types.iter().collect::<Vec<_>>();
            reductions.sort_by_key(|(_, count)| std::cmp::Reverse(*count));
            let rows = reductions.into_iter().map(|(kind, count)| {
                Row::new([Cell::from(kind.clone()), Cell::from(format_count(*count))])
            });
            let table = Table::new(
                rows,
                [
                    LayoutConstraint::Percentage(70),
                    LayoutConstraint::Percentage(30),
                ],
            )
            .header(
                Row::new(["Reduction type", "Count"])
                    .style(Style::default().fg(MUTED).add_modifier(Modifier::BOLD)),
            )
            .block(
                Block::default()
                    .title(" Reduction breakdown ")
                    .borders(Borders::ALL),
            );
            frame.render_widget(table, right[1]);
        } else {
            self.draw_log_panel(frame, chunks[1], " Presolve log ");
        }
    }

    fn draw_solve(&self, frame: &mut Frame, area: Rect) {
        let has_solution = self.solve_report.is_some() && self.solver_task == SolverTask::Idle;
        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints(if has_solution {
                vec![
                    LayoutConstraint::Length(6),
                    LayoutConstraint::Min(6),
                    LayoutConstraint::Length(8),
                ]
            } else {
                vec![LayoutConstraint::Length(6), LayoutConstraint::Min(6)]
            })
            .split(area);

        let (title, text, style) = match self.solver_task {
            SolverTask::Solving(backend, mode) => (
                format!(" {} {} · running ", backend.label(), mode.label()),
                Text::from(vec![
                    Line::from("Solving on a background thread…"),
                    Line::from("The default time limit for this preview is 60 seconds."),
                ]),
                Style::default().fg(ORANGE),
            ),
            SolverTask::RootCuts(backend) => (
                format!(" {} root cuts · running ", backend.label()),
                Text::from(vec![
                    Line::from("Running a short MIP solve to capture root-node cuts…"),
                    Line::from("The default time limit for this preview is 60 seconds."),
                ]),
                Style::default().fg(ORANGE),
            ),
            _ if self.solve_error.is_some() => (
                format!(" {} solve · error ", self.solver_backend.label()),
                Text::from(vec![
                    Line::from(self.solve_error.clone().unwrap_or_default()),
                    Line::from("Press l for the LP relaxation or m for the MIP."),
                ]),
                Style::default().fg(Color::Red),
            ),
            _ if self.solve_report.is_some() => {
                let report = self.solve_report.as_ref().expect("checked above");
                let mut text = vec![
                    Line::from(format!("Objective   {}", format_number(report.objective))),
                    Line::from(format!(
                        "Elapsed     {} · {} {}",
                        format_duration(report.elapsed),
                        format_count(report.iterations.max(0) as usize),
                        if report.backend == SolverBackend::Scip {
                            "nodes"
                        } else {
                            "iterations"
                        }
                    )),
                ];
                if let Some(cuts) = &self.root_cuts {
                    text.push(Line::from(vec![
                        muted("Root cuts   "),
                        Span::styled(
                            format_count(cuts.total_cuts),
                            Style::default().fg(ORANGE).add_modifier(Modifier::BOLD),
                        ),
                        Span::styled(
                            format!(
                                " total · {} separation round{} · ",
                                format_count(cuts.separation_rounds),
                                if cuts.separation_rounds == 1 { "" } else { "s" }
                            ),
                            Style::default().fg(MUTED),
                        ),
                        if cuts.backend == SolverBackend::Highs {
                            Span::raw(format!(
                                "{} in LP  (press r to re-extract)",
                                format_count(cuts.cuts_in_lp)
                            ))
                        } else {
                            Span::raw(format!(
                                "{} root LP rows  (press r to re-extract)",
                                format_count(cuts.lp_rows)
                            ))
                        },
                    ]));
                }
                text.push(Line::from(if report.objective.is_finite() {
                    "Press Enter to apply this solution to the Variables view."
                } else {
                    "No primal solution is available to apply."
                }));
                (
                    format!(
                        " {} {} · {} ",
                        report.backend.label(),
                        report.mode.label(),
                        report.status
                    ),
                    Text::from(text),
                    if report.status == "Optimal" {
                        Style::default().fg(GREEN)
                    } else {
                        Style::default().fg(ORANGE)
                    },
                )
            }
            _ => (
                format!(" {} solve ", self.solver_backend.label()),
                Text::from(vec![
                    Line::from(format!(
                        "Press l to solve the LP relaxation with {}.",
                        self.solver_backend.label()
                    )),
                    Line::from(format!(
                        "Press m to solve the mixed-integer model with {}.",
                        self.solver_backend.label()
                    )),
                ]),
                Style::default().fg(ACCENT),
            ),
        };
        frame.render_widget(
            Paragraph::new(text)
                .style(style)
                .block(Block::default().title(title).borders(Borders::ALL)),
            chunks[0],
        );
        self.draw_log_panel(frame, chunks[1], " Solver log · ↑/↓ scroll ");

        if let Some(report) = &self.solve_report
            && let Some(solution_area) = chunks.get(2)
        {
            let rows = report
                .values
                .iter()
                .enumerate()
                .filter(|(_, value)| value.abs() > 1e-9)
                .take(100)
                .map(|(index, value)| {
                    Row::new([
                        Cell::from(index.to_string()),
                        Cell::from(self.model.variables[index].name.clone()),
                        Cell::from(format_number(*value)),
                    ])
                });
            let table = Table::new(
                rows,
                [
                    LayoutConstraint::Length(8),
                    LayoutConstraint::Percentage(60),
                    LayoutConstraint::Percentage(30),
                ],
            )
            .header(
                Row::new(["#", "Variable", "Value"])
                    .style(Style::default().fg(MUTED).add_modifier(Modifier::BOLD)),
            )
            .block(
                Block::default()
                    .title(" First nonzero solution values ")
                    .borders(Borders::ALL),
            );
            frame.render_widget(table, *solution_area);
        }
    }

    fn draw_analysis(&mut self, frame: &mut Frame, area: Rect) {
        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([LayoutConstraint::Length(3), LayoutConstraint::Min(5)])
            .split(area);
        let tabs = ["1 Cliques", "2 Implications", "3 Symmetry"]
            .into_iter()
            .map(Line::from)
            .collect::<Vec<_>>();
        frame.render_widget(
            Tabs::new(tabs)
                .select(self.analysis_tab)
                .block(
                    Block::default()
                        .title(format!(
                            " Presolve analysis · HiGHS{}",
                            self.analysis_report
                                .as_ref()
                                .map(|report| {
                                    format!(" · {}", format_duration(report.elapsed))
                                })
                                .unwrap_or_default()
                        ))
                        .borders(Borders::ALL),
                )
                .highlight_style(
                    Style::default()
                        .fg(ACCENT)
                        .add_modifier(Modifier::BOLD),
                )
                .divider(" │ "),
            chunks[0],
        );

        if let Some(error) = &self.analysis_error {
            frame.render_widget(
                Paragraph::new(vec![
                    Line::from(error.clone()),
                    Line::from("Press r to retry."),
                ])
                .style(Style::default().fg(Color::Red))
                .block(
                    Block::default()
                        .title(" Analysis error ")
                        .borders(Borders::ALL),
                ),
                chunks[1],
            );
            return;
        }
        if self.solver_task == SolverTask::Analyzing(SolverBackend::Highs) {
            frame.render_widget(
                Paragraph::new("Analyzing with HiGHS on a background thread…")
                    .style(Style::default().fg(ORANGE))
                    .block(
                        Block::default()
                            .title(" Running ")
                            .borders(Borders::ALL),
                    ),
                chunks[1],
            );
            return;
        }
        let Some(report) = self.analysis_report.clone() else {
            frame.render_widget(
                Paragraph::new(
                    "Run HiGHS presolve analysis (cliques, implications, symmetry) with r.",
                )
                .style(Style::default().fg(ACCENT))
                .block(
                    Block::default()
                        .title(" Analysis · idle ")
                        .borders(Borders::ALL),
                ),
                chunks[1],
            );
            return;
        };

        match self.analysis_tab {
            0 => self.draw_cliques(frame, chunks[1], &report),
            1 => self.draw_implications(frame, chunks[1], &report),
            _ => self.draw_symmetry(frame, chunks[1], &report),
        }
    }

    fn draw_cliques(&mut self, frame: &mut Frame, area: Rect, report: &AnalysisReport) {
        let cliques = &report.cliques;
        let size = cliques.len();
        let max_size = cliques
            .iter()
            .map(|clique| clique.members.len())
            .max()
            .unwrap_or(0);
        let eq = cliques
            .iter()
            .filter(|clique| clique.is_equation)
            .count();
        self.selected = clamp_selection(self.selected, size);
        let mut state =
            TableState::default().with_selected((!cliques.is_empty()).then_some(self.selected));
        let rows = cliques
            .iter()
            .map(|clique| {
                let members = clique
                    .members
                    .iter()
                    .take(24)
                    .map(|(col, value)| {
                        format!(
                            "{}{}·{}",
                            if *value { "" } else { "¬" },
                            report
                                .symmetry_names
                                .get(*col)
                                .cloned()
                                .unwrap_or_else(|| format!("x{col}")),
                            if *value { "1" } else { "0" }
                        )
                    })
                    .collect::<Vec<_>>()
                    .join(" ");
                let more = if clique.members.len() > 24 {
                    format!(" … +{} more", clique.members.len() - 24)
                } else {
                    String::new()
                };
                Row::new([
                    Cell::from(format!("#{}", clique.id)),
                    Cell::from(clique.members.len().to_string()),
                    Cell::from(if clique.is_equation { "eq" } else { "" }),
                    Cell::from(format!("{members}{more}")),
                ])
            })
            .collect::<Vec<_>>();
        let title = format!(
            " Cliques · {} total · max {} · {} eq ",
            format_count(size),
            format_count(max_size),
            format_count(eq)
        );
        let table = Table::new(
            rows,
            [
                LayoutConstraint::Length(6),
                LayoutConstraint::Length(6),
                LayoutConstraint::Length(5),
                LayoutConstraint::Min(24),
            ],
        )
        .header(
            Row::new(["#", "Size", "Type", "Members"])
                .style(Style::default().fg(MUTED).add_modifier(Modifier::BOLD)),
        )
        .row_highlight_style(Style::default().bg(Color::Rgb(38, 49, 60)).fg(Color::White))
        .highlight_symbol("› ")
        .block(Block::default().title(title).borders(Borders::ALL));
        frame.render_stateful_widget(table, area, &mut state);
    }

    fn draw_implications(&mut self, frame: &mut Frame, area: Rect, report: &AnalysisReport) {
        let implications = &report.implications;
        self.selected = clamp_selection(self.selected, implications.len());
        let mut state = TableState::default().with_selected(
            (!implications.is_empty()).then_some(self.selected),
        );
        let rows = implications.iter().map(|imp| {
            let from = report
                .variable_names
                .get(imp.from_col)
                .cloned()
                .unwrap_or_else(|| format!("x{}", imp.from_col));
            let to = report
                .variable_names
                .get(imp.to_col)
                .cloned()
                .unwrap_or_else(|| format!("x{}", imp.to_col));
            Row::new([
                Cell::from(format!(
                    "{} = {}",
                    from,
                    if imp.from_value { "1" } else { "0" }
                )),
                Cell::from("⇒"),
                Cell::from(format!(
                    "{} {} {}",
                    to,
                    if imp.bound_type == "lower" { "≥" } else { "≤" },
                    format_number(imp.bound_value)
                )),
            ])
        });
        let table = Table::new(
            rows,
            [
                LayoutConstraint::Min(14),
                LayoutConstraint::Length(3),
                LayoutConstraint::Min(14),
            ],
        )
        .header(
            Row::new(["From", "", "To"])
                .style(Style::default().fg(MUTED).add_modifier(Modifier::BOLD)),
        )
        .row_highlight_style(Style::default().bg(Color::Rgb(38, 49, 60)).fg(Color::White))
        .highlight_symbol("› ")
        .block(
            Block::default()
                .title(format!(
                    " Implications · {} total ",
                    format_count(report.num_implications)
                ))
                .borders(Borders::ALL),
        );
        frame.render_stateful_widget(table, area, &mut state);
    }

    fn draw_symmetry(&mut self, frame: &mut Frame, area: Rect, report: &AnalysisReport) {
        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([LayoutConstraint::Length(4), LayoutConstraint::Min(4)])
            .split(area);
        let generators = report.symmetry_generators;
        let group_size = if report.symmetry_log10_group_size > 6.0 {
            format!("10^{:.1}", report.symmetry_log10_group_size)
        } else {
            format_count(10f64.powf(report.symmetry_log10_group_size).round() as usize)
        };
        let summary = Paragraph::new(vec![Line::from(vec![
            Span::styled(
                format!("{} generators", format_count(generators)),
                Style::default().fg(ACCENT).add_modifier(Modifier::BOLD),
            ),
            muted("  ·  "),
            Span::styled(
                format!("{} affected vars", format_count(report.symmetry_perm_vars)),
                Style::default().fg(ACCENT).add_modifier(Modifier::BOLD),
            ),
            muted("  ·  "),
            Span::styled(
                format!("{} components", format_count(report.symmetry_components.len())),
                Style::default().fg(ACCENT).add_modifier(Modifier::BOLD),
            ),
            muted("  ·  "),
            Span::styled(
                format!("group size ≈ {group_size}"),
                Style::default().fg(ACCENT).add_modifier(Modifier::BOLD),
            ),
            if report.symmetry_binary_affected {
                Span::styled(
                    "  ·  binary vars affected",
                    Style::default().fg(GREEN),
                )
            } else {
                Span::raw("")
            },
        ])])
        .block(Block::default().title(" Symmetry (HiGHS) ").borders(Borders::ALL));
        frame.render_widget(summary, chunks[0]);

        let components = &report.symmetry_components;
        self.analysis_selected = clamp_selection(self.analysis_selected, components.len());
        let mut state = TableState::default().with_selected(
            (!components.is_empty()).then_some(self.analysis_selected),
        );
        let rows = components
            .iter()
            .enumerate()
            .map(|(index, group)| {
                let names = group
                    .iter()
                    .take(20)
                    .map(|col| {
                        report
                            .symmetry_names
                            .get(*col)
                            .cloned()
                            .unwrap_or_else(|| format!("x{col}"))
                    })
                    .collect::<Vec<_>>()
                    .join(" ");
                let more = if group.len() > 20 {
                    format!(" … +{} more", group.len() - 20)
                } else {
                    String::new()
                };
                Row::new([
                    Cell::from(format!("#{}", index + 1)),
                    Cell::from(group.len().to_string()),
                    Cell::from(format!("{names}{more}")),
                ])
            })
            .collect::<Vec<_>>();
        let table = Table::new(
            rows,
            [
                LayoutConstraint::Length(6),
                LayoutConstraint::Length(8),
                LayoutConstraint::Min(24),
            ],
        )
        .header(
            Row::new(["#", "Vars", "Interchangeable variables"])
                .style(Style::default().fg(MUTED).add_modifier(Modifier::BOLD)),
        )
        .row_highlight_style(Style::default().bg(Color::Rgb(38, 49, 60)).fg(Color::White))
        .highlight_symbol("› ")
        .block(Block::default().title(" Symmetry components ").borders(Borders::ALL));
        frame.render_stateful_widget(table, chunks[1], &mut state);
    }


    fn draw_log_panel(&self, frame: &mut Frame, area: Rect, title: &str) {
        let visible = area.height.saturating_sub(2) as usize;
        let end = self
            .solver_logs
            .len()
            .saturating_sub(self.log_scroll.min(self.solver_logs.len()));
        let start = end.saturating_sub(visible);
        let lines = if start == end {
            vec![Line::from("No solver output yet.")]
        } else {
            self.solver_logs[start..end]
                .iter()
                .cloned()
                .map(Line::from)
                .collect()
        };
        frame.render_widget(
            Paragraph::new(lines)
                .style(Style::default().fg(Color::Gray))
                .block(Block::default().title(title).borders(Borders::ALL)),
            area,
        );
    }

    fn draw_status(&self, frame: &mut Frame, area: Rect) {
        let message = if self.component_filter_open {
            " component picker · Enter applies across views 2–4 · x clears · Esc cancels".to_owned()
        } else if self.filter_open {
            " constraint filter editor open · changes apply immediately".to_owned()
        } else if self.params_open {
            format!(
                " {} parameters · ↑↓ select · Enter edit · a append · d delete · = or Esc close",
                self.solver_backend.label()
            )
        } else if self.editing_search {
            format!(" /{}█   Enter accept · Esc cancel/clear", self.search)
        } else if self.view == View::Matrix {
            if self.stepper_open {
                " [ ] step · 0 reset · s close · +/- zoom · arrows pan · c components · mouse: tooltips · q quit".to_owned()
            } else {
                " arrows/hjkl pan · +/- zoom · s presolve stepper · c components · x clear · a presolved · u original · = params".to_owned()
            }
        } else if self.view == View::Presolve {
            format!(
                " b solver ({}) · p presolve · a load in views 1–4 · u original · = params · q quit",
                self.solver_backend.label()
            )
        } else if self.view == View::Solve {
            if self.solver_task == SolverTask::Idle {
                format!(
                    " b solver ({}) · l LP · m MIP · r root cuts · = params · ↑↓ log · q quit",
                    self.solver_backend.label()
                )
            } else {
                format!(
                    " b solver ({}) · l LP · m MIP · r root cuts · ↑↓ scroll log · q quit",
                    self.solver_backend.label()
                )
            }
        } else if self.view == View::Analysis {
            " r run analysis · 1/2/3 tabs · ↑↓ navigate · = params · q quit".to_owned()
        } else if self.view == View::Constraints {
            format!(
                " ↑↓ navigate · d {} details · / search · f filters · n nonzeros · c components · x clear",
                if self.constraint_detail_open {
                    "hide"
                } else {
                    "show"
                }
            )
        } else if self.view == View::Variables {
            " ↑↓ navigate · Space select var · o ANY/ALL · / search · c components · x clear"
                .to_owned()
        } else {
            " ↑↓/jk navigate · / search · b solver · p presolve · l LP · m MIP · = params · q quit"
                .to_owned()
        };
        frame.render_widget(
            Paragraph::new(message)
                .style(Style::default().fg(MUTED))
                .alignment(Alignment::Center),
            area,
        );
    }

    fn drain_solver_events(&mut self) {
        while let Ok(event) = self.solver_rx.try_recv() {
            match event {
                SolverEvent::Log(line) => {
                    let following_tail = self.log_scroll == 0;
                    self.solver_logs.push(line);
                    if self.solver_logs.len() > 10_000 {
                        self.solver_logs.drain(..1_000);
                    }
                    if following_tail {
                        self.log_scroll = 0;
                    } else {
                        self.log_scroll = self.log_scroll.saturating_add(1);
                    }
                }
                SolverEvent::PresolveFinished(result) => {
                    self.solver_task = SolverTask::Idle;
                    match result {
                        Ok(report) => {
                            // HiGHS presolve also yields the ordered reduction
                            // stack, which powers the matrix stepper.
                            if report.backend == SolverBackend::Highs
                                && !report.reduction_items.is_empty()
                            {
                                self.reductions = Some(report.reduction_items.clone());
                            }
                            self.presolve_report = Some(report);
                        }
                        Err(error) => self.presolve_error = Some(error),
                    }
                }
                SolverEvent::SolveFinished(result) => {
                    self.solver_task = SolverTask::Idle;
                    match result {
                        Ok(report) => {
                            self.solve_report = Some(report);
                            // Pull root-cut stats from the log we just captured.
                            self.root_cuts = Some(solver::parse_root_cuts(
                                &self.solver_logs,
                                self.solver_backend,
                            ));
                            self.root_cuts_error = None;
                        }
                        Err(error) => self.solve_error = Some(error),
                    }
                }
                SolverEvent::ReductionsFinished(result) => {
                    self.solver_task = SolverTask::Idle;
                    match result {
                        Ok(items) => {
                            self.reductions = Some(items);
                            self.reductions_error = None;
                            self.step_index = 0;
                        }
                        Err(error) => self.reductions_error = Some(error),
                    }
                }
                SolverEvent::AnalysisFinished(result) => {
                    self.solver_task = SolverTask::Idle;
                    match result {
                        Ok(report) => self.analysis_report = Some(report),
                        Err(error) => self.analysis_error = Some(error),
                    }
                }
                SolverEvent::RootCutsFinished(result) => {
                    self.solver_task = SolverTask::Idle;
                    match result {
                        Ok(stats) => self.root_cuts = Some(stats),
                        Err(error) => self.root_cuts_error = Some(error),
                    }
                }
            }
        }
    }

    fn start_presolve(&mut self) {
        if self.solver_task != SolverTask::Idle {
            return;
        }
        self.view = View::Presolve;
        self.solution_applied = false;
        self.set_model_source(ModelSource::Original);
        self.presolve_report = None;
        self.presolve_error = None;
        self.solver_logs.clear();
        self.log_scroll = 0;
        self.solver_task = SolverTask::Presolving(self.solver_backend);
        let params = self.params_text.clone();
        match self.solver_backend {
            SolverBackend::Highs => solver::start_highs_presolve(
                Arc::clone(&self.model),
                params,
                self.solver_tx.clone(),
            ),
            SolverBackend::Scip => solver::start_scip_presolve(
                Arc::clone(&self.model),
                self.path.clone(),
                params,
                self.solver_tx.clone(),
            ),
        }
    }

    fn start_solve(&mut self, mode: SolveMode) {
        if self.solver_task != SolverTask::Idle {
            return;
        }
        self.view = View::Solve;
        self.solution_applied = false;
        self.solve_report = None;
        self.solve_error = None;
        self.solver_logs.clear();
        self.log_scroll = 0;
        self.solver_task = SolverTask::Solving(self.solver_backend, mode);
        let params = self.params_text.clone();
        match self.solver_backend {
            SolverBackend::Highs => solver::start_highs_solve(
                Arc::clone(&self.model),
                mode,
                params,
                self.solver_tx.clone(),
            ),
            SolverBackend::Scip => solver::start_scip_solve(
                Arc::clone(&self.model),
                mode,
                params,
                self.solver_tx.clone(),
            ),
        }
    }

    fn start_analysis(&mut self) {
        if self.solver_task != SolverTask::Idle {
            return;
        }
        self.analysis_report = None;
        self.analysis_error = None;
        self.solver_task = SolverTask::Analyzing(SolverBackend::Highs);
        solver::start_highs_analysis(Arc::clone(&self.model), self.solver_tx.clone());
    }

    fn load_reductions(&mut self) {
        if self.solver_task != SolverTask::Idle {
            return;
        }
        self.reductions = None;
        self.reductions_error = None;
        self.step_index = 0;
        self.stepper_open = true;
        self.solver_task = SolverTask::LoadingReductions;
        solver::start_highs_reductions(Arc::clone(&self.model), self.solver_tx.clone());
    }

    fn extract_root_cuts(&mut self) {
        if self.solver_task != SolverTask::Idle {
            return;
        }
        self.root_cuts = None;
        self.root_cuts_error = None;
        self.solver_logs.clear();
        self.log_scroll = 0;
        self.solver_task = SolverTask::RootCuts(self.solver_backend);
        let params = self.params_text.clone();
        let source_path = if self.solver_backend == SolverBackend::Scip {
            self.path.clone()
        } else {
            String::new()
        };
        solver::start_root_cuts(
            Arc::clone(&self.model),
            source_path,
            params,
            self.solver_tx.clone(),
        );
    }

    fn stepper_hidden(&self) -> (BTreeSet<usize>, BTreeSet<usize>) {
        let mut rows = BTreeSet::new();
        let mut columns = BTreeSet::new();
        if !self.stepper_open {
            return (rows, columns);
        }
        if let Some(items) = &self.reductions {
            for item in items.iter().take(self.step_index) {
                match item.reduction_type.as_str() {
                    "FixedCol"
                    | "FreeColSubstitution"
                    | "SlackColSubstitution"
                    | "DuplicateColumn"
                    | "ForcingColumn" => {
                        if item.col >= 0 {
                            columns.insert(item.col as usize);
                        }
                    }
                    "RedundantRow"
                    | "SingletonRow"
                    | "ForcingRow"
                    | "ForcingColumnRemovedRow"
                    | "DuplicateRow" => {
                        if item.row >= 0 {
                            rows.insert(item.row as usize);
                        }
                    }
                    "DoubletonEquation" => {
                        if item.row >= 0 {
                            rows.insert(item.row as usize);
                        }
                        if item.col >= 0 {
                            columns.insert(item.col as usize);
                        }
                    }
                    _ => {}
                }
            }
        }
        (rows, columns)
    }

    fn matrix_lp_values(&self) -> Option<&[f64]> {
        if self.model_source != ModelSource::Original || !self.solution_applied {
            return None;
        }
        self.solve_report.as_ref().map(|report| report.values.as_slice())
    }

    fn apply_solution(&mut self) {
        if !self
            .solve_report
            .as_ref()
            .is_some_and(|report| report.objective.is_finite())
            || self.solver_task != SolverTask::Idle
        {
            return;
        }
        self.set_model_source(ModelSource::Original);
        self.solution_applied = true;
        self.switch_view(View::Variables);
    }

    fn handle_filter_key(&mut self, code: KeyCode) {
        if self.editing_nonzero {
            match code {
                KeyCode::Enter | KeyCode::Esc => self.editing_nonzero = false,
                KeyCode::Backspace => {
                    self.nonzero_input.pop();
                    self.nonzero_filter = self.nonzero_input.parse().ok();
                    self.selected = 0;
                }
                KeyCode::Char(character) if character.is_ascii_digit() => {
                    self.nonzero_input.push(character);
                    self.nonzero_filter = self.nonzero_input.parse().ok();
                    self.selected = 0;
                }
                _ => {}
            }
            return;
        }

        match code {
            KeyCode::Esc | KeyCode::Char('f') | KeyCode::Enter => self.filter_open = false,
            KeyCode::Up | KeyCode::Char('k') => {
                self.filter_cursor = self.filter_cursor.saturating_sub(1)
            }
            KeyCode::Down | KeyCode::Char('j') => {
                self.filter_cursor = (self.filter_cursor + 1).min(ConstraintTag::ALL.len() - 1)
            }
            KeyCode::Char(' ') => {
                let tag = ConstraintTag::ALL[self.filter_cursor];
                if !self.active_constraint_types.remove(&tag) {
                    self.active_constraint_types.insert(tag);
                }
                self.selected = 0;
            }
            KeyCode::Char('o') => {
                self.constraint_types_all = !self.constraint_types_all;
                self.selected = 0;
            }
            KeyCode::Char('n') => {
                self.nonzero_input = self
                    .nonzero_filter
                    .map(|value| value.to_string())
                    .unwrap_or_default();
                self.editing_nonzero = true;
            }
            KeyCode::Char('[') => {
                self.nonzero_operator = self.nonzero_operator.previous();
                self.selected = 0;
            }
            KeyCode::Char(']') => {
                self.nonzero_operator = self.nonzero_operator.next();
                self.selected = 0;
            }
            KeyCode::Char('x') => {
                self.active_constraint_types.clear();
                self.constraint_types_all = false;
                self.active_variable_filters.clear();
                self.variable_filters_all = false;
                self.nonzero_filter = None;
                self.nonzero_operator = NonzeroOperator::LessOrEqual;
                self.nonzero_input.clear();
                self.active_component = None;
                self.search.clear();
                self.reset_filtered_navigation();
            }
            _ => {}
        }
    }

    fn handle_component_filter_key(&mut self, code: KeyCode) {
        let count = self.active_model().components.components.len();
        match code {
            KeyCode::Esc | KeyCode::Char('c') => self.component_filter_open = false,
            KeyCode::Up | KeyCode::Char('k') => {
                self.component_cursor = self.component_cursor.saturating_sub(1)
            }
            KeyCode::Down | KeyCode::Char('j') => {
                self.component_cursor = (self.component_cursor + 1).min(count.saturating_sub(1))
            }
            KeyCode::Enter if count > 0 => {
                self.active_component = Some(self.component_cursor);
                self.component_filter_open = false;
                self.reset_filtered_navigation();
            }
            KeyCode::Char('x') => {
                self.active_component = None;
                self.component_filter_open = false;
                self.reset_filtered_navigation();
            }
            _ => {}
        }
    }

    fn handle_params_key(&mut self, code: KeyCode) {
        if self.editing_param {
            match code {
                KeyCode::Enter | KeyCode::Esc => self.editing_param = false,
                KeyCode::Backspace => {
                    self.edit_param_line(|line| {
                        line.pop();
                    });
                }
                KeyCode::Char(character) => {
                    self.edit_param_line(|line| line.push(character));
                }
                _ => {}
            }
            return;
        }
        match code {
            KeyCode::Esc | KeyCode::Char('=') => self.params_open = false,
            KeyCode::Up | KeyCode::Char('k') => {
                self.params_cursor = self.params_cursor.saturating_sub(1)
            }
            KeyCode::Down | KeyCode::Char('j') => {
                let count = self.params_text.lines().count();
                self.params_cursor = (self.params_cursor + 1).min(count.saturating_sub(1).max(0));
            }
            KeyCode::Enter => {
                if self.params_text.lines().count() == 0 {
                    self.params_text.push_str("# e.g. time_limit = 10");
                }
                self.editing_param = true;
            }
            KeyCode::Char('a') => {
                let mut lines = self.params_text.lines().map(str::to_owned).collect::<Vec<_>>();
                lines.push(String::new());
                self.params_text = lines.join("\n");
                self.params_cursor = lines.len().saturating_sub(1);
            }
            KeyCode::Char('d') => {
                let mut lines = self.params_text.lines().map(str::to_owned).collect::<Vec<_>>();
                if !lines.is_empty() {
                    lines.remove(self.params_cursor.min(lines.len() - 1));
                }
                self.params_text = lines.join("\n");
                self.params_cursor = self.params_cursor.saturating_sub(1);
            }
            _ => {}
        }
    }

    fn edit_param_line<F>(&mut self, mutate: F)
    where
        F: FnOnce(&mut String),
    {
        let mut lines = self.params_text.lines().map(str::to_owned).collect::<Vec<_>>();
        if lines.is_empty() {
            lines.push(String::new());
        }
        let cursor = self.params_cursor.min(lines.len() - 1);
        mutate(&mut lines[cursor]);
        self.params_text = lines.join("\n");
    }

    fn handle_mouse(&mut self, mouse: MouseEvent) {
        match mouse.kind {
            MouseEventKind::Moved => self.mouse_position = Some((mouse.column, mouse.row)),
            MouseEventKind::Down(..) | MouseEventKind::Drag(..) => {
                self.mouse_position = Some((mouse.column, mouse.row))
            }
            _ => {}
        }
        if self.view != View::Matrix {
            self.mouse_position = None;
        }
    }

    fn reset_filtered_navigation(&mut self) {
        self.selected = 0;
        self.matrix_zoom = 1;
        self.matrix_row = 0;
        self.matrix_column = 0;
    }

    fn handle_key(&mut self, key: KeyEvent) -> bool {
        if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('c') {
            return true;
        }
        if self.component_filter_open {
            self.handle_component_filter_key(key.code);
            return false;
        }
        if self.filter_open {
            self.handle_filter_key(key.code);
            return false;
        }
        if self.params_open {
            self.handle_params_key(key.code);
            return false;
        }
        if self.editing_search {
            match key.code {
                KeyCode::Enter => self.editing_search = false,
                KeyCode::Esc => {
                    self.search.clear();
                    self.editing_search = false;
                    self.selected = 0;
                }
                KeyCode::Backspace => {
                    self.search.pop();
                    self.selected = 0;
                }
                KeyCode::Char(character) => {
                    self.search.push(character);
                    self.selected = 0;
                }
                _ => {}
            }
            return false;
        }

        match key.code {
            KeyCode::Char('q') => return true,
            KeyCode::Char('1')
                if self.view == View::Analysis =>
            {
                self.analysis_tab = 0;
                self.selected = 0;
            }
            KeyCode::Char('2')
                if self.view == View::Analysis =>
            {
                self.analysis_tab = 1;
                self.selected = 0;
            }
            KeyCode::Char('3')
                if self.view == View::Analysis =>
            {
                self.analysis_tab = 2;
                self.selected = 0;
            }
            KeyCode::Char('1') => self.switch_view(View::Overview),
            KeyCode::Char('2') => self.switch_view(View::Variables),
            KeyCode::Char('3') => self.switch_view(View::Constraints),
            KeyCode::Char('4') => self.switch_view(View::Matrix),
            KeyCode::Char('5') => self.switch_view(View::Presolve),
            KeyCode::Char('6') => self.switch_view(View::Solve),
            KeyCode::Char('7') => self.switch_view(View::Analysis),
            KeyCode::Char('=') if !self.params_open => {
                self.params_open = true;
                self.params_cursor = self.params_cursor.min(
                    self.params_text.lines().count().saturating_sub(1).max(0),
                );
            }
            KeyCode::Enter if self.view == View::Solve && self.solve_report.is_some() => {
                self.apply_solution()
            }
            KeyCode::Char('a') if self.presolve_report.is_some() => {
                self.set_model_source(ModelSource::Presolved);
                self.switch_view(View::Overview);
            }
            KeyCode::Char('u') => self.set_model_source(ModelSource::Original),
            KeyCode::Char('f') if self.view == View::Constraints => self.filter_open = true,
            KeyCode::Char('d') if self.view == View::Constraints => {
                self.constraint_detail_open = !self.constraint_detail_open;
            }
            KeyCode::Char('n') if self.view == View::Constraints => {
                self.filter_open = true;
                self.nonzero_input = self
                    .nonzero_filter
                    .map(|value| value.to_string())
                    .unwrap_or_default();
                self.editing_nonzero = true;
            }
            KeyCode::Char(' ') if self.view == View::Variables => {
                if let Some(&index) = self.filtered_variable_indices().get(self.selected)
                    && !self.active_variable_filters.remove(&index)
                {
                    self.active_variable_filters.insert(index);
                }
            }
            KeyCode::Char('o')
                if self.view == View::Variables && !self.active_variable_filters.is_empty() =>
            {
                self.variable_filters_all = !self.variable_filters_all;
            }
            KeyCode::Char('c')
                if matches!(
                    self.view,
                    View::Variables | View::Constraints | View::Matrix
                ) =>
            {
                self.component_cursor = self.active_component.unwrap_or(0);
                self.component_filter_open = true;
            }
            KeyCode::Char('x')
                if self.active_component.is_some() || !self.active_variable_filters.is_empty() =>
            {
                self.active_component = None;
                self.active_variable_filters.clear();
                self.variable_filters_all = false;
                self.reset_filtered_navigation();
            }
            KeyCode::Char('b') if self.solver_task == SolverTask::Idle => {
                self.solver_backend = self.solver_backend.next();
                if self.view == View::Presolve && self.presolve_error.is_some() {
                    self.presolve_error = None;
                }
            }
            KeyCode::Char('p') => self.start_presolve(),
            KeyCode::Char('l') if self.view != View::Matrix => {
                self.start_solve(SolveMode::Relaxation)
            }
            KeyCode::Char('m') => self.start_solve(SolveMode::Mip),
            KeyCode::Char('r') if self.view == View::Analysis => self.start_analysis(),
            KeyCode::Char('r') if self.view == View::Solve => self.extract_root_cuts(),
            KeyCode::Tab => self.switch_view(View::ALL[(self.view.index() + 1) % View::ALL.len()]),
            KeyCode::BackTab => self.switch_view(
                View::ALL[(self.view.index() + View::ALL.len() - 1) % View::ALL.len()],
            ),
            KeyCode::Char('/') if matches!(self.view, View::Variables | View::Constraints) => {
                self.editing_search = true;
            }
            KeyCode::Esc => {
                self.search.clear();
                self.selected = 0;
            }
            _ if self.view == View::Matrix => self.handle_matrix_key(key.code),
            KeyCode::Up | KeyCode::Char('k')
                if matches!(self.view, View::Presolve | View::Solve) =>
            {
                self.log_scroll = self.log_scroll.saturating_add(1)
            }
            KeyCode::Down | KeyCode::Char('j')
                if matches!(self.view, View::Presolve | View::Solve) =>
            {
                self.log_scroll = self.log_scroll.saturating_sub(1)
            }
            KeyCode::PageUp if matches!(self.view, View::Presolve | View::Solve) => {
                self.log_scroll = self.log_scroll.saturating_add(10)
            }
            KeyCode::PageDown if matches!(self.view, View::Presolve | View::Solve) => {
                self.log_scroll = self.log_scroll.saturating_sub(10)
            }
            KeyCode::Up | KeyCode::Char('k') => self.selected = self.selected.saturating_sub(1),
            KeyCode::Down | KeyCode::Char('j') => self.selected = self.selected.saturating_add(1),
            KeyCode::PageUp => self.selected = self.selected.saturating_sub(10),
            KeyCode::PageDown => self.selected = self.selected.saturating_add(10),
            KeyCode::Home => self.selected = 0,
            KeyCode::End => self.selected = usize::MAX,
            _ => {}
        }
        false
    }

    fn switch_view(&mut self, view: View) {
        self.view = view;
        self.selected = 0;
        self.search.clear();
        self.editing_search = false;
    }

    fn handle_matrix_key(&mut self, code: KeyCode) {
        let model = self.active_model();
        let (row_count, column_count) = self
            .active_component
            .and_then(|index| model.components.components.get(index))
            .map(|component| (component.rows.len(), component.columns.len()))
            .unwrap_or((model.constraints.len(), model.variables.len()));
        let row_step = (row_count / (self.matrix_zoom * 12)).max(1);
        let column_step = (column_count / (self.matrix_zoom * 12)).max(1);
        match code {
            KeyCode::Left | KeyCode::Char('h') => {
                self.matrix_column = self.matrix_column.saturating_sub(column_step)
            }
            KeyCode::Right | KeyCode::Char('l') => {
                self.matrix_column = self.matrix_column.saturating_add(column_step)
            }
            KeyCode::Up | KeyCode::Char('k') => {
                self.matrix_row = self.matrix_row.saturating_sub(row_step)
            }
            KeyCode::Down | KeyCode::Char('j') => {
                self.matrix_row = self.matrix_row.saturating_add(row_step)
            }
            KeyCode::Char('+') => self.matrix_zoom = (self.matrix_zoom * 2).min(64),
            KeyCode::Char('-') => self.matrix_zoom = (self.matrix_zoom / 2).max(1),
            KeyCode::Char('0') => {
                self.matrix_zoom = 1;
                self.matrix_row = 0;
                self.matrix_column = 0;
                if self.stepper_open {
                    self.step_index = 0;
                }
            }
            KeyCode::Char('s') if !self.stepper_open => {
                if self.reductions.is_none() && self.reductions_error.is_none() {
                    self.load_reductions();
                } else {
                    self.stepper_open = true;
                    self.step_index = 0;
                }
            }
            KeyCode::Char('s') if self.stepper_open => self.stepper_open = false,
            KeyCode::Char('[') if self.stepper_open => {
                self.step_index = self.step_index.saturating_sub(1);
            }
            KeyCode::Char(']') if self.stepper_open => {
                let total = self.reductions.as_ref().map_or(0, Vec::len);
                self.step_index = (self.step_index + 1).min(total);
            }
            _ => {}
        }
        let visible_rows = row_count.div_ceil(self.matrix_zoom);
        let visible_columns = column_count.div_ceil(self.matrix_zoom);
        self.matrix_row = self.matrix_row.min(row_count.saturating_sub(visible_rows));
        self.matrix_column = self
            .matrix_column
            .min(column_count.saturating_sub(visible_columns));
    }

    fn filtered_variable_indices(&self) -> Vec<usize> {
        let model = self.active_model();
        let needle = self.search.to_lowercase();
        let candidates = self
            .active_component
            .and_then(|index| model.components.components.get(index))
            .map(|component| component.columns.clone())
            .unwrap_or_else(|| (0..model.variables.len()).collect());
        let mut indices = candidates
            .into_iter()
            .filter(|&index| {
                let variable = &model.variables[index];
                needle.is_empty()
                    || format!("{} {}", variable.name, variable.kind.label())
                        .to_lowercase()
                        .contains(&needle)
            })
            .collect::<Vec<_>>();
        if self.solution_applied
            && let Some(report) = &self.solve_report
        {
            indices.sort_by_key(|&index| {
                (
                    !report
                        .values
                        .get(index)
                        .is_some_and(|value| is_fractional(&model.variables[index], *value)),
                    index,
                )
            });
        }
        indices
    }

    fn filtered_constraint_indices(&self) -> Vec<usize> {
        let needle = self.search.to_lowercase();
        let model = self.active_model();
        let candidates = self
            .active_component
            .and_then(|index| model.components.components.get(index))
            .map(|component| component.rows.clone())
            .unwrap_or_else(|| (0..model.constraints.len()).collect());
        candidates
            .into_iter()
            .filter(|&index| {
                let row = &model.constraints[index];
                needle.is_empty() || row.name.to_lowercase().contains(&needle)
            })
            .filter(|&index| {
                let row = &model.constraints[index];
                if self.active_constraint_types.is_empty() {
                    true
                } else if self.constraint_types_all {
                    self.active_constraint_types
                        .iter()
                        .all(|tag| row.tags.contains(tag))
                } else {
                    row.tags
                        .iter()
                        .any(|tag| self.active_constraint_types.contains(tag))
                }
            })
            .filter(|&index| {
                let row = &model.constraints[index];
                self.nonzero_filter
                    .is_none_or(|expected| self.nonzero_operator.matches(row.terms.len(), expected))
            })
            .filter(|&index| {
                if self.active_variable_filters.is_empty() {
                    return true;
                }
                let row = &model.constraints[index];
                if self.variable_filters_all {
                    self.active_variable_filters
                        .iter()
                        .all(|column| row.terms.iter().any(|term| term.column == *column))
                } else {
                    row.terms
                        .iter()
                        .any(|term| self.active_variable_filters.contains(&term.column))
                }
            })
            .collect()
    }
}

fn classify_constraint(row: &ConstraintRow, variables: &[Variable]) -> Vec<ConstraintTag> {
    const EPSILON: f64 = 1e-10;
    const BIG_M_RATIO: f64 = 100.0;

    if row.terms.is_empty() {
        return vec![ConstraintTag::Empty];
    }

    let kinds = row
        .terms
        .iter()
        .map(|term| variables[term.column].kind)
        .collect::<Vec<_>>();
    let has_continuous = kinds.contains(&VariableKind::Continuous);
    let has_binary = kinds.contains(&VariableKind::Binary);
    let has_integer = kinds.contains(&VariableKind::Integer);
    let all_continuous = kinds.iter().all(|kind| *kind == VariableKind::Continuous);
    let all_binary = kinds.iter().all(|kind| *kind == VariableKind::Binary);
    let mut tags = Vec::new();

    if all_continuous {
        tags.push(ConstraintTag::Continuous);
    } else if all_binary {
        tags.push(ConstraintTag::PureBinary);
    } else if has_integer {
        tags.push(ConstraintTag::MixedInteger);
    } else if has_binary && has_continuous {
        tags.push(ConstraintTag::MixedBinary);
    }

    if has_binary && (has_continuous || has_integer) {
        let mut max_binary = 0.0_f64;
        let mut max_other = 0.0_f64;
        for term in &row.terms {
            let magnitude = term.coefficient.abs();
            if variables[term.column].kind == VariableKind::Binary {
                max_binary = max_binary.max(magnitude);
            } else {
                max_other = max_other.max(magnitude);
            }
        }
        if max_other > 0.0 && max_binary >= BIG_M_RATIO * max_other {
            tags.push(ConstraintTag::BigM);
        }
    }

    let lower_unbounded = !row.lower.is_finite() || row.lower <= -1e20;
    let upper_unbounded = !row.upper.is_finite() || row.upper >= 1e20;
    let equality = !lower_unbounded && !upper_unbounded && (row.lower - row.upper).abs() < EPSILON;
    let less_or_equal = lower_unbounded && !upper_unbounded;
    let greater_or_equal = !lower_unbounded && upper_unbounded;
    let all_one = row
        .terms
        .iter()
        .all(|term| (term.coefficient - 1.0).abs() < EPSILON);
    let all_unit = row
        .terms
        .iter()
        .all(|term| (term.coefficient.abs() - 1.0).abs() < EPSILON);
    let rhs_one = if equality {
        (row.lower - 1.0).abs() < EPSILON
    } else if less_or_equal {
        (row.upper - 1.0).abs() < EPSILON
    } else if greater_or_equal {
        (row.lower - 1.0).abs() < EPSILON
    } else {
        false
    };
    let rhs_zero = if equality {
        row.lower.abs() < EPSILON
    } else if less_or_equal {
        row.upper.abs() < EPSILON
    } else if greater_or_equal {
        row.lower.abs() < EPSILON
    } else {
        false
    };
    let precedence = row.terms.len() == 2
        && all_unit
        && (less_or_equal || greater_or_equal)
        && rhs_zero
        && row.terms[0].coefficient * row.terms[1].coefficient < 0.0
        && kinds[0] == kinds[1];

    if row.terms.len() == 1 {
        tags.push(ConstraintTag::Bound);
        return tags;
    }
    if precedence {
        tags.push(ConstraintTag::Precedence);
        return tags;
    }
    if row.terms.len() == 2 && has_binary && (has_continuous || has_integer) {
        tags.push(ConstraintTag::VariableBound);
        return tags;
    }

    if all_binary && all_one && rhs_one {
        if equality {
            tags.push(ConstraintTag::SetPartitioning);
        } else if less_or_equal {
            tags.push(ConstraintTag::SetPacking);
        } else if greater_or_equal {
            tags.push(ConstraintTag::SetCovering);
        }
    } else if all_binary && all_unit {
        tags.push(ConstraintTag::Cardinality);
    } else if all_binary && (less_or_equal || greater_or_equal) {
        tags.push(ConstraintTag::Knapsack);
    } else if all_binary && equality {
        tags.push(ConstraintTag::BinaryKnapsackEquation);
    } else if !has_continuous && has_integer && (less_or_equal || greater_or_equal) {
        tags.push(ConstraintTag::IntegerKnapsack);
    }

    if equality && !tags.contains(&ConstraintTag::SetPartitioning) {
        tags.push(ConstraintTag::Equality);
    }
    tags
}

fn centered_rect(area: Rect, preferred_width: u16, preferred_height: u16) -> Rect {
    let width = preferred_width.min(area.width);
    let height = preferred_height.min(area.height);
    Rect::new(
        area.x + area.width.saturating_sub(width) / 2,
        area.y + area.height.saturating_sub(height) / 2,
        width,
        height,
    )
}

fn comparison_line(
    label: &'static str,
    original: usize,
    presolved: usize,
    removed: usize,
) -> Line<'static> {
    let percent = if original == 0 {
        0.0
    } else {
        removed as f64 / original as f64 * 100.0
    };
    Line::from(vec![
        muted(label),
        Span::raw(format!(
            "  {:>10}  →  {:>10}   removed {:>10} ({percent:.1}%)",
            format_count(original),
            format_count(presolved),
            format_count(removed)
        )),
    ])
}

fn format_duration(duration: Duration) -> String {
    if duration.as_secs() >= 60 {
        format!(
            "{}m {:.1}s",
            duration.as_secs() / 60,
            duration.as_secs_f64() % 60.0
        )
    } else if duration.as_secs() > 0 {
        format!("{:.2}s", duration.as_secs_f64())
    } else {
        format!("{}ms", duration.as_millis())
    }
}

fn matrix_braille(
    model: &Model,
    width: usize,
    height: usize,
    zoom: usize,
    row_offset: usize,
    column_offset: usize,
    selected_component: Option<usize>,
    hidden_rows: &BTreeSet<usize>,
    hidden_columns: &BTreeSet<usize>,
    lp_values: Option<&[f64]>,
) -> Text<'static> {
    let (row_base, row_count, column_base, column_count) = selected_component
        .and_then(|selected| {
            model.components.components.get(selected).map(|component| {
                let row_base = model.components.components[..selected]
                    .iter()
                    .map(|item| item.rows.len())
                    .sum();
                let column_base = model.components.components[..selected]
                    .iter()
                    .map(|item| item.columns.len())
                    .sum();
                (
                    row_base,
                    component.rows.len(),
                    column_base,
                    component.columns.len(),
                )
            })
        })
        .unwrap_or((
            0,
            model.constraints.len(),
            0,
            model.components.column_order.len(),
        ));
    if row_count == 0 || column_count == 0 {
        return Text::from("Empty matrix");
    }
    let pixel_width = width * 2;
    let pixel_height = height * 4;
    let visible_rows = row_count.div_ceil(zoom).max(1);
    let visible_columns = column_count.div_ceil(zoom).max(1);
    let row_end = (row_offset + visible_rows).min(row_count);
    let column_end = (column_offset + visible_columns).min(column_count);
    let mut cells = vec![0u8; width * height];
    let mut cell_components = vec![None; width * height];
    let mut cell_fractional = vec![false; width * height];

    for display_row in row_offset..row_end {
        let row_index = model.components.row_order[row_base + display_row];
        if hidden_rows.contains(&row_index) {
            continue;
        }
        let component = model.components.row_component[row_index];
        for term in &model.constraints[row_index].terms {
            if hidden_columns.contains(&term.column) {
                continue;
            }
            let global_column = model.components.column_position[term.column];
            if global_column < column_base {
                continue;
            }
            let display_column = global_column - column_base;
            if display_column < column_offset || display_column >= column_end {
                continue;
            }
            let x = (display_column - column_offset) * pixel_width / visible_columns;
            let y = (display_row - row_offset) * pixel_height / visible_rows;
            let cell_x = (x / 2).min(width - 1);
            let cell_y = (y / 4).min(height - 1);
            let dot_x = x % 2;
            let dot_y = y % 4;
            let bit = braille_bit(dot_x, dot_y);
            let cell = cell_y * width + cell_x;
            cells[cell] |= bit;
            cell_components[cell].get_or_insert(component);
            if let Some(values) = lp_values
                && let Some(value) = values.get(term.column)
                && is_fractional(&model.variables[term.column], *value)
            {
                cell_fractional[cell] = true;
            }
        }
    }

    let mut lines = Vec::with_capacity(height);
    for y in 0..height {
        let mut spans = Vec::with_capacity(width);
        for x in 0..width {
            let index = y * width + x;
            let character = char::from_u32(0x2800 + cells[index] as u32).unwrap_or(' ');
            let color = if cell_fractional[index] {
                ORANGE
            } else {
                cell_components[index].map_or(MUTED, component_color)
            };
            spans.push(Span::styled(
                character.to_string(),
                Style::default().fg(color),
            ));
        }
        lines.push(Line::from(spans));
    }
    Text::from(lines)
}

/// Small overview of the whole matrix with a viewport rectangle. One terminal
/// cell is one mini-pixel; any nonzero in a mini-pixel lights it up.
fn matrix_minimap(
    model: &Model,
    width: usize,
    height: usize,
    viewport: MatrixViewport,
    hidden_rows: &BTreeSet<usize>,
    hidden_columns: &BTreeSet<usize>,
) -> Text<'static> {
    let (row_base, row_count, column_base, column_count) = viewport
        .component
        .and_then(|selected| {
            model.components.components.get(selected).map(|component| {
                let row_base = model.components.components[..selected]
                    .iter()
                    .map(|item| item.rows.len())
                    .sum();
                let column_base = model.components.components[..selected]
                    .iter()
                    .map(|item| item.columns.len())
                    .sum();
                (
                    row_base,
                    component.rows.len(),
                    column_base,
                    component.columns.len(),
                )
            })
        })
        .unwrap_or((
            0,
            model.constraints.len(),
            0,
            model.components.column_order.len(),
        ));
    if row_count == 0 || column_count == 0 || width == 0 || height == 0 {
        return Text::from(String::new());
    }
    let mut grid = vec![false; width * height];
    for display_row in 0..row_count {
        let row_index = model.components.row_order[row_base + display_row];
        if hidden_rows.contains(&row_index) {
            continue;
        }
        let mini_y = display_row * height / row_count;
        for term in &model.constraints[row_index].terms {
            if hidden_columns.contains(&term.column) {
                continue;
            }
            let global_column = model.components.column_position[term.column];
            if global_column < column_base {
                continue;
            }
            let display_column = global_column - column_base;
            let mini_x = display_column * width / column_count;
            if mini_x < width && mini_y < height {
                grid[mini_y * width + mini_x] = true;
            }
        }
    }

    let visible_rows = row_count.div_ceil(viewport.zoom).max(1);
    let visible_columns = column_count.div_ceil(viewport.zoom).max(1);
    let vrow0 = viewport.row * height / row_count;
    let vrow1 = (viewport.row + visible_rows) * height / row_count;
    let vcol0 = viewport.column * width / column_count;
    let vcol1 = (viewport.column + visible_columns) * width / column_count;

    let mut lines = Vec::with_capacity(height);
    for y in 0..height {
        let mut spans = Vec::with_capacity(width);
        for x in 0..width {
            let on_border = (x == vcol0 || x == vcol1.saturating_sub(1))
                && y >= vrow0
                && y < vrow1
                || (y == vrow0 || y == vrow1.saturating_sub(1))
                    && x >= vcol0
                    && x < vcol1;
            if on_border {
                spans.push(Span::styled(
                    "+",
                    Style::default().fg(ACCENT).add_modifier(Modifier::BOLD),
                ));
            } else if grid[y * width + x] {
                spans.push(Span::styled(
                    "▪",
                    Style::default().fg(component_color(y * 2 + x / 8)),
                ));
            } else {
                spans.push(Span::raw(" "));
            }
        }
        lines.push(Line::from(spans));
    }
    Text::from(lines)
}

/// Map a terminal cell inside the matrix braille panel back to the original
/// (row, column) and coefficient under the cursor. `cell_x`/`cell_y` are
/// braille-cell coordinates relative to the panel's inner area.
fn matrix_cell_at(
    model: &Model,
    cell_x: usize,
    cell_y: usize,
    panel_width: usize,
    panel_height: usize,
    viewport: MatrixViewport,
    hidden_rows: &BTreeSet<usize>,
    hidden_columns: &BTreeSet<usize>,
) -> Option<(usize, usize, f64)> {
    let (row_base, row_count, column_base, column_count) = viewport
        .component
        .and_then(|selected| {
            model.components.components.get(selected).map(|component| {
                let row_base = model.components.components[..selected]
                    .iter()
                    .map(|item| item.rows.len())
                    .sum();
                let column_base = model.components.components[..selected]
                    .iter()
                    .map(|item| item.columns.len())
                    .sum();
                (
                    row_base,
                    component.rows.len(),
                    column_base,
                    component.columns.len(),
                )
            })
        })
        .unwrap_or((
            0,
            model.constraints.len(),
            0,
            model.components.column_order.len(),
        ));
    if row_count == 0 || column_count == 0 || panel_width == 0 || panel_height == 0 {
        return None;
    }
    let pixel_width = panel_width * 2;
    let pixel_height = panel_height * 4;
    let visible_rows = row_count.div_ceil(viewport.zoom).max(1);
    let visible_columns = column_count.div_ceil(viewport.zoom).max(1);
    let display_row = (cell_y * 4 + 1) * visible_rows / pixel_height;
    let display_col = (cell_x * 2 + 1) * visible_columns / pixel_width;
    if display_row >= row_count || display_col >= column_count {
        return None;
    }
    let row_index = model.components.row_order[row_base + display_row];
    if hidden_rows.contains(&row_index) {
        return None;
    }
    let mut column_index = None;
    for (original, &position) in model.components.column_position.iter().enumerate() {
        if position == column_base + display_col {
            if !hidden_columns.contains(&original) {
                column_index = Some(original);
            }
            break;
        }
    }
    let column_index = column_index?;
    let row = &model.constraints[row_index];
    for term in &row.terms {
        if term.column == column_index {
            return Some((row_index, column_index, term.coefficient));
        }
    }
    None
}

fn component_color(index: usize) -> Color {
    const COLORS: [Color; 8] = [
        ACCENT,
        ORANGE,
        GREEN,
        Color::Magenta,
        Color::Cyan,
        Color::Yellow,
        Color::LightRed,
        Color::LightBlue,
    ];
    COLORS[index % COLORS.len()]
}

fn component_cell(index: usize) -> Cell<'static> {
    Cell::from(format!("● {}", index + 1)).style(Style::default().fg(component_color(index)))
}

fn component_row_style(row: Row<'static>, selected: bool) -> Row<'static> {
    if selected {
        row.style(Style::default().bg(Color::Rgb(38, 49, 60)))
    } else {
        row
    }
}

fn braille_bit(x: usize, y: usize) -> u8 {
    match (x, y) {
        (0, 0) => 1 << 0,
        (0, 1) => 1 << 1,
        (0, 2) => 1 << 2,
        (1, 0) => 1 << 3,
        (1, 1) => 1 << 4,
        (1, 2) => 1 << 5,
        (0, 3) => 1 << 6,
        (1, 3) => 1 << 7,
        _ => 0,
    }
}

fn constraint_detail(model: &Model, index: usize) -> Text<'static> {
    let row = &model.constraints[index];
    let mut type_spans = vec![Span::raw("Types: ")];
    if row.tags.is_empty() {
        type_spans.push(Span::styled("unclassified", Style::default().fg(MUTED)));
    } else {
        for (position, tag) in row.tags.iter().enumerate() {
            if position > 0 {
                type_spans.push(Span::raw(" · "));
            }
            type_spans.push(Span::styled(
                tag.label(),
                Style::default()
                    .fg(tag.color())
                    .add_modifier(Modifier::BOLD),
            ));
        }
    }
    Text::from(vec![
        Line::from(format!(
            "{}  [row #{index}, {} nonzeros]",
            row.name,
            row.terms.len()
        )),
        Line::from(type_spans),
        Line::from(""),
        constraint_expression_line(model, row, 80),
    ])
}

fn constraint_expression_line(
    model: &Model,
    row: &ConstraintRow,
    term_limit: usize,
) -> Line<'static> {
    let equality =
        row.lower.is_finite() && row.upper.is_finite() && (row.lower - row.upper).abs() <= 1e-10;
    let mut spans = Vec::new();
    if row.lower.is_finite() && row.upper.is_finite() && !equality {
        spans.push(Span::styled(
            format!("{} ≤ ", format_number(row.lower)),
            Style::default().fg(MUTED),
        ));
    }
    if row.terms.is_empty() {
        spans.push(Span::raw("0"));
    }
    for (position, term) in row.terms.iter().take(term_limit).enumerate() {
        let magnitude = term.coefficient.abs();
        let operator = if position == 0 {
            if term.coefficient < 0.0 { "−" } else { "" }
        } else if term.coefficient < 0.0 {
            " − "
        } else {
            " + "
        };
        spans.push(Span::styled(operator, Style::default().fg(MUTED)));
        if (magnitude - 1.0).abs() > f64::EPSILON {
            spans.push(Span::raw(format!("{}·", format_number(magnitude))));
        }
        let variable = &model.variables[term.column];
        spans.push(Span::styled(
            variable.name.clone(),
            Style::default().fg(variable.kind.color()),
        ));
    }
    if row.terms.len() > term_limit {
        spans.push(Span::styled(
            format!(" … + {} more", row.terms.len() - term_limit),
            Style::default().fg(MUTED),
        ));
    }
    if equality {
        spans.push(Span::styled(
            format!(" = {}", format_number(row.upper)),
            Style::default().fg(MUTED),
        ));
    } else if row.upper.is_finite() {
        spans.push(Span::styled(
            format!(" ≤ {}", format_number(row.upper)),
            Style::default().fg(MUTED),
        ));
    } else if row.lower.is_finite() {
        spans.push(Span::styled(
            format!(" ≥ {}", format_number(row.lower)),
            Style::default().fg(MUTED),
        ));
    } else {
        spans.push(Span::styled(" free", Style::default().fg(MUTED)));
    }
    Line::from(spans)
}

fn constraint_types_line(row: &ConstraintRow) -> Line<'static> {
    if row.tags.is_empty() {
        return Line::from(Span::styled("unclassified", Style::default().fg(MUTED)));
    }
    let mut spans = Vec::new();
    for (position, tag) in row.tags.iter().enumerate() {
        if position > 0 {
            spans.push(Span::raw(", "));
        }
        spans.push(Span::styled(tag.label(), Style::default().fg(tag.color())));
    }
    Line::from(spans)
}

fn clamp_selection(selected: usize, count: usize) -> usize {
    selected.min(count.saturating_sub(1))
}

fn list_title(prefix: &str, shown: usize, total: usize, search: &str, editing: bool) -> String {
    if search.is_empty() && !editing {
        format!("{prefix}· {} ", format_count(total))
    } else {
        format!(
            "{prefix}· {shown}/{total} · /{search}{} ",
            if editing { "█" } else { "" }
        )
    }
}

fn range_row<I>(label: &str, values: I) -> Row<'static>
where
    I: Iterator<Item = f64>,
{
    let finite = values.filter(|value| value.is_finite()).collect::<Vec<_>>();
    let minimum = finite.iter().copied().reduce(f64::min);
    let maximum = finite.iter().copied().reduce(f64::max);
    Row::new([
        Cell::from(label.to_owned()),
        Cell::from(minimum.map(format_number).unwrap_or_else(|| "—".to_owned())),
        Cell::from(maximum.map(format_number).unwrap_or_else(|| "—".to_owned())),
    ])
}

fn format_bound(value: f64) -> String {
    if value.is_infinite() || value.abs() >= 1e20 {
        if value.is_sign_negative() {
            "−∞".to_owned()
        } else {
            "+∞".to_owned()
        }
    } else {
        format_number(value)
    }
}

fn is_fractional(variable: &Variable, value: f64) -> bool {
    variable.kind != VariableKind::Continuous
        && value.is_finite()
        && (value - value.round()).abs() > 1e-6
}

fn format_number(value: f64) -> String {
    if value == 0.0 {
        "0".to_owned()
    } else if value.abs() >= 1e6 || value.abs() < 1e-4 {
        format!("{value:.4e}")
    } else if value.fract() == 0.0 {
        format!("{value:.0}")
    } else {
        format!("{value:.5}").trim_end_matches('0').to_owned()
    }
}

fn format_count(value: usize) -> String {
    let digits = value.to_string();
    let mut output = String::with_capacity(digits.len() + digits.len() / 3);
    for (index, character) in digits.chars().enumerate() {
        if index > 0 && (digits.len() - index).is_multiple_of(3) {
            output.push(',');
        }
        output.push(character);
    }
    output
}

fn muted(value: &'static str) -> Span<'static> {
    Span::styled(value, Style::default().fg(MUTED))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn disconnected_model() -> Model {
        let variables = (0..3)
            .map(|index| Variable {
                name: format!("x{index}"),
                kind: VariableKind::Binary,
                lower: 0.0,
                upper: 1.0,
                objective: 0.0,
            })
            .collect::<Vec<_>>();
        let constraints = (0..2)
            .map(|index| ConstraintRow {
                name: format!("r{index}"),
                lower: 0.0,
                upper: 1.0,
                terms: vec![Term {
                    column: index,
                    coefficient: 1.0,
                }],
                tags: vec![ConstraintTag::Continuous],
            })
            .collect::<Vec<_>>();
        let components = ComponentAnalysis::analyze(&constraints, &variables);
        Model {
            name: "disconnected".to_owned(),
            minimize: true,
            objective_offset: 0.0,
            variables,
            constraints,
            nonzeros: 2,
            components,
        }
    }

    #[test]
    fn braille_dot_layout_matches_unicode() {
        assert_eq!(braille_bit(0, 0), 1);
        assert_eq!(braille_bit(1, 3), 128);
    }

    #[test]
    fn formats_counts_with_separators() {
        assert_eq!(format_count(12), "12");
        assert_eq!(format_count(1_234_567), "1,234,567");
    }

    #[test]
    fn clamps_empty_and_oversized_selections() {
        assert_eq!(clamp_selection(12, 0), 0);
        assert_eq!(clamp_selection(12, 5), 4);
    }

    #[test]
    fn component_filter_limits_variables_and_constraints() {
        let mut app = App::new(disconnected_model(), "test.mps".to_owned());
        app.active_component = Some(1);
        assert_eq!(app.filtered_variable_indices(), vec![1]);
        assert_eq!(app.filtered_constraint_indices(), vec![1]);

        app.search = "not-present".to_owned();
        assert!(app.filtered_variable_indices().is_empty());
        assert!(app.filtered_constraint_indices().is_empty());
    }

    #[test]
    fn variable_filters_support_any_and_all_matching() {
        let mut model = disconnected_model();
        model.constraints[1].terms.push(Term {
            column: 0,
            coefficient: 1.0,
        });
        let mut app = App::new(model, "test.mps".to_owned());
        app.active_variable_filters.extend([0, 1]);

        assert_eq!(app.filtered_constraint_indices(), vec![0, 1]);
        app.variable_filters_all = true;
        assert_eq!(app.filtered_constraint_indices(), vec![1]);
    }

    #[test]
    fn selected_row_types_use_constraint_tag_colors() {
        let model = disconnected_model();
        let detail = constraint_detail(&model, 0);
        let type_line = &detail.lines[1];

        assert_eq!(
            type_line.spans[1].style.fg,
            Some(ConstraintTag::Continuous.color())
        );
    }

    #[test]
    fn constraint_list_expression_includes_coefficients_and_relation() {
        let mut model = disconnected_model();
        model.constraints[0].terms[0].coefficient = 2.5;
        model.constraints[0].lower = 1.0;
        model.constraints[0].upper = 1.0;
        let line = constraint_expression_line(&model, &model.constraints[0], 16);
        let rendered = line
            .spans
            .iter()
            .map(|span| span.content.as_ref())
            .collect::<String>();

        assert_eq!(rendered, "2.5·x0 = 1");
    }

    #[test]
    fn applied_solution_sorts_fractional_discrete_variables_first() {
        let mut app = App::new(disconnected_model(), "test.mps".to_owned());
        app.solve_report = Some(SolveReport {
            backend: SolverBackend::Highs,
            mode: SolveMode::Relaxation,
            status: "Optimal".to_owned(),
            objective: 1.5,
            iterations: 1,
            values: vec![1.0, 0.5, 0.0],
            elapsed: Duration::from_millis(1),
        });
        app.solution_applied = true;

        assert_eq!(app.filtered_variable_indices(), vec![1, 0, 2]);
        assert!(is_fractional(&app.model.variables[1], 0.5));
        assert!(!is_fractional(&app.model.variables[0], 1.0));
    }

    #[test]
    fn classifies_set_packing_row() {
        let variables = (0..3)
            .map(|index| Variable {
                name: format!("x{index}"),
                kind: VariableKind::Binary,
                lower: 0.0,
                upper: 1.0,
                objective: 0.0,
            })
            .collect::<Vec<_>>();
        let row = ConstraintRow {
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
        };

        let tags = classify_constraint(&row, &variables);
        assert!(tags.contains(&ConstraintTag::PureBinary));
        assert!(tags.contains(&ConstraintTag::SetPacking));
    }

    #[test]
    fn classifies_big_m_row() {
        let variables = vec![
            Variable {
                name: "active".to_owned(),
                kind: VariableKind::Binary,
                lower: 0.0,
                upper: 1.0,
                objective: 0.0,
            },
            Variable {
                name: "flow".to_owned(),
                kind: VariableKind::Continuous,
                lower: 0.0,
                upper: f64::INFINITY,
                objective: 0.0,
            },
        ];
        let row = ConstraintRow {
            name: "activation".to_owned(),
            lower: f64::NEG_INFINITY,
            upper: 0.0,
            terms: vec![
                Term {
                    column: 0,
                    coefficient: -1_000.0,
                },
                Term {
                    column: 1,
                    coefficient: 1.0,
                },
            ],
            tags: Vec::new(),
        };

        let tags = classify_constraint(&row, &variables);
        assert!(tags.contains(&ConstraintTag::MixedBinary));
        assert!(tags.contains(&ConstraintTag::BigM));
        assert!(tags.contains(&ConstraintTag::VariableBound));
    }

    #[test]
    fn matrix_cell_at_maps_braille_to_matrix_coordinates() {
        let model = disconnected_model();
        let viewport = MatrixViewport {
            zoom: 1,
            row: 0,
            column: 0,
            component: None,
        };
        // Cell (0,0) with a 20x8 panel → display row 0, col 0 → original row 0,
        // col 0, coefficient 1.0.
        let hit = matrix_cell_at(
            &model,
            0,
            0,
            20,
            8,
            viewport,
            &BTreeSet::new(),
            &BTreeSet::new(),
        );
        assert_eq!(hit, Some((0, 0, 1.0)));
        // Cell (7,4) maps to display col 1, row 1 → original row 1, col 1.
        let hit = matrix_cell_at(
            &model,
            7,
            4,
            20,
            8,
            viewport,
            &BTreeSet::new(),
            &BTreeSet::new(),
        );
        assert_eq!(hit, Some((1, 1, 1.0)));
    }

    #[test]
    fn matrix_cell_at_respects_hidden_rows() {
        let model = disconnected_model();
        let viewport = MatrixViewport {
            zoom: 1,
            row: 0,
            column: 0,
            component: None,
        };
        let hidden = BTreeSet::from([0]);
        // Cell (0,0) points at original row 0, which is hidden → no tooltip.
        let hit = matrix_cell_at(
            &model,
            0,
            0,
            20,
            8,
            viewport,
            &hidden,
            &BTreeSet::new(),
        );
        assert_eq!(hit, None);
        // Cell (7,4) points at original row 1 (visible) → tooltip works.
        let hit = matrix_cell_at(
            &model,
            7,
            4,
            20,
            8,
            viewport,
            &hidden,
            &BTreeSet::new(),
        );
        assert_eq!(hit, Some((1, 1, 1.0)));
    }

    #[test]
    fn matrix_minimap_renders_viewport_and_dots() {
        let model = disconnected_model();
        let viewport = MatrixViewport {
            zoom: 1,
            row: 0,
            column: 0,
            component: None,
        };
        let text = matrix_minimap(
            &model,
            10,
            4,
            viewport,
            &BTreeSet::new(),
            &BTreeSet::new(),
        );
        // Renders width×height characters (plus no blank lines).
        assert_eq!(text.lines.len(), 4);
        assert!(text.lines.iter().all(|line| line.width() == 10));
    }

    #[test]
    fn matrix_braille_renders_without_panicking_when_hidden() {
        let model = disconnected_model();
        let hidden = BTreeSet::from([0]);
        let text = matrix_braille(
            &model,
            20,
            8,
            1,
            0,
            0,
            None,
            &hidden,
            &BTreeSet::new(),
            None,
        );
        assert_eq!(text.lines.len(), 8);
    }
}

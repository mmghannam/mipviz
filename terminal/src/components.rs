use std::collections::HashMap;

use crate::{ConstraintRow, Variable, VariableKind};

pub struct Component {
    pub rows: Vec<usize>,
    pub columns: Vec<usize>,
    pub nonzeros: usize,
    pub continuous: usize,
    pub integer: usize,
    pub binary: usize,
}

pub struct ComponentAnalysis {
    pub components: Vec<Component>,
    pub row_order: Vec<usize>,
    pub column_order: Vec<usize>,
    pub column_position: Vec<usize>,
    pub row_component: Vec<usize>,
}

impl ComponentAnalysis {
    pub fn analyze(constraints: &[ConstraintRow], variables: &[Variable]) -> Self {
        let row_count = constraints.len();
        let column_count = variables.len();
        let mut union_find = UnionFind::new(row_count + column_count);
        for (row_index, row) in constraints.iter().enumerate() {
            for term in &row.terms {
                union_find.union(row_index, row_count + term.column);
            }
        }

        let mut grouped = HashMap::<usize, (Vec<usize>, Vec<usize>)>::new();
        for row in 0..row_count {
            grouped.entry(union_find.find(row)).or_default().0.push(row);
        }
        for column in 0..column_count {
            grouped
                .entry(union_find.find(row_count + column))
                .or_default()
                .1
                .push(column);
        }

        let mut groups = grouped.into_values().collect::<Vec<_>>();
        groups.sort_by(|(left_rows, left_columns), (right_rows, right_columns)| {
            let left_size = left_rows.len() + left_columns.len();
            let right_size = right_rows.len() + right_columns.len();
            right_size.cmp(&left_size).then_with(|| {
                left_rows
                    .first()
                    .copied()
                    .unwrap_or(row_count + left_columns.first().copied().unwrap_or(0))
                    .cmp(
                        &right_rows
                            .first()
                            .copied()
                            .unwrap_or(row_count + right_columns.first().copied().unwrap_or(0)),
                    )
            })
        });
        let mut components = Vec::with_capacity(groups.len());
        let mut row_order = Vec::with_capacity(row_count);
        let mut column_order = Vec::with_capacity(column_count);
        let mut row_component = vec![0; row_count];

        for (component_index, (rows, columns)) in groups.into_iter().enumerate() {
            let nonzeros = rows.iter().map(|&row| constraints[row].terms.len()).sum();
            let mut continuous = 0;
            let mut integer = 0;
            let mut binary = 0;
            for &column in &columns {
                match variables[column].kind {
                    VariableKind::Continuous => continuous += 1,
                    VariableKind::Integer => integer += 1,
                    VariableKind::Binary => binary += 1,
                }
            }
            for &row in &rows {
                row_component[row] = component_index;
            }
            row_order.extend(rows.iter().copied());
            column_order.extend(columns.iter().copied());
            components.push(Component {
                rows,
                columns,
                nonzeros,
                continuous,
                integer,
                binary,
            });
        }

        let mut column_position = vec![0; column_count];
        for (position, &column) in column_order.iter().enumerate() {
            column_position[column] = position;
        }
        Self {
            components,
            row_order,
            column_order,
            column_position,
            row_component,
        }
    }
}

struct UnionFind {
    parent: Vec<usize>,
    rank: Vec<u8>,
}

impl UnionFind {
    fn new(size: usize) -> Self {
        Self {
            parent: (0..size).collect(),
            rank: vec![0; size],
        }
    }

    fn find(&mut self, mut item: usize) -> usize {
        while self.parent[item] != item {
            self.parent[item] = self.parent[self.parent[item]];
            item = self.parent[item];
        }
        item
    }

    fn union(&mut self, left: usize, right: usize) {
        let mut left = self.find(left);
        let mut right = self.find(right);
        if left == right {
            return;
        }
        if self.rank[left] < self.rank[right] {
            std::mem::swap(&mut left, &mut right);
        }
        self.parent[right] = left;
        if self.rank[left] == self.rank[right] {
            self.rank[left] += 1;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Term;

    #[test]
    fn finds_and_orders_disconnected_components() {
        let variables = (0..4)
            .map(|index| Variable {
                name: format!("x{index}"),
                kind: if index == 3 {
                    VariableKind::Binary
                } else {
                    VariableKind::Continuous
                },
                lower: 0.0,
                upper: 1.0,
                objective: 0.0,
            })
            .collect::<Vec<_>>();
        let constraints = vec![
            ConstraintRow {
                name: "large-a".to_owned(),
                lower: 0.0,
                upper: 1.0,
                terms: vec![Term {
                    column: 0,
                    coefficient: 1.0,
                }],
                tags: Vec::new(),
            },
            ConstraintRow {
                name: "large-b".to_owned(),
                lower: 0.0,
                upper: 1.0,
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
            },
            ConstraintRow {
                name: "small".to_owned(),
                lower: 0.0,
                upper: 1.0,
                terms: vec![Term {
                    column: 2,
                    coefficient: 1.0,
                }],
                tags: Vec::new(),
            },
        ];

        let analysis = ComponentAnalysis::analyze(&constraints, &variables);
        assert_eq!(analysis.components.len(), 3); // two matrix blocks + isolated x3
        assert_eq!(analysis.components[0].rows.len(), 2);
        assert_eq!(analysis.components[0].columns.len(), 2);
        assert_eq!(analysis.components[0].nonzeros, 3);
        assert_eq!(analysis.column_order.len(), 4);
    }
}

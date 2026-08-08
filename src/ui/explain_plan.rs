//! Turning an execution plan into something readable in the result grid.
//!
//! The grid is the right surface for this: it already searches, exports, copies
//! and totals selections, and a plan is a table with one extra thing to show —
//! which step feeds which. That relationship is drawn with connector glyphs in
//! the `Operation` column, built from the real parent links the database
//! reports. Nothing here guesses structure from indentation.

use crate::utils::arithmetic::safe_div;
use std::collections::{HashMap, HashSet};

/// One step of an Oracle plan, straight out of `PLAN_TABLE`.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct PlanNode {
    pub id: i64,
    pub parent_id: Option<i64>,
    /// `OPERATION` and `OPTIONS` already joined, e.g. `TABLE ACCESS FULL`.
    pub operation: String,
    /// `OWNER.NAME`, or just the name when the owner adds nothing.
    pub object_name: String,
    pub cardinality: Option<i64>,
    pub bytes: Option<i64>,
    pub cost: Option<i64>,
    /// `ACCESS_PREDICATES` and `FILTER_PREDICATES`, joined.
    pub predicates: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ExplainPlanData {
    /// A real tree: every row knows its parent. Oracle.
    Tree(Vec<PlanNode>),
    /// The server's own EXPLAIN table, passed through unchanged. MySQL and
    /// MariaDB: classic `EXPLAIN` has no parent column, and deriving one from
    /// `id`/`select_type` would be a guess, so no tree is drawn.
    Flat {
        columns: Vec<String>,
        rows: Vec<Vec<String>>,
    },
}

impl ExplainPlanData {
    pub fn is_empty(&self) -> bool {
        match self {
            Self::Tree(nodes) => nodes.is_empty(),
            Self::Flat { rows, .. } => rows.is_empty(),
        }
    }
}

/// Width of the share bar, in cells.
const SHARE_BAR_CELLS: usize = 10;

/// Render a plan as grid columns and rows.
pub fn plan_grid(data: &ExplainPlanData) -> (Vec<String>, Vec<Vec<String>>) {
    match data {
        ExplainPlanData::Tree(nodes) => tree_grid(nodes),
        ExplainPlanData::Flat { columns, rows } => flat_grid(columns, rows),
    }
}

fn tree_grid(nodes: &[PlanNode]) -> (Vec<String>, Vec<Vec<String>>) {
    let columns = vec![
        "Operation".to_string(),
        "Object".to_string(),
        "Rows".to_string(),
        "Bytes".to_string(),
        "Cost".to_string(),
        "Cost %".to_string(),
        "Predicates".to_string(),
    ];

    let index_of = index_by_id(nodes);
    let last_child = last_child_flags(nodes);
    let prefixes = connector_prefixes(nodes, &index_of, &last_child);
    let children_cost = children_cost_by_parent(nodes);
    let total_cost = root_cost(nodes);
    let rows = nodes
        .iter()
        .enumerate()
        .map(|(index, node)| {
            let prefix = prefixes.get(index).cloned().unwrap_or_default();
            let own_cost = node
                .cost
                .unwrap_or(0)
                .saturating_sub(children_cost.get(&node.id).copied().unwrap_or(0))
                .max(0);
            vec![
                format!("{prefix}{}", node.operation),
                node.object_name.clone(),
                format_count(node.cardinality),
                format_count(node.bytes),
                format_count(node.cost),
                share_cell(own_cost, total_cost),
                node.predicates.clone(),
            ]
        })
        .collect();

    (columns, rows)
}

fn flat_grid(columns: &[String], rows: &[Vec<String>]) -> (Vec<String>, Vec<Vec<String>>) {
    let Some(rows_column) = columns
        .iter()
        .position(|column| column.eq_ignore_ascii_case("rows"))
    else {
        return (columns.to_vec(), rows.to_vec());
    };

    // The server's row estimate is the only quantity in a classic EXPLAIN worth
    // comparing across steps, so it gets the share bar the tree gives to cost.
    let values: Vec<Option<i64>> = rows
        .iter()
        .map(|row| row.get(rows_column).and_then(|value| parse_count(value)))
        .collect();
    let total = values
        .iter()
        .flatten()
        .fold(0i64, |total, value| total.saturating_add(*value));
    if total <= 0 {
        return (columns.to_vec(), rows.to_vec());
    }

    let mut columns = columns.to_vec();
    columns.push("Rows %".to_string());
    let rows = rows
        .iter()
        .zip(values)
        .map(|(row, value)| {
            let mut row = row.clone();
            // A row estimate the server did not give is left blank rather than
            // drawn as 0%, which would read as "this step returns nothing".
            row.push(match value {
                Some(value) => share_cell(value, total),
                None => String::new(),
            });
            row
        })
        .collect();
    (columns, rows)
}

/// Total reported cost of each step's direct children.
///
/// Subtracting this from a step's own cost is what turns Oracle's cumulative
/// `COST` into the cost a step spends on itself — otherwise every ancestor
/// looks as expensive as the whole plan simply for containing it.
fn children_cost_by_parent(nodes: &[PlanNode]) -> HashMap<i64, i64> {
    let mut totals: HashMap<i64, i64> = HashMap::with_capacity(nodes.len());
    for node in nodes {
        let Some(parent_id) = node.parent_id else {
            continue;
        };
        let total = totals.entry(parent_id).or_insert(0);
        *total = total.saturating_add(node.cost.unwrap_or(0));
    }
    totals
}

/// Row index of each step id. Plans can run to thousands of steps, and this is
/// what keeps the walk below from rescanning the list for every ancestor.
fn index_by_id(nodes: &[PlanNode]) -> HashMap<i64, usize> {
    let mut index_of: HashMap<i64, usize> = HashMap::with_capacity(nodes.len());
    for (index, node) in nodes.iter().enumerate() {
        index_of.entry(node.id).or_insert(index);
    }
    index_of
}

/// Whether each step is the last of its parent's children, in display order.
/// One reverse pass: the first time a parent is seen from the end, that row is
/// its last child.
fn last_child_flags(nodes: &[PlanNode]) -> Vec<bool> {
    let mut flags = vec![false; nodes.len()];
    let mut seen: HashSet<i64> = HashSet::with_capacity(nodes.len());
    for (index, node) in nodes.iter().enumerate().rev() {
        if let Some(parent_id) = node.parent_id {
            flags[index] = seen.insert(parent_id);
        }
    }
    flags
}

fn root_cost(nodes: &[PlanNode]) -> i64 {
    nodes
        .iter()
        .filter(|node| node.parent_id.is_none())
        .map(|node| node.cost.unwrap_or(0))
        .max()
        .unwrap_or(0)
}

fn share_cell(value: i64, total: i64) -> String {
    if total <= 0 {
        return String::new();
    }
    let percent = safe_div(value.saturating_mul(100), total).clamp(0, 100);
    let filled = usize::try_from(safe_div(
        percent.saturating_mul(SHARE_BAR_CELLS as i64),
        100,
    ))
    .unwrap_or(0)
    .min(SHARE_BAR_CELLS);
    format!(
        "{}{} {percent:>3}%",
        "\u{2588}".repeat(filled),
        "\u{2591}".repeat(SHARE_BAR_CELLS - filled)
    )
}

/// Connector prefix per row, in the order the rows are given.
///
/// Rows arrive in the database's own display order, so the prefix only has to
/// describe the parent chain: a vertical bar for every ancestor that still has
/// a following sibling, then a corner for this row.
fn connector_prefixes(
    nodes: &[PlanNode],
    index_of: &HashMap<i64, usize>,
    last_child: &[bool],
) -> Vec<String> {
    let mut prefixes = Vec::with_capacity(nodes.len());
    for (index, node) in nodes.iter().enumerate() {
        if node.parent_id.is_none() {
            prefixes.push(String::new());
            continue;
        }
        let mut segments: Vec<&'static str> = Vec::new();
        segments.push(if last_child.get(index).copied().unwrap_or(true) {
            "\u{2514}\u{2500} "
        } else {
            "\u{251c}\u{2500} "
        });

        // Walk up the ancestors, prepending a trunk for each one that is itself
        // followed by a sibling. The step count is bounded by the number of
        // rows: the ids come from the server, and a row that claims itself (or
        // an ancestor) as its parent would otherwise spin here forever.
        let mut current_index = index;
        for _ in 0..nodes.len() {
            let Some(parent_index) = nodes
                .get(current_index)
                .and_then(|step| step.parent_id)
                .and_then(|parent_id| index_of.get(&parent_id).copied())
            else {
                break;
            };
            if nodes
                .get(parent_index)
                .and_then(|step| step.parent_id)
                .is_none()
            {
                break;
            }
            segments.push(if last_child.get(parent_index).copied().unwrap_or(true) {
                "   "
            } else {
                "\u{2502}  "
            });
            current_index = parent_index;
        }

        segments.reverse();
        prefixes.push(segments.concat());
    }
    prefixes
}

fn format_count(value: Option<i64>) -> String {
    match value {
        Some(value) => group_digits(value),
        None => String::new(),
    }
}

fn group_digits(value: i64) -> String {
    let negative = value < 0;
    let digits = value.unsigned_abs().to_string();
    let mut grouped = String::with_capacity(digits.len() + safe_div(digits.len(), 3) + 1);
    if negative {
        grouped.push('-');
    }
    for (offset, digit) in digits.chars().enumerate() {
        if offset > 0 && (digits.len() - offset).is_multiple_of(3) {
            grouped.push(',');
        }
        grouped.push(digit);
    }
    grouped
}

/// Join `OPERATION` and `OPTIONS` the way `DBMS_XPLAN` prints them.
pub fn operation_label(operation: &str, options: &str) -> String {
    match (operation.trim(), options.trim()) {
        (operation, "") => operation.to_string(),
        ("", options) => options.to_string(),
        (operation, options) => format!("{operation} {options}"),
    }
}

/// `OWNER.NAME`, dropping an owner that adds nothing.
pub fn object_label(owner: &str, name: &str) -> String {
    match (owner.trim(), name.trim()) {
        (_, "") => String::new(),
        ("", name) => name.to_string(),
        (owner, name) => format!("{owner}.{name}"),
    }
}

/// The two predicate columns, labelled so it is clear which is which.
pub fn predicate_label(access: &str, filter: &str) -> String {
    let mut parts = Vec::new();
    if !access.trim().is_empty() {
        parts.push(format!("access({})", access.trim()));
    }
    if !filter.trim().is_empty() {
        parts.push(format!("filter({})", filter.trim()));
    }
    parts.join(" ")
}

fn parse_count(value: &str) -> Option<i64> {
    let trimmed = value.trim().replace(',', "");
    trimmed.parse::<i64>().ok()
}

/// Columns the Oracle plan query selects, in order. Both drivers hand the rows
/// back as text so the two paths cannot drift.
pub const ORACLE_PLAN_COLUMN_COUNT: usize = 11;

/// Build plan nodes from the text rows of the Oracle plan query.
///
/// A row without a usable `ID` is dropped rather than guessed at: it would have
/// no place in the parent chain, and inventing one would draw a tree the
/// database never described.
pub fn oracle_plan_nodes(rows: &[Vec<String>]) -> Vec<PlanNode> {
    rows.iter()
        .filter_map(|row| {
            let field = |index: usize| row.get(index).map(String::as_str).unwrap_or("");
            Some(PlanNode {
                id: parse_count(field(0))?,
                parent_id: parse_count(field(1)),
                operation: operation_label(field(2), field(3)),
                object_name: object_label(field(4), field(5)),
                cardinality: parse_count(field(6)),
                bytes: parse_count(field(7)),
                cost: parse_count(field(8)),
                predicates: predicate_label(field(9), field(10)),
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn node(id: i64, parent_id: Option<i64>, operation: &str, cost: Option<i64>) -> PlanNode {
        PlanNode {
            id,
            parent_id,
            operation: operation.to_string(),
            cost,
            ..PlanNode::default()
        }
    }

    /// SELECT STATEMENT
    /// └─ HASH JOIN
    ///    ├─ TABLE ACCESS FULL (DEPT)
    ///    └─ TABLE ACCESS FULL (EMP)
    fn join_plan() -> Vec<PlanNode> {
        vec![
            node(0, None, "SELECT STATEMENT", Some(47)),
            node(1, Some(0), "HASH JOIN", Some(47)),
            node(2, Some(1), "TABLE ACCESS FULL", Some(3)),
            node(3, Some(1), "TABLE ACCESS FULL", Some(31)),
        ]
    }

    fn operations(nodes: &[PlanNode]) -> Vec<String> {
        let (_, rows) = tree_grid(nodes);
        rows.into_iter()
            .map(|row| row.first().cloned().unwrap_or_default())
            .collect()
    }

    #[test]
    fn the_root_has_no_connector() {
        assert_eq!(operations(&join_plan())[0], "SELECT STATEMENT");
    }

    #[test]
    fn an_only_child_gets_a_corner() {
        assert_eq!(operations(&join_plan())[1], "└─ HASH JOIN");
    }

    #[test]
    fn a_non_final_sibling_gets_a_tee() {
        assert_eq!(operations(&join_plan())[2], "   ├─ TABLE ACCESS FULL");
    }

    #[test]
    fn the_final_sibling_gets_a_corner() {
        assert_eq!(operations(&join_plan())[3], "   └─ TABLE ACCESS FULL");
    }

    #[test]
    fn an_ancestor_with_a_following_sibling_draws_a_trunk() {
        // 0 ─┬ 1 ─ 3
        //    └ 2
        let nodes = vec![
            node(0, None, "SELECT STATEMENT", Some(10)),
            node(1, Some(0), "NESTED LOOPS", Some(6)),
            node(3, Some(1), "INDEX RANGE SCAN", Some(2)),
            node(2, Some(0), "TABLE ACCESS FULL", Some(4)),
        ];
        let rendered = operations(&nodes);
        assert_eq!(rendered[1], "├─ NESTED LOOPS");
        assert_eq!(rendered[2], "│  └─ INDEX RANGE SCAN");
        assert_eq!(rendered[3], "└─ TABLE ACCESS FULL");
    }

    #[test]
    fn deep_nesting_keeps_one_segment_per_level() {
        let nodes = vec![
            node(0, None, "A", Some(9)),
            node(1, Some(0), "B", Some(9)),
            node(2, Some(1), "C", Some(9)),
            node(3, Some(2), "D", Some(9)),
        ];
        assert_eq!(operations(&nodes)[3], "      └─ D");
    }

    #[test]
    fn a_row_whose_parent_is_missing_still_renders() {
        let nodes = vec![node(7, Some(99), "ORPHAN", Some(1))];
        assert_eq!(operations(&nodes), vec!["└─ ORPHAN".to_string()]);
    }

    #[test]
    fn a_step_that_claims_itself_as_its_parent_does_not_spin() {
        let nodes = vec![node(0, Some(0), "SELF PARENT", Some(1))];
        // The point of the assertion is that this returns at all.
        assert_eq!(operations(&nodes).len(), 1);
    }

    #[test]
    fn a_cycle_between_two_steps_does_not_spin() {
        let nodes = vec![
            node(0, Some(1), "A", Some(1)),
            node(1, Some(0), "B", Some(1)),
        ];
        assert_eq!(operations(&nodes).len(), 2);
    }

    #[test]
    fn a_large_plan_renders_quickly_enough_for_the_ui_thread() {
        // A wide, deep plan: 200 levels of nesting, 10 siblings at each.
        let mut nodes = vec![node(0, None, "SELECT STATEMENT", Some(100_000))];
        let mut next_id = 1i64;
        let mut parent = 0i64;
        for _ in 0..200 {
            let mut first = None;
            for _ in 0..10 {
                nodes.push(node(next_id, Some(parent), "TABLE ACCESS FULL", Some(10)));
                if first.is_none() {
                    first = Some(next_id);
                }
                next_id += 1;
            }
            parent = first.unwrap_or(parent);
        }
        let started = std::time::Instant::now();
        let (_, rows) = tree_grid(&nodes);
        let elapsed = started.elapsed();
        assert_eq!(rows.len(), nodes.len());
        println!("{} steps rendered in {elapsed:?}", nodes.len());
        assert!(
            elapsed < std::time::Duration::from_secs(2),
            "rendering {} steps took {elapsed:?}",
            nodes.len()
        );
    }

    #[test]
    fn an_empty_plan_produces_headers_but_no_rows() {
        let (columns, rows) = tree_grid(&[]);
        assert_eq!(columns.len(), 7);
        assert!(rows.is_empty());
    }

    #[test]
    fn cost_is_reported_exactly_as_the_database_gave_it() {
        let (_, rows) = tree_grid(&join_plan());
        assert_eq!(rows[0][4], "47");
        assert_eq!(rows[3][4], "31");
    }

    #[test]
    fn the_cost_share_is_the_step_s_own_cost_not_the_cumulative_one() {
        // Root 47, its only child also 47, so the root spends nothing itself.
        let (_, rows) = tree_grid(&join_plan());
        assert!(rows[0][5].ends_with("  0%"), "{}", rows[0][5]);
        // The join reports 47 with children summing to 34, so 13/47 = 27%.
        assert!(rows[1][5].ends_with(" 27%"), "{}", rows[1][5]);
        assert!(rows[2][5].ends_with("  6%"), "{}", rows[2][5]);
        assert!(rows[3][5].ends_with(" 65%"), "{}", rows[3][5]);
    }

    #[test]
    fn the_share_bar_is_filled_in_proportion() {
        assert_eq!(share_cell(50, 100), "█████░░░░░  50%");
        assert_eq!(share_cell(0, 100), "░░░░░░░░░░   0%");
        assert_eq!(share_cell(100, 100), "██████████ 100%");
    }

    #[test]
    fn a_zero_total_leaves_the_share_blank_rather_than_dividing_by_zero() {
        assert_eq!(share_cell(5, 0), "");
        let nodes = vec![node(0, None, "SELECT STATEMENT", Some(0))];
        let (_, rows) = tree_grid(&nodes);
        assert_eq!(rows[0][5], "");
    }

    #[test]
    fn a_child_costing_more_than_its_parent_never_goes_negative() {
        let nodes = vec![
            node(0, None, "SELECT STATEMENT", Some(10)),
            node(1, Some(0), "CHILD", Some(40)),
        ];
        let (_, rows) = tree_grid(&nodes);
        assert!(rows[0][5].ends_with("  0%"), "{}", rows[0][5]);
    }

    #[test]
    fn a_missing_cost_reads_as_blank_not_zero() {
        let nodes = vec![
            node(0, None, "SELECT STATEMENT", Some(10)),
            node(1, Some(0), "REMOTE", None),
        ];
        let (_, rows) = tree_grid(&nodes);
        assert_eq!(rows[1][4], "");
    }

    #[test]
    fn large_numbers_are_grouped_for_reading() {
        assert_eq!(group_digits(0), "0");
        assert_eq!(group_digits(999), "999");
        assert_eq!(group_digits(1_000), "1,000");
        assert_eq!(group_digits(1_234_567), "1,234,567");
        assert_eq!(group_digits(-1_234), "-1,234");
        assert_eq!(group_digits(i64::MIN), "-9,223,372,036,854,775,808");
    }

    #[test]
    fn rows_and_bytes_carry_the_estimates() {
        let nodes = vec![PlanNode {
            id: 0,
            parent_id: None,
            operation: "SELECT STATEMENT".to_string(),
            object_name: "SCOTT.EMP".to_string(),
            cardinality: Some(1_000),
            bytes: Some(52_000),
            cost: Some(3),
            predicates: "access(\"ID\"=1)".to_string(),
        }];
        let (_, rows) = tree_grid(&nodes);
        assert_eq!(rows[0][1], "SCOTT.EMP");
        assert_eq!(rows[0][2], "1,000");
        assert_eq!(rows[0][3], "52,000");
        assert_eq!(rows[0][6], "access(\"ID\"=1)");
    }

    #[test]
    fn an_operation_label_joins_operation_and_options() {
        assert_eq!(operation_label("TABLE ACCESS", "FULL"), "TABLE ACCESS FULL");
        assert_eq!(operation_label("HASH JOIN", ""), "HASH JOIN");
        assert_eq!(operation_label("", "FULL"), "FULL");
        assert_eq!(operation_label("", ""), "");
    }

    #[test]
    fn an_object_label_drops_an_empty_owner() {
        assert_eq!(object_label("SCOTT", "EMP"), "SCOTT.EMP");
        assert_eq!(object_label("", "EMP"), "EMP");
        assert_eq!(object_label("SCOTT", ""), "");
    }

    #[test]
    fn predicates_say_which_kind_they_are() {
        assert_eq!(predicate_label("A=1", ""), "access(A=1)");
        assert_eq!(predicate_label("", "B>2"), "filter(B>2)");
        assert_eq!(predicate_label("A=1", "B>2"), "access(A=1) filter(B>2)");
        assert_eq!(predicate_label("  ", ""), "");
    }

    #[test]
    fn a_flat_plan_keeps_the_servers_own_columns() {
        let data = ExplainPlanData::Flat {
            columns: vec!["id".to_string(), "table".to_string(), "rows".to_string()],
            rows: vec![
                vec!["1".to_string(), "orders".to_string(), "300".to_string()],
                vec!["1".to_string(), "items".to_string(), "100".to_string()],
            ],
        };
        let (columns, rows) = plan_grid(&data);
        assert_eq!(columns[..3], ["id", "table", "rows"]);
        assert_eq!(rows[0][..3], ["1", "orders", "300"]);
    }

    #[test]
    fn a_flat_plan_gains_a_row_share_column() {
        let data = ExplainPlanData::Flat {
            columns: vec!["rows".to_string()],
            rows: vec![vec!["300".to_string()], vec!["100".to_string()]],
        };
        let (columns, rows) = plan_grid(&data);
        assert_eq!(columns, vec!["rows".to_string(), "Rows %".to_string()]);
        assert!(rows[0][1].ends_with(" 75%"), "{}", rows[0][1]);
        assert!(rows[1][1].ends_with(" 25%"), "{}", rows[1][1]);
    }

    #[test]
    fn a_flat_row_with_no_estimate_gets_a_blank_share_not_a_zero() {
        let data = ExplainPlanData::Flat {
            columns: vec!["rows".to_string()],
            rows: vec![vec!["300".to_string()], vec!["NULL".to_string()]],
        };
        let (_, rows) = plan_grid(&data);
        assert!(rows[0][1].ends_with(" 100%"), "{}", rows[0][1]);
        assert_eq!(rows[1][1], "");
    }

    #[test]
    fn a_flat_plan_without_a_rows_column_is_passed_through_untouched() {
        let data = ExplainPlanData::Flat {
            columns: vec!["id".to_string(), "Extra".to_string()],
            rows: vec![vec!["1".to_string(), "Using index".to_string()]],
        };
        let (columns, rows) = plan_grid(&data);
        assert_eq!(columns.len(), 2);
        assert_eq!(rows[0].len(), 2);
    }

    #[test]
    fn a_flat_plan_whose_rows_are_all_null_gains_no_share_column() {
        let data = ExplainPlanData::Flat {
            columns: vec!["rows".to_string()],
            rows: vec![vec!["NULL".to_string()], vec!["".to_string()]],
        };
        let (columns, _) = plan_grid(&data);
        assert_eq!(columns, vec!["rows".to_string()]);
    }

    fn oracle_row(id: &str, parent: &str, operation: &str, options: &str) -> Vec<String> {
        vec![
            id.to_string(),
            parent.to_string(),
            operation.to_string(),
            options.to_string(),
            "SCOTT".to_string(),
            "EMP".to_string(),
            "1000".to_string(),
            "52000".to_string(),
            "47".to_string(),
            "\"ID\"=1".to_string(),
            String::new(),
        ]
    }

    #[test]
    fn oracle_text_rows_become_plan_nodes() {
        let nodes = oracle_plan_nodes(&[
            oracle_row("0", "", "SELECT STATEMENT", ""),
            oracle_row("1", "0", "TABLE ACCESS", "FULL"),
        ]);
        assert_eq!(nodes.len(), 2);
        assert_eq!(nodes[0].id, 0);
        assert_eq!(nodes[0].parent_id, None);
        assert_eq!(nodes[0].operation, "SELECT STATEMENT");
        assert_eq!(nodes[1].parent_id, Some(0));
        assert_eq!(nodes[1].operation, "TABLE ACCESS FULL");
        assert_eq!(nodes[1].object_name, "SCOTT.EMP");
        assert_eq!(nodes[1].cardinality, Some(1000));
        assert_eq!(nodes[1].bytes, Some(52000));
        assert_eq!(nodes[1].cost, Some(47));
        assert_eq!(nodes[1].predicates, "access(\"ID\"=1)");
    }

    #[test]
    fn an_oracle_row_without_an_id_is_dropped_rather_than_guessed() {
        assert!(oracle_plan_nodes(&[oracle_row("", "", "A", "")]).is_empty());
        assert!(oracle_plan_nodes(&[Vec::new()]).is_empty());
    }

    #[test]
    fn a_short_oracle_row_does_not_panic() {
        let nodes = oracle_plan_nodes(&[vec!["0".to_string()]]);
        assert_eq!(nodes.len(), 1);
        assert_eq!(nodes[0].operation, "");
        assert_eq!(nodes[0].cost, None);
    }

    #[test]
    fn oracle_plan_nodes_render_straight_into_a_grid() {
        let nodes = oracle_plan_nodes(&[
            oracle_row("0", "", "SELECT STATEMENT", ""),
            oracle_row("1", "0", "TABLE ACCESS", "FULL"),
        ]);
        let (columns, rows) = plan_grid(&ExplainPlanData::Tree(nodes));
        assert_eq!(columns[0], "Operation");
        assert_eq!(rows[1][0], "└─ TABLE ACCESS FULL");
    }

    #[test]
    fn emptiness_is_reported_for_both_shapes() {
        assert!(ExplainPlanData::Tree(Vec::new()).is_empty());
        assert!(!ExplainPlanData::Tree(join_plan()).is_empty());
        assert!(ExplainPlanData::Flat {
            columns: vec!["id".to_string()],
            rows: Vec::new(),
        }
        .is_empty());
    }
}

#![allow(clippy::cargo, clippy::pedantic)]

// Dialect probe for the "filter a general query result by re-querying" design
// (docs_items/item_list.md, appendix A). It answers the three questions A.5
// lists as unverified, before any implementation work starts:
//
//   1. Does `(<user query>) sq_src` — a derived-table alias without `AS` —
//      parse on Oracle, MySQL, and MariaDB alike?
//   2. What exactly happens when the wrapped query has duplicate column names
//      (the `SELECT *` join case), and which error code comes back?
//   3. Is a `WITH` clause accepted inside the derived table?
//
// It also runs the full paging shape the implementation would emit, so the
// answers transfer to the real SQL rather than to a simplified stand-in.
//
// This is a DESIGN PROBE, not a regression test: nothing calls it in CI, and it
// asserts facts about the servers, not about our code. The SQL builders below
// mirror `src/ui/table_browse.rs` — `marked_materialized_sql` (line 199),
// `build_logical_sql` (line 226), and `build_page_sql` (line 261) — because
// those are `pub(crate)` and this is a separate crate. Keep them in sync; once
// the real builder learns to take a derived relation, call it directly instead.
//
// Usage: cargo run --bin verify_derived_table_wrap <oracle|mysql|mariadb>
//
// Run ONE container at a time (the boxes are memory-hungry):
//   docker start oracle                  # 127.0.0.1:1521 FREE system/password
//   docker start space-query-mysql80     # 127.0.0.1:3307 root/spacequery
//   docker start space-query-mariadb122  # 127.0.0.1:3306 root/password

use mysql::prelude::Queryable;
use mysql::{Conn, OptsBuilder};
use std::env;
use tns_thin::exec::{OracleValue, StatementRequest};
use tns_thin::{ConnectTarget, OracleThinConfig, OracleThinSession};

// ---------------------------------------------------------------------------
// SQL shapes mirrored from src/ui/table_browse.rs
// ---------------------------------------------------------------------------

const PAGE_COL: &str = "SQ_INTERNAL_PAGE_ROW";

/// The production writer, not a copy of it.
///
/// This mirrored `marked_materialized_sql` by hand, and the copy drifted the
/// first time the real one moved: the mark now goes after the leading KEYWORD
/// rather than in front of a statement, because a comment that OPENS a
/// statement is one the executor's splitter drops.
fn marked(sql: &str) -> String {
    space_query::ui::table_browse::marked_materialized_sql(sql)
}

/// Mirrors `build_logical_sql` (`table_browse.rs:226`).
fn logical(relation: &str, where_expr: &str, order_by_expr: &str) -> String {
    let mut sql = format!("SELECT * FROM {relation}");
    if !where_expr.is_empty() {
        sql.push_str("\nWHERE ");
        sql.push_str(where_expr);
    }
    if !order_by_expr.is_empty() {
        sql.push_str("\nORDER BY ");
        sql.push_str(order_by_expr);
    }
    sql
}

/// Mirrors the `Rownum` arm of `build_page_sql` (`table_browse.rs:283`).
fn oracle_page(logical_sql: &str, offset: u64, page_size: u64) -> String {
    let upper_bound = offset + page_size + 1;
    let inner = logical_sql
        .lines()
        .map(|line| format!("    {line}"))
        .collect::<Vec<_>>()
        .join("\n");
    marked(&format!(
        "SELECT *\nFROM (\n  SELECT sq_page_source.*, ROWNUM AS {PAGE_COL}\n  FROM (\n{inner}\n  ) sq_page_source\n  WHERE ROWNUM <= {upper_bound}\n)\nWHERE {PAGE_COL} > {offset}"
    ))
}

/// Mirrors the `LimitOffset` arm of `build_page_sql` (`table_browse.rs:296`).
fn mysql_page(logical_sql: &str, offset: u64, page_size: u64) -> String {
    format!(
        "{}\nLIMIT {} OFFSET {}",
        marked(logical_sql),
        page_size + 1,
        offset
    )
}

/// The relation expression the implementation builds from a user query.
///
/// Mirrors `crate::ui::result_filter::derived_relation_sql`. The closing paren
/// sits on its own line so a statement ending in a line comment cannot swallow
/// it — the `trailing_line_comment` case below is what proves that.
fn derived(user_sql: &str) -> String {
    format!(
        "(\n{}\n) sq_src",
        user_sql.trim().trim_end_matches(';').trim()
    )
}

// ---------------------------------------------------------------------------
// Cases
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, PartialEq, Eq)]
enum Expect {
    Accepts,
    Rejects,
    Unknown,
}

impl Expect {
    fn label(self) -> &'static str {
        match self {
            Expect::Accepts => "accepts",
            Expect::Rejects => "rejects",
            Expect::Unknown => "UNKNOWN (this is what we are here to find out)",
        }
    }
}

struct Case {
    name: &'static str,
    question: &'static str,
    sql: String,
    expect: Expect,
}

struct Outcome {
    name: &'static str,
    question: &'static str,
    expect: Expect,
    accepted: bool,
    detail: String,
    rows: Vec<Vec<String>>,
}

impl Outcome {
    /// A case "matches" when the server agreed with a stated expectation.
    /// `Unknown` cases never fail the run — they are recorded, not judged.
    fn matches(&self) -> bool {
        match self.expect {
            Expect::Accepts => self.accepted,
            Expect::Rejects => !self.accepted,
            Expect::Unknown => true,
        }
    }
}

/// The user queries that get wrapped. Shared by all three backends so the
/// answers are comparable; only the dialect-specific spelling differs.
fn cases(oracle: bool) -> Vec<Case> {
    let dual = if oracle { " FROM DUAL" } else { "" };
    let plain = "SELECT ID, NAME, DEPTNO FROM SQ_WRAP_A";
    let joined = "SELECT * FROM SQ_WRAP_A a JOIN SQ_WRAP_B b ON a.DEPTNO = b.DEPTNO";
    let mut cases = vec![
        Case {
            name: "alias_no_as",
            question: "Q1: does `(...) sq_src` (alias, no AS) parse?",
            sql: format!("SELECT * FROM {}", derived(plain)),
            expect: Expect::Accepts,
        },
        Case {
            name: "alias_with_as",
            question: "Q1: is `AS sq_src` usable instead? (Oracle should reject)",
            sql: format!("SELECT * FROM ({}) AS sq_src", plain),
            expect: if oracle {
                Expect::Rejects
            } else {
                Expect::Accepts
            },
        },
        Case {
            name: "no_alias",
            question: "Q1: is the alias required? (MySQL family should reject)",
            sql: format!("SELECT * FROM ({plain})"),
            expect: if oracle {
                Expect::Accepts
            } else {
                Expect::Rejects
            },
        },
        // Measured 2026-08-06: the two families split here. Oracle wraps a
        // duplicate-column relation happily and only fails when something
        // *references* the duplicated name; MySQL/MariaDB reject the derived
        // table outright. Expectations below encode that measured truth.
        Case {
            name: "dup_columns_join",
            question: "Q2: SELECT * over a join — the common real case",
            sql: format!("SELECT * FROM {}", derived(joined)),
            expect: if oracle {
                Expect::Accepts
            } else {
                Expect::Rejects
            },
        },
        Case {
            name: "dup_columns_explicit",
            question: "Q2: two columns aliased to the same name",
            sql: format!(
                "SELECT * FROM {}",
                derived(&format!("SELECT 1 AS X, 2 AS X{dual}"))
            ),
            expect: Expect::Rejects,
        },
        Case {
            name: "dup_columns_unwrapped",
            question: "Q2: does the same query run fine WITHOUT the wrap?",
            sql: joined.to_string(),
            expect: Expect::Accepts,
        },
        // These three separate "cannot wrap at all" from "cannot filter on that
        // one column" — the distinction the Oracle gate depends on.
        Case {
            name: "dup_join_where_ambiguous",
            question: "Q2: wrapped join, WHERE names the DUPLICATED column",
            sql: logical(&derived(joined), "DEPTNO = 10", ""),
            expect: Expect::Rejects,
        },
        Case {
            name: "dup_join_where_unique",
            question: "Q2: wrapped join, WHERE names a NON-duplicated column",
            sql: logical(&derived(joined), "NAME = 'alpha'", ""),
            expect: if oracle {
                Expect::Accepts
            } else {
                Expect::Rejects
            },
        },
        Case {
            name: "dup_join_order_ambiguous",
            question: "Q2: wrapped join, ORDER BY names the duplicated column",
            sql: logical(&derived(joined), "", "DEPTNO"),
            expect: Expect::Rejects,
        },
        Case {
            name: "with_inside",
            question: "Q3: WITH clause inside the derived table",
            sql: format!(
                "SELECT * FROM {}",
                derived("WITH q AS (SELECT ID, NAME FROM SQ_WRAP_A) SELECT * FROM q")
            ),
            expect: Expect::Accepts,
        },
        Case {
            name: "with_inside_filtered",
            question: "Q3: same, with the WHERE/ORDER BY the feature adds",
            sql: logical(
                &derived("WITH q AS (SELECT ID, NAME FROM SQ_WRAP_A) SELECT * FROM q"),
                "ID > 1",
                "ID DESC",
            ),
            expect: Expect::Accepts,
        },
        Case {
            name: "set_operator",
            question: "bonus: UNION ALL source (a key reason to wrap at all)",
            sql: logical(
                &derived(
                    "SELECT ID, NAME FROM SQ_WRAP_A UNION ALL SELECT DEPTNO, DNAME FROM SQ_WRAP_B",
                ),
                "ID >= 10",
                "ID",
            ),
            expect: Expect::Accepts,
        },
        Case {
            name: "user_order_by_inside",
            question: "bonus: user query already ends with ORDER BY",
            sql: logical(
                &derived("SELECT ID, NAME FROM SQ_WRAP_A ORDER BY NAME DESC"),
                "ID > 0",
                "",
            ),
            expect: Expect::Accepts,
        },
        Case {
            name: "trailing_line_comment",
            question: "does a statement ending in `--` survive the wrap?",
            sql: logical(
                &derived(&format!("{plain} -- rows I care about")),
                "DEPTNO = 10",
                "NAME DESC",
            ),
            expect: Expect::Accepts,
        },
        Case {
            name: "trailing_block_comment",
            question: "same for a trailing block comment",
            sql: logical(&derived(&format!("{plain} /* note */")), "DEPTNO = 10", ""),
            expect: Expect::Accepts,
        },
        Case {
            name: "logical_where_order",
            question: "the plain logical shape the filter bar produces",
            sql: logical(&derived(plain), "DEPTNO = 10", "NAME DESC"),
            expect: Expect::Accepts,
        },
    ];

    // The full paging shape, which is what actually reaches the server.
    // page_size 2 at offset 1 over NAME-ordered rows must return 3 rows
    // (page_size + 1 lookahead): bravo, charlie, delta.
    let paged_logical = logical(&derived(plain), "", "NAME");
    cases.push(Case {
        name: "full_page_sql",
        question: "the complete paging wrap, incl. the +1 lookahead row",
        sql: if oracle {
            oracle_page(&paged_logical, 1, 2)
        } else {
            mysql_page(&paged_logical, 1, 2)
        },
        expect: Expect::Accepts,
    });

    // The paging wrap adds another `*` expansion (`sq_page_source.*`) on top of
    // a relation that may already carry duplicate names.
    let dup_paged_logical = logical(&derived(joined), "", "");
    cases.push(Case {
        name: "dup_join_full_page_sql",
        question: "Q2: the paging wrap over a duplicate-column join",
        sql: if oracle {
            oracle_page(&dup_paged_logical, 0, 2)
        } else {
            mysql_page(&dup_paged_logical, 0, 2)
        },
        expect: if oracle {
            Expect::Accepts
        } else {
            Expect::Rejects
        },
    });

    cases
}

const ORACLE_SETUP: &[&str] = &[
    "CREATE TABLE SQ_WRAP_A (ID NUMBER, NAME VARCHAR2(20), DEPTNO NUMBER)",
    "CREATE TABLE SQ_WRAP_B (DEPTNO NUMBER, DNAME VARCHAR2(20))",
    "INSERT INTO SQ_WRAP_A VALUES (1, 'alpha', 10)",
    "INSERT INTO SQ_WRAP_A VALUES (2, 'bravo', 20)",
    "INSERT INTO SQ_WRAP_A VALUES (3, 'charlie', 10)",
    "INSERT INTO SQ_WRAP_A VALUES (4, 'delta', 20)",
    "INSERT INTO SQ_WRAP_A VALUES (5, 'echo', 30)",
    "INSERT INTO SQ_WRAP_B VALUES (10, 'ACCT')",
    "INSERT INTO SQ_WRAP_B VALUES (20, 'SALES')",
    "INSERT INTO SQ_WRAP_B VALUES (30, 'OPS')",
    "COMMIT",
];

const MYSQL_SETUP: &[&str] = &[
    "CREATE TABLE SQ_WRAP_A (ID INT, NAME VARCHAR(20), DEPTNO INT)",
    "CREATE TABLE SQ_WRAP_B (DEPTNO INT, DNAME VARCHAR(20))",
    "INSERT INTO SQ_WRAP_A VALUES (1,'alpha',10),(2,'bravo',20),(3,'charlie',10),(4,'delta',20),(5,'echo',30)",
    "INSERT INTO SQ_WRAP_B VALUES (10,'ACCT'),(20,'SALES'),(30,'OPS')",
];

// ---------------------------------------------------------------------------
// Oracle (thin — the dialect answer does not depend on the driver, and thin
// needs no Instant Client)
// ---------------------------------------------------------------------------

fn oracle_value_string(value: &OracleValue) -> String {
    match value {
        OracleValue::Null => "NULL".to_string(),
        OracleValue::Number(text) | OracleValue::Text(text) => text.clone(),
        other => format!("{other:?}"),
    }
}

fn run_oracle() -> Result<Vec<Outcome>, String> {
    let host = env::var("ORACLE_TEST_HOST").unwrap_or_else(|_| "127.0.0.1".into());
    let port: u16 = env::var("ORACLE_TEST_PORT")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(1521);
    let service = env::var("ORACLE_TEST_SERVICE").unwrap_or_else(|_| "FREE".into());
    let user = env::var("ORACLE_TEST_USERNAME").unwrap_or_else(|_| "system".into());
    let pass = env::var("ORACLE_TEST_PASSWORD").unwrap_or_else(|_| "password".into());

    let mut config =
        OracleThinConfig::new(ConnectTarget::service_name(host, port, service), user, pass);
    config.connect_options.disable_oob_probe = true;
    let mut session =
        OracleThinSession::connect(config).map_err(|e| format!("thin connect: {e}"))?;
    println!(
        "connected (thin, protocol {:?})",
        session.capabilities().protocol_version
    );

    for table in ["SQ_WRAP_A", "SQ_WRAP_B"] {
        let _ = session.execute(
            &StatementRequest::statement(format!("DROP TABLE {table} PURGE")),
            0,
        );
    }
    for sql in ORACLE_SETUP {
        session
            .execute(&StatementRequest::statement((*sql).to_string()), 0)
            .map_err(|e| format!("setup `{sql}`: {e}"))?;
    }

    let mut outcomes = Vec::new();
    for case in cases(true) {
        print_case(&case);
        let outcome = match session.execute(&StatementRequest::query(case.sql.clone(), 100), 0) {
            Ok(result) => {
                let rows = result
                    .rows
                    .iter()
                    .map(|row| row.iter().map(oracle_value_string).collect())
                    .collect::<Vec<Vec<String>>>();
                Outcome {
                    name: case.name,
                    question: case.question,
                    expect: case.expect,
                    accepted: true,
                    detail: format!("{} row(s)", rows.len()),
                    rows,
                }
            }
            Err(error) => Outcome {
                name: case.name,
                question: case.question,
                expect: case.expect,
                accepted: false,
                detail: first_line(&error.to_string()),
                rows: Vec::new(),
            },
        };
        print_outcome(&outcome);
        outcomes.push(outcome);
    }

    for table in ["SQ_WRAP_A", "SQ_WRAP_B"] {
        let _ = session.execute(
            &StatementRequest::statement(format!("DROP TABLE {table} PURGE")),
            0,
        );
    }
    Ok(outcomes)
}

// ---------------------------------------------------------------------------
// MySQL / MariaDB
// ---------------------------------------------------------------------------

fn mysql_value_string(value: &mysql::Value) -> String {
    match value {
        mysql::Value::NULL => "NULL".to_string(),
        mysql::Value::Bytes(bytes) => String::from_utf8_lossy(bytes).into_owned(),
        other => format!("{other:?}"),
    }
}

fn run_mysql_family(label: &str) -> Result<Vec<Outcome>, String> {
    let (port, user, pass, db) = if label == "mysql" {
        (3307u16, "root", "spacequery", "query_tool_mysql8")
    } else {
        (3306u16, "root", "password", "query_tool_test")
    };
    let opts = OptsBuilder::new()
        .ip_or_hostname(Some("127.0.0.1"))
        .tcp_port(port)
        .user(Some(user))
        .pass(Some(pass))
        .db_name(Some(db));
    let mut conn = Conn::new(opts).map_err(|e| format!("{label} connect: {e}"))?;
    let version: Option<String> = conn
        .query_first("SELECT VERSION()")
        .map_err(|e| format!("{label} version: {e}"))?;
    println!(
        "connected ({label}, server {})",
        version.unwrap_or_default()
    );

    for table in ["SQ_WRAP_A", "SQ_WRAP_B"] {
        conn.query_drop(format!("DROP TABLE IF EXISTS {table}"))
            .map_err(|e| format!("{label} drop {table}: {e}"))?;
    }
    for sql in MYSQL_SETUP {
        conn.query_drop(*sql)
            .map_err(|e| format!("{label} setup `{sql}`: {e}"))?;
    }

    let mut outcomes = Vec::new();
    for case in cases(false) {
        print_case(&case);
        let outcome = match conn.query::<mysql::Row, _>(case.sql.clone()) {
            Ok(result) => {
                let rows = result
                    .iter()
                    .map(|row| {
                        (0..row.len())
                            .map(|i| row.as_ref(i).map(mysql_value_string).unwrap_or_default())
                            .collect()
                    })
                    .collect::<Vec<Vec<String>>>();
                Outcome {
                    name: case.name,
                    question: case.question,
                    expect: case.expect,
                    accepted: true,
                    detail: format!("{} row(s)", rows.len()),
                    rows,
                }
            }
            Err(error) => Outcome {
                name: case.name,
                question: case.question,
                expect: case.expect,
                accepted: false,
                detail: first_line(&error.to_string()),
                rows: Vec::new(),
            },
        };
        print_outcome(&outcome);
        outcomes.push(outcome);
    }

    for table in ["SQ_WRAP_A", "SQ_WRAP_B"] {
        let _ = conn.query_drop(format!("DROP TABLE IF EXISTS {table}"));
    }
    Ok(outcomes)
}

// ---------------------------------------------------------------------------
// Reporting
// ---------------------------------------------------------------------------

fn first_line(text: &str) -> String {
    text.lines()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .unwrap_or("")
        .to_string()
}

fn print_case(case: &Case) {
    println!("\n--- {} ---", case.name);
    println!("  {}", case.question);
    println!("  expected: {}", case.expect.label());
    for line in case.sql.lines() {
        println!("  | {line}");
    }
}

fn print_outcome(outcome: &Outcome) {
    if outcome.accepted {
        println!("  => ACCEPTED, {}", outcome.detail);
        for row in outcome.rows.iter().take(5) {
            println!("     {}", row.join(" | "));
        }
    } else {
        println!("  => REJECTED: {}", outcome.detail);
    }
    if !outcome.matches() {
        println!("  !! does not match the expectation");
    }
}

fn print_summary(target: &str, outcomes: &[Outcome]) -> bool {
    println!("\n==================== {target} SUMMARY ====================");
    let mut surprises = Vec::new();
    for outcome in outcomes {
        let verdict = if outcome.accepted {
            "ACCEPTED"
        } else {
            "REJECTED"
        };
        let flag = if outcome.matches() { "   " } else { "!! " };
        println!(
            "{flag}{:<24} {:<9} {}",
            outcome.name, verdict, outcome.detail
        );
        if !outcome.matches() {
            surprises.push(outcome.name);
        }
    }
    println!("\nQuestions this run answers:");
    for outcome in outcomes {
        if outcome.expect == Expect::Unknown {
            println!(
                "  {} -> {}",
                outcome.question,
                if outcome.accepted {
                    format!("ACCEPTED ({})", outcome.detail)
                } else {
                    format!("REJECTED ({})", outcome.detail)
                }
            );
        }
    }
    if surprises.is_empty() {
        println!("\nNo surprises: every stated expectation held.");
        true
    } else {
        println!("\nUNEXPECTED: {}", surprises.join(", "));
        false
    }
}

fn main() {
    let target = env::args().nth(1).unwrap_or_default();
    let result = match target.as_str() {
        "oracle" => run_oracle(),
        "mysql" => run_mysql_family("mysql"),
        "mariadb" => run_mysql_family("mariadb"),
        other => {
            eprintln!("usage: verify_derived_table_wrap <oracle|mysql|mariadb>");
            if !other.is_empty() {
                eprintln!("unknown target: {other}");
            }
            std::process::exit(2);
        }
    };

    match result {
        Ok(outcomes) => {
            if !print_summary(&target, &outcomes) {
                std::process::exit(1);
            }
        }
        Err(error) => {
            eprintln!("\nFAILED to run {target}: {error}");
            std::process::exit(1);
        }
    }
}

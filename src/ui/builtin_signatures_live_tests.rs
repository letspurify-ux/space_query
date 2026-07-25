use super::builtin_signatures::{
    builtin_signature_label, builtin_signature_syntaxes, MARIADB_FUNCTIONS, MYSQL_FUNCTIONS,
    ORACLE_FUNCTIONS,
};
use crate::db::query::mysql_executor::MysqlExecutor;
use crate::db::DatabaseType;
use mysql::prelude::Queryable;
use tns_thin::{ConnectTarget, OracleThinConfig, OracleThinSession};

fn assert_catalog(db_type: DatabaseType, names: &[&str], expected_len: usize) {
    assert_eq!(names.len(), expected_len);
    assert!(names.windows(2).all(|pair| pair[0] < pair[1]));
    for name in names {
        let label = builtin_signature_label(db_type, name)
            .unwrap_or_else(|| panic!("missing {db_type:?} signature for {name}"));
        assert!(
            label.text.to_ascii_uppercase().starts_with(name),
            "{db_type:?} signature does not start with {name}: {}",
            label.text
        );
        let syntaxes = builtin_signature_syntaxes(db_type, name)
            .unwrap_or_else(|| panic!("missing {db_type:?} syntaxes for {name}"));
        assert!(
            !syntaxes.is_empty(),
            "empty {db_type:?} syntaxes for {name}"
        );
        assert_eq!(label.overloads.len(), syntaxes.len());
    }
}

#[test]
fn builtin_signature_catalogs_match_official_manual_indices() {
    assert_catalog(DatabaseType::Oracle, ORACLE_FUNCTIONS, 464);
    assert_catalog(DatabaseType::MySQL, MYSQL_FUNCTIONS, 408);
    assert_catalog(DatabaseType::MariaDB, MARIADB_FUNCTIONS, 475);
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ArgumentBoundary {
    Minimum,
    Maximum,
}

fn argument_boundaries(required: usize, parsed_maximum: usize) -> Vec<(ArgumentBoundary, usize)> {
    let maximum = parsed_maximum.max(required);
    let mut boundaries = vec![(ArgumentBoundary::Minimum, required)];
    if maximum != required {
        boundaries.push((ArgumentBoundary::Maximum, maximum));
    }
    boundaries
}

fn record_overload_probe_coverage(
    failures: &mut Vec<String>,
    name: &str,
    overload_index: usize,
    syntax: &str,
    boundary_probes: &[(ArgumentBoundary, usize, String)],
    overload_probes: &mut Vec<(usize, String, String)>,
) {
    let first = boundary_probes.first().expect("minimum boundary probe");
    let last = boundary_probes.last().expect("maximum boundary probe");
    if first.1 != last.1 && first.2 == last.2 {
        failures.push(format!(
            "{name} overload {} reuses one SQL probe for argument counts {} and {}: `{syntax}`\n  {}",
            overload_index + 1,
            first.1,
            last.1,
            first.2
        ));
    }
    for (previous_index, previous_syntax, previous_sql) in overload_probes.iter() {
        if previous_sql == &last.2 {
            failures.push(format!(
                "{name} overloads {} and {} reuse one SQL probe:\n  `{previous_syntax}`\n  `{syntax}`\n  {}",
                previous_index + 1,
                overload_index + 1,
                last.2
            ));
        }
    }
    overload_probes.push((overload_index, syntax.to_string(), last.2.clone()));
}

fn generic_expression(name: &str, syntax: &str, argument_count: usize) -> String {
    if !syntax.to_ascii_uppercase().contains(&format!("{name}("))
        && !syntax.to_ascii_uppercase().contains(&format!("{name} ("))
    {
        return name.to_string();
    }
    format!("{name}({})", vec!["NULL"; argument_count].join(","))
}

fn mysql_expression(name: &str, syntax: &str, argument_count: usize) -> String {
    let syntax_upper = syntax.to_ascii_uppercase();
    if argument_count > 0
        && matches!(
            name,
            "CURRENT_TIME"
                | "CURRENT_TIMESTAMP"
                | "CURTIME"
                | "LOCALTIME"
                | "LOCALTIMESTAMP"
                | "NOW"
                | "SYSDATE"
                | "UTC_TIME"
                | "UTC_TIMESTAMP"
        )
    {
        return format!("{name}(0)");
    }
    if matches!(name, "MID" | "SUBSTR" | "SUBSTRING") {
        return if syntax_upper.contains(" FROM ") && syntax_upper.contains(" FOR ") {
            format!("{name}('abc' FROM 1 FOR 1)")
        } else if syntax_upper.contains(" FROM ") {
            format!("{name}('abc' FROM 1)")
        } else {
            generic_expression(name, syntax, argument_count)
        };
    }
    let expression = match name {
        "ADDDATE" if syntax_upper.contains(",DAYS)") => "ADDDATE(CURRENT_DATE, 1)",
        "ADDDATE" => "ADDDATE(CURRENT_DATE, INTERVAL 1 DAY)",
        "ASYNCHRONOUS_CONNECTION_FAILOVER_ADD_MANAGED" => "ASYNCHRONOUS_CONNECTION_FAILOVER_ADD_MANAGED('sq_missing_channel', 'GroupReplication', 'aaaaaaaa-aaaa-aaaa-aaaa-aaaaaaaaaaaa', '127.0.0.1', 3306, '', 80, 60)",
        "ASYNCHRONOUS_CONNECTION_FAILOVER_ADD_SOURCE" => "ASYNCHRONOUS_CONNECTION_FAILOVER_ADD_SOURCE('sq_missing_channel', '127.0.0.1', 3306, '', 50)",
        "ASYNCHRONOUS_CONNECTION_FAILOVER_DELETE_MANAGED" => "ASYNCHRONOUS_CONNECTION_FAILOVER_DELETE_MANAGED('sq_missing_channel', 'aaaaaaaa-aaaa-aaaa-aaaa-aaaaaaaaaaaa')",
        "ASYNCHRONOUS_CONNECTION_FAILOVER_DELETE_SOURCE" => "ASYNCHRONOUS_CONNECTION_FAILOVER_DELETE_SOURCE('sq_missing_channel', '127.0.0.1', 3306, '')",
        "CAST" if syntax_upper.contains("AT TIME ZONE") => {
            "CAST(CURRENT_TIMESTAMP AT TIME ZONE '+00:00' AS DATETIME)"
        }
        "CAST" => "CAST(NULL AS CHAR)",
        "COLUMN_ADD" => {
            let key = if syntax_upper.contains("COLUMN_NR") {
                "1"
            } else {
                "'a'"
            };
            return if argument_count > 3 {
                format!("COLUMN_ADD(COLUMN_CREATE({key}, 1), {key}, 2, {key}, 3)")
            } else {
                format!("COLUMN_ADD(COLUMN_CREATE({key}, 1), {key}, 2)")
            };
        }
        "COLUMN_CREATE" => {
            let key = if syntax_upper.contains("COLUMN_NR") {
                "1"
            } else {
                "'a'"
            };
            return if argument_count > 2 {
                format!("COLUMN_CREATE({key}, 1, {key}, 2)")
            } else {
                format!("COLUMN_CREATE({key}, 1)")
            };
        }
        "COLUMN_DELETE" => {
            return if syntax_upper.contains("COLUMN_NR") {
                "COLUMN_DELETE(COLUMN_CREATE(1, 1), 1, 2)".to_string()
            } else {
                "COLUMN_DELETE(COLUMN_CREATE('a', 1), 'a', 'b')".to_string()
            };
        }
        "COLUMN_EXISTS" => {
            return if syntax_upper.contains("COLUMN_NR") {
                "COLUMN_EXISTS(COLUMN_CREATE(1, 1), 1)".to_string()
            } else {
                "COLUMN_EXISTS(COLUMN_CREATE('a', 1), 'a')".to_string()
            };
        }
        "CONVERT" if syntax_upper.contains(" USING ") => "CONVERT(NULL USING utf8mb4)",
        "CONVERT" => "CONVERT(NULL, CHAR)",
        "DATE_ADD" => "DATE_ADD(CURRENT_DATE, INTERVAL 1 DAY)",
        "DATE_SUB" => "DATE_SUB(CURRENT_DATE, INTERVAL 1 DAY)",
        "EXTRACT" => "EXTRACT(YEAR FROM CURRENT_DATE)",
        "GEOMCOLLECTION" | "GEOMETRYCOLLECTION" => {
            return if argument_count > 1 {
                format!("{name}(POINT(0, 0), POINT(1, 1))")
            } else {
                format!("{name}(POINT(0, 0))")
            };
        }
        "GET_FORMAT" => "GET_FORMAT(DATE, 'USA')",
        "JSON_VALUE" => "JSON_VALUE('{}', '$')",
        "LAG" | "LEAD" => match argument_count {
            0 | 1 => return format!("{name}(NULL)"),
            2 => return format!("{name}(NULL,1)"),
            _ => return format!("{name}(NULL,1,NULL)"),
        },
        "HEX" if syntax_upper.contains("STR") => "HEX('a')",
        "HEX" => "HEX(1)",
        "LINESTRING" => {
            return if argument_count > 1 {
                "LINESTRING(POINT(0, 0), POINT(1, 1))".to_string()
            } else {
                "LINESTRING(POINT(0, 0))".to_string()
            };
        }
        "MULTILINESTRING" => return if argument_count > 1 {
            "MULTILINESTRING(LINESTRING(POINT(0, 0), POINT(1, 1)), LINESTRING(POINT(2, 2), POINT(3, 3)))".to_string()
        } else {
            "MULTILINESTRING(LINESTRING(POINT(0, 0), POINT(1, 1)))".to_string()
        },
        "MULTIPOINT" => return if argument_count > 1 {
            "MULTIPOINT(POINT(0, 0), POINT(1, 1))".to_string()
        } else {
            "MULTIPOINT(POINT(0, 0))".to_string()
        },
        "MULTIPOLYGON" => return if argument_count > 1 {
            "MULTIPOLYGON(POLYGON(LINESTRING(POINT(0, 0), POINT(2, 0), POINT(0, 2), POINT(0, 0))), POLYGON(LINESTRING(POINT(3, 3), POINT(5, 3), POINT(3, 5), POINT(3, 3))))".to_string()
        } else {
            "MULTIPOLYGON(POLYGON(LINESTRING(POINT(0, 0), POINT(2, 0), POINT(0, 2), POINT(0, 0))))".to_string()
        },
        "NAME_CONST" => "NAME_CONST('sq_name', 1)",
        "POLYGON" => return if argument_count > 1 {
            "POLYGON(LINESTRING(POINT(0, 0), POINT(4, 0), POINT(0, 4), POINT(0, 0)), LINESTRING(POINT(1, 1), POINT(2, 1), POINT(1, 2), POINT(1, 1)))".to_string()
        } else {
            "POLYGON(LINESTRING(POINT(0, 0), POINT(4, 0), POINT(0, 4), POINT(0, 0)))".to_string()
        },
        "POSITION" => "POSITION('a' IN 'a')",
        "SUBDATE" if syntax_upper.contains(",DAYS)") => "SUBDATE(CURRENT_DATE, 1)",
        "SUBDATE" => "SUBDATE(CURRENT_DATE, INTERVAL 1 DAY)",
        "TIMESTAMPADD" => "TIMESTAMPADD(DAY, 1, CURRENT_DATE)",
        "TIMESTAMPDIFF" => "TIMESTAMPDIFF(DAY, CURRENT_DATE, CURRENT_DATE)",
        "TO_CHAR" if argument_count > 1 => "TO_CHAR(CURRENT_DATE, 'YYYY-MM-DD')",
        "TO_CHAR" => "TO_CHAR(CURRENT_DATE)",
        "TRIM" if argument_count > 1 && syntax_upper.contains("BOTH") => {
            "TRIM(BOTH 'x' FROM ' x ')"
        }
        "TRIM" if argument_count > 1 && syntax_upper.contains("REMSTR FROM") => {
            "TRIM('x' FROM ' x ')"
        }
        "TRIM" => "TRIM(' x ')",
        "WEIGHT_STRING" => "WEIGHT_STRING('a')",
        _ => return generic_expression(name, syntax, argument_count),
    };
    expression.to_string()
}

fn mysql_live_sql(
    db_type: DatabaseType,
    name: &str,
    syntax: &str,
    argument_count: usize,
    sequence_name: Option<&str>,
) -> String {
    let statement = match (db_type, name) {
        (DatabaseType::MySQL, "CAST") if syntax.contains("AT TIME ZONE") => "SELECT CAST(sq_timestamp AT TIME ZONE '+00:00' AS DATETIME) FROM sq_builtin_signature_probe WHERE 0".to_string(),
        (_, "DEFAULT") => {
            "SELECT DEFAULT(sq_default) FROM sq_builtin_signature_probe WHERE 0".to_string()
        }
        (DatabaseType::MySQL, "GROUPING") if argument_count > 1 => "SELECT GROUPING(sq_id, sq_default) FROM sq_builtin_signature_probe WHERE 0 GROUP BY sq_id, sq_default WITH ROLLUP".to_string(),
        (DatabaseType::MySQL, "GROUPING") => "SELECT GROUPING(sq_default) FROM sq_builtin_signature_probe WHERE 0 GROUP BY sq_default WITH ROLLUP".to_string(),
        (DatabaseType::MySQL, "JSON_TABLE") => "SELECT * FROM JSON_TABLE('[1]', '$[*]' COLUMNS (v INT PATH '$')) AS jt WHERE 0".to_string(),
        (DatabaseType::MySQL, "MATCH") => "SELECT MATCH(sq_text) AGAINST ('x') FROM sq_builtin_signature_probe WHERE 0".to_string(),
        (DatabaseType::MySQL, "NTH_VALUE") => "SELECT NTH_VALUE(sq_default, 1) OVER (ORDER BY sq_default) FROM sq_builtin_signature_probe WHERE 0".to_string(),
        (DatabaseType::MySQL, "NTILE") => "SELECT NTILE(1) OVER (ORDER BY sq_default) FROM sq_builtin_signature_probe WHERE 0".to_string(),
        (DatabaseType::MySQL, "ROW") if argument_count > 2 => "SELECT 1 FROM sq_builtin_signature_probe WHERE ROW(1, 2, 3) = ROW(1, 2, 3) AND 0".to_string(),
        (DatabaseType::MySQL, "ROW") => "SELECT 1 FROM sq_builtin_signature_probe WHERE ROW(1, 2) = ROW(1, 2) AND 0".to_string(),
        (DatabaseType::MySQL, "VALUES") => "INSERT INTO sq_builtin_signature_probe (sq_id, sq_default) VALUES (1, 1) ON DUPLICATE KEY UPDATE sq_default = VALUES(sq_default)".to_string(),
        (DatabaseType::MariaDB, "COLUMN_GET") if syntax.to_ascii_uppercase().contains("COLUMN_NR") => "SELECT COLUMN_GET(COLUMN_CREATE(1, 1), 1 AS INTEGER) FROM sq_builtin_signature_probe WHERE 0".to_string(),
        (DatabaseType::MariaDB, "COLUMN_GET") => "SELECT COLUMN_GET(COLUMN_CREATE('a', 1), 'a' AS INTEGER) FROM sq_builtin_signature_probe WHERE 0".to_string(),
        (DatabaseType::MariaDB, "DECODE") if syntax.contains("search_expr") => {
            if argument_count > 3 {
                "SELECT DECODE(NULL, NULL, 1, 2, 3)".to_string()
            } else {
                "SELECT DECODE(NULL, NULL, 1)".to_string()
            }
        }
        (DatabaseType::MariaDB, "CUME_DIST") => "SELECT CUME_DIST() OVER (ORDER BY sq_default) FROM sq_builtin_signature_probe WHERE 0".to_string(),
        (DatabaseType::MariaDB, "DENSE_RANK") => "SELECT DENSE_RANK() OVER (ORDER BY sq_default) FROM sq_builtin_signature_probe WHERE 0".to_string(),
        (DatabaseType::MariaDB, "JSON_TABLE") => "SELECT * FROM JSON_TABLE('[1]', '$[*]' COLUMNS (v INT PATH '$')) AS jt WHERE 0".to_string(),
        (DatabaseType::MariaDB, "LASTVAL") => format!(
            "SELECT LASTVAL({})",
            sequence_name.expect("MariaDB sequence probe name")
        ),
        (DatabaseType::MariaDB, "MEDIAN") => "SELECT MEDIAN(sq_default) OVER () FROM sq_builtin_signature_probe WHERE 0".to_string(),
        (DatabaseType::MariaDB, "NEXTVAL") => format!(
            "SELECT NEXTVAL({})",
            sequence_name.expect("MariaDB sequence probe name")
        ),
        (DatabaseType::MariaDB, "PERCENTILE_CONT") => "SELECT PERCENTILE_CONT(0.5) WITHIN GROUP (ORDER BY sq_default) OVER () FROM sq_builtin_signature_probe WHERE 0".to_string(),
        (DatabaseType::MariaDB, "PERCENTILE_DISC") => "SELECT PERCENTILE_DISC(0.5) WITHIN GROUP (ORDER BY sq_default) OVER () FROM sq_builtin_signature_probe WHERE 0".to_string(),
        (DatabaseType::MariaDB, "PERCENT_RANK") => "SELECT PERCENT_RANK() OVER (ORDER BY sq_default) FROM sq_builtin_signature_probe WHERE 0".to_string(),
        (DatabaseType::MariaDB, "RANK") => "SELECT RANK() OVER (ORDER BY sq_default) FROM sq_builtin_signature_probe WHERE 0".to_string(),
        (DatabaseType::MariaDB, "SETVAL") => {
            let arguments = match argument_count {
                0..=2 => "1",
                3 => "1, 1",
                _ => "1, 1, 0",
            };
            format!(
                "SELECT SETVAL({}, {arguments})",
                sequence_name.expect("MariaDB sequence probe name")
            )
        }
        _ => {
            let mut expression = mysql_expression(name, syntax, argument_count);
            let requires_window_clause = matches!(
                name,
                "CUME_DIST"
                    | "DENSE_RANK"
                    | "FIRST_VALUE"
                    | "LAG"
                    | "LAST_VALUE"
                    | "LEAD"
                    | "NTH_VALUE"
                    | "NTILE"
                    | "PERCENT_RANK"
                    | "RANK"
                    | "ROW_NUMBER"
            ) && !matches!((db_type, name), (DatabaseType::MariaDB, "LAST_VALUE"))
                || matches!((db_type, name), (DatabaseType::MariaDB, "LAST_VALUE"))
                    && syntax.to_ascii_uppercase().contains(" OVER");
            if requires_window_clause && !expression.to_ascii_uppercase().contains(" OVER ")
            {
                expression.push_str(" OVER (ORDER BY NULL)");
            }
            format!("SELECT {expression} FROM (SELECT 1) AS sq_builtin_probe WHERE 0")
        }
    };
    statement
}

fn mysql_connection_from_env() -> mysql::Conn {
    let host = std::env::var("SPACE_QUERY_TEST_MYSQL_HOST")
        .expect("SPACE_QUERY_TEST_MYSQL_HOST must be set");
    let database = std::env::var("SPACE_QUERY_TEST_MYSQL_DATABASE")
        .expect("SPACE_QUERY_TEST_MYSQL_DATABASE must be set");
    let user = std::env::var("SPACE_QUERY_TEST_MYSQL_USER")
        .expect("SPACE_QUERY_TEST_MYSQL_USER must be set");
    let password = std::env::var("SPACE_QUERY_TEST_MYSQL_PASSWORD")
        .expect("SPACE_QUERY_TEST_MYSQL_PASSWORD must be set");
    let port = std::env::var("SPACE_QUERY_TEST_MYSQL_PORT")
        .ok()
        .and_then(|value| value.parse::<u16>().ok())
        .unwrap_or(3306);
    let opts = mysql::OptsBuilder::new()
        .ip_or_hostname(Some(host))
        .tcp_port(port)
        .user(Some(user))
        .pass(Some(password))
        .db_name(Some(database))
        .prefer_socket(false);
    mysql::Conn::new(opts).expect("connect to MySQL-family live test database")
}

fn run_mysql_catalog_live(db_type: DatabaseType, names: &[&str], expected_len: usize) {
    assert_catalog(db_type, names, expected_len);
    let mut conn = mysql_connection_from_env();
    conn.query_drop("DROP TEMPORARY TABLE IF EXISTS sq_builtin_signature_probe")
        .expect("drop stale temporary built-in probe table");
    conn.query_drop("CREATE TEMPORARY TABLE sq_builtin_signature_probe (sq_id INT PRIMARY KEY, sq_default INT DEFAULT 0, sq_timestamp TIMESTAMP NULL, sq_text TEXT, FULLTEXT KEY (sq_text)) ENGINE=MyISAM")
        .expect("create temporary built-in probe table");

    let sequence_name = if db_type == DatabaseType::MariaDB {
        let connection_id = conn
            .query_first::<u64, _>("SELECT CONNECTION_ID()")
            .expect("read MariaDB connection id")
            .expect("MariaDB connection id");
        let name = format!("sq_builtin_signature_probe_seq_{connection_id}");
        conn.query_drop(format!("CREATE SEQUENCE {name}"))
            .expect("create isolated MariaDB sequence probe");
        Some(name)
    } else {
        None
    };

    let mut failures = Vec::new();
    for name in names {
        let label = builtin_signature_label(db_type, name).expect("catalog label");
        let mut overload_probes = Vec::new();
        for (overload_index, syntax) in builtin_signature_syntaxes(db_type, name)
            .expect("catalog syntaxes")
            .iter()
            .enumerate()
        {
            let required = label.overloads[overload_index].required_args;
            let mut boundary_probes = Vec::new();
            for (boundary, argument_count) in
                argument_boundaries(required, label.overloads[overload_index].arg_spans.len())
            {
                let sql = mysql_live_sql(
                    db_type,
                    name,
                    syntax,
                    argument_count,
                    sequence_name.as_deref(),
                );
                boundary_probes.push((boundary, argument_count, sql.clone()));
                let previous_sql_mode = if db_type == DatabaseType::MariaDB
                    && *name == "DECODE"
                    && syntax.contains("search_expr")
                {
                    let previous = conn
                        .query_first::<String, _>("SELECT @@SESSION.sql_mode")
                        .expect("read MariaDB sql_mode")
                        .unwrap_or_default();
                    conn.query_drop("SET SESSION sql_mode='ORACLE'")
                        .expect("enable MariaDB Oracle mode for DECODE overload");
                    Some(previous)
                } else {
                    None
                };
                let result = MysqlExecutor::execute_for_db_type(&mut conn, &sql, db_type);
                if let Some(previous) = previous_sql_mode {
                    conn.exec_drop("SET SESSION sql_mode = ?", (previous,))
                        .expect("restore MariaDB sql_mode after DECODE overload");
                }
                if let Err(error) = result {
                    let message = error.to_string();
                    let expected_environment_error = match (db_type, *name) {
                        (DatabaseType::MySQL, name) if name.starts_with("GROUP_REPLICATION_") => {
                            message.contains("1305") || message.contains("does not exist")
                        }
                        (DatabaseType::MariaDB, "BINLOG_GTID_POS") => {
                            message.contains("1381") || message.contains("binary logging")
                        }
                        (DatabaseType::MariaDB, "VEC_DISTANCE") => {
                            message.contains("4206") || message.contains("index is not found")
                        }
                        _ => false,
                    };
                    if !expected_environment_error {
                        failures.push(format!(
                            "{name} overload {} {boundary:?}({argument_count}) `{syntax}`: {message}\n  {sql}",
                            overload_index + 1
                        ));
                    }
                }
            }
            record_overload_probe_coverage(
                &mut failures,
                name,
                overload_index,
                syntax,
                &boundary_probes,
                &mut overload_probes,
            );
        }
    }

    if let Some(name) = sequence_name {
        conn.query_drop(format!("DROP SEQUENCE {name}"))
            .expect("drop isolated MariaDB sequence probe");
    }
    conn.query_drop("DROP TEMPORARY TABLE sq_builtin_signature_probe")
        .expect("drop temporary built-in probe table");
    assert!(
        failures.is_empty(),
        "{} built-in signature live failures:\n{}",
        db_type.display_name(),
        failures.join("\n")
    );
}

fn oracle_expression(name: &str, syntax: &str, argument_count: usize) -> String {
    let syntax_upper = syntax.to_ascii_uppercase();
    let expression = match name {
        "APPENDCHILDXML" if argument_count > 3 => {
            "APPENDCHILDXML(XMLTYPE('<a/>'), '/a', XMLTYPE('<b/>'), 'xmlns=\"urn:sq\"')"
        }
        "APPENDCHILDXML" => "APPENDCHILDXML(XMLTYPE('<a/>'), '/a', XMLTYPE('<b/>'))",
        "APPROX_MEDIAN" if argument_count > 1 => {
            "APPROX_MEDIAN(1 DETERMINISTIC, 'ERROR_RATE')"
        }
        "APPROX_MEDIAN" => "APPROX_MEDIAN(1)",
        "APPROX_PERCENTILE" if argument_count > 1 => {
            "APPROX_PERCENTILE(0.5 DETERMINISTIC, 'ERROR_RATE') WITHIN GROUP (ORDER BY 1)"
        }
        "APPROX_PERCENTILE" => "APPROX_PERCENTILE(0.5) WITHIN GROUP (ORDER BY 1)",
        "APPROX_PERCENTILE_DETAIL" => "APPROX_PERCENTILE_DETAIL(1)",
        "BITMAP_BIT_POSITION" => "BITMAP_BIT_POSITION(1)",
        "BITMAP_BUCKET_NUMBER" => "BITMAP_BUCKET_NUMBER(1)",
        "BITMAP_CONSTRUCT_AGG" => "BITMAP_CONSTRUCT_AGG(1)",
        "CAST" if argument_count > 1 => {
            "CAST('2026-07-21' AS DATE, 'YYYY-MM-DD', 'NLS_DATE_LANGUAGE=American')"
        }
        "CAST" => "CAST(NULL AS VARCHAR2(1))",
        "CEIL" | "FLOOR" if syntax_upper.contains("DATETIMES") => {
            return if argument_count > 1 {
                format!("{name}(DATE '2026-07-21', 'MM')")
            } else {
                format!("{name}(DATE '2026-07-21')")
            };
        }
        "CEIL" | "FLOOR" if syntax_upper.contains("INTERVAL") => {
            return if argument_count > 1 {
                format!("{name}(INTERVAL '+4 12:42:10.222' DAY(2) TO SECOND(3), 'DD')")
            } else {
                format!("{name}(INTERVAL '+4 12:42:10.222' DAY(2) TO SECOND(3))")
            };
        }
        "COALESCE" if argument_count > 2 => "COALESCE(NULL, NULL, NULL)",
        "COALESCE" => "COALESCE(NULL, NULL)",
        "COLLECT" => "COLLECT(CAST(NULL AS NUMBER))",
        "CONVERT" if argument_count > 2 => "CONVERT('a', 'AL32UTF8', 'AL32UTF8')",
        "CONVERT" => "CONVERT('a', 'AL32UTF8')",
        "CUME_DIST" if syntax_upper.contains("WITHIN GROUP") => {
            return if argument_count > 1 {
                "CUME_DIST(1, 1) WITHIN GROUP (ORDER BY 1, 1)".to_string()
            } else {
                "CUME_DIST(1) WITHIN GROUP (ORDER BY 1)".to_string()
            };
        }
        "CUME_DIST" => "CUME_DIST() OVER (ORDER BY NULL)",
        "CURRENT_DATE" => "CURRENT_DATE",
        "CURRENT_TIMESTAMP" if argument_count > 0 => "CURRENT_TIMESTAMP(0)",
        "CURRENT_TIMESTAMP" => "CURRENT_TIMESTAMP",
        "DBTIMEZONE" => "DBTIMEZONE",
        "DELETEXML" if argument_count > 2 => {
            "DELETEXML(XMLTYPE('<a xmlns=\"urn:sq\"/>'), '/n:a', 'xmlns:n=\"urn:sq\"')"
        }
        "DELETEXML" => "DELETEXML(XMLTYPE('<a/>'), '/a')",
        "DENSE_RANK" if syntax_upper.contains("WITHIN GROUP") => {
            return if argument_count > 1 {
                "DENSE_RANK(1, 1) WITHIN GROUP (ORDER BY 1, 1)".to_string()
            } else {
                "DENSE_RANK(1) WITHIN GROUP (ORDER BY 1)".to_string()
            };
        }
        "DENSE_RANK" => "DENSE_RANK() OVER (ORDER BY NULL)",
        "DOMAIN_CHECK" if argument_count > 2 => "DOMAIN_CHECK(not_a_domain, 1, 2)",
        "DOMAIN_CHECK" => "DOMAIN_CHECK(not_a_domain, 1)",
        "DOMAIN_CHECK_TYPE" if argument_count > 2 => {
            "DOMAIN_CHECK_TYPE(not_a_domain, 1, 2)"
        }
        "DOMAIN_CHECK_TYPE" => "DOMAIN_CHECK_TYPE(not_a_domain, 1)",
        "EXISTSNODE" if argument_count > 2 => {
            "EXISTSNODE(XMLTYPE('<a xmlns=\"urn:sq\"/>'), '/n:a', 'xmlns:n=\"urn:sq\"')"
        }
        "EXISTSNODE" => "EXISTSNODE(XMLTYPE('<a/>'), '/a')",
        "EXTRACT" if syntax_upper.contains("XMLTYPE_INSTANCE") && argument_count > 2 => {
            "EXTRACT(XMLTYPE('<a xmlns=\"urn:sq\"/>'), '/n:a', 'xmlns:n=\"urn:sq\"')"
        }
        "EXTRACT" if syntax_upper.contains("XMLTYPE_INSTANCE") => {
            "EXTRACT(XMLTYPE('<a/>'), '/a')"
        }
        "EXTRACT" => "EXTRACT(YEAR FROM CURRENT_DATE)",
        "EXTRACTVALUE" if argument_count > 2 => {
            "EXTRACTVALUE(XMLTYPE('<a xmlns=\"urn:sq\">1</a>'), '/n:a', 'xmlns:n=\"urn:sq\"')"
        }
        "EXTRACTVALUE" => "EXTRACTVALUE(XMLTYPE('<a>1</a>'), '/a')",
        "FIRST_VALUE" if syntax_upper.contains("EXPR [ { RESPECT") => {
            "FIRST_VALUE(1 IGNORE NULLS) OVER (ORDER BY NULL)"
        }
        "FIRST_VALUE" => "FIRST_VALUE(1) IGNORE NULLS OVER (ORDER BY NULL)",
        "FROM_TZ" => "FROM_TZ(TIMESTAMP '2026-01-01 00:00:00', '+00:00')",
        "INSERTCHILDXML" if argument_count > 4 => "INSERTCHILDXML(XMLTYPE('<a xmlns=\"urn:sq\"/>'), '/n:a', 'b', XMLTYPE('<b/>'), 'xmlns:n=\"urn:sq\"')",
        "INSERTCHILDXML" => "INSERTCHILDXML(XMLTYPE('<a/>'), '/a', 'b', XMLTYPE('<b/>'))",
        "INSERTCHILDXMLAFTER" if argument_count > 4 => {
            "INSERTCHILDXMLAFTER(XMLTYPE('<a xmlns=\"urn:sq\"><b/></a>'), '/n:a/b', 'c', XMLTYPE('<c/>'), 'xmlns:n=\"urn:sq\"')"
        }
        "INSERTCHILDXMLAFTER" => {
            "INSERTCHILDXMLAFTER(XMLTYPE('<a><b/></a>'), '/a/b', 'c', XMLTYPE('<c/>'))"
        }
        "INSERTCHILDXMLBEFORE" if argument_count > 4 => {
            "INSERTCHILDXMLBEFORE(XMLTYPE('<a xmlns=\"urn:sq\"><b/></a>'), '/n:a/b', 'c', XMLTYPE('<c/>'), 'xmlns:n=\"urn:sq\"')"
        }
        "INSERTCHILDXMLBEFORE" => {
            "INSERTCHILDXMLBEFORE(XMLTYPE('<a><b/></a>'), '/a/b', 'c', XMLTYPE('<c/>'))"
        }
        "INSERTXMLBEFORE" if argument_count > 3 => "INSERTXMLBEFORE(XMLTYPE('<a xmlns=\"urn:sq\"><b/></a>'), '/n:a/b', XMLTYPE('<c/>'), 'xmlns:n=\"urn:sq\"')",
        "INSERTXMLBEFORE" => "INSERTXMLBEFORE(XMLTYPE('<a><b/></a>'), '/a/b', XMLTYPE('<c/>'))",
        "JSON_EXISTS" if argument_count > 2 => {
            "JSON_EXISTS('{\"a\":1}', '$?(@.a == $v)' PASSING 1 AS \"v\")"
        }
        "JSON_EXISTS" => "JSON_EXISTS('{}', '$')",
        "JSON_ARRAY" if syntax_upper.starts_with("JSON [") => "JSON [1]",
        "JSON_OBJECT" if syntax_upper.starts_with("JSON {") => "JSON {'a': 1}",
        "JSON_OBJECT" => "JSON_OBJECT('a' VALUE 1)",
        "JSON_OBJECTAGG" => "JSON_OBJECTAGG('a' VALUE 1)",
        "JSON_QUERY" => "JSON_QUERY('{}', '$')",
        "JSON_TRANSFORM" if argument_count > 2 => {
            "JSON_TRANSFORM('{}', SET '$.a' = 1, SET '$.b' = 2)"
        }
        "JSON_TRANSFORM" => "JSON_TRANSFORM('{}', SET '$.a' = 1)",
        "JSON_VALUE" if argument_count > 1 => "JSON_VALUE('{}', '$')",
        "JSON_VALUE" => "JSON_VALUE('{}')",
        "LAG" if syntax_upper.contains("VALUE_EXPR [ { RESPECT") => match argument_count {
            0 | 1 => "LAG(1 IGNORE NULLS) OVER (ORDER BY NULL)",
            2 => "LAG(1 IGNORE NULLS, 1) OVER (ORDER BY NULL)",
            _ => "LAG(1 IGNORE NULLS, 1, 0) OVER (ORDER BY NULL)",
        },
        "LAG" => match argument_count {
            0 | 1 => "LAG(1) IGNORE NULLS OVER (ORDER BY NULL)",
            2 => "LAG(1, 1) IGNORE NULLS OVER (ORDER BY NULL)",
            _ => "LAG(1, 1, 0) IGNORE NULLS OVER (ORDER BY NULL)",
        },
        "LAST_VALUE" if syntax_upper.contains("EXPR [ { RESPECT") => {
            "LAST_VALUE(1 IGNORE NULLS) OVER (ORDER BY NULL)"
        }
        "LAST_VALUE" => "LAST_VALUE(1) IGNORE NULLS OVER (ORDER BY NULL)",
        "LEAD" if syntax_upper.contains("VALUE_EXPR [ { RESPECT") => match argument_count {
            0 | 1 => "LEAD(1 IGNORE NULLS) OVER (ORDER BY NULL)",
            2 => "LEAD(1 IGNORE NULLS, 1) OVER (ORDER BY NULL)",
            _ => "LEAD(1 IGNORE NULLS, 1, 0) OVER (ORDER BY NULL)",
        },
        "LEAD" => match argument_count {
            0 | 1 => "LEAD(1) IGNORE NULLS OVER (ORDER BY NULL)",
            2 => "LEAD(1, 1) IGNORE NULLS OVER (ORDER BY NULL)",
            _ => "LEAD(1, 1, 0) IGNORE NULLS OVER (ORDER BY NULL)",
        },
        "LOCALTIMESTAMP" if argument_count > 0 => "LOCALTIMESTAMP(0)",
        "LOCALTIMESTAMP" => "LOCALTIMESTAMP",
        "MEDIAN" => "MEDIAN(1)",
        "NTH_VALUE" => "NTH_VALUE(1, 1) OVER (ORDER BY NULL)",
        "NTILE" => "NTILE(1) OVER (ORDER BY NULL)",
        "NULLIF" => "NULLIF(1, 1)",
        "ODCINUMBERLIST" if argument_count > 1 => "SYS.ODCINUMBERLIST(1, 2)",
        "ODCINUMBERLIST" => "SYS.ODCINUMBERLIST(1)",
        "ORA_INVOKING_USER" => "ORA_INVOKING_USER",
        "ORA_INVOKING_USERID" => "ORA_INVOKING_USERID",
        "PERCENTILE_CONT" => "PERCENTILE_CONT(0.5) WITHIN GROUP (ORDER BY 1)",
        "PERCENTILE_DISC" => "PERCENTILE_DISC(0.5) WITHIN GROUP (ORDER BY 1)",
        "PERCENT_RANK" if syntax_upper.contains("WITHIN GROUP") => {
            return if argument_count > 1 {
                "PERCENT_RANK(1, 1) WITHIN GROUP (ORDER BY 1, 1)".to_string()
            } else {
                "PERCENT_RANK(1) WITHIN GROUP (ORDER BY 1)".to_string()
            };
        }
        "PERCENT_RANK" => "PERCENT_RANK() OVER (ORDER BY NULL)",
        "RANK" if syntax_upper.contains("WITHIN GROUP") && argument_count > 1 => {
            "RANK(1, 1) WITHIN GROUP (ORDER BY 1, 1)"
        }
        "RANK" if syntax_upper.contains("WITHIN GROUP") => "RANK(1) WITHIN GROUP (ORDER BY 1)",
        "RANK" => "RANK() OVER (ORDER BY NULL)",
        "RATIO_TO_REPORT" => "RATIO_TO_REPORT(1) OVER ()",
        "ROW_NUMBER" => "ROW_NUMBER() OVER (ORDER BY NULL)",
        "ROUND" | "TRUNC" if syntax_upper.contains("INTERVAL") => {
            return if argument_count > 1 {
                format!("{name}(INTERVAL '+4 12:42:10' DAY TO SECOND, 'DD')")
            } else {
                format!("{name}(INTERVAL '+4 12:42:10' DAY TO SECOND)")
            };
        }
        "ROUND" | "TRUNC" if syntax_upper.contains("DATE") => {
            return if argument_count > 1 {
                format!("{name}(DATE '2026-07-21', 'MM')")
            } else {
                format!("{name}(DATE '2026-07-21')")
            };
        }
        "ROUND" | "TRUNC" => {
            return if argument_count > 1 {
                format!("{name}(1.25, 1)")
            } else {
                format!("{name}(1.25)")
            };
        }
        "SESSIONTIMEZONE" => "SESSIONTIMEZONE",
        "SKEWNESS_POP" => "SKEWNESS_POP(1)",
        "SKEWNESS_SAMP" => "SKEWNESS_SAMP(1)",
        "SYSDATE" => "SYSDATE",
        "SYS_CONTEXT" if argument_count > 2 => {
            "SYS_CONTEXT('USERENV', 'SESSION_USER', 128)"
        }
        "SYS_CONTEXT" => "SYS_CONTEXT('USERENV', 'SESSION_USER')",
        "SYS_DBURIGEN" if argument_count > 2 => "SYS_DBURIGEN(dummy, dummy, 'text()')",
        "SYS_DBURIGEN" => "SYS_DBURIGEN(dummy)",
        "SYS_EXTRACT_UTC" => "SYS_EXTRACT_UTC(TIMESTAMP '2026-01-01 00:00:00 +00:00')",
        "SYSTIMESTAMP" => "SYSTIMESTAMP",
        "STANDARD_HASH" if argument_count > 1 => "STANDARD_HASH(NULL, 'SHA256')",
        "TIME_BUCKET" => {
            return if argument_count > 3 {
                "TIME_BUCKET(DATE '2026-07-21', INTERVAL '1' DAY, DATE '2026-01-01', START)"
                    .to_string()
            } else {
                "TIME_BUCKET(DATE '2026-07-21', INTERVAL '1' DAY, DATE '2026-01-01')".to_string()
            };
        }
        "TO_BLOB" if syntax_upper.contains("BFILE") => {
            return if argument_count > 1 {
                "TO_BLOB(BFILENAME('SQ_MISSING_DIR', 'missing'), 'application/octet-stream')"
                    .to_string()
            } else {
                "TO_BLOB(BFILENAME('SQ_MISSING_DIR', 'missing'))".to_string()
            };
        }
        "TO_BLOB" => "TO_BLOB(HEXTORAW('00'))",
        "TO_CLOB" if syntax_upper.contains("BFILE") => {
            return match argument_count {
                0 | 1 => "TO_CLOB(EMPTY_BLOB())".to_string(),
                2 => "TO_CLOB(EMPTY_BLOB(), 0)".to_string(),
                _ => "TO_CLOB(EMPTY_BLOB(), 0, 'text/plain')".to_string(),
            };
        }
        "TO_CLOB" => "TO_CLOB('x')",
        "TO_CHAR" if syntax_upper.contains("BFILE") => {
            return if argument_count > 1 {
                "TO_CHAR(EMPTY_BLOB(), 0)".to_string()
            } else {
                "TO_CHAR(EMPTY_BLOB())".to_string()
            };
        }
        "TO_CHAR" if syntax_upper.contains("NCHAR") => "TO_CHAR(N'x')",
        "TO_CHAR" if syntax_upper.contains("DATETIME") => match argument_count {
            0 | 1 => "TO_CHAR(DATE '2026-07-21')",
            2 => "TO_CHAR(DATE '2026-07-21', 'YYYY-MM-DD')",
            _ => "TO_CHAR(DATE '2026-07-21', 'YYYY-MM-DD', 'NLS_DATE_LANGUAGE=American')",
        },
        "TO_CHAR" => match argument_count {
            0 | 1 => "TO_CHAR(1)",
            2 => "TO_CHAR(1, 'FM9990')",
            _ => "TO_CHAR(1, 'FM9990', 'NLS_NUMERIC_CHARACTERS=''.,''')",
        },
        "TO_NCHAR" if syntax_upper.contains("{CHAR | CLOB") => "TO_NCHAR(N'x')",
        "TO_NCHAR" if syntax_upper.contains("DATETIME") => match argument_count {
            0 | 1 => "TO_NCHAR(DATE '2026-07-21')",
            2 => "TO_NCHAR(DATE '2026-07-21', 'YYYY-MM-DD')",
            _ => "TO_NCHAR(DATE '2026-07-21', 'YYYY-MM-DD', 'NLS_DATE_LANGUAGE=American')",
        },
        "TO_NCHAR" => match argument_count {
            0 | 1 => "TO_NCHAR(1)",
            2 => "TO_NCHAR(1, 'FM9990')",
            _ => "TO_NCHAR(1, 'FM9990', 'NLS_NUMERIC_CHARACTERS=''.,''')",
        },
        "TO_VECTOR" | "VECTOR" => match argument_count {
            0 | 1 => return format!("{name}('[1,2]')"),
            2 => return format!("{name}('[1,2]', 2)"),
            _ => return format!("{name}('[1,2]', 2, FLOAT32)"),
        },
        "TREAT" => "TREAT(NULL AS XMLTYPE)",
        "TRANSLATE" if syntax_upper.contains(" USING ") => "TRANSLATE('a' USING CHAR_CS)",
        "TRIM" if argument_count > 1 => "TRIM(BOTH 'x' FROM ' x ')",
        "TRIM" => "TRIM(' x ')",
        "UID" => "UID",
        "UPDATEXML" if argument_count > 3 => {
            "UPDATEXML(XMLTYPE('<a><b/></a>'), '/a', XMLTYPE('<c/>'), '/a/b', XMLTYPE('<d/>'))"
        }
        "UPDATEXML" => "UPDATEXML(XMLTYPE('<a/>'), '/a', XMLTYPE('<b/>'))",
        "USER" => "USER",
        "USERENV" => "USERENV('SESSIONID')",
        "VALIDATE_CONVERSION" if argument_count > 1 => "VALIDATE_CONVERSION('1' AS NUMBER, 'TM9', 'NLS_NUMERIC_CHARACTERS=''.,''')",
        "VALIDATE_CONVERSION" => "VALIDATE_CONVERSION('1' AS NUMBER)",
        "VECTOR_DISTANCE" => match argument_count {
            0..=2 => "VECTOR_DISTANCE(TO_VECTOR('[1,2]'), TO_VECTOR('[1,2]'))",
            _ => "VECTOR_DISTANCE(TO_VECTOR('[1,2]'), TO_VECTOR('[1,2]'), COSINE)",
        },
        "WIDTH_BUCKET" => "WIDTH_BUCKET(1, 0, 10, 5)",
        "XMLDIFF" => match argument_count {
            0..=2 => "XMLDIFF(XMLTYPE('<a/>'), XMLTYPE('<a/>'))",
            3 => "XMLDIFF(XMLTYPE('<a/>'), XMLTYPE('<a/>'), 0)",
            _ => "XMLDIFF(XMLTYPE('<a/>'), XMLTYPE('<a/>'), 0, '')",
        },
        "XMLCAST" => "XMLCAST(XMLTYPE('<a>1</a>') AS VARCHAR2(10))",
        "XMLCOLATTVAL" if argument_count > 1 => {
            "XMLCOLATTVAL(1 AS \"A\", 2 AS \"B\")"
        }
        "XMLCOLATTVAL" => "XMLCOLATTVAL(1 AS \"A\")",
        "XMLELEMENT" if argument_count > 1 => {
            "XMLELEMENT(NAME \"A\", XMLATTRIBUTES(1 AS \"X\"), 2)"
        }
        "XMLELEMENT" => "XMLELEMENT(NAME \"A\")",
        "XMLFOREST" if argument_count > 1 => "XMLFOREST(1 AS \"A\", 2 AS \"B\")",
        "XMLFOREST" => "XMLFOREST(1 AS \"A\")",
        "XMLPARSE" => "XMLPARSE(DOCUMENT '<a/>' WELLFORMED)",
        "XMLPI" if argument_count > 1 => "XMLPI(NAME \"A\", 'x')",
        "XMLPI" => "XMLPI(NAME \"A\")",
        "XMLQUERY" => "XMLQUERY('$p/a' PASSING XMLTYPE('<p><a/></p>') AS \"p\" RETURNING CONTENT)",
        "XMLROOT" => "XMLROOT(XMLTYPE('<a/>'), VERSION '1.0')",
        "XMLSERIALIZE" => "XMLSERIALIZE(DOCUMENT XMLTYPE('<a/>') AS VARCHAR2(100))",
        "XMLTYPE" if argument_count > 1 => "XMLTYPE('<a/>', NULL, 0, 1)",
        "XMLTYPE" => "XMLTYPE('<a/>')",
        _ => {
            let mut expression = generic_expression(name, syntax, argument_count);
            if syntax_upper.contains(" OVER ")
                && !expression.to_ascii_uppercase().contains(" OVER ")
            {
                expression.push_str(" OVER (ORDER BY NULL)");
            }
            return expression;
        }
    };
    expression.to_string()
}

fn oracle_live_sql(name: &str, syntax: &str, argument_count: usize) -> String {
    match name {
        "APPROX_COUNT" => "SELECT dummy, APPROX_COUNT(*) FROM dual GROUP BY dummy HAVING APPROX_RANK(ORDER BY APPROX_COUNT(*) DESC) <= 1".to_string(),
        "APPROX_RANK" => "SELECT dummy FROM dual GROUP BY dummy HAVING APPROX_RANK(ORDER BY APPROX_COUNT(*) DESC) <= 1".to_string(),
        "APPROX_SUM" if argument_count > 1 => "SELECT dummy, APPROX_SUM(1, 'MAX_ERROR') FROM dual GROUP BY dummy HAVING APPROX_RANK(ORDER BY APPROX_SUM(1, 'MAX_ERROR') DESC) <= 1".to_string(),
        "APPROX_SUM" => "SELECT dummy, APPROX_SUM(1) FROM dual GROUP BY dummy HAVING APPROX_RANK(ORDER BY APPROX_SUM(1) DESC) <= 1".to_string(),
        "GROUPING" => "SELECT GROUPING(x) FROM (SELECT 1 x FROM dual) GROUP BY ROLLUP(x)".to_string(),
        "GROUPING_ID" if argument_count > 1 => "SELECT GROUPING_ID(x, y) FROM (SELECT 1 x, 2 y FROM dual) GROUP BY ROLLUP(x, y)".to_string(),
        "GROUPING_ID" => "SELECT GROUPING_ID(x) FROM (SELECT 1 x FROM dual) GROUP BY ROLLUP(x)".to_string(),
        "GROUP_ID" => "SELECT GROUP_ID() FROM (SELECT 1 x FROM dual) GROUP BY x".to_string(),
        "EQUALS_PATH" if argument_count > 2 => "SELECT any_path FROM resource_view WHERE EQUALS_PATH(res, '/', 1) = 1 AND 1 = 0".to_string(),
        "EQUALS_PATH" => "SELECT any_path FROM resource_view WHERE EQUALS_PATH(res, '/') = 1 AND 1 = 0".to_string(),
        "UNDER_PATH" if syntax.contains(", levels,") && argument_count > 3 => "SELECT any_path FROM resource_view WHERE UNDER_PATH(res, 1, '/', 2) = 1 AND 1 = 0".to_string(),
        "UNDER_PATH" if syntax.contains(", levels,") => "SELECT any_path FROM resource_view WHERE UNDER_PATH(res, 1, '/') = 1 AND 1 = 0".to_string(),
        "UNDER_PATH" if argument_count > 2 => "SELECT any_path FROM resource_view WHERE UNDER_PATH(res, '/', 1) = 1 AND 1 = 0".to_string(),
        "UNDER_PATH" => "SELECT any_path FROM resource_view WHERE UNDER_PATH(res, '/') = 1 AND 1 = 0".to_string(),
        "CLASSIFIER" | "MATCH_NUMBER" => format!(
            "SELECT * FROM (SELECT 1 id FROM dual) MATCH_RECOGNIZE (ORDER BY id MEASURES {name}() AS sq_measure ONE ROW PER MATCH PATTERN (a) DEFINE a AS 1 = 1) WHERE 1 = 0"
        ),
        "FIRST" | "LAST" | "PREV" | "NEXT" => {
            let expression = if argument_count > 1 {
                format!("{name}(a.id, 0)")
            } else {
                format!("{name}(a.id)")
            };
            format!(
                "SELECT * FROM (SELECT 1 id FROM dual) MATCH_RECOGNIZE (ORDER BY id MEASURES {expression} AS sq_measure ONE ROW PER MATCH PATTERN (a) DEFINE a AS 1 = 1) WHERE 1 = 0"
            )
        }
        "JSON_TABLE" if argument_count > 1 => "SELECT * FROM JSON_TABLE('{}', '$' COLUMNS (x NUMBER PATH '$.x')) WHERE 1 = 0".to_string(),
        "JSON_TABLE" => "SELECT * FROM JSON_TABLE('{}' COLUMNS (x NUMBER PATH '$.x')) WHERE 1 = 0".to_string(),
        "JSON_TEXTCONTAINS" => "SELECT 1 FROM (SELECT '{}' doc FROM dual) WHERE JSON_TEXTCONTAINS(doc, '$', 'x') AND 1 = 0".to_string(),
        "SYS_ROW_ETAG" if argument_count > 1 => "SELECT SYS_ROW_ETAG(c1, c2) FROM (SELECT 1 c1, 2 c2 FROM dual) WHERE 1 = 0".to_string(),
        "SYS_ROW_ETAG" => "SELECT SYS_ROW_ETAG(c1) FROM (SELECT 1 c1 FROM dual) WHERE 1 = 0".to_string(),
        "VECTOR_CHUNKS" => "SELECT * FROM VECTOR_CHUNKS(DBMS_VECTOR_CHAIN.UTL_TO_TEXT('x')) WHERE 1 = 0".to_string(),
        "XMLATTRIBUTES" if argument_count > 1 => "SELECT XMLELEMENT(NAME \"A\", XMLATTRIBUTES(1 AS \"X\", 2 AS \"Y\")) FROM dual WHERE 1 = 0".to_string(),
        "XMLATTRIBUTES" => "SELECT XMLELEMENT(NAME \"A\", XMLATTRIBUTES(1 AS \"X\")) FROM dual WHERE 1 = 0".to_string(),
        "XMLTABLE" if argument_count > 1 => "SELECT * FROM XMLTABLE(XMLNAMESPACES(DEFAULT 'urn:sq'), '/a' PASSING XMLTYPE('<a xmlns=\"urn:sq\"/>') COLUMNS x VARCHAR2(1) PATH '.') WHERE 1 = 0".to_string(),
        "XMLTABLE" => "SELECT * FROM XMLTABLE('/a' PASSING XMLTYPE('<a/>') COLUMNS x VARCHAR2(1) PATH '.') WHERE 1 = 0".to_string(),
        _ => format!(
            "SELECT {} FROM dual WHERE 1 = 0",
            oracle_expression(name, syntax, argument_count)
        ),
    }
}

fn oracle_context_only_name(name: &str) -> bool {
    name.starts_with("CALENDAR_")
        || name.starts_with("CLUSTER_")
        || name.starts_with("FEATURE_")
        || name.starts_with("FISCAL_")
        || name.starts_with("PREDICTION")
        || name.starts_with("RETAIL_")
        || matches!(
            name,
            "DATEDIFF"
                | "ELEMENT_NUMBER"
                | "GRAPHQL"
                | "ITERATION_NUMBER"
                | "JSON_CONSTRUCTOR"
                | "MATCHNUM"
                | "ORA_CHECK_DATA_PRIVILEGE"
                | "ORA_END_USER_CONTEXT"
                | "ORA_IS_COLUMN_AUTHORIZED"
                | "PATH_NAME"
                | "TIMESTAMPDIFF"
                | "VECTOR_EMBEDDING"
        )
}

fn oracle_expected_prerequisite_error(name: &str, message: &str) -> bool {
    if message.contains("ORA-00904") {
        return oracle_context_only_name(name);
    }
    if message.contains("ORA-00903") {
        return matches!(
            name,
            "DATAOBJ_TO_MAT_PARTITION" | "DATAOBJ_TO_PARTITION" | "MAKE_REF"
        );
    }
    if message.contains("ORA-03050") {
        return matches!(name, "REF" | "SYS_OP_ZONE_ID" | "VALUE");
    }
    match name {
        "APPROX_COUNT_DISTINCT_AGG"
        | "APPROX_PERCENTILE_AGG"
        | "COLLECT"
        | "DEREF"
        | "SYS_TYPEID"
        | "TO_APPROX_COUNT_DISTINCT"
        | "TO_APPROX_PERCENTILE"
        | "TREAT"
        | "XMLISVALID" => message.contains("ORA-00932"),
        "ORA_DST_AFFECTED" | "ORA_DST_CONVERT" | "ORA_DST_ERROR" => message.contains("ORA-08186"),
        "DOMAIN_CHECK" | "DOMAIN_CHECK_TYPE" => message.contains("ORA-11504"),
        "POWERMULTISET" | "POWERMULTISET_BY_CARDINALITY" => message.contains("ORA-22957"),
        "TO_LOB" => message.contains("ORA-24856"),
        "DEPTH" | "PATH" => message.contains("ORA-29909"),
        "SYS_CONNECT_BY_PATH" => message.contains("ORA-30003"),
        "CV" | "PRESENTNNV" | "PRESENTV" | "PREVIOUS" => message.contains("ORA-32644"),
        "CUBE_TABLE" => message.contains("ORA-33262"),
        "ORA_DM_PARTITION_NAME" => message.contains("ORA-40281"),
        "JSON_TEXTCONTAINS" => message.contains("ORA-40467") || message.contains("ORA-40468"),
        "XMLSEQUENCE" => message.contains("ORA-06553"),
        _ => false,
    }
}

fn oracle_connection_from_env() -> OracleThinSession {
    let username = std::env::var("ORACLE_TEST_USERNAME").expect("ORACLE_TEST_USERNAME must be set");
    let password = std::env::var("ORACLE_TEST_PASSWORD").expect("ORACLE_TEST_PASSWORD must be set");
    let service =
        std::env::var("ORACLE_TEST_SERVICE_NAME").expect("ORACLE_TEST_SERVICE_NAME must be set");
    let host = std::env::var("ORACLE_TEST_HOST").unwrap_or_else(|_| "localhost".to_string());
    let port = std::env::var("ORACLE_TEST_PORT")
        .ok()
        .and_then(|value| value.parse::<u16>().ok())
        .unwrap_or(1521);
    let mut config = OracleThinConfig::new(
        ConnectTarget::service_name(host, port, service),
        username,
        password,
    );
    config.program = "space-query-builtin-signatures".to_string();
    config.connect_options.disable_oob_probe = true;
    OracleThinSession::connect(config).expect("connect to Oracle live test database")
}

#[test]
#[ignore = "requires local Oracle 26ai via ORACLE_TEST_* environment variables"]
fn oracle_builtin_function_signatures_execute_live() {
    assert_catalog(DatabaseType::Oracle, ORACLE_FUNCTIONS, 464);
    let mut session = oracle_connection_from_env();
    let mut failures = Vec::new();
    for name in ORACLE_FUNCTIONS {
        let label =
            builtin_signature_label(DatabaseType::Oracle, name).expect("Oracle catalog label");
        let mut overload_probes = Vec::new();
        for (overload_index, syntax) in builtin_signature_syntaxes(DatabaseType::Oracle, name)
            .expect("Oracle catalog syntaxes")
            .iter()
            .enumerate()
        {
            let required = label.overloads[overload_index].required_args;
            let mut boundary_probes = Vec::new();
            for (boundary, argument_count) in
                argument_boundaries(required, label.overloads[overload_index].arg_spans.len())
            {
                let sql = oracle_live_sql(name, syntax, argument_count);
                boundary_probes.push((boundary, argument_count, sql.clone()));
                if let Err(error) = session.query_drop(&sql) {
                    let message = error.to_string();
                    if !oracle_expected_prerequisite_error(name, &message) {
                        failures.push(format!(
                            "{name} overload {} {boundary:?}({argument_count}) `{syntax}`: {message}\n  {sql}",
                            overload_index + 1
                        ));
                    }
                }
            }
            record_overload_probe_coverage(
                &mut failures,
                name,
                overload_index,
                syntax,
                &boundary_probes,
                &mut overload_probes,
            );
        }
    }
    assert!(
        failures.is_empty(),
        "Oracle built-in signature live failures:\n{}",
        failures.join("\n")
    );
}

#[test]
#[ignore = "requires local MySQL 8.0 via SPACE_QUERY_TEST_MYSQL_* environment variables"]
fn mysql_builtin_function_signatures_execute_live() {
    run_mysql_catalog_live(DatabaseType::MySQL, MYSQL_FUNCTIONS, 408);
}

#[test]
#[ignore = "requires local MariaDB 12.2 via SPACE_QUERY_TEST_MYSQL_* environment variables"]
fn mariadb_builtin_function_signatures_execute_live() {
    run_mysql_catalog_live(DatabaseType::MariaDB, MARIADB_FUNCTIONS, 475);
}

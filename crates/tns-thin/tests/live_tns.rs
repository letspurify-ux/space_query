use std::sync::atomic::{AtomicUsize, Ordering};
use std::thread;
use std::time::{Duration, Instant};

use tns_thin::exec::{BindInputValue, BindValue, OracleColumnType, OracleValue, StatementRequest};
use tns_thin::{ConnectTarget, OracleDateTime, OracleThinConfig, OracleThinSession};

static OBJECT_COUNTER: AtomicUsize = AtomicUsize::new(1);

#[test]
#[ignore = "requires local Oracle listener via ORACLE_THIN_TEST_* environment variables"]
fn negotiated_protocol_matches_requested_protocol() {
    let conn = connect();
    if let Some(requested) = protocol_env("ORACLE_THIN_DESIRED_PROTOCOL") {
        assert_eq!(conn.capabilities().protocol_version, Some(requested));
    } else {
        assert!(
            conn.capabilities().protocol_version.is_some(),
            "thin connection should report the negotiated protocol"
        );
    }
}

#[test]
#[ignore = "requires local Oracle listener via ORACLE_THIN_TEST_* environment variables"]
fn auth_alter_session_sets_initial_time_zone() {
    let mut conn = connect();
    let expected = std::env::var("ORA_SDTZ").unwrap_or_else(|_| local_timezone_offset_string());
    let result = conn
        .query_described_fetch_all("SELECT SESSIONTIMEZONE AS tz FROM dual", 1)
        .expect("fetch session timezone after login");

    assert_eq!(rows_to_strings(&result.result.rows), vec![vec![expected]]);
}

#[test]
#[ignore = "requires local Oracle listener via ORACLE_THIN_TEST_* environment variables"]
fn fetch_and_fetch_all_return_all_rows() {
    let mut conn = connect();
    let sql = "SELECT level AS n, 'R' || TO_CHAR(level) AS label FROM dual CONNECT BY level <= 7";
    let request = StatementRequest::query(sql, 2);

    let initial = conn
        .query_described_initial_request(&request)
        .expect("initial described fetch");
    assert_eq!(
        initial
            .columns
            .iter()
            .map(|column| column.name.as_str())
            .collect::<Vec<_>>(),
        vec!["N", "LABEL"]
    );
    assert_eq!(rows_to_strings(&initial.result.rows), expected_rows(1, 2));
    assert!(
        !initial.result.exhausted,
        "initial fetch should leave rows for explicit fetch calls"
    );
    let cursor_id = initial
        .result
        .cursor_id
        .expect("initial fetch should leave an open cursor");

    let fetched = conn
        .fetch_ref_cursor_batch(cursor_id, &initial.columns, 2, false)
        .expect("fetch next batch");
    assert_eq!(rows_to_strings(&fetched.rows), expected_rows(3, 4));
    assert!(
        !fetched.exhausted,
        "second fetch should still leave rows for fetch_all"
    );

    let remaining = conn
        .fetch_ref_cursor_all(cursor_id, initial.columns.clone(), 3)
        .expect("fetch all remaining rows");
    assert!(remaining.result.exhausted);
    assert_eq!(rows_to_strings(&remaining.result.rows), expected_rows(5, 7));
}

#[test]
#[ignore = "requires local Oracle listener via ORACLE_THIN_TEST_* environment variables"]
fn large_exact_fetch_batches_complete_without_read_wait() {
    let mut conn = connect();
    conn.set_call_timeout(Some(Duration::from_secs(10)))
        .expect("set large fetch timeout");

    let direct = conn
        .query_described_fetch_all(
            "SELECT level AS n, 'R' || TO_CHAR(level) AS label \
             FROM dual CONNECT BY level <= 5000",
            500,
        )
        .expect("large exact direct fetch batches");
    assert_eq!(direct.result.rows.len(), 5000);
    assert_eq!(
        rows_to_strings(&direct.result.rows[..2]),
        expected_rows(1, 2)
    );
    assert_eq!(
        rows_to_strings(&direct.result.rows[4998..]),
        expected_rows(4999, 5000)
    );

    let mut request = StatementRequest::statement(
        "BEGIN \
         OPEN :1 FOR \
         SELECT level AS n, 'C' || TO_CHAR(level) AS label \
         FROM dual CONNECT BY level <= 1000; \
         END;",
    );
    request.binds.push(BindValue::Out {
        column_type: OracleColumnType::Cursor,
        max_len: 1,
    });
    let values = conn
        .execute_out_binds(&request, &[])
        .expect("open large exact ref cursor");
    let cursor = match values.first() {
        Some(OracleValue::Cursor(cursor)) => cursor.clone(),
        other => panic!("expected OUT REF CURSOR, got {other:?}"),
    };
    let fetched = conn
        .fetch_ref_cursor_all(cursor.cursor_id, cursor.columns, 250)
        .expect("large exact ref cursor fetch batches");

    assert_eq!(fetched.result.rows.len(), 1000);
    assert_eq!(
        rows_to_strings(&fetched.result.rows[..2]),
        vec![
            vec!["1".to_string(), "C1".to_string()],
            vec!["2".to_string(), "C2".to_string()],
        ]
    );
    assert_eq!(
        rows_to_strings(&fetched.result.rows[998..]),
        vec![
            vec!["999".to_string(), "C999".to_string()],
            vec!["1000".to_string(), "C1000".to_string()],
        ]
    );
}

#[test]
#[ignore = "requires local Oracle listener via ORACLE_THIN_TEST_* environment variables"]
fn fetch_interval_columns_decodes_vendor_formats() {
    let mut conn = connect();
    let result = conn
        .query_described_fetch_all(
            "SELECT \
             TO_YMINTERVAL('2021-10') AS ym_pos, \
             TO_YMINTERVAL('-05-03') AS ym_neg, \
             TO_DSINTERVAL('2 12:23:34.456') AS ds_pos, \
             TO_DSINTERVAL('-0 10:20:30.456789') AS ds_neg \
             FROM dual",
            1,
        )
        .expect("fetch interval columns");

    assert_eq!(
        result
            .columns
            .iter()
            .map(|column| column.column_type)
            .collect::<Vec<_>>(),
        vec![
            OracleColumnType::IntervalYearMonth,
            OracleColumnType::IntervalYearMonth,
            OracleColumnType::IntervalDaySecond,
            OracleColumnType::IntervalDaySecond,
        ]
    );
    assert_eq!(
        rows_to_strings(&result.result.rows),
        vec![vec![
            "+2021-10".to_string(),
            "-05-03".to_string(),
            "+02 12:23:34.456000".to_string(),
            "-00 10:20:30.456789".to_string(),
        ]]
    );
}

#[test]
#[ignore = "requires local Oracle listener via ORACLE_THIN_TEST_* environment variables"]
fn fetch_binary_float_and_double_decodes_vendor_formats() {
    let mut conn = connect();
    let result = conn
        .query_described_fetch_all(
            "SELECT \
             CAST(134.45 AS BINARY_FLOAT) AS bf_pos, \
             CAST(-134.45 AS BINARY_FLOAT) AS bf_neg, \
             CAST(134.45 AS BINARY_DOUBLE) AS bd_pos, \
             CAST(-134.45 AS BINARY_DOUBLE) AS bd_neg \
             FROM dual",
            1,
        )
        .expect("fetch binary float and double columns");

    assert_eq!(
        result
            .columns
            .iter()
            .map(|column| column.column_type)
            .collect::<Vec<_>>(),
        vec![
            OracleColumnType::Number,
            OracleColumnType::Number,
            OracleColumnType::Number,
            OracleColumnType::Number,
        ]
    );
    assert_eq!(
        rows_to_strings(&result.result.rows),
        vec![vec![
            "134.45".to_string(),
            "-134.45".to_string(),
            "134.45".to_string(),
            "-134.45".to_string(),
        ]]
    );
}

#[test]
#[ignore = "requires local Oracle listener via ORACLE_THIN_TEST_* environment variables"]
fn fetch_vector_columns_decode_vendor_formats() {
    let mut conn = connect();
    let result = match conn.query_described_fetch_all(
        "SELECT \
         TO_VECTOR('[34.6, 77.8]', 2, FLOAT32) AS v32, \
         TO_VECTOR('[34.6, 77.8]', 2, FLOAT64) AS v64, \
         TO_VECTOR('[34, -77]', 2, INT8) AS vi8, \
         TO_VECTOR('[3, 2, 3]', 24, BINARY) AS vb \
         FROM dual",
        1,
    ) {
        Ok(result) => result,
        Err(err)
            if err.to_string().contains("ORA-00904") || err.to_string().contains("ORA-00902") =>
        {
            eprintln!("skipping vector fetch test: database does not support VECTOR");
            return;
        }
        Err(err) => panic!("fetch vector columns: {err}"),
    };

    assert_eq!(
        result
            .columns
            .iter()
            .map(|column| column.column_type)
            .collect::<Vec<_>>(),
        vec![
            OracleColumnType::Vector,
            OracleColumnType::Vector,
            OracleColumnType::Vector,
            OracleColumnType::Vector,
        ]
    );
    assert_eq!(
        rows_to_strings(&result.result.rows),
        vec![vec![
            "[34.6, 77.8]".to_string(),
            "[34.6, 77.8]".to_string(),
            "[34, -77]".to_string(),
            "[3, 2, 3]".to_string(),
        ]]
    );
}

#[test]
#[ignore = "requires local Oracle listener via ORACLE_THIN_TEST_* environment variables"]
fn fetch_sparse_vector_columns_decode_vendor_formats() {
    let mut conn = connect();
    let result = match conn.query_described_fetch_all(
        "SELECT \
         TO_VECTOR('[16, [1, 3, 5], [1, 0, 5]]', 16, FLOAT32, SPARSE) AS sv32, \
         TO_VECTOR('[16, [1, 3, 5], [1, 0, 5]]', 16, FLOAT64, SPARSE) AS sv64, \
         TO_VECTOR('[16, [1, 3, 5], [1, 0, 5]]', 16, INT8, SPARSE) AS svi8 \
         FROM dual",
        1,
    ) {
        Ok(result) => result,
        Err(err)
            if err.to_string().contains("ORA-00904")
                || err.to_string().contains("ORA-00902")
                || err.to_string().contains("ORA-03001")
                || err.to_string().contains("ORA-518") =>
        {
            eprintln!("skipping sparse vector fetch test: database does not support sparse VECTOR");
            return;
        }
        Err(err) => panic!("fetch sparse vector columns: {err}"),
    };

    assert_eq!(
        result
            .columns
            .iter()
            .map(|column| column.column_type)
            .collect::<Vec<_>>(),
        vec![
            OracleColumnType::Vector,
            OracleColumnType::Vector,
            OracleColumnType::Vector,
        ]
    );
    assert_eq!(
        rows_to_strings(&result.result.rows),
        vec![vec![
            "SparseVector(dimensions=16, indices=[1, 3, 5], values=[1, 0, 5])".to_string(),
            "SparseVector(dimensions=16, indices=[1, 3, 5], values=[1, 0, 5])".to_string(),
            "SparseVector(dimensions=16, indices=[1, 3, 5], values=[1, 0, 5])".to_string(),
        ]]
    );
}

#[test]
#[ignore = "requires local Oracle listener via ORACLE_THIN_TEST_* environment variables"]
fn fetch_json_column_decodes_oson_payload_as_json_text() {
    let config = live_config();
    let table = unique_table_name("JSON");
    let _guard = TableDropGuard::new(config.clone(), table.clone());
    let mut conn = connect_with_config(config);

    match conn.query_drop(&format!(
        "CREATE TABLE {table} (id NUMBER PRIMARY KEY, doc JSON) TABLESPACE USERS"
    )) {
        Ok(()) => {}
        Err(err)
            if err.to_string().contains("ORA-00902")
                || err.to_string().contains("ORA-00959")
                || err.to_string().contains("ORA-03001")
                || err.to_string().contains("ORA-43853") =>
        {
            eprintln!("skipping JSON fetch test: database does not support native JSON");
            return;
        }
        Err(err) => panic!("create JSON test table: {err}"),
    }
    conn.query_drop(&format!(
        "INSERT INTO {table} (id, doc) \
         SELECT 1, JSON_OBJECT(\
             KEY 'a' VALUE 1, \
             KEY 'b' VALUE JSON_ARRAY(2, 'x'), \
             KEY 'flag' VALUE 'true' FORMAT JSON \
             RETURNING JSON\
         ) FROM dual"
    ))
    .expect("insert JSON test row");

    let result = conn
        .query_described_fetch_all(format!("SELECT doc FROM {table} WHERE id = 1"), 1)
        .expect("fetch native JSON column");
    assert_eq!(
        result
            .columns
            .iter()
            .map(|column| column.column_type)
            .collect::<Vec<_>>(),
        vec![OracleColumnType::Json]
    );
    assert_eq!(
        rows_to_strings(&result.result.rows),
        vec![vec![r#"{"a":1,"b":[2,"x"],"flag":true}"#.to_string()]]
    );
}

#[test]
#[ignore = "requires local Oracle listener via ORACLE_THIN_TEST_* environment variables"]
fn fetch_json_extended_scalars_decode_oson_payload_as_json_text() {
    let mut conn = connect();
    let result = match conn.query_described_fetch_all(
        "SELECT JSON_OBJECT(\
             KEY 'date' VALUE DATE '2024-02-29', \
             KEY 'ts' VALUE TIMESTAMP '2024-01-02 03:04:05.123456', \
             KEY 'bf' VALUE CAST(3.5 AS BINARY_FLOAT), \
             KEY 'bd' VALUE CAST(-2.25 AS BINARY_DOUBLE) \
             RETURNING JSON\
         ) AS doc FROM dual",
        1,
    ) {
        Ok(result) => result,
        Err(err)
            if err.to_string().contains("ORA-00902")
                || err.to_string().contains("ORA-03001")
                || err.to_string().contains("ORA-40449") =>
        {
            eprintln!(
                "skipping JSON extended scalar fetch test: database does not support native JSON"
            );
            return;
        }
        Err(err) => panic!("fetch JSON extended scalar document: {err}"),
    };

    assert_eq!(
        result
            .columns
            .iter()
            .map(|column| column.column_type)
            .collect::<Vec<_>>(),
        vec![OracleColumnType::Json]
    );
    assert_eq!(
        rows_to_strings(&result.result.rows),
        vec![vec![
            r#"{"date":"2024-02-29T00:00:00","ts":"2024-01-02T03:04:05.123456","bf":3.5,"bd":-2.25}"#
                .to_string()
        ]]
    );
}

#[test]
#[ignore = "requires local Oracle listener via ORACLE_THIN_TEST_* environment variables"]
fn fetch_json_extended_raw_decodes_oson_payload_as_json_text() {
    let mut conn = connect();
    let result = match conn.query_described_fetch_all(
        r#"SELECT JSON('{"short_raw":{"$rawhex":"73686f72745f726177"}}' EXTENDED) AS doc FROM dual"#,
        1,
    ) {
        Ok(result) => result,
        Err(err)
            if err.to_string().contains("ORA-00902")
                || err.to_string().contains("ORA-03001")
                || err.to_string().contains("ORA-40441")
                || err.to_string().contains("ORA-40449") =>
        {
            eprintln!(
                "skipping JSON extended raw fetch test: database does not support native JSON"
            );
            return;
        }
        Err(err) => panic!("fetch JSON extended raw document: {err}"),
    };

    assert_eq!(
        result
            .columns
            .iter()
            .map(|column| column.column_type)
            .collect::<Vec<_>>(),
        vec![OracleColumnType::Json]
    );
    let expected = if conn.capabilities().protocol_version == Some(314) {
        r#"{"short_raw":"73686F72745F726177"}"#
    } else {
        r#"{"short_raw":{"$rawhex":"73686f72745f726177"}}"#
    };
    assert_eq!(
        rows_to_strings(&result.result.rows),
        vec![vec![expected.to_string()]]
    );
}

#[test]
#[ignore = "requires local Oracle listener via ORACLE_THIN_TEST_* environment variables"]
fn fetch_json_embedded_vector_decodes_oson_payload_as_json_text() {
    let mut conn = connect();
    let result = match conn.query_described_fetch_all(
        "SELECT JSON_OBJECT(\
             KEY 'id' VALUE 6432, \
             KEY 'vector' VALUE TO_VECTOR('[1, 2, 3]') \
             RETURNING JSON\
         ) AS doc FROM dual",
        1,
    ) {
        Ok(result) => result,
        Err(err)
            if err.to_string().contains("ORA-00902")
                || err.to_string().contains("ORA-00904")
                || err.to_string().contains("ORA-03001")
                || err.to_string().contains("ORA-40449")
                || err.to_string().contains("ORA-518") =>
        {
            eprintln!(
                "skipping JSON embedded vector fetch test: database does not support JSON VECTOR"
            );
            return;
        }
        Err(err) => panic!("fetch JSON embedded vector document: {err}"),
    };

    assert_eq!(
        result
            .columns
            .iter()
            .map(|column| column.column_type)
            .collect::<Vec<_>>(),
        vec![OracleColumnType::Json]
    );
    let expected = if conn.capabilities().protocol_version == Some(314) {
        r#"{"id":6432,"vector":[1.0E+00,2.0E+00,3.0E+00]}"#
    } else {
        r#"{"id":6432,"vector":[1, 2, 3]}"#
    };
    assert_eq!(
        rows_to_strings(&result.result.rows),
        vec![vec![expected.to_string()]]
    );
}

#[test]
#[ignore = "requires local Oracle listener via ORACLE_THIN_TEST_* environment variables"]
fn fetch_xmltype_column_decodes_text_across_batches() {
    let mut conn = connect();
    let result = conn
        .query_described_fetch_all(
            "SELECT XMLTYPE('<root><n>' || level || '</n><txt>' || UNISTR('\\D55C\\AE00') || '</txt></root>') AS doc \
             FROM dual CONNECT BY level <= 3",
            1,
        )
        .expect("fetch XMLTYPE column");

    assert_eq!(
        result
            .columns
            .iter()
            .map(|column| column.column_type)
            .collect::<Vec<_>>(),
        vec![OracleColumnType::Xml]
    );
    let rows = rows_to_strings(&result.result.rows);
    assert_eq!(rows.len(), 3);
    assert!(rows[0][0].contains("<n>1</n>"));
    assert!(rows[1][0].contains("<n>2</n>"));
    assert!(rows[2][0].contains("<n>3</n>"));
    assert!(rows.iter().all(|row| row[0].contains("한글")));
}

#[test]
#[ignore = "requires local Oracle listener via ORACLE_THIN_TEST_* environment variables"]
fn plsql_json_out_bind_decodes_oson_payload_as_json_text() {
    let mut conn = connect();
    let mut request = StatementRequest::statement(
        "BEGIN \
         :1 := JSON_OBJECT(\
             KEY 'a' VALUE 1, \
             KEY 'b' VALUE JSON_ARRAY(2, 'x'), \
             KEY 'flag' VALUE 'true' FORMAT JSON \
             RETURNING JSON\
         ); \
         END;",
    );
    request.binds.push(BindValue::Out {
        column_type: OracleColumnType::Json,
        max_len: 1024,
    });

    let values = match conn.execute_out_binds(&request, &[]) {
        Ok(values) => values,
        Err(err)
            if err.to_string().contains("ORA-00902")
                || err.to_string().contains("ORA-00932")
                || err.to_string().contains("ORA-03001")
                || err.to_string().contains("ORA-06550")
                || err.to_string().contains("ORA-43853") =>
        {
            eprintln!("skipping JSON OUT bind test: database does not support native JSON bind");
            return;
        }
        Err(err) => panic!("PL/SQL JSON OUT bind: {err}"),
    };

    assert_eq!(
        rows_to_strings(&[values]),
        vec![vec![r#"{"a":1,"b":[2,"x"],"flag":true}"#.to_string()]]
    );
}

#[test]
#[ignore = "requires local Oracle listener via ORACLE_THIN_TEST_* environment variables"]
fn plsql_json_inout_bind_round_trips_native_json() {
    let mut conn = connect();
    let mut request = StatementRequest::statement(
        "BEGIN \
         :1 := JSON_OBJECT(\
             KEY 'input' VALUE JSON_VALUE(:1, '$.input'), \
             KEY 'added' VALUE 2 \
             RETURNING JSON\
         ); \
         END;",
    );
    request.binds.push(BindValue::InOut {
        column_type: OracleColumnType::Json,
        max_len: 1024,
        value: Some(BindInputValue::Text(r#"{"input":"ok"}"#.to_string())),
    });

    let values = match conn.execute_out_binds(&request, &[]) {
        Ok(values) => values,
        Err(err)
            if err.to_string().contains("ORA-00902")
                || err.to_string().contains("ORA-00932")
                || err.to_string().contains("ORA-03001")
                || err.to_string().contains("ORA-06550")
                || err.to_string().contains("ORA-43853") =>
        {
            eprintln!("skipping JSON IN OUT bind test: database does not support native JSON bind");
            return;
        }
        Err(err) => panic!("PL/SQL JSON IN OUT bind: {err}"),
    };

    assert_eq!(
        rows_to_strings(&[values]),
        vec![vec![r#"{"input":"ok","added":2}"#.to_string()]]
    );
}

#[test]
#[ignore = "requires local Oracle listener via ORACLE_THIN_TEST_* environment variables"]
fn fetch_timestamp_ltz_decodes_like_python_oracledb_timestamp() {
    let mut conn = connect();
    conn.query_drop("ALTER SESSION SET TIME_ZONE = '+00:00'")
        .expect("set deterministic session time zone");
    let result = conn
        .query_described_fetch_all(
            "SELECT \
             CAST(TIMESTAMP '2024-01-02 03:04:05.123456' \
             AS TIMESTAMP WITH LOCAL TIME ZONE) AS ts_ltz \
             FROM dual",
            1,
        )
        .expect("fetch timestamp with local time zone column");

    assert_eq!(
        result
            .columns
            .iter()
            .map(|column| column.column_type)
            .collect::<Vec<_>>(),
        vec![OracleColumnType::Timestamp]
    );
    assert_eq!(
        result
            .result
            .rows
            .first()
            .and_then(|row| row.first())
            .map(timestamp_value_to_string),
        Some("2024-01-02 03:04:05.123456".to_string())
    );
}

#[test]
#[ignore = "requires local Oracle listener via ORACLE_THIN_TEST_* environment variables"]
fn fetch_timestamp_tz_fixed_offset_decodes_offset() {
    let mut conn = connect();
    let result = conn
        .query_described_fetch_all(
            "SELECT \
             FROM_TZ(TIMESTAMP '2024-01-02 03:04:05.123456', '+05:45') AS ts_tz \
             FROM dual",
            1,
        )
        .expect("fetch timestamp with time zone column");

    assert_eq!(
        result
            .columns
            .iter()
            .map(|column| column.column_type)
            .collect::<Vec<_>>(),
        vec![OracleColumnType::Timestamp]
    );
    let value = result
        .result
        .rows
        .first()
        .and_then(|row| row.first())
        .expect("timestamp with time zone row");
    assert_eq!(
        timestamp_value_to_string(value),
        "2024-01-02 03:04:05.123456"
    );
    assert_eq!(
        timestamp_value_timezone_suffix(value),
        Some("+05:45".to_string())
    );
}

#[test]
#[ignore = "requires local Oracle listener via ORACLE_THIN_TEST_* environment variables"]
fn fetch_timestamp_tz_named_region_decodes_region() {
    let mut conn = connect();
    let result = conn
        .query_described_fetch_all(
            "SELECT \
             TIMESTAMP '2024-01-02 03:04:05.123456' AT TIME ZONE 'Asia/Seoul' AS ts_tz \
             FROM dual",
            1,
        )
        .expect("fetch named-region timestamp with time zone column");

    assert_eq!(
        result
            .columns
            .iter()
            .map(|column| column.column_type)
            .collect::<Vec<_>>(),
        vec![OracleColumnType::Timestamp]
    );
    let value = result
        .result
        .rows
        .first()
        .and_then(|row| row.first())
        .expect("named-region timestamp with time zone row");
    assert_eq!(
        timestamp_value_to_string(value),
        "2024-01-02 03:04:05.123456"
    );
    assert_eq!(
        timestamp_value_timezone_suffix(value),
        Some(" Asia/Seoul".to_string())
    );
}

#[test]
#[ignore = "requires local Oracle listener via ORACLE_THIN_TEST_* environment variables"]
fn fetch_bfile_locator_decodes_as_lob() {
    let mut conn = connect();
    let result = conn
        .query_described_fetch_all(
            "SELECT BFILENAME('DATA_PUMP_DIR', 'space_query_bfile_probe.bin') AS bf FROM dual",
            1,
        )
        .expect("fetch BFILE locator");

    assert_eq!(
        result
            .columns
            .iter()
            .map(|column| column.column_type)
            .collect::<Vec<_>>(),
        vec![OracleColumnType::Bfile]
    );
    match result.result.rows.first().and_then(|row| row.first()) {
        Some(OracleValue::Lob(locator)) => assert!(
            !locator.is_empty(),
            "BFILE locator should include non-empty locator bytes"
        ),
        other => panic!("expected BFILE locator LOB, got {other:?}"),
    }
}

#[test]
#[ignore = "requires local Oracle listener via ORACLE_THIN_TEST_* environment variables"]
fn plsql_bfile_out_bind_returns_locator() {
    let mut conn = connect();
    let mut request = StatementRequest::statement(
        "BEGIN \
         :1 := BFILENAME('DATA_PUMP_DIR', 'space_query_bfile_probe.bin'); \
         END;",
    );
    request.binds.push(BindValue::Out {
        column_type: OracleColumnType::Bfile,
        max_len: 1,
    });

    let values = conn
        .execute_out_binds(&request, &[])
        .expect("PL/SQL BFILE OUT bind");

    match values.first() {
        Some(OracleValue::Lob(locator)) => assert!(
            !locator.is_empty(),
            "BFILE OUT locator should include non-empty locator bytes"
        ),
        other => panic!("expected BFILE OUT locator, got {other:?}"),
    }
}

#[test]
#[ignore = "requires local Oracle listener via ORACLE_THIN_TEST_* environment variables"]
fn fetch_lob_columns_define_as_full_values() {
    let mut conn = connect();
    let result = conn
        .query_described_fetch_all(
            "SELECT \
             TO_CLOB(RPAD('x', 4000, 'x')) || TO_CLOB(RPAD('y', 4000, 'y')) AS c, \
             TO_NCLOB(UNISTR('\\D55C\\AE00')) AS nc, \
             TO_BLOB(HEXTORAW(RPAD('AB', 4000, 'CD'))) AS b \
             FROM dual",
            1,
        )
        .expect("fetch full LOB values");

    assert_eq!(
        result
            .columns
            .iter()
            .map(|column| column.column_type)
            .collect::<Vec<_>>(),
        vec![
            OracleColumnType::Clob,
            OracleColumnType::Clob,
            OracleColumnType::Blob
        ]
    );
    let row = result.result.rows.first().expect("LOB row");
    let clob = value_to_string(&row[0]);
    assert_eq!(clob.len(), 8000);
    assert!(clob.starts_with('x'));
    assert!(clob.ends_with('y'));
    assert_eq!(value_to_string(&row[1]), "\u{D55C}\u{AE00}");
    match &row[2] {
        OracleValue::Bytes(bytes) => {
            assert_eq!(bytes.len(), 2000);
            assert_eq!(bytes.first().copied(), Some(0xab));
            assert_eq!(bytes.last().copied(), Some(0xcd));
        }
        other => panic!("expected BLOB bytes, got {other:?}"),
    }
}

#[test]
#[ignore = "requires local Oracle listener via ORACLE_THIN_TEST_* environment variables"]
fn execute_typed_fetch_all_defines_lob_before_first_fetch() {
    let mut conn = connect();
    let request = StatementRequest::query(
        "SELECT TO_CLOB(RPAD('x', 4000, 'x')) || TO_CLOB(RPAD('y', 4000, 'y')) FROM dual",
        1,
    );
    let result = conn
        .execute_typed_fetch_all(&request, &[OracleColumnType::Clob])
        .expect("typed CLOB fetch all");

    let row = result.rows.first().expect("typed CLOB row");
    let clob = value_to_string(&row[0]);
    assert_eq!(clob.len(), 8000);
    assert!(clob.starts_with('x'));
    assert!(clob.ends_with('y'));
}

#[test]
#[ignore = "requires local Oracle listener via ORACLE_THIN_TEST_* environment variables"]
fn execute_typed_defines_lob_before_initial_fetch() {
    let mut conn = connect();
    let request = StatementRequest::query(
        "SELECT TO_CLOB(RPAD('a', 4000, 'a')) || TO_CLOB(RPAD('b', 4000, 'b')) FROM dual",
        1,
    );
    let result = conn
        .execute_typed(&request, &[OracleColumnType::Clob])
        .expect("typed CLOB initial execute");

    let row = result.rows.first().expect("typed CLOB row");
    let clob = value_to_string(&row[0]);
    assert_eq!(clob.len(), 8000);
    assert!(clob.starts_with('a'));
    assert!(clob.ends_with('b'));
}

#[test]
#[ignore = "requires local Oracle listener via ORACLE_THIN_TEST_* environment variables"]
fn fetch_nchar_and_nvarchar_lazy_batches_keep_charset_form() {
    let mut conn = connect();
    let request = StatementRequest::query(
        "SELECT \
         CAST(UNISTR('\\D55C') || TO_NCHAR(level) AS NVARCHAR2(10)) AS nv, \
         CAST(UNISTR('\\AE00') AS NCHAR(1)) AS nc \
         FROM dual CONNECT BY level <= 5",
        2,
    );
    let initial = conn
        .query_described_initial_request(&request)
        .expect("initial NCHAR described fetch");
    assert_eq!(
        initial
            .columns
            .iter()
            .map(|column| (column.column_type, column.charset_form))
            .collect::<Vec<_>>(),
        vec![
            (OracleColumnType::Varchar, 2),
            (OracleColumnType::Varchar, 2)
        ]
    );
    assert_eq!(
        rows_to_strings(&initial.result.rows),
        vec![
            vec!["\u{D55C}1".to_string(), "\u{AE00}".to_string()],
            vec!["\u{D55C}2".to_string(), "\u{AE00}".to_string()],
        ]
    );
    let cursor_id = initial
        .result
        .cursor_id
        .expect("initial NCHAR fetch should leave an open cursor");

    let fetched = conn
        .fetch_ref_cursor_batch(cursor_id, &initial.columns, 2, false)
        .expect("fetch next NCHAR batch");
    assert_eq!(
        rows_to_strings(&fetched.rows),
        vec![
            vec!["\u{D55C}3".to_string(), "\u{AE00}".to_string()],
            vec!["\u{D55C}4".to_string(), "\u{AE00}".to_string()],
        ]
    );

    let remaining = conn
        .fetch_ref_cursor_all(cursor_id, initial.columns.clone(), 2)
        .expect("fetch remaining NCHAR rows");
    assert_eq!(
        rows_to_strings(&remaining.result.rows),
        vec![vec!["\u{D55C}5".to_string(), "\u{AE00}".to_string()]]
    );
}

#[test]
#[ignore = "requires local Oracle listener via ORACLE_THIN_TEST_* environment variables"]
fn fetch_korean_varchar_uses_negotiated_utf8_charset() {
    let mut conn = connect();
    let result = conn
        .query_described_fetch_all(
            "SELECT CAST(UNISTR('\\D55C\\AE00') AS VARCHAR2(10)) AS ko FROM dual",
            1,
        )
        .expect("fetch Korean VARCHAR2");

    assert_eq!(
        rows_to_strings(&result.result.rows),
        vec![vec!["\u{D55C}\u{AE00}".to_string()]]
    );
}

#[test]
#[ignore = "requires local Oracle listener via ORACLE_THIN_TEST_* environment variables"]
fn fetch_varchar2_4000_boundary_keeps_full_value() {
    let mut conn = connect();
    let result = conn
        .query_described_fetch_all(
            "SELECT CAST(RPAD('x', 4000, 'x') AS VARCHAR2(4000)) AS payload FROM dual",
            1,
        )
        .expect("fetch VARCHAR2(4000)");
    let value = value_to_string(
        result
            .result
            .rows
            .first()
            .and_then(|row| row.first())
            .expect("VARCHAR2(4000) row"),
    );

    assert_eq!(value.len(), 4000);
    assert!(value.bytes().all(|byte| byte == b'x'));
}

#[test]
#[ignore = "requires local Oracle listener via ORACLE_THIN_TEST_* environment variables"]
fn fetch_large_korean_varchar_keeps_utf8_byte_boundary() {
    let mut conn = connect();
    let result = conn
        .query_described_fetch_all(
            "SELECT CAST(REPLACE(RPAD('x', 1333, 'x'), 'x', UNISTR('\\D55C')) AS VARCHAR2(4000)) AS payload FROM dual",
            1,
        )
        .expect("fetch large Korean VARCHAR2");
    let value = value_to_string(
        result
            .result
            .rows
            .first()
            .and_then(|row| row.first())
            .expect("large Korean VARCHAR2 row"),
    );

    assert_eq!(value.chars().count(), 1333);
    assert_eq!(value.len(), 3999);
    assert!(value.starts_with('\u{D55C}'));
    assert!(value.ends_with('\u{D55C}'));
}

#[test]
#[ignore = "requires local Oracle listener via ORACLE_THIN_TEST_* environment variables"]
fn fetch_varchar2_4000_lazy_batches_keep_full_values() {
    let mut conn = connect();
    let request = StatementRequest::query(
        "SELECT \
         level AS n, \
         CAST(RPAD(TO_CHAR(level), 4000, TO_CHAR(level)) AS VARCHAR2(4000)) AS payload \
         FROM dual CONNECT BY level <= 3",
        1,
    );
    let initial = conn
        .query_described_initial_request(&request)
        .expect("initial VARCHAR2(4000) fetch");
    assert_large_digit_row(&initial.result.rows[0], "1");
    let cursor_id = initial
        .result
        .cursor_id
        .expect("large VARCHAR2 fetch should leave an open cursor");

    let fetched = conn
        .fetch_ref_cursor_batch(cursor_id, &initial.columns, 1, false)
        .expect("fetch second VARCHAR2(4000) batch");
    assert_large_digit_row(&fetched.rows[0], "2");

    let remaining = conn
        .fetch_ref_cursor_all(cursor_id, initial.columns.clone(), 1)
        .expect("fetch remaining VARCHAR2(4000) rows");
    assert_eq!(remaining.result.rows.len(), 1);
    assert_large_digit_row(&remaining.result.rows[0], "3");
}

#[test]
#[ignore = "requires local Oracle listener via ORACLE_THIN_TEST_* environment variables"]
fn bind_varchar2_4000_boundary_round_trips_full_value() {
    let mut conn = connect();
    let payload = "x".repeat(4000);
    let mut request = StatementRequest::query("SELECT :1 AS payload FROM dual", 1);
    request.binds.push(BindValue::Text(payload.clone()));

    let result = conn
        .query_described_fetch_all_request(&request)
        .expect("round-trip VARCHAR2(4000) bind");
    let value = value_to_string(
        result
            .result
            .rows
            .first()
            .and_then(|row| row.first())
            .expect("VARCHAR2(4000) bind row"),
    );

    assert_eq!(value, payload);
}

#[test]
#[ignore = "requires local Oracle listener via ORACLE_THIN_TEST_* environment variables"]
fn bind_large_sql_value_before_small_value_round_trips_positions() {
    let mut conn = connect();
    let large_payload = "x".repeat(1001);
    let small_payload = "tail".to_string();
    let mut request = StatementRequest::query(
        "SELECT :1 AS large_payload, :2 AS small_payload FROM dual",
        1,
    );
    request.binds.push(BindValue::Text(large_payload.clone()));
    request.binds.push(BindValue::Text(small_payload.clone()));

    let result = conn
        .query_described_fetch_all_request(&request)
        .expect("round-trip mixed large and small SQL binds");
    let row = result
        .result
        .rows
        .first()
        .expect("mixed large and small bind row");

    assert_eq!(value_to_string(&row[0]), large_payload);
    assert_eq!(value_to_string(&row[1]), small_payload);
}

#[test]
#[ignore = "requires local Oracle listener via ORACLE_THIN_TEST_* environment variables"]
fn bind_large_korean_varchar_round_trips_utf8_boundary() {
    let mut conn = connect();
    let payload = "\u{D55C}".repeat(1333);
    assert_eq!(payload.len(), 3999);
    let mut request = StatementRequest::query("SELECT :1 AS payload FROM dual", 1);
    request.binds.push(BindValue::Text(payload.clone()));

    let result = conn
        .query_described_fetch_all_request(&request)
        .expect("round-trip large Korean VARCHAR2 bind");
    let value = value_to_string(
        result
            .result
            .rows
            .first()
            .and_then(|row| row.first())
            .expect("large Korean VARCHAR2 bind row"),
    );

    assert_eq!(value, payload);
    assert_eq!(value.chars().count(), 1333);
}

#[test]
#[ignore = "requires local Oracle listener via ORACLE_THIN_TEST_* environment variables"]
fn bind_raw_bytes_round_trips_as_raw() {
    let mut conn = connect();
    let mut request = StatementRequest::query("SELECT RAWTOHEX(:1) AS payload FROM dual", 1);
    request
        .binds
        .push(BindValue::Bytes(vec![0xca, 0xfe, 0xba, 0xbe]));

    let result = conn
        .query_described_fetch_all_request(&request)
        .expect("round-trip RAW bind");

    assert_eq!(
        rows_to_strings(&result.result.rows),
        vec![vec!["CAFEBABE".to_string()]]
    );
}

#[test]
#[ignore = "requires local Oracle listener via ORACLE_THIN_TEST_* environment variables"]
fn bind_number_edge_values_round_trip() {
    let mut conn = connect();
    let mut request = StatementRequest::query("SELECT :1, :2, :3, :4 FROM dual", 1);
    request.binds.push(BindValue::Number("-123.4".to_string()));
    request
        .binds
        .push(BindValue::Number("0.0098765".to_string()));
    request
        .binds
        .push(BindValue::Number("-9.8765E-3".to_string()));
    request.binds.push(BindValue::Number("1.2E3".to_string()));

    let result = conn
        .query_described_fetch_all_request(&request)
        .expect("round-trip NUMBER edge binds");

    assert_eq!(
        rows_to_strings(&result.result.rows),
        vec![vec![
            "-123.4".to_string(),
            "0.0098765".to_string(),
            "-0.0098765".to_string(),
            "1200".to_string(),
        ]]
    );
}

#[test]
#[ignore = "requires local Oracle listener via ORACLE_THIN_TEST_* environment variables"]
fn bind_date_and_timestamp_round_trip_values() {
    let mut conn = connect();
    let mut request = StatementRequest::query(
        "SELECT \
         TO_CHAR(:1, 'YYYY-MM-DD HH24:MI:SS') AS d, \
         TO_CHAR(:2, 'YYYY-MM-DD HH24:MI:SS.FF6') AS ts \
         FROM dual",
        1,
    );
    request
        .binds
        .push(BindValue::Date(oracle_datetime(2024, 2, 29, 13, 14, 15, 0)));
    request.binds.push(BindValue::Timestamp(oracle_datetime(
        2024,
        2,
        29,
        13,
        14,
        15,
        123_456_000,
    )));

    let result = conn
        .query_described_fetch_all_request(&request)
        .expect("round-trip DATE and TIMESTAMP binds");

    assert_eq!(
        rows_to_strings(&result.result.rows),
        vec![vec![
            "2024-02-29 13:14:15".to_string(),
            "2024-02-29 13:14:15.123456".to_string(),
        ]]
    );
}

#[test]
#[ignore = "requires local Oracle listener via ORACLE_THIN_TEST_* environment variables"]
fn bind_boolean_values_round_trip_when_supported() {
    let mut conn = connect();
    if !conn.capabilities().supports_sql_boolean {
        return;
    }
    let mut request = StatementRequest::query(
        "SELECT \
         CASE WHEN :1 = TRUE THEN 'TRUE' ELSE 'FALSE' END AS b_true, \
         CASE WHEN :2 = FALSE THEN 'FALSE' ELSE 'TRUE' END AS b_false, \
         CASE WHEN :3 IS NULL THEN 'NULL' ELSE 'NOT NULL' END AS b_null \
         FROM dual",
        1,
    );
    request.binds.push(BindValue::Boolean(true));
    request.binds.push(BindValue::Boolean(false));
    request
        .binds
        .push(BindValue::Null(OracleColumnType::Boolean));

    let result = conn
        .query_described_fetch_all_request(&request)
        .expect("round-trip BOOLEAN binds");

    assert_eq!(
        rows_to_strings(&result.result.rows),
        vec![vec![
            "TRUE".to_string(),
            "FALSE".to_string(),
            "NULL".to_string(),
        ]]
    );
}

#[test]
#[ignore = "requires local Oracle listener via ORACLE_THIN_TEST_* environment variables"]
fn plsql_scalar_out_and_inout_binds_return_values() {
    let mut conn = connect();
    let mut request = StatementRequest::statement(
        "BEGIN \
         :1 := 'OUT-' || :2; \
         :3 := TO_NUMBER(:4) + 5; \
         :5 := :5 || '-IO'; \
         END;",
    );
    request.binds.push(BindValue::Out {
        column_type: OracleColumnType::Varchar,
        max_len: 50,
    });
    request.binds.push(BindValue::Text("TXT".to_string()));
    request.binds.push(BindValue::Out {
        column_type: OracleColumnType::Number,
        max_len: 22,
    });
    request.binds.push(BindValue::Number("37".to_string()));
    request.binds.push(BindValue::InOut {
        column_type: OracleColumnType::Varchar,
        max_len: 50,
        value: Some(BindInputValue::Text("IN".to_string())),
    });

    let values = conn
        .execute_out_binds(&request, &[])
        .expect("PL/SQL scalar OUT and IN OUT binds");

    assert_eq!(
        rows_to_strings(&[values]),
        vec![vec![
            "OUT-TXT".to_string(),
            "42".to_string(),
            "IN-IO".to_string(),
        ]]
    );
}

#[test]
#[ignore = "requires local Oracle listener via ORACLE_THIN_TEST_* environment variables"]
fn plsql_out_bind_types_fill_empty_request_binds() {
    let mut conn = connect();
    let request = StatementRequest::statement(
        "BEGIN \
         :1 := 'TYPE-FALLBACK'; \
         :2 := 42; \
         END;",
    );

    let values = conn
        .execute_out_binds(
            &request,
            &[OracleColumnType::Varchar, OracleColumnType::Number],
        )
        .expect("PL/SQL OUT bind types should fill empty request binds");

    assert_eq!(
        rows_to_strings(&[values]),
        vec![vec!["TYPE-FALLBACK".to_string(), "42".to_string()]]
    );
}

#[test]
#[ignore = "requires local Oracle listener via ORACLE_THIN_TEST_* environment variables"]
fn plsql_boolean_out_and_inout_binds_return_values_when_supported() {
    let mut conn = connect();
    if !conn.capabilities().supports_sql_boolean {
        return;
    }
    let mut request = StatementRequest::statement(
        "BEGIN \
         :1 := TRUE; \
         :2 := NOT :2; \
         :3 := NULL; \
         END;",
    );
    request.binds.push(BindValue::Out {
        column_type: OracleColumnType::Boolean,
        max_len: 4,
    });
    request.binds.push(BindValue::InOut {
        column_type: OracleColumnType::Boolean,
        max_len: 4,
        value: Some(BindInputValue::Boolean(true)),
    });
    request.binds.push(BindValue::Out {
        column_type: OracleColumnType::Boolean,
        max_len: 4,
    });

    let values = conn
        .execute_out_binds(&request, &[])
        .expect("PL/SQL BOOLEAN OUT and IN OUT binds");

    assert_eq!(
        values,
        vec![
            OracleValue::Boolean(true),
            OracleValue::Boolean(false),
            OracleValue::Null,
        ]
    );
}

#[test]
#[ignore = "requires local Oracle listener via ORACLE_THIN_TEST_* environment variables"]
fn plsql_date_and_timestamp_out_and_inout_binds_return_values() {
    let mut conn = connect();
    let mut request = StatementRequest::statement(
        "BEGIN \
         :1 := DATE '2024-02-29'; \
         :2 := :2 + INTERVAL '1' SECOND; \
         :3 := TIMESTAMP '2024-02-29 01:02:03.456789'; \
         :4 := :4 + INTERVAL '2' SECOND; \
         END;",
    );
    request.binds.push(BindValue::Out {
        column_type: OracleColumnType::Date,
        max_len: 7,
    });
    request.binds.push(BindValue::InOut {
        column_type: OracleColumnType::Date,
        max_len: 7,
        value: Some(BindInputValue::Date(oracle_datetime(
            2024, 2, 29, 1, 2, 3, 0,
        ))),
    });
    request.binds.push(BindValue::Out {
        column_type: OracleColumnType::Timestamp,
        max_len: 11,
    });
    request.binds.push(BindValue::InOut {
        column_type: OracleColumnType::Timestamp,
        max_len: 11,
        value: Some(BindInputValue::Timestamp(oracle_datetime(
            2024,
            2,
            29,
            1,
            2,
            3,
            123_456_000,
        ))),
    });

    let values = conn
        .execute_out_binds(&request, &[])
        .expect("PL/SQL DATE and TIMESTAMP OUT and IN OUT binds");

    assert_eq!(
        values,
        vec![
            OracleValue::DateTime(oracle_datetime(2024, 2, 29, 0, 0, 0, 0)),
            OracleValue::DateTime(oracle_datetime(2024, 2, 29, 1, 2, 4, 0)),
            OracleValue::Timestamp(oracle_datetime(2024, 2, 29, 1, 2, 3, 456_789_000)),
            OracleValue::Timestamp(oracle_datetime(2024, 2, 29, 1, 2, 5, 123_456_000)),
        ]
    );
}

#[test]
#[ignore = "requires local Oracle listener via ORACLE_THIN_TEST_* environment variables"]
fn timestamptz_bind_round_trips_fixed_offset() {
    let mut conn = connect();
    let mut request = StatementRequest::query(
        "SELECT TO_CHAR(:1, 'YYYY-MM-DD HH24:MI:SS.FF6 TZH:TZM') FROM dual",
        1,
    );
    let mut value = oracle_datetime(2024, 1, 2, 3, 4, 5, 123_456_000);
    value.timezone_offset_minutes = Some(345);
    request.binds.push(BindValue::Timestamp(value));

    let result = conn
        .query_described_fetch_all_request(&request)
        .expect("round-trip TIMESTAMP WITH TIME ZONE bind");

    assert_eq!(
        rows_to_strings(&result.result.rows),
        vec![vec!["2024-01-02 03:04:05.123456 +05:45".to_string()]]
    );
}

#[test]
#[ignore = "requires local Oracle listener via ORACLE_THIN_TEST_* environment variables"]
fn timestamptz_bind_round_trips_named_region() {
    let mut conn = connect();
    let mut request = StatementRequest::query(
        "SELECT \
         TO_CHAR(:1, 'YYYY-MM-DD HH24:MI:SS.FF6 TZH:TZM'), \
         TO_CHAR(:1, 'TZR') \
         FROM dual",
        1,
    );
    let mut value = oracle_datetime(2024, 1, 2, 3, 4, 5, 123_456_000);
    value.timezone_region_id = Some(273);
    request.binds.push(BindValue::Timestamp(value));

    let result = conn
        .query_described_fetch_all_request(&request)
        .expect("round-trip named-region TIMESTAMP WITH TIME ZONE bind");
    let rows = rows_to_strings(&result.result.rows);

    assert_eq!(rows[0][0], "2024-01-02 03:04:05.123456 +09:00");
    assert!(
        rows[0][1].to_ascii_uppercase().contains("SEOUL"),
        "unexpected TIMESTAMP WITH TIME ZONE region: {}",
        rows[0][1]
    );
}

#[test]
#[ignore = "requires local Oracle listener via ORACLE_THIN_TEST_* environment variables"]
fn plsql_timestamptz_out_bind_returns_fixed_offset() {
    let mut conn = connect();
    let mut request = StatementRequest::statement(
        "BEGIN \
         :1 := FROM_TZ(TIMESTAMP '2024-01-02 03:04:05.123456', '+05:45'); \
         END;",
    );
    request.binds.push(BindValue::Out {
        column_type: OracleColumnType::Timestamp,
        max_len: 13,
    });

    let values = conn
        .execute_out_binds(&request, &[])
        .expect("PL/SQL TIMESTAMP WITH TIME ZONE OUT bind");
    let value = values
        .first()
        .expect("TIMESTAMP WITH TIME ZONE OUT bind value");

    assert_eq!(
        timestamp_value_to_string(value),
        "2024-01-02 03:04:05.123456"
    );
    assert_eq!(
        timestamp_value_timezone_suffix(value),
        Some("+05:45".to_string())
    );
}

#[test]
#[ignore = "requires local Oracle listener via ORACLE_THIN_TEST_* environment variables"]
fn plsql_interval_out_binds_return_values() {
    let mut conn = connect();
    let mut request = StatementRequest::statement(
        "BEGIN \
         :1 := TO_YMINTERVAL('2021-10'); \
         :2 := TO_YMINTERVAL('-05-03'); \
         :3 := TO_DSINTERVAL('2 12:23:34.456'); \
         :4 := TO_DSINTERVAL('-0 10:20:30.456789'); \
         END;",
    );
    request.binds.push(BindValue::Out {
        column_type: OracleColumnType::IntervalYearMonth,
        max_len: 5,
    });
    request.binds.push(BindValue::Out {
        column_type: OracleColumnType::IntervalYearMonth,
        max_len: 5,
    });
    request.binds.push(BindValue::Out {
        column_type: OracleColumnType::IntervalDaySecond,
        max_len: 11,
    });
    request.binds.push(BindValue::Out {
        column_type: OracleColumnType::IntervalDaySecond,
        max_len: 11,
    });

    let values = conn
        .execute_out_binds(&request, &[])
        .expect("PL/SQL INTERVAL OUT binds");

    assert_eq!(
        rows_to_strings(&[values]),
        vec![vec![
            "+2021-10".to_string(),
            "-05-03".to_string(),
            "+02 12:23:34.456000".to_string(),
            "-00 10:20:30.456789".to_string(),
        ]]
    );
}

#[test]
#[ignore = "requires local Oracle listener via ORACLE_THIN_TEST_* environment variables"]
fn plsql_raw_out_and_inout_binds_return_bytes() {
    let mut conn = connect();
    let mut request = StatementRequest::statement(
        "BEGIN \
         :1 := HEXTORAW('CAFE'); \
         :2 := UTL_RAW.CONCAT(:2, HEXTORAW('BE')); \
         END;",
    );
    request.binds.push(BindValue::Out {
        column_type: OracleColumnType::Raw,
        max_len: 16,
    });
    request.binds.push(BindValue::InOut {
        column_type: OracleColumnType::Raw,
        max_len: 16,
        value: Some(BindInputValue::Bytes(vec![0xca])),
    });

    let values = conn
        .execute_out_binds(&request, &[])
        .expect("PL/SQL RAW OUT and IN OUT binds");

    assert_eq!(
        values,
        vec![
            OracleValue::Bytes(vec![0xca, 0xfe]),
            OracleValue::Bytes(vec![0xca, 0xbe]),
        ]
    );
}

#[test]
#[ignore = "requires local Oracle listener via ORACLE_THIN_TEST_* environment variables"]
fn plsql_clob_out_bind_returns_large_text() {
    let mut conn = connect();
    let mut request = StatementRequest::statement(
        "BEGIN \
         :1 := TO_CLOB(RPAD('x', 4000, 'x')) || TO_CLOB(RPAD('y', 4000, 'y')); \
         END;",
    );
    request.binds.push(BindValue::Out {
        column_type: OracleColumnType::Clob,
        max_len: 8000,
    });

    let values = conn
        .execute_out_binds(&request, &[])
        .expect("PL/SQL CLOB OUT bind");
    let value = value_to_string(values.first().expect("CLOB OUT value"));

    assert_eq!(value.len(), 8000);
    assert!(value.starts_with('x'));
    assert!(value.ends_with('y'));
}

#[test]
#[ignore = "requires local Oracle listener via ORACLE_THIN_TEST_* environment variables"]
fn plsql_nclob_out_bind_returns_korean_text() {
    let mut conn = connect();
    let mut request = StatementRequest::statement(
        "BEGIN \
         :1 := TO_NCLOB(UNISTR('\\D55C\\AE00')); \
         END;",
    );
    request.binds.push(BindValue::Out {
        column_type: OracleColumnType::Nclob,
        max_len: 20,
    });

    let values = conn
        .execute_out_binds(&request, &[])
        .expect("PL/SQL NCLOB OUT bind");

    assert_eq!(
        value_to_string(values.first().expect("NCLOB OUT value")),
        "\u{D55C}\u{AE00}"
    );
}

#[test]
#[ignore = "requires local Oracle listener via ORACLE_THIN_TEST_* environment variables"]
fn plsql_blob_out_bind_returns_bytes() {
    let mut conn = connect();
    let mut request = StatementRequest::statement(
        "BEGIN \
         :1 := TO_BLOB(HEXTORAW(RPAD('AB', 4000, 'CD'))); \
         END;",
    );
    request.binds.push(BindValue::Out {
        column_type: OracleColumnType::Blob,
        max_len: 2000,
    });

    let values = conn
        .execute_out_binds(&request, &[])
        .expect("PL/SQL BLOB OUT bind");

    match values.first() {
        Some(OracleValue::Bytes(bytes)) => {
            assert_eq!(bytes.len(), 2000);
            assert_eq!(bytes.first().copied(), Some(0xab));
            assert_eq!(bytes.last().copied(), Some(0xcd));
        }
        other => panic!("expected BLOB OUT bytes, got {other:?}"),
    }
}

#[test]
#[ignore = "requires local Oracle listener via ORACLE_THIN_TEST_* environment variables"]
fn plsql_clob_inout_bind_keeps_large_text() {
    let mut conn = connect();
    let payload = "x".repeat(5000);
    let mut request = StatementRequest::statement(
        "BEGIN \
         :1 := :1 || TO_CLOB('tail'); \
         END;",
    );
    request.binds.push(BindValue::InOut {
        column_type: OracleColumnType::Clob,
        max_len: 6000,
        value: Some(BindInputValue::Text(payload.clone())),
    });

    let values = conn
        .execute_out_binds(&request, &[])
        .expect("PL/SQL CLOB IN OUT bind");
    let value = value_to_string(values.first().expect("CLOB IN OUT value"));

    assert_eq!(value, format!("{payload}tail"));
}

#[test]
#[ignore = "requires local Oracle listener via ORACLE_THIN_TEST_* environment variables"]
fn plsql_nclob_inout_bind_keeps_korean_text() {
    let mut conn = connect();
    let mut request = StatementRequest::statement(
        "BEGIN \
         :1 := :1 || TO_NCLOB(UNISTR('\\AE00')); \
         END;",
    );
    request.binds.push(BindValue::InOut {
        column_type: OracleColumnType::Nclob,
        max_len: 20,
        value: Some(BindInputValue::Text("\u{D55C}".to_string())),
    });

    let values = conn
        .execute_out_binds(&request, &[])
        .expect("PL/SQL NCLOB IN OUT bind");

    assert_eq!(
        value_to_string(values.first().expect("NCLOB IN OUT value")),
        "\u{D55C}\u{AE00}"
    );
}

#[test]
#[ignore = "requires local Oracle listener via ORACLE_THIN_TEST_* environment variables"]
fn plsql_blob_inout_bind_returns_bytes() {
    let mut conn = connect();
    let mut request = StatementRequest::statement(
        "BEGIN \
         :1 := TO_BLOB(UTL_RAW.CONCAT(:1, HEXTORAW('BEEF'))); \
         END;",
    );
    request.binds.push(BindValue::InOut {
        column_type: OracleColumnType::Blob,
        max_len: 8,
        value: Some(BindInputValue::Bytes(vec![0xca, 0xfe])),
    });

    let values = conn
        .execute_out_binds(&request, &[])
        .expect("PL/SQL BLOB IN OUT bind");

    assert_eq!(
        values,
        vec![OracleValue::Bytes(vec![0xca, 0xfe, 0xbe, 0xef])]
    );
}

#[test]
#[ignore = "requires local Oracle listener via ORACLE_THIN_TEST_* environment variables"]
fn plsql_out_ref_cursor_fetches_rows() {
    let mut conn = connect();
    let mut request = StatementRequest::statement(
        "BEGIN \
         OPEN :1 FOR SELECT 7 AS n, 'ref cursor' AS label FROM dual; \
         END;",
    );
    request.binds.push(BindValue::Out {
        column_type: OracleColumnType::Cursor,
        max_len: 1,
    });

    let values = conn
        .execute_out_binds(&request, &[])
        .expect("PL/SQL OUT REF CURSOR bind");
    let cursor = match values.first() {
        Some(OracleValue::Cursor(cursor)) => cursor.clone(),
        other => panic!("expected OUT REF CURSOR, got {other:?}"),
    };
    let rows = conn
        .fetch_ref_cursor_all(cursor.cursor_id, cursor.columns, 10)
        .expect("fetch OUT REF CURSOR rows");

    assert_eq!(
        rows_to_strings(&rows.result.rows),
        vec![vec!["7".to_string(), "ref cursor".to_string()]]
    );
}

#[test]
#[ignore = "requires local Oracle listener via ORACLE_THIN_TEST_* environment variables"]
fn plsql_out_ref_cursor_between_scalar_out_binds_preserves_alignment() {
    let mut conn = connect();
    let mut request = StatementRequest::statement(
        "BEGIN \
         :1 := 101; \
         OPEN :2 FOR SELECT 8 AS n, 'middle cursor' AS label FROM dual; \
         :3 := 'after cursor'; \
         END;",
    );
    request.binds.push(BindValue::Out {
        column_type: OracleColumnType::Number,
        max_len: 22,
    });
    request.binds.push(BindValue::Out {
        column_type: OracleColumnType::Cursor,
        max_len: 4,
    });
    request.binds.push(BindValue::Out {
        column_type: OracleColumnType::Varchar,
        max_len: 100,
    });

    let values = conn
        .execute_out_binds(&request, &[])
        .expect("PL/SQL OUT bind row with middle REF CURSOR");
    assert_eq!(value_to_string(&values[0]), "101");
    assert_eq!(value_to_string(&values[2]), "after cursor");
    let cursor = match values.get(1) {
        Some(OracleValue::Cursor(cursor)) => cursor.clone(),
        other => panic!("expected middle OUT REF CURSOR, got {other:?}"),
    };
    let rows = conn
        .fetch_ref_cursor_all(cursor.cursor_id, cursor.columns, 10)
        .expect("fetch middle OUT REF CURSOR rows");

    assert_eq!(
        rows_to_strings(&rows.result.rows),
        vec![vec!["8".to_string(), "middle cursor".to_string()]]
    );
}

#[test]
#[ignore = "requires local Oracle listener via ORACLE_THIN_TEST_* environment variables"]
fn plsql_multiple_out_ref_cursors_keep_independent_metadata() {
    let mut conn = connect();
    let mut request = StatementRequest::statement(
        "BEGIN \
         OPEN :1 FOR \
             SELECT CAST(RPAD('A', 4000, 'A') AS VARCHAR2(4000)) AS payload, \
                    CAST('first' AS VARCHAR2(10)) AS label \
             FROM dual; \
         OPEN :2 FOR \
             SELECT TO_CLOB('CLOB-') || TO_CLOB(RPAD('x', 4000, 'x')) AS doc, \
                    HEXTORAW('CAFE') AS raw_value, \
                    FROM_TZ(TIMESTAMP '2024-01-02 03:04:05.123456', '+05:45') AS ts_tz \
             FROM dual; \
         END;",
    );
    request.binds.push(BindValue::Out {
        column_type: OracleColumnType::Cursor,
        max_len: 1,
    });
    request.binds.push(BindValue::Out {
        column_type: OracleColumnType::Cursor,
        max_len: 1,
    });

    let values = conn
        .execute_out_binds(&request, &[])
        .expect("PL/SQL multiple OUT REF CURSOR binds");
    let first_cursor = match values.first() {
        Some(OracleValue::Cursor(cursor)) => cursor.clone(),
        other => panic!("expected first OUT REF CURSOR, got {other:?}"),
    };
    let second_cursor = match values.get(1) {
        Some(OracleValue::Cursor(cursor)) => cursor.clone(),
        other => panic!("expected second OUT REF CURSOR, got {other:?}"),
    };
    assert_ne!(first_cursor.cursor_id, second_cursor.cursor_id);
    assert_eq!(
        first_cursor
            .columns
            .iter()
            .map(|column| column.column_type)
            .collect::<Vec<_>>(),
        vec![OracleColumnType::Varchar, OracleColumnType::Varchar]
    );
    assert_eq!(
        second_cursor
            .columns
            .iter()
            .map(|column| column.column_type)
            .collect::<Vec<_>>(),
        vec![
            OracleColumnType::Clob,
            OracleColumnType::Raw,
            OracleColumnType::Timestamp,
        ]
    );

    let second_rows = conn
        .fetch_ref_cursor_all(second_cursor.cursor_id, second_cursor.columns, 1)
        .expect("fetch second OUT REF CURSOR first");
    let second_row = second_rows.result.rows.first().expect("second cursor row");
    assert_eq!(value_to_string(&second_row[0]).len(), 4005);
    assert!(value_to_string(&second_row[0]).starts_with("CLOB-"));
    assert_eq!(second_row[1], OracleValue::Bytes(vec![0xca, 0xfe]));
    assert_eq!(
        timestamp_value_to_string(&second_row[2]),
        "2024-01-02 03:04:05.123456"
    );
    assert_eq!(
        timestamp_value_timezone_suffix(&second_row[2]),
        Some("+05:45".to_string())
    );

    let first_rows = conn
        .fetch_ref_cursor_all(first_cursor.cursor_id, first_cursor.columns, 1)
        .expect("fetch first OUT REF CURSOR after second");
    let first_row = first_rows.result.rows.first().expect("first cursor row");
    assert_eq!(value_to_string(&first_row[0]).len(), 4000);
    assert!(value_to_string(&first_row[0])
        .chars()
        .all(|ch| ch == 'A'));
    assert_eq!(value_to_string(&first_row[1]), "first");
}

#[test]
#[ignore = "requires local Oracle listener via ORACLE_THIN_TEST_* environment variables"]
fn dml_returning_single_row_out_binds_return_values() {
    let config = live_config();
    let table = unique_table_name("RET");
    let _guard = TableDropGuard::new(config.clone(), table.clone());
    let mut conn = connect_with_config(config);
    conn.query_drop(&format!(
        "CREATE TABLE {table} (id NUMBER PRIMARY KEY, name VARCHAR2(30))"
    ))
    .expect("create DML RETURNING test table");

    let mut request = StatementRequest::statement(format!(
        "INSERT INTO {table} (id, name) VALUES (:1, :2) RETURNING id, name INTO :3, :4"
    ));
    request.binds.push(BindValue::Number("1".to_string()));
    request.binds.push(BindValue::Text("alpha".to_string()));
    request.binds.push(BindValue::Out {
        column_type: OracleColumnType::Number,
        max_len: 22,
    });
    request.binds.push(BindValue::Out {
        column_type: OracleColumnType::Varchar,
        max_len: 30,
    });

    let values = conn
        .execute_out_binds(&request, &[])
        .expect("DML RETURNING OUT binds");

    assert_eq!(
        rows_to_strings(&[values]),
        vec![vec!["1".to_string(), "alpha".to_string()]]
    );
}

#[test]
#[ignore = "requires local Oracle listener via ORACLE_THIN_TEST_* environment variables"]
fn dml_returning_expression_can_use_input_bind_before_into() {
    let config = live_config();
    let table = unique_table_name("RET_EXPR");
    let _guard = TableDropGuard::new(config.clone(), table.clone());
    let mut conn = connect_with_config(config);
    conn.query_drop(&format!("CREATE TABLE {table} (id NUMBER PRIMARY KEY)"))
        .expect("create DML RETURNING expression test table");

    let mut request = StatementRequest::statement(format!(
        "INSERT INTO {table} (id) VALUES (:1) RETURNING id + :2 INTO :3"
    ));
    request.binds.push(BindValue::Number("5".to_string()));
    request.binds.push(BindValue::Number("18".to_string()));
    request.binds.push(BindValue::Out {
        column_type: OracleColumnType::Number,
        max_len: 22,
    });

    let values = conn
        .execute_out_binds(&request, &[])
        .expect("DML RETURNING expression with input bind");

    assert_eq!(rows_to_strings(&[values]), vec![vec!["23".to_string()]]);
}

#[test]
#[ignore = "requires local Oracle listener via ORACLE_THIN_TEST_* environment variables"]
fn dml_returning_supports_quoted_bind_names() {
    let config = live_config();
    let table = unique_table_name("RET_QUOTED");
    let _guard = TableDropGuard::new(config.clone(), table.clone());
    let mut conn = connect_with_config(config);
    conn.query_drop(&format!(
        "CREATE TABLE {table} (id NUMBER PRIMARY KEY, name VARCHAR2(30))"
    ))
    .expect("create quoted DML RETURNING test table");

    let mut request = StatementRequest::statement(format!(
        r#"INSERT INTO {table} (id, name) VALUES (:int_val, :str_val)
           RETURNING id, name INTO :"_val1", :"VaL_2""#
    ));
    request.binds.push(BindValue::Number("1".to_string()));
    request.binds.push(BindValue::Text("alpha".to_string()));
    request.binds.push(BindValue::Out {
        column_type: OracleColumnType::Number,
        max_len: 22,
    });
    request.binds.push(BindValue::Out {
        column_type: OracleColumnType::Varchar,
        max_len: 30,
    });

    let values = conn
        .execute_out_binds(&request, &[])
        .expect("DML RETURNING quoted OUT binds");

    assert_eq!(
        rows_to_strings(&[values]),
        vec![vec!["1".to_string(), "alpha".to_string()]]
    );
}

#[test]
#[ignore = "requires local Oracle listener via ORACLE_THIN_TEST_* environment variables"]
fn dml_returning_supports_non_ascii_bind_names() {
    let config = live_config();
    let table = unique_table_name("RET_NONASCII");
    let _guard = TableDropGuard::new(config.clone(), table.clone());
    let mut conn = connect_with_config(config);
    conn.query_drop(&format!("CREATE TABLE {table} (id NUMBER PRIMARY KEY)"))
        .expect("create non-ASCII DML RETURNING test table");

    let mut request = StatementRequest::statement(format!(
        "INSERT INTO {table} (id) VALUES (:int_val) RETURNING id INTO :m\u{00E9}il"
    ));
    request.binds.push(BindValue::Number("7".to_string()));
    request.binds.push(BindValue::Out {
        column_type: OracleColumnType::Number,
        max_len: 22,
    });

    let values = conn
        .execute_out_binds(&request, &[])
        .expect("DML RETURNING non-ASCII OUT bind");

    assert_eq!(rows_to_strings(&[values]), vec![vec!["7".to_string()]]);
}

#[test]
#[ignore = "requires local Oracle listener via ORACLE_THIN_TEST_* environment variables"]
fn dml_returning_rejects_invalid_rowid_bind_name() {
    let config = live_config();
    let table = unique_table_name("RET_BAD_BIND");
    let _guard = TableDropGuard::new(config.clone(), table.clone());
    let mut conn = connect_with_config(config);
    conn.query_drop(&format!("CREATE TABLE {table} (id NUMBER PRIMARY KEY)"))
        .expect("create invalid DML RETURNING bind test table");

    let mut request = StatementRequest::statement(format!(
        "INSERT INTO {table} (id) VALUES (:1) RETURNING id INTO :ROWID"
    ));
    request.binds.push(BindValue::Number("1".to_string()));
    request.binds.push(BindValue::Out {
        column_type: OracleColumnType::Number,
        max_len: 22,
    });

    let err = conn
        .execute_out_binds(&request, &[])
        .expect_err("invalid ROWID bind name should fail");
    let message = err.to_string();
    assert!(
        message.contains("ORA-01745") || message.contains("invalid host/bind variable name"),
        "unexpected error: {message}"
    );
}

#[test]
#[ignore = "requires local Oracle listener via ORACLE_THIN_TEST_* environment variables"]
fn dml_returning_parses_without_spaces_around_returning_into() {
    let config = live_config();
    let table = unique_table_name("RET_NOSPACE");
    let _guard = TableDropGuard::new(config.clone(), table.clone());
    let mut conn = connect_with_config(config);
    conn.query_drop(&format!("CREATE TABLE {table} (id NUMBER PRIMARY KEY)"))
        .expect("create no-space DML RETURNING test table");

    let mut request = StatementRequest::statement(format!(
        "INSERT INTO {table} (id) VALUES (:1)returning(id)into :2"
    ));
    request.binds.push(BindValue::Number("25".to_string()));
    request.binds.push(BindValue::Out {
        column_type: OracleColumnType::Number,
        max_len: 22,
    });

    let values = conn
        .execute_out_binds(&request, &[])
        .expect("DML RETURNING without spaces");

    assert_eq!(rows_to_strings(&[values]), vec![vec!["25".to_string()]]);
}

#[test]
#[ignore = "requires local Oracle listener via ORACLE_THIN_TEST_* environment variables"]
fn dml_returning_rowid_can_be_used_to_fetch_inserted_row() {
    let config = live_config();
    let table = unique_table_name("RET_ROWID");
    let _guard = TableDropGuard::new(config.clone(), table.clone());
    let mut conn = connect_with_config(config);
    conn.query_drop(&format!(
        "CREATE TABLE {table} (id NUMBER PRIMARY KEY, name VARCHAR2(30))"
    ))
    .expect("create ROWID DML RETURNING test table");

    let mut request = StatementRequest::statement(format!(
        "INSERT INTO {table} (id, name) VALUES (:1, :2) RETURNING ROWID INTO :3"
    ));
    request.binds.push(BindValue::Number("278".to_string()));
    request.binds.push(BindValue::Text("String 278".to_string()));
    request.binds.push(BindValue::Out {
        column_type: OracleColumnType::Varchar,
        max_len: 64,
    });

    let values = conn
        .execute_out_binds(&request, &[])
        .expect("DML RETURNING ROWID");
    let rowid = match values.first() {
        Some(OracleValue::Text(value)) => value,
        other => panic!("expected ROWID text, got {other:?}"),
    };

    let mut fetch_request =
        StatementRequest::query(format!("SELECT id, name FROM {table} WHERE ROWID = :1"), 10);
    fetch_request.binds.push(BindValue::Text(rowid.clone()));
    let result = conn
        .query_described_fetch_all_request(&fetch_request)
        .expect("fetch row by returned ROWID");

    assert_eq!(
        rows_to_strings(&result.result.rows),
        vec![vec!["278".to_string(), "String 278".to_string()]]
    );
}

#[test]
#[ignore = "requires local Oracle listener via ORACLE_THIN_TEST_* environment variables"]
fn dml_returning_iot_rowid_can_be_used_to_fetch_inserted_row() {
    let config = live_config();
    let table = unique_table_name("RET_IOT_RID");
    let _guard = TableDropGuard::new(config.clone(), table.clone());
    let mut conn = connect_with_config(config);
    conn.query_drop(&format!(
        "CREATE TABLE {table} (id NUMBER PRIMARY KEY, name VARCHAR2(30), created_at DATE) \
         ORGANIZATION INDEX"
    ))
    .expect("create IOT ROWID DML RETURNING test table");

    let mut request = StatementRequest::statement(format!(
        "INSERT INTO {table} (id, name, created_at) \
         VALUES (:1, :2, TO_DATE(:3, 'YYYY-MM-DD')) \
         RETURNING ROWID INTO :4"
    ));
    request.binds.push(BindValue::Number("1".to_string()));
    request.binds.push(BindValue::Text("ABC".to_string()));
    request.binds.push(BindValue::Text("2017-04-11".to_string()));
    request.binds.push(BindValue::Out {
        column_type: OracleColumnType::Varchar,
        max_len: 4000,
    });

    let values = conn
        .execute_out_binds(&request, &[])
        .expect("DML RETURNING IOT ROWID");
    let rowid = match values.first() {
        Some(OracleValue::Text(value)) => value,
        other => panic!("expected IOT ROWID text, got {other:?}"),
    };

    let mut fetch_request = StatementRequest::query(
        format!("SELECT id, name, TO_CHAR(created_at, 'YYYY-MM-DD') FROM {table} WHERE ROWID = :1"),
        10,
    );
    fetch_request.binds.push(BindValue::Text(rowid.clone()));
    let result = conn
        .query_described_fetch_all_request(&fetch_request)
        .expect("fetch IOT row by returned ROWID");

    assert_eq!(
        rows_to_strings(&result.result.rows),
        vec![vec![
            "1".to_string(),
            "ABC".to_string(),
            "2017-04-11".to_string()
        ]]
    );
}

#[test]
#[ignore = "requires local Oracle listener via ORACLE_THIN_TEST_* environment variables"]
fn plsql_dml_returning_iot_rowid_can_be_used_to_fetch_inserted_row() {
    let config = live_config();
    let table = unique_table_name("RET_IOT_PLSQL");
    let _guard = TableDropGuard::new(config.clone(), table.clone());
    let mut conn = connect_with_config(config);
    conn.query_drop(&format!(
        "CREATE TABLE {table} (id NUMBER PRIMARY KEY, name VARCHAR2(30), created_at DATE) \
         ORGANIZATION INDEX"
    ))
    .expect("create PL/SQL IOT ROWID DML RETURNING test table");

    let mut request = StatementRequest::statement(format!(
        "BEGIN \
         INSERT INTO {table} (id, name, created_at) \
         VALUES (:1, :2, TO_DATE(:3, 'YYYY-MM-DD')) \
         RETURNING ROWID INTO :4; \
         END;"
    ));
    request.is_plsql = true;
    request.binds.push(BindValue::Number("1".to_string()));
    request.binds.push(BindValue::Text("ABC".to_string()));
    request.binds.push(BindValue::Text("2017-04-11".to_string()));
    request.binds.push(BindValue::Out {
        column_type: OracleColumnType::Varchar,
        max_len: 4000,
    });

    let values = conn
        .execute_out_binds(&request, &[])
        .expect("PL/SQL DML RETURNING IOT ROWID");
    let rowid = match values.first() {
        Some(OracleValue::Text(value)) => value,
        other => panic!("expected PL/SQL IOT ROWID text, got {other:?}"),
    };

    let mut fetch_request = StatementRequest::query(
        format!("SELECT id, name, TO_CHAR(created_at, 'YYYY-MM-DD') FROM {table} WHERE ROWID = :1"),
        10,
    );
    fetch_request.binds.push(BindValue::Text(rowid.clone()));
    let result = conn
        .query_described_fetch_all_request(&fetch_request)
        .expect("fetch PL/SQL IOT row by returned ROWID");

    assert_eq!(
        rows_to_strings(&result.result.rows),
        vec![vec![
            "1".to_string(),
            "ABC".to_string(),
            "2017-04-11".to_string()
        ]]
    );
}

#[test]
#[ignore = "requires local Oracle listener via ORACLE_THIN_TEST_* environment variables"]
fn dml_returning_lob_out_binds_return_values() {
    let config = live_config();
    let table = unique_table_name("RET_LOB");
    let _guard = TableDropGuard::new(config.clone(), table.clone());
    let mut conn = connect_with_config(config);
    conn.query_drop(&format!(
        "CREATE TABLE {table} (id NUMBER PRIMARY KEY, clob_col CLOB, blob_col BLOB)"
    ))
    .expect("create LOB DML RETURNING test table");

    let mut request = StatementRequest::statement(format!(
        "INSERT INTO {table} (id, clob_col, blob_col) \
         VALUES (:1, :2, HEXTORAW(:3)) \
         RETURNING clob_col, blob_col INTO :4, :5"
    ));
    request.binds.push(BindValue::Number("1".to_string()));
    request
        .binds
        .push(BindValue::Text("A short CLOB - 1618".to_string()));
    request.binds.push(BindValue::Text("CAFE".to_string()));
    request.binds.push(BindValue::Out {
        column_type: OracleColumnType::Clob,
        max_len: 200,
    });
    request.binds.push(BindValue::Out {
        column_type: OracleColumnType::Blob,
        max_len: 2,
    });

    let values = conn
        .execute_out_binds(&request, &[])
        .expect("DML RETURNING LOB OUT binds");

    assert_eq!(
        value_to_string(values.first().expect("CLOB RETURNING value")),
        "A short CLOB - 1618"
    );
    assert_eq!(
        values.get(1),
        Some(&OracleValue::Bytes(vec![0xca, 0xfe]))
    );
}

#[test]
#[ignore = "requires local Oracle listener via ORACLE_THIN_TEST_* environment variables"]
fn dml_returning_multiple_clobs_preserves_all_rows() {
    let config = live_config();
    let table = unique_table_name("RET_CLOBS");
    let _guard = TableDropGuard::new(config.clone(), table.clone());
    let mut conn = connect_with_config(config);
    conn.query_drop(&format!(
        "CREATE TABLE {table} (id NUMBER PRIMARY KEY, clob_col CLOB, touched NUMBER)"
    ))
    .expect("create multi-CLOB DML RETURNING test table");
    let clob_data = [
        "Short CLOB - 1625a",
        "Short CLOB - 1625b",
        "Short CLOB - 1625c",
        "Short CLOB - 1625d",
    ];
    for (index, value) in clob_data.iter().enumerate() {
        let mut insert_request = StatementRequest::statement(format!(
            "INSERT INTO {table} (id, clob_col) VALUES (:1, :2)"
        ));
        insert_request
            .binds
            .push(BindValue::Number((index + 1).to_string()));
        insert_request.binds.push(BindValue::Text((*value).to_string()));
        conn.execute_typed_with_implicit(&insert_request, &[])
            .expect("insert CLOB source row");
    }

    let mut request = StatementRequest::statement(format!(
        "UPDATE {table} SET touched = 1 WHERE touched IS NULL RETURNING clob_col INTO :1"
    ));
    request.binds.push(BindValue::Out {
        column_type: OracleColumnType::Clob,
        max_len: 200,
    });

    let result = conn
        .execute_out_binds_with_implicit(&request, &[])
        .expect("multi-row DML RETURNING CLOB OUT bind");
    let mut returned = result
        .rows
        .iter()
        .map(|row| value_to_string(row.first().expect("CLOB row value")))
        .collect::<Vec<_>>();
    returned.sort();

    assert_eq!(returned, clob_data);
}

#[test]
#[ignore = "requires local Oracle listener via ORACLE_THIN_TEST_* environment variables"]
fn dml_returning_repeated_deletes_clear_previous_out_rows() {
    let config = live_config();
    let table = unique_table_name("RET_REPEAT");
    let _guard = TableDropGuard::new(config.clone(), table.clone());
    let mut conn = connect_with_config(config);
    conn.query_drop(&format!(
        "CREATE TABLE {table} (id NUMBER PRIMARY KEY, name VARCHAR2(30))"
    ))
    .expect("create repeated DML RETURNING test table");
    for id in 1..=10 {
        conn.query_drop(&format!(
            "INSERT INTO {table} (id, name) VALUES ({id}, 'Test String {id}')"
        ))
        .expect("insert repeated DML RETURNING source row");
    }

    let mut results = Vec::new();
    for threshold in [5, 8, 10, 4] {
        let mut request = StatementRequest::statement(format!(
            "DELETE FROM {table} WHERE id < :1 RETURNING id INTO :2"
        ));
        request.binds.push(BindValue::Number(threshold.to_string()));
        request.binds.push(BindValue::Out {
            column_type: OracleColumnType::Number,
            max_len: 22,
        });
        let result = conn
            .execute_out_binds_with_implicit(&request, &[])
            .expect("repeated DML RETURNING delete");
        let mut values = result
            .rows
            .iter()
            .map(|row| value_to_string(row.first().expect("RETURNING id")))
            .collect::<Vec<_>>();
        values.sort();
        results.push(values);
    }

    assert_eq!(
        results,
        vec![
            vec![
                "1".to_string(),
                "2".to_string(),
                "3".to_string(),
                "4".to_string()
            ],
            vec!["5".to_string(), "6".to_string(), "7".to_string()],
            vec!["8".to_string(), "9".to_string()],
            Vec::<String>::new(),
        ]
    );
}

#[test]
#[ignore = "requires local Oracle listener via ORACLE_THIN_TEST_* environment variables"]
fn dml_returning_followed_by_plsql_out_bind_uses_fresh_state() {
    let config = live_config();
    let table = unique_table_name("RET_REUSE");
    let _guard = TableDropGuard::new(config.clone(), table.clone());
    let mut conn = connect_with_config(config);
    conn.query_drop(&format!("CREATE TABLE {table} (id NUMBER PRIMARY KEY)"))
        .expect("create DML RETURNING reuse test table");

    let mut returning_request = StatementRequest::statement(format!(
        "INSERT INTO {table} (id) VALUES (:1) RETURNING id + 15 INTO :2"
    ));
    returning_request
        .binds
        .push(BindValue::Number("25".to_string()));
    returning_request.binds.push(BindValue::Out {
        column_type: OracleColumnType::Number,
        max_len: 22,
    });
    let returning_values = conn
        .execute_out_binds(&returning_request, &[])
        .expect("DML RETURNING before PL/SQL OUT bind");
    assert_eq!(
        rows_to_strings(&[returning_values]),
        vec![vec!["40".to_string()]]
    );

    let mut plsql_request = StatementRequest::statement("BEGIN :1 := :2 + 35; END;");
    plsql_request.is_plsql = true;
    plsql_request.binds.push(BindValue::Out {
        column_type: OracleColumnType::Number,
        max_len: 22,
    });
    plsql_request
        .binds
        .push(BindValue::Number("35".to_string()));
    let plsql_values = conn
        .execute_out_binds(&plsql_request, &[])
        .expect("PL/SQL OUT bind after DML RETURNING");
    assert_eq!(
        rows_to_strings(&[plsql_values]),
        vec![vec!["70".to_string()]]
    );
}

#[test]
#[ignore = "requires local Oracle listener via ORACLE_THIN_TEST_* environment variables"]
fn dml_returning_multiple_rows_preserves_all_out_bind_rows() {
    let config = live_config();
    let table = unique_table_name("RET_MULTI");
    let _guard = TableDropGuard::new(config.clone(), table.clone());
    let mut conn = connect_with_config(config);
    conn.query_drop(&format!(
        "CREATE TABLE {table} (id NUMBER PRIMARY KEY, name VARCHAR2(30))"
    ))
    .expect("create multi-row DML RETURNING test table");
    for (id, name) in [(1, "alpha"), (2, "beta"), (3, "gamma")] {
        conn.query_drop(&format!(
            "INSERT INTO {table} (id, name) VALUES ({id}, '{name}')"
        ))
        .expect("insert DML RETURNING source row");
    }

    let mut request = StatementRequest::statement(format!(
        "UPDATE {table} SET name = name || '_x' WHERE id <= 3 RETURNING id, name INTO :1, :2"
    ));
    request.binds.push(BindValue::Out {
        column_type: OracleColumnType::Number,
        max_len: 22,
    });
    request.binds.push(BindValue::Out {
        column_type: OracleColumnType::Varchar,
        max_len: 30,
    });

    let result = conn
        .execute_out_binds_with_implicit(&request, &[])
        .expect("multi-row DML RETURNING OUT binds");
    let mut rows = rows_to_strings(&result.rows);
    rows.sort();

    assert_eq!(
        rows,
        vec![
            vec!["1".to_string(), "alpha_x".to_string()],
            vec!["2".to_string(), "beta_x".to_string()],
            vec!["3".to_string(), "gamma_x".to_string()],
        ]
    );
    let values_row = rows_to_strings(&[result.values])
        .into_iter()
        .next()
        .unwrap_or_default();
    assert!(
        rows.contains(&values_row),
        "compatibility values should contain the first returned row"
    );
}

#[test]
#[ignore = "requires local Oracle listener via ORACLE_THIN_TEST_* environment variables"]
fn dml_returning_no_rows_returns_empty_out_bind_rows() {
    let config = live_config();
    let table = unique_table_name("RET_NONE");
    let _guard = TableDropGuard::new(config.clone(), table.clone());
    let mut conn = connect_with_config(config);
    conn.query_drop(&format!(
        "CREATE TABLE {table} (id NUMBER PRIMARY KEY, name VARCHAR2(30))"
    ))
    .expect("create no-row DML RETURNING test table");

    let mut request = StatementRequest::statement(format!(
        "UPDATE {table} SET name = 'unused' WHERE id = -1 RETURNING id, name INTO :1, :2"
    ));
    request.binds.push(BindValue::Out {
        column_type: OracleColumnType::Number,
        max_len: 22,
    });
    request.binds.push(BindValue::Out {
        column_type: OracleColumnType::Varchar,
        max_len: 30,
    });

    let result = conn
        .execute_out_binds_with_implicit(&request, &[])
        .expect("no-row DML RETURNING OUT binds");

    assert!(result.rows.is_empty());
    assert!(result.values.is_empty());
}

#[test]
#[ignore = "requires local Oracle listener via ORACLE_THIN_TEST_* environment variables"]
fn plsql_function_scalar_return_bind_returns_value() {
    let function_name = unique_object_name("FUNC_SCALAR");
    let mut conn = connect();
    drop_function_ignore(&mut conn, &function_name);
    conn.query_drop(&format!(
        "CREATE OR REPLACE FUNCTION {function_name}(p_value VARCHAR2) RETURN VARCHAR2 IS \
         BEGIN \
         RETURN 'FN-' || p_value; \
         END;"
    ))
    .expect("create scalar return function");

    let mut request =
        StatementRequest::statement(format!("BEGIN :1 := {function_name}(:2); END;"));
    request.binds.push(BindValue::Out {
        column_type: OracleColumnType::Varchar,
        max_len: 50,
    });
    request.binds.push(BindValue::Text("TXT".to_string()));

    let values = conn
        .execute_out_binds(&request, &[])
        .expect("PL/SQL function scalar return bind");

    assert_eq!(rows_to_strings(&[values]), vec![vec!["FN-TXT".to_string()]]);
    drop_function_ignore(&mut conn, &function_name);
}

#[test]
#[ignore = "requires local Oracle listener via ORACLE_THIN_TEST_* environment variables"]
fn plsql_function_ref_cursor_return_bind_fetches_rows() {
    let function_name = unique_object_name("FUNC_RC");
    let mut conn = connect();
    drop_function_ignore(&mut conn, &function_name);
    conn.query_drop(&format!(
        "CREATE OR REPLACE FUNCTION {function_name} RETURN SYS_REFCURSOR IS \
         rc SYS_REFCURSOR; \
         BEGIN \
         OPEN rc FOR SELECT 11 AS n, 'function ref cursor' AS label FROM dual; \
         RETURN rc; \
         END;"
    ))
    .expect("create ref cursor return function");

    let mut request = StatementRequest::statement(format!("BEGIN :1 := {function_name}(); END;"));
    request.binds.push(BindValue::Out {
        column_type: OracleColumnType::Cursor,
        max_len: 1,
    });

    let values = conn
        .execute_out_binds(&request, &[])
        .expect("PL/SQL function REF CURSOR return bind");
    let cursor = match values.first() {
        Some(OracleValue::Cursor(cursor)) => cursor.clone(),
        other => panic!("expected function return REF CURSOR, got {other:?}"),
    };
    let rows = conn
        .fetch_ref_cursor_all(cursor.cursor_id, cursor.columns, 10)
        .expect("fetch function return REF CURSOR rows");

    assert_eq!(
        rows_to_strings(&rows.result.rows),
        vec![vec![
            "11".to_string(),
            "function ref cursor".to_string()
        ]]
    );
    drop_function_ignore(&mut conn, &function_name);
}

#[test]
#[ignore = "requires local Oracle listener via ORACLE_THIN_TEST_* environment variables"]
fn plsql_procedure_ref_cursor_out_bind_fetches_mixed_scalar_columns() {
    let procedure_name = unique_object_name("PROC_RC_MIXED");
    let mut conn = connect();
    drop_procedure_ignore(&mut conn, &procedure_name);
    conn.query_drop(&format!(
        "CREATE OR REPLACE PROCEDURE {procedure_name}(p_rc OUT SYS_REFCURSOR) IS \
         BEGIN \
         OPEN p_rc FOR \
         SELECT TO_CHAR(DATE '2024-01-02', 'YYYY-MM-DD') AS c_text_func, \
                CAST('plain varchar' AS VARCHAR2(30)) AS c_varchar, \
                CAST(42 AS NUMBER) AS c_number, \
                TIMESTAMP '2024-01-02 03:04:05.123456' AS c_timestamp, \
                DATE '2024-01-03' AS c_date \
         FROM dual; \
         END;"
    ))
    .expect("create mixed scalar ref cursor procedure");

    let mut request = StatementRequest::statement(format!("BEGIN {procedure_name}(:1); END;"));
    request.binds.push(BindValue::Out {
        column_type: OracleColumnType::Cursor,
        max_len: 1,
    });

    let values = conn
        .execute_out_binds(&request, &[])
        .expect("PL/SQL procedure REF CURSOR OUT bind");
    let cursor = match values.first() {
        Some(OracleValue::Cursor(cursor)) => cursor.clone(),
        other => panic!("expected procedure OUT REF CURSOR, got {other:?}"),
    };
    assert_eq!(
        cursor
            .columns
            .iter()
            .map(|column| column.column_type)
            .collect::<Vec<_>>(),
        vec![
            OracleColumnType::Varchar,
            OracleColumnType::Varchar,
            OracleColumnType::Number,
            OracleColumnType::Timestamp,
            OracleColumnType::Date,
        ]
    );

    let rows = conn
        .fetch_ref_cursor_all(cursor.cursor_id, cursor.columns, 10)
        .expect("fetch mixed scalar REF CURSOR rows");
    let row = rows.result.rows.first().expect("mixed scalar row");
    assert_eq!(value_to_string(&row[0]), "2024-01-02");
    assert_eq!(value_to_string(&row[1]), "plain varchar");
    assert_eq!(value_to_string(&row[2]), "42");
    assert_eq!(timestamp_value_to_string(&row[3]), "2024-01-02 03:04:05.123456");
    assert_eq!(date_value_to_string(&row[4]), "2024-01-03 00:00:00");
    drop_procedure_ignore(&mut conn, &procedure_name);
}

#[test]
#[ignore = "requires local Oracle listener via ORACLE_THIN_TEST_* environment variables"]
fn plsql_procedure_ref_cursor_out_bind_fetches_mixed_wire_types() {
    let config = live_config();
    let table = unique_table_name("PROC_RC_TYPES_TAB");
    let procedure_name = unique_object_name("PROC_RC_TYPES");
    let _guard = TableDropGuard::new(config.clone(), table.clone());
    let mut conn = connect_with_config(config);
    drop_procedure_ignore(&mut conn, &procedure_name);
    conn.query_drop(&format!(
        "CREATE TABLE {table} (\
         id NUMBER PRIMARY KEY, \
         vc VARCHAR2(30), \
         nv NVARCHAR2(10), \
         raw_col RAW(4), \
         clob_col CLOB, \
         nclob_col NCLOB, \
         blob_col BLOB)"
    ))
    .expect("create mixed REF CURSOR type table");
    conn.query_drop(&format!(
        "INSERT INTO {table} \
         (id, vc, nv, raw_col, clob_col, nclob_col, blob_col) VALUES \
         (42, 'plain varchar', UNISTR('\\D55C\\AE00'), HEXTORAW('CAFE'), \
          TO_CLOB('CLOB-') || TO_CLOB(RPAD('x', 4000, 'x')), \
          TO_NCLOB(UNISTR('\\D55C\\AE00')), \
          TO_BLOB(HEXTORAW('DEADBEEF')))"
    ))
    .expect("insert mixed REF CURSOR type row");
    conn.query_drop(&format!(
        "CREATE OR REPLACE PROCEDURE {procedure_name}(p_rc OUT SYS_REFCURSOR) IS \
         BEGIN \
         OPEN p_rc FOR \
         SELECT id AS c_number, \
                vc AS c_varchar, \
                nv AS c_nvarchar, \
                DATE '2024-02-29' AS c_date, \
                TIMESTAMP '2024-01-02 03:04:05.123456' AS c_timestamp, \
                FROM_TZ(TIMESTAMP '2024-01-02 03:04:05.123456', '+05:45') AS c_tstz, \
                TO_YMINTERVAL('2021-10') AS c_iym, \
                TO_DSINTERVAL('2 12:23:34.456789') AS c_ids, \
                raw_col AS c_raw, \
                clob_col AS c_clob, \
                nclob_col AS c_nclob, \
                blob_col AS c_blob, \
                ROWID AS c_rowid, \
                CAST(ROWID AS UROWID) AS c_urowid \
         FROM {table} WHERE id = 42; \
         END;"
    ))
    .expect("create mixed REF CURSOR type procedure");

    let mut request = StatementRequest::statement(format!("BEGIN {procedure_name}(:1); END;"));
    request.binds.push(BindValue::Out {
        column_type: OracleColumnType::Cursor,
        max_len: 1,
    });
    let values = conn
        .execute_out_binds(&request, &[])
        .expect("PL/SQL mixed type REF CURSOR OUT bind");
    let cursor = match values.first() {
        Some(OracleValue::Cursor(cursor)) => cursor.clone(),
        other => panic!("expected mixed type OUT REF CURSOR, got {other:?}"),
    };
    assert_eq!(
        cursor
            .columns
            .iter()
            .map(|column| column.column_type)
            .collect::<Vec<_>>(),
        vec![
            OracleColumnType::Number,
            OracleColumnType::Varchar,
            OracleColumnType::Varchar,
            OracleColumnType::Date,
            OracleColumnType::Timestamp,
            OracleColumnType::Timestamp,
            OracleColumnType::IntervalYearMonth,
            OracleColumnType::IntervalDaySecond,
            OracleColumnType::Raw,
            OracleColumnType::Clob,
            OracleColumnType::Clob,
            OracleColumnType::Blob,
            OracleColumnType::Varchar,
            OracleColumnType::Varchar,
        ]
    );

    let rows = conn
        .fetch_ref_cursor_all(cursor.cursor_id, cursor.columns, 10)
        .expect("fetch mixed type REF CURSOR rows");
    let row = rows.result.rows.first().expect("mixed type row");
    assert_eq!(value_to_string(&row[0]), "42");
    assert_eq!(value_to_string(&row[1]), "plain varchar");
    assert_eq!(value_to_string(&row[2]), "\u{D55C}\u{AE00}");
    assert_eq!(date_value_to_string(&row[3]), "2024-02-29 00:00:00");
    assert_eq!(timestamp_value_to_string(&row[4]), "2024-01-02 03:04:05.123456");
    assert_eq!(timestamp_value_to_string(&row[5]), "2024-01-02 03:04:05.123456");
    assert_eq!(
        timestamp_value_timezone_suffix(&row[5]),
        Some("+05:45".to_string())
    );
    assert_eq!(value_to_string(&row[6]), "+2021-10");
    assert_eq!(value_to_string(&row[7]), "+02 12:23:34.456789");
    assert_eq!(row[8], OracleValue::Bytes(vec![0xca, 0xfe]));
    assert_eq!(value_to_string(&row[9]).len(), 4005);
    assert!(value_to_string(&row[9]).starts_with("CLOB-"));
    assert_eq!(value_to_string(&row[10]), "\u{D55C}\u{AE00}");
    assert_eq!(
        row[11],
        OracleValue::Bytes(vec![0xde, 0xad, 0xbe, 0xef])
    );
    let rowid = value_to_string(&row[12]);
    let urowid = value_to_string(&row[13]);
    assert_eq!(rowid, urowid);
    assert_eq!(rowid.len(), 18);

    drop_procedure_ignore(&mut conn, &procedure_name);
}

#[test]
#[ignore = "requires local Oracle listener via ORACLE_THIN_TEST_* environment variables"]
fn plsql_procedure_ref_cursor_out_bind_fetches_boolean_columns_when_supported() {
    let procedure_name = unique_object_name("PROC_RC_BOOL");
    let mut conn = connect();
    if !conn.capabilities().supports_sql_boolean {
        return;
    }
    drop_procedure_ignore(&mut conn, &procedure_name);
    conn.query_drop(&format!(
        "CREATE OR REPLACE PROCEDURE {procedure_name}(p_rc OUT SYS_REFCURSOR) IS \
         BEGIN \
         OPEN p_rc FOR \
         SELECT TRUE AS c_true, FALSE AS c_false, CAST(NULL AS BOOLEAN) AS c_null \
         FROM dual; \
         END;"
    ))
    .expect("create BOOLEAN REF CURSOR procedure");

    let mut request = StatementRequest::statement(format!("BEGIN {procedure_name}(:1); END;"));
    request.binds.push(BindValue::Out {
        column_type: OracleColumnType::Cursor,
        max_len: 1,
    });
    let values = conn
        .execute_out_binds(&request, &[])
        .expect("PL/SQL BOOLEAN REF CURSOR OUT bind");
    let cursor = match values.first() {
        Some(OracleValue::Cursor(cursor)) => cursor.clone(),
        other => panic!("expected BOOLEAN OUT REF CURSOR, got {other:?}"),
    };
    assert_eq!(
        cursor
            .columns
            .iter()
            .map(|column| column.column_type)
            .collect::<Vec<_>>(),
        vec![
            OracleColumnType::Boolean,
            OracleColumnType::Boolean,
            OracleColumnType::Boolean,
        ]
    );

    let rows = conn
        .fetch_ref_cursor_all(cursor.cursor_id, cursor.columns, 1)
        .expect("fetch BOOLEAN REF CURSOR rows");
    assert_eq!(
        rows.result.rows,
        vec![vec![
            OracleValue::Boolean(true),
            OracleValue::Boolean(false),
            OracleValue::Null,
        ]]
    );

    drop_procedure_ignore(&mut conn, &procedure_name);
}

#[test]
#[ignore = "requires local Oracle listener via ORACLE_THIN_TEST_* environment variables"]
fn plsql_procedure_ref_cursor_out_bind_fetches_long_wire_types() {
    let config = live_config();
    let long_table = unique_table_name("PROC_RC_LONG_TAB");
    let long_raw_table = unique_table_name("PROC_RC_LRAW_TAB");
    let procedure_name = unique_object_name("PROC_RC_LONGS");
    let _long_guard = TableDropGuard::new(config.clone(), long_table.clone());
    let _long_raw_guard = TableDropGuard::new(config.clone(), long_raw_table.clone());
    let mut conn = connect_with_config(config);
    drop_procedure_ignore(&mut conn, &procedure_name);
    conn.query_drop(&format!(
        "CREATE TABLE {long_table} (id NUMBER PRIMARY KEY, payload LONG)"
    ))
    .expect("create LONG REF CURSOR test table");
    conn.query_drop(&format!(
        "CREATE TABLE {long_raw_table} (id NUMBER PRIMARY KEY, payload LONG RAW)"
    ))
    .expect("create LONG RAW REF CURSOR test table");
    conn.query_drop(&format!(
        "INSERT INTO {long_table} (id, payload) VALUES (1, RPAD('L', 4000, 'L'))"
    ))
    .expect("insert LONG REF CURSOR test row");
    conn.query_drop(&format!(
        "INSERT INTO {long_raw_table} (id, payload) VALUES (1, HEXTORAW('DEADBEEF'))"
    ))
    .expect("insert LONG RAW REF CURSOR test row");
    conn.query_drop(&format!(
        "CREATE OR REPLACE PROCEDURE {procedure_name}(\
         p_long OUT SYS_REFCURSOR, p_long_raw OUT SYS_REFCURSOR) IS \
         BEGIN \
         OPEN p_long FOR SELECT payload FROM {long_table} WHERE id = 1; \
         OPEN p_long_raw FOR SELECT payload FROM {long_raw_table} WHERE id = 1; \
         END;"
    ))
    .expect("create LONG REF CURSOR procedure");

    let mut request = StatementRequest::statement(format!("BEGIN {procedure_name}(:1, :2); END;"));
    request.binds.push(BindValue::Out {
        column_type: OracleColumnType::Cursor,
        max_len: 1,
    });
    request.binds.push(BindValue::Out {
        column_type: OracleColumnType::Cursor,
        max_len: 1,
    });

    let values = conn
        .execute_out_binds(&request, &[])
        .expect("PL/SQL LONG REF CURSOR OUT binds");
    let long_cursor = match values.first() {
        Some(OracleValue::Cursor(cursor)) => cursor.clone(),
        other => panic!("expected LONG OUT REF CURSOR, got {other:?}"),
    };
    let long_raw_cursor = match values.get(1) {
        Some(OracleValue::Cursor(cursor)) => cursor.clone(),
        other => panic!("expected LONG RAW OUT REF CURSOR, got {other:?}"),
    };
    assert_eq!(
        long_cursor
            .columns
            .iter()
            .map(|column| column.column_type)
            .collect::<Vec<_>>(),
        vec![OracleColumnType::Long]
    );
    assert_eq!(
        long_raw_cursor
            .columns
            .iter()
            .map(|column| column.column_type)
            .collect::<Vec<_>>(),
        vec![OracleColumnType::Raw]
    );

    let long_rows = conn
        .fetch_ref_cursor_all(long_cursor.cursor_id, long_cursor.columns, 1)
        .expect("fetch LONG REF CURSOR rows");
    let long_payload = value_to_string(&long_rows.result.rows[0][0]);
    assert_eq!(long_payload.len(), 4000);
    assert!(long_payload.chars().all(|ch| ch == 'L'));

    let long_raw_rows = conn
        .fetch_ref_cursor_all(long_raw_cursor.cursor_id, long_raw_cursor.columns, 1)
        .expect("fetch LONG RAW REF CURSOR rows");
    assert_eq!(
        long_raw_rows.result.rows[0][0],
        OracleValue::Bytes(vec![0xde, 0xad, 0xbe, 0xef])
    );

    drop_procedure_ignore(&mut conn, &procedure_name);
}

#[test]
#[ignore = "requires local Oracle listener via ORACLE_THIN_TEST_* environment variables"]
fn plsql_procedure_ref_cursor_out_bind_fetches_nested_cursor_value() {
    let procedure_name = unique_object_name("PROC_RC_NESTED");
    let mut conn = connect();
    drop_procedure_ignore(&mut conn, &procedure_name);
    conn.query_drop(&format!(
        "CREATE OR REPLACE PROCEDURE {procedure_name}(p_rc OUT SYS_REFCURSOR) IS \
         BEGIN \
         OPEN p_rc FOR \
         SELECT 7 AS parent_id, \
                CURSOR(\
                    SELECT CAST('child' AS VARCHAR2(20)) AS child_label, \
                           CAST(RPAD('N', 4000, 'N') AS VARCHAR2(4000)) AS child_payload \
                    FROM dual\
                ) AS child_cursor \
         FROM dual; \
         END;"
    ))
    .expect("create nested cursor REF CURSOR procedure");

    let mut request = StatementRequest::statement(format!("BEGIN {procedure_name}(:1); END;"));
    request.binds.push(BindValue::Out {
        column_type: OracleColumnType::Cursor,
        max_len: 1,
    });

    let values = conn
        .execute_out_binds(&request, &[])
        .expect("PL/SQL nested cursor REF CURSOR OUT bind");
    let outer_cursor = match values.first() {
        Some(OracleValue::Cursor(cursor)) => cursor.clone(),
        other => panic!("expected nested OUT REF CURSOR, got {other:?}"),
    };
    assert_eq!(
        outer_cursor
            .columns
            .iter()
            .map(|column| column.column_type)
            .collect::<Vec<_>>(),
        vec![OracleColumnType::Number, OracleColumnType::Cursor]
    );

    let outer_rows = conn
        .fetch_ref_cursor_all(outer_cursor.cursor_id, outer_cursor.columns, 1)
        .expect("fetch outer REF CURSOR with nested cursor");
    let outer_row = outer_rows
        .result
        .rows
        .first()
        .expect("outer nested cursor row");
    assert_eq!(value_to_string(&outer_row[0]), "7");
    let child_cursor = match &outer_row[1] {
        OracleValue::Cursor(cursor) => cursor.clone(),
        other => panic!("expected nested cursor value, got {other:?}"),
    };
    assert_eq!(
        child_cursor
            .columns
            .iter()
            .map(|column| column.column_type)
            .collect::<Vec<_>>(),
        vec![OracleColumnType::Varchar, OracleColumnType::Varchar]
    );
    assert!(
        child_cursor.columns[1].buffer_size >= 4000,
        "nested VARCHAR2(4000) describe should preserve buffer, got {}",
        child_cursor.columns[1].buffer_size
    );

    let child_rows = conn
        .fetch_ref_cursor_all(child_cursor.cursor_id, child_cursor.columns, 1)
        .expect("fetch nested child cursor after parent close piggyback");
    let child_row = child_rows
        .result
        .rows
        .first()
        .expect("nested child cursor row");
    assert_eq!(value_to_string(&child_row[0]), "child");
    assert_eq!(value_to_string(&child_row[1]).len(), 4000);
    assert!(value_to_string(&child_row[1]).chars().all(|ch| ch == 'N'));

    drop_procedure_ignore(&mut conn, &procedure_name);
}

#[test]
#[ignore = "requires local Oracle listener via ORACLE_THIN_TEST_* environment variables"]
fn plsql_nested_cursor_manual_close_releases_parent_on_next_call() {
    let procedure_name = unique_object_name("PROC_RC_NCLOSE");
    let mut conn = connect();
    drop_procedure_ignore(&mut conn, &procedure_name);
    conn.query_drop(&format!(
        "CREATE OR REPLACE PROCEDURE {procedure_name}(p_rc OUT SYS_REFCURSOR) IS \
         BEGIN \
         OPEN p_rc FOR \
         SELECT CURSOR(SELECT 99 AS child_value FROM dual) AS child_cursor \
         FROM dual; \
         END;"
    ))
    .expect("create nested cursor manual close procedure");

    let mut request = StatementRequest::statement(format!("BEGIN {procedure_name}(:1); END;"));
    request.binds.push(BindValue::Out {
        column_type: OracleColumnType::Cursor,
        max_len: 1,
    });
    let values = conn
        .execute_out_binds(&request, &[])
        .expect("PL/SQL nested cursor manual close OUT bind");
    let outer_cursor = match values.first() {
        Some(OracleValue::Cursor(cursor)) => cursor.clone(),
        other => panic!("expected nested close OUT REF CURSOR, got {other:?}"),
    };
    let outer_rows = conn
        .fetch_ref_cursor_all(outer_cursor.cursor_id, outer_cursor.columns, 1)
        .expect("fetch outer cursor for manual nested close");
    let child_cursor = match &outer_rows.result.rows[0][0] {
        OracleValue::Cursor(cursor) => cursor.clone(),
        other => panic!("expected nested child cursor, got {other:?}"),
    };

    conn.close_cursor_on_next_call(Some(child_cursor.cursor_id));
    conn.query_drop("SELECT 1 FROM dual")
        .expect("next call should close child and deferred parent without ORA-01001");

    drop_procedure_ignore(&mut conn, &procedure_name);
}

#[test]
#[ignore = "requires local Oracle listener via ORACLE_THIN_TEST_* environment variables"]
fn plsql_procedure_ref_cursor_out_bind_fetches_varchar2_4000_batches() {
    let procedure_name = unique_object_name("PROC_RC_VC4000");
    let mut conn = connect();
    drop_procedure_ignore(&mut conn, &procedure_name);
    conn.query_drop(&format!(
        "CREATE OR REPLACE PROCEDURE {procedure_name}(p_rc OUT SYS_REFCURSOR) IS \
         BEGIN \
         OPEN p_rc FOR \
         SELECT level AS n, \
                CAST(RPAD(TO_CHAR(level), 4000, TO_CHAR(level)) AS VARCHAR2(4000)) AS ascii_payload, \
                CAST(REPLACE(RPAD('x', 1333, 'x'), 'x', UNISTR('\\D55C')) AS VARCHAR2(4000)) AS utf8_payload, \
                CAST('tail-' || TO_CHAR(level) AS VARCHAR2(20)) AS tail \
         FROM dual CONNECT BY level <= 3; \
         END;"
    ))
    .expect("create VARCHAR2(4000) REF CURSOR procedure");

    let mut request = StatementRequest::statement(format!("BEGIN {procedure_name}(:1); END;"));
    request.fetch_array_size = 1;
    request.binds.push(BindValue::Out {
        column_type: OracleColumnType::Cursor,
        max_len: 1,
    });
    let values = conn
        .execute_out_binds(&request, &[])
        .expect("PL/SQL VARCHAR2(4000) REF CURSOR OUT bind");
    let cursor = match values.first() {
        Some(OracleValue::Cursor(cursor)) => cursor.clone(),
        other => panic!("expected VARCHAR2(4000) OUT REF CURSOR, got {other:?}"),
    };
    assert_eq!(
        cursor
            .columns
            .iter()
            .map(|column| column.column_type)
            .collect::<Vec<_>>(),
        vec![
            OracleColumnType::Number,
            OracleColumnType::Varchar,
            OracleColumnType::Varchar,
            OracleColumnType::Varchar,
        ]
    );
    assert!(
        cursor.columns[1].buffer_size >= 4000,
        "VARCHAR2(4000) describe should preserve a large enough buffer, got {}",
        cursor.columns[1].buffer_size
    );

    let first = conn
        .fetch_ref_cursor_batch(cursor.cursor_id, &cursor.columns, 1, false)
        .expect("fetch first VARCHAR2(4000) REF CURSOR batch");
    assert_ref_cursor_varchar2_4000_row(&first.rows[0], "1");

    let second = conn
        .fetch_ref_cursor_batch(cursor.cursor_id, &cursor.columns, 1, false)
        .expect("fetch second VARCHAR2(4000) REF CURSOR batch");
    assert_ref_cursor_varchar2_4000_row(&second.rows[0], "2");

    let remaining = conn
        .fetch_ref_cursor_all(cursor.cursor_id, cursor.columns, 1)
        .expect("fetch remaining VARCHAR2(4000) REF CURSOR rows");
    assert_eq!(remaining.result.rows.len(), 1);
    assert_ref_cursor_varchar2_4000_row(&remaining.result.rows[0], "3");

    drop_procedure_ignore(&mut conn, &procedure_name);
}

#[test]
#[ignore = "requires local Oracle listener via ORACLE_THIN_TEST_* environment variables"]
fn plsql_procedure_ref_cursor_out_bind_fetches_json_column() {
    let config = live_config();
    let table = unique_table_name("RCJSON");
    let procedure_name = unique_object_name("RCJSONP");
    let _guard = TableDropGuard::new(config.clone(), table.clone());
    let mut conn = connect_with_config(config);
    drop_procedure_ignore(&mut conn, &procedure_name);

    match conn.query_drop(&format!(
        "CREATE TABLE {table} (id NUMBER PRIMARY KEY, doc JSON) TABLESPACE USERS"
    )) {
        Ok(()) => {}
        Err(err)
            if err.to_string().contains("ORA-00902")
                || err.to_string().contains("ORA-00959")
                || err.to_string().contains("ORA-03001")
                || err.to_string().contains("ORA-43853") =>
        {
            eprintln!("skipping JSON REF CURSOR test: database does not support native JSON");
            return;
        }
        Err(err) => panic!("create JSON REF CURSOR test table: {err}"),
    }
    conn.query_drop(&format!(
        "INSERT INTO {table} (id, doc) \
         SELECT 1, JSON_OBJECT(\
             KEY 'a' VALUE 1, \
             KEY 'b' VALUE JSON_ARRAY(2, 'x'), \
             KEY 'flag' VALUE 'true' FORMAT JSON \
             RETURNING JSON\
         ) FROM dual"
    ))
    .expect("insert JSON REF CURSOR test row");
    conn.query_drop(&format!(
        "CREATE OR REPLACE PROCEDURE {procedure_name}(p_rc OUT SYS_REFCURSOR) IS \
         BEGIN \
         OPEN p_rc FOR SELECT doc FROM {table} WHERE id = 1; \
         END;"
    ))
    .expect("create JSON REF CURSOR procedure");

    let mut request = StatementRequest::statement(format!("BEGIN {procedure_name}(:1); END;"));
    request.binds.push(BindValue::Out {
        column_type: OracleColumnType::Cursor,
        max_len: 1,
    });
    let values = conn
        .execute_out_binds(&request, &[])
        .expect("PL/SQL JSON REF CURSOR OUT bind");
    let cursor = match values.first() {
        Some(OracleValue::Cursor(cursor)) => cursor.clone(),
        other => panic!("expected JSON OUT REF CURSOR, got {other:?}"),
    };
    assert_eq!(
        cursor
            .columns
            .iter()
            .map(|column| column.column_type)
            .collect::<Vec<_>>(),
        vec![OracleColumnType::Json]
    );

    let rows = match conn.fetch_ref_cursor_all(cursor.cursor_id, cursor.columns, 1) {
        Ok(rows) => rows,
        Err(err)
            if conn.capabilities().protocol_version == Some(314)
                && err.to_string().contains("ORA-40569") =>
        {
            eprintln!(
                "skipping JSON REF CURSOR fetch test: protocol 314 server rejected native JSON REF CURSOR fetch"
            );
            return;
        }
        Err(err) => panic!("fetch JSON REF CURSOR rows: {err}"),
    };
    assert_eq!(
        rows_to_strings(&rows.result.rows),
        vec![vec![r#"{"a":1,"b":[2,"x"],"flag":true}"#.to_string()]]
    );

    drop_procedure_ignore(&mut conn, &procedure_name);
}

#[test]
#[ignore = "requires local Oracle listener via ORACLE_THIN_TEST_* environment variables"]
fn plsql_procedure_ref_cursor_out_bind_fetches_extended_wire_types() {
    let procedure_name = unique_object_name("RCEXT");
    let mut conn = connect();
    conn.query_drop("ALTER SESSION SET TIME_ZONE = '+00:00'")
        .expect("set deterministic session time zone");
    drop_procedure_ignore(&mut conn, &procedure_name);
    match conn.query_drop(&format!(
        "CREATE OR REPLACE PROCEDURE {procedure_name}(p_rc OUT SYS_REFCURSOR) IS \
         BEGIN \
         OPEN p_rc FOR \
         SELECT CAST(3.5 AS BINARY_FLOAT) AS c_bfloat, \
                CAST(-2.25 AS BINARY_DOUBLE) AS c_bdouble, \
                CAST('abc' AS CHAR(5)) AS c_char, \
                CAST(UNISTR('\\D55C\\AE00') AS NCHAR(2)) AS c_nchar, \
                CAST(TIMESTAMP '2024-01-02 03:04:05.123456' AS TIMESTAMP WITH LOCAL TIME ZONE) AS c_tsltz, \
                XMLTYPE('<root><n>7</n><txt>' || UNISTR('\\D55C\\AE00') || '</txt></root>') AS c_xml, \
                TO_VECTOR('[1, 2, 3]', 3, FLOAT32) AS c_vector, \
                TO_VECTOR('[16, [1, 3, 5], [1, 0, 5]]', 16, FLOAT32, SPARSE) AS c_sparse_vector, \
                BFILENAME('DATA_PUMP_DIR', 'space_query_bfile_probe.bin') AS c_bfile \
         FROM dual; \
         END;"
    )) {
        Ok(()) => {}
        Err(err)
            if err.to_string().contains("ORA-00902")
                || err.to_string().contains("ORA-00904")
                || err.to_string().contains("ORA-03001")
                || err.to_string().contains("ORA-06550")
                || err.to_string().contains("ORA-518") =>
        {
            eprintln!(
                "skipping extended REF CURSOR type test: database does not support one of XML/VECTOR/BFILE/TSLTZ"
            );
            return;
        }
        Err(err) => panic!("create extended REF CURSOR type procedure: {err}"),
    }

    let mut request = StatementRequest::statement(format!("BEGIN {procedure_name}(:1); END;"));
    request.binds.push(BindValue::Out {
        column_type: OracleColumnType::Cursor,
        max_len: 1,
    });
    let values = conn
        .execute_out_binds(&request, &[])
        .expect("PL/SQL extended type REF CURSOR OUT bind");
    let cursor = match values.first() {
        Some(OracleValue::Cursor(cursor)) => cursor.clone(),
        other => panic!("expected extended type OUT REF CURSOR, got {other:?}"),
    };
    assert_eq!(
        cursor
            .columns
            .iter()
            .map(|column| column.column_type)
            .collect::<Vec<_>>(),
        vec![
            OracleColumnType::Number,
            OracleColumnType::Number,
            OracleColumnType::Varchar,
            OracleColumnType::Varchar,
            OracleColumnType::Timestamp,
            OracleColumnType::Xml,
            OracleColumnType::Vector,
            OracleColumnType::Vector,
            OracleColumnType::Bfile,
        ]
    );

    let rows = conn
        .fetch_ref_cursor_all(cursor.cursor_id, cursor.columns, 1)
        .expect("fetch extended type REF CURSOR rows");
    let row = rows.result.rows.first().expect("extended type row");
    assert_eq!(value_to_string(&row[0]), "3.5");
    assert_eq!(value_to_string(&row[1]), "-2.25");
    assert_eq!(value_to_string(&row[2]), "abc  ");
    assert_eq!(value_to_string(&row[3]), "\u{D55C}\u{AE00}");
    assert_eq!(timestamp_value_to_string(&row[4]), "2024-01-02 03:04:05.123456");
    assert!(value_to_string(&row[5]).contains("<n>7</n>"));
    assert!(value_to_string(&row[5]).contains("\u{D55C}\u{AE00}"));
    assert_eq!(value_to_string(&row[6]), "[1, 2, 3]");
    assert_eq!(
        value_to_string(&row[7]),
        "SparseVector(dimensions=16, indices=[1, 3, 5], values=[1, 0, 5])"
    );
    match &row[8] {
        OracleValue::Lob(locator) => assert!(
            !locator.is_empty(),
            "BFILE REF CURSOR locator should include non-empty locator bytes"
        ),
        other => panic!("expected BFILE REF CURSOR locator, got {other:?}"),
    }

    drop_procedure_ignore(&mut conn, &procedure_name);
}

#[test]
#[ignore = "requires local Oracle listener via ORACLE_THIN_TEST_* environment variables"]
fn plsql_procedure_ref_cursor_out_bind_preserves_udt_metadata() {
    let config = live_config();
    let nested_type_name = unique_object_name("RCUDT_CHILD");
    let type_name = unique_object_name("RCUDT_OBJ");
    let procedure_name = unique_object_name("RCUDT");
    let _nested_guard = TypeDropGuard::new(config.clone(), nested_type_name.clone());
    let _guard = TypeDropGuard::new(config.clone(), type_name.clone());
    let mut conn = connect_with_config(config);
    drop_procedure_ignore(&mut conn, &procedure_name);
    drop_type_ignore(&mut conn, &type_name);
    drop_type_ignore(&mut conn, &nested_type_name);
    conn.query_drop(&format!(
        "CREATE TYPE {nested_type_name} AS OBJECT (\
         child_id NUMBER, \
         child_label VARCHAR2(30))"
    ))
    .expect("create nested UDT type");
    conn.query_drop(&format!(
        "CREATE TYPE {type_name} AS OBJECT (\
         id NUMBER, \
         payload VARCHAR2(4000), \
         raw_payload RAW(4), \
         created_on DATE, \
         stamped_at TIMESTAMP, \
         score_float BINARY_FLOAT, \
         score_double BINARY_DOUBLE, \
         active BOOLEAN, \
         inactive BOOLEAN, \
         period_ym INTERVAL YEAR(4) TO MONTH, \
         period_ds INTERVAL DAY TO SECOND, \
         clob_payload CLOB, \
         blob_payload BLOB, \
         file_payload BFILE, \
         xml_payload XMLTYPE, \
         child {nested_type_name})"
    ))
    .expect("create UDT type");
    conn.query_drop(&format!(
        "CREATE OR REPLACE PROCEDURE {procedure_name}(p_rc OUT SYS_REFCURSOR) IS \
         BEGIN \
         OPEN p_rc FOR \
         SELECT obj \
         FROM (\
             SELECT 1 AS sort_key, \
                    {type_name}(\
                        7, \
                        CAST(RPAD('U', 4000, 'U') AS VARCHAR2(4000)), \
                        HEXTORAW('CAFE'), \
                        DATE '2024-02-29', \
                        TIMESTAMP '2024-01-02 03:04:05.123456', \
                        CAST(3.5 AS BINARY_FLOAT), \
                        CAST(-2.25 AS BINARY_DOUBLE), \
                        TRUE, \
                        FALSE, \
                        TO_YMINTERVAL('2021-10'), \
                        TO_DSINTERVAL('2 12:23:34.456789'), \
                        TO_CLOB('OBJECT-CLOB'), \
                        TO_BLOB(HEXTORAW('BEEF')), \
                        BFILENAME('DATA_PUMP_DIR', 'space_query_bfile_probe.bin'), \
                        XMLTYPE('<root><kind>object</kind><txt>' || UNISTR('\\D55C') || '</txt></root>'), \
                        {nested_type_name}(99, 'nested child')\
                    ) AS obj \
             FROM dual \
             UNION ALL \
             SELECT 2 AS sort_key, CAST(NULL AS {type_name}) AS obj \
             FROM dual\
         ) \
         ORDER BY sort_key; \
         END;"
    ))
    .expect("create UDT REF CURSOR procedure");

    let mut request = StatementRequest::statement(format!("BEGIN {procedure_name}(:1); END;"));
    request.binds.push(BindValue::Out {
        column_type: OracleColumnType::Cursor,
        max_len: 1,
    });
    let values = conn
        .execute_out_binds(&request, &[])
        .expect("PL/SQL UDT REF CURSOR OUT bind");
    let cursor = match values.first() {
        Some(OracleValue::Cursor(cursor)) => cursor.clone(),
        other => panic!("expected UDT OUT REF CURSOR, got {other:?}"),
    };
    let column = cursor.columns.first().expect("UDT cursor column");
    assert_eq!(column.ora_type_num, 109);
    assert_eq!(column.type_name, type_name);
    assert!(
        !column.schema_name.is_empty(),
        "UDT metadata should preserve the owning schema"
    );

    let rows = conn
        .fetch_ref_cursor_all(cursor.cursor_id, cursor.columns, 1)
        .expect("fetch UDT REF CURSOR rows");
    assert_eq!(rows.result.rows.len(), 2);
    let object_attrs = match &rows.result.rows[0][0] {
        OracleValue::Object(attrs) => attrs,
        other => panic!("expected decoded UDT object, got {other:?}"),
    };
    assert_eq!(object_attrs[0].0, "ID");
    assert_eq!(object_attrs[0].1, OracleValue::Number("7".to_string()));
    assert_eq!(object_attrs[1].0, "PAYLOAD");
    let payload = value_to_string(&object_attrs[1].1);
    assert_eq!(payload.len(), 4000);
    assert!(payload.chars().all(|ch| ch == 'U'));
    assert_eq!(object_attrs[2].0, "RAW_PAYLOAD");
    assert_eq!(object_attrs[2].1, OracleValue::Bytes(vec![0xca, 0xfe]));
    assert_eq!(object_attrs[3].0, "CREATED_ON");
    assert_eq!(date_value_to_string(&object_attrs[3].1), "2024-02-29 00:00:00");
    assert_eq!(object_attrs[4].0, "STAMPED_AT");
    assert_eq!(
        timestamp_value_to_string(&object_attrs[4].1),
        "2024-01-02 03:04:05.123456"
    );
    assert_eq!(object_attrs[5].0, "SCORE_FLOAT");
    assert_eq!(value_to_string(&object_attrs[5].1), "3.5");
    assert_eq!(object_attrs[6].0, "SCORE_DOUBLE");
    assert_eq!(value_to_string(&object_attrs[6].1), "-2.25");
    assert_eq!(object_attrs[7].0, "ACTIVE");
    assert_eq!(object_attrs[7].1, OracleValue::Boolean(true));
    assert_eq!(object_attrs[8].0, "INACTIVE");
    assert_eq!(object_attrs[8].1, OracleValue::Boolean(false));
    assert_eq!(object_attrs[9].0, "PERIOD_YM");
    assert_eq!(value_to_string(&object_attrs[9].1), "+2021-10");
    assert_eq!(object_attrs[10].0, "PERIOD_DS");
    assert_eq!(value_to_string(&object_attrs[10].1), "+02 12:23:34.456789");
    assert_eq!(object_attrs[11].0, "CLOB_PAYLOAD");
    assert_lob_value_not_empty(&object_attrs[11].1);
    assert_eq!(object_attrs[12].0, "BLOB_PAYLOAD");
    assert_lob_value_not_empty(&object_attrs[12].1);
    assert_eq!(object_attrs[13].0, "FILE_PAYLOAD");
    assert_lob_value_not_empty(&object_attrs[13].1);
    assert_eq!(object_attrs[14].0, "XML_PAYLOAD");
    let xml_payload = value_to_string(&object_attrs[14].1);
    assert!(xml_payload.contains("<kind>object</kind>"));
    assert!(xml_payload.contains("\u{D55C}"));
    assert_eq!(object_attrs[15].0, "CHILD");
    let child_attrs = match &object_attrs[15].1 {
        OracleValue::Object(attrs) => attrs,
        other => panic!("expected decoded nested UDT object, got {other:?}"),
    };
    assert_eq!(child_attrs[0].0, "CHILD_ID");
    assert_eq!(child_attrs[0].1, OracleValue::Number("99".to_string()));
    assert_eq!(child_attrs[1].0, "CHILD_LABEL");
    assert_eq!(value_to_string(&child_attrs[1].1), "nested child");
    assert_eq!(rows.result.rows[1][0], OracleValue::Null);

    drop_procedure_ignore(&mut conn, &procedure_name);
}

#[test]
#[ignore = "requires local Oracle listener via ORACLE_THIN_TEST_* environment variables"]
fn plsql_procedure_ref_cursor_out_bind_decodes_udt_collection_attribute() {
    let config = live_config();
    let child_type_name = unique_object_name("RCUDT_COLL_CHILD");
    let collection_type_name = unique_object_name("RCUDT_COLL_TAB");
    let parent_type_name = unique_object_name("RCUDT_COLL_PARENT");
    let procedure_name = unique_object_name("RCUDT_COLL");
    let _child_guard = TypeDropGuard::new(config.clone(), child_type_name.clone());
    let _collection_guard = TypeDropGuard::new(config.clone(), collection_type_name.clone());
    let _parent_guard = TypeDropGuard::new(config.clone(), parent_type_name.clone());
    let mut conn = connect_with_config(config);
    drop_procedure_ignore(&mut conn, &procedure_name);
    drop_type_ignore(&mut conn, &parent_type_name);
    drop_type_ignore(&mut conn, &collection_type_name);
    drop_type_ignore(&mut conn, &child_type_name);
    conn.query_drop(&format!(
        "CREATE TYPE {child_type_name} AS OBJECT (child_id NUMBER)"
    ))
    .expect("create collection element UDT type");
    conn.query_drop(&format!(
        "CREATE TYPE {collection_type_name} AS TABLE OF {child_type_name}"
    ))
    .expect("create UDT collection type");
    conn.query_drop(&format!(
        "CREATE TYPE {parent_type_name} AS OBJECT (items {collection_type_name})"
    ))
    .expect("create UDT parent type with collection attribute");
    conn.query_drop(&format!(
        "CREATE OR REPLACE PROCEDURE {procedure_name}(p_rc OUT SYS_REFCURSOR) IS \
         BEGIN \
         OPEN p_rc FOR \
         SELECT {parent_type_name}({collection_type_name}({child_type_name}(1))) AS obj \
         FROM dual; \
         END;"
    ))
    .expect("create UDT collection REF CURSOR procedure");

    let mut request = StatementRequest::statement(format!("BEGIN {procedure_name}(:1); END;"));
    request.binds.push(BindValue::Out {
        column_type: OracleColumnType::Cursor,
        max_len: 1,
    });
    let values = conn
        .execute_out_binds(&request, &[])
        .expect("PL/SQL UDT collection REF CURSOR OUT bind");
    let cursor = match values.first() {
        Some(OracleValue::Cursor(cursor)) => cursor.clone(),
        other => panic!("expected UDT collection OUT REF CURSOR, got {other:?}"),
    };
    let rows = conn
        .fetch_ref_cursor_all(cursor.cursor_id, cursor.columns, 1)
        .expect("fetch UDT collection REF CURSOR rows");
    assert_eq!(rows.result.rows.len(), 1);
    let object_attrs = match &rows.result.rows[0][0] {
        OracleValue::Object(attrs) => attrs,
        other => panic!("expected decoded UDT parent object, got {other:?}"),
    };
    assert_eq!(object_attrs[0].0, "ITEMS");
    let collection_values = match &object_attrs[0].1 {
        OracleValue::Array(values) => values,
        other => panic!("expected decoded UDT collection attribute, got {other:?}"),
    };
    assert_eq!(collection_values.len(), 1);
    let child_attrs = match &collection_values[0] {
        OracleValue::Object(attrs) => attrs,
        other => panic!("expected decoded UDT collection element object, got {other:?}"),
    };
    assert_eq!(child_attrs[0].0, "CHILD_ID");
    assert_eq!(child_attrs[0].1, OracleValue::Number("1".to_string()));

    drop_procedure_ignore(&mut conn, &procedure_name);
}

#[test]
#[ignore = "requires local Oracle listener via ORACLE_THIN_TEST_* environment variables"]
fn plsql_procedure_ref_cursor_out_bind_decodes_top_level_scalar_collection() {
    let config = live_config();
    let collection_type_name = unique_object_name("RCUDT_VC_TAB");
    let procedure_name = unique_object_name("RCUDT_VC_TAB");
    let _collection_guard = TypeDropGuard::new(config.clone(), collection_type_name.clone());
    let mut conn = connect_with_config(config);
    drop_procedure_ignore(&mut conn, &procedure_name);
    drop_type_ignore(&mut conn, &collection_type_name);
    conn.query_drop(&format!(
        "CREATE TYPE {collection_type_name} AS TABLE OF VARCHAR2(4000)"
    ))
    .expect("create VARCHAR2 collection type");
    conn.query_drop(&format!(
        "CREATE OR REPLACE PROCEDURE {procedure_name}(p_rc OUT SYS_REFCURSOR) IS \
         BEGIN \
         OPEN p_rc FOR \
         SELECT items \
         FROM (\
             SELECT 1 AS sort_key, \
                    {collection_type_name}(\
                        CAST(RPAD('C', 4000, 'C') AS VARCHAR2(4000)), \
                        NULL, \
                        'tail'\
                    ) AS items \
             FROM dual \
             UNION ALL \
             SELECT 2 AS sort_key, CAST(NULL AS {collection_type_name}) AS items \
             FROM dual\
         ) \
         ORDER BY sort_key; \
         END;"
    ))
    .expect("create scalar collection REF CURSOR procedure");

    let mut request = StatementRequest::statement(format!("BEGIN {procedure_name}(:1); END;"));
    request.binds.push(BindValue::Out {
        column_type: OracleColumnType::Cursor,
        max_len: 1,
    });
    let values = conn
        .execute_out_binds(&request, &[])
        .expect("PL/SQL scalar collection REF CURSOR OUT bind");
    let cursor = match values.first() {
        Some(OracleValue::Cursor(cursor)) => cursor.clone(),
        other => panic!("expected scalar collection OUT REF CURSOR, got {other:?}"),
    };
    let column = cursor.columns.first().expect("scalar collection cursor column");
    assert_eq!(column.ora_type_num, 109);
    assert_eq!(column.type_name, collection_type_name);
    let rows = conn
        .fetch_ref_cursor_all(cursor.cursor_id, cursor.columns, 1)
        .expect("fetch scalar collection REF CURSOR rows");
    assert_eq!(rows.result.rows.len(), 2);
    let collection_values = match &rows.result.rows[0][0] {
        OracleValue::Array(values) => values,
        other => panic!("expected decoded scalar collection, got {other:?}"),
    };
    assert_eq!(collection_values.len(), 3);
    let payload = value_to_string(&collection_values[0]);
    assert_eq!(payload.len(), 4000);
    assert!(payload.chars().all(|ch| ch == 'C'));
    assert_eq!(collection_values[1], OracleValue::Null);
    assert_eq!(collection_values[2], OracleValue::Text("tail".to_string()));
    assert_eq!(rows.result.rows[1][0], OracleValue::Null);

    drop_procedure_ignore(&mut conn, &procedure_name);
}

#[test]
#[ignore = "requires local Oracle listener via ORACLE_THIN_TEST_* environment variables"]
fn plsql_implicit_resultset_fetches_rows_when_supported() {
    let mut conn = connect();
    if !conn.capabilities().supports_implicit_resultsets {
        return;
    }
    let request = StatementRequest::statement(
        "DECLARE \
         rc SYS_REFCURSOR; \
         BEGIN \
         OPEN rc FOR SELECT 8 AS n, 'implicit' AS label FROM dual; \
         DBMS_SQL.RETURN_RESULT(rc); \
         END;",
    );

    let mut outcome = conn
        .execute_typed_with_implicit(&request, &[])
        .expect("PL/SQL implicit resultset");
    assert!(
        outcome.result.rows.is_empty(),
        "implicit result call should not emit statement rows"
    );
    let cursor = outcome
        .implicit_results
        .pop()
        .expect("implicit result cursor");
    let rows = conn
        .fetch_ref_cursor_all(cursor.cursor_id, cursor.columns, 10)
        .expect("fetch implicit resultset rows");

    assert_eq!(
        rows_to_strings(&rows.result.rows),
        vec![vec!["8".to_string(), "implicit".to_string()]]
    );
}

#[test]
#[ignore = "requires local Oracle listener via ORACLE_THIN_TEST_* environment variables"]
fn plsql_multiple_implicit_resultsets_fetch_in_order_when_supported() {
    let mut conn = connect();
    if !conn.capabilities().supports_implicit_resultsets {
        return;
    }
    let request = StatementRequest::statement(
        "DECLARE \
         rc1 SYS_REFCURSOR; \
         rc2 SYS_REFCURSOR; \
         BEGIN \
         OPEN rc1 FOR SELECT 1 AS id, 'first' AS label FROM dual; \
         DBMS_SQL.RETURN_RESULT(rc1); \
         OPEN rc2 FOR SELECT 2 AS id, 'second' AS label FROM dual; \
         DBMS_SQL.RETURN_RESULT(rc2); \
         END;",
    );

    let outcome = conn
        .execute_typed_with_implicit(&request, &[])
        .expect("PL/SQL multiple implicit resultsets");
    assert!(
        outcome.result.rows.is_empty(),
        "implicit result call should not emit statement rows"
    );
    assert_eq!(
        outcome.implicit_results.len(),
        2,
        "expected two implicit result cursors"
    );
    let mut cursors = outcome.implicit_results.into_iter();
    let first = cursors.next().expect("first implicit cursor");
    let second = cursors.next().expect("second implicit cursor");

    let first_rows = conn
        .fetch_ref_cursor_all(first.cursor_id, first.columns, 10)
        .expect("fetch first implicit resultset rows");
    let second_rows = conn
        .fetch_ref_cursor_all(second.cursor_id, second.columns, 10)
        .expect("fetch second implicit resultset rows");

    assert_eq!(
        rows_to_strings(&first_rows.result.rows),
        vec![vec!["1".to_string(), "first".to_string()]]
    );
    assert_eq!(
        rows_to_strings(&second_rows.result.rows),
        vec![vec!["2".to_string(), "second".to_string()]]
    );
}

#[test]
#[ignore = "requires local Oracle listener via ORACLE_THIN_TEST_* environment variables"]
fn fetch_rowid_and_urowid_match_oracle_text_encoding() {
    let config = live_config();
    let table = unique_table_name("RID");
    let _guard = TableDropGuard::new(config.clone(), table.clone());
    let mut conn = connect_with_config(config);

    conn.query_drop(&format!("CREATE TABLE {table} (id NUMBER PRIMARY KEY)"))
        .expect("create ROWID test table");
    conn.query_drop(&format!("INSERT INTO {table} VALUES (1)"))
        .expect("insert ROWID test row");

    let result = conn
        .query_described_fetch_all(
            format!(
                "SELECT ROWID AS rid, \
                 ROWIDTOCHAR(ROWID) AS rid_text, \
                 CAST(ROWID AS UROWID) AS urid \
                 FROM {table} WHERE id = 1"
            ),
            1,
        )
        .expect("fetch ROWID and UROWID");
    assert_eq!(
        result
            .columns
            .iter()
            .map(|column| column.column_type)
            .collect::<Vec<_>>(),
        vec![
            OracleColumnType::Varchar,
            OracleColumnType::Varchar,
            OracleColumnType::Varchar
        ]
    );
    let row = result.result.rows.first().expect("ROWID row");
    let rowid = value_to_string(&row[0]);
    let rowid_text = value_to_string(&row[1]);
    let urowid = value_to_string(&row[2]);

    assert_eq!(rowid, rowid_text);
    assert_eq!(urowid, rowid_text);
    assert_eq!(rowid.len(), 18);
    assert!(rowid
        .bytes()
        .all(|byte| byte.is_ascii_alphanumeric() || byte == b'+' || byte == b'/'));
}

#[test]
#[ignore = "requires local Oracle listener via ORACLE_THIN_TEST_* environment variables"]
fn ddl_statements_execute_and_take_effect() {
    let config = live_config();
    let table = unique_table_name("DDL_T");
    let index = unique_object_name("DDL_I");
    let view = unique_object_name("DDL_V");
    let sequence = unique_object_name("DDL_S");
    let function_name = unique_object_name("DDL_F");
    let procedure_name = unique_object_name("DDL_P");
    let package_name = unique_object_name("DDL_PKG");
    let _guard = DdlDropGuard::new(
        config.clone(),
        vec![
            format!("DROP PACKAGE {package_name}"),
            format!("DROP PROCEDURE {procedure_name}"),
            format!("DROP FUNCTION {function_name}"),
            format!("DROP VIEW {view}"),
            format!("DROP SEQUENCE {sequence}"),
            format!("DROP TABLE {table} PURGE"),
        ],
    );
    let mut conn = connect_with_config(config);

    conn.query_drop(&format!("CREATE TABLE {table} (id NUMBER PRIMARY KEY)"))
        .expect("create DDL test table");
    conn.query_drop(&format!("ALTER TABLE {table} ADD (name VARCHAR2(30))"))
        .expect("alter DDL test table");
    conn.query_drop(&format!("CREATE INDEX {index} ON {table} (name)"))
        .expect("create DDL test index");
    conn.query_drop(&format!(
        "CREATE OR REPLACE VIEW {view} AS SELECT id, name FROM {table}"
    ))
    .expect("create DDL test view");
    conn.query_drop(&format!("CREATE SEQUENCE {sequence} START WITH 10"))
        .expect("create DDL test sequence");
    conn.query_drop(&format!(
        "CREATE OR REPLACE FUNCTION {function_name} RETURN NUMBER AS \
         BEGIN RETURN 42; END;"
    ))
    .expect("create DDL test function");
    conn.query_drop(&format!(
        "CREATE OR REPLACE PROCEDURE {procedure_name} AS \
         BEGIN NULL; END;"
    ))
    .expect("create DDL test procedure");
    conn.query_drop(&format!(
        "CREATE OR REPLACE PACKAGE {package_name} AS \
         PROCEDURE p; \
         END;"
    ))
    .expect("create DDL test package spec");
    conn.query_drop(&format!(
        "CREATE OR REPLACE PACKAGE BODY {package_name} AS \
         PROCEDURE p IS BEGIN NULL; END; \
         END;"
    ))
    .expect("create DDL test package body");

    conn.query_drop(&format!(
        "INSERT INTO {table} (id, name) VALUES ({sequence}.NEXTVAL, 'created')"
    ))
    .expect("insert through sequence after DDL");
    let result = conn
        .query_described_fetch_all(
            format!("SELECT id, name, {function_name}() FROM {view}"),
            1,
        )
        .expect("fetch objects created by DDL");
    assert_eq!(
        rows_to_strings(&result.result.rows),
        vec![vec![
            "10".to_string(),
            "created".to_string(),
            "42".to_string(),
        ]]
    );

    conn.query_drop(&format!("ALTER TABLE {table} MODIFY (name VARCHAR2(40))"))
        .expect("modify DDL test table");
    conn.query_drop(&format!("TRUNCATE TABLE {table}"))
        .expect("truncate DDL test table");
    assert_eq!(select_count(&mut conn, &table), 0);

    conn.query_drop(&format!("DROP PACKAGE {package_name}"))
        .expect("drop DDL test package");
    conn.query_drop(&format!("DROP PROCEDURE {procedure_name}"))
        .expect("drop DDL test procedure");
    conn.query_drop(&format!("DROP FUNCTION {function_name}"))
        .expect("drop DDL test function");
    conn.query_drop(&format!("DROP VIEW {view}"))
        .expect("drop DDL test view");
    conn.query_drop(&format!("DROP SEQUENCE {sequence}"))
        .expect("drop DDL test sequence");
    conn.query_drop(&format!("DROP TABLE {table} PURGE"))
        .expect("drop DDL test table");
}

#[test]
#[ignore = "requires local Oracle listener via ORACLE_THIN_TEST_* environment variables"]
fn commit_and_auto_commit_make_changes_visible() {
    let config = live_config();
    let table = unique_table_name("TX");
    let _guard = TableDropGuard::new(config.clone(), table.clone());
    let mut writer = connect_with_config(config.clone());
    let mut reader = connect_with_config(config);

    writer
        .query_drop(&format!("CREATE TABLE {table} (id NUMBER PRIMARY KEY)"))
        .expect("create transaction table");

    writer
        .query_drop(&format!("INSERT INTO {table} VALUES (1)"))
        .expect("insert before explicit commit");
    assert_eq!(
        select_count(&mut reader, &table),
        0,
        "uncommitted row must not be visible to another session"
    );
    writer.commit().expect("commit transaction");
    assert_eq!(
        select_count(&mut reader, &table),
        1,
        "committed row must be visible to another session"
    );

    let mut request = StatementRequest::statement(format!("INSERT INTO {table} VALUES (2)"));
    request.auto_commit = true;
    writer
        .execute_typed(&request, &[])
        .expect("insert with protocol auto-commit");
    writer
        .rollback()
        .expect("rollback after auto-commit should not undo committed row");
    assert_eq!(
        select_count(&mut reader, &table),
        2,
        "auto-committed row must survive a later rollback"
    );
}

#[test]
#[ignore = "requires local Oracle listener via ORACLE_THIN_TEST_* environment variables"]
fn cancel_interrupts_long_running_query() {
    let mut conn = connect();
    conn.set_call_timeout(Some(Duration::from_secs(10)))
        .expect("set cancel test timeout");
    let cancel = conn.cancel_handle();
    let cancel_thread = thread::spawn(move || {
        thread::sleep(Duration::from_millis(300));
        cancel.break_execution()
    });

    let started = Instant::now();
    let result = conn.query(
        "SELECT COUNT(*) FROM all_objects a, all_objects b, all_objects c",
        1,
    );
    cancel_thread
        .join()
        .expect("cancel thread should not panic")
        .expect("send thin cancel marker");

    let message = result
        .expect_err("cancelled query should fail with ORA-01013")
        .to_string()
        .to_ascii_lowercase();
    assert!(
        message.contains("ora-01013") || message.contains("user requested cancel"),
        "expected ORA-01013 cancel error, got {message}"
    );
    assert!(
        started.elapsed() < Duration::from_secs(5),
        "cancel took {:?}",
        started.elapsed()
    );
}

fn live_config() -> OracleThinConfig {
    let host = env_or("ORACLE_THIN_TEST_HOST", "ORACLE_TEST_HOST", "127.0.0.1");
    let port = env_or("ORACLE_THIN_TEST_PORT", "ORACLE_TEST_PORT", "1521")
        .parse::<u16>()
        .expect("invalid Oracle test port");
    let service = env_or("ORACLE_THIN_TEST_SERVICE", "ORACLE_TEST_SERVICE", "FREE");
    let username = env_or(
        "ORACLE_THIN_TEST_USERNAME",
        "ORACLE_TEST_USERNAME",
        "system",
    );
    let password = env_or(
        "ORACLE_THIN_TEST_PASSWORD",
        "ORACLE_TEST_PASSWORD",
        "password",
    );
    let mut config = OracleThinConfig::new(
        ConnectTarget::service_name(host, port, service),
        username,
        password,
    );
    if let Some(version) = protocol_env("ORACLE_THIN_DESIRED_PROTOCOL") {
        config.connect_options.desired_protocol_version = version;
        config.connect_options.minimum_protocol_version = version;
    }
    if let Some(version) = protocol_env("ORACLE_THIN_MINIMUM_PROTOCOL") {
        config.connect_options.minimum_protocol_version = version;
    }
    if let Some(version) = ttc_field_version_env("ORACLE_THIN_TTC_FIELD_VERSION") {
        config.connect_options.desired_ttc_field_version = Some(version);
    }
    config.connect_options.disable_oob_probe = false;
    config
}

fn connect() -> OracleThinSession {
    connect_with_config(live_config())
}

fn connect_with_config(config: OracleThinConfig) -> OracleThinSession {
    OracleThinSession::connect(config).expect("thin login")
}

fn env_or(primary: &str, fallback: &str, default: &str) -> String {
    std::env::var(primary)
        .or_else(|_| std::env::var(fallback))
        .unwrap_or_else(|_| default.to_string())
}

fn local_timezone_offset_string() -> String {
    let seconds = chrono::Local::now().offset().local_minus_utc();
    let sign = if seconds < 0 { '-' } else { '+' };
    let absolute = seconds.abs();
    let hours = absolute / 3600;
    let minutes = (absolute % 3600) / 60;
    format!("{sign}{hours:02}:{minutes:02}")
}

fn protocol_env(name: &str) -> Option<u16> {
    let value = std::env::var(name).ok()?;
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return None;
    }
    Some(
        trimmed
            .parse::<u16>()
            .unwrap_or_else(|err| panic!("invalid {name} value `{trimmed}`: {err}")),
    )
}

fn ttc_field_version_env(name: &str) -> Option<u8> {
    let value = std::env::var(name).ok()?;
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return None;
    }
    Some(
        trimmed
            .parse::<u8>()
            .unwrap_or_else(|err| panic!("invalid {name} value `{trimmed}`: {err}")),
    )
}

fn unique_table_name(prefix: &str) -> String {
    unique_object_name(prefix)
}

fn unique_object_name(prefix: &str) -> String {
    let counter = OBJECT_COUNTER.fetch_add(1, Ordering::SeqCst);
    format!(
        "OQT_{}_{}_{}",
        prefix,
        std::process::id() % 100_000,
        counter
    )
}

fn drop_table_ignore(conn: &mut OracleThinSession, table: &str) {
    let _ = conn.query_drop(&format!("DROP TABLE {table} PURGE"));
}

fn drop_function_ignore(conn: &mut OracleThinSession, function_name: &str) {
    let _ = conn.query_drop(&format!("DROP FUNCTION {function_name}"));
}

fn drop_procedure_ignore(conn: &mut OracleThinSession, procedure_name: &str) {
    let _ = conn.query_drop(&format!("DROP PROCEDURE {procedure_name}"));
}

fn drop_type_ignore(conn: &mut OracleThinSession, type_name: &str) {
    let _ = conn.query_drop(&format!("DROP TYPE {type_name} FORCE"));
}

fn select_count(conn: &mut OracleThinSession, table: &str) -> i64 {
    let result = conn
        .query_described_fetch_all(format!("SELECT COUNT(*) FROM {table}"), 1)
        .expect("select count");
    let value = result
        .result
        .rows
        .first()
        .and_then(|row| row.first())
        .expect("count row");
    match value {
        OracleValue::Number(value) => value.parse::<i64>().expect("numeric count"),
        other => panic!("expected NUMBER count, got {other:?}"),
    }
}

fn rows_to_strings(rows: &[Vec<OracleValue>]) -> Vec<Vec<String>> {
    rows.iter()
        .map(|row| row.iter().map(value_to_string).collect())
        .collect()
}

fn assert_large_digit_row(row: &[OracleValue], digit: &str) {
    assert_eq!(value_to_string(&row[0]), digit);
    let payload = value_to_string(&row[1]);
    let expected = digit.chars().next().expect("single digit test value");
    assert_eq!(payload.len(), 4000);
    assert!(payload.chars().all(|ch| ch == expected));
}

fn assert_ref_cursor_varchar2_4000_row(row: &[OracleValue], digit: &str) {
    assert_eq!(value_to_string(&row[0]), digit);

    let ascii_payload = value_to_string(&row[1]);
    let expected = digit.chars().next().expect("single digit test value");
    assert_eq!(ascii_payload.len(), 4000);
    assert!(ascii_payload.chars().all(|ch| ch == expected));

    let utf8_payload = value_to_string(&row[2]);
    assert_eq!(utf8_payload.chars().count(), 1333);
    assert_eq!(utf8_payload.len(), 3999);
    assert!(utf8_payload.starts_with('\u{D55C}'));
    assert!(utf8_payload.ends_with('\u{D55C}'));

    assert_eq!(value_to_string(&row[3]), format!("tail-{digit}"));
}

fn value_to_string(value: &OracleValue) -> String {
    match value {
        OracleValue::Number(value) | OracleValue::Text(value) => value.clone(),
        other => panic!("unexpected test value {other:?}"),
    }
}

fn assert_lob_value_not_empty(value: &OracleValue) {
    match value {
        OracleValue::Lob(bytes) => assert!(
            !bytes.is_empty(),
            "expected non-empty object LOB/BFILE payload"
        ),
        other => panic!("expected object LOB/BFILE value, got {other:?}"),
    }
}

fn timestamp_value_to_string(value: &OracleValue) -> String {
    match value {
        OracleValue::Timestamp(value) => format!(
            "{:04}-{:02}-{:02} {:02}:{:02}:{:02}.{:06}",
            value.year,
            value.month,
            value.day,
            value.hour,
            value.minute,
            value.second,
            value.nanosecond / 1_000
        ),
        other => panic!("unexpected timestamp test value {other:?}"),
    }
}

fn date_value_to_string(value: &OracleValue) -> String {
    match value {
        OracleValue::DateTime(value) => format!(
            "{:04}-{:02}-{:02} {:02}:{:02}:{:02}",
            value.year, value.month, value.day, value.hour, value.minute, value.second
        ),
        other => panic!("unexpected date test value {other:?}"),
    }
}

fn timestamp_value_timezone_suffix(value: &OracleValue) -> Option<String> {
    match value {
        OracleValue::Timestamp(value) => value.timezone_suffix(),
        other => panic!("unexpected timestamp test value {other:?}"),
    }
}

fn expected_rows(start: i32, end: i32) -> Vec<Vec<String>> {
    (start..=end)
        .map(|value| vec![value.to_string(), format!("R{value}")])
        .collect()
}

fn oracle_datetime(
    year: u16,
    month: u8,
    day: u8,
    hour: u8,
    minute: u8,
    second: u8,
    nanosecond: u32,
) -> OracleDateTime {
    OracleDateTime {
        year,
        month,
        day,
        hour,
        minute,
        second,
        nanosecond,
        timezone_offset_minutes: None,
        timezone_region_id: None,
    }
}

struct TableDropGuard {
    config: OracleThinConfig,
    table: String,
}

impl TableDropGuard {
    fn new(config: OracleThinConfig, table: String) -> Self {
        Self { config, table }
    }
}

impl Drop for TableDropGuard {
    fn drop(&mut self) {
        if let Ok(mut conn) = OracleThinSession::connect(self.config.clone()) {
            drop_table_ignore(&mut conn, &self.table);
        }
    }
}

struct TypeDropGuard {
    config: OracleThinConfig,
    type_name: String,
}

impl TypeDropGuard {
    fn new(config: OracleThinConfig, type_name: String) -> Self {
        Self { config, type_name }
    }
}

impl Drop for TypeDropGuard {
    fn drop(&mut self) {
        if let Ok(mut conn) = OracleThinSession::connect(self.config.clone()) {
            drop_type_ignore(&mut conn, &self.type_name);
        }
    }
}

struct DdlDropGuard {
    config: OracleThinConfig,
    drop_sql: Vec<String>,
}

impl DdlDropGuard {
    fn new(config: OracleThinConfig, drop_sql: Vec<String>) -> Self {
        Self { config, drop_sql }
    }
}

impl Drop for DdlDropGuard {
    fn drop(&mut self) {
        if let Ok(mut conn) = OracleThinSession::connect(self.config.clone()) {
            for sql in &self.drop_sql {
                let _ = conn.query_drop(sql);
            }
        }
    }
}

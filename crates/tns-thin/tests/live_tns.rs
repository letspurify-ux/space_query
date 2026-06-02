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

fn value_to_string(value: &OracleValue) -> String {
    match value {
        OracleValue::Number(value) | OracleValue::Text(value) => value.clone(),
        other => panic!("unexpected test value {other:?}"),
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

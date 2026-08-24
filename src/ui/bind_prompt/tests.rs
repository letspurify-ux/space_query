use std::collections::HashMap;

use super::*;
use crate::db::{BindDataType, BindValue, DatabaseType};

fn session(db_type: DatabaseType) -> SessionState {
    SessionState::for_connection(db_type)
}

fn params(sql: &str, db_type: DatabaseType) -> Vec<BindParam> {
    collect_bind_params(
        sql,
        db_type,
        &session(db_type),
        &HashMap::new(),
        &HashMap::new(),
    )
}

fn labels(sql: &str, db_type: DatabaseType) -> Vec<String> {
    params(sql, db_type)
        .into_iter()
        .map(|param| param.label)
        .collect()
}

fn filled(sql: &str, db_type: DatabaseType, values: &[(BindParamType, &str)]) -> PreparedBinds {
    let mut collected = params(sql, db_type);
    assert_eq!(
        collected.len(),
        values.len(),
        "test supplied {} values for {} parameters",
        values.len(),
        collected.len()
    );
    for (param, (param_type, value)) in collected.iter_mut().zip(values) {
        param.param_type = *param_type;
        param.value = (*value).to_string();
    }
    prepare(sql, db_type, &collected)
}

const ORACLE: DatabaseType = DatabaseType::Oracle;
const MYSQL: DatabaseType = DatabaseType::MySQL;
const MARIADB: DatabaseType = DatabaseType::MariaDB;

fn anchor(sql: &str, db_type: DatabaseType) -> Vec<(String, Option<String>, String)> {
    bind_anchors(sql, db_type)
        .into_iter()
        .map(|(key, anchor)| (key, anchor.qualifier, anchor.column))
        .collect()
}

#[test]
fn a_comparison_names_the_column_the_placeholder_is_measured_against() {
    for db_type in [ORACLE, MYSQL, MARIADB] {
        assert_eq!(
            anchor("SELECT * FROM emp WHERE hire_date = :d", db_type),
            vec![("D".to_string(), None, "hire_date".to_string())],
            "{db_type:?}"
        );
        assert_eq!(
            anchor("SELECT * FROM emp e WHERE e.sal >= :low", db_type),
            vec![("LOW".to_string(), Some("e".to_string()), "sal".to_string())],
            "{db_type:?}"
        );
        assert_eq!(
            anchor("UPDATE emp SET hired = :d WHERE id = :id", db_type),
            vec![
                ("D".to_string(), None, "hired".to_string()),
                ("ID".to_string(), None, "id".to_string()),
            ],
            "{db_type:?}"
        );
        assert_eq!(
            anchor("SELECT * FROM emp WHERE name LIKE :pattern", db_type),
            vec![("PATTERN".to_string(), None, "name".to_string())],
            "{db_type:?}"
        );
    }
}

#[test]
fn a_range_and_a_list_name_their_column_too() {
    assert_eq!(
        anchor(
            "SELECT * FROM emp WHERE hired BETWEEN :from AND :to",
            ORACLE
        ),
        vec![
            ("FROM".to_string(), None, "hired".to_string()),
            ("TO".to_string(), None, "hired".to_string()),
        ]
    );
    assert_eq!(
        anchor("SELECT * FROM emp WHERE dept IN (:a, :b)", ORACLE),
        vec![
            ("A".to_string(), None, "dept".to_string()),
            ("B".to_string(), None, "dept".to_string()),
        ]
    );
}

#[test]
fn an_insert_pairs_each_value_with_its_own_column() {
    assert_eq!(
        anchor(
            "INSERT INTO emp (id, name, hired) VALUES (:id, :name, :hired)",
            ORACLE
        ),
        vec![
            ("ID".to_string(), None, "id".to_string()),
            ("NAME".to_string(), None, "name".to_string()),
            ("HIRED".to_string(), None, "hired".to_string()),
        ]
    );
    assert_eq!(
        anchor("INSERT INTO emp (id, name) VALUES (?, ?)", MYSQL),
        vec![
            ("?1".to_string(), None, "id".to_string()),
            ("?2".to_string(), None, "name".to_string()),
        ]
    );
}

#[test]
fn a_quoted_column_and_a_quoted_alias_are_read_unquoted() {
    assert_eq!(
        anchor(r#"SELECT * FROM emp e WHERE e."Hire Date" = :d"#, ORACLE),
        vec![(
            "D".to_string(),
            Some("e".to_string()),
            "Hire Date".to_string()
        )]
    );
    assert_eq!(
        anchor("SELECT * FROM emp WHERE `hire date` = :d", MYSQL),
        vec![("D".to_string(), None, "hire date".to_string())]
    );
}

#[test]
fn nothing_is_claimed_where_the_statement_names_no_column() {
    // An expression has no catalog type, and reporting the column inside it
    // would be a different claim than the statement makes.
    assert!(anchor("SELECT * FROM emp WHERE UPPER(name) = :n", ORACLE).is_empty());
    assert!(anchor("SELECT * FROM emp FETCH FIRST :n ROWS ONLY", ORACLE).is_empty());
    // A literal that looks like syntax must not be read as syntax.
    assert!(
        anchor("SELECT 'hired =' AS a FROM emp WHERE x = :y", ORACLE)
            .iter()
            .all(|(_, _, column)| column == "x")
    );
    // A commented-out comparison is not the one in force.
    assert_eq!(
        anchor(
            "SELECT * FROM emp\n-- WHERE hired =\nWHERE id = :id",
            ORACLE
        ),
        vec![("ID".to_string(), None, "id".to_string())]
    );
}

#[test]
fn a_data_type_maps_to_the_prompt_type_that_can_carry_it() {
    for (type_display, expected) in [
        ("NUMBER(10,2)", BindParamType::Number),
        ("NUMBER", BindParamType::Number),
        ("BINARY_DOUBLE", BindParamType::Number),
        ("int(11)", BindParamType::Number),
        ("bigint unsigned", BindParamType::Number),
        ("decimal(12,4)", BindParamType::Number),
        ("DATE", BindParamType::Date),
        ("date", BindParamType::Date),
        ("TIMESTAMP(6)", BindParamType::Timestamp),
        ("TIMESTAMP(6) WITH TIME ZONE", BindParamType::Timestamp),
        ("datetime(3)", BindParamType::Timestamp),
        ("timestamp", BindParamType::Timestamp),
        ("VARCHAR2(64)", BindParamType::String),
        ("varchar(64)", BindParamType::String),
        ("CLOB", BindParamType::String),
        ("text", BindParamType::String),
        ("RAW(16)", BindParamType::String),
        ("time", BindParamType::String),
        ("REF CURSOR", BindParamType::RefCursor),
        ("SYS_REFCURSOR", BindParamType::RefCursor),
    ] {
        assert_eq!(
            param_type_for_data_type(type_display),
            expected,
            "{type_display}"
        );
    }
}

fn types(sql: &str, db_type: DatabaseType) -> Vec<BindParamType> {
    params(sql, db_type)
        .into_iter()
        .map(|param| param.param_type)
        .collect()
}

#[test]
fn a_row_count_placeholder_opens_as_a_number() {
    // A quoted value here is a parse error, not a wrong result, so the dialog
    // must not open on String and wait to be corrected.
    assert_eq!(
        types("SELECT * FROM emp FETCH FIRST :n ROWS ONLY", ORACLE),
        vec![BindParamType::Number]
    );
    assert_eq!(
        types(
            "SELECT * FROM emp OFFSET :skip ROWS FETCH NEXT :n ROWS ONLY",
            ORACLE
        ),
        vec![BindParamType::Number, BindParamType::Number]
    );
    assert_eq!(
        types("SELECT * FROM emp WHERE ROWNUM <= :n", ORACLE),
        vec![BindParamType::Number]
    );
    for db_type in [MYSQL, MARIADB] {
        assert_eq!(
            types("SELECT * FROM emp LIMIT :n", db_type),
            vec![BindParamType::Number],
            "{db_type:?}"
        );
        assert_eq!(
            types("SELECT * FROM emp LIMIT :n OFFSET :skip", db_type),
            vec![BindParamType::Number, BindParamType::Number],
            "{db_type:?}"
        );
    }
}

#[test]
fn a_positional_row_count_opens_as_a_number_too() {
    assert_eq!(
        types("SELECT * FROM emp WHERE name = ? LIMIT ?", MYSQL),
        vec![BindParamType::String, BindParamType::Number]
    );
}

#[test]
fn an_out_cursor_opens_as_a_ref_cursor() {
    assert_eq!(
        types("BEGIN OPEN :rc FOR SELECT * FROM emp; END;", ORACLE),
        vec![BindParamType::RefCursor]
    );
    // The MySQL family is never offered the type, so it must not be guessed
    // there even if the word shows up.
    assert_eq!(
        types("SELECT * FROM emp WHERE state = :open", MYSQL),
        vec![BindParamType::String]
    );
}

#[test]
fn an_ordinary_value_is_left_as_a_string() {
    // Nothing here says what `:id` is, and String is the only answer that
    // cannot turn a working statement into a failing one.
    for db_type in [ORACLE, MYSQL, MARIADB] {
        assert_eq!(
            types("SELECT * FROM emp WHERE id = :id AND name = :name", db_type),
            vec![BindParamType::String, BindParamType::String],
            "{db_type:?}"
        );
        // A format string is a string, and the default already says so.
        assert_eq!(
            types(
                "SELECT * FROM emp WHERE hired = TO_DATE(:d, 'YYYY-MM-DD')",
                db_type
            ),
            vec![BindParamType::String],
            "{db_type:?}"
        );
    }
}

#[test]
fn a_guess_never_overrides_the_answer_the_user_already_gave() {
    let mut remembered = HashMap::new();
    remembered.insert(
        "N".to_string(),
        RememberedValue {
            param_type: BindParamType::String,
            value: "10".to_string(),
            is_null: false,
        },
    );
    let collected = collect_bind_params(
        "SELECT * FROM emp FETCH FIRST :n ROWS ONLY",
        ORACLE,
        &session(ORACLE),
        &remembered,
        &HashMap::new(),
    );
    assert_eq!(collected[0].param_type, BindParamType::String);
    assert_eq!(collected[0].value, "10");
}

fn call_anchor(
    sql: &str,
    db_type: DatabaseType,
) -> Vec<(String, Option<String>, String, CallParameter)> {
    bind_call_anchors(sql, db_type)
        .into_iter()
        .map(|(key, anchor)| (key, anchor.qualifier, anchor.routine, anchor.parameter))
        .collect()
}

#[test]
fn a_named_argument_names_the_parameter_the_placeholder_fills() {
    assert_eq!(
        call_anchor(
            "BEGIN SYSTEM.GET_HELP(P_CURSOR => :v_p_cursor); END;",
            ORACLE
        ),
        vec![(
            "V_P_CURSOR".to_string(),
            Some("SYSTEM".to_string()),
            "GET_HELP".to_string(),
            CallParameter::Named("P_CURSOR".to_string()),
        )]
    );
}

#[test]
fn a_plain_argument_list_gives_the_parameter_by_position() {
    assert_eq!(
        call_anchor("BEGIN pkg.load(:a, 1, :b); END;", ORACLE),
        vec![
            (
                "A".to_string(),
                Some("pkg".to_string()),
                "load".to_string(),
                CallParameter::Position(1),
            ),
            (
                "B".to_string(),
                Some("pkg".to_string()),
                "load".to_string(),
                CallParameter::Position(3),
            ),
        ]
    );
    assert_eq!(
        call_anchor("CALL load_batch(?, ?)", MYSQL),
        vec![
            (
                "?1".to_string(),
                None,
                "load_batch".to_string(),
                CallParameter::Position(1),
            ),
            (
                "?2".to_string(),
                None,
                "load_batch".to_string(),
                CallParameter::Position(2),
            ),
        ]
    );
}

/// On the MySQL family the statement itself names the routine namespace —
/// `CALL name(` can only be a procedure, any other call only a function — and
/// one name can be BOTH at once, so the anchor must carry that choice to the
/// lookup instead of letting the two routines answer for each other.
#[test]
fn a_mysql_call_site_names_its_routine_namespace() {
    use crate::db::query::mysql_executor::MysqlRoutineKind;

    let kinds = |sql: &str, db_type: DatabaseType| -> Vec<Option<MysqlRoutineKind>> {
        bind_call_anchors(sql, db_type)
            .into_iter()
            .map(|(_, anchor)| anchor.mysql_routine_kind)
            .collect()
    };

    assert_eq!(
        kinds("CALL sq_dup(:a)", MYSQL),
        vec![Some(MysqlRoutineKind::Procedure)]
    );
    assert_eq!(
        kinds("CALL `db`.`sq_dup`(:a)", MARIADB),
        vec![Some(MysqlRoutineKind::Procedure)]
    );
    assert_eq!(
        kinds("SELECT sq_dup(:x)", MYSQL),
        vec![Some(MysqlRoutineKind::Function)]
    );
    // The enclosing call for :x is the inner function, not the CALLed
    // procedure around it.
    assert_eq!(
        kinds("CALL sq_proc(sq_fn(:x))", MYSQL),
        vec![Some(MysqlRoutineKind::Function)]
    );
    // Oracle keeps its routines in one namespace: nothing to choose.
    assert_eq!(kinds("BEGIN pkg.load(:a); END;", ORACLE), vec![None]);
}

#[test]
fn no_routine_is_claimed_where_the_statement_calls_none() {
    // A built-in has no entry in any argument view, and its arguments are
    // ordinary values the user types.
    assert!(call_anchor("SELECT TO_DATE(:d, 'YYYY-MM-DD') FROM dual", ORACLE).is_empty());
    // A value list and an `IN` list are not calls.
    assert!(call_anchor("INSERT INTO t (a, b) VALUES (:a, :b)", ORACLE).is_empty());
    assert!(call_anchor("SELECT * FROM t WHERE id IN (:a, :b)", ORACLE).is_empty());
    assert!(call_anchor("SELECT * FROM emp WHERE id = :id", ORACLE).is_empty());
    // A commented-out call is not the one in force.
    assert!(call_anchor("-- pkg.load(:a)\nSELECT :a FROM dual", ORACLE).is_empty());
}

fn procedure_argument(
    name: Option<&str>,
    position: i32,
    data_type: &str,
    overload: Option<i32>,
) -> crate::db::ProcedureArgument {
    crate::db::ProcedureArgument {
        name: name.map(str::to_string),
        position,
        sequence: position,
        data_type: Some(data_type.to_string()),
        in_out: Some("IN".to_string()),
        data_length: None,
        data_precision: None,
        data_scale: None,
        type_owner: None,
        type_name: None,
        type_subname: None,
        pls_type: None,
        overload,
        default_value: None,
    }
}

#[test]
fn a_parameter_list_answers_by_name_and_by_position() {
    let arguments = [
        procedure_argument(None, 0, "VARCHAR2", None),
        procedure_argument(Some("P_ID"), 1, "NUMBER", None),
        procedure_argument(Some("P_CURSOR"), 2, "REF CURSOR", None),
    ];
    assert_eq!(
        param_type_for_call_parameter(&arguments, &CallParameter::Named("p_cursor".to_string())),
        Some(BindParamType::RefCursor)
    );
    assert_eq!(
        param_type_for_call_parameter(&arguments, &CallParameter::Position(1)),
        Some(BindParamType::Number)
    );
    // A function's return value is not a parameter the call passes, so
    // position 1 is the first real one either way.
    assert_eq!(
        param_type_for_call_parameter(&arguments, &CallParameter::Position(2)),
        Some(BindParamType::RefCursor)
    );
    // Nothing is claimed for a parameter the routine does not have.
    assert_eq!(
        param_type_for_call_parameter(&arguments, &CallParameter::Named("P_NAME".to_string())),
        None
    );
    assert_eq!(
        param_type_for_call_parameter(&arguments, &CallParameter::Position(3)),
        None
    );
}

#[test]
fn overloads_that_disagree_leave_the_type_unset() {
    let arguments = [
        procedure_argument(Some("P_KEY"), 1, "NUMBER", Some(1)),
        procedure_argument(Some("P_KEY"), 1, "VARCHAR2", Some(2)),
    ];
    assert_eq!(
        param_type_for_call_parameter(&arguments, &CallParameter::Position(1)),
        None
    );
    // Overloads that agree still answer.
    let arguments = [
        procedure_argument(Some("P_KEY"), 1, "NUMBER", Some(1)),
        procedure_argument(Some("P_KEY"), 1, "INTEGER", Some(2)),
    ];
    assert_eq!(
        param_type_for_call_parameter(&arguments, &CallParameter::Named("P_KEY".to_string())),
        Some(BindParamType::Number)
    );
}

#[test]
fn a_remembered_answer_never_makes_a_cursor_a_value_or_the_reverse() {
    // A `Ref Cursor` carries no value, so a remembered `String` is not a
    // preference to honour here — it is an answer to a different question.
    let mut remembered = HashMap::new();
    remembered.insert(
        "RC".to_string(),
        RememberedValue {
            param_type: BindParamType::String,
            value: "x".to_string(),
            is_null: false,
        },
    );
    let catalog = HashMap::from([("RC".to_string(), BindParamType::RefCursor)]);
    let collected = collect_bind_params(
        "BEGIN pkg.report(:rc); END;",
        ORACLE,
        &session(ORACLE),
        &remembered,
        &catalog,
    );
    assert_eq!(collected[0].param_type, BindParamType::RefCursor);

    // And the other way round: a value parameter answered as a cursor once.
    let mut remembered = HashMap::new();
    remembered.insert(
        "ID".to_string(),
        RememberedValue {
            param_type: BindParamType::RefCursor,
            value: String::new(),
            is_null: false,
        },
    );
    let catalog = HashMap::from([("ID".to_string(), BindParamType::Number)]);
    let collected = collect_bind_params(
        "SELECT * FROM emp WHERE id = :id",
        ORACLE,
        &session(ORACLE),
        &remembered,
        &catalog,
    );
    assert_eq!(collected[0].param_type, BindParamType::Number);

    // A disagreement that is only about how a value is typed still leaves the
    // user's own answer in place.
    let mut remembered = HashMap::new();
    remembered.insert(
        "ID".to_string(),
        RememberedValue {
            param_type: BindParamType::String,
            value: "7".to_string(),
            is_null: false,
        },
    );
    let catalog = HashMap::from([("ID".to_string(), BindParamType::Number)]);
    let collected = collect_bind_params(
        "SELECT * FROM emp WHERE id = :id",
        ORACLE,
        &session(ORACLE),
        &remembered,
        &catalog,
    );
    assert_eq!(collected[0].param_type, BindParamType::String);
}

#[test]
fn a_word_that_only_looks_like_a_clause_does_not_force_a_number() {
    // `limit` as a column name, and the same word inside a literal, must not
    // reach the placeholder beside it.
    assert_eq!(
        types("SELECT * FROM t WHERE limit_kind = :kind", MYSQL),
        vec![BindParamType::String]
    );
    assert_eq!(
        types("SELECT ':limit' AS a FROM t WHERE b = :c", MYSQL),
        vec![BindParamType::String]
    );
}

#[test]
fn a_named_bind_is_prompted_once_per_name() {
    for db_type in [ORACLE, MYSQL, MARIADB] {
        assert_eq!(
            labels("SELECT * FROM emp WHERE id = :id", db_type),
            vec![":ID".to_string()],
            "{db_type:?}"
        );
        assert_eq!(
            labels("SELECT * FROM t WHERE a = :x OR b = :X", db_type),
            vec![":X".to_string()],
            "{db_type:?}"
        );
    }
}

#[test]
fn a_numbered_bind_is_prompted() {
    for db_type in [ORACLE, MYSQL] {
        assert_eq!(
            labels("SELECT * FROM t WHERE a = :1", db_type),
            vec![":1".to_string()],
            "{db_type:?}"
        );
    }
}

#[test]
fn a_colon_inside_a_literal_or_comment_is_not_a_placeholder() {
    for db_type in [ORACLE, MYSQL, MARIADB] {
        assert!(labels("SELECT TO_CHAR(d, 'HH24:MI:SS') FROM t", db_type).is_empty());
        assert!(labels("SELECT 1 FROM t -- :note\n", db_type).is_empty());
        assert!(labels("SELECT 1 /* :note */ FROM t", db_type).is_empty());
    }
    assert!(labels("SELECT q'[a:b]' FROM dual", ORACLE).is_empty());
    assert!(labels("SELECT `a:b` FROM t", MYSQL).is_empty());
    assert!(labels("SELECT 1 FROM t # :note\n", MYSQL).is_empty());
}

#[test]
fn an_assignment_operator_is_not_a_placeholder() {
    assert!(labels("BEGIN v := 1; END;", ORACLE).is_empty());
    assert!(labels("SET @a := 1", MYSQL).is_empty());
}

#[test]
fn trigger_correlation_names_are_not_prompted() {
    let trigger = "CREATE OR REPLACE TRIGGER t BEFORE INSERT ON emp FOR EACH ROW \
                   BEGIN :NEW.id := :OLD.id; END;";
    assert!(labels(trigger, ORACLE).is_empty());

    let block = "BEGIN :NEW := 1; END;";
    assert_eq!(labels(block, ORACLE), vec![":NEW".to_string()]);
}

#[test]
fn a_variable_declaration_in_the_same_text_suppresses_the_prompt() {
    let script = "VARIABLE id NUMBER\nEXEC :id := 7\nSELECT * FROM emp WHERE id = :id;";
    assert!(labels(script, ORACLE).is_empty());
}

#[test]
fn a_bind_already_in_the_session_is_not_prompted() {
    let mut state = session(ORACLE);
    state
        .binds
        .insert("ID".to_string(), BindVar::new(BindDataType::Number));
    let collected = collect_bind_params(
        "SELECT * FROM emp WHERE id = :id",
        ORACLE,
        &state,
        &HashMap::new(),
        &HashMap::new(),
    );
    assert!(collected.is_empty());
}

// --- Declared binds: the `VARIABLE` flow must be untouched by the prompt -----

/// `VARIABLE id NUMBER` followed by `EXEC :id := 7` leaves a bind that already
/// carries a value. It is a declaration either way, so it is never asked about.
#[test]
fn a_declared_bind_holding_a_value_is_not_prompted() {
    let mut state = session(ORACLE);
    let mut declared = BindVar::new(BindDataType::Number);
    declared.value = BindValue::Scalar(Some("7".to_string()));
    state.binds.insert("ID".to_string(), declared);

    assert!(collect_bind_params(
        "SELECT * FROM emp WHERE id = :id",
        ORACLE,
        &state,
        &HashMap::new(),
        &HashMap::new(),
    )
    .is_empty());
}

/// Declarations and uses are matched case-insensitively, the way every other
/// bind lookup in the app matches them.
#[test]
fn a_declaration_matches_the_use_whatever_the_case() {
    let mut state = session(ORACLE);
    state
        .binds
        .insert("ID".to_string(), BindVar::new(BindDataType::Number));

    for sql in [
        "SELECT * FROM emp WHERE id = :id",
        "SELECT * FROM emp WHERE id = :ID",
        "SELECT * FROM emp WHERE id = :Id",
    ] {
        assert!(
            collect_bind_params(sql, ORACLE, &state, &HashMap::new(), &HashMap::new()).is_empty(),
            "{sql}"
        );
    }

    let script = "VARIABLE Id NUMBER
SELECT * FROM emp WHERE id = :ID;";
    assert!(labels(script, ORACLE).is_empty());
}

/// A `REFCURSOR` bind has no text value a prompt could ask for, so leaving
/// declared binds alone is what keeps `VAR rc REFCURSOR` working.
#[test]
fn a_declared_refcursor_bind_is_not_prompted() {
    let mut state = session(ORACLE);
    state
        .binds
        .insert("RC".to_string(), BindVar::new(BindDataType::RefCursor));

    assert!(collect_bind_params(
        "BEGIN open_emps(:rc); END;",
        ORACLE,
        &state,
        &HashMap::new(),
        &HashMap::new(),
    )
    .is_empty());

    assert!(labels(
        "VAR rc REFCURSOR
BEGIN open_emps(:rc); END;
",
        ORACLE
    )
    .is_empty());
}

/// The regression the `prompted` flag exists to prevent: the value a prompt
/// wrote into the session must not look like a declaration to the next run.
#[test]
fn a_prompted_bind_is_asked_about_again_on_the_next_run() {
    let sql = "SELECT * FROM emp WHERE id = :id";
    let mut state = session(ORACLE);

    let mut first = collect_bind_params(sql, ORACLE, &state, &HashMap::new(), &HashMap::new());
    assert_eq!(first.len(), 1);
    first[0].param_type = BindParamType::Number;
    first[0].value = "7".to_string();

    let prepared = prepare(sql, ORACLE, &first);
    for (name, bind) in prepared.session_binds {
        state.binds.insert(name, bind);
    }

    let remembered: HashMap<String, RememberedValue> = first
        .iter()
        .map(|param| (param.memo_key.clone(), RememberedValue::from(param)))
        .collect();
    let second = collect_bind_params(sql, ORACLE, &state, &remembered, &HashMap::new());

    assert_eq!(second.len(), 1, "the prompt must come back on the next run");
    assert_eq!(second[0].param_type, BindParamType::Number);
    assert_eq!(second[0].value, "7", "prefilled with the previous answer");
}

/// …and an explicit declaration made afterwards ends the prompting, because it
/// replaces the prompted entry with a declared one.
#[test]
fn declaring_a_previously_prompted_bind_stops_the_prompt() {
    let mut state = session(ORACLE);
    state.binds.insert(
        "ID".to_string(),
        BindVar::from_prompt(BindDataType::Number, Some("7".to_string())),
    );
    assert_eq!(
        collect_bind_params(
            "SELECT * FROM emp WHERE id = :id",
            ORACLE,
            &state,
            &HashMap::new(),
            &HashMap::new(),
        )
        .len(),
        1,
        "a prompted bind is still prompted"
    );

    state
        .binds
        .insert("ID".to_string(), BindVar::new(BindDataType::Number));
    assert!(collect_bind_params(
        "SELECT * FROM emp WHERE id = :id",
        ORACLE,
        &state,
        &HashMap::new(),
        &HashMap::new(),
    )
    .is_empty());
}

// --- Mixed: some declared, some not -----------------------------------------

#[test]
fn only_the_undeclared_names_are_prompted_when_a_statement_mixes_both() {
    let mut state = session(ORACLE);
    state
        .binds
        .insert("ID".to_string(), BindVar::new(BindDataType::Number));

    let collected = collect_bind_params(
        "SELECT * FROM emp WHERE id = :id AND dept = :dept AND name = :name",
        ORACLE,
        &state,
        &HashMap::new(),
        &HashMap::new(),
    );

    let names: Vec<&str> = collected.iter().map(|param| param.label.as_str()).collect();
    assert_eq!(names, vec![":DEPT", ":NAME"]);
}

/// Only the prompted names may be written back: a declared bind's value and
/// type must survive the run untouched.
#[test]
fn preparing_a_mixed_statement_writes_back_only_the_prompted_names() {
    let sql = "SELECT * FROM emp WHERE id = :id AND dept = :dept";
    let mut state = session(ORACLE);
    let mut declared = BindVar::new(BindDataType::Number);
    declared.value = BindValue::Scalar(Some("7".to_string()));
    state.binds.insert("ID".to_string(), declared);

    let mut collected = collect_bind_params(sql, ORACLE, &state, &HashMap::new(), &HashMap::new());
    assert_eq!(collected.len(), 1);
    collected[0].param_type = BindParamType::Number;
    collected[0].value = "3".to_string();

    let prepared = prepare(sql, ORACLE, &collected);
    assert_eq!(prepared.sql, sql);
    let written: Vec<&str> = prepared
        .session_binds
        .iter()
        .map(|(name, _)| name.as_str())
        .collect();
    assert_eq!(written, vec!["DEPT"]);

    for (name, bind) in prepared.session_binds {
        state.binds.insert(name, bind);
    }
    assert!(matches!(
        state.binds["ID"].value,
        BindValue::Scalar(Some(ref value)) if value == "7"
    ));
    assert!(!state.binds["ID"].prompted);
    assert!(state.binds["DEPT"].prompted);
}

/// A script that declares one bind and leaves another open: the declaration has
/// not run yet, so it is read out of the text rather than the session.
#[test]
fn a_script_declaring_one_bind_is_only_prompted_for_the_other() {
    let script = "VARIABLE id NUMBER
                  EXEC :id := 7
                  SELECT * FROM emp WHERE id = :id AND dept = :dept;";
    assert_eq!(labels(script, ORACLE), vec![":DEPT".to_string()]);
}

/// A declared name and a previously prompted name in the same statement: one is
/// left alone, the other comes back prefilled.
#[test]
fn a_declared_bind_and_a_remembered_one_are_told_apart() {
    let mut state = session(ORACLE);
    state
        .binds
        .insert("ID".to_string(), BindVar::new(BindDataType::Number));
    state.binds.insert(
        "DEPT".to_string(),
        BindVar::from_prompt(BindDataType::Number, Some("3".to_string())),
    );

    let mut remembered = HashMap::new();
    remembered.insert(
        "DEPT".to_string(),
        RememberedValue {
            param_type: BindParamType::Number,
            value: "3".to_string(),
            is_null: false,
        },
    );

    let collected = collect_bind_params(
        "SELECT * FROM emp WHERE id = :id AND dept = :dept",
        ORACLE,
        &state,
        &remembered,
        &HashMap::new(),
    );

    assert_eq!(collected.len(), 1);
    assert_eq!(collected[0].label, ":DEPT");
    assert_eq!(collected[0].value, "3");
}

/// A declared name must not lose its place to a generated `?` name either.
#[test]
fn a_generated_name_avoids_a_declared_bind() {
    let mut state = session(ORACLE);
    state
        .binds
        .insert("SQ_P1".to_string(), BindVar::new(BindDataType::Number));

    let mut collected = collect_bind_params(
        "SELECT * FROM emp WHERE id = :sq_p1 AND dept = ?",
        ORACLE,
        &state,
        &HashMap::new(),
        &HashMap::new(),
    );
    assert_eq!(collected.len(), 1, "the declared name is not prompted");
    collected[0].param_type = BindParamType::Number;
    collected[0].value = "3".to_string();

    let prepared = prepare(
        "SELECT * FROM emp WHERE id = :sq_p1 AND dept = ?",
        ORACLE,
        &collected,
    );
    assert_eq!(
        prepared.sql,
        "SELECT * FROM emp WHERE id = :sq_p1 AND dept = :SQ_P1_1"
    );
}

/// The MySQL family has no declarations at all, so a mixed text there is just
/// "everything is prompted" — and every placeholder must still be substituted.
#[test]
fn mysql_substitutes_a_mix_of_named_and_positional_placeholders() {
    for db_type in [MYSQL, MARIADB] {
        let sql = "SELECT * FROM emp WHERE id = :id AND dept = ? AND name = :id";
        let mut collected = params(sql, db_type);
        assert_eq!(collected.len(), 2, "{db_type:?}");
        collected[0].param_type = BindParamType::Number;
        collected[0].value = "7".to_string();
        collected[1].param_type = BindParamType::String;
        collected[1].value = "sales".to_string();

        let prepared = prepare(sql, db_type, &collected);
        assert_eq!(
            prepared.sql, "SELECT * FROM emp WHERE id = 7 AND dept = 'sales' AND name = 7",
            "{db_type:?}"
        );
        assert!(prepared.session_binds.is_empty(), "{db_type:?}");
    }
}

#[test]
fn positional_placeholders_are_numbered_in_order() {
    for db_type in [ORACLE, MYSQL, MARIADB] {
        assert_eq!(
            labels("SELECT * FROM t WHERE a = ? AND b = ?", db_type),
            vec!["? 1".to_string(), "? 2".to_string()],
            "{db_type:?}"
        );
    }
    assert!(labels("SELECT '?' FROM t WHERE a = 1", MYSQL).is_empty());
}

#[test]
fn named_parameters_are_listed_before_positional_ones() {
    assert_eq!(
        labels("SELECT * FROM t WHERE a = ? AND b = :b", MYSQL),
        vec![":B".to_string(), "? 1".to_string()]
    );
}

#[test]
fn oracle_keeps_the_statement_text_and_declares_the_binds() {
    let sql = "SELECT * FROM emp WHERE id = :id AND name = :name";
    let prepared = filled(
        sql,
        ORACLE,
        &[(BindParamType::Number, "7"), (BindParamType::String, "ann")],
    );

    assert_eq!(prepared.sql, sql);
    let names: Vec<&str> = prepared
        .session_binds
        .iter()
        .map(|(name, _)| name.as_str())
        .collect();
    assert_eq!(names, vec!["ID", "NAME"]);
    assert!(matches!(
        prepared.session_binds[0].1.data_type,
        BindDataType::Number
    ));
    assert!(matches!(
        prepared.session_binds[0].1.value,
        BindValue::Scalar(Some(ref value)) if value == "7"
    ));
    assert!(matches!(
        prepared.session_binds[1].1.data_type,
        BindDataType::Varchar2(4000)
    ));
}

#[test]
fn oracle_rewrites_question_marks_into_generated_binds() {
    let prepared = filled(
        "SELECT * FROM emp WHERE id = ? AND dept = ?",
        ORACLE,
        &[(BindParamType::Number, "7"), (BindParamType::Number, "3")],
    );

    assert_eq!(
        prepared.sql,
        "SELECT * FROM emp WHERE id = :SQ_P1 AND dept = :SQ_P2"
    );
    let names: Vec<&str> = prepared
        .session_binds
        .iter()
        .map(|(name, _)| name.as_str())
        .collect();
    assert_eq!(names, vec!["SQ_P1", "SQ_P2"]);
}

#[test]
fn a_generated_bind_name_avoids_a_name_the_statement_already_uses() {
    let prepared = filled(
        "SELECT * FROM emp WHERE id = :sq_p1 AND dept = ?",
        ORACLE,
        &[(BindParamType::Number, "7"), (BindParamType::Number, "3")],
    );

    assert_eq!(
        prepared.sql,
        "SELECT * FROM emp WHERE id = :sq_p1 AND dept = :SQ_P1_1"
    );
    let names: Vec<&str> = prepared
        .session_binds
        .iter()
        .map(|(name, _)| name.as_str())
        .collect();
    assert_eq!(names, vec!["SQ_P1", "SQ_P1_1"]);
}

#[test]
fn mysql_substitutes_literals_and_declares_nothing() {
    let prepared = filled(
        "SELECT * FROM emp WHERE id = :id AND name = :name",
        MYSQL,
        &[
            (BindParamType::Number, "7"),
            (BindParamType::String, "o'ha\\re"),
        ],
    );

    assert_eq!(
        prepared.sql,
        "SELECT * FROM emp WHERE id = 7 AND name = 'o''ha\\\\re'"
    );
    assert!(prepared.session_binds.is_empty());
}

#[test]
fn mysql_substitutes_every_occurrence_of_a_repeated_name() {
    let prepared = filled(
        "SELECT * FROM t WHERE a = :x OR b = :x",
        MARIADB,
        &[(BindParamType::Number, "5")],
    );

    assert_eq!(prepared.sql, "SELECT * FROM t WHERE a = 5 OR b = 5");
}

#[test]
fn mysql_substitutes_positional_placeholders_in_order() {
    let prepared = filled(
        "SELECT * FROM t WHERE a = ? AND b = ?",
        MYSQL,
        &[
            (BindParamType::String, "one"),
            (BindParamType::String, "two"),
        ],
    );

    assert_eq!(
        prepared.sql,
        "SELECT * FROM t WHERE a = 'one' AND b = 'two'"
    );
    assert!(!prepared.sql.contains('?'));
}

#[test]
fn a_temporal_value_is_quoted_on_mysql_and_bound_as_a_date_on_oracle() {
    let mysql = filled(
        "SELECT * FROM t WHERE d = :d",
        MYSQL,
        &[(BindParamType::Date, "2026-08-08")],
    );
    assert_eq!(mysql.sql, "SELECT * FROM t WHERE d = '2026-08-08'");

    let oracle = filled(
        "SELECT * FROM t WHERE d = :d",
        ORACLE,
        &[(BindParamType::Timestamp, "2026-08-08 10:11:12")],
    );
    assert!(matches!(
        oracle.session_binds[0].1.data_type,
        BindDataType::Timestamp(6)
    ));
}

#[test]
fn a_null_answer_becomes_null_on_both_families() {
    let sql = "SELECT * FROM t WHERE a = :a";
    for db_type in [ORACLE, MYSQL] {
        let mut collected = params(sql, db_type);
        collected[0].value = "ignored".to_string();
        collected[0].is_null = true;
        let prepared = prepare(sql, db_type, &collected);
        if db_type.is_mysql_or_mariadb() {
            assert_eq!(prepared.sql, "SELECT * FROM t WHERE a = NULL");
        } else {
            assert!(matches!(
                prepared.session_binds[0].1.value,
                BindValue::Scalar(None)
            ));
        }
    }
}

#[test]
fn a_remembered_answer_prefills_the_next_prompt() {
    let mut remembered = HashMap::new();
    remembered.insert(
        "ID".to_string(),
        RememberedValue {
            param_type: BindParamType::Number,
            value: "42".to_string(),
            is_null: false,
        },
    );
    let collected = collect_bind_params(
        "SELECT * FROM emp WHERE id = :id",
        ORACLE,
        &session(ORACLE),
        &remembered,
        &HashMap::new(),
    );

    assert_eq!(collected[0].param_type, BindParamType::Number);
    assert_eq!(collected[0].value, "42");
}

// --- OUT parameters and ref cursors -----------------------------------------

/// An undeclared OUT cursor can be answered as one, and becomes the same kind
/// of session bind `VARIABLE rc REFCURSOR` would have declared.
#[test]
fn a_ref_cursor_answer_declares_a_refcursor_bind_with_no_value() {
    let sql = "BEGIN open_emps(:rc); END;";
    let mut collected = params(sql, ORACLE);
    assert_eq!(collected.len(), 1);
    collected[0].param_type = BindParamType::RefCursor;

    let prepared = prepare(sql, ORACLE, &collected);
    assert_eq!(prepared.sql, sql);
    assert!(matches!(
        prepared.session_binds[0].1.data_type,
        BindDataType::RefCursor
    ));
    assert!(matches!(
        prepared.session_binds[0].1.value,
        BindValue::Cursor(None)
    ));
    assert!(prepared.session_binds[0].1.prompted);
}

/// The MySQL family renders answers as literals in the statement text, where a
/// cursor means nothing, so the type is not offered there.
#[test]
fn a_ref_cursor_is_only_offered_on_oracle() {
    assert!(BindParamType::offered_for(ORACLE).contains(&BindParamType::RefCursor));
    for db_type in [MYSQL, MARIADB] {
        assert!(
            !BindParamType::offered_for(db_type).contains(&BindParamType::RefCursor),
            "{db_type:?}"
        );
    }
}

/// An OUT scalar is answered by leaving the box empty: there is no such thing
/// as an empty number, so it binds NULL rather than an empty literal.
#[test]
fn an_empty_answer_is_null_for_every_type_but_string() {
    let sql = "SELECT * FROM t WHERE a = :a";
    for param_type in [
        BindParamType::Number,
        BindParamType::Date,
        BindParamType::Timestamp,
    ] {
        let mut collected = params(sql, ORACLE);
        collected[0].param_type = param_type;
        let prepared = prepare(sql, ORACLE, &collected);
        assert!(
            matches!(prepared.session_binds[0].1.value, BindValue::Scalar(None)),
            "{param_type:?}"
        );

        let mut collected = params(sql, MYSQL);
        collected[0].param_type = param_type;
        assert_eq!(
            prepare(sql, MYSQL, &collected).sql,
            "SELECT * FROM t WHERE a = NULL",
            "{param_type:?}"
        );
    }
}

/// An empty String answer is the empty string, which is the only place the two
/// families differ: Oracle treats it as NULL, the MySQL family as `''`.
#[test]
fn an_empty_string_answer_stays_an_empty_string() {
    let sql = "SELECT * FROM t WHERE a = :a";
    let collected = params(sql, MYSQL);
    assert_eq!(
        prepare(sql, MYSQL, &collected).sql,
        "SELECT * FROM t WHERE a = ''"
    );
}

/// A PL/SQL call mixing an IN value with an OUT cursor asks about both and
/// declares each as its own type.
#[test]
fn a_call_mixing_an_in_value_and_an_out_cursor_declares_both() {
    let sql = "BEGIN emps_by_dept(:dept, :rc); END;";
    let mut collected = params(sql, ORACLE);
    assert_eq!(collected.len(), 2);
    collected[0].param_type = BindParamType::Number;
    collected[0].value = "20".to_string();
    collected[1].param_type = BindParamType::RefCursor;

    let prepared = prepare(sql, ORACLE, &collected);
    assert!(matches!(
        prepared.session_binds[0].1.data_type,
        BindDataType::Number
    ));
    assert!(matches!(
        prepared.session_binds[1].1.data_type,
        BindDataType::RefCursor
    ));
}

#[test]
fn a_statement_without_placeholders_asks_nothing() {
    for db_type in [ORACLE, MYSQL, MARIADB] {
        assert!(labels("SELECT 1 FROM t", db_type).is_empty(), "{db_type:?}");
    }
}

// --- every way a routine gets called ----------------------------------------

/// The prompt has to see the placeholders whichever spelling the call uses.
/// `EXEC` in particular is rewritten into a PL/SQL block deep in the execution
/// worker, long after this scan runs, so it has to be recognized as written.
#[test]
fn every_oracle_call_form_is_scanned() {
    for (sql, expected) in [
        ("BEGIN p(:a, :b); END;", vec![":A", ":B"]),
        ("EXEC p(:a, :b)", vec![":A", ":B"]),
        ("EXECUTE p(:a, :b)", vec![":A", ":B"]),
        ("exec p(:a,:b)", vec![":A", ":B"]),
        ("CALL p(:a, :b)", vec![":A", ":B"]),
        ("call p(:a, :b);", vec![":A", ":B"]),
        ("DECLARE v NUMBER; BEGIN p(:a, :b); END;", vec![":A", ":B"]),
        ("BEGIN :r := f(:a); END;", vec![":R", ":A"]),
        ("EXEC :r := f(:a)", vec![":R", ":A"]),
        ("SELECT f(:a) FROM DUAL", vec![":A"]),
    ] {
        assert_eq!(labels(sql, ORACLE), expected, "{sql}");
    }
}

#[test]
fn every_mysql_call_form_is_scanned() {
    for db_type in [MYSQL, MARIADB] {
        for (sql, expected) in [
            ("CALL p(:a, :b)", vec![":A", ":B"]),
            ("call p(:a, @out);", vec![":A"]),
            ("SELECT f(:a)", vec![":A"]),
            ("CALL p(:a, ?)", vec![":A", "? 1"]),
        ] {
            assert_eq!(labels(sql, db_type), expected, "{db_type:?} {sql}");
        }
    }
}

/// A MySQL OUT argument must be a user variable, and `@out` is not a
/// placeholder — so it passes through untouched while the IN value is
/// substituted.
#[test]
fn a_mysql_user_variable_is_left_alone_beside_a_substituted_value() {
    let sql = "CALL p(:dept, @cnt)";
    let mut collected = params(sql, MYSQL);
    assert_eq!(collected.len(), 1);
    collected[0].param_type = BindParamType::Number;
    collected[0].value = "30".to_string();

    assert_eq!(prepare(sql, MYSQL, &collected).sql, "CALL p(30, @cnt)");
}

/// A Number answer that is not a number is refused, not reinterpreted.
///
/// On the MySQL family the answer is substituted INTO the statement, and a
/// Number is the one type emitted without quotes. Both other ends are wrong: as
/// written it carries whatever it says into the statement (a value of
/// `1; SET GLOBAL max_connections = 5000` adds a statement of its own, past a
/// connection read-only guard that judged the text before the value was in it),
/// and quoted it silently becomes a string comparison — `WHERE id = 'abc'`
/// matches the rows where `id` is 0 on a server that coerces. The value came
/// from a person, so the app says which placeholder is wrong.
#[test]
fn a_number_answer_that_is_not_a_number_is_refused() {
    let mut answered = params("SELECT * FROM t WHERE id = ?", MYSQL);
    assert_eq!(answered.len(), 1);
    answered[0].param_type = BindParamType::Number;

    for value in [
        "1; SET GLOBAL max_connections = 5000",
        "1 OR 1=1",
        "abc",
        "1,234",
    ] {
        answered[0].value = value.to_string();
        let message = non_numeric_answer_message(MYSQL, &answered)
            .unwrap_or_else(|| panic!("{value:?} must be refused"));
        assert!(
            message.contains(&answered[0].label) && message.contains("not a number"),
            "the refusal must name the placeholder and the reason: {message}"
        );
    }

    // A real number runs, in every shape.
    for value in ["30", "-1.5", "+1", ".5", "1e3"] {
        answered[0].value = value.to_string();
        assert_eq!(
            non_numeric_answer_message(MYSQL, &answered),
            None,
            "{value}"
        );
    }

    // A Text answer is quoted and escaped, so it is a value whatever it says.
    answered[0].param_type = BindParamType::String;
    answered[0].value = "1; SET GLOBAL max_connections = 5000".to_string();
    assert_eq!(non_numeric_answer_message(MYSQL, &answered), None);
    assert_eq!(
        prepare("SELECT * FROM t WHERE id = ?", MYSQL, &answered).sql,
        "SELECT * FROM t WHERE id = '1; SET GLOBAL max_connections = 5000'"
    );

    // Oracle passes answers as real binds, so nothing of the value becomes SQL
    // text there and there is nothing to refuse.
    let mut oracle_answered = params("SELECT * FROM t WHERE id = :id", ORACLE);
    oracle_answered[0].param_type = BindParamType::Number;
    oracle_answered[0].value = "1; SET GLOBAL x = 1".to_string();
    assert_eq!(non_numeric_answer_message(ORACLE, &oracle_answered), None);
    assert_eq!(
        prepare("SELECT * FROM t WHERE id = :id", ORACLE, &oracle_answered).sql,
        "SELECT * FROM t WHERE id = :id"
    );
}

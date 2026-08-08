use std::collections::HashMap;

use super::*;
use crate::db::{BindDataType, BindValue, DatabaseType};

fn session(db_type: DatabaseType) -> SessionState {
    SessionState::for_connection(db_type)
}

fn params(sql: &str, db_type: DatabaseType) -> Vec<BindParam> {
    collect_bind_params(sql, db_type, &session(db_type), &HashMap::new())
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
            collect_bind_params(sql, ORACLE, &state, &HashMap::new()).is_empty(),
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

    let mut first = collect_bind_params(sql, ORACLE, &state, &HashMap::new());
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
    let second = collect_bind_params(sql, ORACLE, &state, &remembered);

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

    let mut collected = collect_bind_params(sql, ORACLE, &state, &HashMap::new());
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

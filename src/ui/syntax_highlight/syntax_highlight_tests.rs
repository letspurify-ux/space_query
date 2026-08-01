use super::*;
use std::collections::{BTreeMap, HashSet};
use std::fs;
use std::path::PathBuf;

fn load_mariadb_highlight_test_file(name: &str) -> String {
    let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    for dir in ["test_mysql", "test_mariadb"] {
        let path = manifest_dir.join(dir).join(name);
        if let Ok(contents) = fs::read_to_string(path) {
            return contents;
        }
    }
    String::new()
}

fn assert_token_has_style(text: &str, styles: &str, token: &str, expected_style: char) {
    let start = text.find(token).expect("token should exist in test SQL");
    let end = start + token.len();
    assert!(
        styles[start..end]
            .chars()
            .all(|style| style == expected_style),
        "{token} should use style {expected_style}"
    );
}

fn assert_word_has_style(text: &str, styles: &str, word: &str, expected_style: char) {
    let is_word_byte =
        |byte: u8| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'$' | b'#');
    let start = text
        .match_indices(word)
        .map(|(start, _)| start)
        .find(|&start| {
            let end = start + word.len();
            !start
                .checked_sub(1)
                .and_then(|index| text.as_bytes().get(index))
                .is_some_and(|byte| is_word_byte(*byte))
                && !text
                    .as_bytes()
                    .get(end)
                    .is_some_and(|byte| is_word_byte(*byte))
        })
        .unwrap_or_else(|| panic!("word `{word}` should exist in test SQL"));
    let end = start + word.len();
    assert!(
        styles[start..end]
            .chars()
            .all(|style| style == expected_style),
        "{word} should use style {expected_style}"
    );
}

#[test]
#[ignore = "audits every SQL fixture word through the production full-highlight path"]
fn syntax_highlighting_sweep_all_files_generate_out_report() {
    let manifest = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let output_root = manifest.join("target/highlight-sweep");
    let mut failures = Vec::new();
    let mut checked_files = 0usize;
    let mut checked_words = 0usize;
    let mut checked_catalog_words = 0usize;
    let mut highlighted_catalog_words = 0usize;
    let mut contextual_catalog_words = 0usize;
    let mut checked_object_words = 0usize;
    let mut highlighted_object_words = 0usize;
    let mut contextual_object_words = 0usize;
    let mut unexpected_highlights = 0usize;
    let mut protected_lexical_spans = 0usize;
    let mut protected_lexical_bytes = 0usize;
    let mut ordinary_default_words = BTreeMap::<String, (usize, String)>::new();
    let mut ordinary_default_call_words = BTreeMap::<String, (usize, String)>::new();
    let mut ordinary_default_percent_words = BTreeMap::<String, (usize, String)>::new();
    let mut contextual_catalog_word_counts = BTreeMap::<String, (usize, String)>::new();

    for (dir, db_type) in [
        ("test", DatabaseType::Oracle),
        ("test_mysql", DatabaseType::MySQL),
        ("test_mariadb", DatabaseType::MariaDB),
    ] {
        let input_dir = manifest.join(dir);
        let mut entries: Vec<PathBuf> = fs::read_dir(&input_dir)
            .unwrap_or_else(|err| panic!("failed to read `{}`: {err}", input_dir.display()))
            .filter_map(Result::ok)
            .map(|entry| entry.path())
            .filter(|path| {
                path.extension()
                    .and_then(|ext| ext.to_str())
                    .is_some_and(|ext| matches!(ext, "sql" | "txt"))
            })
            .collect();
        entries.sort();

        for input_path in entries {
            checked_files = checked_files.saturating_add(1);
            let source = fs::read_to_string(&input_path)
                .unwrap_or_else(|err| panic!("failed to read `{}`: {err}", input_path.display()));
            let (highlight_data, object_names) = highlight_sweep_fixture_objects(&source);
            let mut highlighter = SqlHighlighter::new();
            highlighter.set_db_type(db_type);
            highlighter.set_highlight_data(highlight_data);
            let (styles, _line_states, word_audits) =
                crate::ui::sql_editor::build_logical_styles_and_line_states_with_word_audit(
                    &highlighter,
                    &source,
                );
            assert_eq!(
                styles.len(),
                source.len(),
                "style/source byte length mismatch"
            );
            let relative = input_path.strip_prefix(&manifest).unwrap_or(&input_path);
            let file_checked_words = word_audits.len();
            let mut file_issues = Vec::new();
            let mut file_catalog_words = 0usize;
            let mut file_contextual_words = 0usize;
            let mut file_object_words = 0usize;
            let mut file_unexpected_highlights = 0usize;
            let mut file_ordinary_default_words = BTreeMap::<String, (usize, String)>::new();

            for span in crate::sql_parser_engine::lexical_spans(
                &source,
                !matches!(db_type, DatabaseType::Oracle),
            ) {
                if !matches!(db_type, DatabaseType::Oracle)
                    && span.kind == crate::sql_parser_engine::LexicalKind::String
                    && source[span.start..span.end].starts_with('$')
                {
                    // MySQL/MariaDB fixture `DELIMITER $$` markers are not
                    // PostgreSQL dollar-quoted strings.
                    continue;
                }
                protected_lexical_spans = protected_lexical_spans.saturating_add(1);
                protected_lexical_bytes =
                    protected_lexical_bytes.saturating_add(span.end.saturating_sub(span.start));
                let has_semantic_highlight = styles[span.start..span.end].bytes().any(|style| {
                    matches!(
                        char::from(style),
                        STYLE_KEYWORD | STYLE_FUNCTION | STYLE_IDENTIFIER | STYLE_COLUMN
                    )
                });
                if has_semantic_highlight {
                    let (line, column) = highlight_sweep_line_column(&source, span.start);
                    let issue = format!(
                        "{}:{line}:{column}: {:?} span contains semantic highlight",
                        relative.display(),
                        span.kind
                    );
                    unexpected_highlights = unexpected_highlights.saturating_add(1);
                    file_unexpected_highlights = file_unexpected_highlights.saturating_add(1);
                    file_issues.push(issue.clone());
                    failures.push(issue);
                }
            }

            for audit in word_audits {
                checked_words = checked_words.saturating_add(1);
                let word = source.get(audit.start..audit.end).unwrap_or("");
                let upper = ascii_upper_cow(word).into_owned();
                let is_object_word = object_names.contains(&upper);
                let expected_style = audit
                    .expected_style
                    .or(is_object_word.then_some(STYLE_IDENTIFIER));
                let Some(expected_style) = expected_style else {
                    if audit.actual_style == STYLE_DEFAULT {
                        let (line, column) = highlight_sweep_line_column(&source, audit.start);
                        let location = format!("{}:{line}:{column}", relative.display());
                        ordinary_default_words
                            .entry(upper.clone())
                            .and_modify(|(count, _)| *count = count.saturating_add(1))
                            .or_insert((1, location.clone()));
                        file_ordinary_default_words
                            .entry(upper.clone())
                            .and_modify(|(count, _)| *count = count.saturating_add(1))
                            .or_insert((1, location.clone()));
                        if source[audit.end..]
                            .trim_start_matches(char::is_whitespace)
                            .starts_with('(')
                        {
                            ordinary_default_call_words
                                .entry(upper.clone())
                                .and_modify(|(count, _)| *count = count.saturating_add(1))
                                .or_insert((1, location.clone()));
                        }
                        if source[..audit.start].ends_with('%') {
                            ordinary_default_percent_words
                                .entry(upper)
                                .and_modify(|(count, _)| *count = count.saturating_add(1))
                                .or_insert((1, location));
                        }
                    } else {
                        let (line, column) = highlight_sweep_line_column(&source, audit.start);
                        let issue = format!(
                            "{}:{line}:{column}: `{word}` expected={} actual={} disposition={:?} unexpected-highlight",
                            relative.display(),
                            STYLE_DEFAULT,
                            audit.actual_style,
                            audit.disposition
                        );
                        unexpected_highlights = unexpected_highlights.saturating_add(1);
                        file_unexpected_highlights = file_unexpected_highlights.saturating_add(1);
                        file_issues.push(issue.clone());
                        failures.push(issue);
                    }
                    continue;
                };

                if is_object_word && audit.expected_style.is_none() {
                    checked_object_words = checked_object_words.saturating_add(1);
                    file_object_words = file_object_words.saturating_add(1);
                } else {
                    checked_catalog_words = checked_catalog_words.saturating_add(1);
                    file_catalog_words = file_catalog_words.saturating_add(1);
                }

                if audit.actual_style == expected_style
                    || (audit.disposition == HighlightWordDisposition::DateTimeLiteral
                        && audit.actual_style == STYLE_DATETIME_LITERAL)
                {
                    if is_object_word && audit.expected_style.is_none() {
                        highlighted_object_words = highlighted_object_words.saturating_add(1);
                    } else {
                        highlighted_catalog_words = highlighted_catalog_words.saturating_add(1);
                    }
                    continue;
                }
                if matches!(
                    audit.disposition,
                    HighlightWordDisposition::ControlAlias
                        | HighlightWordDisposition::LocalAlias
                        | HighlightWordDisposition::IdentifierContext
                        | HighlightWordDisposition::PathIdentifier
                ) && matches!(
                    audit.actual_style,
                    STYLE_DEFAULT | STYLE_IDENTIFIER | STYLE_COLUMN | STYLE_FUNCTION
                ) {
                    if is_object_word && audit.expected_style.is_none() {
                        contextual_object_words = contextual_object_words.saturating_add(1);
                    } else {
                        contextual_catalog_words = contextual_catalog_words.saturating_add(1);
                        file_contextual_words = file_contextual_words.saturating_add(1);
                    }
                    let upper = ascii_upper_cow(word);
                    let key = format!(
                        "{upper} actual={} disposition={:?}",
                        audit.actual_style, audit.disposition
                    );
                    let (line, column) = highlight_sweep_line_column(&source, audit.start);
                    let location = format!("{}:{line}:{column}", relative.display());
                    contextual_catalog_word_counts
                        .entry(key)
                        .and_modify(|(count, _)| *count = count.saturating_add(1))
                        .or_insert((1, location));
                    continue;
                }

                let (line, column) = highlight_sweep_line_column(&source, audit.start);
                let issue = format!(
                    "{}:{line}:{column}: `{word}` expected={expected_style} actual={} disposition={:?}",
                    relative.display(),
                    audit.actual_style,
                    audit.disposition
                );
                file_issues.push(issue.clone());
                failures.push(issue);
            }

            let file_name = relative
                .file_name()
                .and_then(|name| name.to_str())
                .unwrap_or("highlight.sql");
            let out_path = output_root
                .join(relative.parent().unwrap_or(std::path::Path::new("")))
                .join(format!("{file_name}.highlight.out"));
            if let Some(parent) = out_path.parent() {
                fs::create_dir_all(parent).expect("create highlight sweep output directory");
            }
            let mut report = format!(
                "Syntax-highlight sweep report\nsource={}\ndb={db_type:?}\nchecked_words={}\nchecked_catalog_words={file_catalog_words}\ncontextual_catalog_words={file_contextual_words}\nchecked_object_words={file_object_words}\nunexpected_highlights={file_unexpected_highlights}\nissues={}\n",
                relative.display(),
                file_checked_words,
                file_issues.len()
            );
            for issue in file_issues {
                report.push_str(&issue);
                report.push('\n');
            }
            report.push_str("ordinary_default_words:\n");
            for (word, (count, location)) in file_ordinary_default_words {
                report.push_str(&format!("{word}={count} first={location}\n"));
            }
            fs::write(&out_path, report)
                .unwrap_or_else(|err| panic!("failed to write `{}`: {err}", out_path.display()));
        }
    }

    let mut aggregate = format!(
        "Syntax-highlight sweep aggregate\nchecked_files={checked_files}\nchecked_words={checked_words}\nchecked_catalog_words={checked_catalog_words}\nhighlighted_catalog_words={highlighted_catalog_words}\ncontextual_catalog_words={contextual_catalog_words}\nchecked_object_words={checked_object_words}\nhighlighted_object_words={highlighted_object_words}\ncontextual_object_words={contextual_object_words}\nprotected_lexical_spans={protected_lexical_spans}\nprotected_lexical_bytes={protected_lexical_bytes}\nunexpected_highlights={unexpected_highlights}\nordinary_default_distinct_words={}\nfailures={}\n",
        ordinary_default_words.len(),
        failures.len()
    );
    aggregate.push_str("ordinary_default_words:\n");
    for (word, (count, location)) in ordinary_default_words {
        aggregate.push_str(&format!("{word}={count} first={location}\n"));
    }
    aggregate.push_str("ordinary_default_call_words:\n");
    for (word, (count, location)) in ordinary_default_call_words {
        aggregate.push_str(&format!("{word}={count} first={location}\n"));
    }
    aggregate.push_str("ordinary_default_percent_words:\n");
    for (word, (count, location)) in ordinary_default_percent_words {
        aggregate.push_str(&format!("{word}={count} first={location}\n"));
    }
    aggregate.push_str("contextual_catalog_words:\n");
    for (word, (count, location)) in contextual_catalog_word_counts {
        aggregate.push_str(&format!("{word}={count} first={location}\n"));
    }
    for failure in &failures {
        aggregate.push_str(failure);
        aggregate.push('\n');
    }
    fs::create_dir_all(&output_root).expect("create highlight sweep output directory");
    fs::write(output_root.join("highlight-sweep.out"), aggregate)
        .expect("write highlight sweep aggregate");
    assert!(
        failures.is_empty(),
        "syntax highlight sweep found {} style mismatches across {checked_files} files ({checked_words} words, {checked_catalog_words} catalog words, {checked_object_words} object words, {unexpected_highlights} unexpected highlights); see `{}`:\n{}",
        failures.len(),
        output_root.join("highlight-sweep.out").display(),
        failures.join("\n")
    );
}

fn highlight_sweep_line_column(source: &str, start: usize) -> (usize, usize) {
    let line = source[..start]
        .bytes()
        .filter(|byte| *byte == b'\n')
        .count()
        + 1;
    let line_start = source[..start].rfind('\n').map_or(0, |pos| pos + 1);
    let column = source[line_start..start].chars().count() + 1;
    (line, column)
}

fn highlight_sweep_fixture_objects(source: &str) -> (HighlightData, HashSet<String>) {
    use crate::ui::sql_editor::SqlToken;

    let spans = crate::ui::sql_editor::query_text::tokenize_sql_spanned(source);
    let mut data = HighlightData::new();
    let mut object_names = HashSet::new();
    let mut idx = 0usize;
    while idx < spans.len() {
        let SqlToken::Word(word) = &spans[idx].token else {
            idx += 1;
            continue;
        };
        if !word.eq_ignore_ascii_case("CREATE") {
            idx += 1;
            continue;
        }

        let mut scan = idx.saturating_add(1);
        let mut is_public = false;
        let mut is_materialized = false;
        let mut object_kind = None;
        while scan < spans.len() {
            match &spans[scan].token {
                SqlToken::Symbol(symbol) if symbol == ";" => break,
                SqlToken::Word(candidate) => {
                    let upper = candidate.to_ascii_uppercase();
                    match upper.as_str() {
                        "PUBLIC" => is_public = true,
                        "MATERIALIZED" => is_materialized = true,
                        "TABLE" | "VIEW" | "FUNCTION" | "PROCEDURE" | "PACKAGE" | "SEQUENCE"
                        | "TRIGGER" | "EVENT" | "TYPE" | "INDEX" | "SYNONYM" | "SCHEMA"
                        | "DATABASE" => {
                            object_kind = Some(upper);
                            break;
                        }
                        _ => {}
                    }
                }
                _ => {}
            }
            scan += 1;
        }
        let Some(object_kind) = object_kind else {
            idx += 1;
            continue;
        };

        scan = scan.saturating_add(1);
        let mut parts = Vec::new();
        while scan < spans.len() {
            match &spans[scan].token {
                SqlToken::Word(candidate)
                    if matches!(
                        candidate.to_ascii_uppercase().as_str(),
                        "BODY" | "IF" | "NOT" | "EXISTS"
                    ) => {}
                SqlToken::Word(candidate) => {
                    parts.push(candidate.to_ascii_uppercase());
                    if !spans.get(scan + 1).is_some_and(
                        |span| matches!(&span.token, SqlToken::Symbol(symbol) if symbol == "."),
                    ) {
                        break;
                    }
                }
                SqlToken::Symbol(symbol) if symbol == "." => {}
                SqlToken::Comment(_) => {}
                _ => break,
            }
            scan += 1;
        }
        let Some(name) = parts.last().cloned() else {
            idx += 1;
            continue;
        };
        if parts.len() > 1 {
            for schema in &parts[..parts.len() - 1] {
                push_highlight_sweep_name(&mut data.schemas, schema, &mut object_names);
            }
        }
        let target = match object_kind.as_str() {
            "TABLE" => &mut data.tables,
            "VIEW" if is_materialized => &mut data.materialized_views,
            "VIEW" => &mut data.views,
            "FUNCTION" => &mut data.functions,
            "PROCEDURE" => &mut data.procedures,
            "PACKAGE" => &mut data.packages,
            "SEQUENCE" => &mut data.sequences,
            "TRIGGER" => &mut data.triggers,
            "EVENT" => &mut data.events,
            "TYPE" => &mut data.types,
            "INDEX" => &mut data.indexes,
            "SYNONYM" if is_public => &mut data.public_synonyms,
            "SYNONYM" => &mut data.synonyms,
            "SCHEMA" | "DATABASE" => &mut data.schemas,
            _ => {
                idx += 1;
                continue;
            }
        };
        push_highlight_sweep_name(target, &name, &mut object_names);
        idx = scan.saturating_add(1);
    }

    (data, object_names)
}

fn push_highlight_sweep_name(
    target: &mut Vec<String>,
    name: &str,
    object_names: &mut HashSet<String>,
) {
    if object_names.insert(name.to_string()) {
        target.push(name.to_string());
    }
}

#[test]
fn production_full_highlight_keeps_oracle_grammar_out_of_alias_path() {
    let text = r#"
CREATE OR REPLACE TYPE pair_t AS OBJECT (k VARCHAR2(30), v CLOB);
CREATE TABLE t (
    id NUMBER GENERATED BY DEFAULT AS IDENTITY,
    upper_name VARCHAR2(30) GENERATED ALWAYS AS (UPPER(name)) VIRTUAL
) XMLTYPE COLUMN payload STORE AS BASICFILE CLOB;
DECLARE
    c_name CONSTANT VARCHAR2(30) := 'x';
    r t%ROWTYPE;
BEGIN
    SELECT COUNT(*) FROM t GROUP BY CUBE(status) GROUPING SETS ((status), ());
    x := seq.NEXTVAL;
    y := DBMS_UTILITY.FORMAT_ERROR_STACK;
    y := MATCH_NUMBER() + PREV(x) + SYS.ODCINUMBERLIST(1).COUNT;
    n := SQL%ROWCOUNT;
    n := SQL%BULK_EXCEPTIONS.COUNT;
    EXIT WHEN c%NOTFOUND;
    IF c%ISOPEN THEN NULL; END IF;
    SELECT CASE WHEN 1 = 1 THEN 1 ELSE 0 END, id FROM t FOR UPDATE;
END;
"#;
    let highlighter = SqlHighlighter::new();
    let (styles, _) =
        crate::ui::sql_editor::build_logical_styles_and_line_states(&highlighter, text);

    for keyword in [
        "OBJECT",
        "CLOB",
        "IDENTITY",
        "ALWAYS",
        "VIRTUAL",
        "BASICFILE",
        "CONSTANT",
        "ROWTYPE",
        "CUBE",
        "SETS",
        "NEXTVAL",
        "ROWCOUNT",
        "BULK_EXCEPTIONS",
        "NOTFOUND",
        "ISOPEN",
    ] {
        assert_token_has_style(text, &styles, keyword, STYLE_KEYWORD);
    }
    assert_token_has_style(text, &styles, "FORMAT_ERROR_STACK", STYLE_FUNCTION);
    for function in ["MATCH_NUMBER", "PREV", "ODCINUMBERLIST"] {
        assert_token_has_style(text, &styles, function, STYLE_FUNCTION);
    }
    let case_end = text.find("END, id").expect("CASE END");
    assert!(
        styles[case_end..case_end + 3]
            .chars()
            .all(|style| style == STYLE_KEYWORD),
        "CASE END must not be treated as an implicit alias"
    );
}

#[test]
fn production_full_highlight_covers_mysql_family_fixture_grammar() {
    let text = r#"
CREATE SEQUENCE s START WITH 1 INCREMENT BY 1 MINVALUE 1;
CREATE TABLE t (
    id INT,
    ip INET6,
    p POINT SRID 4326,
    tags JSON,
    generated_col INT AS (id + 1) PERSISTENT,
    PERIOD FOR SYSTEM_TIME(row_start, row_end),
    PRIMARY KEY(id, validity WITHOUT OVERLAPS),
    KEY tags_idx ((CAST(tags AS CHAR(24) ARRAY))),
    VECTOR INDEX vector_idx(embedding) M=4 DISTANCE=COSINE
) WITH SYSTEM VERSIONING;
UPDATE t FOR PORTION OF validity FROM '2026-01-01' TO '2026-02-01' SET id=1;
SELECT UUID_V7(), COLUMN_CHECK(attrs), COLUMN_JSON(attrs),
       'vip' MEMBER OF(tags), PERCENTILE_CONT(0.5) WITHIN GROUP (ORDER BY id)
FROM t LIMIT 3 ROWS EXAMINED 1000;
EXECUTE IMMEDIATE 'SELECT 1';
SELECT id FROM t MINUS SELECT id FROM t;
CYCLE id RESTRICT;
"#;
    let mut highlighter = SqlHighlighter::new();
    highlighter.set_db_type(DatabaseType::MariaDB);
    let (styles, _) =
        crate::ui::sql_editor::build_logical_styles_and_line_states(&highlighter, text);

    for keyword in [
        "SEQUENCE",
        "INCREMENT",
        "MINVALUE",
        "INET6",
        "SRID",
        "PERSISTENT",
        "PERIOD",
        "SYSTEM_TIME",
        "WITHOUT",
        "OVERLAPS",
        "ARRAY",
        "DISTANCE",
        "COSINE",
        "VERSIONING",
        "PORTION",
        "MEMBER",
        "OF",
        "WITHIN",
        "EXAMINED",
        "IMMEDIATE",
        "MINUS",
        "CYCLE",
    ] {
        assert_token_has_style(text, &styles, keyword, STYLE_KEYWORD);
    }
    for function in ["UUID_V7", "COLUMN_CHECK", "COLUMN_JSON"] {
        assert_token_has_style(text, &styles, function, STYLE_FUNCTION);
    }
}

#[test]
fn production_full_highlight_covers_manual_final_oracle_grammar() {
    let text = r#"
CREATE DOMAIN category_domain AS VARCHAR2(32) DISPLAY UPPER(VALUE);
CREATE TABLE child (embedding VECTOR(3, FLOAT32));
FORALL i IN INDICES OF values_to_update SAVE EXCEPTIONS
  UPDATE t SET value = value WHERE id = i;
CREATE PROPERTY GRAPH app_graph
  VERTEX TABLES (nodes KEY (id) LABEL node PROPERTIES (id))
  EDGE TABLES (edges KEY (id) SOURCE KEY (source_id) REFERENCES nodes (id)
    DESTINATION KEY (target_id) REFERENCES nodes (id) LABEL owns PROPERTIES (id));
CREATE MATERIALIZED VIEW app_mv BUILD IMMEDIATE REFRESH COMPLETE ON DEMAND AS SELECT 1 value FROM dual;
LOCK TABLE t IN ROW SHARE MODE NOWAIT;
EXPLAIN PLAN FOR SELECT 1 FROM dual;
ANALYZE TABLE t COMPUTE STATISTICS FOR ALL INDEXED COLUMNS;
SELECT JSON_SERIALIZE(payload RETURNING VARCHAR2 PRETTY),
       VECTOR_DISTANCE(embedding, TO_VECTOR('[1,1,1]'), COSINE)
FROM child MATCH_RECOGNIZE (AFTER MATCH SKIP PAST LAST ROW PATTERN (x));
"#;
    let mut highlighter = SqlHighlighter::new();
    highlighter.set_db_type(DatabaseType::Oracle);
    let (styles, _) =
        crate::ui::sql_editor::build_logical_styles_and_line_states(&highlighter, text);

    for keyword in [
        "BUILD",
        "COSINE",
        "DEMAND",
        "DESTINATION",
        "DISPLAY",
        "DOMAIN",
        "EDGE",
        "FLOAT32",
        "GRAPH",
        "INDEXED",
        "INDICES",
        "LABEL",
        "MODE",
        "PAST",
        "PLAN",
        "PRETTY",
        "PROPERTIES",
        "PROPERTY",
        "SAVE",
        "STATISTICS",
        "TABLES",
        "VECTOR",
        "VERTEX",
    ] {
        assert_word_has_style(text, &styles, keyword, STYLE_KEYWORD);
    }
    for function in ["TO_VECTOR", "VECTOR_DISTANCE"] {
        assert_word_has_style(text, &styles, function, STYLE_FUNCTION);
    }
}

#[test]
fn production_full_highlight_covers_manual_final_mysql_family_grammar() {
    let text = r#"
CREATE OR REPLACE VIEW app_v AS SELECT id FROM t WITH CASCADED CHECK OPTION;
GET CURRENT DIAGNOSTICS @condition_count = NUMBER;
COMMIT AND NO CHAIN;
CREATE EVENT syntax_event ON SCHEDULE AT CURRENT_TIMESTAMP
  ON COMPLETION PRESERVE DISABLE DO SELECT 1;
CREATE ROLE app_reader;
ALTER USER app_user PASSWORD EXPIRE INTERVAL 30 DAY;
DELETE HISTORY FROM contract_history BEFORE SYSTEM_TIME TIMESTAMP '2000-01-01 00:00:00';
SET STATEMENT max_statement_time = 5 FOR SELECT 1;
"#;
    let mut highlighter = SqlHighlighter::new();
    highlighter.set_db_type(DatabaseType::MariaDB);
    let (styles, _) =
        crate::ui::sql_editor::build_logical_styles_and_line_states(&highlighter, text);

    for keyword in [
        "AT",
        "CASCADED",
        "CHAIN",
        "COMPLETION",
        "EXPIRE",
        "HISTORY",
        "NUMBER",
        "ROLE",
        "STATEMENT",
    ] {
        assert_word_has_style(text, &styles, keyword, STYLE_KEYWORD);
    }
}

#[test]
fn test_number_highlighting_supports_single_decimal_point() {
    let highlighter = SqlHighlighter::new();
    let text = "SELECT 12.34 FROM dual";
    let styles = highlighter.generate_styles(text);

    let start = text.find("12.34").unwrap_or(0);
    let end = start + "12.34".len();
    assert!(styles[start..end].chars().all(|c| c == STYLE_NUMBER));
}

#[test]
fn test_number_highlighting_stops_before_second_decimal_point() {
    let highlighter = SqlHighlighter::new();
    let text = "SELECT 12.34.56 FROM dual";
    let styles = highlighter.generate_styles(text);

    let first_number_start = text.find("12.34").unwrap_or(0);
    let first_number_end = first_number_start + "12.34".len();
    assert!(styles[first_number_start..first_number_end]
        .chars()
        .all(|c| c == STYLE_NUMBER));

    let second_dot_pos = first_number_end;
    let second_number_end = second_dot_pos + ".56".len();
    assert!(styles[second_dot_pos..second_number_end]
        .chars()
        .all(|c| c == STYLE_NUMBER));
}

#[test]
fn test_number_highlighting_supports_exponent_notation() {
    let highlighter = SqlHighlighter::new();
    let text = "SELECT 1e10, 3.14E-2, .5e+7 FROM dual";
    let styles = highlighter.generate_styles(text);

    for token in ["1e10", "3.14E-2", ".5e+7"] {
        let start = text.find(token).unwrap_or(0);
        let end = start + token.len();
        assert!(
            styles[start..end].chars().all(|c| c == STYLE_NUMBER),
            "{token} should be highlighted as a single numeric literal"
        );
    }
}

#[test]
fn test_number_highlighting_does_not_consume_incomplete_exponent() {
    let highlighter = SqlHighlighter::new();
    let text = "SELECT 1e+, 2E- FROM dual";
    let styles = highlighter.generate_styles(text);

    let first = text.find("1e+").unwrap_or(0);
    assert_eq!(
        styles.as_bytes().get(first).copied(),
        Some(STYLE_NUMBER as u8)
    );
    assert_ne!(
        styles.as_bytes().get(first + 1).copied(),
        Some(STYLE_NUMBER as u8)
    );

    let second = text.find("2E-").unwrap_or(0);
    assert_eq!(
        styles.as_bytes().get(second).copied(),
        Some(STYLE_NUMBER as u8)
    );
    assert_ne!(
        styles.as_bytes().get(second + 1).copied(),
        Some(STYLE_NUMBER as u8)
    );
}

#[test]
fn test_open_for_highlights_for_as_keyword() {
    let highlighter = SqlHighlighter::new();
    let text = "begin
    open cv for
        select 1 from dual;
end;";
    let styles = highlighter.generate_styles(text);

    let for_start = text.find("for").unwrap_or(0);
    let for_end = for_start + 3;
    assert!(
        styles[for_start..for_end]
            .chars()
            .all(|c| c == STYLE_KEYWORD),
        "FOR in OPEN ... FOR should be highlighted as a keyword"
    );
}

#[test]
fn test_keyword_highlighting() {
    let highlighter = SqlHighlighter::new();
    let text = "SELECT * FROM";
    let styles = highlighter.generate_styles(text);

    // "SELECT" should be keyword (B)
    assert!(styles.starts_with("BBBBBB"));
}

#[test]
fn test_oracle_sequence_options_highlight_as_keywords() {
    let mut highlighter = SqlHighlighter::new();
    highlighter.set_db_type(crate::db::connection::DatabaseType::Oracle);
    let text = r#"CREATE SEQUENCE "SYSTEM"."MVIEW$_ADVSEQ_GENERIC" MINVALUE 1 MAXVALUE 4294967295 INCREMENT BY 1 START WITH 1 CACHE 50 NOORDER NOCYCLE NOKEEP NOSCALE GLOBAL;"#;
    let styles = highlighter.generate_styles(text);

    for token in ["CACHE", "NOORDER", "NOCYCLE", "NOKEEP", "NOSCALE", "GLOBAL"] {
        assert_token_has_style(text, &styles, token, STYLE_KEYWORD);
    }
}

#[test]
fn test_transaction_level_keywords_highlight_as_keywords() {
    let mut mysql_highlighter = SqlHighlighter::new();
    mysql_highlighter.set_db_type(crate::db::connection::DatabaseType::MySQL);
    let mysql_text = "SET GLOBAL TRANSACTION ISOLATION LEVEL READ COMMITTED";
    let mysql_styles = mysql_highlighter.generate_styles(mysql_text);
    for token in ["GLOBAL", "TRANSACTION", "ISOLATION", "LEVEL", "COMMITTED"] {
        assert_token_has_style(mysql_text, &mysql_styles, token, STYLE_KEYWORD);
    }
    let mysql_repeatable_text = "SET SESSION TRANSACTION ISOLATION LEVEL REPEATABLE READ";
    let mysql_repeatable_styles = mysql_highlighter.generate_styles(mysql_repeatable_text);
    for token in ["SESSION", "REPEATABLE"] {
        assert_token_has_style(
            mysql_repeatable_text,
            &mysql_repeatable_styles,
            token,
            STYLE_KEYWORD,
        );
    }
    let mysql_uncommitted_text = "SET TRANSACTION ISOLATION LEVEL READ UNCOMMITTED";
    let mysql_uncommitted_styles = mysql_highlighter.generate_styles(mysql_uncommitted_text);
    for token in ["READ", "UNCOMMITTED"] {
        assert_token_has_style(
            mysql_uncommitted_text,
            &mysql_uncommitted_styles,
            token,
            STYLE_KEYWORD,
        );
    }
    let mysql_access_text = "START TRANSACTION READ WRITE";
    let mysql_access_styles = mysql_highlighter.generate_styles(mysql_access_text);
    for token in ["START", "TRANSACTION", "READ", "WRITE"] {
        assert_token_has_style(
            mysql_access_text,
            &mysql_access_styles,
            token,
            STYLE_KEYWORD,
        );
    }
    let mysql_snapshot_text = "START TRANSACTION WITH CONSISTENT SNAPSHOT";
    let mysql_snapshot_styles = mysql_highlighter.generate_styles(mysql_snapshot_text);
    for token in ["WITH", "CONSISTENT", "SNAPSHOT"] {
        assert_token_has_style(
            mysql_snapshot_text,
            &mysql_snapshot_styles,
            token,
            STYLE_KEYWORD,
        );
    }

    let mut oracle_highlighter = SqlHighlighter::new();
    oracle_highlighter.set_db_type(crate::db::connection::DatabaseType::Oracle);
    let oracle_text = "SET TRANSACTION ISOLATION LEVEL READ COMMITTED";
    let oracle_styles = oracle_highlighter.generate_styles(oracle_text);
    for token in ["TRANSACTION", "ISOLATION", "LEVEL", "COMMITTED"] {
        assert_token_has_style(oracle_text, &oracle_styles, token, STYLE_KEYWORD);
    }
    let oracle_access_text = "SET TRANSACTION READ ONLY";
    let oracle_access_styles = oracle_highlighter.generate_styles(oracle_access_text);
    for token in ["TRANSACTION", "READ", "ONLY"] {
        assert_token_has_style(
            oracle_access_text,
            &oracle_access_styles,
            token,
            STYLE_KEYWORD,
        );
    }

    let oracle_audit_text = "AUDIT CREATE SESSION BY ACCESS";
    let oracle_audit_styles = oracle_highlighter.generate_styles(oracle_audit_text);
    assert_token_has_style(
        oracle_audit_text,
        &oracle_audit_styles,
        "ACCESS",
        STYLE_KEYWORD,
    );
}

#[test]
fn test_oracle_date_format_parameters_highlight_as_keywords() {
    let mut highlighter = SqlHighlighter::new();
    highlighter.set_db_type(crate::db::connection::DatabaseType::Oracle);
    let text = "\
ALTER SESSION SET NLS_DATE_FORMAT = 'YYYY-MM-DD';
ALTER SESSION SET NLS_TIMESTAMP_FORMAT = 'YYYY-MM-DD HH24:MI:SS';
ALTER SESSION SET NLS_TIMESTAMP_TZ_FORMAT = 'YYYY-MM-DD HH24:MI:SS TZH:TZM';
ALTER SESSION SET NLS_DATE_LANGUAGE = AMERICAN";
    let styles = highlighter.generate_styles(text);

    for token in [
        "ALTER",
        "SESSION",
        "SET",
        "NLS_DATE_FORMAT",
        "NLS_TIMESTAMP_FORMAT",
        "NLS_TIMESTAMP_TZ_FORMAT",
        "NLS_DATE_LANGUAGE",
    ] {
        assert_token_has_style(text, &styles, token, STYLE_KEYWORD);
    }
}

#[test]
fn test_char_datatype_is_highlighted_as_keyword() {
    let highlighter = SqlHighlighter::new();
    let text = "CREATE TABLE t_char (c CHAR(10))";
    let styles = highlighter.generate_styles(text);

    let start = text.find("CHAR").unwrap_or(0);
    let end = start + "CHAR".len();
    assert!(
        styles[start..end].chars().all(|c| c == STYLE_KEYWORD),
        "CHAR should be highlighted as a keyword"
    );
}

#[test]
fn test_plsql_diagnostics_are_highlighted_as_keywords() {
    let highlighter = SqlHighlighter::new();
    let text = r#"BEGIN
    DBMS_OUTPUT.PUT_LINE(SQLCODE);
    DBMS_OUTPUT.PUT_LINE(SQLERRM);
END;"#;
    let styles = highlighter.generate_styles(text);

    for token in ["SQLCODE", "SQLERRM"] {
        let start = text.find(token).unwrap_or(0);
        let end = start + token.len();
        assert!(
            styles[start..end].chars().all(|c| c == STYLE_KEYWORD),
            "{token} should be highlighted as a keyword"
        );
    }
}

#[test]
fn test_type_body_member_modifiers_are_highlighted_as_keywords() {
    let highlighter = SqlHighlighter::new();
    let text = r#"CREATE OR REPLACE TYPE BODY money_t AS
    CONSTRUCTOR FUNCTION money_t (p_amount IN NUMBER) RETURN SELF AS RESULT IS
    BEGIN
        RETURN;
    END;

    MAP MEMBER FUNCTION get_normalized RETURN NUMBER IS
    BEGIN
        RETURN 1;
    END;"#;
    let styles = highlighter.generate_styles(text);

    for token in ["CONSTRUCTOR", "MAP", "MEMBER"] {
        let start = text.find(token).unwrap_or(0);
        let end = start + token.len();
        assert!(
            styles[start..end].chars().all(|c| c == STYLE_KEYWORD),
            "{token} should be highlighted as a keyword"
        );
    }
}
#[test]
fn test_if_alias_member_access_is_not_highlighted_as_keyword() {
    let highlighter = SqlHighlighter::new();
    let text = "SELECT if.a, if.b FROM tablename if";
    let styles = highlighter.generate_styles(text);

    for token in ["if.a", "if.b"] {
        let start = text.find(token).unwrap_or(0);
        let alias_end = start + 2;
        assert!(
            styles[start..alias_end].chars().all(|c| c != STYLE_KEYWORD),
            "IF alias in `{token}` must not be highlighted as keyword"
        );
    }
}

#[test]
fn test_long_line_if_alias_member_access_stays_non_keyword() {
    let highlighter = SqlHighlighter::new();
    let mut text = String::from("SELECT ");
    for idx in 0..2048usize {
        if idx > 0 {
            text.push_str(", ");
        }
        text.push_str(&format!("if.col_{idx:04}"));
    }
    text.push_str(" FROM tablename if");

    let styles = highlighter.generate_styles(&text);

    for idx in [0usize, 511, 1023, 1535, 2047] {
        let token = format!("if.col_{idx:04}");
        let start = text.find(&token).unwrap_or(0);
        let alias_end = start + 2;
        assert!(
            styles[start..alias_end].chars().all(|c| c != STYLE_KEYWORD),
            "IF alias in `{token}` must not be highlighted as keyword on long lines"
        );
    }
}

#[test]
fn test_trim_cte_alias_and_qualified_access_are_not_function_highlighted() {
    let highlighter = SqlHighlighter::new();
    let text = "WITH trim AS\n\
(\n\
    SELECT\n\
        a,\n\
        TRIM(b) AS b_trimmed,\n\
        c\n\
    FROM qt_kw_base\n\
)\n\
SELECT trim.a, trim.b_trimmed, trim.c\n\
FROM trim\n\
ORDER BY trim.a;";
    let styles = highlighter.generate_styles(text);

    let cte_trim_start = text.find("trim AS").unwrap_or(0);
    assert!(
        styles[cte_trim_start..cte_trim_start + 4]
            .chars()
            .all(|c| c != STYLE_FUNCTION && c != STYLE_KEYWORD),
        "CTE alias trim should not be highlighted as function or keyword"
    );

    let function_trim_start = text.find("TRIM(").unwrap_or(0);
    assert!(
        styles[function_trim_start..function_trim_start + 4]
            .chars()
            .all(|c| c == STYLE_FUNCTION),
        "TRIM function call should remain function-highlighted"
    );

    for token in ["trim.a", "trim.b_trimmed", "trim.c"] {
        let start = text.find(token).unwrap_or(0);
        assert!(
            styles[start..start + 4]
                .chars()
                .all(|c| c != STYLE_FUNCTION && c != STYLE_KEYWORD),
            "qualified alias `{token}` should not be highlighted as function or keyword"
        );
    }

    let from_trim_start = text.find("FROM trim").unwrap_or(0) + 5;
    assert!(
        styles[from_trim_start..from_trim_start + 4]
            .chars()
            .all(|c| c != STYLE_FUNCTION && c != STYLE_KEYWORD),
        "relation reference trim should not be highlighted as function or keyword"
    );
}

#[test]
fn test_oracle_lengthb_is_function_highlighted() {
    let highlighter = SqlHighlighter::new();
    let text = r#"SELECT
    CAST (REPLACE (RPAD ('x', 1333, 'x'), 'x', UNISTR ('\D55C')) AS VARCHAR2 (4000)) AS payload_ko,
    LENGTHB (CAST (REPLACE (RPAD ('x', 1333, 'x'), 'x', UNISTR ('\D55C')) AS VARCHAR2 (4000))) AS payload_bytes
FROM dual;"#;
    let styles = highlighter.generate_styles(text);

    assert_token_has_style(text, &styles, "LENGTHB", STYLE_FUNCTION);
}

#[test]
fn test_predefined_plsql_exceptions_are_highlighted() {
    let highlighter = SqlHighlighter::new();
    let text = "BEGIN\n\
    SELECT 1 INTO v FROM dual;\n\
EXCEPTION\n\
    WHEN NO_DATA_FOUND THEN\n\
        RAISE_APPLICATION_ERROR(-20001, 'x');\n\
    WHEN TOO_MANY_ROWS THEN\n\
        NULL;\n\
END;";
    let styles = highlighter.generate_styles(text);

    for token in ["NO_DATA_FOUND", "TOO_MANY_ROWS"] {
        let start = text.find(token).unwrap();
        assert!(
            styles[start..start + token.len()]
                .chars()
                .all(|c| c == STYLE_FUNCTION),
            "predefined exception `{token}` should be highlighted like a builtin"
        );
    }

    let raise_start = text.find("RAISE_APPLICATION_ERROR").unwrap();
    assert!(
        styles[raise_start..raise_start + "RAISE_APPLICATION_ERROR".len()]
            .chars()
            .all(|c| c == STYLE_KEYWORD),
        "RAISE_APPLICATION_ERROR should be highlighted as a PL/SQL keyword"
    );
}

#[test]
fn test_function_name_alias_after_dot_is_not_function_highlighted() {
    let highlighter = SqlHighlighter::new();
    let text = "SELECT o.trim, o.count, o.max FROM orders o";
    let styles = highlighter.generate_styles(text);

    for token in ["trim", "count", "max"] {
        let qualified = format!("o.{token}");
        let start = text.find(&qualified).unwrap_or(0) + 2;
        let end = start + token.len();
        assert!(
            styles[start..end]
                .chars()
                .all(|c| c != STYLE_FUNCTION && c != STYLE_KEYWORD),
            "member access `{qualified}` should not be highlighted as function or keyword"
        );
    }
}

#[test]
fn test_schema_qualified_function_name_is_not_function_highlighted() {
    let highlighter = SqlHighlighter::new();
    let text = "SELECT user_env.trim FROM dual user_env";
    let styles = highlighter.generate_styles(text);

    let token = "user_env.trim";
    let trim_start = text.find(token).unwrap_or(0) + "user_env.".len();
    let trim_end = trim_start + "trim".len();
    assert!(
        styles[trim_start..trim_end]
            .chars()
            .all(|c| c != STYLE_FUNCTION && c != STYLE_KEYWORD),
        "qualified name `user_env.trim` should not be highlighted as function or keyword"
    );
}

#[test]
fn test_keyword_like_member_access_is_not_highlighted_as_keyword() {
    let highlighter = SqlHighlighter::new();
    let text = "SELECT t.group, t.order, t.from FROM keyword_table t";
    let styles = highlighter.generate_styles(text);

    for keyword_name in ["group", "order", "from"] {
        let qualified = format!("t.{keyword_name}");
        let start = text.find(&qualified).unwrap_or(0) + 2;
        let end = start + keyword_name.len();
        assert!(
            styles[start..end]
                .chars()
                .all(|c| c != STYLE_KEYWORD && c != STYLE_FUNCTION),
            "member access `{qualified}` should not be highlighted as keyword or function"
        );
    }
}

#[test]
fn test_keyword_like_alias_before_dot_is_not_highlighted_as_keyword() {
    let highlighter = SqlHighlighter::new();
    let text = "SELECT from_alias.col1, from.col2 FROM dual from";
    let styles = highlighter.generate_styles(text);

    let alias_in_member = text.find("from.col2").unwrap_or(0);
    assert!(
        styles[alias_in_member..alias_in_member + 4]
            .chars()
            .all(|c| c != STYLE_KEYWORD && c != STYLE_FUNCTION),
        "alias `from` used before dot should not be highlighted as keyword"
    );
}

#[test]
fn test_alias_matching_loaded_object_name_is_not_object_highlighted() {
    let mut highlighter = SqlHighlighter::new();
    highlighter.set_highlight_data(HighlightData {
        tables: vec!["ALL_OBJECTS".to_string(), "A".to_string()],
        columns: vec!["A".to_string(), "OBJECT_ID".to_string()],
        ..HighlightData::new()
    });

    let text = "SELECT a.object_id, b.object_id FROM all_objects a JOIN all_objects b ON b.object_id = a.object_id";
    let styles = highlighter.generate_styles(text);

    assert_token_has_style(text, &styles, "all_objects", STYLE_IDENTIFIER);

    let first_alias_use = text.find("a.object_id").unwrap_or(0);
    assert!(
        styles[first_alias_use..first_alias_use + 1]
            .chars()
            .all(|c| c == STYLE_DEFAULT),
        "alias qualifier `a` must not be highlighted as the loaded object A"
    );

    let alias_declaration = text.find("all_objects a").unwrap_or(0) + "all_objects ".len();
    assert!(
        styles[alias_declaration..alias_declaration + 1]
            .chars()
            .all(|c| c == STYLE_DEFAULT),
        "alias declaration `a` must not be highlighted as the loaded object A"
    );

    let second_alias_use = text.rfind("a.object_id").unwrap_or(0);
    assert!(
        styles[second_alias_use..second_alias_use + 1]
            .chars()
            .all(|c| c == STYLE_DEFAULT),
        "later alias qualifier `a` must stay non-object even when A exists in metadata"
    );
}

#[test]
fn test_long_line_keyword_like_aliases_after_as_stay_non_keyword() {
    let highlighter = SqlHighlighter::new();
    let mut text = String::from("SELECT ");
    let mut alias_positions: Vec<(usize, &'static str)> = Vec::new();
    for idx in 0..1024usize {
        if idx > 0 {
            text.push_str(", ");
        }
        let alias = if idx % 2 == 0 { "if" } else { "end" };
        let fragment = format!("{idx} AS {alias}");
        text.push_str(&fragment);
        if matches!(idx, 0 | 255 | 511 | 767 | 1023) {
            alias_positions.push((text.len().saturating_sub(alias.len()), alias));
        }
    }
    text.push_str(" FROM dual");

    let styles = highlighter.generate_styles(&text);

    for (start, alias) in alias_positions {
        let end = start + alias.len();
        assert!(
            styles[start..end]
                .chars()
                .all(|c| c != STYLE_KEYWORD && c != STYLE_FUNCTION),
            "alias `{alias}` must remain identifier-highlighted on long lines"
        );
    }
}

#[test]
fn test_string_highlighting() {
    let highlighter = SqlHighlighter::new();
    let text = "'hello world'";
    let styles = highlighter.generate_styles(text);

    // Entire string should be string style (D)
    assert!(styles.chars().all(|c| c == STYLE_STRING));
}

#[test]
fn test_comment_highlighting() {
    let highlighter = SqlHighlighter::new();
    let text = "-- this is a comment";
    let styles = highlighter.generate_styles(text);

    // Entire line should be comment style (E)
    assert!(styles.chars().all(|c| c == STYLE_COMMENT));
}

#[test]
fn test_keyword_after_line_comment_with_trailing_dot_is_highlighted() {
    let highlighter = SqlHighlighter::new();
    let text = "-- note.\nSELECT * FROM dual";
    let styles = highlighter.generate_styles(text);

    let select_start = text.find("SELECT").unwrap_or(0);
    assert!(styles[select_start..select_start + 6]
        .chars()
        .all(|c| c == STYLE_KEYWORD));
}

#[test]
fn test_keyword_after_line_comment_with_trailing_punctuation_is_highlighted() {
    let highlighter = SqlHighlighter::new();

    for trailing in [".", ")", ","] {
        let text = format!("-- trailing{trailing}\nFROM dual");
        let styles = highlighter.generate_styles(&text);
        let from_start = text.find("FROM").unwrap_or(0);
        assert!(
            styles[from_start..from_start + 4]
                .chars()
                .all(|c| c == STYLE_KEYWORD),
            "FROM should be highlighted after comment ending with `{trailing}`"
        );
    }
}

#[test]
fn test_prompt_highlighting() {
    let highlighter = SqlHighlighter::new();
    let text = "PROMPT Enter value for id";
    let styles = highlighter.generate_styles(text);

    assert!(styles.chars().all(|c| c == STYLE_COMMENT));
}

#[test]
fn test_prompt_highlighting_with_leading_whitespace() {
    let highlighter = SqlHighlighter::new();
    let text = "  prompt Enter value\nSELECT * FROM dual";
    let styles = highlighter.generate_styles(text);

    let first_line_end = text.find('\n').unwrap();
    assert!(styles[..first_line_end].chars().all(|c| c == STYLE_COMMENT));
    assert!(styles[first_line_end + 1..]
        .chars()
        .any(|c| c != STYLE_COMMENT));
}

#[test]
fn test_prompt_highlighting_with_carriage_return_line_break() {
    let highlighter = SqlHighlighter::new();
    let text = "PROMPT first line\rSELECT * FROM dual";
    let styles = highlighter.generate_styles(text);

    let prompt_end = text.find('\r').unwrap();
    assert!(styles[..prompt_end].chars().all(|c| c == STYLE_COMMENT));

    let select_start = text.find("SELECT").unwrap();
    assert!(styles[select_start..select_start + 6]
        .chars()
        .all(|c| c == STYLE_KEYWORD));
}

#[test]
fn test_connect_line_disables_rest_highlighting() {
    let highlighter = SqlHighlighter::new();
    let text = "CONNECT system/password@localhost:1521/FREE\nSELECT * FROM dual";
    let styles = highlighter.generate_styles(text);

    let first_line_end = text.find('\n').unwrap();
    assert!(styles[0..7].chars().all(|c| c == STYLE_KEYWORD));
    assert!(styles[7..first_line_end]
        .chars()
        .all(|c| c == STYLE_DEFAULT));

    let select_start = text.find("SELECT").unwrap();
    assert!(styles[select_start..select_start + 6]
        .chars()
        .all(|c| c == STYLE_KEYWORD));
}

#[test]
fn test_connect_line_with_leading_whitespace_disables_rest_highlighting() {
    let highlighter = SqlHighlighter::new();
    let text = "  connect system/password@localhost:1521/FREE";
    let styles = highlighter.generate_styles(text);

    assert!(styles[..2].chars().all(|c| c == STYLE_DEFAULT));
    assert!(styles[2..9].chars().all(|c| c == STYLE_KEYWORD));
    assert!(styles[9..].chars().all(|c| c == STYLE_DEFAULT));
}

#[test]
fn test_connect_line_with_carriage_return_line_break_disables_rest_highlighting() {
    let highlighter = SqlHighlighter::new();
    let text = "CONNECT system/password@localhost:1521/FREE\rSELECT * FROM dual";
    let styles = highlighter.generate_styles(text);

    let line_break = text.find('\r').unwrap();
    assert!(styles[0..7].chars().all(|c| c == STYLE_KEYWORD));
    assert!(styles[7..line_break].chars().all(|c| c == STYLE_DEFAULT));

    let select_start = text.find("SELECT").unwrap();
    assert!(styles[select_start..select_start + 6]
        .chars()
        .all(|c| c == STYLE_KEYWORD));
}

#[test]
fn test_connect_by_prior_keywords_are_highlighted() {
    let highlighter = SqlHighlighter::new();
    let text = "SELECT node_id FROM oqt_t_tree CONNECT BY PRIOR node_id = parent_id";
    let styles = highlighter.generate_styles(text);

    for keyword in ["CONNECT", "BY", "PRIOR"] {
        let start = text
            .find(keyword)
            .unwrap_or_else(|| panic!("missing keyword: {keyword}"));
        let end = start + keyword.len();
        assert!(
            styles[start..end].chars().all(|c| c == STYLE_KEYWORD),
            "{keyword} should be highlighted as keyword"
        );
    }
}

#[test]
fn test_connect_with_comment_then_by_is_not_sqlplus_connect() {
    let highlighter = SqlHighlighter::new();
    let text = "SELECT level FROM dual CONNECT /*comment*/ BY level <= 2";
    let styles = highlighter.generate_styles(text);

    for keyword in ["CONNECT", "BY"] {
        let start = text
            .find(keyword)
            .unwrap_or_else(|| panic!("missing keyword: {keyword}"));
        let end = start + keyword.len();
        assert!(
            styles[start..end].chars().all(|c| c == STYLE_KEYWORD),
            "{keyword} should be highlighted as keyword"
        );
    }
}

#[test]
fn test_connect_followed_by_newline_and_by_is_not_sqlplus_connect() {
    let highlighter = SqlHighlighter::new();
    let text = "SELECT level\nFROM dual\nCONNECT\nBY PRIOR id = parent_id";
    let styles = highlighter.generate_styles(text);

    for keyword in ["CONNECT", "BY", "PRIOR"] {
        let start = text
            .find(keyword)
            .unwrap_or_else(|| panic!("missing keyword: {keyword}"));
        let end = start + keyword.len();
        assert!(
            styles[start..end].chars().all(|c| c == STYLE_KEYWORD),
            "{keyword} should be highlighted as keyword"
        );
    }
}

#[test]
fn test_connect_followed_by_comment_line_then_by_is_not_sqlplus_connect() {
    let highlighter = SqlHighlighter::new();
    let text = "SELECT level\nFROM dual\nCONNECT\n-- keep hierarchy\nBY PRIOR id = parent_id";
    let styles = highlighter.generate_styles(text);

    for keyword in ["CONNECT", "BY", "PRIOR"] {
        let start = text
            .find(keyword)
            .unwrap_or_else(|| panic!("missing keyword: {keyword}"));
        let end = start + keyword.len();
        assert!(
            styles[start..end].chars().all(|c| c == STYLE_KEYWORD),
            "{keyword} should be highlighted as keyword"
        );
    }
}

#[test]
fn test_parse_connect_continuation_detects_by_with_comment() {
    let bytes = b"CONNECT /*+ hint */ BY PRIOR id = parent_id";
    let connect_end = "CONNECT".len();
    assert_eq!(
        parse_connect_continuation(bytes, connect_end),
        ConnectContinuation::ByClause
    );
}

#[test]
fn test_parse_connect_continuation_detects_sqlplus_connect_line() {
    let bytes = b"CONNECT system/password@db";
    let connect_end = "CONNECT".len();
    assert_eq!(
        parse_connect_continuation(bytes, connect_end),
        ConnectContinuation::Other
    );
}

#[test]
fn test_parse_connect_continuation_detects_multiline_by_clause() {
    let bytes = b"CONNECT\n-- comment\nBY PRIOR id = parent_id";
    let connect_end = "CONNECT".len();
    assert_eq!(
        parse_connect_continuation(bytes, connect_end),
        ConnectContinuation::ByClause
    );
}

#[test]
fn test_prompt_keyword_with_large_start_is_safe() {
    assert!(!is_prompt_keyword(b"PROMPT", usize::MAX));
}

#[test]
fn test_connect_keyword_with_large_start_is_safe() {
    assert!(!is_connect_keyword(b"CONNECT", usize::MAX));
}

#[test]
fn test_sqlplus_break_compute_highlighting() {
    let highlighter = SqlHighlighter::new();
    let text = "BREAK ON deptno\nCOMPUTE SUM\nCOLUMN col NEW_VALUE var";
    let styles = highlighter.generate_styles(text);

    let break_pos = text.find("BREAK").unwrap();
    assert!(styles[break_pos..break_pos + 5]
        .chars()
        .all(|c| c == STYLE_KEYWORD));

    let compute_pos = text.find("COMPUTE").unwrap();
    assert!(styles[compute_pos..compute_pos + 7]
        .chars()
        .all(|c| c == STYLE_KEYWORD));

    let new_value_pos = text.find("NEW_VALUE").unwrap();
    assert!(styles[new_value_pos..new_value_pos + 9]
        .chars()
        .all(|c| c == STYLE_KEYWORD));
}

#[test]
fn test_sqlplus_set_spool_keywords_highlighting() {
    let highlighter = SqlHighlighter::new();
    let text = "SET TRIMSPOOL ON\nSET COLSEP ||\nSET NULL (null)\nSPOOL APPEND\nSPOOL OFF";
    let styles = highlighter.generate_styles(text);

    let trimspool_pos = text.find("TRIMSPOOL").unwrap();
    assert!(styles[trimspool_pos..trimspool_pos + 9]
        .chars()
        .all(|c| c == STYLE_KEYWORD));

    let colsep_pos = text.find("COLSEP").unwrap();
    assert!(styles[colsep_pos..colsep_pos + 6]
        .chars()
        .all(|c| c == STYLE_KEYWORD));

    let spool_append_pos = text.find("SPOOL APPEND").unwrap();
    assert!(styles[spool_append_pos..spool_append_pos + 5]
        .chars()
        .all(|c| c == STYLE_KEYWORD));
    assert!(styles[spool_append_pos + 6..spool_append_pos + 12]
        .chars()
        .all(|c| c == STYLE_KEYWORD));

    let spool_off_pos = text.rfind("SPOOL OFF").unwrap();
    assert!(styles[spool_off_pos..spool_off_pos + 5]
        .chars()
        .all(|c| c == STYLE_KEYWORD));
    assert!(styles[spool_off_pos + 6..spool_off_pos + 9]
        .chars()
        .all(|c| c == STYLE_KEYWORD));
}

#[test]
fn test_alter_session_keywords_highlighting() {
    let highlighter = SqlHighlighter::new();
    let text = "ALTER SESSION SET CURRENT_SCHEMA = app_user";
    let styles = highlighter.generate_styles(text);

    for keyword in ["ALTER", "SESSION", "SET", "CURRENT_SCHEMA"] {
        let start = text.find(keyword).unwrap();
        let end = start + keyword.len();
        assert!(
            styles[start..end].chars().all(|c| c == STYLE_KEYWORD),
            "{} should be highlighted as keyword",
            keyword
        );
    }
}

#[test]
fn test_minus_keyword_highlighting() {
    let highlighter = SqlHighlighter::new();
    let text = "SELECT 1 FROM dual MINUS SELECT 2 FROM dual";
    let styles = highlighter.generate_styles(text);

    let minus_pos = text.find("MINUS").unwrap();
    assert!(
        styles[minus_pos..minus_pos + 5]
            .chars()
            .all(|c| c == STYLE_KEYWORD),
        "MINUS should be highlighted as keyword"
    );
}

#[test]
fn test_match_recognize_keywords_highlighting() {
    let highlighter = SqlHighlighter::new();
    let text = "SELECT * FROM oqt_t_emp MATCH_RECOGNIZE (ONE ROW PER MATCH PATTERN (a b+))";
    let styles = highlighter.generate_styles(text);

    for keyword in ["MATCH_RECOGNIZE", "ONE", "PER", "MATCH", "PATTERN"] {
        let start = text
            .find(keyword)
            .unwrap_or_else(|| panic!("missing keyword: {keyword}"));
        let end = start + keyword.len();
        assert!(
            styles[start..end].chars().all(|c| c == STYLE_KEYWORD),
            "{keyword} should be highlighted as keyword"
        );
    }
}

#[test]
fn test_match_recognize_classifier_keyword_highlighting() {
    let highlighter = SqlHighlighter::new();
    let text = "SELECT * FROM qt_sales MATCH_RECOGNIZE (MEASURES CLASSIFIER() AS cls PATTERN (A+) DEFINE A AS A.amount > 0)";
    let styles = highlighter.generate_styles(text);

    let start = text.find("CLASSIFIER").expect("missing CLASSIFIER token");
    let end = start + "CLASSIFIER".len();
    assert!(
        styles[start..end].chars().all(|c| c == STYLE_KEYWORD),
        "CLASSIFIER should be highlighted as keyword in MATCH_RECOGNIZE"
    );
}

#[test]
fn test_additional_oracle_structural_keywords_highlighting() {
    let highlighter = SqlHighlighter::new();

    for (text, keywords) in [
        (
            "SELECT * FROM XMLTABLE('/x' PASSING payload COLUMNS id NUMBER PATH '$.id') t",
            &["XMLTABLE"][..],
        ),
        (
            "WITH XMLNAMESPACES (DEFAULT 'urn:emp') SELECT * FROM dual",
            &["XMLNAMESPACES"][..],
        ),
        (
            "SELECT CAST(COLLECT(empno) AS sys.odcinumberlist) MULTISET UNION DISTINCT SELECT CAST(COLLECT(empno) AS sys.odcinumberlist) FROM emp",
            &["MULTISET"][..],
        ),
        (
            "SELECT sum(sal) OVER (ORDER BY empno GROUPS BETWEEN 1 PRECEDING AND CURRENT ROW) FROM emp",
            &["GROUPS"][..],
        ),
        (
            "SELECT * FROM sales MATCH_RECOGNIZE (WITH UNMATCHED ROWS SHOW EMPTY MATCHES PATTERN (A B+))",
            &["UNMATCHED", "MATCHES"][..],
        ),
    ] {
        let styles = highlighter.generate_styles(text);

        for keyword in keywords {
            let start = text.find(keyword).unwrap_or(0);
            let end = start + keyword.len();
            assert!(
                styles[start..end].chars().all(|c| c == STYLE_KEYWORD),
                "{keyword} should be highlighted as keyword in: {text}"
            );
        }
    }
}

#[test]
fn test_path_in_recursive_cte_column_list_is_not_keyword() {
    let highlighter = SqlHighlighter::new();
    let text = "WITH r (node_id, parent_id, node_name, lvl, path) AS (\n\
  SELECT node_id, parent_id, node_name, 1 AS lvl, '/'||node_name AS path\n\
  FROM oqt_t_tree\n\
)\n\
SELECT r.path FROM r";
    let styles = highlighter.generate_styles(text);

    let cte_path_start = text.find("path) AS").unwrap();
    assert!(
        styles[cte_path_start..cte_path_start + 4]
            .chars()
            .all(|c| c == STYLE_DEFAULT),
        "CTE explicit column name `path` should not be keyword"
    );

    let alias_path_start = text.find("AS path").unwrap() + 3;
    assert!(
        styles[alias_path_start..alias_path_start + 4]
            .chars()
            .all(|c| c == STYLE_DEFAULT),
        "SELECT alias `path` should not be keyword"
    );

    let qualified_path_start = text.find("r.path").unwrap() + 2;
    assert!(
        styles[qualified_path_start..qualified_path_start + 4]
            .chars()
            .all(|c| c == STYLE_DEFAULT),
        "qualified column `r.path` should not be keyword"
    );
}

#[test]
fn test_path_keyword_in_xmltable_remains_keyword() {
    let highlighter = SqlHighlighter::new();
    let text = "SELECT * FROM XMLTABLE('/x' PASSING payload COLUMNS id NUMBER PATH '$.id') t";
    let styles = highlighter.generate_styles(text);

    let path_start = text.find("PATH").unwrap();
    assert!(
        styles[path_start..path_start + 4]
            .chars()
            .all(|c| c == STYLE_KEYWORD),
        "XMLTABLE PATH clause should stay keyword style"
    );
}

#[test]
fn test_path_keyword_in_xmltable_remains_keyword_for_nq_and_uq_literals() {
    let highlighter = SqlHighlighter::new();

    for text in [
        "SELECT * FROM XMLTABLE(nq'[x]' PASSING payload COLUMNS id NUMBER PATH '$.id') t",
        "SELECT * FROM XMLTABLE(uq'[x]' PASSING payload COLUMNS id NUMBER PATH '$.id') t",
        "SELECT * FROM XMLTABLE(UQ'[x]' PASSING payload COLUMNS id NUMBER PATH '$.id') t",
    ] {
        let styles = highlighter.generate_styles(text);
        let path_start = text.find("PATH").unwrap_or(0);
        assert!(
            styles[path_start..path_start + 4]
                .chars()
                .all(|c| c == STYLE_KEYWORD),
            "XMLTABLE PATH clause should stay keyword style for text: {text}"
        );
    }
}

#[test]
fn test_q_quote_highlighting() {
    let highlighter = SqlHighlighter::new();
    let text = "SELECT q'[test string]' FROM dual";
    let styles = highlighter.generate_styles(text);

    // "SELECT" (0-5) should be keyword (B)
    assert!(
        styles[0..6].chars().all(|c| c == STYLE_KEYWORD),
        "SELECT should be keyword, got: {}",
        &styles[0..6]
    );

    // "q'[test string]'" (7-22) should be string (D)
    // Find the position of q'[
    let q_start = text.find("q'[").unwrap();
    let q_end = text.find("]'").unwrap() + 2;
    assert!(
        styles[q_start..q_end]
            .chars()
            .all(|c| c == STYLE_Q_QUOTE_STRING),
        "q'[...]' should be q-quote string style, got: {}",
        &styles[q_start..q_end]
    );
}

#[test]
fn test_nq_quote_highlighting() {
    let highlighter = SqlHighlighter::new();
    let text = "SELECT nq'[national string]' FROM dual";
    let styles = highlighter.generate_styles(text);

    // "SELECT" should be keyword (B)
    assert!(
        styles[0..6].chars().all(|c| c == STYLE_KEYWORD),
        "SELECT should be keyword"
    );

    // "nq'[national string]'" should be string (D)
    let nq_start = text.find("nq'[").unwrap();
    let nq_end = text.find("]'").unwrap() + 2;
    assert!(
        styles[nq_start..nq_end]
            .chars()
            .all(|c| c == STYLE_Q_QUOTE_STRING),
        "nq'[...]' should be q-quote string style, got: {}",
        &styles[nq_start..nq_end]
    );
}

#[test]
fn test_nq_quote_case_insensitive_highlighting() {
    let highlighter = SqlHighlighter::new();

    // Test NQ (uppercase)
    let text1 = "SELECT NQ'[test]' FROM dual";
    let styles1 = highlighter.generate_styles(text1);
    let nq_start1 = text1.find("NQ'[").unwrap();
    let nq_end1 = text1.find("]'").unwrap() + 2;
    assert!(
        styles1[nq_start1..nq_end1]
            .chars()
            .all(|c| c == STYLE_Q_QUOTE_STRING),
        "NQ'[...]' should be q-quote string style"
    );

    // Test Nq (mixed case)
    let text2 = "SELECT Nq'[test]' FROM dual";
    let styles2 = highlighter.generate_styles(text2);
    let nq_start2 = text2.find("Nq'[").unwrap();
    let nq_end2 = text2.find("]'").unwrap() + 2;
    assert!(
        styles2[nq_start2..nq_end2]
            .chars()
            .all(|c| c == STYLE_Q_QUOTE_STRING),
        "Nq'[...]' should be q-quote string style"
    );
}

#[test]
fn test_uq_quote_highlighting() {
    let highlighter = SqlHighlighter::new();
    let text = "SELECT uq'[unicode string]' FROM dual";
    let styles = highlighter.generate_styles(text);

    let uq_start = text.find("uq'[").unwrap();
    let uq_end = text.find("]'").unwrap() + 2;
    assert!(
        styles[uq_start..uq_end]
            .chars()
            .all(|c| c == STYLE_Q_QUOTE_STRING),
        "uq'[...]' should be q-quote string style, got: {}",
        &styles[uq_start..uq_end]
    );
}

#[test]
fn test_unicode_q_quote_delimiter_highlighting() {
    let highlighter = SqlHighlighter::new();
    let text = "SELECT q'가한글가' AS txt FROM dual";
    let styles = highlighter.generate_styles(text);

    let q_start = text.find("q'가").unwrap();
    let q_end = text.find("가'").unwrap() + "가'".len();
    assert!(
        styles[q_start..q_end]
            .chars()
            .all(|c| c == STYLE_Q_QUOTE_STRING),
        "unicode q-quote should remain one string span, got: {}",
        &styles[q_start..q_end]
    );
}

#[test]
fn test_unicode_uq_quote_delimiter_highlighting() {
    let highlighter = SqlHighlighter::new();
    let text = "SELECT uq'가문자열가' AS txt FROM dual";
    let styles = highlighter.generate_styles(text);

    let uq_start = text.find("uq'가").unwrap();
    let uq_end = text.find("가'").unwrap() + "가'".len();
    assert!(
        styles[uq_start..uq_end]
            .chars()
            .all(|c| c == STYLE_Q_QUOTE_STRING),
        "unicode uq-quote should remain one string span, got: {}",
        &styles[uq_start..uq_end]
    );
}

#[test]
fn test_prefixed_single_quote_literals_highlighting() {
    let highlighter = SqlHighlighter::new();
    let text = "SELECT n'가', b'0101', x'FF', u'유니코드', u&'\\0041' FROM dual";
    let styles = highlighter.generate_styles(text);

    for literal in ["n'가'", "b'0101'", "x'FF'", "u'유니코드'", "u&'\\0041'"] {
        let start = text.find(literal).unwrap();
        let end = start + literal.len();
        assert!(
            styles[start..end].chars().all(|c| c == STYLE_STRING),
            "prefixed literal should be a single string span: {literal}"
        );
    }
}

#[test]
fn test_q_quote_different_delimiters() {
    let highlighter = SqlHighlighter::new();

    // Test q'(...)'
    let text1 = "SELECT q'(parentheses)' FROM dual";
    let styles1 = highlighter.generate_styles(text1);
    let q_start1 = text1.find("q'(").unwrap();
    let q_end1 = text1.find(")'").unwrap() + 2;
    assert!(
        styles1[q_start1..q_end1]
            .chars()
            .all(|c| c == STYLE_Q_QUOTE_STRING),
        "q'(...)' should be string style"
    );

    // Test q'{...}'
    let text2 = "SELECT q'{braces}' FROM dual";
    let styles2 = highlighter.generate_styles(text2);
    let q_start2 = text2.find("q'{").unwrap();
    let q_end2 = text2.find("}'").unwrap() + 2;
    assert!(
        styles2[q_start2..q_end2]
            .chars()
            .all(|c| c == STYLE_Q_QUOTE_STRING),
        "q'{{...}}' should be string style"
    );

    // Test q'<...>'
    let text3 = "SELECT q'<angle>' FROM dual";
    let styles3 = highlighter.generate_styles(text3);
    let q_start3 = text3.find("q'<").unwrap();
    let q_end3 = text3.find(">'").unwrap() + 2;
    assert!(
        styles3[q_start3..q_end3]
            .chars()
            .all(|c| c == STYLE_Q_QUOTE_STRING),
        "q'<...>' should be string style"
    );
}

#[test]
fn test_q_quote_with_embedded_quotes() {
    let highlighter = SqlHighlighter::new();
    // q-quoted strings can contain single quotes without escaping
    let text = "SELECT q'[It's a test]' FROM dual";
    let styles = highlighter.generate_styles(text);

    let q_start = text.find("q'[").unwrap();
    let q_end = text.find("]'").unwrap() + 2;
    assert!(
        styles[q_start..q_end]
            .chars()
            .all(|c| c == STYLE_Q_QUOTE_STRING),
        "q'[...]' with embedded quote should be string style"
    );
}

#[test]
fn test_multiline_q_quote_recovers_when_nested_same_delimiter_appears_inside() {
    let highlighter = SqlHighlighter::new();
    let text = "PROCEDURE upsert_row(\n\
p_id NUMBER,\n\
p_grp NUMBER,\n\
p_name VARCHAR2,\n\
p_note VARCHAR2,\n\
p_amount NUMBER,\n\
p_status VARCHAR2\n\
)\n\
IS\n\
v_count NUMBER;\n\
v_sql CLOB;\n\
BEGIN\n\
SELECT COUNT(*)\n\
INTO v_count\n\
FROM qt_splitter_boss\n\
WHERE id = p_id;\n\
\n\
IF v_count = 0 THEN\n\
INSERT INTO qt_splitter_boss\n\
(\n\
id, grp, name, note_text, created_at, amount, status_cd, payload\n\
)\n\
VALUES\n\
(\n\
p_id,\n\
p_grp,\n\
normalize_name(p_name),\n\
p_note,\n\
SYSDATE,\n\
p_amount,\n\
p_status,\n\
q'[inserted;payload/with tricky tokens]'\n\
);\n\
ELSE\n\
v_sql := q'[\n\
UPDATE qt_splitter_boss\n\
SET grp = :1,\n\
name = :2,\n\
note_text = :3,\n\
amount = :4,\n\
status_cd = :5,\n\
payload = q'[dynamic ; payload / still string]'\n\
WHERE id = :6\n\
]';\n\
\n\
EXECUTE IMMEDIATE v_sql\n\
USING p_grp, normalize_name(p_name), p_note, p_amount, p_status, p_id;\n\
END IF;\n\
\n\
BEGIN\n\
IF p_amount > 999 THEN\n\
log_row(p_id, 'amount>999; flagged');\n\
ELSE\n\
log_row(p_id, 'amount<=999; ok');\n\
END IF;\n\
EXCEPTION\n\
WHEN OTHERS THEN\n\
NULL;\n\
END;\n\
END upsert_row;";
    let styles = highlighter.generate_styles(text);

    let outer_start = text.find("q'[\nUPDATE").unwrap_or(0);
    let outer_end = text.rfind("\n]'").unwrap_or(text.len()) + "\n]'".len();
    assert!(
        styles[outer_start..outer_end]
            .chars()
            .all(|c| c == STYLE_Q_QUOTE_STRING),
        "outer q-quote should keep string style through nested inner q-quote"
    );

    let where_start = text.find("WHERE id = :6").unwrap_or(0);
    let where_end = where_start + "WHERE".len();
    assert!(
        styles[where_start..where_end]
            .chars()
            .all(|c| c == STYLE_Q_QUOTE_STRING),
        "WHERE inside outer q-quote must remain string style"
    );

    let execute_start = text.find("EXECUTE IMMEDIATE").unwrap_or(0);
    let execute_end = execute_start + "EXECUTE".len();
    assert!(
        styles[execute_start..execute_end]
            .chars()
            .all(|c| c == STYLE_KEYWORD),
        "EXECUTE after outer q-quote should return to keyword style"
    );

    for literal in ["'amount>999; flagged'", "'amount<=999; ok'"] {
        let start = text.find(literal).unwrap_or(0);
        let end = start + literal.len();
        assert!(
            styles[start..end].chars().all(|c| c == STYLE_STRING),
            "later quoted literal should remain string style: {literal}"
        );
    }
}

#[test]
fn test_hint_highlighting() {
    let highlighter = SqlHighlighter::new();
    let text = "SELECT /*+ FULL(t) */ * FROM table t";
    let styles = highlighter.generate_styles(text);

    // Find the hint position
    let hint_start = text.find("/*+").unwrap();
    let hint_end = text.find("*/").unwrap() + 2;

    assert!(
        styles[hint_start..hint_end]
            .chars()
            .all(|c| c == STYLE_HINT),
        "Hint /*+ ... */ should be styled as hint, got: {}",
        &styles[hint_start..hint_end]
    );
}

#[test]
fn test_hint_vs_regular_comment() {
    let highlighter = SqlHighlighter::new();

    // Regular comment should be comment style
    let text1 = "SELECT /* comment */ * FROM dual";
    let styles1 = highlighter.generate_styles(text1);
    let comment_start = text1.find("/*").unwrap();
    let comment_end = text1.find("*/").unwrap() + 2;
    assert!(
        styles1[comment_start..comment_end]
            .chars()
            .all(|c| c == STYLE_COMMENT),
        "Regular comment should be comment style"
    );

    // Hint should be hint style
    let text2 = "SELECT /*+ INDEX(t) */ * FROM dual";
    let styles2 = highlighter.generate_styles(text2);
    let hint_start = text2.find("/*+").unwrap();
    let hint_end = text2.find("*/").unwrap() + 2;
    assert!(
        styles2[hint_start..hint_end]
            .chars()
            .all(|c| c == STYLE_HINT),
        "Hint should be hint style"
    );
}

#[test]
fn test_complex_hint_highlighting() {
    let highlighter = SqlHighlighter::new();
    let text = "SELECT /*+ PARALLEL(t,4) FULL(t) INDEX(x idx_name) */ * FROM table t";
    let styles = highlighter.generate_styles(text);

    let hint_start = text.find("/*+").unwrap();
    let hint_end = text.find("*/").unwrap() + 2;
    assert!(
        styles[hint_start..hint_end]
            .chars()
            .all(|c| c == STYLE_HINT),
        "Complex hint should be fully styled as hint"
    );
}

#[test]
fn test_date_literal_highlighting() {
    let highlighter = SqlHighlighter::new();
    let text = "SELECT DATE '2024-01-01' FROM dual";
    let styles = highlighter.generate_styles(text);

    // Find DATE literal position
    let date_start = text.find("DATE").unwrap();
    let date_end = text.find("'2024-01-01'").unwrap() + "'2024-01-01'".len();

    assert!(
        styles[date_start..date_end]
            .chars()
            .all(|c| c == STYLE_DATETIME_LITERAL),
        "DATE literal should be styled as datetime literal, got: {}",
        &styles[date_start..date_end]
    );
}

#[test]
fn test_timestamp_literal_highlighting() {
    let highlighter = SqlHighlighter::new();
    let text = "SELECT TIMESTAMP '2024-01-01 12:30:00' FROM dual";
    let styles = highlighter.generate_styles(text);

    let ts_start = text.find("TIMESTAMP").unwrap();
    let ts_end = text.find("'2024-01-01 12:30:00'").unwrap() + "'2024-01-01 12:30:00'".len();

    assert!(
        styles[ts_start..ts_end]
            .chars()
            .all(|c| c == STYLE_DATETIME_LITERAL),
        "TIMESTAMP literal should be styled as datetime literal"
    );
}

#[test]
fn test_interval_literal_highlighting() {
    let highlighter = SqlHighlighter::new();
    let text = "SELECT INTERVAL '5' DAY FROM dual";
    let styles = highlighter.generate_styles(text);

    let int_start = text.find("INTERVAL").unwrap();
    let int_end = text.find("'5'").unwrap() + "'5'".len();

    assert!(
        styles[int_start..int_end]
            .chars()
            .all(|c| c == STYLE_DATETIME_LITERAL),
        "INTERVAL literal should be styled as datetime literal"
    );
}

#[test]
fn test_date_literal_with_newline_gap_highlighting() {
    let highlighter = SqlHighlighter::new();
    let text = "SELECT DATE
'2024-01-01' FROM dual";
    let styles = highlighter.generate_styles(text);

    let date_start = text.find("DATE").unwrap();
    let date_end = text.find("'2024-01-01'").unwrap() + "'2024-01-01'".len();

    assert!(
        styles[date_start..date_end]
            .chars()
            .all(|c| c == STYLE_DATETIME_LITERAL),
        "DATE literal with newline gap should be styled as datetime literal"
    );
}

#[test]
fn test_interval_literal_with_carriage_return_gap_highlighting() {
    let highlighter = SqlHighlighter::new();
    let text = "SELECT INTERVAL
'5' DAY FROM dual";
    let styles = highlighter.generate_styles(text);

    let int_start = text.find("INTERVAL").unwrap();
    let int_end = text.find("'5'").unwrap() + "'5'".len();

    assert!(
        styles[int_start..int_end]
            .chars()
            .all(|c| c == STYLE_DATETIME_LITERAL),
        "INTERVAL literal with carriage return gap should be styled as datetime literal"
    );
}

#[test]
fn test_date_keyword_without_literal() {
    let highlighter = SqlHighlighter::new();
    // DATE as column name or keyword should be keyword style
    let text = "SELECT hire_date FROM employees";
    let styles = highlighter.generate_styles(text);

    // "date" in "hire_date" should not be specially styled
    // The whole identifier should be default
    let hire_date_start = text.find("hire_date").unwrap();
    let hire_date_end = hire_date_start + "hire_date".len();
    // hire_date is not a keyword or function, should be default
    assert!(
        styles[hire_date_start..hire_date_end]
            .chars()
            .all(|c| c == STYLE_DEFAULT),
        "hire_date should be default style"
    );
}

#[test]
fn test_lowercase_date_literal() {
    let highlighter = SqlHighlighter::new();
    let text = "SELECT date '2024-01-01' FROM dual";
    let styles = highlighter.generate_styles(text);

    let date_start = text.find("date").unwrap();
    let date_end = text.find("'2024-01-01'").unwrap() + "'2024-01-01'".len();

    assert!(
        styles[date_start..date_end]
            .chars()
            .all(|c| c == STYLE_DATETIME_LITERAL),
        "Lowercase date literal should be styled as datetime literal"
    );
}

#[test]
fn test_quoted_identifier_does_not_trigger_keyword_or_comment() {
    let highlighter = SqlHighlighter::new();
    let text = r#"SELECT "FROM" AS "A--B" FROM dual"#;
    let styles = highlighter.generate_styles(text);

    let from_ident_start = text.find(r#""FROM""#).unwrap();
    let from_ident_end = from_ident_start + r#""FROM""#.len();
    assert!(
        styles[from_ident_start..from_ident_end]
            .chars()
            .all(|c| c == STYLE_QUOTED_IDENTIFIER),
        "quoted identifier should use quoted-identifier style"
    );

    let comment_like_start = text.find(r#""A--B""#).unwrap();
    let comment_like_end = comment_like_start + r#""A--B""#.len();
    assert!(
        styles[comment_like_start..comment_like_end]
            .chars()
            .all(|c| c == STYLE_QUOTED_IDENTIFIER),
        "double dash inside quoted identifier must not start comment"
    );
}

#[test]
fn test_quoted_identifier_with_escaped_quote_is_quoted_identifier_style() {
    let highlighter = SqlHighlighter::new();
    let text = r#"SELECT "A""B" FROM dual"#;
    let styles = highlighter.generate_styles(text);

    let quoted_start = text.find(r#""A""B""#).unwrap();
    let quoted_end = quoted_start + r#""A""B""#.len();
    assert!(
        styles[quoted_start..quoted_end]
            .chars()
            .all(|c| c == STYLE_QUOTED_IDENTIFIER),
        "escaped quote in quoted identifier should remain quoted-identifier style"
    );
}

#[test]
fn test_columns_and_relations_use_different_styles() {
    let mut highlighter = SqlHighlighter::new();
    highlighter.set_highlight_data(HighlightData {
        tables: vec!["EMP".to_string()],
        columns: vec!["ENAME".to_string()],
        ..HighlightData::new()
    });

    let text = "SELECT ENAME FROM EMP";
    let styles = highlighter.generate_styles(text);

    let col_start = text.find("ENAME").unwrap();
    let col_end = col_start + "ENAME".len();
    assert!(
        styles[col_start..col_end]
            .chars()
            .all(|c| c == STYLE_COLUMN),
        "columns should use column style"
    );

    let table_start = text.find("EMP").unwrap();
    let table_end = table_start + "EMP".len();
    assert!(
        styles[table_start..table_end]
            .chars()
            .all(|c| c == STYLE_IDENTIFIER),
        "relations should use identifier style"
    );
}

#[test]
fn test_plsql_if_after_routine_is_with_comment_newline_stays_keyword() {
    let highlighter = SqlHighlighter::new();
    let text = "CREATE OR REPLACE PROCEDURE p IS /* comment */
IF 1 = 1 THEN
NULL;
END;";
    let styles = highlighter.generate_styles(text);

    let if_start = text
        .find(
            "
IF",
        )
        .unwrap_or(0)
        + 1;
    assert!(
        styles[if_start..if_start + 2]
            .chars()
            .all(|c| c == STYLE_KEYWORD),
        "PL/SQL IF should remain keyword style after IS comment newline"
    );
}

#[test]
fn test_plsql_begin_after_routine_as_inline_comment_stays_keyword() {
    let highlighter = SqlHighlighter::new();
    let text = "CREATE OR REPLACE PROCEDURE p AS /* comment */ BEGIN
NULL;
END;";
    let styles = highlighter.generate_styles(text);

    let begin_start = text.find("BEGIN").unwrap_or(0);
    let begin_end = begin_start + "BEGIN".len();
    assert!(
        styles[begin_start..begin_end]
            .chars()
            .all(|c| c == STYLE_KEYWORD),
        "PL/SQL BEGIN should remain keyword style after AS inline comment"
    );
}

#[test]
fn test_plsql_declare_after_comment_banner_stays_keyword() {
    let highlighter = SqlHighlighter::new();
    let text = "-- banner
DECLARE
    v_count NUMBER;
BEGIN
    NULL;
END;";
    let styles = highlighter.generate_styles(text);

    let declare_start = text.find("DECLARE").unwrap_or(0);
    let declare_end = declare_start + "DECLARE".len();
    assert!(
        styles[declare_start..declare_end]
            .chars()
            .all(|c| c == STYLE_KEYWORD),
        "top-level DECLARE after comment banner should stay keyword"
    );
}

#[test]
fn test_plsql_if_after_comment_banner_inside_block_stays_keyword() {
    let highlighter = SqlHighlighter::new();
    let text = "BEGIN
    -- branch
    IF 1 = 1 THEN
        NULL;
    END IF;
END;";
    let styles = highlighter.generate_styles(text);

    let if_start = text.find("IF 1 = 1 THEN").unwrap_or(0);
    let if_end = if_start + "IF".len();
    assert!(
        styles[if_start..if_end].chars().all(|c| c == STYLE_KEYWORD),
        "PL/SQL IF after comment banner inside block should stay keyword"
    );
}

#[test]
fn test_plsql_loop_after_close_paren_stays_keyword() {
    let highlighter = SqlHighlighter::new();
    let text = "BEGIN
    WHILE (v_count < 10) LOOP
        v_count := v_count + 1;
    END LOOP;
END;";
    let styles = highlighter.generate_styles(text);

    let loop_start = text.find(") LOOP").unwrap_or(0) + 2;
    let loop_end = loop_start + "LOOP".len();
    assert!(
        styles[loop_start..loop_end]
            .chars()
            .all(|c| c == STYLE_KEYWORD),
        "LOOP after close paren should remain keyword-highlighted"
    );

    let end_loop_start = text.rfind("LOOP").unwrap_or(0);
    let end_loop_end = end_loop_start + "LOOP".len();
    assert!(
        styles[end_loop_start..end_loop_end]
            .chars()
            .all(|c| c == STYLE_KEYWORD),
        "END LOOP qualifier should remain keyword-highlighted"
    );
}

#[test]
fn test_package_spec_first_procedure_after_as_newline_is_keyword() {
    let highlighter = SqlHighlighter::new();
    let text =
        "CREATE OR REPLACE PACKAGE oqt_demo_pkg AS\nPROCEDURE proc_in_only (p_tag IN VARCHAR2);";
    let styles = highlighter.generate_styles(text);

    let procedure_start = text.find("PROCEDURE").unwrap_or(0);
    let procedure_end = procedure_start + "PROCEDURE".len();
    assert!(
        styles[procedure_start..procedure_end]
            .chars()
            .all(|c| c == STYLE_KEYWORD),
        "first package-spec PROCEDURE after AS newline should remain keyword style"
    );
}

#[test]
fn test_package_spec_first_procedure_after_as_comment_newline_is_keyword() {
    let highlighter = SqlHighlighter::new();
    let text = "CREATE OR REPLACE PACKAGE oqt_demo_pkg AS\n    -- (A) IN only\n    PROCEDURE proc_in_only (p_tag IN VARCHAR2);";
    let styles = highlighter.generate_styles(text);

    let procedure_start = text.find("PROCEDURE").unwrap_or(0);
    let procedure_end = procedure_start + "PROCEDURE".len();
    assert!(
        styles[procedure_start..procedure_end]
            .chars()
            .all(|c| c == STYLE_KEYWORD),
        "first package-spec PROCEDURE after AS comment newline should remain keyword style"
    );
}

#[test]
fn test_plsql_control_keyword_alias_after_as_with_comment_is_not_keyword() {
    let highlighter = SqlHighlighter::new();
    let text = "SELECT salary AS /* marker */ IF FROM dual";
    let styles = highlighter.generate_styles(text);

    let if_start = text.rfind("IF").unwrap_or(0);
    assert!(
        styles[if_start..if_start + 2]
            .chars()
            .all(|c| c == STYLE_DEFAULT),
        "AS alias IF with inline comment should not be keyword"
    );
}

#[test]
fn test_plsql_control_keyword_implicit_alias_in_select_list_is_not_keyword_lowercase() {
    let highlighter = SqlHighlighter::new();
    let text = "SELECT salary if, bonus end FROM dual";
    let styles = highlighter.generate_styles(text);

    let if_start = text.find(" if,").unwrap_or(0) + 1;
    assert!(
        styles[if_start..if_start + 2]
            .chars()
            .all(|c| c == STYLE_DEFAULT),
        "implicit alias if in select-list should not be keyword"
    );

    let end_start = text.find(" end ").unwrap_or(0) + 1;
    assert!(
        styles[end_start..end_start + 3]
            .chars()
            .all(|c| c == STYLE_DEFAULT),
        "implicit alias end in select-list should not be keyword"
    );
}

#[test]
fn test_plsql_control_keyword_implicit_alias_in_select_list_is_not_keyword() {
    let highlighter = SqlHighlighter::new();
    let text = "SELECT salary IF, bonus END FROM dual";
    let styles = highlighter.generate_styles(text);

    let if_start = text.find(" IF,").unwrap_or(0) + 1;
    assert!(
        styles[if_start..if_start + 2]
            .chars()
            .all(|c| c == STYLE_DEFAULT),
        "implicit alias IF in select-list should not be keyword"
    );

    let end_start = text.find(" END ").unwrap_or(0) + 1;
    assert!(
        styles[end_start..end_start + 3]
            .chars()
            .all(|c| c == STYLE_DEFAULT),
        "implicit alias END in select-list should not be keyword"
    );
}

#[test]
fn test_plsql_control_keyword_implicit_alias_before_close_paren_is_not_keyword() {
    let highlighter = SqlHighlighter::new();
    let text = "SELECT * FROM (SELECT salary IF FROM dual)";
    let styles = highlighter.generate_styles(text);

    let if_start = text.find(" IF FROM").unwrap_or(0) + 1;
    assert!(
        styles[if_start..if_start + 2]
            .chars()
            .all(|c| c == STYLE_DEFAULT),
        "implicit alias IF before close paren should not be keyword"
    );
}

#[test]
fn test_plsql_control_keyword_alias_if_before_case_then_is_not_keyword() {
    let highlighter = SqlHighlighter::new();

    let text = "SELECT amount IF, CASE WHEN flag = 1 THEN 1 ELSE 0 END score FROM sales";
    let styles = highlighter.generate_styles(text).into_bytes();

    let if_start = text.find(" IF,").unwrap_or(0) + 1;
    assert!(
        styles
            .get(if_start..if_start + 2)
            .is_some_and(|slice| slice.iter().all(|&c| c == STYLE_DEFAULT as u8)),
        "select-list alias IF before CASE/THEN should not be keyword"
    );

    let then_start = text.find("THEN").unwrap_or(0);
    assert!(
        styles
            .get(then_start..then_start + 4)
            .is_some_and(|slice| slice.iter().all(|&c| c == STYLE_KEYWORD as u8)),
        "CASE expression THEN should remain keyword"
    );
}

#[test]
fn test_case_keywords_after_plsql_range_operator_remain_highlighted() {
    let highlighter = SqlHighlighter::new();
    let text = r#"BEGIN
    FOR i IN 1..
        CASE
            WHEN v_x = 1 THEN
                5
            ELSE
                10
        END
    LOOP
        NULL;
    END LOOP;
END;"#;
    let styles = highlighter.generate_styles(text);

    for keyword in ["CASE", "WHEN", "ELSE", "END"] {
        let mut search_start = 0usize;
        while let Some(relative_start) = text[search_start..].find(keyword) {
            let start = search_start + relative_start;
            let end = start + keyword.len();
            assert!(
                styles[start..end].chars().all(|c| c == STYLE_KEYWORD),
                "{keyword} at byte offset {start} should remain keyword-highlighted after `..`"
            );
            search_start = end;
        }
    }
}

#[test]
fn test_case_end_before_plsql_range_operator_remains_keyword_highlighted() {
    let highlighter = SqlHighlighter::new();
    let text =
        "BEGIN\n    FOR i IN CASE WHEN v_x = 1 THEN 5 ELSE 0 END..10 LOOP\n        NULL;\n    END LOOP;\nEND;";
    let styles = highlighter.generate_styles(text);

    for keyword in ["CASE", "WHEN", "ELSE", "END"] {
        let mut search_start = 0usize;
        while let Some(relative_start) = text[search_start..].find(keyword) {
            let start = search_start + relative_start;
            let end = start + keyword.len();
            assert!(
                styles[start..end].chars().all(|c| c == STYLE_KEYWORD),
                "{keyword} at byte offset {start} should remain keyword-highlighted before `..`"
            );
            search_start = end;
        }
    }
}

#[test]
fn test_function_after_plsql_range_operator_remains_function_highlighted() {
    let highlighter = SqlHighlighter::new();
    let text = r#"BEGIN
    FOR i IN 1..
        TRIM(v_limit)
    LOOP
        NULL;
    END LOOP;
END;"#;
    let styles = highlighter.generate_styles(text);

    let trim_start = text.find("TRIM").unwrap_or(0);
    let trim_end = trim_start + "TRIM".len();
    assert!(
        styles[trim_start..trim_end]
            .chars()
            .all(|c| c == STYLE_FUNCTION),
        "function call after `..` should remain function-highlighted"
    );
}

#[test]
fn test_multiline_case_expression_keywords_remain_highlighted() {
    let highlighter = SqlHighlighter::new();
    let text = r#"SELECT
    e.empno,
    e.ename,
    CASE
        WHEN (
                 e.sal > 2000
                 AND (
                         e.comm IS NOT NULL
                         OR e.job IN (
                             'SALESMAN',
                             'MANAGER',
                             'ANALYST'
                         )
                     )
             ) THEN
            CASE
                WHEN e.deptno = 10 THEN 'A'
                WHEN e.deptno = 20 THEN
                    CASE
                        WHEN e.sal > 3000 THEN 'B1'
                        ELSE 'B2'
                    END
                ELSE 'C'
            END
        ELSE
            DECODE (
                SIGN (NVL (e.sal, 0) - 1500),
                -1, 'LOW',
                0, 'MID',
                1, COALESCE (e.job, 'UNKNOWN'),
                'ETC'
            )
    END AS complex_flag
FROM emp e"#;
    let styles = highlighter.generate_styles(text);

    for keyword in ["CASE", "ELSE", "END"] {
        let mut search_start = 0usize;
        while let Some(relative_start) = text[search_start..].find(keyword) {
            let start = search_start + relative_start;
            let end = start + keyword.len();
            assert!(
                styles[start..end].chars().all(|c| c == STYLE_KEYWORD),
                "{keyword} at byte offset {start} should remain keyword-highlighted"
            );
            search_start = end;
        }
    }
}

#[test]
fn test_begin_after_set_commands_and_comment_banner_remains_keyword() {
    let highlighter = SqlHighlighter::new();
    let text = "SET SERVEROUTPUT ON\n\
SET DEFINE OFF\n\
\n\
--------------------------------------------------------------------------------\n\
-- CLEANUP\n\
--------------------------------------------------------------------------------\n\
BEGIN\n\
    EXECUTE IMMEDIATE 'DROP TABLE qt_if_child PURGE';\n\
EXCEPTION\n\
    WHEN OTHERS THEN NULL;\n\
END;\n\
/";
    let styles = highlighter.generate_styles(text);

    let begin_start = text.find("BEGIN").unwrap_or(0);
    assert!(
        styles[begin_start..begin_start + 5]
            .chars()
            .all(|c| c == STYLE_KEYWORD),
        "statement-head BEGIN after SET/comment banner should stay keyword"
    );
}

#[test]
fn test_plsql_control_keyword_aliases_ignore_matching_metadata_names() {
    let mut highlighter = SqlHighlighter::new();
    highlighter.set_highlight_data(HighlightData {
        tables: vec!["IF".to_string()],
        columns: vec!["END".to_string()],
        ..HighlightData::new()
    });

    let text = "SELECT salary AS END FROM sales IF";
    let styles = highlighter.generate_styles(text);

    let end_start = text.find("END").unwrap_or(0);
    assert!(
        styles[end_start..end_start + 3]
            .chars()
            .all(|c| c == STYLE_DEFAULT),
        "alias END should not be styled as a loaded column"
    );

    let if_start = text.rfind("IF").unwrap_or(0);
    assert!(
        styles[if_start..if_start + 2]
            .chars()
            .all(|c| c == STYLE_DEFAULT),
        "alias IF should not be styled as a loaded table"
    );
}

#[test]
fn test_plsql_control_keyword_alias_in_plsql_select_list_is_not_keyword() {
    let highlighter = SqlHighlighter::new();
    let text = "BEGIN SELECT salary AS IF, bonus END INTO v_salary, v_bonus FROM dual; END;";
    let styles = highlighter.generate_styles(text);

    let if_start = text.find("AS IF").unwrap_or(0) + 3;
    assert!(
        styles[if_start..if_start + 2]
            .chars()
            .all(|c| c == STYLE_DEFAULT),
        "AS alias IF inside PL/SQL SELECT should not be keyword"
    );

    let end_start = text.find(" bonus END ").unwrap_or(0) + 7;
    assert!(
        styles[end_start..end_start + 3]
            .chars()
            .all(|c| c == STYLE_DEFAULT),
        "implicit alias END inside PL/SQL SELECT should not be keyword"
    );
}

#[test]
fn test_plsql_control_keywords_remain_keywords_outside_alias_context_with_metadata() {
    let mut highlighter = SqlHighlighter::new();
    highlighter.set_highlight_data(HighlightData {
        tables: vec!["IF".to_string()],
        columns: vec!["THEN".to_string(), "END".to_string()],
        ..HighlightData::new()
    });

    let text = "BEGIN IF cond THEN NULL; END IF; END;";
    let styles = highlighter.generate_styles(text);

    let if_start = text.find("IF cond").unwrap_or(0);
    assert!(
        styles[if_start..if_start + 2]
            .chars()
            .all(|c| c == STYLE_KEYWORD),
        "PL/SQL IF should remain keyword even when IF exists in metadata"
    );

    let then_start = text.find("THEN").unwrap_or(0);
    assert!(
        styles[then_start..then_start + 4]
            .chars()
            .all(|c| c == STYLE_KEYWORD),
        "PL/SQL THEN should remain keyword even when THEN exists in metadata"
    );

    let end_if_start = text.find("END IF").unwrap_or(0);
    assert!(
        styles[end_if_start..end_if_start + 3]
            .chars()
            .all(|c| c == STYLE_KEYWORD),
        "PL/SQL END should remain keyword in END IF"
    );
}

#[test]
fn test_multibyte_text_preserves_byte_length_styles() {
    let highlighter = SqlHighlighter::new();
    let text = "SELECT '한글🙂' AS 이름 FROM dual";
    let styles = highlighter.generate_styles(text);

    assert_eq!(
        styles.len(),
        text.len(),
        "style length must match byte length"
    );

    let string_start = text.find("'").unwrap();
    let string_end = text[string_start + 1..].find("'").unwrap() + string_start + 2;
    assert!(
        styles[string_start..string_end]
            .chars()
            .all(|c| c == STYLE_STRING),
        "multibyte string literal should be string style"
    );
}

// ── LexerState / generate_styles_with_state tests ─────────────────────

#[test]
fn test_generate_styles_with_state_normal_matches_generate_styles() {
    let highlighter = SqlHighlighter::new();
    let text = "SELECT 'hello' FROM dual -- comment";
    let plain = highlighter.generate_styles(text);
    let (stateful, exit) = highlighter.generate_styles_with_state(text, LexerState::Normal);
    assert_eq!(plain, stateful);
    assert_eq!(exit, LexerState::Normal);
}

#[test]
fn test_exit_state_unclosed_block_comment() {
    let highlighter = SqlHighlighter::new();
    let text = "SELECT /* unclosed comment";
    let (styles, exit) = highlighter.generate_styles_with_state(text, LexerState::Normal);
    assert_eq!(exit, LexerState::InBlockComment);
    let comment_start = text.find("/*").unwrap();
    assert!(
        styles[comment_start..]
            .chars()
            .all(|c| c == STYLE_BLOCK_COMMENT),
        "unclosed block comment should be BLOCK_COMMENT style"
    );
}

#[test]
fn test_exit_state_unclosed_hint() {
    let highlighter = SqlHighlighter::new();
    let text = "SELECT /*+ FULL(t)";
    let (_styles, exit) = highlighter.generate_styles_with_state(text, LexerState::Normal);
    assert_eq!(exit, LexerState::InHintComment);
}

#[test]
fn test_exit_state_unclosed_string() {
    let highlighter = SqlHighlighter::new();
    let text = "SELECT 'unclosed string";
    let (styles, exit) = highlighter.generate_styles_with_state(text, LexerState::Normal);
    assert_eq!(exit, LexerState::InSingleQuote);
    let str_start = text.find("'").unwrap();
    assert!(
        styles[str_start..].chars().all(|c| c == STYLE_STRING),
        "unclosed string should be STRING style"
    );
}

#[test]
fn test_exit_state_unclosed_q_quote() {
    let highlighter = SqlHighlighter::new();
    let text = "SELECT q'[unclosed q-string";
    let (_styles, exit) = highlighter.generate_styles_with_state(text, LexerState::Normal);
    assert!(
        matches!(
            exit,
            LexerState::InQQuote {
                closing: ']',
                depth: 1
            }
        ),
        "expected InQQuote with ']', got {:?}",
        exit
    );
}

#[test]
fn test_exit_state_unclosed_double_quote() {
    let highlighter = SqlHighlighter::new();
    let text = r#"SELECT "unclosed_ident"#;
    let (_styles, exit) = highlighter.generate_styles_with_state(text, LexerState::Normal);
    assert_eq!(exit, LexerState::InDoubleQuote);
}

#[test]
fn test_entry_state_in_block_comment_continues() {
    let highlighter = SqlHighlighter::new();
    // Simulate a window that starts in the middle of a block comment
    let text = "still commenting */ SELECT 1";
    let (styles, exit) = highlighter.generate_styles_with_state(text, LexerState::InBlockComment);
    assert_eq!(exit, LexerState::Normal);
    // "still commenting */" should be comment
    let comment_end = text.find("*/").unwrap() + 2;
    assert!(
        styles[..comment_end]
            .chars()
            .all(|c| c == STYLE_BLOCK_COMMENT),
        "continued block comment should keep BLOCK_COMMENT continuation style"
    );
    // "SELECT" after should be keyword
    let select_pos = text.find("SELECT").unwrap();
    assert!(
        styles[select_pos..select_pos + 6]
            .chars()
            .all(|c| c == STYLE_KEYWORD),
        "SELECT after comment close should be KEYWORD"
    );
}

#[test]
fn test_entry_state_in_hint_continues() {
    let highlighter = SqlHighlighter::new();
    let text = "FULL(t) */ SELECT 1";
    let (styles, exit) = highlighter.generate_styles_with_state(text, LexerState::InHintComment);
    assert_eq!(exit, LexerState::Normal);
    let hint_end = text.find("*/").unwrap() + 2;
    assert!(
        styles[..hint_end].chars().all(|c| c == STYLE_HINT),
        "continued hint should be HINT"
    );
}

#[test]
fn test_entry_state_in_single_quote_continues() {
    let highlighter = SqlHighlighter::new();
    let text = "still in string' FROM dual";
    let (styles, exit) = highlighter.generate_styles_with_state(text, LexerState::InSingleQuote);
    assert_eq!(exit, LexerState::Normal);
    let str_end = text.find("'").unwrap() + 1;
    assert!(
        styles[..str_end].chars().all(|c| c == STYLE_STRING),
        "continued string should be STRING"
    );
}

#[test]
fn test_entry_state_in_q_quote_continues() {
    let highlighter = SqlHighlighter::new();
    let text = "still in q-string]' FROM dual";
    let (styles, exit) = highlighter.generate_styles_with_state(
        text,
        LexerState::InQQuote {
            closing: ']',
            depth: 1,
        },
    );
    assert_eq!(exit, LexerState::Normal);
    let q_end = text.find("]'").unwrap() + 2;
    assert!(
        styles[..q_end].chars().all(|c| c == STYLE_Q_QUOTE_STRING),
        "continued q-quote should be STRING"
    );
}

#[test]
fn test_zero_depth_q_quote_entry_state_closes_safely() {
    let highlighter = SqlHighlighter::new();
    let text = "still in q-string]' FROM dual";
    let (styles, exit) = highlighter.generate_styles_with_state(
        text,
        LexerState::InQQuote {
            closing: ']',
            depth: 0,
        },
    );
    assert_eq!(exit, LexerState::Normal);
    let q_end = text.find("]'").unwrap() + 2;
    assert!(
        styles[..q_end].chars().all(|c| c == STYLE_Q_QUOTE_STRING),
        "zero-depth Q-quote state should be treated as the outermost level"
    );
}

#[test]
fn test_entry_state_in_double_quote_continues() {
    let highlighter = SqlHighlighter::new();
    let text = r#"continued_ident" FROM dual"#;
    let (styles, exit) = highlighter.generate_styles_with_state(text, LexerState::InDoubleQuote);
    assert_eq!(exit, LexerState::Normal);
    let ident_end = text.find('"').unwrap() + 1;
    assert!(
        styles[..ident_end]
            .chars()
            .all(|c| c == STYLE_QUOTED_IDENTIFIER),
        "continued quoted identifier should be IDENTIFIER"
    );
}

#[test]
fn test_entry_state_block_comment_never_closes() {
    let highlighter = SqlHighlighter::new();
    let text = "all of this is inside the comment";
    let (styles, exit) = highlighter.generate_styles_with_state(text, LexerState::InBlockComment);
    assert_eq!(exit, LexerState::InBlockComment);
    assert!(
        styles.chars().all(|c| c == STYLE_BLOCK_COMMENT),
        "entire text should be BLOCK_COMMENT when starting InBlockComment and no close"
    );
}

#[test]
fn test_cross_window_block_comment_round_trip() {
    let highlighter = SqlHighlighter::new();
    // Window 1: opens comment
    let window1 = "SELECT 1; /* long comment starts here";
    let (_s1, state1) = highlighter.generate_styles_with_state(window1, LexerState::Normal);
    assert_eq!(state1, LexerState::InBlockComment);

    // Window 2: continues comment
    let window2 = "...still commenting...\nmore comment text";
    let (_s2, state2) = highlighter.generate_styles_with_state(window2, state1);
    assert_eq!(state2, LexerState::InBlockComment);

    // Window 3: closes comment
    let window3 = "end of comment */ SELECT 2 FROM dual";
    let (s3, state3) = highlighter.generate_styles_with_state(window3, state2);
    assert_eq!(state3, LexerState::Normal);
    let select_pos = window3.find("SELECT").unwrap();
    assert!(
        s3[select_pos..select_pos + 6]
            .chars()
            .all(|c| c == STYLE_KEYWORD),
        "SELECT after comment close should be KEYWORD"
    );
}

#[test]
fn test_entry_state_from_continuation_style_maps_segmented_styles() {
    let highlighter = SqlHighlighter::new();

    assert_eq!(
        highlighter.entry_state_from_continuation_style(STYLE_BLOCK_COMMENT),
        LexerState::InBlockComment
    );
    assert_eq!(
        highlighter.entry_state_from_continuation_style(STYLE_Q_QUOTE_STRING),
        LexerState::Normal
    );
    assert_eq!(
        highlighter.entry_state_from_continuation_style(STYLE_QUOTED_IDENTIFIER),
        LexerState::InDoubleQuote
    );
}

#[test]
fn test_q_quote_style_does_not_force_single_quote_entry_state() {
    let highlighter = SqlHighlighter::new();

    assert_eq!(
        highlighter.entry_state_from_continuation_style(STYLE_Q_QUOTE_STRING),
        LexerState::Normal
    );
}

#[test]
fn test_probe_entry_state_skips_probe_for_stale_default_inside_comment() {
    let highlighter = SqlHighlighter::new();
    let text = "SELECT 1; /* open comment\ncontinued comment\nSELECT 2";
    let style_text = std::iter::repeat_n(STYLE_DEFAULT, text.len()).collect::<String>();

    let pos = text.find("continued").unwrap_or(0);
    let entry = highlighter.probe_entry_state_for_text(text, &style_text, pos);
    assert_eq!(entry, LexerState::Normal);
}

#[test]
fn test_probe_entry_state_clamps_mid_byte_cursor_inside_comment() {
    let highlighter = SqlHighlighter::new();
    let text = "SELECT 1; /* 한글\n계속 */";
    let style_text = std::iter::repeat_n(STYLE_DEFAULT, text.len()).collect::<String>();
    let comment_pos = text.find("한").unwrap_or(0);
    let mid_byte_pos = comment_pos + 1;

    assert!(!text.is_char_boundary(mid_byte_pos));

    let entry = highlighter.probe_entry_state_for_text(text, &style_text, mid_byte_pos);
    assert_eq!(entry, LexerState::Normal);
}

#[test]
fn test_probe_entry_state_skips_probe_for_stale_default_q_quote_window() {
    let highlighter = SqlHighlighter::new();
    let text = "SELECT q'[first line\nsecond line with ' quote\nthird line]'\nFROM dual";
    let style_text = std::iter::repeat_n(STYLE_DEFAULT, text.len()).collect::<String>();
    let pos = text.find("second line").unwrap();

    let entry = highlighter.probe_entry_state_for_text(text, &style_text, pos);
    assert_eq!(entry, LexerState::Normal);
}

#[test]
fn test_probe_entry_state_skips_probe_for_stale_default_single_quote_window() {
    let highlighter = SqlHighlighter::new();
    let text = "SELECT 'first line\nsecond line with '' quote\nthird line'\nFROM dual";
    let style_text = std::iter::repeat_n(STYLE_DEFAULT, text.len()).collect::<String>();
    let pos = text.find("second line").unwrap();

    let entry = highlighter.probe_entry_state_for_text(text, &style_text, pos);
    assert_eq!(entry, LexerState::Normal);
}

#[test]
fn test_probe_entry_state_returns_normal_for_non_multiline_prev_style() {
    let highlighter = SqlHighlighter::new();
    let text = "SELECT abc FROM dual";
    let style_text = highlighter.generate_styles(text);
    let pos = text.find("FROM").unwrap();

    let entry = highlighter.probe_entry_state_for_text(text, &style_text, pos);
    assert_eq!(entry, LexerState::Normal);
}

#[test]
fn test_clamp_buffer_boundary_keeps_ascii_boundary() {
    let text = "SELECT 1";
    let idx = "SELECT".len();
    assert_eq!(clamp_to_utf8_boundary(text, idx), idx);
}

#[test]
fn test_clamp_buffer_boundary_clamps_mid_byte_utf8() {
    let text = "SELECT 한글";
    let char_start = text.find('한').unwrap_or(0);
    let mid_byte = char_start + 1;
    assert!(!text.is_char_boundary(mid_byte));

    assert_eq!(clamp_to_utf8_boundary(text, mid_byte), char_start);
}

#[test]
fn test_probe_entry_state_skips_probe_for_stale_default_long_block_comment() {
    let highlighter = SqlHighlighter::new();
    let filler = "a".repeat(STATE_PROBE_DISTANCE + 128);
    let text = format!("/*{}{}", filler, "\ncontinued comment");
    let style_text = std::iter::repeat_n(STYLE_DEFAULT, text.len()).collect::<String>();
    let pos = text.find("continued").unwrap();

    let entry = highlighter.probe_entry_state_for_text(&text, &style_text, pos);
    assert_eq!(entry, LexerState::Normal);
}

#[test]
fn test_probe_entry_state_skips_probe_for_stale_default_long_q_quote() {
    let highlighter = SqlHighlighter::new();
    let filler = "가".repeat((STATE_PROBE_DISTANCE / 3) + 64);
    let text = format!("SELECT uq'가{}계속가' FROM dual", filler);
    let style_text = std::iter::repeat_n(STYLE_DEFAULT, text.len()).collect::<String>();
    let pos = text.find("계속").unwrap();

    let entry = highlighter.probe_entry_state_for_text(&text, &style_text, pos);
    assert_eq!(entry, LexerState::Normal);
}

#[test]
fn test_incremental_highlight_inherits_comment_entry_state() {
    let highlighter = SqlHighlighter::new();
    let text = "/* open comment\nupdated text still comment */\nSELECT 1";
    let previous_styles = highlighter.generate_styles(text);
    let start = text.find("updated").unwrap_or(0);

    let result = highlighter.generate_incremental_styles(IncrementalHighlightRequest {
        start,
        tail_text: text[start..].to_string(),
        previous_tail_styles: previous_styles[start..].to_string(),
        entry_state: LexerState::InBlockComment,
    });

    assert!(result.is_some());
    let updated = result.unwrap_or(IncrementalHighlightResult {
        start: 0,
        end: 0,
        styles: String::new(),
    });
    assert!(updated.end >= updated.start);
    if !updated.styles.is_empty() {
        let comment_end = text[start..]
            .find("*/")
            .map(|idx| idx + 2)
            .unwrap_or(updated.styles.len());
        assert!(
            updated.styles[..comment_end]
                .chars()
                .all(|c| c == STYLE_BLOCK_COMMENT),
            "continued block comment bytes should keep BLOCK_COMMENT style"
        );
    }
}

#[test]
fn test_closed_multiline_block_comment_keeps_continuation_style() {
    let highlighter = SqlHighlighter::new();
    let text = "/* header\nasdf\nfooter */\nSELECT 1";
    let styles = highlighter.generate_styles(text);
    let comment_end = text.find("*/").unwrap() + 2;

    assert!(
        styles[..comment_end]
            .chars()
            .all(|c| c == STYLE_BLOCK_COMMENT),
        "closed multiline block comment should keep BLOCK_COMMENT style for incremental continuation"
    );
}

#[test]
fn test_generate_styles_with_block_comment_entry_matches_full_highlighting() {
    let highlighter = SqlHighlighter::new();
    let text = "/* open comment\ncontinued line */\nSELECT 1";
    let start = text.find("continued").unwrap_or(0);
    let expected = highlighter.generate_styles(text);

    let (window_styles, _) =
        highlighter.generate_styles_for_window(&text[start..], LexerState::InBlockComment);

    assert_eq!(window_styles, expected[start..]);
}

#[test]
fn test_incremental_highlight_stops_when_style_tail_matches() {
    let highlighter = SqlHighlighter::new();
    let original = "SELECT alpha FROM dual";
    let updated_text = "SELECT alphax FROM dual";

    let previous_styles = highlighter.generate_styles(original);
    let result = highlighter.generate_incremental_styles(IncrementalHighlightRequest {
        start: "SELECT ".len(),
        tail_text: updated_text["SELECT ".len()..].to_string(),
        previous_tail_styles: previous_styles["SELECT ".len()..].to_string(),
        entry_state: LexerState::Normal,
    });

    assert!(result.is_some());
    let updated = result.unwrap_or(IncrementalHighlightResult {
        start: 0,
        end: 0,
        styles: String::new(),
    });
    assert!(updated.end <= updated_text.len());
    assert_eq!(
        updated.styles.len(),
        updated.end.saturating_sub(updated.start)
    );
}

#[test]
fn test_mysql_hash_comment_highlighting_marks_comment_tail() {
    let mut highlighter = SqlHighlighter::new();
    highlighter.set_db_type(crate::db::connection::DatabaseType::MySQL);
    let text = "SELECT 1 # line comment torture: ; ; ; DELIMITER $$ should not matter here.....";
    let styles = highlighter.generate_styles(text);

    let comment_start = text.find('#').unwrap_or(0);
    assert!(
        styles[comment_start..]
            .chars()
            .all(|style| style == STYLE_COMMENT),
        "MySQL hash comment tail should stay comment-highlighted"
    );
}

#[test]
fn test_mysql_hash_comment_highlighting_marks_identifier_adjacent_comment_tail() {
    let mut highlighter = SqlHighlighter::new();
    highlighter.set_db_type(crate::db::connection::DatabaseType::MySQL);
    let text = "SELECT col# trailing";
    let styles = highlighter.generate_styles(text);

    let comment_start = text.find('#').unwrap_or(0);
    assert!(
        styles[comment_start..]
            .chars()
            .all(|style| style == STYLE_COMMENT),
        "identifier-adjacent MySQL # comment tail should stay comment-highlighted"
    );
}

#[test]
fn test_mysql_hash_after_backslash_escaped_quote_stays_string_highlighted() {
    let mut highlighter = SqlHighlighter::new();
    highlighter.set_db_type(crate::db::connection::DatabaseType::MySQL);
    let text = "SELECT 'abc\\'#still string' AS value";
    let styles = highlighter.generate_styles(text);

    let hash_idx = text.find('#').unwrap_or(0);
    assert_eq!(
        styles.as_bytes().get(hash_idx).copied(),
        Some(STYLE_STRING as u8),
        "hash inside a backslash-escaped MySQL string must stay string-highlighted"
    );
}

#[test]
fn test_mysql_backtick_identifier_highlighting_uses_quoted_identifier_style() {
    let mut highlighter = SqlHighlighter::new();
    highlighter.set_db_type(crate::db::connection::DatabaseType::MySQL);
    let text = "SELECT `order`, `complex``name` FROM `sales`";
    let styles = highlighter.generate_styles(text);

    for token in ["`order`", "`complex``name`", "`sales`"] {
        let start = text.find(token).unwrap_or(0);
        let end = start + token.len();
        assert!(
            styles[start..end]
                .chars()
                .all(|style| style == STYLE_QUOTED_IDENTIFIER),
            "{token} should use quoted identifier highlighting"
        );
    }
}

#[test]
fn test_mysql_family_bracket_identifier_highlighting_uses_quoted_identifier_style() {
    for db_type in [
        crate::db::connection::DatabaseType::MySQL,
        crate::db::connection::DatabaseType::MariaDB,
    ] {
        let mut highlighter = SqlHighlighter::new();
        highlighter.set_db_type(db_type);
        let text = "SELECT [select], [bracket]]name], [semi;--/*comment*/] FROM [mode table]";
        let styles = highlighter.generate_styles(text);

        for token in [
            "[select]",
            "[bracket]]name]",
            "[semi;--/*comment*/]",
            "[mode table]",
        ] {
            let start = text.find(token).unwrap_or(0);
            let end = start + token.len();
            assert!(
                styles[start..end]
                    .chars()
                    .all(|style| style == STYLE_QUOTED_IDENTIFIER),
                "{db_type:?} {token} should use quoted identifier highlighting"
            );
        }
    }
}

#[test]
fn test_mysql_family_multiline_bracket_identifier_carries_lexer_state() {
    let mut highlighter = SqlHighlighter::new();
    highlighter.set_db_type(crate::db::connection::DatabaseType::MariaDB);

    let (first_styles, state) =
        highlighter.generate_styles_for_window("SELECT [line\n", LexerState::Normal);
    assert_eq!(state, LexerState::InBracketQuote);
    assert!(first_styles["SELECT ".len()..]
        .chars()
        .all(|style| style == STYLE_QUOTED_IDENTIFIER));

    let (second_styles, state) = highlighter.generate_styles_for_window("break] FROM t", state);
    assert_eq!(state, LexerState::Normal);
    assert!(second_styles[.."break]".len()]
        .chars()
        .all(|style| style == STYLE_QUOTED_IDENTIFIER));
}

#[test]
fn test_mysql_double_dash_without_whitespace_is_not_comment_highlighted() {
    let mut highlighter = SqlHighlighter::new();
    highlighter.set_db_type(crate::db::connection::DatabaseType::MySQL);
    let text = "SELECT 5--2 AS diff";
    let styles = highlighter.generate_styles(text);

    let expr_start = text.find("5--2").unwrap_or(0);
    let expr_end = expr_start + "5--2".len();
    assert!(
        styles[expr_start..expr_end]
            .chars()
            .all(|style| style != STYLE_COMMENT),
        "MySQL `--<non-space>` arithmetic must not be comment-highlighted"
    );
}

#[test]
fn test_mysql_highlighting_covers_mariadb_keyword_gaps_and_end_delimiter_suffix() {
    let text = "END$$
GET DIAGNOSTICS CONDITION 1 v_state = RETURNED_SQLSTATE, v_errno = MYSQL_ERRNO, v_msg = MESSAGE_TEXT;
DEALLOCATE PREPARE stmt;
CREATE TABLE t (
    c1 INT GENERATED ALWAYS AS (col_a + 1) STORED,
    c2 INT AS (col_b + 1) VIRTUAL
);
SELECT 1
FROM t
WINDOW w AS (ORDER BY id ROWS BETWEEN UNBOUNDED PRECEDING AND CURRENT ROW);
ALTER TABLE t ENGINE = InnoDB;
RELEASE SAVEPOINT sp;
SHOW ERRORS;
SHOW WARNINGS;";

    let mut highlighter = SqlHighlighter::new();
    highlighter.set_db_type(crate::db::connection::DatabaseType::MySQL);
    let styles = highlighter.generate_styles(text);

    for token in [
        "END",
        "DIAGNOSTICS",
        "DEALLOCATE",
        "RETURNED_SQLSTATE",
        "MYSQL_ERRNO",
        "MESSAGE_TEXT",
        "ERRORS",
        "WARNINGS",
        "WINDOW",
        "GENERATED",
        "ALWAYS",
        "STORED",
        "VIRTUAL",
        "UNBOUNDED",
        "PRECEDING",
        "CURRENT",
        "ENGINE",
        "RELEASE",
    ] {
        assert_token_has_style(text, &styles, token, STYLE_KEYWORD);
    }

    let delimiter_start = text.find("$$").expect("delimiter suffix should exist");
    let delimiter_end = delimiter_start + 2;
    assert!(
        styles[delimiter_start..delimiter_end]
            .chars()
            .all(|style| style != STYLE_KEYWORD),
        "delimiter suffix after END should not inherit keyword highlighting"
    );

    let engine_name_start = text.find("InnoDB").expect("engine name should exist");
    let engine_name_end = engine_name_start + "InnoDB".len();
    assert!(
        styles[engine_name_start..engine_name_end]
            .chars()
            .all(|style| style == STYLE_DEFAULT),
        "storage engine names should remain non-keyword text"
    );
}

#[test]
fn test_mysql_admin_option_keywords_highlight_as_keywords() {
    let text = "\
CREATE TABLESPACE ts ADD DATAFILE 'ts.ibd' AUTOEXTEND_SIZE 64M ENGINE_ATTRIBUTE '{\"k\":\"v\"}';
CREATE LOGFILE GROUP lg ADD UNDOFILE 'undo.dat' INITIAL_SIZE 64M REDO_BUFFER_SIZE 8M;
CREATE RESOURCE GROUP rg TYPE USER VCPU 0-3 THREAD_PRIORITY 5;
START GROUP_REPLICATION USER repl DEFAULT_AUTH mysql_native_password PASSWORD 'secret';
CHANGE REPLICATION SOURCE TO SOURCE_HOST = 'db', SOURCE_PASSWORD = 'secret', SOURCE_SSL_VERIFY_SERVER_CERT = 1;
START REPLICA UNTIL SQL_BEFORE_GTIDS = 'uuid:1-10', RELAY_LOG_FILE = 'relay.000001';
CHANGE REPLICATION FILTER REPLICATE_DO_DB = (app), REPLICATE_WILD_IGNORE_TABLE = ('tmp.%');
GRANT APPLICATION_PASSWORD_ADMIN, GROUP_REPLICATION_ADMIN ON *.* TO alice;
CREATE USER alice FAILED_LOGIN_ATTEMPTS 3 PASSWORD_LOCK_TIME 2 WITH MAX_QUERIES_PER_HOUR 10;
ALTER INSTANCE RELOAD TLS FOR CHANNEL MYSQL_MAIN;
SET PERSIST_ONLY sql_mode = 'STRICT_ALL_TABLES';
CHECKSUM TABLE emp QUICK;
INSTALL PLUGIN auth_socket SONAME 'auth_socket.so';
UNINSTALL COMPONENT 'file://component_validate_password';
CREATE DEFINER = root TABLE t (id INT) ENCRYPTION = 'Y' INSERT_METHOD = LAST KEY_BLOCK_SIZE = 8;";

    let mut highlighter = SqlHighlighter::new();
    highlighter.set_db_type(crate::db::connection::DatabaseType::MySQL);
    let styles = highlighter.generate_styles(text);

    for token in [
        "AUTOEXTEND_SIZE",
        "CHANNEL",
        "CHECKSUM",
        "DEFINER",
        "ENCRYPTION",
        "ENGINE_ATTRIBUTE",
        "UNDOFILE",
        "INITIAL_SIZE",
        "INSERT_METHOD",
        "INSTALL",
        "KEY_BLOCK_SIZE",
        "REDO_BUFFER_SIZE",
        "VCPU",
        "THREAD_PRIORITY",
        "GROUP_REPLICATION",
        "DEFAULT_AUTH",
        "REPLICATION",
        "REPLICA",
        "SOURCE_HOST",
        "SOURCE_PASSWORD",
        "SOURCE_SSL_VERIFY_SERVER_CERT",
        "SQL_BEFORE_GTIDS",
        "RELAY_LOG_FILE",
        "REPLICATE_DO_DB",
        "REPLICATE_WILD_IGNORE_TABLE",
        "APPLICATION_PASSWORD_ADMIN",
        "GROUP_REPLICATION_ADMIN",
        "FAILED_LOGIN_ATTEMPTS",
        "PASSWORD_LOCK_TIME",
        "MAX_QUERIES_PER_HOUR",
        "TLS",
        "MYSQL_MAIN",
        "PERSIST_ONLY",
        "UNINSTALL",
    ] {
        assert_token_has_style(text, &styles, token, STYLE_KEYWORD);
    }
}

#[test]
fn test_mysql_highlighting_marks_control_and_cast_keywords_from_mariadb_scripts() {
    let text = "DECLARE CONTINUE HANDLER FOR NOT FOUND SET done = 1;
WHILE v_i <= 5 DO
    SET signed_total = CAST(v_i AS SIGNED);
END WHILE;
CLOSE cur_task;
IF NOT (OLD.status_code <=> NEW.status_code) THEN
    SET done = 0;
END IF;";

    let mut highlighter = SqlHighlighter::new();
    highlighter.set_db_type(crate::db::connection::DatabaseType::MySQL);
    let styles = highlighter.generate_styles(text);

    for token in ["FOUND", "DO", "SIGNED", "CLOSE", "OLD"] {
        assert_token_has_style(text, &styles, token, STYLE_KEYWORD);
    }
}

#[test]
fn test_mysql_drop_table_if_exists_highlights_if_as_keyword() {
    let text = "DROP TABLE IF EXISTS boss_monthly_stats;";

    let mut highlighter = SqlHighlighter::new();
    highlighter.set_db_type(crate::db::connection::DatabaseType::MySQL);
    let styles = highlighter.generate_styles(text);

    for token in ["DROP", "TABLE", "IF", "EXISTS"] {
        assert_token_has_style(text, &styles, token, STYLE_KEYWORD);
    }
}

#[test]
fn test_mysql_highlighting_handles_mariadb_final_boss_regression() {
    let text = load_mariadb_highlight_test_file("test1.txt");
    assert!(!text.is_empty(), "test_mysql/test1.txt should not be empty");

    let mut highlighter = SqlHighlighter::new();
    highlighter.set_db_type(crate::db::connection::DatabaseType::MySQL);
    let styles = highlighter.generate_styles(&text);

    assert_eq!(
        styles.len(),
        text.len(),
        "highlight output must stay byte-aligned with the source text"
    );

    let comment_line = "# line comment torture: ; ; ; DELIMITER $$ should not matter here";
    let comment_start = text.find(comment_line).unwrap_or(0);
    let comment_end = comment_start + comment_line.len();
    assert!(
        styles[comment_start..comment_end]
            .chars()
            .all(|style| style == STYLE_COMMENT),
        "MariaDB hash comment line should remain fully comment-highlighted"
    );

    let quoted_identifier = "`group`";
    let quoted_start = text.find(quoted_identifier).unwrap_or(0);
    let quoted_end = quoted_start + quoted_identifier.len();
    assert!(
        styles[quoted_start..quoted_end]
            .chars()
            .all(|style| style == STYLE_QUOTED_IDENTIFIER),
        "backtick identifier should use quoted-identifier highlighting"
    );

    let string_literal = r#"'contains ; semicolon, ''quote'', backslash \\\\, text -- not comment, text /* not comment */, token DELIMITER $$, emoji 😊'"#;
    let string_start = text.find(string_literal).unwrap_or(0);
    let string_end = string_start + string_literal.len();
    assert!(
        styles[string_start..string_end]
            .chars()
            .all(|style| style == STYLE_STRING),
        "delimiters and comment markers inside a MariaDB string literal must stay string-highlighted"
    );

    assert!(
        text.contains("RESIGNAL"),
        "test_mysql/test1.txt should contain RESIGNAL"
    );
    let resignal_start = text.find("RESIGNAL").unwrap_or(0);
    let resignal_end = resignal_start + "RESIGNAL".len();
    assert!(
        styles[resignal_start..resignal_end]
            .chars()
            .all(|style| style == STYLE_KEYWORD),
        "MariaDB RESIGNAL should be highlighted as a keyword"
    );
}

#[test]
fn test_user_function_is_highlighted_like_table() {
    let mut highlighter = SqlHighlighter::new();
    highlighter.set_highlight_data(HighlightData {
        functions: vec!["MY_FUNC".to_string()],
        ..HighlightData::new()
    });

    let text = "SELECT MY_FUNC(1) FROM dual";
    let styles = highlighter.generate_styles(text);
    assert_token_has_style(text, &styles, "MY_FUNC", STYLE_IDENTIFIER);
}

#[test]
fn test_user_procedure_is_highlighted_like_table() {
    let mut highlighter = SqlHighlighter::new();
    highlighter.set_highlight_data(HighlightData {
        procedures: vec!["MY_PROC".to_string()],
        ..HighlightData::new()
    });

    let text = "BEGIN MY_PROC(1); END;";
    let styles = highlighter.generate_styles(text);
    assert_token_has_style(text, &styles, "MY_PROC", STYLE_IDENTIFIER);
}

#[test]
fn test_oracle_schema_qualified_function_is_highlighted() {
    let mut highlighter = SqlHighlighter::new();
    highlighter.set_highlight_data(HighlightData {
        functions: vec!["MY_FUNC".to_string()],
        schemas: vec!["HR".to_string()],
        ..HighlightData::new()
    });

    let text = "SELECT HR.MY_FUNC(1) FROM dual";
    let styles = highlighter.generate_styles(text);
    assert_token_has_style(text, &styles, "HR", STYLE_IDENTIFIER);
    assert_token_has_style(text, &styles, "MY_FUNC", STYLE_IDENTIFIER);
}

#[test]
fn test_oracle_package_qualified_procedure_is_highlighted() {
    let mut highlighter = SqlHighlighter::new();
    highlighter.set_highlight_data(HighlightData {
        procedures: vec!["MY_PROC".to_string()],
        packages: vec!["MY_PKG".to_string()],
        ..HighlightData::new()
    });

    let text = "BEGIN MY_PKG.MY_PROC(1); END;";
    let styles = highlighter.generate_styles(text);
    assert_token_has_style(text, &styles, "MY_PKG", STYLE_IDENTIFIER);
    assert_token_has_style(text, &styles, "MY_PROC", STYLE_IDENTIFIER);
}

#[test]
fn test_oracle_schema_package_function_chain_is_highlighted() {
    let mut highlighter = SqlHighlighter::new();
    highlighter.set_highlight_data(HighlightData {
        functions: vec!["MY_FUNC".to_string()],
        packages: vec!["MY_PKG".to_string()],
        schemas: vec!["HR".to_string()],
        ..HighlightData::new()
    });

    let text = "SELECT HR.MY_PKG.MY_FUNC(1) FROM dual";
    let styles = highlighter.generate_styles(text);
    for token in ["HR", "MY_PKG", "MY_FUNC"] {
        assert_token_has_style(text, &styles, token, STYLE_IDENTIFIER);
    }
}

#[test]
fn test_oracle_schema_package_procedure_chain_is_highlighted() {
    let mut highlighter = SqlHighlighter::new();
    highlighter.set_highlight_data(HighlightData {
        procedures: vec!["MY_PROC".to_string()],
        packages: vec!["MY_PKG".to_string()],
        schemas: vec!["HR".to_string()],
        ..HighlightData::new()
    });

    let text = "BEGIN HR.MY_PKG.MY_PROC(1); END;";
    let styles = highlighter.generate_styles(text);
    for token in ["HR", "MY_PKG", "MY_PROC"] {
        assert_token_has_style(text, &styles, token, STYLE_IDENTIFIER);
    }
}

#[test]
fn test_mysql_db_qualified_function_is_highlighted() {
    let mut highlighter = SqlHighlighter::new();
    highlighter.set_db_type(crate::db::connection::DatabaseType::MySQL);
    highlighter.set_highlight_data(HighlightData {
        functions: vec!["MY_FUNC".to_string()],
        schemas: vec!["MY_DB".to_string()],
        ..HighlightData::new()
    });

    let text = "SELECT MY_DB.MY_FUNC(1)";
    let styles = highlighter.generate_styles(text);
    assert_token_has_style(text, &styles, "MY_DB", STYLE_IDENTIFIER);
    assert_token_has_style(text, &styles, "MY_FUNC", STYLE_IDENTIFIER);
}

#[test]
fn test_mariadb_db_qualified_procedure_is_highlighted() {
    let mut highlighter = SqlHighlighter::new();
    highlighter.set_db_type(crate::db::connection::DatabaseType::MariaDB);
    highlighter.set_highlight_data(HighlightData {
        procedures: vec!["MY_PROC".to_string()],
        schemas: vec!["MY_DB".to_string()],
        ..HighlightData::new()
    });

    let text = "CALL MY_DB.MY_PROC(1)";
    let styles = highlighter.generate_styles(text);
    assert_token_has_style(text, &styles, "MY_DB", STYLE_IDENTIFIER);
    assert_token_has_style(text, &styles, "MY_PROC", STYLE_IDENTIFIER);
}

#[test]
fn test_user_function_lowercase_metadata_matches_uppercase_text() {
    let mut highlighter = SqlHighlighter::new();
    highlighter.set_highlight_data(HighlightData {
        functions: vec!["my_func".to_string()],
        ..HighlightData::new()
    });

    let text = "SELECT MY_FUNC(1) FROM dual";
    let styles = highlighter.generate_styles(text);
    assert_token_has_style(text, &styles, "MY_FUNC", STYLE_IDENTIFIER);
}

#[test]
fn test_builtin_function_takes_precedence_over_user_function_with_same_name() {
    let mut highlighter = SqlHighlighter::new();
    highlighter.set_highlight_data(HighlightData {
        functions: vec!["UPPER".to_string()],
        ..HighlightData::new()
    });

    let text = "SELECT UPPER(name) FROM dual";
    let styles = highlighter.generate_styles(text);
    assert_token_has_style(text, &styles, "UPPER", STYLE_FUNCTION);
}

#[test]
fn test_oracle_user_package_does_not_disturb_builtin_function_call() {
    let mut highlighter = SqlHighlighter::new();
    highlighter.set_highlight_data(HighlightData {
        packages: vec!["MY_PKG".to_string()],
        ..HighlightData::new()
    });

    let text = "SELECT UPPER('a') FROM dual";
    let styles = highlighter.generate_styles(text);
    assert_token_has_style(text, &styles, "UPPER", STYLE_FUNCTION);
}

#[test]
fn test_user_function_unknown_name_remains_default() {
    let highlighter = SqlHighlighter::new();
    let text = "SELECT MY_UNKNOWN(1) FROM dual";
    let styles = highlighter.generate_styles(text);

    let start = text.find("MY_UNKNOWN").unwrap();
    let end = start + "MY_UNKNOWN".len();
    assert!(
        styles[start..end].chars().all(|c| c == STYLE_DEFAULT),
        "unknown identifier should not be highlighted as identifier"
    );
}

#[test]
fn test_user_materialized_view_is_highlighted_like_table() {
    let mut highlighter = SqlHighlighter::new();
    highlighter.set_highlight_data(HighlightData {
        materialized_views: vec!["SALES_MV".to_string()],
        ..HighlightData::new()
    });

    let text = "SELECT * FROM SALES_MV";
    let styles = highlighter.generate_styles(text);
    assert_token_has_style(text, &styles, "SALES_MV", STYLE_IDENTIFIER);
}

#[test]
fn test_user_sequence_is_highlighted_like_table() {
    let mut highlighter = SqlHighlighter::new();
    highlighter.set_highlight_data(HighlightData {
        sequences: vec!["EMP_SEQ".to_string()],
        ..HighlightData::new()
    });

    let text = "SELECT EMP_SEQ.NEXTVAL FROM dual";
    let styles = highlighter.generate_styles(text);
    assert_token_has_style(text, &styles, "EMP_SEQ", STYLE_IDENTIFIER);
}

#[test]
fn test_user_synonym_is_highlighted_like_table() {
    let mut highlighter = SqlHighlighter::new();
    highlighter.set_highlight_data(HighlightData {
        synonyms: vec!["EMPLOYEE_SYN".to_string()],
        ..HighlightData::new()
    });

    let text = "SELECT * FROM EMPLOYEE_SYN";
    let styles = highlighter.generate_styles(text);
    assert_token_has_style(text, &styles, "EMPLOYEE_SYN", STYLE_IDENTIFIER);
}

#[test]
fn test_user_public_synonym_is_highlighted_like_table() {
    let mut highlighter = SqlHighlighter::new();
    highlighter.set_highlight_data(HighlightData {
        public_synonyms: vec!["DUAL_PUB".to_string()],
        ..HighlightData::new()
    });

    let text = "SELECT 1 FROM DUAL_PUB";
    let styles = highlighter.generate_styles(text);
    assert_token_has_style(text, &styles, "DUAL_PUB", STYLE_IDENTIFIER);
}

#[test]
fn test_user_type_is_highlighted_like_table() {
    let mut highlighter = SqlHighlighter::new();
    highlighter.set_highlight_data(HighlightData {
        types: vec!["MONEY_T".to_string()],
        ..HighlightData::new()
    });

    let text = "DECLARE v MONEY_T; BEGIN NULL; END;";
    let styles = highlighter.generate_styles(text);
    assert_token_has_style(text, &styles, "MONEY_T", STYLE_IDENTIFIER);
}

#[test]
fn test_user_trigger_is_highlighted_like_table() {
    let mut highlighter = SqlHighlighter::new();
    highlighter.set_highlight_data(HighlightData {
        triggers: vec!["EMP_AUDIT_TRG".to_string()],
        ..HighlightData::new()
    });

    let text = "ALTER TRIGGER EMP_AUDIT_TRG DISABLE";
    let styles = highlighter.generate_styles(text);
    assert_token_has_style(text, &styles, "EMP_AUDIT_TRG", STYLE_IDENTIFIER);
}

#[test]
fn test_user_index_is_highlighted_like_table() {
    let mut highlighter = SqlHighlighter::new();
    highlighter.set_highlight_data(HighlightData {
        indexes: vec!["IDX_EMP_NAME".to_string()],
        ..HighlightData::new()
    });

    let text = "ALTER INDEX IDX_EMP_NAME REBUILD";
    let styles = highlighter.generate_styles(text);
    assert_token_has_style(text, &styles, "IDX_EMP_NAME", STYLE_IDENTIFIER);
}

#[test]
fn test_user_event_is_highlighted_like_table() {
    let mut highlighter = SqlHighlighter::new();
    highlighter.set_db_type(crate::db::connection::DatabaseType::MySQL);
    highlighter.set_highlight_data(HighlightData {
        events: vec!["DAILY_PURGE_EVT".to_string()],
        ..HighlightData::new()
    });

    let text = "ALTER EVENT DAILY_PURGE_EVT DISABLE";
    let styles = highlighter.generate_styles(text);
    assert_token_has_style(text, &styles, "DAILY_PURGE_EVT", STYLE_IDENTIFIER);
}

#[test]
fn test_oracle_schema_qualified_sequence_is_highlighted() {
    let mut highlighter = SqlHighlighter::new();
    highlighter.set_highlight_data(HighlightData {
        sequences: vec!["EMP_SEQ".to_string()],
        schemas: vec!["HR".to_string()],
        ..HighlightData::new()
    });

    let text = "SELECT HR.EMP_SEQ.NEXTVAL FROM dual";
    let styles = highlighter.generate_styles(text);
    assert_token_has_style(text, &styles, "HR", STYLE_IDENTIFIER);
    assert_token_has_style(text, &styles, "EMP_SEQ", STYLE_IDENTIFIER);
}

#[test]
fn test_all_object_types_are_highlighted_on_oracle() {
    let mut highlighter = SqlHighlighter::new();
    highlighter.set_db_type(crate::db::connection::DatabaseType::Oracle);
    highlighter.set_highlight_data(HighlightData {
        tables: vec!["T_TBL".to_string()],
        views: vec!["V_VIEW".to_string()],
        materialized_views: vec!["MV_VIEW".to_string()],
        functions: vec!["FN_FUNC".to_string()],
        procedures: vec!["PR_PROC".to_string()],
        packages: vec!["PKG_NAME".to_string()],
        sequences: vec!["SEQ_NAME".to_string()],
        triggers: vec!["TRG_NAME".to_string()],
        types: vec!["TYP_NAME".to_string()],
        indexes: vec!["IDX_NAME".to_string()],
        synonyms: vec!["SYN_NAME".to_string()],
        public_synonyms: vec!["PUB_SYN".to_string()],
        schemas: vec!["HR".to_string()],
        ..HighlightData::new()
    });

    let text = "SELECT HR.T_TBL.x, HR.V_VIEW.y, HR.MV_VIEW.z, HR.FN_FUNC(1), HR.PKG_NAME.PR_PROC, HR.SEQ_NAME.NEXTVAL, HR.TRG_NAME, HR.TYP_NAME, HR.IDX_NAME, HR.SYN_NAME, HR.PUB_SYN FROM dual";
    let styles = highlighter.generate_styles(text);
    for token in [
        "HR", "T_TBL", "V_VIEW", "MV_VIEW", "FN_FUNC", "PKG_NAME", "PR_PROC", "SEQ_NAME",
        "TRG_NAME", "TYP_NAME", "IDX_NAME", "SYN_NAME", "PUB_SYN",
    ] {
        assert_token_has_style(text, &styles, token, STYLE_IDENTIFIER);
    }
}

#[test]
fn test_all_object_types_are_highlighted_on_mysql() {
    let mut highlighter = SqlHighlighter::new();
    highlighter.set_db_type(crate::db::connection::DatabaseType::MySQL);
    highlighter.set_highlight_data(HighlightData {
        tables: vec!["T_TBL".to_string()],
        views: vec!["V_VIEW".to_string()],
        functions: vec!["FN_FUNC".to_string()],
        procedures: vec!["PR_PROC".to_string()],
        triggers: vec!["TRG_NAME".to_string()],
        events: vec!["EVT_NAME".to_string()],
        indexes: vec!["IDX_NAME".to_string()],
        schemas: vec!["MY_DB".to_string()],
        ..HighlightData::new()
    });

    let text = "SELECT MY_DB.T_TBL.x, MY_DB.V_VIEW.y, MY_DB.FN_FUNC(1), MY_DB.PR_PROC, MY_DB.TRG_NAME, MY_DB.EVT_NAME, MY_DB.IDX_NAME";
    let styles = highlighter.generate_styles(text);
    for token in [
        "MY_DB", "T_TBL", "V_VIEW", "FN_FUNC", "PR_PROC", "TRG_NAME", "EVT_NAME", "IDX_NAME",
    ] {
        assert_token_has_style(text, &styles, token, STYLE_IDENTIFIER);
    }
}

#[test]
fn test_all_object_types_are_highlighted_on_mariadb() {
    let mut highlighter = SqlHighlighter::new();
    highlighter.set_db_type(crate::db::connection::DatabaseType::MariaDB);
    highlighter.set_highlight_data(HighlightData {
        tables: vec!["T_TBL".to_string()],
        views: vec!["V_VIEW".to_string()],
        functions: vec!["FN_FUNC".to_string()],
        procedures: vec!["PR_PROC".to_string()],
        triggers: vec!["TRG_NAME".to_string()],
        events: vec!["EVT_NAME".to_string()],
        indexes: vec!["IDX_NAME".to_string()],
        schemas: vec!["MY_DB".to_string()],
        ..HighlightData::new()
    });

    let text = "SELECT MY_DB.T_TBL.x, MY_DB.V_VIEW.y, MY_DB.FN_FUNC(1), MY_DB.PR_PROC, MY_DB.TRG_NAME, MY_DB.EVT_NAME, MY_DB.IDX_NAME";
    let styles = highlighter.generate_styles(text);
    for token in [
        "MY_DB", "T_TBL", "V_VIEW", "FN_FUNC", "PR_PROC", "TRG_NAME", "EVT_NAME", "IDX_NAME",
    ] {
        assert_token_has_style(text, &styles, token, STYLE_IDENTIFIER);
    }
}

#[test]
fn test_mariadb_db_qualified_event_is_highlighted() {
    let mut highlighter = SqlHighlighter::new();
    highlighter.set_db_type(crate::db::connection::DatabaseType::MariaDB);
    highlighter.set_highlight_data(HighlightData {
        events: vec!["DAILY_PURGE".to_string()],
        schemas: vec!["MY_DB".to_string()],
        ..HighlightData::new()
    });

    let text = "ALTER EVENT MY_DB.DAILY_PURGE DISABLE";
    let styles = highlighter.generate_styles(text);
    assert_token_has_style(text, &styles, "MY_DB", STYLE_IDENTIFIER);
    assert_token_has_style(text, &styles, "DAILY_PURGE", STYLE_IDENTIFIER);
}

/// The JavaScript body of an MLE module is inert: no keyword, function or
/// identifier styling may reach inside it, and the body ends at the bare `/`.
#[test]
fn test_mle_module_body_is_highlighted_as_foreign_source() {
    let highlighter = SqlHighlighter::new();
    let text = "CREATE OR REPLACE MLE MODULE js_mod LANGUAGE JAVASCRIPT AS\n\
export function addUp(a, b) {\n\
  const parts = [a, b].map((n) => n * 1);   // SELECT * FROM dual; not SQL\n\
  return parts.reduce((acc, n) => acc + n, 0);\n\
}\n\
/\n\
SELECT 1 FROM dual";
    let styles = highlighter.generate_styles(text);

    let body_start = text.find("export").expect("body");
    let body_end = text.find("\n/\n").expect("terminator");
    assert!(
        styles[body_start..body_end]
            .chars()
            .all(|style| style == STYLE_FOREIGN_SOURCE),
        "module body must be one inert span: {:?}",
        &styles[body_start..body_end]
    );

    // The SQL that follows the terminator is highlighted normally again.
    let select_start = text.rfind("SELECT").expect("SQL after MLE terminator");
    assert!(
        styles[select_start..select_start + "SELECT".len()]
            .chars()
            .all(|style| style == STYLE_KEYWORD),
        "SQL after the MLE terminator must be highlighted normally"
    );
    assert_word_has_style(text, &styles, "CREATE", STYLE_KEYWORD);
}

/// The inline `{{ ... }}` call-spec body is inert too, and a nested object
/// literal does not end it early.
#[test]
fn test_inline_mle_body_is_highlighted_as_foreign_source() {
    let highlighter = SqlHighlighter::new();
    let text = "CREATE OR REPLACE FUNCTION reverse_text (word VARCHAR2) RETURN VARCHAR2\n\
AS MLE LANGUAGE JAVASCRIPT\n\
{{ const map = { first: 1 }; return [...WORD].reverse().join('') + map.first; }};\n\
SELECT 2 FROM dual";
    let styles = highlighter.generate_styles(text);

    let body_start = text.find("{{").expect("inline body");
    let body_end = text.find("}}").expect("inline body end") + 2;
    assert!(
        styles[body_start..body_end]
            .chars()
            .all(|style| style == STYLE_FOREIGN_SOURCE),
        "inline body must be one inert span: {:?}",
        &styles[body_start..body_end]
    );
    assert_word_has_style(text, &styles, "SELECT", STYLE_KEYWORD);
}

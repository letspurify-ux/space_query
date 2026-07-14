# 자동 포맷팅 오류 감지 및 개선 리포트

## 1. 목적

SQL 테스트 파일이 많아 사람이 모든 자동 포맷 결과를 반복해서 확인하기 어려운 문제를 개선한다.

- 첫 번째 자동 포맷 결과부터 오류를 감지한다.
- 실제 SQL 문법 전체를 다시 구현하지 않고 단순한 검사로 이상을 찾는다.
- Oracle/PLSQL, MySQL, MariaDB 및 SQL*Plus 혼합 스크립트를 점검한다.
- 발견된 오류는 회귀 테스트로 고정한다.

## 2. 자동 감지 방식

첫 번째 포맷 결과에 다음 검사를 적용한다.

1. 코드 줄의 들여쓰기가 4칸 단위인지 검사
2. 줄 끝 불필요한 공백 검사
3. 포맷 전후 SQL statement/token fingerprint 비교
4. 포맷 결과를 다시 포맷했을 때 결과가 달라지는지 검사
5. 불필요한 줄바꿈 의존성과 안전한 토큰 간격 검사

SQL 문맥을 모두 재현하는 규칙은 넣지 않았다. 포맷 결과 자체에서 확인할 수 있는 구조적 신호와 토큰 보존 여부만 사용한다.

## 3. 기존 테스트 기대 결과 변경

아래 항목은 입력 SQL은 유지하면서 기존 테스트가 기대하던 잘못된 포맷 결과를 수정한 사례다.

### 3.1 `IS NULL` 복합 조건

AS-IS:

```sql
IF :NEW.note IS
NULL THEN
```

TO-BE:

```sql
IF :NEW.note IS NULL THEN
```

관련 테스트: `format_sql_trigger_if_elsif_alignment_matches_expected`

### 3.2 `WITHIN GROUP` 복합 구문

AS-IS:

```sql
LISTAGG (ename, ',') WITHIN
GROUP (ORDER BY ename) OVER (
    PARTITION BY deptno
)
```

TO-BE:

```sql
LISTAGG (ename, ',') WITHIN GROUP (ORDER BY ename) OVER (
    PARTITION BY deptno
)
```

관련 테스트: `format_sql_window_functions_and_listagg_exact_layout`

### 3.3 단항 음수 공백

AS-IS:

```sql
RAISE_APPLICATION_ERROR (- 20002, 'error');
EXIT outer_loop WHEN v_inner = - 1;
```

TO-BE:

```sql
RAISE_APPLICATION_ERROR (-20002, 'error');
EXIT outer_loop WHEN v_inner = -1;
```

관련 테스트:

- `format_sql_parenthesized_if_condition_continuation_uses_single_extra_indent`
- `format_sql_fmt_pkg_extreme_package_body_keeps_member_recovery_after_nested_exception_sections`
- `format_sql_torture_package_body_keeps_nested_blocks_and_labels`

### 3.4 주석 다음 고립된 쉼표

AS-IS:

```sql
SET first_value = 1
    -- comment
    ,
    second_value = 2
```

TO-BE:

```sql
SET first_value = 1
    -- comment
    , second_value = 2
```

관련 테스트:

- `full_auto_formatting_test24_package_body_set_comment_and_comma_follow_set_depth`
- `format_sql_basic_keeps_set_comment_and_comma_aligned_to_existing_multiline_set_depth`
- `format_sql_basic_keeps_comma_indent_after_line_comment_in_merge_using_clause`

### 3.5 Oracle/MySQL/MariaDB `RETURN CASE`

AS-IS:

```sql
RETURN
    CASE UPPER(TRIM(p_currency_code))
        WHEN 'USD' THEN
            1.0000
        ELSE
            0.0000
    END;
```

TO-BE:

```sql
RETURN CASE UPPER(TRIM(p_currency_code))
    WHEN 'USD' THEN
        1.0000
    ELSE
        0.0000
END;
```

관련 테스트:

- `format_sql_keeps_mariadb_test1_function_case_and_window_definition_depths`
- `visual_oracle_return_case_stays_on_return_owner_depth`
- `visual_mysql_profiles_keep_return_case_as_one_expression_phrase`

## 4. 실제 스윕에서 추가로 발견한 오류

### 4.1 CASE 분기 내부 DML 절 들여쓰기

AS-IS:

```sql
CASE p_action
    WHEN 'BONUS' THEN
        UPDATE employees
    SET salary = salary + 1
    WHERE employee_id = p_id;
    WHEN 'AUDIT' THEN
        INSERT INTO audit_log
    VALUES (p_id, 'PROCESSED');
END CASE;
```

TO-BE:

```sql
CASE p_action
    WHEN 'BONUS' THEN
        UPDATE employees
        SET salary = salary + 1
        WHERE employee_id = p_id;
    WHEN 'AUDIT' THEN
        INSERT INTO audit_log
        VALUES (p_id, 'PROCESSED');
END CASE;
```

`UPDATE`, `SET`, `WHERE`가 같은 깊이를 사용하고 `INSERT`, `VALUES`도 같은 깊이를 사용하도록 수정했다.

추가 회귀 테스트: `visual_oracle_keeps_case_branch_dml_clauses_on_the_statement_depth`

### 4.2 한 줄에 붙은 SQL*Plus 명령과 SQL

AS-IS:

```sql
SET PAGESIZE 50 WHENEVER SQLERROR EXIT SQL.SQLCODE ROLLBACK PROMPT Creating tables CREATE TABLE test_table (
id NUMBER
);
```

TO-BE:

```sql
SET PAGESIZE 50

WHENEVER SQLERROR EXIT SQL.SQLCODE ROLLBACK

PROMPT Creating tables

CREATE TABLE test_table (
    id NUMBER
);
```

포맷터에서 위 문자열 조합만 다시 쓰던 후처리를 제거했다. 대신 format/script 양쪽이 함께 사용하는 `process_split_line` 앞에서 실제 tool command parser가 완결된 명령 prefix를 소비하고, 다음 SQL*Plus 명령 또는 SQL statement head에서 경계를 여는 방식으로 수정했다. 따라서 `SET` 옵션, `WHENEVER` action, `PROMPT` payload, 뒤따르는 SQL의 구체적인 문자열 조합에 의존하지 않는다.

독립 `PROMPT example CREATE TABLE text only`의 payload는 분리하지 않으며, q-quoted literal 내부의 `/`도 실행 경계로 오인하지 않는다.

추가 회귀 테스트:

- `split_format_items_recovers_concatenated_sqlplus_commands_by_command_grammar`
- `split_format_items_does_not_split_sql_words_inside_standalone_prompt_payload`
- `split_format_items_does_not_recover_slash_text_inside_q_quote`
- `split_format_items_keeps_compound_clear_command_before_following_sql`

### 4.3 Slash와 여러 `COLUMN` 명령이 한 줄에 붙은 경우

AS-IS:

```sql
/ COLUMN id FORMAT 9999 COLUMN data FORMAT A30
```

TO-BE:

```sql
/

COLUMN id FORMAT 9999

COLUMN data FORMAT A30
```

Slash 실행 경계와 각 SQL*Plus `COLUMN` 명령을 별도 실행 단위로 분리한다.

추가 회귀 테스트: `visual_sqlplus_mixed_line_preserves_all_source_tokens`

### 4.4 중첩 SELECT의 첫 표현식 앞 주석 들여쓰기

47개 최종 포맷 결과 28,567줄을 육안 검토하면서 자동 검사에서 놓친 항목을 발견했다.

AS-IS:

```sql
FROM (
        SELECT
        /* comment before expression */
        calc_net(i.qty) AS net_amount
        FROM visual_order i
    ) v
```

TO-BE:

```sql
FROM (
        SELECT
            /* comment before expression */
            calc_net(i.qty) AS net_amount
        FROM visual_order i
    ) v
```

단독 `SELECT` 다음에 새 줄 블록 주석이 오면, 주석과 첫 표현식 모두 활성 select-list 깊이를 사용하도록 수정했다.

추가 회귀 테스트: `visual_mariadb_indents_comment_and_first_nested_select_item`

### 4.5 인라인뷰 내부 `WITH` 블록 과다 들여쓰기

AS-IS:

```sql
FROM (
            /* inline view */
            WITH x AS (
                SELECT id
                FROM paid
            )
        SELECT x.*
        FROM x
    ) v
```

TO-BE:

```sql
FROM (
        /* inline view */
        WITH x AS (
            SELECT id
            FROM paid
        )
        SELECT x.*
        FROM x
    ) v
```

인라인뷰 시작 주석 다음 토큰이 `WITH`인 경우 기존 query child depth를 재사용하여 주석, `WITH`, 메인 `SELECT`가 같은 깊이를 사용하도록 수정했다.

추가 회귀 테스트: `visual_oracle_aligns_commented_inline_view_with_and_main_select`

### 4.6 `WITHIN GROUP ORDER BY` CASE 다음 형제 항목 깊이

최신 47개 결과를 다시 육안 검토하면서 자동 검사에서 놓친 항목을 발견했다.

AS-IS:

```sql
ORDER BY
    CASE
        WHEN salary IS NULL THEN 999999999
        ELSE salary * -1
    END,
employee_id
```

TO-BE:

```sql
ORDER BY
    CASE
        WHEN salary IS NULL THEN 999999999
        ELSE salary * -1
    END,
    employee_id
```

`WITHIN GROUP`의 multiline `ORDER BY`에서 CASE가 끝난 뒤 쉼표가 나오면 다음 형제 정렬 항목이 CASE operand 깊이를 유지하도록 수정했다. 일반 `ORDER BY`와 subquery 정렬 항목의 기존 동작은 변경하지 않는다.

추가 회귀 테스트: `visual_oracle_aligns_case_and_following_within_group_order_items`

### 4.7 `CREATE VIEW AS` CTE와 메인 SELECT 사이 주석 깊이

AS-IS:

```sql
CREATE OR REPLACE VIEW visual_v AS
    WITH visual_cte AS (
        SELECT id
        FROM visual_source
    )
/* query body */
    SELECT id
    FROM visual_cte
```

TO-BE:

```sql
CREATE OR REPLACE VIEW visual_v AS
    WITH visual_cte AS (
        SELECT id
        FROM visual_source
    )
    /* query body */
    SELECT id
    FROM visual_cte
```

다음 토큰이 query head이고 활성 query-body 기준 깊이가 있는 경우, standalone 주석에도 그 깊이를 최솟값으로 적용한다. 일반 SELECT의 절 사이 주석이나 괄호 내부 주석 규칙은 그대로 유지한다.

추가 회귀 테스트: `visual_oracle_aligns_create_view_query_boundary_comment`

### 4.8 Named argument의 CASE와 다음 인자 경계

AS-IS:

```sql
mutate_emp (
    p_status =>
        CASE
            WHEN MOD (p_id, 3) = 0 THEN 'ON_HOLD'
            ELSE 'ACTIVE'
        END, p_note => v_note
);
```

TO-BE:

```sql
mutate_emp (
    p_status => CASE
        WHEN MOD (p_id, 3) = 0 THEN 'ON_HOLD'
        ELSE 'ACTIVE'
    END,
    p_note => v_note
);
```

`=> CASE`를 하나의 표현식 시작으로 유지하고, CASE가 끝난 뒤의 다음 named argument만 형제 깊이로 분리했다.

추가 회귀 테스트: `visual_oracle_keeps_named_argument_case_attached_and_splits_the_next_argument`

### 4.9 `EXECUTE IMMEDIATE`와 CASE 표현식 깊이

AS-IS:

```sql
EXECUTE IMMEDIATE
CASE
    WHEN p_mode = 'A' THEN v_sql_a
    ELSE v_sql_b
END;
```

TO-BE:

```sql
EXECUTE IMMEDIATE
    CASE
        WHEN p_mode = 'A' THEN v_sql_a
        ELSE v_sql_b
    END;
```

`EXECUTE IMMEDIATE` 구문 frame을 열고 그 frame의 owner scope를 기준으로 CASE를 한 단계 깊게 배치했다. `USING` phase와 괄호 깊이도 같은 frame에 저장해 바깥 실행문의 상태와 섞이지 않게 했다.

추가 회귀 테스트: `visual_oracle_indents_execute_immediate_case_expression_from_its_owner`

### 4.10 Conditional `INSERT ALL` 분기와 driving query

AS-IS:

```sql
INSERT ALL
    WHEN dept_id = 30
    AND salary >= 100000 THEN
        INTO visual_high (id) VALUES (id)
    SELECT id
    FROM visual_emp
```

TO-BE:

```sql
INSERT ALL
    WHEN dept_id = 30
        AND salary >= 100000 THEN
        INTO visual_high (id) VALUES (id)
SELECT id
FROM visual_emp
```

`INSERT ALL/FIRST`에서 owner frame을 열고 `WHEN/ELSE`, 조건, branch body를 각각 owner +1, +2, +2의 상대 깊이로 계산한다. 분기들이 끝나는 driving `SELECT`에서 해당 frame만 닫아 `SELECT`/`FROM`을 바깥 소유 깊이로 복귀시켰다.

추가 회귀 테스트: `visual_oracle_aligns_conditional_insert_all_and_driving_select`

### 4.11 FOR 범위 CASE, 예외 처리 루프, 커서 `FOR UPDATE`

AS-IS:

```sql
FOR i IN 1..
CASE
    WHEN p_limit IS NULL THEN 5
    ELSE p_limit
END LOOP
    NULL;
END LOOP;
```

```sql
FOR j IN 1..SQL%BULK_EXCEPTIONS.COUNT LOOP
write_log ('ERROR');
END LOOP;
```

```sql
FOR r IN (
    SELECT id
    FROM visual_emp
    FOR UPDATE
) LOOP
        BEGIN
            NULL;
        END;
    END LOOP;
```

TO-BE:

```sql
FOR i IN 1..
    CASE
        WHEN p_limit IS NULL THEN 5
        ELSE p_limit
    END
LOOP
    NULL;
END LOOP;
```

```sql
FOR j IN 1..SQL%BULK_EXCEPTIONS.COUNT LOOP
    write_log ('ERROR');
END LOOP;
```

```sql
FOR r IN (
    SELECT id
    FROM visual_emp
    FOR UPDATE
) LOOP
    BEGIN
        NULL;
    END;
END LOOP;
```

FOR/WHILE 헤더의 최종 렌더링 깊이를 구문 frame으로 열어 LOOP 종결자까지 보존했다. 특히 `FOR`의 종료자 owner frame과 `IF/WHILE` 조건식 frame을 분리하여, 커서 서브쿼리가 조건식 레이아웃을 상속하지 않게 했다. 범위식의 CASE `END`와 루프 시작 `LOOP`는 서로 다른 줄에 두어 종결 대상의 모호성을 없앴고, 커서 쿼리 안의 `FOR UPDATE`는 바깥 `FOR` frame 안의 SQL 절로 유지한다.

추가 회귀 테스트:

- `visual_oracle_indents_case_used_as_a_for_range_expression`
- `visual_oracle_indents_for_loop_body_inside_exception_handler`
- `visual_oracle_keeps_cursor_for_update_inside_the_loop_header`

### 4.12 MODEL 규칙 형제와 다차원 좌표

AS-IS:

```sql
RULES UPSERT SEQUENTIAL ORDER (
    calc_bonus [ ANY,
    ANY ] = CASE
        WHEN calc_sal [ CV (),
    CV (empno) ] > 3000 THEN calc_sal [ CV (),
    CV (empno) ] * 0.20
        ELSE calc_sal [ CV (),
    CV (empno) ] * 0.05
    END, calc_sal [ ANY, ANY ] = calc_sal [ CV (), CV (empno) ]
)
```

TO-BE:

```sql
RULES UPSERT SEQUENTIAL ORDER (
    calc_bonus [ ANY, ANY ] = CASE
        WHEN calc_sal [ CV (), CV (empno) ] > 3000 THEN calc_sal [ CV (), CV (empno) ] * 0.20
        ELSE calc_sal [ CV (), CV (empno) ] * 0.05
    END,
    calc_sal [ ANY, ANY ] = calc_sal [ CV (), CV (empno) ]
)
```

`RULES (...)`를 규칙 형제 목록으로 인식하되, 대괄호 안의 다차원 좌표 쉼표는 형제 규칙 구분자로 취급하지 않는다. `ITERATE (...)`와 `UNTIL (...)` modifier 괄호는 기존처럼 compact하게 유지한다.

추가 회귀 테스트:

- `visual_oracle_splits_model_rule_siblings_after_case`
- `visual_oracle_keeps_model_iterate_and_until_modifier_parens_compact`

### 4.13 MariaDB IF 괄호 안 OR와 본문 깊이

AS-IS:

```sql
IF v_orders_cnt > 0
    AND (v_top_category IS NULL
    OR v_top_category = '') THEN
    SET v_message = 'missing category';
END IF;
```

TO-BE:

```sql
IF v_orders_cnt > 0
    AND (v_top_category IS NULL
        OR v_top_category = '') THEN
    SET v_message = 'missing category';
END IF;
```

괄호 내부 `OR`는 바깥 `AND`보다 한 단계 깊게 두고, IF 본문은 중첩 괄호 깊이를 상속하지 않고 조건 continuation 깊이에 맞춘다.

추가 회귀 테스트: `visual_mariadb_indents_nested_or_inside_procedure_if_parentheses`

### 4.14 MODEL cell reference의 FOR 범위

AS-IS:

```sql
RULES UPSERT (
    total_amt [
    FOR month_no
    FROM 1 TO 12 INCREMENT 1 ] = NVL (...)
)
```

TO-BE:

```sql
RULES UPSERT (
    total_amt [ FOR month_no FROM 1 TO 12 INCREMENT 1 ] = NVL (...)
)
```

MODEL 대괄호 안의 `FOR ... FROM ...`은 SQL 절이나 PL/SQL 루프가 아니라 cell reference 범위이므로 구조적 줄바꿈을 적용하지 않는다.

추가 회귀 테스트: `visual_oracle_keeps_model_for_cell_reference_inline`

### 4.15 빈 analytic OVER 괄호

AS-IS:

```sql
COUNT(*) OVER (
) AS total_cnt
```

TO-BE:

```sql
COUNT(*) OVER () AS total_cnt
```

토큰이 실제로 비어 있는 analytic 괄호는 multiline query frame이 아니라 compact frame으로 처리한다.

추가 회귀 테스트: `visual_oracle_keeps_empty_analytic_over_parentheses_compact`

### 4.16 CTE query 끝 블록 주석 깊이

AS-IS:

```sql
WITH base AS (
        SELECT d.id
        FROM visual_data d
    /* trailing query comment */
    )
```

TO-BE:

```sql
WITH base AS (
        SELECT d.id
        FROM visual_data d
        /* trailing query comment */
    )
```

query-like 괄호가 닫히기 직전의 주석은 닫는 괄호가 아니라 해당 query body 깊이에 맞춘다.

추가 회귀 테스트: `visual_oracle_aligns_trailing_cte_query_comment_with_query_body`

### 4.17 Analytic/named WINDOW 다중 키 깊이

AS-IS:

```sql
ROW_NUMBER () OVER (
    PARTITION BY dept_id,
    team_id
    ORDER BY salary DESC,
    employee_id
)
```

TO-BE:

```sql
ROW_NUMBER () OVER (
    PARTITION BY dept_id,
        team_id
    ORDER BY salary DESC,
        employee_id
)
```

analytic `OVER (...)`와 named `WINDOW ... AS (...)` 내부에서 `PARTITION BY`/`ORDER BY` 뒤의 형제 키를 헤더보다 한 단계 깊게 정렬한다.

추가 회귀 테스트:

- `visual_oracle_indents_following_analytic_order_key_below_order_by`
- `visual_mysql_indents_following_named_window_order_key_below_order_by`

### 4.18 Multiline SELECT의 BULK COLLECT 경계

AS-IS:

```sql
SELECT emp_id,
    NVL (bonus, 0) +
    CASE
        WHEN status = 'ACTIVE' THEN 11
        ELSE 7
    END BULK COLLECT INTO v_emp_ids,
    v_bonus
FROM qt_x_emp;
```

TO-BE:

```sql
SELECT emp_id,
    NVL (bonus, 0) +
    CASE
        WHEN status = 'ACTIVE' THEN 11
        ELSE 7
    END
BULK COLLECT INTO v_emp_ids,
    v_bonus
FROM qt_x_emp;
```

SELECT 목록이 이미 여러 줄이면 `BULK COLLECT INTO`를 마지막 식에서 분리한다. 단일 식 SELECT와 `FETCH ... BULK COLLECT INTO`는 기존처럼 inline으로 유지한다.

추가 회귀 테스트: `visual_oracle_breaks_bulk_collect_after_a_multiline_select_list`

### 4.19 여러 줄 WHEN 조건의 CASE 결과 깊이

AS-IS:

```sql
RETURN CASE
    WHEN p_sal >= 4000
        AND NVL (p_comm, 0) > 0 THEN
            'TOP_PLUS_COMM'
    WHEN p_sal >= 3000 THEN
        'TOP'
END;
```

TO-BE:

```sql
RETURN CASE
    WHEN p_sal >= 4000
        AND NVL (p_comm, 0) > 0 THEN
        'TOP_PLUS_COMM'
    WHEN p_sal >= 3000 THEN
        'TOP'
END;
```

`THEN`이 `AND` continuation 줄에 있어도 결과 깊이는 그 줄이 아니라 소유 `WHEN` 깊이를 기준으로 계산한다.

추가 회귀 테스트: `visual_oracle_aligns_case_results_after_multiline_when_conditions`

### 4.20 MERGE INSERT와 VALUES 절 경계

AS-IS:

```sql
WHEN NOT MATCHED THEN
    INSERT (id, emp_name, created_at) VALUES (s.id, s.emp_name, SYSTIMESTAMP);
```

TO-BE:

```sql
WHEN NOT MATCHED THEN
    INSERT (id, emp_name, created_at)
    VALUES (s.id, s.emp_name, SYSTIMESTAMP);
```

MERGE 분기에서도 일반 INSERT와 같이 `VALUES`를 독립 절로 렌더링하여 긴 column/value 목록이 한 행에 결합되지 않게 한다.

추가 회귀 테스트: `visual_oracle_splits_merge_insert_and_values_clauses`

### 4.21 CREATE TABLE 가상 컬럼의 datatype 폭 오염

AS-IS:

```sql
CREATE TABLE visual_virtual_column (
    id           NUMBER                                     NOT NULL,
    total_amount AS (ROUND (id * 11 * 12 * 13, 2))
);
```

TO-BE:

```sql
CREATE TABLE visual_virtual_column (
    id           NUMBER NOT NULL,
    total_amount AS (ROUND (id * 11 * 12 * 13, 2))
);
```

datatype이 없는 `AS (...)` 가상 컬럼 식은 다른 컬럼의 datatype 정렬 폭 계산에서 제외한다. 가상 컬럼 자체의 식과 토큰은 그대로 보존한다.

추가 회귀 테스트: `visual_oracle_virtual_column_does_not_pad_other_column_constraints`

### 4.22 CREATE TABLE partition/subpartition 정의 목록

AS-IS:

```sql
SUBPARTITION TEMPLATE (SUBPARTITION sp_open VALUES ('OPEN'), SUBPARTITION sp_closed VALUES ('CLOSED')) (PARTITION p_2025 VALUES LESS THAN (DATE '2026-01-01'), PARTITION p_future VALUES LESS THAN (MAXVALUE))
```

TO-BE:

```sql
SUBPARTITION TEMPLATE (
    SUBPARTITION sp_open VALUES ('OPEN'),
    SUBPARTITION sp_closed VALUES ('CLOSED')
)
(
    PARTITION p_2025 VALUES LESS THAN (DATE '2026-01-01'),
    PARTITION p_future VALUES LESS THAN (MAXVALUE)
)
```

`SUBPARTITION TEMPLATE` 뒤에 template 목록과 partition 정의 목록의 outer 괄호가 연속하는 구조에만 적용하여, 각 최상위 항목을 한 줄씩 렌더링한다. 함수 호출이나 `VALUES (...)` 내부 쉼표는 분리하지 않는다.

추가 회귀 테스트: `format_for_auto_formatting_expands_create_table_partition_definition_lists`

### 4.23 Analytic PARTITION BY 안의 제어 키워드형 별칭

AS-IS:

```sql
ROW_NUMBER () OVER (
    PARTITION BY
    IF.grp
    ORDER BY IF.a) AS rn,
        SUM (IF.c) OVER (...)
```

TO-BE:

```sql
ROW_NUMBER () OVER (
    PARTITION BY IF.grp
    ORDER BY IF.a
) AS rn,
SUM (IF.c) OVER (...)
```

`PARTITION BY` 안에서 점이 뒤따르는 `IF.`는 PL/SQL 제어문 시작이 아니라 테이블 별칭 qualifier로 처리한다. 잘못 열린 IF block frame이 이후 analytic 형제와 `FROM`/`ORDER BY` 깊이를 오염시키지 않게 했다.

추가 회귀 테스트: `visual_oracle_treats_if_qualifier_as_identifier_inside_analytic_partition`

### 4.24 Multiline EXECUTE IMMEDIATE USING bind 목록

AS-IS:

```sql
EXECUTE IMMEDIATE v_sql USING visual_seq.NEXTVAL,
CASE ...
END, v_name,
CASE ...
END, ROUND (v_amount, 2), v_note;
```

TO-BE:

```sql
EXECUTE IMMEDIATE v_sql USING visual_seq.NEXTVAL,
    CASE ...
    END,
    v_name,
    CASE ...
    END,
    ROUND (v_amount, 2),
    v_note;
```

`EXECUTE IMMEDIATE` frame의 `USING` phase에서 CASE가 하나라도 여러 줄로 렌더링되면 그 CASE와 이후 bind를 실행문 소유자보다 한 단계 깊은 형제 목록으로 분리한다. frame은 현재 scope와 괄호 깊이를 함께 보유하고 문장 경계에서 닫히므로, 내부 CASE/괄호가 바깥 실행문의 bind 정렬을 오염시키지 않는다. 짧은 `EXECUTE IMMEDIATE ... USING v1, v2`는 기존처럼 한 줄을 유지한다.

추가 회귀 테스트: `visual_oracle_splits_multiline_execute_immediate_using_bind_arguments`

### 4.25 ALTER TABLE SPLIT PARTITION 목적지 목록

AS-IS:

```sql
ALTER TABLE orders SPLIT PARTITION orders_2024
INTO (PARTITION orders_2024_h1
VALUES LESS THAN (TO_DATE ('2024-07-01', 'YYYY-MM-DD')), PARTITION orders_2024_h2
VALUES LESS THAN (TO_DATE ('2025-01-01', 'YYYY-MM-DD')));
```

TO-BE:

```sql
ALTER TABLE orders SPLIT PARTITION orders_2024
INTO (
    PARTITION orders_2024_h1
    VALUES LESS THAN (TO_DATE ('2024-07-01', 'YYYY-MM-DD')),
    PARTITION orders_2024_h2
    VALUES LESS THAN (TO_DATE ('2025-01-01', 'YYYY-MM-DD'))
);
```

Oracle `ALTER TABLE ... SPLIT PARTITION ... INTO (`의 최상위 목적지 목록에 쉼표가 있을 때만 outer 괄호를 여러 줄 목록으로 렌더링한다. `VALUES (...)`와 `TO_DATE (...)` 같은 중첩 괄호 내부는 기존처럼 유지한다.

추가 회귀 테스트: `visual_oracle_expands_alter_table_split_partition_destination_list`

### 4.26 Analytic `ORDER BY` 내부 scalar subquery 절 붕괴

자동 스윕이 PASS였지만 전체 결과를 육안 검토하면서 발견한 실제 오류다.

AS-IS:

```sql
SELECT SUM (b2.amount) FROM qt_fmt_bonus b2 WHERE b2.emp_id = e.emp_id
```

TO-BE:

```sql
SELECT SUM (b2.amount)
FROM qt_fmt_bonus b2
WHERE b2.emp_id = e.emp_id
```

analytic `OVER (...)` 안에서도 가장 안쪽 괄호가 query-like frame이면 일반 절 줄바꿈을 억제하지 않도록 수정했다. 그 결과 `ORDER BY (SELECT ... FROM ... WHERE ...)`의 세 절이 같은 query body 깊이에서 유지된다.

추가 회귀 테스트: `visual_oracle_keeps_analytic_order_by_scalar_subquery_clauses_multiline`

### 4.27 `EXCEPTION` 선언과 handler section 구분

AS-IS:

```sql
e_bad
EXCEPTION;
```

TO-BE:

```sql
e_bad EXCEPTION;
```

선언부의 `identifier EXCEPTION;`은 한 선언으로 유지하되, 아래 handler section의 `EXCEPTION`은 계속 구조적 블록 경계로 처리한다.

```sql
EXCEPTION
    WHEN OTHERS THEN
```

추가 회귀 테스트:

- `visual_oracle_keeps_exception_declaration_and_outer_begin_on_owner_depth`
- `format_sql_basic_oracle_declaration_exception_and_predicate_phrases_stay_inline`

### 4.28 제어 키워드형 `IF` CTE 이름

AS-IS:

```sql
WITH
IF AS (
        SELECT 1 AS id FROM dual
    )
    SELECT IF.id
    FROM IF;
```

TO-BE:

```sql
WITH IF AS (
    SELECT 1 AS id
    FROM DUAL
)
SELECT IF.id
FROM IF;
```

CTE/alias 위치의 `IF`는 PL/SQL block opener가 아니라 식별자로 처리한다. 같은 `WITH` 문맥의 local `PROCEDURE` 선언도 선언 소유 깊이를 유지한다.

추가 회귀 테스트:

- `visual_oracle_keeps_if_cte_and_outer_query_at_root_depth`
- `format_sql_basic_oracle_keyword_like_cte_and_with_procedure_keep_owner_depth`

### 4.29 행 시작 `REMARK` 식별자와 SQL*Plus 명령 구분

AS-IS:

```sql
CREATE TABLE visual_remark (
    id NUMBER,
remark VARCHAR2(20)
);
```

TO-BE:

```sql
CREATE TABLE visual_remark (
    id     NUMBER,
    remark VARCHAR2 (20)
);
```

괄호 안의 `REMARK`는 컬럼/표현식 식별자로 토큰화하고, top-level 행 시작 `REM`/`REMARK`만 SQL*Plus comment 명령으로 유지한다.

추가 회귀 테스트:

- `visual_oracle_treats_line_initial_remark_as_an_identifier`
- `format_sql_basic_oracle_remark_identifier_does_not_become_script_command`

### 4.30 MySQL/MariaDB `REPEAT()` 함수와 `REPEAT` loop 구분

AS-IS:

```sql
SELECT
    REPEAT
    ('ab', 3) AS repeated_value;
```

TO-BE:

```sql
SELECT REPEAT('ab', 3) AS repeated_value;
```

함수 호출은 compact expression으로 유지하고, procedural form만 block frame을 연다.

```sql
REPEAT
    SET i = i + 1;
UNTIL i >= 3
END REPEAT;
```

추가 회귀 테스트: `visual_mysql_profiles_distinguish_repeat_function_from_repeat_loop`

### 4.31 Compound trigger와 LOB/XMLTYPE storage header 경계

AS-IS:

```sql
FOR INSERT OR UPDATE ON visual_t COMPOUND TRIGGER TYPE id_tab IS TABLE OF NUMBER;
```

```sql
LOB (doc) STORE AS BASICFILE LOB (ndoc) STORE AS BASICFILE XMLTYPE COLUMN xdoc STORE AS BASICFILE CLOB;
```

TO-BE:

```sql
FOR INSERT OR UPDATE ON visual_t
    COMPOUND TRIGGER
    TYPE id_tab IS TABLE OF NUMBER;
```

```sql
LOB (doc) STORE AS BASICFILE
LOB (ndoc) STORE AS BASICFILE
XMLTYPE COLUMN xdoc STORE AS BASICFILE CLOB;
```

compound trigger 선언부와 연속 storage clause의 구조적 header를 각각 독립 행으로 복원한다.

추가 회귀 테스트: `visual_oracle_breaks_compound_trigger_and_storage_clause_headers`

### 4.32 `MATCH_RECOGNIZE` pattern quantifier 공백

AS-IS:

```sql
PATTERN (A B + C *)
```

TO-BE:

```sql
PATTERN (A B+ C*)
```

`MATCH_RECOGNIZE PATTERN` 안에서 identifier 뒤의 `+`/`*`는 산술 연산자가 아니라 row-pattern quantifier이므로 operand에 붙여 렌더링한다.

추가 회귀 테스트: `visual_oracle_keeps_match_recognize_quantifiers_attached`

### 4.33 PL/SQL label과 collection `.DELETE`

AS-IS:

```sql
<<top>> l_count := l_count + 1;
g_ids.
DELETE;
```

TO-BE:

```sql
<<top>>
l_count := l_count + 1;
g_ids.DELETE;
```

PL/SQL label은 다음 statement와 같은 깊이의 독립 행으로 유지한다. 점으로 한정된 collection method `.DELETE`는 SQL `DELETE` 절로 오인하지 않는다.

추가 회귀 테스트:

- `visual_oracle_keeps_plsql_label_on_its_own_line`
- `visual_oracle_aligns_plsql_comments_nested_blocks_and_member_calls`

### 4.34 Scalar/PIVOT/APPLY 주석 뒤 body 깊이

AS-IS:

```sql
SELECT (
    /* scalar */
        SELECT MAX(metric)
    FROM inner_t
)
PIVOT (
    /* aggregate */
        SUM(amount)
FOR category IN ('A')
)
CROSS APPLY (
    -- aggregate
        SELECT COUNT(*)
    FROM item_t
)
```

TO-BE:

```sql
SELECT (
    /* scalar */
    SELECT MAX (metric)
    FROM inner_t
)
PIVOT (
    /* aggregate */
    SUM (amount)
    FOR category IN ('A')
)
CROSS APPLY (
    -- aggregate
    SELECT COUNT(*)
    FROM item_t
)
```

괄호 body 첫 주석은 뒤따르는 query/aggregate head와 같은 child frame 깊이를 사용하고, `FROM`/`FOR` 같은 형제 절도 그 깊이를 재사용한다.

추가 회귀 테스트:

- `visual_oracle_comment_and_outer_from_return_to_query_depth`
- `visual_oracle_aligns_commented_query_pivot_and_apply_siblings`

### 4.35 MERGE의 키워드형 alias와 분리된 branch phrase

AS-IS:

```sql
MERGE INTO qt_if_base
IF
USING dual src
...
WHEN
NOT MATCHED THEN
```

TO-BE:

```sql
MERGE INTO qt_if_base IF
USING dual src
...
WHEN NOT MATCHED THEN
```

object alias 위치의 `IF`는 식별자로 유지하고, 입력에서 분리된 `WHEN NOT MATCHED`는 하나의 canonical MERGE branch phrase로 합친다.

추가 회귀 테스트: `format_sql_basic_merge_split_alias_and_when_not_converge_to_canonical_branches`

### 4.36 여러 줄 IF 조건 다음 body 깊이

AS-IS:

```sql
IF condition
    AND other_condition THEN
BEGIN
NULL;
END;
```

TO-BE:

```sql
IF condition
    AND other_condition THEN
    BEGIN
        NULL;
    END;
```

조건 continuation 깊이를 IF body frame으로 오인하지 않고, body는 IF owner에서 정확히 한 단계 깊게 연다.

추가 회귀 테스트: `visual_oracle_multiline_if_body_uses_one_structural_step`

### 4.37 입력 줄바꿈과 무관한 compound phrase

AS-IS:

```sql
FOR
UPDATE OF sal;
) AS
emp_cur
```

TO-BE:

```sql
FOR UPDATE OF sal;
) AS emp_cur
```

입력 줄바꿈만으로 `FOR UPDATE`나 close-paren 뒤 `AS alias` 같은 단일 문법 phrase를 분리하지 않는다.

추가 회귀 테스트: `format_sql_basic_oracle_sql_phrases_ignore_source_newlines_inside_phrase`

### 4.38 코드 행 trailing whitespace 제거

AS-IS (`␠`는 공백, `⇥`는 tab):

```text
SELECT -1 AS value;␠␠
FROM visual_t;⇥
```

TO-BE:

```text
SELECT -1 AS value;
FROM visual_t;
```

문자열 literal과 SQL*Plus `PROMPT` payload는 건드리지 않고 일반 코드 행 끝의 공백/tab만 제거한다.

추가 회귀 테스트: `visual_all_profiles_trim_line_ends_and_distinguish_unary_minus`

### 4.39 Oracle 조건부 컴파일 directive와 branch body 경계

AS-IS:

```sql
        $IF DBMS_DB_VERSION.VERSION >= 12 $THEN AUDIT ('qt_torture_pkg', 'complex_block.ccflag', 'conditional-compilation=true');
        $ELSE AUDIT ('qt_torture_pkg', 'complex_block.ccflag', 'conditional-compilation=false');
        $END AUDIT ('qt_torture_pkg', 'complex_block.end', 'updated=' || l_rows || '; aggregate=' || l_count);
```

TO-BE:

```sql
        $IF DBMS_DB_VERSION.VERSION >= 12 $THEN
            AUDIT ('qt_torture_pkg', 'complex_block.ccflag', 'conditional-compilation=true');
        $ELSE
            AUDIT ('qt_torture_pkg', 'complex_block.ccflag', 'conditional-compilation=false');
        $END
        AUDIT ('qt_torture_pkg', 'complex_block.end', 'updated=' || l_rows || '; aggregate=' || l_count);
```

일반 PL/SQL block stack과 분리된 조건부 컴파일 frame stack을 사용한다. `$IF`에서 owner/body 상대 깊이를 가진 frame을 열고 `$END`에서 닫는다. directive는 owner 깊이, 활성 branch body는 owner +1에 두며, 내부 `$IF`를 닫으면 바깥 branch의 body 깊이가 복원된다. 이 frame은 일반 문장 frame과 달리 세미콜론 경계에서 유지되므로 여러 문장과 중첩 `$IF`도 같은 상대 기준을 유지한다.

추가 회귀 테스트: `visual_oracle_splits_conditional_compilation_directives_from_branch_bodies`

### 4.40 Multiline `XMLTABLE`/`JSON_TABLE` 구조

AS-IS:

```sql
FROM XMLTABLE ('/rows/row' PASSING XMLTYPE ('<rows>
...
</rows>') COLUMNS emp_id NUMBER PATH 'emp_id', emp_name VARCHAR2 (100) PATH 'emp_name') x

FROM JSON_TABLE ('{
...
}', '$.employees[*]' COLUMNS emp_id NUMBER PATH '$.id', emp_name VARCHAR2 (100) PATH '$.name') j
```

TO-BE:

```sql
FROM XMLTABLE (
    '/rows/row'
    PASSING XMLTYPE (
        '<rows>
...
</rows>'
    )
    COLUMNS
        emp_id NUMBER PATH 'emp_id',
        emp_name VARCHAR2 (100) PATH 'emp_name'
) x

FROM JSON_TABLE (
    '{
...
}',
    '$.employees[*]'
    COLUMNS
        emp_id NUMBER PATH '$.id',
        emp_name VARCHAR2 (100) PATH '$.name'
) j
```

멀티라인 문자열 literal을 포함한 table function만 구조형 괄호 frame으로 승격한다. `PASSING`, `COLUMNS`, 괄호 없는 column 목록을 각각 구조 깊이에 맞춰 분리하고, 단일행 호출과 기존 `COLUMNS (...)` compact/nested 정책은 유지한다. `t.passing`, `t.columns`, `:columns` 및 projection column 이름은 frame phase 전환 키워드로 보지 않으며, `JSON_TABLE + (...)` 같은 grouping 괄호도 table-function frame으로 오인하지 않는다.

추가 회귀 테스트: `visual_oracle_expands_multiline_xmltable_and_json_table_clauses`

### 4.41 어제·오늘 변경분의 구문 frame 원칙 재감사

AS-IS:

```rust
let mut pending_for_while_owner_indent = None;
let mut insert_all_owner_indent = None;
let mut insert_all_branch_body_indent = None;
let mut execute_immediate_using_paren_depth = None;
let mut oracle_error_directive_depth = 0;
```

TO-BE:

```text
$IF                 -> OracleConditionalCompilationFrame(owner, body, branch/error phase)
INSERT ALL/FIRST    -> InsertAllFormatFrame(scope, owner, branch phase)
EXECUTE IMMEDIATE   -> ExecuteImmediateFormatFrame(scope, USING paren/phase)
FOR                 -> LoopHeader condition-owner frame
XML/JSON_TABLE (...) -> ParenStackFrame(owner/body, COLUMNS phase)
```

일회성 토큰 판단을 제외한 장기 레이아웃 상태를 `FormatFrameStack`의 구문 frame으로 이동했다. 각 자식 들여쓰기는 현재 출력의 절대값이나 별도 전역 변수에서 누적하지 않고 frame owner의 상대 깊이로 계산한다. 문장 단위 frame은 세미콜론에서 닫고, 조건부 컴파일 frame은 실제 `$END`에서 닫아 서로 다른 수명을 섞지 않는다.

중첩 `FOR (SELECT ... FOR UPDATE) LOOP`, 중첩 조건부 컴파일, 괄호 안/밖 `INSERT ALL` frame을 검증했고, 내부 frame을 닫은 뒤 바깥 상대 깊이가 복원되는 단위 테스트를 추가했다.

추가 회귀 테스트:

- `syntax_frames_restore_outer_relative_indent_after_nested_close`
- `statement_boundary_closes_statement_syntax_frames_but_keeps_conditional_owner`

## 5. 그 외 개선 범위

- Oracle/PLSQL 중첩 블록, `EXCEPTION`, `CASE`, `INSERT ALL`, `PIVOT`, `APPLY` 정렬
- `DELETE WHERE`, PL/SQL collection `.DELETE` 구분
- 분석 함수 `OVER`, `WITHIN GROUP`, `MATCH_RECOGNIZE` 줄바꿈
- MySQL/MariaDB `CASE`, `REPEAT` 함수/루프, 중첩 `ORDER BY` 구분
- Oracle 조건부 컴파일 branch와 multiline `XMLTABLE`/`JSON_TABLE` 구조 정렬
- 코드 줄의 trailing whitespace 제거
- 포맷 전후 SQL 토큰 보존 검사

## 6. 의도적으로 유지한 동작

SQL*Plus `PROMPT`는 화면에 출력되는 payload이므로 기존 정책대로 원본 대소문자와 선행 공백을 verbatim으로 보존한다. 따라서 `PROMPT`의 선행 공백 자체는 자동 포맷 오류로 분류하지 않는다.

일반 표현식의 최대 행 길이 정책은 새로 추가하지 않았다. 따라서 `oracle_format_final_boss_v2.sql`의 496자 dynamic SQL 문자열 결합은 기존 compact 정책대로 유지한다.

`oracle splitter final boss test.sql`의 `ORDER BY salary /`는 slash가 독립 행 terminator가 아닌 원본 fixture 경계 문제이며 formatter가 복구하지 않는다. 같은 파일에서 외부 block comment 안에 들어간 TEST-024/025와 요약 수치의 불일치도 formatter 변경 범위에서 제외했다.

## 7. 검증 결과

| 검증 | 결과 |
| --- | --- |
| 초기 `.format.out` 전수 수동 검토 | 47개, 28,567줄, formatter 오류 2종 확인 |
| 최종 `.format.out` 검증 | 47개, 28,599줄, 변경 2개 전수 재검토 + 미변경 45개 byte 동일, 오류 0건 |
| 지정 ignored 포맷 스윕 테스트 | 1/1 통과 |
| 실제 SQL 파일 자동 포맷 스윕 집계 | 47/47 통과, 실패 0건 |
| 수정 전후 `.format.out` delta 검토 | 45개 동일, `test11.txt`와 `test25.sql`만 변경 |
| 최종 반복 재생성 byte 비교 | 47개 모두 동일 |
| `visual_` 회귀 테스트 필터 | 50/50 통과 |
| 중첩 구문 frame 수명/복원 단위 테스트 | 2/2 통과 |
| 전체 라이브러리 테스트 | 6,424 통과, 실패 0, ignored 212 |
| `cargo fmt -- --check` | 통과 |
| `git diff --check` | 통과 |

스윕 집계 결과는 `target/format-sweep/format-sweep.out`에서 확인할 수 있다.

## 8. 주요 변경 파일

- `src/ui/sql_editor/formatter.rs`: 자동 포맷 로직 수정
- `src/ui/sql_editor/format_sweep_tests.rs`: 1차 오류 감지 및 전체 파일 리포트 생성
- `src/ui/sql_editor/visual_format_regression_tests.rs`: 육안 검토 결과 회귀 테스트
- `src/ui/sql_editor/query_text.rs`: SQL/SQL*Plus 토큰 처리 보완
- `src/db/query/script.rs`: SQL*Plus 명령 인자 및 실행 경계 처리 보완
- `src/sql_delimiter.rs`: MODEL cell reference용 대괄호 frame 상태 보완
- `src/ui/sql_editor/execution.rs`: 수정된 정상 출력에 맞춘 기존 회귀 기대값
- `src/ui/sql_editor/sql_editor_tests.rs`: 수정된 정상 출력에 맞춘 기존 회귀 기대값
- `src/ui/sql_editor/mod.rs`: 포맷 스윕 및 시각 회귀 테스트 모듈 등록
- `change.md`: 수동 검토에서 확인한 AS-IS/TO-BE와 최종 검증 수치 기록

---

# 2차 검토 회차 (2026-07-13, MySQL/MariaDB `@` 변수 · PREPARE)

스윕 47개 파일(28,599줄) 재검토와 최소 재현 프로브에서 formatter 결함 3건을 추가로
발견해 수정했다. 세 건 모두 frame(직전 토큰 인접성, 괄호/블록 depth, 문장 경계)을
참조하지 않고 절대 규칙으로 동작하던 지점이다. 수정은 전부
`src/ui/sql_editor/formatter.rs`에 적용했다.

## 9-1. `@` 사용자 변수 이름이 키워드 대문자화로 훼손

`@` 심볼 바로 뒤의 단어가 키워드 목록에 있으면(예: `sql`) 식별자임에도
대문자화되었다. 직전 토큰이 `@` 심볼이면 식별자로 취급하도록 수정
(`follows_at_sign_variable` → `keyword_preserves_original_case`).

AS-IS (원본 입력):
```sql
SET @sql = 'select 1';
```

AS-IS (수정 전 포맷 결과 — 변수명 훼손):
```sql
SET @SQL = 'select 1';
```

TO-BE (수정 후 포맷 결과):
```sql
SET @sql = 'select 1';
```

발견 위치: `test_mysql/test1.txt`(원본 628행), `test_mysql/test2.txt`(원본 607행).
`test_mariadb/test4.txt`는 과거 훼손 결과(`SET @SQL`)가 원본에 굳어진 상태로,
수정 후에는 원본 표기가 그대로 보존된다.

같은 원인의 파생 결함 — 절 키워드 이름의 변수에서 구조 줄바꿈 발생:

AS-IS (원본 입력):
```sql
SET @from = 1;
```

AS-IS (수정 전 포맷 결과 — 변수 내부에서 절 줄바꿈, 토큰 구조 파괴):
```sql
SET @
FROM = 1;
```

TO-BE (수정 후 포맷 결과):
```sql
SET @from = 1;
```

## 9-2. top-level 행이 `@`로 시작하면 실행단위 분리기가 문장을 절단

분리기는 paren/block depth 0에서 `@`/`@@`로 시작하는 라인을 스크립트 include
tool command로 취급해 열린 문장을 강제 종료한다(`script.rs` 9124행 부근의
`should_try_tool_command_with_open_statement`). 포맷터가 SELECT 리스트 줄바꿈으로
`@@var`를 행 첫머리에 놓으면 문장이 절단되었다. `@` 심볼을 top-level
(괄호 frame 0 && 블록 frame 없음) 행 첫머리에 그리려는 순간 직전 행으로
되붙이는 join-back을 추가했다. 괄호/블록 frame 내부에서는 분리기가 문장을
유지하므로 기존 레이아웃을 건드리지 않는다(1차 시도에서 무조건 join했다가
`CALL f(\n    @a ...)` 케이스의 idempotence가 깨져 조건을 frame depth로 좁혔다).

AS-IS (원본 입력):
```sql
SELECT @sql, @@sql_mode;
```

AS-IS (수정 전 포맷 결과 — 2행이 include 지시어로 오인되어 문장 절단):
```sql
SELECT @sql,
    @@sql_mode;
```

TO-BE (수정 후 포맷 결과):
```sql
SELECT @sql, @@sql_mode;
```

블록 내부(BEGIN..END, 분리기 안전 구간)는 기존 줄바꿈 유지:
```sql
    SELECT @a,
        @b,
        @@global.sql_mode,
        c
    INTO @r1,
        @r2
```

## 9-3. `PREPARE <name> FROM <source>`의 FROM을 쿼리 절로 오인

문장 컨텍스트를 보지 않고 FROM을 항상 쿼리 절로 취급해 줄바꿈했다.
`suppresses_clause_break`에 "문장 경계(`;` 초기화 / BEGIN/THEN/ELSE/DO/LOOP)
직후의 `PREPARE <name>` 다음 FROM이면 줄바꿈 억제" 규칙을 추가했다.

AS-IS (원본 입력):
```sql
PREPARE stmt_flat FROM @sql;
```

AS-IS (수정 전 포맷 결과):
```sql
PREPARE stmt_flat
FROM @sql;
```

TO-BE (수정 후 포맷 결과):
```sql
PREPARE stmt_flat FROM @sql;
```

발견 위치(총 8곳): `test_mysql/test1.txt`, `test_mysql/test2.txt`,
`test_mysql/test3.txt`(2), `test_mariadb/test4.txt`, `test_mariadb/test6.txt`,
`test_mariadb/test7.txt`(2). `SELECT prepare_col, x FROM t2;`처럼 `prepare`가
일반 식별자인 경우 FROM 줄바꿈이 정상 유지됨을 확인했다.

## 9-4. 검토했으나 수정하지 않은 항목 (의도된 설계로 확인)

- `@TRANSACTION` → `SET AUTOCOMMIT OFF` 렌더링(`test_mariadb/test7.txt`):
  `script.rs:9968`의 별칭 정규화 설계.
- mid-line CASE의 frame 앵커(`p_status => CASE` 뒤 WHEN이 소유 라인 +4):
  "CASE opened mid-line" 회귀 테스트로 보호되는 의도된 상대 규칙.
- FROM-서브쿼리(내용 +8/닫기 +4) vs JOIN-서브쿼리(내용 +4/닫기 +0) 비대칭:
  각각 opener 라인 기준 상대 규칙으로 일관, 회귀 테스트로 고정된 스타일.
- Oracle `emp@dblink` → `emp @dblink`(@ 앞 공백): 이번 수정과 무관한 기존
  동작이며 Oracle 문법상 유효. 추후 개선 후보로만 기록.
- `EXECUTE stmt USING ...`의 USING 줄바꿈: Oracle `OPEN ... FOR ... USING`과
  동일 계열 절 취급으로 유지.

## 9-5. 추가된 회귀 테스트 (`formatter_regression_tests`)

| 테스트 | 검증 내용 |
| --- | --- |
| `mysql_user_variable_word_after_at_sign_preserves_case` | `@sql` 케이스 보존 |
| `mysql_user_variable_named_clause_keyword_stays_inline` | `SET @from = 1;` 한 줄 유지 |
| `mysql_top_level_statement_line_never_starts_with_at_sign` | top-level 행이 `@`로 시작 금지 |
| `mysql_prepare_from_stays_on_one_line` | PREPARE FROM 한 줄 유지 (top-level·프로시저 본문) |

## 9-6. 2차 회차 검증 결과

| 검증 | 결과 |
| --- | --- |
| `.format.out` 전수 눈검토 | 47개, 28,599줄 |
| 최소 재현 프로브 3종 (mysql 2, oracle 1) | 전부 PASS (수정 전 mysql 프로브는 FAIL 2건) |
| 실제 SQL 파일 자동 포맷 스윕 집계 | 47/47 통과, 실패 0건 |
| 전체 라이브러리 테스트 | 6,428 통과, 실패 0, ignored 212 |

---

# 3차 정밀 검토 회차 (2026-07-13, 주석 뒤 `@` 변수 continuation 절단)

2차 회차에서 요약 검토에 그쳤던 대형 파일 9개(test11, test18, test21,
oracle_format_ultimate_boss, oracle_format_final_boss, oracle_format_final_boss_v2,
test_mariadb/test5·test8, oracle splitter final boss test — 약 10,700줄)를 추가로
전량 정독했고(전부 이상 없음, splitter final boss의 함정 구간은 1차 §6 문서화 유지),
적대적 프로브에서 결함 1건을 추가 발견해 수정했다.

## 10-1. 주석 뒤 `@` 변수 continuation을 스플리터가 include로 오인해 문장 절단

`SELECT a, -- 주석` 다음 줄이 `@x, b FROM t;`처럼 `@` 사용자 변수로 시작하면,
행 첫머리 `@`는 join-back이 불가능(주석을 삼키게 됨)해 그대로 남고, 스플리터가
열린 문장을 tool command(@ include)로 강제 종료해 문장이 파괴되었다. 이는
포맷터 출력뿐 아니라 **사용자가 직접 입력한 SQL 실행 경로에서도 동일하게
발생하던 기존 결함**이다(블록 주석 `/* */` 뒤에서도 동일).

AS-IS (원본 입력):
```sql
SELECT a, -- comment before var
@x, b FROM t;
```

AS-IS (수정 전 — 2행이 include로 오인되어 문장 절단, 포맷 결과도 미종결):
```sql
SELECT a,
-- comment before var

@x, b FROM t
```

TO-BE (수정 후 포맷 결과 — 한 문장 유지):
```sql
SELECT a, -- comment before var
    @x,
    b
FROM t;
```

수정 내용:
- `sql_parser_engine/engine.rs`: `current_ends_with_list_continuation_comma()` 추가 —
  누적 문장의 끝이 (후행 라인/블록 주석을 걷어낸 뒤) 콤마이면 다음 줄은 문법적
  continuation으로 판단.
- `db/query/script.rs`: 열린 문장 중 tool command 판정 gate 2곳
  (slash-terminable 경로, with-open-statement 경로)에서 `@` 후보 + 콤마
  continuation이면 절단하지 않고 문장에 이어붙임. 완결된 문장 뒤의
  `@script.sql` include는 기존대로 tool command로 인식(회귀 테스트로 고정).
- `ui/sql_editor/formatter.rs`: join-back을 모든 주석 뒤에서 건너뛰도록 단순화
  (블록 주석 뒤 join 시 1·2차 패스 간 주석 배치가 달라지는 비멱등 발견분 해소;
  주석+콤마 케이스는 위 스플리터 가드가 보호).

## 10-2. 추가된 회귀 테스트

| 테스트 | 검증 내용 |
| --- | --- |
| `split_script_items_keeps_at_sign_variable_after_trailing_comma_in_open_statement` | 주석 뒤 `@x` continuation이 한 문장으로 유지 |
| `split_script_items_still_treats_include_after_complete_statement_as_command` | 문장 경계의 `@path`는 여전히 include 명령 |

## 10-3. 3차 회차 검증 결과

| 검증 | 결과 |
| --- | --- |
| 대형 9개 파일 추가 정독 | 약 10,700줄, 신규 이상 없음 |
| 적대적 프로브 6종 (할당식·backtick 변수·시스템 변수·CASE+@·prepare 동명 식별자·주석/공백행+@) | 전부 PASS |
| 실제 SQL 파일 자동 포맷 스윕 집계 | 47/47 통과, 실패 0건 |
| 전체 라이브러리 테스트 | 6,430 통과, 실패 0, ignored 212 |
| `cargo fmt -- --check` | 통과 |

---

# 4차 정밀 검토 회차 (2026-07-14, frame 상대 구조 정합성)

`formatting_sweep_all_files_generate_out_report` 1차 검사(47개 파일 자동검사 PASS) 후,
생성된 `.format.out` 47개(총 28,589줄)를 전량 육안 정독했다. 자동검사 PASS와
무관하게 "구문에 따라 frame이 열리고 닫히는 상대적 구조" 원칙에 어긋나는 지점
4건을 발견해 모두 수정했다. (검토: Oracle 39 · MySQL 3 · MariaDB 5 파일)

## 11-1. 호출 괄호 안 inline `CASE ... END` 뒤 인자가 괄호 frame을 이탈

인자 위치(`=>` / `,` / `(` 직후)에서 줄 중간에 시작한 CASE의 owner depth가
문장 라인 기준으로 고정되어, `END`와 그 뒤 인자가 괄호 frame(+1)이 아닌
문장 레벨(+0)로 떨어졌다. CASE owner를 compact 괄호 frame의
`sibling_body_indent` 기준으로 상향해 END·후속 인자가 모두 괄호 내부 레벨에
정렬되도록 수정했다. (`||` 등 연산자 뒤에 줄바꿈되는 CASE는 기존 동작 유지)

- 발견 위치: `test/test22.sql.format.out` 378~384행, `test/test23.sql.format.out` 400~406행
- 수정 코드: `ui/sql_editor/formatter.rs` — CASE push 시 `case_starts_call_argument`
  판정 + `suppresses_comma_breaks` 괄호의 `sibling_body_indent`로 owner 상향

### AS-IS (test22.sql)

```sql
            qt_split_pkg.complex_upsert (p_user_id => v_ids (i), p_status => CASE
                WHEN MOD (v_ids (i), 2) = 0 THEN
                    'A'
                ELSE
                    'I'
            END,
            p_note => 'generated in anon block; idx=' || i || ' / id=' || v_ids (i));
```

`END,`/`p_note =>`가 호출문과 같은 레벨로 떨어져 괄호 내부임이 드러나지 않음.

### TO-BE

```sql
            qt_split_pkg.complex_upsert (p_user_id => v_ids (i), p_status => CASE
                    WHEN MOD (v_ids (i), 2) = 0 THEN
                        'A'
                    ELSE
                        'I'
                END,
                p_note => 'generated in anon block; idx=' || i || ' / id=' || v_ids (i));
```

`END,`와 `p_note =>`가 호출 괄호 frame +1 레벨에 정렬. test23의
`p_append_text => ...`도 동일하게 교정됨.

## 11-2. MySQL/MariaDB `DECLARE ... CONDITION FOR`가 FOR에서 잘못 줄바꿈

`FOR`가 루프 키워드 규칙에 걸려 선언문 중간에서 +0 레벨로 줄바꿈됐다.
`DECLARE <name> CONDITION FOR <condition>`을 커서/핸들러 선언과 같은
declare-for 계열로 인식해 인라인 유지하도록 수정했다.

- 발견 위치: `test_mariadb/test5.txt.format.out` 531~532행
- 수정 코드: `ui/sql_editor/formatter.rs` —
  `is_mysql_declare_condition_for_clause_from_indices` 추가,
  `mysql_declare_for_clause`에 포함

### AS-IS (test_mariadb/test5.txt)

```sql
    DECLARE user_error CONDITION
    FOR SQLSTATE '45000';
```

### TO-BE

```sql
    DECLARE user_error CONDITION FOR SQLSTATE '45000';
```

## 11-3. 주석으로 끊긴 절 연속행이 앞선 frame 닫힘 후 +1을 잃음

주석 연속행 판정에 쓰는 `comment_prefix_text`(현재 줄의 구조 prefix)가
"실제 렌더된 현재 줄"이 아니라 "마지막 line-start 이후 누적 토큰"이어서,
MATCH_RECOGNIZE 닫는 괄호처럼 한 토큰 처리 중 줄바꿈이 여러 번 일어나는
문맥에서는 `... ) FETCH FIRST` 같은 오염된 prefix로 구조 헤더 매칭이
실패했고, 주석 뒤 연속 피연산자가 +0으로 떨어졌다. `newline_with`에서
prefix를 비워 prefix가 항상 현재 렌더 라인과 일치하도록 수정했다.

- 발견 위치: `test/test_open_with.sql.format.out` 472~473행
  (`OPEN ... FOR` + MATCH_RECOGNIZE 문맥)
- 수정 코드: `ui/sql_editor/formatter.rs` — `comment_prefix_text`를
  `RefCell<String>`으로 전환하고 `newline_with`에서 clear

### AS-IS (test_open_with.sql)

```sql
        )
        FETCH FIRST /* BV: 상위 20건 */
        20 ROWS ONLY;
```

### TO-BE

```sql
        )
        FETCH FIRST /* BV: 상위 20건 */
            20 ROWS ONLY;
```

## 11-4. MATCH_RECOGNIZE DEFINE 조건 연속행(AND/OR)이 항목 라인 frame을 무시

DEFINE 조건의 `AND`/`OR` 연속행 indent가 DEFINE 절 기준 절대 +1로 계산되어,
주석 때문에 조건 항목(`B AS ...`)이 새 줄로 밀린 경우 연속행이 항목과 같은
레벨로 충돌했다. MATCH_RECOGNIZE 괄호 안에서는 조건 항목이 시작된 현재 렌더
라인 기준 +1로 anchor하도록 수정했다(항목이 DEFINE과 같은 줄이면 기존 출력과
동일).

- 발견 위치: `test/test_open_with.sql.format.out` 460~470행
- 수정 코드: `ui/sql_editor/formatter.rs` — 조건 break fallback 앞에
  `in_match_recognize_paren()` 분기 추가(현재 라인 indent +1)

### AS-IS (test_open_with.sql)

```sql
            DEFINE
                -- [BR] B 조건: 이전보다 높고 부서 평균의 1.5배 미만
                B AS B.sal > PREV (B.sal)
                AND B.sal < (
                    /* BS: 부서 평균 서브쿼리 */
                    SELECT AVG (sal) * /* BT: 상한 배율 */
                        1.5
                    FROM emp
                    WHERE deptno = /* BU: correlated */
                        B.deptno
                )
```

`AND`가 조건 항목 `B AS ...`와 같은 레벨 → 별개 항목처럼 보임.

### TO-BE

```sql
            DEFINE
                -- [BR] B 조건: 이전보다 높고 부서 평균의 1.5배 미만
                B AS B.sal > PREV (B.sal)
                    AND B.sal < (
                        /* BS: 부서 평균 서브쿼리 */
                        SELECT AVG (sal) * /* BT: 상한 배율 */
                            1.5
                        FROM emp
                        WHERE deptno = /* BU: correlated */
                            B.deptno
                    )
```

연속 조건과 그 서브쿼리 전체가 항목 라인 기준 상대 frame으로 이동.

## 11-5. 검토했으나 수정하지 않은 항목 (의도된 상대 규칙으로 확인)

- `FROM (` / `USING (` / `WHERE EXISTS (` 서브쿼리 본문 +2·닫힘 +1 vs
  `JOIN (` / `AND EXISTS (` 본문 +1·닫힘 +0: 소유 키워드의 연속행 레벨을
  보정하는 일관된 상대 규칙으로 47개 파일 전체에서 동일하게 적용됨.
- `WHERE 식 <비교연산자> (` 서브쿼리 본문 +1: 전 파일 일관.
- `CAST(ROUND(` 등 같은 줄 다중 미닫힘 괄호의 연속행 가산 indent: 일관.
- `oracle splitter final boss test.sql`의 `PROMPT = = =`, `' 2024 - 07 - 01 '`,
  8칸 들여쓰기된 PROMPT 등은 원본 입력 자체가 그런 형태이며 포맷터는
  보존만 함(결함 아님, 원본 대조로 확인).

## 11-6. 추가된 회귀 테스트 (`format_sweep_tests`)

| 테스트 | 검증 내용 |
| --- | --- |
| `inline_case_call_argument_keeps_paren_frame_for_following_arguments` | inline CASE 인자 뒤 END/후속 인자가 괄호 frame +1 유지 |
| `mysql_declare_condition_for_clause_stays_inline` | DECLARE ... CONDITION FOR 인라인 유지 |
| `fetch_first_comment_continuation_stays_one_deeper_after_match_recognize_close` | MR 닫힘 뒤 FETCH 주석 연속행 +1 유지 |
| `match_recognize_define_condition_continuation_anchors_to_item_line` | DEFINE 조건 연속행이 항목 라인 +1에 anchor |

## 11-7. 4차 회차 검증 결과

| 검증 | 결과 |
| --- | --- |
| `.format.out` 47개 파일 / 28,589줄 전량 육안 정독 | 결함 4건 발견·수정 |
| 수정 후 스윕 before/after diff | 의도한 4개 파일·4개 지점만 변경 (test22, test23, test_open_with, test_mariadb/test5) |
| 실제 SQL 파일 자동 포맷 스윕 집계 | checked_files=47, failed_files=0 (전 파일 PASS, 비멱등·불변식 위반 0) |
| 전체 라이브러리 테스트 | 6,434 통과, 실패 0, ignored 212 |

---

# 5차 정밀 검증 회차 (2026-07-14, 4차 수정분 심층 재검증)

4차 수정(frame 상대 구조 4건) 이후 스윕을 재실행하고 `.format.out` 재검토 +
시그니처 스캔 + 적대적 프로브로 심층 검증했다. **신규 결함 0건, 포맷터 코드
변경 없음.** 회귀 방지용 멱등성 프로브 테스트 1건을 추가했다.

## 12-1. 1차 자동 검사

`formatting_sweep_all_files_generate_out_report`: checked_files=47, failed_files=0
(전 파일 PASS — 토큰 불변식·들여쓰기 단위·멱등성·재들여쓰기/붕괴/전개 프로브 포함).

## 12-2. 출력 재검토 및 시그니처 스캔 (47개 파일 / 28,588줄)

| 검사 | 대상 | 결과 |
| --- | --- | --- |
| 4차 변경 4개 파일 전체 재정독 (test22, test23, test_open_with, test_mariadb/test5) | 4 파일 | frame 정합 확인, 이상 없음 |
| inline `=> CASE` / `, CASE` / `(CASE` 발생 지점 전수 확인 | 47 파일 | 2건(test22·test23) 모두 괄호 frame +1로 정렬됨 |
| 인라인 블록주석 뒤 연속행 붕괴(+0) 스캔 | 47 파일 | 7건 검출 → 문자열 내부 2건, `WITH /* */`·`ON /* */` 5건은 CTE 이름 레벨·ON 동일깊이 명시 분기(의도된 상대 규칙) |
| AND/OR가 직전 코드행과 같은 indent인 지점 스캔 | 47 파일 | 9건 → 전부 `EXISTS(...)` 닫힘 뒤 형제 조건(확립 규칙) 또는 블록주석/문자열 내부 |
| 단독 `FOR` 행(선언 오절단 의심) 스캔 | 47 파일 | 2건 → 모두 문자열 내부 원문 |
| 3레벨 이상 indent 점프 스캔 | 47 파일 | 20건 → 전부 q-quote 문자열 경계 또는 같은 줄 다중 미닫힘 괄호 가산 규칙 |
| 단독 라인주석 anchoring 드리프트 스캔 | 47 파일 | 4건 → 블록 dedent 직전 본문 주석(표준) 2건, 문자열 내부 2건 |
| `END`/`END IF` 등 owner 정렬 스캔 | 47 파일 | 50건 → TYPE BODY의 CREATE 정렬, 문자열 내 가짜 END, MySQL `UNTIL`/`END REPEAT` 규약으로 전부 정상 |

## 12-3. 적대적 프로브 10종 (전부 PASS + 멱등)

4차 수정 경로를 더 깊은 중첩으로 공격:

1. 중첩 호출 안 inline CASE 인자 2개 (`outer_call(a => inner_call(..., y => CASE...), b => CASE...)`)
2. PACKAGE BODY → PROCEDURE → IF → FOR LOOP 내부의 inline CASE 인자 (깊은 블록 중첩)
3. 여는 괄호 직후 첫 인자 CASE
4. SQL SELECT 목록 함수 호출의 `=> CASE`
5. MariaDB `CONDITION FOR 1062`(에러코드형) + 중첩 BEGIN 내 `CONDITION FOR SQLSTATE`
6. PIVOT 닫힘 뒤 `FETCH FIRST /* 주석 */` 연속행
7. OPEN FOR + UNPIVOT 닫힘 뒤 FETCH 주석 연속행
8. MR DEFINE 다중 항목 + 항목별 주석 + AND (항목 라인 +1, 콤마 후 다음 항목 복귀)
9. MR DEFINE 조건 내부 서브쿼리의 WHERE AND (WHERE owner가 MR 규칙보다 우선함 확인)
10. MERGE USING 서브쿼리 안의 MR DEFINE AND (2중 중첩)

10종 모두 frame 상대 구조 유지 + 2회 포맷 결과 동일(멱등).
프로브는 `adversarial_frame_probes_stay_idempotent` 테스트로 영구 고정.

## 12-4. 5차 회차 검증 결과

| 검증 | 결과 |
| --- | --- |
| 스윕 자동 검사 | 47/47 PASS, 실패 0 |
| 출력 재검토·시그니처 스캔 8종 | 신규 결함 0건 |
| 적대적 프로브 10종 | 전부 PASS, 멱등 |
| 전체 라이브러리 테스트 | 6,435 통과, 실패 0, ignored 212 |
| `cargo fmt -- --check` | 통과 |
| 포맷터 코드 변경 | 없음 (검증 전용 회차) |

---

# 6차 독립 정밀 검증 회차 (2026-07-14, 전수 육안 정독 + frame 구조 감사)

기존 PASS 보고나 5차 기록을 결론의 근거로 재사용하지 않고, 지정 스윕을 새로
실행한 뒤 생성된 모든 `.format.out`을 1행부터 EOF까지 직접 정독했다. 이어서
포맷터의 frame open/close 및 상대 depth 복원 구현과 관련 회귀 테스트를 다시
감사했다. **신규 포맷 결함 0건, 포맷터 코드 수정 0건**이다.

## 13-1. 지정 스윕 및 출력 전수 육안 검토

실행 명령:

```text
cargo test --lib formatting_sweep_all_files_generate_out_report -- --ignored --nocapture
```

| 대상 | 파일 수 | 처음부터 끝까지 확인한 줄 수 | 결과 |
| --- | ---: | ---: | --- |
| `target/format-sweep/test/*.format.out` (Oracle) | 39 | 22,087 | 신규 결함 0 |
| `target/format-sweep/test_mysql/*.format.out` (MySQL) | 3 | 1,827 | 신규 결함 0 |
| `target/format-sweep/test_mariadb/*.format.out` (MariaDB) | 5 | 4,674 | 신규 결함 0 |
| **합계** | **47** | **28,588** | **47/47 PASS, 실패 0** |

PASS/`issues=0` 표시는 참고만 하고 다음 항목을 실제 출력 줄에서 확인했다.

- 괄호·CASE·CTE·인라인 뷰·분석 함수·MODEL·MATCH_RECOGNIZE·JSON_TABLE의
  소유 frame 시작, 중첩 frame 종료, 부모/형제 절 depth 복귀
- PL/SQL 및 MySQL/MariaDB 루틴의 BEGIN/END, IF/CASE/LOOP, handler,
  `DELIMITER` 경계와 statement boundary 복귀
- 주석, 일반 문자열, Oracle q-quote, 동적 SQL 내부의 가짜 괄호·세미콜론·END가
  외부 frame을 열거나 닫지 않는지
- 괄호 닫힘 뒤 후속 인자, `FETCH FIRST` 주석 연속행, DEFINE 조건의 AND/OR,
  `DECLARE ... CONDITION FOR` 등 기존 결함 지점

최종 스윕 재생성 후 47개 파일의 SHA-256을 육안 검토 직전 결과와 비교했으며
`BASELINE=47 CURRENT=47 HASH_DIFFS=0`이었다. 따라서 최종 산출물은 전수 정독한
산출물과 바이트 단위로 동일하다.

## 13-2. frame open/close 및 상대 depth 구현 감사

구현은 전역 depth를 문맥 없이 재계산하는 방식이 아니라 다음 frame stack을
단일 구조 상태로 사용한다.

- `FormatFrameStack`은 Paren, Block, QueryRuntime, ScopedIndent,
  ConditionOwner 등 구문 frame을 한 스택에서 관리한다.
- 괄호 시작은 `push_paren`에서 고유 frame ID, 부모 frame ID, 상대 indent delta,
  쿼리 runtime 상태를 함께 저장한다. 종료는 `pop_paren`에서 정확히 해당 frame을
  제거하고 부모 frame의 metrics/runtime/indent를 복원한다.
- 블록 시작은 owner depth와 body indent를 `FormatBlockDepthFrame`에 저장하고,
  종료는 `pop_block`이 남아 있는 frame stack에서 기대 indent를 재구성한다.
- `FormatScope`는 depth뿐 아니라 frame ID까지 비교하므로 같은 depth의 형제
  구문을 부모/자식으로 오인하지 않는다. scope를 벗어난 보조 frame은 token 진입
  및 statement boundary에서 만료된다.
- debug invariant는 paren/block depth, 최근 frame ID, `indent_level`과 stack의
  기대값이 어긋나면 즉시 실패시킨다.

확인한 핵심 구현 위치:

- `src/ui/sql_editor/formatter.rs`: frame 종류/stack(1883~2006), 공통 push/pop
  (2106~2377), 괄호 push/pop(2754~2861), 블록 push/pop(3140~3224), statement
  boundary 및 invariant(3835~4018)
- `src/sql_format.rs`: frame ID를 포함한 동일-depth scope 판정(103~127)

결론: 구문 시작이 frame을 만들고 구문 종료가 그 frame을 닫으며, 중첩 종료 후
부모 기준 상대 depth로 복귀하는 요구 구조가 구현되어 있고 이번 전수 출력에서도
frame 누수나 형제 scope 오인이 발견되지 않았다.

## 13-3. 회귀 및 전체 검증 결과

| 검증 | 결과 |
| --- | --- |
| `cargo test --lib format_frame_stack -- --nocapture` | 33 통과, 실패 0 |
| `cargo test --lib ui::sql_editor::format_sweep_tests:: -- --nocapture` | 13 통과, 실패 0, 스윕용 ignored 2 |
| 적대적 frame 프로브 10종 | 전부 2회 포맷 동일(멱등), 실패 0 |
| `cargo test --lib` | 6,435 통과, 실패 0, ignored 212 |
| `cargo fmt -- --check` | 통과 |
| 지정 ignored 스윕 최종 재실행 | 1 통과, checked_files=47, failed_files=0 |
| 최종 출력 vs 전수 정독본 SHA-256 | 47/47 동일, diff 0 |

## 13-4. AS-IS / TO-BE

이번 회차에는 새 포맷 결함과 코드 수정이 없으므로 신규 AS-IS/TO-BE 쿼리
차이는 없다.

- **AS-IS:** 5차 검증 완료 상태의 47개 포맷 출력
- **TO-BE:** 6차 최종 스윕 출력 — AS-IS와 SHA-256 47/47 동일
- 실제 수정이 있었던 4개 쿼리의 AS-IS/TO-BE는 11-1~11-4에 각각 보존되어
  있으며, 이번 회차에서 해당 TO-BE 지점과 그 주변 중첩 frame을 다시 정독했다.

## 13-5. 6차 회차 결론

| 항목 | 결과 |
| --- | --- |
| 전수 확인 파일/라인 | 47개 / 28,588줄 |
| 새로 발견한 오류 | 0건 |
| 포맷터·테스트 코드 수정 | 없음 |
| 문서 변경 | 본 6차 독립 검증 기록 추가 |
| 최종 상태 | 스윕·집중 회귀·전체 테스트·fmt 전부 성공 |

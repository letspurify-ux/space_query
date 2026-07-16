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

---

# 7차 최신 fixture 독립 검증 회차 (2026-07-14, 53개 전수 정독)

6차 기록의 47개 결과를 결론으로 재사용하지 않고 지정 스윕을 다시 실행했다.
그 사이 추가된 MySQL 3개·MariaDB 3개 fixture를 포함해 생성된 모든
`.format.out`을 1행부터 EOF까지 직접 정독하고, frame open/close 상대 depth
구현과 실제 토큰 처리 경로를 다시 감사했다. **신규 포맷 결함 0건, 포매터·테스트
코드 수정 0건**이다.

## 14-1. 지정 스윕 및 출력 전수 육안 검토

실행 명령:

```text
cargo test --lib formatting_sweep_all_files_generate_out_report -- --ignored --nocapture
```

| 대상 | 파일 수 | 처음부터 끝까지 확인한 줄 수 | 결과 |
| --- | ---: | ---: | --- |
| `target/format-sweep/test/*.format.out` (Oracle) | 39 | 22,089 | 신규 결함 0 |
| `target/format-sweep/test_mysql/*.format.out` (MySQL) | 6 | 3,689 | 신규 결함 0 |
| `target/format-sweep/test_mariadb/*.format.out` (MariaDB) | 8 | 6,363 | 신규 결함 0 |
| **합계** | **53** | **32,141** | **53/53 PASS, 실패 0** |

PASS 표시는 결론의 근거로 대신하지 않고, 실제 출력에서 괄호·블록·CASE·CTE·
서브쿼리·JSON/XML table function·MATCH_RECOGNIZE·루틴/handler의 시작 frame,
중첩 종료, 부모와 다음 형제 depth 복귀를 확인했다. 주석·일반 문자열·Oracle
q-quote·동적 SQL 안의 가짜 괄호/세미콜론/END가 바깥 frame을 변경하지 않는지도
함께 확인했다.

보조 검사에서는 실제 `[[FMT:E숫자...]]` 오류 마커 0개, `status: PASS` 53개,
`issues: total=0` 53개, 집계 `checked_files=53 / failed_files=0`이었다. 최종
스윕 재생성 후 육안 정독 직전 보관본과 `diff -qr`로 비교했으며 파일 추가·삭제·
내용 차이가 모두 0이었다. 따라서 최종 53개·32,141줄은 정독한 산출물과 바이트
단위로 동일하다.

## 14-2. frame open/close 상대 depth 구현 재감사

- `FormatFrameStack`이 Paren·Block·QueryRuntime·ScopedIndent·ConditionOwner를
  한 스택에서 관리하고, 괄호와 블록 시작마다 고유 frame ID와 부모 frame ID를
  기록한다.
- 실제 토큰 루프는 토큰 진입마다 현재 frame ID를 포함한 scope를 동기화한다.
  `(`는 `push_paren`에서 상대 indent delta와 runtime을 함께 열고, `)`는
  `pop_paren`에서 같은 frame의 delta를 되돌린 뒤 부모 상태를 복원한다.
- 블록은 owner/body depth를 `FormatBlockDepthFrame`에 저장하며, `pop_block`은
  닫힌 뒤 살아남은 frame stack에서 기준 indent를 재구성한다.
- `FormatScope::contains`는 depth뿐 아니라 frame ID도 대조하므로 동일 depth의
  형제 frame을 부모/자식으로 오인하지 않는다. debug invariant와 statement
  boundary 정리도 stack의 기대 indent 및 최근 frame ID를 검증한다.

이번 53개 전수 출력과 집중 테스트에서 frame 누수, 중첩 close 후 외부 depth
손실, 동일 depth 형제 scope 오인은 재현되지 않았다. 따라서 단순 depth 보정
코드를 추가하는 것보다 현재 frame 기반 구현을 그대로 유지하는 것이 최소·정확한
조치라고 판단했다.

## 14-3. AS-IS / TO-BE query

이번 회차에는 수정할 결함이 없으므로 AS-IS와 TO-BE의 SQL 및 출력이 동일하다.
아래 중첩 query는 바깥 `MATCH_RECOGNIZE` frame 안에서 서브쿼리 frame이 열리고
닫힌 뒤 바깥 `DEFINE`과 괄호 depth로 정확히 복귀하는 대표 확인 사례다.

### AS-IS

```sql
SELECT *
FROM e
MATCH_RECOGNIZE (
    PARTITION BY d
    ORDER BY rn
    PATTERN (A B+)
    DEFINE B AS B.sal > (
        SELECT AVG (x.sal)
        FROM e x
        WHERE x.d = B.d
            AND x.flag = 'Y'
    )
);
```

### TO-BE

```sql
SELECT *
FROM e
MATCH_RECOGNIZE (
    PARTITION BY d
    ORDER BY rn
    PATTERN (A B+)
    DEFINE B AS B.sal > (
        SELECT AVG (x.sal)
        FROM e x
        WHERE x.d = B.d
            AND x.flag = 'Y'
    )
);
```

즉, 이번 회차의 변경 내역은 `AS-IS = TO-BE`이며 프로덕션 SQL 포매터 diff는
없다. 실제 과거 수정 쿼리의 차이는 11-1~11-4에 계속 보존한다.

## 14-4. 회귀 및 전체 검증 결과

| 검증 | 결과 |
| --- | --- |
| `cargo test --lib format_frame_stack -- --nocapture` | 33 통과, 실패 0 |
| `cargo test --lib sql_format_ -- --nocapture` | 8 통과, 실패 0 |
| 인라인 CASE 후속 인자 frame 회귀 | 1 통과, 실패 0 |
| 적대적 frame query 10종 | 전부 2회 포맷 동일(멱등), 실패 0 |
| `cargo test --lib` | 6,466 통과, 실패 0, ignored 218 |
| `cargo fmt -- --check` | 통과 |
| 지정 ignored 스윕 최초·최종 실행 | 각 1 통과, checked_files=53, failed_files=0 |
| 최초 산출물 vs 최종 산출물 `diff -qr` | 차이 0 |

## 14-5. 7차 회차 결론

| 항목 | 결과 |
| --- | --- |
| 전수 확인 파일/라인 | 53개 / 32,141줄 |
| 새로 발견한 오류 | 0건 |
| 포매터·테스트 코드 수정 | 없음 |
| 문서 변경 | 본 7차 독립 검증 기록 추가 |
| 최종 상태 | 스윕·집중 회귀·전체 테스트·fmt 전부 성공 |

# 15. 8차 검증 회차 (2026-07-14): 전수 육안 재검토 및 frame depth 결함 2건 수정

## 15-1. 수행 내용

1. `cargo test --lib formatting_sweep_all_files_generate_out_report -- --ignored --nocapture` 1차 실행
   → checked_files=53, failed_files=0 (자동 검사 전건 PASS)
2. `target/format-sweep` 아래 53개 `.format.out` 전체(32,141줄)를 처음부터 끝까지 육안 검토.
   자동 PASS를 신뢰하지 않고 frame open/close 상대 depth 일관성(목표3) 기준으로 전 라인 대조.
   보조로 인덴트 급증(±2레벨 이상)·4배수 위반 지점을 스캐너로 표시하고 전 지점을 원문 대조로 판정.
3. 육안 검토에서 자동 검사(토큰 보존·4배수·멱등성)가 잡지 못하는 상대 depth 결함 2건 + 의도 설계 확인 1건 발견.
4. 결함 2건을 최소 SQL로 재현 → `formatter.rs` 수정 → 회귀 테스트 2건 추가 → 스윕 재실행 후 산출물 재검토.

## 15-2. 오류 1: 여러 줄 서브쿼리 인자 닫힘 후 후속 인자 depth 이탈

- 발견 위치: `target/format-sweep/test_mariadb/test10.txt.format.out` 211~218행
  (`sq_mariadb_format_cert_2`의 `mb_run_gauntlet_2` 내 INSERT … JSON_OBJECT)
- 증상: 같은 줄에서 여러 괄호 프레임이 열린 상태(`VALUES ( … JSON_OBJECT ( … (`)에서
  인라인 서브쿼리가 여러 줄로 펼쳐진 뒤, 닫는 `)` 줄에 이어지는 인자는 프레임 depth를 유지하지만
  줄바꿈된 후속 인자(`v_forward` 등)는 "여는 줄 indent+1"로 계산되어 한 프레임 바깥 depth로 떨어짐.
  같은 JSON_OBJECT 인자 목록의 형제끼리 depth가 어긋나는 frame 계약 위반.
- 원인: `ParenFormatFrame::sibling_body_indent()`가 `body_indent` 미기록 시
  `open_line_indent + 1`을 기본값으로 사용. 여는 줄에서 프레임이 2개 이상 열리면
  실제 프레임 depth(닫는 줄 depth)보다 얕게 계산됨.
- 수정: `formatter.rs`의 `)` 처리에서 여러 줄 자식 프레임이 닫힐 때
  (`contributes_multiline_close && closes_indented`) 닫는 줄 indent를 부모 프레임의
  `record_body_indent`로 기록. 이후 콤마 줄바꿈 형제는 닫는 줄과 같은 depth로 정렬.

AS-IS (수정 전 포맷 결과):

```sql
        VALUES (summary_rec.asset_id, ..., JSON_OBJECT('uuid', (
                    SELECT CAST(asset_uuid AS CHAR)
                    FROM mb_asset
                    WHERE asset_id = summary_rec.asset_id
                ), 'forward_sum',
            v_forward,
            'reverse_sum',
            v_reverse))
```

TO-BE (수정 후 포맷 결과):

```sql
        VALUES (summary_rec.asset_id, ..., JSON_OBJECT('uuid', (
                    SELECT CAST(asset_uuid AS CHAR)
                    FROM mb_asset
                    WHERE asset_id = summary_rec.asset_id
                ), 'forward_sum',
                v_forward,
                'reverse_sum',
                v_reverse))
```

- 회귀 테스트: `format_for_auto_formatting_aligns_wrapped_siblings_with_multiline_subquery_close_line`
  (정렬 + 멱등성 고정)

## 15-3. 오류 2: 서브쿼리가 CASE…END로 끝나면 `) LOOP` 가 두 줄로 분리

- 발견 위치: `test/test21.sql.format.out` 46~47행, `test/oracle_format_ultimate_boss.sql.format.out` 32~33행
  (커서 FOR 루프의 IN 서브쿼리가 `ORDER BY CASE … END`로 끝나는 경우)
- 증상: 전 파일에서 `) LOOP`는 한 줄이 규범(기존 테스트
  `plsql_for_split_in_subquery_keeps_child_query_on_for_depth`로 고정)인데,
  서브쿼리 마지막 단어가 `END`이면 `)`와 `LOOP`가 줄 분리됨.
- 원인: `FOR i IN 1..CASE … END LOOP`(범위식이 CASE로 끝나는 경우)용
  `follows_for_range_case_end` 판정이 "직전 단어 == END"만 확인해서,
  END와 LOOP 사이에 `)`가 있는 경우까지 오탐.
- 수정: 직전 비주석 토큰이 실제 `END` 단어일 때만 줄 분리하도록 조건 강화.
  (`)` 뒤의 LOOP는 기존 규범대로 같은 줄 유지, 범위식 CASE END 직후의 LOOP는 기존대로 분리)

AS-IS (수정 전 포맷 결과):

```sql
    FOR r IN (
        SELECT object_name
        FROM user_objects
        ORDER BY
            CASE object_type
                WHEN 'VIEW' THEN
                    1
                ELSE
                    2
            END
    )
    LOOP
```

TO-BE (수정 후 포맷 결과):

```sql
    FOR r IN (
        SELECT object_name
        FROM user_objects
        ORDER BY
            CASE object_type
                WHEN 'VIEW' THEN
                    1
                ELSE
                    2
            END
    ) LOOP
```

- 회귀 테스트: `plsql_for_in_subquery_ending_with_case_end_keeps_loop_on_close_paren_line`
  (`) LOOP` 결합 + 멱등성 고정)

## 15-4. 의도 설계 확인 (수정하지 않음)

- Oracle `JSON_OBJECT (… 'k' VALUE ( 서브쿼리 ), …)`에서 닫는 `)` 줄이 서브쿼리 본문과
  같은 depth에 놓이는 레이아웃(`test/oracle_format_ultimate_boss.sql` QUERY 8,
  `test/oracle splitter final boss test.sql` TEST-038)은 결함이 아니라
  `query_like_paren_layout`의 `ends_with_value` 분기(`close_depth = child_head_depth`)로
  구현된 의도된 설계이며, 회귀 테스트
  `format_for_auto_formatting_keeps_json_object_value_scalar_subquery_on_paren_frame_depth`가
  해당 레이아웃(후속 `'k' VALUE …` 인자가 닫는 괄호 줄에서 프레임 depth로 이어지는 형태)을 고정하고 있어 유지.
- WITH 절에서 인라인 `FUNCTION`/`PROCEDURE` 선언은 +1 depth, CTE 이름은 `WITH`와 동일 depth로
  정렬되는 형제 depth 차이는 전 파일에서 일관 적용되는 스타일로 확인(오류 아님).
- 문자열/주석/q-quote 내부 보호 구간(멀티라인 리터럴의 비정형 indent)은 전부 원문 보존으로 정상.

## 15-5. 반복 및 최종 검증 결과

| 검증 | 결과 |
| --- | --- |
| 스윕 1차(수정 전) | checked_files=53, failed_files=0 |
| 육안 전수 검토 | 53개 파일 / 32,141줄, 결함 2건·설계 확인 1건 |
| 수정 후 스윕 재실행 | checked_files=53, failed_files=0 |
| 수정 전후 산출물 diff | 의도한 3개 지점(test_mariadb/test10, test/test21, test/oracle_format_ultimate_boss)만 변경, 그 외 50개 파일 diff 0 |
| 재검토(변경 지점 육안 확인) | 형제 인자 depth 정렬·`) LOOP` 결합 정상, 잔여 오류 0건 |
| `cargo test --lib` | 6,468 통과(신규 회귀 2건 포함), 실패 0, ignored 218 |
| `cargo fmt -- --check` | 통과 |

## 15-6. 8차 회차 결론

| 항목 | 결과 |
| --- | --- |
| 전수 확인 파일/라인 | 53개 / 32,141줄 |
| 새로 발견한 오류 | 2건 (전건 수정 완료) |
| 포매터 수정 | `formatter.rs` 2곳 (`)` 닫힘 시 부모 body indent 기록, LOOP 줄바꿈 판정 강화) |
| 회귀 테스트 추가 | 2건 |
| 최종 상태 | 스윕·전체 테스트·fmt 전부 성공, 잔여 오류 0건 |

# 16. 9차 검증 회차 (2026-07-15): 3개 DB 최종 보스 실실행·포맷·IntelliSense 판매 게이트

## 16-1. 신규 단일 문장 fixture

자동 포맷터가 단순 예제뿐 아니라 실제 판매 전 품질 게이트로 사용할 수 있도록,
부작용 없이 반복 실행 가능한 `WITH ... SELECT` 한 문장을 DB별로 추가했다.

| DB | 파일 | 원본 줄 수 | 주요 문법 |
| --- | --- | ---: | --- |
| Oracle | `test/oracle_format_final_boss_2.sql` | 209 | 재귀 CTE `CYCLE`, 계층 질의, `JSON_TABLE`/`NESTED PATH`, `APPLY`, `MATCH_RECOGNIZE`, `PIVOT`, 집합 연산, 분석 함수, JSON/XML 생성 |
| MySQL | `test_mysql/test7.txt` | 186 | 재귀 CTE, `JSON_TABLE`/`NESTED PATH`, `LATERAL`, `INTERSECT`/`EXCEPT`, `WITH ROLLUP`/`GROUPING`, named window, 중첩 JSON |
| MariaDB | `test_mariadb/test12.txt` | 174 | 재귀 CTE, `JSON_TABLE`, dynamic column, vector 함수, `PERCENTILE_DISC`, `INTERSECT ALL`/`EXCEPT ALL`, 중첩 JSON |

각 쿼리는 내부 데이터 집합의 건수·합계·트리 노드·집합 연산 결과를 최종
`CASE`에서 다시 검산한다. 성공 조건은 결과 4행의 `status`가 모두 `PASS`인
것이며, DDL/DML이나 세션 상태 변경은 포함하지 않는다.

## 16-2. 실제 DB 원본·포맷 결과 실행

원본뿐 아니라 `target/format-sweep`에 생성된 `.format.out` 파일 자체(후미의
SQL 주석 보고서 포함)를 컨테이너로 복사해 동일 엔진에서 다시 실행했다.

| 엔진 | 원본 | 포맷 결과 | 판정 |
| --- | --- | --- | --- |
| Oracle Database Free 26ai | exit 0, `PASS` 4행 | exit 0, `PASS` 4행 | 성공 |
| MySQL 8.0.46 | exit 0, `PASS` 4행 | exit 0, `PASS` 4행 | 성공 |
| MariaDB 12.2.2 | exit 0, `PASS` 4행 | exit 0, `PASS` 4행 | 성공 |

따라서 이번 fixture에서 자동 포맷 전후의 실행 의미와 자체 검산 결과는 같다.
각 엔진의 원본/포맷 stdout 전체를 별도 파일로 저장해 `diff -u`한 결과도 세 DB
모두 차이 0이었다.

## 16-3. 포맷 스윕·육안 검토·결정성

| 항목 | 결과 |
| --- | --- |
| 전체 스윕 | 56개 파일, failed_files=0 |
| frame 검사 | checked_frames=8,103, body_items=565, closes=1,393 |
| 전체 `.format.out` | 33,330줄 |
| 신규 3개 결과 | 1,191줄(Oracle 394, MySQL 410, MariaDB 387), 처음부터 끝까지 육안 검토 완료 |
| 기존 53개 결과 | 직전 전수 육안 검토 최종 스냅샷과 개별 파일 byte diff 0 |
| 최종 재생성 결정성 | 재실행 전후 `diff -rq` 차이 0 |

신규 결과에서 토큰 손실, frame 누수, 비정상 close depth, 절 경계 오인 또는
실행 불가능한 SQL은 발견되지 않았다. 포매터 구현 자체에는 추가 결함이 없어
이번 회차의 프로덕션 포매터 코드는 수정하지 않았다.

## 16-4. IntelliSense AS-IS / TO-BE

신규 파일을 실제 `intellisense_sweep_generate_report_for_file` 경로로 검사했을 때
최초 누락은 Oracle 53개, MySQL 7개, MariaDB 2개였다. 대부분은 새 문법의 구조
키워드였고, Oracle의 다수 누락은 아래 하나의 CTE 상태 전이 결함에서 파생됐다.

### AS-IS: Oracle `CYCLE` 뒤의 다음 CTE를 잃음

```sql
WITH days (day_no, day_value) AS (
    SELECT 0, DATE '2026-01-01' FROM dual
)
CYCLE day_no SET cycle_yn TO 'Y' DEFAULT 'N',
customer_tree (customer_id) AS (
    SELECT 1 FROM dual
)
SELECT t.customer_id
FROM customer_tree t;
```

기존 단일 패스 CTE 상태기는 MariaDB의 `CYCLE ... RESTRICT,`만 다음 CTE
구분자로 복구했다. Oracle의 `SEARCH ... SET ... ,` 또는
`CYCLE ... SET ... TO ... DEFAULT ... ,` 뒤 쉼표는 처리하지 않아
`customer_tree` 이후 CTE 정의와 명시적 컬럼을 전부 잃었다.

### TO-BE: 완결된 재귀 CTE 옵션의 쉼표만 구분자로 인식

`recursive_cte_option_is_complete_before_separator`가 다음을 구분한다.

- `SEARCH ... SET generated_column`이 완결된 뒤의 쉼표
- `CYCLE ... SET marker TO value DEFAULT value`가 완결된 뒤의 쉼표
- `SEARCH ... BY a, b`와 `CYCLE a, b SET ...` 내부 쉼표(CTE 구분자가 아님)

Oracle의 후속 CTE가 다시 scope에 등록되어 명시적 컬럼과 qualified column
완성이 복원됐다. 내부 쉼표 오탐 방지까지 비무시 회귀 테스트로 고정했다.

구조 키워드 추론도 다음 실제 구문에 맞게 보강했다.

- Oracle: `UNION ALL`, `CROSS JOIN/APPLY`, `OUTER APPLY`,
  `MATCH_RECOGNIZE`의 `ONE ROW`, `AFTER MATCH SKIP PAST LAST ROW`,
  `XMLSERIALIZE CONTENT`, JSON `VALUE`/`FORMAT JSON`, select-list `AS`
- MySQL/MariaDB 공통: `JSON_TABLE ... NESTED PATH`, `WITH ROLLUP`,
  `IS [NOT] NULL/TRUE/FALSE`
- MySQL: `LATERAL (subquery)`
- prefix 충돌: `ONE`을 `ON`보다, `IS`를 `INTO`보다 먼저 suffix 구조로 판정
- 스윕 분류: `PATTERN` 변수와 quoted `XMLELEMENT` 태그는 참조 키워드가 아닌
  사용자 정의 이름 슬롯으로 제외

## 16-5. 파일별 IntelliSense 인증

세 보고서는 입력 파일 옆에 저장하며 누락이 있으면 테스트가 실패한다.

| 보고서 | checked | missing |
| --- | ---: | ---: |
| `test/oracle_format_final_boss_2.sql.out` | 576 | 0 |
| `test_mysql/test7.txt.out` | 530 | 0 |
| `test_mariadb/test12.txt.out` | 523 | 0 |

개별 ignored 테스트 3개와 통합 테스트
`intellisense_sweep_generate_report_for_file_certifies_new_final_boss_queries`를 추가했다.
통합 테스트는 세 파일 모두에 `fail_on_missing=true`를 전달하며 최종 실행에서
1건 통과, 실패 0이었다. 대표 구조 키워드 8종은 별도 비무시 테스트
`final_boss_structural_keywords_use_production_completion`으로도 고정했다.

## 16-6. 최종 회귀 결과

| 검증 | 결과 |
| --- | --- |
| `cargo check --lib` | 통과 |
| Oracle 재귀 CTE 경계 회귀 | 2 통과, 실패 0 |
| 신규 구조 키워드 production completion 회귀 | 1 통과(8 case), 실패 0 |
| `cargo test --lib format_frame_stack -- --nocapture` | 33 통과, 실패 0 |
| `cargo test --lib sql_format_ -- --nocapture` | 8 통과, 실패 0 |
| 통합 IntelliSense 파일 스윕 | 1 통과, Oracle/MySQL/MariaDB 모두 missing 0 |
| 전체 포맷 스윕 | 1 통과, 56개 파일·failed_files=0 |
| `cargo test --lib -- --test-threads=1` 최종 재실행 | 6,474 통과, 실패 0, ignored 222 |
| `cargo fmt -- --check` | 통과 |

병렬 전체 실행에서는 기존 비동기 wildcard 테스트 1건이 스케줄링 부하에 따라
간헐적으로 `Timeout`을 냈다. 해당 테스트 단독 실행은 0.05초에 통과했고 병렬
전체 재실행도 한 차례 실패 0으로 통과했다. 최종 판정은 스케줄링 영향을 제거한
단일 테스트 스레드로 전체 6,696개를 다시 실행해 실패 0을 확인했다.

## 16-7. 결론

세 DB의 새 최종 보스 쿼리는 원본·자동 포맷 결과 모두 실제 엔진에서 실행되며,
자체 검산 4행이 모두 `PASS`다. 56개 전체 포맷 스윕, 신규 결과 육안 검토,
결정성 비교, 파일별 IntelliSense 전수 검사, 전체 Rust 회귀를 모두 통과했다.
이번 회차에서 발견된 IntelliSense scope/구조 키워드 결함은 회귀 테스트와 함께
수정됐고, 포매터의 잔여 오류는 발견되지 않았다.

# 17. 10차 검증 회차 (2026-07-15): Final Boss III/V와 frame-relative 상용 품질 재인증

## 17-1. 신규 DB별 단일 문장 fixture

기존 final-boss 세트에서 상대적으로 약했던 문법을 실제 엔진에서 함께 조합하기 위해
DB별로 반복 실행 가능하고 부작용이 없는 `WITH ... SELECT` 한 문장을 더 추가했다.
상호 배타적인 문법까지 한 문장에 억지로 섞는 대신, 기존 전체 fixture와 이번 3개를
합친 스윕이 현재 제품이 지원하는 구조·키워드 조합을 모두 지나도록 구성했다.

| DB | 파일 | 원본 줄 수 | 이번 문장의 핵심 조합 |
| --- | --- | ---: | --- |
| Oracle | `test/oracle_format_final_boss_3.sql` | 192 | `WITH FUNCTION`, 복수 재귀 CTE, 연속 `SEARCH`/`CYCLE`, 중첩 `JSON_TABLE`, `GROUPING SETS`, `PIVOT`/`UNPIVOT`, 분석 window/frame, `KEEP`, `LISTAGG ... ON OVERFLOW`, XML/JSON 생성 |
| MySQL | `test_mysql/test8.txt` | 195 | 재귀 CTE, 중첩 `JSON_TABLE`, JSON schema/predicate, `LATERAL`, `INTERSECT`/`EXCEPT`, `ROLLUP`, named window, 정규식, 다중 중첩 JSON |
| MariaDB | `test_mariadb/test13.txt` | 153 | 재귀 `CYCLE ... RESTRICT`, 중첩 `JSON_TABLE`, dynamic column, `INET6`, percentile 분석, `INTERSECT ALL`/`EXCEPT ALL`, ordered/limited JSON aggregate |

세 파일 모두 내부 fixture의 트리·집계·집합 연산·JSON 결과를 마지막 `CASE`에서
재검산한다. 원본과 자동 포맷 결과를 각각 Oracle Database Free 26ai,
MySQL 8.0.46, MariaDB 12.2.2 컨테이너에서 실행했으며 모두 exit code 0,
모든 반환 행의 `status = 'PASS'`를 확인했다.

## 17-2. 포매터 AS-IS / TO-BE: `SEARCH/CYCLE` 뒤 CTE body indent 복귀

이번 Oracle 문장을 최소화한 회귀 테스트에서, `WITH FUNCTION` 뒤 재귀 CTE의
`SEARCH`/`CYCLE` option tail이 끝난 쉼표를 일반 절 내부 쉼표로 취급하면 다음 CTE가
직전 option clause의 continuation depth를 물려받을 수 있음을 재현했다. 이는
"쉼표 다음 항목은 쉼표를 소유한 목록 frame의 고정 `body_indent`로 복귀"한다는
계약과 같은 `WITH` frame 최상위 CTE 형제의 depth 계약을 동시에 위반한다.

AS-IS (수정 전의 잘못된 상대 depth):

```sql
WITH
    FUNCTION f(p NUMBER) RETURN NUMBER IS
    BEGIN
        RETURN p;
    END;
first_r (n) AS (
    ...
)
CYCLE n SET first_cycle TO 'Y' DEFAULT 'N',
    second_r (n) AS (          -- 잘못: CYCLE continuation depth 상속
        ...
    )
    CYCLE n SET second_cycle TO 'Y' DEFAULT 'N',
        tail_cte (n) AS (      -- 잘못: 다음 형제에서 다시 depth 누적
            ...
        )
SELECT n FROM tail_cte;
```

TO-BE (수정 후 owning `WITH` frame의 고정 body depth):

```sql
WITH
    FUNCTION f(p NUMBER) RETURN NUMBER IS
    BEGIN
        RETURN p;
    END;
first_r (n) AS (
    ...
)
CYCLE n SET first_cycle TO 'Y' DEFAULT 'N',
second_r (n) AS (
    ...
)
CYCLE n SET second_cycle TO 'Y' DEFAULT 'N',
tail_cte (n) AS (
    ...
)
SELECT n FROM tail_cte;
```

수정 내용:

- 쉼표 뒤 토큰을 bounded look-ahead하여 `name [(column-list)] AS (` 형태의 완전한
  다음 CTE 정의인지 확인한다.
- 같은 괄호 depth의 활성 `SEARCH/CYCLE` tail 뒤에서만 현재 `WITH` body indent를
  캡처하고 option construct를 닫은 뒤 그 고정 indent로 개행한다.
- `SEARCH ... BY a, b`, `CYCLE a, b SET ...` 내부 쉼표나 일반 함수/목록 쉼표에는
  적용하지 않는다.
- 회귀 테스트
  `format_sql_recursive_cte_cycle_separator_restores_with_body_indent`가 첫째·둘째·마지막
  CTE 이름의 indent 동일성과 두 번 포맷한 결과의 멱등성을 함께 고정한다.

## 17-3. 전체 `.format.out` 수동 판독

정확한 명령
`cargo test --lib formatting_sweep_all_files_generate_out_report -- --ignored --nocapture`로
산출물을 다시 만든 뒤, `target/format-sweep` 아래 모든 `.format.out`을 파일별 line
number와 함께 처음부터 끝까지 직접 판독했다. 자동 PASS나 통계만으로 판정하지 않았다.

| 항목 | 결과 |
| --- | ---: |
| 전체 `.format.out` | 59개 |
| 직접 읽은 전체 줄 | 34,382줄 |
| 신규 3개 포맷 결과 | 1,051줄(Oracle 354, MySQL 374, MariaDB 323) |
| comma sibling body-depth 위반 | 0건 |
| 동일 paren frame 최상위 항목 depth 위반 | 0건 |
| close-indent 및 닫힘 뒤 parent body 복귀 위반 | 0건 |

판독 중 별도 표시했던 두 패턴도 구현과 전용 회귀 테스트까지 대조했다.

- `test/test24.sql`의 주석 다음 선행 쉼표는 쉼표 문자와 첫 `SET` 항목이 모두 같은
  column 12의 고정 SET-list body indent이므로 정상이다.
- Oracle `JSON_OBJECT ('k' VALUE (subquery))`의 긴 inline frame은
  `query_like_paren_layout`의 의도된 close-depth이며
  `format_for_auto_formatting_keeps_json_object_value_scalar_subquery_on_paren_frame_depth`가
  해당 계약을 고정한다.

따라서 위 `SEARCH/CYCLE` 결함 수정 이후에는 추가 production formatter 변경이
필요한 실제 위반이 남지 않았다.

## 17-4. IntelliSense AS-IS / TO-BE와 production 경로 일치

`intellisense_sweep_generate_report_for_file`의 신규 세 파일을 최초 실행했을 때
실제로 추천 가능한 누락을 최소 재현으로 분리한 뒤 production main path를 수정했다.

AS-IS:

```sql
-- SEARCH가 생성한 column 뒤 바로 CYCLE이 오면 recursive-column phase를 잃음
WITH r(n) AS (...)
SEARCH DEPTH FIRST BY n SET search_order
CYCLE n SET cycle_yn TO 'Y' DEFAULT 'N'
SELECT * FROM r;

-- 큰 WITH 문장의 중첩 grammar에서 broad classifier가 exact continuation보다 먼저 종료
SELECT LISTAGG(name, ',' ON OVER|) FROM emp;
SELECT SUM(amount) OVER (ORDER BY day_no ROW| BETWEEN ...) FROM sales;

-- WITH FUNCTION 뒤 CTE 이름을 로컬 declaration으로 분류하지 못함
WITH FUNCTION f RETURN NUMBER IS BEGIN RETURN 1; END;
base AS (SELECT 1 id FROM dual)
SELECT * FROM base;
```

TO-BE:

- 완결된 Oracle `SEARCH ... SET generated_column` 다음의 `CYCLE`이 recursive CTE
  column phase를 다시 열어, 연속 `SEARCH -> CYCLE -> 다음 CTE` scope를 유지한다.
- `completion.rs` production 경로에서 bounded statement token을 사용한 exact
  window/LISTAGG grammar를 broad statement/CTE classifier보다 먼저 판정한다.
- `LISTAGG`의 `ON -> OVERFLOW -> ERROR|TRUNCATE -> WITH|WITHOUT -> COUNT` 전 체인을
  열린 `LISTAGG(` frame 안에서만 추천하며 다른 함수로 누출하지 않는다.
- token span을 한 번 순회해 괄호 짝을 O(n)에 만들고, `name [(columns)] AS (` 형태의
  CTE/window declaration을 수집하여 `WITH FUNCTION` 뒤 CTE 이름도 정의로 분류한다.
- MariaDB 전용 `COLUMN_CHECK`, `COLUMN_JSON`, `JSON_EXISTS`를 별도 catalog로 추가해
  MariaDB에는 추천하되 MySQL에는 누출하지 않는다.

스윕은 별도 축약 추천기를 쓰지 않는다. 각 단어 위치에 실제 cursor marker를 넣고
프로덕션과 같은 `query_completion_suggestions_with_data(..., true, ...)` 진입점을
DB 종류와 file-scoped metadata로 호출한다. 따라서 테스트 흐름을 따로 수정할 필요가
없었고, 실패를 재현한 개별 테스트도 같은 production completion helper를 사용한다.

## 17-5. IntelliSense 보고서 결과

| 보고서 | checked | missing |
| --- | ---: | ---: |
| `test/oracle_format_final_boss_3.sql.out` | 375 | 0 |
| `test_mysql/test8.txt.out` | 512 | 0 |
| `test_mariadb/test13.txt.out` | 449 | 0 |

통합 ignored 테스트
`intellisense_sweep_generate_report_for_file_certifies_new_final_boss_queries`는 이전
final-boss 3개와 이번 3개, 총 6개 모두를 `fail_on_missing=true`로 검사한다.
실제 추천 가능한 누락은 0건이다.

## 17-6. 100만 줄 성능 계약

새 로직은 full document 재스캔을 completion 후보별로 반복하지 않는다. CTE/window
declaration용 괄호 짝은 token span당 한 번 O(n)으로 만들고, exact nested grammar는
이미 bounded된 local/statement token slice만 본다. 다음 100만 줄 초과 production
회귀 4종으로 parse window, completion index, edit/undo delta, highlighting refresh가
bounded fast path를 유지하는지 함께 검증한다.

- `million_line_oracle_plsql_completion_window_stays_hard_capped`
- `million_line_production_completion_index_uses_the_bounded_fast_path`
- `million_line_production_undo_records_a_small_delta_and_shares_untouched_chunks`
- `million_line_production_shadow_edit_and_semantic_refresh_stay_bounded`

## 17-7. 최종 품질 게이트 결과

| 검증 | 결과 |
| --- | --- |
| formatter CTE body-indent 최소 회귀 | 1 통과, 실패 0 |
| 6개 final-boss 통합 IntelliSense 스윕 | 1 통과, 실패 0, 198.81초 |
| 100만 줄 초과 production 성능 회귀 | 4 통과, 실패 0, 0.14초 |
| 최종 전체 포맷 스윕 | 1 통과, 실패 0, 59개·34,382줄 |
| 수동 판독본/최종 재생성본 aggregate SHA-1 | 양쪽 모두 `14dfa65cbfef6ab813e92b995d83472c505783eb` |
| formatter error marker 검색 | 0건 |
| 정확한 `cargo test` | lib 6,479 통과·225 ignored, 모든 binary/integration/guard/doc-test 포함 실패 0 |
| 정확한 `cargo clippy -- -D warnings -W clippy::perf -W clippy::complexity` | 통과, 오류·경고 0 |
| `cargo fmt --all -- --check` | 통과 |

| `git diff --check` | 통과 |

## 17-8. 결론

신규 세 쿼리는 원본과 자동 포맷 결과가 모두 실제 대상 DB에서 실행되고 자체 검산을
통과했다. 신규 formatter 결함은 재현 테스트를 먼저 추가한 뒤 owning frame의 고정
`body_indent` 복귀로 수정했고, 59개 산출물 34,382줄을 직접 전수 판독한 최종본과
재생성본이 byte-level aggregate hash까지 같다. IntelliSense 스윕은 production
completion 흐름에서 신규 세 파일 총 1,336개 토큰을 검사해 누락 0이며, 전체 테스트와
엄격한 clippy에도 오류나 경고가 남지 않았다.


# 18. 11차 검증 회차 (2026-07-15): MySQL/MariaDB Final Boss VI와 재발 방지

## 18-1. 신규 실행 가능 단일 문장 fixture

기존 final-boss가 지나지 않던 문법을 한 문장 안에서 함께 검증하도록, 반복 실행해도
부작용이 없는 `WITH ... SELECT` fixture를 MySQL과 MariaDB에 각각 추가했다.

| DB | 파일 | 원본 줄 수 | 핵심 조합 |
| --- | --- | ---: | --- |
| MySQL 8.0 | `test_mysql/test9.txt` | 221 | 재귀 CTE, 중첩 `JSON_TABLE`, `JSON_VALUE ... RETURNING/DEFAULT`, `LATERAL`, inherited named window, `ROLLUP`, `INTERSECT`/`EXCEPT`, `MEMBER OF`, 다중 scalar/list frame |
| MariaDB 12.2 | `test_mariadb/test14.txt` | 213 | 재귀 `CYCLE`, 중첩 `JSON_TABLE`, dynamic column, `INET6`, percentile window, ordered/limited aggregate, `INTERSECT ALL`/`EXCEPT ALL`, 다중 scalar/list frame |

두 문장은 마지막 `CASE`가 중간 집계와 집합 연산 결과를 다시 검산한다. 원본과 자동
포맷 결과를 MySQL 8.0.46 및 MariaDB 12.2.2 컨테이너에서 각각 실행했으며 모두
exit code 0, 반환 4행의 `status = 'PASS'`를 확인했다.

파일별 IntelliSense 보고서도 함께 추가했다.

| 보고서 | checked | missing |
| --- | ---: | ---: |
| `test_mysql/test9.txt.out` | 566 | 0 |
| `test_mariadb/test14.txt.out` | 572 | 0 |

## 18-2. 포매터 AS-IS / TO-BE: aggregate 내부 `LIMIT`의 owning frame

전체 포맷 결과를 직접 읽는 과정에서 자동 PASS가 놓친 MariaDB aggregate 내부 절의
상대 깊이 오류를 발견했다. 일반 query의 `LIMIT` clause depth를 그대로 적용해 함수
호출 괄호가 소유한 body보다 바깥으로 빠지는 문제였다.

AS-IS:

```sql
GROUP_CONCAT(DISTINCT f.source_code ORDER BY f.source_code SEPARATOR ','
LIMIT 4) AS source_list,
JSON_ARRAYAGG(JSON_OBJECT('execution', f.execution_id) ORDER BY f.execution_id
LIMIT 10) AS execution_json
```

TO-BE:

```sql
GROUP_CONCAT(DISTINCT f.source_code ORDER BY f.source_code SEPARATOR ','
    LIMIT 4) AS source_list,
JSON_ARRAYAGG(JSON_OBJECT('execution', f.execution_id) ORDER BY f.execution_id
    LIMIT 10) AS execution_json
```

근본 수정은 키워드별 보정이 아니라 frame 계약으로 구현했다.

- ordinary expression/call paren 안의 clause header는 기존 clause depth와 소유 frame의
  고정 `body_indent` 중 더 깊은 값을 사용한다.
- query-like paren과 column-list paren은 기존 전용 layout을 유지한다.
- frame audit에 `ContainedClause` 이벤트를 추가해, 괄호 내부 clause가 소유 frame의
  body보다 얕아지면 스윕이 실패하도록 했다.
- `format_for_auto_formatting_mariadb_keeps_aggregate_limit_inside_owner_frame`가
  `GROUP_CONCAT`/`JSON_ARRAYAGG`와 두 번 포맷한 멱등성을 고정한다.

이 수정으로 바뀐 포맷 SQL은 MariaDB `test9.txt`, `test13.txt`, `test14.txt`의 aggregate
`LIMIT` 7곳뿐이다. 나머지 58개 결과의 SQL body는 이전 검토본과 byte 단위로 같았다.
변경된 `test13.txt` 포맷본도 MariaDB 12.2.2에서 재실행해 4행 모두 `PASS`를 확인했다.

## 18-3. IntelliSense AS-IS / TO-BE와 production 경로 일치

최초 전체 스윕에서 실제 추천 가능한 누락을 정확한 fixture 위치의 회귀 테스트로 먼저
고정했다. 대표 사례는 다음과 같다.

AS-IS:

```sql
-- MySQL: 고정 연산자 tail 누락
COALESCE('critical' MEMBER O| (JSON_ARRAY('critical')), FALSE)
-- suggestions: []

-- Oracle: indexed collection의 record field scope 소실
l_rows(i).REMA|;
-- REMARK 없음

-- Oracle: 먼 이전 SELECT가 현재 cursor 선언을 오염
CURSOR c_src I| SELECT ...
-- suggestions: INTERSECT, INTO

-- Oracle: 실행 MERGE의 typed keyword 누락
WHEN MATC| THEN
-- suggestions: []
```

TO-BE:

```sql
COALESCE('critical' MEMBER OF (JSON_ARRAY('critical')), FALSE)
l_rows(i).REMARK;
CURSOR c_src IS SELECT ...
WHEN MATCHED THEN
```

production `completion.rs`와 local symbol 경로를 다음 범위에서 수정했다.

- MySQL에만 `MEMBER -> OF`를 열고 MariaDB와 Oracle의 기존 문법을 분리했다.
- nested `WITH`/`SELECT`, `CASE`/exception `WHEN`, `FORALL`, `GOTO`, `SELECT INTO`,
  `REPLACE INTO`, `CONNECT_BY_ISCYCLE`, named `END`의 typed structural anchor를 보강했다.
- indexed qualifier `l_rows(i).field`, 앞쪽에서 선언되는 outer relation alias,
  `SYS.ODCINUMBERLIST`의 `COLUMN_VALUE`, quoted `COMMENT ON COLUMN`을 bounded text와
  file-scoped metadata로 복원했다.
- record field 이름에서는 문맥 키워드인 `REMARK`도 declaration identifier로 인정하되,
  PL/SQL 비타입 키워드는 계속 제외한다.
- 관계/컬럼 후보 merge는 typed table/column 문맥으로 제한하고 MySQL maintenance
  statement, Oracle privilege list, `EXTRACT(` argument frame에는 누출하지 않는다.

스윕 테스트도 production과 같은 흐름으로 바꿨다. 파일 전체 문자열을 매번 새로 만드는
대신 실제 편집과 같은 최소 `ChunkedText` splice를 적용하고, production worker가 쓰는
`compute_intellisense_suggestions`에 동일한 expanded statement와 analysis를 전달한다.
또한 production UI가 팝업을 억제하는 string/q-quote/comment 위치는 스윕도 같은 lexical
mode로 제외한다. 따라서 동적 SQL 문자열의 `WHEN MATCHED`는 false miss가 아니며, 실제
실행 SQL의 같은 토큰은 계속 검사된다.

모든 새 fallback은 4 KiB look-behind와 2 KiB look-ahead 또는 이미 bounded된 statement
token slice만 사용한다. 100만 줄 초과 회귀 5종이 0.28초에 통과해 전체 문서 길이에
비례하는 completion 후보별 재스캔이 없음을 확인했다.

## 18-4. 전체 스윕 및 수동 판독 결과

정확한 명령
`cargo test --lib formatting_sweep_all_files_generate_out_report -- --ignored --nocapture`로
최종 산출물을 만든 뒤 `target/format-sweep`의 모든 `.format.out`을 자동 PASS와 무관하게
처음부터 끝까지 다시 읽었다.

| 항목 | 결과 |
| --- | ---: |
| 전체 `.format.out` | 61개 |
| 직접 검토한 전체 report 줄 | 33,220줄 |
| footer 제외 포맷 SQL 줄 | 23,319줄 |
| PASS / issues 0 | 61 / 61 |
| 검사 frame / body item / close | 8,385 / 715 / 1,519 |
| 실제 상대 depth 수정 | aggregate `LIMIT` 7곳 |
| comma sibling body-depth 위반 | 0건 |
| same-paren top-level body-depth 위반 | 0건 |
| close-indent 및 parent body 복귀 위반 | 0건 |

최종 production 경로 IntelliSense 전수 스윕은 Oracle/MySQL/MariaDB 61파일에서
49,742개 토큰을 검사했으며 missing 0이었다. 최종 재실행 시간은 477.44초다.

## 18-5. 최종 품질 게이트

| 검증 | 결과 |
| --- | --- |
| 신규 MySQL/MariaDB 원본·포맷본 실서버 실행 | 각각 4행 PASS, exit code 0 |
| 기존 MariaDB `test13` 수정 포맷본 실서버 재실행 | 4행 PASS, exit code 0 |
| 100만 줄 초과 production 성능 회귀 | 5 통과, 실패 0, 0.28초 |
| 정확한 전체 포맷 스윕 | 1 통과, 실패 0, 61개·33,220줄 |
| 최종 전체 IntelliSense 스윕 | 1 통과, 61개·49,742 checked·missing 0 |
| 8개 final-boss `intellisense_sweep_generate_report_for_file` 통합 인증 | 1 통과, 실패 0, 52.99초 |
| 정확한 `cargo test` | lib 6,513 통과·228 ignored, 모든 binary/integration/guard/doc-test 실패 0 |
| `cargo clippy --locked --all-targets -- -D warnings -W clippy::perf -W clippy::complexity` | 통과, 오류·경고 0 |
| `cargo fmt --all -- --check` | 통과 |

## 18-6. 결론

신규 두 final-boss 쿼리와 포맷본은 실제 대상 DB에서 실행 가능하고 자체 검산을
통과했다. 포매터 문제는 특정 `LIMIT` 예외가 아니라 owning frame의 최소 body depth와
audit 계약으로 고쳤으며, IntelliSense는 production과 같은 snapshot/analysis/completion
경로 및 같은 literal/comment 억제 규칙으로 검사한다. 전체 리포트 수동 판독, 100만 줄
성능 회귀, 전체 Rust 테스트와 엄격한 Clippy까지 최종 오류와 경고가 남지 않았다.

# 19. 12차 검증 회차 (2026-07-15): 동일 frame 자식 owner+1 및 frame 완전성 감사

## 19-1. 자동 포맷 depth 계약

이번 수정은 첫 자식의 기존 출력에서 나머지 자식의 depth를 소급해 정하는 방식이 아니다.
frame을 만드는 시점에 다음 계약을 먼저 고정한다.

- owner가 depth `d`이면 frame body는 처음부터 `d + 1`이다.
- 첫 자식이 owner와 같은 줄에 남는 compact 표현은 허용한다.
- 첫 자식이 줄 시작 위치로 내려가면 첫 자식과 모든 직접 sibling은 `d + 1`이다.
- `AND`/`OR`와 쉼표 뒤의 직접 자식도 같은 body depth를 사용한다.
- 괄호와 블록처럼 명시적인 시작/종료가 있는 frame의 종료 구문은 시작 owner와 같은
  depth `d`다. 한 줄 안에서 닫히는 compact frame에는 줄 시작 close 규칙을 적용하지 않는다.
- `CREATE OR REPLACE`, `BETWEEN ... AND ...`, trigger event의 `OR`처럼 문법상 하나인 고정
  구문은 sibling 목록으로 해석하지 않는다.

따라서 다음과 같이 첫 조건까지 개행되는 경우에도 자식 정렬이 일관된다.

AS-IS:

```sql
WHERE
condition_a
    AND condition_b
    OR condition_c
```

TO-BE:

```sql
WHERE
    condition_a
    AND condition_b
    OR condition_c
```

자식 전체가 owner와 같은 줄에 유지되는 다음 compact 표현은 그대로 허용한다.

```sql
WHERE condition_a
LISTAGG(value, ',') WITHIN GROUP (ORDER BY key_a, key_b)
```

## 19-2. 조건 frame 전수 범위

Oracle, MySQL, MariaDB별 fixture와 독립 syntax inventory에서 다음 조건 owner를 typed
condition frame으로 검사한다.

- 공통 query 조건: `WHERE`, `JOIN ... ON`, `HAVING`, non-CASE `WHEN`
- 제어/반복 조건: `IF`, `ELSIF`, `WHILE`, `UNTIL`
- Oracle 계열: `START WITH`, `CONNECT BY`, `QUALIFY`, `MATCH_RECOGNIZE ... DEFINE`,
  conditional compilation의 `$IF`/`$ELSIF`
- CASE 조건은 기존 CASE branch frame과 결합하고, `BETWEEN ... AND ...` 및 고정 phrase는
  조건 sibling audit에서 제외한다.

멀티라인 condition frame의 첫 줄 시작 자식과 이후 `AND`/`OR` sibling은 모두 frame을
생성할 때 정한 owner+1을 사용한다.

## 19-3. 여러 자식을 갖는 list frame 전수 범위

쉼표가 나타나는 위치만 사후 보정하지 않고, 여러 직접 자식을 소유하는 구문을 다음 typed
list frame 40종으로 분류했다.

- query/list: `SELECT`, `FROM`, `SET`, `VALUES`, `GROUP BY`, `ORDER BY`, `WINDOW`,
  `INTO`, `WITH`, `USING`, `RETURNING`
- analytic/model/pattern: `PARTITION`, `DIMENSION`, `MEASURES`, model rules, `DEFINE`,
  `SUBSET`, `SEARCH BY`, `CYCLE` columns
- 괄호형 semantic list: 일반 직접 인자/식 목록, structured table arguments,
  `JSON_TABLE`/`XMLTABLE` columns, `PIVOT` aggregates, structured column declarations
- DML/DDL: delete/update targets, `FOR UPDATE OF`, trigger `UPDATE OF`, drop targets,
  rename pairs, `ALTER` actions
- 권한/관리: grant/revoke privileges와 grantees, lock tables, maintenance tables,
  account targets, flashback targets
- routine/vendor: handler conditions, diagnostics items, `DECLARE` names, `DO` expressions,
  trigger `FOLLOWS`/`PRECEDES`

예를 들어 compact 표현은 유지하되, 실제 줄 시작 자식은 owner+1로 정렬한다.

```sql
LISTAGG(value, ',') WITHIN GROUP (ORDER BY key_a, key_b)

MAX(value) KEEP (
    DENSE_RANK LAST ORDER BY (
        SELECT sort_key
        FROM source_table
    ),
        second_sort_key
)
```

`WITH`의 sibling CTE, `COLUMNS`의 column 정의, MODEL/MATCH_RECOGNIZE section의 항목도
같은 규칙을 사용한다. 세미콜론으로 연결되는 block statement, query set branch,
`MERGE` branch, cursor SQL, `FORALL`, handler/control body, `INSERT ALL` 등은 쉼표 list가
아니므로 기존 structural frame으로 관리하고 frame-kind inventory와 각 전용 레이아웃
회귀로 검증한다.

## 19-4. frame lifecycle 및 자동 이상 감지

`formatting_sweep_all_files_generate_out_report`를 단순 멱등성/토큰 보존 검사에서 frame
구조 감사까지 확장했다.

- frame ID 중복, opener 없는 close, 중복 close, close-before-open, 명시적 frame 미종료
- parent 누락/비포함, parent보다 늦게 닫히는 child
- 줄 시작 첫 자식과 모든 직접 sibling의 body-depth 불일치
- `AND`/`OR` 조건 sibling 및 쉼표 sibling의 owner 누락
- semantic list와 일반 parenthesis direct-list의 typed ownership 누락
- 줄 시작 괄호 close와 block/conditional-compilation 종료의 owner-depth 불일치
- leading comment가 있는 첫 list item, multiline 첫 condition, nested frame 복귀 시 depth drift
- production managed-frame enum 17종과 typed-list enum 40종이 테스트 inventory에서 모두
  실제 생성되는지 확인

이 감사의 핵심은 “모든 토큰을 list frame으로 만든다”가 아니다. 여러 직접 자식의 경계를
가진 구문은 list/condition/structural frame 중 하나가 반드시 소유하고, `OR REPLACE` 같은
단일 문법 phrase와 scalar token은 잘못된 자식 frame으로 만들지 않는 것이다.

## 19-5. 전체 sweep 및 품질 게이트

Oracle `test`, MySQL `test_mysql`, MariaDB `test_mariadb` 아래 모든 SQL/TXT fixture를
다시 생성하고 감사했다.

| 항목 | 결과 |
| --- | ---: |
| 전체 fixture | 61개 |
| 검사 frame | 18,346개 |
| 줄 시작 body item/sibling | 7,190개 |
| 검사 close | 9,660개 |
| production managed-frame kind | 17 / 17 |
| typed list-owner kind 독립 inventory | 40 / 40 |
| 실패 파일 / frame issue | 0 / 0 |

최종 검증 결과는 다음과 같다.

| 검증 | 결과 |
| --- | --- |
| `formatting_sweep_all_files_generate_out_report` | 1 통과, 61개 파일·issue 0 |
| `cargo test --lib` | 6,532 통과·228 ignored·실패 0 |
| 전체 `cargo test` | 모든 lib/binary/integration/guard/doc-test 실패 0 |
| `cargo clippy --locked --all-targets -- -D warnings -W clippy::perf -W clippy::complexity` | 통과, 경고 0 |
| `cargo fmt --all -- --check` | 통과 |

# 20. 스윕 PASS 불신 전제 61개 전수 육안 재검토와 절 depth drift 4건 수정

## 20-1. 검토 방법

- `formatting_sweep_all_files_generate_out_report`로 61개 fixture의 `.format.out`을 생성했다
  (자동 감사 결과는 전부 PASS).
- PASS를 신뢰하지 않고 61개 파일 33,626줄(포맷된 SQL 23,568줄 + 리포트 footer)을 처음부터
  끝까지 육안으로 읽고, 각 줄의 depth를 `docs/auto_format_rule.md`의 frame 계약
  (owner+1 고정 depth, sibling 단일 body depth, close=owner depth, drift 금지)과 대조했다.
- 의심 지점은 `SPACE_QUERY_FORMAT_SWEEP_FILE` 프로브로 최소 재현을 만들어 실제 위반 여부를
  확정했다. 자동 감사가 절(clause) header의 depth는 검사하지 않기 때문에 아래 4건은 모두
  status PASS 상태에서 육안으로만 발견됐다.

## 20-2. 여러 줄 join ON 조건 뒤 `ON DUPLICATE KEY UPDATE` depth (test_mariadb/test6)

`INSERT ... SELECT`의 마지막 JOIN이 여러 줄 `ON` 조건으로 끝나면, 그 다음의
`ON DUPLICATE KEY UPDATE`가 INSERT 문의 절이 아니라 join ON처럼 JoinBody depth에 붙었다.

AS-IS:

```sql
    ) tc
        ON
            tc.month_key = b.month_key
            AND tc.region_id = b.region_id
        ON DUPLICATE KEY UPDATE orders_cnt = VALUES(orders_cnt),
            customers_cnt = VALUES(customers_cnt),
```

TO-BE:

```sql
    ) tc
        ON
            tc.month_key = b.month_key
            AND tc.region_id = b.region_id
    ON DUPLICATE KEY UPDATE orders_cnt = VALUES(orders_cnt),
        customers_cnt = VALUES(customers_cnt),
```

`ON DUPLICATE KEY UPDATE`의 `ON`을 `starts_mysql_on_duplicate_key_update_clause`로 판별해
(1) `WHERE`/`GROUP` 등과 같은 clause 경계로 취급해 JoinBody scoped indent를 비활성화하고
(2) join-ON depth 재사용 분기에서 제외했다. VALUES(col) 함수 처리와 assignment sibling
depth(+1)는 기존 동작을 유지한다.

추가 회귀 테스트:
`format_sql_basic_for_mysql_db_type_keeps_on_duplicate_clause_depth_after_multiline_join_on`

## 20-3. WITH 본문 query에서 함수로 감싼 window item 뒤 `FROM` drift (test_mariadb/test6)

WITH 문의 main query에서 `ROUND( ... OVER (...) ... )`처럼 일반 함수 괄호 안에 analytic
window가 들어간 select item 이후, 다음 절 header가 그 괄호 안 깊이를 상속했다.

AS-IS:

```sql
    DENSE_RANK() OVER (
        PARTITION BY month_key
        ORDER BY net_sales DESC,
            segment
    ) AS month_rank
            FROM monthly
```

TO-BE:

```sql
    DENSE_RANK() OVER (
        PARTITION BY month_key
        ORDER BY net_sales DESC,
            segment
    ) AS month_rank
FROM monthly
```

원인은 “WITH 문 SELECT 절의 쉼표는 select-list layout 상태를 재고정한다”는 분기가 함수
인자 쉼표(`ROUND(..., 2)`의 쉼표)에도 발동해, statement 레벨 select-list 상태를 중첩 괄호
깊이로 덮어쓴 것이다. 이 분기를 열려 있는 모든 괄호가 query-like frame일 때
(`all_paren_frames_are_query_like`)로 제한해, 일반 괄호 안 쉼표는 그 괄호의 list frame이
소유하도록 했다.

추가 회귀 테스트:
`format_sql_basic_for_mysql_db_type_returns_from_clause_to_query_depth_after_wrapped_window_item`

## 20-4. MariaDB `SET STATEMENT ... FOR` 래핑 query의 절 depth 분열 (test_mariadb/test9)

래핑된 query의 `SELECT`/`FROM`은 SET 할당식 depth(+2)를 상속하고 `GROUP BY`부터는 depth 0으로
떨어져, 한 문장 안에서 절 depth가 갈라졌다.

AS-IS:

```sql
SET STATEMENT max_statement_time = 5 FOR
        SELECT account_id,
            COUNT(*) event_count,
            ROUND(SUM(amount), 2) total_amount
        FROM mf_event
GROUP BY account_id
HAVING COUNT(*) >= 1
```

TO-BE:

```sql
SET STATEMENT max_statement_time = 5 FOR
SELECT account_id,
    COUNT(*) event_count,
    ROUND(SUM(amount), 2) total_amount
FROM mf_event
GROUP BY account_id
HAVING COUNT(*) >= 1
```

`SET STATEMENT <assignments> FOR`의 `FOR`에서 SET 할당의 AssignmentValue frame과 Set list
owner를 정리하고 clause 상태를 초기화해, 래핑된 문장이 새 statement 문맥(depth 0)에서
시작하도록 했다. 모든 절이 같은 depth를 공유한다.

추가 회귀 테스트:
`format_sql_basic_for_mysql_db_type_keeps_set_statement_for_query_clauses_on_one_depth`

## 20-5. 확장형 SELECT에서 `DISTINCT` modifier 분리 (test_mariadb/test14)

select list가 확장(줄바꿈) 스타일로 렌더링될 때 `DISTINCT`가 owner header에서 떨어져 첫
item 줄로 내려갔다. 계약상 owner modifier는 owner header에 남아야 한다
(`WITH RECURSIVE`와 같은 규칙).

AS-IS:

```sql
        SELECT
            DISTINCT agent_id,
            PERCENTILE_CONT(0.5) WITHIN GROUP (ORDER BY duration_ms) OVER (
```

TO-BE:

```sql
        SELECT DISTINCT
            agent_id,
            PERCENTILE_CONT(0.5) WITHIN GROUP (ORDER BY duration_ms) OVER (
```

확장형 SELECT에서 다음 토큰이 `DISTINCT`/`UNIQUE`/`DISTINCTROW`이면 header 줄바꿈을
modifier 뒤로 미룬다. 첫 item은 그대로 select-list body depth를 사용한다.

추가 회귀 테스트:
`format_sql_basic_for_mysql_db_type_keeps_distinct_on_the_expanded_select_header`

## 20-6. 육안 검토에서 확인 후 의도된 동작으로 판정한 항목

- 재귀 CTE의 `SEARCH ... SET` / `CYCLE ... SET` 절이 WITH 키워드 depth에 오는 레이아웃:
  `format_sql_recursive_cte_search_cycle_*`, `format_sql_recursive_cte_cycle_separator_restores_with_body_indent`
  등 다수 테스트로 고정된 설계다.
- `JSON_ARRAYAGG(... RETURNING CLOB)`류 continuation이 “함수 호출이 시작된 줄 depth + 1”에
  오는 레이아웃: `format_for_auto_formatting_restores_outer_select_clause_after_nested_json_xml_select_item`의
  expected 출력으로 고정된 설계다.
- `OUTER APPLY (`의 body가 join 줄 +1에 오는 것: JoinBody typed view가 괄호와 owner edge를
  공유하는 설계로, 감사 인벤토리와 일치한다.

## 20-7. 전체 sweep 및 품질 게이트

| 항목 | 결과 |
| --- | ---: |
| 전체 fixture / 리포트 줄 수(육안 검토 분량) | 61개 / 33,626줄 |
| 포맷된 SQL 줄 수 | 23,568줄 |
| 검사 frame / 줄 시작 body item / close | 20,878 / 12,567 / 9,660 |
| 실패 파일 / frame issue | 0 / 0 |
| 이번 수정으로 출력이 바뀐 fixture | test_mariadb/test6·test9·test14 (3개, 모두 개선분만) |

| 검증 | 결과 |
| --- | --- |
| `formatting_sweep_all_files_generate_out_report` | 통과, 61개 파일·issue 0 |
| `cargo test --lib` | 6,547 통과·228 ignored·실패 0 (회귀 테스트 4건 추가) |
| 전체 `cargo test` | 모든 lib/binary/integration/guard 실패 0 |
| `cargo clippy --locked --all-targets -- -D warnings -W clippy::perf -W clippy::complexity` | 통과, 경고 0 |
| `cargo fmt --all -- --check` | 통과 |

# 21. PASS 출력 43,695줄 전수 검토와 frame 자동 감사 사각지대 4건 보강

## 21-1. 검토 범위와 판정 기준

- `cargo test --lib formatting_sweep_all_files_generate_out_report -- --ignored --nocapture`로
  Oracle 41개, MySQL 9개, MariaDB 11개 등 `.format.out` 61개를 생성했다.
- 자동 PASS를 판정 근거로 사용하지 않고 리포트 footer를 포함한 43,695줄을 처음부터 끝까지
  육안 검토했다. 각 줄은 `docs/auto_format_rule.md`의 문법 소유권, owner+1 body depth,
  sibling 동일 depth, close 후 외부 depth 복구, 주석 비구조 규칙과 대조했다.
- 1차 육안 검토에서 PASS가 놓친 4개 원인을 찾았다. 수정 후 전체 sweep을 다시 생성하고,
  네 변경 계열의 모든 출력 지점(함수-local `RETURNING`, 조건부 컴파일, 세미콜론 후행 주석,
  standalone routine header/body)을 재검토했다.

## 21-2. `JSON_VALUE` 함수-local `RETURNING` option depth

중첩 함수 또는 CTE SELECT item의 `JSON_VALUE`에서 path 다음 줄의 `RETURNING`이 활성 함수
괄호 body가 아니라 현재 렌더링 줄에서 한 단계 더 내려가는 경우가 있었다.

AS-IS:

```sql
JSON_VALUE (e.json_profile,
    '$.level'
        RETURNING VARCHAR2 (30)) AS profile_level,
JSON_VALUE (e.json_profile,
    '$.flags.remote'
    RETURNING VARCHAR2 (10)) AS remote_flag
```

TO-BE:

```sql
JSON_VALUE (e.json_profile,
    '$.level'
    RETURNING VARCHAR2 (30)) AS profile_level,
JSON_VALUE (e.json_profile,
    '$.flags.remote'
    RETURNING VARCHAR2 (10)) AS remote_flag
```

원인은 SELECT 안 함수-local `RETURNING` depth를 `현재 렌더링 줄 depth + 1`로 계산한 것이다.
이 값을 활성 non-query paren frame의 `sibling_body_indent()`로 바꿨다. 따라서 바깥 `CAST`,
CTE, SELECT depth와 무관하게 path/option sibling이 같은 함수 body depth를 사용한다.

자동 감사에는 `parenthesized RETURNING option` 문법 예상 depth 이벤트를 추가했다. 기존에는
괄호의 comma sibling과 close만 검사했으므로 comma가 없는 option line은 검사 대상이 아니었다.

추가 회귀 테스트:

- `formatting_sweep_audits_function_local_returning_option_depth` (Oracle/MySQL)

## 21-3. Oracle conditional compilation `$ELSE` branch의 call frame

`$THEN` branch의 다중 인자 호출은 정상이나 `$ELSE` branch에서 같은 호출의 두 번째 이후
인자가 call paren depth를 잃었다.

AS-IS:

```sql
$ELSE
    AUDIT ('qt_torture_pkg',
    'complex_block.ccflag',
    'conditional-compilation=false');
```

TO-BE:

```sql
$ELSE
    AUDIT ('qt_torture_pkg',
        'complex_block.ccflag',
        'conditional-compilation=false');
```

conditional-compilation frame은 별도 vector에서 branch body depth를 제공했지만, 자식 frame의
문법 parent를 고르는 `nearest_child_owner_frame()`은 주 `FormatFrameStack::frames`만 검색했다.
`$THEN` 직후에는 우연히 condition-owner frame이 남아 정상 depth를 제공했지만 `$ELSE`에서는
그 frame이 없어 바깥 PL/SQL block을 parent로 선택했다.

활성 conditional branch를 자식 owner 후보에 포함하고, 같은 depth에서는 더 구체적인 주 stack
frame을 우선하도록 했다. 테스트 감사에서는 conditional body 직계 호출 paren의 예상 depth를
branch body+1로 독립 기록하므로, 잘못된 parent를 다시 선택하면 frame 자체가 내부적으로
일관돼도 검출된다.

추가 회귀 테스트:

- `formatting_sweep_audits_conditional_branch_call_argument_depth`

## 21-4. 닫힌 호출의 후행 주석 뒤 statement sibling depth

세미콜론 뒤에 `--` 후행 주석이 있으면 newline 처리를 주석 token에 미루면서, 닫힌 호출의
마지막 인자 depth가 다음 PL/SQL statement에 남았다.

AS-IS:

```sql
BEGIN
    oqt_pkg.p_basic (7,
        p_out_txt => v_out,
        p_inout_n => v_inout); -- p_in_txt omitted
        DBMS_OUTPUT.PUT_LINE ('[default] ...');
END;
```

TO-BE:

```sql
BEGIN
    oqt_pkg.p_basic (7,
        p_out_txt => v_out,
        p_inout_n => v_inout); -- p_in_txt omitted
    DBMS_OUTPUT.PUT_LINE ('[default] ...');
END;
```

세미콜론에서 다음 token이 inline comment이면 물리 newline을 즉시 출력하지 않는 기존 동작은
유지하되, 논리 `line_indent`는 닫힌 frame이 제거된 `base_indent`로 즉시 복구한다. 다음 token이
`BEGIN`, `END`, `ELSE`, `EXCEPTION`, `$ELSE`, `$END` 같은 구조 경계이면 각 경계 전용 로직이
소유 depth를 정하도록 이 sibling 복구에서 제외했다.

자동 감사에는 후행 주석 다음 일반 statement token의 예상 sibling depth를 기록했다. 기존
감사는 block의 첫 자식만 검사하고 세미콜론으로 연결된 이후 statement sibling은 기록하지
않았으므로 이 drift를 보지 못했다.

추가 회귀 테스트:

- `formatting_sweep_audits_statement_sibling_after_trailing_comment`

## 21-5. 여러 줄 standalone routine parameter header 뒤 body depth

Oracle standalone function/procedure의 parameter header가 여러 줄로 확장되면 `) IS` 줄의
parameter continuation depth가 routine body owner로 사용돼 선언부와 실행부 전체가 한 단계
깊어졌다.

AS-IS:

```sql
CREATE OR REPLACE PROCEDURE qt_fb_log_proc (p_module IN VARCHAR2,
    p_action IN VARCHAR2,
    p_msg IN CLOB,
    p_extra IN CLOB DEFAULT NULL) IS
        PRAGMA AUTONOMOUS_TRANSACTION;
    BEGIN
        INSERT INTO qt_fb_audit ...
    END;
```

TO-BE:

```sql
CREATE OR REPLACE PROCEDURE qt_fb_log_proc (p_module IN VARCHAR2,
    p_action IN VARCHAR2,
    p_msg IN CLOB,
    p_extra IN CLOB DEFAULT NULL) IS
    PRAGMA AUTONOMOUS_TRANSACTION;
BEGIN
    INSERT INTO qt_fb_audit ...
END;
```

`AS`/`IS`가 standalone `CREATE PROCEDURE/FUNCTION` body를 열 때 렌더링 중인 `line_indent`가
아니라 현재 구조 stack의 statement base를 routine owner로 사용하도록 했다. package member와
WITH PL/SQL 선언은 기존 전용 owner 계산을 유지한다.

자동 감사에는 standalone routine의 첫 선언 token은 owner+1, 즉시 `BEGIN`이면 owner depth라는
문법 예상 이벤트를 추가했다. 기존 block 감사는 잘못된 owner=1, body=2를 함께 등록했으므로
서로 일관된 잘못을 정상으로 판정했다.

추가 회귀 테스트:

- `formatting_sweep_audits_multiline_standalone_routine_header_close`

## 21-6. 기존 frame 구조 판단 테스트가 네 오류를 검출하지 못한 이유

기존 자동화의 각 계층은 다음 이유로 모두 PASS를 반환했다.

1. first-pass line 감사는 tab, trailing whitespace, 4-space 배수만 검사했다. 네 오류 모두
   4-space 단위였으므로 통과했다.
2. idempotence와 whitespace mutation probe는 “같은 token이 같은 canonical 출력으로
   수렴하는가”를 검사한다. 잘못된 depth도 안정적으로 재생성됐으므로 통과했다.
3. frame alignment 감사는 등록된 opener, comma/condition sibling, direct body item, close만
   검사했다. 함수 option과 세미콜론 뒤 statement sibling은 이벤트가 없었다.
4. conditional call과 routine body는 frame 이벤트가 있었지만, 잘못 계산한 owner depth를
   expected와 actual 양쪽에 같은 값으로 기록했다. 즉 내부 일관성만 확인하고 문법 parent와의
   독립 교차 검증이 없었다.

이를 보완하기 위해 `ExpectedIndent` 감사 이벤트를 추가했다. renderer가 선택한 물리 indent와
별도로 문법 경계에서 예상 depth를 기록해 다음 네 계열을 sweep에서 자동 검출한다.

- parenthesized function option
- conditional branch의 직계 child paren
- trailing comment 뒤 statement sibling
- standalone routine body boundary

`frame_alignment_audit_reports_grammatical_expected_indent_drift`는 인위적인 잘못된 출력이 새
이벤트에서 실제 `FrameAlignment` issue가 되는지 검증한다.

## 21-7. 전체 sweep 및 품질 게이트

| 항목 | 결과 |
| --- | ---: |
| 전체 fixture / 육안 검토 줄 수 | 61개 / 43,695줄 |
| Oracle / MySQL / MariaDB fixture | 41 / 9 / 11 |
| 수정한 독립 원인 | 4건 |
| 검사 frame / 문법·body item / close | 21,071 / 22,780 / 9,709 |
| managed frame / list-owner kind | 22 / 31 |
| built-in sweep regression | 26개 |
| 실패 파일 / frame issue | 0 / 0 |

| 검증 | 결과 |
| --- | --- |
| `formatting_sweep_all_files_generate_out_report` | 1 통과, 61개 파일·failure 0 |
| 추가 frame/format 회귀 테스트 | 5 통과·실패 0 |
| `cargo test` | lib 6,555 통과·228 ignored, 전체 binary/integration/guard/doc-test 실패 0 |
| `cargo clippy --locked --all-targets -- -D warnings -W clippy::perf -W clippy::complexity` | 통과, 경고 0 |
| `cargo fmt --all -- --check` | 통과 |

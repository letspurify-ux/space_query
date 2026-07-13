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

일반 SQL 문맥을 분석하는 복잡한 규칙 대신 다음 순서가 한 줄에 붙은 경우만 보수적으로 경계를 복원한다.

```text
SET PAGESIZE/LINESIZE -> WHENEVER SQLERROR -> PROMPT -> CREATE TABLE
```

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

동적 SQL 표현식의 CASE를 `EXECUTE IMMEDIATE` 소유자보다 한 단계 깊게 배치했다.

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

분기 조건의 `AND`는 `WHEN`보다 한 단계 깊게 유지하고, 분기들이 끝난 뒤 driving `SELECT`/`FROM`은 `INSERT ALL` 소유 깊이로 복귀시켰다.

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

FOR/WHILE 헤더의 최종 렌더링 깊이를 LOOP 종결자까지 보존했다. 범위식의 CASE `END`와 루프 시작 `LOOP`는 서로 다른 줄에 두어 종결 대상의 모호성을 없앴다. 커서 쿼리 안의 `FOR UPDATE`는 새 루프 헤더가 아니므로 바깥 루프 소유 깊이를 덮어쓰지 않게 했다.

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

`USING` bind 중 CASE가 하나라도 여러 줄로 렌더링되면 그 CASE와 이후 bind를 실행문 소유자보다 한 단계 깊은 형제 목록으로 분리한다. 짧은 `EXECUTE IMMEDIATE ... USING v1, v2`는 기존처럼 한 줄을 유지한다.

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

## 5. 그 외 개선 범위

- Oracle/PLSQL 중첩 블록, `EXCEPTION`, `CASE`, `INSERT ALL`, `PIVOT`, `APPLY` 정렬
- `DELETE WHERE`, PL/SQL collection `.DELETE` 구분
- 분석 함수 `OVER`, `WITHIN GROUP`, `MATCH_RECOGNIZE` 줄바꿈
- MySQL/MariaDB `CASE`, `REPEAT` 함수/루프, 중첩 `ORDER BY` 구분
- 코드 줄의 trailing whitespace 제거
- 포맷 전후 SQL 토큰 보존 검사

## 6. 의도적으로 유지한 동작

SQL*Plus `PROMPT`는 화면에 출력되는 payload이므로 기존 정책대로 원본 대소문자와 선행 공백을 verbatim으로 보존한다. 따라서 `PROMPT`의 선행 공백 자체는 자동 포맷 오류로 분류하지 않는다.

## 7. 검증 결과

| 검증 | 결과 |
| --- | --- |
| 최종 `.format.out` 수동 검토 | 47개, 28,567줄, 확정 오류 0건 |
| 지정 ignored 포맷 스윕 테스트 | 1/1 통과 |
| 실제 SQL 파일 자동 포맷 스윕 집계 | 47/47 통과, 실패 0건 |
| 수정 전후 `.format.out` delta 검토 | 46개 동일, scalar subquery 수정 대상 1개만 변경 |
| 최종 반복 재생성 해시 비교 | 47개 모두 동일 |
| 시각 회귀 테스트 | 47/47 통과 |
| 전체 라이브러리 테스트 | 6,414 통과, 실패 0, ignored 212 |
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

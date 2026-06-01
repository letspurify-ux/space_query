-- ╔══════════════════════════════════════════════════════════════════════════════╗
-- ║  ORACLE 실행단위 분리기 - 최종보스 검증 테스트 스크립트 v3.0                    ║
-- ║  "이보다 더 복잡할 수 없다" 에디션                                             ║
-- ║                                                                              ║
-- ║  총 50개 테스트 케이스 / 예상 실행단위 수 명시                                  ║
-- ║  난이도: ★☆☆☆☆ ~ ★★★★★                                                  ║
-- ╚══════════════════════════════════════════════════════════════════════════════╝
-- ============================================================================
-- CATEGORY 1: 문자열 리터럴 지옥 (String Literal Hell)
-- ============================================================================
-- [TEST-001] 세미콜론/슬래시가 포함된 문자열 (예상: 1 실행단위)
-- 난이도: ★★☆☆☆

SELECT 'INSERT INTO t VALUES (1; 2; 3);' AS fake_sql,
    'END; / BEGIN' AS trap1,
    'CREATE OR REPLACE PROCEDURE test IS BEGIN NULL; END;' AS trap2,
    q'[She said "it's done"; then left/]' AS trap3
FROM DUAL;

-- [TEST-002] q-quote 중첩 + 다양한 구분자 (예상: 1 실행단위)
-- 난이도: ★★★☆☆

SELECT q'!It's a "test" with ; and / inside!',
    q'{BEGIN; END; / CREATE}',
    q'<Don't; stop; me; now;>',
    q'(Nested (parens) with; semi)',
    q'#Hash # delim with END; here#',
    nq'[유니코드; 세미콜론; 슬래시/]'
FROM DUAL;

-- [TEST-003] 연속된 빈 문자열과 이스케이프 (예상: 1 실행단위)
-- 난이도: ★★☆☆☆

SELECT '' AS empty1,
    'it''s' AS escaped1,
    '''''' AS triple_escape,
    '''' AS just_quote,
    'END;' || '/' || chr (10) || 'BEGIN' AS concat_trap,
    q'['Don''t break;']' AS q_with_escape
FROM DUAL;

-- [TEST-004] 문자열 안의 PL/SQL 블록 전체 (예상: 1 실행단위)
-- 난이도: ★★★☆☆

DECLARE
    v_sql CLOB := '
        CREATE OR REPLACE PACKAGE BODY pkg AS
            PROCEDURE p1 IS
            BEGIN
                FOR i IN 1..10 LOOP
                    INSERT INTO t VALUES(i);
                    COMMIT;
                END LOOP;
            EXCEPTION
                WHEN OTHERS THEN
                    ROLLBACK;
            END p1;
        END pkg;
        /
    ';
BEGIN
    EXECUTE IMMEDIATE v_sql;
END;
/

-- ============================================================================
-- CATEGORY 2: 주석 미궁 (Comment Labyrinth)
-- ============================================================================
-- [TEST-005] 중첩 주석 (Oracle 미지원이지만 파서는 처리해야 함) (예상: 1 실행단위)
-- 난이도: ★★★☆☆

SELECT /* 외부 주석
    /* 내부 주석처럼 보이는 것 */
    아직 외부 주석 안 -- 라인 주석도 무시
    'END;' 이것도 주석 안 / 이것도 주석 안 * / 1 AS result
FROM DUAL;

-- [TEST-006] 주석과 문자열의 교차 (예상: 1 실행단위)
-- 난이도: ★★★★☆

SELECT -- 이 라인주석 안에 'BEGIN 문자열 시작
    1 AS col1, -- END; 주석 안의 가짜 종료
    /* 블록주석 시작 'string inside comment */
    'real string /* not comment */' AS col2,
    'value with -- not a comment' AS col3
FROM DUAL
WHERE 1 = 1 -- /* 이건 라인주석이지 블록주석 시작이 아님
    AND 2 = 2;

-- [TEST-007] 힌트 주석 vs 일반 주석 (예상: 1 실행단위)
-- 난이도: ★★☆☆☆

SELECT /*+ FULL(t) PARALLEL(t, 8) USE_HASH(t s)
          INDEX(s idx_status)
          NO_MERGE QB_NAME(main_query) */
        t.id,
    /* 이건 일반 주석; END; / */
    s.status
FROM my_table t
JOIN status_table s
    ON t.id = s.id
WHERE /*+ 이건 힌트가 아닌 주석 */
t.active = 'Y';

-- ============================================================================
-- CATEGORY 3: PL/SQL 블록 심연 (PL/SQL Block Abyss)
-- ============================================================================
-- [TEST-008] 다중 중첩 BEGIN-END (예상: 1 실행단위)
-- 난이도: ★★★★☆

DECLARE
    v_result NUMBER;
BEGIN
    BEGIN
        -- depth 2
        BEGIN
            -- depth 3
            BEGIN
                -- depth 4
                BEGIN
                    -- depth 5
                    v_result := 1;
                    IF v_result = 1 THEN
                        BEGIN
                            -- depth 6, inside IF
                            NULL;
                        EXCEPTION
                            WHEN OTHERS THEN
                                BEGIN
                                    -- depth 7
                                    DBMS_OUTPUT.PUT_LINE ('error');
                                END; -- depth 7
                        END; -- depth 6
                    END IF;
                END; -- depth 5
            END; -- depth 4
        END; -- depth 3
    END; -- depth 2
END;

-- depth 1
/

-- [TEST-009] CASE 표현식 END vs 블록 END 구분 (예상: 1 실행단위)
-- 난이도: ★★★★★

DECLARE
    v_x NUMBER := 1;
    v_y VARCHAR2 (100);
BEGIN
    v_y :=
    CASE
        WHEN v_x = 1 THEN
            CASE v_x
                WHEN 1 THEN
                    'ONE'
                WHEN 2 THEN
                    'TWO'
                ELSE
                    CASE
                        WHEN v_x > 2 THEN
                            'BIG'
                        ELSE
                            'SMALL'
                    END
            END
        WHEN v_x = 2 THEN
            'TWO'
        ELSE
            CASE
                WHEN v_x IS NULL THEN
                    'NULL'
                ELSE
                    'OTHER'
            END
    END;
    FOR i IN 1..
    CASE
        WHEN v_x = 1 THEN
            5
        ELSE
            10
    END LOOP
        DBMS_OUTPUT.PUT_LINE (
            CASE i
                WHEN 1 THEN
                'first'
                WHEN 2 THEN
                'second'
                ELSE
                'other'
            END
            );
    END LOOP;
    UPDATE my_table
    SET status =
        CASE
            WHEN id IN (
                SELECT
                    CASE
                        WHEN active = 'Y' THEN id
                        ELSE NULL
                    END
                FROM sub_t
            ) THEN
                'ACTIVE'
            ELSE
                'INACTIVE'
        END,
        priority =
        CASE category
            WHEN 'A' THEN
                1
            WHEN 'B' THEN
                2
            ELSE
            CASE
                WHEN amount > 1000 THEN
                    3
                ELSE
                    4
            END
        END
    WHERE dept_id = v_x;
    COMMIT;
END;
/

-- [TEST-010] 라벨 + GOTO + 루프 혼합 (예상: 1 실행단위)
-- 난이도: ★★★★☆

DECLARE
    v_cnt NUMBER := 0;
BEGIN
    <<outer_loop>>
    FOR i IN 1..10 LOOP
        <<inner_loop>>
        WHILE v_cnt < 100 LOOP
            v_cnt := v_cnt + 1;
            IF v_cnt = 50 THEN
                GOTO skip_section;
            END IF;
            <<nested_block>>
            BEGIN
                IF MOD (v_cnt, 7) = 0 THEN
                    EXIT outer_loop;
                ELSIF MOD (v_cnt, 5) = 0 THEN
                    EXIT inner_loop;
                ELSIF MOD (v_cnt, 3) = 0 THEN
                    CONTINUE outer_loop;
                END IF;
            END nested_block;
        END LOOP inner_loop;
        <<skip_section>> NULL;
    END LOOP outer_loop;
END;
/

-- [TEST-011] 익명 블록 → DML → 익명 블록 연속 (예상: 4 실행단위)
-- 난이도: ★★★☆☆

BEGIN
    DBMS_OUTPUT.PUT_LINE ('First block');
END;
/

INSERT INTO log_table (msg)
VALUES ('between blocks; with semicolons');

BEGIN
    DBMS_OUTPUT.PUT_LINE ('Second block');
END;
/

DELETE
FROM log_table
WHERE msg LIKE '%;%';

-- ============================================================================
-- CATEGORY 4: DDL 오브젝트 생성 카오스 (DDL Creation Chaos)
-- ============================================================================
-- [TEST-012] CREATE PACKAGE BODY - 다중 프로시저/함수 (예상: 1 실행단위)
-- 난이도: ★★★★★

CREATE OR REPLACE PACKAGE BODY complex_pkg AS
    gc_version CONSTANT VARCHAR2 (10) := '3.0';
    CURSOR c_employees IS
        SELECT e.*,
            d.department_name,
            CASE
                WHEN e.salary > 10000 THEN 'HIGH'
                WHEN e.salary > 5000 THEN 'MID'
                ELSE 'LOW'
            END AS salary_grade
        FROM employees e
        JOIN departments d
            ON e.department_id = d.department_id;
    FUNCTION calculate_bonus (p_emp_id IN NUMBER, p_year IN NUMBER DEFAULT EXTRACT (YEAR FROM SYSDATE)) RETURN NUMBER IS
        v_bonus NUMBER;
        v_sql VARCHAR2 (4000);
    BEGIN
        v_sql := 'SELECT SUM(amount) FROM bonus_history
                   WHERE emp_id = :1 AND fiscal_year = :2';
        EXECUTE IMMEDIATE v_sql INTO v_bonus USING p_emp_id, p_year;
        RETURN NVL (v_bonus, 0) *
        CASE
            WHEN p_year = EXTRACT (YEAR FROM SYSDATE) THEN
                1.1
            ELSE
                1.0
        END;
    EXCEPTION
        WHEN NO_DATA_FOUND THEN
            RETURN 0;
        WHEN OTHERS THEN
            log_error (SQLERRM);
            RAISE;
    END calculate_bonus;

    PROCEDURE process_employees (p_dept_id IN NUMBER, p_action IN VARCHAR2) IS
        TYPE emp_tab_t IS TABLE OF c_employees%ROWTYPE;
        l_employees emp_tab_t;
        PROCEDURE inner_validate (p_emp IN c_employees%ROWTYPE) IS
        BEGIN
            IF p_emp.salary IS NULL THEN
                RAISE_APPLICATION_ERROR (- 20001, 'Null salary for emp: ' || p_emp.employee_id);
            END IF;
        END inner_validate;
    BEGIN
        OPEN c_employees;
        LOOP
            FETCH c_employees BULK COLLECT INTO l_employees LIMIT 1000;
            EXIT WHEN l_employees.COUNT = 0;
            FORALL i IN 1..l_employees.COUNT
            INSERT INTO emp_staging
            VALUES l_employees (i);
            FOR i IN 1..l_employees.COUNT LOOP
                inner_validate (l_employees (i));
                CASE p_action
                    WHEN 'BONUS' THEN
                        UPDATE employees
                        SET salary = salary + calculate_bonus (l_employees (i).employee_id)
                        WHERE employee_id = l_employees (i).employee_id;
                    WHEN 'AUDIT' THEN
                        INSERT INTO audit_log (emp_id, action, ts)
                        VALUES (l_employees (i).employee_id, 'PROCESSED', SYSTIMESTAMP);
                    ELSE
                        NULL;
                END CASE;
            END LOOP;
            COMMIT;
        END LOOP;
        CLOSE c_employees;
    EXCEPTION
        WHEN OTHERS THEN
            IF c_employees%ISOPEN THEN
                CLOSE c_employees;
            END IF;
            ROLLBACK;
            RAISE;
    END process_employees;

    PROCEDURE log_error (p_msg IN VARCHAR2) IS
        PRAGMA AUTONOMOUS_TRANSACTION;
    BEGIN
        INSERT INTO error_log (msg, ts)
        VALUES (p_msg, SYSTIMESTAMP);
        COMMIT;
    END log_error;
END complex_pkg;
/

-- [TEST-013] CREATE TYPE BODY with MAP/ORDER (예상: 1 실행단위)
-- 난이도: ★★★★☆

CREATE OR REPLACE TYPE BODY money_t AS
    CONSTRUCTOR FUNCTION money_t (p_amount IN NUMBER, p_currency IN VARCHAR2 DEFAULT 'KRW') RETURN SELF AS RESULT IS
    BEGIN
        self.amount := p_amount;
        self.currency := UPPER (p_currency);
        RETURN;
    END;

    MAP MEMBER FUNCTION get_normalized RETURN NUMBER IS v_rate NUMBER;

    BEGIN
            SELECT rate
            INTO v_rate
            FROM exchange_rates
            WHERE from_curr = self.currency
                AND to_curr = 'USD'
                AND rate_date = (
                    SELECT MAX (rate_date)
                    FROM exchange_rates
                    WHERE from_curr = self.currency
                        AND to_curr = 'USD'
                );
        RETURN self.amount * v_rate;
    EXCEPTION
        WHEN NO_DATA_FOUND THEN
            RETURN
            CASE self.currency
            WHEN 'USD' THEN
                    self.amount
            WHEN 'KRW' THEN
                    self.amount / 1300
                ELSE
                    NULL
            END;
    END get_normalized;

    MEMBER FUNCTION ADD (p_other IN money_t) RETURN money_t IS
    BEGIN
        IF self.currency = p_other.currency THEN
            RETURN money_t (self.amount + p_other.amount, self.currency);
        ELSE
            RETURN money_t (self.get_normalized + p_other.get_normalized, 'USD');
        END IF;
    END ADD;

    MEMBER FUNCTION to_string RETURN VARCHAR2 IS
    BEGIN
        RETURN TO_CHAR (self.amount, 'FM999,999,999,990.00') || ' ' || self.currency;
    END to_string;

END;
/

-- [TEST-014] COMPOUND TRIGGER (Oracle 11g+) (예상: 1 실행단위)
-- 난이도: ★★★★★

CREATE OR REPLACE TRIGGER trg_employee_compound
    FOR INSERT OR UPDATE OR DELETE ON employees COMPOUND TRIGGER TYPE emp_id_set_t IS
    TABLE OF NUMBER INDEX BY PLS_INTEGER;
    g_emp_ids emp_id_set_t;
    g_idx PLS_INTEGER := 0;
    BEFORE STATEMENT IS
    BEGIN
        g_emp_ids.
        DELETE;
        g_idx := 0;
        DBMS_OUTPUT.PUT_LINE ('--- Statement Start ---');
    END BEFORE STATEMENT;
    BEFORE EACH ROW IS
    BEGIN
        IF INSERTING THEN
            :NEW.created_date := SYSDATE;
            :NEW.created_by := USER;
        ELSIF UPDATING THEN
            :NEW.modified_date := SYSDATE;
            IF :OLD.salary != :NEW.salary THEN
                g_idx := g_idx + 1;
                g_emp_ids (g_idx) := :NEW.employee_id;
            END IF;
        END IF;
    END BEFORE EACH ROW;
    AFTER EACH ROW IS
    BEGIN
        INSERT INTO employee_history (employee_id, action, old_salary, new_salary, change_date)
        VALUES (NVL (:NEW.employee_id, :OLD.employee_id), 
            CASE
                WHEN INSERTING THEN
                    'INSERT'
                    WHEN UPDATING THEN
                        'UPDATE'
                    WHEN DELETING THEN
                        'DELETE'
                END, :OLD.salary, :NEW.salary, SYSTIMESTAMP);
    END AFTER EACH ROW;
    AFTER STATEMENT IS
    BEGIN
        IF g_idx > 0 THEN
            FORALL i IN 1..g_idx
            UPDATE salary_audit
            SET last_change = SYSTIMESTAMP
            WHERE employee_id = g_emp_ids (i);
        END IF;
        DBMS_OUTPUT.PUT_LINE ('--- Statement End: ' || g_idx || ' salary changes ---');
    END AFTER STATEMENT;
END trg_employee_compound;
/

-- [TEST-015] CREATE FUNCTION with PIPELINED + PARALLEL_ENABLE (예상: 1 실행단위)
-- 난이도: ★★★★☆

CREATE OR REPLACE FUNCTION pipe_transform (p_cursor IN SYS_REFCURSOR) RETURN output_tab_t PIPELINED PARALLEL_ENABLE (PARTITION p_cursor BY ANY) IS
    TYPE input_tab_t IS TABLE OF input_rec_t;
    l_input input_tab_t;
    l_output output_rec_t;
BEGIN
    LOOP
        FETCH p_cursor BULK COLLECT INTO l_input LIMIT 500;
        EXIT WHEN l_input.COUNT = 0;
        FOR i IN 1..l_input.COUNT LOOP
            l_output.id := l_input (i).id;
            l_output.value :=
            CASE
                WHEN l_input (i).category = 'A' THEN
                    l_input (i).amount * 1.1
                WHEN l_input (i).category = 'B' THEN
                    l_input (i).amount * 1.2
                ELSE
                    l_input (i).amount
            END;
            l_output.processed_at := SYSTIMESTAMP;
            PIPE ROW (l_output);
        END LOOP;
    END LOOP;
    CLOSE p_cursor;
    RETURN;
EXCEPTION
    WHEN NO_DATA_NEEDED THEN
        IF p_cursor%ISOPEN THEN
            CLOSE p_cursor;
        END IF;
    WHEN OTHERS THEN
        IF p_cursor%ISOPEN THEN
            CLOSE p_cursor;
        END IF;
        RAISE;
END pipe_transform;
/

-- ============================================================================
-- CATEGORY 5: 복합 DML 지옥 (Complex DML Hell)
-- ============================================================================
-- [TEST-016] MERGE + 서브쿼리 + CASE (예상: 1 실행단위)
-- 난이도: ★★★☆☆

MERGE /*+ USE_HASH(t s) */
INTO target_table t
USING (
        SELECT s.id,
            s.NAME,
            CASE
                WHEN s.score >= (
                    SELECT AVG (score)
                    FROM source_table
                    WHERE active = 'Y'
                ) THEN 'ABOVE'
                ELSE 'BELOW'
            END AS category,
            (
                SELECT MAX (update_date)
                FROM history_table h
                WHERE h.source_id = s.id
            ) AS last_update
        FROM source_table s
        WHERE s.status IN (
            SELECT status_code
            FROM valid_statuses
            WHERE effective_date <= SYSDATE
                AND NVL (expiry_date, SYSDATE + 1) > SYSDATE
        )
    ) src
ON (t.id = src.id)
WHEN MATCHED THEN
    UPDATE SET t.NAME = src.NAME,
    t.category = src.category,
    t.last_update = src.last_update,
    t.modified = SYSTIMESTAMP
    WHERE t.NAME != src.NAME
        OR t.category != src.category
    DELETE
    WHERE t.category = 'BELOW'
        AND t.last_update < ADD_MONTHS (SYSDATE, - 12)
WHEN NOT MATCHED THEN
    INSERT (id, NAME, category, last_update, created) VALUES (src.id, src.NAME, src.category, src.last_update, SYSTIMESTAMP)
    WHERE src.category = 'ABOVE';

-- [TEST-017] INSERT ALL / CONDITIONAL INSERT (예상: 1 실행단위)
-- 난이도: ★★★☆☆

INSERT ALL
    WHEN total_amount > 10000 THEN
        INTO high_value_orders (order_id, customer_id, amount, order_date)
        VALUES (oid, cid, total_amount, odate)
        INTO vip_notifications (customer_id, message, created)
        VALUES (cid, 'High value order: ' || TO_CHAR (total_amount, 'FM$999,999.00'), SYSDATE)
    WHEN total_amount BETWEEN 1000 AND 10000 THEN
        INTO medium_value_orders (order_id, customer_id, amount)
        VALUES (oid, cid, total_amount)
    WHEN category = 'ELECTRONICS' THEN
        INTO electronics_orders (order_id, amount, warranty_end)
        VALUES (oid, total_amount, ADD_MONTHS (odate, 
    CASE
        WHEN total_amount > 5000 THEN 24
            ELSE 12
    END
    ))
    ELSE
        INTO standard_orders (order_id, customer_id, amount)
        VALUES (oid, cid, total_amount)
SELECT o.order_id AS oid,
    o.customer_id AS cid,
    o.total_amount,
    o.order_date AS odate,
    p.category
FROM orders o
JOIN products p
    ON o.product_id = p.product_id
WHERE o.order_date >= TRUNC (SYSDATE, 'MM');

-- [TEST-018] WITH 절 + 재귀 CTE + DML (예상: 1 실행단위)
-- 난이도: ★★★★☆

WITH
    FUNCTION calc_depth (p_id NUMBER) RETURN NUMBER IS v_depth NUMBER;

BEGIN
    SELECT MAX (LEVEL)
    INTO v_depth
    FROM org_tree
    START WITH parent_id IS NULL
    CONNECT BY PRIOR node_id = parent_id;
    RETURN v_depth;
END calc_depth;

recursive_tree (node_id, parent_id, node_name, DEPTH, PATH) AS (
    SELECT node_id,
        parent_id,
        node_name,
        1 AS DEPTH,
        CAST (node_name AS VARCHAR2 (4000)) AS PATH
    FROM org_tree
    WHERE parent_id IS NULL
    UNION ALL
    SELECT t.node_id,
        t.parent_id,
        t.node_name,
        rt.DEPTH + 1,
        rt.PATH || ' > ' || t.node_name
    FROM org_tree t
    JOIN recursive_tree rt
        ON t.parent_id = rt.node_id
    WHERE rt.DEPTH < calc_depth (t.node_id)
),
    aggregated AS (
        SELECT parent_id,
            COUNT (*) AS child_count,
            MAX (DEPTH) AS max_depth,
            LISTAGG (node_name, ', ') WITHIN GROUP (ORDER BY node_name) AS children
        FROM recursive_tree
        WHERE DEPTH > 1
        GROUP BY parent_id
    )
SELECT rt.*,
    a.child_count,
    a.max_depth,
    a.children
FROM recursive_tree rt
LEFT JOIN aggregated a
    ON rt.node_id = a.parent_id
ORDER BY rt.PATH;

-- ============================================================================
-- CATEGORY 6: EXECUTE IMMEDIATE 인셉션 (Dynamic SQL Inception)
-- ============================================================================
-- [TEST-019] 다층 동적 SQL (예상: 1 실행단위)
-- 난이도: ★★★★★

DECLARE
    v_outer_sql CLOB;
    v_result NUMBER;
BEGIN
    -- 동적 SQL이 또 다른 동적 SQL을 생성하는 구조
    v_outer_sql := q'[
        DECLARE
            v_inner_sql VARCHAR2(4000);
            v_val       NUMBER;
        BEGIN
            v_inner_sql := 'BEGIN '
                || 'EXECUTE IMMEDIATE ''SELECT COUNT(*) FROM '
                || 'dual WHERE 1=1'' INTO :out; '
                || 'END;';
            EXECUTE IMMEDIATE v_inner_sql USING OUT v_val;
            :result := v_val;
        END;
    ]';
    EXECUTE IMMEDIATE v_outer_sql USING OUT v_result;
    DBMS_OUTPUT.PUT_LINE ('Result: ' || v_result);
    -- DDL via 동적 SQL
    EXECUTE IMMEDIATE 'CREATE TABLE temp_' || TO_CHAR (SYSDATE, 'YYYYMMDD') || ' AS
        SELECT * FROM (
            SELECT e.*, ROW_NUMBER() OVER (PARTITION BY department_id ORDER BY salary DESC) rn
            FROM employees e
        ) WHERE rn <= 5';
    -- 동적 PL/SQL 블록 실행
    EXECUTE IMMEDIATE '
        BEGIN
            FOR r IN (SELECT table_name FROM user_tables WHERE table_name LIKE ''TEMP_%'') LOOP
                EXECUTE IMMEDIATE ''DROP TABLE '' || r.table_name || '' PURGE'';
            END LOOP;
        END;
    ';
END;
/

-- ============================================================================
-- CATEGORY 7: 극한의 서브쿼리 (Extreme Subqueries)
-- ============================================================================
-- [TEST-020] 5단계 중첩 서브쿼리 + 스칼라 서브쿼리 (예상: 1 실행단위)
-- 난이도: ★★★★★

SELECT
    (
        SELECT department_name
        FROM departments d
        WHERE d.department_id = (
            SELECT department_id
            FROM employees e2
            WHERE e2.employee_id = (
                SELECT MAX (employee_id)
                FROM employees e3
                WHERE e3.salary > (
                    SELECT AVG (salary)
                    FROM employees e4
                    WHERE e4.department_id = (
                        SELECT MIN (department_id)
                        FROM departments d2
                        WHERE d2.location_id IN (
                                SELECT location_id
                                FROM locations
                                WHERE country_id = 'US'
                            )
                    )
                )
            )
        )
    ) AS deepest_dept_name,
    e.employee_id,
    e.salary,
    (
        SELECT COUNT (*)
        FROM (
                SELECT 1
                FROM employees e5
                WHERE e5.manager_id = e.employee_id
                    AND EXISTS (
                    SELECT 1
                    FROM employees e6
                    WHERE e6.manager_id = e5.employee_id
                        AND e6.salary > (
                            SELECT PERCENTILE_CONT (0.75) WITHIN GROUP (ORDER BY salary)
                            FROM employees e7
                            WHERE e7.department_id = e5.department_id
                        )
                )
            )
    ) AS deep_report_count
FROM employees e
WHERE e.hire_date >= (
    SELECT ADD_MONTHS (MIN (hire_date), 12)
    FROM employees
    WHERE department_id = e.department_id
)
ORDER BY e.salary DESC
FETCH FIRST 10 ROWS ONLY;

-- [TEST-021] LATERAL 인라인 뷰 + CROSS/OUTER APPLY (예상: 1 실행단위)
-- 난이도: ★★★★☆

SELECT d.department_name,
    emp_stats.avg_sal,
    emp_stats.emp_count,
    top_emp.employee_name,
    top_emp.salary
FROM departments d
CROSS APPLY (
        SELECT AVG (e.salary) AS avg_sal,
            COUNT (*) AS emp_count,
            MAX (e.salary) AS max_sal
        FROM employees e
        WHERE e.department_id = d.department_id
        HAVING COUNT (*) > 5
    ) emp_stats OUTER APPLY (
            SELECT e2.first_name || ' ' || e2.last_name AS employee_name,
                e2.salary
            FROM employees e2
            WHERE e2.department_id = d.department_id
                AND e2.salary = emp_stats.max_sal
    FETCH FIRST 1 ROW ONLY
        ) top_emp
WHERE emp_stats.avg_sal > (
    SELECT AVG (salary)
    FROM employees
);

-- ============================================================================
-- CATEGORY 8: 연속 실행단위 혼합 (Mixed Execution Units)
-- ============================================================================
-- [TEST-022] DDL → DML → PL/SQL → DML 연속 (예상: 6 실행단위)
-- 난이도: ★★★★☆

CREATE TABLE test_split_1 (
    id     NUMBER        GENERATED ALWAYS AS IDENTITY,
    NAME   VARCHAR2(100),
    status VARCHAR2(20)  DEFAULT 'ACTIVE',
    CONSTRAINT pk_test_split_1 PRIMARY KEY(id)
);

CREATE INDEX idx_test_split_1_status ON test_split_1 (status);

INSERT INTO test_split_1 (NAME, status)
SELECT 'Item ' || LEVEL,
    CASE MOD (LEVEL, 3)
        WHEN 0 THEN 'ACTIVE'
        WHEN 1 THEN 'INACTIVE'
        ELSE 'PENDING'
    END
FROM DUAL
CONNECT BY LEVEL <= 1000;

COMMIT;

BEGIN
    FOR r IN (
        SELECT id,
            NAME
        FROM test_split_1
        WHERE status = 'PENDING'
    ) LOOP
        UPDATE test_split_1
        SET status = 'PROCESSED'
        WHERE id = r.id;
    END LOOP;
    COMMIT;
END;
/

SELECT status,
    COUNT (*) AS cnt
FROM test_split_1
GROUP BY status
ORDER BY cnt DESC;

-- [TEST-023] PACKAGE SPEC → PACKAGE BODY 연속 (예상: 2 실행단위)
-- 난이도: ★★★★☆

CREATE OR REPLACE PACKAGE util_pkg AS
    TYPE string_table_t IS TABLE OF VARCHAR2 (4000);
    TYPE number_table_t IS TABLE OF NUMBER;
    FUNCTION split_string (p_string IN VARCHAR2, p_delimiter IN VARCHAR2 DEFAULT ',') RETURN string_table_t PIPELINED;
    FUNCTION running_total (p_numbers IN number_table_t) RETURN number_table_t PIPELINED;
    gc_max_length CONSTANT NUMBER := 4000;
    PROCEDURE batch_process (p_table_name IN VARCHAR2, p_batch_size IN NUMBER DEFAULT 500, p_callback IN VARCHAR2 DEFAULT NULL);
END util_pkg;
/

CREATE OR REPLACE PACKAGE BODY util_pkg AS
    FUNCTION split_string (p_string IN VARCHAR2, p_delimiter IN VARCHAR2 DEFAULT ',') RETURN string_table_t PIPELINED IS
        v_start PLS_INTEGER := 1;
        v_end PLS_INTEGER;
    BEGIN
        IF p_string IS NULL THEN
            RETURN;
        END IF;
        LOOP
            v_end := INSTR (p_string, p_delimiter, v_start);
            IF v_end = 0 THEN
                PIPE ROW (SUBSTR (p_string, v_start));
                EXIT;
            ELSE
                PIPE ROW (SUBSTR (p_string, v_start, v_end - v_start));
                v_start := v_end + LENGTH (p_delimiter);
            END IF;
        END LOOP;
        RETURN;
    END split_string;

    FUNCTION running_total (p_numbers IN number_table_t) RETURN number_table_t PIPELINED IS
        v_sum NUMBER := 0;
    BEGIN
        IF p_numbers IS NOT NULL THEN
            FOR i IN 1..p_numbers.COUNT LOOP
                v_sum := v_sum + p_numbers (i);
                PIPE ROW (v_sum);
            END LOOP;
        END IF;
        RETURN;
    END running_total;

    PROCEDURE batch_process (p_table_name IN VARCHAR2, p_batch_size IN NUMBER DEFAULT 500, p_callback IN VARCHAR2 DEFAULT NULL) IS
        v_sql CLOB;
        v_count NUMBER;
        v_offset NUMBER := 0;
    BEGIN
        EXECUTE IMMEDIATE 'SELECT COUNT(*) FROM ' || DBMS_ASSERT.SIMPLE_SQL_NAME (p_table_name) INTO v_count;
        WHILE v_offset < v_count LOOP
            v_sql := 'BEGIN ' || NVL (p_callback, 'NULL') || '; ' || 'END;';
            EXECUTE IMMEDIATE v_sql;
            v_offset := v_offset + p_batch_size;
            COMMIT;
        END LOOP;
    END batch_process;
END util_pkg;
/
/*
-- ============================================================================
-- CATEGORY 9: 조건부 컴파일과 프리프로세서 (Conditional Compilation)
-- ============================================================================
-- [TEST-024] $IF, $THEN, $ELSE, $END (예상: 1 실행단위)
-- 난이도: ★★★★★

CREATE OR REPLACE PACKAGE BODY conditional_pkg AS
    PROCEDURE debug_proc IS
    BEGIN
        $IF $$debug_mode $THEN
            DBMS_OUTPUT.PUT_LINE('Debug ON');
            $IF $$ verbose $THEN DBMS_OUTPUT.PUT_LINE ('Verbose mode');
        FOR r IN (
            SELECT *
            FROM debug_settings
        ) LOOP
            DBMS_OUTPUT.PUT_LINE (r.KEY || '=' || r.value);
        END LOOP;
        $ELSE DBMS_OUTPUT.PUT_LINE ('Brief mode');
        $END $ELSE NULL; -- production: no debug output
        $END $IF DBMS_DB_VERSION.VERSION >= 12 $THEN
        -- 12c+ feature
        DECLARE
            v_id NUMBER;
        BEGIN
            v_id := my_seq.NEXTVAL;
    END;
        $ELSE
        -- Pre-12c fallback
        DECLARE
            v_id NUMBER;
        BEGIN
            SELECT my_seq.NEXTVAL
            INTO v_id
            FROM DUAL;
    END;
        $END $ERROR 'This should not compile' $END
    END debug_proc;
    END conditional_pkg;

        /
        -- ============================================================================
        -- CATEGORY 10: CREATE JAVA SOURCE (예상: 1 실행단위)
        -- ============================================================================
        -- [TEST-025] Java 소스 내 세미콜론 (예상: 1 실행단위)
        -- 난이도: ★★★★★
        CREATE OR REPLACE
        AND COMPILE JAVA SOURCE NAMED "OracleHelper" AS import JAVA.sql.*;

        import JAVA.util.*;

        PUBLIC CLASS OracleHelper { / / 세미콜론이 가득한 JAVA 코드 PRIVATE STATIC FINAL String SQL = "SELECT * FROM dual; -- not a delimiter";

        PRIVATE STATIC FINAL String BLOCK = "BEGIN NULL; END; /";

        PUBLIC STATIC String processData (String input) throws SQLException { Connection CONN = NULL;

        PreparedStatement stmt = NULL;

        ResultSet rs = NULL;

        StringBuilder result = new StringBuilder ();

        try { CONN = DriverManager.getConnection ("jdbc:default:connection:") stmt = CONN.prepareStatement ("SELECT column_value FROM TABLE(string_split(?, ','))");

        stmt.setString (1, input);

        rs = stmt.executeQuery ();

        WHILE (rs.NEXT ()) {
        IF (result.length () > 0) result.APPEND (";");
        result.APPEND (rs.getString (1).trim ());
        } / / NESTED try - catch try { stmt = CONN.prepareStatement ("INSERT INTO process_log VALUES(?, SYSTIMESTAMP)");
        stmt.setString (1, result.toString ());
        stmt.executeUpdate ();
        } catch (SQLException e) { / / Log but don 't fail; END; is not PL/SQL
                System.err.println("Log failed: " + e.getMessage());
            }

        } finally {
            if (rs != null) try { rs.close(); } catch (Exception e) {}
            if (stmt != null) try { stmt.close(); } catch (Exception e) {}
            // Don' t CLOSE connection - it 's the default connection
        }

        return result.toString();
    }

    public static int calculate(int[] values) {
        int sum = 0;
        for (int v : values) {
            switch (v % 3) {
                case 0: sum += v; break;
                case 1: sum += v * 2; break;
                default: sum -= v; break;
            }
        }
        return sum;
    }
}'; 

*/

/
        -- ============================================================================
        -- CATEGORY 11: 세미콜론 vs 슬래시 종결자 함정 (Terminator Traps)
        -- ============================================================================
        -- [TEST-026] 슬래시(/)가 나눗셈인 경우 vs 종결자인 경우 (예상: 2 실행단위)
        -- 난이도: ★★★★★
        SELECT employee_id,
            salary / 12 AS monthly_salary,
            commission_pct / 100 AS commission_rate,
            (salary * 12 + NVL (salary * commission_pct / 100, 0)) / (
                SELECT AVG (salary * 12)
                FROM employees
            ) AS salary_ratio
                FROM employees
                WHERE department_id = 50
                ORDER BY salary /
                SELECT 1 / 2 AS half
                FROM DUAL;

        -- [TEST-027] 줄 시작의 슬래시만 종결자 (예상: 3 실행단위)
        -- 난이도: ★★★★☆
        BEGIN
        NULL;
    END;
        /
        SELECT 100 / 2 AS fifty,
            200 / 4 AS also_fifty
        FROM DUAL;
        SELECT ' done '
        FROM DUAL;
        -- ============================================================================
        -- CATEGORY 12: 멀티바이트/유니코드 식별자 (Unicode Identifiers)
        -- ============================================================================
        -- [TEST-028] 한글 식별자 + 특수문자 (예상: 1 실행단위)
        -- 난이도: ★★★☆☆
        CREATE OR REPLACE PROCEDURE "직원_처리" ("부서코드" IN NUMBER, "처리구분" IN VARCHAR2) AS
        "총건수" NUMBER := 0;
        "처리결과" VARCHAR2 (4000);
        BEGIN
        FOR "직원" IN (
            SELECT "사번",
                "이름",
                "급여"
            FROM "직원테이블"
            WHERE "부서코드" = "직원_처리"."부서코드"
        ) LOOP
            "총건수" := "총건수" + 1;
            "처리결과" := "처리결과" || "직원"."이름" || ';

-- VARCHAR2
-- NUMBER
-- DATE
-- 전부 주석
-- 이 END는 CASE의 END
-- 이 END; + /가 진짜 종결
-- outer CASE
-- nested exception handler block
-- CASE 문 (표현식이 아닌 문)
-- 진짜 마지막 방어

';
        END LOOP;
        INSERT INTO "처리이력" ("구분", "건수", "결과", "처리일시")
        VALUES ("처리구분", "총건수", "처리결과", SYSTIMESTAMP);
        COMMIT;
    END "직원_처리";
        /
        -- ============================================================================
        -- CATEGORY 13: 에디션/ACCESSIBLE BY/SHARING (12c+ 문법)
        -- ============================================================================
        -- [TEST-029] EDITIONABLE + ACCESSIBLE BY + SHARING (예상: 1 실행단위)
        -- 난이도: ★★★★☆
        CREATE OR REPLACE EDITIONABLE PACKAGE secure_pkg ACCESSIBLE BY (PACKAGE admin_pkg, 
        PROCEDURE maintenance_proc) SHARING = METADATA AS
        FUNCTION validate_access (p_user IN VARCHAR2) RETURN BOOLEAN;
        PROCEDURE grant_temporary_access (p_user IN VARCHAR2, p_duration IN INTERVAL DAY TO SECOND DEFAULT INTERVAL ' 1 ' HOUR);
        FUNCTION get_access_token (p_user IN VARCHAR2) RETURN RAW DETERMINISTIC RESULT_CACHE;
    END secure_pkg;
        /
        -- ============================================================================
        -- CATEGORY 14: ALTER + 멀티라인 DDL 혼합
        -- ============================================================================
        -- [TEST-030] ALTER TABLE 연속 + 제약조건 추가/수정 (예상: 4 실행단위)
        -- 난이도: ★★★☆☆
        ALTER TABLE orders ADD (tracking_number VARCHAR2 (50), estimated_delivery DATE, delivery_notes CLOB, CONSTRAINT chk_delivery CHECK (estimated_delivery >= order_date));
        ALTER TABLE orders MODIFY (status VARCHAR2 (30) DEFAULT ' NEW ' NOT NULL, total_amount NUMBER (15, 2));
        ALTER TABLE orders DROP CONSTRAINT fk_customer CASCADE;
        ALTER TABLE orders SPLIT PARTITION orders_2024
        INTO (PARTITION orders_2024_h1
        VALUES LESS THAN (TO_DATE (' 2024 - 07 - 01 ', ' YYYY - MM - DD ')), PARTITION orders_2024_h2
        VALUES LESS THAN (TO_DATE (
        ' 2025 - 01 - 01 ', ' YYYY - MM - DD '
            )));
        -- ============================================================================
        -- CATEGORY 15: 극한의 WITH 절 (CTE Extreme)
        -- ============================================================================
        -- [TEST-031] WITH + PL/SQL 함수 + 다중 CTE + SEARCH/CYCLE (예상: 1 실행단위)
        -- 난이도: ★★★★★
        WITH
            FUNCTION weighted_score (p_base NUMBER, p_weight NUMBER) RETURN NUMBER IS
            BEGIN
            RETURN ROUND (p_base * p_weight / NULLIF (
            (
                SELECT SUM (weight)
                FROM weight_config
                WHERE active = ' Y '
            ), 0
        ), 4);
    EXCEPTION
        WHEN ZERO_DIVIDE THEN
            RETURN 0;
    END weighted_score;

        FUNCTION normalize (p_val NUMBER, p_min NUMBER, p_max NUMBER) RETURN NUMBER IS
        BEGIN
        IF p_max = p_min THEN
            RETURN 0.5;
    END IF;
        RETURN (p_val - p_min) / (p_max - p_min);
    END normalize;
        base_data AS (
            SELECT
                id,
                val,
                category,
                ROW_NUMBER () OVER (PARTITION BY category ORDER BY val DESC) AS rn
            FROM raw_data
            WHERE status = ' VALID '
        ),
        stats AS (
            SELECT
                category,
                MIN (val) AS min_val,
                MAX (val) AS max_val,
                AVG (val) AS avg_val,
                STDDEV (val) AS std_val,
                COUNT (*) AS cnt
            FROM base_data
            GROUP BY category
        ),
        normalized AS (
            SELECT
                b.id,
                b.category,
                normalize (b.val, s.min_val, s.max_val) AS norm_val,
                weighted_score (b.val, w.weight) AS w_score
            FROM base_data b
            JOIN stats s
                ON b.category = s.category
            LEFT JOIN weight_config w
                ON b.category = w.category
                    AND w.active = ' Y '
                ),
                hierarchy (id, parent_id, NAME, DEPTH) AS (
                        SELECT
                            id,
                            parent_id,
                            NAME,
                            1
                        FROM categories
                        WHERE parent_id IS NULL
                        UNION ALL
                        SELECT
                            c.id,
                            c.parent_id,
                            c.NAME,
                            h.DEPTH + 1
                        FROM categories c
                        JOIN hierarchy h
                            ON c.parent_id = h.id
                        ) SEARCH DEPTH FIRST BY NAME SET order_col CYCLE id SET is_cycle TO ' Y ' DEFAULT ' N'
        SELECT
            h.NAME AS category_path,
                            n.norm_val,
                            n.w_score,
                            s.cnt,
                            CASE
                                WHEN h.is_cycle = ' Y ' THEN ' CIRCULAR REF ! '
                                ELSE ' OK '
                            END AS status
        FROM hierarchy h
        JOIN normalized n
        ON h.id = n.id
        JOIN stats s
        ON n.category = s.category
        WHERE h.is_cycle = ' N'
        ORDER BY h.order_col;
        -- ============================================================================
        -- CATEGORY 16: 프라그마/예외 처리 복합 (Pragma & Exception Complex)
        -- ============================================================================
        -- [TEST-032] 다중 PRAGMA + 사용자 예외 + RAISE_APPLICATION_ERROR (예상: 1 실행단위)
        -- 난이도: ★★★★☆
        CREATE OR REPLACE PACKAGE BODY exception_demo_pkg AS
        e_business_rule EXCEPTION;
        e_data_integrity EXCEPTION;
        e_resource_busy EXCEPTION;
        PRAGMA EXCEPTION_INIT (e_business_rule, - 20100);
        PRAGMA EXCEPTION_INIT (e_data_integrity, - 20200);
        PRAGMA EXCEPTION_INIT (e_resource_busy, - 20300);
        PROCEDURE complex_transaction (p_id IN NUMBER) IS
            PRAGMA AUTONOMOUS_TRANSACTION;
            v_status VARCHAR2 (20);
            v_retry NUMBER := 0;
            c_max_retry CONSTANT NUMBER := 3;
            PROCEDURE log_attempt (p_msg VARCHAR2) IS
                PRAGMA AUTONOMOUS_TRANSACTION;
            BEGIN
                INSERT INTO transaction_log (id, msg, ts)
                VALUES (p_id, p_msg, SYSTIMESTAMP);
                COMMIT;
    END;
        BEGIN
            <<retry_block>>
            LOOP
                BEGIN
                    SAVEPOINT before_operation;
        SELECT status
        INTO v_status
        FROM resources
        WHERE id = p_id
                FOR
        UPDATE NOWAIT;
                    IF v_status = ' LOCKED ' THEN
        RAISE e_resource_busy;
    ELSIF v_status = ' INVALID ' THEN
        RAISE e_data_integrity;
    END IF;
        UPDATE resources
        SET status = ' PROCESSING ',
            modified = SYSTIMESTAMP
        WHERE id = p_id;
                    -- 비즈니스 로직
                    BEGIN
        validate_business_rules (p_id);
    EXCEPTION
        WHEN e_business_rule THEN
                        log_attempt (' Business rule violation : ' || SQLERRM);
                        RAISE;
    END;
        UPDATE resources
        SET status = ' COMPLETE ',
            modified = SYSTIMESTAMP
        WHERE id = p_id;
                    COMMIT;
                    EXIT retry_block;
    EXCEPTION
        WHEN e_resource_busy THEN
                    ROLLBACK TO before_operation;
                    v_retry := v_retry + 1;
                    log_attempt (' Retry ' || v_retry || ' OF ' || c_max_retry);
                    IF v_retry >= c_max_retry THEN
                        RAISE_APPLICATION_ERROR (- 20300, ' MAX retries exceeded
    FOR id : ' || p_id);
        END IF;
                    DBMS_LOCK.SLEEP (POWER (2, v_retry));
        WHEN e_data_integrity THEN
                    ROLLBACK TO before_operation;
                    log_attempt (' Data integrity ERROR ');
                    RAISE_APPLICATION_ERROR (- 20200, ' Data integrity violation
FOR id : ' || p_id, TRUE -- preserve error stack
        );
        WHEN OTHERS THEN
                    ROLLBACK TO before_operation;
                    log_attempt (' Unexpected : ' || DBMS_UTILITY.FORMAT_ERROR_STACK);
                    RAISE;
    END;
        END LOOP retry_block;
        END complex_transaction;
    END exception_demo_pkg;
        /
        -- ============================================================================
        -- CATEGORY 17: SQL*Plus 커맨드 혼합 (SQL*Plus Commands)
        -- ============================================================================
        -- [TEST-033] SQL*Plus 커맨드가 섞인 스크립트 (예상: 파서 구현에 따라 다름)
        -- 난이도: ★★★★★
        -- NOTE: SQL*Plus 커맨드를 실행단위로 볼지 여부는 구현에 따라 다르나,
        --       최소한 SQL/PLSQL 구문은 올바르게 분리되어야 함
        SET SERVEROUTPUT
        ON SIZE UNLIMITED
        SET TIMING
        ON
        SET LINESIZE 200
        SET PAGESIZE 50 WHENEVER SQLERROR EXIT SQL.SQLCODE ROLLBACK PROMPT = = = Creating tables = = = CREATE TABLE sqlplus_test (
        id NUMBER,
        data VARCHAR2 (100)
        );
        PROMPT = = = Loading data = = =
        INSERT INTO sqlplus_test
        VALUES (1, ' FIRST ');
        INSERT INTO sqlplus_test
        VALUES (2, ' SECOND ');
        COMMIT;
        PROMPT = = = Running PL / SQL = = =
        DECLARE
        v_count NUMBER;
        BEGIN
                SELECT COUNT (*)
                INTO v_count
                FROM sqlplus_test;
        DBMS_OUTPUT.PUT_LINE (' COUNT : ' || v_count);
    END;
        / COLUMN id FORMAT 9999 COLUMN data FORMAT A30
        SELECT *
        FROM sqlplus_test
        ORDER BY id;
        SPOOL OFF
        SET TIMING OFF
        -- ============================================================================
        -- CATEGORY 18: VIEW/MATERIALIZED VIEW 복합 (Complex Views)
        -- ============================================================================
        -- [TEST-034] CREATE MATERIALIZED VIEW + 복합 쿼리 (예상: 1 실행단위)
        -- 난이도: ★★★★☆
        CREATE MATERIALIZED VIEW mv_sales_dashboard BUILD DEFERRED REFRESH FAST
        ON DEMAND ENABLE QUERY REWRITE AS
        WITH date_dim AS (
                SELECT
                    TRUNC (SYSDATE, ' YYYY ') + LEVEL - 1 AS cal_date,
                    TO_CHAR (TRUNC (SYSDATE, ' YYYY ') + LEVEL - 1, ' YYYY ') AS cal_year,
                    TO_CHAR (TRUNC (SYSDATE, ' YYYY ') + LEVEL - 1, ' Q') AS cal_quarter,
                    TO_CHAR (TRUNC (SYSDATE, 'YYYY') + LEVEL - 1, 'MM') AS cal_month,
                    TO_CHAR (TRUNC (SYSDATE, 'YYYY') + LEVEL - 1, 'IW') AS cal_week
                FROM DUAL
                CONNECT BY LEVEL <= 366
            ),
            sales_agg AS (
                    SELECT
                        s.product_id,
                        TRUNC (s.sale_date, 'MM') AS sale_month,
                        SUM (s.quantity) AS total_qty,
                        SUM (s.amount) AS total_amount,
                        COUNT (DISTINCT s.customer_id) AS unique_customers,
                        AVG (s.amount) AS avg_order_value,
                        PERCENTILE_CONT (0.5) WITHIN GROUP (ORDER BY s.amount) AS median_order
                    FROM sales s
                    WHERE s.sale_date >= ADD_MONTHS (TRUNC (SYSDATE, 'YYYY'), - 12)
                    GROUP BY s.product_id,
                        TRUNC (s.sale_date, 'MM')
                    ),
                    product_rank AS (
                            SELECT
                                sa.*,
                                DENSE_RANK () OVER (PARTITION BY sale_month ORDER BY total_amount DESC) AS revenue_rank,
                                LAG (total_amount) OVER (PARTITION BY product_id ORDER BY sale_month) AS prev_month_amount,
                                CASE
                                    WHEN LAG (total_amount) OVER (PARTITION BY product_id ORDER BY sale_month) IS NOT NULL THEN ROUND ((total_amount - LAG (total_amount) OVER (PARTITION BY product_id ORDER BY sale_month)) / NULLIF (LAG (total_amount) OVER (PARTITION BY product_id ORDER BY sale_month), 0) * 100, 2)
                                END AS mom_growth_pct
        FROM sales_agg sa
        )
        SELECT
            pr.product_id,
            p.product_name,
            p.category,
            pr.sale_month,
            pr.total_qty,
            pr.total_amount,
            pr.unique_customers,
            pr.avg_order_value,
            pr.median_order,
            pr.revenue_rank,
            pr.mom_growth_pct,
            SUM (pr.total_amount) OVER (PARTITION BY pr.product_id ORDER BY pr.sale_month ROWS BETWEEN UNBOUNDED PRECEDING AND CURRENT ROW) AS cumulative_revenue
        FROM product_rank pr
        JOIN products p
            ON pr.product_id = p.product_id;
        -- ============================================================================
        -- CATEGORY 19: DBMS_SCHEDULER + 복합 프로시저 체인
        -- ============================================================================
        -- [TEST-035] SCHEDULER JOB + CHAIN (예상: 1 실행단위)
        -- 난이도: ★★★★☆
        BEGIN
        -- Chain 정의
        DBMS_SCHEDULER.CREATE_CHAIN (chain_name => 'ETL_CHAIN', rule_set_name => NULL, evaluation_interval => NULL, comments => 'ETL processing chain');
        -- Chain 스텝 추가
        DBMS_SCHEDULER.DEFINE_CHAIN_STEP (chain_name => 'ETL_CHAIN', step_name => 'STEP_EXTRACT', program_name => 'EXTRACT_PROG');
        DBMS_SCHEDULER.DEFINE_CHAIN_STEP (chain_name => 'ETL_CHAIN', step_name => 'STEP_TRANSFORM', program_name => 'TRANSFORM_PROG');
        DBMS_SCHEDULER.DEFINE_CHAIN_STEP (chain_name => 'ETL_CHAIN', step_name => 'STEP_LOAD', program_name => 'LOAD_PROG');
        -- Chain 규칙 정의
        DBMS_SCHEDULER.DEFINE_CHAIN_RULE (chain_name => 'ETL_CHAIN', condition => 'TRUE', action => 'START "STEP_EXTRACT"', rule_name => 'RULE_START');
        DBMS_SCHEDULER.DEFINE_CHAIN_RULE (chain_name => 'ETL_CHAIN', condition => '"STEP_EXTRACT" COMPLETED', action => 'START "STEP_TRANSFORM"', rule_name => 'RULE_AFTER_EXTRACT');
        DBMS_SCHEDULER.DEFINE_CHAIN_RULE (chain_name => 'ETL_CHAIN', condition => '"STEP_TRANSFORM" COMPLETED', action => 'START "STEP_LOAD"', rule_name => 'RULE_AFTER_TRANSFORM');
        DBMS_SCHEDULER.DEFINE_CHAIN_RULE (chain_name => 'ETL_CHAIN', condition => '"STEP_LOAD" COMPLETED', action => 'END', rule_name => 'RULE_END');
        DBMS_SCHEDULER.ENABLE ('ETL_CHAIN');
    END;
        /
        -- ============================================================================
        -- CATEGORY 20: 극한의 혼합 시나리오 (Ultimate Mixed Scenarios)
        -- ============================================================================
        -- [TEST-036] 모든 것이 섞인 시나리오 1 (예상: 7 실행단위)
        -- 난이도: ★★★★★
        -- 1) 타입 생성
        CREATE OR REPLACE TYPE score_rec_t AS
        OBJECT (student_id NUMBER, subject VARCHAR2 (50), score NUMBER, grade VARCHAR2 (2), MEMBER FUNCTION is_passing RETURN BOOLEAN, MAP MEMBER FUNCTION sort_key RETURN NUMBER);
        /
        -- 2) 타입 바디
        CREATE OR REPLACE TYPE BODY score_rec_t AS
            MEMBER FUNCTION is_passing RETURN BOOLEAN IS
        BEGIN
            RETURN
        CASE
        WHEN self.score >= 60 THEN
                    TRUE
    ELSE
                FALSE
    END;
    END;
        MAP MEMBER FUNCTION sort_key RETURN NUMBER IS
        BEGIN
        RETURN self.score;
    END;
    END;

        /
        -- 3) 컬렉션 타입
        CREATE OR REPLACE TYPE score_tab_t AS
        TABLE OF score_rec_t;
        /
        -- 4) 패키지 스펙
        CREATE OR REPLACE PACKAGE grade_pkg AS
        FUNCTION calculate_grades (p_class_id NUMBER) RETURN score_tab_t PIPELINED;
        PROCEDURE generate_report (p_class_id NUMBER);
    END grade_pkg;
        /
        -- 5) 패키지 바디
        CREATE OR REPLACE PACKAGE BODY grade_pkg AS
        FUNCTION calculate_grades (p_class_id NUMBER) RETURN score_tab_t PIPELINED IS
            v_rec score_rec_t;
        BEGIN
            FOR r IN (
                SELECT
                    s.student_id,
                    sub.subject_name,
                    e.score
                FROM enrollments e
                JOIN students s
                    ON e.student_id = s.student_id
                JOIN subjects sub
                    ON e.subject_id = sub.subject_id
                WHERE e.class_id = p_class_id
            ) LOOP
                v_rec := score_rec_t (r.student_id, r.subject_name, r.score, 
                    CASE
                        WHEN r.score >= 90 THEN
                            'A'
                        WHEN r.score >= 80 THEN
                            'B'
                        WHEN r.score >= 70 THEN
                            'C'
                        WHEN r.score >= 60 THEN
                            'D'
                        ELSE
                            'F'
                    END
                    );
                PIPE ROW (v_rec);
        END LOOP;
            RETURN;
    END;

        PROCEDURE generate_report (p_class_id NUMBER) IS
            v_grades score_tab_t;
        BEGIN
            SELECT VALUE (g) BULK COLLECT INTO v_grades
            FROM TABLE (calculate_grades (p_class_id)) g;
            FOR i IN 1..v_grades.COUNT LOOP
                IF v_grades (i).is_passing () THEN
                    INSERT INTO grade_report (student_id, subject, grade, report_date)
                VALUES (v_grades (i).student_id, v_grades (i).subject, v_grades (i).grade, SYSDATE);
    ELSE
                    INSERT INTO failing_students (student_id, subject, score, flagged_date)
                VALUES (v_grades (i).student_id, v_grades (i).subject, v_grades (i).score, SYSDATE);
    END IF;
        END LOOP;
            COMMIT;
    END;
    END grade_pkg;
        /
        -- 6) 실행
        BEGIN
        grade_pkg.generate_report (101);
    END;

        /
        -- 7) 결과 조회
        SELECT *
        FROM grade_report
        WHERE report_date = TRUNC (SYSDATE)
        ORDER BY student_id,
            subject;

        -- ============================================================================
        -- CATEGORY 21: 다중 커서와 벌크 오퍼레이션
        -- ============================================================================
        -- [TEST-037] BULK COLLECT + FORALL + SAVE EXCEPTIONS (예상: 1 실행단위)
        -- 난이도: ★★★★★
        DECLARE
        TYPE emp_id_t IS TABLE OF employees.employee_id%TYPE;
        TYPE salary_t IS TABLE OF employees.salary%TYPE;
        l_emp_ids emp_id_t;
        l_salaries salary_t;
        l_errors NUMBER;
        CURSOR c_dept_employees (p_dept_id NUMBER) IS
        SELECT
            employee_id,
            salary *
            CASE
                WHEN hire_date < ADD_MONTHS (SYSDATE, - 120) THEN 1.15
                WHEN hire_date < ADD_MONTHS (SYSDATE, - 60) THEN 1.10
                ELSE 1.05
            END AS new_salary
        FROM employees
        WHERE department_id = p_dept_id
            AND status = 'ACTIVE'
        ORDER BY hire_date;
        CURSOR c_departments IS
        SELECT
            department_id,
            department_name
        FROM departments
        WHERE active_flag = 'Y'
        ORDER BY department_id;
        BEGIN
        FOR dept IN c_departments LOOP
        OPEN c_dept_employees (dept.department_id);
        LOOP
            FETCH c_dept_employees BULK COLLECT INTO l_emp_ids,
            l_salaries LIMIT 200;
            EXIT WHEN l_emp_ids.COUNT = 0;
            BEGIN
                FORALL i IN 1..l_emp_ids.COUNT SAVE EXCEPTIONS
                UPDATE employees
                SET salary = l_salaries (i),
                    last_raise_date = SYSDATE,
                    modified_by = USER
                WHERE employee_id = l_emp_ids (i)
                    AND salary < l_salaries (i);
    EXCEPTION
        WHEN OTHERS THEN
                l_errors := SQL%BULK_EXCEPTIONS.COUNT;
                FOR j IN 1..l_errors LOOP
            INSERT INTO salary_errors (employee_id, error_code, error_msg, error_date)
            VALUES (l_emp_ids (SQL%BULK_EXCEPTIONS (j).ERROR_INDEX), SQL%BULK_EXCEPTIONS (j).ERROR_CODE, SQLERRM (- SQL%BULK_EXCEPTIONS (j).ERROR_CODE), SYSTIMESTAMP);
            END LOOP;
    END;
            COMMIT;
        END LOOP;
        CLOSE c_dept_employees;
        DBMS_OUTPUT.PUT_LINE ('Department ' || dept.department_name || ' processed');
    END LOOP;
    END;

        /
        -- ============================================================================
        -- CATEGORY 22: XMLTYPE, JSON, LOB 조작
        -- ============================================================================
        -- [TEST-038] XML/JSON 복합 쿼리 (예상: 1 실행단위)
        -- 난이도: ★★★★☆
        SELECT
            x.employee_id,
            XMLQUERY ('for $i in /employees/employee
         where $i/salary > 5000
         return <result>
             <name>{$i/name/text()}</name>
             <bonus>{$i/salary * 0.1}</bonus>
         </result>' PASSING x.xml_data RETURNING CONTENT) AS xml_result,
        JSON_OBJECT (KEY 'id' VALUE x.employee_id, KEY 'name' VALUE x.emp_name, KEY 'details' VALUE JSON_OBJECT (KEY 'salary' VALUE x.salary, KEY 'department' VALUE x.dept_name, KEY 'skills' VALUE (
            SELECT JSON_ARRAYAGG (JSON_OBJECT (KEY 'skill' VALUE s.skill_name, KEY 'level' VALUE s.proficiency) ORDER BY s.proficiency DESC
                RETURNING CLOB)
            FROM employee_skills s
            WHERE s.employee_id = x.employee_id
        )), KEY 'metadata' VALUE JSON_OBJECT (KEY 'generated' VALUE TO_CHAR (SYSTIMESTAMP, 'YYYY-MM-DD"T"HH24:MI:SS.FF3"Z"'), KEY 'version' VALUE '2.0') RETURNING CLOB) AS json_output
            FROM (
                    SELECT
                        e.employee_id,
                        e.first_name || ' ' || e.last_name AS emp_name,
                        e.salary,
                        d.department_name AS dept_name,
                        XMLTYPE ('<employees><employee><name>' || e.first_name || '</name>' || '<salary>' || e.salary || '</salary></employee></employees>') AS xml_data
                    FROM employees e
                    JOIN departments d
                        ON e.department_id = d.department_id
                    WHERE e.salary > (
                                    SELECT AVG (salary)
                                    FROM employees
                                )
                            ) x
                                    WHERE XMLEXISTS ('/employees/employee[salary > 10000]' PASSING x.xml_data)
                                    ORDER BY x.salary DESC
        FETCH FIRST 20 ROWS ONLY;

        -- ============================================================================
        -- CATEGORY 23: FLASHBACK / TEMPORAL 쿼리
        -- ============================================================================
        -- [TEST-039] FLASHBACK + VERSIONS BETWEEN + AS OF (예상: 2 실행단위)
        -- 난이도: ★★★☆☆
        SELECT
            employee_id,
                    salary,
                    versions_starttime AS change_start,
                    versions_endtime AS change_end,
                    versions_operation AS operation,
                    CASE versions_operation
                        WHEN 'I' THEN 'Inserted'
                        WHEN 'U' THEN 'Updated'
                        WHEN 'D' THEN 'Deleted'
                    END AS operation_desc
        FROM employees VERSIONS BETWEEN TIMESTAMP SYSTIMESTAMP - INTERVAL '7' DAY AND SYSTIMESTAMP
        WHERE department_id = 50
        ORDER BY versions_starttime;

        SELECT
            e_now.employee_id,
            e_now.salary AS current_salary,
            e_past.salary AS past_salary,
            e_now.salary - e_past.salary AS salary_change
        FROM employees e_now
        JOIN employees AS OF TIMESTAMP (SYSTIMESTAMP - INTERVAL '30' DAY) e_past
            ON e_now.employee_id = e_past.employee_id
        WHERE e_now.salary != e_past.salary
        ORDER BY salary_change DESC;

        -- ============================================================================
        -- CATEGORY 24: 정규표현식 + 분석함수 복합
        -- ============================================================================
        -- [TEST-040] REGEXP + Window Functions + MODEL (예상: 1 실행단위)
        -- 난이도: ★★★★★
        SELECT *
        FROM (
                SELECT
                    product_id,
                    product_name,
                    category,
                    price,
                    REGEXP_REPLACE (REGEXP_SUBSTR (product_name, '[A-Z][a-z]+(;[A-Z][a-z]+)*', 1, 1), ';', ' / ') AS parsed_name,
                    SUM (price) OVER (PARTITION BY category ORDER BY price ROWS BETWEEN 2 PRECEDING AND 2 FOLLOWING) AS moving_sum,
                    RATIO_TO_REPORT (price) OVER (PARTITION BY category) AS price_ratio,
                    NTILE (4) OVER (ORDER BY price) AS price_quartile,
                    LISTAGG (
                        CASE
                            WHEN REGEXP_LIKE (tag, '^(END|BEGIN|CREATE);?$') THEN '[keyword]'
                            ELSE tag
                        END, ', ') WITHIN
                        GROUP (ORDER BY tag) OVER (PARTITION BY category) AS all_tags
                        FROM products p
        LEFT JOIN product_tags pt
        ON p.product_id = pt.product_id
        WHERE REGEXP_LIKE (product_name, '^[A-Z]{2,}-\d{3,}')
            AND price > 0
            )
        MODEL
        PARTITION BY (category)
        DIMENSION BY (product_id)
        MEASURES (price, price_ratio, 0 AS adjusted_price)
        RULES (adjusted_price [ ANY ] =
        CASE
            WHEN price_ratio [ CV () ] > 0.3 THEN
                price [ CV () ] * 0.9
            WHEN price_ratio [ CV () ] < 0.05 THEN
                price [ CV () ] * 1.1
            ELSE
                price [ CV () ]
        END
                )
        ORDER BY category,
            adjusted_price DESC;

        -- ============================================================================
        -- CATEGORY 25: 마지막 보스 - 모든 함정이 한 파일에 (Final Boss)
        -- ============================================================================
        -- [TEST-041] 문자열 안의 슬래시 종결자 미끼 (예상: 1 실행단위)
        -- 난이도: ★★★★★
        DECLARE
        v_script CLOB := q'[
BEGIN
    EXECUTE IMMEDIATE 'CREATE TABLE t AS SELECT 1 AS id FROM dual';
END;
/
INSERT INTO t VALUES(2);
COMMIT;
BEGIN
    NULL;
END;
/
]';
        -- 위의 / 들은 전부 문자열 안이므로 종결자가 아님!
        v_count NUMBER;
        BEGIN
        v_count := REGEXP_COUNT (v_script, '^/$', 1, 'm');
        DBMS_OUTPUT.PUT_LINE ('Fake terminators found: ' || v_count);
    -- 이 END; 와 아래 / 만이 진짜 종결자
    END;

        /
        -- [TEST-042] 빈 줄의 슬래시와 주석 사이 DML (예상: 3 실행단위)
        -- 난이도: ★★★★☆
        /* 이 주석 블록에는 슬래시가 포함됨
/
하지만 이건 주석 안이라 종결자 아님
*/
        SELECT 'after comment block'
        FROM DUAL;

        -- 라인 주석 아래에 바로 DML
        INSERT INTO test_t (id)
        VALUES (1);

        /* 또 다른 주석
   여러 줄
   세미콜론; 도 있고
   슬래시/ 도 있음
*/
        UPDATE test_t
        SET id = 2
        WHERE id = 1;

        -- [TEST-043] GRANT/REVOKE + 시노님 + 코멘트 연속 (예상: 6 실행단위)
        -- 난이도: ★★☆☆☆
        GRANT SELECT, INSERT, UPDATE ON employees TO hr_role;

        GRANT EXECUTE ON complex_pkg TO app_user;

        CREATE OR REPLACE PUBLIC SYNONYM emp FOR hr.employees;

        COMMENT ON TABLE employees IS 'Main employee table; stores all active and inactive records';

        COMMENT ON COLUMN employees.salary IS 'Annual salary in USD; updated during review cycle';

        REVOKE INSERT ON employees
        FROM temp_role;

        -- [TEST-044] CREATE TABLE + 인라인 제약조건 + 가상 컬럼 + 파티션 (예상: 1 실행단위)
        -- 난이도: ★★★★☆
        CREATE TABLE sales_history (
        sale_id NUMBER GENERATED BY DEFAULT
        ON NULL AS IDENTITY CONSTRAINT pk_sales_history PRIMARY KEY,
        customer_id NUMBER NOT NULL CONSTRAINT fk_sh_customer REFERENCES customers (customer_id),
        product_id NUMBER NOT NULL,
        quantity NUMBER (10) CHECK (quantity > 0),
        unit_price NUMBER (15, 2) NOT NULL,
        discount_pct NUMBER (5, 2) DEFAULT 0 CHECK (discount_pct BETWEEN 0 AND 100),
        total_amount AS (ROUND (quantity * unit_price * (1 - discount_pct / 100), 2)),
        sale_date DATE DEFAULT SYSDATE NOT NULL,
        sale_quarter AS (TO_CHAR (sale_date, 'YYYY"Q"Q')),
        status VARCHAR2 (20) DEFAULT 'COMPLETED' CONSTRAINT chk_sh_status CHECK (status IN ('PENDING', 'COMPLETED', 'CANCELLED', 'REFUNDED')),
        notes CLOB,
        METADATA JSON,
        CONSTRAINT uq_sh_order UNIQUE (customer_id, product_id, sale_date)
        ) PARTITION BY RANGE (sale_date) SUBPARTITION BY LIST (status) SUBPARTITION TEMPLATE (SUBPARTITION sp_completed
        VALUES ('COMPLETED'), SUBPARTITION sp_pending
        VALUES ('PENDING'), SUBPARTITION sp_cancelled
        VALUES ('CANCELLED'), SUBPARTITION sp_refunded
        VALUES ('REFUNDED')) (PARTITION p_2024_q1
        VALUES LESS THAN (DATE '2024-04-01'), PARTITION p_2024_q2
        VALUES LESS THAN (DATE '2024-07-01'), PARTITION p_2024_q3
        VALUES LESS THAN (DATE '2024-10-01'), PARTITION p_2024_q4
        VALUES LESS THAN (DATE '2025-01-01'), PARTITION p_future
        VALUES LESS THAN (MAXVALUE)) ENABLE ROW MOVEMENT TABLESPACE users;

        -- [TEST-045] DBMS_SQL 동적 다중 바인드 (예상: 1 실행단위)
        -- 난이도: ★★★★★
        DECLARE
        v_cursor_id INTEGER;
        v_sql CLOB;
        v_rows INTEGER;
        v_desc_tab DBMS_SQL.DESC_TAB;
        v_col_count INTEGER;
        v_varchar VARCHAR2 (4000);
        v_number NUMBER;
        v_date DATE;
        TYPE bind_rec_t IS RECORD (NAME VARCHAR2 (30), dtype VARCHAR2 (30), value VARCHAR2 (4000));
        TYPE bind_tab_t IS TABLE OF bind_rec_t;
        l_binds bind_tab_t := bind_tab_t ();
        PROCEDURE add_bind (p_name VARCHAR2, p_type VARCHAR2, p_val VARCHAR2) IS
        BEGIN
        l_binds.EXTEND;
        l_binds (l_binds.COUNT) := bind_rec_t (p_name, p_type, p_val);
    END;
        BEGIN
        add_bind (':dept_id', 'NUMBER', '50');
        add_bind (':status', 'VARCHAR2', 'ACTIVE');
        add_bind (':min_sal', 'NUMBER', '5000');
        v_sql := 'SELECT employee_id, first_name || '' '' || last_name AS name, ' || 'salary, hire_date ' || 'FROM employees ' || 'WHERE department_id = :dept_id ' || 'AND status = :status ' || 'AND salary >= :min_sal ' || 'ORDER BY salary DESC';
        v_cursor_id := DBMS_SQL.OPEN_CURSOR;
        DBMS_SQL.PARSE (v_cursor_id, v_sql, DBMS_SQL.NATIVE);
        FOR i IN 1..l_binds.COUNT LOOP
        CASE l_binds (i).dtype
        WHEN 'NUMBER' THEN
                DBMS_SQL.BIND_VARIABLE (v_cursor_id, l_binds (i).NAME, TO_NUMBER (l_binds (i).value));
        WHEN 'VARCHAR2' THEN
                DBMS_SQL.BIND_VARIABLE (v_cursor_id, l_binds (i).NAME, l_binds (i).value);
        WHEN 'DATE' THEN
                DBMS_SQL.BIND_VARIABLE (v_cursor_id, l_binds (i).NAME, TO_DATE (l_binds (i).value, 'YYYY-MM-DD'));
    END CASE;
    END LOOP;
        DBMS_SQL.DESCRIBE_COLUMNS (v_cursor_id, v_col_count, v_desc_tab);
        FOR i IN 1..v_col_count LOOP
        CASE v_desc_tab (i).col_type
        WHEN 1 THEN
                DBMS_SQL.DEFINE_COLUMN (v_cursor_id, i, v_varchar, 4000);
        WHEN 2 THEN
                DBMS_SQL.DEFINE_COLUMN (v_cursor_id, i, v_number);
        WHEN 12 THEN
                DBMS_SQL.DEFINE_COLUMN (v_cursor_id, i, v_date);
    END CASE;
    END LOOP;
        v_rows := DBMS_SQL.EXECUTE (v_cursor_id);
        WHILE DBMS_SQL.FETCH_ROWS (v_cursor_id) > 0 LOOP
        FOR i IN 1..v_col_count LOOP
        CASE v_desc_tab (i).col_type
        WHEN 1 THEN
                    DBMS_SQL.COLUMN_VALUE (v_cursor_id, i, v_varchar);
                DBMS_OUTPUT.PUT (RPAD (NVL (v_varchar, 'NULL'), 30));
        WHEN 2 THEN
                    DBMS_SQL.COLUMN_VALUE (v_cursor_id, i, v_number);
                DBMS_OUTPUT.PUT (RPAD (TO_CHAR (v_number), 15));
        WHEN 12 THEN
                    DBMS_SQL.COLUMN_VALUE (v_cursor_id, i, v_date);
                DBMS_OUTPUT.PUT (RPAD (TO_CHAR (v_date, 'YYYY-MM-DD'), 15));
    END CASE;
        END LOOP;
        DBMS_OUTPUT.NEW_LINE;
    END LOOP;
        DBMS_SQL.CLOSE_CURSOR (v_cursor_id);
    EXCEPTION
        WHEN OTHERS THEN
            IF DBMS_SQL.IS_OPEN (v_cursor_id) THEN
            DBMS_SQL.CLOSE_CURSOR (v_cursor_id);
        END IF;
        RAISE;
    END;

        /
        -- [TEST-046] 문자열+주석+중첩블록+CASE가 모두 겹침 (예상: 1 실행단위)
        -- 난이도: ★★★★★
        DECLARE
        v_sql VARCHAR2 (32767) := 'SELECT /* hint */ CASE WHEN 1=1 THEN ''yes; END;'' ELSE ''no'' END FROM dual';
        /* 주석 안에
       DECLARE
       BEGIN -- 이런 키워드가 있어도
           v_sql := 'END; /';
       END;
       /
    */
        v_result VARCHAR2 (100);
        BEGIN
        -- v_sql에 'END;' 가 있지만 문자열임
        EXECUTE IMMEDIATE v_sql INTO v_result;
        /* 여기도 함정:
       END;
    */
        v_result :=
        CASE
        WHEN v_result = 'yes; END;' THEN
        -- 비교값에도 END;
        q'[Result is: 'yes; END;' -- tricky!]'
    ELSE
        'other'
    END;
        DBMS_OUTPUT.PUT_LINE (v_result);
    END;

        /
        -- [TEST-047] PIPE ROW + 컬렉션 + TABLE() 사용 복합 (예상: 1 실행단위)
        -- 난이도: ★★★★☆
        CREATE OR REPLACE FUNCTION generate_calendar (p_start_date DATE, p_end_date DATE) RETURN date_table_t PIPELINED IS
        v_date DATE := p_start_date;
        v_rec date_rec_t;
        BEGIN
        WHILE v_date <= p_end_date LOOP
        v_rec.cal_date := v_date;
        v_rec.day_name := TO_CHAR (v_date, 'Day', 'NLS_DATE_LANGUAGE=ENGLISH');
        v_rec.week_number := TO_NUMBER (TO_CHAR (v_date, 'IW'));
        v_rec.is_weekend :=
        CASE
        WHEN TO_CHAR (v_date, 'DY', 'NLS_DATE_LANGUAGE=ENGLISH') IN ('SAT', 'SUN') THEN
                'Y'
    ELSE
            'N'
    END;
        v_rec.is_holiday :=
        CASE
        WHEN v_date IN (
                        SELECT holiday_date
                        FROM company_holidays
                        WHERE EXTRACT (YEAR
                        FROM holiday_date) = EXTRACT (YEAR
                        FROM v_date)
        ) THEN
                'Y'
            ELSE
                'N'
        END;
        v_rec.working_day_seq :=
        CASE
        WHEN v_rec.is_weekend = 'N'
                AND v_rec.is_holiday = 'N' THEN
                (
                    SELECT COUNT (*) + 1
                    FROM TABLE (generate_calendar (TRUNC (v_date, 'YYYY'), v_date - 1))
                    WHERE is_weekend = 'N'
                        AND is_holiday = 'N'
                    )
            ELSE
                NULL
        END;
        PIPE ROW (v_rec);
        v_date := v_date + 1;
    END LOOP;
        RETURN;
    END generate_calendar;

        /
        -- [TEST-048] INSTEAD OF TRIGGER on VIEW (예상: 1 실행단위)
        -- 난이도: ★★★★☆
        CREATE OR REPLACE TRIGGER trg_instead_of_emp_view
        INSTEAD OF INSERT OR UPDATE OR DELETE ON v_employee_details
        FOR EACH ROW
        DECLARE
        v_dept_id NUMBER;
        v_mgr_id NUMBER;
        BEGIN
        IF INSERTING THEN
            -- 부서 확인 또는 생성
            BEGIN
                SELECT department_id
                INTO v_dept_id
                FROM departments
                WHERE department_name = :NEW.department_name;
    EXCEPTION
        WHEN NO_DATA_FOUND THEN
            INSERT INTO departments (department_name, created_date)
            VALUES (:NEW.department_name, SYSDATE)
            RETURNING department_id INTO v_dept_id;
    END;
        -- 매니저 확인
        IF :NEW.manager_name IS
                NOT NULL THEN
                    SELECT employee_id
                    INTO v_mgr_id
                    FROM employees
                    WHERE first_name || ' ' || last_name = :NEW.manager_name
                        AND ROWNUM = 1;
    END IF;
        INSERT INTO employees (first_name, last_name, email, salary, department_id, manager_id, hire_date)
        VALUES (REGEXP_SUBSTR (:NEW.full_name, '^\S+'), REGEXP_SUBSTR (:NEW.full_name, '\S+$'), LOWER (REGEXP_SUBSTR (:NEW.full_name, '^\S') || REGEXP_SUBSTR (:NEW.full_name, '\S+$')) || '@company.com', :NEW.salary, v_dept_id, v_mgr_id, SYSDATE);
    ELSIF UPDATING THEN
        UPDATE employees
        SET salary = :NEW.salary,
            department_id = (
                            SELECT department_id
                            FROM departments
                            WHERE department_name = :NEW.department_name
                        )
                            WHERE employee_id = :OLD.employee_id;
    ELSIF DELETING THEN
        -- Soft delete
        UPDATE employees
        SET status = 'TERMINATED',
            termination_date = SYSDATE
        WHERE employee_id = :OLD.employee_id;
    END IF;
    END;
        /
        -- [TEST-049] DBMS_PARALLEL_EXECUTE 청크 처리 (예상: 1 실행단위)
        -- 난이도: ★★★★☆
        DECLARE
        l_task_name VARCHAR2 (100) := 'SALARY_UPDATE_' || TO_CHAR (SYSDATE, 'YYYYMMDDHH24MISS');
        l_sql_stmt CLOB;
        l_status NUMBER;
        BEGIN
        DBMS_PARALLEL_EXECUTE.CREATE_TASK (l_task_name);
        DBMS_PARALLEL_EXECUTE.CREATE_CHUNKS_BY_SQL (task_name => l_task_name, sql_stmt => q'[
            SELECT MIN(employee_id) AS start_id, MAX(employee_id) AS end_id
            FROM (
                SELECT employee_id,
                       NTILE(10) OVER (ORDER BY employee_id) AS chunk_num
                FROM employees
                WHERE status = 'ACTIVE'
            )
            GROUP BY chunk_num
        ]', by_rowid => FALSE);
        l_sql_stmt := q'[
        DECLARE
            v_start_id NUMBER := :start_id;
            v_end_id   NUMBER := :end_id;
        BEGIN
            UPDATE employees
            SET salary = salary * CASE
                    WHEN hire_date < ADD_MONTHS(SYSDATE, -120) THEN 1.15
                    WHEN hire_date < ADD_MONTHS(SYSDATE, -60) THEN 1.10
                    ELSE 1.05
                END,
                modified_date = SYSDATE,
                modified_by = USER
            WHERE employee_id BETWEEN v_start_id AND v_end_id
              AND status = 'ACTIVE';
            COMMIT;
        END;
    ]';
        DBMS_PARALLEL_EXECUTE.RUN_TASK (task_name => l_task_name, sql_stmt => l_sql_stmt, language_flag => DBMS_SQL.NATIVE, parallel_level => 4);
        l_status := DBMS_PARALLEL_EXECUTE.TASK_STATUS (l_task_name);
        IF l_status = DBMS_PARALLEL_EXECUTE.FINISHED THEN
            DBMS_OUTPUT.PUT_LINE ('Task completed successfully');
    ELSE
            DBMS_OUTPUT.PUT_LINE ('Task status: ' || l_status);
            FOR r IN (
                SELECT
                    chunk_id,
                    status,
                    start_id,
                    end_id,
                    error_message
                FROM user_parallel_execute_chunks
                WHERE task_name = l_task_name
                    AND status != DBMS_PARALLEL_EXECUTE.PROCESSED
            ) LOOP
                DBMS_OUTPUT.PUT_LINE ('Chunk ' || r.chunk_id || ': ' || r.error_message);
        END LOOP;
    END IF;
        DBMS_PARALLEL_EXECUTE.DROP_TASK (l_task_name);
    END;
        /
        -- [TEST-050] 최종 보스: 문자열+주석+동적SQL+중첩+CASE+라벨 모두 포함 (예상: 1 실행단위)
        -- 난이도: ★★★★★★ (6성)
        CREATE OR REPLACE PACKAGE BODY final_boss_pkg AS
        /*
     * 이 패키지 바디는 파서의 모든 약점을 공격합니다.
     * 
     * 문자열 안: END; / BEGIN CREATE
     * 주석 안: END; / BEGIN CREATE
     * CASE END vs 블록 END
     * 라벨 <<label>>
     * 동적 SQL 안의 PL/SQL
     * q-quote 안의 모든 것
     * 중첩 BEGIN-END
     */
        gc_fake_terminator CONSTANT VARCHAR2 (100) := 'END;
/
BEGIN';
        FUNCTION ultimate_test (p_input IN CLOB) RETURN CLOB IS
            v_result CLOB;
            v_dynamic_sql CLOB;
            v_temp VARCHAR2 (4000);
        BEGIN
            <<main_block>>
            BEGIN
                -- 문자열 안에 온갖 키워드
                v_result := 'CREATE OR REPLACE FUNCTION test RETURN NUMBER IS BEGIN RETURN 1; END;' || chr (10) || '/' || chr (10) || 'BEGIN NULL; END;' || chr (10) || '/';
                /* 주석 안에도 온갖 키워드
               CREATE OR REPLACE PROCEDURE trap IS
               BEGIN
                   NULL;
               END trap;
               /
            */
                -- q-quote 총집합
                v_result := v_result || q'[END; / BEGIN DECLARE v NUMBER; BEGIN NULL; END; /]' || q'{PACKAGE BODY x AS PROCEDURE p IS BEGIN END; END x; /}' || q'!CREATE TRIGGER t BEFORE INSERT ON x FOR EACH ROW BEGIN END; /!';
                -- CASE 표현식 안의 CASE 안의 CASE (END 폭탄)
                v_temp :=
                CASE
        WHEN LENGTH (v_result) > 100 THEN
        CASE
        WHEN INSTR (v_result, 'END') > 0 THEN
        CASE SUBSTR (v_result, 1, 1)
        WHEN 'C' THEN
        'starts with C'
        WHEN 'E' THEN
        'starts with E'
    ELSE
        CASE
        WHEN v_result IS NOT NULL THEN
        'not null'
    ELSE
        'null'
    END
    END -- inner CASE 3
    ELSE
        'no END found'
    END -- inner CASE 2
    ELSE
                    'short string'
    END;
                -- 동적 SQL: PL/SQL 블록을 문자열로 조립
                v_dynamic_sql := q'[
                DECLARE
                    v_inner VARCHAR2(4000) := 'nested; string; END; /';
                    /* 동적 SQL 안의 주석
                       END; /
                    */
                BEGIN
                    <<inner_label>>
                    BEGIN
                        FOR i IN 1..CASE WHEN 1=1 THEN 3 ELSE 5 END LOOP
                            DBMS_OUTPUT.PUT_LINE('Iteration: ' || i || '; still going');
                        END LOOP;
                    END inner_label;
                EXCEPTION
                    WHEN OTHERS THEN
                        BEGIN
                            NULL;
                        END;
                END;
            ]';
                EXECUTE IMMEDIATE v_dynamic_sql;
                -- 라벨 + 루프 + EXIT
                <<process_loop>>
                FOR i IN 1..3 LOOP
                    <<inner_process>>
                    BEGIN
        CASE MOD (i, 3)
        WHEN 0 THEN
        EXIT process_loop;
        WHEN 1 THEN
        CONTINUE process_loop;
    ELSE
        NULL;
    END CASE;
    END inner_process;
        END LOOP process_loop;
                -- FORALL 안에서 CASE 사용
                DECLARE
                    TYPE id_t IS TABLE OF NUMBER;
                    l_ids id_t := id_t (1, 2, 3);
                BEGIN
                    FORALL i IN 1..l_ids.COUNT
                INSERT INTO test_table (id, category)
                VALUES (l_ids (i), 
                    CASE MOD (l_ids (i), 2)
                        WHEN 0 THEN
                            'EVEN'
                        ELSE
                            'ODD'
                    END
                        );
    END;
    EXCEPTION
        WHEN NO_DATA_FOUND THEN
                v_result := 'NOT FOUND; END; / BEGIN';
        WHEN TOO_MANY_ROWS THEN
                v_result := q'<TOO MANY; END; / >';
        WHEN OTHERS THEN
            BEGIN
                -- nested exception handling
                INSERT INTO error_log (msg, ts)
                    VALUES (SQLERRM || ' at ' || DBMS_UTILITY.FORMAT_ERROR_BACKTRACE, SYSTIMESTAMP);
                    COMMIT;
        EXCEPTION
        WHEN OTHERS THEN
                    NULL;
    END;
                RAISE;
            END main_block;
            RETURN v_result;
        END ultimate_test;

        PROCEDURE cleanup IS
            PRAGMA AUTONOMOUS_TRANSACTION;
        BEGIN
            EXECUTE IMMEDIATE 'TRUNCATE TABLE error_log';
            EXECUTE IMMEDIATE 'TRUNCATE TABLE test_table';
            COMMIT;
        END cleanup;
    END final_boss_pkg;
        /
        -- ╔══════════════════════════════════════════════════════════════════════════════╗
        -- ║  검증 요약 (Validation Summary)                                             ║
        -- ╠══════════════════════════════════════════════════════════════════════════════╣
        -- ║  TEST-001: 1    TEST-011: 4    TEST-021: 1    TEST-031: 1    TEST-041: 1  ║
        -- ║  TEST-002: 1    TEST-012: 1    TEST-022: 6    TEST-032: 1    TEST-042: 3  ║
        -- ║  TEST-003: 1    TEST-013: 1    TEST-023: 2    TEST-033: ?*   TEST-043: 6  ║
        -- ║  TEST-004: 1    TEST-014: 1    TEST-024: 1    TEST-034: 1    TEST-044: 1  ║
        -- ║  TEST-005: 1    TEST-015: 1    TEST-025: 1    TEST-035: 1    TEST-045: 1  ║
        -- ║  TEST-006: 1    TEST-016: 1    TEST-026: 2    TEST-036: 7    TEST-046: 1  ║
        -- ║  TEST-007: 1    TEST-017: 1    TEST-027: 3    TEST-037: 1    TEST-047: 1  ║
        -- ║  TEST-008: 1    TEST-018: 1    TEST-028: 1    TEST-038: 1    TEST-048: 1  ║
        -- ║  TEST-009: 1    TEST-019: 1    TEST-029: 1    TEST-039: 2    TEST-049: 1  ║
        -- ║  TEST-010: 1    TEST-020: 1    TEST-030: 4    TEST-040: 1    TEST-050: 1  ║
        -- ║                                                                            ║
        -- ║  *TEST-033: SQL*Plus 커맨드 처리 정책에 따라 다름                               ║
        -- ║                                                                            ║
        -- ║  주요 검증 포인트:                                                            ║
        -- ║  1. 세미콜론(;) 종결 DML/DDL vs PL/SQL 블록 내부 세미콜론                       ║
        -- ║  2. 슬래시(/) 종결자 vs 나눗셈 연산자 vs 줄바꿈 위치                            ║
        -- ║  3. 문자열 리터럴 내부의 모든 키워드 무시                                       ║
        -- ║  4. q-quote 다양한 구분자 처리                                                ║
        -- ║  5. 주석(라인/블록/힌트) 내부 키워드 무시                                       ║
        -- ║  6. CASE END vs BEGIN-END 블록 END 정확한 구분                                ║
        -- ║  7. CREATE PACKAGE/TYPE/TRIGGER BODY의 정확한 범위 인식                        ║
        -- ║  8. COMPOUND TRIGGER의 다중 섹션 구조                                         ║
        -- ║  9. CREATE JAVA SOURCE 내부 Java 세미콜론 무시                                ║
        -- ║  10. 조건부 컴파일 ($IF/$END) 처리                                            ║
        -- ║  11. 라벨(<<label>>) 처리                                                    ║
        -- ║  12. 동적 SQL(EXECUTE IMMEDIATE) 내부 문자열 처리                              ║
        -- ║  13. WITH 절 인라인 PL/SQL 함수                                              ║
        -- ║  14. 유니코드/멀티바이트 식별자                                                ║
        -- ║  15. SQL*Plus 커맨드와 SQL/PLSQL 혼합                                         ║
        -- ╚══════════════════════════════════════════════════════════════════════════════╝
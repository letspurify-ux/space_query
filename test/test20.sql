--------------------------------------------------------------------------------
-- FINAL BOSS ORACLE TEST SCRIPT
-- 목적:
--   1) 실행단위 분리기
--   2) SQL/PLSQL depth 계산
--   3) 자동 포맷터
--   4) 문자열/주석/동적SQL 오탐
--   5) / 종결 처리
--   6) 복합 구문 안정성
--
-- 주의:
--   - 이 스크립트는 "최고 난이도" 검증용이다.
--   - 여러 객체를 생성/삭제한다.
--   - SQL*Plus/TOAD 스타일로 / 실행이 필요한 단위가 포함된다.
--   - q-quote, JSON, XML, dynamic SQL, package body, trigger, merge, model 등 포함.
--------------------------------------------------------------------------------

--------------------------------------------------------------------------------
-- 0. CLEANUP
--------------------------------------------------------------------------------
BEGIN
    FOR r IN (
        SELECT object_name, object_type
          FROM user_objects
         WHERE object_name IN (
                'QT_FB_EMP',
                'QT_FB_DEPT',
                'QT_FB_AUDIT',
                'QT_FB_STAGE',
                'QT_FB_JSON_DOC',
                'QT_FB_SEQ',
                'QT_FB_OBJ',
                'QT_FB_OBJ_TAB',
                'QT_FB_PKG',
                'QT_FB_VIEW',
                'QT_FB_TRG',
                'QT_FB_LOG_PROC',
                'QT_FB_PIPE_FUNC'
         )
    )
    LOOP
        BEGIN
            IF r.object_type = 'TABLE' THEN
                EXECUTE IMMEDIATE 'DROP TABLE ' || r.object_name || ' PURGE';
            ELSIF r.object_type = 'VIEW' THEN
                EXECUTE IMMEDIATE 'DROP VIEW ' || r.object_name;
            ELSIF r.object_type = 'SEQUENCE' THEN
                EXECUTE IMMEDIATE 'DROP SEQUENCE ' || r.object_name;
            ELSIF r.object_type = 'PACKAGE' THEN
                EXECUTE IMMEDIATE 'DROP PACKAGE ' || r.object_name;
            ELSIF r.object_type = 'PROCEDURE' THEN
                EXECUTE IMMEDIATE 'DROP PROCEDURE ' || r.object_name;
            ELSIF r.object_type = 'FUNCTION' THEN
                EXECUTE IMMEDIATE 'DROP FUNCTION ' || r.object_name;
            ELSIF r.object_type = 'TRIGGER' THEN
                EXECUTE IMMEDIATE 'DROP TRIGGER ' || r.object_name;
            ELSIF r.object_type = 'TYPE' THEN
                EXECUTE IMMEDIATE 'DROP TYPE ' || r.object_name || ' FORCE';
            END IF;
        EXCEPTION
            WHEN OTHERS THEN
                NULL;
        END;
    END LOOP;
END;
/
--------------------------------------------------------------------------------
-- 1. BASE OBJECTS
--------------------------------------------------------------------------------
CREATE TABLE qt_fb_dept
(
    dept_id        NUMBER         CONSTRAINT qt_fb_dept_pk PRIMARY KEY,
    dept_name      VARCHAR2(100)  NOT NULL,
    parent_dept_id NUMBER,
    meta_json      CLOB CHECK (meta_json IS JSON)
);
/

CREATE TABLE qt_fb_emp
(
    emp_id          NUMBER         CONSTRAINT qt_fb_emp_pk PRIMARY KEY,
    dept_id         NUMBER         NOT NULL,
    emp_name        VARCHAR2(200)  NOT NULL,
    salary          NUMBER(12,2),
    bonus           NUMBER(12,2),
    hire_dt         DATE,
    status          VARCHAR2(30),
    remarks         CLOB,
    xml_payload     XMLTYPE,
    created_at      TIMESTAMP DEFAULT SYSTIMESTAMP,
    updated_at      TIMESTAMP,
    CONSTRAINT qt_fb_emp_fk1 FOREIGN KEY (dept_id) REFERENCES qt_fb_dept(dept_id)
);
/

CREATE TABLE qt_fb_stage
(
    stage_id        NUMBER PRIMARY KEY,
    dept_id         NUMBER,
    emp_name        VARCHAR2(200),
    salary          NUMBER(12,2),
    bonus           NUMBER(12,2),
    hire_dt         DATE,
    status          VARCHAR2(30),
    remarks         CLOB
);
/

CREATE TABLE qt_fb_audit
(
    audit_id        NUMBER PRIMARY KEY,
    module_name     VARCHAR2(100),
    action_name     VARCHAR2(100),
    message_text    CLOB,
    extra_text      CLOB,
    created_at      TIMESTAMP DEFAULT SYSTIMESTAMP
);
/

CREATE TABLE qt_fb_json_doc
(
    doc_id          NUMBER PRIMARY KEY,
    doc_body        CLOB CHECK (doc_body IS JSON),
    created_at      TIMESTAMP DEFAULT SYSTIMESTAMP
);
/

CREATE SEQUENCE qt_fb_seq START WITH 1 INCREMENT BY 1 NOCACHE;
/

--------------------------------------------------------------------------------
-- 2. TYPES
--------------------------------------------------------------------------------
CREATE OR REPLACE TYPE qt_fb_obj AS OBJECT
(
    emp_id      NUMBER,
    emp_name    VARCHAR2(200),
    salary      NUMBER
);
/

CREATE OR REPLACE TYPE qt_fb_obj_tab AS TABLE OF qt_fb_obj;
/

--------------------------------------------------------------------------------
-- 3. SEED DATA
--------------------------------------------------------------------------------
INSERT INTO qt_fb_dept (dept_id, dept_name, parent_dept_id, meta_json)
VALUES
(
    10,
    'HQ',
    NULL,
    '{"region":"KR","flags":["core","top"],"notes":"contains ; semicolon and / slash"}'
);

INSERT INTO qt_fb_dept (dept_id, dept_name, parent_dept_id, meta_json)
VALUES
(
    20,
    'PLATFORM',
    10,
    '{"region":"US","flags":["dev","api"],"notes":"BEGIN END IF LOOP CASE MERGE"}'
);

INSERT INTO qt_fb_dept (dept_id, dept_name, parent_dept_id, meta_json)
VALUES
(
    30,
    'DATA',
    10,
    '{"region":"JP","flags":["ml","etl"],"notes":"q''[not terminator ; / ]''"}'
);

INSERT INTO qt_fb_stage
SELECT 1, 20, 'ALICE',  9000, 300, DATE '2020-01-15', 'ACTIVE',
       q'[
remark line 1;
remark line 2 / still text
embedded SQL words: SELECT FROM WHERE BEGIN END;
]'
  FROM dual
UNION ALL
SELECT 2, 20, 'BOB',   12000, 500, DATE '2021-03-01', 'LEAVE',
       q'!JSON-like {"a":"b;c/d","x":"BEGIN;END/"}!' FROM dual
UNION ALL
SELECT 3, 30, 'CAROL', 15000, 700, DATE '2019-07-07', 'ACTIVE',
       q'<xml-ish <tag attr="x;y/z">TEXT</tag> >' FROM dual
UNION ALL
SELECT 4, 30, 'DAVE',   8000, NULL, DATE '2022-12-12', 'INACTIVE',
       q'~regexp chars .* + ? ^ $ [ ] ( ) { } | \ ; / ~' FROM dual;
/

INSERT INTO qt_fb_json_doc (doc_id, doc_body)
VALUES
(
    1,
    q'!{
        "name":"boss-doc",
        "items":[
            {"k":"alpha","v":"BEGIN;END/"},
            {"k":"beta","v":"q''[abc;def/]''"},
            {"k":"gamma","v":"/* not comment */ -- not line comment"}
        ],
        "nested":{"a":{"b":{"c":[1,2,3]}}}
    }!'
);
/

COMMIT;
/

--------------------------------------------------------------------------------
-- 4. VIEW WITH RECURSIVE-LIKE HIERARCHY, JSON_TABLE, XMLTABLE, ANALYTIC
--------------------------------------------------------------------------------
CREATE OR REPLACE VIEW qt_fb_view
AS
WITH dept_tree AS
(
    SELECT d.dept_id,
           d.dept_name,
           d.parent_dept_id,
           LEVEL AS lvl,
           SYS_CONNECT_BY_PATH(d.dept_name, ' > ') AS path_txt,
           d.meta_json
      FROM qt_fb_dept d
     START WITH d.parent_dept_id IS NULL
   CONNECT BY PRIOR d.dept_id = d.parent_dept_id
),
emp_enriched AS
(
    SELECT e.emp_id,
           e.dept_id,
           e.emp_name,
           e.salary,
           e.bonus,
           e.hire_dt,
           e.status,
           e.remarks,
           ROW_NUMBER() OVER (PARTITION BY e.dept_id ORDER BY e.salary DESC NULLS LAST, e.emp_id) AS rn,
           DENSE_RANK() OVER (ORDER BY e.salary DESC NULLS LAST) AS dr,
           SUM(NVL(e.salary,0)) OVER (PARTITION BY e.dept_id) AS dept_salary_sum
      FROM qt_fb_emp e
)
SELECT dt.dept_id,
       dt.dept_name,
       dt.lvl,
       dt.path_txt,
       ee.emp_id,
       ee.emp_name,
       ee.salary,
       ee.bonus,
       ee.hire_dt,
       ee.status,
       ee.rn,
       ee.dr,
       ee.dept_salary_sum,
       jt.region,
       jt.flag1,
       xt.tag_text
  FROM dept_tree dt
  LEFT JOIN emp_enriched ee
    ON ee.dept_id = dt.dept_id
  LEFT JOIN JSON_TABLE
       (
           dt.meta_json,
           '$'
           COLUMNS
           (
               region VARCHAR2(30) PATH '$.region',
               flag1  VARCHAR2(30) PATH '$.flags[0]'
           )
       ) jt
    ON 1 = 1
  LEFT JOIN XMLTABLE
       (
           '/root'
           PASSING XMLTYPE('<root><x>tag-text</x></root>')
           COLUMNS tag_text VARCHAR2(100) PATH 'x'
       ) xt
    ON 1 = 1;
/

--------------------------------------------------------------------------------
-- 5. AUTONOMOUS LOGGER PROCEDURE
--------------------------------------------------------------------------------
CREATE OR REPLACE PROCEDURE qt_fb_log_proc
(
    p_module IN VARCHAR2,
    p_action IN VARCHAR2,
    p_msg    IN CLOB,
    p_extra  IN CLOB DEFAULT NULL
)
IS
    PRAGMA AUTONOMOUS_TRANSACTION;
BEGIN
    INSERT INTO qt_fb_audit
    (
        audit_id,
        module_name,
        action_name,
        message_text,
        extra_text,
        created_at
    )
    VALUES
    (
        qt_fb_seq.NEXTVAL,
        p_module,
        p_action,
        p_msg,
        p_extra,
        SYSTIMESTAMP
    );

    COMMIT;
EXCEPTION
    WHEN OTHERS THEN
        ROLLBACK;
        RAISE;
END;
/
--------------------------------------------------------------------------------
-- 6. PACKAGE SPEC
--------------------------------------------------------------------------------
CREATE OR REPLACE PACKAGE qt_fb_pkg
IS
    SUBTYPE t_name IS VARCHAR2(200);

    TYPE t_num_tab IS TABLE OF NUMBER INDEX BY PLS_INTEGER;
    TYPE t_name_tab IS TABLE OF t_name INDEX BY PLS_INTEGER;

    c_status_active   CONSTANT VARCHAR2(30) := 'ACTIVE';
    c_status_inactive CONSTANT VARCHAR2(30) := 'INACTIVE';

    g_last_message    CLOB;

    FUNCTION weird_text(p_id NUMBER) RETURN CLOB;

    FUNCTION obj_rows(p_min_salary NUMBER) RETURN qt_fb_obj_tab PIPELINED;

    PROCEDURE load_stage_to_emp(
        p_raise_pct    NUMBER DEFAULT 0,
        p_commit_each  NUMBER DEFAULT 0
    );

    PROCEDURE run_dynamic_report(
        p_dept_id       NUMBER,
        p_status        VARCHAR2 DEFAULT NULL,
        p_result_count  OUT NUMBER
    );

    PROCEDURE test_nested_everything;
END qt_fb_pkg;
/
--------------------------------------------------------------------------------
-- 7. PACKAGE BODY
--------------------------------------------------------------------------------
CREATE OR REPLACE PACKAGE BODY qt_fb_pkg
IS
    ----------------------------------------------------------------------------
    -- private helper
    ----------------------------------------------------------------------------
    FUNCTION weird_text(p_id NUMBER) RETURN CLOB
    IS
        v_text CLOB;
    BEGIN
        v_text :=
              q'[This is a q-quote block with ; semicolon and / slash.]'
           || CHR(10)
           || q'!Line with fake terminators: BEGIN; END; / -- /* */ !'
           || CHR(10)
           || 'ID=' || p_id
           || CHR(10)
           || q'~Nested-looking text: q''[abc;def/]'' and q'!ghi!'~';

        RETURN v_text;
    END weird_text;

    ----------------------------------------------------------------------------
    -- pipelined function
    ----------------------------------------------------------------------------
    FUNCTION obj_rows(p_min_salary NUMBER) RETURN qt_fb_obj_tab PIPELINED
    IS
    BEGIN
        FOR r IN
        (
            SELECT emp_id, emp_name, salary
              FROM qt_fb_emp
             WHERE salary >= p_min_salary
             ORDER BY salary DESC, emp_id
        )
        LOOP
            PIPE ROW (qt_fb_obj(r.emp_id, r.emp_name, r.salary));
        END LOOP;

        RETURN;
    END obj_rows;

    ----------------------------------------------------------------------------
    -- dynamic loader
    ----------------------------------------------------------------------------
    PROCEDURE load_stage_to_emp
    (
        p_raise_pct    NUMBER DEFAULT 0,
        p_commit_each  NUMBER DEFAULT 0
    )
    IS
        TYPE t_stage_tab IS TABLE OF qt_fb_stage%ROWTYPE INDEX BY PLS_INTEGER;

        v_stage_tab      t_stage_tab;
        v_sql            CLOB;
        v_commit_counter NUMBER := 0;
    BEGIN
        SELECT *
          BULK COLLECT INTO v_stage_tab
          FROM qt_fb_stage
         ORDER BY stage_id;

        v_sql := q'[
            MERGE INTO qt_fb_emp t
            USING
            (
                SELECT :1 AS emp_id,
                       :2 AS dept_id,
                       :3 AS emp_name,
                       :4 AS salary,
                       :5 AS bonus,
                       :6 AS hire_dt,
                       :7 AS status,
                       :8 AS remarks
                  FROM dual
            ) s
               ON (t.emp_id = s.emp_id)
             WHEN MATCHED THEN
                 UPDATE SET
                     t.dept_id     = s.dept_id,
                     t.emp_name    = s.emp_name,
                     t.salary      = s.salary,
                     t.bonus       = s.bonus,
                     t.hire_dt     = s.hire_dt,
                     t.status      = s.status,
                     t.remarks     = s.remarks,
                     t.updated_at  = SYSTIMESTAMP
             WHEN NOT MATCHED THEN
                 INSERT
                 (
                     emp_id, dept_id, emp_name, salary, bonus, hire_dt, status,
                     remarks, xml_payload, created_at, updated_at
                 )
                 VALUES
                 (
                     s.emp_id, s.dept_id, s.emp_name, s.salary, s.bonus, s.hire_dt,
                     s.status, s.remarks,
                     XMLTYPE('<emp><name>' || s.emp_name || '</name></emp>'),
                     SYSTIMESTAMP,
                     SYSTIMESTAMP
                 )
        ]';

        FOR i IN 1 .. v_stage_tab.COUNT
        LOOP
            EXECUTE IMMEDIATE v_sql
                USING
                    v_stage_tab(i).stage_id,
                    v_stage_tab(i).dept_id,
                    v_stage_tab(i).emp_name,
                    ROUND(v_stage_tab(i).salary * (1 + NVL(p_raise_pct,0) / 100), 2),
                    v_stage_tab(i).bonus,
                    v_stage_tab(i).hire_dt,
                    v_stage_tab(i).status,
                    v_stage_tab(i).remarks;

            v_commit_counter := v_commit_counter + 1;

            IF p_commit_each > 0 AND MOD(v_commit_counter, p_commit_each) = 0 THEN
                COMMIT;
            END IF;
        END LOOP;

        qt_fb_log_proc(
            p_module => 'qt_fb_pkg.load_stage_to_emp',
            p_action => 'MERGE',
            p_msg    => weird_text(1001),
            p_extra  => q'[load completed; possible fake terminators inside text: ; / BEGIN END]'
        );
    END load_stage_to_emp;

    ----------------------------------------------------------------------------
    -- complex dynamic report
    ----------------------------------------------------------------------------
    PROCEDURE run_dynamic_report
    (
        p_dept_id       NUMBER,
        p_status        VARCHAR2 DEFAULT NULL,
        p_result_count  OUT NUMBER
    )
    IS
        v_sql       CLOB;
        v_count     NUMBER;
    BEGIN
        v_sql :=
            q'[
                SELECT COUNT(*)
                  FROM
                  (
                      SELECT e.emp_id,
                             e.emp_name,
                             e.salary,
                             CASE
                                 WHEN e.salary >= 15000 THEN 'TOP'
                                 WHEN e.salary >= 10000 THEN 'MID'
                                 ELSE 'LOW'
                             END AS grade
                        FROM qt_fb_emp e
                       WHERE e.dept_id = :b_dept_id
                         AND (:b_status IS NULL OR e.status = :b_status)
                  )
            ]';

        EXECUTE IMMEDIATE v_sql
           INTO v_count
          USING p_dept_id, p_status, p_status;

        p_result_count := v_count;

        qt_fb_log_proc(
            p_module => 'qt_fb_pkg.run_dynamic_report',
            p_action => 'COUNT',
            p_msg    => 'count=' || v_count,
            p_extra  => q'!dynamic SQL with bind reuse :b_status and CASE;END; / !'
        );
    END run_dynamic_report;

    ----------------------------------------------------------------------------
    -- extreme nested test
    ----------------------------------------------------------------------------
    PROCEDURE test_nested_everything
    IS
        v_num_tab        t_num_tab;
        v_name_tab       t_name_tab;
        v_count          NUMBER := 0;
        v_dummy          VARCHAR2(32767);
        v_json_value     VARCHAR2(4000);
        v_cursor         SYS_REFCURSOR;
        v_emp_id         NUMBER;
        v_emp_name       VARCHAR2(200);
        v_salary         NUMBER;
        e_custom EXCEPTION;
        PRAGMA EXCEPTION_INIT(e_custom, -20001);

        FUNCTION inner_fn(p_txt VARCHAR2) RETURN VARCHAR2
        IS
        BEGIN
            RETURN REPLACE(
                       REGEXP_REPLACE(
                           p_txt,
                           '(BEGIN|END|IF|LOOP|CASE)',
                           '[KW]'
                       ),
                       ';',
                       ':'
                   );
        END inner_fn;

    BEGIN
        v_num_tab(1) := 10;
        v_num_tab(2) := 20;
        v_num_tab(3) := 30;

        v_name_tab(1) := 'ALICE';
        v_name_tab(2) := 'BOB';
        v_name_tab(3) := q'[CAROL; / BEGIN END]';

        SELECT JSON_VALUE(doc_body, '$.items[1].v')
          INTO v_json_value
          FROM qt_fb_json_doc
         WHERE doc_id = 1;

        OPEN v_cursor FOR
            WITH x AS
            (
                SELECT e.emp_id,
                       e.emp_name,
                       e.salary,
                       ROW_NUMBER() OVER (ORDER BY e.salary DESC, e.emp_id) AS rn
                  FROM qt_fb_emp e
                 WHERE e.dept_id IN (SELECT COLUMN_VALUE FROM TABLE(sys.odcinumberlist(20,30)))
            )
            SELECT emp_id, emp_name, salary
              FROM x
             WHERE rn <= 100;

        LOOP
            FETCH v_cursor INTO v_emp_id, v_emp_name, v_salary;
            EXIT WHEN v_cursor%NOTFOUND;

            v_count := v_count + 1;

            BEGIN
                IF v_salary IS NULL THEN
                    RAISE_APPLICATION_ERROR(-20001, 'salary is null; this ; / is inside message');
                ELSIF v_salary > 14000 THEN
                    v_dummy :=
                        CASE
                            WHEN MOD(v_emp_id, 2) = 0 THEN inner_fn('BEGIN;END;')
                            ELSE inner_fn(q'[IF;LOOP;CASE;/]')
                        END;
                ELSE
                    v_dummy := inner_fn('normal;text');
                END IF;

                qt_fb_log_proc(
                    p_module => 'qt_fb_pkg.test_nested_everything',
                    p_action => 'LOOP_ROW',
                    p_msg    => 'emp=' || v_emp_id || ', dummy=' || v_dummy,
                    p_extra  => 'json=' || v_json_value
                );
            EXCEPTION
                WHEN e_custom THEN
                    qt_fb_log_proc(
                        p_module => 'qt_fb_pkg.test_nested_everything',
                        p_action => 'ROW_ERROR',
                        p_msg    => SQLERRM,
                        p_extra  => DBMS_UTILITY.FORMAT_ERROR_BACKTRACE
                    );
                WHEN OTHERS THEN
                    qt_fb_log_proc(
                        p_module => 'qt_fb_pkg.test_nested_everything',
                        p_action => 'ROW_OTHERS',
                        p_msg    => SQLERRM,
                        p_extra  => DBMS_UTILITY.FORMAT_CALL_STACK
                    );
            END;
        END LOOP;

        CLOSE v_cursor;

        g_last_message :=
            q'[
Package body final message.
Contains:
  1) semicolon ;
  2) slash /
  3) fake anonymous block:
        BEGIN
            NULL;
        END;
  4) fake trigger text:
        CREATE OR REPLACE TRIGGER x
        BEFORE INSERT ON y
        BEGIN
            NULL;
        END;
        /
]';

        qt_fb_log_proc(
            p_module => 'qt_fb_pkg.test_nested_everything',
            p_action => 'DONE',
            p_msg    => g_last_message,
            p_extra  => 'count=' || v_count
        );
    END test_nested_everything;
END qt_fb_pkg;
/
--------------------------------------------------------------------------------
-- 8. TRIGGER
--------------------------------------------------------------------------------
CREATE OR REPLACE TRIGGER qt_fb_trg
BEFORE INSERT OR UPDATE ON qt_fb_emp
FOR EACH ROW
DECLARE
    v_msg CLOB;
BEGIN
    v_msg :=
           'trigger fired for emp_id=' || :NEW.emp_id
        || '; old_status=' || NVL(:OLD.status, 'NULL')
        || '; new_status=' || NVL(:NEW.status, 'NULL')
        || '; fake terminator / inside text';

    :NEW.updated_at := SYSTIMESTAMP;

    IF INSERTING AND :NEW.created_at IS NULL THEN
        :NEW.created_at := SYSTIMESTAMP;
    END IF;

    IF :NEW.salary < 0 THEN
        RAISE_APPLICATION_ERROR(-20002, 'negative salary is invalid; / ;');
    END IF;
END;
/
--------------------------------------------------------------------------------
-- 9. EXECUTION BLOCKS
--------------------------------------------------------------------------------

-- 9-1. Load stage -> emp via package
BEGIN
    qt_fb_pkg.load_stage_to_emp(p_raise_pct => 12.5, p_commit_each => 2);
END;
/

-- 9-2. Dynamic report
DECLARE
    v_cnt NUMBER;
BEGIN
    qt_fb_pkg.run_dynamic_report(
        p_dept_id      => 20,
        p_status       => 'ACTIVE',
        p_result_count => v_cnt
    );

    DBMS_OUTPUT.PUT_LINE('dept 20 active count = ' || v_cnt);
END;
/

-- 9-3. Nested everything
BEGIN
    qt_fb_pkg.test_nested_everything;
END;
/

--------------------------------------------------------------------------------
-- 10. HARDCORE SELECT SET
--------------------------------------------------------------------------------

-- 10-1. Deep WITH + analytic + scalar subquery + cursor expression
WITH base AS
(
    SELECT e.emp_id,
           e.emp_name,
           e.dept_id,
           e.salary,
           e.bonus,
           e.status,
           (SELECT d.dept_name FROM qt_fb_dept d WHERE d.dept_id = e.dept_id) AS dept_name,
           ROW_NUMBER() OVER (PARTITION BY e.dept_id ORDER BY e.salary DESC NULLS LAST, e.emp_id) AS rn,
           LAG(e.salary)  OVER (PARTITION BY e.dept_id ORDER BY e.salary DESC NULLS LAST, e.emp_id) AS prev_sal,
           LEAD(e.salary) OVER (PARTITION BY e.dept_id ORDER BY e.salary DESC NULLS LAST, e.emp_id) AS next_sal
      FROM qt_fb_emp e
),
graded AS
(
    SELECT b.*,
           CASE
               WHEN b.salary >= 15000 THEN 'S'
               WHEN b.salary >= 12000 THEN 'A'
               WHEN b.salary >=  9000 THEN 'B'
               ELSE 'C'
           END AS grade
      FROM base b
)
SELECT g.*,
       CURSOR
       (
           SELECT a.audit_id, a.action_name, a.created_at
             FROM qt_fb_audit a
            WHERE a.module_name LIKE 'qt_fb_pkg%'
              AND a.created_at >= SYSTIMESTAMP - INTERVAL '1' DAY
       ) AS audit_cur
  FROM graded g
 WHERE EXISTS
       (
           SELECT 1
             FROM qt_fb_dept d
            WHERE d.dept_id = g.dept_id
              AND JSON_EXISTS(d.meta_json, '$.flags[*]?(@ == "dev" || @ == "ml")')
       )
 ORDER BY g.dept_id, g.salary DESC, g.emp_id;
/

-- 10-2. PIVOT
SELECT *
  FROM
  (
      SELECT dept_id, status, salary
        FROM qt_fb_emp
  )
  PIVOT
  (
      SUM(salary)
      FOR status IN ('ACTIVE' AS active_sal, 'LEAVE' AS leave_sal, 'INACTIVE' AS inactive_sal)
  )
 ORDER BY dept_id;
/

-- 10-3. UNPIVOT
SELECT dept_id, metric_name, metric_value
  FROM
  (
      SELECT dept_id,
             SUM(NVL(salary,0)) AS total_salary,
             SUM(NVL(bonus,0))  AS total_bonus
        FROM qt_fb_emp
       GROUP BY dept_id
  )
  UNPIVOT
  (
      metric_value FOR metric_name IN
      (
          total_salary AS 'TOTAL_SALARY',
          total_bonus  AS 'TOTAL_BONUS'
      )
  )
 ORDER BY dept_id, metric_name;
/

-- 10-4. MODEL clause
SELECT dept_id, seq_no, calc_value
  FROM
  (
      SELECT dept_id,
             ROW_NUMBER() OVER (PARTITION BY dept_id ORDER BY emp_id) AS seq_no,
             NVL(salary,0) AS calc_value
        FROM qt_fb_emp
  )
  MODEL
  PARTITION BY (dept_id)
  DIMENSION BY (seq_no)
  MEASURES (calc_value)
  RULES
  (
      calc_value[ANY] = calc_value[CV()] + NVL(calc_value[CV()-1], 0)
  )
 ORDER BY dept_id, seq_no;
/

-- 10-5. MATCH_RECOGNIZE
SELECT *
  FROM qt_fb_emp
 MATCH_RECOGNIZE
 (
     PARTITION BY dept_id
     ORDER BY hire_dt
     MEASURES
         FIRST(emp_name) AS first_emp,
         LAST(emp_name)  AS last_emp,
         COUNT(*)        AS cnt
     PATTERN (A B*)
     DEFINE
         A AS salary >= 8000,
         B AS salary >= PREV(salary)
 );
/

-- 10-6. JSON_TABLE against JSON doc
SELECT jd.doc_id,
       jt.k,
       jt.v
  FROM qt_fb_json_doc jd,
       JSON_TABLE
       (
           jd.doc_body,
           '$.items[*]'
           COLUMNS
           (
               k VARCHAR2(100) PATH '$.k',
               v VARCHAR2(4000) PATH '$.v'
           )
       ) jt
 ORDER BY jd.doc_id, jt.k;
/

--------------------------------------------------------------------------------
-- 11. PIPELINED FUNCTION USAGE
--------------------------------------------------------------------------------
SELECT *
  FROM TABLE(qt_fb_pkg.obj_rows(9000))
 ORDER BY salary DESC, emp_id;
/

--------------------------------------------------------------------------------
-- 12. MERGE WITH COMPLEX SOURCE
--------------------------------------------------------------------------------
MERGE INTO qt_fb_emp t
USING
(
    WITH s AS
    (
        SELECT 1001 AS emp_id,
               20   AS dept_id,
               'EVE' AS emp_name,
               11111 AS salary,
               111   AS bonus,
               DATE '2024-01-01' AS hire_dt,
               'ACTIVE' AS status,
               q'[remarks with ; and / and BEGIN END]' AS remarks
          FROM dual
    )
    SELECT * FROM s
) src
   ON (t.emp_id = src.emp_id)
 WHEN MATCHED THEN
     UPDATE
        SET t.salary     = src.salary,
            t.bonus      = src.bonus,
            t.updated_at = SYSTIMESTAMP,
            t.remarks    = src.remarks
 WHEN NOT MATCHED THEN
     INSERT
     (
         emp_id, dept_id, emp_name, salary, bonus, hire_dt, status, remarks,
         xml_payload, created_at, updated_at
     )
     VALUES
     (
         src.emp_id, src.dept_id, src.emp_name, src.salary, src.bonus,
         src.hire_dt, src.status, src.remarks,
         XMLTYPE('<emp><name>' || src.emp_name || '</name><s>' || src.salary || '</s></emp>'),
         SYSTIMESTAMP, SYSTIMESTAMP
     );
/

--------------------------------------------------------------------------------
-- 13. BULK COLLECT + FORALL + SAVE EXCEPTIONS
--------------------------------------------------------------------------------
DECLARE
    TYPE t_emp_id_tab IS TABLE OF NUMBER INDEX BY PLS_INTEGER;
    TYPE t_bonus_tab  IS TABLE OF NUMBER INDEX BY PLS_INTEGER;

    v_emp_ids t_emp_id_tab;
    v_bonus   t_bonus_tab;

    bulk_errors EXCEPTION;
    PRAGMA EXCEPTION_INIT(bulk_errors, -24381);
BEGIN
    SELECT emp_id, NVL(bonus,0) + 10
      BULK COLLECT INTO v_emp_ids, v_bonus
      FROM qt_fb_emp
     WHERE dept_id IN (20,30)
     ORDER BY emp_id;

    BEGIN
        FORALL i IN INDICES OF v_emp_ids SAVE EXCEPTIONS
            UPDATE qt_fb_emp
               SET bonus = v_bonus(i),
                   remarks = NVL(remarks, EMPTY_CLOB()) || CHR(10) || 'bulk updated; index=' || i || '; /'
             WHERE emp_id = v_emp_ids(i);
    EXCEPTION
        WHEN bulk_errors THEN
            FOR j IN 1 .. SQL%BULK_EXCEPTIONS.COUNT
            LOOP
                qt_fb_log_proc(
                    p_module => 'bulk_forall',
                    p_action => 'SAVE_EXCEPTIONS',
                    p_msg    => 'err index=' || SQL%BULK_EXCEPTIONS(j).ERROR_INDEX,
                    p_extra  => 'err code='  || SQL%BULK_EXCEPTIONS(j).ERROR_CODE
                );
            END LOOP;
    END;
END;
/

--------------------------------------------------------------------------------
-- 14. DYNAMIC DDL INSIDE BLOCK
--------------------------------------------------------------------------------
DECLARE
    v_sql CLOB;
BEGIN
    v_sql := q'[
        DECLARE
            v_x NUMBER := 1;
        BEGIN
            -- fake terminators in dynamic body ; / BEGIN END
            INSERT INTO qt_fb_audit(audit_id, module_name, action_name, message_text, extra_text, created_at)
            VALUES
            (
                qt_fb_seq.NEXTVAL,
                'dynamic-inner',
                'insert',
                q'[text with ; / BEGIN END "quotes" ''single quotes'']',
                'ok',
                SYSTIMESTAMP
            );
        END;
    ]';

    EXECUTE IMMEDIATE v_sql;
END;
/

--------------------------------------------------------------------------------
-- 15. HINTS + COMMENT TRAPS + REGEXP + ALTERNATIVE QUOTES
--------------------------------------------------------------------------------
SELECT /*+ qb_name(main_qb) leading(e d) use_hash(d) */
       e.emp_id,
       e.emp_name,
       REGEXP_REPLACE(
           q'[A;B/C(1)[x]{y}|z^$.*+?]',
           '([;\[\]\(\)\{\}\|\^\$\.\*\+\?\/])',
           '(\1)'
       ) AS regex_out,
       d.dept_name
  FROM qt_fb_emp e
  JOIN qt_fb_dept d
    ON d.dept_id = e.dept_id
 WHERE e.emp_name LIKE q'[%A%]' ESCAPE '\'
 ORDER BY e.emp_id;
/

--------------------------------------------------------------------------------
-- 16. XMLQUERY / XMLTABLE
--------------------------------------------------------------------------------
SELECT e.emp_id,
       x.emp_name_from_xml
  FROM qt_fb_emp e,
       XMLTABLE
       (
           '/emp'
           PASSING e.xml_payload
           COLUMNS emp_name_from_xml VARCHAR2(200) PATH 'name'
       ) x
 ORDER BY e.emp_id;
/

--------------------------------------------------------------------------------
-- 17. MULTI-LAYERED CASE + SUBQUERY + EXISTS + NOT EXISTS
--------------------------------------------------------------------------------
SELECT e.emp_id,
       e.emp_name,
       CASE
           WHEN EXISTS (SELECT 1 FROM qt_fb_audit a WHERE a.module_name = 'qt_fb_pkg.test_nested_everything')
                AND NOT EXISTS (SELECT 1 FROM qt_fb_emp z WHERE z.emp_id = -999)
               THEN
                   CASE
                       WHEN e.salary >= (SELECT AVG(salary) FROM qt_fb_emp WHERE dept_id = e.dept_id)
                           THEN 'ABOVE_AVG'
                       ELSE 'BELOW_AVG'
                   END
           ELSE 'UNKNOWN'
       END AS salary_band
  FROM qt_fb_emp e
 ORDER BY e.emp_id;
/

--------------------------------------------------------------------------------
-- 18. FINAL VALIDATION BLOCK
--------------------------------------------------------------------------------
DECLARE
    v_emp_cnt      NUMBER;
    v_audit_cnt    NUMBER;
    v_json_cnt     NUMBER;
    v_pipe_cnt     NUMBER;
    v_text         CLOB;
BEGIN
    SELECT COUNT(*) INTO v_emp_cnt   FROM qt_fb_emp;
    SELECT COUNT(*) INTO v_audit_cnt FROM qt_fb_audit;
    SELECT COUNT(*) INTO v_json_cnt
      FROM qt_fb_json_doc
     WHERE JSON_EXISTS(doc_body, '$.nested.a.b.c[*]?(@ == 2)');

    SELECT COUNT(*) INTO v_pipe_cnt
      FROM TABLE(qt_fb_pkg.obj_rows(8000));

    v_text :=
           'Validation summary'
        || CHR(10) || 'emp_cnt='   || v_emp_cnt
        || CHR(10) || 'audit_cnt=' || v_audit_cnt
        || CHR(10) || 'json_cnt='  || v_json_cnt
        || CHR(10) || 'pipe_cnt='  || v_pipe_cnt
        || CHR(10) || q'[fake end of block:
BEGIN
    NULL;
END;
/]';

    DBMS_OUTPUT.PUT_LINE(v_text);

    qt_fb_log_proc(
        p_module => 'final_validation',
        p_action => 'SUMMARY',
        p_msg    => v_text,
        p_extra  => DBMS_UTILITY.FORMAT_CALL_STACK
    );
END;
/

--------------------------------------------------------------------------------
-- 19. FINAL RESULT QUERIES
--------------------------------------------------------------------------------
SELECT *
  FROM qt_fb_view
 ORDER BY dept_id, emp_id;
/

SELECT audit_id, module_name, action_name, SUBSTR(message_text, 1, 120) AS msg_preview
  FROM qt_fb_audit
 ORDER BY audit_id;
/
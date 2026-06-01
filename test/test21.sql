--------------------------------------------------------------------------------
-- ORACLE QUERY TOOL FINAL BOSS - EXTREME CHAOS EDITION
-- 목적:
--   - 실행단위 분리기 최종 검증
--   - depth 계산 / BEGIN~END 매칭 검증
--   - 문자열/주석/q-quote/정규식/JSON/XML 오탐 검증
--   - package / trigger / procedure / function / type / view / merge / dynamic SQL 검증
--   - slash(/) 처리 검증
--
-- 주의:
--   - 여러 객체를 생성/삭제함
--   - SQL*Plus 스타일 "/" 실행 단위 포함
--   - 간단한 테스트 전부 제거
--------------------------------------------------------------------------------

--------------------------------------------------------------------------------
-- 0. CLEANUP
--------------------------------------------------------------------------------
BEGIN
    FOR r IN
    (
        SELECT object_name, object_type
          FROM user_objects
         WHERE object_name IN
         (
             'QT_X_DEPT',
             'QT_X_EMP',
             'QT_X_AUDIT',
             'QT_X_STAGE',
             'QT_X_JSON',
             'QT_X_XML',
             'QT_X_SEQ',
             'QT_X_OBJ',
             'QT_X_OBJ_TAB',
             'QT_X_NESTED_TAB',
             'QT_X_UTIL_PKG',
             'QT_X_CHAOS_PKG',
             'QT_X_TRG_BIU',
             'QT_X_VIEW',
             'QT_X_LOG_PROC',
             'QT_X_PIPE_FN',
             'QT_X_ERR_LOG'
         )
         ORDER BY
             CASE object_type
                 WHEN 'TRIGGER' THEN 1
                 WHEN 'VIEW' THEN 2
                 WHEN 'PACKAGE' THEN 3
                 WHEN 'PROCEDURE' THEN 4
                 WHEN 'FUNCTION' THEN 5
                 WHEN 'TABLE' THEN 6
                 WHEN 'TYPE' THEN 7
                 WHEN 'SEQUENCE' THEN 8
                 ELSE 9
             END
    )
    LOOP
        BEGIN
            IF r.object_type = 'TRIGGER' THEN
                EXECUTE IMMEDIATE 'DROP TRIGGER ' || r.object_name;
            ELSIF r.object_type = 'VIEW' THEN
                EXECUTE IMMEDIATE 'DROP VIEW ' || r.object_name;
            ELSIF r.object_type = 'PACKAGE' THEN
                EXECUTE IMMEDIATE 'DROP PACKAGE ' || r.object_name;
            ELSIF r.object_type = 'PROCEDURE' THEN
                EXECUTE IMMEDIATE 'DROP PROCEDURE ' || r.object_name;
            ELSIF r.object_type = 'FUNCTION' THEN
                EXECUTE IMMEDIATE 'DROP FUNCTION ' || r.object_name;
            ELSIF r.object_type = 'TABLE' THEN
                EXECUTE IMMEDIATE 'DROP TABLE ' || r.object_name || ' PURGE';
            ELSIF r.object_type = 'TYPE' THEN
                EXECUTE IMMEDIATE 'DROP TYPE ' || r.object_name || ' FORCE';
            ELSIF r.object_type = 'SEQUENCE' THEN
                EXECUTE IMMEDIATE 'DROP SEQUENCE ' || r.object_name;
            END IF;
        EXCEPTION
            WHEN OTHERS THEN
                NULL;
        END;
    END LOOP;
END;
/
--------------------------------------------------------------------------------
-- 1. BASE TABLES
--------------------------------------------------------------------------------
CREATE TABLE qt_x_dept
(
    dept_id            NUMBER         CONSTRAINT qt_x_dept_pk PRIMARY KEY,
    dept_name          VARCHAR2(200)  NOT NULL,
    parent_dept_id     NUMBER,
    dept_code          VARCHAR2(30),
    meta_json          CLOB CHECK (meta_json IS JSON),
    note_text          CLOB
);
/

CREATE TABLE qt_x_emp
(
    emp_id             NUMBER         CONSTRAINT qt_x_emp_pk PRIMARY KEY,
    dept_id            NUMBER         NOT NULL,
    emp_name           VARCHAR2(200)  NOT NULL,
    login_name         VARCHAR2(200),
    salary             NUMBER(12,2),
    bonus              NUMBER(12,2),
    hire_dt            DATE,
    status             VARCHAR2(30),
    note_text          CLOB,
    raw_json           CLOB CHECK (raw_json IS JSON),
    xml_payload        XMLTYPE,
    calc_expr          VARCHAR2(4000),
    created_at         TIMESTAMP DEFAULT SYSTIMESTAMP,
    updated_at         TIMESTAMP,
    CONSTRAINT qt_x_emp_fk1 FOREIGN KEY (dept_id) REFERENCES qt_x_dept(dept_id)
);
/

CREATE TABLE qt_x_stage
(
    stage_id           NUMBER PRIMARY KEY,
    dept_id            NUMBER,
    emp_name           VARCHAR2(200),
    salary             NUMBER(12,2),
    bonus              NUMBER(12,2),
    hire_dt            DATE,
    status             VARCHAR2(30),
    note_text          CLOB,
    raw_json           CLOB CHECK (raw_json IS JSON)
);
/

CREATE TABLE qt_x_audit
(
    audit_id           NUMBER PRIMARY KEY,
    module_name        VARCHAR2(200),
    action_name        VARCHAR2(200),
    message_text       CLOB,
    extra_text         CLOB,
    created_at         TIMESTAMP DEFAULT SYSTIMESTAMP
);
/

CREATE TABLE qt_x_json
(
    doc_id             NUMBER PRIMARY KEY,
    doc_body           CLOB CHECK (doc_body IS JSON),
    created_at         TIMESTAMP DEFAULT SYSTIMESTAMP
);
/

CREATE TABLE qt_x_xml
(
    doc_id             NUMBER PRIMARY KEY,
    doc_body           XMLTYPE,
    created_at         TIMESTAMP DEFAULT SYSTIMESTAMP
);
/

CREATE TABLE qt_x_err_log
(
    err_id             NUMBER PRIMARY KEY,
    err_module         VARCHAR2(200),
    err_code           NUMBER,
    err_msg            CLOB,
    err_stack          CLOB,
    created_at         TIMESTAMP DEFAULT SYSTIMESTAMP
);
/

CREATE SEQUENCE qt_x_seq START WITH 1 INCREMENT BY 1 NOCACHE;
/

--------------------------------------------------------------------------------
-- 2. TYPES
--------------------------------------------------------------------------------
CREATE OR REPLACE TYPE qt_x_obj AS OBJECT
(
    emp_id             NUMBER,
    emp_name           VARCHAR2(200),
    salary             NUMBER,
    status             VARCHAR2(30)
);
/

CREATE OR REPLACE TYPE qt_x_obj_tab AS TABLE OF qt_x_obj;
/

CREATE OR REPLACE TYPE qt_x_nested_tab AS TABLE OF VARCHAR2(4000);
/

--------------------------------------------------------------------------------
-- 3. EXTREME SEED DATA
--------------------------------------------------------------------------------
INSERT INTO qt_x_dept
(
    dept_id, dept_name, parent_dept_id, dept_code, meta_json, note_text
)
VALUES
(
    10,
    'ROOT',
    NULL,
    'ROOT',
    q'!{
        "region":"KR",
        "flags":["root","top","begin;end/"],
        "note":"contains ; / -- /* */ CREATE OR REPLACE"
    }!',
    q'[
root note line 1;
root note line 2 /
fake block:
BEGIN
    NULL;
END;
]'
);

INSERT INTO qt_x_dept
(
    dept_id, dept_name, parent_dept_id, dept_code, meta_json, note_text
)
VALUES
(
    20,
    'PLATFORM',
    10,
    'PLT',
    q'~{
        "region":"US",
        "flags":["dev","api","q''[x;y/z]''"],
        "note":"/* comment-like */ -- line-comment-like"
    }~',
    q'!dept text with weird chars [](){}<>; / \ | ^ $ . * + ? !'
);

INSERT INTO qt_x_dept
(
    dept_id, dept_name, parent_dept_id, dept_code, meta_json, note_text
)
VALUES
(
    30,
    'DATA',
    10,
    'DAT',
    q'#{
        "region":"JP",
        "flags":["etl","ml","pivot/unpivot"],
        "note":"MATCH_RECOGNIZE MODEL JSON_TABLE XMLTABLE"
    }#',
    q'<xml-like <a x="1;2/3">TEXT</a> fake slash / fake ;>'
);
/

INSERT INTO qt_x_stage
SELECT
    101,
    20,
    'ALICE',
    11000,
    700,
    DATE '2021-01-01',
    'ACTIVE',
    q'[
ALICE note;
contains / slash
contains fake trigger:
CREATE OR REPLACE TRIGGER x
BEFORE INSERT ON y
BEGIN
    NULL;
END;
/
]',
    q'!{"name":"ALICE","tags":["A","BEGIN;END/","/*x*/","--y"]}!'
FROM dual
UNION ALL
SELECT
    102,
    20,
    'BOB',
    14500,
    500,
    DATE '2022-02-02',
    'LEAVE',
    q'!BOB says: q''[abc;def/]'' and q'~ghi/~'!',
    q'~{"name":"BOB","tags":["regex","[](){}","^$.*+?","/"]}~'
FROM dual
UNION ALL
SELECT
    103,
    30,
    'CAROL',
    17000,
    900,
    DATE '2020-03-03',
    'ACTIVE',
    q'#CAROL text with "quotes", ''single quotes'', ; ; ;, / / /#',
    q'#{"name":"CAROL","tags":["json","xml","case","loop"]}#'
FROM dual
UNION ALL
SELECT
    104,
    30,
    'DAVE',
    8000,
    NULL,
    DATE '2023-04-04',
    'INACTIVE',
    q'~DAVE text with fake comment open /* and close */ and --line~',
    q'~{"name":"DAVE","tags":["inactive","merge","dynamic sql"]}~'
FROM dual;
/

INSERT INTO qt_x_json(doc_id, doc_body)
VALUES
(
    1,
    q'!{
      "meta":{
        "title":"boss-json",
        "text":"BEGIN; END; / /* */ -- q''[x;y/z]''"
      },
      "items":[
        {"k":"k1","v":"v1;"},
        {"k":"k2","v":"v2/"},
        {"k":"k3","v":"/*not comment*/"},
        {"k":"k4","v":"--not line comment"},
        {"k":"k5","v":"CREATE OR REPLACE PACKAGE x IS END; /"}
      ],
      "nested":{"a":{"b":{"c":[1,2,3,4]}}}
    }!'
);
/

INSERT INTO qt_x_xml(doc_id, doc_body)
VALUES
(
    1,
    XMLTYPE(q'[
        <root>
            <meta text="BEGIN;END;/"/>
            <item k="k1">value-1</item>
            <item k="k2">value/2</item>
            <item k="k3">/*not comment*/</item>
            <item k="k4">--not line comment</item>
        </root>
    ]')
);
/

COMMIT;
/

--------------------------------------------------------------------------------
-- 4. LOGGER PROCEDURE (AUTONOMOUS TRANSACTION)
--------------------------------------------------------------------------------
CREATE OR REPLACE PROCEDURE qt_x_log_proc
(
    p_module   IN VARCHAR2,
    p_action   IN VARCHAR2,
    p_msg      IN CLOB,
    p_extra    IN CLOB DEFAULT NULL
)
IS
    PRAGMA AUTONOMOUS_TRANSACTION;
BEGIN
    INSERT INTO qt_x_audit
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
        qt_x_seq.NEXTVAL,
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
-- 5. UTILITY PACKAGE
--------------------------------------------------------------------------------
CREATE OR REPLACE PACKAGE qt_x_util_pkg
IS
    FUNCTION weird_text(p_seed NUMBER) RETURN CLOB;
    FUNCTION normalize_text(p_txt CLOB) RETURN CLOB;
    FUNCTION json_probe(p_doc_id NUMBER, p_path VARCHAR2) RETURN VARCHAR2;
    PROCEDURE save_error(p_module VARCHAR2, p_code NUMBER, p_msg CLOB, p_stack CLOB);
END qt_x_util_pkg;
/
CREATE OR REPLACE PACKAGE BODY qt_x_util_pkg
IS
    FUNCTION weird_text(p_seed NUMBER) RETURN CLOB
    IS
        v_text CLOB;
    BEGIN
        v_text :=
               q'[line-1 ; / BEGIN END IF LOOP CASE]'
            || CHR(10)
            || q'!line-2 /* not comment */ -- not line comment!'
            || CHR(10)
            || q'~line-3 q''[abc;def/]'' q'!ghi!' q'#jkl/#'~'
            || CHR(10)
            || 'seed=' || p_seed
            || CHR(10)
            || q'[
fake package:
CREATE OR REPLACE PACKAGE p IS
END;
/
]';

        RETURN v_text;
    END weird_text;

    FUNCTION normalize_text(p_txt CLOB) RETURN CLOB
    IS
        v_ret CLOB;
    BEGIN
        v_ret :=
            REGEXP_REPLACE(
                REPLACE(REPLACE(p_txt, CHR(13), NULL), CHR(9), '    '),
                '([;\/])',
                '[\1]'
            );

        RETURN v_ret;
    END normalize_text;

    FUNCTION json_probe(p_doc_id NUMBER, p_path VARCHAR2) RETURN VARCHAR2
    IS
        v_ret VARCHAR2(4000);
        v_sql VARCHAR2(32767);
    BEGIN
        v_sql :=
               'SELECT JSON_VALUE(doc_body, '''
            || REPLACE(p_path, '''', '''''')
            || ''') FROM qt_x_json WHERE doc_id = :x';

        EXECUTE IMMEDIATE v_sql INTO v_ret USING p_doc_id;

        RETURN v_ret;
    END json_probe;

    PROCEDURE save_error(p_module VARCHAR2, p_code NUMBER, p_msg CLOB, p_stack CLOB)
    IS
        PRAGMA AUTONOMOUS_TRANSACTION;
    BEGIN
        INSERT INTO qt_x_err_log
        (
            err_id, err_module, err_code, err_msg, err_stack, created_at
        )
        VALUES
        (
            qt_x_seq.NEXTVAL, p_module, p_code, p_msg, p_stack, SYSTIMESTAMP
        );

        COMMIT;
    EXCEPTION
        WHEN OTHERS THEN
            ROLLBACK;
            NULL;
    END save_error;
END qt_x_util_pkg;
/
--------------------------------------------------------------------------------
-- 6. MAIN PACKAGE SPEC
--------------------------------------------------------------------------------
CREATE OR REPLACE PACKAGE qt_x_chaos_pkg
IS
    SUBTYPE t_name IS VARCHAR2(200);

    TYPE t_num_tab  IS TABLE OF NUMBER INDEX BY PLS_INTEGER;
    TYPE t_name_tab IS TABLE OF t_name INDEX BY PLS_INTEGER;

    g_last_message       CLOB;
    g_last_dynamic_block CLOB;

    FUNCTION calc_grade(p_salary NUMBER, p_bonus NUMBER) RETURN VARCHAR2;
    FUNCTION pipe_rows(p_min_salary NUMBER) RETURN qt_x_obj_tab PIPELINED;

    PROCEDURE load_stage_to_emp(
        p_raise_pct       NUMBER DEFAULT 0,
        p_commit_interval NUMBER DEFAULT 0
    );

    PROCEDURE run_chaos_report(
        p_dept_id         NUMBER,
        p_status          VARCHAR2 DEFAULT NULL,
        p_count_out       OUT NUMBER
    );

    PROCEDURE execute_dynamic_hell(
        p_mode            VARCHAR2 DEFAULT 'FULL'
    );

    PROCEDURE test_everything;
END qt_x_chaos_pkg;
/
--------------------------------------------------------------------------------
-- 7. MAIN PACKAGE BODY
--------------------------------------------------------------------------------
CREATE OR REPLACE PACKAGE BODY qt_x_chaos_pkg
IS
    FUNCTION calc_grade(p_salary NUMBER, p_bonus NUMBER) RETURN VARCHAR2
    IS
        v_total NUMBER := NVL(p_salary, 0) + NVL(p_bonus, 0);
    BEGIN
        RETURN
            CASE
                WHEN v_total >= 18000 THEN 'S'
                WHEN v_total >= 14000 THEN 'A'
                WHEN v_total >= 10000 THEN 'B'
                WHEN v_total >=  7000 THEN 'C'
                ELSE 'D'
            END;
    END calc_grade;

    FUNCTION pipe_rows(p_min_salary NUMBER) RETURN qt_x_obj_tab PIPELINED
    IS
    BEGIN
        FOR r IN
        (
            SELECT emp_id, emp_name, salary, status
              FROM qt_x_emp
             WHERE salary >= p_min_salary
             ORDER BY salary DESC, emp_id
        )
        LOOP
            PIPE ROW (qt_x_obj(r.emp_id, r.emp_name, r.salary, r.status));
        END LOOP;

        RETURN;
    END pipe_rows;

    PROCEDURE load_stage_to_emp(
        p_raise_pct       NUMBER DEFAULT 0,
        p_commit_interval NUMBER DEFAULT 0
    )
    IS
        TYPE t_stage_tab IS TABLE OF qt_x_stage%ROWTYPE INDEX BY PLS_INTEGER;
        v_stage_tab        t_stage_tab;
        v_sql              CLOB;
        v_i                PLS_INTEGER;
        v_commit_cnt       NUMBER := 0;
    BEGIN
        SELECT *
          BULK COLLECT INTO v_stage_tab
          FROM qt_x_stage
         ORDER BY stage_id;

        v_sql := q'[
MERGE INTO qt_x_emp t
USING
(
    SELECT :1 AS emp_id,
           :2 AS dept_id,
           :3 AS emp_name,
           :4 AS login_name,
           :5 AS salary,
           :6 AS bonus,
           :7 AS hire_dt,
           :8 AS status,
           :9 AS note_text,
           :10 AS raw_json
      FROM dual
) s
   ON (t.emp_id = s.emp_id)
 WHEN MATCHED THEN
     UPDATE SET
         t.dept_id     = s.dept_id,
         t.emp_name    = s.emp_name,
         t.login_name  = s.login_name,
         t.salary      = s.salary,
         t.bonus       = s.bonus,
         t.hire_dt     = s.hire_dt,
         t.status      = s.status,
         t.note_text   = s.note_text,
         t.raw_json    = s.raw_json,
         t.xml_payload = XMLTYPE('<emp><name>' || s.emp_name || '</name><status>' || s.status || '</status></emp>'),
         t.calc_expr   = 'salary + bonus / 2 ; fake',
         t.updated_at  = SYSTIMESTAMP
 WHEN NOT MATCHED THEN
     INSERT
     (
         emp_id, dept_id, emp_name, login_name, salary, bonus, hire_dt, status,
         note_text, raw_json, xml_payload, calc_expr, created_at, updated_at
     )
     VALUES
     (
         s.emp_id, s.dept_id, s.emp_name, s.login_name, s.salary, s.bonus, s.hire_dt, s.status,
         s.note_text, s.raw_json,
         XMLTYPE('<emp><name>' || s.emp_name || '</name><status>' || s.status || '</status></emp>'),
         'CASE WHEN x THEN y END; /',
         SYSTIMESTAMP, SYSTIMESTAMP
     )
]';

        v_i := v_stage_tab.FIRST;

        WHILE v_i IS NOT NULL
        LOOP
            EXECUTE IMMEDIATE v_sql
                USING
                    v_stage_tab(v_i).stage_id,
                    v_stage_tab(v_i).dept_id,
                    v_stage_tab(v_i).emp_name,
                    LOWER(v_stage_tab(v_i).emp_name) || '@example.com',
                    ROUND(v_stage_tab(v_i).salary * (1 + NVL(p_raise_pct,0)/100), 2),
                    v_stage_tab(v_i).bonus,
                    v_stage_tab(v_i).hire_dt,
                    v_stage_tab(v_i).status,
                    v_stage_tab(v_i).note_text,
                    v_stage_tab(v_i).raw_json;

            v_commit_cnt := v_commit_cnt + 1;

            IF p_commit_interval > 0 AND MOD(v_commit_cnt, p_commit_interval) = 0 THEN
                COMMIT;
            END IF;

            v_i := v_stage_tab.NEXT(v_i);
        END LOOP;

        qt_x_log_proc(
            p_module => 'qt_x_chaos_pkg.load_stage_to_emp',
            p_action => 'MERGE',
            p_msg    => qt_x_util_pkg.weird_text(101),
            p_extra  => q'!load finished ; / BEGIN END /* */ -- !'
        );
    EXCEPTION
        WHEN OTHERS THEN
            qt_x_util_pkg.save_error(
                'qt_x_chaos_pkg.load_stage_to_emp',
                SQLCODE,
                SQLERRM,
                DBMS_UTILITY.FORMAT_ERROR_BACKTRACE
            );
            RAISE;
    END load_stage_to_emp;

    PROCEDURE run_chaos_report(
        p_dept_id         NUMBER,
        p_status          VARCHAR2 DEFAULT NULL,
        p_count_out       OUT NUMBER
    )
    IS
        v_sql   CLOB;
    BEGIN
        v_sql := q'[
WITH base AS
(
    SELECT e.emp_id,
           e.emp_name,
           e.salary,
           e.bonus,
           e.status,
           CASE
               WHEN NVL(e.salary,0) + NVL(e.bonus,0) >= 18000 THEN 'S'
               WHEN NVL(e.salary,0) + NVL(e.bonus,0) >= 14000 THEN 'A'
               WHEN NVL(e.salary,0) + NVL(e.bonus,0) >= 10000 THEN 'B'
               ELSE 'C'
           END AS grade
      FROM qt_x_emp e
     WHERE e.dept_id = :b1
       AND (:b2 IS NULL OR e.status = :b2)
)
SELECT COUNT(*)
  FROM base
 WHERE EXISTS
       (
           SELECT 1
             FROM qt_x_dept d
            WHERE d.dept_id = :b1
              AND JSON_EXISTS(d.meta_json, '$.flags[*]?(@ == "dev" || @ == "etl" || @ == "ml")')
       )
]';

        EXECUTE IMMEDIATE v_sql INTO p_count_out USING p_dept_id, p_status, p_status, p_dept_id;

        qt_x_log_proc(
            p_module => 'qt_x_chaos_pkg.run_chaos_report',
            p_action => 'COUNT',
            p_msg    => 'count=' || p_count_out,
            p_extra  => q'[dynamic WITH/CASE/EXISTS ; / fake slash]'
        );
    EXCEPTION
        WHEN OTHERS THEN
            qt_x_util_pkg.save_error(
                'qt_x_chaos_pkg.run_chaos_report',
                SQLCODE,
                SQLERRM,
                DBMS_UTILITY.FORMAT_ERROR_STACK || CHR(10) || DBMS_UTILITY.FORMAT_ERROR_BACKTRACE
            );
            RAISE;
    END run_chaos_report;

    PROCEDURE execute_dynamic_hell(
        p_mode            VARCHAR2 DEFAULT 'FULL'
    )
    IS
        v_block1 CLOB;
        v_block2 CLOB;
        v_x      NUMBER;
    BEGIN
        v_block1 := q'[
DECLARE
    v_txt   CLOB;
    v_cnt   NUMBER;
BEGIN
    v_txt := q'[dynamic text ; / BEGIN END /* */ -- ]';

    SELECT COUNT(*) INTO v_cnt
      FROM qt_x_emp
     WHERE status IN ('ACTIVE','LEAVE','INACTIVE');

    INSERT INTO qt_x_audit
    (
        audit_id, module_name, action_name, message_text, extra_text, created_at
    )
    VALUES
    (
        qt_x_seq.NEXTVAL,
        'dynamic-1',
        'INSERT',
        v_txt,
        'count=' || v_cnt,
        SYSTIMESTAMP
    );
END;
]';

        v_block2 := q'~
DECLARE
    v_sql  VARCHAR2(32767);
    v_val  VARCHAR2(4000);
BEGIN
    v_sql := 'SELECT JSON_VALUE(doc_body, ''$.meta.text'') FROM qt_x_json WHERE doc_id = :x';
    EXECUTE IMMEDIATE v_sql INTO v_val USING 1;

    INSERT INTO qt_x_audit
    (
        audit_id, module_name, action_name, message_text, extra_text, created_at
    )
    VALUES
    (
        qt_x_seq.NEXTVAL,
        'dynamic-2',
        'JSON_READ',
        v_val,
        q'[contains q''[abc;def/]'' and slash / and semicolon ;]',
        SYSTIMESTAMP
    );
END;
~';

        g_last_dynamic_block := v_block1 || CHR(10) || '-----' || CHR(10) || v_block2;

        EXECUTE IMMEDIATE v_block1;

        IF UPPER(NVL(p_mode, 'FULL')) = 'FULL' THEN
            EXECUTE IMMEDIATE v_block2;
        ELSE
            v_x := 1 / 1;
        END IF;

        qt_x_log_proc(
            p_module => 'qt_x_chaos_pkg.execute_dynamic_hell',
            p_action => 'DONE',
            p_msg    => g_last_dynamic_block,
            p_extra  => 'mode=' || p_mode
        );
    EXCEPTION
        WHEN OTHERS THEN
            qt_x_util_pkg.save_error(
                'qt_x_chaos_pkg.execute_dynamic_hell',
                SQLCODE,
                SQLERRM,
                DBMS_UTILITY.FORMAT_ERROR_STACK || CHR(10) || DBMS_UTILITY.FORMAT_CALL_STACK
            );
            RAISE;
    END execute_dynamic_hell;

    PROCEDURE test_everything
    IS
        v_num_tab         t_num_tab;
        v_name_tab        t_name_tab;
        v_idx             PLS_INTEGER;
        v_count           NUMBER := 0;
        v_count2          NUMBER := 0;
        v_json_text       VARCHAR2(4000);
        v_cursor          SYS_REFCURSOR;
        v_emp_id          NUMBER;
        v_emp_name        VARCHAR2(200);
        v_salary          NUMBER;
        v_bonus           NUMBER;
        v_grade           VARCHAR2(30);
        v_dynamic_sql     CLOB;
        e_test            EXCEPTION;
        PRAGMA EXCEPTION_INIT(e_test, -20011);

        FUNCTION inner_format(p_txt VARCHAR2) RETURN VARCHAR2
        IS
        BEGIN
            RETURN REGEXP_REPLACE(
                       REPLACE(
                           REPLACE(p_txt, ';', '[;]'),
                           '/', '[/]'
                       ),
                       '(BEGIN|END|CASE|LOOP|IF)',
                       '<\1>'
                   );
        END inner_format;
    BEGIN
        v_num_tab(1) := 10;
        v_num_tab(2) := 20;
        v_num_tab(3) := 30;

        v_name_tab(1) := 'ALICE';
        v_name_tab(2) := q'[BOB; / BEGIN END]';
        v_name_tab(3) := q'!CAROL /* */ -- !';

        v_json_text := qt_x_util_pkg.json_probe(1, '$.meta.text');

        OPEN v_cursor FOR
            WITH x AS
            (
                SELECT e.emp_id,
                       e.emp_name,
                       e.salary,
                       e.bonus,
                       ROW_NUMBER() OVER (ORDER BY e.salary DESC NULLS LAST, e.emp_id) AS rn
                  FROM qt_x_emp e
                 WHERE e.dept_id IN
                 (
                     SELECT COLUMN_VALUE
                       FROM TABLE(sys.odcinumberlist(20, 30))
                 )
            )
            SELECT emp_id, emp_name, salary, bonus
              FROM x
             WHERE rn <= 999;

        LOOP
            FETCH v_cursor INTO v_emp_id, v_emp_name, v_salary, v_bonus;
            EXIT WHEN v_cursor%NOTFOUND;

            v_count := v_count + 1;
            v_grade := calc_grade(v_salary, v_bonus);

            BEGIN
                IF v_grade = 'S' THEN
                    v_dynamic_sql := q'[
                        BEGIN
                            INSERT INTO qt_x_audit
                            (
                                audit_id, module_name, action_name, message_text, extra_text, created_at
                            )
                            VALUES
                            (
                                qt_x_seq.NEXTVAL,
                                'inner-dynamic',
                                'S-GRADE',
                                q'[top grade ; / BEGIN END]',
                                :x,
                                SYSTIMESTAMP
                            );
                        END;
                    ]';

                    EXECUTE IMMEDIATE v_dynamic_sql USING 'emp=' || v_emp_id || ', grade=' || v_grade;
                ELSIF v_salary IS NULL THEN
                    RAISE_APPLICATION_ERROR(-20011, 'salary null ; /');
                ELSE
                    qt_x_log_proc(
                        p_module => 'qt_x_chaos_pkg.test_everything',
                        p_action => 'ROW',
                        p_msg    => 'emp=' || v_emp_id || ', grade=' || v_grade,
                        p_extra  => inner_format(v_json_text)
                    );
                END IF;

                v_count2 :=
                    CASE
                        WHEN v_grade IN ('S','A') THEN v_count2 + 10
                        WHEN v_grade = 'B' THEN v_count2 + 5
                        ELSE v_count2 + 1
                    END;
            EXCEPTION
                WHEN e_test THEN
                    qt_x_log_proc(
                        p_module => 'qt_x_chaos_pkg.test_everything',
                        p_action => 'E_TEST',
                        p_msg    => SQLERRM,
                        p_extra  => DBMS_UTILITY.FORMAT_ERROR_BACKTRACE
                    );
                WHEN OTHERS THEN
                    qt_x_log_proc(
                        p_module => 'qt_x_chaos_pkg.test_everything',
                        p_action => 'OTHERS',
                        p_msg    => SQLERRM,
                        p_extra  => DBMS_UTILITY.FORMAT_CALL_STACK
                    );
            END;
        END LOOP;

        CLOSE v_cursor;

        g_last_message :=
            q'[
FINAL PACKAGE MESSAGE
1) ; semicolon
2) / slash
3) fake block
   BEGIN
       NULL;
   END;
4) fake object
   CREATE OR REPLACE VIEW v AS SELECT 1 FROM dual;
5) fake end slash
   /
6) fake comments
   /* x */
   -- y
]';

        qt_x_log_proc(
            p_module => 'qt_x_chaos_pkg.test_everything',
            p_action => 'DONE',
            p_msg    => g_last_message,
            p_extra  => 'count=' || v_count || ', score=' || v_count2
        );
    EXCEPTION
        WHEN OTHERS THEN
            qt_x_util_pkg.save_error(
                'qt_x_chaos_pkg.test_everything',
                SQLCODE,
                SQLERRM,
                DBMS_UTILITY.FORMAT_ERROR_STACK || CHR(10) || DBMS_UTILITY.FORMAT_ERROR_BACKTRACE
            );
            RAISE;
    END test_everything;
END qt_x_chaos_pkg;
/
--------------------------------------------------------------------------------
-- 8. TRIGGER
--------------------------------------------------------------------------------
CREATE OR REPLACE TRIGGER qt_x_trg_biu
BEFORE INSERT OR UPDATE ON qt_x_emp
FOR EACH ROW
DECLARE
    v_msg CLOB;
BEGIN
    v_msg :=
           'trigger fired; emp_id=' || :NEW.emp_id
        || '; old=' || NVL(:OLD.status, 'NULL')
        || '; new=' || NVL(:NEW.status, 'NULL')
        || '; fake slash / fake semicolon ;';

    IF :NEW.salary < 0 THEN
        RAISE_APPLICATION_ERROR(-20021, 'negative salary ; /');
    END IF;

    IF INSERTING AND :NEW.created_at IS NULL THEN
        :NEW.created_at := SYSTIMESTAMP;
    END IF;

    :NEW.updated_at := SYSTIMESTAMP;
END;
/
--------------------------------------------------------------------------------
-- 9. VIEW WITH HIERARCHY + JSON_TABLE + XMLTABLE + ANALYTIC + SCALAR SUBQUERY
--------------------------------------------------------------------------------
CREATE OR REPLACE VIEW qt_x_view
AS
WITH dept_tree AS
(
    SELECT d.dept_id,
           d.dept_name,
           d.parent_dept_id,
           LEVEL AS lvl,
           SYS_CONNECT_BY_PATH(d.dept_name, ' > ') AS path_txt,
           d.meta_json,
           d.note_text
      FROM qt_x_dept d
     START WITH d.parent_dept_id IS NULL
   CONNECT BY PRIOR d.dept_id = d.parent_dept_id
),
emp_ext AS
(
    SELECT e.emp_id,
           e.dept_id,
           e.emp_name,
           e.salary,
           e.bonus,
           e.status,
           e.hire_dt,
           ROW_NUMBER() OVER (PARTITION BY e.dept_id ORDER BY e.salary DESC NULLS LAST, e.emp_id) AS rn,
           DENSE_RANK() OVER (ORDER BY e.salary DESC NULLS LAST) AS dr,
           SUM(NVL(e.salary,0)) OVER (PARTITION BY e.dept_id) AS dept_sal_sum,
           (SELECT COUNT(*) FROM qt_x_audit a WHERE a.module_name LIKE 'qt_x_chaos_pkg%') AS audit_cnt
      FROM qt_x_emp e
)
SELECT
    dt.dept_id,
    dt.dept_name,
    dt.lvl,
    dt.path_txt,
    ex.emp_id,
    ex.emp_name,
    ex.salary,
    ex.bonus,
    ex.status,
    ex.rn,
    ex.dr,
    ex.dept_sal_sum,
    ex.audit_cnt,
    jt.region,
    jt.flag1,
    xt.item_text
FROM dept_tree dt
LEFT JOIN emp_ext ex
    ON ex.dept_id = dt.dept_id
LEFT JOIN JSON_TABLE
(
    dt.meta_json,
    '$'
    COLUMNS
    (
        region VARCHAR2(100) PATH '$.region',
        flag1  VARCHAR2(100) PATH '$.flags[0]'
    )
) jt
    ON 1 = 1
LEFT JOIN XMLTABLE
(
    '/root/item[1]'
    PASSING XMLTYPE('<root><item>tag-1</item><item>tag-2</item></root>')
    COLUMNS item_text VARCHAR2(100) PATH '.'
) xt
    ON 1 = 1;
/

--------------------------------------------------------------------------------
-- 10. EXECUTION BLOCKS
--------------------------------------------------------------------------------
BEGIN
    qt_x_chaos_pkg.load_stage_to_emp(
        p_raise_pct       => 7.25,
        p_commit_interval => 2
    );
END;
/

DECLARE
    v_cnt NUMBER;
BEGIN
    qt_x_chaos_pkg.run_chaos_report(
        p_dept_id   => 20,
        p_status    => 'ACTIVE',
        p_count_out => v_cnt
    );

    DBMS_OUTPUT.PUT_LINE('chaos report cnt=' || v_cnt);
END;
/

BEGIN
    qt_x_chaos_pkg.execute_dynamic_hell('FULL');
END;
/

BEGIN
    qt_x_chaos_pkg.test_everything;
END;
/

--------------------------------------------------------------------------------
-- 11. HARDEST SQL SET
--------------------------------------------------------------------------------

-- 11-1. Deep WITH + nested CASE + cursor expression + scalar subquery
WITH base AS
(
    SELECT e.emp_id,
           e.emp_name,
           e.dept_id,
           e.salary,
           e.bonus,
           e.status,
           e.hire_dt,
           ROW_NUMBER() OVER (PARTITION BY e.dept_id ORDER BY e.salary DESC NULLS LAST, e.emp_id) AS rn,
           LAG(e.salary)  OVER (PARTITION BY e.dept_id ORDER BY e.salary DESC NULLS LAST, e.emp_id) AS prev_sal,
           LEAD(e.salary) OVER (PARTITION BY e.dept_id ORDER BY e.salary DESC NULLS LAST, e.emp_id) AS next_sal,
           (SELECT d.dept_name FROM qt_x_dept d WHERE d.dept_id = e.dept_id) AS dept_name
      FROM qt_x_emp e
),
graded AS
(
    SELECT b.*,
           CASE
               WHEN NVL(b.salary,0) + NVL(b.bonus,0) >= 18000 THEN
                   CASE
                       WHEN b.status = 'ACTIVE' THEN 'S-ACT'
                       ELSE 'S-NON'
                   END
               WHEN NVL(b.salary,0) + NVL(b.bonus,0) >= 14000 THEN 'A'
               WHEN NVL(b.salary,0) + NVL(b.bonus,0) >= 10000 THEN 'B'
               ELSE 'C'
           END AS grade
      FROM base b
)
SELECT
    g.*,
    CURSOR
    (
        SELECT a.audit_id, a.action_name, a.created_at
          FROM qt_x_audit a
         WHERE a.created_at >= SYSTIMESTAMP - INTERVAL '1' DAY
    ) AS audit_cur
FROM graded g
WHERE EXISTS
(
    SELECT 1
      FROM qt_x_dept d
     WHERE d.dept_id = g.dept_id
       AND JSON_EXISTS(d.meta_json, '$.flags[*]?(@ == "dev" || @ == "etl" || @ == "ml")')
)
ORDER BY g.dept_id, g.salary DESC, g.emp_id;
/

-- 11-2. PIVOT
SELECT *
  FROM
  (
      SELECT dept_id, status, salary
        FROM qt_x_emp
  )
  PIVOT
  (
      SUM(salary)
      FOR status IN
      (
          'ACTIVE'   AS active_sal,
          'LEAVE'    AS leave_sal,
          'INACTIVE' AS inactive_sal
      )
  )
 ORDER BY dept_id;
/

-- 11-3. UNPIVOT
SELECT dept_id, metric_name, metric_value
  FROM
  (
      SELECT dept_id,
             SUM(NVL(salary,0)) AS total_salary,
             SUM(NVL(bonus,0))  AS total_bonus
        FROM qt_x_emp
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

-- 11-4. MODEL
SELECT dept_id, seq_no, calc_value
  FROM
  (
      SELECT dept_id,
             ROW_NUMBER() OVER (PARTITION BY dept_id ORDER BY emp_id) AS seq_no,
             NVL(salary,0) AS calc_value
        FROM qt_x_emp
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

-- 11-5. MATCH_RECOGNIZE
SELECT *
  FROM qt_x_emp
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

-- 11-6. JSON_TABLE
SELECT j.doc_id,
       t.k,
       t.v
  FROM qt_x_json j,
       JSON_TABLE
       (
           j.doc_body,
           '$.items[*]'
           COLUMNS
           (
               k VARCHAR2(100) PATH '$.k',
               v VARCHAR2(4000) PATH '$.v'
           )
       ) t
 ORDER BY j.doc_id, t.k;
/

-- 11-7. XMLTABLE
SELECT x.doc_id,
       t.k,
       t.v
  FROM qt_x_xml x,
       XMLTABLE
       (
           '/root/item'
           PASSING x.doc_body
           COLUMNS
               k FOR ORDINALITY,
               v VARCHAR2(4000) PATH '.'
       ) t
 ORDER BY x.doc_id, t.k;
/

-- 11-8. REGEXP + q-quote + hint
SELECT /*+ qb_name(q_main) */
       e.emp_id,
       e.emp_name,
       REGEXP_REPLACE(
           q'[A;B/C(1)[x]{y}|z^$.*+?]',
           '([;\[\]\(\)\{\}\|\^\$\.\*\+\?\/])',
           '<\1>'
       ) AS regex_out,
       qt_x_util_pkg.normalize_text(e.note_text) AS normalized_text
  FROM qt_x_emp e
 ORDER BY e.emp_id;
/

--------------------------------------------------------------------------------
-- 12. PIPELINED FUNCTION USAGE
--------------------------------------------------------------------------------
SELECT *
  FROM TABLE(qt_x_chaos_pkg.pipe_rows(9000))
 ORDER BY salary DESC, emp_id;
/

--------------------------------------------------------------------------------
-- 13. MERGE WITH COMPLEX SOURCE
--------------------------------------------------------------------------------
MERGE INTO qt_x_emp t
USING
(
    WITH src AS
    (
        SELECT
            2001 AS emp_id,
            20   AS dept_id,
            'EVE' AS emp_name,
            'eve@example.com' AS login_name,
            12345 AS salary,
            321   AS bonus,
            DATE '2024-08-08' AS hire_dt,
            'ACTIVE' AS status,
            q'[EVE note ; / BEGIN END /* */]' AS note_text,
            q'!{"name":"EVE","tags":["merge","with","slash/semicolon;"]}!' AS raw_json
        FROM dual
    )
    SELECT * FROM src
) s
ON (t.emp_id = s.emp_id)
WHEN MATCHED THEN
    UPDATE SET
        t.salary      = s.salary,
        t.bonus       = s.bonus,
        t.note_text   = s.note_text,
        t.raw_json    = s.raw_json,
        t.updated_at  = SYSTIMESTAMP
WHEN NOT MATCHED THEN
    INSERT
    (
        emp_id, dept_id, emp_name, login_name, salary, bonus, hire_dt, status,
        note_text, raw_json, xml_payload, calc_expr, created_at, updated_at
    )
    VALUES
    (
        s.emp_id, s.dept_id, s.emp_name, s.login_name, s.salary, s.bonus, s.hire_dt, s.status,
        s.note_text, s.raw_json,
        XMLTYPE('<emp><name>' || s.emp_name || '</name></emp>'),
        '1 + 2 / 3 ; fake',
        SYSTIMESTAMP, SYSTIMESTAMP
    );
/

--------------------------------------------------------------------------------
-- 14. BULK COLLECT + FORALL + SAVE EXCEPTIONS
--------------------------------------------------------------------------------
DECLARE
    TYPE t_emp_id_tab IS TABLE OF NUMBER INDEX BY PLS_INTEGER;
    TYPE t_bonus_tab  IS TABLE OF NUMBER INDEX BY PLS_INTEGER;

    v_emp_ids  t_emp_id_tab;
    v_bonus    t_bonus_tab;

    bulk_errors EXCEPTION;
    PRAGMA EXCEPTION_INIT(bulk_errors, -24381);
BEGIN
    SELECT emp_id,
           NVL(bonus,0) + CASE WHEN status = 'ACTIVE' THEN 11 ELSE 7 END
      BULK COLLECT INTO v_emp_ids, v_bonus
      FROM qt_x_emp
     WHERE dept_id IN (20,30)
     ORDER BY emp_id;

    BEGIN
        FORALL i IN INDICES OF v_emp_ids SAVE EXCEPTIONS
            UPDATE qt_x_emp
               SET bonus = v_bonus(i),
                   note_text = NVL(note_text, EMPTY_CLOB()) || CHR(10) || 'bulk idx=' || i || '; /'
             WHERE emp_id = v_emp_ids(i);
    EXCEPTION
        WHEN bulk_errors THEN
            FOR j IN 1 .. SQL%BULK_EXCEPTIONS.COUNT
            LOOP
                qt_x_log_proc(
                    p_module => 'bulk_forall',
                    p_action => 'SAVE_EXCEPTIONS',
                    p_msg    => 'err_index=' || SQL%BULK_EXCEPTIONS(j).ERROR_INDEX,
                    p_extra  => 'err_code='  || SQL%BULK_EXCEPTIONS(j).ERROR_CODE
                );
            END LOOP;
    END;
END;
/

--------------------------------------------------------------------------------
-- 15. DYNAMIC DDL/PLSQL MIX
--------------------------------------------------------------------------------
DECLARE
    v_stmt1 CLOB;
    v_stmt2 CLOB;
BEGIN
    v_stmt1 := q'[
        DECLARE
            v_x NUMBER := 1;
            v_t CLOB := q'[inner dynamic ; / BEGIN END]';
        BEGIN
            INSERT INTO qt_x_audit
            (
                audit_id, module_name, action_name, message_text, extra_text, created_at
            )
            VALUES
            (
                qt_x_seq.NEXTVAL,
                'dyn-mix-1',
                'INSERT',
                v_t,
                'ok',
                SYSTIMESTAMP
            );
        END;
    ]';

    v_stmt2 := q'[
        BEGIN
            INSERT INTO qt_x_audit
            (
                audit_id, module_name, action_name, message_text, extra_text, created_at
            )
            VALUES
            (
                qt_x_seq.NEXTVAL,
                'dyn-mix-2',
                'INSERT',
                q'[CREATE OR REPLACE TRIGGER x BEGIN NULL; END; /]',
                q'[/* */ -- ; /]',
                SYSTIMESTAMP
            );
        END;
    ]';

    EXECUTE IMMEDIATE v_stmt1;
    EXECUTE IMMEDIATE v_stmt2;
END;
/

--------------------------------------------------------------------------------
-- 16. SELECTION-EXECUTION KILLER BLOCK
-- 특정 구간만 선택 실행할 때도 잘 버티는지 보기 좋음
--------------------------------------------------------------------------------
DECLARE
    v_text1   CLOB := q'[
text-1 ;
/
BEGIN
NULL;
END;
]';
    v_text2   CLOB := q'!text-2 /*x*/ --y q''[z;w/]''!';
    v_sql     VARCHAR2(32767);
    v_cnt     NUMBER;
BEGIN
    v_sql := 'SELECT COUNT(*) FROM qt_x_emp WHERE emp_name LIKE :x';
    EXECUTE IMMEDIATE v_sql INTO v_cnt USING '%A%';

    IF v_cnt > 0 THEN
        qt_x_log_proc(
            p_module => 'selection_killer',
            p_action => 'COUNT_A',
            p_msg    => v_text1,
            p_extra  => v_text2
        );
    ELSE
        qt_x_log_proc(
            p_module => 'selection_killer',
            p_action => 'COUNT_ZERO',
            p_msg    => 'none',
            p_extra  => 'none'
        );
    END IF;
END;
/

--------------------------------------------------------------------------------
-- 17. FINAL VALIDATION BLOCK
--------------------------------------------------------------------------------
DECLARE
    v_emp_cnt        NUMBER;
    v_audit_cnt      NUMBER;
    v_err_cnt        NUMBER;
    v_json_cnt       NUMBER;
    v_pipe_cnt       NUMBER;
    v_msg            CLOB;
BEGIN
    SELECT COUNT(*) INTO v_emp_cnt   FROM qt_x_emp;
    SELECT COUNT(*) INTO v_audit_cnt FROM qt_x_audit;
    SELECT COUNT(*) INTO v_err_cnt   FROM qt_x_err_log;
    SELECT COUNT(*) INTO v_json_cnt
      FROM qt_x_json
     WHERE JSON_EXISTS(doc_body, '$.nested.a.b.c[*]?(@ == 3)');

    SELECT COUNT(*) INTO v_pipe_cnt
      FROM TABLE(qt_x_chaos_pkg.pipe_rows(7000));

    v_msg :=
           'FINAL VALIDATION'
        || CHR(10) || 'emp_cnt='   || v_emp_cnt
        || CHR(10) || 'audit_cnt=' || v_audit_cnt
        || CHR(10) || 'err_cnt='   || v_err_cnt
        || CHR(10) || 'json_cnt='  || v_json_cnt
        || CHR(10) || 'pipe_cnt='  || v_pipe_cnt
        || CHR(10) || q'[
fake block:
BEGIN
    NULL;
END;
/
fake comment:
-- not real
/* not real */
]';

    DBMS_OUTPUT.PUT_LINE(v_msg);

    qt_x_log_proc(
        p_module => 'final_validation',
        p_action => 'SUMMARY',
        p_msg    => v_msg,
        p_extra  => DBMS_UTILITY.FORMAT_CALL_STACK
    );
END;
/

--------------------------------------------------------------------------------
-- 18. FINAL RESULT QUERIES
--------------------------------------------------------------------------------
SELECT *
  FROM qt_x_view
 ORDER BY dept_id, emp_id;
/

SELECT audit_id,
       module_name,
       action_name,
       SUBSTR(message_text, 1, 160) AS msg_preview
  FROM qt_x_audit
 ORDER BY audit_id;
/

SELECT err_id,
       err_module,
       err_code,
       SUBSTR(err_msg, 1, 160) AS err_msg_preview
  FROM qt_x_err_log
 ORDER BY err_id;
/
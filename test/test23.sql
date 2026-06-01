--------------------------------------------------------------------------------
-- FINAL BOSS: Oracle Execution-Unit Splitter Torture Script
-- Expected executable unit count: 21
--
-- 목표:
-- 1) ; 가 문자열/주석/q-quote/dynamic sql 내부에 있어도 잘못 끊기면 안 됨
-- 2) / 가 "문자열 내부 줄 단독", "주석 내부 줄 단독"으로 있어도 잘못 끊기면 안 됨
-- 3) CREATE TYPE / PACKAGE / TRIGGER / FUNCTION / PROCEDURE / ANON BLOCK 의
--    진짜 종료용 slash line 만 인식해야 함
-- 4) DDL / DML / PL-SQL / MERGE / INSERT ALL / VIEW / TYPE / TABLE FUNCTION 혼합
--------------------------------------------------------------------------------
--------------------------------------------------------------------------------
-- UNIT 01: cleanup block
--------------------------------------------------------------------------------

DECLARE
    PROCEDURE drop_ignore (p_sql VARCHAR2) IS
    BEGIN
        EXECUTE IMMEDIATE p_sql;
    EXCEPTION
        WHEN OTHERS THEN
            NULL;
    END;

BEGIN
    drop_ignore ('DROP TRIGGER qt_boss_emp_biu_trg');
    drop_ignore ('DROP VIEW qt_boss_emp_v');
    drop_ignore ('DROP PACKAGE qt_boss_pkg');
    drop_ignore ('DROP PROCEDURE qt_boss_rebuild_note');
    drop_ignore ('DROP FUNCTION qt_boss_pairs');
    drop_ignore ('DROP TYPE qt_boss_pair_tab');
    drop_ignore ('DROP TYPE BODY qt_boss_pair_obj');
    drop_ignore ('DROP TYPE qt_boss_pair_obj');
    drop_ignore ('DROP SEQUENCE qt_boss_audit_seq');
    drop_ignore ('DROP TABLE qt_boss_audit PURGE');
    drop_ignore ('DROP TABLE qt_boss_emp PURGE');
    drop_ignore ('DROP TABLE qt_boss_cfg PURGE');
    drop_ignore ('DROP TABLE qt_boss_dept PURGE');
END;
/

--------------------------------------------------------------------------------
-- UNIT 02: base table - dept
--------------------------------------------------------------------------------

CREATE TABLE qt_boss_dept (
    dept_id    NUMBER        CONSTRAINT qt_boss_dept_pk PRIMARY KEY,
    dept_name  VARCHAR2(100) NOT NULL,
    region     VARCHAR2(50)  NOT NULL,
    enabled_yn CHAR(1)       DEFAULT 'Y' NOT NULL CONSTRAINT qt_boss_dept_ck1 CHECK(enabled_yn IN('Y', 'N')),
    created_at DATE          DEFAULT SYSDATE NOT NULL
);

--------------------------------------------------------------------------------
-- UNIT 03: base table - emp
--------------------------------------------------------------------------------

CREATE TABLE qt_boss_emp (
    emp_id     NUMBER         CONSTRAINT qt_boss_emp_pk PRIMARY KEY,
    dept_id    NUMBER         NOT NULL CONSTRAINT qt_boss_emp_fk1 REFERENCES qt_boss_dept(dept_id),
    emp_name   VARCHAR2(200)  NOT NULL,
    login_name VARCHAR2(200),
    salary     NUMBER(12, 2),
    status     VARCHAR2(30)   DEFAULT 'ACTIVE' NOT NULL CONSTRAINT qt_boss_emp_ck1 CHECK(status IN('ACTIVE', 'INACTIVE', 'ON_HOLD', 'ARCHIVED')),
    note_text  VARCHAR2(4000),
    created_at DATE           DEFAULT SYSDATE NOT NULL,
    updated_at DATE,
    CONSTRAINT qt_boss_emp_uk1 UNIQUE(login_name)
);

--------------------------------------------------------------------------------
-- UNIT 04: base table - audit
--------------------------------------------------------------------------------

CREATE TABLE qt_boss_audit (
    audit_id   NUMBER         CONSTRAINT qt_boss_audit_pk PRIMARY KEY,
    emp_id     NUMBER,
    tag        VARCHAR2(50)   NOT NULL,
    msg        VARCHAR2(4000),
    created_at DATE           DEFAULT SYSDATE NOT NULL,
    upper_tag  VARCHAR2(50)   GENERATED ALWAYS AS(UPPER(tag)) VIRTUAL
);

--------------------------------------------------------------------------------
-- UNIT 05: base table - cfg
--------------------------------------------------------------------------------

CREATE TABLE qt_boss_cfg (
    dept_id    NUMBER         NOT NULL CONSTRAINT qt_boss_cfg_fk1 REFERENCES qt_boss_dept(dept_id),
    cfg_key    VARCHAR2(100)  NOT NULL,
    cfg_val    VARCHAR2(4000),
    sort_ord   NUMBER         DEFAULT 1 NOT NULL,
    created_at DATE           DEFAULT SYSDATE NOT NULL,
    CONSTRAINT qt_boss_cfg_pk PRIMARY KEY(dept_id, cfg_key)
);

--------------------------------------------------------------------------------
-- UNIT 06: sequence
--------------------------------------------------------------------------------

CREATE SEQUENCE qt_boss_audit_seq
START WITH 1 INCREMENT BY 1 NOCACHE;

--------------------------------------------------------------------------------
-- UNIT 07: seed dept + cfg with multi-table insert
--------------------------------------------------------------------------------

INSERT ALL
INTO qt_boss_dept (dept_id, dept_name, region, enabled_yn, created_at)
VALUES (10, 'ENGINEERING', 'SEOUL', 'Y', SYSDATE)
INTO qt_boss_dept (dept_id, dept_name, region, enabled_yn, created_at)
VALUES (20, 'OPS', 'BUSAN', 'Y', SYSDATE)
INTO qt_boss_dept (dept_id, dept_name, region, enabled_yn, created_at)
VALUES (30, 'DATA', 'DAEJEON', 'N', SYSDATE)
INTO qt_boss_cfg (dept_id, cfg_key, cfg_val, sort_ord, created_at)
VALUES (10, 'MODE', 'STRICT', 1, SYSDATE)
INTO qt_boss_cfg (dept_id, cfg_key, cfg_val, sort_ord, created_at)
VALUES (10, 'THRESHOLD', '9000', 2, SYSDATE)
INTO qt_boss_cfg (dept_id, cfg_key, cfg_val, sort_ord, created_at)
VALUES (20, 'MODE', 'BALANCED', 1, SYSDATE)
INTO qt_boss_cfg (dept_id, cfg_key, cfg_val, sort_ord, created_at)
VALUES (30, 'MODE', 'ARCHIVE_ONLY', 1, SYSDATE)
SELECT 1
FROM DUAL;

--------------------------------------------------------------------------------
-- UNIT 08: seed emp with fake delimiters embedded in data
--------------------------------------------------------------------------------

INSERT ALL
INTO qt_boss_emp (emp_id, dept_id, emp_name, login_name, salary, status, note_text, created_at, updated_at)
VALUES (1, 10, 'ALICE', 'alice', 5000, 'ACTIVE', q'[
alpha line 1 ; still text
/
alpha line 3 -- not delimiter
/* text, not comment */
]', SYSDATE, SYSDATE)
INTO qt_boss_emp (emp_id, dept_id, emp_name, login_name, salary, status, note_text, created_at, updated_at)
VALUES (2, 10, 'BOB', 'bob', 6200, 'ACTIVE', q'~Bob's payload ; slash=/ ; tokens=/*x*/ -- y~', SYSDATE, SYSDATE)
INTO qt_boss_emp (emp_id, dept_id, emp_name, login_name, salary, status, note_text, created_at, updated_at)
VALUES (3, 10, 'CAROL', 'carol', 8100, 'ON_HOLD', q'[
BEGIN pretend;
/
END pretend;
-- still just text
]', SYSDATE, SYSDATE)
INTO qt_boss_emp (emp_id, dept_id, emp_name, login_name, salary, status, note_text, created_at, updated_at)
VALUES (4, 20, 'DAVE', 'dave', 4300, 'INACTIVE', q'!select * from dual; / merge; not real!', SYSDATE, SYSDATE)
SELECT 1
FROM DUAL;

--------------------------------------------------------------------------------
-- UNIT 09: view with CTE + analytic + scalar subquery + comment bait
--------------------------------------------------------------------------------

CREATE OR REPLACE VIEW qt_boss_emp_v AS
WITH dept_base AS (
    SELECT
        d.dept_id,
        d.dept_name,
        d.region,
        MAX (
            CASE
                WHEN c.cfg_key = 'MODE' THEN c.cfg_val
            END
        ) AS mode_cfg
    FROM qt_boss_dept d
    LEFT JOIN qt_boss_cfg c
        ON c.dept_id = d.dept_id
    GROUP BY d.dept_id,
        d.dept_name,
        d.region
),
emp_ranked AS (
    SELECT
        e.emp_id,
        e.dept_id,
        e.emp_name,
        e.login_name,
        e.salary,
        e.status,
        e.note_text,
        ROW_NUMBER () OVER (PARTITION BY e.dept_id ORDER BY e.salary DESC NULLS LAST, e.emp_id) AS rn,
        COUNT (*) OVER (PARTITION BY e.dept_id) AS cnt_in_dept
    FROM qt_boss_emp e
)
/* splitter bait: ; / BEGIN END; */
SELECT
    er.emp_id,
    er.dept_id,
    db.dept_name,
    er.emp_name,
    er.login_name,
    er.salary,
    er.status,
    er.rn,
    er.cnt_in_dept,
    CASE
        WHEN er.note_text LIKE '%;%'
        OR er.note_text LIKE '%/%' THEN 'HAS_DELIM'
        ELSE 'PLAIN'
    END AS note_class,
    (
        SELECT MAX (a.created_at)
        FROM qt_boss_audit a
        WHERE a.emp_id = er.emp_id
    ) AS last_audit_at,
    db.mode_cfg AS mode_cfg
FROM emp_ranked er
JOIN dept_base db
    ON db.dept_id = er.dept_id;

--------------------------------------------------------------------------------
-- UNIT 10: object type spec
--------------------------------------------------------------------------------

CREATE OR REPLACE TYPE qt_boss_pair_obj AS
    OBJECT (key_txt VARCHAR2 (100), val_txt VARCHAR2 (4000), MEMBER FUNCTION render RETURN VARCHAR2);
/

--------------------------------------------------------------------------------
-- UNIT 11: object type body
--------------------------------------------------------------------------------

CREATE OR REPLACE TYPE BODY qt_boss_pair_obj IS
    MEMBER FUNCTION render RETURN VARCHAR2 IS
    BEGIN
        RETURN key_txt || ' => ' || REPLACE (SUBSTR (val_txt, 1, 120), CHR (10), ' | ');
    END render;

END;
/

--------------------------------------------------------------------------------
-- UNIT 12: collection type
--------------------------------------------------------------------------------

CREATE OR REPLACE TYPE qt_boss_pair_tab AS
    TABLE OF qt_boss_pair_obj;
/

--------------------------------------------------------------------------------
-- UNIT 13: pipelined table function
--------------------------------------------------------------------------------

CREATE OR REPLACE FUNCTION qt_boss_pairs (p_dept_id NUMBER) RETURN qt_boss_pair_tab PIPELINED IS
BEGIN
    FOR r IN (
        SELECT 'EMP:' || TO_CHAR (e.emp_id) AS key_txt,
            q'[name=]' || e.emp_name || q'[; status=]' || e.status || q'[; note=]' || NVL (SUBSTR (REPLACE (e.note_text, CHR (10), ' | '), 1, 120), '(null)') AS val_txt
        FROM qt_boss_emp e
        WHERE e.dept_id = p_dept_id
        ORDER BY e.emp_id
    ) LOOP
        PIPE ROW (qt_boss_pair_obj (r.key_txt, r.val_txt));
    END LOOP;
    RETURN;
END;
/

--------------------------------------------------------------------------------
-- UNIT 14: package spec
--------------------------------------------------------------------------------

CREATE OR REPLACE PACKAGE qt_boss_pkg IS
    SUBTYPE t_flag IS VARCHAR2 (1);
    TYPE t_num_tab IS TABLE OF NUMBER INDEX BY PLS_INTEGER;
    TYPE t_vc_tab IS TABLE OF VARCHAR2 (4000) INDEX BY PLS_INTEGER;
    g_tag CONSTANT VARCHAR2 (30) := 'QT_BOSS';
    PROCEDURE seed_audit (p_emp_id NUMBER, p_tag VARCHAR2, p_msg VARCHAR2);
    FUNCTION build_note (p_emp_name VARCHAR2, p_dept_id NUMBER, p_status VARCHAR2) RETURN VARCHAR2;
    PROCEDURE mutate_emp (p_emp_id NUMBER, p_new_salary NUMBER, p_status VARCHAR2 DEFAULT NULL, p_append_text VARCHAR2 DEFAULT NULL);
    FUNCTION checksum_for_emp (p_emp_id NUMBER) RETURN NUMBER;
    PROCEDURE run_extreme (p_dept_id NUMBER);
END qt_boss_pkg;
/

--------------------------------------------------------------------------------
-- UNIT 15: package body (real final boss)
--------------------------------------------------------------------------------

CREATE OR REPLACE PACKAGE BODY qt_boss_pkg IS
    PROCEDURE log_it (p_emp_id NUMBER, p_tag VARCHAR2, p_msg VARCHAR2) IS
        PRAGMA AUTONOMOUS_TRANSACTION;
    BEGIN
        INSERT INTO qt_boss_audit (audit_id, emp_id, tag, msg, created_at)
        VALUES (qt_boss_audit_seq.NEXTVAL, p_emp_id, p_tag, SUBSTR (p_msg, 1, 4000), SYSDATE);
        COMMIT;
    EXCEPTION
        WHEN OTHERS THEN
            ROLLBACK;
            RAISE;
    END log_it;

    PROCEDURE seed_audit (p_emp_id NUMBER, p_tag VARCHAR2, p_msg VARCHAR2) IS
    BEGIN
        INSERT INTO qt_boss_audit (audit_id, emp_id, tag, msg, created_at)
        VALUES (qt_boss_audit_seq.NEXTVAL, p_emp_id, p_tag, SUBSTR (p_msg, 1, 4000), SYSDATE);
    END seed_audit;

    FUNCTION build_note (p_emp_name VARCHAR2, p_dept_id NUMBER, p_status VARCHAR2) RETURN VARCHAR2 IS
        v_note VARCHAR2 (4000);
    BEGIN
        /*
            splitter bait inside comment:
            BEGIN
            /
            END;
        */
        v_note := q'[
BEGIN fake_inner;
/
END fake_inner;
]' || CHR (10) || 'emp=' || p_emp_name || '; dept=' || TO_CHAR (p_dept_id) || '; status=' || p_status || '; marker=/*text*/ -- text only';
        RETURN SUBSTR (v_note, 1, 4000);
    END build_note;

    PROCEDURE mutate_emp (p_emp_id NUMBER, p_new_salary NUMBER, p_status VARCHAR2 DEFAULT NULL, p_append_text VARCHAR2 DEFAULT NULL) IS
        v_sql VARCHAR2 (32767);
        v_append VARCHAR2 (4000);
    BEGIN
        v_sql := q'[
UPDATE qt_boss_emp
   SET salary     = :b1,
       status     = COALESCE(:b2, status),
       note_text  = SUBSTR(note_text || :b3, 1, 4000),
       updated_at = SYSDATE
 WHERE emp_id = :b4
]';
        v_append :=
        CASE
            WHEN p_append_text IS NOT NULL THEN
                CHR (10) || p_append_text
            ELSE
                ''
        END;
        EXECUTE IMMEDIATE v_sql
        USING p_new_salary,
            p_status,
            v_append,
            p_emp_id;
    END mutate_emp;

    FUNCTION checksum_for_emp (p_emp_id NUMBER) RETURN NUMBER IS
        v_txt VARCHAR2 (4000);
        v_sum NUMBER := 0;
    BEGIN
        SELECT emp_name || '|' || NVL (status, '?') || '|' || NVL (TO_CHAR (salary), '0')
        INTO v_txt
        FROM qt_boss_emp
        WHERE emp_id = p_emp_id;
        FOR i IN 1..LENGTH (v_txt) LOOP
            v_sum := v_sum + ASCII (SUBSTR (v_txt, i, 1)) * i;
        END LOOP;
        RETURN v_sum;
    EXCEPTION
        WHEN NO_DATA_FOUND THEN
            RETURN - 1;
    END checksum_for_emp;

    PROCEDURE run_extreme (p_dept_id NUMBER) IS
        v_names t_vc_tab;
        v_note VARCHAR2 (4000);
        v_plsql VARCHAR2 (32767);
        v_salary NUMBER;
    BEGIN
        v_names (1) := q'[alpha ; beta / gamma -- delta]';
        v_names (2) := q'[
line-1
/
line-2 ; END;
]';
        v_names (3) := q'~"quoted"; /*text*/ / final~';
        <<outer_loop>>
        FOR r IN (
            SELECT emp_id,
                emp_name,
                salary
            FROM qt_boss_emp
            WHERE dept_id = p_dept_id
            ORDER BY emp_id
            FOR
            UPDATE OF salary,
                status
        ) LOOP
            BEGIN
                v_salary := NVL (r.salary, 0) +
                CASE
                        WHEN MOD (r.emp_id, 2) = 0 THEN
                        17
                        ELSE
                        29
                END;
                v_note := build_note (r.emp_name, p_dept_id, 
                CASE
                    WHEN MOD (r.emp_id, 2) = 0 THEN
                        'EVEN'
                    ELSE
                        'ODD'
                END
                    );
                mutate_emp (p_emp_id => r.emp_id, p_new_salary => v_salary, p_status =>
                CASE
                    WHEN MOD (r.emp_id, 3) = 0 THEN
                        'ON_HOLD'
                    ELSE
                        'ACTIVE'
                    END, p_append_text => v_note || CHR (10) || v_names (MOD (r.emp_id - 1, 3) + 1));
                v_plsql := q'[
DECLARE
    v_txt VARCHAR2(200) := q'~dynamic ; slash / end;~';
BEGIN
    INSERT INTO qt_boss_audit
    (
        audit_id, emp_id, tag, msg, created_at
    )
    VALUES
    (
        qt_boss_audit_seq.NEXTVAL,
        :x1,
        'DYN',
        'dynamic-block=>' || v_txt,
        SYSDATE
    );
END;
]';
                EXECUTE IMMEDIATE v_plsql
                USING r.emp_id;
                IF MOD (r.emp_id, 5) = 0 THEN
                    log_it (r.emp_id, 'AUTO', 'checkpoint; slash=/; name=' || r.emp_name);
                ELSE
                    seed_audit (r.emp_id, 'STEP', 'mutated=>' || r.emp_name || '; cs=' || checksum_for_emp (r.emp_id));
                END IF;
            EXCEPTION
                WHEN OTHERS THEN
                    log_it (r.emp_id, 'ERR', SQLERRM || '; unit=run_extreme');
                    RAISE;
            END;
        END LOOP outer_loop;
    END run_extreme;
END qt_boss_pkg;
/

--------------------------------------------------------------------------------
-- UNIT 16: row trigger with fake slash inside comment and q-quote
--------------------------------------------------------------------------------

CREATE OR REPLACE TRIGGER qt_boss_emp_biu_trg
    BEFORE INSERT OR UPDATE ON qt_boss_emp
    FOR EACH ROW
DECLARE
    v_msg VARCHAR2 (4000);
BEGIN
    /*
        trigger bait
        /
        end bait;
    */
    IF INSERTING THEN
        :NEW.created_at := NVL (:NEW.created_at, SYSDATE);
    END IF;
    :NEW.updated_at := SYSDATE;
    IF :NEW.status IS
            NULL THEN
            :NEW.status := 'ACTIVE';
    END IF;
        v_msg := q'[
trigger-text
/
not-the-end;
]';
        IF :NEW.note_text IS NULL THEN
            :NEW.note_text := SUBSTR ('TRG=' || REPLACE (v_msg, CHR (10), ' ') || ' :: ' || :NEW.emp_name, 1, 4000);
    END IF;
END;
/

--------------------------------------------------------------------------------
-- UNIT 17: standalone procedure
--------------------------------------------------------------------------------

CREATE OR REPLACE PROCEDURE qt_boss_rebuild_note (p_emp_id NUMBER) AUTHID CURRENT_USER IS
    v_name qt_boss_emp.emp_name%TYPE;
    v_dept qt_boss_emp.dept_id%TYPE;
    v_status qt_boss_emp.status%TYPE;
BEGIN
    SELECT emp_name,
        dept_id,
        status
    INTO v_name,
        v_dept,
        v_status
    FROM qt_boss_emp
    WHERE emp_id = p_emp_id;
    UPDATE qt_boss_emp
    SET note_text = qt_boss_pkg.build_note (v_name, v_dept, v_status)
    WHERE emp_id = p_emp_id;
END;
/

--------------------------------------------------------------------------------
-- UNIT 18: MERGE with string data containing delimiters
--------------------------------------------------------------------------------

MERGE INTO qt_boss_emp t
USING (
    SELECT 101 AS emp_id,
        10 AS dept_id,
        'NEO;ONE' AS emp_name,
        'neo.one' AS login_name,
        7100 AS salary,
        'ACTIVE' AS status,
        q'[
merge-insert
/
still-text ; not delimiter
]'         AS note_text
    FROM DUAL
    UNION ALL
    SELECT 2,
        10,
        'BOB',
        'bob',
        9200,
        'ON_HOLD',
        q'[matched; update / text]'
    FROM DUAL
) s
    ON (t.emp_id = s.emp_id)
    WHEN MATCHED THEN
UPDATE
SET t.salary = s.salary,
    t.status = s.status,
    t.note_text = SUBSTR (t.note_text || CHR (10) || s.note_text, 1, 4000),
    t.updated_at = SYSDATE
    WHEN NOT MATCHED THEN
INSERT (emp_id, dept_id, emp_name, login_name, salary, status, note_text, created_at, updated_at)
VALUES (s.emp_id, s.dept_id, s.emp_name, s.login_name, s.salary, s.status, s.note_text, SYSDATE, SYSDATE);

--------------------------------------------------------------------------------
-- UNIT 19: anonymous block with nested dynamic PL/SQL
--------------------------------------------------------------------------------

DECLARE
    v_dummy NUMBER;
    v_note VARCHAR2 (4000) := q'[
anon-block-start
/
anon-block-middle; end;
]';
BEGIN
    qt_boss_pkg.seed_audit (1, 'PRE', 'before-run=>' || REPLACE (v_note, CHR (10), '|'));
    qt_boss_pkg.run_extreme (10);
    qt_boss_rebuild_note (2);
    EXECUTE IMMEDIATE q'[
BEGIN
    INSERT INTO qt_boss_audit
    (
        audit_id, emp_id, tag, msg, created_at
    )
    VALUES
    (
        qt_boss_audit_seq.NEXTVAL,
        :x1,
        'ANON_DYN',
        q'~dyn text ; / not the end~',
        SYSDATE
    );
END;
]'
    USING 3;
    SELECT qt_boss_pkg.checksum_for_emp (2)
    INTO v_dummy
    FROM DUAL;
    INSERT INTO qt_boss_audit (audit_id, emp_id, tag, msg, created_at)
    VALUES (qt_boss_audit_seq.NEXTVAL, 2, 'CHK', 'checksum=' || TO_CHAR (v_dummy) || '; note=' || SUBSTR (v_note, 1, 60), SYSDATE);
END;
/

--------------------------------------------------------------------------------
-- UNIT 20: INSERT ALL from view + table function
--------------------------------------------------------------------------------

INSERT ALL
INTO qt_boss_audit (audit_id, emp_id, tag, msg, created_at)
VALUES (qt_boss_audit_seq.NEXTVAL, emp_id, 'SNAP', msg, SYSDATE)
SELECT v.emp_id AS emp_id,
    'view_status=' || v.status || '; note_class=' || v.note_class || '; render=' || NVL (
        (
                SELECT MIN (p.render ())
                FROM TABLE (qt_boss_pairs (v.dept_id)) p
                WHERE p.key_txt = 'EMP:' || TO_CHAR (v.emp_id)
            ),
            'NONE'
    ) AS msg
FROM qt_boss_emp_v v
WHERE v.emp_id IN (1, 2, 3, 101);

--------------------------------------------------------------------------------
-- UNIT 21: final verification query
--------------------------------------------------------------------------------

-- SELECT v.emp_id,
    -- v.emp_name,
    -- v.status,
    -- v.note_class,
    -- (
        -- SELECT MIN (p.render ())
        -- FROM TABLE (qt_boss_pairs (v.dept_id)) p
        -- WHERE p.key_txt = 'EMP:' || TO_CHAR (v.emp_id)
    -- ) AS pair_render,
    -- TO_CHAR (v.last_audit_at, 'YYYY-MM-DD HH24:MI:SS') AS last_audit_at
-- FROM qt_boss_emp_v v
-- WHERE v.emp_id IN (1, 2, 3, 101)
-- ORDER BY v.emp_id;
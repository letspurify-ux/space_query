--------------------------------------------------------------------------------
-- UNIT 001
-- SQL*Plus 스타일 명령: splitter 정책상 독립 단위로 취급 권장
--------------------------------------------------------------------------------
SET DEFINE ON

--------------------------------------------------------------------------------
-- UNIT 002
--------------------------------------------------------------------------------
SET SERVEROUTPUT ON

--------------------------------------------------------------------------------
-- UNIT 003
--------------------------------------------------------------------------------
PROMPT === QT SPLITTER FINAL ULTIMATE BOSS START ===

--------------------------------------------------------------------------------
-- UNIT 004
--------------------------------------------------------------------------------
ALTER SESSION SET NLS_DATE_FORMAT = 'YYYY-MM-DD HH24:MI:SS';

--------------------------------------------------------------------------------
-- UNIT 005
--------------------------------------------------------------------------------
BEGIN
    EXECUTE IMMEDIATE 'DROP VIEW qt_splitter_ultimate_v';
EXCEPTION
    WHEN OTHERS THEN
        NULL;
END;
/

--------------------------------------------------------------------------------
-- UNIT 006
--------------------------------------------------------------------------------
BEGIN
    EXECUTE IMMEDIATE 'DROP TRIGGER qt_splitter_ultimate_trg';
EXCEPTION
    WHEN OTHERS THEN
        NULL;
END;
/

--------------------------------------------------------------------------------
-- UNIT 007
--------------------------------------------------------------------------------
BEGIN
    EXECUTE IMMEDIATE 'DROP PACKAGE qt_splitter_ultimate_pkg';
EXCEPTION
    WHEN OTHERS THEN
        NULL;
END;
/

--------------------------------------------------------------------------------
-- UNIT 008
--------------------------------------------------------------------------------
BEGIN
    EXECUTE IMMEDIATE 'DROP TYPE qt_splitter_force_obj';
EXCEPTION
    WHEN OTHERS THEN
        NULL;
END;
/

--------------------------------------------------------------------------------
-- UNIT 009
--------------------------------------------------------------------------------
BEGIN
    EXECUTE IMMEDIATE 'DROP TYPE qt_splitter_force_tab';
EXCEPTION
    WHEN OTHERS THEN
        NULL;
END;
/

--------------------------------------------------------------------------------
-- UNIT 010
--------------------------------------------------------------------------------
BEGIN
    EXECUTE IMMEDIATE 'DROP PROCEDURE qt_splitter_ultimate_proc';
EXCEPTION
    WHEN OTHERS THEN
        NULL;
END;
/

--------------------------------------------------------------------------------
-- UNIT 011
--------------------------------------------------------------------------------
BEGIN
    EXECUTE IMMEDIATE 'DROP FUNCTION qt_splitter_ultimate_func';
EXCEPTION
    WHEN OTHERS THEN
        NULL;
END;
/

--------------------------------------------------------------------------------
-- UNIT 012
--------------------------------------------------------------------------------
BEGIN
    EXECUTE IMMEDIATE 'DROP TABLE qt_splitter_ultimate PURGE';
EXCEPTION
    WHEN OTHERS THEN
        NULL;
END;
/

--------------------------------------------------------------------------------
-- UNIT 013
-- quoted identifier, reserved-like column names, slash/semicolon in literals
--------------------------------------------------------------------------------
CREATE TABLE qt_splitter_ultimate
(
    id                NUMBER                          PRIMARY KEY,
    parent_id         NUMBER,
    grp               NUMBER                          NOT NULL,
    "DATE"            DATE                            DEFAULT SYSDATE,
    "COMMENT"         VARCHAR2(4000),
    name              VARCHAR2(200),
    status_cd         VARCHAR2(30),
    amount            NUMBER(18,2),
    calc_text         VARCHAR2(4000),
    payload           CLOB,
    json_text         CLOB,
    created_at        TIMESTAMP                       DEFAULT SYSTIMESTAMP,
    updated_at        TIMESTAMP,
    constraint qt_splitter_ultimate_ck1
        CHECK (status_cd IN ('NEW', 'DONE', 'VIP', 'HOLD', 'ERR'))
);

--------------------------------------------------------------------------------
-- UNIT 014
--------------------------------------------------------------------------------
CREATE INDEX qt_splitter_ultimate_ix1
    ON qt_splitter_ultimate (grp, status_cd, amount);

--------------------------------------------------------------------------------
-- UNIT 015
-- 오브젝트 타입 스펙: "/" 단독 라인 종결
--------------------------------------------------------------------------------
CREATE OR REPLACE TYPE qt_splitter_force_obj AS OBJECT
(
    id          NUMBER,
    name        VARCHAR2(200),
    note_text   VARCHAR2(4000),
    MEMBER FUNCTION render RETURN VARCHAR2
);
/

--------------------------------------------------------------------------------
-- UNIT 016
-- 오브젝트 타입 바디: 내부 ; 많음, 끝은 "/" 이어야 함
--------------------------------------------------------------------------------
CREATE OR REPLACE TYPE BODY qt_splitter_force_obj
AS
    MEMBER FUNCTION render RETURN VARCHAR2
    IS
    BEGIN
        RETURN 'id=' || id || ';name=' || name || ';note=' || note_text || '/tail';
    END render;
END;
/

--------------------------------------------------------------------------------
-- UNIT 017
--------------------------------------------------------------------------------
CREATE OR REPLACE TYPE qt_splitter_force_tab AS TABLE OF qt_splitter_force_obj;
/

--------------------------------------------------------------------------------
-- UNIT 018
-- INSERT ALL + q-quote + quoted identifier + fake comment tokens inside strings
--------------------------------------------------------------------------------
INSERT ALL
    INTO qt_splitter_ultimate
    (
        id, parent_id, grp, "DATE", "COMMENT", name, status_cd, amount, calc_text, payload, json_text, updated_at
    )
    VALUES
    (
        1,
        NULL,
        10,
        SYSDATE,
        'alpha ; slash / dash -- star /* */',
        'Alpha',
        'NEW',
        100,
        '(1);(2)/(3)',
        q'[payload ; / -- '' " ]',
        q'!{"msg":"hello ; / -- /* */","items":[1,2,3]}!',
        SYSTIMESTAMP
    )
    INTO qt_splitter_ultimate
    (
        id, parent_id, grp, "DATE", "COMMENT", name, status_cd, amount, calc_text, payload, json_text, updated_at
    )
    VALUES
    (
        2,
        1,
        10,
        SYSDATE,
        q'[beta comment ; ; / / -- line marker]',
        'Beta',
        'HOLD',
        250,
        q'{CASE WHEN x THEN y; ELSE z; END / fake}',
        q'<xml><a>;</a><b>/</b><c>--</c></xml>',
        q'#{"nested":{"text":"q-quote ; and / and ''quote''"}}#',
        SYSTIMESTAMP
    )
    INTO qt_splitter_ultimate
    (
        id, parent_id, grp, "DATE", "COMMENT", name, status_cd, amount, calc_text, payload, json_text, updated_at
    )
    VALUES
    (
        3,
        1,
        20,
        SYSDATE,
        'gamma',
        'Gamma',
        'DONE',
        999.99,
        'plain text',
        'ordinary payload / not delimiter',
        '{"ok":true}',
        SYSTIMESTAMP
    )
SELECT 1 FROM dual;

--------------------------------------------------------------------------------
-- UNIT 019
--------------------------------------------------------------------------------
COMMIT;

--------------------------------------------------------------------------------
-- UNIT 020
-- 함수: 문자열과 q-quote, nested CASE, dynamic expression
--------------------------------------------------------------------------------
CREATE OR REPLACE FUNCTION qt_splitter_ultimate_func(p_id NUMBER)
RETURN VARCHAR2
IS
    v_result   VARCHAR2(4000);
    v_name     VARCHAR2(200);
    v_comment  VARCHAR2(4000);
BEGIN
    SELECT name, "COMMENT"
      INTO v_name, v_comment
      FROM qt_splitter_ultimate
     WHERE id = p_id;

    v_result :=
        CASE
            WHEN p_id IS NULL THEN q'[NULL-ID ; / ]'
            WHEN p_id < 0 THEN 'NEGATIVE;ID'
            ELSE
                'ID=' || p_id
                || ';NAME=' || REPLACE(v_name, ';', ':')
                || ';COMMENT=' || REPLACE(v_comment, '/', '|')
        END;

    RETURN v_result;
EXCEPTION
    WHEN NO_DATA_FOUND THEN
        RETURN 'NOT_FOUND;ID=' || p_id;
END;
/

--------------------------------------------------------------------------------
-- UNIT 021
-- 프로시저: dynamic SQL, EXECUTE IMMEDIATE, q-quote, nested block
--------------------------------------------------------------------------------
CREATE OR REPLACE PROCEDURE qt_splitter_ultimate_proc(p_grp NUMBER)
IS
    v_sql         CLOB;
    v_cnt         NUMBER;
    v_rendered    VARCHAR2(4000);
BEGIN
    v_sql := q'[
        SELECT COUNT(*)
          FROM qt_splitter_ultimate t
         WHERE t.grp = :x
           AND t."COMMENT" IS NOT NULL
           AND t."COMMENT" LIKE q'[%;%]'
    ]';

    EXECUTE IMMEDIATE v_sql INTO v_cnt USING p_grp;

    BEGIN
        SELECT qt_splitter_ultimate_func(MIN(id))
          INTO v_rendered
          FROM qt_splitter_ultimate
         WHERE grp = p_grp;
    EXCEPTION
        WHEN OTHERS THEN
            v_rendered := q'[fallback ; / ]';
    END;

    UPDATE qt_splitter_ultimate
       SET calc_text =
           SUBSTR(
               NVL(calc_text, '')
               || ' | PROC_CNT=' || v_cnt
               || ' | R=' || v_rendered
               || ' | TXT=' || q'[proc text ; / -- ]',
               1,
               4000
           ),
           updated_at = SYSTIMESTAMP
     WHERE grp = p_grp;
END qt_splitter_ultimate_proc;
/

--------------------------------------------------------------------------------
-- UNIT 022
-- 패키지 스펙
--------------------------------------------------------------------------------
CREATE OR REPLACE PACKAGE qt_splitter_ultimate_pkg
IS
    gc_new   CONSTANT VARCHAR2(30) := 'NEW';
    gc_done  CONSTANT VARCHAR2(30) := 'DONE';

    SUBTYPE t_status IS VARCHAR2(30);

    TYPE t_num_tab IS TABLE OF NUMBER INDEX BY PLS_INTEGER;
    TYPE t_rec IS RECORD
    (
        id          NUMBER,
        grp         NUMBER,
        name        VARCHAR2(200),
        status_cd   VARCHAR2(30),
        amount      NUMBER
    );

    FUNCTION normalize_name(p_name VARCHAR2) RETURN VARCHAR2;
    FUNCTION make_obj(p_id NUMBER, p_name VARCHAR2, p_note VARCHAR2)
        RETURN qt_splitter_force_obj;
    PROCEDURE touch_row(p_id NUMBER, p_note VARCHAR2);
    PROCEDURE merge_like(
        p_id       NUMBER,
        p_parent   NUMBER,
        p_grp      NUMBER,
        p_name     VARCHAR2,
        p_status   t_status,
        p_amount   NUMBER
    );
    PROCEDURE run_all(p_grp NUMBER);
END qt_splitter_ultimate_pkg;
/

--------------------------------------------------------------------------------
-- UNIT 023
-- 패키지 바디: 분리기 난이도 최상
--------------------------------------------------------------------------------
CREATE OR REPLACE PACKAGE BODY qt_splitter_ultimate_pkg
IS
    FUNCTION normalize_name(p_name VARCHAR2) RETURN VARCHAR2
    IS
    BEGIN
        RETURN UPPER(
            REPLACE(
                REPLACE(
                    TRIM(p_name),
                    ';',
                    ':'
                ),
                '/',
                '|'
            )
        );
    END normalize_name;

    FUNCTION make_obj(p_id NUMBER, p_name VARCHAR2, p_note VARCHAR2)
        RETURN qt_splitter_force_obj
    IS
    BEGIN
        RETURN qt_splitter_force_obj(
            p_id,
            normalize_name(p_name),
            SUBSTR(NVL(p_note, '') || q'[ ; / object-note ]', 1, 4000)
        );
    END make_obj;

    PROCEDURE touch_row(p_id NUMBER, p_note VARCHAR2)
    IS
        v_dyn CLOB;
    BEGIN
        v_dyn := q'[
            UPDATE qt_splitter_ultimate
               SET "COMMENT" =
                   SUBSTR(
                       NVL("COMMENT", '')
                       || ' | '
                       || :x
                       || ' | '
                       || q'[dyn ; / -- '' ]',
                       1,
                       4000
                   ),
                   updated_at = SYSTIMESTAMP
             WHERE id = :y
        ]';

        EXECUTE IMMEDIATE v_dyn USING p_note, p_id;
    END touch_row;

    PROCEDURE merge_like(
        p_id       NUMBER,
        p_parent   NUMBER,
        p_grp      NUMBER,
        p_name     VARCHAR2,
        p_status   t_status,
        p_amount   NUMBER
    )
    IS
        v_exists NUMBER;
    BEGIN
        SELECT COUNT(*)
          INTO v_exists
          FROM qt_splitter_ultimate
         WHERE id = p_id;

        IF v_exists = 0 THEN
            INSERT INTO qt_splitter_ultimate
            (
                id, parent_id, grp, "DATE", "COMMENT", name, status_cd, amount, calc_text, payload, json_text, updated_at
            )
            VALUES
            (
                p_id,
                p_parent,
                p_grp,
                SYSDATE,
                q'[inserted ; / by package]',
                normalize_name(p_name),
                p_status,
                p_amount,
                'calc;' || p_id || '/',
                q'[payload from merge_like ; / ]',
                q'!{"source":"merge_like","txt":"; / --"}!',
                SYSTIMESTAMP
            );
        ELSE
            UPDATE qt_splitter_ultimate
               SET parent_id  = p_parent,
                   grp        = p_grp,
                   name       = normalize_name(p_name),
                   status_cd  = p_status,
                   amount     = p_amount,
                   calc_text  = SUBSTR(NVL(calc_text, '') || ' | updated;package/', 1, 4000),
                   updated_at = SYSTIMESTAMP
             WHERE id = p_id;
        END IF;

        BEGIN
            IF p_amount >= 5000 THEN
                touch_row(p_id, 'AMOUNT>=5000;VIP-CANDIDATE');
            ELSIF p_amount >= 1000 THEN
                touch_row(p_id, 'AMOUNT>=1000;DONE-CANDIDATE');
            ELSE
                touch_row(p_id, 'AMOUNT<1000;NORMAL');
            END IF;
        EXCEPTION
            WHEN OTHERS THEN
                NULL;
        END;
    END merge_like;

    PROCEDURE run_all(p_grp NUMBER)
    IS
        v_tab       t_num_tab;
        v_idx       PLS_INTEGER := 0;
        v_obj       qt_splitter_force_obj;
    BEGIN
        FOR r IN
        (
            SELECT id, name, "COMMENT"
              FROM qt_splitter_ultimate
             WHERE grp = p_grp
             ORDER BY id
        )
        LOOP
            v_idx := v_idx + 1;
            v_tab(v_idx) := r.id;

            v_obj := make_obj(r.id, r.name, r."COMMENT");

            UPDATE qt_splitter_ultimate
               SET payload =
                   SUBSTR(
                       NVL(payload, '')
                       || ' | OBJ=' || v_obj.render()
                       || ' | IDX=' || v_idx
                       || ' | LOOPTXT=' || q'[loop ; / text]',
                       1,
                       4000
                   ),
                   updated_at = SYSTIMESTAMP
             WHERE id = r.id;
        END LOOP;

        IF v_idx > 0 THEN
            FOR i IN 1 .. v_idx
            LOOP
                UPDATE qt_splitter_ultimate
                   SET calc_text =
                       SUBSTR(
                           NVL(calc_text, '')
                           || ' | TAB(' || i || ')=' || v_tab(i),
                           1,
                           4000
                       )
                 WHERE id = v_tab(i);
            END LOOP;
        END IF;
    END run_all;
END qt_splitter_ultimate_pkg;
/

--------------------------------------------------------------------------------
-- UNIT 024
-- 트리거
--------------------------------------------------------------------------------
CREATE OR REPLACE TRIGGER qt_splitter_ultimate_trg
BEFORE INSERT OR UPDATE
ON qt_splitter_ultimate
FOR EACH ROW
DECLARE
    v_msg VARCHAR2(4000);
BEGIN
    IF :NEW.name IS NOT NULL THEN
        :NEW.name := qt_splitter_ultimate_pkg.normalize_name(:NEW.name);
    END IF;

    v_msg :=
        CASE
            WHEN INSERTING THEN 'TRG-INSERT;ID=' || :NEW.id
            WHEN UPDATING THEN 'TRG-UPDATE;ID=' || :NEW.id
            ELSE 'TRG-OTHER'
        END;

    :NEW."COMMENT" :=
        SUBSTR(
            NVL(:NEW."COMMENT", '')
            || ' | '
            || v_msg
            || ' | '
            || q'[trigger text ; / -- ]',
            1,
            4000
        );

    :NEW.updated_at := SYSTIMESTAMP;
END;
/

--------------------------------------------------------------------------------
-- UNIT 025
-- 익명 블록 1
--------------------------------------------------------------------------------
DECLARE
    v_sql   CLOB;
BEGIN
    v_sql := q'[
        INSERT INTO qt_splitter_ultimate
        (
            id, parent_id, grp, "DATE", "COMMENT", name, status_cd, amount, calc_text, payload, json_text, updated_at
        )
        VALUES
        (
            :1,
            :2,
            :3,
            SYSDATE,
            q'[anon insert ; / -- ]',
            :4,
            :5,
            :6,
            q'[calc ; / ]',
            q'[payload ; / ]',
            q'!{"anon":"yes","txt":"; /"}!',
            SYSTIMESTAMP
        )
    ]';

    EXECUTE IMMEDIATE v_sql
        USING 10, 3, 30, 'delta/name', 'NEW', 444;

    qt_splitter_ultimate_pkg.merge_like(
        p_id     => 11,
        p_parent => 10,
        p_grp    => 30,
        p_name   => 'epsilon;slash/name',
        p_status => 'DONE',
        p_amount => 1234
    );

    qt_splitter_ultimate_proc(10);
END;
/

--------------------------------------------------------------------------------
-- UNIT 026
-- 익명 블록 2: collection/object 사용
--------------------------------------------------------------------------------
DECLARE
    v_list   qt_splitter_force_tab := qt_splitter_force_tab();
BEGIN
    v_list.EXTEND(2);

    v_list(1) := qt_splitter_force_obj(100, 'objA', q'[noteA ; / ]');
    v_list(2) := qt_splitter_force_obj(101, 'objB', q'[noteB ; / ]');

    FOR i IN 1 .. v_list.COUNT
    LOOP
        INSERT INTO qt_splitter_ultimate
        (
            id, parent_id, grp, "DATE", "COMMENT", name, status_cd, amount, calc_text, payload, json_text, updated_at
        )
        VALUES
        (
            v_list(i).id,
            NULL,
            90,
            SYSDATE,
            v_list(i).render(),
            v_list(i).name,
            'HOLD',
            i * 10,
            'from object collection',
            q'[collection payload ; / ]',
            '{"collection":true}',
            SYSTIMESTAMP
        );
    END LOOP;
END;
/

--------------------------------------------------------------------------------
-- UNIT 027
-- 복잡 SELECT + WITH + analytic + scalar subquery + XML 스타일 문자열
--------------------------------------------------------------------------------
WITH base_data AS
(
    SELECT
        t.id,
        t.parent_id,
        t.grp,
        t.name,
        t.status_cd,
        t.amount,
        ROW_NUMBER() OVER (PARTITION BY t.grp ORDER BY t.amount DESC NULLS LAST, t.id) AS rn,
        AVG(t.amount) OVER (PARTITION BY t.grp) AS avg_amt,
        SUM(t.amount) OVER (PARTITION BY t.grp) AS sum_amt
    FROM qt_splitter_ultimate t
),
flagged AS
(
    SELECT
        b.*,
        CASE
            WHEN b.amount > b.avg_amt THEN 'ABOVE'
            WHEN b.amount < b.avg_amt THEN 'BELOW'
            ELSE 'EQUAL'
        END AS pos_flag
    FROM base_data b
)
SELECT
    f.id,
    f.parent_id,
    f.grp,
    f.name,
    f.status_cd,
    f.amount,
    f.rn,
    f.avg_amt,
    f.sum_amt,
    f.pos_flag,
    (
        SELECT COUNT(*)
          FROM qt_splitter_ultimate x
         WHERE x.grp = f.grp
           AND x.amount >= f.amount
    ) AS ge_count,
    '<tag>' || f.name || ';</tag>' AS xmlish_text
FROM flagged f
WHERE EXISTS
(
    SELECT 1
      FROM qt_splitter_ultimate e
     WHERE e.grp = f.grp
       AND e.id <> f.id
)
ORDER BY f.grp, f.rn, f.id;

--------------------------------------------------------------------------------
-- UNIT 028
-- CONNECT BY
--------------------------------------------------------------------------------
SELECT
    LEVEL AS lvl,
    SYS_CONNECT_BY_PATH(name, ' / ') AS path_txt
FROM qt_splitter_ultimate
START WITH parent_id IS NULL
CONNECT BY PRIOR id = parent_id
ORDER SIBLINGS BY id;

--------------------------------------------------------------------------------
-- UNIT 029
-- MERGE
--------------------------------------------------------------------------------
MERGE INTO qt_splitter_ultimate d
USING
(
    SELECT 2 AS id, 1 AS parent_id, 10 AS grp, 'beta-merged' AS name, 'DONE' AS status_cd, 888 AS amount FROM dual
    UNION ALL
    SELECT 12 AS id, 11 AS parent_id, 30 AS grp, 'zeta' AS name, 'NEW' AS status_cd, 12 AS amount FROM dual
    UNION ALL
    SELECT 13 AS id, NULL AS parent_id, 40 AS grp, 'eta' AS name, 'VIP' AS status_cd, 7777 AS amount FROM dual
) s
ON (d.id = s.id)
WHEN MATCHED THEN
    UPDATE
       SET d.parent_id  = s.parent_id,
           d.grp        = s.grp,
           d.name       = s.name,
           d.status_cd  = s.status_cd,
           d.amount     = s.amount,
           d."COMMENT"  = SUBSTR(NVL(d."COMMENT", '') || ' | MERGE-MATCH ; / ', 1, 4000),
           d.updated_at = SYSTIMESTAMP
WHEN NOT MATCHED THEN
    INSERT
    (
        id, parent_id, grp, "DATE", "COMMENT", name, status_cd, amount, calc_text, payload, json_text, updated_at
    )
    VALUES
    (
        s.id, s.parent_id, s.grp, SYSDATE,
        'MERGE-INSERT ; / ',
        s.name, s.status_cd, s.amount,
        'merge calc',
        'merge payload',
        '{"merge":true}',
        SYSTIMESTAMP
    );

--------------------------------------------------------------------------------
-- UNIT 030
-- UPDATE with CASE, subquery
--------------------------------------------------------------------------------
UPDATE qt_splitter_ultimate t
   SET t.status_cd =
       CASE
           WHEN t.amount >= 5000 THEN 'VIP'
           WHEN t.amount >= 1000 THEN 'DONE'
           WHEN t.amount IS NULL THEN 'ERR'
           ELSE 'NEW'
       END,
       t.calc_text =
       SUBSTR(
           NVL(t.calc_text, '')
           || ' | AVG='
           || (
                SELECT TO_CHAR(AVG(x.amount))
                  FROM qt_splitter_ultimate x
                 WHERE x.grp = t.grp
              )
           || ' | MAX='
           || (
                SELECT TO_CHAR(MAX(y.amount))
                  FROM qt_splitter_ultimate y
                 WHERE y.grp = t.grp
              ),
           1,
           4000
       ),
       t.updated_at = SYSTIMESTAMP
 WHERE t.id IN
 (
     SELECT id
       FROM qt_splitter_ultimate
      WHERE grp IN (10, 20, 30, 40, 90)
 );

--------------------------------------------------------------------------------
-- UNIT 031
-- DELETE
--------------------------------------------------------------------------------
DELETE FROM qt_splitter_ultimate
 WHERE id IN
 (
     SELECT z.id
       FROM qt_splitter_ultimate z
      WHERE z.status_cd = 'NEW'
        AND z.amount < 50
 );

--------------------------------------------------------------------------------
-- UNIT 032
-- 뷰
--------------------------------------------------------------------------------
CREATE OR REPLACE VIEW qt_splitter_ultimate_v
AS
SELECT
    t.id,
    t.parent_id,
    t.grp,
    t."DATE",
    t."COMMENT",
    t.name,
    t.status_cd,
    t.amount,
    CASE
        WHEN t.status_cd = 'VIP' THEN 'TOP;TIER'
        WHEN t.status_cd = 'DONE' THEN 'PROCESSED/OK'
        WHEN t.status_cd = 'NEW' THEN 'FRESH'
        ELSE 'OTHER'
    END AS status_desc,
    '(' || t.id || ');(' || t.grp || ')/(' || NVL(t.parent_id, -1) || ')' AS weird_text
FROM qt_splitter_ultimate t;

--------------------------------------------------------------------------------
-- UNIT 033
-- COMMENT ON
--------------------------------------------------------------------------------
COMMENT ON TABLE qt_splitter_ultimate IS 'ultimate splitter boss ; / table comment';

--------------------------------------------------------------------------------
-- UNIT 034
--------------------------------------------------------------------------------
COMMENT ON COLUMN qt_splitter_ultimate."COMMENT" IS 'column comment ; / tricky';

--------------------------------------------------------------------------------
-- UNIT 035
-- 익명 블록 3: slash는 문자열/주석/나눗셈에서 delimiter가 아니어야 함
--------------------------------------------------------------------------------
DECLARE
    v_a NUMBER := 10;
    v_b NUMBER := 2;
    v_c NUMBER;
    v_t VARCHAR2(4000);
BEGIN
    v_c := v_a / v_b;
    v_t := 'division=' || v_c || '; slash=/; not delimiter';
    v_t := v_t || q'[ ; text / comment-like -- /* */ ]';

    UPDATE qt_splitter_ultimate
       SET payload = SUBSTR(NVL(payload, '') || ' | ' || v_t, 1, 4000)
     WHERE id = 1;

    /* multi-line comment with fake terminators
       ; ; ; / / / -- not real
    */

    NULL;
END;
/

--------------------------------------------------------------------------------
-- UNIT 036
-- BEGIN ... END; 다음 줄 slash 가 진짜 경계인지 확인
--------------------------------------------------------------------------------
BEGIN
    qt_splitter_ultimate_pkg.run_all(10);
    qt_splitter_ultimate_pkg.run_all(30);
    qt_splitter_ultimate_pkg.run_all(90);
END;
/

--------------------------------------------------------------------------------
-- UNIT 037
--------------------------------------------------------------------------------
SELECT
    t.id,
    t.grp,
    t.name,
    t.status_cd,
    t.amount,
    t.calc_text,
    t.payload
FROM qt_splitter_ultimate t
ORDER BY t.id;

--------------------------------------------------------------------------------
-- UNIT 038
--------------------------------------------------------------------------------
COMMIT;

--------------------------------------------------------------------------------
-- UNIT 039
--------------------------------------------------------------------------------
PROMPT === QT SPLITTER FINAL ULTIMATE BOSS END ===
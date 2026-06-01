--------------------------------------------------------------------------------
-- UNIT 01
-- 세션 설정. 주석 안의 ; 와 / 는 절대 분리 기준이 되면 안 된다 ; / ;
--------------------------------------------------------------------------------
ALTER SESSION SET NLS_DATE_FORMAT = 'YYYY-MM-DD HH24:MI:SS';

--------------------------------------------------------------------------------
-- UNIT 02
--------------------------------------------------------------------------------

drop table qt_splitter_boss;

CREATE TABLE qt_splitter_boss
(
    id           NUMBER          PRIMARY KEY,
    grp          NUMBER          NOT NULL,
    name         VARCHAR2(200),
    note_text    VARCHAR2(4000),
    created_at   DATE            DEFAULT SYSDATE,
    amount       NUMBER(18,2),
    status_cd    VARCHAR2(30),
    payload      CLOB
);

--------------------------------------------------------------------------------
-- UNIT 03
-- 문자열 내부 세미콜론 ;;; 및 slash / 포함
--------------------------------------------------------------------------------
INSERT ALL
    INTO qt_splitter_boss (id, grp, name, note_text, amount, status_cd, payload)
    VALUES (1, 10, 'ALPHA', 'semi;colon;inside;text', 100, 'NEW', 'payload / not delimiter')
    INTO qt_splitter_boss (id, grp, name, note_text, amount, status_cd, payload)
    VALUES (2, 10, 'BETA', 'text with -- fake comment ; and slash /', 200, 'NEW', 'q1')
    INTO qt_splitter_boss (id, grp, name, note_text, amount, status_cd, payload)
    VALUES (3, 20, 'GAMMA', q'[q-quote ; inside / and 'single quotes' and -- text]', 300, 'HOLD', 'q2')
SELECT 1 FROM dual;

--------------------------------------------------------------------------------
-- UNIT 04
--------------------------------------------------------------------------------
COMMIT;

--------------------------------------------------------------------------------
-- UNIT 05
-- 패키지 스펙: 내부 ; 많지만 실행단위 끝은 "/" 이어야 함
--------------------------------------------------------------------------------
CREATE OR REPLACE PACKAGE qt_splitter_pkg
IS
    gc_status_new   CONSTANT VARCHAR2(30) := 'NEW';
    gc_status_done  CONSTANT VARCHAR2(30) := 'DONE';

    TYPE t_num_tab IS TABLE OF NUMBER INDEX BY PLS_INTEGER;

    FUNCTION normalize_name(p_name VARCHAR2) RETURN VARCHAR2;
    PROCEDURE log_row(p_id NUMBER, p_msg VARCHAR2);
    PROCEDURE upsert_row(
        p_id       NUMBER,
        p_grp      NUMBER,
        p_name     VARCHAR2,
        p_note     VARCHAR2,
        p_amount   NUMBER,
        p_status   VARCHAR2
    );
END qt_splitter_pkg;
/

--------------------------------------------------------------------------------
-- UNIT 06
-- 패키지 바디:
--   - 주석 내부 ; / 혼합
--   - q-quote
--   - 동적 SQL
--   - nested begin/end
-- 단위 종료는 마지막 END; 다음 줄의 "/" 이다
--------------------------------------------------------------------------------
CREATE OR REPLACE PACKAGE BODY qt_splitter_pkg
IS
    FUNCTION normalize_name(p_name VARCHAR2) RETURN VARCHAR2
    IS
    BEGIN
        RETURN UPPER(REPLACE(TRIM(p_name), ';', ':'));
    END normalize_name;

    PROCEDURE log_row(p_id NUMBER, p_msg VARCHAR2)
    IS
        v_dummy VARCHAR2(4000);
    BEGIN
        v_dummy := q'[log text ; / -- '' ]';
        UPDATE qt_splitter_boss
           SET note_text = SUBSTR(NVL(note_text, '') || ' | ' || p_msg || ' | ' || v_dummy, 1, 4000)
         WHERE id = p_id;
    END log_row;

    PROCEDURE upsert_row(
        p_id       NUMBER,
        p_grp      NUMBER,
        p_name     VARCHAR2,
        p_note     VARCHAR2,
        p_amount   NUMBER,
        p_status   VARCHAR2
    )
    IS
        v_count NUMBER;
        v_sql   CLOB;
    BEGIN
        SELECT COUNT(*)
          INTO v_count
          FROM qt_splitter_boss
         WHERE id = p_id;

        IF v_count = 0 THEN
            INSERT INTO qt_splitter_boss
            (
                id, grp, name, note_text, created_at, amount, status_cd, payload
            )
            VALUES
            (
                p_id,
                p_grp,
                normalize_name(p_name),
                p_note,
                SYSDATE,
                p_amount,
                p_status,
                q'[inserted;payload/with tricky tokens]'
            );
        ELSE
            v_sql := q'[
                UPDATE qt_splitter_boss
                   SET grp = :1,
                       name = :2,
                       note_text = :3,
                       amount = :4,
                       status_cd = :5,
                       payload = q'[dynamic ; payload / still string]'
                 WHERE id = :6
            ]';

            EXECUTE IMMEDIATE v_sql
                USING p_grp, normalize_name(p_name), p_note, p_amount, p_status, p_id;
        END IF;

        BEGIN
            IF p_amount > 999 THEN
                log_row(p_id, 'amount>999; flagged');
            ELSE
                log_row(p_id, 'amount<=999; ok');
            END IF;
        EXCEPTION
            WHEN OTHERS THEN
                NULL;
        END;
    END upsert_row;
END qt_splitter_pkg;
/

--------------------------------------------------------------------------------
-- UNIT 07
-- 트리거: 내부 세미콜론 다수. 종료는 "/" 이어야 함
--------------------------------------------------------------------------------
CREATE OR REPLACE TRIGGER qt_splitter_biu
BEFORE INSERT OR UPDATE
ON qt_splitter_boss
FOR EACH ROW
DECLARE
    v_msg VARCHAR2(200);
BEGIN
    IF :NEW.name IS NOT NULL THEN
        :NEW.name := qt_splitter_pkg.normalize_name(:NEW.name);
    END IF;

    IF INSERTING THEN
        v_msg := 'before insert; id=' || :NEW.id;
    ELSIF UPDATING THEN
        v_msg := 'before update; id=' || :NEW.id;
    END IF;

    :NEW.note_text := SUBSTR(NVL(:NEW.note_text, '') || ' | ' || v_msg, 1, 4000);
END;
/

--------------------------------------------------------------------------------
-- UNIT 08
-- 익명 블록 + q-quote + 동적 SQL + 문자열 내 ; / --
-- 실행단위 끝은 END; 뒤 "/" 이다
--------------------------------------------------------------------------------
DECLARE
    v_sql   CLOB;
    v_id    NUMBER := 4;
BEGIN
    v_sql := q'[
        INSERT INTO qt_splitter_boss
        (
            id, grp, name, note_text, created_at, amount, status_cd, payload
        )
        VALUES
        (
            :1,
            30,
            'delta',
            q'[anonymous block text ; / -- not split]',
            SYSDATE,
            444,
            'NEW',
            q'[payload with ; and / and ''quotes'']'
        )
    ]';

    EXECUTE IMMEDIATE v_sql USING v_id;

    qt_splitter_pkg.upsert_row(
        p_id     => 2,
        p_grp    => 11,
        p_name   => 'beta;revised',
        p_note   => q'[updated in anon block ; / ]',
        p_amount => 1200,
        p_status => 'DONE'
    );
END;
/

--------------------------------------------------------------------------------
-- UNIT 09
-- 복잡 SELECT + CTE + analytic + scalar subquery + CASE
--------------------------------------------------------------------------------
WITH base_data AS
(
    SELECT
        t.id,
        t.grp,
        t.name,
        t.amount,
        t.status_cd,
        ROW_NUMBER() OVER (PARTITION BY t.grp ORDER BY t.amount DESC, t.id) AS rn,
        AVG(t.amount) OVER (PARTITION BY t.grp) AS avg_amt
    FROM qt_splitter_boss t
),
ranked_data AS
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
    r.id,
    r.grp,
    r.name,
    r.amount,
    r.status_cd,
    r.rn,
    r.avg_amt,
    r.pos_flag,
    (
        SELECT COUNT(*)
          FROM qt_splitter_boss x
         WHERE x.grp = r.grp
           AND x.amount >= r.amount
    ) AS ge_count
FROM ranked_data r
WHERE EXISTS
(
    SELECT 1
      FROM qt_splitter_boss e
     WHERE e.grp = r.grp
       AND e.id <> r.id
)
ORDER BY r.grp, r.rn, r.id;

--------------------------------------------------------------------------------
-- UNIT 10
-- MERGE
--------------------------------------------------------------------------------
MERGE INTO qt_splitter_boss d
USING
(
    SELECT 3 AS id, 99 AS grp, 'gamma-merged' AS name, 3333 AS amount, 'DONE' AS status_cd FROM dual
    UNION ALL
    SELECT 5 AS id, 50 AS grp, 'epsilon'      AS name,  555 AS amount, 'NEW'  AS status_cd FROM dual
) s
ON (d.id = s.id)
WHEN MATCHED THEN
    UPDATE
       SET d.grp       = s.grp,
           d.name      = s.name,
           d.amount    = s.amount,
           d.status_cd = s.status_cd,
           d.note_text = SUBSTR(NVL(d.note_text, '') || ' | merged;updated', 1, 4000)
WHEN NOT MATCHED THEN
    INSERT
    (
        id, grp, name, note_text, created_at, amount, status_cd, payload
    )
    VALUES
    (
        s.id, s.grp, s.name, 'merged;inserted', SYSDATE, s.amount, s.status_cd, 'merge payload'
    );

--------------------------------------------------------------------------------
-- UNIT 11
-- UPDATE 안의 CASE / scalar subquery
--------------------------------------------------------------------------------
UPDATE qt_splitter_boss t
   SET t.status_cd =
       CASE
           WHEN t.amount >= 3000 THEN 'VIP'
           WHEN t.amount >= 1000 THEN 'DONE'
           ELSE 'NEW'
       END,
       t.note_text =
       SUBSTR(
           NVL(t.note_text, '')
           || ' | grp_avg='
           || (
                SELECT TO_CHAR(AVG(x.amount))
                  FROM qt_splitter_boss x
                 WHERE x.grp = t.grp
              ),
           1,
           4000
       )
 WHERE t.id IN
 (
     SELECT id
       FROM qt_splitter_boss
      WHERE grp IN (10, 11, 20, 30, 50, 99)
 );

--------------------------------------------------------------------------------
-- UNIT 12
--------------------------------------------------------------------------------
DELETE FROM qt_splitter_boss
 WHERE id IN
 (
     SELECT z.id
       FROM qt_splitter_boss z
      WHERE z.status_cd = 'NEW'
        AND z.amount < 150
 );

--------------------------------------------------------------------------------
-- UNIT 13
-- CREATE VIEW. 문자열 속 ; 존재
--------------------------------------------------------------------------------
CREATE OR REPLACE VIEW qt_splitter_boss_v
AS
SELECT
    t.id,
    t.grp,
    t.name,
    t.amount,
    t.status_cd,
    CASE
        WHEN t.status_cd = 'VIP'  THEN 'top;tier'
        WHEN t.status_cd = 'DONE' THEN 'processed'
        ELSE 'other'
    END AS status_desc,
    '(' || t.id || ');(' || t.grp || ')' AS weird_text
FROM qt_splitter_boss t;

--------------------------------------------------------------------------------
-- UNIT 14
-- 프로시저 + 내부 동적 SQL + q-quote
-- 끝은 "/" 이다
--------------------------------------------------------------------------------
CREATE OR REPLACE PROCEDURE qt_splitter_proc(p_grp NUMBER)
IS
    v_sql   CLOB;
    v_cnt   NUMBER;
BEGIN
    v_sql := q'[
        SELECT COUNT(*)
          FROM qt_splitter_boss
         WHERE grp = :x
           AND note_text LIKE q'[%;%]'
    ]';

    EXECUTE IMMEDIATE v_sql INTO v_cnt USING p_grp;

    UPDATE qt_splitter_boss
       SET note_text = SUBSTR(NVL(note_text, '') || ' | proc_cnt=' || v_cnt, 1, 4000)
     WHERE grp = p_grp;

    BEGIN
        NULL; -- ; / inside comment should not matter ;
    END;
END qt_splitter_proc;
/

--------------------------------------------------------------------------------
-- UNIT 15
-- 또 다른 익명 블록
--------------------------------------------------------------------------------
BEGIN
    qt_splitter_proc(10);
    qt_splitter_proc(11);
    qt_splitter_proc(99);

    FOR r IN
    (
        SELECT id, grp
          FROM qt_splitter_boss
         ORDER BY id
    )
    LOOP
        UPDATE qt_splitter_boss
           SET payload =
               q'[loop payload ; / ]'
               || ' grp=' || r.grp
               || ' id=' || r.id
         WHERE id = r.id;
    END LOOP;
END;
/

--------------------------------------------------------------------------------
-- UNIT 16
-- 복합 SELECT: inline view + EXISTS + CONNECT BY
--------------------------------------------------------------------------------
SELECT
    a.id,
    a.grp,
    a.name,
    a.status_cd,
    lvl.lvl_no
FROM
    (
        SELECT t.*
          FROM qt_splitter_boss t
         WHERE EXISTS
               (
                   SELECT 1
                     FROM qt_splitter_boss x
                    WHERE x.grp = t.grp
                      AND x.id <> t.id
               )
    ) a
    CROSS JOIN
    (
        SELECT LEVEL AS lvl_no
          FROM dual
        CONNECT BY LEVEL <= 2
    ) lvl
WHERE a.id IS NOT NULL
ORDER BY a.id, lvl.lvl_no;

--------------------------------------------------------------------------------
-- UNIT 17
--------------------------------------------------------------------------------
COMMENT ON TABLE qt_splitter_boss IS 'splitter final boss ; comment / text';

--------------------------------------------------------------------------------
-- UNIT 18
--------------------------------------------------------------------------------
CREATE INDEX qt_splitter_boss_ix1 ON qt_splitter_boss (grp, status_cd, amount);

--------------------------------------------------------------------------------
-- UNIT 19
--------------------------------------------------------------------------------
COMMIT;
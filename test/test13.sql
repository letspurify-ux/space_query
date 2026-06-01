SET SERVEROUTPUT ON

SET DEFINE OFF

--------------------------------------------------------------------------------
-- CLEANUP
--------------------------------------------------------------------------------

BEGIN
    EXECUTE IMMEDIATE 'DROP VIEW qt_kw_mix_v';
EXCEPTION
    WHEN OTHERS THEN
        NULL;
END;
/

BEGIN
    EXECUTE IMMEDIATE 'DROP TABLE qt_kw_child PURGE';
EXCEPTION
    WHEN OTHERS THEN
        NULL;
END;
/

BEGIN
    EXECUTE IMMEDIATE 'DROP TABLE qt_kw_base PURGE';
EXCEPTION
    WHEN OTHERS THEN
        NULL;
END;
/

--------------------------------------------------------------------------------
-- SETUP
--------------------------------------------------------------------------------

CREATE TABLE qt_kw_base (
    a        NUMBER        NOT NULL,
    b        VARCHAR2(100),
    c        NUMBER,
    grp      NUMBER,
    flag     VARCHAR2(1),
    dt       DATE,
    category VARCHAR2(30),
    subcat   VARCHAR2(30),
    CONSTRAINT qt_kw_base_pk PRIMARY KEY(a)
);
/

CREATE TABLE qt_kw_child (
    child_id NUMBER        NOT NULL,
    ref_a    NUMBER        NOT NULL,
    seq_no   NUMBER        NOT NULL,
    metric   NUMBER,
    note_txt VARCHAR2(100),
    kind     VARCHAR2(20),
    CONSTRAINT qt_kw_child_pk PRIMARY KEY(child_id),
    CONSTRAINT qt_kw_child_fk FOREIGN KEY(ref_a) REFERENCES qt_kw_base(a)
);
/

INSERT ALL
INTO qt_kw_base
VALUES (1, 'alpha', 10, 1, 'Y', DATE '2024-01-01', 'A', 'A1')
INTO qt_kw_base
VALUES (2, 'beta', 20, 1, 'N', DATE '2024-01-02', 'A', 'A2')
INTO qt_kw_base
VALUES (3, 'gamma', 30, 2, 'Y', DATE '2024-01-03', 'B', 'B1')
INTO qt_kw_base
VALUES (4, 'delta', 40, 2, 'N', DATE '2024-01-04', 'B', 'B2')
INTO qt_kw_base
VALUES (5, 'epsilon', 50, 3, 'Y', DATE '2024-01-05', 'C', 'C1')
INTO qt_kw_base
VALUES (6, 'zeta', 60, 3, 'N', DATE '2024-01-06', 'C', 'C2')
SELECT 1
FROM DUAL;
/

INSERT ALL
INTO qt_kw_child
VALUES (101, 1, 1, 100, 'n1', 'X')
INTO qt_kw_child
VALUES (102, 1, 2, 150, 'n2', 'Y')
INTO qt_kw_child
VALUES (103, 2, 1, 200, 'n3', 'X')
INTO qt_kw_child
VALUES (104, 3, 1, 300, 'n4', 'Y')
INTO qt_kw_child
VALUES (105, 3, 2, 350, 'n5', 'Z')
INTO qt_kw_child
VALUES (106, 4, 1, 400, 'n6', 'X')
INTO qt_kw_child
VALUES (107, 5, 1, 500, 'n7', 'Y')
INTO qt_kw_child
VALUES (108, 6, 1, 600, 'n8', 'Z')
SELECT 1
FROM DUAL;
/

COMMIT;
/

--------------------------------------------------------------------------------
-- 1. BASIC TABLE ALIAS CASES
--------------------------------------------------------------------------------

SELECT IF.a,
    IF.b,
    IF.c,
    IF.grp,
    IF.flag,
    IF.dt
FROM qt_kw_base IF
ORDER BY IF.a;
/
/

SELECT RANK.a,
    RANK.b,
    RANK.c,
    RANK.subcat
FROM qt_kw_base RANK
ORDER BY RANK.a;
/

SELECT COUNT.a,
    COUNT.b,
    COUNT.flag,
    COUNT.dt
FROM qt_kw_base COUNT
ORDER BY COUNT.a;
/

SELECT trim.a,
    trim.b,
    trim.c,
    trim.grp
FROM qt_kw_base trim
ORDER BY trim.a;
/

--------------------------------------------------------------------------------
-- 2. EXPRESSIONS + QUALIFIED IDENTIFIER
--------------------------------------------------------------------------------

SELECT IF.a,
    UPPER (IF.b) AS upper_b,
    IF.c * 10 AS c_x_10,
    CASE
        WHEN IF.flag = 'Y' THEN 'YES'
        ELSE 'NO'
    END AS flag_text
FROM qt_kw_base IF
ORDER BY IF.a;
/

SELECT trim.a,
    TRIM (trim.b) AS b_trimmed,
    LENGTH (trim.b) AS b_len
FROM qt_kw_base trim
ORDER BY trim.a;
/

--------------------------------------------------------------------------------
-- 3. JOINS WITH MIXED KEYWORD-LIKE ALIASES
--------------------------------------------------------------------------------

SELECT IF.a,
    IF.b,
    COUNT.child_id,
    COUNT.metric
FROM qt_kw_base IF
JOIN qt_kw_child COUNT
    ON COUNT.ref_a = IF.a
ORDER BY IF.a,
    COUNT.child_id;
/

--------------------------------------------------------------------------------
-- 4. INLINE VIEW ALIAS
--------------------------------------------------------------------------------

SELECT IF.a,
    IF.b,
    IF.c
FROM (
        SELECT a,
            b,
            c
        FROM qt_kw_base
        WHERE flag = 'Y'
    ) IF
ORDER BY IF.a;
/

SELECT RANK.a,
    RANK.max_metric,
    RANK.cnt_metric
FROM (
        SELECT trim.ref_a AS a,
            MAX (trim.metric) AS max_metric,
            COUNT (*) AS cnt_metric
        FROM qt_kw_child trim
        GROUP BY trim.ref_a
    ) RANK
ORDER BY RANK.a;
/

--------------------------------------------------------------------------------
-- 5. CTE NAME = if / level / date / rank / count / trim
--------------------------------------------------------------------------------

WITH
IF AS (
        SELECT
            a,
            b,
            c,
            grp,
            flag
        FROM qt_kw_base
    )
    SELECT
        IF.a,
        IF.b,
        IF.c,
        IF.grp,
        IF.flag
    FROM IF
    ORDER BY IF.a;
/
/

WITH RANK AS (
    SELECT
        a,
        grp,
        DENSE_RANK () OVER (ORDER BY c DESC) AS dr
    FROM qt_kw_base
)
SELECT
    RANK.a,
    RANK.grp,
    RANK.dr
FROM RANK
ORDER BY RANK.a;
/

WITH COUNT AS (
    SELECT
        a,
        b,
        c
    FROM qt_kw_base
    WHERE grp IN (1, 3)
)
SELECT
    COUNT.a,
    COUNT.b,
    COUNT.c
FROM COUNT
ORDER BY COUNT.a;
/

WITH trim AS (
    SELECT
        a,
        TRIM (b) AS b_trimmed,
        c
    FROM qt_kw_base
)
SELECT
    trim.a,
    trim.b_trimmed,
    trim.c
FROM trim
ORDER BY trim.a;
/

--------------------------------------------------------------------------------
-- 6. EXISTS / IN / SCALAR SUBQUERY / CORRELATED SUBQUERY
--------------------------------------------------------------------------------

SELECT IF.a,
    IF.b
FROM qt_kw_base IF
WHERE EXISTS (
        SELECT 1
        FROM qt_kw_child COUNT
        WHERE COUNT.ref_a = IF.a
    )
ORDER BY IF.a;
/

SELECT COUNT.a,
    COUNT.b,
    (
        SELECT COUNT (*)
        FROM qt_kw_child trim
        WHERE trim.ref_a = COUNT.a
    ) AS child_cnt
FROM qt_kw_base COUNT
ORDER BY COUNT.a;
/

SELECT trim.a,
    trim.b,
    CASE
        WHEN EXISTS (
            SELECT 1
            FROM qt_kw_child IF
            WHERE IF.ref_a = trim.a
                AND IF.metric >= 350
        ) THEN 'HAS_BIG'
        ELSE 'SMALL_ONLY'
    END AS metric_class
FROM qt_kw_base trim
ORDER BY trim.a;
/

--------------------------------------------------------------------------------
-- 7. MULTI-SCOPE / NESTED ALIAS REUSE
--------------------------------------------------------------------------------

SELECT IF.a,
    IF.b
FROM (
        SELECT IF.a,
            IF.b
        FROM qt_kw_base IF
        WHERE IF.flag = 'Y'
    ) IF
ORDER BY IF.a;
/

WITH COUNT AS (
    SELECT
        a,
        b
    FROM qt_kw_base
)
SELECT COUNT.a
FROM (
        SELECT
            COUNT.a,
            COUNT.b
        FROM COUNT
    ) COUNT
ORDER BY COUNT.a;
/

--------------------------------------------------------------------------------
-- 8. GROUP BY / HAVING / ORDER BY / ANALYTIC
--------------------------------------------------------------------------------

SELECT IF.grp,
    COUNT (*) AS cnt,
    SUM (IF.c) AS sum_c,
    AVG (IF.c) AS avg_c
FROM qt_kw_base IF
GROUP BY IF.grp
HAVING SUM (IF.c) >= 30
ORDER BY IF.grp;
/

SELECT RANK.a,
    RANK.grp,
    ROW_NUMBER () OVER (PARTITION BY RANK.grp ORDER BY RANK.a) AS rn,
    SUM (RANK.c) OVER (PARTITION BY RANK.grp ORDER BY RANK.a ROWS BETWEEN UNBOUNDED PRECEDING AND CURRENT ROW) AS running_sum
FROM qt_kw_base RANK
ORDER BY RANK.a;
/

SELECT COUNT.a,
    COUNT.b,
    DENSE_RANK () OVER (ORDER BY COUNT.c DESC) AS dr
FROM qt_kw_base COUNT
ORDER BY COUNT.a;
/

--------------------------------------------------------------------------------
-- 9. SET OPERATORS
--------------------------------------------------------------------------------

SELECT *
FROM (
        SELECT IF.a,
            IF.b
        FROM qt_kw_base IF
        WHERE IF.grp = 1
        UNION ALL
        SELECT IF.a,
            IF.b
        FROM qt_kw_base IF
        WHERE IF.grp = 2
    ) IF
ORDER BY IF.a;
/

--------------------------------------------------------------------------------
-- 10. VIEW
--------------------------------------------------------------------------------

CREATE OR REPLACE VIEW qt_kw_mix_v AS
SELECT IF.a,
    IF.b,
    IF.c,
    IF.grp,
    IF.flag,
    (
        SELECT COUNT (*)
        FROM qt_kw_child COUNT
        WHERE COUNT.ref_a = IF.a
    ) AS child_cnt,
    (
        SELECT MAX (RANK.metric)
        FROM qt_kw_child RANK
        WHERE RANK.ref_a = IF.a
    ) AS max_metric
FROM qt_kw_base IF;
/

SELECT trim.a,
    trim.b,
    trim.child_cnt,
    trim.max_metric
FROM qt_kw_mix_v trim
ORDER BY trim.a;
/

--------------------------------------------------------------------------------
-- 11. INSERT ... SELECT
--------------------------------------------------------------------------------

INSERT INTO qt_kw_child (child_id, ref_a, seq_no, metric, note_txt, kind)
SELECT 900 + IF.a,
    IF.a,
    99,
    IF.c * 10,
    'bulk_insert',
    'N'
FROM qt_kw_base IF
WHERE IF.a IN (2, 4, 6);
/

ROLLBACK;
/

--------------------------------------------------------------------------------
-- 12. MERGE
--------------------------------------------------------------------------------
/

ROLLBACK;
/

--------------------------------------------------------------------------------
-- 13. UPDATE / DELETE
--------------------------------------------------------------------------------

UPDATE qt_kw_base COUNT
SET COUNT.c = (
    SELECT MAX (RANK.metric)
    FROM qt_kw_child RANK
    WHERE RANK.ref_a = COUNT.a
)
WHERE EXISTS (
        SELECT 1
        FROM qt_kw_child trim
        WHERE trim.ref_a = COUNT.a
    );
/

ROLLBACK;
/

DELETE
FROM qt_kw_child trim
WHERE trim.kind = 'NO_MATCH_KIND';
/

ROLLBACK;
/

--------------------------------------------------------------------------------
-- 14. FORMATTER / COMMENT / INDENT / CASE TORTURE
--------------------------------------------------------------------------------

SELECT IF.a,
    -- qualified identifier
    IF.b,
    /* block comment */
    IF.c,
    IF.grp
FROM qt_kw_base IF
WHERE IF.flag = 'Y'
ORDER BY IF.a;
/
/

SELECT
    CASE
        WHEN RANK.flag = 'Y' THEN (
            SELECT MAX (trim.metric)
            FROM qt_kw_child trim
            WHERE trim.ref_a = RANK.a
        )
        ELSE (
            SELECT MIN (trim.metric)
            FROM qt_kw_child trim
            WHERE trim.ref_a = RANK.a
        )
    END AS metric_pick
FROM qt_kw_base RANK
ORDER BY 1;
/

SELECT IF.a,
    CASE
        WHEN IF.grp = 1 THEN
        CASE
            WHEN IF.flag = 'Y' THEN 'G1Y'
            ELSE 'G1N'
        END
        WHEN IF.grp = 2 THEN
        CASE
            WHEN IF.c >= 35 THEN 'G2BIG'
            ELSE 'G2SMALL'
        END
        ELSE 'OTHER'
    END AS complex_case_result
FROM qt_kw_base IF
ORDER BY IF.a;
/

--------------------------------------------------------------------------------
-- 15. FINAL SANITY
--------------------------------------------------------------------------------

SELECT COUNT (*) AS total_base
FROM qt_kw_base;
/

SELECT COUNT (*) AS total_child
FROM qt_kw_child;
/

SELECT COUNT (*) AS total_view_rows
FROM qt_kw_mix_v;
/
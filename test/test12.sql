SET SERVEROUTPUT ON

SET DEFINE OFF

--------------------------------------------------------------------------------
-- CLEANUP
--------------------------------------------------------------------------------

BEGIN
    EXECUTE IMMEDIATE 'DROP TABLE qt_if_child PURGE';
EXCEPTION
    WHEN OTHERS THEN
        NULL;
END;
/

BEGIN
    EXECUTE IMMEDIATE 'DROP TABLE qt_if_base PURGE';
EXCEPTION
    WHEN OTHERS THEN
        NULL;
END;
/

--------------------------------------------------------------------------------
-- SETUP
--------------------------------------------------------------------------------

CREATE TABLE qt_if_base (
    a        NUMBER        NOT NULL,
    b        VARCHAR2(100),
    c        NUMBER,
    grp      NUMBER,
    flag     VARCHAR2(1),
    dt       DATE,
    category VARCHAR2(30),
    CONSTRAINT qt_if_base_pk PRIMARY KEY(a)
);
/

CREATE TABLE qt_if_child (
    child_id NUMBER        NOT NULL,
    ref_a    NUMBER        NOT NULL,
    seq_no   NUMBER        NOT NULL,
    metric   NUMBER,
    note_txt VARCHAR2(100),
    kind     VARCHAR2(20),
    CONSTRAINT qt_if_child_pk PRIMARY KEY(child_id),
    CONSTRAINT qt_if_child_fk FOREIGN KEY(ref_a) REFERENCES qt_if_base(a)
);
/

INSERT ALL
INTO qt_if_base
VALUES (1, 'alpha', 10, 1, 'Y', DATE '2024-01-01', 'A')
INTO qt_if_base
VALUES (2, 'beta', 20, 1, 'N', DATE '2024-01-02', 'A')
INTO qt_if_base
VALUES (3, 'gamma', 30, 2, 'Y', DATE '2024-01-03', 'B')
INTO qt_if_base
VALUES (4, 'delta', 40, 2, 'N', DATE '2024-01-04', 'B')
INTO qt_if_base
VALUES (5, 'epsilon', 50, 3, 'Y', DATE '2024-01-05', 'C')
SELECT 1
FROM DUAL;
/

INSERT ALL
INTO qt_if_child
VALUES (101, 1, 1, 100, 'n1', 'X')
INTO qt_if_child
VALUES (102, 1, 2, 150, 'n2', 'Y')
INTO qt_if_child
VALUES (103, 2, 1, 200, 'n3', 'X')
INTO qt_if_child
VALUES (104, 3, 1, 300, 'n4', 'Y')
INTO qt_if_child
VALUES (105, 3, 2, 350, 'n5', 'Z')
INTO qt_if_child
VALUES (106, 5, 1, 500, 'n6', 'X')
SELECT 1
FROM DUAL;
/

COMMIT;
/

--------------------------------------------------------------------------------
-- 1. BASIC TABLE ALIAS = if
--------------------------------------------------------------------------------

SELECT IF.a,
    IF.b,
    IF.c,
    IF.grp,
    IF.flag,
    IF.dt
FROM qt_if_base IF
ORDER BY IF.a;
/

SELECT IF.a,
    IF.b,
    IF.c * 10 AS c_x_10
FROM qt_if_base IF
WHERE IF.a >= 2
ORDER BY IF.a;
/

SELECT IF.a,
    UPPER (IF.b) AS upper_b,
    CASE
        WHEN IF.flag = 'Y' THEN 'YES'
        ELSE 'NO'
    END AS flag_text
FROM qt_if_base IF
ORDER BY IF.a;
/

--------------------------------------------------------------------------------
-- 2. JOIN + QUALIFIED COLUMN REFERENCES
--------------------------------------------------------------------------------

SELECT IF.a,
    IF.b,
    ch.child_id,
    ch.seq_no,
    ch.metric
FROM qt_if_base IF
JOIN qt_if_child ch
    ON ch.ref_a = IF.a
ORDER BY IF.a,
    ch.seq_no;
/

SELECT IF.a,
    IF.b,
    ch.metric,
    CASE
        WHEN ch.metric >= 300 THEN 'HIGH'
        WHEN ch.metric >= 150 THEN 'MID'
        ELSE 'LOW'
    END AS metric_band
FROM qt_if_base IF
LEFT JOIN qt_if_child ch
    ON ch.ref_a = IF.a
ORDER BY IF.a,
    ch.metric;
/

SELECT IF.a,
    IF.b,
    ch.metric
FROM qt_if_base IF
JOIN qt_if_child ch
    ON ch.ref_a = IF.a
    AND ch.kind IN ('X', 'Y')
WHERE IF.flag = 'Y'
ORDER BY IF.a,
    ch.metric;
/

--------------------------------------------------------------------------------
-- 3. INLINE VIEW ALIAS = if
--------------------------------------------------------------------------------

SELECT IF.a,
    IF.b,
    IF.c
FROM (
        SELECT a,
            b,
            c
        FROM qt_if_base
        WHERE flag = 'Y'
    ) IF
ORDER BY IF.a;
/

SELECT IF.a,
    IF.b,
    IF.grp
FROM (
        SELECT a,
            b,
            grp
        FROM qt_if_base
        WHERE grp IN (1, 2)
    ) IF
ORDER BY IF.a;
/

SELECT IF.a,
    IF.sum_metric
FROM (
        SELECT ch.ref_a AS a,
            SUM (ch.metric) AS sum_metric
        FROM qt_if_child ch
        GROUP BY ch.ref_a
    ) IF
ORDER BY IF.a;
/

--------------------------------------------------------------------------------
-- 4. CTE NAME = if
--------------------------------------------------------------------------------

WITH
IF AS (
        SELECT
            a,
            b,
            c,
            grp,
            flag
        FROM qt_if_base
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

WITH
IF AS (
        SELECT
            a,
            b,
            ROW_NUMBER () OVER (ORDER BY a) AS rn
        FROM qt_if_base
    )
    SELECT
        IF.a,
        IF.b,
        IF.rn
    FROM IF
    ORDER BY IF.a;
/

WITH
IF AS (
        SELECT
            ch.ref_a,
            COUNT (*) AS cnt,
            MAX (ch.metric) AS max_metric
        FROM qt_if_child ch
        GROUP BY ch.ref_a
    )
    SELECT
        IF.ref_a,
        IF.cnt,
        IF.max_metric
    FROM IF
    ORDER BY IF.ref_a;
/

--------------------------------------------------------------------------------
-- 5. EXISTS / IN / SCALAR SUBQUERY
--------------------------------------------------------------------------------

SELECT IF.a,
    IF.b
FROM qt_if_base IF
WHERE EXISTS (
        SELECT 1
        FROM qt_if_child ch
        WHERE ch.ref_a = IF.a
    )
ORDER BY IF.a;
/

SELECT IF.a,
    IF.b
FROM qt_if_base IF
WHERE IF.a IN (
        SELECT ch.ref_a
        FROM qt_if_child ch
        WHERE ch.metric >= 200
    )
ORDER BY IF.a;
/

SELECT IF.a,
    IF.b,
    (
        SELECT MAX (ch.metric)
        FROM qt_if_child ch
        WHERE ch.ref_a = IF.a
    ) AS max_metric
FROM qt_if_base IF
ORDER BY IF.a;
/

SELECT IF.a,
    IF.b,
    (
        SELECT COUNT (*)
        FROM qt_if_child ch
        WHERE ch.ref_a = IF.a
    ) AS child_count
FROM qt_if_base IF
ORDER BY IF.a;
/

--------------------------------------------------------------------------------
-- 6. NESTED SUBQUERY / MULTI-SCOPE
--------------------------------------------------------------------------------

SELECT IF.a,
    IF.b
FROM (
        SELECT IF.a,
            IF.b
        FROM qt_if_base IF
        WHERE IF.flag = 'Y'
    ) IF
ORDER BY IF.a;
/

WITH
IF AS (
        SELECT
            a,
            b
        FROM qt_if_base
    )
    SELECT IF.a
    FROM (
            SELECT
                IF.a,
                IF.b
            FROM IF
        ) IF
    ORDER BY IF.a;
/

SELECT IF.a,
    (
        SELECT COUNT (*)
        FROM (
                SELECT ch.metric
                FROM qt_if_child ch
                WHERE ch.ref_a = IF.a
            ) subq
    ) AS nested_cnt
FROM qt_if_base IF
ORDER BY IF.a;
/

--------------------------------------------------------------------------------
-- 7. GROUP BY / HAVING / ORDER BY / ANALYTIC
--------------------------------------------------------------------------------

SELECT IF.grp,
    COUNT (*) AS cnt,
    SUM (IF.c) AS sum_c,
    AVG (IF.c) AS avg_c
FROM qt_if_base IF
GROUP BY IF.grp
HAVING SUM (IF.c) >= 30
ORDER BY IF.grp;
/

SELECT IF.a,
    IF.grp,
    ROW_NUMBER () OVER (PARTITION BY IF.grp ORDER BY IF.a) AS rn,
    SUM (IF.c) OVER (PARTITION BY IF.grp ORDER BY IF.a ROWS BETWEEN UNBOUNDED PRECEDING AND CURRENT ROW) AS running_sum
FROM qt_if_base IF
ORDER BY IF.a;
/

SELECT IF.a,
    IF.b,
    DENSE_RANK () OVER (ORDER BY IF.c DESC) AS dr
FROM qt_if_base IF
ORDER BY IF.a;
/

--------------------------------------------------------------------------------
-- 8. SET OPERATORS
--------------------------------------------------------------------------------

SELECT IF.a,
    IF.b
FROM qt_if_base IF
WHERE IF.grp = 1
UNION ALL
SELECT IF.a,
    IF.b
FROM qt_if_base IF
WHERE IF.grp = 2
ORDER BY 1;
/

SELECT IF.a
FROM qt_if_base IF
WHERE IF.flag = 'Y'
MINUS
SELECT IF.a
FROM qt_if_base IF
WHERE IF.grp = 3
ORDER BY 1;
/

--------------------------------------------------------------------------------
-- 9. INSERT ... SELECT
--------------------------------------------------------------------------------

INSERT INTO qt_if_child (child_id, ref_a, seq_no, metric, note_txt, kind)
SELECT 900 + IF.a,
    IF.a,
    99,
    IF.c * 10,
    'bulk_insert',
    'N'
FROM qt_if_base IF
WHERE IF.a IN (2, 4);
/

ROLLBACK;
/

--------------------------------------------------------------------------------
-- 10. MERGE
--------------------------------------------------------------------------------

MERGE INTO qt_if_base
IF USING (
        SELECT 2 AS a,
            222 AS c,
            'beta_merge' AS b
        FROM DUAL
        UNION ALL
        SELECT 6 AS a,
            666 AS c,
            'zeta_merge' AS b
        FROM DUAL
    ) src
        ON (
    IF.a = src.a)
            WHEN MATCHED THEN
        UPDATE
        SET
        IF.c = src.c,
            IF.b = src.b
                    WHEN NOT MATCHED THEN
                INSERT (a, b, c, grp, flag, dt, category)
                VALUES (src.a, src.b, src.c, 9, 'N', DATE '2024-02-01', 'M');
/

ROLLBACK;
/

--------------------------------------------------------------------------------
-- 11. UPDATE / DELETE WITH ALIAS = if
--------------------------------------------------------------------------------

UPDATE qt_if_base
IF
    SET
    IF.c = (
            SELECT MAX (ch.metric)
            FROM qt_if_child ch
            WHERE ch.ref_a = IF.a
        )
        WHERE EXISTS (
                SELECT 1
                FROM qt_if_child ch
                WHERE ch.ref_a = IF.a
            );
/

ROLLBACK;
/

DELETE
FROM qt_if_child IF
WHERE IF.kind = 'NO_DATA_TO_DELETE';
/

ROLLBACK;
/

--------------------------------------------------------------------------------
-- 12. FORMATTER / COMMENT / INDENT TORTURE
--------------------------------------------------------------------------------

SELECT IF.a,
    -- qualified reference
    IF.b,
    /* block comment */
    IF.c,
    IF.grp
FROM qt_if_base IF
WHERE IF.flag = 'Y'
ORDER BY IF.a;
/

SELECT IF.a,
    IF.b,
    IF.c
FROM qt_if_base IF
WHERE IF.a IN (
        SELECT ch.ref_a
        FROM qt_if_child ch
        WHERE ch.metric >= 150
    )
ORDER BY IF.a;
/

SELECT
    CASE
        WHEN IF.flag = 'Y' THEN (
            SELECT MAX (ch.metric)
            FROM qt_if_child ch
            WHERE ch.ref_a = IF.a
        )
        ELSE (
            SELECT MIN (ch.metric)
            FROM qt_if_child ch
            WHERE ch.ref_a = IF.a
        )
    END AS metric_pick
FROM qt_if_base IF
ORDER BY 1;
/

--------------------------------------------------------------------------------
-- 13. FINAL SANITY
--------------------------------------------------------------------------------

SELECT COUNT (*) AS total_base
FROM qt_if_base;
/

SELECT COUNT (*) AS total_child
FROM qt_if_child;
/
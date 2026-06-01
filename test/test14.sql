SET SERVEROUTPUT ON
SET DEFINE OFF

--------------------------------------------------------------------------------
-- CLEANUP
--------------------------------------------------------------------------------
BEGIN
    EXECUTE IMMEDIATE 'DROP VIEW qt_depth_monster_v';
EXCEPTION
    WHEN OTHERS THEN NULL;
END;
/

BEGIN
    EXECUTE IMMEDIATE 'DROP TABLE qt_depth_child PURGE';
EXCEPTION
    WHEN OTHERS THEN NULL;
END;
/

BEGIN
    EXECUTE IMMEDIATE 'DROP TABLE qt_depth_base PURGE';
EXCEPTION
    WHEN OTHERS THEN NULL;
END;
/

--------------------------------------------------------------------------------
-- SETUP
--------------------------------------------------------------------------------
CREATE TABLE qt_depth_base
(
    a          NUMBER         NOT NULL,
    b          VARCHAR2(100),
    c          NUMBER,
    grp        NUMBER,
    flag       VARCHAR2(1),
    dt         DATE,
    category   VARCHAR2(30),
    subcat     VARCHAR2(30),
    CONSTRAINT qt_depth_base_pk PRIMARY KEY (a)
);
/

CREATE TABLE qt_depth_child
(
    child_id   NUMBER         NOT NULL,
    ref_a      NUMBER         NOT NULL,
    seq_no     NUMBER         NOT NULL,
    metric     NUMBER,
    note_txt   VARCHAR2(100),
    kind       VARCHAR2(20),
    CONSTRAINT qt_depth_child_pk PRIMARY KEY (child_id),
    CONSTRAINT qt_depth_child_fk FOREIGN KEY (ref_a) REFERENCES qt_depth_base(a)
);
/

INSERT ALL
    INTO qt_depth_base VALUES (1, 'alpha',   10, 1, 'Y', DATE '2024-01-01', 'A', 'A1')
    INTO qt_depth_base VALUES (2, 'beta',    20, 1, 'N', DATE '2024-01-02', 'A', 'A2')
    INTO qt_depth_base VALUES (3, 'gamma',   30, 2, 'Y', DATE '2024-01-03', 'B', 'B1')
    INTO qt_depth_base VALUES (4, 'delta',   40, 2, 'N', DATE '2024-01-04', 'B', 'B2')
    INTO qt_depth_base VALUES (5, 'epsilon', 50, 3, 'Y', DATE '2024-01-05', 'C', 'C1')
    INTO qt_depth_base VALUES (6, 'zeta',    60, 3, 'N', DATE '2024-01-06', 'C', 'C2')
    INTO qt_depth_base VALUES (7, 'eta',     70, 4, 'Y', DATE '2024-01-07', 'D', 'D1')
SELECT 1 FROM dual;
/

INSERT ALL
    INTO qt_depth_child VALUES (101, 1, 1, 100, 'n1',  'X')
    INTO qt_depth_child VALUES (102, 1, 2, 150, 'n2',  'Y')
    INTO qt_depth_child VALUES (103, 1, 3, 175, 'n3',  'Z')
    INTO qt_depth_child VALUES (104, 2, 1, 200, 'n4',  'X')
    INTO qt_depth_child VALUES (105, 2, 2, 225, 'n5',  'Y')
    INTO qt_depth_child VALUES (106, 3, 1, 300, 'n6',  'Y')
    INTO qt_depth_child VALUES (107, 3, 2, 350, 'n7',  'Z')
    INTO qt_depth_child VALUES (108, 4, 1, 400, 'n8',  'X')
    INTO qt_depth_child VALUES (109, 4, 2, 425, 'n9',  'Y')
    INTO qt_depth_child VALUES (110, 5, 1, 500, 'n10', 'Y')
    INTO qt_depth_child VALUES (111, 5, 2, 550, 'n11', 'Z')
    INTO qt_depth_child VALUES (112, 6, 1, 600, 'n12', 'X')
    INTO qt_depth_child VALUES (113, 6, 2, 650, 'n13', 'Z')
    INTO qt_depth_child VALUES (114, 7, 1, 700, 'n14', 'Y')
SELECT 1 FROM dual;
/

COMMIT;
/

--------------------------------------------------------------------------------
-- MONSTER VIEW
-- Focus:
--   - deep CTE chain
--   - nested scalar subqueries
--   - EXISTS inside EXISTS
--   - repeated keyword-like aliases across scopes
--   - analytic functions on already deep derived data
--------------------------------------------------------------------------------
CREATE OR REPLACE VIEW qt_depth_monster_v
AS
WITH
s1_base AS
(
    SELECT
        if.a,
        if.b,
        if.c,
        if.grp,
        if.flag,
        if.dt,
        if.category,
        if.subcat
    FROM qt_depth_base if
),
s2_child_rollup AS
(
    SELECT
        date.ref_a AS a,
        COUNT(*) AS cnt_child,
        SUM(date.metric) AS sum_metric,
        MAX(date.metric) AS max_metric,
        MIN(date.metric) AS min_metric
    FROM
    (
        SELECT
            date.ref_a,
            date.metric
        FROM qt_depth_child date
        WHERE date.kind IN ('X', 'Y')

        UNION ALL

        SELECT
            date.ref_a,
            date.metric
        FROM qt_depth_child date
        WHERE date.kind NOT IN ('X', 'Y')
    ) date
    GROUP BY date.ref_a
),
s3_nested AS
(
    SELECT
        level.a,
        level.b,
        level.c,
        level.grp,
        level.flag,
        level.dt,
        level.category,
        level.subcat,
        NVL(roll.cnt_child, 0) AS cnt_child,
        NVL(roll.sum_metric, 0) AS sum_metric,
        NVL(roll.max_metric, 0) AS max_metric,
        NVL(roll.min_metric, 0) AS min_metric,

        (
            SELECT COUNT(*)
            FROM
            (
                SELECT 1
                FROM qt_depth_child trim
                WHERE trim.ref_a = level.a
                  AND EXISTS
                  (
                      SELECT 1
                      FROM
                      (
                          SELECT
                              rank.child_id,
                              rank.metric
                          FROM qt_depth_child rank
                          WHERE rank.ref_a = trim.ref_a
                      ) rank
                      WHERE rank.child_id = trim.child_id
                        AND rank.metric >= trim.metric
                  )
            ) count
        ) AS stable_cnt,

        (
            SELECT NVL(SUM(x.metric), 0)
            FROM
            (
                SELECT x.metric
                FROM
                (
                    SELECT count.metric
                    FROM qt_depth_child count
                    WHERE count.ref_a = level.a
                      AND count.metric >=
                          (
                              SELECT NVL(MIN(date.metric), 0)
                              FROM
                              (
                                  SELECT date.metric
                                  FROM qt_depth_child date
                                  WHERE date.ref_a = level.a
                              ) date
                          )
                ) x
            ) x
        ) AS sum_of_ge_min,

        CASE
            WHEN EXISTS
                 (
                     SELECT 1
                     FROM
                     (
                         SELECT
                             trim.ref_a,
                             SUM(trim.metric) AS sum_metric
                         FROM qt_depth_child trim
                         GROUP BY trim.ref_a
                     ) trim
                     WHERE trim.ref_a = level.a
                       AND trim.sum_metric >
                           (
                               SELECT AVG(count.sum_metric)
                               FROM
                               (
                                   SELECT
                                       count.ref_a,
                                       SUM(count.metric) AS sum_metric
                                   FROM qt_depth_child count
                                   GROUP BY count.ref_a
                               ) count
                           )
                 )
                THEN 'ABOVE_AVG'
            ELSE 'NOT_ABOVE_AVG'
        END AS bucket
    FROM s1_base level
    LEFT JOIN s2_child_rollup roll
        ON roll.a = level.a
),
s4_analytic AS
(
    SELECT
        rank.a,
        rank.b,
        rank.c,
        rank.grp,
        rank.flag,
        rank.dt,
        rank.category,
        rank.subcat,
        rank.cnt_child,
        rank.sum_metric,
        rank.max_metric,
        rank.min_metric,
        rank.stable_cnt,
        rank.sum_of_ge_min,
        rank.bucket,
        ROW_NUMBER() OVER
        (
            PARTITION BY rank.grp
            ORDER BY rank.max_metric DESC, rank.a
        ) AS rn,
        DENSE_RANK() OVER
        (
            ORDER BY rank.sum_metric DESC, rank.a
        ) AS dr,
        SUM(rank.c) OVER
        (
            PARTITION BY rank.grp
            ORDER BY rank.a
            ROWS BETWEEN UNBOUNDED PRECEDING AND CURRENT ROW
        ) AS running_c
    FROM s3_nested rank
),
s5_case AS
(
    SELECT
        count.a,
        count.b,
        count.c,
        count.grp,
        count.flag,
        count.dt,
        count.category,
        count.subcat,
        count.cnt_child,
        count.sum_metric,
        count.max_metric,
        count.min_metric,
        count.stable_cnt,
        count.sum_of_ge_min,
        count.bucket,
        count.rn,
        count.dr,
        count.running_c,

        CASE
            WHEN count.flag = 'Y'
                THEN
                    (
                        SELECT NVL(MAX(x.metric), 0)
                        FROM
                        (
                            SELECT x.metric
                            FROM
                            (
                                SELECT rank.metric
                                FROM qt_depth_child rank
                                WHERE rank.ref_a = count.a
                            ) x
                        ) x
                    )
            ELSE
                    (
                        SELECT NVL(MIN(x.metric), 0)
                        FROM
                        (
                            SELECT x.metric
                            FROM
                            (
                                SELECT level.metric
                                FROM qt_depth_child level
                                WHERE level.ref_a = count.a
                            ) x
                        ) x
                    )
        END AS case_metric,

        CASE
            WHEN count.cnt_child > 0
                THEN
                    (
                        SELECT NVL(MAX(last.metric_val), 0)
                        FROM
                        (
                            SELECT last.metric_val
                            FROM
                            (
                                SELECT trim.metric AS metric_val
                                FROM qt_depth_child trim
                                WHERE trim.ref_a = count.a

                                UNION ALL

                                SELECT 0 AS metric_val
                                FROM dual
                            ) last
                        ) last
                    )
            ELSE 0
        END AS final_max_metric
    FROM s4_analytic count
),
s6_final AS
(
    SELECT
        trim.a,
        trim.b,
        trim.c,
        trim.grp,
        trim.flag,
        trim.dt,
        trim.category,
        trim.subcat,
        trim.cnt_child,
        trim.sum_metric,
        trim.max_metric,
        trim.min_metric,
        trim.stable_cnt,
        trim.sum_of_ge_min,
        trim.bucket,
        trim.rn,
        trim.dr,
        trim.running_c,
        trim.case_metric,
        trim.final_max_metric,

        CASE
            WHEN trim.case_metric >
                 (
                     SELECT AVG(z.case_metric)
                     FROM
                     (
                         SELECT z.case_metric
                         FROM
                         (
                             SELECT date.case_metric
                             FROM s5_case date
                             WHERE date.grp = trim.grp
                         ) z
                     ) z
                 )
                THEN 'CASE_GT_GRP_AVG'
            ELSE 'CASE_LE_GRP_AVG'
        END AS case_vs_group,

        (
            SELECT COUNT(*)
            FROM
            (
                SELECT 1
                FROM
                (
                    SELECT
                        if.ref_a,
                        if.kind
                    FROM qt_depth_child if
                    WHERE if.ref_a = trim.a
                ) if
                WHERE if.kind IN ('X', 'Y', 'Z')
            ) count
        ) AS final_kind_cnt
    FROM s5_case trim
)
SELECT
    if.a,
    if.b,
    if.c,
    if.grp,
    if.flag,
    if.dt,
    if.category,
    if.subcat,
    if.cnt_child,
    if.sum_metric,
    if.max_metric,
    if.min_metric,
    if.stable_cnt,
    if.sum_of_ge_min,
    if.bucket,
    if.rn,
    if.dr,
    if.running_c,
    if.case_metric,
    if.final_max_metric,
    if.case_vs_group,
    if.final_kind_cnt,

    (
        SELECT COUNT(*)
        FROM
        (
            SELECT 1
            FROM s6_final level
            WHERE level.grp = if.grp
              AND level.a <= if.a
              AND EXISTS
                  (
                      SELECT 1
                      FROM
                      (
                          SELECT date.a
                          FROM s6_final date
                          WHERE date.a = level.a
                      ) date
                      WHERE date.a = level.a
                  )
        ) count
    ) AS grp_prefix_cnt

FROM s6_final if
WHERE if.a IN
(
    SELECT level.a
    FROM
    (
        SELECT level.a
        FROM s6_final level
        WHERE level.dr <= 999
          AND level.a IN
              (
                  SELECT date.ref_a
                  FROM
                  (
                      SELECT date.ref_a
                      FROM qt_depth_child date
                      GROUP BY date.ref_a
                      HAVING COUNT(*) >= 1
                  ) date
              )
    ) level
);
/

--------------------------------------------------------------------------------
-- FINAL EXECUTION
--------------------------------------------------------------------------------
SELECT
    trim.a,
    trim.b,
    trim.grp,
    trim.cnt_child,
    trim.sum_metric,
    trim.max_metric,
    trim.min_metric,
    trim.stable_cnt,
    trim.sum_of_ge_min,
    trim.bucket,
    trim.rn,
    trim.dr,
    trim.running_c,
    trim.case_metric,
    trim.final_max_metric,
    trim.case_vs_group,
    trim.final_kind_cnt,
    trim.grp_prefix_cnt
FROM
(
    SELECT
        trim.*
    FROM
    (
        SELECT
            trim.*
        FROM qt_depth_monster_v trim
    ) trim
) trim
ORDER BY trim.grp, trim.a;
/

--------------------------------------------------------------------------------
-- SANITY
--------------------------------------------------------------------------------
SELECT COUNT(*) AS total_base FROM qt_depth_base;
/
SELECT COUNT(*) AS total_child FROM qt_depth_child;
/
SELECT COUNT(*) AS total_view_rows FROM qt_depth_monster_v;
/
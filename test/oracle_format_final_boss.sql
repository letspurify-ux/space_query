/*==============================================================================
  ORACLE AUTO-FORMATTING FINAL BOSS - SINGLE INTEGRATED SCRIPT
  목적:
    - 자동 포맷팅기의 괄호 깊이/절 정렬/CASE 중첩/블록 경계/실행단위 분리 검증
    - 한 파일 안에 최고 난이도 구조를 몰아넣은 통합 테스트 스크립트
==============================================================================*/

/*==============================================================================
  UNIT 1
  Deep WITH + Analytic + Scalar Subquery + CROSS APPLY + XML/JSON + CASE Nest
==============================================================================*/
WITH
    dept_data AS
    (
        SELECT 10 AS dept_id, 'DEV' AS dept_name FROM dual
        UNION ALL
        SELECT 20 AS dept_id, 'OPS' AS dept_name FROM dual
        UNION ALL
        SELECT 30 AS dept_id, 'QA'  AS dept_name FROM dual
    ),
    emp_data AS
    (
        SELECT 1001 AS emp_id, 10 AS dept_id, 'ALICE' AS emp_name, 9000 AS salary, DATE '2024-01-10' AS hire_dt, 'A' AS grade FROM dual
        UNION ALL
        SELECT 1002, 10, 'BOB',   8700, DATE '2023-07-01', 'B' FROM dual
        UNION ALL
        SELECT 1003, 10, 'CAROL', 8700, DATE '2022-02-11', 'A' FROM dual
        UNION ALL
        SELECT 2001, 20, 'DAVE',  7200, DATE '2021-09-15', 'C' FROM dual
        UNION ALL
        SELECT 2002, 20, 'ERIN',  7600, DATE '2020-03-20', 'B' FROM dual
        UNION ALL
        SELECT 3001, 30, 'FRANK', 6500, DATE '2024-02-01', 'B' FROM dual
        UNION ALL
        SELECT 3002, 30, 'GRACE', 6100, DATE '2024-02-14', 'A' FROM dual
    ),
    bonus_data AS
    (
        SELECT 1001 AS emp_id, 202401 AS yyyymm, 300 AS bonus_amt FROM dual
        UNION ALL
        SELECT 1001, 202402, 250 FROM dual
        UNION ALL
        SELECT 1002, 202401, 200 FROM dual
        UNION ALL
        SELECT 2001, 202401, 180 FROM dual
        UNION ALL
        SELECT 3002, 202402, 500 FROM dual
    ),
    ranked_emp AS
    (
        SELECT
            e.*,
            DENSE_RANK() OVER (PARTITION BY e.dept_id ORDER BY e.salary DESC, e.emp_id) AS salary_rank,
            ROW_NUMBER() OVER (PARTITION BY e.dept_id ORDER BY e.hire_dt, e.emp_id) AS hire_seq,
            AVG(e.salary) OVER (PARTITION BY e.dept_id) AS dept_avg_salary
        FROM emp_data e
    )
SELECT
    d.dept_id,
    d.dept_name,
    x.emp_id,
    x.emp_name,
    x.salary,
    x.salary_rank,
    x.hire_seq,
    x.dept_avg_salary,
    CASE
        WHEN x.salary > x.dept_avg_salary THEN
            CASE
                WHEN EXISTS
                     (
                         SELECT 1
                         FROM bonus_data b
                         WHERE b.emp_id = x.emp_id
                           AND b.bonus_amt >= 300
                     )
                THEN 'TOP_WITH_BONUS'
                ELSE 'TOP_NO_BIG_BONUS'
            END
        ELSE
            CASE
                WHEN x.grade IN ('A', 'B') THEN 'MID_GOOD_GRADE'
                ELSE 'MID_OTHER'
            END
    END AS emp_class,
    (
        SELECT MAX(b.bonus_amt) KEEP (DENSE_RANK LAST ORDER BY b.yyyymm)
        FROM bonus_data b
        WHERE b.emp_id = x.emp_id
    ) AS latest_bonus,
    (
        SELECT LISTAGG(t.token, ' | ') WITHIN GROUP (ORDER BY t.ord)
        FROM
        (
            SELECT 1 AS ord, 'EMP=' || x.emp_name AS token FROM dual
            UNION ALL
            SELECT 2, 'DEPT=' || d.dept_name FROM dual
            UNION ALL
            SELECT 3, 'RANK=' || TO_CHAR(x.salary_rank) FROM dual
        ) t
    ) AS pretty_line,
    JSON_OBJECT(
        'dept' VALUE d.dept_name,
        'emp' VALUE x.emp_name,
        'salary' VALUE x.salary,
        'meta' VALUE JSON_OBJECT(
            'rank' VALUE x.salary_rank,
            'hireSeq' VALUE x.hire_seq,
            'avgSalary' VALUE ROUND(x.dept_avg_salary, 2)
        )
    ) AS json_payload,
    XMLSERIALIZE
    (
        CONTENT
            XMLELEMENT
            (
                "employee",
                XMLATTRIBUTES(x.emp_id AS "id", d.dept_name AS "dept"),
                XMLELEMENT("name", x.emp_name),
                XMLELEMENT("salary", x.salary),
                XMLELEMENT
                (
                    "flags",
                    XMLELEMENT("is_top", CASE WHEN x.salary_rank = 1 THEN 'Y' ELSE 'N' END),
                    XMLELEMENT("grade", x.grade)
                )
            )
        AS CLOB
    ) AS xml_payload
FROM dept_data d
CROSS APPLY
(
    SELECT r.*
    FROM ranked_emp r
    WHERE r.dept_id = d.dept_id
      AND r.salary >=
          (
              SELECT AVG(r2.salary)
              FROM ranked_emp r2
              WHERE r2.dept_id = r.dept_id
          )
) x
WHERE EXISTS
(
    SELECT 1
    FROM emp_data e2
    WHERE e2.dept_id = d.dept_id
      AND e2.emp_id = x.emp_id
)
ORDER BY
    d.dept_id,
    x.salary_rank,
    x.emp_id
;

/*==============================================================================
  UNIT 2
  Hierarchical Query + Nested Scalar Subquery + CONNECT BY + SYS_CONNECT_BY_PATH
==============================================================================*/
WITH
    org_tree AS
    (
        SELECT 1 AS node_id, CAST(NULL AS NUMBER) AS parent_id, 'ROOT' AS node_name, 100 AS node_val FROM dual
        UNION ALL
        SELECT 2, 1, 'SALES',    50 FROM dual
        UNION ALL
        SELECT 3, 1, 'TECH',     80 FROM dual
        UNION ALL
        SELECT 4, 2, 'DOMESTIC', 20 FROM dual
        UNION ALL
        SELECT 5, 2, 'GLOBAL',   30 FROM dual
        UNION ALL
        SELECT 6, 3, 'PLATFORM', 40 FROM dual
        UNION ALL
        SELECT 7, 3, 'DATA',     40 FROM dual
        UNION ALL
        SELECT 8, 6, 'API',      10 FROM dual
        UNION ALL
        SELECT 9, 6, 'CLIENT',   15 FROM dual
        UNION ALL
        SELECT 10, 7, 'ML',      25 FROM dual
    )
SELECT
    CONNECT_BY_ROOT o.node_name AS root_name,
    o.node_id,
    o.parent_id,
    o.node_name,
    LEVEL AS lvl,
    SYS_CONNECT_BY_PATH(o.node_name, ' > ') AS node_path,
    CONNECT_BY_ISLEAF AS is_leaf,
    (
        SELECT SUM(x.node_val)
        FROM org_tree x
        START WITH x.node_id = o.node_id
        CONNECT BY PRIOR x.node_id = x.parent_id
    ) AS subtree_sum,
    CASE
        WHEN CONNECT_BY_ISLEAF = 1 THEN
            (
                SELECT MAX(y.node_val)
                FROM org_tree y
                WHERE y.parent_id = o.parent_id
            )
        ELSE
            (
                SELECT COUNT(*)
                FROM org_tree c
                WHERE c.parent_id = o.node_id
            )
    END AS compare_metric
FROM org_tree o
START WITH o.parent_id IS NULL
CONNECT BY PRIOR o.node_id = o.parent_id
ORDER SIBLINGS BY
    CASE
        WHEN o.parent_id IS NULL THEN 0
        ELSE 1
    END,
    o.node_name
;

/*==============================================================================
  UNIT 3
  Recursive WITH + SEARCH + CYCLE
==============================================================================*/
WITH
    graph (node_id, parent_id, node_name) AS
    (
        SELECT 1, CAST(NULL AS NUMBER), 'A' FROM dual
        UNION ALL
        SELECT 2, 1, 'B' FROM dual
        UNION ALL
        SELECT 3, 1, 'C' FROM dual
        UNION ALL
        SELECT 4, 2, 'D' FROM dual
        UNION ALL
        SELECT 5, 4, 'E' FROM dual
    ),
    r (node_id, parent_id, node_name, lvl, path_txt) AS
    (
        SELECT
            g.node_id,
            g.parent_id,
            g.node_name,
            1 AS lvl,
            TO_CHAR(g.node_name) AS path_txt
        FROM graph g
        WHERE g.parent_id IS NULL
        UNION ALL
        SELECT
            g.node_id,
            g.parent_id,
            g.node_name,
            r.lvl + 1,
            r.path_txt || ' > ' || g.node_name
        FROM graph g
        JOIN r
          ON g.parent_id = r.node_id
    )
    SEARCH DEPTH FIRST BY node_id SET dfs_order
    CYCLE node_id SET is_cycle TO 'Y' DEFAULT 'N'
SELECT
    node_id,
    parent_id,
    node_name,
    lvl,
    path_txt,
    dfs_order,
    is_cycle
FROM r
ORDER BY
    dfs_order
;

/*==============================================================================
  UNIT 4
  PIVOT + UNPIVOT + CASE + GROUP BY
==============================================================================*/
WITH
    src AS
    (
        SELECT 10 AS dept_id, 'ALICE' AS emp_name, 'Q1' AS qtr, 100 AS amt FROM dual
        UNION ALL
        SELECT 10, 'ALICE', 'Q2', 120 FROM dual
        UNION ALL
        SELECT 10, 'BOB',   'Q1',  90 FROM dual
        UNION ALL
        SELECT 10, 'BOB',   'Q2', 150 FROM dual
        UNION ALL
        SELECT 20, 'DAVE',  'Q1',  80 FROM dual
        UNION ALL
        SELECT 20, 'DAVE',  'Q2', 110 FROM dual
        UNION ALL
        SELECT 20, 'ERIN',  'Q1', 140 FROM dual
        UNION ALL
        SELECT 20, 'ERIN',  'Q2', 130 FROM dual
    ),
    p AS
    (
        SELECT *
        FROM src
        PIVOT
        (
            SUM(amt)
            FOR qtr IN ('Q1' AS q1, 'Q2' AS q2)
        )
    ),
    u AS
    (
        SELECT *
        FROM p
        UNPIVOT
        (
            amt FOR qtr IN
            (
                q1 AS 'Q1',
                q2 AS 'Q2'
            )
        )
    )
SELECT
    u.dept_id,
    u.emp_name,
    MAX(CASE WHEN u.qtr = 'Q1' THEN u.amt END) AS q1_amt,
    MAX(CASE WHEN u.qtr = 'Q2' THEN u.amt END) AS q2_amt,
    CASE
        WHEN MAX(CASE WHEN u.qtr = 'Q2' THEN u.amt END)
             >
             MAX(CASE WHEN u.qtr = 'Q1' THEN u.amt END)
        THEN 'UP'
        WHEN MAX(CASE WHEN u.qtr = 'Q2' THEN u.amt END)
             <
             MAX(CASE WHEN u.qtr = 'Q1' THEN u.amt END)
        THEN 'DOWN'
        ELSE 'SAME'
    END AS trend_flag
FROM u
GROUP BY
    u.dept_id,
    u.emp_name
ORDER BY
    u.dept_id,
    u.emp_name
;

/*==============================================================================
  UNIT 5
  MATCH_RECOGNIZE
==============================================================================*/
WITH
    price_series AS
    (
        SELECT 1 AS seq_no, 100 AS price FROM dual
        UNION ALL
        SELECT 2,  95 FROM dual
        UNION ALL
        SELECT 3,  90 FROM dual
        UNION ALL
        SELECT 4,  92 FROM dual
        UNION ALL
        SELECT 5,  97 FROM dual
        UNION ALL
        SELECT 6,  93 FROM dual
        UNION ALL
        SELECT 7,  91 FROM dual
        UNION ALL
        SELECT 8,  94 FROM dual
        UNION ALL
        SELECT 9, 101 FROM dual
    )
SELECT
    start_seq,
    end_seq,
    start_price,
    end_price,
    span_len
FROM price_series
MATCH_RECOGNIZE
(
    ORDER BY seq_no
    MEASURES
        FIRST(down.seq_no) AS start_seq,
        LAST(up.seq_no) AS end_seq,
        FIRST(down.price) AS start_price,
        LAST(up.price) AS end_price,
        COUNT(*) AS span_len
    ONE ROW PER MATCH
    PATTERN (down+ up+)
    DEFINE
        down AS down.price < PREV(down.price),
        up   AS up.price   > PREV(up.price)
)
;

/*==============================================================================
  UNIT 6
  MODEL Clause
==============================================================================*/
SELECT
    dept_id,
    month_no,
    amount,
    forecast,
    adjusted
FROM
(
    SELECT 10 AS dept_id, 1 AS month_no, 100 AS amount FROM dual
    UNION ALL
    SELECT 10, 2, 120 FROM dual
    UNION ALL
    SELECT 10, 3, 115 FROM dual
    UNION ALL
    SELECT 20, 1, 200 FROM dual
    UNION ALL
    SELECT 20, 2, 210 FROM dual
    UNION ALL
    SELECT 20, 3, 190 FROM dual
)
MODEL
    PARTITION BY (dept_id)
    DIMENSION BY (month_no)
    MEASURES
    (
        amount,
        CAST(NULL AS NUMBER) AS forecast,
        CAST(NULL AS NUMBER) AS adjusted
    )
    RULES SEQUENTIAL ORDER
    (
        forecast[ANY] =
            CASE
                WHEN CV(month_no) = 1 THEN amount[CV()]
                ELSE ROUND((amount[CV()] + amount[CV() - 1]) / 2, 2)
            END,
        adjusted[ANY] =
            CASE
                WHEN CV(month_no) = 1 THEN forecast[CV()]
                ELSE ROUND(forecast[CV()] + NVL(adjusted[CV() - 1], 0) * 0.05, 2)
            END
    )
ORDER BY
    dept_id,
    month_no
;

/*==============================================================================
  UNIT 7
  WITH inside scalar subquery + EXISTS / ANY / ALL
==============================================================================*/
WITH
    main_data AS
    (
        SELECT 1 AS id, 'A' AS category, 10 AS score FROM dual
        UNION ALL
        SELECT 2, 'A', 20 FROM dual
        UNION ALL
        SELECT 3, 'B', 15 FROM dual
        UNION ALL
        SELECT 4, 'B', 40 FROM dual
        UNION ALL
        SELECT 5, 'C',  5 FROM dual
    )
SELECT
    m.id,
    m.category,
    m.score,
    (
        WITH cat_stat AS
        (
            SELECT
                x.category,
                AVG(x.score) AS avg_score,
                MAX(x.score) AS max_score,
                MIN(x.score) AS min_score
            FROM main_data x
            GROUP BY x.category
        )
        SELECT
            CASE
                WHEN m.score >= c.max_score THEN 'AT_MAX'
                WHEN m.score <= c.min_score THEN 'AT_MIN'
                WHEN m.score > c.avg_score THEN 'ABOVE_AVG'
                ELSE 'BELOW_OR_EQUAL_AVG'
            END
        FROM cat_stat c
        WHERE c.category = m.category
    ) AS category_position
FROM main_data m
WHERE EXISTS
(
    SELECT 1
    FROM main_data e
    WHERE e.category = m.category
      AND e.score >= ALL
          (
              SELECT z.score
              FROM main_data z
              WHERE z.category = m.category
                AND z.id <> m.id
          )
)
OR m.score > ANY
(
    SELECT q.score
    FROM main_data q
    WHERE q.category <> m.category
)
ORDER BY
    m.category,
    m.score DESC,
    m.id
;

/*==============================================================================
  UNIT 8
  CROSS JOIN generated sets + Deep Parentheses + Arithmetic + Nested CASE
==============================================================================*/
WITH
    a AS
    (
        SELECT LEVEL AS n
        FROM dual
        CONNECT BY LEVEL <= 3
    ),
    b AS
    (
        SELECT LEVEL * 10 AS n
        FROM dual
        CONNECT BY LEVEL <= 3
    ),
    c AS
    (
        SELECT LEVEL * 100 AS n
        FROM dual
        CONNECT BY LEVEL <= 2
    )
SELECT
    a.n AS a_n,
    b.n AS b_n,
    c.n AS c_n,
    (
        (
            (a.n + b.n)
            * (c.n - a.n)
        )
        /
        NULLIF
        (
            (
                CASE
                    WHEN MOD(b.n, 20) = 0 THEN 2
                    ELSE 1
                END
            ),
            0
        )
    ) AS complex_calc,
    CASE
        WHEN
            (
                (
                    SELECT COUNT(*)
                    FROM dual
                    WHERE a.n < b.n
                ) = 1
                AND
                (
                    SELECT COUNT(*)
                    FROM dual
                    WHERE c.n > b.n
                ) IN (0, 1)
            )
        THEN
            CASE
                WHEN a.n = 1 AND b.n = 10 THEN 'PATH_1'
                WHEN a.n = 2 OR c.n = 200 THEN 'PATH_2'
                ELSE 'PATH_3'
            END
        ELSE 'PATH_4'
    END AS branch_flag
FROM a
CROSS JOIN b
CROSS JOIN c
ORDER BY
    a.n,
    b.n,
    c.n
;

/*==============================================================================
  UNIT 9
  Scalar Subquery + Inline View + Analytic + LISTAGG + Nested CASE
==============================================================================*/
WITH
    t AS
    (
        SELECT 1 AS grp_id, 'A' AS code, 10 AS val FROM dual
        UNION ALL
        SELECT 1, 'B', 20 FROM dual
        UNION ALL
        SELECT 1, 'C', 30 FROM dual
        UNION ALL
        SELECT 2, 'A',  5 FROM dual
        UNION ALL
        SELECT 2, 'B', 15 FROM dual
        UNION ALL
        SELECT 2, 'C', 25 FROM dual
    )
SELECT
    x.grp_id,
    x.code,
    x.val,
    (
        SELECT LISTAGG(y.code || ':' || y.val, ',')
               WITHIN GROUP (ORDER BY y.val DESC, y.code)
        FROM t y
        WHERE y.grp_id = x.grp_id
    ) AS grp_summary,
    (
        SELECT MAX(z.val)
        FROM
        (
            SELECT
                t2.*,
                DENSE_RANK() OVER (PARTITION BY t2.grp_id ORDER BY t2.val DESC, t2.code) AS dr
            FROM t t2
            WHERE t2.grp_id = x.grp_id
        ) z
        WHERE z.dr = 1
    ) AS grp_top_val,
    CASE
        WHEN x.val =
             (
                 SELECT MAX(m.val)
                 FROM t m
                 WHERE m.grp_id = x.grp_id
             )
        THEN
            CASE
                WHEN x.code =
                     (
                         SELECT MIN(n.code) KEEP (DENSE_RANK FIRST ORDER BY n.val DESC, n.code)
                         FROM t n
                         WHERE n.grp_id = x.grp_id
                     )
                THEN 'TOP_AND_FIRST_CODE'
                ELSE 'TOP_BUT_NOT_FIRST_CODE'
            END
        ELSE
            CASE
                WHEN x.val >
                     (
                         SELECT AVG(a2.val)
                         FROM t a2
                         WHERE a2.grp_id = x.grp_id
                     )
                THEN 'ABOVE_AVG'
                ELSE 'NOT_ABOVE_AVG'
            END
    END AS class_flag
FROM t x
ORDER BY
    x.grp_id,
    x.code
;

/*==============================================================================
  UNIT 10
  Anonymous PL/SQL Block + Labels + Nested DECLARE/BEGIN/EXCEPTION/END
  + Dynamic SQL + q-Quote + CASE + Loop + SQL inside string
==============================================================================*/
DECLARE
    TYPE t_num_tab IS TABLE OF NUMBER INDEX BY PLS_INTEGER;
    TYPE t_str_tab IS TABLE OF VARCHAR2(100) INDEX BY PLS_INTEGER;

    v_nums           t_num_tab;
    v_names          t_str_tab;
    v_idx            PLS_INTEGER := 0;
    v_total          NUMBER := 0;
    v_sql            VARCHAR2(32767);
    v_result         VARCHAR2(4000);
    v_flag           VARCHAR2(30) := 'INIT';
    v_json_like      VARCHAR2(4000);
    e_custom         EXCEPTION;

    FUNCTION classify_value(p_val IN NUMBER, p_name IN VARCHAR2) RETURN VARCHAR2 IS
        v_ret VARCHAR2(200);
    BEGIN
        v_ret :=
            CASE
                WHEN p_val IS NULL THEN 'NULL_VAL'
                WHEN p_val >= 100 THEN
                    CASE
                        WHEN p_name LIKE 'A%' THEN 'BIG_A'
                        WHEN p_name LIKE 'B%' THEN 'BIG_B'
                        ELSE 'BIG_OTHER'
                    END
                WHEN p_val BETWEEN 50 AND 99 THEN 'MID'
                ELSE 'SMALL'
            END;

        RETURN v_ret;
    END classify_value;

BEGIN
    v_nums(1) := 10;
    v_nums(2) := 55;
    v_nums(3) := 120;

    v_names(1) := 'ALICE';
    v_names(2) := 'BOB';
    v_names(3) := 'CAROL';

    <<outer_loop>>
    FOR i IN 1 .. v_nums.COUNT LOOP
        <<inner_block>>
        DECLARE
            v_local_class VARCHAR2(100);
            v_local_msg   VARCHAR2(4000);
        BEGIN
            v_local_class := classify_value(v_nums(i), v_names(i));

            v_sql :=
                   q'[
                        SELECT
                            CASE
                                WHEN :p_num > 100 THEN
                                    '(' || :p_name || ')-H'
                                WHEN :p_num BETWEEN 50 AND 100 THEN
                                    '(' || :p_name || ')-M'
                                ELSE
                                    '(' || :p_name || ')-L'
                            END
                        FROM dual
                     ]';

            EXECUTE IMMEDIATE v_sql
                INTO v_result
                USING v_nums(i), v_names(i), v_nums(i), v_names(i);

            v_local_msg :=
                   'IDX=' || i
                || ',NAME=' || v_names(i)
                || ',CLASS=' || v_local_class
                || ',SQL_RESULT=' || v_result;

            v_total := v_total + v_nums(i);

            v_json_like :=
                   '{'
                || '"idx":"' || i || '",'
                || '"name":"' || REPLACE(v_names(i), '"', '\"') || '",'
                || '"meta":{"class":"' || v_local_class || '","value":"' || v_nums(i) || '"}'
                || '}';

            DBMS_OUTPUT.PUT_LINE(v_local_msg);
            DBMS_OUTPUT.PUT_LINE(v_json_like);

            IF i = 2 THEN
                BEGIN
                    IF v_nums(i) < 0 THEN
                        RAISE e_custom;
                    ELSIF v_nums(i) BETWEEN 1 AND 100 THEN
                        NULL;
                    ELSE
                        RAISE VALUE_ERROR;
                    END IF;
                EXCEPTION
                    WHEN e_custom THEN
                        DBMS_OUTPUT.PUT_LINE('CUSTOM_ERROR');
                    WHEN VALUE_ERROR THEN
                        DBMS_OUTPUT.PUT_LINE('VALUE_ERROR');
                    WHEN OTHERS THEN
                        DBMS_OUTPUT.PUT_LINE('INNER_OTHERS:' || SQLERRM);
                END;
            END IF;

            FOR j IN 1 .. 2 LOOP
                BEGIN
                    v_idx := v_idx + 1;
                    DBMS_OUTPUT.PUT_LINE(
                           'LOOP_INFO='
                        || CASE
                               WHEN MOD(j, 2) = 0 THEN 'EVEN'
                               ELSE 'ODD'
                           END
                        || ',I=' || i
                        || ',J=' || j
                        || ',SEQ=' || v_idx
                    );
                EXCEPTION
                    WHEN OTHERS THEN
                        DBMS_OUTPUT.PUT_LINE('J_LOOP_ERR=' || SQLERRM);
                END;
            END LOOP;
        END inner_block;
    END LOOP outer_loop;

    v_flag :=
        CASE
            WHEN v_total > 200 THEN 'VERY_HIGH'
            WHEN v_total > 100 THEN 'HIGH'
            WHEN v_total > 50 THEN 'MID'
            ELSE 'LOW'
        END;

    DBMS_OUTPUT.PUT_LINE('TOTAL=' || v_total || ',FLAG=' || v_flag);

EXCEPTION
    WHEN OTHERS THEN
        DBMS_OUTPUT.PUT_LINE('OUTER_ERROR=' || SQLERRM);
        RAISE;
END;
/

/*==============================================================================
  UNIT 11
  Mixed expression stress: nested CASE + COALESCE + NULLIF + DECODE + subqueries
==============================================================================*/
WITH
    src AS
    (
        SELECT 1 AS id, 'X' AS category, 10 AS a_val, 20 AS b_val FROM dual
        UNION ALL
        SELECT 2, 'X', 30, 15 FROM dual
        UNION ALL
        SELECT 3, 'Y',  5,  5 FROM dual
        UNION ALL
        SELECT 4, 'Z', 40, 10 FROM dual
    )
SELECT
    s.id,
    s.category,
    CASE
        WHEN COALESCE(s.a_val, 0) > COALESCE(s.b_val, 0) THEN
            CASE
                WHEN NULLIF(s.a_val - s.b_val, 0) IS NOT NULL THEN
                    DECODE(
                        SIGN(s.a_val - s.b_val),
                        1, 'A_GT_B',
                        0, 'A_EQ_B',
                        -1, 'A_LT_B',
                        'UNKNOWN'
                    )
                ELSE 'ZERO_DELTA'
            END
        WHEN s.a_val = s.b_val THEN 'A_EQUALS_B'
        ELSE
            CASE
                WHEN s.b_val >
                     (
                         SELECT AVG(x.b_val)
                         FROM src x
                         WHERE x.category = s.category
                     )
                THEN 'B_ABOVE_CAT_AVG'
                ELSE 'B_NOT_ABOVE_CAT_AVG'
            END
    END AS relation_flag,
    (
        SELECT MAX(y.a_val + y.b_val)
        FROM src y
        WHERE y.category = s.category
    ) AS category_max_pair_sum
FROM src s
ORDER BY
    s.id
;
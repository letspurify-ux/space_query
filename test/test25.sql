/* =====================================================================
   ORACLE AUTO-FORMATTING FINAL BOSS
   목적:
     - Ctrl + Shift + F 자동 포맷팅 품질 극한 검증
     - 괄호/인덴트/중첩/CASE/CTE/PLSQL 블록 정렬 완전성 테스트
   주의:
     - 포맷팅 엔진 검증용 초고난도 스크립트
     - 일부 구문은 DB 버전/권한/객체 유무에 따라 실행보다 포맷팅 검증에 의미가 있음
   ===================================================================== */

--------------------------------------------------------------------------------
-- 1. WITH FUNCTION / PROCEDURE / 다단계 CTE / analytic / scalar subquery / EXISTS
--------------------------------------------------------------------------------
WITH
    FUNCTION fmt_mask (
        p_txt IN VARCHAR2
    ) RETURN VARCHAR2
    IS
    BEGIN
        RETURN REGEXP_REPLACE (
                   NVL (p_txt, 'NULL'),
                   '([[:alnum:]])',
                   '*'
               );
    END fmt_mask,
    PROCEDURE noop (
        p_msg IN VARCHAR2
    )
    IS
    BEGIN
        NULL;
    END noop,
    base_emp
    AS
        (
            SELECT
                e.empno,
                e.ename,
                e.job,
                e.mgr,
                e.hiredate,
                e.sal,
                e.comm,
                e.deptno,
                ROW_NUMBER () OVER (
                    PARTITION BY e.deptno
                    ORDER BY e.sal DESC, e.empno
                ) AS rn,
                DENSE_RANK () OVER (
                    PARTITION BY e.deptno
                    ORDER BY e.sal DESC
                ) AS dr,
                SUM (e.sal) OVER (
                    PARTITION BY e.deptno
                    ORDER BY e.hiredate, e.empno
                    ROWS BETWEEN UNBOUNDED PRECEDING AND CURRENT ROW
                ) AS running_sal,
                (
                    SELECT
                        MAX (x.sal)
                    FROM
                        emp x
                    WHERE
                            x.deptno = e.deptno
                        AND x.hiredate <= e.hiredate
                ) AS max_sal_until_now
            FROM
                emp e
            WHERE
                    e.sal > 0
                AND EXISTS
                        (
                            SELECT
                                1
                            FROM
                                dept d
                            WHERE
                                    d.deptno = e.deptno
                                AND d.loc IS NOT NULL
                        )
        ),
    filtered_emp
    AS
        (
            SELECT
                b.*,
                CASE
                    WHEN b.rn = 1 THEN 'TOP'
                    WHEN b.dr <= 3 THEN 'UPPER'
                    ELSE 'NORMAL'
                END AS band,
                CASE
                    WHEN b.comm IS NULL THEN
                        CASE
                            WHEN b.sal >= 3000 THEN 'NO_COMM_HIGH'
                            ELSE 'NO_COMM_LOW'
                        END
                    ELSE 'HAS_COMM'
                END AS comm_flag
            FROM
                base_emp b
            WHERE
                    (
                        b.sal >= 1000
                        AND b.deptno IN (
                            SELECT
                                d.deptno
                            FROM
                                dept d
                            WHERE
                                    d.loc IN ('NEW YORK', 'DALLAS', 'CHICAGO')
                                OR (
                                       d.loc LIKE 'B%'
                                   AND d.deptno BETWEEN 10 AND 99
                                   )
                        )
                    )
                OR (
                       b.job IN ('ANALYST', 'MANAGER')
                   AND NOT EXISTS
                           (
                               SELECT
                                   1
                               FROM
                                   emp z
                               WHERE
                                       z.mgr = b.empno
                                   AND z.sal > b.sal
                           )
                   )
        ),
    dept_stat
    AS
        (
            SELECT
                f.deptno,
                COUNT (*) AS cnt_emp,
                MIN (f.sal) AS min_sal,
                MAX (f.sal) AS max_sal,
                AVG (f.sal) AS avg_sal,
                SUM (
                    CASE
                        WHEN f.band = 'TOP' THEN 1
                        ELSE 0
                    END
                ) AS cnt_top,
                LISTAGG (
                    f.ename,
                    ', '
                ) WITHIN GROUP (
                    ORDER BY f.sal DESC, f.empno
                ) AS emp_list
            FROM
                filtered_emp f
            GROUP BY
                f.deptno
        )
SELECT
    f.deptno,
    f.empno,
    f.ename,
    f.job,
    f.band,
    f.comm_flag,
    f.running_sal,
    d.cnt_emp,
    d.avg_sal,
    (
        SELECT
            COUNT (*)
        FROM
            emp c
        WHERE
                c.deptno = f.deptno
            AND c.sal > f.sal
    ) AS cnt_higher_same_dept,
    fmt_mask (f.ename) AS masked_name,
    CASE
        WHEN f.sal > d.avg_sal THEN 'ABOVE_AVG'
        WHEN f.sal = d.avg_sal THEN 'AT_AVG'
        ELSE 'BELOW_AVG'
    END AS avg_cmp
FROM
    filtered_emp f
    JOIN dept_stat d
        ON d.deptno = f.deptno
ORDER BY
    f.deptno,
    CASE
        WHEN f.band = 'TOP' THEN 1
        WHEN f.band = 'UPPER' THEN 2
        ELSE 3
    END,
    f.sal DESC,
    f.empno
;

--------------------------------------------------------------------------------
-- 2. 복합 괄호 / CASE inside CASE / DECODE / NVL / COALESCE / NULLIF / scalar select
--------------------------------------------------------------------------------
SELECT
    e.empno,
    e.ename,
    CASE
        WHEN (
                 e.sal > 2000
                 AND (
                         e.comm IS NOT NULL
                         OR e.job IN (
                             'SALESMAN',
                             'MANAGER',
                             'ANALYST'
                         )
                     )
             ) THEN
            CASE
                WHEN e.deptno = 10 THEN 'A'
                WHEN e.deptno = 20 THEN
                    CASE
                        WHEN e.sal > 3000 THEN 'B1'
                        ELSE 'B2'
                    END
                ELSE 'C'
            END
        ELSE
            DECODE (
                SIGN (NVL (e.sal, 0) - 1500),
                -1, 'LOW',
                0, 'MID',
                1, COALESCE (e.job, 'UNKNOWN'),
                'ETC'
            )
    END AS complex_flag,
    NVL (
        TO_CHAR (
            (
                SELECT
                    MAX (x.hiredate)
                FROM
                    emp x
                WHERE
                    x.mgr = e.empno
            ),
            'YYYY-MM-DD'
        ),
        'NO_SUBORDINATE'
    ) AS max_sub_hiredate,
    COALESCE (
        TO_CHAR (e.comm),
        TO_CHAR (
            NULLIF (
                (
                    SELECT
                        COUNT (*)
                    FROM
                        emp y
                    WHERE
                            y.deptno = e.deptno
                        AND y.sal > e.sal
                ),
                0
            )
        ),
        'NONE'
    ) AS comm_or_rankinfo
FROM
    emp e
WHERE
        (
            e.ename LIKE 'A%'
            OR e.ename LIKE 'S%'
        )
    AND (
            e.sal BETWEEN 800 AND 5000
            OR e.job = 'PRESIDENT'
        )
ORDER BY
    e.deptno,
    e.sal DESC,
    e.empno
;

--------------------------------------------------------------------------------
-- 3. 계층형 쿼리 / CONNECT BY / SYS_CONNECT_BY_PATH / CONNECT_BY_ROOT / ORDER SIBLINGS
--------------------------------------------------------------------------------
SELECT
    LEVEL AS lvl,
    CONNECT_BY_ROOT e.empno AS root_empno,
    CONNECT_BY_ROOT e.ename AS root_ename,
    e.empno,
    e.ename,
    e.mgr,
    SYS_CONNECT_BY_PATH (e.ename, ' > ') AS full_path,
    CONNECT_BY_ISLEAF AS is_leaf,
    CONNECT_BY_ISCYCLE AS is_cycle
FROM
    emp e
START WITH
    e.mgr IS NULL
CONNECT BY NOCYCLE
    PRIOR e.empno = e.mgr
ORDER SIBLINGS BY
    e.sal DESC,
    e.empno
;

--------------------------------------------------------------------------------
-- 4. PIVOT / UNPIVOT / nested inline view / alias depth
--------------------------------------------------------------------------------
SELECT
    pvt.deptno,
    pvt."CLERK"     AS clerk_cnt,
    pvt."MANAGER"   AS manager_cnt,
    pvt."ANALYST"   AS analyst_cnt,
    pvt."SALESMAN"  AS salesman_cnt,
    pvt."PRESIDENT" AS president_cnt
FROM
    (
        SELECT
            e.deptno,
            e.job
        FROM
            emp e
    )
    PIVOT
    (
        COUNT (*)
        FOR job IN (
            'CLERK'     AS "CLERK",
            'MANAGER'   AS "MANAGER",
            'ANALYST'   AS "ANALYST",
            'SALESMAN'  AS "SALESMAN",
            'PRESIDENT' AS "PRESIDENT"
        )
    ) pvt
ORDER BY
    pvt.deptno
;

SELECT
    u.empno,
    u.metric_name,
    u.metric_value
FROM
    (
        SELECT
            e.empno,
            e.sal,
            NVL (e.comm, 0) AS comm,
            e.deptno
        FROM
            emp e
    )
    UNPIVOT INCLUDE NULLS
    (
        metric_value
        FOR metric_name IN (
            sal    AS 'SAL',
            comm   AS 'COMM',
            deptno AS 'DEPTNO'
        )
    ) u
ORDER BY
    u.empno,
    u.metric_name
;

--------------------------------------------------------------------------------
-- 5. MODEL 절 / 복잡한 RULES / 참조 셀 계산
--------------------------------------------------------------------------------
SELECT
    deptno,
    empno,
    calc_sal,
    calc_bonus
FROM
    (
        SELECT
            deptno,
            empno,
            sal AS calc_sal,
            NVL (comm, 0) AS calc_bonus
        FROM
            emp
        WHERE
            deptno IN (10, 20, 30)
    )
    MODEL
        PARTITION BY (deptno)
        DIMENSION BY (ROW_NUMBER () OVER (PARTITION BY deptno ORDER BY empno) AS seq, empno)
        MEASURES (calc_sal, calc_bonus)
        RULES UPSERT SEQUENTIAL ORDER
        (
            calc_bonus[ANY, ANY] =
                CASE
                    WHEN calc_sal[CV (), CV (empno)] > 3000 THEN calc_sal[CV (), CV (empno)] * 0.20
                    WHEN calc_sal[CV (), CV (empno)] > 1500 THEN calc_sal[CV (), CV (empno)] * 0.10
                    ELSE calc_sal[CV (), CV (empno)] * 0.05
                END,
            calc_sal[ANY, ANY] =
                calc_sal[CV (), CV (empno)]
                + NVL (calc_bonus[CV (), CV (empno)], 0)
        )
ORDER BY
    deptno,
    empno
;

--------------------------------------------------------------------------------
-- 6. XMLTABLE / JSON_TABLE / 깊은 함수 괄호 정렬
--------------------------------------------------------------------------------
SELECT
    x.emp_id,
    x.emp_name,
    x.dept_id
FROM
    XMLTABLE (
        '/rows/row'
        PASSING XMLTYPE (
            '<rows>
                <row><emp_id>100</emp_id><emp_name>ALICE</emp_name><dept_id>10</dept_id></row>
                <row><emp_id>200</emp_id><emp_name>BOB</emp_name><dept_id>20</dept_id></row>
             </rows>'
        )
        COLUMNS
            emp_id   NUMBER        PATH 'emp_id',
            emp_name VARCHAR2(100) PATH 'emp_name',
            dept_id  NUMBER        PATH 'dept_id'
    ) x
ORDER BY
    x.emp_id
;

SELECT
    j.emp_id,
    j.emp_name,
    j.role_name,
    j.salary
FROM
    JSON_TABLE (
        '{
           "employees": [
             { "id": 1, "name": "ALICE", "role": "DEV", "salary": 3000 },
             { "id": 2, "name": "BOB",   "role": "DBA", "salary": 4200 }
           ]
         }',
        '$.employees[*]'
        COLUMNS
            emp_id    NUMBER        PATH '$.id',
            emp_name  VARCHAR2(100) PATH '$.name',
            role_name VARCHAR2(100) PATH '$.role',
            salary    NUMBER        PATH '$.salary'
    ) j
WHERE
    j.salary >= 3000
ORDER BY
    j.emp_id
;

--------------------------------------------------------------------------------
-- 7. set operator / UNION ALL / INTERSECT / MINUS / nested ORDER BY
--------------------------------------------------------------------------------
SELECT
    z.src,
    z.empno,
    z.ename
FROM
    (
        SELECT
            'A' AS src,
            e.empno,
            e.ename
        FROM
            emp e
        WHERE
            e.deptno = 10

        UNION ALL

        SELECT
            'B' AS src,
            e.empno,
            e.ename
        FROM
            emp e
        WHERE
            e.job = 'ANALYST'

        INTERSECT

        SELECT
            'B' AS src,
            e.empno,
            e.ename
        FROM
            emp e
        WHERE
            e.sal >= 2000

        MINUS

        SELECT
            'B' AS src,
            e.empno,
            e.ename
        FROM
            emp e
        WHERE
            e.ename LIKE 'S%'
    ) z
ORDER BY
    z.src,
    z.empno
;

--------------------------------------------------------------------------------
-- 8. MERGE / 복잡한 ON / UPDATE CASE / DELETE WHERE / INSERT VALUES
--------------------------------------------------------------------------------
MERGE INTO emp_bonus b
USING
(
    SELECT
        e.empno,
        e.deptno,
        e.sal,
        CASE
            WHEN e.sal >= 4000 THEN ROUND (e.sal * 0.20, 2)
            WHEN e.sal >= 2500 THEN ROUND (e.sal * 0.10, 2)
            ELSE ROUND (e.sal * 0.05, 2)
        END AS calc_bonus
    FROM
        emp e
    WHERE
            e.deptno IN (10, 20, 30)
        AND EXISTS
                (
                    SELECT
                        1
                    FROM
                        dept d
                    WHERE
                            d.deptno = e.deptno
                        AND d.loc IS NOT NULL
                )
) s
ON
(
        b.empno = s.empno
    AND (
            b.deptno = s.deptno
            OR (
                   b.deptno IS NULL
               AND s.deptno IS NOT NULL
               )
        )
)
WHEN MATCHED THEN
    UPDATE
       SET b.bonus_amount =
               CASE
                   WHEN s.calc_bonus > 1000 THEN s.calc_bonus
                   ELSE s.calc_bonus + 100
               END,
           b.updated_at   = SYSTIMESTAMP,
           b.note_text    =
               'UPDATED:' || TO_CHAR (SYSTIMESTAMP, 'YYYY-MM-DD HH24:MI:SS')
     WHERE
             s.sal > 0
         AND (
                 b.bonus_amount IS NULL
                 OR b.bonus_amount <> s.calc_bonus
             )
    DELETE
     WHERE
         s.sal < 500
WHEN NOT MATCHED THEN
    INSERT
    (
        b.empno,
        b.deptno,
        b.bonus_amount,
        b.created_at,
        b.note_text
    )
    VALUES
    (
        s.empno,
        s.deptno,
        s.calc_bonus,
        SYSTIMESTAMP,
        CASE
            WHEN s.calc_bonus >= 500 THEN 'HIGH'
            ELSE 'LOW'
        END
    )
;

--------------------------------------------------------------------------------
-- 9. INSERT ALL / FIRST / 서브쿼리 조건 분기
--------------------------------------------------------------------------------
INSERT ALL
    WHEN sal >= 4000 THEN
        INTO emp_grade_high (
            empno,
            ename,
            sal,
            deptno,
            grade_text
        )
        VALUES (
            empno,
            ename,
            sal,
            deptno,
            'HIGH'
        )
    WHEN sal >= 2000 THEN
        INTO emp_grade_mid (
            empno,
            ename,
            sal,
            deptno,
            grade_text
        )
        VALUES (
            empno,
            ename,
            sal,
            deptno,
            'MID'
        )
    ELSE
        INTO emp_grade_low (
            empno,
            ename,
            sal,
            deptno,
            grade_text
        )
        VALUES (
            empno,
            ename,
            sal,
            deptno,
            'LOW'
        )
SELECT
    e.empno,
    e.ename,
    e.sal,
    e.deptno
FROM
    emp e
WHERE
    e.deptno IN (
        SELECT
            d.deptno
        FROM
            dept d
        WHERE
            d.loc IS NOT NULL
    )
;

INSERT FIRST
    WHEN deptno = 10 THEN
        INTO emp_bucket_a (empno, ename, deptno) VALUES (empno, ename, deptno)
    WHEN deptno = 20 THEN
        INTO emp_bucket_b (empno, ename, deptno) VALUES (empno, ename, deptno)
    ELSE
        INTO emp_bucket_c (empno, ename, deptno) VALUES (empno, ename, deptno)
SELECT
    e.empno,
    e.ename,
    e.deptno
FROM
    emp e
;

--------------------------------------------------------------------------------
-- 10. UPDATE / correlated subquery / EXISTS / nested CASE
--------------------------------------------------------------------------------
UPDATE emp e
   SET e.comm =
           CASE
               WHEN e.comm IS NULL THEN
                   (
                       SELECT
                           CASE
                               WHEN AVG (x.sal) > 3000 THEN 300
                               WHEN AVG (x.sal) > 2000 THEN 200
                               ELSE 100
                           END
                       FROM
                           emp x
                       WHERE
                           x.deptno = e.deptno
                   )
               ELSE e.comm + 10
           END,
       e.job  =
           CASE
               WHEN e.job = 'CLERK' AND e.sal > 1500 THEN 'SENIOR_CLERK'
               ELSE e.job
           END
 WHERE EXISTS
           (
               SELECT
                   1
               FROM
                   dept d
               WHERE
                       d.deptno = e.deptno
                   AND d.loc IS NOT NULL
           )
;

--------------------------------------------------------------------------------
-- 11. DELETE / nested EXISTS / NOT EXISTS / IN with subquery
--------------------------------------------------------------------------------
DELETE FROM emp_bonus b
 WHERE EXISTS
           (
               SELECT
                   1
               FROM
                   emp e
               WHERE
                       e.empno = b.empno
                   AND e.deptno IN (
                       SELECT
                           d.deptno
                       FROM
                           dept d
                       WHERE
                               d.loc IS NOT NULL
                           AND NOT EXISTS
                                   (
                                       SELECT
                                           1
                                       FROM
                                           dept x
                                       WHERE
                                               x.deptno = d.deptno
                                           AND x.loc LIKE 'X%'
                                   )
                   )
           )
   AND NOT EXISTS
           (
               SELECT
                   1
               FROM
                   emp_keep_list k
               WHERE
                   k.empno = b.empno
           )
;

--------------------------------------------------------------------------------
-- 12. WITH + recursive-like self join style / lateral-ish nested scalar views
--------------------------------------------------------------------------------
WITH
    t1
    AS
        (
            SELECT
                e.empno,
                e.ename,
                e.mgr,
                e.deptno,
                1 AS lvl
            FROM
                emp e
            WHERE
                e.mgr IS NULL
        ),
    t2
    AS
        (
            SELECT
                c.empno,
                c.ename,
                c.mgr,
                c.deptno,
                p.lvl + 1 AS lvl
            FROM
                emp c
                JOIN t1 p
                    ON p.empno = c.mgr
        )
SELECT
    q.empno,
    q.ename,
    q.lvl,
    (
        SELECT
            LISTAGG (x.ename, ' / ') WITHIN GROUP (ORDER BY x.empno)
        FROM
            emp x
        WHERE
            x.mgr = q.empno
    ) AS children_names,
    (
        SELECT
            MAX (y.sal)
        FROM
            emp y
        WHERE
            y.deptno = q.deptno
    ) AS dept_max_sal
FROM
    (
        SELECT * FROM t1
        UNION ALL
        SELECT * FROM t2
    ) q
ORDER BY
    q.lvl,
    q.empno
;

--------------------------------------------------------------------------------
-- 13. PL/SQL DECLARE block / record / collection / cursor / bulk collect / forall
--------------------------------------------------------------------------------
DECLARE
    TYPE t_emp_rec IS RECORD
    (
        empno  emp.empno%TYPE,
        ename  emp.ename%TYPE,
        deptno emp.deptno%TYPE,
        sal    emp.sal%TYPE
    );

    TYPE t_emp_tab IS TABLE OF t_emp_rec INDEX BY PLS_INTEGER;

    v_tab           t_emp_tab;
    v_sql           VARCHAR2 (32767);
    v_cnt           PLS_INTEGER := 0;
    v_dummy         VARCHAR2 (4000);
    v_max_sal       NUMBER;
    v_min_sal       NUMBER;

    CURSOR c_emp (
        p_deptno IN NUMBER
    )
    IS
        SELECT
            e.empno,
            e.ename,
            e.deptno,
            e.sal
        FROM
            emp e
        WHERE
                e.deptno = p_deptno
            AND e.sal >= (
                SELECT
                    AVG (x.sal)
                FROM
                    emp x
                WHERE
                    x.deptno = e.deptno
            )
        ORDER BY
            e.sal DESC,
            e.empno;

    PROCEDURE log_line (
        p_msg IN VARCHAR2
    )
    IS
    BEGIN
        DBMS_OUTPUT.PUT_LINE (p_msg);
    EXCEPTION
        WHEN OTHERS THEN
            NULL;
    END log_line;

    FUNCTION classify_emp (
        p_sal   IN NUMBER,
        p_comm  IN NUMBER
    ) RETURN VARCHAR2
    IS
    BEGIN
        RETURN CASE
                   WHEN p_sal >= 4000 AND NVL (p_comm, 0) > 0 THEN 'TOP_PLUS_COMM'
                   WHEN p_sal >= 3000 THEN 'TOP'
                   WHEN p_sal >= 2000 THEN 'MID'
                   ELSE 'LOW'
               END;
    END classify_emp;
BEGIN
    v_sql :=
           'SELECT MAX(sal), MIN(sal) '
        || 'FROM emp '
        || 'WHERE deptno IN (SELECT deptno FROM dept WHERE loc IS NOT NULL)';

    EXECUTE IMMEDIATE v_sql
        INTO v_max_sal, v_min_sal;

    log_line (
           'MAX='
        || TO_CHAR (v_max_sal)
        || ', MIN='
        || TO_CHAR (v_min_sal)
    );

    OPEN c_emp (10);

    LOOP
        FETCH c_emp
        BULK COLLECT INTO v_tab LIMIT 5;

        EXIT WHEN v_tab.COUNT = 0;

        FOR i IN 1 .. v_tab.COUNT
        LOOP
            v_cnt := v_cnt + 1;

            log_line (
                   LPAD (' ', MOD (i, 3) * 2)
                || '['
                || TO_CHAR (v_cnt)
                || '] EMPNO='
                || v_tab (i).empno
                || ', ENAME='
                || v_tab (i).ename
                || ', CLASS='
                || classify_emp (
                       v_tab (i).sal,
                       NULL
                   )
            );
        END LOOP;
    END LOOP;

    CLOSE c_emp;

    BEGIN
        FORALL idx IN INDICES OF v_tab
            INSERT INTO emp_audit_log (
                log_id,
                empno,
                deptno,
                log_text,
                created_at
            )
            VALUES (
                emp_audit_seq.NEXTVAL,
                v_tab (idx).empno,
                v_tab (idx).deptno,
                'AUDIT-' || v_tab (idx).ename,
                SYSTIMESTAMP
            );
    EXCEPTION
        WHEN DUP_VAL_ON_INDEX THEN
            log_line ('DUP_VAL_ON_INDEX');
        WHEN OTHERS THEN
            log_line ('FORALL ERROR: ' || SQLERRM);
    END;

    <<final_check>>
    BEGIN
        SELECT
            CASE
                WHEN COUNT (*) > 0 THEN 'OK'
                ELSE 'EMPTY'
            END
        INTO v_dummy
        FROM
            emp
        WHERE
            deptno = 10;

        IF v_dummy = 'OK' THEN
            NULL;
        ELSIF v_dummy = 'EMPTY' THEN
            NULL;
        ELSE
            GOTO final_label;
        END IF;
    END;

    <<final_label>>
    NULL;
EXCEPTION
    WHEN NO_DATA_FOUND THEN
        DBMS_OUTPUT.PUT_LINE ('NO_DATA_FOUND');
    WHEN TOO_MANY_ROWS THEN
        DBMS_OUTPUT.PUT_LINE ('TOO_MANY_ROWS');
    WHEN OTHERS THEN
        DBMS_OUTPUT.PUT_LINE (
               'ERROR='
            || SQLERRM
            || CHR (10)
            || DBMS_UTILITY.FORMAT_ERROR_BACKTRACE
        );
END;
/

--------------------------------------------------------------------------------
-- 14. anonymous block with nested block / cursor expression / dynamic SQL / q-quote
--------------------------------------------------------------------------------
DECLARE
    v_stmt      CLOB;
    v_result    NUMBER;
    v_text      VARCHAR2 (4000);
    v_deptno    NUMBER := 20;

    TYPE t_num_tab IS TABLE OF NUMBER;
    v_nums      t_num_tab := t_num_tab (10, 20, 30);

    CURSOR c_mix
    IS
        SELECT
            d.deptno,
            CURSOR (
                SELECT
                    e.empno,
                    e.ename,
                    e.sal
                FROM
                    emp e
                WHERE
                    e.deptno = d.deptno
                ORDER BY
                    e.sal DESC,
                    e.empno
            ) AS emp_cur
        FROM
            dept d
        WHERE
            d.deptno MEMBER OF v_nums;
BEGIN
    v_stmt := q'[
        SELECT
            COUNT(*)
        FROM
            emp e
        WHERE
                e.deptno = :b1
            AND EXISTS
                    (
                        SELECT
                            1
                        FROM
                            dept d
                        WHERE
                                d.deptno = e.deptno
                            AND d.loc IS NOT NULL
                    )
    ]';

    EXECUTE IMMEDIATE v_stmt
        INTO v_result
        USING v_deptno;

    v_text :=
           CASE
               WHEN v_result > 0 THEN
                   q'[FOUND_ROWS]'
               ELSE
                   q'[NO_ROWS]'
           END
        || ' / CNT='
        || TO_CHAR (v_result);

    DBMS_OUTPUT.PUT_LINE (v_text);

    FOR r IN c_mix
    LOOP
        DBMS_OUTPUT.PUT_LINE ('DEPT=' || r.deptno);
    END LOOP;

    BEGIN
        EXECUTE IMMEDIATE q'[
            UPDATE emp
               SET sal = sal
             WHERE deptno = :x
               AND empno IN (
                       SELECT
                           z.empno
                       FROM
                           emp z
                       WHERE
                               z.deptno = :x
                           AND z.sal >= (
                               SELECT
                                   AVG(y.sal)
                               FROM
                                   emp y
                               WHERE
                                   y.deptno = z.deptno
                           )
                   )
        ]'
        USING v_deptno, v_deptno;
    EXCEPTION
        WHEN OTHERS THEN
            DBMS_OUTPUT.PUT_LINE ('UPDATE ERROR=' || SQLERRM);
    END;
END;
/

--------------------------------------------------------------------------------
-- 15. CREATE VIEW with nested SELECT / CASE / aggregation / outer join style
--------------------------------------------------------------------------------
CREATE OR REPLACE VIEW v_emp_formatter_boss
AS
    SELECT
        d.deptno,
        d.dname,
        d.loc,
        COUNT (e.empno) AS emp_cnt,
        SUM (NVL (e.sal, 0)) AS total_sal,
        AVG (NVL (e.sal, 0)) AS avg_sal,
        MAX (
            CASE
                WHEN e.sal = (
                    SELECT
                        MAX (x.sal)
                    FROM
                        emp x
                    WHERE
                        x.deptno = d.deptno
                ) THEN e.ename
                ELSE NULL
            END
        ) AS top_name,
        LISTAGG (
            CASE
                WHEN e.job IN ('MANAGER', 'ANALYST') THEN
                    '[' || e.job || ':' || e.ename || ']'
                ELSE
                    e.ename
            END,
            ', '
        ) WITHIN GROUP (
            ORDER BY
                CASE
                    WHEN e.sal IS NULL THEN 999999999
                    ELSE e.sal * -1
                END,
                e.empno
        ) AS emp_desc
    FROM
        dept d
        LEFT JOIN emp e
            ON e.deptno = d.deptno
    GROUP BY
        d.deptno,
        d.dname,
        d.loc
;

--------------------------------------------------------------------------------
-- 16. 최종 복합 SELECT: view + subquery factoring + having + ordered case
--------------------------------------------------------------------------------
WITH
    s
    AS
        (
            SELECT
                v.deptno,
                v.dname,
                v.loc,
                v.emp_cnt,
                v.total_sal,
                v.avg_sal,
                v.top_name,
                CASE
                    WHEN v.emp_cnt >= 5 AND v.avg_sal >= 2500 THEN 'BIG_HIGH'
                    WHEN v.emp_cnt >= 3 THEN 'MID'
                    ELSE 'SMALL'
                END AS dept_class
            FROM
                v_emp_formatter_boss v
        )
SELECT
    s.deptno,
    s.dname,
    s.loc,
    s.emp_cnt,
    s.total_sal,
    s.avg_sal,
    s.top_name,
    s.dept_class,
    (
        SELECT
            COUNT (*)
        FROM
            emp e
        WHERE
                e.deptno = s.deptno
            AND e.sal > s.avg_sal
    ) AS cnt_above_avg,
    (
        SELECT
            LISTAGG (x.job, ', ') WITHIN GROUP (ORDER BY x.job)
        FROM
            (
                SELECT DISTINCT
                    e.job
                FROM
                    emp e
                WHERE
                    e.deptno = s.deptno
            ) x
    ) AS jobs_in_dept
FROM
    s
GROUP BY
    s.deptno,
    s.dname,
    s.loc,
    s.emp_cnt,
    s.total_sal,
    s.avg_sal,
    s.top_name,
    s.dept_class
HAVING
    SUM (
        CASE
            WHEN s.emp_cnt > 0 THEN 1
            ELSE 0
        END
    ) > 0
ORDER BY
    CASE
        WHEN s.dept_class = 'BIG_HIGH' THEN 1
        WHEN s.dept_class = 'MID' THEN 2
        ELSE 3
    END,
    s.avg_sal DESC,
    s.deptno
;

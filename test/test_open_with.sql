--------------------------------------------------------------------------------
-- test_open_with.sql
-- OPEN p_rc FOR WITH ... 복잡 구문 10종 depth 테스트
-- 각 쿼리에 가상 주석(-- / /* */) 삽입 포인트를 표기
--------------------------------------------------------------------------------

CREATE OR REPLACE PROCEDURE test_open_with_proc IS
    p_rc SYS_REFCURSOR;
BEGIN
    --------------------------------------------------------------------------
    -- [Q1] WITH 기본 CTE + 스칼라 서브쿼리 + LEFT JOIN
    --      depth: SUM() 안 depth1, 스칼라서브쿼리 depth1, ON 조건 depth2
    --------------------------------------------------------------------------
    OPEN p_rc FOR
        -- [CW1] 기본 CTE
        WITH /* A: dept 집계 CTE */
        dept_stats AS (
            SELECT /* B: dept 집계 */
                deptno,
                COUNT(*) AS cnt,
                AVG(sal) AS avg_sal,
                SUM (NVL (comm, /* C: NULL→0 */
                        0)) AS sum_comm
            FROM emp
            GROUP BY deptno
        )
        SELECT d.deptno,
            d.dname,
            -- [D] 스칼라 서브쿼리 시작
            (
                /* E: correlated max */
                SELECT MAX(e2.sal)
                FROM emp e2
                WHERE e2.deptno = d.deptno -- [F] correlated
            ) AS max_sal,
            ds.cnt,
            ROUND (ds.avg_sal, 2) AS avg_sal
        FROM dept d
        LEFT JOIN dept_stats ds
            ON /* G: join 조건 */
            ds.deptno = d.deptno
        ORDER BY d.deptno;
    --------------------------------------------------------------------------
    -- [Q2] WITH 다중 CTE + 분석 함수 + GROUP BY CUBE
    --      depth: OVER() 안 depth1, CUBE() depth1, GROUPING_ID() depth1
    --------------------------------------------------------------------------
    OPEN p_rc FOR
        WITH /* H: 다중 CTE */
        base AS (
            SELECT e.empno,
                e.ename,
                e.job,
                e.deptno,
                e.sal,
                -- [I] 분석 RANK
                RANK () OVER (
                    /* J: partition spec */
                    PARTITION BY e.deptno
                    ORDER BY e.sal DESC
                ) AS rnk
            FROM emp e
        ),
        grp_base AS ( -- [K] 집계 CTE
            SELECT deptno,
                job,
                COUNT(*) AS cnt,
                SUM (
                    /* L: 1등 급여만 합산 */
                    CASE
                        WHEN rnk = 1 THEN sal
                        ELSE 0
                    END
                ) AS top_sal
            FROM base
            GROUP BY deptno,
                job
        )
        SELECT deptno,
            job,
            cnt,
            top_sal,
            /* M: GROUPING_ID */
            GROUPING_ID (deptno, job) AS gid
        FROM grp_base
        GROUP BY CUBE (deptno /* N: cube 1 */, job /* O: cube 2 */)
        ORDER BY gid,
            deptno NULLS LAST,
            job NULLS LAST;
    --------------------------------------------------------------------------
    -- [Q3] WITH + 인라인 뷰 안 중첩 WITH + EXISTS / NOT EXISTS
    --      depth: 외부 FROM( depth1, 내부 WITH x AS( depth2,
    --             EXISTS( depth1, NOT EXISTS( depth1
    --------------------------------------------------------------------------
    OPEN p_rc FOR
        WITH paid AS (
            SELECT oh.order_id,
                oh.cust_name,
                oh.order_dt
            FROM order_hdr oh
            WHERE oh.status = 'PAID' -- [P] paid filter
        ),
        amounts AS (
            SELECT oi.order_id,
                SUM (oi.qty * oi.unit_price) AS amt
            FROM order_item oi
            GROUP BY oi.order_id
        )
        SELECT *
        FROM (
                /* Q: 인라인뷰 시작 */
                WITH x AS (
                    SELECT p.order_id,
                        p.cust_name,
                        p.order_dt,
                        a.amt
                    FROM paid p
                    JOIN amounts a
                        ON /* R: join key */
                        a.order_id = p.order_id
                    WHERE a.amt > /* S: threshold */
                            50
                )
                SELECT x.*,
                    (
                        -- [T] 라인수 서브쿼리
                        SELECT COUNT(*)
                        FROM order_item oi
                        WHERE oi.order_id = x.order_id
                    ) AS line_cnt
                FROM x
            ) v
        WHERE EXISTS (
                /* U: SKU 존재 조건 */
                SELECT 1
                FROM order_item oi
                WHERE oi.order_id = v.order_id
                    AND oi.sku LIKE 'SKU-%' -- [V] SKU 패턴
            )
            AND NOT EXISTS (
                -- [W] 음수 수량 배제
                SELECT 1
                FROM order_item oi
                WHERE oi.order_id = v.order_id
                    AND oi.qty <= /* X: 0 이하 */
                        0
            )
        ORDER BY v.amt DESC;
    --------------------------------------------------------------------------
    -- [Q4] WITH 재귀 CTE (UNION ALL) + CAST + CASE in DEFINE
    --      depth: r(...) AS( depth1, SELECT depth1 컬럼 함수들 depth2,
    --             UNION ALL 이후 SELECT depth1, JOIN ON depth1
    --------------------------------------------------------------------------
    OPEN p_rc FOR
        WITH r (node_id, parent_id, node_name, lvl, PATH, cycle_flag) AS (
            -- [Y] anchor member
            SELECT node_id,
                parent_id,
                node_name,
                1 AS lvl,
                CAST ( /* Z: 초기 경로 */
                    node_name AS VARCHAR2 (4000)) AS PATH,
                0 AS cycle_flag
            FROM tree_nodes
            WHERE parent_id IS NULL
            UNION ALL
            -- [AA] recursive member
            SELECT t.node_id,
                t.parent_id,
                t.node_name,
                r.lvl + 1,
                r.PATH || '/' || t.node_name,
                /* AB: 사이클 감지 */
                CASE
                    WHEN INSTR (
                            /* AC: 경로 검색 */
                            r.PATH, t.node_name) > 0 THEN 1
                    ELSE 0
                END AS cycle_flag
            FROM tree_nodes t
            JOIN r
                ON /* AD: 부모-자식 */
                t.parent_id = r.node_id
            WHERE r.cycle_flag = /* AE: 사이클 없는것만 */
                    0
        )
        SELECT node_id,
            LPAD (' ', (lvl - 1) * 2) || node_name AS tree_display,
            lvl,
            PATH
        FROM r
        WHERE cycle_flag = 0
        ORDER BY lvl,
            node_id;
    --------------------------------------------------------------------------
    -- [Q5] WITH + PIVOT + UNPIVOT (이중 CTE)
    --      depth: PIVOT( depth1, FOR IN( depth2, SUM() depth2,
    --             UNPIVOT( depth1, FOR IN( depth2
    --------------------------------------------------------------------------
    OPEN p_rc FOR
        WITH src AS (
            SELECT deptno,
                job,
                sal
            FROM emp
        ),
        pivoted AS (
            SELECT *
            FROM src
            PIVOT (
                /* AF: SUM 집계 */
                SUM (sal) AS sum_sal
                FOR deptno IN (
                    /* AG: 피벗 값 목록 */
                    10 AS D10, 20 AS D20, 30 AS D30)
            ) -- [AH] pivot 끝
        )
        SELECT job,
            dept_tag,
            sal_amt
        FROM pivoted
        UNPIVOT (
            /* AI: UNPIVOT 컬럼 */
            sal_amt
            FOR dept_tag IN (
                -- [AJ] unpivot 대상
                D10 AS '10', D20 AS '20', D30 AS '30')
        )
        WHERE sal_amt IS NOT NULL -- [AK] 필터
        ORDER BY job,
            dept_tag;
    --------------------------------------------------------------------------
    -- [Q6] WITH + JSON_TABLE + NESTED PATH (이중 COLUMNS depth3)
    --      depth: JSON_TABLE( depth1, outer COLUMNS( depth2,
    --             NESTED PATH COLUMNS( depth3
    --------------------------------------------------------------------------
    OPEN p_rc FOR
        WITH jdocs AS (
            SELECT id,
                payload
            FROM json_docs
            WHERE /* AL: 활성 문서만 */
                active_flag = 1
        )
        SELECT jd.id,
            /* AM: JSON 파싱 결과 */
            jt.order_id,
            jt.cust_name,
            jt.tier,
            it.sku,
            it.qty,
            it.price,
            (it.qty * it.price) AS line_amt
        FROM jdocs jd
        CROSS JOIN JSON_TABLE (jd.payload,
            /* AN: root path */
            '$' COLUMNS (
                -- [AO] 최상위 컬럼
                order_id NUMBER PATH '$.order_id',
                cust_name VARCHAR2 (100) PATH '$.customer.name',
                tier VARCHAR2 (20) PATH '$.customer.tier',
                NESTED PATH '$.items[*]' COLUMNS (
                    /* AP: 아이템 컬럼 */
                    sku VARCHAR2 (30) PATH '$.sku',
                    qty NUMBER PATH '$.qty',
                    price NUMBER PATH '$.price'
                ) -- [AQ] nested columns 끝
            ) -- [AR] outer columns 끝
        ) jt
        CROSS APPLY (
            -- [AS] item alias
            SELECT jt.sku,
                jt.qty,
                jt.price
            FROM DUAL
        ) it
        ORDER BY jd.id,
            it.sku;
    --------------------------------------------------------------------------
    -- [Q7] WITH + OUTER APPLY + NTILE (윈도우 함수 중첩)
    --      depth: OUTER APPLY( depth1, 내부 SELECT depth1,
    --             NTILE() OVER() depth0, OVER( depth1
    --------------------------------------------------------------------------
    OPEN p_rc FOR
        WITH recent_orders AS (
            SELECT order_id,
                cust_name,
                order_dt
            FROM order_hdr
            WHERE order_dt >= SYSDATE - /* AT: 기간 */
                    90
        )
        SELECT o.order_id,
            o.cust_name,
            o.order_dt,
            x.item_cnt,
            x.total_amt,
            x.max_price,
            -- [AU] 분위수 계산
            NTILE (4) OVER (
                /* AV: 금액 기준 분위 */
                ORDER BY x.total_amt DESC NULLS LAST
            ) AS amt_quartile,
            SUM (x.total_amt) OVER (
                PARTITION BY /* AW: 고객별 누적 */
                o.cust_name
                ORDER BY o.order_dt
                ROWS BETWEEN UNBOUNDED PRECEDING AND CURRENT ROW
            ) AS running_total
        FROM recent_orders o
        OUTER APPLY (
            /* AX: 주문별 집계 */
            SELECT COUNT(*) AS item_cnt,
                SUM (qty * unit_price) AS total_amt,
                MAX(unit_price) AS max_price
            FROM order_item oi
            WHERE oi.order_id = /* AY: correlated */
                    o.order_id
        ) x
        ORDER BY o.order_id;
    --------------------------------------------------------------------------
    -- [Q8] WITH + 3단 중첩 CASE + scalar subquery in ORDER BY
    --      depth: base CTE( depth1, enriched CASE depth1,
    --             내부 CASE depth1 (CASE 자체는 depth 변화 없음),
    --             ORDER BY 스칼라서브쿼리 depth1
    --------------------------------------------------------------------------
    OPEN p_rc FOR
        WITH base AS (
            SELECT e.empno,
                e.ename,
                e.job,
                e.deptno,
                e.sal,
                e.comm,
                e.mgr
            FROM emp e
        ),
        enriched AS (
            SELECT b.*,
                NVL (b.comm, /* AZ: comm NULL→0 */
                    0) AS eff_comm,
                -- [BA] 3단 CASE 시작
                CASE
                    WHEN b.job = 'PRESIDENT' THEN
                        CASE
                            WHEN b.sal > /* BB: elite 기준 */
                                4000 THEN 'ELITE'
                            ELSE 'SENIOR'
                        END
                    WHEN b.job IN (
                        /* BC: 관리직 목록 */
                        'MANAGER', 'ANALYST') THEN
                            CASE
                                WHEN b.sal >= 3000 THEN 'HIGH'
                                WHEN b.sal >= /* BD: mid 기준 */
                                    2000 THEN 'MID'
                                ELSE 'LOW'
                            END
                    ELSE
                        CASE
                            WHEN NVL (b.comm, /* BE: NULL→0 */
                                    0) > 0 THEN 'COMMISSION'
                            ELSE 'BASE'
                        END
                END AS pay_tier -- [BF] CASE 끝
            FROM base b
        )
        SELECT e.empno,
            e.ename,
            e.job,
            e.deptno,
            e.sal,
            e.pay_tier,
            /* BG: 부서명 스칼라서브쿼리 */
            (
                SELECT d.dname
                FROM dept d
                WHERE d.deptno = e.deptno -- [BH] correlated
            ) AS dname
        FROM enriched e
        ORDER BY
            /* BI: 인원수 기준 정렬 */
            (
                SELECT COUNT(*)
                FROM emp e2
                WHERE e2.deptno = e.deptno
            ) DESC,
            e.sal DESC NULLS LAST;
    --------------------------------------------------------------------------
    -- [Q9] WITH + MODEL (DIMENSION BY + MEASURES + RULES)
    --      depth: MODEL MEASURES( depth1, RULES 각 식 depth0→함수들 depth1,
    --             NULLIF( depth1, CV() depth2
    --------------------------------------------------------------------------
    OPEN p_rc FOR
        WITH base AS (
            SELECT deptno,
                SUM (sal) AS sum_sal,
                COUNT(*) AS cnt,
                MAX(sal) AS max_sal
            FROM emp
            GROUP BY deptno
        )
        SELECT deptno,
            sum_sal,
            cnt,
            max_sal,
            avg_sal_calc,
            bonus_pool,
            efficiency
        FROM base
        MODEL
            DIMENSION BY (deptno)
            MEASURES (
                /* BJ: 계산 컬럼 */
                sum_sal, cnt, max_sal, 0 AS avg_sal_calc, 0 AS bonus_pool, 0 AS efficiency)
            RULES (
                -- [BK] 평균 급여 계산
                avg_sal_calc [ ANY ] = ROUND (sum_sal [ CV () ] / NULLIF (
                        /* BL: 0 방지 */
                        cnt [ CV () ], 0), 2), bonus_pool [ ANY ] = ROUND (sum_sal [ CV () ] * /* BM: 보너스율 */
                            0.10, 2), efficiency [ ANY ] = ROUND (NULLIF (max_sal [ CV () ], /* BN: 0 방지 */
                                    0) / NULLIF (sum_sal [ CV () ], 0) * 100, 2))
        ORDER BY deptno;
    --------------------------------------------------------------------------
    -- [Q10] WITH + MATCH_RECOGNIZE (PARTITION + DEFINE 안 서브쿼리)
    --       depth: MATCH_RECOGNIZE( depth1, MEASURES depth1,
    --              DEFINE B AS B.sal > ... AND B.sal < ( depth2,
    --              내부 SELECT AVG depth2, WHERE depth2
    --------------------------------------------------------------------------
    OPEN p_rc FOR
        WITH ordered_emp AS (
            SELECT empno,
                ename,
                deptno,
                sal,
                hiredate,
                -- [BO] 부서 내 순서
                ROW_NUMBER () OVER (
                    PARTITION BY deptno
                    ORDER BY hiredate,
                    empno
                ) AS rn
            FROM emp
        )
        SELECT *
        FROM ordered_emp
        MATCH_RECOGNIZE (
            PARTITION BY deptno
            ORDER BY rn
            MEASURES
            /* BP: 시작/끝 이름 */
            FIRST (ename) AS start_name,
            LAST (ename) AS end_name,
            COUNT(*) AS streak_len,
            SUM (sal) AS streak_sal
            ONE ROW PER MATCH
            PATTERN (A B +) -- [BQ] 패턴 정의
            DEFINE
            -- [BR] B 조건: 이전보다 높고 부서 평균의 1.5배 미만
            B AS B.sal > PREV (B.sal)
            AND B.sal < (
                /* BS: 부서 평균 서브쿼리 */
                SELECT AVG(sal) * /* BT: 상한 배율 */
                        1.5
                FROM emp
                WHERE deptno = /* BU: correlated */
                        B.deptno
            )
        )
        FETCH FIRST /* BV: 상위 20건 */
        20 ROWS ONLY;
END test_open_with_proc;
/
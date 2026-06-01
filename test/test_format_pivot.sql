SELECT pvt.deptno,
    pvt."CLERK" AS clerk_cnt,
    pvt."MANAGER" AS manager_cnt,
    pvt."ANALYST" AS analyst_cnt,
    pvt."SALESMAN" AS salesman_cnt,
    pvt."PRESIDENT" AS president_cnt
FROM (
        SELECT e.deptno,
            e.job
        FROM emp e
    )
PIVOT (
    COUNT(*)
    FOR job IN ('CLERK' AS "CLERK", 'MANAGER' AS "MANAGER", 'ANALYST' AS "ANALYST", 'SALESMAN' AS "SALESMAN", 'PRESIDENT' AS "PRESIDENT")
) pvt
ORDER BY pvt.deptno;

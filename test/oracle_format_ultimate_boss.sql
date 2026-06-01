--------------------------------------------------------------------------------
-- ULTIMATE ORACLE FORMATTER BOSS SCRIPT
-- 목적:
--   1) 자동 포매터의 괄호/깊이/정렬/키워드 배치를 극한으로 검증
--   2) SQL + PL/SQL + DDL + DML + 고급 구문을 1개 스크립트에서 통합 검증
-- 권장 대상 버전:
--   Oracle 19c+
--------------------------------------------------------------------------------

--------------------------------------------------------------------------------
-- CLEANUP
--------------------------------------------------------------------------------
BEGIN
    FOR r IN (
        SELECT object_name, object_type
        FROM user_objects
        WHERE object_name IN (
            'QT_FMT_EMP',
            'QT_FMT_DEPT',
            'QT_FMT_SALES',
            'QT_FMT_CAL',
            'QT_FMT_EMP_V',
            'QT_FMT_PKG'
        )
        ORDER BY
            CASE object_type
                WHEN 'VIEW' THEN 1
                WHEN 'PACKAGE BODY' THEN 2
                WHEN 'PACKAGE' THEN 3
                WHEN 'TABLE' THEN 4
                ELSE 5
            END
    ) LOOP
        BEGIN
            EXECUTE IMMEDIATE
                CASE
                    WHEN r.object_type = 'VIEW' THEN
                        'DROP VIEW ' || r.object_name
                    WHEN r.object_type = 'PACKAGE BODY' THEN
                        NULL
                    WHEN r.object_type = 'PACKAGE' THEN
                        'DROP PACKAGE ' || r.object_name
                    WHEN r.object_type = 'TABLE' THEN
                        'DROP TABLE ' || r.object_name || ' PURGE'
                    ELSE
                        NULL
                END;
        EXCEPTION
            WHEN OTHERS THEN
                NULL;
        END;
    END LOOP;
END;
/

--------------------------------------------------------------------------------
-- TABLES
--------------------------------------------------------------------------------
CREATE TABLE qt_fmt_dept
(
    dept_id        NUMBER        CONSTRAINT qt_fmt_dept_pk PRIMARY KEY,
    parent_dept_id NUMBER        NULL,
    dept_code      VARCHAR2(30)  NOT NULL,
    dept_name      VARCHAR2(100) NOT NULL,
    region         VARCHAR2(30)  NOT NULL,
    active_yn      CHAR(1)       DEFAULT 'Y' NOT NULL,
    created_at     DATE          DEFAULT SYSDATE NOT NULL,
    CONSTRAINT qt_fmt_dept_fk1 FOREIGN KEY (parent_dept_id)
        REFERENCES qt_fmt_dept (dept_id)
);

CREATE TABLE qt_fmt_emp
(
    emp_id          NUMBER         CONSTRAINT qt_fmt_emp_pk PRIMARY KEY,
    dept_id         NUMBER         NOT NULL,
    mgr_emp_id      NUMBER         NULL,
    emp_name        VARCHAR2(100)  NOT NULL,
    login_name      VARCHAR2(100)  NOT NULL,
    job_title       VARCHAR2(100)  NOT NULL,
    grade_no        NUMBER(3)      NOT NULL,
    salary          NUMBER(12, 2)  NOT NULL,
    bonus           NUMBER(12, 2)  NULL,
    hire_date       DATE           NOT NULL,
    term_date       DATE           NULL,
    status          VARCHAR2(20)   NOT NULL,
    email_addr      VARCHAR2(200)  NULL,
    phone_no        VARCHAR2(50)   NULL,
    note_text       VARCHAR2(4000) NULL,
    json_profile    VARCHAR2(4000) NULL,
    created_at      TIMESTAMP      DEFAULT SYSTIMESTAMP NOT NULL,
    updated_at      TIMESTAMP      NULL,
    CONSTRAINT qt_fmt_emp_fk1 FOREIGN KEY (dept_id)
        REFERENCES qt_fmt_dept (dept_id),
    CONSTRAINT qt_fmt_emp_fk2 FOREIGN KEY (mgr_emp_id)
        REFERENCES qt_fmt_emp (emp_id),
    CONSTRAINT qt_fmt_emp_ck1 CHECK (status IN ('ACTIVE', 'LEAVE', 'TERM')),
    CONSTRAINT qt_fmt_emp_ck2 CHECK (json_profile IS JSON)
);

CREATE TABLE qt_fmt_sales
(
    sale_id         NUMBER         CONSTRAINT qt_fmt_sales_pk PRIMARY KEY,
    emp_id          NUMBER         NOT NULL,
    dept_id         NUMBER         NOT NULL,
    sale_date       DATE           NOT NULL,
    channel_code    VARCHAR2(30)   NOT NULL,
    product_code    VARCHAR2(30)   NOT NULL,
    qty             NUMBER(12, 2)  NOT NULL,
    unit_price      NUMBER(12, 2)  NOT NULL,
    discount_amt    NUMBER(12, 2)  DEFAULT 0 NOT NULL,
    tax_amt         NUMBER(12, 2)  DEFAULT 0 NOT NULL,
    remark          VARCHAR2(2000) NULL,
    created_at      TIMESTAMP      DEFAULT SYSTIMESTAMP NOT NULL,
    CONSTRAINT qt_fmt_sales_fk1 FOREIGN KEY (emp_id)
        REFERENCES qt_fmt_emp (emp_id),
    CONSTRAINT qt_fmt_sales_fk2 FOREIGN KEY (dept_id)
        REFERENCES qt_fmt_dept (dept_id)
);

CREATE TABLE qt_fmt_cal
(
    dt              DATE          CONSTRAINT qt_fmt_cal_pk PRIMARY KEY,
    yyyy            NUMBER(4)     NOT NULL,
    mm              NUMBER(2)     NOT NULL,
    dd              NUMBER(2)     NOT NULL,
    qtr             NUMBER(1)     NOT NULL,
    dow_no          NUMBER(1)     NOT NULL,
    dow_name        VARCHAR2(20)  NOT NULL,
    month_name      VARCHAR2(20)  NOT NULL,
    week_of_year    NUMBER(2)     NOT NULL,
    is_month_start  CHAR(1)       NOT NULL,
    is_month_end    CHAR(1)       NOT NULL
);

--------------------------------------------------------------------------------
-- SEED DATA
--------------------------------------------------------------------------------
INSERT INTO qt_fmt_dept (dept_id, parent_dept_id, dept_code, dept_name, region, active_yn)
VALUES (10, NULL, 'HQ', 'Headquarters', 'GLOBAL', 'Y');

INSERT INTO qt_fmt_dept (dept_id, parent_dept_id, dept_code, dept_name, region, active_yn)
VALUES (20, 10, 'ENG', 'Engineering', 'APAC', 'Y');

INSERT INTO qt_fmt_dept (dept_id, parent_dept_id, dept_code, dept_name, region, active_yn)
VALUES (30, 10, 'SAL', 'Sales', 'APAC', 'Y');

INSERT INTO qt_fmt_dept (dept_id, parent_dept_id, dept_code, dept_name, region, active_yn)
VALUES (40, 10, 'FIN', 'Finance', 'EMEA', 'Y');

INSERT INTO qt_fmt_dept (dept_id, parent_dept_id, dept_code, dept_name, region, active_yn)
VALUES (50, 20, 'DBA', 'Database', 'APAC', 'Y');

INSERT INTO qt_fmt_dept (dept_id, parent_dept_id, dept_code, dept_name, region, active_yn)
VALUES (60, 20, 'APP', 'Applications', 'AMER', 'Y');

INSERT INTO qt_fmt_emp
(
    emp_id, dept_id, mgr_emp_id, emp_name, login_name, job_title, grade_no,
    salary, bonus, hire_date, term_date, status, email_addr, phone_no, note_text, json_profile
)
VALUES
(
    100, 10, NULL, 'ALICE', 'alice', 'CEO', 1,
    300000, 50000, DATE '2015-01-01', NULL, 'ACTIVE',
    'alice@example.com', '010-1000-1000', 'top executive',
    '{"skills":["leadership","strategy"],"level":"exec","flags":{"remote":false,"travel":true}}'
);

INSERT INTO qt_fmt_emp
(
    emp_id, dept_id, mgr_emp_id, emp_name, login_name, job_title, grade_no,
    salary, bonus, hire_date, term_date, status, email_addr, phone_no, note_text, json_profile
)
VALUES
(
    200, 20, 100, 'BOB', 'bob', 'VP ENGINEERING', 2,
    200000, 30000, DATE '2017-03-15', NULL, 'ACTIVE',
    'bob@example.com', '010-2000-2000', 'owns engineering',
    '{"skills":["architecture","management"],"level":"vp","flags":{"remote":true,"travel":true}}'
);

INSERT INTO qt_fmt_emp
(
    emp_id, dept_id, mgr_emp_id, emp_name, login_name, job_title, grade_no,
    salary, bonus, hire_date, term_date, status, email_addr, phone_no, note_text, json_profile
)
VALUES
(
    210, 50, 200, 'CAROL', 'carol', 'DBA MANAGER', 3,
    160000, 20000, DATE '2018-06-01', NULL, 'ACTIVE',
    'carol@example.com', '010-2100-2100', 'oracle specialist',
    '{"skills":["oracle","performance","backup"],"level":"mgr","flags":{"remote":true,"travel":false}}'
);

INSERT INTO qt_fmt_emp
(
    emp_id, dept_id, mgr_emp_id, emp_name, login_name, job_title, grade_no,
    salary, bonus, hire_date, term_date, status, email_addr, phone_no, note_text, json_profile
)
VALUES
(
    220, 60, 200, 'DAVE', 'dave', 'APP MANAGER', 3,
    150000, 18000, DATE '2019-07-11', NULL, 'ACTIVE',
    'dave@example.com', '010-2200-2200', 'platform owner',
    '{"skills":["java","rust","apis"],"level":"mgr","flags":{"remote":false,"travel":true}}'
);

INSERT INTO qt_fmt_emp
(
    emp_id, dept_id, mgr_emp_id, emp_name, login_name, job_title, grade_no,
    salary, bonus, hire_date, term_date, status, email_addr, phone_no, note_text, json_profile
)
VALUES
(
    300, 30, 100, 'ERIN', 'erin', 'VP SALES', 2,
    190000, 50000, DATE '2016-11-21', NULL, 'ACTIVE',
    'erin@example.com', '010-3000-3000', 'revenue leader',
    '{"skills":["sales","forecasting"],"level":"vp","flags":{"remote":true,"travel":true}}'
);

INSERT INTO qt_fmt_emp
(
    emp_id, dept_id, mgr_emp_id, emp_name, login_name, job_title, grade_no,
    salary, bonus, hire_date, term_date, status, email_addr, phone_no, note_text, json_profile
)
VALUES
(
    310, 30, 300, 'FRANK', 'frank', 'ACCOUNT EXECUTIVE', 4,
    110000, 12000, DATE '2021-02-10', NULL, 'ACTIVE',
    'frank@example.com', '010-3100-3100', 'enterprise sales',
    '{"skills":["negotiation","crm"],"level":"ic","flags":{"remote":true,"travel":true}}'
);

INSERT INTO qt_fmt_emp
(
    emp_id, dept_id, mgr_emp_id, emp_name, login_name, job_title, grade_no,
    salary, bonus, hire_date, term_date, status, email_addr, phone_no, note_text, json_profile
)
VALUES
(
    320, 30, 300, 'GRACE', 'grace', 'SALES OPS', 4,
    105000, 10000, DATE '2022-01-05', NULL, 'ACTIVE',
    'grace@example.com', '010-3200-3200', 'pipeline analytics',
    '{"skills":["sql","ops","forecast"],"level":"ic","flags":{"remote":false,"travel":false}}'
);

INSERT INTO qt_fmt_emp
(
    emp_id, dept_id, mgr_emp_id, emp_name, login_name, job_title, grade_no,
    salary, bonus, hire_date, term_date, status, email_addr, phone_no, note_text, json_profile
)
VALUES
(
    400, 40, 100, 'HELEN', 'helen', 'CFO', 2,
    210000, 40000, DATE '2017-09-01', NULL, 'ACTIVE',
    'helen@example.com', '010-4000-4000', 'finance head',
    '{"skills":["finance","audit"],"level":"cxo","flags":{"remote":false,"travel":true}}'
);

INSERT INTO qt_fmt_sales
    (sale_id, emp_id, dept_id, sale_date, channel_code, product_code, qty, unit_price, discount_amt, tax_amt, remark)
VALUES
    (1, 310, 30, DATE '2024-01-03', 'ONLINE', 'P1', 10, 1200, 100, 55, 'first order');

INSERT INTO qt_fmt_sales
    (sale_id, emp_id, dept_id, sale_date, channel_code, product_code, qty, unit_price, discount_amt, tax_amt, remark)
VALUES
    (2, 310, 30, DATE '2024-01-08', 'PARTNER', 'P2', 5, 4000, 0, 150, 'partner close');

INSERT INTO qt_fmt_sales
    (sale_id, emp_id, dept_id, sale_date, channel_code, product_code, qty, unit_price, discount_amt, tax_amt, remark)
VALUES
    (3, 320, 30, DATE '2024-02-14', 'ONLINE', 'P1', 7, 1200, 20, 40, 'renewal');

INSERT INTO qt_fmt_sales
    (sale_id, emp_id, dept_id, sale_date, channel_code, product_code, qty, unit_price, discount_amt, tax_amt, remark)
VALUES
    (4, 320, 30, DATE '2024-02-20', 'DIRECT', 'P3', 2, 10000, 1000, 330, 'special bid');

INSERT INTO qt_fmt_sales
    (sale_id, emp_id, dept_id, sale_date, channel_code, product_code, qty, unit_price, discount_amt, tax_amt, remark)
VALUES
    (5, 310, 30, DATE '2024-03-09', 'ONLINE', 'P2', 12, 4100, 500, 210, 'large order');

INSERT INTO qt_fmt_sales
    (sale_id, emp_id, dept_id, sale_date, channel_code, product_code, qty, unit_price, discount_amt, tax_amt, remark)
VALUES
    (6, 310, 30, DATE '2024-03-15', 'DIRECT', 'P4', 1, 25000, 0, 900, 'executive sign-off');

INSERT INTO qt_fmt_sales
    (sale_id, emp_id, dept_id, sale_date, channel_code, product_code, qty, unit_price, discount_amt, tax_amt, remark)
VALUES
    (7, 320, 30, DATE '2024-04-01', 'PARTNER', 'P2', 3, 3900, 0, 100, 'q2 kickoff');

INSERT INTO qt_fmt_sales
    (sale_id, emp_id, dept_id, sale_date, channel_code, product_code, qty, unit_price, discount_amt, tax_amt, remark)
VALUES
    (8, 310, 30, DATE '2024-04-12', 'ONLINE', 'P1', 20, 1190, 250, 120, 'campaign');

INSERT INTO qt_fmt_sales
    (sale_id, emp_id, dept_id, sale_date, channel_code, product_code, qty, unit_price, discount_amt, tax_amt, remark)
VALUES
    (9, 320, 30, DATE '2024-04-21', 'DIRECT', 'P3', 4, 9800, 200, 350, 'upsell');

INSERT INTO qt_fmt_sales
    (sale_id, emp_id, dept_id, sale_date, channel_code, product_code, qty, unit_price, discount_amt, tax_amt, remark)
VALUES
    (10, 310, 30, DATE '2024-05-02', 'ONLINE', 'P5', 6, 5000, 0, 175, 'new sku');

INSERT INTO qt_fmt_cal
SELECT
    d.dt,
    EXTRACT(YEAR FROM d.dt)                                                AS yyyy,
    EXTRACT(MONTH FROM d.dt)                                               AS mm,
    EXTRACT(DAY FROM d.dt)                                                 AS dd,
    TO_NUMBER(TO_CHAR(d.dt, 'Q'))                                          AS qtr,
    TO_NUMBER(TO_CHAR(d.dt, 'D'))                                          AS dow_no,
    TO_CHAR(d.dt, 'DY', 'NLS_DATE_LANGUAGE=ENGLISH')                       AS dow_name,
    TO_CHAR(d.dt, 'MONTH', 'NLS_DATE_LANGUAGE=ENGLISH')                    AS month_name,
    TO_NUMBER(TO_CHAR(d.dt, 'IW'))                                         AS week_of_year,
    CASE WHEN TRUNC(d.dt) = TRUNC(d.dt, 'MM') THEN 'Y' ELSE 'N' END        AS is_month_start,
    CASE WHEN TRUNC(d.dt) = LAST_DAY(d.dt) THEN 'Y' ELSE 'N' END           AS is_month_end
FROM (
    SELECT DATE '2024-01-01' + LEVEL - 1 AS dt
    FROM dual
    CONNECT BY LEVEL <= 180
) d;

COMMIT;

--------------------------------------------------------------------------------
-- VIEW: deeply nested query, analytics, scalar subquery, JSON, LISTAGG
--------------------------------------------------------------------------------
CREATE OR REPLACE VIEW qt_fmt_emp_v
AS
WITH base_emp AS (
    SELECT
        e.emp_id,
        e.dept_id,
        e.mgr_emp_id,
        e.emp_name,
        e.login_name,
        e.job_title,
        e.grade_no,
        e.salary,
        NVL(e.bonus, 0)                                                   AS bonus,
        e.hire_date,
        e.status,
        d.dept_code,
        d.dept_name,
        d.region,
        JSON_VALUE(e.json_profile, '$.level' RETURNING VARCHAR2(30))      AS profile_level,
        JSON_VALUE(e.json_profile, '$.flags.remote' RETURNING VARCHAR2(10)) AS remote_flag
    FROM qt_fmt_emp e
    JOIN qt_fmt_dept d
        ON d.dept_id = e.dept_id
),
sales_agg AS (
    SELECT
        s.emp_id,
        COUNT(*)                                                          AS sale_cnt,
        SUM((s.qty * s.unit_price) - s.discount_amt + s.tax_amt)          AS gross_sales,
        AVG((s.qty * s.unit_price) - s.discount_amt + s.tax_amt)          AS avg_sales,
        MAX(s.sale_date) KEEP (DENSE_RANK LAST ORDER BY s.sale_date)      AS last_sale_date,
        LISTAGG(
            s.product_code || ':' || TO_CHAR((s.qty * s.unit_price) - s.discount_amt + s.tax_amt),
            ' | '
        ) WITHIN GROUP (ORDER BY s.sale_date, s.sale_id)                  AS sale_breakdown
    FROM qt_fmt_sales s
    GROUP BY s.emp_id
)
SELECT
    b.emp_id,
    b.dept_id,
    b.mgr_emp_id,
    b.emp_name,
    b.login_name,
    b.job_title,
    b.grade_no,
    b.salary,
    b.bonus,
    b.hire_date,
    b.status,
    b.dept_code,
    b.dept_name,
    b.region,
    b.profile_level,
    b.remote_flag,
    NVL(sa.sale_cnt, 0)                                                   AS sale_cnt,
    NVL(sa.gross_sales, 0)                                                AS gross_sales,
    NVL(sa.avg_sales, 0)                                                  AS avg_sales,
    sa.last_sale_date,
    sa.sale_breakdown,
    RANK() OVER (PARTITION BY b.dept_id ORDER BY b.salary DESC, b.emp_id) AS dept_salary_rank,
    DENSE_RANK() OVER (ORDER BY NVL(sa.gross_sales, 0) DESC, b.emp_id)    AS company_sales_rank,
    (
        SELECT COUNT(*)
        FROM qt_fmt_emp c
        WHERE c.mgr_emp_id = b.emp_id
    )                                                                     AS direct_report_cnt,
    JSON_OBJECT(
        'empId' VALUE b.emp_id,
        'name' VALUE b.emp_name,
        'dept' VALUE b.dept_name,
        'sales' VALUE NVL(sa.gross_sales, 0),
        'flags' VALUE JSON_OBJECT(
            'remote' VALUE b.remote_flag,
            'active' VALUE CASE WHEN b.status = 'ACTIVE' THEN 'true' ELSE 'false' END
        )
        RETURNING CLOB
    )                                                                     AS profile_json
FROM base_emp b
LEFT JOIN sales_agg sa
    ON sa.emp_id = b.emp_id;

--------------------------------------------------------------------------------
-- QUERY 1: monstrous WITH + recursive + analytics + scalar subqueries + apply
--------------------------------------------------------------------------------
WITH
seed AS (
    SELECT
        e.emp_id,
        e.mgr_emp_id,
        e.dept_id,
        e.emp_name,
        e.salary,
        NVL(e.bonus, 0)                                                    AS bonus,
        e.status,
        e.hire_date
    FROM qt_fmt_emp e
    WHERE e.status = 'ACTIVE'
),
org_tree (emp_id, mgr_emp_id, dept_id, emp_name, lvl, root_emp_id, path_txt, salary, bonus, hire_date) AS (
    SELECT
        s.emp_id,
        s.mgr_emp_id,
        s.dept_id,
        s.emp_name,
        1                                                                  AS lvl,
        s.emp_id                                                           AS root_emp_id,
        '/' || s.emp_name                                                  AS path_txt,
        s.salary,
        s.bonus,
        s.hire_date
    FROM seed s
    WHERE s.mgr_emp_id IS NULL
    UNION ALL
    SELECT
        c.emp_id,
        c.mgr_emp_id,
        c.dept_id,
        c.emp_name,
        p.lvl + 1                                                          AS lvl,
        p.root_emp_id,
        p.path_txt || '/' || c.emp_name                                    AS path_txt,
        c.salary,
        c.bonus,
        c.hire_date
    FROM seed c
    JOIN org_tree p
        ON p.emp_id = c.mgr_emp_id
),
org_enriched AS (
    SELECT
        o.*,
        SUM(o.salary + o.bonus) OVER (
            PARTITION BY o.root_emp_id
            ORDER BY o.lvl, o.emp_id
            ROWS BETWEEN UNBOUNDED PRECEDING AND CURRENT ROW
        )                                                                  AS running_root_comp,
        AVG(o.salary) OVER (PARTITION BY o.dept_id)                        AS dept_avg_salary,
        MIN(o.hire_date) OVER (PARTITION BY o.root_emp_id)                 AS root_min_hire_date,
        MAX(o.hire_date) OVER (PARTITION BY o.root_emp_id)                 AS root_max_hire_date,
        ROW_NUMBER() OVER (PARTITION BY o.dept_id ORDER BY o.salary DESC)  AS dept_rn
    FROM org_tree o
),
sales_by_emp AS (
    SELECT
        s.emp_id,
        SUM((s.qty * s.unit_price) - s.discount_amt + s.tax_amt)           AS sales_amt,
        COUNT(*)                                                           AS sales_cnt,
        MAX(s.sale_date)                                                   AS last_sale_date
    FROM qt_fmt_sales s
    GROUP BY s.emp_id
)
SELECT
    oe.root_emp_id,
    oe.emp_id,
    oe.mgr_emp_id,
    oe.dept_id,
    d.dept_name,
    oe.emp_name,
    oe.lvl,
    LPAD(' ', (oe.lvl - 1) * 2, ' ') || oe.emp_name                        AS indented_name,
    oe.path_txt,
    oe.salary,
    oe.bonus,
    oe.running_root_comp,
    oe.dept_avg_salary,
    oe.dept_rn,
    NVL(sbe.sales_amt, 0)                                                  AS sales_amt,
    NVL(sbe.sales_cnt, 0)                                                  AS sales_cnt,
    sbe.last_sale_date,
    ca.peer_cnt,
    ca.peer_names,
    (
        SELECT COUNT(*)
        FROM qt_fmt_emp x
        WHERE x.dept_id = oe.dept_id
          AND x.salary > oe.salary
    )                                                                      AS same_dept_higher_paid_cnt,
    (
        SELECT MAX(y.salary)
        FROM qt_fmt_emp y
        WHERE y.mgr_emp_id = oe.emp_id
    )                                                                      AS max_direct_report_salary,
    CASE
        WHEN NVL(sbe.sales_amt, 0) >= 50000 AND oe.salary >= oe.dept_avg_salary THEN
            'STAR'
        WHEN NVL(sbe.sales_amt, 0) >= 10000 THEN
            'SELLER'
        WHEN oe.dept_rn = 1 THEN
            'TOP_PAID'
        ELSE
            'NORMAL'
    END                                                                    AS complex_class
FROM org_enriched oe
JOIN qt_fmt_dept d
    ON d.dept_id = oe.dept_id
CROSS APPLY (
    SELECT
        COUNT(*)                                                           AS peer_cnt,
        LISTAGG(p.emp_name, ', ') WITHIN GROUP (ORDER BY p.emp_name)       AS peer_names
    FROM qt_fmt_emp p
    WHERE p.dept_id = oe.dept_id
) ca
LEFT JOIN sales_by_emp sbe
    ON sbe.emp_id = oe.emp_id
ORDER BY
    oe.root_emp_id,
    oe.lvl,
    oe.emp_id;

--------------------------------------------------------------------------------
-- QUERY 2: CONNECT BY + nested scalar subqueries + CONNECT_BY_ROOT + SYS_CONNECT_BY_PATH
--------------------------------------------------------------------------------
SELECT
    CONNECT_BY_ROOT d.dept_name                                             AS root_dept_name,
    d.dept_id,
    d.parent_dept_id,
    d.dept_name,
    LEVEL                                                                   AS lvl,
    SYS_CONNECT_BY_PATH(d.dept_code, ' > ')                                 AS dept_path,
    (
        SELECT COUNT(*)
        FROM qt_fmt_emp e
        WHERE e.dept_id = d.dept_id
    )                                                                       AS emp_cnt,
    (
        SELECT NVL(SUM(e.salary + NVL(e.bonus, 0)), 0)
        FROM qt_fmt_emp e
        WHERE e.dept_id = d.dept_id
    )                                                                       AS payroll_amt,
    CASE
        WHEN CONNECT_BY_ISLEAF = 1 THEN
            'LEAF'
        ELSE
            'NODE'
    END                                                                     AS node_type
FROM qt_fmt_dept d
START WITH d.parent_dept_id IS NULL
CONNECT BY PRIOR d.dept_id = d.parent_dept_id
ORDER SIBLINGS BY d.dept_name;

--------------------------------------------------------------------------------
-- QUERY 3: SEARCH + CYCLE recursive subquery factoring
--------------------------------------------------------------------------------
WITH dept_walk (dept_id, parent_dept_id, dept_name, lvl, path_txt) AS (
    SELECT
        d.dept_id,
        d.parent_dept_id,
        d.dept_name,
        1                                                            AS lvl,
        '/' || d.dept_name                                           AS path_txt
    FROM qt_fmt_dept d
    WHERE d.parent_dept_id IS NULL
    UNION ALL
    SELECT
        c.dept_id,
        c.parent_dept_id,
        c.dept_name,
        p.lvl + 1                                                    AS lvl,
        p.path_txt || '/' || c.dept_name                             AS path_txt
    FROM qt_fmt_dept c
    JOIN dept_walk p
        ON p.dept_id = c.parent_dept_id
)
SEARCH DEPTH FIRST BY dept_name SET dfs_order
CYCLE dept_id SET is_cycle TO 'Y' DEFAULT 'N'
SELECT
    dept_id,
    parent_dept_id,
    dept_name,
    lvl,
    path_txt,
    dfs_order,
    is_cycle
FROM dept_walk
ORDER BY dfs_order;

--------------------------------------------------------------------------------
-- QUERY 4: PIVOT
--------------------------------------------------------------------------------
SELECT *
FROM (
    SELECT
        TO_CHAR(s.sale_date, 'YYYY-MM')                                    AS sale_month,
        s.channel_code,
        ((s.qty * s.unit_price) - s.discount_amt + s.tax_amt)              AS net_amt
    FROM qt_fmt_sales s
)
PIVOT (
    SUM(net_amt)
    FOR channel_code IN (
        'ONLINE'  AS online_amt,
        'DIRECT'  AS direct_amt,
        'PARTNER' AS partner_amt
    )
)
ORDER BY sale_month;

--------------------------------------------------------------------------------
-- QUERY 5: UNPIVOT
--------------------------------------------------------------------------------
SELECT
    emp_id,
    comp_type,
    comp_value
FROM (
    SELECT
        e.emp_id,
        e.salary,
        NVL(e.bonus, 0)                                                    AS bonus,
        (e.salary + NVL(e.bonus, 0))                                       AS total_comp
    FROM qt_fmt_emp e
)
UNPIVOT INCLUDE NULLS (
    comp_value FOR comp_type IN (
        salary     AS 'SALARY',
        bonus      AS 'BONUS',
        total_comp AS 'TOTAL_COMP'
    )
)
ORDER BY emp_id, comp_type;

--------------------------------------------------------------------------------
-- QUERY 6: MATCH_RECOGNIZE
--------------------------------------------------------------------------------
SELECT
    *
FROM qt_fmt_sales
MATCH_RECOGNIZE (
    PARTITION BY emp_id
    ORDER BY sale_date, sale_id
    MEASURES
        MATCH_NUMBER()                                                     AS match_no,
        CLASSIFIER()                                                       AS cls,
        FIRST(sale_date)                                                   AS first_sale_date,
        LAST(sale_date)                                                    AS last_sale_date,
        SUM((qty * unit_price) - discount_amt + tax_amt)                   AS pattern_sales
    ALL ROWS PER MATCH
    PATTERN (low+ mid* high)
    DEFINE
        low  AS ((qty * unit_price) - discount_amt + tax_amt) < 10000,
        mid  AS ((qty * unit_price) - discount_amt + tax_amt) BETWEEN 10000 AND 30000,
        high AS ((qty * unit_price) - discount_amt + tax_amt) > 30000
)
ORDER BY emp_id, sale_date, sale_id;

--------------------------------------------------------------------------------
-- QUERY 7: MODEL clause
--------------------------------------------------------------------------------
SELECT
    year_key,
    month_key,
    channel_code,
    base_amt,
    proj_amt
FROM (
    SELECT
        EXTRACT(YEAR FROM s.sale_date)                                     AS year_key,
        EXTRACT(MONTH FROM s.sale_date)                                    AS month_key,
        s.channel_code,
        SUM((s.qty * s.unit_price) - s.discount_amt + s.tax_amt)           AS base_amt,
        CAST(NULL AS NUMBER)                                               AS proj_amt
    FROM qt_fmt_sales s
    GROUP BY
        EXTRACT(YEAR FROM s.sale_date),
        EXTRACT(MONTH FROM s.sale_date),
        s.channel_code
)
MODEL
    PARTITION BY (year_key, channel_code)
    DIMENSION BY (month_key)
    MEASURES (base_amt, proj_amt)
    RULES SEQUENTIAL ORDER (
        proj_amt[ANY] =
            CASE
                WHEN CV(month_key) = 1 THEN
                    NVL(base_amt[CV(month_key)], 0)
                ELSE
                    ROUND(
                        NVL(base_amt[CV(month_key)], 0) * 1.05
                        + NVL(proj_amt[CV(month_key) - 1], 0) * 0.10,
                        2
                    )
            END
    )
ORDER BY year_key, channel_code, month_key;

--------------------------------------------------------------------------------
-- QUERY 8: complex JSON aggregation + nested SELECT + ORDER BY inside aggregation
--------------------------------------------------------------------------------
SELECT
    d.dept_id,
    d.dept_name,
    JSON_ARRAYAGG(
        JSON_OBJECT(
            'empId' VALUE e.emp_id,
            'name' VALUE e.emp_name,
            'job' VALUE e.job_title,
            'salary' VALUE e.salary,
            'sales' VALUE (
                SELECT NVL(SUM((s.qty * s.unit_price) - s.discount_amt + s.tax_amt), 0)
                FROM qt_fmt_sales s
                WHERE s.emp_id = e.emp_id
            ),
            'meta' VALUE JSON_OBJECT(
                'grade' VALUE e.grade_no,
                'status' VALUE e.status,
                'hireDate' VALUE TO_CHAR(e.hire_date, 'YYYY-MM-DD')
            )
            RETURNING CLOB
        )
        ORDER BY e.salary DESC, e.emp_id
        RETURNING CLOB
    )                                                                      AS dept_emp_json
FROM qt_fmt_dept d
LEFT JOIN qt_fmt_emp e
    ON e.dept_id = d.dept_id
GROUP BY d.dept_id, d.dept_name
ORDER BY d.dept_id;

--------------------------------------------------------------------------------
-- QUERY 9: deeply nested CASE / DECODE / NULLIF / COALESCE / analytic
--------------------------------------------------------------------------------
SELECT
    v.emp_id,
    v.emp_name,
    v.dept_name,
    v.salary,
    v.bonus,
    v.gross_sales,
    COALESCE(
        NULLIF(v.remote_flag, 'null'),
        'unknown'
    )                                                                       AS normalized_remote_flag,
    CASE
        WHEN v.gross_sales > 50000 THEN
            CASE
                WHEN v.salary > (
                    SELECT AVG(x.salary)
                    FROM qt_fmt_emp x
                    WHERE x.dept_id = v.dept_id
                ) THEN
                    'HIGH_SALES_HIGH_PAY'
                WHEN v.salary = (
                    SELECT MAX(x.salary)
                    FROM qt_fmt_emp x
                    WHERE x.dept_id = v.dept_id
                ) THEN
                    'HIGH_SALES_TOP_PAY'
                ELSE
                    'HIGH_SALES_NORMAL_PAY'
            END
        WHEN v.gross_sales BETWEEN 10000 AND 50000 THEN
            DECODE(
                SIGN(v.salary - v.avg_sales),
                1, 'MID_SALES_PAY_GT_AVGSALE',
                0, 'MID_SALES_PAY_EQ_AVGSALE',
                -1, 'MID_SALES_PAY_LT_AVGSALE',
                'MID_SALES_UNKNOWN'
            )
        ELSE
            CASE
                WHEN v.direct_report_cnt > 0 THEN
                    'MANAGER_LOW_SALES'
                ELSE
                    'IC_LOW_SALES'
            END
    END                                                                      AS classification_1,
    SUM(v.salary + v.bonus) OVER (
        PARTITION BY v.dept_id
        ORDER BY v.hire_date, v.emp_id
        ROWS BETWEEN UNBOUNDED PRECEDING AND CURRENT ROW
    )                                                                        AS running_comp,
    AVG(v.salary) OVER (
        PARTITION BY v.dept_id
        ORDER BY v.hire_date, v.emp_id
        RANGE BETWEEN UNBOUNDED PRECEDING AND UNBOUNDED FOLLOWING
    )                                                                        AS full_dept_avg_salary
FROM qt_fmt_emp_v v
ORDER BY v.dept_id, v.hire_date, v.emp_id;

--------------------------------------------------------------------------------
-- DML 1: MERGE with nested source query
--------------------------------------------------------------------------------
MERGE INTO qt_fmt_emp t
USING (
    WITH recent_sales AS (
        SELECT
            s.emp_id,
            SUM((s.qty * s.unit_price) - s.discount_amt + s.tax_amt)       AS recent_amt
        FROM qt_fmt_sales s
        WHERE s.sale_date >= DATE '2024-03-01'
        GROUP BY s.emp_id
    ),
    scored AS (
        SELECT
            e.emp_id,
            e.note_text,
            rs.recent_amt,
            CASE
                WHEN NVL(rs.recent_amt, 0) >= 50000 THEN ' | HOT'
                WHEN NVL(rs.recent_amt, 0) >= 20000 THEN ' | WARM'
                ELSE ' | COLD'
            END                                                            AS score_tag
        FROM qt_fmt_emp e
        LEFT JOIN recent_sales rs
            ON rs.emp_id = e.emp_id
        WHERE e.dept_id = 30
    )
    SELECT
        s.emp_id,
        SUBSTR(
            NVL(s.note_text, 'sales-profile')
            || s.score_tag
            || ' | recent='
            || TO_CHAR(NVL(s.recent_amt, 0)),
            1,
            4000
        )                                                                  AS new_note_text
    FROM scored s
) src
ON (t.emp_id = src.emp_id)
WHEN MATCHED THEN
    UPDATE SET
        t.note_text   = src.new_note_text,
        t.updated_at  = SYSTIMESTAMP;

--------------------------------------------------------------------------------
-- DML 2: INSERT ALL with conditional branches
--------------------------------------------------------------------------------
INSERT ALL
    WHEN dept_id = 30 AND salary >= 100000 THEN
        INTO qt_fmt_sales
            (sale_id, emp_id, dept_id, sale_date, channel_code, product_code, qty, unit_price, discount_amt, tax_amt, remark)
        VALUES
            (1000 + emp_id, emp_id, dept_id, DATE '2024-06-01', 'DIRECT', 'BONUSSKU', 1, 1000, 0, 0, 'insert all high salary sales seed')
    WHEN dept_id = 30 AND salary < 100000 THEN
        INTO qt_fmt_sales
            (sale_id, emp_id, dept_id, sale_date, channel_code, product_code, qty, unit_price, discount_amt, tax_amt, remark)
        VALUES
            (2000 + emp_id, emp_id, dept_id, DATE '2024-06-01', 'ONLINE', 'SMALLSKU', 1, 100, 0, 0, 'insert all low salary sales seed')
SELECT
    e.emp_id,
    e.dept_id,
    e.salary
FROM qt_fmt_emp e
WHERE e.dept_id = 30
  AND NOT EXISTS (
        SELECT 1
        FROM qt_fmt_sales s
        WHERE s.emp_id = e.emp_id
          AND s.sale_date = DATE '2024-06-01'
    );

COMMIT;

--------------------------------------------------------------------------------
-- PACKAGE SPEC
--------------------------------------------------------------------------------
CREATE OR REPLACE PACKAGE qt_fmt_pkg
AS
    FUNCTION get_emp_snapshot (
        p_emp_id         IN NUMBER,
        p_include_sales  IN VARCHAR2 DEFAULT 'Y'
    ) RETURN CLOB;

    PROCEDURE print_dept_rollup (
        p_root_dept_id   IN NUMBER,
        p_min_salary     IN NUMBER DEFAULT 0
    );

    PROCEDURE run_dynamic_report (
        p_dept_id        IN NUMBER,
        p_sort_expr      IN VARCHAR2 DEFAULT 'salary DESC, emp_id'
    );
END qt_fmt_pkg;
/

--------------------------------------------------------------------------------
-- PACKAGE BODY: dynamic sql + q quote + nested blocks + comments
--------------------------------------------------------------------------------
CREATE OR REPLACE PACKAGE BODY qt_fmt_pkg
AS
    FUNCTION get_emp_snapshot (
        p_emp_id         IN NUMBER,
        p_include_sales  IN VARCHAR2 DEFAULT 'Y'
    ) RETURN CLOB
    IS
        v_json   CLOB;
    BEGIN
        SELECT JSON_OBJECT(
                   'emp' VALUE JSON_OBJECT(
                       'empId' VALUE e.emp_id,
                       'name' VALUE e.emp_name,
                       'dept' VALUE d.dept_name,
                       'job' VALUE e.job_title,
                       'salary' VALUE e.salary,
                       'bonus' VALUE NVL(e.bonus, 0),
                       'status' VALUE e.status
                   ),
                   'sales' VALUE CASE
                       WHEN p_include_sales = 'Y' THEN (
                           SELECT JSON_ARRAYAGG(
                                      JSON_OBJECT(
                                          'saleId' VALUE s.sale_id,
                                          'dt' VALUE TO_CHAR(s.sale_date, 'YYYY-MM-DD'),
                                          'amt' VALUE ((s.qty * s.unit_price) - s.discount_amt + s.tax_amt),
                                          'channel' VALUE s.channel_code,
                                          'product' VALUE s.product_code
                                      )
                                      ORDER BY s.sale_date, s.sale_id
                                      RETURNING CLOB
                                  )
                           FROM qt_fmt_sales s
                           WHERE s.emp_id = e.emp_id
                       )
                       ELSE TO_CLOB('[]')
                   END
                   RETURNING CLOB
               )
        INTO v_json
        FROM qt_fmt_emp e
        JOIN qt_fmt_dept d
            ON d.dept_id = e.dept_id
        WHERE e.emp_id = p_emp_id;

        RETURN v_json;
    EXCEPTION
        WHEN NO_DATA_FOUND THEN
            RETURN TO_CLOB(
                q'!{"error":"NO_DATA_FOUND","detail":"employee not found"}!'
            );
    END get_emp_snapshot;

    PROCEDURE print_dept_rollup (
        p_root_dept_id   IN NUMBER,
        p_min_salary     IN NUMBER DEFAULT 0
    )
    IS
    BEGIN
        FOR r IN (
            WITH dept_tree (dept_id, parent_dept_id, dept_name, lvl, path_txt) AS (
                SELECT
                    d.dept_id,
                    d.parent_dept_id,
                    d.dept_name,
                    1                                                   AS lvl,
                    '/' || d.dept_name                                  AS path_txt
                FROM qt_fmt_dept d
                WHERE d.dept_id = p_root_dept_id
                UNION ALL
                SELECT
                    c.dept_id,
                    c.parent_dept_id,
                    c.dept_name,
                    p.lvl + 1                                           AS lvl,
                    p.path_txt || '/' || c.dept_name                    AS path_txt
                FROM qt_fmt_dept c
                JOIN dept_tree p
                    ON p.dept_id = c.parent_dept_id
            )
            SELECT
                dt.dept_id,
                dt.dept_name,
                dt.lvl,
                dt.path_txt,
                COUNT(e.emp_id)                                         AS emp_cnt,
                NVL(SUM(e.salary + NVL(e.bonus, 0)), 0)                 AS comp_sum,
                MAX(e.salary)                                           AS max_salary
            FROM dept_tree dt
            LEFT JOIN qt_fmt_emp e
                ON e.dept_id = dt.dept_id
               AND e.salary >= p_min_salary
            GROUP BY
                dt.dept_id,
                dt.dept_name,
                dt.lvl,
                dt.path_txt
            ORDER BY dt.lvl, dt.dept_id
        ) LOOP
            DBMS_OUTPUT.PUT_LINE(
                RPAD(' ', (r.lvl - 1) * 2, ' ')
                || r.dept_name
                || ' | cnt=' || r.emp_cnt
                || ' | sum=' || r.comp_sum
                || ' | max=' || NVL(r.max_salary, 0)
            );
        END LOOP;
    END print_dept_rollup;

    PROCEDURE run_dynamic_report (
        p_dept_id        IN NUMBER,
        p_sort_expr      IN VARCHAR2 DEFAULT 'salary DESC, emp_id'
    )
    IS
        v_sql           VARCHAR2(32767);
        TYPE t_refcur IS REF CURSOR;
        c               t_refcur;
        v_emp_id        qt_fmt_emp.emp_id%TYPE;
        v_emp_name      qt_fmt_emp.emp_name%TYPE;
        v_salary        qt_fmt_emp.salary%TYPE;
        v_gross_sales   NUMBER;
    BEGIN
        v_sql :=
               q'[
                    SELECT
                        e.emp_id,
                        e.emp_name,
                        e.salary,
                        (
                            SELECT NVL(SUM((s.qty * s.unit_price) - s.discount_amt + s.tax_amt), 0)
                            FROM qt_fmt_sales s
                            WHERE s.emp_id = e.emp_id
                        ) AS gross_sales
                    FROM qt_fmt_emp e
                    WHERE e.dept_id = :b1
               ]'
            || ' ORDER BY ' || p_sort_expr;

        OPEN c FOR v_sql USING p_dept_id;

        LOOP
            FETCH c INTO v_emp_id, v_emp_name, v_salary, v_gross_sales;
            EXIT WHEN c%NOTFOUND;

            BEGIN
                DBMS_OUTPUT.PUT_LINE(
                    'EMP_ID=' || v_emp_id
                    || ', NAME=' || v_emp_name
                    || ', SALARY=' || v_salary
                    || ', GROSS_SALES=' || NVL(v_gross_sales, 0)
                );
            EXCEPTION
                WHEN OTHERS THEN
                    DBMS_OUTPUT.PUT_LINE('PRINT_ERROR:' || SQLERRM);
            END;
        END LOOP;

        CLOSE c;
    EXCEPTION
        WHEN OTHERS THEN
            IF c%ISOPEN THEN
                CLOSE c;
            END IF;
            RAISE;
    END run_dynamic_report;
END qt_fmt_pkg;
/

--------------------------------------------------------------------------------
-- PACKAGE CALLS
--------------------------------------------------------------------------------
DECLARE
    v_json CLOB;
BEGIN
    v_json := qt_fmt_pkg.get_emp_snapshot(310, 'Y');
    DBMS_OUTPUT.PUT_LINE(DBMS_LOB.SUBSTR(v_json, 4000, 1));

    qt_fmt_pkg.print_dept_rollup(10, 100000);
    qt_fmt_pkg.run_dynamic_report(30, 'salary DESC, emp_id');
END;
/

--------------------------------------------------------------------------------
-- QUERY 10: view over view style consumption, nested ORDER BY expressions
--------------------------------------------------------------------------------
SELECT
    x.*
FROM (
    SELECT
        v.emp_id,
        v.emp_name,
        v.dept_name,
        v.salary,
        v.gross_sales,
        v.dept_salary_rank,
        v.company_sales_rank,
        CASE
            WHEN v.gross_sales > 0 THEN ROUND(v.salary / v.gross_sales, 6)
            ELSE NULL
        END                                                                 AS salary_to_sales_ratio,
        NTILE(4) OVER (
            ORDER BY
                NVL(v.gross_sales, 0) DESC,
                v.salary DESC,
                v.emp_id
        )                                                                    AS sales_quartile
    FROM qt_fmt_emp_v v
    WHERE v.status = 'ACTIVE'
) x
WHERE x.sales_quartile IN (1, 2, 3, 4)
ORDER BY
    CASE
        WHEN x.sales_quartile = 1 THEN 1
        WHEN x.sales_quartile = 2 THEN 2
        WHEN x.sales_quartile = 3 THEN 3
        ELSE 4
    END,
    x.salary_to_sales_ratio DESC NULLS LAST,
    x.emp_id;

--------------------------------------------------------------------------------
-- QUERY 11: intentionally ugly logical nesting for formatter stress
--------------------------------------------------------------------------------
SELECT
    e.emp_id,
    e.emp_name,
    d.dept_name,
    (
        (
            (
                NVL(e.salary, 0)
                + NVL(e.bonus, 0)
            )
            * CASE
                  WHEN e.status = 'ACTIVE' THEN 1
                  ELSE 0
              END
        )
        - NVL(
              (
                  SELECT SUM(s.discount_amt)
                  FROM qt_fmt_sales s
                  WHERE s.emp_id = e.emp_id
              ),
              0
          )
    )                                                                        AS absurd_calc_1,
    CASE
        WHEN EXISTS (
            SELECT 1
            FROM qt_fmt_sales s
            WHERE s.emp_id = e.emp_id
              AND (
                    s.channel_code = 'ONLINE'
                    OR (
                           s.channel_code = 'DIRECT'
                       AND (
                               s.product_code IN ('P3', 'P4', 'P5')
                               OR (
                                      s.product_code = 'P2'
                                  AND s.sale_date >= DATE '2024-03-01'
                                  )
                           )
                       )
                    OR (
                           s.channel_code = 'PARTNER'
                       AND NOT EXISTS (
                               SELECT 1
                               FROM qt_fmt_sales z
                               WHERE z.emp_id = s.emp_id
                                 AND z.sale_date > s.sale_date
                                 AND z.product_code = s.product_code
                           )
                       )
                  )
        ) THEN
            'COMPLEX_MATCH'
        ELSE
            'NO_MATCH'
    END                                                                      AS absurd_flag
FROM qt_fmt_emp e
JOIN qt_fmt_dept d
    ON d.dept_id = e.dept_id
ORDER BY e.emp_id;

--------------------------------------------------------------------------------
-- QUERY 12: correlated subquery + HAVING + set operators
--------------------------------------------------------------------------------
(
    SELECT
        e.dept_id,
        'HIGH'                                                               AS bucket,
        COUNT(*)                                                             AS cnt
    FROM qt_fmt_emp e
    WHERE e.salary >= (
        SELECT AVG(x.salary)
        FROM qt_fmt_emp x
        WHERE x.dept_id = e.dept_id
    )
    GROUP BY e.dept_id
    HAVING COUNT(*) > 0
)
UNION ALL
(
    SELECT
        e.dept_id,
        'LOW'                                                                AS bucket,
        COUNT(*)                                                             AS cnt
    FROM qt_fmt_emp e
    WHERE e.salary < (
        SELECT AVG(x.salary)
        FROM qt_fmt_emp x
        WHERE x.dept_id = e.dept_id
    )
    GROUP BY e.dept_id
    HAVING COUNT(*) > 0
)
MINUS
(
    SELECT
        9999                                                                 AS dept_id,
        'LOW'                                                                AS bucket,
        0                                                                    AS cnt
    FROM dual
)
ORDER BY 1, 2;

--------------------------------------------------------------------------------
-- FINAL SANITY QUERY
--------------------------------------------------------------------------------
SELECT
    'DEPT=' || d.dept_name
    || ' | EMP=' || e.emp_name
    || ' | SALES=' || TO_CHAR(
           NVL(
               (
                   SELECT SUM((s.qty * s.unit_price) - s.discount_amt + s.tax_amt)
                   FROM qt_fmt_sales s
                   WHERE s.emp_id = e.emp_id
               ),
               0
           )
       )
    || ' | JSON_LEVEL=' || JSON_VALUE(e.json_profile, '$.level' RETURNING VARCHAR2(30))
    || ' | HIER=' || (
           SELECT MAX(SYS_CONNECT_BY_PATH(x.dept_code, '/'))
           FROM qt_fmt_dept x
           START WITH x.dept_id = d.dept_id
           CONNECT BY PRIOR x.parent_dept_id = x.dept_id
       )                                                                     AS summary_line
FROM qt_fmt_emp e
JOIN qt_fmt_dept d
    ON d.dept_id = e.dept_id
ORDER BY e.emp_id;
/

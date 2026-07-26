--------------------------------------------------------------------------------
-- Oracle SQL / PL/SQL HARDCORE editor-engine stress suite.
-- Live target: Oracle AI Database Free 26ai (23.26.0.0.0).
-- Run from the repository root with SYSTEM connected to the FREE service.
--
-- Purpose: DELIBERATELY hostile-but-legal grammar that pushes the completion,
-- auto-formatting, and syntax-highlighting engines far past the gentle coverage
-- of test/final.sql. Everything here still parses and executes on the live
-- server so the formatted output can be re-executed for certification.
--
-- Standalone: creates only isolated SQ_HARD_* objects, verifies every result it
-- depends on, and can be run repeatedly.
--------------------------------------------------------------------------------

SET SERVEROUTPUT ON SIZE UNLIMITED
SET FEEDBACK ON
SET DEFINE OFF
WHENEVER SQLERROR EXIT SQL.SQLCODE ROLLBACK

PROMPT [ORACLE HARDCORE] hostile-but-legal grammar stress

BEGIN
  FOR ddl_text IN (
    SELECT 'NOAUDIT POLICY sq_hard_w7_pol' text_value FROM dual UNION ALL
    SELECT 'DROP AUDIT POLICY sq_hard_w7_pol' FROM dual UNION ALL
    SELECT 'DISASSOCIATE STATISTICS FROM PACKAGES sq_hard_w7_ctx_pkg' FROM dual UNION ALL
    SELECT 'DROP DIMENSION sq_hard_w7_prod_dim' FROM dual UNION ALL
    SELECT 'DROP DIMENSION sq_hard_w7_time_dim' FROM dual UNION ALL
    SELECT 'DROP CONTEXT sq_hard_w7_ctx' FROM dual UNION ALL
    SELECT 'DROP PACKAGE sq_hard_w7_ctx_pkg' FROM dual UNION ALL
    SELECT 'DROP TABLE sq_hard_w7_sales PURGE' FROM dual UNION ALL
    SELECT 'DROP TABLE sq_hard_w7_prod PURGE' FROM dual UNION ALL
    SELECT 'DROP TABLE sq_hard_w7_cat PURGE' FROM dual UNION ALL
    SELECT 'DROP TABLE sq_hard_w7_time PURGE' FROM dual UNION ALL
    SELECT 'DROP TABLE sq_hard_w7_arch PURGE' FROM dual UNION ALL
    SELECT 'DROP TABLE sq_hard_w7_pos PURGE' FROM dual UNION ALL
    SELECT 'DROP TABLE sq_hard_w7_src PURGE' FROM dual UNION ALL
    SELECT 'DROP TABLE sq_hard_w7_xml PURGE' FROM dual UNION ALL
    SELECT 'DROP TABLE sq_hard_w7_note PURGE' FROM dual UNION ALL
    SELECT 'DROP TYPE sq_hard_w7_num_bag FORCE' FROM dual UNION ALL
    SELECT 'DROP TYPE sq_hard_w7_num_tab FORCE' FROM dual UNION ALL
    SELECT 'DROP TYPE sq_hard_w7_str_tab FORCE' FROM dual UNION ALL
    SELECT 'DROP ANALYTIC VIEW sq_hard_av' FROM dual UNION ALL
    SELECT 'DROP HIERARCHY sq_hard_node_h' FROM dual UNION ALL
    SELECT 'DROP ATTRIBUTE DIMENSION sq_hard_node_ad' FROM dual UNION ALL
    SELECT 'DROP PACKAGE sq_hard_ptf_pkg' FROM dual UNION ALL
    SELECT 'DROP PROCEDURE sq_hard_autolog' FROM dual UNION ALL
    SELECT 'DROP PROCEDURE sq_hard_w5_guarded' FROM dual UNION ALL
    SELECT 'DROP PACKAGE sq_hard_w5_pkg' FROM dual UNION ALL
    SELECT 'DROP FUNCTION sq_hard_w5_scale' FROM dual UNION ALL
    SELECT 'TRUNCATE TABLE sq_hard_w5_gtt' FROM dual UNION ALL
    SELECT 'DROP TABLE sq_hard_w5_gtt PURGE' FROM dual UNION ALL
    SELECT 'DROP TABLE ora$ptt_sq_hard_w5' FROM dual UNION ALL
    SELECT 'DROP TABLE sq_hard_w5_err PURGE' FROM dual UNION ALL
    SELECT 'DROP TRIGGER sq_hard_w6_iov_late' FROM dual UNION ALL
    SELECT 'DROP TRIGGER sq_hard_w6_iov_trg' FROM dual UNION ALL
    SELECT 'DROP VIEW sq_hard_w6_iov' FROM dual UNION ALL
    SELECT 'DROP VIEW sq_hard_w6_checked' FROM dual UNION ALL
    SELECT 'DROP VIEW sq_hard_w6_frozen' FROM dual UNION ALL
    SELECT 'DROP SYNONYM sq_hard_w6_syn' FROM dual UNION ALL
    SELECT 'DROP MATERIALIZED VIEW sq_hard_w6_mv' FROM dual UNION ALL
    SELECT 'DROP MATERIALIZED VIEW LOG ON sq_hard_w6_fact' FROM dual UNION ALL
    SELECT 'DROP TABLE sq_hard_w6_child PURGE' FROM dual UNION ALL
    SELECT 'DROP TABLE sq_hard_w6_fact PURGE' FROM dual UNION ALL
    SELECT 'DROP PACKAGE sq_hard_w6_reuse' FROM dual UNION ALL
    SELECT 'DROP PACKAGE sq_hard_w6_types' FROM dual UNION ALL
    SELECT 'DROP FUNCTION sq_hard_w6_pct' FROM dual UNION ALL
    SELECT 'DROP FUNCTION sq_hard_w6_span' FROM dual UNION ALL
    SELECT 'DROP TABLE sq_hard_w6_clustered PURGE' FROM dual UNION ALL
    SELECT 'DROP CLUSTER sq_hard_w6_cl INCLUDING TABLES' FROM dual UNION ALL
    SELECT 'DROP TABLE sq_hard_w6_iot PURGE' FROM dual UNION ALL
    SELECT 'DROP TABLE sq_hard_w6_recycle PURGE' FROM dual UNION ALL
    SELECT 'DROP TABLE sq_hard_w6_ddl PURGE' FROM dual UNION ALL
    SELECT 'DROP TABLE sq_hard_w6_ver PURGE' FROM dual UNION ALL
    SELECT 'DROP SEQUENCE sq_hard_w6_seq' FROM dual UNION ALL
    SELECT 'DROP TABLE sq_hard_w6_note PURGE' FROM dual UNION ALL
    SELECT 'PURGE RECYCLEBIN' FROM dual UNION ALL
    SELECT 'DROP TABLE sq_hard_w5_note PURGE' FROM dual UNION ALL
    SELECT 'DROP TABLE sq_hard_w5_wallet PURGE' FROM dual UNION ALL
    SELECT 'DROP TABLE sq_hard_w5_bulk PURGE' FROM dual UNION ALL
    SELECT 'DROP TYPE sq_hard_w5_money_t FORCE' FROM dual UNION ALL
    SELECT 'DROP TABLE sq_hard_bag PURGE' FROM dual UNION ALL
    SELECT 'DROP TABLE sq_hard_sales PURGE' FROM dual UNION ALL
    SELECT 'DROP TABLE sq_hard_ledger PURGE' FROM dual UNION ALL
    SELECT 'DROP DOMAIN sq_hard_share_d' FROM dual UNION ALL
    SELECT 'DROP TABLE sq_hard_node_dim PURGE' FROM dual UNION ALL
    SELECT 'DROP TABLE sq_hard_w3_log PURGE' FROM dual UNION ALL
    SELECT 'DROP TABLE sq_hard_w4_temporal PURGE' FROM dual UNION ALL
    SELECT 'DROP TABLE sq_hard_w4_source PURGE' FROM dual UNION ALL
    SELECT 'DROP TABLE sq_hard_w4_target PURGE' FROM dual UNION ALL
    SELECT 'DROP PROPERTY GRAPH sq_hard_graph' FROM dual UNION ALL
    SELECT 'DROP VIEW sq_hard_dv' FROM dual UNION ALL
    SELECT 'DROP MATERIALIZED VIEW sq_hard_mv' FROM dual UNION ALL
    SELECT 'DROP VIEW sq_hard_v' FROM dual UNION ALL
    SELECT 'DROP FUNCTION sq_hard_pipe' FROM dual UNION ALL
    SELECT 'DROP FUNCTION sq_hard_topn' FROM dual UNION ALL
    SELECT 'DROP FUNCTION sq_hard_taxed' FROM dual UNION ALL
    SELECT 'DROP PACKAGE sq_hard_pkg' FROM dual UNION ALL
    SELECT 'DROP TYPE sq_hard_point3_t FORCE' FROM dual UNION ALL
    SELECT 'DROP TYPE sq_hard_point_t FORCE' FROM dual UNION ALL
    SELECT 'DROP TYPE sq_hard_num_tab FORCE' FROM dual UNION ALL
    SELECT 'DROP TABLE sq_hard_merge PURGE' FROM dual UNION ALL
    SELECT 'DROP TABLE sq_hard_edge PURGE' FROM dual UNION ALL
    SELECT 'DROP TABLE sq_hard_split_hi PURGE' FROM dual UNION ALL
    SELECT 'DROP TABLE sq_hard_split_lo PURGE' FROM dual UNION ALL
    SELECT 'DROP TABLE sq_hard_metric PURGE' FROM dual UNION ALL
    SELECT 'DROP TABLE sq_hard_metric_audit PURGE' FROM dual UNION ALL
    SELECT 'DROP TABLE sq_hard_quoted PURGE' FROM dual UNION ALL
    SELECT 'DROP TABLE sq_hard_doc PURGE' FROM dual
  ) LOOP
    BEGIN
      EXECUTE IMMEDIATE ddl_text.text_value;
    EXCEPTION
      WHEN OTHERS THEN NULL;
    END;
  END LOOP;
END;
/

--------------------------------------------------------------------------------
-- Quoted identifiers that collide with reserved words, embed spaces, doubled
-- quotes, and $/# characters. Highlighting and completion must treat these as
-- identifiers, never keywords.
--------------------------------------------------------------------------------
CREATE TABLE sq_hard_quoted (
  "SELECT"                 NUMBER,
  "FROM"                   VARCHAR2(30),
  "Group By"               DATE,
  "order"                  NUMBER,
  "x$weird#col$"           NUMBER,
  "Column With Spaces"     VARCHAR2(30),
  CONSTRAINT "sq_hard_quoted$pk" PRIMARY KEY ("SELECT")
) TABLESPACE users;

INSERT INTO sq_hard_quoted (
  "SELECT", "FROM", "Group By", "order", "x$weird#col$", "Column With Spaces"
) VALUES (1, 'from-value', DATE '2024-02-29', 2, 3, 'q"q');

SELECT q."SELECT" + q."order"                      AS "Total Sum",
       q."FROM" || '/' || q."Column With Spaces"   AS joined_text,
       EXTRACT(YEAR FROM q."Group By")             AS leap_year
FROM sq_hard_quoted q
WHERE q."SELECT" = 1 AND q."order" BETWEEN 1 AND 9;

--------------------------------------------------------------------------------
-- Deeply nested inline views + scalar subqueries + optimizer hints + comments
-- interleaved mid-statement (block and line).
--------------------------------------------------------------------------------
SELECT /* torture: deep nesting */ deep.total,
       (SELECT MAX(LEVEL) FROM dual CONNECT BY LEVEL <= deep.total) AS max_level
FROM (
  SELECT /*+ NO_MERGE */
         (SELECT COUNT(*) -- innermost scalar
          FROM (SELECT 3 c FROM (SELECT 2 b FROM (SELECT 1 a FROM dual) t3) t2) t1
         ) AS total
  FROM dual
) deep;

--------------------------------------------------------------------------------
-- Operator adjacency without whitespace, alternative-quoted and national
-- literals with nested delimiters, float/scientific suffixes.
--------------------------------------------------------------------------------
SELECT 1+2*3-4/2                          AS arithmetic_value,
       'a'||'b'||'c'||'d'                 AS concat_chain,
       q'{brace {nested} literal}'        AS q_brace,
       q'!bang ' apostrophe!'             AS q_bang,
       nq'[national <<quote>>]'           AS q_national,
       3.14f                              AS binary_float_lit,
       2.71d                              AS binary_double_lit,
       0.5e-3                             AS scientific_lit,
       CASE WHEN 5<>6 AND 7<=8 AND 9>=9 THEN 'Y' ELSE 'N' END AS comparison_chain
FROM dual;

--------------------------------------------------------------------------------
-- Analytic RANGE window over DATE with an INTERVAL bound, nested CASE inside the
-- projection, DECODE / NVL2 / NULLIF stacked.
--------------------------------------------------------------------------------
CREATE TABLE sq_hard_metric (
  metric_id   NUMBER CONSTRAINT sq_hard_metric_pk PRIMARY KEY,
  node_id     NUMBER NOT NULL,
  metric_day  DATE   NOT NULL,
  metric_name VARCHAR2(32) NOT NULL,
  metric_value NUMBER(12,2) NOT NULL,
  payload     JSON
) TABLESPACE users;

INSERT INTO sq_hard_metric VALUES (1, 1, DATE '2026-01-01', 'LATENCY', 12,
  JSON_OBJECT('tags' VALUE JSON_ARRAY('sql', 'manual')));
INSERT INTO sq_hard_metric VALUES (2, 1, DATE '2026-01-03', 'LATENCY', 18,
  JSON_OBJECT('tags' VALUE JSON_ARRAY('query')));
INSERT INTO sq_hard_metric VALUES (3, 1, DATE '2026-01-08', 'LATENCY', 24,
  JSON_OBJECT('tags' VALUE JSON_ARRAY('json', 'vector')));
INSERT INTO sq_hard_metric VALUES (4, 2, DATE '2026-01-02', 'ERRORS', 2,
  JSON_OBJECT('tags' VALUE JSON_ARRAY('routine')));

--------------------------------------------------------------------------------
-- Compound-trigger state shared across timing points. The no-op UPDATE is
-- intentional: it makes INSERTING/UPDATING/DELETING, :OLD/:NEW, associative
-- arrays, and statement/row timing sections executable rather than decorative.
--------------------------------------------------------------------------------
CREATE TABLE sq_hard_metric_audit (
  statement_no    NUMBER GENERATED ALWAYS AS IDENTITY,
  action_name     VARCHAR2(10) NOT NULL,
  changed_rows    NUMBER NOT NULL,
  sample_metric_id NUMBER NOT NULL,
  CONSTRAINT sq_hard_metric_audit_pk PRIMARY KEY (statement_no)
) TABLESPACE users;

CREATE OR REPLACE TRIGGER sq_hard_metric_ct
FOR INSERT OR UPDATE OF metric_value OR DELETE ON sq_hard_metric
COMPOUND TRIGGER
  TYPE metric_id_tab IS TABLE OF sq_hard_metric.metric_id%TYPE
    INDEX BY PLS_INTEGER;
  g_metric_ids metric_id_tab;
  g_row_count  PLS_INTEGER := 0;
  g_action     VARCHAR2(10);

  BEFORE STATEMENT IS
  BEGIN
    g_metric_ids.DELETE;
    g_row_count := 0;
    IF INSERTING THEN
      g_action := 'INSERT';
    ELSIF UPDATING THEN
      g_action := 'UPDATE';
    ELSE
      g_action := 'DELETE';
    END IF;
  END BEFORE STATEMENT;

  AFTER EACH ROW IS
  BEGIN
    g_row_count := g_row_count + 1;
    IF INSERTING OR UPDATING THEN
      g_metric_ids(g_row_count) := :NEW.metric_id;
    ELSE
      g_metric_ids(g_row_count) := :OLD.metric_id;
    END IF;
  END AFTER EACH ROW;

  AFTER STATEMENT IS
    l_sample_metric_id sq_hard_metric.metric_id%TYPE;
  BEGIN
    IF g_row_count > 0 THEN
      l_sample_metric_id := g_metric_ids(g_metric_ids.FIRST);
      INSERT INTO sq_hard_metric_audit (
        action_name, changed_rows, sample_metric_id
      ) VALUES (
        g_action, g_row_count, l_sample_metric_id
      );
    END IF;
  END AFTER STATEMENT;
END sq_hard_metric_ct;
/

UPDATE sq_hard_metric
SET metric_value = metric_value
WHERE node_id = 1;

SELECT metric_id, node_id, metric_day, metric_value,
       CASE WHEN metric_value >
                 (CASE WHEN node_id = 1 THEN 10 ELSE 20 END)
            THEN NVL2(payload, 'has-json',
                      DECODE(node_id, 1, 'one', 2, 'two', 'other'))
            ELSE NULLIF(metric_name, 'MISSING') END        AS nested_branch,
       SUM(metric_value) OVER (
         PARTITION BY node_id ORDER BY metric_day
         RANGE BETWEEN INTERVAL '7' DAY PRECEDING AND CURRENT ROW
       )                                                    AS windowed_sum
FROM sq_hard_metric
ORDER BY node_id, metric_id;

--------------------------------------------------------------------------------
-- Doubly nested JSON_TABLE with NESTED PATH, ordinality, and ON EMPTY defaults.
--------------------------------------------------------------------------------
SELECT m.metric_id, jt.tag_no, jt.tag_value
FROM sq_hard_metric m,
     JSON_TABLE(m.payload, '$'
       COLUMNS (
         NESTED PATH '$.tags[*]' COLUMNS (
           tag_no    FOR ORDINALITY,
           tag_value VARCHAR2(32) PATH '$'
         )
       )) jt
ORDER BY m.metric_id, jt.tag_no;

--------------------------------------------------------------------------------
-- MATCH_RECOGNIZE with a quantified pattern, DEFINE, MEASURES, and PREV/NEXT.
--------------------------------------------------------------------------------
SELECT node_id, match_no, rising_days
FROM sq_hard_metric
MATCH_RECOGNIZE (
  PARTITION BY node_id
  ORDER BY metric_day
  MEASURES MATCH_NUMBER() AS match_no,
           COUNT(rise.*)  AS rising_days
  ONE ROW PER MATCH
  AFTER MATCH SKIP PAST LAST ROW
  PATTERN (strt rise+)
  DEFINE rise AS rise.metric_value > PREV(rise.metric_value)
)
ORDER BY node_id, match_no;

--------------------------------------------------------------------------------
-- Single-statement frame collision: local PL/SQL function + two recursive
-- clauses (SEARCH/CYCLE), JSON_TABLE, inherited data through OUTER APPLY,
-- GROUPING SETS, LISTAGG overflow syntax, JSON aggregation, analytics, and
-- QUALIFY. This deliberately makes the parser restore every parent scope before
-- it can resolve the final aliases.
--------------------------------------------------------------------------------
WITH
  FUNCTION canonical_code(p_text VARCHAR2) RETURN VARCHAR2 IS
  BEGIN
    RETURN UPPER(REGEXP_REPLACE(TRIM(p_text), '[^[:alnum:]]+', '_'));
  END;
  hard_nodes (node_id, parent_node_id, node_name) AS (
    SELECT 1, CAST(NULL AS NUMBER), 'Root Node' FROM dual
    UNION ALL SELECT 2, 1, 'Blue Child' FROM dual
    UNION ALL SELECT 3, 1, 'Green Child' FROM dual
    UNION ALL SELECT 4, 2, 'Leaf Node' FROM dual
  ),
  hard_tree (
    node_id, parent_node_id, node_name, tree_depth, node_path
  ) AS (
    SELECT n.node_id, n.parent_node_id, n.node_name, 0,
           CAST('/' || canonical_code(n.node_name) AS VARCHAR2(400))
    FROM hard_nodes n
    WHERE n.parent_node_id IS NULL
    UNION ALL
    SELECT n.node_id, n.parent_node_id, n.node_name, t.tree_depth + 1,
           t.node_path || '/' || canonical_code(n.node_name)
    FROM hard_nodes n
    JOIN hard_tree t ON t.node_id = n.parent_node_id
  )
  SEARCH DEPTH FIRST BY node_name SET traversal_no
  CYCLE node_id SET cycle_yn TO 'Y' DEFAULT 'N',
  tag_rows (
    metric_id, node_id, metric_name, metric_value, tag_no, tag_value
  ) AS (
    SELECT m.metric_id, m.node_id, m.metric_name, m.metric_value,
           jt.tag_no, jt.tag_value
    FROM sq_hard_metric m
    CROSS APPLY JSON_TABLE(
      m.payload,
      '$.tags[*]' COLUMNS (
        tag_no    FOR ORDINALITY,
        tag_value VARCHAR2(32) PATH '$' NULL ON ERROR
      )
    ) jt
  ),
  metric_analytics AS (
    SELECT m.metric_id, m.node_id, m.metric_day, m.metric_name, m.metric_value,
           SUM(m.metric_value) OVER (
             PARTITION BY m.node_id
             ORDER BY m.metric_day, m.metric_id
             ROWS BETWEEN UNBOUNDED PRECEDING AND CURRENT ROW
           ) AS running_value,
           DENSE_RANK() OVER (
             PARTITION BY m.node_id
             ORDER BY m.metric_value DESC, m.metric_id
           ) AS value_rank
    FROM sq_hard_metric m
  ),
  metric_rollup (
    node_id, metric_name, metric_total, grouping_value
  ) AS (
    SELECT node_id, metric_name, SUM(metric_value),
           GROUPING_ID(node_id, metric_name)
    FROM sq_hard_metric
    GROUP BY GROUPING SETS (
      (node_id, metric_name),
      (node_id),
      ()
    )
  ),
  final_rows AS (
    SELECT t.node_id, t.parent_node_id, t.node_name, t.tree_depth,
           t.node_path, t.cycle_yn,
           top_metric.metric_id, top_metric.metric_name,
           top_metric.metric_value, top_metric.running_value,
           (SELECT LISTAGG(
                     r.tag_value,
                     ',' ON OVERFLOW TRUNCATE '...' WITH COUNT
                   ) WITHIN GROUP (
                     ORDER BY r.metric_id, r.tag_no
                   )
            FROM tag_rows r
            WHERE r.node_id = t.node_id) AS tag_list,
           (SELECT JSON_ARRAYAGG(
                     r.tag_value
                     ORDER BY r.metric_id, r.tag_no
                     RETURNING CLOB
                   )
            FROM tag_rows r
            WHERE r.node_id = t.node_id) AS tag_json
    FROM hard_tree t
    OUTER APPLY (
      SELECT a.metric_id, a.metric_name, a.metric_value, a.running_value
      FROM metric_analytics a
      WHERE a.node_id = t.node_id
      ORDER BY a.value_rank, a.metric_id
      FETCH FIRST 1 ROW ONLY
    ) top_metric
  )
SELECT CASE
         WHEN (SELECT COUNT(*) FROM hard_tree) = 4
          AND (SELECT COUNT(*) FROM tag_rows) = 6
          AND (SELECT metric_total
               FROM metric_rollup
               WHERE grouping_value = 3) = 56
         THEN 'PASS'
         ELSE TO_CHAR(1 / 0)
       END AS integrated_status,
       f.node_id, f.node_name, f.tree_depth, f.node_path,
       f.metric_name, f.metric_value, f.running_value, f.tag_list,
       JSON_OBJECT(
         'cycle' VALUE f.cycle_yn,
         'tags' VALUE f.tag_json FORMAT JSON,
         'grandTotal' VALUE (
           SELECT metric_total
           FROM metric_rollup
           WHERE grouping_value = 3
         )
         RETURNING CLOB
       ) AS evidence
FROM final_rows f
QUALIFY ROW_NUMBER() OVER (
  PARTITION BY NVL(f.parent_node_id, -1)
  ORDER BY f.node_id
) = 1
ORDER BY f.node_id;
/

--------------------------------------------------------------------------------
-- Recursive WITH carrying a PL/SQL WITH FUNCTION, hierarchical CONNECT BY, and
-- a MODEL rule in the same script.
--------------------------------------------------------------------------------
WITH
  FUNCTION sq_hard_double(p NUMBER) RETURN NUMBER IS
  BEGIN
    RETURN p * 2;
  END;
  counter (n) AS (
    SELECT 1 FROM dual
    UNION ALL
    SELECT n + 1 FROM counter WHERE n < 5
  )
SELECT n, sq_hard_double(n) AS doubled
FROM counter
ORDER BY n;
/

SELECT metric_id, node_id, metric_value
FROM sq_hard_metric
MODEL
  PARTITION BY (node_id)
  DIMENSION BY (metric_id)
  MEASURES (metric_value)
  RULES UPSERT SEQUENTIAL ORDER (
    metric_value[999] = MAX(metric_value)[ANY] + 1
  )
ORDER BY node_id, metric_id;

--------------------------------------------------------------------------------
-- XMLTABLE with namespaces, PIVOT, and a set operator chain with parentheses.
--------------------------------------------------------------------------------
SELECT x.seq, x.label
FROM XMLTABLE(
       XMLNAMESPACES ('urn:sq:hard' AS "h"),
       '/h:root/h:item'
       PASSING XMLTYPE(
         '<h:root xmlns:h="urn:sq:hard">' ||
         '<h:item seq="1">alpha</h:item>' ||
         '<h:item seq="2">beta</h:item></h:root>')
       COLUMNS
         seq   NUMBER       PATH '@seq',
         label VARCHAR2(16) PATH 'text()'
     ) x
ORDER BY x.seq;

SELECT *
FROM (SELECT node_id, metric_name, metric_value FROM sq_hard_metric)
PIVOT (SUM(metric_value) AS total
       FOR metric_name IN ('LATENCY' AS lat, 'ERRORS' AS err))
ORDER BY node_id;

(SELECT node_id FROM sq_hard_metric WHERE metric_value >= 10)
INTERSECT
(SELECT node_id FROM sq_hard_metric WHERE metric_name = 'LATENCY')
MINUS
(SELECT node_id FROM sq_hard_metric WHERE metric_name = 'NEVER')
ORDER BY node_id;

--------------------------------------------------------------------------------
-- PL/SQL: pipelined table function over a collection type.
--------------------------------------------------------------------------------
CREATE OR REPLACE TYPE sq_hard_num_tab AS TABLE OF NUMBER;
/
CREATE OR REPLACE FUNCTION sq_hard_pipe(p_n PLS_INTEGER)
  RETURN sq_hard_num_tab PIPELINED
IS
BEGIN
  FOR i IN 1 .. p_n LOOP
    PIPE ROW (i * i);
  END LOOP;
  RETURN;
END sq_hard_pipe;
/
SELECT COLUMN_VALUE AS squared
FROM TABLE(sq_hard_pipe(4))
ORDER BY 1;

--------------------------------------------------------------------------------
-- PL/SQL: quoted keyword identifiers, nested labelled blocks, %ROWTYPE
-- associative array, BULK COLLECT, FORALL with SAVE EXCEPTIONS, conditional
-- compilation, and a REF CURSOR.
--------------------------------------------------------------------------------
CREATE OR REPLACE PACKAGE sq_hard_pkg AUTHID DEFINER AS
  TYPE metric_rows IS TABLE OF sq_hard_metric%ROWTYPE INDEX BY PLS_INTEGER;
  TYPE metric_ref  IS REF CURSOR RETURN sq_hard_metric%ROWTYPE;
  FUNCTION bulk_total RETURN NUMBER;
END sq_hard_pkg;
/
CREATE OR REPLACE PACKAGE BODY sq_hard_pkg AS
  FUNCTION bulk_total RETURN NUMBER IS
    l_rows   metric_rows;
    l_total  NUMBER := 0;
    "BEGIN"  PLS_INTEGER := 0;
    l_cursor metric_ref;
    l_one    sq_hard_metric%ROWTYPE;
  BEGIN
    <<load_block>>
    BEGIN
      SELECT * BULK COLLECT INTO l_rows FROM sq_hard_metric ORDER BY metric_id;
      FORALL i IN 1 .. l_rows.COUNT SAVE EXCEPTIONS
        UPDATE sq_hard_metric SET metric_value = metric_value
        WHERE metric_id = l_rows(i).metric_id;
    END load_block;

    FOR i IN 1 .. l_rows.COUNT LOOP
      "BEGIN" := "BEGIN" + 1;
      l_total := l_total + l_rows(i).metric_value;
    END LOOP;

    OPEN l_cursor FOR SELECT * FROM sq_hard_metric WHERE ROWNUM <= 1;
    FETCH l_cursor INTO l_one;
    CLOSE l_cursor;

    $IF DBMS_DB_VERSION.VER_LE_11 $THEN
      l_total := l_total;
    $ELSE
      l_total := l_total + 0;
    $END

    RETURN l_total + "BEGIN" * 0;
  END bulk_total;
END sq_hard_pkg;
/

--------------------------------------------------------------------------------
-- Multi-branch MERGE with a matched DELETE clause and conditional INSERT.
--------------------------------------------------------------------------------
CREATE TABLE sq_hard_merge (
  k NUMBER CONSTRAINT sq_hard_merge_pk PRIMARY KEY,
  v NUMBER,
  tag VARCHAR2(10)
) TABLESPACE users;

INSERT INTO sq_hard_merge
SELECT LEVEL, LEVEL * 10, 'init' FROM dual CONNECT BY LEVEL <= 5;

MERGE INTO sq_hard_merge t
USING (SELECT LEVEL k, LEVEL * 100 v FROM dual CONNECT BY LEVEL <= 7) s
ON (t.k = s.k)
WHEN MATCHED THEN UPDATE SET t.v = s.v, t.tag = 'upd'
  DELETE WHERE t.v < 300
WHEN NOT MATCHED THEN INSERT (k, v, tag) VALUES (s.k, s.v, 'ins')
  WHERE s.k > 5;

--------------------------------------------------------------------------------
-- View + materialized view built on the torture tables, then a flashback query.
--------------------------------------------------------------------------------
CREATE OR REPLACE VIEW sq_hard_v AS
SELECT node_id, COUNT(*) AS metric_count, SUM(metric_value) AS total_value
FROM sq_hard_metric
GROUP BY node_id
WITH READ ONLY;

CREATE MATERIALIZED VIEW sq_hard_mv
  BUILD IMMEDIATE REFRESH COMPLETE ON DEMAND
  AS SELECT node_id, MAX(metric_value) AS peak FROM sq_hard_metric GROUP BY node_id;

--------------------------------------------------------------------------------
-- 23ai surface: BOOLEAN column (never projected raw), VECTOR similarity,
-- XMLTYPE storage, DEFAULT ON NULL, Unicode quoted identifier, and UNISTR /
-- national literals in one table.
--------------------------------------------------------------------------------
CREATE TABLE IF NOT EXISTS sq_hard_doc (
  doc_id       NUMBER DEFAULT ON NULL 0 NOT NULL,
  "한글 컬럼"  NVARCHAR2(40),
  is_hot       BOOLEAN NOT NULL,
  body_xml     XMLTYPE,
  embedding    VECTOR(3, FLOAT32),
  CONSTRAINT sq_hard_doc_pk PRIMARY KEY (doc_id)
) TABLESPACE users;

INSERT INTO sq_hard_doc (doc_id, "한글 컬럼", is_hot, body_xml, embedding) VALUES
  (1, N'첫번째 문서', TRUE,
   XMLTYPE('<doc><v>10</v><v>30</v><v>20</v></doc>'),
   TO_VECTOR('[1.0, 2.0, 2.0]')),
  (2, UNISTR('\B450\BC88\C9F8'), FALSE,
   XMLTYPE('<doc><v>5</v></doc>'),
   TO_VECTOR('[0.0, 3.0, 4.0]'));

SELECT d.doc_id,
       d."한글 컬럼"                                     AS unicode_name,
       CASE WHEN d.is_hot THEN 'hot' ELSE 'cold' END     AS heat,
       ROUND(VECTOR_DISTANCE(d.embedding,
                             TO_VECTOR('[1.0, 2.0, 2.0]'),
                             EUCLIDEAN), 4)              AS vec_dist,
       XMLCAST(XMLQUERY('sum(/doc/v)' PASSING d.body_xml
                        RETURNING CONTENT) AS NUMBER)    AS xml_sum
FROM sq_hard_doc d
WHERE d.is_hot OR NOT d.is_hot
ORDER BY d.doc_id;

--------------------------------------------------------------------------------
-- XQuery FLWOR inside XMLQUERY, XMLEXISTS predicate, and indented
-- XMLSERIALIZE over aggregated XMLELEMENT/XMLFOREST/XMLAGG output.
--------------------------------------------------------------------------------
SELECT XMLSERIALIZE(
         CONTENT XMLELEMENT("metrics",
           XMLAGG(
             XMLELEMENT("m",
               XMLFOREST(m.node_id AS "node", m.metric_value AS "value"))
             ORDER BY m.metric_id))
         AS CLOB INDENT SIZE = 2)                        AS xml_report
FROM sq_hard_metric m;

SELECT XMLCAST(
         XMLQUERY('for $v in /doc/v where xs:integer($v) > 5 order by xs:integer($v) descending return $v'
                  PASSING d.body_xml RETURNING CONTENT
         ) AS VARCHAR2(40))                              AS flwor_values
FROM sq_hard_doc d
WHERE XMLEXISTS('/doc/v[. > 25]' PASSING d.body_xml);

--------------------------------------------------------------------------------
-- Multi-table conditional INSERT FIRST with overlapping WHEN branches and an
-- ELSE bucket fed by a correlated source query.
--------------------------------------------------------------------------------
CREATE TABLE sq_hard_split_hi (
  metric_id NUMBER PRIMARY KEY,
  bucket_tag VARCHAR2(8) NOT NULL
) TABLESPACE users;
CREATE TABLE sq_hard_split_lo (
  metric_id NUMBER PRIMARY KEY,
  bucket_tag VARCHAR2(8) NOT NULL
) TABLESPACE users;

INSERT FIRST
  WHEN src_value >= 20 THEN
    INTO sq_hard_split_hi (metric_id, bucket_tag) VALUES (src_id, 'peak')
  WHEN src_value >= 15 THEN
    INTO sq_hard_split_hi (metric_id, bucket_tag) VALUES (src_id, 'high')
  ELSE
    INTO sq_hard_split_lo (metric_id, bucket_tag) VALUES (src_id, 'low')
SELECT m.metric_id AS src_id, m.metric_value AS src_value
FROM sq_hard_metric m;

--------------------------------------------------------------------------------
-- Object-relational: NOT FINAL supertype, subtype with OVERRIDING member, MAP
-- ordering, TREAT / IS OF (TYPE ...) narrowing, and method calls through a
-- table alias.
--------------------------------------------------------------------------------
CREATE OR REPLACE TYPE sq_hard_point_t AS OBJECT (
  x NUMBER,
  y NUMBER,
  MEMBER FUNCTION norm RETURN NUMBER,
  MAP MEMBER FUNCTION sort_key RETURN NUMBER
) NOT FINAL;
/
CREATE OR REPLACE TYPE BODY sq_hard_point_t AS
  MEMBER FUNCTION norm RETURN NUMBER IS
  BEGIN
    RETURN SQRT(x * x + y * y);
  END norm;
  MAP MEMBER FUNCTION sort_key RETURN NUMBER IS
  BEGIN
    RETURN SELF.norm();
  END sort_key;
END;
/
CREATE OR REPLACE TYPE sq_hard_point3_t UNDER sq_hard_point_t (
  z NUMBER,
  OVERRIDING MEMBER FUNCTION norm RETURN NUMBER
);
/
CREATE OR REPLACE TYPE BODY sq_hard_point3_t AS
  OVERRIDING MEMBER FUNCTION norm RETURN NUMBER IS
  BEGIN
    RETURN SQRT(x * x + y * y + z * z);
  END norm;
END;
/

SELECT t.tag_name,
       t.pt.norm()                                        AS point_norm,
       CASE WHEN t.pt IS OF (ONLY sq_hard_point3_t)
            THEN TREAT(t.pt AS sq_hard_point3_t).z
            ELSE NULL END                                 AS depth_z,
       lat.echo_norm
FROM (SELECT 'flat' tag_name, sq_hard_point_t(3, 4) pt FROM dual
      UNION ALL
      SELECT 'deep', sq_hard_point3_t(3, 4, 12) FROM dual) t
CROSS JOIN LATERAL (SELECT t.pt.norm() AS echo_norm FROM dual) lat
ORDER BY t.tag_name;

--------------------------------------------------------------------------------
-- Collection algebra over the pipelined type: MULTISET UNION DISTINCT /
-- INTERSECT / EXCEPT, SUBMULTISET, MEMBER OF, IS A SET, CARDINALITY, and
-- CAST(COLLECT(...)) reduced back to a scalar.
--------------------------------------------------------------------------------
SELECT CARDINALITY(sq_hard_num_tab(1, 2, 3)
                   MULTISET UNION DISTINCT sq_hard_num_tab(3, 4)) AS union_card,
       CARDINALITY(sq_hard_num_tab(1, 2, 3)
                   MULTISET INTERSECT sq_hard_num_tab(2, 3, 9))   AS intersect_card,
       CARDINALITY(sq_hard_num_tab(1, 2, 2, 3)
                   MULTISET EXCEPT sq_hard_num_tab(2))            AS except_card,
       CASE WHEN sq_hard_num_tab(1, 2)
                 SUBMULTISET OF sq_hard_num_tab(1, 2, 3) THEN 'sub' END AS sub_flag,
       CASE WHEN 2 MEMBER OF sq_hard_num_tab(1, 2, 3) THEN 'member' END AS member_flag,
       CASE WHEN sq_hard_num_tab(1, 2, 3) IS A SET THEN 'set' END AS set_flag,
       (SELECT CARDINALITY(CAST(COLLECT(CAST(m.metric_value AS NUMBER)
                                        ORDER BY m.metric_id)
                                AS sq_hard_num_tab))
        FROM sq_hard_metric m)                                    AS collected_card
FROM dual;

--------------------------------------------------------------------------------
-- 23ai table value constructor feeding CONNECT BY with PRIOR on both ends,
-- CONNECT_BY_ROOT, SYS_CONNECT_BY_PATH, CONNECT_BY_ISLEAF, NOCYCLE, and
-- ORDER SIBLINGS BY.
--------------------------------------------------------------------------------
SELECT LPAD(' ', 2 * (LEVEL - 1)) || v.step_name          AS indented_step,
       CONNECT_BY_ROOT v.step_name                        AS root_step,
       SYS_CONNECT_BY_PATH(v.step_name, '>')              AS step_path,
       CONNECT_BY_ISLEAF                                  AS leaf_yn,
       LEVEL                                              AS tree_level
FROM (VALUES (1, NULL, 'plan'),
             (2, 1,    'parse'),
             (3, 1,    'bind'),
             (4, 2,    'optimize')) v (step_id, parent_step_id, step_name)
START WITH v.parent_step_id IS NULL
CONNECT BY NOCYCLE PRIOR v.step_id = v.parent_step_id
ORDER SIBLINGS BY v.step_name;

--------------------------------------------------------------------------------
-- PIVOT with two aggregates and aliases per cell, then a multi-column UNPIVOT
-- INCLUDE NULLS over the same shape, and percent row limiting WITH TIES.
--------------------------------------------------------------------------------
SELECT node_id,
       lat_total, lat_ct, err_total, err_ct
FROM (SELECT node_id, metric_name, metric_value FROM sq_hard_metric)
PIVOT (SUM(metric_value) AS total, COUNT(*) AS ct
       FOR metric_name IN ('LATENCY' AS lat, 'ERRORS' AS err))
ORDER BY node_id;

SELECT node_id, metric_kind, kind_total, kind_ct
FROM (
  SELECT node_id,
         SUM(CASE WHEN metric_name = 'LATENCY' THEN metric_value END) AS lat_total,
         COUNT(CASE WHEN metric_name = 'LATENCY' THEN 1 END)          AS lat_ct,
         SUM(CASE WHEN metric_name = 'ERRORS' THEN metric_value END)  AS err_total,
         COUNT(CASE WHEN metric_name = 'ERRORS' THEN 1 END)           AS err_ct
  FROM sq_hard_metric
  GROUP BY node_id
)
UNPIVOT INCLUDE NULLS (
  (kind_total, kind_ct) FOR metric_kind IN (
    (lat_total, lat_ct) AS 'LATENCY',
    (err_total, err_ct) AS 'ERRORS'
  )
)
ORDER BY node_id, metric_kind;

SELECT metric_id, metric_value
FROM sq_hard_metric
ORDER BY metric_value DESC, metric_id
OFFSET 1 ROW FETCH NEXT 50 PERCENT ROWS WITH TIES;

--------------------------------------------------------------------------------
-- Analytic edge functions: KEEP (DENSE_RANK FIRST/LAST), RATIO_TO_REPORT,
-- CUME_DIST, PERCENT_RANK, NTILE, WIDTH_BUCKET, MEDIAN, LISTAGG DISTINCT,
-- NTH_VALUE FROM LAST IGNORE NULLS, and a GROUPS frame with EXCLUDE.
--------------------------------------------------------------------------------
SELECT metric_id, node_id, metric_value,
       MAX(metric_value) KEEP (DENSE_RANK FIRST ORDER BY metric_day)
         OVER (PARTITION BY node_id)                      AS first_day_value,
       ROUND(RATIO_TO_REPORT(metric_value) OVER (), 4)    AS value_share,
       CUME_DIST() OVER (ORDER BY metric_value)           AS cume_share,
       PERCENT_RANK() OVER (ORDER BY metric_value)        AS pct_rank,
       NTILE(2) OVER (ORDER BY metric_value)              AS half_bucket,
       WIDTH_BUCKET(metric_value, 0, 30, 3)               AS width_bin,
       MEDIAN(metric_value) OVER (PARTITION BY node_id)   AS node_median,
       NTH_VALUE(metric_value, 1) FROM LAST IGNORE NULLS
         OVER (PARTITION BY node_id ORDER BY metric_day
               ROWS BETWEEN UNBOUNDED PRECEDING AND UNBOUNDED FOLLOWING)
                                                          AS last_value_seen,
       SUM(metric_value) OVER (
         ORDER BY metric_value
         GROUPS BETWEEN 1 PRECEDING AND 1 FOLLOWING
         EXCLUDE CURRENT ROW)                             AS neighbor_sum,
       (SELECT LISTAGG(DISTINCT metric_name, '+')
                 WITHIN GROUP (ORDER BY metric_name)
        FROM sq_hard_metric)                              AS name_menu
FROM sq_hard_metric
ORDER BY metric_id;

--------------------------------------------------------------------------------
-- SQL table macro: the macro text itself is an alternative-quoted literal, and
-- the call site must resolve macro columns for completion.
--------------------------------------------------------------------------------
CREATE OR REPLACE FUNCTION sq_hard_topn(p_rows NUMBER)
  RETURN VARCHAR2 SQL_MACRO(TABLE)
IS
BEGIN
  RETURN q'~
    SELECT m.metric_id, m.node_id, m.metric_value
    FROM sq_hard_metric m
    ORDER BY m.metric_value DESC, m.metric_id
    FETCH FIRST p_rows ROWS ONLY
  ~';
END sq_hard_topn;
/

SELECT metric_id, metric_value
FROM sq_hard_topn(p_rows => 2)
ORDER BY metric_id;

--------------------------------------------------------------------------------
-- PRAGMA UDF scalar function used from SQL, with named-notation defaults.
--------------------------------------------------------------------------------
CREATE OR REPLACE FUNCTION sq_hard_taxed(
  p_amount NUMBER,
  p_rate   NUMBER DEFAULT 0.1
) RETURN NUMBER DETERMINISTIC IS
  PRAGMA UDF;
BEGIN
  RETURN ROUND(p_amount * (1 + p_rate), 2);
END sq_hard_taxed;
/

SELECT metric_id,
       sq_hard_taxed(metric_value)                        AS default_taxed,
       sq_hard_taxed(p_rate => 0.5, p_amount => metric_value) AS named_taxed
FROM sq_hard_metric
ORDER BY metric_id;

--------------------------------------------------------------------------------
-- Property graph over metric rows plus an explicit edge table; GRAPH_TABLE
-- arrow patterns are deliberately hostile to tokenizers.
--------------------------------------------------------------------------------
CREATE TABLE sq_hard_edge (
  edge_id NUMBER PRIMARY KEY,
  src_metric_id NUMBER NOT NULL REFERENCES sq_hard_metric (metric_id),
  dst_metric_id NUMBER NOT NULL REFERENCES sq_hard_metric (metric_id),
  hop_cost NUMBER NOT NULL
) TABLESPACE users;

INSERT INTO sq_hard_edge VALUES (1, 1, 2, 5);
INSERT INTO sq_hard_edge VALUES (2, 2, 3, 7);
INSERT INTO sq_hard_edge VALUES (3, 1, 4, 11);

CREATE PROPERTY GRAPH sq_hard_graph
  VERTEX TABLES (
    sq_hard_metric
      KEY (metric_id)
      LABEL metric
      PROPERTIES (metric_id, node_id, metric_value)
  )
  EDGE TABLES (
    sq_hard_edge
      KEY (edge_id)
      SOURCE KEY (src_metric_id) REFERENCES sq_hard_metric (metric_id)
      DESTINATION KEY (dst_metric_id) REFERENCES sq_hard_metric (metric_id)
      LABEL links
      PROPERTIES (hop_cost)
  );

SELECT src_metric, dst_metric, total_cost
FROM GRAPH_TABLE (sq_hard_graph
  MATCH (a IS metric) -[e IS links]-> (b IS metric)
  WHERE a.metric_value < b.metric_value
  COLUMNS (a.metric_id AS src_metric,
           b.metric_id AS dst_metric,
           e.hop_cost  AS total_cost)
)
ORDER BY src_metric, dst_metric;

--------------------------------------------------------------------------------
-- JSON-relational duality view with the 23ai JSON {...} constructor syntax.
--------------------------------------------------------------------------------
CREATE OR REPLACE JSON RELATIONAL DUALITY VIEW sq_hard_dv AS
SELECT JSON {
         '_id'         : m.metric_id,
         'nodeId'      : m.node_id,
         'metricValue' : m.metric_value WITH UPDATE
       }
FROM sq_hard_metric m WITH UPDATE INSERT DELETE;

SELECT JSON_VALUE(data, '$.nodeId' RETURNING NUMBER) AS node_id_from_dv
FROM sq_hard_dv
WHERE JSON_VALUE(data, '$._id' RETURNING NUMBER) = 1;

--------------------------------------------------------------------------------
-- Time-zone and interval arithmetic, projected as text so every driver agrees.
--------------------------------------------------------------------------------
SELECT TO_CHAR(FROM_TZ(TIMESTAMP '2026-01-01 09:30:00', 'UTC')
                 AT TIME ZONE 'Asia/Seoul',
               'YYYY-MM-DD HH24:MI:SS TZR')               AS seoul_time,
       TO_CHAR((TIMESTAMP '2026-01-03 12:00:00'
                - TIMESTAMP '2026-01-01 09:30:00') DAY(2) TO SECOND(0)) AS gap_text,
       TO_CHAR(DATE '2026-01-31' + INTERVAL '1' MONTH * 2 -
               NUMTODSINTERVAL(36, 'HOUR'), 'YYYY-MM-DD HH24:MI')       AS shifted,
       EXTRACT(TIMEZONE_HOUR FROM
               FROM_TZ(TIMESTAMP '2026-06-01 00:00:00', '+09:00'))      AS tz_hour
FROM dual;

--------------------------------------------------------------------------------
-- PL/SQL grammar torture: forward-declared nested subprograms, record + VARRAY
-- + INDEX BY VARCHAR2 collections, REVERSE loop, CONTINUE WHEN, EXIT ... WHEN
-- with labels, GOTO, searched CASE statement, EXECUTE IMMEDIATE with OUT bind
-- and RETURNING INTO, and a PRAGMA EXCEPTION_INIT handler.
--------------------------------------------------------------------------------
DECLARE
  TYPE metric_rec_t IS RECORD (
    metric_id sq_hard_metric.metric_id%TYPE,
    label     VARCHAR2(20) := 'unset'
  );
  TYPE rec_by_name_t IS TABLE OF metric_rec_t INDEX BY VARCHAR2(20);
  TYPE triple_t IS VARRAY(4) OF NUMBER NOT NULL;

  by_name    rec_by_name_t;
  triple     triple_t := triple_t(10, 20, 30);
  walk_total NUMBER := 0;
  probe_row  sq_hard_metric%ROWTYPE;
  out_double NUMBER;
  updated_v  NUMBER;
  stuck      EXCEPTION;
  PRAGMA EXCEPTION_INIT(stuck, -54);

  FUNCTION shrink(p_in VARCHAR2) RETURN VARCHAR2;
  PROCEDURE stamp(p_key VARCHAR2, p_id NUMBER);

  FUNCTION shrink(p_in VARCHAR2) RETURN VARCHAR2 IS
  BEGIN
    RETURN SUBSTR(p_in, 1, 4);
  END shrink;

  PROCEDURE stamp(p_key VARCHAR2, p_id NUMBER) IS
    r metric_rec_t;
  BEGIN
    r.metric_id := p_id;
    r.label := shrink(p_key) || '#' || p_id;
    by_name(p_key) := r;
  END stamp;
BEGIN
  <<fill_loop>>
  FOR i IN REVERSE 1 .. triple.COUNT LOOP
    CONTINUE fill_loop WHEN MOD(triple(i), 20) = 0;
    walk_total := walk_total + triple(i);
  END LOOP fill_loop;

  triple.EXTEND;
  triple(4) := 40;
  triple.TRIM(1);

  stamp('latency', 1);
  stamp('errors', 4);

  <<scan_loop>>
  LOOP
    EXIT scan_loop WHEN by_name.COUNT = 2;
    NULL;
  END LOOP scan_loop;

  IF walk_total = 40 THEN
    GOTO verified;
  END IF;
  RAISE_APPLICATION_ERROR(-20030, 'reverse/continue walk ' || walk_total);

  <<verified>>
  CASE
    WHEN by_name('latency').label LIKE 'late#%' THEN
      NULL;
    ELSE
      RAISE_APPLICATION_ERROR(-20031, 'record label ' || by_name('latency').label);
  END CASE;

  EXECUTE IMMEDIATE
    'BEGIN :doubled := :seed * 2; END;'
    USING OUT out_double, IN walk_total;
  IF out_double <> 80 THEN
    RAISE_APPLICATION_ERROR(-20032, 'out bind ' || out_double);
  END IF;

  SELECT * INTO probe_row FROM sq_hard_metric WHERE metric_id = 1;
  BEGIN
    EXECUTE IMMEDIATE
      'UPDATE sq_hard_metric SET metric_value = metric_value
       WHERE metric_id = :id RETURNING metric_value INTO :v'
      USING probe_row.metric_id RETURNING INTO updated_v;
  EXCEPTION
    WHEN stuck THEN
      RAISE_APPLICATION_ERROR(-20033, 'unexpected resource lock');
  END;
  IF updated_v <> probe_row.metric_value THEN
    RAISE_APPLICATION_ERROR(-20034, 'returning into ' || updated_v);
  END IF;

  DBMS_OUTPUT.PUT_LINE('plsql torture total=' || walk_total);
END;
/

--------------------------------------------------------------------------------
-- Whitespace-hostile but legal literals and aliases in a single tight line.
--------------------------------------------------------------------------------
SELECT 1"X",.5+2."Y",3.5e1"Z",'it''s'||q'[ok]'"W" FROM dual WHERE 1=1 AND 2>1;

--------------------------------------------------------------------------------
-- Extension self-verification.
--------------------------------------------------------------------------------
DECLARE
  hi_rows    PLS_INTEGER;
  lo_rows    PLS_INTEGER;
  edge_pairs PLS_INTEGER;
  macro_rows PLS_INTEGER;
  deep_norm  NUMBER;
  dv_node    NUMBER;
  doc_names  VARCHAR2(200);
BEGIN
  SELECT COUNT(*) INTO hi_rows FROM sq_hard_split_hi;
  SELECT COUNT(*) INTO lo_rows FROM sq_hard_split_lo;
  SELECT COUNT(*) INTO edge_pairs
  FROM GRAPH_TABLE (sq_hard_graph
    MATCH (a IS metric) -[e IS links]-> (b IS metric)
    COLUMNS (e.hop_cost AS hop_cost));
  SELECT COUNT(*) INTO macro_rows FROM sq_hard_topn(p_rows => 2);
  SELECT sq_hard_point3_t(3, 4, 12).norm() INTO deep_norm FROM dual;
  SELECT JSON_VALUE(data, '$.nodeId' RETURNING NUMBER) INTO dv_node
  FROM sq_hard_dv
  WHERE JSON_VALUE(data, '$._id' RETURNING NUMBER) = 1;
  SELECT LISTAGG("한글 컬럼", ',') WITHIN GROUP (ORDER BY doc_id)
  INTO doc_names FROM sq_hard_doc;

  IF hi_rows <> 2 OR lo_rows <> 2 THEN
    RAISE_APPLICATION_ERROR(-20040, 'insert-first split ' || hi_rows || '/' || lo_rows);
  END IF;
  IF edge_pairs <> 3 THEN
    RAISE_APPLICATION_ERROR(-20041, 'graph edges ' || edge_pairs);
  END IF;
  IF macro_rows <> 2 THEN
    RAISE_APPLICATION_ERROR(-20042, 'macro rows ' || macro_rows);
  END IF;
  IF deep_norm <> 13 THEN
    RAISE_APPLICATION_ERROR(-20043, 'subtype norm ' || deep_norm);
  END IF;
  IF dv_node <> 1 THEN
    RAISE_APPLICATION_ERROR(-20044, 'duality view node ' || dv_node);
  END IF;
  IF doc_names <> N'첫번째 문서,두번째' THEN
    RAISE_APPLICATION_ERROR(-20045, 'unicode roundtrip ' || doc_names);
  END IF;
END;
/

--------------------------------------------------------------------------------
-- ULTRA WAVE 3: 23ai FROM-less SELECT and GROUP BY over a select-list alias.
--------------------------------------------------------------------------------
SELECT 6 * 7 AS no_from_answer;

SELECT node_id AS grp_alias, COUNT(*) AS grp_ct
FROM sq_hard_metric
GROUP BY grp_alias
ORDER BY grp_alias;

--------------------------------------------------------------------------------
-- Lexer bait: comment openers inside string literals AND inside quoted aliases,
-- q-quotes with every delimiter family (including the double-quote delimiter),
-- mixed-case keywords, and tab-separated tokens on one line.
--------------------------------------------------------------------------------
SELECT '/* not a comment */'            AS "co--mment",
       '-- still a string'              AS "/*alias*/",
       q'(paren (nested) quote)'        AS q_paren,
       q'<angle <deep> quote>'          AS q_angle,
       q'"double "" trick"'             AS q_double,
       'it''s -- fine'                  AS doubled_quote
FROM dual
WHERE 1 = 1;

sElEcT	COUNT(*)	aS	mIxEd_CaSe_CoUnT	FrOm	sq_hard_metric;

--------------------------------------------------------------------------------
-- Graceful conversion: CAST/TO_NUMBER/TO_DATE DEFAULT ... ON CONVERSION ERROR
-- (with an NLS argument carried in a q-quote) and VALIDATE_CONVERSION.
--------------------------------------------------------------------------------
SELECT CAST('12x' AS NUMBER DEFAULT -1 ON CONVERSION ERROR)          AS safe_cast,
       TO_NUMBER('1.234,5' DEFAULT 0 ON CONVERSION ERROR,
                 '9G999D9', q'[NLS_NUMERIC_CHARACTERS = ',.']')      AS eu_number,
       VALIDATE_CONVERSION('2026-02-30' AS DATE, 'YYYY-MM-DD')       AS bad_date_flag,
       TO_CHAR(TO_DATE('2026-13-01' DEFAULT '2026-01-01'
                       ON CONVERSION ERROR, 'YYYY-MM-DD'),
               'YYYY-MM-DD')                                         AS coerced_date
FROM dual;

--------------------------------------------------------------------------------
-- 23ai usage domain with DISPLAY expression, then a table stacking identity
-- options, column/table ANNOTATIONS, an INVISIBLE column, a VIRTUAL column,
-- a domain column, 23ai multi-row VALUES, and a multi-column SET subquery.
--------------------------------------------------------------------------------
CREATE DOMAIN sq_hard_share_d AS NUMBER(5, 2)
  CONSTRAINT sq_hard_share_ck CHECK (sq_hard_share_d BETWEEN 0 AND 100)
  DISPLAY TO_CHAR(sq_hard_share_d, 'FM990.0') || '%';

CREATE TABLE sq_hard_ledger (
  ledger_id   NUMBER GENERATED BY DEFAULT ON NULL AS IDENTITY
                (START WITH 100 INCREMENT BY 10 CACHE 5),
  base_amt    NUMBER(10, 2) NOT NULL ANNOTATIONS (Display '기본 금액'),
  share_pct   NUMBER(5, 2) DOMAIN sq_hard_share_d,
  hidden_note VARCHAR2(40) INVISIBLE,
  gross_amt   NUMBER(12, 2) GENERATED ALWAYS AS (ROUND(base_amt * 1.1, 2)) VIRTUAL,
  CONSTRAINT sq_hard_ledger_pk PRIMARY KEY (ledger_id),
  CONSTRAINT sq_hard_ledger_amt_ck CHECK (base_amt >= 0)
) ANNOTATIONS (Purpose '울트라 웨이브3');

INSERT INTO sq_hard_ledger (base_amt, share_pct, hidden_note) VALUES
  (10, 25.5, N'그림자'),
  (20, 75.25, 'shadow');
INSERT INTO sq_hard_ledger (ledger_id, base_amt, share_pct)
VALUES (DEFAULT, 30, 100);

UPDATE sq_hard_ledger l
SET (l.base_amt, l.share_pct) =
      (SELECT l.base_amt + 1, l.share_pct FROM dual)
WHERE l.share_pct = 100;

SELECT ledger_id, base_amt, gross_amt, hidden_note,
       DOMAIN_DISPLAY(share_pct) AS share_display
FROM sq_hard_ledger
ORDER BY ledger_id;

--------------------------------------------------------------------------------
-- Interval-partitioned + list-subpartitioned table with a subpartition
-- template, then PARTITION FOR / SUBPARTITION FOR data-driven selection.
--------------------------------------------------------------------------------
CREATE TABLE sq_hard_sales (
  sale_id  NUMBER NOT NULL,
  sale_day DATE NOT NULL,
  region   VARCHAR2(8) NOT NULL,
  amount   NUMBER(10, 2) NOT NULL
)
PARTITION BY RANGE (sale_day) INTERVAL (NUMTOYMINTERVAL(1, 'MONTH'))
SUBPARTITION BY LIST (region)
SUBPARTITION TEMPLATE (
  SUBPARTITION sp_east VALUES ('EAST'),
  SUBPARTITION sp_rest VALUES (DEFAULT)
)
(PARTITION p_seed VALUES LESS THAN (DATE '2026-01-01'));

INSERT INTO sq_hard_sales VALUES (1, DATE '2025-12-15', 'EAST', 11);
INSERT INTO sq_hard_sales VALUES (2, DATE '2026-02-10', 'EAST', 22);
INSERT INTO sq_hard_sales VALUES (3, DATE '2026-02-20', 'WEST', 33);
INSERT INTO sq_hard_sales VALUES (4, DATE '2026-03-05', 'EAST', 44);

SELECT sale_id, region, amount
FROM sq_hard_sales PARTITION FOR (DATE '2026-02-15')
ORDER BY sale_id;

SELECT COUNT(*) AS feb_east_rows
FROM sq_hard_sales SUBPARTITION FOR (DATE '2026-02-15', 'EAST');

--------------------------------------------------------------------------------
-- MATCH_RECOGNIZE boss round: ALL ROWS PER MATCH WITH UNMATCHED ROWS, SUBSET,
-- CLASSIFIER, RUNNING vs FINAL, reluctant quantifier, SKIP TO NEXT ROW; then a
-- PERMUTE pattern reduced to ONE ROW PER MATCH.
--------------------------------------------------------------------------------
SELECT node_id, metric_day, row_class, m_no, run_val, span_len
FROM sq_hard_metric
MATCH_RECOGNIZE (
  PARTITION BY node_id
  ORDER BY metric_day
  MEASURES CLASSIFIER()              AS row_class,
           MATCH_NUMBER()            AS m_no,
           RUNNING SUM(metric_value) AS run_val,
           FINAL COUNT(stretch.*)    AS span_len
  ALL ROWS PER MATCH WITH UNMATCHED ROWS
  AFTER MATCH SKIP TO NEXT ROW
  PATTERN (strt up+?)
  SUBSET stretch = (strt, up)
  DEFINE up AS up.metric_value >= PREV(up.metric_value)
) mr
ORDER BY node_id, metric_day, m_no;

SELECT node_id, m_no, lo_val, hi_val
FROM sq_hard_metric
MATCH_RECOGNIZE (
  PARTITION BY node_id
  ORDER BY metric_day
  MEASURES MATCH_NUMBER()        AS m_no,
           low_row.metric_value  AS lo_val,
           high_row.metric_value AS hi_val
  ONE ROW PER MATCH
  AFTER MATCH SKIP PAST LAST ROW
  PATTERN (PERMUTE(low_row, high_row))
  DEFINE low_row  AS low_row.metric_value < 20,
         high_row AS high_row.metric_value >= 20
)
ORDER BY node_id, m_no;

--------------------------------------------------------------------------------
-- JSON_TRANSFORM DML, JSON dot-notation with item methods, JSON_EXISTS with a
-- PASSING bind, item-method paths inside JSON_VALUE, and JSON_SERIALIZE /
-- JSON_MERGEPATCH round-trips.
--------------------------------------------------------------------------------
UPDATE sq_hard_metric m
SET m.payload = JSON_TRANSFORM(
      m.payload,
      SET '$.grade' = CASE WHEN m.metric_value >= 20 THEN 'A' ELSE 'B' END,
      APPEND '$.tags' = 'wave3',
      REMOVE '$.ghost' IGNORE ON MISSING
    )
WHERE m.payload IS NOT NULL;

SELECT m.metric_id,
       m.payload.grade.string()                                  AS grade_dot,
       JSON_VALUE(m.payload, '$.tags.size()' RETURNING NUMBER)   AS tag_ct,
       JSON_SERIALIZE(JSON_QUERY(m.payload, '$.tags'
                                 WITH CONDITIONAL ARRAY WRAPPER)
                      PRETTY)                                    AS tags_pretty,
       JSON_SERIALIZE(JSON_MERGEPATCH(m.payload,
                                      '{"patched":true}')
                      RETURNING VARCHAR2(400) ORDERED)           AS merged
FROM sq_hard_metric m
WHERE JSON_EXISTS(m.payload, '$.tags[*]?(@ == $needle)'
                  PASSING 'wave3' AS "needle")
ORDER BY m.metric_id;

--------------------------------------------------------------------------------
-- Nested-table column with its own store table, then collection-level DML
-- through TABLE(subquery): INSERT INTO TABLE, UPDATE TABLE, DELETE FROM TABLE.
--------------------------------------------------------------------------------
CREATE TABLE sq_hard_bag (
  bag_id NUMBER CONSTRAINT sq_hard_bag_pk PRIMARY KEY,
  nums   sq_hard_num_tab
) NESTED TABLE nums STORE AS sq_hard_bag_store;

INSERT INTO sq_hard_bag VALUES (1, sq_hard_num_tab(1, 2, 3));
INSERT INTO TABLE(SELECT b.nums FROM sq_hard_bag b WHERE b.bag_id = 1)
VALUES (4);
UPDATE TABLE(SELECT b.nums FROM sq_hard_bag b WHERE b.bag_id = 1) t
SET t.COLUMN_VALUE = t.COLUMN_VALUE * 10
WHERE t.COLUMN_VALUE = 4;
DELETE FROM TABLE(SELECT b.nums FROM sq_hard_bag b WHERE b.bag_id = 1) t
WHERE t.COLUMN_VALUE = 1;

SELECT b.bag_id, t.COLUMN_VALUE AS bag_value
FROM sq_hard_bag b, TABLE(b.nums) t
ORDER BY b.bag_id, t.COLUMN_VALUE;

--------------------------------------------------------------------------------
-- MODEL ITERATE with ITERATION_NUMBER and an UNTIL condition.
--------------------------------------------------------------------------------
SELECT metric_id, iterated_value
FROM (SELECT metric_id, metric_value FROM sq_hard_metric WHERE node_id = 1)
MODEL
  DIMENSION BY (metric_id)
  MEASURES (metric_value AS iterated_value)
  RULES ITERATE (5) UNTIL (ITERATION_NUMBER >= 2) (
    iterated_value[1] = iterated_value[1] + ITERATION_NUMBER
  )
ORDER BY metric_id;

--------------------------------------------------------------------------------
-- CUBE with GROUPING_ID filtering in HAVING, plus a CTE that deliberately
-- shadows the physical table name it reads from.
--------------------------------------------------------------------------------
SELECT node_id, metric_name, SUM(metric_value) AS cube_total,
       GROUPING_ID(node_id, metric_name)       AS cube_gid
FROM sq_hard_metric
GROUP BY CUBE(node_id, metric_name)
HAVING GROUPING_ID(node_id, metric_name) IN (0, 3)
ORDER BY cube_gid, node_id, metric_name;

WITH sq_hard_merge (metric_id, boosted) AS (
  SELECT metric_id, metric_value * 100
  FROM sq_hard_metric
  WHERE node_id = 2
)
SELECT metric_id, boosted
FROM sq_hard_merge
ORDER BY metric_id;

--------------------------------------------------------------------------------
-- Cyclic edge data walked with CONNECT BY NOCYCLE + CONNECT_BY_ISCYCLE.
--------------------------------------------------------------------------------
INSERT INTO sq_hard_edge VALUES (9, 3, 1, 2);

SELECT LEVEL AS lvl, e.src_metric_id, e.dst_metric_id,
       CONNECT_BY_ISCYCLE AS cyc
FROM sq_hard_edge e
START WITH e.src_metric_id = 1
CONNECT BY NOCYCLE PRIOR e.dst_metric_id = e.src_metric_id
ORDER SIBLINGS BY e.edge_id;

--------------------------------------------------------------------------------
-- Flashback read at the exact current SCN, savepoint/lock torture, and a
-- multi-line optimizer hint that must never be mistaken for a plain comment.
--------------------------------------------------------------------------------
SELECT CASE WHEN COUNT(*) > 0 THEN 'has-rows' ELSE 'empty' END AS flashback_scn_probe
FROM help AS OF SCN DBMS_FLASHBACK.GET_SYSTEM_CHANGE_NUMBER;

SELECT CASE WHEN COUNT(*) > 0 THEN 'has-rows' ELSE 'empty' END AS flashback_ts_probe
FROM help AS OF TIMESTAMP (SYSTIMESTAMP - INTERVAL '5' SECOND);

LOCK TABLE sq_hard_merge IN ROW EXCLUSIVE MODE NOWAIT;
SELECT k FROM sq_hard_merge WHERE k = 1 FOR UPDATE OF v SKIP LOCKED;
COMMIT;

BEGIN
  SAVEPOINT wave3_sp;
  UPDATE sq_hard_merge SET v = v + 1000 WHERE k = 1;
  ROLLBACK TO SAVEPOINT wave3_sp;
  COMMIT;
END;
/

SELECT /*+
         LEADING(m)
         FULL(m)
         NO_PARALLEL
       */ COUNT(*) AS hinted_count
FROM sq_hard_metric m;

--------------------------------------------------------------------------------
-- EXPLAIN PLAN INTO with a statement id, summarized deterministically.
--------------------------------------------------------------------------------
DELETE FROM plan_table WHERE statement_id = 'SQ_HARD_W3';

EXPLAIN PLAN SET STATEMENT_ID = 'SQ_HARD_W3' INTO plan_table FOR
SELECT /*+ FULL(m) */ node_id, SUM(metric_value)
FROM sq_hard_metric m
GROUP BY node_id;

SELECT CASE WHEN COUNT(*) >= 2 THEN 'planned' ELSE 'missing' END AS plan_status
FROM plan_table
WHERE statement_id = 'SQ_HARD_W3';

--------------------------------------------------------------------------------
-- Polymorphic table function (DBMS_TF): describe adds a synthetic column and
-- fetch_rows fills it per row; call site passes a live table plus named args.
--------------------------------------------------------------------------------
CREATE OR REPLACE PACKAGE sq_hard_ptf_pkg AS
  FUNCTION tag_rows(p_tab    TABLE,
                    p_suffix VARCHAR2 DEFAULT 'W3')
    RETURN TABLE PIPELINED ROW POLYMORPHIC USING sq_hard_ptf_pkg;
  FUNCTION describe(p_tab    IN OUT DBMS_TF.TABLE_T,
                    p_suffix VARCHAR2 DEFAULT 'W3') RETURN DBMS_TF.DESCRIBE_T;
  PROCEDURE fetch_rows(p_suffix VARCHAR2 DEFAULT 'W3');
END sq_hard_ptf_pkg;
/
CREATE OR REPLACE PACKAGE BODY sq_hard_ptf_pkg AS
  FUNCTION describe(p_tab    IN OUT DBMS_TF.TABLE_T,
                    p_suffix VARCHAR2 DEFAULT 'W3') RETURN DBMS_TF.DESCRIBE_T IS
  BEGIN
    RETURN DBMS_TF.DESCRIBE_T(
      new_columns => DBMS_TF.COLUMNS_NEW_T(
        1 => DBMS_TF.COLUMN_METADATA_T(
               name => 'ROW_TAG',
               type => DBMS_TF.TYPE_VARCHAR2)));
  END describe;

  PROCEDURE fetch_rows(p_suffix VARCHAR2 DEFAULT 'W3') IS
    l_env  DBMS_TF.ENV_T := DBMS_TF.GET_ENV;
    l_tags DBMS_TF.TAB_VARCHAR2_T;
  BEGIN
    FOR i IN 1 .. l_env.row_count LOOP
      l_tags(i) := p_suffix || ':' || TO_CHAR(i);
    END LOOP;
    DBMS_TF.PUT_COL(1, l_tags);
  END fetch_rows;
END sq_hard_ptf_pkg;
/
SELECT COUNT(*)                AS tagged_rows,
       COUNT(DISTINCT row_tag) AS distinct_tags
FROM sq_hard_ptf_pkg.tag_rows(sq_hard_metric, p_suffix => 'wave3');

--------------------------------------------------------------------------------
-- Analytic view stack: attribute dimension, hierarchy, analytic view with two
-- FACT measures, queried through HIERARCHIES with hierarchy attributes.
--------------------------------------------------------------------------------
CREATE TABLE sq_hard_node_dim (
  node_id   NUMBER CONSTRAINT sq_hard_node_dim_pk PRIMARY KEY,
  node_name VARCHAR2(20) NOT NULL
);
INSERT INTO sq_hard_node_dim VALUES (1, 'alpha');
INSERT INTO sq_hard_node_dim VALUES (2, 'beta');

CREATE OR REPLACE ATTRIBUTE DIMENSION sq_hard_node_ad
  DIMENSION TYPE STANDARD
  USING sq_hard_node_dim
  ATTRIBUTES (node_id, node_name)
  LEVEL node_lvl
    LEVEL TYPE STANDARD
    KEY node_id
    MEMBER NAME TO_CHAR(node_id)
    MEMBER CAPTION node_name
    ORDER BY node_id
  ALL MEMBER NAME 'ALL_NODES';

CREATE OR REPLACE HIERARCHY sq_hard_node_h
  USING sq_hard_node_ad (node_lvl);

CREATE OR REPLACE ANALYTIC VIEW sq_hard_av
  USING sq_hard_metric
  DIMENSION BY (
    sq_hard_node_ad
      KEY node_id REFERENCES node_id
      HIERARCHIES (sq_hard_node_h DEFAULT)
  )
  MEASURES (
    total_value FACT metric_value AGGREGATE BY SUM,
    avg_value   FACT metric_value AGGREGATE BY AVG
  )
  DEFAULT MEASURE total_value;

SELECT sq_hard_node_h.member_name AS node_member,
       sq_hard_node_h.level_name  AS lvl,
       total_value
FROM sq_hard_av HIERARCHIES (sq_hard_node_h)
ORDER BY sq_hard_node_h.hier_order;

--------------------------------------------------------------------------------
-- PL/SQL 21c iterator torture: stepped ranges, multiple iteration controls,
-- INDICES OF / VALUES OF, iterator-built qualified expressions, SUBTYPE with a
-- RANGE constraint, and an autonomous-transaction logger.
--------------------------------------------------------------------------------
CREATE TABLE sq_hard_w3_log (
  log_id NUMBER GENERATED ALWAYS AS IDENTITY,
  note   VARCHAR2(80) NOT NULL,
  CONSTRAINT sq_hard_w3_log_pk PRIMARY KEY (log_id)
);

CREATE OR REPLACE PROCEDURE sq_hard_autolog(p_note VARCHAR2) IS
  PRAGMA AUTONOMOUS_TRANSACTION;
BEGIN
  INSERT INTO sq_hard_w3_log (note) VALUES (p_note);
  COMMIT;
END sq_hard_autolog;
/

DECLARE
  SUBTYPE small_n IS PLS_INTEGER RANGE 0 .. 9999;
  TYPE num_list IS TABLE OF NUMBER;
  TYPE name_map IS TABLE OF NUMBER INDEX BY VARCHAR2(10);
  l_squares num_list := num_list(FOR i IN 1 .. 4 SEQUENCE => i * i);
  l_map     name_map := name_map('one' => 1, 'two' => 2);
  l_total   small_n := 0;
  l_walk    NUMBER := 0;
BEGIN
  FOR i IN 1 .. 2, REVERSE 5 .. 6, 9 .. 9 LOOP
    l_total := l_total + i;
  END LOOP;
  FOR i IN 1 .. 10 BY 3 LOOP
    l_walk := l_walk + i;
  END LOOP;
  FOR idx IN INDICES OF l_map LOOP
    l_walk := l_walk + l_map(idx);
  END LOOP;
  FOR v IN VALUES OF l_squares LOOP
    l_walk := l_walk + v;
  END LOOP;
  IF l_total <> 23 OR l_walk <> 55 THEN
    RAISE_APPLICATION_ERROR(-20050, 'iterator torture ' || l_total || '/' || l_walk);
  END IF;
  sq_hard_autolog('iterators=' || l_total);
END;
/

--------------------------------------------------------------------------------
-- Wave-3 self-verification.
--------------------------------------------------------------------------------
DECLARE
  ledger_ids    NUMBER;
  ledger_gross  NUMBER;
  feb_rows      PLS_INTEGER;
  feb_east      PLS_INTEGER;
  bag_sum       NUMBER;
  iter_value    NUMBER;
  ptf_tags      PLS_INTEGER;
  av_all_total  NUMBER;
  wave3_json    PLS_INTEGER;
  cyc_edges     PLS_INTEGER;
  permute_rows  PLS_INTEGER;
  log_rows      PLS_INTEGER;
BEGIN
  SELECT SUM(ledger_id), MAX(gross_amt)
  INTO ledger_ids, ledger_gross FROM sq_hard_ledger;
  SELECT COUNT(*) INTO feb_rows
  FROM sq_hard_sales PARTITION FOR (DATE '2026-02-15');
  SELECT COUNT(*) INTO feb_east
  FROM sq_hard_sales SUBPARTITION FOR (DATE '2026-02-15', 'EAST');
  SELECT SUM(t.COLUMN_VALUE) INTO bag_sum
  FROM sq_hard_bag b, TABLE(b.nums) t;
  SELECT iterated_value INTO iter_value
  FROM (
    SELECT metric_id, iterated_value
    FROM (SELECT metric_id, metric_value FROM sq_hard_metric WHERE node_id = 1)
    MODEL
      DIMENSION BY (metric_id)
      MEASURES (metric_value AS iterated_value)
      RULES ITERATE (5) UNTIL (ITERATION_NUMBER >= 2) (
        iterated_value[1] = iterated_value[1] + ITERATION_NUMBER
      )
  )
  WHERE metric_id = 1;
  SELECT COUNT(DISTINCT row_tag) INTO ptf_tags
  FROM sq_hard_ptf_pkg.tag_rows(sq_hard_metric, p_suffix => 'wave3');
  SELECT total_value INTO av_all_total
  FROM sq_hard_av HIERARCHIES (sq_hard_node_h)
  WHERE sq_hard_node_h.member_name = 'ALL_NODES';
  SELECT COUNT(*) INTO wave3_json
  FROM sq_hard_metric m
  WHERE JSON_EXISTS(m.payload, '$.tags[*]?(@ == "wave3")');
  SELECT COUNT(*) INTO cyc_edges
  FROM (
    SELECT CONNECT_BY_ISCYCLE AS cyc
    FROM sq_hard_edge e
    START WITH e.src_metric_id = 1
    CONNECT BY NOCYCLE PRIOR e.dst_metric_id = e.src_metric_id
  )
  WHERE cyc = 1;
  SELECT COUNT(*) INTO permute_rows
  FROM sq_hard_metric
  MATCH_RECOGNIZE (
    PARTITION BY node_id
    ORDER BY metric_day
    MEASURES MATCH_NUMBER() AS m_no
    ONE ROW PER MATCH
    PATTERN (PERMUTE(low_row, high_row))
    DEFINE low_row  AS low_row.metric_value < 20,
           high_row AS high_row.metric_value >= 20
  );
  SELECT COUNT(*) INTO log_rows FROM sq_hard_w3_log;

  IF ledger_ids <> 330 OR ledger_gross <> 34.1 THEN
    RAISE_APPLICATION_ERROR(-20051, 'ledger ' || ledger_ids || '/' || ledger_gross);
  END IF;
  IF feb_rows <> 2 OR feb_east <> 1 THEN
    RAISE_APPLICATION_ERROR(-20052, 'partition-for ' || feb_rows || '/' || feb_east);
  END IF;
  IF bag_sum <> 45 THEN
    RAISE_APPLICATION_ERROR(-20053, 'nested-table dml ' || bag_sum);
  END IF;
  IF iter_value <> 15 THEN
    RAISE_APPLICATION_ERROR(-20054, 'model iterate ' || iter_value);
  END IF;
  IF ptf_tags <> 4 THEN
    RAISE_APPLICATION_ERROR(-20055, 'polymorphic tags ' || ptf_tags);
  END IF;
  IF av_all_total <> 56 THEN
    RAISE_APPLICATION_ERROR(-20056, 'analytic view total ' || av_all_total);
  END IF;
  IF wave3_json <> 4 THEN
    RAISE_APPLICATION_ERROR(-20057, 'json_transform tags ' || wave3_json);
  END IF;
  IF cyc_edges < 1 THEN
    RAISE_APPLICATION_ERROR(-20058, 'nocycle flag ' || cyc_edges);
  END IF;
  IF permute_rows <> 1 THEN
    RAISE_APPLICATION_ERROR(-20059, 'permute matches ' || permute_rows);
  END IF;
  IF log_rows < 1 THEN
    RAISE_APPLICATION_ERROR(-20060, 'autonomous log ' || log_rows);
  END IF;
END;
/

--------------------------------------------------------------------------------
-- ULTRA WAVE 4: 26ai non-positional INSERT, direct-join UPDATE, OLD/NEW
-- RETURNING, GROUP BY ALL, TIME_BUCKET, vector arithmetic/aggregation/distance
-- operators, query-fed JSON_ARRAY, JSON set transforms, JSON_ID, Temporal
-- Validity, DDL IF EXISTS/IF NOT EXISTS, and extended PL/SQL CASE controls.
--------------------------------------------------------------------------------
DROP TABLE IF EXISTS sq_hard_w4_absent PURGE;

CREATE TABLE IF NOT EXISTS sq_hard_w4_target (
  target_id   NUMBER CONSTRAINT sq_hard_w4_target_pk PRIMARY KEY,
  node_id     NUMBER NOT NULL,
  target_name VARCHAR2(30) NOT NULL,
  amount      NUMBER(10, 2) NOT NULL,
  embedding   VECTOR(3, FLOAT32) NOT NULL,
  payload     JSON NOT NULL
) TABLESPACE users;

-- SET makes each target/value association explicit; the next INSERT reverses
-- every source column so BY NAME, not position, must resolve the mapping.
INSERT INTO sq_hard_w4_target SET
  target_id = 1,
  node_id = 1,
  target_name = 'alpha',
  amount = 10,
  embedding = VECTOR('[1, 2, 3]', 3, FLOAT32),
  payload = JSON_OBJECT(
    'tags' VALUE JSON_ARRAY('core', 'sql')
    RETURNING JSON
  );

INSERT INTO sq_hard_w4_target BY NAME
SELECT JSON_OBJECT(
         'tags' VALUE JSON_ARRAY('edge')
         RETURNING JSON
       )                                           AS payload,
       VECTOR('[2, 2, 2]', 3, FLOAT32)             AS embedding,
       20                                          AS amount,
       'beta'                                      AS target_name,
       2                                           AS node_id,
       2                                           AS target_id;

CREATE TABLE sq_hard_w4_source (
  target_id  NUMBER CONSTRAINT sq_hard_w4_source_pk PRIMARY KEY,
  new_amount NUMBER(10, 2) NOT NULL
) TABLESPACE users;

INSERT INTO sq_hard_w4_source VALUES (2, 25);

UPDATE sq_hard_w4_target t
SET t.amount = s.new_amount
FROM sq_hard_w4_source s
WHERE s.target_id = t.target_id;

--------------------------------------------------------------------------------
-- RETURNING sees both row images. The simple CASE mixes a choice list with
-- dangling predicates; this is PL/SQL-only grammar despite looking like SQL.
--------------------------------------------------------------------------------
DECLARE
  old_amount NUMBER;
  new_amount NUMBER;
  probe      PLS_INTEGER := 25;
  band_name  VARCHAR2(20);
BEGIN
  UPDATE sq_hard_w4_target
  SET amount = amount + 5
  WHERE target_id = 1
  RETURNING OLD amount, NEW amount INTO old_amount, new_amount;

  band_name :=
    CASE probe
      WHEN < 0, > 100 THEN 'invalid'
      WHEN 0, IS NULL THEN 'empty'
      WHEN BETWEEN 20 AND 29, 42 THEN 'boss'
      ELSE 'ordinary'
    END;

  IF old_amount <> 10 OR new_amount <> 15 OR band_name <> 'boss' THEN
    RAISE_APPLICATION_ERROR(
      -20061,
      'old/new/case ' || old_amount || '/' || new_amount || '/' || band_name
    );
  END IF;
END;
/

--------------------------------------------------------------------------------
-- GROUP BY ALL infers both non-aggregate expressions; TIME_BUCKET has two
-- interval-like argument families and a bare START control token.
--------------------------------------------------------------------------------
SELECT node_id,
       UPPER(target_name)                                  AS normalized_name,
       SUM(amount)                                         AS total_amount,
       SUM(CASE WHEN amount >= 20 THEN amount ELSE 0 END)  AS filtered_amount,
       COUNT(CASE WHEN target_name LIKE '%a' THEN 1 END)   AS matched_names
FROM sq_hard_w4_target
GROUP BY ALL
ORDER BY node_id, normalized_name;

SELECT TIME_BUCKET(
         TIMESTAMP '2026-01-03 10:37:00',
         INTERVAL '15' MINUTE,
         TIMESTAMP '2026-01-01 00:00:00',
         START
       ) AS bucket_start,
       TIME_BUCKET(
         DATE '2026-03-10',
         'P1Y',
         DATE '2024-02-29',
         START ON OVERFLOW ROUND
       ) AS leap_bucket
FROM dual;

--------------------------------------------------------------------------------
-- The three vector distance operators intentionally collide with ordinary
-- comparison/comment punctuation. Addition and Hadamard multiplication return
-- vectors, while AVG changes the element format to FLOAT64.
--------------------------------------------------------------------------------
SELECT target_id,
       FROM_VECTOR(
         embedding + VECTOR('[1, 1, 1]', 3, FLOAT32)
       )                                                       AS vector_sum,
       FROM_VECTOR(
         embedding * VECTOR('[2, 3, 4]', 3, FLOAT32)
       )                                                       AS vector_product,
       ROUND(
         embedding <-> VECTOR('[1, 1, 1]', 3, FLOAT32), 6
       )                                                       AS l2_distance,
       ROUND(
         embedding <=> VECTOR('[1, 1, 1]', 3, FLOAT32), 6
       )                                                       AS cosine_distance,
       ROUND(
         embedding <#> VECTOR('[1, 1, 1]', 3, FLOAT32), 6
       )                                                       AS neg_dot_product
FROM sq_hard_w4_target
ORDER BY target_id;

SELECT FROM_VECTOR(AVG(embedding)) AS centroid
FROM sq_hard_w4_target;

--------------------------------------------------------------------------------
-- A complete subquery is the single JSON_ARRAY argument. The transform then
-- treats tags as a mathematical set, including its two specialized handlers.
--------------------------------------------------------------------------------
SELECT JSON_SERIALIZE(
         JSON_ARRAY(
           SELECT JSON_OBJECT(
                    'id' VALUE target_id,
                    'name' VALUE target_name
                    RETURNING JSON
                  )
           FROM sq_hard_w4_target
           ORDER BY target_id
         )
         RETURNING CLOB VALUE PRETTY ORDERED
       ) AS target_array
FROM dual;

UPDATE sq_hard_w4_target
SET payload = JSON_TRANSFORM(
      payload,
      ADD_SET '$.tags' = 'wave4' IGNORE IF PRESENT,
      REMOVE_SET '$.tags' = 'ghost' IGNORE IF ABSENT
    );

SELECT target_id,
       JSON_SERIALIZE(
         payload
         RETURNING VARCHAR2(400) ORDERED
       )                                                       AS ordered_payload,
       LENGTH(RAWTOHEX(JSON_ID('UUID')))                       AS generated_uuid_len
FROM sq_hard_w4_target
ORDER BY target_id;

--------------------------------------------------------------------------------
-- Temporal Validity has its own PERIOD owner. AS OF returns rows valid at one
-- instant; VERSIONS returns rows intersecting an application-time interval.
--------------------------------------------------------------------------------
CREATE TABLE sq_hard_w4_temporal (
  temporal_id NUMBER CONSTRAINT sq_hard_w4_temporal_pk PRIMARY KEY,
  label_name  VARCHAR2(30) NOT NULL,
  valid_from  DATE NOT NULL,
  valid_to    DATE NOT NULL,
  PERIOD FOR validity (valid_from, valid_to)
) TABLESPACE users;

INSERT INTO sq_hard_w4_temporal VALUES
  (1, 'winter', DATE '2026-01-01', DATE '2026-04-01'),
  (2, 'summer', DATE '2026-04-01', DATE '2026-10-01'),
  (3, 'winter-next', DATE '2027-01-01', DATE '2027-04-01');

SELECT temporal_id, label_name
FROM sq_hard_w4_temporal
AS OF PERIOD FOR validity DATE '2026-02-01'
ORDER BY temporal_id;

SELECT temporal_id, label_name, valid_from, valid_to
FROM sq_hard_w4_temporal
VERSIONS PERIOD FOR validity
BETWEEN DATE '2026-03-01' AND DATE '2026-05-01'
ORDER BY temporal_id;

--------------------------------------------------------------------------------
-- Wave-4 self-verification.
--------------------------------------------------------------------------------
DECLARE
  target_rows    PLS_INTEGER;
  amount_total   NUMBER;
  beta_amount    NUMBER;
  tagged_rows    PLS_INTEGER;
  temporal_asof  PLS_INTEGER;
  temporal_range PLS_INTEGER;
  json_items     PLS_INTEGER;
  uuid_bytes     PLS_INTEGER;
  centroid_gap   NUMBER;
  bucket_start   TIMESTAMP;
BEGIN
  SELECT COUNT(*), SUM(amount)
  INTO target_rows, amount_total
  FROM sq_hard_w4_target;

  SELECT amount
  INTO beta_amount
  FROM sq_hard_w4_target
  WHERE target_id = 2;

  SELECT COUNT(*)
  INTO tagged_rows
  FROM sq_hard_w4_target t
  WHERE JSON_EXISTS(t.payload, '$.tags[*]?(@ == "wave4")');

  SELECT COUNT(*)
  INTO temporal_asof
  FROM sq_hard_w4_temporal
  AS OF PERIOD FOR validity DATE '2026-02-01';

  SELECT COUNT(*)
  INTO temporal_range
  FROM sq_hard_w4_temporal
  VERSIONS PERIOD FOR validity
  BETWEEN DATE '2026-03-01' AND DATE '2026-05-01';

  SELECT JSON_VALUE(
           JSON_ARRAY(
             SELECT target_id
             FROM sq_hard_w4_target
             ORDER BY target_id
           ),
           '$.size()' RETURNING NUMBER
         )
  INTO json_items
  FROM dual;

  SELECT UTL_RAW.LENGTH(JSON_ID('UUID'))
  INTO uuid_bytes
  FROM dual;

  SELECT VECTOR_DISTANCE(
           AVG(embedding),
           VECTOR('[1.5, 2, 2.5]', 3, FLOAT64),
           EUCLIDEAN
         )
  INTO centroid_gap
  FROM sq_hard_w4_target;

  SELECT TIME_BUCKET(
           TIMESTAMP '2026-01-03 10:37:00',
           INTERVAL '15' MINUTE,
           TIMESTAMP '2026-01-01 00:00:00',
           START
         )
  INTO bucket_start
  FROM dual;

  IF target_rows <> 2 OR amount_total <> 40 OR beta_amount <> 25 THEN
    RAISE_APPLICATION_ERROR(
      -20062,
      'non-positional/update-from ' ||
      target_rows || '/' || amount_total || '/' || beta_amount
    );
  END IF;
  IF tagged_rows <> 2 OR json_items <> 2 OR uuid_bytes <> 16 THEN
    RAISE_APPLICATION_ERROR(
      -20063,
      'json wave4 ' || tagged_rows || '/' || json_items || '/' || uuid_bytes
    );
  END IF;
  IF temporal_asof <> 1 OR temporal_range <> 2 THEN
    RAISE_APPLICATION_ERROR(
      -20064,
      'temporal wave4 ' || temporal_asof || '/' || temporal_range
    );
  END IF;
  IF ABS(centroid_gap) > 0.000001 THEN
    RAISE_APPLICATION_ERROR(-20065, 'vector centroid ' || centroid_gap);
  END IF;
  IF bucket_start <> TIMESTAMP '2026-01-03 10:30:00' THEN
    RAISE_APPLICATION_ERROR(
      -20066,
      'time bucket ' || TO_CHAR(bucket_start, 'YYYY-MM-DD HH24:MI:SS')
    );
  END IF;
END;
/

--------------------------------------------------------------------------------
-- ULTRA WAVE 5: join grammar the parser cannot resolve from token shape alone
-- (partitioned outer join, CROSS APPLY / LATERAL, NATURAL JOIN, USING, legacy
-- (+)), DML through an updatable inline view, INSERT ... LOG ERRORS, object
-- types with user constructors / STATIC / ORDER members and a live ALTER TYPE,
-- PIVOT XML with ANY, temporary-table families, subprogram attribute stacking,
-- bulk-binding torture, and hint syntax that is one line-join away from
-- commenting out its own projection.
--------------------------------------------------------------------------------

CREATE TABLE sq_hard_w5_note (
  note_key   VARCHAR2(30) CONSTRAINT sq_hard_w5_note_pk PRIMARY KEY,
  note_value NUMBER NOT NULL
) TABLESPACE users;

--------------------------------------------------------------------------------
-- W5-A: join-grammar torture. The partitioned outer join densifies a calendar
-- per node, CROSS APPLY and CROSS JOIN LATERAL correlate back into the outer
-- row, NATURAL JOIN / JOIN USING hide their predicates (and forbid qualifying
-- the join column), and the legacy (+) operator sits flush against a column.
--------------------------------------------------------------------------------
SELECT cal.metric_day,
       m.node_id,
       NVL(m.metric_value, 0) AS filled_value
FROM sq_hard_metric m PARTITION BY (m.node_id)
RIGHT OUTER JOIN (SELECT DATE '2026-01-01' + LEVEL - 1 AS metric_day
                  FROM dual
                  CONNECT BY LEVEL <= 3) cal
  ON cal.metric_day = m.metric_day
ORDER BY m.node_id, cal.metric_day;

SELECT n.node_id,
       peak.metric_name    AS peak_name,
       peak.metric_value   AS peak_value,
       spread.value_spread AS value_spread
FROM (SELECT DISTINCT node_id FROM sq_hard_metric) n
CROSS APPLY (SELECT m.metric_name, m.metric_value
             FROM sq_hard_metric m
             WHERE m.node_id = n.node_id
             ORDER BY m.metric_value DESC, m.metric_id
             FETCH FIRST 1 ROW ONLY) peak
CROSS JOIN LATERAL (SELECT MAX(x.metric_value) - MIN(x.metric_value) AS value_spread
                    FROM sq_hard_metric x
                    WHERE x.node_id = n.node_id) spread
ORDER BY n.node_id;

SELECT node_id, metric_count, peak_value
FROM (SELECT node_id, COUNT(*) AS metric_count
      FROM sq_hard_metric
      GROUP BY node_id)
NATURAL JOIN (SELECT node_id, MAX(metric_value) AS peak_value
              FROM sq_hard_metric
              GROUP BY node_id)
ORDER BY node_id;

SELECT node_id, COUNT(*) AS joined_rows
FROM sq_hard_metric
JOIN sq_hard_v USING (node_id)
GROUP BY node_id
ORDER BY node_id;

SELECT m.metric_id, COUNT(e.edge_id) AS out_edges
FROM sq_hard_metric m, sq_hard_edge e
WHERE e.src_metric_id(+) = m.metric_id
GROUP BY m.metric_id
ORDER BY m.metric_id;

--------------------------------------------------------------------------------
-- W5-B: quantified comparisons, a row-value constructor IN list, IS JSON and
-- JSON_EQUAL conditions, and the ORA_ROWSCN / ROWID pseudo-columns.
--------------------------------------------------------------------------------
SELECT COUNT(*) AS above_all,
       COUNT(CASE WHEN pair_hit = 1 THEN 1 END) AS pair_hits
FROM (SELECT m.metric_id,
             CASE WHEN (m.node_id, m.metric_name) IN ((1, 'LATENCY'), (2, 'ERRORS'))
                  THEN 1 ELSE 0 END AS pair_hit
      FROM sq_hard_metric m
      WHERE m.metric_value >= ALL (SELECT x.metric_value
                                   FROM sq_hard_metric x
                                   WHERE x.node_id = m.node_id)
        AND m.metric_name = ANY ('LATENCY', 'ERRORS')
        AND m.metric_id <> SOME (SELECT y.metric_id FROM sq_hard_metric y));

SELECT COUNT(*) AS json_rows
FROM sq_hard_metric m
WHERE m.payload IS JSON
  AND JSON_EQUAL('{"a":1,"b":[2,3]}', '{"b":[2,3],"a":1}')
  AND NOT ('{oops' IS JSON);

SELECT COUNT(DISTINCT ROWIDTOCHAR(m.ROWID))                       AS distinct_rowids,
       CASE WHEN COUNT(DISTINCT ORA_ROWSCN) >= 1 THEN 'scn-ok' END AS scn_shape
FROM sq_hard_metric m;

--------------------------------------------------------------------------------
-- W5-C: DML through an updatable inline view, then INSERT ... LOG ERRORS
-- routing the primary-key collision into a DBMS_ERRLOG shadow table instead of
-- failing the statement.
--------------------------------------------------------------------------------
CREATE TABLE sq_hard_w5_bulk (
  bulk_id  NUMBER CONSTRAINT sq_hard_w5_bulk_pk PRIMARY KEY,
  amount   NUMBER(10, 2) NOT NULL,
  note     VARCHAR2(30)
) TABLESPACE users;

INSERT INTO sq_hard_w5_bulk (bulk_id, amount, note)
SELECT LEVEL, LEVEL * 10, 'seed' FROM dual CONNECT BY LEVEL <= 5;

UPDATE (SELECT amount, note
        FROM sq_hard_w5_bulk
        WHERE bulk_id <= 2)
SET note = 'inline-view', amount = amount + 1;

DELETE FROM (SELECT * FROM sq_hard_w5_bulk WHERE bulk_id = 5);

BEGIN
  DBMS_ERRLOG.CREATE_ERROR_LOG(
    dml_table_name   => 'SQ_HARD_W5_BULK',
    err_log_table_name => 'SQ_HARD_W5_ERR'
  );
END;
/

INSERT INTO sq_hard_w5_bulk (bulk_id, amount, note)
SELECT 1, 999, 'dup' FROM dual UNION ALL
SELECT 6, 60, 'logged-ok' FROM dual
LOG ERRORS INTO sq_hard_w5_err ('wave5') REJECT LIMIT UNLIMITED;

ALTER TABLE sq_hard_w5_bulk ADD CONSTRAINT sq_hard_w5_bulk_ck
  CHECK (amount >= 0) DEFERRABLE INITIALLY DEFERRED;

SET CONSTRAINTS ALL IMMEDIATE;

CREATE INDEX sq_hard_w5_bulk_fx ON sq_hard_w5_bulk (UPPER(note)) INVISIBLE;

ALTER INDEX sq_hard_w5_bulk_fx VISIBLE;

INSERT INTO sq_hard_w5_note (note_key, note_value)
SELECT 'errlog', COUNT(*) FROM sq_hard_w5_err;

--------------------------------------------------------------------------------
-- W5-D: object type round two. A user-defined constructor returns SELF AS
-- RESULT, a STATIC function builds a zero value, ORDER MEMBER makes the object
-- column sortable, and ALTER TYPE ... ADD ATTRIBUTE CASCADE rewrites live rows
-- before an attribute-path UPDATE fills the new attribute in.
--------------------------------------------------------------------------------
CREATE OR REPLACE TYPE sq_hard_w5_money_t AS OBJECT (
  amount NUMBER,
  CONSTRUCTOR FUNCTION sq_hard_w5_money_t(cents NUMBER, per_unit NUMBER)
    RETURN SELF AS RESULT,
  STATIC FUNCTION zero RETURN sq_hard_w5_money_t,
  MEMBER FUNCTION scaled(factor NUMBER DEFAULT 2) RETURN NUMBER,
  ORDER MEMBER FUNCTION rank_against(other sq_hard_w5_money_t) RETURN INTEGER
);
/

CREATE OR REPLACE TYPE BODY sq_hard_w5_money_t AS
  CONSTRUCTOR FUNCTION sq_hard_w5_money_t(cents NUMBER, per_unit NUMBER)
    RETURN SELF AS RESULT
  IS
  BEGIN
    SELF.amount := cents / NULLIF(per_unit, 0);
    RETURN;
  END;

  STATIC FUNCTION zero RETURN sq_hard_w5_money_t IS
  BEGIN
    RETURN sq_hard_w5_money_t(0, 1);
  END zero;

  MEMBER FUNCTION scaled(factor NUMBER DEFAULT 2) RETURN NUMBER IS
  BEGIN
    RETURN SELF.amount * factor;
  END scaled;

  ORDER MEMBER FUNCTION rank_against(other sq_hard_w5_money_t) RETURN INTEGER IS
  BEGIN
    RETURN CASE
             WHEN other IS NULL THEN 1
             WHEN SELF.amount < other.amount THEN -1
             WHEN SELF.amount > other.amount THEN 1
             ELSE 0
           END;
  END rank_against;
END;
/

CREATE TABLE sq_hard_w5_wallet (
  wallet_id NUMBER CONSTRAINT sq_hard_w5_wallet_pk PRIMARY KEY,
  balance   sq_hard_w5_money_t
) TABLESPACE users;

INSERT INTO sq_hard_w5_wallet VALUES (1, sq_hard_w5_money_t(500, 5));
INSERT INTO sq_hard_w5_wallet VALUES (2, sq_hard_w5_money_t(90));
INSERT INTO sq_hard_w5_wallet VALUES (3, sq_hard_w5_money_t.zero());

ALTER TYPE sq_hard_w5_money_t
  ADD ATTRIBUTE (currency VARCHAR2(3)) CASCADE INCLUDING TABLE DATA;

ALTER TYPE sq_hard_w5_money_t COMPILE BODY;

UPDATE sq_hard_w5_wallet w
SET w.balance.currency = CASE WHEN w.wallet_id = 3 THEN 'KRW' ELSE 'USD' END;

SELECT w.wallet_id,
       w.balance.amount        AS balance_amount,
       w.balance.currency      AS balance_currency,
       w.balance.scaled(3)     AS tripled,
       w.balance.scaled()      AS doubled
FROM sq_hard_w5_wallet w
ORDER BY w.balance, w.wallet_id;

INSERT INTO sq_hard_w5_note (note_key, note_value)
SELECT 'wallet', SUM(w.balance.amount) FROM sq_hard_w5_wallet w;

--------------------------------------------------------------------------------
-- W5-E: PIVOT XML with the ANY pseudo-value, a WITH clause carrying an explicit
-- column alias list, GROUPING SETS with an empty grouping set, a COLLATE'd
-- ORDER BY, and a seeded block SAMPLE.
--------------------------------------------------------------------------------
SELECT px.node_id,
       CASE WHEN INSTR(XMLSERIALIZE(CONTENT px.metric_name_xml), 'LATENCY') > 0
            THEN 'has-latency' ELSE 'no-latency' END AS xml_shape
FROM (SELECT node_id, metric_name, metric_value FROM sq_hard_metric)
PIVOT XML (SUM(metric_value) AS total FOR metric_name IN (ANY)) px
ORDER BY px.node_id;

WITH tally (bucket_name, bucket_total, bucket_rows) AS (
  SELECT metric_name, SUM(metric_value), COUNT(*)
  FROM sq_hard_metric
  GROUP BY metric_name
), rollup_grid AS (
  SELECT metric_name,
         GROUPING(metric_name) AS is_total,
         SUM(metric_value)     AS grid_total
  FROM sq_hard_metric
  GROUP BY GROUPING SETS ((metric_name), ())
)
SELECT t.bucket_name,
       t.bucket_total,
       t.bucket_rows,
       g.grid_total,
       g.is_total
FROM tally t
JOIN rollup_grid g
  ON g.metric_name = t.bucket_name
ORDER BY t.bucket_name COLLATE BINARY_CI, t.bucket_total DESC NULLS LAST;

SELECT CASE WHEN COUNT(*) BETWEEN 0 AND 4 THEN 'sample-ok' END AS sample_shape
FROM sq_hard_metric SAMPLE (99) SEED (7);

--------------------------------------------------------------------------------
-- W5-F: a session-scoped global temporary table that survives the commit, and a
-- private temporary table whose ORA$PTT_ name carries a dollar sign.
--------------------------------------------------------------------------------
CREATE GLOBAL TEMPORARY TABLE sq_hard_w5_gtt (
  slot_id   NUMBER,
  slot_note VARCHAR2(30)
) ON COMMIT PRESERVE ROWS;

INSERT INTO sq_hard_w5_gtt (slot_id, slot_note)
SELECT LEVEL, 'gtt-' || LEVEL FROM dual CONNECT BY LEVEL <= 3;

CREATE PRIVATE TEMPORARY TABLE ora$ptt_sq_hard_w5 (
  slot_id   NUMBER,
  slot_note VARCHAR2(30)
) ON COMMIT PRESERVE DEFINITION;

INSERT INTO sq_hard_w5_note (note_key, note_value)
SELECT 'ptt', COUNT(*)
FROM user_private_temp_tables
WHERE table_name = 'ORA$PTT_SQ_HARD_W5';

DROP TABLE ora$ptt_sq_hard_w5;

INSERT INTO sq_hard_w5_note (note_key, note_value)
SELECT 'gtt', COUNT(*) FROM sq_hard_w5_gtt;

--------------------------------------------------------------------------------
-- W5-G: subprogram attribute stacking (DETERMINISTIC / PARALLEL_ENABLE /
-- RESULT_CACHE), a package with two overloads of one name and a RANGE-limited
-- SUBTYPE, an ACCESSIBLE BY whitelist, and a $ERROR arm that must stay unlit.
--------------------------------------------------------------------------------
CREATE OR REPLACE FUNCTION sq_hard_w5_scale(p_value NUMBER)
  RETURN NUMBER
  DETERMINISTIC
  PARALLEL_ENABLE
  RESULT_CACHE
IS
BEGIN
  RETURN p_value * 3;
END sq_hard_w5_scale;
/

CREATE OR REPLACE PROCEDURE sq_hard_w5_guarded(p_seed IN NUMBER,
                                               p_out  OUT NOCOPY NUMBER)
  ACCESSIBLE BY (PACKAGE sq_hard_w5_pkg)
IS
BEGIN
  p_out := sq_hard_w5_scale(p_seed) + 1;
END sq_hard_w5_guarded;
/

CREATE OR REPLACE PACKAGE sq_hard_w5_pkg AUTHID CURRENT_USER AS
  SUBTYPE small_count IS PLS_INTEGER RANGE 0 .. 9999;
  FUNCTION render(p_value NUMBER) RETURN VARCHAR2;
  FUNCTION render(p_value VARCHAR2, p_pad small_count DEFAULT 4) RETURN VARCHAR2;
  FUNCTION guarded_total(p_seed NUMBER) RETURN NUMBER;
END sq_hard_w5_pkg;
/

CREATE OR REPLACE PACKAGE BODY sq_hard_w5_pkg AS
  FUNCTION render(p_value NUMBER) RETURN VARCHAR2 IS
  BEGIN
    $IF DBMS_DB_VERSION.VER_LE_10 $THEN
      $ERROR 'sq_hard_w5_pkg requires 11g or later' $END
    $END
    RETURN 'n:' || TO_CHAR(p_value);
  END render;

  FUNCTION render(p_value VARCHAR2, p_pad small_count DEFAULT 4)
    RETURN VARCHAR2 IS
  BEGIN
    RETURN 's:' || LPAD(p_value, p_pad, '.');
  END render;

  FUNCTION guarded_total(p_seed NUMBER) RETURN NUMBER IS
    l_out NUMBER;
  BEGIN
    sq_hard_w5_guarded(p_seed => p_seed, p_out => l_out);
    RETURN l_out;
  END guarded_total;
END sq_hard_w5_pkg;
/

SELECT sq_hard_w5_pkg.render(12)             AS numeric_render,
       sq_hard_w5_pkg.render('ab')           AS text_render,
       sq_hard_w5_pkg.render('ab', p_pad => 6) AS padded_render,
       sq_hard_w5_pkg.guarded_total(5)       AS guarded_total
FROM dual;

--------------------------------------------------------------------------------
-- W5-H: bulk-binding torture. INDICES OF walks a sparse collection, VALUES OF
-- walks an index collection, SQL%BULK_ROWCOUNT is read per iteration, and the
-- DELETE hands its keys back through RETURNING ... BULK COLLECT INTO.
--------------------------------------------------------------------------------
DECLARE
  TYPE id_tab  IS TABLE OF sq_hard_w5_bulk.bulk_id%TYPE;
  TYPE idx_tab IS TABLE OF PLS_INTEGER;

  ids         id_tab  := id_tab(1, 2, 3, 4, 6);
  chosen      idx_tab := idx_tab(2, 5);
  removed     id_tab;
  touched     PLS_INTEGER := 0;
  second_rows PLS_INTEGER := 0;
  removed_ct  PLS_INTEGER := 0;
  dml_errors  PLS_INTEGER := 0;
  walker      PLS_INTEGER;
BEGIN
  ids.DELETE(3);

  FORALL i IN INDICES OF ids SAVE EXCEPTIONS
    UPDATE sq_hard_w5_bulk
    SET amount = amount + 1
    WHERE bulk_id = ids(i);

  walker := ids.FIRST;
  WHILE walker IS NOT NULL LOOP
    touched := touched + NVL(SQL%BULK_ROWCOUNT(walker), 0);
    walker  := ids.NEXT(walker);
  END LOOP;

  FORALL j IN VALUES OF chosen
    UPDATE sq_hard_w5_bulk
    SET note = note || '+v'
    WHERE bulk_id = ids(j);

  second_rows := SQL%ROWCOUNT;

  DELETE FROM sq_hard_w5_bulk
  WHERE bulk_id >= 6
  RETURNING bulk_id BULK COLLECT INTO removed;

  removed_ct := removed.COUNT;

  INSERT INTO sq_hard_w5_note (note_key, note_value) VALUES ('bulk_touched', touched);
  INSERT INTO sq_hard_w5_note (note_key, note_value) VALUES ('bulk_second', second_rows);
  INSERT INTO sq_hard_w5_note (note_key, note_value) VALUES ('bulk_removed', removed_ct);
EXCEPTION
  WHEN OTHERS THEN
    IF SQLCODE = -24381 THEN
      dml_errors := SQL%BULK_EXCEPTIONS.COUNT;
      RAISE_APPLICATION_ERROR(-20090, 'unexpected bulk errors ' || dml_errors);
    END IF;
    RAISE;
END;
/

--------------------------------------------------------------------------------
-- W5-I: hint torture. The single-line --+ form must keep its own line or it
-- comments the projection out, and the block form carries query-block names
-- that are referenced from an outer hint with @.
--------------------------------------------------------------------------------
SELECT --+ FULL(m) NO_PARALLEL(m)
       m.metric_id,
       m.metric_value
FROM sq_hard_metric m
WHERE m.metric_id <= 2
ORDER BY m.metric_id;

SELECT /*+ QB_NAME(outer_qb)
           LEADING(@outer_qb m@outer_qb e@outer_qb)
           NO_MERGE(@inner_qb) */
       m.metric_id, e.edge_cost
FROM sq_hard_metric m,
     (SELECT /*+ QB_NAME(inner_qb) */
             src_metric_id, MIN(hop_cost) AS edge_cost
      FROM sq_hard_edge
      GROUP BY src_metric_id) e
WHERE e.src_metric_id = m.metric_id
ORDER BY m.metric_id;

ALTER SESSION SET STATISTICS_LEVEL = ALL;

ALTER SESSION SET OPTIMIZER_MODE = ALL_ROWS;

--------------------------------------------------------------------------------
-- W5-J: wave-5 self-verification.
--------------------------------------------------------------------------------
DECLARE
  dense_rows   PLS_INTEGER;
  apply_spread NUMBER;
  outer_join   PLS_INTEGER;
  quant_rows   PLS_INTEGER;
  bulk_rows    PLS_INTEGER;
  bulk_sum     NUMBER;
  wallet_sum   NUMBER;
  wallet_cur   VARCHAR2(3);
  errlog_rows  NUMBER;
  gtt_rows     NUMBER;
  ptt_rows     NUMBER;
  touched      NUMBER;
  second_rows  NUMBER;
  removed_rows NUMBER;
  render_text  VARCHAR2(40);
  guarded_val  NUMBER;
  pivot_hits   PLS_INTEGER;
BEGIN
  SELECT COUNT(*)
  INTO dense_rows
  FROM sq_hard_metric m PARTITION BY (m.node_id)
  RIGHT OUTER JOIN (SELECT DATE '2026-01-01' + LEVEL - 1 AS metric_day
                    FROM dual
                    CONNECT BY LEVEL <= 3) cal
    ON cal.metric_day = m.metric_day;

  SELECT MAX(spread.value_spread)
  INTO apply_spread
  FROM (SELECT DISTINCT node_id FROM sq_hard_metric) n
  CROSS JOIN LATERAL (SELECT MAX(x.metric_value) - MIN(x.metric_value) AS value_spread
                      FROM sq_hard_metric x
                      WHERE x.node_id = n.node_id) spread;

  SELECT COUNT(*)
  INTO outer_join
  FROM sq_hard_metric m, sq_hard_edge e
  WHERE e.src_metric_id(+) = m.metric_id;

  SELECT COUNT(*)
  INTO quant_rows
  FROM sq_hard_metric m
  WHERE m.metric_value >= ALL (SELECT x.metric_value
                               FROM sq_hard_metric x
                               WHERE x.node_id = m.node_id);

  SELECT COUNT(*), SUM(amount) INTO bulk_rows, bulk_sum FROM sq_hard_w5_bulk;
  SELECT SUM(w.balance.amount) INTO wallet_sum FROM sq_hard_w5_wallet w;
  SELECT w.balance.currency INTO wallet_cur
  FROM sq_hard_w5_wallet w WHERE w.wallet_id = 3;

  SELECT MAX(CASE note_key WHEN 'errlog'       THEN note_value END),
         MAX(CASE note_key WHEN 'gtt'          THEN note_value END),
         MAX(CASE note_key WHEN 'ptt'          THEN note_value END),
         MAX(CASE note_key WHEN 'bulk_touched' THEN note_value END),
         MAX(CASE note_key WHEN 'bulk_second'  THEN note_value END),
         MAX(CASE note_key WHEN 'bulk_removed' THEN note_value END)
  INTO errlog_rows, gtt_rows, ptt_rows, touched, second_rows, removed_rows
  FROM sq_hard_w5_note;

  render_text := sq_hard_w5_pkg.render('ab', 6);
  guarded_val := sq_hard_w5_pkg.guarded_total(5);

  SELECT COUNT(*)
  INTO pivot_hits
  FROM (SELECT node_id, metric_name, metric_value FROM sq_hard_metric)
  PIVOT XML (SUM(metric_value) AS total FOR metric_name IN (ANY));

  IF dense_rows <> 6 THEN
    RAISE_APPLICATION_ERROR(-20080, 'partitioned outer join ' || dense_rows);
  END IF;
  IF apply_spread <> 12 THEN
    RAISE_APPLICATION_ERROR(-20081, 'lateral spread ' || apply_spread);
  END IF;
  IF outer_join <> 5 THEN
    RAISE_APPLICATION_ERROR(-20082, 'legacy outer join ' || outer_join);
  END IF;
  IF quant_rows <> 2 THEN
    RAISE_APPLICATION_ERROR(-20083, 'quantified rows ' || quant_rows);
  END IF;
  IF bulk_rows <> 4 OR bulk_sum <> 105 THEN
    RAISE_APPLICATION_ERROR(-20084, 'bulk ' || bulk_rows || '/' || bulk_sum);
  END IF;
  IF wallet_sum <> 190 OR wallet_cur <> 'KRW' THEN
    RAISE_APPLICATION_ERROR(-20085, 'wallet ' || wallet_sum || '/' || wallet_cur);
  END IF;
  IF errlog_rows <> 1 OR gtt_rows <> 3 OR ptt_rows <> 1 THEN
    RAISE_APPLICATION_ERROR(
      -20086,
      'temp/errlog ' || errlog_rows || '/' || gtt_rows || '/' || ptt_rows
    );
  END IF;
  IF touched <> 4 OR second_rows <> 2 OR removed_rows <> 1 THEN
    RAISE_APPLICATION_ERROR(
      -20087,
      'forall ' || touched || '/' || second_rows || '/' || removed_rows
    );
  END IF;
  IF render_text <> 's:....ab' OR guarded_val <> 16 THEN
    RAISE_APPLICATION_ERROR(-20088, 'package ' || render_text || '/' || guarded_val);
  END IF;
  IF pivot_hits <> 2 THEN
    RAISE_APPLICATION_ERROR(-20089, 'pivot xml ' || pivot_hits);
  END IF;
END;
/
--------------------------------------------------------------------------------
-- ULTRA WAVE 6: the PL/SQL JSON and XMLTYPE object APIs, a fast-refresh
-- materialized view fed by its own log, physical-storage DDL (index-organized
-- table, cluster, unused columns, recyclebin recovery), flashback version
-- pseudo-columns, approximate and bitwise aggregates, DBMS_SQL handed to a
-- native ref cursor, INSTEAD OF triggers with renamed correlation names, a
-- scalar SQL macro, WITH PROCEDURE, referential DDL, and a lexer torture round.
--------------------------------------------------------------------------------
CREATE TABLE sq_hard_w6_note (
  note_key   VARCHAR2(30) CONSTRAINT sq_hard_w6_note_pk PRIMARY KEY,
  note_value NUMBER,
  note_text  VARCHAR2(400)
) TABLESPACE users;

-- Created first so the flashback version query in W6-E has a settled history.
CREATE TABLE sq_hard_w6_ver (
  ver_id NUMBER CONSTRAINT sq_hard_w6_ver_pk PRIMARY KEY,
  state  VARCHAR2(12) NOT NULL
) TABLESPACE users;

INSERT INTO sq_hard_w6_ver (ver_id, state) VALUES (1, 'created');
COMMIT;

UPDATE sq_hard_w6_ver SET state = 'changed' WHERE ver_id = 1;
COMMIT;

--------------------------------------------------------------------------------
-- W6-A: the PL/SQL JSON object API. Method chains through JSON_OBJECT_T /
-- JSON_ARRAY_T / JSON_ELEMENT_T mix mixed-case member names with SQL keywords
-- (get, put, parse) and a varray key list walked by COUNT.
--------------------------------------------------------------------------------
DECLARE
  doc      JSON_OBJECT_T;
  limits   JSON_OBJECT_T;
  tag_arr  JSON_ARRAY_T;
  elem     JSON_ELEMENT_T;
  key_list JSON_KEY_LIST;
  key_line VARCHAR2(200);
  hard_cap NUMBER;
  key_count PLS_INTEGER;
  tag_count PLS_INTEGER;
  shape_txt VARCHAR2(30);
  spare_txt VARCHAR2(30);
BEGIN
  doc := JSON_OBJECT_T.parse(
    '{"node":"alpha","limits":{"soft":10,"hard":20},"tags":["sql","json"]}');
  limits := doc.get_Object('limits');
  hard_cap := limits.get_Number('hard');
  tag_arr := doc.get_Array('tags');
  tag_arr.append('wave6');
  doc.put('tags', tag_arr);
  doc.put('checked', TRUE);
  doc.put_null('spare');
  elem := doc.get('limits');
  IF elem.is_Object() AND NOT elem.is_Array() THEN
    doc.put('shape', 'object');
  END IF;
  key_list := doc.get_Keys;
  FOR key_pos IN 1 .. key_list.COUNT LOOP
    key_line := key_line || CASE WHEN key_line IS NULL THEN '' ELSE '|' END
                || key_list(key_pos);
  END LOOP;

  key_count := key_list.COUNT;
  tag_count := tag_arr.get_size;
  shape_txt := doc.get_String('shape');
  spare_txt := CASE WHEN doc.has('spare') THEN 'has-spare' END;

  INSERT INTO sq_hard_w6_note (note_key, note_value, note_text)
  VALUES ('json_api', hard_cap + tag_count,
          shape_txt || ':' || key_count || ':' || spare_txt || ':' || key_line);
END;
/

SELECT note_key, note_value, note_text
FROM sq_hard_w6_note
WHERE note_key = 'json_api';

--------------------------------------------------------------------------------
-- W6-B: XMLTYPE member functions chained off a local variable and SYS.ANYDATA
-- reflection. Both are object method paths that look like package calls.
--------------------------------------------------------------------------------
DECLARE
  doc_xml  XMLTYPE;
  leaf_xml XMLTYPE;
  leaf_txt VARCHAR2(200);
  root_tag VARCHAR2(60);
  has_node PLS_INTEGER;
  any_val  SYS.ANYDATA;
  any_kind VARCHAR2(80);
  any_num  NUMBER;
  any_ok   PLS_INTEGER;
BEGIN
  doc_xml  := XMLTYPE('<metrics unit="ms"><m id="1">12</m><m id="2">18</m></metrics>');
  leaf_xml := doc_xml.extract('/metrics/m[@id="2"]/text()');
  leaf_txt := leaf_xml.getStringVal();
  root_tag := doc_xml.getRootElement();
  has_node := doc_xml.existsNode('/metrics/m[@id="1"]');

  any_val  := SYS.ANYDATA.convertNumber(has_node * 21);
  any_kind := any_val.getTypeName();
  any_ok   := any_val.getNumber(any_num);

  INSERT INTO sq_hard_w6_note (note_key, note_value, note_text)
  VALUES ('xml_anydata', any_num + TO_NUMBER(leaf_txt),
          root_tag || ':' || any_kind || ':' || any_ok || ':' || leaf_txt);
END;
/

--------------------------------------------------------------------------------
-- W6-C: materialized view log with SEQUENCE / INCLUDING NEW VALUES feeding a
-- FAST REFRESH ON COMMIT aggregate materialized view.
--------------------------------------------------------------------------------
CREATE TABLE sq_hard_w6_fact (
  fact_id NUMBER CONSTRAINT sq_hard_w6_fact_pk PRIMARY KEY,
  bucket  VARCHAR2(10) NOT NULL,
  qty     NUMBER(8, 2) NOT NULL
) TABLESPACE users;

INSERT INTO sq_hard_w6_fact (fact_id, bucket, qty) VALUES (1, 'alpha', 10);
INSERT INTO sq_hard_w6_fact (fact_id, bucket, qty) VALUES (2, 'alpha', 15);
INSERT INTO sq_hard_w6_fact (fact_id, bucket, qty) VALUES (3, 'beta', 4);
COMMIT;

CREATE MATERIALIZED VIEW LOG ON sq_hard_w6_fact
  WITH ROWID, SEQUENCE (bucket, qty)
  INCLUDING NEW VALUES;

CREATE MATERIALIZED VIEW sq_hard_w6_mv
  BUILD IMMEDIATE
  REFRESH FAST ON COMMIT
  ENABLE QUERY REWRITE
AS
  SELECT bucket,
         COUNT(*)   AS bucket_rows,
         SUM(qty)   AS bucket_qty,
         COUNT(qty) AS qty_rows
  FROM sq_hard_w6_fact
  GROUP BY bucket;

INSERT INTO sq_hard_w6_fact (fact_id, bucket, qty) VALUES (4, 'beta', 6);
COMMIT;

SELECT bucket, bucket_rows, bucket_qty
FROM sq_hard_w6_mv
ORDER BY bucket;

--------------------------------------------------------------------------------
-- W6-D: physical storage grammar - index-organized table with an overflow
-- segment, a hash-free cluster with a clustered table, unused-column surgery,
-- and a table recovered out of the recyclebin.
--------------------------------------------------------------------------------
CREATE TABLE sq_hard_w6_iot (
  iot_key  NUMBER,
  iot_name VARCHAR2(20),
  iot_note VARCHAR2(100),
  CONSTRAINT sq_hard_w6_iot_pk PRIMARY KEY (iot_key, iot_name)
) ORGANIZATION INDEX
  PCTTHRESHOLD 20
  INCLUDING iot_name
  OVERFLOW TABLESPACE users;

INSERT INTO sq_hard_w6_iot (iot_key, iot_name, iot_note) VALUES (1, 'a', 'first');
INSERT INTO sq_hard_w6_iot (iot_key, iot_name, iot_note) VALUES (2, 'b', 'second');

CREATE CLUSTER sq_hard_w6_cl (cluster_key NUMBER) SIZE 512 TABLESPACE users;

CREATE INDEX sq_hard_w6_cl_ix ON CLUSTER sq_hard_w6_cl;

CREATE TABLE sq_hard_w6_clustered (
  cluster_key NUMBER NOT NULL,
  payload     VARCHAR2(30)
) CLUSTER sq_hard_w6_cl (cluster_key);

INSERT INTO sq_hard_w6_clustered (cluster_key, payload) VALUES (1, 'clustered');

CREATE TABLE sq_hard_w6_ddl (
  ddl_id      NUMBER CONSTRAINT sq_hard_w6_ddl_pk PRIMARY KEY,
  legacy_flag CHAR(1),
  old_name    VARCHAR2(20),
  note        VARCHAR2(30)
) TABLESPACE users;

INSERT INTO sq_hard_w6_ddl (ddl_id, legacy_flag, old_name, note)
VALUES (1, 'Y', 'renamed-me', 'kept');

ALTER TABLE sq_hard_w6_ddl SET UNUSED COLUMN legacy_flag ONLINE;
ALTER TABLE sq_hard_w6_ddl DROP UNUSED COLUMNS CHECKPOINT 250;
ALTER TABLE sq_hard_w6_ddl RENAME COLUMN old_name TO new_name;
ALTER TABLE sq_hard_w6_ddl MODIFY (note VARCHAR2(60) DEFAULT 'w6-default');
ALTER TABLE sq_hard_w6_ddl ENABLE ROW MOVEMENT;

INSERT INTO sq_hard_w6_ddl (ddl_id, new_name) VALUES (2, 'defaulted');

SELECT ddl_id, new_name, note FROM sq_hard_w6_ddl ORDER BY ddl_id;

CREATE TABLE sq_hard_w6_recycle (
  bin_id NUMBER CONSTRAINT sq_hard_w6_recycle_pk PRIMARY KEY,
  label  VARCHAR2(20)
) TABLESPACE users;

INSERT INTO sq_hard_w6_recycle (bin_id, label) VALUES (1, 'restored');
COMMIT;

DROP TABLE sq_hard_w6_recycle;

FLASHBACK TABLE sq_hard_w6_recycle TO BEFORE DROP;

SELECT label FROM sq_hard_w6_recycle ORDER BY bin_id;

--------------------------------------------------------------------------------
-- W6-E: row-version pseudo-columns from a VERSIONS BETWEEN flashback query,
-- locking modifiers that read like ordinary keywords, and named transactions.
--------------------------------------------------------------------------------
-- A flashback version range that reaches back across a fresh CREATE TABLE is
-- rejected with ORA-01466 until the DDL leaves the current timestamp bucket.
BEGIN
  DBMS_SESSION.SLEEP(6);
END;
/

SELECT CASE WHEN COUNT(*) >= 1 THEN 'versions-ok' END       AS version_shape,
       CASE WHEN COUNT(v.VERSIONS_STARTSCN) >= 0 THEN 'scn-ok' END AS scn_shape,
       CASE WHEN COUNT(CASE WHEN v.VERSIONS_OPERATION IN ('I', 'U', 'D')
                          THEN 1 END) >= 0
            THEN 'ops-ok' END                                AS operation_shape
FROM sq_hard_w6_ver VERSIONS BETWEEN SCN MINVALUE AND MAXVALUE v
WHERE v.ver_id = 1;

SELECT ver_id, state
FROM sq_hard_w6_ver
WHERE ver_id = 1
FOR UPDATE OF state WAIT 3;

COMMIT;

SELECT ver_id
FROM sq_hard_w6_ver
ORDER BY ver_id
FOR UPDATE SKIP LOCKED;

COMMIT;

SET TRANSACTION ISOLATION LEVEL SERIALIZABLE NAME 'sq_hard_w6_tx';

SELECT COUNT(*) AS serializable_rows FROM sq_hard_w6_ver;

COMMIT;

SET TRANSACTION READ ONLY;

SELECT COUNT(*) AS read_only_rows FROM sq_hard_w6_ver;

COMMIT;

--------------------------------------------------------------------------------
-- W6-F: aggregate/analytic zoo - approximate aggregates with a DETERMINISTIC
-- modifier, 23ai bitwise aggregates, an analytic LISTAGG, and regular
-- expressions whose escapes look like broken string literals.
--------------------------------------------------------------------------------
SELECT APPROX_COUNT_DISTINCT(metric_name)                          AS approx_names,
       APPROX_PERCENTILE(0.5 DETERMINISTIC)
         WITHIN GROUP (ORDER BY metric_value)                      AS approx_median,
       BIT_OR_AGG(TRUNC(metric_value))                             AS bits_or,
       BIT_AND_AGG(TRUNC(metric_value))                            AS bits_and,
       BIT_XOR_AGG(TRUNC(metric_value))                            AS bits_xor,
       CASE WHEN CHECKSUM(metric_value) IS NOT NULL
            THEN 'checksum-ok' END                                 AS checksum_shape,
       CASE WHEN ANY_VALUE(metric_name) IN ('LATENCY', 'ERRORS')
            THEN 'any-ok' END                                      AS any_shape
FROM sq_hard_metric;

SELECT m.metric_id,
       LISTAGG(m.metric_name, ',') WITHIN GROUP (ORDER BY m.metric_id)
         OVER (PARTITION BY m.node_id)                      AS node_names,
       REGEXP_SUBSTR('a1-b22-c3', '([a-z])(\d+)', 1, 2, 'i', 2) AS second_digits,
       REGEXP_REPLACE('a1-b22', '([a-z])(\d+)', '\2:\1')        AS swapped,
       REGEXP_COUNT('x/*y*/z--w', '[*/-]')                       AS punct_hits,
       REGEXP_INSTR('one two three', '\s\w+\s', 1, 1, 1, 'x')    AS after_match
FROM sq_hard_metric m
ORDER BY m.metric_id;

--------------------------------------------------------------------------------
-- W6-G: PL/SQL torture round two - a locally declared collection type used in
-- static SQL through TABLE(), every collection method in one block, a
-- multi-exception handler, PRAGMA INLINE, and a DBMS_SQL cursor handed to a
-- native ref cursor mid-fetch.
--------------------------------------------------------------------------------
CREATE OR REPLACE PACKAGE sq_hard_w6_types AS
  TYPE span_tab IS TABLE OF NUMBER;
END sq_hard_w6_types;
/

DECLARE
  TYPE span_arr IS VARRAY(8) OF NUMBER;
  spans      sq_hard_w6_types.span_tab := sq_hard_w6_types.span_tab(5, 3, 9, 1);
  bounded    span_arr := span_arr(2, 4);
  table_sum  NUMBER;
  probe_line VARCHAR2(200);
  cur_id     INTEGER;
  exec_rows  INTEGER;
  ref_cur    SYS_REFCURSOR;
  got_id     NUMBER;
  got_name   VARCHAR2(32);
  fetched    PLS_INTEGER := 0;
  missing    NUMBER;

  FUNCTION describe(p_at PLS_INTEGER) RETURN VARCHAR2 IS
  BEGIN
    PRAGMA INLINE(describe, 'YES');
    RETURN 'at' || p_at;
  END describe;
BEGIN
  SELECT SUM(COLUMN_VALUE) INTO table_sum FROM TABLE(spans);

  spans.EXTEND;
  spans(spans.LAST) := 7;
  spans.TRIM(1);
  spans.DELETE(2);
  bounded.EXTEND(2, 1);

  probe_line := describe(spans.FIRST)
                || CASE WHEN spans.EXISTS(2) THEN '/live' ELSE '/gone' END
                || '/' || spans.NEXT(1)
                || '/' || spans.PRIOR(spans.LAST)
                || '/' || spans.COUNT
                || '/' || bounded.LIMIT
                || '/' || bounded.COUNT;

  cur_id := DBMS_SQL.OPEN_CURSOR;
  DBMS_SQL.PARSE(
    cur_id,
    'SELECT metric_id, metric_name FROM sq_hard_metric WHERE node_id = :node_bind'
      || ' ORDER BY metric_id',
    DBMS_SQL.NATIVE
  );
  DBMS_SQL.BIND_VARIABLE(cur_id, ':node_bind', 1);
  exec_rows := DBMS_SQL.EXECUTE(cur_id);
  ref_cur := DBMS_SQL.TO_REFCURSOR(cur_id);
  LOOP
    FETCH ref_cur INTO got_id, got_name;
    EXIT WHEN ref_cur%NOTFOUND;
    fetched := fetched + 1;
  END LOOP;
  CLOSE ref_cur;

  BEGIN
    SELECT metric_value INTO missing FROM sq_hard_metric WHERE 1 = 0;
  EXCEPTION
    WHEN NO_DATA_FOUND OR TOO_MANY_ROWS THEN
      missing := -1;
  END;

  INSERT INTO sq_hard_w6_note (note_key, note_value, note_text)
  VALUES ('plsql_round2', table_sum + fetched + missing + exec_rows, probe_line);
END;
/

ALTER SESSION SET PLSQL_CCFLAGS = 'sq_hard_w6_level:3';

CREATE OR REPLACE PACKAGE sq_hard_w6_reuse AS
  PRAGMA SERIALLY_REUSABLE;
  FUNCTION tick RETURN PLS_INTEGER;
END sq_hard_w6_reuse;
/

CREATE OR REPLACE PACKAGE BODY sq_hard_w6_reuse AS
  PRAGMA SERIALLY_REUSABLE;
  g_ticks PLS_INTEGER := 0;

  FUNCTION tick RETURN PLS_INTEGER IS
  BEGIN
    g_ticks := g_ticks + 1;
    RETURN g_ticks;
  END tick;
END sq_hard_w6_reuse;
/

CREATE OR REPLACE FUNCTION sq_hard_w6_span(p_scale NUMBER DEFAULT 1)
  RETURN NUMBER
  DETERMINISTIC
IS
  base NUMBER;
BEGIN
$IF $$sq_hard_w6_level >= 3 $THEN
  base := 30;
$ELSIF $$sq_hard_w6_level IS NULL $THEN
  base := 0;
$ELSE
  base := 10;
$END
  RETURN base * p_scale;
END sq_hard_w6_span;
/

SELECT sq_hard_w6_span(p_scale => 2) AS scaled_span FROM dual;

--------------------------------------------------------------------------------
-- W6-H: INSTEAD OF trigger over a join-free view with renamed correlation
-- names, plus a WHEN-restricted row trigger ordered by FOLLOWS on the base
-- table.
--------------------------------------------------------------------------------
CREATE OR REPLACE VIEW sq_hard_w6_iov AS
SELECT f.fact_id, f.bucket, f.qty, f.qty * 2 AS doubled_qty
FROM sq_hard_w6_fact f;

CREATE OR REPLACE TRIGGER sq_hard_w6_iov_trg
INSTEAD OF INSERT OR UPDATE ON sq_hard_w6_iov
REFERENCING NEW AS incoming OLD AS existing
FOR EACH ROW
BEGIN
  IF INSERTING THEN
    INSERT INTO sq_hard_w6_fact (fact_id, bucket, qty)
    VALUES (:incoming.fact_id, :incoming.bucket, :incoming.qty);
  ELSIF UPDATING THEN
    UPDATE sq_hard_w6_fact
    SET qty = :incoming.qty
    WHERE fact_id = :existing.fact_id;
  END IF;
END;
/

CREATE OR REPLACE TRIGGER sq_hard_w6_iov_late
INSTEAD OF INSERT ON sq_hard_w6_iov
REFERENCING NEW AS incoming
FOR EACH ROW
FOLLOWS sq_hard_w6_iov_trg
BEGIN
  UPDATE sq_hard_w6_fact
  SET bucket = LOWER(:incoming.bucket)
  WHERE fact_id = :incoming.fact_id;
END;
/

INSERT INTO sq_hard_w6_iov (fact_id, bucket, qty) VALUES (5, 'GAMMA', 8);

UPDATE sq_hard_w6_iov SET qty = 9 WHERE fact_id = 5;

COMMIT;

SELECT fact_id, bucket, qty, doubled_qty
FROM sq_hard_w6_iov
WHERE fact_id = 5;

--------------------------------------------------------------------------------
-- W6-I: scalar SQL macro whose body is a bare expression string, a WITH clause
-- carrying both a PROCEDURE and a FUNCTION, and read-only / check-option views
-- plus a synonym over the same base table.
--------------------------------------------------------------------------------
CREATE OR REPLACE FUNCTION sq_hard_w6_pct(p_part NUMBER, p_whole NUMBER)
  RETURN VARCHAR2 SQL_MACRO(SCALAR)
IS
BEGIN
  RETURN 'ROUND(p_part * 100 / NULLIF(p_whole, 0), 2)';
END sq_hard_w6_pct;
/

SELECT bucket,
       sq_hard_w6_pct(SUM(qty), (SELECT SUM(qty) FROM sq_hard_w6_fact)) AS share_pct
FROM sq_hard_w6_fact
GROUP BY bucket
ORDER BY bucket;

WITH
  PROCEDURE bump(io_value IN OUT NUMBER) IS
  BEGIN
    io_value := io_value * 2;
  END;
  FUNCTION bumped(p_value NUMBER) RETURN NUMBER IS
    local_value NUMBER := p_value;
  BEGIN
    bump(local_value);
    RETURN local_value + 1;
  END;
SELECT fact_id, bumped(qty) AS bumped_qty
FROM sq_hard_w6_fact
WHERE fact_id <= 2
ORDER BY fact_id;
/

CREATE OR REPLACE FORCE EDITIONABLE VIEW sq_hard_w6_checked AS
SELECT fact_id, bucket, qty
FROM sq_hard_w6_fact
WHERE qty >= 0
WITH CHECK OPTION CONSTRAINT sq_hard_w6_checked_ck;

CREATE OR REPLACE VIEW sq_hard_w6_frozen AS
SELECT bucket, SUM(qty) AS bucket_qty
FROM sq_hard_w6_fact
GROUP BY bucket
WITH READ ONLY;

CREATE OR REPLACE SYNONYM sq_hard_w6_syn FOR sq_hard_w6_fact;

UPDATE sq_hard_w6_checked SET qty = qty + 0 WHERE fact_id = 1;

SELECT (SELECT COUNT(*) FROM sq_hard_w6_syn)    AS synonym_rows,
       (SELECT COUNT(*) FROM sq_hard_w6_frozen) AS frozen_buckets
FROM dual;

CREATE SEQUENCE sq_hard_w6_seq START WITH 1 INCREMENT BY 2 NOCACHE ORDER;

SELECT sq_hard_w6_seq.NEXTVAL AS first_value FROM dual;

ALTER SEQUENCE sq_hard_w6_seq RESTART START WITH 50;

SELECT sq_hard_w6_seq.NEXTVAL AS restarted_value FROM dual;

--------------------------------------------------------------------------------
-- W6-J: referential grammar and object metadata - a self-referencing FOREIGN
-- KEY with ON DELETE SET NULL, COMMENT ON whose text contains comment openers,
-- GRANT/REVOKE, an unconditional multi-table INSERT ALL, TRUNCATE with a
-- storage clause, and ANALYZE.
--------------------------------------------------------------------------------
ALTER TABLE sq_hard_w6_ddl ADD CONSTRAINT sq_hard_w6_ddl_uq UNIQUE (new_name);

CREATE TABLE sq_hard_w6_child (
  child_id  NUMBER CONSTRAINT sq_hard_w6_child_pk PRIMARY KEY,
  parent_id NUMBER,
  label     VARCHAR2(20),
  CONSTRAINT sq_hard_w6_child_fk FOREIGN KEY (parent_id)
    REFERENCES sq_hard_w6_ddl (ddl_id) ON DELETE SET NULL
    DEFERRABLE INITIALLY IMMEDIATE
) TABLESPACE users;

COMMENT ON TABLE sq_hard_w6_child IS 'wave 6 /* not a comment */ child rows';
COMMENT ON COLUMN sq_hard_w6_child.parent_id IS 'points at sq_hard_w6_ddl -- not a comment';

GRANT SELECT, INSERT ON sq_hard_w6_child TO PUBLIC;
REVOKE INSERT ON sq_hard_w6_child FROM PUBLIC;

INSERT ALL
  INTO sq_hard_w6_child (child_id, parent_id, label) VALUES (src_id, 1, 'left')
  INTO sq_hard_w6_child (child_id, parent_id, label) VALUES (src_id + 10, 2, 'right')
SELECT 1 AS src_id FROM dual;

DELETE FROM sq_hard_w6_ddl WHERE ddl_id = 1;

SELECT child_id, parent_id, label
FROM sq_hard_w6_child
ORDER BY child_id;

TRUNCATE TABLE sq_hard_w6_iot DROP STORAGE;

INSERT INTO sq_hard_w6_iot (iot_key, iot_name, iot_note) VALUES (3, 'c', 'refilled');

ANALYZE TABLE sq_hard_w6_iot COMPUTE STATISTICS FOR TABLE;

--------------------------------------------------------------------------------
-- W6-K: parameterized cursor with a defaulted parameter, a cursor %ROWTYPE
-- record, a temporary CLOB assembled through DBMS_LOB, and the serially
-- reusable package counter.
--------------------------------------------------------------------------------
DECLARE
  CURSOR node_cur(p_node NUMBER, p_floor NUMBER DEFAULT 0) IS
    SELECT metric_id, metric_name, metric_value
    FROM sq_hard_metric
    WHERE node_id = p_node AND metric_value >= p_floor
    ORDER BY metric_id;
  node_rec  node_cur%ROWTYPE;
  seen_rows PLS_INTEGER := 0;
  scratch   CLOB;
  clob_len  PLS_INTEGER;
  clob_head VARCHAR2(40);
  slash_at  PLS_INTEGER;
  tick_one  PLS_INTEGER;
  tick_two  PLS_INTEGER;
BEGIN
  OPEN node_cur(1, p_floor => 18);
  LOOP
    FETCH node_cur INTO node_rec;
    EXIT WHEN node_cur%NOTFOUND;
    seen_rows := seen_rows + node_cur%ROWCOUNT;
  END LOOP;
  CLOSE node_cur;

  DBMS_LOB.CREATETEMPORARY(scratch, TRUE);
  DBMS_LOB.WRITEAPPEND(scratch, 6, 'wave6/');
  DBMS_LOB.APPEND(scratch, TO_CLOB('tail'));
  clob_len  := DBMS_LOB.GETLENGTH(scratch);
  clob_head := DBMS_LOB.SUBSTR(scratch, 5, 1);
  slash_at  := DBMS_LOB.INSTR(scratch, '/', 1, 1);
  DBMS_LOB.FREETEMPORARY(scratch);

  tick_one := sq_hard_w6_reuse.tick;
  tick_two := sq_hard_w6_reuse.tick;

  INSERT INTO sq_hard_w6_note (note_key, note_value, note_text)
  VALUES ('cursor_lob', seen_rows + clob_len + slash_at + tick_one + tick_two,
          clob_head || ':' || node_rec.metric_name);
END;
/

--------------------------------------------------------------------------------
-- W6-L: pure lexer and layout torture. A comment sits between nearly every
-- token, CASE nests five deep inside COALESCE, the q-quote payload carries both
-- comment openers and its own delimiter family, and the last projection is one
-- very long unbroken line with no space around any operator.
--------------------------------------------------------------------------------
SELECT /*a*/ m.metric_id /*b*/ AS /*c*/ id_out /*d*/,
       /*e*/ COALESCE(
         CASE WHEN m.metric_value > 20 THEN
           CASE WHEN m.node_id = 1 THEN
             CASE WHEN m.metric_name LIKE 'L%' THEN
               CASE WHEN LENGTH(m.metric_name) = 7 THEN
                 CASE WHEN m.metric_id > 0 THEN 'deep-hit' ELSE 'deep-miss' END
               ELSE 'len-miss' END
             ELSE 'like-miss' END
           ELSE 'node-miss' END
         ELSE NULL END,
         'fallback') AS nested_case,
       q'[a ]] b /* still literal */ -- still literal]' AS bracket_payload,
       q'{outer {inner} /* literal */}' AS brace_payload,
       NVL2(m.payload, 'json', 'none') AS payload_shape
FROM /*f*/ sq_hard_metric /*g*/ m /*h*/
WHERE /*i*/ m.metric_id /*j*/ IN /*k*/ (1, 2, 3, 4)
ORDER BY /*l*/ m.metric_id /*m*/ DESC /*n*/ NULLS FIRST;

SELECT m.metric_id||'-'||m.node_id||'/'||TO_CHAR(m.metric_value,'FM9990D00')||CASE WHEN m.metric_value>=18 THEN'/hi'ELSE'/lo'END AS packed_line,(m.metric_value*2)-(m.node_id*3)+MOD(m.metric_id,2)AS packed_math FROM sq_hard_metric m WHERE m.node_id IN(1,2)AND m.metric_value BETWEEN 1 AND 999 ORDER BY m.metric_id;

sElEcT	CoUnT(*)	As	mixed_case_rows	FrOm	sq_hard_w6_note	WhErE	note_key	iS	NoT	nUlL;

--------------------------------------------------------------------------------
-- W6-M: wave-6 self-verification.
--------------------------------------------------------------------------------
DECLARE
  json_val    NUMBER;
  json_txt    VARCHAR2(400);
  xml_val     NUMBER;
  xml_txt     VARCHAR2(400);
  plsql_val   NUMBER;
  plsql_txt   VARCHAR2(400);
  lob_val     NUMBER;
  lob_txt     VARCHAR2(400);
  mv_buckets  PLS_INTEGER;
  mv_qty      NUMBER;
  iot_rows    PLS_INTEGER;
  child_rows  PLS_INTEGER;
  parented    PLS_INTEGER;
  ddl_rows    PLS_INTEGER;
  span_val    NUMBER;
  synonym_rows PLS_INTEGER;
  recycled    PLS_INTEGER;
  clustered   PLS_INTEGER;
BEGIN
  SELECT note_value, note_text INTO json_val, json_txt
  FROM sq_hard_w6_note WHERE note_key = 'json_api';
  SELECT note_value, note_text INTO xml_val, xml_txt
  FROM sq_hard_w6_note WHERE note_key = 'xml_anydata';
  SELECT note_value, note_text INTO plsql_val, plsql_txt
  FROM sq_hard_w6_note WHERE note_key = 'plsql_round2';
  SELECT note_value, note_text INTO lob_val, lob_txt
  FROM sq_hard_w6_note WHERE note_key = 'cursor_lob';

  SELECT COUNT(*), SUM(bucket_qty) INTO mv_buckets, mv_qty FROM sq_hard_w6_mv;
  SELECT COUNT(*) INTO iot_rows FROM sq_hard_w6_iot;
  SELECT COUNT(*), COUNT(parent_id) INTO child_rows, parented FROM sq_hard_w6_child;
  SELECT COUNT(*) INTO ddl_rows FROM sq_hard_w6_ddl;
  SELECT COUNT(*) INTO recycled FROM sq_hard_w6_recycle;
  SELECT COUNT(*) INTO clustered FROM sq_hard_w6_clustered;
  SELECT COUNT(*) INTO synonym_rows FROM sq_hard_w6_syn;
  span_val := sq_hard_w6_span(2);

  IF json_val <> 23
     OR json_txt <> 'object:6:has-spare:node|limits|tags|checked|spare|shape' THEN
    RAISE_APPLICATION_ERROR(-20090, 'json api ' || json_val || '/' || json_txt);
  END IF;
  IF xml_val <> 39 OR xml_txt <> 'metrics:SYS.NUMBER:0:18' THEN
    RAISE_APPLICATION_ERROR(-20091, 'xml/anydata ' || xml_val || '/' || xml_txt);
  END IF;
  IF plsql_val <> 20 OR plsql_txt <> 'at1/gone/3/3/3/8/4' THEN
    RAISE_APPLICATION_ERROR(-20092, 'plsql round2 ' || plsql_val || '/' || plsql_txt);
  END IF;
  IF lob_val <> 22 OR lob_txt <> 'wave6:LATENCY' THEN
    RAISE_APPLICATION_ERROR(-20093, 'cursor/lob ' || lob_val || '/' || lob_txt);
  END IF;
  IF mv_buckets <> 3 OR mv_qty <> 44 THEN
    RAISE_APPLICATION_ERROR(-20094, 'fast mv ' || mv_buckets || '/' || mv_qty);
  END IF;
  IF iot_rows <> 1 OR clustered <> 1 OR recycled <> 1 THEN
    RAISE_APPLICATION_ERROR(
      -20095,
      'storage ddl ' || iot_rows || '/' || clustered || '/' || recycled
    );
  END IF;
  IF child_rows <> 2 OR parented <> 1 OR ddl_rows <> 1 THEN
    RAISE_APPLICATION_ERROR(
      -20096,
      'referential ' || child_rows || '/' || parented || '/' || ddl_rows
    );
  END IF;
  IF span_val <> 60 OR synonym_rows <> 5 THEN
    RAISE_APPLICATION_ERROR(-20097, 'span/synonym ' || span_val || '/' || synonym_rows);
  END IF;
END;
/
--------------------------------------------------------------------------------
-- ULTRA WAVE 7: the legacy XML DML/generation family, an application context
-- driven by its own trusted package plus call-stack reflection, CREATE
-- DIMENSION and ASSOCIATE STATISTICS, in-database archiving with a hidden
-- ORA_ARCHIVE_STATE column, cursor-positioned and record-shaped DML,
-- collection algebra with the binary float/double literal suffixes, the
-- scalar and REGR_ statistical builtins, unified audit policy DDL, and a
-- second lexer torture round.
--------------------------------------------------------------------------------
CREATE TABLE sq_hard_w7_note (
  note_key   VARCHAR2(30) CONSTRAINT sq_hard_w7_note_pk PRIMARY KEY,
  note_value NUMBER,
  note_text  VARCHAR2(400)
) TABLESPACE users;

--------------------------------------------------------------------------------
-- W7-A: the legacy XML DML and generation family. UPDATEXML / INSERTCHILDXML /
-- DELETEXML take an XPath string in an argument slot that looks like a column,
-- XMLPI and XMLCOLATTVAL take a NAME/AS keyword inside the argument list, and
-- XMLSEQUENCE returns a collection that TABLE() then unnests.
--------------------------------------------------------------------------------
CREATE TABLE sq_hard_w7_xml (
  doc_id   NUMBER CONSTRAINT sq_hard_w7_xml_pk PRIMARY KEY,
  body_xml XMLTYPE
) TABLESPACE users;

INSERT INTO sq_hard_w7_xml (doc_id, body_xml)
VALUES (1, XMLTYPE('<doc><v unit="ms">12</v><v unit="ms">18</v><spare/></doc>'));

UPDATE sq_hard_w7_xml
   SET body_xml = UPDATEXML(body_xml, '/doc/v[1]/text()', '30')
 WHERE doc_id = 1;

UPDATE sq_hard_w7_xml
   SET body_xml = INSERTCHILDXML(body_xml, '/doc', 'v',
                                 XMLTYPE('<v unit="s">7</v>'))
 WHERE doc_id = 1;

UPDATE sq_hard_w7_xml
   SET body_xml = DELETEXML(body_xml, '/doc/spare')
 WHERE doc_id = 1;

COMMIT;

SELECT SUM(TO_NUMBER(seq_row.COLUMN_VALUE.extract('/v/text()').getStringVal()))
         AS xml_total,
       COUNT(*) AS xml_nodes
FROM sq_hard_w7_xml x,
     TABLE(XMLSEQUENCE(x.body_xml.extract('/doc/v'))) seq_row
WHERE x.doc_id = 1;

SELECT XMLSERIALIZE(CONTENT
         XMLELEMENT("wrap",
           XMLPI(NAME "sq-hard", 'version="7"'),
           XMLCOLATTVAL(x.doc_id AS "id"),
           XMLCDATA('a > b & c < d')
         )
       AS CLOB INDENT SIZE = 2) AS built_xml
FROM sq_hard_w7_xml x
WHERE x.doc_id = 1;

DECLARE
  gen_ctx  DBMS_XMLGEN.ctxHandle;
  gen_clob CLOB;
  gen_rows NUMBER;
BEGIN
  gen_ctx := DBMS_XMLGEN.newContext(
    'SELECT doc_id FROM sq_hard_w7_xml ORDER BY doc_id');
  DBMS_XMLGEN.setRowSetTag(gen_ctx, 'docs');
  DBMS_XMLGEN.setRowTag(gen_ctx, 'doc');
  gen_clob := DBMS_XMLGEN.getXML(gen_ctx);
  gen_rows := DBMS_XMLGEN.getNumRowsProcessed(gen_ctx);
  DBMS_XMLGEN.closeContext(gen_ctx);

  INSERT INTO sq_hard_w7_note (note_key, note_value, note_text)
  VALUES ('xmlgen', gen_rows,
          CASE WHEN gen_clob LIKE '%<docs>%' THEN 'rowset-tag' ELSE 'plain' END);
END;
/

--------------------------------------------------------------------------------
-- W7-B: an application context driven by its own trusted package, plus the
-- call-stack reflection API. UTL_CALL_STACK members take a numeric depth and
-- CONCATENATE_SUBPROGRAM takes the collection another member returns.
--------------------------------------------------------------------------------
CREATE OR REPLACE PACKAGE sq_hard_w7_ctx_pkg AUTHID CURRENT_USER AS
  PROCEDURE set_tenant(p_tenant IN VARCHAR2, p_tier IN VARCHAR2);
  FUNCTION where_am_i RETURN VARCHAR2;
  FUNCTION doubled(p_n IN NUMBER) RETURN NUMBER DETERMINISTIC;
  PRAGMA RESTRICT_REFERENCES(doubled, WNDS, RNPS, WNPS);
END sq_hard_w7_ctx_pkg;
/

CREATE OR REPLACE PACKAGE BODY sq_hard_w7_ctx_pkg AS
  PROCEDURE set_tenant(p_tenant IN VARCHAR2, p_tier IN VARCHAR2) IS
  BEGIN
    DBMS_SESSION.set_context('SQ_HARD_W7_CTX', 'tenant', p_tenant);
    DBMS_SESSION.set_context('SQ_HARD_W7_CTX', 'tier', p_tier);
  END set_tenant;

  FUNCTION where_am_i RETURN VARCHAR2 IS
  BEGIN
    RETURN UTL_CALL_STACK.concatenate_subprogram(
             UTL_CALL_STACK.subprogram(1));
  END where_am_i;

  FUNCTION doubled(p_n IN NUMBER) RETURN NUMBER DETERMINISTIC IS
  BEGIN
    RETURN p_n * 2;
  END doubled;
END sq_hard_w7_ctx_pkg;
/

CREATE OR REPLACE CONTEXT sq_hard_w7_ctx USING sq_hard_w7_ctx_pkg;

DECLARE
  tenant_now VARCHAR2(60);
  tier_now   VARCHAR2(60);
  who_now    VARCHAR2(200);
  depth_now  PLS_INTEGER;
  trace_ok   VARCHAR2(10);
BEGIN
  sq_hard_w7_ctx_pkg.set_tenant('wave7', 'gold');
  tenant_now := SYS_CONTEXT('SQ_HARD_W7_CTX', 'tenant');
  tier_now   := SYS_CONTEXT('SQ_HARD_W7_CTX', 'tier');
  who_now    := sq_hard_w7_ctx_pkg.where_am_i;
  depth_now  := UTL_CALL_STACK.dynamic_depth;

  DBMS_APPLICATION_INFO.set_module(module_name => 'sq_hard_w7',
                                   action_name => 'context');
  BEGIN
    RAISE_APPLICATION_ERROR(-20701, 'planted');
  EXCEPTION
    WHEN OTHERS THEN
      trace_ok := CASE
                    WHEN DBMS_UTILITY.format_error_backtrace IS NOT NULL
                    THEN 'traced'
                    ELSE 'bare'
                  END;
  END;

  INSERT INTO sq_hard_w7_note (note_key, note_value, note_text)
  VALUES ('context', CASE WHEN depth_now >= 1 THEN 1 ELSE 0 END,
          tenant_now || '/' || tier_now || '/' || who_now || '/' || trace_ok);
END;
/

SELECT note_key, note_value, note_text
FROM sq_hard_w7_note
WHERE note_key = 'context';

--------------------------------------------------------------------------------
-- W7-C: CREATE DIMENSION -- a DDL statement built almost entirely out of words
-- that are clause keywords everywhere else (LEVEL, HIERARCHY, CHILD OF,
-- JOIN KEY, REFERENCES, ATTRIBUTE, DETERMINES), plus ASSOCIATE STATISTICS.
--------------------------------------------------------------------------------
CREATE TABLE sq_hard_w7_time (
  day_id    DATE         NOT NULL,
  month_id  VARCHAR2(7)  NOT NULL,
  year_id   NUMBER       NOT NULL,
  day_name  VARCHAR2(12),
  CONSTRAINT sq_hard_w7_time_pk PRIMARY KEY (day_id)
) TABLESPACE users;

CREATE TABLE sq_hard_w7_cat (
  cat_id   VARCHAR2(10) NOT NULL,
  cat_name VARCHAR2(30),
  CONSTRAINT sq_hard_w7_cat_pk PRIMARY KEY (cat_id)
) TABLESPACE users;

CREATE TABLE sq_hard_w7_prod (
  prod_id   NUMBER       NOT NULL,
  cat_id    VARCHAR2(10) NOT NULL,
  prod_name VARCHAR2(30),
  CONSTRAINT sq_hard_w7_prod_pk PRIMARY KEY (prod_id),
  CONSTRAINT sq_hard_w7_prod_fk FOREIGN KEY (cat_id)
    REFERENCES sq_hard_w7_cat (cat_id)
) TABLESPACE users;

CREATE TABLE sq_hard_w7_sales (
  day_id  DATE   NOT NULL,
  prod_id NUMBER NOT NULL,
  amount  NUMBER(10, 2)
) TABLESPACE users;

INSERT INTO sq_hard_w7_time (day_id, month_id, year_id, day_name)
VALUES (DATE '2024-02-29', '2024-02', 2024, 'Thursday');
INSERT INTO sq_hard_w7_time (day_id, month_id, year_id, day_name)
VALUES (DATE '2024-03-01', '2024-03', 2024, 'Friday');

INSERT INTO sq_hard_w7_cat (cat_id, cat_name) VALUES ('CORE', 'Core parts');
INSERT INTO sq_hard_w7_cat (cat_id, cat_name) VALUES ('AUX', 'Auxiliary');

INSERT INTO sq_hard_w7_prod (prod_id, cat_id, prod_name)
VALUES (10, 'CORE', 'widget');
INSERT INTO sq_hard_w7_prod (prod_id, cat_id, prod_name)
VALUES (20, 'AUX', 'gasket');

INSERT INTO sq_hard_w7_sales (day_id, prod_id, amount)
VALUES (DATE '2024-02-29', 10, 40.50);
INSERT INTO sq_hard_w7_sales (day_id, prod_id, amount)
VALUES (DATE '2024-02-29', 20, 9.50);
INSERT INTO sq_hard_w7_sales (day_id, prod_id, amount)
VALUES (DATE '2024-03-01', 10, 50.00);
COMMIT;

CREATE DIMENSION sq_hard_w7_time_dim
  LEVEL day   IS sq_hard_w7_time.day_id
  LEVEL month IS sq_hard_w7_time.month_id
  LEVEL year  IS sq_hard_w7_time.year_id
  HIERARCHY cal_rollup (
    day   CHILD OF
    month CHILD OF
    year
  )
  ATTRIBUTE day DETERMINES (day_name);

CREATE DIMENSION sq_hard_w7_prod_dim
  LEVEL prod IS sq_hard_w7_prod.prod_id
  LEVEL cat  IS sq_hard_w7_cat.cat_id
  HIERARCHY prod_rollup (
    prod CHILD OF cat
    JOIN KEY sq_hard_w7_prod.cat_id REFERENCES cat
  )
  ATTRIBUTE prod DETERMINES (sq_hard_w7_prod.prod_name)
  ATTRIBUTE cat  DETERMINES (sq_hard_w7_cat.cat_name);

ASSOCIATE STATISTICS WITH PACKAGES sq_hard_w7_ctx_pkg
  DEFAULT SELECTIVITY 5 DEFAULT COST (100, 5, 0);

SELECT COUNT(*) AS dimension_count
FROM user_dimensions
WHERE dimension_name IN ('SQ_HARD_W7_TIME_DIM', 'SQ_HARD_W7_PROD_DIM');

DISASSOCIATE STATISTICS FROM PACKAGES sq_hard_w7_ctx_pkg;

--------------------------------------------------------------------------------
-- W7-D: in-database archiving. ROW ARCHIVAL adds a hidden ORA_ARCHIVE_STATE
-- column that only becomes visible through ALTER SESSION SET ROW ARCHIVAL
-- VISIBILITY, and the segment DDL tail stacks physical attributes.
--------------------------------------------------------------------------------
CREATE TABLE sq_hard_w7_arch (
  arch_id  NUMBER CONSTRAINT sq_hard_w7_arch_pk PRIMARY KEY,
  arch_tag VARCHAR2(20)
) PCTFREE 5 INITRANS 2 MAXTRANS 40 NOLOGGING ROWDEPENDENCIES
  TABLESPACE users;

ALTER TABLE sq_hard_w7_arch ROW ARCHIVAL;

INSERT INTO sq_hard_w7_arch (arch_id, arch_tag) VALUES (1, 'live');
INSERT INTO sq_hard_w7_arch (arch_id, arch_tag) VALUES (2, 'retired');
COMMIT;

UPDATE sq_hard_w7_arch
   SET ORA_ARCHIVE_STATE = DBMS_ILM.ARCHIVESTATENAME(1)
 WHERE arch_id = 2;
COMMIT;

SELECT COUNT(*) AS visible_rows FROM sq_hard_w7_arch;

ALTER SESSION SET ROW ARCHIVAL VISIBILITY = ALL;

SELECT COUNT(*)                                        AS all_rows,
       COUNT(NULLIF(ORA_ARCHIVE_STATE, '0'))           AS archived_rows
FROM sq_hard_w7_arch;

ALTER SESSION SET ROW ARCHIVAL VISIBILITY = ACTIVE;

ALTER TABLE sq_hard_w7_arch ENABLE ROW MOVEMENT;
ALTER TABLE sq_hard_w7_arch SHRINK SPACE COMPACT;
ALTER TABLE sq_hard_w7_arch LOGGING;

--------------------------------------------------------------------------------
-- W7-E: cursor-positioned and record-shaped DML. FOR UPDATE OF feeding
-- WHERE CURRENT OF, SET ROW = record, INSERT VALUES record, a multi-column
-- subquery assignment, and a MERGE whose matched branch both filters and
-- deletes.
--------------------------------------------------------------------------------
CREATE TABLE sq_hard_w7_pos (
  pos_id NUMBER CONSTRAINT sq_hard_w7_pos_pk PRIMARY KEY,
  qty    NUMBER,
  tag    VARCHAR2(20)
) TABLESPACE users;

CREATE TABLE sq_hard_w7_src (
  pos_id NUMBER,
  qty    NUMBER,
  tag    VARCHAR2(20)
) TABLESPACE users;

INSERT INTO sq_hard_w7_pos (pos_id, qty, tag) VALUES (1, 5, 'one');
INSERT INTO sq_hard_w7_pos (pos_id, qty, tag) VALUES (2, 7, 'two');
INSERT INTO sq_hard_w7_pos (pos_id, qty, tag) VALUES (3, 9, 'three');

INSERT INTO sq_hard_w7_src (pos_id, qty, tag) VALUES (2, 70, 'two-new');
INSERT INTO sq_hard_w7_src (pos_id, qty, tag) VALUES (3, -1, 'kill');
INSERT INTO sq_hard_w7_src (pos_id, qty, tag) VALUES (4, 11, 'four');
INSERT INTO sq_hard_w7_src (pos_id, qty, tag) VALUES (5, 999, 'skipped');
COMMIT;

DECLARE
  CURSOR pos_cur IS
    SELECT pos_id, qty
    FROM sq_hard_w7_pos
    WHERE qty < 8
    ORDER BY pos_id
    FOR UPDATE OF qty;
  bumped  PLS_INTEGER := 0;
  new_row sq_hard_w7_pos%ROWTYPE;
BEGIN
  FOR pos_rec IN pos_cur LOOP
    UPDATE sq_hard_w7_pos
       SET qty = pos_rec.qty * 2
     WHERE CURRENT OF pos_cur;
    bumped := bumped + 1;
  END LOOP;

  new_row.pos_id := 6;
  new_row.qty    := 60;
  new_row.tag    := 'six';
  INSERT INTO sq_hard_w7_pos VALUES new_row;

  new_row.qty := 61;
  new_row.tag := 'six-fixed';
  UPDATE sq_hard_w7_pos SET ROW = new_row WHERE pos_id = 6;

  INSERT INTO sq_hard_w7_note (note_key, note_value, note_text)
  VALUES ('cursor_dml', bumped, 'current-of/set-row');
END;
/

UPDATE sq_hard_w7_pos
   SET (qty, tag) = (SELECT s.qty, s.tag
                     FROM sq_hard_w7_src s
                     WHERE s.pos_id = sq_hard_w7_pos.pos_id)
 WHERE pos_id = 2;

MERGE INTO sq_hard_w7_pos t
USING (SELECT pos_id, qty, tag FROM sq_hard_w7_src) s
   ON (t.pos_id = s.pos_id)
 WHEN MATCHED THEN
   UPDATE SET t.tag = s.tag
   WHERE  s.qty > 0
   DELETE WHERE t.qty < 0
 WHEN NOT MATCHED THEN
   INSERT (pos_id, qty, tag)
   VALUES (s.pos_id, s.qty, s.tag)
   WHERE  s.qty <> 999;

COMMIT;

SELECT pos_id, qty, tag FROM sq_hard_w7_pos ORDER BY pos_id;

--------------------------------------------------------------------------------
-- W7-F: collection algebra round 2 and the numeric-literal suffixes.
-- POWERMULTISET_BY_CARDINALITY nests a collection inside a collection, SET()
-- and COLLECT(... ORDER BY) build them in SQL, and the PL/SQL side walks a
-- string-indexed associative array while labelled loops jump around it.
--------------------------------------------------------------------------------
CREATE OR REPLACE TYPE sq_hard_w7_num_tab AS TABLE OF NUMBER;
/
CREATE OR REPLACE TYPE sq_hard_w7_str_tab AS TABLE OF VARCHAR2(30);
/
CREATE OR REPLACE TYPE sq_hard_w7_num_bag AS TABLE OF sq_hard_w7_num_tab;
/

SELECT CARDINALITY(SET(sq_hard_w7_num_tab(1, 1, 2, 2, 3)))    AS deduped,
       (SELECT COUNT(*)
        FROM TABLE(POWERMULTISET_BY_CARDINALITY(
                     sq_hard_w7_num_tab(1, 2, 3), 2)))        AS pair_subsets,
       (SELECT COUNT(*) FROM TABLE(sys.odcivarchar2list('a', 'b', 'c')))
                                                              AS odci_rows
FROM dual;

-- Projected as a count, not as the collection itself: the two drivers render a
-- collection column differently (OCI spells out the type name, the thin path
-- renders decoded elements as JSON), so a raw collection projection is not
-- driver-neutral.
SELECT CARDINALITY(CAST(COLLECT(p.tag ORDER BY p.tag) AS sq_hard_w7_str_tab))
         AS tag_count
FROM sq_hard_w7_pos p
WHERE p.qty IS NOT NULL;

SELECT bag_row.COLUMN_VALUE AS ordered_tag,
       ROWNUM               AS tag_ord
FROM TABLE(CAST(MULTISET(SELECT p.tag
                         FROM sq_hard_w7_pos p
                         ORDER BY p.tag) AS sq_hard_w7_str_tab)) bag_row;

DECLARE
  TYPE tally_t IS TABLE OF PLS_INTEGER INDEX BY VARCHAR2(20);
  SUBTYPE small_count IS PLS_INTEGER RANGE 0 .. 99 NOT NULL;

  tally      tally_t;
  walk_key   VARCHAR2(20);
  seen       small_count := 0;
  skipped    small_count := 0;
  float_val  BINARY_FLOAT  := 1.5f;
  double_val BINARY_DOUBLE := 2.5d;
  fast_ctr   SIMPLE_INTEGER := 0;
  left_over  sq_hard_w7_num_tab;
  left_count PLS_INTEGER;
BEGIN
  tally('core')  := 3;
  tally('aux')   := 0;
  tally('spare') := 5;

  walk_key := tally.FIRST;
  <<walk_loop>>
  WHILE walk_key IS NOT NULL LOOP
    IF tally(walk_key) = 0 THEN
      skipped  := skipped + 1;
      walk_key := tally.NEXT(walk_key);
      CONTINUE walk_loop;
    END IF;

    seen     := seen + 1;
    fast_ctr := fast_ctr + tally(walk_key);
    walk_key := tally.NEXT(walk_key);

    EXIT walk_loop WHEN seen >= 9;
  END LOOP walk_loop;

  left_over := sq_hard_w7_num_tab(1, 2, 3, 4)
                 MULTISET EXCEPT DISTINCT sq_hard_w7_num_tab(2, 4);
  left_count := left_over.COUNT;

  IF float_val > 1 AND double_val > 2 THEN
    GOTO record_it;
  END IF;
  RAISE_APPLICATION_ERROR(-20702, 'literal suffix compare');

  <<record_it>>
  INSERT INTO sq_hard_w7_note (note_key, note_value, note_text)
  VALUES ('collections', fast_ctr + left_count,
          seen || '/' || skipped || '/' || left_count || '/'
          || TO_CHAR(float_val + double_val));
END;
/

--------------------------------------------------------------------------------
-- W7-G: the scalar and statistical builtin zoo. LNNVL only makes sense in a
-- WHERE clause, DUMP returns a formatted string, GROUP_ID needs a grouping
-- set, and the REGR_ family takes two arguments in a fixed order.
--------------------------------------------------------------------------------
SELECT COUNT(*) AS lnnvl_rows
FROM sq_hard_w7_pos p
WHERE LNNVL(p.qty > 1000);

SELECT DUMP(42, 10)                                AS dumped_number,
       VSIZE(42)                                   AS number_bytes,
       -- Wrapped in CASE on purpose: a bare BOOLEAN projection comes back as
       -- TRUE on the newer protocols and as 1 on 314, which has no native
       -- boolean, so the raw form is not protocol-neutral.
       CASE
         WHEN STANDARD_HASH('wave7', 'SHA256') IS NOT NULL THEN 'hashed'
         ELSE 'unhashed'
       END                                         AS hashed_ok,
       ORA_HASH('wave7', 4095)                     AS bucketed,
       NANVL(1, 0)                                 AS nan_guarded,
       REMAINDER(17, 5)                            AS remainder_val,
       BITAND(12, 10)                              AS bit_and,
       TZ_OFFSET('+05:30')                         AS fixed_offset,
       -- Both constructors return the server-default precision of 9. The thin
       -- driver must preserve that describe metadata and render the same width
       -- as OCI instead of collapsing the leading field to two digits.
       NUMTOYMINTERVAL(14, 'MONTH')                 AS ym_interval,
       TO_DSINTERVAL('P1DT2H')                     AS ds_interval
FROM dual;

SELECT NVL(s.prod_id, -1)                                  AS prod_id,
       GROUP_ID()                                          AS gid,
       GROUPING_ID(s.prod_id)                              AS grouping_id,
       ROUND(REGR_SLOPE(s.amount, s.prod_id), 4)           AS regr_slope,
       REGR_COUNT(s.amount, s.prod_id)                     AS regr_count,
       ROUND(REGR_R2(s.amount, s.prod_id), 4)              AS regr_r2,
       ROUND(REGR_AVGX(s.amount, s.prod_id), 4)            AS regr_avgx,
       ROUND(CORR(s.amount, s.prod_id), 4)                 AS corr_val,
       ROUND(COVAR_POP(s.amount, s.prod_id), 4)            AS covar_pop,
       STATS_MODE(s.prod_id)                               AS modal_prod
FROM sq_hard_w7_sales s
GROUP BY GROUPING SETS ((s.prod_id), ())
ORDER BY prod_id;

--------------------------------------------------------------------------------
-- W7-H: unified audit policy DDL -- ACTIONS / ON / WHEN read as clause
-- keywords, and AUDIT/NOAUDIT POLICY are statements that start with a word the
-- lexer also sees as an object type.
--------------------------------------------------------------------------------
CREATE AUDIT POLICY sq_hard_w7_pol
  ACTIONS SELECT ON system.sq_hard_w7_note,
          UPDATE ON system.sq_hard_w7_note;

AUDIT POLICY sq_hard_w7_pol;

SELECT COUNT(*) AS audit_policy_rows
FROM audit_unified_enabled_policies
WHERE policy_name = 'SQ_HARD_W7_POL';

NOAUDIT POLICY sq_hard_w7_pol;

--------------------------------------------------------------------------------
-- W7-I: lexer round 2. A not-equals spelled ^=, comment and terminator
-- lookalikes buried inside literals, an N-literal and a U-literal, keyword
-- aliases with no AS, and one line that never uses a space.
--------------------------------------------------------------------------------
SELECT p.pos_id                                           AS "select",
       'a -- not a comment /* nor this */; still text'     AS bait_text,
       q'#hash-quoted 'inner' quotes#'                     AS hash_quoted,
       N'nchar literal'                                    AS nchar_text,
       U'esc\00e9ape'                                      AS uchar_text,
       'doubled '' quote'                                  count,
       LENGTH('a' || CHR(10) || 'b')                       partition
FROM sq_hard_w7_pos p
WHERE p.pos_id ^= 999
  AND p.qty IS NOT NULL
ORDER BY p.pos_id
FETCH FIRST 2 ROWS ONLY;

SELECT(1+2)*3-4/2 AS crammed,MOD(7,3)modded,ABS(-8)absed FROM dual WHERE 1^=2;

SeLeCt CaSe WhEn 1=1 tHeN 'mixed' ElSe 'case' EnD AS alternating FROM DuAl;

--------------------------------------------------------------------------------
-- W7 self-verification.
--------------------------------------------------------------------------------
DECLARE
  note_rows  PLS_INTEGER;
  ctx_text   VARCHAR2(400);
  pos_rows   PLS_INTEGER;
  pos_two    VARCHAR2(20);
  arch_rows  PLS_INTEGER;
  xml_total  NUMBER;
  coll_value NUMBER;
BEGIN
  SELECT COUNT(*) INTO note_rows FROM sq_hard_w7_note;
  SELECT note_text INTO ctx_text
  FROM sq_hard_w7_note WHERE note_key = 'context';
  SELECT COUNT(*) INTO pos_rows FROM sq_hard_w7_pos;
  SELECT tag INTO pos_two FROM sq_hard_w7_pos WHERE pos_id = 2;
  SELECT COUNT(*) INTO arch_rows FROM sq_hard_w7_arch;
  SELECT SUM(TO_NUMBER(s.COLUMN_VALUE.extract('/v/text()').getStringVal()))
    INTO xml_total
  FROM sq_hard_w7_xml x,
       TABLE(XMLSEQUENCE(x.body_xml.extract('/doc/v'))) s
  WHERE x.doc_id = 1;
  SELECT note_value INTO coll_value
  FROM sq_hard_w7_note WHERE note_key = 'collections';

  IF note_rows <> 4 THEN
    RAISE_APPLICATION_ERROR(-20710, 'w7 note rows ' || note_rows);
  END IF;
  IF ctx_text NOT LIKE 'wave7/gold/%' THEN
    RAISE_APPLICATION_ERROR(-20711, 'w7 context ' || ctx_text);
  END IF;
  IF pos_rows <> 5 THEN
    RAISE_APPLICATION_ERROR(-20712, 'w7 pos rows ' || pos_rows);
  END IF;
  IF pos_two <> 'two-new' THEN
    RAISE_APPLICATION_ERROR(-20713, 'w7 pos tag ' || pos_two);
  END IF;
  IF arch_rows <> 1 THEN
    RAISE_APPLICATION_ERROR(-20714, 'w7 archival rows ' || arch_rows);
  END IF;
  IF xml_total <> 55 THEN
    RAISE_APPLICATION_ERROR(-20715, 'w7 xml total ' || xml_total);
  END IF;
  IF coll_value <> 10 THEN
    RAISE_APPLICATION_ERROR(-20716, 'w7 collections ' || coll_value);
  END IF;
END;
/

--------------------------------------------------------------------------------
-- Final self-verification and PASS banner.
--------------------------------------------------------------------------------
DECLARE
  quoted_sum   NUMBER;
  metric_total NUMBER;
  merge_rows   PLS_INTEGER;
  pipe_rows    PLS_INTEGER;
  view_nodes   PLS_INTEGER;
  audit_rows   PLS_INTEGER;
BEGIN
  SELECT "SELECT" + "order" INTO quoted_sum FROM sq_hard_quoted;
  metric_total := sq_hard_pkg.bulk_total;
  SELECT COUNT(*) INTO merge_rows FROM sq_hard_merge;
  SELECT COUNT(*) INTO pipe_rows FROM TABLE(sq_hard_pipe(4));
  SELECT COUNT(*) INTO view_nodes FROM sq_hard_v;
  SELECT COUNT(*) INTO audit_rows
  FROM sq_hard_metric_audit
  WHERE action_name = 'UPDATE' AND changed_rows = 3;

  IF quoted_sum <> 3 THEN
    RAISE_APPLICATION_ERROR(-20010, 'quoted-identifier sum');
  END IF;
  IF metric_total <> 56 THEN
    RAISE_APPLICATION_ERROR(-20011, 'metric bulk total ' || metric_total);
  END IF;
  IF merge_rows <> 5 THEN
    RAISE_APPLICATION_ERROR(-20012, 'merge row count ' || merge_rows);
  END IF;
  IF pipe_rows <> 4 THEN
    RAISE_APPLICATION_ERROR(-20013, 'pipelined row count ' || pipe_rows);
  END IF;
  IF view_nodes <> 2 THEN
    RAISE_APPLICATION_ERROR(-20014, 'view node count ' || view_nodes);
  END IF;
  IF audit_rows < 1 THEN
    RAISE_APPLICATION_ERROR(-20015, 'compound trigger audit');
  END IF;
END;
/

SELECT 'PASS' AS final_status,
       (SELECT COUNT(*) FROM sq_hard_metric)  AS metric_rows,
       (SELECT COUNT(*) FROM sq_hard_merge)   AS merge_rows,
       (SELECT COUNT(*) FROM sq_hard_w6_note) AS wave6_notes,
       (SELECT SUM(bucket_qty) FROM sq_hard_w6_mv) AS wave6_mv_qty,
       (SELECT COUNT(*) FROM sq_hard_w7_note) AS wave7_notes,
       (SELECT COUNT(*) FROM sq_hard_w7_pos)  AS wave7_pos
FROM dual;

PROMPT [ORACLE HARDCORE] PASS

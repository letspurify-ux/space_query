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
    SELECT 'DROP TABLE sq_hard_w26_note PURGE' text_value FROM dual UNION ALL
    SELECT 'DROP TABLE sq_hard_w26_event PURGE' FROM dual UNION ALL
    SELECT 'DROP TABLE sq_hard_w17_note PURGE' FROM dual UNION ALL
    SELECT 'DROP TABLE sq_hard_w17_edge PURGE' FROM dual UNION ALL
    SELECT 'DROP TABLE sq_hard_w16_note PURGE' FROM dual UNION ALL
    SELECT 'DROP TABLE sq_hard_w15_note PURGE' FROM dual UNION ALL
    SELECT 'DROP TABLE sq_hard_w15_event PURGE' FROM dual UNION ALL
    SELECT 'DROP TABLE sq_hard_w15_account PURGE' FROM dual UNION ALL
    SELECT 'DROP TABLE sq_hard_w15_region PURGE' FROM dual UNION ALL
    SELECT 'DROP TABLE sq_hard_w14_money PURGE' FROM dual UNION ALL
    SELECT 'DROP TABLE sq_hard_w14_note PURGE' FROM dual UNION ALL
    SELECT 'DROP DOMAIN sq_hard_w14_currency' FROM dual UNION ALL
    SELECT 'DROP TABLE sq_hard_w13_assign PURGE' FROM dual UNION ALL
    SELECT 'DROP TABLE sq_hard_w13_note PURGE' FROM dual UNION ALL
    SELECT 'DROP VIEW sq_hard_w12_definer_v' FROM dual UNION ALL
    SELECT 'DROP VIEW sq_hard_w12_bequeath_v' FROM dual UNION ALL
    SELECT 'DROP FUNCTION sq_hard_w12_shadow' FROM dual UNION ALL
    SELECT 'DROP PACKAGE sq_hard_w12_pkg' FROM dual UNION ALL
    SELECT 'DROP TABLE sq_hard_w12_json PURGE' FROM dual UNION ALL
    SELECT 'DROP TABLE sq_hard_w12_don PURGE' FROM dual UNION ALL
    SELECT 'DROP TABLE "sq_hard_w12_select" PURGE' FROM dual UNION ALL
    SELECT 'DROP TABLE sq_hard_w12_fn PURGE' FROM dual UNION ALL
    SELECT 'DROP TABLE sq_hard_w12_kw PURGE' FROM dual UNION ALL
    SELECT 'DROP TABLE sq_hard_w12_note PURGE' FROM dual UNION ALL
    SELECT 'DROP TABLE sq_hard_w11_stamp PURGE' FROM dual UNION ALL
    SELECT 'DROP SEQUENCE sq_hard_w11_seq' FROM dual UNION ALL
    SELECT 'DROP PACKAGE sq_hard_w11_util_pkg' FROM dual UNION ALL
    SELECT 'DROP VIEW sq_hard_w11_ledger_v' FROM dual UNION ALL
    SELECT 'DROP TABLE sq_hard_w11_ledger PURGE' FROM dual UNION ALL
    SELECT 'DROP TABLE sq_hard_w11_type PURGE' FROM dual UNION ALL
    SELECT 'DROP TABLE sq_hard_w11_note PURGE' FROM dual UNION ALL
    SELECT 'DROP VIEW sq_hard_w10_dyn' FROM dual UNION ALL
    SELECT 'DROP OPERATOR sq_hard_w10_weight FORCE' FROM dual UNION ALL
    SELECT 'DROP FUNCTION sq_hard_w10_weigh' FROM dual UNION ALL
    SELECT 'DROP PROCEDURE sq_hard_w10_raiser' FROM dual UNION ALL
    SELECT 'DROP TABLE sq_hard_w10_shape PURGE' FROM dual UNION ALL
    SELECT 'DROP TABLE sq_hard_w10_bulk PURGE' FROM dual UNION ALL
    SELECT 'DROP TABLE sq_hard_w10_lease PURGE' FROM dual UNION ALL
    SELECT 'DROP TABLE sq_hard_w10_doc PURGE' FROM dual UNION ALL
    SELECT 'DROP TABLE sq_hard_w10_route PURGE' FROM dual UNION ALL
    SELECT 'DROP TABLE sq_hard_w10_note PURGE' FROM dual UNION ALL
    SELECT 'DROP PUBLIC SYNONYM sq_hard_w9_pub' FROM dual UNION ALL
    SELECT 'ALTER USER c##sq_hard_w9_u REVOKE CONNECT THROUGH system' FROM dual UNION ALL
    SELECT 'DROP USER c##sq_hard_w9_u CASCADE' FROM dual UNION ALL
    SELECT 'DROP ROLE c##sq_hard_w9_r' FROM dual UNION ALL
    SELECT 'DROP PROFILE sq_hard_w9_prof CASCADE' FROM dual UNION ALL
    SELECT 'BEGIN DBMS_RLS.DROP_POLICY(''SYSTEM'', ''SQ_HARD_W9_SEC'', ''SQ_HARD_W9_ROWPOL''); END;' FROM dual UNION ALL
    SELECT 'BEGIN DBMS_RLS.DROP_POLICY(''SYSTEM'', ''SQ_HARD_W9_SEC'', ''SQ_HARD_W9_COLPOL''); END;' FROM dual UNION ALL
    SELECT 'DROP FUNCTION sq_hard_w9_region_pred' FROM dual UNION ALL
    SELECT 'DROP TABLE sq_hard_w9_sec PURGE' FROM dual UNION ALL
    SELECT 'DROP PACKAGE sq_hard_w9_qual_pkg' FROM dual UNION ALL
    SELECT 'DROP VIEW sq_hard_w9_ov' FROM dual UNION ALL
    SELECT 'DROP TABLE sq_hard_w9_flat PURGE' FROM dual UNION ALL
    SELECT 'DROP TABLE sq_hard_w9_edge PURGE' FROM dual UNION ALL
    SELECT 'DROP TABLE sq_hard_w9_node PURGE' FROM dual UNION ALL
    SELECT 'DROP TYPE sq_hard_w9_leaf_t FORCE' FROM dual UNION ALL
    SELECT 'DROP TYPE sq_hard_w9_node_t FORCE' FROM dual UNION ALL
    SELECT 'DROP TABLE sq_hard_w9_bucket PURGE' FROM dual UNION ALL
    SELECT 'DROP TABLE sq_hard_w9_note PURGE' FROM dual UNION ALL
    SELECT 'DROP TRIGGER sq_hard_w8_ddl_trg' FROM dual UNION ALL
    SELECT 'BEGIN DBMS_SCHEDULER.DROP_JOB(''sq_hard_w8_job'', TRUE); END;' FROM dual UNION ALL
    SELECT 'BEGIN DBMS_SCHEDULER.DROP_PROGRAM(''sq_hard_w8_prog'', TRUE); END;' FROM dual UNION ALL
    SELECT 'BEGIN DBMS_SCHEDULER.DROP_SCHEDULE(''sq_hard_w8_sched'', TRUE); END;' FROM dual UNION ALL
    SELECT 'BEGIN DBMS_AQADM.STOP_QUEUE(''sq_hard_w8_q''); END;' FROM dual UNION ALL
    SELECT 'BEGIN DBMS_AQADM.DROP_QUEUE(''sq_hard_w8_q''); END;' FROM dual UNION ALL
    SELECT 'BEGIN DBMS_AQADM.DROP_QUEUE_TABLE(''sq_hard_w8_qt'', TRUE); END;' FROM dual UNION ALL
    SELECT 'DROP FUNCTION sq_hard_w8_product' FROM dual UNION ALL
    SELECT 'DROP TYPE sq_hard_w8_prod_t FORCE' FROM dual UNION ALL
    SELECT 'DROP PACKAGE sq_hard_w8_pipe_pkg' FROM dual UNION ALL
    SELECT 'DROP FUNCTION sq_hard_w8_addup' FROM dual UNION ALL
    SELECT 'DROP FUNCTION sq_hard_w8_shout' FROM dual UNION ALL
    SELECT 'DROP FUNCTION sq_hard_w8_reverse' FROM dual UNION ALL
    SELECT 'DROP MLE MODULE sq_hard_w8_js' FROM dual UNION ALL
    SELECT 'DROP INDEX sq_hard_w8_doc_ix' FROM dual UNION ALL
    SELECT 'DROP TABLE sq_hard_w8_doc PURGE' FROM dual UNION ALL
    SELECT 'DROP TABLE sq_hard_w8_ref PURGE' FROM dual UNION ALL
    SELECT 'DROP TABLE sq_hard_w8_par PURGE' FROM dual UNION ALL
    SELECT 'DROP TABLE sq_hard_w8_auto PURGE' FROM dual UNION ALL
    SELECT 'DROP TABLE sq_hard_w8_hash PURGE' FROM dual UNION ALL
    SELECT 'DROP TABLE sq_hard_w8_flat PURGE' FROM dual UNION ALL
    SELECT 'DROP TABLE sq_hard_w8_clu PURGE' FROM dual UNION ALL
    SELECT 'DROP TABLE sq_hard_w8_fact PURGE' FROM dual UNION ALL
    SELECT 'DROP TABLE sq_hard_w8_dim PURGE' FROM dual UNION ALL
    SELECT 'DROP TABLE sq_hard_w8_ui PURGE' FROM dual UNION ALL
    SELECT 'DROP TABLE sq_hard_w8_jct PURGE' FROM dual UNION ALL
    SELECT 'DROP TABLE sq_hard_w8_geo PURGE' FROM dual UNION ALL
    SELECT 'DROP TABLE sq_hard_w8_ddl_probe PURGE' FROM dual UNION ALL
    SELECT 'DROP TABLE sq_hard_w8_ddl_log PURGE' FROM dual UNION ALL
    SELECT 'DROP TABLE sq_hard_w8_job_log PURGE' FROM dual UNION ALL
    SELECT 'DROP TABLE sq_hard_w8_note PURGE' FROM dual UNION ALL
    SELECT 'NOAUDIT POLICY sq_hard_w7_pol' FROM dual UNION ALL
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
FROM help AS OF TIMESTAMP (SYSTIMESTAMP - INTERVAL '0' SECOND);

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
-- ULTRA WAVE 8: embedded JavaScript through the Multilingual Engine (stored
-- modules, call specs, inline {{ }} bodies and dynamic DBMS_MLE contexts), a
-- user-defined aggregate built on the ODCIAggregate interface, partition
-- maintenance DDL (reference and automatic-list partitioning, SPLIT / MERGE /
-- RENAME / MOVE / TRUNCATE, online partitioning of a heap table, linear-order
-- clustering), the Oracle Text query sub-language, Advanced Queuing record
-- types, DBMS_SCHEDULER calendaring strings, a schema-level DDL event trigger,
-- nested cursor expressions feeding a cursor-argument pipelined function,
-- SDO_GEOMETRY constructor nesting, a JSON collection table, a bitmap join
-- index, SQL*Plus client-command torture and lexer round 3.
--------------------------------------------------------------------------------

-- Notes table carrying every value the wave-8 self-verification asserts.
CREATE TABLE sq_hard_w8_note (
  note_key   VARCHAR2(30) PRIMARY KEY,
  note_text  VARCHAR2(400),
  note_value NUMBER
);

--------------------------------------------------------------------------------
-- W8-A: Multilingual Engine. The module body is JavaScript, not SQL: export
-- declarations, arrow functions, template literals with ${} interpolation,
-- regular-expression literals, backtick strings and // comments all sit inside
-- a CREATE statement the SQL lexer must hand over wholesale. The call spec that
-- publishes a module function is a bodyless PL/SQL unit whose trailing
-- semicolon is mandatory, and the inline form embeds the body in {{ }}.
--------------------------------------------------------------------------------
CREATE OR REPLACE MLE MODULE sq_hard_w8_js LANGUAGE JAVASCRIPT AS
/**
 * Hostile payload: SQL keywords inside comments and strings, a semicolon-free
 * arrow body, and a template literal that carries both comment openers.
 */
export function addUp(a, b) {
  const parts = [a, b].map((n) => n * 1);   // SELECT * FROM dual; not SQL
  return parts.reduce((acc, n) => acc + n, 0);
}

export function shout(text) {
  const banner = `${text} -- /* still text */`;
  return banner === banner.toUpperCase() ? banner : banner.toUpperCase();
}

export function digits(text) {
  return (text.match(/[0-9]+/g) || []).join('|');
}
/

CREATE OR REPLACE FUNCTION sq_hard_w8_addup (a NUMBER, b NUMBER) RETURN NUMBER AS
  MLE MODULE sq_hard_w8_js
  SIGNATURE 'addUp(number, number)';
/

CREATE OR REPLACE FUNCTION sq_hard_w8_shout (text VARCHAR2) RETURN VARCHAR2 AS
  MLE MODULE sq_hard_w8_js SIGNATURE 'shout(string)';
/

CREATE OR REPLACE FUNCTION sq_hard_w8_reverse (word VARCHAR2) RETURN VARCHAR2
AS MLE LANGUAGE JAVASCRIPT
{{
  // WORD is the PL/SQL formal, upper-cased by the SQL name resolver.
  const chars = [...WORD];
  return chars.reverse().join('');
}};
/

SELECT sq_hard_w8_addup(20, 22)                      AS mle_sum,
       sq_hard_w8_shout('wave8')                     AS mle_shout,
       sq_hard_w8_reverse('stressed')                AS mle_reverse,
       (SELECT COUNT(*)
        FROM user_mle_modules
        WHERE module_name = 'SQ_HARD_W8_JS'
          AND language_name = 'JAVASCRIPT')          AS mle_modules
FROM dual;

DECLARE
  ctx     DBMS_MLE.context_handle_t := DBMS_MLE.create_context();
  js_src  CLOB := q'~
const bindings = require("mle-js-bindings");
const seed = bindings.importValue("seed");
let total = 0;
for (let i = 1; i <= seed; i++) { total += i * i; }
bindings.exportValue("squares", total);
~';
  squares NUMBER;
BEGIN
  DBMS_MLE.export_to_mle(ctx, 'seed', 5);
  DBMS_MLE.eval(ctx, 'JAVASCRIPT', js_src);
  DBMS_MLE.import_from_mle(ctx, 'squares', squares);
  DBMS_MLE.drop_context(ctx);
  INSERT INTO sq_hard_w8_note (note_key, note_text, note_value)
  VALUES ('mle', 'dynamic javascript context', squares);
END;
/

--------------------------------------------------------------------------------
-- W8-B: a user-defined aggregate. The object type implements the ODCIAggregate
-- interface, so STATIC / MEMBER methods take SELF IN OUT and return
-- ODCIConst.Success, and the publishing function is another bodyless unit whose
-- AGGREGATE USING tail replaces a body.
--------------------------------------------------------------------------------
CREATE OR REPLACE TYPE sq_hard_w8_prod_t AS OBJECT (
  running NUMBER,
  STATIC FUNCTION ODCIAggregateInitialize (sctx IN OUT sq_hard_w8_prod_t)
    RETURN NUMBER,
  MEMBER FUNCTION ODCIAggregateIterate (self IN OUT sq_hard_w8_prod_t,
                                        value IN NUMBER) RETURN NUMBER,
  MEMBER FUNCTION ODCIAggregateTerminate (self  IN  sq_hard_w8_prod_t,
                                          ret   OUT NUMBER,
                                          flags IN  NUMBER) RETURN NUMBER,
  MEMBER FUNCTION ODCIAggregateMerge (self IN OUT sq_hard_w8_prod_t,
                                      ctx2 IN     sq_hard_w8_prod_t)
    RETURN NUMBER
);
/

CREATE OR REPLACE TYPE BODY sq_hard_w8_prod_t AS
  STATIC FUNCTION ODCIAggregateInitialize (sctx IN OUT sq_hard_w8_prod_t)
    RETURN NUMBER IS
  BEGIN
    sctx := sq_hard_w8_prod_t(1);
    RETURN ODCIConst.Success;
  END ODCIAggregateInitialize;

  MEMBER FUNCTION ODCIAggregateIterate (self IN OUT sq_hard_w8_prod_t,
                                        value IN NUMBER) RETURN NUMBER IS
  BEGIN
    self.running := self.running * NVL(value, 1);
    RETURN ODCIConst.Success;
  END ODCIAggregateIterate;

  MEMBER FUNCTION ODCIAggregateTerminate (self  IN  sq_hard_w8_prod_t,
                                          ret   OUT NUMBER,
                                          flags IN  NUMBER) RETURN NUMBER IS
  BEGIN
    ret := self.running;
    RETURN ODCIConst.Success;
  END ODCIAggregateTerminate;

  MEMBER FUNCTION ODCIAggregateMerge (self IN OUT sq_hard_w8_prod_t,
                                      ctx2 IN     sq_hard_w8_prod_t)
    RETURN NUMBER IS
  BEGIN
    self.running := self.running * ctx2.running;
    RETURN ODCIConst.Success;
  END ODCIAggregateMerge;
END;
/

CREATE OR REPLACE FUNCTION sq_hard_w8_product (input NUMBER) RETURN NUMBER
  PARALLEL_ENABLE AGGREGATE USING sq_hard_w8_prod_t;
/

CREATE TABLE sq_hard_w8_dim (
  dim_id   NUMBER CONSTRAINT sq_hard_w8_dim_pk PRIMARY KEY,
  dim_name VARCHAR2(20) NOT NULL
);

CREATE TABLE sq_hard_w8_fact (
  fact_id NUMBER,
  dim_id  NUMBER,
  factor  NUMBER
);

INSERT INTO sq_hard_w8_dim (dim_id, dim_name) VALUES (1, 'alpha');
INSERT INTO sq_hard_w8_dim (dim_id, dim_name) VALUES (2, 'beta');

INSERT INTO sq_hard_w8_fact (fact_id, dim_id, factor) VALUES (1, 1, 2);
INSERT INTO sq_hard_w8_fact (fact_id, dim_id, factor) VALUES (2, 1, 3);
INSERT INTO sq_hard_w8_fact (fact_id, dim_id, factor) VALUES (3, 1, 3);
INSERT INTO sq_hard_w8_fact (fact_id, dim_id, factor) VALUES (4, 2, 5);
INSERT INTO sq_hard_w8_fact (fact_id, dim_id, factor) VALUES (5, 2, 7);

SELECT d.dim_name,
       sq_hard_w8_product(f.factor)          AS product_all,
       sq_hard_w8_product(DISTINCT f.factor) AS product_distinct,
       COUNT(*)                              AS factor_rows
FROM sq_hard_w8_fact f
     JOIN sq_hard_w8_dim d ON d.dim_id = f.dim_id
GROUP BY d.dim_name
HAVING sq_hard_w8_product(f.factor) > 1
ORDER BY d.dim_name;

--------------------------------------------------------------------------------
-- W8-C: partition maintenance. A reference-partitioned child inherits its
-- parent's key, AUTOMATIC list partitioning grows a partition per unseen value,
-- and SPLIT / MERGE / RENAME / MOVE / TRUNCATE stack partition-only vocabulary
-- (UPDATE INDEXES, DROP STORAGE) after a table name. ALTER TABLE ... MODIFY
-- PARTITION BY re-shapes a heap table ONLINE, and CLUSTERING BY LINEAR ORDER
-- carries a YES/NO option tail no other statement uses.
--------------------------------------------------------------------------------
CREATE TABLE sq_hard_w8_par (
  order_id  NUMBER CONSTRAINT sq_hard_w8_par_pk PRIMARY KEY,
  placed_on DATE NOT NULL,
  region    VARCHAR2(10)
)
PARTITION BY RANGE (placed_on)
(PARTITION p_jan VALUES LESS THAN (DATE '2024-02-01'),
 PARTITION p_feb VALUES LESS THAN (DATE '2024-03-01'),
 PARTITION p_max VALUES LESS THAN (MAXVALUE));

CREATE TABLE sq_hard_w8_ref (
  line_id  NUMBER CONSTRAINT sq_hard_w8_ref_pk PRIMARY KEY,
  order_id NUMBER NOT NULL,
  qty      NUMBER,
  CONSTRAINT sq_hard_w8_ref_fk FOREIGN KEY (order_id)
    REFERENCES sq_hard_w8_par (order_id) ON DELETE CASCADE
)
PARTITION BY REFERENCE (sq_hard_w8_ref_fk);

INSERT INTO sq_hard_w8_par (order_id, placed_on, region)
VALUES (1, DATE '2024-01-15', 'KR');
INSERT INTO sq_hard_w8_par (order_id, placed_on, region)
VALUES (2, DATE '2024-02-15', 'JP');
INSERT INTO sq_hard_w8_par (order_id, placed_on, region)
VALUES (3, DATE '2024-03-15', 'KR');
INSERT INTO sq_hard_w8_ref (line_id, order_id, qty) VALUES (10, 1, 4);
INSERT INTO sq_hard_w8_ref (line_id, order_id, qty) VALUES (11, 2, 6);
INSERT INTO sq_hard_w8_ref (line_id, order_id, qty) VALUES (12, 3, 9);

ALTER TABLE sq_hard_w8_par SPLIT PARTITION p_max
  INTO (PARTITION p_mar VALUES LESS THAN (DATE '2024-04-01'), PARTITION p_max)
  UPDATE INDEXES;

ALTER TABLE sq_hard_w8_par MERGE PARTITIONS p_jan, p_feb
  INTO PARTITION p_q1_head UPDATE INDEXES;

ALTER TABLE sq_hard_w8_par RENAME PARTITION p_q1_head TO p_head;

ALTER TABLE sq_hard_w8_par MOVE PARTITION p_head;

SELECT p.order_id, p.region, r.qty
FROM sq_hard_w8_par PARTITION (p_head) p
     JOIN sq_hard_w8_ref r ON r.order_id = p.order_id
ORDER BY p.order_id;

SELECT (SELECT COUNT(*)
        FROM user_tab_partitions
        WHERE table_name = 'SQ_HARD_W8_PAR')           AS parent_partitions,
       (SELECT COUNT(*)
        FROM user_tab_partitions
        WHERE table_name = 'SQ_HARD_W8_REF')           AS child_partitions,
       (SELECT COUNT(*)
        FROM user_part_tables
        WHERE table_name = 'SQ_HARD_W8_REF'
          AND partitioning_type = 'REFERENCE')         AS reference_tables
FROM dual;

CREATE TABLE sq_hard_w8_auto (
  hit_id NUMBER,
  region VARCHAR2(10)
)
PARTITION BY LIST (region) AUTOMATIC
(PARTITION p_kr VALUES ('KR'));

INSERT INTO sq_hard_w8_auto (hit_id, region) VALUES (1, 'KR');
INSERT INTO sq_hard_w8_auto (hit_id, region) VALUES (2, 'SG');
INSERT INTO sq_hard_w8_auto (hit_id, region) VALUES (3, 'SG');

CREATE TABLE sq_hard_w8_hash (
  bucket_id NUMBER,
  payload   VARCHAR2(10)
)
PARTITION BY HASH (bucket_id) PARTITIONS 4 STORE IN (users);

INSERT INTO sq_hard_w8_hash (bucket_id, payload)
SELECT LEVEL, 'row' || LEVEL FROM dual CONNECT BY LEVEL <= 8;

CREATE TABLE sq_hard_w8_flat (
  flat_id   NUMBER,
  sensed_on DATE,
  reading   NUMBER
);

INSERT INTO sq_hard_w8_flat (flat_id, sensed_on, reading)
VALUES (1, DATE '2024-01-10', 5);
INSERT INTO sq_hard_w8_flat (flat_id, sensed_on, reading)
VALUES (2, DATE '2024-05-10', 9);

ALTER TABLE sq_hard_w8_flat MODIFY
  PARTITION BY RANGE (sensed_on) INTERVAL (NUMTOYMINTERVAL(1, 'MONTH'))
  (PARTITION p_first VALUES LESS THAN (DATE '2024-02-01')) ONLINE;

CREATE TABLE sq_hard_w8_clu (
  cluster_id NUMBER,
  bucket     VARCHAR2(10),
  amount     NUMBER
)
CLUSTERING BY LINEAR ORDER (bucket, cluster_id)
  YES ON LOAD YES ON DATA MOVEMENT
  WITHOUT MATERIALIZED ZONEMAP;

INSERT INTO sq_hard_w8_clu (cluster_id, bucket, amount) VALUES (1, 'b', 10);
INSERT INTO sq_hard_w8_clu (cluster_id, bucket, amount) VALUES (2, 'a', 20);

-- CASCADE is mandatory here: the reference-partitioned child hangs off this
-- key, so truncating the parent partition must reach the matching child one.
ALTER TABLE sq_hard_w8_par TRUNCATE PARTITION p_mar
  DROP STORAGE CASCADE UPDATE INDEXES;

--------------------------------------------------------------------------------
-- W8-D: the Oracle Text query sub-language. CONTAINS takes an entire second
-- grammar inside a string literal - &, |, ~, -, the NEAR term list, the $ stem
-- and ? fuzzy prefixes, a % wildcard and a {escaped} phrase - and SCORE(label)
-- ties back to the numeric label given to CONTAINS. ALTER INDEX ... REBUILD
-- PARAMETERS carries a third sub-language.
--------------------------------------------------------------------------------
CREATE TABLE sq_hard_w8_doc (
  doc_id NUMBER CONSTRAINT sq_hard_w8_doc_pk PRIMARY KEY,
  body   VARCHAR2(400)
);

INSERT INTO sq_hard_w8_doc (doc_id, body)
VALUES (1, 'the quick brown dog jumped over the lazy cat');
INSERT INTO sq_hard_w8_doc (doc_id, body)
VALUES (2, 'a slow bird watched the lazy cat sleep');
INSERT INTO sq_hard_w8_doc (doc_id, body)
VALUES (3, 'jumping dogs and sleeping cats share one lazy afternoon');

CREATE INDEX sq_hard_w8_doc_ix ON sq_hard_w8_doc (body)
  INDEXTYPE IS CTXSYS.CONTEXT;

SELECT d.doc_id,
       CASE WHEN SCORE(7) > 0 THEN 'scored' ELSE 'zero' END AS relevance
FROM sq_hard_w8_doc d
WHERE CONTAINS(d.body, 'NEAR((dog, cat), 8) & lazy ~ quick - bird', 7) > 0
ORDER BY SCORE(7) DESC, d.doc_id;

SELECT COUNT(*) AS stem_hits
FROM sq_hard_w8_doc
WHERE CONTAINS(body, '$jump | ?slow | sleep% | {lazy cat}', 11) > 0;

ALTER INDEX sq_hard_w8_doc_ix REBUILD
  PARAMETERS ('replace stoplist ctxsys.empty_stoplist');

SELECT COUNT(*) AS after_rebuild
FROM sq_hard_w8_doc
WHERE CONTAINS(body, 'the & lazy', 3) > 0;

--------------------------------------------------------------------------------
-- W8-E: Advanced Queuing. The admin calls are named-notation only, the payload
-- is a SYS object type constructed through a static member, and the enqueue and
-- dequeue option records are PL/SQL record types whose fields are assigned one
-- at a time next to package constants that read like literals.
--------------------------------------------------------------------------------
BEGIN
  DBMS_AQADM.CREATE_QUEUE_TABLE(queue_table        => 'sq_hard_w8_qt',
                                queue_payload_type => 'SYS.AQ$_JMS_TEXT_MESSAGE',
                                multiple_consumers => FALSE,
                                sort_list          => 'PRIORITY,ENQ_TIME',
                                comment            => 'wave8 -- queue table');
  DBMS_AQADM.CREATE_QUEUE(queue_name  => 'sq_hard_w8_q',
                          queue_table => 'sq_hard_w8_qt',
                          max_retries => 3,
                          retry_delay => 0);
  DBMS_AQADM.ALTER_QUEUE(queue_name => 'sq_hard_w8_q', max_retries => 5);
  DBMS_AQADM.START_QUEUE(queue_name => 'sq_hard_w8_q');
END;
/

DECLARE
  enqueue_opts DBMS_AQ.enqueue_options_t;
  dequeue_opts DBMS_AQ.dequeue_options_t;
  msg_props    DBMS_AQ.message_properties_t;
  payload      SYS.AQ$_JMS_TEXT_MESSAGE;
  message_id   RAW(16);
  body_text    VARCHAR2(200);
BEGIN
  payload := SYS.AQ$_JMS_TEXT_MESSAGE.construct();
  payload.set_text('wave8 -- queued /* payload */');
  msg_props.priority    := 3;
  msg_props.correlation := 'sq_hard_w8';
  msg_props.expiration  := DBMS_AQ.NEVER;
  enqueue_opts.visibility := DBMS_AQ.ON_COMMIT;

  DBMS_AQ.ENQUEUE(queue_name         => 'sq_hard_w8_q',
                  enqueue_options    => enqueue_opts,
                  message_properties => msg_props,
                  payload            => payload,
                  msgid              => message_id);
  COMMIT;

  dequeue_opts.wait       := DBMS_AQ.NO_WAIT;
  dequeue_opts.navigation := DBMS_AQ.FIRST_MESSAGE;
  dequeue_opts.dequeue_mode := DBMS_AQ.REMOVE;
  dequeue_opts.correlation := 'sq_hard_w8';

  DBMS_AQ.DEQUEUE(queue_name         => 'sq_hard_w8_q',
                  dequeue_options    => dequeue_opts,
                  message_properties => msg_props,
                  payload            => payload,
                  msgid              => message_id);
  payload.get_text(body_text);
  COMMIT;

  INSERT INTO sq_hard_w8_note (note_key, note_text, note_value)
  VALUES ('queue', body_text, msg_props.priority);
END;
/

--------------------------------------------------------------------------------
-- W8-F: DBMS_SCHEDULER. The calendaring expression is a semicolon-separated
-- sub-language inside a string, the job attribute takes an INTERVAL literal as
-- an argument value, and RUN_JOB with use_current_session runs the program body
-- synchronously so the row it writes can be asserted in the same script.
--------------------------------------------------------------------------------
CREATE TABLE sq_hard_w8_job_log (
  log_id   NUMBER GENERATED ALWAYS AS IDENTITY,
  log_text VARCHAR2(100)
);

BEGIN
  DBMS_SCHEDULER.CREATE_SCHEDULE(
    schedule_name   => 'sq_hard_w8_sched',
    start_date      => TIMESTAMP '2024-01-01 02:30:00 +00:00',
    repeat_interval => 'FREQ=WEEKLY;BYDAY=MON,WED;BYHOUR=2;BYMINUTE=30;BYSECOND=0',
    comments        => 'wave8 calendaring string');

  DBMS_SCHEDULER.CREATE_PROGRAM(
    program_name        => 'sq_hard_w8_prog',
    program_type        => 'PLSQL_BLOCK',
    program_action      => 'BEGIN
                              INSERT INTO sq_hard_w8_job_log (log_text)
                              VALUES (''ran from scheduler'');
                              COMMIT;
                            END;',
    number_of_arguments => 0,
    enabled             => TRUE,
    comments            => 'wave8 program');

  DBMS_SCHEDULER.CREATE_JOB(job_name      => 'sq_hard_w8_job',
                            program_name  => 'sq_hard_w8_prog',
                            schedule_name => 'sq_hard_w8_sched',
                            enabled       => FALSE,
                            auto_drop     => FALSE,
                            comments      => 'wave8 job');

  DBMS_SCHEDULER.SET_ATTRIBUTE(name      => 'sq_hard_w8_job',
                               attribute => 'max_run_duration',
                               value     => INTERVAL '5' MINUTE);

  DBMS_SCHEDULER.RUN_JOB(job_name            => 'sq_hard_w8_job',
                         use_current_session => TRUE);
END;
/

SELECT (SELECT COUNT(*) FROM sq_hard_w8_job_log)          AS job_rows,
       (SELECT COUNT(*)
        FROM user_scheduler_schedules
        WHERE schedule_name = 'SQ_HARD_W8_SCHED')         AS schedules,
       (SELECT repeat_interval
        FROM user_scheduler_schedules
        WHERE schedule_name = 'SQ_HARD_W8_SCHED')         AS calendaring
FROM dual;

--------------------------------------------------------------------------------
-- W8-G: a schema-level DDL event trigger. The trigger header names an event
-- list instead of a table, its WHEN clause calls event attribute functions that
-- look like ordinary columns, and the body reads more of them. Dropped again as
-- soon as it has fired so it cannot observe the rest of the script.
--------------------------------------------------------------------------------
CREATE TABLE sq_hard_w8_ddl_log (
  event_seq  NUMBER GENERATED ALWAYS AS IDENTITY,
  event_text VARCHAR2(200)
);

CREATE OR REPLACE TRIGGER sq_hard_w8_ddl_trg
  AFTER CREATE OR DROP OR TRUNCATE ON SCHEMA
  WHEN (ora_dict_obj_name LIKE 'SQ_HARD_W8_DDL_PROBE%')
BEGIN
  INSERT INTO sq_hard_w8_ddl_log (event_text)
  VALUES (ora_sysevent || '/' || ora_dict_obj_type || '/'
          || ora_dict_obj_name || '/' || ora_dict_obj_owner);
END sq_hard_w8_ddl_trg;
/

CREATE TABLE sq_hard_w8_ddl_probe (probe_id NUMBER);

TRUNCATE TABLE sq_hard_w8_ddl_probe;

DROP TABLE sq_hard_w8_ddl_probe PURGE;

DROP TRIGGER sq_hard_w8_ddl_trg;

SELECT event_text
FROM sq_hard_w8_ddl_log
ORDER BY event_seq;

--------------------------------------------------------------------------------
-- W8-H: nested cursor expressions. CURSOR (SELECT ...) in a select list is a
-- whole query in an expression slot, and the same shape is the argument of a
-- pipelined function whose PARALLEL_ENABLE clause partitions the cursor itself.
--------------------------------------------------------------------------------
CREATE OR REPLACE PACKAGE sq_hard_w8_pipe_pkg AS
  TYPE dim_row IS RECORD (dim_id NUMBER);
  TYPE dim_cursor IS REF CURSOR RETURN dim_row;
  TYPE widened_row IS RECORD (dim_id NUMBER, doubled NUMBER);
  TYPE widened_tab IS TABLE OF widened_row;

  -- A cursor argument can only be partitioned when it is strongly typed, and
  -- the partition key names a column of the cursor's own row type.
  FUNCTION widen (src dim_cursor) RETURN widened_tab PIPELINED
    CLUSTER src BY (dim_id)
    PARALLEL_ENABLE (PARTITION src BY HASH (dim_id));
END sq_hard_w8_pipe_pkg;
/

CREATE OR REPLACE PACKAGE BODY sq_hard_w8_pipe_pkg AS
  FUNCTION widen (src dim_cursor) RETURN widened_tab PIPELINED
    CLUSTER src BY (dim_id)
    PARALLEL_ENABLE (PARTITION src BY HASH (dim_id)) IS
    out_row widened_row;
    dim_key NUMBER;
  BEGIN
    LOOP
      FETCH src INTO dim_key;
      EXIT WHEN src%NOTFOUND;
      out_row.dim_id  := dim_key;
      out_row.doubled := dim_key * 2;
      PIPE ROW (out_row);
    END LOOP;
    CLOSE src;
    RETURN;
  END widen;
END sq_hard_w8_pipe_pkg;
/

DECLARE
  outer_cur   SYS_REFCURSOR;
  nested_cur  SYS_REFCURSOR;
  dim_key     NUMBER;
  dim_label   VARCHAR2(20);
  fact_factor NUMBER;
  fact_seen   PLS_INTEGER := 0;
  factor_sum  NUMBER := 0;
BEGIN
  OPEN outer_cur FOR
    SELECT d.dim_id,
           d.dim_name,
           CURSOR (SELECT f.factor
                   FROM sq_hard_w8_fact f
                   WHERE f.dim_id = d.dim_id
                   ORDER BY f.fact_id) AS factors
    FROM sq_hard_w8_dim d
    ORDER BY d.dim_id;

  LOOP
    FETCH outer_cur INTO dim_key, dim_label, nested_cur;
    EXIT WHEN outer_cur%NOTFOUND;
    LOOP
      FETCH nested_cur INTO fact_factor;
      EXIT WHEN nested_cur%NOTFOUND;
      fact_seen  := fact_seen + 1;
      factor_sum := factor_sum + fact_factor;
    END LOOP;
  END LOOP;
  CLOSE outer_cur;

  INSERT INTO sq_hard_w8_note (note_key, note_text, note_value)
  VALUES ('cursors', 'nested cursor rows ' || fact_seen, factor_sum);
END;
/

SELECT w.dim_id, w.doubled
FROM TABLE(sq_hard_w8_pipe_pkg.widen(CURSOR (SELECT dim_id
                                             FROM sq_hard_w8_dim
                                             ORDER BY dim_id))) w
ORDER BY w.dim_id;

--------------------------------------------------------------------------------
-- W8-I: SDO_GEOMETRY. The constructor nests two more collection constructors,
-- the element-info array is a flat triplet list that means nothing to the
-- parser, and the attribute path reaches through an object column into a
-- nested object attribute.
--------------------------------------------------------------------------------
CREATE TABLE sq_hard_w8_geo (
  geo_id NUMBER CONSTRAINT sq_hard_w8_geo_pk PRIMARY KEY,
  shape  SDO_GEOMETRY
);

INSERT INTO sq_hard_w8_geo (geo_id, shape)
VALUES (1, SDO_GEOMETRY(2001, NULL, SDO_POINT_TYPE(10, 10, NULL), NULL, NULL));

INSERT INTO sq_hard_w8_geo (geo_id, shape)
VALUES (2, SDO_GEOMETRY(2003, NULL, NULL,
                        SDO_ELEM_INFO_ARRAY(1, 1003, 3),
                        SDO_ORDINATE_ARRAY(0, 0, 4, 4)));

SELECT g.geo_id,
       g.shape.sdo_gtype                            AS gtype,
       g.shape.sdo_point.x                          AS point_x,
       SDO_UTIL.TO_WKTGEOMETRY(g.shape)             AS wkt_text
FROM sq_hard_w8_geo g
WHERE g.geo_id = 1;

SELECT ROUND(SDO_GEOM.SDO_DISTANCE(a.shape, b.shape, 0.005), 3) AS gap
FROM sq_hard_w8_geo a,
     sq_hard_w8_geo b
WHERE a.geo_id = 2
  AND b.geo_id = 1;

--------------------------------------------------------------------------------
-- W8-J: a JSON collection table has no column list at all, a bitmap join index
-- carries its own FROM and WHERE clauses inside CREATE INDEX, an inline
-- USING INDEX (CREATE INDEX ...) nests one DDL statement inside another, and
-- MINUS ALL keeps duplicate rows a plain MINUS would collapse.
--------------------------------------------------------------------------------
CREATE JSON COLLECTION TABLE sq_hard_w8_jct TABLESPACE users;

INSERT INTO sq_hard_w8_jct
VALUES ('{"_id": 1, "tags": ["sql", "json"], "reading": 7}');
INSERT INTO sq_hard_w8_jct
VALUES ('{"_id": 2, "tags": ["mle"], "reading": 11}');

SELECT t.data."_id".number()      AS doc_id,
       t.data.reading.number()    AS reading,
       t.data.tags.size()         AS tag_count
FROM sq_hard_w8_jct t
ORDER BY t.data."_id".number();

SELECT j.tag
FROM sq_hard_w8_jct t
     NESTED data.tags[*] COLUMNS (tag PATH '$') j
ORDER BY j.tag;

CREATE BITMAP INDEX sq_hard_w8_bji ON sq_hard_w8_fact (d.dim_name)
  FROM sq_hard_w8_fact f, sq_hard_w8_dim d
  WHERE f.dim_id = d.dim_id;

ALTER INDEX sq_hard_w8_bji MONITORING USAGE;

CREATE TABLE sq_hard_w8_ui (
  ui_id  NUMBER,
  label  VARCHAR2(10),
  CONSTRAINT sq_hard_w8_ui_pk PRIMARY KEY (ui_id)
    USING INDEX (CREATE UNIQUE INDEX sq_hard_w8_ui_ix ON sq_hard_w8_ui (ui_id))
);

INSERT INTO sq_hard_w8_ui (ui_id, label) VALUES (1, 'one');

SELECT factor FROM sq_hard_w8_fact WHERE dim_id = 1
MINUS ALL
SELECT 2 FROM dual
ORDER BY 1;

--------------------------------------------------------------------------------
-- W8-K: SQL*Plus client-command torture. Bind variables are declared, filled
-- from an anonymous block, printed, then referenced from SQL; a substitution
-- variable is defined, expanded while DEFINE is on and undefined again; and
-- BREAK / COMPUTE add a report footer to a plain query.
--------------------------------------------------------------------------------
VARIABLE w8_bind NUMBER
VARIABLE w8_label VARCHAR2(30)

EXEC :w8_bind := 41 + 1

BEGIN
  :w8_label := 'wave8/' || TO_CHAR(:w8_bind);
END;
/

PRINT w8_bind

SELECT :w8_bind  AS bound_number,
       :w8_label AS bound_label
FROM dual;

SET DEFINE ON
DEFINE w8_floor = 5

SELECT COUNT(*) AS above_floor
FROM sq_hard_w8_note
WHERE note_value >= &w8_floor;

UNDEFINE w8_floor
SET DEFINE OFF

BREAK ON report
COMPUTE SUM LABEL 'total' OF note_value ON report

SELECT note_key, note_value
FROM sq_hard_w8_note
ORDER BY note_key;

CLEAR COMPUTES
CLEAR BREAKS

--------------------------------------------------------------------------------
-- W8-L: lexer round 3. Bare decimal points on both sides of a number, two
-- exponent spellings, a q-quote whose payload contains its own closing
-- delimiter, an unnestable block comment that swallows the AS keyword, three
-- spellings of not-equals, and a 123-byte identifier raised to a power.
--------------------------------------------------------------------------------
SELECT/*+ FULL(d) */DISTINCT d.doc_id/*id*/AS/**/doc_number,
       .5 + 5.                                          AS bare_decimals,
       1e2 + 1E-2                                       AS exponent_pair,
       'it''s' || q'[a]b]'                              AS glued_text,
       CASE
         WHEN 1 <> 2 AND 1 != 2 AND 1 ^= 2 THEN 'all-three'
       END                                              AS not_equal_spellings
FROM sq_hard_w8_doc d
WHERE d.doc_id = 1;

SELECT 1 /* outer /* inner */ AS still_aliased FROM dual;

DECLARE
  a_very_long_identifier_that_uses_most_of_the_one_hundred_and_twenty_eight_byte_limit_allowed_by_oracle_database_23ai NUMBER := 2;
  powered NUMBER;
BEGIN
  powered := a_very_long_identifier_that_uses_most_of_the_one_hundred_and_twenty_eight_byte_limit_allowed_by_oracle_database_23ai ** 10;
  INSERT INTO sq_hard_w8_note (note_key, note_text, note_value)
  VALUES ('lexer', 'long identifier powered', powered);
END;
/

--------------------------------------------------------------------------------
-- W8 self-verification.
--------------------------------------------------------------------------------
DECLARE
  mle_squares   NUMBER;
  queue_prio    NUMBER;
  queue_body    VARCHAR2(200);
  cursor_sum    NUMBER;
  powered       NUMBER;
  product_beta  NUMBER;
  auto_parts    PLS_INTEGER;
  hash_parts    PLS_INTEGER;
  flat_parts    PLS_INTEGER;
  ref_parts     PLS_INTEGER;
  head_rows     PLS_INTEGER;
  ddl_events    PLS_INTEGER;
  job_rows      PLS_INTEGER;
  text_hits     PLS_INTEGER;
  jct_reading   NUMBER;
  geo_gtype     NUMBER;
  mle_shouted   VARCHAR2(100);
BEGIN
  SELECT note_value INTO mle_squares
  FROM sq_hard_w8_note WHERE note_key = 'mle';
  SELECT note_value, note_text INTO queue_prio, queue_body
  FROM sq_hard_w8_note WHERE note_key = 'queue';
  SELECT note_value INTO cursor_sum
  FROM sq_hard_w8_note WHERE note_key = 'cursors';
  SELECT note_value INTO powered
  FROM sq_hard_w8_note WHERE note_key = 'lexer';

  SELECT sq_hard_w8_product(f.factor) INTO product_beta
  FROM sq_hard_w8_fact f WHERE f.dim_id = 2;

  SELECT COUNT(*) INTO auto_parts
  FROM user_tab_partitions WHERE table_name = 'SQ_HARD_W8_AUTO';
  SELECT COUNT(*) INTO hash_parts
  FROM user_tab_partitions WHERE table_name = 'SQ_HARD_W8_HASH';
  SELECT COUNT(*) INTO flat_parts
  FROM user_tab_partitions WHERE table_name = 'SQ_HARD_W8_FLAT';
  SELECT COUNT(*) INTO ref_parts
  FROM user_tab_partitions WHERE table_name = 'SQ_HARD_W8_REF';
  SELECT COUNT(*) INTO head_rows
  FROM sq_hard_w8_par PARTITION (p_head);
  SELECT COUNT(*) INTO ddl_events FROM sq_hard_w8_ddl_log;
  SELECT COUNT(*) INTO job_rows FROM sq_hard_w8_job_log;
  SELECT COUNT(*) INTO text_hits
  FROM sq_hard_w8_doc WHERE CONTAINS(body, 'lazy', 5) > 0;
  SELECT t.data.reading.number() INTO jct_reading
  FROM sq_hard_w8_jct t WHERE t.data."_id".number() = 2;
  SELECT g.shape.sdo_gtype INTO geo_gtype
  FROM sq_hard_w8_geo g WHERE g.geo_id = 2;
  SELECT sq_hard_w8_shout('mle') INTO mle_shouted FROM dual;

  IF mle_squares <> 55 THEN
    RAISE_APPLICATION_ERROR(-20810, 'w8 mle squares ' || mle_squares);
  END IF;
  IF queue_prio <> 3 OR queue_body NOT LIKE 'wave8 -- queued%' THEN
    RAISE_APPLICATION_ERROR(-20811, 'w8 queue ' || queue_prio || '/' || queue_body);
  END IF;
  IF cursor_sum <> 20 THEN
    RAISE_APPLICATION_ERROR(-20812, 'w8 cursor sum ' || cursor_sum);
  END IF;
  IF powered <> 1024 THEN
    RAISE_APPLICATION_ERROR(-20813, 'w8 powered ' || powered);
  END IF;
  IF product_beta <> 35 THEN
    RAISE_APPLICATION_ERROR(-20814, 'w8 product ' || product_beta);
  END IF;
  IF auto_parts <> 2 THEN
    RAISE_APPLICATION_ERROR(-20815, 'w8 automatic partitions ' || auto_parts);
  END IF;
  IF hash_parts <> 4 THEN
    RAISE_APPLICATION_ERROR(-20816, 'w8 hash partitions ' || hash_parts);
  END IF;
  IF flat_parts < 1 THEN
    RAISE_APPLICATION_ERROR(-20817, 'w8 flat partitions ' || flat_parts);
  END IF;
  IF ref_parts <> 3 THEN
    RAISE_APPLICATION_ERROR(-20818, 'w8 reference partitions ' || ref_parts);
  END IF;
  IF head_rows <> 2 THEN
    RAISE_APPLICATION_ERROR(-20819, 'w8 merged partition rows ' || head_rows);
  END IF;
  IF ddl_events <> 3 THEN
    RAISE_APPLICATION_ERROR(-20820, 'w8 ddl events ' || ddl_events);
  END IF;
  IF job_rows <> 1 THEN
    RAISE_APPLICATION_ERROR(-20821, 'w8 job rows ' || job_rows);
  END IF;
  IF text_hits <> 3 THEN
    RAISE_APPLICATION_ERROR(-20822, 'w8 text hits ' || text_hits);
  END IF;
  IF jct_reading <> 11 THEN
    RAISE_APPLICATION_ERROR(-20823, 'w8 json collection ' || jct_reading);
  END IF;
  IF geo_gtype <> 2003 THEN
    RAISE_APPLICATION_ERROR(-20824, 'w8 geometry gtype ' || geo_gtype);
  END IF;
  IF mle_shouted NOT LIKE 'MLE%' THEN
    RAISE_APPLICATION_ERROR(-20825, 'w8 mle shout ' || mle_shouted);
  END IF;
END;
/

--------------------------------------------------------------------------------
-- ULTRA WAVE 9: the object-relational surface (a NOT FINAL type with an
-- OVERRIDING subtype, an object table whose identity is its primary key, REF
-- columns scoped inline and by ALTER TABLE, and dot paths hanging off
-- DEREF()/VALUE()/TREAT() call results), PL/SQL qualified expressions with an
-- iterated aggregate and mutually recursive forward-declared local functions,
-- Virtual Private Database row and column policies, account-administration DDL
-- (profile, common user and role, ADMIN/GRANT OPTION, DEFAULT ROLE ALL EXCEPT,
-- proxy authentication, PUBLIC synonym), hypothetical-set and
-- inverse-distribution aggregates with LISTAGG ON OVERFLOW, number/date format
-- models nested two quoting levels deep, and client-command plus lexer round 4.
--------------------------------------------------------------------------------

-- Notes table carrying every value the wave-9 self-verification asserts.
CREATE TABLE sq_hard_w9_note (
  note_key   VARCHAR2(30) PRIMARY KEY,
  note_text  VARCHAR2(400),
  note_value NUMBER
);

--------------------------------------------------------------------------------
-- W9-A: the object-relational surface. A NOT FINAL object type, a subtype that
-- OVERRIDES a member, an object table whose identity IS its primary key, REF
-- columns scoped both inline and by ALTER TABLE, and dot paths that hang off
-- DEREF()/VALUE()/TREAT() call results rather than off a table alias.
--------------------------------------------------------------------------------
CREATE OR REPLACE TYPE sq_hard_w9_node_t AS OBJECT (
  node_id   NUMBER,
  node_name VARCHAR2(30),
  MEMBER FUNCTION shout RETURN VARCHAR2,
  MEMBER FUNCTION weighted (factor NUMBER) RETURN NUMBER
) NOT FINAL;
/

CREATE OR REPLACE TYPE BODY sq_hard_w9_node_t AS
  MEMBER FUNCTION shout RETURN VARCHAR2 IS
  BEGIN
    RETURN UPPER(node_name);
  END shout;

  MEMBER FUNCTION weighted (factor NUMBER) RETURN NUMBER IS
  BEGIN
    RETURN node_id * NVL(factor, 1);
  END weighted;
END;
/

CREATE OR REPLACE TYPE sq_hard_w9_leaf_t UNDER sq_hard_w9_node_t (
  weight NUMBER,
  OVERRIDING MEMBER FUNCTION shout RETURN VARCHAR2
);
/

CREATE OR REPLACE TYPE BODY sq_hard_w9_leaf_t AS
  OVERRIDING MEMBER FUNCTION shout RETURN VARCHAR2 IS
  BEGIN
    RETURN UPPER(node_name) || '#' || TO_CHAR(weight);
  END shout;
END;
/

CREATE TABLE sq_hard_w9_node OF sq_hard_w9_node_t (
  node_id   PRIMARY KEY,
  node_name NOT NULL
) OBJECT IDENTIFIER IS PRIMARY KEY;

INSERT INTO sq_hard_w9_node VALUES (sq_hard_w9_node_t(1, 'root'));
INSERT INTO sq_hard_w9_node VALUES (sq_hard_w9_leaf_t(2, 'left', 40));
INSERT INTO sq_hard_w9_node VALUES (sq_hard_w9_leaf_t(3, 'right', 60));

CREATE TABLE sq_hard_w9_edge (
  edge_id NUMBER CONSTRAINT sq_hard_w9_edge_pk PRIMARY KEY,
  child   REF sq_hard_w9_node_t SCOPE IS sq_hard_w9_node,
  parent  REF sq_hard_w9_node_t
);

ALTER TABLE sq_hard_w9_edge ADD SCOPE FOR (parent) IS sq_hard_w9_node;

INSERT INTO sq_hard_w9_edge (edge_id, child, parent)
SELECT 10,
       REF(c),
       (SELECT REF(p) FROM sq_hard_w9_node p WHERE p.node_id = 1)
FROM sq_hard_w9_node c
WHERE c.node_id = 2;

INSERT INTO sq_hard_w9_edge (edge_id, child, parent)
SELECT 20, REF(c), MAKE_REF(sq_hard_w9_node, 1)
FROM sq_hard_w9_node c
WHERE c.node_id = 3;

SELECT e.edge_id,
       DEREF(e.parent).node_name                              AS parent_name,
       DEREF(e.child).shout()                                 AS child_shout,
       DEREF(e.child).weighted(3)                             AS child_weighted,
       TREAT(DEREF(e.child) AS sq_hard_w9_leaf_t).weight      AS child_weight,
       CASE WHEN e.parent IS DANGLING THEN 'Y' ELSE 'N' END   AS parent_dangling
FROM sq_hard_w9_edge e
ORDER BY e.edge_id;

SELECT VALUE(n).shout()                                      AS shouted,
       n.node_id,
       CASE WHEN VALUE(n) IS OF (ONLY sq_hard_w9_leaf_t)
            THEN NVL(TREAT(VALUE(n) AS sq_hard_w9_leaf_t).weight, 0)
            ELSE -1 END                                      AS leaf_weight
FROM sq_hard_w9_node n
ORDER BY n.node_id;

CREATE TABLE sq_hard_w9_flat (
  flat_id   NUMBER CONSTRAINT sq_hard_w9_flat_pk PRIMARY KEY,
  flat_name VARCHAR2(30)
);

INSERT INTO sq_hard_w9_flat (flat_id, flat_name) VALUES (7, 'viewed');
INSERT INTO sq_hard_w9_flat (flat_id, flat_name) VALUES (8, 'mapped');

CREATE OR REPLACE VIEW sq_hard_w9_ov OF sq_hard_w9_node_t
  WITH OBJECT IDENTIFIER (node_id) AS
SELECT f.flat_id   AS node_id,
       f.flat_name AS node_name
FROM sq_hard_w9_flat f
WHERE f.flat_id > 0;

SELECT o.node_id,
       o.node_name,
       VALUE(o).shout()                                     AS view_shout,
       CASE WHEN REF(o) IS NOT NULL THEN 'Y' ELSE 'N' END   AS has_oid
FROM sq_hard_w9_ov o
ORDER BY o.node_id;

DECLARE
  edge_shout   VARCHAR2(60);
  leaf_weight  NUMBER;
  view_rows    PLS_INTEGER;
BEGIN
  SELECT DEREF(e.child).shout(), TREAT(DEREF(e.child) AS sq_hard_w9_leaf_t).weight
  INTO edge_shout, leaf_weight
  FROM sq_hard_w9_edge e
  WHERE e.edge_id = 10;

  SELECT COUNT(*) INTO view_rows FROM sq_hard_w9_ov;

  INSERT INTO sq_hard_w9_note (note_key, note_text, note_value)
  VALUES ('ref-graph', edge_shout || '/' || view_rows, leaf_weight);
END;
/

--------------------------------------------------------------------------------
-- W9-B: qualified expressions. A record, an associative array indexed by a
-- string, and an iterated aggregate build their whole value inside a single
-- expression, and two locally forward-declared subprograms recurse into each
-- other before either has a body.
--------------------------------------------------------------------------------
CREATE OR REPLACE PACKAGE sq_hard_w9_qual_pkg AUTHID DEFINER AS
  TYPE bucket_rec IS RECORD (
    label VARCHAR2(20),
    qty   NUMBER,
    tags  sys.odcivarchar2list
  );
  TYPE bucket_map IS TABLE OF bucket_rec INDEX BY VARCHAR2(20);
  TYPE square_arr IS TABLE OF NUMBER INDEX BY PLS_INTEGER;

  FUNCTION squares_total (upto PLS_INTEGER) RETURN NUMBER;
  FUNCTION bucket_digest RETURN VARCHAR2;
  FUNCTION parity_of (n PLS_INTEGER) RETURN VARCHAR2;
END sq_hard_w9_qual_pkg;
/

CREATE OR REPLACE PACKAGE BODY sq_hard_w9_qual_pkg AS
  FUNCTION squares_total (upto PLS_INTEGER) RETURN NUMBER IS
    squares square_arr := square_arr(FOR i IN 1 .. 6 => i * i);
    total   NUMBER := 0;
  BEGIN
    FOR idx IN 1 .. LEAST(upto, squares.COUNT) LOOP
      total := total + squares(idx);
    END LOOP;
    RETURN total;
  END squares_total;

  FUNCTION bucket_digest RETURN VARCHAR2 IS
    buckets bucket_map := bucket_map(
      'alpha' => bucket_rec(label => 'alpha',
                            qty   => 10,
                            tags  => sys.odcivarchar2list('a', 'b')),
      'beta'  => bucket_rec(label => 'beta',
                            qty   => 20,
                            tags  => sys.odcivarchar2list('c')));
    key_text VARCHAR2(20);
    digest   VARCHAR2(200);
  BEGIN
    key_text := buckets.FIRST;
    WHILE key_text IS NOT NULL LOOP
      digest := digest || buckets(key_text).label || '=' ||
                buckets(key_text).qty || ':' ||
                buckets(key_text).tags.COUNT || ';';
      key_text := buckets.NEXT(key_text);
    END LOOP;
    RETURN digest;
  END bucket_digest;

  FUNCTION parity_of (n PLS_INTEGER) RETURN VARCHAR2 IS
    FUNCTION is_even (v PLS_INTEGER) RETURN BOOLEAN;

    FUNCTION is_odd (v PLS_INTEGER) RETURN BOOLEAN IS
    BEGIN
      RETURN CASE WHEN v = 0 THEN FALSE ELSE is_even(v - 1) END;
    END is_odd;

    FUNCTION is_even (v PLS_INTEGER) RETURN BOOLEAN IS
    BEGIN
      RETURN CASE WHEN v = 0 THEN TRUE ELSE is_odd(v - 1) END;
    END is_even;
  BEGIN
    RETURN CASE WHEN is_even(n) THEN 'even' ELSE 'odd' END;
  END parity_of;
END sq_hard_w9_qual_pkg;
/

SELECT sq_hard_w9_qual_pkg.squares_total(4)  AS squares_total,
       sq_hard_w9_qual_pkg.bucket_digest     AS bucket_digest,
       sq_hard_w9_qual_pkg.parity_of(9)      AS parity_nine
FROM dual;

DECLARE
  digest VARCHAR2(200) := sq_hard_w9_qual_pkg.bucket_digest;
BEGIN
  INSERT INTO sq_hard_w9_note (note_key, note_text, note_value)
  VALUES ('qualified-expression',
          digest || sq_hard_w9_qual_pkg.parity_of(9),
          sq_hard_w9_qual_pkg.squares_total(6));
END;
/

--------------------------------------------------------------------------------
-- W9-C: Virtual Private Database. A policy function returns a predicate as a
-- q-quoted string, one policy filters rows and a second masks a single column
-- through SEC_RELEVANT_COLS with ALL_ROWS, and the policies are toggled off and
-- back on around a query that must see the difference.
--------------------------------------------------------------------------------
CREATE TABLE sq_hard_w9_sec (
  sec_id NUMBER CONSTRAINT sq_hard_w9_sec_pk PRIMARY KEY,
  region VARCHAR2(4)  NOT NULL,
  secret VARCHAR2(20) NOT NULL
);

INSERT INTO sq_hard_w9_sec (sec_id, region, secret) VALUES (1, 'EU', 'hush-eu');
INSERT INTO sq_hard_w9_sec (sec_id, region, secret) VALUES (2, 'US', 'hush-us');
INSERT INTO sq_hard_w9_sec (sec_id, region, secret) VALUES (3, 'EU', 'quiet-eu');
COMMIT;

CREATE OR REPLACE FUNCTION sq_hard_w9_region_pred (
  schema_name IN VARCHAR2,
  object_name IN VARCHAR2
) RETURN VARCHAR2 AS
BEGIN
  RETURN q'[region = 'EU']';
END sq_hard_w9_region_pred;
/

BEGIN
  DBMS_RLS.ADD_POLICY(object_schema   => 'SYSTEM',
                      object_name     => 'SQ_HARD_W9_SEC',
                      policy_name     => 'SQ_HARD_W9_ROWPOL',
                      function_schema => 'SYSTEM',
                      policy_function => 'SQ_HARD_W9_REGION_PRED',
                      statement_types => 'SELECT',
                      policy_type     => DBMS_RLS.STATIC);

  DBMS_RLS.ADD_POLICY(object_schema         => 'SYSTEM',
                      object_name           => 'SQ_HARD_W9_SEC',
                      policy_name           => 'SQ_HARD_W9_COLPOL',
                      function_schema       => 'SYSTEM',
                      policy_function       => 'SQ_HARD_W9_REGION_PRED',
                      statement_types       => 'SELECT',
                      sec_relevant_cols     => 'SECRET',
                      sec_relevant_cols_opt => DBMS_RLS.ALL_ROWS);
END;
/

SELECT s.sec_id, s.region, s.secret
FROM sq_hard_w9_sec s
ORDER BY s.sec_id;

SELECT p.policy_name, p.sel, p.policy_type
FROM user_policies p
WHERE p.object_name = 'SQ_HARD_W9_SEC'
ORDER BY p.policy_name;

BEGIN
  DBMS_RLS.ENABLE_POLICY(object_schema => 'SYSTEM',
                         object_name   => 'SQ_HARD_W9_SEC',
                         policy_name   => 'SQ_HARD_W9_ROWPOL',
                         enable        => FALSE);
END;
/

DECLARE
  filtered_rows PLS_INTEGER;
  all_rows      PLS_INTEGER;
  masked_rows   PLS_INTEGER;
BEGIN
  SELECT COUNT(*) INTO all_rows FROM sq_hard_w9_sec;

  SELECT COUNT(*) INTO masked_rows
  FROM sq_hard_w9_sec s
  WHERE s.secret IS NULL;

  DBMS_RLS.ENABLE_POLICY('SYSTEM', 'SQ_HARD_W9_SEC', 'SQ_HARD_W9_ROWPOL', TRUE);

  SELECT COUNT(*) INTO filtered_rows FROM sq_hard_w9_sec;

  INSERT INTO sq_hard_w9_note (note_key, note_text, note_value)
  VALUES ('vpd',
          'all=' || all_rows || ' masked=' || masked_rows ||
          ' filtered=' || filtered_rows,
          filtered_rows);

  IF all_rows <> 3 OR masked_rows <> 1 OR filtered_rows <> 2 THEN
    RAISE_APPLICATION_ERROR(-20901,
      'vpd shape ' || all_rows || '/' || masked_rows || '/' || filtered_rows);
  END IF;
END;
/

BEGIN
  DBMS_RLS.DROP_POLICY('SYSTEM', 'SQ_HARD_W9_SEC', 'SQ_HARD_W9_ROWPOL');
  DBMS_RLS.DROP_POLICY('SYSTEM', 'SQ_HARD_W9_SEC', 'SQ_HARD_W9_COLPOL');
END;
/

--------------------------------------------------------------------------------
-- W9-D: account administration. A profile with a LIMIT list, a common user and
-- a common role whose names carry the C## prefix, privilege grants with ADMIN
-- and GRANT OPTION, a DEFAULT ROLE ALL EXCEPT list, proxy authentication and a
-- PUBLIC synonym -- all DDL shapes no earlier wave produced.
--------------------------------------------------------------------------------
CREATE PROFILE sq_hard_w9_prof LIMIT
  SESSIONS_PER_USER      3
  CPU_PER_CALL           DEFAULT
  CONNECT_TIME           UNLIMITED
  IDLE_TIME              30
  FAILED_LOGIN_ATTEMPTS  5
  PASSWORD_LIFE_TIME     90
  PASSWORD_REUSE_TIME    UNLIMITED
  PASSWORD_GRACE_TIME    7
  PASSWORD_VERIFY_FUNCTION NULL;

CREATE USER c##sq_hard_w9_u IDENTIFIED BY "Str0ng#W9pass"
  DEFAULT TABLESPACE users
  QUOTA 5M ON users
  PROFILE sq_hard_w9_prof
  PASSWORD EXPIRE
  ACCOUNT LOCK
  CONTAINER = ALL;

CREATE ROLE c##sq_hard_w9_r IDENTIFIED BY "R0le#W9pass" CONTAINER = ALL;

GRANT CREATE SESSION, CREATE TABLE, CREATE VIEW TO c##sq_hard_w9_r CONTAINER = ALL;
GRANT c##sq_hard_w9_r TO c##sq_hard_w9_u WITH ADMIN OPTION CONTAINER = ALL;
GRANT SELECT, INSERT (sec_id, region) ON sq_hard_w9_sec
  TO c##sq_hard_w9_u WITH GRANT OPTION;

ALTER USER c##sq_hard_w9_u DEFAULT ROLE ALL EXCEPT c##sq_hard_w9_r CONTAINER = ALL;
ALTER USER c##sq_hard_w9_u GRANT CONNECT THROUGH system;
ALTER PROFILE sq_hard_w9_prof LIMIT IDLE_TIME 45;

CREATE PUBLIC SYNONYM sq_hard_w9_pub FOR system.sq_hard_w9_sec;

SELECT u.username,
       u.account_status,
       u.profile,
       (SELECT COUNT(*)
        FROM dba_role_privs r
        WHERE r.grantee = u.username
          AND r.granted_role = 'C##SQ_HARD_W9_R'
          AND r.admin_option = 'YES')                AS admin_grants,
       (SELECT COUNT(*)
        FROM dba_proxies x
        WHERE x.client = u.username
          AND x.proxy = 'SYSTEM')                    AS proxy_rows,
       (SELECT p."LIMIT"
        FROM dba_profiles p
        WHERE p.profile = 'SQ_HARD_W9_PROF'
          AND p.resource_name = 'IDLE_TIME')         AS idle_limit
FROM dba_users u
WHERE u.username = 'C##SQ_HARD_W9_U';

DECLARE
  syn_rows   PLS_INTEGER;
  col_privs  PLS_INTEGER;
BEGIN
  SELECT COUNT(*) INTO syn_rows
  FROM dba_synonyms
  WHERE owner = 'PUBLIC' AND synonym_name = 'SQ_HARD_W9_PUB';

  SELECT COUNT(*) INTO col_privs
  FROM dba_col_privs
  WHERE grantee = 'C##SQ_HARD_W9_U' AND table_name = 'SQ_HARD_W9_SEC';

  INSERT INTO sq_hard_w9_note (note_key, note_text, note_value)
  VALUES ('accounts', 'synonyms=' || syn_rows || ' colprivs=' || col_privs,
          syn_rows + col_privs);
END;
/

REVOKE INSERT ON sq_hard_w9_sec FROM c##sq_hard_w9_u;
DROP PUBLIC SYNONYM sq_hard_w9_pub;
ALTER USER c##sq_hard_w9_u REVOKE CONNECT THROUGH system;
DROP USER c##sq_hard_w9_u CASCADE;
DROP ROLE c##sq_hard_w9_r;
DROP PROFILE sq_hard_w9_prof CASCADE;

--------------------------------------------------------------------------------
-- W9-E: aggregate grammar that puts a whole argument list in front of the
-- WITHIN GROUP: hypothetical-set ranks, inverse-distribution percentiles used
-- both as aggregates and as analytics, and LISTAGG carrying an ON OVERFLOW
-- clause whose truncation literal must not be read as the separator.
--------------------------------------------------------------------------------
CREATE TABLE sq_hard_w9_bucket (
  bucket_id NUMBER CONSTRAINT sq_hard_w9_bucket_pk PRIMARY KEY,
  bucket    VARCHAR2(10) NOT NULL,
  label     VARCHAR2(20) NOT NULL,
  qty       NUMBER       NOT NULL
);

INSERT INTO sq_hard_w9_bucket VALUES (1, 'alpha', 'a-one',   10);
INSERT INTO sq_hard_w9_bucket VALUES (2, 'alpha', 'a-two',   20);
INSERT INTO sq_hard_w9_bucket VALUES (3, 'alpha', 'a-three', 30);
INSERT INTO sq_hard_w9_bucket VALUES (4, 'beta',  'b-one',   40);
INSERT INTO sq_hard_w9_bucket VALUES (5, 'beta',  'b-two',   50);
INSERT INTO sq_hard_w9_bucket VALUES (6, 'beta',  'b-three', 50);
COMMIT;

SELECT b.bucket,
       RANK(35) WITHIN GROUP (ORDER BY b.qty)                     AS hypo_rank,
       DENSE_RANK(35) WITHIN GROUP (ORDER BY b.qty DESC)          AS hypo_dense,
       ROUND(PERCENT_RANK(35) WITHIN GROUP (ORDER BY b.qty), 3)   AS hypo_pct,
       ROUND(CUME_DIST(35) WITHIN GROUP (ORDER BY b.qty), 3)      AS hypo_cume,
       PERCENTILE_DISC(0.5) WITHIN GROUP (ORDER BY b.qty)         AS med_disc,
       PERCENTILE_CONT(0.5) WITHIN GROUP (ORDER BY b.qty)         AS med_cont,
       MEDIAN(b.qty)                                              AS median_qty,
       STATS_MODE(b.qty)                                          AS modal_qty,
       LISTAGG(b.label, ';' ON OVERFLOW TRUNCATE '..' WITH COUNT)
         WITHIN GROUP (ORDER BY b.qty DESC, b.label)              AS rolled
FROM sq_hard_w9_bucket b
GROUP BY b.bucket
ORDER BY b.bucket;

SELECT DISTINCT b.bucket,
       PERCENTILE_DISC(0.25) WITHIN GROUP (ORDER BY b.qty)
         OVER (PARTITION BY b.bucket)                             AS q1_disc,
       ROUND(PERCENTILE_CONT(0.75) WITHIN GROUP (ORDER BY b.qty)
         OVER (PARTITION BY b.bucket), 2)                         AS q3_cont,
       COUNT(*) OVER (PARTITION BY b.bucket)                      AS in_bucket
FROM sq_hard_w9_bucket b
ORDER BY b.bucket;

SELECT grouped.bucket,
       ROUND(grouped.qty_share, 3)                                AS qty_share,
       grouped.width_slot,
       ROUND(grouped.qty_corr, 3)                                 AS qty_corr,
       ROUND(grouped.qty_stddev, 3)                               AS qty_stddev
FROM (
  SELECT b.bucket,
         RATIO_TO_REPORT(SUM(b.qty)) OVER ()                      AS qty_share,
         WIDTH_BUCKET(SUM(b.qty), 0, 200, 4)                      AS width_slot,
         CORR(b.qty, b.bucket_id)                                 AS qty_corr,
         STDDEV_SAMP(b.qty)                                       AS qty_stddev
  FROM sq_hard_w9_bucket b
  GROUP BY b.bucket
) grouped
ORDER BY grouped.bucket;

DECLARE
  alpha_med  NUMBER;
  beta_roll  VARCHAR2(200);
  hypo       NUMBER;
BEGIN
  SELECT PERCENTILE_DISC(0.5) WITHIN GROUP (ORDER BY qty),
         RANK(35) WITHIN GROUP (ORDER BY qty)
  INTO alpha_med, hypo
  FROM sq_hard_w9_bucket
  WHERE bucket = 'alpha';

  SELECT LISTAGG(label, ';' ON OVERFLOW TRUNCATE '..' WITH COUNT)
           WITHIN GROUP (ORDER BY qty DESC, label)
  INTO beta_roll
  FROM sq_hard_w9_bucket
  WHERE bucket = 'beta';

  INSERT INTO sq_hard_w9_note (note_key, note_text, note_value)
  VALUES ('aggregates', beta_roll || '/' || hypo, alpha_med);
END;
/

--------------------------------------------------------------------------------
-- W9-F: number and date format models. The model itself is a string that
-- contains double-quoted literal text, and the NLS parameter argument is a
-- string containing doubled quotes -- two nested quoting levels inside one
-- function call.
--------------------------------------------------------------------------------
SELECT TO_CHAR(1234.5, 'FM999G999D00', 'NLS_NUMERIC_CHARACTERS = '',.''') AS euro_amount,
       TO_CHAR(1234.5, 'FM999G999D00', 'NLS_NUMERIC_CHARACTERS = ''.,''') AS us_amount,
       TO_CHAR(-42, '999PR')                                              AS paren_negative,
       TO_CHAR(255, 'FMXXXX')                                             AS hex_digits,
       TO_CHAR(2024, 'FMRN')                                              AS roman_year,
       TO_CHAR(0.5, 'FM90D900')                                           AS padded_decimal,
       TO_CHAR(DATE '2024-02-29', '"day "DDD" of "YYYY')                  AS labelled_day,
       TO_CHAR(DATE '2024-02-29', 'IYYY"-W"IW')                           AS iso_week,
       TO_CHAR(TIMESTAMP '2024-02-29 13:45:56.123456',
               'YYYY-MM-DD"T"HH24:MI:SSxFF3')                             AS iso_stamp,
       TO_DATE('29-FEB-24', 'DD-MON-RR', 'NLS_DATE_LANGUAGE = AMERICAN')  AS parsed_date
FROM dual;

SELECT EXTRACT(YEAR FROM NUMTOYMINTERVAL(14, 'MONTH'))                    AS ym_years,
       EXTRACT(MONTH FROM NUMTOYMINTERVAL(14, 'MONTH'))                   AS ym_months,
       EXTRACT(MINUTE FROM NUMTODSINTERVAL(90, 'MINUTE'))                 AS ds_minutes,
       CASE WHEN NLSSORT('Ärger', 'NLS_SORT = GERMAN') >
                 NLSSORT('Aal', 'NLS_SORT = GERMAN')
            THEN 'after' ELSE 'before' END                                AS german_order,
       TO_CHAR(TRUNC(DATE '2024-02-29', 'IW'), 'YYYY-MM-DD')              AS iso_week_start
FROM dual;

DECLARE
  euro_text VARCHAR2(40);
  roman     VARCHAR2(20);
BEGIN
  SELECT TO_CHAR(9876.54, 'FM999G999D00', 'NLS_NUMERIC_CHARACTERS = '',.'''),
         TO_CHAR(1999, 'FMRN')
  INTO euro_text, roman
  FROM dual;

  INSERT INTO sq_hard_w9_note (note_key, note_text, note_value)
  VALUES ('formats', euro_text || '|' || roman, LENGTH(euro_text));
END;
/

--------------------------------------------------------------------------------
-- W9-G: client-command round 4 and lexer round 4. A REFCURSOR bind opened by
-- EXEC and drained by PRINT, doubled-ampersand substitution that defines the
-- variable on first use, then literals that sit one character away from being
-- something else entirely.
--------------------------------------------------------------------------------
VARIABLE w9_cursor REFCURSOR
VARIABLE w9_count NUMBER

EXEC OPEN :w9_cursor FOR SELECT bucket, qty FROM sq_hard_w9_bucket WHERE qty >= 40 ORDER BY bucket_id

PRINT w9_cursor

EXEC SELECT COUNT(*) INTO :w9_count FROM sq_hard_w9_bucket

PRINT w9_count

SET DEFINE ON
DEFINE w9_floor = 30

SELECT bucket, SUM(qty) AS bucket_qty
FROM sq_hard_w9_bucket
WHERE qty >= &&w9_floor
GROUP BY bucket
ORDER BY bucket;

UNDEFINE w9_floor
SET DEFINE OFF

BREAK ON report
COMPUTE SUM LABEL 'w9 total' OF bucket_qty ON report

SELECT bucket, SUM(qty) AS bucket_qty
FROM sq_hard_w9_bucket
GROUP BY bucket
ORDER BY bucket;

CLEAR COMPUTES
CLEAR BREAKS

SELECT 1--1
FROM dual;

SELECT q'{outer {balanced} done}'                AS brace_quote,
       nq'#national ' inside#'                   AS national_quote,
       'It''s'                                   AS apostrophe,
       'a' || CHR(10) || 'b'                     AS two_lines,
       1 AS "colümn
break",
       -(-3)                                     AS double_negate,
       3--(-2)
                                                 AS comment_then_operand
FROM dual;

SELECT COUNT(*) AS lexer_rows
FROM sq_hard_w9_bucket b
WHERE b.qty>=10AND b.qty<=50 OR b.qty=NULL;

--------------------------------------------------------------------------------
-- Wave-9 self-verification.
--------------------------------------------------------------------------------
DECLARE
  ref_weight   NUMBER;
  squares      NUMBER;
  parity       VARCHAR2(10);
  vpd_rows     NUMBER;
  account_sum  NUMBER;
  alpha_med    NUMBER;
  format_len   NUMBER;
  note_rows    PLS_INTEGER;
  node_rows    PLS_INTEGER;
  leaf_shout   VARCHAR2(60);
BEGIN
  SELECT COUNT(*) INTO note_rows FROM sq_hard_w9_note;
  SELECT COUNT(*) INTO node_rows FROM sq_hard_w9_node;

  SELECT note_value INTO ref_weight FROM sq_hard_w9_note WHERE note_key = 'ref-graph';
  SELECT note_value INTO squares FROM sq_hard_w9_note WHERE note_key = 'qualified-expression';
  SELECT note_value INTO vpd_rows FROM sq_hard_w9_note WHERE note_key = 'vpd';
  SELECT note_value INTO account_sum FROM sq_hard_w9_note WHERE note_key = 'accounts';
  SELECT note_value INTO alpha_med FROM sq_hard_w9_note WHERE note_key = 'aggregates';
  SELECT note_value INTO format_len FROM sq_hard_w9_note WHERE note_key = 'formats';

  parity := sq_hard_w9_qual_pkg.parity_of(8);

  SELECT DEREF(e.child).shout() INTO leaf_shout
  FROM sq_hard_w9_edge e
  WHERE e.edge_id = 20;

  IF note_rows <> 6 THEN
    RAISE_APPLICATION_ERROR(-20910, 'w9 note rows ' || note_rows);
  END IF;
  IF node_rows <> 3 THEN
    RAISE_APPLICATION_ERROR(-20911, 'w9 node rows ' || node_rows);
  END IF;
  IF ref_weight <> 40 THEN
    RAISE_APPLICATION_ERROR(-20912, 'w9 ref weight ' || ref_weight);
  END IF;
  IF leaf_shout <> 'RIGHT#60' THEN
    RAISE_APPLICATION_ERROR(-20913, 'w9 leaf shout ' || leaf_shout);
  END IF;
  IF squares <> 91 THEN
    RAISE_APPLICATION_ERROR(-20914, 'w9 squares total ' || squares);
  END IF;
  IF parity <> 'even' THEN
    RAISE_APPLICATION_ERROR(-20915, 'w9 parity ' || parity);
  END IF;
  IF vpd_rows <> 2 THEN
    RAISE_APPLICATION_ERROR(-20916, 'w9 vpd rows ' || vpd_rows);
  END IF;
  IF account_sum <> 3 THEN
    RAISE_APPLICATION_ERROR(-20917, 'w9 account objects ' || account_sum);
  END IF;
  IF alpha_med <> 20 THEN
    RAISE_APPLICATION_ERROR(-20918, 'w9 alpha median ' || alpha_med);
  END IF;
  IF format_len <> 8 THEN
    RAISE_APPLICATION_ERROR(-20919, 'w9 format length ' || format_len);
  END IF;
END;
/

--------------------------------------------------------------------------------
-- ULTRA WAVE 10: recursive subquery factoring closed by SEARCH and CYCLE
-- clauses beside a NOCYCLE CONNECT BY walk of the same cyclic graph, the
-- SQL/XML and SQL/JSON row sources (XMLTABLE with XMLNAMESPACES and
-- FOR ORDINALITY, JSON_TABLE with a NESTED PATH and EXISTS/DEFAULT-ON-ERROR
-- columns, simplified JSON dot notation with an array step, an XQuery prolog
-- carried inside a string), temporal validity (PERIOD FOR with AS OF PERIOD FOR
-- and VERSIONS PERIOD FOR), bulk DML error harvesting (FORALL SAVE EXCEPTIONS
-- feeding SQL%BULK_EXCEPTIONS(i).ERROR_INDEX) with a call-stack backtrace, a
-- user-defined OPERATOR bound to a NONEDITIONABLE function, parenthesised
-- multi-action ALTER TABLE, the NLS-parameterised scalar family, and a client
-- plus lexer round 5 whose identifiers and literals carry the terminator,
-- comment introducers and block keywords the splitter looks for.
--------------------------------------------------------------------------------

-- Notes table carrying every value the wave-10 self-verification asserts.
CREATE TABLE sq_hard_w10_note (
  note_key   VARCHAR2(30) PRIMARY KEY,
  note_text  VARCHAR2(400),
  note_value NUMBER
);

--------------------------------------------------------------------------------
-- W10-A: recursive subquery factoring over a graph that really does cycle. The
-- SEARCH clause invents an ordering column, the CYCLE clause invents a flag
-- column, and neither name exists anywhere in the FROM list -- both are only
-- resolvable from the WITH header. The same graph is then walked by a legacy
-- CONNECT BY whose NOCYCLE keyword is the only reason it terminates.
--------------------------------------------------------------------------------
CREATE TABLE sq_hard_w10_route (
  route_id NUMBER CONSTRAINT sq_hard_w10_route_pk PRIMARY KEY,
  origin   VARCHAR2(10) NOT NULL,
  target   VARCHAR2(10) NOT NULL,
  cost     NUMBER(6, 2)
);

INSERT INTO sq_hard_w10_route (route_id, origin, target, cost) VALUES (1, 'ALPHA', 'BETA', 10);
INSERT INTO sq_hard_w10_route (route_id, origin, target, cost) VALUES (2, 'BETA', 'GAMMA', 20);
INSERT INTO sq_hard_w10_route (route_id, origin, target, cost) VALUES (3, 'GAMMA', 'ALPHA', 30);
INSERT INTO sq_hard_w10_route (route_id, origin, target, cost) VALUES (4, 'BETA', 'DELTA', 40);
COMMIT;

WITH hops (hop_no, origin, target, total_cost) AS (
  SELECT 1, r.origin, r.target, r.cost
  FROM sq_hard_w10_route r
  WHERE r.origin = 'ALPHA'
  UNION ALL
  SELECT h.hop_no + 1, r.origin, r.target, h.total_cost + r.cost
  FROM hops h
  JOIN sq_hard_w10_route r
    ON r.origin = h.target
  WHERE h.hop_no < 4
)
SEARCH DEPTH FIRST BY origin ASC NULLS LAST SET walk_order
CYCLE target SET is_cycle TO 'Y' DEFAULT 'N'
SELECT hop_no, origin, target, total_cost, walk_order, is_cycle
FROM hops
ORDER BY walk_order;

SELECT LPAD(' ', 2 * (LEVEL - 1)) || r.target AS tree_line,
       SYS_CONNECT_BY_PATH(r.target, '/')     AS hop_path,
       CONNECT_BY_ROOT r.origin               AS root_origin,
       CONNECT_BY_ISLEAF                      AS is_leaf,
       LEVEL                                  AS depth
FROM sq_hard_w10_route r
START WITH r.origin = 'ALPHA'
CONNECT BY NOCYCLE PRIOR r.target = r.origin
ORDER SIBLINGS BY r.target;

DECLARE
  walked PLS_INTEGER;
  cycled PLS_INTEGER;
BEGIN
  WITH hops (hop_no, origin, target, total_cost) AS (
    SELECT 1, r.origin, r.target, r.cost
    FROM sq_hard_w10_route r
    WHERE r.origin = 'ALPHA'
    UNION ALL
    SELECT h.hop_no + 1, r.origin, r.target, h.total_cost + r.cost
    FROM hops h
    JOIN sq_hard_w10_route r
      ON r.origin = h.target
    WHERE h.hop_no < 4
  )
  SEARCH BREADTH FIRST BY target DESC SET walk_order
  CYCLE target SET is_cycle TO 'Y' DEFAULT 'N'
  SELECT COUNT(*), COUNT(CASE WHEN is_cycle = 'Y' THEN 1 END)
  INTO walked, cycled
  FROM hops;

  DBMS_OUTPUT.PUT_LINE('[w10] recursive walk ' || walked || ' cycle flags ' || cycled);
  INSERT INTO sq_hard_w10_note (note_key, note_text, note_value)
  VALUES ('recursive', 'walked ' || walked || ' cycled ' || cycled, walked);
END;
/

--------------------------------------------------------------------------------
-- W10-B: the SQL/XML and SQL/JSON row sources. XMLTABLE declares namespaces in
-- its own sub-clause and mints columns from XPath fragments; JSON_TABLE nests a
-- second COLUMNS list under a path step and mixes EXISTS with DEFAULT ... ON
-- EMPTY / ON ERROR; simplified dot notation reaches through a JSON column with
-- an array subscript and a type method; and an XQuery prolog with its own
-- comment syntax rides inside a SQL string literal.
--------------------------------------------------------------------------------
CREATE TABLE sq_hard_w10_doc (
  doc_id  NUMBER CONSTRAINT sq_hard_w10_doc_pk PRIMARY KEY,
  payload XMLTYPE,
  profile JSON
) TABLESPACE users;

INSERT INTO sq_hard_w10_doc (doc_id, payload, profile)
VALUES (1,
        XMLTYPE('<catalog xmlns="http://sq/hard" xmlns:m="http://sq/meta">'
                || '<item id="1"><name>widget</name><m:qty>5</m:qty></item>'
                || '<item id="2"><name>gadget</name><m:qty>7</m:qty></item>'
                || '</catalog>'),
        JSON('{"name":"atlas","score":42,"roles":{"admin":true},'
             || '"tags":["x","y","z"]}'));
COMMIT;

SELECT x.item_id, x.item_name, x.seq, x.qty
FROM sq_hard_w10_doc d,
     XMLTABLE(XMLNAMESPACES(DEFAULT 'http://sq/hard', 'http://sq/meta' AS "m"),
              '/catalog/item'
              PASSING d.payload
              COLUMNS item_id   NUMBER       PATH '@id',
                      item_name VARCHAR2(30) PATH 'name',
                      seq       FOR ORDINALITY,
                      qty       NUMBER       PATH 'm:qty') x
WHERE d.doc_id = 1
ORDER BY x.item_id;

SELECT j.profile_name, j.has_admin, j.score, j.tag_pos, j.tag
FROM sq_hard_w10_doc d,
     JSON_TABLE(d.profile, '$'
       COLUMNS (profile_name VARCHAR2(30) PATH '$.name',
                has_admin    VARCHAR2(5)  EXISTS PATH '$.roles.admin',
                score        NUMBER       PATH '$.score' DEFAULT 0 ON EMPTY DEFAULT -1 ON ERROR,
                NESTED PATH '$.tags[*]'
                  COLUMNS (tag_pos FOR ORDINALITY,
                           tag     VARCHAR2(20) PATH '$'))) j
WHERE d.doc_id = 1
ORDER BY j.tag_pos;

SELECT d.profile.name.string()      AS dotted_name,
       d.profile.score.number()     AS dotted_score,
       d.profile.tags[1].string()   AS dotted_tag,
       d.profile.roles.admin.string() AS dotted_admin
FROM sq_hard_w10_doc d
WHERE d.doc_id = 1;

SELECT REPLACE(XMLSERIALIZE(CONTENT
         XMLQUERY('declare default element namespace "http://sq/hard"; (::)
                   for $i in /catalog/item
                   where $i/@id > 1
                   return $i/name'
                  PASSING d.payload RETURNING CONTENT)
         AS VARCHAR2(200) INDENT SIZE = 2), CHR(10), '|') AS serialized
FROM sq_hard_w10_doc d
WHERE d.doc_id = 1;

SELECT REPLACE(JSON_SERIALIZE(
         JSON_MERGEPATCH(d.profile, '{"score":99,"roles":null}')
         RETURNING VARCHAR2(300) PRETTY), CHR(10), '|') AS merged_profile
FROM sq_hard_w10_doc d
WHERE d.doc_id = 1;

SELECT JSON_ARRAYAGG(t.target ORDER BY t.target RETURNING VARCHAR2(200)) AS target_array,
       JSON_OBJECTAGG(KEY t.route_key VALUE t.cost RETURNING VARCHAR2(300)) AS route_object
FROM (SELECT TO_CHAR(r.route_id) AS route_key, r.target, r.cost
      FROM sq_hard_w10_route r
      ORDER BY r.route_id) t;

DECLARE
  tag_count PLS_INTEGER;
  qty_total NUMBER;
BEGIN
  SELECT COUNT(*) INTO tag_count
  FROM sq_hard_w10_doc d,
       JSON_TABLE(d.profile, '$.tags[*]' COLUMNS (tag VARCHAR2(20) PATH '$')) t;

  SELECT SUM(x.qty) INTO qty_total
  FROM sq_hard_w10_doc d,
       XMLTABLE(XMLNAMESPACES('http://sq/meta' AS "m", DEFAULT 'http://sq/hard'),
                '/catalog/item'
                PASSING d.payload
                COLUMNS qty NUMBER PATH 'm:qty') x;

  INSERT INTO sq_hard_w10_note (note_key, note_text, note_value)
  VALUES ('xml-json', 'tags ' || tag_count || ' qty ' || qty_total,
          tag_count + qty_total);
END;
/

--------------------------------------------------------------------------------
-- W10-C: temporal validity. PERIOD FOR mints a pseudo-column that exists only
-- in the table's metadata, and the two row sources that consume it -- AS OF
-- PERIOD FOR and VERSIONS PERIOD FOR ... BETWEEN -- sit exactly where a table
-- alias would otherwise be.
--------------------------------------------------------------------------------
CREATE TABLE sq_hard_w10_lease (
  lease_id   NUMBER CONSTRAINT sq_hard_w10_lease_pk PRIMARY KEY,
  tenant     VARCHAR2(20),
  valid_from DATE,
  valid_to   DATE,
  PERIOD FOR user_valid_time (valid_from, valid_to)
);

INSERT INTO sq_hard_w10_lease (lease_id, tenant, valid_from, valid_to)
VALUES (1, 'ada', DATE '2024-01-01', DATE '2024-06-01');
INSERT INTO sq_hard_w10_lease (lease_id, tenant, valid_from, valid_to)
VALUES (2, 'linus', DATE '2024-03-01', DATE '2024-12-01');
INSERT INTO sq_hard_w10_lease (lease_id, tenant, valid_from, valid_to)
VALUES (3, 'grace', DATE '2024-07-01', NULL);
COMMIT;

SELECT lease_id, tenant
FROM sq_hard_w10_lease AS OF PERIOD FOR user_valid_time DATE '2024-04-15'
ORDER BY lease_id;

SELECT lease_id, tenant
FROM sq_hard_w10_lease VERSIONS PERIOD FOR user_valid_time
       BETWEEN DATE '2024-06-15' AND DATE '2024-08-15'
ORDER BY lease_id;

DECLARE
  as_of_rows   PLS_INTEGER;
  between_rows PLS_INTEGER;
BEGIN
  SELECT COUNT(*) INTO as_of_rows
  FROM sq_hard_w10_lease AS OF PERIOD FOR user_valid_time DATE '2024-04-15';

  SELECT COUNT(*) INTO between_rows
  FROM sq_hard_w10_lease VERSIONS PERIOD FOR user_valid_time
         BETWEEN DATE '2024-06-15' AND DATE '2024-08-15';

  DBMS_OUTPUT.PUT_LINE('[w10] valid-time as-of ' || as_of_rows
                       || ' versions ' || between_rows);
  INSERT INTO sq_hard_w10_note (note_key, note_text, note_value)
  VALUES ('temporal', 'asof ' || as_of_rows || ' between ' || between_rows,
          as_of_rows * 10 + between_rows);
END;
/

--------------------------------------------------------------------------------
-- W10-D: bulk DML error harvesting. FORALL ... SAVE EXCEPTIONS keeps going past
-- a NOT NULL violation and parks the failures in SQL%BULK_EXCEPTIONS, whose
-- elements are records reached through a cursor-attribute subscript. A second
-- block re-raises with a preserved error stack and reads its own backtrace.
--------------------------------------------------------------------------------
CREATE TABLE sq_hard_w10_bulk (
  bulk_id NUMBER CONSTRAINT sq_hard_w10_bulk_pk PRIMARY KEY,
  label   VARCHAR2(10) NOT NULL
);

DECLARE
  TYPE label_tab IS TABLE OF sq_hard_w10_bulk.label%TYPE INDEX BY PLS_INTEGER;
  TYPE id_tab    IS TABLE OF sq_hard_w10_bulk.bulk_id%TYPE INDEX BY PLS_INTEGER;
  labels     label_tab;
  ids        id_tab;
  bad_count  PLS_INTEGER := 0;
  dml_errors EXCEPTION;
  PRAGMA EXCEPTION_INIT(dml_errors, -24381);
BEGIN
  FOR i IN 1 .. 6 LOOP
    ids(i)    := i;
    labels(i) := CASE WHEN MOD(i, 3) = 0 THEN NULL ELSE 'L' || i END;
  END LOOP;

  FORALL i IN 1 .. labels.COUNT SAVE EXCEPTIONS
    INSERT INTO sq_hard_w10_bulk (bulk_id, label) VALUES (ids(i), labels(i));
EXCEPTION
  WHEN dml_errors THEN
    bad_count := SQL%BULK_EXCEPTIONS.COUNT;
    FOR e IN 1 .. bad_count LOOP
      DBMS_OUTPUT.PUT_LINE('[w10] bulk failure at index '
                           || SQL%BULK_EXCEPTIONS(e).ERROR_INDEX
                           || ' code '
                           || SQL%BULK_EXCEPTIONS(e).ERROR_CODE);
    END LOOP;
    INSERT INTO sq_hard_w10_note (note_key, note_text, note_value)
    VALUES ('bulk-exceptions', 'saved ' || bad_count, bad_count);
END;
/
COMMIT;

SELECT COUNT(*) AS bulk_rows FROM sq_hard_w10_bulk;

CREATE OR REPLACE PROCEDURE sq_hard_w10_raiser AUTHID DEFINER IS
BEGIN
  RAISE_APPLICATION_ERROR(-20950, 'deliberate w10 failure', TRUE);
END sq_hard_w10_raiser;
/

DECLARE
  depth PLS_INTEGER;
BEGIN
  BEGIN
    sq_hard_w10_raiser;
  EXCEPTION
    WHEN OTHERS THEN
      depth := UTL_CALL_STACK.BACKTRACE_DEPTH;
      DBMS_OUTPUT.PUT_LINE('[w10] backtrace depth ' || depth);
      INSERT INTO sq_hard_w10_note (note_key, note_text, note_value)
      VALUES ('backtrace', 'depth captured', depth);
  END;
END;
/

--------------------------------------------------------------------------------
-- W10-E: a user-defined OPERATOR bound to a NONEDITIONABLE function, so a bare
-- identifier in a select list is neither a column nor a registered builtin; a
-- parenthesised multi-action ALTER TABLE; and the NLS-parameterised scalar
-- family, whose second argument is a settings string that itself contains an
-- equals sign and an embedded double-quoted literal in the format model.
--------------------------------------------------------------------------------
CREATE OR REPLACE NONEDITIONABLE FUNCTION sq_hard_w10_weigh (p_text IN VARCHAR2)
  RETURN NUMBER DETERMINISTIC
IS
BEGIN
  RETURN LENGTH(p_text) * 10;
END sq_hard_w10_weigh;
/

CREATE OR REPLACE OPERATOR sq_hard_w10_weight
  BINDING (VARCHAR2) RETURN NUMBER USING sq_hard_w10_weigh;

SELECT sq_hard_w10_weight(l.tenant) AS weighed, l.tenant
FROM sq_hard_w10_lease l
ORDER BY l.lease_id;

CREATE TABLE sq_hard_w10_shape (
  shape_id NUMBER CONSTRAINT sq_hard_w10_shape_pk PRIMARY KEY,
  label    VARCHAR2(20)
);

ALTER TABLE sq_hard_w10_shape ADD (extra_note VARCHAR2(20) DEFAULT 'none' NOT NULL,
                                   extra_qty  NUMBER       DEFAULT 0);
ALTER TABLE sq_hard_w10_shape MODIFY (extra_note VARCHAR2(40), label VARCHAR2(30));
ALTER TABLE sq_hard_w10_shape DROP (extra_qty);

INSERT INTO sq_hard_w10_shape (shape_id, label) VALUES (1, 'strasse');
INSERT INTO sq_hard_w10_shape (shape_id, label) VALUES (2, 'strabe');
COMMIT;

SELECT s.label,
       NLS_UPPER(s.label, 'NLS_SORT = XGERMAN')            AS nls_upper,
       TO_CHAR(DATE '2024-03-05', 'DD "de" Month YYYY',
               'NLS_DATE_LANGUAGE = SPANISH')              AS spanish_date,
       VALIDATE_CONVERSION('12x' AS NUMBER)                AS bad_number,
       CASE WHEN STANDARD_HASH(s.label, 'SHA256') IS NOT NULL
            THEN 'Y' ELSE 'N' END                          AS hashed
FROM sq_hard_w10_shape s
ORDER BY NLSSORT(s.label, 'NLS_SORT = GERMAN_CI');

SELECT WIDTH_BUCKET(37, 0, 100, 4)                    AS bucketed,
       REMAINDER(17, 5)                               AS remainder_val,
       CAST(NANVL(BINARY_DOUBLE_NAN, -1) AS NUMBER)   AS nan_replaced,
       BITAND(12, 10)                                 AS bit_and,
       DUMP(1, 16)                                    AS dumped
FROM dual;

DECLARE
  weighed NUMBER;
BEGIN
  SELECT sq_hard_w10_weight(l.tenant) INTO weighed
  FROM sq_hard_w10_lease l
  WHERE l.lease_id = 2;

  INSERT INTO sq_hard_w10_note (note_key, note_text, note_value)
  VALUES ('operator', 'weighed linus', weighed);
END;
/

--------------------------------------------------------------------------------
-- W10-F: client-command and lexer round 5. Break/compute report state wraps a
-- query, then identifiers and literals carry the statement terminator, both
-- comment introducers, an apostrophe and a trailing space; a string spans a
-- line break; a block-shaped literal carries BEGIN/END; and SQLBLANKLINES lets
-- a blank line sit in the middle of a statement.
--------------------------------------------------------------------------------
SET LINESIZE 200
SET PAGESIZE 60
SET HEADING ON
BREAK ON report
COMPUTE SUM LABEL 'lease id total' OF lease_id ON report

REM the query below is bracketed by report state, not by comments
PROMPT [w10] don't stop; keep going -- this prompt is not a comment

SELECT l.lease_id, l.tenant
FROM sq_hard_w10_lease l
ORDER BY l.lease_id;

CLEAR BREAKS
CLEAR COMPUTES

SELECT 1 AS "semi;colon",
       2 AS "dash--dash",
       3 AS "slash/*star",
       4 AS "it's",
       5 AS "trailing space "
FROM dual;

SELECT '-- not a comment /* either */' AS literal_comment,
       q'"double quoted q"'            AS q_double,
       q'<angle < bracket>'            AS q_angle,
       LENGTH('line one
line two')                             AS multiline_len
FROM dual
WHERE 1 = 1 -- trailing comment with an unbalanced ' quote
  AND 2 = 2 /* block comment with a lone ' quote */;

SET SQLBLANKLINES ON
SELECT 'blank-line-inside' AS lexer_probe,

       COUNT(*)            AS route_rows

FROM sq_hard_w10_route;
SET SQLBLANKLINES OFF

DECLARE
  fake_block VARCHAR2(200) := 'BEGIN NULL; END;';
  fake_stmt  VARCHAR2(200) := q'{SELECT 'x' FROM dual; -- still a literal}';
  lexer_len  PLS_INTEGER;
BEGIN
  SELECT LENGTH('-- not a comment /* either */') INTO lexer_len FROM dual;
  EXECUTE IMMEDIATE 'CREATE OR REPLACE VIEW sq_hard_w10_dyn AS SELECT '
                    || LENGTH(fake_block || fake_stmt)
                    || ' AS built_len FROM dual';
  INSERT INTO sq_hard_w10_note (note_key, note_text, note_value)
  VALUES ('lexer', 'literal comment length', lexer_len);
  COMMIT;
END;
/

SELECT built_len FROM sq_hard_w10_dyn;

--------------------------------------------------------------------------------
-- Wave-10 self-verification.
--------------------------------------------------------------------------------
DECLARE
  note_rows  PLS_INTEGER;
  walked     NUMBER;
  xml_json   NUMBER;
  temporal   NUMBER;
  bulk_saved NUMBER;
  bulk_rows  PLS_INTEGER;
  weighed    NUMBER;
  lexer_len  NUMBER;
  dyn_len    NUMBER;
  dotted_tag VARCHAR2(20);
BEGIN
  SELECT COUNT(*) INTO note_rows FROM sq_hard_w10_note;
  SELECT note_value INTO walked FROM sq_hard_w10_note WHERE note_key = 'recursive';
  SELECT note_value INTO xml_json FROM sq_hard_w10_note WHERE note_key = 'xml-json';
  SELECT note_value INTO temporal FROM sq_hard_w10_note WHERE note_key = 'temporal';
  SELECT note_value INTO bulk_saved FROM sq_hard_w10_note WHERE note_key = 'bulk-exceptions';
  SELECT note_value INTO weighed FROM sq_hard_w10_note WHERE note_key = 'operator';
  SELECT note_value INTO lexer_len FROM sq_hard_w10_note WHERE note_key = 'lexer';
  SELECT COUNT(*) INTO bulk_rows FROM sq_hard_w10_bulk;
  SELECT y.built_len INTO dyn_len FROM sq_hard_w10_dyn y;

  SELECT d.profile.tags[1].string() INTO dotted_tag
  FROM sq_hard_w10_doc d
  WHERE d.doc_id = 1;

  IF note_rows <> 7 THEN
    RAISE_APPLICATION_ERROR(-20920, 'w10 note rows ' || note_rows);
  END IF;
  IF walked <> 5 THEN
    RAISE_APPLICATION_ERROR(-20921, 'w10 recursive walk ' || walked);
  END IF;
  IF xml_json <> 15 THEN
    RAISE_APPLICATION_ERROR(-20922, 'w10 xml/json total ' || xml_json);
  END IF;
  IF temporal <> 22 THEN
    RAISE_APPLICATION_ERROR(-20923, 'w10 temporal shape ' || temporal);
  END IF;
  IF bulk_saved <> 2 OR bulk_rows <> 4 THEN
    RAISE_APPLICATION_ERROR(-20924, 'w10 bulk ' || bulk_saved || '/' || bulk_rows);
  END IF;
  IF weighed <> 50 THEN
    RAISE_APPLICATION_ERROR(-20925, 'w10 operator weight ' || weighed);
  END IF;
  IF lexer_len <> 29 THEN
    RAISE_APPLICATION_ERROR(-20926, 'w10 literal comment length ' || lexer_len);
  END IF;
  IF dyn_len <> 56 THEN
    RAISE_APPLICATION_ERROR(-20927, 'w10 built length ' || dyn_len);
  END IF;
  IF dotted_tag <> 'y' THEN
    RAISE_APPLICATION_ERROR(-20928, 'w10 dotted tag ' || dotted_tag);
  END IF;
END;
/

--------------------------------------------------------------------------------
-- WAVE 11 -- the declaration surface: every ANSI/legacy data-type spelling the
-- server accepts, a schema built by one multi-DDL statement, scalable sequences,
-- optimizer-hint comments that carry their own quotes and line breaks, the
-- PL/SQL type system (constrained subtypes, records of records, a strongly
-- typed ref cursor passed IN OUT) driven by the SYS utility packages, and a
-- sixth lexer round.
--------------------------------------------------------------------------------

CREATE TABLE sq_hard_w11_note (
  note_key   VARCHAR2(30) CONSTRAINT sq_hard_w11_note_pk PRIMARY KEY,
  note_text  VARCHAR2(200),
  note_value NUMBER
) TABLESPACE users;

--------------------------------------------------------------------------------
-- W11-A: the data-type zoo. ANSI aliases (INTEGER / SMALLINT / DEC / NUMERIC /
-- DOUBLE PRECISION / REAL / CHARACTER VARYING / NATIONAL CHARACTER VARYING) sit
-- beside a negative-scale NUMBER, both length semantics, precision-carrying
-- interval and timestamp types, an identity column with its own option list,
-- DEFAULT ON NULL, and a SECUREFILE LOB storage clause with a named segment.
--------------------------------------------------------------------------------
CREATE TABLE sq_hard_w11_type (
  row_id        NUMBER GENERATED BY DEFAULT ON NULL AS IDENTITY
                  (START WITH 5 INCREMENT BY 2 NOCACHE ORDER),
  ansi_int      INTEGER,
  ansi_small    SMALLINT,
  ansi_dec      DEC(7, 2),
  ansi_numeric  NUMERIC(9, 3),
  ansi_double   DOUBLE PRECISION,
  ansi_real     REAL,
  ansi_float    FLOAT(126),
  rounded_num   NUMBER(*, -2),
  char_semantic VARCHAR2(10 CHAR),
  byte_semantic VARCHAR2(10 BYTE),
  ansi_varying  CHARACTER VARYING (12),
  national_txt  NATIONAL CHARACTER VARYING (12),
  fixed_nchar   NCHAR(3),
  raw_bytes     RAW(8),
  local_tz      TIMESTAMP(3) WITH LOCAL TIME ZONE,
  ds_span       INTERVAL DAY(4) TO SECOND(2),
  ym_span       INTERVAL YEAR(3) TO MONTH,
  long_note     NCLOB,
  hidden_note   VARCHAR2(20) DEFAULT ON NULL 'unset' NOT NULL,
  CONSTRAINT sq_hard_w11_type_pk PRIMARY KEY (row_id)
)
TABLESPACE users
LOB (long_note) STORE AS SECUREFILE sq_hard_w11_note_lob
  (TABLESPACE users ENABLE STORAGE IN ROW CHUNK 8192 RETENTION AUTO
   NOCACHE LOGGING);

INSERT INTO sq_hard_w11_type (
  ansi_int, ansi_small, ansi_dec, ansi_numeric, ansi_double, ansi_real,
  ansi_float, rounded_num, char_semantic, byte_semantic, ansi_varying,
  national_txt, fixed_nchar, raw_bytes, local_tz, ds_span, ym_span,
  long_note, hidden_note
) VALUES (
  42, 7, 12.34, 5.678, 1.5E+2, 2.25, 3.5, 1234, 'char-wide', 'byte-wide',
  'varying', N'nchar-var', 'abc', HEXTORAW('DEADBEEF'),
  TIMESTAMP '2024-05-06 07:08:09.123',
  INTERVAL '3 04:05:06.70' DAY(4) TO SECOND(2),
  INTERVAL '2-6' YEAR(3) TO MONTH,
  TO_NCLOB('nclob payload'), NULL
);

INSERT INTO sq_hard_w11_type (ansi_int, ansi_small, rounded_num, char_semantic)
VALUES (43, 7, 4567, 'char-wide');
COMMIT;

SELECT t.row_id,
       t.rounded_num,
       t.hidden_note,
       RAWTOHEX(t.raw_bytes)              AS raw_hex,
       NVL2(t.local_tz, 'set', 'unset')   AS local_tz_state,
       TO_CHAR(t.ds_span)                 AS day_second_text,
       TO_CHAR(t.ym_span)                 AS year_month_text,
       DBMS_LOB.GETLENGTH(t.long_note)    AS nclob_len,
       LENGTHB(t.national_txt)            AS national_bytes
FROM sq_hard_w11_type t
ORDER BY t.row_id;

-- The dictionary is the proof that every ANSI alias resolved to a base type.
SELECT c.column_name,
       c.data_type,
       c.data_precision,
       c.data_scale,
       c.char_used
FROM user_tab_columns c
WHERE c.table_name = 'SQ_HARD_W11_TYPE'
  AND c.column_name IN ('ANSI_INT', 'ANSI_SMALL', 'ANSI_DEC', 'ANSI_NUMERIC',
                        'ANSI_DOUBLE', 'ANSI_REAL', 'ANSI_FLOAT', 'ROUNDED_NUM',
                        'CHAR_SEMANTIC', 'BYTE_SEMANTIC', 'ANSI_VARYING',
                        'NATIONAL_TXT')
ORDER BY c.column_name;

-- SELECT UNIQUE is DISTINCT's older spelling; 23ai also groups by a select-list
-- alias, so the same expression is written once instead of twice.
SELECT UNIQUE t.ansi_small AS small_value
FROM sq_hard_w11_type t;

SELECT SUBSTR(t.char_semantic, 1, 4) AS head_text,
       COUNT(*)                      AS row_count,
       SUM(t.rounded_num)            AS rounded_total
FROM sq_hard_w11_type t
GROUP BY head_text
ORDER BY head_text;

--------------------------------------------------------------------------------
-- W11-B: one CREATE SCHEMA statement carries a CREATE TABLE, a CREATE VIEW and
-- a GRANT with no terminator between them -- the splitter must not cut it. A
-- scalable sequence declares SCALE EXTEND (values gain an instance prefix), is
-- read back from the dictionary, then flipped to NOSCALE and RESTARTed so a
-- column DEFAULT can take its NEXTVAL deterministically.
--------------------------------------------------------------------------------
CREATE SCHEMA AUTHORIZATION system
  CREATE TABLE sq_hard_w11_ledger (
    entry_id NUMBER CONSTRAINT sq_hard_w11_ledger_pk PRIMARY KEY,
    amount   NUMBER(9, 2) DEFAULT 0 NOT NULL)
  CREATE VIEW sq_hard_w11_ledger_v AS
    SELECT entry_id, amount FROM sq_hard_w11_ledger WHERE amount >= 0
  GRANT SELECT ON sq_hard_w11_ledger TO PUBLIC;

CREATE SEQUENCE sq_hard_w11_seq
  START WITH 1000 INCREMENT BY 10 MINVALUE 1000 MAXVALUE 99999
  NOCYCLE CACHE 20 NOORDER NOKEEP SCALE EXTEND;

SELECT s.sequence_name,
       s.increment_by,
       s.cache_size,
       s.scale_flag,
       s.extend_flag,
       s.keep_value
FROM user_sequences s
WHERE s.sequence_name = 'SQ_HARD_W11_SEQ';

ALTER SEQUENCE sq_hard_w11_seq NOSCALE KEEP RESTART START WITH 1000;

CREATE TABLE sq_hard_w11_stamp (
  stamp_id NUMBER DEFAULT sq_hard_w11_seq.NEXTVAL
             CONSTRAINT sq_hard_w11_stamp_pk PRIMARY KEY,
  tag      VARCHAR2(20)
) TABLESPACE users;

INSERT INTO sq_hard_w11_stamp (tag) VALUES ('first');
INSERT INTO sq_hard_w11_stamp (tag) VALUES ('second');
COMMIT;

SELECT p.stamp_id, p.tag FROM sq_hard_w11_stamp p ORDER BY p.stamp_id;

INSERT INTO sq_hard_w11_ledger (entry_id, amount) VALUES (1, 10.50);
INSERT INTO sq_hard_w11_ledger (entry_id, amount) VALUES (2, -3.00);
INSERT INTO sq_hard_w11_ledger (entry_id, amount) VALUES (3, 7.25);
COMMIT;

--------------------------------------------------------------------------------
-- W11-C: optimizer-hint comments. A hint block spans six lines, names a query
-- block with @, nests parentheses, carries two levels of quoting inside
-- OPT_PARAM, holds a dash-comment-shaped run that is NOT a comment, and ends
-- with a hint the server does not know (silently ignored, still legal). Every
-- DML verb then takes its own hint, including APPEND_VALUES, which makes the
-- table unreadable until the COMMIT that follows it.
--------------------------------------------------------------------------------
SELECT /*+ QB_NAME(w11_main)
           LEADING(@w11_main l s)
           USE_NL(@w11_main s)
           INDEX(@w11_main l (sq_hard_w11_ledger.entry_id))
           OPT_PARAM('optimizer_dynamic_sampling' 3)
           OPT_PARAM('star_transformation_enabled' 'false')
           -- this dash run lives inside the hint, so it comments out nothing
           NO_PARALLEL MONITOR SQ_HARD_W11_NOT_A_HINT(zzz) */
       l.entry_id,
       s.tag
FROM sq_hard_w11_ledger l,
     sq_hard_w11_stamp s
WHERE l.entry_id = 1
  AND s.tag = 'first';

UPDATE /*+ INDEX(l sq_hard_w11_ledger_pk) NO_PARALLEL(l) */ sq_hard_w11_ledger l
SET l.amount = l.amount + 1
WHERE l.entry_id = 1;

DELETE /*+ FULL(l) */ FROM sq_hard_w11_ledger l
WHERE l.amount < 0;

INSERT /*+ APPEND_VALUES */ INTO sq_hard_w11_ledger (entry_id, amount)
VALUES (9, 9);
COMMIT;

MERGE /*+ USE_HASH(t s) LEADING(s) */ INTO sq_hard_w11_ledger t
USING (SELECT 2 AS entry_id, 20 AS amount FROM dual) s
ON (t.entry_id = s.entry_id)
WHEN MATCHED THEN UPDATE SET t.amount = s.amount
WHEN NOT MATCHED THEN INSERT (entry_id, amount) VALUES (s.entry_id, s.amount);
COMMIT;

SELECT --+ FULL(v) NO_INDEX(v)
       /* a plain comment immediately after a single-line hint */
       COUNT(*)    AS ledger_rows,
       SUM(amount) AS ledger_total
FROM sq_hard_w11_ledger_v v;

--------------------------------------------------------------------------------
-- W11-D: the PL/SQL type system. A range-constrained subtype and a NOT NULL
-- subtype anchor a record; the record fills an associative array; a strongly
-- typed ref cursor (RETURN <table>%ROWTYPE) is handed to a procedure IN OUT;
-- and the body drives the SYS utility packages -- DBMS_ASSERT, DBMS_UTILITY's
-- comma/table pair, UTL_RAW, a temporary LOB and a seeded DBMS_RANDOM.
--------------------------------------------------------------------------------
CREATE OR REPLACE PACKAGE sq_hard_w11_util_pkg AUTHID DEFINER AS
  SUBTYPE small_count IS PLS_INTEGER RANGE 0 .. 999;
  SUBTYPE nonnull_tag IS VARCHAR2(30) NOT NULL;

  TYPE tag_rec IS RECORD (
    tag    nonnull_tag := 'unset',
    score  small_count := 0,
    hashed NUMBER
  );
  TYPE tag_tab   IS TABLE OF tag_rec INDEX BY PLS_INTEGER;
  TYPE ledger_cur IS REF CURSOR RETURN sq_hard_w11_ledger%ROWTYPE;

  FUNCTION describe (probe IN VARCHAR2) RETURN VARCHAR2;
  FUNCTION seeded_bucket (seed_value IN BINARY_INTEGER) RETURN PLS_INTEGER;
  FUNCTION range_guard (probe IN NUMBER) RETURN VARCHAR2;
  PROCEDURE walk_ledger (src IN OUT ledger_cur, total OUT NUMBER,
                         seen OUT small_count);
END sq_hard_w11_util_pkg;
/

CREATE OR REPLACE PACKAGE BODY sq_hard_w11_util_pkg AS
  FUNCTION describe (probe IN VARCHAR2) RETURN VARCHAR2 IS
    tags     tag_tab;
    names    DBMS_UTILITY.uncl_array;
    name_cnt BINARY_INTEGER;
    joined   VARCHAR2(200);
    payload  RAW(64);
    fast     SIMPLE_INTEGER := 0;
    scratch  CLOB;
    lob_len  PLS_INTEGER;
  BEGIN
    DBMS_UTILITY.COMMA_TO_TABLE(probe, name_cnt, names);
    DBMS_UTILITY.TABLE_TO_COMMA(names, name_cnt, joined);

    FOR i IN 1 .. name_cnt LOOP
      tags(i).tag   := DBMS_ASSERT.SIMPLE_SQL_NAME(names(i));
      tags(i).score := i * 3;
      fast          := fast + tags(i).score;
      SELECT ORA_HASH(names(i), 1023, 7) INTO tags(i).hashed FROM dual;
    END LOOP;

    payload := UTL_RAW.CAST_TO_RAW(tags(tags.FIRST).tag);
    DBMS_LOB.CREATETEMPORARY(scratch, TRUE, DBMS_LOB.CALL);
    DBMS_LOB.WRITEAPPEND(scratch, LENGTH(joined), joined);
    lob_len := DBMS_LOB.GETLENGTH(scratch);
    DBMS_LOB.FREETEMPORARY(scratch);

    RETURN name_cnt
           || ':' || fast
           || ':' || lob_len
           || ':' || UTL_RAW.CAST_TO_VARCHAR2(UTL_RAW.REVERSE(payload))
           || ':' || DBMS_ASSERT.ENQUOTE_NAME(tags(tags.LAST).tag, FALSE);
  END describe;

  FUNCTION seeded_bucket (seed_value IN BINARY_INTEGER) RETURN PLS_INTEGER IS
  BEGIN
    DBMS_RANDOM.SEED(seed_value);
    RETURN TRUNC(DBMS_RANDOM.VALUE(1, 10));
  END seeded_bucket;

  FUNCTION range_guard (probe IN NUMBER) RETURN VARCHAR2 IS
    bounded small_count;
  BEGIN
    bounded := probe;
    RETURN 'accepted ' || bounded;
  EXCEPTION
    WHEN VALUE_ERROR THEN
      RETURN CASE
               WHEN DBMS_UTILITY.FORMAT_ERROR_BACKTRACE IS NOT NULL
                    AND DBMS_UTILITY.FORMAT_ERROR_STACK LIKE '%ORA-06502%'
               THEN 'range-checked'
               ELSE 'unexpected'
             END;
  END range_guard;

  PROCEDURE walk_ledger (src IN OUT ledger_cur, total OUT NUMBER,
                         seen OUT small_count) IS
    row_rec sq_hard_w11_ledger%ROWTYPE;
  BEGIN
    total := 0;
    seen  := 0;
    LOOP
      FETCH src INTO row_rec;
      EXIT WHEN src%NOTFOUND;
      total := total + row_rec.amount;
      seen  := seen + 1;
    END LOOP;
    CLOSE src;
  END walk_ledger;
END sq_hard_w11_util_pkg;
/

DECLARE
  described  VARCHAR2(200);
  guarded    VARCHAR2(60);
  bucket     PLS_INTEGER;
  cur        sq_hard_w11_util_pkg.ledger_cur;
  total      NUMBER;
  seen       sq_hard_w11_util_pkg.small_count;
BEGIN
  described := sq_hard_w11_util_pkg.describe('alpha,beta,gamma');
  guarded   := sq_hard_w11_util_pkg.range_guard(1000);
  bucket    := sq_hard_w11_util_pkg.seeded_bucket(4242);

  OPEN cur FOR SELECT * FROM sq_hard_w11_ledger ORDER BY entry_id;
  sq_hard_w11_util_pkg.walk_ledger(cur, total, seen);

  DBMS_OUTPUT.PUT_LINE('w11 describe ' || described);
  DBMS_OUTPUT.PUT_LINE('w11 guard ' || guarded);

  INSERT INTO sq_hard_w11_note (note_key, note_text, note_value)
  VALUES ('subtypes', described || '/' || guarded, seen);

  INSERT INTO sq_hard_w11_note (note_key, note_text, note_value)
  VALUES ('ledger',
          CASE WHEN bucket BETWEEN 1 AND 9 THEN 'seeded bucket in range'
               ELSE 'seeded bucket out of range' END,
          total);
  COMMIT;
END;
/

--------------------------------------------------------------------------------
-- W11-E: client and lexer round 6. SET NULL / SET COLSEP / SET TAB steer the
-- report, SET DEFINE hands substitution to '^' so '&' is plain text, a string
-- literal carries a block-comment terminator, a quoted identifier is all
-- digits, and a division operator opens the continuation line -- one character
-- away from the bare slash that would end the statement.
--------------------------------------------------------------------------------
SET NULL '<null>'
SET COLSEP '|'
SET TAB OFF
SET VERIFY OFF

SELECT '*/ not the end of a comment'  AS star_slash,
       1                              AS "123",
       2                              AS "back\slash%",
       ''                             AS empty_is_null,
       NULL                           AS explicit_null
FROM dual;

SELECT 100
/ 4                                   AS quartered,
       LENGTH(q'[a slash / inside]')  AS slash_literal_len
FROM dual;

SET NULL ''
SET COLSEP ' '
SET TAB ON

SET DEFINE ^
DEFINE w11_tag = caret_substituted
SELECT '^w11_tag'                        AS caret_value,
       'a && b & c is literal here'      AS ampersand_literal,
       LENGTH('a && b & c is literal here') AS ampersand_len
FROM dual;
UNDEFINE w11_tag
SET DEFINE OFF

DECLARE
  star_len  PLS_INTEGER;
  slash_len PLS_INTEGER;
BEGIN
  SELECT LENGTH('*/ not the end of a comment') INTO star_len FROM dual;
  slash_len := LENGTH(q'[/ still a literal /]');

  INSERT INTO sq_hard_w11_note (note_key, note_text, note_value)
  VALUES ('lexer6', 'star-slash literal and slash q-quote',
          star_len + slash_len);
  COMMIT;
END;
/

--------------------------------------------------------------------------------
-- Wave-11 self-verification.
--------------------------------------------------------------------------------
DECLARE
  note_rows   PLS_INTEGER;
  described   VARCHAR2(200);
  guard_text  VARCHAR2(60);
  ledger_sum  NUMBER;
  ledger_rows PLS_INTEGER;
  seen_rows   NUMBER;
  lexer_len   NUMBER;
  stamp_ids   VARCHAR2(40);
  type_rows   PLS_INTEGER;
  identity_hi NUMBER;
  scale_flag  VARCHAR2(3);
  grant_cnt   PLS_INTEGER;
BEGIN
  SELECT COUNT(*) INTO note_rows FROM sq_hard_w11_note;
  SELECT note_text, note_value INTO described, seen_rows
  FROM sq_hard_w11_note WHERE note_key = 'subtypes';
  SELECT note_value INTO ledger_sum
  FROM sq_hard_w11_note WHERE note_key = 'ledger';
  SELECT note_value INTO lexer_len
  FROM sq_hard_w11_note WHERE note_key = 'lexer6';
  SELECT COUNT(*) INTO ledger_rows FROM sq_hard_w11_ledger_v;
  SELECT LISTAGG(stamp_id, '/') WITHIN GROUP (ORDER BY stamp_id)
  INTO stamp_ids FROM sq_hard_w11_stamp;
  SELECT COUNT(*), MAX(row_id) INTO type_rows, identity_hi
  FROM sq_hard_w11_type;
  SELECT s.scale_flag INTO scale_flag
  FROM user_sequences s WHERE s.sequence_name = 'SQ_HARD_W11_SEQ';
  SELECT COUNT(*) INTO grant_cnt
  FROM user_tab_privs
  WHERE table_name = 'SQ_HARD_W11_LEDGER' AND grantee = 'PUBLIC';

  guard_text := SUBSTR(described, INSTR(described, '/') + 1);

  IF note_rows <> 3 THEN
    RAISE_APPLICATION_ERROR(-20930, 'w11 note rows ' || note_rows);
  END IF;
  IF described NOT LIKE '3:18:16:ahpla:"gamma"%' THEN
    RAISE_APPLICATION_ERROR(-20931, 'w11 describe ' || described);
  END IF;
  IF guard_text <> 'range-checked' THEN
    RAISE_APPLICATION_ERROR(-20932, 'w11 range guard ' || guard_text);
  END IF;
  IF ledger_sum <> 47.75 OR ledger_rows <> 4 OR seen_rows <> 4 THEN
    RAISE_APPLICATION_ERROR(-20933,
      'w11 ledger ' || ledger_sum || '/' || ledger_rows || '/' || seen_rows);
  END IF;
  IF stamp_ids <> '1000/1010' THEN
    RAISE_APPLICATION_ERROR(-20934, 'w11 stamp ids ' || stamp_ids);
  END IF;
  IF type_rows <> 2 OR identity_hi <> 7 THEN
    RAISE_APPLICATION_ERROR(-20935,
      'w11 type rows ' || type_rows || '/' || identity_hi);
  END IF;
  IF scale_flag <> 'N' THEN
    RAISE_APPLICATION_ERROR(-20936, 'w11 scale flag ' || scale_flag);
  END IF;
  IF grant_cnt < 1 THEN
    RAISE_APPLICATION_ERROR(-20937, 'w11 public grant missing');
  END IF;
  IF lexer_len <> 46 THEN
    RAISE_APPLICATION_ERROR(-20938, 'w11 lexer length ' || lexer_len);
  END IF;
END;
/

--------------------------------------------------------------------------------
-- ULTRA WAVE 12 -- the name-collision surface. Every column, alias, package
-- member, record field, cursor parameter, block label and bind below is
-- deliberately spelled like a keyword, a builtin function or a statement verb,
-- and is then used in the exact slot where the parser expects the grammar
-- meaning: an alias named OVER carrying an analytic OVER clause, a column named
-- PARTITION inside PARTITION BY, SUM(sum), LEFT JOIN over a column named LEFT,
-- FROM dual beside a column named DUAL, and quoted identifiers spelled "*",
-- "&", ":bind", "END;", "--" and "/*+ full */".
--------------------------------------------------------------------------------
CREATE TABLE sq_hard_w12_note (
  note_key   VARCHAR2(30) CONSTRAINT sq_hard_w12_note_pk PRIMARY KEY,
  note_text  VARCHAR2(200),
  note_value NUMBER
) TABLESPACE users;

--------------------------------------------------------------------------------
-- W12-A: the keyword column battery. Thirty-one columns named after clause
-- words; only ROWS needs quoting because Oracle reserves it. The queries below
-- then put each one back into its own grammatical slot.
--------------------------------------------------------------------------------
CREATE TABLE sq_hard_w12_kw (
  type      NUMBER CONSTRAINT sq_hard_w12_kw_pk PRIMARY KEY,
  value     NUMBER,
  first     NUMBER,
  last      NUMBER,
  only      NUMBER,
  nulls     NUMBER,
  over      NUMBER,
  partition NUMBER,
  "rows"    NUMBER,
  range     NUMBER,
  preceding NUMBER,
  following NUMBER,
  unbounded NUMBER,
  merge     NUMBER,
  pivot     NUMBER,
  model     NUMBER,
  year      NUMBER,
  month     NUMBER,
  body      VARCHAR2(20),
  role      VARCHAR2(20),
  profile   VARCHAR2(20),
  source    VARCHAR2(20),
  matched   NUMBER DEFAULT 0,
  errors    NUMBER DEFAULT 0,
  limit     NUMBER,
  cache     NUMBER,
  cycle     NUMBER,
  at        NUMBER,
  wait      NUMBER,
  skip      NUMBER,
  locked    NUMBER
) TABLESPACE users;

INSERT INTO sq_hard_w12_kw (
  type, value, first, last, only, nulls, over, partition, "rows", range,
  preceding, following, unbounded, merge, pivot, model, year, month, body,
  role, profile, source, limit, cache, cycle, at, wait, skip, locked
) VALUES (
  1, 10, 1, 3, 1, 0, 5, 7, 3, 4, 1, 2, 0, 1, 1, 1, 2024, 5, 'body-one',
  'role-a', 'prof-a', 'src-a', 10, 1, 0, NULL, 1, 0, 0
);

INSERT INTO sq_hard_w12_kw (
  type, value, first, last, only, nulls, over, partition, "rows", range,
  preceding, following, unbounded, merge, pivot, model, year, month, body,
  role, profile, source, limit, cache, cycle, at, wait, skip, locked
) VALUES (
  2, 20, 2, 2, 1, 0, 6, 7, 4, 5, 1, 2, 0, 1, 2, 1, 2024, 6, 'body-two',
  'role-b', 'prof-b', 'src-b', 20, 1, 0, 1, 1, 0, 0
);

INSERT INTO sq_hard_w12_kw (
  type, value, first, last, only, nulls, over, partition, "rows", range,
  preceding, following, unbounded, merge, pivot, model, year, month, body,
  role, profile, source, limit, cache, cycle, at, wait, skip, locked
) VALUES (
  3, 30, 3, 1, 0, 1, 7, 8, 5, 6, 1, 2, 0, 2, 1, 2, 2025, 7, 'body-three',
  'role-c', 'prof-c', 'src-c', 30, 1, 0, 2, 1, 0, 0
);
COMMIT;

-- Window frames spelled out of columns with the same names as the frame words.
SELECT k.partition,
       k.type,
       SUM(k.value) OVER (PARTITION BY k.partition
                          ORDER BY k."rows"
                          ROWS BETWEEN UNBOUNDED PRECEDING
                               AND CURRENT ROW)              AS running_value,
       COUNT(*) OVER (ORDER BY k.type
                      RANGE BETWEEN CURRENT ROW
                            AND UNBOUNDED FOLLOWING)          AS tail_rows,
       FIRST_VALUE(k.first) OVER (ORDER BY k.last DESC)       AS first_of_last,
       LAST_VALUE(k.last) OVER (ORDER BY k.first
                                ROWS BETWEEN k.preceding PRECEDING
                                     AND k.following FOLLOWING) AS last_of_first
FROM sq_hard_w12_kw k
ORDER BY k.first NULLS FIRST, k.last DESC NULLS LAST
FETCH FIRST 3 ROWS ONLY;

-- The relation alias is OVER, and the same statement carries a real OVER clause.
SELECT over.type,
       over.value,
       over.only,
       SUM(over.value) OVER (PARTITION BY over.partition)   AS partition_total,
       COUNT(*) OVER ()                                     AS over_rows
FROM sq_hard_w12_kw over
WHERE over.only = 1
ORDER BY over.type;

-- MERGE whose SET target is called MATCHED, with a hint and a DELETE branch.
MERGE /*+ USE_NL(k) */ INTO sq_hard_w12_kw k
USING (SELECT 1 AS type, 100 AS value FROM dual UNION ALL
       SELECT 4 AS type, 40 AS value FROM dual) source
ON (k.type = source.type)
WHEN MATCHED THEN UPDATE SET k.matched = k.matched + 1
  DELETE WHERE k.value > 1000
WHEN NOT MATCHED THEN INSERT (k.type, k.value, k.first, k.last, k.partition,
                              k."rows", k.at)
  VALUES (source.type, source.value, 4, 4, 8, 6, 3);
COMMIT;

-- CONNECT BY where the parent link is a column named AT and the level column is
-- called TYPE; ORDER SIBLINGS BY closes it.
SELECT LEVEL                                          AS depth,
       k.type,
       k.at,
       SYS_CONNECT_BY_PATH(k.type, '/')               AS type_path,
       CONNECT_BY_ISLEAF                              AS leaf_flag
FROM sq_hard_w12_kw k
START WITH k.at IS NULL
CONNECT BY PRIOR k.type = k.at
ORDER SIBLINGS BY k.type;

-- PIVOT driven by a column named PIVOT, GROUP BY ROLLUP over PARTITION.
SELECT * FROM (SELECT k.type, k.value, k.pivot FROM sq_hard_w12_kw k)
PIVOT (SUM(value) AS total FOR pivot IN (1 AS p1, 2 AS p2))
ORDER BY type;

SELECT k.partition,
       k.type,
       SUM(k.value)                                   AS total,
       GROUPING(k.type)                               AS type_grouping
FROM sq_hard_w12_kw k
GROUP BY ROLLUP (k.partition, k.type)
HAVING SUM(k.value) > 0
ORDER BY k.partition NULLS LAST, k.type NULLS LAST;

-- Row locking whose OF list names the columns WAIT, SKIP and LOCKED.
SELECT k.value
FROM sq_hard_w12_kw k
WHERE k.type = 1
FOR UPDATE OF k.wait, k.skip WAIT 1;
ROLLBACK;

SELECT k.value
FROM sq_hard_w12_kw k
WHERE k.type = 2
FOR UPDATE OF k.locked SKIP LOCKED;
ROLLBACK;

--------------------------------------------------------------------------------
-- W12-B: builtin-named columns handed to the builtin of the same name, plus a
-- LEFT OUTER JOIN whose predicate is built from columns called LEFT, RIGHT,
-- FULL, CROSS, INNER and OUTER, and a FROM list that mixes DUAL with a column
-- named DUAL.
--------------------------------------------------------------------------------
CREATE TABLE sq_hard_w12_fn (
  count     NUMBER,
  sum       NUMBER,
  length    NUMBER,
  mod       NUMBER,
  trim      VARCHAR2(10),
  nvl       VARCHAR2(10),
  decode    NUMBER,
  abs       NUMBER,
  sign      NUMBER,
  power     NUMBER,
  replace   VARCHAR2(10),
  translate VARCHAR2(10),
  extract   NUMBER,
  cast      NUMBER,
  treat     NUMBER,
  using     NUMBER,
  natural   NUMBER,
  cross     NUMBER,
  outer     NUMBER,
  inner     NUMBER,
  full      NUMBER,
  left      NUMBER,
  right     NUMBER,
  dual      NUMBER,
  key       NUMBER,
  json      NUMBER,
  vector    NUMBER,
  timestamp NUMBER
) TABLESPACE users;

INSERT INTO sq_hard_w12_fn (
  count, sum, length, mod, trim, nvl, decode, abs, sign, power, replace,
  translate, extract, cast, treat, using, natural, cross, outer, inner, full,
  left, right, dual, key, json, vector, timestamp
) VALUES (
  3, 4, 5, 7, '  pad  ', NULL, 9, -2, -1, 2, 'aa', 'bb', 2024, 1, 1, 1, 1, 1,
  1, 1, 1, 1, 2, 1, 1, 1, 1, 1
);

INSERT INTO sq_hard_w12_fn (count, sum, left, right, full, cross, natural,
                            outer, inner, dual, trim)
VALUES (3, 40, 2, 3, 1, 1, 1, 2, 2, 1, ' pad ');
COMMIT;

SELECT SUM(sum)                                AS sum,
       COUNT(count)                            AS count,
       LENGTH(TRIM(MIN(trim)))                  AS length,
       MOD(SUM(mod), 3)                         AS mod,
       NVL(MIN(nvl), 'fallback')                AS nvl,
       DECODE(MIN(decode), 9, 'nine', 'other')  AS decode,
       ABS(MIN(abs))                            AS abs,
       SIGN(MIN(sign))                          AS sign,
       POWER(MAX(power), 2)                     AS power,
       REPLACE(MAX(replace), 'a', 'z')          AS replace,
       CAST(MAX(cast) AS VARCHAR2(5))           AS cast
FROM sq_hard_w12_fn;

SELECT count, COUNT(*) AS how_many, SUM(sum) AS summed
FROM sq_hard_w12_fn
GROUP BY count
HAVING COUNT(*) >= 1
ORDER BY count;

SELECT a.left,
       a.right,
       b.outer,
       b.inner,
       a.full + a.cross                         AS join_flags
FROM sq_hard_w12_fn a
LEFT OUTER JOIN sq_hard_w12_fn b ON a.left = b.right
                                AND a.full = b.cross
WHERE a.natural = 1
ORDER BY a.left, a.right NULLS LAST;

SELECT f.dual, d.dummy, f.json + f.vector + f.timestamp AS type_named_sum
FROM sq_hard_w12_fn f, dual d
WHERE f.dual = 1 AND d.dummy = 'X' AND f.json IS NOT NULL
ORDER BY f.sum;

--------------------------------------------------------------------------------
-- W12-C: quoted identifiers that are punctuation, comment introducers, bind
-- shapes and statement tails. The table name itself is quoted lower case, so
-- every reference must keep its quotes.
--------------------------------------------------------------------------------
CREATE TABLE "sq_hard_w12_select" (
  "select"      NUMBER,
  "where"       NUMBER,
  "group by"    NUMBER,
  "*"           NUMBER,
  "|"           NUMBER,
  "&"           NUMBER,
  ":bind"       NUMBER,
  "7up"         NUMBER,
  "END;"        NUMBER,
  "--"          NUMBER,
  "/*+ full */" NUMBER
) TABLESPACE users;

INSERT INTO "sq_hard_w12_select" (
  "select", "where", "group by", "*", "|", "&", ":bind", "7up", "END;", "--",
  "/*+ full */"
) VALUES (1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11);
COMMIT;

SELECT s."select" + s."*"                       AS "star sum",
       s."|" || '/' || s."&"                    AS "|piped|",
       s.":bind"                                AS ":x",
       s."END;"                                 AS "END;",
       s."--"                                   AS "--",
       s."/*+ full */"                          AS "/*+ full */",
       s."7up" * s."group by"                   AS "1"
FROM "sq_hard_w12_select" s
WHERE s."where" = 2
  AND s."--" > 0
GROUP BY s."select", s."*", s."|", s."&", s.":bind", s."END;", s."--",
         s."/*+ full */", s."7up", s."group by"
ORDER BY "star sum", "1";

--------------------------------------------------------------------------------
-- W12-D: PL/SQL name capture. A package declares a record type named VALUE with
-- a field named VALUE, members named after builtins, and a cursor whose
-- parameter is also VALUE; a standalone function takes a parameter named after
-- the table it queries and disambiguates with its own name; a labelled block
-- qualifies a variable that a column of the same name would otherwise capture.
--------------------------------------------------------------------------------
CREATE OR REPLACE PACKAGE sq_hard_w12_pkg AS
  TYPE value IS RECORD (
    value NUMBER,
    type  VARCHAR2(12)
  );

  FUNCTION to_char (value NUMBER) RETURN VARCHAR2;
  FUNCTION count (value NUMBER) RETURN NUMBER;
  FUNCTION length (value NUMBER) RETURN NUMBER;
  FUNCTION trim (value VARCHAR2) RETURN VARCHAR2;
  FUNCTION decode (value NUMBER) RETURN VARCHAR2;
  FUNCTION extract (value NUMBER) RETURN NUMBER;
  FUNCTION treat (value NUMBER) RETURN NUMBER;
  FUNCTION describe (row_in value) RETURN VARCHAR2;
END sq_hard_w12_pkg;
/

CREATE OR REPLACE PACKAGE BODY sq_hard_w12_pkg AS
  FUNCTION to_char (value NUMBER) RETURN VARCHAR2 IS
  BEGIN
    RETURN 'n=' || value;
  END to_char;

  FUNCTION count (value NUMBER) RETURN NUMBER IS
  BEGIN
    RETURN value + 1;
  END count;

  FUNCTION length (value NUMBER) RETURN NUMBER IS
  BEGIN
    RETURN value * 2;
  END length;

  FUNCTION trim (value VARCHAR2) RETURN VARCHAR2 IS
  BEGIN
    RETURN '[' || value || ']';
  END trim;

  FUNCTION decode (value NUMBER) RETURN VARCHAR2 IS
  BEGIN
    RETURN CASE value WHEN 9 THEN 'nine' ELSE 'other' END;
  END decode;

  FUNCTION extract (value NUMBER) RETURN NUMBER IS
  BEGIN
    RETURN MOD(value, 10);
  END extract;

  FUNCTION treat (value NUMBER) RETURN NUMBER IS
  BEGIN
    RETURN GREATEST(value, 0);
  END treat;

  FUNCTION describe (row_in value) RETURN VARCHAR2 IS
    copy_row sq_hard_w12_pkg.value := row_in;
  BEGIN
    RETURN sq_hard_w12_pkg.to_char(copy_row.value) || ':' || copy_row.type;
  END describe;
END sq_hard_w12_pkg;
/

SELECT sq_hard_w12_pkg.to_char(1)  AS to_char,
       sq_hard_w12_pkg.count(1)    AS count,
       sq_hard_w12_pkg.length(3)   AS length,
       sq_hard_w12_pkg.trim(' x ') AS trim,
       sq_hard_w12_pkg.decode(9)   AS decode,
       sq_hard_w12_pkg.extract(24) AS extract,
       sq_hard_w12_pkg.treat(-5)   AS treat
FROM dual;

CREATE OR REPLACE FUNCTION sq_hard_w12_shadow (sq_hard_w12_kw NUMBER)
  RETURN NUMBER
IS
  value NUMBER;
BEGIN
  SELECT COUNT(*)
  INTO   value
  FROM   sq_hard_w12_kw
  WHERE  sq_hard_w12_kw.type >= sq_hard_w12_shadow.sq_hard_w12_kw;

  RETURN value;
END sq_hard_w12_shadow;
/

SELECT sq_hard_w12_shadow(0) AS shadowed_all,
       sq_hard_w12_shadow(3) AS shadowed_tail
FROM dual;

<<sq_hard_w12_outer>>
DECLARE
  value      NUMBER := 0;
  described  VARCHAR2(60);
  TYPE type IS TABLE OF NUMBER INDEX BY PLS_INTEGER;
  type_bag   type;
  -- The cursor parameter is called VALUE, so inside the cursor's own SQL the
  -- predicate 'k.value >= value' captures the COLUMN, not the parameter: the
  -- fetched row is the lowest value in the table, and that is the proof.
  CURSOR value_cur (value NUMBER) IS
    SELECT k.value, 'kw' AS type
    FROM   sq_hard_w12_kw k
    WHERE  k.value >= value
    ORDER  BY k.value;
  row_out    sq_hard_w12_pkg.value;
BEGIN
  SELECT MAX(value)
  INTO   sq_hard_w12_outer.value
  FROM   sq_hard_w12_kw;

  OPEN value_cur (value => sq_hard_w12_outer.value);
  FETCH value_cur INTO row_out;
  CLOSE value_cur;

  described := sq_hard_w12_pkg.describe(row_out);
  type_bag(1) := sq_hard_w12_outer.value;

  <<sq_hard_w12_inner>>
  DECLARE
    value NUMBER := sq_hard_w12_outer.value + 1;
  BEGIN
    type_bag(2) := sq_hard_w12_inner.value;
    DBMS_OUTPUT.PUT_LINE('[w12] captured '
                         || sq_hard_w12_outer.value || '/'
                         || sq_hard_w12_inner.value);
  END sq_hard_w12_inner;

  INSERT INTO sq_hard_w12_note (note_key, note_text, note_value)
  VALUES ('capture', described || '|' || type_bag(2),
          type_bag(1) + type_bag(2));
  COMMIT;
END sq_hard_w12_outer;
/

--------------------------------------------------------------------------------
-- W12-E: the set-operator precedence tower. Oracle gives INTERSECT and
-- MINUS/EXCEPT the same precedence and evaluates them left to right, so the
-- first tower collapses to a single row -- the projected value is the proof.
-- EXCEPT and EXCEPT ALL are the 23ai spellings of MINUS and MINUS ALL.
--------------------------------------------------------------------------------
SELECT LISTAGG(n, '/') WITHIN GROUP (ORDER BY n) AS left_to_right_shape
FROM (
  SELECT n
  FROM (SELECT 1 AS n FROM dual UNION ALL
        SELECT 2 FROM dual UNION ALL
        SELECT 3 FROM dual)
  MINUS ALL
  SELECT 2 FROM dual
  INTERSECT
  SELECT 2 FROM dual
  UNION ALL
  SELECT 9 FROM dual
);

SELECT LISTAGG(n, '/') WITHIN GROUP (ORDER BY n) AS parenthesised_shape
FROM (
  (SELECT n
   FROM (SELECT 1 AS n FROM dual UNION ALL
         SELECT 2 FROM dual UNION ALL
         SELECT 3 FROM dual)
   MINUS ALL
   SELECT 2 FROM dual)
  INTERSECT
  (SELECT 1 AS n FROM dual UNION ALL SELECT 3 FROM dual)
);

SELECT n AS except_shape
FROM (SELECT 1 AS n FROM dual EXCEPT SELECT 2 FROM dual);

SELECT LISTAGG(n, '/') WITHIN GROUP (ORDER BY n) AS except_all_shape
FROM (
  SELECT 1 AS n FROM dual UNION ALL SELECT 1 FROM dual UNION ALL
  SELECT 5 FROM dual
  EXCEPT ALL
  SELECT 1 FROM dual
);

-- Parenthesised branches carrying their own ORDER BY, OFFSET and FETCH.
(SELECT k.type AS branch_type FROM sq_hard_w12_kw k ORDER BY k.type
 FETCH FIRST 1 ROWS ONLY)
UNION ALL
(SELECT k.type FROM sq_hard_w12_kw k ORDER BY k.type DESC
 OFFSET 1 ROWS FETCH NEXT 1 ROWS ONLY)
ORDER BY 1;

-- A WITH clause nested inside a correlated EXISTS, and set operators inside IN.
WITH base AS (
  SELECT k.type AS n FROM sq_hard_w12_kw k
)
SELECT b.n AS nested_with_n
FROM base b
WHERE EXISTS (WITH inner_cte AS (SELECT 2 AS n FROM dual UNION ALL
                                 SELECT 3 FROM dual)
              SELECT 1 FROM inner_cte i WHERE i.n = b.n)
  AND b.n IN (SELECT 2 FROM dual UNION ALL SELECT 3 FROM dual
              MINUS SELECT 4 FROM dual)
ORDER BY b.n;

--------------------------------------------------------------------------------
-- W12-F: 23ai declaration tails that only appear once in a script. A JSON
-- column validated against an inline schema, DEFAULT ON NULL FOR INSERT AND
-- UPDATE (the update half is 23ai), and both BEQUEATH spellings on a view.
--------------------------------------------------------------------------------
CREATE TABLE sq_hard_w12_json (
  doc_id NUMBER GENERATED ALWAYS AS IDENTITY,
  doc    JSON VALIDATE '{"type" : "object", "required" : ["kind"]}',
  CONSTRAINT sq_hard_w12_json_pk PRIMARY KEY (doc_id)
) TABLESPACE users;

INSERT INTO sq_hard_w12_json (doc)
VALUES (JSON('{"kind" : "note", "tags" : ["a", "b"]}'));
COMMIT;

SELECT j.doc.kind.string()                                    AS doc_kind,
       DBMS_JSON_SCHEMA.IS_VALID(j.doc,
         JSON('{"type" : "object", "required" : ["kind"]}'))   AS schema_valid,
       JSON_VALUE(DBMS_JSON_SCHEMA.VALIDATE_REPORT(j.doc,
         JSON('{"type" : "object", "required" : ["kind"]}')),
         '$.valid')                                            AS report_valid
FROM sq_hard_w12_json j;

CREATE TABLE sq_hard_w12_don (
  don_id NUMBER,
  tag    VARCHAR2(12) DEFAULT ON NULL FOR INSERT AND UPDATE 'auto' NOT NULL,
  seeded VARCHAR2(12) DEFAULT ON NULL 'insert-only' NOT NULL
) TABLESPACE users;

INSERT INTO sq_hard_w12_don (don_id, tag, seeded) VALUES (1, NULL, NULL);
UPDATE sq_hard_w12_don SET tag = NULL WHERE don_id = 1;
COMMIT;

SELECT don_id, tag, seeded FROM sq_hard_w12_don ORDER BY don_id;

CREATE OR REPLACE VIEW sq_hard_w12_bequeath_v BEQUEATH CURRENT_USER AS
SELECT k.type, k.value, k.matched
FROM sq_hard_w12_kw k
WHERE k.value >= 10
WITH READ ONLY;

CREATE OR REPLACE EDITIONABLE VIEW sq_hard_w12_definer_v BEQUEATH DEFINER AS
SELECT f.left, f.right, f.dual
FROM sq_hard_w12_fn f;

SELECT (SELECT COUNT(*) FROM sq_hard_w12_bequeath_v) AS bequeath_rows,
       (SELECT COUNT(*) FROM sq_hard_w12_definer_v)  AS definer_rows
FROM dual;

--------------------------------------------------------------------------------
-- W12-G: dynamic SQL whose payload is a whole PL/SQL block. The q-quoted texts
-- carry statement terminators, a nested block comment and bind placeholders
-- named after keywords, and the driver mixes BULK COLLECT INTO, RETURNING INTO
-- and USING IN OUT on one anonymous block.
--------------------------------------------------------------------------------
DECLARE
  dyn_text   VARCHAR2(4000);
  dyn_values SYS.ODCINUMBERLIST;
  bulk_rows  PLS_INTEGER;
  returned   NUMBER;
  counter    NUMBER := 5;
BEGIN
  dyn_text := q'{SELECT k.value FROM sq_hard_w12_kw k
                 WHERE k.type > :type ORDER BY k.value}';
  EXECUTE IMMEDIATE dyn_text BULK COLLECT INTO dyn_values USING 0;
  bulk_rows := dyn_values.COUNT;

  EXECUTE IMMEDIATE 'UPDATE sq_hard_w12_kw SET errors = errors + 1 '
                    || 'WHERE type = :type RETURNING errors INTO :errors'
    USING 1
    RETURNING INTO returned;

  EXECUTE IMMEDIATE q'{BEGIN /* a nested block; with a comment */
                         :counter := :counter * 2;
                       END;}'
    USING IN OUT counter;

  EXECUTE IMMEDIATE q'{BEGIN NULL; /* END; */ END;}';

  INSERT INTO sq_hard_w12_note (note_key, note_text, note_value)
  VALUES ('dynamic',
          'bulk=' || bulk_rows || ' returning=' || returned
          || ' inout=' || counter,
          bulk_rows * 100 + returned * 10 + counter);
  COMMIT;
END;
/

--------------------------------------------------------------------------------
-- W12-H: client and lexer round 7. SET HEADSEP splits a COLUMN heading, a
-- number format carries its own commas, SET CONCAT ends a substitution name
-- early, SET ESCAPE hides an ampersand, and the last query packs a dash comment
-- carrying */, a block comment carrying an apostrophe and a semicolon, a
-- q-quoted anonymous block, a bare terminator inside a string and an unspaced
-- boolean expression into one statement.
--------------------------------------------------------------------------------
SET HEADSEP |
COLUMN w12_wrapped FORMAT A12 HEADING 'left|right' WRAP
COLUMN w12_amount FORMAT 999,990.00 JUSTIFY LEFT
SELECT 'abcdefghijklmnop' AS w12_wrapped, 1234.5 AS w12_amount FROM dual;
CLEAR COLUMNS
SET HEADSEP ON

SET DEFINE &
SET CONCAT #
DEFINE w12_tag = concat_split
SELECT '&w12_tag#-suffix' AS concatenated FROM dual;
UNDEFINE w12_tag
SET CONCAT .
SET DEFINE OFF

SET ESCAPE \
SELECT 'a\&b' AS escaped_ampersand FROM dual;
SET ESCAPE OFF

REM the next statement is one line; this REM ends with a semicolon;
SELECT (((((((((((((((((((((((((((((1))))))))))))))))))))))))))))) AS paren_tower FROM dual;

SELECT 'has */ inside'          AS star_slash, -- dash comment with ' and */
       'q''uoted'               AS doubled_quote, /* block with ' and -- and ; */
       q'{BEGIN NULL; END;}'    AS embedded_block,
       'END; / commit;'         AS terminator_bait,
       LENGTH('has */ inside') + LENGTH('END; / commit;') AS lexer_length,
       1 AS a,2 AS b,3 AS c
FROM dual WHERE 1=1 AND(2=2)OR(3=3)AND NOT(4=5);

--------------------------------------------------------------------------------
-- Wave-12 self-verification.
--------------------------------------------------------------------------------
DECLARE
  kw_rows        PLS_INTEGER;
  kw_total       NUMBER;
  matched_one    NUMBER;
  errors_one     NUMBER;
  type_path      VARCHAR2(60);
  fn_sum         NUMBER;
  fn_join_rows   PLS_INTEGER;
  quoted_sum     NUMBER;
  pkg_shape      VARCHAR2(120);
  shadow_all     NUMBER;
  shadow_tail    NUMBER;
  capture_note   VARCHAR2(200);
  capture_value  NUMBER;
  tower_shape    VARCHAR2(60);
  paren_shape    VARCHAR2(60);
  except_shape   VARCHAR2(60);
  schema_valid   NUMBER;
  don_tag        VARCHAR2(12);
  don_seeded     VARCHAR2(12);
  bequeath_rows  PLS_INTEGER;
  dyn_note       VARCHAR2(200);
  dyn_value      NUMBER;
BEGIN
  SELECT COUNT(*), SUM(value) INTO kw_rows, kw_total FROM sq_hard_w12_kw;

  SELECT matched, errors
  INTO   matched_one, errors_one
  FROM   sq_hard_w12_kw
  WHERE  type = 1;

  SELECT MAX(SYS_CONNECT_BY_PATH(k.type, '/'))
  INTO   type_path
  FROM   sq_hard_w12_kw k
  START WITH k.at IS NULL
  CONNECT BY PRIOR k.type = k.at;

  SELECT SUM(sum) INTO fn_sum FROM sq_hard_w12_fn;

  SELECT COUNT(*)
  INTO   fn_join_rows
  FROM   sq_hard_w12_fn a
  LEFT OUTER JOIN sq_hard_w12_fn b ON a.left = b.right AND a.full = b.cross
  WHERE  a.natural = 1;

  SELECT s."select" + s."*" + s."END;" + s."/*+ full */"
  INTO   quoted_sum
  FROM   "sq_hard_w12_select" s;

  pkg_shape := sq_hard_w12_pkg.to_char(1) || '|'
               || sq_hard_w12_pkg.count(1) || '|'
               || sq_hard_w12_pkg.length(3) || '|'
               || sq_hard_w12_pkg.trim(' x ') || '|'
               || sq_hard_w12_pkg.decode(9) || '|'
               || sq_hard_w12_pkg.extract(24) || '|'
               || sq_hard_w12_pkg.treat(-5);

  shadow_all  := sq_hard_w12_shadow(0);
  shadow_tail := sq_hard_w12_shadow(3);

  SELECT note_text, note_value
  INTO   capture_note, capture_value
  FROM   sq_hard_w12_note
  WHERE  note_key = 'capture';

  SELECT LISTAGG(n, '/') WITHIN GROUP (ORDER BY n)
  INTO   tower_shape
  FROM   (SELECT n
          FROM (SELECT 1 AS n FROM dual UNION ALL
                SELECT 2 FROM dual UNION ALL
                SELECT 3 FROM dual)
          MINUS ALL
          SELECT 2 FROM dual
          INTERSECT
          SELECT 2 FROM dual
          UNION ALL
          SELECT 9 FROM dual);

  SELECT LISTAGG(n, '/') WITHIN GROUP (ORDER BY n)
  INTO   paren_shape
  FROM   ((SELECT n
           FROM (SELECT 1 AS n FROM dual UNION ALL
                 SELECT 2 FROM dual UNION ALL
                 SELECT 3 FROM dual)
           MINUS ALL
           SELECT 2 FROM dual)
          INTERSECT
          (SELECT 1 AS n FROM dual UNION ALL SELECT 3 FROM dual));

  SELECT LISTAGG(n, '/') WITHIN GROUP (ORDER BY n)
  INTO   except_shape
  FROM   (SELECT 1 AS n FROM dual UNION ALL SELECT 1 FROM dual UNION ALL
          SELECT 5 FROM dual
          EXCEPT ALL
          SELECT 1 FROM dual);

  SELECT DBMS_JSON_SCHEMA.IS_VALID(j.doc,
           JSON('{"type" : "object", "required" : ["kind"]}'))
  INTO   schema_valid
  FROM   sq_hard_w12_json j;

  SELECT tag, seeded INTO don_tag, don_seeded
  FROM   sq_hard_w12_don WHERE don_id = 1;

  SELECT COUNT(*) INTO bequeath_rows FROM sq_hard_w12_bequeath_v;

  SELECT note_text, note_value
  INTO   dyn_note, dyn_value
  FROM   sq_hard_w12_note
  WHERE  note_key = 'dynamic';

  IF kw_rows <> 4 OR kw_total <> 100 THEN
    RAISE_APPLICATION_ERROR(-20940,
      'w12 keyword rows ' || kw_rows || '/' || kw_total);
  END IF;
  IF matched_one <> 1 OR errors_one <> 1 THEN
    RAISE_APPLICATION_ERROR(-20941,
      'w12 merge/dynamic counters ' || matched_one || '/' || errors_one);
  END IF;
  IF type_path <> '/1/2/3/4' THEN
    RAISE_APPLICATION_ERROR(-20942, 'w12 connect-by path ' || type_path);
  END IF;
  IF fn_sum <> 44 OR fn_join_rows <> 2 THEN
    RAISE_APPLICATION_ERROR(-20943,
      'w12 builtin-named columns ' || fn_sum || '/' || fn_join_rows);
  END IF;
  IF quoted_sum <> 25 THEN
    RAISE_APPLICATION_ERROR(-20944, 'w12 quoted identifier sum ' || quoted_sum);
  END IF;
  IF pkg_shape <> 'n=1|2|6|[ x ]|nine|4|0' THEN
    RAISE_APPLICATION_ERROR(-20945, 'w12 package shape ' || pkg_shape);
  END IF;
  IF shadow_all <> 4 OR shadow_tail <> 2 THEN
    RAISE_APPLICATION_ERROR(-20946,
      'w12 shadowed parameter ' || shadow_all || '/' || shadow_tail);
  END IF;
  IF capture_note <> 'n=10:kw|41' OR capture_value <> 81 THEN
    RAISE_APPLICATION_ERROR(-20947,
      'w12 label capture ' || capture_note || '/' || capture_value);
  END IF;
  IF tower_shape <> '9' OR paren_shape <> '1/3' OR except_shape <> '1/5' THEN
    RAISE_APPLICATION_ERROR(-20948,
      'w12 set-operator tower ' || tower_shape || '#' || paren_shape || '#'
      || except_shape);
  END IF;
  IF schema_valid <> 1 THEN
    RAISE_APPLICATION_ERROR(-20949, 'w12 json schema ' || schema_valid);
  END IF;
  IF don_tag <> 'auto' OR don_seeded <> 'insert-only' THEN
    RAISE_APPLICATION_ERROR(-20950,
      'w12 default on null ' || don_tag || '/' || don_seeded);
  END IF;
  IF bequeath_rows <> 4 THEN
    RAISE_APPLICATION_ERROR(-20951, 'w12 bequeath rows ' || bequeath_rows);
  END IF;
  IF dyn_note <> 'bulk=4 returning=1 inout=10' OR dyn_value <> 420 THEN
    RAISE_APPLICATION_ERROR(-20952,
      'w12 dynamic sql ' || dyn_note || '/' || dyn_value);
  END IF;
END;
/

--------------------------------------------------------------------------------
-- ULTRA WAVE 13 -- recovery after the grammar changes meaning mid-clause.
-- PARTITION BY belongs to an outer join instead of an analytic, MODEL turns
-- brackets into cell addressing and invents a row, UPDATE grows a FROM clause
-- and OLD/NEW aggregate projections, and JSON_DATAGUIDE is simultaneously an
-- aggregate result, a FORMAT JSON source and the input to JSON_EXISTS.
--------------------------------------------------------------------------------
CREATE TABLE sq_hard_w13_note (
  note_key   VARCHAR2(30) CONSTRAINT sq_hard_w13_note_pk PRIMARY KEY,
  note_text  VARCHAR2(300),
  note_value NUMBER
) TABLESPACE users;

CREATE TABLE sq_hard_w13_assign (
  row_key       NUMBER CONSTRAINT sq_hard_w13_assign_pk PRIMARY KEY,
  current_value NUMBER NOT NULL,
  payload       JSON NOT NULL
) TABLESPACE users;

INSERT INTO sq_hard_w13_assign (row_key, current_value, payload)
VALUES (1, 10, JSON('{"kind":"seed","history":[10]}'));

INSERT INTO sq_hard_w13_assign (row_key, current_value, payload)
VALUES (2, 20, JSON('{"kind":"seed","history":[20]}'));
COMMIT;

--------------------------------------------------------------------------------
-- W13-A: the PARTITION BY below belongs to the row source, not the two analytic
-- functions that follow it. Each NODE_ID partition is independently right
-- joined to the three-day calendar, manufacturing two null-extended rows.
--------------------------------------------------------------------------------
WITH calendar_days (metric_day) AS (
  SELECT DATE '2026-01-01' FROM dual
  UNION ALL SELECT DATE '2026-01-02' FROM dual
  UNION ALL SELECT DATE '2026-01-03' FROM dual
)
SELECT m.node_id,
       c.metric_day,
       NVL(m.metric_value, 0) AS dense_value,
       LAST_VALUE(m.metric_value IGNORE NULLS) OVER (
         PARTITION BY m.node_id
         ORDER BY c.metric_day
         ROWS BETWEEN UNBOUNDED PRECEDING AND CURRENT ROW
       ) AS carried_value,
       COUNT(m.metric_value) OVER (
         PARTITION BY m.node_id
         ORDER BY c.metric_day
         ROWS BETWEEN UNBOUNDED PRECEDING AND CURRENT ROW
       ) AS observations_so_far
FROM sq_hard_metric m
PARTITION BY (m.node_id)
RIGHT OUTER JOIN calendar_days c
  ON (c.metric_day = m.metric_day)
ORDER BY m.node_id, c.metric_day;

DECLARE
  dense_rows NUMBER;
  dense_sum  NUMBER;
  last_carry NUMBER;
BEGIN
  WITH calendar_days (metric_day) AS (
    SELECT DATE '2026-01-01' FROM dual
    UNION ALL SELECT DATE '2026-01-02' FROM dual
    UNION ALL SELECT DATE '2026-01-03' FROM dual
  ),
  dense_rows AS (
    SELECT m.node_id,
           c.metric_day,
           NVL(m.metric_value, 0) AS dense_value,
           LAST_VALUE(m.metric_value IGNORE NULLS) OVER (
             PARTITION BY m.node_id
             ORDER BY c.metric_day
             ROWS BETWEEN UNBOUNDED PRECEDING AND CURRENT ROW
           ) AS carried_value
    FROM sq_hard_metric m
    PARTITION BY (m.node_id)
    RIGHT OUTER JOIN calendar_days c
      ON (c.metric_day = m.metric_day)
  )
  SELECT COUNT(*),
         SUM(dense_value),
         MAX(CASE
               WHEN node_id = 2 AND metric_day = DATE '2026-01-03'
               THEN carried_value
             END)
  INTO dense_rows, dense_sum, last_carry
  FROM dense_rows;

  INSERT INTO sq_hard_w13_note (note_key, note_text, note_value)
  VALUES ('partition-join',
          'rows=' || dense_rows || ',sum=' || dense_sum
          || ',carry=' || last_carry,
          dense_rows * 1000 + dense_sum * 10 + last_carry);
  COMMIT;
END;
/

--------------------------------------------------------------------------------
-- W13-B: MODEL changes the meaning of brackets, ANY, CV and FOR. RETURN UPDATED
-- ROWS suppresses untouched cells; UPSERT creates dimension 5; PRESENTNNV sees
-- whether each cell existed before any rule ran.
--------------------------------------------------------------------------------
SELECT metric_id, projected
FROM (SELECT metric_id, metric_value FROM sq_hard_metric)
MODEL RETURN UPDATED ROWS
  DIMENSION BY (metric_id)
  MEASURES (metric_value AS projected)
  IGNORE NAV
  UNIQUE DIMENSION
  RULES UPSERT SEQUENTIAL ORDER (
    projected[FOR metric_id FROM 1 TO 5 INCREMENT 1] =
      PRESENTNNV(
        projected[CV(metric_id)],
        projected[CV(metric_id)] * 2,
        -CV(metric_id)
      )
  )
ORDER BY metric_id;

DECLARE
  model_shape VARCHAR2(200);
  model_total NUMBER;
BEGIN
  SELECT LISTAGG(metric_id || ':' || projected, '/')
           WITHIN GROUP (ORDER BY metric_id),
         SUM(projected)
  INTO model_shape, model_total
  FROM (
    SELECT metric_id, projected
    FROM (SELECT metric_id, metric_value FROM sq_hard_metric)
    MODEL RETURN UPDATED ROWS
      DIMENSION BY (metric_id)
      MEASURES (metric_value AS projected)
      IGNORE NAV
      UNIQUE DIMENSION
      RULES UPSERT SEQUENTIAL ORDER (
        projected[FOR metric_id FROM 1 TO 5 INCREMENT 1] =
          PRESENTNNV(
            projected[CV(metric_id)],
            projected[CV(metric_id)] * 2,
            -CV(metric_id)
          )
      )
  );

  INSERT INTO sq_hard_w13_note (note_key, note_text, note_value)
  VALUES ('model-cells', model_shape, model_total);
  COMMIT;
END;
/

--------------------------------------------------------------------------------
-- W13-C: Oracle 23ai UPDATE ... FROM with a two-row source. RETURNING projects
-- aggregate OLD and NEW images across both changed rows; JSON_TRANSFORM nests
-- an APPEND action in the same SET list, forcing three expression grammars to
-- close before the FROM owner can be resolved.
--------------------------------------------------------------------------------
DECLARE
  old_total NUMBER;
  new_total NUMBER;
BEGIN
  UPDATE sq_hard_w13_assign target
  SET target.current_value = source.current_value,
      target.payload = JSON_TRANSFORM(
        target.payload,
        SET '$.kind' = source.kind_code,
        APPEND '$.history' = source.current_value
      )
  FROM (
    SELECT 1 AS row_key, 17 AS current_value, 'raised' AS kind_code FROM dual
    UNION ALL
    SELECT 2 AS row_key, 29 AS current_value, 'raised' AS kind_code FROM dual
  ) source
  WHERE target.row_key = source.row_key
  RETURNING SUM(OLD target.current_value), SUM(NEW target.current_value)
  INTO old_total, new_total;

  INSERT INTO sq_hard_w13_note (note_key, note_text, note_value)
  VALUES ('update-from',
          'old=' || old_total || ',new=' || new_total,
          old_total * 100 + new_total);
  COMMIT;
END;
/

SELECT a.row_key,
       a.current_value,
       a.payload.kind.string() AS kind,
       JSON_SERIALIZE(
         JSON_QUERY(a.payload, '$.history' WITH ARRAY WRAPPER)
         RETURNING VARCHAR2(100)
       ) AS history
FROM sq_hard_w13_assign a
ORDER BY a.row_key;

--------------------------------------------------------------------------------
-- W13-D: JSON_DATAGUIDE aggregates all JSON payloads. FORMAT JSON changes the
-- CLOB's parse role inside both JSON_SERIALIZE and JSON_EXISTS; PRETTY and
-- RETURNING then belong to the outer serializer, not to the aggregate.
--------------------------------------------------------------------------------
SELECT JSON_SERIALIZE(
         JSON_DATAGUIDE(payload, DBMS_JSON.FORMAT_HIERARCHICAL)
         FORMAT JSON
         RETURNING VARCHAR2(4000) PRETTY
       ) AS payload_guide
FROM sq_hard_w13_assign;

DECLARE
  guide_doc CLOB;
  guide_len NUMBER;
  has_kind  NUMBER;
BEGIN
  SELECT JSON_DATAGUIDE(payload, DBMS_JSON.FORMAT_HIERARCHICAL)
  INTO guide_doc
  FROM sq_hard_w13_assign;

  guide_len := DBMS_LOB.GETLENGTH(guide_doc);
  has_kind := CASE
                WHEN JSON_EXISTS(
                       guide_doc FORMAT JSON,
                       '$.properties.kind'
                     )
                THEN 1 ELSE 0
              END;

  INSERT INTO sq_hard_w13_note (note_key, note_text, note_value)
  VALUES ('data-guide',
          'kind=' || has_kind || ',bytes=' || guide_len,
          guide_len);
  COMMIT;
END;
/

--------------------------------------------------------------------------------
-- Wave-13 self-verification.
--------------------------------------------------------------------------------
DECLARE
  note_rows     PLS_INTEGER;
  dense_text    VARCHAR2(300);
  dense_value   NUMBER;
  model_text    VARCHAR2(300);
  model_value   NUMBER;
  update_text   VARCHAR2(300);
  update_value  NUMBER;
  guide_text    VARCHAR2(300);
  guide_value   NUMBER;
  assign_total  NUMBER;
  history_total NUMBER;
BEGIN
  SELECT COUNT(*) INTO note_rows FROM sq_hard_w13_note;

  SELECT note_text, note_value INTO dense_text, dense_value
  FROM sq_hard_w13_note WHERE note_key = 'partition-join';

  SELECT note_text, note_value INTO model_text, model_value
  FROM sq_hard_w13_note WHERE note_key = 'model-cells';

  SELECT note_text, note_value INTO update_text, update_value
  FROM sq_hard_w13_note WHERE note_key = 'update-from';

  SELECT note_text, note_value INTO guide_text, guide_value
  FROM sq_hard_w13_note WHERE note_key = 'data-guide';

  SELECT SUM(current_value),
         SUM(JSON_VALUE(payload, '$.history.size()' RETURNING NUMBER))
  INTO assign_total, history_total
  FROM sq_hard_w13_assign;

  IF note_rows <> 4 THEN
    RAISE_APPLICATION_ERROR(-20960, 'w13 note rows ' || note_rows);
  END IF;
  IF dense_text <> 'rows=6,sum=32,carry=2' OR dense_value <> 6322 THEN
    RAISE_APPLICATION_ERROR(-20961,
      'w13 partition join ' || dense_text || '/' || dense_value);
  END IF;
  IF model_text <> '1:24/2:36/3:48/4:4/5:-5' OR model_value <> 107 THEN
    RAISE_APPLICATION_ERROR(-20962,
      'w13 model cells ' || model_text || '/' || model_value);
  END IF;
  IF update_text <> 'old=30,new=46' OR update_value <> 3046 THEN
    RAISE_APPLICATION_ERROR(-20963,
      'w13 update from ' || update_text || '/' || update_value);
  END IF;
  IF guide_text NOT LIKE 'kind=1,bytes=%' OR guide_value < 100 THEN
    RAISE_APPLICATION_ERROR(-20964,
      'w13 data guide ' || guide_text || '/' || guide_value);
  END IF;
  IF assign_total <> 46 OR history_total <> 4 THEN
    RAISE_APPLICATION_ERROR(-20965,
      'w13 assignment state ' || assign_total || '/' || history_total);
  END IF;
END;
/

--------------------------------------------------------------------------------
-- ULTRA WAVE 14 -- 26ai grammar whose columns and row images do not exist in
-- the base table declaration. A multi-column use-case domain binds two ordinary
-- columns into one semantic value; MERGE returns OLD and NEW images from both
-- UPDATE and INSERT branches into PL/SQL collections; partitioned row limiting
-- mints a top-N inside each ordered group; and a nested WITH query correlates
-- through CROSS APPLY before DOMAIN_* expressions are resolved.
--------------------------------------------------------------------------------
CREATE TABLE sq_hard_w14_note (
  note_key   VARCHAR2(30) CONSTRAINT sq_hard_w14_note_pk PRIMARY KEY,
  note_text  VARCHAR2(400),
  note_value NUMBER
) TABLESPACE users;

--------------------------------------------------------------------------------
-- W14-A: a multi-column domain. AMOUNT and CURRENCY_CODE remain separately
-- addressable columns, but DOMAIN binds them into a single constrained value.
-- DOMAIN_NAME, DOMAIN_DISPLAY and DOMAIN_ORDER infer that association from the
-- expression list; DOMAIN_CHECK instead names the domain as grammar, not data.
--------------------------------------------------------------------------------
CREATE DOMAIN sq_hard_w14_currency AS (
  amount        AS NUMBER(10, 2),
  currency_code AS CHAR(3 CHAR)
)
CONSTRAINT sq_hard_w14_currency_ck CHECK (
  amount >= 0
  AND currency_code IN ('EUR', 'KRW', 'USD')
)
DISPLAY (
  TO_CHAR(
    amount,
    'FM9999990D00',
    'NLS_NUMERIC_CHARACTERS = ''.,'''
  ) || ' ' || currency_code
)
ORDER (
  currency_code
  || LPAD(TO_CHAR(amount * 100, 'FM9999999990'), 16, '0')
)
ANNOTATIONS (
  Title 'Validated amount and ISO currency',
  Purpose 'Wave 14 multi-column completion'
);

CREATE TABLE sq_hard_w14_money (
  row_id        NUMBER CONSTRAINT sq_hard_w14_money_pk PRIMARY KEY,
  owner_name    VARCHAR2(30) NOT NULL,
  amount        NUMBER(10, 2) NOT NULL,
  currency_code CHAR(3 CHAR) NOT NULL,
  changed_at    TIMESTAMP(6) DEFAULT SYSTIMESTAMP NOT NULL,
  DOMAIN sq_hard_w14_currency (amount, currency_code)
) TABLESPACE users;

INSERT INTO sq_hard_w14_money (
  row_id, owner_name, amount, currency_code
)
VALUES (1, 'alpha', 10, 'USD'),
       (2, 'alpha', 30, 'USD'),
       (3, 'beta',  20, 'EUR'),
       (4, 'beta',  40, 'EUR'),
       (5, 'gamma', 50, 'KRW'),
       (6, 'gamma', 60, 'KRW');
COMMIT;

SELECT m.row_id,
       DOMAIN_NAME(m.amount, m.currency_code)    AS semantic_type,
       DOMAIN_DISPLAY(m.amount, m.currency_code) AS display_value,
       DOMAIN_ORDER(m.amount, m.currency_code)   AS semantic_sort_key,
       DOMAIN_CHECK(
         sq_hard_w14_currency,
         m.amount,
         m.currency_code
       )                                        AS domain_valid
FROM sq_hard_w14_money m
ORDER BY DOMAIN_ORDER(m.amount, m.currency_code), m.row_id;

DECLARE
  inferred_name VARCHAR2(261);
  display_value VARCHAR2(100);
  order_value   VARCHAR2(100);
  valid_flag    PLS_INTEGER;
  invalid_flag  PLS_INTEGER;
BEGIN
  SELECT DOMAIN_NAME(m.amount, m.currency_code),
         DOMAIN_DISPLAY(m.amount, m.currency_code),
         DOMAIN_ORDER(m.amount, m.currency_code),
         CASE
           WHEN DOMAIN_CHECK(
                  sq_hard_w14_currency,
                  m.amount,
                  m.currency_code
                )
           THEN 1 ELSE 0
         END,
         CASE
           WHEN DOMAIN_CHECK(sq_hard_w14_currency, -1, 'BTC')
           THEN 1 ELSE 0
         END
  INTO inferred_name, display_value, order_value, valid_flag, invalid_flag
  FROM sq_hard_w14_money m
  WHERE m.row_id = 1;

  INSERT INTO sq_hard_w14_note (note_key, note_text, note_value)
  VALUES (
    'domain',
    inferred_name || '|' || display_value || '|' || order_value,
    valid_flag * 10 + invalid_flag
  );
  COMMIT;
END;
/

--------------------------------------------------------------------------------
-- W14-B: VALUES is a row source inside MERGE. RETURNING sees both branches and
-- exposes OLD/NEW row images as independent collection elements: the inserted
-- row contributes NULL to OLD, while both changed rows contribute to NEW.
--------------------------------------------------------------------------------
DECLARE
  TYPE number_list_t IS TABLE OF NUMBER;
  TYPE id_list_t     IS TABLE OF sq_hard_w14_money.row_id%TYPE;
  changed_ids id_list_t;
  old_amounts number_list_t;
  new_amounts number_list_t;
  changed_count PLS_INTEGER;
  old_total   NUMBER := 0;
  new_total   NUMBER := 0;
BEGIN
  MERGE INTO sq_hard_w14_money target
  USING (
    VALUES (2, 'alpha-updated', 35, 'USD'),
           (7, 'delta',         70, 'USD')
  ) source (row_id, owner_name, amount, currency_code)
  ON (target.row_id = source.row_id)
  WHEN MATCHED THEN
    UPDATE SET
      target.owner_name    = source.owner_name,
      target.amount        = source.amount,
      target.currency_code = source.currency_code,
      target.changed_at    = SYSTIMESTAMP
  WHEN NOT MATCHED THEN
    INSERT (row_id, owner_name, amount, currency_code)
    VALUES (
      source.row_id,
      source.owner_name,
      source.amount,
      source.currency_code
    )
  RETURNING target.row_id, OLD target.amount, NEW target.amount
  BULK COLLECT INTO changed_ids, old_amounts, new_amounts;

  changed_count := changed_ids.COUNT;
  FOR i IN 1 .. changed_count LOOP
    old_total := old_total + NVL(old_amounts(i), 0);
    new_total := new_total + NVL(new_amounts(i), 0);
  END LOOP;

  INSERT INTO sq_hard_w14_note (note_key, note_text, note_value)
  VALUES (
    'merge-images',
    'rows=' || changed_count
    || ',old=' || old_total
    || ',new=' || new_total,
    old_total * 1000 + new_total
  );
  COMMIT;
END;
/

--------------------------------------------------------------------------------
-- W14-C: this is not an analytic PARTITION BY. The first numeric expression
-- limits the number of currency partitions, while the second limits rows inside
-- each one. Both invented limits belong to FETCH after the ORDER BY has closed.
--------------------------------------------------------------------------------
SELECT m.currency_code,
       m.row_id,
       m.owner_name,
       m.amount,
       DOMAIN_DISPLAY(m.amount, m.currency_code) AS display_value
FROM sq_hard_w14_money m
ORDER BY m.currency_code, m.amount DESC, m.row_id
FETCH FIRST 3 PARTITIONS BY m.currency_code, 2 ROWS ONLY;

DECLARE
  top_rows  PLS_INTEGER;
  top_total NUMBER;
BEGIN
  SELECT COUNT(*), SUM(amount)
  INTO top_rows, top_total
  FROM (
    SELECT m.currency_code, m.row_id, m.amount
    FROM sq_hard_w14_money m
    ORDER BY m.currency_code, m.amount DESC, m.row_id
    FETCH FIRST 3 PARTITIONS BY m.currency_code, 2 ROWS ONLY
  );

  INSERT INTO sq_hard_w14_note (note_key, note_text, note_value)
  VALUES (
    'partition-topn',
    'rows=' || top_rows || ',sum=' || top_total,
    top_rows * 1000 + top_total
  );
  COMMIT;
END;
/

--------------------------------------------------------------------------------
-- W14-D: a WITH clause lives inside the right side of CROSS APPLY, while its
-- final query correlates back to CURRENCY_CODE from the left side. The nested
-- scope preserves a two-column domain association through LISTAGG ordering.
--------------------------------------------------------------------------------
SELECT RTRIM(c.currency_code) AS currency_code,
       local_rows.total_amount,
       local_rows.display_list
FROM (
  SELECT DISTINCT m.currency_code
  FROM sq_hard_w14_money m
) c
CROSS APPLY (
  WITH scoped_rows AS (
    SELECT m.row_id, m.amount, m.currency_code
    FROM sq_hard_w14_money m
  )
  SELECT SUM(s.amount) AS total_amount,
         LISTAGG(
           DOMAIN_DISPLAY(s.amount, s.currency_code),
           '/'
         ) WITHIN GROUP (
           ORDER BY DOMAIN_ORDER(s.amount, s.currency_code), s.row_id
         ) AS display_list
  FROM scoped_rows s
  WHERE s.currency_code = c.currency_code
) local_rows
ORDER BY c.currency_code;

DECLARE
  group_shape VARCHAR2(300);
  grand_total NUMBER := 0;
BEGIN
  FOR currency_row IN (
    SELECT c.currency_code, local_rows.total_amount
    FROM (
      SELECT DISTINCT m.currency_code
      FROM sq_hard_w14_money m
    ) c
    CROSS APPLY (
      WITH scoped_rows AS (
        SELECT m.row_id, m.amount, m.currency_code
        FROM sq_hard_w14_money m
      )
      SELECT SUM(s.amount) AS total_amount,
             LISTAGG(
               DOMAIN_DISPLAY(s.amount, s.currency_code),
               '/'
             ) WITHIN GROUP (
               ORDER BY DOMAIN_ORDER(s.amount, s.currency_code), s.row_id
             ) AS display_list
      FROM scoped_rows s
      WHERE s.currency_code = c.currency_code
    ) local_rows
    ORDER BY c.currency_code
  ) LOOP
    group_shape := group_shape
      || CASE WHEN group_shape IS NULL THEN NULL ELSE '/' END
      || RTRIM(currency_row.currency_code)
      || '='
      || TO_CHAR(
           currency_row.total_amount,
           'FM9999990D00',
           'NLS_NUMERIC_CHARACTERS = ''.,'''
         );
    grand_total := grand_total + currency_row.total_amount;
  END LOOP;

  INSERT INTO sq_hard_w14_note (note_key, note_text, note_value)
  VALUES ('nested-with', group_shape, grand_total);
  COMMIT;
END;
/

--------------------------------------------------------------------------------
-- Wave-14 self-verification.
--------------------------------------------------------------------------------
DECLARE
  note_rows    PLS_INTEGER;
  money_rows   PLS_INTEGER;
  money_total  NUMBER;
  domain_text  VARCHAR2(400);
  domain_value NUMBER;
  merge_text   VARCHAR2(400);
  merge_value  NUMBER;
  topn_text    VARCHAR2(400);
  topn_value   NUMBER;
  nested_text  VARCHAR2(400);
  nested_value NUMBER;
BEGIN
  SELECT COUNT(*) INTO note_rows FROM sq_hard_w14_note;
  SELECT COUNT(*), SUM(amount)
  INTO money_rows, money_total
  FROM sq_hard_w14_money;

  SELECT note_text, note_value INTO domain_text, domain_value
  FROM sq_hard_w14_note WHERE note_key = 'domain';

  SELECT note_text, note_value INTO merge_text, merge_value
  FROM sq_hard_w14_note WHERE note_key = 'merge-images';

  SELECT note_text, note_value INTO topn_text, topn_value
  FROM sq_hard_w14_note WHERE note_key = 'partition-topn';

  SELECT note_text, note_value INTO nested_text, nested_value
  FROM sq_hard_w14_note WHERE note_key = 'nested-with';

  IF note_rows <> 4 OR money_rows <> 7 OR money_total <> 285 THEN
    RAISE_APPLICATION_ERROR(
      -20970,
      'w14 cardinality ' || note_rows || '/' || money_rows || '/' || money_total
    );
  END IF;
  IF domain_text NOT LIKE '%.SQ_HARD_W14_CURRENCY|10.00 USD|USD%'
     OR domain_value <> 10 THEN
    RAISE_APPLICATION_ERROR(
      -20971,
      'w14 domain ' || domain_text || '/' || domain_value
    );
  END IF;
  IF merge_text <> 'rows=2,old=30,new=105' OR merge_value <> 30105 THEN
    RAISE_APPLICATION_ERROR(
      -20972,
      'w14 merge images ' || merge_text || '/' || merge_value
    );
  END IF;
  IF topn_text <> 'rows=6,sum=275' OR topn_value <> 6275 THEN
    RAISE_APPLICATION_ERROR(
      -20973,
      'w14 partition topn ' || topn_text || '/' || topn_value
    );
  END IF;
  IF nested_text <> 'EUR=60.00/KRW=110.00/USD=115.00'
     OR nested_value <> 285 THEN
    RAISE_APPLICATION_ERROR(
      -20974,
      'w14 nested with ' || nested_text || '/' || nested_value
    );
  END IF;
END;
/

--------------------------------------------------------------------------------
-- W15: Oracle AI Database 26ai "parser endgame".
--      This wave deliberately makes five rarely adjacent grammar islands collide:
--        * DEFAULT ON NULL FOR INSERT AND UPDATE on a SQL BOOLEAN row
--        * UUID() as an explicit RAW(16) DML expression beside a default
--        * three-table ANSI joins across enforced PK/FK relationships
--        * BOOLEAN_AND_AGG / BOOLEAN_OR_AGG in grouped and analytic contexts
--        * exact, multi-level partitioned row limiting
--------------------------------------------------------------------------------
CREATE TABLE sq_hard_w15_region (
  region_id   NUMBER       CONSTRAINT sq_hard_w15_region_pk PRIMARY KEY,
  region_name VARCHAR2(30) CONSTRAINT sq_hard_w15_region_uq UNIQUE NOT NULL
);

CREATE TABLE sq_hard_w15_account (
  account_id   NUMBER       CONSTRAINT sq_hard_w15_account_pk PRIMARY KEY,
  region_id    NUMBER       NOT NULL,
  account_name VARCHAR2(30) NOT NULL,
  CONSTRAINT sq_hard_w15_account_region_fk
    FOREIGN KEY (region_id) REFERENCES sq_hard_w15_region (region_id)
);

CREATE TABLE sq_hard_w15_event (
  event_uuid  RAW(16) DEFAULT SYS_GUID()
    CONSTRAINT sq_hard_w15_event_pk PRIMARY KEY,
  account_id  NUMBER NOT NULL,
  event_kind  VARCHAR2(12) NOT NULL,
  amount      NUMBER(10, 2)
    DEFAULT ON NULL FOR INSERT AND UPDATE 15 NOT NULL,
  active      BOOLEAN DEFAULT TRUE NOT NULL,
  occurred_at TIMESTAMP WITH TIME ZONE NOT NULL,
  CONSTRAINT sq_hard_w15_event_account_fk
    FOREIGN KEY (account_id) REFERENCES sq_hard_w15_account (account_id),
  CONSTRAINT sq_hard_w15_event_kind_ck
    CHECK (event_kind IN ('sale', 'refund', 'audit'))
);

CREATE TABLE sq_hard_w15_note (
  note_key   VARCHAR2(30) CONSTRAINT sq_hard_w15_note_pk PRIMARY KEY,
  note_text  VARCHAR2(400) NOT NULL,
  note_value NUMBER NOT NULL
);

INSERT ALL
  INTO sq_hard_w15_region (region_id, region_name) VALUES (1, 'NORTH')
  INTO sq_hard_w15_region (region_id, region_name) VALUES (2, 'SOUTH')
SELECT 1 FROM dual;

INSERT ALL
  INTO sq_hard_w15_account (account_id, region_id, account_name)
    VALUES (10, 1, 'alpha')
  INTO sq_hard_w15_account (account_id, region_id, account_name)
    VALUES (11, 1, 'beta')
  INTO sq_hard_w15_account (account_id, region_id, account_name)
    VALUES (20, 2, 'gamma')
SELECT 1 FROM dual;

INSERT ALL
  INTO sq_hard_w15_event (
    event_uuid, account_id, event_kind, amount, active, occurred_at
  )
    VALUES (
      UUID(), 10, 'sale', 10, TRUE,
      TIMESTAMP '2025-01-01 09:00:00 +09:00'
    )
  INTO sq_hard_w15_event (
    event_uuid, account_id, event_kind, amount, active, occurred_at
  )
    VALUES (
      UUID(), 10, 'refund', NULL, FALSE,
      TIMESTAMP '2025-01-01 10:00:00 +09:00'
    )
  INTO sq_hard_w15_event (
    event_uuid, account_id, event_kind, amount, active, occurred_at
  )
    VALUES (
      UUID(), 11, 'sale', 20, TRUE,
      TIMESTAMP '2025-01-01 11:00:00 +09:00'
    )
  INTO sq_hard_w15_event (
    event_uuid, account_id, event_kind, amount, active, occurred_at
  )
    VALUES (
      UUID(), 20, 'sale', 30, TRUE,
      TIMESTAMP '2025-01-01 12:00:00 +09:00'
    )
  INTO sq_hard_w15_event (
    event_uuid, account_id, event_kind, amount, active, occurred_at
  )
    VALUES (
      UUID(), 20, 'audit', 40, FALSE,
      TIMESTAMP '2025-01-01 13:00:00 +09:00'
    )
SELECT 1 FROM dual;

-- The UPDATE-specific half of DEFAULT ON NULL must turn this NULL back into 15.
UPDATE sq_hard_w15_event
SET amount = NULL
WHERE account_id = 20
  AND event_kind = 'sale';

DECLARE
  regional_shape VARCHAR2(400);
  active_total   NUMBER;
  analytic_shape VARCHAR2(400);
  fetched_shape  VARCHAR2(400);
  fetched_rows   PLS_INTEGER;
  fetched_total  NUMBER;
  uuid_rows      PLS_INTEGER;
  uuid_hex_min   PLS_INTEGER;
  uuid_hex_max   PLS_INTEGER;
  defaulted_rows PLS_INTEGER;
BEGIN
  WITH regional AS (
    SELECT r.region_name,
           SUM(CASE WHEN e.active THEN e.amount ELSE 0 END) AS active_amount,
           SUM(CASE WHEN e.event_kind = 'sale' THEN 1 ELSE 0 END) AS sale_rows,
           SUM(CASE WHEN e.event_kind = 'refund' THEN 1 ELSE 0 END)
             AS refund_rows,
           BOOLEAN_AND_AGG(e.active) AS all_active,
           BOOLEAN_OR_AGG(e.active) AS any_active
    FROM sq_hard_w15_event e
         JOIN sq_hard_w15_account a
           ON a.account_id = e.account_id
         JOIN sq_hard_w15_region r
           ON r.region_id = a.region_id
    GROUP BY r.region_name
  )
  SELECT LISTAGG(
           region_name || '=' ||
           CASE WHEN all_active THEN 'T' ELSE 'F' END || '/' ||
           CASE WHEN any_active THEN 'T' ELSE 'F' END || ':' ||
           TO_CHAR(
             active_amount,
             'FM9990D00',
             'NLS_NUMERIC_CHARACTERS=''.,'''
           ) || ':' || sale_rows || ':' || refund_rows,
           '/'
         ) WITHIN GROUP (ORDER BY region_name),
         SUM(active_amount)
  INTO regional_shape, active_total
  FROM regional;

  INSERT INTO sq_hard_w15_note (note_key, note_text, note_value)
  VALUES ('boolean-join', regional_shape, active_total);

  WITH widened AS (
    SELECT r.region_name,
           a.account_id,
           e.event_kind,
           SUM(CASE WHEN e.active THEN e.amount ELSE 0 END)
             OVER (PARTITION BY r.region_id) AS region_active_amount,
           BOOLEAN_AND_AGG(e.active)
             OVER (PARTITION BY r.region_id) AS region_all_active,
           BOOLEAN_OR_AGG(e.active)
             OVER (PARTITION BY r.region_id) AS region_any_active,
           ROW_NUMBER() OVER (
             PARTITION BY r.region_id
             ORDER BY e.amount DESC, e.occurred_at, e.event_uuid
           ) AS region_position
    FROM sq_hard_w15_event e
         JOIN sq_hard_w15_account a
           ON a.account_id = e.account_id
         JOIN sq_hard_w15_region r
           ON r.region_id = a.region_id
  )
  SELECT LISTAGG(
           region_name || ':' || account_id || ':' || event_kind || ':' ||
           TO_CHAR(
             region_active_amount,
             'FM9990D00',
             'NLS_NUMERIC_CHARACTERS=''.,'''
           ) || ':' ||
           CASE WHEN region_all_active THEN 'T' ELSE 'F' END || '/' ||
           CASE WHEN region_any_active THEN 'T' ELSE 'F' END,
           '/'
         ) WITHIN GROUP (ORDER BY region_name)
  INTO analytic_shape
  FROM widened
  WHERE region_position = 1;

  INSERT INTO sq_hard_w15_note (note_key, note_text, note_value)
  VALUES ('analytic-filter', analytic_shape, LENGTH(analytic_shape));

  SELECT LISTAGG(account_id, '/') WITHIN GROUP (ORDER BY account_id),
         COUNT(*),
         SUM(amount)
  INTO fetched_shape, fetched_rows, fetched_total
  FROM (
    SELECT a.account_id,
           e.amount
    FROM sq_hard_w15_event e
         JOIN sq_hard_w15_account a
           ON a.account_id = e.account_id
         JOIN sq_hard_w15_region r
           ON r.region_id = a.region_id
    ORDER BY r.region_id,
             a.account_id,
             e.occurred_at,
             e.event_uuid
    FETCH EXACT FIRST
      2 PARTITIONS BY r.region_id,
      2 PARTITIONS BY a.account_id,
      1 ROW ONLY
  );

  INSERT INTO sq_hard_w15_note (note_key, note_text, note_value)
  VALUES (
    'partition-fetch',
    'accounts=' || fetched_shape || ',rows=' || fetched_rows ||
      ',sum=' || fetched_total,
    fetched_rows * 1000 + fetched_total
  );

  SELECT COUNT(DISTINCT event_uuid),
         MIN(LENGTH(RAWTOHEX(event_uuid))),
         MAX(LENGTH(RAWTOHEX(event_uuid))),
         SUM(CASE WHEN amount = 15 THEN 1 ELSE 0 END)
  INTO uuid_rows, uuid_hex_min, uuid_hex_max, defaulted_rows
  FROM sq_hard_w15_event;

  INSERT INTO sq_hard_w15_note (note_key, note_text, note_value)
  VALUES (
    'uuid-default',
    'uuid=' || uuid_rows || ',hex=' || uuid_hex_min || '-' || uuid_hex_max ||
      ',defaulted=' || defaulted_rows,
    uuid_rows * 100 + defaulted_rows
  );

  COMMIT;
END;
/

DECLARE
  note_rows      PLS_INTEGER;
  event_rows     PLS_INTEGER;
  event_total    NUMBER;
  regional_shape VARCHAR2(400);
  regional_total NUMBER;
  analytic_shape VARCHAR2(400);
  fetched_text   VARCHAR2(400);
  fetched_value  NUMBER;
  uuid_text      VARCHAR2(400);
  uuid_value     NUMBER;
BEGIN
  SELECT COUNT(*) INTO note_rows FROM sq_hard_w15_note;
  SELECT COUNT(*), SUM(amount)
  INTO event_rows, event_total
  FROM sq_hard_w15_event;

  SELECT note_text, note_value
  INTO regional_shape, regional_total
  FROM sq_hard_w15_note
  WHERE note_key = 'boolean-join';

  SELECT note_text
  INTO analytic_shape
  FROM sq_hard_w15_note
  WHERE note_key = 'analytic-filter';

  SELECT note_text, note_value
  INTO fetched_text, fetched_value
  FROM sq_hard_w15_note
  WHERE note_key = 'partition-fetch';

  SELECT note_text, note_value
  INTO uuid_text, uuid_value
  FROM sq_hard_w15_note
  WHERE note_key = 'uuid-default';

  IF note_rows <> 4 OR event_rows <> 5 OR event_total <> 100 THEN
    RAISE_APPLICATION_ERROR(
      -20980,
      'w15 cardinality ' || note_rows || '/' || event_rows || '/' || event_total
    );
  END IF;
  IF regional_shape <> 'NORTH=F/T:30.00:2:1/SOUTH=F/T:15.00:1:0'
     OR regional_total <> 45 THEN
    RAISE_APPLICATION_ERROR(
      -20981,
      'w15 boolean join ' || regional_shape || '/' || regional_total
    );
  END IF;
  IF analytic_shape <>
       'NORTH:11:sale:30.00:F/T/SOUTH:20:audit:15.00:F/T' THEN
    RAISE_APPLICATION_ERROR(
      -20982,
      'w15 analytic filter ' || analytic_shape
    );
  END IF;
  IF fetched_text <> 'accounts=10/11/20,rows=3,sum=45'
     OR fetched_value <> 3045 THEN
    RAISE_APPLICATION_ERROR(
      -20983,
      'w15 partition fetch ' || fetched_text || '/' || fetched_value
    );
  END IF;
  IF uuid_text <> 'uuid=5,hex=32-32,defaulted=2' OR uuid_value <> 502 THEN
    RAISE_APPLICATION_ERROR(
      -20984,
      'w15 uuid/default ' || uuid_text || '/' || uuid_value
    );
  END IF;
END;
/

--------------------------------------------------------------------------------
-- ULTRA WAVE 16 -- SQL/PGQ graph-shape and relational-pattern final boss.
-- One variable-length graph pattern is projected three incompatible ways:
--   * ONE ROW PER STEP feeds a relational MATCH_RECOGNIZE partition
--   * ONE ROW PER VERTEX expands an iterator's properties through v.*
--   * ONE ROW PER MATCH feeds QUALIFY and typed JSON aggregation
-- Curly graph quantifiers, arrow tokens, path/iterator variables, graph-local
-- aggregates, SQL pattern variables, PIVOT-generated names and JSON aliases all
-- coexist while every result remains deterministic and self-verifying.
--------------------------------------------------------------------------------
CREATE TABLE sq_hard_w16_note (
  note_key   VARCHAR2(30) CONSTRAINT sq_hard_w16_note_pk PRIMARY KEY,
  note_text  VARCHAR2(700) NOT NULL,
  note_value NUMBER NOT NULL
) TABLESPACE users;

--------------------------------------------------------------------------------
-- W16-A: ROUTE is a SQL/PGQ path variable. E has group degree outside the
-- {1,2} quantifier, while STEP_* are iterator variables owned only by the
-- ONE ROW PER STEP shape. MATCH_RECOGNIZE then interprets those graph rows as
-- three independent ordered streams and checks that every multi-hop route rises.
--------------------------------------------------------------------------------
INSERT INTO sq_hard_w16_note (note_key, note_text, note_value)
WITH graph_steps AS (
  SELECT match_no,
         path_name,
         edge_element_no,
         src_id,
         step_cost,
         dst_id,
         cost_path
  FROM GRAPH_TABLE (
    sq_hard_graph
    MATCH route = (a IS metric) -[e IS links]->{1,2} (b IS metric)
    WHERE a.metric_id = 1
      AND COUNT(EDGE_ID(e)) = COUNT(DISTINCT EDGE_ID(e))
    ONE ROW PER STEP (step_src, step_edge, step_dst) IN (route)
    COLUMNS (
      MATCHNUM() AS match_no,
      PATH_NAME() AS path_name,
      ELEMENT_NUMBER(step_edge) AS edge_element_no,
      step_src.metric_id AS src_id,
      step_edge.hop_cost AS step_cost,
      step_dst.metric_id AS dst_id,
      LISTAGG(e.hop_cost, '>') AS cost_path
    )
  )
),
recognized_paths AS (
  SELECT match_no,
         path_name,
         hop_count,
         total_cost,
         first_src,
         last_dst
  FROM graph_steps
  MATCH_RECOGNIZE (
    PARTITION BY match_no, path_name
    ORDER BY edge_element_no
    MEASURES
      COUNT(*)         AS hop_count,
      SUM(step_cost)   AS total_cost,
      FIRST(src_id)    AS first_src,
      LAST(dst_id)     AS last_dst
    ONE ROW PER MATCH
    AFTER MATCH SKIP PAST LAST ROW
    PATTERN (head rising*)
    DEFINE
      rising AS rising.step_cost > PREV(rising.step_cost)
  )
)
SELECT 'graph-step-pattern',
       LISTAGG(
         first_src || '>' || last_dst || ':' || hop_count || ':' || total_cost,
         '/'
       ) WITHIN GROUP (
         ORDER BY hop_count, first_src, last_dst
       ),
       SUM(total_cost)
FROM recognized_paths;

--------------------------------------------------------------------------------
-- W16-B: STEP_VERTEX.* is not a SQL table wildcard: GRAPH_TABLE expands the
-- iterator's declared graph properties. The generated rows are condensed by
-- path, pivoted into six generated identifiers, then multi-column UNPIVOT turns
-- three pairs back into rows. Every alias crosses at least two grammar scopes.
--------------------------------------------------------------------------------
INSERT INTO sq_hard_w16_note (note_key, note_text, note_value)
WITH vertex_rows AS (
  SELECT match_no,
         path_name,
         element_no,
         metric_id,
         node_id,
         metric_value,
         cost_path
  FROM GRAPH_TABLE (
    sq_hard_graph
    MATCH route = (a IS metric) -[e IS links]->{1,2} (b IS metric)
    WHERE a.metric_id = 1
    ONE ROW PER VERTEX (step_vertex) IN (route)
    COLUMNS (
      MATCHNUM() AS match_no,
      PATH_NAME() AS path_name,
      ELEMENT_NUMBER(step_vertex) AS element_no,
      step_vertex.*,
      LISTAGG(e.hop_cost, '>') AS cost_path
    )
  )
),
path_totals AS (
  SELECT cost_path,
         LISTAGG(metric_id, '>')
           WITHIN GROUP (ORDER BY element_no) AS vertex_path,
         SUM(metric_value) AS vertex_total
  FROM vertex_rows
  GROUP BY cost_path
),
path_matrix AS (
  SELECT *
  FROM path_totals
  PIVOT (
    MAX(vertex_total) AS total,
    MAX(vertex_path)  AS nodes
    FOR cost_path IN (
      '5'   AS direct_five,
      '11'  AS direct_eleven,
      '5>7' AS two_hop
    )
  )
),
path_cells AS (
  SELECT path_kind, path_total, node_path
  FROM path_matrix
  UNPIVOT INCLUDE NULLS (
    (path_total, node_path) FOR path_kind IN (
      (direct_five_total, direct_five_nodes)     AS 'FIVE',
      (direct_eleven_total, direct_eleven_nodes) AS 'ELEVEN',
      (two_hop_total, two_hop_nodes)             AS 'TWO'
    )
  )
)
SELECT 'graph-vertex-pivot',
       LISTAGG(
         path_kind || '=' || node_path || ':' || path_total,
         '/'
       ) WITHIN GROUP (ORDER BY path_kind),
       SUM(path_total)
FROM path_cells;

--------------------------------------------------------------------------------
-- W16-C: the default ONE ROW PER MATCH shape exposes group-degree aggregates.
-- QUALIFY chooses the cheapest route independently at each hop count before
-- JSON_OBJECT values are ordered into a typed JSON array and serialized.
--------------------------------------------------------------------------------
INSERT INTO sq_hard_w16_note (note_key, note_text, note_value)
WITH graph_paths AS (
  SELECT start_id,
         end_id,
         hop_count,
         total_cost,
         cost_path
  FROM GRAPH_TABLE (
    sq_hard_graph
    MATCH (a IS metric) -[e IS links]->{1,2} (b IS metric)
    WHERE a.metric_id = 1
    COLUMNS (
      a.metric_id      AS start_id,
      b.metric_id      AS end_id,
      COUNT(e.hop_cost) AS hop_count,
      SUM(e.hop_cost)   AS total_cost,
      LISTAGG(e.hop_cost, '>') AS cost_path
    )
  )
),
cheapest_per_depth AS (
  SELECT p.*,
         JSON_OBJECT(
           'from'  VALUE p.start_id,
           'to'    VALUE p.end_id,
           'hops'  VALUE p.hop_count,
           'cost'  VALUE p.total_cost,
           'edges' VALUE p.cost_path
           RETURNING JSON
         ) AS path_doc
  FROM graph_paths p
  QUALIFY ROW_NUMBER() OVER (
    PARTITION BY p.hop_count
    ORDER BY p.total_cost, p.end_id
  ) = 1
)
SELECT 'graph-qualify-json',
       JSON_SERIALIZE(
         JSON_ARRAYAGG(path_doc ORDER BY hop_count RETURNING JSON)
         RETURNING VARCHAR2(700)
       ),
       SUM(total_cost)
FROM cheapest_per_depth;

COMMIT;

--------------------------------------------------------------------------------
-- Wave-16 self-verification.
--------------------------------------------------------------------------------
DECLARE
  note_rows    PLS_INTEGER;
  step_text    VARCHAR2(700);
  step_value   NUMBER;
  vertex_text  VARCHAR2(700);
  vertex_value NUMBER;
  json_text    VARCHAR2(700);
  json_value   NUMBER;
  json_size    NUMBER;
  json_first   NUMBER;
  json_second  NUMBER;
  json_edges   VARCHAR2(30);
BEGIN
  SELECT COUNT(*) INTO note_rows FROM sq_hard_w16_note;

  SELECT note_text, note_value
  INTO step_text, step_value
  FROM sq_hard_w16_note
  WHERE note_key = 'graph-step-pattern';

  SELECT note_text, note_value
  INTO vertex_text, vertex_value
  FROM sq_hard_w16_note
  WHERE note_key = 'graph-vertex-pivot';

  SELECT note_text, note_value
  INTO json_text, json_value
  FROM sq_hard_w16_note
  WHERE note_key = 'graph-qualify-json';

  SELECT JSON_VALUE(json_text, '$.size()' RETURNING NUMBER),
         JSON_VALUE(json_text, '$[0].to' RETURNING NUMBER),
         JSON_VALUE(json_text, '$[1].to' RETURNING NUMBER),
         JSON_VALUE(json_text, '$[1].edges')
  INTO json_size, json_first, json_second, json_edges
  FROM dual;

  IF note_rows <> 3 THEN
    RAISE_APPLICATION_ERROR(-20990, 'w16 note rows ' || note_rows);
  END IF;
  IF step_text <> '1>2:1:5/1>4:1:11/1>3:2:12'
     OR step_value <> 28 THEN
    RAISE_APPLICATION_ERROR(
      -20991,
      'w16 graph step ' || step_text || '/' || step_value
    );
  END IF;
  IF vertex_text <> 'ELEVEN=1>4:14/FIVE=1>2:30/TWO=1>2>3:54'
     OR vertex_value <> 98 THEN
    RAISE_APPLICATION_ERROR(
      -20992,
      'w16 graph vertex ' || vertex_text || '/' || vertex_value
    );
  END IF;
  IF json_size <> 2
     OR json_first <> 2
     OR json_second <> 3
     OR json_edges <> '5>7'
     OR json_value <> 17 THEN
    RAISE_APPLICATION_ERROR(
      -20993,
      'w16 graph json ' || json_text || '/' || json_value
    );
  END IF;
END;
/

--------------------------------------------------------------------------------
-- ULTRA WAVE 17 -- SQL-standard window inheritance and multiset singularity.
-- A recursive SEARCH/CYCLE owner feeds three named WINDOW specifications, each
-- inheriting a different portion of the previous specification; QUALIFY closes
-- that query block before MATCH_RECOGNIZE reinterprets the surviving rows.
-- INTERSECT ALL then preserves duplicate cardinality through a correlated
-- CROSS APPLY. These are deliberately adjacent because WINDOW names, pattern
-- variables, CTE names and APPLY aliases all occupy incompatible scopes.
--------------------------------------------------------------------------------
CREATE TABLE sq_hard_w17_edge (
  parent_id  NUMBER NOT NULL,
  child_id   NUMBER NOT NULL,
  edge_value NUMBER NOT NULL,
  CONSTRAINT sq_hard_w17_edge_pk PRIMARY KEY (parent_id, child_id),
  CONSTRAINT sq_hard_w17_edge_no_self_ck CHECK (parent_id <> child_id)
) TABLESPACE users;

CREATE TABLE sq_hard_w17_note (
  note_key   VARCHAR2(30) CONSTRAINT sq_hard_w17_note_pk PRIMARY KEY,
  note_text  VARCHAR2(700) NOT NULL,
  note_value NUMBER NOT NULL
) TABLESPACE users;

INSERT ALL
  INTO sq_hard_w17_edge (parent_id, child_id, edge_value) VALUES (1, 2, 5)
  INTO sq_hard_w17_edge (parent_id, child_id, edge_value) VALUES (1, 3, 7)
  INTO sq_hard_w17_edge (parent_id, child_id, edge_value) VALUES (2, 4, 11)
  INTO sq_hard_w17_edge (parent_id, child_id, edge_value) VALUES (3, 5, 13)
SELECT 1;

--------------------------------------------------------------------------------
-- W17-A: W_ORDERED inherits only the partition from W_DEPTH; W_RUNNING then
-- inherits that order and adds a frame. OVER w_ordered and OVER (w_ordered ...)
-- are different grammar paths, so both appear before QUALIFY. The recursive
-- traversal metadata remains visible until MATCH_RECOGNIZE creates HEAD and
-- RISING as pattern variables with names that never existed in the base table.
--------------------------------------------------------------------------------
INSERT INTO sq_hard_w17_note (note_key, note_text, note_value)
WITH
  walk (node_id, parent_id, edge_value, tree_depth, path_text) AS (
    SELECT 1, CAST(NULL AS NUMBER), 0, 0, CAST('1' AS VARCHAR2(100))
    UNION ALL
    SELECT e.child_id,
           e.parent_id,
           e.edge_value,
           w.tree_depth + 1,
           w.path_text || '>' || e.child_id
    FROM walk w
    JOIN sq_hard_w17_edge e
      ON e.parent_id = w.node_id
  )
  SEARCH DEPTH FIRST BY node_id SET traversal_order
  CYCLE node_id SET cycle_mark TO 1 DEFAULT 0,
  inherited_windows AS (
    SELECT node_id,
           parent_id,
           edge_value,
           tree_depth,
           path_text,
           traversal_order,
           SUM(edge_value) OVER w_running AS depth_running_value,
           AVG(edge_value) OVER w_depth   AS depth_average,
           ROW_NUMBER() OVER w_ordered   AS depth_position
    FROM walk
    WHERE cycle_mark = 0
    WINDOW
      w_depth AS (
        PARTITION BY tree_depth
      ),
      w_ordered AS (
        w_depth
        ORDER BY node_id
      ),
      w_running AS (
        w_ordered
        ROWS BETWEEN UNBOUNDED PRECEDING AND CURRENT ROW
      )
    QUALIFY ROW_NUMBER() OVER w_ordered <= 2
  ),
  recognized_depths AS (
    SELECT tree_depth, row_count, depth_total, last_running
    FROM inherited_windows
    MATCH_RECOGNIZE (
      PARTITION BY tree_depth
      ORDER BY node_id
      MEASURES
        COUNT(*)                  AS row_count,
        SUM(edge_value)           AS depth_total,
        LAST(depth_running_value) AS last_running
      ONE ROW PER MATCH
      AFTER MATCH SKIP PAST LAST ROW
      PATTERN (head rising*)
      DEFINE
        rising AS rising.edge_value > PREV(rising.edge_value)
    )
  )
SELECT 'window-pattern',
       LISTAGG(
         tree_depth || ':' || row_count || ':' || depth_total
         || ':' || last_running,
         '/'
       ) WITHIN GROUP (ORDER BY tree_depth),
       SUM(depth_total)
FROM recognized_depths;

--------------------------------------------------------------------------------
-- W17-B: the left and right bags deliberately contain different duplicate
-- counts. INTERSECT ALL keeps the lower multiplicity, then CROSS APPLY mints a
-- correlated scalar beside the multiset column before both are aggregated.
--------------------------------------------------------------------------------
INSERT INTO sq_hard_w17_note (note_key, note_text, note_value)
WITH
  left_bag (bag_value) AS (
    SELECT 1 UNION ALL SELECT 1 UNION ALL SELECT 1
    UNION ALL
    SELECT 2 UNION ALL SELECT 2
    UNION ALL
    SELECT 3
  ),
  right_bag (bag_value) AS (
    SELECT 1 UNION ALL SELECT 1
    UNION ALL
    SELECT 2 UNION ALL SELECT 2 UNION ALL SELECT 2
    UNION ALL
    SELECT 4
  ),
  common_bag AS (
    (SELECT bag_value FROM left_bag)
    INTERSECT ALL
    (SELECT bag_value FROM right_bag)
  )
SELECT 'multiset-apply',
       LISTAGG(
         c.bag_value || ':' || projected.projected_value,
         '/'
       ) WITHIN GROUP (ORDER BY c.bag_value, projected.projected_value),
       SUM(projected.projected_value)
FROM common_bag c
CROSS APPLY (
  SELECT c.bag_value * 10
         + COUNT(*) OVER () AS projected_value
) projected;

COMMIT;

--------------------------------------------------------------------------------
-- Wave-17 self-verification.
--------------------------------------------------------------------------------
DECLARE
  note_rows       PLS_INTEGER;
  window_text     VARCHAR2(700);
  window_value    NUMBER;
  multiset_text   VARCHAR2(700);
  multiset_value  NUMBER;
BEGIN
  SELECT COUNT(*) INTO note_rows FROM sq_hard_w17_note;

  SELECT note_text, note_value
  INTO window_text, window_value
  FROM sq_hard_w17_note
  WHERE note_key = 'window-pattern';

  SELECT note_text, note_value
  INTO multiset_text, multiset_value
  FROM sq_hard_w17_note
  WHERE note_key = 'multiset-apply';

  IF note_rows <> 2 THEN
    RAISE_APPLICATION_ERROR(-20994, 'w17 note rows ' || note_rows);
  END IF;
  IF window_text <> '0:1:0:0/1:2:12:12/2:2:24:24'
     OR window_value <> 36 THEN
    RAISE_APPLICATION_ERROR(
      -20995,
      'w17 inherited windows ' || window_text || '/' || window_value
    );
  END IF;
  IF multiset_text <> '1:11/1:11/2:21/2:21'
     OR multiset_value <> 64 THEN
    RAISE_APPLICATION_ERROR(
      -20996,
      'w17 multiset apply ' || multiset_text || '/' || multiset_value
    );
  END IF;
END;
/

--------------------------------------------------------------------------------
-- ULTRA WAVE 18 -- five-grammar scope-collapse singularity.
-- JSON_TABLE first turns array positions into STEP_NO and AMOUNT. MODEL then
-- reinterprets STEP_NO as a cell coordinate while CV() reaches the previous
-- calculated cell. MATCH_RECOGNIZE immediately reinterprets the modeled rows as
-- START_ROW/RISE pattern variables. Three inherited named windows finally rank
-- and frame those pattern rows before QUALIFY removes no rows. The same names
-- therefore travel through JSON paths, model dimensions, pattern variables,
-- window specifications and the final aggregate without sharing a namespace.
--------------------------------------------------------------------------------
BEGIN
  EXECUTE IMMEDIATE 'DROP TABLE sq_hard_w18_note PURGE';
EXCEPTION
  WHEN OTHERS THEN
    IF SQLCODE <> -942 THEN
      RAISE;
    END IF;
END;
/

BEGIN
  EXECUTE IMMEDIATE 'DROP TABLE sq_hard_w18_stream PURGE';
EXCEPTION
  WHEN OTHERS THEN
    IF SQLCODE <> -942 THEN
      RAISE;
    END IF;
END;
/

CREATE TABLE sq_hard_w18_stream (
  stream_id NUMBER CONSTRAINT sq_hard_w18_stream_pk PRIMARY KEY,
  payload   JSON NOT NULL
) TABLESPACE users;

CREATE TABLE sq_hard_w18_note (
  note_key   VARCHAR2(30) CONSTRAINT sq_hard_w18_note_pk PRIMARY KEY,
  note_text  VARCHAR2(1000) NOT NULL,
  note_value NUMBER NOT NULL
) TABLESPACE users;

INSERT ALL
  INTO sq_hard_w18_stream (stream_id, payload)
    VALUES (
      1,
      JSON_OBJECT(
        'label' VALUE 'alpha',
        'values' VALUE JSON_ARRAY(3, 5, 9)
      )
    )
  INTO sq_hard_w18_stream (stream_id, payload)
    VALUES (
      2,
      JSON_OBJECT(
        'label' VALUE 'beta',
        'values' VALUE JSON_ARRAY(2, 4, 8)
      )
    )
SELECT 1;

--------------------------------------------------------------------------------
-- W18-A: every CTE changes the grammar of the identifiers emitted by the one
-- before it. RUNNING_AMOUNT is a MODEL measure, ROW_CLASS is a pattern measure,
-- and WINDOW_POSITION / WINDOW_RUNNING are analytic aliases consumed only
-- after QUALIFY has closed the inherited-window clause.
--------------------------------------------------------------------------------
INSERT INTO sq_hard_w18_note (note_key, note_text, note_value)
WITH
  expanded AS (
    SELECT s.stream_id,
           j.stream_label,
           j.step_no,
           j.amount
    FROM sq_hard_w18_stream s
    CROSS APPLY JSON_TABLE(
      s.payload,
      '$'
      COLUMNS (
        stream_label VARCHAR2(20) PATH '$.label',
        NESTED PATH '$.values[*]' COLUMNS (
          step_no FOR ORDINALITY,
          amount  NUMBER PATH '$'
        )
      )
    ) j
  ),
  modeled AS (
    SELECT stream_id,
           stream_label,
           step_no,
           amount,
           running_amount
    FROM expanded
    MODEL
      PARTITION BY (stream_id, stream_label)
      DIMENSION BY (step_no)
      MEASURES (
        amount,
        CAST(0 AS NUMBER) AS running_amount
      )
      RULES UPDATE SEQUENTIAL ORDER (
        running_amount[ANY] =
          amount[CV(step_no)]
          + NVL(running_amount[CV(step_no) - 1], 0)
      )
  ),
  patterned AS (
    SELECT stream_id,
           stream_label,
           step_no,
           amount,
           running_amount,
           row_class,
           match_no,
           pattern_running
    FROM modeled
    MATCH_RECOGNIZE (
      PARTITION BY stream_id, stream_label
      ORDER BY step_no
      MEASURES
        CLASSIFIER()        AS row_class,
        MATCH_NUMBER()      AS match_no,
        RUNNING SUM(amount) AS pattern_running
      ALL ROWS PER MATCH
      AFTER MATCH SKIP PAST LAST ROW
      PATTERN (start_row rise+)
      DEFINE
        rise AS rise.amount > PREV(rise.amount)
    )
  ),
  qualified AS (
    SELECT stream_id,
           stream_label,
           step_no,
           amount,
           running_amount,
           row_class,
           match_no,
           pattern_running,
           ROW_NUMBER() OVER w_ordered AS window_position,
           SUM(amount) OVER w_running  AS window_running
    FROM patterned
    WINDOW
      w_stream AS (
        PARTITION BY stream_id
      ),
      w_ordered AS (
        w_stream
        ORDER BY step_no
      ),
      w_running AS (
        w_ordered
        ROWS BETWEEN UNBOUNDED PRECEDING AND CURRENT ROW
      )
    QUALIFY ROW_NUMBER() OVER w_ordered <= 3
  )
SELECT 'scope-collapse',
       LISTAGG(
         stream_id || ':' || stream_label || ':' || step_no || ':'
         || running_amount || ':' || row_class || ':' || window_running,
         '/'
       ) WITHIN GROUP (ORDER BY stream_id, window_position),
       SUM(running_amount + pattern_running + window_running)
FROM qualified;

COMMIT;

--------------------------------------------------------------------------------
-- Wave-18 self-verification.
--------------------------------------------------------------------------------
DECLARE
  note_rows  PLS_INTEGER;
  scope_text VARCHAR2(1000);
  scope_sum  NUMBER;
BEGIN
  SELECT COUNT(*) INTO note_rows FROM sq_hard_w18_note;

  SELECT note_text, note_value
  INTO scope_text, scope_sum
  FROM sq_hard_w18_note
  WHERE note_key = 'scope-collapse';

  IF note_rows <> 1 THEN
    RAISE_APPLICATION_ERROR(-20997, 'w18 note rows ' || note_rows);
  END IF;
  IF scope_text <>
       '1:alpha:1:3:START_ROW:3/'
       || '1:alpha:2:8:RISE:8/'
       || '1:alpha:3:17:RISE:17/'
       || '2:beta:1:2:START_ROW:2/'
       || '2:beta:2:6:RISE:6/'
       || '2:beta:3:14:RISE:14'
     OR scope_sum <> 150 THEN
    RAISE_APPLICATION_ERROR(
      -20998,
      'w18 scope collapse ' || scope_text || '/' || scope_sum
    );
  END IF;
END;
/

--------------------------------------------------------------------------------
-- ULTRA WAVE 19 -- JSON-relational duality-view document/relational singularity.
-- A nested duality document remains writable while its fields still map to two
-- normalized tables. One JSON_TABLE crosses two NESTED PATH levels, LISTAGG
-- restores each event after the tag fan-out, inherited WINDOW specifications
-- carry the result through QUALIFY, MATCH_RECOGNIZE reinterprets the surviving
-- aliases as pattern variables, and JSON_OBJECTAGG closes the document loop.
-- The update and assertions prove that the JSON and relational views did not
-- merely parse: both representations changed and remained mutually consistent.
--------------------------------------------------------------------------------
BEGIN
  EXECUTE IMMEDIATE 'DROP VIEW sq_hard_w19_dv';
EXCEPTION
  WHEN OTHERS THEN
    IF SQLCODE <> -942 THEN
      RAISE;
    END IF;
END;
/

BEGIN
  EXECUTE IMMEDIATE 'DROP TABLE sq_hard_w19_note PURGE';
EXCEPTION
  WHEN OTHERS THEN
    IF SQLCODE <> -942 THEN
      RAISE;
    END IF;
END;
/

BEGIN
  EXECUTE IMMEDIATE 'DROP TABLE sq_hard_w19_event PURGE';
EXCEPTION
  WHEN OTHERS THEN
    IF SQLCODE <> -942 THEN
      RAISE;
    END IF;
END;
/

BEGIN
  EXECUTE IMMEDIATE 'DROP TABLE sq_hard_w19_stream PURGE';
EXCEPTION
  WHEN OTHERS THEN
    IF SQLCODE <> -942 THEN
      RAISE;
    END IF;
END;
/

CREATE TABLE sq_hard_w19_stream (
  stream_id   NUMBER
    CONSTRAINT sq_hard_w19_stream_pk PRIMARY KEY,
  stream_name VARCHAR2(30) NOT NULL
) TABLESPACE users;

CREATE TABLE sq_hard_w19_event (
  event_id  NUMBER
    CONSTRAINT sq_hard_w19_event_pk PRIMARY KEY,
  stream_id NUMBER NOT NULL,
  amount    NUMBER NOT NULL,
  payload   JSON NOT NULL,
  CONSTRAINT sq_hard_w19_event_stream_fk
    FOREIGN KEY (stream_id)
    REFERENCES sq_hard_w19_stream (stream_id)
) TABLESPACE users;

CREATE TABLE sq_hard_w19_note (
  note_key   VARCHAR2(30)
    CONSTRAINT sq_hard_w19_note_pk PRIMARY KEY,
  note_text  VARCHAR2(1000) NOT NULL,
  note_value NUMBER NOT NULL
) TABLESPACE users;

INSERT ALL
  INTO sq_hard_w19_stream (stream_id, stream_name) VALUES (1, 'alpha')
  INTO sq_hard_w19_stream (stream_id, stream_name) VALUES (2, 'beta')
SELECT 1;

INSERT ALL
  INTO sq_hard_w19_event (event_id, stream_id, amount, payload)
    VALUES (101, 1, 7, JSON('{"tags":["sql","json"]}'))
  INTO sq_hard_w19_event (event_id, stream_id, amount, payload)
    VALUES (102, 1, 11, JSON('{"tags":["format"]}'))
  INTO sq_hard_w19_event (event_id, stream_id, amount, payload)
    VALUES (103, 1, 13, JSON('{"tags":["parser","sql"]}'))
  INTO sq_hard_w19_event (event_id, stream_id, amount, payload)
    VALUES (201, 2, 5, JSON('{"tags":["edge"]}'))
  INTO sq_hard_w19_event (event_id, stream_id, amount, payload)
    VALUES (202, 2, 3, JSON('{"tags":["rollback"]}'))
SELECT 1;

--------------------------------------------------------------------------------
-- W19-A: JSON { ... } and JSON [ SELECT ... ] belong to duality-view DDL, not
-- ordinary JSON constructors. WITH UPDATE INSERT DELETE annotates each table
-- owner independently; the nested WHERE is the relational parent/child edge.
--------------------------------------------------------------------------------
CREATE JSON RELATIONAL DUALITY VIEW sq_hard_w19_dv AS
SELECT JSON {
  '_id' : stream_owner.stream_id,
  'name' : stream_owner.stream_name,
  'events' : [
    SELECT JSON {
      'eventId' : event_owner.event_id,
      'amount' : event_owner.amount,
      'payload' : event_owner.payload
    }
    FROM sq_hard_w19_event event_owner WITH INSERT UPDATE DELETE
    WHERE event_owner.stream_id = stream_owner.stream_id
  ]
}
FROM sq_hard_w19_stream stream_owner WITH UPDATE INSERT DELETE;

--------------------------------------------------------------------------------
-- W19-B: DML targets the duality document's DATA column. JSON_TRANSFORM changes
-- the root field while JSON_VALUE in the predicate resolves the generated _id.
--------------------------------------------------------------------------------
UPDATE sq_hard_w19_dv document_owner
SET data = JSON_TRANSFORM(
      document_owner.data,
      SET '$.name' = 'alpha-updated'
    )
WHERE JSON_VALUE(
        document_owner.data,
        '$._id' RETURNING NUMBER
      ) = 1;

--------------------------------------------------------------------------------
-- W19-C: the single JSON_TABLE owns root columns, an event array, and a tag
-- array nested inside every event payload. EVENT_ROLLUP collapses only the
-- innermost expansion. Three inherited windows then cross QUALIFY before
-- MATCH_RECOGNIZE treats SEED and RISE as a new namespace. The final aggregate
-- returns native JSON, which JSON_SERIALIZE converts for the assertion table.
--------------------------------------------------------------------------------
INSERT INTO sq_hard_w19_note (note_key, note_text, note_value)
WITH
  expanded AS (
    SELECT nested_row.stream_id,
           nested_row.stream_name,
           nested_row.event_no,
           nested_row.event_id,
           nested_row.amount,
           nested_row.tag_no,
           nested_row.tag_name
    FROM sq_hard_w19_dv document_owner
    CROSS APPLY JSON_TABLE(
      document_owner.data,
      '$'
      COLUMNS (
        stream_id   NUMBER       PATH '$._id',
        stream_name VARCHAR2(30) PATH '$.name',
        NESTED PATH '$.events[*]' COLUMNS (
          event_no FOR ORDINALITY,
          event_id NUMBER PATH '$.eventId',
          amount   NUMBER PATH '$.amount',
          NESTED PATH '$.payload.tags[*]' COLUMNS (
            tag_no   FOR ORDINALITY,
            tag_name VARCHAR2(20) PATH '$'
          )
        )
      )
    ) nested_row
  ),
  event_rollup AS (
    SELECT expanded.stream_id,
           expanded.stream_name,
           expanded.event_no,
           expanded.event_id,
           expanded.amount,
           LISTAGG(expanded.tag_name, ',')
             WITHIN GROUP (ORDER BY expanded.tag_no) AS tag_shape
    FROM expanded
    GROUP BY expanded.stream_id,
             expanded.stream_name,
             expanded.event_no,
             expanded.event_id,
             expanded.amount
  ),
  event_windows AS (
    SELECT event_rollup.*,
           SUM(amount) OVER w_running       AS running_amount,
           ROW_NUMBER() OVER w_ordered      AS event_position
    FROM event_rollup
    WINDOW
      w_stream AS (
        PARTITION BY stream_id
      ),
      w_ordered AS (
        w_stream
        ORDER BY event_id
      ),
      w_running AS (
        w_ordered
        ROWS BETWEEN UNBOUNDED PRECEDING AND CURRENT ROW
      )
    QUALIFY ROW_NUMBER() OVER w_ordered <= 3
  ),
  patterned AS (
    SELECT stream_id,
           stream_name,
           first_id,
           last_id,
           event_count,
           event_total,
           last_running
    FROM event_windows
    MATCH_RECOGNIZE (
      PARTITION BY stream_id, stream_name
      ORDER BY event_id
      MEASURES
        FIRST(event_id)      AS first_id,
        LAST(event_id)       AS last_id,
        COUNT(*)             AS event_count,
        SUM(amount)          AS event_total,
        LAST(running_amount) AS last_running
      ONE ROW PER MATCH
      AFTER MATCH SKIP PAST LAST ROW
      PATTERN (seed rise+)
      DEFINE
        rise AS rise.amount > PREV(rise.amount)
    )
  )
SELECT 'duality-pattern',
       JSON_SERIALIZE(
         JSON_OBJECTAGG(
           KEY TO_CHAR(stream_id)
           VALUE JSON_OBJECT(
             'name'    VALUE stream_name,
             'first'   VALUE first_id,
             'last'    VALUE last_id,
             'count'   VALUE event_count,
             'total'   VALUE event_total,
             'running' VALUE last_running
             RETURNING JSON
           )
           RETURNING JSON
         )
         RETURNING VARCHAR2(1000)
       ),
       SUM(event_total)
FROM patterned;

COMMIT;

--------------------------------------------------------------------------------
-- Wave-19 self-verification.
--------------------------------------------------------------------------------
DECLARE
  note_rows       PLS_INTEGER;
  event_rows      PLS_INTEGER;
  stream_name     VARCHAR2(30);
  duality_text    VARCHAR2(1000);
  duality_total   NUMBER;
  pattern_name    VARCHAR2(30);
  pattern_first   NUMBER;
  pattern_last    NUMBER;
  pattern_count   NUMBER;
  pattern_total   NUMBER;
  pattern_running NUMBER;
BEGIN
  SELECT COUNT(*) INTO note_rows FROM sq_hard_w19_note;
  SELECT COUNT(*) INTO event_rows FROM sq_hard_w19_event;
  SELECT stream_owner.stream_name INTO stream_name
  FROM sq_hard_w19_stream stream_owner
  WHERE stream_owner.stream_id = 1;

  SELECT note_text, note_value
  INTO duality_text, duality_total
  FROM sq_hard_w19_note
  WHERE note_key = 'duality-pattern';

  SELECT JSON_VALUE(duality_text, '$."1".name'),
         JSON_VALUE(duality_text, '$."1".first' RETURNING NUMBER),
         JSON_VALUE(duality_text, '$."1".last' RETURNING NUMBER),
         JSON_VALUE(duality_text, '$."1".count' RETURNING NUMBER),
         JSON_VALUE(duality_text, '$."1".total' RETURNING NUMBER),
         JSON_VALUE(duality_text, '$."1".running' RETURNING NUMBER)
  INTO pattern_name,
       pattern_first,
       pattern_last,
       pattern_count,
       pattern_total,
       pattern_running;

  IF note_rows <> 1 OR event_rows <> 5 THEN
    RAISE_APPLICATION_ERROR(
      -20999,
      'w19 cardinality ' || note_rows || '/' || event_rows
    );
  END IF;
  IF stream_name <> 'alpha-updated' OR pattern_name <> stream_name THEN
    RAISE_APPLICATION_ERROR(
      -20990,
      'w19 duality update ' || stream_name || '/' || pattern_name
    );
  END IF;
  IF pattern_first <> 101
     OR pattern_last <> 103
     OR pattern_count <> 3
     OR pattern_total <> 31
     OR pattern_running <> 31
     OR duality_total <> 31 THEN
    RAISE_APPLICATION_ERROR(
      -20991,
      'w19 duality pattern ' || pattern_first || '/' || pattern_last || '/'
      || pattern_count || '/' || pattern_total || '/' || pattern_running || '/'
      || duality_total
    );
  END IF;
END;
/

--------------------------------------------------------------------------------
-- ULTRA WAVE 20 -- recursive-cycle, JSON pivot, and row-pattern convergence.
--
-- One hierarchy is interpreted three incompatible ways. SEARCH/CYCLE owns two
-- generated columns that do not exist in the recursive CTE declaration.
-- JSON_TABLE then fans every node into attributes, PIVOT turns the attribute
-- names into identifiers, UNPIVOT INCLUDE NULLS turns them back into rows, and
-- MATCH_RECOGNIZE assigns the same physical rows to pattern/subset namespaces.
-- Every result is persisted and verified so none of these constructs merely
-- parse without producing the intended data.
--------------------------------------------------------------------------------
BEGIN
  EXECUTE IMMEDIATE 'DROP TABLE sq_hard_w20_note PURGE';
EXCEPTION
  WHEN OTHERS THEN
    IF SQLCODE <> -942 THEN
      RAISE;
    END IF;
END;
/

BEGIN
  EXECUTE IMMEDIATE 'DROP TABLE sq_hard_w20_node CASCADE CONSTRAINTS PURGE';
EXCEPTION
  WHEN OTHERS THEN
    IF SQLCODE <> -942 THEN
      RAISE;
    END IF;
END;
/

CREATE TABLE sq_hard_w20_node (
  node_id     NUMBER
    CONSTRAINT sq_hard_w20_node_pk PRIMARY KEY,
  parent_id   NUMBER,
  node_name   VARCHAR2(30) NOT NULL,
  node_weight NUMBER NOT NULL,
  payload     JSON NOT NULL,
  CONSTRAINT sq_hard_w20_node_parent_fk
    FOREIGN KEY (parent_id)
    REFERENCES sq_hard_w20_node (node_id)
) TABLESPACE users;

CREATE TABLE sq_hard_w20_note (
  note_key   VARCHAR2(30)
    CONSTRAINT sq_hard_w20_note_pk PRIMARY KEY,
  note_text  VARCHAR2(1000) NOT NULL,
  note_value NUMBER NOT NULL
) TABLESPACE users;

INSERT ALL
  INTO sq_hard_w20_node
    (node_id, parent_id, node_name, node_weight, payload)
    VALUES (
      1,
      NULL,
      'root',
      10,
      JSON('{"attrs":[{"name":"alpha","value":2},'
           || '{"name":"beta","value":3}]}')
    )
  INTO sq_hard_w20_node
    (node_id, parent_id, node_name, node_weight, payload)
    VALUES (
      2,
      1,
      'left',
      4,
      JSON('{"attrs":[{"name":"alpha","value":5},'
           || '{"name":"gamma","value":7}]}')
    )
  INTO sq_hard_w20_node
    (node_id, parent_id, node_name, node_weight, payload)
    VALUES (
      3,
      1,
      'right',
      6,
      JSON('{"attrs":[{"name":"beta","value":11}]}')
    )
  INTO sq_hard_w20_node
    (node_id, parent_id, node_name, node_weight, payload)
    VALUES (
      4,
      2,
      'left-leaf',
      3,
      JSON('{"attrs":[{"name":"alpha","value":13},'
           || '{"name":"beta","value":17}]}')
    )
  INTO sq_hard_w20_node
    (node_id, parent_id, node_name, node_weight, payload)
    VALUES (
      5,
      2,
      'middle-leaf',
      5,
      JSON('{"attrs":[{"name":"gamma","value":19}]}')
    )
  INTO sq_hard_w20_node
    (node_id, parent_id, node_name, node_weight, payload)
    VALUES (
      6,
      3,
      'right-leaf',
      7,
      JSON('{"attrs":[{"name":"alpha","value":23},'
           || '{"name":"gamma","value":29}]}')
    )
SELECT 1;

--------------------------------------------------------------------------------
-- W20-A: VISIT_NO and IS_CYCLE are created by SEARCH/CYCLE after the recursive
-- CTE closes. ATTR_OWNER is lateral to NODE_OWNER, while LISTAGG must order by
-- the generated traversal key and the JSON_TABLE ordinality at the same time.
--------------------------------------------------------------------------------
INSERT INTO sq_hard_w20_note (note_key, note_text, note_value)
WITH
  tree (
    root_id,
    node_id,
    parent_id,
    level_no,
    path_text
  ) AS (
    SELECT node_id,
           node_id,
           parent_id,
           1,
           TO_CHAR(node_id)
    FROM sq_hard_w20_node
    WHERE parent_id IS NULL
    UNION ALL
    SELECT tree.root_id,
           child_owner.node_id,
           child_owner.parent_id,
           tree.level_no + 1,
           tree.path_text || '/' || child_owner.node_id
    FROM tree
    JOIN sq_hard_w20_node child_owner
      ON child_owner.parent_id = tree.node_id
  )
  SEARCH DEPTH FIRST BY node_id SET visit_no
  CYCLE node_id SET is_cycle TO 1 DEFAULT 0,
  expanded AS (
    SELECT tree.node_id,
           tree.level_no,
           tree.visit_no,
           attr_owner.attr_no,
           attr_owner.attr_name,
           attr_owner.attr_value
    FROM tree
    JOIN sq_hard_w20_node node_owner
      ON node_owner.node_id = tree.node_id
    CROSS APPLY JSON_TABLE(
      node_owner.payload,
      '$.attrs[*]'
      COLUMNS (
        attr_no    FOR ORDINALITY,
        attr_name  VARCHAR2(20) PATH '$.name' ERROR ON ERROR,
        attr_value NUMBER       PATH '$.value' ERROR ON ERROR
      )
    ) attr_owner
    WHERE tree.is_cycle = 0
  )
SELECT 'recursive-json',
       LISTAGG(
         node_id || ':' || level_no || ':' ||
         attr_name || '=' || attr_value,
         '/'
       ) WITHIN GROUP (ORDER BY visit_no, attr_no),
       SUM(attr_value)
FROM expanded;

--------------------------------------------------------------------------------
-- W20-B: PIVOT creates ALPHA/BETA/GAMMA columns from JSON keys. UNPIVOT then
-- consumes those generated identifiers and deliberately preserves absent keys
-- as NULL rows, making keyword, identifier, and literal ownership overlap.
--------------------------------------------------------------------------------
INSERT INTO sq_hard_w20_note (note_key, note_text, note_value)
WITH
  attributes AS (
    SELECT node_owner.node_id,
           attr_owner.attr_name,
           attr_owner.attr_value
    FROM sq_hard_w20_node node_owner
    CROSS APPLY JSON_TABLE(
      node_owner.payload,
      '$.attrs[*]'
      COLUMNS (
        attr_name  VARCHAR2(20) PATH '$.name',
        attr_value NUMBER       PATH '$.value'
      )
    ) attr_owner
  ),
  pivoted AS (
    SELECT node_id,
           alpha,
           beta,
           gamma
    FROM attributes
    PIVOT (
      SUM(attr_value)
      FOR attr_name IN (
        'alpha' AS alpha,
        'beta'  AS beta,
        'gamma' AS gamma
      )
    )
  ),
  unpivoted AS (
    SELECT node_id,
           attr_name,
           attr_total
    FROM pivoted
    UNPIVOT INCLUDE NULLS (
      attr_total FOR attr_name IN (
        alpha AS 'alpha',
        beta  AS 'beta',
        gamma AS 'gamma'
      )
    )
  )
SELECT 'pivot-unpivot',
       LISTAGG(
         node_id || ':' || attr_name || '=' ||
         NVL(TO_CHAR(attr_total), 'null'),
         '/'
       ) WITHIN GROUP (ORDER BY node_id, attr_name),
       SUM(NVL(attr_total, 0))
FROM unpivoted;

--------------------------------------------------------------------------------
-- W20-C: LOW/HIGH are pattern variables, CLIMB is a subset containing both,
-- and NODE_WEIGHT remains an ordinary relation column outside MATCH_RECOGNIZE.
--------------------------------------------------------------------------------
INSERT INTO sq_hard_w20_note (note_key, note_text, note_value)
WITH
  pattern_rows AS (
    SELECT first_id,
           last_id,
           match_total
    FROM sq_hard_w20_node
    MATCH_RECOGNIZE (
      ORDER BY node_id
      MEASURES
        FIRST(climb.node_id)     AS first_id,
        LAST(climb.node_id)      AS last_id,
        SUM(climb.node_weight)   AS match_total
      ONE ROW PER MATCH
      AFTER MATCH SKIP PAST LAST ROW
      PATTERN (low high+)
      SUBSET climb = (low, high)
      DEFINE
        high AS high.node_weight > PREV(high.node_weight)
    )
  )
SELECT 'pattern-subset',
       LISTAGG(
         first_id || '-' || last_id || ':' || match_total,
         '/'
       ) WITHIN GROUP (ORDER BY first_id),
       SUM(match_total)
FROM pattern_rows;

--------------------------------------------------------------------------------
-- Wave-20 self-verification.
--------------------------------------------------------------------------------
DECLARE
  recursive_shape VARCHAR2(1000);
  recursive_total NUMBER;
  pivot_shape     VARCHAR2(1000);
  pivot_total     NUMBER;
  pattern_shape   VARCHAR2(1000);
  pattern_total   NUMBER;
  note_rows       PLS_INTEGER;
BEGIN
  SELECT note_text, note_value
  INTO recursive_shape, recursive_total
  FROM sq_hard_w20_note
  WHERE note_key = 'recursive-json';

  SELECT note_text, note_value
  INTO pivot_shape, pivot_total
  FROM sq_hard_w20_note
  WHERE note_key = 'pivot-unpivot';

  SELECT note_text, note_value
  INTO pattern_shape, pattern_total
  FROM sq_hard_w20_note
  WHERE note_key = 'pattern-subset';

  SELECT COUNT(*)
  INTO note_rows
  FROM sq_hard_w20_note;

  IF recursive_shape <>
       '1:1:alpha=2/1:1:beta=3/2:2:alpha=5/2:2:gamma=7/'
       || '4:3:alpha=13/4:3:beta=17/5:3:gamma=19/3:2:beta=11/'
       || '6:3:alpha=23/6:3:gamma=29'
     OR recursive_total <> 129 THEN
    RAISE_APPLICATION_ERROR(
      -20992,
      'w20 recursive-json ' || recursive_shape || '/' || recursive_total
    );
  END IF;

  IF pivot_shape <>
       '1:alpha=2/1:beta=3/1:gamma=null/'
       || '2:alpha=5/2:beta=null/2:gamma=7/'
       || '3:alpha=null/3:beta=11/3:gamma=null/'
       || '4:alpha=13/4:beta=17/4:gamma=null/'
       || '5:alpha=null/5:beta=null/5:gamma=19/'
       || '6:alpha=23/6:beta=null/6:gamma=29'
     OR pivot_total <> 129 THEN
    RAISE_APPLICATION_ERROR(
      -20993,
      'w20 pivot-unpivot ' || pivot_shape || '/' || pivot_total
    );
  END IF;

  IF pattern_shape <> '2-3:10/4-6:15'
     OR pattern_total <> 25
     OR note_rows <> 3 THEN
    RAISE_APPLICATION_ERROR(
      -20994,
      'w20 pattern-subset ' || pattern_shape || '/' || pattern_total || '/'
      || note_rows
    );
  END IF;
END;
/

--------------------------------------------------------------------------------
-- ULTRA WAVE 21: correlated JSON_TABLE rows feed INTERVAL aggregates, GROUP BY
-- ALL and QUALIFY in one query block; a direct-join DELETE then returns
-- aggregate OLD images from the deleted target rows. This deliberately places
-- table-function scope, interval arithmetic, nested aggregate analytics, DML
-- FROM ownership and transition-image qualifiers next to one another.
--------------------------------------------------------------------------------
BEGIN
  FOR ddl_text IN (
    SELECT 'DROP TABLE sq_hard_w21_note PURGE' text_value FROM dual UNION ALL
    SELECT 'DROP TABLE sq_hard_w21_task PURGE' FROM dual UNION ALL
    SELECT 'DROP TABLE sq_hard_w21_prune PURGE' FROM dual UNION ALL
    SELECT 'DROP TABLE sq_hard_w21_project PURGE' FROM dual
  ) LOOP
    BEGIN
      EXECUTE IMMEDIATE ddl_text.text_value;
    EXCEPTION
      WHEN OTHERS THEN
        IF SQLCODE <> -942 THEN
          RAISE;
        END IF;
    END;
  END LOOP;
END;
/

CREATE TABLE sq_hard_w21_project (
  project_id   NUMBER PRIMARY KEY,
  project_name VARCHAR2(20) NOT NULL
) TABLESPACE users;

CREATE TABLE sq_hard_w21_task (
  task_id    NUMBER PRIMARY KEY,
  project_id NUMBER NOT NULL
    REFERENCES sq_hard_w21_project(project_id),
  task_kind  VARCHAR2(20) NOT NULL,
  elapsed    INTERVAL DAY TO SECOND NOT NULL,
  payload    JSON NOT NULL
) TABLESPACE users;

CREATE TABLE sq_hard_w21_prune (
  task_kind VARCHAR2(20) PRIMARY KEY
) TABLESPACE users;

CREATE TABLE sq_hard_w21_note (
  note_key   VARCHAR2(40) PRIMARY KEY,
  note_text  VARCHAR2(1000) NOT NULL,
  note_value NUMBER NOT NULL
) TABLESPACE users;

INSERT ALL
  INTO sq_hard_w21_project VALUES (1, 'parser')
  INTO sq_hard_w21_project VALUES (2, 'formatter')
SELECT 1 FROM dual;

INSERT ALL
  INTO sq_hard_w21_task
    VALUES (
      1, 1, 'compile', TO_DSINTERVAL('PT1H'),
      JSON('{"tags":["hot","sql"]}')
    )
  INTO sq_hard_w21_task
    VALUES (
      2, 1, 'lint', TO_DSINTERVAL('PT2H30M'),
      JSON('{"tags":["sql","ui"]}')
    )
  INTO sq_hard_w21_task
    VALUES (
      3, 2, 'format', TO_DSINTERVAL('PT45M'),
      JSON('{"tags":["ui","pretty"]}')
    )
  INTO sq_hard_w21_task
    VALUES (
      4, 2, 'obsolete', TO_DSINTERVAL('PT15M'),
      JSON('{"tags":["old"]}')
    )
SELECT 1 FROM dual;

--------------------------------------------------------------------------------
-- W21-A: JSON_TABLE is correlated to TASK_OWNER. SUM and AVG consume an
-- INTERVAL DAY TO SECOND, while SUM(SUM(...)) crosses the aggregate/analytic
-- boundary. GROUP BY ALL infers PROJECT_NAME, then QUALIFY closes the inherited
-- relation scopes after the analytic rank has been resolved.
--------------------------------------------------------------------------------
INSERT INTO sq_hard_w21_note (note_key, note_text, note_value)
WITH expanded AS (
  SELECT project_owner.project_name,
         task_owner.task_id,
         task_owner.elapsed,
         tag_owner.tag_no,
         tag_owner.tag_name
  FROM sq_hard_w21_task task_owner
  JOIN sq_hard_w21_project project_owner
    ON project_owner.project_id = task_owner.project_id
  CROSS APPLY JSON_TABLE(
    task_owner.payload,
    '$.tags[*]'
    COLUMNS (
      tag_no FOR ORDINALITY,
      tag_name VARCHAR2(20) PATH '$'
    )
  ) tag_owner
),
grouped AS (
  SELECT project_name,
         COUNT(*) AS tag_rows,
         SUM(elapsed) AS project_elapsed,
         AVG(elapsed) AS average_elapsed,
         SUM(SUM(elapsed)) OVER () AS all_elapsed
  FROM expanded
  GROUP BY ALL
  QUALIFY ROW_NUMBER() OVER (
    ORDER BY SUM(elapsed) DESC, project_name
  ) <= 2
)
SELECT 'interval-window',
       LISTAGG(
         project_name || ':' || tag_rows || ':'
         || TO_CHAR(
              EXTRACT(DAY FROM project_elapsed) * 1440
              + EXTRACT(HOUR FROM project_elapsed) * 60
              + EXTRACT(MINUTE FROM project_elapsed),
              'FM9990'
            ) || ':'
         || TO_CHAR(
              EXTRACT(DAY FROM average_elapsed) * 1440
              + EXTRACT(HOUR FROM average_elapsed) * 60
              + EXTRACT(MINUTE FROM average_elapsed),
              'FM9990'
            ) || ':'
         || TO_CHAR(
              EXTRACT(DAY FROM all_elapsed) * 1440
              + EXTRACT(HOUR FROM all_elapsed) * 60
              + EXTRACT(MINUTE FROM all_elapsed),
              'FM9990'
            ),
         '/'
       ) WITHIN GROUP (ORDER BY project_name),
       SUM(tag_rows)
FROM grouped;

--------------------------------------------------------------------------------
-- W21-B: Oracle's direct-join DELETE owns a second FROM clause. Aggregate OLD
-- transition images return both a scalar key checksum and an INTERVAL into
-- PL/SQL variables before the surviving target rows are summarized.
--------------------------------------------------------------------------------
INSERT INTO sq_hard_w21_prune VALUES ('obsolete');

DECLARE
  deleted_id_sum  NUMBER;
  deleted_elapsed INTERVAL DAY TO SECOND;
  deleted_minutes NUMBER;
  remaining_shape VARCHAR2(100);
BEGIN
  DELETE FROM sq_hard_w21_task target_owner
  FROM sq_hard_w21_prune prune_owner
  WHERE target_owner.task_kind = prune_owner.task_kind
  RETURNING SUM(OLD target_owner.task_id),
            SUM(OLD target_owner.elapsed)
  INTO deleted_id_sum, deleted_elapsed;

  deleted_minutes :=
    EXTRACT(DAY FROM deleted_elapsed) * 1440
    + EXTRACT(HOUR FROM deleted_elapsed) * 60
    + EXTRACT(MINUTE FROM deleted_elapsed);

  SELECT LISTAGG(task_id, '/') WITHIN GROUP (ORDER BY task_id)
  INTO remaining_shape
  FROM sq_hard_w21_task;

  INSERT INTO sq_hard_w21_note (note_key, note_text, note_value)
  VALUES (
    'delete-from-old',
    'ids=' || deleted_id_sum || ',minutes=' || deleted_minutes
      || ',remaining=' || remaining_shape,
    deleted_id_sum * 100 + deleted_minutes
  );
  COMMIT;
END;
/

--------------------------------------------------------------------------------
-- Wave-21 self-verification.
--------------------------------------------------------------------------------
DECLARE
  interval_shape VARCHAR2(1000);
  interval_rows  NUMBER;
  delete_shape   VARCHAR2(1000);
  delete_value   NUMBER;
  note_rows      PLS_INTEGER;
  task_rows      PLS_INTEGER;
BEGIN
  SELECT note_text, note_value
  INTO interval_shape, interval_rows
  FROM sq_hard_w21_note
  WHERE note_key = 'interval-window';

  SELECT note_text, note_value
  INTO delete_shape, delete_value
  FROM sq_hard_w21_note
  WHERE note_key = 'delete-from-old';

  SELECT COUNT(*) INTO note_rows FROM sq_hard_w21_note;
  SELECT COUNT(*) INTO task_rows FROM sq_hard_w21_task;

  IF interval_shape <>
       'formatter:3:105:35:525/parser:4:420:105:525'
     OR interval_rows <> 7 THEN
    RAISE_APPLICATION_ERROR(
      -20995,
      'w21 interval-window ' || interval_shape || '/' || interval_rows
    );
  END IF;

  IF delete_shape <> 'ids=4,minutes=15,remaining=1/2/3'
     OR delete_value <> 415
     OR note_rows <> 2
     OR task_rows <> 3 THEN
    RAISE_APPLICATION_ERROR(
      -20996,
      'w21 delete-from-old ' || delete_shape || '/' || delete_value || '/'
      || note_rows || '/' || task_rows
    );
  END IF;
END;
/

--------------------------------------------------------------------------------
-- ULTRA WAVE 22 -- 26ai time buckets and native-JSON ownership collision.
--
-- IF [NOT] EXISTS DDL wraps an OBJECT-constrained JSON column. Three distinct
-- JSON_TRANSFORM set/sort operators then normalize the same array before the
-- JSON constructor grammar changes from braces to a query-bearing bracket.
-- The analytical half makes native JSON a GROUP BY and ORDER BY key while two
-- TIME_BUCKET overloads, correlated JSON_TABLE, inherited windows and QUALIFY
-- all compete for the same aliases. JSON_ID and value-based CLOB output close
-- the wave with types that conventional editor fixtures rarely expose.
--------------------------------------------------------------------------------
DROP TABLE IF EXISTS sq_hard_w22_note PURGE;
DROP TABLE IF EXISTS sq_hard_w22_event PURGE;

CREATE TABLE IF NOT EXISTS sq_hard_w22_event (
  event_id    NUMBER PRIMARY KEY,
  stream_name VARCHAR2(20) NOT NULL,
  observed_at TIMESTAMP(6) NOT NULL,
  amount      NUMBER NOT NULL,
  accepted    BOOLEAN NOT NULL,
  payload     JSON (OBJECT) NOT NULL
) TABLESPACE users;

CREATE TABLE IF NOT EXISTS sq_hard_w22_note (
  note_key   VARCHAR2(40) PRIMARY KEY,
  note_text  VARCHAR2(2000) NOT NULL,
  note_value NUMBER NOT NULL
) TABLESPACE users;

INSERT ALL
  INTO sq_hard_w22_event VALUES (
    1,
    'parser',
    TIMESTAMP '2026-01-01 00:02:00',
    2,
    TRUE,
    JSON('{"tags":["hot","sql","hot","obsolete"],"score":2}')
  )
  INTO sq_hard_w22_event VALUES (
    2,
    'parser',
    TIMESTAMP '2026-01-01 00:14:00',
    3,
    FALSE,
    JSON('{"tags":["cold","sql"],"score":3}')
  )
  INTO sq_hard_w22_event VALUES (
    3,
    'formatter',
    TIMESTAMP '2026-01-01 00:17:00',
    5,
    TRUE,
    JSON('{"tags":["ui","sql"],"score":5}')
  )
  INTO sq_hard_w22_event VALUES (
    4,
    'formatter',
    TIMESTAMP '2026-01-01 00:31:00',
    7,
    TRUE,
    JSON('{"tags":["ui","pretty","obsolete"],"score":7}')
  )
SELECT 1;

--------------------------------------------------------------------------------
-- W22-A: ADD_SET, REMOVE_SET and SORT are JSON_TRANSFORM operators, not DML
-- verbs. The final JSON [ SELECT JSON { ... } ] nests two non-SQL grammars
-- around a correlated dot-notation owner and preserves subquery ordering.
--------------------------------------------------------------------------------
UPDATE sq_hard_w22_event
SET payload = JSON_TRANSFORM(
  payload,
  ADD_SET '$.tags' = 'verified'
);

UPDATE sq_hard_w22_event
SET payload = JSON_TRANSFORM(
  payload,
  REMOVE_SET '$.tags' = 'obsolete'
)
WHERE JSON_EXISTS(
  payload,
  '$.tags[*]?(@ == "obsolete")'
);

UPDATE sq_hard_w22_event
SET payload = JSON_TRANSFORM(
  payload,
  SORT '$.tags' ASC UNIQUE
);

INSERT INTO sq_hard_w22_note (note_key, note_text, note_value)
SELECT 'json-set-query',
       JSON_SERIALIZE(
         JSON [
           SELECT JSON {
             'id'   : event_owner.event_id,
             'tags' : event_owner.payload.tags
           }
           FROM sq_hard_w22_event event_owner
           ORDER BY event_owner.event_id
         ]
         RETURNING VARCHAR2(2000)
       ),
       (SELECT COUNT(*) FROM sq_hard_w22_event)
FROM dual;

--------------------------------------------------------------------------------
-- W22-B: PAYLOAD is a native JSON grouping and window-ordering expression.
-- TIME_BUCKET consumes both an INTERVAL literal and an ISO-8601 stride string;
-- START/END belong to the function calls, not a block. JSON_TABLE creates the
-- tag namespace, three inherited windows consume the collapsed rows, and
-- QUALIFY refers to the post-window relation without another query wrapper.
--------------------------------------------------------------------------------
INSERT INTO sq_hard_w22_note (note_key, note_text, note_value)
WITH normalized AS (
  SELECT event_owner.event_id,
         event_owner.stream_name,
         event_owner.observed_at,
         event_owner.amount,
         event_owner.accepted,
         event_owner.payload,
         TIME_BUCKET(
           event_owner.observed_at,
           INTERVAL '15' MINUTE,
           TIMESTAMP '2026-01-01 00:00:00',
           START
         ) AS bucket_start,
         TIME_BUCKET(
           event_owner.observed_at,
           'PT15M',
           TIMESTAMP '2026-01-01 00:00:00',
           END
         ) AS bucket_end
  FROM sq_hard_w22_event event_owner
),
expanded AS (
  SELECT normalized_owner.event_id,
         normalized_owner.stream_name,
         normalized_owner.observed_at,
         normalized_owner.amount,
         normalized_owner.accepted,
         normalized_owner.payload,
         normalized_owner.bucket_start,
         normalized_owner.bucket_end,
         tag_owner.tag_no,
         tag_owner.tag_name
  FROM normalized normalized_owner
  CROSS APPLY JSON_TABLE(
    normalized_owner.payload,
    '$.tags[*]'
    COLUMNS (
      tag_no   FOR ORDINALITY,
      tag_name VARCHAR2(20) PATH '$' ERROR ON ERROR
    )
  ) tag_owner
),
collapsed AS (
  SELECT event_id,
         stream_name,
         observed_at,
         amount,
         accepted,
         payload,
         bucket_start,
         bucket_end,
         LISTAGG(tag_name, ',')
           WITHIN GROUP (ORDER BY tag_no) AS tag_shape
  FROM expanded
  GROUP BY event_id,
           stream_name,
           observed_at,
           amount,
           accepted,
           payload,
           bucket_start,
           bucket_end
),
windowed AS (
  SELECT collapsed_owner.*,
         SUM(amount) OVER w_running AS running_amount,
         ROW_NUMBER() OVER w_payload AS payload_position
  FROM collapsed collapsed_owner
  WINDOW
    w_stream AS (PARTITION BY stream_name),
    w_ordered AS (
      w_stream ORDER BY observed_at, event_id
    ),
    w_running AS (
      w_ordered ROWS BETWEEN UNBOUNDED PRECEDING AND CURRENT ROW
    ),
    w_payload AS (
      ORDER BY payload, event_id
    )
  QUALIFY ROW_NUMBER() OVER (
    PARTITION BY event_id
    ORDER BY tag_shape
  ) = 1
)
SELECT 'bucket-json-window',
       LISTAGG(
         event_id || ':'
         || TO_CHAR(bucket_start, 'HH24:MI') || '-'
         || TO_CHAR(bucket_end, 'HH24:MI') || ':'
         || tag_shape || ':' || running_amount,
         '/'
       ) WITHIN GROUP (ORDER BY event_id),
       SUM(running_amount)
FROM windowed;

--------------------------------------------------------------------------------
-- W22-C: JSON_ID returns two RAW widths from string-literal type selectors.
-- A query argument belongs to JSON_ARRAY, JSON_OBJECT owns colon separators,
-- native JSON owns ORDER BY, and JSON_SERIALIZE returns a value-based CLOB
-- which is narrowed only after the document has been fully constructed.
--------------------------------------------------------------------------------
INSERT INTO sq_hard_w22_note (note_key, note_text, note_value)
WITH generated_ids AS (
  SELECT event_owner.event_id,
         event_owner.payload,
         JSON_ID('UUID') AS uuid_id,
         JSON_ID('OID')  AS oid_id
  FROM sq_hard_w22_event event_owner
),
document_owner AS (
  SELECT JSON_SERIALIZE(
           JSON_ARRAY(
             SELECT JSON_OBJECT(
                      'id'        : id_owner.event_id,
                      'uuidBytes' : UTL_RAW.LENGTH(id_owner.uuid_id),
                      'oidBytes'  : UTL_RAW.LENGTH(id_owner.oid_id)
                      RETURNING JSON
                    )
             FROM generated_ids id_owner
             ORDER BY id_owner.payload, id_owner.event_id
           )
           RETURNING CLOB VALUE
         ) AS document_text
  FROM dual
),
byte_owner AS (
  SELECT SUM(
           UTL_RAW.LENGTH(uuid_id) + UTL_RAW.LENGTH(oid_id)
         ) AS identifier_bytes
  FROM generated_ids
)
SELECT 'json-order-id',
       DBMS_LOB.SUBSTR(document_text, 2000, 1),
       identifier_bytes
FROM document_owner
CROSS JOIN byte_owner;

COMMIT;

--------------------------------------------------------------------------------
-- Wave-22 self-verification. The alternate JSON_ARRAY spelling deliberately
-- uses VALUE instead of colon because it is embedded in a PL/SQL static query.
--------------------------------------------------------------------------------
DECLARE
  json_set_shape    VARCHAR2(2000);
  json_set_rows     NUMBER;
  bucket_shape      VARCHAR2(2000);
  bucket_total      NUMBER;
  id_shape          VARCHAR2(2000);
  id_bytes          NUMBER;
  note_rows         PLS_INTEGER;
  alternate_document JSON;
BEGIN
  SELECT note_text, note_value
  INTO json_set_shape, json_set_rows
  FROM sq_hard_w22_note
  WHERE note_key = 'json-set-query';

  SELECT note_text, note_value
  INTO bucket_shape, bucket_total
  FROM sq_hard_w22_note
  WHERE note_key = 'bucket-json-window';

  SELECT note_text, note_value
  INTO id_shape, id_bytes
  FROM sq_hard_w22_note
  WHERE note_key = 'json-order-id';

  SELECT COUNT(*)
  INTO note_rows
  FROM sq_hard_w22_note;

  SELECT JSON_ARRAY(
           SELECT JSON_OBJECT(
                    'id' VALUE event_owner.event_id,
                    'tags' VALUE event_owner.payload.tags
                    RETURNING JSON
                  )
           FROM sq_hard_w22_event event_owner
           ORDER BY event_owner.event_id
         )
  INTO alternate_document
  FROM dual;

  IF NOT JSON_EQUAL(JSON(json_set_shape), alternate_document)
     OR json_set_rows <> 4 THEN
    RAISE_APPLICATION_ERROR(
      -20997,
      'w22 json set ' || json_set_shape || '/' || json_set_rows
    );
  END IF;

  IF bucket_shape <>
       '1:00:00-00:15:hot,sql,verified:2/'
       || '2:00:00-00:15:cold,sql,verified:5/'
       || '3:00:15-00:30:sql,ui,verified:5/'
       || '4:00:30-00:45:pretty,ui,verified:12'
     OR bucket_total <> 24 THEN
    RAISE_APPLICATION_ERROR(
      -20998,
      'w22 bucket ' || bucket_shape || '/' || bucket_total
    );
  END IF;

  IF id_shape <>
       '[{"id":1,"uuidBytes":16,"oidBytes":12},'
       || '{"id":2,"uuidBytes":16,"oidBytes":12},'
       || '{"id":3,"uuidBytes":16,"oidBytes":12},'
       || '{"id":4,"uuidBytes":16,"oidBytes":12}]'
     OR id_bytes <> 112
     OR note_rows <> 3 THEN
    RAISE_APPLICATION_ERROR(
      -20999,
      'w22 ids ' || id_shape || '/' || id_bytes || '/' || note_rows
    );
  END IF;
END;
/

--------------------------------------------------------------------------------
-- ULTRA WAVE 23 -- SQL/JSON path-language and SQL-BOOLEAN scope singularity.
--
-- JSON_TRANSFORM opens a second expression language inside SQL string literals:
-- DECODE / CASE, path variables, arithmetic, NESTED PATH and relative @ paths
-- all mutate one native JSON value. JSON_TABLE then projects those additions
-- back into SQL while an OUTER APPLY preserves an empty array. Separately,
-- EMPTY STRING ON NULL changes JSON null generation and SQL BOOLEAN values move
-- through BULK COLLECT into a PL/SQL-only collection before SQL's four
-- IS [NOT] TRUE/FALSE predicates classify the same three-valued rows.
--------------------------------------------------------------------------------
DROP TABLE IF EXISTS sq_hard_w23_note PURGE;
DROP TABLE IF EXISTS sq_hard_w23_order PURGE;

CREATE TABLE sq_hard_w23_order (
  order_id NUMBER PRIMARY KEY,
  active   BOOLEAN,
  payload  JSON (OBJECT) NOT NULL
) TABLESPACE users;

CREATE TABLE sq_hard_w23_note (
  note_key   VARCHAR2(40) PRIMARY KEY,
  note_text  VARCHAR2(2000) NOT NULL,
  note_value NUMBER NOT NULL
) TABLESPACE users;

INSERT ALL
  INTO sq_hard_w23_order VALUES (
    1,
    TRUE,
    JSON(
      '{"kind":"alpha","priority":9,'
      || '"items":[{"sku":"a","qty":2,"price":3},'
      || '{"sku":"b","qty":4,"price":5}]}'
    )
  )
  INTO sq_hard_w23_order VALUES (
    2,
    FALSE,
    JSON(
      '{"kind":"beta","priority":4,'
      || '"items":[{"sku":"c","qty":1,"price":7}]}'
    )
  )
  INTO sq_hard_w23_order VALUES (
    3,
    CAST(NULL AS BOOLEAN),
    JSON('{"kind":"gamma","priority":1,"items":[]}')
  )
SELECT 1;

--------------------------------------------------------------------------------
-- W23-A: $RUNNING is a JSON_TRANSFORM path variable, not a SQL bind. The
-- quoted DECODE/CASE texts are parsed as SQL/JSON path expressions; comment
-- introducers inside their JSON strings remain data. NESTED PATH changes @
-- from the root document to each item while retaining the outer accumulator.
--------------------------------------------------------------------------------
UPDATE sq_hard_w23_order
SET payload = JSON_TRANSFORM(
  payload,
  SET '$running' = PATH '0',
  SET '$.class' = PATH
    'decode($.kind, "alpha", "hot", "beta", "cold", "--other/*json*/")',
  SET '$.band' = PATH
    'case($.priority >= 8, "high", $.priority >= 4, "mid", "#low")',
  NESTED PATH '$.items[*]' (
    SET '$running' = PATH '$running + (@.qty * @.price)',
    SET '@.lineTotal' = PATH '@.qty * @.price',
    SET '@.bulk' = PATH 'case(@.qty >= 3, true, false)'
  ),
  SET '$.total' = PATH '$running'
);

INSERT INTO sq_hard_w23_note (note_key, note_text, note_value)
WITH expanded AS (
  SELECT order_owner.order_id,
         JSON_VALUE(order_owner.payload, '$.class') AS class_name,
         JSON_VALUE(order_owner.payload, '$.band') AS band_name,
         JSON_VALUE(
           order_owner.payload,
           '$.total' RETURNING NUMBER
         ) AS order_total,
         line_owner.line_no,
         line_owner.sku,
         line_owner.line_total,
         line_owner.bulk_exists
  FROM sq_hard_w23_order order_owner
  OUTER APPLY JSON_TABLE(
    order_owner.payload,
    '$.items[*]'
    COLUMNS (
      line_no     FOR ORDINALITY,
      sku         VARCHAR2(20) PATH '$.sku',
      line_total  NUMBER PATH '$.lineTotal',
      bulk_exists NUMBER EXISTS PATH '$?(@.bulk == true)'
    )
  ) line_owner
),
collapsed AS (
  SELECT order_id,
         class_name,
         band_name,
         order_total,
         COUNT(line_no) AS line_rows,
         SUM(
           CASE WHEN bulk_exists = 1 THEN 1 ELSE 0 END
         ) AS bulk_rows
  FROM expanded
  GROUP BY order_id, class_name, band_name, order_total
)
SELECT 'path-transform',
       LISTAGG(
         order_id || ':' || class_name || ':' || band_name || ':'
         || order_total || ':' || line_rows || ':' || bulk_rows,
         '/'
       ) WITHIN GROUP (ORDER BY order_id),
       SUM(order_total)
FROM collapsed;

--------------------------------------------------------------------------------
-- W23-B: EMPTY STRING ON NULL follows the entry list rather than either entry,
-- so even a numeric SQL NULL becomes a JSON string. JSON_SCALAR uses the same
-- clause inside JSON_SERIALIZE and must produce the two-character token "".
--------------------------------------------------------------------------------
INSERT INTO sq_hard_w23_note (note_key, note_text, note_value)
SELECT 'empty-string-json',
       JSON_SERIALIZE(
         JSON_OBJECT(
           'objectEmpty': CAST(NULL AS VARCHAR2(1)),
           'numberEmpty': CAST(NULL AS NUMBER)
           EMPTY STRING ON NULL
           RETURNING JSON
         )
         RETURNING VARCHAR2(200)
       ),
       LENGTH(
         JSON_SERIALIZE(
           JSON_SCALAR(
             CAST(NULL AS VARCHAR2(1))
             EMPTY STRING ON NULL
           )
           RETURNING VARCHAR2(20)
         )
       )
FROM dual;

--------------------------------------------------------------------------------
-- W23-C: BOOLEAN is simultaneously a SQL column type, a PL/SQL collection
-- element, two aggregate return values and the operand of SQL predicates.
-- NULL stays UNKNOWN in the collection while IS NOT TRUE / IS NOT FALSE include
-- it on opposite sides of SQL's three-valued truth table.
--------------------------------------------------------------------------------
DECLARE
  TYPE boolean_bag_t IS TABLE OF BOOLEAN;
  flags          boolean_bag_t;
  all_flag       BOOLEAN;
  any_flag       BOOLEAN;
  flag_shape     VARCHAR2(100);
  true_rows      PLS_INTEGER;
  false_rows     PLS_INTEGER;
  not_true_rows  PLS_INTEGER;
  not_false_rows PLS_INTEGER;
BEGIN
  SELECT active
  BULK COLLECT INTO flags
  FROM sq_hard_w23_order
  ORDER BY order_id;

  SELECT BOOLEAN_AND_AGG(active),
         BOOLEAN_OR_AGG(active)
  INTO all_flag, any_flag
  FROM sq_hard_w23_order;

  FOR i IN 1 .. flags.COUNT LOOP
    IF flags(i) IS NULL THEN
      flag_shape := flag_shape || 'U';
    ELSIF flags(i) THEN
      flag_shape := flag_shape || 'T';
    ELSE
      flag_shape := flag_shape || 'F';
    END IF;
  END LOOP;

  IF all_flag THEN
    flag_shape := flag_shape || ':A=T';
  ELSIF NOT all_flag THEN
    flag_shape := flag_shape || ':A=F';
  ELSE
    flag_shape := flag_shape || ':A=U';
  END IF;

  IF any_flag THEN
    flag_shape := flag_shape || ':O=T';
  ELSIF NOT any_flag THEN
    flag_shape := flag_shape || ':O=F';
  ELSE
    flag_shape := flag_shape || ':O=U';
  END IF;

  SELECT SUM(CASE WHEN active IS TRUE THEN 1 ELSE 0 END),
         SUM(CASE WHEN active IS FALSE THEN 1 ELSE 0 END),
         SUM(CASE WHEN active IS NOT TRUE THEN 1 ELSE 0 END),
         SUM(CASE WHEN active IS NOT FALSE THEN 1 ELSE 0 END)
  INTO true_rows, false_rows, not_true_rows, not_false_rows
  FROM sq_hard_w23_order;

  INSERT INTO sq_hard_w23_note (note_key, note_text, note_value)
  VALUES (
    'boolean-bulk',
    flag_shape || ':T=' || true_rows || ':F=' || false_rows
      || ':NT=' || not_true_rows || ':NF=' || not_false_rows,
    true_rows * 1000 + false_rows * 100
      + not_true_rows * 10 + not_false_rows
  );
END;
/

COMMIT;

--------------------------------------------------------------------------------
-- Wave-23 self-verification.
--------------------------------------------------------------------------------
DECLARE
  path_shape    VARCHAR2(2000);
  path_total    NUMBER;
  empty_shape   VARCHAR2(2000);
  empty_bytes   NUMBER;
  boolean_shape VARCHAR2(2000);
  boolean_score NUMBER;
  note_rows     PLS_INTEGER;
BEGIN
  SELECT note_text, note_value
  INTO path_shape, path_total
  FROM sq_hard_w23_note
  WHERE note_key = 'path-transform';

  SELECT note_text, note_value
  INTO empty_shape, empty_bytes
  FROM sq_hard_w23_note
  WHERE note_key = 'empty-string-json';

  SELECT note_text, note_value
  INTO boolean_shape, boolean_score
  FROM sq_hard_w23_note
  WHERE note_key = 'boolean-bulk';

  SELECT COUNT(*)
  INTO note_rows
  FROM sq_hard_w23_note;

  IF path_shape <>
       '1:hot:high:26:2:1/2:cold:mid:7:1:0/'
       || '3:--other/*json*/:#low:0:0:0'
     OR path_total <> 33 THEN
    RAISE_APPLICATION_ERROR(
      -20991,
      'w23 path ' || path_shape || '/' || path_total
    );
  END IF;

  IF NOT JSON_EQUAL(
           JSON(empty_shape),
           JSON('{"objectEmpty":"","numberEmpty":""}')
         )
     OR empty_bytes <> 2 THEN
    RAISE_APPLICATION_ERROR(
      -20992,
      'w23 empty ' || empty_shape || '/' || empty_bytes
    );
  END IF;

  IF boolean_shape <> 'TFU:A=F:O=T:T=1:F=1:NT=2:NF=2'
     OR boolean_score <> 1122
     OR note_rows <> 3 THEN
    RAISE_APPLICATION_ERROR(
      -20993,
      'w23 boolean ' || boolean_shape || '/' || boolean_score
      || '/' || note_rows
    );
  END IF;
END;
/

--------------------------------------------------------------------------------
-- ULTRA WAVE 24 -- external-loader, recursive-NESTED, and collection-row-source
-- grammar singularity.
--
-- ORACLE_LOADER embeds a second, line-oriented language inside CREATE TABLE;
-- EXTERNAL MODIFY then replaces three of that object's clauses for one query.
-- A NESTED shorthand recursively opens JSON COLUMNS scopes whose sibling arrays
-- are union-joined, while SAMPLE BLOCK, IS [NOT] EMPTY, and legacy THE(subquery)
-- force the same collection tokens to alternate between condition and row-source
-- grammar. Every branch is persisted and verified independently.
--------------------------------------------------------------------------------
DROP TABLE IF EXISTS sq_hard_w24_ext PURGE;
DROP TABLE IF EXISTS sq_hard_w24_doc PURGE;
DROP TABLE IF EXISTS sq_hard_w24_bag PURGE;
DROP TABLE IF EXISTS sq_hard_w24_note PURGE;
DROP TYPE IF EXISTS sq_hard_w24_num_tab FORCE;
DROP DIRECTORY IF EXISTS sq_hard_w24_dir;

CREATE TABLE sq_hard_w24_note (
  note_key   VARCHAR2(40) CONSTRAINT sq_hard_w24_note_pk PRIMARY KEY,
  note_text  VARCHAR2(2000) NOT NULL,
  note_value NUMBER NOT NULL
) TABLESPACE users;

--------------------------------------------------------------------------------
-- W24-A: ACCESS PARAMETERS is not SQL even though RECORDS, CHARACTERSET,
-- BADFILE, LOGFILE, FIELDS, MISSING, NULL, INTEGER and CHAR are SQL-looking
-- tokens. Comment introducers in the physical payload remain ordinary data.
-- EXTERNAL MODIFY belongs to the table reference and closes before LISTAGG's
-- WITHIN GROUP; none of its clauses modifies the dictionary definition.
--------------------------------------------------------------------------------
CREATE OR REPLACE DIRECTORY sq_hard_w24_dir AS '/tmp';

DECLARE
  output_file UTL_FILE.FILE_TYPE;
BEGIN
  output_file := UTL_FILE.FOPEN(
    'SQ_HARD_W24_DIR',
    'sq_hard_w24.csv',
    'W',
    32767
  );
  UTL_FILE.PUT_LINE(output_file, '1|alpha|10|sql;--/*data*/#');
  UTL_FILE.PUT_LINE(output_file, '2|beta|20|format');
  UTL_FILE.PUT_LINE(output_file, '3|gamma|30|highlight');
  UTL_FILE.FCLOSE(output_file);
EXCEPTION
  WHEN OTHERS THEN
    IF UTL_FILE.IS_OPEN(output_file) THEN
      UTL_FILE.FCLOSE(output_file);
    END IF;
    RAISE;
END;
/

CREATE TABLE sq_hard_w24_ext (
  record_id   NUMBER,
  record_name VARCHAR2(30),
  amount      NUMBER,
  payload     VARCHAR2(100)
)
ORGANIZATION EXTERNAL (
  TYPE ORACLE_LOADER
  DEFAULT DIRECTORY sq_hard_w24_dir
  ACCESS PARAMETERS (
    RECORDS DELIMITED BY NEWLINE
    CHARACTERSET AL32UTF8
    BADFILE sq_hard_w24_dir:'sq_hard_w24_%p.bad'
    LOGFILE sq_hard_w24_dir:'sq_hard_w24_%p.log'
    FIELDS TERMINATED BY '|'
    MISSING FIELD VALUES ARE NULL
    (
      record_id   INTEGER EXTERNAL,
      record_name CHAR(30),
      amount      DECIMAL EXTERNAL,
      payload     CHAR(100)
    )
  )
  LOCATION ('sq_hard_w24.csv')
)
REJECT LIMIT UNLIMITED;

INSERT INTO sq_hard_w24_note (note_key, note_text, note_value)
SELECT 'external-loader',
       LISTAGG(
         record_id || ':' || record_name || ':' || amount || ':' || payload,
         '/'
       ) WITHIN GROUP (ORDER BY record_id),
       SUM(amount)
FROM sq_hard_w24_ext
EXTERNAL MODIFY (
  DEFAULT DIRECTORY sq_hard_w24_dir
  LOCATION ('sq_hard_w24.csv')
  REJECT LIMIT 0
);

--------------------------------------------------------------------------------
-- W24-B: NESTED is the implicit left-outer JSON_TABLE join, not a nested-table
-- type declaration. GROUPS owns another NESTED below it, while TAGS is its
-- sibling and therefore contributes union-join rows with every group/item
-- column null. The empty document still contributes one outer row.
--------------------------------------------------------------------------------
CREATE TABLE sq_hard_w24_doc (
  doc_id  NUMBER CONSTRAINT sq_hard_w24_doc_pk PRIMARY KEY,
  payload JSON NOT NULL
) TABLESPACE users;

INSERT INTO sq_hard_w24_doc VALUES (
  1,
  JSON(
    '{"kind":"alpha",'
    || '"groups":[{"name":"g1","items":['
    || '{"sku":"a","qty":2},{"sku":"b","qty":3}]}],'
    || '"tags":["sql","json"]}'
  )
);

INSERT INTO sq_hard_w24_doc VALUES (
  2,
  JSON('{"kind":"empty","groups":[],"tags":[]}')
);

INSERT INTO sq_hard_w24_note (note_key, note_text, note_value)
WITH expanded AS (
  SELECT document_owner.doc_id,
         nested_owner.kind_name,
         nested_owner.group_no,
         nested_owner.group_name,
         nested_owner.item_no,
         nested_owner.item_sku,
         nested_owner.item_qty,
         nested_owner.tag_no,
         nested_owner.tag_name
  FROM sq_hard_w24_doc document_owner
  NESTED payload COLUMNS (
    kind_name VARCHAR2(20) PATH '$.kind',
    NESTED PATH '$.groups[*]' COLUMNS (
      group_no FOR ORDINALITY,
      group_name VARCHAR2(20) PATH '$.name',
      NESTED PATH '$.items[*]' COLUMNS (
        item_no FOR ORDINALITY,
        item_sku VARCHAR2(20) PATH '$.sku',
        item_qty NUMBER PATH '$.qty'
      )
    ),
    NESTED PATH '$.tags[*]' COLUMNS (
      tag_no FOR ORDINALITY,
      tag_name VARCHAR2(20) PATH '$'
    )
  ) nested_owner
)
SELECT 'recursive-nested',
       LISTAGG(
         doc_id || ':' || kind_name || ':'
         || COALESCE(TO_CHAR(group_no), '-') || ':'
         || COALESCE(group_name, '-') || ':'
         || COALESCE(TO_CHAR(item_no), '-') || ':'
         || COALESCE(item_sku, '-') || ':'
         || COALESCE(TO_CHAR(item_qty), '-') || ':'
         || COALESCE(TO_CHAR(tag_no), '-') || ':'
         || COALESCE(tag_name, '-'),
         '/'
       ) WITHIN GROUP (
         ORDER BY doc_id,
                  group_no NULLS LAST,
                  item_no NULLS LAST,
                  tag_no NULLS LAST
       ),
       COUNT(*)
FROM expanded;

--------------------------------------------------------------------------------
-- W24-C: NUMS is a nested-table value in the two IS EMPTY predicates, a
-- collection expression consumed by CARDINALITY, and a relation produced by
-- THE(subquery). SAMPLE BLOCK owns BLOCK/SEED as table-reference modifiers,
-- even though both words are also valid names in other statement families.
--------------------------------------------------------------------------------
CREATE TYPE sq_hard_w24_num_tab AS TABLE OF NUMBER;
/

CREATE TABLE sq_hard_w24_bag (
  bag_id   NUMBER CONSTRAINT sq_hard_w24_bag_pk PRIMARY KEY,
  bag_name VARCHAR2(20) NOT NULL,
  nums     sq_hard_w24_num_tab
) TABLESPACE users
NESTED TABLE nums STORE AS sq_hard_w24_bag_nt;

INSERT INTO sq_hard_w24_bag
VALUES (1, 'filled', sq_hard_w24_num_tab(3, 1, 2, 2));

INSERT INTO sq_hard_w24_bag
VALUES (2, 'empty', sq_hard_w24_num_tab());

INSERT INTO sq_hard_w24_bag
VALUES (3, 'null', NULL);

COMMIT;

DECLARE
  state_shape  VARCHAR2(1000);
  legacy_shape VARCHAR2(1000);
  sampled_rows PLS_INTEGER;
  legacy_rows  PLS_INTEGER;
BEGIN
  SELECT LISTAGG(
           sampled_owner.bag_id || ':'
           || CASE
                WHEN sampled_owner.nums IS EMPTY THEN 'empty'
                WHEN sampled_owner.nums IS NOT EMPTY THEN 'filled'
                ELSE 'unknown'
              END || ':'
           || COALESCE(
                TO_CHAR(CARDINALITY(sampled_owner.nums)),
                '-'
              ),
           '/'
         ) WITHIN GROUP (ORDER BY sampled_owner.bag_id),
         COUNT(*)
  INTO state_shape, sampled_rows
  FROM sq_hard_w24_bag
       SAMPLE BLOCK (99.999999) SEED (240024) sampled_owner;

  SELECT LISTAGG(
           legacy_owner.COLUMN_VALUE,
           '/'
         ) WITHIN GROUP (ORDER BY legacy_owner.COLUMN_VALUE),
         COUNT(*)
  INTO legacy_shape, legacy_rows
  FROM THE (
    SELECT bag_owner.nums
    FROM sq_hard_w24_bag bag_owner
    WHERE bag_owner.bag_id = 1
  ) legacy_owner;

  INSERT INTO sq_hard_w24_note (note_key, note_text, note_value)
  VALUES (
    'sample-collection',
    state_shape || '|' || legacy_shape,
    sampled_rows * 100 + legacy_rows
  );
END;
/

COMMIT;

--------------------------------------------------------------------------------
-- Wave-24 self-verification.
--------------------------------------------------------------------------------
DECLARE
  external_shape VARCHAR2(2000);
  external_total NUMBER;
  nested_shape   VARCHAR2(2000);
  nested_rows    NUMBER;
  bag_shape      VARCHAR2(2000);
  bag_score      NUMBER;
  external_defs  PLS_INTEGER;
  note_rows      PLS_INTEGER;
BEGIN
  SELECT note_text, note_value
  INTO external_shape, external_total
  FROM sq_hard_w24_note
  WHERE note_key = 'external-loader';

  SELECT note_text, note_value
  INTO nested_shape, nested_rows
  FROM sq_hard_w24_note
  WHERE note_key = 'recursive-nested';

  SELECT note_text, note_value
  INTO bag_shape, bag_score
  FROM sq_hard_w24_note
  WHERE note_key = 'sample-collection';

  SELECT COUNT(*)
  INTO external_defs
  FROM user_external_tables
  WHERE table_name = 'SQ_HARD_W24_EXT'
    AND type_owner = 'SYS'
    AND type_name = 'ORACLE_LOADER';

  SELECT COUNT(*)
  INTO note_rows
  FROM sq_hard_w24_note;

  IF external_shape <>
       '1:alpha:10:sql;--/*data*/#/2:beta:20:format/'
       || '3:gamma:30:highlight'
     OR external_total <> 60
     OR external_defs <> 1 THEN
    RAISE_APPLICATION_ERROR(
      -20984,
      'w24 external ' || external_shape || '/' || external_total
      || '/' || external_defs
    );
  END IF;

  IF nested_shape <>
       '1:alpha:1:g1:1:a:2:-:-/'
       || '1:alpha:1:g1:2:b:3:-:-/'
       || '1:alpha:-:-:-:-:-:1:sql/'
       || '1:alpha:-:-:-:-:-:2:json/'
       || '2:empty:-:-:-:-:-:-:-'
     OR nested_rows <> 5 THEN
    RAISE_APPLICATION_ERROR(
      -20985,
      'w24 nested ' || nested_shape || '/' || nested_rows
    );
  END IF;

  IF bag_shape <> '1:filled:4/2:empty:0/3:unknown:-|1/2/2/3'
     OR bag_score <> 304
     OR note_rows <> 3 THEN
    RAISE_APPLICATION_ERROR(
      -20986,
      'w24 bag ' || bag_shape || '/' || bag_score || '/' || note_rows
    );
  END IF;
END;
/

--------------------------------------------------------------------------------
-- ULTRA WAVE 25 -- indexed-vector, typed-JSON, analytic-null and row-pattern
-- scope singularity.
--
-- One VECTOR value crosses four incompatible grammar owners:
--   * CREATE VECTOR INDEX owns INCLUDE, IVF clustering and TABLESPACE clauses;
--   * FETCH APPROXIMATE owns two PARTITIONS BY levels plus probe parameters;
--   * JSON_TABLE converts an ordinary JSON array into VECTOR(4, FLOAT32);
--   * MATCH_RECOGNIZE classifies rows by VECTOR_DISTANCE inside DEFINE.
-- Named-window inheritance then applies FROM LAST RESPECT NULLS to the
-- JSON-minted event columns. Every branch is deterministic and self-verifying.
--------------------------------------------------------------------------------
DROP INDEX IF EXISTS sq_hard_w25_doc_vix;
DROP TABLE IF EXISTS sq_hard_w25_doc PURGE;
DROP TABLE IF EXISTS sq_hard_w25_note PURGE;

CREATE TABLE sq_hard_w25_note (
  note_key   VARCHAR2(40) CONSTRAINT sq_hard_w25_note_pk PRIMARY KEY,
  note_text  VARCHAR2(1000) NOT NULL,
  note_value NUMBER NOT NULL
) TABLESPACE users;

CREATE TABLE sq_hard_w25_doc (
  tenant_id    NUMBER NOT NULL,
  document_id  NUMBER CONSTRAINT sq_hard_w25_doc_pk PRIMARY KEY,
  document_name VARCHAR2(30) NOT NULL,
  embedding    VECTOR(4, FLOAT32) NOT NULL,
  payload      JSON NOT NULL,
  CONSTRAINT sq_hard_w25_doc_uq UNIQUE (tenant_id, document_name)
) TABLESPACE users;

INSERT ALL
  INTO sq_hard_w25_doc (
    tenant_id, document_id, document_name, embedding, payload
  )
  VALUES (
    1, 101, 'alpha', VECTOR('[1,0,0,0]', 4, FLOAT32),
    JSON(
      '{"name":"alpha","embedding":[1,0,0,0],'
      || '"events":[{"seq":1,"score":10},{"seq":2,"score":null},'
      || '{"seq":3,"score":30}]}'
    )
  )
  INTO sq_hard_w25_doc (
    tenant_id, document_id, document_name, embedding, payload
  )
  VALUES (
    1, 102, 'beta', VECTOR('[0.9,0.1,0,0]', 4, FLOAT32),
    JSON(
      '{"name":"beta","embedding":[0.8,0.2,0,0],'
      || '"events":[{"seq":1,"score":5},{"seq":2,"score":15}]}'
    )
  )
  INTO sq_hard_w25_doc (
    tenant_id, document_id, document_name, embedding, payload
  )
  VALUES (
    1, 103, 'gamma', VECTOR('[0,1,0,0]', 4, FLOAT32),
    JSON(
      '{"name":"gamma","embedding":[0,1,0,0],"events":[]}'
    )
  )
  INTO sq_hard_w25_doc (
    tenant_id, document_id, document_name, embedding, payload
  )
  VALUES (
    2, 201, 'delta', VECTOR('[0.7,0.3,0,0]', 4, FLOAT32),
    JSON(
      '{"name":"delta","embedding":[0.6,0.4,0,0],'
      || '"events":[{"seq":1,"score":7},{"seq":2,"score":null},'
      || '{"seq":3,"score":21}]}'
    )
  )
  INTO sq_hard_w25_doc (
    tenant_id, document_id, document_name, embedding, payload
  )
  VALUES (
    2, 202, 'epsilon', VECTOR('[0,0,1,0]', 4, FLOAT32),
    JSON(
      '{"name":"epsilon","embedding":[0,0,0.9,0.1],'
      || '"events":[{"seq":1,"score":8},{"seq":2,"score":16}]}'
    )
  )
  INTO sq_hard_w25_doc (
    tenant_id, document_id, document_name, embedding, payload
  )
  VALUES (
    2, 203, 'zeta', VECTOR('[-1,0,0,0]', 4, FLOAT32),
    JSON(
      '{"name":"zeta","embedding":[-1,0,0,0],"events":[]}'
    )
  )
SELECT 1;

-- W25-A: INCLUDE columns belong to the vector index, while the repeated
-- PARTITIONS BY clauses below belong to approximate row limiting rather than
-- analytics. The accuracy tail owns its own parenthesized probe parameters.
CREATE VECTOR INDEX sq_hard_w25_doc_vix
ON sq_hard_w25_doc (embedding)
INCLUDE (tenant_id, document_name)
ORGANIZATION NEIGHBOR PARTITIONS
DISTANCE COSINE
WITH TARGET ACCURACY 90
PARAMETERS (
  TYPE IVF,
  NEIGHBOR PARTITIONS 1,
  SAMPLES_PER_PARTITION 6,
  MIN_VECTORS_PER_PARTITION 0
)
TABLESPACE users;

DECLARE
  search_shape VARCHAR2(1000);
  search_total NUMBER;
  index_rows   PLS_INTEGER;
BEGIN
  SELECT LISTAGG(
           tenant_id || ':' || document_id || ':' || document_name,
           '/'
         ) WITHIN GROUP (
           ORDER BY tenant_id, document_id
         ),
         SUM(document_id)
  INTO search_shape, search_total
  FROM (
    SELECT tenant_id,
           document_id,
           document_name
    FROM sq_hard_w25_doc
    ORDER BY VECTOR_DISTANCE(
               embedding,
               VECTOR('[1,0,0,0]', 4, FLOAT32),
               COSINE
             ),
             document_id
    FETCH APPROXIMATE FIRST
      2 PARTITIONS BY tenant_id,
      2 ROWS ONLY
    WITH TARGET ACCURACY PARAMETERS (
      NEIGHBOR PARTITION PROBES 1
    )
  );

  SELECT COUNT(*)
  INTO index_rows
  FROM user_indexes
  WHERE index_name = 'SQ_HARD_W25_DOC_VIX'
    AND index_type = 'VECTOR';

  INSERT INTO sq_hard_w25_note (note_key, note_text, note_value)
  VALUES (
    'approximate-vector-index',
    search_shape,
    search_total * 10 + index_rows
  );
END;
/

-- W25-B: JSON_TABLE mints both a scalar namespace and a typed VECTOR from the
-- same document, then its NESTED PATH creates event columns. FROM LAST belongs
-- to NTH_VALUE, RESPECT NULLS belongs to the value expression, and W_FULL
-- inherits its partition/order clauses through two named windows.
DECLARE
  analytic_shape VARCHAR2(1000);
  score_total    NUMBER;
BEGIN
  WITH expanded AS (
    SELECT document_owner.tenant_id,
           document_owner.document_id,
           event_owner.document_name,
           event_owner.event_no,
           event_owner.score_value,
           VECTOR_DISTANCE(
             event_owner.payload_embedding,
             document_owner.embedding,
             COSINE
           ) AS payload_gap
    FROM sq_hard_w25_doc document_owner
         CROSS APPLY JSON_TABLE(
           document_owner.payload,
           '$'
           COLUMNS (
             document_name VARCHAR2(30) PATH '$.name',
             payload_embedding VECTOR(4, FLOAT32) PATH '$.embedding',
             NESTED PATH '$.events[*]'
               COLUMNS (
                 event_no FOR ORDINALITY,
                 score_value NUMBER PATH '$.score'
                   NULL ON EMPTY
                   NULL ON ERROR
               )
           )
         ) event_owner
    WHERE event_owner.event_no IS NOT NULL
  ),
  analytic AS (
    SELECT tenant_id,
           document_id,
           document_name,
           event_no,
           score_value,
           payload_gap,
           FIRST_VALUE(score_value)
             RESPECT NULLS OVER w_full AS first_respected,
           NTH_VALUE(score_value, 2)
             FROM LAST RESPECT NULLS OVER w_full AS second_from_last,
           LAST_VALUE(score_value)
             IGNORE NULLS OVER w_full AS last_nonnull
    FROM expanded
    WINDOW
      w_document AS (
        PARTITION BY tenant_id, document_id
      ),
      w_ordered AS (
        w_document
        ORDER BY event_no
      ),
      w_full AS (
        w_ordered
        ROWS BETWEEN UNBOUNDED PRECEDING AND UNBOUNDED FOLLOWING
      )
    QUALIFY ROW_NUMBER() OVER (
      PARTITION BY tenant_id, document_id
      ORDER BY event_no DESC
    ) = 1
  )
  SELECT LISTAGG(
           document_id || ':' ||
           first_respected || ':' ||
           COALESCE(TO_CHAR(second_from_last), '-') || ':' ||
           last_nonnull,
           '/'
         ) WITHIN GROUP (ORDER BY tenant_id, document_id),
         SUM(score_value)
  INTO analytic_shape, score_total
  FROM analytic
  WHERE payload_gap >= 0;

  INSERT INTO sq_hard_w25_note (note_key, note_text, note_value)
  VALUES ('typed-json-window', analytic_shape, score_total);
END;
/

-- W25-C: a vector expression is now a relational ordering key. DEFINE owns
-- NEAR/FAR as pattern variables; FIRST/LAST/COUNT in MEASURES resolve against
-- those variables instead of the identically named SQL aggregates.
DECLARE
  pattern_shape VARCHAR2(1000);
  pattern_rows  NUMBER;
BEGIN
  WITH ranked AS (
    SELECT tenant_id,
           document_id,
           document_name,
           VECTOR_DISTANCE(
             embedding,
             VECTOR('[1,0,0,0]', 4, FLOAT32),
             COSINE
           ) AS distance_value
    FROM sq_hard_w25_doc
  ),
  recognized AS (
    SELECT tenant_id,
           first_document,
           last_document,
           near_rows,
           match_rows
    FROM ranked
    MATCH_RECOGNIZE (
      PARTITION BY tenant_id
      ORDER BY distance_value, document_id
      MEASURES
        FIRST(near.document_id) AS first_document,
        LAST(far.document_id) AS last_document,
        COUNT(near.*) AS near_rows,
        COUNT(*) AS match_rows
      ONE ROW PER MATCH
      AFTER MATCH SKIP PAST LAST ROW
      PATTERN (near+ far*)
      DEFINE
        near AS near.distance_value < 0.1,
        far AS far.distance_value >= 0.1
    )
  )
  SELECT LISTAGG(
           tenant_id || ':' ||
           first_document || ':' ||
           last_document || ':' ||
           near_rows || ':' ||
           match_rows,
           '/'
         ) WITHIN GROUP (ORDER BY tenant_id),
         SUM(match_rows)
  INTO pattern_shape, pattern_rows
  FROM recognized;

  INSERT INTO sq_hard_w25_note (note_key, note_text, note_value)
  VALUES ('vector-row-pattern', pattern_shape, pattern_rows);
END;
/

COMMIT;

--------------------------------------------------------------------------------
-- Wave-25 self-verification.
--------------------------------------------------------------------------------
DECLARE
  search_shape   VARCHAR2(1000);
  search_value   NUMBER;
  analytic_shape VARCHAR2(1000);
  analytic_value NUMBER;
  pattern_shape  VARCHAR2(1000);
  pattern_value  NUMBER;
  note_rows      PLS_INTEGER;
BEGIN
  SELECT note_text, note_value
  INTO search_shape, search_value
  FROM sq_hard_w25_note
  WHERE note_key = 'approximate-vector-index';

  SELECT note_text, note_value
  INTO analytic_shape, analytic_value
  FROM sq_hard_w25_note
  WHERE note_key = 'typed-json-window';

  SELECT note_text, note_value
  INTO pattern_shape, pattern_value
  FROM sq_hard_w25_note
  WHERE note_key = 'vector-row-pattern';

  SELECT COUNT(*)
  INTO note_rows
  FROM sq_hard_w25_note;

  IF search_shape <>
       '1:101:alpha/1:102:beta/2:201:delta/2:202:epsilon'
     OR search_value <> 6061 THEN
    RAISE_APPLICATION_ERROR(
      -20987,
      'w25 vector index ' || search_shape || '/' || search_value
    );
  END IF;

  IF analytic_shape <>
       '101:10:-:30/102:5:5:15/201:7:-:21/202:8:8:16'
     OR analytic_value <> 82 THEN
    RAISE_APPLICATION_ERROR(
      -20988,
      'w25 typed json ' || analytic_shape || '/' || analytic_value
    );
  END IF;

  IF pattern_shape <> '1:101:103:2:3/2:201:203:1:3'
     OR pattern_value <> 6
     OR note_rows <> 3 THEN
    RAISE_APPLICATION_ERROR(
      -20989,
      'w25 pattern ' || pattern_shape || '/' || pattern_value ||
      '/' || note_rows
    );
  END IF;
END;
/

--------------------------------------------------------------------------------
-- ULTRA WAVE 26 -- window-exclusion, FULL-join, SQL/JSON, SQL/XML and
-- conversion-grammar ownership singularity.
--
-- This wave targets grammar that remains legal on the live 23.26.0 image:
--   * one inherited GROUPS frame is closed by each remaining EXCLUDE variant;
--   * UNPIVOT EXCLUDE NULLS turns those analytic aliases back into rows;
--   * quoted LEFT/RIGHT CTEs meet in a FULL OUTER JOIN before two APPLY clauses;
--   * JSON_TABLE and XMLTABLE mint parallel ordinality/name scopes, while
--     RETURNING SEQUENCE BY REF reaches from an XML tag to its parent/sibling;
--   * DEFAULT ... ON CONVERSION ERROR and VALIDATE_CONVERSION put datatype
--     grammar inside function arguments beside both forms of TRANSLATE USING.
-- Every branch is deterministic and persists an independently checked result.
--------------------------------------------------------------------------------
DROP TABLE IF EXISTS sq_hard_w26_note PURGE;
DROP TABLE IF EXISTS sq_hard_w26_event PURGE;

CREATE TABLE sq_hard_w26_note (
  note_key   VARCHAR2(40) CONSTRAINT sq_hard_w26_note_pk PRIMARY KEY,
  note_text  VARCHAR2(2000) NOT NULL,
  note_value NUMBER NOT NULL
) TABLESPACE users;

CREATE TABLE sq_hard_w26_event (
  stream_id      NUMBER NOT NULL,
  event_id       NUMBER NOT NULL,
  peer_key       NUMBER NOT NULL,
  amount         NUMBER NOT NULL,
  state_code     VARCHAR2(10) NOT NULL,
  event_label    VARCHAR2(20) NOT NULL,
  amount_text    VARCHAR2(20) NOT NULL,
  occurred_text  VARCHAR2(20) NOT NULL,
  payload        JSON NOT NULL,
  event_xml      XMLTYPE NOT NULL,
  CONSTRAINT sq_hard_w26_event_pk PRIMARY KEY (stream_id, event_id),
  CONSTRAINT sq_hard_w26_event_state_ck
    CHECK (state_code IN ('OPEN', 'CLOSED'))
) TABLESPACE users;

INSERT INTO sq_hard_w26_event (
  stream_id, event_id, peer_key, amount, state_code, event_label,
  amount_text, occurred_text, payload, event_xml
)
VALUES (
  1, 1, 1, 10, 'OPEN', 'alpha', '10', '2024-01-01',
  JSON('{"label":"alpha","tags":["alpha-tag"]}'),
  XMLTYPE(
    '<event code="A"><label>alpha</label><tag>alpha-tag</tag></event>'
  )
);

INSERT INTO sq_hard_w26_event (
  stream_id, event_id, peer_key, amount, state_code, event_label,
  amount_text, occurred_text, payload, event_xml
)
VALUES (
  1, 2, 1, 20, 'CLOSED', 'beta', 'not-a-number', 'not-a-date',
  JSON('{"label":"beta","tags":["beta-tag"]}'),
  XMLTYPE(
    '<event code="B"><label>beta</label><tag>beta-tag</tag></event>'
  )
);

INSERT INTO sq_hard_w26_event (
  stream_id, event_id, peer_key, amount, state_code, event_label,
  amount_text, occurred_text, payload, event_xml
)
VALUES (
  1, 3, 2, 30, 'OPEN', 'gamma', '30', '2024-03-03',
  JSON('{"label":"gamma","tags":["gamma-tag"]}'),
  XMLTYPE(
    '<event code="C"><label>gamma</label><tag>gamma-tag</tag></event>'
  )
);

INSERT INTO sq_hard_w26_event (
  stream_id, event_id, peer_key, amount, state_code, event_label,
  amount_text, occurred_text, payload, event_xml
)
VALUES (
  1, 4, 3, 40, 'CLOSED', 'delta', '40', '2024-04-04',
  JSON('{"label":"delta","tags":["delta-tag"]}'),
  XMLTYPE(
    '<event code="D"><label>delta</label><tag>delta-tag</tag></event>'
  )
);

INSERT INTO sq_hard_w26_event (
  stream_id, event_id, peer_key, amount, state_code, event_label,
  amount_text, occurred_text, payload, event_xml
)
VALUES (
  1, 5, 3, 50, 'OPEN', 'epsilon', '50', '2024-05-05',
  JSON('{"label":"epsilon","tags":["epsilon-tag"]}'),
  XMLTYPE(
    '<event code="E"><label>epsilon</label><tag>epsilon-tag</tag></event>'
  )
);

--------------------------------------------------------------------------------
-- W26-A: GROUP is simultaneously a frame-exclusion keyword and the literal
-- emitted by UNPIVOT. Each derived window inherits partition/order from
-- W_ORDERED and adds the same GROUPS frame, changing only its EXCLUDE tail.
-- EXCLUDE GROUP produces NULL for the first peer group; EXCLUDE NULLS then
-- removes only those two cells while retaining all other analytic projections.
--------------------------------------------------------------------------------
INSERT INTO sq_hard_w26_note (note_key, note_text, note_value)
WITH
  framed AS (
    SELECT stream_id,
           event_id,
           peer_key,
           amount,
           SUM(amount) OVER w_exclude_group AS excluding_group,
           SUM(amount) OVER w_exclude_ties  AS excluding_ties,
           SUM(amount) OVER w_no_others     AS excluding_nothing
    FROM sq_hard_w26_event
    WINDOW
      w_ordered AS (
        PARTITION BY stream_id
        ORDER BY peer_key
      ),
      w_exclude_group AS (
        w_ordered
        GROUPS BETWEEN UNBOUNDED PRECEDING AND CURRENT ROW
        EXCLUDE GROUP
      ),
      w_exclude_ties AS (
        w_ordered
        GROUPS BETWEEN UNBOUNDED PRECEDING AND CURRENT ROW
        EXCLUDE TIES
      ),
      w_no_others AS (
        w_ordered
        GROUPS BETWEEN UNBOUNDED PRECEDING AND CURRENT ROW
        EXCLUDE NO OTHERS
      )
  ),
  exclusions AS (
    SELECT stream_id,
           event_id,
           peer_key,
           amount,
           exclusion_name,
           window_total
    FROM framed
    UNPIVOT EXCLUDE NULLS (
      window_total
      FOR exclusion_name IN (
        excluding_group   AS 'GROUP',
        excluding_ties    AS 'TIES',
        excluding_nothing AS 'NO OTHERS'
      )
    )
  )
SELECT 'window-exclusions',
       LISTAGG(
         event_id || ':' || exclusion_name || '=' ||
         TO_CHAR(window_total, 'FM9990'),
         '/'
       ) WITHIN GROUP (ORDER BY event_id, exclusion_name),
       SUM(window_total)
FROM exclusions;

--------------------------------------------------------------------------------
-- W26-B: quoted CTE names LEFT/RIGHT remain identifiers until FULL OUTER JOIN
-- consumes them. The chosen JSON and XML documents then feed independent
-- lateral row sources. JSON_TABLE's nested ordinality is aligned with
-- XMLTABLE's ordinality; RETURNING SEQUENCE BY REF lets paths from the selected
-- <tag> reach its parent attribute and preceding <label> sibling.
--------------------------------------------------------------------------------
INSERT INTO sq_hard_w26_note (note_key, note_text, note_value)
WITH
  "LEFT" (
    stream_id, event_id, amount, json_text, xml_text
  ) AS (
    SELECT stream_id,
           event_id,
           amount,
           JSON_SERIALIZE(payload RETURNING VARCHAR2(1000)),
           XMLSERIALIZE(CONTENT event_xml AS VARCHAR2(1000))
    FROM sq_hard_w26_event
    WHERE state_code = 'OPEN'
  ),
  "RIGHT" (
    stream_id, event_id, amount, json_text, xml_text
  ) AS (
    SELECT stream_id,
           event_id,
           amount,
           JSON_SERIALIZE(payload RETURNING VARCHAR2(1000)),
           XMLSERIALIZE(CONTENT event_xml AS VARCHAR2(1000))
    FROM sq_hard_w26_event
    WHERE amount BETWEEN 20 AND 40
  ),
  joined_documents AS (
    SELECT COALESCE(left_owner.stream_id, right_owner.stream_id) AS stream_id,
           COALESCE(left_owner.event_id, right_owner.event_id)   AS event_id,
           COALESCE(left_owner.amount, right_owner.amount)       AS amount,
           COALESCE(left_owner.json_text, right_owner.json_text) AS json_text,
           COALESCE(left_owner.xml_text, right_owner.xml_text)   AS xml_text,
           CASE
             WHEN left_owner.event_id IS NULL THEN 'R'
             WHEN right_owner.event_id IS NULL THEN 'L'
             ELSE 'B'
           END AS join_side
    FROM "LEFT" left_owner
         FULL OUTER JOIN "RIGHT" right_owner
           ON right_owner.stream_id = left_owner.stream_id
          AND right_owner.event_id = left_owner.event_id
  ),
  expanded_documents AS (
    SELECT document_owner.stream_id,
           document_owner.event_id,
           document_owner.amount,
           document_owner.join_side,
           json_owner.json_label,
           json_owner.json_tag_no,
           json_owner.json_tag,
           xml_owner.xml_label,
           xml_owner.xml_tag_no,
           xml_owner.xml_tag,
           xml_owner.xml_code
    FROM joined_documents document_owner
         CROSS APPLY JSON_TABLE(
           JSON(document_owner.json_text),
           '$'
           COLUMNS (
             json_label VARCHAR2(20) PATH '$.label',
             NESTED PATH '$.tags[*]'
               COLUMNS (
                 json_tag_no FOR ORDINALITY,
                 json_tag VARCHAR2(30) PATH '$'
               )
           )
         ) json_owner
         CROSS APPLY XMLTABLE(
           '/event/tag'
           PASSING XMLTYPE(document_owner.xml_text)
           RETURNING SEQUENCE BY REF
           COLUMNS
             xml_tag_no FOR ORDINALITY,
             xml_label VARCHAR2(20) PATH '../label',
             xml_tag   VARCHAR2(30) PATH '.',
             xml_code  VARCHAR2(1)  PATH '../@code'
         ) xml_owner
  )
SELECT 'full-json-xml',
       LISTAGG(
         event_id || ':' || json_label || ':' || xml_code || ':' ||
         json_tag || ':' || join_side,
         '/'
       ) WITHIN GROUP (ORDER BY event_id, json_tag_no),
       SUM(amount)
FROM expanded_documents
WHERE json_tag_no = xml_tag_no
  AND json_label = xml_label
  AND json_tag = xml_tag;

--------------------------------------------------------------------------------
-- W26-C: DEFAULT and ON CONVERSION ERROR belong to TO_NUMBER/TO_DATE, while AS
-- NUMBER belongs to VALIDATE_CONVERSION. Nested TRANSLATE calls use USING as a
-- character-set selector rather than a join clause, and round-trip every label
-- through NCHAR_CS before LISTAGG closes the conversion scope.
--------------------------------------------------------------------------------
INSERT INTO sq_hard_w26_note (note_key, note_text, note_value)
WITH
  converted AS (
    SELECT event_id,
           TO_NUMBER(
             amount_text DEFAULT '0' ON CONVERSION ERROR,
             '999'
           ) AS amount_number,
           TO_DATE(
             occurred_text DEFAULT '1970-01-01' ON CONVERSION ERROR,
             'YYYY-MM-DD'
           ) AS occurred_date,
           VALIDATE_CONVERSION(amount_text AS NUMBER) AS amount_is_number,
           TRANSLATE(
             TRANSLATE(event_label USING NCHAR_CS)
             USING CHAR_CS
           ) AS roundtrip_label
    FROM sq_hard_w26_event
  )
SELECT 'conversion-ownership',
       LISTAGG(
         event_id || ':' ||
         TO_CHAR(amount_number, 'FM9990') || ':' ||
         TO_CHAR(occurred_date, 'YYYY') || ':' ||
         amount_is_number || ':' ||
         roundtrip_label,
         '/'
       ) WITHIN GROUP (ORDER BY event_id),
       SUM(amount_number)
FROM converted;

COMMIT;

--------------------------------------------------------------------------------
-- Wave-26 self-verification.
--------------------------------------------------------------------------------
DECLARE
  exclusion_shape VARCHAR2(2000);
  exclusion_total NUMBER;
  document_shape  VARCHAR2(2000);
  document_total  NUMBER;
  conversion_shape VARCHAR2(2000);
  conversion_total NUMBER;
  note_rows       PLS_INTEGER;
BEGIN
  SELECT note_text, note_value
  INTO exclusion_shape, exclusion_total
  FROM sq_hard_w26_note
  WHERE note_key = 'window-exclusions';

  SELECT note_text, note_value
  INTO document_shape, document_total
  FROM sq_hard_w26_note
  WHERE note_key = 'full-json-xml';

  SELECT note_text, note_value
  INTO conversion_shape, conversion_total
  FROM sq_hard_w26_note
  WHERE note_key = 'conversion-ownership';

  SELECT COUNT(*)
  INTO note_rows
  FROM sq_hard_w26_note;

  IF exclusion_shape <>
       '1:NO OTHERS=30/1:TIES=10/2:NO OTHERS=30/2:TIES=20/' ||
       '3:GROUP=30/3:NO OTHERS=60/3:TIES=60/' ||
       '4:GROUP=60/4:NO OTHERS=150/4:TIES=100/' ||
       '5:GROUP=60/5:NO OTHERS=150/5:TIES=110'
     OR exclusion_total <> 870 THEN
    RAISE_APPLICATION_ERROR(
      -20990,
      'w26 exclusions ' || exclusion_shape || '/' || exclusion_total
    );
  END IF;

  IF document_shape <>
       '1:alpha:A:alpha-tag:L/2:beta:B:beta-tag:R/' ||
       '3:gamma:C:gamma-tag:B/4:delta:D:delta-tag:R/' ||
       '5:epsilon:E:epsilon-tag:L'
     OR document_total <> 150 THEN
    RAISE_APPLICATION_ERROR(
      -20991,
      'w26 documents ' || document_shape || '/' || document_total
    );
  END IF;

  IF conversion_shape <>
       '1:10:2024:1:alpha/2:0:1970:0:beta/3:30:2024:1:gamma/' ||
       '4:40:2024:1:delta/5:50:2024:1:epsilon'
     OR conversion_total <> 130
     OR note_rows <> 3 THEN
    RAISE_APPLICATION_ERROR(
      -20992,
      'w26 conversions ' || conversion_shape || '/' ||
      conversion_total || '/' || note_rows
    );
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
       (SELECT COUNT(*) FROM sq_hard_w7_pos)  AS wave7_pos,
       (SELECT COUNT(*) FROM sq_hard_w8_note) AS wave8_notes,
       (SELECT COUNT(*) FROM sq_hard_w8_doc)  AS wave8_docs,
       (SELECT SUM(doubled)
        FROM TABLE(sq_hard_w8_pipe_pkg.widen(CURSOR (SELECT dim_id
                                                     FROM sq_hard_w8_dim)))) AS wave8_widened,
       (SELECT COUNT(*) FROM sq_hard_w9_note)   AS wave9_notes,
       (SELECT COUNT(*) FROM sq_hard_w9_bucket) AS wave9_buckets,
       (SELECT DEREF(e.child).shout()
        FROM sq_hard_w9_edge e
        WHERE e.edge_id = 20)                   AS wave9_leaf_shout,
       (SELECT COUNT(*) FROM sq_hard_w10_note)  AS wave10_notes,
       (SELECT COUNT(*) FROM sq_hard_w10_bulk)  AS wave10_bulk_rows,
       (SELECT d.profile.name.string()
        FROM sq_hard_w10_doc d
        WHERE d.doc_id = 1)                     AS wave10_dotted,
       (SELECT COUNT(*)
        FROM sq_hard_w10_lease
               AS OF PERIOD FOR user_valid_time DATE '2024-04-15')
                                                AS wave10_valid_at,
       (SELECT COUNT(*) FROM sq_hard_w11_note)  AS wave11_notes,
       (SELECT SUM(amount) FROM sq_hard_w11_ledger_v)
                                                AS wave11_ledger,
       (SELECT MAX(row_id) FROM sq_hard_w11_type)
                                                AS wave11_identity_hi,
       (SELECT LISTAGG(stamp_id, '/') WITHIN GROUP (ORDER BY stamp_id)
        FROM sq_hard_w11_stamp)                 AS wave11_stamp_ids,
       sq_hard_w11_util_pkg.describe('alpha,beta,gamma')
                                                AS wave11_described,
       (SELECT COUNT(*) FROM sq_hard_w12_note)  AS wave12_notes,
       (SELECT SUM(value) FROM sq_hard_w12_kw)  AS wave12_kw_total,
       (SELECT SUM(sum) FROM sq_hard_w12_fn)    AS wave12_fn_sum,
       (SELECT s."select" + s."*" + s."END;"
        FROM "sq_hard_w12_select" s)            AS wave12_quoted,
       sq_hard_w12_shadow(0)                    AS wave12_shadowed,
       sq_hard_w12_pkg.to_char(1)               AS wave12_named_call,
       (SELECT COUNT(*) FROM sq_hard_w13_note)  AS wave13_notes,
       (SELECT SUM(current_value)
        FROM sq_hard_w13_assign)                AS wave13_assign_total,
       (SELECT note_text
        FROM sq_hard_w13_note
        WHERE note_key = 'model-cells')         AS wave13_model_shape,
       (SELECT note_value
        FROM sq_hard_w13_note
        WHERE note_key = 'data-guide')          AS wave13_guide_bytes,
       (SELECT COUNT(*) FROM sq_hard_w14_note)   AS wave14_notes,
       (SELECT SUM(amount) FROM sq_hard_w14_money)
                                                AS wave14_money_total,
       (SELECT note_text
        FROM sq_hard_w14_note
        WHERE note_key = 'merge-images')         AS wave14_merge_images,
       (SELECT note_text
        FROM sq_hard_w14_note
        WHERE note_key = 'nested-with')          AS wave14_nested_shape
       ,
       (SELECT COUNT(*) FROM sq_hard_w15_note)   AS wave15_notes,
       (SELECT note_text
        FROM sq_hard_w15_note
        WHERE note_key = 'boolean-join')         AS wave15_boolean_shape,
       (SELECT note_text
        FROM sq_hard_w15_note
        WHERE note_key = 'partition-fetch')      AS wave15_partition_shape,
       (SELECT note_text
        FROM sq_hard_w15_note
        WHERE note_key = 'uuid-default')         AS wave15_uuid_shape
       ,
       (SELECT COUNT(*) FROM sq_hard_w16_note)    AS wave16_notes,
       (SELECT note_text
        FROM sq_hard_w16_note
        WHERE note_key = 'graph-step-pattern')    AS wave16_step_shape,
       (SELECT note_text
        FROM sq_hard_w16_note
        WHERE note_key = 'graph-vertex-pivot')    AS wave16_vertex_shape,
       (SELECT note_value
        FROM sq_hard_w16_note
        WHERE note_key = 'graph-qualify-json')    AS wave16_json_cost,
       (SELECT COUNT(*) FROM sq_hard_w17_note)    AS wave17_notes,
       (SELECT note_text
        FROM sq_hard_w17_note
        WHERE note_key = 'window-pattern')         AS wave17_window_shape,
       (SELECT note_text
        FROM sq_hard_w17_note
        WHERE note_key = 'multiset-apply')         AS wave17_multiset_shape,
       (SELECT COUNT(*) FROM sq_hard_w18_note)      AS wave18_notes,
       (SELECT note_text
        FROM sq_hard_w18_note
        WHERE note_key = 'scope-collapse')          AS wave18_scope_shape,
       (SELECT COUNT(*) FROM sq_hard_w19_note)      AS wave19_notes,
       (SELECT JSON_VALUE(note_text, '$."1".name')
        FROM sq_hard_w19_note
        WHERE note_key = 'duality-pattern')         AS wave19_duality_name,
       (SELECT COUNT(*) FROM sq_hard_w20_note)      AS wave20_notes,
       (SELECT note_text
        FROM sq_hard_w20_note
        WHERE note_key = 'pattern-subset')          AS wave20_pattern_shape,
       (SELECT COUNT(*) FROM sq_hard_w21_note)      AS wave21_notes,
       (SELECT note_text
        FROM sq_hard_w21_note
        WHERE note_key = 'interval-window')         AS wave21_interval_shape,
       (SELECT note_text
        FROM sq_hard_w21_note
        WHERE note_key = 'delete-from-old')         AS wave21_delete_shape,
       (SELECT COUNT(*) FROM sq_hard_w22_note)      AS wave22_notes,
       (SELECT note_text
        FROM sq_hard_w22_note
        WHERE note_key = 'bucket-json-window')       AS wave22_bucket_shape,
       (SELECT note_value
        FROM sq_hard_w22_note
        WHERE note_key = 'json-order-id')            AS wave22_id_bytes,
       (SELECT COUNT(*) FROM sq_hard_w23_note)       AS wave23_notes,
       (SELECT note_text
        FROM sq_hard_w23_note
        WHERE note_key = 'path-transform')           AS wave23_path_shape,
       (SELECT note_text
        FROM sq_hard_w23_note
        WHERE note_key = 'boolean-bulk')             AS wave23_boolean_shape
       ,
       (SELECT COUNT(*) FROM sq_hard_w24_note)        AS wave24_notes,
       (SELECT note_text
        FROM sq_hard_w24_note
        WHERE note_key = 'external-loader')           AS wave24_external_shape,
       (SELECT note_text
        FROM sq_hard_w24_note
        WHERE note_key = 'recursive-nested')          AS wave24_nested_shape,
       (SELECT note_text
        FROM sq_hard_w24_note
        WHERE note_key = 'sample-collection')         AS wave24_collection_shape
       ,
       (SELECT COUNT(*) FROM sq_hard_w25_note)         AS wave25_notes,
       (SELECT note_text
        FROM sq_hard_w25_note
        WHERE note_key = 'approximate-vector-index')   AS wave25_vector_shape,
       (SELECT note_text
        FROM sq_hard_w25_note
        WHERE note_key = 'typed-json-window')          AS wave25_window_shape,
       (SELECT note_text
        FROM sq_hard_w25_note
        WHERE note_key = 'vector-row-pattern')         AS wave25_pattern_shape
       ,
       (SELECT COUNT(*) FROM sq_hard_w26_note)          AS wave26_notes,
       (SELECT note_text
        FROM sq_hard_w26_note
        WHERE note_key = 'window-exclusions')            AS wave26_window_shape,
       (SELECT note_text
        FROM sq_hard_w26_note
        WHERE note_key = 'full-json-xml')                 AS wave26_document_shape,
       (SELECT note_text
        FROM sq_hard_w26_note
        WHERE note_key = 'conversion-ownership')          AS wave26_conversion_shape
FROM dual;

PROMPT [ORACLE HARDCORE] PASS

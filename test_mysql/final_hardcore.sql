-- MySQL 8.0 HARDCORE editor-engine stress suite.
-- Live target: MySQL Community Server 8.0.46.
-- Run from the repository root:
--   mariadb --protocol=TCP -h127.0.0.1 -P3307 -uroot -pspacequery \
--     --show-warnings --binary-mode < test_mysql/final_hardcore.sql
--
-- Purpose: DELIBERATELY hostile-but-legal grammar that pushes the completion,
-- auto-formatting, and syntax-highlighting engines far past the gentle coverage
-- of test_mysql/final.sql. Everything still parses and executes on the live
-- server so the formatted output can be re-executed for certification.

DROP DATABASE IF EXISTS sq_hard_mysql;
CREATE DATABASE sq_hard_mysql CHARACTER SET utf8mb4 COLLATE utf8mb4_0900_ai_ci;
USE sq_hard_mysql;
SET NAMES utf8mb4;
SET SESSION sql_mode = 'STRICT_ALL_TABLES';

-- Backtick identifiers that collide with reserved words, embed spaces, and use
-- $/# characters. Highlighting/completion must treat these as identifiers.
CREATE TABLE `select` (
  `from`               INT NOT NULL,
  `Group By`           DATE,
  `order`              INT,
  `x$weird#col`        INT,
  `Column ``With`` Backtick` VARCHAR(30),
  json_value           JSON,
  location             POINT SRID 4326 NOT NULL,
  computed_upper       VARCHAR(30) GENERATED ALWAYS AS (UPPER(`from`)) VIRTUAL,
  PRIMARY KEY (`from`),
  SPATIAL KEY sx_select_loc (location)
) ENGINE = InnoDB;

INSERT INTO `select` (`from`, `Group By`, `order`, `x$weird#col`,
                      `Column ``With`` Backtick`, json_value, location)
VALUES (1, DATE '2024-02-29', 2, 3, 'b`t',
        JSON_OBJECT('tags', JSON_ARRAY('sql', 'json'),
                    'items', JSON_ARRAY(JSON_OBJECT('n', 'a', 'v', 10),
                                        JSON_OBJECT('n', 'b', 'v', 20))),
        ST_GeomFromText('POINT(1 2)', 4326, 'axis-order=long-lat'));

SELECT s.`from` + s.`order`                            AS summed,
       s.`Column ``With`` Backtick`                    AS quoted_col,
       EXTRACT(YEAR FROM s.`Group By`)                 AS leap_year,
       ST_AsText(s.location)                           AS point_text
FROM `select` s
WHERE s.`from` = 1 AND s.`order` BETWEEN 1 AND 9;

-- Deeply nested derived tables (each aliased), scalar subqueries, and inline
-- comments mid-statement.
SELECT /* deep nesting */ deep.total,
       (SELECT COUNT(*) FROM `select`) AS row_ct
FROM (
  SELECT /* level 1 */
         (SELECT COUNT(*)
          FROM (SELECT 3 c FROM (SELECT 2 b FROM (SELECT 1 a) t3) t2) t1) AS total
) deep;

-- Operator adjacency, exotic literals, JSON path operators chained.
SELECT 1+2*3-4 DIV 2                       AS arithmetic_value,
       5%3, 6&3, 7|1, 8^2, ~9, 1<<4, 256>>2 AS bit_ops,
       X'53514C'                           AS hex_literal,
       0x4D79                              AS zerox_literal,
       b'1010'                             AS bit_literal,
       _utf8mb4'유니코드' COLLATE utf8mb4_0900_ai_ci AS charset_literal,
       json_value->'$.tags[0]'             AS json_arrow,
       json_value->>'$.items[1].n'         AS json_unquote,
       'sql' MEMBER OF (json_value->'$.tags') AS json_member
FROM `select`;

-- JSON_TABLE with NESTED PATH, ordinality, and ON EMPTY defaults.
SELECT jt.item_no, jt.item_name, jt.item_value
FROM `select` s,
     JSON_TABLE(s.json_value, '$'
       COLUMNS (
         NESTED PATH '$.items[*]' COLUMNS (
           item_no    FOR ORDINALITY,
           item_name  VARCHAR(16) PATH '$.n',
           item_value INT PATH '$.v' DEFAULT '0' ON EMPTY
         )
       )) jt
ORDER BY jt.item_no;

-- Recursive CTE whose recursive branch references its own name in FROM.
WITH RECURSIVE counter (n, fact) AS (
  SELECT 1, 1
  UNION ALL
  SELECT n + 1, fact * (n + 1) FROM counter WHERE n < 6
)
SELECT n, fact FROM counter ORDER BY n;

-- Window functions with a named window, frame, and nested CASE.
CREATE TABLE metric (
  metric_id   INT NOT NULL,
  node_id     INT NOT NULL,
  measured_on DATE NOT NULL,
  metric_value DECIMAL(12,2) NOT NULL,
  PRIMARY KEY (metric_id)
) ENGINE = InnoDB;
INSERT INTO metric VALUES
  (1, 1, '2026-01-01', 12), (2, 1, '2026-01-03', 18),
  (3, 1, '2026-01-08', 24), (4, 2, '2026-01-02', 2);

-- Invisible data, functional/invisible index, multi-valued JSON index, stored
-- generated columns, and a JSON Schema CHECK in the same table definition.
CREATE TABLE hard_document (
  doc_id       BIGINT NOT NULL,
  node_id      INT NOT NULL,
  secret_token VARBINARY(32) INVISIBLE,
  payload      JSON NOT NULL,
  kind_code    VARCHAR(16) GENERATED ALWAYS AS (
    JSON_UNQUOTE(JSON_EXTRACT(payload, '$.kind'))
  ) STORED,
  score_value  DECIMAL(12,2) GENERATED ALWAYS AS (
    CAST(JSON_UNQUOTE(JSON_EXTRACT(payload, '$.score')) AS DECIMAL(12,2))
  ) STORED,
  PRIMARY KEY (doc_id),
  KEY ix_hard_document_node_score (node_id, score_value DESC),
  KEY ix_hard_document_kind ((
    CAST(JSON_UNQUOTE(JSON_EXTRACT(payload, '$.kind')) AS CHAR(16))
  )) INVISIBLE,
  KEY ix_hard_document_tags ((
    CAST(payload->'$.tags' AS CHAR(24) ARRAY)
  )),
  CONSTRAINT chk_hard_document_schema CHECK (
    JSON_SCHEMA_VALID(
      '{"type":"object","required":["kind","score","tags","items"],"properties":{"kind":{"type":"string"},"score":{"type":"number"},"tags":{"type":"array","items":{"type":"string"}},"items":{"type":"array"}}}',
      payload
    )
  )
) ENGINE = InnoDB;

INSERT INTO hard_document (doc_id, node_id, secret_token, payload) VALUES
  (101, 1, 'alpha-secret',
   JSON_OBJECT(
     'kind', 'latency', 'score', 12,
     'tags', JSON_ARRAY('critical', 'sql'),
     'items', JSON_ARRAY(
       JSON_OBJECT('name', 'parse', 'value', 4),
       JSON_OBJECT('name', 'execute', 'value', 8)
     )
   )),
  (102, 1, 'beta-secret',
   JSON_OBJECT(
     'kind', 'latency', 'score', 18,
     'tags', JSON_ARRAY('stable'),
     'items', JSON_ARRAY(JSON_OBJECT('name', 'format', 'value', 18))
   )),
  (201, 2, 'gamma-secret',
   JSON_OBJECT(
     'kind', 'errors', 'score', 24,
     'tags', JSON_ARRAY('critical', 'json'),
     'items', JSON_ARRAY(
       JSON_OBJECT('name', 'load', 'value', 10),
       JSON_OBJECT('name', 'run', 'value', 14)
     )
   )),
  (301, 3, 'delta-secret',
   JSON_OBJECT(
     'kind', 'errors', 'score', 2,
     'tags', JSON_ARRAY('archive'),
     'items', JSON_ARRAY(JSON_OBJECT('name', 'audit', 'value', 2))
   ));

-- Row-alias upsert keeps deprecated VALUES() out of the expression grammar
-- while forcing alias/member completion immediately after a long JSON value.
INSERT INTO hard_document (doc_id, node_id, secret_token, payload)
VALUES (
  101, 1, 'alpha-secret-v2',
  JSON_OBJECT(
    'kind', 'latency', 'score', 12,
    'tags', JSON_ARRAY('critical', 'sql'),
    'items', JSON_ARRAY(
      JSON_OBJECT('name', 'parse', 'value', 4),
      JSON_OBJECT('name', 'execute', 'value', 8)
    ),
    'checked', TRUE
  )
) AS incoming
ON DUPLICATE KEY UPDATE
  secret_token = incoming.secret_token,
  payload = JSON_MERGE_PATCH(
    hard_document.payload,
    JSON_OBJECT('checked', JSON_EXTRACT(incoming.payload, '$.checked'))
  );

-- Single-statement collision suite: VALUES ROW, recursive CTE, nested
-- JSON_TABLE, inherited named windows, INTERSECT/EXCEPT, ROLLUP/GROUPING, and a
-- correlated LATERAL owner all feed the final JSON projection.
WITH RECURSIVE
tag_weights (tag_name, tag_weight) AS (
  SELECT *
  FROM (
    VALUES ROW('critical', 5), ROW('sql', 3),
           ROW('json', 2), ROW('stable', 1), ROW('archive', 1)
  ) AS weights(tag_name, tag_weight)
),
hard_nodes (node_id, parent_node_id, node_name) AS (
  SELECT *
  FROM (
    VALUES ROW(1, CAST(NULL AS SIGNED), 'root'),
           ROW(2, 1, 'blue'), ROW(3, 1, 'green'), ROW(4, 2, 'leaf')
  ) AS nodes(node_id, parent_node_id, node_name)
),
hard_tree (node_id, parent_node_id, node_path, tree_depth) AS (
  SELECT node_id, parent_node_id, CAST(UPPER(node_name) AS CHAR(400)), 0
  FROM hard_nodes
  WHERE parent_node_id IS NULL
  UNION ALL
  SELECT n.node_id, n.parent_node_id,
         CONCAT(t.node_path, '/', UPPER(n.node_name)), t.tree_depth + 1
  FROM hard_nodes n
  JOIN hard_tree t ON t.node_id = n.parent_node_id
),
expanded AS (
  SELECT d.doc_id, d.node_id, d.kind_code, d.score_value,
         jt.item_no, jt.item_name, jt.item_value,
         COALESCE((
           SELECT SUM(w.tag_weight)
           FROM tag_weights w
           WHERE w.tag_name MEMBER OF (d.payload->'$.tags')
         ), 0) AS tag_weight
  FROM hard_document d
  JOIN JSON_TABLE(
    d.payload,
    '$.items[*]' COLUMNS (
      item_no    FOR ORDINALITY,
      item_name  VARCHAR(24) PATH '$.name' ERROR ON ERROR,
      item_value DECIMAL(12,2) PATH '$.value'
                 DEFAULT '0' ON EMPTY DEFAULT '0' ON ERROR
    )
  ) jt ON TRUE
),
analytic AS (
  SELECT e.*,
         SUM(e.item_value) OVER w_running AS running_value,
         LAG(e.item_value, 1, 0) OVER w_ordered AS previous_value,
         NTH_VALUE(e.item_value, 2) OVER w_full AS second_value,
         ROW_NUMBER() OVER w_rank AS value_rank
  FROM expanded e
  WINDOW
    w_node AS (PARTITION BY e.node_id),
    w_ordered AS (
      w_node ORDER BY e.score_value DESC, e.doc_id, e.item_no
    ),
    w_running AS (
      w_ordered ROWS BETWEEN UNBOUNDED PRECEDING AND CURRENT ROW
    ),
    w_full AS (
      w_ordered ROWS BETWEEN UNBOUNDED PRECEDING AND UNBOUNDED FOLLOWING
    ),
    w_rank AS (
      w_node ORDER BY e.item_value DESC, e.doc_id, e.item_no
    )
),
eligible (doc_id) AS (
  (SELECT doc_id
   FROM hard_document
   WHERE 'critical' MEMBER OF (payload->'$.tags'))
  INTERSECT
  (SELECT doc_id FROM hard_document WHERE score_value >= 12)
  EXCEPT
  (SELECT doc_id FROM hard_document WHERE score_value < 0)
),
document_rollup AS (
  SELECT node_id, kind_code,
         GROUPING(node_id) AS node_grouped,
         GROUPING(kind_code) AS kind_grouped,
         COUNT(*) AS document_count,
         SUM(score_value) AS score_total
  FROM hard_document
  GROUP BY node_id, kind_code WITH ROLLUP
)
SELECT CASE
         WHEN (SELECT COUNT(*) FROM hard_tree) = 4
          AND (SELECT COUNT(*) FROM expanded) = 6
          AND (SELECT COUNT(*) FROM eligible) = 2
         THEN 'PASS' ELSE 'FAIL'
       END AS integrated_status,
       t.node_id, t.node_path, t.tree_depth,
       top_item.doc_id, top_item.kind_code, top_item.item_name,
       top_item.item_value, top_item.running_value,
       COALESCE(r.document_count, 0) AS document_count,
       JSON_OBJECT(
         'scoreTotal', COALESCE(r.score_total, 0),
         'eligibleDocuments', (
           SELECT COUNT(*)
           FROM eligible e
           JOIN hard_document d ON d.doc_id = e.doc_id
           WHERE d.node_id = t.node_id
         ),
         'allItems', (
           SELECT GROUP_CONCAT(
                    CONCAT(a.item_name, ':', a.item_value)
                    ORDER BY a.doc_id, a.item_no SEPARATOR ','
                  )
           FROM analytic a
           WHERE a.node_id = t.node_id
         )
       ) AS evidence
FROM hard_tree t
LEFT JOIN LATERAL (
  SELECT a.doc_id, a.kind_code, a.item_name,
         a.item_value, a.running_value
  FROM analytic a
  WHERE a.node_id = t.node_id
  ORDER BY a.value_rank, a.doc_id, a.item_no
  LIMIT 1
) top_item ON TRUE
LEFT JOIN document_rollup r
  ON r.node_id = t.node_id
 AND r.kind_code IS NULL
 AND r.node_grouped = 0
 AND r.kind_grouped = 1
ORDER BY t.node_id;

SELECT metric_id, node_id, metric_value,
       CASE WHEN metric_value > (CASE WHEN node_id = 1 THEN 10 ELSE 20 END)
            THEN 'high' ELSE 'low' END                       AS band,
       SUM(metric_value) OVER w_run                          AS running_sum,
       LAG(metric_value, 1, 0) OVER w_ord                    AS prev_value,
       NTH_VALUE(metric_value, 2) OVER w_frame               AS second_value
FROM metric
WINDOW w_ord   AS (PARTITION BY node_id ORDER BY measured_on, metric_id),
       w_run   AS (w_ord ROWS BETWEEN UNBOUNDED PRECEDING AND CURRENT ROW),
       w_frame AS (w_ord ROWS BETWEEN UNBOUNDED PRECEDING AND UNBOUNDED FOLLOWING)
ORDER BY node_id, metric_id;

-- LATERAL derived table and GROUP BY ... WITH ROLLUP + GROUPING.
SELECT n.node_id, top.metric_id, top.metric_value
FROM (SELECT DISTINCT node_id FROM metric) n
LEFT JOIN LATERAL (
  SELECT metric_id, metric_value FROM metric m
  WHERE m.node_id = n.node_id ORDER BY metric_value DESC LIMIT 1
) top ON TRUE
ORDER BY n.node_id;

SELECT node_id, SUM(metric_value) AS total, GROUPING(node_id) AS is_rollup
FROM metric GROUP BY node_id WITH ROLLUP;

-- Stored routine torture: labelled loops, cursor, handlers, SIGNAL/RESIGNAL,
-- GET DIAGNOSTICS, nested BEGIN blocks, CASE, and a scalar function.
DELIMITER $$
CREATE FUNCTION sq_hard_band(v DECIMAL(12,2)) RETURNS VARCHAR(8) DETERMINISTIC
RETURN CASE WHEN v >= 20 THEN 'HIGH' WHEN v >= 10 THEN 'MID' ELSE 'LOW' END$$

CREATE PROCEDURE sq_hard_walk(OUT total_out DECIMAL(12,2))
outer_block: BEGIN
  DECLARE done_flag BOOLEAN DEFAULT FALSE;
  DECLARE v DECIMAL(12,2);
  DECLARE acc DECIMAL(12,2) DEFAULT 0;
  DECLARE cur CURSOR FOR SELECT metric_value FROM metric ORDER BY metric_id;
  DECLARE CONTINUE HANDLER FOR NOT FOUND SET done_flag = TRUE;

  OPEN cur;
  scan: LOOP
    FETCH cur INTO v;
    IF done_flag THEN
      LEAVE scan;
    END IF;
    SET acc = acc + v;
  END LOOP scan;
  CLOSE cur;

  IF acc < 0 THEN
    BEGIN
      DECLARE EXIT HANDLER FOR SQLEXCEPTION
      BEGIN
        GET DIAGNOSTICS CONDITION 1 @sq_state = RETURNED_SQLSTATE;
        RESIGNAL;
      END;
      SIGNAL SQLSTATE '45000' SET MESSAGE_TEXT = 'unreachable';
    END;
  END IF;

  SET total_out = acc;
END outer_block$$
DELIMITER ;

CALL sq_hard_walk(@walk_total);

-- Multi-table UPDATE/DELETE with join, then self-verification and PASS.
START TRANSACTION;
UPDATE metric m JOIN (SELECT node_id, MAX(metric_value) mx FROM metric GROUP BY node_id) g
  ON g.node_id = m.node_id
SET m.metric_value = m.metric_value
WHERE m.metric_value = g.mx;
ROLLBACK;

DELIMITER $$
CREATE PROCEDURE sq_hard_assert(IN cond BOOLEAN, IN msg VARCHAR(255))
BEGIN
  IF COALESCE(cond, FALSE) = FALSE THEN
    SIGNAL SQLSTATE '45000' SET MESSAGE_TEXT = msg;
  END IF;
END$$
DELIMITER ;

CALL sq_hard_assert(@walk_total = 56, 'cursor walk total');
CALL sq_hard_assert((SELECT `from` + `order` FROM `select`) = 3, 'quoted identifier sum');
CALL sq_hard_assert((SELECT COUNT(*) FROM metric) = 4, 'metric rows');
CALL sq_hard_assert(sq_hard_band(24) = 'HIGH' COLLATE utf8mb4_0900_ai_ci, 'scalar function');
CALL sq_hard_assert((SELECT COUNT(*) FROM hard_document) = 4, 'document rows');
CALL sq_hard_assert(
  (SELECT COUNT(secret_token) FROM hard_document) = 4,
  'invisible values'
);
CALL sq_hard_assert(
  (SELECT COUNT(*) FROM hard_document
   WHERE 'critical' MEMBER OF (payload->'$.tags')) = 2,
  'multi-valued critical tags'
);
CALL sq_hard_assert(
  JSON_EXTRACT(
    (SELECT payload FROM hard_document WHERE doc_id = 101),
    '$.checked'
  ) = TRUE,
  'row-alias upsert'
);

START TRANSACTION;
WITH boosted AS (
  SELECT doc_id
  FROM hard_document
  WHERE score_value >= 18
)
UPDATE hard_document d
JOIN boosted b ON b.doc_id = d.doc_id
SET d.payload = JSON_SET(d.payload, '$.temporary', TRUE);
SELECT doc_id, node_id, score_value
FROM hard_document
WHERE score_value >= 18
ORDER BY score_value DESC, doc_id
FOR SHARE SKIP LOCKED;
ROLLBACK;

-- Bare TABLE and VALUES statements (MySQL 8.0), plus UNION between them.
TABLE metric ORDER BY metric_id LIMIT 2;
VALUES ROW(1, 'one'), ROW(2, 'two'), ROW(3, 'three');
(TABLE metric ORDER BY metric_id DESC LIMIT 1)
UNION ALL
(TABLE metric ORDER BY metric_id ASC LIMIT 1);

-- Version-gated executable comments and hash comments.
# hash comment before an optimizer-hint-laden query
SELECT /*+ NO_RANGE_OPTIMIZATION(m PRIMARY) */
       /*!80000 SQL_NO_CACHE */ m.metric_id, m.metric_value
FROM metric m
WHERE m.metric_id IN (1, 2)
ORDER BY m.metric_id;

-- Unquoted and backticked Unicode identifiers plus Unicode aliases.
CREATE TABLE 한글테이블 (
  식별자 INT NOT NULL PRIMARY KEY,
  `한글 컬럼` VARCHAR(40) NOT NULL,
  숨김열 VARCHAR(20) INVISIBLE DEFAULT '그림자'
) ENGINE = InnoDB;

INSERT INTO 한글테이블 (식별자, `한글 컬럼`) VALUES (1, '첫번째'), (2, '두번째');

SELECT t.식별자 AS 한글별칭,
       CONCAT(t.`한글 컬럼`, '/', t.숨김열) AS 조합값
FROM 한글테이블 t
ORDER BY t.식별자;

-- FULLTEXT search: natural language, boolean operators, query expansion.
CREATE TABLE article (
  article_id INT NOT NULL PRIMARY KEY,
  title VARCHAR(80) NOT NULL,
  body TEXT NOT NULL,
  FULLTEXT KEY fx_article (title, body)
) ENGINE = InnoDB;

INSERT INTO article VALUES
  (1, 'critical latency spike', 'the parser slowed under critical load'),
  (2, 'stable release notes', 'formatting and highlighting were stable'),
  (3, 'json vector search', 'vector search meets json documents');

SELECT article_id,
       MATCH (title, body) AGAINST ('critical load') AS nat_score
FROM article
WHERE MATCH (title, body) AGAINST ('critical load')
ORDER BY nat_score DESC, article_id;

SELECT article_id
FROM article
WHERE MATCH (title, body)
      AGAINST ('+critical -stable' IN BOOLEAN MODE)
ORDER BY article_id;

SELECT COUNT(*) AS expanded_hits
FROM article
WHERE MATCH (title, body)
      AGAINST ('latency' WITH QUERY EXPANSION);

-- Range/key subpartitioned table with explicit partition selection.
CREATE TABLE part_metric (
  metric_id INT NOT NULL,
  measured_year INT NOT NULL,
  metric_value DECIMAL(12,2) NOT NULL,
  PRIMARY KEY (metric_id, measured_year)
) ENGINE = InnoDB
PARTITION BY RANGE (measured_year)
SUBPARTITION BY KEY (metric_id)
SUBPARTITIONS 2 (
  PARTITION p_old VALUES LESS THAN (2026),
  PARTITION p_now VALUES LESS THAN (2027),
  PARTITION p_max VALUES LESS THAN MAXVALUE
);

INSERT INTO part_metric VALUES
  (1, 2025, 11), (2, 2026, 22), (3, 2026, 33), (4, 2030, 44);

SELECT metric_id, metric_value
FROM part_metric PARTITION (p_now)
ORDER BY metric_id;

SELECT COUNT(*) AS old_and_max_rows
FROM part_metric PARTITION (p_old, p_max);

-- HANDLER interface: OPEN/READ FIRST/READ NEXT/CLOSE.
HANDLER metric OPEN AS metric_handle;
HANDLER metric_handle READ FIRST;
HANDLER metric_handle READ NEXT;
HANDLER metric_handle CLOSE;

-- Prepared statements with placeholders and session variables.
PREPARE hard_stmt FROM
  'SELECT metric_id, metric_value FROM metric
   WHERE node_id = ? AND metric_value >= ?
   ORDER BY metric_id';
SET @want_node = 1, @want_floor = 15;
EXECUTE hard_stmt USING @want_node, @want_floor;
DEALLOCATE PREPARE hard_stmt;

-- Window frame with a temporal RANGE INTERVAL bound (the construct the Oracle
-- suite also exercises), plus CUME_DIST/FIRST_VALUE through a named window.
SELECT metric_id, node_id, metric_value,
       SUM(metric_value) OVER (
         PARTITION BY node_id ORDER BY measured_on
         RANGE BETWEEN INTERVAL 7 DAY PRECEDING AND CURRENT ROW
       )                                                  AS trailing_week,
       CUME_DIST() OVER w_val                             AS cume_share,
       FIRST_VALUE(metric_id) OVER w_val                  AS smallest_metric
FROM metric
WINDOW w_val AS (ORDER BY metric_value)
ORDER BY metric_id;

-- JSON_VALUE with RETURNING and ON EMPTY/ON ERROR defaults, JSON_OVERLAPS,
-- and inline JSON path chains over the document table.
SELECT doc_id,
       JSON_VALUE(payload, '$.score'
                  RETURNING DECIMAL(12,2)
                  DEFAULT '0' ON EMPTY DEFAULT '-1' ON ERROR) AS scored,
       JSON_OVERLAPS(payload->'$.tags', JSON_ARRAY('critical', 'missing'))
                                                              AS tag_overlap,
       JSON_STORAGE_SIZE(payload)                             > 0 AS has_bytes
FROM hard_document
ORDER BY doc_id;

-- EXPLAIN variants execute without touching data.
EXPLAIN FORMAT=TREE
SELECT node_id, SUM(metric_value) FROM metric GROUP BY node_id;

-- Disabled event: never fires, but exercises the event grammar.
CREATE EVENT sq_hard_event
  ON SCHEDULE AT CURRENT_TIMESTAMP + INTERVAL 10 YEAR
  ON COMPLETION PRESERVE
  DISABLE
  COMMENT '편집기 스트레스용'
  DO UPDATE metric SET metric_value = metric_value WHERE metric_id = -1;

-- Routine with REPEAT/UNTIL, WHILE, ITERATE, CASE statement, and a named
-- condition handler.
DELIMITER $$
CREATE PROCEDURE sq_hard_loops(OUT spin_total INT)
main_block: BEGIN
  DECLARE tick INT DEFAULT 0;
  DECLARE spin INT DEFAULT 0;
  DECLARE overflow_guard CONDITION FOR SQLSTATE '22003';
  DECLARE CONTINUE HANDLER FOR overflow_guard SET spin = -1;

  count_up: REPEAT
    SET tick = tick + 1;
    CASE
      WHEN MOD(tick, 2) = 0 THEN SET spin = spin + tick;
      ELSE BEGIN END;
    END CASE;
  UNTIL tick >= 6
  END REPEAT count_up;

  drain: WHILE tick > 0 DO
    SET tick = tick - 1;
    IF MOD(tick, 3) = 0 THEN
      ITERATE drain;
    END IF;
    SET spin = spin + 1;
  END WHILE drain;

  SET spin_total = spin;
END main_block$$
DELIMITER ;

CALL sq_hard_loops(@spin_total);

-- Locking reads: NOWAIT on an uncontended row inside a transaction.
START TRANSACTION;
SELECT metric_id FROM metric WHERE metric_id = 1 FOR UPDATE NOWAIT;
ROLLBACK;

-- Extension self-verification.
CALL sq_hard_assert(@spin_total = 16, 'repeat/while spin total');
CALL sq_hard_assert(
  (SELECT COUNT(*) FROM article
   WHERE MATCH (title, body) AGAINST ('+critical -stable' IN BOOLEAN MODE)) = 1,
  'boolean fulltext'
);
CALL sq_hard_assert(
  (SELECT COUNT(*) FROM part_metric PARTITION (p_now)) = 2,
  'partition selection'
);
CALL sq_hard_assert(
  (SELECT 식별자 FROM 한글테이블 WHERE `한글 컬럼` = '두번째') = 2,
  'unicode identifiers'
);
CALL sq_hard_assert(
  (SELECT JSON_VALUE(payload, '$.score' RETURNING DECIMAL(12,2))
   FROM hard_document WHERE doc_id = 201) = 24,
  'json_value returning'
);

SELECT 'PASS' AS final_status,
       VERSION() AS server_version,
       (SELECT COUNT(*) FROM metric) AS metric_rows,
       @walk_total AS walk_total;

-- MySQL 8.0 HARDCORE editor-engine stress suite.
-- Live target: MySQL Community Server 8.0.46.
-- Run from the repository root:
--   mysql --protocol=TCP -h127.0.0.1 -P3307 -uroot -pspacequery \
--     --default-character-set=utf8mb4 --show-warnings --binary-mode \
--     < test_mysql/final_hardcore.sql
--
-- Purpose: DELIBERATELY hostile-but-legal grammar that pushes the completion,
-- auto-formatting, and syntax-highlighting engines far past the gentle coverage
-- of test_mysql/final.sql. Everything still parses and executes on the live
-- server so the formatted output can be re-executed for certification.

DROP DATABASE IF EXISTS sq_hard_mysql;
DROP DATABASE IF EXISTS sq_hard_mysql_aux;
CREATE DATABASE sq_hard_mysql CHARACTER SET utf8mb4 COLLATE utf8mb4_0900_ai_ci;
USE sq_hard_mysql;
SET NAMES utf8mb4;
SET SESSION sql_mode = 'STRICT_ALL_TABLES';
SET SESSION autocommit = 1;
SET TRANSACTION ISOLATION LEVEL REPEATABLE READ;

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

SELECT t.`식별자` AS `한글별칭`,
       CONCAT(t.`한글 컬럼`, '/', t.`숨김열`) AS `조합값`
FROM 한글테이블 t
ORDER BY t.`식별자`;

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

-- ULTRA WAVE 3: cross-database objects force schema-qualified completion.
CREATE DATABASE sq_hard_mysql_aux CHARACTER SET utf8mb4;
CREATE TABLE sq_hard_mysql_aux.node_lookup (
  node_id  INT NOT NULL PRIMARY KEY,
  node_tag VARCHAR(20) NOT NULL
) ENGINE = InnoDB;
INSERT INTO sq_hard_mysql_aux.node_lookup VALUES (1, 'core'), (2, 'edge');

SELECT m.metric_id, l.node_tag
FROM metric m
JOIN sq_hard_mysql_aux.node_lookup l ON l.node_id = m.node_id
WHERE m.metric_id <= 2
ORDER BY m.metric_id;

-- Lexer bait: backtick aliases containing comment openers, charset introducers
-- with hex payloads, bit literals, null-safe / XOR / && operators, and a
-- running user-variable assignment inside the projection.
SELECT '/* not a comment */'                    AS `co--mment`,
       '-- still a string'                      AS `/*alias*/`,
       _utf8mb4 X'E29C93'                       AS check_glyph,
       _binary X'DEAD'                          AS bin_lit,
       b'1011'                                  AS bit_literal,
       NULL <=> NULL                            AS null_safe_eq,
       TRUE XOR FALSE                           AS xor_flag,
       (1 < 2) && (3 < 4)                       AS and_legacy
FROM DUAL;

SET @running_total := 0;
SELECT (@running_total := @running_total + s.metric_value) AS running_assign
FROM (SELECT metric_value FROM metric ORDER BY metric_id) s;

-- Non-recursive CTE that shadows the physical table it reads from: the inner
-- reference must resolve to the base table, the outer one to the CTE.
WITH metric AS (
  SELECT metric_id, metric_value * 100 AS boosted
  FROM metric
  WHERE node_id = 2
)
SELECT metric_id, boosted FROM metric ORDER BY metric_id;

-- Functional/expression DEFAULTs, an unenforced CHECK, and an INSTANT column
-- add on the same table.
CREATE TABLE gadget (
  gadget_id BIGINT NOT NULL AUTO_INCREMENT PRIMARY KEY,
  gid       BINARY(16) NOT NULL DEFAULT (UUID_TO_BIN(UUID())),
  made_on   DATE NOT NULL DEFAULT ((CURRENT_DATE + INTERVAL 1 YEAR)),
  qty       INT NOT NULL,
  CONSTRAINT chk_gadget_qty  CHECK (qty >= 0),
  CONSTRAINT chk_gadget_soft CHECK (qty < 1000) NOT ENFORCED
) ENGINE = InnoDB;

INSERT INTO gadget (qty) VALUES (1), (2), (3);
ALTER TABLE gadget ADD COLUMN note VARCHAR(20) NOT NULL DEFAULT '즉시', ALGORITHM = INSTANT;

-- REGEXP family with explicit match_type arguments.
SELECT article_id,
       REGEXP_LIKE(title, 'CRITICAL', 'i')             AS rx_like,
       REGEXP_INSTR(body, 'search')                    AS rx_instr,
       REGEXP_SUBSTR(title, '[[:alpha:]]+')            AS rx_first_word,
       REGEXP_REPLACE(title, '[aeiou]', '*', 1, 0, 'c') AS rx_masked
FROM article
ORDER BY article_id;

-- Geographic SRID 4326 math over the spatial column.
SELECT s.`from`,
       ST_SRID(s.location)                             AS srid_value,
       ST_Latitude(s.location)                         AS lat_value,
       ST_Longitude(s.location)                        AS lon_value,
       ROUND(ST_Distance_Sphere(
         s.location,
         ST_GeomFromText('POINT(3 4)', 4326, 'axis-order=long-lat')
       ))                                              AS sphere_dist_m
FROM `select` s;

-- View with CHECK OPTION plus DML routed through the view.
CREATE ALGORITHM = MERGE SQL SECURITY INVOKER VIEW big_gadget_v AS
SELECT gadget_id, qty, note
FROM gadget
WHERE qty >= 1
WITH CHECK OPTION;

INSERT INTO big_gadget_v (qty, note) VALUES (7, 'via view');
CALL sq_hard_assert(
  (SELECT COUNT(*) FROM gadget WHERE qty = 7) = 1,
  'insert through check-option view'
);
DELETE FROM big_gadget_v WHERE qty = 7;

-- Multi-table UPDATE and both multi-table DELETE spellings, REPLACE,
-- INSERT IGNORE, and a CTE-fed multi-table DELETE.
CREATE TABLE prune_rows (
  id INT NOT NULL PRIMARY KEY,
  v  INT NOT NULL
) ENGINE = InnoDB;
INSERT INTO prune_rows VALUES (1, 1), (2, 2), (3, 3), (4, 4);

UPDATE prune_rows p
JOIN metric m ON m.metric_id = p.id
SET p.v = p.v + m.node_id;

DELETE p
FROM prune_rows p
JOIN metric m ON m.metric_id = p.id
WHERE m.node_id = 2;

DELETE FROM p
USING prune_rows p
JOIN metric m ON m.metric_id = p.id
WHERE m.metric_value >= 24;

REPLACE INTO prune_rows (id, v) VALUES (1, 100);
INSERT IGNORE INTO prune_rows (id, v) VALUES (1, 999), (8, 80);

WITH doomed AS (
  SELECT id FROM prune_rows WHERE v >= 100
)
DELETE FROM p
USING prune_rows p
JOIN doomed d ON d.id = p.id;

-- JSON inspection helpers over the document table.
SELECT doc_id,
       JSON_KEYS(payload)                                   AS key_list,
       JSON_UNQUOTE(JSON_SEARCH(payload, 'one', 'critical')) AS critical_path,
       JSON_STORAGE_FREE(payload)                           AS storage_free,
       JSON_PRETTY(payload->'$.tags')                       AS pretty_tags
FROM hard_document
WHERE doc_id = 101;

-- Invisible index brought back through a SET_VAR optimizer-switch hint.
SELECT /*+ SET_VAR(optimizer_switch = 'use_invisible_indexes=on') */
       COUNT(*) AS kind_rows
FROM hard_document
WHERE CAST(JSON_UNQUOTE(JSON_EXTRACT(payload, '$.kind')) AS CHAR(16)) = 'latency';

-- Deep recursion beyond the default depth cap.
SET SESSION cte_max_recursion_depth = 2000;
WITH RECURSIVE deep (n) AS (
  SELECT 1
  UNION ALL
  SELECT n + 1 FROM deep WHERE n < 1500
)
SELECT COUNT(*) AS deep_rows, MAX(n) AS deep_max FROM deep;

-- RENAME TABLE three-way swap and TRUNCATE.
CREATE TABLE swap_a (id INT NOT NULL PRIMARY KEY, tag CHAR(1) NOT NULL);
CREATE TABLE swap_b (id INT NOT NULL PRIMARY KEY, tag CHAR(1) NOT NULL);
INSERT INTO swap_a VALUES (1, 'a'), (2, 'a');
INSERT INTO swap_b VALUES (9, 'b');

RENAME TABLE swap_a TO swap_tmp, swap_b TO swap_a, swap_tmp TO swap_b;
TRUNCATE TABLE swap_a;

-- Advisory locks and the DO statement.
SELECT GET_LOCK('sq_hard_lock', 0) AS got_lock;
DO RELEASE_LOCK('sq_hard_lock');

-- Prepared statement whose text is assembled in a user variable.
SET @dyn_sql = CONCAT('SELECT COUNT(*) AS dyn_count FROM ',
                      'metric WHERE node_id = ?');
PREPARE dyn_stmt FROM @dyn_sql;
SET @dyn_node = 1;
EXECUTE dyn_stmt USING @dyn_node;
DEALLOCATE PREPARE dyn_stmt;

-- Warning-class SIGNAL (does not abort) plus GET DIAGNOSTICS accounting.
DELIMITER $$
CREATE PROCEDURE sq_hard_warn(OUT warn_count INT)
BEGIN
  SIGNAL SQLSTATE '01000' SET MESSAGE_TEXT = '경고만 발생', MYSQL_ERRNO = 1642;
  GET DIAGNOSTICS warn_count = NUMBER;
END$$
DELIMITER ;
CALL sq_hard_warn(@warn_total);

-- EXPLAIN ANALYZE executes the plan for real.
EXPLAIN ANALYZE
SELECT node_id, SUM(metric_value)
FROM metric
GROUP BY node_id;

-- ULTRA WAVE 4: change the lexical grammar in-flight. ANSI_QUOTES turns
-- double quotes into identifier delimiters, PIPES_AS_CONCAT changes || from
-- logical OR into concatenation, and NO_BACKSLASH_ESCAPES makes the Windows
-- path below literal. Semicolons/comment openers remain inert inside tokens.
SET @sq_hard_w4_saved_mode = @@SESSION.sql_mode;
SET SESSION sql_mode = CONCAT(
  @sq_hard_w4_saved_mode,
  ',ANSI_QUOTES,PIPES_AS_CONCAT,NO_BACKSLASH_ESCAPES'
);

CREATE TABLE "mode--table" (
  "select"        INT NOT NULL PRIMARY KEY,
  "semi;column"   VARCHAR(80) NOT NULL,
  "quote""column" VARCHAR(80) NOT NULL
) ENGINE = InnoDB;

INSERT INTO "mode--table" (
  "select", "semi;column", "quote""column"
) VALUES
  (1, 'C:\tmp\semi;--literal', 'left/*middle*/right'),
  (2, 'DELIMITER |!| $$ //', 'quote"value');

SET @sq_hard_w4_pipe_concat = 'left' || '/' || 'right';
SET @'odd--variable' := 41;
SELECT "select", "semi;column", "quote""column",
       'left' || '/' || 'right' AS "pipe||alias",
       @'odd--variable' + "select" AS "quoted user variable"
FROM "mode--table"
ORDER BY "select";
SET SESSION sql_mode = @sq_hard_w4_saved_mode;

-- A multi-character delimiter that also occurs inside a string. The routine
-- combines a cursor, labelled LOOP, nested handler, stacked diagnostics, a
-- deliberate SIGNAL, and PREPARE/EXECUTE with quoted hostile identifiers.
DELIMITER |!|
CREATE PROCEDURE sq_hard_w4_walk(OUT total_out DECIMAL(12,2))
outer_w4: BEGIN
  DECLARE done_flag BOOLEAN DEFAULT FALSE;
  DECLARE current_value DECIMAL(12,2);
  DECLARE metric_total DECIMAL(12,2) DEFAULT 0;
  DECLARE cur_metric CURSOR FOR
    SELECT metric_value FROM metric ORDER BY metric_id;
  DECLARE CONTINUE HANDLER FOR NOT FOUND SET done_flag = TRUE;

  OPEN cur_metric;
  metric_loop: LOOP
    FETCH cur_metric INTO current_value;
    IF done_flag THEN
      LEAVE metric_loop;
    END IF;
    SET metric_total = metric_total + current_value;
  END LOOP metric_loop;
  CLOSE cur_metric;

  expected_signal: BEGIN
    DECLARE CONTINUE HANDLER FOR SQLSTATE '45000'
    BEGIN
      GET STACKED DIAGNOSTICS CONDITION 1
        @sq_hard_w4_stacked_state = RETURNED_SQLSTATE,
        @sq_hard_w4_stacked_errno = MYSQL_ERRNO,
        @sq_hard_w4_stacked_message = MESSAGE_TEXT;
    END;
    SIGNAL SQLSTATE '45000'
      SET MYSQL_ERRNO = 1644,
          MESSAGE_TEXT = 'expected stacked diagnostics';
  END expected_signal;

  SET @sq_hard_w4_dyn_id = 3;
  SET @sq_hard_w4_dyn_text = 'dynamic; -- /* */';
  SET @sq_hard_w4_dyn_sql =
    'INSERT INTO `mode--table` (`select`, `semi;column`, `quote"column`)
     VALUES (?, ?, ?)';
  PREPARE sq_hard_w4_insert FROM @sq_hard_w4_dyn_sql;
  EXECUTE sq_hard_w4_insert
    USING @sq_hard_w4_dyn_id, @sq_hard_w4_dyn_text, @sq_hard_w4_dyn_text;
  DEALLOCATE PREPARE sq_hard_w4_insert;

  SET @sq_hard_w4_delimiter_literal =
    '|!| inside literal; $$ and // stay inert';
  SET total_out = metric_total;
END outer_w4|!|
DELIMITER ;

CALL sq_hard_w4_walk(@sq_hard_w4_walk_total);

-- Geographic objects, spatial indexes, JSON documents, a stored generated
-- column, and LIST COLUMNS partitioning form a single metadata/completion web.
CREATE TABLE sq_hard_w4_place (
  place_id   INT NOT NULL PRIMARY KEY,
  place_code VARCHAR(20) NOT NULL,
  location   POINT SRID 4326 NOT NULL,
  attributes JSON NOT NULL,
  SPATIAL KEY sx_sq_hard_w4_place_location (location)
) ENGINE = InnoDB;

CREATE TABLE sq_hard_w4_zone (
  zone_id   INT NOT NULL PRIMARY KEY,
  zone_name VARCHAR(30) NOT NULL,
  boundary  POLYGON SRID 4326 NOT NULL,
  SPATIAL KEY sx_sq_hard_w4_zone_boundary (boundary)
) ENGINE = InnoDB;

CREATE TABLE sq_hard_w4_route (
  route_id   INT NOT NULL PRIMARY KEY,
  from_id    INT NOT NULL,
  to_id      INT NOT NULL,
  route_path LINESTRING SRID 4326 NOT NULL,
  route_meta JSON NOT NULL,
  SPATIAL KEY sx_sq_hard_w4_route_path (route_path),
  KEY ix_sq_hard_w4_route_endpoints (from_id, to_id)
) ENGINE = InnoDB;

CREATE TABLE sq_hard_w4_region_metric (
  metric_id    INT NOT NULL,
  place_id     INT NOT NULL,
  region_code  CHAR(2) NOT NULL,
  measured_on  DATE NOT NULL,
  metric_value INT NOT NULL,
  signals      JSON NOT NULL,
  signal_count INT GENERATED ALWAYS AS (JSON_LENGTH(signals)) STORED,
  PRIMARY KEY (metric_id, region_code),
  KEY ix_sq_hard_w4_region_value (metric_value)
) ENGINE = InnoDB
PARTITION BY LIST COLUMNS (region_code) (
  PARTITION p_asia VALUES IN ('KR', 'JP'),
  PARTITION p_west VALUES IN ('US', 'DE')
);

INSERT INTO sq_hard_w4_place VALUES
  (1, 'seoul',
   ST_GeomFromText('POINT(127.0276 37.4979)', 4326, 'axis-order=long-lat'),
   JSON_OBJECT('tags', JSON_ARRAY('sql', 'spatial'))),
  (2, 'busan',
   ST_GeomFromText('POINT(129.0756 35.1796)', 4326, 'axis-order=long-lat'),
   JSON_OBJECT('tags', JSON_ARRAY('harbor'))),
  (3, 'berlin',
   ST_GeomFromText('POINT(13.4050 52.5200)', 4326, 'axis-order=long-lat'),
   JSON_OBJECT('tags', JSON_ARRAY('archive')));

INSERT INTO sq_hard_w4_zone VALUES
  (1, 'ASIA_LAB',
   ST_GeomFromText(
     'POLYGON((124 33,142 33,142 39,124 39,124 33))',
     4326, 'axis-order=long-lat'
   )),
  (2, 'EUROPE_LAB',
   ST_GeomFromText(
     'POLYGON((10 50,16 50,16 55,10 55,10 50))',
     4326, 'axis-order=long-lat'
   ));

INSERT INTO sq_hard_w4_route VALUES
  (1, 1, 2,
   ST_GeomFromText(
     'LINESTRING(127.0276 37.4979,128 36.5,129.0756 35.1796)',
     4326, 'axis-order=long-lat'
   ),
   JSON_OBJECT('mode', 'rail', 'waypoints', 3)),
  (2, 1, 3,
   ST_GeomFromText(
     'LINESTRING(127.0276 37.4979,80 45,13.4050 52.5200)',
     4326, 'axis-order=long-lat'
   ),
   JSON_OBJECT('mode', 'air', 'waypoints', 3));

INSERT INTO sq_hard_w4_region_metric VALUES
  (1, 1, 'KR', '2026-01-01', 120,
   JSON_ARRAY(JSON_OBJECT('name', 'cpu', 'value', 31),
              JSON_OBJECT('name', 'io', 'value', 12)), DEFAULT),
  (2, 1, 'KR', '2026-01-02', 240,
   JSON_ARRAY(JSON_OBJECT('name', 'cpu', 'value', 45)), DEFAULT),
  (3, 2, 'KR', '2026-01-03', 360,
   JSON_ARRAY(JSON_OBJECT('name', 'net', 'value', 17)), DEFAULT),
  (4, 3, 'DE', '2026-01-04', 480,
   JSON_ARRAY(JSON_OBJECT('name', 'archive', 'value', 81)), DEFAULT);

-- Histogram DDL and index visibility are intentionally adjacent to stress
-- statement classification around ANALYZE TABLE and ALTER ... ALTER INDEX.
ANALYZE TABLE sq_hard_w4_region_metric
  UPDATE HISTOGRAM ON metric_value WITH 8 BUCKETS;
ALTER TABLE sq_hard_w4_region_metric
  ALTER INDEX ix_sq_hard_w4_region_value INVISIBLE;
ALTER TABLE sq_hard_w4_region_metric
  ALTER INDEX ix_sq_hard_w4_region_value VISIBLE;

-- CTE + spatial predicates + JSON_TABLE + named/in-line windows + a correlated
-- JSON aggregate. This is the densest single SELECT in the MySQL fixture.
WITH expanded AS (
  SELECT m.metric_id, m.place_id, m.region_code, m.measured_on,
         m.metric_value, p.place_code, z.zone_name,
         s.signal_no, s.signal_name, s.signal_value
  FROM sq_hard_w4_region_metric m
  JOIN sq_hard_w4_place p ON p.place_id = m.place_id
  JOIN sq_hard_w4_zone z
    ON MBRContains(z.boundary, p.location)
   AND ST_Contains(z.boundary, p.location)
  JOIN JSON_TABLE(
    m.signals,
    '$[*]' COLUMNS (
      signal_no    FOR ORDINALITY,
      signal_name  VARCHAR(30) PATH '$.name',
      signal_value INT PATH '$.value' DEFAULT '0' ON EMPTY
    )
  ) s ON TRUE
),
analytic AS (
  SELECT e.*,
         SUM(e.metric_value) OVER w_run AS running_value,
         JSON_ARRAYAGG(e.signal_name) OVER (
           PARTITION BY e.place_id
         ) AS signal_window
  FROM expanded e
  WINDOW w_run AS (
    PARTITION BY e.place_id
    ORDER BY e.measured_on, e.metric_id, e.signal_no
    ROWS BETWEEN UNBOUNDED PRECEDING AND CURRENT ROW
  )
)
SELECT a.*,
       (SELECT JSON_ARRAYAGG(
                 JSON_OBJECT(
                   'route', r.route_id,
                   'points', ST_NumPoints(r.route_path)
                 )
               )
        FROM sq_hard_w4_route r
        WHERE r.from_id = a.place_id OR r.to_id = a.place_id) AS routes
FROM analytic a
ORDER BY a.metric_id, a.signal_no;

SET @sq_hard_w4_schema =
  '{"type":"object","required":["kind","score"],"properties":{"kind":{"type":"string"},"score":{"type":"number"}}}';
SET @sq_hard_w4_invalid_doc =
  JSON_OBJECT('kind', 'probe', 'score', 'not-a-number');
SELECT JSON_PRETTY(
         JSON_SCHEMA_VALIDATION_REPORT(
           @sq_hard_w4_schema,
           @sq_hard_w4_invalid_doc
         )
       ) AS schema_report;

-- Distributed transaction grammar plus savepoint rollback. Both perform real
-- changes, and the assertions below distinguish committed from reverted data.
XA START 'sq_hard_mysql_w4_xa';
INSERT INTO `mode--table`
  (`select`, `semi;column`, `quote"column`)
VALUES (4, 'xa', 'committed');
XA END 'sq_hard_mysql_w4_xa';
XA COMMIT 'sq_hard_mysql_w4_xa' ONE PHASE;

START TRANSACTION;
SAVEPOINT before_w4_change;
UPDATE `mode--table`
SET `semi;column` = 'temporary'
WHERE `select` = 1;
ROLLBACK TO SAVEPOINT before_w4_change;
RELEASE SAVEPOINT before_w4_change;
COMMIT;

-- Wave-3 self-verification.
CALL sq_hard_assert(@running_total = 56, 'user variable running total');
CALL sq_hard_assert(
  (SELECT boosted FROM (
     WITH metric AS (
       SELECT metric_id, metric_value * 100 AS boosted
       FROM metric
       WHERE node_id = 2
     )
     SELECT metric_id, boosted FROM metric
   ) shadowed WHERE metric_id = 4) = 200,
  'cte shadowing base table'
);
CALL sq_hard_assert(
  (SELECT COUNT(*) FROM gadget
   WHERE made_on > CURRENT_DATE AND note = '즉시') = 3,
  'functional defaults + instant column'
);
CALL sq_hard_assert(
  (SELECT COUNT(*) FROM prune_rows) = 2,
  'multi-table delete/replace/ignore net rows'
);
CALL sq_hard_assert(
  (SELECT COUNT(*) FROM article WHERE REGEXP_LIKE(title, 'CRITICAL', 'i')) = 1,
  'regexp match_type'
);
CALL sq_hard_assert(
  (SELECT ST_Latitude(location) FROM `select`) = 2,
  'geographic latitude'
);
CALL sq_hard_assert(
  (SELECT COUNT(*) FROM swap_a) = 0 AND
  (SELECT COUNT(*) FROM swap_b) = 2,
  'rename swap + truncate'
);
CALL sq_hard_assert(@warn_total = 1, 'warning-class signal diagnostics');

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
  (SELECT `식별자` FROM 한글테이블 WHERE `한글 컬럼` = '두번째') = 2,
  'unicode identifiers'
);
CALL sq_hard_assert(
  (SELECT JSON_VALUE(payload, '$.score' RETURNING DECIMAL(12,2))
   FROM hard_document WHERE doc_id = 201) = 24,
  'json_value returning'
);

-- Wave-4 self-verification.
CALL sq_hard_assert(
  @sq_hard_w4_walk_total = 56,
  'wave4 cursor walk total'
);
CALL sq_hard_assert(
  @sq_hard_w4_stacked_state = '45000'
  AND @sq_hard_w4_stacked_errno = 1644
  AND @sq_hard_w4_stacked_message = 'expected stacked diagnostics',
  'stacked diagnostics'
);
CALL sq_hard_assert(
  @sq_hard_w4_delimiter_literal = '|!| inside literal; $$ and // stay inert'
  AND @sq_hard_w4_pipe_concat = 'left/right'
  AND @'odd--variable' = 41,
  'delimiter + live sql mode'
);
CALL sq_hard_assert(
  (SELECT COUNT(*) FROM `mode--table`) = 4
  AND (SELECT `semi;column` FROM `mode--table` WHERE `select` = 3)
      = 'dynamic; -- /* */'
  AND (SELECT `quote"column` FROM `mode--table` WHERE `select` = 4)
      = 'committed'
  AND (SELECT `semi;column` FROM `mode--table` WHERE `select` = 1)
      <> 'temporary',
  'dynamic sql + xa + savepoint'
);
CALL sq_hard_assert(
  (SELECT COUNT(*) FROM sq_hard_w4_place) = 3
  AND (SELECT COUNT(*) FROM sq_hard_w4_zone) = 2
  AND (SELECT COUNT(*) FROM sq_hard_w4_route) = 2
  AND (SELECT SUM(ST_NumPoints(route_path)) FROM sq_hard_w4_route) = 6,
  'spatial object graph'
);
CALL sq_hard_assert(
  (SELECT COUNT(*)
   FROM sq_hard_w4_place p
   JOIN sq_hard_w4_zone z
     ON ST_Contains(z.boundary, p.location)) = 3,
  'spatial containment'
);
CALL sq_hard_assert(
  (SELECT COUNT(*) FROM sq_hard_w4_region_metric PARTITION (p_asia)) = 3
  AND (SELECT COUNT(*) FROM sq_hard_w4_region_metric PARTITION (p_west)) = 1,
  'list columns partitions'
);
CALL sq_hard_assert(
  (SELECT COUNT(*)
   FROM INFORMATION_SCHEMA.COLUMN_STATISTICS
   WHERE SCHEMA_NAME = DATABASE()
     AND TABLE_NAME = 'sq_hard_w4_region_metric'
     AND COLUMN_NAME = 'metric_value') = 1,
  'histogram metadata'
);
CALL sq_hard_assert(
  (SELECT IS_VISIBLE
   FROM INFORMATION_SCHEMA.STATISTICS
   WHERE TABLE_SCHEMA = DATABASE()
     AND TABLE_NAME = 'sq_hard_w4_region_metric'
     AND INDEX_NAME = 'ix_sq_hard_w4_region_value'
   LIMIT 1) = 'YES',
  'index visibility'
);
CALL sq_hard_assert(
  JSON_UNQUOTE(JSON_EXTRACT(
    JSON_SCHEMA_VALIDATION_REPORT(
      @sq_hard_w4_schema,
      @sq_hard_w4_invalid_doc
    ),
    '$.valid'
  )) = 'false',
  'json schema validation report'
);

-- ULTRA WAVE 5: optimizer-steering syntax that only exists between FROM and ON,
-- ROLLUP with GROUPING(), LATERAL derived tables, a multi-valued JSON index
-- driving MEMBER OF, the 8.0.19 row-alias upsert, CTE-driven UPDATE/DELETE,
-- locking reads with an explicit OF list, and routine attribute stacking.

-- W5-A: join and index steering. STRAIGHT_JOIN appears twice with two different
-- grammars in one statement (SELECT modifier and join operator), the index hints
-- carry FOR JOIN / FOR ORDER BY scopes, and NATURAL LEFT JOIN plus JOIN ...
-- USING keep their join columns unqualifiable.
CREATE TABLE sq_hard_w5_hint (
  hint_id INT NOT NULL,
  node_id INT NOT NULL,
  bucket  VARCHAR(16) NOT NULL,
  weight  DECIMAL(10,2) NOT NULL,
  PRIMARY KEY (hint_id),
  KEY ix_sq_hard_w5_hint_node (node_id),
  KEY ix_sq_hard_w5_hint_bucket (bucket, weight)
) ENGINE = InnoDB;

INSERT INTO sq_hard_w5_hint VALUES
  (1, 1, 'alpha', 10.50), (2, 1, 'beta', 20.25),
  (3, 2, 'alpha', 5.00),  (4, 2, 'gamma', 7.75);

SELECT STRAIGHT_JOIN SQL_SMALL_RESULT SQL_BUFFER_RESULT
       h.node_id                AS node_id,
       COUNT(*)                 AS pair_rows,
       ROUND(SUM(h.weight), 2)  AS weight_sum
FROM sq_hard_w5_hint AS h USE INDEX FOR JOIN (ix_sq_hard_w5_hint_node)
     STRAIGHT_JOIN metric AS m FORCE INDEX (PRIMARY)
       ON m.node_id = h.node_id
GROUP BY h.node_id
ORDER BY h.node_id;

SELECT node_id, ROUND(SUM(weight), 2) AS natural_weight
FROM sq_hard_w5_hint
NATURAL LEFT JOIN (SELECT node_id, MAX(metric_value) AS peak
                   FROM metric
                   GROUP BY node_id) AS pk
GROUP BY node_id
ORDER BY node_id;

SELECT node_id, COUNT(*) AS using_rows
FROM metric
JOIN sq_hard_w5_hint USING (node_id)
GROUP BY node_id
ORDER BY node_id;

-- W5-B: quantified subqueries, a row-constructor IN list, WITH ROLLUP with a
-- GROUPING() super-aggregate marker, and an ORDER BY that mixes FIELD() with an
-- explicit COLLATE over the rollup result.
SELECT bucket,
       GROUPING(bucket)      AS is_total,
       ROUND(SUM(weight), 2) AS bucket_total
FROM sq_hard_w5_hint AS h IGNORE INDEX FOR ORDER BY (ix_sq_hard_w5_hint_bucket)
WHERE weight >= ANY (SELECT weight FROM sq_hard_w5_hint WHERE node_id = 2)
  AND (node_id, bucket) IN ((1, 'alpha'), (1, 'beta'), (2, 'gamma'))
  AND weight <> ALL (SELECT 999)
  AND EXISTS (SELECT 1 FROM metric AS m WHERE m.node_id = h.node_id)
GROUP BY bucket WITH ROLLUP
HAVING bucket_total > 0
ORDER BY is_total,
         FIELD(bucket, 'gamma', 'beta', 'alpha'),
         bucket COLLATE utf8mb4_bin;

-- W5-C: LATERAL derived tables. The first correlates a per-node top row, the
-- second is joined ON TRUE so the correlation is the only thing linking it.
SELECT n.node_id,
       peak.top_value,
       spread.value_spread
FROM (SELECT DISTINCT node_id FROM metric) AS n
JOIN LATERAL (SELECT x.metric_value AS top_value
              FROM metric AS x
              WHERE x.node_id = n.node_id
              ORDER BY x.metric_value DESC, x.metric_id
              LIMIT 1) AS peak ON TRUE
LEFT JOIN LATERAL (SELECT MAX(y.metric_value) - MIN(y.metric_value) AS value_spread
                   FROM metric AS y
                   WHERE y.node_id = n.node_id) AS spread ON TRUE
ORDER BY n.node_id;

-- W5-D: a multi-valued index over a JSON array, the three predicates that can
-- use it, and the 8.0.19 row-alias upsert whose alias shadows nothing.
CREATE TABLE sq_hard_w5_doc (
  doc_id INT NOT NULL,
  doc    JSON NOT NULL,
  PRIMARY KEY (doc_id),
  KEY mv_sq_hard_w5_tags ((CAST(doc->'$.tags' AS UNSIGNED ARRAY)))
) ENGINE = InnoDB;

INSERT INTO sq_hard_w5_doc VALUES
  (1, JSON_OBJECT('tags', JSON_ARRAY(1, 2, 3), 'name', 'alpha')),
  (2, JSON_OBJECT('tags', JSON_ARRAY(3, 4, 5), 'name', 'beta'));

SELECT d.doc_id
FROM sq_hard_w5_doc AS d
WHERE 3 MEMBER OF (d.doc->'$.tags')
ORDER BY d.doc_id;

SELECT d.doc_id
FROM sq_hard_w5_doc AS d
WHERE JSON_OVERLAPS(d.doc->'$.tags', CAST('[5,9]' AS JSON))
ORDER BY d.doc_id;

SELECT d.doc_id
FROM sq_hard_w5_doc AS d
WHERE JSON_CONTAINS(d.doc->'$.tags', CAST('[1,2]' AS JSON))
ORDER BY d.doc_id;

INSERT INTO sq_hard_w5_doc (doc_id, doc)
VALUES (1, JSON_OBJECT('tags', JSON_ARRAY(7), 'name', 'upserted')) AS incoming
ON DUPLICATE KEY UPDATE
  doc = JSON_SET(sq_hard_w5_doc.doc, '$.name', incoming.doc->>'$.name');

SELECT d.doc_id, d.doc->>'$.name' AS doc_name
FROM sq_hard_w5_doc AS d
ORDER BY d.doc_id;

-- W5-E: common table expressions feeding a multi-table UPDATE and a DELETE.
-- The CTE name is only visible to the statement it is attached to.
WITH renamed AS (
  SELECT doc_id FROM sq_hard_w5_doc WHERE doc_id = 2
)
UPDATE sq_hard_w5_doc
JOIN renamed ON renamed.doc_id = sq_hard_w5_doc.doc_id
SET sq_hard_w5_doc.doc = JSON_SET(sq_hard_w5_doc.doc, '$.name', 'cte-renamed');

WITH doomed AS (
  SELECT hint_id FROM sq_hard_w5_hint WHERE bucket = 'gamma'
)
DELETE h FROM sq_hard_w5_hint AS h
JOIN doomed ON doomed.hint_id = h.hint_id;

INSERT INTO sq_hard_w5_hint VALUES (4, 2, 'gamma', 7.75);

-- W5-F: locking reads that name their tables. OF narrows the lock to one table
-- of the join, and the two wait modifiers spell out opposite policies.
START TRANSACTION;

SELECT h.hint_id
FROM sq_hard_w5_hint AS h
WHERE h.node_id = 1
ORDER BY h.hint_id
FOR UPDATE OF h SKIP LOCKED;

SELECT h.hint_id
FROM sq_hard_w5_hint AS h
JOIN metric AS m ON m.node_id = h.node_id
WHERE h.node_id = 2
ORDER BY h.hint_id
FOR SHARE OF h NOWAIT;

COMMIT;

-- W5-G: a MEMORY temporary table, a multi-target SELECT ... INTO, a table value
-- constructor with an explicit column alias list, and two charset conversion
-- spellings that both name a character set.
CREATE TEMPORARY TABLE sq_hard_w5_tmp (
  tmp_id   INT NOT NULL,
  tmp_note VARCHAR(20) NOT NULL,
  PRIMARY KEY (tmp_id)
) ENGINE = Memory;

INSERT INTO sq_hard_w5_tmp
SELECT hint_id, CONCAT('tmp-', bucket) FROM sq_hard_w5_hint;

SELECT COUNT(*), MAX(tmp_id)
INTO @sq_hard_w5_tmp_rows, @sq_hard_w5_tmp_max
FROM sq_hard_w5_tmp;

SELECT v.row_key, v.row_label
FROM (VALUES ROW(1, 'first'), ROW(2, 'second')) AS v (row_key, row_label)
ORDER BY v.row_key;

SELECT CONVERT('hostile' USING utf8mb4)                    AS converted,
       CAST('hostile' AS CHAR CHARACTER SET utf8mb4)       AS cast_charset,
       CHARSET(CONVERT('hostile' USING latin1))            AS converted_charset;

-- W5-H: routine attribute stacking plus IN / INOUT / OUT parameter modes; the
-- COMMENT payload carries delimiter bait that must stay inside the literal.
DELIMITER //
CREATE FUNCTION sq_hard_w5_triple(p_value INT)
  RETURNS INT
  DETERMINISTIC
  CONTAINS SQL
  SQL SECURITY INVOKER
  COMMENT 'wave5 // $$ ; attribute stack'
BEGIN
  RETURN p_value * 3;
END//

CREATE PROCEDURE sq_hard_w5_accumulate(IN    p_seed  INT,
                                       INOUT p_acc   INT,
                                       OUT   p_label VARCHAR(32))
  MODIFIES SQL DATA
  SQL SECURITY DEFINER
  COMMENT 'wave5 out params'
BEGIN
  SET p_acc = p_acc + sq_hard_w5_triple(p_seed);
  INSERT INTO sq_hard_w5_tmp (tmp_id, tmp_note)
  VALUES (100 + p_seed, CONCAT('acc-', p_acc)) AS incoming
  ON DUPLICATE KEY UPDATE tmp_note = incoming.tmp_note;
  SET p_label = CONCAT('acc=', p_acc);
END//
DELIMITER ;

SET @sq_hard_w5_acc = 1;
CALL sq_hard_w5_accumulate(4, @sq_hard_w5_acc, @sq_hard_w5_label);

SELECT @sq_hard_w5_acc AS accumulated, @sq_hard_w5_label AS accumulated_label;

-- W5-I: wave-5 self-verification.
CALL sq_hard_assert(
  (SELECT COUNT(*)
   FROM sq_hard_w5_hint AS h
        STRAIGHT_JOIN metric AS m ON m.node_id = h.node_id) = 8
  AND (SELECT COUNT(*) FROM metric JOIN sq_hard_w5_hint USING (node_id)) = 8,
  'join steering + using join'
);
CALL sq_hard_assert(
  (SELECT COUNT(*)
   FROM (SELECT bucket, GROUPING(bucket) AS is_total, SUM(weight) AS bucket_total
         FROM sq_hard_w5_hint
         WHERE (node_id, bucket) IN ((1, 'alpha'), (1, 'beta'), (2, 'gamma'))
         GROUP BY bucket WITH ROLLUP) AS r
   WHERE r.is_total = 1 AND r.bucket_total = 38.50) = 1,
  'rollup grouping super-aggregate'
);
CALL sq_hard_assert(
  (SELECT MAX(peak.top_value)
   FROM (SELECT DISTINCT node_id FROM metric) AS n
   JOIN LATERAL (SELECT x.metric_value AS top_value
                 FROM metric AS x
                 WHERE x.node_id = n.node_id
                 ORDER BY x.metric_value DESC, x.metric_id
                 LIMIT 1) AS peak ON TRUE) = 24.00,
  'lateral derived table'
);
CALL sq_hard_assert(
  (SELECT COUNT(*) FROM sq_hard_w5_doc WHERE 3 MEMBER OF (doc->'$.tags')) = 2
  AND (SELECT doc->>'$.name' FROM sq_hard_w5_doc WHERE doc_id = 1) = 'upserted'
  AND (SELECT doc->>'$.name' FROM sq_hard_w5_doc WHERE doc_id = 2) = 'cte-renamed',
  'multi-valued index + row alias + cte update'
);
CALL sq_hard_assert(
  (SELECT COUNT(*) FROM sq_hard_w5_hint) = 4
  AND (SELECT bucket FROM sq_hard_w5_hint WHERE hint_id = 4) = 'gamma',
  'cte-fed delete then restore'
);
CALL sq_hard_assert(
  @sq_hard_w5_tmp_rows = 4
  AND @sq_hard_w5_tmp_max = 4
  AND @sq_hard_w5_acc = 13
  AND @sq_hard_w5_label = 'acc=13'
  AND (SELECT tmp_note FROM sq_hard_w5_tmp WHERE tmp_id = 104) = 'acc-13',
  'temporary table + out params'
);
CALL sq_hard_assert(
  (SELECT COUNT(*)
   FROM (VALUES ROW(1, 'first'), ROW(2, 'second')) AS v (row_key, row_label)) = 2
  AND CHARSET(CONVERT('hostile' USING latin1)) = 'latin1',
  'table value constructor + charset conversion'
);

-- ULTRA WAVE 6: named WINDOW definitions extended by frame clauses (including
-- an INTERVAL RANGE frame MariaDB cannot parse), a depth-capped recursive CTE,
-- partition maintenance DDL, roles, the DML modifier zoo, a labelled nested
-- block whose handler re-raises through RESIGNAL, histogram statistics and
-- invisible index/column DDL, and a pure lexer round.

-- W6-A: one WINDOW clause defines two named windows; the frame-extended forms
-- `(w RANGE ...)` and `(w ROWS ...)` inherit the partition and ordering, and the
-- RANGE bound is a temporal INTERVAL over the ORDER BY date.
CREATE TABLE sq_hard_w6_series (
  series_id INT           NOT NULL,
  bucket    VARCHAR(8)    NOT NULL,
  taken_on  DATE          NOT NULL,
  amount    DECIMAL(10,2) NOT NULL,
  PRIMARY KEY (series_id)
) ENGINE = InnoDB;

INSERT INTO sq_hard_w6_series (series_id, bucket, taken_on, amount)
VALUES (1, 'alpha', DATE '2026-01-01', 10.00),
       (2, 'alpha', DATE '2026-01-03', 20.00),
       (3, 'alpha', DATE '2026-01-08', 30.00),
       (4, 'beta',  DATE '2026-01-02', 40.00);

SELECT s.series_id,
       SUM(s.amount)        OVER w_bucket  AS bucket_total,
       ROW_NUMBER()         OVER w_ordered AS ordered_no,
       SUM(s.amount)        OVER (w_ordered RANGE BETWEEN INTERVAL 4 DAY PRECEDING
                                                      AND CURRENT ROW) AS window_4d,
       LAST_VALUE(s.amount) OVER (w_ordered ROWS BETWEEN UNBOUNDED PRECEDING
                                                     AND UNBOUNDED FOLLOWING) AS last_amount
FROM sq_hard_w6_series AS s
WINDOW w_bucket  AS (PARTITION BY s.bucket),
       w_ordered AS (PARTITION BY s.bucket ORDER BY s.taken_on)
ORDER BY s.series_id;

-- W6-B: a cyclic edge set. MySQL has no CYCLE clause, so the walk is capped by
-- a depth predicate and the session recursion limit set right beside it.
CREATE TABLE sq_hard_w6_edge (
  src INT NOT NULL,
  dst INT NOT NULL,
  PRIMARY KEY (src, dst)
) ENGINE = InnoDB;

INSERT INTO sq_hard_w6_edge (src, dst) VALUES (1, 2), (2, 3), (3, 1), (3, 4);

SET SESSION cte_max_recursion_depth = 32;

WITH RECURSIVE walk (node, depth, path) AS (
  SELECT 1, 0, CAST('1' AS CHAR(64))
  UNION ALL
  SELECT e.dst, w.depth + 1, CONCAT(w.path, '>', e.dst)
  FROM walk AS w
       JOIN sq_hard_w6_edge AS e ON e.src = w.node
  WHERE w.depth < 4
    AND LOCATE(CONCAT('>', e.dst), w.path) = 0
    AND e.dst <> 1
)
SELECT COUNT(*), MAX(depth) INTO @sq_hard_w6_walk_rows, @sq_hard_w6_walk_depth
FROM walk;

SELECT @sq_hard_w6_walk_rows AS walk_rows, @sq_hard_w6_walk_depth AS max_depth;

-- W6-C: RANGE-partitioned table reshaped in place - REORGANIZE splits a range,
-- TRUNCATE PARTITION empties one, and ANALYZE PARTITION returns a result set.
CREATE TABLE sq_hard_w6_part (
  part_id INT        NOT NULL,
  region  VARCHAR(8) NOT NULL,
  PRIMARY KEY (part_id)
) ENGINE = InnoDB
PARTITION BY RANGE (part_id) (
  PARTITION p0   VALUES LESS THAN (10),
  PARTITION p1   VALUES LESS THAN (20),
  PARTITION pmax VALUES LESS THAN MAXVALUE
);

INSERT INTO sq_hard_w6_part (part_id, region)
VALUES (1, 'a'), (11, 'b'), (21, 'c');

ALTER TABLE sq_hard_w6_part
  REORGANIZE PARTITION p0, p1 INTO (
    PARTITION p0 VALUES LESS THAN (5),
    PARTITION p1 VALUES LESS THAN (20)
  );

ALTER TABLE sq_hard_w6_part TRUNCATE PARTITION p1;

ALTER TABLE sq_hard_w6_part ANALYZE PARTITION p0;

SELECT COUNT(*) AS partition_rows
FROM sq_hard_w6_part PARTITION (p0, pmax);

-- W6-D: role grammar. The role is a global object, so it is dropped defensively
-- before it is created and again once the grants have been exercised.
DROP ROLE IF EXISTS sq_hard_w6_role;

CREATE ROLE sq_hard_w6_role;

GRANT SELECT ON sq_hard_mysql.sq_hard_w6_series TO sq_hard_w6_role;

GRANT sq_hard_w6_role TO CURRENT_USER();

SET ROLE sq_hard_w6_role;

SELECT CURRENT_ROLE() AS active_role;

SET ROLE NONE;

REVOKE SELECT ON sq_hard_mysql.sq_hard_w6_series FROM sq_hard_w6_role;

DROP ROLE sq_hard_w6_role;

-- W6-E: the DML modifier zoo. Every statement keyword here can also appear as
-- an ordinary identifier, so the modifiers must be read positionally.
CREATE TABLE sq_hard_w6_dml (
  dml_id INT         NOT NULL,
  tag    VARCHAR(20) NOT NULL,
  PRIMARY KEY (dml_id),
  UNIQUE KEY sq_hard_w6_dml_uq (tag)
) ENGINE = InnoDB;

INSERT IGNORE INTO sq_hard_w6_dml (dml_id, tag)
VALUES (1, 'one'), (1, 'duplicate-key'), (2, 'two');

REPLACE INTO sq_hard_w6_dml (dml_id, tag) VALUES (2, 'two-replaced');

INSERT INTO sq_hard_w6_dml (dml_id, tag)
VALUES (1, 'three') AS incoming
ON DUPLICATE KEY UPDATE tag = CONCAT(incoming.tag, '-merged');

UPDATE IGNORE sq_hard_w6_dml SET tag = 'two-replaced' WHERE dml_id = 1;

DELETE LOW_PRIORITY QUICK IGNORE FROM sq_hard_w6_dml WHERE dml_id = 99;

SELECT dml_id, tag FROM sq_hard_w6_dml ORDER BY dml_id;

-- W6-F: named condition, a CONTINUE handler for it, and a labelled nested block
-- whose own handler re-raises through RESIGNAL with a rewritten message.
DELIMITER //
CREATE PROCEDURE sq_hard_w6_guard(IN  p_value INT,
                                  OUT p_state VARCHAR(40))
  MODIFIES SQL DATA
BEGIN
  DECLARE sq_hard_w6_negative CONDITION FOR SQLSTATE '45001';
  DECLARE handled_errno INT DEFAULT 0;
  DECLARE CONTINUE HANDLER FOR sq_hard_w6_negative
  BEGIN
    GET DIAGNOSTICS CONDITION 1 handled_errno = MYSQL_ERRNO;
    SET p_state = CONCAT('handled-', handled_errno);
  END;
  DECLARE CONTINUE HANDLER FOR SQLSTATE '45003'
  BEGIN
    SET p_state = CONCAT(p_state, '/resignalled');
  END;

  SET p_state = 'clean';

  IF p_value < 0 THEN
    SIGNAL sq_hard_w6_negative SET MESSAGE_TEXT = 'negative input',
                                   MYSQL_ERRNO = 1451;
  END IF;

  wrapper: BEGIN
    DECLARE EXIT HANDLER FOR SQLSTATE '45002'
      RESIGNAL SET MYSQL_ERRNO = 1452, MESSAGE_TEXT = 'rewrapped';

    IF p_value = 0 THEN
      SIGNAL SQLSTATE '45002' SET MESSAGE_TEXT = 'zero input';
    END IF;

    LEAVE wrapper;
  END wrapper;
END//
DELIMITER ;

CALL sq_hard_w6_guard(-1, @sq_hard_w6_state_neg);
CALL sq_hard_w6_guard(1, @sq_hard_w6_state_ok);

SELECT @sq_hard_w6_state_neg AS state_negative,
       @sq_hard_w6_state_ok  AS state_ok;

-- W6-G: CREATE TABLE ... AS SELECT, then MySQL-only statistics and visibility
-- DDL: a histogram built over a column, an index made invisible and visible
-- again, and a column hidden from SELECT *.
CREATE TABLE sq_hard_w6_ctas
  ENGINE = InnoDB
  AS
SELECT s.bucket, SUM(s.amount) AS bucket_total, COUNT(*) AS bucket_rows
FROM sq_hard_w6_series AS s
GROUP BY s.bucket;

ALTER TABLE sq_hard_w6_ctas CONVERT TO CHARACTER SET utf8mb4 COLLATE utf8mb4_bin;

ALTER TABLE sq_hard_w6_ctas ADD INDEX sq_hard_w6_ctas_ix (bucket) USING BTREE;

ALTER TABLE sq_hard_w6_ctas ALTER INDEX sq_hard_w6_ctas_ix INVISIBLE;

ALTER TABLE sq_hard_w6_ctas ALTER INDEX sq_hard_w6_ctas_ix VISIBLE;

ALTER TABLE sq_hard_w6_ctas ADD COLUMN audit_note VARCHAR(20) NULL INVISIBLE AFTER bucket;

ANALYZE TABLE sq_hard_w6_ctas UPDATE HISTOGRAM ON bucket_total WITH 4 BUCKETS;

ANALYZE TABLE sq_hard_w6_ctas DROP HISTOGRAM ON bucket_total;

SELECT bucket, bucket_total, bucket_rows FROM sq_hard_w6_ctas ORDER BY bucket;

-- W6-H: pure lexer and layout torture. CASE nests five deep, a comment sits
-- between nearly every token, one projection is a single unspaced line, and the
-- keywords arrive in alternating case.
SELECT /*a*/ s.series_id /*b*/ AS /*c*/ id_out /*d*/,
       /*e*/ COALESCE(
         CASE WHEN s.amount > 25 THEN
           CASE WHEN s.bucket = 'alpha' THEN
             CASE WHEN s.series_id > 2 THEN
               CASE WHEN DAYOFMONTH(s.taken_on) = 8 THEN
                 CASE WHEN s.amount = 30 THEN 'deep-hit' ELSE 'deep-miss' END
               ELSE 'day-miss' END
             ELSE 'id-miss' END
           ELSE 'bucket-miss' END
         ELSE NULL END,
         'fallback') AS nested_case,
       'a `b` /* still literal */ -- still literal' AS quoted_payload
FROM /*f*/ sq_hard_w6_series /*g*/ s /*h*/
WHERE /*i*/ s.series_id /*j*/ IN /*k*/ (1, 2, 3, 4)
ORDER BY /*l*/ s.series_id /*m*/ DESC /*n*/, s.bucket /*o*/ ASC;

SELECT CONCAT(s.bucket,'-',CAST(s.amount AS CHAR))AS packed_line,(s.amount*2)-(s.series_id*3)+MOD(s.series_id,2)AS packed_math FROM sq_hard_w6_series s WHERE s.series_id IN(1,2)AND s.amount BETWEEN 1 AND 999 ORDER BY s.series_id;

sElEcT	CoUnT(*)	As	mixed_case_rows	FrOm	sq_hard_w6_series	WhErE	bucket	iS	NoT	nUlL;

-- W6-I: wave-6 self-verification.
CALL sq_hard_assert(
  (SELECT SUM(window_4d)
   FROM (SELECT SUM(s.amount) OVER (PARTITION BY s.bucket ORDER BY s.taken_on
                                    RANGE BETWEEN INTERVAL 4 DAY PRECEDING
                                              AND CURRENT ROW) AS window_4d
         FROM sq_hard_w6_series AS s) AS framed) = 110.00,
  'named window frames'
);
CALL sq_hard_assert(
  @sq_hard_w6_walk_rows = 4 AND @sq_hard_w6_walk_depth = 3,
  'recursive depth-capped walk'
);
CALL sq_hard_assert(
  (SELECT COUNT(*) FROM sq_hard_w6_part PARTITION (p0, pmax)) = 2
  AND (SELECT COUNT(*) FROM sq_hard_w6_part) = 2,
  'partition maintenance'
);
CALL sq_hard_assert(
  (SELECT COUNT(*) FROM sq_hard_w6_dml) = 2
  AND (SELECT tag FROM sq_hard_w6_dml WHERE dml_id = 1) = 'three-merged',
  'dml modifier zoo'
);
CALL sq_hard_assert(
  @sq_hard_w6_state_neg = 'handled-1451' AND @sq_hard_w6_state_ok = 'clean',
  'condition handler chain'
);
CALL sq_hard_assert(
  (SELECT ROUND(SUM(bucket_total), 2) FROM sq_hard_w6_ctas) = 100.00
  AND (SELECT COUNT(*) FROM sq_hard_w6_ctas) = 2
  AND (SELECT COUNT(*)
       FROM information_schema.STATISTICS
       WHERE TABLE_SCHEMA = DATABASE()
         AND TABLE_NAME = 'sq_hard_w6_ctas'
         AND INDEX_NAME = 'sq_hard_w6_ctas_ix'
         AND IS_VISIBLE = 'YES') = 1,
  'ctas + visibility ddl'
);

-- ULTRA WAVE 7: transaction control (consistent-snapshot start, savepoint
-- release, LOCK TABLES ... READ LOCAL), roles round 2 with SET DEFAULT ROLE and
-- SHOW GRANTS ... USING, two MySQL-only option-word DDL statements (resource
-- group, spatial reference system), a three-argument LAG/LEAD MariaDB rejects,
-- the JSON mutation family, RENAME INDEX / ALTER COLUMN DEFAULT maintenance
-- with the SHOW/CHECKSUM/FLUSH/ANALYZE family, and a lexer round.

-- W7-A: transaction control. START TRANSACTION takes modifier phrases that
-- read as clause keywords, savepoint names sit where an identifier is only
-- legal in this one statement, and LOCK TABLES takes a per-table lock mode.
CREATE TABLE sq_hard_w7_txn (
  txn_id INT PRIMARY KEY,
  state  VARCHAR(12) NOT NULL
) ENGINE = InnoDB;

-- The isolation level is set once up in the prelude, not here: a stored-procedure
-- CALL earlier in the script leaves the session conservatively marked as possibly
-- holding a named lock and carrying unknown state (a procedure body can call
-- GET_LOCK, which outlives COMMIT), and a transaction-mode change is refused
-- while that is outstanding. REPEATABLE READ is what makes the snapshot below
-- meaningful.
START TRANSACTION WITH CONSISTENT SNAPSHOT;
INSERT INTO sq_hard_w7_txn (txn_id, state) VALUES (1, 'kept');
SAVEPOINT after_first;
INSERT INTO sq_hard_w7_txn (txn_id, state) VALUES (2, 'rolled-back');
ROLLBACK TO SAVEPOINT after_first;
INSERT INTO sq_hard_w7_txn (txn_id, state) VALUES (3, 'kept-too');
RELEASE SAVEPOINT after_first;
COMMIT;

LOCK TABLES sq_hard_w7_txn READ LOCAL;
SELECT COUNT(*) AS locked_rows FROM sq_hard_w7_txn;
UNLOCK TABLES;

SELECT txn_id, state FROM sq_hard_w7_txn ORDER BY txn_id;

-- W7-B: roles round 2. SET DEFAULT ROLE and SHOW GRANTS ... USING put role
-- names in slots that otherwise only accept users, and a role name is a
-- two-part user@host identifier.
DROP ROLE IF EXISTS 'sq_hard_w7_reader', 'sq_hard_w7_writer';
DROP USER IF EXISTS 'sq_hard_w7_app'@'localhost';

CREATE ROLE 'sq_hard_w7_reader', 'sq_hard_w7_writer';
CREATE USER 'sq_hard_w7_app'@'localhost' IDENTIFIED BY 'Wave7Pass!';

GRANT SELECT ON sq_hard_mysql.* TO 'sq_hard_w7_reader';
GRANT INSERT, UPDATE ON sq_hard_mysql.sq_hard_w7_txn TO 'sq_hard_w7_writer';
GRANT 'sq_hard_w7_reader', 'sq_hard_w7_writer' TO 'sq_hard_w7_app'@'localhost';
SET DEFAULT ROLE 'sq_hard_w7_reader' TO 'sq_hard_w7_app'@'localhost';

SHOW GRANTS FOR 'sq_hard_w7_app'@'localhost' USING 'sq_hard_w7_reader';

SELECT COUNT(*) AS granted_roles
FROM INFORMATION_SCHEMA.APPLICABLE_ROLES
WHERE GRANTEE = 'sq_hard_w7_app';

REVOKE 'sq_hard_w7_writer' FROM 'sq_hard_w7_app'@'localhost';
DROP USER 'sq_hard_w7_app'@'localhost';
DROP ROLE 'sq_hard_w7_reader', 'sq_hard_w7_writer';

-- W7-C: two MySQL-only DDL statements built from `=`-joined option words --
-- a resource group and a spatial reference system whose DEFINITION is one
-- 700-character bracket-nested WKT literal on a single line. Both are reset
-- through a swallowing handler first, because DROP RESOURCE GROUP has no
-- IF EXISTS form and the prepared-statement protocol refuses it outright.
DROP PROCEDURE IF EXISTS sq_hard_w7_reset;
DELIMITER $$
CREATE PROCEDURE sq_hard_w7_reset()
BEGIN
  DECLARE CONTINUE HANDLER FOR SQLEXCEPTION BEGIN END;
  DECLARE CONTINUE HANDLER FOR SQLWARNING BEGIN END;
  DROP RESOURCE GROUP sq_hard_w7_rg;
  DROP SPATIAL REFERENCE SYSTEM IF EXISTS 998877;
END$$
DELIMITER ;
CALL sq_hard_w7_reset();

CREATE RESOURCE GROUP sq_hard_w7_rg
  TYPE = USER
  VCPU = 0
  THREAD_PRIORITY = 0
  ENABLE;

ALTER RESOURCE GROUP sq_hard_w7_rg VCPU = 0 THREAD_PRIORITY = 0 DISABLE;

SELECT RESOURCE_GROUP_TYPE, THREAD_PRIORITY, RESOURCE_GROUP_ENABLED
FROM INFORMATION_SCHEMA.RESOURCE_GROUPS
WHERE RESOURCE_GROUP_NAME = 'sq_hard_w7_rg';

DROP RESOURCE GROUP sq_hard_w7_rg;

CREATE SPATIAL REFERENCE SYSTEM 998877
  NAME 'sq_hard_w7 mercator'
  DESCRIPTION 'wave 7 fixture projection'
  DEFINITION 'PROJCS["sq_hard_w7 pseudo-mercator",GEOGCS["WGS 84",DATUM["World Geodetic System 1984",SPHEROID["WGS 84",6378137,298.257223563,AUTHORITY["EPSG","7030"]],AUTHORITY["EPSG","6326"]],PRIMEM["Greenwich",0,AUTHORITY["EPSG","8901"]],UNIT["degree",0.017453292519943278,AUTHORITY["EPSG","9122"]],AXIS["Lat",NORTH],AXIS["Lon",EAST],AUTHORITY["EPSG","4326"]],PROJECTION["Popular Visualisation Pseudo Mercator",AUTHORITY["EPSG","1024"]],PARAMETER["Latitude of natural origin",0,AUTHORITY["EPSG","8801"]],PARAMETER["Longitude of natural origin",0,AUTHORITY["EPSG","8802"]],PARAMETER["False easting",0,AUTHORITY["EPSG","8806"]],PARAMETER["False northing",0,AUTHORITY["EPSG","8807"]],UNIT["metre",1,AUTHORITY["EPSG","9001"]],AXIS["X",EAST],AXIS["Y",NORTH]]';

SELECT SRS_NAME, ORGANIZATION IS NULL AS org_absent
FROM INFORMATION_SCHEMA.ST_SPATIAL_REFERENCE_SYSTEMS
WHERE SRS_ID = 998877;

DROP SPATIAL REFERENCE SYSTEM 998877;

-- W7-D: a three-argument LAG/LEAD -- whose default argument sits where a frame
-- clause could otherwise start, and which MariaDB rejects outright -- over a
-- named window that a later reference frame-extends. This baseline omits the
-- optional FROM FIRST / RESPECT NULLS tokens; Wave 24 spells both explicitly.
CREATE TABLE sq_hard_w7_win (
  win_id INT PRIMARY KEY,
  bucket VARCHAR(10) NOT NULL,
  amount INT NOT NULL
) ENGINE = InnoDB;

INSERT INTO sq_hard_w7_win (win_id, bucket, amount)
VALUES (1, 'a', 10), (2, 'a', 30), (3, 'a', 20), (4, 'b', 40);

SELECT w.win_id,
       w.bucket,
       w.amount,
       NTH_VALUE(w.amount, 2) OVER (
         w_ord ROWS BETWEEN UNBOUNDED PRECEDING AND UNBOUNDED FOLLOWING
       )                                                AS second_value,
       FIRST_VALUE(w.amount) OVER (
         w_ord ROWS BETWEEN UNBOUNDED PRECEDING AND UNBOUNDED FOLLOWING
       )                                                AS earliest_value,
       LAG(w.amount, 2, -1) OVER w_ord                   AS lag_two_default,
       LEAD(w.amount, 2, -2) OVER w_ord                  AS lead_two_default,
       ROUND(PERCENT_RANK() OVER w_ord, 4)               AS pct_rank,
       ROUND(CUME_DIST() OVER w_ord, 4)                  AS cume
FROM sq_hard_w7_win w
WINDOW w_ord AS (PARTITION BY w.bucket ORDER BY w.amount)
ORDER BY w.win_id;

-- W7-E: the JSON mutation family nested into one expression, plus the 8.0.17
-- CAST target types that are otherwise only column-type keywords.
SELECT JSON_PRETTY(
         JSON_MERGE_PATCH(
           JSON_SET(
             JSON_REMOVE(
               JSON_ARRAY_INSERT(
                 JSON_ARRAY_APPEND(
                   JSON_OBJECT('tags', JSON_ARRAY('sql'), 'drop', 1),
                   '$.tags', 'json'),
                 '$.tags[0]', 'first'),
               '$.drop'),
             '$.depth', JSON_DEPTH(JSON_ARRAY(1, JSON_ARRAY(2)))),
           JSON_OBJECT('patched', TRUE))
       )                                                AS mutated,
       JSON_REPLACE('{"a":1}', '$.a', 9)                AS replaced,
       CAST(1 AS DOUBLE)                                AS as_double,
       CAST(2 AS FLOAT)                                 AS as_float,
       CAST(3 AS REAL)                                  AS as_real,
       CAST('4.5' AS DECIMAL(4, 2))                     AS as_decimal;

-- W7-F: table maintenance. RENAME INDEX / ALTER COLUMN SET DEFAULT read as
-- action phrases, and SHOW / CHECKSUM / FLUSH / ANALYZE are statements whose
-- second word is an object-kind keyword.
CREATE TABLE sq_hard_w7_maint (
  maint_id INT NOT NULL PRIMARY KEY,
  bucket   VARCHAR(10) NOT NULL,
  measured INT NOT NULL DEFAULT 0,
  KEY ix_before (bucket)
) ENGINE = InnoDB;

INSERT INTO sq_hard_w7_maint (maint_id, bucket, measured)
VALUES (1, 'a', 10), (2, 'b', 20);

ALTER TABLE sq_hard_w7_maint RENAME INDEX ix_before TO ix_after;
ALTER TABLE sq_hard_w7_maint ALTER COLUMN measured SET DEFAULT 7;
INSERT INTO sq_hard_w7_maint (maint_id, bucket) VALUES (3, 'c');
ALTER TABLE sq_hard_w7_maint ALTER COLUMN measured DROP DEFAULT;

SHOW CREATE TABLE sq_hard_w7_maint;
SHOW INDEX FROM sq_hard_w7_maint FROM sq_hard_mysql;
SHOW COLUMNS FROM sq_hard_w7_maint LIKE 'meas%';
CHECKSUM TABLE sq_hard_w7_maint QUICK;
FLUSH LOCAL TABLES sq_hard_w7_maint;
ANALYZE TABLE sq_hard_w7_maint;

SELECT maint_id, bucket, measured FROM sq_hard_w7_maint ORDER BY maint_id;

-- W7-G: lexer round. MySQL rejects a long dash ruler as a comment, so every
-- comment here is `-- text`; the rest is literals that impersonate comments and
-- terminators, an executable comment, and one unspaced line.
SELECT /*! 1 + */ 1                                      AS versioned_comment,
       'a -- not a comment /* nor this */; still text'    AS bait_text,
       0x4D7953514C                                      AS hex_literal,
       b'1011'                                           AS bit_literal,
       1.5e-3                                            AS sci_literal,
       X'4D79'                                           AS x_literal,
       `select`.`from`                                    AS quoted_path
FROM (SELECT 1 AS `from`) AS `select`;

SELECT(1+2)*3-4/2 AS crammed,MOD(7,3)modded,ABS(-8)absed FROM DUAL WHERE 1<>2;

sElEcT cAsE wHeN 1=1 tHeN 'mixed' eLsE 'case' eNd AS alternating;

-- W7-H: wave-7 self-verification.
CALL sq_hard_assert(
  (SELECT COUNT(*) FROM sq_hard_w7_txn) = 2
  AND (SELECT COUNT(*) FROM sq_hard_w7_txn WHERE txn_id = 2) = 0,
  'savepoint rollback'
);
CALL sq_hard_assert(
  (SELECT measured FROM sq_hard_w7_maint WHERE maint_id = 3) = 7,
  'alter column default'
);
CALL sq_hard_assert(
  (SELECT COUNT(*)
   FROM INFORMATION_SCHEMA.STATISTICS
   WHERE TABLE_SCHEMA = 'sq_hard_mysql'
     AND TABLE_NAME = 'sq_hard_w7_maint'
     AND INDEX_NAME = 'ix_after') = 1,
  'rename index'
);
CALL sq_hard_assert(
  (SELECT COUNT(*) FROM INFORMATION_SCHEMA.RESOURCE_GROUPS
   WHERE RESOURCE_GROUP_NAME = 'sq_hard_w7_rg') = 0
  AND (SELECT COUNT(*) FROM INFORMATION_SCHEMA.ST_SPATIAL_REFERENCE_SYSTEMS
       WHERE SRS_ID = 998877) = 0,
  'resource group + srs cleanup'
);

-- ULTRA WAVE 8: set-operation algebra with ALL, the TABLE statement carrying
-- query modifiers, the SET form of INSERT/REPLACE, account management with
-- password-policy clauses and JSON user attributes, JSON Schema validation
-- (including inside a CHECK constraint), the table-option and maintenance
-- families, persisted system variables, view algorithm/security headers,
-- EXCHANGE PARTITION, a general tablespace and a literal round.

-- W8-A: INTERSECT ALL and EXCEPT ALL keep the duplicates their DISTINCT forms
-- collapse, a parenthesised branch carries its own ORDER BY and LIMIT, and
-- `TABLE t ORDER BY ... LIMIT` is a whole query with no SELECT in it.
CREATE TABLE sq_hard_w8_fact (
  fact_id INT NOT NULL PRIMARY KEY,
  bucket  VARCHAR(10) NOT NULL,
  factor  INT NOT NULL
) ENGINE = InnoDB;

INSERT INTO sq_hard_w8_fact (fact_id, bucket, factor)
VALUES (1, 'alpha', 2), (2, 'alpha', 3), (3, 'alpha', 3),
       (4, 'beta', 5), (5, 'beta', 7);

SELECT factor FROM sq_hard_w8_fact WHERE factor < 6
INTERSECT ALL
SELECT factor FROM sq_hard_w8_fact WHERE factor > 2
ORDER BY factor;

(SELECT factor FROM sq_hard_w8_fact ORDER BY factor DESC LIMIT 3)
EXCEPT ALL
(SELECT 3)
ORDER BY factor;

SELECT bucket FROM sq_hard_w8_fact
UNION DISTINCT
SELECT 'gamma'
EXCEPT
SELECT 'beta'
ORDER BY bucket;

TABLE sq_hard_w8_fact ORDER BY factor DESC LIMIT 2;

-- W8-B: the assignment forms of INSERT and REPLACE put a SET clause where a
-- VALUES list belongs, and DEFAULT() reads a column's declared default from
-- inside that clause.
CREATE TABLE sq_hard_w8_assign (
  assign_id INT NOT NULL PRIMARY KEY,
  measured  INT,
  note      VARCHAR(30) DEFAULT 'defaulted'
) ENGINE = InnoDB;

INSERT INTO sq_hard_w8_assign SET assign_id = 1, measured = 7, note = 'inserted';
INSERT INTO sq_hard_w8_assign SET assign_id = 2, measured = 9, note = DEFAULT(note);
REPLACE INTO sq_hard_w8_assign SET assign_id = 1, measured = 11, note = 'replaced';

SELECT assign_id, measured, note
FROM sq_hard_w8_assign
ORDER BY assign_id;

-- W8-C: account management. The password-policy clauses are option words in
-- statement position, and ATTRIBUTE takes a JSON document that MySQL merges
-- into whatever the account already carried.
-- MySQL notes an authorization ID that is missing (3162) as readily as one that
-- already exists (3163), so no spelling of this reset is note-free on every run.
-- IF EXISTS is the quiet one from the second run onwards, which is what the
-- idempotency gate re-runs.
DROP USER IF EXISTS 'sq_hard_w8_user'@'localhost';
DROP ROLE IF EXISTS 'sq_hard_w8_role';

CREATE USER 'sq_hard_w8_user'@'localhost'
  IDENTIFIED WITH caching_sha2_password BY 'sq-Hard-w8-Secret-1'
  REQUIRE NONE
  WITH MAX_QUERIES_PER_HOUR 120
       MAX_USER_CONNECTIONS 2
  PASSWORD HISTORY 3
  PASSWORD REUSE INTERVAL 365 DAY
  FAILED_LOGIN_ATTEMPTS 3
  PASSWORD_LOCK_TIME 2
  ATTRIBUTE '{"team": "editor", "wave": 8}';

ALTER USER 'sq_hard_w8_user'@'localhost'
  ATTRIBUTE '{"wave": 8, "verified": true}';

CREATE ROLE 'sq_hard_w8_role';
GRANT SELECT, INSERT (measured, note) ON sq_hard_mysql.sq_hard_w8_assign
  TO 'sq_hard_w8_role';
GRANT 'sq_hard_w8_role' TO 'sq_hard_w8_user'@'localhost';
SET DEFAULT ROLE 'sq_hard_w8_role' TO 'sq_hard_w8_user'@'localhost';

SELECT JSON_EXTRACT(ATTRIBUTE, '$.team')     AS attribute_team,
       JSON_EXTRACT(ATTRIBUTE, '$.verified') AS attribute_verified
FROM INFORMATION_SCHEMA.USER_ATTRIBUTES
WHERE USER = 'sq_hard_w8_user'
  AND HOST = 'localhost';

SELECT COUNT(*) AS granted_columns
FROM INFORMATION_SCHEMA.COLUMN_PRIVILEGES
WHERE GRANTEE = "'sq_hard_w8_role'@'%'"
  AND TABLE_NAME = 'sq_hard_w8_assign';

REVOKE INSERT (measured, note) ON sq_hard_mysql.sq_hard_w8_assign
  FROM 'sq_hard_w8_role';

-- W8-D: JSON Schema validation. The schema is a JSON document inside a SQL
-- string, so quoting nests three deep, and the same call is legal inside a
-- CHECK constraint where it decides whether a row may exist at all.
SET @sq_hard_w8_schema = '{"type": "object",
                           "properties": {"n": {"type": "number", "minimum": 1},
                                          "tags": {"type": "array"}},
                           "required": ["n"]}';

SELECT JSON_SCHEMA_VALID(@sq_hard_w8_schema, '{"n": 5, "tags": ["ok"]}') AS schema_ok,
       JSON_SCHEMA_VALID(@sq_hard_w8_schema, '{"n": 0}')                 AS schema_bad,
       JSON_EXTRACT(
         JSON_SCHEMA_VALIDATION_REPORT(@sq_hard_w8_schema, '{"n": 0}'),
         '$.valid')                                                      AS report_valid;

CREATE TABLE sq_hard_w8_doc (
  doc_id INT NOT NULL PRIMARY KEY,
  doc    JSON NOT NULL,
  CONSTRAINT sq_hard_w8_doc_shape
    CHECK (JSON_SCHEMA_VALID('{"type": "object", "required": ["n"]}', doc))
) ENGINE = InnoDB;

INSERT INTO sq_hard_w8_doc (doc_id, doc) VALUES (1, '{"n": 3, "tags": ["a"]}');

SELECT doc_id,
       doc ->> '$.n'                       AS extracted_n,
       JSON_LENGTH(doc -> '$.tags')        AS tag_count
FROM sq_hard_w8_doc
ORDER BY doc_id;

-- W8-E: table options and the maintenance family. Every option is `name = value`
-- in a slot where a constraint could start, and each maintenance statement is a
-- bare verb followed by a table list and its own option word.
CREATE TABLE sq_hard_w8_opts (
  opt_id  INT NOT NULL PRIMARY KEY,
  payload VARCHAR(60),
  KEY ix_payload (payload(10) DESC)
) ENGINE = InnoDB
  STATS_PERSISTENT = 1
  STATS_AUTO_RECALC = 0
  STATS_SAMPLE_PAGES = 16
  ROW_FORMAT = DYNAMIC
  COMPRESSION = 'none'
  COMMENT = 'wave8 -- option zoo';

INSERT INTO sq_hard_w8_opts (opt_id, payload) VALUES (1, 'first'), (2, 'second');

CHECK TABLE sq_hard_w8_opts FOR UPGRADE;
ANALYZE TABLE sq_hard_w8_opts;
ALTER TABLE sq_hard_w8_opts ENGINE = InnoDB, ALGORITHM = INPLACE, LOCK = NONE;

SHOW COLUMNS FROM sq_hard_w8_opts LIKE 'pay%';

SELECT TABLE_ROWS IS NOT NULL AS has_row_estimate,
       ROW_FORMAT                                       AS row_format,
       TABLE_COMMENT                                    AS table_comment
FROM INFORMATION_SCHEMA.TABLES
WHERE TABLE_SCHEMA = 'sq_hard_mysql'
  AND TABLE_NAME = 'sq_hard_w8_opts';

-- W8-F: persisted system variables. `SET PERSIST` and `SET PERSIST_ONLY` write
-- mysqld-auto.cnf, `RESET PERSIST` takes an optional IF EXISTS, and the values
-- chosen here are the server defaults so the runtime state never changes.
SET PERSIST_ONLY innodb_deadlock_detect = ON;
SET PERSIST max_prepared_stmt_count = 16382;
SET @@SESSION.sql_select_limit = DEFAULT;

SELECT COUNT(*)                            AS persisted_rows,
       @@GLOBAL.max_prepared_stmt_count    AS prepared_limit,
       @@SESSION.sql_select_limit IS NOT NULL AS session_limit_set
FROM performance_schema.persisted_variables
WHERE VARIABLE_NAME IN ('innodb_deadlock_detect', 'max_prepared_stmt_count');

RESET PERSIST innodb_deadlock_detect;
RESET PERSIST IF EXISTS max_prepared_stmt_count;

-- W8-G: a view header can stack ALGORITHM, DEFINER and SQL SECURITY before the
-- word VIEW ever appears, and the check-option scope word decides whether an
-- UPDATE through the view may push a row out of it.
CREATE OR REPLACE ALGORITHM = TEMPTABLE
  DEFINER = CURRENT_USER
  SQL SECURITY INVOKER
  VIEW sq_hard_w8_temptable_view AS
SELECT fact_id, bucket, factor
FROM sq_hard_w8_fact
WHERE factor > 2;

CREATE OR REPLACE ALGORITHM = MERGE
  SQL SECURITY DEFINER
  VIEW sq_hard_w8_checked_view AS
SELECT fact_id, bucket, factor
FROM sq_hard_w8_fact
WHERE factor > 2
WITH LOCAL CHECK OPTION;

UPDATE sq_hard_w8_checked_view SET factor = factor + 1 WHERE fact_id = 4;

SELECT (SELECT COUNT(*) FROM sq_hard_w8_temptable_view) AS temptable_rows,
       (SELECT factor FROM sq_hard_w8_checked_view WHERE fact_id = 4) AS checked_factor,
       (SELECT VIEW_DEFINITION IS NOT NULL
        FROM INFORMATION_SCHEMA.VIEWS
        WHERE TABLE_SCHEMA = 'sq_hard_mysql'
          AND TABLE_NAME = 'sq_hard_w8_checked_view')  AS definition_present;

-- W8-H: EXCHANGE PARTITION swaps a partition's data with a whole table, so the
-- statement names two tables and a partition in one breath; the receiving table
-- must first have its own partitioning removed. A general tablespace then holds
-- a table through its own two-statement lifecycle.
CREATE TABLE sq_hard_w8_part (
  part_id INT NOT NULL PRIMARY KEY,
  reading INT
) ENGINE = InnoDB
PARTITION BY RANGE (part_id)
(PARTITION p_low VALUES LESS THAN (10),
 PARTITION p_high VALUES LESS THAN MAXVALUE);

INSERT INTO sq_hard_w8_part (part_id, reading) VALUES (1, 100), (11, 200);

CREATE TABLE sq_hard_w8_swap LIKE sq_hard_w8_part;
ALTER TABLE sq_hard_w8_swap REMOVE PARTITIONING;

ALTER TABLE sq_hard_w8_part
  EXCHANGE PARTITION p_low WITH TABLE sq_hard_w8_swap WITHOUT VALIDATION;

SELECT (SELECT COUNT(*) FROM sq_hard_w8_swap)                      AS swapped_rows,
       (SELECT COUNT(*) FROM sq_hard_w8_part PARTITION (p_low))    AS low_rows,
       (SELECT COUNT(*) FROM sq_hard_w8_part PARTITION (p_high))   AS high_rows;

CREATE TABLESPACE sq_hard_w8_ts ADD DATAFILE 'sq_hard_w8_ts.ibd' ENGINE = InnoDB;

CREATE TABLE sq_hard_w8_spaced (
  spaced_id INT NOT NULL PRIMARY KEY
) TABLESPACE sq_hard_w8_ts ENGINE = InnoDB;

INSERT INTO sq_hard_w8_spaced (spaced_id) VALUES (1);

SELECT COUNT(*) AS tablespace_tables
FROM INFORMATION_SCHEMA.TABLES
WHERE TABLE_SCHEMA = 'sq_hard_mysql'
  AND TABLE_NAME = 'sq_hard_w8_spaced';

DROP TABLE sq_hard_w8_spaced;
DROP TABLESPACE sq_hard_w8_ts;

-- W8-I: literal round. A charset introducer in front of a hex literal, 0x and
-- b'' forms, a double-quoted string (a string, not an identifier, while
-- ANSI_QUOTES is off), a backslash escape, and a `#` comment closing the line.
SELECT _utf8mb4 X'41'                          AS introduced_hex, # trailing hash
       0x42 + 0                                AS zero_x_literal,
       b'1010' + 0                             AS bit_literal,
       "double quoted is a string"             AS double_quoted,
       'back\\slash and \'quote\''             AS escaped_text,
       CHAR_LENGTH('tab\there')                AS escaped_length,
       _binary'binary introduced' = 'binary introduced' AS binary_compare;

-- W8-J: wave-8 self-verification.
CALL sq_hard_assert(
  (SELECT COUNT(*) FROM sq_hard_w8_assign) = 2
  AND (SELECT measured FROM sq_hard_w8_assign WHERE assign_id = 1) = 11
  AND (SELECT note FROM sq_hard_w8_assign WHERE assign_id = 2) = 'defaulted',
  'insert/replace set form'
);
CALL sq_hard_assert(
  (SELECT JSON_UNQUOTE(JSON_EXTRACT(ATTRIBUTE, '$.team'))
   FROM INFORMATION_SCHEMA.USER_ATTRIBUTES
   WHERE USER = 'sq_hard_w8_user' AND HOST = 'localhost') = 'editor'
  AND (SELECT COUNT(*) FROM mysql.default_roles
       WHERE USER = 'sq_hard_w8_user'
         AND DEFAULT_ROLE_USER = 'sq_hard_w8_role') = 1,
  'account attributes and default role'
);
CALL sq_hard_assert(
  JSON_SCHEMA_VALID(@sq_hard_w8_schema, '{"n": 5}') = 1
  AND JSON_SCHEMA_VALID(@sq_hard_w8_schema, '{"n": 0}') = 0
  AND (SELECT doc ->> '$.n' FROM sq_hard_w8_doc WHERE doc_id = 1) = '3',
  'json schema validation'
);
CALL sq_hard_assert(
  (SELECT COUNT(*) FROM INFORMATION_SCHEMA.TABLES
   WHERE TABLE_SCHEMA = 'sq_hard_mysql'
     AND TABLE_NAME = 'sq_hard_w8_opts'
     AND TABLE_COMMENT = 'wave8 -- option zoo') = 1,
  'table option zoo'
);
CALL sq_hard_assert(
  (SELECT COUNT(*) FROM performance_schema.persisted_variables
   WHERE VARIABLE_NAME IN ('innodb_deadlock_detect', 'max_prepared_stmt_count')) = 0,
  'persisted variables reset'
);
CALL sq_hard_assert(
  (SELECT factor FROM sq_hard_w8_fact WHERE fact_id = 4) = 6
  AND (SELECT COUNT(*) FROM sq_hard_w8_temptable_view) = 4,
  'view algorithms and check option'
);
CALL sq_hard_assert(
  (SELECT COUNT(*) FROM sq_hard_w8_swap) = 1
  AND (SELECT COUNT(*) FROM sq_hard_w8_part PARTITION (p_low)) = 0
  AND (SELECT COUNT(*) FROM sq_hard_w8_part PARTITION (p_high)) = 1,
  'exchange partition'
);

-- ULTRA WAVE 9: a mid-script sql_mode island where PIPES_AS_CONCAT turns `||`
-- into concatenation, ANSI_QUOTES turns double quotes into identifiers and
-- NO_BACKSLASH_ESCAPES turns `\` back into an ordinary character -- three
-- lexical rules changed by one statement; the predicate and operator zoo
-- (SOUNDS LIKE / RLIKE / ESCAPE / <=> / XOR / inline assignment); DATE_FORMAT
-- and STR_TO_DATE format models; a character-set and collation round with
-- introducers, WEIGHT_STRING and accent-sensitive comparison; a foreign server
-- definition, a HASH-indexed MEMORY table and geometry collections; and a
-- delimiter round that redefines the statement terminator three times.

CREATE TABLE sq_hard_w9_note (
  note_key   VARCHAR(30) NOT NULL PRIMARY KEY,
  note_text  VARCHAR(400),
  note_value BIGINT
) ENGINE = InnoDB;

CREATE TABLE sq_hard_w9_staff (
  staff_id   INT NOT NULL PRIMARY KEY,
  staff_name VARCHAR(30) NOT NULL,
  grade      INT NOT NULL
) ENGINE = InnoDB;

INSERT INTO sq_hard_w9_staff (staff_id, staff_name, grade)
VALUES (1, 'ada', 3), (2, 'linus', 5), (3, 'grace', 7);

-- W9-A: the lexical island. Inside it `||` concatenates, "staff_name" is an
-- identifier rather than a string, and a backslash is just a backslash -- so the
-- same three tokens mean something different on either side of the SET.
SELECT (1 || 0)                        AS pipes_before,
       'back\\slash'                   AS escaped_before,
       LENGTH('one\ttwo')              AS escaped_length_before
FROM sq_hard_w9_staff
LIMIT 1;

SET SESSION sql_mode = 'STRICT_ALL_TABLES,PIPES_AS_CONCAT,ANSI_QUOTES,NO_BACKSLASH_ESCAPES';

SELECT "staff_name"                    AS quoted_identifier,
       "staff_name" || '/' || grade    AS piped_concat,
       'back\slash'                    AS literal_backslash,
       LENGTH('one\ttwo')              AS literal_length,
       'quote '' inside'               AS doubled_quote
FROM sq_hard_w9_staff
WHERE "grade" >= 5
ORDER BY "staff_id";

SET @sq_hard_w9_island = (SELECT "staff_name" || ':' || "grade"
                          FROM sq_hard_w9_staff
                          WHERE "staff_id" = 3);
SET @sq_hard_w9_backslash = (SELECT LENGTH('one\ttwo'));

SET SESSION sql_mode = 'STRICT_ALL_TABLES';

-- Outside the island the same expressions mean the old things again: `||` is
-- OR, a double-quoted word is a string, and `\t` is one control character.
SELECT (1 || 0)                        AS pipes_after,
       "staff_name"                    AS string_after,
       LENGTH('one\ttwo')              AS escaped_length_after,
       @sq_hard_w9_island              AS island_value,
       @sq_hard_w9_backslash           AS island_length;

INSERT INTO sq_hard_w9_note (note_key, note_text, note_value)
VALUES ('sql-mode-island', @sq_hard_w9_island, @sq_hard_w9_backslash);

-- W9-B: the predicate and operator zoo. Several of these are two-word operators
-- that look like keywords, and the projection mutates a user variable in place.
SET @sq_hard_w9_row = 0;

SELECT s.staff_name,
       @sq_hard_w9_row := @sq_hard_w9_row + 1              AS running_row,
       s.staff_name SOUNDS LIKE 'adda'                     AS sounds_like_ada,
       s.staff_name RLIKE '^[ag]'                          AS starts_a_or_g,
       s.staff_name NOT REGEXP 'z$'                        AS not_ending_z,
       s.staff_name LIKE 'a|_a%' ESCAPE '|'                AS escaped_like,
       CAST(s.staff_name AS BINARY) = 'ADA'                AS binary_compare,
       s.grade <=> NULL                                    AS null_safe_equal,
       (s.grade > 2) XOR (s.grade > 6)                     AS exclusive_or,
       s.grade DIV 2                                       AS integer_divide,
       s.grade MOD 2                                       AS modulo,
       CASE s.grade WHEN 3 THEN 'low' WHEN 5 THEN 'mid' ELSE 'high' END AS simple_case,
       CASE WHEN s.grade BETWEEN 4 AND 8 THEN 'band' ELSE 'edge' END    AS searched_case,
       COALESCE(NULLIF(s.grade, 5), -1)                    AS nullif_value,
       GREATEST(s.grade, 4) + LEAST(s.grade, 4)            AS bounded_sum,
       INTERVAL(s.grade, 2, 4, 6, 8)                       AS interval_slot
FROM sq_hard_w9_staff s
ORDER BY s.staff_id;

SELECT DATE '2024-02-29' + INTERVAL 1 DAY                  AS next_day,
       DATE '2024-02-29' - INTERVAL '1-2' YEAR_MONTH       AS back_a_year,
       TIMESTAMPDIFF(MONTH, DATE '2024-02-29', DATE '2025-01-31') AS month_span,
       TIMESTAMPADD(QUARTER, 2, DATE '2024-02-29')         AS two_quarters,
       (SELECT COUNT(*) FROM sq_hard_w9_staff WHERE grade IS NOT NULL) AS graded_rows;

-- W9-C: date and number format models. The format string is a mini-language of
-- percent escapes -- including a doubled `%%` -- and STR_TO_DATE parses one back.
SELECT DATE_FORMAT(DATE '2024-02-29', '%Y-%m-%dT%H:%i:%s')     AS iso_stamp,
       DATE_FORMAT(DATE '2024-02-29', '%W the %D of %M %Y')    AS long_form,
       DATE_FORMAT(DATE '2024-02-29', 'week %v of %x (%%)')    AS iso_week,
       DATE_FORMAT(TIME '13:45:56', '%r / %T / %f')            AS clock_forms,
       STR_TO_DATE('29,2,2024', '%d,%m,%Y')                    AS parsed_date,
       STR_TO_DATE('2024-02-29 13:45:56', '%Y-%m-%d %H:%i:%s') AS parsed_stamp,
       FORMAT(1234567.891, 2, 'de_DE')                         AS german_number,
       FORMAT(1234567.891, 2)                                  AS plain_number,
       LPAD(CONV(255, 10, 16), 6, '0')                         AS hex_padded,
       CAST(CONV('ff', 16, 10) AS UNSIGNED)                    AS from_hex;

INSERT INTO sq_hard_w9_note (note_key, note_text, note_value)
VALUES ('formats',
        DATE_FORMAT(DATE '2024-02-29', 'week %v of %x (%%)'),
        LENGTH(FORMAT(1234567.891, 2, 'de_DE')));

-- W9-D: character sets and collations. An introducer binds a charset to a
-- literal before any function sees it, COLLATE re-binds comparison rules
-- mid-expression, and WEIGHT_STRING exposes the sort key itself.
SELECT _utf8mb4'Ärger' COLLATE utf8mb4_0900_ai_ci = _utf8mb4'arger' AS accent_insensitive,
       _utf8mb4'Ärger' COLLATE utf8mb4_0900_as_cs = _utf8mb4'arger' AS accent_sensitive,
       _utf8mb4 0x53514C                                           AS introduced_hex,
       HEX(WEIGHT_STRING('ab' AS CHAR(4)))                         AS sort_key,
       HEX(CONVERT('Ärger' USING latin1))                          AS converted_hex,
       CHAR(83, 81, 76 USING utf8mb4)                              AS built_chars,
       ORD('Ä')                                                    AS first_codepoint,
       CHARSET('literal')                                          AS literal_charset,
       COLLATION(_utf8mb4'x' COLLATE utf8mb4_bin)                  AS bound_collation,
       REGEXP_SUBSTR('sq-hard-9', '[0-9]+', 1, 1, 'c')             AS matched_digits,
       REGEXP_LIKE('SQ', 'sq', 'i')                                AS case_insensitive_match,
       REGEXP_REPLACE('a1b2', '[0-9]', '#', 1, 0, 'c')             AS masked_digits;

-- W9-E: a foreign server definition, a MEMORY table whose secondary index is a
-- hash, and geometry collections built from well-known text.
DROP SERVER IF EXISTS sq_hard_w9_srv;
CREATE SERVER sq_hard_w9_srv
  FOREIGN DATA WRAPPER mysql
  OPTIONS (HOST '127.0.0.1', DATABASE 'sq_hard_mysql', USER 'root',
           PASSWORD 'spacequery', PORT 3306, SOCKET '', OWNER 'root');

SELECT server_name, db, port
FROM mysql.servers
WHERE server_name = 'sq_hard_w9_srv';

CREATE TABLE sq_hard_w9_cache (
  cache_id  INT NOT NULL PRIMARY KEY,
  cache_key VARCHAR(20) NOT NULL,
  hits      INT NOT NULL,
  INDEX sq_hard_w9_cache_hash (cache_key) USING HASH
) ENGINE = MEMORY;

INSERT INTO sq_hard_w9_cache (cache_id, cache_key, hits)
VALUES (1, 'alpha', 10), (2, 'beta', 20);

SELECT cache_key, hits FROM sq_hard_w9_cache WHERE cache_key = 'beta';

CREATE TABLE sq_hard_w9_shape (
  shape_id INT NOT NULL PRIMARY KEY,
  shape    GEOMETRY NOT NULL
) ENGINE = InnoDB;

INSERT INTO sq_hard_w9_shape (shape_id, shape)
VALUES (1, ST_GeomFromText('GEOMETRYCOLLECTION(POINT(1 1),LINESTRING(0 0,2 2))')),
       (2, ST_GeomFromText('POLYGON((0 0,4 0,4 4,0 4,0 0))'));

SELECT shape_id,
       ST_GeometryType(shape)                                  AS geom_type,
       ST_NumGeometries(shape)                                 AS parts,
       CASE WHEN ST_GeometryType(shape) = 'POLYGON'
            THEN ROUND(ST_Area(shape), 2) END                  AS area,
       CASE WHEN ST_GeometryType(shape) = 'POLYGON'
            THEN ST_AsText(ST_Centroid(shape)) END             AS centroid,
       CASE WHEN ST_GeometryType(shape) = 'POLYGON'
            THEN ST_Contains(shape, ST_GeomFromText('POINT(1 1)')) END AS holds_point
FROM sq_hard_w9_shape
ORDER BY shape_id;

DROP SERVER sq_hard_w9_srv;

-- W9-F: the delimiter round. The terminator is redefined three times, and each
-- routine body carries the *other* delimiters inside a string literal and a
-- comment so a naive splitter cuts the body apart.
DELIMITER ;;
CREATE FUNCTION sq_hard_w9_semi() RETURNS VARCHAR(60) DETERMINISTIC
BEGIN
  -- this comment holds a $$ and a ;; on purpose
  RETURN 'body with ; and $$ and ;; inside a string';
END;;
DELIMITER //
CREATE FUNCTION sq_hard_w9_slash() RETURNS VARCHAR(60) DETERMINISTIC
BEGIN
  /* block comment carrying // and ;; */
  RETURN CONCAT('slashed', ';;', '//');
END//
DELIMITER $$
CREATE PROCEDURE sq_hard_w9_delim_probe(OUT joined VARCHAR(140))
BEGIN
  SET joined = CONCAT(sq_hard_w9_semi(), '|', sq_hard_w9_slash());
END$$
DELIMITER ;

CALL sq_hard_w9_delim_probe(@sq_hard_w9_delims);

SELECT @sq_hard_w9_delims AS delimiter_round;

INSERT INTO sq_hard_w9_note (note_key, note_text, note_value)
VALUES ('delimiters', @sq_hard_w9_delims, LENGTH(@sq_hard_w9_delims));

-- W9-G: the aggregate zoo and column-positioning DDL. Bitwise and statistical
-- aggregates sit beside JSON constructors that build their own documents, an
-- ordered GROUP_CONCAT carries a SEPARATOR keyword where a comma belongs, and
-- ALTER TABLE moves a column by name with CHANGE ... AFTER, then FIRST.
SELECT s.grade MOD 2                                          AS parity,
       COUNT(*)                                               AS in_group,
       ANY_VALUE(s.staff_name)                                AS any_name,
       BIT_AND(s.grade)                                       AS bit_and_grade,
       BIT_OR(s.grade)                                        AS bit_or_grade,
       BIT_XOR(s.grade)                                       AS bit_xor_grade,
       ROUND(VARIANCE(s.grade), 4)                            AS population_variance,
       ROUND(VAR_SAMP(s.grade), 4)                            AS sample_variance,
       ROUND(STDDEV_POP(s.grade), 4)                          AS population_stddev,
       GROUP_CONCAT(s.staff_name ORDER BY s.grade DESC SEPARATOR ' > ') AS ranked_names,
       JSON_ARRAYAGG(s.grade)                                 AS grades_json,
       JSON_OBJECTAGG(s.staff_name, s.grade)                  AS by_name_json
FROM sq_hard_w9_staff s
GROUP BY s.grade MOD 2 WITH ROLLUP
ORDER BY parity;

SELECT GROUPING(s.grade MOD 2)                                AS is_rollup_row,
       SUM(s.grade)                                           AS grade_total
FROM sq_hard_w9_staff s
GROUP BY s.grade MOD 2 WITH ROLLUP
HAVING is_rollup_row = 1;

ALTER TABLE sq_hard_w9_staff
  ADD COLUMN hired_on DATE NULL DEFAULT NULL AFTER staff_name,
  ADD COLUMN badge VARCHAR(10) NULL FIRST;

UPDATE sq_hard_w9_staff
SET badge = CONCAT('B', LPAD(staff_id, 3, '0')),
    hired_on = DATE '2024-02-29' + INTERVAL staff_id DAY;

ALTER TABLE sq_hard_w9_staff
  CHANGE COLUMN badge badge_code VARCHAR(12) NULL AFTER grade,
  MODIFY COLUMN hired_on DATE NULL DEFAULT NULL FIRST;

SELECT ordinal_position, column_name, column_type
FROM information_schema.columns
WHERE table_schema = 'sq_hard_mysql'
  AND table_name = 'sq_hard_w9_staff'
ORDER BY ordinal_position;

SELECT hired_on, staff_id, staff_name, grade, badge_code
FROM sq_hard_w9_staff
ORDER BY staff_id;

SET @sq_hard_w9_badges = (SELECT GROUP_CONCAT(badge_code ORDER BY staff_id SEPARATOR '/')
                          FROM sq_hard_w9_staff);

INSERT INTO sq_hard_w9_note (note_key, note_text, note_value)
VALUES ('aggregates', @sq_hard_w9_badges,
        (SELECT BIT_XOR(grade) FROM sq_hard_w9_staff));

-- Wave-9 self-verification.
CALL sq_hard_assert(
  @sq_hard_w9_island = 'grace:7',
  CONCAT('sql_mode island value: ', COALESCE(@sq_hard_w9_island, 'NULL')));
CALL sq_hard_assert(
  @sq_hard_w9_backslash = 8,
  CONCAT('no-backslash-escape length: ', COALESCE(@sq_hard_w9_backslash, 'NULL')));
CALL sq_hard_assert(
  (SELECT COUNT(*) FROM sq_hard_w9_shape
   WHERE ST_GeometryType(shape) = 'GEOMCOLLECTION') = 1,
  'geometry collection stored');
CALL sq_hard_assert(
  (SELECT hits FROM sq_hard_w9_cache WHERE cache_key = 'beta') = 20,
  'memory hash index lookup');
CALL sq_hard_assert(
  (SELECT note_value FROM sq_hard_w9_note WHERE note_key = 'delimiters') = 53,
  CONCAT('delimiter round length: ',
         (SELECT note_value FROM sq_hard_w9_note WHERE note_key = 'delimiters')));
CALL sq_hard_assert(
  @sq_hard_w9_badges = 'B001/B002/B003',
  CONCAT('badge codes: ', COALESCE(@sq_hard_w9_badges, 'NULL')));
CALL sq_hard_assert(
  (SELECT COUNT(*) FROM sq_hard_w9_note) = 4,
  CONCAT('wave9 note rows: ', (SELECT COUNT(*) FROM sq_hard_w9_note)));

-- ULTRA WAVE 10: the JSON_TABLE row source with a NESTED PATH, FOR ORDINALITY
-- and EXISTS/DEFAULT-ON-ERROR columns beside JSON_VALUE ... RETURNING and a
-- JSON Schema validation report; distributed transactions driven by the XA
-- statement family including XA RECOVER CONVERT XID; the event scheduler DDL
-- (ON SCHEDULE EVERY/AT with STARTS/ENDS, ON COMPLETION PRESERVE, ALTER EVENT
-- RENAME TO); instance-level backup locking with an INSTANT algorithm ALTER, an
-- ngram full-text parser and a generated invisible primary key; and lexer
-- round 5 whose backtick identifiers and literals carry the terminator, every
-- comment introducer and a line break.

CREATE TABLE sq_hard_w10_note (
  note_key   VARCHAR(30) NOT NULL PRIMARY KEY,
  note_text  VARCHAR(400),
  note_value BIGINT
) ENGINE = InnoDB;

-- W10-A: JSON_TABLE. The COLUMNS list mints relation columns that exist nowhere
-- in the catalog, a NESTED PATH opens a second COLUMNS list one level down,
-- FOR ORDINALITY invents a counter, EXISTS and DEFAULT ... ON EMPTY|ON ERROR
-- change what a missing path yields, and JSON_VALUE re-types the result.
CREATE TABLE sq_hard_w10_profile (
  profile_id INT NOT NULL PRIMARY KEY,
  doc        JSON NOT NULL
) ENGINE = InnoDB;

INSERT INTO sq_hard_w10_profile (profile_id, doc)
VALUES (1, '{"name":"atlas","score":42,"roles":{"admin":true},"tags":["x","y","z"]}'),
       (2, '{"name":"borg","tags":["q"]}');

SELECT p.profile_id, jt.profile_name, jt.has_admin, jt.score, jt.tag_pos, jt.tag
FROM sq_hard_w10_profile p,
     JSON_TABLE(p.doc, '$'
       COLUMNS (profile_name VARCHAR(30) PATH '$.name',
                has_admin    INT         EXISTS PATH '$.roles.admin',
                score        INT         PATH '$.score' DEFAULT '0' ON EMPTY
                                                        DEFAULT '-1' ON ERROR,
                NESTED PATH '$.tags[*]'
                  COLUMNS (tag_pos FOR ORDINALITY,
                           tag     VARCHAR(20) PATH '$'))) AS jt
ORDER BY p.profile_id, jt.tag_pos;

SELECT tags.tag, COUNT(*) AS seen
FROM sq_hard_w10_profile p,
     JSON_TABLE(p.doc, '$.tags[*]' COLUMNS (tag VARCHAR(20) PATH '$')) AS tags
GROUP BY tags.tag
ORDER BY tags.tag;

SELECT p.profile_id,
       JSON_VALUE(p.doc, '$.score' RETURNING DECIMAL(6, 2) DEFAULT -1.00 ON EMPTY
                                                           DEFAULT -2.00 ON ERROR) AS typed_score,
       JSON_VALUE(p.doc, '$.name' RETURNING CHAR(10))                              AS typed_name
FROM sq_hard_w10_profile p
ORDER BY p.profile_id;

SELECT JSON_EXTRACT(
         JSON_SCHEMA_VALIDATION_REPORT(
           '{"type":"object","required":["name","score"]}',
           doc),
         '$.valid') AS schema_valid
FROM sq_hard_w10_profile
ORDER BY profile_id;

SET @sq_hard_w10_tag_total = (
  SELECT COUNT(*)
  FROM sq_hard_w10_profile p,
       JSON_TABLE(p.doc, '$.tags[*]' COLUMNS (tag VARCHAR(20) PATH '$')) AS tags);

SET @sq_hard_w10_score_sum = (
  SELECT SUM(jt.score)
  FROM sq_hard_w10_profile p,
       JSON_TABLE(p.doc, '$'
         COLUMNS (score INT PATH '$.score' DEFAULT '0' ON EMPTY
                                           DEFAULT '-1' ON ERROR)) AS jt);

INSERT INTO sq_hard_w10_note (note_key, note_text, note_value)
VALUES ('json-table',
        CONCAT('tags ', @sq_hard_w10_tag_total, ' scores ', @sq_hard_w10_score_sum),
        @sq_hard_w10_tag_total + @sq_hard_w10_score_sum);

-- W10-B: distributed transactions. Every XA verb takes a three-part transaction
-- identifier where a table name would normally sit, the branch moves through
-- ACTIVE, IDLE and PREPARED states before it can commit, and XA RECOVER takes
-- an option word that changes how the identifier is rendered.
CREATE TABLE sq_hard_w10_xa_log (
  log_id INT NOT NULL PRIMARY KEY,
  note   VARCHAR(30) NOT NULL
) ENGINE = InnoDB;

XA START 'sq_hard_w10_xa', 'branch_one', 1;
INSERT INTO sq_hard_w10_xa_log (log_id, note) VALUES (1, 'committed branch');
XA END 'sq_hard_w10_xa', 'branch_one', 1;
XA PREPARE 'sq_hard_w10_xa', 'branch_one', 1;
XA COMMIT 'sq_hard_w10_xa', 'branch_one', 1;

XA START 'sq_hard_w10_xb', 'branch_two', 1;
INSERT INTO sq_hard_w10_xa_log (log_id, note) VALUES (2, 'rolled back branch');
XA END 'sq_hard_w10_xb', 'branch_two', 1;
XA ROLLBACK 'sq_hard_w10_xb', 'branch_two', 1;

XA RECOVER CONVERT XID;

SELECT log_id, note FROM sq_hard_w10_xa_log ORDER BY log_id;

INSERT INTO sq_hard_w10_note (note_key, note_text, note_value)
VALUES ('xa', 'prepare/commit and rollback branches',
        (SELECT COUNT(*) FROM sq_hard_w10_xa_log));

-- W10-C: the event scheduler DDL. The schedule clause is a small grammar of its
-- own -- EVERY <n> <unit> with STARTS and ENDS offsets, or AT a single instant
-- -- and the body after DO is a complete statement nested inside the DDL.
CREATE EVENT sq_hard_w10_evt
  ON SCHEDULE EVERY 1 DAY
    STARTS CURRENT_TIMESTAMP + INTERVAL 1 HOUR
    ENDS CURRENT_TIMESTAMP + INTERVAL 30 DAY
  ON COMPLETION PRESERVE
  DISABLE
  COMMENT 'w10 recurring event'
  DO UPDATE sq_hard_w10_note
     SET note_value = note_value + 1
     WHERE note_key = 'json-table';

CREATE EVENT sq_hard_w10_once
  ON SCHEDULE AT CURRENT_TIMESTAMP + INTERVAL 1 YEAR
  ON COMPLETION NOT PRESERVE
  DISABLE
  DO DELETE FROM sq_hard_w10_xa_log WHERE log_id < 0;

ALTER EVENT sq_hard_w10_evt RENAME TO sq_hard_w10_evt_renamed;

SELECT EVENT_NAME, STATUS, ON_COMPLETION, INTERVAL_VALUE, INTERVAL_FIELD, EVENT_TYPE
FROM INFORMATION_SCHEMA.EVENTS
WHERE EVENT_SCHEMA = 'sq_hard_mysql'
ORDER BY EVENT_NAME;

SET @sq_hard_w10_events = (SELECT COUNT(*) FROM INFORMATION_SCHEMA.EVENTS
                           WHERE EVENT_SCHEMA = 'sq_hard_mysql'
                             AND EVENT_NAME LIKE 'sq\_hard\_w10%');

DROP EVENT sq_hard_w10_evt_renamed;
DROP EVENT IF EXISTS sq_hard_w10_once;

INSERT INTO sq_hard_w10_note (note_key, note_text, note_value)
VALUES ('events', 'every-day and at-once schedules', @sq_hard_w10_events);

-- W10-D: instance-level backup locking around an INSTANT-algorithm ALTER, a
-- full-text index that names its own parser plugin, and a table whose primary
-- key the server invents and hides.
LOCK INSTANCE FOR BACKUP;

CREATE TABLE sq_hard_w10_doc (
  doc_id INT NOT NULL PRIMARY KEY,
  body   TEXT,
  FULLTEXT KEY ft_w10_body (body) WITH PARSER ngram
) ENGINE = InnoDB;

UNLOCK INSTANCE;

INSERT INTO sq_hard_w10_doc (doc_id, body)
VALUES (1, '데이터베이스 엔진 테스트'),
       (2, '쿼리 편집기 하이라이팅'),
       (3, 'plain ascii body without ngram tokens');

CREATE TABLE sq_hard_w10_instant (
  row_id INT NOT NULL PRIMARY KEY,
  label  VARCHAR(20) NOT NULL
) ENGINE = InnoDB;

INSERT INTO sq_hard_w10_instant (row_id, label) VALUES (1, 'before');

ALTER TABLE sq_hard_w10_instant
  ADD COLUMN tag_count INT NOT NULL DEFAULT 0,
  ALGORITHM = INSTANT;

ALTER TABLE sq_hard_w10_instant
  ADD COLUMN reviewed TINYINT NOT NULL DEFAULT 0 AFTER label,
  ALGORITHM = INSTANT;

ALTER TABLE sq_hard_w10_instant
  ALTER COLUMN tag_count SET DEFAULT 7,
  ALGORITHM = INSTANT;

ALTER TABLE sq_hard_w10_instant
  ADD INDEX ix_w10_instant_label (label),
  ALGORITHM = INPLACE, LOCK = NONE;

SELECT row_id, label, reviewed, tag_count FROM sq_hard_w10_instant ORDER BY row_id;

SELECT d.doc_id,
       CASE WHEN MATCH(d.body) AGAINST('데이터' IN BOOLEAN MODE) > 0
            THEN 1 ELSE 0 END AS matched_ngram
FROM sq_hard_w10_doc d
ORDER BY d.doc_id;

SET SESSION sql_generate_invisible_primary_key = ON;

CREATE TABLE sq_hard_w10_gipk (
  label VARCHAR(20) NOT NULL
) ENGINE = InnoDB;

SET SESSION sql_generate_invisible_primary_key = OFF;

INSERT INTO sq_hard_w10_gipk (label) VALUES ('alpha'), ('beta');

SELECT COLUMN_NAME, EXTRA, COLUMN_KEY
FROM INFORMATION_SCHEMA.COLUMNS
WHERE TABLE_SCHEMA = 'sq_hard_mysql'
  AND TABLE_NAME = 'sq_hard_w10_gipk'
ORDER BY ORDINAL_POSITION;

SELECT my_row_id, label FROM sq_hard_w10_gipk ORDER BY my_row_id;

SET @sq_hard_w10_ngram = (
  SELECT COUNT(*) FROM sq_hard_w10_doc
  WHERE MATCH(body) AGAINST('데이터' IN BOOLEAN MODE) > 0);

INSERT INTO sq_hard_w10_note (note_key, note_text, note_value)
VALUES ('storage', 'backup lock, instant alter, ngram, invisible pk',
        @sq_hard_w10_ngram);

-- W10-E: lexer round 5. Backtick identifiers carry the terminator, both SQL
-- comment introducers and the hash comment; literals span a line break and
-- impersonate a compound statement; every numeric and binary literal spelling
-- appears side by side; and a routine body hides the active delimiter inside
-- each of the three comment forms.
SELECT 1 AS `semi;colon`,
       2 AS `dash--dash`,
       3 AS `slash/*star`,
       4 AS `hash#comment`,
       5 AS `it's`,
       6 AS `Space Inside`
FROM DUAL;

SELECT '-- not a comment /* either */ # nor this' AS literal_comment,
       "double quoted string"                     AS dq_string,
       0x41                                       AS hex_literal,
       X'42'                                      AS x_literal,
       b'1010' + 0                                AS bit_literal,
       0b1011 + 0                                 AS zerob_literal,
       _utf8mb4 0x4142                            AS introduced_literal,
       CHAR_LENGTH('line one
line two')                                        AS multiline_len
FROM DUAL # trailing hash comment
;

SET @sq_hard_w10_block = 'BEGIN NOT ATOMIC DECLARE x INT; END;';

DELIMITER $$
CREATE PROCEDURE sq_hard_w10_lexer(OUT out_len INT)
BEGIN
  -- a double-dash comment holding the delimiter $$ and a terminator ;
  # a hash comment holding the delimiter $$ and a terminator ;
  /* a block comment holding the delimiter $$ and a terminator ; */
  DECLARE payload VARCHAR(200) DEFAULT 'body with $$ and ; and -- and # inside';
  SET out_len = CHAR_LENGTH(payload) + CHAR_LENGTH(@sq_hard_w10_block);
END$$
DELIMITER ;

CALL sq_hard_w10_lexer(@sq_hard_w10_lexer_len);

SELECT @sq_hard_w10_block AS block_literal,
       @sq_hard_w10_lexer_len AS lexer_len;

INSERT INTO sq_hard_w10_note (note_key, note_text, note_value)
VALUES ('lexer', @sq_hard_w10_block, @sq_hard_w10_lexer_len);

-- Wave-10 self-verification.
CALL sq_hard_assert(
  @sq_hard_w10_tag_total = 4,
  CONCAT('json_table tag rows: ', COALESCE(@sq_hard_w10_tag_total, 'NULL')));
CALL sq_hard_assert(
  @sq_hard_w10_score_sum = 42,
  CONCAT('json_table score sum: ', COALESCE(@sq_hard_w10_score_sum, 'NULL')));
CALL sq_hard_assert(
  (SELECT COUNT(*) FROM sq_hard_w10_xa_log) = 1,
  CONCAT('xa log rows: ', (SELECT COUNT(*) FROM sq_hard_w10_xa_log)));
CALL sq_hard_assert(
  @sq_hard_w10_events = 2,
  CONCAT('event count: ', COALESCE(@sq_hard_w10_events, 'NULL')));
CALL sq_hard_assert(
  @sq_hard_w10_ngram = 1,
  CONCAT('ngram matches: ', COALESCE(@sq_hard_w10_ngram, 'NULL')));
CALL sq_hard_assert(
  (SELECT COUNT(*) FROM sq_hard_w10_gipk) = 2,
  CONCAT('invisible pk rows: ', (SELECT COUNT(*) FROM sq_hard_w10_gipk)));
CALL sq_hard_assert(
  @sq_hard_w10_lexer_len = 74,
  CONCAT('lexer length: ', COALESCE(@sq_hard_w10_lexer_len, 'NULL')));
CALL sq_hard_assert(
  (SELECT COUNT(*) FROM sq_hard_w10_note) = 5,
  CONCAT('wave10 note rows: ', (SELECT COUNT(*) FROM sq_hard_w10_note)));

-- WAVE 11 -- the declaration surface: every numeric/string/temporal type
-- spelling with its option words, an sql_mode island that changes what an
-- expression MEANS rather than how it is spelled, the optimizer-hint grammar
-- with query-block labels, the sys schema's own functions, dual-password
-- account rotation, the temporal/string builtin zoo, and a sixth lexer round.
CREATE TABLE sq_hard_w11_note (
  note_key   VARCHAR(30) NOT NULL PRIMARY KEY,
  note_text  VARCHAR(400),
  note_value BIGINT
) ENGINE = InnoDB;

-- W11-A: the data-type zoo. UNSIGNED stacks on the integer family, DECIMAL
-- carries precision and scale, BIT/ENUM/SET/YEAR are value-set types,
-- fractional seconds appear on TIME and on DATETIME with CURRENT_TIMESTAMP(6)
-- as both DEFAULT and ON UPDATE, and the blob/text ladder takes its own
-- CHARACTER SET and COLLATE. information_schema proves what the server stored.
CREATE TABLE sq_hard_w11_type (
  row_id       INT UNSIGNED NOT NULL,
  tiny_flag    TINYINT UNSIGNED,
  medium_num   MEDIUMINT,
  big_num      BIGINT UNSIGNED,
  wide_dec     DECIMAL(30, 10),
  double_free  DOUBLE,
  bit_mask     BIT(8),
  grade        ENUM('alpha', 'beta', 'gamma') DEFAULT 'beta',
  flags        SET('read', 'write', 'admin'),
  made_year    YEAR,
  precise_time TIME(6),
  made_at      DATETIME(6) DEFAULT CURRENT_TIMESTAMP(6)
                 ON UPDATE CURRENT_TIMESTAMP(6),
  stamped      TIMESTAMP NULL DEFAULT NULL,
  raw_bytes    VARBINARY(16),
  tiny_note    TINYTEXT CHARACTER SET utf8mb4 COLLATE utf8mb4_bin,
  blob_note    MEDIUMBLOB,
  padded       CHAR(10),
  doc          JSON,
  PRIMARY KEY (row_id)
) ENGINE = InnoDB;

INSERT INTO sq_hard_w11_type (row_id, tiny_flag, medium_num, big_num, wide_dec,
                              double_free, bit_mask, grade, flags, made_year,
                              precise_time, stamped, raw_bytes, tiny_note,
                              blob_note, padded, doc)
VALUES (7, 1, -8388608, 18446744073709551615, 12345.0123456789, 2.5,
        b'10101010', 'gamma', 'read,admin', 2024, '01:02:03.456789',
        TIMESTAMP '2024-05-06 07:08:09', X'DEADBEEF', 'tiny note', X'0102',
        'abc', JSON_OBJECT('kind', 'type-zoo', 'depth', 2));

INSERT INTO sq_hard_w11_type (row_id, medium_num, flags, made_year, padded)
VALUES (8, 8388607, '', 2000, 'de');

SELECT t.row_id,
       t.tiny_flag,
       t.medium_num,
       t.big_num,
       t.wide_dec,
       t.bit_mask + 0                AS bit_value,
       t.grade,
       t.flags,
       FIND_IN_SET('admin', t.flags) AS admin_bit,
       t.made_year,
       t.precise_time,
       HEX(t.raw_bytes)              AS raw_hex,
       t.made_at IS NOT NULL         AS made_at_set,
       OCTET_LENGTH(t.blob_note)     AS blob_bytes,
       CHAR_LENGTH(t.padded)         AS pad_len,
       JSON_VALUE(t.doc, '$.kind')   AS doc_kind
FROM sq_hard_w11_type t
ORDER BY t.row_id;

SELECT c.COLUMN_NAME,
       c.DATA_TYPE,
       c.COLUMN_TYPE,
       c.NUMERIC_PRECISION,
       c.NUMERIC_SCALE,
       c.DATETIME_PRECISION,
       c.COLLATION_NAME
FROM information_schema.COLUMNS c
WHERE c.TABLE_SCHEMA = 'sq_hard_mysql'
  AND c.TABLE_NAME = 'sq_hard_w11_type'
  AND c.COLUMN_NAME IN ('row_id', 'tiny_flag', 'medium_num', 'big_num',
                        'wide_dec', 'bit_mask', 'grade', 'flags',
                        'precise_time', 'made_at', 'tiny_note', 'padded')
ORDER BY c.ORDINAL_POSITION;

SHOW COLUMNS FROM sq_hard_w11_type WHERE Field LIKE 'b%';

SET @sq_hard_w11_type_sum = (SELECT SUM(row_id) FROM sq_hard_w11_type);

-- W11-B: an sql_mode island that changes MEANING, not spelling.
-- HIGH_NOT_PRECEDENCE makes `NOT a BETWEEN b AND c` parse as `(NOT a) BETWEEN
-- b AND c`, NO_UNSIGNED_SUBTRACTION lets an unsigned difference go negative,
-- and TIME_TRUNCATE_FRACTIONAL truncates a fractional second that would
-- otherwise round. The same three expressions run on both sides of the island.
SELECT NOT 1 BETWEEN -5 AND 5           AS not_precedence,
       CAST(3 AS UNSIGNED) - 2          AS unsigned_diff,
       CAST('12:34:56.789' AS TIME(2))  AS fractional_time;

SET SESSION sql_mode = 'STRICT_ALL_TABLES,NO_ZERO_DATE,NO_ZERO_IN_DATE,ERROR_FOR_DIVISION_BY_ZERO,HIGH_NOT_PRECEDENCE,NO_UNSIGNED_SUBTRACTION,TIME_TRUNCATE_FRACTIONAL';

SELECT NOT 1 BETWEEN -5 AND 5           AS not_precedence,
       CAST(3 AS UNSIGNED) - 2          AS unsigned_diff,
       CAST(1 AS UNSIGNED) - 2          AS negative_unsigned,
       CAST('12:34:56.789' AS TIME(2))  AS fractional_time;

SET @sq_hard_w11_island = CONCAT_WS('/',
                                    NOT 1 BETWEEN -5 AND 5,
                                    CAST(1 AS UNSIGNED) - 2,
                                    CAST('12:34:56.789' AS TIME(2)));

SET SESSION sql_mode = 'STRICT_ALL_TABLES,NO_ZERO_DATE,NO_ZERO_IN_DATE,ERROR_FOR_DIVISION_BY_ZERO';

SELECT @sq_hard_w11_island                        AS island_shape,
       CONCAT_WS('/',
                 NOT 1 BETWEEN -5 AND 5,
                 CAST(3 AS UNSIGNED) - 2,
                 CAST('12:34:56.789' AS TIME(2))) AS mainland_shape;

-- W11-C: optimizer-hint comments. `/*+ ... */` is a comment to the lexer and a
-- directive to the planner. A hint block names a query block with QB_NAME,
-- reaches into it from the outer query behind @, orders the join, blocks a
-- derived-table merge, pushes a condition down, and sets a session variable for
-- the duration of one statement.
SELECT /*+ QB_NAME(w11_outer)
           MAX_EXECUTION_TIME(5000)
           NO_MERGE(dt)
           JOIN_ORDER(dt, t)
           DERIVED_CONDITION_PUSHDOWN(dt) */
       t.row_id,
       dt.total
FROM sq_hard_w11_type t
JOIN (SELECT COUNT(*) AS total FROM sq_hard_w11_type) dt
WHERE t.row_id = 7;

SELECT /*+ SEMIJOIN(@w11_sub MATERIALIZATION) */ /* plain comment after a hint */
       t.row_id
FROM sq_hard_w11_type t
WHERE t.row_id IN (SELECT /*+ QB_NAME(w11_sub) */ s.row_id
                   FROM sq_hard_w11_type s
                   WHERE s.grade = 'gamma');

SELECT /*+ SET_VAR(sort_buffer_size = 262144) GROUP_INDEX(t PRIMARY) */
       t.grade,
       COUNT(*) AS graded
FROM sq_hard_w11_type t
GROUP BY t.grade
ORDER BY t.grade;

-- W11-D: the sys schema ships its own stored functions, so a schema qualifier
-- precedes what otherwise looks like a builtin call.
SELECT sys.version_major()                                 AS sys_major,
       sys.version_minor() >= 0                            AS sys_minor_ok,
       sys.list_add('read,write', 'admin')                 AS list_added,
       sys.list_drop('read,write,admin', 'write')          AS list_dropped,
       sys.extract_schema_from_file_name('/data/db1/t.ibd') AS schema_from_file,
       sys.extract_table_from_file_name('/data/db1/t.ibd')  AS table_from_file,
       sys.quote_identifier('a b')                         AS quoted_ident;

SET @sq_hard_w11_sys = sys.list_add('read,write', 'admin');

-- W11-E: dual-password account rotation. The account is created with an
-- explicit authentication plugin and a password-verification policy, rotated
-- while RETAINing the current password (both passwords are live at once), then
-- the old one is DISCARDed.
CREATE USER sq_hard_w11_u@'%'
  IDENTIFIED WITH caching_sha2_password BY 'w11-secret-one'
  PASSWORD REQUIRE CURRENT OPTIONAL
  FAILED_LOGIN_ATTEMPTS 3 PASSWORD_LOCK_TIME 1;

ALTER USER sq_hard_w11_u@'%'
  IDENTIFIED BY 'w11-secret-two' RETAIN CURRENT PASSWORD;

GRANT SELECT (row_id, grade) ON sq_hard_mysql.sq_hard_w11_type
  TO sq_hard_w11_u@'%';

SELECT u.User,
       LENGTH(u.user_attributes) > 0 AS has_attributes,
       JSON_EXTRACT(u.user_attributes, '$.Password_locking.failed_login_attempts')
         AS failed_login_attempts
FROM mysql.user u
WHERE u.User = 'sq_hard_w11_u';

SELECT p.COLUMN_NAME
FROM information_schema.COLUMN_PRIVILEGES p
WHERE p.GRANTEE = '''sq_hard_w11_u''@''%'''
ORDER BY p.COLUMN_NAME;

SET @sq_hard_w11_grants = (SELECT COUNT(*)
                           FROM information_schema.COLUMN_PRIVILEGES
                           WHERE GRANTEE = '''sq_hard_w11_u''@''%''');

ALTER USER sq_hard_w11_u@'%' DISCARD OLD PASSWORD;
DROP USER sq_hard_w11_u@'%';

-- W11-F: the temporal and string builtin zoo. Interval units are bare keywords
-- inside a function argument list, GET_FORMAT returns a format model consumed
-- by DATE_FORMAT, and the set-shaped string functions each take a different
-- argument order.
SELECT CONVERT_TZ(TIMESTAMP '2024-05-06 07:08:09', '+00:00', '+09:00')
                                                    AS tz_shifted,
       MAKETIME(12, 34, 56)                         AS made_time,
       SEC_TO_TIME(3725)                            AS sec_time,
       TIMESTAMPADD(QUARTER, 2, DATE '2024-01-31')  AS quarter_added,
       TIMESTAMPDIFF(MONTH, DATE '2024-01-31', DATE '2024-06-30')
                                                    AS months_between,
       EXTRACT(YEAR_MONTH FROM DATE '2024-05-06')   AS ym_value,
       DATE_FORMAT(DATE '2024-05-06', GET_FORMAT(DATE, 'EUR'))
                                                    AS eur_date,
       LAST_DAY(DATE '2024-02-01')                  AS leap_last_day,
       PERIOD_ADD(202401, 5)                        AS period_added,
       PERIOD_DIFF(202406, 202401)                  AS period_diffed;

SELECT ELT(2, 'alpha', 'beta', 'gamma')             AS elt_pick,
       FIELD('beta', 'alpha', 'beta', 'gamma')      AS field_pos,
       EXPORT_SET(5, 'Y', 'N', ',', 4)              AS exported,
       MAKE_SET(5, 'read', 'write', 'admin')        AS made_set,
       INSERT('abcdef', 2, 3, 'XY')                 AS spliced,
       SUBSTRING_INDEX('a.b.c', '.', -2)            AS tail_parts,
       BIT_COUNT(255)                               AS bits_set,
       INTERVAL(7, 1, 5, 10, 15)                    AS interval_slot,
       LPAD(SPACE(2), 5, '.')                       AS padded_space;

SET @sq_hard_w11_funcs = (SELECT CONCAT_WS('/',
                                           ELT(2, 'alpha', 'beta', 'gamma'),
                                           FIELD('beta', 'alpha', 'beta',
                                                 'gamma'),
                                           MAKE_SET(5, 'read', 'write',
                                                    'admin'),
                                           INTERVAL(7, 1, 5, 10, 15)));

-- W11-G: lexer round 6. A string literal carries a block-comment terminator, a
-- backtick identifier is all digits, backslash escapes appear inside and
-- outside backticks, a binary introducer prefixes a hex literal, and two
-- version-gated executable comments sit side by side -- one below this server's
-- version (it runs) and one above it (it does not).
SELECT '*/ not the end of a comment'    AS star_slash,
       1                                AS `123`,
       2                                AS `back\slash%`,
       'tab\there and quote\'inside'    AS escaped_text,
       _binary 0x4142                   AS binary_intro,
       3 /*!99999 + 100 */              AS future_version,
       4 /*!80000 + 100 */              AS applied_version
FROM DUAL;

SET @sq_hard_w11_lexer = (SELECT CHAR_LENGTH('*/ not the end of a comment')
                                 + (3 /*!99999 + 100 */)
                                 + (4 /*!80000 + 100 */));

INSERT INTO sq_hard_w11_note (note_key, note_text, note_value)
VALUES ('types', @sq_hard_w11_island, @sq_hard_w11_type_sum),
       ('functions', @sq_hard_w11_funcs, @sq_hard_w11_grants),
       ('lexer6', 'star-slash literal and version-gated comments',
        @sq_hard_w11_lexer);

-- Wave-11 self-verification.
CALL sq_hard_assert(
  @sq_hard_w11_type_sum = 15,
  CONCAT('type zoo row sum: ', COALESCE(@sq_hard_w11_type_sum, 'NULL')));
CALL sq_hard_assert(
  @sq_hard_w11_island = '1/-1/12:34:56.78',
  CONCAT('sql_mode island shape: ', COALESCE(@sq_hard_w11_island, 'NULL')));
CALL sq_hard_assert(
  CONCAT_WS('/', NOT 1 BETWEEN -5 AND 5, CAST(3 AS UNSIGNED) - 2,
            CAST('12:34:56.789' AS TIME(2))) = '0/1/12:34:56.79',
  CONCAT('mainland shape: ',
         CONCAT_WS('/', NOT 1 BETWEEN -5 AND 5, CAST(3 AS UNSIGNED) - 2,
                   CAST('12:34:56.789' AS TIME(2)))));
CALL sq_hard_assert(
  @sq_hard_w11_sys = 'read,write,admin',
  CONCAT('sys list_add: ', COALESCE(@sq_hard_w11_sys, 'NULL')));
CALL sq_hard_assert(
  @sq_hard_w11_grants = 2,
  CONCAT('column grants: ', COALESCE(@sq_hard_w11_grants, 'NULL')));
CALL sq_hard_assert(
  @sq_hard_w11_funcs = 'beta/2/read,admin/2',
  CONCAT('function zoo: ', COALESCE(@sq_hard_w11_funcs, 'NULL')));
CALL sq_hard_assert(
  @sq_hard_w11_lexer = 134,
  CONCAT('lexer round 6: ', COALESCE(@sq_hard_w11_lexer, 'NULL')));
CALL sq_hard_assert(
  (SELECT COUNT(*) FROM sq_hard_w11_note) = 3,
  CONCAT('wave11 note rows: ', (SELECT COUNT(*) FROM sq_hard_w11_note)));
CALL sq_hard_assert(
  (SELECT bit_mask + 0 FROM sq_hard_w11_type WHERE row_id = 7) = 170,
  CONCAT('bit mask: ',
         (SELECT bit_mask + 0 FROM sq_hard_w11_type WHERE row_id = 7)));

-- ULTRA WAVE 12 -- the name-collision surface. Builtin functions whose names
-- are statement verbs (INSERT / REPLACE / TRUNCATE / REPEAT / IF / LEFT /
-- RIGHT), builtins whose arguments are separated by keywords (POSITION ... IN,
-- SUBSTRING ... FROM ... FOR, TRIM BOTH ... FROM, EXTRACT ... FROM), a table
-- whose name is two keywords with a space in it and whose columns are spelled
-- `--`, `/*`, `;`, `?`, `@var`, `#hash` and `'quoted'`, the set-operator
-- precedence tower, dynamic SQL whose payload looks like the surrounding
-- script, and lexer round 7.
CREATE TABLE sq_hard_w12_note (
  note_key   VARCHAR(30) NOT NULL PRIMARY KEY,
  note_text  VARCHAR(400),
  note_value BIGINT
) ENGINE = InnoDB;

-- W12-A: every builtin below has the name of a statement verb, so each item in
-- this select list is one token away from being read as a new statement.
SELECT INSERT('abcdef', 2, 3, 'XY')                AS insert_fn,
       REPLACE('a-b-c', '-', '+')                  AS replace_fn,
       LEFT('abcdef', 2)                           AS left_fn,
       RIGHT('abcdef', 2)                          AS right_fn,
       TRUNCATE(3.14159, 2)                        AS truncate_fn,
       REPEAT('ab', 3)                             AS repeat_fn,
       IF(1 = 1, 'yes', 'no')                      AS if_fn,
       CHAR(65, 66 USING utf8mb4)                  AS char_fn,
       CONVERT('7', SIGNED)                        AS convert_fn,
       CONVERT('abc' USING latin1)                 AS convert_using,
       DATABASE()                                  AS database_fn,
       SCHEMA()                                    AS schema_fn,
       POSITION('c' IN 'abcdef')                   AS position_in,
       SUBSTRING('abcdef' FROM 2 FOR 3)            AS substring_from,
       TRIM(BOTH 'x' FROM 'xaxx')                  AS trim_both,
       TRIM(LEADING FROM '  a')                    AS trim_leading,
       EXTRACT(DAY FROM DATE '2024-03-04')         AS extract_day,
       INTERVAL(7, 1, 5, 9)                        AS interval_fn,
       CAST(1 AS CHAR)                             AS cast_char,
       CAST('bin' AS BINARY)                       AS cast_binary
FROM DUAL;

SET @sq_hard_w12_verbs = (
  SELECT CONCAT_WS('/', INSERT('abcdef', 2, 3, 'XY'),
                   TRUNCATE(3.14159, 2),
                   REPEAT('ab', 3),
                   IF(1 = 1, 'yes', 'no'),
                   POSITION('c' IN 'abcdef'),
                   SUBSTRING('abcdef' FROM 2 FOR 3),
                   TRIM(BOTH 'x' FROM 'xaxx'),
                   INTERVAL(7, 1, 5, 9))
  FROM DUAL);

-- A stored procedure named after a statement verb and a function named after a
-- reserved word; both can only ever be called through backticks.
DELIMITER $$
CREATE PROCEDURE `insert` (OUT total INT)
BEGIN
  SET total = 41 + 1;
END$$

CREATE FUNCTION `select` (n INT) RETURNS INT
  DETERMINISTIC NO SQL
BEGIN
  RETURN n + 1;
END$$
DELIMITER ;

CALL `insert`(@sq_hard_w12_proc);

SET @sq_hard_w12_native = `select`(1);

SELECT @sq_hard_w12_native AS keyword_named_function,
       @sq_hard_w12_proc   AS verb_named_procedure;

-- W12-B: the punctuation identifier battery. The table name is two keywords
-- with a space between them; the columns are comment introducers, a statement
-- terminator, a placeholder, a user-variable spelling and a quoted literal.
CREATE TABLE `left join` (
  `select`   INT NOT NULL,
  `from`     INT,
  `where`    INT,
  `group`    INT,
  `order`    INT,
  `having`   INT,
  `join`     INT,
  `on`       INT,
  `and`      INT,
  `null`     INT,
  `default`  INT DEFAULT 9,
  `insert`   INT,
  `update`   INT,
  `delete`   INT,
  `--`       INT,
  `/*`       INT,
  `;`        INT,
  `?`        INT,
  `@var`     INT,
  `#hash`    INT,
  `'quoted'` INT,
  `\`       INT,
  PRIMARY KEY (`select`)
) ENGINE = InnoDB;

INSERT INTO `left join` (`select`, `from`, `where`, `group`, `order`, `having`,
                         `join`, `on`, `and`, `null`, `insert`, `update`,
                         `delete`, `--`, `/*`, `;`, `?`, `@var`, `#hash`,
                         `'quoted'`, `\`)
VALUES (1, 2, 2, 3, 4, 5, 6, 6, 7, 8, 10, 11, 12, 13, 14, 15, 16, 17, 18, 19,
        20),
       (2, 3, 2, 3, 5, 6, 6, 6, 7, 8, 10, 11, 12, 13, 14, 15, 16, 17, 18, 19,
        20);

SELECT a.`select`,
       a.`from`,
       b.`join`,
       a.`--` + a.`/*`  AS comment_named,
       a.`;`            AS semicolon_named,
       a.`?`            AS placeholder_named,
       a.`@var`         AS variable_named,
       a.`#hash`        AS hash_named,
       a.`'quoted'`     AS quote_named,
       a.`\`            AS backslash_named,
       SUM(a.`having`)  AS having_total
FROM `left join` a
LEFT JOIN `left join` b ON a.`join` = b.`on` AND a.`and` = b.`and`
WHERE a.`where` = 2 AND a.`null` IS NOT NULL
GROUP BY a.`select`, a.`from`, b.`join`, a.`--`, a.`/*`, a.`;`, a.`?`,
         a.`@var`, a.`#hash`, a.`'quoted'`, a.`\`
HAVING SUM(a.`having`) > 0
ORDER BY a.`order` DESC;

-- The prepared statement's placeholder sits next to a column literally named ?.
SET @sq_hard_w12_ps =
  'SELECT `?`, `;` FROM `left join` WHERE `select` = ? ORDER BY `order`';
PREPARE sq_hard_w12_stmt FROM @sq_hard_w12_ps;
SET @sq_hard_w12_which = 1;
EXECUTE sq_hard_w12_stmt USING @sq_hard_w12_which;
DEALLOCATE PREPARE sq_hard_w12_stmt;

-- A derived table aliased UNION, and a row-aliased upsert whose SET list mixes
-- the alias with DEFAULT().
SELECT `union`.`select` AS `;`
FROM (SELECT `select` FROM `left join`) AS `union`
ORDER BY `union`.`select`;

INSERT INTO `left join` (`select`, `from`, `where`, `null`, `having`, `order`)
VALUES (1, 99, 2, 8, 5, 4) AS incoming
ON DUPLICATE KEY UPDATE `from` = incoming.`from`,
                        `default` = DEFAULT(`default`);

SET @sq_hard_w12_quoted = (
  SELECT CONCAT_WS('/', a.`--` + a.`/*`, a.`;`, a.`?`, a.`@var`, a.`#hash`,
                   a.`'quoted'`, a.`\`, a.`from`, a.`default`)
  FROM `left join` a
  WHERE a.`select` = 1);

-- W12-C: the set-operator precedence tower. INTERSECT binds tighter than UNION
-- and only consumes the branch next to it, so the first tower keeps 1, 2 and 9
-- while the intersected pair collapses to nothing. The parenthesised EXCEPT and
-- the EXCEPT ALL multiset difference below prove the difference.
SELECT GROUP_CONCAT(n ORDER BY n SEPARATOR '/') AS precedence_shape
FROM (
  SELECT 1 AS n UNION ALL SELECT 2 UNION ALL SELECT 3
  INTERSECT SELECT 2
  UNION ALL SELECT 9
) AS tower;

SELECT GROUP_CONCAT(n ORDER BY n SEPARATOR '/') AS parenthesised_shape
FROM (
  (SELECT 1 AS n UNION ALL SELECT 2 UNION ALL SELECT 3)
  EXCEPT
  (SELECT 2)
) AS parenthesised;

SELECT GROUP_CONCAT(n ORDER BY n SEPARATOR '/') AS except_all_shape
FROM (
  SELECT 1 AS n UNION ALL SELECT 1 UNION ALL SELECT 5
  EXCEPT ALL
  SELECT 1
) AS except_all;

(SELECT `select` AS branch FROM `left join` ORDER BY `select` LIMIT 1)
UNION ALL
(SELECT `select` FROM `left join` ORDER BY `select` DESC LIMIT 1)
ORDER BY branch;

SET @sq_hard_w12_setops = (
  SELECT CONCAT_WS('#',
    (SELECT GROUP_CONCAT(n ORDER BY n SEPARATOR '/')
     FROM (SELECT 1 AS n UNION ALL SELECT 2 UNION ALL SELECT 3
           INTERSECT SELECT 2
           UNION ALL SELECT 9) AS t1),
    (SELECT GROUP_CONCAT(n ORDER BY n SEPARATOR '/')
     FROM ((SELECT 1 AS n UNION ALL SELECT 2 UNION ALL SELECT 3)
           EXCEPT
           (SELECT 2)) AS t2),
    (SELECT GROUP_CONCAT(n ORDER BY n SEPARATOR '/')
     FROM (SELECT 1 AS n UNION ALL SELECT 1 UNION ALL SELECT 5
           EXCEPT ALL
           SELECT 1) AS t3))
  FROM DUAL);

-- W12-D: dynamic SQL whose payload carries a statement terminator, both comment
-- shapes and a backtick identifier, built by CONCAT inside a routine and handed
-- to PREPARE.
DELIMITER $$
CREATE PROCEDURE sq_hard_w12_dyn (IN floor_in INT, OUT hits INT,
                                  OUT payload TEXT)
BEGIN
  DECLARE stmt_text TEXT;
  SET stmt_text = CONCAT('SELECT COUNT(*) INTO @sq_hard_w12_hits ',
                         'FROM `left join` ',
                         '/* built at runtime; carries a ; and a `backtick` */ ',
                         'WHERE `select` >= ', floor_in,
                         ' AND `;` = 15 # trailing hash comment\n',
                         ' -- trailing dash comment');
  SET @sq_hard_w12_sql = stmt_text;
  PREPARE sq_hard_w12_dynamic FROM @sq_hard_w12_sql;
  EXECUTE sq_hard_w12_dynamic;
  DEALLOCATE PREPARE sq_hard_w12_dynamic;
  SET hits = @sq_hard_w12_hits;
  SET payload = CONCAT('len=', CHAR_LENGTH(stmt_text));
END$$
DELIMITER ;

CALL sq_hard_w12_dyn(1, @sq_hard_w12_dyn_hits, @sq_hard_w12_dyn_payload);

SELECT @sq_hard_w12_dyn_hits    AS dynamic_hits,
       @sq_hard_w12_dyn_payload AS dynamic_payload;

-- W12-E: lexer round 7. Adjacent string literals concatenate, a double-quoted
-- token is a string (ANSI_QUOTES is off), hex and bit literals appear in both
-- spellings, escapes carry NUL and ctrl-Z, a literal spells out DELIMITER and
-- both alternate terminators, and the number zoo mixes leading/trailing dots
-- with two exponent forms.
SELECT 'a' 'b' 'c'                     AS adjacent_literals,
       "double quoted"                 AS double_quoted,
       X'414243'                       AS hex_literal,
       0x444546                        AS hex_prefix,
       b'1000001'                      AS bit_literal,
       0b1000010                       AS bit_prefix,
       'tab\there\0zero\Zctrl'         AS escapes,
       'DELIMITER //  ;;  $$'          AS delimiter_bait,
       .5 + 5. + 1e3 + 0.5e-3          AS number_zoo,
       'ends with END; / and #hash'    AS terminator_bait, -- dash comment with ' and */
       1                               AS trailing_item
FROM DUAL;

SET @sq_hard_w12_lexer = (
  SELECT CHAR_LENGTH(CONCAT('a' 'b' 'c', "double quoted", X'414243'))
         + LENGTH(0x444546) + (0b1000010 - b'1000001')
  FROM DUAL);

INSERT INTO sq_hard_w12_note (note_key, note_text, note_value)
VALUES ('verbs', @sq_hard_w12_verbs, @sq_hard_w12_proc),
       ('quoted', @sq_hard_w12_quoted, @sq_hard_w12_dyn_hits),
       ('setops', @sq_hard_w12_setops, @sq_hard_w12_lexer);

-- Wave-12 self-verification.
CALL sq_hard_assert(
  @sq_hard_w12_verbs = 'aXYef/3.14/ababab/yes/3/bcd/a/2',
  CONCAT('verb builtins: ', COALESCE(@sq_hard_w12_verbs, 'NULL')));
CALL sq_hard_assert(
  @sq_hard_w12_native = 2 AND @sq_hard_w12_proc = 42,
  CONCAT('keyword-named routines: ', COALESCE(@sq_hard_w12_proc, 'NULL')));
CALL sq_hard_assert(
  @sq_hard_w12_quoted = '27/15/16/17/18/19/20/99/9',
  CONCAT('punctuation columns: ', COALESCE(@sq_hard_w12_quoted, 'NULL')));
CALL sq_hard_assert(
  @sq_hard_w12_setops = '1/2/9#1/3#1/5',
  CONCAT('set operator tower: ', COALESCE(@sq_hard_w12_setops, 'NULL')));
CALL sq_hard_assert(
  @sq_hard_w12_dyn_hits = 2 AND @sq_hard_w12_dyn_payload IS NOT NULL,
  CONCAT('dynamic sql hits: ', COALESCE(@sq_hard_w12_dyn_hits, 'NULL')));
CALL sq_hard_assert(
  @sq_hard_w12_lexer = 23,
  CONCAT('lexer round 7: ', COALESCE(@sq_hard_w12_lexer, 'NULL')));
CALL sq_hard_assert(
  (SELECT COUNT(*) FROM sq_hard_w12_note) = 3,
  CONCAT('wave12 note rows: ', (SELECT COUNT(*) FROM sq_hard_w12_note)));

-- ------------------------------------------------------------------------------
-- ULTRA WAVE 13 -- MySQL-only recovery points. Three BEFORE triggers reorder
-- themselves with PRECEDES/FOLLOWS and mutate NEW through nested JSON calls; an
-- AFTER trigger projects OLD/NEW into an audit row; CHECK enforcement flips
-- twice; JSON_TABLE is correlated through CROSS JOIN; EXPLAIN returns JSON; and
-- SHOW/GET DIAGNOSTICS expose a harmless DDL note.
-- ------------------------------------------------------------------------------
CREATE TABLE sq_hard_w13_note (
  note_key   VARCHAR(30) NOT NULL PRIMARY KEY,
  note_text  VARCHAR(400),
  note_value BIGINT
) ENGINE = InnoDB;

CREATE TABLE sq_hard_w13_event (
  event_id   INT NOT NULL PRIMARY KEY,
  event_name VARCHAR(30) NOT NULL,
  amount     INT NOT NULL DEFAULT 0,
  payload    JSON NOT NULL,
  kind_code  VARCHAR(20) GENERATED ALWAYS AS (
    JSON_UNQUOTE(JSON_EXTRACT(payload, '$.kind'))
  ) STORED,
  CONSTRAINT ck_sq_hard_w13_amount
    CHECK (amount BETWEEN 0 AND 100) NOT ENFORCED
) ENGINE = InnoDB;

CREATE TABLE sq_hard_w13_audit (
  audit_id   BIGINT NOT NULL AUTO_INCREMENT PRIMARY KEY,
  event_id   INT NOT NULL,
  old_amount INT,
  new_amount INT,
  delta_doc  JSON NOT NULL
) ENGINE = InnoDB;

-- ------------------------------------------------------------------------------
-- W13-A: EVENT_NORMALIZE is created first, EVENT_SEED later inserts itself
-- before it with PRECEDES, and EVENT_AMOUNT follows it. The resulting action
-- order is seed -> normalize -> amount even though creation order differs.
-- ------------------------------------------------------------------------------
DELIMITER $$
CREATE DEFINER = CURRENT_USER TRIGGER IF NOT EXISTS sq_hard_w13_normalize
BEFORE INSERT ON sq_hard_w13_event
FOR EACH ROW
BEGIN
  SET NEW.event_name = LOWER(TRIM(NEW.event_name));
  SET NEW.payload = JSON_ARRAY_APPEND(
    JSON_SET(NEW.payload, '$.normalized', TRUE),
    '$.triggerOrder',
    'normalize'
  );
END$$

CREATE TRIGGER sq_hard_w13_seed
BEFORE INSERT ON sq_hard_w13_event
FOR EACH ROW PRECEDES sq_hard_w13_normalize
BEGIN
  SET NEW.payload = JSON_MERGE_PATCH(
    JSON_OBJECT('triggerOrder', JSON_ARRAY('seed')),
    NEW.payload
  );
END$$

CREATE TRIGGER sq_hard_w13_amount
BEFORE INSERT ON sq_hard_w13_event
FOR EACH ROW FOLLOWS sq_hard_w13_normalize
BEGIN
  SET NEW.amount = COALESCE(
    JSON_VALUE(
      NEW.payload,
      '$.amount' RETURNING SIGNED NULL ON EMPTY NULL ON ERROR
    ),
    NEW.amount
  );
  SET NEW.payload = JSON_ARRAY_APPEND(
    NEW.payload,
    '$.triggerOrder',
    'amount'
  );
END$$

CREATE TRIGGER sq_hard_w13_audit_update
AFTER UPDATE ON sq_hard_w13_event
FOR EACH ROW
BEGIN
  INSERT INTO sq_hard_w13_audit (
    event_id, old_amount, new_amount, delta_doc
  ) VALUES (
    NEW.event_id,
    OLD.amount,
    NEW.amount,
    JSON_OBJECT(
      'oldKind', OLD.kind_code,
      'newKind', NEW.kind_code,
      'payloadChanged', NOT (OLD.payload <=> NEW.payload)
    )
  );
END$$
DELIMITER ;

INSERT INTO sq_hard_w13_event (event_id, event_name, amount, payload)
VALUES
  (1, ' Alpha ', 0,
   '{"kind":"build","amount":12,"tags":["sql","json"]}'),
  (2, 'BETA', 7,
   '{"kind":"deploy","tags":["ops"]}');

UPDATE sq_hard_w13_event
SET amount = 20,
    payload = JSON_SET(payload, '$.amount', 20, '$.updated', TRUE)
WHERE event_id = 1;

SHOW CREATE TRIGGER sq_hard_w13_amount;

SET @sq_hard_w13_trigger_order = (
  SELECT GROUP_CONCAT(
           CONCAT(TRIGGER_NAME, ':', ACTION_ORDER)
           ORDER BY ACTION_ORDER SEPARATOR '/'
         )
  FROM information_schema.triggers
  WHERE trigger_schema = 'sq_hard_mysql'
    AND event_object_table = 'sq_hard_w13_event'
    AND action_timing = 'BEFORE'
    AND event_manipulation = 'INSERT');

INSERT INTO sq_hard_w13_note (note_key, note_text, note_value)
SELECT 'trigger-order',
       @sq_hard_w13_trigger_order,
       SUM(action_order)
FROM information_schema.triggers
WHERE trigger_schema = 'sq_hard_mysql'
  AND event_object_table = 'sq_hard_w13_event'
  AND action_timing = 'BEFORE'
  AND event_manipulation = 'INSERT';

-- ------------------------------------------------------------------------------
-- W13-B: the initially advisory CHECK becomes enforced after the rows are
-- normalized, then advisory again. RENAME COLUMN runs in both directions so
-- completion must invalidate and restore the same column symbol.
-- ------------------------------------------------------------------------------
ALTER TABLE sq_hard_w13_event
  ALTER CHECK ck_sq_hard_w13_amount ENFORCED;

ALTER TABLE sq_hard_w13_event
  ALTER CHECK ck_sq_hard_w13_amount NOT ENFORCED;

ALTER TABLE sq_hard_w13_event
  RENAME COLUMN event_name TO event_label;

ALTER TABLE sq_hard_w13_event
  RENAME COLUMN event_label TO event_name;

INSERT INTO sq_hard_w13_note (note_key, note_text, note_value)
VALUES (
  'event-state',
  CONCAT(
    'rows=', (SELECT COUNT(*) FROM sq_hard_w13_event),
    ',amount=', (SELECT SUM(amount) FROM sq_hard_w13_event),
    ',audit=', (SELECT COUNT(*) FROM sq_hard_w13_audit)
  ),
  (SELECT SUM(amount) FROM sq_hard_w13_event)
);

-- ------------------------------------------------------------------------------
-- W13-C: JSON_TABLE is a correlated table function written as CROSS JOIN.
-- Windows, JSON path predicates, type inspection and JSON_QUOTE all consume
-- columns whose scope is minted inside its COLUMNS list.
-- ------------------------------------------------------------------------------
SELECT e.event_id,
       e.event_name,
       e.amount,
       e.kind_code,
       jt.tag_no,
       jt.tag_name,
       ROW_NUMBER() OVER (
         PARTITION BY e.event_id
         ORDER BY jt.tag_no
       ) AS tag_rank,
       JSON_CONTAINS_PATH(
         e.payload, 'all', '$.kind', '$.normalized', '$.triggerOrder'
       ) AS paths_ok,
       JSON_TYPE(e.payload -> '$.tags') AS tags_type,
       JSON_QUOTE(jt.tag_name)          AS quoted_tag
FROM sq_hard_w13_event e
CROSS JOIN JSON_TABLE(
  e.payload,
  '$.tags[*]' COLUMNS (
    tag_no   FOR ORDINALITY,
    tag_name VARCHAR(20) PATH '$' ERROR ON ERROR
  )
) jt
ORDER BY e.event_id, jt.tag_no;

SET @sq_hard_w13_tag_rows = (
  SELECT COUNT(*)
  FROM sq_hard_w13_event e
  CROSS JOIN JSON_TABLE(
    e.payload,
    '$.tags[*]' COLUMNS (
      tag_no   FOR ORDINALITY,
      tag_name VARCHAR(20) PATH '$'
    )
  ) jt);

EXPLAIN FORMAT=JSON
SELECT e.event_id, jt.tag_name
FROM sq_hard_w13_event e
CROSS JOIN JSON_TABLE(
  e.payload,
  '$.tags[*]' COLUMNS (
    tag_name VARCHAR(20) PATH '$'
  )
) jt
WHERE e.amount >= 7;

OPTIMIZE TABLE sq_hard_w13_event;

-- ------------------------------------------------------------------------------
-- W13-D: IF EXISTS emits a note without failing the script. The routine keeps
-- GET DIAGNOSTICS in the same server call as the warning-producing DDL, so both
-- raw clients and clients that inspect every result preserve its cardinality.
-- SHOW COUNT/WARNINGS/ERRORS then exercise the result-producing command forms.
-- ------------------------------------------------------------------------------
DELIMITER $$
CREATE PROCEDURE sq_hard_w13_capture_warning(OUT warning_count INT)
BEGIN
  DROP TABLE IF EXISTS sq_hard_w13_never_there;
  GET DIAGNOSTICS warning_count = NUMBER;
END$$
DELIMITER ;

CALL sq_hard_w13_capture_warning(@sq_hard_w13_warning_count);
SHOW COUNT(*) WARNINGS;
SHOW WARNINGS LIMIT 0, 5;
SHOW ERRORS;

INSERT INTO sq_hard_w13_note (note_key, note_text, note_value)
VALUES (
  'diagnostics',
  'drop-if-exists warning',
  @sq_hard_w13_warning_count
);

-- ------------------------------------------------------------------------------
-- Wave-13 self-verification.
-- ------------------------------------------------------------------------------
CALL sq_hard_assert(
  @sq_hard_w13_trigger_order =
    'sq_hard_w13_seed:1/sq_hard_w13_normalize:2/sq_hard_w13_amount:3',
  CONCAT('trigger order: ',
         COALESCE(@sq_hard_w13_trigger_order, 'NULL')));
CALL sq_hard_assert(
  (SELECT COUNT(*)
   FROM sq_hard_w13_event
   WHERE JSON_LENGTH(payload -> '$.triggerOrder') = 3
     AND payload ->> '$.triggerOrder[0]' = 'seed'
     AND payload ->> '$.triggerOrder[1]' = 'normalize'
     AND payload ->> '$.triggerOrder[2]' = 'amount') = 2,
  'trigger payload order');
CALL sq_hard_assert(
  (SELECT COUNT(*) FROM sq_hard_w13_event) = 2
  AND (SELECT SUM(amount) FROM sq_hard_w13_event) = 27
  AND (SELECT COUNT(*) FROM sq_hard_w13_audit) = 1,
  'trigger row state');
CALL sq_hard_assert(
  (SELECT old_amount = 12 AND new_amount = 20
          AND delta_doc ->> '$.payloadChanged' = 'true'
   FROM sq_hard_w13_audit
   WHERE event_id = 1),
  'old/new audit row');
CALL sq_hard_assert(
  (SELECT ENFORCED
   FROM information_schema.table_constraints
   WHERE constraint_schema = 'sq_hard_mysql'
     AND table_name = 'sq_hard_w13_event'
     AND constraint_name = 'ck_sq_hard_w13_amount') = 'NO',
  'check enforcement toggle');
CALL sq_hard_assert(
  @sq_hard_w13_tag_rows = 3,
  CONCAT('json table rows: ',
         COALESCE(@sq_hard_w13_tag_rows, 'NULL')));
CALL sq_hard_assert(
  @sq_hard_w13_warning_count = 1,
  CONCAT('diagnostic warnings: ',
         COALESCE(@sq_hard_w13_warning_count, 'NULL')));
CALL sq_hard_assert(
  (SELECT COUNT(*) FROM sq_hard_w13_note) = 3,
  CONCAT('wave13 note rows: ',
         (SELECT COUNT(*) FROM sq_hard_w13_note)));

-- ------------------------------------------------------------------------------
-- ULTRA WAVE 14 -- MySQL-only metadata and diagnostics surfaces. Secondary
-- engine attributes are JSON documents embedded in DDL and exposed through
-- three *_EXTENSIONS relations; OPTIMIZER_TRACE turns a CTE/window plan into a
-- queryable JSON document; SIGNAL and GET STACKED DIAGNOSTICS exchange every
-- standard condition-information item; and a recursive member owns LIMIT while
-- a row-alias upsert feeds statement-level diagnostics.
-- ------------------------------------------------------------------------------
CREATE TABLE sq_hard_w14_note (
  note_key   VARCHAR(30) NOT NULL PRIMARY KEY,
  note_text  VARCHAR(600),
  note_value BIGINT
) ENGINE = InnoDB;

-- ------------------------------------------------------------------------------
-- W14-A: SECONDARY_ENGINE_ATTRIBUTE appears at three different grammar levels:
-- after a column type, after an index definition, and among table options. SHOW
-- CREATE serializes each as a versioned executable comment, while the extension
-- metadata views expose the same JSON as ordinary relation columns.
-- ------------------------------------------------------------------------------
CREATE TABLE sq_hard_w14_attribute (
  attribute_id BIGINT NOT NULL
    SECONDARY_ENGINE_ATTRIBUTE =
      '{"role":"identifier","tokens":";--/*#$$"}',
  amount       INT NOT NULL,
  note         VARCHAR(80) NOT NULL,
  payload      JSON NOT NULL,
  PRIMARY KEY (attribute_id),
  KEY ix_sq_hard_w14_amount (amount DESC, attribute_id)
    SECONDARY_ENGINE_ATTRIBUTE =
      '{"path":"$.amount","order":"descending"}'
) ENGINE = InnoDB
  SECONDARY_ENGINE_ATTRIBUTE =
    '{"wave":14,"table":"sq_hard_w14_attribute","secondary":true}';

INSERT INTO sq_hard_w14_attribute (
  attribute_id, amount, note, payload
)
VALUES
  (1, 10, 'alpha', JSON_OBJECT('tags', JSON_ARRAY('sql', 'trace'))),
  (2, 20, 'beta',  JSON_OBJECT('tags', JSON_ARRAY('window'))),
  (3, 30, 'gamma', JSON_OBJECT('tags', JSON_ARRAY('json'))),
  (4, 40, 'delta', JSON_OBJECT('tags', JSON_ARRAY('metadata')));

SHOW CREATE TABLE sq_hard_w14_attribute;

SELECT te.table_catalog,
       te.table_schema,
       te.table_name,
       te.engine_attribute,
       te.secondary_engine_attribute
FROM information_schema.tables_extensions AS te
WHERE te.table_schema = 'sq_hard_mysql'
  AND te.table_name = 'sq_hard_w14_attribute';

SELECT ce.table_catalog,
       ce.table_schema,
       ce.table_name,
       ce.column_name,
       ce.engine_attribute,
       ce.secondary_engine_attribute
FROM information_schema.columns_extensions AS ce
WHERE ce.table_schema = 'sq_hard_mysql'
  AND ce.table_name = 'sq_hard_w14_attribute'
ORDER BY ce.column_name;

SELECT tce.constraint_catalog,
       tce.constraint_schema,
       tce.constraint_name,
       tce.table_name,
       tce.engine_attribute,
       tce.secondary_engine_attribute
FROM information_schema.table_constraints_extensions AS tce
WHERE tce.constraint_schema = 'sq_hard_mysql'
  AND tce.table_name = 'sq_hard_w14_attribute'
ORDER BY tce.constraint_name;

SELECT JSON_VALUE(
         te.secondary_engine_attribute,
         '$.wave' RETURNING UNSIGNED
       ),
       JSON_VALUE(ce.secondary_engine_attribute, '$.role'),
       JSON_VALUE(tce.secondary_engine_attribute, '$.path')
INTO @sq_hard_w14_attribute_wave,
     @sq_hard_w14_column_role,
     @sq_hard_w14_index_path
FROM information_schema.tables_extensions AS te
JOIN information_schema.columns_extensions AS ce
  ON ce.table_schema = te.table_schema
 AND ce.table_name = te.table_name
 AND ce.column_name = 'attribute_id'
JOIN information_schema.table_constraints_extensions AS tce
  ON tce.constraint_schema = te.table_schema
 AND tce.table_name = te.table_name
 AND tce.constraint_name = 'ix_sq_hard_w14_amount'
WHERE te.table_schema = 'sq_hard_mysql'
  AND te.table_name = 'sq_hard_w14_attribute';

INSERT INTO sq_hard_w14_note (note_key, note_text, note_value)
VALUES (
  'extension-metadata',
  CONCAT(
    'wave=', @sq_hard_w14_attribute_wave,
    ',column=', @sq_hard_w14_column_role,
    ',index=', @sq_hard_w14_index_path
  ),
  @sq_hard_w14_attribute_wave
);

-- ------------------------------------------------------------------------------
-- W14-B: the traced statement has a CTE, a range predicate, ROW_NUMBER and an
-- outer filter. A routine captures INFORMATION_SCHEMA.OPTIMIZER_TRACE
-- immediately after that statement, before a GUI/driver session probe can
-- replace the trace. Recursive-descent JSON paths then inspect optimizer-owned
-- keys such as `join_optimization` and `range_analysis`.
-- ------------------------------------------------------------------------------
DROP PROCEDURE IF EXISTS sq_hard_w14_capture_trace;
DELIMITER |!|
CREATE PROCEDURE sq_hard_w14_capture_trace()
BEGIN
  WITH ranked AS (
    SELECT /*+ QB_NAME(sq_hard_w14_ranked)
               SET_VAR(range_optimizer_max_mem_size = 8388608) */
           a.attribute_id,
           a.amount,
           ROW_NUMBER() OVER (
             ORDER BY a.amount, a.attribute_id
           ) AS amount_rank
    FROM sq_hard_w14_attribute AS a
    WHERE a.amount BETWEEN 10 AND 30
  )
  SELECT COUNT(*), SUM(amount)
  INTO @sq_hard_w14_trace_rows,
       @sq_hard_w14_trace_total
  FROM ranked
  WHERE amount_rank <= 2;

  SELECT ot.trace,
         ot.missing_bytes_beyond_max_mem_size,
         ot.insufficient_privileges
  INTO @sq_hard_w14_trace,
       @sq_hard_w14_trace_missing,
       @sq_hard_w14_trace_privileges
  FROM information_schema.optimizer_trace AS ot
  LIMIT 1;
END|!|
DELIMITER ;

SET @sq_hard_w14_saved_trace = @@SESSION.optimizer_trace;
SET @sq_hard_w14_saved_trace_mem = @@SESSION.optimizer_trace_max_mem_size;
SET SESSION optimizer_trace = 'enabled=on,one_line=off';
SET SESSION optimizer_trace_max_mem_size = 1048576;

CALL sq_hard_w14_capture_trace();

SET SESSION optimizer_trace = @sq_hard_w14_saved_trace;
SET SESSION optimizer_trace_max_mem_size = @sq_hard_w14_saved_trace_mem;

SET @sq_hard_w14_trace_valid =
  JSON_VALID(@sq_hard_w14_trace);
SET @sq_hard_w14_trace_steps =
  JSON_LENGTH(@sq_hard_w14_trace, '$.steps');
SET @sq_hard_w14_trace_has_join =
  JSON_CONTAINS_PATH(
    @sq_hard_w14_trace,
    'one',
    '$**.join_optimization'
  );
SET @sq_hard_w14_trace_has_range =
  JSON_CONTAINS_PATH(
    @sq_hard_w14_trace,
    'one',
    '$**.range_analysis'
  );

SELECT @sq_hard_w14_trace_valid AS trace_valid,
       @sq_hard_w14_trace_steps AS trace_steps,
       @sq_hard_w14_trace_has_join AS has_join_optimization,
       @sq_hard_w14_trace_has_range AS has_range_analysis,
       OCTET_LENGTH(@sq_hard_w14_trace) AS trace_bytes,
       @sq_hard_w14_trace_missing AS missing_bytes,
       @sq_hard_w14_trace_privileges AS insufficient_privileges;

INSERT INTO sq_hard_w14_note (note_key, note_text, note_value)
VALUES (
  'optimizer-trace',
  CONCAT(
    'rows=', @sq_hard_w14_trace_rows,
    ',total=', @sq_hard_w14_trace_total,
    ',steps=', @sq_hard_w14_trace_steps,
    ',join=', @sq_hard_w14_trace_has_join,
    ',range=', @sq_hard_w14_trace_has_range,
    ',bytes=', OCTET_LENGTH(@sq_hard_w14_trace)
  ),
  OCTET_LENGTH(@sq_hard_w14_trace)
);

-- ------------------------------------------------------------------------------
-- W14-C: SIGNAL populates all assignable condition-information items. The EXIT
-- handler reads statement information and all thirteen condition items from the
-- stacked diagnostics area after the current area is allowed to change.
-- ------------------------------------------------------------------------------
CREATE TABLE sq_hard_w14_diagnostic (
  diagnostic_id  INT NOT NULL PRIMARY KEY,
  payload        JSON NOT NULL,
  condition_count INT NOT NULL,
  affected_rows  BIGINT NOT NULL
) ENGINE = InnoDB;

DELIMITER |!|
CREATE PROCEDURE sq_hard_w14_capture_diagnostics()
BEGIN
  DECLARE v_class              VARCHAR(64);
  DECLARE v_subclass           VARCHAR(64);
  DECLARE v_state              CHAR(5);
  DECLARE v_message            VARCHAR(128);
  DECLARE v_errno              INT;
  DECLARE v_constraint_catalog VARCHAR(64);
  DECLARE v_constraint_schema  VARCHAR(64);
  DECLARE v_constraint_name    VARCHAR(64);
  DECLARE v_catalog            VARCHAR(64);
  DECLARE v_schema             VARCHAR(64);
  DECLARE v_table              VARCHAR(64);
  DECLARE v_column             VARCHAR(64);
  DECLARE v_cursor             VARCHAR(64);
  DECLARE v_number             INT DEFAULT -1;
  DECLARE v_rows               BIGINT DEFAULT -999;

  DECLARE EXIT HANDLER FOR SQLSTATE '45014'
  BEGIN
    GET STACKED DIAGNOSTICS
      v_number = NUMBER,
      v_rows   = ROW_COUNT;

    SET @sq_hard_w14_current_area_changed = TRUE;

    GET STACKED DIAGNOSTICS CONDITION 1
      v_class              = CLASS_ORIGIN,
      v_subclass           = SUBCLASS_ORIGIN,
      v_state              = RETURNED_SQLSTATE,
      v_message            = MESSAGE_TEXT,
      v_errno              = MYSQL_ERRNO,
      v_constraint_catalog = CONSTRAINT_CATALOG,
      v_constraint_schema  = CONSTRAINT_SCHEMA,
      v_constraint_name    = CONSTRAINT_NAME,
      v_catalog            = CATALOG_NAME,
      v_schema             = SCHEMA_NAME,
      v_table              = TABLE_NAME,
      v_column             = COLUMN_NAME,
      v_cursor             = CURSOR_NAME;

    INSERT INTO sq_hard_w14_diagnostic (
      diagnostic_id, payload, condition_count, affected_rows
    )
    VALUES (
      1,
      JSON_OBJECT(
        'class',             v_class,
        'subclass',          v_subclass,
        'state',             v_state,
        'message',           v_message,
        'errno',             v_errno,
        'constraintCatalog', v_constraint_catalog,
        'constraintSchema',  v_constraint_schema,
        'constraint',        v_constraint_name,
        'catalog',           v_catalog,
        'schema',            v_schema,
        'table',             v_table,
        'column',            v_column,
        'cursor',            v_cursor
      ),
      v_number,
      v_rows
    );
  END;

  SIGNAL SQLSTATE '45014'
    SET CLASS_ORIGIN       = 'SPACE_QUERY',
        SUBCLASS_ORIGIN    = 'WAVE_14',
        MESSAGE_TEXT       = 'signal;--/*diagnostic*/#$$',
        MYSQL_ERRNO        = 1644,
        CONSTRAINT_CATALOG = 'def',
        CONSTRAINT_SCHEMA  = 'sq_hard_mysql',
        CONSTRAINT_NAME    = 'ck_sq_hard_w14_signal',
        CATALOG_NAME       = 'def',
        SCHEMA_NAME        = 'sq_hard_mysql',
        TABLE_NAME         = 'sq_hard_w14_attribute',
        COLUMN_NAME        = 'amount',
        CURSOR_NAME        = 'sq_hard_w14_cursor';
END|!|
DELIMITER ;

CALL sq_hard_w14_capture_diagnostics();

SELECT d.diagnostic_id,
       d.payload ->> '$.class'      AS class_origin,
       d.payload ->> '$.subclass'   AS subclass_origin,
       d.payload ->> '$.state'      AS returned_sqlstate,
       d.payload ->> '$.message'    AS message_text,
       d.payload ->> '$.constraint' AS constraint_name,
       d.payload ->> '$.table'      AS table_name,
       d.payload ->> '$.column'     AS column_name,
       d.payload ->> '$.cursor'     AS cursor_name,
       d.condition_count,
       d.affected_rows
FROM sq_hard_w14_diagnostic AS d;

INSERT INTO sq_hard_w14_note (note_key, note_text, note_value)
SELECT 'full-diagnostics',
       CONCAT(
         d.payload ->> '$.state',
         '/',
         d.payload ->> '$.class',
         '/',
         d.payload ->> '$.subclass',
         '/',
         d.payload ->> '$.constraint',
         '/',
         d.payload ->> '$.table',
         '.',
         d.payload ->> '$.column',
         '/',
         d.payload ->> '$.cursor'
       ),
       JSON_VALUE(d.payload, '$.errno' RETURNING UNSIGNED)
FROM sq_hard_w14_diagnostic AS d
WHERE d.diagnostic_id = 1;

-- ------------------------------------------------------------------------------
-- W14-D: LIMIT closes the recursive SELECT, not the final query, and therefore
-- caps recursion at six rows even though the WHERE predicate permits one
-- hundred. A row alias then belongs to VALUES inside an upsert. A routine keeps
-- that DML and GET CURRENT DIAGNOSTICS adjacent even when a GUI client performs
-- session probes between top-level statements, preserving MySQL's affected-row
-- accounting (one insert + one changed duplicate = three).
-- ------------------------------------------------------------------------------
WITH RECURSIVE bounded (n, path) AS (
  SELECT 1, CAST('1' AS CHAR(100))
  UNION ALL
  SELECT b.n + 1,
         CONCAT(b.path, '>', b.n + 1)
  FROM bounded AS b
  WHERE b.n < 100
  LIMIT 6
)
SELECT GROUP_CONCAT(n ORDER BY n SEPARATOR '/'),
       MAX(path),
       COUNT(*)
INTO @sq_hard_w14_recursive_shape,
     @sq_hard_w14_recursive_path,
     @sq_hard_w14_recursive_rows
FROM bounded;

DROP PROCEDURE IF EXISTS sq_hard_w14_upsert_diagnostics;
DELIMITER |!|
CREATE PROCEDURE sq_hard_w14_upsert_diagnostics()
BEGIN
  INSERT INTO sq_hard_w14_attribute (
    attribute_id, amount, note, payload
  )
  VALUES
    (1, 11, 'alpha-updated', JSON_OBJECT('tags', JSON_ARRAY('updated'))),
    (5, 50, 'epsilon',       JSON_OBJECT('tags', JSON_ARRAY('inserted')))
  AS incoming
  ON DUPLICATE KEY UPDATE
    amount  = incoming.amount,
    note    = incoming.note,
    payload = incoming.payload;

  GET CURRENT DIAGNOSTICS
    @sq_hard_w14_current_conditions = NUMBER,
    @sq_hard_w14_current_rows       = ROW_COUNT;
END|!|
DELIMITER ;

CALL sq_hard_w14_upsert_diagnostics();

INSERT INTO sq_hard_w14_note (note_key, note_text, note_value)
VALUES (
  'recursive-current',
  CONCAT(
    'shape=', @sq_hard_w14_recursive_shape,
    ',path=', @sq_hard_w14_recursive_path,
    ',conditions=', @sq_hard_w14_current_conditions,
    ',rows=', @sq_hard_w14_current_rows
  ),
  @sq_hard_w14_recursive_rows * 1000 + @sq_hard_w14_current_rows
);

-- ------------------------------------------------------------------------------
-- Wave-14 self-verification.
-- ------------------------------------------------------------------------------
CALL sq_hard_assert(
  @sq_hard_w14_attribute_wave = 14
  AND @sq_hard_w14_column_role = 'identifier'
  AND @sq_hard_w14_index_path = '$.amount',
  CONCAT('extension metadata: ',
         COALESCE(@sq_hard_w14_column_role, 'NULL'),
         '/', COALESCE(@sq_hard_w14_index_path, 'NULL')));
CALL sq_hard_assert(
  @sq_hard_w14_trace_rows = 2
  AND @sq_hard_w14_trace_total = 30
  AND @sq_hard_w14_trace_valid = 1
  AND @sq_hard_w14_trace_steps >= 3
  AND @sq_hard_w14_trace_has_join = 1
  AND @sq_hard_w14_trace_has_range = 1
  AND @sq_hard_w14_trace_missing = 0
  AND @sq_hard_w14_trace_privileges = 0,
  CONCAT('optimizer trace: ',
         COALESCE(@sq_hard_w14_trace_steps, -1),
         '/', COALESCE(@sq_hard_w14_trace_missing, -1)));
CALL sq_hard_assert(
  (SELECT condition_count
   FROM sq_hard_w14_diagnostic
   WHERE diagnostic_id = 1) = 1
  AND (SELECT affected_rows
       FROM sq_hard_w14_diagnostic
       WHERE diagnostic_id = 1) = -1,
  'diagnostic statement information');
CALL sq_hard_assert(
  (SELECT payload ->> '$.class'
   FROM sq_hard_w14_diagnostic
   WHERE diagnostic_id = 1) = 'SPACE_QUERY'
  AND (SELECT payload ->> '$.subclass'
       FROM sq_hard_w14_diagnostic
       WHERE diagnostic_id = 1) = 'WAVE_14'
  AND (SELECT payload ->> '$.state'
       FROM sq_hard_w14_diagnostic
       WHERE diagnostic_id = 1) = '45014'
  AND (SELECT JSON_VALUE(payload, '$.errno' RETURNING UNSIGNED)
       FROM sq_hard_w14_diagnostic
       WHERE diagnostic_id = 1) = 1644
  AND (SELECT payload ->> '$.constraint'
       FROM sq_hard_w14_diagnostic
       WHERE diagnostic_id = 1) = 'ck_sq_hard_w14_signal'
  AND (SELECT payload ->> '$.table'
       FROM sq_hard_w14_diagnostic
       WHERE diagnostic_id = 1) = 'sq_hard_w14_attribute'
  AND (SELECT payload ->> '$.column'
       FROM sq_hard_w14_diagnostic
       WHERE diagnostic_id = 1) = 'amount'
  AND (SELECT payload ->> '$.cursor'
       FROM sq_hard_w14_diagnostic
       WHERE diagnostic_id = 1) = 'sq_hard_w14_cursor',
  'full stacked diagnostics');
CALL sq_hard_assert(
  @sq_hard_w14_recursive_shape = '1/2/3/4/5/6'
  AND @sq_hard_w14_recursive_path = '1>2>3>4>5>6'
  AND @sq_hard_w14_recursive_rows = 6
  AND @sq_hard_w14_current_conditions = 0
  AND @sq_hard_w14_current_rows = 3,
  CONCAT('recursive/current diagnostics: ',
         COALESCE(@sq_hard_w14_recursive_shape, 'NULL'),
         '/', COALESCE(@sq_hard_w14_current_rows, -1)));
CALL sq_hard_assert(
  (SELECT COUNT(*) FROM sq_hard_w14_attribute) = 5
  AND (SELECT SUM(amount) FROM sq_hard_w14_attribute) = 151,
  'attribute row state');
CALL sq_hard_assert(
  (SELECT COUNT(*) FROM sq_hard_w14_note) = 4,
  CONCAT('wave14 note rows: ',
         (SELECT COUNT(*) FROM sq_hard_w14_note)));

-- ------------------------------------------------------------------------------
-- ULTRA WAVE 15 -- generated-expression and analytic-JSON scope endgame.
-- A UUID crosses a nondeterministic default, two deterministic generated
-- columns and a binary/text round trip. Separately, JSON_OBJECTAGG/JSON_ARRAYAGG
-- run as ordered window functions inside CTEs attached to INSERT and UPDATE;
-- two correlated JSON_TABLE sources then turn the materialized snapshots back
-- into relational columns.
-- ------------------------------------------------------------------------------
CREATE TABLE sq_hard_w15_note (
  note_key   VARCHAR(30) NOT NULL PRIMARY KEY,
  note_text  VARCHAR(700) NOT NULL,
  note_value BIGINT NOT NULL
) ENGINE = InnoDB;

-- ------------------------------------------------------------------------------
-- W15-A: UUID() returns a version-1 string on MySQL 8.0. The swap flag is used
-- in both directions, so IntelliSense must keep the nested default and generated
-- expressions distinct while resolving dependencies across three table columns.
-- ------------------------------------------------------------------------------
CREATE TABLE sq_hard_w15_token (
  token_id    INT NOT NULL,
  token_bin   BINARY(16) NOT NULL DEFAULT (UUID_TO_BIN(UUID(), 1)),
  token_text  CHAR(36)
    GENERATED ALWAYS AS (BIN_TO_UUID(token_bin, 1)) STORED,
  token_valid TINYINT
    GENERATED ALWAYS AS (IS_UUID(token_text)) VIRTUAL,
  PRIMARY KEY (token_id),
  UNIQUE KEY uq_sq_hard_w15_token_text (token_text)
) ENGINE = InnoDB;

INSERT INTO sq_hard_w15_token (token_id)
VALUES (1), (2), (3);

SELECT COUNT(*),
       COUNT(DISTINCT token_bin),
       SUM(token_valid),
       MIN(SUBSTRING(token_text, 15, 1)),
       MAX(SUBSTRING(token_text, 15, 1)),
       SUM(token_bin = UUID_TO_BIN(token_text, 1))
INTO @sq_hard_w15_token_rows,
     @sq_hard_w15_token_distinct,
     @sq_hard_w15_token_valid,
     @sq_hard_w15_token_version_min,
     @sq_hard_w15_token_version_max,
     @sq_hard_w15_token_roundtrip
FROM sq_hard_w15_token;

SET @sq_hard_w15_token_shape = CONCAT(
  'rows=', @sq_hard_w15_token_rows,
  ',distinct=', @sq_hard_w15_token_distinct,
  ',valid=', @sq_hard_w15_token_valid,
  ',version=', @sq_hard_w15_token_version_min,
  ',roundtrip=', @sq_hard_w15_token_roundtrip
);

INSERT INTO sq_hard_w15_note (note_key, note_text, note_value)
VALUES (
  'uuid-generated-chain',
  @sq_hard_w15_token_shape,
  @sq_hard_w15_token_rows * 1000
    + @sq_hard_w15_token_valid * 100
    + @sq_hard_w15_token_roundtrip * 10
    + @sq_hard_w15_token_version_min
);

-- ------------------------------------------------------------------------------
-- W15-B: duplicate `status` keys make JSON_OBJECTAGG order-sensitive. The
-- explicit ROWS frame ensures the latest row has the complete object/history.
-- The first CTE pipeline is attached after an INSERT target; a second is attached
-- before UPDATE and resolves the same minted aliases through a joined DML scope.
-- ------------------------------------------------------------------------------
CREATE TABLE sq_hard_w15_change (
  entity_id       INT NOT NULL,
  change_no       INT NOT NULL,
  attribute_name  VARCHAR(30) NOT NULL,
  attribute_value VARCHAR(80) NOT NULL,
  change_doc      JSON GENERATED ALWAYS AS (
    JSON_OBJECT(
      'attribute', attribute_name,
      'value', attribute_value,
      'tokens', 'semi;--/*json*/#$$'
    )
  ) STORED,
  PRIMARY KEY (entity_id, change_no),
  KEY ix_sq_hard_w15_attribute (
    (JSON_VALUE(change_doc, '$.attribute' RETURNING CHAR(30))),
    entity_id,
    change_no DESC
  )
) ENGINE = InnoDB;

CREATE TABLE sq_hard_w15_snapshot (
  entity_id    INT NOT NULL PRIMARY KEY,
  state_doc    JSON NOT NULL,
  history_doc  JSON NOT NULL,
  status_text  VARCHAR(30) GENERATED ALWAYS AS (
    JSON_UNQUOTE(JSON_EXTRACT(state_doc, '$.status'))
  ) STORED,
  owner_text   VARCHAR(30) GENERATED ALWAYS AS (
    JSON_UNQUOTE(JSON_EXTRACT(state_doc, '$.owner'))
  ) STORED,
  CONSTRAINT ck_sq_hard_w15_state_object
    CHECK (JSON_TYPE(state_doc) = 'OBJECT'),
  CONSTRAINT ck_sq_hard_w15_history_array
    CHECK (JSON_TYPE(history_doc) = 'ARRAY'),
  KEY ix_sq_hard_w15_state (
    (JSON_VALUE(state_doc, '$.status' RETURNING CHAR(30))),
    entity_id DESC
  )
) ENGINE = InnoDB;

INSERT INTO sq_hard_w15_change (
  entity_id, change_no, attribute_name, attribute_value
)
VALUES
  (1, 1, 'status', 'open'),
  (1, 2, 'owner',  'ada'),
  (1, 3, 'status', 'closed'),
  (2, 1, 'status', 'new'),
  (2, 2, 'owner',  'linus');

INSERT INTO sq_hard_w15_snapshot (
  entity_id, state_doc, history_doc
)
WITH stateful AS (
  SELECT c.entity_id,
         c.change_no,
         JSON_OBJECTAGG(c.attribute_name, c.attribute_value)
           OVER w_state AS state_doc,
         JSON_ARRAYAGG(
           JSON_OBJECT(
             'n', c.change_no,
             'attribute', c.attribute_name,
             'value', c.attribute_value
           )
         ) OVER w_state AS history_doc,
         ROW_NUMBER() OVER w_latest AS latest_no
  FROM sq_hard_w15_change AS c
  WINDOW
    w_state AS (
      PARTITION BY c.entity_id
      ORDER BY c.change_no
      ROWS BETWEEN UNBOUNDED PRECEDING AND CURRENT ROW
    ),
    w_latest AS (
      PARTITION BY c.entity_id
      ORDER BY c.change_no DESC
    )
)
SELECT entity_id, state_doc, history_doc
FROM stateful
WHERE latest_no = 1;

INSERT INTO sq_hard_w15_change (
  entity_id, change_no, attribute_name, attribute_value
)
VALUES (2, 3, 'status', 'active');

WITH stateful AS (
  SELECT c.entity_id,
         JSON_OBJECTAGG(c.attribute_name, c.attribute_value)
           OVER w_state AS state_doc,
         JSON_ARRAYAGG(
           JSON_OBJECT(
             'n', c.change_no,
             'attribute', c.attribute_name,
             'value', c.attribute_value
           )
         ) OVER w_state AS history_doc,
         ROW_NUMBER() OVER w_latest AS latest_no
  FROM sq_hard_w15_change AS c
  WINDOW
    w_state AS (
      PARTITION BY c.entity_id
      ORDER BY c.change_no
      ROWS BETWEEN UNBOUNDED PRECEDING AND CURRENT ROW
    ),
    w_latest AS (
      PARTITION BY c.entity_id
      ORDER BY c.change_no DESC
    )
),
latest AS (
  SELECT entity_id, state_doc, history_doc
  FROM stateful
  WHERE latest_no = 1
)
UPDATE sq_hard_w15_snapshot AS snapshot
JOIN latest
  ON latest.entity_id = snapshot.entity_id
SET snapshot.state_doc = latest.state_doc,
    snapshot.history_doc = latest.history_doc
WHERE snapshot.entity_id = 2;

-- Both JSON_TABLE calls are implicitly lateral to each snapshot row. The first
-- mints state columns from an object; the second mints ordinality and change
-- columns from the window-produced history array.
SELECT GROUP_CONCAT(
         CONCAT(
           materialized.entity_id, '=',
           materialized.status_value, '/',
           materialized.owner_value, ':',
           materialized.history_rows, ':',
           materialized.status_changes
         )
         ORDER BY materialized.entity_id
         SEPARATOR ','
       ),
       SUM(materialized.history_rows)
INTO @sq_hard_w15_state_shape,
     @sq_hard_w15_history_total
FROM (
  SELECT snapshot.entity_id,
         state_row.status_value,
         state_row.owner_value,
         COUNT(*) AS history_rows,
         SUM(history_row.changed_attribute = 'status') AS status_changes
  FROM sq_hard_w15_snapshot AS snapshot
  CROSS JOIN JSON_TABLE(
    snapshot.state_doc,
    '$' COLUMNS (
      status_value VARCHAR(30) PATH '$.status' ERROR ON ERROR,
      owner_value  VARCHAR(30) PATH '$.owner' ERROR ON ERROR,
      missing_flag INT EXISTS PATH '$.missing'
    )
  ) AS state_row
  CROSS JOIN JSON_TABLE(
    snapshot.history_doc,
    '$[*]' COLUMNS (
      history_no       FOR ORDINALITY,
      changed_no       INT PATH '$.n' ERROR ON ERROR,
      changed_attribute VARCHAR(30) PATH '$.attribute' ERROR ON ERROR,
      changed_value     VARCHAR(80) PATH '$.value' ERROR ON ERROR
    )
  ) AS history_row
  GROUP BY snapshot.entity_id,
           state_row.status_value,
           state_row.owner_value
) AS materialized;

SET @sq_hard_w15_state_note = CONCAT(
  @sq_hard_w15_state_shape,
  ',rows=', (SELECT COUNT(*) FROM sq_hard_w15_snapshot),
  ',history=', @sq_hard_w15_history_total
);

INSERT INTO sq_hard_w15_note (note_key, note_text, note_value)
VALUES (
  'analytic-json-state',
  @sq_hard_w15_state_note,
  (SELECT COUNT(*) FROM sq_hard_w15_snapshot) * 100
    + @sq_hard_w15_history_total
);

-- ------------------------------------------------------------------------------
-- Wave-15 self-verification.
-- ------------------------------------------------------------------------------
CALL sq_hard_assert(
  @sq_hard_w15_token_rows = 3
  AND @sq_hard_w15_token_distinct = 3
  AND @sq_hard_w15_token_valid = 3
  AND @sq_hard_w15_token_version_min = '1'
  AND @sq_hard_w15_token_version_max = '1'
  AND @sq_hard_w15_token_roundtrip = 3,
  CONCAT('uuid/generated/roundtrip: ',
         COALESCE(@sq_hard_w15_token_shape, 'NULL')));
CALL sq_hard_assert(
  @sq_hard_w15_state_shape =
    '1=closed/ada:3:2,2=active/linus:3:2'
  AND @sq_hard_w15_history_total = 6
  AND (SELECT status_text
       FROM sq_hard_w15_snapshot WHERE entity_id = 1) = 'closed'
  AND (SELECT owner_text
       FROM sq_hard_w15_snapshot WHERE entity_id = 2) = 'linus',
  CONCAT('analytic JSON state: ',
         COALESCE(@sq_hard_w15_state_note, 'NULL')));
CALL sq_hard_assert(
  (SELECT COUNT(*) FROM sq_hard_w15_note) = 2,
  CONCAT('wave15 note rows: ',
         (SELECT COUNT(*) FROM sq_hard_w15_note)));

-- ------------------------------------------------------------------------------
-- ULTRA WAVE 16 -- JSON sibling-nesting and table-expression scope singularity.
-- A CTAS owns a WITH clause whose JSON_TABLE has two sibling NESTED paths;
-- those null-complemented rows feed an INSERT with a CTE and chained LATERAL
-- owners; finally TABLE, VALUES ROW and EXCEPT branches become a second INSERT
-- source. Generated JSON, functional indexes and exact assertions close every
-- inferred alias/type boundary without relying on client-side interpretation.
-- ------------------------------------------------------------------------------
CREATE TABLE sq_hard_w16_note (
  note_key   VARCHAR(30) NOT NULL PRIMARY KEY,
  note_text  VARCHAR(700) NOT NULL,
  note_value BIGINT NOT NULL
) ENGINE = InnoDB;

CREATE TABLE sq_hard_w16_document (
  document_id INT NOT NULL PRIMARY KEY,
  body        JSON NOT NULL,
  CONSTRAINT ck_sq_hard_w16_document_object
    CHECK (JSON_TYPE(body) = 'OBJECT')
) ENGINE = InnoDB;

INSERT INTO sq_hard_w16_document (document_id, body)
VALUES
  (
    1,
    JSON_OBJECT(
      'bucket', 'alpha',
      'items', JSON_ARRAY(
        JSON_OBJECT('id', 10, 'name', 'parse',   'qty', 2),
        JSON_OBJECT('id', 11, 'name', 'execute', 'qty', 5)
      ),
      'tags', JSON_ARRAY('sql', 'json')
    )
  ),
  (
    2,
    JSON_OBJECT(
      'bucket', 'beta',
      'items', JSON_ARRAY(
        JSON_OBJECT('id', 20, 'name', 'plan', 'qty', 7)
      ),
      'tags', JSON_ARRAY('sql', 'optimizer', 'window')
    )
  );

-- ------------------------------------------------------------------------------
-- W16-A: AS closes the CREATE TABLE header and opens a WITH query. The two
-- sibling NESTED clauses are additive rather than a Cartesian product: ITEM
-- rows have null TAG columns and TAG rows have null ITEM columns. ROW_NUMBER
-- orders the null-complemented union after every JSON_TABLE alias is minted.
-- ------------------------------------------------------------------------------
CREATE TABLE sq_hard_w16_exploded
  ENGINE = InnoDB
  AS
WITH sibling_rows AS (
  SELECT d.document_id,
         jt.bucket_name,
         jt.item_no,
         jt.item_id,
         jt.item_name,
         jt.item_qty,
         jt.tag_no,
         jt.tag_name
  FROM sq_hard_w16_document AS d
  CROSS JOIN JSON_TABLE(
    d.body,
    '$' COLUMNS (
      bucket_name VARCHAR(20) PATH '$.bucket' ERROR ON ERROR,
      NESTED PATH '$.items[*]' COLUMNS (
        item_no   FOR ORDINALITY,
        item_id   INT PATH '$.id' ERROR ON ERROR,
        item_name VARCHAR(30) PATH '$.name' ERROR ON ERROR,
        item_qty  INT PATH '$.qty' ERROR ON ERROR
      ),
      NESTED PATH '$.tags[*]' COLUMNS (
        tag_no   FOR ORDINALITY,
        tag_name VARCHAR(30) PATH '$' ERROR ON ERROR
      )
    )
  ) AS jt
)
SELECT s.*,
       CASE
         WHEN s.item_id IS NOT NULL THEN 'ITEM'
         ELSE 'TAG'
       END AS row_kind,
       ROW_NUMBER() OVER (
         PARTITION BY s.document_id
         ORDER BY
           s.item_id IS NULL,
           COALESCE(s.item_no, s.tag_no),
           COALESCE(s.item_id, 0),
           COALESCE(s.tag_name, '')
       ) AS stream_no
FROM sibling_rows AS s;

SELECT document_id,
       bucket_name,
       item_no,
       item_id,
       item_name,
       item_qty,
       tag_no,
       tag_name,
       row_kind,
       stream_no
FROM sq_hard_w16_exploded
ORDER BY document_id, stream_no;

SELECT GROUP_CONCAT(
         CONCAT(
           grouped.document_id,
           ':I', grouped.item_rows,
           '/T', grouped.tag_rows
         )
         ORDER BY grouped.document_id
         SEPARATOR '/'
       ),
       SUM(grouped.total_rows)
INTO @sq_hard_w16_sibling_shape,
     @sq_hard_w16_sibling_rows
FROM (
  SELECT document_id,
         SUM(item_id IS NOT NULL) AS item_rows,
         SUM(tag_name IS NOT NULL) AS tag_rows,
         COUNT(*) AS total_rows
  FROM sq_hard_w16_exploded
  GROUP BY document_id
) AS grouped;

INSERT INTO sq_hard_w16_note (note_key, note_text, note_value)
VALUES (
  'sibling-json-ctas',
  @sq_hard_w16_sibling_shape,
  @sq_hard_w16_sibling_rows
);

-- ------------------------------------------------------------------------------
-- W16-B: DOCUMENT_COUNTS is visible to the INSERT's SELECT and both LATERAL
-- owners. TAG_ROLL also correlates to TOP_ROW, so its COUNT and GROUP_CONCAT
-- share two outer scopes. Generated columns turn the selected aliases back into
-- scalar text and JSON; a functional index immediately re-enters that JSON.
-- ------------------------------------------------------------------------------
CREATE TABLE sq_hard_w16_summary (
  document_id INT NOT NULL PRIMARY KEY,
  bucket_name VARCHAR(20) NOT NULL,
  item_count  INT NOT NULL,
  tag_count   INT NOT NULL,
  total_qty   INT NOT NULL,
  top_item    VARCHAR(30) NOT NULL,
  tag_list    VARCHAR(120) NOT NULL,
  shape_text  VARCHAR(300) GENERATED ALWAYS AS (
    CONCAT(
      bucket_name, ':',
      item_count, '/', tag_count, '/', total_qty, ':',
      top_item, ':', tag_list
    )
  ) STORED,
  summary_doc JSON GENERATED ALWAYS AS (
    JSON_OBJECT(
      'bucket', bucket_name,
      'items', item_count,
      'tags', tag_count,
      'qty', total_qty,
      'top', top_item,
      'tagList', tag_list
    )
  ) STORED,
  KEY ix_sq_hard_w16_summary_bucket (
    (JSON_VALUE(summary_doc, '$.bucket' RETURNING CHAR(20))),
    document_id DESC
  )
) ENGINE = InnoDB;

INSERT INTO sq_hard_w16_summary (
  document_id,
  bucket_name,
  item_count,
  tag_count,
  total_qty,
  top_item,
  tag_list
)
WITH document_counts AS (
  SELECT e.document_id,
         MAX(e.bucket_name) AS bucket_name,
         SUM(e.item_id IS NOT NULL) AS item_count,
         SUM(e.tag_name IS NOT NULL) AS tag_count,
         SUM(COALESCE(e.item_qty, 0)) AS total_qty
  FROM sq_hard_w16_exploded AS e
  GROUP BY e.document_id
)
SELECT counts.document_id,
       counts.bucket_name,
       counts.item_count,
       counts.tag_count,
       counts.total_qty,
       top_row.item_name,
       tag_roll.tag_list
FROM document_counts AS counts
JOIN LATERAL (
  SELECT e.item_name
  FROM sq_hard_w16_exploded AS e
  WHERE e.document_id = counts.document_id
    AND e.item_id IS NOT NULL
  ORDER BY e.item_qty DESC, e.item_id
  LIMIT 1
) AS top_row ON TRUE
JOIN LATERAL (
  SELECT GROUP_CONCAT(
           e.tag_name
           ORDER BY e.tag_no
           SEPARATOR ','
         ) AS tag_list,
         CONCAT(top_row.item_name, '@', COUNT(*)) AS top_evidence
  FROM sq_hard_w16_exploded AS e
  WHERE e.document_id = counts.document_id
    AND e.tag_name IS NOT NULL
) AS tag_roll ON TRUE;

SELECT GROUP_CONCAT(
         shape_text
         ORDER BY document_id
         SEPARATOR '/'
       ),
       SUM(total_qty)
INTO @sq_hard_w16_summary_shape,
     @sq_hard_w16_summary_total
FROM sq_hard_w16_summary;

INSERT INTO sq_hard_w16_note (note_key, note_text, note_value)
VALUES (
  'cte-lateral-summary',
  @sq_hard_w16_summary_shape,
  @sq_hard_w16_summary_total
);

-- ------------------------------------------------------------------------------
-- W16-C: TABLE contributes physical column names, VALUES ROW contributes derived
-- names, and EXCEPT removes one structurally identical row. The resulting CTE
-- owns the INSERT source, forcing all three table-expression grammars to agree
-- on arity, types and aliases before the target columns can be resolved.
-- ------------------------------------------------------------------------------
CREATE TABLE sq_hard_w16_item_stage
  ENGINE = InnoDB
  AS
SELECT item_id, item_name, item_qty
FROM sq_hard_w16_exploded
WHERE item_id IS NOT NULL;

ALTER TABLE sq_hard_w16_item_stage
  ADD PRIMARY KEY (item_id);

CREATE TABLE sq_hard_w16_candidate LIKE sq_hard_w16_item_stage;

INSERT INTO sq_hard_w16_candidate (item_id, item_name, item_qty)
WITH selected AS (
  TABLE sq_hard_w16_item_stage
  UNION ALL
  VALUES ROW(30, 'synthetic', 9)
  EXCEPT
  VALUES ROW(10, 'parse', 2)
)
SELECT item_id, item_name, item_qty
FROM selected;

SELECT GROUP_CONCAT(
         CONCAT(item_id, ':', item_name, ':', item_qty)
         ORDER BY item_id
         SEPARATOR '/'
       ),
       SUM(item_qty)
INTO @sq_hard_w16_candidate_shape,
     @sq_hard_w16_candidate_total
FROM sq_hard_w16_candidate;

INSERT INTO sq_hard_w16_note (note_key, note_text, note_value)
VALUES (
  'table-values-except',
  @sq_hard_w16_candidate_shape,
  @sq_hard_w16_candidate_total
);

-- ------------------------------------------------------------------------------
-- Wave-16 self-verification.
-- ------------------------------------------------------------------------------
CALL sq_hard_assert(
  @sq_hard_w16_sibling_shape = '1:I2/T2/2:I1/T3'
  AND @sq_hard_w16_sibling_rows = 8
  AND (SELECT COUNT(*) FROM sq_hard_w16_exploded) = 8,
  CONCAT('sibling JSON_TABLE: ',
         COALESCE(@sq_hard_w16_sibling_shape, 'NULL')));
CALL sq_hard_assert(
  @sq_hard_w16_summary_shape =
    'alpha:2/2/7:execute:sql,json/beta:1/3/7:plan:sql,optimizer,window'
  AND @sq_hard_w16_summary_total = 14
  AND (SELECT JSON_VALUE(summary_doc, '$.top')
       FROM sq_hard_w16_summary
       WHERE document_id = 1) = 'execute'
  AND (SELECT JSON_VALUE(summary_doc, '$.tags' RETURNING UNSIGNED)
       FROM sq_hard_w16_summary
       WHERE document_id = 2) = 3,
  CONCAT('CTE/LATERAL summary: ',
         COALESCE(@sq_hard_w16_summary_shape, 'NULL')));
CALL sq_hard_assert(
  @sq_hard_w16_candidate_shape =
    '11:execute:5/20:plan:7/30:synthetic:9'
  AND @sq_hard_w16_candidate_total = 21
  AND (SELECT COUNT(*) FROM sq_hard_w16_candidate) = 3,
  CONCAT('TABLE/VALUES/EXCEPT: ',
         COALESCE(@sq_hard_w16_candidate_shape, 'NULL')));
CALL sq_hard_assert(
  (SELECT COUNT(*) FROM sq_hard_w16_note) = 3,
  CONCAT('wave16 note rows: ',
         (SELECT COUNT(*) FROM sq_hard_w16_note)));

-- ------------------------------------------------------------------------------
-- ULTRA WAVE 17 -- spatial/full-text hybrid-ranking scope singularity.
-- One physical table owns BTREE, descending, FULLTEXT, SPATIAL and multi-valued
-- JSON indexes. Spatial and text scores become incompatible window ranks,
-- UNION ALL erases their original expressions, reciprocal-rank fusion groups
-- them back into documents, and inherited windows rank the fused result while
-- a correlated LATERAL JSON_TABLE mints the last relation in the pipeline.
-- ------------------------------------------------------------------------------
CREATE TABLE sq_hard_w17_document (
  document_id INT NOT NULL,
  title_text  VARCHAR(40) NOT NULL,
  body_text   TEXT NOT NULL,
  location    POINT SRID 4326 NOT NULL,
  attributes  JSON NOT NULL,
  boost_value INT GENERATED ALWAYS AS (
    JSON_VALUE(attributes, '$.boost' RETURNING UNSIGNED)
  ) STORED,
  PRIMARY KEY (document_id),
  SPATIAL KEY sx_sq_hard_w17_document (location),
  FULLTEXT KEY ft_sq_hard_w17_document (title_text, body_text),
  KEY mv_sq_hard_w17_document_tags ((
    CAST(attributes->'$.tags' AS CHAR(20) ARRAY)
  )),
  KEY ix_sq_hard_w17_document_boost (
    boost_value DESC,
    document_id
  )
) ENGINE = InnoDB;

CREATE TABLE sq_hard_w17_note (
  note_key   VARCHAR(30) NOT NULL PRIMARY KEY,
  note_text  VARCHAR(700) NOT NULL,
  note_value BIGINT NOT NULL
) ENGINE = InnoDB;

INSERT INTO sq_hard_w17_document (
  document_id,
  title_text,
  body_text,
  location,
  attributes
)
VALUES
  (
    1,
    'parser',
    'sql parser ; -- literal /* formatter */',
    ST_GeomFromText(
      'POINT(127.0276 37.4979)',
      4326,
      'axis-order=long-lat'
    ),
    JSON_MERGE_PRESERVE(
      JSON_OBJECT('tags', JSON_ARRAY('sql', 'parser')),
      JSON_OBJECT('boost', 9)
    )
  ),
  (
    2,
    'optimizer',
    'sql query optimizer # literal $$',
    ST_GeomFromText(
      'POINT(127.1000 37.6000)',
      4326,
      'axis-order=long-lat'
    ),
    JSON_OBJECT('tags', JSON_ARRAY('sql', 'optimizer'), 'boost', 7)
  ),
  (
    3,
    'spatial',
    'spatial search index',
    ST_GeomFromText(
      'POINT(129.0756 35.1796)',
      4326,
      'axis-order=long-lat'
    ),
    JSON_OBJECT('tags', JSON_ARRAY('spatial', 'search'), 'boost', 5)
  ),
  (
    4,
    'highlight',
    'format highlighter',
    ST_GeomFromText(
      'POINT(13.4050 52.5200)',
      4326,
      'axis-order=long-lat'
    ),
    JSON_OBJECT('tags', JSON_ARRAY('format', 'highlight'), 'boost', 3)
  );

SET @sq_hard_w17_query_point = ST_GeomFromText(
  'POINT(127.0276 37.4979)',
  4326,
  'axis-order=long-lat'
);

-- ------------------------------------------------------------------------------
-- W17-A: each source rank has a different ordering expression. RANK_UNION
-- preserves only SOURCE_NAME and SOURCE_RANK. W_SCORE inherits the empty W_ALL
-- specification before adding its order; ROW_NUMBER and DENSE_RANK consume the
-- same inherited window after the LATERAL tag owner has correlated to D.
-- ------------------------------------------------------------------------------
WITH
  spatial_ranked AS (
    SELECT d.document_id,
           ROW_NUMBER() OVER (
             ORDER BY ST_Distance_Sphere(
                        d.location,
                        @sq_hard_w17_query_point
                      ),
                      d.document_id
           ) AS source_rank
    FROM sq_hard_w17_document AS d
  ),
  text_ranked AS (
    SELECT d.document_id,
           ROW_NUMBER() OVER (
             ORDER BY MATCH(d.title_text, d.body_text)
                        AGAINST ('+sql' IN BOOLEAN MODE) DESC,
                      d.document_id
           ) AS source_rank
    FROM sq_hard_w17_document AS d
    WHERE MATCH(d.title_text, d.body_text)
            AGAINST ('+sql' IN BOOLEAN MODE) > 0
  ),
  rank_union AS (
    SELECT document_id, 'spatial' AS source_name, source_rank
    FROM spatial_ranked
    WHERE source_rank <= 3
    UNION ALL
    SELECT document_id, 'text', source_rank
    FROM text_ranked
  ),
  reciprocal_scores AS (
    SELECT document_id,
           SUM(1.0 / (60 + source_rank)) AS reciprocal_score,
           GROUP_CONCAT(
             CONCAT(source_name, '#', source_rank)
             ORDER BY source_name
             SEPARATOR ','
           ) AS source_shape
    FROM rank_union
    GROUP BY document_id
  ),
  hybrid_ranked AS (
    SELECT d.document_id,
           d.title_text,
           d.attributes,
           tags.tag_shape,
           r.source_shape,
           r.reciprocal_score,
           ROW_NUMBER() OVER w_score AS hybrid_position,
           DENSE_RANK() OVER w_score AS hybrid_dense_rank
    FROM reciprocal_scores AS r
    JOIN sq_hard_w17_document AS d USING (document_id)
    JOIN LATERAL (
      SELECT GROUP_CONCAT(
               jt.tag_name
               ORDER BY jt.tag_no
               SEPARATOR ','
             ) AS tag_shape
      FROM JSON_TABLE(
        d.attributes,
        '$.tags[*]' COLUMNS (
          tag_no   FOR ORDINALITY,
          tag_name VARCHAR(20) PATH '$'
        )
      ) AS jt
    ) AS tags ON TRUE
    WINDOW
      w_all AS (),
      w_score AS (
        w_all
        ORDER BY r.reciprocal_score DESC, d.document_id
      )
  )
SELECT GROUP_CONCAT(
         CONCAT(
           document_id,
           ':',
           source_shape,
           ':',
           tag_shape
         )
         ORDER BY hybrid_position
         SEPARATOR '/'
       ),
       SUM(hybrid_dense_rank),
       JSON_OBJECTAGG(
         document_id,
         JSON_OBJECT(
           'title', title_text,
           'position', hybrid_position,
           'sources', source_shape,
           'tags', tag_shape,
           'document', JSON_MERGE_PRESERVE(
             attributes,
             JSON_OBJECT('position', hybrid_position)
           )
         )
       )
INTO @sq_hard_w17_hybrid_shape,
     @sq_hard_w17_rank_total,
     @sq_hard_w17_hybrid_json
FROM hybrid_ranked;

-- ------------------------------------------------------------------------------
-- W17-B: INFORMATION_SCHEMA must expose five physical index grammars. The
-- ordered scalar intentionally includes multi-part FULLTEXT and BTREE indexes.
-- ------------------------------------------------------------------------------
SELECT GROUP_CONCAT(
         CONCAT(INDEX_NAME, ':', INDEX_TYPE, ':', SEQ_IN_INDEX)
         ORDER BY INDEX_NAME, SEQ_IN_INDEX
         SEPARATOR '/'
       )
INTO @sq_hard_w17_index_shape
FROM information_schema.STATISTICS
WHERE TABLE_SCHEMA = DATABASE()
  AND TABLE_NAME = 'sq_hard_w17_document';

INSERT INTO sq_hard_w17_note (note_key, note_text, note_value)
VALUES (
         'hybrid-rank',
         @sq_hard_w17_hybrid_shape,
         @sq_hard_w17_rank_total
       ),
       (
         'index-json',
         @sq_hard_w17_index_shape,
         JSON_LENGTH(@sq_hard_w17_hybrid_json)
       );

-- ------------------------------------------------------------------------------
-- Wave-17 self-verification.
-- ------------------------------------------------------------------------------
CALL sq_hard_assert(
  @sq_hard_w17_hybrid_shape =
    '1:spatial#1,text#1:sql,parser/'
    '2:spatial#2,text#2:sql,optimizer/'
    '3:spatial#3:spatial,search'
  AND @sq_hard_w17_rank_total = 6,
  CONCAT('hybrid rank: ',
         COALESCE(@sq_hard_w17_hybrid_shape, 'NULL')));
CALL sq_hard_assert(
  JSON_VALUE(@sq_hard_w17_hybrid_json, '$."1".title') = 'parser'
  AND JSON_VALUE(@sq_hard_w17_hybrid_json, '$."3".position') = 3
  AND JSON_VALUE(
        @sq_hard_w17_hybrid_json,
        '$."1".document.position'
      ) = 1
  AND JSON_LENGTH(@sq_hard_w17_hybrid_json) = 3,
  CONCAT(
    'hybrid json: ',
    COALESCE(
      JSON_VALUE(@sq_hard_w17_hybrid_json, '$."1".title'),
      'NULL'
    ),
    '/',
    COALESCE(
      JSON_VALUE(@sq_hard_w17_hybrid_json, '$."3".position'),
      'NULL'
    )
  ));
CALL sq_hard_assert(
  @sq_hard_w17_index_shape =
    'ft_sq_hard_w17_document:FULLTEXT:1/'
    'ft_sq_hard_w17_document:FULLTEXT:2/'
    'ix_sq_hard_w17_document_boost:BTREE:1/'
    'ix_sq_hard_w17_document_boost:BTREE:2/'
    'mv_sq_hard_w17_document_tags:BTREE:1/'
    'PRIMARY:BTREE:1/'
    'sx_sq_hard_w17_document:SPATIAL:1'
  AND (SELECT ROUND(
                ST_Distance_Sphere(
                  location,
                  @sq_hard_w17_query_point
                )
              )
       FROM sq_hard_w17_document
       WHERE document_id = 1) = 0
  AND (SELECT COUNT(*)
       FROM sq_hard_w17_document
       WHERE 'sql' MEMBER OF (attributes->'$.tags')) = 2,
  CONCAT('hybrid indexes: ',
         COALESCE(@sq_hard_w17_index_shape, 'NULL')));
CALL sq_hard_assert(
  (SELECT COUNT(*) FROM sq_hard_w17_note) = 2,
  CONCAT('wave17 note rows: ',
         (SELECT COUNT(*) FROM sq_hard_w17_note)));

-- ------------------------------------------------------------------------------
-- ULTRA WAVE 18 -- recursive JSON graph/window/set-algebra scope singularity.
--
-- EDGES and TAGS exist only inside JSON. The recursive member must correlate a
-- JSON_TABLE to its current graph row, carry a JSON visited-set, reject a real
-- cycle with MEMBER OF, and preserve a widened text path across recursion.
-- INTERSECT/EXCEPT then derives terminal nodes from both physical and recursive
-- relations. A LATERAL JSON_TABLE aggregate decorates every repeated endpoint,
-- after which four named/inherited windows compute path order, running state,
-- depth rank, and a JSON window array. One deterministic signature closes every
-- recursive, table-function, set-operation, lateral and analytic alias scope.
-- ------------------------------------------------------------------------------
CREATE TABLE sq_hard_w18_graph (
  node_id    INT NOT NULL PRIMARY KEY,
  node_label VARCHAR(30) NOT NULL,
  payload    JSON NOT NULL,
  node_score INT GENERATED ALWAYS AS (
    JSON_VALUE(payload, '$.score' RETURNING UNSIGNED)
  ) STORED,
  KEY ix_sq_hard_w18_score (node_score DESC, node_id),
  CONSTRAINT ck_sq_hard_w18_payload_object
    CHECK (JSON_TYPE(payload) = 'OBJECT')
) ENGINE = InnoDB;

CREATE TABLE sq_hard_w18_note (
  note_key   VARCHAR(30) NOT NULL PRIMARY KEY,
  note_text  VARCHAR(1000) NOT NULL,
  note_value BIGINT NOT NULL
) ENGINE = InnoDB;

INSERT INTO sq_hard_w18_graph (node_id, node_label, payload)
VALUES
  (
    1,
    'root',
    JSON_OBJECT(
      'score', 5,
      'edges', JSON_ARRAY(2, 3),
      'tags', JSON_ARRAY('root', 'sql')
    )
  ),
  (
    2,
    'parser',
    JSON_OBJECT(
      'score', 7,
      'edges', JSON_ARRAY(4),
      'tags', JSON_ARRAY('parser', 'sql')
    )
  ),
  (
    3,
    'formatter',
    JSON_OBJECT(
      'score', 11,
      'edges', JSON_ARRAY(4, 1),
      'tags', JSON_ARRAY('formatter', 'sql')
    )
  ),
  (
    4,
    'leaf',
    JSON_OBJECT(
      'score', 13,
      'edges', JSON_ARRAY(),
      'tags', JSON_ARRAY('leaf', 'sql')
    )
  );

WITH RECURSIVE
  graph_walk (
    root_id,
    node_id,
    depth_no,
    path_text,
    visited_nodes,
    path_score
  ) AS (
    SELECT g.node_id,
           g.node_id,
           0,
           CAST(g.node_id AS CHAR(200)),
           JSON_ARRAY(g.node_id),
           g.node_score
    FROM sq_hard_w18_graph AS g
    WHERE g.node_id = 1

    UNION ALL

    SELECT w.root_id,
           edge.child_id,
           w.depth_no + 1,
           CONCAT(w.path_text, '>', edge.child_id),
           JSON_ARRAY_APPEND(
             w.visited_nodes,
             '$',
             edge.child_id
           ),
           w.path_score + child.node_score
    FROM graph_walk AS w
    JOIN sq_hard_w18_graph AS parent
      ON parent.node_id = w.node_id
    JOIN JSON_TABLE(
           parent.payload,
           '$.edges[*]' COLUMNS (
             edge_no  FOR ORDINALITY,
             child_id INT PATH '$' ERROR ON ERROR
           )
         ) AS edge ON TRUE
    JOIN sq_hard_w18_graph AS child
      ON child.node_id = edge.child_id
    WHERE w.depth_no < 4
      AND NOT (edge.child_id MEMBER OF (w.visited_nodes))
  ),
  terminal_nodes AS (
    SELECT g.node_id
    FROM sq_hard_w18_graph AS g
    WHERE 'leaf' MEMBER OF (g.payload->'$.tags')

    INTERSECT

    SELECT w.node_id
    FROM graph_walk AS w
    WHERE w.depth_no > 0

    EXCEPT

    SELECT excluded.node_id
    FROM (VALUES ROW(999)) AS excluded (node_id)
  ),
  endpoint_facts AS (
    SELECT w.root_id,
           w.node_id,
           w.depth_no,
           w.path_text,
           w.path_score,
           tag_roll.tag_shape,
           terminal.node_id IS NOT NULL AS is_terminal
    FROM graph_walk AS w
    JOIN sq_hard_w18_graph AS endpoint
      ON endpoint.node_id = w.node_id
    JOIN LATERAL (
      SELECT GROUP_CONCAT(
               tag.tag_name
               ORDER BY tag.tag_no
               SEPARATOR ','
             ) AS tag_shape
      FROM sq_hard_w18_graph AS tag_owner
      JOIN JSON_TABLE(
             tag_owner.payload,
             '$.tags[*]' COLUMNS (
               tag_no   FOR ORDINALITY,
               tag_name VARCHAR(30) PATH '$' ERROR ON ERROR
             )
           ) AS tag ON TRUE
      WHERE tag_owner.node_id = endpoint.node_id
    ) AS tag_roll ON TRUE
    LEFT JOIN terminal_nodes AS terminal
      ON terminal.node_id = w.node_id
  ),
  analytic_paths AS (
    SELECT f.root_id,
           f.node_id,
           f.depth_no,
           f.path_text,
           f.path_score,
           f.tag_shape,
           f.is_terminal,
           ROW_NUMBER() OVER w_path AS path_no,
           SUM(f.path_score) OVER w_running AS running_score,
           DENSE_RANK() OVER w_depth_score AS depth_rank,
           JSON_ARRAYAGG(f.node_id) OVER w_running AS visited_window
    FROM endpoint_facts AS f
    WINDOW
      w_root AS (
        PARTITION BY f.root_id
      ),
      w_path AS (
        w_root
        ORDER BY f.depth_no, f.path_text
      ),
      w_running AS (
        w_path
        ROWS BETWEEN UNBOUNDED PRECEDING AND CURRENT ROW
      ),
      w_depth_score AS (
        PARTITION BY f.root_id, f.depth_no
        ORDER BY f.path_score DESC
      )
  )
SELECT GROUP_CONCAT(
         CONCAT(
           path_text, '@', node_id, ':',
           path_score, ':', running_score, ':',
           depth_rank, ':', tag_shape, ':',
           IF(is_terminal, 'T', 'N')
         )
         ORDER BY path_no
         SEPARATOR '/'
       ),
       MAX(running_score),
       SUM(JSON_LENGTH(visited_window)),
       COUNT(*)
INTO @sq_hard_w18_walk_shape,
     @sq_hard_w18_running_score,
     @sq_hard_w18_json_cells,
     @sq_hard_w18_walk_rows
FROM analytic_paths;

INSERT INTO sq_hard_w18_note (note_key, note_text, note_value)
VALUES (
  'recursive-json-window',
  @sq_hard_w18_walk_shape,
  @sq_hard_w18_running_score
);

-- ------------------------------------------------------------------------------
-- Wave-18 self-verification.
-- ------------------------------------------------------------------------------
CALL sq_hard_assert(
  @sq_hard_w18_walk_shape =
    '1@1:5:5:1:root,sql:N/'
    '1>2@2:12:17:2:parser,sql:N/'
    '1>3@3:16:33:1:formatter,sql:N/'
    '1>2>4@4:25:58:2:leaf,sql:T/'
    '1>3>4@4:29:87:1:leaf,sql:T',
  CONCAT(
    'recursive graph shape length: ',
    COALESCE(CHAR_LENGTH(@sq_hard_w18_walk_shape), -1)
  ));
CALL sq_hard_assert(
  @sq_hard_w18_running_score = 87
  AND @sq_hard_w18_json_cells = 15
  AND @sq_hard_w18_walk_rows = 5
  AND (SELECT COUNT(*)
       FROM sq_hard_w18_graph
       WHERE 'sql' MEMBER OF (payload->'$.tags')) = 4,
  CONCAT(
    'recursive graph totals: ',
    COALESCE(@sq_hard_w18_running_score, -1),
    '/', COALESCE(@sq_hard_w18_json_cells, -1),
    '/', COALESCE(@sq_hard_w18_walk_rows, -1)
  ));
CALL sq_hard_assert(
  (SELECT COUNT(*) FROM sq_hard_w18_note) = 1
  AND (SELECT note_value
       FROM sq_hard_w18_note
       WHERE note_key = 'recursive-json-window') = 87,
  CONCAT('wave18 note rows: ',
         (SELECT COUNT(*) FROM sq_hard_w18_note)));

-- ------------------------------------------------------------------------------
-- ULTRA WAVE 19 -- right-correlated table-function and lock-set singularity.
--
-- JSON_TABLE appears syntactically before the table that supplies its document,
-- yet RIGHT JOIN makes that right-hand outer table visible to the function.
-- Three inherited windows operate over the reversed dependency, JSON_ARRAYAGG
-- carries a running JSON value, and a second CTE picks each final state. A
-- separate parenthesized UNION puts FOR UPDATE and FOR SHARE inside different
-- query-expression owners while NOWAIT and SKIP LOCKED close each branch.
-- ------------------------------------------------------------------------------
CREATE TABLE sq_hard_w19_document (
  document_id INT NOT NULL PRIMARY KEY,
  payload     JSON NOT NULL,
  score       INT GENERATED ALWAYS AS (
    JSON_VALUE(payload, '$.score' RETURNING UNSIGNED)
  ) STORED,
  CONSTRAINT ck_sq_hard_w19_payload_object
    CHECK (JSON_TYPE(payload) = 'OBJECT'),
  KEY ix_sq_hard_w19_score (score DESC, document_id)
) ENGINE = InnoDB;

CREATE TABLE sq_hard_w19_note (
  note_key   VARCHAR(30) NOT NULL PRIMARY KEY,
  note_text  VARCHAR(700) NOT NULL,
  note_value BIGINT NOT NULL
) ENGINE = InnoDB;

INSERT INTO sq_hard_w19_document (document_id, payload)
VALUES
  (
    1,
    JSON_OBJECT(
      'score', 8,
      'items', JSON_ARRAY(3, 5),
      'tags', JSON_ARRAY('sql', 'json')
    )
  ),
  (
    2,
    JSON_OBJECT(
      'score', 18,
      'items', JSON_ARRAY(7, 11),
      'tags', JSON_ARRAY('window', 'right')
    )
  ),
  (
    3,
    JSON_OBJECT(
      'score', 13,
      'items', JSON_ARRAY(13),
      'tags', JSON_ARRAY('lock')
    )
  );

-- ------------------------------------------------------------------------------
-- W19-A: DOCUMENT_OWNER is declared to the right of ITEM_ROW, but is visible
-- inside JSON_TABLE because it is the outer side of RIGHT JOIN. W_RUNNING and
-- W_LAST independently inherit W_DOCUMENT before the CTE boundary turns the
-- window outputs back into ordinary relation columns.
-- ------------------------------------------------------------------------------
WITH
  right_correlated AS (
    SELECT document_owner.document_id,
           document_owner.payload,
           item_row.item_no,
           item_row.item_value,
           SUM(item_row.item_value) OVER w_running AS running_value,
           ROW_NUMBER() OVER w_last                AS reverse_no,
           JSON_ARRAYAGG(item_row.item_value)
             OVER w_running                        AS running_json
    FROM JSON_TABLE(
           document_owner.payload,
           '$.items[*]' COLUMNS (
             item_no    FOR ORDINALITY,
             item_value INT PATH '$'
           )
         ) AS item_row
    RIGHT JOIN sq_hard_w19_document AS document_owner
      ON TRUE
    WINDOW
      w_document AS (
        PARTITION BY document_owner.document_id
      ),
      w_running AS (
        w_document
        ORDER BY item_row.item_no
        ROWS BETWEEN UNBOUNDED PRECEDING AND CURRENT ROW
      ),
      w_last AS (
        w_document
        ORDER BY item_row.item_no DESC
      )
  ),
  final_items AS (
    SELECT right_correlated.*
    FROM right_correlated
    WHERE reverse_no = 1
  ),
  decorated AS (
    SELECT final_items.document_id,
           final_items.running_value,
           final_items.running_json,
           CONCAT_WS(
             ',',
             final_items.payload ->> '$.tags[0]',
             final_items.payload ->> '$.tags[1]'
           ) AS tag_shape
    FROM final_items
  )
SELECT GROUP_CONCAT(
         CONCAT(
           document_id, ':',
           running_value, ':',
           tag_shape
         )
         ORDER BY document_id
         SEPARATOR '/'
       ),
       SUM(JSON_LENGTH(running_json)),
       JSON_OBJECTAGG(document_id, running_json)
INTO @sq_hard_w19_right_shape,
     @sq_hard_w19_json_cells,
     @sq_hard_w19_running_doc
FROM decorated;

INSERT INTO sq_hard_w19_note (note_key, note_text, note_value)
VALUES (
  'right-json-window',
  @sq_hard_w19_right_shape,
  @sq_hard_w19_json_cells
);

-- ------------------------------------------------------------------------------
-- W19-B: each parenthesized set operand owns its own locking clause. OF resolves
-- the branch-local table alias; NOWAIT and SKIP LOCKED are mutually exclusive
-- tails whose scopes must close before UNION ALL and the outer GROUP_CONCAT.
-- ------------------------------------------------------------------------------
START TRANSACTION;

SELECT GROUP_CONCAT(
         CONCAT(lock_rows.document_id, ':', lock_rows.lock_mode)
         ORDER BY lock_rows.document_id
         SEPARATOR '/'
       )
INTO @sq_hard_w19_lock_shape
FROM (
  (
    SELECT document_owner.document_id,
           'U' AS lock_mode
    FROM sq_hard_w19_document AS document_owner
    WHERE document_owner.document_id IN (1, 2)
    FOR UPDATE OF document_owner NOWAIT
  )
  UNION ALL
  (
    SELECT document_owner.document_id,
           'S' AS lock_mode
    FROM sq_hard_w19_document AS document_owner
    WHERE document_owner.document_id = 3
    FOR SHARE OF document_owner SKIP LOCKED
  )
) AS lock_rows;

ROLLBACK;

INSERT INTO sq_hard_w19_note (note_key, note_text, note_value)
VALUES (
  'parenthesized-lock-set',
  @sq_hard_w19_lock_shape,
  (SELECT SUM(score) FROM sq_hard_w19_document)
);

-- ------------------------------------------------------------------------------
-- Wave-19 self-verification.
-- ------------------------------------------------------------------------------
CALL sq_hard_assert(
  @sq_hard_w19_right_shape =
    '1:8:sql,json/2:18:window,right/3:13:lock'
  AND @sq_hard_w19_json_cells = 5,
  CONCAT(
    'right-correlated JSON window: ',
    COALESCE(@sq_hard_w19_right_shape, 'NULL'),
    '/',
    COALESCE(@sq_hard_w19_json_cells, -1)
  ));
CALL sq_hard_assert(
  JSON_EXTRACT(@sq_hard_w19_running_doc, '$."1"') =
    CAST('[3, 5]' AS JSON)
  AND JSON_EXTRACT(@sq_hard_w19_running_doc, '$."2"') =
    CAST('[7, 11]' AS JSON)
  AND JSON_EXTRACT(@sq_hard_w19_running_doc, '$."3"') =
    CAST('[13]' AS JSON),
  CONCAT(
    'right-correlated JSON document: ',
    COALESCE(@sq_hard_w19_running_doc, 'NULL')
  ));
CALL sq_hard_assert(
  @sq_hard_w19_lock_shape = '1:U/2:U/3:S'
  AND (SELECT note_value
       FROM sq_hard_w19_note
       WHERE note_key = 'parenthesized-lock-set') = 39,
  CONCAT(
    'parenthesized lock set: ',
    COALESCE(@sq_hard_w19_lock_shape, 'NULL')
  ));
CALL sq_hard_assert(
  (SELECT COUNT(*) FROM sq_hard_w19_note) = 2,
  CONCAT('wave19 note rows: ',
         (SELECT COUNT(*) FROM sq_hard_w19_note)));

-- ------------------------------------------------------------------------------
-- ULTRA WAVE 20 -- multi-valued JSON index, set-table, and cycle-guard endgame.
--
-- A JSON Schema CHECK validates every document while a multi-valued functional
-- key indexes each member of $.codes. JSON_TABLE and three inherited windows
-- then rebuild matching documents, row-alias upsert mutates one document, and
-- TABLE / VALUES participate directly in INTERSECT ALL / EXCEPT ALL. Recursive
-- traversal cannot use a CYCLE clause in MySQL, so a JSON array becomes the
-- visited-path relation and MEMBER OF guards the back edge.
-- ------------------------------------------------------------------------------
CREATE TABLE sq_hard_w20_document (
  document_id INT NOT NULL PRIMARY KEY,
  tenant_id   INT NOT NULL,
  payload     JSON NOT NULL,
  state_name  VARCHAR(20)
    GENERATED ALWAYS AS (
      JSON_UNQUOTE(JSON_EXTRACT(payload, '$.state'))
    ) STORED,
  CONSTRAINT ck_sq_hard_w20_document_schema
    CHECK (
      JSON_SCHEMA_VALID(
        '{
          "type": "object",
          "required": ["state", "codes", "items"],
          "properties": {
            "state": {"type": "string"},
            "codes": {
              "type": "array",
              "items": {"type": "integer"}
            },
            "items": {"type": "array"}
          }
        }',
        payload
      )
    ),
  INDEX ix_sq_hard_w20_tenant_codes (
    tenant_id,
    (CAST(payload -> '$.codes' AS UNSIGNED ARRAY))
  ),
  INDEX ix_sq_hard_w20_state (state_name, document_id DESC)
) ENGINE = InnoDB;

CREATE TABLE sq_hard_w20_edge (
  from_id INT NOT NULL,
  to_id   INT NOT NULL,
  PRIMARY KEY (from_id, to_id),
  CONSTRAINT fk_sq_hard_w20_edge_from
    FOREIGN KEY (from_id)
    REFERENCES sq_hard_w20_document (document_id),
  CONSTRAINT fk_sq_hard_w20_edge_to
    FOREIGN KEY (to_id)
    REFERENCES sq_hard_w20_document (document_id)
) ENGINE = InnoDB;

CREATE TABLE sq_hard_w20_left_bag (
  token INT NOT NULL
) ENGINE = InnoDB;

CREATE TABLE sq_hard_w20_right_bag (
  token INT NOT NULL
) ENGINE = InnoDB;

CREATE TABLE sq_hard_w20_remove_bag (
  token INT NOT NULL
) ENGINE = InnoDB;

CREATE TABLE sq_hard_w20_note (
  note_key   VARCHAR(30) NOT NULL PRIMARY KEY,
  note_text  VARCHAR(700) NOT NULL,
  note_value BIGINT NOT NULL
) ENGINE = InnoDB;

INSERT INTO sq_hard_w20_document (document_id, tenant_id, payload)
VALUES
  (
    1,
    1,
    JSON_OBJECT(
      'state', 'open',
      'codes', JSON_ARRAY(7, 11),
      'items', JSON_ARRAY(
        JSON_OBJECT('kind', 'alpha', 'value', 3, 'flag', TRUE),
        JSON_OBJECT('kind', 'beta',  'value', 5)
      )
    )
  ),
  (
    2,
    1,
    JSON_OBJECT(
      'state', 'open',
      'codes', JSON_ARRAY(11, 13),
      'items', JSON_ARRAY(
        JSON_OBJECT('kind', 'gamma',   'value', 7),
        JSON_OBJECT('kind', 'delta',   'value', 11),
        JSON_OBJECT('kind', 'epsilon', 'value', 13)
      )
    )
  ),
  (
    3,
    1,
    JSON_OBJECT(
      'state', 'closed',
      'codes', JSON_ARRAY(17),
      'items', JSON_ARRAY(
        JSON_OBJECT('kind', 'zeta', 'value', 17)
      )
    )
  ),
  (
    4,
    2,
    JSON_OBJECT(
      'state', 'open',
      'codes', JSON_ARRAY(13, 19),
      'items', JSON_ARRAY(
        JSON_OBJECT('kind', 'eta',   'value', 6),
        JSON_OBJECT('kind', 'theta', 'value', 10)
      )
    )
  ),
  (
    5,
    2,
    JSON_OBJECT(
      'state', 'closed',
      'codes', JSON_ARRAY(23),
      'items', JSON_ARRAY(
        JSON_OBJECT('kind', 'omega', 'value', 12)
      )
    )
  );

INSERT INTO sq_hard_w20_edge (from_id, to_id)
VALUES
  (1, 2),
  (2, 3),
  (2, 4),
  (3, 1),
  (4, 5);

INSERT INTO sq_hard_w20_left_bag (token)
VALUES ROW(1), ROW(1), ROW(2), ROW(2), ROW(3);

INSERT INTO sq_hard_w20_right_bag (token)
VALUES ROW(1), ROW(1), ROW(1), ROW(2), ROW(3), ROW(3);

INSERT INTO sq_hard_w20_remove_bag (token)
VALUES ROW(1), ROW(3);

-- ------------------------------------------------------------------------------
-- W20-A: MEMBER OF can use the multi-valued key while JSON_TABLE owns item
-- ordinality and EXISTS columns. JSON_ARRAYAGG is a window function here; the
-- last row of W_REVERSE simultaneously owns the completed W_RUNNING frame.
-- ------------------------------------------------------------------------------
WITH
  expanded AS (
    SELECT document_owner.document_id,
           item_owner.item_no,
           item_owner.item_kind,
           item_owner.item_value,
           item_owner.has_flag,
           SUM(item_owner.item_value)
             OVER w_running AS running_value,
           JSON_ARRAYAGG(item_owner.item_kind)
             OVER w_running AS running_kinds,
           ROW_NUMBER()
             OVER w_reverse AS reverse_no
    FROM sq_hard_w20_document AS document_owner
      FORCE INDEX (ix_sq_hard_w20_tenant_codes)
    JOIN JSON_TABLE(
           document_owner.payload,
           '$.items[*]' COLUMNS (
             item_no    FOR ORDINALITY,
             item_kind  VARCHAR(20) PATH '$.kind'
               ERROR ON EMPTY ERROR ON ERROR,
             item_value INT PATH '$.value'
               ERROR ON EMPTY ERROR ON ERROR,
             has_flag   INT EXISTS PATH '$.flag'
           )
         ) AS item_owner ON TRUE
    WHERE document_owner.tenant_id = 1
      AND 11 MEMBER OF (document_owner.payload -> '$.codes')
    WINDOW
      w_document AS (
        PARTITION BY document_owner.document_id
      ),
      w_running AS (
        w_document
        ORDER BY item_owner.item_no
        ROWS BETWEEN UNBOUNDED PRECEDING AND CURRENT ROW
      ),
      w_reverse AS (
        w_document
        ORDER BY item_owner.item_no DESC
      )
  ),
  final_items AS (
    SELECT document_id,
           running_value,
           running_kinds
    FROM expanded
    WHERE reverse_no = 1
  )
SELECT GROUP_CONCAT(
         CONCAT(
           document_id, ':',
           running_value, ':',
           JSON_UNQUOTE(
             JSON_EXTRACT(
               running_kinds,
               CONCAT('$[', JSON_LENGTH(running_kinds) - 1, ']')
             )
           )
         )
         ORDER BY document_id
         SEPARATOR '/'
       ),
       SUM(JSON_LENGTH(running_kinds)),
       JSON_OBJECTAGG(document_id, running_kinds),
       (SELECT SUM(has_flag) FROM expanded)
INTO @sq_hard_w20_json_shape,
     @sq_hard_w20_json_cells,
     @sq_hard_w20_running_doc,
     @sq_hard_w20_flag_total
FROM final_items;

SELECT COUNT(*)
INTO @sq_hard_w20_overlap_rows
FROM sq_hard_w20_document
WHERE JSON_OVERLAPS(
        payload -> '$.codes',
        CAST('[7, 13]' AS JSON)
      );

INSERT INTO sq_hard_w20_note (note_key, note_text, note_value)
VALUES (
  'json-multivalue-window',
  @sq_hard_w20_json_shape,
  @sq_hard_w20_json_cells
);

-- ------------------------------------------------------------------------------
-- W20-B: the alias after VALUES names the candidate row and its insert columns.
-- JSON_MERGE_PATCH reads both the target row and INCOMING_PAYLOAD without using
-- deprecated VALUES(column) syntax.
-- ------------------------------------------------------------------------------
INSERT INTO sq_hard_w20_document (document_id, tenant_id, payload)
VALUES (
  2,
  1,
  JSON_OBJECT(
    'state', 'review',
    'codes', JSON_ARRAY(11, 13),
    'items', JSON_ARRAY(
      JSON_OBJECT('kind', 'gamma',   'value', 7),
      JSON_OBJECT('kind', 'delta',   'value', 11),
      JSON_OBJECT('kind', 'epsilon', 'value', 13)
    ),
    'extra', JSON_OBJECT('priority', 9)
  )
) AS incoming (
  document_id,
  tenant_id,
  payload
)
ON DUPLICATE KEY UPDATE
  payload = JSON_MERGE_PATCH(
    sq_hard_w20_document.payload,
    incoming.payload
  );

-- ------------------------------------------------------------------------------
-- W20-C: TABLE and VALUES are complete query expressions. INTERSECT ALL keeps
-- duplicate multiplicity before EXCEPT ALL removes one 1 and the only 3.
-- ------------------------------------------------------------------------------
SELECT GROUP_CONCAT(
         multiset_owner.token
         ORDER BY multiset_owner.token
         SEPARATOR ','
       )
INTO @sq_hard_w20_bag_shape
FROM (
  (
    (TABLE sq_hard_w20_left_bag)
    INTERSECT ALL
    (TABLE sq_hard_w20_right_bag)
  )
  EXCEPT ALL
  (VALUES ROW(1), ROW(3))
) AS multiset_owner;

INSERT INTO sq_hard_w20_note (note_key, note_text, note_value)
VALUES (
  'table-values-multiset',
  @sq_hard_w20_bag_shape,
  2
);

-- ------------------------------------------------------------------------------
-- W20-D: MySQL has no CYCLE clause. PATH_DOC is both JSON state and a recursive
-- relation; MEMBER OF prevents 3 -> 1 while JSON_ARRAY_APPEND extends each
-- surviving branch. The SET_VAR hint belongs to the outer query block.
-- ------------------------------------------------------------------------------
WITH RECURSIVE
  walk (
    node_id,
    depth_no,
    path_doc
  ) AS (
    SELECT 1,
           0,
           JSON_ARRAY(1)
    UNION ALL
    SELECT edge_owner.to_id,
           walk.depth_no + 1,
           JSON_ARRAY_APPEND(
             walk.path_doc,
             '$',
             edge_owner.to_id
           )
    FROM walk
    JOIN sq_hard_w20_edge AS edge_owner
      ON edge_owner.from_id = walk.node_id
    WHERE NOT (
      edge_owner.to_id MEMBER OF (walk.path_doc)
    )
  )
SELECT /*+ SET_VAR(cte_max_recursion_depth=20) */
       GROUP_CONCAT(
         CONCAT(node_id, ':', depth_no)
         ORDER BY depth_no, node_id
         SEPARATOR '/'
       ),
       SUM(depth_no),
       MAX(JSON_LENGTH(path_doc))
INTO @sq_hard_w20_walk_shape,
     @sq_hard_w20_walk_depth,
     @sq_hard_w20_walk_path_length
FROM walk;

INSERT INTO sq_hard_w20_note (note_key, note_text, note_value)
VALUES (
  'json-cycle-guard',
  @sq_hard_w20_walk_shape,
  @sq_hard_w20_walk_depth
);

-- ------------------------------------------------------------------------------
-- Wave-20 self-verification.
-- ------------------------------------------------------------------------------
CALL sq_hard_assert(
  @sq_hard_w20_json_shape = '1:8:beta/2:31:epsilon'
  AND @sq_hard_w20_json_cells = 5
  AND @sq_hard_w20_flag_total = 1,
  CONCAT(
    'multi-valued JSON window: ',
    COALESCE(@sq_hard_w20_json_shape, 'NULL'),
    '/',
    COALESCE(@sq_hard_w20_json_cells, -1),
    '/',
    COALESCE(@sq_hard_w20_flag_total, -1)
  ));
CALL sq_hard_assert(
  JSON_EXTRACT(@sq_hard_w20_running_doc, '$."1"') =
    CAST('["alpha", "beta"]' AS JSON)
  AND JSON_EXTRACT(@sq_hard_w20_running_doc, '$."2"') =
    CAST('["gamma", "delta", "epsilon"]' AS JSON)
  AND @sq_hard_w20_overlap_rows = 3,
  CONCAT(
    'multi-valued JSON document: ',
    COALESCE(@sq_hard_w20_running_doc, 'NULL'),
    '/',
    COALESCE(@sq_hard_w20_overlap_rows, -1)
  ));
CALL sq_hard_assert(
  (SELECT state_name
   FROM sq_hard_w20_document
   WHERE document_id = 2) = 'review'
  AND JSON_VALUE(
        (SELECT payload
         FROM sq_hard_w20_document
         WHERE document_id = 2),
        '$.extra.priority' RETURNING UNSIGNED
      ) = 9,
  'row-alias upsert');
CALL sq_hard_assert(
  @sq_hard_w20_bag_shape = '1,2',
  CONCAT(
    'table/values multiset: ',
    COALESCE(@sq_hard_w20_bag_shape, 'NULL')
  ));
CALL sq_hard_assert(
  @sq_hard_w20_walk_shape = '1:0/2:1/3:2/4:2/5:3'
  AND @sq_hard_w20_walk_depth = 8
  AND @sq_hard_w20_walk_path_length = 4
  AND (SELECT COUNT(*) FROM sq_hard_w20_note) = 3,
  CONCAT(
    'json cycle guard: ',
    COALESCE(@sq_hard_w20_walk_shape, 'NULL'),
    '/',
    COALESCE(@sq_hard_w20_walk_depth, -1),
    '/',
    COALESCE(@sq_hard_w20_walk_path_length, -1)
  ));

-- ------------------------------------------------------------------------------
-- ULTRA WAVE 21: the ODBC { OJ ... } escape encloses a partition-qualified
-- table, a correlated JSON_TABLE and a second outer join. The same partitioned
-- table is then traversed through MySQL's low-level HANDLER index cursor,
-- including a composite-key equality read guarded by a JSON predicate.
-- ------------------------------------------------------------------------------
CREATE TABLE sq_hard_w21_document (
  document_id INT NOT NULL,
  bucket_id   INT NOT NULL,
  payload     JSON NOT NULL,
  score       INT GENERATED ALWAYS AS (
    JSON_VALUE(
      payload,
      '$.score' RETURNING UNSIGNED DEFAULT 0 ON EMPTY
    )
  ) STORED,
  PRIMARY KEY (document_id, bucket_id)
) ENGINE = InnoDB
PARTITION BY LIST (bucket_id) (
  PARTITION p_hot VALUES IN (1),
  PARTITION p_cold VALUES IN (2)
);

CREATE TABLE sq_hard_w21_weight (
  item_name    VARCHAR(20) NOT NULL PRIMARY KEY,
  weight_value INT NOT NULL
) ENGINE = InnoDB;

CREATE TABLE sq_hard_w21_note (
  note_key   VARCHAR(40) NOT NULL PRIMARY KEY,
  note_text  VARCHAR(1000) NOT NULL,
  note_value BIGINT NOT NULL
) ENGINE = InnoDB;

INSERT INTO sq_hard_w21_weight (item_name, weight_value)
VALUES ('a', 10), ('b', 20), ('c', 30);

INSERT INTO sq_hard_w21_document (document_id, bucket_id, payload)
VALUES
  (
    1,
    1,
    JSON_OBJECT(
      'score', 7,
      'tags', JSON_ARRAY('hot'),
      'items', JSON_ARRAY(
        JSON_OBJECT('name', 'a', 'value', 2),
        JSON_OBJECT('name', 'b', 'value', 3)
      )
    )
  ),
  (
    2,
    2,
    JSON_OBJECT(
      'score', 9,
      'tags', JSON_ARRAY('cold'),
      'items', JSON_ARRAY(
        JSON_OBJECT('name', 'b', 'value', 5),
        JSON_OBJECT('name', 'c', 'value', 7)
      )
    )
  ),
  (
    3,
    1,
    JSON_OBJECT(
      'score', 1,
      'tags', JSON_ARRAY('empty')
    )
  );

-- ------------------------------------------------------------------------------
-- W21-A: braces are SQL grammar, not a client placeholder. JSON_TABLE is
-- implicitly lateral to DOCUMENT_OWNER inside the escape; its missing-path row
-- is null-complemented before the second outer join supplies the weight.
-- ------------------------------------------------------------------------------
SELECT GROUP_CONCAT(
         CONCAT(
           document_id, ':',
           COALESCE(item_no, 0), ':',
           COALESCE(item_name, '_'), ':',
           weighted_value
         )
         ORDER BY document_id, item_no
         SEPARATOR '/'
       ),
       SUM(weighted_value)
INTO @sq_hard_w21_oj_shape,
     @sq_hard_w21_weight_total
FROM (
  SELECT document_owner.document_id,
         item_owner.item_no,
         item_owner.item_name,
         COALESCE(
           item_owner.item_value * weight_owner.weight_value,
           0
         ) AS weighted_value
  FROM { OJ sq_hard_w21_document
              PARTITION (p_hot, p_cold) AS document_owner
         LEFT OUTER JOIN JSON_TABLE(
           document_owner.payload,
           '$.items[*]'
           COLUMNS (
             item_no FOR ORDINALITY,
             item_name VARCHAR(20) PATH '$.name',
             item_value INT PATH '$.value'
           )
         ) AS item_owner
           ON TRUE
         LEFT OUTER JOIN sq_hard_w21_weight AS weight_owner
           ON weight_owner.item_name = item_owner.item_name }
) AS escaped_owner;

INSERT INTO sq_hard_w21_note (note_key, note_text, note_value)
VALUES (
  'odbc-json-outer-join',
  @sq_hard_w21_oj_shape,
  @sq_hard_w21_weight_total
);

-- ------------------------------------------------------------------------------
-- W21-B: HANDLER has its own OPEN/READ/CLOSE statement grammar. The alias is a
-- quoted keyword, PRIMARY is an index owner, FIRST is a cursor direction, and
-- the equality read consumes both parts of the composite partition key.
-- ------------------------------------------------------------------------------
HANDLER sq_hard_w21_document OPEN AS `window`;
HANDLER `window` READ `PRIMARY` FIRST;
HANDLER `window` READ `PRIMARY` = (2, 2)
  WHERE JSON_OVERLAPS(
    payload -> '$.tags',
    JSON_ARRAY('cold')
  )
  LIMIT 1;
HANDLER `window` CLOSE;

SELECT GROUP_CONCAT(
         CONCAT(document_id, ':', score)
         ORDER BY document_id
         SEPARATOR '/'
       )
INTO @sq_hard_w21_handler_shape
FROM sq_hard_w21_document
PARTITION (p_hot, p_cold);

INSERT INTO sq_hard_w21_note (note_key, note_text, note_value)
VALUES (
  'handler-index-cursor',
  @sq_hard_w21_handler_shape,
  (
    SELECT COUNT(*)
    FROM sq_hard_w21_document
    WHERE JSON_OVERLAPS(
      payload -> '$.tags',
      JSON_ARRAY('hot', 'cold')
    )
  )
);

-- ------------------------------------------------------------------------------
-- Wave-21 self-verification.
-- ------------------------------------------------------------------------------
CALL sq_hard_assert(
  @sq_hard_w21_oj_shape =
    '1:1:a:20/1:2:b:60/2:1:b:100/2:2:c:210/3:0:_:0'
  AND @sq_hard_w21_weight_total = 390,
  CONCAT(
    'ODBC JSON outer join: ',
    COALESCE(@sq_hard_w21_oj_shape, 'NULL'),
    '/',
    COALESCE(@sq_hard_w21_weight_total, -1)
  ));
CALL sq_hard_assert(
  @sq_hard_w21_handler_shape = '1:7/2:9/3:1'
  AND (SELECT note_value
       FROM sq_hard_w21_note
       WHERE note_key = 'handler-index-cursor') = 2
  AND (SELECT COUNT(*) FROM sq_hard_w21_note) = 2,
  CONCAT(
    'HANDLER index cursor: ',
    COALESCE(@sq_hard_w21_handler_shape, 'NULL')
  ));

-- ------------------------------------------------------------------------------
-- ULTRA WAVE 22: a temporary table deliberately shadows a permanent table with
-- an incompatible schema, MyISAM's key-cache statements name selected indexes,
-- and an InnoDB file-per-table export lock is read and then explicitly released.
-- These three namespaces all reuse TABLE-like grammar but have different
-- lifetimes, legal engines and follow-up statements.
-- ------------------------------------------------------------------------------
CREATE TABLE sq_hard_w22_scope (
  permanent_id    INT NOT NULL PRIMARY KEY,
  permanent_label VARCHAR(32) NOT NULL,
  payload         JSON NOT NULL
) ENGINE = InnoDB;

CREATE TABLE sq_hard_w22_cache (
  cache_id    INT NOT NULL PRIMARY KEY,
  cache_label VARCHAR(32) NOT NULL,
  payload     VARCHAR(64) NOT NULL,
  KEY ix_cache_label (cache_label)
) ENGINE = MyISAM;

CREATE TABLE sq_hard_w22_export (
  export_id    INT NOT NULL PRIMARY KEY,
  export_label VARCHAR(32) NOT NULL
) ENGINE = InnoDB;

CREATE TABLE sq_hard_w22_note (
  note_key   VARCHAR(40) NOT NULL PRIMARY KEY,
  note_text  VARCHAR(1000) NOT NULL,
  note_value BIGINT NOT NULL
) ENGINE = InnoDB;

-- ------------------------------------------------------------------------------
-- W22-A: the temporary relation has the exact qualified name of the permanent
-- relation, so name resolution must switch to a generated-column schema and
-- switch back after DROP TEMPORARY TABLE.
-- ------------------------------------------------------------------------------
INSERT INTO sq_hard_w22_scope (permanent_id, permanent_label, payload)
VALUES
  (
    1,
    'permanent-alpha',
    JSON_OBJECT('kind', 'base', 'n', 1)
  ),
  (
    2,
    'permanent-beta',
    JSON_OBJECT('kind', 'base', 'n', 2)
  );

SELECT GROUP_CONCAT(
         CONCAT(
           permanent_id, ':',
           permanent_label, ':',
           payload ->> '$.kind'
         )
         ORDER BY permanent_id
         SEPARATOR '/'
       )
INTO @sq_hard_w22_permanent_before
FROM sq_hard_w22_scope;

CREATE TEMPORARY TABLE sq_hard_w22_scope (
  temporary_id INT NOT NULL PRIMARY KEY,
  source_label VARCHAR(32) NOT NULL,
  squared      INT GENERATED ALWAYS AS (
    temporary_id * temporary_id
  ) STORED
) ENGINE = InnoDB;

INSERT INTO sq_hard_w22_scope (temporary_id, source_label)
VALUES (7, 'select'), (42, 'from');

SELECT GROUP_CONCAT(
         CONCAT(temporary_id, ':', source_label, ':', squared)
         ORDER BY temporary_id
         SEPARATOR '/'
       )
INTO @sq_hard_w22_temporary_shape
FROM sq_hard_w22_scope;

DROP TEMPORARY TABLE sq_hard_w22_scope;

SELECT GROUP_CONCAT(
         CONCAT(
           permanent_id, ':',
           permanent_label, ':',
           payload ->> '$.kind'
         )
         ORDER BY permanent_id
         SEPARATOR '/'
       )
INTO @sq_hard_w22_permanent_after
FROM sq_hard_w22_scope;

INSERT INTO sq_hard_w22_note (note_key, note_text, note_value)
VALUES (
  'temporary-shadow',
  CONCAT(
    @sq_hard_w22_permanent_before,
    ' -> ',
    @sq_hard_w22_temporary_shape,
    ' -> ',
    @sq_hard_w22_permanent_after
  ),
  (SELECT COUNT(*) FROM sq_hard_w22_scope)
);

-- ------------------------------------------------------------------------------
-- W22-B: CACHE INDEX assigns only the named MyISAM indexes to the quoted
-- default key cache; LOAD INDEX INTO CACHE then preloads their non-leaf blocks.
-- ------------------------------------------------------------------------------
INSERT INTO sq_hard_w22_cache (cache_id, cache_label, payload)
VALUES
  (1, 'alpha', 'window'),
  (2, 'beta', 'qualify'),
  (3, 'gamma', 'recursive');

CACHE INDEX sq_hard_w22_cache
  KEY (PRIMARY, ix_cache_label)
  IN `default`;

LOAD INDEX INTO CACHE sq_hard_w22_cache
  KEY (PRIMARY, ix_cache_label)
  IGNORE LEAVES;

SELECT GROUP_CONCAT(
         CONCAT(cache_id, ':', cache_label, ':', payload)
         ORDER BY cache_id
         SEPARATOR '/'
       )
INTO @sq_hard_w22_cache_shape
FROM sq_hard_w22_cache;

INSERT INTO sq_hard_w22_note (note_key, note_text, note_value)
VALUES (
  'myisam-key-cache',
  @sq_hard_w22_cache_shape,
  (SELECT COUNT(*) FROM sq_hard_w22_cache)
);

-- ------------------------------------------------------------------------------
-- W22-C: FOR EXPORT freezes a committed InnoDB snapshot for external copying.
-- The table stays readable while locked; the post-UNLOCK insert proves that
-- ordinary write grammar and transaction state were restored.
-- ------------------------------------------------------------------------------
INSERT INTO sq_hard_w22_export (export_id, export_label)
VALUES (1, 'locked'), (2, 'snapshot');
COMMIT;

FLUSH TABLES sq_hard_w22_export FOR EXPORT;

SELECT GROUP_CONCAT(
         CONCAT(export_id, ':', export_label)
         ORDER BY export_id
         SEPARATOR '/'
       )
INTO @sq_hard_w22_export_locked
FROM sq_hard_w22_export;

UNLOCK TABLES;

INSERT INTO sq_hard_w22_export (export_id, export_label)
VALUES (3, 'released');
COMMIT;

SELECT GROUP_CONCAT(
         CONCAT(export_id, ':', export_label)
         ORDER BY export_id
         SEPARATOR '/'
       )
INTO @sq_hard_w22_export_released
FROM sq_hard_w22_export;

INSERT INTO sq_hard_w22_note (note_key, note_text, note_value)
VALUES (
  'innodb-export-lock',
  CONCAT(
    @sq_hard_w22_export_locked,
    ' -> ',
    @sq_hard_w22_export_released
  ),
  (SELECT COUNT(*) FROM sq_hard_w22_export)
);

-- ------------------------------------------------------------------------------
-- Wave-22 self-verification.
-- ------------------------------------------------------------------------------
CALL sq_hard_assert(
  @sq_hard_w22_permanent_before =
    '1:permanent-alpha:base/2:permanent-beta:base'
  AND @sq_hard_w22_temporary_shape =
    '7:select:49/42:from:1764'
  AND @sq_hard_w22_permanent_after =
    @sq_hard_w22_permanent_before,
  CONCAT(
    'temporary shadow: ',
    COALESCE(@sq_hard_w22_permanent_before, 'NULL'),
    '/',
    COALESCE(@sq_hard_w22_temporary_shape, 'NULL'),
    '/',
    COALESCE(@sq_hard_w22_permanent_after, 'NULL')
  ));
CALL sq_hard_assert(
  @sq_hard_w22_cache_shape =
    '1:alpha:window/2:beta:qualify/3:gamma:recursive',
  CONCAT(
    'MyISAM key cache: ',
    COALESCE(@sq_hard_w22_cache_shape, 'NULL')
  ));
CALL sq_hard_assert(
  @sq_hard_w22_export_locked =
    '1:locked/2:snapshot'
  AND @sq_hard_w22_export_released =
    '1:locked/2:snapshot/3:released'
  AND (SELECT COUNT(*) FROM sq_hard_w22_note) = 3,
  CONCAT(
    'InnoDB export lock: ',
    COALESCE(@sq_hard_w22_export_locked, 'NULL'),
    '/',
    COALESCE(@sq_hard_w22_export_released, 'NULL')
  ));

-- ------------------------------------------------------------------------------
-- ULTRA WAVE 23: one atomic RENAME TABLE rotates incompatible relation shapes
-- across schemas. The auxiliary schema is then made READ ONLY while a qualified
-- temporary table legally shadows its permanent namesake. Finally, TABLE is
-- used both as a statement and as a subquery inside an altered view whose JSON
-- rows participate in a NATURAL JOIN. Completion must track object identity,
-- schema state, view shape and temporary-name precedence without conflating any
-- of their identically spelled TABLE-like tokens.
-- ------------------------------------------------------------------------------
CREATE TABLE sq_hard_w23_note (
  note_key   VARCHAR(40) NOT NULL PRIMARY KEY,
  note_text  VARCHAR(1000) NOT NULL,
  note_value BIGINT NOT NULL
) ENGINE = InnoDB;

CREATE TABLE sq_hard_w23_left (
  left_id    INT NOT NULL PRIMARY KEY,
  left_label VARCHAR(32) NOT NULL,
  payload    JSON NOT NULL
) ENGINE = InnoDB;

CREATE TABLE sq_hard_mysql_aux.sq_hard_w23_right (
  right_id    INT NOT NULL PRIMARY KEY,
  right_label VARCHAR(32) NOT NULL,
  payload     JSON NOT NULL DEFAULT (
    JSON_OBJECT(
      'kind', 'right',
      'score', 0,
      'tags', JSON_ARRAY()
    )
  ),
  score_value INT GENERATED ALWAYS AS (
    CAST(payload ->> '$.score' AS UNSIGNED)
  ) STORED INVISIBLE
) ENGINE = InnoDB;

INSERT INTO sq_hard_w23_left (left_id, left_label, payload)
VALUES
  (1, 'left-alpha', JSON_OBJECT('kind', 'left', 'n', 1)),
  (2, 'left-beta', JSON_OBJECT('kind', 'left', 'n', 2));

INSERT INTO sq_hard_mysql_aux.sq_hard_w23_right (
  right_id,
  right_label,
  payload
)
VALUES
  (
    10,
    'right-parser',
    JSON_OBJECT(
      'kind', 'right',
      'score', 10,
      'tags', JSON_ARRAY('sql', 'parser')
    )
  ),
  (
    20,
    'right-format',
    JSON_OBJECT(
      'kind', 'right',
      'score', 20,
      'tags', JSON_ARRAY('sql', 'format')
    )
  );

-- ------------------------------------------------------------------------------
-- W23-A: all three rename clauses commit atomically. The unqualified name in
-- the current schema now has right_* columns, while the identically suffixed
-- auxiliary object has left_* columns and the original left-side data.
-- ------------------------------------------------------------------------------
RENAME TABLE
  sq_hard_mysql.sq_hard_w23_left
    TO sq_hard_mysql_aux.sq_hard_w23_stage,
  sq_hard_mysql_aux.sq_hard_w23_right
    TO sq_hard_mysql.sq_hard_w23_left,
  sq_hard_mysql_aux.sq_hard_w23_stage
    TO sq_hard_mysql_aux.sq_hard_w23_right;

SELECT GROUP_CONCAT(
         column_name
         ORDER BY ordinal_position
         SEPARATOR '/'
       )
INTO @sq_hard_w23_main_columns
FROM information_schema.columns
WHERE table_schema = 'sq_hard_mysql'
  AND table_name = 'sq_hard_w23_left';

SELECT GROUP_CONCAT(
         column_name
         ORDER BY ordinal_position
         SEPARATOR '/'
       )
INTO @sq_hard_w23_aux_columns
FROM information_schema.columns
WHERE table_schema = 'sq_hard_mysql_aux'
  AND table_name = 'sq_hard_w23_right';

SELECT GROUP_CONCAT(
         CONCAT(
           right_id, ':',
           right_label, ':',
           score_value
         )
         ORDER BY right_id
         SEPARATOR '/'
       )
INTO @sq_hard_w23_main_shape
FROM sq_hard_w23_left;

SELECT GROUP_CONCAT(
         CONCAT(
           left_id, ':',
           left_label, ':',
           payload ->> '$.kind'
         )
         ORDER BY left_id
         SEPARATOR '/'
       )
INTO @sq_hard_w23_permanent_before
FROM sq_hard_mysql_aux.sq_hard_w23_right;

INSERT INTO sq_hard_w23_note (note_key, note_text, note_value)
VALUES (
  'atomic-schema-rotation',
  CONCAT(
    @sq_hard_w23_main_columns,
    ' | ',
    @sq_hard_w23_aux_columns,
    ' | ',
    @sq_hard_w23_main_shape,
    ' | ',
    @sq_hard_w23_permanent_before
  ),
  (
    SELECT COUNT(*)
    FROM information_schema.columns
    WHERE table_schema IN ('sq_hard_mysql', 'sq_hard_mysql_aux')
      AND table_name IN ('sq_hard_w23_left', 'sq_hard_w23_right')
  )
);

-- ------------------------------------------------------------------------------
-- W23-B: a READ ONLY schema still permits a TEMPORARY relation. It deliberately
-- takes the exact qualified name of the permanent rotated table but exposes a
-- generated/invisible score instead of left_* columns. Dropping it restores the
-- permanent binding before ALTER DATABASE changes the schema back to writable.
-- ------------------------------------------------------------------------------
ALTER SCHEMA sq_hard_mysql_aux READ ONLY = 1;

SELECT options
INTO @sq_hard_w23_read_only_options
FROM information_schema.schemata_extensions
WHERE catalog_name = 'def'
  AND schema_name = 'sq_hard_mysql_aux';

CREATE TEMPORARY TABLE sq_hard_mysql_aux.sq_hard_w23_right (
  temporary_id INT NOT NULL PRIMARY KEY,
  source_label VARCHAR(32) NOT NULL,
  payload      JSON NOT NULL,
  score_value  INT GENERATED ALWAYS AS (
    temporary_id * temporary_id
  ) STORED INVISIBLE
) ENGINE = InnoDB;

INSERT INTO sq_hard_mysql_aux.sq_hard_w23_right (
  temporary_id,
  source_label,
  payload
)
VALUES
  (
    7,
    'select',
    JSON_OBJECT('scope', 'temporary', 'token', '/* not comment */')
  ),
  (
    42,
    'from',
    JSON_OBJECT('scope', 'temporary', 'token', '-- not comment')
  );

SELECT GROUP_CONCAT(
         CONCAT(
           temporary_id, ':',
           source_label, ':',
           score_value
         )
         ORDER BY temporary_id
         SEPARATOR '/'
       )
INTO @sq_hard_w23_temporary_shape
FROM sq_hard_mysql_aux.sq_hard_w23_right;

DROP TEMPORARY TABLE sq_hard_mysql_aux.sq_hard_w23_right;

SELECT GROUP_CONCAT(
         CONCAT(
           left_id, ':',
           left_label, ':',
           payload ->> '$.kind'
         )
         ORDER BY left_id
         SEPARATOR '/'
       )
INTO @sq_hard_w23_permanent_after
FROM sq_hard_mysql_aux.sq_hard_w23_right;

ALTER DATABASE sq_hard_mysql_aux
  READ ONLY = 0
  DEFAULT COLLATE = utf8mb4_0900_ai_ci;

SELECT COALESCE(options, '')
INTO @sq_hard_w23_writable_options
FROM information_schema.schemata_extensions
WHERE catalog_name = 'def'
  AND schema_name = 'sq_hard_mysql_aux';

INSERT INTO sq_hard_w23_note (note_key, note_text, note_value)
VALUES (
  'read-only-temporary-shadow',
  CONCAT(
    @sq_hard_w23_permanent_before,
    ' -> ',
    @sq_hard_w23_temporary_shape,
    ' -> ',
    @sq_hard_w23_permanent_after
  ),
  @sq_hard_w23_read_only_options = 'READ ONLY=1'
    AND @sq_hard_w23_writable_options = ''
);

-- ------------------------------------------------------------------------------
-- W23-C: the first view is updatable and inserts a row whose hidden generated
-- score depends on a base-table JSON default. ALTER VIEW then changes its column
-- shape and security/algorithm clauses. Two TABLE subqueries filter the view;
-- the standalone TABLE statement exposes it; JSON_TABLE, a VALUES constructor
-- and NATURAL JOIN finish the same relation with three independent row sources.
-- ------------------------------------------------------------------------------
CREATE TABLE sq_hard_w23_allowed (
  item_id INT NOT NULL PRIMARY KEY
) ENGINE = InnoDB;

INSERT INTO sq_hard_w23_allowed (item_id)
VALUES (10), (20), (30);

CREATE ALGORITHM = MERGE
SQL SECURITY INVOKER
VIEW sq_hard_w23_item_v (item_id, item_label) AS
SELECT right_id, right_label
FROM sq_hard_w23_left
WHERE right_id BETWEEN 10 AND 30
WITH CASCADED CHECK OPTION;

INSERT INTO sq_hard_w23_item_v (item_id, item_label)
VALUES (30, 'right-empty');

ALTER ALGORITHM = UNDEFINED
SQL SECURITY DEFINER
VIEW sq_hard_w23_item_v (
  item_id,
  item_label,
  payload,
  score_value
) AS
SELECT right_id, right_label, payload, score_value
FROM sq_hard_w23_left
WHERE right_id IN (TABLE sq_hard_w23_allowed)
  AND right_id >= ANY (TABLE sq_hard_w23_allowed);

TABLE sq_hard_w23_item_v ORDER BY item_id;

WITH expanded AS (
  SELECT
    view_owner.item_id,
    view_owner.item_label,
    view_owner.score_value,
    weight_owner.weight_value,
    tag_owner.tag_no,
    tag_owner.tag_value
  FROM sq_hard_w23_item_v AS view_owner
  LEFT JOIN JSON_TABLE(
    JSON_EXTRACT(view_owner.payload, '$'),
    '$.tags[*]' COLUMNS (
      tag_no FOR ORDINALITY,
      tag_value VARCHAR(32) PATH '$'
    )
  ) AS tag_owner ON TRUE
  NATURAL JOIN (
    VALUES
      ROW(10, 2),
      ROW(20, 3),
      ROW(30, 5)
  ) AS weight_owner(item_id, weight_value)
),
collapsed AS (
  SELECT
    item_id,
    item_label,
    score_value,
    weight_value,
    COALESCE(
      GROUP_CONCAT(
        tag_value
        ORDER BY tag_no
        SEPARATOR ','
      ),
      '_'
    ) AS tag_shape,
    score_value * weight_value AS weighted_score
  FROM expanded
  GROUP BY item_id, item_label, score_value, weight_value
)
SELECT
  GROUP_CONCAT(
    CONCAT_WS(
      ':',
      item_id,
      item_label,
      score_value,
      weight_value,
      tag_shape
    )
    ORDER BY item_id
    SEPARATOR '/'
  ),
  SUM(weighted_score)
INTO
  @sq_hard_w23_view_shape,
  @sq_hard_w23_weighted_total
FROM collapsed;

SELECT GROUP_CONCAT(
         column_name
         ORDER BY ordinal_position
         SEPARATOR '/'
       )
INTO @sq_hard_w23_view_columns
FROM information_schema.columns
WHERE table_schema = 'sq_hard_mysql'
  AND table_name = 'sq_hard_w23_item_v';

INSERT INTO sq_hard_w23_note (note_key, note_text, note_value)
VALUES (
  'altered-view-table-subquery',
  CONCAT(
    @sq_hard_w23_view_columns,
    ' | ',
    @sq_hard_w23_view_shape
  ),
  @sq_hard_w23_weighted_total
);

-- ------------------------------------------------------------------------------
-- Wave-23 self-verification.
-- ------------------------------------------------------------------------------
CALL sq_hard_assert(
  @sq_hard_w23_main_columns =
    'right_id/right_label/payload/score_value'
  AND @sq_hard_w23_aux_columns =
    'left_id/left_label/payload'
  AND @sq_hard_w23_main_shape =
    '10:right-parser:10/20:right-format:20'
  AND @sq_hard_w23_permanent_before =
    '1:left-alpha:left/2:left-beta:left',
  CONCAT(
    'atomic schema rotation: ',
    COALESCE(@sq_hard_w23_main_columns, 'NULL'),
    '/',
    COALESCE(@sq_hard_w23_aux_columns, 'NULL'),
    '/',
    COALESCE(@sq_hard_w23_main_shape, 'NULL'),
    '/',
    COALESCE(@sq_hard_w23_permanent_before, 'NULL')
  ));
CALL sq_hard_assert(
  @sq_hard_w23_read_only_options = 'READ ONLY=1'
  AND @sq_hard_w23_writable_options = ''
  AND @sq_hard_w23_temporary_shape =
    '7:select:49/42:from:1764'
  AND @sq_hard_w23_permanent_after =
    @sq_hard_w23_permanent_before,
  CONCAT(
    'read-only temporary shadow: ',
    COALESCE(@sq_hard_w23_read_only_options, 'NULL'),
    '/',
    COALESCE(@sq_hard_w23_writable_options, 'NULL'),
    '/',
    COALESCE(@sq_hard_w23_temporary_shape, 'NULL'),
    '/',
    COALESCE(@sq_hard_w23_permanent_after, 'NULL')
  ));
CALL sq_hard_assert(
  @sq_hard_w23_view_columns =
    'item_id/item_label/payload/score_value'
  AND @sq_hard_w23_view_shape =
    '10:right-parser:10:2:sql,parser/20:right-format:20:3:sql,format/30:right-empty:0:5:_'
  AND @sq_hard_w23_weighted_total = 80
  AND (SELECT COUNT(*) FROM sq_hard_w23_note) = 3,
  CONCAT(
    'altered view/table subquery: ',
    COALESCE(@sq_hard_w23_view_columns, 'NULL'),
    '/',
    COALESCE(@sq_hard_w23_view_shape, 'NULL'),
    '/',
    COALESCE(@sq_hard_w23_weighted_total, -1)
  ));

-- ------------------------------------------------------------------------------
-- ULTRA WAVE 24 -- query-expression ownership, explicit analytic-null clauses,
-- and the statement-level HANDLER grammar collide in one final parser gauntlet.
-- Every construct below is legal on the pinned MySQL 8.0 target and has an
-- independently asserted result, despite reusing words such as TABLE, WINDOW,
-- READ, FIRST, LAST, and HANDLER in radically different syntactic roles.
-- ------------------------------------------------------------------------------
CREATE TABLE sq_hard_w24_note (
  note_key   VARCHAR(40) NOT NULL PRIMARY KEY,
  note_text  VARCHAR(1200) NOT NULL,
  note_value BIGINT NOT NULL
) ENGINE = InnoDB;

-- ------------------------------------------------------------------------------
-- W24-A: each parenthesized query-expression owns its own ORDER BY/LIMIT.
-- TABLE, VALUES, and SELECT are set operands at different depths; INTERSECT and
-- EXCEPT retain their precedence before the outer UNION. Only the outermost
-- expression owns INTO, after its LIMIT/OFFSET, so a formatter that moves even
-- one clause changes the selected row.
-- ------------------------------------------------------------------------------
CREATE TABLE sq_hard_w24_atom (
  atom_id   INT NOT NULL PRIMARY KEY,
  atom_text VARCHAR(40) NOT NULL
) ENGINE = InnoDB;

INSERT INTO sq_hard_w24_atom (atom_id, atom_text)
VALUES
  (1, 'one'),
  (2, 'two'),
  (3, 'three'),
  (4, 'four'),
  (5, 'five'),
  (6, 'six;--/*value*/#'),
  (7, 'seven');

SET @sq_hard_w24_picked_id = NULL,
    @sq_hard_w24_picked_text = NULL;

(
  (
    (
      TABLE sq_hard_w24_atom
      ORDER BY atom_id DESC
      LIMIT 6
    )
    INTERSECT DISTINCT
    (
      VALUES
        ROW(1, 'one'),
        ROW(2, 'two'),
        ROW(3, 'three'),
        ROW(4, 'four'),
        ROW(5, 'five'),
        ROW(6, 'six;--/*value*/#')
      ORDER BY column_0 DESC
      LIMIT 6
    )
    ORDER BY atom_id DESC
    LIMIT 5
  )
  EXCEPT DISTINCT
  (
    SELECT atom_id,
           atom_text
    FROM sq_hard_w24_atom
    WHERE atom_id IN (2, 5)
    ORDER BY atom_id
    LIMIT 2
  )
  ORDER BY atom_id ASC
  LIMIT 3
)
UNION ALL
(
  VALUES ROW(99, 'sentinel')
  ORDER BY column_0 DESC
  LIMIT 1
)
ORDER BY atom_id DESC
LIMIT 1 OFFSET 1
INTO
  @sq_hard_w24_picked_id,
  @sq_hard_w24_picked_text;

SET @sq_hard_w24_query_shape = CONCAT(
  @sq_hard_w24_picked_id,
  ':',
  @sq_hard_w24_picked_text
);

INSERT INTO sq_hard_w24_note (note_key, note_text, note_value)
VALUES (
  'nested-query-expression',
  @sq_hard_w24_query_shape,
  @sq_hard_w24_picked_id
);

-- ------------------------------------------------------------------------------
-- W24-B: explicit standard options that are often mis-highlighted as aliases.
-- FROM FIRST belongs to NTH_VALUE, RESPECT NULLS belongs to the value function,
-- and each OVER reference resolves through an inherited named-window chain.
-- Duplicate JSON object keys make frame order observable while JSON null and
-- SQL NULL cross the analytic/JSON boundary in opposite directions.
-- ------------------------------------------------------------------------------
CREATE TABLE sq_hard_w24_stream_event (
  stream_id       INT NOT NULL,
  sequence_no     INT NOT NULL,
  attribute_name  VARCHAR(16) NOT NULL,
  attribute_value VARCHAR(16) NULL,
  score_value     INT NOT NULL,
  PRIMARY KEY (stream_id, sequence_no)
) ENGINE = InnoDB;

INSERT INTO sq_hard_w24_stream_event (
  stream_id,
  sequence_no,
  attribute_name,
  attribute_value,
  score_value
)
VALUES
  (1, 1, 'status', NULL,     5),
  (1, 2, 'owner',  'ada',    8),
  (1, 3, 'status', 'ready', 13),
  (1, 4, 'mode',   'strict', 21),
  (2, 1, 'status', 'new',    3),
  (2, 2, 'owner',  NULL,     4),
  (2, 3, 'mode',   'safe',   7);

WITH analytic AS (
  SELECT
    stream_id,
    sequence_no,
    FIRST_VALUE(attribute_value)
      RESPECT NULLS OVER w_full AS first_respected,
    NTH_VALUE(attribute_value, 2)
      FROM FIRST RESPECT NULLS OVER w_full AS second_respected,
    LAST_VALUE(attribute_value)
      RESPECT NULLS OVER w_full AS last_respected,
    JSON_OBJECTAGG(attribute_name, attribute_value)
      OVER w_running AS running_object,
    JSON_ARRAYAGG(
      JSON_OBJECT(
        'n', sequence_no,
        'v', attribute_value,
        'lexer', 'semi;--/*json*/#'
      )
    ) OVER w_running AS running_array,
    ROW_NUMBER() OVER w_reverse AS reverse_no
  FROM sq_hard_w24_stream_event
  WINDOW
    w_partition AS (
      PARTITION BY stream_id
    ),
    w_ordered AS (
      w_partition
      ORDER BY sequence_no
    ),
    w_running AS (
      w_ordered
      ROWS BETWEEN UNBOUNDED PRECEDING AND CURRENT ROW
    ),
    w_full AS (
      w_ordered
      ROWS BETWEEN UNBOUNDED PRECEDING AND UNBOUNDED FOLLOWING
    ),
    w_reverse AS (
      w_partition
      ORDER BY sequence_no DESC
    )
)
SELECT
  GROUP_CONCAT(
    CONCAT(
      stream_id,
      ':',
      COALESCE(first_respected, '<NULL>'),
      ':',
      COALESCE(second_respected, '<NULL>'),
      ':',
      COALESCE(last_respected, '<NULL>'),
      ':',
      COALESCE(
        JSON_UNQUOTE(JSON_EXTRACT(running_object, '$.status')),
        '<NULL>'
      ),
      ':',
      JSON_LENGTH(running_array)
    )
    ORDER BY stream_id
    SEPARATOR '/'
  ),
  SUM(JSON_LENGTH(running_array))
INTO
  @sq_hard_w24_window_shape,
  @sq_hard_w24_window_total
FROM analytic
WHERE reverse_no = 1;

INSERT INTO sq_hard_w24_note (note_key, note_text, note_value)
VALUES (
  'explicit-null-window',
  @sq_hard_w24_window_shape,
  @sq_hard_w24_window_total
);

-- ------------------------------------------------------------------------------
-- W24-C: this HANDLER is a top-level storage-engine cursor, not a declared
-- condition handler. The alias `window` and index `read` are quoted keywords.
-- FIRST/NEXT/LAST change from analytic tokens into index-navigation commands;
-- the composite range then attaches WHERE and LIMIT to the HANDLER READ itself.
-- ------------------------------------------------------------------------------
CREATE TABLE sq_hard_w24_handler_source (
  event_id    INT NOT NULL PRIMARY KEY,
  event_kind  VARCHAR(16) NOT NULL,
  score_value INT NOT NULL,
  payload     JSON NOT NULL,
  KEY `read` (event_kind, score_value DESC)
) ENGINE = InnoDB;

INSERT INTO sq_hard_w24_handler_source (
  event_id,
  event_kind,
  score_value,
  payload
)
VALUES
  (10, 'format', 8,  JSON_OBJECT('token', 'semi;--/*json*/#')),
  (20, 'format', 13, JSON_OBJECT('token', 'window')),
  (30, 'parser', 21, JSON_OBJECT('token', 'handler')),
  (40, 'parser', 34, JSON_OBJECT('token', 'close'));

HANDLER sq_hard_w24_handler_source OPEN AS `window`;
HANDLER `window` READ `PRIMARY` FIRST;
HANDLER `window` READ `PRIMARY` NEXT;
HANDLER `window` READ `read` >= ('format', 13)
  WHERE score_value BETWEEN 10 AND 40
    AND payload ->> '$.token' IS NOT NULL
  LIMIT 2;
HANDLER `window` READ `read` LAST;
HANDLER `window` CLOSE;

SELECT
  GROUP_CONCAT(
    CONCAT(
      event_id,
      ':',
      event_kind,
      ':',
      score_value,
      ':',
      payload ->> '$.token'
    )
    ORDER BY event_id
    SEPARATOR '/'
  ),
  SUM(score_value)
INTO
  @sq_hard_w24_handler_shape,
  @sq_hard_w24_handler_total
FROM sq_hard_w24_handler_source;

INSERT INTO sq_hard_w24_note (note_key, note_text, note_value)
VALUES (
  'statement-handler',
  @sq_hard_w24_handler_shape,
  @sq_hard_w24_handler_total
);

-- ------------------------------------------------------------------------------
-- Wave-24 self-verification.
-- ------------------------------------------------------------------------------
CALL sq_hard_assert(
  @sq_hard_w24_picked_id = 6
  AND @sq_hard_w24_picked_text = 'six;--/*value*/#'
  AND @sq_hard_w24_query_shape = '6:six;--/*value*/#',
  CONCAT(
    'nested query expression: ',
    COALESCE(@sq_hard_w24_query_shape, 'NULL')
  ));
CALL sq_hard_assert(
  @sq_hard_w24_window_shape =
    '1:<NULL>:ada:strict:ready:4/2:new:<NULL>:safe:new:3'
  AND @sq_hard_w24_window_total = 7,
  CONCAT(
    'explicit null window: ',
    COALESCE(@sq_hard_w24_window_shape, 'NULL'),
    '/',
    COALESCE(@sq_hard_w24_window_total, -1)
  ));
CALL sq_hard_assert(
  @sq_hard_w24_handler_shape =
    '10:format:8:semi;--/*json*/#/20:format:13:window/30:parser:21:handler/40:parser:34:close'
  AND @sq_hard_w24_handler_total = 76
  AND (SELECT COUNT(*) FROM sq_hard_w24_note) = 3,
  CONCAT(
    'statement handler: ',
    COALESCE(@sq_hard_w24_handler_shape, 'NULL'),
    '/',
    COALESCE(@sq_hard_w24_handler_total, -1)
  ));

-- ------------------------------------------------------------------------------
-- ULTRA WAVE 25 -- DISTINCT-spatial-window, operator-function and recursive
-- JSON/LATERAL geometry singularity.
--
-- MySQL's ST_COLLECT is one of the rare aggregates that accepts DISTINCT and
-- OVER together. Geometry values then cross interpolation, similarity,
-- validation, GeoHash and SRS-transform signatures before a recursive CTE,
-- JSON_TABLE and LATERAL derived table mint a second set of spatial window
-- columns. The same aliases change owners across JSON paths, CTEs, windows,
-- spatial constructors and optimizer output while every result stays exact.
-- ------------------------------------------------------------------------------
CREATE TABLE sq_hard_w25_note (
  note_key   VARCHAR(40) NOT NULL PRIMARY KEY,
  note_text  VARCHAR(1200) NOT NULL,
  note_value BIGINT NOT NULL
) ENGINE = InnoDB;

CREATE TABLE sq_hard_w25_point (
  route_id  INT NOT NULL,
  point_id  INT NOT NULL,
  position  POINT SRID 0 NOT NULL,
  payload   JSON NOT NULL,
  PRIMARY KEY (route_id, point_id),
  CONSTRAINT ck_sq_hard_w25_point_payload
    CHECK (JSON_TYPE(payload) = 'OBJECT')
) ENGINE = InnoDB;

CREATE TABLE sq_hard_w25_route (
  route_id    INT NOT NULL PRIMARY KEY,
  route_name  VARCHAR(30) NOT NULL UNIQUE,
  route_path  LINESTRING SRID 0 NOT NULL,
  sample_plan JSON NOT NULL,
  CONSTRAINT ck_sq_hard_w25_route_plan
    CHECK (JSON_TYPE(sample_plan) = 'ARRAY')
) ENGINE = InnoDB;

INSERT INTO sq_hard_w25_point (
  route_id,
  point_id,
  position,
  payload
)
VALUES
  (
    1, 1, ST_GeomFromText('POINT(0 0)', 0),
    JSON_OBJECT('token', 'start;--/*point*/#')
  ),
  (
    1, 2, ST_GeomFromText('POINT(0 0)', 0),
    JSON_OBJECT('token', 'duplicate')
  ),
  (
    1, 3, ST_GeomFromText('POINT(0 5)', 0),
    JSON_OBJECT('token', 'corner')
  ),
  (
    1, 4, ST_GeomFromText('POINT(5 5)', 0),
    JSON_OBJECT('token', 'finish')
  ),
  (
    2, 1, ST_GeomFromText('POINT(1 0)', 0),
    JSON_OBJECT('token', 'left')
  ),
  (
    2, 2, ST_GeomFromText('POINT(1 0)', 0),
    JSON_OBJECT('token', 'left-again')
  ),
  (
    2, 3, ST_GeomFromText('POINT(2 0)', 0),
    JSON_OBJECT('token', 'right')
  );

INSERT INTO sq_hard_w25_route (
  route_id,
  route_name,
  route_path,
  sample_plan
)
VALUES
  (
    1,
    'elbow',
    ST_GeomFromText('LINESTRING(0 0,0 5,5 5)', 0),
    JSON_ARRAY(
      JSON_OBJECT('fraction', 0.25, 'label', 'quarter'),
      JSON_OBJECT('fraction', 0.50, 'label', 'half'),
      JSON_OBJECT('fraction', 0.75, 'label', 'three-quarter'),
      JSON_OBJECT('fraction', 1.00, 'label', 'finish')
    )
  ),
  (
    2,
    'straight',
    ST_GeomFromText('LINESTRING(0 0,4 0)', 0),
    JSON_ARRAY(
      JSON_OBJECT('fraction', 0.25, 'label', 'quarter'),
      JSON_OBJECT('fraction', 0.50, 'label', 'half'),
      JSON_OBJECT('fraction', 1.00, 'label', 'finish')
    )
  );

-- ------------------------------------------------------------------------------
-- W25-A: the two ST_COLLECT calls share one inherited named window, but only
-- one owns DISTINCT. The terminal row therefore exposes both duplicate-aware
-- and set-like geometry cardinalities without GROUP BY collapsing row scope.
-- ------------------------------------------------------------------------------
WITH spatial_window AS (
  SELECT point_owner.route_id,
         point_owner.point_id,
         ST_Collect(point_owner.position) OVER w_running AS all_points,
         ST_Collect(DISTINCT point_owner.position)
           OVER w_running AS distinct_points,
         ROW_NUMBER() OVER (
           PARTITION BY point_owner.route_id
           ORDER BY point_owner.point_id DESC
         ) AS reverse_position
  FROM sq_hard_w25_point AS point_owner
  WINDOW
    w_route AS (
      PARTITION BY point_owner.route_id
    ),
    w_running AS (
      w_route
      ORDER BY point_owner.point_id
      ROWS BETWEEN UNBOUNDED PRECEDING AND CURRENT ROW
    )
),
terminal_window AS (
  SELECT route_id,
         all_points,
         distinct_points
  FROM spatial_window
  WHERE reverse_position = 1
)
SELECT GROUP_CONCAT(
         CONCAT(
           terminal_owner.route_id,
           ':',
           ST_AsText(terminal_owner.distinct_points),
           ':',
           ST_NumGeometries(terminal_owner.all_points)
         )
         ORDER BY terminal_owner.route_id
         SEPARATOR '/'
       ),
       SUM(ST_NumGeometries(terminal_owner.distinct_points))
INTO @sq_hard_w25_window_shape,
     @sq_hard_w25_window_total
FROM terminal_window AS terminal_owner;

INSERT INTO sq_hard_w25_note (note_key, note_text, note_value)
VALUES (
  'distinct-spatial-window',
  @sq_hard_w25_window_shape,
  @sq_hard_w25_window_total
);

-- ------------------------------------------------------------------------------
-- W25-B: every adjacent call has a different spatial contract. Fréchet and
-- Hausdorff compare whole lines; interpolation returns POINT/MULTIPOINT;
-- simplification and validation return geometries; GeoHash crosses string and
-- POINT domains; ST_TRANSFORM changes projected SRS twice before geographic
-- longitude/latitude accessors read the round trip.
-- ------------------------------------------------------------------------------
SET @sq_hard_w25_line_a = ST_GeomFromText(
  'LINESTRING(0 0,0 5,5 5)',
  0
);
SET @sq_hard_w25_line_b = ST_GeomFromText(
  'LINESTRING(0 1,0 6,3 3,5 6)',
  0
);
SET @sq_hard_w25_wiggle = ST_GeomFromText(
  'LINESTRING(0 0,1 0.01,2 -0.01,3 0)',
  0
);
SET @sq_hard_w25_hash = ST_GeoHash(
  ST_GeomFromText('POINT(1 1)', 0),
  15
);
SET @sq_hard_w25_geographic = ST_GeomFromText(
  'POINT(127.0276 37.4979)',
  4326,
  'axis-order=long-lat'
);
SET @sq_hard_w25_projected = ST_Transform(
  @sq_hard_w25_geographic,
  3857
);
SET @sq_hard_w25_roundtrip = ST_Transform(
  @sq_hard_w25_projected,
  4326
);

SELECT CONCAT(
         'frechet=',
         ROUND(
           ST_FrechetDistance(
             @sq_hard_w25_line_a,
             @sq_hard_w25_line_b
           ),
           6
         ),
         '|hausdorff=',
         ROUND(
           ST_HausdorffDistance(
             @sq_hard_w25_line_a,
             @sq_hard_w25_line_b
           ),
           6
         ),
         '|mid=',
         ST_AsText(
           ST_LineInterpolatePoint(@sq_hard_w25_line_a, 0.50)
         ),
         '|quarters=',
         ST_AsText(
           ST_LineInterpolatePoints(@sq_hard_w25_line_a, 0.25)
         ),
         '|simple=',
         ST_AsText(ST_Simplify(@sq_hard_w25_wiggle, 0.05)),
         '|valid=',
         ST_IsValid(@sq_hard_w25_wiggle),
         '|validated=',
         ST_AsText(ST_Validate(@sq_hard_w25_wiggle)),
         '|hash=',
         @sq_hard_w25_hash,
         '|lat=',
         ST_LatFromGeoHash(@sq_hard_w25_hash),
         '|long=',
         ST_LongFromGeoHash(@sq_hard_w25_hash),
         '|decoded=',
         ST_AsText(ST_PointFromGeoHash(@sq_hard_w25_hash, 0)),
         '|projected=',
         ST_SRID(@sq_hard_w25_projected),
         '|roundtrip=',
         ROUND(ST_Longitude(@sq_hard_w25_roundtrip), 4),
         ',',
         ROUND(ST_Latitude(@sq_hard_w25_roundtrip), 4)
       ),
       ST_NumGeometries(
         ST_LineInterpolatePoints(@sq_hard_w25_line_a, 0.25)
       ) * 100
         + ST_NumPoints(ST_Simplify(@sq_hard_w25_wiggle, 0.05))
INTO @sq_hard_w25_function_shape,
     @sq_hard_w25_function_total;

INSERT INTO sq_hard_w25_note (note_key, note_text, note_value)
VALUES (
  'spatial-function-chain',
  @sq_hard_w25_function_shape,
  @sq_hard_w25_function_total
);

-- ------------------------------------------------------------------------------
-- W25-C: REQUESTED_STEP recursively mints row ordinals, PLAN_OWNER obtains the
-- matching fraction/label namespace from JSON_TABLE, and SAMPLE_OWNER is a
-- correlated LATERAL derived table whose only column is a computed POINT.
-- ST_COLLECT and JSON_OBJECTAGG then consume that minted column as windows.
-- ------------------------------------------------------------------------------
WITH RECURSIVE requested_step (route_id, step_no) AS (
  SELECT route_owner.route_id,
         1
  FROM sq_hard_w25_route AS route_owner

  UNION ALL

  SELECT requested_step.route_id,
         requested_step.step_no + 1
  FROM requested_step
  JOIN sq_hard_w25_route AS route_limit
    ON route_limit.route_id = requested_step.route_id
  WHERE requested_step.step_no < JSON_LENGTH(route_limit.sample_plan)
),
expanded_sample AS (
  SELECT route_owner.route_id,
         route_owner.route_name,
         requested_step.step_no,
         plan_owner.label_name,
         plan_owner.fraction_value,
         sample_owner.point_value
  FROM sq_hard_w25_route AS route_owner
  JOIN requested_step
    ON requested_step.route_id = route_owner.route_id
  JOIN JSON_TABLE(
    route_owner.sample_plan,
    '$[*]'
    COLUMNS (
      plan_no FOR ORDINALITY,
      fraction_value DECIMAL(5, 2) PATH '$.fraction'
        ERROR ON EMPTY
        ERROR ON ERROR,
      label_name VARCHAR(30) PATH '$.label'
        ERROR ON EMPTY
        ERROR ON ERROR
    )
  ) AS plan_owner
    ON plan_owner.plan_no = requested_step.step_no
  JOIN LATERAL (
    SELECT ST_LineInterpolatePoint(
             route_owner.route_path,
             plan_owner.fraction_value
           ) AS point_value
  ) AS sample_owner
    ON TRUE
),
analytic_sample AS (
  SELECT expanded_sample.*,
         ST_Collect(point_value) OVER w_running AS point_cloud,
         JSON_OBJECTAGG(
           label_name,
           ST_AsText(point_value)
         ) OVER w_running AS point_map,
         ROW_NUMBER() OVER (
           PARTITION BY route_id
           ORDER BY step_no DESC
         ) AS reverse_position
  FROM expanded_sample
  WINDOW
    w_route AS (
      PARTITION BY route_id
    ),
    w_running AS (
      w_route
      ORDER BY step_no
      ROWS BETWEEN UNBOUNDED PRECEDING AND CURRENT ROW
    )
)
SELECT GROUP_CONCAT(
         CONCAT(
           route_id,
           ':',
           ST_AsText(point_cloud),
           ':',
           JSON_UNQUOTE(JSON_EXTRACT(point_map, '$.half'))
         )
         ORDER BY route_id
         SEPARATOR '/'
       ),
       SUM(ST_NumGeometries(point_cloud))
INTO @sq_hard_w25_sample_shape,
     @sq_hard_w25_sample_total
FROM analytic_sample
WHERE reverse_position = 1;

INSERT INTO sq_hard_w25_note (note_key, note_text, note_value)
VALUES (
  'recursive-json-lateral',
  @sq_hard_w25_sample_shape,
  @sq_hard_w25_sample_total
);

-- EXPLAIN ANALYZE executes the correlated JSON/spatial plan rather than merely
-- parsing it; NO_MERGE addresses the LATERAL alias inside an optimizer comment.
EXPLAIN ANALYZE
SELECT /*+ NO_MERGE(sample_owner) */
       route_owner.route_id,
       plan_owner.plan_no,
       ST_AsText(sample_owner.point_value) AS sampled_point
FROM sq_hard_w25_route AS route_owner
JOIN JSON_TABLE(
  route_owner.sample_plan,
  '$[*]'
  COLUMNS (
    plan_no FOR ORDINALITY,
    fraction_value DECIMAL(5, 2) PATH '$.fraction',
    label_name VARCHAR(30) PATH '$.label'
  )
) AS plan_owner
  ON TRUE
JOIN LATERAL (
  SELECT ST_LineInterpolatePoint(
           route_owner.route_path,
           plan_owner.fraction_value
         ) AS point_value
) AS sample_owner
  ON TRUE
WHERE plan_owner.label_name IN ('half', 'finish')
ORDER BY route_owner.route_id, plan_owner.plan_no;

-- ------------------------------------------------------------------------------
-- Wave-25 self-verification.
-- ------------------------------------------------------------------------------
CALL sq_hard_assert(
  @sq_hard_w25_window_shape =
    '1:MULTIPOINT((0 0),(0 5),(5 5)):4/'
    '2:MULTIPOINT((1 0),(2 0)):3'
  AND @sq_hard_w25_window_total = 5,
  CONCAT(
    'distinct spatial window: ',
    COALESCE(@sq_hard_w25_window_shape, 'NULL'),
    '/',
    COALESCE(@sq_hard_w25_window_total, -1)
  ));

CALL sq_hard_assert(
  @sq_hard_w25_function_shape =
    'frechet=2.828427|hausdorff=1|mid=POINT(0 5)|'
    'quarters=MULTIPOINT((0 2.5),(0 5),(2.5 5),(5 5))|'
    'simple=LINESTRING(0 0,3 0)|valid=1|validated='
    'LINESTRING(0 0,1 0.01,2 -0.01,3 0)|'
    'hash=s00twy01mtw037m|lat=1|long=1|decoded=POINT(1 1)|'
    'projected=3857|roundtrip=127.0276,37.4979'
  AND @sq_hard_w25_function_total = 402,
  'wave25 spatial function chain'
);

CALL sq_hard_assert(
  @sq_hard_w25_sample_shape =
    '1:MULTIPOINT((0 2.5),(0 5),(2.5 5),(5 5)):POINT(0 5)/'
    '2:MULTIPOINT((1 0),(2 0),(4 0)):POINT(2 0)'
  AND @sq_hard_w25_sample_total = 7
  AND (SELECT COUNT(*) FROM sq_hard_w25_note) = 3,
  CONCAT(
    'recursive json lateral: ',
    COALESCE(@sq_hard_w25_sample_shape, 'NULL'),
    '/',
    COALESCE(@sq_hard_w25_sample_total, -1)
  ));

-- ------------------------------------------------------------------------------
-- ULTRA WAVE 26 -- partition-qualified DML, row-subquery analytics, stored
-- program declaration states, dynamic SQL and transaction-chain singularity.
--
-- One RANGE/LINEAR-HASH relation crosses every syntactic owner in this wave:
-- INSERT names parent partitions, UPDATE names subpartitions before its alias,
-- while DELETE puts its alias before the same clause. A correlated row subquery
-- then feeds named-window ranking. Finally, a cursor declared over parenthesized
-- query expressions drives a numeric condition handler, stacked diagnostics and
-- a prepared INSERT while the client delimiter occurs inside a string literal.
-- Explicit CHAIN/NO CHAIN and NO RELEASE tails close the transaction grammar
-- without disconnecting the certification client or changing global state.
-- ------------------------------------------------------------------------------
CREATE TABLE sq_hard_w26_note (
  note_key   VARCHAR(40) NOT NULL PRIMARY KEY,
  note_text  VARCHAR(1200) NOT NULL,
  note_value BIGINT NOT NULL
) ENGINE = InnoDB;

CREATE TABLE sq_hard_w26_event (
  tenant_id          INT NOT NULL,
  event_id           INT NOT NULL,
  `condition`        VARCHAR(20) NOT NULL,
  amount             INT NOT NULL,
  `payload;--/*#*/`  VARCHAR(120) NOT NULL,
  `signal`            VARCHAR(180) GENERATED ALWAYS AS (
    CONCAT(
      tenant_id, ':', event_id, ':', `condition`, ':',
      `payload;--/*#*/`
    )
  ) STORED,
  PRIMARY KEY (tenant_id, event_id),
  KEY ix_sq_hard_w26_condition_amount (
    `condition`,
    amount DESC,
    event_id
  ),
  CONSTRAINT ck_sq_hard_w26_amount CHECK (amount >= 0)
) ENGINE = InnoDB
PARTITION BY RANGE (tenant_id)
SUBPARTITION BY LINEAR HASH (event_id) (
  PARTITION p_low VALUES LESS THAN (10) (
    SUBPARTITION `p_low$0`,
    SUBPARTITION `p_low$1`
  ),
  PARTITION p_high VALUES LESS THAN MAXVALUE (
    SUBPARTITION `p_high$0`,
    SUBPARTITION `p_high$1`
  )
);

CREATE TABLE sq_hard_w26_audit (
  `rank`              INT NOT NULL PRIMARY KEY,
  tenant_id           INT NOT NULL,
  event_id            INT NOT NULL,
  `condition`         VARCHAR(20) NOT NULL,
  amount              INT NOT NULL,
  `prepare;--/*#*/`   VARCHAR(120) NOT NULL,
  UNIQUE KEY uk_sq_hard_w26_audit_event (tenant_id, event_id)
) ENGINE = InnoDB;

CREATE TABLE sq_hard_w26_transaction (
  transaction_id INT NOT NULL PRIMARY KEY,
  `commit`       VARCHAR(80) NOT NULL
) ENGINE = InnoDB;

-- ------------------------------------------------------------------------------
-- W26-A: PARTITION belongs to a different syntactic position in each DML form.
-- Both low subpartitions are named so the UPDATE is independent of the hash
-- result. DELETE's quoted keyword alias precedes PARTITION by grammar, and its
-- ORDER BY/LIMIT tail removes exactly the newest discard candidate.
-- ------------------------------------------------------------------------------
INSERT INTO sq_hard_w26_event
PARTITION (p_low) (
  tenant_id,
  event_id,
  `condition`,
  amount,
  `payload;--/*#*/`
)
VALUES
  (1, 1, 'open',  5, 'semi;--not-comment'),
  (1, 2, 'open',  8, 'block/*inside*/text'),
  (2, 3, 'hold', 13, 'hash#inside-text');

INSERT INTO sq_hard_w26_event
PARTITION (p_high) (
  tenant_id,
  event_id,
  `condition`,
  amount,
  `payload;--/*#*/`
)
VALUES
  (11, 4, 'open',    21, 'DELIMITER |26|'),
  (11, 5, 'discard', 34, 'quote''and`tick'),
  (12, 6, 'discard', 55, 'remove-me');

UPDATE sq_hard_w26_event
PARTITION (`p_low$0`, `p_low$1`) AS `update`
SET `condition` = 'ready',
    amount = `update`.amount + 1,
    `payload;--/*#*/` = CONCAT(
      `update`.`payload;--/*#*/`,
      '|updated'
    )
WHERE `update`.tenant_id = 1
  AND ROW(`update`.tenant_id, `update`.event_id)
      IN (ROW(1, 1), ROW(1, 2));
SET @sq_hard_w26_update_rows = ROW_COUNT();

DELETE FROM sq_hard_w26_event AS `delete`
PARTITION (`p_high$0`, `p_high$1`)
WHERE `delete`.`condition` = 'discard'
ORDER BY `delete`.event_id DESC
LIMIT 1;
SET @sq_hard_w26_delete_rows = ROW_COUNT();

SELECT GROUP_CONCAT(
         CONCAT(
           event_owner.tenant_id, ':',
           event_owner.event_id, ':',
           event_owner.`condition`, ':',
           event_owner.amount
         )
         ORDER BY event_owner.tenant_id, event_owner.event_id
         SEPARATOR '/'
       ),
       SUM(event_owner.amount)
INTO @sq_hard_w26_partition_shape,
     @sq_hard_w26_partition_total
FROM sq_hard_w26_event
PARTITION (
  `p_low$0`,
  `p_low$1`,
  `p_high$0`,
  `p_high$1`
) AS event_owner;

INSERT INTO sq_hard_w26_note (note_key, note_text, note_value)
VALUES (
  'partition-qualified-dml',
  @sq_hard_w26_partition_shape,
  @sq_hard_w26_partition_total
);

-- ------------------------------------------------------------------------------
-- W26-B: the two-column subquery is a scalar ROW, not a derived relation. It is
-- correlated by tenant and picks the lexicographically smallest benchmark.
-- A second row-constructor predicate changes ROW back into a value-list owner;
-- DENSE_RANK and the previously uncovered NTILE then share a named window.
-- ------------------------------------------------------------------------------
WITH tuple_candidate AS (
  SELECT DISTINCT
         `row`.tenant_id,
         `row`.event_id,
         `row`.`condition`,
         `row`.amount
  FROM sq_hard_w26_event
       PARTITION (
         `p_low$0`,
         `p_low$1`,
         `p_high$0`,
         `p_high$1`
       ) AS `row`
  WHERE ROW(`row`.amount, `row`.event_id) >
        (
          SELECT benchmark.amount,
                 benchmark.event_id
          FROM sq_hard_w26_event
               PARTITION (
                 `p_low$0`,
                 `p_low$1`,
                 `p_high$0`,
                 `p_high$1`
               ) AS benchmark
          WHERE benchmark.tenant_id = `row`.tenant_id
          ORDER BY benchmark.amount, benchmark.event_id
          LIMIT 1
        )
    AND ROW(`row`.`condition`, MOD(`row`.event_id, 2)) IN (
      ROW('ready', 0),
      ROW('discard', 1)
    )
),
ranked_tuple AS (
  SELECT /*+ NO_MERGE(tuple_candidate) */
         tuple_candidate.*,
         DENSE_RANK() OVER w_amount AS `rank`,
         NTILE(2) OVER w_amount AS `window`
  FROM tuple_candidate
  WINDOW w_amount AS (
    ORDER BY amount, event_id
  )
)
SELECT GROUP_CONCAT(
         CONCAT(
           ranked_tuple.tenant_id, ':',
           ranked_tuple.event_id, ':',
           ranked_tuple.`condition`, ':',
           ranked_tuple.amount, ':',
           ranked_tuple.`rank`, ':',
           ranked_tuple.`window`
         )
         ORDER BY ranked_tuple.`rank`
         SEPARATOR '/'
       ),
       SUM(ranked_tuple.amount)
INTO @sq_hard_w26_tuple_shape,
     @sq_hard_w26_tuple_total
FROM ranked_tuple;

INSERT INTO sq_hard_w26_note (note_key, note_text, note_value)
VALUES (
  'row-subquery-window',
  @sq_hard_w26_tuple_shape,
  @sq_hard_w26_tuple_total
);

-- ------------------------------------------------------------------------------
-- W26-C: declaration context changes five times before executable code begins:
-- variables, numeric CONDITION, CTE/set-expression CURSOR, NOT FOUND handler,
-- and duplicate-key handler. The cursor's branches own independent ORDER/LIMIT
-- tails. The expected duplicate is swallowed only after GET STACKED DIAGNOSTICS
-- records it; dynamic SQL then addresses quoted keyword/punctuation columns.
--
-- The exact |26| delimiter also occurs inside the prepared SQL literal together
-- with semicolon, dash, block-comment and hash tokens. None terminates CREATE
-- PROCEDURE early, yet the prepared text later parses its own block comment.
-- ------------------------------------------------------------------------------
DELIMITER |26|
CREATE PROCEDURE sq_hard_w26_cursor_prepare(
  OUT p_shape VARCHAR(1200),
  OUT p_total BIGINT,
  OUT p_state CHAR(5),
  OUT p_errno INT
)
SQL SECURITY INVOKER
MODIFIES SQL DATA
routine_owner: BEGIN
  DECLARE v_done      BOOLEAN DEFAULT FALSE;
  DECLARE v_rank      INT DEFAULT 0;
  DECLARE v_tenant_id INT;
  DECLARE v_event_id  INT;
  DECLARE v_condition VARCHAR(20);
  DECLARE v_amount    INT;
  DECLARE v_state     CHAR(5);
  DECLARE v_errno     INT DEFAULT 0;
  DECLARE v_message   VARCHAR(512);

  DECLARE duplicate_key CONDITION FOR 1062;

  DECLARE event_cursor CURSOR FOR
    WITH cursor_source AS (
      (
        SELECT low_owner.tenant_id,
               low_owner.event_id,
               low_owner.`condition`,
               low_owner.amount
        FROM sq_hard_w26_event
             PARTITION (`p_low$0`, `p_low$1`) AS low_owner
        ORDER BY low_owner.amount DESC, low_owner.event_id
        LIMIT 2
      )
      UNION ALL
      (
        SELECT high_owner.tenant_id,
               high_owner.event_id,
               high_owner.`condition`,
               high_owner.amount
        FROM sq_hard_w26_event
             PARTITION (`p_high$0`, `p_high$1`) AS high_owner
        WHERE high_owner.`condition` = 'open'
        ORDER BY high_owner.amount, high_owner.event_id
        LIMIT 1
      )
    )
    SELECT cursor_source.tenant_id,
           cursor_source.event_id,
           cursor_source.`condition`,
           cursor_source.amount
    FROM cursor_source
    ORDER BY cursor_source.tenant_id, cursor_source.event_id;

  DECLARE CONTINUE HANDLER FOR NOT FOUND SET v_done = TRUE;
  DECLARE CONTINUE HANDLER FOR duplicate_key
  BEGIN
    GET STACKED DIAGNOSTICS CONDITION 1
      v_state   = RETURNED_SQLSTATE,
      v_errno   = MYSQL_ERRNO,
      v_message = MESSAGE_TEXT;
  END;

  OPEN event_cursor;
  cursor_loop: LOOP
    FETCH event_cursor
      INTO v_tenant_id, v_event_id, v_condition, v_amount;

    IF v_done THEN
      LEAVE cursor_loop;
    END IF;

    SET v_rank = v_rank + 1;
    INSERT INTO sq_hard_w26_audit (
      `rank`,
      tenant_id,
      event_id,
      `condition`,
      amount,
      `prepare;--/*#*/`
    )
    VALUES (
      v_rank,
      v_tenant_id,
      v_event_id,
      v_condition,
      v_amount,
      'cursor|26|;--/*#*/'
    );
  END LOOP cursor_loop;
  CLOSE event_cursor;

  -- This duplicate is the only statement expected to enter DUPLICATE_KEY.
  INSERT INTO sq_hard_w26_audit (
    `rank`,
    tenant_id,
    event_id,
    `condition`,
    amount,
    `prepare;--/*#*/`
  )
  VALUES (1, 999, 999, 'must-not-land', 999, 'duplicate');

  SET @sq_hard_w26_min_amount = CAST(0 AS SIGNED);
  SET @sq_hard_w26_excluded_condition = 'never';
  SET @sq_hard_w26_dynamic_sql =
    'INSERT INTO sq_hard_w26_note (note_key, note_text, note_value)
     SELECT /* dynamic|26|;-- # */ ''cursor-prepared'',
            GROUP_CONCAT(
              CONCAT(`rank`, '':'', event_id, '':'', `condition`)
              ORDER BY `rank`
              SEPARATOR ''/''
            ),
            SUM(amount)
     FROM sq_hard_w26_audit
     WHERE amount >= ?
       AND `condition` <> ?';

  PREPARE `prepare;--/*#*/` FROM @sq_hard_w26_dynamic_sql;
  EXECUTE `prepare;--/*#*/`
    USING @sq_hard_w26_min_amount,
          @sq_hard_w26_excluded_condition;
  DEALLOCATE PREPARE `prepare;--/*#*/`;

  SELECT note_text,
         note_value
    INTO p_shape,
         p_total
  FROM sq_hard_w26_note
  WHERE note_key = 'cursor-prepared';

  SET p_state = v_state;
  SET p_errno = v_errno;
  SET @sq_hard_w26_diagnostic_message = v_message;
END routine_owner|26|
DELIMITER ;

CALL sq_hard_w26_cursor_prepare(
  @sq_hard_w26_cursor_shape,
  @sq_hard_w26_cursor_total,
  @sq_hard_w26_cursor_state,
  @sq_hard_w26_cursor_errno
);

-- ------------------------------------------------------------------------------
-- W26-D: CHAIN opens the successor transaction immediately; NO RELEASE keeps
-- this client connected. A savepoint with lexer punctuation is rolled back and
-- released before COMMIT CHAIN, the next transaction is wholly rolled back and
-- chained, and the final NO CHAIN commit leaves exactly two durable rows.
-- ------------------------------------------------------------------------------
START TRANSACTION READ WRITE;
INSERT INTO sq_hard_w26_transaction (transaction_id, `commit`)
VALUES (1, 'commit-before-chain');
SAVEPOINT `save;--/*#*/`;
INSERT INTO sq_hard_w26_transaction (transaction_id, `commit`)
VALUES (2, 'rollback-to-savepoint');
ROLLBACK WORK TO SAVEPOINT `save;--/*#*/`;
RELEASE SAVEPOINT `save;--/*#*/`;
COMMIT WORK AND CHAIN NO RELEASE;

INSERT INTO sq_hard_w26_transaction (transaction_id, `commit`)
VALUES (3, 'rollback-whole-chain');
ROLLBACK WORK AND CHAIN NO RELEASE;

INSERT INTO sq_hard_w26_transaction (transaction_id, `commit`)
VALUES (4, 'commit-after-rollback-chain');
COMMIT WORK AND NO CHAIN NO RELEASE;

SELECT GROUP_CONCAT(
         CONCAT(transaction_id, ':', `commit`)
         ORDER BY transaction_id
         SEPARATOR '/'
       ),
       SUM(transaction_id)
INTO @sq_hard_w26_transaction_shape,
     @sq_hard_w26_transaction_total
FROM sq_hard_w26_transaction;

INSERT INTO sq_hard_w26_note (note_key, note_text, note_value)
VALUES (
  'transaction-chain',
  @sq_hard_w26_transaction_shape,
  @sq_hard_w26_transaction_total
);

-- ------------------------------------------------------------------------------
-- Wave-26 self-verification.
-- ------------------------------------------------------------------------------
CALL sq_hard_assert(
  @sq_hard_w26_partition_shape =
    '1:1:ready:6/1:2:ready:9/2:3:hold:13/'
    '11:4:open:21/11:5:discard:34'
  AND @sq_hard_w26_partition_total = 83
  AND @sq_hard_w26_update_rows = 2
  AND @sq_hard_w26_delete_rows = 1
  AND (
    SELECT COUNT(*)
    FROM sq_hard_w26_event
         PARTITION (`p_low$0`, `p_low$1`)
  ) = 3
  AND (
    SELECT COUNT(*)
    FROM sq_hard_w26_event
         PARTITION (`p_high$0`, `p_high$1`)
  ) = 2,
  CONCAT(
    'partition-qualified dml: ',
    COALESCE(@sq_hard_w26_partition_shape, 'NULL')
  )
);

CALL sq_hard_assert(
  @sq_hard_w26_tuple_shape =
    '1:2:ready:9:1:1/11:5:discard:34:2:2'
  AND @sq_hard_w26_tuple_total = 43,
  CONCAT(
    'row-subquery window: ',
    COALESCE(@sq_hard_w26_tuple_shape, 'NULL')
  )
);

CALL sq_hard_assert(
  @sq_hard_w26_cursor_shape =
    '1:2:ready/2:3:hold/3:4:open'
  AND @sq_hard_w26_cursor_total = 43
  AND @sq_hard_w26_cursor_state = '23000'
  AND @sq_hard_w26_cursor_errno = 1062
  AND CHAR_LENGTH(@sq_hard_w26_diagnostic_message) > 0
  AND (SELECT COUNT(*) FROM sq_hard_w26_audit) = 3
  AND (
    SELECT COUNT(*)
    FROM sq_hard_w26_audit
    WHERE `prepare;--/*#*/` = 'cursor|26|;--/*#*/'
  ) = 3,
  CONCAT(
    'cursor/handler/prepare: ',
    COALESCE(@sq_hard_w26_cursor_shape, 'NULL'),
    '/',
    COALESCE(@sq_hard_w26_cursor_state, 'NULL'),
    '/',
    COALESCE(@sq_hard_w26_cursor_errno, -1)
  )
);

CALL sq_hard_assert(
  @sq_hard_w26_transaction_shape =
    '1:commit-before-chain/4:commit-after-rollback-chain'
  AND @sq_hard_w26_transaction_total = 5
  AND (SELECT COUNT(*) FROM sq_hard_w26_transaction) = 2
  AND (SELECT COUNT(*) FROM sq_hard_w26_note) = 4,
  CONCAT(
    'transaction chain: ',
    COALESCE(@sq_hard_w26_transaction_shape, 'NULL')
  )
);

SELECT 'PASS' AS final_status,
       VERSION() AS server_version,
       (SELECT COUNT(*) FROM metric) AS metric_rows,
       @walk_total AS walk_total,
       (SELECT COUNT(*) FROM sq_hard_w4_place) AS spatial_rows,
       @sq_hard_w4_walk_total AS wave4_total,
       (SELECT COUNT(*) FROM sq_hard_w5_hint) AS wave5_rows,
       @sq_hard_w5_label AS wave5_label,
       (SELECT COUNT(*) FROM sq_hard_w6_series) AS wave6_rows,
       @sq_hard_w6_walk_rows AS wave6_walk_rows,
       (SELECT COUNT(*) FROM sq_hard_w7_txn) AS wave7_txn_rows,
       (SELECT COUNT(*) FROM sq_hard_w7_maint) AS wave7_maint_rows,
       (SELECT COUNT(*) FROM sq_hard_w8_fact) AS wave8_fact_rows,
       (SELECT doc ->> '$.n' FROM sq_hard_w8_doc WHERE doc_id = 1) AS wave8_doc_n,
       (SELECT COUNT(*) FROM sq_hard_w8_swap) AS wave8_swapped_rows,
       (SELECT COUNT(*) FROM sq_hard_w9_note) AS wave9_notes,
       @sq_hard_w9_island AS wave9_island,
       (SELECT COUNT(*) FROM sq_hard_w9_shape) AS wave9_shapes,
       @sq_hard_w9_delims AS wave9_delims,
       (SELECT COUNT(*) FROM sq_hard_w10_note) AS wave10_notes,
       (SELECT COUNT(*) FROM sq_hard_w10_xa_log) AS wave10_xa_rows,
       @sq_hard_w10_ngram AS wave10_ngram,
       (SELECT COUNT(*) FROM sq_hard_w10_gipk) AS wave10_gipk_rows,
       (SELECT COUNT(*) FROM sq_hard_w11_note) AS wave11_notes,
       @sq_hard_w11_type_sum AS wave11_type_sum,
       @sq_hard_w11_island AS wave11_island,
       @sq_hard_w11_sys AS wave11_sys_list,
       @sq_hard_w11_funcs AS wave11_funcs,
       @sq_hard_w11_lexer AS wave11_lexer,
       (SELECT COUNT(*) FROM sq_hard_w12_note) AS wave12_notes,
       @sq_hard_w12_verbs AS wave12_verbs,
       @sq_hard_w12_quoted AS wave12_quoted,
       @sq_hard_w12_setops AS wave12_setops,
       @sq_hard_w12_dyn_hits AS wave12_dynamic_hits,
       @sq_hard_w12_lexer AS wave12_lexer,
       (SELECT COUNT(*) FROM sq_hard_w13_note) AS wave13_notes,
       @sq_hard_w13_trigger_order AS wave13_trigger_order,
       @sq_hard_w13_tag_rows AS wave13_json_tags,
       (SELECT SUM(amount) FROM sq_hard_w13_event) AS wave13_amount_total,
       (SELECT COUNT(*) FROM sq_hard_w13_audit) AS wave13_audit_rows,
       @sq_hard_w13_warning_count AS wave13_warning_count,
       (SELECT COUNT(*) FROM sq_hard_w14_note) AS wave14_notes,
       @sq_hard_w14_column_role AS wave14_column_attribute,
       @sq_hard_w14_index_path AS wave14_index_attribute,
       @sq_hard_w14_trace_steps AS wave14_trace_steps,
       OCTET_LENGTH(@sq_hard_w14_trace) AS wave14_trace_bytes,
       (SELECT payload ->> '$.state'
        FROM sq_hard_w14_diagnostic
       WHERE diagnostic_id = 1) AS wave14_sqlstate,
       @sq_hard_w14_recursive_shape AS wave14_recursive_shape,
       @sq_hard_w14_current_rows AS wave14_current_rows,
       (SELECT COUNT(*) FROM sq_hard_w15_note) AS wave15_notes,
       @sq_hard_w15_token_shape AS wave15_token_shape,
       @sq_hard_w15_state_shape AS wave15_state_shape,
       @sq_hard_w15_history_total AS wave15_history_total,
       (SELECT COUNT(*) FROM sq_hard_w16_note) AS wave16_notes,
       @sq_hard_w16_sibling_shape AS wave16_sibling_shape,
       @sq_hard_w16_summary_shape AS wave16_summary_shape,
       @sq_hard_w16_candidate_shape AS wave16_candidate_shape,
       (SELECT COUNT(*) FROM sq_hard_w17_note) AS wave17_notes,
       @sq_hard_w17_hybrid_shape AS wave17_hybrid_shape,
       JSON_VALUE(
         @sq_hard_w17_hybrid_json,
         '$."1".title'
       ) AS wave17_top_title,
       (SELECT COUNT(*) FROM sq_hard_w18_note) AS wave18_notes,
       @sq_hard_w18_walk_shape AS wave18_walk_shape,
       (SELECT COUNT(*) FROM sq_hard_w19_note) AS wave19_notes,
       @sq_hard_w19_right_shape AS wave19_right_shape,
       (SELECT COUNT(*) FROM sq_hard_w20_note) AS wave20_notes,
       @sq_hard_w20_json_shape AS wave20_json_shape,
       @sq_hard_w20_bag_shape AS wave20_bag_shape,
       @sq_hard_w20_walk_shape AS wave20_walk_shape,
       (SELECT COUNT(*) FROM sq_hard_w21_note) AS wave21_notes,
       @sq_hard_w21_oj_shape AS wave21_oj_shape,
       @sq_hard_w21_handler_shape AS wave21_handler_shape,
       (SELECT COUNT(*) FROM sq_hard_w22_note) AS wave22_notes,
       @sq_hard_w22_temporary_shape AS wave22_temporary_shape,
       @sq_hard_w22_cache_shape AS wave22_cache_shape,
       @sq_hard_w22_export_released AS wave22_export_shape,
       (SELECT COUNT(*) FROM sq_hard_w23_note) AS wave23_notes,
       @sq_hard_w23_main_columns AS wave23_main_columns,
       @sq_hard_w23_temporary_shape AS wave23_temporary_shape,
       @sq_hard_w23_view_shape AS wave23_view_shape,
       (SELECT COUNT(*) FROM sq_hard_w24_note) AS wave24_notes,
       @sq_hard_w24_query_shape AS wave24_query_shape,
       @sq_hard_w24_window_shape AS wave24_window_shape,
       @sq_hard_w24_handler_shape AS wave24_handler_shape,
       (SELECT COUNT(*) FROM sq_hard_w25_note) AS wave25_notes,
       @sq_hard_w25_window_shape AS wave25_window_shape,
       @sq_hard_w25_function_shape AS wave25_function_shape,
       @sq_hard_w25_sample_shape AS wave25_sample_shape,
       (SELECT COUNT(*) FROM sq_hard_w26_note) AS wave26_notes,
       @sq_hard_w26_partition_shape AS wave26_partition_shape,
       @sq_hard_w26_tuple_shape AS wave26_tuple_shape,
       @sq_hard_w26_cursor_shape AS wave26_cursor_shape,
       @sq_hard_w26_transaction_shape AS wave26_transaction_shape;

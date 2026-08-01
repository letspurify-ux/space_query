-- MariaDB HARDCORE editor-engine stress suite.
-- Live target: MariaDB Server 12.2.2.
-- Run from the repository root:
--   mariadb --protocol=TCP -h127.0.0.1 -P3306 -uroot -ppassword \
--     --show-warnings --binary-mode < test_mariadb/final_hardcore.sql
--
-- Purpose: DELIBERATELY hostile-but-legal grammar that pushes the completion,
-- auto-formatting, and syntax-highlighting engines far past the gentle coverage
-- of test_mariadb/final.sql. Everything still parses and executes on the live
-- server so the formatted output can be re-executed for certification.

DROP DATABASE IF EXISTS sq_hard_mariadb;
DROP DATABASE IF EXISTS sq_hard_mariadb_aux;
CREATE DATABASE sq_hard_mariadb CHARACTER SET utf8mb4 COLLATE utf8mb4_unicode_ci;
USE sq_hard_mariadb;
SET SESSION sql_mode = 'STRICT_TRANS_TABLES,NO_ENGINE_SUBSTITUTION';
SET SESSION autocommit = 1;

-- Backtick identifiers that collide with reserved words, embed spaces, doubled
-- backticks, and $/# characters; plus a PERSISTENT generated column.
CREATE TABLE `select` (
  `from`               INT NOT NULL,
  `Group By`           DATE,
  `order`              INT,
  `x$weird#col`        INT,
  `Column ``With`` Backtick` VARCHAR(30),
  json_value           JSON,
  computed_upper       VARCHAR(30) AS (UPPER(`from`)) PERSISTENT,
  PRIMARY KEY (`from`)
) ENGINE = InnoDB;

INSERT INTO `select` (`from`, `Group By`, `order`, `x$weird#col`,
                      `Column ``With`` Backtick`, json_value)
VALUES (1, DATE '2024-02-29', 2, 3, 'b`t',
        JSON_OBJECT('tags', JSON_ARRAY('sql', 'json'),
                    'items', JSON_ARRAY(JSON_OBJECT('n', 'a', 'v', 10),
                                        JSON_OBJECT('n', 'b', 'v', 20))));

SELECT s.`from` + s.`order`                            AS summed,
       s.`Column ``With`` Backtick`                    AS quoted_col,
       EXTRACT(YEAR FROM s.`Group By`)                 AS leap_year
FROM `select` s
WHERE s.`from` = 1 AND s.`order` BETWEEN 1 AND 9;

-- Sequence, MariaDB-specific types (UUID / INET6), and INSERT ... RETURNING.
CREATE SEQUENCE seq_hard START WITH 100 INCREMENT BY 5;

CREATE TABLE endpoint (
  id     INT NOT NULL DEFAULT (NEXT VALUE FOR seq_hard),
  guid   UUID   NOT NULL,
  addr   INET6  NOT NULL,
  PRIMARY KEY (id)
) ENGINE = InnoDB;

INSERT INTO endpoint (guid, addr)
VALUES (UUID(), INET6_ATON('2001:db8::1'))
RETURNING id, INET6_NTOA(addr) AS printable;

-- Deeply nested derived tables (aliased), scalar subqueries, inline comments.
SELECT /* deep nesting */ deep.total
FROM (
  SELECT /* level 1 */
         (SELECT COUNT(*)
          FROM (SELECT 3 c FROM (SELECT 2 b FROM (SELECT 1 a) t3) t2) t1) AS total
) deep;

-- Operator adjacency, exotic literals, JSON path operators, JSON_CONTAINS.
SELECT 1+2*3-4 DIV 2                       AS arithmetic_value,
       5%3, 6&3, 7|1, 8^2, ~9, 1<<4, 256>>2 AS bit_ops,
       X'53514C'                           AS hex_literal,
       0x4D79                              AS zerox_literal,
       b'1010'                             AS bit_literal,
       JSON_EXTRACT(json_value, '$.tags[0]') AS json_extract_value,
       JSON_VALUE(json_value, '$.items[1].n') AS json_scalar_value,
       JSON_CONTAINS(JSON_EXTRACT(json_value, '$.tags'), JSON_QUOTE('sql')) AS json_has_tag
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

CREATE TABLE metric (
  metric_id    INT NOT NULL,
  node_id      INT NOT NULL,
  measured_on  DATE NOT NULL,
  metric_value DECIMAL(12,2) NOT NULL,
  PRIMARY KEY (metric_id)
) ENGINE = InnoDB;
INSERT INTO metric VALUES
  (1, 1, '2026-01-01', 12), (2, 1, '2026-01-03', 18),
  (3, 1, '2026-01-08', 24), (4, 2, '2026-01-02', 2);

-- Recursive CYCLE + nested JSON_TABLE + named-window inheritance +
-- ordered-set analytics + INTERSECT ALL/EXCEPT ALL. Keeping these constructs
-- in one WITH chain forces each editor subsystem to recover the exact CTE,
-- JSON, window, and set-operation owner before the final SELECT.
WITH RECURSIVE
hard_nodes (node_id, parent_node_id, node_name) AS (
  SELECT 1, NULL, 'root'
  UNION ALL SELECT 2, 1, 'blue'
  UNION ALL SELECT 3, 1, 'green'
  UNION ALL SELECT 4, 2, 'leaf'
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
)
CYCLE node_id RESTRICT,
metric_docs (metric_id, node_id, measured_on, payload) AS (
  SELECT metric_id, node_id, measured_on,
         JSON_OBJECT(
           'samples', JSON_ARRAY(
             JSON_OBJECT('kind', 'actual', 'value', metric_value),
             JSON_OBJECT('kind', 'forecast', 'value', metric_value + 1)
           )
         )
  FROM metric
),
expanded AS (
  SELECT d.metric_id, d.node_id, d.measured_on,
         jt.sample_no, jt.sample_kind, jt.sample_value
  FROM metric_docs d
  JOIN JSON_TABLE(
    d.payload,
    '$.samples[*]' COLUMNS (
      sample_no    FOR ORDINALITY,
      sample_kind  VARCHAR(16) PATH '$.kind' ERROR ON ERROR,
      sample_value DECIMAL(12,2) PATH '$.value'
                   DEFAULT '0' ON EMPTY DEFAULT '0' ON ERROR
    )
  ) jt ON TRUE
),
analytic AS (
  SELECT e.*,
         SUM(e.sample_value) OVER w_running AS running_value,
         COALESCE(LAG(e.sample_value, 1) OVER w_ordered, 0) AS previous_value,
         PERCENTILE_CONT(0.5) WITHIN GROUP (
           ORDER BY e.sample_value
         ) OVER (PARTITION BY e.node_id) AS median_value,
         ROW_NUMBER() OVER w_rank AS value_rank
  FROM expanded e
  WINDOW
    w_node AS (PARTITION BY e.node_id),
    w_ordered AS (
      w_node ORDER BY e.measured_on, e.metric_id, e.sample_no
    ),
    w_running AS (
      w_ordered ROWS BETWEEN UNBOUNDED PRECEDING AND CURRENT ROW
    ),
    w_rank AS (
      w_node ORDER BY e.sample_value DESC, e.metric_id, e.sample_no
    )
),
eligible (node_id) AS (
  (SELECT node_id FROM hard_nodes)
  INTERSECT ALL
  (SELECT node_id FROM metric)
  EXCEPT ALL
  (SELECT node_id FROM metric WHERE metric_value < 0)
),
final_rows AS (
  SELECT t.node_id, t.node_path, t.tree_depth,
         CASE WHEN e.node_id IS NULL THEN 'EMPTY' ELSE 'MEASURED' END AS state_name,
         (SELECT a.sample_value
          FROM analytic a
          WHERE a.node_id = t.node_id AND a.value_rank = 1
          LIMIT 1) AS peak_value,
         (SELECT MAX(a.running_value)
          FROM analytic a
          WHERE a.node_id = t.node_id) AS peak_running,
         (SELECT JSON_ARRAYAGG(
                   JSON_OBJECT(
                     'metric', a.metric_id,
                     'kind', a.sample_kind,
                     'value', a.sample_value
                   )
                   ORDER BY a.metric_id, a.sample_no
                 )
          FROM analytic a
          WHERE a.node_id = t.node_id) AS evidence
  FROM hard_tree t
  LEFT JOIN (SELECT DISTINCT node_id FROM eligible) e
    ON e.node_id = t.node_id
)
SELECT CASE
         WHEN (SELECT COUNT(*) FROM hard_tree) = 4
          AND (SELECT COUNT(*) FROM expanded) = 8
          AND (SELECT COUNT(*) FROM eligible) = 2
         THEN 'PASS' ELSE 'FAIL'
       END AS integrated_status,
       node_id, node_path, tree_depth, state_name,
       peak_value, peak_running, evidence
FROM final_rows
ORDER BY node_id;

-- Window functions with named windows, frame, nested CASE, and top-per-group
-- via ROW_NUMBER (MariaDB has no LATERAL).
SELECT metric_id, node_id, metric_value,
       CASE WHEN metric_value > (CASE WHEN node_id = 1 THEN 10 ELSE 20 END)
            THEN 'high' ELSE 'low' END               AS band,
       SUM(metric_value) OVER w_run                  AS running_sum,
       LAG(metric_value, 1) OVER w_ord               AS prev_value
FROM metric
WINDOW w_ord AS (PARTITION BY node_id ORDER BY measured_on, metric_id),
       w_run AS (w_ord ROWS BETWEEN UNBOUNDED PRECEDING AND CURRENT ROW)
ORDER BY node_id, metric_id;

SELECT node_id, metric_id, metric_value
FROM (
  SELECT metric_id, node_id, metric_value,
         ROW_NUMBER() OVER (PARTITION BY node_id ORDER BY metric_value DESC) AS rn
  FROM metric
) ranked
WHERE rn = 1
ORDER BY node_id;

SELECT node_id, SUM(metric_value) AS total
FROM metric GROUP BY node_id WITH ROLLUP;

-- Simultaneous application-time + system-time table. FOR PORTION OF splits the
-- current application interval while FOR SYSTEM_TIME preserves the pre-split
-- snapshot, creating deliberately ambiguous PERIOD/FOR/FROM/TO token contexts.
CREATE TABLE versioned (
  id         INT NOT NULL,
  valid_from DATE NOT NULL,
  valid_to   DATE NOT NULL,
  v          INT NOT NULL,
  row_start  TIMESTAMP(6) GENERATED ALWAYS AS ROW START,
  row_end    TIMESTAMP(6) GENERATED ALWAYS AS ROW END,
  PERIOD FOR validity (valid_from, valid_to),
  PERIOD FOR SYSTEM_TIME (row_start, row_end),
  PRIMARY KEY (id, validity WITHOUT OVERLAPS)
) WITH SYSTEM VERSIONING;
INSERT INTO versioned (id, valid_from, valid_to, v) VALUES
  (1, '2026-01-01', '2027-01-01', 10),
  (2, '2026-01-01', '2027-01-01', 20);
SET @version_cutover = NOW(6);
DO SLEEP(0.02);
UPDATE versioned FOR PORTION OF validity
FROM '2026-03-01' TO '2026-06-01'
SET v = v + 5
WHERE id = 1;
UPDATE versioned SET v = v + 1 WHERE id = 2;

SELECT id, valid_from, valid_to, v, row_start, row_end
FROM versioned FOR SYSTEM_TIME AS OF @version_cutover
ORDER BY id, valid_from;

SELECT id, valid_from, valid_to, v, row_start, row_end
FROM versioned FOR SYSTEM_TIME ALL
ORDER BY id, row_start, valid_from;

-- Stored routine torture: labelled loops, cursor, handlers, SIGNAL/RESIGNAL,
-- GET DIAGNOSTICS, nested blocks, and a scalar function.
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

CREATE PROCEDURE sq_hard_assert(IN cond BOOLEAN, IN msg VARCHAR(255))
BEGIN
  IF COALESCE(cond, FALSE) = FALSE THEN
    SIGNAL SQLSTATE '45000' SET MESSAGE_TEXT = msg;
  END IF;
END$$
DELIMITER ;

CALL sq_hard_walk(@walk_total);

-- Anonymous compound statement with implicit cursor FOR, nested label, local
-- handler, diagnostics, and parameterized EXECUTE IMMEDIATE.
DELIMITER $$
BEGIN NOT ATOMIC
  DECLARE local_total DECIMAL(18,2) DEFAULT 0;
  DECLARE local_rows INT DEFAULT 0;
  DECLARE local_state CHAR(5);
  DECLARE local_errno INT;
  DECLARE local_message TEXT;

  metric_loop: FOR metric_rec IN (
    SELECT node_id, SUM(metric_value) AS node_total
    FROM metric
    GROUP BY node_id
    ORDER BY node_id
  ) DO
    SET local_total = local_total + metric_rec.node_total;
    SET local_rows = local_rows + 1;
  END FOR metric_loop;

  unreachable_probe: BEGIN
    DECLARE CONTINUE HANDLER FOR SQLEXCEPTION
    BEGIN
      GET DIAGNOSTICS CONDITION 1
        local_state = RETURNED_SQLSTATE,
        local_errno = MYSQL_ERRNO,
        local_message = MESSAGE_TEXT;
      RESIGNAL SET MESSAGE_TEXT = local_message;
    END;
    IF local_total < 0 THEN
      SIGNAL SQLSTATE '45000' SET MESSAGE_TEXT = 'unreachable';
    END IF;
  END unreachable_probe;

  CALL sq_hard_assert(local_total = 56 AND local_rows = 2,
                      'anonymous cursor totals');
  SET @hard_floor = 20;
  EXECUTE IMMEDIATE
    'SELECT metric_id, node_id, metric_value
       FROM metric
      WHERE metric_value >= ?
      ORDER BY metric_value DESC, metric_id'
    USING @hard_floor;
END$$
DELIMITER ;

CALL sq_hard_assert(@walk_total = 56, 'cursor walk total');
CALL sq_hard_assert((SELECT `from` + `order` FROM `select`) = 3, 'quoted identifier sum');
CALL sq_hard_assert((SELECT COUNT(*) FROM metric) = 4, 'metric rows');
CALL sq_hard_assert(sq_hard_band(24) = 'HIGH' COLLATE utf8mb4_unicode_ci, 'scalar function');
CALL sq_hard_assert((SELECT COUNT(*) FROM versioned) = 4, 'application fragments');
CALL sq_hard_assert(
  (SELECT COUNT(*) FROM versioned FOR SYSTEM_TIME AS OF @version_cutover) = 2,
  'system-time snapshot'
);
CALL sq_hard_assert(
  (SELECT COUNT(*) FROM versioned FOR SYSTEM_TIME ALL) >= 6,
  'bitemporal versions'
);

-- Standalone table value constructor statement and hash/executable comments.
# hash comment: the next statement is a bare VALUES table constructor
VALUES (1, 'one'), (2, 'two'), (3, 'three');

SELECT /*M! STRAIGHT_JOIN */ /*!40001 SQL_NO_CACHE */ m.metric_id, n.node_total
FROM metric m
JOIN (SELECT node_id, SUM(metric_value) AS node_total
      FROM metric GROUP BY node_id) n
  ON n.node_id = m.node_id
WHERE m.metric_id = 1;

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

-- MariaDB dynamic columns round-trip: CREATE/ADD/GET/JSON/LIST/EXISTS.
SELECT COLUMN_GET(doc.blob_col, 'depth' AS INT)              AS depth_value,
       COLUMN_GET(doc.blob_col, '색상' AS CHAR)              AS color_value,
       COLUMN_JSON(COLUMN_ADD(doc.blob_col, 'extra', 9))     AS json_view,
       COLUMN_LIST(doc.blob_col)                             AS key_list,
       COLUMN_EXISTS(doc.blob_col, '색상')                   AS has_color
FROM (SELECT COLUMN_CREATE('depth', 3, '색상', '파랑' AS CHAR) AS blob_col) doc;

-- Sequence algebra: PREVIOUS/NEXT VALUE, SETVAL, LASTVAL, ALTER ... RESTART.
SELECT NEXT VALUE FOR seq_hard AS seq_first;
SELECT PREVIOUS VALUE FOR seq_hard AS seq_prev, LASTVAL(seq_hard) AS seq_last;
SELECT SETVAL(seq_hard, 500) AS seq_forced;
ALTER SEQUENCE seq_hard RESTART WITH 900;
SELECT NEXT VALUE FOR seq_hard AS seq_after_restart;

-- DELETE ... RETURNING and single-table UPDATE ... ORDER BY ... LIMIT.
CREATE TABLE scratch_rows (
  id INT NOT NULL PRIMARY KEY,
  v  INT NOT NULL
) ENGINE = InnoDB;
INSERT INTO scratch_rows VALUES (1, 10), (2, 20), (3, 30), (4, 40);

DELETE FROM scratch_rows
WHERE v > 25
ORDER BY v DESC
LIMIT 1
RETURNING id, v * 2 AS doubled;

UPDATE scratch_rows
SET v = v + 1
ORDER BY v ASC
LIMIT 2;

INSERT INTO scratch_rows (id, v) VALUES (1, 99)
ON DUPLICATE KEY UPDATE v = VALUES(v) + scratch_rows.v;

-- Range/hash subpartitioned table with explicit partition selection.
CREATE TABLE part_metric (
  metric_id INT NOT NULL,
  measured_year INT NOT NULL,
  metric_value DECIMAL(12,2) NOT NULL,
  PRIMARY KEY (metric_id, measured_year)
) ENGINE = InnoDB
PARTITION BY RANGE (measured_year)
SUBPARTITION BY HASH (metric_id)
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

-- XA transaction (one-phase) wrapping an insert.
XA START 'sq_hard_xa';
INSERT INTO scratch_rows (id, v) VALUES (7, 70);
XA END 'sq_hard_xa';
XA COMMIT 'sq_hard_xa' ONE PHASE;

-- MEDIAN window function, GROUP_CONCAT with ORDER BY + SEPARATOR + LIMIT,
-- and NTILE/PERCENT_RANK stacked over one partition spec.
SELECT metric_id, node_id, metric_value,
       MEDIAN(metric_value) OVER (PARTITION BY node_id)        AS node_median,
       NTILE(2) OVER (ORDER BY metric_value)                   AS half_bucket,
       PERCENT_RANK() OVER (ORDER BY metric_value)             AS pct_rank
FROM metric
ORDER BY metric_id;

SELECT GROUP_CONCAT(metric_value ORDER BY metric_value DESC SEPARATOR '|' LIMIT 3)
         AS top_values
FROM metric;

-- Trigger ordering with FOLLOWS plus CREATE OR REPLACE.
CREATE TABLE trigger_log (
  log_id INT NOT NULL AUTO_INCREMENT PRIMARY KEY,
  note VARCHAR(40) NOT NULL
) ENGINE = InnoDB;

CREATE TRIGGER scratch_first
BEFORE INSERT ON scratch_rows FOR EACH ROW
INSERT INTO trigger_log (note) VALUES (CONCAT('first:', NEW.id));

CREATE OR REPLACE TRIGGER scratch_second
BEFORE INSERT ON scratch_rows FOR EACH ROW
FOLLOWS scratch_first
INSERT INTO trigger_log (note) VALUES (CONCAT('second:', NEW.id * 10));

INSERT INTO scratch_rows (id, v) VALUES (9, 90);

-- Disabled event: never fires, but exercises the event grammar.
CREATE EVENT sq_hard_event
  ON SCHEDULE AT CURRENT_TIMESTAMP + INTERVAL 10 YEAR
  ON COMPLETION PRESERVE
  DISABLE
  COMMENT '편집기 스트레스용'
  DO UPDATE scratch_rows SET v = v WHERE id = -1;

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

-- Date/interval algebra with STR_TO_DATE and TIMESTAMPDIFF.
SELECT DATE_ADD(STR_TO_DATE('2026/01/31', '%Y/%m/%d'),
                INTERVAL '1-2' YEAR_MONTH)               AS pushed_out,
       TIMESTAMPDIFF(HOUR,
                     '2026-01-01 00:00:00',
                     '2026-01-03 12:00:00')              AS hour_gap,
       ADDTIME(TIME'10:30:00', '01:15:30.5')             AS padded_time;

-- ULTRA WAVE 3: cross-database objects force schema-qualified completion.
CREATE DATABASE sq_hard_mariadb_aux CHARACTER SET utf8mb4;
CREATE TABLE sq_hard_mariadb_aux.node_lookup (
  node_id   INT NOT NULL PRIMARY KEY,
  node_tag  VARCHAR(20) NOT NULL
) ENGINE = InnoDB;
INSERT INTO sq_hard_mariadb_aux.node_lookup VALUES (1, 'core'), (2, 'edge');

SELECT m.metric_id, l.node_tag
FROM metric m
JOIN sq_hard_mariadb_aux.node_lookup l ON l.node_id = m.node_id
WHERE m.metric_id <= 2
ORDER BY m.metric_id;

-- Lexer bait: backtick aliases containing comment openers, charset introducers
-- with hex payloads, bit literals, null-safe / XOR / && operators, and a
-- running user-variable assignment inside the projection.
SELECT '/* not a comment */'                    AS `co--mment`,
       '-- still a string'                      AS `/*alias*/`,
       _utf8mb4 X'E29C93'                       AS check_glyph,
       _binary X'DEAD'                          AS bin_lit,
       0b1011                                   AS zerob_literal,
       N'국가별'                                AS national_lit,
       NULL <=> NULL                            AS null_safe_eq,
       TRUE XOR FALSE                           AS xor_flag,
       (1 < 2) && (3 < 4)                       AS and_legacy
FROM DUAL;

SET @running_total := 0;
SELECT (@running_total := @running_total + s.metric_value) AS running_assign
FROM (SELECT metric_value FROM metric ORDER BY metric_id) s;

-- MariaDB 11.4+ packages outside sql_mode=ORACLE, built under DELIMITER //
-- while the body carries a literal '$$' splitter bait.
DELIMITER //
CREATE PACKAGE sq_hard_pack
  PROCEDURE ping(OUT x INT);
  FUNCTION fortytwo() RETURNS INT;
END//
CREATE PACKAGE BODY sq_hard_pack
  PROCEDURE ping(OUT x INT)
  BEGIN
    SET x = CHAR_LENGTH('$$') + 40;
  END;
  FUNCTION fortytwo() RETURNS INT
  BEGIN
    RETURN 42;
  END;
END//
DELIMITER ;

CALL sq_hard_pack.ping(@pack_probe);

-- Compound block with ROW TYPE OF / TYPE OF anchored declarations and a
-- ROW-constructor comparison.
DELIMITER $$
BEGIN NOT ATOMIC
  DECLARE rec  ROW TYPE OF metric;
  DECLARE v_id TYPE OF metric.metric_id;
  SELECT metric_id, node_id, measured_on, metric_value
    INTO rec
    FROM metric WHERE metric_id = 1;
  SET v_id = rec.metric_id;
  IF ROW(rec.metric_id, rec.node_id) = ROW(1, 1) THEN
    SET @row_probe = rec.metric_value + v_id;
  ELSE
    SET @row_probe = -1;
  END IF;
END$$
DELIMITER ;

-- MariaDB-only statement shells: SET STATEMENT ... FOR, LIMIT ROWS EXAMINED,
-- and OFFSET ... FETCH ... WITH TIES row limiting.
SET STATEMENT max_statement_time = 0 FOR
SELECT COUNT(*) AS stmt_scoped_count FROM metric;

SELECT metric_id, metric_value
FROM metric
ORDER BY metric_id
LIMIT 2 ROWS EXAMINED 1000;

SELECT metric_id, metric_value
FROM metric
ORDER BY metric_value DESC
OFFSET 0 ROWS FETCH FIRST 2 ROWS WITH TIES;

-- Diagnostic statement family: ANALYZE (executes!), EXPLAIN EXTENDED,
-- EXPLAIN FORMAT=JSON, ANALYZE TABLE, CHECKSUM TABLE.
ANALYZE FORMAT=JSON
SELECT node_id, SUM(metric_value) FROM metric GROUP BY node_id;

EXPLAIN EXTENDED
SELECT m.metric_id FROM metric m WHERE m.metric_value > 10;

EXPLAIN FORMAT=JSON
SELECT node_id FROM metric GROUP BY node_id;

ANALYZE TABLE metric;
CHECKSUM TABLE metric, scratch_rows;

-- View stack: ALGORITHM/DEFINER/SQL SECURITY plus WITH CASCADED CHECK OPTION,
-- then DML routed through the view (insert and delete).
CREATE ALGORITHM = MERGE DEFINER = CURRENT_USER SQL SECURITY INVOKER
VIEW high_metric_v AS
SELECT metric_id, node_id, measured_on, metric_value
FROM metric
WHERE metric_value >= 10
WITH CASCADED CHECK OPTION;

INSERT INTO high_metric_v (metric_id, node_id, measured_on, metric_value)
VALUES (91, 9, '2026-05-01', 99);
CALL sq_hard_assert(
  (SELECT COUNT(*) FROM metric WHERE metric_id = 91) = 1,
  'insert through check-option view'
);
DELETE FROM high_metric_v WHERE metric_id = 91;

-- Engine zoo: Aria with PAGE_CHECKSUM, CREATE TABLE ... LIKE onto a partitioned
-- source, REMOVE PARTITIONING, and EXCHANGE PARTITION handover.
CREATE TABLE aria_notes (
  note_id INT NOT NULL PRIMARY KEY,
  note    VARCHAR(60) NOT NULL
) ENGINE = Aria TRANSACTIONAL = 1 PAGE_CHECKSUM = 1;
INSERT INTO aria_notes VALUES (1, '아리아 엔진'), (2, 'page checksum');

CREATE TABLE exch_metric (
  id INT NOT NULL PRIMARY KEY,
  v  INT NOT NULL
) ENGINE = InnoDB
PARTITION BY RANGE (id) (
  PARTITION pe_lo VALUES LESS THAN (100),
  PARTITION pe_hi VALUES LESS THAN MAXVALUE
);
INSERT INTO exch_metric VALUES (1, 10), (2, 20), (500, 50);

CREATE TABLE exch_swap LIKE exch_metric;
ALTER TABLE exch_swap REMOVE PARTITIONING;
ALTER TABLE exch_metric EXCHANGE PARTITION pe_lo WITH TABLE exch_swap;

-- Multi-table UPDATE and both multi-table DELETE spellings.
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

INSERT INTO prune_rows (id, v)
SELECT metric_id + 20, node_id FROM metric WHERE node_id = 2
RETURNING id, v;

-- JSON path wildcards, JSON_DETAILED/JSON_COMPACT round trip, and JSON_EXISTS.
SELECT JSON_EXTRACT(s.json_value, '$**.v')                        AS wildcard_vals,
       JSON_COMPACT(JSON_DETAILED(s.json_value)) = s.json_value  AS json_roundtrip,
       JSON_EXISTS(s.json_value, '$.items[1].n')                 AS has_second_item
FROM `select` s;

-- PERCENTILE_DISC ordered-set function beside its window siblings.
SELECT DISTINCT node_id,
       PERCENTILE_DISC(0.5) WITHIN GROUP (ORDER BY metric_value)
         OVER (PARTITION BY node_id) AS median_disc
FROM metric
ORDER BY node_id;

-- Third trigger wired in with PRECEDES, then a probing insert.
CREATE TRIGGER scratch_zero
BEFORE INSERT ON scratch_rows FOR EACH ROW
PRECEDES scratch_first
INSERT INTO trigger_log (note) VALUES (CONCAT('zero:', NEW.id));

INSERT INTO scratch_rows (id, v) VALUES (11, 110);

-- HANDLER with an explicit backticked index probe, advisory locks, and DO.
HANDLER prune_rows OPEN AS prune_handle;
HANDLER prune_handle READ `PRIMARY` FIRST;
HANDLER prune_handle READ `PRIMARY` = (8);
HANDLER prune_handle CLOSE;

SELECT GET_LOCK('sq_hard_lock', 0) AS got_lock;
DO RELEASE_LOCK('sq_hard_lock');

-- Prepared statement whose text is assembled in a user variable.
SET @dyn_sql = CONCAT('SELECT COUNT(*) AS dyn_count FROM ',
                      'metric WHERE node_id = ?');
PREPARE dyn_stmt FROM @dyn_sql;
SET @dyn_node = 1;
EXECUTE dyn_stmt USING @dyn_node;
DEALLOCATE PREPARE dyn_stmt;

-- Explicit INTERSECT/EXCEPT DISTINCT spellings.
(SELECT node_id FROM metric)
INTERSECT DISTINCT
(SELECT node_id FROM sq_hard_mariadb_aux.node_lookup)
EXCEPT DISTINCT
(SELECT 999 FROM DUAL)
ORDER BY node_id;

-- ULTRA WAVE 4: native vectors + UUIDv7 + INET6, a live mid-script lexer-mode
-- transition, square-bracket identifiers, a nonstandard multi-symbol delimiter,
-- parameterized cursor FOR / forward and reverse range FOR loops, EXECUTE
-- IMMEDIATE binds, DML RETURNING, Oracle-mode MINUS, and temporal history ranges.
CREATE TABLE sq_hard_w4_vector_asset (
  asset_id    INT NOT NULL,
  asset_uuid  UUID NOT NULL DEFAULT (UUID_v7()),
  asset_name  VARCHAR(30) NOT NULL,
  embedding   VECTOR(3) NOT NULL,
  source_addr INET6 NOT NULL,
  metadata    JSON NOT NULL,
  PRIMARY KEY (asset_id),
  UNIQUE KEY uk_sq_hard_w4_asset_uuid (asset_uuid),
  VECTOR INDEX vx_sq_hard_w4_embedding (embedding)
    M = 4 DISTANCE = cosine,
  CONSTRAINT chk_sq_hard_w4_metadata CHECK (JSON_VALID(metadata))
) ENGINE = InnoDB;

INSERT INTO sq_hard_w4_vector_asset (
  asset_id, asset_name, embedding, source_addr, metadata
) VALUES
  (1, 'alpha', VEC_FromText('[1,2,3]'), '2001:db8::1',
   JSON_OBJECT('tags', JSON_ARRAY('sql', 'vector'))),
  (2, 'beta', VEC_FromText('[2,2,2]'), '10.0.0.2',
   JSON_OBJECT('tags', JSON_ARRAY('nearest'))),
  (3, 'gamma', VEC_FromText('[0,1,0]'), '2001:db8::3',
   JSON_OBJECT('tags', JSON_ARRAY('archive')));

WITH vector_distances AS (
  SELECT a.asset_id, a.asset_name,
         CAST(a.asset_uuid AS CHAR) AS uuid_text,
         CAST(a.source_addr AS CHAR) AS address_text,
         VEC_ToText(a.embedding) AS vector_text,
         VEC_DISTANCE_COSINE(
           a.embedding, VEC_FromText('[1,1,1]')
         ) AS cosine_distance,
         VEC_DISTANCE_EUCLIDEAN(
           a.embedding, VEC_FromText('[1,1,1]')
         ) AS euclidean_distance
  FROM sq_hard_w4_vector_asset a
)
SELECT asset_id, asset_name, uuid_text, address_text, vector_text,
       cosine_distance, euclidean_distance,
       ROW_NUMBER() OVER (
         ORDER BY cosine_distance, asset_id
       ) AS nearest_rank
FROM vector_distances
ORDER BY cosine_distance, asset_id;

-- The client must keep parsing across a real mode transition. MSSQL enables
-- square brackets, ANSI_QUOTES, and PIPES_AS_CONCAT; NO_BACKSLASH_ESCAPES
-- changes the string lexer at the same boundary.
SET @sq_hard_w4_saved_mode = @@SESSION.sql_mode;
SET SESSION sql_mode = CONCAT(
  @sq_hard_w4_saved_mode,
  ',MSSQL,NO_BACKSLASH_ESCAPES'
);

CREATE TABLE [mode table] (
  [select]        INT NOT NULL PRIMARY KEY,
  [semi colon]    VARCHAR(80) NOT NULL,
  [bracket]]name] VARCHAR(80) NOT NULL
);

INSERT INTO [mode table] (
  [select], [semi colon], [bracket]]name]
) VALUES
  (1, 'C:\tmp\semi;--literal', 'left/*middle*/right'),
  (2, 'DELIMITER |!| $$ //', 'bracket]value');

SET @sq_hard_w4_pipe_concat = 'left' || '/' || 'right';
SET @'odd--variable' := 41;

SELECT [select],
       'left' || '/' || 'right' AS [pipe||alias],
       [semi colon],
       [bracket]]name],
       @'odd--variable' + [select] AS [quoted user variable]
FROM [mode table]
ORDER BY [select];

SET SESSION sql_mode = @sq_hard_w4_saved_mode;

-- |!| is intentionally unlike either delimiter used above. Its exact bytes
-- also occur inside a quoted literal in the body.
DELIMITER |!|
CREATE PROCEDURE sq_hard_w4_walk(OUT total_out INT)
outer_w4: BEGIN
  DECLARE asset_total   INT DEFAULT 0;
  DECLARE forward_total INT DEFAULT 0;
  DECLARE reverse_total INT DEFAULT 0;
  DECLARE cur_asset CURSOR(p_floor INT) FOR
    SELECT asset_id
    FROM sq_hard_w4_vector_asset
    WHERE asset_id >= p_floor
    ORDER BY asset_id;

  FOR asset_rec IN cur_asset(2) DO
    SET asset_total = asset_total + asset_rec.asset_id;
  END FOR;

  FOR i IN 1..3 DO
    SET forward_total = forward_total + i;
  END FOR;

  FOR j IN REVERSE 1..3 DO
    SET reverse_total = reverse_total + j;
  END FOR;

  SET @sq_hard_w4_dyn_id = 3;
  SET @sq_hard_w4_dyn_text = 'dynamic; -- /* */';
  EXECUTE IMMEDIATE
    'INSERT INTO `mode table` (`select`, `semi colon`, `bracket]name`)
     VALUES (?, ?, ?)'
    USING @sq_hard_w4_dyn_id,
          @sq_hard_w4_dyn_text,
          @sq_hard_w4_dyn_text;

  SET @sq_hard_w4_delimiter_literal =
    '|!| inside literal; $$ and // stay inert';
  SET total_out = asset_total + forward_total + reverse_total;
END outer_w4|!|
DELIMITER ;

CALL sq_hard_w4_walk(@sq_hard_w4_walk_total);

-- MariaDB returns rows from both the duplicate-update branch and REPLACE /
-- DELETE. The latter pair is rollback-only and must leave no asset 9 behind.
INSERT INTO sq_hard_w4_vector_asset (
  asset_id, asset_uuid, asset_name, embedding, source_addr, metadata
) VALUES (
  1, UUID_v7(), 'alpha-upsert', VEC_FromText('[9,9,9]'), '10.0.0.1',
  JSON_OBJECT('tags', JSON_ARRAY('updated'))
)
ON DUPLICATE KEY UPDATE
  asset_name = VALUES(asset_name),
  metadata = VALUES(metadata)
RETURNING asset_id, asset_name, VEC_ToText(embedding) AS retained_vector;

START TRANSACTION;
REPLACE INTO sq_hard_w4_vector_asset (
  asset_id, asset_name, embedding, source_addr, metadata
) VALUES (
  9, 'temporary', VEC_FromText('[9,0,0]'), '2001:db8::9',
  JSON_OBJECT('tags', JSON_ARRAY('temporary'))
)
RETURNING asset_id, asset_name, source_addr;

DELETE FROM sq_hard_w4_vector_asset
WHERE asset_id = 9
RETURNING asset_id, asset_name, source_addr;
ROLLBACK;

-- The same input switches dialect spelling and switches back. Parenthesized
-- owners force MINUS to be distinguished from arithmetic subtraction.
SET @sq_hard_w4_saved_mode = @@SESSION.sql_mode;
SET SESSION sql_mode = CONCAT(@sq_hard_w4_saved_mode, ',ORACLE');
(SELECT asset_id FROM sq_hard_w4_vector_asset)
MINUS
(SELECT asset_id
 FROM sq_hard_w4_vector_asset
 WHERE asset_id = 2)
ORDER BY asset_id;
SET SESSION sql_mode = @sq_hard_w4_saved_mode;

-- BETWEEN is a third system-time shape after AS OF and ALL. DELETE HISTORY is
-- deliberately a no-op but exercises its statement boundary and timestamp arm.
SELECT id, valid_from, valid_to, v, row_start, row_end
FROM versioned
FOR SYSTEM_TIME BETWEEN
  (@version_cutover - INTERVAL 1 SECOND) AND NOW(6)
ORDER BY id, row_start, valid_from;

DELETE HISTORY FROM versioned
BEFORE SYSTEM_TIME TIMESTAMP '2000-01-01 00:00:00';

-- Wave-3 self-verification.
CALL sq_hard_assert(@pack_probe = 42 AND sq_hard_pack.fortytwo() = 42,
                    'package procedure/function');
CALL sq_hard_assert(@row_probe = 13, 'row type of block');
CALL sq_hard_assert(@running_total = 56, 'user variable running total');
CALL sq_hard_assert(
  (SELECT COUNT(*) FROM exch_metric) = 1 AND
  (SELECT COUNT(*) FROM exch_swap) = 2,
  'exchange partition'
);
CALL sq_hard_assert(
  (SELECT COUNT(*) FROM prune_rows) = 4,
  'multi-table delete/replace/ignore net rows'
);
CALL sq_hard_assert(
  (SELECT COUNT(*) FROM trigger_log
   WHERE note IN ('zero:11', 'first:11', 'second:110')) = 3,
  'precedes/follows trigger trio'
);
CALL sq_hard_assert(
  (SELECT JSON_COMPACT(JSON_EXTRACT(json_value, '$**.v')) FROM `select`) = '[10,20]',
  'json wildcard path'
);
CALL sq_hard_assert(
  (SELECT COUNT(*) FROM aria_notes) = 2,
  'aria engine rows'
);

-- Extension self-verification.
CALL sq_hard_assert(@spin_total = 16, 'repeat/while spin total');
CALL sq_hard_assert(
  (SELECT COUNT(*) FROM scratch_rows) = 6,
  'scratch rows after returning-delete/xa/trigger/wave3 inserts'
);
CALL sq_hard_assert(
  (SELECT v FROM scratch_rows WHERE id = 1) = 110,
  'values() upsert result'
);
CALL sq_hard_assert(
  (SELECT COUNT(*) FROM trigger_log WHERE note IN ('first:9', 'second:90')) = 2,
  'ordered trigger pair'
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
  (SELECT COLUMN_GET(COLUMN_CREATE('k', 42), 'k' AS INT)) = 42,
  'dynamic columns'
);

-- Wave-4 self-verification.
CALL sq_hard_assert(
  (SELECT COUNT(*) FROM sq_hard_w4_vector_asset) = 3 AND
  (SELECT COUNT(*) FROM sq_hard_w4_vector_asset WHERE asset_id = 9) = 0,
  'vector rows + returning rollback'
);
CALL sq_hard_assert(
  (SELECT asset_id
   FROM sq_hard_w4_vector_asset
   ORDER BY VEC_DISTANCE_COSINE(
              embedding, VEC_FromText('[1,1,1]')
            ),
            asset_id
   LIMIT 1) = 2,
  'cosine nearest vector'
);
CALL sq_hard_assert(
  (SELECT COUNT(*)
   FROM sq_hard_w4_vector_asset
   WHERE SUBSTRING(CAST(asset_uuid AS CHAR), 15, 1) = '7') = 3,
  'uuid v7 defaults'
);
CALL sq_hard_assert(
  (SELECT asset_name FROM sq_hard_w4_vector_asset WHERE asset_id = 1) =
    'alpha-upsert' AND
  JSON_VALUE(
    (SELECT metadata
     FROM sq_hard_w4_vector_asset
     WHERE asset_id = 1),
    '$.tags[0]'
  ) = 'updated',
  'upsert returning branch'
);
CALL sq_hard_assert(
  (SELECT COUNT(*) FROM `mode table`) = 3 AND
  (SELECT `bracket]name`
   FROM `mode table`
   WHERE `select` = 3) = 'dynamic; -- /* */',
  'mode transition + dynamic insert'
);
CALL sq_hard_assert(
  @sq_hard_w4_walk_total = 17 AND
  @sq_hard_w4_pipe_concat = 'left/right' AND
  @'odd--variable' = 41,
  'parameter cursor + loop + lexer mode'
);
CALL sq_hard_assert(
  @sq_hard_w4_delimiter_literal =
    '|!| inside literal; $$ and // stay inert',
  'custom delimiter literal'
);
CALL sq_hard_assert(
  (SELECT COUNT(*)
   FROM versioned
   FOR SYSTEM_TIME BETWEEN
     (@version_cutover - INTERVAL 1 SECOND) AND NOW(6)) >= 6,
  'system-time between range'
);

-- ULTRA WAVE 5: optimizer-steering syntax that only exists between FROM and ON,
-- ROLLUP hidden inside a derived table, the 11.x/12.x JSON function zoo,
-- MariaDB-only DDL spellings, spatial and full-text index families, temporary
-- tables, row-locking modifiers, and routine attribute stacking.

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

SELECT SQL_CALC_FOUND_ROWS h.hint_id, h.bucket
FROM sq_hard_w5_hint AS h IGNORE INDEX FOR ORDER BY (ix_sq_hard_w5_hint_bucket)
ORDER BY h.bucket, h.hint_id
LIMIT 2;

SET @sq_hard_w5_found = FOUND_ROWS();

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

-- W5-B: quantified subqueries, a row-constructor IN list, WITH ROLLUP tucked
-- inside a derived table (MariaDB refuses ROLLUP beside ORDER BY), and an ORDER
-- BY that mixes FIELD() with an explicit COLLATE.
SELECT r.bucket, r.is_total, r.bucket_total
FROM (SELECT bucket,
             (bucket IS NULL)       AS is_total,
             ROUND(SUM(weight), 2)  AS bucket_total
      FROM sq_hard_w5_hint
      WHERE weight >= ANY (SELECT weight FROM sq_hard_w5_hint WHERE node_id = 2)
        AND (node_id, bucket) IN ((1, 'alpha'), (1, 'beta'), (2, 'gamma'))
        AND weight <> ALL (SELECT 999)
        AND EXISTS (SELECT 1 FROM metric WHERE metric.node_id = sq_hard_w5_hint.node_id)
      GROUP BY bucket WITH ROLLUP
      HAVING bucket_total > 0) AS r
ORDER BY r.is_total,
         FIELD(r.bucket, 'gamma', 'beta', 'alpha'),
         r.bucket COLLATE utf8mb4_bin;

-- W5-C: the 11.x/12.x JSON zoo. Every one of these takes a path or a document
-- literal whose braces and $ sigils must never leak into the SQL lexer.
CREATE TABLE sq_hard_w5_doc (
  doc_id  INT NOT NULL,
  body    JSON NOT NULL CHECK (JSON_VALID(body)),
  tag_set JSON NOT NULL,
  PRIMARY KEY (doc_id)
) ENGINE = InnoDB;

INSERT INTO sq_hard_w5_doc VALUES
  (1, '{"name":"alpha","score":10,"nested":{"deep":[1,2,3]}}', '[1,2,3]'),
  (2, '{"score":10,"name":"alpha","nested":{"deep":[1,2,3]}}', '[3,4,5]');

SELECT d.doc_id,
       JSON_OVERLAPS(d.tag_set, '[3,9]')                       AS overlaps_three,
       JSON_ARRAY_INTERSECT('[2,3,4]', d.tag_set)              AS shared_tags,
       JSON_EQUALS(d.body, '{"score":10,"name":"alpha",
                             "nested":{"deep":[1,2,3]}}')      AS same_document,
       JSON_SCHEMA_VALID('{"type":"object",
                           "required":["name"]}', d.body)      AS schema_ok,
       JSON_KEY_VALUE(d.body, '$')                             AS key_values,
       JSON_OBJECT_TO_ARRAY(JSON_OBJECT('k', d.doc_id))        AS object_pairs,
       JSON_OBJECT_FILTER_KEYS(d.body, '["name"]')             AS kept_keys,
       JSON_ARRAY_APPEND(d.tag_set, '$', 99)                   AS appended,
       JSON_ARRAY_INSERT(d.tag_set, '$[0]', 0)                 AS inserted
FROM sq_hard_w5_doc AS d
ORDER BY d.doc_id;

SELECT JSON_NORMALIZE('{"b":2,"a":1}') = JSON_NORMALIZE('{"a":1,"b":2}')
         AS normalized_match;

-- W5-D: MariaDB-only DDL spellings. CREATE OR REPLACE TABLE, an INVISIBLE
-- column that SELECT * must skip, a parenthesized expression DEFAULT, a named
-- CHECK, IF NOT EXISTS / IF EXISTS clause tails, RENAME COLUMN, and an
-- ALGORITHM/LOCK pair on the same ALTER.
CREATE OR REPLACE TABLE sq_hard_w5_ddl (
  ddl_id     INT NOT NULL,
  visible_v  VARCHAR(20) DEFAULT (CONCAT('v', '-', 'default')),
  hidden_v   INT INVISIBLE DEFAULT 7,
  PRIMARY KEY (ddl_id),
  CONSTRAINT ck_sq_hard_w5_ddl CHECK (ddl_id > 0)
) ENGINE = InnoDB;

INSERT INTO sq_hard_w5_ddl (ddl_id) VALUES (1), (2);

SELECT * FROM sq_hard_w5_ddl ORDER BY ddl_id;

SELECT ddl_id, visible_v, hidden_v FROM sq_hard_w5_ddl ORDER BY ddl_id;

ALTER TABLE sq_hard_w5_ddl
  ADD COLUMN IF NOT EXISTS added_v INT NOT NULL DEFAULT 0,
  ALGORITHM = INSTANT;

ALTER TABLE sq_hard_w5_ddl
  RENAME COLUMN added_v TO renamed_v,
  ALGORITHM = INPLACE,
  LOCK = NONE;

ALTER TABLE sq_hard_w5_ddl DROP CONSTRAINT IF EXISTS ck_sq_hard_w5_ddl;

DROP INDEX IF EXISTS ix_never_created ON sq_hard_w5_ddl;

CREATE OR REPLACE TABLE sq_hard_w5_swap_a (swap_v INT) ENGINE = InnoDB;
CREATE OR REPLACE TABLE sq_hard_w5_swap_b (swap_v INT) ENGINE = InnoDB;
INSERT INTO sq_hard_w5_swap_a VALUES (1);
INSERT INTO sq_hard_w5_swap_b VALUES (2);

RENAME TABLE sq_hard_w5_swap_a TO sq_hard_w5_swap_t,
             sq_hard_w5_swap_b TO sq_hard_w5_swap_a,
             sq_hard_w5_swap_t TO sq_hard_w5_swap_b;

-- W5-E: geometry columns with a spatial index, WKT constructors, and predicate
-- plus measurement functions over them.
CREATE TABLE sq_hard_w5_geo (
  geo_id INT NOT NULL,
  label  VARCHAR(20) NOT NULL,
  spot   POINT NOT NULL,
  zone   POLYGON NOT NULL,
  PRIMARY KEY (geo_id),
  SPATIAL INDEX sx_sq_hard_w5_geo (spot)
) ENGINE = InnoDB;

INSERT INTO sq_hard_w5_geo VALUES
  (1, 'inner', ST_GeomFromText('POINT(1 1)'),
   ST_GeomFromText('POLYGON((0 0,0 5,5 5,5 0,0 0))')),
  (2, 'outer', ST_PointFromText('POINT(9 9)'),
   ST_PolygonFromText('POLYGON((8 8,8 10,10 10,10 8,8 8))'));

SELECT g.geo_id,
       ST_X(g.spot)                          AS spot_x,
       ST_AsText(ST_Centroid(g.zone))        AS zone_centre,
       ST_Contains(g.zone, g.spot)           AS zone_holds_spot,
       ROUND(ST_Distance(g.spot, POINT(0, 0)), 4) AS origin_gap,
       ST_GeometryType(g.zone)               AS zone_kind
FROM sq_hard_w5_geo AS g
ORDER BY g.geo_id;

-- W5-F: full-text index with all three search modes. The boolean-mode operators
-- sit inside a string literal where they must stay inert.
CREATE TABLE sq_hard_w5_ft (
  ft_id INT NOT NULL,
  body  TEXT NOT NULL,
  PRIMARY KEY (ft_id),
  FULLTEXT KEY fx_sq_hard_w5_ft (body)
) ENGINE = InnoDB;

INSERT INTO sq_hard_w5_ft VALUES
  (1, 'hostile grammar stress engine'),
  (2, 'gentle grammar coverage baseline');

SELECT f.ft_id,
       ROUND(MATCH(f.body) AGAINST ('grammar'), 4) AS natural_score
FROM sq_hard_w5_ft AS f
ORDER BY f.ft_id;

SELECT f.ft_id
FROM sq_hard_w5_ft AS f
WHERE MATCH(f.body) AGAINST ('+hostile -gentle' IN BOOLEAN MODE)
ORDER BY f.ft_id;

SELECT f.ft_id
FROM sq_hard_w5_ft AS f
WHERE MATCH(f.body) AGAINST ('engine' WITH QUERY EXPANSION)
ORDER BY f.ft_id;

-- W5-G: a MEMORY temporary table, a multi-target SELECT ... INTO, and the three
-- row-locking modifier spellings inside one explicit transaction.
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

START TRANSACTION;

SELECT hint_id FROM sq_hard_w5_hint WHERE node_id = 1 ORDER BY hint_id
FOR UPDATE SKIP LOCKED;

SELECT hint_id FROM sq_hard_w5_hint WHERE node_id = 2 ORDER BY hint_id
FOR UPDATE NOWAIT;

SELECT hint_id FROM sq_hard_w5_hint WHERE bucket = 'alpha' ORDER BY hint_id
LOCK IN SHARE MODE;

COMMIT;

-- W5-H: routine attribute stacking plus IN / INOUT / OUT parameter modes; the
-- COMMENT payload carries delimiter bait that must stay inside the literal.
DELIMITER //
CREATE OR REPLACE FUNCTION sq_hard_w5_triple(p_value INT)
  RETURNS INT
  DETERMINISTIC
  CONTAINS SQL
  SQL SECURITY INVOKER
  COMMENT 'wave5 // $$ ; attribute stack'
BEGIN
  RETURN p_value * 3;
END//

CREATE OR REPLACE PROCEDURE sq_hard_w5_accumulate(IN    p_seed  INT,
                                                  INOUT p_acc   INT,
                                                  OUT   p_label VARCHAR(32))
  MODIFIES SQL DATA
  SQL SECURITY DEFINER
  COMMENT 'wave5 out params'
BEGIN
  SET p_acc = p_acc + sq_hard_w5_triple(p_seed);
  INSERT INTO sq_hard_w5_tmp (tmp_id, tmp_note)
  VALUES (100 + p_seed, CONCAT('acc-', p_acc))
  ON DUPLICATE KEY UPDATE tmp_note = VALUES(tmp_note);
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
  AND @sq_hard_w5_found = 4,
  'join steering + calc found rows'
);
CALL sq_hard_assert(
  (SELECT ROUND(SUM(r.bucket_total), 2)
   FROM (SELECT bucket, ROUND(SUM(weight), 2) AS bucket_total
         FROM sq_hard_w5_hint
         WHERE (node_id, bucket) IN ((1, 'alpha'), (1, 'beta'), (2, 'gamma'))
         GROUP BY bucket WITH ROLLUP) AS r) = 77.00,
  'rollup inside derived table'
);
CALL sq_hard_assert(
  (SELECT JSON_OVERLAPS(tag_set, '[3,9]') FROM sq_hard_w5_doc WHERE doc_id = 1) = 1
  AND (SELECT JSON_EQUALS(
                 (SELECT body FROM sq_hard_w5_doc WHERE doc_id = 1),
                 (SELECT body FROM sq_hard_w5_doc WHERE doc_id = 2))) = 1
  AND JSON_NORMALIZE('{"b":2,"a":1}') = JSON_NORMALIZE('{"a":1,"b":2}'),
  'json zoo equality'
);
CALL sq_hard_assert(
  (SELECT COUNT(*)
   FROM information_schema.COLUMNS
   WHERE TABLE_SCHEMA = DATABASE()
     AND TABLE_NAME = 'sq_hard_w5_ddl'
     AND COLUMN_NAME = 'renamed_v') = 1
  AND (SELECT hidden_v FROM sq_hard_w5_ddl WHERE ddl_id = 1) = 7
  AND (SELECT visible_v FROM sq_hard_w5_ddl WHERE ddl_id = 2) = 'v-default'
  AND (SELECT swap_v FROM sq_hard_w5_swap_a) = 2
  AND (SELECT swap_v FROM sq_hard_w5_swap_b) = 1,
  'ddl spellings + rename swap'
);
CALL sq_hard_assert(
  (SELECT COUNT(*)
   FROM sq_hard_w5_geo AS g
   WHERE ST_Contains(g.zone, g.spot)) = 2
  AND (SELECT ST_AsText(ST_Centroid(zone))
       FROM sq_hard_w5_geo WHERE geo_id = 1) = 'POINT(2.5 2.5)',
  'spatial containment + centroid'
);
CALL sq_hard_assert(
  (SELECT COUNT(*)
   FROM sq_hard_w5_ft
   WHERE MATCH(body) AGAINST ('+hostile -gentle' IN BOOLEAN MODE)) = 1
  AND (SELECT COUNT(*)
       FROM sq_hard_w5_ft
       WHERE MATCH(body) AGAINST ('engine' WITH QUERY EXPANSION)) = 2,
  'fulltext boolean + query expansion'
);
CALL sq_hard_assert(
  @sq_hard_w5_tmp_rows = 4
  AND @sq_hard_w5_tmp_max = 4
  AND @sq_hard_w5_acc = 13
  AND @sq_hard_w5_label = 'acc=13'
  AND (SELECT tmp_note FROM sq_hard_w5_tmp WHERE tmp_id = 104) = 'acc-13',
  'temporary table + out params'
);

--------------------------------------------------------------------------------
-- ULTRA WAVE 6: named WINDOW definitions reused and extended by frame clauses,
-- a recursive CTE closed by MariaDB's CYCLE clause, partition maintenance DDL,
-- roles, the DML modifier zoo, a condition/handler/RESIGNAL chain, CTAS with
-- table rewrites, and a pure lexer round where a comment sits between nearly
-- every token.
--------------------------------------------------------------------------------

-- W6-A: one WINDOW clause defines two named windows; the frame-extended forms
-- `(w RANGE ...)` and `(w ROWS ...)` inherit the partition and ordering.
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
       SUM(s.amount)        OVER (w_ordered RANGE BETWEEN 4 PRECEDING
                                                      AND CURRENT ROW) AS window_4d,
       LAST_VALUE(s.amount) OVER (w_ordered ROWS BETWEEN UNBOUNDED PRECEDING
                                                     AND UNBOUNDED FOLLOWING) AS last_amount
FROM sq_hard_w6_series AS s
WINDOW w_bucket  AS (PARTITION BY s.bucket),
       w_ordered AS (PARTITION BY s.bucket ORDER BY TO_DAYS(s.taken_on))
ORDER BY s.series_id;

-- W6-B: a cyclic edge set walked by a recursive CTE that would never terminate
-- without MariaDB's CYCLE ... RESTRICT clause between the CTE and the query.
CREATE TABLE sq_hard_w6_edge (
  src INT NOT NULL,
  dst INT NOT NULL,
  PRIMARY KEY (src, dst)
) ENGINE = InnoDB;

INSERT INTO sq_hard_w6_edge (src, dst) VALUES (1, 2), (2, 3), (3, 1), (3, 4);

WITH RECURSIVE walk (node, depth, path) AS (
  SELECT 1, 0, CAST('1' AS CHAR(64))
  UNION ALL
  SELECT e.dst, w.depth + 1, CONCAT(w.path, '>', e.dst)
  FROM walk AS w
       JOIN sq_hard_w6_edge AS e ON e.src = w.node
)
CYCLE node RESTRICT
SELECT COUNT(*) AS walk_rows, MAX(depth) AS max_depth INTO @sq_hard_w6_walk_rows,
       @sq_hard_w6_walk_depth
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

GRANT SELECT ON sq_hard_mariadb.sq_hard_w6_series TO sq_hard_w6_role;

GRANT sq_hard_w6_role TO CURRENT_USER;

SET ROLE sq_hard_w6_role;

SELECT CURRENT_ROLE() AS active_role;

SET ROLE NONE;

REVOKE SELECT ON sq_hard_mariadb.sq_hard_w6_series FROM sq_hard_w6_role;

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
VALUES (1, 'three')
ON DUPLICATE KEY UPDATE tag = CONCAT(VALUES(tag), '-merged');

UPDATE IGNORE sq_hard_w6_dml SET tag = 'two-replaced' WHERE dml_id = 1;

DELETE LOW_PRIORITY QUICK IGNORE FROM sq_hard_w6_dml WHERE dml_id = 99;

SELECT dml_id, tag FROM sq_hard_w6_dml ORDER BY dml_id;

-- W6-F: named condition, a CONTINUE handler for it, a nested block whose EXIT
-- handler re-raises through RESIGNAL, and diagnostics read back out.
DELIMITER //
CREATE OR REPLACE PROCEDURE sq_hard_w6_guard(IN  p_value INT,
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

  SET p_state = 'clean';

  IF p_value < 0 THEN
    SIGNAL sq_hard_w6_negative SET MESSAGE_TEXT = 'negative input',
                                   MYSQL_ERRNO = 1451;
  END IF;

  BEGIN
    DECLARE CONTINUE HANDLER FOR SQLEXCEPTION
    BEGIN
      SET p_state = CONCAT(p_state, '/inner');
    END;

    IF p_value = 0 THEN
      SIGNAL SQLSTATE '45002' SET MESSAGE_TEXT = 'zero input';
    END IF;
  END;
END//
DELIMITER ;

CALL sq_hard_w6_guard(-1, @sq_hard_w6_state_neg);
CALL sq_hard_w6_guard(0, @sq_hard_w6_state_zero);
CALL sq_hard_w6_guard(1, @sq_hard_w6_state_ok);

SELECT @sq_hard_w6_state_neg  AS state_negative,
       @sq_hard_w6_state_zero AS state_zero,
       @sq_hard_w6_state_ok   AS state_ok;

-- W6-G: CREATE TABLE ... AS SELECT, then three ALTER spellings that rewrite the
-- whole table: charset conversion, an index with an explicit algorithm, and a
-- physical ORDER BY.
CREATE TABLE sq_hard_w6_ctas
  ENGINE = InnoDB
  AS
SELECT s.bucket, SUM(s.amount) AS bucket_total, COUNT(*) AS bucket_rows
FROM sq_hard_w6_series AS s
GROUP BY s.bucket;

ALTER TABLE sq_hard_w6_ctas CONVERT TO CHARACTER SET utf8mb4 COLLATE utf8mb4_bin;

ALTER TABLE sq_hard_w6_ctas ADD INDEX sq_hard_w6_ctas_ix (bucket) USING BTREE;

ALTER TABLE sq_hard_w6_ctas ORDER BY bucket;

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

SELECT s.series_id||'' AS packed_id,CONCAT(s.bucket,'-',CAST(s.amount AS CHAR))AS packed_line,(s.amount*2)-(s.series_id*3)+MOD(s.series_id,2)AS packed_math FROM sq_hard_w6_series s WHERE s.series_id IN(1,2)AND s.amount BETWEEN 1 AND 999 ORDER BY s.series_id;

sElEcT	CoUnT(*)	As	mixed_case_rows	FrOm	sq_hard_w6_series	WhErE	bucket	iS	NoT	nUlL;

-- W6-I: wave-6 self-verification.
CALL sq_hard_assert(
  (SELECT SUM(window_4d)
   FROM (SELECT SUM(s.amount) OVER (PARTITION BY s.bucket
                                    ORDER BY TO_DAYS(s.taken_on)
                                    RANGE BETWEEN 4 PRECEDING
                                              AND CURRENT ROW) AS window_4d
         FROM sq_hard_w6_series AS s) AS framed) = 110.00,
  'named window frames'
);
CALL sq_hard_assert(
  @sq_hard_w6_walk_rows = 4 AND @sq_hard_w6_walk_depth = 3,
  'recursive cycle walk'
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
  @sq_hard_w6_state_neg = 'handled-1451'
  AND @sq_hard_w6_state_zero = 'clean/inner'
  AND @sq_hard_w6_state_ok = 'clean',
  'condition handler chain'
);
CALL sq_hard_assert(
  (SELECT ROUND(SUM(bucket_total), 2) FROM sq_hard_w6_ctas) = 100.00
  AND (SELECT COUNT(*) FROM sq_hard_w6_ctas) = 2,
  'ctas + table rewrites'
);

--------------------------------------------------------------------------------
-- ULTRA WAVE 7: an application-time period closed over by a WITHOUT OVERLAPS
-- primary key, a system-versioned table partitioned BY SYSTEM_TIME INTERVAL,
-- index/column maintenance (descending key part, ALTER INDEX IGNORED, ALTER
-- COLUMN SET/DROP DEFAULT), the SHOW/FLUSH/CHECKSUM family, the window
-- functions no earlier wave reached, the regexp trio and JSON mutation family,
-- a ROW-typed local variable with a SQLWARNING handler, and a lexer round.
--
-- Dialect delta pinned here on purpose: MariaDB has no three-argument LAG,
-- which MySQL does accept (see test_mysql/final_hardcore.sql W7-D).
--------------------------------------------------------------------------------
--------------------------------------------------------------------------------
-- W7-A: an application-time period whose primary key closes over it with
-- WITHOUT OVERLAPS, so the period name sits in a key-part slot where only
-- column names are otherwise legal, and UPDATE/DELETE FOR PORTION OF split
-- rows against it.
--------------------------------------------------------------------------------
CREATE TABLE sq_hard_w7_lease (
  lease_id  INT NOT NULL,
  tenant    VARCHAR(20) NOT NULL,
  starts_on DATE NOT NULL,
  ends_on   DATE NOT NULL,
  rate      DECIMAL(8,2) NOT NULL,
  PERIOD FOR term (starts_on, ends_on),
  PRIMARY KEY (lease_id, term WITHOUT OVERLAPS)
) ENGINE = InnoDB;

INSERT INTO sq_hard_w7_lease (lease_id, tenant, starts_on, ends_on, rate)
VALUES (1, 'alpha', '2024-01-01', '2024-12-31', 100.00),
       (2, 'beta',  '2024-03-01', '2024-06-30', 200.00);

UPDATE sq_hard_w7_lease
   FOR PORTION OF term FROM '2024-04-01' TO '2024-07-01'
   SET rate = rate + 50
 WHERE lease_id = 1;

DELETE FROM sq_hard_w7_lease
   FOR PORTION OF term FROM '2024-05-01' TO '2024-05-15'
 WHERE lease_id = 2;

SELECT lease_id, tenant, starts_on, ends_on, rate
FROM sq_hard_w7_lease
ORDER BY lease_id, starts_on;

--------------------------------------------------------------------------------
-- W7-B: a system-versioned table partitioned BY SYSTEM_TIME INTERVAL, where
-- INTERVAL/LIMIT/CURRENT read as clause keywords inside the partition list and
-- the partition names are definition slots.
--------------------------------------------------------------------------------
CREATE TABLE sq_hard_w7_ver (
  ver_id INT PRIMARY KEY,
  state  VARCHAR(12) NOT NULL
) ENGINE = InnoDB
  WITH SYSTEM VERSIONING
  PARTITION BY SYSTEM_TIME INTERVAL 10 YEAR
  STARTS '2020-01-01 00:00:00' (
    PARTITION p_hist HISTORY,
    PARTITION p_cur  CURRENT
  );

INSERT INTO sq_hard_w7_ver (ver_id, state) VALUES (1, 'created');
UPDATE sq_hard_w7_ver SET state = 'changed' WHERE ver_id = 1;

SELECT COUNT(*) AS current_rows FROM sq_hard_w7_ver;
SELECT COUNT(*) AS all_rows
FROM sq_hard_w7_ver FOR SYSTEM_TIME ALL;

--------------------------------------------------------------------------------
-- W7-C: index and column maintenance whose action words are all keywords
-- elsewhere -- a descending key part, ALTER INDEX ... IGNORED (the optimizer
-- keeps the index but refuses to use it), and ALTER COLUMN SET/DROP DEFAULT.
--------------------------------------------------------------------------------
CREATE TABLE sq_hard_w7_idx (
  idx_id   INT NOT NULL PRIMARY KEY,
  bucket   VARCHAR(10) NOT NULL,
  measured INT NOT NULL DEFAULT 0,
  KEY ix_desc (bucket, measured DESC)
) ENGINE = InnoDB PAGE_COMPRESSED = 1;

INSERT INTO sq_hard_w7_idx (idx_id, bucket, measured)
VALUES (1, 'a', 10), (2, 'a', 30), (3, 'b', 20);

ALTER TABLE sq_hard_w7_idx ALTER INDEX ix_desc IGNORED;
ALTER TABLE sq_hard_w7_idx ALTER COLUMN measured SET DEFAULT 7;

INSERT INTO sq_hard_w7_idx (idx_id, bucket) VALUES (4, 'b');

ALTER TABLE sq_hard_w7_idx ALTER COLUMN measured DROP DEFAULT;
ALTER TABLE sq_hard_w7_idx ALTER INDEX ix_desc NOT IGNORED;

SELECT idx_id, bucket, measured FROM sq_hard_w7_idx ORDER BY idx_id;

--------------------------------------------------------------------------------
-- W7-D: the SHOW / FLUSH / CHECKSUM family. These are statements whose second
-- word is an object-kind keyword, and SHOW ... LIKE takes a pattern where a
-- table name would otherwise sit.
--------------------------------------------------------------------------------
SHOW CREATE TABLE sq_hard_w7_idx;
SHOW INDEX FROM sq_hard_w7_idx FROM sq_hard_mariadb;
SHOW COLUMNS FROM sq_hard_w7_lease LIKE 'rate%';
SHOW TABLE STATUS LIKE 'sq\_hard\_w7\_idx';
FLUSH LOCAL TABLES sq_hard_w7_idx;
CHECKSUM TABLE sq_hard_w7_idx, sq_hard_w7_lease QUICK;

--------------------------------------------------------------------------------
-- W7-E: the window functions no earlier wave reached, plus a named window that
-- a later frame-extended reference re-anchors and a three-argument LAG.
--------------------------------------------------------------------------------
SELECT i.idx_id,
       i.bucket,
       i.measured,
       ROUND(CUME_DIST() OVER w_ordered, 4)              AS cume,
       ROUND(PERCENT_RANK() OVER w_ordered, 4)           AS pct_rank,
       -- No FROM FIRST / FROM LAST modifier on NTH_VALUE here: MariaDB rejects
       -- it and MySQL 8.0 reports it unimplemented, so the frame-extended named
       -- window carries the whole partition instead.
       NTH_VALUE(i.measured, 2) OVER (
         w_ordered ROWS BETWEEN UNBOUNDED PRECEDING AND UNBOUNDED FOLLOWING
       )                                                 AS second_value,
       -- Two-argument LAG only: MariaDB rejects the third default argument
       -- that MySQL accepts, so the fallback is spelled with COALESCE.
       COALESCE(LAG(i.measured, 2) OVER w_ordered, -1)   AS lag_two_default,
       NTILE(2) OVER w_ordered                           AS half
FROM sq_hard_w7_idx i
WINDOW w_ordered AS (PARTITION BY i.bucket ORDER BY i.measured)
ORDER BY i.idx_id;

--------------------------------------------------------------------------------
-- W7-F: the regular-expression trio and the JSON mutation family, whose
-- arguments alternate between a path literal and a value.
--------------------------------------------------------------------------------
SELECT REGEXP_REPLACE('wave-7-maria', '([a-z]+)-([0-9]+)', '\\2:\\1') AS replaced,
       REGEXP_SUBSTR('wave-7-maria', '[0-9]+')                        AS matched,
       REGEXP_INSTR('wave-7-maria', '[0-9]+')                         AS matched_at;

SELECT JSON_DETAILED(
         JSON_MERGE_PATCH(
           JSON_SET(
             JSON_REMOVE(
               JSON_ARRAY_APPEND(
                 JSON_OBJECT('tags', JSON_ARRAY('sql'), 'drop', 1),
                 '$.tags', 'json'),
               '$.drop'),
             '$.depth', JSON_LENGTH(JSON_ARRAY(1, 2, 3))),
           JSON_OBJECT('patched', TRUE))
       )                                              AS mutated,
       JSON_TYPE(JSON_EXTRACT('{"a":[1]}', '$.a'))    AS extracted_type,
       JSON_VALID('{"a":1}')                          AS is_valid,
       JSON_UNQUOTE(JSON_QUOTE('quoted'))             AS round_tripped;

--------------------------------------------------------------------------------
-- W7-G: a ROW-typed local variable, VALUE() as the MariaDB spelling of the
-- upsert source, and a handler that fires on SQLWARNING rather than an error.
--------------------------------------------------------------------------------
CREATE TABLE sq_hard_w7_up (
  up_id INT PRIMARY KEY,
  hits  INT NOT NULL,
  label VARCHAR(20)
) ENGINE = InnoDB;

INSERT INTO sq_hard_w7_up (up_id, hits, label) VALUES (1, 1, 'first');

INSERT INTO sq_hard_w7_up (up_id, hits, label)
VALUES (1, 10, 'second'), (2, 20, 'new')
ON DUPLICATE KEY UPDATE hits  = hits + VALUE(hits),
                        label = CONCAT(VALUE(label), '/merged');

DELIMITER $$
CREATE PROCEDURE sq_hard_w7_rowvar(OUT out_text VARCHAR(60))
BEGIN
  DECLARE row_var ROW(up_id INT, hits INT, label VARCHAR(20));
  DECLARE warn_seen INT DEFAULT 0;
  DECLARE CONTINUE HANDLER FOR SQLWARNING SET warn_seen = warn_seen + 1;

  SELECT up_id, hits, label INTO row_var FROM sq_hard_w7_up WHERE up_id = 1;
  SET @truncated = CAST('12abc' AS DECIMAL(4,0));

  SET out_text = CONCAT_WS('/', row_var.up_id, row_var.hits, row_var.label,
                           warn_seen);
END$$
DELIMITER ;

CALL sq_hard_w7_rowvar(@sq_hard_w7_rowvar);
SELECT @sq_hard_w7_rowvar AS rowvar_text;

--------------------------------------------------------------------------------
-- W7-H: lexer round. A dash banner (legal in MariaDB, rejected by MySQL), an
-- executable comment, backtick identifiers that spell statements, comment and
-- terminator lookalikes inside literals, and one unspaced line.
--------------------------------------------------------------------------------
------------------------------------------------------------ banner ------------
SELECT /*!100000 1 + */ 1                                   AS versioned_comment,
       'a -- not a comment /* nor this */; still text'       AS bait_text,
       "double-quoted string in default mode"               AS dq_text,
       0x4D61726961                                         AS hex_literal,
       b'1011'                                              AS bit_literal,
       1.5e-3                                               AS sci_literal,
       `select`.`from`                                       AS quoted_path
FROM (SELECT 1 AS `from`) AS `select`;

SELECT(1+2)*3-4/2 AS crammed,MOD(7,3)modded,ABS(-8)absed FROM DUAL WHERE 1<>2;

sElEcT cAsE wHeN 1=1 tHeN 'mixed' eLsE 'case' eNd AS alternating;

--------------------------------------------------------------------------------
-- W7-I: wave-7 self-verification.
--------------------------------------------------------------------------------
CALL sq_hard_assert(
  (SELECT COUNT(*) FROM sq_hard_w7_lease) = 5
  AND (SELECT rate FROM sq_hard_w7_lease
       WHERE lease_id = 1 AND starts_on = '2024-04-01') = 150.00,
  'application-time portions'
);
CALL sq_hard_assert(
  (SELECT COUNT(*) FROM sq_hard_w7_ver) = 1
  AND (SELECT COUNT(*) FROM sq_hard_w7_ver FOR SYSTEM_TIME ALL) = 2,
  'system versioning partitions'
);
CALL sq_hard_assert(
  (SELECT measured FROM sq_hard_w7_idx WHERE idx_id = 4) = 7,
  'alter column default'
);
CALL sq_hard_assert(
  (SELECT hits FROM sq_hard_w7_up WHERE up_id = 1) = 11
  AND (SELECT label FROM sq_hard_w7_up WHERE up_id = 1) = 'second/merged',
  'value() upsert'
);
CALL sq_hard_assert(
  @sq_hard_w7_rowvar = '1/11/second/merged/1',
  CONCAT('row variable + warning handler: ', @sq_hard_w7_rowvar)
);

--------------------------------------------------------------------------------
-- ULTRA WAVE 8: a user-defined aggregate driven by FETCH GROUP NEXT ROW, the
-- SET form of INSERT/REPLACE, CTAS carrying a duplicate-row modifier, account
-- and privilege administration, the table-maintenance family with its option
-- words, a MERGE-engine table over MyISAM children, COMPRESSED columns with
-- Aria table options, EXECUTE IMMEDIATE ... USING, anchored declarations
-- (TYPE OF / ROW TYPE OF), set-operation algebra with ALL, and a literal round.
--------------------------------------------------------------------------------
--------------------------------------------------------------------------------
-- W8-A: a custom aggregate. `CREATE AGGREGATE FUNCTION` bodies pull their rows
-- with `FETCH GROUP NEXT ROW`, a statement that exists nowhere else and needs a
-- NOT FOUND handler to end the group, so the parser sees an infinite LOOP whose
-- only exit is a handler RETURN.
--------------------------------------------------------------------------------
CREATE TABLE sq_hard_w8_fact (
  fact_id INT NOT NULL PRIMARY KEY,
  bucket  VARCHAR(10) NOT NULL,
  factor  INT NOT NULL
) ENGINE = InnoDB;

INSERT INTO sq_hard_w8_fact (fact_id, bucket, factor)
VALUES (1, 'alpha', 2), (2, 'alpha', 3), (3, 'alpha', 3),
       (4, 'beta', 5), (5, 'beta', 7);

DELIMITER $$
CREATE AGGREGATE FUNCTION sq_hard_w8_product(operand INT) RETURNS BIGINT
  DETERMINISTIC
  CONTAINS SQL
BEGIN
  DECLARE running BIGINT DEFAULT 1;
  DECLARE CONTINUE HANDLER FOR NOT FOUND RETURN running;

  product_loop: LOOP
    FETCH GROUP NEXT ROW;
    IF operand IS NULL THEN
      ITERATE product_loop;
    END IF;
    SET running = running * operand;
  END LOOP product_loop;
END$$
DELIMITER ;

SELECT bucket,
       sq_hard_w8_product(factor)                       AS product_all,
       COUNT(*)                                         AS factor_rows,
       GROUP_CONCAT(factor ORDER BY factor SEPARATOR '/') AS factors
FROM sq_hard_w8_fact
GROUP BY bucket
HAVING product_all > 1
ORDER BY bucket;

--------------------------------------------------------------------------------
-- W8-B: the assignment forms of INSERT and REPLACE put a SET clause where a
-- VALUES list belongs, and `CREATE OR REPLACE TABLE ... REPLACE SELECT` slips a
-- duplicate-row modifier between the table definition and the query that fills
-- it, so the last source row for each key survives.
--------------------------------------------------------------------------------
CREATE TABLE sq_hard_w8_assign (
  assign_id INT NOT NULL PRIMARY KEY,
  measured  INT,
  note      VARCHAR(30)
) ENGINE = InnoDB;

INSERT INTO sq_hard_w8_assign SET assign_id = 1, measured = 7, note = 'inserted';
INSERT INTO sq_hard_w8_assign SET assign_id = 2, measured = 9, note = DEFAULT(note);
REPLACE INTO sq_hard_w8_assign SET assign_id = 1, measured = 11, note = 'replaced';

SELECT assign_id, measured, note
FROM sq_hard_w8_assign
ORDER BY assign_id;

CREATE OR REPLACE TABLE sq_hard_w8_deduped (
  bucket VARCHAR(10) NOT NULL PRIMARY KEY,
  factor INT
) ENGINE = InnoDB REPLACE
SELECT bucket, factor FROM sq_hard_w8_fact ORDER BY fact_id;

SELECT bucket, factor FROM sq_hard_w8_deduped ORDER BY bucket;

--------------------------------------------------------------------------------
-- W8-C: account and privilege administration. Resource limits, TLS requirements
-- and password lifetime clauses are all option words in statement position, and
-- the grant target mixes a database wildcard with a routine kind.
--------------------------------------------------------------------------------
DROP USER IF EXISTS 'sq_hard_w8_user'@'localhost';
DROP ROLE IF EXISTS sq_hard_w8_role;

CREATE USER 'sq_hard_w8_user'@'localhost'
  IDENTIFIED BY 'sq-hard-w8-secret'
  REQUIRE NONE
  WITH MAX_QUERIES_PER_HOUR 120
       MAX_USER_CONNECTIONS 2;

ALTER USER 'sq_hard_w8_user'@'localhost' PASSWORD EXPIRE INTERVAL 90 DAY;

CREATE ROLE sq_hard_w8_role;

GRANT SELECT, INSERT (measured, note) ON sq_hard_mariadb.sq_hard_w8_assign
  TO sq_hard_w8_role;
GRANT EXECUTE ON FUNCTION sq_hard_mariadb.sq_hard_w8_product TO sq_hard_w8_role;
GRANT sq_hard_w8_role TO 'sq_hard_w8_user'@'localhost';
SET DEFAULT ROLE sq_hard_w8_role FOR 'sq_hard_w8_user'@'localhost';

SELECT COUNT(*)                                              AS granted_columns,
       (SELECT max_questions
        FROM mysql.user
        WHERE user = 'sq_hard_w8_user'
          AND host = 'localhost')                            AS max_queries,
       (SELECT COUNT(*)
        FROM mysql.procs_priv
        WHERE routine_name = 'sq_hard_w8_product'
          AND routine_type = 'FUNCTION')                     AS granted_routines
FROM mysql.columns_priv
WHERE user = 'sq_hard_w8_role'
  AND db = 'sq_hard_mariadb'
  AND table_name = 'sq_hard_w8_assign';

REVOKE INSERT (measured, note) ON sq_hard_mariadb.sq_hard_w8_assign
  FROM sq_hard_w8_role;

--------------------------------------------------------------------------------
-- W8-D: the maintenance family. Every statement here is a bare verb followed by
-- a table list and option words that look like clause keywords - and MariaDB's
-- engine-independent statistics form takes two parenthesised name lists.
--------------------------------------------------------------------------------
CREATE TABLE sq_hard_w8_maint (
  maint_id INT NOT NULL PRIMARY KEY,
  reading  INT,
  KEY ix_reading (reading)
) ENGINE = MyISAM;

INSERT INTO sq_hard_w8_maint (maint_id, reading) VALUES (1, 10), (2, 20), (3, 30);

OPTIMIZE LOCAL TABLE sq_hard_w8_maint;
CHECK TABLE sq_hard_w8_maint EXTENDED;
REPAIR TABLE sq_hard_w8_maint QUICK;
ANALYZE TABLE sq_hard_w8_maint PERSISTENT FOR COLUMNS (reading) INDEXES (ix_reading);

BACKUP LOCK sq_hard_w8_maint;
BACKUP UNLOCK;

SELECT COUNT(*) AS stats_columns
FROM mysql.column_stats
WHERE db_name = 'sq_hard_mariadb'
  AND table_name = 'sq_hard_w8_maint';

--------------------------------------------------------------------------------
-- W8-E: a MERGE-engine table is defined by table options alone - UNION lists
-- its children and INSERT_METHOD decides which one receives writes - and the
-- COMPRESSED column attribute sits exactly where a constraint would.
--------------------------------------------------------------------------------
CREATE TABLE sq_hard_w8_leaf_a (
  leaf_id INT,
  KEY ix_leaf_a (leaf_id)
) ENGINE = MyISAM;

CREATE TABLE sq_hard_w8_leaf_b (
  leaf_id INT,
  KEY ix_leaf_b (leaf_id)
) ENGINE = MyISAM;

INSERT INTO sq_hard_w8_leaf_a (leaf_id) VALUES (1), (2);
INSERT INTO sq_hard_w8_leaf_b (leaf_id) VALUES (3);

CREATE TABLE sq_hard_w8_leaf_all (
  leaf_id INT,
  KEY ix_leaf_all (leaf_id)
) ENGINE = MRG_MyISAM UNION = (sq_hard_w8_leaf_a, sq_hard_w8_leaf_b)
  INSERT_METHOD = LAST;

INSERT INTO sq_hard_w8_leaf_all (leaf_id) VALUES (9);

SELECT (SELECT COUNT(*) FROM sq_hard_w8_leaf_all) AS merged_rows,
       (SELECT COUNT(*) FROM sq_hard_w8_leaf_b)   AS last_child_rows;

CREATE TABLE sq_hard_w8_packed (
  packed_id INT NOT NULL PRIMARY KEY,
  payload   VARCHAR(500) COMPRESSED,
  shorthand VARCHAR(40) COMPRESSED = zlib
) ENGINE = Aria
  PAGE_CHECKSUM = 1
  TRANSACTIONAL = 1
  ROW_FORMAT = PAGE;

INSERT INTO sq_hard_w8_packed (packed_id, payload, shorthand)
VALUES (1, REPEAT('compressible-', 30), 'short');

SELECT packed_id,
       LENGTH(payload)   AS payload_length,
       CHAR_LENGTH(shorthand) AS shorthand_chars
FROM sq_hard_w8_packed;

--------------------------------------------------------------------------------
-- W8-F: EXECUTE IMMEDIATE takes a statement string and a USING list in one
-- statement, and anchored declarations name their type by pointing at a column
-- (TYPE OF) or a whole row (ROW TYPE OF) instead of spelling a data type.
--------------------------------------------------------------------------------
SET @sq_hard_w8_floor = 3;

EXECUTE IMMEDIATE 'SELECT COUNT(*) AS above_floor
                   FROM sq_hard_w8_fact
                   WHERE factor > ?'
  USING @sq_hard_w8_floor;

EXECUTE IMMEDIATE CONCAT('SELECT ', '''immediate''', ' AS built_from_concat');

DELIMITER $$
CREATE PROCEDURE sq_hard_w8_anchored()
BEGIN
  DECLARE anchored_bucket TYPE OF sq_hard_w8_fact.bucket DEFAULT 'unset';
  DECLARE whole_row ROW TYPE OF sq_hard_w8_fact;
  DECLARE fetched CURSOR FOR
    SELECT fact_id, bucket, factor FROM sq_hard_w8_fact ORDER BY fact_id;
  DECLARE CONTINUE HANDLER FOR NOT FOUND SET @sq_hard_w8_anchor_done = 1;

  OPEN fetched;
  FETCH fetched INTO whole_row;
  CLOSE fetched;

  SET anchored_bucket = whole_row.bucket;
  SET @sq_hard_w8_anchored = CONCAT_WS('/', anchored_bucket,
                                       whole_row.fact_id, whole_row.factor);
END$$
DELIMITER ;

CALL sq_hard_w8_anchored();

SELECT @sq_hard_w8_anchored AS anchored_text;

--------------------------------------------------------------------------------
-- W8-G: set-operation algebra. INTERSECT ALL and EXCEPT ALL keep the duplicates
-- their DISTINCT forms collapse, a parenthesised branch carries its own
-- ORDER BY and LIMIT, and the trailing ORDER BY belongs to the whole chain.
--------------------------------------------------------------------------------
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

--------------------------------------------------------------------------------
-- W8-H: literal round. Hex, bit and 0x forms, a charset introducer with an
-- explicit collation, a double-quoted string (a string, not an identifier,
-- unless ANSI_QUOTES is on), a backslash escape and a `#` comment all in one
-- projection.
--------------------------------------------------------------------------------
SELECT CAST(X'41' AS CHAR)                       AS hex_literal, # trailing hash comment
       CAST(b'1010' AS UNSIGNED)                 AS bit_literal,
       0x42 + 0                                  AS zero_x_literal,
       _utf8mb4'weiß' COLLATE utf8mb4_bin        AS introduced_literal,
       "double quoted is a string"               AS double_quoted,
       'back\\slash and \'quote\''               AS escaped_text,
       CHAR_LENGTH('tab\there')                  AS escaped_length;

--------------------------------------------------------------------------------
-- W8-I: wave-8 self-verification.
--------------------------------------------------------------------------------
CALL sq_hard_assert(
  (SELECT sq_hard_w8_product(factor) FROM sq_hard_w8_fact WHERE bucket = 'alpha') = 18
  AND (SELECT sq_hard_w8_product(factor) FROM sq_hard_w8_fact WHERE bucket = 'beta') = 35,
  'custom aggregate product'
);
CALL sq_hard_assert(
  (SELECT measured FROM sq_hard_w8_assign WHERE assign_id = 1) = 11
  AND (SELECT note FROM sq_hard_w8_assign WHERE assign_id = 1) = 'replaced'
  AND (SELECT COUNT(*) FROM sq_hard_w8_assign) = 2,
  'insert/replace set form'
);
CALL sq_hard_assert(
  (SELECT COUNT(*) FROM sq_hard_w8_deduped) = 2
  AND (SELECT factor FROM sq_hard_w8_deduped WHERE bucket = 'alpha') = 3,
  'ctas with replace modifier'
);
CALL sq_hard_assert(
  (SELECT COUNT(*) FROM mysql.user WHERE user = 'sq_hard_w8_user') = 1
  AND (SELECT COUNT(*) FROM mysql.roles_mapping
       WHERE user = 'sq_hard_w8_user'
         AND host = 'localhost'
         AND role = 'sq_hard_w8_role') = 1
  AND (SELECT default_role FROM mysql.user
       WHERE user = 'sq_hard_w8_user' AND host = 'localhost') = 'sq_hard_w8_role',
  'account administration'
);
CALL sq_hard_assert(
  (SELECT COUNT(*) FROM mysql.column_stats
   WHERE db_name = 'sq_hard_mariadb' AND table_name = 'sq_hard_w8_maint') = 1,
  'persistent statistics'
);
CALL sq_hard_assert(
  (SELECT COUNT(*) FROM sq_hard_w8_leaf_all) = 4
  AND (SELECT COUNT(*) FROM sq_hard_w8_leaf_b) = 2,
  'merge engine insert method'
);
CALL sq_hard_assert(
  (SELECT LENGTH(payload) FROM sq_hard_w8_packed WHERE packed_id = 1) = 390,
  CONCAT('compressed column length: ',
         (SELECT LENGTH(payload) FROM sq_hard_w8_packed WHERE packed_id = 1))
);
CALL sq_hard_assert(
  @sq_hard_w8_anchored = 'alpha/1/2',
  CONCAT('anchored declarations: ', COALESCE(@sq_hard_w8_anchored, 'NULL'))
);

--------------------------------------------------------------------------------
-- ULTRA WAVE 9: a mid-script dialect island (SET sql_mode = ORACLE turns `||`
-- into concatenation, opens the PL/SQL block grammar, and legalises %TYPE /
-- %ROWTYPE anchors, cursor FOR loops, EXCEPTION handlers and MINUS), the view
-- family with its ALGORITHM/DEFINER/SQL SECURITY header and a cascaded check
-- option, table locking with savepoint-scoped rollback, the predicate and
-- operator zoo (SOUNDS LIKE / RLIKE / ESCAPE / BINARY / <=> / XOR / inline
-- assignment), DATE_FORMAT and STR_TO_DATE format models, a foreign server
-- definition, a HASH-indexed MEMORY table, geometry collections, and a
-- delimiter round that redefines the statement terminator three times.
--------------------------------------------------------------------------------

CREATE TABLE sq_hard_w9_note (
  note_key   VARCHAR(30) NOT NULL PRIMARY KEY,
  note_text  VARCHAR(400),
  note_value BIGINT
) ENGINE = InnoDB;

--------------------------------------------------------------------------------
-- W9-A: the Oracle-dialect island. Between the two SET sql_mode statements the
-- lexer must read `||` as concatenation instead of OR, accept a package with a
-- separate body, anchor locals with %TYPE and %ROWTYPE, run a cursor FOR loop
-- whose record is never declared, and treat MINUS as a set operator.
--------------------------------------------------------------------------------
CREATE TABLE sq_hard_w9_staff (
  staff_id   INT NOT NULL PRIMARY KEY,
  staff_name VARCHAR(30) NOT NULL,
  grade      INT NOT NULL
) ENGINE = InnoDB;

INSERT INTO sq_hard_w9_staff (staff_id, staff_name, grade)
VALUES (1, 'ada', 3), (2, 'linus', 5), (3, 'grace', 7);

SET SESSION sql_mode = 'ORACLE';

DELIMITER $$
CREATE PACKAGE sq_hard_w9_pkg AS
  FUNCTION graded_total(floor_grade INT) RETURN INT;
  FUNCTION digest RETURN VARCHAR(100);
END$$

CREATE PACKAGE BODY sq_hard_w9_pkg AS
  FUNCTION graded_total(floor_grade INT) RETURN INT AS
    running   sq_hard_w9_staff.grade%TYPE := 0;
    one_row   sq_hard_w9_staff%ROWTYPE;
    CURSOR staff_cur IS
      SELECT staff_id, staff_name, grade
      FROM sq_hard_w9_staff
      WHERE grade >= floor_grade
      ORDER BY staff_id;
  BEGIN
    OPEN staff_cur;
    LOOP
      FETCH staff_cur INTO one_row;
      EXIT WHEN staff_cur%NOTFOUND;
      running := running + one_row.grade;
    END LOOP;
    CLOSE staff_cur;
    RETURN running;
  EXCEPTION
    WHEN OTHERS THEN
      RETURN -1;
  END;

  FUNCTION digest RETURN VARCHAR(100) AS
    line VARCHAR(100) := '';
  BEGIN
    FOR rec IN (SELECT staff_name, grade FROM sq_hard_w9_staff ORDER BY staff_id)
    LOOP
      line := line || rec.staff_name || ':' || rec.grade || ';';
    END LOOP;
    RETURN line;
  END;
END$$
DELIMITER ;

SELECT sq_hard_w9_pkg.graded_total(5) AS graded_total,
       sq_hard_w9_pkg.digest()        AS staff_digest,
       'a' || 'b' || 'c'              AS oracle_concat,
       NVL(NULL, 'fallback')          AS nvl_value,
       DECODE(3, 1, 'one', 3, 'three', 'other') AS decoded
FROM dual;

SELECT staff_id FROM sq_hard_w9_staff
MINUS
SELECT staff_id FROM sq_hard_w9_staff WHERE grade < 5;

SET @sq_hard_w9_oracle_total = (SELECT sq_hard_w9_pkg.graded_total(5) FROM dual);
SET @sq_hard_w9_oracle_digest = (SELECT sq_hard_w9_pkg.digest() FROM dual);

SET SESSION sql_mode = 'STRICT_TRANS_TABLES,NO_ENGINE_SUBSTITUTION';

-- Back in the default dialect `||` is OR again, so the same two operands now
-- produce a boolean instead of a string.
SELECT (1 || 0)          AS default_mode_or,
       CONCAT('a', 'b')  AS default_mode_concat,
       @sq_hard_w9_oracle_total  AS island_total,
       @sq_hard_w9_oracle_digest AS island_digest;

INSERT INTO sq_hard_w9_note (note_key, note_text, note_value)
VALUES ('oracle-island', @sq_hard_w9_oracle_digest, @sq_hard_w9_oracle_total);

--------------------------------------------------------------------------------
-- W9-B: the view family. The header stacks ALGORITHM, DEFINER and SQL SECURITY
-- before the VIEW keyword, the body is a UNION whose branches carry their own
-- ORDER BY inside parentheses, and the check option rejects a write that would
-- fall outside the view.
--------------------------------------------------------------------------------
CREATE OR REPLACE ALGORITHM = MERGE DEFINER = CURRENT_USER SQL SECURITY DEFINER
VIEW sq_hard_w9_senior AS
SELECT s.staff_id, s.staff_name, s.grade
FROM sq_hard_w9_staff s
WHERE s.grade >= 5
WITH CASCADED CHECK OPTION;

CREATE OR REPLACE ALGORITHM = TEMPTABLE SQL SECURITY INVOKER
VIEW sq_hard_w9_ranked AS
(SELECT staff_id, staff_name, grade FROM sq_hard_w9_staff ORDER BY grade DESC LIMIT 2)
UNION ALL
(SELECT staff_id, staff_name, grade FROM sq_hard_w9_staff ORDER BY grade ASC LIMIT 1);

INSERT INTO sq_hard_w9_senior (staff_id, staff_name, grade) VALUES (4, 'ken', 9);
UPDATE sq_hard_w9_senior SET grade = grade + 1 WHERE staff_id = 4;

SELECT v.staff_id, v.staff_name, v.grade
FROM sq_hard_w9_senior v
ORDER BY v.staff_id;

SELECT r.staff_name, r.grade FROM sq_hard_w9_ranked r ORDER BY r.grade DESC, r.staff_id;

SELECT table_name, is_updatable, check_option, security_type
FROM information_schema.views
WHERE table_schema = 'sq_hard_mariadb'
  AND table_name IN ('sq_hard_w9_senior', 'sq_hard_w9_ranked')
ORDER BY table_name;

DELIMITER $$
CREATE PROCEDURE sq_hard_w9_check_option_probe()
BEGIN
  DECLARE rejected INT DEFAULT 0;
  DECLARE CONTINUE HANDLER FOR SQLEXCEPTION SET rejected = 1;

  INSERT INTO sq_hard_w9_senior (staff_id, staff_name, grade) VALUES (5, 'low', 1);
  SET @sq_hard_w9_check_rejected = rejected;
END$$
DELIMITER ;

CALL sq_hard_w9_check_option_probe();

CALL sq_hard_assert(@sq_hard_w9_check_rejected = 1,
  CONCAT('check option rejection: ', COALESCE(@sq_hard_w9_check_rejected, 'NULL')));

--------------------------------------------------------------------------------
-- W9-C: locking and savepoint-scoped rollback. LOCK TABLES names the same table
-- twice through an alias, the transaction rolls back to a named savepoint so
-- only the second write survives, and the lock is released by UNLOCK TABLES.
--------------------------------------------------------------------------------
CREATE TABLE sq_hard_w9_ledger (
  entry_id INT NOT NULL PRIMARY KEY,
  amount   INT NOT NULL
) ENGINE = InnoDB;

INSERT INTO sq_hard_w9_ledger (entry_id, amount) VALUES (1, 100);

START TRANSACTION;
SAVEPOINT before_writes;
INSERT INTO sq_hard_w9_ledger (entry_id, amount) VALUES (2, 200);
SAVEPOINT after_first;
INSERT INTO sq_hard_w9_ledger (entry_id, amount) VALUES (3, 300);
ROLLBACK TO SAVEPOINT after_first;
RELEASE SAVEPOINT before_writes;
COMMIT;

LOCK TABLES sq_hard_w9_ledger WRITE, sq_hard_w9_ledger AS l READ, sq_hard_w9_staff READ;

SELECT COUNT(*) AS locked_rows, SUM(amount) AS locked_amount
FROM sq_hard_w9_ledger AS l;

UNLOCK TABLES;

CALL sq_hard_assert(
  (SELECT COUNT(*) FROM sq_hard_w9_ledger) = 2,
  CONCAT('savepoint rollback rows: ', (SELECT COUNT(*) FROM sq_hard_w9_ledger)));

--------------------------------------------------------------------------------
-- W9-D: the predicate and operator zoo. Every line below is a distinct operator
-- family, several of which are two-word operators that look like keywords, plus
-- an inline `:=` assignment that mutates a user variable inside the projection.
--------------------------------------------------------------------------------
SET @sq_hard_w9_row = 0;

SELECT s.staff_name,
       @sq_hard_w9_row := @sq_hard_w9_row + 1              AS running_row,
       s.staff_name SOUNDS LIKE 'adda'                     AS sounds_like_ada,
       s.staff_name RLIKE '^[ag]'                          AS starts_a_or_g,
       s.staff_name NOT REGEXP 'z$'                        AS not_ending_z,
       s.staff_name LIKE 'a|_a%' ESCAPE '|'                AS escaped_like,
       BINARY s.staff_name = 'ADA'                         AS binary_compare,
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
       (SELECT COUNT(*) FROM sq_hard_w9_staff WHERE grade IS NOT UNKNOWN) AS not_unknown_rows;

--------------------------------------------------------------------------------
-- W9-E: date and number format models. The format string is a mini-language of
-- percent escapes -- including a doubled `%%` and a literal that spells a
-- specifier -- and STR_TO_DATE parses one back.
--------------------------------------------------------------------------------
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

--------------------------------------------------------------------------------
-- W9-F: a foreign server definition, a MEMORY table whose secondary index is a
-- hash, and a geometry collection built from well-known text.
--------------------------------------------------------------------------------
DROP SERVER IF EXISTS sq_hard_w9_srv;
CREATE SERVER sq_hard_w9_srv
  FOREIGN DATA WRAPPER mysql
  OPTIONS (HOST '127.0.0.1', DATABASE 'sq_hard_mariadb', USER 'root',
           PASSWORD 'password', PORT 3306, SOCKET '', OWNER 'root');

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
       ST_GeometryType(shape)                    AS geom_type,
       ST_NumGeometries(shape)                   AS parts,
       ROUND(ST_Area(shape), 2)                  AS area,
       ST_AsText(ST_Centroid(shape))             AS centroid,
       ST_Contains(shape, ST_GeomFromText('POINT(1 1)')) AS holds_point
FROM sq_hard_w9_shape
ORDER BY shape_id;

DROP SERVER sq_hard_w9_srv;

--------------------------------------------------------------------------------
-- W9-G: the delimiter round. The terminator is redefined three times, and each
-- routine body carries the *other* delimiters inside a string literal and a
-- comment so a naive splitter cuts the body apart.
--------------------------------------------------------------------------------
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

--------------------------------------------------------------------------------
-- Wave-9 self-verification.
--------------------------------------------------------------------------------
CALL sq_hard_assert(
  @sq_hard_w9_oracle_total = 12,
  CONCAT('oracle island total: ', COALESCE(@sq_hard_w9_oracle_total, 'NULL')));
CALL sq_hard_assert(
  @sq_hard_w9_oracle_digest = 'ada:3;linus:5;grace:7;',
  CONCAT('oracle island digest: ', COALESCE(@sq_hard_w9_oracle_digest, 'NULL')));
CALL sq_hard_assert(
  (SELECT COUNT(*) FROM sq_hard_w9_senior) = 3,
  CONCAT('senior view rows: ', (SELECT COUNT(*) FROM sq_hard_w9_senior)));
CALL sq_hard_assert(
  (SELECT COUNT(*) FROM sq_hard_w9_ranked) = 3,
  CONCAT('ranked view rows: ', (SELECT COUNT(*) FROM sq_hard_w9_ranked)));
CALL sq_hard_assert(
  (SELECT SUM(amount) FROM sq_hard_w9_ledger) = 300,
  CONCAT('ledger amount: ', (SELECT SUM(amount) FROM sq_hard_w9_ledger)));
CALL sq_hard_assert(
  (SELECT COUNT(*) FROM sq_hard_w9_shape WHERE ST_GeometryType(shape) = 'GEOMETRYCOLLECTION') = 1,
  'geometry collection stored');
CALL sq_hard_assert(
  (SELECT note_value FROM sq_hard_w9_note WHERE note_key = 'delimiters') = 53,
  CONCAT('delimiter round length: ',
         (SELECT note_value FROM sq_hard_w9_note WHERE note_key = 'delimiters')));
CALL sq_hard_assert(
  (SELECT COUNT(*) FROM sq_hard_w9_note) = 3,
  CONCAT('wave9 note rows: ', (SELECT COUNT(*) FROM sq_hard_w9_note)));

--------------------------------------------------------------------------------
-- ULTRA WAVE 10: the JSON_TABLE row source with a NESTED PATH and
-- EXISTS/DEFAULT-ON-ERROR columns, top-level BEGIN NOT ATOMIC compound
-- statements carrying an integer FOR loop, a labelled WHILE with ITERATE, a
-- named CONDITION, a SIGNAL caught by its own handler and GET DIAGNOSTICS
-- CONDITION, the RETURNING family on INSERT/REPLACE/DELETE beside ROWNUM(),
-- an ANSI sql_mode island that changes three lexical rules at once, MariaDB
-- versioned executable comments, and lexer round 5 whose backtick identifiers
-- and literals carry the terminator, every comment introducer and a line break.
--------------------------------------------------------------------------------

CREATE TABLE sq_hard_w10_note (
  note_key   VARCHAR(30) NOT NULL PRIMARY KEY,
  note_text  VARCHAR(400),
  note_value BIGINT
) ENGINE = InnoDB;

--------------------------------------------------------------------------------
-- W10-A: JSON_TABLE. The COLUMNS list mints relation columns that exist nowhere
-- in the catalog, a NESTED PATH opens a second COLUMNS list one level down,
-- FOR ORDINALITY invents a counter, and EXISTS/DEFAULT ... ON EMPTY|ON ERROR
-- change what a missing path yields.
--------------------------------------------------------------------------------
CREATE TABLE sq_hard_w10_profile (
  profile_id INT NOT NULL PRIMARY KEY,
  doc        JSON NOT NULL CHECK (JSON_VALID(doc))
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

--------------------------------------------------------------------------------
-- W10-B: compound statements outside a routine. BEGIN NOT ATOMIC opens a block
-- with its own DECLARE section at script top level, an integer FOR loop needs
-- no cursor, a labelled WHILE is short-circuited by ITERATE, and a SIGNAL is
-- caught by a handler for its own named CONDITION which then reads the
-- diagnostics area back out.
--------------------------------------------------------------------------------
CREATE TABLE sq_hard_w10_bucket (
  bucket_id INT NOT NULL PRIMARY KEY,
  qty       INT NOT NULL
) ENGINE = InnoDB;

DELIMITER //
BEGIN NOT ATOMIC
  DECLARE running INT DEFAULT 0;
  DECLARE i       INT DEFAULT 0;
  DECLARE custom_fault CONDITION FOR SQLSTATE '45001';
  DECLARE CONTINUE HANDLER FOR custom_fault
  BEGIN
    GET DIAGNOSTICS CONDITION 1
      @sq_hard_w10_errno = MYSQL_ERRNO,
      @sq_hard_w10_state = RETURNED_SQLSTATE;
  END;

  FOR n IN 1 .. 5 DO
    SET running = running + n;
  END FOR;

  counting: WHILE i < 3 DO
    SET i = i + 1;
    IF i = 2 THEN
      ITERATE counting;
    END IF;
    SET running = running + 100;
  END WHILE counting;

  SIGNAL custom_fault SET MESSAGE_TEXT = 'w10 signalled', MYSQL_ERRNO = 1644;

  INSERT INTO sq_hard_w10_bucket (bucket_id, qty) VALUES (1, running);
  GET DIAGNOSTICS @sq_hard_w10_inserted = ROW_COUNT;
  SET @sq_hard_w10_running = running;
END//
DELIMITER ;

SELECT bucket_id, qty FROM sq_hard_w10_bucket ORDER BY bucket_id;

INSERT INTO sq_hard_w10_note (note_key, note_text, note_value)
VALUES ('compound',
        CONCAT('errno ', @sq_hard_w10_errno, ' state ', @sq_hard_w10_state,
               ' inserted ', @sq_hard_w10_inserted),
        @sq_hard_w10_running);

--------------------------------------------------------------------------------
-- W10-C: the RETURNING family. INSERT, REPLACE and DELETE each project a result
-- set from the rows they just changed, so a DML verb opens a select list, and
-- ROWNUM() numbers rows from a function that takes no argument.
--------------------------------------------------------------------------------
CREATE TABLE sq_hard_w10_ledger (
  entry_id INT NOT NULL PRIMARY KEY,
  amount   DECIMAL(10, 2) NOT NULL,
  memo     VARCHAR(30)
) ENGINE = InnoDB;

INSERT INTO sq_hard_w10_ledger (entry_id, amount, memo)
VALUES (1, 100.00, 'opening'), (2, 200.00, 'second')
RETURNING entry_id, amount * 2 AS doubled, UPPER(memo) AS shouted;

REPLACE INTO sq_hard_w10_ledger (entry_id, amount, memo)
VALUES (2, 250.00, 'replaced')
RETURNING entry_id, amount, memo;

DELETE FROM sq_hard_w10_ledger
WHERE entry_id = 1
RETURNING entry_id, memo;

INSERT INTO sq_hard_w10_ledger (entry_id, amount, memo)
VALUES (3, 50.00, 'third'), (4, 75.00, 'fourth');

SELECT ROWNUM() AS rn, l.entry_id, l.amount
FROM sq_hard_w10_ledger l
ORDER BY l.entry_id;

INSERT INTO sq_hard_w10_note (note_key, note_text, note_value)
VALUES ('returning', 'insert/replace/delete returning',
        (SELECT SUM(amount) FROM sq_hard_w10_ledger));

--------------------------------------------------------------------------------
-- W10-D: an ANSI sql_mode island. One statement makes double quotes delimit
-- identifiers, `||` concatenate, and a space legal between a function name and
-- its parenthesis -- three lexer rules that flip back at the end of the island.
-- MariaDB versioned executable comments hide real code inside comment syntax.
--------------------------------------------------------------------------------
SET SESSION sql_mode = 'ANSI';

CREATE TABLE "sq_hard_w10_ansi" (
  "group" INT NOT NULL PRIMARY KEY,
  "value" VARCHAR(20) NOT NULL
);

INSERT INTO "sq_hard_w10_ansi" ("group", "value") VALUES (1, 'ansi'), (2, 'island');

SELECT "group" AS "grouped id",
       "value" || '-' || "value" AS "doubled value",
       COUNT (*) OVER () AS "row total"
FROM "sq_hard_w10_ansi"
ORDER BY "group";

SET @sq_hard_w10_ansi = (SELECT "value" || '/' || CAST("group" AS CHAR)
                         FROM "sq_hard_w10_ansi"
                         WHERE "group" = 2);

SET SESSION sql_mode = 'STRICT_TRANS_TABLES,NO_ENGINE_SUBSTITUTION';

SELECT /*M!100503 1 + */ 2                       AS versioned_sum,
       /*!40101 CONCAT('mysql', */ '-comment' /*!40101 ) */ AS versioned_text,
       @sq_hard_w10_ansi                         AS ansi_island;

INSERT INTO sq_hard_w10_note (note_key, note_text, note_value)
VALUES ('ansi-island', @sq_hard_w10_ansi, CHAR_LENGTH(@sq_hard_w10_ansi));

--------------------------------------------------------------------------------
-- W10-E: lexer round 5. Backtick identifiers carry the terminator, both SQL
-- comment introducers, the MySQL hash comment, an apostrophe and a trailing
-- space; literals span a line break and impersonate a compound statement; and
-- every numeric/binary literal spelling appears side by side.
--------------------------------------------------------------------------------
SELECT 1 AS `semi;colon`,
       2 AS `dash--dash`,
       3 AS `slash/*star`,
       4 AS `hash#comment`,
       5 AS `it's`,
       6 AS `trailing space `
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

--------------------------------------------------------------------------------
-- Wave-10 self-verification.
--------------------------------------------------------------------------------
CALL sq_hard_assert(
  @sq_hard_w10_tag_total = 4,
  CONCAT('json_table tag rows: ', COALESCE(@sq_hard_w10_tag_total, 'NULL')));
CALL sq_hard_assert(
  @sq_hard_w10_score_sum = 42,
  CONCAT('json_table score sum: ', COALESCE(@sq_hard_w10_score_sum, 'NULL')));
CALL sq_hard_assert(
  @sq_hard_w10_running = 215,
  CONCAT('compound running total: ', COALESCE(@sq_hard_w10_running, 'NULL')));
CALL sq_hard_assert(
  @sq_hard_w10_errno = 1644 AND @sq_hard_w10_state = '45001',
  CONCAT('signal diagnostics: ', COALESCE(@sq_hard_w10_errno, 'NULL'), '/',
         COALESCE(@sq_hard_w10_state, 'NULL')));
CALL sq_hard_assert(
  (SELECT SUM(amount) FROM sq_hard_w10_ledger) = 375,
  CONCAT('ledger total: ', (SELECT SUM(amount) FROM sq_hard_w10_ledger)));
CALL sq_hard_assert(
  @sq_hard_w10_ansi = 'island/2',
  CONCAT('ansi island: ', COALESCE(@sq_hard_w10_ansi, 'NULL')));
CALL sq_hard_assert(
  @sq_hard_w10_lexer_len = 74,
  CONCAT('lexer length: ', COALESCE(@sq_hard_w10_lexer_len, 'NULL')));
CALL sq_hard_assert(
  (SELECT COUNT(*) FROM sq_hard_w10_note) = 5,
  CONCAT('wave10 note rows: ', (SELECT COUNT(*) FROM sq_hard_w10_note)));

--------------------------------------------------------------------------------
-- WAVE 11 -- the declaration surface: every numeric/string/temporal type
-- spelling with its option words, an sql_mode island that changes what an
-- expression MEANS rather than how it is spelled, optimizer-hint comments,
-- the system-variable spelling zoo, VIA-authenticated accounts, a cursor FOR
-- loop beside REPEAT/UNTIL, and a sixth lexer round.
--------------------------------------------------------------------------------
CREATE TABLE sq_hard_w11_note (
  note_key   VARCHAR(30) NOT NULL PRIMARY KEY,
  note_text  VARCHAR(400),
  note_value BIGINT
) ENGINE = InnoDB;

--------------------------------------------------------------------------------
-- W11-A: the data-type zoo. Display widths, UNSIGNED and ZEROFILL stack on the
-- integer family; DECIMAL/FLOAT carry precision and scale; BIT, ENUM, SET and
-- YEAR are value-set types; fractional seconds appear on TIME/DATETIME with
-- CURRENT_TIMESTAMP(6) on both DEFAULT and ON UPDATE; and the blob/text ladder
-- takes its own CHARACTER SET and COLLATE. information_schema proves what the
-- server actually stored.
--------------------------------------------------------------------------------
CREATE TABLE sq_hard_w11_type (
  row_id       INT UNSIGNED ZEROFILL NOT NULL,
  tiny_flag    TINYINT(1) UNSIGNED,
  medium_num   MEDIUMINT,
  big_num      BIGINT UNSIGNED,
  wide_dec     DECIMAL(30, 10),
  float_scaled FLOAT(10, 3),
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
  long_note    LONGTEXT,
  blob_note    MEDIUMBLOB,
  doc          JSON,
  PRIMARY KEY (row_id)
) ENGINE = InnoDB;

INSERT INTO sq_hard_w11_type (row_id, tiny_flag, medium_num, big_num, wide_dec,
                              float_scaled, double_free, bit_mask, grade, flags,
                              made_year, precise_time, stamped, raw_bytes,
                              tiny_note, long_note, blob_note, doc)
VALUES (7, 1, -8388608, 18446744073709551615, 12345.0123456789, 1.25, 2.5,
        b'10101010', 'gamma', 'read,admin', 2024, '01:02:03.456789',
        TIMESTAMP '2024-05-06 07:08:09', X'DEADBEEF', 'tiny note', 'long note',
        X'0102', JSON_OBJECT('kind', 'type-zoo', 'depth', 2));

INSERT INTO sq_hard_w11_type (row_id, medium_num, flags, made_year)
VALUES (8, 8388607, '', 2000);

SELECT t.row_id,
       t.tiny_flag,
       t.medium_num,
       t.big_num,
       t.wide_dec,
       t.float_scaled,
       t.bit_mask + 0                       AS bit_value,
       t.grade,
       t.flags,
       FIND_IN_SET('admin', t.flags)        AS admin_bit,
       t.made_year,
       t.precise_time,
       HEX(t.raw_bytes)                     AS raw_hex,
       t.made_at IS NOT NULL                AS made_at_set,
       OCTET_LENGTH(t.blob_note)            AS blob_bytes,
       JSON_VALUE(t.doc, '$.kind')          AS doc_kind
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
WHERE c.TABLE_SCHEMA = 'sq_hard_mariadb'
  AND c.TABLE_NAME = 'sq_hard_w11_type'
  AND c.COLUMN_NAME IN ('row_id', 'tiny_flag', 'medium_num', 'big_num',
                        'wide_dec', 'float_scaled', 'bit_mask', 'grade',
                        'flags', 'precise_time', 'made_at', 'tiny_note')
ORDER BY c.ORDINAL_POSITION;

SET @sq_hard_w11_type_sum = (SELECT SUM(row_id) FROM sq_hard_w11_type);

--------------------------------------------------------------------------------
-- W11-B: an sql_mode island that changes MEANING, not spelling.
-- HIGH_NOT_PRECEDENCE makes `NOT a BETWEEN b AND c` parse as `(NOT a) BETWEEN
-- b AND c`, NO_UNSIGNED_SUBTRACTION lets an unsigned difference go negative,
-- and PAD_CHAR_TO_FULL_LENGTH stops CHAR from being trimmed on read. The same
-- three expressions are evaluated on both sides of the island.
--------------------------------------------------------------------------------
CREATE TABLE sq_hard_w11_pad (
  pad_id  INT NOT NULL PRIMARY KEY,
  padded  CHAR(10)
) ENGINE = InnoDB;

INSERT INTO sq_hard_w11_pad (pad_id, padded) VALUES (1, 'abc');

SELECT NOT 1 BETWEEN -5 AND 5           AS not_precedence,
       CAST(3 AS UNSIGNED) - 2          AS unsigned_diff,
       CHAR_LENGTH(padded)              AS padded_len
FROM sq_hard_w11_pad;

SET SESSION sql_mode = 'STRICT_TRANS_TABLES,HIGH_NOT_PRECEDENCE,NO_UNSIGNED_SUBTRACTION,PAD_CHAR_TO_FULL_LENGTH';

SELECT NOT 1 BETWEEN -5 AND 5           AS not_precedence,
       CAST(3 AS UNSIGNED) - 2          AS unsigned_diff,
       CAST(1 AS UNSIGNED) - 2          AS negative_unsigned,
       CHAR_LENGTH(padded)              AS padded_len
FROM sq_hard_w11_pad;

SET @sq_hard_w11_island = (SELECT CONCAT_WS('/',
                                            NOT 1 BETWEEN -5 AND 5,
                                            CAST(1 AS UNSIGNED) - 2,
                                            CHAR_LENGTH(padded))
                           FROM sq_hard_w11_pad);

SET SESSION sql_mode = 'STRICT_TRANS_TABLES,NO_ENGINE_SUBSTITUTION';

SELECT @sq_hard_w11_island                AS island_shape,
       CONCAT_WS('/',
                 NOT 1 BETWEEN -5 AND 5,
                 CAST(3 AS UNSIGNED) - 2,
                 (SELECT CHAR_LENGTH(padded) FROM sq_hard_w11_pad)) AS mainland_shape;

--------------------------------------------------------------------------------
-- W11-C: optimizer-hint comments. `/*+ ... */` is a comment to the lexer and a
-- directive to the planner; the hints name tables, nest a query-block label
-- behind @, and carry a numeric argument. A statement then wears both a hint
-- comment and an ordinary comment in the same slot.
--------------------------------------------------------------------------------
SELECT /*+ BNL(t) NO_ICP(t) MAX_EXECUTION_TIME(5000) */
       COUNT(*)          AS hinted_rows,
       SUM(t.medium_num) AS hinted_total
FROM sq_hard_w11_type t;

SELECT /*+ QB_NAME(w11_qb) NO_BNL(@w11_qb t) */ /* plain comment after a hint */
       t.row_id,
       t.grade
FROM sq_hard_w11_type t
WHERE t.row_id = 7;

UPDATE /*+ NO_ICP(t) BNL(t) */ sq_hard_w11_type t
SET t.tiny_flag = 1
WHERE t.row_id = 8;

--------------------------------------------------------------------------------
-- W11-D: the system-variable spelling zoo. @@name, @@SESSION.name,
-- @@session.name and @@GLOBAL.name all address variables; SET reaches the same
-- variable through two spellings; and a user variable holds the result of a
-- scalar subquery so it can be compared later.
--------------------------------------------------------------------------------
SELECT @@version_comment                     AS version_comment,
       @@SESSION.sql_mode = @@sql_mode       AS session_is_default,
       @@GLOBAL.max_connections > 0          AS global_readable,
       @@in_transaction                      AS inside_transaction,
       LENGTH(@@character_set_connection) > 0 AS charset_named;

SET @@SESSION.group_concat_max_len = 4096;
SET SESSION sql_select_limit = DEFAULT;

SELECT @@session.group_concat_max_len        AS group_concat_len,
       @@sql_select_limit > 0                AS select_limit_set;

SET @sq_hard_w11_vars = (SELECT CONCAT(@@session.group_concat_max_len, ':',
                                       CHAR_LENGTH(@@version_comment) > 0));

--------------------------------------------------------------------------------
-- W11-E: account administration through MariaDB's VIA grammar. One
-- CREATE OR REPLACE USER names two authentication plugins joined by OR, the
-- second without a USING clause; ALTER USER then re-states the same shape.
--------------------------------------------------------------------------------
CREATE OR REPLACE USER sq_hard_w11_u@localhost
  IDENTIFIED VIA mysql_native_password USING PASSWORD('w11-secret')
              OR unix_socket
  WITH MAX_QUERIES_PER_HOUR 120 MAX_USER_CONNECTIONS 3;

ALTER USER sq_hard_w11_u@localhost
  IDENTIFIED VIA mysql_native_password USING PASSWORD('w11-rotated');

GRANT SELECT (row_id, grade) ON sq_hard_mariadb.sq_hard_w11_type
  TO sq_hard_w11_u@localhost;

SELECT u.User,
       u.plugin <> ''            AS has_plugin,
       u.max_questions           AS max_questions,
       u.max_user_connections    AS max_user_conn
FROM mysql.user u
WHERE u.User = 'sq_hard_w11_u';

SELECT p.Column_name
FROM mysql.columns_priv p
WHERE p.User = 'sq_hard_w11_u' AND p.Table_name = 'sq_hard_w11_type'
ORDER BY p.Column_name;

SET @sq_hard_w11_grants = (SELECT COUNT(*) FROM mysql.columns_priv
                           WHERE User = 'sq_hard_w11_u');

DROP USER sq_hard_w11_u@localhost;

--------------------------------------------------------------------------------
-- W11-F: a cursor FOR loop whose record is minted by the loop header, a
-- REPEAT/UNTIL counter, and a nested handler that swallows a duplicate key so
-- the outer block keeps running.
--------------------------------------------------------------------------------
DELIMITER $$
CREATE PROCEDURE sq_hard_w11_walk(OUT out_total INT, OUT out_reps INT,
                                  OUT out_swallowed INT)
BEGIN
  DECLARE reps INT DEFAULT 0;
  DECLARE swallowed INT DEFAULT 0;

  SET out_total = 0;

  FOR rec IN (SELECT row_id, medium_num FROM sq_hard_w11_type ORDER BY row_id) DO
    SET out_total = out_total + rec.row_id;
  END FOR;

  REPEAT
    SET reps = reps + 1;
  UNTIL reps >= 3 END REPEAT;

  BEGIN
    DECLARE CONTINUE HANDLER FOR 1062
      SET swallowed = swallowed + 1;
    INSERT INTO sq_hard_w11_pad (pad_id, padded) VALUES (1, 'dup');
  END;

  SET out_reps = reps;
  SET out_swallowed = swallowed;
END$$
DELIMITER ;

CALL sq_hard_w11_walk(@sq_hard_w11_total, @sq_hard_w11_reps,
                      @sq_hard_w11_swallowed);

SELECT @sq_hard_w11_total     AS for_loop_total,
       @sq_hard_w11_reps      AS repeat_count,
       @sq_hard_w11_swallowed AS swallowed_dup;

INSERT INTO sq_hard_w11_note (note_key, note_text, note_value)
VALUES ('routine', 'for-loop, repeat-until, nested handler',
        @sq_hard_w11_total + @sq_hard_w11_reps + @sq_hard_w11_swallowed);

--------------------------------------------------------------------------------
-- W11-G: lexer round 6. A string literal carries a block-comment terminator, a
-- backtick identifier is all digits, backslash escapes appear inside and
-- outside backticks, a binary introducer prefixes a hex literal, and two
-- version-gated executable comments sit side by side -- one below this server's
-- version (it runs) and one above it (it does not).
--------------------------------------------------------------------------------
SELECT '*/ not the end of a comment'    AS star_slash,
       1                                AS `123`,
       2                                AS `back\slash%`,
       'tab\there and quote\'inside'    AS escaped_text,
       _binary 0x4142                   AS binary_intro,
       3 /*!99999 + 100 */              AS future_version,
       4 /*!100000 + 100 */             AS applied_version,
       5 /*M!100503 + 100 */            AS mariadb_version
FROM DUAL;

SET @sq_hard_w11_lexer = (SELECT CHAR_LENGTH('*/ not the end of a comment')
                                 + (3 /*!99999 + 100 */)
                                 + (4 /*!100000 + 100 */));

INSERT INTO sq_hard_w11_note (note_key, note_text, note_value)
VALUES ('lexer6', 'star-slash literal and version-gated comments',
        @sq_hard_w11_lexer);

INSERT INTO sq_hard_w11_note (note_key, note_text, note_value)
VALUES ('types', @sq_hard_w11_island, @sq_hard_w11_type_sum);

--------------------------------------------------------------------------------
-- Wave-11 self-verification.
--------------------------------------------------------------------------------
CALL sq_hard_assert(
  @sq_hard_w11_type_sum = 15,
  CONCAT('type zoo row sum: ', COALESCE(@sq_hard_w11_type_sum, 'NULL')));
CALL sq_hard_assert(
  @sq_hard_w11_island = '1/-1/10',
  CONCAT('sql_mode island shape: ', COALESCE(@sq_hard_w11_island, 'NULL')));
CALL sq_hard_assert(
  CONCAT_WS('/', NOT 1 BETWEEN -5 AND 5, CAST(3 AS UNSIGNED) - 2,
            (SELECT CHAR_LENGTH(padded) FROM sq_hard_w11_pad)) = '0/1/3',
  CONCAT('mainland shape: ',
         CONCAT_WS('/', NOT 1 BETWEEN -5 AND 5, CAST(3 AS UNSIGNED) - 2,
                   (SELECT CHAR_LENGTH(padded) FROM sq_hard_w11_pad))));
CALL sq_hard_assert(
  @sq_hard_w11_grants = 2,
  CONCAT('column grants: ', COALESCE(@sq_hard_w11_grants, 'NULL')));
CALL sq_hard_assert(
  @sq_hard_w11_total = 15 AND @sq_hard_w11_reps = 3
    AND @sq_hard_w11_swallowed = 1,
  CONCAT('routine walk: ', COALESCE(@sq_hard_w11_total, 'NULL'), '/',
         COALESCE(@sq_hard_w11_reps, 'NULL'), '/',
         COALESCE(@sq_hard_w11_swallowed, 'NULL')));
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

--------------------------------------------------------------------------------
-- ULTRA WAVE 12 -- the name-collision surface. Builtin functions whose names
-- are statement verbs (INSERT / REPLACE / TRUNCATE / REPEAT / IF / LEFT /
-- RIGHT), builtins whose arguments are separated by keywords (POSITION ... IN,
-- SUBSTRING ... FROM ... FOR, TRIM BOTH ... FROM, EXTRACT ... FROM), a table
-- whose name is two keywords with a space in it and whose columns are spelled
-- `--`, `/*`, `;`, `?`, `@var`, `#hash` and `'quoted'`, the set-operator
-- precedence tower, dynamic SQL whose payload looks like the surrounding
-- script, and lexer round 7.
--------------------------------------------------------------------------------
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

-- A stored function and a stored procedure named after a native function and a
-- statement verb; both can only ever be called through backticks.
CREATE FUNCTION `insert` (n INT) RETURNS INT
  DETERMINISTIC NO SQL
  RETURN n + 1;

DELIMITER $$
CREATE PROCEDURE `select` (OUT total INT)
BEGIN
  SET total = 41 + 1;
END$$
DELIMITER ;

CALL `select`(@sq_hard_w12_proc);

SET @sq_hard_w12_native = `insert`(1);

SELECT @sq_hard_w12_native AS native_named_function,
       @sq_hard_w12_proc   AS verb_named_procedure;

--------------------------------------------------------------------------------
-- W12-B: the punctuation identifier battery. The table name is two keywords
-- with a space between them; the columns are comment introducers, a statement
-- terminator, a placeholder, a user-variable spelling and a quoted literal.
--------------------------------------------------------------------------------
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

-- A derived table aliased UNION, and DEFAULT()/VALUES() in one upsert.
SELECT `union`.`select` AS `;`
FROM (SELECT `select` FROM `left join`) AS `union`
ORDER BY `union`.`select`;

INSERT INTO `left join` (`select`, `from`, `where`, `null`, `having`, `order`)
VALUES (1, 99, 2, 8, 5, 4)
ON DUPLICATE KEY UPDATE `from` = VALUES(`from`), `default` = DEFAULT(`default`);

SET @sq_hard_w12_quoted = (
  SELECT CONCAT_WS('/', a.`--` + a.`/*`, a.`;`, a.`?`, a.`@var`, a.`#hash`,
                   a.`'quoted'`, a.`\`, a.`from`, a.`default`)
  FROM `left join` a
  WHERE a.`select` = 1);

--------------------------------------------------------------------------------
-- W12-C: the set-operator precedence tower. MariaDB binds INTERSECT tighter
-- than UNION, so it only consumes the branch next to it: the first tower keeps
-- 1, 2 and 9 while the intersected pair collapses to nothing. The parenthesised
-- EXCEPT below and the EXCEPT ALL multiset difference prove the difference.
--------------------------------------------------------------------------------
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

--------------------------------------------------------------------------------
-- W12-D: dynamic SQL whose payload carries a statement terminator, both comment
-- shapes and a doubled quote, built by CONCAT inside a routine and handed to
-- PREPARE; EXECUTE IMMEDIATE runs a second payload directly.
--------------------------------------------------------------------------------
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

EXECUTE IMMEDIATE
  'SELECT ''EXECUTE IMMEDIATE keeps its ; inside a literal'' AS payload';

--------------------------------------------------------------------------------
-- W12-E: lexer round 7. Adjacent string literals concatenate, a double-quoted
-- token is a string (ANSI_QUOTES is off), hex and bit literals appear in both
-- spellings, escapes carry NUL and ctrl-Z, a literal spells out DELIMITER and
-- both alternate terminators, and the number zoo mixes leading/trailing dots
-- with two exponent forms.
--------------------------------------------------------------------------------
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
       1                               AS trailing_item # hash comment with ' and ;
FROM DUAL;

SET @sq_hard_w12_lexer = (
  SELECT CHAR_LENGTH(CONCAT('a' 'b' 'c', "double quoted", X'414243'))
         + LENGTH(0x444546) + (0b1000010 - b'1000001')
  FROM DUAL);

INSERT INTO sq_hard_w12_note (note_key, note_text, note_value)
VALUES ('verbs', @sq_hard_w12_verbs, @sq_hard_w12_proc),
       ('quoted', @sq_hard_w12_quoted, @sq_hard_w12_dyn_hits),
       ('setops', @sq_hard_w12_setops, @sq_hard_w12_lexer);

--------------------------------------------------------------------------------
-- Wave-12 self-verification.
--------------------------------------------------------------------------------
CALL sq_hard_assert(
  @sq_hard_w12_verbs = 'aXYef/3.14/ababab/yes/3/bcd/a/2',
  CONCAT('verb builtins: ', COALESCE(@sq_hard_w12_verbs, 'NULL')));
CALL sq_hard_assert(
  @sq_hard_w12_native = 2 AND @sq_hard_w12_proc = 42,
  CONCAT('native-named routines: ', COALESCE(@sq_hard_w12_proc, 'NULL')));
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

--------------------------------------------------------------------------------
-- ULTRA WAVE 13 -- MariaDB-only recovery points. NATURAL_SORT_KEY is persisted
-- in an INVISIBLE indexed column, SFORMAT consumes a JSON scalar, system
-- versioning is added and removed by ALTER TABLE, WAIT belongs to both a row
-- lock and TRUNCATE, EXECUTE IMMEDIATE binds an OUT variable and a DEFAULT
-- indicator, and SHOW/GET DIAGNOSTICS read the same warning area.
--------------------------------------------------------------------------------
CREATE TABLE sq_hard_w13_note (
  note_key   VARCHAR(30) NOT NULL PRIMARY KEY,
  note_text  VARCHAR(300),
  note_value BIGINT
) ENGINE = InnoDB;

--------------------------------------------------------------------------------
-- W13-A: the sort key is deliberately INVISIBLE, so SELECT * omits it while an
-- index and ORDER BY still resolve it. JSON_TABLE is implicitly lateral to each
-- release row; SFORMAT then combines relational, ordinal and JSON scalar data.
--------------------------------------------------------------------------------
CREATE TABLE sq_hard_w13_release (
  release_id   INT NOT NULL PRIMARY KEY,
  release_name VARCHAR(30) NOT NULL,
  sort_key     VARCHAR(120) INVISIBLE,
  doc          JSON NOT NULL,
  KEY ix_sq_hard_w13_natural (sort_key, release_name)
) ENGINE = InnoDB;

INSERT INTO sq_hard_w13_release
  (release_id, release_name, sort_key, doc)
VALUES
  (1, 'app-2', NATURAL_SORT_KEY('app-2'),
   '{"tags":["sql","two"],"score":2}'),
  (2, 'app-10', NATURAL_SORT_KEY('app-10'),
   '{"tags":["sql","ten"],"score":10}'),
  (3, 'app-1', NATURAL_SORT_KEY('app-1'),
   '{"tags":["one"],"score":1}'),
  (4, 'app-02', NATURAL_SORT_KEY('app-02'),
   '{"tags":["zero","two"],"score":2}');

SELECT r.release_id,
       r.release_name,
       jt.tag_no,
       jt.tag_name,
       SFORMAT('{}#{}:{}', r.release_id, jt.tag_no, jt.tag_name)
         AS formatted_tag,
       SUM(JSON_VALUE(r.doc, '$.score')) OVER w_release
         AS repeated_score,
       JSON_LOOSE(r.doc) AS loose_doc
FROM sq_hard_w13_release r
JOIN JSON_TABLE(
  r.doc,
  '$.tags[*]' COLUMNS (
    tag_no   FOR ORDINALITY,
    tag_name VARCHAR(20) PATH '$' ERROR ON ERROR
  )
) jt ON TRUE
WINDOW w_release AS (
  PARTITION BY r.release_id
  ORDER BY jt.tag_no
  ROWS BETWEEN UNBOUNDED PRECEDING AND CURRENT ROW
)
ORDER BY r.sort_key, r.release_name, jt.tag_no;

SET @sq_hard_w13_natural = (
  SELECT GROUP_CONCAT(
           release_name ORDER BY sort_key, release_name SEPARATOR '/'
         )
  FROM sq_hard_w13_release);

SET @sq_hard_w13_tags = (
  SELECT COUNT(*)
  FROM sq_hard_w13_release r
  JOIN JSON_TABLE(
    r.doc,
    '$.tags[*]' COLUMNS (
      tag_no   FOR ORDINALITY,
      tag_name VARCHAR(20) PATH '$'
    )
  ) jt ON TRUE);

INSERT INTO sq_hard_w13_note (note_key, note_text, note_value)
VALUES ('natural-json', @sq_hard_w13_natural, @sq_hard_w13_tags);

--------------------------------------------------------------------------------
-- W13-B: the table begins ordinary, gains hidden ROW_START/ROW_END columns via
-- ALTER, is read through two FOR SYSTEM_TIME shapes, then loses versioning and
-- the generated columns. FOR UPDATE WAIT has a lock-option grammar no other
-- MySQL-family statement in this wave shares.
--------------------------------------------------------------------------------
CREATE TABLE sq_hard_w13_versioned (
  row_id INT NOT NULL PRIMARY KEY,
  amount INT NOT NULL
) ENGINE = InnoDB;

INSERT INTO sq_hard_w13_versioned (row_id, amount) VALUES (1, 10);

ALTER TABLE sq_hard_w13_versioned ADD SYSTEM VERSIONING;

SET @sq_hard_w13_cutover = NOW(6);
DO SLEEP(0.01);

UPDATE sq_hard_w13_versioned SET amount = 15 WHERE row_id = 1;

SELECT row_id, amount, ROW_START, ROW_END
FROM sq_hard_w13_versioned FOR SYSTEM_TIME ALL
ORDER BY ROW_START;

SET @sq_hard_w13_old_rows = (
  SELECT COUNT(*)
  FROM sq_hard_w13_versioned FOR SYSTEM_TIME AS OF @sq_hard_w13_cutover
  WHERE amount = 10);

SET @sq_hard_w13_history_rows = (
  SELECT COUNT(*)
  FROM sq_hard_w13_versioned FOR SYSTEM_TIME ALL);

START TRANSACTION;
SELECT row_id, amount
FROM sq_hard_w13_versioned
WHERE row_id = 1
FOR UPDATE WAIT 1;
COMMIT;

ALTER TABLE sq_hard_w13_versioned DROP SYSTEM VERSIONING;

INSERT INTO sq_hard_w13_note (note_key, note_text, note_value)
VALUES (
  'alter-versioning',
  CONCAT('old=', @sq_hard_w13_old_rows,
         ',history=', @sq_hard_w13_history_rows,
         ',current=', (SELECT amount
                       FROM sq_hard_w13_versioned
                       WHERE row_id = 1)),
  @sq_hard_w13_history_rows
);

--------------------------------------------------------------------------------
-- W13-C: a prepared CALL binds the second marker as an OUT user variable. A
-- second dynamic statement supplies DEFAULT as a parameter indicator, which is
-- neither an identifier nor a scalar expression in this position.
--------------------------------------------------------------------------------
DELIMITER $$
CREATE PROCEDURE sq_hard_w13_double(IN base_value INT, OUT doubled INT)
BEGIN
  SET doubled = base_value * 2;
END$$
DELIMITER ;

SET @sq_hard_w13_dynamic_out = 0;

EXECUTE IMMEDIATE 'CALL sq_hard_w13_double(?, ?)'
  USING 21, @sq_hard_w13_dynamic_out;

CREATE TABLE sq_hard_w13_bind (
  bind_id INT NOT NULL DEFAULT 100 PRIMARY KEY,
  note    VARCHAR(30) NOT NULL DEFAULT 'defaulted'
) ENGINE = InnoDB;

EXECUTE IMMEDIATE
  'INSERT INTO sq_hard_w13_bind (bind_id, note) VALUES (?, ?)'
  USING DEFAULT, 'bound';

INSERT INTO sq_hard_w13_note (note_key, note_text, note_value)
SELECT 'dynamic-bind',
       SFORMAT('out={};default={}', @sq_hard_w13_dynamic_out, bind_id),
       @sq_hard_w13_dynamic_out
FROM sq_hard_w13_bind;

TRUNCATE TABLE sq_hard_w13_bind WAIT 1;

--------------------------------------------------------------------------------
-- W13-D: a harmless IF EXISTS miss populates the diagnostics area. The routine
-- keeps GET DIAGNOSTICS in the same server call as the warning-producing DDL,
-- so raw clients and clients that inspect every result preserve its cardinality.
-- SHOW COUNT/WARNINGS/ERRORS then exercise the result-producing command forms.
--------------------------------------------------------------------------------
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
VALUES ('diagnostics', 'drop-if-exists warning', @sq_hard_w13_warning_count);

--------------------------------------------------------------------------------
-- Wave-13 self-verification.
--------------------------------------------------------------------------------
CALL sq_hard_assert(
  @sq_hard_w13_natural = 'app-1/app-02/app-2/app-10',
  CONCAT('natural order: ', COALESCE(@sq_hard_w13_natural, 'NULL')));
CALL sq_hard_assert(
  @sq_hard_w13_tags = 7,
  CONCAT('json tag rows: ', COALESCE(@sq_hard_w13_tags, 'NULL')));
CALL sq_hard_assert(
  @sq_hard_w13_old_rows = 1 AND @sq_hard_w13_history_rows = 2
  AND (SELECT amount FROM sq_hard_w13_versioned WHERE row_id = 1) = 15,
  CONCAT('alter versioning: ', COALESCE(@sq_hard_w13_old_rows, 'NULL'),
         '/', COALESCE(@sq_hard_w13_history_rows, 'NULL')));
CALL sq_hard_assert(
  (SELECT COUNT(*)
   FROM information_schema.columns
   WHERE table_schema = 'sq_hard_mariadb'
     AND table_name = 'sq_hard_w13_versioned'
     AND column_name IN ('ROW_START', 'ROW_END')) = 0,
  'drop system versioning columns');
CALL sq_hard_assert(
  @sq_hard_w13_dynamic_out = 42
  AND (SELECT COUNT(*) FROM sq_hard_w13_bind) = 0,
  CONCAT('dynamic/truncate: ',
         COALESCE(@sq_hard_w13_dynamic_out, 'NULL')));
CALL sq_hard_assert(
  @sq_hard_w13_warning_count = 1,
  CONCAT('diagnostic warnings: ',
         COALESCE(@sq_hard_w13_warning_count, 'NULL')));
CALL sq_hard_assert(
  (SELECT COUNT(*) FROM sq_hard_w13_note) = 4,
  CONCAT('wave13 note rows: ',
         (SELECT COUNT(*) FROM sq_hard_w13_note)));

--------------------------------------------------------------------------------
-- ULTRA WAVE 14 -- MariaDB 12.2 grammar and metadata that older parsers cannot
-- infer. Routine parameters carry executable DEFAULT expressions, UPDATE OF
-- gives a trigger its own column list, two new INFORMATION_SCHEMA surfaces
-- expose those declarations, JSON functions cross the former 32-level nesting
-- boundary, and 12.2 optimizer hints live inside a comment while changing the
-- query plan outside it.
--------------------------------------------------------------------------------
CREATE TABLE sq_hard_w14_note (
  note_key   VARCHAR(30) NOT NULL PRIMARY KEY,
  note_text  VARCHAR(500),
  note_value BIGINT
) ENGINE = InnoDB;

--------------------------------------------------------------------------------
-- W14-A: omitted CALL arguments resolve from routine-parameter DEFAULT clauses.
-- The default label deliberately contains every SQL comment introducer plus
-- two dollar signs. INFORMATION_SCHEMA.PARAMETERS returns the original SQL
-- expressions, including quotes, through the new PARAMETER_DEFAULT column.
--------------------------------------------------------------------------------
DELIMITER //
CREATE PROCEDURE sq_hard_w14_defaults(
  IN p_base  INT,
  IN p_scale DECIMAL(8, 2) DEFAULT 1.50,
  IN p_label VARCHAR(80) DEFAULT 'semi;--/*label*/#$$'
)
MODIFIES SQL DATA
COMMENT '12.2 parameter defaults; delimiter // remains active'
BEGIN
  INSERT INTO sq_hard_w14_note (note_key, note_text, note_value)
  VALUES (
    'parameter-defaults',
    CONCAT(
      'scaled=', CAST(p_base * p_scale AS DECIMAL(10, 2)),
      ',label=', p_label
    ),
    ROUND(p_base * p_scale * 100)
  );
END//
DELIMITER ;

CALL sq_hard_w14_defaults(4);

SELECT p.ordinal_position,
       p.parameter_mode,
       p.parameter_name,
       p.data_type,
       p.dtd_identifier,
       p.parameter_default
FROM information_schema.parameters AS p
WHERE p.specific_schema = 'sq_hard_mariadb'
  AND p.specific_name = 'sq_hard_w14_defaults'
ORDER BY p.ordinal_position;

SELECT COUNT(*),
       SUM(p.parameter_default IS NOT NULL),
       GROUP_CONCAT(
         CONCAT(
           p.parameter_name,
           '=',
           COALESCE(p.parameter_default, '<required>')
         )
         ORDER BY p.ordinal_position
         SEPARATOR '/'
       )
INTO @sq_hard_w14_parameter_rows,
     @sq_hard_w14_optional_rows,
     @sq_hard_w14_parameter_shape
FROM information_schema.parameters AS p
WHERE p.specific_schema = 'sq_hard_mariadb'
  AND p.specific_name = 'sq_hard_w14_defaults';

--------------------------------------------------------------------------------
-- W14-B: UPDATE OF is part of the trigger event, not an UPDATE statement. The
-- first UPDATE names no watched column and must not fire; the second names both
-- watched columns but fires once. The 12.2 metadata table mints one row for
-- each event column even though neither is a column of that metadata query.
--------------------------------------------------------------------------------
CREATE TABLE sq_hard_w14_event (
  event_id     INT NOT NULL PRIMARY KEY,
  amount       INT NOT NULL,
  note         VARCHAR(80) NOT NULL,
  ignored      INT NOT NULL DEFAULT 0,
  fired_count  INT NOT NULL DEFAULT 0,
  payload      JSON NOT NULL CHECK (JSON_VALID(payload)),
  KEY ix_sq_hard_w14_amount (amount),
  KEY ix_sq_hard_w14_note (note),
  KEY ix_sq_hard_w14_ignored (ignored)
) ENGINE = InnoDB;

INSERT INTO sq_hard_w14_event (
  event_id, amount, note, ignored, fired_count, payload
)
VALUES (1, 10, 'seed', 0, 0, '{"kind":"seed"}');

DELIMITER |!|
CREATE TRIGGER sq_hard_w14_update_of
BEFORE UPDATE OF amount, note ON sq_hard_w14_event
FOR EACH ROW
BEGIN
  SET NEW.fired_count = OLD.fired_count + 1;
  SET NEW.payload = JSON_SET(
    OLD.payload,
    '$.lastChange',
    JSON_OBJECT(
      'oldAmount', OLD.amount,
      'newAmount', NEW.amount,
      'newNote',   NEW.note
    )
  );
END|!|
DELIMITER ;

UPDATE sq_hard_w14_event
SET ignored = ignored + 1
WHERE event_id = 1;

UPDATE sq_hard_w14_event
SET amount = amount + 5,
    note = 'raised;--/*note*/#'
WHERE event_id = 1;

SELECT tuc.trigger_catalog,
       tuc.trigger_schema,
       tuc.trigger_name,
       tuc.event_object_schema,
       tuc.event_object_table,
       tuc.event_object_column
FROM information_schema.triggered_update_columns AS tuc
WHERE tuc.trigger_schema = 'sq_hard_mariadb'
  AND tuc.trigger_name = 'sq_hard_w14_update_of'
ORDER BY tuc.event_object_column;

SELECT COUNT(*),
       GROUP_CONCAT(
         tuc.event_object_column
         ORDER BY tuc.event_object_column
         SEPARATOR '/'
       )
INTO @sq_hard_w14_trigger_columns,
     @sq_hard_w14_trigger_shape
FROM information_schema.triggered_update_columns AS tuc
WHERE tuc.trigger_schema = 'sq_hard_mariadb'
  AND tuc.trigger_name = 'sq_hard_w14_update_of';

INSERT INTO sq_hard_w14_note (note_key, note_text, note_value)
SELECT 'update-of',
       CONCAT(
         'columns=', @sq_hard_w14_trigger_shape,
         ',fired=', e.fired_count,
         ',old=', JSON_VALUE(e.payload, '$.lastChange.oldAmount'),
         ',new=', JSON_VALUE(e.payload, '$.lastChange.newAmount')
       ),
       @sq_hard_w14_trigger_columns
FROM sq_hard_w14_event AS e
WHERE e.event_id = 1;

--------------------------------------------------------------------------------
-- W14-C: MariaDB 12.2 removed the JSON-function nesting ceiling of 32. Forty
-- object levels are built without dynamic SQL; the path is built separately so
-- JSON_EXTRACT must walk every level and still return a string containing SQL
-- comment tokens that remain JSON data.
--------------------------------------------------------------------------------
SET @sq_hard_w14_deep_json = CONCAT(
  REPEAT('{"level":', 40),
  '"leaf;--/*json*/#"',
  REPEAT('}', 40)
);
SET @sq_hard_w14_deep_path = CONCAT('$', REPEAT('.level', 40));

SELECT JSON_VALID(@sq_hard_w14_deep_json),
       JSON_DEPTH(@sq_hard_w14_deep_json),
       JSON_UNQUOTE(
         JSON_EXTRACT(
           @sq_hard_w14_deep_json,
           @sq_hard_w14_deep_path
         )
       ),
       JSON_LENGTH(JSON_DETAILED(@sq_hard_w14_deep_json, 2))
INTO @sq_hard_w14_json_valid,
     @sq_hard_w14_json_depth,
     @sq_hard_w14_json_leaf,
     @sq_hard_w14_json_root_members;

SELECT @sq_hard_w14_json_valid AS valid_document,
       @sq_hard_w14_json_depth AS document_depth,
       @sq_hard_w14_json_leaf AS leaf_value,
       @sq_hard_w14_json_root_members AS root_members;

INSERT INTO sq_hard_w14_note (note_key, note_text, note_value)
VALUES (
  'deep-json',
  CONCAT(
    'valid=', @sq_hard_w14_json_valid,
    ',depth=', @sq_hard_w14_json_depth,
    ',leaf=', @sq_hard_w14_json_leaf
  ),
  @sq_hard_w14_json_depth
);

--------------------------------------------------------------------------------
-- W14-D: optimizer directives are comment tokens to the SQL lexer, yet each
-- argument has table/index completion semantics. The query is followed by two
-- 12.2 Oracle-compatibility functions and MariaDB's trailing PROCEDURE clause,
-- whose ANALYSE routine is unrelated to CREATE PROCEDURE above.
--------------------------------------------------------------------------------
EXPLAIN FORMAT = JSON
SELECT /*+ NO_INDEX_MERGE(
              e ix_sq_hard_w14_amount, ix_sq_hard_w14_note
            )
            NO_ROWID_FILTER(e ix_sq_hard_w14_amount)
            JOIN_INDEX(e ix_sq_hard_w14_amount) */
       e.event_id,
       e.amount,
       e.note
FROM sq_hard_w14_event AS e
WHERE e.amount = 15 OR e.note = 'raised;--/*note*/#';

SELECT /*+ NO_INDEX_MERGE(
              e ix_sq_hard_w14_amount, ix_sq_hard_w14_note
            )
            NO_ROWID_FILTER(e ix_sq_hard_w14_amount) */
       COUNT(*)
INTO @sq_hard_w14_hint_rows
FROM sq_hard_w14_event AS e
WHERE e.amount = 15 OR e.note = 'raised;--/*note*/#';

SELECT TO_NUMBER('-1234.50'),
       DATE_FORMAT(
         TRUNC(DATE '2026-07-28', 'MONTH'),
         '%Y-%m-%d'
       ),
       DATE_FORMAT(
         TRUNC(DATE '2026-07-28', 'YEAR'),
         '%Y-%m-%d'
       )
INTO @sq_hard_w14_number,
     @sq_hard_w14_month_start,
     @sq_hard_w14_year_start;

SELECT e.event_id, e.amount, e.note, e.fired_count
FROM sq_hard_w14_event AS e
PROCEDURE ANALYSE(10, 1000);

INSERT INTO sq_hard_w14_note (note_key, note_text, note_value)
VALUES (
  'hints-functions',
  CONCAT(
    'hint=', @sq_hard_w14_hint_rows,
    ',number=', CAST(@sq_hard_w14_number AS DECIMAL(10, 2)),
    ',month=', @sq_hard_w14_month_start,
    ',year=', @sq_hard_w14_year_start
  ),
  ROUND(ABS(@sq_hard_w14_number) * 100) + @sq_hard_w14_hint_rows
);

--------------------------------------------------------------------------------
-- Wave-14 self-verification.
--------------------------------------------------------------------------------
CALL sq_hard_assert(
  @sq_hard_w14_parameter_rows = 3
  AND @sq_hard_w14_optional_rows = 2
  AND @sq_hard_w14_parameter_shape LIKE
      'p_base=<required>/p_scale=1.50/p_label=%semi;--/*label*/#$$%',
  CONCAT('parameter defaults: ',
         COALESCE(@sq_hard_w14_parameter_shape, 'NULL')));
CALL sq_hard_assert(
  (SELECT note_text
   FROM sq_hard_w14_note
   WHERE note_key = 'parameter-defaults') =
    'scaled=6.00,label=semi;--/*label*/#$$'
  AND (SELECT note_value
       FROM sq_hard_w14_note
       WHERE note_key = 'parameter-defaults') = 600,
  'defaulted procedure call');
CALL sq_hard_assert(
  @sq_hard_w14_trigger_columns = 2
  AND @sq_hard_w14_trigger_shape = 'amount/note'
  AND (SELECT fired_count FROM sq_hard_w14_event WHERE event_id = 1) = 1
  AND (SELECT amount FROM sq_hard_w14_event WHERE event_id = 1) = 15,
  CONCAT('update-of metadata: ',
         COALESCE(@sq_hard_w14_trigger_shape, 'NULL')));
CALL sq_hard_assert(
  (SELECT JSON_VALUE(payload, '$.lastChange.oldAmount')
   FROM sq_hard_w14_event WHERE event_id = 1) = 10
  AND (SELECT JSON_VALUE(payload, '$.lastChange.newAmount')
       FROM sq_hard_w14_event WHERE event_id = 1) = 15,
  'update-of JSON row image');
CALL sq_hard_assert(
  @sq_hard_w14_json_valid = 1
  AND @sq_hard_w14_json_depth = 41
  AND @sq_hard_w14_json_leaf = 'leaf;--/*json*/#'
  AND @sq_hard_w14_json_root_members = 1,
  CONCAT('deep json: ', COALESCE(@sq_hard_w14_json_depth, -1),
         '/', COALESCE(@sq_hard_w14_json_leaf, 'NULL')));
CALL sq_hard_assert(
  @sq_hard_w14_hint_rows = 1
  AND @sq_hard_w14_number = -1234.50
  AND @sq_hard_w14_month_start = '2026-07-01'
  AND @sq_hard_w14_year_start = '2026-01-01',
  CONCAT('12.2 functions: ',
         COALESCE(@sq_hard_w14_number, 0),
         '/', COALESCE(@sq_hard_w14_month_start, 'NULL')));
CALL sq_hard_assert(
  (SELECT COUNT(*) FROM sq_hard_w14_note) = 4,
  CONCAT('wave14 note rows: ',
         (SELECT COUNT(*) FROM sq_hard_w14_note)));

--------------------------------------------------------------------------------
-- ULTRA WAVE 15 -- MariaDB semantic/parser collision chamber.
-- A statement-scoped SQL mode changes assignment evaluation under an UPDATE OF
-- trigger; a two-level JSON_TABLE feeds INSERT...SELECT...ON DUPLICATE KEY
-- UPDATE...RETURNING; and that materialized result becomes a cyclic recursive
-- graph whose rows flow through a named window and OFFSET/FETCH WITH TIES.
--------------------------------------------------------------------------------
CREATE TABLE sq_hard_w15_note (
  note_key   VARCHAR(30) NOT NULL PRIMARY KEY,
  note_text  VARCHAR(600) NOT NULL,
  note_value BIGINT NOT NULL
) ENGINE = InnoDB;

--------------------------------------------------------------------------------
-- W15-A: MariaDB normally evaluates single-table UPDATE assignments from left
-- to right. SIMULTANEOUS_ASSIGNMENT makes the same token stream swap values.
-- SET STATEMENT scopes that sql_mode to one UPDATE, while UPDATE OF limits the
-- trigger event to the two watched columns and JSON captures both row images.
--------------------------------------------------------------------------------
CREATE TABLE sq_hard_w15_swap (
  swap_id     INT NOT NULL PRIMARY KEY,
  left_value  INT NOT NULL,
  right_value INT NOT NULL,
  pair_text   VARCHAR(40)
    AS (CONCAT(left_value, '/', right_value)) PERSISTENT,
  payload     JSON NOT NULL CHECK (JSON_VALID(payload)),
  KEY ix_sq_hard_w15_pair (pair_text)
) ENGINE = InnoDB;

INSERT INTO sq_hard_w15_swap (
  swap_id, left_value, right_value, payload
)
VALUES
  (1, 1, 2, '{"mode":"sequential"}'),
  (2, 1, 2, '{"mode":"simultaneous"}');

DELIMITER |/|
CREATE TRIGGER sq_hard_w15_swap_update
BEFORE UPDATE OF left_value, right_value ON sq_hard_w15_swap
FOR EACH ROW
BEGIN
  SET NEW.payload = JSON_SET(
    OLD.payload,
    '$.oldPair', JSON_ARRAY(OLD.left_value, OLD.right_value),
    '$.newPair', JSON_ARRAY(NEW.left_value, NEW.right_value)
  );
END|/|
DELIMITER ;

UPDATE sq_hard_w15_swap
SET left_value = right_value,
    right_value = left_value
WHERE swap_id = 1;

SET STATEMENT
  sql_mode =
    'STRICT_TRANS_TABLES,NO_ENGINE_SUBSTITUTION,SIMULTANEOUS_ASSIGNMENT'
FOR
UPDATE sq_hard_w15_swap
SET left_value = right_value,
    right_value = left_value
WHERE swap_id = 2;

SELECT GROUP_CONCAT(
         CONCAT(
           s.swap_id, '=', s.pair_text, ':',
           JSON_VALUE(s.payload, '$.mode'), ':',
           JSON_VALUE(s.payload, '$.oldPair[0]'), '/',
           JSON_VALUE(s.payload, '$.oldPair[1]'), '>',
           JSON_VALUE(s.payload, '$.newPair[0]'), '/',
           JSON_VALUE(s.payload, '$.newPair[1]')
         )
         ORDER BY s.swap_id
         SEPARATOR ','
       ),
       SUM(s.left_value * 10 + s.right_value)
INTO @sq_hard_w15_assignment_shape,
     @sq_hard_w15_assignment_value
FROM sq_hard_w15_swap AS s;

INSERT INTO sq_hard_w15_note (note_key, note_text, note_value)
VALUES (
  'simultaneous-assignment',
  @sq_hard_w15_assignment_shape,
  @sq_hard_w15_assignment_value
);

--------------------------------------------------------------------------------
-- W15-B: two documents expand through two nested JSON_TABLE COLUMNS lists.
-- fact_id=10 appears in both documents, so one INSERT statement first inserts
-- it and later takes the duplicate branch. VALUES() means the incoming image,
-- qualified columns mean the stored image, and RETURNING projects the final
-- PERSISTENT generated key plus a JSON scalar for every affected source row.
--------------------------------------------------------------------------------
CREATE TABLE sq_hard_w15_document (
  document_id INT NOT NULL PRIMARY KEY,
  body        JSON NOT NULL CHECK (JSON_VALID(body))
) ENGINE = InnoDB;

CREATE TABLE sq_hard_w15_fact (
  fact_id    INT NOT NULL PRIMARY KEY,
  bucket     VARCHAR(20) NOT NULL,
  qty        INT NOT NULL,
  source_key VARCHAR(60)
    AS (CONCAT(bucket, '#', fact_id)) PERSISTENT,
  meta       JSON NOT NULL CHECK (JSON_VALID(meta)),
  UNIQUE KEY uq_sq_hard_w15_source (source_key),
  CONSTRAINT ck_sq_hard_w15_qty CHECK (qty > 0)
) ENGINE = InnoDB;

INSERT INTO sq_hard_w15_document (document_id, body)
VALUES
  (
    1,
    '{"batches":[{"bucket":"alpha","items":[{"id":10,"qty":2},{"id":11,"qty":3}]},{"bucket":"beta","items":[{"id":20,"qty":4}]}]}'
  ),
  (
    2,
    '{"batches":[{"bucket":"alpha","items":[{"id":10,"qty":5}]},{"bucket":"gamma","items":[{"id":30,"qty":7}]}]}'
  );

INSERT INTO sq_hard_w15_fact (fact_id, bucket, qty, meta)
SELECT jt.fact_id,
       jt.bucket,
       jt.qty,
       JSON_OBJECT(
         'document', d.document_id,
         'batch', jt.batch_no,
         'item', jt.item_no,
         'lexer', 'semi;--/*json*/#'
       )
FROM sq_hard_w15_document AS d
JOIN JSON_TABLE(
  d.body,
  '$.batches[*]' COLUMNS (
    batch_no FOR ORDINALITY,
    bucket VARCHAR(20) PATH '$.bucket' ERROR ON ERROR,
    NESTED PATH '$.items[*]' COLUMNS (
      item_no FOR ORDINALITY,
      fact_id INT PATH '$.id' ERROR ON ERROR,
      qty     INT PATH '$.qty' DEFAULT '1' ON EMPTY ERROR ON ERROR
    )
  )
) AS jt ON TRUE
ORDER BY d.document_id, jt.batch_no, jt.item_no
ON DUPLICATE KEY UPDATE
  bucket = VALUES(bucket),
  qty = sq_hard_w15_fact.qty + VALUES(qty),
  meta = JSON_MERGE_PATCH(sq_hard_w15_fact.meta, VALUES(meta))
RETURNING fact_id,
          bucket,
          qty,
          source_key,
          JSON_VALUE(meta, '$.document') AS last_document,
          JSON_VALUE(meta, '$.lexer') AS lexer_payload;

SELECT GROUP_CONCAT(
         CONCAT(
           f.source_key, '=', f.qty, ':d',
           JSON_VALUE(f.meta, '$.document')
         )
         ORDER BY f.fact_id
         SEPARATOR '/'
       ),
       SUM(f.qty)
INTO @sq_hard_w15_fact_shape,
     @sq_hard_w15_fact_total
FROM sq_hard_w15_fact AS f;

INSERT INTO sq_hard_w15_note (note_key, note_text, note_value)
VALUES (
  'json-upsert-returning',
  @sq_hard_w15_fact_shape,
  @sq_hard_w15_fact_total
);

--------------------------------------------------------------------------------
-- W15-C: a back-edge closes 10->11->20->10. CYCLE fact_id RESTRICT suppresses
-- that re-entry but keeps 20->30. The derived table must preserve the recursive
-- CTE's minted columns and a named running window before the outer aggregate.
-- The independent FETCH query requests one row yet returns both qty=7 ties.
--------------------------------------------------------------------------------
CREATE TABLE sq_hard_w15_edge (
  source_id INT NOT NULL,
  target_id INT NOT NULL,
  PRIMARY KEY (source_id, target_id),
  CONSTRAINT fk_sq_hard_w15_edge_source
    FOREIGN KEY (source_id) REFERENCES sq_hard_w15_fact (fact_id),
  CONSTRAINT fk_sq_hard_w15_edge_target
    FOREIGN KEY (target_id) REFERENCES sq_hard_w15_fact (fact_id)
) ENGINE = InnoDB;

INSERT INTO sq_hard_w15_edge (source_id, target_id)
VALUES (10, 11), (11, 20), (20, 10), (20, 30);

WITH RECURSIVE walk (fact_id, qty, depth_no, fact_path) AS (
  SELECT f.fact_id,
         f.qty,
         0,
         CAST(f.fact_id AS CHAR(100))
  FROM sq_hard_w15_fact AS f
  WHERE f.fact_id = 10
  UNION ALL
  SELECT next_fact.fact_id,
         next_fact.qty,
         walk.depth_no + 1,
         CONCAT(walk.fact_path, '>', next_fact.fact_id)
  FROM walk
  JOIN sq_hard_w15_edge AS e
    ON e.source_id = walk.fact_id
  JOIN sq_hard_w15_fact AS next_fact
    ON next_fact.fact_id = e.target_id
)
CYCLE fact_id RESTRICT
SELECT GROUP_CONCAT(
         CONCAT(fact_id, ':', qty)
         ORDER BY depth_no, fact_id
         SEPARATOR '/'
       ),
       MAX(running_qty),
       COUNT(*)
INTO @sq_hard_w15_walk_shape,
     @sq_hard_w15_running_qty,
     @sq_hard_w15_walk_rows
FROM (
  SELECT fact_id,
         qty,
         depth_no,
         fact_path,
         SUM(qty) OVER w_walk AS running_qty
  FROM walk
  WINDOW w_walk AS (
    ORDER BY depth_no, fact_id
    ROWS BETWEEN UNBOUNDED PRECEDING AND CURRENT ROW
  )
) AS windowed_walk;

SELECT GROUP_CONCAT(fact_id ORDER BY fact_id SEPARATOR '/')
INTO @sq_hard_w15_ties
FROM (
  SELECT fact_id
  FROM sq_hard_w15_fact
  ORDER BY qty DESC
  OFFSET 0 ROWS FETCH FIRST 1 ROWS WITH TIES
) AS top_with_ties;

INSERT INTO sq_hard_w15_note (note_key, note_text, note_value)
VALUES (
  'cycle-window-ties',
  CONCAT(
    'walk=', @sq_hard_w15_walk_shape,
    ',ties=', @sq_hard_w15_ties,
    ',running=', @sq_hard_w15_running_qty
  ),
  @sq_hard_w15_walk_rows * 100 + @sq_hard_w15_running_qty
);

--------------------------------------------------------------------------------
-- Wave-15 self-verification.
--------------------------------------------------------------------------------
CALL sq_hard_assert(
  @sq_hard_w15_assignment_shape =
    '1=2/2:sequential:1/2>2/2,2=2/1:simultaneous:1/2>2/1'
  AND @sq_hard_w15_assignment_value = 43,
  CONCAT('simultaneous assignment: ',
         COALESCE(@sq_hard_w15_assignment_shape, 'NULL')));
CALL sq_hard_assert(
  @sq_hard_w15_fact_shape =
    'alpha#10=7:d2/alpha#11=3:d1/beta#20=4:d1/gamma#30=7:d2'
  AND @sq_hard_w15_fact_total = 21
  AND (SELECT COUNT(*) FROM sq_hard_w15_fact) = 4,
  CONCAT('json upsert: ', COALESCE(@sq_hard_w15_fact_shape, 'NULL')));
CALL sq_hard_assert(
  @sq_hard_w15_walk_shape = '10:7/11:3/20:4/30:7'
  AND @sq_hard_w15_running_qty = 21
  AND @sq_hard_w15_walk_rows = 4
  AND @sq_hard_w15_ties = '10/30',
  CONCAT('cycle/window/ties: ',
         COALESCE(@sq_hard_w15_walk_shape, 'NULL'),
         '/', COALESCE(@sq_hard_w15_ties, 'NULL')));
CALL sq_hard_assert(
  (SELECT COUNT(*) FROM sq_hard_w15_note) = 3,
  CONCAT('wave15 note rows: ',
         (SELECT COUNT(*) FROM sq_hard_w15_note)));

--------------------------------------------------------------------------------
-- ULTRA WAVE 16 -- MariaDB 12.2 cursor/function/DML protocol endgame.
-- A SYS_REFCURSOR transports JSON_TABLE and inherited-window columns across a
-- stored-function boundary; an OUT/INOUT stored function mutates user-variable
-- arguments while omitting a trailing DEFAULT parameter; and REPLACE...SELECT
-- consumes another correlated JSON_TABLE before RETURNING generated columns,
-- JSON expressions and a scalar subquery. Every value is asserted afterward.
--------------------------------------------------------------------------------
CREATE TABLE sq_hard_w16_note (
  note_key   VARCHAR(30) NOT NULL PRIMARY KEY,
  note_text  VARCHAR(700) NOT NULL,
  note_value BIGINT NOT NULL
) ENGINE = InnoDB;

CREATE TABLE sq_hard_w16_document (
  document_id INT NOT NULL PRIMARY KEY,
  body        JSON NOT NULL CHECK (JSON_VALID(body))
) ENGINE = InnoDB;

INSERT INTO sq_hard_w16_document (document_id, body)
VALUES
  (
    1,
    '{"bucket":"alpha","items":[{"id":10,"qty":2},{"id":11,"qty":5}]}'
  ),
  (
    2,
    '{"bucket":"beta","items":[{"id":20,"qty":7},{"id":21,"qty":11}]}'
  );

--------------------------------------------------------------------------------
-- W16-A: MariaDB 12.0+ permits a stored function to RETURN SYS_REFCURSOR.
-- OPEN owns a two-CTE query: JSON_TABLE mints five columns, then two inherited
-- named windows mint two more. The consuming procedure must FETCH that inferred
-- seven-column row type while c%NOTFOUND is still parsed as a cursor attribute.
--------------------------------------------------------------------------------
SET SESSION sql_mode = 'ORACLE';

DELIMITER $$
CREATE FUNCTION sq_hard_w16_cursor(p_floor INT)
RETURN SYS_REFCURSOR AS
  c SYS_REFCURSOR;
BEGIN
  OPEN c FOR
    WITH expanded AS (
      SELECT d.document_id,
             jt.bucket_name,
             jt.item_no,
             jt.item_id,
             jt.qty
      FROM sq_hard_w16_document d
      JOIN JSON_TABLE(
        d.body,
        '$' COLUMNS (
          bucket_name VARCHAR(20) PATH '$.bucket' ERROR ON ERROR,
          NESTED PATH '$.items[*]' COLUMNS (
            item_no FOR ORDINALITY,
            item_id INT PATH '$.id' ERROR ON ERROR,
            qty     INT PATH '$.qty' ERROR ON ERROR
          )
        )
      ) jt ON TRUE
    ),
    ranked AS (
      SELECT e.*,
             SUM(e.qty) OVER w_running AS running_qty,
             ROW_NUMBER() OVER w_desc AS descending_no
      FROM expanded e
      WHERE e.qty >= p_floor
      WINDOW
        w_ordered AS (
          PARTITION BY e.bucket_name
          ORDER BY e.qty, e.item_id
        ),
        w_running AS (
          w_ordered ROWS BETWEEN UNBOUNDED PRECEDING AND CURRENT ROW
        ),
        w_desc AS (
          PARTITION BY e.bucket_name
          ORDER BY e.qty DESC, e.item_id
        )
    )
    SELECT document_id,
           bucket_name,
           item_no,
           item_id,
           qty,
           running_qty,
           descending_no
    FROM ranked
    ORDER BY bucket_name, qty, item_id;
  RETURN c;
END$$

CREATE PROCEDURE sq_hard_w16_consume(
  p_floor IN INT,
  p_shape OUT VARCHAR2,
  p_total OUT INT,
  p_rows  OUT INT
) AS
  one_document   INT;
  one_bucket     VARCHAR2(20);
  one_item_no    INT;
  one_item_id    INT;
  one_qty        INT;
  one_running    INT;
  one_descending INT;
  c SYS_REFCURSOR DEFAULT sq_hard_w16_cursor(p_floor);
BEGIN
  p_shape := '';
  p_total := 0;
  p_rows := 0;
  LOOP
    FETCH c INTO one_document,
                 one_bucket,
                 one_item_no,
                 one_item_id,
                 one_qty,
                 one_running,
                 one_descending;
    EXIT WHEN c%NOTFOUND;
    p_shape := p_shape
      || CASE WHEN p_shape = '' THEN '' ELSE '/' END
      || one_bucket || '#' || one_item_id || ':' || one_qty
      || ':' || one_running || ':' || one_descending;
    p_total := p_total + one_qty;
    p_rows := p_rows + 1;
  END LOOP;
  CLOSE c;
END$$
DELIMITER ;

CALL sq_hard_w16_consume(
  5,
  @sq_hard_w16_cursor_shape,
  @sq_hard_w16_cursor_total,
  @sq_hard_w16_cursor_rows
);

SET SESSION sql_mode = 'STRICT_TRANS_TABLES,NO_ENGINE_SUBSTITUTION';

INSERT INTO sq_hard_w16_note (note_key, note_text, note_value)
VALUES (
  'refcursor-json-window',
  @sq_hard_w16_cursor_shape,
  @sq_hard_w16_cursor_rows * 1000 + @sq_hard_w16_cursor_total
);

--------------------------------------------------------------------------------
-- W16-B: OUT and INOUT are legal on stored functions when the call is a SET
-- expression. P_SCALE is trailing and optional, so the invocation has three
-- arguments although metadata declares four. The function returns one scalar,
-- mutates two arguments and writes hostile comment tokens into a JSON string.
--------------------------------------------------------------------------------
DELIMITER //
CREATE FUNCTION sq_hard_w16_out_function(
  IN    p_base    INT,
  INOUT p_running INT,
  OUT   p_doc     JSON,
  IN    p_scale   INT DEFAULT 3
)
RETURNS INT
DETERMINISTIC
NO SQL
BEGIN
  SET p_running = COALESCE(p_running, 0) + p_base * p_scale;
  SET p_doc = JSON_OBJECT(
    'base', p_base,
    'scale', p_scale,
    'running', p_running,
    'lexer', 'semi;--/*json*/#$$'
  );
  RETURN p_running + p_scale;
END//
DELIMITER ;

SET @sq_hard_w16_running = 5;
SET @sq_hard_w16_doc = NULL;
SET @sq_hard_w16_return = sq_hard_w16_out_function(
  4,
  @sq_hard_w16_running,
  @sq_hard_w16_doc
);

INSERT INTO sq_hard_w16_note (note_key, note_text, note_value)
VALUES (
  'out-inout-default',
  CONCAT(
    'return=', @sq_hard_w16_return,
    ',running=', @sq_hard_w16_running,
    ',json=', JSON_VALUE(@sq_hard_w16_doc, '$.running'),
    ',lexer=', JSON_VALUE(@sq_hard_w16_doc, '$.lexer')
  ),
  @sq_hard_w16_return * 100 + @sq_hard_w16_running
);

--------------------------------------------------------------------------------
-- W16-C: REPLACE reads a correlated, nested JSON_TABLE plus a UNION ALL branch.
-- RETURNING is attached to the complete SELECT source and projects PERSISTENT
-- generated columns, a JSON scalar and a scalar subquery for every replaced row.
--------------------------------------------------------------------------------
CREATE TABLE sq_hard_w16_snapshot (
  item_id     INT NOT NULL PRIMARY KEY,
  bucket_name VARCHAR(20) NOT NULL,
  qty         INT NOT NULL,
  label_text  VARCHAR(80)
    AS (CONCAT(bucket_name, '#', item_id, '=', qty)) PERSISTENT,
  snapshot_doc JSON
    AS (
      JSON_OBJECT(
        'item', item_id,
        'bucket', bucket_name,
        'qty', qty
      )
    ) PERSISTENT,
  KEY ix_sq_hard_w16_snapshot_label (label_text)
) ENGINE = InnoDB;

INSERT INTO sq_hard_w16_snapshot (item_id, bucket_name, qty)
VALUES (20, 'stale', 1);

REPLACE INTO sq_hard_w16_snapshot (item_id, bucket_name, qty)
SELECT jt.item_id,
       jt.bucket_name,
       jt.qty
FROM sq_hard_w16_document AS d
JOIN JSON_TABLE(
  d.body,
  '$' COLUMNS (
    bucket_name VARCHAR(20) PATH '$.bucket' ERROR ON ERROR,
    NESTED PATH '$.items[*]' COLUMNS (
      item_id INT PATH '$.id' ERROR ON ERROR,
      qty     INT PATH '$.qty' ERROR ON ERROR
    )
  )
) AS jt ON TRUE
WHERE jt.qty >= 5
UNION ALL
SELECT 99, 'sentinel', 13
RETURNING item_id,
          bucket_name,
          qty,
          label_text,
          JSON_VALUE(snapshot_doc, '$.bucket') AS json_bucket,
          (SELECT COUNT(*) FROM sq_hard_w16_document) AS document_count;

SELECT GROUP_CONCAT(
         label_text
         ORDER BY item_id
         SEPARATOR '/'
       ),
       SUM(qty)
INTO @sq_hard_w16_snapshot_shape,
     @sq_hard_w16_snapshot_total
FROM sq_hard_w16_snapshot;

INSERT INTO sq_hard_w16_note (note_key, note_text, note_value)
VALUES (
  'replace-select-returning',
  @sq_hard_w16_snapshot_shape,
  @sq_hard_w16_snapshot_total
);

--------------------------------------------------------------------------------
-- Wave-16 self-verification.
--------------------------------------------------------------------------------
CALL sq_hard_assert(
  @sq_hard_w16_cursor_shape =
    'alpha#11:5:5:1/beta#20:7:7:2/beta#21:11:18:1'
  AND @sq_hard_w16_cursor_total = 23
  AND @sq_hard_w16_cursor_rows = 3,
  CONCAT('sys_refcursor: ',
         COALESCE(@sq_hard_w16_cursor_shape, 'NULL')));
CALL sq_hard_assert(
  @sq_hard_w16_return = 20
  AND @sq_hard_w16_running = 17
  AND JSON_VALUE(@sq_hard_w16_doc, '$.running') = 17
  AND JSON_VALUE(@sq_hard_w16_doc, '$.lexer') = 'semi;--/*json*/#$$',
  CONCAT('out/inout/default: ',
         COALESCE(@sq_hard_w16_return, -1),
         '/', COALESCE(@sq_hard_w16_running, -1)));
CALL sq_hard_assert(
  @sq_hard_w16_snapshot_shape =
    'alpha#11=5/beta#20=7/beta#21=11/sentinel#99=13'
  AND @sq_hard_w16_snapshot_total = 36
  AND (SELECT COUNT(*) FROM sq_hard_w16_snapshot) = 4,
  CONCAT('replace/select/returning: ',
         COALESCE(@sq_hard_w16_snapshot_shape, 'NULL')));
CALL sq_hard_assert(
  (SELECT COUNT(*) FROM sq_hard_w16_note) = 3,
  CONCAT('wave16 note rows: ',
         (SELECT COUNT(*) FROM sq_hard_w16_note)));

--------------------------------------------------------------------------------
-- ULTRA WAVE 17 -- vector/full-text hybrid-ranking scope singularity.
-- One physical table owns BTREE, FULLTEXT and option-bearing VECTOR indexes.
-- Two incompatible search scores become window ranks, UNION ALL turns those
-- ranks into a polymorphic stream, reciprocal-rank fusion groups it back into
-- documents, and inherited named windows rank the fused result. JSON_OBJECTAGG
-- finally serializes aliases that were minted in every preceding scope.
--------------------------------------------------------------------------------
CREATE TABLE sq_hard_w17_document (
  document_id INT NOT NULL,
  title_text  VARCHAR(40) NOT NULL,
  body_text   TEXT NOT NULL,
  embedding   VECTOR(3) NOT NULL,
  attributes  JSON NOT NULL
    CHECK (JSON_VALID(attributes)),
  PRIMARY KEY (document_id),
  FULLTEXT KEY ft_sq_hard_w17_document (title_text, body_text),
  VECTOR INDEX vx_sq_hard_w17_document (embedding)
    M=4 DISTANCE=cosine
) ENGINE = InnoDB;

CREATE TABLE sq_hard_w17_note (
  note_key   VARCHAR(30) NOT NULL PRIMARY KEY,
  note_text  VARCHAR(700) NOT NULL,
  note_value DECIMAL(20, 8) NOT NULL
) ENGINE = InnoDB;

INSERT INTO sq_hard_w17_document (
  document_id,
  title_text,
  body_text,
  embedding,
  attributes
)
VALUES
  (
    1,
    'parser',
    'sql parser ; -- literal /* formatter */',
    VEC_FromText('[1,0,0]'),
    JSON_OBJECT('tags', JSON_ARRAY('sql', 'parser'), 'boost', 9)
  ),
  (
    2,
    'optimizer',
    'sql query optimizer # literal $$',
    VEC_FromText('[0.9,0.1,0]'),
    JSON_OBJECT('tags', JSON_ARRAY('sql', 'optimizer'), 'boost', 7)
  ),
  (
    3,
    'vector',
    'vector search index',
    VEC_FromText('[0,1,0]'),
    JSON_OBJECT('tags', JSON_ARRAY('vector', 'search'), 'boost', 5)
  ),
  (
    4,
    'highlight',
    'format highlighter',
    VEC_FromText('[0,0,1]'),
    JSON_OBJECT('tags', JSON_ARRAY('format', 'highlight'), 'boost', 3)
  );

--------------------------------------------------------------------------------
-- W17-A: each source rank has a different ordering expression. RANK_UNION
-- erases that expression but preserves SOURCE_NAME as a discriminator.
-- W_SCORE inherits an empty W_ALL specification before adding its own order;
-- both ROW_NUMBER and DENSE_RANK consume the inherited specification.
--------------------------------------------------------------------------------
WITH
  vector_ranked AS (
    SELECT d.document_id,
           ROW_NUMBER() OVER (
             ORDER BY VEC_DISTANCE(
                        d.embedding,
                        VEC_FromText('[1,0,0]')
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
    SELECT document_id, 'vector' AS source_name, source_rank
    FROM vector_ranked
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
           r.source_shape,
           r.reciprocal_score,
           ROW_NUMBER() OVER w_score AS hybrid_position,
           DENSE_RANK() OVER w_score AS hybrid_dense_rank
    FROM reciprocal_scores AS r
    JOIN sq_hard_w17_document AS d USING (document_id)
    WINDOW
      w_all AS (),
      w_score AS (
        w_all
        ORDER BY r.reciprocal_score DESC, d.document_id
      )
  )
SELECT GROUP_CONCAT(
         CONCAT(document_id, ':', source_shape)
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
           'boost', JSON_VALUE(attributes, '$.boost')
         )
       )
INTO @sq_hard_w17_hybrid_shape,
     @sq_hard_w17_rank_total,
     @sq_hard_w17_hybrid_json
FROM hybrid_ranked;

--------------------------------------------------------------------------------
-- W17-B: INFORMATION_SCHEMA must expose three different index parsers in one
-- ordered scalar. VECTOR is a real index type here, not a function-like token.
--------------------------------------------------------------------------------
SELECT GROUP_CONCAT(
         CONCAT(INDEX_NAME, ':', INDEX_TYPE, ':', COLUMN_NAME)
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

--------------------------------------------------------------------------------
-- Wave-17 self-verification.
--------------------------------------------------------------------------------
CALL sq_hard_assert(
  @sq_hard_w17_hybrid_shape =
    '1:text#1,vector#1/2:text#2,vector#2/3:vector#3'
  AND @sq_hard_w17_rank_total = 6,
  CONCAT('hybrid rank: ',
         COALESCE(@sq_hard_w17_hybrid_shape, 'NULL')));
CALL sq_hard_assert(
  JSON_VALUE(@sq_hard_w17_hybrid_json, '$."1".title') = 'parser'
  AND JSON_VALUE(@sq_hard_w17_hybrid_json, '$."3".position') = 3
  AND JSON_LENGTH(@sq_hard_w17_hybrid_json) = 3,
  CONCAT('hybrid json: ',
         COALESCE(JSON_COMPACT(@sq_hard_w17_hybrid_json), 'NULL')));
CALL sq_hard_assert(
  @sq_hard_w17_index_shape =
    'ft_sq_hard_w17_document:FULLTEXT:title_text/'
    'ft_sq_hard_w17_document:FULLTEXT:body_text/'
    'PRIMARY:BTREE:document_id/'
    'vx_sq_hard_w17_document:VECTOR:embedding'
  AND (SELECT ROUND(
                VEC_DISTANCE(embedding, VEC_FromText('[1,0,0]')),
                6
              )
       FROM sq_hard_w17_document
       WHERE document_id = 1) = 0,
  CONCAT('hybrid indexes: ',
         COALESCE(@sq_hard_w17_index_shape, 'NULL')));
CALL sq_hard_assert(
  (SELECT COUNT(*) FROM sq_hard_w17_note) = 2,
  CONCAT('wave17 note rows: ',
         (SELECT COUNT(*) FROM sq_hard_w17_note)));

--------------------------------------------------------------------------------
-- ULTRA WAVE 18 -- dual-temporal vector/JSON scope-collision singularity.
--
-- One row simultaneously owns an application-time PERIOD, system-versioning,
-- a native VECTOR and a nested JSON document. FOR PORTION OF changes the
-- vector and JSON payload only inside one validity slice, forcing MariaDB to
-- create both application-time fragments and system-time history. A single CTE
-- pipeline then expands current and AS OF snapshots through JSON_TABLE, folds
-- the nested rows back into interval facts, and feeds four inherited windows.
-- The final scalar is deliberately alias-dense and order-sensitive so relation
-- scope, generated JSON_TABLE columns, temporal clause boundaries, VECTOR
-- typing, window inheritance and aggregate formatting must all stay coherent.
--------------------------------------------------------------------------------
CREATE TABLE sq_hard_w18_asset (
  asset_id    INT NOT NULL,
  valid_from  DATE NOT NULL,
  valid_to    DATE NOT NULL,
  embedding   VECTOR(3) NOT NULL,
  payload     JSON NOT NULL CHECK (JSON_VALID(payload)),
  row_start   TIMESTAMP(6) GENERATED ALWAYS AS ROW START,
  row_end     TIMESTAMP(6) GENERATED ALWAYS AS ROW END,
  PERIOD FOR validity (valid_from, valid_to),
  PERIOD FOR SYSTEM_TIME (row_start, row_end),
  PRIMARY KEY (asset_id, validity WITHOUT OVERLAPS)
) ENGINE = InnoDB WITH SYSTEM VERSIONING;

CREATE TABLE sq_hard_w18_note (
  note_key   VARCHAR(30) NOT NULL PRIMARY KEY,
  note_text  VARCHAR(1800) NOT NULL,
  note_value DECIMAL(20, 4) NOT NULL
) ENGINE = InnoDB;

INSERT INTO sq_hard_w18_asset (
  asset_id,
  valid_from,
  valid_to,
  embedding,
  payload
) VALUES
  (
    1,
    '2026-01-01',
    '2027-01-01',
    VEC_FromText('[1,0,0]'),
    JSON_OBJECT(
      'kind', 'alpha',
      'tags', JSON_ARRAY(
        JSON_OBJECT('name', 'sql', 'weight', 2),
        JSON_OBJECT('name', 'temporal', 'weight', 3)
      )
    )
  ),
  (
    2,
    '2026-01-01',
    '2027-01-01',
    VEC_FromText('[0,0,1]'),
    JSON_OBJECT(
      'kind', 'beta',
      'tags', JSON_ARRAY(
        JSON_OBJECT('name', 'vector', 'weight', 5)
      )
    )
  );

SET @sq_hard_w18_cutover = NOW(6);
DO SLEEP(0.02);

UPDATE sq_hard_w18_asset
   FOR PORTION OF validity FROM '2026-04-01' TO '2026-07-01'
   SET embedding = VEC_FromText('[0,1,0]'),
       payload = JSON_OBJECT(
         'kind', 'alpha-hot',
         'tags', JSON_ARRAY(
           JSON_OBJECT('name', 'period', 'weight', 7),
           JSON_OBJECT('name', 'window', 'weight', 11)
         )
       )
 WHERE asset_id = 1;

WITH
  current_expanded AS (
    SELECT 'current' AS scope_name,
           a.asset_id,
           a.valid_from,
           a.valid_to,
           a.embedding,
           JSON_VALUE(a.payload, '$.kind') AS asset_kind,
           j.tag_no,
           j.tag_name,
           j.tag_weight
    FROM sq_hard_w18_asset AS a
    JOIN JSON_TABLE(
           a.payload,
           '$.tags[*]' COLUMNS (
             tag_no     FOR ORDINALITY,
             tag_name   VARCHAR(30) PATH '$.name' ERROR ON EMPTY,
             tag_weight INT PATH '$.weight' ERROR ON ERROR
           )
         ) AS j ON TRUE
  ),
  snapshot_expanded AS (
    SELECT 'snapshot' AS scope_name,
           a.asset_id,
           a.valid_from,
           a.valid_to,
           a.embedding,
           JSON_VALUE(a.payload, '$.kind') AS asset_kind,
           j.tag_no,
           j.tag_name,
           j.tag_weight
    FROM sq_hard_w18_asset
         FOR SYSTEM_TIME AS OF @sq_hard_w18_cutover AS a
    JOIN JSON_TABLE(
           a.payload,
           '$.tags[*]' COLUMNS (
             tag_no     FOR ORDINALITY,
             tag_name   VARCHAR(30) PATH '$.name' ERROR ON EMPTY,
             tag_weight INT PATH '$.weight' ERROR ON ERROR
           )
         ) AS j ON TRUE
  ),
  scope_union AS (
    SELECT * FROM current_expanded
    UNION ALL
    SELECT * FROM snapshot_expanded
  ),
  interval_tags AS (
    SELECT scope_name,
           asset_id,
           valid_from,
           valid_to,
           embedding,
           asset_kind,
           GROUP_CONCAT(
             CONCAT(tag_name, '#', tag_weight)
             ORDER BY tag_no
             SEPARATOR ','
           ) AS tag_shape,
           SUM(tag_weight) AS tag_total
    FROM scope_union
    GROUP BY scope_name,
             asset_id,
             valid_from,
             valid_to,
             embedding,
             asset_kind
  ),
  interval_windows AS (
    SELECT scope_name,
           asset_id,
           valid_from,
           valid_to,
           asset_kind,
           tag_shape,
           tag_total,
           ROW_NUMBER() OVER w_interval AS interval_no,
           SUM(tag_total) OVER w_running AS running_weight,
           DENSE_RANK() OVER w_distance AS distance_rank,
           ROUND(
             VEC_DISTANCE_EUCLIDEAN(
               embedding,
               VEC_FromText('[1,0,0]')
             ),
             6
           ) AS vector_gap
    FROM interval_tags
    WINDOW
      w_asset AS (
        PARTITION BY scope_name, asset_id
      ),
      w_interval AS (
        w_asset
        ORDER BY valid_from, valid_to
      ),
      w_running AS (
        w_interval
        ROWS BETWEEN UNBOUNDED PRECEDING AND CURRENT ROW
      ),
      w_distance AS (
        PARTITION BY scope_name
        ORDER BY
          VEC_DISTANCE_EUCLIDEAN(
            embedding,
            VEC_FromText('[1,0,0]')
          ),
          asset_id,
          valid_from
      )
  )
SELECT GROUP_CONCAT(
         CONCAT(
           scope_name, '>', asset_id, '@',
           DATE_FORMAT(valid_from, '%Y-%m-%d'), '..',
           DATE_FORMAT(valid_to, '%Y-%m-%d'), ':',
           asset_kind, ':', tag_shape, ':',
           running_weight, ':', interval_no, ':',
           distance_rank
         )
         ORDER BY scope_name, asset_id, valid_from
         SEPARATOR '/'
       ),
       SUM(
         CASE scope_name
           WHEN 'current' THEN running_weight
           ELSE 0
         END
       ),
       COUNT(*)
INTO @sq_hard_w18_scope_shape,
     @sq_hard_w18_running_total,
     @sq_hard_w18_interval_rows
FROM interval_windows;

INSERT INTO sq_hard_w18_note (note_key, note_text, note_value)
VALUES (
  'dual-temporal-scope',
  @sq_hard_w18_scope_shape,
  @sq_hard_w18_running_total
);

--------------------------------------------------------------------------------
-- Wave-18 self-verification.
--------------------------------------------------------------------------------
CALL sq_hard_assert(
  @sq_hard_w18_scope_shape =
    'current>1@2026-01-01..2026-04-01:alpha:sql#2,temporal#3:5:1:1/'
    'current>1@2026-04-01..2026-07-01:alpha-hot:period#7,window#11:23:2:3/'
    'current>1@2026-07-01..2027-01-01:alpha:sql#2,temporal#3:28:3:2/'
    'current>2@2026-01-01..2027-01-01:beta:vector#5:5:1:4/'
    'snapshot>1@2026-01-01..2027-01-01:alpha:sql#2,temporal#3:5:1:1/'
    'snapshot>2@2026-01-01..2027-01-01:beta:vector#5:5:1:2',
  CONCAT(
    'dual temporal scope length: ',
    COALESCE(CHAR_LENGTH(@sq_hard_w18_scope_shape), -1)
  ));
CALL sq_hard_assert(
  @sq_hard_w18_running_total = 61
  AND @sq_hard_w18_interval_rows = 6
  AND (SELECT COUNT(*) FROM sq_hard_w18_asset) = 4
  AND (SELECT COUNT(*)
       FROM sq_hard_w18_asset
            FOR SYSTEM_TIME AS OF @sq_hard_w18_cutover) = 2,
  CONCAT(
    'dual temporal totals: ',
    COALESCE(@sq_hard_w18_running_total, -1),
    '/', COALESCE(@sq_hard_w18_interval_rows, -1)
  ));
CALL sq_hard_assert(
  (SELECT COUNT(*) FROM sq_hard_w18_note) = 1
  AND (SELECT note_value
       FROM sq_hard_w18_note
       WHERE note_key = 'dual-temporal-scope') = 61,
  CONCAT('wave18 note rows: ',
         (SELECT COUNT(*) FROM sq_hard_w18_note)));

-- ------------------------------------------------------------------------------
-- ULTRA WAVE 19 -- reverse-visibility JSON DML and metadata-namespace endgame.
--
-- MariaDB permits a leading-dot default-database qualifier, ODBC date/time
-- literals, and duplicate constraint names on different tables. A system-
-- versioned JSON table is then updated through a grouped JSON_TABLE derived
-- owner and deleted through a directly correlated JSON_TABLE in multi-table
-- DML. The history therefore preserves both pre-update and post-update images
-- while INFORMATION_SCHEMA must resolve two same-named constraints by table.
-- ------------------------------------------------------------------------------
CREATE TABLE sq_hard_w19_left_rule (
  rule_id INT NOT NULL PRIMARY KEY,
  amount  INT NOT NULL,
  CONSTRAINT sq_hard_w19_positive CHECK (amount > 0)
) ENGINE = InnoDB;

CREATE TABLE sq_hard_w19_right_rule (
  rule_id INT NOT NULL PRIMARY KEY,
  amount  INT NOT NULL,
  CONSTRAINT sq_hard_w19_positive CHECK (amount > 0)
) ENGINE = InnoDB;

CREATE TABLE sq_hard_w19_document (
  document_id    INT NOT NULL PRIMARY KEY,
  observed_at    DATETIME(6) NOT NULL,
  payload        JSON NOT NULL,
  computed_total INT NOT NULL DEFAULT 0,
  item_shape     VARCHAR(100) NOT NULL DEFAULT ''
) WITH SYSTEM VERSIONING;

CREATE TABLE sq_hard_w19_note (
  note_key   VARCHAR(30) NOT NULL PRIMARY KEY,
  note_text  VARCHAR(700) NOT NULL,
  note_value BIGINT NOT NULL
) ENGINE = InnoDB;

INSERT INTO . sq_hard_w19_left_rule (rule_id, amount)
VALUES (1, 7);

INSERT INTO . sq_hard_w19_right_rule (rule_id, amount)
VALUES (1, 11);

-- ------------------------------------------------------------------------------
-- W19-A: `. table` is MariaDB's ODBC-compatible spelling for the current
-- database. The braces are SQL tokens, not JSON objects or compound blocks.
-- ------------------------------------------------------------------------------
INSERT INTO . sq_hard_w19_document (
  document_id,
  observed_at,
  payload
)
VALUES
  (
    1,
    {ts '2026-01-01 00:00:00.000001'},
    JSON_OBJECT(
      'items',
      JSON_ARRAY(
        JSON_OBJECT('kind', 'alpha', 'value', 3),
        JSON_OBJECT('kind', 'beta',  'value', 5)
      )
    )
  ),
  (
    2,
    {ts '2026-01-02 00:00:00.000002'},
    JSON_OBJECT(
      'items',
      JSON_ARRAY(
        JSON_OBJECT('kind', 'gamma', 'value', 7),
        JSON_OBJECT('kind', 'delta', 'value', 11)
      )
    )
  ),
  (
    3,
    {ts '2026-01-03 00:00:00.000003'},
    JSON_OBJECT(
      'items',
      JSON_ARRAY(
        JSON_OBJECT('kind', 'delete', 'value', 13)
      )
    )
  );

DO SLEEP(0.01);

-- ------------------------------------------------------------------------------
-- W19-B: the first JSON_TABLE is correlated inside a grouped derived table,
-- whose minted columns then drive a multi-table UPDATE. JSON_SET writes both
-- relational aggregates back into the source document in the same assignment
-- list that updates ordinary columns.
-- ------------------------------------------------------------------------------
UPDATE . sq_hard_w19_document AS document_target
JOIN (
  SELECT document_owner.document_id,
         SUM(item_row.item_value) AS item_total,
         GROUP_CONCAT(
           item_row.item_kind
           ORDER BY item_row.item_no
           SEPARATOR ','
         ) AS item_shape
  FROM . sq_hard_w19_document AS document_owner
  JOIN JSON_TABLE(
         document_owner.payload,
         '$.items[*]' COLUMNS (
           item_no    FOR ORDINALITY,
           item_kind  VARCHAR(20) PATH '$.kind' ERROR ON EMPTY,
           item_value INT PATH '$.value' ERROR ON ERROR
         )
       ) AS item_row ON TRUE
  GROUP BY document_owner.document_id
) AS rolled
  ON rolled.document_id = document_target.document_id
SET document_target.computed_total = rolled.item_total,
    document_target.item_shape = rolled.item_shape,
    document_target.payload = JSON_SET(
      document_target.payload,
      '$.summary',
      JSON_OBJECT(
        'total',
        rolled.item_total,
        'shape',
        rolled.item_shape
      )
    );

DO SLEEP(0.01);

-- ------------------------------------------------------------------------------
-- W19-C: JSON_TABLE is now a direct member of the DELETE join list and is
-- implicitly lateral to the target immediately before it. The target alias
-- before FROM belongs to multi-table DELETE, not to a projection list.
-- ------------------------------------------------------------------------------
DELETE doomed
FROM . sq_hard_w19_document AS doomed
JOIN JSON_TABLE(
       doomed.payload,
       '$.items[*]' COLUMNS (
         item_kind VARCHAR(20) PATH '$.kind'
       )
     ) AS item_row ON TRUE
WHERE item_row.item_kind = 'delete';

SELECT GROUP_CONCAT(
         CONCAT(
           document_id, ':',
           computed_total, ':',
           item_shape
         )
         ORDER BY document_id
         SEPARATOR '/'
       ),
       SUM(computed_total)
INTO @sq_hard_w19_document_shape,
     @sq_hard_w19_document_total
FROM . sq_hard_w19_document;

SELECT COUNT(*)
INTO @sq_hard_w19_constraint_count
FROM information_schema.TABLE_CONSTRAINTS
WHERE CONSTRAINT_SCHEMA = DATABASE()
  AND CONSTRAINT_NAME = 'sq_hard_w19_positive';

SELECT CONCAT(
         DATE_FORMAT({d '2026-01-01'}, '%Y-%m-%d'),
         'T',
         TIME_FORMAT({t '12:34:56'}, '%H:%i:%s'),
         '.',
         DATE_FORMAT(
           {ts '2026-01-01 12:34:56.123456'},
           '%f'
         )
       )
INTO @sq_hard_w19_odbc_shape;

INSERT INTO sq_hard_w19_note (note_key, note_text, note_value)
VALUES
  (
    'json-multi-dml',
    @sq_hard_w19_document_shape,
    @sq_hard_w19_document_total
  ),
  (
    'constraint-odbc',
    @sq_hard_w19_odbc_shape,
    @sq_hard_w19_constraint_count
  );

-- ------------------------------------------------------------------------------
-- Wave-19 self-verification.
-- ------------------------------------------------------------------------------
CALL sq_hard_assert(
  @sq_hard_w19_document_shape =
    '1:8:alpha,beta/2:18:gamma,delta'
  AND @sq_hard_w19_document_total = 26,
  CONCAT(
    'json multi-table dml: ',
    COALESCE(@sq_hard_w19_document_shape, 'NULL'),
    '/',
    COALESCE(@sq_hard_w19_document_total, -1)
  ));
CALL sq_hard_assert(
  @sq_hard_w19_constraint_count = 2
  AND @sq_hard_w19_odbc_shape =
    '2026-01-01T12:34:56.123456',
  CONCAT(
    'constraint/odbc: ',
    COALESCE(@sq_hard_w19_constraint_count, -1),
    '/',
    COALESCE(@sq_hard_w19_odbc_shape, 'NULL')
  ));
CALL sq_hard_assert(
  (SELECT COUNT(*) FROM . sq_hard_w19_document) = 2
  AND (SELECT COUNT(*)
       FROM . sq_hard_w19_document FOR SYSTEM_TIME ALL) = 6
  AND JSON_VALUE(
        (SELECT payload
         FROM . sq_hard_w19_document
         WHERE document_id = 2),
        '$.summary.total'
      ) = 18,
  'system-versioned JSON DML history');
CALL sq_hard_assert(
  (SELECT COUNT(*) FROM sq_hard_w19_note) = 2,
  CONCAT('wave19 note rows: ',
         (SELECT COUNT(*) FROM sq_hard_w19_note)));

-- ------------------------------------------------------------------------------
-- ULTRA WAVE 20 -- cycle-restricted graph, JSON window, and multiset singularity.
--
-- MariaDB's relaxed CYCLE grammar deliberately differs from Oracle's standard
-- cycle-mark/path form and from MySQL's manual JSON path guard. The same nodes
-- then cross JSON_TABLE, ordered-set window aggregation, inherited windows,
-- INTERSECT ALL / EXCEPT ALL, and DML RETURNING result sets. Assertions prove
-- that recursion, duplicate multiplicity, JSON expansion, and returned DML all
-- changed the intended rows.
-- ------------------------------------------------------------------------------
CREATE TABLE sq_hard_w20_node (
  node_id INT NOT NULL PRIMARY KEY,
  payload JSON NOT NULL,
  CONSTRAINT ck_sq_hard_w20_payload_object
    CHECK (JSON_TYPE(payload) = 'OBJECT')
) ENGINE = InnoDB;

CREATE TABLE sq_hard_w20_edge (
  from_id INT NOT NULL,
  to_id   INT NOT NULL,
  PRIMARY KEY (from_id, to_id),
  CONSTRAINT fk_sq_hard_w20_edge_from
    FOREIGN KEY (from_id) REFERENCES sq_hard_w20_node (node_id),
  CONSTRAINT fk_sq_hard_w20_edge_to
    FOREIGN KEY (to_id) REFERENCES sq_hard_w20_node (node_id)
) ENGINE = InnoDB;

CREATE TABLE sq_hard_w20_bag (
  bag_id   INT NOT NULL PRIMARY KEY,
  bag_side ENUM('L', 'R', 'X') NOT NULL,
  token    INT NOT NULL,
  KEY ix_sq_hard_w20_bag_side_token (bag_side, token)
) ENGINE = InnoDB;

CREATE TABLE sq_hard_w20_returning (
  return_id INT NOT NULL PRIMARY KEY,
  payload   JSON NOT NULL
) ENGINE = InnoDB;

CREATE TABLE sq_hard_w20_note (
  note_key   VARCHAR(30) NOT NULL PRIMARY KEY,
  note_text  VARCHAR(700) NOT NULL,
  note_value BIGINT NOT NULL
) ENGINE = InnoDB;

INSERT INTO sq_hard_w20_node (node_id, payload)
VALUES
  (
    1,
    JSON_OBJECT(
      'label', 'root',
      'items', JSON_ARRAY(
        JSON_OBJECT('kind', 'alpha', 'value', 2, 'flag', TRUE),
        JSON_OBJECT('kind', 'beta',  'value', 4)
      )
    )
  ),
  (
    2,
    JSON_OBJECT(
      'label', 'branch',
      'items', JSON_ARRAY(
        JSON_OBJECT('kind', 'alpha', 'value', 5),
        JSON_OBJECT('kind', 'beta',  'value', 9),
        JSON_OBJECT('kind', 'gamma', 'value', 13)
      )
    )
  ),
  (
    3,
    JSON_OBJECT(
      'label', 'cycle',
      'items', JSON_ARRAY(
        JSON_OBJECT('kind', 'gamma', 'value', 7)
      )
    )
  ),
  (
    4,
    JSON_OBJECT(
      'label', 'leaf',
      'items', JSON_ARRAY(
        JSON_OBJECT('kind', 'delta',   'value', 6),
        JSON_OBJECT('kind', 'epsilon', 'value', 10)
      )
    )
  ),
  (
    5,
    JSON_OBJECT(
      'label', 'end',
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

INSERT INTO sq_hard_w20_bag (bag_id, bag_side, token)
VALUES
  ( 1, 'L', 1),
  ( 2, 'L', 1),
  ( 3, 'L', 2),
  ( 4, 'L', 2),
  ( 5, 'L', 3),
  ( 6, 'R', 1),
  ( 7, 'R', 1),
  ( 8, 'R', 1),
  ( 9, 'R', 2),
  (10, 'R', 3),
  (11, 'R', 3),
  (12, 'X', 1),
  (13, 'X', 3);

-- ------------------------------------------------------------------------------
-- W20-A: CYCLE NODE_ID RESTRICT suppresses the 3 -> 1 back edge without adding
-- user-visible cycle or path columns. UNION ALL therefore remains a multiset
-- operator everywhere except the cycle key owned by the CTE.
-- ------------------------------------------------------------------------------
WITH RECURSIVE
  walk (
    origin_id,
    node_id,
    depth_no,
    path_text
  ) AS (
    SELECT 1,
           1,
           0,
           CAST('1' AS CHAR(200))
    UNION ALL
    SELECT walk.origin_id,
           edge_owner.to_id,
           walk.depth_no + 1,
           CONCAT(walk.path_text, '>', edge_owner.to_id)
    FROM walk
    JOIN sq_hard_w20_edge AS edge_owner
      ON edge_owner.from_id = walk.node_id
  )
  CYCLE node_id RESTRICT
SELECT GROUP_CONCAT(
         CONCAT(node_id, ':', depth_no)
         ORDER BY depth_no, node_id
         SEPARATOR '/'
       ),
       SUM(depth_no)
INTO @sq_hard_w20_walk_shape,
     @sq_hard_w20_walk_depth
FROM walk;

INSERT INTO sq_hard_w20_note (note_key, note_text, note_value)
VALUES (
  'cycle-restrict',
  @sq_hard_w20_walk_shape,
  @sq_hard_w20_walk_depth
);

-- ------------------------------------------------------------------------------
-- W20-B: JSON_TABLE owns an EXISTS column and an ordinal/value namespace.
-- PERCENTILE_CONT is an ordered-set aggregate used as a window function while
-- W_RUNNING and W_REVERSE inherit the same partition in opposite directions.
-- ------------------------------------------------------------------------------
WITH
  expanded AS (
    SELECT node_owner.node_id,
           item_owner.item_no,
           item_owner.item_kind,
           item_owner.item_value,
           item_owner.has_flag,
           PERCENTILE_CONT(0.5)
             WITHIN GROUP (ORDER BY item_owner.item_value)
             OVER (PARTITION BY node_owner.node_id) AS median_value,
           SUM(item_owner.item_value)
             OVER w_running AS running_value,
           ROW_NUMBER()
             OVER w_reverse AS reverse_no
    FROM sq_hard_w20_node AS node_owner
    JOIN JSON_TABLE(
           node_owner.payload,
           '$.items[*]' COLUMNS (
             item_no    FOR ORDINALITY,
             item_kind  VARCHAR(20) PATH '$.kind' ERROR ON EMPTY,
             item_value INT         PATH '$.value' ERROR ON ERROR,
             has_flag   INT         EXISTS PATH '$.flag'
           )
         ) AS item_owner ON TRUE
    WINDOW
      w_node AS (
        PARTITION BY node_owner.node_id
      ),
      w_running AS (
        w_node
        ORDER BY item_owner.item_no
        ROWS BETWEEN UNBOUNDED PRECEDING AND CURRENT ROW
      ),
      w_reverse AS (
        w_node
        ORDER BY item_owner.item_no DESC
      )
  ),
  final_items AS (
    SELECT node_id,
           median_value,
           running_value
    FROM expanded
    WHERE reverse_no = 1
  )
SELECT GROUP_CONCAT(
         CONCAT(
           node_id, ':',
           ROUND(median_value, 0), ':',
           running_value
         )
         ORDER BY node_id
         SEPARATOR '/'
       ),
       SUM(running_value),
       (SELECT SUM(has_flag) FROM expanded)
INTO @sq_hard_w20_json_shape,
     @sq_hard_w20_json_total,
     @sq_hard_w20_flag_total
FROM final_items;

INSERT INTO sq_hard_w20_note (note_key, note_text, note_value)
VALUES (
  'json-window-median',
  @sq_hard_w20_json_shape,
  @sq_hard_w20_json_total
);

-- ------------------------------------------------------------------------------
-- W20-C: duplicate multiplicity survives INTERSECT ALL, then EXCEPT ALL removes
-- one copy of 1 and the only copy of 3. Parenthesized query-expression owners
-- must not leak into the surrounding GROUP_CONCAT.
-- ------------------------------------------------------------------------------
SELECT GROUP_CONCAT(
         multiset_owner.token
         ORDER BY multiset_owner.token
         SEPARATOR ','
       )
INTO @sq_hard_w20_bag_shape
FROM (
  (
    (SELECT token
     FROM sq_hard_w20_bag
     WHERE bag_side = 'L')
    INTERSECT ALL
    (SELECT token
     FROM sq_hard_w20_bag
     WHERE bag_side = 'R')
  )
  EXCEPT ALL
  (SELECT token
   FROM sq_hard_w20_bag
   WHERE bag_side = 'X')
) AS multiset_owner;

INSERT INTO sq_hard_w20_note (note_key, note_text, note_value)
VALUES (
  'multiset-bag',
  @sq_hard_w20_bag_shape,
  2
)
RETURNING note_key,
          note_text,
          note_value;

-- ------------------------------------------------------------------------------
-- W20-D: INSERT RETURNING and DELETE RETURNING are result-producing DML, not
-- SELECT clauses. The second statement proves that its returned row was deleted.
-- ------------------------------------------------------------------------------
INSERT INTO sq_hard_w20_returning (return_id, payload)
VALUES
  (1, JSON_OBJECT('state', 'keep',   'rank', 1)),
  (2, JSON_OBJECT('state', 'delete', 'rank', 2))
RETURNING return_id,
          JSON_VALUE(payload, '$.state') AS returned_state,
          JSON_VALUE(payload, '$.rank')  AS returned_rank;

DELETE FROM sq_hard_w20_returning
WHERE return_id = 2
RETURNING return_id,
          JSON_VALUE(payload, '$.state') AS deleted_state;

-- ------------------------------------------------------------------------------
-- Wave-20 self-verification.
-- ------------------------------------------------------------------------------
CALL sq_hard_assert(
  @sq_hard_w20_walk_shape = '1:0/2:1/3:2/4:2/5:3'
  AND @sq_hard_w20_walk_depth = 8,
  CONCAT(
    'cycle restrict: ',
    COALESCE(@sq_hard_w20_walk_shape, 'NULL'),
    '/',
    COALESCE(@sq_hard_w20_walk_depth, -1)
  ));
CALL sq_hard_assert(
  @sq_hard_w20_json_shape =
    '1:3:6/2:9:27/3:7:7/4:8:16/5:12:12'
  AND @sq_hard_w20_json_total = 68
  AND @sq_hard_w20_flag_total = 1,
  CONCAT(
    'json window median: ',
    COALESCE(@sq_hard_w20_json_shape, 'NULL'),
    '/',
    COALESCE(@sq_hard_w20_json_total, -1),
    '/',
    COALESCE(@sq_hard_w20_flag_total, -1)
  ));
CALL sq_hard_assert(
  @sq_hard_w20_bag_shape = '1,2',
  CONCAT(
    'multiset bag: ',
    COALESCE(@sq_hard_w20_bag_shape, 'NULL')
  ));
CALL sq_hard_assert(
  (SELECT COUNT(*) FROM sq_hard_w20_returning) = 1
  AND JSON_VALUE(
        (SELECT payload
         FROM sq_hard_w20_returning
         WHERE return_id = 1),
        '$.state'
      ) = 'keep'
  AND (SELECT COUNT(*) FROM sq_hard_w20_note) = 3,
  'returning rows or wave20 notes');

-- ------------------------------------------------------------------------------
-- ULTRA WAVE 21: a JSON_TABLE on the textual left of RIGHT JOIN correlates to
-- the preserved table on its textual right. A VALUES-backed view contributes
-- literal-derived quoted column names, while nested dynamic-column functions
-- rewrite a binary document. These are MariaDB-specific scope and lexer traps.
-- ------------------------------------------------------------------------------
CREATE TABLE sq_hard_w21_document (
  document_id INT NOT NULL PRIMARY KEY,
  payload     JSON NOT NULL
) ENGINE = InnoDB;

CREATE TABLE sq_hard_w21_dynamic (
  state_id     INT NOT NULL PRIMARY KEY,
  dynamic_blob MEDIUMBLOB NOT NULL
) ENGINE = InnoDB;

CREATE TABLE sq_hard_w21_note (
  note_key   VARCHAR(40) NOT NULL PRIMARY KEY,
  note_text  VARCHAR(1000) NOT NULL,
  note_value BIGINT NOT NULL
) ENGINE = InnoDB;

INSERT INTO sq_hard_w21_document (document_id, payload)
VALUES
  (
    1,
    JSON_OBJECT(
      'items',
      JSON_ARRAY(
        JSON_OBJECT('name', 'a', 'value', 2),
        JSON_OBJECT('name', 'b', 'value', 3)
      )
    )
  ),
  (
    2,
    JSON_OBJECT(
      'items',
      JSON_ARRAY(
        JSON_OBJECT('name', 'b', 'value', 5),
        JSON_OBJECT('name', 'c', 'value', 7)
      )
    )
  );

-- VALUES derives the view column names from its first row expressions. The
-- resulting identifiers are literally `a` and `10`.
CREATE VIEW sq_hard_w21_weight AS
VALUES ('a', 10), ('b', 20), ('c', 30), ('d', 40);

-- ------------------------------------------------------------------------------
-- W21-A: for RIGHT JOIN, MariaDB treats DOC_OWNER (the preserved/right side) as
-- preceding ITEM_OWNER, so JSON_TABLE may legally reference DOC_OWNER before
-- its declaration appears in the text. The VALUES view's numeric identifier
-- then participates in ordinary arithmetic.
-- ------------------------------------------------------------------------------
SELECT GROUP_CONCAT(
         CONCAT(
           document_id, ':', item_name, ':', weighted_value
         )
         ORDER BY document_id, item_no
         SEPARATOR '/'
       )
INTO @sq_hard_w21_right_shape
FROM (
  SELECT doc_owner.document_id,
         item_owner.item_no,
         item_owner.item_name,
         item_owner.item_value * weight_owner.`10` AS weighted_value
  FROM JSON_TABLE(
         doc_owner.payload,
         '$.items[*]'
         COLUMNS (
           item_no FOR ORDINALITY,
           item_name VARCHAR(20) PATH '$.name',
           item_value INT PATH '$.value'
         )
       ) AS item_owner
  RIGHT JOIN sq_hard_w21_document AS doc_owner
    ON item_owner.item_no IS NOT NULL
  JOIN sq_hard_w21_weight AS weight_owner
    ON weight_owner.`a` = item_owner.item_name
) AS rightward_owner;

INSERT INTO sq_hard_w21_note (note_key, note_text, note_value)
VALUES (
  'rightward-json-table',
  @sq_hard_w21_right_shape,
  (
    SELECT SUM(item_owner.item_value * weight_owner.`10`)
    FROM JSON_TABLE(
           doc_owner.payload,
           '$.items[*]'
           COLUMNS (
             item_name VARCHAR(20) PATH '$.name',
             item_value INT PATH '$.value'
           )
         ) AS item_owner
    RIGHT JOIN sq_hard_w21_document AS doc_owner
      ON item_owner.item_name IS NOT NULL
    JOIN sq_hard_w21_weight AS weight_owner
      ON weight_owner.`a` = item_owner.item_name
  )
);

-- ------------------------------------------------------------------------------
-- W21-B: COLUMN_GET supplies an operand to COLUMN_ADD, COLUMN_DELETE removes a
-- sibling key from that newly built binary value, and the stored result is read
-- through CHECK/GET/EXISTS/LIST/JSON interfaces with different return grammars.
-- ------------------------------------------------------------------------------
INSERT INTO sq_hard_w21_dynamic (state_id, dynamic_blob)
VALUES (
  1,
  COLUMN_CREATE(
    'name', 'parser' AS CHAR,
    'score', 7,
    'obsolete', 1
  )
);

UPDATE sq_hard_w21_dynamic
SET dynamic_blob = COLUMN_DELETE(
  COLUMN_ADD(
    dynamic_blob,
    'score', COLUMN_GET(dynamic_blob, 'score' AS INT) + 5,
    'active', 'yes' AS CHAR
  ),
  'obsolete'
)
WHERE state_id = 1;

SELECT CONCAT(
         COLUMN_GET(dynamic_blob, 'name' AS CHAR), ':',
         COLUMN_GET(dynamic_blob, 'score' AS INT), ':',
         COLUMN_GET(dynamic_blob, 'active' AS CHAR), ':',
         COLUMN_EXISTS(dynamic_blob, 'obsolete')
       ),
       COLUMN_JSON(dynamic_blob),
       COLUMN_LIST(dynamic_blob)
INTO @sq_hard_w21_dynamic_shape,
     @sq_hard_w21_dynamic_json,
     @sq_hard_w21_dynamic_list
FROM sq_hard_w21_dynamic
WHERE COLUMN_CHECK(dynamic_blob) = 1;

INSERT INTO sq_hard_w21_note (note_key, note_text, note_value)
VALUES (
  'dynamic-rewrite',
  @sq_hard_w21_dynamic_shape,
  COLUMN_GET(
    (SELECT dynamic_blob
     FROM sq_hard_w21_dynamic
     WHERE state_id = 1),
    'score' AS INT
  )
);

-- ------------------------------------------------------------------------------
-- Wave-21 self-verification.
-- ------------------------------------------------------------------------------
CALL sq_hard_assert(
  @sq_hard_w21_right_shape = '1:a:20/1:b:60/2:b:100/2:c:210'
  AND (SELECT note_value
       FROM sq_hard_w21_note
       WHERE note_key = 'rightward-json-table') = 390,
  CONCAT(
    'rightward JSON_TABLE: ',
    COALESCE(@sq_hard_w21_right_shape, 'NULL')
  ));
CALL sq_hard_assert(
  @sq_hard_w21_dynamic_shape = 'parser:12:yes:0'
  AND JSON_VALUE(@sq_hard_w21_dynamic_json, '$.score') = 12
  AND JSON_VALUE(@sq_hard_w21_dynamic_json, '$.active') = 'yes'
  AND BINARY @sq_hard_w21_dynamic_list =
      BINARY '`name`,`score`,`active`'
  AND (SELECT COUNT(*) FROM sq_hard_w21_note) = 2,
  CONCAT(
    'dynamic rewrite: ',
    COALESCE(@sq_hard_w21_dynamic_shape, 'NULL'),
    '/',
    COALESCE(@sq_hard_w21_dynamic_json, 'NULL')
  ));

-- ------------------------------------------------------------------------------
-- ULTRA WAVE 22: the SEQUENCE storage engine creates relations from identifier
-- spelling alone. The same name is then rebound to a physical TEMPORARY table
-- and restored to a virtual sequence after DROP. Before that scope switch, two
-- independently aliased sequence relations feed a correlated JSON aggregate
-- into INSERT ... SELECT ... ON DUPLICATE KEY UPDATE ... RETURNING *. Persistent
-- statistics and the complete BACKUP STAGE state machine close the wave.
-- ------------------------------------------------------------------------------
CREATE TABLE sq_hard_w22_target (
  item_id INT NOT NULL PRIMARY KEY,
  hits    INT NOT NULL DEFAULT 1,
  parity  VARCHAR(8) NOT NULL,
  payload JSON NOT NULL CHECK (JSON_VALID(payload)),
  KEY ix_sq_hard_w22_parity_hits (parity, hits)
) ENGINE = InnoDB;

CREATE TABLE sq_hard_w22_note (
  note_key   VARCHAR(40) NOT NULL PRIMARY KEY,
  note_text  VARCHAR(1000) NOT NULL,
  note_value BIGINT NOT NULL
) ENGINE = InnoDB;

INSERT INTO sq_hard_w22_target (item_id, hits, parity, payload)
VALUES
  (
    1,
    1,
    'odd',
    JSON_OBJECT(
      'n', 1,
      'square', 0,
      'divisors', JSON_ARRAY()
    )
  ),
  (
    2,
    1,
    'even',
    JSON_OBJECT(
      'n', 2,
      'square', 0,
      'divisors', JSON_ARRAY()
    )
  );

-- ------------------------------------------------------------------------------
-- W22-A: SEQ_1_TO_5 is never declared, yet its spelling creates a virtual table
-- whose sole column is SEQ. The inner relation has the same physical spelling
-- but a different alias and correlation owner. VALUE() belongs to MariaDB's
-- upsert grammar, while RETURNING * exposes the post-upsert target row.
-- ------------------------------------------------------------------------------
INSERT INTO sq_hard_w22_target (item_id, hits, parity, payload)
SELECT sequence_owner.seq,
       1,
       IF(sequence_owner.seq MOD 2 = 0, 'even', 'odd'),
       JSON_OBJECT(
         'n', sequence_owner.seq,
         'square', sequence_owner.seq * sequence_owner.seq,
         'divisors', (
           SELECT JSON_ARRAYAGG(
                    divisor_owner.seq ORDER BY divisor_owner.seq
                  )
           FROM seq_1_to_5 AS divisor_owner
           WHERE sequence_owner.seq MOD divisor_owner.seq = 0
         )
       )
FROM seq_1_to_5 AS sequence_owner
ON DUPLICATE KEY UPDATE
  hits = sq_hard_w22_target.hits + VALUE(hits),
  parity = VALUE(parity),
  payload = VALUE(payload)
RETURNING *,
          JSON_VALUE(payload, '$.square') AS returned_square;

SELECT GROUP_CONCAT(
         CONCAT(
           item_id, ':', hits, ':', parity, ':',
           JSON_VALUE(payload, '$.square'), ':',
           JSON_COMPACT(JSON_QUERY(payload, '$.divisors'))
         )
         ORDER BY item_id
         SEPARATOR '/'
       ),
       SUM(hits)
INTO @sq_hard_w22_upsert_shape,
     @sq_hard_w22_hit_total
FROM sq_hard_w22_target;

INSERT INTO sq_hard_w22_note (note_key, note_text, note_value)
VALUES (
  'sequence-upsert',
  @sq_hard_w22_upsert_shape,
  @sq_hard_w22_hit_total
);

-- ------------------------------------------------------------------------------
-- W22-B: a TEMPORARY table legally shadows the automatically generated
-- SEQ_1_TO_5 relation. Completion must replace its one virtual SEQ column with
-- SOURCE_LABEL and SQUARED, then forget those columns as soon as DROP reveals
-- the virtual table again.
-- ------------------------------------------------------------------------------
SELECT GROUP_CONCAT(seq ORDER BY seq SEPARATOR '/')
INTO @sq_hard_w22_virtual_before
FROM seq_1_to_5;

CREATE TEMPORARY TABLE seq_1_to_5 (
  seq          INT NOT NULL PRIMARY KEY,
  source_label VARCHAR(20) NOT NULL,
  squared      INT AS (seq * seq) STORED
) ENGINE = InnoDB;

INSERT INTO seq_1_to_5 (seq, source_label)
VALUES (42, 'from'), (7, 'select');

SELECT GROUP_CONCAT(
         CONCAT(seq, ':', source_label, ':', squared)
         ORDER BY seq
         SEPARATOR '/'
       )
INTO @sq_hard_w22_shadow_shape
FROM seq_1_to_5;

DROP TEMPORARY TABLE seq_1_to_5;

SELECT GROUP_CONCAT(seq ORDER BY seq SEPARATOR '/')
INTO @sq_hard_w22_virtual_after
FROM seq_1_to_5;

INSERT INTO sq_hard_w22_note (note_key, note_text, note_value)
VALUES (
  'sequence-shadow',
  CONCAT(
    'virtual=', @sq_hard_w22_virtual_before,
    ';shadow=', @sq_hard_w22_shadow_shape,
    ';restored=', @sq_hard_w22_virtual_after
  ),
  (SELECT COUNT(*) FROM seq_1_to_5) + 2
);

-- ------------------------------------------------------------------------------
-- W22-C: PERSISTENT FOR splits column and index owner lists across clauses.
-- BACKUP STAGE walks every server-side phase in order; after END, a descending
-- stepped virtual sequence and an INFORMATION_SCHEMA engine row feed ordinary
-- DML, proving that the global backup state was fully released.
-- ------------------------------------------------------------------------------
ANALYZE TABLE sq_hard_w22_target PERSISTENT FOR
  COLUMNS (hits, parity)
  INDEXES (PRIMARY, ix_sq_hard_w22_parity_hits);

SELECT COUNT(*),
       GROUP_CONCAT(column_name ORDER BY column_name SEPARATOR '/')
INTO @sq_hard_w22_stats_rows,
     @sq_hard_w22_stats_shape
FROM mysql.column_stats
WHERE db_name = 'sq_hard_mariadb'
  AND table_name = 'sq_hard_w22_target'
  AND column_name IN ('hits', 'parity');

COMMIT;

BACKUP STAGE START;
BACKUP STAGE FLUSH;
BACKUP STAGE BLOCK_DDL;
BACKUP STAGE BLOCK_COMMIT;
BACKUP STAGE END;

SELECT GROUP_CONCAT(seq ORDER BY seq DESC SEPARATOR '/')
INTO @sq_hard_w22_reverse_shape
FROM seq_15_to_2_step_3;

INSERT INTO sq_hard_w22_note (note_key, note_text, note_value)
SELECT 'persistent-backup',
       CONCAT(
         'engine=', engine_owner.support,
         ',stats=', @sq_hard_w22_stats_shape,
         ',reverse=', @sq_hard_w22_reverse_shape
       ),
       @sq_hard_w22_stats_rows
FROM information_schema.engines AS engine_owner
WHERE engine_owner.engine = 'SEQUENCE';

COMMIT;

-- ------------------------------------------------------------------------------
-- Wave-22 self-verification.
-- ------------------------------------------------------------------------------
CALL sq_hard_assert(
  @sq_hard_w22_upsert_shape =
    '1:2:odd:1:[1]/2:2:even:4:[1,2]/3:1:odd:9:[1,3]/'
    '4:1:even:16:[1,2,4]/5:1:odd:25:[1,5]'
  AND @sq_hard_w22_hit_total = 7
  AND (SELECT COUNT(*) FROM sq_hard_w22_target) = 5,
  CONCAT(
    'sequence upsert: ',
    COALESCE(@sq_hard_w22_upsert_shape, 'NULL')
  ));
CALL sq_hard_assert(
  @sq_hard_w22_virtual_before = '1/2/3/4/5'
  AND @sq_hard_w22_shadow_shape = '7:select:49/42:from:1764'
  AND @sq_hard_w22_virtual_after = '1/2/3/4/5'
  AND (SELECT note_value
       FROM sq_hard_w22_note
       WHERE note_key = 'sequence-shadow') = 7,
  CONCAT(
    'sequence shadow: ',
    COALESCE(@sq_hard_w22_shadow_shape, 'NULL')
  ));
CALL sq_hard_assert(
  @sq_hard_w22_stats_rows = 2
  AND @sq_hard_w22_stats_shape = 'hits/parity'
  AND @sq_hard_w22_reverse_shape = '14/11/8/5/2'
  AND (SELECT note_text
       FROM sq_hard_w22_note
       WHERE note_key = 'persistent-backup') =
        'engine=YES,stats=hits/parity,reverse=14/11/8/5/2'
  AND (SELECT COUNT(*) FROM sq_hard_w22_note) = 3,
  CONCAT(
    'persistent stats/backup: ',
    COALESCE(@sq_hard_w22_stats_shape, 'NULL'),
    '/',
    COALESCE(@sq_hard_w22_reverse_shape, 'NULL')
  ));

-- ------------------------------------------------------------------------------
-- ULTRA WAVE 23 -- expression-named view, storage-engine and lock singularity.
--
-- A table-value constructor used as a view body derives its metadata from the
-- first VALUES row, producing a numeric column name, a literal-shaped name and
-- a complete JSON function call as a third name. A wrapper must quote those
-- inferred names before correlated JSON_TABLE rows meet every legal SELECT
-- result modifier. CSV's IETF quote mode then changes how physical rows are
-- encoded, while MyISAM key-cache and alias-sensitive lock grammars reuse
-- INDEX, KEY, READ, WRITE, LOCAL, CONCURRENT, WAIT and NOWAIT contextually.
-- ------------------------------------------------------------------------------
CREATE TABLE sq_hard_w23_note (
  note_key   VARCHAR(40) NOT NULL PRIMARY KEY,
  note_text  VARCHAR(1000) NOT NULL,
  note_value BIGINT NOT NULL
) ENGINE = InnoDB;

-- ------------------------------------------------------------------------------
-- W23-A: VALUES supplies both rows and metadata. The raw view therefore owns
-- the deliberately hostile columns `1`, `alpha`, and the normalized JSON_OBJECT
-- expression. ALTER VIEW changes its cardinality in place; the wrapper view
-- immediately turns those three expression names into conventional aliases.
-- CHECK/REPAIR then parse both dependency layers before the modifier battery
-- and an implicitly lateral JSON_TABLE execute with SQL_CALC_FOUND_ROWS.
-- ------------------------------------------------------------------------------
CREATE ALGORITHM = TEMPTABLE SQL SECURITY INVOKER
VIEW sq_hard_w23_values AS
VALUES
  (
    1,
    'alpha',
    JSON_OBJECT(
      'tags', JSON_ARRAY('sql', 'json'),
      'score', 10
    )
  ),
  (
    2,
    'beta',
    JSON_OBJECT(
      'tags', JSON_ARRAY('format'),
      'score', 20
    )
  ),
  (
    3,
    'gamma',
    JSON_OBJECT(
      'tags', JSON_ARRAY('sql', 'lexer'),
      'score', 30
    )
  );

SET @sq_hard_w23_before_rows = (
  SELECT COUNT(*)
  FROM sq_hard_w23_values
);

ALTER ALGORITHM = UNDEFINED SQL SECURITY DEFINER
VIEW sq_hard_w23_values AS
(
  VALUES
    (
      1,
      'alpha',
      JSON_OBJECT(
        'tags', JSON_ARRAY('sql', 'json'),
        'score', 10
      )
    ),
    (
      2,
      'beta',
      JSON_OBJECT(
        'tags', JSON_ARRAY('format'),
        'score', 20
      )
    ),
    (
      3,
      'gamma',
      JSON_OBJECT(
        'tags', JSON_ARRAY('sql', 'lexer'),
        'score', 30
      )
    ),
    (
      4,
      'delta',
      JSON_OBJECT(
        'tags', JSON_ARRAY(),
        'score', 40
      )
    )
);

CREATE ALGORITHM = MERGE SQL SECURITY INVOKER
VIEW sq_hard_w23_value_v (item_id, item_label, payload) AS
SELECT raw_owner.`1`,
       raw_owner.`alpha`,
       raw_owner
         .`json_object('tags',json_array('sql','json'),'score',10)`
FROM sq_hard_w23_values AS raw_owner;

CHECK VIEW sq_hard_w23_values, sq_hard_w23_value_v FOR UPGRADE;
REPAIR VIEW sq_hard_w23_values FROM MYSQL;

SELECT DISTINCTROW HIGH_PRIORITY STRAIGHT_JOIN
       SQL_BIG_RESULT SQL_BUFFER_RESULT SQL_CACHE SQL_CALC_FOUND_ROWS
       value_owner.item_id,
       value_owner.item_label,
       COUNT(tag_owner.tag_name) AS tag_rows,
       MAX(
         JSON_VALUE(value_owner.payload, '$.score')
       ) AS score_value
FROM sq_hard_w23_value_v AS value_owner
LEFT JOIN JSON_TABLE(
  value_owner.payload,
  '$.tags[*]'
  COLUMNS (
    tag_no   FOR ORDINALITY,
    tag_name VARCHAR(20) PATH '$'
  )
) AS tag_owner
  ON TRUE
GROUP BY value_owner.item_id, value_owner.item_label
ORDER BY value_owner.item_id
LIMIT 2;

SET @sq_hard_w23_found_rows = FOUND_ROWS();

SELECT GROUP_CONCAT(
         CONCAT(
           item_id, ':', item_label, ':', tag_rows, ':', score_value
         )
         ORDER BY item_id
         SEPARATOR '/'
       )
INTO @sq_hard_w23_value_shape
FROM (
  SELECT value_owner.item_id,
         value_owner.item_label,
         COUNT(tag_owner.tag_name) AS tag_rows,
         MAX(
           JSON_VALUE(value_owner.payload, '$.score')
         ) AS score_value
  FROM sq_hard_w23_value_v AS value_owner
  LEFT JOIN JSON_TABLE(
    value_owner.payload,
    '$.tags[*]'
    COLUMNS (
      tag_no   FOR ORDINALITY,
      tag_name VARCHAR(20) PATH '$'
    )
  ) AS tag_owner
    ON TRUE
  GROUP BY value_owner.item_id, value_owner.item_label
) AS grouped_owner;

INSERT INTO sq_hard_w23_note (note_key, note_text, note_value)
VALUES (
  'value-view',
  @sq_hard_w23_value_shape,
  @sq_hard_w23_before_rows * 10 + @sq_hard_w23_found_rows
);

-- ------------------------------------------------------------------------------
-- W23-B: IETF_QUOTES belongs to the CSV table option grammar. Embedded comma,
-- quote, newline, semicolon and every comment introducer are persisted through
-- that external representation, then CHECK and REPAIR consume table-maintenance
-- modifiers that are neither query predicates nor stored-routine handlers.
-- ------------------------------------------------------------------------------
CREATE TABLE sq_hard_w23_csv (
  row_id  INT NOT NULL,
  payload VARCHAR(200) NOT NULL
) ENGINE = CSV
  IETF_QUOTES = YES;

INSERT INTO sq_hard_w23_csv (row_id, payload)
VALUES
  (1, 'comma,"quote"'),
  (2, CONCAT('line-1', CHAR(10), 'line-2')),
  (3, 'semi;--/*csv*/#');

CHECK TABLE sq_hard_w23_csv EXTENDED;
REPAIR TABLE sq_hard_w23_csv QUICK;

SELECT GROUP_CONCAT(
         CONCAT(row_id, ':', HEX(payload))
         ORDER BY row_id
         SEPARATOR '/'
       ),
       SUM(OCTET_LENGTH(payload))
INTO @sq_hard_w23_csv_shape,
     @sq_hard_w23_csv_bytes
FROM sq_hard_w23_csv;

INSERT INTO sq_hard_w23_note (note_key, note_text, note_value)
VALUES (
  'csv-ietf',
  @sq_hard_w23_csv_shape,
  @sq_hard_w23_csv_bytes
);

-- ------------------------------------------------------------------------------
-- W23-C: CACHE INDEX and LOAD INDEX each accept table-local index lists but put
-- KEY/INDEX in opposite statement positions. LOCK TABLES then rebinds the two
-- physical names to reserved-word aliases with different lock modes. All
-- references while locked must use those aliases exactly; the note write is
-- deliberately delayed until UNLOCK TABLES restores the full catalog.
-- ------------------------------------------------------------------------------
CREATE TABLE sq_hard_w23_cache_a (
  item_id    INT NOT NULL PRIMARY KEY,
  item_label VARCHAR(30) NOT NULL,
  KEY ix_w23_cache_a_label (item_label)
) ENGINE = MyISAM;

CREATE TABLE sq_hard_w23_cache_b LIKE sq_hard_w23_cache_a;

INSERT INTO sq_hard_w23_cache_a (item_id, item_label)
VALUES (1, 'alpha'), (2, 'beta');

INSERT INTO sq_hard_w23_cache_b (item_id, item_label)
VALUES (10, 'lexer'), (20, 'format');

CACHE INDEX
  sq_hard_w23_cache_a KEY (PRIMARY, ix_w23_cache_a_label),
  sq_hard_w23_cache_b INDEX (PRIMARY)
IN `default`;

LOAD INDEX INTO CACHE
  sq_hard_w23_cache_a
    KEY (PRIMARY, ix_w23_cache_a_label) IGNORE LEAVES,
  sq_hard_w23_cache_b
    INDEX (PRIMARY);

LOCK TABLES
  sq_hard_w23_cache_a AS `select` READ LOCAL,
  sq_hard_w23_cache_b AS `from` WRITE CONCURRENT
  NOWAIT;

SELECT GROUP_CONCAT(
         CONCAT(
           `select`.item_id, ':', `select`.item_label, '>',
           `from`.item_id, ':', `from`.item_label
         )
         ORDER BY `select`.item_id, `from`.item_id
         SEPARATOR '/'
       ),
       COUNT(*)
INTO @sq_hard_w23_lock_shape,
     @sq_hard_w23_lock_rows
FROM sq_hard_w23_cache_a AS `select`
CROSS JOIN sq_hard_w23_cache_b AS `from`;

UNLOCK TABLES;

INSERT INTO sq_hard_w23_note (note_key, note_text, note_value)
VALUES (
  'cache-lock',
  @sq_hard_w23_lock_shape,
  @sq_hard_w23_lock_rows
);

-- ------------------------------------------------------------------------------
-- Wave-23 self-verification.
-- ------------------------------------------------------------------------------
CALL sq_hard_assert(
  @sq_hard_w23_before_rows = 3
  AND @sq_hard_w23_found_rows = 4
  AND @sq_hard_w23_value_shape =
    '1:alpha:2:10/2:beta:1:20/3:gamma:2:30/4:delta:0:40',
  CONCAT(
    'VALUES view: ',
    COALESCE(@sq_hard_w23_value_shape, 'NULL')
  ));
CALL sq_hard_assert(
  @sq_hard_w23_csv_shape =
    '1:636F6D6D612C2271756F746522/'
    '2:6C696E652D310A6C696E652D32/'
    '3:73656D693B2D2D2F2A6373762A2F23'
  AND @sq_hard_w23_csv_bytes = 41,
  CONCAT(
    'CSV IETF quotes: ',
    COALESCE(@sq_hard_w23_csv_shape, 'NULL'),
    '/',
    COALESCE(@sq_hard_w23_csv_bytes, -1)
  ));
CALL sq_hard_assert(
  @sq_hard_w23_lock_shape =
    '1:alpha>10:lexer/1:alpha>20:format/'
    '2:beta>10:lexer/2:beta>20:format'
  AND @sq_hard_w23_lock_rows = 4
  AND (SELECT COUNT(*) FROM sq_hard_w23_note) = 3,
  CONCAT(
    'key cache/locks: ',
    COALESCE(@sq_hard_w23_lock_shape, 'NULL')
  ));

-- ------------------------------------------------------------------------------
-- ULTRA WAVE 24 -- Oracle-compatible associative-array and positional
-- query-block singularity.
--
-- An in-flight sql_mode island adds record constructors, %TYPE/%ROWTYPE
-- anchors, sparse INDEX BY collections and the legacy (+) outer-join marker.
-- Back in MariaDB mode, a 12.2 INDEX_MERGE hint reaches through a derived table
-- by addressing its implicit backtick-quoted `select#2` query-block name. The
-- same aliases therefore change meaning across declaration, expression, join,
-- optimizer-comment and derived-table scopes.
-- ------------------------------------------------------------------------------
CREATE TABLE sq_hard_w24_note (
  note_key   VARCHAR(40) NOT NULL PRIMARY KEY,
  note_text  VARCHAR(1000) NOT NULL,
  note_value BIGINT NOT NULL
) ENGINE = InnoDB;

CREATE TABLE sq_hard_w24_source (
  row_id      INT NOT NULL PRIMARY KEY,
  lookup_key  VARCHAR(20) NOT NULL UNIQUE,
  label_text  VARCHAR(30) NOT NULL,
  amount      INT NOT NULL
) ENGINE = InnoDB;

INSERT INTO sq_hard_w24_source (
  row_id,
  lookup_key,
  label_text,
  amount
)
VALUES
  (1, 'beta',  'format',    20),
  (2, 'alpha', 'parse',     10),
  (3, 'gamma', 'highlight', 30);

-- ------------------------------------------------------------------------------
-- W24-A: TYPE first names a RECORD constructor, then two TABLE OF declarations
-- turn the same word into collection grammar. One element is explicit; the
-- other is anchored to a complete physical row. Dot completion must distinguish
-- collection methods from record fields on both sides of each assignment.
-- ------------------------------------------------------------------------------
SET @sq_hard_w24_saved_mode = @@SESSION.sql_mode;
SET SESSION sql_mode = 'ORACLE';

DELIMITER /
DECLARE
  TYPE explicit_row_t IS RECORD (
    row_id     INT,
    label_text VARCHAR2(30),
    amount     INT
  );
  TYPE explicit_map_t IS TABLE OF explicit_row_t INDEX BY VARCHAR2(20);
  TYPE anchored_map_t IS TABLE OF sq_hard_w24_source%ROWTYPE
    INDEX BY VARCHAR2(20);
  explicit_map explicit_map_t;
  anchored_map anchored_map_t;
  current_key sq_hard_w24_source.lookup_key%TYPE;
  prior_key   sq_hard_w24_source.lookup_key%TYPE;
  shape_text  VARCHAR2(1000) := NULL;
BEGIN
  FOR source_row IN (
    SELECT row_id, lookup_key, label_text, amount
    FROM sq_hard_w24_source
    ORDER BY row_id
  ) LOOP
    anchored_map(source_row.lookup_key) := source_row;
    explicit_map(source_row.lookup_key) := explicit_row_t(
      source_row.row_id,
      source_row.label_text,
      source_row.amount
    );
  END LOOP;

  explicit_map('discard') := explicit_row_t(99, 'remove', 99);
  IF explicit_map.EXISTS('discard') THEN
    explicit_map.DELETE('discard');
  END IF;

  current_key := explicit_map.FIRST;
  WHILE current_key IS NOT NULL LOOP
    shape_text := shape_text
      || CASE WHEN shape_text IS NULL THEN NULL ELSE '/' END
      || current_key || ':'
      || explicit_map(current_key).row_id || ':'
      || anchored_map(current_key).label_text || ':'
      || explicit_map(current_key).amount;
    current_key := explicit_map.NEXT(current_key);
  END LOOP;

  prior_key := explicit_map.PRIOR(explicit_map.LAST);

  INSERT INTO sq_hard_w24_note (note_key, note_text, note_value)
  VALUES (
    'associative-array',
    shape_text
      || '|first=' || explicit_map.FIRST
      || '|prior=' || prior_key
      || '|last=' || explicit_map.LAST,
    explicit_map.COUNT * 100 + anchored_map.COUNT
  );
END;
/
DELIMITER ;

-- ------------------------------------------------------------------------------
-- W24-B: the (+) token belongs to the nullable CHILD side of an Oracle-style
-- outer join, not arithmetic. GROUP_CONCAT remains MariaDB grammar inside the
-- Oracle island, while ||, NVL and the join marker use their Oracle meanings.
-- ------------------------------------------------------------------------------
CREATE TABLE sq_hard_w24_parent (
  parent_id   INT NOT NULL PRIMARY KEY,
  parent_name VARCHAR(20) NOT NULL
) ENGINE = InnoDB;

CREATE TABLE sq_hard_w24_child (
  child_id   INT NOT NULL PRIMARY KEY,
  parent_id  INT NOT NULL,
  child_name VARCHAR(20) NOT NULL,
  KEY ix_sq_hard_w24_child_parent (parent_id)
) ENGINE = InnoDB;

INSERT INTO sq_hard_w24_parent (parent_id, parent_name)
VALUES (1, 'root'), (2, 'empty');

INSERT INTO sq_hard_w24_child (child_id, parent_id, child_name)
VALUES (10, 1, 'leaf-a'), (20, 1, 'leaf-b');

SELECT GROUP_CONCAT(
         parent_owner.parent_id || ':' || parent_owner.parent_name || ':'
         || NVL(CAST(child_owner.child_id AS CHAR), '-') || ':'
         || NVL(child_owner.child_name, '-')
         ORDER BY parent_owner.parent_id, child_owner.child_id
         SEPARATOR '/'
       ),
       COUNT(*)
INTO @sq_hard_w24_outer_shape,
     @sq_hard_w24_outer_rows
FROM sq_hard_w24_parent parent_owner,
     sq_hard_w24_child child_owner
WHERE parent_owner.parent_id = child_owner.parent_id(+);

INSERT INTO sq_hard_w24_note (note_key, note_text, note_value)
VALUES (
  'oracle-outer-join',
  @sq_hard_w24_outer_shape,
  @sq_hard_w24_outer_rows
);

SET SESSION sql_mode = @sq_hard_w24_saved_mode;

-- ------------------------------------------------------------------------------
-- W24-C: `select#2` is a server-minted query-block name inside a quoted
-- optimizer-hint identifier. INDEX_MERGE targets E in that nested block and
-- whitelists two comma-separated keys. LIMIT prevents derived-table merging,
-- so EXPLAIN must expose a union of the two physical range scans.
-- ------------------------------------------------------------------------------
CREATE TABLE sq_hard_w24_event (
  event_id    INT NOT NULL PRIMARY KEY,
  event_kind  VARCHAR(12) NOT NULL,
  score_value INT NOT NULL,
  amount      INT NOT NULL,
  KEY ix_sq_hard_w24_kind (event_kind),
  KEY ix_sq_hard_w24_score (score_value)
) ENGINE = InnoDB;

INSERT INTO sq_hard_w24_event (
  event_id,
  event_kind,
  score_value,
  amount
)
SELECT seq,
       CASE WHEN MOD(seq, 100) = 0 THEN 'alpha' ELSE 'other' END,
       CASE WHEN seq = 777 THEN 777 ELSE MOD(seq, 50) END,
       MOD(seq, 17) + 1
FROM seq_1_to_1000;

EXPLAIN FORMAT = JSON
SELECT /*+ INDEX_MERGE(
              event_owner@`select#2`
              ix_sq_hard_w24_kind, ix_sq_hard_w24_score
            ) */
       SUM(derived_owner.amount)
FROM (
  SELECT event_owner.event_id, event_owner.amount
  FROM sq_hard_w24_event event_owner
  WHERE event_owner.event_kind = 'alpha'
     OR event_owner.score_value = 777
  LIMIT 100000
) derived_owner;

SELECT /*+ INDEX_MERGE(
              event_owner@`select#2`
              ix_sq_hard_w24_kind, ix_sq_hard_w24_score
            ) */
       GROUP_CONCAT(
         CONCAT(derived_owner.event_id, ':', derived_owner.amount)
         ORDER BY derived_owner.event_id
         SEPARATOR '/'
       ),
       SUM(derived_owner.amount),
       COUNT(*)
INTO @sq_hard_w24_hint_shape,
     @sq_hard_w24_hint_total,
     @sq_hard_w24_hint_rows
FROM (
  SELECT event_owner.event_id, event_owner.amount
  FROM sq_hard_w24_event event_owner
  WHERE event_owner.event_kind = 'alpha'
     OR event_owner.score_value = 777
  LIMIT 100000
) derived_owner;

INSERT INTO sq_hard_w24_note (note_key, note_text, note_value)
VALUES (
  'positional-index-merge',
  @sq_hard_w24_hint_shape,
  @sq_hard_w24_hint_total * 100 + @sq_hard_w24_hint_rows
);

-- ------------------------------------------------------------------------------
-- Wave-24 self-verification.
-- ------------------------------------------------------------------------------
CALL sq_hard_assert(
  (SELECT note_text
   FROM sq_hard_w24_note
   WHERE note_key = 'associative-array') =
    'alpha:2:parse:10/beta:1:format:20/gamma:3:highlight:30'
    '|first=alpha|prior=beta|last=gamma'
  AND (SELECT note_value
       FROM sq_hard_w24_note
       WHERE note_key = 'associative-array') = 303,
  CONCAT(
    'associative array: ',
    COALESCE(
      (SELECT note_text
       FROM sq_hard_w24_note
       WHERE note_key = 'associative-array'),
      'NULL'
    )
  ));
CALL sq_hard_assert(
  @sq_hard_w24_outer_shape =
    '1:root:10:leaf-a/1:root:20:leaf-b/2:empty:-:-'
  AND @sq_hard_w24_outer_rows = 3
  AND @@SESSION.sql_mode = @sq_hard_w24_saved_mode,
  CONCAT(
    'Oracle outer join/mode: ',
    COALESCE(@sq_hard_w24_outer_shape, 'NULL'),
    '/',
    COALESCE(@sq_hard_w24_outer_rows, -1),
    '/',
    @@SESSION.sql_mode
  ));
CALL sq_hard_assert(
  @sq_hard_w24_hint_shape =
    '100:16/200:14/300:12/400:10/500:8/600:6/'
    '700:4/777:13/800:2/900:17/1000:15'
  AND @sq_hard_w24_hint_total = 117
  AND @sq_hard_w24_hint_rows = 11
  AND (SELECT COUNT(*) FROM sq_hard_w24_note) = 3,
  CONCAT(
    'positional index merge: ',
    COALESCE(@sq_hard_w24_hint_shape, 'NULL'),
    '/',
    COALESCE(@sq_hard_w24_hint_total, -1),
    '/',
    COALESCE(@sq_hard_w24_hint_rows, -1)
  ));

SELECT 'PASS' AS final_status,
       VERSION() AS server_version,
       (SELECT COUNT(*) FROM metric) AS metric_rows,
       @walk_total AS walk_total,
       (SELECT COUNT(*) FROM sq_hard_w4_vector_asset) AS vector_rows,
       @sq_hard_w4_walk_total AS wave4_total,
       (SELECT COUNT(*) FROM sq_hard_w5_hint) AS wave5_rows,
       @sq_hard_w5_label AS wave5_label,
       (SELECT COUNT(*) FROM sq_hard_w6_series) AS wave6_rows,
       @sq_hard_w6_walk_rows AS wave6_walk_rows,
       (SELECT COUNT(*) FROM sq_hard_w7_lease) AS wave7_lease_rows,
       @sq_hard_w7_rowvar AS wave7_rowvar,
       (SELECT COUNT(*) FROM sq_hard_w8_fact) AS wave8_fact_rows,
       (SELECT sq_hard_w8_product(factor) FROM sq_hard_w8_fact
        WHERE bucket = 'beta') AS wave8_product,
       @sq_hard_w8_anchored AS wave8_anchored,
       (SELECT COUNT(*) FROM sq_hard_w9_note) AS wave9_notes,
       @sq_hard_w9_oracle_total AS wave9_island_total,
       (SELECT SUM(amount) FROM sq_hard_w9_ledger) AS wave9_ledger,
       @sq_hard_w9_delims AS wave9_delims,
       (SELECT COUNT(*) FROM sq_hard_w10_note) AS wave10_notes,
       @sq_hard_w10_running AS wave10_running,
       (SELECT SUM(amount) FROM sq_hard_w10_ledger) AS wave10_ledger,
       @sq_hard_w10_ansi AS wave10_ansi,
       (SELECT COUNT(*) FROM sq_hard_w11_note) AS wave11_notes,
       @sq_hard_w11_type_sum AS wave11_type_sum,
       @sq_hard_w11_island AS wave11_island,
       @sq_hard_w11_total AS wave11_walk_total,
       @sq_hard_w11_lexer AS wave11_lexer,
       (SELECT COUNT(*) FROM sq_hard_w12_note) AS wave12_notes,
       @sq_hard_w12_verbs AS wave12_verbs,
       @sq_hard_w12_quoted AS wave12_quoted,
       @sq_hard_w12_setops AS wave12_setops,
       @sq_hard_w12_dyn_hits AS wave12_dynamic_hits,
       @sq_hard_w12_lexer AS wave12_lexer,
       (SELECT COUNT(*) FROM sq_hard_w13_note) AS wave13_notes,
       @sq_hard_w13_natural AS wave13_natural_order,
       @sq_hard_w13_tags AS wave13_json_tags,
       @sq_hard_w13_history_rows AS wave13_history_rows,
       @sq_hard_w13_dynamic_out AS wave13_dynamic_out,
       @sq_hard_w13_warning_count AS wave13_warning_count,
       (SELECT COUNT(*) FROM sq_hard_w14_note) AS wave14_notes,
       @sq_hard_w14_parameter_shape AS wave14_parameter_defaults,
       @sq_hard_w14_trigger_shape AS wave14_trigger_columns,
       @sq_hard_w14_json_depth AS wave14_json_depth,
       @sq_hard_w14_json_leaf AS wave14_json_leaf,
       @sq_hard_w14_hint_rows AS wave14_hint_rows,
       @sq_hard_w14_month_start AS wave14_month_start,
       (SELECT COUNT(*) FROM sq_hard_w15_note) AS wave15_notes,
       @sq_hard_w15_assignment_shape AS wave15_assignment_shape,
       @sq_hard_w15_fact_shape AS wave15_fact_shape,
       @sq_hard_w15_walk_shape AS wave15_walk_shape,
       @sq_hard_w15_ties AS wave15_ties,
       (SELECT COUNT(*) FROM sq_hard_w16_note) AS wave16_notes,
       @sq_hard_w16_cursor_shape AS wave16_cursor_shape,
       @sq_hard_w16_return AS wave16_function_return,
       @sq_hard_w16_running AS wave16_function_inout,
       @sq_hard_w16_snapshot_shape AS wave16_snapshot_shape,
       (SELECT COUNT(*) FROM sq_hard_w17_note) AS wave17_notes,
       @sq_hard_w17_hybrid_shape AS wave17_hybrid_shape,
       JSON_VALUE(
         @sq_hard_w17_hybrid_json,
         '$."1".title'
       ) AS wave17_top_title,
       (SELECT COUNT(*) FROM sq_hard_w18_note) AS wave18_notes,
       @sq_hard_w18_scope_shape AS wave18_scope_shape,
       (SELECT COUNT(*) FROM sq_hard_w19_note) AS wave19_notes,
       @sq_hard_w19_document_shape AS wave19_document_shape,
       (SELECT COUNT(*) FROM sq_hard_w20_note) AS wave20_notes,
       @sq_hard_w20_walk_shape AS wave20_walk_shape,
       @sq_hard_w20_json_shape AS wave20_json_shape,
       @sq_hard_w20_bag_shape AS wave20_bag_shape,
       (SELECT COUNT(*) FROM sq_hard_w21_note) AS wave21_notes,
       @sq_hard_w21_right_shape AS wave21_right_shape,
       @sq_hard_w21_dynamic_shape AS wave21_dynamic_shape,
       (SELECT COUNT(*) FROM sq_hard_w22_note) AS wave22_notes,
       @sq_hard_w22_upsert_shape AS wave22_upsert_shape,
       @sq_hard_w22_shadow_shape AS wave22_shadow_shape,
       @sq_hard_w22_reverse_shape AS wave22_reverse_shape,
       (SELECT COUNT(*) FROM sq_hard_w23_note) AS wave23_notes,
       @sq_hard_w23_value_shape AS wave23_value_shape,
       @sq_hard_w23_csv_shape AS wave23_csv_shape,
       @sq_hard_w23_lock_shape AS wave23_lock_shape,
       (SELECT COUNT(*) FROM sq_hard_w24_note) AS wave24_notes,
       (SELECT note_text
        FROM sq_hard_w24_note
        WHERE note_key = 'associative-array') AS wave24_array_shape,
       @sq_hard_w24_outer_shape AS wave24_outer_shape,
       @sq_hard_w24_hint_shape AS wave24_hint_shape;

-- ------------------------------------------------------------------------------
-- ULTRA WAVE 25 -- geospatial-window, derived-hint and authorization-state
-- singularity.
--
-- MariaDB 11.8+ GIS values move through aggregate-window, validation,
-- simplification and GeoHash grammars. MariaDB 12.0/12.1 optimizer hints then
-- target the alias of a spatially aggregated derived table. Finally,
-- SET SESSION AUTHORIZATION deliberately replaces every session attribute,
-- so this wave persists all evidence in tables and executes after the W1-W24
-- user-variable checkpoint above.
-- ------------------------------------------------------------------------------
DROP USER IF EXISTS 'sq_hard_w25_actor'@'localhost';

CREATE TABLE sq_hard_w25_note (
  note_key   VARCHAR(40) NOT NULL PRIMARY KEY,
  note_text  VARCHAR(1000) NOT NULL,
  note_value BIGINT NOT NULL
) ENGINE = InnoDB;

CREATE TABLE sq_hard_w25_route (
  route_id   INT NOT NULL PRIMARY KEY,
  route_name VARCHAR(30) NOT NULL UNIQUE
) ENGINE = InnoDB;

CREATE TABLE sq_hard_w25_point (
  route_id  INT NOT NULL,
  point_id  INT NOT NULL,
  position  POINT NOT NULL,
  payload   JSON NOT NULL CHECK (JSON_VALID(payload)),
  PRIMARY KEY (route_id, point_id),
  CONSTRAINT fk_sq_hard_w25_point_route
    FOREIGN KEY (route_id) REFERENCES sq_hard_w25_route (route_id)
) ENGINE = InnoDB;

INSERT INTO sq_hard_w25_route (route_id, route_name)
VALUES (1, 'alpha'), (2, 'beta');

INSERT INTO sq_hard_w25_point (
  route_id,
  point_id,
  position,
  payload
)
VALUES
  (
    1, 1, ST_GeomFromText('POINT(0 0)', 0),
    JSON_OBJECT('token', 'start;--/*gis*/#', 'bytes', 1024)
  ),
  (
    1, 2, ST_GeomFromText('POINT(1 0)', 0),
    JSON_OBJECT('token', 'middle', 'bytes', 1536)
  ),
  (
    1, 3, ST_GeomFromText('POINT(2 0)', 0),
    JSON_OBJECT('token', 'finish', 'bytes', 1048576)
  ),
  (
    2, 1, ST_GeomFromText('POINT(1 1)', 0),
    JSON_OBJECT('token', 'north', 'bytes', 2048)
  ),
  (
    2, 2, ST_GeomFromText('POINT(1 2)', 0),
    JSON_OBJECT('token', 'south', 'bytes', 4096)
  );

-- ------------------------------------------------------------------------------
-- W25-A: ST_COLLECT is an aggregate with an OVER clause here, not a scalar
-- geometry constructor. Its running frame returns a different MULTIPOINT type
-- at every row; ROW_NUMBER selects only each partition's terminal collection.
-- ------------------------------------------------------------------------------
INSERT INTO sq_hard_w25_note (note_key, note_text, note_value)
WITH running_geometry AS (
  SELECT point_owner.route_id,
         point_owner.point_id,
         ST_Collect(point_owner.position) OVER (
           PARTITION BY point_owner.route_id
           ORDER BY point_owner.point_id
           ROWS BETWEEN UNBOUNDED PRECEDING AND CURRENT ROW
         ) AS running_shape,
         ROW_NUMBER() OVER (
           PARTITION BY point_owner.route_id
           ORDER BY point_owner.point_id DESC
         ) AS reverse_position
  FROM sq_hard_w25_point AS point_owner
),
terminal_geometry AS (
  SELECT route_id,
         running_shape
  FROM running_geometry
  WHERE reverse_position = 1
)
SELECT 'spatial-window',
       GROUP_CONCAT(
         CONCAT(
           terminal_owner.route_id,
           ':',
           ST_AsText(terminal_owner.running_shape)
         )
         ORDER BY terminal_owner.route_id
         SEPARATOR '/'
       ),
       SUM(ST_NumGeometries(terminal_owner.running_shape))
FROM terminal_geometry AS terminal_owner;

-- ------------------------------------------------------------------------------
-- W25-B: simplification returns a geometry, validation returns a geometry,
-- validity returns a Boolean integer, and GeoHash alternates between POINT,
-- string, latitude/longitude scalars and POINT again. FORMAT_BYTES adds a unit
-- mini-language beside JSON_VALUE's path mini-language.
-- ------------------------------------------------------------------------------
SET @sq_hard_w25_line = ST_GeomFromText(
  'LINESTRING(0 0,1 0.01,2 -0.01,3 0)',
  0
);
SET @sq_hard_w25_hash = ST_GeoHash(
  ST_GeomFromText('POINT(1 1)', 0),
  15
);

INSERT INTO sq_hard_w25_note (note_key, note_text, note_value)
SELECT 'spatial-function-chain',
       CONCAT(
         ST_AsText(ST_Simplify(@sq_hard_w25_line, 0.05)),
         '|valid=', ST_IsValid(@sq_hard_w25_line),
         '|validated=', ST_AsText(ST_Validate(@sq_hard_w25_line)),
         '|hash=', @sq_hard_w25_hash,
         '|lat=', ST_LatFromGeoHash(@sq_hard_w25_hash),
         '|long=', ST_LongFromGeoHash(@sq_hard_w25_hash),
         '|point=', ST_AsText(
           ST_PointFromGeoHash(@sq_hard_w25_hash, 0)
         ),
         '|sizes=',
         (
           SELECT GROUP_CONCAT(
                    FORMAT_BYTES(
                      JSON_VALUE(point_owner.payload, '$.bytes')
                    )
                    ORDER BY point_owner.point_id
                    SEPARATOR ','
                  )
           FROM sq_hard_w25_point AS point_owner
           WHERE point_owner.route_id = 1
         )
       ),
       ST_NumPoints(ST_Simplify(@sq_hard_w25_line, 0.05)) * 100
         + ST_IsValid(ST_Validate(@sq_hard_w25_line));

-- ------------------------------------------------------------------------------
-- W25-C: the outer hint list addresses a derived-table alias, while QB_NAME
-- establishes a separate namespace inside the derived query. NO_MERGE keeps
-- the aggregate materialized, DERIVED_CONDITION_PUSHDOWN moves route filters
-- inward, and SPLIT_MATERIALIZED permits route-correlated partial grouping.
-- ------------------------------------------------------------------------------
EXPLAIN FORMAT = JSON
SELECT /*+ NO_MERGE(point_summary)
            DERIVED_CONDITION_PUSHDOWN(point_summary)
            SPLIT_MATERIALIZED(point_summary) */
       route_owner.route_id,
       route_owner.route_name,
       point_summary.geometry_type,
       point_summary.point_count
FROM sq_hard_w25_route AS route_owner
JOIN (
  SELECT /*+ QB_NAME(w25_spatial_summary) */
         point_owner.route_id,
         ST_GeometryType(ST_Collect(point_owner.position)) AS geometry_type,
         ST_NumGeometries(ST_Collect(point_owner.position)) AS point_count
  FROM sq_hard_w25_point AS point_owner
  GROUP BY point_owner.route_id
) AS point_summary
  ON point_summary.route_id = route_owner.route_id
WHERE route_owner.route_id IN (1, 2)
ORDER BY route_owner.route_id;

INSERT INTO sq_hard_w25_note (note_key, note_text, note_value)
SELECT 'derived-spatial-hints',
       GROUP_CONCAT(
         CONCAT(
           hinted_owner.route_id,
           ':',
           hinted_owner.route_name,
           ':',
           hinted_owner.geometry_type,
           ':',
           hinted_owner.point_count
         )
         ORDER BY hinted_owner.route_id
         SEPARATOR '/'
       ),
       SUM(hinted_owner.point_count)
FROM (
  SELECT /*+ NO_MERGE(point_summary)
              DERIVED_CONDITION_PUSHDOWN(point_summary)
              SPLIT_MATERIALIZED(point_summary) */
         route_owner.route_id,
         route_owner.route_name,
         point_summary.geometry_type,
         point_summary.point_count
  FROM sq_hard_w25_route AS route_owner
  JOIN (
    SELECT /*+ QB_NAME(w25_spatial_summary) */
           point_owner.route_id,
           ST_GeometryType(ST_Collect(point_owner.position)) AS geometry_type,
           ST_NumGeometries(ST_Collect(point_owner.position)) AS point_count
    FROM sq_hard_w25_point AS point_owner
    GROUP BY point_owner.route_id
  ) AS point_summary
    ON point_summary.route_id = route_owner.route_id
  WHERE route_owner.route_id IN (1, 2)
) AS hinted_owner;

-- ------------------------------------------------------------------------------
-- W25-D: this statement changes authorization for the connection itself, not
-- a stored-program DEFINER. MariaDB resets DATABASE(), user variables and other
-- session state on both switches. The low-privilege actor therefore selects
-- the database again, records all three identity functions, then uses SET USER
-- solely to restore the original root authorization.
-- ------------------------------------------------------------------------------
CREATE USER 'sq_hard_w25_actor'@'localhost'
IDENTIFIED BY 'sq-hard-w25;--/*auth*/#';

GRANT SELECT, INSERT
ON sq_hard_mariadb.sq_hard_w25_note
TO 'sq_hard_w25_actor'@'localhost';

GRANT SET USER
ON *.*
TO 'sq_hard_w25_actor'@'localhost';

COMMIT;

SET SESSION AUTHORIZATION 'sq_hard_w25_actor'@'localhost';
USE sq_hard_mariadb;

INSERT INTO sq_hard_w25_note (note_key, note_text, note_value)
VALUES (
  'session-authorization',
  CONCAT(
    USER(),
    '|',
    CURRENT_USER(),
    '|',
    SESSION_USER(),
    '|',
    DATABASE()
  ),
  USER() = 'sq_hard_w25_actor@localhost'
    AND CURRENT_USER() = 'sq_hard_w25_actor@localhost'
    AND SESSION_USER() = 'sq_hard_w25_actor@localhost'
);

SET SESSION AUTHORIZATION 'root'@'localhost';
USE sq_hard_mariadb;

DROP USER 'sq_hard_w25_actor'@'localhost';

-- ------------------------------------------------------------------------------
-- Wave-25 self-verification.
-- ------------------------------------------------------------------------------
CALL sq_hard_assert(
  (SELECT note_text
   FROM sq_hard_w25_note
   WHERE note_key = 'spatial-window') =
    '1:MULTIPOINT(0 0,1 0,2 0)/2:MULTIPOINT(1 1,1 2)'
  AND (SELECT note_value
       FROM sq_hard_w25_note
       WHERE note_key = 'spatial-window') = 5,
  'wave25 spatial window'
);

CALL sq_hard_assert(
  (SELECT note_text
   FROM sq_hard_w25_note
   WHERE note_key = 'spatial-function-chain') =
    'LINESTRING(0 0,3 0)|valid=1|validated='
    'LINESTRING(0 0,1 0.01,2 -0.01,3 0)'
    '|hash=s00twy01mtw037m|lat=1|long=1|point=POINT(1 1)'
    '|sizes=1.00 KiB,1.50 KiB,1.00 MiB'
  AND (SELECT note_value
       FROM sq_hard_w25_note
       WHERE note_key = 'spatial-function-chain') = 201,
  'wave25 spatial functions'
);

CALL sq_hard_assert(
  (SELECT note_text
   FROM sq_hard_w25_note
   WHERE note_key = 'derived-spatial-hints') =
    '1:alpha:MULTIPOINT:3/2:beta:MULTIPOINT:2'
  AND (SELECT note_value
       FROM sq_hard_w25_note
       WHERE note_key = 'derived-spatial-hints') = 5,
  'wave25 derived hints'
);

CALL sq_hard_assert(
  (SELECT note_text
   FROM sq_hard_w25_note
   WHERE note_key = 'session-authorization') =
    'sq_hard_w25_actor@localhost|'
    'sq_hard_w25_actor@localhost|'
    'sq_hard_w25_actor@localhost|'
    'sq_hard_mariadb'
  AND (SELECT note_value
       FROM sq_hard_w25_note
       WHERE note_key = 'session-authorization') = 1
  AND (SELECT COUNT(*) FROM sq_hard_w25_note) = 4
  AND (
    SELECT COUNT(*)
    FROM mysql.user
    WHERE User = 'sq_hard_w25_actor'
      AND Host = 'localhost'
  ) = 0,
  'wave25 session authorization'
);

SELECT 'PASS' AS final_status,
       VERSION() AS server_version,
       (SELECT COUNT(*) FROM sq_hard_w25_note) AS wave25_notes,
       (SELECT note_text
        FROM sq_hard_w25_note
        WHERE note_key = 'spatial-window') AS wave25_window_shape,
       (SELECT note_text
        FROM sq_hard_w25_note
        WHERE note_key = 'spatial-function-chain') AS wave25_function_shape,
       (SELECT note_text
        FROM sq_hard_w25_note
        WHERE note_key = 'derived-spatial-hints') AS wave25_hint_shape,
       (SELECT note_text
        FROM sq_hard_w25_note
        WHERE note_key = 'session-authorization') AS wave25_auth_shape;

-- ------------------------------------------------------------------------------
-- ULTRA WAVE 26 -- package/table-function/window namespace collapse.
--
-- The package is literally named `json_table`; its public routines are named
-- after a window function, a JSON scalar function, a prepared-statement verb,
-- and an aggregate. The source table is literally `window`, and its columns
-- occupy WITH/SELECT/OVER/JSON_TABLE/RANK keyword namespaces. Inside one package
-- body those owners cross a two-level JSON_TABLE, inherited named windows,
-- bitwise window aggregates, dynamic SQL, parameter markers, and an alternate
-- delimiter that also occurs inside the dynamic statement text. Backticks are
-- semantically required throughout: removing any one of them changes the token
-- class instead of merely changing presentation.
-- ------------------------------------------------------------------------------
CREATE TABLE `window` (
  `with`       INT NOT NULL,
  `select`     VARCHAR(30) NOT NULL,
  `over`       INT NOT NULL,
  `json_table` JSON NOT NULL,
  `rank`       INT AS (
                 JSON_LENGTH(`json_table`, '$.rows')
               ) PERSISTENT,
  PRIMARY KEY (`with`),
  CONSTRAINT ck_sq_hard_w26_json
    CHECK (JSON_VALID(`json_table`))
) ENGINE = InnoDB;

CREATE TABLE sq_hard_w26_note (
  note_key   VARCHAR(40) NOT NULL PRIMARY KEY,
  note_text  VARCHAR(1200) NOT NULL,
  note_value BIGINT NOT NULL
) ENGINE = InnoDB;

INSERT INTO `window` (
  `with`,
  `select`,
  `over`,
  `json_table`
)
VALUES
  (
    1,
    'alpha',
    100,
    JSON_OBJECT(
      'select', 'alpha',
      'rows', JSON_ARRAY(
        JSON_OBJECT(
          'order', 1,
          'range', 'A',
          'values', JSON_ARRAY(2, 4)
        ),
        JSON_OBJECT(
          'order', 2,
          'range', 'B',
          'values', JSON_ARRAY(8)
        )
      )
    )
  ),
  (
    2,
    'beta',
    200,
    JSON_OBJECT(
      'select', 'beta',
      'rows', JSON_ARRAY(
        JSON_OBJECT(
          'order', 1,
          'range', 'C',
          'values', JSON_ARRAY(1, 3, 9)
        )
      )
    )
  );

-- ------------------------------------------------------------------------------
-- W26-A: the SQL/PSM package uses quoted builtin names as public entry points.
-- `rank` contains the statically parsed relation graph. `prepare` builds and
-- executes another JSON_TABLE/window graph whose text contains the active
-- |~| terminator plus all three SQL comment openers without ending the package.
-- Package initialization and both procedures mutate private session state.
-- ------------------------------------------------------------------------------
DELIMITER |~|
CREATE OR REPLACE PACKAGE `json_table`
SQL SECURITY INVOKER
COMMENT 'keyword owners; |~| is data inside this package comment'
  PROCEDURE `rank`(
    IN  p_floor INT,
    OUT p_shape VARCHAR(1000),
    OUT p_total BIGINT
  );
  PROCEDURE `prepare`(
    IN  p_document LONGTEXT,
    OUT p_shape    VARCHAR(1000)
  );
  FUNCTION `json_value`(
    p_document LONGTEXT,
    p_path     VARCHAR(200)
  ) RETURNS VARCHAR(120);
  FUNCTION `count`() RETURNS INT;
END|~|

CREATE OR REPLACE PACKAGE BODY `json_table`
  DECLARE package_invocations INT DEFAULT 0;

  FUNCTION `json_value`(
    p_document LONGTEXT,
    p_path     VARCHAR(200)
  ) RETURNS VARCHAR(120)
  BEGIN
    RETURN CONCAT(
      'pkg:',
      JSON_UNQUOTE(JSON_EXTRACT(p_document, p_path))
    );
  END;

  FUNCTION `count`() RETURNS INT
  BEGIN
    RETURN package_invocations;
  END;

  PROCEDURE `rank`(
    IN  p_floor INT,
    OUT p_shape VARCHAR(1000),
    OUT p_total BIGINT
  )
  BEGIN
    WITH
      `with` AS (
        SELECT `from`.`with`,
               `from`.`select`,
               `from`.`over`,
               `from`.`rank`,
               `json_table`.`row_number`,
               `json_table`.`order`,
               `json_table`.`range`,
               `json_table`.`value_number`,
               `json_table`.`value`,
               `json_table`.`exists`,
               `json_table`.`json_value`(
                 `from`.`json_table`,
                 '$.select'
               ) AS package_label
        FROM sq_hard_mariadb.`window` AS `from`
        JOIN JSON_TABLE(
               `from`.`json_table`,
               '$.rows[*]' COLUMNS (
                 `row_number` FOR ORDINALITY,
                 `order`      INT         PATH '$.order' ERROR ON ERROR,
                 `range`      VARCHAR(20) PATH '$.range' ERROR ON EMPTY,
                 NESTED PATH '$.values[*]' COLUMNS (
                   `value_number` FOR ORDINALITY,
                   `value`        INT PATH '$' ERROR ON ERROR,
                   `exists`       INT EXISTS PATH '$'
                 )
               )
             ) AS `json_table` ON TRUE
      ),
      `window` AS (
        SELECT `with`.*,
               SUM(`with`.`value`) OVER `rows` AS running_value,
               DENSE_RANK() OVER `rank` AS value_rank,
               BIT_AND(`with`.`value`) OVER `partition` AS bit_and_value,
               BIT_OR(`with`.`value`)  OVER `partition` AS bit_or_value,
               BIT_XOR(`with`.`value`) OVER `partition` AS bit_xor_value
        FROM `with`
        WINDOW
          `partition` AS (
            PARTITION BY `with`.`with`
          ),
          `rows` AS (
            `partition`
            ORDER BY `with`.`order`,
                     `with`.`row_number`,
                     `with`.`value_number`
            ROWS BETWEEN UNBOUNDED PRECEDING AND CURRENT ROW
          ),
          `rank` AS (
            `partition`
            ORDER BY `with`.`value` DESC
          )
      )
    SELECT GROUP_CONCAT(
             CONCAT(
               `window`.`with`, ':',
               `window`.package_label, ':',
               `window`.`order`, ':',
               `window`.`row_number`, ':',
               `window`.`range`, ':',
               `window`.`value`, ':',
               `window`.running_value, ':',
               `window`.value_rank, ':',
               `window`.bit_and_value, ':',
               `window`.bit_or_value, ':',
               `window`.bit_xor_value
             )
             ORDER BY `window`.`with`,
                      `window`.`order`,
                      `window`.`row_number`,
                      `window`.`value_number`
             SEPARATOR '/'
           ),
           SUM(`window`.`value`)
    INTO p_shape,
         p_total
    FROM `window`
    WHERE `window`.`value` >= p_floor
      AND `window`.`exists` = 1;

    SET package_invocations = package_invocations + 1;
  END;

  PROCEDURE `prepare`(
    IN  p_document LONGTEXT,
    OUT p_shape    VARCHAR(1000)
  )
  BEGIN
    SET @sq_hard_w26_document = p_document;
    SET @sq_hard_w26_prepared_shape = NULL;
    SET @sq_hard_w26_statement =
      'WITH `with` AS (
         SELECT `json_table`.`order`,
                `json_table`.`value`,
                '';--/*dynamic*/#|~|$$//'' AS `delimiter`
         FROM JSON_TABLE(
                ?,
                ''$.rows[*]'' COLUMNS (
                  `order` FOR ORDINALITY,
                  `value` INT PATH ''$.value'' ERROR ON ERROR
                )
              ) AS `json_table`
       ),
       `window` AS (
         SELECT `with`.`order`,
                `with`.`value`,
                `with`.`delimiter`,
                SUM(`with`.`value`) OVER `rows` AS `range`
         FROM `with`
         WINDOW `rows` AS (
           ORDER BY `with`.`order`
           ROWS BETWEEN UNBOUNDED PRECEDING AND CURRENT ROW
         )
       )
       SELECT GROUP_CONCAT(
                CONCAT(
                  `window`.`order`, '':'',
                  `window`.`value`, '':'',
                  `window`.`range`
                )
                ORDER BY `window`.`order`
                SEPARATOR ''/''
              )
       INTO @sq_hard_w26_prepared_shape
       FROM `window`
       WHERE `window`.`delimiter` = '';--/*dynamic*/#|~|$$//''';

    PREPARE `select` FROM @sq_hard_w26_statement;
    EXECUTE `select` USING @sq_hard_w26_document;
    DEALLOCATE PREPARE `select`;

    SET p_shape = @sq_hard_w26_prepared_shape;
    SET package_invocations = package_invocations + 1;
  END;

  SET package_invocations = package_invocations + 1;
END|~|
DELIMITER ;

-- ------------------------------------------------------------------------------
-- W26-B: each dotted call below looks like schema qualification until metadata
-- establishes that JSON_TABLE is a package. The final function call also has
-- the exact spelling and arity of the builtin JSON_VALUE function, but its
-- `pkg:` prefix proves that package ownership won.
-- ------------------------------------------------------------------------------
CALL `json_table`.`rank`(
  3,
  @sq_hard_w26_static_shape,
  @sq_hard_w26_static_total
);

SET @sq_hard_w26_dynamic_document = JSON_OBJECT(
  'rows',
  JSON_ARRAY(
    JSON_OBJECT('value', 7),
    JSON_OBJECT('value', 11),
    JSON_OBJECT('value', 13)
  )
);

CALL `json_table`.`prepare`(
  @sq_hard_w26_dynamic_document,
  @sq_hard_w26_dynamic_shape
);

SET @sq_hard_w26_collision =
  `json_table`.`json_value`(
    '{"select":"DELIMITER |~|; -- /* */ # $$ //"}',
    '$.select'
  );
SET @sq_hard_w26_package_count = `json_table`.`count`();

INSERT INTO sq_hard_w26_note (note_key, note_text, note_value)
VALUES
  (
    'static-package-window',
    @sq_hard_w26_static_shape,
    @sq_hard_w26_static_total
  ),
  (
    'prepared-json-window',
    @sq_hard_w26_dynamic_shape,
    JSON_LENGTH(@sq_hard_w26_dynamic_document, '$.rows')
  ),
  (
    'builtin-name-collision',
    @sq_hard_w26_collision,
    @sq_hard_w26_package_count
  );

-- ------------------------------------------------------------------------------
-- Wave-26 self-verification.
-- ------------------------------------------------------------------------------
CALL sq_hard_assert(
  @sq_hard_w26_static_shape =
    '1:pkg:alpha:1:1:A:4:6:2:0:14:14/'
    '1:pkg:alpha:2:2:B:8:14:1:0:14:14/'
    '2:pkg:beta:1:1:C:3:4:2:1:11:11/'
    '2:pkg:beta:1:1:C:9:13:1:1:11:11'
  AND @sq_hard_w26_static_total = 24,
  CONCAT(
    'package/static window: ',
    COALESCE(@sq_hard_w26_static_shape, 'NULL'),
    '/',
    COALESCE(@sq_hard_w26_static_total, -1)
  )
);

CALL sq_hard_assert(
  @sq_hard_w26_dynamic_shape = '1:7:7/2:11:18/3:13:31'
  AND (
    SELECT note_value
    FROM sq_hard_w26_note
    WHERE note_key = 'prepared-json-window'
  ) = 3,
  CONCAT(
    'package/prepared window: ',
    COALESCE(@sq_hard_w26_dynamic_shape, 'NULL')
  )
);

CALL sq_hard_assert(
  @sq_hard_w26_collision =
    'pkg:DELIMITER |~|; -- /* */ # $$ //'
  AND @sq_hard_w26_package_count = 3,
  CONCAT(
    'package builtin collision/state: ',
    COALESCE(@sq_hard_w26_collision, 'NULL'),
    '/',
    COALESCE(@sq_hard_w26_package_count, -1)
  )
);

CALL sq_hard_assert(
  (SELECT SUM(`rank`) FROM `window`) = 3
  AND (SELECT SUM(`over`) FROM `window`) = 300
  AND (SELECT COUNT(*) FROM sq_hard_w26_note) = 3,
  'wave26 generated columns or note rows'
);

SELECT 'PASS' AS final_status,
       VERSION() AS server_version,
       (SELECT COUNT(*) FROM sq_hard_w26_note) AS wave26_notes,
       @sq_hard_w26_static_shape AS wave26_static_shape,
       @sq_hard_w26_dynamic_shape AS wave26_dynamic_shape,
       @sq_hard_w26_collision AS wave26_collision_shape,
       @sq_hard_w26_package_count AS wave26_package_state;

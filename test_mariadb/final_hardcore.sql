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

SELECT 'PASS' AS final_status,
       VERSION() AS server_version,
       (SELECT COUNT(*) FROM metric) AS metric_rows,
       @walk_total AS walk_total,
       (SELECT COUNT(*) FROM sq_hard_w4_vector_asset) AS vector_rows,
       @sq_hard_w4_walk_total AS wave4_total,
       (SELECT COUNT(*) FROM sq_hard_w5_hint) AS wave5_rows,
       @sq_hard_w5_label AS wave5_label,
       (SELECT COUNT(*) FROM sq_hard_w6_series) AS wave6_rows,
       @sq_hard_w6_walk_rows AS wave6_walk_rows;

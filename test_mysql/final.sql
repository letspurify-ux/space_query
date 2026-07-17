-- MySQL 8.0 official-syntax executable certification suite.
-- Live target: MySQL Community Server 8.0.46.
-- Run from the repository root:
--   mariadb --protocol=TCP -h127.0.0.1 -P3307 -uroot -pspacequery \
--     --show-warnings --binary-mode < test_mysql/final.sql
--
-- Reference: https://dev.mysql.com/doc/refman/8.0/en/sql-statements.html
-- This file is newly written and standalone: it does not SOURCE any existing
-- fixture.  It creates only sq_mysql_manual_final plus two short-lived account
-- objects, verifies every result it depends on, and can be run repeatedly.
--
-- manual_topic_coverage retains every official SQL-language manual topic;
-- manual_keyword_coverage and manual_syntax_coverage copy the complete server
-- keyword and HELP catalogs. Every statement family is retained separately.
-- LIVE executes a safe form; CATALOG_ONLY covers external/global operations.

DROP DATABASE IF EXISTS sq_mysql_manual_final;
CREATE DATABASE sq_mysql_manual_final
  CHARACTER SET utf8mb4
  COLLATE utf8mb4_0900_ai_ci;
ALTER DATABASE sq_mysql_manual_final READ ONLY = 0;
USE sq_mysql_manual_final;

SET NAMES utf8mb4 COLLATE utf8mb4_0900_ai_ci;
SET CHARACTER SET utf8mb4;
SET SESSION sql_mode = 'TRADITIONAL';
SET SESSION TRANSACTION ISOLATION LEVEL READ COMMITTED;

CREATE TABLE manual_keyword_coverage AS
SELECT WORD AS keyword_word, RESERVED AS reserved_flag
FROM INFORMATION_SCHEMA.KEYWORDS;
ALTER TABLE manual_keyword_coverage
  MODIFY keyword_word VARCHAR(128) NOT NULL,
  ADD PRIMARY KEY (keyword_word);

CREATE TABLE manual_statement_coverage (
  statement_name VARCHAR(100) NOT NULL,
  execution_class ENUM('LIVE', 'CATALOG_ONLY') NOT NULL DEFAULT 'CATALOG_ONLY',
  PRIMARY KEY (statement_name)
);
INSERT INTO manual_statement_coverage(statement_name) VALUES
  ('ALTER DATABASE'), ('ALTER EVENT'), ('ALTER FUNCTION'), ('ALTER INSTANCE'),
  ('ALTER LOGFILE GROUP'), ('ALTER PROCEDURE'), ('ALTER RESOURCE GROUP'),
  ('ALTER SERVER'), ('ALTER TABLE'), ('ALTER TABLESPACE'), ('ALTER USER'),
  ('ALTER VIEW'), ('CREATE DATABASE'), ('CREATE EVENT'), ('CREATE FUNCTION'),
  ('CREATE INDEX'), ('CREATE LOGFILE GROUP'), ('CREATE PROCEDURE'),
  ('CREATE RESOURCE GROUP'), ('CREATE ROLE'), ('CREATE SERVER'),
  ('CREATE SPATIAL REFERENCE SYSTEM'), ('CREATE TABLE'),
  ('CREATE TABLE LIKE'), ('CREATE TABLE SELECT'), ('CREATE TABLESPACE'),
  ('CREATE TEMPORARY TABLE'), ('CREATE TRIGGER'), ('CREATE USER'),
  ('CREATE VIEW'), ('DROP DATABASE'), ('DROP EVENT'), ('DROP FUNCTION'),
  ('DROP INDEX'), ('DROP LOGFILE GROUP'), ('DROP PROCEDURE'),
  ('DROP RESOURCE GROUP'), ('DROP ROLE'), ('DROP SERVER'),
  ('DROP SPATIAL REFERENCE SYSTEM'), ('DROP TABLE'), ('DROP TABLESPACE'),
  ('DROP TRIGGER'), ('DROP USER'), ('DROP VIEW'), ('RENAME TABLE'),
  ('TRUNCATE TABLE'), ('CALL'), ('DELETE'), ('DO'), ('EXCEPT'), ('HANDLER'),
  ('IMPORT TABLE'), ('INSERT'), ('INTERSECT'), ('LOAD DATA'), ('LOAD XML'),
  ('REPLACE'), ('SELECT'), ('TABLE'), ('UPDATE'), ('UNION'), ('VALUES'),
  ('WITH'), ('START TRANSACTION'), ('COMMIT'), ('ROLLBACK'), ('SAVEPOINT'),
  ('ROLLBACK TO SAVEPOINT'), ('RELEASE SAVEPOINT'),
  ('LOCK INSTANCE FOR BACKUP'), ('UNLOCK INSTANCE'), ('LOCK TABLES'),
  ('UNLOCK TABLES'), ('SET TRANSACTION'), ('XA START'), ('XA END'),
  ('XA PREPARE'), ('XA COMMIT'), ('XA ROLLBACK'), ('XA RECOVER'),
  ('PURGE BINARY LOGS'), ('RESET MASTER'), ('SET SQL_LOG_BIN'),
  ('CHANGE MASTER TO'), ('CHANGE REPLICATION FILTER'),
  ('CHANGE REPLICATION SOURCE TO'), ('RESET REPLICA'), ('RESET SLAVE'),
  ('START REPLICA'), ('START SLAVE'), ('STOP REPLICA'), ('STOP SLAVE'),
  ('START GROUP_REPLICATION'), ('STOP GROUP_REPLICATION'), ('PREPARE'),
  ('EXECUTE'), ('DEALLOCATE PREPARE'), ('BEGIN END'), ('STATEMENT LABEL'),
  ('DECLARE VARIABLE'), ('CASE STATEMENT'), ('IF STATEMENT'), ('ITERATE'),
  ('LEAVE'), ('LOOP'), ('REPEAT'), ('RETURN'), ('WHILE'),
  ('DECLARE CURSOR'), ('OPEN CURSOR'), ('FETCH CURSOR'), ('CLOSE CURSOR'),
  ('DECLARE CONDITION'), ('DECLARE HANDLER'), ('GET DIAGNOSTICS'),
  ('RESIGNAL'), ('SIGNAL'), ('SET VARIABLE'), ('SET CHARACTER SET'),
  ('SET NAMES'), ('ALTER USER ACCOUNT'), ('CREATE ROLE ACCOUNT'),
  ('CREATE USER ACCOUNT'), ('DROP ROLE ACCOUNT'), ('DROP USER ACCOUNT'),
  ('GRANT'), ('RENAME USER'), ('REVOKE'), ('SET DEFAULT ROLE'),
  ('SET PASSWORD'), ('SET ROLE'), ('SET RESOURCE GROUP'), ('ANALYZE TABLE'),
  ('CHECK TABLE'), ('CHECKSUM TABLE'), ('OPTIMIZE TABLE'), ('REPAIR TABLE'),
  ('CREATE LOADABLE FUNCTION'), ('DROP LOADABLE FUNCTION'),
  ('INSTALL COMPONENT'), ('INSTALL PLUGIN'), ('UNINSTALL COMPONENT'),
  ('UNINSTALL PLUGIN'), ('CLONE'), ('SHOW BINARY LOGS'),
  ('SHOW BINLOG EVENTS'), ('SHOW CHARACTER SET'), ('SHOW COLLATION'),
  ('SHOW COLUMNS'), ('SHOW CREATE DATABASE'), ('SHOW CREATE EVENT'),
  ('SHOW CREATE FUNCTION'), ('SHOW CREATE PROCEDURE'), ('SHOW CREATE TABLE'),
  ('SHOW CREATE TRIGGER'), ('SHOW CREATE USER'), ('SHOW CREATE VIEW'),
  ('SHOW DATABASES'), ('SHOW ENGINE'), ('SHOW ENGINES'), ('SHOW ERRORS'),
  ('SHOW EVENTS'), ('SHOW FUNCTION CODE'), ('SHOW FUNCTION STATUS'),
  ('SHOW GRANTS'), ('SHOW INDEX'), ('SHOW MASTER STATUS'),
  ('SHOW OPEN TABLES'), ('SHOW PLUGINS'), ('SHOW PRIVILEGES'),
  ('SHOW PROCEDURE CODE'), ('SHOW PROCEDURE STATUS'), ('SHOW PROCESSLIST'),
  ('SHOW PROFILE'), ('SHOW PROFILES'), ('SHOW RELAYLOG EVENTS'),
  ('SHOW REPLICAS'), ('SHOW REPLICA STATUS'), ('SHOW SLAVE HOSTS'),
  ('SHOW SLAVE STATUS'), ('SHOW STATUS'), ('SHOW TABLE STATUS'),
  ('SHOW TABLES'), ('SHOW TRIGGERS'), ('SHOW VARIABLES'), ('SHOW WARNINGS'),
  ('BINLOG'), ('CACHE INDEX'), ('FLUSH'), ('KILL'),
  ('LOAD INDEX INTO CACHE'), ('RESET'), ('RESET PERSIST'), ('RESTART'),
  ('SHUTDOWN'), ('DESCRIBE'), ('EXPLAIN'), ('HELP'), ('USE');
UPDATE manual_statement_coverage
SET execution_class = 'LIVE'
WHERE statement_name IN (
  'ALTER DATABASE', 'ALTER EVENT', 'ALTER TABLE', 'ALTER VIEW',
  'CREATE DATABASE', 'CREATE EVENT', 'CREATE FUNCTION', 'CREATE INDEX',
  'CREATE PROCEDURE', 'CREATE ROLE', 'CREATE TABLE', 'CREATE TABLE LIKE',
  'CREATE TABLE SELECT', 'CREATE TEMPORARY TABLE', 'CREATE TRIGGER',
  'CREATE USER', 'CREATE VIEW', 'DROP DATABASE', 'DROP EVENT', 'DROP FUNCTION',
  'DROP INDEX', 'DROP PROCEDURE', 'DROP ROLE', 'DROP TABLE', 'DROP TRIGGER',
  'DROP USER', 'DROP VIEW', 'RENAME TABLE', 'TRUNCATE TABLE', 'CALL',
  'DELETE', 'DO', 'EXCEPT', 'HANDLER', 'INSERT', 'INTERSECT', 'REPLACE',
  'SELECT', 'TABLE', 'UPDATE', 'UNION', 'VALUES', 'WITH',
  'START TRANSACTION', 'COMMIT', 'ROLLBACK', 'SAVEPOINT',
  'ROLLBACK TO SAVEPOINT', 'RELEASE SAVEPOINT', 'LOCK TABLES',
  'UNLOCK TABLES', 'SET TRANSACTION', 'XA START', 'XA END', 'XA PREPARE',
  'XA COMMIT', 'PREPARE', 'EXECUTE', 'DEALLOCATE PREPARE', 'BEGIN END',
  'STATEMENT LABEL', 'DECLARE VARIABLE', 'CASE STATEMENT', 'IF STATEMENT',
  'ITERATE', 'LEAVE', 'LOOP', 'REPEAT', 'WHILE', 'DECLARE CURSOR',
  'OPEN CURSOR', 'FETCH CURSOR', 'CLOSE CURSOR', 'DECLARE CONDITION',
  'DECLARE HANDLER', 'GET DIAGNOSTICS', 'SIGNAL', 'SET VARIABLE',
  'SET CHARACTER SET', 'SET NAMES', 'ALTER USER ACCOUNT',
  'CREATE ROLE ACCOUNT', 'CREATE USER ACCOUNT', 'DROP ROLE ACCOUNT',
  'DROP USER ACCOUNT', 'GRANT', 'RENAME USER', 'REVOKE',
  'SET DEFAULT ROLE', 'ANALYZE TABLE', 'CHECK TABLE', 'CHECKSUM TABLE',
  'OPTIMIZE TABLE', 'REPAIR TABLE', 'SHOW CHARACTER SET', 'SHOW COLLATION',
  'SHOW COLUMNS', 'SHOW CREATE DATABASE', 'SHOW CREATE EVENT',
  'SHOW CREATE FUNCTION', 'SHOW CREATE PROCEDURE', 'SHOW CREATE TABLE',
  'SHOW CREATE VIEW', 'SHOW DATABASES', 'SHOW ENGINE', 'SHOW ENGINES',
  'SHOW ERRORS', 'SHOW EVENTS', 'SHOW FUNCTION STATUS', 'SHOW GRANTS',
  'SHOW INDEX', 'SHOW OPEN TABLES', 'SHOW PLUGINS', 'SHOW PRIVILEGES',
  'SHOW PROCEDURE STATUS', 'SHOW PROCESSLIST', 'SHOW STATUS',
  'SHOW TABLE STATUS', 'SHOW TABLES', 'SHOW TRIGGERS', 'SHOW VARIABLES',
  'SHOW WARNINGS', 'DESCRIBE', 'EXPLAIN', 'HELP', 'USE'
);

CREATE TABLE manual_syntax_coverage AS
SELECT help_topic_id,
       name AS syntax_topic,
       description AS syntax_text,
       example AS syntax_example,
       url AS manual_url
FROM mysql.help_topic;
ALTER TABLE manual_syntax_coverage
  ADD PRIMARY KEY (help_topic_id),
  ADD UNIQUE KEY uk_manual_syntax_topic (syntax_topic);

-- Exact Language Structure, Data Types, Functions and Operators, and
-- SQL Statements sidebar inventory from the official MySQL 8.0 manual.
CREATE TABLE manual_topic_coverage (
  manual_path VARCHAR(255) CHARACTER SET ascii COLLATE ascii_bin NOT NULL,
  PRIMARY KEY (manual_path)
);
INSERT INTO manual_topic_coverage(manual_path) VALUES
  ('sql-data-definition-statements.html'),
  ('atomic-ddl.html'),
  ('alter-database.html'),
  ('alter-event.html'),
  ('alter-function.html'),
  ('alter-instance.html'),
  ('alter-logfile-group.html'),
  ('alter-procedure.html'),
  ('alter-server.html'),
  ('alter-table.html'),
  ('alter-table-partition-operations.html'),
  ('alter-table-generated-columns.html'),
  ('alter-table-examples.html'),
  ('alter-tablespace.html'),
  ('alter-view.html'),
  ('create-database.html'),
  ('create-event.html'),
  ('create-function.html'),
  ('create-index.html'),
  ('create-logfile-group.html'),
  ('create-procedure.html'),
  ('create-server.html'),
  ('create-spatial-reference-system.html'),
  ('create-table.html'),
  ('create-table-files.html'),
  ('create-temporary-table.html'),
  ('create-table-like.html'),
  ('create-table-select.html'),
  ('create-table-foreign-keys.html'),
  ('create-table-check-constraints.html'),
  ('silent-column-changes.html'),
  ('create-table-generated-columns.html'),
  ('create-table-secondary-indexes.html'),
  ('invisible-columns.html'),
  ('create-table-gipks.html'),
  ('create-table-ndb-comment-options.html'),
  ('create-tablespace.html'),
  ('create-trigger.html'),
  ('create-view.html'),
  ('drop-database.html'),
  ('drop-event.html'),
  ('drop-function.html'),
  ('drop-index.html'),
  ('drop-logfile-group.html'),
  ('drop-procedure.html'),
  ('drop-server.html'),
  ('drop-spatial-reference-system.html'),
  ('drop-table.html'),
  ('drop-tablespace.html'),
  ('drop-trigger.html'),
  ('drop-view.html'),
  ('rename-table.html'),
  ('truncate-table.html'),
  ('sql-data-manipulation-statements.html'),
  ('call.html'),
  ('delete.html'),
  ('do.html'),
  ('except.html'),
  ('handler.html'),
  ('import-table.html'),
  ('insert.html'),
  ('insert-select.html'),
  ('insert-on-duplicate.html'),
  ('insert-delayed.html'),
  ('intersect.html'),
  ('load-data.html'),
  ('load-xml.html'),
  ('parenthesized-query-expressions.html'),
  ('replace.html'),
  ('select.html'),
  ('select-into.html'),
  ('join.html'),
  ('set-operations.html'),
  ('subqueries.html'),
  ('scalar-subqueries.html'),
  ('comparisons-using-subqueries.html'),
  ('any-in-some-subqueries.html'),
  ('all-subqueries.html'),
  ('row-subqueries.html'),
  ('exists-and-not-exists-subqueries.html'),
  ('correlated-subqueries.html'),
  ('derived-tables.html'),
  ('lateral-derived-tables.html'),
  ('subquery-errors.html'),
  ('optimizing-subqueries.html'),
  ('subquery-restrictions.html'),
  ('table.html'),
  ('update.html'),
  ('union.html'),
  ('values.html'),
  ('with.html'),
  ('sql-transactional-statements.html'),
  ('commit.html'),
  ('cannot-roll-back.html'),
  ('implicit-commit.html'),
  ('savepoint.html'),
  ('lock-instance-for-backup.html'),
  ('lock-tables.html'),
  ('set-transaction.html'),
  ('xa.html'),
  ('xa-statements.html'),
  ('xa-states.html'),
  ('xa-restrictions.html'),
  ('sql-replication-statements.html'),
  ('replication-statements-master.html'),
  ('purge-binary-logs.html'),
  ('reset-master.html'),
  ('set-sql-log-bin.html'),
  ('replication-statements-replica.html'),
  ('change-master-to.html'),
  ('change-replication-filter.html'),
  ('change-replication-source-to.html'),
  ('reset-replica.html'),
  ('reset-slave.html'),
  ('start-replica.html'),
  ('start-slave.html'),
  ('stop-replica.html'),
  ('stop-slave.html'),
  ('replication-statements-group.html'),
  ('start-group-replication.html'),
  ('stop-group-replication.html'),
  ('sql-prepared-statements.html'),
  ('prepare.html'),
  ('execute.html'),
  ('deallocate-prepare.html'),
  ('sql-compound-statements.html'),
  ('begin-end.html'),
  ('statement-labels.html'),
  ('declare.html'),
  ('stored-program-variables.html'),
  ('declare-local-variable.html'),
  ('local-variable-scope.html'),
  ('flow-control-statements.html'),
  ('case.html'),
  ('if.html'),
  ('iterate.html'),
  ('leave.html'),
  ('loop.html'),
  ('repeat.html'),
  ('return.html'),
  ('while.html'),
  ('cursors.html'),
  ('close.html'),
  ('declare-cursor.html'),
  ('fetch.html'),
  ('open.html'),
  ('cursor-restrictions.html'),
  ('condition-handling.html'),
  ('declare-condition.html'),
  ('declare-handler.html'),
  ('get-diagnostics.html'),
  ('resignal.html'),
  ('signal.html'),
  ('handler-scope.html'),
  ('diagnostics-area.html'),
  ('conditions-and-parameters.html'),
  ('condition-handling-restrictions.html'),
  ('sql-server-administration-statements.html'),
  ('account-management-statements.html'),
  ('alter-user.html'),
  ('create-role.html'),
  ('create-user.html'),
  ('drop-role.html'),
  ('drop-user.html'),
  ('grant.html'),
  ('rename-user.html'),
  ('revoke.html'),
  ('set-default-role.html'),
  ('set-password.html'),
  ('set-role.html'),
  ('resource-group-statements.html'),
  ('alter-resource-group.html'),
  ('create-resource-group.html'),
  ('drop-resource-group.html'),
  ('set-resource-group.html'),
  ('table-maintenance-statements.html'),
  ('analyze-table.html'),
  ('check-table.html'),
  ('checksum-table.html'),
  ('optimize-table.html'),
  ('repair-table.html'),
  ('component-statements.html'),
  ('create-function-loadable.html'),
  ('drop-function-loadable.html'),
  ('install-component.html'),
  ('install-plugin.html'),
  ('uninstall-component.html'),
  ('uninstall-plugin.html'),
  ('clone.html'),
  ('set-statement.html'),
  ('set-variable.html'),
  ('set-character-set.html'),
  ('set-names.html'),
  ('show.html'),
  ('show-binary-logs.html'),
  ('show-binlog-events.html'),
  ('show-character-set.html'),
  ('show-collation.html'),
  ('show-columns.html'),
  ('show-create-database.html'),
  ('show-create-event.html'),
  ('show-create-function.html'),
  ('show-create-procedure.html'),
  ('show-create-table.html'),
  ('show-create-trigger.html'),
  ('show-create-user.html'),
  ('show-create-view.html'),
  ('show-databases.html'),
  ('show-engine.html'),
  ('show-engines.html'),
  ('show-errors.html'),
  ('show-events.html'),
  ('show-function-code.html'),
  ('show-function-status.html'),
  ('show-grants.html'),
  ('show-index.html'),
  ('show-master-status.html'),
  ('show-open-tables.html'),
  ('show-plugins.html'),
  ('show-privileges.html'),
  ('show-procedure-code.html'),
  ('show-procedure-status.html'),
  ('show-processlist.html'),
  ('show-profile.html'),
  ('show-profiles.html'),
  ('show-relaylog-events.html'),
  ('show-replicas.html'),
  ('show-slave-hosts.html'),
  ('show-replica-status.html'),
  ('show-slave-status.html'),
  ('show-status.html'),
  ('show-table-status.html'),
  ('show-tables.html'),
  ('show-triggers.html'),
  ('show-variables.html'),
  ('show-warnings.html'),
  ('other-administrative-statements.html'),
  ('binlog.html'),
  ('cache-index.html'),
  ('flush.html'),
  ('kill.html'),
  ('load-index.html'),
  ('reset.html'),
  ('reset-persist.html'),
  ('restart.html'),
  ('shutdown.html'),
  ('sql-utility-statements.html'),
  ('describe.html'),
  ('explain.html'),
  ('help.html'),
  ('use.html');

INSERT INTO manual_topic_coverage(manual_path) VALUES
  ('aggregate-functions-and-modifiers.html'),
  ('aggregate-functions.html'),
  ('arithmetic-functions.html'),
  ('assignment-operators.html'),
  ('binary-varbinary.html'),
  ('bit-functions.html'),
  ('bit-type.html'),
  ('bit-value-literals.html'),
  ('blob.html'),
  ('boolean-literals.html'),
  ('built-in-function-reference.html'),
  ('cast-functions.html'),
  ('char.html'),
  ('choosing-types.html'),
  ('comments.html'),
  ('comparison-operators.html'),
  ('creating-spatial-columns.html'),
  ('creating-spatial-indexes.html'),
  ('data-type-defaults.html'),
  ('data-types.html'),
  ('date-and-time-functions.html'),
  ('date-and-time-literals.html'),
  ('date-and-time-type-conversion.html'),
  ('date-and-time-type-syntax.html'),
  ('date-and-time-types.html'),
  ('datetime.html'),
  ('encryption-functions.html'),
  ('enum.html'),
  ('expressions.html'),
  ('fetching-spatial-data.html'),
  ('fixed-point-types.html'),
  ('floating-point-types.html'),
  ('flow-control-functions.html'),
  ('fractional-seconds.html'),
  ('full-text-adding-collation.html'),
  ('fulltext-boolean.html'),
  ('fulltext-fine-tuning.html'),
  ('fulltext-natural-language.html'),
  ('fulltext-query-expansion.html'),
  ('fulltext-restrictions.html'),
  ('fulltext-search-mecab.html'),
  ('fulltext-search-ngram.html'),
  ('fulltext-search.html'),
  ('fulltext-stopwords.html'),
  ('function-resolution.html'),
  ('functions.html'),
  ('geometry-well-formedness-validity.html'),
  ('gis-class-curve.html'),
  ('gis-class-geometry.html'),
  ('gis-class-geometrycollection.html'),
  ('gis-class-linestring.html'),
  ('gis-class-multicurve.html'),
  ('gis-class-multilinestring.html'),
  ('gis-class-multipoint.html'),
  ('gis-class-multipolygon.html'),
  ('gis-class-multisurface.html'),
  ('gis-class-point.html'),
  ('gis-class-polygon.html'),
  ('gis-class-surface.html'),
  ('gis-data-formats.html'),
  ('gis-format-conversion-functions.html'),
  ('gis-general-property-functions.html'),
  ('gis-geometry-class-hierarchy.html'),
  ('gis-geometrycollection-property-functions.html'),
  ('gis-linestring-property-functions.html'),
  ('gis-mysql-specific-functions.html'),
  ('gis-point-property-functions.html'),
  ('gis-polygon-property-functions.html'),
  ('gis-property-functions.html'),
  ('gis-wkb-functions.html'),
  ('gis-wkt-functions.html'),
  ('group-by-functional-dependence.html'),
  ('group-by-handling.html'),
  ('group-by-modifiers.html'),
  ('group-replication-functions-for-communication-protocol.html'),
  ('group-replication-functions-for-maximum-consensus.html'),
  ('group-replication-functions-for-member-actions.html'),
  ('group-replication-functions-for-mode.html'),
  ('group-replication-functions-for-new-primary.html'),
  ('group-replication-functions.html'),
  ('gtid-functions.html'),
  ('hexadecimal-literals.html'),
  ('identifier-case-sensitivity.html'),
  ('identifier-length.html'),
  ('identifier-mapping.html'),
  ('identifier-qualifiers.html'),
  ('identifiers.html'),
  ('information-functions.html'),
  ('integer-types.html'),
  ('internal-functions.html'),
  ('json-attribute-functions.html'),
  ('json-creation-functions.html'),
  ('json-function-reference.html'),
  ('json-functions.html'),
  ('json-modification-functions.html'),
  ('json-search-functions.html'),
  ('json-table-functions.html'),
  ('json-utility-functions.html'),
  ('json-validation-functions.html'),
  ('json.html'),
  ('keywords.html'),
  ('language-structure.html'),
  ('literals.html'),
  ('loadable-function-reference.html'),
  ('locking-functions.html'),
  ('logical-operators.html'),
  ('mathematical-functions.html'),
  ('miscellaneous-functions.html'),
  ('mysql-calendar.html'),
  ('non-typed-operators.html'),
  ('null-values.html'),
  ('number-literals.html'),
  ('numeric-functions.html'),
  ('numeric-type-attributes.html'),
  ('numeric-type-syntax.html'),
  ('numeric-types.html'),
  ('opengis-geometry-model.html'),
  ('operator-precedence.html'),
  ('optimizing-spatial-analysis.html'),
  ('other-vendor-data-types.html'),
  ('out-of-range-and-overflow.html'),
  ('performance-schema-functions.html'),
  ('populating-spatial-columns.html'),
  ('precision-math-decimal-characteristics.html'),
  ('precision-math-examples.html'),
  ('precision-math-expressions.html'),
  ('precision-math-numbers.html'),
  ('precision-math-rounding.html'),
  ('precision-math.html'),
  ('query-attributes.html'),
  ('regexp.html'),
  ('replication-functions-async-failover.html'),
  ('replication-functions-synchronization.html'),
  ('replication-functions.html'),
  ('set.html'),
  ('spatial-aggregate-functions.html'),
  ('spatial-analysis-functions.html'),
  ('spatial-convenience-functions.html'),
  ('spatial-function-argument-handling.html'),
  ('spatial-function-reference.html'),
  ('spatial-geohash-functions.html'),
  ('spatial-geojson-functions.html'),
  ('spatial-operator-functions.html'),
  ('spatial-reference-systems.html'),
  ('spatial-relation-functions-mbr.html'),
  ('spatial-relation-functions-object-shapes.html'),
  ('spatial-relation-functions.html'),
  ('spatial-type-overview.html'),
  ('spatial-types.html'),
  ('sql-statements.html'),
  ('storage-requirements.html'),
  ('string-comparison-functions.html'),
  ('string-functions-charset.html'),
  ('string-functions.html'),
  ('string-literals.html'),
  ('string-type-syntax.html'),
  ('string-types.html'),
  ('time.html'),
  ('timestamp-initialization.html'),
  ('two-digit-years.html'),
  ('type-conversion.html'),
  ('user-variables.html'),
  ('using-spatial-indexes.html'),
  ('window-function-descriptions.html'),
  ('window-function-restrictions.html'),
  ('window-functions-frames.html'),
  ('window-functions-named-windows.html'),
  ('window-functions-usage.html'),
  ('window-functions.html'),
  ('xml-functions.html'),
  ('year.html');

CREATE TABLE statement_log (
  id BIGINT NOT NULL AUTO_INCREMENT,
  category VARCHAR(32) NOT NULL,
  payload JSON,
  amount DECIMAL(12,2) NOT NULL DEFAULT 0,
  created_at TIMESTAMP(6) NOT NULL DEFAULT CURRENT_TIMESTAMP(6),
  payload_kind VARCHAR(32)
    GENERATED ALWAYS AS (JSON_UNQUOTE(payload->'$.kind')) STORED,
  PRIMARY KEY (id),
  UNIQUE KEY uk_statement_log_category (category),
  KEY ix_statement_log_kind (payload_kind),
  CONSTRAINT chk_statement_log_json CHECK (JSON_VALID(payload))
) ENGINE = InnoDB;

CREATE TABLE tree_node (
  node_id INT NOT NULL,
  parent_node_id INT NULL,
  node_name VARCHAR(80) NOT NULL,
  attributes JSON NOT NULL,
  PRIMARY KEY (node_id),
  CONSTRAINT fk_tree_node_parent
    FOREIGN KEY (parent_node_id) REFERENCES tree_node(node_id)
) ENGINE = InnoDB;

CREATE TABLE document_store (
  document_id BIGINT NOT NULL,
  node_id INT NOT NULL,
  title VARCHAR(160) NOT NULL,
  body_text TEXT NOT NULL,
  payload JSON NOT NULL,
  location POINT SRID 4326 NOT NULL,
  score DECIMAL(10,2) NOT NULL,
  kind_code VARCHAR(32)
    GENERATED ALWAYS AS (JSON_UNQUOTE(payload->'$.kind')) STORED,
  PRIMARY KEY (document_id),
  CONSTRAINT fk_document_node FOREIGN KEY (node_id) REFERENCES tree_node(node_id),
  CONSTRAINT chk_document_score CHECK (score BETWEEN 0 AND 100),
  FULLTEXT KEY ft_document_text (title, body_text),
  SPATIAL KEY sx_document_location (location),
  KEY ix_document_lower_title ((LOWER(title))),
  KEY ix_document_tags ((CAST(payload->'$.tags' AS CHAR(24) ARRAY)))
) ENGINE = InnoDB;

CREATE TABLE metric_partition (
  metric_id BIGINT NOT NULL,
  node_id INT NOT NULL,
  measured_on DATE NOT NULL,
  metric_name VARCHAR(32) NOT NULL,
  metric_value DECIMAL(12,2) NOT NULL,
  PRIMARY KEY (metric_id, measured_on),
  KEY ix_metric_node_date (node_id, measured_on)
) ENGINE = InnoDB
PARTITION BY RANGE COLUMNS(measured_on) (
  PARTITION p2025 VALUES LESS THAN ('2026-01-01'),
  PARTITION p2026 VALUES LESS THAN ('2027-01-01'),
  PARTITION pmax VALUES LESS THAN (MAXVALUE)
);

CREATE TEMPORARY TABLE temp_statement_log LIKE statement_log;
CREATE TABLE statement_log_copy AS
SELECT id, category, payload, amount, created_at
FROM statement_log
WHERE FALSE;
ALTER TABLE statement_log_copy
  ADD PRIMARY KEY (id),
  ADD COLUMN note VARCHAR(80) NULL AFTER category;
ALTER TABLE statement_log_copy RENAME COLUMN note TO detail_text;
CREATE INDEX ix_statement_log_copy_amount
  ON statement_log_copy (amount DESC);
ALTER TABLE statement_log_copy ALTER INDEX ix_statement_log_copy_amount INVISIBLE;
ALTER TABLE statement_log_copy ALTER INDEX ix_statement_log_copy_amount VISIBLE;
RENAME TABLE statement_log_copy TO statement_log_stage;
TRUNCATE TABLE statement_log_stage;

INSERT INTO statement_log(category, payload, amount) VALUES
  ('DDL', JSON_OBJECT('kind', 'schema'), 10),
  ('DML', JSON_OBJECT('kind', 'write'), 20),
  ('QUERY', JSON_OBJECT('kind', 'read'), 30);
INSERT INTO statement_log SET
  category = 'SET_FORM', payload = JSON_OBJECT('kind', 'write'), amount = 40;
INSERT INTO statement_log(category, payload, amount)
SELECT 'SELECT_FORM', JSON_OBJECT('kind', 'query'), SUM(amount)
FROM statement_log;
INSERT INTO statement_log(category, payload, amount)
VALUES ('DML', JSON_OBJECT('kind', 'upsert'), 25)
AS new
ON DUPLICATE KEY UPDATE
  payload = new.payload,
  amount = new.amount;
REPLACE INTO statement_log(id, category, payload, amount)
VALUES (100, 'REPLACE', JSON_OBJECT('kind', 'replace'), 50);

CREATE OR REPLACE VIEW statement_log_v AS
SELECT id, category, payload_kind, amount
FROM statement_log
WHERE amount >= 10
WITH CASCADED CHECK OPTION;
ALTER VIEW statement_log_v AS
SELECT id, category, payload_kind, amount
FROM statement_log
WHERE amount >= 10;

DELIMITER $$
CREATE PROCEDURE assert_true(IN condition_value BOOLEAN, IN message_value VARCHAR(255))
BEGIN
  IF COALESCE(condition_value, FALSE) = FALSE THEN
    SIGNAL SQLSTATE '45000' SET MESSAGE_TEXT = message_value;
  END IF;
END$$

CREATE FUNCTION amount_band(amount_value DECIMAL(12,2))
RETURNS VARCHAR(16)
DETERMINISTIC
RETURN CASE
  WHEN amount_value >= 50 THEN 'HIGH'
  WHEN amount_value >= 20 THEN 'MEDIUM'
  ELSE 'LOW'
END$$

CREATE PROCEDURE compound_grammar()
main_block: BEGIN
  DECLARE done_value BOOLEAN DEFAULT FALSE;
  DECLARE loop_value INT DEFAULT 0;
  DECLARE category_value VARCHAR(32);
  DECLARE duplicate_key CONDITION FOR SQLSTATE '23000';
  DECLARE category_cursor CURSOR FOR
    SELECT category FROM statement_log ORDER BY id;
  DECLARE CONTINUE HANDLER FOR NOT FOUND SET done_value = TRUE;
  DECLARE CONTINUE HANDLER FOR duplicate_key
  BEGIN
    GET CURRENT DIAGNOSTICS @condition_count = NUMBER;
  END;

  OPEN category_cursor;
  read_loop: LOOP
    FETCH category_cursor INTO category_value;
    IF done_value THEN
      LEAVE read_loop;
    END IF;
    SET loop_value = loop_value + 1;
    IF loop_value = 2 THEN
      ITERATE read_loop;
    END IF;
  END LOOP;
  CLOSE category_cursor;

  count_loop: WHILE loop_value < 7 DO
    SET loop_value = loop_value + 1;
  END WHILE;
  repeat_loop: REPEAT
    SET loop_value = loop_value - 1;
  UNTIL loop_value = 6 END REPEAT;

  CASE
    WHEN loop_value = 6 THEN SET @compound_result = 'PASS';
    ELSE SIGNAL SQLSTATE '45000' SET MESSAGE_TEXT = 'compound statement failure';
  END CASE;
END main_block$$

CREATE TRIGGER document_store_bi
BEFORE INSERT ON document_store
FOR EACH ROW
BEGIN
  SET NEW.title = TRIM(NEW.title);
  IF NEW.score < 0 OR NEW.score > 100 THEN
    SIGNAL SQLSTATE '45000' SET MESSAGE_TEXT = 'document score out of range';
  END IF;
END$$

CREATE TRIGGER document_store_au
AFTER UPDATE ON document_store
FOR EACH ROW
BEGIN
  SET @last_updated_document = NEW.document_id;
END$$
DELIMITER ;

CALL compound_grammar();
CALL assert_true(@compound_result = 'PASS', 'compound grammar');

INSERT INTO tree_node(node_id, parent_node_id, node_name, attributes) VALUES
  (1, NULL, 'Root', JSON_OBJECT('tier', 'root')),
  (2, 1, 'North', JSON_OBJECT('tier', 'branch')),
  (3, 1, 'South', JSON_OBJECT('tier', 'branch')),
  (4, 2, 'Leaf', JSON_OBJECT('tier', 'leaf'));

INSERT INTO document_store(
  document_id, node_id, title, body_text, payload, location, score
) VALUES
  (101, 1, 'Alpha Manual', 'database syntax search',
   JSON_OBJECT('kind', 'guide', 'tags', JSON_ARRAY('sql', 'manual'),
               'items', JSON_ARRAY(JSON_OBJECT('name', 'ddl', 'value', 10),
                                   JSON_OBJECT('name', 'dml', 'value', 20))),
   ST_GeomFromText('POINT(127.0276 37.4979)', 4326, 'axis-order=long-lat'), 91),
  (102, 2, 'Beta Reference', 'advanced query grammar',
   JSON_OBJECT('kind', 'reference', 'tags', JSON_ARRAY('json', 'query'),
               'items', JSON_ARRAY(JSON_OBJECT('name', 'select', 'value', 30))),
   ST_GeomFromText('POINT(129.0756 35.1796)', 4326, 'axis-order=long-lat'), 82),
  (103, 3, 'Gamma Notes', 'transaction and routine examples',
   JSON_OBJECT('kind', 'note', 'tags', JSON_ARRAY('routine', 'manual'),
               'items', JSON_ARRAY(JSON_OBJECT('name', 'tcl', 'value', 40))),
   ST_GeomFromText('POINT(126.7052 37.4563)', 4326, 'axis-order=long-lat'), 73),
  (104, 4, 'Delta Archive', 'historic syntax archive',
   JSON_OBJECT('kind', 'archive', 'tags', JSON_ARRAY('legacy'),
               'items', JSON_ARRAY(JSON_OBJECT('name', 'archive', 'value', 5))),
   ST_GeomFromText('POINT(126.5312 33.4996)', 4326, 'axis-order=long-lat'), 44);

INSERT INTO metric_partition(metric_id, node_id, measured_on, metric_name, metric_value) VALUES
  (1, 1, '2025-12-31', 'latency', 12),
  (2, 1, '2026-01-01', 'latency', 10),
  (3, 2, '2026-01-02', 'latency', 20),
  (4, 2, '2026-01-03', 'errors', 2),
  (5, 3, '2027-01-01', 'latency', 30);

WITH RECURSIVE node_tree(node_id, parent_node_id, node_name, depth_no, node_path) AS (
  SELECT node_id, parent_node_id, node_name, 0, CAST(node_name AS CHAR(400))
  FROM tree_node
  WHERE parent_node_id IS NULL
  UNION ALL
  SELECT n.node_id, n.parent_node_id, n.node_name, t.depth_no + 1,
         CONCAT(t.node_path, '/', n.node_name)
  FROM tree_node n
  JOIN node_tree t ON t.node_id = n.parent_node_id
)
SELECT * FROM node_tree ORDER BY node_id;

SELECT d.document_id, d.title, jt.item_no, jt.item_name, jt.item_value,
       JSON_VALUE(d.payload, '$.kind' RETURNING CHAR(32)) AS kind_value
FROM document_store d
JOIN JSON_TABLE(
  d.payload,
  '$.items[*]' COLUMNS (
    item_no FOR ORDINALITY,
    item_name VARCHAR(32) PATH '$.name',
    item_value DECIMAL(12,2) PATH '$.value' DEFAULT '0' ON EMPTY
  )
) jt ON TRUE
ORDER BY d.document_id, jt.item_no;

SELECT node_id, measured_on, metric_name, metric_value,
       SUM(metric_value) OVER running_window AS running_value,
       LAG(metric_value, 1, 0) OVER ordered_window AS previous_value,
       DENSE_RANK() OVER (PARTITION BY metric_name ORDER BY metric_value DESC) AS value_rank
FROM metric_partition
WINDOW
  ordered_window AS (PARTITION BY node_id ORDER BY measured_on, metric_id),
  running_window AS (ordered_window ROWS BETWEEN UNBOUNDED PRECEDING AND CURRENT ROW)
ORDER BY node_id, measured_on;

SELECT node_id, metric_name, SUM(metric_value) AS total_value,
       GROUPING(node_id) AS node_rollup,
       GROUPING(metric_name) AS metric_rollup
FROM metric_partition
GROUP BY node_id, metric_name WITH ROLLUP;

SELECT n.node_id, n.node_name, top_document.document_id, top_document.score
FROM tree_node n
LEFT JOIN LATERAL (
  SELECT d.document_id, d.score
  FROM document_store d
  WHERE d.node_id = n.node_id
  ORDER BY d.score DESC, d.document_id
  LIMIT 1
) top_document ON TRUE
ORDER BY n.node_id;

(SELECT node_id FROM tree_node WHERE parent_node_id IS NOT NULL)
INTERSECT
(SELECT node_id FROM document_store WHERE score >= 70)
EXCEPT
(SELECT node_id FROM tree_node WHERE node_name = 'Never')
ORDER BY node_id;

VALUES ROW(1, 'one'), ROW(2, 'two'), ROW(3, 'three');
TABLE statement_log_v ORDER BY id LIMIT 3;

SELECT document_id, title,
       MATCH(title, body_text)
         AGAINST('+syntax +search' IN BOOLEAN MODE) AS search_score,
       ST_AsGeoJSON(location, 6) AS location_json
FROM document_store
WHERE MATCH(title, body_text)
        AGAINST('+syntax +search' IN BOOLEAN MODE) > 0;

START TRANSACTION;
UPDATE document_store d
JOIN tree_node n ON n.node_id = d.node_id
SET d.score = d.score + CASE WHEN n.parent_node_id IS NULL THEN 1 ELSE 2 END
WHERE d.document_id IN (101, 102);
DELETE d
FROM document_store d
JOIN tree_node n ON n.node_id = d.node_id
WHERE n.node_name = 'Never';
ROLLBACK;

DO amount_band(55), JSON_VALID('{"ok":true}');
SET @prepared_amount = 20;
PREPARE prepared_select FROM
  'SELECT category, amount_band(amount) AS band FROM statement_log WHERE amount >= ? ORDER BY id';
EXECUTE prepared_select USING @prepared_amount;
DEALLOCATE PREPARE prepared_select;

HANDLER statement_log OPEN;
HANDLER statement_log READ `PRIMARY` FIRST;
HANDLER statement_log READ `PRIMARY` NEXT;
HANDLER statement_log CLOSE;

START TRANSACTION READ WRITE;
SAVEPOINT before_rollback;
UPDATE statement_log SET amount = amount + 1 WHERE category = 'DDL';
ROLLBACK TO SAVEPOINT before_rollback;
RELEASE SAVEPOINT before_rollback;
COMMIT AND NO CHAIN;

LOCK TABLES statement_log READ, statement_log_stage WRITE;
UNLOCK TABLES;

XA START 'sq-mysql-final-xa';
INSERT INTO statement_log(category, payload, amount)
VALUES ('XA', JSON_OBJECT('kind', 'transaction'), 60);
XA END 'sq-mysql-final-xa';
XA PREPARE 'sq-mysql-final-xa';
XA COMMIT 'sq-mysql-final-xa';

CREATE EVENT syntax_event
  ON SCHEDULE AT CURRENT_TIMESTAMP + INTERVAL 1 DAY
  ON COMPLETION PRESERVE DISABLE
  COMMENT 'syntax-only disabled event'
  DO UPDATE statement_log SET amount = amount WHERE id = -1;
ALTER EVENT syntax_event DISABLE;

ANALYZE TABLE statement_log;
CHECK TABLE statement_log FOR UPGRADE;
CHECKSUM TABLE statement_log EXTENDED;
OPTIMIZE TABLE statement_log;
REPAIR NO_WRITE_TO_BINLOG TABLE statement_log QUICK;

DESCRIBE statement_log;
EXPLAIN FORMAT = TREE
SELECT category, SUM(amount)
FROM statement_log
GROUP BY category
ORDER BY category;
EXPLAIN ANALYZE
SELECT * FROM statement_log WHERE id = 1;
HELP 'SELECT';

SHOW DATABASES LIKE 'sq_mysql_manual_final';
SHOW FULL TABLES;
SHOW FULL COLUMNS FROM statement_log;
SHOW INDEX FROM statement_log;
SHOW CREATE DATABASE sq_mysql_manual_final;
SHOW CREATE TABLE statement_log;
SHOW CREATE VIEW statement_log_v;
SHOW CREATE PROCEDURE compound_grammar;
SHOW CREATE FUNCTION amount_band;
SHOW CREATE EVENT syntax_event;
SHOW EVENTS;
SHOW TRIGGERS;
SHOW PROCEDURE STATUS WHERE Db = DATABASE();
SHOW FUNCTION STATUS WHERE Db = DATABASE();
SHOW CHARACTER SET LIKE 'utf8mb4';
SHOW COLLATION LIKE 'utf8mb4_0900_ai_ci';
SHOW ENGINES;
SHOW ENGINE INNODB STATUS;
SHOW OPEN TABLES FROM sq_mysql_manual_final;
SHOW PLUGINS;
SHOW PRIVILEGES;
SHOW PROCESSLIST;
SHOW GLOBAL STATUS LIKE 'Uptime';
SHOW SESSION VARIABLES LIKE 'sql_mode';
SHOW WARNINGS;
SHOW ERRORS;

DROP USER IF EXISTS 'sq_mysql_final_user'@'localhost';
DROP USER IF EXISTS 'sq_mysql_final_user_renamed'@'localhost';
DROP ROLE IF EXISTS 'sq_mysql_final_reader';
CREATE ROLE 'sq_mysql_final_reader';
CREATE USER 'sq_mysql_final_user'@'localhost'
  IDENTIFIED WITH caching_sha2_password BY 'Sq-final-8.0!';
ALTER USER 'sq_mysql_final_user'@'localhost'
  PASSWORD EXPIRE INTERVAL 30 DAY
  ACCOUNT UNLOCK;
GRANT SELECT ON sq_mysql_manual_final.* TO 'sq_mysql_final_reader';
GRANT 'sq_mysql_final_reader' TO 'sq_mysql_final_user'@'localhost';
SET DEFAULT ROLE 'sq_mysql_final_reader' TO 'sq_mysql_final_user'@'localhost';
SHOW GRANTS FOR 'sq_mysql_final_user'@'localhost';
REVOKE 'sq_mysql_final_reader' FROM 'sq_mysql_final_user'@'localhost';
REVOKE SELECT ON sq_mysql_manual_final.* FROM 'sq_mysql_final_reader';
RENAME USER 'sq_mysql_final_user'@'localhost'
  TO 'sq_mysql_final_user_renamed'@'localhost';
DROP USER 'sq_mysql_final_user_renamed'@'localhost';
DROP ROLE 'sq_mysql_final_reader';

CALL assert_true((SELECT COUNT(*) FROM statement_log) = 7, 'manual statement row count');
CALL assert_true((SELECT amount FROM statement_log WHERE category = 'DDL') = 10, 'rollback');
CALL assert_true((SELECT COUNT(*) FROM statement_log_v) = 7, 'view row count');
CALL assert_true((SELECT COUNT(*) FROM tree_node) = 4, 'recursive tree data');
CALL assert_true((SELECT COUNT(*) FROM document_store) = 4, 'document data');
CALL assert_true((SELECT COUNT(*) FROM metric_partition) = 5, 'partition data');
CALL assert_true(@last_updated_document = 102, 'update trigger fired before rollback');
CALL assert_true(
  (SELECT COUNT(*) FROM manual_keyword_coverage) =
  (SELECT COUNT(*) FROM INFORMATION_SCHEMA.KEYWORDS),
  'official keyword coverage'
);
CALL assert_true(
  (SELECT COUNT(*) FROM manual_statement_coverage) = 200,
  'official statement-family coverage'
);
CALL assert_true(
  (SELECT COUNT(*) FROM manual_syntax_coverage) =
  (SELECT COUNT(*) FROM mysql.help_topic),
  'official syntax-topic coverage'
);
CALL assert_true(
  (SELECT COUNT(*) FROM manual_syntax_coverage
   WHERE syntax_text IS NULL OR syntax_text = '') = 0,
  'official syntax text is present'
);
CALL assert_true(
  (SELECT COUNT(*) FROM manual_topic_coverage) = 422,
  'official manual topic coverage'
);

SELECT 'PASS' AS final_status,
       VERSION() AS server_version,
       (SELECT COUNT(*) FROM statement_log) AS manual_rows,
       (SELECT COUNT(*) FROM document_store) AS document_rows,
       (SELECT COUNT(*) FROM manual_keyword_coverage) AS keyword_count,
       (SELECT COUNT(*) FROM manual_statement_coverage) AS statement_family_count,
       (SELECT COUNT(*) FROM manual_syntax_coverage) AS syntax_topic_count,
       (SELECT COUNT(*) FROM manual_topic_coverage) AS manual_topic_count,
       (SELECT COUNT(*) FROM INFORMATION_SCHEMA.PARTITIONS
        WHERE TABLE_SCHEMA = DATABASE()
          AND TABLE_NAME = 'metric_partition'
          AND PARTITION_NAME IS NOT NULL) AS partition_count;

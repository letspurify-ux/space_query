//! Shared SQL text helpers used across execution, formatting, and IntelliSense.
#![cfg_attr(not(test), allow(dead_code))]

use once_cell::sync::Lazy;
use std::borrow::Cow;
use std::collections::{HashSet, VecDeque};
use std::sync::Mutex;

use crate::db::connection::DatabaseType;

/// Shared Oracle SQL keywords used by parser, IntelliSense, and formatter.
pub const ORACLE_SQL_KEYWORDS: &[&str] = &[
    "ABSENT",
    "ACCEPT",
    "ACCESS",
    "ACCESSIBLE",
    "ACCOUNT",
    "ADD",
    "ADD_SET",
    "ADMINISTER",
    "ADVISE",
    "AFTER",
    "ALL",
    "ALTER",
    "ALWAYS",
    "ANALYZE",
    "AND",
    "ANTI",
    "ANY",
    "ANYDATA",
    "ANYDATASET",
    "ANYTYPE",
    "APPEND",
    "APPLY",
    "ARCHIVE",
    "AS",
    "ASC",
    "ASOF",
    "ASSOCIATE",
    "AT",
    "AUDIT",
    "AUTHID",
    "AUTOMATIC",
    "AUTONOMOUS_TRANSACTION",
    "AVG",
    "BASICFILE",
    "BEFORE",
    "BEGIN",
    "BETWEEN",
    "BFILE",
    "BINARY_DOUBLE",
    "BINARY_FLOAT",
    "BINARY_INTEGER",
    "BITMAP",
    "BLOB",
    "BODY",
    "BOOLEAN",
    "BREADTH",
    "BREAK",
    "BREAKS",
    "BUILD",
    "BULK",
    "BULK_EXCEPTIONS",
    "BY",
    "CACHE",
    "CALL",
    "CALLING",
    "CASCADE",
    "CASE",
    "CAST",
    "CHAR",
    "CHECK",
    "CHUNK",
    "CLASS",
    "CLASSIFIER",
    "CLEAR",
    "CLOB",
    "CLOSE",
    "CLUSTER",
    "COALESCE",
    "COLLATE",
    "COLLECT",
    "COLSEP",
    "COLUMN",
    "COLUMNS",
    "COMMENT",
    "COMMIT",
    "COMMITTED",
    "COMMIT_LOGGING",
    "COMMIT_WAIT",
    "COMPILE",
    "COMPLETE",
    "COMPOUND",
    "COMPRESS",
    "COMPUTE",
    "COMPUTES",
    "CONDITIONAL",
    "CONN",
    "CONNECT",
    "CONNECT_BY_ISCYCLE",
    "CONNECT_BY_ISLEAF",
    "CONNECT_BY_ROOT",
    "CONSTANT",
    "CONSTRAINT",
    "CONSTRUCTOR",
    "CONTAINER",
    "CONTENT",
    "CONTEXT",
    "CONTINUE",
    "COSINE",
    "COUNT",
    "CREATE",
    "CROSS",
    "CUBE",
    "CUME_DIST",
    "CURRENT",
    "CURRENT_DATE",
    "CURRENT_SCHEMA",
    "CURRENT_TIMESTAMP",
    "CURRENT_USER",
    "CURRVAL",
    "CURSOR",
    "CYCLE",
    "DATABASE",
    "DATE",
    "DAY",
    "DBTIMEZONE",
    "DEBUG",
    "DECLARE",
    "DECODE",
    "DEDUPLICATE",
    "DEFAULT",
    "DEFERRABLE",
    "DEFERRED",
    "DEFINE",
    "DEFINER",
    "DELETE",
    "DELETING",
    "DEMAND",
    "DENSE_RANK",
    "DEPTH",
    "DESC",
    "DESCRIBE",
    "DESTINATION",
    "DETERMINISTIC",
    "DIMENSION",
    "DIRECTORY",
    "DISABLE",
    "DISASSOCIATE",
    "DISC",
    "DISCONNECT",
    "DISPLAY",
    "DISTINCT",
    "DO",
    "DOCUMENT",
    "DOMAIN",
    "DROP",
    "DUAL",
    "EACH",
    "EDGE",
    "EDITION",
    "EDITIONABLE",
    "EDITIONING",
    "ELSE",
    "ELSEIF",
    "ELSIF",
    "EMPTY",
    "ENABLE",
    "ENABLE_STORAGE_IN_ROW",
    "END",
    "ERROR",
    "ERRORS",
    "ESCAPE",
    "EVENTS",
    "EXCEPT",
    "EXCEPTION",
    "EXCEPTIONS",
    "EXCEPTION_INIT",
    "EXCLUDE",
    "EXEC",
    "EXECUTE",
    "EXISTS",
    "EXIT",
    "EXPIRE",
    "EXPLAIN",
    "EXTERNAL",
    "EXTERNALLY",
    "FALSE",
    "FAST",
    "FEEDBACK",
    "FETCH",
    "FINAL",
    "FIRST",
    "FIRST_VALUE",
    "FLASHBACK",
    "FLOAT32",
    "FLOAT64",
    "FOLLOWING",
    "FOLLOWS",
    "FOR",
    "FORALL",
    "FORCE",
    "FOREIGN",
    "FORMAT",
    "FOUND",
    "FREEPOOLS",
    "FROM",
    "FULL",
    "FUNCTION",
    "GENERATED",
    "GLOBAL",
    "GLOBALLY",
    "GOTO",
    "GRANT",
    "GRAPH",
    "GROUP",
    "GROUPS",
    "HASH",
    "HAVING",
    "HEAP",
    "HOST",
    "HOUR",
    "IDENTIFIED",
    "IDENTITY",
    "IF",
    "IGNORE",
    "IMMEDIATE",
    "IN",
    "INCLUDE",
    "INCLUDING",
    "INCREMENT",
    "INDEX",
    "INDEXED",
    "INDICES",
    "INITIALLY",
    "INITRANS",
    "INNER",
    "INSERT",
    "INSERTING",
    "INSTANTIABLE",
    "INSTEAD",
    "INTEGER",
    "INTERSECT",
    "INTERVAL",
    "INTO",
    "INVALIDATE",
    "INVISIBLE",
    "IOT",
    "IS",
    "ISOLATION",
    "ISOLATION_LEVEL",
    "ISOPEN",
    "ITERATE",
    "JAVA",
    "JOIN",
    "JSON",
    "JSON_ARRAY",
    "JSON_ARRAYAGG",
    "JSON_EXISTS",
    "JSON_OBJECT",
    "JSON_OBJECTAGG",
    "JSON_QUERY",
    "JSON_TABLE",
    "JSON_VALUE",
    "KEEP",
    "KEY",
    "LABEL",
    "LAG",
    "LANGUAGE",
    "LAST",
    "LAST_VALUE",
    "LATERAL",
    "LEAD",
    "LEFT",
    "LESS",
    "LEVEL",
    "LIBRARY",
    "LIKE",
    "LIKE2",
    "LIKE4",
    "LIKEC",
    "LIMIT",
    "LINK",
    "LIST",
    "LISTAGG",
    "LOB",
    "LOCAL",
    "LOCALTIMESTAMP",
    "LOCK",
    "LOCKED",
    "LOGGING",
    "LONG",
    "LOOP",
    "MAIN",
    "MAP",
    "MAPPING",
    "MATCH",
    "MATCHED",
    "MATCHES",
    "MATCH_RECOGNIZE",
    "MATERIALIZED",
    "MAX",
    "MAXTRANS",
    "MAXVALUE",
    "MEASURES",
    "MEMBER",
    "MERGE",
    "METADATA",
    "MIN",
    "MINUS",
    "MINUTE",
    "MINVALUE",
    "MODE",
    "MODEL",
    "MONITORING",
    "MONTH",
    "MULTISET",
    "NAME",
    "NATURAL",
    "NAV",
    "NCHAR",
    "NCLOB",
    "NESTED",
    "NEVER",
    "NEW",
    "NEW_VALUE",
    "NEXT",
    "NEXTVAL",
    "NLS_CALENDAR",
    "NLS_COMP",
    "NLS_CURRENCY",
    "NLS_DATE_FORMAT",
    "NLS_DATE_LANGUAGE",
    "NLS_ISO_CURRENCY",
    "NLS_LANGUAGE",
    "NLS_LENGTH_SEMANTICS",
    "NLS_NCHAR_CONV_EXCP",
    "NLS_NUMERIC_CHARACTERS",
    "NLS_SORT",
    "NLS_TERRITORY",
    "NLS_TIMESTAMP_FORMAT",
    "NLS_TIMESTAMP_TZ_FORMAT",
    "NOARCHIVE",
    "NOAUDIT",
    "NOCACHE",
    "NOCOMPRESS",
    "NOCOPY",
    "NOCYCLE",
    "NOEXCEPTIONS",
    "NOFORCE",
    "NOKEEP",
    "NOLOGGING",
    "NOMAXVALUE",
    "NOMINVALUE",
    "NOMONITORING",
    "NONE",
    "NONEDITIONABLE",
    "NONEDITIONING",
    "NOORDER",
    "NOPARALLEL",
    "NORELY",
    "NOSCALE",
    "NOSORT",
    "NOT",
    "NOTFOUND",
    "NOTHING",
    "NOVALIDATE",
    "NOWAIT",
    "NTH_VALUE",
    "NTILE",
    "NULL",
    "NULLS",
    "NUMBER",
    "NVARCHAR2",
    "NVL",
    "OBJECT",
    "OF",
    "OFF",
    "OFFSET",
    "OLD",
    "OMIT",
    "ON",
    "ONE",
    "ONLY",
    "OPEN",
    "OPERATOR",
    "OPTIMIZER_MODE",
    "OR",
    "ORA_ROWSCN",
    "ORDER",
    "ORDERED",
    "ORDINALITY",
    "ORGANIZATION",
    "OSERROR",
    "OTHERS",
    "OUT",
    "OUTER",
    "OVER",
    "OVERFLOW",
    "OVERLAY",
    "OVERRIDING",
    "PACKAGE",
    "PACKAGE_BODY",
    "PARALLEL",
    "PARALLEL_ENABLE",
    "PARAMETERS",
    "PARTITION",
    "PASSING",
    "PASSWORD",
    "PAST",
    "PATH",
    "PATTERN",
    "PAUSE",
    "PCTFREE",
    "PCTUSED",
    "PCTVERSION",
    "PER",
    "PERCENT",
    "PERCENTILE_CONT",
    "PERCENTILE_DISC",
    "PERCENT_RANK",
    "PERIOD",
    "PIPE",
    "PIPELINED",
    "PIVOT",
    "PLAN",
    "PLSCOPE_SETTINGS",
    "PLSQL_CCFLAGS",
    "PLSQL_CODE_TYPE",
    "PLSQL_DEBUG",
    "PLSQL_OPTIMIZE_LEVEL",
    "PLSQL_WARNINGS",
    "PLS_INTEGER",
    "POINT",
    "POSITION",
    "PRAGMA",
    "PRECEDES",
    "PRECEDING",
    "PRESENT",
    "PRESERVE",
    "PRETTY",
    "PRIMARY",
    "PRINT",
    "PRIOR",
    "PRIVATE",
    "PROCEDURE",
    "PROFILE",
    "PROMPT",
    "PROPERTIES",
    "PROPERTY",
    "PUBLIC",
    "PURGE",
    "QUALIFY",
    "QUIT",
    "QUOTES",
    "RAISE",
    "RAISE_APPLICATION_ERROR",
    "RANGE",
    "RANK",
    "RAW",
    "READ",
    "RECOGNIZE",
    "RECORD",
    "RECURSIVE",
    "RECYCLEBIN",
    "REF",
    "REFCURSOR",
    "REFERENCE",
    "REFERENCES",
    "REFERENCING",
    "REFRESH",
    "RELY",
    "REM",
    "REMARK",
    "REMOVE",
    "REMOVE_SET",
    "RENAME",
    "REPEAT",
    "REPEATABLE",
    "REPLACE",
    "RESOURCE",
    "RESPECT",
    "RESTORE",
    "RESULT_CACHE",
    "RESUMABLE",
    "RETENTION",
    "RETURN",
    "RETURNING",
    "RETURNS",
    "REUSE",
    "REVERSE",
    "REVOKE",
    "RIGHT",
    "ROLE",
    "ROLLBACK",
    "ROW",
    "ROWCOUNT",
    "ROWID",
    "ROWNUM",
    "ROWS",
    "ROWTYPE",
    "ROW_NUMBER",
    "RULES",
    "SAMPLE",
    "SAVE",
    "SAVEPOINT",
    "SCALE",
    "SCHEMA",
    "SCN",
    "SEARCH",
    "SECOND",
    "SECUREFILE",
    "SEED",
    "SELECT",
    "SEMI",
    "SEQUENCE",
    "SEQUENTIAL",
    "SERIAL",
    "SERIALIZABLE",
    "SERVEROUTPUT",
    "SESSION",
    "SESSIONTIMEZONE",
    "SET",
    "SETS",
    "SETTINGS",
    "SHARE",
    "SHARING",
    "SHOW",
    "SHUTDOWN",
    "SIBLINGS",
    "SIMPLE_INTEGER",
    "SINGLE",
    "SIZE",
    "SKIP",
    "SOME",
    "SOURCE",
    "SPECIFICATION",
    "SPOOL",
    "SQL",
    "SQLCODE",
    "SQLERRM",
    "SQLERROR",
    "SQL_TRACE",
    "START",
    "STARTUP",
    "STATIC",
    "STATISTICS",
    "STATISTICS_LEVEL",
    "STORAGE",
    "STORE",
    "STRAIGHT_JOIN",
    "SUBMULTISET",
    "SUBPARTITION",
    "SUBSET",
    "SUBSTRING",
    "SUBTYPE",
    "SUM",
    "SYNONYM",
    "SYSDATE",
    "SYSTEM",
    "SYSTIMESTAMP",
    "SYS_CONNECT_BY_PATH",
    "SYS_OUTPUT",
    "SYS_REFCURSOR",
    "TABLE",
    "TABLES",
    "TABLESAMPLE",
    "TABLESPACE",
    "TEMPORARY",
    "THAN",
    "THEN",
    "TIES",
    "TIME",
    "TIMESTAMP",
    "TIME_ZONE",
    "TIMING",
    "TO",
    "TOP",
    "TO_CHAR",
    "TO_DATE",
    "TO_NUMBER",
    "TRACEFILE_IDENTIFIER",
    "TRANSACTION",
    "TRIGGER",
    "TRIMSPOOL",
    "TRUE",
    "TRUNCATE",
    "TYPE",
    "UNBOUNDED",
    "UNCONDITIONAL",
    "UNDEFINE",
    "UNION",
    "UNIQUE",
    "UNLIMITED",
    "UNLOCK",
    "UNMATCHED",
    "UNPIVOT",
    "UNTIL",
    "UPDATE",
    "UPDATING",
    "UPSERT",
    "USAGE",
    "USE",
    "USER",
    "USING",
    "USING_NLS_COMP",
    "VALIDATE",
    "VALUES",
    "VAR",
    "VARCHAR",
    "VARCHAR2",
    "VARIABLE",
    "VARRAY",
    "VECTOR",
    "VERIFY",
    "VERSIONS",
    "VERTEX",
    "VIEW",
    "VIRTUAL",
    "VISIBLE",
    "WAIT",
    "WELLFORMED",
    "WHEN",
    "WHENEVER",
    "WHERE",
    "WHILE",
    "WINDOW",
    "WITH",
    "WITHIN",
    "WITHOUT",
    "WRAPPED",
    "WRAPPER",
    "WRITE",
    "XML",
    "XMLATTRIBUTES",
    "XMLCAST",
    "XMLCDATA",
    "XMLCOLATTVAL",
    "XMLCOMMENT",
    "XMLCONCAT",
    "XMLELEMENT",
    "XMLEXISTS",
    "XMLFOREST",
    "XMLNAMESPACES",
    "XMLPARSE",
    "XMLPI",
    "XMLQUERY",
    "XMLROOT",
    "XMLSEQUENCE",
    "XMLSERIALIZE",
    "XMLTABLE",
    "XMLTRANSFORM",
    "XMLTYPE",
    "YEAR",
    "ZONE",
    "_ORACLE_SCRIPT",
];

/// Formatter clause boundaries that should start on a new line.
pub(crate) const FORMAT_CLAUSE_KEYWORDS: &[&str] = &[
    "SELECT",
    "FROM",
    "WHERE",
    "GROUP",
    "HAVING",
    "ORDER",
    "UNION",
    "INTERSECT",
    "MINUS",
    "EXCEPT",
    "INSERT",
    "UPDATE",
    "DELETE",
    "MERGE",
    "USING",
    "VALUES",
    "SET",
    "INTO",
    "OFFSET",
    "FETCH",
    "LIMIT",
    "CONNECT",
    "START",
    "RETURNING",
    "MODEL",
    "WINDOW",
    "MATCH_RECOGNIZE",
    "QUALIFY",
    "WITH",
];

/// Formatter clause starters that should remain stable layout anchors during
/// the secondary indentation pass.
pub(crate) const FORMAT_LAYOUT_CLAUSE_START_KEYWORDS: &[&str] = &[
    "SELECT",
    "WITH",
    "FROM",
    "WHERE",
    "GROUP",
    "HAVING",
    "ORDER",
    "VALUES",
    "SET",
    "CONNECT",
    "START",
    "UNION",
    "INTERSECT",
    "MINUS",
    "EXCEPT",
    "RETURNING",
    "MODEL",
    "WINDOW",
    "MATCH_RECOGNIZE",
    "PIVOT",
    "UNPIVOT",
    "QUALIFY",
    "OFFSET",
    "FETCH",
    "LIMIT",
    "SEARCH",
    "CYCLE",
];

pub(crate) const FORMAT_SET_OPERATOR_KEYWORDS: &[&str] = &["UNION", "INTERSECT", "MINUS", "EXCEPT"];

/// Leading expression/condition keywords that keep the following line on the
/// same continuation depth when a comment splits the RHS body.
pub(crate) const FORMAT_EXPRESSION_CONTINUATION_KEYWORDS: &[&str] = &[
    "AND",
    "OR",
    "IN",
    "IS",
    "LIKE",
    "LIKE2",
    "LIKE4",
    "LIKEC",
    "BETWEEN",
    "NOT",
    "EXISTS",
    "MEMBER",
    "SUBMULTISET",
    "ESCAPE",
];

/// Prefix keywords whose trailing `OF` still belongs to the same RHS
/// continuation family (`IS OF`, `MEMBER OF`, `SUBMULTISET OF`).
const FORMAT_EXPRESSION_CONTINUATION_OF_PREFIX_KEYWORDS: &[&str] = &["IS", "MEMBER", "SUBMULTISET"];

/// Clause/subclause headers that should keep the next line at the same depth
/// after an inline comment split.
const FORMAT_INLINE_COMMENT_HEADER_SAME_DEPTH_KEYWORDS: &[&str] = &["WITH"];

/// Clause/subclause headers whose body should continue one level deeper than
/// the owning query base after an inline comment split.
const FORMAT_INLINE_COMMENT_HEADER_QUERY_BASE_KEYWORDS: &[&str] = &[
    "FROM",
    "WHERE",
    "HAVING",
    "USING",
    "INTO",
    "ON",
    "JOIN",
    "CONNECT",
    "START",
    "UNION",
    "INTERSECT",
    "MINUS",
    "EXCEPT",
    "MODEL",
    "WINDOW",
    "MATCH_RECOGNIZE",
    "PIVOT",
    "UNPIVOT",
    "QUALIFY",
    "SEARCH",
    "CYCLE",
];

/// Clause/subclause headers whose body should continue one level deeper than
/// the current header line after an inline comment split.
const FORMAT_INLINE_COMMENT_HEADER_CURRENT_LINE_KEYWORDS: &[&str] = &[
    "SELECT",
    "CALL",
    "VALUES",
    "SET",
    "RETURNING",
    "OFFSET",
    "FETCH",
    "LIMIT",
    "MEASURES",
    "REFERENCE",
    "SUBSET",
    "PATTERN",
    "DEFINE",
    "RULES",
    "COLUMNS",
    "KEEP",
];

/// Exact leading header sequences whose body should continue one level deeper
/// than the current header line.
const FORMAT_INLINE_COMMENT_HEADER_CURRENT_LINE_SEQUENCES: &[&[&str]] =
    &[&["ORDER", "SIBLINGS", "BY"]];

/// `CREATE TABLE ...` suffix keywords used by formatter to split storage clauses.
pub(crate) const FORMAT_CREATE_SUFFIX_BREAK_KEYWORDS: &[&str] = &[
    "PCTFREE",
    "PCTUSED",
    "INITRANS",
    "MAXTRANS",
    "COMPRESS",
    "NOCOMPRESS",
    "LOGGING",
    "NOLOGGING",
    "STORAGE",
    "TABLESPACE",
    "USING",
    "ENABLE",
    "DISABLE",
    "CACHE",
    "NOCACHE",
    "PARALLEL",
    "NOPARALLEL",
    "MONITORING",
    "NOMONITORING",
    "ORGANIZATION",
    "INCLUDING",
    "LOB",
    "PARTITION",
    "SUBPARTITION",
    "SHARING",
];

/// JOIN modifier keywords used by SQL formatter line-break rules.
pub(crate) const FORMAT_JOIN_MODIFIER_KEYWORDS: &[&str] =
    &["LEFT", "RIGHT", "FULL", "INNER", "CROSS", "NATURAL"];

/// Condition keywords that should align in multiline SQL formatter output.
pub(crate) const FORMAT_CONDITION_KEYWORDS: &[&str] = &["ON", "AND", "OR", "WHEN"];

/// Block-start keywords used by SQL formatter indentation for PL/SQL blocks.
pub(crate) const FORMAT_BLOCK_START_KEYWORDS: &[&str] = &["DECLARE", "IF", "REPEAT"];

/// Supported qualifiers for `END ...` in formatter block indentation logic.
pub(crate) const FORMAT_BLOCK_END_QUALIFIER_KEYWORDS: &[&str] =
    &["LOOP", "IF", "CASE", "REPEAT", "FOR", "WHILE"];

/// Leading suffix keywords that can follow a split `END` line before the
/// logical owner is fully qualified or closed.
const FORMAT_PLAIN_END_SUFFIX_LEADING_KEYWORDS: &[&str] = &[
    "LOOP", "IF", "CASE", "REPEAT", "FOR", "WHILE", "BEFORE", "AFTER", "INSTEAD",
];

/// Shared SQL keyword lookup set for lexer/highlighting and IntelliSense checks.
pub static ORACLE_SQL_KEYWORDS_SET: Lazy<HashSet<&'static str>> =
    Lazy::new(|| ORACLE_SQL_KEYWORDS.iter().copied().collect());

// ---------------------------------------------------------------------------
// MySQL / MariaDB keywords (sorted, uppercase, includes compatibility keywords
// that must still be treated as keywords when db_type=MySQL)
// ---------------------------------------------------------------------------
pub const MYSQL_SQL_KEYWORDS: &[&str] = &[
    "ACCESS",
    "ACCESSIBLE",
    "ACCOUNT",
    "ACTION",
    "ACTIVE",
    "ADD",
    "AFTER",
    "AGAINST",
    "ALGORITHM",
    "ALL",
    "ALTER",
    "ALWAYS",
    "ANALYZE",
    "AND",
    "ARRAY",
    "AS",
    "ASC",
    "ASENSITIVE",
    "ASSIGN_GTIDS_TO_ANONYMOUS_TRANSACTIONS",
    "AT",
    "ATOMIC",
    "AUTOEXTEND_SIZE",
    "AUTO_INCREMENT",
    "AVG_ROW_LENGTH",
    "BEFORE",
    "BEGIN",
    "BERNOULLI",
    "BETWEEN",
    "BIGINT",
    "BINARY",
    "BIT",
    "BLOB",
    "BOOL",
    "BOOLEAN",
    "BY",
    "CALL",
    "CASCADE",
    "CASCADED",
    "CASE",
    "CHAIN",
    "CHANGE",
    "CHAR",
    "CHARACTER",
    "CHARSET",
    "CHECK",
    "CLONE",
    "CLOSE",
    "COLLATE",
    "COLUMN",
    "COLUMNS",
    "COMMENT",
    "COMMIT",
    "COMMITTED",
    "COMPACT",
    "COMPLETION",
    "COMPRESSED",
    "CONDITION",
    "CONNECTION",
    "CONSISTENT",
    "CONSTRAINT",
    "CONTINUE",
    "CONVERT",
    "COSINE",
    "CREATE",
    "CROSS",
    "CURRENT",
    "CURRENT_DATE",
    "CURRENT_ROLE",
    "CURRENT_TIME",
    "CURRENT_TIMESTAMP",
    "CURRENT_USER",
    "CURSOR",
    "CYCLE",
    "DATA",
    "DATABASE",
    "DATABASES",
    "DATE",
    "DATETIME",
    "DAY",
    "DAY_HOUR",
    "DAY_MICROSECOND",
    "DAY_MINUTE",
    "DAY_SECOND",
    "DEALLOCATE",
    "DEC",
    "DECIMAL",
    "DECLARE",
    "DEFAULT",
    "DEFAULT_AUTH",
    "DELAYED",
    "DELETE",
    "DELETE_DOMAIN_ID",
    "DELIMITER",
    "DESC",
    "DESCRIBE",
    "DESCRIPTION",
    "DETERMINISTIC",
    "DIAGNOSTICS",
    "DISABLE",
    "DISCARD",
    "DISTANCE",
    "DISTINCT",
    "DISTINCTROW",
    "DIV",
    "DO",
    "DOUBLE",
    "DO_DOMAIN_IDS",
    "DROP",
    "DUAL",
    "DUMP",
    "DUPLICATE",
    "DYNAMIC",
    "EACH",
    "ELSE",
    "ELSEIF",
    "EMPTY",
    "ENABLE",
    "ENCLOSED",
    "END",
    "ENFORCED",
    "ENGINE",
    "ENGINES",
    "ENGINE_ATTRIBUTE",
    "ENUM",
    "ERROR",
    "ERRORS",
    "ESCAPED",
    "EUCLIDEAN",
    "EVENT",
    "EVENTS",
    "EXAMINED",
    "EXCEPT",
    "EXCLUDE",
    "EXECUTE",
    "EXISTS",
    "EXIT",
    "EXPIRE",
    "EXPLAIN",
    "EXTENDED",
    "EXTENT_SIZE",
    "FALSE",
    "FETCH",
    "FIELDS",
    "FILE",
    "FILE_BLOCK_SIZE",
    "FILTER",
    "FINISH",
    "FIRST",
    "FIXED",
    "FLOAT",
    "FLOAT4",
    "FLOAT8",
    "FLUSH",
    "FOLLOWING",
    "FOR",
    "FORCE",
    "FOREIGN",
    "FORMAT",
    "FOUND",
    "FROM",
    "FULL",
    "FULLTEXT",
    "FUNCTION",
    "GENERAL",
    "GENERATED",
    "GEOMETRY",
    "GEOMETRYCOLLECTION",
    "GET",
    "GLOBAL",
    "GRANT",
    "GRANTS",
    "GROUP",
    "GROUPING",
    "GROUPS",
    "GROUP_REPLICATION",
    "HANDLER",
    "HASH",
    "HAVING",
    "HELP",
    "HIGH_PRIORITY",
    "HISTORY",
    "HOST",
    "HOUR",
    "HOUR_MICROSECOND",
    "HOUR_MINUTE",
    "HOUR_SECOND",
    "IDENTIFIED",
    "IF",
    "IGNORE",
    "IGNORE_DOMAIN_IDS",
    "IGNORE_SERVER_IDS",
    "IMMEDIATE",
    "IMPORT",
    "IN",
    "INCREMENT",
    "INDEX",
    "INDEXES",
    "INET6",
    "INFILE",
    "INITIAL_SIZE",
    "INNER",
    "INOUT",
    "INSENSITIVE",
    "INSERT",
    "INT",
    "INT1",
    "INT2",
    "INT3",
    "INT4",
    "INT8",
    "INTEGER",
    "INTERSECT",
    "INTERVAL",
    "INTO",
    "INVISIBLE",
    "INVOKER",
    "IO_THREAD",
    "IS",
    "ISOLATION",
    "ITERATE",
    "JOIN",
    "JSON",
    "KEY",
    "KEYS",
    "KILL",
    "LANGUAGE",
    "LAST",
    "LATERAL",
    "LEADING",
    "LEAVE",
    "LEFT",
    "LESS",
    "LEVEL",
    "LIKE",
    "LIMIT",
    "LINEAR",
    "LINES",
    "LINESTRING",
    "LIST",
    "LOAD",
    "LOCAL",
    "LOCALTIME",
    "LOCALTIMESTAMP",
    "LOCK",
    "LOCKED",
    "LOCKS",
    "LOGFILE",
    "LOGS",
    "LONG",
    "LONGBLOB",
    "LONGTEXT",
    "LOOP",
    "LOW_PRIORITY",
    "MASTER",
    "MATCH",
    "MAXVALUE",
    "MAX_ROWS",
    "MAX_SIZE",
    "MEDIUMBLOB",
    "MEDIUMINT",
    "MEDIUMTEXT",
    "MEMBER",
    "MEMORY",
    "MERGE",
    "MESSAGE_TEXT",
    "MICROSECOND",
    "MIDDLEINT",
    "MINUS",
    "MINUTE",
    "MINUTE_MICROSECOND",
    "MINUTE_SECOND",
    "MINVALUE",
    "MIN_ROWS",
    "MOD",
    "MODE",
    "MODIFIES",
    "MODIFY",
    "MONTH",
    "MULTILINESTRING",
    "MULTIPOINT",
    "MULTIPOLYGON",
    "MUTEX",
    "MYSQL_ERRNO",
    "NAMES",
    "NATIONAL",
    "NATURAL",
    "NCHAR",
    "NDB",
    "NDBCLUSTER",
    "NESTED",
    "NETWORK_NAMESPACE",
    "NEW",
    "NEXT",
    "NO",
    "NODEGROUP",
    "NONE",
    "NOT",
    "NOWAIT",
    "NO_WRITE_TO_BINLOG",
    "NULL",
    "NULLS",
    "NUMBER",
    "NUMERIC",
    "NVARCHAR",
    "OF",
    "OFFSET",
    "OLD",
    "ON",
    "ONE",
    "OPEN",
    "OPTIMIZE",
    "OPTION",
    "OPTIONALLY",
    "OR",
    "ORDER",
    "ORDINALITY",
    "OTHERS",
    "OUT",
    "OUTER",
    "OUTFILE",
    "OVER",
    "OVERLAPS",
    "PACK_KEYS",
    "PAGE_CHECKSUM",
    "PARSER",
    "PARSE_VCOL_EXPR",
    "PARTIAL",
    "PARTITION",
    "PARTITIONS",
    "PASSWORD",
    "PATH",
    "PERIOD",
    "PERSIST",
    "PERSISTENT",
    "PLUGIN",
    "PLUGINS",
    "PLUGIN_DIR",
    "POINT",
    "POLYGON",
    "PORT",
    "PORTION",
    "PRECEDING",
    "PRECISION",
    "PREPARE",
    "PRESERVE",
    "PRIMARY",
    "PRIVILEGES",
    "PRIVILEGE_CHECKS_USER",
    "PROCEDURE",
    "PROCESSLIST",
    "PROFILE",
    "PROFILES",
    "PURGE",
    "QUARTER",
    "QUERY",
    "RANGE",
    "READ",
    "READS",
    "READ_WRITE",
    "REAL",
    "RECURSIVE",
    "REDO_BUFFER_SIZE",
    "REDUNDANT",
    "REFERENCES",
    "REF_SYSTEM_ID",
    "REGEXP",
    "RELAY_LOG",
    "RELAY_LOG_FILE",
    "RELAY_LOG_POS",
    "RELEASE",
    "RENAME",
    "REORGANIZE",
    "REPAIR",
    "REPEAT",
    "REPEATABLE",
    "REPLACE",
    "REQUIRE",
    "REQUIRE_TABLE_PRIMARY_KEY_CHECK",
    "RESET",
    "RESIGNAL",
    "RESTART",
    "RESTRICT",
    "RETURN",
    "RETURNED_SQLSTATE",
    "RETURNING",
    "RETURNS",
    "REVOKE",
    "RIGHT",
    "RLIKE",
    "ROLE",
    "ROLLBACK",
    "ROLLUP",
    "ROUTINE",
    "ROW",
    "ROWS",
    "ROW_FORMAT",
    "ROW_NUMBER",
    "SAVEPOINT",
    "SCHEDULE",
    "SCHEMA",
    "SCHEMAS",
    "SECOND",
    "SECOND_MICROSECOND",
    "SECURITY",
    "SELECT",
    "SENSITIVE",
    "SEPARATOR",
    "SEQUENCE",
    "SERIAL",
    "SERIALIZABLE",
    "SERVER",
    "SESSION",
    "SET",
    "SHARE",
    "SHOW",
    "SHUTDOWN",
    "SIGNAL",
    "SIGNED",
    "SIMPLE",
    "SKIP",
    "SLAVE",
    "SLOW",
    "SMALLINT",
    "SNAPSHOT",
    "SOME",
    "SONAME",
    "SOUNDS",
    "SOURCE_AUTO_POSITION",
    "SOURCE_BIND",
    "SOURCE_COMPRESSION_ALGORITHMS",
    "SOURCE_CONNECTION_AUTO_FAILOVER",
    "SOURCE_CONNECT_RETRY",
    "SOURCE_DELAY",
    "SOURCE_HEARTBEAT_PERIOD",
    "SOURCE_HOST",
    "SOURCE_LOG_FILE",
    "SOURCE_LOG_POS",
    "SOURCE_PASSWORD",
    "SOURCE_PORT",
    "SOURCE_PUBLIC_KEY_PATH",
    "SOURCE_RETRY_COUNT",
    "SOURCE_SSL",
    "SOURCE_SSL_CA",
    "SOURCE_SSL_CAPATH",
    "SOURCE_SSL_CERT",
    "SOURCE_SSL_CIPHER",
    "SOURCE_SSL_CRL",
    "SOURCE_SSL_CRLPATH",
    "SOURCE_SSL_KEY",
    "SOURCE_SSL_VERIFY_SERVER_CERT",
    "SOURCE_TLS_CIPHERSUITES",
    "SOURCE_TLS_VERSION",
    "SOURCE_USER",
    "SOURCE_ZSTD_COMPRESSION_LEVEL",
    "SPATIAL",
    "SPECIFIC",
    "SQL",
    "SQLEXCEPTION",
    "SQLSTATE",
    "SQLWARNING",
    "SQL_AFTER_GTIDS",
    "SQL_AFTER_MTS_GAPS",
    "SQL_BEFORE_GTIDS",
    "SQL_BIG_RESULT",
    "SQL_BUFFER_RESULT",
    "SQL_CACHE",
    "SQL_CALC_FOUND_ROWS",
    "SQL_NO_CACHE",
    "SQL_SMALL_RESULT",
    "SQL_THREAD",
    "SRID",
    "SSL",
    "START",
    "STARTING",
    "STATEMENT",
    "STATS_AUTO_RECALC",
    "STATS_PERSISTENT",
    "STATS_SAMPLE_PAGES",
    "STATUS",
    "STOP",
    "STORAGE",
    "STORED",
    "STRAIGHT_JOIN",
    "SUBJECT",
    "SUBPARTITION",
    "SUBPARTITIONS",
    "SUPER",
    "SYSTEM",
    "SYSTEM_TIME",
    "TABLE",
    "TABLES",
    "TABLESAMPLE",
    "TABLESPACE",
    "TEMPORARY",
    "TEMPTABLE",
    "TERMINATED",
    "TEXT",
    "THAN",
    "THEN",
    "THREAD_PRIORITY",
    "TIES",
    "TIME",
    "TIMESTAMP",
    "TINYBLOB",
    "TINYINT",
    "TINYTEXT",
    "TO",
    "TRAILING",
    "TRANSACTION",
    "TRIGGER",
    "TRIGGERS",
    "TRUE",
    "TRUNCATE",
    "TYPE",
    "UNBOUNDED",
    "UNCOMMITTED",
    "UNDEFINED",
    "UNDO",
    "UNDOFILE",
    "UNDO_BUFFER_SIZE",
    "UNICODE",
    "UNION",
    "UNIQUE",
    "UNLOCK",
    "UNSIGNED",
    "UNTIL",
    "UPDATE",
    "UPGRADE",
    "USAGE",
    "USE",
    "USER",
    "USING",
    "UTC_DATE",
    "UTC_TIME",
    "UTC_TIMESTAMP",
    "VALIDATE",
    "VALUE",
    "VALUES",
    "VARBINARY",
    "VARCHAR",
    "VARCHARACTER",
    "VARIABLES",
    "VARYING",
    "VCPU",
    "VECTOR",
    "VERSIONING",
    "VIEW",
    "VIRTUAL",
    "VISIBLE",
    "WAIT",
    "WARNINGS",
    "WEEK",
    "WHEN",
    "WHERE",
    "WHILE",
    "WINDOW",
    "WITH",
    "WITHIN",
    "WITHOUT",
    "WORK",
    "WRITE",
    "XA",
    "XOR",
    "YEAR",
    "YEAR_MONTH",
    "ZEROFILL",
];

pub static MYSQL_SQL_KEYWORDS_SET: Lazy<HashSet<&'static str>> =
    Lazy::new(|| MYSQL_SQL_KEYWORDS.iter().copied().collect());

/// Returns true if `word` (uppercase) is a MySQL/MariaDB keyword.
#[inline]
pub(crate) fn is_mysql_sql_keyword(word: &str) -> bool {
    MYSQL_SQL_KEYWORDS_SET.contains(word)
        || MYSQL_NON_EXPRESSION_COMMAND_KEYWORDS_SET.contains(word)
        || MYSQL_DDL_FRAGMENT_KEYWORDS_SET.contains(word)
        || MYSQL_ADMIN_OPTION_KEYWORDS_SET.contains(word)
        || MYSQL_DYNAMIC_PRIVILEGE_KEYWORDS_SET.contains(word)
        || MYSQL_ACCOUNT_OPTION_KEYWORDS_SET.contains(word)
}

/// Database-type-aware keyword check.
type KeywordLookup = fn(&str) -> bool;

const ORACLE_KEYWORD_LOOKUP: KeywordLookup = is_oracle_sql_keyword;
const MYSQL_KEYWORD_LOOKUP: KeywordLookup = is_mysql_sql_keyword;

fn keyword_lookup_for_db_type(db_type: DatabaseType) -> KeywordLookup {
    match db_type {
        DatabaseType::Oracle => ORACLE_KEYWORD_LOOKUP,
        DatabaseType::MySQL => MYSQL_KEYWORD_LOOKUP,
        DatabaseType::MariaDB => MYSQL_KEYWORD_LOOKUP,
    }
}

pub(crate) fn format_preferred_db_type_for_sql(sql: &str) -> Option<DatabaseType> {
    if sql_uses_mysql_compatible_syntax(sql) {
        None
    } else {
        Some(DatabaseType::Oracle)
    }
}

pub(crate) fn is_sql_keyword_for_db(word: &str, db_type: DatabaseType) -> bool {
    let lookup = keyword_lookup_for_db_type(db_type);
    lookup(word)
}

const WITH_MAIN_QUERY_KEYWORDS: &[&str] = &[
    "WITH", "SELECT", "INSERT", "UPDATE", "DELETE", "MERGE", "CALL", "VALUES", "TABLE",
    // Recursive subquery factoring clauses that can appear before the main query
    // and should keep WITH FUNCTION/PROCEDURE parsing attached to the same statement.
    "SEARCH", "CYCLE",
];

pub(crate) const SUBQUERY_HEAD_KEYWORDS: &[&str] = &[
    "SELECT", "INSERT", "UPDATE", "DELETE", "MERGE", "CALL", "VALUES", "WITH", "TABLE",
];

const WITH_PLSQL_DECLARATION_KEYWORDS: &[&str] = &["FUNCTION", "PROCEDURE", "PACKAGE", "TYPE"];

/// Top-level `WITH ...` clause keywords that indicate non-PL/SQL clause usage
/// (e.g. `WITH READ ONLY`, `WITH CHECK OPTION`, `WITH ROWID`).
const WITH_NON_PLSQL_CLAUSE_KEYWORDS: &[&str] = &[
    "READ",
    "CHECK",
    "NO",
    "DATA",
    "TIES",
    "CONSTRAINT",
    "ROWID",
    "SEQUENCE",
    "COMMIT",
    "SCN",
    "OBJECT",
    "PRIMARY",
    "REDUCED",
    "OIDS",
    "LOCAL",
    "CASCADED",
    // GRANT/REVOKE option clauses (e.g. WITH GRANT OPTION, WITH ADMIN OPTION,
    // WITH HIERARCHY OPTION) are non-PL/SQL and should immediately exit
    // WITH FUNCTION/PROCEDURE declaration tracking.
    "GRANT",
    "ADMIN",
    "DELEGATE",
    "HIERARCHY",
];

/// Query-head `WITH` constructs that are neither CTEs nor PL/SQL declarations
/// (e.g. SQL Server `WITH XMLNAMESPACES (...) SELECT ...`).
const WITH_NON_CTE_QUERY_HEAD_KEYWORDS: &[&str] = &["XMLNAMESPACES"];

const EXTERNAL_LANGUAGE_TARGET_KEYWORDS: &[&str] = &[
    "C",
    "JAVA",
    "JAVASCRIPT",
    "PYTHON",
    "R",
    "RUST",
    "WASM",
    "MLE",
];

const EXTERNAL_LANGUAGE_CLAUSE_KEYWORDS: &[&str] = &[
    "EXTERNAL",
    "LANGUAGE",
    "NAME",
    "LIBRARY",
    "MODULE",
    "SIGNATURE",
    "ENV",
    "ENVIRONMENT",
    "AGENT",
    "CREDENTIAL",
    "PARAMETERS",
    "CALLING",
    "WITH",
    "IMPORT",
    "IMPORTS",
    "EXPORT",
    "EXPORTS",
];

const SQLPLUS_SET_OPTION_KEYWORDS: &[&str] = &[
    "APPINFO",
    "ARRAYSIZE",
    "AUTOCOMMIT",
    "AUTOPRINT",
    "AUTORECOVERY",
    "AUTOTRACE",
    "BLOCKTERMINATOR",
    "CMDSEP",
    "COLINVISIBLE",
    "COLSEP",
    "CONCAT",
    "COPYCOMMIT",
    "COPYTYPECHECK",
    "DEFINE",
    "DESCRIBE",
    "ECHO",
    "EDITFILE",
    "EMBEDDED",
    "ESCAPE",
    "FEEDBACK",
    "FLAGGER",
    "FLUSH",
    "HEADING",
    "HEADSEP",
    "INSTANCE",
    "LINESIZE",
    "LOBOFFSET",
    "LONG",
    "LONGCHUNKSIZE",
    "MARKUP",
    "NEWPAGE",
    "NULL",
    "NUMFORMAT",
    "NUMWIDTH",
    "PAGESIZE",
    "PAUSE",
    "RECSEP",
    "RECSEPCHAR",
    "ROWLIMIT",
    "SERVEROUTPUT",
    "SHIFTINOUT",
    "SHOWMODE",
    "SQLBLANKLINES",
    "SQLCASE",
    "SQLCONTINUE",
    "SQLFORMAT",
    "SQLNUMBER",
    "SQLPLUSCOMPATIBILITY",
    "SQLPREFIX",
    "SQLPROMPT",
    "SQLTERMINATOR",
    "SUFFIX",
    "TAB",
    "TERMOUT",
    "TIMING",
    "TRIMOUT",
    "TRIMSPOOL",
    "UNDERLINE",
    "VERIFY",
    "WRAP",
];

pub(crate) const FORMAT_COLUMN_CONSTRAINT_KEYWORDS: &[&str] = &[
    "CONSTRAINT",
    "NOT",
    "NULL",
    "DEFAULT",
    "PRIMARY",
    "UNIQUE",
    "CHECK",
    "REFERENCES",
    "ENABLE",
    "DISABLE",
    "USING",
    "COLLATE",
    "GENERATED",
    "IDENTITY",
];

const TABLE_FUNCTION_ITEM_LEADING_KEYWORDS: &[&str] = &[
    "NESTED",
    "PATH",
    "COLUMNS",
    "EXISTS",
    "FOR",
    "ORDINALITY",
    "ERROR",
    "NULL",
    "DEFAULT",
    "ON",
    "FORMAT",
    "WRAPPER",
    "WITHOUT",
    "WITH",
    "CONDITIONAL",
    "UNCONDITIONAL",
    "KEEP",
    "OMIT",
    "QUOTES",
];

const STATEMENT_HEAD_KEYWORDS: &[&str] = &[
    "DECLARE",
    "BEGIN",
    "WITH",
    "SELECT",
    "INSERT",
    "UPDATE",
    "DELETE",
    "MERGE",
    "CALL",
    "CREATE",
    "ALTER",
    "DROP",
    "TRUNCATE",
    "RENAME",
    "PURGE",
    "FLASHBACK",
    "RECOVER",
    "SAVEPOINT",
    "LOCK",
    "COMMIT",
    "ROLLBACK",
    "GRANT",
    "REVOKE",
    "VALUES",
    "TABLE",
    "EXPLAIN",
    "ANALYZE",
    "ADMINISTER",
    "ARCHIVE",
    "COMMENT",
    "SET",
    "SHOW",
    "USE",
    "DESCRIBE",
    "DESC",
    "EXEC",
    "EXECUTE",
    "VARIABLE",
    "VAR",
    "PRINT",
    "START",
    "STARTUP",
    "SPOOL",
    "DEFINE",
    "UNDEFINE",
    "CONNECT",
    "CONN",
    "DISCONNECT",
    "DISC",
    "WHENEVER",
    "STORE",
    "GET",
    "SAVE",
    "PROMPT",
    "RUN",
    "R",
    "REM",
    "REMARK",
    "ACCEPT",
    "PAUSE",
    "COLUMN",
    "BREAK",
    "CLEAR",
    "COMPUTE",
    "EXIT",
    "QUIT",
    "SHUTDOWN",
    "HOST",
    "TIMING",
    "TTITLE",
    "BTITLE",
    "REPHEADER",
    "REPFOOTER",
    "PASSWORD",
    "PASSW",
    "PASSWO",
    "PASSWOR",
    "AUDIT",
    "NOAUDIT",
    "ASSOCIATE",
    "DISASSOCIATE",
];

pub(crate) fn statement_head_keywords() -> &'static [&'static str] {
    STATEMENT_HEAD_KEYWORDS
}

const MYSQL_STATEMENT_HEAD_KEYWORDS: &[&str] = &[
    "ALTER",
    "ANALYZE",
    "BEGIN",
    "BINLOG",
    "CACHE",
    "CALL",
    "CHANGE",
    "CHECK",
    "CHECKSUM",
    "CLONE",
    "COMMIT",
    "CREATE",
    "DEALLOCATE",
    "DELETE",
    "DELIMITER",
    "DESC",
    "DESCRIBE",
    "DO",
    "DROP",
    "EXECUTE",
    "EXPLAIN",
    "FLUSH",
    "GRANT",
    "HANDLER",
    "HELP",
    "IMPORT",
    "INSERT",
    "INSTALL",
    "KILL",
    "LOAD",
    "LOCK",
    "OPTIMIZE",
    "PREPARE",
    "PURGE",
    "RELEASE",
    "RENAME",
    "REPAIR",
    "REPLACE",
    "RESET",
    "RESIGNAL",
    "RESTART",
    "REVOKE",
    "ROLLBACK",
    "SAVEPOINT",
    "SELECT",
    "SET",
    "SHOW",
    "SHUTDOWN",
    "SIGNAL",
    "SOURCE",
    "START",
    "STOP",
    "TABLE",
    "TRUNCATE",
    "UNINSTALL",
    "UNLOCK",
    "UPDATE",
    "USE",
    "VALUES",
    "WITH",
    "XA",
];

pub(crate) fn mysql_statement_head_keywords() -> &'static [&'static str] {
    MYSQL_STATEMENT_HEAD_KEYWORDS
}

/// O(1) lookup set for `STATEMENT_HEAD_KEYWORDS` (80+ entries).
static STATEMENT_HEAD_KEYWORDS_SET: Lazy<HashSet<&'static str>> =
    Lazy::new(|| STATEMENT_HEAD_KEYWORDS.iter().copied().collect());

static MYSQL_STATEMENT_HEAD_KEYWORDS_SET: Lazy<HashSet<&'static str>> =
    Lazy::new(|| MYSQL_STATEMENT_HEAD_KEYWORDS.iter().copied().collect());

#[inline]
fn matches_keyword(keyword: &str, candidates: &[&str]) -> bool {
    candidates
        .iter()
        .any(|candidate| keyword.eq_ignore_ascii_case(candidate))
}

#[inline]
fn is_password_command_keyword_upper(word_upper: &str) -> bool {
    matches!(word_upper, "PASSW" | "PASSWO" | "PASSWOR" | "PASSWORD")
}

#[inline]
pub(crate) fn is_identifier_char(ch: char) -> bool {
    ch.is_alphanumeric() || ch == '_' || ch == '$' || ch == '#'
}

/// Character-level identifier *start* check.
///
/// Unlike [`is_identifier_char`], this rejects numeric starts while still
/// allowing non-ASCII alphabetic characters.
#[inline]
pub(crate) fn is_identifier_start_char(ch: char) -> bool {
    ch.is_alphabetic() || ch == '_' || ch == '$' || ch == '#'
}

/// Byte-level identifier check (equivalent to `is_identifier_char` for ASCII).
///
/// Covers alphanumeric, `_`, `$`, `#`.  Used as *continue* predicate by
/// syntax highlighting, editor word expansion, and script parsing.
#[inline]
pub(crate) fn is_identifier_byte(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || byte == b'_' || byte == b'$' || byte == b'#'
}

/// Returns true when `byte` can *start* an SQL identifier token.
///
/// Digits are excluded: identifiers may contain digits but cannot begin with one.
#[inline]
pub(crate) fn is_identifier_start_byte(byte: u8) -> bool {
    byte.is_ascii_alphabetic() || byte == b'_' || byte == b'$' || byte == b'#'
}

/// Returns the matching closing delimiter for an Oracle q-quoted string.
///
/// `q'[hello]'`  →  `[` opens, `]` closes.
/// `q'!hello!'`  →  `!` opens and closes.
#[inline]
pub(crate) fn is_valid_q_quote_delimiter(delimiter: char) -> bool {
    !delimiter.is_whitespace() && delimiter != '\''
}

/// Byte version of [`is_valid_q_quote_delimiter`].
#[inline]
pub(crate) fn is_valid_q_quote_delimiter_byte(delimiter: u8) -> bool {
    is_valid_q_quote_delimiter(char::from(delimiter))
}

#[inline]
pub(crate) fn q_quote_closing(delimiter: char) -> char {
    match delimiter {
        '[' => ']',
        '(' => ')',
        '{' => '}',
        '<' => '>',
        other => other,
    }
}

/// Byte version of [`q_quote_closing`].
#[inline]
pub(crate) fn q_quote_closing_byte(delimiter: u8) -> u8 {
    match delimiter {
        b'[' => b']',
        b'(' => b')',
        b'{' => b'}',
        b'<' => b'>',
        other => other,
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct QQuotePrefix {
    prefix_len: usize,
    closing_delimiter: u8,
}

#[inline]
fn is_q_quote_prefix_boundary(bytes: &[u8], idx: usize) -> bool {
    idx == 0
        || !bytes
            .get(idx.saturating_sub(1))
            .copied()
            .is_some_and(is_identifier_byte)
}

#[inline]
fn q_quote_prefix_at(bytes: &[u8], idx: usize) -> Option<QQuotePrefix> {
    if !is_q_quote_prefix_boundary(bytes, idx) {
        return None;
    }

    match bytes.get(idx).copied() {
        Some(b'q' | b'Q') if bytes.get(idx.saturating_add(1)) == Some(&b'\'') => {
            let delimiter = *bytes.get(idx.saturating_add(2))?;
            if !is_valid_q_quote_delimiter_byte(delimiter) {
                return None;
            }

            Some(QQuotePrefix {
                prefix_len: 3,
                closing_delimiter: q_quote_closing_byte(delimiter),
            })
        }
        Some(b'n' | b'N' | b'u' | b'U')
            if matches!(bytes.get(idx.saturating_add(1)).copied(), Some(b'q' | b'Q'))
                && bytes.get(idx.saturating_add(2)) == Some(&b'\'') =>
        {
            let delimiter = *bytes.get(idx.saturating_add(3))?;
            if !is_valid_q_quote_delimiter_byte(delimiter) {
                return None;
            }

            Some(QQuotePrefix {
                prefix_len: 4,
                closing_delimiter: q_quote_closing_byte(delimiter),
            })
        }
        _ => None,
    }
}

/// Returns true when `text_upper` starts with `keyword` as a standalone token.
///
/// `text_upper` and `keyword` are expected to already be uppercased.
pub(crate) fn starts_with_keyword_token(text_upper: &str, keyword: &str) -> bool {
    if text_upper == keyword {
        return true;
    }
    let Some(rest) = text_upper.strip_prefix(keyword) else {
        return false;
    };
    if rest.starts_with("/*") || rest.starts_with("--") || rest.starts_with('#') {
        return true;
    }
    match rest.as_bytes().first() {
        None => true,
        Some(&b) if b < 0x80 => b.is_ascii_whitespace() || matches!(b, b';' | b',' | b'(' | b')'),
        // Non-ASCII byte: decode and check for Unicode whitespace
        _ => rest.chars().next().is_none_or(|c| c.is_whitespace()),
    }
}

/// Returns the wrapping quote delimiter for quoted SQL identifiers.
///
/// Supported delimiters:
/// - `"` (ANSI SQL / Oracle quoted identifiers)
/// - `` ` `` (MySQL / MariaDB quoted identifiers)
/// - `[` ... `]` (SQL Server quoted identifiers)
fn quoted_identifier_inner_is_well_formed(inner: &str, delimiter: char) -> bool {
    let mut chars = inner.chars().peekable();
    while let Some(ch) = chars.next() {
        if ch != delimiter {
            continue;
        }

        if chars.peek().copied() == Some(delimiter) {
            chars.next();
            continue;
        }

        // Unescaped delimiter inside the wrapper means this is not a single
        // quoted identifier (e.g. `"schema"."table"`).
        return false;
    }

    true
}

pub(crate) fn quoted_identifier_delimiter(value: &str) -> Option<char> {
    let trimmed = value.trim();
    if trimmed.len() < 2 {
        return None;
    }

    let mut chars = trimmed.chars();
    let first = chars.next()?;
    let last = trimmed.chars().next_back()?;

    let closing = match first {
        '"' | '`' if first == last => first,
        '[' if last == ']' => ']',
        _ => return None,
    };
    let opening_str = first.to_string();
    let closing_str = closing.to_string();
    let inner = trimmed
        .strip_prefix(opening_str.as_str())
        .and_then(|v| v.strip_suffix(closing_str.as_str()))?;

    quoted_identifier_inner_is_well_formed(inner, closing).then_some(first)
}

/// Returns true if `value` is wrapped as a quoted identifier.
pub(crate) fn is_quoted_identifier(value: &str) -> bool {
    quoted_identifier_delimiter(value).is_some()
}

/// Strips surrounding identifier quotes and unescapes doubled quote delimiters.
pub(crate) fn strip_identifier_quotes(value: &str) -> String {
    let trimmed = value.trim();
    let Some(delimiter) = quoted_identifier_delimiter(trimmed) else {
        return trimmed.to_string();
    };

    let delimiter_str = match delimiter {
        '"' => "\"",
        '`' => "`",
        '[' => "[",
        _ => return trimmed.to_string(),
    };
    let closing_str = if delimiter == '[' { "]" } else { delimiter_str };

    let Some(inner) = trimmed
        .strip_prefix(delimiter_str)
        .and_then(|v| v.strip_suffix(closing_str))
    else {
        return trimmed.to_string();
    };

    match delimiter {
        '"' => inner.replace("\"\"", "\""),
        '`' => inner.replace("``", "`"),
        '[' => inner.replace("]]", "]"),
        _ => inner.to_string(),
    }
}

/// Returns true when a line starts with SQL*Plus `REM`/`REMARK` comment commands.
pub(crate) fn is_sqlplus_remark_comment_line(line: &str) -> bool {
    let trimmed = line.trim_start();
    matches!(
        trimmed.split_whitespace().next(),
        Some(first)
            if first.eq_ignore_ascii_case("REM") || first.eq_ignore_ascii_case("REMARK")
    )
}

#[inline]
pub(crate) fn is_mysql_hash_comment_start(bytes: &[u8], idx: usize) -> bool {
    bytes.get(idx) == Some(&b'#')
        && idx
            .checked_sub(1)
            .and_then(|prev_idx| bytes.get(prev_idx))
            .is_none_or(|prev| !is_identifier_byte(*prev))
}

#[inline]
pub(crate) fn is_mysql_dash_comment_start(bytes: &[u8], idx: usize) -> bool {
    if bytes.get(idx) != Some(&b'-') || bytes.get(idx + 1) != Some(&b'-') {
        return false;
    }
    match bytes.get(idx + 2) {
        None => true,
        Some(byte) if byte.is_ascii_whitespace() || byte.is_ascii_control() => true,
        // A dash ruler (`--------…` to end of line) is a comment banner, not a
        // chain of minus operators. MySQL's `-- ` rule alone would open the
        // comment at the *last* two dashes and re-render everything before it as
        // operators, so re-formatting kept peeling the ruler apart.
        Some(b'-') => bytes[idx + 2..]
            .iter()
            .take_while(|byte| **byte != b'\n' && **byte != b'\r')
            .all(|byte| *byte == b'-' || byte.is_ascii_whitespace()),
        Some(_) => false,
    }
}

#[inline]
pub(crate) fn is_dash_line_comment_start(bytes: &[u8], idx: usize, mysql_compatible: bool) -> bool {
    if mysql_compatible {
        is_mysql_dash_comment_start(bytes, idx)
    } else {
        bytes.get(idx) == Some(&b'-') && bytes.get(idx + 1) == Some(&b'-')
    }
}

#[inline]
pub(crate) fn is_mysql_executable_comment_start(bytes: &[u8], idx: usize) -> bool {
    bytes.get(idx) == Some(&b'/')
        && bytes.get(idx + 1) == Some(&b'*')
        && (bytes.get(idx + 2) == Some(&b'!')
            || (bytes.get(idx + 2) == Some(&b'M') && bytes.get(idx + 3) == Some(&b'!')))
}

#[inline]
pub(crate) fn is_mysql_hash_comment_line(line: &str) -> bool {
    let trimmed = line.trim_start();
    is_mysql_hash_comment_start(trimmed.as_bytes(), 0)
}

#[inline]
pub(crate) fn is_mysql_block_label_target(word: &str) -> bool {
    matches!(
        word.to_ascii_uppercase().as_str(),
        "BEGIN" | "LOOP" | "REPEAT" | "WHILE"
    )
}

pub(crate) fn mysql_labeled_block_target_before_inline_comment(line: &str) -> Option<&str> {
    fn skip_ws_and_block_comments(bytes: &[u8], mut idx: usize) -> Option<usize> {
        while idx < bytes.len() {
            if bytes[idx].is_ascii_whitespace() {
                idx = idx.saturating_add(1);
                continue;
            }
            if sql_line_comment_prefix_len(bytes, idx).is_some() {
                return None;
            }
            if bytes[idx] == b'/' && bytes.get(idx.saturating_add(1)) == Some(&b'*') {
                idx = idx.saturating_add(2);
                while idx + 1 < bytes.len() {
                    if bytes[idx] == b'*' && bytes[idx + 1] == b'/' {
                        idx = idx.saturating_add(2);
                        break;
                    }
                    idx = idx.saturating_add(1);
                }
                continue;
            }
            break;
        }

        Some(idx)
    }

    let bytes = line.as_bytes();
    let mut idx = skip_ws_and_block_comments(bytes, 0)?;
    if idx >= bytes.len() || !is_identifier_start_byte(bytes[idx]) {
        return None;
    }
    idx = idx.saturating_add(1);
    while idx < bytes.len() && is_identifier_byte(bytes[idx]) {
        idx = idx.saturating_add(1);
    }

    idx = skip_ws_and_block_comments(bytes, idx)?;
    if bytes.get(idx) != Some(&b':') {
        return None;
    }
    idx = idx.saturating_add(1);

    idx = skip_ws_and_block_comments(bytes, idx)?;
    if idx >= bytes.len() || !is_identifier_start_byte(bytes[idx]) {
        return None;
    }
    let target_start = idx;
    idx = idx.saturating_add(1);
    while idx < bytes.len() && is_identifier_byte(bytes[idx]) {
        idx = idx.saturating_add(1);
    }

    let target = line.get(target_start..idx)?;
    is_mysql_block_label_target(target).then_some(target)
}

pub(crate) fn line_starts_mysql_block_keyword_before_inline_comment(
    line: &str,
    keyword: &str,
) -> bool {
    line_starts_with_identifier_sequence_before_inline_comment(line, &[keyword])
        || mysql_labeled_block_target_before_inline_comment(line)
            .is_some_and(|target| target.eq_ignore_ascii_case(keyword))
}

#[inline]
pub(crate) fn sql_line_comment_prefix_len(bytes: &[u8], idx: usize) -> Option<usize> {
    if bytes.get(idx) == Some(&b'-') && bytes.get(idx + 1) == Some(&b'-') {
        Some(2)
    } else if is_mysql_hash_comment_start(bytes, idx) {
        Some(1)
    } else {
        None
    }
}

pub(crate) fn line_has_mysql_hash_comment(line: &str) -> bool {
    let bytes = line.as_bytes();
    let mut idx = 0usize;
    let mut in_single_quote = false;
    let mut in_double_quote = false;
    let mut in_backtick_quote = false;
    let mut in_block_comment = false;
    let mut q_quote_end: Option<u8> = None;

    while idx < bytes.len() {
        let current = bytes[idx];
        let next = bytes.get(idx.saturating_add(1)).copied();

        if in_block_comment {
            if current == b'*' && next == Some(b'/') {
                in_block_comment = false;
                idx = idx.saturating_add(2);
                continue;
            }
            idx = idx.saturating_add(1);
            continue;
        }

        if let Some(closing) = q_quote_end {
            if current == closing && next == Some(b'\'') {
                q_quote_end = None;
                idx = idx.saturating_add(2);
                continue;
            }
            idx = idx.saturating_add(1);
            continue;
        }

        if in_single_quote {
            if current == b'\\' && next.is_some() {
                idx = idx.saturating_add(2);
                continue;
            }
            if current == b'\'' {
                if next == Some(b'\'') {
                    idx = idx.saturating_add(2);
                    continue;
                }
                in_single_quote = false;
            }
            idx = idx.saturating_add(1);
            continue;
        }

        if in_double_quote {
            if current == b'\\' && next.is_some() {
                idx = idx.saturating_add(2);
                continue;
            }
            if current == b'"' {
                if next == Some(b'"') {
                    idx = idx.saturating_add(2);
                    continue;
                }
                in_double_quote = false;
            }
            idx = idx.saturating_add(1);
            continue;
        }

        if in_backtick_quote {
            if current == b'`' {
                if next == Some(b'`') {
                    idx = idx.saturating_add(2);
                    continue;
                }
                in_backtick_quote = false;
            }
            idx = idx.saturating_add(1);
            continue;
        }

        if sql_line_comment_prefix_len(bytes, idx).is_some_and(|prefix_len| prefix_len == 2) {
            break;
        }
        // In MySQL/MariaDB mode `#` starts a line comment anywhere outside
        // strings, quoted identifiers, and block comments.
        if current == b'#' {
            return true;
        }
        if current == b'/' && next == Some(b'*') {
            in_block_comment = true;
            idx = idx.saturating_add(2);
            continue;
        }
        if let Some(q_quote_start) = q_quote_prefix_at(bytes, idx) {
            q_quote_end = Some(q_quote_start.closing_delimiter);
            idx = idx.saturating_add(q_quote_start.prefix_len);
            continue;
        }
        if current == b'\'' {
            in_single_quote = true;
            idx = idx.saturating_add(1);
            continue;
        }
        if current == b'"' {
            in_double_quote = true;
            idx = idx.saturating_add(1);
            continue;
        }
        if current == b'`' {
            in_backtick_quote = true;
            idx = idx.saturating_add(1);
            continue;
        }

        idx = idx.saturating_add(1);
    }

    false
}

pub(crate) fn parse_mysql_delimiter_directive(line: &str) -> Option<String> {
    let (word, start) = next_meaningful_word(line, 0)?;
    if !word.eq_ignore_ascii_case("DELIMITER") {
        return None;
    }

    let bytes = line.as_bytes();
    let mut idx = start.saturating_add(word.len());

    // Skip leading whitespace and block comments to reach the delimiter token.
    // We intentionally do NOT treat `--` or `#` as comment starters here:
    // MySQL's DELIMITER command accepts any non-whitespace string as the
    // delimiter value, including `--` or `#`.
    loop {
        while idx < bytes.len() && bytes[idx].is_ascii_whitespace() {
            idx += 1;
        }
        if idx >= bytes.len() {
            break;
        }
        if line[idx..].starts_with("/*") {
            let block_start = idx.saturating_add(2);
            let block_end = line.get(block_start..)?.find("*/")?;
            idx = block_start.saturating_add(block_end).saturating_add(2);
            continue;
        }
        break;
    }

    // The delimiter value is the first contiguous non-whitespace token.
    // Everything after the first whitespace is ignored (may be a comment).
    let mut delimiter_buf = String::new();
    while idx < bytes.len() && !bytes[idx].is_ascii_whitespace() {
        let ch = line.get(idx..)?.chars().next()?;
        delimiter_buf.push(ch);
        idx += ch.len_utf8();
    }

    Some(if delimiter_buf.is_empty() {
        ";".to_string()
    } else {
        delimiter_buf
    })
}

pub(crate) fn line_has_mysql_begin_not_atomic(line: &str) -> bool {
    meaningful_identifier_words_before_inline_comment(line.trim_start(), 6)
        .windows(3)
        .any(|window| {
            window[0].eq_ignore_ascii_case("BEGIN")
                && window[1].eq_ignore_ascii_case("NOT")
                && window[2].eq_ignore_ascii_case("ATOMIC")
        })
}

#[inline]
fn line_ends_with_mysql_vertical_terminator_candidate(line: &str) -> bool {
    let trimmed = line.trim_end();
    let bytes = trimmed.as_bytes();
    bytes.len() >= 2
        && bytes[bytes.len() - 2] == b'\\'
        && matches!(bytes[bytes.len() - 1], b'g' | b'G')
}

fn sql_contains_mysql_only_operator(sql: &str) -> bool {
    let bytes = sql.as_bytes();
    let mut idx = 0usize;
    let mut in_line_comment = false;
    let mut in_block_comment = false;
    let mut in_single_quote = false;
    let mut in_double_quote = false;
    let mut in_backtick_quote = false;
    let mut q_quote_end: Option<u8> = None;

    while idx < bytes.len() {
        let current = bytes[idx];
        let next = bytes.get(idx.saturating_add(1)).copied();

        if in_line_comment {
            if current == b'\n' {
                in_line_comment = false;
            }
            idx = idx.saturating_add(1);
            continue;
        }

        if in_block_comment {
            if current == b'*' && next == Some(b'/') {
                in_block_comment = false;
                idx = idx.saturating_add(2);
                continue;
            }
            idx = idx.saturating_add(1);
            continue;
        }

        if let Some(closing) = q_quote_end {
            if current == closing && next == Some(b'\'') {
                q_quote_end = None;
                idx = idx.saturating_add(2);
                continue;
            }
            idx = idx.saturating_add(1);
            continue;
        }

        if in_single_quote {
            if current == b'\\' && next.is_some() {
                idx = idx.saturating_add(2);
                continue;
            }
            if current == b'\'' {
                if next == Some(b'\'') {
                    idx = idx.saturating_add(2);
                    continue;
                }
                in_single_quote = false;
            }
            idx = idx.saturating_add(1);
            continue;
        }

        if in_double_quote {
            if current == b'\\' && next.is_some() {
                idx = idx.saturating_add(2);
                continue;
            }
            if current == b'"' {
                if next == Some(b'"') {
                    idx = idx.saturating_add(2);
                    continue;
                }
                in_double_quote = false;
            }
            idx = idx.saturating_add(1);
            continue;
        }

        if in_backtick_quote {
            if current == b'`' {
                if next == Some(b'`') {
                    idx = idx.saturating_add(2);
                    continue;
                }
                in_backtick_quote = false;
            }
            idx = idx.saturating_add(1);
            continue;
        }

        if let Some(prefix_len) = sql_line_comment_prefix_len(bytes, idx) {
            in_line_comment = true;
            idx = idx.saturating_add(prefix_len);
            continue;
        }
        if current == b'/' && next == Some(b'*') {
            in_block_comment = true;
            idx = idx.saturating_add(2);
            continue;
        }
        if let Some(q_quote_start) = q_quote_prefix_at(bytes, idx) {
            q_quote_end = Some(q_quote_start.closing_delimiter);
            idx = idx.saturating_add(q_quote_start.prefix_len);
            continue;
        }
        if current == b'\'' {
            in_single_quote = true;
            idx = idx.saturating_add(1);
            continue;
        }
        if current == b'"' {
            in_double_quote = true;
            idx = idx.saturating_add(1);
            continue;
        }
        if current == b'`' {
            in_backtick_quote = true;
            idx = idx.saturating_add(1);
            continue;
        }

        if current == b'-' && next == Some(b'>') && !arrow_closes_property_graph_edge(bytes, idx)
            || current == b'<'
                && next == Some(b'=')
                && bytes.get(idx.saturating_add(2)) == Some(&b'>')
        {
            return true;
        }

        idx = idx.saturating_add(1);
    }

    false
}

fn arrow_closes_property_graph_edge(bytes: &[u8], arrow_idx: usize) -> bool {
    let Some(close_idx) = previous_non_whitespace_byte(bytes, arrow_idx) else {
        return false;
    };
    if bytes.get(close_idx) != Some(&b']') {
        return false;
    }

    let mut cursor = close_idx;
    while cursor > 0 {
        cursor -= 1;
        match bytes[cursor] {
            b'[' => {
                return previous_non_whitespace_byte(bytes, cursor)
                    .is_some_and(|idx| bytes.get(idx) == Some(&b'-'));
            }
            b']' => return false,
            _ => {}
        }
    }

    false
}

fn previous_non_whitespace_byte(bytes: &[u8], before: usize) -> Option<usize> {
    (0..before)
        .rev()
        .find(|&idx| !bytes[idx].is_ascii_whitespace())
}

pub(crate) fn sql_uses_mysql_compatible_syntax(sql: &str) -> bool {
    sql.as_bytes().contains(&b'`')
        || sql_contains_mysql_only_operator(sql)
        || sql.contains("/*!")
        || sql.contains("/*M!")
        || sql.lines().any(|line| {
            line_has_mysql_hash_comment(line)
                || parse_mysql_delimiter_directive(line).is_some()
                || line_has_mysql_begin_not_atomic(line)
                || line_ends_with_mysql_vertical_terminator_candidate(line)
        })
}

const MYSQL_COMPAT_CACHE_MAX_ENTRIES: usize = 64;
const MYSQL_COMPAT_CACHE_MAX_SQL_BYTES: usize = 2 * 1024 * 1024;
#[cfg(not(test))]
const MYSQL_COMPAT_CACHE_MIN_SQL_LEN: usize = 256;

#[derive(Clone)]
struct MysqlCompatCacheEntry {
    sql: String,
    mysql_compatible: bool,
}

#[derive(Default)]
struct MysqlCompatCache {
    entries: VecDeque<MysqlCompatCacheEntry>,
    total_sql_bytes: usize,
}

static MYSQL_COMPAT_CACHE: Lazy<Mutex<MysqlCompatCache>> =
    Lazy::new(|| Mutex::new(MysqlCompatCache::default()));

#[cfg(test)]
fn should_cache_mysql_compatibility(sql: &str) -> bool {
    sql.len() <= MYSQL_COMPAT_CACHE_MAX_SQL_BYTES
}

#[cfg(not(test))]
fn should_cache_mysql_compatibility(sql: &str) -> bool {
    let sql_len = sql.len();
    if sql_len > MYSQL_COMPAT_CACHE_MAX_SQL_BYTES {
        return false;
    }
    sql_len >= MYSQL_COMPAT_CACHE_MIN_SQL_LEN
}

pub(crate) fn mysql_compatibility_for_sql(
    sql: &str,
    preferred_db_type: Option<DatabaseType>,
) -> bool {
    if let Some(db_type) = preferred_db_type {
        return match db_type {
            DatabaseType::Oracle => false,
            DatabaseType::MySQL => true,
            DatabaseType::MariaDB => true,
        };
    }

    if !should_cache_mysql_compatibility(sql) {
        return sql_uses_mysql_compatible_syntax(sql);
    }

    let cache = &*MYSQL_COMPAT_CACHE;
    {
        let mut guard = cache
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if let Some(hit_idx) = guard.entries.iter().position(|entry| entry.sql == sql) {
            let Some(entry) = guard.entries.remove(hit_idx) else {
                return sql_uses_mysql_compatible_syntax(sql);
            };
            let mysql_compatible = entry.mysql_compatible;
            guard.entries.push_front(entry);
            return mysql_compatible;
        }
    }

    let mysql_compatible = sql_uses_mysql_compatible_syntax(sql);
    let mut guard = cache
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    if let Some(existing_idx) = guard.entries.iter().position(|entry| entry.sql == sql) {
        let _ = guard.entries.remove(existing_idx);
    } else {
        guard.total_sql_bytes = guard.total_sql_bytes.saturating_add(sql.len());
    }
    guard.entries.push_front(MysqlCompatCacheEntry {
        sql: sql.to_string(),
        mysql_compatible,
    });
    while guard.entries.len() > MYSQL_COMPAT_CACHE_MAX_ENTRIES
        || guard.total_sql_bytes > MYSQL_COMPAT_CACHE_MAX_SQL_BYTES
    {
        let Some(removed) = guard.entries.pop_back() else {
            break;
        };
        guard.total_sql_bytes = guard.total_sql_bytes.saturating_sub(removed.sql.len());
    }
    mysql_compatible
}

/// Returns true when a line is a SQL*Plus-style comment-only line.
///
/// Recognizes:
/// - `-- ...`
/// - `# ...`
/// - `REM ...`
/// - `REMARK ...`
pub(crate) fn is_sqlplus_comment_line(line: &str) -> bool {
    let trimmed = line.trim_start();
    trimmed.starts_with("--") || trimmed.starts_with('#') || is_sqlplus_remark_comment_line(trimmed)
}

/// Updates `in_block_comment` state for a single trimmed line.
///
/// This properly handles lines that contain both `*/` (closing) and `/*` (opening)
/// on the same line (e.g. `*/ SELECT /* ... `). `apply_parser_depth_indentation`
/// must use this instead of ad-hoc `contains("*/")`.
pub(crate) fn update_block_comment_state(trimmed: &str, in_block_comment: &mut bool) {
    let bytes = trimmed.as_bytes();
    let mut i = 0usize;

    while i < bytes.len() {
        if *in_block_comment {
            if i + 1 < bytes.len() && bytes[i] == b'*' && bytes[i + 1] == b'/' {
                *in_block_comment = false;
                i += 2;
                continue;
            }
            i += 1;
        } else {
            if i + 1 < bytes.len() && bytes[i] == b'/' && bytes[i + 1] == b'*' {
                *in_block_comment = true;
                i += 2;
                continue;
            }
            // Stop scanning at line comment
            if sql_line_comment_prefix_len(bytes, i).is_some() {
                break;
            }
            // Skip string literals to avoid false matches on /* */ inside strings
            if bytes[i] == b'\'' {
                i += 1;
                while i < bytes.len() {
                    if bytes[i] == b'\'' {
                        i += 1;
                        if i < bytes.len() && bytes[i] == b'\'' {
                            i += 1;
                            continue;
                        }
                        break;
                    }
                    i += 1;
                }
                continue;
            }
            i += 1;
        }
    }
}

/// Returns the meaningful remainder of `line` after skipping leading
/// whitespace and comment-only prefixes.
///
/// This removes any leading `/* ... */` segments and SQL*Plus / `--`
/// comment-only prefixes so structural classifiers can reason from the first
/// significant token instead of the raw visual prefix.
pub(crate) fn trim_leading_sql_comments(line: &str) -> &str {
    let bytes = line.as_bytes();
    let mut idx = 0usize;

    loop {
        while idx < bytes.len() && bytes[idx].is_ascii_whitespace() {
            idx = idx.saturating_add(1);
        }

        let Some(tail) = line.get(idx..) else {
            return "";
        };
        if tail.is_empty() {
            return tail;
        }
        if is_sqlplus_comment_line(tail) {
            return "";
        }
        if tail.starts_with("*/") {
            idx = idx.saturating_add(2);
            continue;
        }
        if tail.starts_with("/*") {
            let Some(block_end) = tail.find("*/") else {
                return "";
            };
            idx = idx.saturating_add(block_end.saturating_add(2));
            continue;
        }

        return tail;
    }
}

/// Returns true when a line has no meaningful SQL tokens outside leading
/// comments, updating `in_block_comment` as it consumes the prefix.
pub(crate) fn line_is_comment_only_with_block_state(
    line: &str,
    in_block_comment: &mut bool,
) -> bool {
    let trimmed = line.trim_start();
    if trimmed.is_empty() {
        return false;
    }
    if !*in_block_comment && is_sqlplus_comment_line(trimmed) {
        return true;
    }

    let bytes = trimmed.as_bytes();
    let mut idx = 0usize;
    while idx < bytes.len() {
        if *in_block_comment {
            let mut closed = false;
            while idx + 1 < bytes.len() {
                if bytes[idx] == b'*' && bytes[idx + 1] == b'/' {
                    *in_block_comment = false;
                    idx = idx.saturating_add(2);
                    closed = true;
                    break;
                }
                idx = idx.saturating_add(1);
            }
            if !closed {
                return true;
            }
            continue;
        }

        while idx < bytes.len() && bytes[idx].is_ascii_whitespace() {
            idx = idx.saturating_add(1);
        }

        if idx >= bytes.len() {
            return true;
        }

        let tail = trimmed.get(idx..).unwrap_or_default();
        if is_sqlplus_comment_line(tail) {
            return true;
        }

        if idx + 1 < bytes.len() && bytes[idx] == b'/' && bytes[idx + 1] == b'*' {
            *in_block_comment = true;
            idx = idx.saturating_add(2);
            continue;
        }

        return false;
    }

    true
}

/// Returns the prefix before a trailing inline comment, if the line ends with
/// `-- ...` or `/* ... */` after skipping quoted SQL literals.
///
/// Non-trailing inline block comments are skipped so later line comments still
/// reuse the full structural prefix (`FOR /* gap */ UPDATE -- ...`,
/// `GROUP /* gap */ BY -- ...`, ...). Returns `None` when the line has no
/// trailing inline comment or when a block comment is unterminated.
pub(crate) fn trailing_inline_comment_prefix(line: &str) -> Option<&str> {
    let bytes = line.as_bytes();
    let mut idx = 0usize;
    let mut in_single_quote = false;
    let mut in_double_quote = false;
    let mut q_quote_end: Option<u8> = None;

    while idx < bytes.len() {
        let current = bytes[idx];
        let next = bytes.get(idx.saturating_add(1)).copied();

        if let Some(closing) = q_quote_end {
            if current == closing && next == Some(b'\'') {
                q_quote_end = None;
                idx = idx.saturating_add(2);
                continue;
            }
            idx = idx.saturating_add(1);
            continue;
        }

        if in_single_quote {
            if current == b'\'' {
                if next == Some(b'\'') {
                    idx = idx.saturating_add(2);
                    continue;
                }
                in_single_quote = false;
            }
            idx = idx.saturating_add(1);
            continue;
        }

        if in_double_quote {
            if current == b'"' {
                if next == Some(b'"') {
                    idx = idx.saturating_add(2);
                    continue;
                }
                in_double_quote = false;
            }
            idx = idx.saturating_add(1);
            continue;
        }

        if sql_line_comment_prefix_len(bytes, idx).is_some() {
            return line.get(..idx);
        }

        if current == b'/' && next == Some(b'*') {
            let comment_start = idx;
            idx = idx.saturating_add(2);
            let mut closed = false;
            while idx + 1 < bytes.len() {
                if bytes[idx] == b'*' && bytes[idx + 1] == b'/' {
                    let comment_end = idx.saturating_add(2);
                    closed = true;
                    let suffix = line.get(comment_end..).unwrap_or_default().trim();
                    if suffix.is_empty() {
                        return line.get(..comment_start);
                    }
                    idx = comment_end;
                    break;
                }
                idx = idx.saturating_add(1);
            }
            if !closed {
                return None;
            }
            continue;
        }

        if let Some(q_quote_start) = q_quote_prefix_at(bytes, idx) {
            q_quote_end = Some(q_quote_start.closing_delimiter);
            idx = idx.saturating_add(q_quote_start.prefix_len);
            continue;
        }
        if current == b'\'' {
            in_single_quote = true;
            idx = idx.saturating_add(1);
            continue;
        }
        if current == b'"' {
            in_double_quote = true;
            idx = idx.saturating_add(1);
            continue;
        }

        idx = idx.saturating_add(1);
    }

    None
}

/// Returns the last non-whitespace ASCII byte before a trailing inline comment.
pub(crate) fn trailing_significant_byte_before_inline_comment(line: &str) -> Option<u8> {
    line_trailing_identifiers_before_inline_comment(line, 0).0
}

pub(crate) fn line_ends_with_open_paren_before_inline_comment(line: &str) -> bool {
    trailing_significant_byte_before_inline_comment(line) == Some(b'(')
}

pub(crate) fn line_ends_with_comma_before_inline_comment(line: &str) -> bool {
    trailing_significant_byte_before_inline_comment(line) == Some(b',')
}

pub(crate) fn line_is_standalone_open_paren_before_inline_comment(line: &str) -> bool {
    matches!(
        trailing_meaningful_tokens_before_inline_comment(line),
        (
            None,
            Some(FormatTrailingMeaningfulToken::Symbol(symbol))
        ) if symbol == "("
    )
}

#[inline]
fn is_dollar_quote_tag_char_byte(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || byte == b'_'
}

fn parse_dollar_quote_tag_bytes(bytes: &[u8], start: usize) -> Option<Vec<u8>> {
    if bytes.get(start) != Some(&b'$') {
        return None;
    }

    let mut idx = start.saturating_add(1);
    while idx < bytes.len() {
        let current = bytes[idx];
        if current == b'$' {
            return bytes.get(start..=idx).map(|tag| tag.to_vec());
        }
        if !is_dollar_quote_tag_char_byte(current) {
            return None;
        }
        idx = idx.saturating_add(1);
    }

    None
}

#[inline]
fn looks_like_oracle_conditional_compilation_flag_bytes(bytes: &[u8], start: usize) -> bool {
    bytes.get(start) == Some(&b'$')
        && bytes.get(start.saturating_add(1)) == Some(&b'$')
        && bytes
            .get(start.saturating_add(2))
            .copied()
            .is_some_and(is_identifier_start_byte)
}

fn delimiter_as_dollar_quote_tag_bytes(delimiter: &str) -> Option<Vec<u8>> {
    let bytes = delimiter.as_bytes();
    if bytes.len() < 2 || bytes.first() != Some(&b'$') || bytes.last() != Some(&b'$') {
        return None;
    }

    bytes
        .get(1..bytes.len().saturating_sub(1))
        .is_some_and(|middle| {
            middle
                .iter()
                .all(|byte| is_dollar_quote_tag_char_byte(*byte))
        })
        .then(|| bytes.to_vec())
}

/// Returns the leading byte length that belongs to an already-open multiline
/// string / quoted identifier / q-quote for each line.
///
/// `Some(prefix_len)` means the line starts inside a multiline quoted payload.
/// If `prefix_len == line.len()`, the whole line belongs to the payload.
/// Otherwise the quoted payload closes on that line and structural SQL starts
/// again at `line[prefix_len..]`.
pub(crate) fn multiline_string_continuation_prefix_lengths(
    text: &str,
    line_count: usize,
) -> Vec<Option<usize>> {
    let mut prefix_lengths = vec![None; line_count];
    if line_count == 0 {
        return prefix_lengths;
    }

    let lines: Vec<&str> = text.lines().collect();
    let mysql_delimiter_directive_lines: Vec<bool> = lines
        .iter()
        .map(|line| parse_mysql_delimiter_directive(line).is_some())
        .collect();
    let mut active_mysql_delimiter = ";".to_string();
    let mut active_mysql_delimiter_dollar_tags: Vec<Option<Vec<u8>>> = vec![None; line_count];
    for (line_idx, line_text) in lines.iter().enumerate() {
        active_mysql_delimiter_dollar_tags[line_idx] =
            delimiter_as_dollar_quote_tag_bytes(active_mysql_delimiter.as_str());
        if let Some(delimiter) = parse_mysql_delimiter_directive(line_text) {
            active_mysql_delimiter = delimiter;
        }
    }
    let bytes = text.as_bytes();
    let mut i = 0usize;
    let mut line = 0usize;
    let mut line_start = 0usize;
    let mut line_starts_in_multiline_string = false;

    let mut in_single_quote = false;
    let mut in_double_quote = false;
    let mut in_backtick_quote = false;
    let mut in_line_comment = false;
    let mut in_block_comment = false;
    let mut in_q_quote = false;
    let mut q_quote_end: Option<u8> = None;
    let mut dollar_quote_tag: Option<Vec<u8>> = None;

    while i < bytes.len() {
        let c = bytes[i];
        let next = bytes.get(i + 1).copied();

        if in_line_comment {
            if c == b'\n' {
                in_line_comment = false;
                line += 1;
                line_start = i.saturating_add(1);
                line_starts_in_multiline_string = false;
            }
            i += 1;
            continue;
        }

        if in_block_comment {
            if c == b'*' && next == Some(b'/') {
                in_block_comment = false;
                i += 2;
                continue;
            }
            if c == b'\n' {
                line += 1;
                line_start = i.saturating_add(1);
                line_starts_in_multiline_string = false;
            }
            i += 1;
            continue;
        }

        if in_q_quote {
            if Some(c) == q_quote_end && next == Some(b'\'') {
                if line_starts_in_multiline_string && line < line_count {
                    prefix_lengths[line] = Some(i.saturating_add(2).saturating_sub(line_start));
                }
                in_q_quote = false;
                q_quote_end = None;
                i += 2;
                continue;
            }
            if c == b'\n' {
                if line_starts_in_multiline_string && line < line_count {
                    prefix_lengths[line] = Some(lines[line].len());
                }
                line += 1;
                line_start = i.saturating_add(1);
                line_starts_in_multiline_string = true;
            }
            i += 1;
            continue;
        }

        if in_single_quote {
            if c == b'\'' {
                if next == Some(b'\'') {
                    i += 2;
                    continue;
                }
                if line_starts_in_multiline_string && line < line_count {
                    prefix_lengths[line] = Some(i.saturating_add(1).saturating_sub(line_start));
                }
                in_single_quote = false;
                i += 1;
                continue;
            }
            if c == b'\n' {
                if line_starts_in_multiline_string && line < line_count {
                    prefix_lengths[line] = Some(lines[line].len());
                }
                line += 1;
                line_start = i.saturating_add(1);
                line_starts_in_multiline_string = true;
            }
            i += 1;
            continue;
        }

        if in_double_quote {
            if c == b'"' {
                if next == Some(b'"') {
                    i += 2;
                    continue;
                }
                if line_starts_in_multiline_string && line < line_count {
                    prefix_lengths[line] = Some(i.saturating_add(1).saturating_sub(line_start));
                }
                in_double_quote = false;
                i += 1;
                continue;
            }
            if c == b'\n' {
                if line_starts_in_multiline_string && line < line_count {
                    prefix_lengths[line] = Some(lines[line].len());
                }
                line += 1;
                line_start = i.saturating_add(1);
                line_starts_in_multiline_string = true;
            }
            i += 1;
            continue;
        }

        if in_backtick_quote {
            if c == b'`' {
                if next == Some(b'`') {
                    i += 2;
                    continue;
                }
                if line_starts_in_multiline_string && line < line_count {
                    prefix_lengths[line] = Some(i.saturating_add(1).saturating_sub(line_start));
                }
                in_backtick_quote = false;
                i += 1;
                continue;
            }
            if c == b'\n' {
                if line_starts_in_multiline_string && line < line_count {
                    prefix_lengths[line] = Some(lines[line].len());
                }
                line += 1;
                line_start = i.saturating_add(1);
                line_starts_in_multiline_string = true;
            }
            i += 1;
            continue;
        }

        if let Some(tag) = dollar_quote_tag.as_ref() {
            let tag_len = tag.len();
            let closes_here = bytes
                .get(i..)
                .is_some_and(|tail| tail.starts_with(tag.as_slice()));
            if closes_here {
                if line_starts_in_multiline_string && line < line_count {
                    prefix_lengths[line] =
                        Some(i.saturating_add(tag_len).saturating_sub(line_start));
                }
                dollar_quote_tag = None;
                i = i.saturating_add(tag_len);
                continue;
            }
            if c == b'\n' {
                if line_starts_in_multiline_string && line < line_count {
                    prefix_lengths[line] = Some(lines[line].len());
                }
                line += 1;
                line_start = i.saturating_add(1);
                line_starts_in_multiline_string = true;
            }
            i += 1;
            continue;
        }

        if c == b'\n' {
            line += 1;
            line_start = i.saturating_add(1);
            line_starts_in_multiline_string = false;
            i += 1;
            continue;
        }

        if let Some(prefix_len) = sql_line_comment_prefix_len(bytes, i) {
            in_line_comment = true;
            i += prefix_len;
            continue;
        }

        if c == b'/' && next == Some(b'*') {
            in_block_comment = true;
            i += 2;
            continue;
        }

        if let Some(q_quote_start) = q_quote_prefix_at(bytes, i) {
            in_q_quote = true;
            q_quote_end = Some(q_quote_start.closing_delimiter);
            i += q_quote_start.prefix_len;
            continue;
        }

        let line_is_mysql_delimiter_directive = mysql_delimiter_directive_lines
            .get(line)
            .copied()
            .unwrap_or(false);
        let active_mysql_delimiter_dollar_tag = active_mysql_delimiter_dollar_tags
            .get(line)
            .and_then(|tag| tag.as_ref());
        if c == b'$' && !line_is_mysql_delimiter_directive {
            if let Some(tag) = parse_dollar_quote_tag_bytes(bytes, i) {
                let starts_active_mysql_delimiter = active_mysql_delimiter_dollar_tag
                    .is_some_and(|delimiter_tag| delimiter_tag.as_slice() == tag.as_slice());
                if !starts_active_mysql_delimiter
                    && !looks_like_oracle_conditional_compilation_flag_bytes(bytes, i)
                {
                    let tag_len = tag.len();
                    dollar_quote_tag = Some(tag);
                    i = i.saturating_add(tag_len);
                    continue;
                }
            }
        }

        if c == b'\'' {
            in_single_quote = true;
            i += 1;
            continue;
        }

        if c == b'"' {
            in_double_quote = true;
            i += 1;
            continue;
        }
        if c == b'`' {
            in_backtick_quote = true;
            i += 1;
            continue;
        }

        i += 1;
    }

    if (in_single_quote
        || in_double_quote
        || in_backtick_quote
        || in_q_quote
        || dollar_quote_tag.is_some())
        && line_starts_in_multiline_string
        && line < line_count
    {
        prefix_lengths[line] = Some(lines[line].len());
    }

    prefix_lengths
}

/// Returns true if `word` is one of the shared Oracle SQL keywords.
#[inline]
pub(crate) fn is_oracle_sql_keyword(word: &str) -> bool {
    ORACLE_SQL_KEYWORDS_SET.contains(word)
}

/// Returns true for PL/SQL control-flow keywords that may also appear as aliases.
pub(crate) fn is_plsql_control_keyword(word: &str) -> bool {
    let upper: Cow<'_, str> = if word.bytes().any(|b| b.is_ascii_lowercase()) {
        Cow::Owned(word.to_ascii_uppercase())
    } else {
        Cow::Borrowed(word)
    };

    matches!(
        upper.as_ref(),
        "IF" | "THEN"
            | "ELSE"
            | "ELSIF"
            | "CASE"
            | "LOOP"
            | "FOR"
            | "WHILE"
            | "REPEAT"
            | "DECLARE"
            | "BEGIN"
            | "END"
    )
}

/// Returns true when a keyword can start the main query after a WITH clause.
pub(crate) fn is_with_main_query_keyword(word: &str) -> bool {
    matches_keyword(word, WITH_MAIN_QUERY_KEYWORDS)
}

/// Returns true when a keyword starts an Oracle top-level `WITH` PL/SQL declaration.
pub(crate) fn is_with_plsql_declaration_keyword(word: &str) -> bool {
    matches_keyword(word, WITH_PLSQL_DECLARATION_KEYWORDS)
}

/// Returns true when a top-level `WITH` PL/SQL declaration uses `AS/IS` to
/// open a declaration body that stays active until a matching `END`.
pub(crate) fn with_plsql_declaration_starts_routine_body(word: &str) -> bool {
    matches!(
        word.to_ascii_uppercase().as_str(),
        "FUNCTION" | "PROCEDURE" | "PACKAGE"
    )
}

/// Returns true when a top-level `WITH` token clearly belongs to a non-PL/SQL
/// clause (for example `WITH READ ONLY` in view definitions).
pub(crate) fn is_with_non_plsql_clause_keyword(word: &str) -> bool {
    matches_keyword(word, WITH_NON_PLSQL_CLAUSE_KEYWORDS)
}

/// Returns true when a top-level `WITH` starts a query-head clause that is not
/// a CTE/PLSQL declaration (for example `WITH XMLNAMESPACES (...) SELECT ...`).
pub(crate) fn is_with_non_cte_query_head_keyword(word: &str) -> bool {
    matches_keyword(word, WITH_NON_CTE_QUERY_HEAD_KEYWORDS)
}

/// Returns true when a token can reasonably start a new top-level statement.
///
/// Used as a recovery signal when the parser stayed inside an Oracle
/// `WITH FUNCTION/PROCEDURE` declaration mode but encountered another
/// statement head instead of a main query keyword.
pub(crate) fn is_statement_head_keyword(word: &str) -> bool {
    let upper = word.to_ascii_uppercase();
    is_statement_head_keyword_upper(upper.as_str())
}

/// Uppercase fast path for statement-head detection.
///
/// Callers that already computed an ASCII-uppercased view of the line can
/// avoid rebuilding a second temporary string for the first identifier token.
pub(crate) fn is_statement_head_keyword_upper(word_upper: &str) -> bool {
    STATEMENT_HEAD_KEYWORDS_SET.contains(word_upper)
        || is_password_command_keyword_upper(word_upper)
}

pub(crate) fn is_mysql_statement_head_keyword_upper(word_upper: &str) -> bool {
    MYSQL_STATEMENT_HEAD_KEYWORDS_SET.contains(word_upper)
}

/// Returns true when an (already ASCII-uppercased) keyword can only begin a
/// statement or SQL*Plus/tool command and therefore can never be a token inside
/// a value expression (a SELECT item, WHERE/JOIN/HAVING predicate, GROUP/ORDER
/// BY key, SET/VALUES operand, …). IntelliSense uses this to drop such keywords
/// from completion while the cursor sits in an expression/column context, where
/// offering `CREATE`/`DROP`/`GRANT`/`SPOOL`/… is pure noise.
///
/// Derived from [`STATEMENT_HEAD_KEYWORDS`] so any statement head added there is
/// covered automatically, minus the few heads that double as an expression
/// operator or a column-clause word and must stay available in an expression:
///   * `DESC`            → `ORDER BY x DESC`
///   * `VALUES`          → `INSERT … VALUES (…)` row constructor / clause
///   * `SET`             → `UPDATE … SET` assignment clause
///   * `TABLE`           → `TABLE(collection_expr)` in a query
///   * `START` / `CONNECT` → hierarchical `START WITH` / `CONNECT BY` starters
///   * `SELECT` / `WITH` → a nested query or set-operator branch
///   * `COLUMN`          → DDL `ALTER TABLE … [ADD|DROP] COLUMN` slot
pub(crate) fn is_non_expression_statement_keyword_upper(word_upper: &str) -> bool {
    STATEMENT_HEAD_KEYWORDS_SET.contains(word_upper)
        && !matches!(
            word_upper,
            "DESC"
                | "VALUES"
                | "SET"
                | "TABLE"
                | "START"
                | "CONNECT"
                | "SELECT"
                | "WITH"
                | "COLUMN"
        )
}

/// Keywords that only appear inside a DDL object/storage/constraint clause
/// (`CREATE`/`ALTER`/`DROP …`) and never as a token in a value expression or a
/// query clause. IntelliSense drops these in an expression/column context, the
/// same way [`is_non_expression_statement_keyword_upper`] handles statement and
/// command heads. Kept separate from the statement-head set because these are
/// mid-clause modifiers rather than statement starters.
///
/// Membership is deliberately conservative: a keyword is included only when it
/// has no role in a query expression, a `SELECT`/`FROM`/`WHERE`/… clause, a
/// window/analytic spec, or a hierarchical clause. Notably excluded for that
/// reason — `PARTITION` (`OVER (PARTITION BY …)`), `USING` (`JOIN … USING`),
/// `NOCYCLE`/`CYCLE` (`CONNECT BY NOCYCLE`, recursive-CTE `CYCLE`), and the data
/// types (valid in `CAST(… AS <type>)`). When unsure, leave a keyword out: a
/// missed entry merely stays as today's noise, whereas a wrong entry would hide
/// a legitimate completion.
const DDL_ONLY_CLAUSE_KEYWORDS: &[&str] = &[
    // Physical/storage clauses.
    "PCTFREE",
    "PCTUSED",
    "PCTVERSION",
    "INITRANS",
    "MAXTRANS",
    "STORAGE",
    "TABLESPACE",
    "LOGGING",
    "NOLOGGING",
    "COMPRESS",
    "NOCOMPRESS",
    "NOCACHE",
    "NOPARALLEL",
    "MONITORING",
    "NOMONITORING",
    "FREEPOOLS",
    "RETENTION",
    "SECUREFILE",
    "BASICFILE",
    "ENABLE_STORAGE_IN_ROW",
    "NOSORT",
    "RECYCLEBIN",
    "NOARCHIVE",
    "ORGANIZATION",
    "OVERFLOW",
    "HEAP",
    "IOT",
    "CHUNK",
    // Integrity-constraint clauses.
    "CONSTRAINT",
    "CASCADE",
    "DEFERRABLE",
    "DEFERRED",
    "INITIALLY",
    "VALIDATE",
    "NOVALIDATE",
    "RELY",
    "NORELY",
    // Object/user attribute clauses.
    "IDENTIFIED",
    "PROFILE",
    "EDITIONABLE",
    "NONEDITIONABLE",
    "NONEDITIONING",
    "INVISIBLE",
    "INVALIDATE",
    "EXTERNALLY",
    "EXPIRE",
    "NOFORCE",
    "REUSE",
    "PRESERVE",
    "ACCOUNT",
    "COMPILE",
    "DEDUPLICATE",
    "ENABLE",
    "DISABLE",
    "EXTERNAL",
    "LANGUAGE",
    "GLOBALLY",
    "UNLIMITED",
    "UNCONDITIONAL",
    "EVENTS",
    // PL/SQL declaration-only keywords (a routine/package header, never a value
    // expression). Kept out as expression/clause-valid: `DOCUMENT`/`CONTENT`
    // (XML `IS DOCUMENT`/serialize), `PERIOD`/`APPLY`/`UPSERT` (temporal / APPLY
    // join / MODEL rules), and `BULK` (`SELECT … BULK COLLECT INTO`).
    "BODY",
    "SPECIFICATION",
    "NOCOPY",
    "NOEXCEPTIONS",
    "SEQUENTIAL",
];

/// O(1) lookup set for [`DDL_ONLY_CLAUSE_KEYWORDS`].
static DDL_ONLY_CLAUSE_KEYWORDS_SET: Lazy<HashSet<&'static str>> =
    Lazy::new(|| DDL_ONLY_CLAUSE_KEYWORDS.iter().copied().collect());

/// Returns true when an (already ASCII-uppercased) keyword is a DDL-only clause
/// modifier that can never appear in a value expression. See
/// [`DDL_ONLY_CLAUSE_KEYWORDS`].
pub(crate) fn is_ddl_only_clause_keyword_upper(word_upper: &str) -> bool {
    DDL_ONLY_CLAUSE_KEYWORDS_SET.contains(word_upper)
}

/// MySQL/MariaDB administrative & utility statement keywords that are absent
/// from the (Oracle-shaped) [`STATEMENT_HEAD_KEYWORDS`] but are likewise only
/// valid as their own statement/command head, never inside a value expression.
/// Kept dialect-specific so the deny check covers MySQL completion too.
///
/// Conservative for the same reason as the other sets: words that double as a
/// function (`REPLACE`, `REPEAT`), a data type (`FLOAT`, `CHAR`), or a query
/// clause are deliberately excluded so a valid completion is never hidden.
const MYSQL_NON_EXPRESSION_COMMAND_KEYWORDS: &[&str] = &[
    "CACHE",
    "FLUSH",
    "KILL",
    "OPTIMIZE",
    "PREPARE",
    "DEALLOCATE",
    "DELIMITER",
    "HANDLER",
    "LOAD",
    "REPAIR",
    "RESET",
    "CHANGE",
    "SIGNAL",
    "RESIGNAL",
    "SOURCE",
    "INSTALL",
    "UNINSTALL",
    "CHECKSUM",
    "BINLOG",
    "RESTART",
];

/// O(1) lookup set for [`MYSQL_NON_EXPRESSION_COMMAND_KEYWORDS`].
static MYSQL_NON_EXPRESSION_COMMAND_KEYWORDS_SET: Lazy<HashSet<&'static str>> = Lazy::new(|| {
    MYSQL_NON_EXPRESSION_COMMAND_KEYWORDS
        .iter()
        .copied()
        .collect()
});

/// MySQL/MariaDB DDL table-option / index / routine-characteristic keywords that
/// only appear inside `CREATE`/`ALTER TABLE … <option>` or a routine header, and
/// never in a value expression. Analogue of [`DDL_ONLY_CLAUSE_KEYWORDS`] for the
/// MySQL grammar.
///
/// Conservative as always — the expression/clause-valid lookalikes are kept out:
/// `DEFAULT` (`VALUES (DEFAULT)`), `COLLATE` (the collation operator), `UNSIGNED`
/// /`SIGNED` (`CAST(x AS UNSIGNED)`), `PARTITION` (`OVER (PARTITION BY …)`),
/// `IGNORE` (Oracle analytic `IGNORE NULLS`), `LOCKED` (`SKIP LOCKED`), and the
/// `COALESCE` function.
const MYSQL_DDL_FRAGMENT_KEYWORDS: &[&str] = &[
    "AUTO_INCREMENT",
    "ENGINE",
    "ENGINES",
    "SPATIAL",
    "FULLTEXT",
    "ALGORITHM",
    "ZEROFILL",
    "ROW_FORMAT",
    "CHARSET",
    "COMPRESSED",
    "TEMPORARY",
    "PARTITIONS",
    "STATS_AUTO_RECALC",
    "STATS_PERSISTENT",
    "STATS_SAMPLE_PAGES",
    "DELAYED",
    "AVG_ROW_LENGTH",
    "MAX_ROWS",
    "MIN_ROWS",
    "PACK_KEYS",
    "KEY_BLOCK_SIZE",
    "INSERT_METHOD",
    "CONNECTION",
    "INVOKER",
    "DEFINER",
    "MODIFIES",
    "READS",
    "LOW_PRIORITY",
    "IGNORE_DOMAIN_IDS",
    "IGNORE_SERVER_IDS",
    "ENCRYPTION",
    "QUICK",
];

/// O(1) lookup set for [`MYSQL_DDL_FRAGMENT_KEYWORDS`].
static MYSQL_DDL_FRAGMENT_KEYWORDS_SET: Lazy<HashSet<&'static str>> =
    Lazy::new(|| MYSQL_DDL_FRAGMENT_KEYWORDS.iter().copied().collect());

/// MySQL/MariaDB administrative option names used in replication, resource
/// group, and NDB tablespace/logfile statements. They should highlight as
/// syntax tokens, but they are not valid value-expression keywords.
const MYSQL_ADMIN_OPTION_KEYWORDS: &[&str] = &[
    "ASSIGN_GTIDS_TO_ANONYMOUS_TRANSACTIONS",
    "AUTHENTICATION",
    "AUTOEXTEND_SIZE",
    "BUCKETS",
    "CHANNEL",
    "CIPHER",
    "DEFAULT_AUTH",
    "ENGINE_ATTRIBUTE",
    "EXTENT_SIZE",
    "FILE_BLOCK_SIZE",
    "FILTER",
    "GET_SOURCE_PUBLIC_KEY",
    "GROUP_REPLICATION",
    "GTIDS",
    "GTID_ONLY",
    "HISTOGRAM",
    "INITIAL",
    "INITIAL_SIZE",
    "IO_THREAD",
    "ISSUER",
    "MAX_SIZE",
    "MYSQL_ADMIN",
    "MYSQL_MAIN",
    "NETWORK_NAMESPACE",
    "NODEGROUP",
    "PLUGIN_DIR",
    "PERSIST_ONLY",
    "PRIVILEGE_CHECKS_USER",
    "RANDOM",
    "REDO_BUFFER_SIZE",
    "REDO_LOG",
    "RELAY_LOG_FILE",
    "RELAY_LOG_POS",
    "REPLICA",
    "REPLICAS",
    "REPLICATE_DO_DB",
    "REPLICATE_DO_TABLE",
    "REPLICATE_IGNORE_DB",
    "REPLICATE_IGNORE_TABLE",
    "REPLICATE_REWRITE_DB",
    "REPLICATE_WILD_DO_TABLE",
    "REPLICATE_WILD_IGNORE_TABLE",
    "REPLICATION",
    "REQUIRE_ROW_FORMAT",
    "REQUIRE_TABLE_PRIMARY_KEY_CHECK",
    "RETAIN",
    "SOURCE_AUTO_POSITION",
    "SOURCE_BIND",
    "SOURCE_COMPRESSION_ALGORITHMS",
    "SOURCE_CONNECTION_AUTO_FAILOVER",
    "SOURCE_CONNECT_RETRY",
    "SOURCE_DELAY",
    "SOURCE_HEARTBEAT_PERIOD",
    "SOURCE_HOST",
    "SOURCE_LOG_FILE",
    "SOURCE_LOG_POS",
    "SOURCE_PASSWORD",
    "SOURCE_PORT",
    "SOURCE_PUBLIC_KEY_PATH",
    "SOURCE_RETRY_COUNT",
    "SOURCE_SSL",
    "SOURCE_SSL_CA",
    "SOURCE_SSL_CAPATH",
    "SOURCE_SSL_CERT",
    "SOURCE_SSL_CIPHER",
    "SOURCE_SSL_CRL",
    "SOURCE_SSL_CRLPATH",
    "SOURCE_SSL_KEY",
    "SOURCE_SSL_VERIFY_SERVER_CERT",
    "SOURCE_TLS_CIPHERSUITES",
    "SOURCE_TLS_VERSION",
    "SOURCE_USER",
    "SOURCE_ZSTD_COMPRESSION_LEVEL",
    "SQL_AFTER_GTIDS",
    "SQL_AFTER_MTS_GAPS",
    "SQL_BEFORE_GTIDS",
    "SQL_THREAD",
    "THREAD_PRIORITY",
    "TLS",
    "UNDOFILE",
    "UNDO_BUFFER_SIZE",
    "USE_FRM",
    "USER_RESOURCES",
    "VCPU",
];

/// O(1) lookup set for [`MYSQL_ADMIN_OPTION_KEYWORDS`].
static MYSQL_ADMIN_OPTION_KEYWORDS_SET: Lazy<HashSet<&'static str>> =
    Lazy::new(|| MYSQL_ADMIN_OPTION_KEYWORDS.iter().copied().collect());

/// MySQL dynamic privilege names. These are syntactic privilege tokens in
/// GRANT/REVOKE, not value-expression operands.
const MYSQL_DYNAMIC_PRIVILEGE_KEYWORDS: &[&str] = &[
    "ALLOW_NONEXISTENT_DEFINER",
    "APPLICATION_PASSWORD_ADMIN",
    "AUDIT_ABORT_EXEMPT",
    "AUDIT_ADMIN",
    "AUTHENTICATION_POLICY_ADMIN",
    "BACKUP_ADMIN",
    "BINLOG_ADMIN",
    "BINLOG_ENCRYPTION_ADMIN",
    "CLONE_ADMIN",
    "CONNECTION_ADMIN",
    "ENCRYPTION_KEY_ADMIN",
    "FIREWALL_ADMIN",
    "FIREWALL_EXEMPT",
    "FIREWALL_USER",
    "FLUSH_OPTIMIZER_COSTS",
    "FLUSH_PRIVILEGES",
    "FLUSH_STATUS",
    "FLUSH_TABLES",
    "FLUSH_USER_RESOURCES",
    "GROUP_REPLICATION_ADMIN",
    "GROUP_REPLICATION_STREAM",
    "INNODB_REDO_LOG_ARCHIVE",
    "INNODB_REDO_LOG_ENABLE",
    "MASKING_DICTIONARIES_ADMIN",
    "NDB_STORED_USER",
    "OPTIMIZE_LOCAL_TABLE",
    "PASSWORDLESS_USER_ADMIN",
    "PERSIST_RO_VARIABLES_ADMIN",
    "REPLICATION_APPLIER",
    "REPLICATION_SLAVE_ADMIN",
    "RESOURCE_GROUP_ADMIN",
    "RESOURCE_GROUP_USER",
    "ROLE_ADMIN",
    "SENSITIVE_VARIABLES_OBSERVER",
    "SESSION_VARIABLES_ADMIN",
    "SET_ANY_DEFINER",
    "SHOW_ROUTINE",
    "SYSTEM_USER",
    "SYSTEM_VARIABLES_ADMIN",
    "TABLE_ENCRYPTION_ADMIN",
    "TELEMETRY_LOG_ADMIN",
    "TRANSACTION_GTID_TAG",
    "XA_RECOVER_ADMIN",
];

/// O(1) lookup set for [`MYSQL_DYNAMIC_PRIVILEGE_KEYWORDS`].
static MYSQL_DYNAMIC_PRIVILEGE_KEYWORDS_SET: Lazy<HashSet<&'static str>> =
    Lazy::new(|| MYSQL_DYNAMIC_PRIVILEGE_KEYWORDS.iter().copied().collect());

/// MySQL account option names used by CREATE/ALTER USER. These should highlight
/// as syntax in account statements but never complete as expression operands.
const MYSQL_ACCOUNT_OPTION_KEYWORDS: &[&str] = &[
    "FAILED_LOGIN_ATTEMPTS",
    "MAX_CONNECTIONS_PER_HOUR",
    "MAX_QUERIES_PER_HOUR",
    "MAX_UPDATES_PER_HOUR",
    "MAX_USER_CONNECTIONS",
    "PASSWORD_LOCK_TIME",
];

/// O(1) lookup set for [`MYSQL_ACCOUNT_OPTION_KEYWORDS`].
static MYSQL_ACCOUNT_OPTION_KEYWORDS_SET: Lazy<HashSet<&'static str>> =
    Lazy::new(|| MYSQL_ACCOUNT_OPTION_KEYWORDS.iter().copied().collect());

/// MySQL/MariaDB statement modifiers that are syntax-only heads/modifiers and
/// should not be offered as operands in value-expression slots.
const MYSQL_NON_EXPRESSION_MODIFIER_KEYWORDS: &[&str] = &[
    "HIGH_PRIORITY",
    "NO_WRITE_TO_BINLOG",
    "SQL_BIG_RESULT",
    "SQL_BUFFER_RESULT",
    "SQL_CACHE",
    "SQL_CALC_FOUND_ROWS",
    "SQL_NO_CACHE",
    "SQL_SMALL_RESULT",
    "STRAIGHT_JOIN",
];

/// O(1) lookup set for [`MYSQL_NON_EXPRESSION_MODIFIER_KEYWORDS`].
static MYSQL_NON_EXPRESSION_MODIFIER_KEYWORDS_SET: Lazy<HashSet<&'static str>> = Lazy::new(|| {
    MYSQL_NON_EXPRESSION_MODIFIER_KEYWORDS
        .iter()
        .copied()
        .collect()
});

/// Session/compiler parameters and routine/PL-SQL declaration property keywords.
/// They only ever appear in `ALTER SESSION SET …`, `SET TRANSACTION …`, a
/// `PRAGMA`, or a `CREATE FUNCTION/PROCEDURE … <property>` header — never as a
/// token in a value expression.
///
/// Conservative as elsewhere: the NLS *functions* (`NLSSORT`, `NLS_UPPER`, …)
/// and `SESSIONTIMEZONE` are distinct names handled by the function catalog and
/// are intentionally absent here so they keep completing.
const SESSION_AND_DECLARATION_KEYWORDS: &[&str] = &[
    // ALTER SESSION / globalization parameters.
    "NLS_CALENDAR",
    "NLS_COMP",
    "NLS_CURRENCY",
    "NLS_DATE_FORMAT",
    "NLS_DATE_LANGUAGE",
    "NLS_ISO_CURRENCY",
    "NLS_LANGUAGE",
    "NLS_LENGTH_SEMANTICS",
    "NLS_NCHAR_CONV_EXCP",
    "NLS_NUMERIC_CHARACTERS",
    "NLS_SORT",
    "NLS_TERRITORY",
    "NLS_TIMESTAMP_FORMAT",
    "NLS_TIMESTAMP_TZ_FORMAT",
    "USING_NLS_COMP",
    // PL/SQL compiler / session parameters.
    "PLSQL_CCFLAGS",
    "PLSQL_CODE_TYPE",
    "PLSQL_DEBUG",
    "PLSQL_OPTIMIZE_LEVEL",
    "PLSQL_WARNINGS",
    "PLSCOPE_SETTINGS",
    "OPTIMIZER_MODE",
    "ISOLATION_LEVEL",
    "STATISTICS_LEVEL",
    "SQL_TRACE",
    "TRACEFILE_IDENTIFIER",
    "SERIALIZABLE",
    "ADVISE",
    "CONTAINER",
    "SHARING",
    // Routine / PL-SQL declaration properties.
    "PRAGMA",
    "AUTONOMOUS_TRANSACTION",
    "EXCEPTION_INIT",
    "ACCESSIBLE",
    "AUTHID",
    "PIPELINED",
    "DETERMINISTIC",
    "RESULT_CACHE",
    "PARALLEL_ENABLE",
    "WRAPPED",
    "WRAPPER",
];

/// O(1) lookup set for [`SESSION_AND_DECLARATION_KEYWORDS`].
static SESSION_AND_DECLARATION_KEYWORDS_SET: Lazy<HashSet<&'static str>> =
    Lazy::new(|| SESSION_AND_DECLARATION_KEYWORDS.iter().copied().collect());

/// SQL*Plus `SET` option keywords that double as a value-expression token and so
/// must stay available even though they are listed in
/// [`SQLPLUS_SET_OPTION_KEYWORDS`]: `NULL` (the literal/`IS NULL`), `ESCAPE`
/// (`LIKE … ESCAPE`), `LONG` (a data type), `CONCAT` (the string function).
fn is_expression_valid_sqlplus_option_upper(word_upper: &str) -> bool {
    matches!(word_upper, "NULL" | "ESCAPE" | "LONG" | "CONCAT")
}

/// Single chokepoint deciding whether an (already ASCII-uppercased) catalog
/// keyword can never appear in a value expression and must be dropped from
/// IntelliSense in an expression/column context. Unifies the statement/command
/// heads, the DDL-only clause modifiers, the MySQL admin/utility commands, the
/// session/declaration parameters, and the SQL*Plus `SET` options (minus the
/// few that double as expression tokens) so every caller stays consistent and
/// new keyword families are added in one place.
pub(crate) fn is_non_expression_keyword_upper(word_upper: &str) -> bool {
    is_non_expression_statement_keyword_upper(word_upper)
        || is_ddl_only_clause_keyword_upper(word_upper)
        || MYSQL_NON_EXPRESSION_COMMAND_KEYWORDS_SET.contains(word_upper)
        || MYSQL_DDL_FRAGMENT_KEYWORDS_SET.contains(word_upper)
        || MYSQL_ADMIN_OPTION_KEYWORDS_SET.contains(word_upper)
        || MYSQL_DYNAMIC_PRIVILEGE_KEYWORDS_SET.contains(word_upper)
        || MYSQL_ACCOUNT_OPTION_KEYWORDS_SET.contains(word_upper)
        || MYSQL_NON_EXPRESSION_MODIFIER_KEYWORDS_SET.contains(word_upper)
        || SESSION_AND_DECLARATION_KEYWORDS_SET.contains(word_upper)
        || (is_sqlplus_set_option_keyword(word_upper)
            && !is_expression_valid_sqlplus_option_upper(word_upper))
}

/// Returns true when `word` is a SQL*Plus `SET` option keyword.
pub(crate) fn is_sqlplus_set_option_keyword(word: &str) -> bool {
    matches_keyword(word, SQLPLUS_SET_OPTION_KEYWORDS)
}

pub(crate) fn is_auto_terminated_tool_command(line: &str) -> bool {
    let trimmed = line.trim();
    if trimmed.is_empty() {
        return false;
    }

    if trimmed.starts_with("@@") || trimmed.starts_with('@') {
        return true;
    }

    if trimmed.starts_with('!') {
        return true;
    }

    let Some(first) = next_meaningful_word(trimmed, 0).map(|(word, _)| word) else {
        return false;
    };

    let first_upper = first.to_ascii_uppercase();
    match first_upper.as_str() {
        // Keywords requiring second-word disambiguation
        "START" => {
            let second = next_meaningful_word(trimmed, 1).map(|(word, _)| word);
            second.is_some_and(|word| !word.eq_ignore_ascii_case("WITH"))
        }
        "R" => next_meaningful_word(trimmed, 1).is_none(),
        "CONNECT" => next_meaningful_word(trimmed, 1)
            .map(|(word, _)| word)
            .is_some_and(|second| !second.eq_ignore_ascii_case("BY")),
        "SET" => {
            let Some(second) = next_meaningful_word(trimmed, 1).map(|(word, _)| word) else {
                return false;
            };
            is_sqlplus_set_option_keyword(second)
        }
        "SHOW" => next_meaningful_word(trimmed, 1).is_some(),
        // PASSWORD abbreviations
        "PASSW" | "PASSWO" | "PASSWOR" | "PASSWORD" => true,
        // Simple auto-terminated keywords (no second-word check needed)
        "DISC" | "DISCONNECT" | "CONN" | "RUN" | "EXIT" | "QUIT" | "STARTUP" | "SHUTDOWN"
        | "RECOVER" | "ARCHIVE" | "HOST" | "TIMING" | "TTITLE" | "BTITLE" | "REPHEADER"
        | "REPFOOTER" | "PROMPT" | "REM" | "REMARK" | "SPOOL" | "STORE" | "GET" | "SAVE"
        | "DESCRIBE" | "DESC" | "DEFINE" | "UNDEFINE" | "VARIABLE" | "VAR" | "PRINT" | "ACCEPT"
        | "PAUSE" | "WHENEVER" | "COLUMN" | "BREAK" | "CLEAR" | "COMPUTE" => true,
        "EXEC" | "EXECUTE" => {
            let tail = trimmed.get(first.len()..).unwrap_or_default().trim();
            !tail.strip_prefix(':').is_some_and(|bind_name| {
                !bind_name.is_empty()
                    && bind_name.bytes().all(|byte| {
                        byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'$' | b'#')
                    })
            })
        }
        _ => false,
    }
}

fn next_meaningful_word(line: &str, skip_words: usize) -> Option<(&str, usize)> {
    let mut idx = 0usize;
    let mut seen_words = 0usize;

    while idx < line.len() {
        if sql_line_comment_prefix_len(line.as_bytes(), idx).is_some() {
            return None;
        }

        if line[idx..].starts_with("/*") {
            let block_start = idx + 2;
            let block_end = line[block_start..].find("*/")?;
            idx = block_start + block_end + 2;
            continue;
        }

        let ch = line[idx..].chars().next()?;
        let ch_len = ch.len_utf8();

        if ch.is_whitespace() {
            idx += ch_len;
            continue;
        }

        let mut end = idx;
        while end < line.len() {
            let word_ch = line[end..].chars().next()?;
            if word_ch.is_whitespace() || line[end..].starts_with("/*") {
                break;
            }
            if sql_line_comment_prefix_len(line.as_bytes(), end).is_some() {
                break;
            }
            end += word_ch.len_utf8();
        }

        if seen_words == skip_words {
            return Some((&line[idx..end], idx));
        }

        seen_words += 1;
        idx = end;
    }

    None
}

/// Returns the first meaningful token-like word on a line, skipping
/// leading whitespace and comments.
pub(crate) fn first_meaningful_word(line: &str) -> Option<&str> {
    next_meaningful_word(line, 0).map(|(word, _)| word)
}

/// Returns identifier tokens in source order before an inline comment or end
/// of line, skipping quoted literals, quoted identifiers, and block comments.
pub(crate) fn meaningful_identifier_words_before_inline_comment(
    line: &str,
    max_identifiers: usize,
) -> Vec<&str> {
    let bytes = line.as_bytes();
    let mut idx = 0usize;
    let mut in_single_quote = false;
    let mut in_double_quote = false;
    let mut in_block_comment = false;
    let mut q_quote_end: Option<u8> = None;
    let mut identifiers = Vec::with_capacity(max_identifiers);

    while idx < bytes.len() {
        let current = bytes[idx];
        let next = bytes.get(idx.saturating_add(1)).copied();

        if in_block_comment {
            if current == b'*' && next == Some(b'/') {
                in_block_comment = false;
                idx = idx.saturating_add(2);
                continue;
            }
            idx = idx.saturating_add(1);
            continue;
        }

        if let Some(closing) = q_quote_end {
            if current == closing && next == Some(b'\'') {
                q_quote_end = None;
                idx = idx.saturating_add(2);
                continue;
            }
            idx = idx.saturating_add(1);
            continue;
        }

        if in_single_quote {
            if current == b'\'' {
                if next == Some(b'\'') {
                    idx = idx.saturating_add(2);
                    continue;
                }
                in_single_quote = false;
            }
            idx = idx.saturating_add(1);
            continue;
        }

        if in_double_quote {
            if current == b'"' {
                if next == Some(b'"') {
                    idx = idx.saturating_add(2);
                    continue;
                }
                in_double_quote = false;
            }
            idx = idx.saturating_add(1);
            continue;
        }

        if sql_line_comment_prefix_len(bytes, idx).is_some() {
            break;
        }
        if current == b'/' && next == Some(b'*') {
            in_block_comment = true;
            idx = idx.saturating_add(2);
            continue;
        }
        if let Some(q_quote_start) = q_quote_prefix_at(bytes, idx) {
            q_quote_end = Some(q_quote_start.closing_delimiter);
            idx = idx.saturating_add(q_quote_start.prefix_len);
            continue;
        }
        if current == b'\'' {
            in_single_quote = true;
            idx = idx.saturating_add(1);
            continue;
        }
        if current == b'"' {
            in_double_quote = true;
            idx = idx.saturating_add(1);
            continue;
        }
        if current.is_ascii_whitespace() {
            idx = idx.saturating_add(1);
            continue;
        }

        if is_identifier_start_byte(current) {
            let start = idx;
            idx = idx.saturating_add(1);
            while idx < bytes.len() && is_identifier_byte(bytes[idx]) {
                idx = idx.saturating_add(1);
            }
            if let Some(identifier) = line.get(start..idx) {
                identifiers.push(identifier);
                if identifiers.len() == max_identifiers {
                    break;
                }
            }
            continue;
        }

        idx = idx.saturating_add(1);
    }

    identifiers
}

pub(crate) fn meaningful_identifier_words_array_before_inline_comment<const N: usize>(
    line: &str,
) -> [Option<&str>; N] {
    let mut identifiers = [None; N];
    for (slot, word) in identifiers
        .iter_mut()
        .zip(meaningful_identifier_words_before_inline_comment(line, N).into_iter())
    {
        *slot = Some(word);
    }

    identifiers
}

pub(crate) fn identifier_words_start_with(words: &[Option<&str>], sequence: &[&str]) -> bool {
    sequence.iter().enumerate().all(|(idx, expected)| {
        words
            .get(idx)
            .copied()
            .flatten()
            .is_some_and(|word| word.eq_ignore_ascii_case(expected))
    })
}

pub(crate) fn identifier_words_exact(words: &[Option<&str>], sequence: &[&str]) -> bool {
    identifier_words_start_with(words, sequence)
        && words.get(sequence.len()).copied().flatten().is_none()
}

pub(crate) fn identifier_words_first_is(words: &[Option<&str>], keyword: &str) -> bool {
    words
        .first()
        .copied()
        .flatten()
        .is_some_and(|word| word.eq_ignore_ascii_case(keyword))
}

fn leading_identifier_words(line: &str, max_identifiers: usize) -> Vec<&str> {
    let bytes = line.as_bytes();
    let mut idx = 0usize;
    let mut identifiers = Vec::with_capacity(max_identifiers);

    while idx < bytes.len() && identifiers.len() < max_identifiers {
        while idx < bytes.len() {
            let current = bytes[idx];
            let next = bytes.get(idx.saturating_add(1)).copied();
            if current.is_ascii_whitespace() {
                idx = idx.saturating_add(1);
                continue;
            }
            if sql_line_comment_prefix_len(bytes, idx).is_some() {
                return identifiers;
            }
            if current == b'/' && next == Some(b'*') {
                idx = idx.saturating_add(2);
                let mut closed_comment = false;
                while idx + 1 < bytes.len() {
                    if bytes[idx] == b'*' && bytes[idx + 1] == b'/' {
                        idx = idx.saturating_add(2);
                        closed_comment = true;
                        break;
                    }
                    idx = idx.saturating_add(1);
                }
                if !closed_comment {
                    return identifiers;
                }
                continue;
            }
            break;
        }

        if idx >= bytes.len() || !is_identifier_start_byte(bytes[idx]) {
            break;
        }

        let start = idx;
        idx = idx.saturating_add(1);
        while idx < bytes.len() && is_identifier_byte(bytes[idx]) {
            idx = idx.saturating_add(1);
        }

        if let Some(identifier) = line.get(start..idx) {
            identifiers.push(identifier);
        } else {
            break;
        }
    }

    identifiers
}

fn next_identifier_word_segment<'a>(word: &'a str, idx: &mut usize) -> Option<&'a str> {
    let bytes = word.as_bytes();

    while *idx < bytes.len() && bytes[*idx] == b'_' {
        *idx = idx.saturating_add(1);
    }
    if *idx >= bytes.len() {
        return None;
    }

    let start = *idx;
    while *idx < bytes.len() && bytes[*idx] != b'_' {
        *idx = idx.saturating_add(1);
    }

    word.get(start..*idx)
}

fn identifier_segment_count(word: &str) -> usize {
    let bytes = word.as_bytes();
    let mut idx = 0usize;
    let mut count = 0usize;

    while idx < bytes.len() {
        while idx < bytes.len() && bytes[idx] == b'_' {
            idx = idx.saturating_add(1);
        }
        if idx >= bytes.len() {
            break;
        }

        count = count.saturating_add(1);
        while idx < bytes.len() && bytes[idx] != b'_' {
            idx = idx.saturating_add(1);
        }
    }

    count
}

fn next_identifier_sequence_segment<'a>(
    sequence: &'a [&'a str],
    word_idx: &mut usize,
    segment_idx: &mut usize,
) -> Option<&'a str> {
    while *word_idx < sequence.len() {
        if let Some(segment) = next_identifier_word_segment(sequence[*word_idx], segment_idx) {
            return Some(segment);
        }

        *word_idx = word_idx.saturating_add(1);
        *segment_idx = 0;
    }

    None
}

fn identifier_sequence_segment_count(sequence: &[&str]) -> usize {
    sequence
        .iter()
        .map(|word| identifier_segment_count(word))
        .sum()
}

fn identifier_word_matches_keyword_sequence(words: &[&str], sequence: &[&str]) -> bool {
    let mut expected_word_idx = 0usize;
    let mut expected_segment_idx = 0usize;

    for word in words {
        let mut actual_segment_idx = 0usize;
        while let Some(segment) = next_identifier_word_segment(word, &mut actual_segment_idx) {
            let Some(expected) = next_identifier_sequence_segment(
                sequence,
                &mut expected_word_idx,
                &mut expected_segment_idx,
            ) else {
                return false;
            };
            if !segment.as_bytes().eq_ignore_ascii_case(expected.as_bytes()) {
                return false;
            }
        }
    }

    next_identifier_sequence_segment(sequence, &mut expected_word_idx, &mut expected_segment_idx)
        .is_none()
}

fn line_starts_with_identifier_sequence(line: &str, sequence: &[&str]) -> bool {
    if sequence.is_empty() {
        return true;
    }

    let expected_segment_count = identifier_sequence_segment_count(sequence);
    let bytes = line.as_bytes();
    let mut idx = 0usize;
    let mut matched_segments = 0usize;
    let mut expected_word_idx = 0usize;
    let mut expected_segment_idx = 0usize;

    while idx < bytes.len() && matched_segments < expected_segment_count {
        while idx < bytes.len() {
            let current = bytes[idx];
            let next = bytes.get(idx.saturating_add(1)).copied();
            if current.is_ascii_whitespace() {
                idx = idx.saturating_add(1);
                continue;
            }
            if sql_line_comment_prefix_len(bytes, idx).is_some() {
                return false;
            }
            if current == b'/' && next == Some(b'*') {
                idx = idx.saturating_add(2);
                let mut closed_comment = false;
                while idx + 1 < bytes.len() {
                    if bytes[idx] == b'*' && bytes[idx + 1] == b'/' {
                        idx = idx.saturating_add(2);
                        closed_comment = true;
                        break;
                    }
                    idx = idx.saturating_add(1);
                }
                if !closed_comment {
                    return false;
                }
                continue;
            }
            break;
        }

        if idx >= bytes.len() || !is_identifier_start_byte(bytes[idx]) {
            return false;
        }

        let start = idx;
        idx = idx.saturating_add(1);
        while idx < bytes.len() && is_identifier_byte(bytes[idx]) {
            idx = idx.saturating_add(1);
        }

        let Some(identifier) = line.get(start..idx) else {
            return false;
        };

        let mut actual_segment_idx = 0usize;
        while let Some(segment) = next_identifier_word_segment(identifier, &mut actual_segment_idx)
        {
            let Some(expected) = next_identifier_sequence_segment(
                sequence,
                &mut expected_word_idx,
                &mut expected_segment_idx,
            ) else {
                return false;
            };
            if !segment.as_bytes().eq_ignore_ascii_case(expected.as_bytes()) {
                return false;
            }
            matched_segments = matched_segments.saturating_add(1);
        }
    }

    matched_segments == expected_segment_count
}

fn line_has_exact_identifier_sequence(line: &str, sequence: &[&str]) -> bool {
    if sequence.is_empty() {
        return auto_format_structural_tail(line).trim().is_empty();
    }

    let expected_segments = sequence
        .iter()
        .flat_map(|word| word.split('_').filter(|segment| !segment.is_empty()))
        .collect::<Vec<_>>();
    let bytes = line.as_bytes();
    let mut idx = 0usize;
    let mut matched_segments = 0usize;

    while idx < bytes.len() && matched_segments < expected_segments.len() {
        while idx < bytes.len() {
            let current = bytes[idx];
            let next = bytes.get(idx.saturating_add(1)).copied();
            if current.is_ascii_whitespace() {
                idx = idx.saturating_add(1);
                continue;
            }
            if sql_line_comment_prefix_len(bytes, idx).is_some() {
                return false;
            }
            if current == b'/' && next == Some(b'*') {
                idx = idx.saturating_add(2);
                while idx + 1 < bytes.len() {
                    if bytes[idx] == b'*' && bytes[idx + 1] == b'/' {
                        idx = idx.saturating_add(2);
                        break;
                    }
                    idx = idx.saturating_add(1);
                }
                continue;
            }
            break;
        }

        if idx >= bytes.len() || !is_identifier_start_byte(bytes[idx]) {
            return false;
        }

        let start = idx;
        idx = idx.saturating_add(1);
        while idx < bytes.len() && is_identifier_byte(bytes[idx]) {
            idx = idx.saturating_add(1);
        }

        let Some(identifier) = line.get(start..idx) else {
            return false;
        };

        let mut segment_idx = 0usize;
        while let Some(segment) = next_identifier_word_segment(identifier, &mut segment_idx) {
            let Some(expected) = expected_segments.get(matched_segments) else {
                return false;
            };
            if !segment.as_bytes().eq_ignore_ascii_case(expected.as_bytes()) {
                return false;
            }
            matched_segments = matched_segments.saturating_add(1);
        }
    }

    if matched_segments != expected_segments.len() {
        return false;
    }

    while idx < bytes.len() {
        let current = bytes[idx];
        let next = bytes.get(idx.saturating_add(1)).copied();
        if current.is_ascii_whitespace() {
            idx = idx.saturating_add(1);
            continue;
        }
        if sql_line_comment_prefix_len(bytes, idx).is_some() {
            return true;
        }
        if current == b'/' && next == Some(b'*') {
            idx = idx.saturating_add(2);
            let mut closed_comment = false;
            while idx + 1 < bytes.len() {
                if bytes[idx] == b'*' && bytes[idx + 1] == b'/' {
                    idx = idx.saturating_add(2);
                    closed_comment = true;
                    break;
                }
                idx = idx.saturating_add(1);
            }
            if !closed_comment {
                return true;
            }
            continue;
        }
        return false;
    }

    true
}

fn line_has_exact_identifier_sequence_then_open_paren_before_inline_comment(
    line: &str,
    sequence: &[&str],
) -> bool {
    if sequence.is_empty() {
        return false;
    }

    let trimmed = trim_leading_sql_comments(line);
    if trimmed.is_empty() {
        return false;
    }

    let expected_segments = sequence
        .iter()
        .flat_map(|word| word.split('_').filter(|segment| !segment.is_empty()))
        .collect::<Vec<_>>();
    let bytes = trimmed.as_bytes();
    let mut idx = 0usize;
    let mut matched_segments = 0usize;

    while idx < bytes.len() && matched_segments < expected_segments.len() {
        while idx < bytes.len() {
            let current = bytes[idx];
            let next = bytes.get(idx.saturating_add(1)).copied();
            if current.is_ascii_whitespace() {
                idx = idx.saturating_add(1);
                continue;
            }
            if sql_line_comment_prefix_len(bytes, idx).is_some() {
                return false;
            }
            if current == b'/' && next == Some(b'*') {
                idx = idx.saturating_add(2);
                while idx + 1 < bytes.len() {
                    if bytes[idx] == b'*' && bytes[idx + 1] == b'/' {
                        idx = idx.saturating_add(2);
                        break;
                    }
                    idx = idx.saturating_add(1);
                }
                continue;
            }
            break;
        }

        if idx >= bytes.len() || !is_identifier_start_byte(bytes[idx]) {
            return false;
        }

        let start = idx;
        idx = idx.saturating_add(1);
        while idx < bytes.len() && is_identifier_byte(bytes[idx]) {
            idx = idx.saturating_add(1);
        }

        let Some(identifier) = trimmed.get(start..idx) else {
            return false;
        };

        let mut segment_idx = 0usize;
        while let Some(segment) = next_identifier_word_segment(identifier, &mut segment_idx) {
            let Some(expected) = expected_segments.get(matched_segments) else {
                return false;
            };
            if !segment.as_bytes().eq_ignore_ascii_case(expected.as_bytes()) {
                return false;
            }
            matched_segments = matched_segments.saturating_add(1);
        }
    }

    if matched_segments != expected_segments.len() {
        return false;
    }

    while idx < bytes.len() {
        let current = bytes[idx];
        let next = bytes.get(idx.saturating_add(1)).copied();
        if current.is_ascii_whitespace() {
            idx = idx.saturating_add(1);
            continue;
        }
        if sql_line_comment_prefix_len(bytes, idx).is_some() {
            return false;
        }
        if current == b'/' && next == Some(b'*') {
            idx = idx.saturating_add(2);
            while idx + 1 < bytes.len() {
                if bytes[idx] == b'*' && bytes[idx + 1] == b'/' {
                    idx = idx.saturating_add(2);
                    break;
                }
                idx = idx.saturating_add(1);
            }
            continue;
        }
        if current != b'(' {
            return false;
        }
        idx = idx.saturating_add(1);
        break;
    }

    if idx == 0 || bytes.get(idx.saturating_sub(1)) != Some(&b'(') {
        return false;
    }

    while idx < bytes.len() {
        let current = bytes[idx];
        let next = bytes.get(idx.saturating_add(1)).copied();
        if current.is_ascii_whitespace() {
            idx = idx.saturating_add(1);
            continue;
        }
        if sql_line_comment_prefix_len(bytes, idx).is_some() {
            return true;
        }
        if current == b'/' && next == Some(b'*') {
            idx = idx.saturating_add(2);
            while idx + 1 < bytes.len() {
                if bytes[idx] == b'*' && bytes[idx + 1] == b'/' {
                    idx = idx.saturating_add(2);
                    break;
                }
                idx = idx.saturating_add(1);
            }
            continue;
        }
        return false;
    }

    true
}

/// Returns true when the leading structural identifier sequence on `line`
/// matches `sequence`, skipping only whitespace and block comments between
/// identifiers.
pub(crate) fn line_starts_with_identifier_sequence_before_inline_comment(
    line: &str,
    sequence: &[&str],
) -> bool {
    line_starts_with_identifier_sequence(line, sequence)
}

pub(crate) fn line_has_exact_identifier_sequence_before_inline_comment(
    line: &str,
    sequence: &[&str],
) -> bool {
    line_has_exact_identifier_sequence(line, sequence)
}

/// Returns true when `line` is an exact bare control-condition header whose
/// only structural token after the header sequence is a standalone `(`.
///
/// This lets analyzer and formatter lookahead logic treat `IF (`, `ELSIF (`,
/// `ELSEIF (`, `WHILE (`, and `WHEN (` the same way even when whitespace or
/// inline block comments split the header from the opening paren.
pub(crate) fn line_is_bare_parenthesized_condition_header(line: &str) -> bool {
    line_has_exact_identifier_sequence_then_open_paren_before_inline_comment(line, &["IF"])
        || line_has_exact_identifier_sequence_then_open_paren_before_inline_comment(
            line,
            &["ELSIF"],
        )
        || line_has_exact_identifier_sequence_then_open_paren_before_inline_comment(
            line,
            &["ELSEIF"],
        )
        || line_has_exact_identifier_sequence_then_open_paren_before_inline_comment(
            line,
            &["WHILE"],
        )
        || line_has_exact_identifier_sequence_then_open_paren_before_inline_comment(line, &["WHEN"])
}

pub(crate) fn is_format_block_end_qualifier_keyword(word: &str) -> bool {
    matches_keyword(word, FORMAT_BLOCK_END_QUALIFIER_KEYWORDS)
}

pub(crate) fn is_format_plain_end_suffix_keyword(word: &str) -> bool {
    matches_keyword(word, FORMAT_PLAIN_END_SUFFIX_LEADING_KEYWORDS)
}

#[cfg_attr(not(test), allow(dead_code))]
pub(crate) fn starts_with_format_end_suffix_terminator(text_upper: &str) -> bool {
    let words = leading_identifier_words(text_upper, 3);
    words
        .first()
        .is_some_and(|word| word.eq_ignore_ascii_case("END"))
        && words
            .get(1)
            .is_some_and(|word| is_format_plain_end_suffix_keyword(word))
}

pub(crate) fn starts_with_format_plain_end(text_upper: &str) -> bool {
    let words = leading_identifier_words(text_upper, 2);
    words
        .first()
        .is_some_and(|word| word.eq_ignore_ascii_case("END"))
        && !words
            .get(1)
            .is_some_and(|word| is_format_plain_end_suffix_keyword(word))
}

pub(crate) fn starts_with_format_bare_end(text_upper: &str) -> bool {
    if !starts_with_format_plain_end(text_upper) {
        return false;
    }

    let bytes = text_upper.as_bytes();
    let mut idx = 0usize;

    while idx < bytes.len() {
        let current = bytes[idx];
        let next = bytes.get(idx.saturating_add(1)).copied();
        if current.is_ascii_whitespace() {
            idx = idx.saturating_add(1);
            continue;
        }
        if current == b'/' && next == Some(b'*') {
            idx = idx.saturating_add(2);
            while idx + 1 < bytes.len() {
                if bytes[idx] == b'*' && bytes[idx + 1] == b'/' {
                    idx = idx.saturating_add(2);
                    break;
                }
                idx = idx.saturating_add(1);
            }
            continue;
        }
        break;
    }

    if idx >= bytes.len() || !is_identifier_start_byte(bytes[idx]) {
        return false;
    }

    idx = idx.saturating_add(1);
    while idx < bytes.len() && is_identifier_byte(bytes[idx]) {
        idx = idx.saturating_add(1);
    }

    while idx < bytes.len() {
        let current = bytes[idx];
        let next = bytes.get(idx.saturating_add(1)).copied();
        if current.is_ascii_whitespace() {
            idx = idx.saturating_add(1);
            continue;
        }
        if current == b'/' && next == Some(b'*') {
            idx = idx.saturating_add(2);
            while idx + 1 < bytes.len() {
                if bytes[idx] == b'*' && bytes[idx + 1] == b'/' {
                    idx = idx.saturating_add(2);
                    break;
                }
                idx = idx.saturating_add(1);
            }
            continue;
        }
        return current == b';';
    }

    true
}

pub(crate) fn starts_with_format_named_plain_end(text_upper: &str) -> bool {
    starts_with_format_plain_end(text_upper) && !starts_with_format_bare_end(text_upper)
}

/// Returns true when a keyword can head a subquery body after `(`.
pub(crate) fn is_subquery_head_keyword(word: &str) -> bool {
    matches_keyword(word, SUBQUERY_HEAD_KEYWORDS)
}

/// Returns true when a CTE state machine should recover back to normal parsing.
pub(crate) fn is_cte_recovery_keyword(word: &str) -> bool {
    is_subquery_head_keyword(word)
}

/// Returns true when a token is a valid Oracle `EXTERNAL LANGUAGE` target.
pub(crate) fn is_external_language_target_keyword(word: &str) -> bool {
    matches_keyword(word, EXTERNAL_LANGUAGE_TARGET_KEYWORDS)
}

/// Returns true when a token participates in Oracle EXTERNAL routine clauses.
pub(crate) fn is_external_language_clause_keyword(word: &str) -> bool {
    matches_keyword(word, EXTERNAL_LANGUAGE_CLAUSE_KEYWORDS)
}

/// Returns true when a token starts a CREATE TABLE column constraint section.
pub(crate) fn is_format_column_constraint_keyword(word: &str) -> bool {
    matches_keyword(word, FORMAT_COLUMN_CONSTRAINT_KEYWORDS)
}

/// Returns true when `text_upper` starts with a formatter clause that should
/// behave as a stable layout anchor in the indentation pass.
pub(crate) fn starts_with_format_layout_clause(text_upper: &str) -> bool {
    FORMAT_LAYOUT_CLAUSE_START_KEYWORDS
        .iter()
        .any(|keyword| line_starts_with_identifier_sequence(text_upper, &[*keyword]))
}

/// Returns true for functions whose syntax consumes `FROM` inside the
/// function call rather than starting a SQL `FROM` clause.
pub(crate) fn is_from_consuming_function(name: &str) -> bool {
    matches!(
        name,
        "EXTRACT" | "TRIM" | "SUBSTRING" | "OVERLAY" | "POSITION" | "NORMALIZE" | "TRIM_ARRAY"
    )
}

/// Function-local clause starters can appear inside ordinary parenthesized
/// expressions (`JSON_QUERY(... WITH WRAPPER)`,
/// `JSON_VALUE(... RETURNING VARCHAR2 (...))`,
/// `JSON_TRANSFORM(... SET ..., INSERT ...)`) without introducing a new
/// structural clause anchor or clause-break family.
pub(crate) fn is_non_subquery_paren_suppressed_clause_start(text_upper: &str) -> bool {
    let sequences: &[&[&str]] = &[&["RETURNING"], &["WITH"], &["SET"], &["INSERT"]];
    sequences
        .iter()
        .any(|sequence| line_starts_with_identifier_sequence(text_upper, sequence))
}

/// Function-local option continuation lines that can follow a suppressed
/// clause-start inside the same ordinary-paren frame.
///
/// Examples:
/// - `ON ERROR NULL`
/// - `ON EMPTY NULL`
/// - `ERROR ON ERROR`
/// - `NULL ON EMPTY`
pub(crate) fn is_non_subquery_paren_suppressed_clause_continuation(text_upper: &str) -> bool {
    let starts_on_error_or_empty =
        line_starts_with_identifier_sequence(text_upper, &["ON", "ERROR"])
            || line_starts_with_identifier_sequence(text_upper, &["ON", "EMPTY"]);
    if starts_on_error_or_empty {
        return true;
    }

    let words = meaningful_identifier_words_before_inline_comment(text_upper, 4);
    let is_error_or_empty =
        |word: &str| word.eq_ignore_ascii_case("ERROR") || word.eq_ignore_ascii_case("EMPTY");
    let is_scalar_option_head = |word: &str| {
        word.eq_ignore_ascii_case("ERROR")
            || word.eq_ignore_ascii_case("EMPTY")
            || word.eq_ignore_ascii_case("NULL")
            || word.eq_ignore_ascii_case("DEFAULT")
            || word.eq_ignore_ascii_case("TRUE")
            || word.eq_ignore_ascii_case("FALSE")
            || word.eq_ignore_ascii_case("UNKNOWN")
    };

    let starts_scalar_option = words.len() >= 3
        && words
            .get(1)
            .is_some_and(|word| word.eq_ignore_ascii_case("ON"))
        && words.get(2).is_some_and(|word| is_error_or_empty(word))
        && words
            .first()
            .is_some_and(|word| is_scalar_option_head(word));
    if starts_scalar_option {
        return true;
    }

    words.len() >= 4
        && words
            .first()
            .is_some_and(|word| word.eq_ignore_ascii_case("EMPTY"))
        && words.get(1).is_some_and(|word| {
            word.eq_ignore_ascii_case("ARRAY") || word.eq_ignore_ascii_case("OBJECT")
        })
        && words
            .get(2)
            .is_some_and(|word| word.eq_ignore_ascii_case("ON"))
        && words.get(3).is_some_and(|word| is_error_or_empty(word))
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum FormatSetOperatorKind {
    Union,
    Intersect,
    Minus,
    Except,
}

impl FormatSetOperatorKind {
    pub(crate) fn from_clause_start(text_upper: &str) -> Option<Self> {
        if line_starts_with_identifier_sequence(text_upper, &["UNION"]) {
            Some(Self::Union)
        } else if line_starts_with_identifier_sequence(text_upper, &["INTERSECT"]) {
            Some(Self::Intersect)
        } else if line_starts_with_identifier_sequence(text_upper, &["MINUS"]) {
            Some(Self::Minus)
        } else if line_starts_with_identifier_sequence(text_upper, &["EXCEPT"]) {
            Some(Self::Except)
        } else {
            None
        }
    }
}

pub(crate) fn is_format_set_operator_keyword(word: &str) -> bool {
    matches_keyword(word, FORMAT_SET_OPERATOR_KEYWORDS)
}

pub(crate) fn starts_with_format_set_operator(text_upper: &str) -> bool {
    FORMAT_SET_OPERATOR_KEYWORDS
        .iter()
        .any(|keyword| line_starts_with_identifier_sequence(text_upper, &[*keyword]))
}

pub(crate) fn starts_with_format_join_clause(text_upper: &str) -> bool {
    let words = leading_identifier_words(text_upper, 4);
    let Some(first_word) = words.first().copied() else {
        return false;
    };

    if first_word.eq_ignore_ascii_case("JOIN") || first_word.eq_ignore_ascii_case("APPLY") {
        return true;
    }

    let starts_with_join_modifier = first_word.eq_ignore_ascii_case("OUTER")
        || FORMAT_JOIN_MODIFIER_KEYWORDS
            .iter()
            .any(|keyword| first_word.eq_ignore_ascii_case(keyword));

    starts_with_join_modifier
        && words
            .iter()
            .skip(1)
            .any(|word| word.eq_ignore_ascii_case("JOIN") || word.eq_ignore_ascii_case("APPLY"))
}

pub(crate) fn is_format_join_condition_clause(text_upper: &str) -> bool {
    line_starts_with_identifier_sequence(text_upper, &["ON"])
        || line_starts_with_identifier_sequence(text_upper, &["USING"])
}

pub(crate) fn starts_with_format_for_update_split_header(text_upper: &str) -> bool {
    line_has_exact_identifier_sequence(owner_header_structural_tail(text_upper), &["FOR"])
}

pub(crate) fn starts_with_format_for_update_clause(text_upper: &str) -> bool {
    starts_with_format_for_update_split_header(text_upper)
        || line_starts_with_identifier_sequence(
            owner_header_structural_tail(text_upper),
            &["FOR", "UPDATE"],
        )
}

pub(crate) fn starts_with_format_merge_branch_header(text_upper: &str) -> bool {
    line_starts_with_identifier_sequence(text_upper, &["WHEN", "MATCHED"])
        || line_starts_with_identifier_sequence(text_upper, &["WHEN", "NOT", "MATCHED"])
}

pub(crate) fn starts_with_format_merge_branch_condition_clause(text_upper: &str) -> bool {
    line_starts_with_identifier_sequence(text_upper, &["AND"])
        || line_starts_with_identifier_sequence(text_upper, &["OR"])
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum PendingFormatMergeBranchHeaderKind {
    When,
    WhenNot,
    Condition,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct PendingFormatMergeBranchHeaderProgress {
    pub(crate) next_kind: Option<PendingFormatMergeBranchHeaderKind>,
    pub(crate) completed: bool,
    pub(crate) uses_condition_depth: bool,
}

impl PendingFormatMergeBranchHeaderKind {
    pub(crate) fn progress_over_line(
        self,
        line: &str,
    ) -> Option<PendingFormatMergeBranchHeaderProgress> {
        let structural_tail = auto_format_structural_tail(line);

        match self {
            Self::When => {
                if line_starts_with_identifier_sequence(structural_tail, &["NOT", "MATCHED"])
                    || line_starts_with_identifier_sequence(
                        structural_tail,
                        &["WHEN", "NOT", "MATCHED"],
                    )
                {
                    let completed = line_ends_with_identifier_sequence_before_inline_comment(
                        structural_tail,
                        &["THEN"],
                    );
                    return Some(PendingFormatMergeBranchHeaderProgress {
                        next_kind: (!completed).then_some(Self::Condition),
                        completed,
                        uses_condition_depth: false,
                    });
                }

                if line_starts_with_identifier_sequence(structural_tail, &["MATCHED"])
                    || line_starts_with_identifier_sequence(structural_tail, &["WHEN", "MATCHED"])
                {
                    let completed = line_ends_with_identifier_sequence_before_inline_comment(
                        structural_tail,
                        &["THEN"],
                    );
                    return Some(PendingFormatMergeBranchHeaderProgress {
                        next_kind: (!completed).then_some(Self::Condition),
                        completed,
                        uses_condition_depth: false,
                    });
                }

                if line_starts_with_identifier_sequence(structural_tail, &["NOT"])
                    || line_starts_with_identifier_sequence(structural_tail, &["WHEN", "NOT"])
                {
                    return Some(PendingFormatMergeBranchHeaderProgress {
                        next_kind: Some(Self::WhenNot),
                        completed: false,
                        uses_condition_depth: false,
                    });
                }

                None
            }
            Self::WhenNot => {
                if line_starts_with_identifier_sequence(structural_tail, &["MATCHED"])
                    || line_starts_with_identifier_sequence(
                        structural_tail,
                        &["WHEN", "NOT", "MATCHED"],
                    )
                {
                    let completed = line_ends_with_identifier_sequence_before_inline_comment(
                        structural_tail,
                        &["THEN"],
                    );
                    return Some(PendingFormatMergeBranchHeaderProgress {
                        next_kind: (!completed).then_some(Self::Condition),
                        completed,
                        uses_condition_depth: false,
                    });
                }

                None
            }
            Self::Condition => {
                if structural_tail.is_empty() {
                    return Some(PendingFormatMergeBranchHeaderProgress {
                        next_kind: Some(Self::Condition),
                        completed: false,
                        uses_condition_depth: true,
                    });
                }

                if line_starts_with_identifier_sequence(structural_tail, &["THEN"])
                    || line_ends_with_identifier_sequence_before_inline_comment(
                        structural_tail,
                        &["THEN"],
                    )
                {
                    return Some(PendingFormatMergeBranchHeaderProgress {
                        next_kind: None,
                        completed: true,
                        uses_condition_depth: true,
                    });
                }

                let structural_tail_upper = structural_tail.to_ascii_uppercase();
                if starts_with_format_merge_branch_condition_clause(&structural_tail_upper) {
                    return Some(PendingFormatMergeBranchHeaderProgress {
                        next_kind: Some(Self::Condition),
                        completed: false,
                        uses_condition_depth: true,
                    });
                }

                if starts_with_auto_format_owner_boundary(structural_tail) {
                    return None;
                }

                Some(PendingFormatMergeBranchHeaderProgress {
                    next_kind: Some(Self::Condition),
                    completed: false,
                    uses_condition_depth: true,
                })
            }
        }
    }
}

pub(crate) fn format_merge_branch_pending_header_kind(
    line: &str,
) -> Option<PendingFormatMergeBranchHeaderKind> {
    let structural_tail = auto_format_structural_tail(line);
    if structural_tail.is_empty()
        || line_ends_with_identifier_sequence_before_inline_comment(structural_tail, &["THEN"])
    {
        return None;
    }

    if line_starts_with_identifier_sequence(structural_tail, &["WHEN", "NOT", "MATCHED"])
        || line_starts_with_identifier_sequence(structural_tail, &["WHEN", "MATCHED"])
    {
        return Some(PendingFormatMergeBranchHeaderKind::Condition);
    }

    if line_has_exact_identifier_sequence(structural_tail, &["WHEN", "NOT"]) {
        return Some(PendingFormatMergeBranchHeaderKind::WhenNot);
    }

    line_has_exact_identifier_sequence(structural_tail, &["WHEN"])
        .then_some(PendingFormatMergeBranchHeaderKind::When)
}

/// Returns true when the current line is a same-depth MERGE branch-header
/// fragment that must stay on the dedicated MERGE pending-header state instead
/// of being reinterpreted as a generic query-owner fragment such as `NOT`.
///
/// This intentionally excludes condition-depth consumers like `AND NOT` inside
/// `WHEN MATCHED ...` so generic split owners like `NOT EXISTS` can still nest
/// inside a MERGE branch condition.
pub(crate) fn line_is_active_merge_branch_same_depth_header_fragment(
    line: &str,
    active_merge_base: bool,
    pending_kind: Option<PendingFormatMergeBranchHeaderKind>,
) -> bool {
    if !active_merge_base {
        return false;
    }

    if matches!(
        format_merge_branch_pending_header_kind(line),
        Some(PendingFormatMergeBranchHeaderKind::When)
            | Some(PendingFormatMergeBranchHeaderKind::WhenNot)
    ) {
        return true;
    }

    pending_kind
        .and_then(|kind| kind.progress_over_line(line))
        .is_some_and(|progress| !progress.uses_condition_depth)
}

/// Returns true when a retained `MERGE WHEN ... THEN` condition state should
/// stay suspended across the current line because the line opens or continues
/// a nested owner/query instead of consuming the branch header itself.
pub(crate) fn line_suspends_active_merge_branch_condition_state(
    line: &str,
    pending_kind: Option<PendingFormatMergeBranchHeaderKind>,
) -> bool {
    if pending_kind != Some(PendingFormatMergeBranchHeaderKind::Condition) {
        return false;
    }

    let structural_tail = auto_format_structural_tail(line);
    if structural_tail.is_empty() {
        return false;
    }

    let generic_condition_not_fragment = matches!(
        format_query_owner_pending_header_kind(structural_tail),
        Some(PendingFormatQueryOwnerHeaderKind::ConditionNot)
    ) && format_query_owner_header_kind(structural_tail)
        .is_none()
        && format_query_owner_kind(structural_tail).is_none();

    line_is_standalone_open_paren_before_inline_comment(structural_tail)
        || (starts_with_auto_format_owner_boundary(structural_tail)
            && !generic_condition_not_fragment)
}

fn starts_with_auto_format_structural_continuation_boundary_without_expression_owner_impl(
    line: &str,
) -> bool {
    let trimmed = line.trim_start();
    if trimmed.is_empty() {
        return false;
    }

    let trimmed_upper = trimmed.to_ascii_uppercase();
    line_starts_query_head(&trimmed_upper)
        || starts_with_format_layout_clause(&trimmed_upper)
        || line_starts_with_identifier_sequence(trimmed, &["INTO"])
        || line_starts_with_identifier_sequence(trimmed, &["USING"])
        || line_is_mysql_on_duplicate_key_update_clause(trimmed)
        || starts_with_format_join_clause(&trimmed_upper)
        || is_format_join_condition_clause(&trimmed_upper)
        || starts_with_format_for_update_clause(&trimmed_upper)
        || format_query_owner_pending_header_kind(trimmed).is_some()
        || format_indented_paren_pending_header_kind(trimmed).is_some()
        || starts_with_format_model_subclause(&trimmed_upper)
        || starts_with_format_match_recognize_subclause(&trimmed_upper)
        || starts_with_auto_format_owner_boundary_without_expression_owner(trimmed)
}

/// Shared structural boundary helper for continuation/comment carry.
///
/// This intentionally excludes generic expression owners such as `MULTISET`
/// so callers can keep operator RHS continuation on those lines when needed.
#[cfg(test)]
pub(crate) fn starts_with_auto_format_structural_continuation_boundary_without_expression_owner(
    line: &str,
) -> bool {
    starts_with_auto_format_structural_continuation_boundary_without_expression_owner_for_structural_tail(
        auto_format_structural_tail(line),
    )
}

#[cfg(test)]
pub(crate) fn starts_with_auto_format_structural_continuation_boundary_without_expression_owner_for_structural_tail(
    trimmed: &str,
) -> bool {
    starts_with_auto_format_structural_continuation_boundary_without_expression_owner_impl(trimmed)
}

/// Returns true when a line begins any shared structural continuation boundary
/// that must terminate carry into the next code line.
///
/// This extends the base boundary taxonomy with standalone wrapper `(` lines
/// and both complete and incomplete MERGE branch-header fragments so analyzer
/// and formatter phase 2 can stop clause/list/operator carry through the same
/// helper.
pub(crate) fn starts_with_auto_format_structural_continuation_boundary(line: &str) -> bool {
    starts_with_auto_format_structural_continuation_boundary_for_structural_tail(
        auto_format_structural_tail(line),
    )
}

pub(crate) fn starts_with_auto_format_structural_continuation_boundary_for_structural_tail(
    trimmed: &str,
) -> bool {
    if trimmed.is_empty() {
        return true;
    }

    let trimmed_upper = trimmed.to_ascii_uppercase();
    starts_with_auto_format_structural_continuation_boundary_without_expression_owner_impl(trimmed)
        || format_merge_branch_pending_header_kind(trimmed).is_some()
        || starts_with_format_merge_branch_header(&trimmed_upper)
        || line_is_standalone_open_paren_before_inline_comment(trimmed)
}

/// Returns true when a line is a CTE definition header that owns a following
/// query body through a trailing `AS (`.
///
/// This helper normalizes the shared structural tail first so analyzer-side
/// WITH sibling detection stays on the same taxonomy as formatter structural
/// helpers.
pub(crate) fn line_is_format_cte_definition_header(line: &str) -> bool {
    let structural_tail = auto_format_structural_tail(line);
    !structural_tail.is_empty()
        && line_ends_with_open_paren_before_inline_comment(structural_tail)
        && line_ends_with_identifier_sequence_before_inline_comment(structural_tail, &["AS"])
}

/// Returns true when a line is a named WINDOW definition header that owns a
/// following window specification through a trailing `AS (`.
///
/// Callers should only use this while a surrounding bare `WINDOW` clause is
/// active. The classifier itself stays lexical so analyzer and formatter share
/// one byte-safe rule.
pub(crate) fn line_is_format_window_definition_header(line: &str) -> bool {
    let structural_tail = auto_format_structural_tail(line);
    if structural_tail.is_empty()
        || !line_ends_with_open_paren_before_inline_comment(structural_tail)
        || !line_ends_with_identifier_sequence_before_inline_comment(structural_tail, &["AS"])
    {
        return false;
    }

    let first_word = next_meaningful_word(structural_tail.trim_start(), 0).map(|(word, _)| word);
    first_word.is_some_and(|word| {
        !word.eq_ignore_ascii_case("WINDOW") && !word.eq_ignore_ascii_case("WITH")
    })
}

/// Returns true when a line is the exact bare `WINDOW` clause header.
///
/// This stays lexical so analyzer and formatter can reuse the same header
/// anchor for sibling named window definitions inside larger query contexts.
pub(crate) fn line_is_format_bare_window_clause_header(line: &str) -> bool {
    let structural_tail = auto_format_structural_tail(line);
    line_has_exact_identifier_sequence(structural_tail, &["WINDOW"])
}

/// Returns true when a line starts MySQL's `ON DUPLICATE KEY UPDATE` clause.
///
/// The helper normalizes away inline comments first so continuation-boundary
/// detection can treat this clause like other structural clause starters
/// instead of leaking a previous `JOIN ... ON` condition depth into it.
pub(crate) fn line_is_mysql_on_duplicate_key_update_clause(line: &str) -> bool {
    let structural_tail = auto_format_structural_tail(line);
    line_starts_with_identifier_sequence(structural_tail, &["ON", "DUPLICATE", "KEY", "UPDATE"])
}

fn identifier_words_start_mysql_view_header(words: &[&str], mut idx: usize) -> bool {
    while idx < words.len() {
        let word = words[idx];
        if word.eq_ignore_ascii_case("VIEW") {
            return true;
        }

        if word.eq_ignore_ascii_case("ALGORITHM") {
            idx = idx.saturating_add(1);
            if !words.get(idx).is_some_and(|value| {
                matches!(
                    value.to_ascii_uppercase().as_str(),
                    "UNDEFINED" | "MERGE" | "TEMPTABLE"
                )
            }) {
                return false;
            }
            idx = idx.saturating_add(1);
            continue;
        }

        if word.eq_ignore_ascii_case("DEFINER") {
            idx = idx.saturating_add(1);
            while idx < words.len()
                && !words[idx].eq_ignore_ascii_case("SQL")
                && !words[idx].eq_ignore_ascii_case("VIEW")
            {
                idx = idx.saturating_add(1);
            }
            continue;
        }

        if word.eq_ignore_ascii_case("SQL")
            && words
                .get(idx.saturating_add(1))
                .is_some_and(|word| word.eq_ignore_ascii_case("SECURITY"))
            && words.get(idx.saturating_add(2)).is_some_and(|word| {
                word.eq_ignore_ascii_case("DEFINER") || word.eq_ignore_ascii_case("INVOKER")
            })
        {
            idx = idx.saturating_add(3);
            continue;
        }

        return false;
    }

    false
}

/// Returns true when a CREATE/ALTER query-body DDL header line owns the
/// following query body through a trailing `AS`.
fn line_starts_ddl_query_body_header_prefix(line: &str) -> bool {
    let structural_tail = owner_header_structural_tail(line);
    let words = meaningful_identifier_words_before_inline_comment(structural_tail, 32);
    let starts_create = words
        .first()
        .is_some_and(|word| word.eq_ignore_ascii_case("CREATE"));
    let starts_alter = words
        .first()
        .is_some_and(|word| word.eq_ignore_ascii_case("ALTER"));
    if !starts_create && !starts_alter {
        return false;
    }
    let mut idx = 1usize;

    while starts_create && idx < words.len() {
        match words.get(idx).copied() {
            Some(word)
                if word.eq_ignore_ascii_case("OR")
                    && words
                        .get(idx + 1)
                        .is_some_and(|word| word.eq_ignore_ascii_case("REPLACE")) =>
            {
                idx += 2;
            }
            Some(word)
                if word.eq_ignore_ascii_case("NO")
                    && words
                        .get(idx + 1)
                        .is_some_and(|word| word.eq_ignore_ascii_case("FORCE")) =>
            {
                idx += 2;
            }
            Some(word)
                if matches!(
                    word.to_ascii_uppercase().as_str(),
                    "FORCE" | "EDITIONABLE" | "NONEDITIONABLE" | "EDITIONING"
                ) =>
            {
                idx += 1;
            }
            _ => break,
        }
    }

    if identifier_words_start_mysql_view_header(&words, idx) {
        return true;
    }

    if starts_alter {
        return false;
    }

    if words
        .get(idx)
        .is_some_and(|word| word.eq_ignore_ascii_case("MATERIALIZED"))
        && words
            .get(idx + 1)
            .is_some_and(|word| word.eq_ignore_ascii_case("VIEW"))
    {
        return true;
    }

    if words
        .get(idx)
        .is_some_and(|word| word.eq_ignore_ascii_case("JSON"))
        && words
            .get(idx + 1)
            .is_some_and(|word| word.eq_ignore_ascii_case("RELATIONAL"))
        && words
            .get(idx + 2)
            .is_some_and(|word| word.eq_ignore_ascii_case("DUALITY"))
        && words
            .get(idx + 3)
            .is_some_and(|word| word.eq_ignore_ascii_case("VIEW"))
    {
        return true;
    }

    if words
        .get(idx)
        .is_some_and(|word| word.eq_ignore_ascii_case("TABLE"))
    {
        return true;
    }

    if words
        .get(idx)
        .is_some_and(|word| word.eq_ignore_ascii_case("TEMPORARY"))
        && words
            .get(idx + 1)
            .is_some_and(|word| word.eq_ignore_ascii_case("TABLE"))
    {
        return true;
    }

    words
        .get(idx)
        .is_some_and(|word| matches!(word.to_ascii_uppercase().as_str(), "GLOBAL" | "PRIVATE"))
        && words
            .get(idx + 1)
            .is_some_and(|word| word.eq_ignore_ascii_case("TEMPORARY"))
        && words
            .get(idx + 2)
            .is_some_and(|word| word.eq_ignore_ascii_case("TABLE"))
}

fn line_completes_ddl_query_body_pending_header(line: &str) -> bool {
    let structural_tail = owner_header_structural_tail(line);
    !structural_tail.is_empty()
        && !line_ends_with_open_paren_before_inline_comment(structural_tail)
        && line_ends_with_keyword(structural_tail, "AS")
}

pub(crate) fn line_is_ddl_query_body_header(line: &str) -> bool {
    line_starts_ddl_query_body_header_prefix(line)
        && line_completes_ddl_query_body_pending_header(line)
}

pub(crate) fn line_starts_query_head(trimmed_upper: &str) -> bool {
    leading_identifier_words(trimmed_upper, 1)
        .first()
        .copied()
        .is_some_and(is_subquery_head_keyword)
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum FormatQueryOwnerKind {
    Clause,
    FromItem,
    Condition,
    Operator,
    DdlBody,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum FormatPlsqlChildQueryOwnerKind {
    ControlBody,
    CursorDeclaration,
    OpenCursorFor,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum MySqlDeclareOwnerKind {
    CursorFor,
    HandlerFor,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum PendingFormatPlsqlChildQueryOwnerHeaderKind {
    CursorDeclaration,
    OpenCursorFor,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum PendingFormatQueryOwnerHeaderKind {
    ReferenceOn,
    JoinLike,
    ConditionNot,
    DdlQueryBody,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum SplitQueryOwnerLookaheadKind {
    GenericExpression,
    DirectFromItem,
}

impl FormatQueryOwnerKind {
    /// Returns the minimum structural owner depth that a split owner-header
    /// line should keep before the child query opens.
    pub(crate) fn header_depth_floor(
        self,
        query_base_depth: Option<usize>,
        condition_header_depth: Option<usize>,
    ) -> Option<usize> {
        match self {
            Self::Clause | Self::FromItem => query_base_depth,
            Self::Condition => condition_header_depth
                .or_else(|| query_base_depth.map(|depth| depth.saturating_add(1))),
            Self::Operator | Self::DdlBody => None,
        }
    }

    /// Returns the analyzer owner-base depth that should feed the next nested
    /// query head after this owner line.
    pub(crate) fn auto_format_child_query_owner_base_depth(
        self,
        resolved_owner_depth: usize,
        query_base_depth: Option<usize>,
    ) -> usize {
        match self {
            Self::Condition => query_base_depth
                .map(|depth| resolved_owner_depth.max(depth.saturating_add(1)))
                .unwrap_or(resolved_owner_depth.saturating_add(1)),
            Self::Clause | Self::FromItem | Self::Operator | Self::DdlBody => resolved_owner_depth,
        }
    }
}

fn is_format_operator_query_owner_symbol(symbol: &str) -> bool {
    matches!(symbol, "=" | "<" | ">" | "<=" | ">=" | "<>" | "!=" | "<=>")
}

fn format_condition_operator_query_owner_kind(line: &str) -> Option<FormatQueryOwnerKind> {
    let (previous, last) = trailing_meaningful_tokens_before_inline_comment(line);

    match (previous, last) {
        (_, Some(FormatTrailingMeaningfulToken::Symbol(symbol)))
            if is_format_operator_query_owner_symbol(symbol) =>
        {
            Some(FormatQueryOwnerKind::Operator)
        }
        (
            Some(FormatTrailingMeaningfulToken::Symbol(symbol)),
            Some(FormatTrailingMeaningfulToken::Symbol("(")),
        ) if is_format_operator_query_owner_symbol(symbol) => Some(FormatQueryOwnerKind::Operator),
        _ => None,
    }
}

pub(crate) fn contextual_format_query_owner_kind(
    line: &str,
    allow_condition_operator_owner: bool,
) -> Option<FormatQueryOwnerKind> {
    format_query_owner_kind(line)
        .or_else(|| format_query_owner_header_kind(line))
        .or_else(|| {
            allow_condition_operator_owner
                .then(|| format_condition_operator_query_owner_kind(line))
                .flatten()
        })
}

impl PendingFormatQueryOwnerHeaderKind {
    pub(crate) fn owner_kind_for_line(self, line: &str) -> Option<FormatQueryOwnerKind> {
        let structural_tail = owner_header_structural_tail(line);
        match self {
            Self::ReferenceOn => Some(FormatQueryOwnerKind::Clause),
            Self::JoinLike => {
                if line_ends_with_keyword(structural_tail, "APPLY") {
                    Some(FormatQueryOwnerKind::FromItem)
                } else if line_ends_with_keyword(structural_tail, "JOIN") {
                    Some(FormatQueryOwnerKind::Clause)
                } else {
                    None
                }
            }
            Self::ConditionNot => {
                let trimmed_upper = structural_tail.to_ascii_uppercase();
                (line_starts_with_identifier_sequence(&trimmed_upper, &["EXISTS"])
                    || line_starts_with_identifier_sequence(&trimmed_upper, &["IN"])
                    || line_ends_with_keyword(structural_tail, "EXISTS")
                    || line_ends_with_keyword(structural_tail, "IN"))
                .then_some(FormatQueryOwnerKind::Condition)
            }
            Self::DdlQueryBody => self
                .line_completes(line)
                .then_some(FormatQueryOwnerKind::DdlBody),
        }
    }

    pub(crate) fn line_completes(self, line: &str) -> bool {
        let structural_tail = owner_header_structural_tail(line);
        match self {
            Self::ReferenceOn => line_ends_with_keyword(structural_tail, "ON"),
            Self::JoinLike => {
                line_ends_with_keyword(structural_tail, "JOIN")
                    || line_ends_with_keyword(structural_tail, "APPLY")
            }
            Self::ConditionNot => self.owner_kind_for_line(line).is_some(),
            Self::DdlQueryBody => line_completes_ddl_query_body_pending_header(line),
        }
    }

    pub(crate) fn line_can_continue(self, line: &str) -> bool {
        if self.line_completes(line) {
            return true;
        }

        let structural_tail = owner_header_structural_tail(line);
        let trimmed_upper = structural_tail.to_ascii_uppercase();
        match self {
            Self::ReferenceOn => !starts_with_auto_format_owner_boundary(structural_tail),
            // JOIN/APPLY modifier chains are the only structural continuation
            // that can legally extend this pending owner family.
            // Everything else must terminate the pending header immediately.
            Self::JoinLike => starts_with_pending_query_owner_join_modifier(&trimmed_upper),
            Self::ConditionNot => self.line_completes(line),
            Self::DdlQueryBody => {
                if structural_tail.is_empty() {
                    return owner_header_has_only_leading_close_parens(line);
                }

                if line_ends_with_semicolon_before_inline_comment(structural_tail)
                    || line_starts_query_head(&trimmed_upper)
                {
                    return false;
                }

                let first_word = leading_meaningful_words(structural_tail, 1)
                    .first()
                    .copied()
                    .map(str::to_ascii_uppercase);
                !matches!(
                    first_word.as_deref(),
                    Some(
                        "CREATE"
                            | "ALTER"
                            | "DROP"
                            | "TRUNCATE"
                            | "COMMENT"
                            | "GRANT"
                            | "REVOKE"
                            | "BEGIN"
                            | "DECLARE"
                    )
                )
            }
        }
    }

    pub(crate) fn normalized_current_line_depth(
        self,
        current_depth: usize,
        query_base_depth: Option<usize>,
        condition_header_depth: Option<usize>,
    ) -> usize {
        match self {
            Self::ReferenceOn => current_depth,
            Self::JoinLike => query_base_depth.unwrap_or(current_depth),
            Self::ConditionNot => FormatQueryOwnerKind::Condition
                .header_depth_floor(query_base_depth, condition_header_depth)
                .map(|depth_floor| current_depth.max(depth_floor))
                .unwrap_or(current_depth),
            Self::DdlQueryBody => current_depth,
        }
    }
}

fn starts_with_auto_format_owner_boundary_impl(
    line: &str,
    include_expression_query_owner: bool,
) -> bool {
    let trimmed = line.trim_start();
    if trimmed.is_empty() {
        return false;
    }

    let trimmed_upper = trimmed.to_ascii_uppercase();
    starts_with_format_layout_clause(&trimmed_upper)
        || starts_with_format_set_operator(&trimmed_upper)
        || format_query_owner_header_kind(trimmed).is_some()
        || format_query_owner_pending_header_kind(trimmed).is_some()
        || format_indented_paren_owner_header_kind(trimmed).is_some()
        || format_indented_paren_pending_header_kind(trimmed).is_some()
        || (include_expression_query_owner
            && line_ends_with_format_expression_query_owner_keyword(trimmed))
        || format_plsql_child_query_owner_kind(&trimmed_upper).is_some()
        || format_plsql_child_query_owner_pending_header_kind(trimmed).is_some()
}

/// Returns true when `line` begins a structural formatter boundary that must
/// terminate any pending split-owner/header continuation.
///
/// This intentionally covers every nested-owner family that can redirect the
/// indentation state machine onto a different stack:
/// - query-owner headers and pending fragments (`FROM`, `JOIN`, `LATERAL`,
///   `REFERENCE ... ON`, `NOT EXISTS`, ...)
/// - multiline owner headers and pending fragments (`WITHIN GROUP`,
///   `WINDOW ... AS`, `NESTED [PATH] ... COLUMNS`, ...)
/// - generic expression query owners (`CURSOR`, `MULTISET`)
/// - PL/SQL child-query owners (`BEGIN`, `CURSOR ... IS`, `OPEN ... FOR`, ...)
pub(crate) fn starts_with_auto_format_owner_boundary(line: &str) -> bool {
    starts_with_auto_format_owner_boundary_impl(line, true)
}

/// Returns true when `line` begins a shared structural owner/layout boundary
/// that should stop comment/header continuation, while still allowing generic
/// expression RHS lines such as `MULTISET` to remain operator continuations
/// when a caller needs that behavior.
pub(crate) fn starts_with_auto_format_owner_boundary_without_expression_owner(line: &str) -> bool {
    starts_with_auto_format_owner_boundary_impl(line, false)
}

pub(crate) fn format_plsql_child_query_owner_kind(
    text_upper: &str,
) -> Option<FormatPlsqlChildQueryOwnerKind> {
    let structural_tail = owner_header_structural_tail(text_upper);
    let words = meaningful_identifier_words_before_inline_comment(structural_tail, 8);
    let first_word = words.first().copied()?;
    let first_word_upper = first_word.to_ascii_uppercase();

    if matches!(first_word_upper.as_str(), "BEGIN" | "EXCEPTION" | "ELSE") {
        return Some(FormatPlsqlChildQueryOwnerKind::ControlBody);
    }

    if matches!(first_word_upper.as_str(), "ELSIF" | "ELSEIF")
        && words
            .iter()
            .skip(1)
            .any(|word| word.eq_ignore_ascii_case("THEN"))
    {
        return Some(FormatPlsqlChildQueryOwnerKind::ControlBody);
    }

    if first_word.eq_ignore_ascii_case("CURSOR")
        && words
            .iter()
            .skip(1)
            .any(|word| word.eq_ignore_ascii_case("IS") || word.eq_ignore_ascii_case("AS"))
    {
        return Some(FormatPlsqlChildQueryOwnerKind::CursorDeclaration);
    }

    (first_word.eq_ignore_ascii_case("OPEN")
        && words
            .iter()
            .skip(1)
            .any(|word| word.eq_ignore_ascii_case("FOR")))
    .then_some(FormatPlsqlChildQueryOwnerKind::OpenCursorFor)
    .or_else(|| match mysql_declare_owner_kind(structural_tail) {
        Some(MySqlDeclareOwnerKind::CursorFor) => {
            Some(FormatPlsqlChildQueryOwnerKind::OpenCursorFor)
        }
        Some(MySqlDeclareOwnerKind::HandlerFor) => {
            Some(FormatPlsqlChildQueryOwnerKind::ControlBody)
        }
        None => None,
    })
}

pub(crate) fn mysql_declare_owner_kind(text_upper: &str) -> Option<MySqlDeclareOwnerKind> {
    let structural_tail = owner_header_structural_tail(text_upper);
    let words = meaningful_identifier_words_before_inline_comment(structural_tail, 10);
    let first_word = words.first().copied()?;
    if !first_word.eq_ignore_ascii_case("DECLARE") {
        return None;
    }

    let has_for = words
        .iter()
        .skip(1)
        .any(|word| word.eq_ignore_ascii_case("FOR"));
    if !has_for {
        return None;
    }

    let has_cursor = words
        .iter()
        .skip(1)
        .any(|word| word.eq_ignore_ascii_case("CURSOR"));
    if has_cursor {
        return Some(MySqlDeclareOwnerKind::CursorFor);
    }

    let has_handler = words
        .iter()
        .skip(1)
        .any(|word| word.eq_ignore_ascii_case("HANDLER"));
    has_handler.then_some(MySqlDeclareOwnerKind::HandlerFor)
}

impl PendingFormatPlsqlChildQueryOwnerHeaderKind {
    pub(crate) fn line_completes(self, line: &str) -> bool {
        let structural_tail = owner_header_structural_tail(line);
        match self {
            Self::CursorDeclaration => {
                line_starts_with_identifier_sequence(structural_tail, &["IS"])
                    || line_starts_with_identifier_sequence(structural_tail, &["AS"])
                    || line_ends_with_keyword(structural_tail, "IS")
                    || line_ends_with_keyword(structural_tail, "AS")
            }
            Self::OpenCursorFor => {
                line_starts_with_identifier_sequence(structural_tail, &["FOR"])
                    || line_ends_with_keyword(structural_tail, "FOR")
            }
        }
    }

    pub(crate) fn line_can_continue(self, line: &str) -> bool {
        if self.line_completes(line) {
            return true;
        }

        let structural_tail = owner_header_structural_tail(line);
        if structural_tail.is_empty() {
            return owner_header_has_only_leading_close_parens(line);
        }

        if owner_header_has_only_semicolon_tail_after_leading_close(line) {
            return true;
        }

        if line_ends_with_semicolon_before_inline_comment(structural_tail) {
            return false;
        }

        !starts_with_auto_format_owner_boundary(structural_tail)
    }
}

pub(crate) fn format_plsql_child_query_owner_pending_header_kind(
    line: &str,
) -> Option<PendingFormatPlsqlChildQueryOwnerHeaderKind> {
    let structural_tail = owner_header_structural_tail(line);
    let ends_with_semicolon = line_ends_with_semicolon_before_inline_comment(structural_tail);

    if line_starts_with_identifier_sequence(structural_tail, &["CURSOR"])
        && format_plsql_child_query_owner_kind(structural_tail)
            != Some(FormatPlsqlChildQueryOwnerKind::CursorDeclaration)
        && !ends_with_semicolon
    {
        return Some(PendingFormatPlsqlChildQueryOwnerHeaderKind::CursorDeclaration);
    }

    (line_starts_with_identifier_sequence(structural_tail, &["OPEN"])
        && format_plsql_child_query_owner_kind(structural_tail)
            != Some(FormatPlsqlChildQueryOwnerKind::OpenCursorFor)
        && !ends_with_semicolon)
        .then_some(PendingFormatPlsqlChildQueryOwnerHeaderKind::OpenCursorFor)
}

pub(crate) fn format_query_owner_pending_header_kind(
    line: &str,
) -> Option<PendingFormatQueryOwnerHeaderKind> {
    let structural_tail = owner_header_structural_tail(line);
    let trimmed_upper = structural_tail.to_ascii_uppercase();
    if line_starts_ddl_query_body_header_prefix(structural_tail)
        && !line_completes_ddl_query_body_pending_header(structural_tail)
        && !line_ends_with_semicolon_before_inline_comment(structural_tail)
    {
        return Some(PendingFormatQueryOwnerHeaderKind::DdlQueryBody);
    }

    if line_starts_with_identifier_sequence(structural_tail, &["REFERENCE"])
        && !line_ends_with_keyword(structural_tail, "ON")
        && !line_ends_with_open_paren_before_inline_comment(structural_tail)
    {
        return Some(PendingFormatQueryOwnerHeaderKind::ReferenceOn);
    }

    if line_ends_with_keyword(structural_tail, "NOT")
        && !line_ends_with_identifier_sequence(structural_tail, &["IS", "NOT"])
        && !line_ends_with_open_paren_before_inline_comment(structural_tail)
    {
        return Some(PendingFormatQueryOwnerHeaderKind::ConditionNot);
    }

    (starts_with_pending_query_owner_join_modifier(&trimmed_upper)
        && !line_ends_with_keyword(structural_tail, "JOIN")
        && !line_ends_with_keyword(structural_tail, "APPLY")
        && !line_ends_with_open_paren_before_inline_comment(structural_tail))
    .then_some(PendingFormatQueryOwnerHeaderKind::JoinLike)
}

fn starts_with_pending_query_owner_join_modifier(text_upper: &str) -> bool {
    line_starts_with_identifier_sequence(text_upper, &["OUTER"])
        || FORMAT_JOIN_MODIFIER_KEYWORDS
            .iter()
            .any(|keyword| line_starts_with_identifier_sequence(text_upper, &[*keyword]))
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum FormatIndentedParenOwnerKind {
    AnalyticOver,
    WithinGroup,
    Keep,
    ModelSubclause,
    Window,
    MatchRecognize,
    Pivot,
    Unpivot,
    StructuredColumns,
    PropertyGraphTables,
}

const FORMAT_ALL_INDENTED_PAREN_OWNER_KINDS: &[FormatIndentedParenOwnerKind] = &[
    FormatIndentedParenOwnerKind::AnalyticOver,
    FormatIndentedParenOwnerKind::WithinGroup,
    FormatIndentedParenOwnerKind::Keep,
    FormatIndentedParenOwnerKind::ModelSubclause,
    FormatIndentedParenOwnerKind::Window,
    FormatIndentedParenOwnerKind::MatchRecognize,
    FormatIndentedParenOwnerKind::Pivot,
    FormatIndentedParenOwnerKind::Unpivot,
    FormatIndentedParenOwnerKind::StructuredColumns,
    FormatIndentedParenOwnerKind::PropertyGraphTables,
];

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum PendingFormatIndentedParenOwnerHeaderKind {
    WindowAs,
    WithinGroup,
    NestedPathColumns,
}

const FORMAT_ANALYTIC_OVER_SUBCLAUSE_KEYWORD_SEQUENCES: &[&[&str]] = &[
    &["PARTITION", "BY"],
    &["ORDER", "BY"],
    &["ROWS"],
    &["RANGE"],
    &["GROUPS"],
    &["EXCLUDE"],
];

const FORMAT_WITHIN_GROUP_SUBCLAUSE_KEYWORD_SEQUENCES: &[&[&str]] = &[&["ORDER", "BY"]];

const FORMAT_KEEP_SUBCLAUSE_KEYWORD_SEQUENCES: &[&[&str]] = &[
    &["DENSE_RANK", "FIRST", "ORDER", "BY"],
    &["DENSE_RANK", "LAST", "ORDER", "BY"],
];

const FORMAT_MODEL_SUBCLAUSE_KEYWORD_SEQUENCES: &[&[&str]] = &[
    &["PARTITION", "BY"],
    &["DIMENSION", "BY"],
    &["MEASURES"],
    &["REFERENCE"],
    &["RULES"],
    &["UPDATE"],
    &["UPSERT"],
    &["UPSERT", "ALL"],
    &["AUTOMATIC", "ORDER"],
    &["SEQUENTIAL", "ORDER"],
    &["ITERATE"],
    &["UNTIL"],
    &["IGNORE", "NAV"],
    &["KEEP", "NAV"],
    &["UNIQUE", "DIMENSION"],
    &["UNIQUE", "SINGLE", "REFERENCE"],
    &["RETURN", "ALL", "ROWS"],
    &["RETURN", "UPDATED", "ROWS"],
];

// MODEL rule modifiers may already be attached to a preceding `RULES` line.
// Phase 2 still needs to recognize them as owner-relative headers when users
// split them manually, but phase 1 should not aggressively force a new break
// between `RULES` and its modifiers.
const FORMAT_MODEL_PHASE1_EXCLUDED_SUBCLAUSE_KEYWORD_SEQUENCES: &[&[&str]] = &[
    &["UPDATE"],
    &["UPSERT"],
    &["UPSERT", "ALL"],
    &["AUTOMATIC", "ORDER"],
    &["SEQUENTIAL", "ORDER"],
    &["ITERATE"],
    &["UNTIL"],
];

const FORMAT_MATCH_RECOGNIZE_SUBCLAUSE_KEYWORD_SEQUENCES: &[&[&str]] = &[
    &["PARTITION", "BY"],
    &["ORDER", "BY"],
    &["MEASURES"],
    &["ONE", "ROW", "PER"],
    &["ALL", "ROWS", "PER"],
    &["WITH", "UNMATCHED", "ROWS"],
    &["WITHOUT", "UNMATCHED", "ROWS"],
    &["SHOW", "EMPTY", "MATCHES"],
    &["OMIT", "EMPTY", "MATCHES"],
    &["AFTER", "MATCH", "SKIP"],
    &["SUBSET"],
    &["PATTERN"],
    &["DEFINE"],
];

const FORMAT_PIVOT_UNPIVOT_SUBCLAUSE_KEYWORD_SEQUENCES: &[&[&str]] = &[&["FOR"]];

const FORMAT_STRUCTURED_COLUMNS_SUBCLAUSE_KEYWORD_SEQUENCES: &[&[&str]] = &[&["NESTED"]];

const FORMAT_PROPERTY_GRAPH_TABLE_SUBCLAUSE_KEYWORD_SEQUENCES: &[&[&str]] = &[
    &["KEY"],
    &["SOURCE", "KEY"],
    &["DESTINATION", "KEY"],
    &["LABEL"],
    &["PROPERTIES"],
];

fn starts_with_keyword_sequence(text_upper: &str, sequence: &[&str]) -> bool {
    !sequence.is_empty() && line_starts_with_identifier_sequence(text_upper, sequence)
}

fn starts_with_any_keyword_sequence(text_upper: &str, sequences: &[&[&str]]) -> bool {
    sequences
        .iter()
        .any(|sequence| starts_with_keyword_sequence(text_upper, sequence))
}

fn leading_meaningful_words(line: &str, max_words: usize) -> Vec<&str> {
    leading_identifier_words(line, max_words)
}

fn leading_words_match_keyword_prefix(words: &[&str], sequence: &[&str]) -> usize {
    let mut matched_sequence_words = 0usize;
    let mut consumed_words = 0usize;

    while matched_sequence_words < sequence.len() && consumed_words < words.len() {
        let expected_word = sequence[matched_sequence_words];
        let expected_segment_count = identifier_sequence_segment_count(&[expected_word]);
        let mut candidate_end = consumed_words;
        let mut consumed_segments = 0usize;

        while candidate_end < words.len() && consumed_segments < expected_segment_count {
            let identifier_segment_count = identifier_segment_count(words[candidate_end]);
            consumed_segments = consumed_segments.saturating_add(identifier_segment_count);
            if consumed_segments > expected_segment_count {
                return matched_sequence_words;
            }
            candidate_end = candidate_end.saturating_add(1);
        }

        if consumed_segments != expected_segment_count
            || !identifier_word_matches_keyword_sequence(
                &words[consumed_words..candidate_end],
                &[expected_word],
            )
        {
            break;
        }

        matched_sequence_words = matched_sequence_words.saturating_add(1);
        consumed_words = candidate_end;
    }

    matched_sequence_words
}

fn leading_words_match_keyword_sequence(
    first_word: &str,
    second_word: Option<&str>,
    third_word: Option<&str>,
    sequence: &[&str],
) -> bool {
    let mut words = Vec::with_capacity(3);
    words.push(first_word);
    if let Some(second_word) = second_word {
        words.push(second_word);
    }
    if let Some(third_word) = third_word {
        words.push(third_word);
    }

    leading_words_match_keyword_prefix(&words, sequence) == sequence.len()
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum FormatBodyHeaderContinuationState {
    Sequence {
        candidate_mask: u64,
        matched_words: usize,
    },
    Freeform,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(crate) struct FormatBodyHeaderLineState {
    pub(crate) is_header: bool,
    pub(crate) next_state: Option<FormatBodyHeaderContinuationState>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct ExactBareSplitBodyHeaderMatch {
    sequence_idx: usize,
    matched_words: usize,
    sequence_len: usize,
    line_word_count: usize,
}

impl FormatIndentedParenOwnerKind {
    fn exact_bare_split_body_header_match(
        self,
        line: &str,
    ) -> Option<ExactBareSplitBodyHeaderMatch> {
        let trimmed = auto_format_structural_tail(line);
        if trimmed.is_empty() {
            return None;
        }

        let max_words = self
            .split_body_header_sequences()
            .iter()
            .fold(8usize, |acc, sequence| {
                acc.max(identifier_sequence_segment_count(sequence))
            });
        let words = leading_meaningful_words(trimmed, max_words);
        if words.is_empty() || !line_has_exact_identifier_sequence(trimmed, words.as_slice()) {
            return None;
        }

        let line_word_count = words.len();
        let line_segment_count = words
            .iter()
            .map(|word| identifier_segment_count(word))
            .sum::<usize>();
        let mut best_match = None;
        let mut best_matched_words = 0usize;

        for (idx, sequence) in self.split_body_header_sequences().iter().enumerate() {
            let matched_words = leading_words_match_keyword_prefix(&words, sequence);
            if matched_words == 0 {
                continue;
            }

            // Exact bare split-header carry is only valid when the entire
            // line is itself a prefix of the owner-relative keyword chain.
            // Otherwise overloaded clause heads like `ORDER SIBLINGS BY`
            // would get hijacked by the generic owner-relative `ORDER BY`
            // matcher and lose their clause-body depth.
            let matched_segment_count =
                identifier_sequence_segment_count(&sequence[..matched_words]);
            if matched_words < sequence.len() && matched_segment_count != line_segment_count {
                continue;
            }

            if matched_words > best_matched_words {
                best_matched_words = matched_words;
                best_match = Some(ExactBareSplitBodyHeaderMatch {
                    sequence_idx: idx,
                    matched_words,
                    sequence_len: sequence.len(),
                    line_word_count,
                });
            }
        }

        best_match
    }

    fn body_header_sequences(self) -> &'static [&'static [&'static str]] {
        match self {
            Self::AnalyticOver | Self::Window => FORMAT_ANALYTIC_OVER_SUBCLAUSE_KEYWORD_SEQUENCES,
            Self::WithinGroup => FORMAT_WITHIN_GROUP_SUBCLAUSE_KEYWORD_SEQUENCES,
            Self::Keep => FORMAT_KEEP_SUBCLAUSE_KEYWORD_SEQUENCES,
            Self::ModelSubclause => FORMAT_MODEL_SUBCLAUSE_KEYWORD_SEQUENCES,
            Self::MatchRecognize => FORMAT_MATCH_RECOGNIZE_SUBCLAUSE_KEYWORD_SEQUENCES,
            Self::Pivot | Self::Unpivot => FORMAT_PIVOT_UNPIVOT_SUBCLAUSE_KEYWORD_SEQUENCES,
            Self::StructuredColumns => FORMAT_STRUCTURED_COLUMNS_SUBCLAUSE_KEYWORD_SEQUENCES,
            Self::PropertyGraphTables => FORMAT_PROPERTY_GRAPH_TABLE_SUBCLAUSE_KEYWORD_SEQUENCES,
        }
    }

    fn split_body_header_sequences(self) -> &'static [&'static [&'static str]] {
        match self {
            Self::AnalyticOver | Self::Window => &[
                &["PARTITION", "BY"],
                &["ORDER", "BY"],
                &["ROWS"],
                &["RANGE"],
                &["GROUPS"],
                &["EXCLUDE"],
            ],
            Self::WithinGroup => FORMAT_WITHIN_GROUP_SUBCLAUSE_KEYWORD_SEQUENCES,
            Self::Keep => FORMAT_KEEP_SUBCLAUSE_KEYWORD_SEQUENCES,
            Self::ModelSubclause => FORMAT_MODEL_SUBCLAUSE_KEYWORD_SEQUENCES,
            Self::MatchRecognize => &[
                &["PARTITION", "BY"],
                &["ORDER", "BY"],
                &["MEASURES"],
                &["ONE", "ROW", "PER", "MATCH"],
                &["ALL", "ROWS", "PER", "MATCH"],
                &["WITH", "UNMATCHED", "ROWS"],
                &["WITHOUT", "UNMATCHED", "ROWS"],
                &["SHOW", "EMPTY", "MATCHES"],
                &["OMIT", "EMPTY", "MATCHES"],
                &["AFTER", "MATCH", "SKIP"],
                &["SUBSET"],
                &["PATTERN"],
                &["DEFINE"],
            ],
            Self::Pivot | Self::Unpivot => &[&["FOR", "IN"]],
            Self::StructuredColumns => &[&["NESTED", "PATH", "COLUMNS"], &["NESTED", "COLUMNS"]],
            Self::PropertyGraphTables => FORMAT_PROPERTY_GRAPH_TABLE_SUBCLAUSE_KEYWORD_SEQUENCES,
        }
    }

    fn split_body_header_sequence_is_freeform(self, sequence_idx: usize) -> bool {
        match self {
            Self::AnalyticOver | Self::Window => (2..=5).contains(&sequence_idx),
            Self::MatchRecognize => sequence_idx == 9,
            Self::WithinGroup
            | Self::Keep
            | Self::ModelSubclause
            | Self::Pivot
            | Self::Unpivot
            | Self::StructuredColumns
            | Self::PropertyGraphTables => false,
        }
    }

    fn freeform_body_header_continues(self, text_upper: &str) -> bool {
        let trimmed_upper = text_upper.trim_start();
        !trimmed_upper.is_empty()
            && !line_has_leading_significant_close_paren(trimmed_upper)
            && self
                .best_split_body_header_prefix_match(trimmed_upper)
                .is_none()
            && !starts_with_format_layout_clause(trimmed_upper)
            && !starts_with_format_set_operator(trimmed_upper)
            && !line_starts_with_identifier_sequence(trimmed_upper, &["MODEL"])
            && !line_starts_with_identifier_sequence(trimmed_upper, &["MATCH_RECOGNIZE"])
            && !line_starts_with_identifier_sequence(trimmed_upper, &["WINDOW"])
    }

    fn best_split_body_header_prefix_match(self, text_upper: &str) -> Option<(u64, usize)> {
        let sequences = self.split_body_header_sequences();
        let max_words = sequences
            .iter()
            .fold(0usize, |acc, sequence| acc.max(sequence.len()));
        if max_words == 0 {
            return None;
        }

        let words = leading_meaningful_words(text_upper.trim_start(), max_words);
        if words.is_empty() {
            return None;
        }

        let mut best_mask = 0u64;
        let mut best_matched_words = 0usize;
        for (idx, sequence) in sequences.iter().enumerate() {
            let matched_words = leading_words_match_keyword_prefix(&words, sequence);
            if matched_words == 0 {
                continue;
            }

            if matched_words > best_matched_words {
                best_mask = 1u64 << idx;
                best_matched_words = matched_words;
            } else if matched_words == best_matched_words {
                best_mask |= 1u64 << idx;
            }
        }

        (best_matched_words > 0).then_some((best_mask, best_matched_words))
    }

    fn best_split_body_header_continuation_match(
        self,
        candidate_mask: u64,
        matched_words: usize,
        text_upper: &str,
    ) -> Option<(u64, usize)> {
        let sequences = self.split_body_header_sequences();
        let max_remaining_words = sequences
            .iter()
            .enumerate()
            .filter(|(idx, _)| (candidate_mask & (1u64 << idx)) != 0)
            .fold(0usize, |acc, (_, sequence)| {
                acc.max(sequence.len().saturating_sub(matched_words))
            });
        if max_remaining_words == 0 {
            return None;
        }

        let words = leading_meaningful_words(text_upper.trim_start(), max_remaining_words);
        if words.is_empty() {
            return None;
        }

        let mut best_mask = 0u64;
        let mut best_total_matched_words = 0usize;
        for (idx, sequence) in sequences.iter().enumerate() {
            if (candidate_mask & (1u64 << idx)) == 0 || matched_words >= sequence.len() {
                continue;
            }

            let matched_suffix_words =
                leading_words_match_keyword_prefix(&words, &sequence[matched_words..]);
            if matched_suffix_words == 0 {
                continue;
            }

            let total_matched_words = matched_words.saturating_add(matched_suffix_words);
            if total_matched_words > best_total_matched_words {
                best_mask = 1u64 << idx;
                best_total_matched_words = total_matched_words;
            } else if total_matched_words == best_total_matched_words {
                best_mask |= 1u64 << idx;
            }
        }

        (best_total_matched_words > 0).then_some((best_mask, best_total_matched_words))
    }

    fn next_body_header_continuation_state(
        self,
        candidate_mask: u64,
        matched_words: usize,
    ) -> Option<FormatBodyHeaderContinuationState> {
        let mut incomplete_mask = 0u64;
        let mut completed_freeform_header = false;

        for (idx, sequence) in self.split_body_header_sequences().iter().enumerate() {
            if (candidate_mask & (1u64 << idx)) == 0 {
                continue;
            }

            if matched_words < sequence.len() {
                incomplete_mask |= 1u64 << idx;
            } else if self.split_body_header_sequence_is_freeform(idx) {
                completed_freeform_header = true;
            }
        }

        if completed_freeform_header {
            Some(FormatBodyHeaderContinuationState::Freeform)
        } else if incomplete_mask != 0 {
            Some(FormatBodyHeaderContinuationState::Sequence {
                candidate_mask: incomplete_mask,
                matched_words,
            })
        } else {
            None
        }
    }

    pub(crate) fn body_header_line_state(
        self,
        text_upper: &str,
        previous_state: Option<FormatBodyHeaderContinuationState>,
    ) -> FormatBodyHeaderLineState {
        let trimmed_upper = text_upper.trim_start();
        if trimmed_upper.is_empty() {
            return FormatBodyHeaderLineState::default();
        }

        if let Some(previous_state) = previous_state {
            match previous_state {
                FormatBodyHeaderContinuationState::Sequence {
                    candidate_mask,
                    matched_words,
                } => {
                    if let Some((matched_mask, total_matched_words)) = self
                        .best_split_body_header_continuation_match(
                            candidate_mask,
                            matched_words,
                            trimmed_upper,
                        )
                    {
                        return FormatBodyHeaderLineState {
                            is_header: true,
                            next_state: self.next_body_header_continuation_state(
                                matched_mask,
                                total_matched_words,
                            ),
                        };
                    }
                }
                FormatBodyHeaderContinuationState::Freeform => {
                    if self.freeform_body_header_continues(trimmed_upper) {
                        return FormatBodyHeaderLineState {
                            is_header: true,
                            next_state: Some(FormatBodyHeaderContinuationState::Freeform),
                        };
                    }
                }
            }
        }

        if let Some((matched_mask, matched_words)) =
            self.best_split_body_header_prefix_match(trimmed_upper)
        {
            return FormatBodyHeaderLineState {
                is_header: true,
                next_state: self.next_body_header_continuation_state(matched_mask, matched_words),
            };
        }

        FormatBodyHeaderLineState::default()
    }

    fn exact_bare_split_body_header_continuation_kind(
        self,
        line: &str,
    ) -> Option<FormatInlineCommentHeaderContinuationKind> {
        self.exact_bare_split_body_header_match(line)
            .map(|matched| {
                if matched.matched_words < matched.sequence_len {
                    FormatInlineCommentHeaderContinuationKind::SameDepth
                } else {
                    self.complete_split_body_header_continuation_kind(matched.sequence_idx)
                }
            })
    }

    fn exact_bare_split_body_header_inline_comment_collision_kind(
        self,
        line: &str,
    ) -> Option<FormatInlineCommentHeaderContinuationKind> {
        let matched = self.exact_bare_split_body_header_match(line)?;
        let matched_sequence = self
            .split_body_header_sequences()
            .get(matched.sequence_idx)
            .copied();

        match self {
            Self::Keep if matched.matched_words < matched.sequence_len => {
                Some(FormatInlineCommentHeaderContinuationKind::SameDepth)
            }
            Self::MatchRecognize
                if matched_sequence == Some(&["AFTER", "MATCH", "SKIP"][..])
                    && (matched.matched_words < matched.sequence_len
                        || matched.line_word_count > matched.sequence_len) =>
            {
                Some(FormatInlineCommentHeaderContinuationKind::SameDepth)
            }
            Self::ModelSubclause
                if matched_sequence == Some(&["UNIQUE", "SINGLE", "REFERENCE"][..])
                    && matched.matched_words == matched.sequence_len =>
            {
                Some(FormatInlineCommentHeaderContinuationKind::SameDepth)
            }
            _ => None,
        }
    }

    fn complete_split_body_header_continuation_kind(
        self,
        sequence_idx: usize,
    ) -> FormatInlineCommentHeaderContinuationKind {
        match self {
            Self::AnalyticOver | Self::Window => match sequence_idx {
                0 | 1 => FormatInlineCommentHeaderContinuationKind::OneDeeperThanCurrentLine,
                2..=5 => FormatInlineCommentHeaderContinuationKind::SameDepth,
                _ => FormatInlineCommentHeaderContinuationKind::SameDepth,
            },
            Self::WithinGroup => match sequence_idx {
                0 => FormatInlineCommentHeaderContinuationKind::OneDeeperThanCurrentLine,
                _ => FormatInlineCommentHeaderContinuationKind::SameDepth,
            },
            Self::Keep => FormatInlineCommentHeaderContinuationKind::OneDeeperThanCurrentLine,
            Self::ModelSubclause => match sequence_idx {
                0..=2 => FormatInlineCommentHeaderContinuationKind::OneDeeperThanCurrentLine,
                3..=15 => FormatInlineCommentHeaderContinuationKind::SameDepth,
                _ => FormatInlineCommentHeaderContinuationKind::SameDepth,
            },
            Self::MatchRecognize => match sequence_idx {
                0..=2 | 10..=12 => {
                    FormatInlineCommentHeaderContinuationKind::OneDeeperThanCurrentLine
                }
                3..=9 => FormatInlineCommentHeaderContinuationKind::SameDepth,
                _ => FormatInlineCommentHeaderContinuationKind::SameDepth,
            },
            Self::Pivot | Self::Unpivot | Self::StructuredColumns | Self::PropertyGraphTables => {
                FormatInlineCommentHeaderContinuationKind::SameDepth
            }
        }
    }

    pub(crate) fn starts_body_header(self, text_upper: &str) -> bool {
        starts_with_any_keyword_sequence(text_upper, self.body_header_sequences())
    }

    #[cfg(test)]
    fn starts_contextual_body_header(
        self,
        text_upper: &str,
        previous_line_upper: Option<&str>,
    ) -> bool {
        let previous_state = previous_line_upper
            .and_then(|previous| self.body_header_line_state(previous, None).next_state);
        self.body_header_line_state(text_upper, previous_state)
            .is_header
    }

    pub(crate) fn starts_body_header_words(
        self,
        first_word: &str,
        second_word: Option<&str>,
        third_word: Option<&str>,
    ) -> bool {
        self.body_header_sequences().iter().any(|sequence| {
            leading_words_match_keyword_sequence(first_word, second_word, third_word, sequence)
        })
    }

    fn phase1_excluded_body_header_sequences(self) -> &'static [&'static [&'static str]] {
        match self {
            Self::MatchRecognize => &[&["PARTITION", "BY"], &["ORDER", "BY"]],
            Self::ModelSubclause => FORMAT_MODEL_PHASE1_EXCLUDED_SUBCLAUSE_KEYWORD_SEQUENCES,
            Self::AnalyticOver
            | Self::WithinGroup
            | Self::Keep
            | Self::Window
            | Self::Pivot
            | Self::Unpivot
            | Self::StructuredColumns
            | Self::PropertyGraphTables => &[],
        }
    }

    pub(crate) fn starts_phase1_body_header_words(
        self,
        first_word: &str,
        second_word: Option<&str>,
        third_word: Option<&str>,
    ) -> bool {
        self.starts_body_header_words(first_word, second_word, third_word)
            && !self
                .phase1_excluded_body_header_sequences()
                .iter()
                .any(|sequence| {
                    leading_words_match_keyword_sequence(
                        first_word,
                        second_word,
                        third_word,
                        sequence,
                    )
                })
    }

    /// Returns the normalized owner depth that formatter phase 2 should use
    /// before pushing the multiline owner stack for this kind.
    pub(crate) fn formatter_owner_depth(
        self,
        fallback_depth: usize,
        query_base_depth: Option<usize>,
        general_paren_continuation_depth: Option<usize>,
    ) -> usize {
        match self {
            Self::MatchRecognize | Self::Pivot | Self::Unpivot => {
                query_base_depth.unwrap_or(fallback_depth)
            }
            Self::Window => fallback_depth,
            Self::ModelSubclause => query_base_depth
                .map(|depth| depth.saturating_add(1))
                .unwrap_or(fallback_depth),
            Self::AnalyticOver
            | Self::WithinGroup
            | Self::Keep
            | Self::StructuredColumns
            | Self::PropertyGraphTables => {
                general_paren_continuation_depth.unwrap_or(fallback_depth)
            }
        }
    }

    /// Returns the standard body depth relative to a multiline owner line.
    pub(crate) fn body_depth(self, owner_depth: usize) -> usize {
        owner_depth.saturating_add(1)
    }

    /// Returns the owner-relative depth for structural body headers that must
    /// snap back to the active owner's body depth after nested expressions.
    #[cfg(test)]
    pub(crate) fn formatter_body_header_depth(
        self,
        text_upper: &str,
        previous_line_upper: Option<&str>,
        owner_depth: usize,
    ) -> Option<usize> {
        self.starts_contextual_body_header(text_upper, previous_line_upper)
            .then(|| {
                self.body_depth(owner_depth)
                    .saturating_add(usize::from(self == Self::PropertyGraphTables))
            })
    }
}

impl PendingFormatIndentedParenOwnerHeaderKind {
    pub(crate) fn owner_kind(self) -> FormatIndentedParenOwnerKind {
        match self {
            Self::WindowAs => FormatIndentedParenOwnerKind::Window,
            Self::WithinGroup => FormatIndentedParenOwnerKind::WithinGroup,
            Self::NestedPathColumns => FormatIndentedParenOwnerKind::StructuredColumns,
        }
    }

    pub(crate) fn line_completes(self, line: &str) -> bool {
        let structural_tail = owner_header_structural_tail(line);
        match self {
            Self::WindowAs => {
                line_starts_with_identifier_sequence(structural_tail, &["AS"])
                    || line_ends_with_keyword(structural_tail, "AS")
            }
            Self::WithinGroup => {
                line_starts_with_identifier_sequence(structural_tail, &["GROUP"])
                    || line_ends_with_keyword(structural_tail, "GROUP")
            }
            Self::NestedPathColumns => {
                line_starts_with_identifier_sequence(structural_tail, &["COLUMNS"])
                    || line_ends_with_keyword(structural_tail, "COLUMNS")
            }
        }
    }

    pub(crate) fn line_can_continue(self, line: &str) -> bool {
        if self.line_completes(line) {
            return true;
        }

        !starts_with_auto_format_owner_boundary(owner_header_structural_tail(line))
    }
}

pub(crate) fn format_indented_paren_pending_header_kind(
    line: &str,
) -> Option<PendingFormatIndentedParenOwnerHeaderKind> {
    let structural_tail = owner_header_structural_tail(line);

    if line_ends_with_keyword(structural_tail, "WITHIN")
        && !line_ends_with_keyword(structural_tail, "GROUP")
        && !line_ends_with_open_paren_before_inline_comment(structural_tail)
    {
        return Some(PendingFormatIndentedParenOwnerHeaderKind::WithinGroup);
    }

    if line_starts_with_identifier_sequence(structural_tail, &["WINDOW"])
        && !line_has_exact_identifier_sequence(structural_tail, &["WINDOW"])
        && !line_ends_with_keyword(structural_tail, "AS")
        && !line_ends_with_open_paren_before_inline_comment(structural_tail)
    {
        return Some(PendingFormatIndentedParenOwnerHeaderKind::WindowAs);
    }

    let nested_second_word =
        next_meaningful_word(structural_tail.trim_start(), 1).map(|(word, _)| word);
    (line_starts_with_identifier_sequence(structural_tail, &["NESTED"])
        && !nested_second_word.is_some_and(|word| word.eq_ignore_ascii_case("TABLE"))
        && !line_ends_with_keyword(structural_tail, "COLUMNS")
        && !line_ends_with_open_paren_before_inline_comment(structural_tail))
    .then_some(PendingFormatIndentedParenOwnerHeaderKind::NestedPathColumns)
}

pub(crate) fn format_indented_paren_owner_header_continues(
    pending_kind: FormatIndentedParenOwnerKind,
    line: &str,
) -> bool {
    let structural_tail = owner_header_structural_tail(line);
    match pending_kind {
        FormatIndentedParenOwnerKind::Pivot => {
            line_starts_with_identifier_sequence(structural_tail, &["XML"])
        }
        FormatIndentedParenOwnerKind::Unpivot => {
            line_starts_with_identifier_sequence(structural_tail, &["INCLUDE"])
                || line_starts_with_identifier_sequence(structural_tail, &["EXCLUDE"])
                || line_starts_with_identifier_sequence(structural_tail, &["NULLS"])
        }
        FormatIndentedParenOwnerKind::AnalyticOver
        | FormatIndentedParenOwnerKind::WithinGroup
        | FormatIndentedParenOwnerKind::Keep
        | FormatIndentedParenOwnerKind::ModelSubclause
        | FormatIndentedParenOwnerKind::Window
        | FormatIndentedParenOwnerKind::MatchRecognize
        | FormatIndentedParenOwnerKind::StructuredColumns
        | FormatIndentedParenOwnerKind::PropertyGraphTables => false,
    }
}

/// Returns true when `text_upper` is the trailing token of a split MODEL
/// subclause header that can still own a following standalone `(` line.
///
/// This covers headers whose final token is not itself recognized as a direct
/// MODEL subclause start, such as `PARTITION / BY / (` or
/// `RULES / AUTOMATIC / ORDER / (`.
pub(crate) fn starts_with_format_model_multiline_owner_tail(text_upper: &str) -> bool {
    line_starts_with_identifier_sequence(text_upper, &["BY"])
        || line_starts_with_identifier_sequence(text_upper, &["ORDER"])
        || line_starts_with_identifier_sequence(text_upper, &["ALL"])
}

/// Returns the formatter query-owner kind when `line` ends on the owner header
/// token itself and the opening parenthesis starts on a later line.
pub(crate) fn format_query_owner_header_kind(line: &str) -> Option<FormatQueryOwnerKind> {
    let structural_tail = owner_header_structural_tail(line);

    if line_is_ddl_query_body_header(structural_tail) {
        return Some(FormatQueryOwnerKind::DdlBody);
    }

    if line_ends_with_format_split_direct_from_item_query_owner_keyword(structural_tail)
        || line_ends_with_keyword(structural_tail, "APPLY")
    {
        return Some(FormatQueryOwnerKind::FromItem);
    }

    let trimmed_upper = structural_tail.to_ascii_uppercase();
    let starts_query_head = line_starts_query_head(structural_tail);
    let starts_non_query_owner_subclause = starts_query_head
        || line_starts_with_identifier_sequence(structural_tail, &["MODEL"])
        || starts_with_format_model_subclause(&trimmed_upper)
        || line_starts_with_identifier_sequence(structural_tail, &["MATCH_RECOGNIZE"])
        || starts_with_format_match_recognize_subclause(&trimmed_upper)
        || line_starts_with_identifier_sequence(structural_tail, &["WINDOW"]);
    if line_starts_with_identifier_sequence(structural_tail, &["REFERENCE"])
        && line_ends_with_keyword(structural_tail, "ON")
    {
        return Some(FormatQueryOwnerKind::Clause);
    }

    if line_ends_with_keyword(structural_tail, "FROM")
        || line_ends_with_keyword(structural_tail, "USING")
        || line_ends_with_keyword(structural_tail, "JOIN")
    {
        return Some(FormatQueryOwnerKind::Clause);
    }

    let starts_set_operator = starts_with_format_set_operator(&trimmed_upper);
    if !starts_non_query_owner_subclause
        && !line_starts_with_identifier_sequence(structural_tail, &["FOR"])
        && !starts_set_operator
        && (line_ends_with_keyword(structural_tail, "IN")
            || line_ends_with_keyword(structural_tail, "EXISTS")
            || line_ends_with_keyword(structural_tail, "ANY")
            || line_ends_with_keyword(structural_tail, "SOME")
            || line_ends_with_keyword(structural_tail, "ALL"))
    {
        return Some(FormatQueryOwnerKind::Condition);
    }

    None
}

/// Returns the formatter query-owner kind when `line` owns a nested query body
/// through a trailing parenthesized group and should therefore participate in
/// the query-owner indentation stack.
pub(crate) fn format_query_owner_kind(line: &str) -> Option<FormatQueryOwnerKind> {
    line_ends_with_open_paren_before_inline_comment(line)
        .then(|| format_query_owner_header_kind(line))
        .flatten()
}

/// Detects split query-owner headers whose nested query starts on a later
/// standalone `(` line followed by a query head.
///
/// The formatter's analyzer and indentation phases both use this helper so the
/// owner-family classification stays consistent for nested wrapper/query-owner
/// constructs such as `CURSOR`, `MULTISET`, `LATERAL`, and `TABLE`.
///
/// Exact completed owner anchors like `CROSS APPLY` / `OUTER APPLY` are
/// intentionally resolved through `format_query_owner_header_kind(...)`
/// instead of this narrower split-lookahead helper.
pub(crate) fn split_query_owner_lookahead_kind(
    line: &str,
    next_code_is_standalone_open_paren: bool,
    head_trimmed_upper: Option<&str>,
) -> Option<SplitQueryOwnerLookaheadKind> {
    if line_ends_with_open_paren_before_inline_comment(line)
        || !next_code_is_standalone_open_paren
        || !head_trimmed_upper.is_some_and(line_starts_query_head)
    {
        return None;
    }

    if line_ends_with_format_expression_query_owner_keyword(line)
        && format_query_owner_header_kind(line).is_none()
        && format_indented_paren_owner_header_kind(line).is_none()
        && format_query_owner_pending_header_kind(line).is_none()
        && format_indented_paren_pending_header_kind(line).is_none()
    {
        return Some(SplitQueryOwnerLookaheadKind::GenericExpression);
    }

    (line_ends_with_format_split_direct_from_item_query_owner_keyword(line)
        && format_query_owner_pending_header_kind(line).is_none()
        && format_indented_paren_pending_header_kind(line).is_none())
    .then_some(SplitQueryOwnerLookaheadKind::DirectFromItem)
}

/// Returns true when `text_upper` starts with a MODEL subclause whose body is
/// owned by a trailing parenthesized block.
pub(crate) fn starts_with_format_model_subclause(text_upper: &str) -> bool {
    FormatIndentedParenOwnerKind::ModelSubclause.starts_body_header(text_upper)
}

pub(crate) fn starts_with_format_match_recognize_subclause(text_upper: &str) -> bool {
    FormatIndentedParenOwnerKind::MatchRecognize.starts_body_header(text_upper)
}

pub(crate) fn format_indented_paren_owner_kind_from_words(
    words: &[&str],
) -> Option<FormatIndentedParenOwnerKind> {
    let first_word = words.first().copied()?;
    let second_word = words.get(1).copied();
    let third_word = words.get(2).copied();

    let last_word = words.last().copied()?;
    let penultimate_word = words.iter().rev().nth(1).copied();
    let third_from_end_word = words.iter().rev().nth(2).copied();

    if last_word.eq_ignore_ascii_case("OVER") {
        Some(FormatIndentedParenOwnerKind::AnalyticOver)
    } else if last_word.eq_ignore_ascii_case("GROUP")
        && penultimate_word.is_some_and(|word| word.eq_ignore_ascii_case("WITHIN"))
    {
        Some(FormatIndentedParenOwnerKind::WithinGroup)
    } else if last_word.eq_ignore_ascii_case("KEEP") {
        Some(FormatIndentedParenOwnerKind::Keep)
    } else if FormatIndentedParenOwnerKind::ModelSubclause.starts_body_header_words(
        first_word,
        second_word,
        third_word,
    ) {
        Some(FormatIndentedParenOwnerKind::ModelSubclause)
    } else if first_word.eq_ignore_ascii_case("WINDOW") && last_word.eq_ignore_ascii_case("AS") {
        Some(FormatIndentedParenOwnerKind::Window)
    } else if last_word.eq_ignore_ascii_case("MATCH_RECOGNIZE") {
        Some(FormatIndentedParenOwnerKind::MatchRecognize)
    } else if last_word.eq_ignore_ascii_case("PIVOT")
        || (last_word.eq_ignore_ascii_case("XML")
            && penultimate_word.is_some_and(|word| word.eq_ignore_ascii_case("PIVOT")))
    {
        Some(FormatIndentedParenOwnerKind::Pivot)
    } else if last_word.eq_ignore_ascii_case("UNPIVOT")
        || (last_word.eq_ignore_ascii_case("NULLS")
            && penultimate_word.is_some_and(|word| {
                word.eq_ignore_ascii_case("INCLUDE") || word.eq_ignore_ascii_case("EXCLUDE")
            })
            && third_from_end_word.is_some_and(|word| word.eq_ignore_ascii_case("UNPIVOT")))
    {
        Some(FormatIndentedParenOwnerKind::Unpivot)
    } else if last_word.eq_ignore_ascii_case("COLUMNS") {
        Some(FormatIndentedParenOwnerKind::StructuredColumns)
    } else if last_word.eq_ignore_ascii_case("TABLES")
        && penultimate_word.is_some_and(|word| {
            word.eq_ignore_ascii_case("VERTEX") || word.eq_ignore_ascii_case("EDGE")
        })
    {
        Some(FormatIndentedParenOwnerKind::PropertyGraphTables)
    } else {
        None
    }
}

/// Returns the structured formatter owner kind when `line` ends on the owner
/// header token itself and the parenthesized body starts on a later line.
pub(crate) fn format_indented_paren_owner_header_kind(
    line: &str,
) -> Option<FormatIndentedParenOwnerKind> {
    let structural_tail = owner_header_structural_tail(line);
    let trimmed_upper = structural_tail.to_ascii_uppercase();

    if line_ends_with_keyword(structural_tail, "OVER") {
        Some(FormatIndentedParenOwnerKind::AnalyticOver)
    } else if line_ends_with_identifier_sequence(structural_tail, &["WITHIN", "GROUP"]) {
        Some(FormatIndentedParenOwnerKind::WithinGroup)
    } else if line_ends_with_keyword(structural_tail, "KEEP") {
        Some(FormatIndentedParenOwnerKind::Keep)
    } else if starts_with_format_model_subclause(&trimmed_upper) {
        Some(FormatIndentedParenOwnerKind::ModelSubclause)
    } else if line_starts_with_identifier_sequence(structural_tail, &["WINDOW"])
        && line_ends_with_keyword(structural_tail, "AS")
    {
        Some(FormatIndentedParenOwnerKind::Window)
    } else if line_ends_with_keyword(structural_tail, "MATCH_RECOGNIZE") {
        Some(FormatIndentedParenOwnerKind::MatchRecognize)
    } else if line_ends_with_pivot_owner(structural_tail) {
        Some(FormatIndentedParenOwnerKind::Pivot)
    } else if line_ends_with_unpivot_owner(structural_tail) {
        Some(FormatIndentedParenOwnerKind::Unpivot)
    } else if line_ends_with_keyword(structural_tail, "COLUMNS") {
        Some(FormatIndentedParenOwnerKind::StructuredColumns)
    } else if line_ends_with_identifier_sequence(structural_tail, &["VERTEX", "TABLES"])
        || line_ends_with_identifier_sequence(structural_tail, &["EDGE", "TABLES"])
    {
        Some(FormatIndentedParenOwnerKind::PropertyGraphTables)
    } else {
        None
    }
}

fn line_trailing_identifiers_before_inline_comment(
    line: &str,
    max_identifiers: usize,
) -> (Option<u8>, Vec<&str>) {
    let bytes = line.as_bytes();
    let fast_path_safe = !bytes
        .iter()
        .any(|byte| matches!(*byte, b'\'' | b'"' | b'`' | b'#' | b'-' | b'/'));
    if fast_path_safe {
        let mut end = bytes.len();
        while end > 0 && bytes[end - 1].is_ascii_whitespace() {
            end -= 1;
        }
        if end == 0 {
            return (None, Vec::new());
        }

        let last_significant_byte = Some(bytes[end - 1]);
        let mut trailing_identifiers = Vec::with_capacity(max_identifiers);
        let mut idx = end;

        while idx > 0 {
            while idx > 0 && bytes[idx - 1].is_ascii_whitespace() {
                idx -= 1;
            }
            if idx == 0 {
                break;
            }

            if is_identifier_byte(bytes[idx - 1]) {
                let token_end = idx;
                while idx > 0 && is_identifier_byte(bytes[idx - 1]) {
                    idx -= 1;
                }
                if !is_identifier_start_byte(bytes[idx]) {
                    continue;
                }

                if max_identifiers > 0 {
                    if let Some(token) = line.get(idx..token_end) {
                        trailing_identifiers.push(token);
                    }
                    if trailing_identifiers.len() == max_identifiers {
                        break;
                    }
                }
                continue;
            }

            idx -= 1;
        }

        trailing_identifiers.reverse();
        return (last_significant_byte, trailing_identifiers);
    }

    let mut idx = 0usize;
    let mut in_single_quote = false;
    let mut in_double_quote = false;
    let mut in_block_comment = false;
    let mut q_quote_end: Option<u8> = None;
    let mut last_significant_byte: Option<u8> = None;
    let mut trailing_identifiers = Vec::with_capacity(max_identifiers);

    while idx < bytes.len() {
        let current = bytes[idx];
        let next = bytes.get(idx.saturating_add(1)).copied();

        if in_block_comment {
            if current == b'*' && next == Some(b'/') {
                in_block_comment = false;
                idx = idx.saturating_add(2);
                continue;
            }
            idx = idx.saturating_add(1);
            continue;
        }

        if let Some(closing) = q_quote_end {
            if current == closing && next == Some(b'\'') {
                q_quote_end = None;
                idx = idx.saturating_add(2);
                continue;
            }
            idx = idx.saturating_add(1);
            continue;
        }

        if in_single_quote {
            if current == b'\'' {
                if next == Some(b'\'') {
                    idx = idx.saturating_add(2);
                    continue;
                }
                in_single_quote = false;
            }
            idx = idx.saturating_add(1);
            continue;
        }

        if in_double_quote {
            if current == b'"' {
                if next == Some(b'"') {
                    idx = idx.saturating_add(2);
                    continue;
                }
                in_double_quote = false;
            }
            idx = idx.saturating_add(1);
            continue;
        }

        if sql_line_comment_prefix_len(bytes, idx).is_some() {
            break;
        }
        if current == b'/' && next == Some(b'*') {
            in_block_comment = true;
            idx = idx.saturating_add(2);
            continue;
        }
        if let Some(q_quote_start) = q_quote_prefix_at(bytes, idx) {
            q_quote_end = Some(q_quote_start.closing_delimiter);
            idx = idx.saturating_add(q_quote_start.prefix_len);
            continue;
        }
        if current == b'\'' {
            in_single_quote = true;
            idx = idx.saturating_add(1);
            continue;
        }
        if current == b'"' {
            in_double_quote = true;
            idx = idx.saturating_add(1);
            continue;
        }
        if current.is_ascii_whitespace() {
            idx = idx.saturating_add(1);
            continue;
        }

        if is_identifier_start_byte(current) {
            let start = idx;
            idx = idx.saturating_add(1);
            while idx < bytes.len() && is_identifier_byte(bytes[idx]) {
                idx = idx.saturating_add(1);
            }

            last_significant_byte = Some(b'a');
            if max_identifiers > 0 {
                if trailing_identifiers.len() == max_identifiers {
                    trailing_identifiers.remove(0);
                }
                if let Some(token) = line.get(start..idx) {
                    trailing_identifiers.push(token);
                }
            }
            continue;
        }

        last_significant_byte = Some(current);
        idx = idx.saturating_add(1);
    }

    (last_significant_byte, trailing_identifiers)
}

/// Returns the trailing identifier words before an inline comment or end of
/// line, skipping quoted literals and inline block comments.
#[cfg_attr(not(test), allow(dead_code))]
pub(crate) fn trailing_identifier_words_before_inline_comment(
    line: &str,
    max_identifiers: usize,
) -> Vec<&str> {
    line_trailing_identifiers_before_inline_comment(line, max_identifiers).1
}

/// Returns true when the trailing identifier sequence before an inline comment
/// or end of line matches `sequence`.
pub(crate) fn line_ends_with_identifier_sequence_before_inline_comment(
    line: &str,
    sequence: &[&str],
) -> bool {
    line_ends_with_identifier_sequence(line, sequence)
}

fn skip_alias_tail_whitespace_and_block_comments(bytes: &[u8], mut idx: usize) -> Option<usize> {
    while idx < bytes.len() {
        let current = bytes[idx];
        let next = bytes.get(idx.saturating_add(1)).copied();
        if current.is_ascii_whitespace() {
            idx = idx.saturating_add(1);
            continue;
        }
        if current == b'/' && next == Some(b'*') {
            idx = idx.saturating_add(2);
            let mut closed_comment = false;
            while idx + 1 < bytes.len() {
                if bytes[idx] == b'*' && bytes[idx + 1] == b'/' {
                    idx = idx.saturating_add(2);
                    closed_comment = true;
                    break;
                }
                idx = idx.saturating_add(1);
            }
            if !closed_comment {
                return None;
            }
            continue;
        }
        break;
    }

    Some(idx)
}

fn consume_alias_tail_identifier(bytes: &[u8], start: usize) -> Option<usize> {
    let quote = *bytes.get(start)?;
    if matches!(quote, b'"' | b'`') {
        let mut idx = start.saturating_add(1);
        while idx < bytes.len() {
            if bytes[idx] == quote {
                if bytes.get(idx.saturating_add(1)) == Some(&quote) {
                    idx = idx.saturating_add(2);
                    continue;
                }
                return Some(idx.saturating_add(1));
            }
            idx = idx.saturating_add(1);
        }
        return None;
    }

    if !is_identifier_start_byte(quote) {
        return None;
    }

    let mut idx = start.saturating_add(1);
    while idx < bytes.len() && is_identifier_byte(bytes[idx]) {
        idx = idx.saturating_add(1);
    }
    Some(idx)
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum AutoFormatAliasTailTermination {
    Delimited,
    EndOfTail,
}

fn auto_format_structural_tail_alias_termination(
    line: &str,
) -> Option<AutoFormatAliasTailTermination> {
    let tail = owner_header_structural_tail(line);
    let bytes = tail.as_bytes();
    let mut idx = skip_alias_tail_whitespace_and_block_comments(bytes, 0)?;
    if idx >= bytes.len() || sql_line_comment_prefix_len(bytes, idx).is_some() {
        return None;
    }

    let token_start = idx;
    let token_end = consume_alias_tail_identifier(bytes, token_start)?;
    idx = token_end;

    if tail
        .get(token_start..token_end)
        .is_some_and(|token| token.eq_ignore_ascii_case("AS"))
    {
        let next_idx = skip_alias_tail_whitespace_and_block_comments(bytes, idx)?;
        idx = next_idx;
        if idx >= bytes.len() || sql_line_comment_prefix_len(bytes, idx).is_some() {
            return None;
        }
        let alias_end = consume_alias_tail_identifier(bytes, idx)?;
        idx = alias_end;
    }

    let mut saw_trailing_delimiter = false;
    loop {
        let next_idx = skip_alias_tail_whitespace_and_block_comments(bytes, idx)?;
        idx = next_idx;
        if idx >= bytes.len() || sql_line_comment_prefix_len(bytes, idx).is_some() {
            return Some(if saw_trailing_delimiter {
                AutoFormatAliasTailTermination::Delimited
            } else {
                AutoFormatAliasTailTermination::EndOfTail
            });
        }
        match bytes[idx] {
            b',' | b';' => {
                saw_trailing_delimiter = true;
                idx = idx.saturating_add(1);
            }
            _ => return None,
        }
    }
}

/// Returns true when the shared structural tail is exactly an alias fragment:
/// optional `AS`, then one identifier/quoted identifier, followed only by
/// comments or an optional trailing delimiter.
pub(crate) fn auto_format_structural_tail_is_alias_fragment(line: &str) -> bool {
    auto_format_structural_tail_alias_termination(line).is_some()
}

/// Returns true when the shared structural tail is exactly a delimited alias
/// fragment: optional `AS`, then one identifier/quoted identifier, followed
/// only by a trailing delimiter, punctuation, or comments.
pub(crate) fn auto_format_structural_tail_is_simple_alias(line: &str) -> bool {
    matches!(
        auto_format_structural_tail_alias_termination(line),
        Some(AutoFormatAliasTailTermination::Delimited)
    )
}

fn line_ends_with_identifier_sequence(line: &str, sequence: &[&str]) -> bool {
    if sequence.is_empty() {
        return true;
    }

    let expected_segment_count = identifier_sequence_segment_count(sequence);
    let (_, trailing_identifiers) =
        line_trailing_identifiers_before_inline_comment(line, expected_segment_count);
    let mut start_idx = trailing_identifiers.len();
    let mut consumed_segments = 0usize;

    while start_idx > 0 && consumed_segments < expected_segment_count {
        start_idx = start_idx.saturating_sub(1);
        let identifier_segment_count = identifier_segment_count(trailing_identifiers[start_idx]);
        consumed_segments = consumed_segments.saturating_add(identifier_segment_count);
        if consumed_segments > expected_segment_count {
            return false;
        }
    }

    consumed_segments == expected_segment_count
        && identifier_word_matches_keyword_sequence(&trailing_identifiers[start_idx..], sequence)
}

fn line_ends_with_pivot_owner(line: &str) -> bool {
    line_ends_with_identifier_sequence(line, &["PIVOT"])
        || line_ends_with_identifier_sequence(line, &["PIVOT", "XML"])
}

fn line_ends_with_unpivot_owner(line: &str) -> bool {
    line_ends_with_identifier_sequence(line, &["UNPIVOT"])
        || line_ends_with_identifier_sequence(line, &["UNPIVOT", "INCLUDE", "NULLS"])
        || line_ends_with_identifier_sequence(line, &["UNPIVOT", "EXCLUDE", "NULLS"])
}

pub(crate) fn line_ends_with_format_expression_query_owner_keyword(line: &str) -> bool {
    line_ends_with_identifier_sequence(line, &["CURSOR"])
        || line_ends_with_identifier_sequence(line, &["MULTISET"])
}

fn line_ends_with_format_table_from_item_query_owner_keyword(line: &str) -> bool {
    (line_starts_with_identifier_sequence(line, &["TABLE"])
        || line_ends_with_identifier_sequence(line, &["FROM", "TABLE"])
        || line_ends_with_identifier_sequence(line, &["JOIN", "TABLE"])
        || line_ends_with_identifier_sequence(line, &["APPLY", "TABLE"])
        || line_ends_with_identifier_sequence(line, &["USING", "TABLE"]))
        && line_ends_with_identifier_sequence(line, &["TABLE"])
}

/// Returns true for direct FROM-item owner headers that are safe to treat as
/// split-owner lookahead when a standalone `(` and child query head follow.
///
/// Exact completed owners like `CROSS APPLY` / `OUTER APPLY` intentionally do
/// not participate here; they stay on the completed-owner-anchor path.
pub(crate) fn line_ends_with_format_split_direct_from_item_query_owner_keyword(line: &str) -> bool {
    line_ends_with_identifier_sequence(line, &["LATERAL"])
        || line_ends_with_format_table_from_item_query_owner_keyword(line)
}

/// Returns true when the structural tail starts with an exact bare direct
/// FROM-item owner (`LATERAL`, `TABLE`) that should keep the sibling item
/// depth even if leading comments are glued onto the line.
pub(crate) fn line_starts_with_format_bare_direct_from_item_query_owner(line: &str) -> bool {
    let structural_tail = auto_format_structural_tail(line);
    line_starts_with_identifier_sequence(structural_tail, &["LATERAL"])
        || line_starts_with_identifier_sequence(structural_tail, &["TABLE"])
}

fn line_is_format_same_depth_deferred_wrapper_owner(line: &str) -> bool {
    matches!(
        format_query_owner_header_kind(line),
        Some(FormatQueryOwnerKind::Condition | FormatQueryOwnerKind::FromItem)
    ) || line_ends_with_format_expression_query_owner_keyword(line)
}

fn line_ends_with_keyword(line: &str, keyword: &str) -> bool {
    line_ends_with_identifier_sequence(line, &[keyword])
}

pub(crate) fn line_ends_with_semicolon_before_inline_comment(line: &str) -> bool {
    matches!(
        trailing_meaningful_tokens_before_inline_comment(line).1,
        Some(FormatTrailingMeaningfulToken::Symbol(symbol))
            if symbol.as_bytes().last() == Some(&b';')
    )
}

/// Returns the structured formatter block kind when a line owns a multiline
/// parenthesized body that should be tracked on a dedicated indentation stack.
pub(crate) fn format_indented_paren_owner_kind(line: &str) -> Option<FormatIndentedParenOwnerKind> {
    line_ends_with_open_paren_before_inline_comment(line)
        .then(|| format_indented_paren_owner_header_kind(line))
        .flatten()
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum SignificantParenEvent {
    Open,
    Close,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub(crate) struct SignificantParenProfile {
    pub(crate) events: Vec<SignificantParenEvent>,
    pub(crate) leading_close_count: usize,
}

/// Returns the ordered sequence of significant `(` / `)` tokens that appear on
/// `line`, excluding content inside comments or quoted literals. The profile
/// also tracks how many close-paren events occur before any other significant
/// token so indentation code can consume nested owner stacks in the same order
/// as the visible leading `)` sequence.
pub(crate) fn significant_paren_profile(line: &str) -> SignificantParenProfile {
    let bytes = line.as_bytes();
    let mut idx = 0usize;
    let mut profile = SignificantParenProfile::default();
    let mut in_single_quote = false;
    let mut in_double_quote = false;
    let mut in_backtick_quote = false;
    let mut in_block_comment = false;
    let mut q_quote_end: Option<u8> = None;
    let mut dollar_quote_tag: Option<Vec<u8>> = None;
    let mut still_in_leading_close_run = true;

    while idx < bytes.len() {
        let current = bytes[idx];
        let next = bytes.get(idx.saturating_add(1)).copied();

        if in_block_comment {
            if current == b'*' && next == Some(b'/') {
                in_block_comment = false;
                idx = idx.saturating_add(2);
                continue;
            }
            idx = idx.saturating_add(1);
            continue;
        }

        if let Some(tag) = dollar_quote_tag.as_ref() {
            let tag_len = tag.len();
            let closes_here = bytes
                .get(idx..)
                .is_some_and(|tail| tail.starts_with(tag.as_slice()));
            if closes_here {
                dollar_quote_tag = None;
                idx = idx.saturating_add(tag_len);
                continue;
            }
            idx = idx.saturating_add(1);
            continue;
        }

        if let Some(closing) = q_quote_end {
            if current == closing && next == Some(b'\'') {
                q_quote_end = None;
                idx = idx.saturating_add(2);
                continue;
            }
            idx = idx.saturating_add(1);
            continue;
        }

        if in_single_quote {
            if current == b'\'' {
                if next == Some(b'\'') {
                    idx = idx.saturating_add(2);
                    continue;
                }
                in_single_quote = false;
            }
            idx = idx.saturating_add(1);
            continue;
        }

        if in_double_quote {
            if current == b'"' {
                if next == Some(b'"') {
                    idx = idx.saturating_add(2);
                    continue;
                }
                in_double_quote = false;
            }
            idx = idx.saturating_add(1);
            continue;
        }

        if in_backtick_quote {
            if current == b'`' {
                if next == Some(b'`') {
                    idx = idx.saturating_add(2);
                    continue;
                }
                in_backtick_quote = false;
            }
            idx = idx.saturating_add(1);
            continue;
        }

        if still_in_leading_close_run && current == b'*' && next == Some(b'/') {
            idx = idx.saturating_add(2);
            continue;
        }
        if sql_line_comment_prefix_len(bytes, idx).is_some() {
            break;
        }
        if current == b'/' && next == Some(b'*') {
            in_block_comment = true;
            idx = idx.saturating_add(2);
            continue;
        }
        if current == b'$' {
            if let Some(tag) = parse_dollar_quote_tag_bytes(bytes, idx) {
                if !looks_like_oracle_conditional_compilation_flag_bytes(bytes, idx) {
                    still_in_leading_close_run = false;
                    let tag_len = tag.len();
                    dollar_quote_tag = Some(tag);
                    idx = idx.saturating_add(tag_len);
                    continue;
                }
            }
        }
        if let Some(q_quote_start) = q_quote_prefix_at(bytes, idx) {
            q_quote_end = Some(q_quote_start.closing_delimiter);
            still_in_leading_close_run = false;
            idx = idx.saturating_add(q_quote_start.prefix_len);
            continue;
        }
        if current == b'\'' {
            in_single_quote = true;
            still_in_leading_close_run = false;
            idx = idx.saturating_add(1);
            continue;
        }
        if current == b'"' {
            in_double_quote = true;
            still_in_leading_close_run = false;
            idx = idx.saturating_add(1);
            continue;
        }
        if current == b'`' {
            in_backtick_quote = true;
            still_in_leading_close_run = false;
            idx = idx.saturating_add(1);
            continue;
        }
        if current.is_ascii_whitespace() {
            idx = idx.saturating_add(1);
            continue;
        }

        match current {
            b'(' => {
                still_in_leading_close_run = false;
                profile.events.push(SignificantParenEvent::Open);
            }
            b')' => {
                if still_in_leading_close_run {
                    profile.leading_close_count = profile.leading_close_count.saturating_add(1);
                }
                profile.events.push(SignificantParenEvent::Close);
            }
            _ => {
                still_in_leading_close_run = false;
            }
        }

        idx = idx.saturating_add(1);
    }

    profile
}

pub(crate) fn significant_paren_depth_after_profile(
    mut depth: usize,
    paren_profile: &SignificantParenProfile,
) -> usize {
    for event in &paren_profile.events {
        match event {
            SignificantParenEvent::Open => {
                depth = depth.saturating_add(1);
            }
            SignificantParenEvent::Close => {
                depth = depth.saturating_sub(1);
            }
        }
    }

    depth
}

pub(crate) fn line_has_leading_significant_close_paren(line: &str) -> bool {
    significant_paren_profile(line).leading_close_count > 0
}

/// Returns the structural prefix that owner/header classifiers should read.
///
/// This always strips leading comments and any leading close-paren run so
/// owner/pending helpers can reason from the surviving structural tail without
/// depending on each caller to normalize mixed leading-close lines first.
fn owner_header_structural_tail(line: &str) -> &str {
    trim_after_leading_close_parens(trim_leading_sql_comments(line))
}

/// Returns true when the meaningful structure on `line` is only a leading
/// close-paren run plus optional comments/whitespace.
///
/// Pending header families that track nested paren wrappers must still observe
/// that close event even when no structural tail survives after normalization.
fn owner_header_has_only_leading_close_parens(line: &str) -> bool {
    let trimmed = trim_leading_sql_comments(line);
    line_has_leading_significant_close_paren(trimmed)
        && trim_after_leading_close_parens(trimmed).is_empty()
}

fn owner_header_has_only_semicolon_tail_after_leading_close(line: &str) -> bool {
    let trimmed = trim_leading_sql_comments(line);
    if !line_has_leading_significant_close_paren(trimmed) {
        return false;
    }

    let structural_tail = trim_after_leading_close_parens(trimmed);
    matches!(first_meaningful_word(structural_tail), Some(";"))
        && next_meaningful_word(structural_tail, 1).is_none()
}

/// Returns the meaningful remainder of `line` after consuming any leading
/// close-paren run, including intervening whitespace or block comments.
pub(crate) fn trim_after_leading_close_parens(line: &str) -> &str {
    let bytes = line.as_bytes();
    let mut idx = 0usize;
    let mut consumed_close = false;

    loop {
        while idx < bytes.len() && bytes[idx].is_ascii_whitespace() {
            idx = idx.saturating_add(1);
        }

        if idx + 1 < bytes.len() && bytes[idx] == b'/' && bytes[idx + 1] == b'*' {
            idx = idx.saturating_add(2);
            while idx + 1 < bytes.len() {
                if bytes[idx] == b'*' && bytes[idx + 1] == b'/' {
                    idx = idx.saturating_add(2);
                    break;
                }
                idx = idx.saturating_add(1);
            }
            continue;
        }

        if sql_line_comment_prefix_len(bytes, idx).is_some() {
            return "";
        }

        if bytes.get(idx) == Some(&b')') {
            consumed_close = true;
            idx = idx.saturating_add(1);
            continue;
        }

        break;
    }

    if consumed_close {
        line.get(idx..).map(str::trim_start).unwrap_or("")
    } else {
        line.trim_start()
    }
}

/// Returns the structural classification tail for auto-formatting helpers.
///
/// Pure close lines stay as-is, but mixed leading-close lines consume the
/// close segment first so clause/header/continuation helpers can classify the
/// surviving structural tail (`) ORDER BY` -> `ORDER BY`, `) FOR UPDATE` ->
/// `FOR UPDATE`, `) AND EXISTS (` -> `AND EXISTS (`).
pub(crate) fn auto_format_structural_tail(line: &str) -> &str {
    let trimmed = trim_leading_sql_comments(line);
    if trimmed.is_empty() {
        return trimmed;
    }

    if line_has_mixed_leading_close_continuation(trimmed) {
        trim_after_leading_close_parens(trimmed)
    } else {
        trimmed
    }
}

/// Returns true when the meaningful remainder of a leading-close line keeps
/// evaluating the same expression after the close has been consumed.
///
/// Examples:
/// - `) AND EXISTS (` -> true
/// - `), 0` -> true
/// - `)` / `),` / `) JOIN t` -> false
pub(crate) fn line_continues_expression_after_leading_close(line: &str) -> bool {
    let trimmed = trim_after_leading_close_parens(line);
    let Some(first_token) = first_meaningful_word(trimmed) else {
        return false;
    };

    match first_token {
        "," => {
            let remainder = trimmed.get(first_token.len()..).unwrap_or("");
            first_meaningful_word(remainder).is_some()
        }
        symbol if is_format_expression_continuation_symbol(symbol) => true,
        _ => starts_with_format_expression_continuation_sequence(trimmed),
    }
}

/// Returns true when a leading-close line must keep interpreting the remaining
/// tokens structurally after consuming the close segment.
///
/// This covers both expression continuations (`) AND ...`, `), value`) and
/// clause/query-header transitions such as `) ORDER BY ...` or
/// `) UPDATE demo ...`.
pub(crate) fn line_has_mixed_leading_close_continuation(line: &str) -> bool {
    let trimmed_line = trim_leading_sql_comments(line);
    if !line_has_leading_significant_close_paren(trimmed_line) {
        return false;
    }

    if auto_format_structural_tail_is_simple_alias(trimmed_line) {
        return false;
    }

    if line_continues_expression_after_leading_close(trimmed_line) {
        return true;
    }

    let trimmed = trim_after_leading_close_parens(trimmed_line);
    let Some(first_token) = first_meaningful_word(trimmed) else {
        return false;
    };

    starts_with_auto_format_structural_continuation_boundary_without_expression_owner_impl(trimmed)
        || starts_with_auto_format_owner_boundary(trimmed)
        || format_bare_structural_header_continuation_kind_for_structural_tail(trimmed).is_some()
        || is_format_comment_continuation_keyword(first_token)
        || is_non_subquery_paren_suppressed_clause_continuation(trimmed)
}

/// Returns true when a leading keyword should preserve the next line as a
/// continuation after a comment split.
pub(crate) fn is_format_comment_continuation_keyword(word: &str) -> bool {
    is_format_comment_header_keyword(word) || is_format_expression_continuation_keyword(word)
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum FormatTrailingMeaningfulToken<'a> {
    Word(&'a str),
    Symbol(&'a str),
    Other,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum FormatTrailingContinuationOperatorKind {
    Keyword,
    Symbol,
}

fn push_format_trailing_meaningful_token<'a>(
    previous: &mut Option<FormatTrailingMeaningfulToken<'a>>,
    last: &mut Option<FormatTrailingMeaningfulToken<'a>>,
    token: FormatTrailingMeaningfulToken<'a>,
) {
    *previous = *last;
    *last = Some(token);
}

fn is_format_identifier_start_byte(byte: u8) -> bool {
    byte.is_ascii_alphabetic() || matches!(byte, b'_' | b'$' | b'#')
}

fn is_format_identifier_continue_byte(byte: u8) -> bool {
    is_format_identifier_start_byte(byte) || byte.is_ascii_digit()
}

fn trailing_meaningful_tokens_before_inline_comment(
    line: &str,
) -> (
    Option<FormatTrailingMeaningfulToken<'_>>,
    Option<FormatTrailingMeaningfulToken<'_>>,
) {
    let bytes = line.as_bytes();
    let mut idx = 0usize;
    let mut previous = None;
    let mut last = None;

    while idx < bytes.len() {
        let Some(remaining) = line.get(idx..) else {
            break;
        };

        if sql_line_comment_prefix_len(bytes, idx).is_some() {
            break;
        }

        if remaining.starts_with("/*") {
            let Some(block_end) = remaining.get(2..).and_then(|body| body.find("*/")) else {
                break;
            };
            idx = idx.saturating_add(block_end).saturating_add(4);
            continue;
        }

        let current = bytes[idx];

        if let Some(q_quote_start) = q_quote_prefix_at(bytes, idx) {
            let mut local_idx = idx.saturating_add(q_quote_start.prefix_len);
            while local_idx < bytes.len() {
                if bytes[local_idx] == q_quote_start.closing_delimiter
                    && bytes.get(local_idx.saturating_add(1)) == Some(&b'\'')
                {
                    local_idx = local_idx.saturating_add(2);
                    break;
                }
                local_idx = local_idx.saturating_add(1);
            }
            push_format_trailing_meaningful_token(
                &mut previous,
                &mut last,
                FormatTrailingMeaningfulToken::Other,
            );
            idx = local_idx.min(bytes.len());
            continue;
        }

        if current == b'\'' {
            let mut local_idx = idx.saturating_add(1);
            while local_idx < bytes.len() {
                if bytes[local_idx] == b'\'' {
                    if bytes.get(local_idx.saturating_add(1)) == Some(&b'\'') {
                        local_idx = local_idx.saturating_add(2);
                        continue;
                    }
                    local_idx = local_idx.saturating_add(1);
                    break;
                }
                local_idx = local_idx.saturating_add(1);
            }
            push_format_trailing_meaningful_token(
                &mut previous,
                &mut last,
                FormatTrailingMeaningfulToken::Other,
            );
            idx = local_idx.min(bytes.len());
            continue;
        }

        if current == b'"' {
            let mut local_idx = idx.saturating_add(1);
            while local_idx < bytes.len() {
                if bytes[local_idx] == b'"' {
                    local_idx = local_idx.saturating_add(1);
                    break;
                }
                local_idx = local_idx.saturating_add(1);
            }
            push_format_trailing_meaningful_token(
                &mut previous,
                &mut last,
                FormatTrailingMeaningfulToken::Other,
            );
            idx = local_idx.min(bytes.len());
            continue;
        }

        let Some(ch) = remaining.chars().next() else {
            break;
        };
        if ch.is_whitespace() {
            idx = idx.saturating_add(ch.len_utf8());
            continue;
        }

        if is_format_identifier_start_byte(current) {
            let start = idx;
            idx = idx.saturating_add(1);
            while idx < bytes.len() && is_format_identifier_continue_byte(bytes[idx]) {
                idx = idx.saturating_add(1);
            }
            push_format_trailing_meaningful_token(
                &mut previous,
                &mut last,
                line.get(start..idx)
                    .map(FormatTrailingMeaningfulToken::Word)
                    .unwrap_or(FormatTrailingMeaningfulToken::Other),
            );
            continue;
        }

        if current.is_ascii_digit() {
            idx = idx.saturating_add(1);
            while idx < bytes.len() {
                let next_byte = bytes[idx];
                if next_byte.is_ascii_whitespace()
                    || (next_byte == b'-' && bytes.get(idx.saturating_add(1)) == Some(&b'-'))
                    || (next_byte == b'/' && bytes.get(idx.saturating_add(1)) == Some(&b'*'))
                    || next_byte == b'\''
                    || next_byte == b'"'
                {
                    break;
                }
                if next_byte.is_ascii_punctuation() && next_byte != b'.' {
                    break;
                }
                idx = idx.saturating_add(1);
            }
            push_format_trailing_meaningful_token(
                &mut previous,
                &mut last,
                FormatTrailingMeaningfulToken::Other,
            );
            continue;
        }

        let start = idx;
        idx = idx.saturating_add(1);
        while idx < bytes.len() {
            let next_byte = bytes[idx];
            if next_byte.is_ascii_whitespace()
                || is_format_identifier_start_byte(next_byte)
                || next_byte.is_ascii_digit()
                || next_byte == b'\''
                || next_byte == b'"'
                || (next_byte == b'-' && bytes.get(idx.saturating_add(1)) == Some(&b'-'))
                || (next_byte == b'/' && bytes.get(idx.saturating_add(1)) == Some(&b'*'))
            {
                break;
            }
            idx = idx.saturating_add(1);
        }

        push_format_trailing_meaningful_token(
            &mut previous,
            &mut last,
            line.get(start..idx)
                .map(FormatTrailingMeaningfulToken::Symbol)
                .unwrap_or(FormatTrailingMeaningfulToken::Other),
        );
    }

    (previous, last)
}

pub(crate) fn is_format_expression_continuation_keyword(word: &str) -> bool {
    matches_keyword(word, FORMAT_EXPRESSION_CONTINUATION_KEYWORDS)
}

pub(crate) fn format_source_gap_is_canonical_inline(
    previous_word: Option<&str>,
    current_word: &str,
) -> bool {
    is_format_expression_continuation_keyword(current_word)
        || previous_word.is_some_and(is_format_expression_continuation_keyword)
        || (current_word.eq_ignore_ascii_case("FOR")
            && previous_word.is_some_and(|word| {
                word.eq_ignore_ascii_case("PARTITION") || word.eq_ignore_ascii_case("SUBPARTITION")
            }))
}

fn is_format_comment_header_keyword(word: &str) -> bool {
    matches_keyword(word, FORMAT_LAYOUT_CLAUSE_START_KEYWORDS)
        || matches_keyword(word, FORMAT_INLINE_COMMENT_HEADER_SAME_DEPTH_KEYWORDS)
        || matches_keyword(word, FORMAT_INLINE_COMMENT_HEADER_QUERY_BASE_KEYWORDS)
        || matches_keyword(word, FORMAT_INLINE_COMMENT_HEADER_CURRENT_LINE_KEYWORDS)
        || matches_keyword(word, FORMAT_CONDITION_KEYWORDS)
        || matches_keyword(word, FORMAT_JOIN_MODIFIER_KEYWORDS)
}

fn is_format_expression_continuation_of_prefix_keyword(word: &str) -> bool {
    matches_keyword(word, FORMAT_EXPRESSION_CONTINUATION_OF_PREFIX_KEYWORDS)
}

fn starts_with_format_expression_continuation_sequence(text: &str) -> bool {
    line_starts_with_identifier_sequence(text, &["IS", "OF"])
        || line_starts_with_identifier_sequence(text, &["MEMBER", "OF"])
        || line_starts_with_identifier_sequence(text, &["SUBMULTISET", "OF"])
        || FORMAT_EXPRESSION_CONTINUATION_KEYWORDS
            .iter()
            .any(|keyword| line_starts_with_identifier_sequence(text, &[*keyword]))
}

fn is_format_expression_continuation_symbol(symbol: &str) -> bool {
    matches!(
        symbol,
        ":=" | "=>"
            | "="
            | "<"
            | ">"
            | "<="
            | ">="
            | "<>"
            | "!="
            | "<=>"
            | "+"
            | "-"
            | "*"
            | "/"
            | "%"
            | "||"
            | "|"
            | "^"
    )
}

pub(crate) fn format_trailing_continuation_operator_kind_from_token_pair(
    previous: Option<FormatTrailingMeaningfulToken<'_>>,
    last: FormatTrailingMeaningfulToken<'_>,
) -> Option<FormatTrailingContinuationOperatorKind> {
    match last {
        FormatTrailingMeaningfulToken::Word(word) => {
            let word_upper = word.to_ascii_uppercase();
            (is_format_expression_continuation_keyword(word_upper.as_str())
                || (word_upper == "OF"
                    && matches!(
                        previous,
                        Some(FormatTrailingMeaningfulToken::Word(previous_word))
                            if is_format_expression_continuation_of_prefix_keyword(previous_word)
                    )))
            .then_some(FormatTrailingContinuationOperatorKind::Keyword)
        }
        FormatTrailingMeaningfulToken::Symbol(symbol) => match symbol {
            "*" => (!matches!(
                previous,
                Some(FormatTrailingMeaningfulToken::Word(word))
                    if word.eq_ignore_ascii_case("SELECT")
            ))
            .then_some(FormatTrailingContinuationOperatorKind::Symbol),
            "/" => previous
                .is_some()
                .then_some(FormatTrailingContinuationOperatorKind::Symbol),
            _ => is_format_expression_continuation_symbol(symbol)
                .then_some(FormatTrailingContinuationOperatorKind::Symbol),
        },
        FormatTrailingMeaningfulToken::Other => None,
    }
}

/// Returns the trailing continuation-operator family for `line` after skipping
/// inline comments and quoted literals.
pub(crate) fn format_trailing_continuation_operator_kind(
    line: &str,
) -> Option<FormatTrailingContinuationOperatorKind> {
    let (previous, last) = trailing_meaningful_tokens_before_inline_comment(line);
    last.and_then(|last| format_trailing_continuation_operator_kind_from_token_pair(previous, last))
}

/// Returns true when the last meaningful token before an inline comment or
/// end-of-line is an operator that keeps the next line as an RHS continuation.
pub(crate) fn line_has_trailing_format_continuation_operator(line: &str) -> bool {
    format_trailing_continuation_operator_kind(line).is_some()
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum FormatInlineCommentHeaderContinuationKind {
    SameDepth,
    OneDeeperThanQueryBase,
    OneDeeperThanCurrentLine,
}

/// Resolves a shared structural continuation kind into the canonical
/// structural depth for the consuming line.
///
/// Keeping this translation in one place prevents analyzer- and
/// formatter-specific match arms from drifting when new continuation families
/// are added. The query-base input is an anchor, not necessarily the raw
/// stored query-frame depth; renderer-only callers may supply a synthetic base
/// that represents the same structural role.
pub(crate) fn resolve_format_header_continuation_depth_with_anchors(
    kind: FormatInlineCommentHeaderContinuationKind,
    same_depth_anchor: usize,
    current_line_depth: usize,
    query_base_anchor: Option<usize>,
) -> usize {
    match kind {
        FormatInlineCommentHeaderContinuationKind::SameDepth => same_depth_anchor,
        FormatInlineCommentHeaderContinuationKind::OneDeeperThanQueryBase => query_base_anchor
            .unwrap_or(same_depth_anchor)
            .saturating_add(1),
        FormatInlineCommentHeaderContinuationKind::OneDeeperThanCurrentLine => {
            current_line_depth.saturating_add(1)
        }
    }
}

pub(crate) fn resolve_format_header_continuation_depth(
    kind: FormatInlineCommentHeaderContinuationKind,
    line_depth: usize,
    query_base_depth: Option<usize>,
) -> usize {
    resolve_format_header_continuation_depth_with_anchors(
        kind,
        line_depth,
        line_depth,
        query_base_depth,
    )
}

/// Returns the continuation kind when an inline comment splits a clause or
/// subclause header and the next line should stay attached to that header.
pub(crate) fn format_inline_comment_header_continuation_kind(
    previous_word: Option<&str>,
    last_word: &str,
) -> Option<FormatInlineCommentHeaderContinuationKind> {
    let last_upper = last_word.to_ascii_uppercase();

    // Two-word combinations take priority over single-word matches so that
    // e.g. (LEFT, JOIN) is classified as OneDeeperThanCurrentLine rather
    // than falling through to JOIN's single-word OneDeeperThanQueryBase.
    let previous_upper = previous_word.map(str::to_ascii_uppercase);
    if let Some(ref previous_upper) = previous_upper {
        if matches!(
            (previous_upper.as_str(), last_upper.as_str()),
            ("DENSE", "RANK")
                | ("WITHIN", "GROUP")
                | ("DENSE_RANK", "FIRST")
                | ("DENSE_RANK", "LAST")
                | ("FOR", "UPDATE")
                | ("GROUP", "BY")
                | ("ORDER", "BY")
                | ("PARTITION", "BY")
                | ("DIMENSION", "BY")
                | ("AFTER", "MATCH")
                | ("MATCH", "SKIP")
                | ("SKIP", "TO")
                | ("START", "WITH")
                | ("CONNECT", "BY")
        ) || (FORMAT_JOIN_MODIFIER_KEYWORDS.contains(&previous_upper.as_str())
            && last_upper == "JOIN")
            || (previous_upper == "SELECT"
                && matches!(last_upper.as_str(), "DISTINCT" | "UNIQUE" | "ALL"))
            || (matches!(previous_upper.as_str(), "BETWEEN" | "OF")
                && is_format_temporal_boundary_keyword(last_upper.as_str()))
        {
            let continuation_kind = if matches!(
                (previous_upper.as_str(), last_upper.as_str()),
                ("FOR", "UPDATE")
            ) {
                FormatInlineCommentHeaderContinuationKind::OneDeeperThanQueryBase
            } else {
                FormatInlineCommentHeaderContinuationKind::OneDeeperThanCurrentLine
            };
            return Some(continuation_kind);
        }
    }

    if last_upper == "DENSE_RANK" {
        return Some(FormatInlineCommentHeaderContinuationKind::OneDeeperThanCurrentLine);
    }

    if matches_keyword(
        last_upper.as_str(),
        FORMAT_INLINE_COMMENT_HEADER_SAME_DEPTH_KEYWORDS,
    ) {
        return Some(FormatInlineCommentHeaderContinuationKind::SameDepth);
    }
    if matches_keyword(
        last_upper.as_str(),
        FORMAT_INLINE_COMMENT_HEADER_QUERY_BASE_KEYWORDS,
    ) {
        return Some(FormatInlineCommentHeaderContinuationKind::OneDeeperThanQueryBase);
    }
    if matches_keyword(
        last_upper.as_str(),
        FORMAT_INLINE_COMMENT_HEADER_CURRENT_LINE_KEYWORDS,
    ) {
        return Some(FormatInlineCommentHeaderContinuationKind::OneDeeperThanCurrentLine);
    }

    None
}

/// Returns the continuation kind for the leading structural header prefix on a
/// formatter/analyzer line.
///
/// This scans the leading meaningful words while they still form a keyword
/// prefix and reuses the inline-comment header classification so secondary
/// indentation can treat inline first-items (`SET col =`, `WHERE id =`,
/// `UPDATE SET col =`, ...) the same way as later list/body items.
pub(crate) fn format_leading_header_continuation_kind(
    line: &str,
) -> Option<FormatInlineCommentHeaderContinuationKind> {
    let trimmed = line.trim_start();
    if let Some(kind) = leading_structural_prefix_override_kind(trimmed) {
        return Some(kind);
    }

    if FORMAT_INLINE_COMMENT_HEADER_CURRENT_LINE_SEQUENCES
        .iter()
        .any(|sequence| line_starts_with_identifier_sequence(trimmed, sequence))
    {
        return Some(FormatInlineCommentHeaderContinuationKind::OneDeeperThanCurrentLine);
    }

    let words = leading_meaningful_words(line.trim_start(), 8);
    let mut continuation_kind = None;

    for idx in 0..words.len() {
        let word_upper = words[idx].to_ascii_uppercase();
        if !is_oracle_sql_keyword(&word_upper) {
            break;
        }

        let previous_word = if idx > 0 { Some(words[idx - 1]) } else { None };
        if let Some(kind) =
            format_inline_comment_header_continuation_kind(previous_word, words[idx])
        {
            continuation_kind = Some(kind);
        }
    }

    continuation_kind
}

fn leading_structural_prefix_override_kind(
    trimmed: &str,
) -> Option<FormatInlineCommentHeaderContinuationKind> {
    // `KEEP DENSE_RANK ...` is structurally one owner-relative body family
    // even when comments split `DENSE` / `RANK`, so reuse the shared matcher
    // instead of maintaining raw literal variants here.
    FormatIndentedParenOwnerKind::Keep
        .exact_bare_split_body_header_match(trimmed)
        .map(|_| FormatInlineCommentHeaderContinuationKind::OneDeeperThanCurrentLine)
}

fn exact_bare_join_clause_continuation_kind(
    trimmed: &str,
) -> Option<FormatInlineCommentHeaderContinuationKind> {
    if !line_is_exact_bare_structural_keyword_line(trimmed)
        || format_query_owner_header_kind(trimmed) != Some(FormatQueryOwnerKind::Clause)
        || !line_ends_with_keyword(trimmed, "JOIN")
    {
        return None;
    }

    if line_has_exact_identifier_sequence(trimmed, &["JOIN"]) {
        Some(FormatInlineCommentHeaderContinuationKind::OneDeeperThanCurrentLine)
    } else {
        Some(FormatInlineCommentHeaderContinuationKind::OneDeeperThanQueryBase)
    }
}

fn line_is_exact_bare_structural_keyword_line(trimmed: &str) -> bool {
    let words = meaningful_identifier_words_before_inline_comment(trimmed, 8);
    !words.is_empty()
        && line_has_exact_identifier_sequence(trimmed, words.as_slice())
        && words
            .iter()
            .all(|word| is_oracle_sql_keyword(&word.to_ascii_uppercase()))
}

/// Returns the shared structural continuation kind for a bare header line or
/// inline first-item prefix.
///
/// Both analyzer and formatter phase 2 use this helper so bare header splits
/// (`WHERE` -> next line operand, `FROM` -> next line item, `ON` -> next line
/// condition, ...) follow the same taxonomy as inline-comment header splits.
///
/// Structural carry still depends on the shared boundary helper above. Owner
/// header chains such as `WINDOW ... AS`, `PIVOT XML`, `RULES AUTOMATIC`, and
/// `MATCH_RECOGNIZE` subclauses are stopped there instead of being treated as
/// generic continuation consumers.
fn line_is_exact_bare_split_for_update_header(line: &str) -> bool {
    let trimmed_upper = line.to_ascii_uppercase();
    line_is_exact_bare_structural_keyword_line(line)
        && starts_with_format_for_update_split_header(&trimmed_upper)
}

pub(crate) fn format_structural_header_continuation_kind_for_structural_tail(
    trimmed: &str,
) -> Option<FormatInlineCommentHeaderContinuationKind> {
    if trimmed.is_empty() {
        return None;
    }

    if line_is_exact_bare_split_for_update_header(trimmed) {
        return Some(FormatInlineCommentHeaderContinuationKind::SameDepth);
    }

    if let Some(kind) = exact_bare_join_clause_continuation_kind(trimmed) {
        return Some(kind);
    }

    if line_has_exact_identifier_sequence(trimmed, &["SELECT"]) {
        return Some(FormatInlineCommentHeaderContinuationKind::OneDeeperThanCurrentLine);
    }

    let trimmed_upper = trimmed.to_ascii_uppercase();
    if !line_is_exact_bare_structural_keyword_line(trimmed)
        && (format_query_owner_header_kind(trimmed).is_some()
            || format_query_owner_pending_header_kind(trimmed).is_some()
            || format_indented_paren_owner_header_kind(trimmed).is_some()
            || format_indented_paren_pending_header_kind(trimmed).is_some()
            || format_plsql_child_query_owner_kind(&trimmed_upper).is_some()
            || format_plsql_child_query_owner_pending_header_kind(trimmed).is_some())
    {
        return None;
    }

    format_leading_header_continuation_kind(trimmed)
}

pub(crate) fn format_structural_header_continuation_kind(
    line: &str,
) -> Option<FormatInlineCommentHeaderContinuationKind> {
    format_structural_header_continuation_kind_for_structural_tail(auto_format_structural_tail(
        line,
    ))
}

pub(crate) fn format_bare_structural_header_continuation_kind_for_structural_tail(
    trimmed: &str,
) -> Option<FormatInlineCommentHeaderContinuationKind> {
    if let Some(kind) = exact_bare_indented_paren_owner_continuation_kind(trimmed) {
        return Some(kind);
    }

    if line_is_exact_bare_split_for_update_header(trimmed) {
        return Some(FormatInlineCommentHeaderContinuationKind::SameDepth);
    }

    if !line_is_exact_bare_structural_keyword_line(trimmed) {
        return None;
    }

    if line_has_exact_identifier_sequence(trimmed, &["WINDOW"]) {
        return Some(FormatInlineCommentHeaderContinuationKind::OneDeeperThanCurrentLine);
    }

    if let Some(kind) = exact_bare_join_clause_continuation_kind(trimmed) {
        return Some(kind);
    }

    if line_has_exact_identifier_sequence(trimmed, &["SELECT"]) {
        return Some(FormatInlineCommentHeaderContinuationKind::OneDeeperThanCurrentLine);
    }

    if line_is_format_same_depth_deferred_wrapper_owner(trimmed) {
        return Some(FormatInlineCommentHeaderContinuationKind::SameDepth);
    }

    if format_indented_paren_owner_header_kind(trimmed)
        .is_some_and(|kind| kind != FormatIndentedParenOwnerKind::ModelSubclause)
        || format_indented_paren_pending_header_kind(trimmed).is_some()
        || format_query_owner_pending_header_kind(trimmed).is_some()
        || format_plsql_child_query_owner_pending_header_kind(trimmed).is_some()
    {
        return Some(FormatInlineCommentHeaderContinuationKind::SameDepth);
    }

    format_leading_header_continuation_kind(trimmed)
}

pub(crate) fn format_bare_structural_header_continuation_kind(
    line: &str,
) -> Option<FormatInlineCommentHeaderContinuationKind> {
    format_bare_structural_header_continuation_kind_for_structural_tail(
        auto_format_structural_tail(line),
    )
}

pub(crate) fn line_is_exact_bare_owner_or_pending_header_for_structural_tail(
    trimmed: &str,
) -> bool {
    if !line_is_exact_bare_structural_keyword_line(trimmed) {
        return false;
    }

    matches!(
        format_query_owner_header_kind(trimmed),
        Some(FormatQueryOwnerKind::FromItem | FormatQueryOwnerKind::Condition)
    ) || format_query_owner_pending_header_kind(trimmed).is_some()
        || line_has_exact_identifier_sequence(trimmed, &["WINDOW"])
        || format_indented_paren_owner_header_kind(trimmed)
            .is_some_and(|kind| kind != FormatIndentedParenOwnerKind::ModelSubclause)
        || format_indented_paren_pending_header_kind(trimmed).is_some()
        || format_plsql_child_query_owner_pending_header_kind(trimmed).is_some()
        || line_is_format_same_depth_deferred_wrapper_owner(trimmed)
}

#[cfg_attr(not(test), allow(dead_code))]
pub(crate) fn line_is_exact_bare_owner_or_pending_header(line: &str) -> bool {
    line_is_exact_bare_owner_or_pending_header_for_structural_tail(auto_format_structural_tail(
        line,
    ))
}

pub(crate) fn format_inline_comment_continuation_kind(
    line: &str,
) -> Option<FormatInlineCommentHeaderContinuationKind> {
    format_inline_comment_continuation_kind_for_structural_tail(auto_format_structural_tail(line))
}

pub(crate) fn format_inline_comment_continuation_kind_for_structural_tail(
    trimmed: &str,
) -> Option<FormatInlineCommentHeaderContinuationKind> {
    if line_is_exact_bare_owner_or_pending_header_for_structural_tail(trimmed) {
        if let Some(kind) =
            format_bare_structural_header_continuation_kind_for_structural_tail(trimmed)
        {
            return Some(kind);
        }
    }

    if format_trailing_continuation_operator_kind(trimmed).is_some() {
        return format_structural_header_continuation_kind_for_structural_tail(trimmed).or(Some(
            FormatInlineCommentHeaderContinuationKind::OneDeeperThanCurrentLine,
        ));
    }

    format_inline_comment_structural_header_continuation_kind_for_structural_tail(trimmed)
}

pub(crate) fn format_inline_comment_structural_header_continuation_kind(
    line: &str,
) -> Option<FormatInlineCommentHeaderContinuationKind> {
    format_inline_comment_structural_header_continuation_kind_for_structural_tail(
        auto_format_structural_tail(line),
    )
}

pub(crate) fn format_inline_comment_structural_header_continuation_kind_for_structural_tail(
    trimmed: &str,
) -> Option<FormatInlineCommentHeaderContinuationKind> {
    if line_is_exact_bare_owner_or_pending_header_for_structural_tail(trimmed) {
        if let Some(kind) =
            format_bare_structural_header_continuation_kind_for_structural_tail(trimmed)
        {
            return Some(kind);
        }
    }

    if line_is_exact_bare_structural_keyword_line(trimmed) {
        if let Some(kind) = exact_bare_indented_paren_owner_inline_comment_collision_kind(trimmed) {
            return Some(kind);
        }
    }

    format_structural_header_continuation_kind_for_structural_tail(trimmed)
        .or_else(|| format_bare_structural_header_continuation_kind_for_structural_tail(trimmed))
}

fn exact_bare_indented_paren_owner_continuation_kind(
    trimmed: &str,
) -> Option<FormatInlineCommentHeaderContinuationKind> {
    FORMAT_ALL_INDENTED_PAREN_OWNER_KINDS
        .iter()
        .find_map(|owner_kind| owner_kind.exact_bare_split_body_header_continuation_kind(trimmed))
}

fn exact_bare_indented_paren_owner_inline_comment_collision_kind(
    trimmed: &str,
) -> Option<FormatInlineCommentHeaderContinuationKind> {
    FORMAT_ALL_INDENTED_PAREN_OWNER_KINDS
        .iter()
        .find_map(|owner_kind| {
            owner_kind.exact_bare_split_body_header_inline_comment_collision_kind(trimmed)
        })
}

/// Returns true when a token starts a flashback/temporal boundary expression.
pub(crate) fn is_format_temporal_boundary_keyword(word: &str) -> bool {
    matches!(word.to_ascii_uppercase().as_str(), "TIMESTAMP" | "SCN")
}

/// Returns true when a token is a leading clause keyword for table-function columns.
pub(crate) fn is_table_function_item_leading_keyword(word: &str) -> bool {
    matches_keyword(word, TABLE_FUNCTION_ITEM_LEADING_KEYWORDS)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    #[test]
    fn oracle_sql_keyword_lookup_uses_uppercase_tokens() {
        assert!(is_oracle_sql_keyword("SELECT"));
        assert!(!is_oracle_sql_keyword("select"));
    }

    #[test]
    fn mysql_keyword_lookup_covers_mariadb_diagnostics_and_generated_column_tokens() {
        for keyword in [
            "ALWAYS",
            "CLOSE",
            "CURRENT",
            "CURRENT_ROLE",
            "DEALLOCATE",
            "DIAGNOSTICS",
            "DO",
            "ERRORS",
            "FOUND",
            "GENERATED",
            "MESSAGE_TEXT",
            "MYSQL_ERRNO",
            "OLD",
            "PRECEDING",
            "SIGNED",
            "RETURNED_SQLSTATE",
            "STORED",
            "UNBOUNDED",
            "UNTIL",
            "VIRTUAL",
            "WINDOW",
        ] {
            assert!(
                is_mysql_sql_keyword(keyword),
                "missing MySQL/MariaDB keyword: {keyword}"
            );
        }
        assert!(
            !is_mysql_sql_keyword("INNODB"),
            "storage engine names should not be promoted to keywords"
        );
    }

    #[test]
    fn mysql_keyword_lookup_covers_admin_option_tokens_used_by_completion() {
        for keyword in MYSQL_ADMIN_OPTION_KEYWORDS {
            assert!(
                is_mysql_sql_keyword(keyword),
                "missing MySQL admin/option keyword: {keyword}"
            );
        }
    }

    #[test]
    fn mysql_admin_option_keywords_are_not_value_expression_keywords() {
        for keyword in MYSQL_ADMIN_OPTION_KEYWORDS {
            assert!(
                is_non_expression_keyword_upper(keyword),
                "MySQL admin option should be filtered from value-expression completion: {keyword}"
            );
        }
    }

    #[test]
    fn mysql_command_and_ddl_fragment_keywords_highlight_but_do_not_complete_as_values() {
        for keyword in MYSQL_NON_EXPRESSION_COMMAND_KEYWORDS
            .iter()
            .chain(MYSQL_DDL_FRAGMENT_KEYWORDS.iter())
        {
            assert!(
                is_mysql_sql_keyword(keyword),
                "missing MySQL command/DDL fragment keyword: {keyword}"
            );
            assert!(
                is_non_expression_keyword_upper(keyword),
                "MySQL command/DDL fragment should be filtered from value-expression completion: {keyword}"
            );
        }
    }

    #[test]
    fn mysql_statement_modifier_keywords_are_not_value_expression_keywords() {
        for keyword in MYSQL_NON_EXPRESSION_MODIFIER_KEYWORDS {
            assert!(
                is_mysql_sql_keyword(keyword),
                "missing MySQL statement modifier keyword: {keyword}"
            );
            assert!(
                is_non_expression_keyword_upper(keyword),
                "MySQL statement modifier should be filtered from value-expression completion: {keyword}"
            );
        }
    }

    #[test]
    fn mysql_dynamic_privilege_keywords_highlight_but_do_not_complete_as_values() {
        for keyword in MYSQL_DYNAMIC_PRIVILEGE_KEYWORDS {
            assert!(
                is_mysql_sql_keyword(keyword),
                "missing MySQL dynamic privilege keyword: {keyword}"
            );
            assert!(
                is_non_expression_keyword_upper(keyword),
                "MySQL dynamic privilege should be filtered from value-expression completion: {keyword}"
            );
        }
    }

    #[test]
    fn mysql_account_option_keywords_highlight_but_do_not_complete_as_values() {
        for keyword in MYSQL_ACCOUNT_OPTION_KEYWORDS {
            assert!(
                is_mysql_sql_keyword(keyword),
                "missing MySQL account option keyword: {keyword}"
            );
            assert!(
                is_non_expression_keyword_upper(keyword),
                "MySQL account option should be filtered from value-expression completion: {keyword}"
            );
        }
    }

    #[test]
    fn mysql_sql_keywords_do_not_contain_duplicates() {
        let mut seen = HashSet::new();
        for keyword in MYSQL_SQL_KEYWORDS {
            assert!(seen.insert(*keyword), "duplicate MySQL keyword: {keyword}");
        }
    }

    #[test]
    fn mysql_sql_keywords_stay_sorted() {
        for pair in MYSQL_SQL_KEYWORDS.windows(2) {
            assert!(
                pair[0] <= pair[1],
                "MYSQL_SQL_KEYWORDS not sorted: {:?} > {:?}",
                pair[0],
                pair[1]
            );
        }
    }

    #[test]
    fn statement_head_keyword_includes_ddl_and_tcl_roots() {
        assert!(is_statement_head_keyword("CREATE"));
        assert!(is_statement_head_keyword("ALTER"));
        assert!(is_statement_head_keyword("DROP"));
        assert!(is_statement_head_keyword("TRUNCATE"));
        assert!(is_statement_head_keyword("RENAME"));
        assert!(is_statement_head_keyword("GRANT"));
        assert!(is_statement_head_keyword("REVOKE"));
        assert!(is_statement_head_keyword("COMMIT"));
        assert!(is_statement_head_keyword("ROLLBACK"));
    }

    #[test]
    fn multiline_string_prefix_lengths_ignore_same_line_literals() {
        let sql = "SELECT JSON_ARRAYAGG (JSON_OBJECT (KEY 'skill' VALUE s.skill_name, KEY 'level' VALUE s.proficiency) ORDER BY s.proficiency DESC\nRETURNING CLOB)";

        assert_eq!(
            multiline_string_continuation_prefix_lengths(sql, sql.lines().count()),
            vec![None, None],
            "same-line literals inside a code line must not hide the structural head of that line"
        );
    }

    #[test]
    fn multiline_string_prefix_lengths_keep_only_the_tail_after_a_multiline_literal_closes() {
        let sql = "XMLQUERY ('for $i in /employees/employee\n</result>' PASSING x.xml_data RETURNING CONTENT)";
        let prefixes = multiline_string_continuation_prefix_lengths(sql, sql.lines().count());

        assert_eq!(prefixes[0], None);
        assert_eq!(
            prefixes[1],
            Some("</result>'".len()),
            "the closing line of a multiline literal should expose only the structural SQL tail"
        );
    }

    #[test]
    fn multiline_string_prefix_lengths_keep_tail_after_multiline_backtick_identifier_closes() {
        let sql = "SELECT `dept\n)name` AS quoted_name\nFROM dual";
        let prefixes = multiline_string_continuation_prefix_lengths(sql, sql.lines().count());

        assert_eq!(prefixes[0], None);
        assert_eq!(
            prefixes[1],
            Some(")name`".len()),
            "multiline backtick identifier continuation must strip payload prefix and keep only structural tail"
        );
        assert_eq!(prefixes[2], None);
    }

    #[test]
    fn multiline_string_prefix_lengths_keep_tail_after_multiline_dollar_quote_closes() {
        let sql = "SELECT $fmt$first line\n)second line$fmt$ AS txt\nFROM dual";
        let prefixes = multiline_string_continuation_prefix_lengths(sql, sql.lines().count());

        assert_eq!(prefixes[0], None);
        assert_eq!(
            prefixes[1],
            Some(")second line$fmt$".len()),
            "multiline dollar-quote continuation must strip payload prefix and expose the structural tail"
        );
        assert_eq!(prefixes[2], None);
    }

    #[test]
    fn multiline_string_prefix_lengths_treat_identifier_suffix_q_quote_like_text_as_single_quote() {
        let sql = "SELECT colq'xyz\n)payload' ) AS txt\nFROM dual";
        let prefixes = multiline_string_continuation_prefix_lengths(sql, sql.lines().count());

        assert_eq!(prefixes[0], None);
        assert_eq!(
            prefixes[1],
            Some(")payload'".len()),
            "identifier suffix `q'` must not be promoted to q-quote prefix; multiline single-quote tail should keep only the literal-closing prefix"
        );
        assert_eq!(prefixes[2], None);
    }

    #[test]
    fn with_non_plsql_clause_keyword_detects_common_oracle_clauses() {
        for keyword in [
            "READ",
            "CHECK",
            "NO",
            "DATA",
            "TIES",
            "ROWID",
            "OBJECT",
            "PRIMARY",
            "REDUCED",
            "CASCADED",
            "CONSTRAINT",
            "GRANT",
            "ADMIN",
            "DELEGATE",
            "HIERARCHY",
        ] {
            assert!(
                is_with_non_plsql_clause_keyword(keyword),
                "{keyword} should be recognized as a non-PL/SQL WITH clause keyword"
            );
        }
        assert!(!is_with_non_plsql_clause_keyword("FUNCTION"));
        assert!(!is_with_non_plsql_clause_keyword("SELECT"));
    }

    #[test]
    fn statement_head_keywords_do_not_contain_duplicates() {
        let mut seen = HashSet::new();
        for keyword in STATEMENT_HEAD_KEYWORDS {
            assert!(
                seen.insert(*keyword),
                "duplicate statement head keyword: {keyword}"
            );
        }
    }

    #[test]
    fn mysql_statement_head_keywords_do_not_contain_duplicates() {
        let mut seen = HashSet::new();
        for keyword in MYSQL_STATEMENT_HEAD_KEYWORDS {
            assert!(
                seen.insert(*keyword),
                "duplicate MySQL statement head keyword: {keyword}"
            );
        }
    }

    #[test]
    fn mysql_statement_head_keywords_stay_in_mysql_keyword_pool() {
        for keyword in MYSQL_STATEMENT_HEAD_KEYWORDS {
            assert!(
                is_mysql_sql_keyword(keyword),
                "MySQL statement head should be highlighted as a MySQL keyword: {keyword}"
            );
        }
    }

    #[test]
    fn formatter_keyword_groups_stay_in_shared_keyword_pool() {
        assert!(FORMAT_CLAUSE_KEYWORDS
            .iter()
            .all(|keyword| is_oracle_sql_keyword(keyword)));
        assert!(FORMAT_CREATE_SUFFIX_BREAK_KEYWORDS
            .iter()
            .all(|keyword| is_oracle_sql_keyword(keyword)));
        assert!(FORMAT_LAYOUT_CLAUSE_START_KEYWORDS
            .iter()
            .all(|keyword| is_oracle_sql_keyword(keyword)));
        assert!(FORMAT_JOIN_MODIFIER_KEYWORDS
            .iter()
            .all(|keyword| is_oracle_sql_keyword(keyword)));
        assert!(FORMAT_CONDITION_KEYWORDS
            .iter()
            .all(|keyword| is_oracle_sql_keyword(keyword)));
        assert!(FORMAT_EXPRESSION_CONTINUATION_KEYWORDS
            .iter()
            .all(|keyword| is_oracle_sql_keyword(keyword)));
        assert!(FORMAT_BLOCK_START_KEYWORDS
            .iter()
            .all(|keyword| is_oracle_sql_keyword(keyword)));
        assert!(FORMAT_BLOCK_END_QUALIFIER_KEYWORDS
            .iter()
            .all(|keyword| is_oracle_sql_keyword(keyword)));
        assert!(FORMAT_COLUMN_CONSTRAINT_KEYWORDS
            .iter()
            .all(|keyword| is_oracle_sql_keyword(keyword)));
    }

    #[test]
    fn starts_with_format_layout_clause_tracks_extended_clause_heads() {
        assert!(starts_with_format_layout_clause("WINDOW w AS ("));
        assert!(starts_with_format_layout_clause(
            "QUALIFY ROW_NUMBER () = 1"
        ));
        assert!(starts_with_format_layout_clause("OFFSET 10 ROWS"));
        assert!(starts_with_format_layout_clause("FETCH FIRST 5 ROWS ONLY"));
        assert!(starts_with_format_layout_clause("LIMIT 50"));
        assert!(starts_with_format_layout_clause(
            "SEARCH DEPTH FIRST BY id SET ord"
        ));
        assert!(starts_with_format_layout_clause(
            "CYCLE id SET is_cycle TO 'Y' DEFAULT 'N'"
        ));
    }

    #[test]
    fn non_subquery_paren_suppressed_clause_start_covers_function_internal_clauses() {
        assert!(is_non_subquery_paren_suppressed_clause_start(
            "RETURNING VARCHAR2 (30))"
        ));
        assert!(is_non_subquery_paren_suppressed_clause_start(
            "WITH WRAPPER"
        ));
        assert!(is_non_subquery_paren_suppressed_clause_start(
            "SET '$.status' = 'DONE'"
        ));
        assert!(is_non_subquery_paren_suppressed_clause_start(
            "INSERT '$.audit.user' = USER"
        ));
        assert!(!is_non_subquery_paren_suppressed_clause_start(
            "FROM hire_date"
        ));
        assert!(!is_non_subquery_paren_suppressed_clause_start(
            "WHERE emp_id = 1"
        ));
    }

    #[test]
    fn non_subquery_paren_suppressed_clause_continuation_covers_json_value_on_error_shapes() {
        assert!(is_non_subquery_paren_suppressed_clause_continuation(
            "ON ERROR NULL"
        ));
        assert!(is_non_subquery_paren_suppressed_clause_continuation(
            "ON EMPTY NULL"
        ));
        assert!(is_non_subquery_paren_suppressed_clause_continuation(
            "NULL ON ERROR"
        ));
        assert!(is_non_subquery_paren_suppressed_clause_continuation(
            "DEFAULT ON EMPTY"
        ));
        assert!(is_non_subquery_paren_suppressed_clause_continuation(
            "TRUE ON ERROR"
        ));
        assert!(is_non_subquery_paren_suppressed_clause_continuation(
            "FALSE ON EMPTY"
        ));
        assert!(is_non_subquery_paren_suppressed_clause_continuation(
            "UNKNOWN ON ERROR"
        ));
        assert!(is_non_subquery_paren_suppressed_clause_continuation(
            "EMPTY ARRAY ON ERROR"
        ));
        assert!(is_non_subquery_paren_suppressed_clause_continuation(
            "EMPTY OBJECT ON EMPTY"
        ));
        assert!(!is_non_subquery_paren_suppressed_clause_continuation(
            "ON deptno = e.deptno"
        ));
        assert!(!is_non_subquery_paren_suppressed_clause_continuation(
            "USING (deptno)"
        ));
    }

    #[test]
    fn from_consuming_function_helper_tracks_shared_function_families() {
        for function_name in [
            "EXTRACT",
            "TRIM",
            "SUBSTRING",
            "OVERLAY",
            "POSITION",
            "NORMALIZE",
            "TRIM_ARRAY",
        ] {
            assert!(
                is_from_consuming_function(function_name),
                "{function_name} should keep FROM inside function-local syntax"
            );
        }
        assert!(!is_from_consuming_function("XMLCAST"));
        assert!(!is_from_consuming_function("JSON_VALUE"));
    }

    #[test]
    fn format_set_operator_helper_covers_except_and_other_set_operators() {
        assert!(is_format_set_operator_keyword("except"));
        assert_eq!(
            FormatSetOperatorKind::from_clause_start("EXCEPT DISTINCT"),
            Some(FormatSetOperatorKind::Except)
        );
        assert_eq!(
            FormatSetOperatorKind::from_clause_start("UNION ALL"),
            Some(FormatSetOperatorKind::Union)
        );
        assert_eq!(FormatSetOperatorKind::from_clause_start("ORDER BY"), None);
        assert!(starts_with_format_set_operator("INTERSECT"));
        assert!(!starts_with_format_set_operator("WHERE col IN"));
    }

    #[test]
    fn structural_prefix_helpers_ignore_inline_block_comment_gaps() {
        assert!(starts_with_format_join_clause(
            "LEFT/* gap */OUTER/* gap */JOIN emp e"
        ));
        assert!(starts_with_format_for_update_clause(
            "FOR/* gap */UPDATE SKIP LOCKED"
        ));
        assert!(starts_with_format_for_update_split_header("FOR/* gap */"));
        assert!(!starts_with_format_for_update_clause("FOR ORDINALITY"));
        assert!(!starts_with_format_for_update_split_header(
            "FOR /* ord */ ORDINALITY"
        ));
        assert!(starts_with_format_merge_branch_header(
            "WHEN/* gap */NOT/* gap */MATCHED/* gap */THEN"
        ));
        assert!(is_format_join_condition_clause("USING/* gap */(deptno)"));
    }

    #[test]
    fn line_starts_query_head_recognizes_select_and_with_heads() {
        assert!(line_starts_query_head("SELECT empno"));
        assert!(line_starts_query_head("WITH bonus_cte AS"));
        assert!(line_starts_query_head("/* gap */SELECT empno"));
        assert!(!line_starts_query_head("ORDER BY empno"));
    }

    #[test]
    fn significant_paren_profile_tracks_event_order_and_leading_closes() {
        let profile = significant_paren_profile(") ) PARTITION BY (expr + (1))");

        assert_eq!(profile.leading_close_count, 2);
        assert_eq!(
            profile.events,
            vec![
                SignificantParenEvent::Close,
                SignificantParenEvent::Close,
                SignificantParenEvent::Open,
                SignificantParenEvent::Open,
                SignificantParenEvent::Close,
                SignificantParenEvent::Close,
            ]
        );
    }

    #[test]
    fn significant_paren_profile_ignores_comments_and_q_quotes() {
        let profile =
            significant_paren_profile("/* ) */ ) q'[ignored ( )]' /* ( */ (col) -- trailing )");

        assert_eq!(profile.leading_close_count, 1);
        assert_eq!(
            profile.events,
            vec![
                SignificantParenEvent::Close,
                SignificantParenEvent::Open,
                SignificantParenEvent::Close,
            ]
        );
    }

    #[test]
    fn significant_paren_profile_treats_leading_block_comment_close_as_comment_tail() {
        let profile = significant_paren_profile("*/ ) + (next)");

        assert_eq!(profile.leading_close_count, 1);
        assert_eq!(
            profile.events,
            vec![
                SignificantParenEvent::Close,
                SignificantParenEvent::Open,
                SignificantParenEvent::Close,
            ]
        );
    }

    #[test]
    fn significant_paren_profile_ignores_mysql_backtick_quoted_identifier_parens() {
        let profile = significant_paren_profile("`raw(` + `tail)` + (expr)");

        assert_eq!(profile.leading_close_count, 0);
        assert_eq!(
            profile.events,
            vec![SignificantParenEvent::Open, SignificantParenEvent::Close]
        );
    }

    #[test]
    fn significant_paren_profile_ignores_dollar_quoted_literal_parens() {
        let profile = significant_paren_profile("$q$raw)($q$ + $fmt$tail($fmt$ + (expr)");

        assert_eq!(profile.leading_close_count, 0);
        assert_eq!(
            profile.events,
            vec![SignificantParenEvent::Open, SignificantParenEvent::Close]
        );
    }

    #[test]
    fn significant_paren_profile_treats_identifier_suffix_q_quote_like_text_as_single_quote() {
        let profile = significant_paren_profile("colq'xyz' ) + (");

        assert_eq!(profile.leading_close_count, 0);
        assert_eq!(
            profile.events,
            vec![SignificantParenEvent::Close, SignificantParenEvent::Open],
            "identifier suffix `q'` must not hide same-line close/open structural events behind an accidental q-quote state"
        );
    }

    #[test]
    fn significant_paren_depth_after_profile_tracks_nested_events() {
        let balanced = significant_paren_profile("(a + (b))");
        assert_eq!(significant_paren_depth_after_profile(0, &balanced), 0);

        let unbalanced = significant_paren_profile("(a + (b)");
        assert_eq!(significant_paren_depth_after_profile(1, &unbalanced), 2);
    }

    #[test]
    fn line_has_leading_significant_close_paren_ignores_comments_and_detects_real_closes() {
        assert!(line_has_leading_significant_close_paren(
            "/* leading note */ ) AND status = 'A'"
        ));
        assert!(line_has_leading_significant_close_paren(
            "*/ ) AND status = 'A'"
        ));
        assert!(line_has_leading_significant_close_paren(" ) "));
        assert!(!line_has_leading_significant_close_paren(
            "/* leading note */ deptno = 10"
        ));
        assert!(!line_has_leading_significant_close_paren("q'[)]'"));
    }

    #[test]
    fn trim_after_leading_close_parens_skips_comments_and_returns_remaining_code() {
        assert_eq!(
            trim_after_leading_close_parens(" ) /* nested */ EXCLUDE CURRENT ROW"),
            "EXCLUDE CURRENT ROW"
        );
        assert_eq!(trim_after_leading_close_parens(") -- comment only"), "");
        assert_eq!(trim_after_leading_close_parens(") # comment only"), "");
        assert_eq!(
            trim_after_leading_close_parens("PARTITION BY deptno"),
            "PARTITION BY deptno"
        );
    }

    #[test]
    fn trim_leading_sql_comments_returns_meaningful_structural_tail() {
        assert_eq!(
            trim_leading_sql_comments("/* note */ ORDER BY empno"),
            "ORDER BY empno"
        );
        assert_eq!(
            trim_leading_sql_comments("/* note */ /* next */ ON e.deptno = d.deptno"),
            "ON e.deptno = d.deptno"
        );
        assert_eq!(trim_leading_sql_comments("/* note only */"), "");
        assert_eq!(trim_leading_sql_comments("-- line comment"), "");
    }

    #[test]
    fn bare_parenthesized_condition_header_helper_tracks_comment_glued_control_headers() {
        assert!(line_is_bare_parenthesized_condition_header(
            "/* lead */ IF /* gap */ ("
        ));
        assert!(line_is_bare_parenthesized_condition_header(
            "ELSIF /* gap */ ("
        ));
        assert!(line_is_bare_parenthesized_condition_header(
            "WHEN /* gap */ ( -- trailing"
        ));

        assert!(!line_is_bare_parenthesized_condition_header(
            "IF ready_flag = 'Y' THEN"
        ));
        assert!(!line_is_bare_parenthesized_condition_header(
            "WHILE total_count > 0 LOOP"
        ));
        assert!(!line_is_bare_parenthesized_condition_header(
            "IF /* gap */ ready_flag"
        ));
    }

    #[test]
    fn line_continues_expression_after_leading_close_distinguishes_mixed_and_pure_close_lines() {
        assert!(line_continues_expression_after_leading_close(
            ") AND EXISTS ("
        ));
        assert!(line_continues_expression_after_leading_close(
            ") /* gap */ IS NULL"
        ));
        assert!(line_continues_expression_after_leading_close("), 0"));
        assert!(line_continues_expression_after_leading_close(
            ") MEMBER OF deptno_nt"
        ));
        assert!(line_continues_expression_after_leading_close(
            ") SUBMULTISET OF num_nt"
        ));
        assert!(line_continues_expression_after_leading_close(
            ") LIKEC 'A%'"
        ));
        assert!(line_continues_expression_after_leading_close(
            ") LIKE2 'A%'"
        ));
        assert!(line_continues_expression_after_leading_close(
            ") LIKE4 'A%'"
        ));
        assert!(line_continues_expression_after_leading_close(") := 1"));
        assert!(line_continues_expression_after_leading_close(
            ") => p_value"
        ));

        assert!(!line_continues_expression_after_leading_close(")"));
        assert!(!line_continues_expression_after_leading_close("),"));
        assert!(!line_continues_expression_after_leading_close(
            ") JOIN dept d"
        ));
    }

    #[test]
    fn line_has_mixed_leading_close_continuation_covers_clause_and_query_head_reclassification() {
        assert!(line_has_mixed_leading_close_continuation(") AND EXISTS ("));
        assert!(line_has_mixed_leading_close_continuation(
            ") MEMBER OF deptno_nt"
        ));
        assert!(line_has_mixed_leading_close_continuation(
            ") SUBMULTISET OF num_nt"
        ));
        assert!(line_has_mixed_leading_close_continuation(") LIKE4 'A%'"));
        assert!(line_has_mixed_leading_close_continuation(
            ") ORDER BY empno"
        ));
        assert!(line_has_mixed_leading_close_continuation(
            ") UPDATE demo SET flag = 'Y'"
        ));
        assert!(line_has_mixed_leading_close_continuation(
            ") FOR UPDATE NOWAIT"
        ));
        assert!(line_has_mixed_leading_close_continuation(") TRUE ON ERROR"));
        assert!(line_has_mixed_leading_close_continuation(
            ") FALSE ON EMPTY"
        ));
        assert!(line_has_mixed_leading_close_continuation(
            ") EMPTY ARRAY ON ERROR"
        ));

        assert!(!line_has_mixed_leading_close_continuation(")"));
        assert!(!line_has_mixed_leading_close_continuation("),"));
        assert!(!line_has_mixed_leading_close_continuation(
            ") # comment only"
        ));
        assert!(!line_has_mixed_leading_close_continuation(") bonus_view"));
        assert!(!line_has_mixed_leading_close_continuation(") window,"));
        assert!(!line_has_mixed_leading_close_continuation(") THEN"));
    }

    #[test]
    fn auto_format_structural_tail_simple_alias_helper_accepts_keyword_like_aliases_after_leading_close(
    ) {
        assert!(auto_format_structural_tail_is_alias_fragment(") window"));
        assert!(auto_format_structural_tail_is_alias_fragment(
            ") /* gap */ AS `window` -- keep"
        ));
        assert!(auto_format_structural_tail_is_simple_alias(") window,"));
        assert!(auto_format_structural_tail_is_simple_alias(
            ") /* gap */ AS `window`,"
        ));
        assert!(!auto_format_structural_tail_is_alias_fragment(
            ") WINDOW w AS ("
        ));
        assert!(!auto_format_structural_tail_is_alias_fragment(
            ") ORDER BY empno"
        ));
        assert!(!auto_format_structural_tail_is_simple_alias(") window"));
        assert!(!auto_format_structural_tail_is_simple_alias(
            ") WINDOW w AS ("
        ));
        assert!(!auto_format_structural_tail_is_simple_alias(
            ") ORDER BY empno"
        ));
    }

    #[test]
    fn format_comment_continuation_keywords_cover_clause_and_condition_heads() {
        assert!(is_format_comment_continuation_keyword("SELECT"));
        assert!(is_format_comment_continuation_keyword("SET"));
        assert!(is_format_comment_continuation_keyword("WINDOW"));
        assert!(is_format_comment_continuation_keyword("QUALIFY"));
        assert!(is_format_comment_continuation_keyword("FETCH"));
        assert!(is_format_comment_continuation_keyword("LIMIT"));
        assert!(is_format_comment_continuation_keyword("ON"));
        assert!(is_format_comment_continuation_keyword("USING"));
        assert!(is_format_comment_continuation_keyword("LEFT"));
        assert!(is_format_comment_continuation_keyword("AND"));
        assert!(is_format_comment_continuation_keyword("JOIN"));
        assert!(is_format_comment_continuation_keyword("MEMBER"));
        assert!(is_format_comment_continuation_keyword("SUBMULTISET"));
        assert!(is_format_comment_continuation_keyword("LIKEC"));
        assert!(is_format_comment_continuation_keyword("LIKE2"));
        assert!(is_format_comment_continuation_keyword("LIKE4"));
        assert!(is_format_comment_continuation_keyword("ESCAPE"));
        assert!(!is_format_comment_continuation_keyword("DUAL"));
    }

    #[test]
    fn trailing_format_continuation_operator_helper_tracks_shared_keyword_and_symbol_taxonomy() {
        assert!(line_has_trailing_format_continuation_operator(
            "WHERE e.empno IS"
        ));
        assert!(line_has_trailing_format_continuation_operator(
            "WHERE e.ename LIKE"
        ));
        assert!(line_has_trailing_format_continuation_operator(
            "WHERE e.sal BETWEEN"
        ));
        assert!(line_has_trailing_format_continuation_operator(
            "WHERE e.member_col MEMBER"
        ));
        assert!(line_has_trailing_format_continuation_operator(
            "WHERE e.num_nt SUBMULTISET"
        ));
        assert!(line_has_trailing_format_continuation_operator(
            "WHERE e.ename LIKEC"
        ));
        assert!(line_has_trailing_format_continuation_operator(
            "WHERE e.ename LIKE2"
        ));
        assert!(line_has_trailing_format_continuation_operator(
            "WHERE e.ename LIKE4"
        ));
        assert!(line_has_trailing_format_continuation_operator(
            "WHERE e.ename LIKE 'A%' ESCAPE"
        ));
        assert!(line_has_trailing_format_continuation_operator(
            "WHERE e.payload IS OF"
        ));
        assert!(line_has_trailing_format_continuation_operator(
            "WHERE e.member_col MEMBER OF"
        ));
        assert!(line_has_trailing_format_continuation_operator(
            "WHERE e.num_nt SUBMULTISET OF"
        ));
        assert!(line_has_trailing_format_continuation_operator(
            "WHERE e.empno ="
        ));
        assert!(line_has_trailing_format_continuation_operator("v_total :="));
        assert!(line_has_trailing_format_continuation_operator(
            "pkg_lock.request =>"
        ));
        assert!(line_has_trailing_format_continuation_operator(
            "e.qty /* gap */ <="
        ));

        assert!(!line_has_trailing_format_continuation_operator("SELECT *"));
        assert!(!line_has_trailing_format_continuation_operator(
            "FOR UPDATE NOWAIT"
        ));
        assert!(!line_has_trailing_format_continuation_operator(
            "WHERE e.empno"
        ));
    }

    #[test]
    fn trailing_continuation_operator_token_pair_helper_preserves_projection_and_division_exceptions(
    ) {
        assert_eq!(
            format_trailing_continuation_operator_kind_from_token_pair(
                Some(FormatTrailingMeaningfulToken::Word("SELECT")),
                FormatTrailingMeaningfulToken::Symbol("*"),
            ),
            None
        );
        assert_eq!(
            format_trailing_continuation_operator_kind_from_token_pair(
                Some(FormatTrailingMeaningfulToken::Word("qty")),
                FormatTrailingMeaningfulToken::Symbol("/"),
            ),
            Some(FormatTrailingContinuationOperatorKind::Symbol)
        );
        assert_eq!(
            format_trailing_continuation_operator_kind_from_token_pair(
                None,
                FormatTrailingMeaningfulToken::Symbol("/"),
            ),
            None
        );
        assert_eq!(
            format_trailing_continuation_operator_kind_from_token_pair(
                Some(FormatTrailingMeaningfulToken::Word("IS")),
                FormatTrailingMeaningfulToken::Word("OF"),
            ),
            Some(FormatTrailingContinuationOperatorKind::Keyword)
        );
        assert_eq!(
            format_trailing_continuation_operator_kind_from_token_pair(
                Some(FormatTrailingMeaningfulToken::Word("MEMBER")),
                FormatTrailingMeaningfulToken::Word("OF"),
            ),
            Some(FormatTrailingContinuationOperatorKind::Keyword)
        );
        assert_eq!(
            format_trailing_continuation_operator_kind_from_token_pair(
                Some(FormatTrailingMeaningfulToken::Word("SUBMULTISET")),
                FormatTrailingMeaningfulToken::Word("OF"),
            ),
            Some(FormatTrailingContinuationOperatorKind::Keyword)
        );
        assert_eq!(
            format_trailing_continuation_operator_kind_from_token_pair(
                None,
                FormatTrailingMeaningfulToken::Word("LIKE4"),
            ),
            Some(FormatTrailingContinuationOperatorKind::Keyword)
        );
        assert_eq!(
            format_trailing_continuation_operator_kind_from_token_pair(
                None,
                FormatTrailingMeaningfulToken::Word("ESCAPE"),
            ),
            Some(FormatTrailingContinuationOperatorKind::Keyword)
        );
    }

    #[test]
    fn format_temporal_boundary_keywords_cover_timestamp_and_scn() {
        assert!(is_format_temporal_boundary_keyword("timestamp"));
        assert!(is_format_temporal_boundary_keyword("SCN"));
        assert!(!is_format_temporal_boundary_keyword("DATE"));
    }

    #[test]
    fn trailing_inline_comment_helpers_ignore_q_quotes_and_preserve_structural_tail() {
        assert_eq!(
            trailing_inline_comment_prefix("q'[-- kept literal]' -- real comment"),
            Some("q'[-- kept literal]' ")
        );
        assert_eq!(
            trailing_inline_comment_prefix("SELECT 1 # real comment"),
            Some("SELECT 1 ")
        );
        assert_eq!(
            trailing_inline_comment_prefix("FOR /* keep */ UPDATE -- real comment"),
            Some("FOR /* keep */ UPDATE ")
        );
        assert_eq!(
            trailing_inline_comment_prefix("GROUP /* keep */ BY -- real comment"),
            Some("GROUP /* keep */ BY ")
        );
        assert!(line_ends_with_open_paren_before_inline_comment(
            "nq'<, )>' ( -- wrapper"
        ));
        assert!(line_is_standalone_open_paren_before_inline_comment(
            "( /* wrap */"
        ));
        assert!(line_is_standalone_open_paren_before_inline_comment(
            "/* lead */ ( /* wrap */"
        ));
        assert!(line_is_standalone_open_paren_before_inline_comment(
            "( /* wrap */ -- trailing"
        ));
        assert!(line_is_standalone_open_paren_before_inline_comment(
            "/* lead */ ( /* wrap */ -- trailing"
        ));
        assert!(!line_is_standalone_open_paren_before_inline_comment(
            "q'[literal]' ( /* wrap */ -- trailing"
        ));
        assert!(line_ends_with_comma_before_inline_comment(
            "q'[/* literal */]' , -- trailing comma"
        ));
    }

    #[test]
    fn trailing_inline_comment_prefix_does_not_treat_identifier_suffix_q_as_q_quote() {
        assert_eq!(
            trailing_inline_comment_prefix("colq'xyz' ) -- real comment"),
            Some("colq'xyz' ) "),
            "identifier suffix `q'` before a single-quoted literal must not hide trailing inline comment detection"
        );
    }

    #[test]
    fn starts_with_keyword_token_treats_immediate_comments_as_token_boundaries() {
        assert!(starts_with_keyword_token("ELSE/* GAP */", "ELSE"));
        assert!(starts_with_keyword_token("EXCEPTION-- GAP", "EXCEPTION"));
        assert!(starts_with_keyword_token("CASE/* GAP */", "CASE"));
        assert!(!starts_with_keyword_token("ELSEIF", "ELSE"));
    }

    #[test]
    fn quoted_identifier_helpers_support_double_quotes_and_backticks() {
        assert!(is_quoted_identifier(r#""Emp Name""#));
        assert!(is_quoted_identifier("`emp name`"));
        assert!(!is_quoted_identifier("emp_name"));
        assert_eq!(strip_identifier_quotes(r#""Emp""Name""#), "Emp\"Name");
        assert_eq!(strip_identifier_quotes("`emp``name`"), "emp`name");
        assert_eq!(strip_identifier_quotes("  `emp name`  "), "emp name");
    }

    #[test]
    fn quoted_identifier_helpers_support_sql_server_brackets() {
        assert!(is_quoted_identifier("[Emp Name]"));
        assert!(is_quoted_identifier("[Emp]]Name]"));
        assert!(!is_quoted_identifier("[Emp]Name]"));
        assert!(!is_quoted_identifier("[Emp Name"));
        assert_eq!(strip_identifier_quotes("[Emp Name]"), "Emp Name");
        assert_eq!(strip_identifier_quotes("[Emp]]Name]"), "Emp]Name");
        assert_eq!(strip_identifier_quotes("  [Emp.Name]  "), "Emp.Name");
    }

    #[test]
    fn trailing_identifier_helpers_skip_inline_block_comments_and_literals() {
        assert_eq!(
            trailing_identifier_words_before_inline_comment("GROUP /* gap */ BY -- keep", 2),
            vec!["GROUP", "BY"]
        );
        assert_eq!(
            trailing_identifier_words_before_inline_comment(
                "FOR q'[ignored UPDATE]' /* gap */ UPDATE -- keep",
                2
            ),
            vec!["FOR", "UPDATE"]
        );
        assert!(line_ends_with_identifier_sequence_before_inline_comment(
            "IF v_ready = /* gap */ 'Y' THEN",
            &["THEN"]
        ));
        assert!(line_ends_with_semicolon_before_inline_comment(
            "OPEN c_emp; -- keep terminated"
        ));
        assert!(line_ends_with_semicolon_before_inline_comment(
            "OPEN c_emp /* gap */ ; /* keep */"
        ));
        assert!(line_ends_with_semicolon_before_inline_comment(
            "VALUES (1); -- keep terminated"
        ));
        assert!(line_ends_with_semicolon_before_inline_comment(
            "VALUES (1); # keep terminated"
        ));
        assert!(line_ends_with_semicolon_before_inline_comment(
            "            ));"
        ));
        assert!(!line_ends_with_semicolon_before_inline_comment(
            "OPEN c_emp -- keep comment"
        ));
    }

    #[test]
    fn mysql_hash_comment_helpers_treat_hash_as_comment_boundary() {
        assert!(line_has_mysql_hash_comment("SELECT 1 # trailing"));
        assert_eq!(trim_leading_sql_comments("  # comment only"), "");
        assert!(line_has_mysql_hash_comment("SELECT col#suffix"));
        assert!(!line_has_mysql_hash_comment(
            "SELECT `col#suffix` FROM demo"
        ));
        assert!(!line_has_mysql_hash_comment("SELECT 'abc\\'#still string'"));
    }

    #[test]
    fn mysql_delimiter_directive_skips_leading_block_comment_and_inline_comment() {
        assert_eq!(
            parse_mysql_delimiter_directive("/* note */ DELIMITER /* gap */ $$ -- keep"),
            Some("$$".to_string())
        );
        assert_eq!(
            parse_mysql_delimiter_directive("DELIMITER"),
            Some(";".to_string())
        );
        // Dollar-sign delimiter with a trailing hash comment
        assert_eq!(
            parse_mysql_delimiter_directive("DELIMITER $$ # switch to $$"),
            Some("$$".to_string())
        );
        // Reset delimiter with trailing dash-dash comment
        assert_eq!(
            parse_mysql_delimiter_directive("DELIMITER ; -- reset"),
            Some(";".to_string())
        );
    }

    #[test]
    fn mysql_delimiter_directive_accepts_special_chars_as_delimiter_value() {
        // `--` must be accepted as the delimiter value, not mistaken for a comment.
        assert_eq!(
            parse_mysql_delimiter_directive("DELIMITER --"),
            Some("--".to_string())
        );
        // `#` must be accepted as the delimiter value, not mistaken for a comment.
        assert_eq!(
            parse_mysql_delimiter_directive("DELIMITER #"),
            Some("#".to_string())
        );
        // `//` should work as a common alternative delimiter.
        assert_eq!(
            parse_mysql_delimiter_directive("DELIMITER //"),
            Some("//".to_string())
        );
        // Inline content after the token is ignored.
        assert_eq!(
            parse_mysql_delimiter_directive("DELIMITER // rest is ignored"),
            Some("//".to_string())
        );
    }

    #[test]
    fn mysql_executable_comment_helpers_cover_mariadb_variant() {
        let comment = "/*M!100100 SET @feature_flag = 1 */";

        assert!(is_mysql_executable_comment_start(comment.as_bytes(), 0));
        assert!(sql_uses_mysql_compatible_syntax(comment));
    }

    #[test]
    fn mysql_compatibility_helper_respects_preferred_db_type() {
        let mysql_sql = "DROP TABLE IF EXISTS demo";

        assert!(mysql_compatibility_for_sql(
            mysql_sql,
            Some(crate::db::connection::DatabaseType::MySQL)
        ));
        assert!(mysql_compatibility_for_sql(
            mysql_sql,
            Some(crate::db::connection::DatabaseType::MariaDB)
        ));
        assert!(!mysql_compatibility_for_sql(
            mysql_sql,
            Some(crate::db::connection::DatabaseType::Oracle)
        ));
        assert!(sql_uses_mysql_compatible_syntax("SELECT OLD.c <=> NEW.c"));
    }

    #[test]
    fn mysql_compatibility_operator_detection_ignores_comments_and_literals() {
        assert!(sql_uses_mysql_compatible_syntax(
            "SELECT payload->'$.tags', payload->>'$.kind' FROM docs"
        ));
        assert!(sql_uses_mysql_compatible_syntax("SELECT OLD.c <=> NEW.c"));

        assert!(!sql_uses_mysql_compatible_syntax(
            "-- Load stage -> emp via package\nSELECT q'[-> and <=>]' FROM dual"
        ));
        assert!(!sql_uses_mysql_compatible_syntax(
            "SELECT '->', \"<=>\", nq'{->}', uq'!<=>!' FROM dual /* -> */"
        ));
        assert!(!sql_uses_mysql_compatible_syntax(
            "SELECT * FROM GRAPH_TABLE (g MATCH (a)-[e IS links]->(b))"
        ));
        assert!(sql_uses_mysql_compatible_syntax(
            "SELECT payload[index_col]->'$.kind' FROM docs"
        ));
    }

    #[test]
    fn keyword_lookup_uses_concrete_database_type_policy() {
        assert!(is_sql_keyword_for_db(
            "ZEROFILL",
            crate::db::connection::DatabaseType::MySQL
        ));
        assert!(is_sql_keyword_for_db(
            "ZEROFILL",
            crate::db::connection::DatabaseType::MariaDB
        ));
        assert!(!is_sql_keyword_for_db(
            "ZEROFILL",
            crate::db::connection::DatabaseType::Oracle
        ));
    }

    #[test]
    fn format_preferred_db_type_falls_back_to_named_policy() {
        assert_eq!(
            format_preferred_db_type_for_sql("CREATE OR REPLACE VIEW v AS SELECT * FROM t"),
            Some(crate::db::connection::DatabaseType::Oracle)
        );
        assert_eq!(
            format_preferred_db_type_for_sql("SELECT OLD.c <=> NEW.c"),
            None
        );
    }

    #[test]
    fn format_plain_end_helpers_cover_named_and_trigger_timing_suffixes() {
        assert!(is_format_block_end_qualifier_keyword("IF"));
        assert!(!is_format_block_end_qualifier_keyword("BEFORE"));
        assert!(is_format_plain_end_suffix_keyword("BEFORE"));
        assert!(is_format_plain_end_suffix_keyword("INSTEAD"));
        assert!(starts_with_format_end_suffix_terminator(
            "END BEFORE EACH ROW"
        ));
        assert!(starts_with_format_end_suffix_terminator(
            "END AFTER STATEMENT"
        ));
        assert!(starts_with_format_end_suffix_terminator("END INSTEAD OF"));
        assert!(starts_with_format_end_suffix_terminator("END /* gap */ IF"));
        assert!(starts_with_format_plain_end("END trg_pkg;"));
        assert!(!starts_with_format_plain_end("END BEFORE STATEMENT"));
        assert!(!starts_with_format_plain_end("END /* gap */ IF;"));
        assert!(starts_with_format_bare_end("END;"));
        assert!(starts_with_format_named_plain_end("END trg_pkg;"));
        assert!(starts_with_format_named_plain_end("END /* gap */ trg_pkg;"));
        assert!(!starts_with_format_named_plain_end("END AFTER STATEMENT"));
        assert!(!starts_with_format_named_plain_end("END /* gap */ IF;"));
    }

    #[test]
    fn format_source_gap_policy_tracks_both_sides_of_expression_continuations() {
        assert!(format_source_gap_is_canonical_inline(Some("NOT"), "value"));
        assert!(format_source_gap_is_canonical_inline(
            Some("value"),
            "BETWEEN"
        ));
        assert!(format_source_gap_is_canonical_inline(Some("IS"), "NULL"));
        assert!(!format_source_gap_is_canonical_inline(
            Some("SELECT"),
            "value"
        ));
    }

    #[test]
    fn identifier_sequence_helpers_treat_composite_keywords_as_comment_stripped_segments() {
        assert!(line_starts_with_identifier_sequence_before_inline_comment(
            "MATCH /* gap */ RECOGNIZE (",
            &["MATCH_RECOGNIZE"]
        ));
        assert!(line_ends_with_identifier_sequence_before_inline_comment(
            "MATCH /* gap */ RECOGNIZE",
            &["MATCH_RECOGNIZE"]
        ));
        assert!(line_has_exact_identifier_sequence(
            "MATCH /* gap */ RECOGNIZE",
            &["MATCH_RECOGNIZE"]
        ));
        assert_eq!(
            format_indented_paren_owner_header_kind("MATCH /* gap */ RECOGNIZE"),
            Some(FormatIndentedParenOwnerKind::MatchRecognize)
        );
    }

    #[test]
    fn format_inline_comment_header_continuation_kind_tracks_clause_and_subclause_headers() {
        assert_eq!(
            format_inline_comment_header_continuation_kind(None, "WITH"),
            Some(FormatInlineCommentHeaderContinuationKind::SameDepth)
        );
        assert_eq!(
            format_inline_comment_header_continuation_kind(None, "CALL"),
            Some(FormatInlineCommentHeaderContinuationKind::OneDeeperThanCurrentLine)
        );
        assert_eq!(
            format_inline_comment_header_continuation_kind(None, "LIMIT"),
            Some(FormatInlineCommentHeaderContinuationKind::OneDeeperThanCurrentLine)
        );
        assert_eq!(
            format_inline_comment_header_continuation_kind(None, "MEASURES"),
            Some(FormatInlineCommentHeaderContinuationKind::OneDeeperThanCurrentLine)
        );
        assert_eq!(
            format_inline_comment_header_continuation_kind(None, "REFERENCE"),
            Some(FormatInlineCommentHeaderContinuationKind::OneDeeperThanCurrentLine)
        );
        assert_eq!(
            format_inline_comment_header_continuation_kind(None, "SUBSET"),
            Some(FormatInlineCommentHeaderContinuationKind::OneDeeperThanCurrentLine)
        );
        assert_eq!(
            format_inline_comment_header_continuation_kind(None, "COLUMNS"),
            Some(FormatInlineCommentHeaderContinuationKind::OneDeeperThanCurrentLine)
        );
        assert_eq!(
            format_inline_comment_header_continuation_kind(Some("ORDER"), "BY"),
            Some(FormatInlineCommentHeaderContinuationKind::OneDeeperThanCurrentLine)
        );
        assert_eq!(
            format_inline_comment_header_continuation_kind(Some("FOR"), "UPDATE"),
            Some(FormatInlineCommentHeaderContinuationKind::OneDeeperThanQueryBase)
        );
        assert_eq!(
            format_inline_comment_header_continuation_kind(Some("MATCH"), "SKIP"),
            Some(FormatInlineCommentHeaderContinuationKind::OneDeeperThanCurrentLine)
        );
        assert_eq!(
            format_inline_comment_header_continuation_kind(Some("WITHIN"), "GROUP"),
            Some(FormatInlineCommentHeaderContinuationKind::OneDeeperThanCurrentLine)
        );
        assert_eq!(
            format_inline_comment_header_continuation_kind(Some("DENSE"), "RANK"),
            Some(FormatInlineCommentHeaderContinuationKind::OneDeeperThanCurrentLine)
        );
        assert_eq!(
            format_inline_comment_header_continuation_kind(None, "DENSE_RANK"),
            Some(FormatInlineCommentHeaderContinuationKind::OneDeeperThanCurrentLine)
        );
        assert_eq!(
            format_inline_comment_header_continuation_kind(Some("DENSE_RANK"), "LAST"),
            Some(FormatInlineCommentHeaderContinuationKind::OneDeeperThanCurrentLine)
        );
        assert_eq!(
            format_inline_comment_header_continuation_kind(None, "KEEP"),
            Some(FormatInlineCommentHeaderContinuationKind::OneDeeperThanCurrentLine)
        );
        assert_eq!(
            format_inline_comment_header_continuation_kind(Some("BETWEEN"), "TIMESTAMP"),
            Some(FormatInlineCommentHeaderContinuationKind::OneDeeperThanCurrentLine)
        );
        assert_eq!(
            format_inline_comment_header_continuation_kind(Some("DENSE_RANK"), "VALUE"),
            None
        );
        assert_eq!(
            format_inline_comment_header_continuation_kind(Some("LEFT"), "JOIN"),
            Some(FormatInlineCommentHeaderContinuationKind::OneDeeperThanCurrentLine)
        );
        assert_eq!(
            format_inline_comment_header_continuation_kind(None, "SEARCH"),
            Some(FormatInlineCommentHeaderContinuationKind::OneDeeperThanQueryBase)
        );
        assert_eq!(
            format_inline_comment_header_continuation_kind(None, "CYCLE"),
            Some(FormatInlineCommentHeaderContinuationKind::OneDeeperThanQueryBase)
        );
        assert_eq!(
            format_inline_comment_header_continuation_kind(None, "DUAL"),
            None
        );
    }

    #[test]
    fn format_leading_header_continuation_kind_tracks_inline_clause_item_prefixes() {
        assert_eq!(
            format_leading_header_continuation_kind("SET e.updated_at ="),
            Some(FormatInlineCommentHeaderContinuationKind::OneDeeperThanCurrentLine)
        );
        assert_eq!(
            format_leading_header_continuation_kind("UPDATE SET t.val ="),
            Some(FormatInlineCommentHeaderContinuationKind::OneDeeperThanCurrentLine)
        );
        assert_eq!(
            format_leading_header_continuation_kind("WHERE e.emp_id ="),
            Some(FormatInlineCommentHeaderContinuationKind::OneDeeperThanQueryBase)
        );
        assert_eq!(
            format_leading_header_continuation_kind("ORDER BY salary DESC"),
            Some(FormatInlineCommentHeaderContinuationKind::OneDeeperThanCurrentLine)
        );
        assert_eq!(
            format_leading_header_continuation_kind("ORDER SIBLINGS BY salary DESC"),
            Some(FormatInlineCommentHeaderContinuationKind::OneDeeperThanCurrentLine)
        );
        assert_eq!(
            format_leading_header_continuation_kind("e.job_title ="),
            None
        );
    }

    #[test]
    fn format_structural_header_continuation_kind_tracks_bare_headers_and_inline_prefixes() {
        assert_eq!(
            format_structural_header_continuation_kind("FROM"),
            Some(FormatInlineCommentHeaderContinuationKind::OneDeeperThanQueryBase)
        );
        assert_eq!(
            format_structural_header_continuation_kind("SELECT"),
            Some(FormatInlineCommentHeaderContinuationKind::OneDeeperThanCurrentLine)
        );
        assert_eq!(
            format_structural_header_continuation_kind("CALL"),
            Some(FormatInlineCommentHeaderContinuationKind::OneDeeperThanCurrentLine)
        );
        assert_eq!(
            format_structural_header_continuation_kind("JOIN"),
            Some(FormatInlineCommentHeaderContinuationKind::OneDeeperThanCurrentLine)
        );
        assert_eq!(
            format_structural_header_continuation_kind("ON"),
            Some(FormatInlineCommentHeaderContinuationKind::OneDeeperThanQueryBase)
        );
        assert_eq!(
            format_structural_header_continuation_kind("WHERE e.emp_id ="),
            Some(FormatInlineCommentHeaderContinuationKind::OneDeeperThanQueryBase)
        );
        assert_eq!(
            format_structural_header_continuation_kind("USING d,"),
            Some(FormatInlineCommentHeaderContinuationKind::OneDeeperThanQueryBase)
        );
        assert_eq!(
            format_structural_header_continuation_kind("LEFT OUTER JOIN"),
            Some(FormatInlineCommentHeaderContinuationKind::OneDeeperThanQueryBase)
        );
        assert_eq!(
            format_structural_header_continuation_kind("INNER JOIN"),
            Some(FormatInlineCommentHeaderContinuationKind::OneDeeperThanQueryBase)
        );
        assert_eq!(
            format_structural_header_continuation_kind("NATURAL LEFT JOIN"),
            Some(FormatInlineCommentHeaderContinuationKind::OneDeeperThanQueryBase)
        );
        assert_eq!(
            format_structural_header_continuation_kind("ORDER SIBLINGS BY"),
            Some(FormatInlineCommentHeaderContinuationKind::OneDeeperThanCurrentLine)
        );
    }

    #[test]
    fn format_structural_header_continuation_kind_preserves_bare_specificity_and_skips_named_owners(
    ) {
        assert_eq!(
            format_bare_structural_header_continuation_kind("WITHIN GROUP -- keep"),
            Some(FormatInlineCommentHeaderContinuationKind::SameDepth)
        );
        assert_eq!(
            format_bare_structural_header_continuation_kind("REFERENCE -- keep"),
            Some(FormatInlineCommentHeaderContinuationKind::SameDepth)
        );
        assert_eq!(
            format_bare_structural_header_continuation_kind("WINDOW -- keep"),
            Some(FormatInlineCommentHeaderContinuationKind::OneDeeperThanCurrentLine)
        );
        assert_eq!(
            format_bare_structural_header_continuation_kind("FROM TABLE -- keep"),
            Some(FormatInlineCommentHeaderContinuationKind::SameDepth)
        );
        assert_eq!(
            format_bare_structural_header_continuation_kind("LEFT JOIN TABLE -- keep"),
            Some(FormatInlineCommentHeaderContinuationKind::SameDepth)
        );
        assert_eq!(
            format_bare_structural_header_continuation_kind("ORDER SIBLINGS BY"),
            Some(FormatInlineCommentHeaderContinuationKind::OneDeeperThanCurrentLine)
        );
        assert_eq!(
            format_structural_header_continuation_kind("SELECT -- keep"),
            Some(FormatInlineCommentHeaderContinuationKind::OneDeeperThanCurrentLine)
        );
        assert_eq!(
            format_structural_header_continuation_kind("JOIN -- keep"),
            Some(FormatInlineCommentHeaderContinuationKind::OneDeeperThanCurrentLine)
        );
        assert_eq!(
            format_structural_header_continuation_kind("WINDOW w_dept AS -- named window"),
            None
        );
        assert_eq!(
            format_structural_header_continuation_kind("REFERENCE ref_limits ON -- named ref"),
            None
        );
        assert_eq!(
            format_structural_header_continuation_kind("OPEN c_emp FOR -- cursor query"),
            None
        );
        assert_eq!(
            format_structural_header_continuation_kind("CURSOR c_emp IS -- cursor query"),
            None
        );
    }

    #[test]
    fn format_leading_header_continuation_kind_uses_shared_keep_sequence_matcher_for_dense_rank_prefixes(
    ) {
        for line in [
            "DENSE_RANK",
            "DENSE_RANK LAST",
            "DENSE /* gap */ RANK",
            "DENSE /* gap */ RANK LAST",
        ] {
            assert_eq!(
                format_leading_header_continuation_kind(line),
                Some(FormatInlineCommentHeaderContinuationKind::OneDeeperThanCurrentLine),
                "leading structural prefix helper should route `{line}` through the shared KEEP sequence matcher"
            );
        }
    }

    #[test]
    fn format_inline_comment_structural_header_continuation_kind_overrides_only_collision_families()
    {
        for line in [
            "WITHIN GROUP -- keep",
            "KEEP -- keep",
            "MATCH_RECOGNIZE -- keep",
            "PIVOT -- keep",
            "UNPIVOT -- keep",
            "FROM TABLE -- keep",
            "USING TABLE -- keep",
            "JOIN TABLE -- keep",
            "LEFT JOIN TABLE -- keep",
            "LEFT OUTER JOIN TABLE -- keep",
            "RIGHT JOIN TABLE -- keep",
            "RIGHT OUTER JOIN TABLE -- keep",
            "FULL JOIN TABLE -- keep",
            "FULL OUTER JOIN TABLE -- keep",
            "INNER JOIN TABLE -- keep",
            "CROSS JOIN TABLE -- keep",
            "NATURAL JOIN TABLE -- keep",
            "NATURAL LEFT JOIN TABLE -- keep",
            "NATURAL LEFT OUTER JOIN TABLE -- keep",
            "NATURAL RIGHT JOIN TABLE -- keep",
            "NATURAL RIGHT OUTER JOIN TABLE -- keep",
            "NATURAL FULL JOIN TABLE -- keep",
            "NATURAL FULL OUTER JOIN TABLE -- keep",
            "EXISTS -- keep",
            "NOT EXISTS -- keep",
            "IN -- keep",
            "NOT IN -- keep",
            "ANY -- keep",
            "SOME -- keep",
            "ALL -- keep",
            "LATERAL -- keep",
            "TABLE -- keep",
            "CROSS APPLY -- keep",
            "OUTER APPLY -- keep",
            "CURSOR -- keep",
            "MULTISET -- keep",
        ] {
            assert!(
                line_is_exact_bare_owner_or_pending_header(line),
                "expected shared owner/pending classifier to recognize `{line}`"
            );
            assert_eq!(
                format_inline_comment_structural_header_continuation_kind(line),
                Some(FormatInlineCommentHeaderContinuationKind::SameDepth),
                "expected `{line}` to keep exact bare owner/header depth after inline comments"
            );
        }

        assert_eq!(
            format_inline_comment_structural_header_continuation_kind("JOIN -- keep"),
            Some(FormatInlineCommentHeaderContinuationKind::OneDeeperThanCurrentLine)
        );
        assert_eq!(
            format_inline_comment_structural_header_continuation_kind("REFERENCE -- keep"),
            Some(FormatInlineCommentHeaderContinuationKind::SameDepth)
        );
        assert_eq!(
            format_inline_comment_structural_header_continuation_kind("WINDOW -- keep"),
            Some(FormatInlineCommentHeaderContinuationKind::OneDeeperThanCurrentLine)
        );
        assert_eq!(
            format_inline_comment_structural_header_continuation_kind("FROM TABLE -- keep"),
            Some(FormatInlineCommentHeaderContinuationKind::SameDepth)
        );
        assert_eq!(
            format_inline_comment_structural_header_continuation_kind("LEFT JOIN TABLE -- keep"),
            Some(FormatInlineCommentHeaderContinuationKind::SameDepth)
        );
        assert_eq!(
            format_inline_comment_structural_header_continuation_kind(
                "/* owner */ RIGHT OUTER JOIN TABLE -- keep",
            ),
            Some(FormatInlineCommentHeaderContinuationKind::SameDepth)
        );
        assert_eq!(
            format_inline_comment_structural_header_continuation_kind("/* owner */ KEEP -- keep"),
            Some(FormatInlineCommentHeaderContinuationKind::SameDepth)
        );
        assert_eq!(
            format_inline_comment_structural_header_continuation_kind(
                "FOR UPDATE -- keep lock depth",
            ),
            Some(FormatInlineCommentHeaderContinuationKind::OneDeeperThanQueryBase)
        );
        assert_eq!(
            format_inline_comment_structural_header_continuation_kind(
                "RULES -- keep model body depth",
            ),
            Some(FormatInlineCommentHeaderContinuationKind::OneDeeperThanCurrentLine)
        );
        assert_eq!(
            format_inline_comment_structural_header_continuation_kind(
                "AFTER MATCH SKIP -- keep row-pattern body depth",
            ),
            Some(FormatInlineCommentHeaderContinuationKind::OneDeeperThanCurrentLine)
        );
    }

    #[test]
    fn resolve_format_header_continuation_depth_matches_shared_kind_semantics() {
        assert_eq!(
            resolve_format_header_continuation_depth(
                FormatInlineCommentHeaderContinuationKind::SameDepth,
                3,
                Some(1),
            ),
            3
        );
        assert_eq!(
            resolve_format_header_continuation_depth(
                FormatInlineCommentHeaderContinuationKind::OneDeeperThanQueryBase,
                3,
                Some(5),
            ),
            6
        );
        assert_eq!(
            resolve_format_header_continuation_depth(
                FormatInlineCommentHeaderContinuationKind::OneDeeperThanQueryBase,
                3,
                None,
            ),
            4
        );
        assert_eq!(
            resolve_format_header_continuation_depth(
                FormatInlineCommentHeaderContinuationKind::OneDeeperThanCurrentLine,
                3,
                Some(1),
            ),
            4
        );
    }

    #[test]
    fn resolve_format_header_continuation_depth_with_anchors_respects_distinct_same_depth_and_current_line_inputs(
    ) {
        assert_eq!(
            resolve_format_header_continuation_depth_with_anchors(
                FormatInlineCommentHeaderContinuationKind::SameDepth,
                2,
                5,
                Some(1),
            ),
            2
        );
        assert_eq!(
            resolve_format_header_continuation_depth_with_anchors(
                FormatInlineCommentHeaderContinuationKind::OneDeeperThanQueryBase,
                2,
                5,
                Some(3),
            ),
            4
        );
        assert_eq!(
            resolve_format_header_continuation_depth_with_anchors(
                FormatInlineCommentHeaderContinuationKind::OneDeeperThanCurrentLine,
                2,
                5,
                Some(3),
            ),
            6
        );
    }

    #[test]
    fn format_inline_comment_structural_header_continuation_kind_normalizes_mixed_leading_close_owner_header_lines(
    ) {
        for (line, expected) in [
            (
                ") RIGHT OUTER JOIN TABLE -- keep",
                Some(FormatInlineCommentHeaderContinuationKind::SameDepth),
            ),
            (
                ") KEEP -- keep",
                Some(FormatInlineCommentHeaderContinuationKind::SameDepth),
            ),
            (
                ") WINDOW -- keep",
                Some(FormatInlineCommentHeaderContinuationKind::OneDeeperThanCurrentLine),
            ),
            (
                ") JOIN -- keep",
                Some(FormatInlineCommentHeaderContinuationKind::OneDeeperThanCurrentLine),
            ),
            (
                ") FOR UPDATE -- keep",
                Some(FormatInlineCommentHeaderContinuationKind::OneDeeperThanQueryBase),
            ),
            (") WINDOW w_dept AS -- named owner", None),
            (
                ") REFERENCE ref_limits ON -- named owner",
                Some(FormatInlineCommentHeaderContinuationKind::SameDepth),
            ),
            (") OPEN c_emp FOR -- cursor query", None),
        ] {
            assert_eq!(
                format_inline_comment_structural_header_continuation_kind(line),
                expected,
                "mixed leading-close structural tail should preserve shared owner/header semantics for `{line}`",
            );
        }
    }

    #[test]
    fn format_inline_comment_structural_header_continuation_kind_uses_owner_relative_sequence_policy_for_exact_bare_split_prefixes(
    ) {
        for line in [
            "DENSE_RANK -- keep",
            "DENSE_RANK FIRST -- keep",
            "DENSE_RANK LAST -- keep",
            "DENSE_RANK FIRST ORDER -- keep",
            "DENSE_RANK LAST ORDER -- keep",
            "AFTER MATCH -- keep",
            "AFTER MATCH SKIP TO -- keep",
            "AFTER MATCH SKIP TO NEXT -- keep",
            "AFTER MATCH SKIP TO NEXT ROW -- keep",
            "ONE ROW PER -- keep",
            "ALL ROWS PER -- keep",
            "WITH UNMATCHED -- keep",
            "WITHOUT UNMATCHED -- keep",
            "SHOW EMPTY -- keep",
            "OMIT EMPTY -- keep",
            "RETURN ALL -- keep",
            "RETURN UPDATED -- keep",
            "AUTOMATIC ORDER -- keep",
            "SEQUENTIAL ORDER -- keep",
            "UNIQUE SINGLE REFERENCE -- keep",
        ] {
            assert_eq!(
                format_inline_comment_structural_header_continuation_kind(line),
                Some(FormatInlineCommentHeaderContinuationKind::SameDepth),
                "expected `{line}` to preserve the exact bare owner-relative split-header depth"
            );
        }

        for line in [
            "RULES -- keep model body depth",
            "AFTER MATCH SKIP -- keep row-pattern body depth",
            "PARTITION BY -- keep analytic body depth",
            "ORDER BY -- keep analytic body depth",
            "MEASURES -- keep row-pattern body depth",
        ] {
            assert_eq!(
                format_inline_comment_structural_header_continuation_kind(line),
                Some(FormatInlineCommentHeaderContinuationKind::OneDeeperThanCurrentLine),
                "expected `{line}` to keep the exact bare body-header operand depth"
            );
        }
    }

    #[test]
    fn format_inline_comment_continuation_kind_prioritizes_exact_bare_deferred_wrapper_owners_over_operator_fallback(
    ) {
        for line in [
            "EXISTS -- keep",
            "NOT EXISTS -- keep",
            "IN -- keep",
            "NOT IN -- keep",
            "ANY -- keep",
            "SOME -- keep",
            "ALL -- keep",
        ] {
            assert_eq!(
                format_inline_comment_continuation_kind(line),
                Some(FormatInlineCommentHeaderContinuationKind::SameDepth),
                "expected `{line}` to preserve same-depth deferred-wrapper owner carry"
            );
        }
    }

    #[test]
    fn format_inline_comment_continuation_kind_prioritizes_structural_header_family_over_trailing_operator_for_header_lines(
    ) {
        for (line, expected) in [
            (
                "WHERE e.emp_id = -- keep",
                Some(FormatInlineCommentHeaderContinuationKind::OneDeeperThanQueryBase),
            ),
            (
                "WHERE e.member_col MEMBER OF -- keep",
                Some(FormatInlineCommentHeaderContinuationKind::OneDeeperThanQueryBase),
            ),
            (
                "ON e.emp_id = -- keep",
                Some(FormatInlineCommentHeaderContinuationKind::OneDeeperThanQueryBase),
            ),
            (
                "AND e.status_cd = -- keep",
                Some(FormatInlineCommentHeaderContinuationKind::OneDeeperThanCurrentLine),
            ),
            (
                "v_total := -- keep",
                Some(FormatInlineCommentHeaderContinuationKind::OneDeeperThanCurrentLine),
            ),
        ] {
            assert_eq!(
                format_inline_comment_continuation_kind(line),
                expected,
                "expected `{line}` to prefer structural header carry before pure RHS fallback"
            );
        }
    }

    #[test]
    fn format_inline_comment_continuation_kind_preserves_exact_bare_structural_family_semantics_before_lexical_fallback(
    ) {
        for (line, expected) in [
            (
                "INNER JOIN -- keep",
                Some(FormatInlineCommentHeaderContinuationKind::OneDeeperThanQueryBase),
            ),
            (
                "RETURN UPDATED -- keep",
                Some(FormatInlineCommentHeaderContinuationKind::SameDepth),
            ),
        ] {
            assert_eq!(
                format_inline_comment_continuation_kind(line),
                expected,
                "expected `{line}` to keep the exact bare structural family semantics instead of a lexical fallback"
            );
        }
    }

    #[test]
    fn format_inline_comment_continuation_kind_keeps_rhs_operator_fallback_for_non_header_lines() {
        for line in [
            "AND e.member_col MEMBER OF -- keep",
            "AND e.num_nt SUBMULTISET OF -- keep",
            "AND e.ename LIKE4 -- keep",
            "AND e.ename LIKE 'A%' ESCAPE -- keep",
            "v_total := -- keep",
            "pkg_lock.request => -- keep",
        ] {
            assert_eq!(
                format_inline_comment_continuation_kind(line),
                Some(FormatInlineCommentHeaderContinuationKind::OneDeeperThanCurrentLine),
                "expected `{line}` to use RHS-operator continuation depth"
            );
        }
    }

    #[test]
    fn structural_tail_helpers_consume_mixed_leading_close_before_reclassifying_headers() {
        assert_eq!(
            auto_format_structural_tail(") ORDER BY empno"),
            "ORDER BY empno"
        );
        assert_eq!(
            auto_format_structural_tail("/*keep*/ ORDER BY empno"),
            "ORDER BY empno"
        );
        assert_eq!(
            auto_format_structural_tail("/*c*/ ) FOR UPDATE"),
            "FOR UPDATE"
        );
        assert_eq!(auto_format_structural_tail(") MULTISET"), "MULTISET");
        assert_eq!(auto_format_structural_tail(") CURSOR"), "CURSOR");
        assert_eq!(
            auto_format_structural_tail(") WITHIN GROUP"),
            "WITHIN GROUP"
        );
        assert_eq!(auto_format_structural_tail(") KEEP"), "KEEP");
        assert_eq!(auto_format_structural_tail(") DENSE_RANK"), "DENSE_RANK");
        assert_eq!(
            auto_format_structural_tail(") DENSE_RANK FIRST"),
            "DENSE_RANK FIRST"
        );
        assert_eq!(
            auto_format_structural_tail(") DENSE_RANK LAST"),
            "DENSE_RANK LAST"
        );
        assert_eq!(
            auto_format_structural_tail(") DENSE_RANK FIRST ORDER"),
            "DENSE_RANK FIRST ORDER"
        );
        assert_eq!(
            auto_format_structural_tail(") DENSE_RANK LAST ORDER BY"),
            "DENSE_RANK LAST ORDER BY"
        );
        assert_eq!(auto_format_structural_tail(") AFTER MATCH"), "AFTER MATCH");
        assert_eq!(
            auto_format_structural_tail(") AFTER MATCH SKIP"),
            "AFTER MATCH SKIP"
        );
        assert_eq!(
            auto_format_structural_tail(") AFTER MATCH SKIP TO"),
            "AFTER MATCH SKIP TO"
        );
        assert_eq!(
            auto_format_structural_tail(") AFTER MATCH SKIP TO NEXT"),
            "AFTER MATCH SKIP TO NEXT"
        );
        assert_eq!(auto_format_structural_tail(") ONE ROW PER"), "ONE ROW PER");
        assert_eq!(
            auto_format_structural_tail(") ALL ROWS PER"),
            "ALL ROWS PER"
        );
        assert_eq!(
            auto_format_structural_tail(") AFTER MATCH SKIP TO NEXT ROW"),
            "AFTER MATCH SKIP TO NEXT ROW"
        );
        assert_eq!(
            auto_format_structural_tail(") WITH UNMATCHED"),
            "WITH UNMATCHED"
        );
        assert_eq!(
            auto_format_structural_tail(") WITHOUT UNMATCHED"),
            "WITHOUT UNMATCHED"
        );
        assert_eq!(auto_format_structural_tail(") SHOW EMPTY"), "SHOW EMPTY");
        assert_eq!(auto_format_structural_tail(") OMIT EMPTY"), "OMIT EMPTY");
        assert_eq!(auto_format_structural_tail(") RETURN ALL"), "RETURN ALL");
        assert_eq!(
            auto_format_structural_tail(") RETURN UPDATED"),
            "RETURN UPDATED"
        );
        assert_eq!(
            auto_format_structural_tail(") AUTOMATIC ORDER"),
            "AUTOMATIC ORDER"
        );
        assert_eq!(
            auto_format_structural_tail(") SEQUENTIAL ORDER"),
            "SEQUENTIAL ORDER"
        );
        assert_eq!(
            auto_format_structural_tail(") TRUE ON ERROR"),
            "TRUE ON ERROR"
        );
        assert_eq!(
            auto_format_structural_tail(") FALSE ON EMPTY"),
            "FALSE ON EMPTY"
        );
        assert_eq!(
            auto_format_structural_tail(") EMPTY ARRAY ON ERROR"),
            "EMPTY ARRAY ON ERROR"
        );
        assert_eq!(
            auto_format_structural_tail(") ALL ROWS PER MATCH"),
            "ALL ROWS PER MATCH"
        );
        assert_eq!(
            auto_format_structural_tail(") RETURN ALL ROWS"),
            "RETURN ALL ROWS"
        );
        assert_eq!(auto_format_structural_tail(") UPSERT ALL"), "UPSERT ALL");
        assert_eq!(
            auto_format_structural_tail(") UNIQUE SINGLE REFERENCE"),
            "UNIQUE SINGLE REFERENCE"
        );
        assert_eq!(
            format_bare_structural_header_continuation_kind(") ORDER BY"),
            Some(FormatInlineCommentHeaderContinuationKind::OneDeeperThanCurrentLine)
        );
        assert_eq!(
            format_bare_structural_header_continuation_kind(") GROUP BY"),
            Some(FormatInlineCommentHeaderContinuationKind::OneDeeperThanCurrentLine)
        );
        assert_eq!(
            format_bare_structural_header_continuation_kind(") MULTISET"),
            Some(FormatInlineCommentHeaderContinuationKind::SameDepth)
        );
        assert_eq!(
            format_bare_structural_header_continuation_kind(") CURSOR"),
            Some(FormatInlineCommentHeaderContinuationKind::SameDepth)
        );
        assert_eq!(
            format_bare_structural_header_continuation_kind(") WITHIN GROUP"),
            Some(FormatInlineCommentHeaderContinuationKind::SameDepth)
        );
        assert_eq!(
            format_bare_structural_header_continuation_kind(") KEEP"),
            Some(FormatInlineCommentHeaderContinuationKind::SameDepth)
        );
        assert_eq!(
            format_bare_structural_header_continuation_kind(") DENSE_RANK"),
            Some(FormatInlineCommentHeaderContinuationKind::SameDepth)
        );
        assert_eq!(
            format_bare_structural_header_continuation_kind(") DENSE_RANK FIRST"),
            Some(FormatInlineCommentHeaderContinuationKind::SameDepth)
        );
        assert_eq!(
            format_bare_structural_header_continuation_kind(") DENSE_RANK LAST"),
            Some(FormatInlineCommentHeaderContinuationKind::SameDepth)
        );
        assert_eq!(
            format_bare_structural_header_continuation_kind(") DENSE_RANK FIRST ORDER"),
            Some(FormatInlineCommentHeaderContinuationKind::SameDepth)
        );
        assert_eq!(
            format_bare_structural_header_continuation_kind(") DENSE_RANK LAST ORDER BY"),
            Some(FormatInlineCommentHeaderContinuationKind::OneDeeperThanCurrentLine)
        );
        assert_eq!(
            format_bare_structural_header_continuation_kind(") AFTER MATCH"),
            Some(FormatInlineCommentHeaderContinuationKind::SameDepth)
        );
        assert_eq!(
            format_bare_structural_header_continuation_kind(") AFTER MATCH SKIP"),
            Some(FormatInlineCommentHeaderContinuationKind::SameDepth)
        );
        assert_eq!(
            format_bare_structural_header_continuation_kind(") AFTER MATCH SKIP TO"),
            Some(FormatInlineCommentHeaderContinuationKind::SameDepth)
        );
        assert_eq!(
            format_bare_structural_header_continuation_kind(") AFTER MATCH SKIP TO NEXT"),
            Some(FormatInlineCommentHeaderContinuationKind::SameDepth)
        );
        assert_eq!(
            format_bare_structural_header_continuation_kind(") ONE ROW PER"),
            Some(FormatInlineCommentHeaderContinuationKind::SameDepth)
        );
        assert_eq!(
            format_bare_structural_header_continuation_kind(") ALL ROWS PER"),
            Some(FormatInlineCommentHeaderContinuationKind::SameDepth)
        );
        assert_eq!(
            format_bare_structural_header_continuation_kind(") AFTER MATCH SKIP TO NEXT ROW"),
            Some(FormatInlineCommentHeaderContinuationKind::SameDepth)
        );
        assert_eq!(
            format_bare_structural_header_continuation_kind(") WITH UNMATCHED"),
            Some(FormatInlineCommentHeaderContinuationKind::SameDepth)
        );
        assert_eq!(
            format_bare_structural_header_continuation_kind(") WITHOUT UNMATCHED"),
            Some(FormatInlineCommentHeaderContinuationKind::SameDepth)
        );
        assert_eq!(
            format_bare_structural_header_continuation_kind(") SHOW EMPTY"),
            Some(FormatInlineCommentHeaderContinuationKind::SameDepth)
        );
        assert_eq!(
            format_bare_structural_header_continuation_kind(") OMIT EMPTY"),
            Some(FormatInlineCommentHeaderContinuationKind::SameDepth)
        );
        assert_eq!(
            format_bare_structural_header_continuation_kind(") RETURN ALL"),
            Some(FormatInlineCommentHeaderContinuationKind::SameDepth)
        );
        assert_eq!(
            format_bare_structural_header_continuation_kind(") RETURN UPDATED"),
            Some(FormatInlineCommentHeaderContinuationKind::SameDepth)
        );
        assert_eq!(
            format_bare_structural_header_continuation_kind(") AUTOMATIC ORDER"),
            Some(FormatInlineCommentHeaderContinuationKind::SameDepth)
        );
        assert_eq!(
            format_bare_structural_header_continuation_kind(") SEQUENTIAL ORDER"),
            Some(FormatInlineCommentHeaderContinuationKind::SameDepth)
        );
        assert_eq!(
            format_bare_structural_header_continuation_kind(") ALL ROWS PER MATCH"),
            Some(FormatInlineCommentHeaderContinuationKind::SameDepth)
        );
        assert_eq!(
            format_bare_structural_header_continuation_kind(") RETURN ALL ROWS"),
            Some(FormatInlineCommentHeaderContinuationKind::SameDepth)
        );
        assert_eq!(
            format_bare_structural_header_continuation_kind(") UPSERT ALL"),
            Some(FormatInlineCommentHeaderContinuationKind::SameDepth)
        );
        assert_eq!(
            format_bare_structural_header_continuation_kind(") UNIQUE SINGLE REFERENCE"),
            Some(FormatInlineCommentHeaderContinuationKind::SameDepth)
        );
        assert_eq!(
            format_structural_header_continuation_kind(") FOR UPDATE -- lock"),
            Some(FormatInlineCommentHeaderContinuationKind::OneDeeperThanQueryBase)
        );
        assert_eq!(
            format_bare_structural_header_continuation_kind(") FOR"),
            Some(FormatInlineCommentHeaderContinuationKind::SameDepth)
        );
        assert_eq!(
            format_inline_comment_structural_header_continuation_kind("FOR -- lock"),
            Some(FormatInlineCommentHeaderContinuationKind::SameDepth)
        );
        assert_eq!(
            format_inline_comment_structural_header_continuation_kind(") FOR -- lock"),
            Some(FormatInlineCommentHeaderContinuationKind::SameDepth)
        );
        assert!(
            starts_with_auto_format_structural_continuation_boundary_without_expression_owner(
                ") ORDER BY empno"
            )
        );
        assert!(
            starts_with_auto_format_structural_continuation_boundary_without_expression_owner(
                ") FOR UPDATE"
            )
        );
        assert!(starts_with_auto_format_structural_continuation_boundary(
            ") FOR UPDATE"
        ));
        assert!(
            starts_with_auto_format_structural_continuation_boundary_without_expression_owner(
                "ON DUPLICATE KEY UPDATE dept_name = CONCAT("
            )
        );
        assert!(starts_with_auto_format_structural_continuation_boundary(
            "ON DUPLICATE KEY UPDATE dept_name = CONCAT("
        ));
        assert!(line_has_mixed_leading_close_continuation(") MULTISET"));
        assert!(line_has_mixed_leading_close_continuation(") CURSOR"));
        assert!(!line_has_mixed_leading_close_continuation("MULTISET"));
        assert!(!line_has_mixed_leading_close_continuation("CURSOR"));
        assert!(!line_has_mixed_leading_close_continuation(
            "DENSE_RANK LAST"
        ));
        assert!(!line_has_mixed_leading_close_continuation("FOR"));
    }

    #[test]
    fn format_bare_structural_header_continuation_kind_reuses_shared_prefix_taxonomy_for_exact_keyword_lines(
    ) {
        assert_eq!(
            format_bare_structural_header_continuation_kind("SELECT DISTINCT"),
            format_structural_header_continuation_kind("SELECT DISTINCT")
        );
        assert_eq!(
            format_bare_structural_header_continuation_kind("CALL"),
            format_structural_header_continuation_kind("CALL")
        );
        assert_eq!(
            format_bare_structural_header_continuation_kind("LEFT OUTER JOIN"),
            format_structural_header_continuation_kind("LEFT OUTER JOIN")
        );
        assert_eq!(
            format_bare_structural_header_continuation_kind("INNER JOIN"),
            format_structural_header_continuation_kind("INNER JOIN")
        );
        assert_eq!(
            format_bare_structural_header_continuation_kind("NATURAL LEFT JOIN"),
            format_structural_header_continuation_kind("NATURAL LEFT JOIN")
        );
        assert_eq!(
            format_bare_structural_header_continuation_kind("PARTITION BY"),
            format_structural_header_continuation_kind("PARTITION BY")
        );
        assert_eq!(
            format_bare_structural_header_continuation_kind("MEASURES"),
            format_structural_header_continuation_kind("MEASURES")
        );
        assert_eq!(
            format_bare_structural_header_continuation_kind("SUBSET"),
            format_structural_header_continuation_kind("SUBSET")
        );
        assert_eq!(
            format_bare_structural_header_continuation_kind("PATTERN"),
            format_structural_header_continuation_kind("PATTERN")
        );
        assert_eq!(
            format_bare_structural_header_continuation_kind("DEFINE"),
            format_structural_header_continuation_kind("DEFINE")
        );
        assert_eq!(
            format_bare_structural_header_continuation_kind("AFTER MATCH SKIP"),
            Some(FormatInlineCommentHeaderContinuationKind::SameDepth)
        );
        assert_eq!(
            format_bare_structural_header_continuation_kind("AFTER MATCH SKIP TO"),
            Some(FormatInlineCommentHeaderContinuationKind::SameDepth)
        );
        assert_eq!(
            format_bare_structural_header_continuation_kind("WITHIN GROUP"),
            Some(FormatInlineCommentHeaderContinuationKind::SameDepth)
        );
        assert_eq!(
            format_bare_structural_header_continuation_kind("DENSE_RANK"),
            Some(FormatInlineCommentHeaderContinuationKind::SameDepth)
        );
        assert_eq!(
            format_bare_structural_header_continuation_kind("DENSE_RANK LAST"),
            Some(FormatInlineCommentHeaderContinuationKind::SameDepth)
        );
        assert_eq!(
            format_bare_structural_header_continuation_kind("DENSE_RANK LAST ORDER BY"),
            Some(FormatInlineCommentHeaderContinuationKind::OneDeeperThanCurrentLine)
        );
        assert_eq!(
            format_bare_structural_header_continuation_kind("DENSE /* gap */ RANK"),
            Some(FormatInlineCommentHeaderContinuationKind::SameDepth)
        );
        assert_eq!(
            format_bare_structural_header_continuation_kind("DENSE /* gap */ RANK LAST ORDER BY"),
            Some(FormatInlineCommentHeaderContinuationKind::OneDeeperThanCurrentLine)
        );
        assert_eq!(
            format_bare_structural_header_continuation_kind("SELECT /*+ keep carrying"),
            Some(FormatInlineCommentHeaderContinuationKind::OneDeeperThanCurrentLine)
        );
        assert_eq!(
            format_bare_structural_header_continuation_kind("GROUP BY /* keep carrying"),
            Some(FormatInlineCommentHeaderContinuationKind::OneDeeperThanCurrentLine)
        );
        assert_eq!(
            format_bare_structural_header_continuation_kind("FOR"),
            Some(FormatInlineCommentHeaderContinuationKind::SameDepth)
        );
        assert_eq!(
            format_bare_structural_header_continuation_kind("FOR /* keep carrying"),
            Some(FormatInlineCommentHeaderContinuationKind::SameDepth)
        );
        assert_eq!(
            format_bare_structural_header_continuation_kind("SELECT DISTINCT empno"),
            None
        );
        assert_eq!(
            format_bare_structural_header_continuation_kind("LEFT OUTER JOIN dept d"),
            None
        );
        assert_eq!(
            format_bare_structural_header_continuation_kind("PARTITION BY (deptno)"),
            None
        );
        assert_eq!(
            format_bare_structural_header_continuation_kind("REFERENCE"),
            Some(FormatInlineCommentHeaderContinuationKind::SameDepth)
        );
        assert_eq!(
            format_bare_structural_header_continuation_kind("RULES"),
            Some(FormatInlineCommentHeaderContinuationKind::SameDepth)
        );
        assert_eq!(
            format_bare_structural_header_continuation_kind("KEEP"),
            Some(FormatInlineCommentHeaderContinuationKind::SameDepth)
        );
        assert_eq!(
            format_bare_structural_header_continuation_kind("LEFT OUTER"),
            Some(FormatInlineCommentHeaderContinuationKind::SameDepth)
        );
        assert_eq!(
            format_bare_structural_header_continuation_kind("INNER JOIN"),
            Some(FormatInlineCommentHeaderContinuationKind::OneDeeperThanQueryBase)
        );
        assert_eq!(
            format_bare_structural_header_continuation_kind("NATURAL LEFT JOIN"),
            Some(FormatInlineCommentHeaderContinuationKind::OneDeeperThanQueryBase)
        );
        assert_eq!(
            format_bare_structural_header_continuation_kind("EXISTS"),
            Some(FormatInlineCommentHeaderContinuationKind::SameDepth)
        );
        assert_eq!(
            format_bare_structural_header_continuation_kind("IN"),
            Some(FormatInlineCommentHeaderContinuationKind::SameDepth)
        );
        assert_eq!(
            format_bare_structural_header_continuation_kind("NOT EXISTS"),
            Some(FormatInlineCommentHeaderContinuationKind::SameDepth)
        );
        assert_eq!(
            format_bare_structural_header_continuation_kind("NOT IN"),
            Some(FormatInlineCommentHeaderContinuationKind::SameDepth)
        );
        assert_eq!(
            format_bare_structural_header_continuation_kind("ANY"),
            Some(FormatInlineCommentHeaderContinuationKind::SameDepth)
        );
        assert_eq!(
            format_bare_structural_header_continuation_kind("SOME"),
            Some(FormatInlineCommentHeaderContinuationKind::SameDepth)
        );
        assert_eq!(
            format_bare_structural_header_continuation_kind("ALL"),
            Some(FormatInlineCommentHeaderContinuationKind::SameDepth)
        );
        assert_eq!(
            format_bare_structural_header_continuation_kind("LATERAL"),
            Some(FormatInlineCommentHeaderContinuationKind::SameDepth)
        );
        assert_eq!(
            format_bare_structural_header_continuation_kind("CROSS APPLY"),
            Some(FormatInlineCommentHeaderContinuationKind::SameDepth)
        );
        assert_eq!(
            format_bare_structural_header_continuation_kind("/* owner */ CROSS APPLY"),
            Some(FormatInlineCommentHeaderContinuationKind::SameDepth)
        );
        assert_eq!(
            format_bare_structural_header_continuation_kind("OUTER APPLY"),
            Some(FormatInlineCommentHeaderContinuationKind::SameDepth)
        );
        assert_eq!(
            format_bare_structural_header_continuation_kind("TABLE"),
            Some(FormatInlineCommentHeaderContinuationKind::SameDepth)
        );
        assert_eq!(
            format_bare_structural_header_continuation_kind("/* owner */ TABLE"),
            Some(FormatInlineCommentHeaderContinuationKind::SameDepth)
        );
        assert_eq!(
            format_bare_structural_header_continuation_kind("MULTISET"),
            Some(FormatInlineCommentHeaderContinuationKind::SameDepth)
        );
        assert_eq!(
            format_bare_structural_header_continuation_kind("CURSOR"),
            Some(FormatInlineCommentHeaderContinuationKind::SameDepth)
        );
        assert_eq!(
            format_bare_structural_header_continuation_kind("PIVOT"),
            Some(FormatInlineCommentHeaderContinuationKind::SameDepth)
        );
        assert_eq!(
            format_bare_structural_header_continuation_kind("UNPIVOT"),
            Some(FormatInlineCommentHeaderContinuationKind::SameDepth)
        );
    }

    #[test]
    fn structural_continuation_boundary_helper_blocks_owner_relative_subclauses() {
        assert!(
            starts_with_auto_format_structural_continuation_boundary_without_expression_owner(
                "AUTOMATIC ORDER"
            )
        );
        assert!(
            starts_with_auto_format_structural_continuation_boundary_without_expression_owner(
                "MEASURES match_number () AS match_no"
            )
        );
        assert!(
            !starts_with_auto_format_structural_continuation_boundary_without_expression_owner(
                "emp e"
            )
        );
    }

    #[test]
    fn structural_continuation_boundary_helper_covers_merge_branches_and_wrappers() {
        assert!(starts_with_auto_format_structural_continuation_boundary(
            "WHEN"
        ));
        assert!(starts_with_auto_format_structural_continuation_boundary(
            "WHEN NOT"
        ));
        assert!(starts_with_auto_format_structural_continuation_boundary(
            "WHEN MATCHED THEN"
        ));
        assert!(starts_with_auto_format_structural_continuation_boundary(
            "WHEN NOT MATCHED THEN"
        ));
        assert!(!starts_with_auto_format_structural_continuation_boundary(
            "MATCHED"
        ));
        assert!(starts_with_auto_format_structural_continuation_boundary(
            "( -- wrapper"
        ));
        assert!(starts_with_format_merge_branch_header("WHEN MATCHED THEN"));
        assert!(starts_with_format_merge_branch_header(
            "WHEN NOT MATCHED THEN"
        ));
        assert!(starts_with_format_merge_branch_condition_clause(
            "AND target.is_active = 1"
        ));
        assert!(starts_with_format_merge_branch_condition_clause(
            "OR target.is_active = 1"
        ));
        assert!(!starts_with_format_merge_branch_header("WHENEVER SQLERROR"));
    }

    #[test]
    fn format_merge_branch_pending_header_kind_tracks_split_merge_headers() {
        assert_eq!(
            format_merge_branch_pending_header_kind("WHEN"),
            Some(PendingFormatMergeBranchHeaderKind::When)
        );
        assert_eq!(
            format_merge_branch_pending_header_kind("WHEN /* gap */ -- keep"),
            Some(PendingFormatMergeBranchHeaderKind::When)
        );
        assert_eq!(
            format_merge_branch_pending_header_kind("WHEN NOT"),
            Some(PendingFormatMergeBranchHeaderKind::WhenNot)
        );
        assert_eq!(
            format_merge_branch_pending_header_kind("WHEN MATCHED"),
            Some(PendingFormatMergeBranchHeaderKind::Condition)
        );
        assert_eq!(
            format_merge_branch_pending_header_kind("WHEN/* gap */NOT/* gap */MATCHED"),
            Some(PendingFormatMergeBranchHeaderKind::Condition)
        );
        assert_eq!(
            format_merge_branch_pending_header_kind("WHEN MATCHED THEN"),
            None
        );
        assert_eq!(
            format_merge_branch_pending_header_kind("WHEN score > 1000 THEN 'HIGH'"),
            None
        );
        assert_eq!(
            format_merge_branch_pending_header_kind("WHEN NOT score > 1000 THEN 'HIGH'"),
            None
        );
        assert_eq!(
            PendingFormatMergeBranchHeaderKind::When.progress_over_line("MATCHED"),
            Some(PendingFormatMergeBranchHeaderProgress {
                next_kind: Some(PendingFormatMergeBranchHeaderKind::Condition),
                completed: false,
                uses_condition_depth: false,
            })
        );
        assert_eq!(
            PendingFormatMergeBranchHeaderKind::Condition
                .progress_over_line("AND target.is_active = 'Y'"),
            Some(PendingFormatMergeBranchHeaderProgress {
                next_kind: Some(PendingFormatMergeBranchHeaderKind::Condition),
                completed: false,
                uses_condition_depth: true,
            })
        );
        assert_eq!(
            PendingFormatMergeBranchHeaderKind::Condition.progress_over_line("AND EXISTS ("),
            Some(PendingFormatMergeBranchHeaderProgress {
                next_kind: Some(PendingFormatMergeBranchHeaderKind::Condition),
                completed: false,
                uses_condition_depth: true,
            })
        );
        assert_eq!(
            PendingFormatMergeBranchHeaderKind::Condition.progress_over_line(") AND EXISTS ("),
            Some(PendingFormatMergeBranchHeaderProgress {
                next_kind: Some(PendingFormatMergeBranchHeaderKind::Condition),
                completed: false,
                uses_condition_depth: true,
            })
        );
        assert_eq!(
            PendingFormatMergeBranchHeaderKind::Condition.progress_over_line("THEN"),
            Some(PendingFormatMergeBranchHeaderProgress {
                next_kind: None,
                completed: true,
                uses_condition_depth: true,
            })
        );
    }

    #[test]
    fn merge_branch_same_depth_header_fragment_helper_keeps_merge_only_fragments_out_of_generic_not_owner_carry(
    ) {
        assert!(line_is_active_merge_branch_same_depth_header_fragment(
            "WHEN", true, None,
        ));
        assert!(line_is_active_merge_branch_same_depth_header_fragment(
            "WHEN NOT", true, None,
        ));
        assert!(line_is_active_merge_branch_same_depth_header_fragment(
            "NOT",
            true,
            Some(PendingFormatMergeBranchHeaderKind::When),
        ));
        assert!(line_is_active_merge_branch_same_depth_header_fragment(
            "MATCHED",
            true,
            Some(PendingFormatMergeBranchHeaderKind::When),
        ));
        assert!(!line_is_active_merge_branch_same_depth_header_fragment(
            "AND NOT",
            true,
            Some(PendingFormatMergeBranchHeaderKind::Condition),
        ));
        assert!(!line_is_active_merge_branch_same_depth_header_fragment(
            "THEN",
            true,
            Some(PendingFormatMergeBranchHeaderKind::Condition),
        ));
        assert!(!line_is_active_merge_branch_same_depth_header_fragment(
            "WHEN NOT", false, None,
        ));
    }

    #[test]
    fn merge_branch_condition_suspend_helper_keeps_nested_owner_lines_attached_to_retained_header_state(
    ) {
        assert!(line_suspends_active_merge_branch_condition_state(
            "EXISTS (",
            Some(PendingFormatMergeBranchHeaderKind::Condition),
        ));
        assert!(line_suspends_active_merge_branch_condition_state(
            "/* gap */ EXISTS (",
            Some(PendingFormatMergeBranchHeaderKind::Condition),
        ));
        assert!(line_suspends_active_merge_branch_condition_state(
            "(",
            Some(PendingFormatMergeBranchHeaderKind::Condition),
        ));
        assert!(!line_suspends_active_merge_branch_condition_state(
            "AND NOT",
            Some(PendingFormatMergeBranchHeaderKind::Condition),
        ));
        assert!(!line_suspends_active_merge_branch_condition_state(
            "THEN",
            Some(PendingFormatMergeBranchHeaderKind::Condition),
        ));
    }

    #[test]
    fn format_indented_paren_owner_kind_detects_analytic_over_without_matching_overlay() {
        assert_eq!(
            format_indented_paren_owner_kind("SUM (sal) OVER ( -- analytic window"),
            Some(FormatIndentedParenOwnerKind::AnalyticOver)
        );
        assert_eq!(
            format_indented_paren_owner_kind("REFERENCE ref_limits ON ( -- model reference"),
            Some(FormatIndentedParenOwnerKind::ModelSubclause)
        );
        assert_eq!(
            format_indented_paren_owner_kind("OVERLAY (name PLACING 'X' FROM 1 FOR 1)"),
            None
        );
    }

    #[test]
    fn format_indented_paren_owner_kind_covers_stack_managed_multiline_owners() {
        assert_eq!(
            format_indented_paren_owner_kind("WINDOW w_dept AS ( -- named window"),
            Some(FormatIndentedParenOwnerKind::Window)
        );
        assert_eq!(
            format_indented_paren_owner_kind("LISTAGG (ename, ', ') WITHIN GROUP ("),
            Some(FormatIndentedParenOwnerKind::WithinGroup)
        );
        assert_eq!(
            format_indented_paren_owner_kind("MAX (sal) KEEP ("),
            Some(FormatIndentedParenOwnerKind::Keep)
        );
        assert_eq!(
            format_indented_paren_owner_kind("MATCH_RECOGNIZE ( -- pattern input"),
            Some(FormatIndentedParenOwnerKind::MatchRecognize)
        );
        assert_eq!(
            format_indented_paren_owner_kind("FROM src PIVOT ( -- rotate rows"),
            Some(FormatIndentedParenOwnerKind::Pivot)
        );
        assert_eq!(
            format_indented_paren_owner_kind("FROM src UNPIVOT ( -- rotate cols"),
            Some(FormatIndentedParenOwnerKind::Unpivot)
        );
        assert_eq!(
            format_indented_paren_owner_kind("NESTED PATH '$.items[*]' COLUMNS ( -- nested"),
            Some(FormatIndentedParenOwnerKind::StructuredColumns)
        );
    }

    #[test]
    fn format_indented_paren_owner_kind_covers_modified_pivot_unpivot_owners() {
        assert_eq!(
            format_indented_paren_owner_kind("FROM src PIVOT XML ( -- xml pivot"),
            Some(FormatIndentedParenOwnerKind::Pivot)
        );
        assert_eq!(
            format_indented_paren_owner_header_kind("PIVOT XML"),
            Some(FormatIndentedParenOwnerKind::Pivot)
        );
        assert_eq!(
            format_indented_paren_owner_kind("FROM src UNPIVOT INCLUDE NULLS ( -- include nulls"),
            Some(FormatIndentedParenOwnerKind::Unpivot)
        );
        assert_eq!(
            format_indented_paren_owner_header_kind("UNPIVOT EXCLUDE NULLS"),
            Some(FormatIndentedParenOwnerKind::Unpivot)
        );
    }

    #[test]
    fn format_window_and_match_recognize_subclause_helpers_cover_extended_body_headers() {
        assert!(FormatIndentedParenOwnerKind::Window
            .starts_body_header("ROWS BETWEEN UNBOUNDED PRECEDING AND CURRENT ROW"));
        assert!(FormatIndentedParenOwnerKind::Window
            .starts_body_header("RANGE BETWEEN 1 PRECEDING AND CURRENT ROW"));
        assert!(FormatIndentedParenOwnerKind::Window
            .starts_body_header("GROUPS BETWEEN 1 PRECEDING AND 1 FOLLOWING"));
        assert!(FormatIndentedParenOwnerKind::Window.starts_body_header("EXCLUDE CURRENT ROW"));
        assert!(starts_with_format_match_recognize_subclause(
            "WITH UNMATCHED ROWS"
        ));
        assert!(starts_with_format_match_recognize_subclause(
            "WITHOUT UNMATCHED ROWS"
        ));
        assert!(starts_with_format_match_recognize_subclause(
            "SHOW EMPTY MATCHES"
        ));
        assert!(starts_with_format_match_recognize_subclause(
            "OMIT EMPTY MATCHES"
        ));
        assert!(starts_with_format_match_recognize_subclause(
            "AFTER/* gap */MATCH/* gap */SKIP"
        ));
    }

    #[test]
    fn format_match_recognize_body_header_words_cover_clause_and_output_modifier_sequences() {
        assert!(
            FormatIndentedParenOwnerKind::MatchRecognize.starts_body_header_words(
                "PARTITION",
                Some("BY"),
                None,
            )
        );
        assert!(
            FormatIndentedParenOwnerKind::MatchRecognize.starts_body_header_words(
                "ORDER",
                Some("BY"),
                None,
            )
        );
        assert!(
            FormatIndentedParenOwnerKind::MatchRecognize.starts_body_header_words(
                "WITH",
                Some("UNMATCHED"),
                Some("ROWS"),
            )
        );
        assert!(
            FormatIndentedParenOwnerKind::MatchRecognize.starts_body_header_words(
                "AFTER",
                Some("MATCH"),
                Some("SKIP"),
            )
        );
        assert!(
            !FormatIndentedParenOwnerKind::MatchRecognize.starts_body_header_words(
                "ORDER",
                Some("SIBLINGS"),
                Some("BY"),
            )
        );
        assert!(
            !FormatIndentedParenOwnerKind::MatchRecognize.starts_phase1_body_header_words(
                "PARTITION",
                Some("BY"),
                None,
            )
        );
        assert!(
            !FormatIndentedParenOwnerKind::MatchRecognize.starts_phase1_body_header_words(
                "ORDER",
                Some("BY"),
                None,
            )
        );
        assert!(
            FormatIndentedParenOwnerKind::MatchRecognize.starts_phase1_body_header_words(
                "WITH",
                Some("UNMATCHED"),
                Some("ROWS"),
            )
        );
        assert!(
            FormatIndentedParenOwnerKind::MatchRecognize.starts_phase1_body_header_words(
                "AFTER",
                Some("MATCH"),
                Some("SKIP"),
            )
        );
    }

    #[test]
    fn format_model_subclause_helper_covers_extended_body_headers() {
        assert!(starts_with_format_model_subclause("UPDATE"));
        assert!(starts_with_format_model_subclause("UPSERT"));
        assert!(starts_with_format_model_subclause("IGNORE NAV"));
        assert!(starts_with_format_model_subclause("KEEP NAV"));
        assert!(starts_with_format_model_subclause("UPSERT ALL"));
        assert!(starts_with_format_model_subclause("AUTOMATIC ORDER"));
        assert!(starts_with_format_model_subclause("SEQUENTIAL ORDER"));
        assert!(starts_with_format_model_subclause("ITERATE (3)"));
        assert!(starts_with_format_model_subclause("UNTIL ("));
        assert!(starts_with_format_model_subclause("UNIQUE DIMENSION"));
        assert!(starts_with_format_model_subclause(
            "UNIQUE SINGLE REFERENCE"
        ));
        assert!(starts_with_format_model_subclause("RETURN ALL ROWS"));
        assert!(starts_with_format_model_subclause("RETURN UPDATED ROWS"));
        assert!(starts_with_format_model_subclause("PARTITION/* gap */BY"));
    }

    #[test]
    fn format_model_multiline_owner_tail_helper_covers_split_owner_header_tails() {
        assert!(starts_with_format_model_multiline_owner_tail("BY"));
        assert!(starts_with_format_model_multiline_owner_tail("ORDER"));
        assert!(starts_with_format_model_multiline_owner_tail("ALL"));
        assert!(!starts_with_format_model_multiline_owner_tail("ROWS"));
        assert!(!starts_with_format_model_multiline_owner_tail("ON"));
    }

    #[test]
    fn format_model_subclause_phase1_breaks_keep_rules_headers_but_not_rules_modifiers() {
        assert!(
            FormatIndentedParenOwnerKind::ModelSubclause.starts_phase1_body_header_words(
                "IGNORE",
                Some("NAV"),
                None,
            )
        );
        assert!(
            FormatIndentedParenOwnerKind::ModelSubclause.starts_phase1_body_header_words(
                "RETURN",
                Some("UPDATED"),
                Some("ROWS"),
            )
        );
        assert!(!FormatIndentedParenOwnerKind::ModelSubclause
            .starts_phase1_body_header_words("UPDATE", None, None,));
        assert!(
            !FormatIndentedParenOwnerKind::ModelSubclause.starts_phase1_body_header_words(
                "UPSERT",
                Some("ALL"),
                None,
            )
        );
        assert!(
            !FormatIndentedParenOwnerKind::ModelSubclause.starts_phase1_body_header_words(
                "SEQUENTIAL",
                Some("ORDER"),
                None,
            )
        );
        assert!(!FormatIndentedParenOwnerKind::ModelSubclause
            .starts_phase1_body_header_words("ITERATE", None, None,));
        assert!(!FormatIndentedParenOwnerKind::ModelSubclause
            .starts_phase1_body_header_words("UNTIL", None, None,));
    }

    #[test]
    fn format_pivot_unpivot_formatter_body_headers_treat_split_in_as_owner_relative_only_after_for()
    {
        assert_eq!(
            FormatIndentedParenOwnerKind::Pivot.formatter_body_header_depth(
                "IN (",
                Some("FOR deptno"),
                2,
            ),
            Some(3)
        );
        assert_eq!(
            FormatIndentedParenOwnerKind::Unpivot.formatter_body_header_depth(
                "IN (",
                Some("FOR dept_tag"),
                2,
            ),
            Some(3)
        );
        assert_eq!(
            FormatIndentedParenOwnerKind::Pivot.formatter_body_header_depth(
                "IN (",
                Some("WHEN deptno"),
                2,
            ),
            None
        );
    }

    #[test]
    fn format_owner_relative_body_header_depth_detects_split_continuations() {
        assert_eq!(
            FormatIndentedParenOwnerKind::Window.formatter_body_header_depth(
                "BY deptno",
                Some("PARTITION"),
                2,
            ),
            Some(3)
        );
        assert_eq!(
            FormatIndentedParenOwnerKind::AnalyticOver.formatter_body_header_depth(
                "UNBOUNDED PRECEDING",
                Some("ROWS"),
                2,
            ),
            Some(3)
        );
        assert_eq!(
            FormatIndentedParenOwnerKind::Window.formatter_body_header_depth(
                "CURRENT ROW",
                Some("EXCLUDE"),
                2,
            ),
            Some(3)
        );
        assert_eq!(
            FormatIndentedParenOwnerKind::Window.formatter_body_header_depth(
                "BETWEEN UNBOUNDED PRECEDING AND CURRENT ROW",
                Some("ROWS"),
                2,
            ),
            Some(3)
        );
        assert_eq!(
            FormatIndentedParenOwnerKind::MatchRecognize.formatter_body_header_depth(
                "TO NEXT ROW",
                Some("AFTER MATCH SKIP"),
                2,
            ),
            Some(3)
        );
        assert_eq!(
            FormatIndentedParenOwnerKind::MatchRecognize.formatter_body_header_depth(
                "MATCH",
                Some("ONE ROW PER"),
                2,
            ),
            Some(3)
        );
        assert_eq!(
            FormatIndentedParenOwnerKind::ModelSubclause.formatter_body_header_depth(
                "UPDATED ROWS",
                Some("RETURN"),
                2,
            ),
            Some(3)
        );
        assert_eq!(
            FormatIndentedParenOwnerKind::StructuredColumns.formatter_body_header_depth(
                "PATH '$.items[*]' COLUMNS (",
                Some("NESTED"),
                2,
            ),
            Some(3)
        );
        assert_eq!(
            FormatIndentedParenOwnerKind::StructuredColumns.formatter_body_header_depth(
                "COLUMNS (",
                Some("NESTED '$.items[*]'"),
                2,
            ),
            Some(3)
        );
        assert_eq!(
            FormatIndentedParenOwnerKind::WithinGroup.formatter_body_header_depth(
                "BY ename",
                Some("ORDER"),
                2,
            ),
            Some(3)
        );
        assert_eq!(
            FormatIndentedParenOwnerKind::Keep.formatter_body_header_depth(
                "LAST ORDER BY sal",
                Some("DENSE_RANK"),
                2,
            ),
            Some(3)
        );
    }

    #[test]
    fn format_body_header_line_state_tracks_multi_step_sequences_and_freeform_continuations() {
        let owner = FormatIndentedParenOwnerKind::ModelSubclause;
        let return_state = owner.body_header_line_state("RETURN", None);
        assert!(return_state.is_header);
        let updated_state = owner.body_header_line_state("UPDATED", return_state.next_state);
        assert!(updated_state.is_header);
        let rows_state = owner.body_header_line_state("ROWS", updated_state.next_state);
        assert!(rows_state.is_header);
        assert_eq!(rows_state.next_state, None);

        let owner = FormatIndentedParenOwnerKind::MatchRecognize;
        let one_state = owner.body_header_line_state("ONE", None);
        assert!(one_state.is_header);
        let row_per_state = owner.body_header_line_state("ROW PER", one_state.next_state);
        assert!(row_per_state.is_header);
        let match_state = owner.body_header_line_state("MATCH", row_per_state.next_state);
        assert!(match_state.is_header);
        assert_eq!(match_state.next_state, None);

        let after_state = owner.body_header_line_state("AFTER", None);
        assert!(after_state.is_header);
        let match_skip_state = owner.body_header_line_state("MATCH SKIP", after_state.next_state);
        assert!(match_skip_state.is_header);
        let to_state = owner.body_header_line_state("TO NEXT ROW", match_skip_state.next_state);
        assert!(to_state.is_header);
        assert_eq!(
            to_state.next_state,
            Some(FormatBodyHeaderContinuationState::Freeform)
        );

        let owner = FormatIndentedParenOwnerKind::Window;
        let rows_state = owner.body_header_line_state("ROWS", None);
        assert!(rows_state.is_header);
        assert_eq!(
            rows_state.next_state,
            Some(FormatBodyHeaderContinuationState::Freeform)
        );
        let between_state = owner.body_header_line_state("BETWEEN", rows_state.next_state);
        assert!(between_state.is_header);
        assert_eq!(
            between_state.next_state,
            Some(FormatBodyHeaderContinuationState::Freeform)
        );
        let bound_state = owner.body_header_line_state(
            "UNBOUNDED PRECEDING AND CURRENT ROW",
            between_state.next_state,
        );
        assert!(bound_state.is_header);
        assert_eq!(
            bound_state.next_state,
            Some(FormatBodyHeaderContinuationState::Freeform)
        );
        let order_state = owner.body_header_line_state("ORDER BY deptno", bound_state.next_state);
        assert!(order_state.is_header);
        assert_eq!(order_state.next_state, None);

        let owner = FormatIndentedParenOwnerKind::WithinGroup;
        let order_state = owner.body_header_line_state("ORDER", None);
        assert!(order_state.is_header);
        let by_state = owner.body_header_line_state("BY ename", order_state.next_state);
        assert!(by_state.is_header);
        assert_eq!(by_state.next_state, None);

        let owner = FormatIndentedParenOwnerKind::Keep;
        let dense_rank_state = owner.body_header_line_state("DENSE_RANK", None);
        assert!(dense_rank_state.is_header);
        let last_state = owner.body_header_line_state("LAST", dense_rank_state.next_state);
        assert!(last_state.is_header);
        let order_state = owner.body_header_line_state("ORDER", last_state.next_state);
        assert!(order_state.is_header);
        let by_state = owner.body_header_line_state("BY sal", order_state.next_state);
        assert!(by_state.is_header);
        assert_eq!(by_state.next_state, None);

        let dense_rank_state = owner.body_header_line_state("DENSE /* gap */ RANK", None);
        assert!(dense_rank_state.is_header);
        let last_state = owner.body_header_line_state("LAST", dense_rank_state.next_state);
        assert!(last_state.is_header);
        let order_state = owner.body_header_line_state("ORDER", last_state.next_state);
        assert!(order_state.is_header);
        let by_state = owner.body_header_line_state("BY sal", order_state.next_state);
        assert!(by_state.is_header);
        assert_eq!(by_state.next_state, None);
    }

    #[test]
    fn format_indented_paren_pending_header_kind_tracks_nested_path_columns_chains() {
        assert_eq!(
            format_indented_paren_pending_header_kind("WITHIN"),
            Some(PendingFormatIndentedParenOwnerHeaderKind::WithinGroup)
        );
        assert_eq!(format_indented_paren_pending_header_kind("WINDOW"), None);
        assert!(PendingFormatIndentedParenOwnerHeaderKind::WithinGroup.line_can_continue("GROUP"));
        assert!(PendingFormatIndentedParenOwnerHeaderKind::WithinGroup.line_completes("GROUP"));
        assert!(PendingFormatIndentedParenOwnerHeaderKind::WithinGroup
            .line_can_continue("GROUP (ORDER BY e.ename)"));
        assert!(PendingFormatIndentedParenOwnerHeaderKind::WithinGroup
            .line_completes("GROUP (ORDER BY e.ename)"));
        assert_eq!(
            format_indented_paren_pending_header_kind("NESTED"),
            Some(PendingFormatIndentedParenOwnerHeaderKind::NestedPathColumns)
        );
        assert_eq!(
            format_indented_paren_pending_header_kind("NESTED PATH '$.items[*]'"),
            Some(PendingFormatIndentedParenOwnerHeaderKind::NestedPathColumns)
        );
        assert_eq!(
            format_indented_paren_pending_header_kind("NESTED '$.items[*]'"),
            Some(PendingFormatIndentedParenOwnerHeaderKind::NestedPathColumns)
        );
        assert_eq!(
            format_indented_paren_pending_header_kind("NESTED TABLE"),
            None
        );
        assert!(PendingFormatIndentedParenOwnerHeaderKind::NestedPathColumns
            .line_can_continue("PATH '$.items[*]'"));
        assert!(PendingFormatIndentedParenOwnerHeaderKind::NestedPathColumns
            .line_can_continue("'$.items[*]'"));
        assert!(PendingFormatIndentedParenOwnerHeaderKind::NestedPathColumns
            .line_can_continue("COLUMNS"));
        assert!(
            PendingFormatIndentedParenOwnerHeaderKind::NestedPathColumns.line_completes("COLUMNS")
        );
        assert!(PendingFormatIndentedParenOwnerHeaderKind::WindowAs
            .line_can_continue("AS (PARTITION BY deptno)"));
        assert!(PendingFormatIndentedParenOwnerHeaderKind::WindowAs
            .line_completes("AS (PARTITION BY deptno)"));
        assert!(PendingFormatIndentedParenOwnerHeaderKind::NestedPathColumns
            .line_can_continue("COLUMNS (deptno PATH '$.deptno')"));
        assert!(PendingFormatIndentedParenOwnerHeaderKind::NestedPathColumns
            .line_completes("COLUMNS (deptno PATH '$.deptno')"));
    }

    #[test]
    fn pending_indented_paren_owner_headers_stop_on_other_structural_owner_boundaries() {
        let within_group = PendingFormatIndentedParenOwnerHeaderKind::WithinGroup;
        let window_as = PendingFormatIndentedParenOwnerHeaderKind::WindowAs;
        let nested_columns = PendingFormatIndentedParenOwnerHeaderKind::NestedPathColumns;

        assert!(!within_group.line_can_continue("LATERAL"));
        assert!(!within_group.line_can_continue("CURSOR"));
        assert!(!window_as.line_can_continue("REFERENCE ref_limits"));
        assert!(!window_as.line_can_continue("OPEN c_emp"));
        assert!(!nested_columns.line_can_continue("LEFT OUTER"));
        assert!(!nested_columns.line_can_continue("BEGIN"));
    }

    #[test]
    fn format_indented_paren_owner_header_kind_covers_split_owner_heads() {
        assert_eq!(
            format_indented_paren_owner_header_kind("SUM (sal) OVER"),
            Some(FormatIndentedParenOwnerKind::AnalyticOver)
        );
        assert_eq!(
            format_indented_paren_owner_header_kind("WITHIN GROUP"),
            Some(FormatIndentedParenOwnerKind::WithinGroup)
        );
        assert_eq!(
            format_indented_paren_owner_header_kind("KEEP"),
            Some(FormatIndentedParenOwnerKind::Keep)
        );
        assert_eq!(
            format_indented_paren_owner_header_kind("WINDOW w_dept AS"),
            Some(FormatIndentedParenOwnerKind::Window)
        );
        assert_eq!(
            format_indented_paren_owner_header_kind("MATCH_RECOGNIZE"),
            Some(FormatIndentedParenOwnerKind::MatchRecognize)
        );
        assert_eq!(
            format_indented_paren_owner_header_kind("FROM src PIVOT"),
            Some(FormatIndentedParenOwnerKind::Pivot)
        );
        assert_eq!(
            format_indented_paren_owner_header_kind("RULES UPDATE"),
            Some(FormatIndentedParenOwnerKind::ModelSubclause)
        );
        assert_eq!(
            format_indented_paren_owner_header_kind("NESTED PATH '$.items[*]' COLUMNS"),
            Some(FormatIndentedParenOwnerKind::StructuredColumns)
        );
        assert_eq!(
            format_indented_paren_owner_header_kind("NESTED '$.items[*]' COLUMNS"),
            Some(FormatIndentedParenOwnerKind::StructuredColumns)
        );
        assert_eq!(
            format_indented_paren_owner_header_kind(
                "SELECT LISTAGG (xr.flag_txt, ', ') WITHIN GROUP (ORDER BY xr.flag_txt)"
            ),
            None
        );
    }

    #[test]
    fn format_indented_paren_owner_kind_from_words_covers_hot_path_owner_heads() {
        assert_eq!(
            format_indented_paren_owner_kind_from_words(&["SUM", "OVER"]),
            Some(FormatIndentedParenOwnerKind::AnalyticOver)
        );
        assert_eq!(
            format_indented_paren_owner_kind_from_words(&["WITHIN", "GROUP"]),
            Some(FormatIndentedParenOwnerKind::WithinGroup)
        );
        assert_eq!(
            format_indented_paren_owner_kind_from_words(&["KEEP"]),
            Some(FormatIndentedParenOwnerKind::Keep)
        );
        assert_eq!(
            format_indented_paren_owner_kind_from_words(&["WINDOW", "w_dept", "AS"]),
            Some(FormatIndentedParenOwnerKind::Window)
        );
        assert_eq!(
            format_indented_paren_owner_kind_from_words(&["MATCH_RECOGNIZE"]),
            Some(FormatIndentedParenOwnerKind::MatchRecognize)
        );
        assert_eq!(
            format_indented_paren_owner_kind_from_words(&["FROM", "src", "PIVOT"]),
            Some(FormatIndentedParenOwnerKind::Pivot)
        );
        assert_eq!(
            format_indented_paren_owner_kind_from_words(&["RULES", "UPDATE"]),
            Some(FormatIndentedParenOwnerKind::ModelSubclause)
        );
        assert_eq!(
            format_indented_paren_owner_kind_from_words(&["NESTED", "PATH", "COLUMNS"]),
            Some(FormatIndentedParenOwnerKind::StructuredColumns)
        );
        assert_eq!(
            format_indented_paren_owner_kind_from_words(&["deptno"]),
            None
        );
    }

    #[test]
    fn format_query_owner_kind_covers_nested_query_owner_heads() {
        assert_eq!(
            format_query_owner_kind("FROM ("),
            Some(FormatQueryOwnerKind::Clause)
        );
        assert_eq!(
            format_query_owner_kind("LEFT OUTER JOIN ("),
            Some(FormatQueryOwnerKind::Clause)
        );
        assert_eq!(
            format_query_owner_kind("USING ("),
            Some(FormatQueryOwnerKind::Clause)
        );
        assert_eq!(
            format_query_owner_kind("LATERAL ("),
            Some(FormatQueryOwnerKind::FromItem)
        );
        assert_eq!(
            format_query_owner_kind("CROSS APPLY ("),
            Some(FormatQueryOwnerKind::FromItem)
        );
        assert_eq!(
            format_query_owner_kind("OUTER APPLY ("),
            Some(FormatQueryOwnerKind::FromItem)
        );
        assert_eq!(
            format_query_owner_kind("FROM TABLE ("),
            Some(FormatQueryOwnerKind::FromItem)
        );
        assert_eq!(
            format_query_owner_kind("LEFT JOIN TABLE ("),
            Some(FormatQueryOwnerKind::FromItem)
        );
        assert_eq!(
            format_query_owner_kind("MERGE INTO dst d USING ("),
            Some(FormatQueryOwnerKind::Clause)
        );
        assert_eq!(
            format_query_owner_kind("REFERENCE ref_limits ON ("),
            Some(FormatQueryOwnerKind::Clause)
        );
        assert_eq!(
            format_query_owner_kind("WHERE col IN ("),
            Some(FormatQueryOwnerKind::Condition)
        );
        assert_eq!(
            format_query_owner_kind("WHERE EXISTS ("),
            Some(FormatQueryOwnerKind::Condition)
        );
        assert_eq!(
            format_query_owner_kind("WHERE NOT EXISTS ("),
            Some(FormatQueryOwnerKind::Condition)
        );
        assert_eq!(
            format_query_owner_kind("WHERE score = ANY ("),
            Some(FormatQueryOwnerKind::Condition)
        );
        assert_eq!(
            format_query_owner_kind("WHERE score < SOME ("),
            Some(FormatQueryOwnerKind::Condition)
        );
        assert_eq!(
            format_query_owner_kind("WHERE score > ALL ("),
            Some(FormatQueryOwnerKind::Condition)
        );
        assert_eq!(format_query_owner_kind("FOR rec IN ("), None);
        assert_eq!(format_query_owner_kind("FOR qtr IN ("), None);
    }

    #[test]
    fn format_query_owner_header_kind_covers_split_owner_heads() {
        assert_eq!(
            format_query_owner_header_kind("FROM"),
            Some(FormatQueryOwnerKind::Clause)
        );
        assert_eq!(
            format_query_owner_header_kind("LEFT OUTER JOIN"),
            Some(FormatQueryOwnerKind::Clause)
        );
        assert_eq!(
            format_query_owner_header_kind("CROSS APPLY"),
            Some(FormatQueryOwnerKind::FromItem)
        );
        assert_eq!(
            format_query_owner_header_kind("FROM TABLE"),
            Some(FormatQueryOwnerKind::FromItem)
        );
        assert_eq!(
            format_query_owner_header_kind("LEFT JOIN TABLE"),
            Some(FormatQueryOwnerKind::FromItem)
        );
        assert_eq!(
            format_query_owner_header_kind("REFERENCE ref_limits ON"),
            Some(FormatQueryOwnerKind::Clause)
        );
        assert_eq!(
            format_query_owner_header_kind("WHERE EXISTS"),
            Some(FormatQueryOwnerKind::Condition)
        );
        assert_eq!(
            format_query_owner_header_kind("WHERE score < SOME"),
            Some(FormatQueryOwnerKind::Condition)
        );
        assert_eq!(
            format_query_owner_header_kind("CREATE MATERIALIZED VIEW mv_demo AS"),
            Some(FormatQueryOwnerKind::DdlBody)
        );
        assert_eq!(format_query_owner_header_kind("INSERT ALL"), None);
        assert_eq!(format_query_owner_header_kind("UPSERT ALL"), None);
        assert_eq!(format_query_owner_header_kind("RETURN ALL ROWS"), None);
        assert_eq!(format_query_owner_header_kind("ALL ROWS PER MATCH"), None);
        assert_eq!(format_query_owner_header_kind("FOR rec IN"), None);
        assert_eq!(format_query_owner_header_kind("UNION ALL"), None);
        assert_eq!(format_query_owner_header_kind("CREATE TABLE"), None);
    }

    #[test]
    fn owner_and_pending_header_helpers_consume_leading_close_before_classifying() {
        assert_eq!(
            format_query_owner_header_kind(") REFERENCE ref_limits ON"),
            Some(FormatQueryOwnerKind::Clause)
        );
        assert_eq!(
            format_query_owner_pending_header_kind(") LEFT OUTER"),
            Some(PendingFormatQueryOwnerHeaderKind::JoinLike)
        );
        assert!(PendingFormatQueryOwnerHeaderKind::JoinLike.line_completes(") JOIN"));
        assert_eq!(
            format_indented_paren_owner_header_kind(") WINDOW w_sales AS"),
            Some(FormatIndentedParenOwnerKind::Window)
        );
        assert_eq!(
            format_indented_paren_pending_header_kind(") WITHIN"),
            Some(PendingFormatIndentedParenOwnerHeaderKind::WithinGroup)
        );
        assert!(PendingFormatIndentedParenOwnerHeaderKind::WithinGroup.line_completes(") GROUP"));
        assert_eq!(
            format_plsql_child_query_owner_pending_header_kind(") OPEN c_emp"),
            Some(PendingFormatPlsqlChildQueryOwnerHeaderKind::OpenCursorFor)
        );
        assert!(PendingFormatPlsqlChildQueryOwnerHeaderKind::OpenCursorFor.line_completes(") FOR"));
        assert!(
            PendingFormatPlsqlChildQueryOwnerHeaderKind::CursorDeclaration.line_can_continue(")")
        );
    }

    #[test]
    fn format_query_owner_pending_header_kind_tracks_split_join_and_apply_modifier_chains() {
        let natural_pending =
            format_query_owner_pending_header_kind("NATURAL").expect("pending NATURAL owner");
        assert!(natural_pending.line_can_continue("LEFT"));
        assert!(!natural_pending.line_completes("LEFT"));

        let join_pending =
            format_query_owner_pending_header_kind("LEFT OUTER").expect("pending LEFT OUTER owner");
        assert!(join_pending.line_can_continue("JOIN"));
        assert!(join_pending.line_completes("JOIN"));

        let apply_pending =
            format_query_owner_pending_header_kind("CROSS").expect("pending CROSS owner");
        assert!(apply_pending.line_can_continue("APPLY"));
        assert!(apply_pending.line_completes("APPLY"));
    }

    #[test]
    fn format_query_owner_pending_header_kind_tracks_split_not_condition_owner_chain() {
        let not_pending = format_query_owner_pending_header_kind("NOT").expect("pending NOT owner");

        assert!(not_pending.line_can_continue("EXISTS"));
        assert!(not_pending.line_can_continue("IN"));
        assert!(not_pending.line_completes("EXISTS"));
        assert!(not_pending.line_completes("IN"));
        assert_eq!(
            not_pending.owner_kind_for_line("EXISTS"),
            Some(FormatQueryOwnerKind::Condition)
        );
        assert_eq!(
            not_pending.owner_kind_for_line("IN"),
            Some(FormatQueryOwnerKind::Condition)
        );
        assert!(!not_pending.line_can_continue("SELECT"));
    }

    #[test]
    fn format_query_owner_pending_header_kind_tracks_split_ddl_query_body_headers() {
        let ddl_pending = format_query_owner_pending_header_kind(
            "CREATE MATERIALIZED VIEW mv_sales_dashboard BUILD DEFERRED REFRESH FAST",
        )
        .expect("pending CREATE MATERIALIZED VIEW owner");

        assert!(ddl_pending.line_can_continue("BUILD IMMEDIATE"));
        assert!(ddl_pending.line_can_continue("ON DEMAND ENABLE QUERY REWRITE AS"));
        assert!(ddl_pending.line_completes("ON DEMAND ENABLE QUERY REWRITE AS"));
        assert_eq!(
            ddl_pending.owner_kind_for_line("ON DEMAND ENABLE QUERY REWRITE AS"),
            Some(FormatQueryOwnerKind::DdlBody)
        );
        assert!(!ddl_pending.line_can_continue("WITH date_dim AS ("));
        assert!(!ddl_pending.line_can_continue("SELECT * FROM dual"));
        assert!(!ddl_pending.line_can_continue("ALTER MATERIALIZED VIEW mv_sales_dashboard"));
    }

    #[test]
    fn ddl_query_body_header_detects_view_ctas_and_alter_view_headers() {
        assert!(line_is_ddl_query_body_header(
            "CREATE OR REPLACE VIEW v_demo AS"
        ));
        assert!(line_is_ddl_query_body_header(
            "CREATE/* gap */OR/* gap */REPLACE/* gap */VIEW \"v_demo\" AS"
        ));
        assert!(line_is_ddl_query_body_header(
            "CREATE OR REPLACE ALGORITHM = MERGE DEFINER = root SQL SECURITY INVOKER VIEW v_demo AS"
        ));
        assert!(line_is_ddl_query_body_header(
            "ALTER ALGORITHM = UNDEFINED VIEW v_demo AS"
        ));
        assert!(line_is_ddl_query_body_header(
            "CREATE MATERIALIZED VIEW mv_demo AS"
        ));
        assert!(line_is_ddl_query_body_header(
            "CREATE OR REPLACE JSON RELATIONAL DUALITY VIEW dv_demo AS"
        ));
        assert!(line_is_ddl_query_body_header("CREATE TABLE t_demo AS"));
        assert!(line_is_ddl_query_body_header(
            "CREATE TEMPORARY TABLE t_demo AS"
        ));
        assert!(line_is_ddl_query_body_header(
            "CREATE GLOBAL TEMPORARY TABLE t_demo AS"
        ));
        assert!(line_is_ddl_query_body_header(
            "CREATE/* gap */GLOBAL/* gap */TEMPORARY/* gap */TABLE \"t demo\" AS"
        ));
        assert!(line_is_ddl_query_body_header(
            "CREATE PRIVATE TEMPORARY TABLE ora$ptt_demo AS"
        ));
        assert!(!line_is_ddl_query_body_header(
            "CREATE TABLE t_demo (id NUMBER)"
        ));
        assert!(!line_is_ddl_query_body_header("CREATE PACKAGE pkg_demo AS"));
        assert!(!line_is_ddl_query_body_header("ALTER TABLE v_demo AS"));
    }

    #[test]
    fn cte_definition_header_helper_tracks_column_lists_and_comment_glued_headers() {
        assert!(line_is_format_cte_definition_header("WITH base_emp AS ("));
        assert!(line_is_format_cte_definition_header("base_emp AS ("));
        assert!(line_is_format_cte_definition_header("b (a, b, c) AS ("));
        assert!(line_is_format_cte_definition_header(
            "/* owner */ b (a, b, c) AS ("
        ));
        assert!(line_is_format_cte_definition_header(") b (a, b, c) AS ("));
        assert!(!line_is_format_cte_definition_header(
            "SELECT empno AS salary"
        ));
        assert!(!line_is_format_cte_definition_header("b (a, b, c)"));
    }

    #[test]
    fn window_definition_header_helper_tracks_named_window_items_only() {
        assert!(line_is_format_bare_window_clause_header("WINDOW"));
        assert!(line_is_format_bare_window_clause_header("WINDOW -- keep"));
        assert!(!line_is_format_bare_window_clause_header(
            "WINDOW w_emp AS ("
        ));
        assert!(line_is_format_window_definition_header("w_emp AS ("));
        assert!(line_is_format_window_definition_header(
            "/* owner */ w_emp_running AS ("
        ));
        assert!(!line_is_format_window_definition_header(
            "WINDOW w_emp AS ("
        ));
        assert!(!line_is_format_window_definition_header(
            "WITH base_emp AS ("
        ));
        assert!(!line_is_format_window_definition_header("w_emp AS"));
        assert!(line_is_mysql_on_duplicate_key_update_clause(
            "ON DUPLICATE KEY UPDATE dept_name = VALUES(dept_name)"
        ));
        assert!(!line_is_mysql_on_duplicate_key_update_clause(
            "ON status = 'DUPLICATE KEY UPDATE'"
        ));
    }

    #[test]
    fn split_query_owner_lookahead_kind_detects_safe_split_owner_headers() {
        assert_eq!(
            split_query_owner_lookahead_kind("CURSOR", true, Some("SELECT empno")),
            Some(SplitQueryOwnerLookaheadKind::GenericExpression)
        );
        assert_eq!(
            split_query_owner_lookahead_kind("MULTISET", true, Some("WITH bonus_cte AS")),
            Some(SplitQueryOwnerLookaheadKind::GenericExpression)
        );
        assert_eq!(
            split_query_owner_lookahead_kind("LATERAL", true, Some("SELECT empno")),
            Some(SplitQueryOwnerLookaheadKind::DirectFromItem)
        );
        assert_eq!(
            split_query_owner_lookahead_kind("FROM TABLE", true, Some("SELECT empno")),
            Some(SplitQueryOwnerLookaheadKind::DirectFromItem)
        );
        assert_eq!(
            split_query_owner_lookahead_kind("ORDER BY", true, Some("SELECT empno")),
            None
        );
        assert_eq!(
            split_query_owner_lookahead_kind("CURSOR", false, Some("SELECT empno")),
            None
        );
    }

    #[test]
    fn pending_reference_owner_header_stops_on_other_structural_owner_boundaries() {
        let reference_pending = PendingFormatQueryOwnerHeaderKind::ReferenceOn;

        assert!(!reference_pending.line_can_continue("LATERAL"));
        assert!(!reference_pending.line_can_continue("CURSOR"));
        assert!(!reference_pending.line_can_continue("BEGIN"));
        assert!(!reference_pending.line_can_continue("OPEN c_emp"));
    }

    #[test]
    fn format_expression_query_owner_keywords_cover_safe_split_expression_wrappers() {
        assert!(line_ends_with_format_expression_query_owner_keyword(
            "CURSOR"
        ));
        assert!(line_ends_with_format_expression_query_owner_keyword(
            "MULTISET -- wrapper comment"
        ));
        assert!(!line_ends_with_format_expression_query_owner_keyword(
            "ORDER BY"
        ));
        assert!(!line_ends_with_format_expression_query_owner_keyword(
            "SELECT e.empno,"
        ));
    }

    #[test]
    fn line_starts_with_format_bare_direct_from_item_query_owner_ignores_leading_comments() {
        assert!(line_starts_with_format_bare_direct_from_item_query_owner(
            "LATERAL"
        ));
        assert!(line_starts_with_format_bare_direct_from_item_query_owner(
            "TABLE"
        ));
        assert!(line_starts_with_format_bare_direct_from_item_query_owner(
            "/* owner */ LATERAL"
        ));
        assert!(line_starts_with_format_bare_direct_from_item_query_owner(
            "/* owner */ TABLE"
        ));
        assert!(!line_starts_with_format_bare_direct_from_item_query_owner(
            "FROM TABLE"
        ));
        assert!(!line_starts_with_format_bare_direct_from_item_query_owner(
            "CROSS APPLY"
        ));
    }

    #[test]
    fn auto_format_owner_boundary_helpers_share_owner_taxonomy_but_can_exclude_expression_rhs() {
        assert!(starts_with_auto_format_owner_boundary("WHERE EXISTS"));
        assert!(starts_with_auto_format_owner_boundary("WINDOW w_dept AS"));
        assert!(starts_with_auto_format_owner_boundary("MULTISET"));
        assert!(starts_with_auto_format_owner_boundary("CURSOR"));

        assert!(starts_with_auto_format_owner_boundary_without_expression_owner("WHERE EXISTS"));
        assert!(
            starts_with_auto_format_owner_boundary_without_expression_owner("WINDOW w_dept AS")
        );
        assert!(!starts_with_auto_format_owner_boundary_without_expression_owner("MULTISET"));
        assert!(starts_with_auto_format_owner_boundary_without_expression_owner("CURSOR"));
    }

    #[test]
    fn structural_continuation_boundary_helper_tracks_join_condition_and_for_update() {
        assert!(
            starts_with_auto_format_structural_continuation_boundary_without_expression_owner(
                "LEFT OUTER JOIN dept d"
            )
        );
        assert!(
            starts_with_auto_format_structural_continuation_boundary_without_expression_owner(
                "ON d.deptno = e.deptno"
            )
        );
        assert!(
            starts_with_auto_format_structural_continuation_boundary_without_expression_owner(
                "FOR UPDATE OF e.sal"
            )
        );
        assert!(
            starts_with_auto_format_structural_continuation_boundary_without_expression_owner(
                "CALL pkg_do_work()"
            )
        );
        assert!(
            !starts_with_auto_format_structural_continuation_boundary_without_expression_owner(
                "MULTISET"
            )
        );
    }

    #[test]
    fn mixed_leading_close_continuation_recognizes_join_condition_boundary() {
        assert!(line_has_mixed_leading_close_continuation(
            ") ON d.deptno = e.deptno"
        ));
        assert!(line_has_mixed_leading_close_continuation(") JOIN dept d"));
    }

    #[test]
    fn format_split_direct_from_item_query_owner_keywords_cover_safe_split_lateral_headers() {
        assert!(line_ends_with_format_split_direct_from_item_query_owner_keyword("LATERAL"));
        assert!(
            line_ends_with_format_split_direct_from_item_query_owner_keyword(
                "LATERAL -- derived rows"
            )
        );
        assert!(line_ends_with_format_split_direct_from_item_query_owner_keyword("TABLE"));
        assert!(
            line_ends_with_format_split_direct_from_item_query_owner_keyword(
                "TABLE -- derived rows"
            )
        );
        assert!(line_ends_with_format_split_direct_from_item_query_owner_keyword("FROM TABLE"));
        assert!(
            line_ends_with_format_split_direct_from_item_query_owner_keyword("LEFT JOIN TABLE")
        );
        assert!(!line_ends_with_format_split_direct_from_item_query_owner_keyword("OUTER APPLY"));
        assert!(
            !line_ends_with_format_split_direct_from_item_query_owner_keyword(
                "TABLE collection_expr"
            )
        );
        assert!(!line_ends_with_format_split_direct_from_item_query_owner_keyword("CREATE TABLE"));
        assert!(!line_ends_with_format_split_direct_from_item_query_owner_keyword("JOIN"));
    }

    #[test]
    fn format_plsql_child_query_owner_kind_covers_nested_control_and_query_owners() {
        assert_eq!(
            mysql_declare_owner_kind("DECLARE cur_emp CURSOR FOR"),
            Some(MySqlDeclareOwnerKind::CursorFor)
        );
        assert_eq!(
            mysql_declare_owner_kind("DECLARE CONTINUE HANDLER FOR NOT FOUND"),
            Some(MySqlDeclareOwnerKind::HandlerFor)
        );
        assert_eq!(
            mysql_declare_owner_kind("DECLARE EXIT HANDLER FOR SQLEXCEPTION"),
            Some(MySqlDeclareOwnerKind::HandlerFor)
        );
        assert_eq!(mysql_declare_owner_kind("DECLARE v_done INT"), None);
        assert_eq!(
            format_plsql_child_query_owner_kind("BEGIN"),
            Some(FormatPlsqlChildQueryOwnerKind::ControlBody)
        );
        assert_eq!(
            format_plsql_child_query_owner_kind("EXCEPTION"),
            Some(FormatPlsqlChildQueryOwnerKind::ControlBody)
        );
        assert_eq!(
            format_plsql_child_query_owner_kind("ELSE"),
            Some(FormatPlsqlChildQueryOwnerKind::ControlBody)
        );
        assert_eq!(
            format_plsql_child_query_owner_kind("ELSIF v_ready THEN"),
            Some(FormatPlsqlChildQueryOwnerKind::ControlBody)
        );
        assert_eq!(
            format_plsql_child_query_owner_kind("ELSEIF v_ready THEN"),
            Some(FormatPlsqlChildQueryOwnerKind::ControlBody)
        );
        assert_eq!(format_plsql_child_query_owner_kind("ELSIF v_ready"), None);
        assert_eq!(
            format_plsql_child_query_owner_kind("ELSEIF /* gap */ v_ready"),
            None
        );
        assert_eq!(
            format_plsql_child_query_owner_kind("ELSIF v_ready THEN OPEN c_emp FOR"),
            Some(FormatPlsqlChildQueryOwnerKind::ControlBody)
        );
        assert_eq!(
            format_plsql_child_query_owner_kind("CURSOR c_emp IS"),
            Some(FormatPlsqlChildQueryOwnerKind::CursorDeclaration)
        );
        assert_eq!(
            format_plsql_child_query_owner_kind("CURSOR c_emp (p_deptno NUMBER) AS"),
            Some(FormatPlsqlChildQueryOwnerKind::CursorDeclaration)
        );
        assert_eq!(
            format_plsql_child_query_owner_kind("CURSOR/* gap */\"c emp\"/* gap */IS"),
            Some(FormatPlsqlChildQueryOwnerKind::CursorDeclaration)
        );
        assert_eq!(
            format_plsql_child_query_owner_kind("OPEN c_emp FOR"),
            Some(FormatPlsqlChildQueryOwnerKind::OpenCursorFor)
        );
        assert_eq!(
            format_plsql_child_query_owner_kind("OPEN c_emp FOR SELECT empno FROM emp"),
            Some(FormatPlsqlChildQueryOwnerKind::OpenCursorFor)
        );
        assert_eq!(
            format_plsql_child_query_owner_kind("OPEN/* gap */c_emp/* gap */FOR"),
            Some(FormatPlsqlChildQueryOwnerKind::OpenCursorFor)
        );
        assert_eq!(
            format_plsql_child_query_owner_kind("DECLARE cur_emp CURSOR FOR"),
            Some(FormatPlsqlChildQueryOwnerKind::OpenCursorFor)
        );
        assert_eq!(
            format_plsql_child_query_owner_kind("DECLARE CONTINUE HANDLER FOR NOT FOUND"),
            Some(FormatPlsqlChildQueryOwnerKind::ControlBody)
        );
        assert_eq!(
            format_plsql_child_query_owner_kind("DECLARE EXIT HANDLER FOR SQLEXCEPTION"),
            Some(FormatPlsqlChildQueryOwnerKind::ControlBody)
        );
        assert_eq!(
            format_plsql_child_query_owner_kind(") CURSOR c_emp IS"),
            Some(FormatPlsqlChildQueryOwnerKind::CursorDeclaration)
        );
        assert_eq!(
            format_plsql_child_query_owner_kind(") OPEN c_emp FOR"),
            Some(FormatPlsqlChildQueryOwnerKind::OpenCursorFor)
        );
        assert_eq!(format_plsql_child_query_owner_kind("LOOP"), None);
        assert_eq!(format_plsql_child_query_owner_kind("END IF;"), None);
        assert_eq!(format_plsql_child_query_owner_kind("FOR rec IN ("), None);
    }

    #[test]
    fn mysql_labeled_block_target_before_inline_comment_recognizes_begin_and_loop_targets() {
        assert_eq!(
            mysql_labeled_block_target_before_inline_comment("main_block: BEGIN"),
            Some("BEGIN")
        );
        assert_eq!(
            mysql_labeled_block_target_before_inline_comment("read_loop /* gap */ : LOOP"),
            Some("LOOP")
        );
        assert_eq!(
            mysql_labeled_block_target_before_inline_comment("v_label: SELECT"),
            None
        );
        assert_eq!(
            mysql_labeled_block_target_before_inline_comment("/* note */ main_block: WHILE"),
            Some("WHILE")
        );
    }

    #[test]
    fn line_starts_mysql_block_keyword_before_inline_comment_keeps_labeled_begin_in_shared_family()
    {
        assert!(line_starts_mysql_block_keyword_before_inline_comment(
            "main_block: BEGIN",
            "BEGIN",
        ));
        assert!(line_starts_mysql_block_keyword_before_inline_comment(
            "BEGIN", "BEGIN",
        ));
        assert!(line_starts_mysql_block_keyword_before_inline_comment(
            "read_loop: LOOP",
            "LOOP",
        ));
        assert!(!line_starts_mysql_block_keyword_before_inline_comment(
            "main_block: SET",
            "BEGIN",
        ));
    }

    #[test]
    fn format_plsql_child_query_owner_pending_header_kind_tracks_split_cursor_and_open_headers() {
        assert_eq!(
            format_plsql_child_query_owner_pending_header_kind("CURSOR c_emp"),
            Some(PendingFormatPlsqlChildQueryOwnerHeaderKind::CursorDeclaration)
        );
        assert_eq!(
            format_plsql_child_query_owner_pending_header_kind("/* gap */CURSOR c_emp"),
            Some(PendingFormatPlsqlChildQueryOwnerHeaderKind::CursorDeclaration)
        );
        assert_eq!(
            format_plsql_child_query_owner_pending_header_kind("OPEN c_emp"),
            Some(PendingFormatPlsqlChildQueryOwnerHeaderKind::OpenCursorFor)
        );
        assert_eq!(
            format_plsql_child_query_owner_pending_header_kind("/* gap */OPEN c_emp"),
            Some(PendingFormatPlsqlChildQueryOwnerHeaderKind::OpenCursorFor)
        );
        assert_eq!(
            format_plsql_child_query_owner_pending_header_kind(") CURSOR c_emp"),
            Some(PendingFormatPlsqlChildQueryOwnerHeaderKind::CursorDeclaration)
        );
        assert_eq!(
            format_plsql_child_query_owner_pending_header_kind(") OPEN c_emp"),
            Some(PendingFormatPlsqlChildQueryOwnerHeaderKind::OpenCursorFor)
        );
        assert_eq!(
            format_plsql_child_query_owner_pending_header_kind("CURSOR c_emp IS"),
            None
        );
        assert_eq!(
            format_plsql_child_query_owner_pending_header_kind("OPEN c_emp FOR"),
            None
        );
        assert_eq!(
            format_plsql_child_query_owner_pending_header_kind("CURSOR c_emp; -- keep"),
            None
        );
        assert_eq!(
            format_plsql_child_query_owner_pending_header_kind("OPEN c_emp; -- keep"),
            None
        );
    }

    #[test]
    fn pending_plsql_child_query_owner_header_kind_recognizes_split_completion_lines() {
        let cursor_kind = PendingFormatPlsqlChildQueryOwnerHeaderKind::CursorDeclaration;
        assert!(cursor_kind.line_completes("IS"));
        assert!(cursor_kind.line_completes("/* gap */IS"));
        assert!(cursor_kind.line_completes(") AS"));
        assert!(cursor_kind.line_can_continue("(p_deptno NUMBER,"));
        assert!(cursor_kind.line_can_continue("p_ename VARCHAR2(30)"));
        assert!(cursor_kind.line_can_continue(")"));
        assert!(cursor_kind.line_can_continue("/* gap */ ) /* wrapper */"));
        assert!(cursor_kind.line_can_continue(");"));
        assert!(!cursor_kind.line_can_continue("c_emp; -- already terminated"));
        assert!(!cursor_kind.line_can_continue("SELECT empno"));

        let open_kind = PendingFormatPlsqlChildQueryOwnerHeaderKind::OpenCursorFor;
        assert!(open_kind.line_completes("FOR"));
        assert!(open_kind.line_completes("/* gap */FOR"));
        assert!(open_kind.line_completes("FOR /* owner */"));
        assert!(open_kind.line_can_continue("c_emp"));
        assert!(open_kind.line_can_continue(");"));
        assert!(!open_kind.line_can_continue("c_emp; -- already terminated"));
        assert!(!open_kind.line_can_continue("SELECT empno"));
    }

    #[test]
    fn format_query_owner_kind_depth_rules_keep_nested_heads_relative_to_owner_context() {
        assert_eq!(
            FormatQueryOwnerKind::Clause.header_depth_floor(Some(2), None),
            Some(2)
        );
        assert_eq!(
            FormatQueryOwnerKind::FromItem.header_depth_floor(Some(3), None),
            Some(3)
        );
        assert_eq!(
            FormatQueryOwnerKind::Condition.header_depth_floor(Some(2), None),
            Some(3)
        );
        assert_eq!(
            FormatQueryOwnerKind::Condition.header_depth_floor(Some(2), Some(5)),
            Some(5)
        );
        assert_eq!(
            FormatQueryOwnerKind::Operator.header_depth_floor(Some(2), None),
            None
        );
        assert_eq!(
            FormatQueryOwnerKind::DdlBody.header_depth_floor(Some(2), None),
            None
        );
        assert_eq!(
            FormatQueryOwnerKind::Clause.auto_format_child_query_owner_base_depth(2, Some(2)),
            2
        );
        assert_eq!(
            FormatQueryOwnerKind::FromItem.auto_format_child_query_owner_base_depth(3, Some(2)),
            3
        );
        assert_eq!(
            FormatQueryOwnerKind::Condition.auto_format_child_query_owner_base_depth(2, Some(2)),
            3
        );
        assert_eq!(
            FormatQueryOwnerKind::Condition.auto_format_child_query_owner_base_depth(3, Some(2)),
            3
        );
        assert_eq!(
            FormatQueryOwnerKind::Operator.auto_format_child_query_owner_base_depth(4, Some(2)),
            4
        );
        assert_eq!(
            FormatQueryOwnerKind::DdlBody.auto_format_child_query_owner_base_depth(3, None),
            3
        );
    }

    #[test]
    fn contextual_format_query_owner_kind_recognizes_condition_operator_scalar_subqueries() {
        assert_eq!(
            contextual_format_query_owner_kind("AND e.salary > (", true),
            Some(FormatQueryOwnerKind::Operator)
        );
        assert_eq!(
            contextual_format_query_owner_kind("AND e.salary >", true),
            Some(FormatQueryOwnerKind::Operator)
        );
        assert_eq!(
            contextual_format_query_owner_kind("AND e.salary > (", false),
            None
        );
        assert_eq!(
            contextual_format_query_owner_kind("SUBSET ab = (", false),
            None
        );
    }

    #[test]
    fn shared_keyword_pool_includes_oracle_trigger_and_edition_keywords() {
        for keyword in [
            "BREADTH",
            "COMPOUND",
            "CYCLE",
            "DEPTH",
            "DO",
            "FOLLOWS",
            "INSTEAD",
            "JSON",
            "JSON_ARRAY",
            "JSON_ARRAYAGG",
            "JSON_EXISTS",
            "JSON_OBJECT",
            "JSON_OBJECTAGG",
            "JSON_QUERY",
            "JSON_TABLE",
            "JSON_VALUE",
            "NOFORCE",
            "NONEDITIONING",
            "PRECEDES",
            "REFERENCING",
        ] {
            assert!(
                is_oracle_sql_keyword(keyword),
                "missing shared keyword: {keyword}"
            );
        }
    }

    #[test]
    fn shared_keyword_pool_includes_parser_and_highlighter_keywords() {
        for keyword in [
            "CONSTRUCTOR",
            "FINAL",
            "GROUPS",
            "INSTANTIABLE",
            "MAP",
            "MATCHES",
            "MULTISET",
            "OVERRIDING",
            "PACKAGE_BODY",
            "RECOGNIZE",
            "REPEATABLE",
            "SHARE",
            "SQLCODE",
            "SQLERRM",
            "STATIC",
            "SUBMULTISET",
            "TABLESAMPLE",
            "UNMATCHED",
            "WRAPPED",
            "XML",
            "XMLNAMESPACES",
        ] {
            assert!(
                is_oracle_sql_keyword(keyword),
                "missing shared keyword: {keyword}"
            );
        }
    }

    #[test]
    fn plsql_control_keyword_lookup_is_case_insensitive() {
        assert!(is_plsql_control_keyword("IF"));
        assert!(is_plsql_control_keyword("if"));
        assert!(is_plsql_control_keyword("Begin"));
        assert!(!is_plsql_control_keyword("iff"));
    }

    #[test]
    fn auto_terminated_tool_command_detects_connect_aliases() {
        assert!(is_auto_terminated_tool_command("CONNECT scott/tiger"));
        assert!(is_auto_terminated_tool_command("CONN scott/tiger"));
        assert!(is_auto_terminated_tool_command("DISCONNECT"));
        assert!(is_auto_terminated_tool_command("DISC"));
        assert!(is_auto_terminated_tool_command("@child.sql"));
        assert!(is_auto_terminated_tool_command("  @@child.sql"));
        assert!(is_auto_terminated_tool_command("PROMPT hello"));
        assert!(is_auto_terminated_tool_command("REM comment"));
        assert!(is_auto_terminated_tool_command("REMARK comment"));
        assert!(is_auto_terminated_tool_command("HOST ls"));
        assert!(is_auto_terminated_tool_command("! ls"));
        assert!(is_auto_terminated_tool_command("EXIT"));
        assert!(is_auto_terminated_tool_command("QUIT"));
        assert!(is_auto_terminated_tool_command("STARTUP"));
        assert!(is_auto_terminated_tool_command("SHUTDOWN IMMEDIATE"));
        assert!(is_auto_terminated_tool_command("ARCHIVE LOG LIST"));
        assert!(is_auto_terminated_tool_command("RECOVER DATABASE"));
        assert!(is_auto_terminated_tool_command("SPOOL out.log"));
        assert!(is_auto_terminated_tool_command("STORE SET out.sql REPLACE"));
        assert!(is_auto_terminated_tool_command("GET script.sql"));
        assert!(is_auto_terminated_tool_command("SAVE script.sql"));
        assert!(is_auto_terminated_tool_command("DESCRIBE emp"));
        assert!(is_auto_terminated_tool_command("DESC emp"));
        assert!(is_auto_terminated_tool_command(
            "EXEC dbms_output.put_line('x')"
        ));
        assert!(is_auto_terminated_tool_command(
            "EXECUTE dbms_output.put_line('x')"
        ));
        assert!(is_auto_terminated_tool_command("DEFINE v = 1"));
        assert!(is_auto_terminated_tool_command("UNDEFINE v"));
        assert!(is_auto_terminated_tool_command("WHENEVER SQLERROR EXIT"));
        assert!(is_auto_terminated_tool_command("COLUMN ename FORMAT A20"));
        assert!(is_auto_terminated_tool_command("CLEAR COLUMNS"));
        assert!(is_auto_terminated_tool_command(
            "SHOW PARAMETER open_cursors"
        ));
        assert!(is_auto_terminated_tool_command("SHOW ERRORS"));
        assert!(is_auto_terminated_tool_command("PASSWO scott"));
        assert!(is_auto_terminated_tool_command("PASSWOR scott"));
        assert!(is_auto_terminated_tool_command("PASSWORD scott"));
    }

    #[test]
    fn auto_terminated_tool_command_detects_recover_and_archive_heads() {
        assert!(is_auto_terminated_tool_command("RECOVER DATABASE"));
        assert!(is_auto_terminated_tool_command("ARCHIVE LOG LIST"));
    }

    #[test]
    fn statement_head_keyword_detects_password_abbreviations() {
        assert!(is_statement_head_keyword("PASSW"));
        assert!(is_statement_head_keyword("PASSWO"));
        assert!(is_statement_head_keyword("PASSWOR"));
        assert!(is_statement_head_keyword("PASSWORD"));
        for keyword in ["PASSW", "PASSWO", "PASSWOR", "PASSWORD"] {
            assert!(
                statement_head_keywords().contains(&keyword),
                "{keyword} must be offered at an Oracle top-level statement start"
            );
        }
    }

    #[test]
    fn sqlplus_set_option_keyword_detects_supported_set_subcommands() {
        assert!(is_sqlplus_set_option_keyword("SQLFORMAT"));
        assert!(is_sqlplus_set_option_keyword("APPINFO"));
        assert!(is_sqlplus_set_option_keyword("TERMOUT"));
        assert!(!is_sqlplus_set_option_keyword("UNKNOWN"));
    }

    #[test]
    fn auto_terminated_tool_command_ignores_connect_by_sql_clause() {
        assert!(!is_auto_terminated_tool_command(
            "CONNECT BY PRIOR id = parent_id"
        ));
        assert!(!is_auto_terminated_tool_command(
            "CONNECT /*hierarchical*/ BY PRIOR id = parent_id"
        ));
        assert!(!is_auto_terminated_tool_command(
            "CONNECT /*+ hint */ BY PRIOR id = parent_id"
        ));
    }

    #[test]
    fn auto_terminated_tool_command_set_with_block_comment_is_detected() {
        assert!(is_auto_terminated_tool_command(
            "SET /*sqlplus*/ TERMOUT ON"
        ));
        assert!(is_auto_terminated_tool_command(
            "SET /*a*/ /*b*/ PAGESIZE 100"
        ));
    }

    #[test]
    fn auto_terminated_tool_command_ignores_leading_line_comment_before_set() {
        assert!(!is_auto_terminated_tool_command(
            "-- comment\nSET TERMOUT ON"
        ));
    }

    #[test]
    fn auto_terminated_tool_command_ignores_comment_only_block_comment_line() {
        assert!(!is_auto_terminated_tool_command("/* comment */"));
    }

    #[test]
    fn auto_terminated_tool_command_ignores_unterminated_block_comment() {
        assert!(!is_auto_terminated_tool_command("SET /* unterminated"));
    }

    #[test]
    fn auto_terminated_tool_command_ignores_start_with_sql_clause() {
        assert!(is_auto_terminated_tool_command("START child.sql"));
        assert!(!is_auto_terminated_tool_command("START WITH"));
        assert!(!is_auto_terminated_tool_command("START /*tree*/ WITH"));
        assert!(!is_auto_terminated_tool_command(
            "START WITH parent_id IS NULL"
        ));
    }

    #[test]
    fn auto_terminated_tool_command_requires_arguments_for_start_and_connect() {
        assert!(!is_auto_terminated_tool_command("START"));
        assert!(!is_auto_terminated_tool_command("CONNECT"));
        assert!(is_auto_terminated_tool_command("START script.sql"));
        assert!(is_auto_terminated_tool_command("CONNECT scott/tiger"));
    }

    #[test]
    fn parser_and_intellisense_keyword_groups_are_shared() {
        assert!(is_with_plsql_declaration_keyword("FUNCTION"));
        assert!(is_with_plsql_declaration_keyword("procedure"));
        assert!(is_with_plsql_declaration_keyword("PACKAGE"));
        assert!(is_with_plsql_declaration_keyword("type"));
        assert!(is_with_non_cte_query_head_keyword("xmlnamespaces"));
        assert!(with_plsql_declaration_starts_routine_body("PACKAGE"));
        assert!(!with_plsql_declaration_starts_routine_body("TYPE"));
        assert!(is_external_language_target_keyword("javascript"));
        assert!(is_external_language_target_keyword("mle"));
        assert!(is_external_language_target_keyword("rust"));
        assert!(is_external_language_clause_keyword("LANGUAGE"));
        assert!(is_external_language_clause_keyword("module"));
        assert!(is_external_language_clause_keyword("SIGNATURE"));
        assert!(is_external_language_clause_keyword("ENV"));
        assert!(is_external_language_clause_keyword("environment"));
        assert!(is_external_language_clause_keyword("AGENT"));
        assert!(is_external_language_clause_keyword("CREDENTIAL"));
        assert!(is_external_language_clause_keyword("IMPORT"));
        assert!(is_external_language_clause_keyword("parameters"));
        assert!(is_external_language_clause_keyword("EXPORT"));
        assert!(is_format_column_constraint_keyword("generated"));
        assert!(is_table_function_item_leading_keyword("ORDINALITY"));
        assert!(is_table_function_item_leading_keyword("quotes"));
    }

    #[test]
    fn shared_keyword_pool_includes_additional_oracle_keywords() {
        for keyword in [
            "EDITIONING",
            "OMIT",
            "ORDINALITY",
            "OVERLAY",
            "POSITION",
            "SUBSET",
            "SUBSTRING",
            "XMLCAST",
            "XMLTABLE",
        ] {
            assert!(
                is_oracle_sql_keyword(keyword),
                "missing shared keyword: {keyword}"
            );
        }
    }

    #[test]
    fn statement_head_keywords_include_common_oracle_ddl_dcl_and_tcl() {
        for keyword in [
            "ALTER",
            "CREATE",
            "DROP",
            "TRUNCATE",
            "RENAME",
            "GRANT",
            "REVOKE",
            "COMMIT",
            "ROLLBACK",
            "SAVEPOINT",
            "LOCK",
            "FLASHBACK",
            "PURGE",
            "SELECT",
            "INSERT",
            "UPDATE",
            "DELETE",
            "MERGE",
        ] {
            assert!(
                is_statement_head_keyword(keyword),
                "missing statement head keyword: {keyword}"
            );
        }
    }

    #[test]
    fn mysql_statement_head_keywords_include_common_sql_statement_families() {
        for keyword in [
            "ALTER",
            "ANALYZE",
            "BEGIN",
            "BINLOG",
            "CACHE",
            "CALL",
            "CHANGE",
            "CHECK",
            "CHECKSUM",
            "CLONE",
            "COMMIT",
            "CREATE",
            "DEALLOCATE",
            "DELETE",
            "DESC",
            "DESCRIBE",
            "DO",
            "DROP",
            "EXECUTE",
            "EXPLAIN",
            "FLUSH",
            "GRANT",
            "HANDLER",
            "HELP",
            "IMPORT",
            "INSERT",
            "INSTALL",
            "KILL",
            "LOAD",
            "LOCK",
            "OPTIMIZE",
            "PREPARE",
            "PURGE",
            "RELEASE",
            "RENAME",
            "REPAIR",
            "REPLACE",
            "RESET",
            "RESIGNAL",
            "RESTART",
            "REVOKE",
            "ROLLBACK",
            "SAVEPOINT",
            "SELECT",
            "SET",
            "SHOW",
            "SHUTDOWN",
            "SIGNAL",
            "START",
            "STOP",
            "TABLE",
            "TRUNCATE",
            "UNINSTALL",
            "UNLOCK",
            "UPDATE",
            "USE",
            "VALUES",
            "WITH",
            "XA",
        ] {
            assert!(
                is_mysql_statement_head_keyword_upper(keyword),
                "missing MySQL statement head keyword: {keyword}"
            );
        }
    }
}

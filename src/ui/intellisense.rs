use crate::sql_text;
use crate::ui::theme;
use fltk::{browser::HoldBrowser, frame::Frame, prelude::*, text::TextEditor, window::Window};
use std::any::Any;
use std::borrow::Cow;
use std::collections::{HashMap, HashSet, VecDeque};
use std::panic::{self, AssertUnwindSafe};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

// Shared Oracle SQL keywords
pub const SQL_KEYWORDS: &[&str] = sql_text::ORACLE_SQL_KEYWORDS;

// Oracle built-in functions
pub const ORACLE_FUNCTIONS: &[&str] = &[
    "ABS",
    "ACOS",
    "ADD_MONTHS",
    "ANY_VALUE",
    "APPENDCHILDXML",
    "APPROX_COUNT_DISTINCT",
    "APPROX_PERCENTILE",
    "ASCII",
    "ASCIISTR",
    "ASIN",
    "ATAN",
    "ATAN2",
    "AVG",
    "BFILENAME",
    "BIN_TO_NUM",
    "BITAND",
    "CARDINALITY",
    "CAST",
    "CEIL",
    "CHARTOROWID",
    "CHR",
    "CLUSTER_DETAILS",
    "CLUSTER_DISTANCE",
    "CLUSTER_ID",
    "CLUSTER_PROBABILITY",
    "CLUSTER_SET",
    "COALESCE",
    "COLLECT",
    "COMPOSE",
    "CONCAT",
    "CONVERT",
    "CORR",
    "COS",
    "COSH",
    "COUNT",
    "COVAR_POP",
    "COVAR_SAMP",
    "CUME_DIST",
    "CURRENT_DATE",
    "CURRENT_TIMESTAMP",
    "CV",
    "DBTIMEZONE",
    "DECODE",
    "DECOMPOSE",
    "DELETEXML",
    "DENSE_RANK",
    "DEREF",
    "DUMP",
    "EMPTY_BLOB",
    "EMPTY_CLOB",
    "EXISTSNODE",
    "EXP",
    "EXTRACT",
    "EXTRACTVALUE",
    "FEATURE_COMPARE",
    "FEATURE_ID",
    "FEATURE_SET",
    "FEATURE_VALUE",
    "FIRST_VALUE",
    "FLOOR",
    "FROM_TZ",
    "GREATEST",
    "GROUPING",
    "GROUPING_ID",
    "GROUP_ID",
    "HEXTORAW",
    "INITCAP",
    "INSERTCHILDXML",
    "INSERTCHILDXMLAFTER",
    "INSERTCHILDXMLBEFORE",
    "INSERTXMLBEFORE",
    "INSTR",
    "JSON_ARRAY",
    "JSON_ARRAYAGG",
    "JSON_EQUAL",
    "JSON_EXISTS",
    "JSON_MERGEPATCH",
    "JSON_OBJECT",
    "JSON_OBJECTAGG",
    "JSON_QUERY",
    "JSON_SCALAR",
    "JSON_SERIALIZE",
    "JSON_TABLE",
    "JSON_TRANSFORM",
    "JSON_VALUE",
    "LAG",
    "LAST_DAY",
    "LAST_VALUE",
    "LEAD",
    "LEAST",
    "LENGTH",
    "LENGTHB",
    "LISTAGG",
    "LN",
    "LNNVL",
    "LOCALTIMESTAMP",
    "LOG",
    "LOWER",
    "LPAD",
    "LTRIM",
    "MAKE_REF",
    "MATCH_NUMBER",
    "MAX",
    "MEDIAN",
    "MIN",
    "MOD",
    "MONTHS_BETWEEN",
    "NANVL",
    "NEW_TIME",
    "NEXT_DAY",
    "NLSSORT",
    "NLS_INITCAP",
    "NLS_LOWER",
    "NLS_UPPER",
    "NTH_VALUE",
    "NTILE",
    "NULLIF",
    "NUMTODSINTERVAL",
    "NUMTOYMINTERVAL",
    "NVL",
    "NVL2",
    "ODCINUMBERLIST",
    "ORA_HASH",
    "ORA_INVOKING_USER",
    "ORA_INVOKING_USERID",
    "PERCENTILE_CONT",
    "PERCENTILE_DISC",
    "PERCENT_RANK",
    "POWER",
    "PREDICTION",
    "PREDICTION_BOUNDS",
    "PREDICTION_COST",
    "PREDICTION_DETAILS",
    "PREDICTION_PROBABILITY",
    "PREDICTION_SET",
    "PREV",
    "RANK",
    "RATIO_TO_REPORT",
    "RAWTOHEX",
    "REF",
    "REFTOHEX",
    "REGEXP_COUNT",
    "REGEXP_INSTR",
    "REGEXP_LIKE",
    "REGEXP_REPLACE",
    "REGEXP_SUBSTR",
    "REGR_AVGX",
    "REGR_AVGY",
    "REGR_COUNT",
    "REGR_INTERCEPT",
    "REGR_R2",
    "REGR_SLOPE",
    "REGR_SXX",
    "REGR_SXY",
    "REGR_SYY",
    "REMAINDER",
    "REPLACE",
    "REVERSE",
    "ROUND",
    "ROWIDTOCHAR",
    "ROW_NUMBER",
    "RPAD",
    "RTRIM",
    "SESSIONTIMEZONE",
    "SIGN",
    "SIN",
    "SINH",
    "SOUNDEX",
    "SQRT",
    "STANDARD_HASH",
    "STDDEV",
    "STDDEV_POP",
    "STDDEV_SAMP",
    "SUBSTR",
    "SUM",
    "SYSDATE",
    "SYSTIMESTAMP",
    "SYS_CONNECT_BY_PATH",
    "SYS_CONTEXT",
    "SYS_EXTRACT_UTC",
    "SYS_GUID",
    "SYS_TYPEID",
    "TAN",
    "TANH",
    "TO_BINARY_DOUBLE",
    "TO_BINARY_FLOAT",
    "TO_BLOB",
    "TO_CHAR",
    "TO_CLOB",
    "TO_DATE",
    "TO_DSINTERVAL",
    "TO_LOB",
    "TO_MULTI_BYTE",
    "TO_NCHAR",
    "TO_NCLOB",
    "TO_NUMBER",
    "TO_SINGLE_BYTE",
    "TO_TIMESTAMP",
    "TO_TIMESTAMP_TZ",
    "TO_VECTOR",
    "TO_YMINTERVAL",
    "TRANSLATE",
    "TREAT",
    "TRIM",
    "TRUNC",
    "TZ_OFFSET",
    "UID",
    "UNISTR",
    "UPDATEXML",
    "UPPER",
    "USER",
    "USERENV",
    "VALIDATE_CONVERSION",
    "VALUE",
    "VARIANCE",
    "VAR_POP",
    "VAR_SAMP",
    "VECTOR_DISTANCE",
    "VSIZE",
    "WIDTH_BUCKET",
    "XMLAGG",
    "XMLATTRIBUTES",
    "XMLCAST",
    "XMLCDATA",
    "XMLCOLATTVAL",
    "XMLCOMMENT",
    "XMLCONCAT",
    "XMLELEMENT",
    "XMLEXISTS",
    "XMLFOREST",
    "XMLPARSE",
    "XMLPI",
    "XMLQUERY",
    "XMLROOT",
    "XMLSEQUENCE",
    "XMLSERIALIZE",
    "XMLTABLE",
    "XMLTRANSFORM",
    "XMLTYPE",
    "XPATH",
];

// ---------------------------------------------------------------------------
// MySQL / MariaDB built-in functions (sorted, uppercase)
// ---------------------------------------------------------------------------
pub const MYSQL_FUNCTIONS: &[&str] = &[
    "ABS",
    "ACOS",
    "ADDDATE",
    "ADDTIME",
    "AES_DECRYPT",
    "AES_ENCRYPT",
    "ANY_VALUE",
    "ASCII",
    "ASIN",
    "ATAN",
    "ATAN2",
    "AVG",
    "BENCHMARK",
    "BIN",
    "BIN_TO_UUID",
    "BIT_AND",
    "BIT_COUNT",
    "BIT_LENGTH",
    "BIT_OR",
    "BIT_XOR",
    "CAST",
    "CEIL",
    "CEILING",
    "CHAR",
    "CHARACTER_LENGTH",
    "CHARSET",
    "CHAR_LENGTH",
    "COALESCE",
    "COERCIBILITY",
    "COLLATION",
    "COLUMN_CREATE",
    "COLUMN_GET",
    "COMPRESS",
    "CONCAT",
    "CONCAT_WS",
    "CONNECTION_ID",
    "CONV",
    "CONVERT",
    "CONVERT_TZ",
    "COS",
    "COT",
    "COUNT",
    "CRC32",
    "CUME_DIST",
    "CURDATE",
    "CURRENT_DATE",
    "CURRENT_TIME",
    "CURRENT_TIMESTAMP",
    "CURRENT_USER",
    "CURTIME",
    "DATABASE",
    "DATE",
    "DATEDIFF",
    "DATE_ADD",
    "DATE_FORMAT",
    "DATE_SUB",
    "DAY",
    "DAYNAME",
    "DAYOFMONTH",
    "DAYOFWEEK",
    "DAYOFYEAR",
    "DECODE",
    "DEFAULT",
    "DEGREES",
    "DENSE_RANK",
    "DES_DECRYPT",
    "DES_ENCRYPT",
    "ELT",
    "ENCODE",
    "ENCRYPT",
    "EXP",
    "EXPORT_SET",
    "EXTRACT",
    "EXTRACTVALUE",
    "FIELD",
    "FIND_IN_SET",
    "FIRST_VALUE",
    "FLOOR",
    "FORMAT",
    "FOUND_ROWS",
    "FROM_BASE64",
    "FROM_DAYS",
    "FROM_UNIXTIME",
    "GET_FORMAT",
    "GET_LOCK",
    "GREATEST",
    "GROUP_CONCAT",
    "GROUPING",
    "HEX",
    "HOUR",
    "IF",
    "IFNULL",
    "INET6_ATON",
    "INET6_NTOA",
    "INET_ATON",
    "INET_NTOA",
    "INSERT",
    "INSTR",
    "ISNULL",
    "IS_FREE_LOCK",
    "IS_IPV4",
    "IS_IPV4_COMPAT",
    "IS_IPV4_MAPPED",
    "IS_IPV6",
    "IS_USED_LOCK",
    "JSON_ARRAY",
    "JSON_ARRAYAGG",
    "JSON_ARRAY_APPEND",
    "JSON_ARRAY_INSERT",
    "JSON_CONTAINS",
    "JSON_CONTAINS_PATH",
    "JSON_DEPTH",
    "JSON_EXTRACT",
    "JSON_INSERT",
    "JSON_KEYS",
    "JSON_LENGTH",
    "JSON_MERGE",
    "JSON_MERGE_PATCH",
    "JSON_MERGE_PRESERVE",
    "JSON_NORMALIZE",
    "JSON_OBJECT",
    "JSON_OBJECTAGG",
    "JSON_OVERLAPS",
    "JSON_PRETTY",
    "JSON_QUOTE",
    "JSON_REMOVE",
    "JSON_REPLACE",
    "JSON_SCHEMA_VALID",
    "JSON_SCHEMA_VALIDATION_REPORT",
    "JSON_SEARCH",
    "JSON_SET",
    "JSON_STORAGE_FREE",
    "JSON_STORAGE_SIZE",
    "JSON_TABLE",
    "JSON_TYPE",
    "JSON_UNQUOTE",
    "JSON_VALID",
    "JSON_VALUE",
    "LAG",
    "LAST_DAY",
    "LAST_INSERT_ID",
    "LAST_VALUE",
    "LCASE",
    "LEAD",
    "LEAST",
    "LEFT",
    "LENGTH",
    "LN",
    "LOAD_FILE",
    "LOCALTIME",
    "LOCALTIMESTAMP",
    "LOCATE",
    "LOG",
    "LOG10",
    "LOG2",
    "LOWER",
    "LPAD",
    "LTRIM",
    "MAKEDATE",
    "MAKETIME",
    "MAKE_SET",
    "MASTER_POS_WAIT",
    "MAX",
    "MBRCONTAINS",
    "MD5",
    "MEDIAN",
    "MICROSECOND",
    "MID",
    "MIN",
    "MINUTE",
    "MOD",
    "MONTH",
    "MONTHNAME",
    "NAME_CONST",
    "NATURAL_SORT_KEY",
    "NOW",
    "NTH_VALUE",
    "NTILE",
    "NULLIF",
    "OCT",
    "OCTET_LENGTH",
    "OLD_PASSWORD",
    "ORD",
    "PASSWORD",
    "PERCENTILE_CONT",
    "PERCENTILE_DISC",
    "PERCENT_RANK",
    "PERIOD_ADD",
    "PERIOD_DIFF",
    "PI",
    "POINT",
    "POLYGON",
    "POSITION",
    "POW",
    "POWER",
    "QUARTER",
    "QUOTE",
    "RADIANS",
    "RAND",
    "RANDOM_BYTES",
    "RANK",
    "REGEXP_INSTR",
    "REGEXP_LIKE",
    "REGEXP_REPLACE",
    "REGEXP_SUBSTR",
    "RELEASE_ALL_LOCKS",
    "RELEASE_LOCK",
    "REPEAT",
    "REPLACE",
    "REVERSE",
    "RIGHT",
    "ROUND",
    "ROW",
    "ROW_COUNT",
    "ROW_NUMBER",
    "RPAD",
    "RTRIM",
    "SCHEMA",
    "SECOND",
    "SEC_TO_TIME",
    "SESSION_USER",
    "SHA1",
    "SHA2",
    "SIGN",
    "SIN",
    "SLEEP",
    "SOUNDEX",
    "SPACE",
    "SQRT",
    "STD",
    "STDDEV",
    "STDDEV_POP",
    "STDDEV_SAMP",
    "STRCMP",
    "STR_TO_DATE",
    "ST_AREA",
    "ST_ASBINARY",
    "ST_ASGEOJSON",
    "ST_ASTEXT",
    "ST_ASWKB",
    "ST_ASWKT",
    "ST_BUFFER",
    "ST_CENTROID",
    "ST_CONTAINS",
    "ST_CONVEXHULL",
    "ST_CROSSES",
    "ST_DIFFERENCE",
    "ST_DIMENSION",
    "ST_DISJOINT",
    "ST_DISTANCE",
    "ST_DISTANCE_SPHERE",
    "ST_ENDPOINT",
    "ST_ENVELOPE",
    "ST_EQUALS",
    "ST_EXTERIORRING",
    "ST_GEOMCOLLFROMTEXT",
    "ST_GEOMCOLLFROMWKB",
    "ST_GEOMETRYCOLLECTIONFROMTEXT",
    "ST_GEOMETRYCOLLECTIONFROMWKB",
    "ST_GEOMETRYFROMTEXT",
    "ST_GEOMETRYFROMWKB",
    "ST_GEOMETRYN",
    "ST_GEOMETRYTYPE",
    "ST_GEOMFROMGEOJSON",
    "ST_GEOMFROMTEXT",
    "ST_GEOMFROMWKB",
    "ST_INTERIORRINGN",
    "ST_INTERSECTION",
    "ST_INTERSECTS",
    "ST_ISCLOSED",
    "ST_ISEMPTY",
    "ST_ISSIMPLE",
    "ST_ISVALID",
    "ST_LATFROMGEOHASH",
    "ST_LATITUDE",
    "ST_LENGTH",
    "ST_LINEFROMTEXT",
    "ST_LINEFROMWKB",
    "ST_LINESTRINGFROMTEXT",
    "ST_LINESTRINGFROMWKB",
    "ST_LONGFROMGEOHASH",
    "ST_LONGITUDE",
    "ST_MAKEENVELOPE",
    "ST_MLINEFROMTEXT",
    "ST_MLINEFROMWKB",
    "ST_MPOINTFROMTEXT",
    "ST_MPOINTFROMWKB",
    "ST_MPOLYFROMTEXT",
    "ST_MPOLYFROMWKB",
    "ST_MULTILINESTRINGFROMTEXT",
    "ST_MULTILINESTRINGFROMWKB",
    "ST_MULTIPOINTFROMTEXT",
    "ST_MULTIPOINTFROMWKB",
    "ST_MULTIPOLYGONFROMTEXT",
    "ST_MULTIPOLYGONFROMWKB",
    "ST_NUMGEOMETRIES",
    "ST_NUMINTERIORRING",
    "ST_NUMINTERIORRINGS",
    "ST_NUMPOINTS",
    "ST_OVERLAPS",
    "ST_POINTATDISTANCE",
    "ST_POINTFROMGEOHASH",
    "ST_POINTFROMTEXT",
    "ST_POINTFROMWKB",
    "ST_POINTN",
    "ST_POLYFROMTEXT",
    "ST_POLYFROMWKB",
    "ST_POLYGONFROMTEXT",
    "ST_POLYGONFROMWKB",
    "ST_SIMPLIFY",
    "ST_SRID",
    "ST_STARTPOINT",
    "ST_SWAPXY",
    "ST_SYMDIFFERENCE",
    "ST_TOUCHES",
    "ST_TRANSFORM",
    "ST_UNION",
    "ST_VALIDATE",
    "ST_WITHIN",
    "ST_X",
    "ST_Y",
    "SUBDATE",
    "SUBSTR",
    "SUBSTRING",
    "SUBSTRING_INDEX",
    "SUBTIME",
    "SUM",
    "SYSDATE",
    "SYSTEM_USER",
    "TAN",
    "TIME",
    "TIMEDIFF",
    "TIMESTAMP",
    "TIMESTAMPADD",
    "TIMESTAMPDIFF",
    "TIME_FORMAT",
    "TIME_TO_SEC",
    "TO_BASE64",
    "TO_DAYS",
    "TO_SECONDS",
    "TRIM",
    "TRUNCATE",
    "UCASE",
    "UNCOMPRESS",
    "UNCOMPRESSED_LENGTH",
    "UNHEX",
    "UNIX_TIMESTAMP",
    "UPDATEXML",
    "UPPER",
    "USER",
    "UTC_DATE",
    "UTC_TIME",
    "UTC_TIMESTAMP",
    "UUID",
    "UUID_SHORT",
    "UUID_TO_BIN",
    "VALIDATE_PASSWORD_STRENGTH",
    "VALUES",
    "VARIANCE",
    "VAR_POP",
    "VAR_SAMP",
    "VEC_DISTANCE_COSINE",
    "VEC_DISTANCE_EUCLIDEAN",
    "VEC_FROMTEXT",
    "VEC_TOTEXT",
    "VERSION",
    "WAIT_FOR_EXECUTED_GTID_SET",
    "WEEK",
    "WEEKDAY",
    "WEEKOFYEAR",
    "WEIGHT_STRING",
    "YEAR",
    "YEARWEEK",
];

/// MariaDB built-ins that are not available in MySQL. Keep this separate from
/// the shared MySQL-family catalog so dialect-specific completion never offers
/// executable-looking functions to the wrong server.
pub const MARIADB_FUNCTIONS: &[&str] = &["COLUMN_CHECK", "COLUMN_JSON", "JSON_EXISTS", "UUID_V7"];

pub static MYSQL_FUNCTIONS_SET: once_cell::sync::Lazy<std::collections::HashSet<&'static str>> =
    once_cell::sync::Lazy::new(|| MYSQL_FUNCTIONS.iter().copied().collect());
pub static MARIADB_FUNCTIONS_SET: once_cell::sync::Lazy<std::collections::HashSet<&'static str>> =
    once_cell::sync::Lazy::new(|| MARIADB_FUNCTIONS.iter().copied().collect());

const FUNCTION_SUFFIX: &str = "()";
const ORACLE_BUILTIN_PACKAGES: &[&str] = &[
    "DBMS_OUTPUT",
    "DBMS_LOB",
    "DBMS_SQL",
    "DBMS_UTILITY",
    "DBMS_RANDOM",
    "DBMS_METADATA",
    "DBMS_STATS",
    "DBMS_XPLAN",
    "DBMS_APPLICATION_INFO",
    "DBMS_SESSION",
    "DBMS_SCHEDULER",
    "DBMS_LOCK",
    "DBMS_JOB",
    "DBMS_CRYPTO",
    "DBMS_ASSERT",
    "DBMS_MVIEW",
    "DBMS_ALERT",
    "DBMS_PIPE",
    "UTL_FILE",
    "UTL_RAW",
    "UTL_HTTP",
    "UTL_ENCODE",
    "UTL_INADDR",
];

pub(crate) const MAX_SUGGESTIONS: usize = 80;
type LanguageCatalog = (
    &'static [&'static str],
    &'static [&'static str],
    &'static [&'static str],
);
const ORACLE_LANGUAGE_CATALOG: LanguageCatalog = (SQL_KEYWORDS, ORACLE_FUNCTIONS, &[]);
const MYSQL_LANGUAGE_CATALOG: LanguageCatalog =
    (sql_text::MYSQL_SQL_KEYWORDS, MYSQL_FUNCTIONS, &[]);
const MARIADB_LANGUAGE_CATALOG: LanguageCatalog = (
    sql_text::MYSQL_SQL_KEYWORDS,
    MYSQL_FUNCTIONS,
    MARIADB_FUNCTIONS,
);

fn language_catalog_for_db_type(db_type: Option<crate::db::DatabaseType>) -> LanguageCatalog {
    match db_type {
        None => ORACLE_LANGUAGE_CATALOG,
        Some(crate::db::DatabaseType::Oracle) => ORACLE_LANGUAGE_CATALOG,
        Some(crate::db::DatabaseType::MySQL) => MYSQL_LANGUAGE_CATALOG,
        Some(crate::db::DatabaseType::MariaDB) => MARIADB_LANGUAGE_CATALOG,
    }
}

pub(crate) fn language_catalog_suggestions_for_db(
    prefix: &str,
    prefer_columns: bool,
    db_type: Option<crate::db::DatabaseType>,
) -> Vec<String> {
    let mut suggestions = Vec::new();
    let mut seen = HashSet::new();
    IntellisenseData::append_language_catalog_suggestions_for_db(
        prefix,
        prefer_columns,
        db_type,
        &mut suggestions,
        &mut seen,
    );
    suggestions
}

#[derive(Clone, PartialEq, Eq)]
struct NameEntry {
    name: String,
    upper: String,
}

impl NameEntry {
    fn new(name: String) -> Self {
        let upper = Self::lookup_upper(&name);
        Self { name, upper }
    }

    fn lookup_upper(name: &str) -> String {
        let trimmed = name.trim();
        if sql_text::is_quoted_identifier(trimmed) {
            sql_text::strip_identifier_quotes(trimmed).to_uppercase()
        } else if trimmed.starts_with('[') && trimmed.ends_with(']') && trimmed.len() >= 2 {
            trimmed[1..trimmed.len().saturating_sub(1)]
                .replace("]]", "]")
                .to_uppercase()
        } else {
            name.to_uppercase()
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum QualifiedMemberKind {
    Table,
    View,
    MaterializedView,
    Type,
    Trigger,
    Event,
    Index,
    Procedure,
    Function,
    Package,
    Sequence,
    Synonym,
    PublicSynonym,
    DatabaseLink,
    Directory,
    Library,
    Cluster,
    Context,
    Dimension,
    Operator,
    Indextype,
    Edition,
    JavaSource,
    JavaClass,
    JavaResource,
    User,
}

impl QualifiedMemberKind {
    pub fn type_label(self) -> &'static str {
        match self {
            Self::Table => "TABLE",
            Self::View => "VIEW",
            Self::MaterializedView => "MVIEW",
            Self::Type => "TYPE",
            Self::Trigger => "TRIGGER",
            Self::Event => "EVENT",
            Self::Index => "INDEX",
            Self::Procedure => "PROCEDURE",
            Self::Function => "FUNCTION",
            Self::Package => "PACKAGE",
            Self::Sequence => "SEQUENCE",
            Self::Synonym => "SYNONYM",
            Self::PublicSynonym => "PUBLIC SYNONYM",
            Self::DatabaseLink => "DATABASE LINK",
            Self::Directory => "DIRECTORY",
            Self::Library => "LIBRARY",
            Self::Cluster => "CLUSTER",
            Self::Context => "CONTEXT",
            Self::Dimension => "DIMENSION",
            Self::Operator => "OPERATOR",
            Self::Indextype => "INDEXTYPE",
            Self::Edition => "EDITION",
            Self::JavaSource => "JAVA SOURCE",
            Self::JavaClass => "JAVA CLASS",
            Self::JavaResource => "JAVA RESOURCE",
            Self::User => "USER",
        }
    }

    pub fn from_object_type_name(object_type: &str) -> Option<Self> {
        match object_type.trim().to_ascii_uppercase().as_str() {
            "TABLE" | "BASE TABLE" => Some(Self::Table),
            "VIEW" | "EDITIONING VIEW" => Some(Self::View),
            "MATERIALIZED VIEW" => Some(Self::MaterializedView),
            "TYPE" | "TYPE BODY" => Some(Self::Type),
            "TRIGGER" => Some(Self::Trigger),
            "EVENT" => Some(Self::Event),
            "INDEX" => Some(Self::Index),
            "PROCEDURE" => Some(Self::Procedure),
            "FUNCTION" => Some(Self::Function),
            "PACKAGE" | "PACKAGE BODY" => Some(Self::Package),
            "SEQUENCE" => Some(Self::Sequence),
            "SYNONYM" => Some(Self::Synonym),
            "PUBLIC SYNONYM" => Some(Self::PublicSynonym),
            "DATABASE LINK" => Some(Self::DatabaseLink),
            "DIRECTORY" => Some(Self::Directory),
            "LIBRARY" => Some(Self::Library),
            "CLUSTER" => Some(Self::Cluster),
            "CONTEXT" => Some(Self::Context),
            "DIMENSION" => Some(Self::Dimension),
            "OPERATOR" => Some(Self::Operator),
            "INDEXTYPE" => Some(Self::Indextype),
            "EDITION" => Some(Self::Edition),
            "JAVA SOURCE" => Some(Self::JavaSource),
            "JAVA CLASS" => Some(Self::JavaClass),
            "JAVA RESOURCE" => Some(Self::JavaResource),
            "USER" | "SCHEMA" => Some(Self::User),
            _ => None,
        }
    }
}

/// Lightweight, display-ready metadata for a single column, used to enrich
/// the completion popup (data type / NOT NULL / PK badges) without changing
/// the `Vec<String>` suggestion pipeline. Populated by the column loader.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ColumnMeta {
    pub type_display: String,
    pub nullable: bool,
    pub is_primary_key: bool,
}

/// Per-suggestion detail rendered in the completion popup's type and badge
/// columns. Keyed (in the descriptions map) by the suggestion's normalized
/// uppercase name.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SuggestionDetail {
    /// Type column: object kind (TABLE / VIEW / KEYWORD / …) or, for columns,
    /// the data type display (e.g. `NUMBER(10)`).
    pub type_text: String,
    /// Badge column: `PK` / `NN` / `FK→TABLE` for columns; empty otherwise.
    pub badges: String,
}

/// A foreign-key relationship for a table, with the local and referenced
/// columns paired by position. Used for FK badges and FK-based auto-join.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ForeignKeyMeta {
    pub columns: Vec<String>,
    pub ref_table: String,
    pub ref_columns: Vec<String>,
}

const MAX_SIGNATURE_CACHE_ENTRIES: usize = 256;

#[derive(Clone)]
pub struct IntellisenseData {
    pub tables: Vec<String>,
    pub columns: HashMap<String, Vec<String>>, // table_name -> column_names
    pub columns_loading: HashSet<String>,
    column_loading_started_at: HashMap<String, Instant>,
    pub views: Vec<String>,
    pub materialized_views: Vec<String>,
    pub types: Vec<String>,
    pub triggers: Vec<String>,
    pub events: Vec<String>,
    pub indexes: Vec<String>,
    pub procedures: Vec<String>,
    pub functions: Vec<String>,
    pub packages: Vec<String>,
    pub sequences: Vec<String>,
    pub synonyms: Vec<String>,
    pub public_synonyms: Vec<String>,
    pub database_links: Vec<String>,
    pub directories: Vec<String>,
    pub libraries: Vec<String>,
    pub clusters: Vec<String>,
    pub contexts: Vec<String>,
    pub dimensions: Vec<String>,
    pub operators: Vec<String>,
    pub indextypes: Vec<String>,
    pub editions: Vec<String>,
    pub java_sources: Vec<String>,
    pub java_classes: Vec<String>,
    pub java_resources: Vec<String>,
    /// MySQL/MariaDB database (schema) names. Kept separate from `users` so
    /// database-target DDL does not expose account/owner names.
    pub schemas: Vec<String>,
    pub users: Vec<String>,
    default_qualifier: Option<String>,
    default_qualifier_name: Option<String>,
    table_entries: Vec<NameEntry>,
    view_entries: Vec<NameEntry>,
    materialized_view_entries: Vec<NameEntry>,
    type_entries: Vec<NameEntry>,
    trigger_entries: Vec<NameEntry>,
    event_entries: Vec<NameEntry>,
    index_entries: Vec<NameEntry>,
    procedure_entries: Vec<NameEntry>,
    function_entries: Vec<NameEntry>,
    package_entries: Vec<NameEntry>,
    sequence_entries: Vec<NameEntry>,
    synonym_entries: Vec<NameEntry>,
    public_synonym_entries: Vec<NameEntry>,
    database_link_entries: Vec<NameEntry>,
    directory_entries: Vec<NameEntry>,
    library_entries: Vec<NameEntry>,
    cluster_entries: Vec<NameEntry>,
    context_entries: Vec<NameEntry>,
    dimension_entries: Vec<NameEntry>,
    operator_entries: Vec<NameEntry>,
    indextype_entries: Vec<NameEntry>,
    edition_entries: Vec<NameEntry>,
    java_source_entries: Vec<NameEntry>,
    java_class_entries: Vec<NameEntry>,
    java_resource_entries: Vec<NameEntry>,
    schema_entries: Vec<NameEntry>,
    user_entries: Vec<NameEntry>,
    column_entries_by_table: HashMap<String, Vec<NameEntry>>,
    virtual_column_entries_by_table: HashMap<String, Vec<NameEntry>>,
    member_entries_by_qualifier: HashMap<String, Vec<NameEntry>>,
    member_kinds_by_qualifier: HashMap<String, HashMap<String, HashSet<QualifiedMemberKind>>>,
    relation_member_entries_by_qualifier: HashMap<String, Vec<NameEntry>>,
    collection_element_type_by_type: HashMap<String, String>,
    synonym_target_by_synonym: HashMap<String, String>,
    relations_upper: HashSet<String>,
    /// Names of virtual tables (CTEs, subquery aliases) whose columns were
    /// derived from SQL text rather than database metadata.
    virtual_table_keys: HashSet<String>,
    /// Display metadata per column: table_upper -> column_upper -> meta.
    column_meta_by_table: HashMap<String, HashMap<String, ColumnMeta>>,
    /// Foreign keys per table: table_upper -> list of FK relationships.
    foreign_keys_by_table: HashMap<String, Vec<ForeignKeyMeta>>,
    /// Table keys with an in-flight foreign-key load, to avoid duplicate work.
    foreign_keys_loading: HashSet<String>,
    /// Resolved routine signatures keyed by routine lookup key. `None` records
    /// that a name was resolved but is not a callable routine, so it is not
    /// fetched again.
    signature_cache: HashMap<String, Option<SignatureLabel>>,
    signature_cache_order: VecDeque<String>,
    /// Routine keys with an in-flight signature fetch, to avoid duplicate work.
    signature_pending: HashSet<String>,
}

impl IntellisenseData {
    pub fn new() -> Self {
        Self {
            tables: Vec::new(),
            columns: HashMap::new(),
            columns_loading: HashSet::new(),
            column_loading_started_at: HashMap::new(),
            views: Vec::new(),
            materialized_views: Vec::new(),
            types: Vec::new(),
            triggers: Vec::new(),
            events: Vec::new(),
            indexes: Vec::new(),
            procedures: Vec::new(),
            functions: Vec::new(),
            packages: Vec::new(),
            sequences: Vec::new(),
            synonyms: Vec::new(),
            public_synonyms: Vec::new(),
            database_links: Vec::new(),
            directories: Vec::new(),
            libraries: Vec::new(),
            clusters: Vec::new(),
            contexts: Vec::new(),
            dimensions: Vec::new(),
            operators: Vec::new(),
            indextypes: Vec::new(),
            editions: Vec::new(),
            java_sources: Vec::new(),
            java_classes: Vec::new(),
            java_resources: Vec::new(),
            schemas: Vec::new(),
            users: Vec::new(),
            default_qualifier: None,
            default_qualifier_name: None,
            table_entries: Vec::new(),
            view_entries: Vec::new(),
            materialized_view_entries: Vec::new(),
            type_entries: Vec::new(),
            trigger_entries: Vec::new(),
            event_entries: Vec::new(),
            index_entries: Vec::new(),
            procedure_entries: Vec::new(),
            function_entries: Vec::new(),
            package_entries: Vec::new(),
            sequence_entries: Vec::new(),
            synonym_entries: Vec::new(),
            public_synonym_entries: Vec::new(),
            database_link_entries: Vec::new(),
            directory_entries: Vec::new(),
            library_entries: Vec::new(),
            cluster_entries: Vec::new(),
            context_entries: Vec::new(),
            dimension_entries: Vec::new(),
            operator_entries: Vec::new(),
            indextype_entries: Vec::new(),
            edition_entries: Vec::new(),
            java_source_entries: Vec::new(),
            java_class_entries: Vec::new(),
            java_resource_entries: Vec::new(),
            schema_entries: Vec::new(),
            user_entries: Vec::new(),
            column_entries_by_table: HashMap::new(),
            virtual_column_entries_by_table: HashMap::new(),
            member_entries_by_qualifier: HashMap::new(),
            member_kinds_by_qualifier: HashMap::new(),
            relation_member_entries_by_qualifier: HashMap::new(),
            collection_element_type_by_type: HashMap::new(),
            synonym_target_by_synonym: HashMap::new(),
            relations_upper: HashSet::new(),
            virtual_table_keys: HashSet::new(),
            column_meta_by_table: HashMap::new(),
            foreign_keys_by_table: HashMap::new(),
            foreign_keys_loading: HashSet::new(),
            signature_cache: HashMap::new(),
            signature_cache_order: VecDeque::new(),
            signature_pending: HashSet::new(),
        }
    }

    pub fn get_suggestions(
        &mut self,
        prefix: &str,
        include_columns: bool,
        column_tables: Option<&[String]>,
        prefer_relations: bool,
        prefer_columns: bool,
    ) -> Vec<String> {
        self.get_suggestions_for_db(
            prefix,
            include_columns,
            column_tables,
            prefer_relations,
            prefer_columns,
            None,
        )
    }

    fn append_language_catalog_suggestions_for_db(
        prefix: &str,
        prefer_columns: bool,
        db_type: Option<crate::db::DatabaseType>,
        suggestions: &mut Vec<String>,
        seen: &mut HashSet<String>,
    ) {
        let prefix_upper = Self::entry_lookup_prefix_upper(prefix);
        if prefix_upper.is_empty() {
            return;
        }

        let (keywords, functions, dialect_functions) = language_catalog_for_db_type(db_type);
        let start = keywords.partition_point(|kw| *kw < prefix_upper.as_str());
        for keyword in &keywords[start..] {
            if !keyword.starts_with(prefix_upper.as_str()) {
                break;
            }
            // In an expression/column context, keywords that can only begin a
            // statement/command or only modify DDL/storage/constraint clauses
            // are irrelevant noise.
            if prefer_columns && crate::sql_text::is_non_expression_keyword_upper(keyword) {
                continue;
            }
            if !suggestion_matches_completion_prefix(keyword, prefix) {
                continue;
            }
            if seen.insert((*keyword).to_string()) {
                suggestions.push((*keyword).to_string());
            }
            if suggestions.len() >= MAX_SUGGESTIONS {
                return;
            }
        }

        let start = functions.partition_point(|f| *f < prefix_upper.as_str());
        for func in &functions[start..] {
            if !func.starts_with(prefix_upper.as_str()) {
                break;
            }
            // Skip functions that are also keywords (e.g. MAX, TO_CHAR,
            // SYSDATE); those are emitted by the keyword loop as bare names.
            if keywords.binary_search(func).is_ok() {
                continue;
            }
            let rendered = format!("{func}{FUNCTION_SUFFIX}");
            if !suggestion_matches_completion_prefix(&rendered, prefix) {
                continue;
            }
            if seen.insert(rendered.to_uppercase()) {
                suggestions.push(rendered);
            }
            if suggestions.len() >= MAX_SUGGESTIONS {
                break;
            }
        }

        if suggestions.len() < MAX_SUGGESTIONS {
            let start = dialect_functions.partition_point(|f| *f < prefix_upper.as_str());
            for func in &dialect_functions[start..] {
                if !func.starts_with(prefix_upper.as_str()) {
                    break;
                }
                let rendered = format!("{func}{FUNCTION_SUFFIX}");
                if suggestion_matches_completion_prefix(&rendered, prefix)
                    && seen.insert(rendered.to_uppercase())
                {
                    suggestions.push(rendered);
                }
                if suggestions.len() >= MAX_SUGGESTIONS {
                    break;
                }
            }
        }
    }

    pub fn get_suggestions_for_db(
        &mut self,
        prefix: &str,
        include_columns: bool,
        column_tables: Option<&[String]>,
        prefer_relations: bool,
        prefer_columns: bool,
        db_type: Option<crate::db::DatabaseType>,
    ) -> Vec<String> {
        self.ensure_base_indices();

        let prefix_upper = Self::entry_lookup_prefix_upper(prefix);
        let mut suggestions = Vec::new();
        let mut seen = HashSet::new();
        let relation_only = prefer_relations && prefix_upper.is_empty();
        let column_only = prefer_columns && prefix_upper.is_empty();

        if prefer_columns && include_columns {
            self.append_column_suggestions(
                &prefix_upper,
                prefix,
                column_tables,
                &mut suggestions,
                &mut seen,
            );
            if column_only && !suggestions.is_empty() {
                suggestions.truncate(MAX_SUGGESTIONS);
                return suggestions;
            }
        }

        // In table context, prioritize real relation names first.
        if prefer_relations {
            if Self::push_matching_entries(
                &self.table_entries,
                &prefix_upper,
                prefix,
                &mut suggestions,
                &mut seen,
            ) {
                return suggestions;
            }
            if Self::push_matching_entries(
                &self.view_entries,
                &prefix_upper,
                prefix,
                &mut suggestions,
                &mut seen,
            ) {
                return suggestions;
            }
            if Self::push_matching_entries(
                &self.materialized_view_entries,
                &prefix_upper,
                prefix,
                &mut suggestions,
                &mut seen,
            ) {
                return suggestions;
            }
            if Self::push_matching_entries(
                &self.synonym_entries,
                &prefix_upper,
                prefix,
                &mut suggestions,
                &mut seen,
            ) {
                return suggestions;
            }
            if Self::push_matching_entries(
                &self.public_synonym_entries,
                &prefix_upper,
                prefix,
                &mut suggestions,
                &mut seen,
            ) {
                return suggestions;
            }
            if relation_only && !suggestions.is_empty() {
                suggestions.truncate(MAX_SUGGESTIONS);
                return suggestions;
            }
        }

        // Add SQL keywords and built-in functions only when a non-empty prefix
        // is available.  With an empty prefix we would iterate the entire
        // sorted array and hit MAX_SUGGESTIONS before useful entries appear,
        // so skip them – the caller already has context-specific entries
        // (tables, views, columns) for empty-prefix completions.
        if !prefix_upper.is_empty() {
            Self::append_language_catalog_suggestions_for_db(
                prefix,
                prefer_columns,
                db_type,
                &mut suggestions,
                &mut seen,
            );
        }

        // A relation name is not a free expression operand. Visible relation
        // names and aliases are supplied separately from the parsed query
        // scope; adding the global relation catalog here would let unrelated
        // schemas/tables leak into SELECT/WHERE/etc. expression completion.
        if !prefer_relations && !prefer_columns {
            if Self::push_matching_entries(
                &self.table_entries,
                &prefix_upper,
                prefix,
                &mut suggestions,
                &mut seen,
            ) {
                return suggestions;
            }

            if Self::push_matching_entries(
                &self.view_entries,
                &prefix_upper,
                prefix,
                &mut suggestions,
                &mut seen,
            ) {
                return suggestions;
            }

            if Self::push_matching_entries(
                &self.materialized_view_entries,
                &prefix_upper,
                prefix,
                &mut suggestions,
                &mut seen,
            ) {
                return suggestions;
            }

            if Self::push_matching_entries(
                &self.synonym_entries,
                &prefix_upper,
                prefix,
                &mut suggestions,
                &mut seen,
            ) {
                return suggestions;
            }

            if Self::push_matching_entries(
                &self.public_synonym_entries,
                &prefix_upper,
                prefix,
                &mut suggestions,
                &mut seen,
            ) {
                return suggestions;
            }
        }

        // Add procedures. A standalone procedure returns no value and can only
        // be invoked as its own statement (`CALL`/`EXEC`/a PL/SQL call), never
        // as a token inside a value expression — only functions can. So they
        // are skipped in an expression/column context (`prefer_columns`), while
        // packages (a `pkg.func` namespace), functions, and Oracle sequences/
        // types stay available. The catalog populates `procedure_entries`
        // strictly from `object_type = 'PROCEDURE'` (functions land in
        // `function_entries`), so this never hides a callable function.
        if !prefer_columns
            && Self::push_matching_entries(
                &self.procedure_entries,
                &prefix_upper,
                prefix,
                &mut suggestions,
                &mut seen,
            )
        {
            return suggestions;
        }

        // Add packages
        if Self::push_matching_entries(
            &self.package_entries,
            &prefix_upper,
            prefix,
            &mut suggestions,
            &mut seen,
        ) {
            return suggestions;
        }

        // Add functions
        if Self::push_matching_entries(
            &self.function_entries,
            &prefix_upper,
            prefix,
            &mut suggestions,
            &mut seen,
        ) {
            return suggestions;
        }

        if !crate::sql_text::mysql_compatibility_for_sql("", db_type)
            && Self::push_matching_entries(
                &self.sequence_entries,
                &prefix_upper,
                prefix,
                &mut suggestions,
                &mut seen,
            )
        {
            return suggestions;
        }

        // `type_entries` are schema TYPE objects (Oracle object/collection types),
        // not scalar data-type keywords. MySQL/MariaDB scalar type names come
        // from the dialect keyword catalog and dedicated data-type slots; a stale
        // or synthetic Oracle TYPE object must not leak into their general
        // expression/object catalog.
        if !crate::sql_text::mysql_compatibility_for_sql("", db_type)
            && Self::push_matching_entries(
                &self.type_entries,
                &prefix_upper,
                prefix,
                &mut suggestions,
                &mut seen,
            )
        {
            return suggestions;
        }

        // Schema/DDL-only object kinds that can never appear as a token in a
        // value expression (a SELECT item, WHERE/JOIN/HAVING predicate, ORDER/
        // GROUP BY key, SET/VALUES operand, …). In an expression/column context
        // (`prefer_columns`) suggesting them is pure noise, so they are skipped
        // entirely there. They stay available in the General catalog flow
        // (`prefer_columns == false`, e.g. a `DROP`/`ALTER`/`COMMENT` head),
        // and the dedicated DDL slots (`DROP TRIGGER …`, `ALTER INDEX …`, the
        // `GRANT … ON …` object slot) surface the exact kind through
        // `expected_object_suggestion_kind`. Categorize any newly added object
        // kind here on purpose so this expression filter cannot silently drift.
        if !prefer_columns {
            if Self::push_matching_entries(
                &self.trigger_entries,
                &prefix_upper,
                prefix,
                &mut suggestions,
                &mut seen,
            ) {
                return suggestions;
            }

            if Self::push_matching_entries(
                &self.event_entries,
                &prefix_upper,
                prefix,
                &mut suggestions,
                &mut seen,
            ) {
                return suggestions;
            }

            if Self::push_matching_entries(
                &self.index_entries,
                &prefix_upper,
                prefix,
                &mut suggestions,
                &mut seen,
            ) {
                return suggestions;
            }

            for entries in [
                &self.database_link_entries,
                &self.directory_entries,
                &self.library_entries,
                &self.cluster_entries,
                &self.context_entries,
                &self.dimension_entries,
                &self.operator_entries,
                &self.indextype_entries,
                &self.edition_entries,
                &self.java_source_entries,
                &self.java_class_entries,
                &self.java_resource_entries,
            ] {
                if Self::push_matching_entries(
                    entries,
                    &prefix_upper,
                    prefix,
                    &mut suggestions,
                    &mut seen,
                ) {
                    return suggestions;
                }
            }

            let _ = Self::push_matching_entries(
                &self.user_entries,
                &prefix_upper,
                prefix,
                &mut suggestions,
                &mut seen,
            );
        }

        if include_columns && !prefer_columns {
            self.append_column_suggestions(
                &prefix_upper,
                prefix,
                column_tables,
                &mut suggestions,
                &mut seen,
            );
        }

        suggestions.truncate(MAX_SUGGESTIONS);
        suggestions
    }

    pub fn get_relation_suggestions(&mut self, prefix: &str) -> Vec<String> {
        self.ensure_base_indices();
        Self::suggestions_from_entry_groups(
            prefix,
            &[
                &self.table_entries,
                &self.view_entries,
                &self.materialized_view_entries,
                &self.synonym_entries,
                &self.public_synonym_entries,
                &self.user_entries,
            ],
        )
    }

    pub fn get_object_suggestions(&mut self, prefix: &str) -> Vec<String> {
        self.ensure_base_indices();
        Self::suggestions_from_entry_groups(
            prefix,
            &[
                &self.table_entries,
                &self.view_entries,
                &self.materialized_view_entries,
                &self.synonym_entries,
                &self.public_synonym_entries,
                &self.procedure_entries,
                &self.package_entries,
                &self.function_entries,
                &self.sequence_entries,
                &self.trigger_entries,
                &self.event_entries,
                &self.type_entries,
                &self.index_entries,
                &self.database_link_entries,
                &self.directory_entries,
                &self.library_entries,
                &self.cluster_entries,
                &self.context_entries,
                &self.dimension_entries,
                &self.operator_entries,
                &self.indextype_entries,
                &self.edition_entries,
                &self.java_source_entries,
                &self.java_class_entries,
                &self.java_resource_entries,
                &self.user_entries,
            ],
        )
    }

    pub fn get_routine_object_suggestions(&mut self, prefix: &str) -> Vec<String> {
        self.ensure_base_indices();
        Self::suggestions_from_entry_groups(
            prefix,
            &[
                &self.procedure_entries,
                &self.package_entries,
                &self.function_entries,
            ],
        )
    }

    pub fn get_executable_object_suggestions(&mut self, prefix: &str) -> Vec<String> {
        self.ensure_base_indices();
        Self::suggestions_from_entry_groups(
            prefix,
            &[
                &self.procedure_entries,
                &self.package_entries,
                &self.function_entries,
                &self.type_entries,
            ],
        )
    }

    pub fn get_relation_or_sequence_object_suggestions(&mut self, prefix: &str) -> Vec<String> {
        self.ensure_base_indices();
        Self::suggestions_from_entry_groups(
            prefix,
            &[
                &self.table_entries,
                &self.view_entries,
                &self.materialized_view_entries,
                &self.sequence_entries,
                &self.synonym_entries,
                &self.public_synonym_entries,
            ],
        )
    }

    pub fn get_column_owner_object_suggestions(&mut self, prefix: &str) -> Vec<String> {
        self.ensure_base_indices();
        Self::suggestions_from_entry_groups(
            prefix,
            &[
                &self.table_entries,
                &self.view_entries,
                &self.materialized_view_entries,
                &self.synonym_entries,
                &self.public_synonym_entries,
            ],
        )
    }

    pub fn get_table_object_suggestions(&mut self, prefix: &str) -> Vec<String> {
        self.ensure_base_indices();
        Self::suggestions_from_entry_groups(prefix, &[&self.table_entries])
    }

    pub fn get_view_object_suggestions(&mut self, prefix: &str) -> Vec<String> {
        self.ensure_base_indices();
        Self::suggestions_from_entry_groups(prefix, &[&self.view_entries])
    }

    pub fn get_materialized_view_object_suggestions(&mut self, prefix: &str) -> Vec<String> {
        self.ensure_base_indices();
        Self::suggestions_from_entry_groups(prefix, &[&self.materialized_view_entries])
    }

    pub fn get_type_object_suggestions(&mut self, prefix: &str) -> Vec<String> {
        self.ensure_base_indices();
        Self::suggestions_from_entry_groups(prefix, &[&self.type_entries])
    }

    pub fn get_trigger_object_suggestions(&mut self, prefix: &str) -> Vec<String> {
        self.ensure_base_indices();
        Self::suggestions_from_entry_groups(prefix, &[&self.trigger_entries])
    }

    pub fn get_event_object_suggestions(&mut self, prefix: &str) -> Vec<String> {
        self.ensure_base_indices();
        Self::suggestions_from_entry_groups(prefix, &[&self.event_entries])
    }

    pub fn get_index_object_suggestions(&mut self, prefix: &str) -> Vec<String> {
        self.ensure_base_indices();
        Self::suggestions_from_entry_groups(prefix, &[&self.index_entries])
    }

    pub fn get_procedure_object_suggestions(&mut self, prefix: &str) -> Vec<String> {
        self.ensure_base_indices();
        Self::suggestions_from_entry_groups(prefix, &[&self.procedure_entries])
    }

    pub fn get_function_object_suggestions(&mut self, prefix: &str) -> Vec<String> {
        self.ensure_base_indices();
        Self::suggestions_from_entry_groups(prefix, &[&self.function_entries])
    }

    pub fn get_package_object_suggestions(&mut self, prefix: &str) -> Vec<String> {
        self.ensure_base_indices();
        Self::suggestions_from_entry_groups(prefix, &[&self.package_entries])
    }

    pub fn get_oracle_builtin_package_suggestions_for_db(
        prefix: &str,
        db_type: Option<crate::db::DatabaseType>,
    ) -> Vec<String> {
        let mut suggestions = Vec::new();
        if crate::sql_text::mysql_compatibility_for_sql("", db_type) {
            return suggestions;
        }
        let prefix_upper = Self::entry_lookup_prefix_upper(prefix);
        let mut seen: HashSet<String> = HashSet::new();
        Self::append_oracle_builtin_package_suggestions(
            &prefix_upper,
            prefix,
            &mut suggestions,
            &mut seen,
        );
        suggestions
    }

    pub fn get_sequence_object_suggestions(&mut self, prefix: &str) -> Vec<String> {
        self.ensure_base_indices();
        Self::suggestions_from_entry_groups(prefix, &[&self.sequence_entries])
    }

    pub fn get_synonym_object_suggestions(&mut self, prefix: &str) -> Vec<String> {
        self.ensure_base_indices();
        Self::suggestions_from_entry_groups(prefix, &[&self.synonym_entries])
    }

    pub fn get_public_synonym_object_suggestions(&mut self, prefix: &str) -> Vec<String> {
        self.ensure_base_indices();
        Self::suggestions_from_entry_groups(prefix, &[&self.public_synonym_entries])
    }

    pub fn get_database_link_object_suggestions(&mut self, prefix: &str) -> Vec<String> {
        self.ensure_base_indices();
        Self::suggestions_from_entry_groups(prefix, &[&self.database_link_entries])
    }

    pub fn get_directory_object_suggestions(&mut self, prefix: &str) -> Vec<String> {
        self.ensure_base_indices();
        Self::suggestions_from_entry_groups(prefix, &[&self.directory_entries])
    }

    pub fn get_library_object_suggestions(&mut self, prefix: &str) -> Vec<String> {
        self.ensure_base_indices();
        Self::suggestions_from_entry_groups(prefix, &[&self.library_entries])
    }

    pub fn get_cluster_object_suggestions(&mut self, prefix: &str) -> Vec<String> {
        self.ensure_base_indices();
        Self::suggestions_from_entry_groups(prefix, &[&self.cluster_entries])
    }

    pub fn get_context_object_suggestions(&mut self, prefix: &str) -> Vec<String> {
        self.ensure_base_indices();
        Self::suggestions_from_entry_groups(prefix, &[&self.context_entries])
    }

    pub fn get_dimension_object_suggestions(&mut self, prefix: &str) -> Vec<String> {
        self.ensure_base_indices();
        Self::suggestions_from_entry_groups(prefix, &[&self.dimension_entries])
    }

    pub fn get_operator_object_suggestions(&mut self, prefix: &str) -> Vec<String> {
        self.ensure_base_indices();
        Self::suggestions_from_entry_groups(prefix, &[&self.operator_entries])
    }

    pub fn get_indextype_object_suggestions(&mut self, prefix: &str) -> Vec<String> {
        self.ensure_base_indices();
        Self::suggestions_from_entry_groups(prefix, &[&self.indextype_entries])
    }

    pub fn get_edition_object_suggestions(&mut self, prefix: &str) -> Vec<String> {
        self.ensure_base_indices();
        Self::suggestions_from_entry_groups(prefix, &[&self.edition_entries])
    }

    pub fn get_java_source_object_suggestions(&mut self, prefix: &str) -> Vec<String> {
        self.ensure_base_indices();
        Self::suggestions_from_entry_groups(prefix, &[&self.java_source_entries])
    }

    pub fn get_java_class_object_suggestions(&mut self, prefix: &str) -> Vec<String> {
        self.ensure_base_indices();
        Self::suggestions_from_entry_groups(prefix, &[&self.java_class_entries])
    }

    pub fn get_java_resource_object_suggestions(&mut self, prefix: &str) -> Vec<String> {
        self.ensure_base_indices();
        Self::suggestions_from_entry_groups(prefix, &[&self.java_resource_entries])
    }

    pub fn get_user_suggestions(&mut self, prefix: &str) -> Vec<String> {
        self.ensure_base_indices();
        Self::suggestions_from_entry_groups(prefix, &[&self.user_entries])
    }

    pub fn get_schema_suggestions(&mut self, prefix: &str) -> Vec<String> {
        self.ensure_base_indices();
        Self::suggestions_from_entry_groups(prefix, &[&self.schema_entries])
    }

    pub fn get_column_suggestions(
        &mut self,
        prefix: &str,
        column_tables: Option<&[String]>,
    ) -> Vec<String> {
        self.ensure_base_indices();

        let prefix_upper = Self::entry_lookup_prefix_upper(prefix);
        let mut suggestions = Vec::new();
        let mut seen = HashSet::new();

        self.append_column_suggestions(
            &prefix_upper,
            prefix,
            column_tables,
            &mut suggestions,
            &mut seen,
        );

        suggestions.truncate(MAX_SUGGESTIONS);
        suggestions
    }

    pub fn set_members_for_qualifier(&mut self, qualifier: &str, members: Vec<String>) {
        let key = Self::normalize_qualifier_lookup_key(qualifier);
        if key.is_empty() {
            return;
        }
        self.member_entries_by_qualifier
            .insert(key, Self::build_entries(&members));
        self.member_kinds_by_qualifier
            .remove(&Self::normalize_qualifier_lookup_key(qualifier));
    }

    pub fn set_members_for_qualifier_with_kinds(
        &mut self,
        qualifier: &str,
        members: Vec<(String, Option<QualifiedMemberKind>)>,
    ) {
        let key = Self::normalize_qualifier_lookup_key(qualifier);
        if key.is_empty() {
            return;
        }

        let mut names = Vec::with_capacity(members.len());
        let mut member_kinds: HashMap<String, HashSet<QualifiedMemberKind>> = HashMap::new();
        for (name, kind) in members {
            names.push(name.clone());
            if let Some(kind) = kind {
                member_kinds
                    .entry(NameEntry::lookup_upper(&name))
                    .or_default()
                    .insert(kind);
            }
        }

        self.member_entries_by_qualifier
            .insert(key.clone(), Self::build_entries(&names));
        if member_kinds.is_empty() {
            self.member_kinds_by_qualifier.remove(&key);
        } else {
            self.member_kinds_by_qualifier.insert(key, member_kinds);
        }
    }

    pub fn set_relation_members_for_qualifier(&mut self, qualifier: &str, members: Vec<String>) {
        let key = Self::normalize_qualifier_lookup_key(qualifier);
        if key.is_empty() {
            return;
        }
        self.relation_member_entries_by_qualifier
            .insert(key, Self::build_entries(&members));
    }

    pub fn set_collection_element_type_for_type(
        &mut self,
        collection_type: &str,
        element_type: &str,
    ) {
        let key = Self::normalize_qualifier_lookup_key(collection_type);
        let element = Self::normalize_qualifier_lookup_key(element_type);
        if key.is_empty() || element.is_empty() {
            return;
        }
        self.collection_element_type_by_type.insert(key, element);
    }

    pub fn collection_element_type_for_type(&self, collection_type: &str) -> Option<&str> {
        for key in Self::qualifier_lookup_keys(collection_type) {
            if let Some(element_type) = self.collection_element_type_by_type.get(&key) {
                return Some(element_type.as_str());
            }
        }
        None
    }

    pub fn set_synonym_target(&mut self, synonym: &str, target: &str) {
        let key = Self::normalize_qualifier_lookup_key(synonym);
        let target = Self::normalize_qualifier_lookup_key(target);
        if key.is_empty() || target.is_empty() {
            return;
        }
        self.synonym_target_by_synonym.insert(key, target);
    }

    pub fn synonym_target(&self, synonym: &str) -> Option<&str> {
        for key in Self::qualifier_lookup_keys(synonym) {
            if let Some(target) = self.synonym_target_by_synonym.get(&key) {
                return Some(target.as_str());
            }
        }
        None
    }

    pub fn has_members_for_qualifier(&self, qualifier: &str, relation_only: bool) -> bool {
        let keys = Self::qualifier_lookup_keys(qualifier);
        let source = if relation_only {
            &self.relation_member_entries_by_qualifier
        } else {
            &self.member_entries_by_qualifier
        };
        keys.iter()
            .any(|key| source.get(key).is_some_and(|entries| !entries.is_empty()))
    }

    pub fn qualifier_relation_member_matches(&self, qualifier: &str, candidate: &str) -> bool {
        let candidate_upper = NameEntry::lookup_upper(candidate.trim());
        if candidate_upper.is_empty() {
            return false;
        }

        for key in Self::qualifier_lookup_keys(qualifier) {
            if let Some(entries) = self.relation_member_entries_by_qualifier.get(&key) {
                return entries.iter().any(|entry| entry.upper == candidate_upper);
            }
        }

        false
    }

    /// Resolves either `object` in the active/default qualifier or an explicit
    /// `qualifier.object` against loaded schema-member kind metadata.
    pub fn object_name_matches_qualifier_member_kind(
        &self,
        object_name: &str,
        expected_kind: QualifiedMemberKind,
    ) -> bool {
        let mut segments = Self::normalize_qualifier_lookup_segments(object_name);
        let Some(candidate) = segments.pop() else {
            return false;
        };
        let owner = if segments.is_empty() {
            self.default_qualifier.clone()
        } else {
            Some(segments.join("."))
        };
        let Some(owner) = owner else {
            return false;
        };

        self.qualifier_member_matches_kinds(&owner, &candidate, &[expected_kind]) == Some(true)
    }

    pub fn qualifier_relation_member_name(
        &self,
        qualifier: &str,
        candidate: &str,
    ) -> Option<String> {
        let candidate_upper = NameEntry::lookup_upper(candidate.trim());
        if candidate_upper.is_empty() {
            return None;
        }

        for key in Self::qualifier_lookup_keys(qualifier) {
            if let Some(entries) = self.relation_member_entries_by_qualifier.get(&key) {
                return entries
                    .iter()
                    .find(|entry| entry.upper == candidate_upper)
                    .map(|entry| entry.name.clone());
            }
        }

        None
    }

    pub fn get_member_suggestions(
        &mut self,
        qualifier: &str,
        prefix: &str,
        relation_only: bool,
    ) -> Vec<String> {
        self.ensure_base_indices();

        let prefix_upper = Self::entry_lookup_prefix_upper(prefix);
        let mut suggestions = Vec::new();
        let mut seen = HashSet::new();

        if let Some(entries) = self.member_entries_for_qualifier(qualifier, relation_only) {
            let _ = Self::push_matching_entries(
                entries,
                &prefix_upper,
                prefix,
                &mut suggestions,
                &mut seen,
            );
            Self::append_fuzzy_entries(&[entries], &prefix_upper, &mut suggestions, &mut seen);
        }
        suggestions.truncate(MAX_SUGGESTIONS);
        suggestions
    }

    pub fn qualifier_member_matches_kinds(
        &self,
        qualifier: &str,
        candidate: &str,
        expected_kinds: &[QualifiedMemberKind],
    ) -> Option<bool> {
        if expected_kinds.is_empty() {
            return Some(true);
        }

        let candidate_upper = NameEntry::lookup_upper(candidate.trim());
        for key in Self::qualifier_lookup_keys(qualifier) {
            let Some(member_kinds) = self.member_kinds_by_qualifier.get(&key) else {
                continue;
            };
            let matches = member_kinds
                .get(&candidate_upper)
                .is_some_and(|kinds| expected_kinds.iter().any(|kind| kinds.contains(kind)));
            return Some(matches);
        }

        None
    }

    pub fn qualifier_member_has_kind_metadata(&self, qualifier: &str, candidate: &str) -> bool {
        let candidate_upper = NameEntry::lookup_upper(candidate.trim());
        if candidate_upper.is_empty() {
            return false;
        }

        for key in Self::qualifier_lookup_keys(qualifier) {
            if let Some(member_kinds) = self.member_kinds_by_qualifier.get(&key) {
                return member_kinds.contains_key(&candidate_upper);
            }
        }

        false
    }

    pub fn qualifier_has_member_kind_metadata(&self, qualifier: &str) -> bool {
        Self::qualifier_lookup_keys(qualifier).iter().any(|key| {
            self.member_kinds_by_qualifier
                .get(key)
                .is_some_and(|members| !members.is_empty())
        })
    }

    pub fn qualifier_member_name_matching_kinds(
        &self,
        qualifier: &str,
        candidate: &str,
        expected_kinds: &[QualifiedMemberKind],
    ) -> Option<String> {
        let candidate_upper = NameEntry::lookup_upper(candidate.trim());
        if candidate_upper.is_empty() || expected_kinds.is_empty() {
            return None;
        }

        for key in Self::qualifier_lookup_keys(qualifier) {
            let Some(member_kinds) = self.member_kinds_by_qualifier.get(&key) else {
                continue;
            };
            if !member_kinds
                .get(&candidate_upper)
                .is_some_and(|kinds| expected_kinds.iter().any(|kind| kinds.contains(kind)))
            {
                return None;
            }

            return self
                .member_entries_by_qualifier
                .get(&key)
                .and_then(|entries| {
                    entries
                        .iter()
                        .find(|entry| entry.upper == candidate_upper)
                        .map(|entry| entry.name.clone())
                })
                .or_else(|| Some(candidate.trim().to_string()));
        }

        None
    }

    pub fn qualifier_member_type_label(
        &self,
        qualifier: &str,
        candidate: &str,
    ) -> Option<&'static str> {
        const LABEL_PRIORITY: &[QualifiedMemberKind] = &[
            QualifiedMemberKind::Table,
            QualifiedMemberKind::View,
            QualifiedMemberKind::MaterializedView,
            QualifiedMemberKind::Type,
            QualifiedMemberKind::Trigger,
            QualifiedMemberKind::Event,
            QualifiedMemberKind::Index,
            QualifiedMemberKind::Procedure,
            QualifiedMemberKind::Function,
            QualifiedMemberKind::Package,
            QualifiedMemberKind::Sequence,
            QualifiedMemberKind::PublicSynonym,
            QualifiedMemberKind::Synonym,
            QualifiedMemberKind::DatabaseLink,
            QualifiedMemberKind::Directory,
            QualifiedMemberKind::Library,
            QualifiedMemberKind::Cluster,
            QualifiedMemberKind::Context,
            QualifiedMemberKind::Dimension,
            QualifiedMemberKind::Operator,
            QualifiedMemberKind::Indextype,
            QualifiedMemberKind::Edition,
            QualifiedMemberKind::JavaSource,
            QualifiedMemberKind::JavaClass,
            QualifiedMemberKind::JavaResource,
            QualifiedMemberKind::User,
        ];

        let candidate_upper = NameEntry::lookup_upper(candidate.trim());
        if candidate_upper.is_empty() {
            return None;
        }

        for key in Self::qualifier_lookup_keys(qualifier) {
            let Some(member_kinds) = self.member_kinds_by_qualifier.get(&key) else {
                continue;
            };
            let kinds = member_kinds.get(&candidate_upper)?;
            return LABEL_PRIORITY
                .iter()
                .find(|kind| kinds.contains(kind))
                .map(|kind| kind.type_label());
        }

        None
    }

    pub fn set_default_qualifier(&mut self, qualifier: Option<String>) {
        let original = qualifier
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_string);
        let normalized = original
            .as_deref()
            .map(Self::normalize_qualifier_lookup_key)
            .filter(|value| !value.is_empty());
        self.default_qualifier_name = if normalized.is_some() { original } else { None };
        self.default_qualifier = normalized;
    }

    pub fn activate_default_qualifier(&mut self, qualifier: &str) -> bool {
        let key = Self::normalize_qualifier_lookup_key(qualifier);
        if key.is_empty() {
            return false;
        }

        let Some(entries) = self.member_entries_by_qualifier.get(&key).cloned() else {
            return false;
        };
        let member_kinds = self
            .member_kinds_by_qualifier
            .get(&key)
            .cloned()
            .unwrap_or_default();

        let mut tables = Vec::new();
        let mut views = Vec::new();
        let mut materialized_views = Vec::new();
        let mut types = Vec::new();
        let mut triggers = Vec::new();
        let mut events = Vec::new();
        let mut indexes = Vec::new();
        let mut procedures = Vec::new();
        let mut functions = Vec::new();
        let mut packages = Vec::new();
        let mut sequences = Vec::new();
        let mut synonyms = Vec::new();
        let mut database_links = Vec::new();
        let mut directories = Vec::new();
        let mut libraries = Vec::new();
        let mut clusters = Vec::new();
        let mut contexts = Vec::new();
        let mut dimensions = Vec::new();
        let mut operators = Vec::new();
        let mut indextypes = Vec::new();
        let mut editions = Vec::new();
        let mut java_sources = Vec::new();
        let mut java_classes = Vec::new();
        let mut java_resources = Vec::new();

        for entry in entries {
            let Some(kinds) = member_kinds.get(&entry.upper) else {
                continue;
            };
            for kind in kinds {
                match kind {
                    QualifiedMemberKind::Table => tables.push(entry.name.clone()),
                    QualifiedMemberKind::View => views.push(entry.name.clone()),
                    QualifiedMemberKind::MaterializedView => {
                        materialized_views.push(entry.name.clone())
                    }
                    QualifiedMemberKind::Type => types.push(entry.name.clone()),
                    QualifiedMemberKind::Trigger => triggers.push(entry.name.clone()),
                    QualifiedMemberKind::Event => events.push(entry.name.clone()),
                    QualifiedMemberKind::Index => indexes.push(entry.name.clone()),
                    QualifiedMemberKind::Procedure => procedures.push(entry.name.clone()),
                    QualifiedMemberKind::Function => functions.push(entry.name.clone()),
                    QualifiedMemberKind::Package => packages.push(entry.name.clone()),
                    QualifiedMemberKind::Sequence => sequences.push(entry.name.clone()),
                    QualifiedMemberKind::Synonym => synonyms.push(entry.name.clone()),
                    QualifiedMemberKind::DatabaseLink => database_links.push(entry.name.clone()),
                    QualifiedMemberKind::Directory => directories.push(entry.name.clone()),
                    QualifiedMemberKind::Library => libraries.push(entry.name.clone()),
                    QualifiedMemberKind::Cluster => clusters.push(entry.name.clone()),
                    QualifiedMemberKind::Context => contexts.push(entry.name.clone()),
                    QualifiedMemberKind::Dimension => dimensions.push(entry.name.clone()),
                    QualifiedMemberKind::Operator => operators.push(entry.name.clone()),
                    QualifiedMemberKind::Indextype => indextypes.push(entry.name.clone()),
                    QualifiedMemberKind::Edition => editions.push(entry.name.clone()),
                    QualifiedMemberKind::JavaSource => java_sources.push(entry.name.clone()),
                    QualifiedMemberKind::JavaClass => java_classes.push(entry.name.clone()),
                    QualifiedMemberKind::JavaResource => java_resources.push(entry.name.clone()),
                    QualifiedMemberKind::PublicSynonym | QualifiedMemberKind::User => {}
                }
            }
        }

        if tables.is_empty()
            && views.is_empty()
            && materialized_views.is_empty()
            && procedures.is_empty()
            && functions.is_empty()
            && packages.is_empty()
            && sequences.is_empty()
            && synonyms.is_empty()
            && database_links.is_empty()
            && directories.is_empty()
            && libraries.is_empty()
            && clusters.is_empty()
            && contexts.is_empty()
            && dimensions.is_empty()
            && operators.is_empty()
            && indextypes.is_empty()
            && editions.is_empty()
            && java_sources.is_empty()
            && java_classes.is_empty()
            && java_resources.is_empty()
        {
            if let Some(relation_entries) = self.relation_member_entries_by_qualifier.get(&key) {
                tables.extend(relation_entries.iter().map(|entry| entry.name.clone()));
            }
        }

        self.default_qualifier = Some(key);
        self.default_qualifier_name = Some(qualifier.trim().to_string());
        self.tables = tables;
        self.views = views;
        self.materialized_views = materialized_views;
        self.types = types;
        self.triggers = triggers;
        self.events = events;
        self.indexes = indexes;
        self.procedures = procedures;
        self.functions = functions;
        self.packages = packages;
        self.sequences = sequences;
        self.synonyms = synonyms;
        self.database_links = database_links;
        self.directories = directories;
        self.libraries = libraries;
        self.clusters = clusters;
        self.contexts = contexts;
        self.dimensions = dimensions;
        self.operators = operators;
        self.indextypes = indextypes;
        self.editions = editions;
        self.java_sources = java_sources;
        self.java_classes = java_classes;
        self.java_resources = java_resources;
        self.rebuild_indices();
        true
    }

    pub fn default_qualifier(&self) -> Option<&str> {
        self.default_qualifier.as_deref()
    }

    pub fn default_qualifier_name(&self) -> Option<&str> {
        self.default_qualifier_name.as_deref()
    }

    pub fn canonical_qualifier_name(&self, qualifier: &str) -> Option<String> {
        let qualifier = qualifier.trim();
        if qualifier.is_empty() {
            return None;
        }

        let mut exact_matches = self
            .users
            .iter()
            .filter(|schema| schema.as_str() == qualifier);
        match (exact_matches.next(), exact_matches.next()) {
            (Some(schema), None) => return Some(schema.clone()),
            (Some(_), Some(_)) => return None,
            _ => {}
        }

        let mut matches = self
            .users
            .iter()
            .filter(|schema| schema.eq_ignore_ascii_case(qualifier));
        match (matches.next(), matches.next()) {
            (Some(schema), None) => Some(schema.clone()),
            (None, None) => self
                .default_qualifier_name()
                .filter(|default| default.eq_ignore_ascii_case(qualifier))
                .map(str::to_string),
            _ => None,
        }
    }

    pub fn qualifier_has_member(
        &self,
        qualifier: &str,
        candidate: &str,
        relation_only: bool,
    ) -> bool {
        let candidate_upper = NameEntry::lookup_upper(candidate.trim());
        if candidate_upper.is_empty() {
            return false;
        }

        self.member_entries_for_qualifier(qualifier, relation_only)
            .is_some_and(|entries| entries.iter().any(|entry| entry.upper == candidate_upper))
    }

    fn unqualified_relation_lookup_segment(table: &str) -> Option<String> {
        let trimmed = table.trim();
        if trimmed.is_empty() {
            return None;
        }

        if sql_text::is_quoted_identifier(trimmed) {
            let unquoted = sql_text::strip_identifier_quotes(trimmed);
            let segment = unquoted.trim();
            if segment.is_empty() || segment.contains('.') {
                return None;
            }
            return Some(segment.to_string());
        }
        if let Some(unquoted) = Self::strip_bracket_identifier(trimmed) {
            let segment = unquoted.trim();
            if segment.is_empty() || segment.contains('.') {
                return None;
            }
            return Some(segment.to_string());
        }

        if trimmed.contains('.') {
            None
        } else {
            Some(trimmed.to_string())
        }
    }

    fn relation_lookup_segments(table: &str) -> Option<Vec<String>> {
        let mut segments = Vec::new();
        let mut current = String::new();
        let mut active_quote = None;
        let mut chars = table.trim().chars().peekable();

        while let Some(ch) = chars.next() {
            if let Some(delimiter) = active_quote {
                current.push(ch);
                if ch == delimiter {
                    if chars.peek() == Some(&delimiter) {
                        current.push(chars.next().unwrap_or(delimiter));
                    } else {
                        active_quote = None;
                    }
                }
                continue;
            }

            match ch {
                '"' | '`' => {
                    active_quote = Some(ch);
                    current.push(ch);
                }
                '[' => {
                    active_quote = Some(']');
                    current.push(ch);
                }
                '.' => {
                    segments.push(Self::relation_lookup_segment(current.trim())?);
                    current.clear();
                }
                _ => current.push(ch),
            }
        }

        if active_quote.is_some() {
            return None;
        }

        segments.push(Self::relation_lookup_segment(current.trim())?);
        Some(segments)
    }

    fn relation_lookup_segment(segment: &str) -> Option<String> {
        let trimmed = segment.trim();
        if trimmed.is_empty() {
            return None;
        }

        let normalized = if sql_text::is_quoted_identifier(trimmed) {
            sql_text::strip_identifier_quotes(trimmed)
        } else if let Some(unquoted) = Self::strip_bracket_identifier(trimmed) {
            unquoted
        } else {
            trimmed.to_string()
        };
        if normalized.trim().is_empty() {
            None
        } else {
            Some(normalized)
        }
    }

    fn relation_lookup_exact_key(table: &str) -> Option<String> {
        let segments = Self::relation_lookup_segments(table)?;
        Some(segments.join(".").to_ascii_uppercase())
    }

    fn default_qualified_relation_key(&self, table: &str) -> Option<String> {
        let table = Self::unqualified_relation_lookup_segment(table)?;
        let qualifier = self.default_qualifier()?;
        if self.qualifier_has_member(qualifier, &table, true)
            || self.qualifier_has_member(qualifier, &table, false)
        {
            Some(format!("{}.{}", qualifier, table).to_ascii_uppercase())
        } else {
            None
        }
    }

    fn column_entries_for_scope_table(&self, table: &str) -> Option<&[NameEntry]> {
        let key = table.to_uppercase();
        if let Some(entries) = self.column_entries_for_exact_key(&key) {
            return Some(entries);
        }
        if let Some(exact_key) = Self::relation_lookup_exact_key(table) {
            if exact_key != key {
                if let Some(entries) = self.column_entries_for_exact_key(&exact_key) {
                    return Some(entries);
                }
            }
        }
        if let Some(default_key) = self.default_qualified_relation_key(table) {
            if let Some(entries) = self.column_entries_for_exact_key(&default_key) {
                return Some(entries);
            }
        }
        None
    }

    fn column_entries_for_exact_key(&self, key: &str) -> Option<&[NameEntry]> {
        self.virtual_column_entries_by_table
            .get(key)
            .map(Vec::as_slice)
            .or_else(|| self.column_entries_by_table.get(key).map(Vec::as_slice))
    }

    fn column_names_for_exact_key(&self, key: &str) -> Option<Vec<String>> {
        if let Some(columns) = self.virtual_column_entries_by_table.get(key) {
            return Some(columns.iter().map(|entry| entry.name.clone()).collect());
        }
        self.columns.get(key).cloned()
    }

    fn append_column_suggestions(
        &mut self,
        prefix_upper: &str,
        raw_prefix: &str,
        column_tables: Option<&[String]>,
        suggestions: &mut Vec<String>,
        seen: &mut HashSet<String>,
    ) {
        let Some(tables) = column_tables.filter(|tables| !tables.is_empty()) else {
            return;
        };

        for table in tables {
            if let Some(cols) = self.column_entries_for_scope_table(table) {
                if Self::push_matching_entries(cols, prefix_upper, raw_prefix, suggestions, seen) {
                    break;
                }
            }
        }
        let groups: Vec<&[NameEntry]> = tables
            .iter()
            .filter_map(|table| self.column_entries_for_scope_table(table))
            .collect();
        Self::append_fuzzy_entries(&groups, prefix_upper, suggestions, seen);
    }

    #[allow(dead_code)]
    pub fn get_columns_for_table(&self, table_name: &str) -> Vec<String> {
        let key = table_name.to_uppercase();
        if let Some(columns) = self.column_names_for_exact_key(&key) {
            return columns;
        }
        if let Some(exact_key) = Self::relation_lookup_exact_key(table_name) {
            if exact_key != key {
                if let Some(columns) = self.column_names_for_exact_key(&exact_key) {
                    return columns;
                }
            }
        }
        if let Some(default_key) = self.default_qualified_relation_key(table_name) {
            if let Some(columns) = self.column_names_for_exact_key(&default_key) {
                return columns;
            }
        }
        Vec::new()
    }

    pub fn get_all_columns_for_highlighting(&self) -> Vec<String> {
        let mut seen: HashSet<&str> = HashSet::new();
        let mut columns = Vec::new();

        for (table, entries) in &self.column_entries_by_table {
            if self.virtual_column_entries_by_table.contains_key(table) {
                continue;
            }
            for entry in entries {
                if seen.insert(entry.upper.as_str()) {
                    columns.push(entry.name.clone());
                }
            }
        }

        for names in self.virtual_column_entries_by_table.values() {
            for entry in names {
                if seen.insert(entry.upper.as_str()) {
                    columns.push(entry.name.clone());
                }
            }
        }

        columns
    }

    pub fn set_columns_for_table(&mut self, table_name: &str, columns: Vec<String>) {
        let key = table_name.to_uppercase();
        self.columns_loading.remove(&key);
        self.column_loading_started_at.remove(&key);
        let entries = Self::build_entries(&columns);
        self.columns.insert(key.clone(), columns);
        self.column_entries_by_table.insert(key, entries);
    }

    /// Store display metadata for a table's columns, keyed by normalized
    /// column lookup name. Replaces any previous metadata for the table.
    pub fn set_column_meta_for_table(
        &mut self,
        table_name: &str,
        meta: HashMap<String, ColumnMeta>,
    ) {
        let key = table_name.to_uppercase();
        let meta = meta
            .into_iter()
            .map(|(column, meta)| (NameEntry::lookup_upper(&column), meta))
            .collect();
        self.column_meta_by_table.insert(key, meta);
    }

    /// Look up display metadata for a single column using an exact normalized
    /// table key, or the verified default qualifier for an unqualified table.
    pub fn get_column_meta(&self, table_name: &str, column_name: &str) -> Option<&ColumnMeta> {
        let column_key = NameEntry::lookup_upper(column_name);
        let table_key = table_name.to_uppercase();
        if let Some(meta) = self
            .column_meta_by_table
            .get(&table_key)
            .and_then(|cols| cols.get(&column_key))
        {
            return Some(meta);
        }
        if let Some(exact_key) = Self::relation_lookup_exact_key(table_name) {
            if exact_key != table_key {
                if let Some(meta) = self
                    .column_meta_by_table
                    .get(&exact_key)
                    .and_then(|cols| cols.get(&column_key))
                {
                    return Some(meta);
                }
            }
        }
        if let Some(default_key) = self.default_qualified_relation_key(table_name) {
            if let Some(meta) = self
                .column_meta_by_table
                .get(&default_key)
                .and_then(|cols| cols.get(&column_key))
            {
                return Some(meta);
            }
        }
        None
    }

    /// Store foreign-key relationships for a table and clear its loading flag.
    pub fn set_foreign_keys_for_table(&mut self, table_name: &str, fks: Vec<ForeignKeyMeta>) {
        let key = table_name.to_uppercase();
        self.foreign_keys_loading.remove(&key);
        self.foreign_keys_by_table.insert(key, fks);
    }

    /// Mark a table's foreign keys as loading. Returns `false` when already
    /// loaded or already in flight (caller should not enqueue another fetch).
    pub fn mark_foreign_keys_loading(&mut self, table_key: &str) -> bool {
        let key = table_key.to_uppercase();
        if self.foreign_keys_by_table.contains_key(&key) || self.foreign_keys_loading.contains(&key)
        {
            return false;
        }
        self.foreign_keys_loading.insert(key);
        true
    }

    /// Clear a table's foreign-key loading flag without caching a result.
    pub fn clear_foreign_keys_loading(&mut self, table_key: &str) {
        self.foreign_keys_loading.remove(&table_key.to_uppercase());
    }

    /// Resolve the stored foreign-key key for a table reference using an exact
    /// normalized key, or the verified default qualifier for an unqualified
    /// relation.
    fn resolve_foreign_keys_key(&self, table_name: &str) -> Option<String> {
        let key = table_name.to_uppercase();
        if self.foreign_keys_by_table.contains_key(&key) {
            return Some(key);
        }
        if let Some(exact_key) = Self::relation_lookup_exact_key(table_name) {
            if exact_key != key && self.foreign_keys_by_table.contains_key(&exact_key) {
                return Some(exact_key);
            }
        }
        if let Some(default_key) = self.default_qualified_relation_key(table_name) {
            if self.foreign_keys_by_table.contains_key(&default_key) {
                return Some(default_key);
            }
        }
        None
    }

    /// Foreign keys declared on a table, if loaded.
    pub fn get_foreign_keys(&self, table_name: &str) -> Option<&[ForeignKeyMeta]> {
        let key = self.resolve_foreign_keys_key(table_name)?;
        self.foreign_keys_by_table.get(&key).map(Vec::as_slice)
    }

    /// Best-effort kind label for a completion suggestion, used to fill the
    /// popup's type column for non-column entries. Checks loaded object stores
    /// first, then the static keyword/function catalog. Returns `None` for
    /// names it cannot classify (columns carry their own type via metadata).
    pub fn suggestion_type_label(
        &self,
        name: &str,
        db_type: Option<crate::db::DatabaseType>,
    ) -> Option<&'static str> {
        // Built-in functions are rendered with a trailing "()".
        if let Some(stripped) = name.strip_suffix(FUNCTION_SUFFIX) {
            if is_builtin_function(stripped) {
                return Some("FUNCTION");
            }
        }
        let upper = NameEntry::lookup_upper(name);
        let has = |entries: &[NameEntry]| {
            entries
                .binary_search_by(|entry| entry.upper.as_str().cmp(upper.as_str()))
                .is_ok()
        };
        let label = if has(&self.table_entries) {
            "TABLE"
        } else if has(&self.view_entries) {
            "VIEW"
        } else if has(&self.materialized_view_entries) {
            "MVIEW"
        } else if has(&self.public_synonym_entries) {
            "PUBLIC SYNONYM"
        } else if has(&self.synonym_entries) {
            "SYNONYM"
        } else if has(&self.procedure_entries) {
            "PROCEDURE"
        } else if has(&self.package_entries)
            || (!crate::sql_text::mysql_compatibility_for_sql("", db_type)
                && Self::is_oracle_builtin_package(&upper))
        {
            "PACKAGE"
        } else if has(&self.function_entries) {
            "FUNCTION"
        } else if has(&self.sequence_entries) {
            "SEQUENCE"
        } else if has(&self.type_entries) {
            "TYPE"
        } else if has(&self.trigger_entries) {
            "TRIGGER"
        } else if has(&self.event_entries) {
            "EVENT"
        } else if has(&self.index_entries) {
            "INDEX"
        } else if has(&self.database_link_entries) {
            "DATABASE LINK"
        } else if has(&self.directory_entries) {
            "DIRECTORY"
        } else if has(&self.library_entries) {
            "LIBRARY"
        } else if has(&self.cluster_entries) {
            "CLUSTER"
        } else if has(&self.context_entries) {
            "CONTEXT"
        } else if has(&self.dimension_entries) {
            "DIMENSION"
        } else if has(&self.operator_entries) {
            "OPERATOR"
        } else if has(&self.indextype_entries) {
            "INDEXTYPE"
        } else if has(&self.edition_entries) {
            "EDITION"
        } else if has(&self.java_source_entries) {
            "JAVA SOURCE"
        } else if has(&self.java_class_entries) {
            "JAVA CLASS"
        } else if has(&self.java_resource_entries) {
            "JAVA RESOURCE"
        } else if has(&self.user_entries) {
            "USER"
        } else if self.is_catalog_keyword(upper.as_str(), db_type) {
            "KEYWORD"
        } else {
            return None;
        };
        Some(label)
    }

    fn is_catalog_keyword(&self, upper: &str, db_type: Option<crate::db::DatabaseType>) -> bool {
        let (keywords, _functions, _dialect_functions) = language_catalog_for_db_type(db_type);
        keywords.binary_search(&upper).is_ok()
    }

    /// True when `upper` (an upper-cased token) is a reserved language keyword for
    /// the dialect. Lets IntelliSense tell a base-catalog keyword apart from a
    /// column/object name when filtering expression completions.
    pub(crate) fn is_language_keyword(
        &self,
        upper: &str,
        db_type: Option<crate::db::DatabaseType>,
    ) -> bool {
        self.is_catalog_keyword(upper, db_type)
    }

    /// True when `upper` names a built-in function for the dialect. Such keywords
    /// are value-producing, so they stay valid wherever an operand is expected.
    pub(crate) fn is_language_function(
        &self,
        upper: &str,
        db_type: Option<crate::db::DatabaseType>,
    ) -> bool {
        let (_keywords, functions, dialect_functions) = language_catalog_for_db_type(db_type);
        functions.binary_search(&upper).is_ok() || dialect_functions.binary_search(&upper).is_ok()
    }

    /// Cached signature for a routine key: `Some(Some(label))` when resolved to
    /// a routine, `Some(None)` when resolved but not callable, `None` when not
    /// yet fetched.
    pub fn cached_signature(&self, key: &str) -> Option<&Option<SignatureLabel>> {
        self.signature_cache.get(key)
    }

    /// Mark a routine key as having an in-flight fetch. Returns `false` when it
    /// is already cached or already pending (caller should not fetch again).
    pub fn mark_signature_pending(&mut self, key: &str) -> bool {
        if self.signature_cache.contains_key(key) || self.signature_pending.contains(key) {
            return false;
        }
        self.signature_pending.insert(key.to_string());
        true
    }

    /// Store a fetched signature result and clear its pending flag.
    pub fn set_signature(&mut self, key: String, label: Option<SignatureLabel>) {
        self.signature_pending.remove(&key);
        if !self.signature_cache.contains_key(&key) {
            if self.signature_cache.len() >= MAX_SIGNATURE_CACHE_ENTRIES {
                while let Some(oldest) = self.signature_cache_order.pop_front() {
                    if self.signature_cache.remove(&oldest).is_some() {
                        break;
                    }
                }
            }
            self.signature_cache_order.push_back(key.clone());
        }
        self.signature_cache.insert(key, label);
    }

    /// Clear the pending flag for a key without caching a result, allowing a
    /// later retry (used when a fetch failed transiently).
    pub fn clear_signature_pending(&mut self, key: &str) {
        self.signature_pending.remove(key);
    }

    pub fn mark_columns_loading(&mut self, table_name: &str) -> bool {
        let key = table_name.to_uppercase();
        if self.columns.contains_key(&key) || self.columns_loading.contains(&key) {
            return false;
        }
        self.columns_loading.insert(key.clone());
        self.column_loading_started_at.insert(key, Instant::now());
        true
    }

    pub fn clear_columns_loading(&mut self, table_name: &str) {
        let key = table_name.to_uppercase();
        self.columns_loading.remove(&key);
        self.column_loading_started_at.remove(&key);
    }

    pub fn clear_stale_columns_loading(&mut self, stale_after: Duration) -> usize {
        let now = Instant::now();
        let stale_keys: Vec<String> = self
            .columns_loading
            .iter()
            .filter(|key| {
                self.column_loading_started_at
                    .get(*key)
                    .is_none_or(|started| now.duration_since(*started) >= stale_after)
            })
            .cloned()
            .collect();

        let stale_count = stale_keys.len();
        for key in stale_keys {
            self.columns_loading.remove(&key);
            self.column_loading_started_at.remove(&key);
        }
        stale_count
    }

    pub fn is_known_relation(&self, name: &str) -> bool {
        let upper = name.to_uppercase();
        if !self.relations_upper.is_empty() {
            return self.relations_upper.contains(&upper);
        }
        self.tables.iter().any(|t| t.eq_ignore_ascii_case(&upper))
            || self.views.iter().any(|v| v.eq_ignore_ascii_case(&upper))
            || self
                .materialized_views
                .iter()
                .any(|v| v.eq_ignore_ascii_case(&upper))
            || self.synonyms.iter().any(|v| v.eq_ignore_ascii_case(&upper))
            || self
                .public_synonyms
                .iter()
                .any(|v| v.eq_ignore_ascii_case(&upper))
    }

    pub fn rebuild_indices(&mut self) {
        self.table_entries = Self::build_entries(&self.tables);
        self.view_entries = Self::build_entries(&self.views);
        self.materialized_view_entries = Self::build_entries(&self.materialized_views);
        self.type_entries = Self::build_entries(&self.types);
        self.trigger_entries = Self::build_entries(&self.triggers);
        self.event_entries = Self::build_entries(&self.events);
        self.index_entries = Self::build_entries(&self.indexes);
        self.procedure_entries = Self::build_entries(&self.procedures);
        self.function_entries = Self::build_entries(&self.functions);
        self.package_entries = Self::build_entries(&self.packages);
        self.sequence_entries = Self::build_entries(&self.sequences);
        self.synonym_entries = Self::build_entries(&self.synonyms);
        self.public_synonym_entries = Self::build_entries(&self.public_synonyms);
        self.database_link_entries = Self::build_entries(&self.database_links);
        self.directory_entries = Self::build_entries(&self.directories);
        self.library_entries = Self::build_entries(&self.libraries);
        self.cluster_entries = Self::build_entries(&self.clusters);
        self.context_entries = Self::build_entries(&self.contexts);
        self.dimension_entries = Self::build_entries(&self.dimensions);
        self.operator_entries = Self::build_entries(&self.operators);
        self.indextype_entries = Self::build_entries(&self.indextypes);
        self.edition_entries = Self::build_entries(&self.editions);
        self.java_source_entries = Self::build_entries(&self.java_sources);
        self.java_class_entries = Self::build_entries(&self.java_classes);
        self.java_resource_entries = Self::build_entries(&self.java_resources);
        self.schema_entries = Self::build_entries(&self.schemas);
        self.user_entries = Self::build_entries(&self.users);
        self.relations_upper = self
            .tables
            .iter()
            .chain(self.views.iter())
            .chain(self.materialized_views.iter())
            .chain(self.synonyms.iter())
            .chain(self.public_synonyms.iter())
            .map(|name| name.to_uppercase())
            .collect();
        self.column_entries_by_table.clear();
        self.columns_loading.clear();
        self.column_loading_started_at.clear();
        for (table, columns) in &self.columns {
            self.column_entries_by_table
                .insert(table.clone(), Self::build_entries(columns));
        }
        self.virtual_column_entries_by_table.clear();
        self.virtual_table_keys.clear();
    }

    /// Clear previously inferred virtual table columns (CTEs, subquery aliases).
    /// These may be stale because the user edited the SQL text.
    #[allow(dead_code)]
    pub fn clear_virtual_tables(&mut self) {
        for key in self.virtual_table_keys.drain() {
            self.virtual_column_entries_by_table.remove(&key);
        }
    }

    /// Register columns for a virtual table (CTE or subquery alias).
    /// These are text-derived columns, not loaded from the database.
    #[allow(dead_code)]
    pub fn set_virtual_table_columns(&mut self, name: &str, columns: Vec<String>) {
        let key = name.to_uppercase();
        self.virtual_column_entries_by_table
            .insert(key.clone(), Self::build_entries(&columns));
        self.virtual_table_keys.insert(key);
    }

    /// Replace all inferred virtual table columns with the provided set while
    /// preserving entry vectors whose derived columns have not changed.
    pub fn replace_virtual_table_columns(&mut self, virtual_columns: HashMap<String, Vec<String>>) {
        let next_keys: HashSet<String> = virtual_columns
            .keys()
            .map(|name| name.to_uppercase())
            .collect();

        let stale_keys: Vec<String> = self
            .virtual_table_keys
            .iter()
            .filter(|key| !next_keys.contains(*key))
            .cloned()
            .collect();
        for key in stale_keys {
            self.virtual_table_keys.remove(&key);
            self.virtual_column_entries_by_table.remove(&key);
        }

        for (name, columns) in virtual_columns {
            let key = name.to_uppercase();
            let entries = Self::build_entries(&columns);
            let is_same = self
                .virtual_column_entries_by_table
                .get(&key)
                .is_some_and(|existing| existing == &entries);
            if !is_same {
                self.virtual_column_entries_by_table
                    .insert(key.clone(), entries);
            }
            self.virtual_table_keys.insert(key);
        }
    }

    fn ensure_base_indices(&mut self) {
        if self.table_entries.len() != self.tables.len()
            || self.view_entries.len() != self.views.len()
            || self.materialized_view_entries.len() != self.materialized_views.len()
            || self.type_entries.len() != self.types.len()
            || self.trigger_entries.len() != self.triggers.len()
            || self.event_entries.len() != self.events.len()
            || self.index_entries.len() != self.indexes.len()
            || self.procedure_entries.len() != self.procedures.len()
            || self.function_entries.len() != self.functions.len()
            || self.package_entries.len() != self.packages.len()
            || self.sequence_entries.len() != self.sequences.len()
            || self.synonym_entries.len() != self.synonyms.len()
            || self.public_synonym_entries.len() != self.public_synonyms.len()
            || self.database_link_entries.len() != self.database_links.len()
            || self.directory_entries.len() != self.directories.len()
            || self.library_entries.len() != self.libraries.len()
            || self.cluster_entries.len() != self.clusters.len()
            || self.context_entries.len() != self.contexts.len()
            || self.dimension_entries.len() != self.dimensions.len()
            || self.operator_entries.len() != self.operators.len()
            || self.indextype_entries.len() != self.indextypes.len()
            || self.edition_entries.len() != self.editions.len()
            || self.java_source_entries.len() != self.java_sources.len()
            || self.java_class_entries.len() != self.java_classes.len()
            || self.java_resource_entries.len() != self.java_resources.len()
            || self.schema_entries.len() != self.schemas.len()
            || self.user_entries.len() != self.users.len()
        {
            self.rebuild_indices();
        }
    }

    fn member_entries_for_qualifier(
        &self,
        qualifier: &str,
        relation_only: bool,
    ) -> Option<&[NameEntry]> {
        let keys = Self::qualifier_lookup_keys(qualifier);
        let source = if relation_only {
            &self.relation_member_entries_by_qualifier
        } else {
            &self.member_entries_by_qualifier
        };

        for key in &keys {
            if let Some(entries) = source.get(key) {
                return Some(entries.as_slice());
            }
        }
        None
    }

    fn suggestions_from_entry_groups(prefix: &str, groups: &[&[NameEntry]]) -> Vec<String> {
        let prefix_upper = Self::entry_lookup_prefix_upper(prefix);
        let mut suggestions = Vec::new();
        let mut seen = HashSet::new();

        for group in groups {
            if Self::push_matching_entries(
                group,
                &prefix_upper,
                prefix,
                &mut suggestions,
                &mut seen,
            ) {
                break;
            }
        }
        Self::append_fuzzy_entries(groups, &prefix_upper, &mut suggestions, &mut seen);

        suggestions.truncate(MAX_SUGGESTIONS);
        suggestions
    }

    fn build_entries(names: &[String]) -> Vec<NameEntry> {
        let mut entries: Vec<NameEntry> = names.iter().cloned().map(NameEntry::new).collect();
        entries.sort_by(|a, b| a.upper.cmp(&b.upper).then_with(|| a.name.cmp(&b.name)));
        entries
    }

    fn entry_lookup_prefix_upper(prefix: &str) -> String {
        let trimmed = prefix.trim();
        if sql_text::is_quoted_identifier(trimmed) {
            return sql_text::strip_identifier_quotes(trimmed).to_uppercase();
        }
        if trimmed.starts_with('[') && trimmed.ends_with(']') && trimmed.len() >= 2 {
            return trimmed[1..trimmed.len().saturating_sub(1)]
                .replace("]]", "]")
                .to_uppercase();
        }

        match trimmed.chars().next() {
            Some('"') | Some('`') | Some('[') => trimmed[1..].to_uppercase(),
            _ => prefix.to_uppercase(),
        }
    }

    fn normalize_qualifier_lookup_segments(qualifier: &str) -> Vec<String> {
        let mut segments = Vec::new();
        let mut current = String::new();
        let mut active_quote = None;
        let mut chars = qualifier.chars().peekable();

        while let Some(ch) = chars.next() {
            if let Some(delimiter) = active_quote {
                current.push(ch);
                if ch == delimiter {
                    if chars.peek() == Some(&delimiter) {
                        current.push(chars.next().unwrap_or(delimiter));
                    } else {
                        active_quote = None;
                    }
                }
                continue;
            }

            match ch {
                '"' | '`' => {
                    active_quote = Some(ch);
                    current.push(ch);
                }
                '[' => {
                    active_quote = Some(']');
                    current.push(ch);
                }
                '.' => {
                    let segment = current.trim();
                    if !segment.is_empty() {
                        segments.push(Self::normalize_qualifier_lookup_segment(segment));
                    }
                    current.clear();
                }
                _ => current.push(ch),
            }
        }

        let segment = current.trim();
        if !segment.is_empty() {
            segments.push(Self::normalize_qualifier_lookup_segment(segment));
        }
        segments
    }

    fn normalize_qualifier_lookup_segment(segment: &str) -> String {
        if sql_text::is_quoted_identifier(segment) {
            let unquoted = sql_text::strip_identifier_quotes(segment);
            unquoted.to_ascii_uppercase()
        } else if let Some(unquoted) = Self::strip_bracket_identifier(segment) {
            unquoted.to_ascii_uppercase()
        } else {
            segment.to_ascii_uppercase()
        }
    }

    fn strip_bracket_identifier(value: &str) -> Option<String> {
        let trimmed = value.trim();
        if trimmed.starts_with('[') && trimmed.ends_with(']') && trimmed.len() >= 2 {
            Some(trimmed[1..trimmed.len().saturating_sub(1)].replace("]]", "]"))
        } else {
            None
        }
    }

    fn normalize_qualifier_lookup_key(qualifier: &str) -> String {
        Self::normalize_qualifier_lookup_segments(qualifier).join(".")
    }

    fn qualifier_lookup_keys(qualifier: &str) -> Vec<String> {
        let normalized = Self::normalize_qualifier_lookup_key(qualifier);
        if normalized.is_empty() {
            return Vec::new();
        }
        vec![normalized]
    }

    fn push_matching_entries(
        entries: &[NameEntry],
        prefix_upper: &str,
        raw_prefix: &str,
        suggestions: &mut Vec<String>,
        seen: &mut HashSet<String>,
    ) -> bool {
        if suggestions.len() >= MAX_SUGGESTIONS || entries.is_empty() {
            return suggestions.len() >= MAX_SUGGESTIONS;
        }
        let start = entries.partition_point(|entry| entry.upper.as_str() < prefix_upper);
        let allow_unquoted_candidate_for_quoted_prefix = identifier_quote_delimiter(raw_prefix)
            .is_some()
            && !raw_prefix.is_empty()
            && !entries
                .iter()
                .skip(start)
                .take_while(|entry| entry.upper.starts_with(prefix_upper))
                .any(|entry| suggestion_matches_completion_prefix(&entry.name, raw_prefix));
        for entry in entries.iter().skip(start) {
            if !entry.upper.starts_with(prefix_upper) {
                break;
            }
            let matches_prefix = raw_prefix.is_empty()
                || suggestion_matches_completion_prefix(&entry.name, raw_prefix)
                || (allow_unquoted_candidate_for_quoted_prefix
                    && identifier_quote_delimiter(&entry.name).is_none());
            if !matches_prefix {
                continue;
            }
            if seen.insert(entry.upper.clone()) {
                suggestions.push(entry.name.clone());
                if suggestions.len() >= MAX_SUGGESTIONS {
                    return true;
                }
            }
        }
        suggestions.len() >= MAX_SUGGESTIONS
    }

    fn append_oracle_builtin_package_suggestions(
        prefix_upper: &str,
        raw_prefix: &str,
        suggestions: &mut Vec<String>,
        seen: &mut HashSet<String>,
    ) {
        for package in ORACLE_BUILTIN_PACKAGES {
            if suggestions.len() >= MAX_SUGGESTIONS {
                break;
            }
            if !package.starts_with(prefix_upper) {
                continue;
            }
            if !raw_prefix.is_empty() && !suggestion_matches_completion_prefix(package, raw_prefix)
            {
                continue;
            }
            if seen.insert((*package).to_string()) {
                suggestions.push((*package).to_string());
            }
        }
    }

    fn is_oracle_builtin_package(name_upper: &str) -> bool {
        ORACLE_BUILTIN_PACKAGES.contains(&name_upper)
    }

    /// Fuzzy (ordered-subsequence) relevance score for a candidate that is *not*
    /// a plain prefix match. Lower is better. Returns `None` when `needle_upper`
    /// is not an *acceptable* ordered subsequence of `haystack_upper`. Callers
    /// exclude empty needles and prefix matches before calling. Contiguous
    /// (substring) and word-boundary aligned matches rank ahead of scattered
    /// ones.
    ///
    /// Acceptance is deliberately tighter than "is a subsequence": a gapped
    /// match is only accepted when its first matched character sits on a word
    /// boundary (start of name or after `_`/space/`.`/`$`/`#`). This keeps
    /// abbreviation/CamelHump matches (`dept`→`DEPARTMENT`, `eid`→`EMPLOYEE_ID`,
    /// `custord`→`CUSTOMER_ORDERS`) while rejecting mid-word scattered noise
    /// (`he`→`WAREHOUSE`), which otherwise floods short prefixes with names that
    /// have no visible relationship to what was typed. A contiguous substring
    /// (`emp`→`TEMPLATE_ID`) is always accepted, anchored or not.
    fn subsequence_match_score(haystack_upper: &str, needle_upper: &str) -> Option<i32> {
        let haystack = haystack_upper.as_bytes();
        let needle = needle_upper.as_bytes();
        let mut hi = 0usize;
        let mut first: Option<usize> = None;
        let mut first_at_boundary = false;
        let mut prev: Option<usize> = None;
        let mut gaps = 0i32;
        let mut boundary_hits = 0i32;
        for &nb in needle {
            let mut matched = false;
            while hi < haystack.len() {
                if haystack[hi] == nb {
                    let at_boundary =
                        hi == 0 || matches!(haystack[hi - 1], b'_' | b' ' | b'.' | b'$' | b'#');
                    if first.is_none() {
                        first = Some(hi);
                        first_at_boundary = at_boundary;
                    }
                    if at_boundary {
                        boundary_hits += 1;
                    }
                    if let Some(p) = prev {
                        if hi != p + 1 {
                            gaps += 1;
                        }
                    }
                    prev = Some(hi);
                    hi += 1;
                    matched = true;
                    break;
                }
                hi += 1;
            }
            if !matched {
                return None;
            }
        }
        // Reject mid-word scattered matches: a gapped match must begin on a word
        // boundary. Contiguous substrings (no gaps) are always allowed.
        if gaps > 0 && !first_at_boundary {
            return None;
        }
        let start = first.unwrap_or(0) as i32;
        Some(
            gaps * 64 + start * 4 - boundary_hits * 8
                + crate::utils::arithmetic::safe_div(haystack.len() as i32, 8),
        )
    }

    /// Append fuzzy subsequence matches across `groups`, ranked by relevance,
    /// after the strict-prefix pass has run. Purely additive: never reorders or
    /// removes existing `suggestions`, and skips anything already in `seen` or
    /// matching the prefix (those are handled earlier). Gated to prefixes of at
    /// least two characters to avoid single-letter noise.
    fn append_fuzzy_entries(
        groups: &[&[NameEntry]],
        prefix_upper: &str,
        suggestions: &mut Vec<String>,
        seen: &mut HashSet<String>,
    ) {
        if prefix_upper.chars().count() < 2 || suggestions.len() >= MAX_SUGGESTIONS {
            return;
        }
        let remaining = MAX_SUGGESTIONS.saturating_sub(suggestions.len());
        let mut scored: Vec<(i32, &NameEntry)> = Vec::with_capacity(remaining);
        let mut fuzzy_seen = HashSet::new();
        for group in groups {
            for entry in group.iter() {
                if entry.upper.starts_with(prefix_upper) || seen.contains(&entry.upper) {
                    continue;
                }
                if let Some(score) = Self::subsequence_match_score(&entry.upper, prefix_upper) {
                    if !fuzzy_seen.insert(entry.upper.clone()) {
                        continue;
                    }
                    if scored.len() < remaining {
                        scored.push((score, entry));
                    } else if let Some((worst_idx, _)) = scored
                        .iter()
                        .enumerate()
                        .max_by(|(_, a), (_, b)| Self::compare_fuzzy_candidates(a, b))
                    {
                        let candidate = (score, entry);
                        if Self::compare_fuzzy_candidates(&candidate, &scored[worst_idx]).is_lt() {
                            scored[worst_idx] = candidate;
                        }
                    }
                }
            }
        }
        scored.sort_by(Self::compare_fuzzy_candidates);
        for (_, entry) in scored {
            if suggestions.len() >= MAX_SUGGESTIONS {
                break;
            }
            if seen.insert(entry.upper.clone()) {
                suggestions.push(entry.name.clone());
            }
        }
    }

    fn compare_fuzzy_candidates(
        a: &(i32, &NameEntry),
        b: &(i32, &NameEntry),
    ) -> std::cmp::Ordering {
        a.0.cmp(&b.0).then_with(|| a.1.upper.cmp(&b.1.upper))
    }
}

impl Default for IntellisenseData {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Clone)]
pub struct IntellisensePopup {
    window: Window,
    browser: HoldBrowser,
    suggestions: Arc<Mutex<Vec<String>>>,
    all_suggestions: Arc<Mutex<Vec<String>>>,
    /// Optional display detail per suggestion (e.g. column type + PK/NN/FK
    /// badges), keyed by the upper-cased suggestion text. The suggestion text
    /// itself remains the inserted value; this only enriches rendering.
    descriptions: Arc<Mutex<HashMap<String, SuggestionDetail>>>,
    selected_callback: Arc<Mutex<Option<Box<dyn FnMut(String)>>>>,
    state: Arc<Mutex<PopupState>>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PopupState {
    Hidden,
    Visible,
}

#[derive(Debug, Default)]
struct PopupPointerGesture {
    pressed_at: Option<(i32, i32)>,
    dragged: bool,
    selection_allowed: bool,
}

impl PopupPointerGesture {
    const DRAG_THRESHOLD: u32 = 4;

    fn handle_event(&mut self, event: fltk::enums::Event, x: i32, y: i32, button: i32) {
        let left_button = fltk::app::MouseButton::Left as i32;
        match event {
            fltk::enums::Event::Push => {
                self.pressed_at = (button == left_button).then_some((x, y));
                self.dragged = false;
                self.selection_allowed = false;
            }
            fltk::enums::Event::Drag => {
                if let Some((start_x, start_y)) = self.pressed_at {
                    self.dragged |= x.abs_diff(start_x) > Self::DRAG_THRESHOLD
                        || y.abs_diff(start_y) > Self::DRAG_THRESHOLD;
                }
            }
            fltk::enums::Event::Released => {
                self.selection_allowed = self.pressed_at.is_some() && !self.dragged;
                self.pressed_at = None;
                self.dragged = false;
            }
            _ => {}
        }
    }

    fn take_selection_allowed(&mut self) -> bool {
        std::mem::take(&mut self.selection_allowed)
    }
}

impl PopupState {
    fn is_visible(self) -> bool {
        matches!(self, Self::Visible)
    }
}

impl IntellisensePopup {
    const POPUP_PAGE_STEP: i32 = 10;

    fn is_deleted(&self) -> bool {
        self.window.was_deleted() || self.browser.was_deleted()
    }

    fn next_page_selection(current: i32, count: i32) -> Option<i32> {
        if count <= 0 {
            return None;
        }

        let normalized = current.max(1);
        Some((normalized + Self::POPUP_PAGE_STEP).min(count))
    }

    fn prev_page_selection(current: i32, count: i32) -> Option<i32> {
        if count <= 0 {
            return None;
        }

        let normalized = current.max(1);
        Some((normalized - Self::POPUP_PAGE_STEP).max(1))
    }

    fn panic_payload_to_string(payload: &(dyn Any + Send)) -> String {
        if let Some(msg) = payload.downcast_ref::<&str>() {
            (*msg).to_string()
        } else if let Some(msg) = payload.downcast_ref::<String>() {
            msg.clone()
        } else {
            "unknown panic payload".to_string()
        }
    }

    fn log_callback_panic(context: &str, payload: &(dyn Any + Send)) {
        let panic_payload = Self::panic_payload_to_string(payload);
        crate::utils::logging::log_error(
            "intellisense_popup::callback",
            &format!("{context} panicked: {panic_payload}"),
        );
        eprintln!("{context} panicked: {panic_payload}");
    }

    fn invoke_selected_callback(
        callback_slot: &Arc<Mutex<Option<Box<dyn FnMut(String)>>>>,
        selected_text: String,
    ) {
        let callback = {
            let mut slot = callback_slot
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            slot.take()
        };

        if let Some(mut cb) = callback {
            let call_result = panic::catch_unwind(AssertUnwindSafe(|| cb(selected_text)));
            let mut slot = callback_slot
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            if slot.is_none() {
                *slot = Some(cb);
            }
            if let Err(payload) = call_result {
                Self::log_callback_panic("intellisense selected callback", payload.as_ref());
            }
        }
    }

    pub fn new() -> Self {
        // Temporarily suspend current group to prevent popup window from being
        // added to the parent container (which causes layout issues)
        let current_group = fltk::group::Group::try_current();

        fltk::group::Group::set_current(None::<&fltk::group::Group>);

        let mut window = Window::default().with_size(320, 200);
        window.set_border(false);
        window.set_color(theme::panel_raised());
        window.make_modal(false);
        // Keep typing focus on the SQL editor even when popup is shown.
        // Override windows are not managed as focus-stealing toplevels.
        window.set_override();

        let mut browser = HoldBrowser::default().with_size(320, 200).with_pos(0, 0);
        browser.set_color(theme::panel_alt());
        browser.set_selection_color(theme::selection_strong());
        // Render items at the configured UI font size so the popup matches the
        // rest of the app and width/height can be measured against it.
        browser.set_text_size(crate::ui::configured_ui_font_size());
        // Two-column layout: suggestion name (fixed width) then a detail
        // column (type / PK / NN / FK badges) that fills the remainder.
        browser.set_column_char('\t');
        browser.set_column_widths(&[180]);
        theme::style_browser_scrollbars(&browser);

        window.end();

        // Restore current group
        if let Some(ref group) = current_group {
            fltk::group::Group::set_current(Some(group));
        }

        let suggestions = Arc::new(Mutex::new(Vec::new()));
        let all_suggestions = Arc::new(Mutex::new(Vec::new()));
        let descriptions = Arc::new(Mutex::new(HashMap::new()));
        let selected_callback: Arc<Mutex<Option<Box<dyn FnMut(String)>>>> =
            Arc::new(Mutex::new(None));
        let state = Arc::new(Mutex::new(PopupState::Hidden));

        window.hide();

        let mut popup = Self {
            window,
            browser,
            suggestions,
            all_suggestions,
            descriptions,
            selected_callback,
            state,
        };

        popup.setup_callbacks();
        popup
    }

    fn setup_callbacks(&mut self) {
        // Browser click callback - handle mouse selection
        let suggestions = self.suggestions.clone();
        let callback = self.selected_callback.clone();
        let mut window = self.window.clone();
        let state = self.state.clone();
        let pointer_gesture = Arc::new(Mutex::new(PopupPointerGesture::default()));
        let pointer_gesture_for_handle = pointer_gesture.clone();

        // Record the gesture before FLTK's built-in browser release handling
        // invokes the selection callback. A drag may move the highlighted row,
        // but releasing it must not insert that row into the editor.
        self.browser.super_handle_first(false);
        self.browser.handle(move |_browser, event| {
            pointer_gesture_for_handle
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .handle_event(
                    event,
                    fltk::app::event_x(),
                    fltk::app::event_y(),
                    fltk::app::event_button(),
                );
            false
        });

        self.browser.set_callback(move |b| {
            if !pointer_gesture
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .take_selection_allowed()
            {
                return;
            }
            let selected = b.value();
            if selected > 0 {
                // First, get the text with suggestions borrow, then release it
                let text = {
                    let suggestions = suggestions
                        .lock()
                        .unwrap_or_else(|poisoned| poisoned.into_inner());
                    suggestions.get((selected - 1) as usize).cloned()
                };
                if let Some(text) = text {
                    // Take the callback out, call it, then put it back if needed.
                    // This ensures the callback slot mutex is not held during callback execution
                    // while preserving callbacks that were replaced during invocation.
                    Self::invoke_selected_callback(&callback, text);
                    window.hide();
                    *state
                        .lock()
                        .unwrap_or_else(|poisoned| poisoned.into_inner()) = PopupState::Hidden;
                }
            }
        });

        // Note: Keyboard events are handled by the editor, not by this popup window.
        // This is because the editor retains focus while the popup is visible,
        // so key events go to the editor's handle(), not the popup's handle().
    }

    pub fn show_suggestions(&mut self, suggestions: Vec<String>, x: i32, y: i32) {
        self.show_suggestions_with_descriptions(suggestions, HashMap::new(), x, y);
    }

    /// Show suggestions with optional per-item display details (column type /
    /// PK / NOT NULL / FK badges). `descriptions` is keyed by normalized
    /// suggestion text; entries without a match render as plain names.
    pub fn show_suggestions_with_descriptions(
        &mut self,
        suggestions: Vec<String>,
        descriptions: HashMap<String, SuggestionDetail>,
        x: i32,
        y: i32,
    ) {
        if self.is_deleted() {
            *self
                .state
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner()) = PopupState::Hidden;
            return;
        }

        if suggestions.is_empty() {
            // Hiding the shown popup hands key-window status back to the main
            // window, which aborts an in-progress composition just like the
            // show transition below. Keep the stale popup until composition
            // ends; the next keyup refresh will hide or repopulate it.
            if self.window.shown() && fltk::app::compose_state() > 0 {
                crate::ui::sql_editor::ime_trace(|| "popup hide deferred (composing)".to_string());
                return;
            }
            self.hide();
            return;
        }

        // Showing a new top-level window makes it macOS's key window, which
        // ends the editor's in-progress IME composition and leaves the first
        // Hangul syllable decomposed into jamo. Defer the first show until
        // composition ends (the next keyup debounce re-triggers it);
        // refreshing an already-visible popup keeps the key window unchanged.
        if !self.window.shown() && fltk::app::compose_state() > 0 {
            crate::ui::sql_editor::ime_trace(|| "popup show deferred (composing)".to_string());
            return;
        }

        *self
            .descriptions
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = descriptions;
        *self
            .all_suggestions
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = suggestions.clone();
        self.set_suggestions(suggestions, None);

        // `set_suggestions` may have widened the window past the caller's
        // estimate, so re-clamp against the screen's work area to keep the
        // right edge visible.
        let width = self.window.width();
        let screen = fltk::app::screen_num(x, y);
        let (sx, _sy, sw, _sh) = fltk::app::screen_work_area(screen);
        let max_x = (sx + sw - width).max(sx);
        let clamped_x = x.clamp(sx, max_x);
        self.window.set_pos(clamped_x, y);
        if !self.window.shown() {
            crate::ui::sql_editor::ime_trace(|| {
                format!(
                    "popup first show: compose_state={}",
                    fltk::app::compose_state()
                )
            });
            self.window.show();
        }
        *self
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = PopupState::Visible;
    }

    fn set_suggestions(&mut self, suggestions: Vec<String>, selected_text: Option<&str>) {
        if self.is_deleted() {
            *self
                .state
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner()) = PopupState::Hidden;
            return;
        }

        let suggestion_count = suggestions.len();
        if suggestion_count == 0 {
            self.hide();
            return;
        }

        // Preserve selection when possible.
        let selected_idx = selected_text
            .and_then(|selected| suggestions.iter().position(|item| item == selected))
            .unwrap_or(0);
        let type_color = theme::text_secondary().bits();
        let badge_color = theme::accent().bits();
        let descriptions = self
            .descriptions
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());

        // Measure rendered text so the popup can be sized to fit the longest
        // entry instead of clipping it at a fixed width. Items render in the
        // browser's font (Helvetica) at its configured text size.
        let item_size = self.browser.text_size();
        fltk::draw::set_font(fltk::enums::Font::Helvetica, item_size);
        let mut max_name_w = 0;
        let mut max_type_w = 0;
        let mut max_badge_w = 0;
        let mut any_type = false;
        let mut any_badge = false;

        self.browser.clear();
        for suggestion in &suggestions {
            let (name_w, _) = fltk::draw::measure(suggestion, false);
            max_name_w = max_name_w.max(name_w);
            let detail = descriptions.get(&NameEntry::lookup_upper(suggestion));
            let type_text = detail.map(|d| d.type_text.as_str()).unwrap_or("");
            let badges = detail.map(|d| d.badges.as_str()).unwrap_or("");
            if !type_text.is_empty() {
                any_type = true;
                let (type_w, _) = fltk::draw::measure(type_text, false);
                max_type_w = max_type_w.max(type_w);
            }
            if !badges.is_empty() {
                any_badge = true;
                let (badge_w, _) = fltk::draw::measure(badges, false);
                max_badge_w = max_badge_w.max(badge_w);
            }
            // Column 1: name; column 2: type; column 3: PK/NN/FK badges. Empty
            // trailing columns are dropped so plain entries render as one cell.
            let row = if !badges.is_empty() {
                format!(
                    "@C255 {}\t@C{} {}\t@C{} {}",
                    suggestion, type_color, type_text, badge_color, badges
                )
            } else if !type_text.is_empty() {
                format!("@C255 {}\t@C{} {}", suggestion, type_color, type_text)
            } else {
                format!("@C255 {}", suggestion)
            };
            self.browser.add(&row);
        }
        drop(descriptions);
        *self
            .suggestions
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = suggestions;

        if suggestion_count > 0 {
            self.browser.select((selected_idx + 1) as i32);
        }

        // Width: fit the longest name, plus a type column and a badge column
        // when present. Each column's tab stop is sized to its widest entry so
        // columns never overlap. Horizontal padding covers the leading-space
        // indent, right margin, and scrollbar.
        const NAME_GAP: i32 = 18;
        const TYPE_GAP: i32 = 18;
        const H_PADDING: i32 = 28;
        let scrollbar = if suggestion_count > Self::POPUP_PAGE_STEP as usize {
            18
        } else {
            0
        };
        let name_col = max_name_w.clamp(60, 520);
        let type_col = max_type_w.min(260);
        let mut content_w = name_col;
        if any_type {
            content_w += NAME_GAP + type_col;
        }
        if any_badge {
            content_w += TYPE_GAP + max_badge_w;
        }
        let width = (content_w + H_PADDING + scrollbar).clamp(160, 900);

        if any_badge {
            self.browser
                .set_column_widths(&[name_col + NAME_GAP, type_col + TYPE_GAP]);
        } else if any_type {
            self.browser.set_column_widths(&[name_col + NAME_GAP]);
        }
        // Row height tracks the font size so larger fonts are not clipped.
        let row_h = (item_size + 6).max(20);
        let height = (suggestion_count.min(10) as i32) * row_h + 10;
        self.window.set_size(width, height);
        self.browser.set_size(width, height);
    }

    pub fn filter_visible_suggestions_by_prefix(&mut self, prefix: &str) {
        if self.is_deleted() {
            *self
                .state
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner()) = PopupState::Hidden;
            return;
        }

        if !self.is_visible() {
            return;
        }

        let selected = self.get_selected();
        let filtered = {
            let all = self
                .all_suggestions
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            filter_suggestions_by_prefix(all.as_slice(), prefix)
        };

        if filtered.is_empty() {
            self.hide();
            self.browser.clear();
            self.suggestions
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .clear();
            return;
        }

        self.set_suggestions(filtered, selected.as_deref());
    }

    pub fn hide(&mut self) {
        if self.is_deleted() {
            *self
                .state
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner()) = PopupState::Hidden;
            return;
        }

        self.window.hide();
        self.window.resize(0, 0, 0, 0);
        *self
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = PopupState::Hidden;
    }

    pub fn clear_for_close(&mut self) {
        if self.is_deleted() {
            *self
                .state
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner()) = PopupState::Hidden;
        } else {
            self.hide();
            self.browser.set_callback(|_| {});
            self.browser.clear();
        }
        self.suggestions
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clear();
        self.all_suggestions
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clear();
        self.descriptions
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clear();
        *self
            .selected_callback
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = None;
    }

    pub fn delete_for_close(&mut self) {
        if self.is_deleted() {
            *self
                .state
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner()) = PopupState::Hidden;
            return;
        }

        self.clear_for_close();
        if !self.window.was_deleted() {
            Window::delete(self.window.clone());
        }
    }

    pub fn is_visible(&self) -> bool {
        if self.is_deleted() {
            return false;
        }

        self.state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .is_visible()
    }

    pub fn popup_dimensions(&self) -> (i32, i32) {
        if self.is_deleted() {
            return (0, 0);
        }

        (self.window.w(), self.window.h())
    }

    pub fn set_position(&mut self, x: i32, y: i32) {
        if self.is_deleted() {
            return;
        }

        self.window.set_pos(x, y);
    }

    pub fn contains_point(&self, x: i32, y: i32) -> bool {
        if self.is_deleted() {
            return false;
        }

        let left = self.window.x();
        let top = self.window.y();
        let right = left + self.window.w();
        let bottom = top + self.window.h();
        x >= left && x < right && y >= top && y < bottom
    }

    pub fn set_selected_callback<F>(&mut self, callback: F)
    where
        F: FnMut(String) + 'static,
    {
        *self
            .selected_callback
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(Box::new(callback));
    }

    pub fn select_next(&mut self) {
        if self.is_deleted() {
            return;
        }

        let current = self.browser.value();
        let count = self.browser.size();
        if current < count {
            self.browser.select(current + 1);
        }
    }

    pub fn select_prev(&mut self) {
        if self.is_deleted() {
            return;
        }

        let current = self.browser.value();
        if current > 1 {
            self.browser.select(current - 1);
        }
    }

    pub fn select_next_page(&mut self) {
        if self.is_deleted() {
            return;
        }

        let count = self.browser.size();
        let current = self.browser.value();
        if let Some(next) = Self::next_page_selection(current, count) {
            self.browser.select(next);
        }
    }

    pub fn select_prev_page(&mut self) {
        if self.is_deleted() {
            return;
        }

        let count = self.browser.size();
        let current = self.browser.value();
        if let Some(prev) = Self::prev_page_selection(current, count) {
            self.browser.select(prev);
        }
    }

    pub fn get_selected(&self) -> Option<String> {
        if self.is_deleted() {
            return None;
        }

        let selected = self.browser.value();
        if selected > 0 {
            self.suggestions
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .get((selected - 1) as usize)
                .cloned()
        } else {
            None
        }
    }
}

pub fn filter_suggestions_by_prefix(suggestions: &[String], prefix: &str) -> Vec<String> {
    if prefix.is_empty() {
        return suggestions.to_vec();
    }

    suggestions
        .iter()
        .filter(|candidate| suggestion_matches_completion_prefix(candidate, prefix))
        .cloned()
        .collect()
}

pub fn suggestion_matches_completion_prefix(candidate: &str, prefix: &str) -> bool {
    identifier_matches_completion_prefix(candidate, prefix)
        || comparison_lhs_identifier_prefix(candidate)
            .is_some_and(|identifier| identifier_matches_completion_prefix(identifier, prefix))
}

fn comparison_lhs_identifier_prefix(candidate: &str) -> Option<&str> {
    let eq_idx = first_unquoted_condition_equals(candidate)?;
    let lhs = candidate.get(..eq_idx)?.trim_end();
    let dot_idx = last_unquoted_identifier_dot(lhs)?;
    lhs.get(dot_idx + 1..).map(str::trim)
}

fn first_unquoted_condition_equals(text: &str) -> Option<usize> {
    let mut active_quote = None;
    let mut chars = text.char_indices().peekable();

    while let Some((idx, ch)) = chars.next() {
        if let Some(delimiter) = active_quote {
            if ch == delimiter {
                if chars.peek().is_some_and(|(_, next)| *next == delimiter) {
                    chars.next();
                } else {
                    active_quote = None;
                }
            }
            continue;
        }

        match ch {
            '"' | '`' => active_quote = Some(ch),
            '[' => active_quote = Some(']'),
            '=' if is_spaced_condition_operator(text, idx) => return Some(idx),
            _ => {}
        }
    }

    None
}

fn is_spaced_condition_operator(text: &str, idx: usize) -> bool {
    text.get(..idx)
        .and_then(|prefix| prefix.chars().next_back())
        .is_some_and(char::is_whitespace)
        && text
            .get(idx + 1..)
            .and_then(|suffix| suffix.chars().next())
            .is_some_and(char::is_whitespace)
}

fn last_unquoted_identifier_dot(text: &str) -> Option<usize> {
    let mut last_dot = None;
    let mut active_quote = None;
    let mut chars = text.char_indices().peekable();

    while let Some((idx, ch)) = chars.next() {
        if let Some(delimiter) = active_quote {
            if ch == delimiter {
                if chars.peek().is_some_and(|(_, next)| *next == delimiter) {
                    chars.next();
                } else {
                    active_quote = None;
                }
            }
            continue;
        }

        match ch {
            '"' | '`' => active_quote = Some(ch),
            '[' => active_quote = Some(']'),
            '.' => last_dot = Some(idx),
            _ => {}
        }
    }

    last_dot
}

fn identifier_matches_completion_prefix(candidate: &str, prefix: &str) -> bool {
    if starts_with_ignore_ascii_case(candidate, prefix) {
        return true;
    }

    let Some(prefix_delimiter) = identifier_quote_delimiter(prefix) else {
        let candidate_lookup = strip_matching_identifier_quotes(candidate);
        return starts_with_ignore_ascii_case(candidate_lookup.as_ref(), prefix);
    };

    if identifier_quote_delimiter(candidate) != Some(prefix_delimiter) {
        return false;
    }

    starts_with_ignore_ascii_case(
        strip_matching_identifier_quotes(candidate).as_ref(),
        strip_incomplete_identifier_quote(prefix),
    )
}

fn identifier_quote_delimiter(value: &str) -> Option<char> {
    match value.chars().next() {
        Some('"') | Some('`') | Some('[') => value.chars().next(),
        _ => None,
    }
}

fn strip_incomplete_identifier_quote(value: &str) -> &str {
    match identifier_quote_delimiter(value) {
        Some(delimiter) => value.get(delimiter.len_utf8()..).unwrap_or(""),
        None => value,
    }
}

fn strip_matching_identifier_quotes(value: &str) -> Cow<'_, str> {
    if value.len() >= 2 {
        let bytes = value.as_bytes();
        let first = bytes[0];
        let last = bytes[value.len() - 1];
        if (first == b'"' && last == b'"') || (first == b'`' && last == b'`') {
            return Cow::Borrowed(&value[1..value.len() - 1]);
        }
        if first == b'[' && last == b']' {
            return Cow::Owned(value[1..value.len() - 1].replace("]]", "]"));
        }
    }
    Cow::Borrowed(value)
}

fn starts_with_ignore_ascii_case(value: &str, prefix: &str) -> bool {
    let value_bytes = value.as_bytes();
    let prefix_bytes = prefix.as_bytes();
    value_bytes.len() >= prefix_bytes.len()
        && value_bytes[..prefix_bytes.len()].eq_ignore_ascii_case(prefix_bytes)
}

impl Default for IntellisensePopup {
    fn default() -> Self {
        Self::new()
    }
}

// Helper function to extract the current word at cursor position (Unicode-aware).
// cursor_pos is a byte offset from FLTK TextBuffer.
fn normalize_cursor_pos(text: &str, cursor_pos: usize) -> usize {
    if text.is_empty() {
        return 0;
    }

    let idx = cursor_pos.min(text.len());
    if text.is_char_boundary(idx) {
        return idx;
    }

    // Clamp invalid UTF-8 byte offsets to the previous valid boundary.
    let mut clamped = idx;
    while clamped > 0 && !text.is_char_boundary(clamped) {
        clamped -= 1;
    }
    clamped
}

pub fn get_word_at_cursor(text: &str, cursor_pos: usize) -> (String, usize, usize) {
    let mysql_compatible = sql_text::mysql_compatibility_for_sql(text, None);
    get_word_at_cursor_with_lexical_mode(
        text,
        cursor_pos,
        mysql_compatible,
        crate::sql_parser_engine::LexMode::Idle,
    )
}

pub(crate) fn get_word_at_cursor_with_lexical_mode(
    text: &str,
    cursor_pos: usize,
    mysql_compatible: bool,
    initial_mode: crate::sql_parser_engine::LexMode,
) -> (String, usize, usize) {
    if text.is_empty() || cursor_pos == 0 {
        return (String::new(), 0, 0);
    }

    let raw_pos = cursor_pos.min(text.len());
    let pos = normalize_cursor_pos(text, raw_pos);
    let cursor_was_non_boundary = raw_pos < text.len() && raw_pos != pos;
    let effective_pos = if cursor_was_non_boundary {
        // If FLTK gives an invalid byte offset in the middle of a UTF-8 character,
        // advance to the end of the current identifier so prefix extraction remains stable.
        let mut p = pos;
        while p < text.len() {
            let Some(ch) = text[p..].chars().next() else {
                break;
            };
            if sql_text::is_identifier_char(ch) {
                p += ch.len_utf8();
            } else {
                break;
            }
        }
        p
    } else {
        pos
    };

    let lexical_spans = crate::sql_parser_engine::lexical_spans_with_initial_mode(
        text,
        mysql_compatible,
        initial_mode,
    )
    .0;
    if let Some((start, delimiter)) =
        incomplete_quoted_identifier_start_before_cursor(text, effective_pos, &lexical_spans)
    {
        let end = quoted_identifier_end_from_cursor(text, effective_pos, delimiter)
            .unwrap_or(effective_pos);
        let word = text.get(start..effective_pos).unwrap_or("").to_string();
        return (word, start, end);
    }

    get_unquoted_word_at_cursor(text, effective_pos)
}

pub(crate) fn get_unquoted_word_at_cursor(
    text: &str,
    effective_pos: usize,
) -> (String, usize, usize) {
    // Find word start by scanning backwards over identifier characters.
    let mut start = effective_pos;
    while start > 0 {
        let Some((prev_start, ch)) = text[..start].char_indices().next_back() else {
            break;
        };
        if sql_text::is_identifier_char(ch) {
            start = prev_start;
        } else {
            break;
        }
    }

    // Find word end by scanning forwards over identifier characters.
    let mut end = effective_pos;
    while end < text.len() {
        let Some(ch) = text[end..].chars().next() else {
            break;
        };
        if sql_text::is_identifier_char(ch) {
            end += ch.len_utf8();
        } else {
            break;
        }
    }

    let word = text.get(start..effective_pos).unwrap_or("").to_string();
    (word, start, end)
}

fn incomplete_quoted_identifier_start_before_cursor(
    text: &str,
    cursor_pos: usize,
    lexical_spans: &[crate::sql_parser_engine::LexicalSpan],
) -> Option<(usize, char)> {
    let mut idx = cursor_pos;
    while idx > 0 {
        let (prev_idx, ch) = text.get(..idx)?.char_indices().next_back()?;
        if matches!(ch, '\n' | '\r') {
            break;
        }
        if matches!(ch, '"' | '`' | '[') && quoted_identifier_start_context(text, prev_idx) {
            let lexical_idx = lexical_spans.partition_point(|span| span.end <= prev_idx);
            let lexical_span = lexical_spans
                .get(lexical_idx)
                .filter(|span| span.contains(prev_idx));
            let is_parser_quoted_identifier_start = lexical_span.is_some_and(|span| {
                span.kind == crate::sql_parser_engine::LexicalKind::QuotedIdentifier
                    && span.start == prev_idx
            });
            let bracket_starts_in_code = ch == '[' && lexical_span.is_none();
            if (is_parser_quoted_identifier_start || bracket_starts_in_code)
                && !has_unescaped_identifier_delimiter(
                    text,
                    prev_idx + ch.len_utf8(),
                    cursor_pos,
                    identifier_closing_delimiter(ch),
                )
            {
                return Some((prev_idx, ch));
            }
        }
        idx = prev_idx;
    }

    None
}

fn quoted_identifier_start_context(text: &str, quote_idx: usize) -> bool {
    text.get(..quote_idx)
        .and_then(|prefix| prefix.chars().next_back())
        .is_none_or(|ch| !sql_text::is_identifier_char(ch) && !matches!(ch, '"' | '`' | '[' | '\''))
}

fn identifier_closing_delimiter(opening: char) -> char {
    match opening {
        '[' => ']',
        _ => opening,
    }
}

fn has_unescaped_identifier_delimiter(
    text: &str,
    start: usize,
    end: usize,
    delimiter: char,
) -> bool {
    let Some(segment) = text.get(start..end) else {
        return false;
    };
    let mut chars = segment.chars().peekable();
    while let Some(ch) = chars.next() {
        if ch == delimiter {
            if chars.peek().is_some_and(|next| *next == delimiter) {
                chars.next();
            } else {
                return true;
            }
        }
    }

    false
}

fn quoted_identifier_end_from_cursor(
    text: &str,
    cursor_pos: usize,
    delimiter: char,
) -> Option<usize> {
    let delimiter = identifier_closing_delimiter(delimiter);
    let mut iter = text.get(cursor_pos..)?.char_indices().peekable();
    while let Some((rel_idx, ch)) = iter.next() {
        if ch == delimiter {
            if iter.peek().is_some_and(|(_, next)| *next == delimiter) {
                iter.next();
            } else {
                return Some(cursor_pos + rel_idx + ch.len_utf8());
            }
        }
    }

    None
}

/// A formatted routine signature for the parameter-hint popup, with the byte
/// range of each positional argument inside `text` so the active one can be
/// emphasized.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct SignatureLabel {
    pub text: String,
    pub arg_spans: Vec<(usize, usize)>,
}

/// Whether `name` is a known built-in SQL function (Oracle/MySQL/MariaDB). Used to
/// skip a futile routine-argument DB lookup for built-ins, which never appear
/// in the data-dictionary argument views.
pub fn is_builtin_function(name: &str) -> bool {
    ORACLE_FUNCTIONS
        .iter()
        .chain(MYSQL_FUNCTIONS.iter())
        .chain(MARIADB_FUNCTIONS.iter())
        .any(|candidate| candidate.eq_ignore_ascii_case(name))
}

/// The function/procedure call that encloses the cursor, for signature hints.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EnclosingCall {
    /// Routine name (last dotted segment), quotes stripped.
    pub name: String,
    /// Anything before the routine name (package or schema[.package]), if any.
    pub qualifier: Option<String>,
    /// Zero-based index of the argument the cursor is currently in.
    pub arg_index: usize,
    /// Byte offset of the call's opening parenthesis.
    pub open_paren: usize,
}

/// Lookup key for a routine call: `QUALIFIER.NAME` uppercased.
pub fn signature_key_for_call(call: &EnclosingCall) -> String {
    match &call.qualifier {
        Some(qualifier) => format!("{}.{}", qualifier.to_uppercase(), call.name.to_uppercase()),
        None => call.name.to_uppercase(),
    }
}

/// Find the innermost function/procedure call whose argument list contains the
/// cursor, counting top-level commas to determine the active argument. Skips
/// string literals, quoted identifiers, and comments. Returns `None` when the
/// cursor is not inside a `name(...)` call (e.g. a bare grouping parenthesis).
pub fn enclosing_call_at_cursor(text: &str, cursor_pos: usize) -> Option<EnclosingCall> {
    let end = normalize_cursor_pos(text, cursor_pos.min(text.len()));
    let mysql_compatible = sql_text::mysql_compatibility_for_sql(&text[..end], None);
    enclosing_call_at_cursor_with_lexical_mode(
        text,
        cursor_pos,
        mysql_compatible,
        crate::sql_parser_engine::LexMode::Idle,
    )
}

pub(crate) fn enclosing_call_at_cursor_with_lexical_mode(
    text: &str,
    cursor_pos: usize,
    mysql_compatible: bool,
    initial_mode: crate::sql_parser_engine::LexMode,
) -> Option<EnclosingCall> {
    let end = normalize_cursor_pos(text, cursor_pos.min(text.len()));
    let slice = &text[..end];

    struct Frame {
        open: usize,
        commas: usize,
    }
    let mut stack: Vec<Frame> = Vec::new();
    let lexical_spans = crate::sql_parser_engine::lexical_spans_with_initial_mode(
        slice,
        mysql_compatible,
        initial_mode,
    )
    .0;
    let bytes = slice.as_bytes();
    let mut idx = 0usize;
    let mut lexical_idx = 0usize;

    while idx < bytes.len() {
        while lexical_spans
            .get(lexical_idx)
            .is_some_and(|span| span.end <= idx)
        {
            lexical_idx += 1;
        }
        if let Some(span) = lexical_spans.get(lexical_idx) {
            if span.start <= idx && idx < span.end {
                idx = span.end;
                continue;
            }
        }

        match bytes[idx] {
            b'(' => stack.push(Frame {
                open: idx,
                commas: 0,
            }),
            b')' => {
                stack.pop();
            }
            b',' => {
                if let Some(frame) = stack.last_mut() {
                    frame.commas += 1;
                }
            }
            _ => {}
        }
        idx += 1;
    }

    let frame = stack.pop()?;
    let (name, qualifier) = dotted_reference_before(text, frame.open)?;
    if is_non_call_keyword(&name) {
        return None;
    }
    Some(EnclosingCall {
        name,
        qualifier,
        arg_index: frame.commas,
        open_paren: frame.open,
    })
}

/// Clause/operator keywords that introduce a grouping or subquery parenthesis
/// rather than a routine call. The general keyword list is unusable here
/// because it also contains built-in functions (DECODE, NVL, TO_CHAR, ...).
fn is_non_call_keyword(name: &str) -> bool {
    const NON_CALL_KEYWORDS: &[&str] = &[
        "SELECT",
        "WHERE",
        "HAVING",
        "AND",
        "OR",
        "NOT",
        "IN",
        "EXISTS",
        "FROM",
        "ON",
        "VALUES",
        "SET",
        "WHEN",
        "THEN",
        "ELSE",
        "INTO",
        "UNION",
        "INTERSECT",
        "MINUS",
        "EXCEPT",
        "START",
        "CONNECT",
        "USING",
        "OVER",
        "AS",
        "ALL",
        "ANY",
        "SOME",
        "BETWEEN",
        "LIKE",
        "CASE",
        "GROUP",
        "ORDER",
        "BY",
        "PARTITION",
        "WITHIN",
        "RETURNING",
        "BEGIN",
        "IF",
        "ELSIF",
        "LOOP",
        "WHILE",
        "RETURN",
        "CHECK",
        "FOR",
        "WITH",
    ];
    let upper = name.to_uppercase();
    NON_CALL_KEYWORDS.contains(&upper.as_str())
}

/// Read the dotted object reference immediately preceding `open_idx` (skipping
/// whitespace). Returns the final segment as `name` and any leading segments
/// joined as `qualifier`. Quoted identifiers (`"a.b"`, `"weird name"`) are
/// handled: dots and spaces inside quotes are part of the segment.
fn dotted_reference_before(text: &str, open_idx: usize) -> Option<(String, Option<String>)> {
    let mut pos = open_idx;
    while pos > 0 {
        let (prev, ch) = text[..pos].char_indices().next_back()?;
        if ch.is_whitespace() {
            pos = prev;
        } else {
            break;
        }
    }

    let token_end = pos;
    while pos > 0 {
        let (prev, ch) = text[..pos].char_indices().next_back()?;
        if ch == '"' {
            // Closing quote of a quoted identifier: consume backward through
            // the whole quoted segment (any chars) up to its opening quote.
            pos = prev;
            while pos > 0 {
                let (inner_prev, inner_ch) = text[..pos].char_indices().next_back()?;
                pos = inner_prev;
                if inner_ch == '"' {
                    break;
                }
            }
        } else if sql_text::is_identifier_char(ch) || ch == '.' {
            pos = prev;
        } else {
            break;
        }
    }

    let token = text.get(pos..token_end)?;
    if token.is_empty() {
        return None;
    }

    let mut segments = split_dotted_identifier(token);
    let name = segments.pop()?;
    if name.is_empty() {
        return None;
    }
    let qualifier = (!segments.is_empty()).then(|| segments.join("."));
    Some((name, qualifier))
}

/// Split a dotted object reference into its segments, treating dots inside
/// double-quoted identifiers as literal and unescaping `""` to `"`.
fn split_dotted_identifier(token: &str) -> Vec<String> {
    let mut segments = Vec::new();
    let mut current = String::new();
    let mut in_quote = false;
    let mut chars = token.chars().peekable();

    while let Some(ch) = chars.next() {
        match ch {
            '"' if in_quote => {
                if chars.peek() == Some(&'"') {
                    current.push('"');
                    chars.next();
                } else {
                    in_quote = false;
                }
            }
            '"' => in_quote = true,
            '.' if !in_quote => segments.push(std::mem::take(&mut current)),
            _ => current.push(ch),
        }
    }
    segments.push(current);
    segments
}

// Detect context for smarter suggestions (after FROM, after SELECT, etc.)
// Uses the deep context analyzer for accurate depth-aware detection.
pub fn detect_sql_context(text: &str, cursor_pos: usize) -> SqlContext {
    use crate::ui::intellisense_context;
    use crate::ui::sql_editor::query_text::tokenize_sql_spanned;

    let end = normalize_cursor_pos(text, cursor_pos);
    let token_spans = tokenize_sql_spanned(text);
    let split_idx = token_spans.partition_point(|span| span.end <= end);
    let full_tokens = token_spans
        .into_iter()
        .map(|span| span.token)
        .collect::<Vec<_>>();
    let ctx = intellisense_context::analyze_cursor_context_owned(full_tokens, split_idx);

    sql_context_for_phase(ctx.phase)
}

#[derive(Debug, Clone, Copy, PartialEq)]
#[allow(dead_code)]
pub enum SqlContext {
    General,
    TableName,
    ColumnName,
    ColumnOrAll,
    VariableName,
    BindValue,
    GeneratedName,
}

pub(crate) fn sql_context_for_phase(
    phase: crate::ui::intellisense_context::SqlPhase,
) -> SqlContext {
    use crate::ui::intellisense_context::SqlPhase;

    match phase {
        SqlPhase::FromClause
        | SqlPhase::IntoClause
        | SqlPhase::UpdateTarget
        | SqlPhase::DeleteTarget
        | SqlPhase::MergeTarget => SqlContext::TableName,
        SqlPhase::SelectIntoTarget
        | SqlPhase::FetchIntoTarget
        | SqlPhase::ExecuteIntoTarget
        | SqlPhase::ReturningIntoTarget => SqlContext::VariableName,
        SqlPhase::UsingBindList => SqlContext::BindValue,
        SqlPhase::SelectList => SqlContext::ColumnOrAll,
        SqlPhase::CteColumnList
        | SqlPhase::DerivedAliasColumnList
        | SqlPhase::ConflictTargetList
        | SqlPhase::JoinUsingColumnList
        | SqlPhase::RecursiveCteColumnList
        | SqlPhase::DmlSetTargetList
        | SqlPhase::InsertColumnList
        | SqlPhase::MergeInsertColumnList
        | SqlPhase::DmlReturningList
        | SqlPhase::LockingColumnList
        | SqlPhase::WhereClause
        | SqlPhase::JoinCondition
        | SqlPhase::GroupByClause
        | SqlPhase::HavingClause
        | SqlPhase::OrderByClause
        | SqlPhase::SetClause
        | SqlPhase::ValuesClause
        | SqlPhase::ConnectByClause
        | SqlPhase::StartWithClause
        | SqlPhase::PivotClause
        | SqlPhase::MatchRecognizeClause
        | SqlPhase::ModelClause
        | SqlPhase::DdlColumnList => SqlContext::ColumnName,
        SqlPhase::RecursiveCteGeneratedColumnName | SqlPhase::HierarchicalGeneratedColumnName => {
            SqlContext::GeneratedName
        }
        _ => SqlContext::General,
    }
}

/// In-window overlay showing the signature of the routine call enclosing the
/// cursor, with the active argument bracketed. Keeping this inside the editor's
/// parent window preserves editor focus while the hint is visible.
pub struct SignaturePopup {
    frame: Option<Frame>,
    visible: bool,
    last_render: Option<SignaturePopupRenderState>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct SignaturePopupRenderState {
    display: String,
    x: i32,
    y: i32,
    width: i32,
    height: i32,
}

impl SignaturePopup {
    /// Label font size, and popup height derived from it, both tracking the
    /// configured UI font size.
    pub(crate) fn font_size() -> i32 {
        crate::ui::configured_ui_font_size()
    }

    pub(crate) fn height() -> i32 {
        (Self::font_size() + 8).max(22)
    }

    pub fn new() -> Self {
        Self {
            frame: None,
            visible: false,
            last_render: None,
        }
    }

    fn is_deleted(&self) -> bool {
        self.frame.as_ref().is_some_and(|frame| frame.was_deleted())
    }

    fn ensure_frame(&mut self, editor: &TextEditor) -> Option<&mut Frame> {
        if self.is_deleted() {
            self.frame = None;
            self.visible = false;
        }
        if self.frame.is_none() {
            let parent = editor.window()?;
            let current_group = fltk::group::Group::try_current();
            parent.begin();
            let mut frame = Frame::default()
                .with_size(200, Self::height())
                .with_pos(0, 0);
            frame.set_frame(fltk::enums::FrameType::FlatBox);
            frame.set_color(theme::panel_raised());
            frame.set_label_color(theme::text_primary());
            frame.set_label_size(Self::font_size());
            frame.set_align(fltk::enums::Align::Left | fltk::enums::Align::Inside);
            frame.clear_visible_focus();
            frame.hide();
            parent.end();
            if let Some(ref group) = current_group {
                fltk::group::Group::set_current(Some(group));
            } else {
                fltk::group::Group::set_current(None::<&fltk::group::Group>);
            }
            self.frame = Some(frame);
        }
        self.frame.as_mut()
    }

    fn display_text(label: &SignatureLabel, active_arg: usize) -> String {
        let Some(&(start, end)) = label.arg_spans.get(active_arg) else {
            return label.text.clone();
        };
        let Some(before) = label.text.get(..start) else {
            return label.text.clone();
        };
        let Some(active) = label.text.get(start..end) else {
            return label.text.clone();
        };
        let Some(after) = label.text.get(end..) else {
            return label.text.clone();
        };
        format!("{before}[ {active} ]{after}")
    }

    /// Position inside the editor's parent window. Mirrors the completion
    /// popup's cursor-position and window-bound clamping, but deliberately keeps
    /// coordinates local because the signature hint is an in-window overlay.
    fn overlay_position(
        editor: &TextEditor,
        anchor_pos: i32,
        width: i32,
        height: i32,
    ) -> (i32, i32) {
        let (cursor_x, cursor_y) = editor.position_to_xy(anchor_pos);
        let mut popup_x = cursor_x;
        let mut popup_y = cursor_y - height - 2;

        if let Some(win) = editor.window() {
            let max_x = (win.w() - width).max(0);
            let max_y = (win.h() - height).max(0);
            popup_x = popup_x.clamp(0, max_x);
            popup_y = popup_y.clamp(0, max_y);
        }

        (popup_x, popup_y)
    }

    /// Render `label` with the argument at `active_arg` bracketed, sizing and
    /// positioning the overlay above the call's opening parenthesis.
    pub fn show(
        &mut self,
        editor: &TextEditor,
        label: &SignatureLabel,
        active_arg: usize,
        anchor_pos: i32,
    ) {
        if label.text.is_empty() {
            self.hide();
            return;
        }

        let display = Self::display_text(label, active_arg);
        fltk::draw::set_font(fltk::enums::Font::Helvetica, Self::font_size());
        let (text_w, _) = fltk::draw::measure(&display, false);
        let width = (text_w + 20).clamp(120, 1100);
        let height = Self::height();
        let (x, y) = Self::overlay_position(editor, anchor_pos, width, height);
        let render_state = SignaturePopupRenderState {
            display,
            x,
            y,
            width,
            height,
        };

        if self.is_visible()
            && self
                .last_render
                .as_ref()
                .is_some_and(|last_render| last_render == &render_state)
        {
            return;
        }

        let Some(frame) = self.ensure_frame(editor) else {
            self.visible = false;
            self.last_render = None;
            return;
        };
        frame.set_label(&render_state.display);
        frame.set_size(width, height);
        frame.set_pos(x, y);
        frame.show();
        frame.redraw();
        if let Some(mut win) = editor.window() {
            win.redraw();
        }
        self.visible = true;
        self.last_render = Some(render_state);
    }

    pub fn hide(&mut self) {
        self.visible = false;
        self.last_render = None;
        if let Some(frame) = self.frame.as_mut() {
            if frame.was_deleted() {
                return;
            }
            frame.hide();
            frame.redraw();
            if let Some(mut win) = frame.window() {
                win.redraw();
            }
        }
    }

    pub fn is_visible(&self) -> bool {
        self.visible && !self.is_deleted()
    }

    pub fn delete_for_close(&mut self) {
        self.hide();
        if let Some(frame) = self.frame.take() {
            if !frame.was_deleted() {
                Frame::delete(frame);
            }
        }
    }
}

impl Default for SignaturePopup {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod intellisense_tests {
    use super::*;

    fn call_at(text: &str) -> Option<EnclosingCall> {
        let cursor = text.find('|').expect("cursor marker");
        let text = text.replace('|', "");
        enclosing_call_at_cursor(&text, cursor)
    }

    #[test]
    fn signature_popup_display_text_rejects_invalid_spans_without_panicking() {
        let label = SignatureLabel {
            text: "PROC(한글)".to_string(),
            arg_spans: vec![(8, 6), (6, 7)],
        };

        assert_eq!(SignaturePopup::display_text(&label, 0), label.text);
        assert_eq!(SignaturePopup::display_text(&label, 1), label.text);

        let valid = SignatureLabel {
            text: "PROC(한글)".to_string(),
            arg_spans: vec![(5, 11)],
        };
        assert_eq!(SignaturePopup::display_text(&valid, 0), "PROC([ 한글 ])");
    }

    #[test]
    fn enclosing_call_detects_name_and_arg_index() {
        let call = call_at("SELECT SUBSTR(name, |) FROM t").expect("inside call");
        assert_eq!(call.name, "SUBSTR");
        assert_eq!(call.qualifier, None);
        assert_eq!(call.arg_index, 1);
    }

    #[test]
    fn enclosing_call_first_arg_has_index_zero() {
        let call = call_at("SELECT TO_CHAR(|) FROM t").expect("inside call");
        assert_eq!(call.name, "TO_CHAR");
        assert_eq!(call.arg_index, 0);
    }

    #[test]
    fn enclosing_call_resolves_package_qualifier() {
        let call = call_at("BEGIN pkg.proc(a, b, |").expect("inside call");
        assert_eq!(call.name, "proc");
        assert_eq!(call.qualifier.as_deref(), Some("pkg"));
        assert_eq!(call.arg_index, 2);
    }

    #[test]
    fn enclosing_call_uses_innermost_call() {
        let call = call_at("SELECT NVL(SUBSTR(x, |), y) FROM t").expect("inside call");
        assert_eq!(call.name, "SUBSTR");
        assert_eq!(call.arg_index, 1);
    }

    #[test]
    fn enclosing_call_ignores_commas_in_strings() {
        let call = call_at("SELECT DECODE(x, 'a,b,c', |) FROM t").expect("inside call");
        assert_eq!(call.name, "DECODE");
        assert_eq!(call.arg_index, 2);
    }

    #[test]
    fn enclosing_call_ignores_oracle_q_quoted_delimiters_before_call() {
        let call = call_at("BEGIN v := q'[payload with (,)]'; qt_splitter_pkg.upsert_row(p_i|")
            .expect("inside call after q-quoted literal");
        assert_eq!(call.name, "upsert_row");
        assert_eq!(call.qualifier.as_deref(), Some("qt_splitter_pkg"));
        assert_eq!(call.arg_index, 0);
    }

    #[test]
    fn enclosing_call_uses_parser_engine_for_nested_prefixed_q_quotes() {
        let call =
            call_at("BEGIN v := nq'[outer q'[nested]' fake ), ( tail]'; qt_pkg.write_row(a, |")
                .expect("inside call after nested prefixed q-quote");
        assert_eq!(call.name, "write_row");
        assert_eq!(call.qualifier.as_deref(), Some("qt_pkg"));
        assert_eq!(call.arg_index, 1);
    }

    #[test]
    fn enclosing_call_uses_parser_engine_for_escaped_quoted_identifiers() {
        let call =
            call_at(r#"SELECT "payload""), (" AS value_text, qt_pkg.write_row(a, | FROM dual"#)
                .expect("inside call after escaped quoted identifier");
        assert_eq!(call.name, "write_row");
        assert_eq!(call.qualifier.as_deref(), Some("qt_pkg"));
        assert_eq!(call.arg_index, 1);
    }

    #[test]
    fn enclosing_call_accepts_bounded_window_entry_lex_mode() {
        let text = "q'[nested]' fake ), ( tail]'; qt_pkg.write_row(a, ";
        let call = enclosing_call_at_cursor_with_lexical_mode(
            text,
            text.len(),
            false,
            crate::sql_parser_engine::LexMode::QQuote {
                end_char: ']',
                depth: 1,
            },
        )
        .expect("inside call after q-quote continuation");

        assert_eq!(call.name, "write_row");
        assert_eq!(call.qualifier.as_deref(), Some("qt_pkg"));
        assert_eq!(call.arg_index, 1);
    }

    #[test]
    fn enclosing_call_none_when_call_closed() {
        assert!(call_at("SELECT TO_CHAR(x) |FROM t").is_none());
    }

    #[test]
    fn enclosing_call_none_for_bare_grouping_paren() {
        assert!(call_at("SELECT (a + |) FROM t").is_none());
    }

    #[test]
    fn suggestion_type_label_classifies_objects_keywords_and_functions() {
        let mut data = IntellisenseData::new();
        data.tables = vec!["EMP".to_string()];
        data.views = vec!["EMP_VIEW".to_string()];
        data.materialized_views = vec!["MVIEW_SALES".to_string()];
        data.synonyms = vec!["EMP_SYN".to_string()];
        data.public_synonyms = vec!["PUBLIC_EMP".to_string()];
        data.procedures = vec!["DO_WORK".to_string()];
        data.packages = vec!["HR_PKG".to_string()];
        data.functions = vec!["CALC_TOTAL".to_string()];
        data.sequences = vec!["EMP_SEQ".to_string()];
        data.types = vec!["ADDRESS_T".to_string()];
        data.triggers = vec!["BI_EMP".to_string()];
        data.events = vec!["CLEANUP_EVENT".to_string()];
        data.indexes = vec!["IDX_EMP_NAME".to_string()];
        data.database_links = vec!["APP_LINK".to_string()];
        data.directories = vec!["DATA_PUMP_DIR".to_string()];
        data.libraries = vec!["APP_LIB".to_string()];
        data.clusters = vec!["EMP_CLUSTER".to_string()];
        data.contexts = vec!["APP_CTX".to_string()];
        data.dimensions = vec!["SALES_DIM".to_string()];
        data.operators = vec!["TEXT_OP".to_string()];
        data.indextypes = vec!["TEXT_ITYPE".to_string()];
        data.editions = vec!["ORA_EDITION".to_string()];
        data.java_sources = vec!["Welcome".to_string()];
        data.java_classes = vec!["WelcomeClass".to_string()];
        data.java_resources = vec!["WelcomeRes".to_string()];
        data.users = vec!["APP_USER".to_string()];
        data.rebuild_indices();

        let oracle = Some(crate::db::DatabaseType::Oracle);
        for (name, label) in [
            ("EMP", "TABLE"),
            ("emp_view", "VIEW"),
            ("MVIEW_SALES", "MVIEW"),
            ("EMP_SYN", "SYNONYM"),
            ("PUBLIC_EMP", "PUBLIC SYNONYM"),
            ("DO_WORK", "PROCEDURE"),
            ("HR_PKG", "PACKAGE"),
            ("CALC_TOTAL", "FUNCTION"),
            ("EMP_SEQ", "SEQUENCE"),
            ("ADDRESS_T", "TYPE"),
            ("BI_EMP", "TRIGGER"),
            ("CLEANUP_EVENT", "EVENT"),
            ("IDX_EMP_NAME", "INDEX"),
            ("APP_LINK", "DATABASE LINK"),
            ("DATA_PUMP_DIR", "DIRECTORY"),
            ("APP_LIB", "LIBRARY"),
            ("EMP_CLUSTER", "CLUSTER"),
            ("APP_CTX", "CONTEXT"),
            ("SALES_DIM", "DIMENSION"),
            ("TEXT_OP", "OPERATOR"),
            ("TEXT_ITYPE", "INDEXTYPE"),
            ("ORA_EDITION", "EDITION"),
            ("Welcome", "JAVA SOURCE"),
            ("WelcomeClass", "JAVA CLASS"),
            ("WelcomeRes", "JAVA RESOURCE"),
            ("APP_USER", "USER"),
        ] {
            assert_eq!(
                data.suggestion_type_label(name, oracle),
                Some(label),
                "{name} should be labelled {label}"
            );
        }
        // Static catalog keyword and a built-in function rendered with "()".
        assert_eq!(
            data.suggestion_type_label("SELECT", oracle),
            Some("KEYWORD")
        );
        assert_eq!(
            data.suggestion_type_label("COUNT()", oracle),
            Some("FUNCTION")
        );
        // Unknown identifier stays unclassified (columns are handled elsewhere).
        assert_eq!(data.suggestion_type_label("NO_SUCH_NAME", oracle), None);
    }

    #[test]
    fn suggestions_do_not_duplicate_function_and_keyword() {
        // MAX is present in both ORACLE_SQL_KEYWORDS and ORACLE_FUNCTIONS; it
        // must surface only once. When a name is both, the bare keyword form
        // wins and the `MAX()` function form is suppressed.
        let mut data = IntellisenseData::new();
        let oracle = Some(crate::db::DatabaseType::Oracle);
        let suggestions = data.get_suggestions_for_db("max", false, None, false, false, oracle);
        assert!(
            suggestions.iter().any(|s| s == "MAX"),
            "expected MAX in {suggestions:?}"
        );
        assert!(
            !suggestions.iter().any(|s| s == "MAX()"),
            "MAX() must not appear when MAX is also a keyword in {suggestions:?}"
        );
    }

    #[test]
    fn oracle_lengthb_is_suggested_as_a_builtin_function() {
        let suggestions = language_catalog_suggestions_for_db(
            "lengthb",
            true,
            Some(crate::db::DatabaseType::Oracle),
        );

        assert!(
            suggestions
                .iter()
                .any(|suggestion| suggestion == "LENGTHB()"),
            "expected LENGTHB() in {suggestions:?}"
        );
    }

    #[test]
    fn enclosing_call_handles_quoted_identifier_with_dot() {
        let call = call_at(r#"SELECT "my.func"(a, |) FROM t"#).expect("inside call");
        assert_eq!(call.name, "my.func");
        assert_eq!(call.qualifier, None);
        assert_eq!(call.arg_index, 1);
    }

    #[test]
    fn enclosing_call_handles_quoted_member_with_dot() {
        let call = call_at(r#"BEGIN pkg."weird.name"(|"#).expect("inside call");
        assert_eq!(call.name, "weird.name");
        assert_eq!(call.qualifier.as_deref(), Some("pkg"));
    }

    #[test]
    fn enclosing_call_handles_quoted_name_with_space() {
        let call = call_at(r#"SELECT "weird name"(|) FROM t"#).expect("inside call");
        assert_eq!(call.name, "weird name");
        assert_eq!(call.qualifier, None);
    }

    #[test]
    fn is_builtin_function_detects_known_builtins() {
        assert!(is_builtin_function("nvl"));
        assert!(is_builtin_function("TO_CHAR"));
        assert!(!is_builtin_function("MY_CUSTOM_PROC"));
    }

    #[test]
    fn foreign_keys_loading_dedups_and_clears() {
        let mut data = IntellisenseData::new();
        // First mark starts a load; a second is suppressed while in flight.
        assert!(data.mark_foreign_keys_loading("EMP"));
        assert!(!data.mark_foreign_keys_loading("EMP"));

        // Storing the result clears the loading flag, and a loaded table is
        // not re-marked.
        data.set_foreign_keys_for_table("EMP", Vec::new());
        assert!(!data.mark_foreign_keys_loading("EMP"));

        // A transient failure clears loading so it can be retried.
        assert!(data.mark_foreign_keys_loading("DEPT"));
        data.clear_foreign_keys_loading("DEPT");
        assert!(data.mark_foreign_keys_loading("DEPT"));
    }

    #[test]
    fn get_suggestions_prefers_relations_in_table_context_with_empty_prefix() {
        let mut data = IntellisenseData::new();
        data.tables = (0..80).map(|i| format!("TBL_{:02}", i)).collect();
        data.rebuild_indices();

        let suggestions = data.get_suggestions("", false, None, true, false);

        assert_eq!(suggestions.len(), MAX_SUGGESTIONS);
        assert!(suggestions.iter().all(|s| s.starts_with("TBL_")));
    }

    #[test]
    fn get_suggestions_keeps_to_underscore_matches() {
        let mut data = IntellisenseData::new();
        let suggestions = data.get_suggestions("TO_", false, None, false, false);

        // TO_CHAR is both a keyword and a function; the bare keyword form wins
        // and the `TO_CHAR()` duplicate is suppressed. TO_TIMESTAMP is a
        // function-only name, so it still renders with parentheses.
        assert!(suggestions.iter().any(|s| s == "TO_CHAR"));
        assert!(!suggestions.iter().any(|s| s == "TO_CHAR()"));
        assert!(suggestions.iter().any(|s| s == "TO_TIMESTAMP()"));
    }

    #[test]
    fn get_suggestions_include_char_keyword() {
        let mut data = IntellisenseData::new();
        let suggestions = data.get_suggestions("ch", false, None, false, false);

        assert!(suggestions.iter().any(|s| s == "CHAR"));
        assert!(!suggestions.iter().any(|s| s == "CHAR()"));
    }

    #[test]
    fn get_suggestions_include_plsql_diagnostics_as_bare_keywords() {
        let mut data = IntellisenseData::new();
        let suggestions = data.get_suggestions("sqlc", false, None, false, false);

        assert!(suggestions.iter().any(|s| s == "SQLCODE"));
        assert!(!suggestions.iter().any(|s| s == "SQLCODE()"));
    }

    #[test]
    fn get_suggestions_include_mysql_control_and_cast_keywords() {
        let mut data = IntellisenseData::new();

        let do_suggestions = data.get_suggestions_for_db(
            "do",
            false,
            None,
            false,
            false,
            Some(crate::db::DatabaseType::MySQL),
        );
        assert!(do_suggestions.iter().any(|s| s == "DO"));

        let close_suggestions = data.get_suggestions_for_db(
            "clo",
            false,
            None,
            false,
            false,
            Some(crate::db::DatabaseType::MySQL),
        );
        assert!(close_suggestions.iter().any(|s| s == "CLOSE"));

        let signed_suggestions = data.get_suggestions_for_db(
            "sig",
            false,
            None,
            false,
            false,
            Some(crate::db::DatabaseType::MySQL),
        );
        assert!(signed_suggestions.iter().any(|s| s == "SIGNED"));

        let found_suggestions = data.get_suggestions_for_db(
            "fou",
            false,
            None,
            false,
            false,
            Some(crate::db::DatabaseType::MySQL),
        );
        assert!(found_suggestions.iter().any(|s| s == "FOUND"));
    }

    #[test]
    fn get_suggestions_prefers_columns_in_column_context_with_empty_prefix() {
        let mut data = IntellisenseData::new();
        data.tables = vec!["EMP".to_string()];
        data.rebuild_indices();
        data.set_columns_for_table("EMP", vec!["EMPNO".to_string(), "ENAME".to_string()]);
        let column_scope = vec!["EMP".to_string()];

        let suggestions = data.get_suggestions("", true, Some(&column_scope), false, true);

        assert!(suggestions.contains(&"EMPNO".to_string()));
        assert!(suggestions.contains(&"ENAME".to_string()));
        assert!(!suggestions.contains(&"SELECT".to_string()));
    }

    #[test]
    fn get_suggestions_table_context_empty_prefix_returns_empty_when_no_relations() {
        let mut data = IntellisenseData::new();

        let suggestions = data.get_suggestions("", false, None, true, false);

        // Keywords are not added for empty prefix – only context-specific
        // entries (tables/views/columns) are shown.
        assert!(suggestions.is_empty());
    }

    #[test]
    fn get_suggestions_column_context_empty_prefix_stays_empty_when_columns_missing() {
        let mut data = IntellisenseData::new();
        data.tables = vec!["EMP".to_string()];
        data.rebuild_indices();
        let column_scope = vec!["EMP".to_string()];

        let suggestions = data.get_suggestions("", true, Some(&column_scope), false, true);

        assert!(suggestions.is_empty());
    }

    #[test]
    fn get_relation_suggestions_non_empty_prefix_stays_relation_only() {
        let mut data = IntellisenseData::new();
        data.tables = vec!["CONFIG".to_string()];
        data.views = vec!["COUNTS_VIEW".to_string()];
        data.rebuild_indices();

        let suggestions = data.get_relation_suggestions("co");

        assert!(suggestions.iter().any(|s| s == "CONFIG"));
        assert!(suggestions.iter().any(|s| s == "COUNTS_VIEW"));
        assert!(!suggestions.iter().any(|s| s == "COLUMN"));
        assert!(!suggestions.iter().any(|s| s == "COALESCE()"));
        assert!(!suggestions.iter().any(|s| s == "COUNT()"));
    }

    #[test]
    fn fuzzy_subsequence_matches_columns_beyond_prefix() {
        let mut data = IntellisenseData::new();
        data.set_columns_for_table(
            "EMP",
            vec![
                "EMPLOYEE_ID".into(),
                "EMPLOYEE_NAME".into(),
                "EMAIL".into(),
                "DEPARTMENT_ID".into(),
                "SALARY".into(),
            ],
        );
        let scope = vec!["EMP".to_string()];

        // Ordered-subsequence (CamelHump-style) matches that no plain prefix
        // search would surface.
        assert!(data
            .get_column_suggestions("eid", Some(&scope))
            .iter()
            .any(|s| s.eq_ignore_ascii_case("EMPLOYEE_ID")));
        assert!(data
            .get_column_suggestions("empname", Some(&scope))
            .iter()
            .any(|s| s.eq_ignore_ascii_case("EMPLOYEE_NAME")));
        assert!(data
            .get_column_suggestions("deptid", Some(&scope))
            .iter()
            .any(|s| s.eq_ignore_ascii_case("DEPARTMENT_ID")));
    }

    #[test]
    fn column_suggestions_require_an_explicit_non_empty_scope() {
        let mut data = IntellisenseData::new();
        data.set_columns_for_table("EMP", vec!["EMPLOYEE_ID".into()]);

        assert!(data.get_column_suggestions("emp", None).is_empty());
        assert!(data.get_column_suggestions("emp", Some(&[])).is_empty());
        assert_eq!(
            data.get_column_suggestions("emp", Some(&["EMP".into()])),
            vec!["EMPLOYEE_ID".to_string()]
        );
    }

    #[test]
    fn fuzzy_does_not_match_unrelated_columns() {
        let mut data = IntellisenseData::new();
        data.set_columns_for_table("EMP", vec!["SALARY".into(), "HIRE_DATE".into()]);
        let scope = vec!["EMP".to_string()];

        // No ordered subsequence "X","Y","Z" exists in either column.
        let suggestions = data.get_column_suggestions("xyz", Some(&scope));
        assert!(suggestions.is_empty(), "got {:?}", suggestions);
    }

    #[test]
    fn fuzzy_rejects_mid_word_scattered_relations() {
        // `from he|` must stay related to "he": names where "he" only occurs as a
        // mid-word scattered subsequence (WAREHOUSE: ware-H-ous-E) are noise and
        // must not surface. A boundary-anchored or contiguous "he" still matches.
        let mut data = IntellisenseData::new();
        data.tables = vec![
            "WAREHOUSE".to_string(),
            "THRESHOLD".to_string(),
            "PURCHASE_LOG".to_string(),
            "HEADER".to_string(),
            "HIRE_EVENTS".to_string(),
        ];
        data.rebuild_indices();

        let suggestions = data.get_relation_suggestions("he");

        assert!(
            suggestions.iter().any(|s| s == "HEADER"),
            "prefix match must remain: {:?}",
            suggestions
        );
        assert!(
            suggestions.iter().any(|s| s == "HIRE_EVENTS"),
            "boundary-anchored fuzzy (H...E) must remain: {:?}",
            suggestions
        );
        assert!(
            !suggestions.iter().any(|s| s == "WAREHOUSE"),
            "mid-word scattered match must be rejected: {:?}",
            suggestions
        );
        assert!(
            !suggestions.iter().any(|s| s == "THRESHOLD"),
            "mid-word scattered match must be rejected: {:?}",
            suggestions
        );
        assert!(
            !suggestions.iter().any(|s| s == "PURCHASE_LOG"),
            "mid-word scattered match must be rejected: {:?}",
            suggestions
        );
    }

    #[test]
    fn fuzzy_is_gated_to_two_or_more_characters() {
        let mut data = IntellisenseData::new();
        // "X" is a subsequence of MAX_RETRIES but a single-char prefix must not
        // trigger fuzzy noise.
        data.set_columns_for_table("T", vec!["MAX_RETRIES".into()]);
        let scope = vec!["T".to_string()];

        let suggestions = data.get_column_suggestions("x", Some(&scope));
        assert!(
            !suggestions
                .iter()
                .any(|s| s.eq_ignore_ascii_case("MAX_RETRIES")),
            "single-char fuzzy should not fire: {:?}",
            suggestions
        );
    }

    #[test]
    fn fuzzy_keeps_prefix_matches_ranked_first() {
        let mut data = IntellisenseData::new();
        data.set_columns_for_table("T", vec!["EMP_ID".into(), "TEMPLATE_ID".into()]);
        let scope = vec!["T".to_string()];

        // "emp" is a prefix of EMP_ID and a subsequence of TEMPLATE_ID; the
        // prefix match must come first.
        let suggestions = data.get_column_suggestions("emp", Some(&scope));
        assert_eq!(suggestions.first().map(String::as_str), Some("EMP_ID"));
        assert!(suggestions.iter().any(|s| s == "TEMPLATE_ID"));
    }

    #[test]
    fn fuzzy_subsequence_matches_relation_names() {
        let mut data = IntellisenseData::new();
        data.tables = vec!["CUSTOMER_ORDERS".to_string(), "PRODUCTS".to_string()];
        data.rebuild_indices();

        let suggestions = data.get_relation_suggestions("custord");
        assert!(suggestions.iter().any(|s| s == "CUSTOMER_ORDERS"));
        assert!(!suggestions.iter().any(|s| s == "PRODUCTS"));
    }

    #[test]
    fn fuzzy_subsequence_matches_qualifier_members() {
        let mut data = IntellisenseData::new();
        data.set_members_for_qualifier("pkg", vec!["CALCULATE_TOTAL".into(), "RESET".into()]);

        let suggestions = data.get_member_suggestions("pkg", "calctot", false);
        assert!(suggestions
            .iter()
            .any(|s| s.eq_ignore_ascii_case("CALCULATE_TOTAL")));
        assert!(!suggestions.iter().any(|s| s.eq_ignore_ascii_case("RESET")));
    }

    #[test]
    fn fuzzy_contiguous_substring_outranks_scattered() {
        // Lower score is better; a contiguous substring match should beat a
        // scattered subsequence match for the same needle.
        let contiguous = IntellisenseData::subsequence_match_score("AMOUNT_DUE", "AMOUNT").unwrap();
        let scattered = IntellisenseData::subsequence_match_score("A_M_O_U_N_T", "AMOUNT").unwrap();
        assert!(
            contiguous < scattered,
            "contiguous {contiguous} should rank ahead of scattered {scattered}"
        );
    }

    #[test]
    fn fuzzy_bounded_ranking_keeps_late_better_matches() {
        let mut data = IntellisenseData::new();
        let mut columns: Vec<String> = (0..MAX_SUGGESTIONS + 20)
            .map(|idx| format!("X{:03}_ALPHA_BETA", idx))
            .collect();
        columns.push("AB_TOTAL".to_string());
        data.set_columns_for_table("T", columns);
        let scope = vec!["T".to_string()];

        let suggestions = data.get_column_suggestions("ab", Some(&scope));

        assert_eq!(suggestions.first().map(String::as_str), Some("AB_TOTAL"));
        assert_eq!(suggestions.len(), MAX_SUGGESTIONS);
    }

    #[test]
    fn get_word_at_cursor_supports_unicode_identifier() {
        let sql = "SELECT 한글컬럼 FROM dual";
        let cursor = sql.find(" FROM").unwrap_or(sql.len());
        let (word, _, _) = get_word_at_cursor(sql, cursor);
        assert_eq!(word, "한글컬럼");
    }

    #[test]
    fn get_word_at_cursor_clamps_non_boundary_utf8_offset() {
        let sql = "SELECT 한글컬럼 FROM dual";
        let cursor = sql.find('한').expect("expected utf-8 anchor") + 1;
        let (word, _, _) = get_word_at_cursor(sql, cursor);
        assert_eq!(word, "한글컬럼");
    }

    #[test]
    fn get_word_at_cursor_expands_incomplete_double_quoted_identifier() {
        let sql = r#"SELECT rec."Street N FROM dual"#;
        let cursor = sql.find(" FROM").expect("expected cursor anchor");
        let (word, start, end) = get_word_at_cursor(sql, cursor);

        assert_eq!(word, r#""Street N"#);
        assert_eq!(sql.get(start..cursor), Some(r#""Street N"#));
        assert_eq!(end, cursor);
    }

    #[test]
    fn get_word_at_cursor_expands_incomplete_backtick_identifier() {
        let sql = "SELECT rec.`Street N FROM dual";
        let cursor = sql.find(" FROM").expect("expected cursor anchor");
        let (word, start, end) = get_word_at_cursor(sql, cursor);

        assert_eq!(word, "`Street N");
        assert_eq!(sql.get(start..cursor), Some("`Street N"));
        assert_eq!(end, cursor);
    }

    #[test]
    fn get_word_at_cursor_expands_incomplete_bracket_identifier() {
        let sql = "SELECT rec.[Street N FROM dual";
        let cursor = sql.find(" FROM").expect("expected cursor anchor");
        let (word, start, end) = get_word_at_cursor(sql, cursor);

        assert_eq!(word, "[Street N");
        assert_eq!(sql.get(start..cursor), Some("[Street N"));
        assert_eq!(end, cursor);
    }

    #[test]
    fn get_word_at_cursor_ignores_completed_quoted_identifier_before_cursor() {
        let sql = r#"SELECT rec."Street Name".ci FROM dual"#;
        let cursor = sql.find(" FROM").expect("expected cursor anchor");
        let (word, start, _) = get_word_at_cursor(sql, cursor);

        assert_eq!(word, "ci");
        assert_eq!(sql.get(start..cursor), Some("ci"));
    }

    #[test]
    fn get_word_at_cursor_ignores_completed_bracket_identifier_before_cursor() {
        let sql = "SELECT rec.[Street Name].ci FROM dual";
        let cursor = sql.find(" FROM").expect("expected cursor anchor");
        let (word, start, _) = get_word_at_cursor(sql, cursor);

        assert_eq!(word, "ci");
        assert_eq!(sql.get(start..cursor), Some("ci"));
    }

    #[test]
    fn get_word_at_cursor_ignores_bracket_inside_single_quoted_literal() {
        let sql = "SELECT CONCAT(' [expected=', p_)";
        let cursor = sql.find("p_").expect("expected cursor anchor") + 2;
        let (word, start, _) = get_word_at_cursor(sql, cursor);

        assert_eq!(word, "p_");
        assert_eq!(sql.get(start..cursor), Some("p_"));
    }

    #[test]
    fn get_word_at_cursor_ignores_quote_inside_oracle_q_literal() {
        let sql = r#"INSERT INTO t VALUES (q'[RETURNING test ; / '' " ]');
BEGIN
  pkg.proc(p_)"#;
        let cursor = sql.find("p_").expect("expected cursor anchor") + 2;
        let (word, start, _) = get_word_at_cursor(sql, cursor);

        assert_eq!(word, "p_");
        assert_eq!(sql.get(start..cursor), Some("p_"));
    }

    #[test]
    fn get_word_at_cursor_uses_parser_engine_for_prefixed_nested_q_literals() {
        let sql =
            r#"SELECT nq'[outer q'[nested]' fake " ), ( tail]' AS txt, rec."Street N FROM dual"#;
        let cursor = sql.find(" FROM").expect("expected cursor anchor");
        let (word, start, end) = get_word_at_cursor(sql, cursor);

        assert_eq!(word, r#""Street N"#);
        assert_eq!(sql.get(start..cursor), Some(r#""Street N"#));
        assert_eq!(end, cursor);
    }

    #[test]
    fn get_word_at_cursor_accepts_bounded_window_entry_lex_mode() {
        let text = r#"payload " ), ( ]'; rec."Street N FROM dual"#;
        let cursor = text.find(" FROM").expect("expected cursor anchor");
        let (word, start, end) = get_word_at_cursor_with_lexical_mode(
            text,
            cursor,
            false,
            crate::sql_parser_engine::LexMode::QQuote {
                end_char: ']',
                depth: 1,
            },
        );

        assert_eq!(word, r#""Street N"#);
        assert_eq!(text.get(start..cursor), Some(r#""Street N"#));
        assert_eq!(end, cursor);
    }

    #[test]
    fn get_word_at_cursor_does_not_extend_quoted_identifier_across_a_previous_line() {
        // Mirrors a bounded editor window that begins in the middle of a
        // multiline q-quoted literal: the earlier `"` has no bearing on the
        // plain identifier being typed on the current line.
        let text = "q-quote fragment with a double quote \"\n]';\nPROCEDURE p(x BOOL";
        let (word, start, end) = get_word_at_cursor(text, text.len());
        assert_eq!(word, "BOOL");
        assert_eq!(text.get(start..end), Some("BOOL"));
    }

    #[test]
    fn get_word_at_cursor_ignores_quote_inside_line_comment() {
        let sql = "-- \"not an identifier\nSELECT v_na";
        let cursor = sql.len();
        let (word, start, _) = get_word_at_cursor(sql, cursor);

        assert_eq!(word, "v_na");
        assert_eq!(sql.get(start..cursor), Some("v_na"));
    }

    #[test]
    fn get_word_at_cursor_ignores_quote_inside_block_comment() {
        let sql = "/* \"not an identifier */\nSELECT v_na";
        let cursor = sql.len();
        let (word, start, _) = get_word_at_cursor(sql, cursor);

        assert_eq!(word, "v_na");
        assert_eq!(sql.get(start..cursor), Some("v_na"));
    }

    #[test]
    fn detect_sql_context_clamps_non_char_boundary_cursor() {
        let sql = "SELECT 한글컬럼 FROM dual";
        let cursor = sql.find("한").unwrap_or(0) + 1;
        let result = std::panic::catch_unwind(|| detect_sql_context(sql, cursor));
        assert!(result.is_ok());
    }

    #[test]
    fn normalize_cursor_pos_clamps_non_boundary_utf8_offset() {
        let sql = "SELECT 한글컬럼 FROM dual";
        let utf8_start = sql.find('한').expect("expected utf-8 anchor");
        let mid_char = utf8_start + 1;
        assert!(!sql.is_char_boundary(mid_char));
        assert_eq!(normalize_cursor_pos(sql, mid_char), utf8_start);
    }

    #[test]
    fn detect_sql_context_clamps_non_boundary_utf8_offset() {
        let sql = "SELECT 한글컬럼 FROM dual";
        let utf8_start = sql.find('한').expect("expected utf-8 anchor");
        let mid_char = utf8_start + 1;
        assert!(!sql.is_char_boundary(mid_char));
        assert_eq!(
            detect_sql_context(sql, mid_char),
            detect_sql_context(sql, utf8_start)
        );
    }

    #[test]
    fn detect_sql_context_qualify_clause_is_column_name() {
        let sql_with_cursor = "SELECT a FROM t QUALIFY |";
        let cursor = sql_with_cursor
            .find('|')
            .expect("expected cursor marker in SQL");
        let sql = format!(
            "{}{}",
            &sql_with_cursor[..cursor],
            &sql_with_cursor[cursor + 1..]
        );
        assert_eq!(detect_sql_context(&sql, cursor), SqlContext::ColumnName);
    }

    #[test]
    fn detect_sql_context_returning_clause_is_column_name() {
        let sql_with_cursor = "INSERT INTO t (a) VALUES (1) RETURNING | INTO :a";
        let cursor = sql_with_cursor
            .find('|')
            .expect("expected cursor marker in SQL");
        let sql = format!(
            "{}{}",
            &sql_with_cursor[..cursor],
            &sql_with_cursor[cursor + 1..]
        );
        assert_eq!(detect_sql_context(&sql, cursor), SqlContext::ColumnName);
    }

    #[test]
    fn detect_sql_context_update_set_target_is_column_name() {
        let sql_with_cursor = "UPDATE emp SET |";
        let cursor = sql_with_cursor
            .find('|')
            .expect("expected cursor marker in SQL");
        let sql = format!(
            "{}{}",
            &sql_with_cursor[..cursor],
            &sql_with_cursor[cursor + 1..]
        );
        assert_eq!(detect_sql_context(&sql, cursor), SqlContext::ColumnName);
    }

    #[test]
    fn detect_sql_context_merge_update_set_target_is_column_name() {
        let sql_with_cursor =
            "MERGE INTO tgt t USING src s ON (t.id = s.id) WHEN MATCHED THEN UPDATE SET |";
        let cursor = sql_with_cursor
            .find('|')
            .expect("expected cursor marker in SQL");
        let sql = format!(
            "{}{}",
            &sql_with_cursor[..cursor],
            &sql_with_cursor[cursor + 1..]
        );
        assert_eq!(detect_sql_context(&sql, cursor), SqlContext::ColumnName);
    }

    #[test]
    fn detect_sql_context_select_into_target_is_variable_name() {
        let sql_with_cursor = "BEGIN SELECT empno INTO | FROM emp; END;";
        let cursor = sql_with_cursor
            .find('|')
            .expect("expected cursor marker in SQL");
        let sql = format!(
            "{}{}",
            &sql_with_cursor[..cursor],
            &sql_with_cursor[cursor + 1..]
        );
        assert_eq!(detect_sql_context(&sql, cursor), SqlContext::VariableName);
    }

    #[test]
    fn detect_sql_context_returning_into_target_is_variable_name() {
        let sql_with_cursor = "UPDATE emp SET sal = sal + 1 RETURNING empno INTO |";
        let cursor = sql_with_cursor
            .find('|')
            .expect("expected cursor marker in SQL");
        let sql = format!(
            "{}{}",
            &sql_with_cursor[..cursor],
            &sql_with_cursor[cursor + 1..]
        );
        assert_eq!(detect_sql_context(&sql, cursor), SqlContext::VariableName);
    }

    #[test]
    fn detect_sql_context_fetch_into_target_is_variable_name() {
        let sql_with_cursor = "BEGIN FETCH c_emp INTO |; END;";
        let cursor = sql_with_cursor
            .find('|')
            .expect("expected cursor marker in SQL");
        let sql = format!(
            "{}{}",
            &sql_with_cursor[..cursor],
            &sql_with_cursor[cursor + 1..]
        );
        assert_eq!(detect_sql_context(&sql, cursor), SqlContext::VariableName);
    }

    #[test]
    fn detect_sql_context_execute_immediate_using_is_bind_value() {
        let sql_with_cursor =
            "BEGIN EXECUTE IMMEDIATE 'select count(*) from emp where deptno = :1' INTO l_cnt USING |; END;";
        let cursor = sql_with_cursor
            .find('|')
            .expect("expected cursor marker in SQL");
        let sql = format!(
            "{}{}",
            &sql_with_cursor[..cursor],
            &sql_with_cursor[cursor + 1..]
        );
        assert_eq!(detect_sql_context(&sql, cursor), SqlContext::BindValue);
    }

    #[test]
    fn detect_sql_context_open_for_using_is_bind_value() {
        let sql_with_cursor = "BEGIN OPEN c FOR l_sql USING |; END;";
        let cursor = sql_with_cursor
            .find('|')
            .expect("expected cursor marker in SQL");
        let sql = format!(
            "{}{}",
            &sql_with_cursor[..cursor],
            &sql_with_cursor[cursor + 1..]
        );
        assert_eq!(detect_sql_context(&sql, cursor), SqlContext::BindValue);
    }

    #[test]
    fn detect_sql_context_join_using_clause_is_column_name() {
        let sql_with_cursor = "SELECT * FROM employees e JOIN departments d USING (|)";
        let cursor = sql_with_cursor
            .find('|')
            .expect("expected cursor marker in SQL");
        let sql = format!(
            "{}{}",
            &sql_with_cursor[..cursor],
            &sql_with_cursor[cursor + 1..]
        );
        assert_eq!(detect_sql_context(&sql, cursor), SqlContext::ColumnName);
    }

    #[test]
    fn detect_sql_context_on_conflict_target_list_is_column_name() {
        let sql_with_cursor =
            "INSERT INTO t (id, val) VALUES (1, 2) ON CONFLICT (|) DO UPDATE SET val = EXCLUDED.val";
        let cursor = sql_with_cursor
            .find('|')
            .expect("expected cursor marker in SQL");
        let sql = format!(
            "{}{}",
            &sql_with_cursor[..cursor],
            &sql_with_cursor[cursor + 1..]
        );
        assert_eq!(detect_sql_context(&sql, cursor), SqlContext::ColumnName);
    }

    #[test]
    fn detect_sql_context_recursive_cte_search_by_is_column_name() {
        let sql_with_cursor =
            "WITH t(n) AS (SELECT 1 FROM dual UNION ALL SELECT n + 1 FROM t WHERE n < 3) SEARCH DEPTH FIRST BY | SET ord SELECT * FROM t";
        let cursor = sql_with_cursor
            .find('|')
            .expect("expected cursor marker in SQL");
        let sql = format!(
            "{}{}",
            &sql_with_cursor[..cursor],
            &sql_with_cursor[cursor + 1..]
        );
        assert_eq!(detect_sql_context(&sql, cursor), SqlContext::ColumnName);
    }

    #[test]
    fn detect_sql_context_recursive_cte_cycle_set_is_generated_name() {
        let sql_with_cursor =
            "WITH t(n) AS (SELECT 1 FROM dual UNION ALL SELECT n + 1 FROM t WHERE n < 3) CYCLE n SET | TO 1 DEFAULT 0 SELECT * FROM t";
        let cursor = sql_with_cursor
            .find('|')
            .expect("expected cursor marker in SQL");
        let sql = format!(
            "{}{}",
            &sql_with_cursor[..cursor],
            &sql_with_cursor[cursor + 1..]
        );
        assert_eq!(detect_sql_context(&sql, cursor), SqlContext::GeneratedName);
    }

    #[test]
    fn detect_sql_context_hierarchical_search_set_is_generated_name() {
        let sql_with_cursor =
            "SELECT * FROM emp CONNECT BY PRIOR empno = mgr SEARCH DEPTH FIRST BY empno SET |";
        let cursor = sql_with_cursor
            .find('|')
            .expect("expected cursor marker in SQL");
        let sql = format!(
            "{}{}",
            &sql_with_cursor[..cursor],
            &sql_with_cursor[cursor + 1..]
        );
        assert_eq!(detect_sql_context(&sql, cursor), SqlContext::GeneratedName);
    }

    #[test]
    fn detect_sql_context_hierarchical_cycle_set_is_generated_name() {
        let sql_with_cursor =
            "SELECT * FROM emp CONNECT BY PRIOR empno = mgr CYCLE empno SET | TO 'Y' DEFAULT 'N'";
        let cursor = sql_with_cursor
            .find('|')
            .expect("expected cursor marker in SQL");
        let sql = format!(
            "{}{}",
            &sql_with_cursor[..cursor],
            &sql_with_cursor[cursor + 1..]
        );
        assert_eq!(detect_sql_context(&sql, cursor), SqlContext::GeneratedName);
    }

    #[test]
    fn detect_sql_context_for_update_of_is_column_name() {
        let sql_with_cursor = "SELECT * FROM emp FOR UPDATE OF |";
        let cursor = sql_with_cursor
            .find('|')
            .expect("expected cursor marker in SQL");
        let sql = format!(
            "{}{}",
            &sql_with_cursor[..cursor],
            &sql_with_cursor[cursor + 1..]
        );
        assert_eq!(detect_sql_context(&sql, cursor), SqlContext::ColumnName);
    }

    #[test]
    fn detect_sql_context_pivot_sum_argument_is_column_name() {
        let sql_with_cursor = "WITH s AS (SELECT DEPTNO, job, sal FROM oqt_t_emp) SELECT * FROM s PIVOT (SUM(|) AS sum_sal FOR DEPTNO IN (10 AS D10))";
        let cursor = sql_with_cursor
            .find('|')
            .expect("expected cursor marker in SQL");
        let sql = format!(
            "{}{}",
            &sql_with_cursor[..cursor],
            &sql_with_cursor[cursor + 1..]
        );
        assert_eq!(detect_sql_context(&sql, cursor), SqlContext::ColumnName);
    }

    #[test]
    fn detect_sql_context_pivot_for_expression_is_column_name() {
        let sql_with_cursor = "WITH s AS (SELECT DEPTNO, job, sal FROM oqt_t_emp) SELECT * FROM s PIVOT (SUM(sal) AS sum_sal FOR | IN (10 AS D10))";
        let cursor = sql_with_cursor
            .find('|')
            .expect("expected cursor marker in SQL");
        let sql = format!(
            "{}{}",
            &sql_with_cursor[..cursor],
            &sql_with_cursor[cursor + 1..]
        );
        assert_eq!(detect_sql_context(&sql, cursor), SqlContext::ColumnName);
    }

    #[test]
    fn detect_sql_context_match_recognize_define_is_column_name() {
        let sql_with_cursor =
            "SELECT * FROM sales MATCH_RECOGNIZE (PARTITION BY dept ORDER BY ts DEFINE A AS |)";
        let cursor = sql_with_cursor
            .find('|')
            .expect("expected cursor marker in SQL");
        let sql = format!(
            "{}{}",
            &sql_with_cursor[..cursor],
            &sql_with_cursor[cursor + 1..]
        );
        assert_eq!(detect_sql_context(&sql, cursor), SqlContext::ColumnName);
    }

    #[test]
    fn detect_sql_context_model_measures_is_column_name() {
        let sql_with_cursor =
            "SELECT * FROM sales MODEL DIMENSION BY (deptno) MEASURES (|) RULES ()";
        let cursor = sql_with_cursor
            .find('|')
            .expect("expected cursor marker in SQL");
        let sql = format!(
            "{}{}",
            &sql_with_cursor[..cursor],
            &sql_with_cursor[cursor + 1..]
        );
        assert_eq!(detect_sql_context(&sql, cursor), SqlContext::ColumnName);
    }

    #[test]
    fn detect_sql_context_insert_values_clause_is_column_name() {
        let sql_with_cursor = "INSERT INTO t (id, val) VALUES (|)";
        let cursor = sql_with_cursor
            .find('|')
            .expect("expected cursor marker in SQL");
        let sql = format!(
            "{}{}",
            &sql_with_cursor[..cursor],
            &sql_with_cursor[cursor + 1..]
        );
        assert_eq!(detect_sql_context(&sql, cursor), SqlContext::ColumnName);
    }

    #[test]
    fn detect_sql_context_merge_insert_values_clause_is_column_name() {
        let sql_with_cursor = "MERGE INTO tgt t USING src s ON (t.id = s.id) WHEN NOT MATCHED THEN INSERT (id) VALUES (s.|)";
        let cursor = sql_with_cursor
            .find('|')
            .expect("expected cursor marker in SQL");
        let sql = format!(
            "{}{}",
            &sql_with_cursor[..cursor],
            &sql_with_cursor[cursor + 1..]
        );
        assert_eq!(detect_sql_context(&sql, cursor), SqlContext::ColumnName);
    }

    #[test]
    fn detect_sql_context_insert_all_into_clause_is_table_name() {
        let sql_with_cursor = "INSERT ALL INTO | (id) VALUES (1) SELECT 1 FROM dual";
        let cursor = sql_with_cursor
            .find('|')
            .expect("expected cursor marker in SQL");
        let sql = format!(
            "{}{}",
            &sql_with_cursor[..cursor],
            &sql_with_cursor[cursor + 1..]
        );
        assert_eq!(detect_sql_context(&sql, cursor), SqlContext::TableName);
    }

    #[test]
    fn detect_sql_context_insert_all_after_first_values_into_clause_is_table_name() {
        let sql_with_cursor =
            "INSERT ALL INTO t1 (id) VALUES (1) INTO | (id) VALUES (2) SELECT 1 FROM dual";
        let cursor = sql_with_cursor
            .find('|')
            .expect("expected cursor marker in SQL");
        let sql = format!(
            "{}{}",
            &sql_with_cursor[..cursor],
            &sql_with_cursor[cursor + 1..]
        );
        assert_eq!(detect_sql_context(&sql, cursor), SqlContext::TableName);
    }

    #[test]
    fn detect_sql_context_insert_first_else_into_clause_is_table_name() {
        let sql_with_cursor =
            "INSERT FIRST WHEN score >= 90 THEN INTO top_rank (id) VALUES (1) ELSE INTO | (id) VALUES (2) SELECT 1 score FROM dual";
        let cursor = sql_with_cursor
            .find('|')
            .expect("expected cursor marker in SQL");
        let sql = format!(
            "{}{}",
            &sql_with_cursor[..cursor],
            &sql_with_cursor[cursor + 1..]
        );
        assert_eq!(detect_sql_context(&sql, cursor), SqlContext::TableName);
    }

    #[test]
    fn detect_sql_context_outer_apply_rhs_is_table_name() {
        let sql_with_cursor = "SELECT * FROM t1 OUTER APPLY |";
        let cursor = sql_with_cursor
            .find('|')
            .expect("expected cursor marker in SQL");
        let sql = format!(
            "{}{}",
            &sql_with_cursor[..cursor],
            &sql_with_cursor[cursor + 1..]
        );
        assert_eq!(detect_sql_context(&sql, cursor), SqlContext::TableName);
    }

    #[test]
    fn detect_sql_context_with_cte_explicit_column_list_is_column_name() {
        let sql_with_cursor = "WITH cte(id, |) AS (SELECT 1, 2 FROM dual) SELECT * FROM cte";
        let cursor = sql_with_cursor
            .find('|')
            .expect("expected cursor marker in SQL");
        let sql = format!(
            "{}{}",
            &sql_with_cursor[..cursor],
            &sql_with_cursor[cursor + 1..]
        );
        assert_eq!(detect_sql_context(&sql, cursor), SqlContext::ColumnName);
    }

    #[test]
    fn detect_sql_context_second_cte_explicit_column_list_is_column_name() {
        let sql_with_cursor =
            "WITH c1(a) AS (SELECT 1 FROM dual), c2(x, |) AS (SELECT 1, 2 FROM dual) SELECT * FROM c2";
        let cursor = sql_with_cursor
            .find('|')
            .expect("expected cursor marker in SQL");
        let sql = format!(
            "{}{}",
            &sql_with_cursor[..cursor],
            &sql_with_cursor[cursor + 1..]
        );
        assert_eq!(detect_sql_context(&sql, cursor), SqlContext::ColumnName);
    }

    #[test]
    fn get_suggestions_includes_exact_prefix_match() {
        let mut data = IntellisenseData::new();
        data.tables = vec!["AB".to_string(), "ABC_TABLE".to_string()];
        data.rebuild_indices();

        let suggestions = data.get_suggestions("ab", false, None, false, false);

        assert!(suggestions.iter().any(|s| s.eq_ignore_ascii_case("ab")));
        assert!(suggestions
            .iter()
            .any(|s| s.eq_ignore_ascii_case("abc_table")));
    }

    #[test]
    fn get_suggestions_includes_exact_keyword_match() {
        let mut data = IntellisenseData::new();

        let suggestions = data.get_suggestions("as", false, None, false, false);

        assert!(suggestions.iter().any(|s| s.eq_ignore_ascii_case("as")));
    }

    #[test]
    fn get_suggestions_includes_exact_function_prefix_match() {
        let mut data = IntellisenseData::new();

        // ABS is a function-only name (not also a keyword), so it renders with
        // parentheses. SUM, by contrast, is both keyword and function and now
        // surfaces only as the bare keyword.
        let suggestions = data.get_suggestions("abs", false, None, false, false);

        assert!(suggestions.iter().any(|s| s.eq_ignore_ascii_case("abs()")));
    }

    #[test]
    fn filter_suggestions_by_prefix_empty_prefix_keeps_all() {
        let suggestions = vec!["SELECT".to_string(), "FROM".to_string()];
        let filtered = filter_suggestions_by_prefix(&suggestions, "");
        assert_eq!(filtered, suggestions);
    }

    #[test]
    fn filter_suggestions_by_prefix_case_insensitive_and_underscore() {
        let suggestions = vec![
            "TO_CHAR".to_string(),
            "to_date".to_string(),
            "TABLE".to_string(),
        ];
        let filtered = filter_suggestions_by_prefix(&suggestions, "to_");
        assert_eq!(filtered, vec!["TO_CHAR".to_string(), "to_date".to_string()]);
    }

    #[test]
    fn filter_suggestions_by_prefix_no_match_returns_empty() {
        let suggestions = vec!["SELECT".to_string(), "FROM".to_string()];
        let filtered = filter_suggestions_by_prefix(&suggestions, "zz");
        assert!(filtered.is_empty());
    }

    #[test]
    fn filter_suggestions_by_prefix_matches_condition_comparison_left_column() {
        let suggestions = vec![
            "a.TOTAL = b.TOTAL".to_string(),
            "a.NAME = b.NAME".to_string(),
        ];
        let filtered = filter_suggestions_by_prefix(&suggestions, "to");
        assert_eq!(filtered, vec!["a.TOTAL = b.TOTAL".to_string()]);
    }

    #[test]
    fn filter_suggestions_by_prefix_matches_quoted_condition_comparison_left_column() {
        let suggestions = vec![
            "a.\"Order Id\" = b.\"Order Id\"".to_string(),
            "a.\"Dept No\" = b.\"Dept No\"".to_string(),
        ];
        let filtered = filter_suggestions_by_prefix(&suggestions, "or");
        assert_eq!(
            filtered,
            vec!["a.\"Order Id\" = b.\"Order Id\"".to_string()]
        );
    }

    #[test]
    fn filter_suggestions_by_prefix_matches_quoted_identifier_by_unquoted_prefix() {
        let suggestions = vec![
            r#""Street.Name""#.to_string(),
            "STATUS".to_string(),
            "city".to_string(),
        ];
        let filtered = filter_suggestions_by_prefix(&suggestions, "St");

        assert_eq!(
            filtered,
            vec![r#""Street.Name""#.to_string(), "STATUS".to_string()]
        );
    }

    #[test]
    fn filter_suggestions_by_prefix_matches_quoted_identifier_by_incomplete_quoted_prefix() {
        let suggestions = vec![
            r#""Street.Name""#.to_string(),
            "STATUS".to_string(),
            "city".to_string(),
        ];
        let filtered = filter_suggestions_by_prefix(&suggestions, r#""St"#);

        assert_eq!(filtered, vec![r#""Street.Name""#.to_string()]);
    }

    #[test]
    fn filter_suggestions_by_prefix_preserves_space_inside_incomplete_quoted_prefix() {
        let suggestions = vec![
            r#""Order Id""#.to_string(),
            r#""Order Date""#.to_string(),
            "ORDER_TOTAL".to_string(),
        ];
        let filtered = filter_suggestions_by_prefix(&suggestions, r#""Order I"#);

        assert_eq!(filtered, vec![r#""Order Id""#.to_string()]);
    }

    #[test]
    fn filter_suggestions_by_prefix_matches_backtick_identifier_by_incomplete_prefix() {
        let suggestions = vec![
            "`Street.Name`".to_string(),
            "STATUS".to_string(),
            "city".to_string(),
        ];
        let filtered = filter_suggestions_by_prefix(&suggestions, "`St");

        assert_eq!(filtered, vec!["`Street.Name`".to_string()]);
    }

    #[test]
    fn filter_suggestions_by_prefix_matches_bracket_identifier_by_unquoted_prefix() {
        let suggestions = vec![
            "[Item Id]".to_string(),
            "[order]".to_string(),
            "plain_name".to_string(),
        ];
        let filtered = filter_suggestions_by_prefix(&suggestions, "Item");

        assert_eq!(filtered, vec!["[Item Id]".to_string()]);
    }

    #[test]
    fn filter_suggestions_by_prefix_matches_bracket_identifier_by_incomplete_prefix() {
        let suggestions = vec![
            "[Item Id]".to_string(),
            "[order]".to_string(),
            "plain_name".to_string(),
        ];
        let filtered = filter_suggestions_by_prefix(&suggestions, "[It");

        assert_eq!(filtered, vec!["[Item Id]".to_string()]);
    }

    #[test]
    fn filter_suggestions_by_prefix_matches_escaped_bracket_identifier_by_unescaped_prefix() {
        let suggestions = vec![
            "[Item]]Id]".to_string(),
            "[order]]line]".to_string(),
            "plain_name".to_string(),
        ];
        let filtered = filter_suggestions_by_prefix(&suggestions, "Item]I");

        assert_eq!(filtered, vec!["[Item]]Id]".to_string()]);
    }

    #[test]
    fn filter_suggestions_by_prefix_matches_quoted_condition_by_incomplete_quoted_prefix() {
        let suggestions = vec![
            "a.\"Order Id\" = b.\"Order Id\"".to_string(),
            "a.order_no = b.order_no".to_string(),
        ];
        let filtered = filter_suggestions_by_prefix(&suggestions, r#""Or"#);

        assert_eq!(
            filtered,
            vec!["a.\"Order Id\" = b.\"Order Id\"".to_string()]
        );
    }

    #[test]
    fn filter_suggestions_by_prefix_matches_quoted_condition_with_dot_in_column_name() {
        let suggestions = vec![
            r#"a."Street.Name" = b."Street.Name""#.to_string(),
            r#"a."Status.Flag" = b."Status.Flag""#.to_string(),
        ];
        let filtered = filter_suggestions_by_prefix(&suggestions, "Street");

        assert_eq!(
            filtered,
            vec![r#"a."Street.Name" = b."Street.Name""#.to_string()]
        );
    }

    #[test]
    fn filter_suggestions_by_prefix_matches_quoted_condition_with_dot_by_quoted_prefix() {
        let suggestions = vec![
            r#"a."Street.Name" = b."Street.Name""#.to_string(),
            r#"a."Status.Flag" = b."Status.Flag""#.to_string(),
        ];
        let filtered = filter_suggestions_by_prefix(&suggestions, r#""Street"#);

        assert_eq!(
            filtered,
            vec![r#"a."Street.Name" = b."Street.Name""#.to_string()]
        );
    }

    #[test]
    fn filter_suggestions_by_prefix_matches_backtick_condition_with_dot_in_column_name() {
        let suggestions = vec![
            "a.`Street.Name` = b.`Street.Name`".to_string(),
            "a.`Status.Flag` = b.`Status.Flag`".to_string(),
        ];
        let filtered = filter_suggestions_by_prefix(&suggestions, "`Street");

        assert_eq!(
            filtered,
            vec!["a.`Street.Name` = b.`Street.Name`".to_string()]
        );
    }

    #[test]
    fn filter_suggestions_by_prefix_matches_quoted_condition_with_equals_in_column_name() {
        let suggestions = vec![
            r#"a."A=B" = b."A=B""#.to_string(),
            r#"a."A=C" = b."A=C""#.to_string(),
        ];
        let filtered = filter_suggestions_by_prefix(&suggestions, r#""A=B"#);

        assert_eq!(filtered, vec![r#"a."A=B" = b."A=B""#.to_string()]);
    }

    #[test]
    fn filter_suggestions_by_prefix_matches_backtick_condition_with_equals_in_column_name() {
        let suggestions = vec![
            "a.`A=B` = b.`A=B`".to_string(),
            "a.`A=C` = b.`A=C`".to_string(),
        ];
        let filtered = filter_suggestions_by_prefix(&suggestions, "`A=B");

        assert_eq!(filtered, vec!["a.`A=B` = b.`A=B`".to_string()]);
    }

    #[test]
    fn filter_suggestions_by_prefix_matches_bracket_condition_column() {
        let suggestions = vec![
            "a.[Item Id] = b.[Item Id]".to_string(),
            "a.[order] = b.[order]".to_string(),
        ];
        let filtered = filter_suggestions_by_prefix(&suggestions, "Item");

        assert_eq!(filtered, vec!["a.[Item Id] = b.[Item Id]".to_string()]);
    }

    #[test]
    fn filter_suggestions_by_prefix_matches_escaped_bracket_condition_column() {
        let suggestions = vec![
            "a.[Item]]Id] = b.[Item]]Id]".to_string(),
            "a.[order]]line] = b.[order]]line]".to_string(),
        ];
        let filtered = filter_suggestions_by_prefix(&suggestions, "Item]I");

        assert_eq!(filtered, vec!["a.[Item]]Id] = b.[Item]]Id]".to_string()]);
    }

    #[test]
    fn filter_suggestions_by_prefix_does_not_treat_named_argument_arrow_as_condition() {
        let suggestions = vec!["pkg.proc(arg => value)".to_string()];
        let filtered = filter_suggestions_by_prefix(&suggestions, "proc");

        assert!(filtered.is_empty());
    }

    #[test]
    fn filter_suggestions_by_prefix_does_not_treat_unspaced_equals_as_condition() {
        let suggestions = vec!["a.abc=b.abc".to_string()];
        let filtered = filter_suggestions_by_prefix(&suggestions, "abc");

        assert!(filtered.is_empty());
    }

    #[test]
    fn popup_page_selection_advances_by_page_size_and_clamps_to_end() {
        assert_eq!(IntellisensePopup::next_page_selection(1, 25), Some(11));
        assert_eq!(IntellisensePopup::next_page_selection(20, 25), Some(25));
        assert_eq!(IntellisensePopup::next_page_selection(0, 7), Some(7));
        assert_eq!(IntellisensePopup::next_page_selection(1, 0), None);
    }

    #[test]
    fn popup_page_selection_moves_up_by_page_size_and_clamps_to_start() {
        assert_eq!(IntellisensePopup::prev_page_selection(21, 30), Some(11));
        assert_eq!(IntellisensePopup::prev_page_selection(5, 30), Some(1));
        assert_eq!(IntellisensePopup::prev_page_selection(0, 8), Some(1));
        assert_eq!(IntellisensePopup::prev_page_selection(3, 0), None);
    }

    #[test]
    fn popup_pointer_gesture_accepts_left_click_without_drag() {
        let mut gesture = PopupPointerGesture::default();

        gesture.handle_event(fltk::enums::Event::Push, 10, 10, 1);
        gesture.handle_event(fltk::enums::Event::Released, 12, 11, 1);

        assert!(gesture.take_selection_allowed());
        assert!(!gesture.take_selection_allowed());
    }

    #[test]
    fn popup_pointer_gesture_rejects_drag_and_non_left_release() {
        let mut dragged = PopupPointerGesture::default();
        dragged.handle_event(fltk::enums::Event::Push, 10, 10, 1);
        dragged.handle_event(fltk::enums::Event::Drag, 20, 10, 1);
        dragged.handle_event(fltk::enums::Event::Released, 20, 10, 1);
        assert!(!dragged.take_selection_allowed());

        let mut right_click = PopupPointerGesture::default();
        right_click.handle_event(fltk::enums::Event::Push, 10, 10, 3);
        right_click.handle_event(fltk::enums::Event::Released, 10, 10, 3);
        assert!(!right_click.take_selection_allowed());
    }

    #[test]
    fn signature_cache_is_bounded_and_keeps_latest_result() {
        let mut data = IntellisenseData::new();
        for index in 0..=MAX_SIGNATURE_CACHE_ENTRIES {
            data.set_signature(
                format!("PROC_{index}"),
                Some(SignatureLabel {
                    text: format!("PROC_{index}()"),
                    arg_spans: Vec::new(),
                }),
            );
        }

        assert!(data.signature_cache.len() <= MAX_SIGNATURE_CACHE_ENTRIES);
        assert!(data.cached_signature("PROC_0").is_none());
        assert!(data.cached_signature("PROC_1").is_some());
        assert!(data
            .cached_signature(&format!("PROC_{MAX_SIGNATURE_CACHE_ENTRIES}"))
            .is_some());
    }

    #[test]
    fn sql_keywords_has_no_duplicates() {
        let mut seen = std::collections::HashSet::new();
        for keyword in SQL_KEYWORDS {
            assert!(
                seen.insert(*keyword),
                "Duplicate SQL keyword found: {}",
                keyword
            );
        }
    }

    #[test]
    fn oracle_functions_has_no_duplicates() {
        let mut seen = std::collections::HashSet::new();
        for func in ORACLE_FUNCTIONS {
            assert!(
                seen.insert(*func),
                "Duplicate Oracle function found: {}",
                func
            );
        }
    }

    #[test]
    fn sql_keywords_is_sorted() {
        for pair in SQL_KEYWORDS.windows(2) {
            assert!(
                pair[0] <= pair[1],
                "SQL_KEYWORDS not sorted: {:?} > {:?}",
                pair[0],
                pair[1]
            );
        }
    }

    #[test]
    fn oracle_functions_is_sorted() {
        for pair in ORACLE_FUNCTIONS.windows(2) {
            assert!(
                pair[0] <= pair[1],
                "ORACLE_FUNCTIONS not sorted: {:?} > {:?}",
                pair[0],
                pair[1]
            );
        }
    }

    #[test]
    fn get_suggestions_deduplicates_case_insensitive_columns() {
        let mut data = IntellisenseData::new();
        data.set_columns_for_table("EMP", vec!["EmpNo".to_string(), "EMPNO".to_string()]);
        let column_scope = vec!["emp".to_string()];

        let suggestions = data.get_suggestions("", true, Some(&column_scope), false, true);

        let empno_count = suggestions
            .iter()
            .filter(|value| value.eq_ignore_ascii_case("EMPNO"))
            .count();
        assert_eq!(empno_count, 1);
        assert_eq!(suggestions.len(), 1);
        assert!(suggestions
            .iter()
            .any(|value| value.eq_ignore_ascii_case("EMPNO")));
    }

    #[test]
    fn get_suggestions_deduplicates_case_insensitive_relations() {
        let mut data = IntellisenseData::new();
        data.tables = vec!["Emp".to_string(), "EMP".to_string(), "emp2".to_string()];
        data.rebuild_indices();

        let suggestions = data.get_suggestions("", false, None, true, false);
        let emp_count = suggestions
            .iter()
            .filter(|value| value.eq_ignore_ascii_case("EMP"))
            .count();
        assert_eq!(emp_count, 1);
        assert!(suggestions
            .iter()
            .any(|value| value.eq_ignore_ascii_case("EMP")));
    }

    #[test]
    fn virtual_table_columns_do_not_remove_real_table_columns() {
        let mut data = IntellisenseData::new();
        data.set_columns_for_table("EMP", vec!["REAL_COL".to_string()]);
        data.set_virtual_table_columns("EMP", vec!["VIRTUAL_COL".to_string()]);
        data.clear_virtual_tables();

        let columns = data.get_column_suggestions("", Some(&["EMP".to_string()]));
        assert!(
            columns.contains(&"REAL_COL".to_string()),
            "real table columns should remain cached after virtual cache clear"
        );
        assert!(
            !columns.contains(&"VIRTUAL_COL".to_string()),
            "virtual table columns should be cleared when clear_virtual_tables is called"
        );
    }

    #[test]
    fn virtual_table_columns_take_precedence_before_real_columns() {
        let mut data = IntellisenseData::new();
        data.set_columns_for_table("EMP", vec!["REAL_COL".to_string()]);
        data.set_virtual_table_columns("EMP", vec!["VIRTUAL_COL".to_string()]);

        let columns = data.get_column_suggestions("", Some(&["EMP".to_string()]));
        assert!(
            columns.contains(&"VIRTUAL_COL".to_string()),
            "virtual table columns should be used while virtual entries exist"
        );
        assert!(
            !columns.contains(&"REAL_COL".to_string()),
            "real table columns should not be included while virtual override exists"
        );
    }

    #[test]
    fn get_columns_for_table_uses_virtual_cache_when_available() {
        let mut data = IntellisenseData::new();
        data.set_columns_for_table("EMP", vec!["REAL_COL".to_string()]);
        data.set_virtual_table_columns("EMP", vec!["VIRTUAL_COL".to_string()]);

        let columns = data.get_columns_for_table("EMP");
        assert_eq!(columns, vec!["VIRTUAL_COL".to_string()]);
    }

    #[test]
    fn qualified_table_does_not_reuse_unqualified_column_cache_key() {
        let mut data = IntellisenseData::new();
        data.set_columns_for_table("EMP", vec!["EMPNO".to_string()]);

        let columns = data.get_columns_for_table("SCOTT.EMP");
        assert!(columns.is_empty());
    }

    #[test]
    fn get_columns_for_table_uses_default_qualifier_cache_key() {
        let mut data = IntellisenseData::new();
        data.set_default_qualifier(Some("SCOTT".to_string()));
        data.set_relation_members_for_qualifier("SCOTT", vec!["EMP".to_string()]);
        data.set_columns_for_table("SCOTT.EMP", vec!["EMPNO".to_string()]);

        let columns = data.get_columns_for_table("EMP");
        assert_eq!(columns, vec!["EMPNO".to_string()]);
    }

    #[test]
    fn get_columns_for_table_uses_default_qualifier_for_quoted_unqualified_name() {
        let mut data = IntellisenseData::new();
        data.set_default_qualifier(Some("SCOTT".to_string()));
        data.set_relation_members_for_qualifier("SCOTT", vec!["EMP".to_string()]);
        data.set_columns_for_table("SCOTT.EMP", vec!["EMPNO".to_string()]);

        let columns = data.get_columns_for_table(r#""EMP""#);

        assert_eq!(columns, vec!["EMPNO".to_string()]);
    }

    #[test]
    fn get_columns_for_table_uses_default_qualifier_for_bracket_unqualified_name() {
        let mut data = IntellisenseData::new();
        data.set_default_qualifier(Some("SCOTT".to_string()));
        data.set_relation_members_for_qualifier("SCOTT", vec!["ORDER DETAILS".to_string()]);
        data.set_columns_for_table("SCOTT.ORDER DETAILS", vec!["ORDER_ID".to_string()]);

        let columns = data.get_columns_for_table("[ORDER DETAILS]");

        assert_eq!(columns, vec!["ORDER_ID".to_string()]);
    }

    #[test]
    fn quoted_dotted_table_does_not_use_default_qualifier() {
        let mut data = IntellisenseData::new();
        data.set_default_qualifier(Some("SCOTT".to_string()));
        data.set_relation_members_for_qualifier("SCOTT", vec!["B".to_string()]);
        data.set_columns_for_table("SCOTT.B", vec!["LEAK".to_string()]);

        let columns = data.get_columns_for_table(r#""A.B""#);

        assert!(columns.is_empty(), "columns: {:?}", columns);
    }

    #[test]
    fn bracket_dotted_table_does_not_use_default_qualifier() {
        let mut data = IntellisenseData::new();
        data.set_default_qualifier(Some("SCOTT".to_string()));
        data.set_relation_members_for_qualifier("SCOTT", vec!["B".to_string()]);
        data.set_columns_for_table("SCOTT.B", vec!["LEAK".to_string()]);

        let columns = data.get_columns_for_table("[A.B]");

        assert!(columns.is_empty(), "columns: {:?}", columns);
    }

    #[test]
    fn get_columns_for_table_matches_exact_known_quoted_dotted_relation() {
        let mut data = IntellisenseData::new();
        data.set_columns_for_table("A.B", vec!["ID".to_string()]);

        let columns = data.get_columns_for_table(r#""A.B""#);

        assert_eq!(columns, vec!["ID".to_string()]);
    }

    #[test]
    fn get_columns_for_table_matches_exact_known_bracket_dotted_relation() {
        let mut data = IntellisenseData::new();
        data.set_columns_for_table("A.B", vec!["ID".to_string()]);

        let columns = data.get_columns_for_table("[A.B]");

        assert_eq!(columns, vec!["ID".to_string()]);
    }

    #[test]
    fn get_column_meta_matches_exact_known_quoted_dotted_relation() {
        let mut data = IntellisenseData::new();
        let mut meta = HashMap::new();
        meta.insert(
            "ID".to_string(),
            ColumnMeta {
                type_display: "NUMBER".to_string(),
                nullable: false,
                is_primary_key: true,
            },
        );
        data.set_column_meta_for_table("A.B", meta);

        let column_meta = data.get_column_meta(r#""A.B""#, "ID");

        assert_eq!(
            column_meta.map(|meta| meta.type_display.as_str()),
            Some("NUMBER")
        );
    }

    #[test]
    fn get_foreign_keys_matches_exact_known_quoted_dotted_relation() {
        let mut data = IntellisenseData::new();
        data.set_foreign_keys_for_table(
            "A.B",
            vec![ForeignKeyMeta {
                columns: vec!["PARENT_ID".to_string()],
                ref_table: "PARENT".to_string(),
                ref_columns: vec!["ID".to_string()],
            }],
        );

        let foreign_keys = data.get_foreign_keys(r#""A.B""#);

        assert_eq!(
            foreign_keys
                .and_then(|fks| fks.first())
                .map(|fk| fk.ref_table.as_str()),
            Some("PARENT")
        );
    }

    #[test]
    fn get_all_columns_for_highlighting_includes_virtual_columns() {
        let mut data = IntellisenseData::new();
        data.set_columns_for_table("EMP", vec!["REAL_COL".to_string()]);
        data.set_virtual_table_columns("VIRTUAL", vec!["VIRTUAL_COL".to_string()]);

        let columns = data.get_all_columns_for_highlighting();
        assert!(columns.contains(&"REAL_COL".to_string()));
        assert!(columns.contains(&"VIRTUAL_COL".to_string()));
    }

    #[test]
    fn qualified_column_scope_does_not_reuse_unqualified_table_cache_key() {
        let mut data = IntellisenseData::new();
        data.tables = vec!["HELP".to_string()];
        data.rebuild_indices();
        data.set_columns_for_table("HELP", vec!["TOPIC".to_string(), "TEXT".to_string()]);

        let scope = vec!["SCOTT.HELP".to_string()];
        let suggestions = data.get_column_suggestions("", Some(scope.as_slice()));

        assert!(suggestions.is_empty(), "suggestions: {suggestions:?}");
    }

    #[test]
    fn get_column_suggestions_match_quoted_columns_by_unquoted_prefix() {
        let mut data = IntellisenseData::new();
        data.set_virtual_table_columns(
            "REC",
            vec![r#""Employee Name""#.to_string(), "NORMAL_SAL".to_string()],
        );

        let scope = vec!["REC".to_string()];
        let suggestions = data.get_column_suggestions("Emp", Some(scope.as_slice()));

        assert_eq!(suggestions, vec![r#""Employee Name""#.to_string()]);
    }

    #[test]
    fn get_column_suggestions_match_quoted_columns_by_incomplete_quoted_prefix() {
        let mut data = IntellisenseData::new();
        data.set_virtual_table_columns(
            "REC",
            vec![r#""Employee Name""#.to_string(), "NORMAL_SAL".to_string()],
        );

        let scope = vec!["REC".to_string()];
        let suggestions = data.get_column_suggestions(r#""Emp"#, Some(scope.as_slice()));

        assert_eq!(suggestions, vec![r#""Employee Name""#.to_string()]);
    }

    #[test]
    fn get_column_suggestions_match_bracket_quoted_columns_by_unquoted_prefix() {
        let mut data = IntellisenseData::new();
        data.set_virtual_table_columns(
            "REC",
            vec![
                "[Item Id]".to_string(),
                "[order]".to_string(),
                "NORMAL_SAL".to_string(),
            ],
        );

        let scope = vec!["REC".to_string()];
        let suggestions = data.get_column_suggestions("Item", Some(scope.as_slice()));

        assert_eq!(suggestions, vec!["[Item Id]".to_string()]);
    }

    #[test]
    fn get_column_suggestions_match_incomplete_quoted_prefix_delimiter() {
        let mut data = IntellisenseData::new();
        data.set_virtual_table_columns(
            "REC",
            vec![
                r#""Emp Name""#.to_string(),
                "`Emp Name`".to_string(),
                "[Emp Name]".to_string(),
            ],
        );

        let scope = vec!["REC".to_string()];

        assert_eq!(
            data.get_column_suggestions(r#""Emp"#, Some(scope.as_slice())),
            vec![r#""Emp Name""#.to_string()]
        );
        assert_eq!(
            data.get_column_suggestions("`Emp", Some(scope.as_slice())),
            vec!["`Emp Name`".to_string()]
        );
        assert_eq!(
            data.get_column_suggestions("[Emp", Some(scope.as_slice())),
            vec!["[Emp Name]".to_string()]
        );
    }

    #[test]
    fn get_column_suggestions_applies_quoted_prefix_before_result_limit() {
        let mut data = IntellisenseData::new();
        let mut columns: Vec<String> = (0..60).map(|idx| format!("`Emp A{idx:03}`")).collect();
        columns.push(r#""Emp Target""#.to_string());
        data.set_virtual_table_columns("REC", columns);

        let scope = vec!["REC".to_string()];
        let suggestions = data.get_column_suggestions(r#""Emp"#, Some(scope.as_slice()));

        assert_eq!(suggestions, vec![r#""Emp Target""#.to_string()]);
    }

    #[test]
    fn get_relation_suggestions_include_synonyms() {
        let mut data = IntellisenseData::new();
        data.tables = vec!["EMP".to_string()];
        data.materialized_views = vec!["PAYROLL_MV".to_string()];
        data.synonyms = vec!["EMP_SYN".to_string()];
        data.public_synonyms = vec!["PUBLIC_EMP".to_string()];
        data.rebuild_indices();

        let suggestions = data.get_relation_suggestions("P");

        assert!(suggestions.iter().any(|name| name == "PAYROLL_MV"));
        assert!(suggestions.iter().any(|name| name == "PUBLIC_EMP"));
        assert!(!suggestions.iter().any(|name| name == "PACKAGE"));
    }

    #[test]
    fn get_relation_suggestions_include_users_for_schema_qualification() {
        let mut data = IntellisenseData::new();
        data.users = vec!["SCOTT".to_string(), "SYS".to_string()];
        data.rebuild_indices();

        let suggestions = data.get_relation_suggestions("SC");

        assert_eq!(suggestions, vec!["SCOTT".to_string()]);
    }

    #[test]
    fn get_object_suggestions_include_packages_sequences_and_synonyms() {
        let mut data = IntellisenseData::new();
        data.materialized_views = vec!["SALES_MV".to_string()];
        data.procedures = vec!["RUN_JOB".to_string()];
        data.packages = vec!["UTIL_PKG".to_string()];
        data.sequences = vec!["SEQ_ORDER".to_string()];
        data.synonyms = vec!["JOB_SYN".to_string()];
        data.rebuild_indices();

        let suggestions = data.get_object_suggestions("");

        assert!(suggestions.iter().any(|name| name == "SALES_MV"));
        assert!(suggestions.iter().any(|name| name == "RUN_JOB"));
        assert!(suggestions.iter().any(|name| name == "UTIL_PKG"));
        assert!(suggestions.iter().any(|name| name == "SEQ_ORDER"));
        assert!(suggestions.iter().any(|name| name == "JOB_SYN"));
    }

    #[test]
    fn get_object_suggestions_include_users_for_schema_qualification() {
        let mut data = IntellisenseData::new();
        data.users = vec!["SCOTT".to_string()];
        data.rebuild_indices();

        let suggestions = data.get_object_suggestions("SC");

        assert_eq!(suggestions, vec!["SCOTT".to_string()]);
    }

    #[test]
    fn get_object_suggestions_match_incomplete_quoted_prefix_delimiter() {
        let mut data = IntellisenseData::new();
        data.tables = vec![
            r#""Config Table""#.to_string(),
            "`Config Table`".to_string(),
            "[Config Table]".to_string(),
        ];
        data.rebuild_indices();

        assert_eq!(
            data.get_table_object_suggestions(r#""Config"#),
            vec![r#""Config Table""#.to_string()]
        );
        assert_eq!(
            data.get_table_object_suggestions("`Config"),
            vec!["`Config Table`".to_string()]
        );
        assert_eq!(
            data.get_table_object_suggestions("[Config"),
            vec!["[Config Table]".to_string()]
        );
    }

    #[test]
    fn get_object_suggestions_applies_quoted_prefix_before_result_limit() {
        let mut data = IntellisenseData::new();
        data.tables = (0..60).map(|idx| format!("`Config A{idx:03}`")).collect();
        data.tables.push(r#""Config Target""#.to_string());
        data.rebuild_indices();

        let suggestions = data.get_table_object_suggestions(r#""Config"#);

        assert_eq!(suggestions, vec![r#""Config Target""#.to_string()]);
    }

    #[test]
    fn get_suggestions_filters_language_items_for_quoted_prefix() {
        let mut data = IntellisenseData::new();
        data.tables = vec![
            r#""Commit Log""#.to_string(),
            "`Commit Log`".to_string(),
            "[Commit Log]".to_string(),
        ];
        data.rebuild_indices();

        let suggestions = data.get_suggestions(r#""Com"#, false, None, false, false);

        assert_eq!(suggestions, vec![r#""Commit Log""#.to_string()]);
        assert!(!suggestions.iter().any(|name| name == "COMMIT"));
        assert!(!suggestions.iter().any(|name| name == "COUNT()"));
    }

    #[test]
    fn get_object_suggestions_include_types_and_indexes() {
        let mut data = IntellisenseData::new();
        data.types = vec!["MONEY_T".to_string()];
        data.indexes = vec!["IDX_EMP_NAME".to_string()];
        data.triggers = vec!["EMP_AUDIT_TRG".to_string()];
        data.events = vec!["DAILY_PURGE_EVT".to_string()];
        data.rebuild_indices();

        let suggestions = data.get_object_suggestions("");

        assert!(
            suggestions.iter().any(|name| name == "MONEY_T"),
            "user types should appear in object suggestions"
        );
        assert!(
            suggestions.iter().any(|name| name == "IDX_EMP_NAME"),
            "user indexes should appear in object suggestions"
        );
        assert!(
            suggestions.iter().any(|name| name == "EMP_AUDIT_TRG"),
            "user triggers should appear in object suggestions"
        );
        assert!(
            suggestions.iter().any(|name| name == "DAILY_PURGE_EVT"),
            "user events should appear in object suggestions"
        );
    }

    #[test]
    fn get_object_suggestions_include_extended_oracle_schema_objects() {
        let mut data = IntellisenseData::new();
        data.database_links = vec!["APP_LINK".to_string()];
        data.directories = vec!["DATA_PUMP_DIR".to_string()];
        data.libraries = vec!["APP_LIB".to_string()];
        data.clusters = vec!["EMP_CLUSTER".to_string()];
        data.contexts = vec!["APP_CTX".to_string()];
        data.dimensions = vec!["SALES_DIM".to_string()];
        data.operators = vec!["EQ_OP".to_string()];
        data.indextypes = vec!["TEXT_ITYPE".to_string()];
        data.editions = vec!["V2_EDITION".to_string()];
        data.java_sources = vec!["Welcome".to_string()];
        data.java_classes = vec!["Agent".to_string()];
        data.java_resources = vec!["appText".to_string()];
        data.rebuild_indices();

        let suggestions = data.get_object_suggestions("");

        for expected in [
            "APP_LINK",
            "DATA_PUMP_DIR",
            "APP_LIB",
            "EMP_CLUSTER",
            "APP_CTX",
            "SALES_DIM",
            "EQ_OP",
            "TEXT_ITYPE",
            "V2_EDITION",
            "Welcome",
            "Agent",
            "appText",
        ] {
            assert!(
                suggestions.iter().any(|name| name == expected),
                "expected `{expected}` in object suggestions, got {suggestions:?}"
            );
        }
    }

    #[test]
    fn get_suggestions_for_db_general_flow_includes_all_object_types() {
        let mut data = IntellisenseData::new();
        data.tables = vec!["EMP_TBL".to_string()];
        data.views = vec!["EMP_VIEW".to_string()];
        data.materialized_views = vec!["EMP_MV".to_string()];
        data.functions = vec!["EMP_FUNC".to_string()];
        data.procedures = vec!["EMP_PROC".to_string()];
        data.packages = vec!["EMP_PKG".to_string()];
        data.sequences = vec!["EMP_SEQ".to_string()];
        data.synonyms = vec!["EMP_SYN".to_string()];
        data.public_synonyms = vec!["EMP_PUB_SYN".to_string()];
        data.types = vec!["EMP_TYP".to_string()];
        data.triggers = vec!["EMP_TRG".to_string()];
        data.events = vec!["EMP_EVT".to_string()];
        data.indexes = vec!["EMP_IDX".to_string()];
        data.database_links = vec!["EMP_DBLINK".to_string()];
        data.directories = vec!["EMP_DIR".to_string()];
        data.libraries = vec!["EMP_LIB".to_string()];
        data.java_sources = vec!["EMP_JAVA_SRC".to_string()];
        data.users = vec!["EMP_USR".to_string()];
        data.rebuild_indices();

        let suggestions = data.get_suggestions_for_db("EMP_", false, None, false, false, None);

        for expected in [
            "EMP_TBL",
            "EMP_VIEW",
            "EMP_MV",
            "EMP_FUNC",
            "EMP_PROC",
            "EMP_PKG",
            "EMP_SEQ",
            "EMP_SYN",
            "EMP_PUB_SYN",
            "EMP_TYP",
            "EMP_TRG",
            "EMP_EVT",
            "EMP_IDX",
            "EMP_DBLINK",
            "EMP_DIR",
            "EMP_LIB",
            "EMP_JAVA_SRC",
            "EMP_USR",
        ] {
            assert!(
                suggestions.iter().any(|name| name == expected),
                "expected `{expected}` in get_suggestions_for_db output, got {suggestions:?}"
            );
        }
    }

    #[test]
    fn expression_context_excludes_non_expression_object_kinds() {
        let mut data = IntellisenseData::new();
        // Expression-valid kinds (relations for qualification, callable/value
        // producers, qualifier namespaces).
        data.tables = vec!["EMP_TBL".to_string()];
        data.views = vec!["EMP_VIEW".to_string()];
        data.functions = vec!["EMP_FUNC".to_string()];
        data.packages = vec!["EMP_PKG".to_string()];
        data.sequences = vec!["EMP_SEQ".to_string()];
        data.types = vec!["EMP_TYP".to_string()];
        // Schema/DDL-only kinds that are never a token in a value expression.
        data.triggers = vec!["EMP_TRG".to_string()];
        data.events = vec!["EMP_EVT".to_string()];
        data.indexes = vec!["EMP_IDX".to_string()];
        data.database_links = vec!["EMP_DBLINK".to_string()];
        data.directories = vec!["EMP_DIR".to_string()];
        data.libraries = vec!["EMP_LIB".to_string()];
        data.clusters = vec!["EMP_CLUSTER".to_string()];
        data.contexts = vec!["EMP_CTX".to_string()];
        data.dimensions = vec!["EMP_DIM".to_string()];
        data.operators = vec!["EMP_OP".to_string()];
        data.indextypes = vec!["EMP_ITYPE".to_string()];
        data.editions = vec!["EMP_EDITION".to_string()];
        data.java_sources = vec!["EMP_JAVA_SRC".to_string()];
        data.users = vec!["EMP_USR".to_string()];
        data.rebuild_indices();

        // `prefer_columns = true` is the column/expression context.
        let suggestions = data.get_suggestions_for_db(
            "EMP_",
            true,
            None,
            false,
            true,
            Some(crate::db::DatabaseType::Oracle),
        );

        for expected in ["EMP_FUNC", "EMP_PKG", "EMP_SEQ", "EMP_TYP"] {
            assert!(
                suggestions.iter().any(|name| name == expected),
                "expression context should still offer `{expected}`, got {suggestions:?}"
            );
        }

        for excluded in [
            "EMP_TBL",
            "EMP_VIEW",
            "EMP_TRG",
            "EMP_EVT",
            "EMP_IDX",
            "EMP_DBLINK",
            "EMP_DIR",
            "EMP_LIB",
            "EMP_CLUSTER",
            "EMP_CTX",
            "EMP_DIM",
            "EMP_OP",
            "EMP_ITYPE",
            "EMP_EDITION",
            "EMP_JAVA_SRC",
            "EMP_USR",
        ] {
            assert!(
                !suggestions.iter().any(|name| name == excluded),
                "expression context must not offer schema-only `{excluded}`, got {suggestions:?}"
            );
        }

        // General context (`prefer_columns = false`) still surfaces every kind.
        let general = data.get_suggestions_for_db(
            "EMP_",
            false,
            None,
            false,
            false,
            Some(crate::db::DatabaseType::Oracle),
        );
        for expected in ["EMP_TRG", "EMP_IDX", "EMP_DBLINK", "EMP_USR"] {
            assert!(
                general.iter().any(|name| name == expected),
                "general context should still offer `{expected}`, got {general:?}"
            );
        }
    }

    #[test]
    fn mysql_general_catalog_excludes_oracle_schema_type_objects() {
        let mut data = IntellisenseData::new();
        data.types = vec!["ADDR_TYPE".to_string()];
        data.functions = vec!["ADDR_FUNC".to_string()];
        data.rebuild_indices();

        let oracle = Some(crate::db::DatabaseType::Oracle);
        let oracle_general = data.get_suggestions_for_db("ADDR", false, None, false, false, oracle);
        assert!(
            oracle_general.iter().any(|name| name == "ADDR_TYPE"),
            "Oracle general catalog should keep schema TYPE objects: {oracle_general:?}"
        );

        for db_type in [
            crate::db::DatabaseType::MySQL,
            crate::db::DatabaseType::MariaDB,
        ] {
            let db = Some(db_type);
            for (prefer_columns, label) in [(false, "general"), (true, "expression")] {
                let suggestions =
                    data.get_suggestions_for_db("ADDR", false, None, false, prefer_columns, db);
                assert!(
                    !suggestions.iter().any(|name| name == "ADDR_TYPE"),
                    "{db_type:?} {label} catalog must not offer Oracle schema TYPE objects: {suggestions:?}"
                );
                assert!(
                    suggestions.iter().any(|name| name == "ADDR_FUNC"),
                    "{db_type:?} {label} catalog should keep ordinary functions: {suggestions:?}"
                );
            }
        }
    }

    #[test]
    fn mysql_general_catalog_excludes_oracle_sequence_objects() {
        let mut data = IntellisenseData::new();
        data.sequences = vec!["EMP_SEQ".to_string()];
        data.functions = vec!["EMP_FUNC".to_string()];
        data.rebuild_indices();

        let oracle = Some(crate::db::DatabaseType::Oracle);
        let oracle_general = data.get_suggestions_for_db("EMP", false, None, false, false, oracle);
        assert!(
            oracle_general.iter().any(|name| name == "EMP_SEQ"),
            "Oracle general catalog should keep sequence objects: {oracle_general:?}"
        );

        for db_type in [
            crate::db::DatabaseType::MySQL,
            crate::db::DatabaseType::MariaDB,
        ] {
            let db = Some(db_type);
            for (prefer_columns, label) in [(false, "general"), (true, "expression")] {
                let suggestions =
                    data.get_suggestions_for_db("EMP", false, None, false, prefer_columns, db);
                assert!(
                    !suggestions.iter().any(|name| name == "EMP_SEQ"),
                    "{db_type:?} {label} catalog must not offer Oracle sequence objects: {suggestions:?}"
                );
                assert!(
                    suggestions.iter().any(|name| name == "EMP_FUNC"),
                    "{db_type:?} {label} catalog should keep ordinary functions: {suggestions:?}"
                );
            }
        }
    }

    #[test]
    fn expression_context_excludes_mysql_command_keywords() {
        let mut data = IntellisenseData::new();
        data.rebuild_indices();
        let mysql = Some(crate::db::DatabaseType::MySQL);

        // (prefix, forbidden command keyword(s), expression-valid sibling(s))
        let cases: &[(&str, &[&str], &[&str])] = &[
            ("fl", &["FLUSH"], &["FLOAT"]),
            ("ki", &["KILL"], &[]),
            ("opt", &["OPTIMIZE"], &[]),
            ("prep", &["PREPARE"], &[]),
            ("deal", &["DEALLOCATE"], &[]),
            ("hand", &["HANDLER"], &[]),
            ("loa", &["LOAD"], &[]),
            // REPLACE/REPEAT are functions and must survive; REPAIR must drop.
            ("rep", &["REPAIR"], &["REPLACE", "REPEAT"]),
            ("cha", &["CHANGE"], &["CHAR", "CHARACTER"]),
            // MySQL DDL table-option / routine fragments drop; the expression
            // and clause lookalikes survive.
            ("auto_", &["AUTO_INCREMENT"], &[]),
            ("engine", &["ENGINE", "ENGINES"], &[]),
            ("spat", &["SPATIAL"], &[]),
            ("algor", &["ALGORITHM"], &[]),
            ("zero", &["ZEROFILL"], &[]),
            ("max_r", &["MAX_ROWS"], &[]),
            ("invok", &["INVOKER"], &[]),
            ("def", &[], &["DEFAULT"]),
            ("coll", &[], &["COLLATE"]),
            ("unsig", &[], &["UNSIGNED"]),
            ("part", &["PARTITIONS"], &["PARTITION"]),
            ("ign", &[], &["IGNORE"]),
            ("coal", &[], &["COALESCE()"]),
        ];

        for (prefix, forbidden, expected) in cases {
            let column = data.get_suggestions_for_db(prefix, false, None, false, true, mysql);
            for word in *forbidden {
                assert!(
                    !column.iter().any(|s| s == word),
                    "mysql expression context must drop `{word}` for `{prefix}`: {column:?}"
                );
            }
            for word in *expected {
                assert!(
                    column.iter().any(|s| s == word),
                    "mysql expression context must keep `{word}` for `{prefix}`: {column:?}"
                );
            }

            let general = data.get_suggestions_for_db(prefix, false, None, false, false, mysql);
            for word in *forbidden {
                assert!(
                    general.iter().any(|s| s == word),
                    "mysql general context should keep `{word}` for `{prefix}`: {general:?}"
                );
            }
        }
    }

    #[test]
    fn expression_context_excludes_standalone_procedures() {
        let mut data = IntellisenseData::new();
        data.tables = vec!["EMP".to_string()];
        data.functions = vec!["EMP_FN".to_string()];
        data.procedures = vec!["EMP_PROC".to_string()];
        data.packages = vec!["EMP_PKG".to_string()];
        data.sequences = vec!["EMP_SEQ".to_string()];
        data.rebuild_indices();
        let oracle = Some(crate::db::DatabaseType::Oracle);

        // Expression/column context (`prefer_columns = true`).
        let column = data.get_suggestions_for_db("EMP", true, None, false, true, oracle);
        assert!(
            !column.iter().any(|s| s == "EMP_PROC"),
            "expression context must not offer a standalone procedure: {column:?}"
        );
        for kept in ["EMP_FN", "EMP_PKG", "EMP_SEQ"] {
            assert!(
                column.iter().any(|s| s == kept),
                "expression context should still offer `{kept}`: {column:?}"
            );
        }

        // General context still offers the procedure (e.g. a `CALL`/`EXEC` head).
        let general = data.get_suggestions_for_db("EMP", false, None, false, false, oracle);
        assert!(
            general.iter().any(|s| s == "EMP_PROC"),
            "general context should still offer the procedure: {general:?}"
        );
    }

    #[test]
    fn expression_context_excludes_statement_and_command_keywords() {
        let mut data = IntellisenseData::new();
        data.rebuild_indices();
        let oracle = Some(crate::db::DatabaseType::Oracle);

        // Each prefix matches at least one statement/command-only keyword that
        // must be dropped in an expression context, plus an expression-valid
        // keyword (or operator) that must survive.
        let cases: &[(&str, &[&str], &[&str])] = &[
            ("cr", &["CREATE"], &["CROSS"]),
            ("dr", &["DROP"], &[]),
            ("gr", &["GRANT"], &["GROUP", "GROUPS"]),
            ("tru", &["TRUNCATE"], &[]),
            ("al", &["ALTER"], &["ALL"]),
            ("ins", &["INSERT"], &["INSERTING"]),
            (
                "co",
                &["COMMENT", "COMMIT", "COMPUTE"],
                &["COALESCE", "COLLATE"],
            ),
            ("de", &["DELETE", "DESCRIBE"], &["DESC"]),
            // DDL-only clause modifiers must drop; their expression-valid
            // siblings (CASE/CAST, CONSTRUCTOR) and the query/analytic-role
            // traps (PARTITION, USING, NOCYCLE) must survive.
            ("cas", &["CASCADE"], &["CASE", "CAST"]),
            ("constr", &["CONSTRAINT"], &["CONSTRUCTOR"]),
            ("stor", &["STORAGE"], &[]),
            ("tablesp", &["TABLESPACE"], &[]),
            ("ident", &["IDENTIFIED"], &["IDENTITY"]),
            ("part", &[], &["PARTITION"]),
            ("us", &[], &["USING"]),
            ("nocy", &[], &["NOCYCLE"]),
            // Session/compiler parameters and routine-declaration properties
            // drop; the NLS *functions* and SESSIONTIMEZONE keep completing.
            ("nls_d", &["NLS_DATE_FORMAT"], &[]),
            ("nlss", &[], &["NLSSORT()"]),
            ("plsql_", &["PLSQL_WARNINGS"], &[]),
            ("determ", &["DETERMINISTIC"], &[]),
            ("pipel", &["PIPELINED"], &[]),
            ("authi", &["AUTHID"], &[]),
            ("session", &[], &["SESSIONTIMEZONE"]),
            // SQL*Plus SET options drop, but the expression-token collisions
            // (NULL/ESCAPE/LONG) must survive.
            ("serv", &["SERVEROUTPUT"], &[]),
            ("feed", &["FEEDBACK"], &[]),
            ("nul", &[], &["NULL"]),
            ("esc", &[], &["ESCAPE"]),
            ("lon", &[], &["LONG"]),
            // Long-tail DDL/object/declaration keywords drop; their expression/
            // clause-valid lookalikes survive.
            ("compi", &["COMPILE"], &[]),
            ("exter", &["EXTERNAL"], &[]),
            ("langu", &["LANGUAGE"], &[]),
            ("unlim", &["UNLIMITED"], &[]),
            ("bod", &["BODY"], &[]),
            ("doc", &[], &["DOCUMENT"]), // x IS DOCUMENT
            ("bul", &[], &["BULK"]),     // SELECT ... BULK COLLECT INTO
            ("per", &[], &["PERIOD"]),   // PERIOD FOR / temporal
            ("appl", &[], &["APPLY"]),   // CROSS/OUTER APPLY
        ];

        for (prefix, forbidden, expected) in cases {
            // prefer_columns = true → expression/column context.
            let column = data.get_suggestions_for_db(prefix, false, None, false, true, oracle);
            for word in *forbidden {
                assert!(
                    !column.iter().any(|s| s == word),
                    "expression context must drop `{word}` for prefix `{prefix}`: {column:?}"
                );
            }
            for word in *expected {
                assert!(
                    column.iter().any(|s| s == word),
                    "expression context must keep `{word}` for prefix `{prefix}`: {column:?}"
                );
            }

            // General context (prefer_columns = false) keeps every keyword.
            let general = data.get_suggestions_for_db(prefix, false, None, false, false, oracle);
            for word in *forbidden {
                assert!(
                    general.iter().any(|s| s == word),
                    "general context should keep `{word}` for prefix `{prefix}`: {general:?}"
                );
            }
        }
    }

    #[test]
    fn get_type_object_suggestions_returns_user_types() {
        let mut data = IntellisenseData::new();
        data.types = vec!["MONEY_T".to_string(), "ADDRESS_T".to_string()];
        data.rebuild_indices();

        let suggestions = data.get_type_object_suggestions("M");
        assert_eq!(suggestions, vec!["MONEY_T".to_string()]);
    }

    #[test]
    fn get_trigger_object_suggestions_returns_user_triggers() {
        let mut data = IntellisenseData::new();
        data.triggers = vec!["EMP_AUDIT_TRG".to_string(), "ORD_TRG".to_string()];
        data.rebuild_indices();

        let suggestions = data.get_trigger_object_suggestions("EMP");
        assert_eq!(suggestions, vec!["EMP_AUDIT_TRG".to_string()]);
    }

    #[test]
    fn get_event_object_suggestions_returns_user_events() {
        let mut data = IntellisenseData::new();
        data.events = vec![
            "DAILY_PURGE_EVT".to_string(),
            "WEEKLY_REPORT_EVT".to_string(),
        ];
        data.rebuild_indices();

        let suggestions = data.get_event_object_suggestions("DAILY");
        assert_eq!(suggestions, vec!["DAILY_PURGE_EVT".to_string()]);
    }

    #[test]
    fn get_index_object_suggestions_returns_user_indexes() {
        let mut data = IntellisenseData::new();
        data.indexes = vec!["IDX_EMP_NAME".to_string(), "IDX_DEPT_LOC".to_string()];
        data.rebuild_indices();

        let suggestions = data.get_index_object_suggestions("IDX_EMP");
        assert_eq!(suggestions, vec!["IDX_EMP_NAME".to_string()]);
    }

    #[test]
    fn get_public_synonym_object_suggestions_returns_user_public_synonyms() {
        let mut data = IntellisenseData::new();
        data.public_synonyms = vec!["DUAL_PUB".to_string(), "ALL_TABLES".to_string()];
        data.rebuild_indices();

        let suggestions = data.get_public_synonym_object_suggestions("DUAL");
        assert_eq!(suggestions, vec!["DUAL_PUB".to_string()]);
    }

    #[test]
    fn get_member_suggestions_use_package_and_schema_qualifiers() {
        let mut data = IntellisenseData::new();
        data.set_members_for_qualifier(
            "DEMO_PKG",
            vec!["RUN_JOB".to_string(), "CALC_BONUS".to_string()],
        );
        data.set_members_for_qualifier(
            "SCOTT",
            vec![
                "EMP".to_string(),
                "EMP_API".to_string(),
                "SEQ_EMP".to_string(),
            ],
        );
        data.set_relation_members_for_qualifier(
            "SCOTT",
            vec!["EMP".to_string(), "EMP_VIEW".to_string()],
        );

        let package_members = data.get_member_suggestions("demo_pkg", "R", false);
        let schema_members = data.get_member_suggestions("scott", "EMP", false);
        let schema_relations = data.get_member_suggestions("scott", "EMP", true);

        assert_eq!(package_members, vec!["RUN_JOB".to_string()]);
        assert!(schema_members.iter().any(|name| name == "EMP_API"));
        assert!(schema_relations.iter().any(|name| name == "EMP_VIEW"));
        assert!(!schema_relations.iter().any(|name| name == "EMP_API"));
    }

    #[test]
    fn relation_member_suggestions_require_a_scoped_relation_cache() {
        let mut data = IntellisenseData::new();
        data.set_members_for_qualifier("SCOTT", vec!["EMP".to_string(), "RUN_JOB".to_string()]);

        assert!(data.get_member_suggestions("SCOTT", "", true).is_empty());
    }

    #[test]
    fn expression_suggestions_do_not_include_global_relations() {
        let mut data = IntellisenseData::new();
        data.tables = vec!["EMP".to_string(), "HELP".to_string()];
        data.set_columns_for_table("HELP", vec!["TOPIC".to_string()]);
        data.rebuild_indices();

        let scope = ["HELP".to_string()];
        let suggestions = data.get_suggestions_for_db("emp", true, Some(&scope), false, true, None);

        for leaked in ["EMP", "HELP", "TOPIC"] {
            assert!(
                !suggestions.iter().any(|item| item == leaked),
                "global relation/column `{leaked}` leaked into HELP scope: {suggestions:?}"
            );
        }
    }

    #[test]
    fn get_member_suggestions_match_quoted_schema_qualifier() {
        let mut data = IntellisenseData::new();
        data.set_members_for_qualifier("SCOTT", vec!["EMP".to_string(), "EMP_API".to_string()]);
        data.set_relation_members_for_qualifier(
            "SCOTT",
            vec!["EMP".to_string(), "EMP_VIEW".to_string()],
        );

        let schema_members = data.get_member_suggestions(r#""SCOTT""#, "EMP", false);
        let schema_relations = data.get_member_suggestions(r#""SCOTT""#, "EMP", true);
        let backtick_members = data.get_member_suggestions("`SCOTT`", "EMP", false);
        let backtick_relations = data.get_member_suggestions("`SCOTT`", "EMP", true);

        assert!(schema_members.iter().any(|name| name == "EMP_API"));
        assert!(schema_relations.iter().any(|name| name == "EMP_VIEW"));
        assert!(!schema_relations.iter().any(|name| name == "EMP_API"));
        assert!(backtick_members.iter().any(|name| name == "EMP_API"));
        assert!(backtick_relations.iter().any(|name| name == "EMP_VIEW"));
        assert!(!backtick_relations.iter().any(|name| name == "EMP_API"));
    }

    #[test]
    fn get_member_suggestions_match_quoted_member_prefix() {
        let mut data = IntellisenseData::new();
        data.set_members_for_qualifier(
            "DEMO_PKG",
            vec![
                r#""Run Job""#.to_string(),
                "`Run Batch`".to_string(),
                "[Run]]Report]".to_string(),
                "RESET_JOB".to_string(),
            ],
        );
        data.set_relation_members_for_qualifier(
            "SCOTT",
            vec![
                r#""Order Header""#.to_string(),
                "`Order Items`".to_string(),
                "[Order]]Audit]".to_string(),
                "EMP".to_string(),
            ],
        );

        assert_eq!(
            data.get_member_suggestions("DEMO_PKG", r#""Ru"#, false),
            vec![r#""Run Job""#.to_string()]
        );
        assert_eq!(
            data.get_member_suggestions("DEMO_PKG", "`Ru", false),
            vec!["`Run Batch`".to_string()]
        );
        assert_eq!(
            data.get_member_suggestions("DEMO_PKG", "Run]", false),
            vec!["[Run]]Report]".to_string()]
        );
        assert_eq!(
            data.get_member_suggestions("SCOTT", "Order]", true),
            vec!["[Order]]Audit]".to_string()]
        );
    }

    #[test]
    fn get_member_suggestions_applies_quoted_prefix_before_result_limit() {
        let mut members = (0..MAX_SUGGESTIONS + 10)
            .map(|idx| format!("RUN_{idx:03}"))
            .collect::<Vec<_>>();
        members.push(r#""Run Target""#.to_string());

        let mut data = IntellisenseData::new();
        data.set_members_for_qualifier("DEMO_PKG", members);

        let suggestions = data.get_member_suggestions("DEMO_PKG", r#""Run"#, false);

        assert_eq!(suggestions, vec![r#""Run Target""#.to_string()]);
    }

    #[test]
    fn quoted_dotted_qualifier_requires_an_exact_member_cache() {
        let mut data = IntellisenseData::new();
        data.set_members_for_qualifier("B", vec!["LEAK".to_string()]);
        data.set_relation_members_for_qualifier("B", vec!["LEAK_TABLE".to_string()]);

        let members = data.get_member_suggestions(r#""A.B""#, "LEAK", false);
        let relations = data.get_member_suggestions(r#""A.B""#, "LEAK", true);

        assert!(members.is_empty(), "members: {:?}", members);
        assert!(relations.is_empty(), "relations: {:?}", relations);
    }

    #[test]
    fn get_member_suggestions_match_exact_quoted_dotted_qualifier() {
        let mut data = IntellisenseData::new();
        data.set_members_for_qualifier("A.B", vec!["RUN_JOB".to_string()]);
        data.set_relation_members_for_qualifier("A.B", vec!["EMP".to_string()]);

        let quoted_members = data.get_member_suggestions(r#""A.B""#, "RUN", false);
        let bracket_members = data.get_member_suggestions("[A.B]", "RUN", false);
        let quoted_relations = data.get_member_suggestions(r#""A.B""#, "EMP", true);
        let bracket_relations = data.get_member_suggestions("[A.B]", "EMP", true);

        assert_eq!(quoted_members, vec!["RUN_JOB".to_string()]);
        assert_eq!(bracket_members, vec!["RUN_JOB".to_string()]);
        assert_eq!(quoted_relations, vec!["EMP".to_string()]);
        assert_eq!(bracket_relations, vec!["EMP".to_string()]);
    }

    #[test]
    fn qualifier_member_kind_lookup_matches_exact_quoted_dotted_qualifier() {
        let mut data = IntellisenseData::new();
        data.set_members_for_qualifier_with_kinds(
            "A.B",
            vec![("CALC".to_string(), Some(QualifiedMemberKind::Function))],
        );

        assert_eq!(
            data.qualifier_member_name_matching_kinds(
                r#""A.B""#,
                "calc",
                &[QualifiedMemberKind::Function],
            )
            .as_deref(),
            Some("CALC")
        );
        assert_eq!(
            data.qualifier_member_name_matching_kinds(
                "[A.B]",
                "calc",
                &[QualifiedMemberKind::Function],
            )
            .as_deref(),
            Some("CALC")
        );
    }

    #[test]
    fn member_suggestions_require_the_exact_dotted_qualifier() {
        let mut data = IntellisenseData::new();
        data.set_members_for_qualifier(
            "ORDER.HEADER",
            vec!["LINE_ID".to_string(), "STATUS".to_string()],
        );

        let suggestions = data.get_member_suggestions("sales.Order.Header", "LINE", false);

        assert!(suggestions.is_empty());
    }

    #[test]
    fn relation_member_suggestions_require_the_exact_dotted_qualifier() {
        let mut data = IntellisenseData::new();
        data.set_relation_members_for_qualifier(
            "ORDER.HEADER",
            vec!["LINE_ITEMS".to_string(), "STATUS_LOG".to_string()],
        );

        let suggestions = data.get_member_suggestions("sales.Order.Header", "LINE", true);

        assert!(suggestions.is_empty());
    }

    #[test]
    fn qualifier_member_kind_lookup_matches_quoted_member_display_names() {
        let mut data = IntellisenseData::new();
        data.set_members_for_qualifier_with_kinds(
            "SCOTT",
            vec![(
                r#""Order Header""#.to_string(),
                Some(QualifiedMemberKind::Table),
            )],
        );

        assert_eq!(
            data.qualifier_member_matches_kinds(
                "scott",
                r#""Order Header""#,
                &[QualifiedMemberKind::Table]
            ),
            Some(true)
        );
        assert_eq!(
            data.qualifier_member_matches_kinds(
                "scott",
                "Order Header",
                &[QualifiedMemberKind::Table]
            ),
            Some(true)
        );
    }

    #[test]
    fn object_name_kind_lookup_supports_explicit_and_default_qualifiers() {
        let mut data = IntellisenseData::new();
        data.set_members_for_qualifier_with_kinds(
            "APP",
            vec![
                (
                    "ORDERS_SEQ".to_string(),
                    Some(QualifiedMemberKind::Sequence),
                ),
                ("ORDERS".to_string(), Some(QualifiedMemberKind::Table)),
            ],
        );
        data.set_default_qualifier(Some("APP".to_string()));

        assert!(data.object_name_matches_qualifier_member_kind(
            "ORDERS_SEQ",
            QualifiedMemberKind::Sequence
        ));
        assert!(data.object_name_matches_qualifier_member_kind(
            "APP.ORDERS_SEQ",
            QualifiedMemberKind::Sequence
        ));
        assert!(!data.object_name_matches_qualifier_member_kind(
            "APP.ORDERS",
            QualifiedMemberKind::Sequence
        ));
    }

    #[test]
    fn activate_default_qualifier_keeps_quoted_schema_members_by_kind() {
        let mut data = IntellisenseData::new();
        data.set_members_for_qualifier_with_kinds(
            "SCOTT",
            vec![
                (
                    r#""Order Header""#.to_string(),
                    Some(QualifiedMemberKind::Table),
                ),
                (
                    r#""Run Job""#.to_string(),
                    Some(QualifiedMemberKind::Procedure),
                ),
            ],
        );

        assert!(data.activate_default_qualifier("scott"));

        assert_eq!(data.tables, vec![r#""Order Header""#.to_string()]);
        assert_eq!(data.procedures, vec![r#""Run Job""#.to_string()]);
    }

    #[test]
    fn activate_default_qualifier_rebuilds_active_object_lists_from_schema_members() {
        let mut data = IntellisenseData::new();
        data.set_members_for_qualifier_with_kinds(
            "SCOTT",
            vec![
                ("EMP".to_string(), Some(QualifiedMemberKind::Table)),
                ("EMP_VIEW".to_string(), Some(QualifiedMemberKind::View)),
                (
                    "EMP_MV".to_string(),
                    Some(QualifiedMemberKind::MaterializedView),
                ),
                ("EMP_TYPE".to_string(), Some(QualifiedMemberKind::Type)),
                ("EMP_TRG".to_string(), Some(QualifiedMemberKind::Trigger)),
                ("EMP_EVENT".to_string(), Some(QualifiedMemberKind::Event)),
                ("EMP_IDX".to_string(), Some(QualifiedMemberKind::Index)),
                ("EMP_PROC".to_string(), Some(QualifiedMemberKind::Procedure)),
                ("EMP_FUNC".to_string(), Some(QualifiedMemberKind::Function)),
                ("EMP_SEQ".to_string(), Some(QualifiedMemberKind::Sequence)),
                ("EMP_PKG".to_string(), Some(QualifiedMemberKind::Package)),
                ("EMP_SYN".to_string(), Some(QualifiedMemberKind::Synonym)),
                (
                    "EMP_LINK".to_string(),
                    Some(QualifiedMemberKind::DatabaseLink),
                ),
                ("EMP_DIR".to_string(), Some(QualifiedMemberKind::Directory)),
                ("EMP_LIB".to_string(), Some(QualifiedMemberKind::Library)),
                (
                    "EMP_CLUSTER".to_string(),
                    Some(QualifiedMemberKind::Cluster),
                ),
                ("EMP_CTX".to_string(), Some(QualifiedMemberKind::Context)),
                ("EMP_DIM".to_string(), Some(QualifiedMemberKind::Dimension)),
                ("EMP_OP".to_string(), Some(QualifiedMemberKind::Operator)),
                (
                    "EMP_ITYPE".to_string(),
                    Some(QualifiedMemberKind::Indextype),
                ),
                (
                    "EMP_EDITION".to_string(),
                    Some(QualifiedMemberKind::Edition),
                ),
                (
                    "EMP_JAVA_SRC".to_string(),
                    Some(QualifiedMemberKind::JavaSource),
                ),
                (
                    "EMP_JAVA_CLASS".to_string(),
                    Some(QualifiedMemberKind::JavaClass),
                ),
                (
                    "EMP_JAVA_RES".to_string(),
                    Some(QualifiedMemberKind::JavaResource),
                ),
                (
                    "EMP_PUB_SYN".to_string(),
                    Some(QualifiedMemberKind::PublicSynonym),
                ),
                ("EMP_USER".to_string(), Some(QualifiedMemberKind::User)),
            ],
        );
        data.set_members_for_qualifier_with_kinds(
            "HR",
            vec![("JOB".to_string(), Some(QualifiedMemberKind::Table))],
        );
        data.set_default_qualifier(Some("HR".to_string()));
        data.tables = vec!["JOB".to_string()];
        data.rebuild_indices();

        assert!(data.activate_default_qualifier("scott"));

        assert_eq!(data.default_qualifier(), Some("SCOTT"));
        assert_eq!(data.tables, vec!["EMP".to_string()]);
        assert_eq!(data.views, vec!["EMP_VIEW".to_string()]);
        assert_eq!(data.materialized_views, vec!["EMP_MV".to_string()]);
        assert_eq!(data.types, vec!["EMP_TYPE".to_string()]);
        assert_eq!(data.triggers, vec!["EMP_TRG".to_string()]);
        assert_eq!(data.events, vec!["EMP_EVENT".to_string()]);
        assert_eq!(data.indexes, vec!["EMP_IDX".to_string()]);
        assert_eq!(data.procedures, vec!["EMP_PROC".to_string()]);
        assert_eq!(data.functions, vec!["EMP_FUNC".to_string()]);
        assert_eq!(data.sequences, vec!["EMP_SEQ".to_string()]);
        assert_eq!(data.packages, vec!["EMP_PKG".to_string()]);
        assert_eq!(data.synonyms, vec!["EMP_SYN".to_string()]);
        assert_eq!(data.database_links, vec!["EMP_LINK".to_string()]);
        assert_eq!(data.directories, vec!["EMP_DIR".to_string()]);
        assert_eq!(data.libraries, vec!["EMP_LIB".to_string()]);
        assert_eq!(data.clusters, vec!["EMP_CLUSTER".to_string()]);
        assert_eq!(data.contexts, vec!["EMP_CTX".to_string()]);
        assert_eq!(data.dimensions, vec!["EMP_DIM".to_string()]);
        assert_eq!(data.operators, vec!["EMP_OP".to_string()]);
        assert_eq!(data.indextypes, vec!["EMP_ITYPE".to_string()]);
        assert_eq!(data.editions, vec!["EMP_EDITION".to_string()]);
        assert_eq!(data.java_sources, vec!["EMP_JAVA_SRC".to_string()]);
        assert_eq!(data.java_classes, vec!["EMP_JAVA_CLASS".to_string()]);
        assert_eq!(data.java_resources, vec!["EMP_JAVA_RES".to_string()]);
        assert!(data.public_synonyms.is_empty());
        assert!(data.users.is_empty());
        assert_eq!(
            data.get_table_object_suggestions("E"),
            vec!["EMP".to_string()]
        );
        assert_eq!(
            data.suggestion_type_label("EMP_LINK", Some(crate::db::DatabaseType::Oracle)),
            Some("DATABASE LINK")
        );
    }

    #[test]
    fn invoke_selected_callback_preserves_replaced_callback() {
        let callback_slot: Arc<Mutex<Option<Box<dyn FnMut(String)>>>> = Arc::new(Mutex::new(None));
        let calls = Arc::new(Mutex::new(Vec::new()));

        let callback_slot_for_first = callback_slot.clone();
        let calls_for_first = calls.clone();
        *callback_slot
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) =
            Some(Box::new(move |value: String| {
                calls_for_first
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner())
                    .push(format!("first:{value}"));
                let calls_for_second = calls_for_first.clone();
                *callback_slot_for_first
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner()) =
                    Some(Box::new(move |next: String| {
                        calls_for_second
                            .lock()
                            .unwrap_or_else(|poisoned| poisoned.into_inner())
                            .push(format!("second:{next}"));
                    }));
            }));

        IntellisensePopup::invoke_selected_callback(&callback_slot, "alpha".to_string());
        IntellisensePopup::invoke_selected_callback(&callback_slot, "beta".to_string());

        assert_eq!(
            calls
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .as_slice(),
            ["first:alpha".to_string(), "second:beta".to_string()]
        );
    }

    #[test]
    fn invoke_selected_callback_restores_original_after_panic() {
        let callback_slot: Arc<Mutex<Option<Box<dyn FnMut(String)>>>> = Arc::new(Mutex::new(None));
        let calls = Arc::new(Mutex::new(Vec::new()));

        let calls_for_cb = calls.clone();
        *callback_slot
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) =
            Some(Box::new(move |value: String| {
                calls_for_cb
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner())
                    .push(value.clone());
                if value == "panic" {
                    panic!("expected test panic");
                }
            }));

        IntellisensePopup::invoke_selected_callback(&callback_slot, "panic".to_string());
        IntellisensePopup::invoke_selected_callback(&callback_slot, "ok".to_string());

        assert_eq!(
            calls
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .as_slice(),
            ["panic".to_string(), "ok".to_string()]
        );
    }
}

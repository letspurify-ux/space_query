//! Guards the DB-backend dispatch convention: `DatabaseBackendKind` and
//! `SqlDialect` decisions in non-test source must use exhaustive `match`
//! expressions, never `==`/`!=` comparisons, `matches!`, `if let`, or wildcard
//! match arms. Equality-style checks compile silently when a new backend kind
//! or dialect is added and fall through to the wrong dialect family;
//! exhaustive matches turn every decision site into a compile error that
//! forces a per-site review. Non-UI source must dispatch concrete
//! `DatabaseType` with wildcard-free exhaustive matches; UI code must avoid
//! direct `DatabaseType` branching entirely and flow through backend
//! specs/registries so new database types cannot silently inherit an
//! unrelated branch.

use std::ffi::OsStr;
use std::fs;
use std::path::{Path, PathBuf};

use proc_macro2::TokenTree;
use quote::ToTokens;
use syn::spanned::Spanned;
use syn::visit::{self, Visit};
use syn::{
    BinOp, Expr, ExprBinary, ExprCall, ExprLet, ExprMatch, ExprMethodCall, ImplItem, Item, ItemFn,
    ItemMod, LitStr, Macro, Pat, TraitItem, Type,
};

#[allow(dead_code)]
#[derive(Debug)]
struct BackendKindDispatchOffender {
    path: String,
    line: usize,
    pattern: String,
}

fn collect_rust_files(root: &Path) -> Vec<PathBuf> {
    let mut files = Vec::new();
    let mut stack = vec![root.to_path_buf()];

    while let Some(dir) = stack.pop() {
        let entries = match fs::read_dir(&dir) {
            Ok(entries) => entries,
            Err(err) => panic!("failed to read directory {}: {err}", dir.display()),
        };

        for entry in entries {
            let entry = match entry {
                Ok(entry) => entry,
                Err(err) => panic!("failed to read directory entry in {}: {err}", dir.display()),
            };
            let path = entry.path();

            if path.is_dir() {
                stack.push(path);
                continue;
            }

            if path.extension().and_then(|ext| ext.to_str()) == Some("rs") {
                files.push(path);
            }
        }
    }

    files
}

fn rust_function_body_by_signature<'a>(content: &'a str, signature: &str, path: &str) -> &'a str {
    let start = match content.find(signature) {
        Some(start) => start,
        None => panic!("failed to find function signature `{signature}` in {path}"),
    };
    let body_start = match content[start..].find('{') {
        Some(offset) => start + offset,
        None => panic!("failed to find function body for `{signature}` in {path}"),
    };

    let mut depth = 0usize;
    for (offset, ch) in content[body_start..].char_indices() {
        match ch {
            '{' => depth = depth.saturating_add(1),
            '}' => {
                depth = depth.saturating_sub(1);
                if depth == 0 {
                    let end = body_start + offset + ch.len_utf8();
                    return &content[body_start..end];
                }
            }
            _ => {}
        }
    }

    panic!("failed to find end of function body for `{signature}` in {path}");
}

fn database_type_enum_variants(content: &str, path: &str) -> Vec<String> {
    let parsed = match syn::parse_file(content) {
        Ok(parsed) => parsed,
        Err(err) => panic!("failed to parse source file {path}: {err}"),
    };

    parsed
        .items
        .iter()
        .find_map(|item| match item {
            Item::Enum(item_enum) if item_enum.ident == "DatabaseType" => Some(
                item_enum
                    .variants
                    .iter()
                    .map(|variant| variant.ident.to_string())
                    .collect::<Vec<_>>(),
            ),
            _ => None,
        })
        .unwrap_or_else(|| panic!("failed to find DatabaseType enum in {path}"))
}

fn impl_self_ty_is_database_type(ty: &Type) -> bool {
    match ty {
        Type::Path(path) => path
            .path
            .segments
            .last()
            .is_some_and(|segment| segment.ident == "DatabaseType"),
        _ => false,
    }
}

fn database_type_all_variants(content: &str, path: &str) -> Vec<String> {
    let parsed = match syn::parse_file(content) {
        Ok(parsed) => parsed,
        Err(err) => panic!("failed to parse source file {path}: {err}"),
    };

    for item in parsed.items {
        let Item::Impl(item_impl) = item else {
            continue;
        };
        if !impl_self_ty_is_database_type(&item_impl.self_ty) {
            continue;
        }

        for impl_item in item_impl.items {
            let ImplItem::Const(item_const) = impl_item else {
                continue;
            };
            if item_const.ident != "ALL" {
                continue;
            }

            let Expr::Array(array) = item_const.expr else {
                panic!("DatabaseType::ALL must be initialized with an array in {path}");
            };

            return array
                .elems
                .iter()
                .map(|expr| match expr {
                    Expr::Path(expr_path) => expr_path
                        .path
                        .segments
                        .last()
                        .map(|segment| segment.ident.to_string())
                        .unwrap_or_else(|| {
                            panic!("DatabaseType::ALL contains an empty path in {path}")
                        }),
                    _ => panic!("DatabaseType::ALL contains a non-path element in {path}"),
                })
                .collect();
        }
    }

    panic!("failed to find DatabaseType::ALL in {path}");
}

fn is_test_source_file(path: &Path) -> bool {
    let file_name = path.file_name().and_then(|name| name.to_str());
    if matches!(file_name, Some("tests.rs")) {
        return true;
    }

    if file_name.is_some_and(|name| name.ends_with("_tests.rs")) {
        return true;
    }

    path.components()
        .any(|component| component.as_os_str() == OsStr::new("tests"))
}

fn attr_is_test_only(attr: &syn::Attribute) -> bool {
    if attr.path().is_ident("test") {
        return true;
    }

    if attr.path().is_ident("cfg") || attr.path().is_ident("cfg_attr") {
        let tokens = attr.meta.to_token_stream().to_string();
        return tokens.contains("test");
    }

    false
}

fn attrs_are_test_only(attrs: &[syn::Attribute]) -> bool {
    attrs.iter().any(attr_is_test_only)
}

fn mentions_backend_kind(tokens: &impl ToTokens) -> bool {
    let text = tokens.to_token_stream().to_string();
    text.contains("DatabaseBackendKind")
        || text.contains("backend_kind")
        || text.contains("SqlDialect")
        || text.contains("sql_dialect")
}

fn mentions_sql_dialect(tokens: &impl ToTokens) -> bool {
    let text = tokens.to_token_stream().to_string();
    text.contains("SqlDialect") || text.contains("sql_dialect")
}

fn mentions_database_type(tokens: &impl ToTokens) -> bool {
    tokens
        .to_token_stream()
        .to_string()
        .contains("DatabaseType")
}

fn method_call_compares_concrete_database_type(node: &ExprMethodCall) -> bool {
    node.method == "is_same_type_as"
        && (mentions_database_type(&*node.receiver) || node.args.iter().any(mentions_database_type))
}

fn mentions_physical_session_enum(tokens: &impl ToTokens) -> bool {
    let text = tokens.to_token_stream().to_string();
    [
        "DbConnection",
        "DbConnectionPool",
        "DbPoolSession",
        "DbSessionLease",
    ]
    .iter()
    .any(|name| text.contains(name))
}

fn matches_macro_pattern_mentions_database_type(node: &Macro) -> bool {
    let mut after_expression_comma = false;
    for token in node.tokens.clone() {
        if !after_expression_comma {
            if matches!(&token, TokenTree::Punct(punct) if punct.as_char() == ',') {
                after_expression_comma = true;
            }
            continue;
        }

        if token.to_string().contains("DatabaseType") {
            return true;
        }
    }
    false
}

fn matches_macro_pattern_mentions_physical_session_enum(node: &Macro) -> bool {
    let mut after_expression_comma = false;
    for token in node.tokens.clone() {
        if !after_expression_comma {
            if matches!(&token, TokenTree::Punct(punct) if punct.as_char() == ',') {
                after_expression_comma = true;
            }
            continue;
        }

        if [
            "DbConnection",
            "DbConnectionPool",
            "DbPoolSession",
            "DbSessionLease",
        ]
        .iter()
        .any(|name| token.to_string().contains(name))
        {
            return true;
        }
    }
    false
}

fn pattern_swallows_new_variants(pat: &Pat) -> bool {
    match pat {
        Pat::Wild(_) => true,
        // A bare lowercase binding (`kind => ...`) matches anything, exactly
        // like `_`, so it hides newly added backend kinds the same way.
        Pat::Ident(ident) => {
            ident.subpat.is_none()
                && ident
                    .ident
                    .to_string()
                    .chars()
                    .next()
                    .is_some_and(|first| first.is_ascii_lowercase())
        }
        _ => false,
    }
}

fn database_type_registry_pattern_groups_variants(pat: &Pat) -> bool {
    match pat {
        Pat::Or(pat_or) => pat_or.cases.iter().any(mentions_database_type),
        _ => false,
    }
}

// Recursively finds wildcard/binding sub-patterns (`Some(_)`, `(x, _)`,
// `Foo { .. }`) that swallow newly added enum variants even though the
// top-level pattern is neither `_` nor a bare binding. Callers combine this
// with a "does the arm mention the dispatched enum at all" check so concrete
// arms with auxiliary wildcards (`(DatabaseType::Oracle, _)`) stay allowed.
fn pattern_contains_swallowing_binding(pat: &Pat) -> bool {
    if pattern_swallows_new_variants(pat) {
        return true;
    }
    match pat {
        Pat::Rest(_) => true,
        Pat::Tuple(tuple) => tuple.elems.iter().any(pattern_contains_swallowing_binding),
        Pat::TupleStruct(tuple_struct) => tuple_struct
            .elems
            .iter()
            .any(pattern_contains_swallowing_binding),
        Pat::Struct(pat_struct) => {
            pat_struct.rest.is_some()
                || pat_struct
                    .fields
                    .iter()
                    .any(|field| pattern_contains_swallowing_binding(&field.pat))
        }
        Pat::Slice(slice) => slice.elems.iter().any(pattern_contains_swallowing_binding),
        Pat::Or(pat_or) => pat_or.cases.iter().any(pattern_contains_swallowing_binding),
        Pat::Paren(paren) => pattern_contains_swallowing_binding(&paren.pat),
        Pat::Reference(reference) => pattern_contains_swallowing_binding(&reference.pat),
        Pat::Ident(ident) => ident
            .subpat
            .as_ref()
            .is_some_and(|(_, subpat)| pattern_contains_swallowing_binding(subpat)),
        _ => false,
    }
}

fn local_is_let_else(node: &syn::Local) -> bool {
    node.init
        .as_ref()
        .is_some_and(|init| init.diverge.is_some())
}

// Physical session matches legitimately bind unexpected variants to a name
// (`Ok(other) => warn!(other.db_type())`) so backends can reject a
// mis-routed session with explicit diagnostics, and let-else extraction must
// diverge. Only anonymous `_` discards silence a newly added session variant,
// so for session enums just silent wildcards are flagged.
fn pattern_contains_silent_wildcard(pat: &Pat) -> bool {
    match pat {
        Pat::Wild(_) => true,
        Pat::Rest(_) => true,
        Pat::Tuple(tuple) => tuple.elems.iter().any(pattern_contains_silent_wildcard),
        Pat::TupleStruct(tuple_struct) => tuple_struct
            .elems
            .iter()
            .any(pattern_contains_silent_wildcard),
        Pat::Struct(pat_struct) => pat_struct
            .fields
            .iter()
            .any(|field| pattern_contains_silent_wildcard(&field.pat)),
        Pat::Slice(slice) => slice.elems.iter().any(pattern_contains_silent_wildcard),
        Pat::Or(pat_or) => pat_or.cases.iter().any(pattern_contains_silent_wildcard),
        Pat::Paren(paren) => pattern_contains_silent_wildcard(&paren.pat),
        Pat::Reference(reference) => pattern_contains_silent_wildcard(&reference.pat),
        Pat::Ident(ident) => ident
            .subpat
            .as_ref()
            .is_some_and(|(_, subpat)| pattern_contains_silent_wildcard(subpat)),
        _ => false,
    }
}

struct BackendKindDispatchVisitor<'a> {
    path: &'a str,
    offenders: Vec<BackendKindDispatchOffender>,
}

impl<'a> BackendKindDispatchVisitor<'a> {
    fn new(path: &'a str) -> Self {
        Self {
            path,
            offenders: Vec::new(),
        }
    }

    fn push_offender(&mut self, line: usize, pattern: String) {
        self.offenders.push(BackendKindDispatchOffender {
            path: self.path.to_string(),
            line,
            pattern,
        });
    }
}

fn shared_result_message_literals() -> &'static [&'static str] {
    &[
        "Commit complete",
        "Rollback complete",
        "Call executed successfully",
        "PL/SQL block executed successfully",
        "Statement executed successfully",
        "Query cancelled",
        "No statements to execute",
        "Auto-commit applied",
        "Commit required",
        "row(s) affected",
        "Current schema changed",
    ]
}

impl Visit<'_> for BackendKindDispatchVisitor<'_> {
    fn visit_item_mod(&mut self, node: &ItemMod) {
        if attrs_are_test_only(&node.attrs) {
            return;
        }
        visit::visit_item_mod(self, node);
    }

    fn visit_item_fn(&mut self, node: &ItemFn) {
        if attrs_are_test_only(&node.attrs) {
            return;
        }
        visit::visit_item_fn(self, node);
    }

    fn visit_expr_binary(&mut self, node: &ExprBinary) {
        if matches!(node.op, BinOp::Eq(_) | BinOp::Ne(_))
            && (mentions_backend_kind(&*node.left) || mentions_backend_kind(&*node.right))
        {
            self.push_offender(
                node.span().start().line,
                "==/!= comparison on DatabaseBackendKind/SqlDialect".to_string(),
            );
        }
        visit::visit_expr_binary(self, node);
    }

    fn visit_macro(&mut self, node: &Macro) {
        let is_matches = node
            .path
            .segments
            .last()
            .is_some_and(|segment| segment.ident == "matches");
        let macro_tokens = node.tokens.to_string();
        if is_matches
            && (macro_tokens.contains("DatabaseBackendKind") || macro_tokens.contains("SqlDialect"))
        {
            self.push_offender(
                node.span().start().line,
                "matches! on DatabaseBackendKind/SqlDialect".to_string(),
            );
        }
        visit::visit_macro(self, node);
    }

    fn visit_expr_let(&mut self, node: &ExprLet) {
        if mentions_backend_kind(&node.pat) || mentions_backend_kind(&*node.expr) {
            self.push_offender(
                node.span().start().line,
                "if let on DatabaseBackendKind/SqlDialect".to_string(),
            );
        }
        visit::visit_expr_let(self, node);
    }

    fn visit_local(&mut self, node: &syn::Local) {
        if local_is_let_else(node) && mentions_backend_kind(&node.pat) {
            self.push_offender(
                node.span().start().line,
                "let-else on DatabaseBackendKind/SqlDialect".to_string(),
            );
        }
        visit::visit_local(self, node);
    }

    fn visit_expr_match(&mut self, node: &ExprMatch) {
        // Trigger on arm patterns too: `let kind = db_type.backend_kind();
        // match kind { ... }` must not hide wildcard arms behind an
        // intermediate binding name the scrutinee text check cannot see.
        let dispatches_backend_kind = mentions_backend_kind(&*node.expr)
            || node.arms.iter().any(|arm| mentions_backend_kind(&arm.pat));
        if dispatches_backend_kind {
            for arm in &node.arms {
                if !mentions_backend_kind(&arm.pat) && pattern_contains_swallowing_binding(&arm.pat)
                {
                    self.push_offender(
                        arm.pat.span().start().line,
                        "wildcard arm in match on DatabaseBackendKind/SqlDialect".to_string(),
                    );
                }
            }
        }
        visit::visit_expr_match(self, node);
    }
}

fn collect_backend_kind_dispatch_offenders(
    content: &str,
    path: &str,
) -> Vec<BackendKindDispatchOffender> {
    let parsed = match syn::parse_file(content) {
        Ok(parsed) => parsed,
        Err(err) => panic!("failed to parse source file {path}: {err}"),
    };
    let mut visitor = BackendKindDispatchVisitor::new(path);
    visitor.visit_file(&parsed);
    visitor.offenders
}

struct DialectToConcreteDatabaseTypeVisitor<'a> {
    path: &'a str,
    offenders: Vec<BackendKindDispatchOffender>,
}

impl<'a> DialectToConcreteDatabaseTypeVisitor<'a> {
    fn new(path: &'a str) -> Self {
        Self {
            path,
            offenders: Vec::new(),
        }
    }

    fn push_offender(&mut self, line: usize, pattern: String) {
        self.offenders.push(BackendKindDispatchOffender {
            path: self.path.to_string(),
            line,
            pattern,
        });
    }
}

impl Visit<'_> for DialectToConcreteDatabaseTypeVisitor<'_> {
    fn visit_item_mod(&mut self, node: &ItemMod) {
        if attrs_are_test_only(&node.attrs) {
            return;
        }
        visit::visit_item_mod(self, node);
    }

    fn visit_item_fn(&mut self, node: &ItemFn) {
        if attrs_are_test_only(&node.attrs) {
            return;
        }
        visit::visit_item_fn(self, node);
    }

    fn visit_expr_match(&mut self, node: &ExprMatch) {
        let matches_sql_dialect = mentions_sql_dialect(&*node.expr)
            || node.arms.iter().any(|arm| mentions_sql_dialect(&arm.pat));
        if matches_sql_dialect {
            for arm in &node.arms {
                if mentions_database_type(&arm.body) {
                    self.push_offender(
                        arm.body.span().start().line,
                        "SqlDialect match arm returns concrete DatabaseType".to_string(),
                    );
                }
            }
        }
        visit::visit_expr_match(self, node);
    }
}

fn collect_dialect_to_concrete_database_type_offenders(
    content: &str,
    path: &str,
) -> Vec<BackendKindDispatchOffender> {
    let parsed = match syn::parse_file(content) {
        Ok(parsed) => parsed,
        Err(err) => panic!("failed to parse source file {path}: {err}"),
    };
    let mut visitor = DialectToConcreteDatabaseTypeVisitor::new(path);
    visitor.visit_file(&parsed);
    visitor.offenders
}

fn mysql_executor_default_method_name(func: &Expr) -> Option<&'static str> {
    let path = func.to_token_stream().to_string().replace(' ', "");
    [
        "execute",
        "execute_batch",
        "classify_statement",
        "is_select_statement",
        "is_displayable_select_statement",
        "is_use_statement",
        "use_statement_database_name",
    ]
    .into_iter()
    .find(|method| path.ends_with(&format!("MysqlExecutor::{method}")))
}

struct MysqlExecutorDefaultMethodVisitor<'a> {
    path: &'a str,
    offenders: Vec<BackendKindDispatchOffender>,
}

impl<'a> MysqlExecutorDefaultMethodVisitor<'a> {
    fn new(path: &'a str) -> Self {
        Self {
            path,
            offenders: Vec::new(),
        }
    }

    fn push_offender(&mut self, line: usize, pattern: String) {
        self.offenders.push(BackendKindDispatchOffender {
            path: self.path.to_string(),
            line,
            pattern,
        });
    }
}

impl Visit<'_> for MysqlExecutorDefaultMethodVisitor<'_> {
    fn visit_item_mod(&mut self, node: &ItemMod) {
        if attrs_are_test_only(&node.attrs) {
            return;
        }
        visit::visit_item_mod(self, node);
    }

    fn visit_item_fn(&mut self, node: &ItemFn) {
        if attrs_are_test_only(&node.attrs) {
            return;
        }
        visit::visit_item_fn(self, node);
    }

    fn visit_expr_call(&mut self, node: &ExprCall) {
        if let Some(method) = mysql_executor_default_method_name(&node.func) {
            self.push_offender(
                node.func.span().start().line,
                format!("MysqlExecutor::{method} call without concrete DatabaseType"),
            );
        }
        visit::visit_expr_call(self, node);
    }
}

fn collect_mysql_executor_default_method_offenders(
    content: &str,
    path: &str,
) -> Vec<BackendKindDispatchOffender> {
    let parsed = match syn::parse_file(content) {
        Ok(parsed) => parsed,
        Err(err) => panic!("failed to parse source file {path}: {err}"),
    };
    let mut visitor = MysqlExecutorDefaultMethodVisitor::new(path);
    visitor.visit_file(&parsed);
    visitor.offenders
}

struct UiDatabaseTypeDispatchVisitor<'a> {
    path: &'a str,
    database_type_registry_depth: usize,
    offenders: Vec<BackendKindDispatchOffender>,
}

impl<'a> UiDatabaseTypeDispatchVisitor<'a> {
    fn new(path: &'a str) -> Self {
        Self {
            path,
            database_type_registry_depth: 0,
            offenders: Vec::new(),
        }
    }

    fn push_offender(&mut self, line: usize, pattern: String) {
        self.offenders.push(BackendKindDispatchOffender {
            path: self.path.to_string(),
            line,
            pattern,
        });
    }
}

const UI_DATABASE_TYPE_REGISTRY_FUNCTIONS: &[&str] = &[
    "execution_worker_backend_for",
    "explain_plan_backend_for",
    "transaction_action_backend_for",
    "quick_describe_backend_for",
    "signature_backend_for",
    "column_load_backend_for",
    "object_browser_behavior_for",
    "schema_metadata_loader_for",
    "language_catalog_for_db_type",
    "function_catalog_for_db_type",
    "mysql_compatible_highlight_mode",
];

fn ui_database_type_registry_function(name: &str) -> bool {
    UI_DATABASE_TYPE_REGISTRY_FUNCTIONS.contains(&name)
}

impl Visit<'_> for UiDatabaseTypeDispatchVisitor<'_> {
    fn visit_item_mod(&mut self, node: &ItemMod) {
        if attrs_are_test_only(&node.attrs) {
            return;
        }
        visit::visit_item_mod(self, node);
    }

    fn visit_item_fn(&mut self, node: &ItemFn) {
        if attrs_are_test_only(&node.attrs) {
            return;
        }
        if ui_database_type_registry_function(&node.sig.ident.to_string()) {
            self.database_type_registry_depth += 1;
            visit::visit_item_fn(self, node);
            self.database_type_registry_depth -= 1;
            return;
        }
        visit::visit_item_fn(self, node);
    }

    fn visit_expr_binary(&mut self, node: &ExprBinary) {
        if matches!(node.op, BinOp::Eq(_) | BinOp::Ne(_))
            && (mentions_database_type(&*node.left) || mentions_database_type(&*node.right))
        {
            self.push_offender(
                node.span().start().line,
                "==/!= comparison on DatabaseType in UI code".to_string(),
            );
        }
        visit::visit_expr_binary(self, node);
    }

    fn visit_expr_method_call(&mut self, node: &ExprMethodCall) {
        if method_call_compares_concrete_database_type(node) {
            self.push_offender(
                node.span().start().line,
                "method comparison on concrete DatabaseType in UI code".to_string(),
            );
        }
        visit::visit_expr_method_call(self, node);
    }

    fn visit_macro(&mut self, node: &Macro) {
        let is_matches = node
            .path
            .segments
            .last()
            .is_some_and(|segment| segment.ident == "matches");
        if is_matches && matches_macro_pattern_mentions_database_type(node) {
            self.push_offender(
                node.span().start().line,
                "matches! on DatabaseType in UI code".to_string(),
            );
        }
        visit::visit_macro(self, node);
    }

    fn visit_expr_let(&mut self, node: &ExprLet) {
        if mentions_database_type(&node.pat) {
            self.push_offender(
                node.span().start().line,
                "if let on DatabaseType in UI code".to_string(),
            );
        }
        visit::visit_expr_let(self, node);
    }

    fn visit_local(&mut self, node: &syn::Local) {
        if local_is_let_else(node) && mentions_database_type(&node.pat) {
            self.push_offender(
                node.span().start().line,
                "let-else on DatabaseType in UI code".to_string(),
            );
        }
        visit::visit_local(self, node);
    }

    fn visit_expr_match(&mut self, node: &ExprMatch) {
        let match_mentions_database_type =
            node.arms.iter().any(|arm| mentions_database_type(&arm.pat));
        if match_mentions_database_type {
            if self.database_type_registry_depth > 0 {
                for arm in &node.arms {
                    if !mentions_database_type(&arm.pat)
                        && pattern_contains_swallowing_binding(&arm.pat)
                    {
                        self.push_offender(
                            arm.pat.span().start().line,
                            "wildcard arm in UI DatabaseType backend registry match".to_string(),
                        );
                    } else if database_type_registry_pattern_groups_variants(&arm.pat) {
                        self.push_offender(
                            arm.pat.span().start().line,
                            "grouped DatabaseType arm in UI backend registry match".to_string(),
                        );
                    }
                }
            } else {
                for arm in &node.arms {
                    if mentions_database_type(&arm.pat) {
                        self.push_offender(
                            arm.pat.span().start().line,
                            "match arm on DatabaseType in UI code".to_string(),
                        );
                    }
                }
            }
        }
        visit::visit_expr_match(self, node);
    }
}

fn collect_ui_database_type_dispatch_offenders(
    content: &str,
    path: &str,
) -> Vec<BackendKindDispatchOffender> {
    let parsed = match syn::parse_file(content) {
        Ok(parsed) => parsed,
        Err(err) => panic!("failed to parse source file {path}: {err}"),
    };
    let mut visitor = UiDatabaseTypeDispatchVisitor::new(path);
    visitor.visit_file(&parsed);
    visitor.offenders
}

struct DbDatabaseTypeDispatchVisitor<'a> {
    path: &'a str,
    offenders: Vec<BackendKindDispatchOffender>,
}

impl<'a> DbDatabaseTypeDispatchVisitor<'a> {
    fn new(path: &'a str) -> Self {
        Self {
            path,
            offenders: Vec::new(),
        }
    }

    fn push_offender(&mut self, line: usize, pattern: String) {
        self.offenders.push(BackendKindDispatchOffender {
            path: self.path.to_string(),
            line,
            pattern,
        });
    }
}

impl Visit<'_> for DbDatabaseTypeDispatchVisitor<'_> {
    fn visit_item_mod(&mut self, node: &ItemMod) {
        if attrs_are_test_only(&node.attrs) {
            return;
        }
        visit::visit_item_mod(self, node);
    }

    fn visit_item_fn(&mut self, node: &ItemFn) {
        if attrs_are_test_only(&node.attrs) {
            return;
        }
        visit::visit_item_fn(self, node);
    }

    fn visit_expr_binary(&mut self, node: &ExprBinary) {
        if matches!(node.op, BinOp::Eq(_) | BinOp::Ne(_))
            && (mentions_database_type(&*node.left) || mentions_database_type(&*node.right))
        {
            self.push_offender(
                node.span().start().line,
                "==/!= comparison on DatabaseType in db code".to_string(),
            );
        }
        visit::visit_expr_binary(self, node);
    }

    fn visit_expr_method_call(&mut self, node: &ExprMethodCall) {
        if method_call_compares_concrete_database_type(node) {
            self.push_offender(
                node.span().start().line,
                "method comparison on concrete DatabaseType in db code".to_string(),
            );
        }
        visit::visit_expr_method_call(self, node);
    }

    fn visit_macro(&mut self, node: &Macro) {
        let is_matches = node
            .path
            .segments
            .last()
            .is_some_and(|segment| segment.ident == "matches");
        if is_matches && matches_macro_pattern_mentions_database_type(node) {
            self.push_offender(
                node.span().start().line,
                "matches! on DatabaseType in db code".to_string(),
            );
        }
        visit::visit_macro(self, node);
    }

    fn visit_expr_let(&mut self, node: &ExprLet) {
        if mentions_database_type(&node.pat) {
            self.push_offender(
                node.span().start().line,
                "if let on DatabaseType in db code".to_string(),
            );
        }
        visit::visit_expr_let(self, node);
    }

    fn visit_local(&mut self, node: &syn::Local) {
        if local_is_let_else(node) && mentions_database_type(&node.pat) {
            self.push_offender(
                node.span().start().line,
                "let-else on DatabaseType in db code".to_string(),
            );
        }
        visit::visit_local(self, node);
    }

    fn visit_expr_match(&mut self, node: &ExprMatch) {
        let match_mentions_database_type =
            node.arms.iter().any(|arm| mentions_database_type(&arm.pat));
        if match_mentions_database_type {
            for arm in &node.arms {
                if !mentions_database_type(&arm.pat)
                    && pattern_contains_swallowing_binding(&arm.pat)
                {
                    self.push_offender(
                        arm.pat.span().start().line,
                        "wildcard arm in match on DatabaseType in db code".to_string(),
                    );
                }
            }
        }
        visit::visit_expr_match(self, node);
    }
}

fn collect_db_database_type_dispatch_offenders(
    content: &str,
    path: &str,
) -> Vec<BackendKindDispatchOffender> {
    let parsed = match syn::parse_file(content) {
        Ok(parsed) => parsed,
        Err(err) => panic!("failed to parse source file {path}: {err}"),
    };
    let mut visitor = DbDatabaseTypeDispatchVisitor::new(path);
    visitor.visit_file(&parsed);
    visitor.offenders
}

struct PhysicalSessionEnumDispatchVisitor<'a> {
    path: &'a str,
    offenders: Vec<BackendKindDispatchOffender>,
}

impl<'a> PhysicalSessionEnumDispatchVisitor<'a> {
    fn new(path: &'a str) -> Self {
        Self {
            path,
            offenders: Vec::new(),
        }
    }

    fn push_offender(&mut self, line: usize, pattern: String) {
        self.offenders.push(BackendKindDispatchOffender {
            path: self.path.to_string(),
            line,
            pattern,
        });
    }
}

impl Visit<'_> for PhysicalSessionEnumDispatchVisitor<'_> {
    fn visit_item_mod(&mut self, node: &ItemMod) {
        if attrs_are_test_only(&node.attrs) {
            return;
        }
        visit::visit_item_mod(self, node);
    }

    fn visit_item_fn(&mut self, node: &ItemFn) {
        if attrs_are_test_only(&node.attrs) {
            return;
        }
        visit::visit_item_fn(self, node);
    }

    fn visit_macro(&mut self, node: &Macro) {
        let is_matches = node
            .path
            .segments
            .last()
            .is_some_and(|segment| segment.ident == "matches");
        if is_matches && matches_macro_pattern_mentions_physical_session_enum(node) {
            self.push_offender(
                node.span().start().line,
                "matches! on physical DB session enum".to_string(),
            );
        }
        visit::visit_macro(self, node);
    }

    fn visit_expr_let(&mut self, node: &ExprLet) {
        if mentions_physical_session_enum(&node.pat) {
            self.push_offender(
                node.span().start().line,
                "if let on physical DB session enum".to_string(),
            );
        }
        visit::visit_expr_let(self, node);
    }

    fn visit_expr_match(&mut self, node: &ExprMatch) {
        let match_mentions_physical_session_enum = node
            .arms
            .iter()
            .any(|arm| mentions_physical_session_enum(&arm.pat));
        if match_mentions_physical_session_enum {
            for arm in &node.arms {
                if !mentions_physical_session_enum(&arm.pat)
                    && (pattern_swallows_new_variants(&arm.pat)
                        || pattern_contains_silent_wildcard(&arm.pat))
                {
                    self.push_offender(
                        arm.pat.span().start().line,
                        "wildcard arm in match on physical DB session enum".to_string(),
                    );
                }
            }
        }
        visit::visit_expr_match(self, node);
    }
}

fn collect_physical_session_enum_dispatch_offenders(
    content: &str,
    path: &str,
) -> Vec<BackendKindDispatchOffender> {
    let parsed = match syn::parse_file(content) {
        Ok(parsed) => parsed,
        Err(err) => panic!("failed to parse source file {path}: {err}"),
    };
    let mut visitor = PhysicalSessionEnumDispatchVisitor::new(path);
    visitor.visit_file(&parsed);
    visitor.offenders
}

struct SharedResultMessageLiteralVisitor<'a> {
    path: &'a str,
    offenders: Vec<BackendKindDispatchOffender>,
}

impl<'a> SharedResultMessageLiteralVisitor<'a> {
    fn new(path: &'a str) -> Self {
        Self {
            path,
            offenders: Vec::new(),
        }
    }

    fn push_offender(&mut self, line: usize, value: &str) {
        self.offenders.push(BackendKindDispatchOffender {
            path: self.path.to_string(),
            line,
            pattern: format!("shared result message literal `{value}`"),
        });
    }
}

impl Visit<'_> for SharedResultMessageLiteralVisitor<'_> {
    fn visit_item_mod(&mut self, node: &ItemMod) {
        if attrs_are_test_only(&node.attrs) {
            return;
        }
        visit::visit_item_mod(self, node);
    }

    fn visit_item_fn(&mut self, node: &ItemFn) {
        if attrs_are_test_only(&node.attrs) {
            return;
        }
        visit::visit_item_fn(self, node);
    }

    fn visit_lit_str(&mut self, node: &LitStr) {
        let value = node.value();
        if shared_result_message_literals()
            .iter()
            .any(|literal| value.contains(literal))
        {
            self.push_offender(node.span().start().line, &value);
        }
    }
}

fn collect_shared_result_message_literal_offenders(
    content: &str,
    path: &str,
) -> Vec<BackendKindDispatchOffender> {
    let parsed = match syn::parse_file(content) {
        Ok(parsed) => parsed,
        Err(err) => panic!("failed to parse source file {path}: {err}"),
    };
    let mut visitor = SharedResultMessageLiteralVisitor::new(path);
    visitor.visit_file(&parsed);
    visitor.offenders
}

// Driver-specific error-code markers (`ORA-`, `DPI-`) hardcoded in generic UI
// classify only that one backend's errors; a new DB's equivalents would be
// silently unclassified. Generic UI must delegate message classification to
// the per-DB marker catalogs in `db::session_policy`
// (`message_indicates_query_cancel`, `message_indicates_connection_loss`,
// `error_line_message_patterns`), whose exhaustive `DatabaseType` matches
// force a decision per backend.
fn driver_error_marker_prefixes() -> &'static [&'static str] {
    &["ora-", "dpi-"]
}

struct DriverErrorMarkerLiteralVisitor<'a> {
    path: &'a str,
    offenders: Vec<BackendKindDispatchOffender>,
}

impl<'a> DriverErrorMarkerLiteralVisitor<'a> {
    fn new(path: &'a str) -> Self {
        Self {
            path,
            offenders: Vec::new(),
        }
    }

    fn push_offender(&mut self, line: usize, value: &str) {
        self.offenders.push(BackendKindDispatchOffender {
            path: self.path.to_string(),
            line,
            pattern: format!("driver error marker literal `{value}`"),
        });
    }
}

impl Visit<'_> for DriverErrorMarkerLiteralVisitor<'_> {
    fn visit_item_mod(&mut self, node: &ItemMod) {
        if attrs_are_test_only(&node.attrs) {
            return;
        }
        visit::visit_item_mod(self, node);
    }

    fn visit_item_fn(&mut self, node: &ItemFn) {
        if attrs_are_test_only(&node.attrs) {
            return;
        }
        visit::visit_item_fn(self, node);
    }

    fn visit_lit_str(&mut self, node: &LitStr) {
        let value = node.value().to_ascii_lowercase();
        if driver_error_marker_prefixes()
            .iter()
            .any(|marker| value.contains(marker))
        {
            self.push_offender(node.span().start().line, &node.value());
        }
    }
}

fn collect_driver_error_marker_literal_offenders(
    content: &str,
    path: &str,
) -> Vec<BackendKindDispatchOffender> {
    let parsed = match syn::parse_file(content) {
        Ok(parsed) => parsed,
        Err(err) => panic!("failed to parse source file {path}: {err}"),
    };
    let mut visitor = DriverErrorMarkerLiteralVisitor::new(path);
    visitor.visit_file(&parsed);
    visitor.offenders
}

// The dispatch guards above recognize decision sites textually
// (`DatabaseType`, `backend_kind`, ...). Renaming the enum at the import or a
// type alias (`use DatabaseType as Dt`, `type Dt = DatabaseType`) or
// importing variants directly (`use DatabaseType::Oracle`) would let `match
// Dt::Oracle`/`match Oracle` bypass every one of them, so those import shapes
// are banned outright in non-test source.
fn dispatch_enum_names() -> &'static [&'static str] {
    &[
        "DatabaseType",
        "DatabaseBackendKind",
        "SqlDialect",
        "DbConnection",
        "DbConnectionPool",
        "DbPoolSession",
        "DbSessionLease",
    ]
}

fn type_path_is_dispatch_enum(ty: &Type) -> Option<String> {
    let Type::Path(type_path) = ty else {
        return None;
    };
    let segment = type_path.path.segments.last()?;
    if !segment.arguments.is_none() {
        return None;
    }
    let ident = segment.ident.to_string();
    dispatch_enum_names()
        .iter()
        .any(|name| *name == ident)
        .then_some(ident)
}

struct DispatchEnumAliasImportVisitor<'a> {
    path: &'a str,
    offenders: Vec<BackendKindDispatchOffender>,
}

impl<'a> DispatchEnumAliasImportVisitor<'a> {
    fn new(path: &'a str) -> Self {
        Self {
            path,
            offenders: Vec::new(),
        }
    }

    fn push_offender(&mut self, line: usize, pattern: String) {
        self.offenders.push(BackendKindDispatchOffender {
            path: self.path.to_string(),
            line,
            pattern,
        });
    }

    fn check_use_tree(&mut self, tree: &syn::UseTree) {
        match tree {
            syn::UseTree::Path(use_path) => {
                let ident = use_path.ident.to_string();
                if dispatch_enum_names().iter().any(|name| *name == ident) {
                    self.push_offender(
                        use_path.ident.span().start().line,
                        format!("variant or glob import under `{ident}::`"),
                    );
                } else {
                    self.check_use_tree(&use_path.tree);
                }
            }
            syn::UseTree::Rename(rename) => {
                let ident = rename.ident.to_string();
                if dispatch_enum_names().iter().any(|name| *name == ident) {
                    self.push_offender(
                        rename.ident.span().start().line,
                        format!("alias import `{ident} as {}`", rename.rename),
                    );
                }
            }
            syn::UseTree::Group(group) => {
                for item in &group.items {
                    self.check_use_tree(item);
                }
            }
            syn::UseTree::Name(_) | syn::UseTree::Glob(_) => {}
        }
    }
}

impl Visit<'_> for DispatchEnumAliasImportVisitor<'_> {
    fn visit_item_mod(&mut self, node: &ItemMod) {
        if attrs_are_test_only(&node.attrs) {
            return;
        }
        visit::visit_item_mod(self, node);
    }

    fn visit_item_fn(&mut self, node: &ItemFn) {
        if attrs_are_test_only(&node.attrs) {
            return;
        }
        visit::visit_item_fn(self, node);
    }

    fn visit_item_use(&mut self, node: &syn::ItemUse) {
        self.check_use_tree(&node.tree);
        visit::visit_item_use(self, node);
    }

    fn visit_item_type(&mut self, node: &syn::ItemType) {
        if let Some(enum_name) = type_path_is_dispatch_enum(&node.ty) {
            self.push_offender(
                node.ident.span().start().line,
                format!("type alias `{}` of dispatch enum `{enum_name}`", node.ident),
            );
        }
        visit::visit_item_type(self, node);
    }

    fn visit_impl_item_type(&mut self, node: &syn::ImplItemType) {
        if let Some(enum_name) = type_path_is_dispatch_enum(&node.ty) {
            self.push_offender(
                node.ident.span().start().line,
                format!(
                    "associated type `{}` aliasing dispatch enum `{enum_name}`",
                    node.ident
                ),
            );
        }
        visit::visit_impl_item_type(self, node);
    }
}

fn collect_dispatch_enum_alias_import_offenders(
    content: &str,
    path: &str,
) -> Vec<BackendKindDispatchOffender> {
    let parsed = match syn::parse_file(content) {
        Ok(parsed) => parsed,
        Err(err) => panic!("failed to parse source file {path}: {err}"),
    };
    let mut visitor = DispatchEnumAliasImportVisitor::new(path);
    visitor.visit_file(&parsed);
    visitor.offenders
}

// Inside `impl DatabaseType { ... }` (or any dispatch enum impl), variant
// patterns can be written as `Self::Oracle`, which none of the text-based
// dispatch guards can attribute to the enum. Such impls must spell out the
// full enum name in patterns so every other guard keeps seeing them.
struct DispatchEnumImplSelfPatternVisitor<'a> {
    path: &'a str,
    dispatch_enum_impl_depth: usize,
    offenders: Vec<BackendKindDispatchOffender>,
}

impl<'a> DispatchEnumImplSelfPatternVisitor<'a> {
    fn new(path: &'a str) -> Self {
        Self {
            path,
            dispatch_enum_impl_depth: 0,
            offenders: Vec::new(),
        }
    }

    fn push_offender(&mut self, line: usize, pattern: String) {
        self.offenders.push(BackendKindDispatchOffender {
            path: self.path.to_string(),
            line,
            pattern,
        });
    }

    fn in_dispatch_enum_impl(&self) -> bool {
        self.dispatch_enum_impl_depth > 0
    }
}

fn impl_self_ty_is_dispatch_enum(ty: &Type) -> bool {
    match ty {
        Type::Path(type_path) => type_path.path.segments.last().is_some_and(|segment| {
            dispatch_enum_names()
                .iter()
                .any(|name| segment.ident == name)
        }),
        _ => false,
    }
}

fn pattern_mentions_self_path(tokens: &impl ToTokens) -> bool {
    tokens.to_token_stream().to_string().contains("Self ::")
}

impl Visit<'_> for DispatchEnumImplSelfPatternVisitor<'_> {
    fn visit_item_mod(&mut self, node: &ItemMod) {
        if attrs_are_test_only(&node.attrs) {
            return;
        }
        visit::visit_item_mod(self, node);
    }

    fn visit_item_fn(&mut self, node: &ItemFn) {
        if attrs_are_test_only(&node.attrs) {
            return;
        }
        visit::visit_item_fn(self, node);
    }

    fn visit_item_impl(&mut self, node: &syn::ItemImpl) {
        if attrs_are_test_only(&node.attrs) {
            return;
        }
        let saved_depth = self.dispatch_enum_impl_depth;
        if impl_self_ty_is_dispatch_enum(&node.self_ty) {
            self.dispatch_enum_impl_depth += 1;
        } else {
            // A nested impl of another type rebinds `Self`, so its patterns
            // must not inherit the outer dispatch-enum context.
            self.dispatch_enum_impl_depth = 0;
        }
        visit::visit_item_impl(self, node);
        self.dispatch_enum_impl_depth = saved_depth;
    }

    fn visit_arm(&mut self, node: &syn::Arm) {
        if self.in_dispatch_enum_impl() && pattern_mentions_self_path(&node.pat) {
            self.push_offender(
                node.pat.span().start().line,
                "Self:: match pattern inside dispatch enum impl".to_string(),
            );
        }
        visit::visit_arm(self, node);
    }

    fn visit_expr_let(&mut self, node: &ExprLet) {
        if self.in_dispatch_enum_impl() && pattern_mentions_self_path(&node.pat) {
            self.push_offender(
                node.span().start().line,
                "Self:: if-let pattern inside dispatch enum impl".to_string(),
            );
        }
        visit::visit_expr_let(self, node);
    }

    fn visit_local(&mut self, node: &syn::Local) {
        if self.in_dispatch_enum_impl()
            && local_is_let_else(node)
            && pattern_mentions_self_path(&node.pat)
        {
            self.push_offender(
                node.span().start().line,
                "Self:: let-else pattern inside dispatch enum impl".to_string(),
            );
        }
        visit::visit_local(self, node);
    }

    fn visit_macro(&mut self, node: &Macro) {
        let is_matches = node
            .path
            .segments
            .last()
            .is_some_and(|segment| segment.ident == "matches");
        if self.in_dispatch_enum_impl() && is_matches && node.tokens.to_string().contains("Self ::")
        {
            self.push_offender(
                node.span().start().line,
                "matches! with Self:: pattern inside dispatch enum impl".to_string(),
            );
        }
        visit::visit_macro(self, node);
    }
}

fn collect_dispatch_enum_impl_self_pattern_offenders(
    content: &str,
    path: &str,
) -> Vec<BackendKindDispatchOffender> {
    let parsed = match syn::parse_file(content) {
        Ok(parsed) => parsed,
        Err(err) => panic!("failed to parse source file {path}: {err}"),
    };
    let mut visitor = DispatchEnumImplSelfPatternVisitor::new(path);
    visitor.visit_file(&parsed);
    visitor.offenders
}

struct BackendTraitDefaultMethodVisitor<'a> {
    path: &'a str,
    trait_names: &'a [&'a str],
    allowed_default_methods: &'a [(&'a str, &'a [&'a str])],
    offenders: Vec<BackendKindDispatchOffender>,
}

impl<'a> BackendTraitDefaultMethodVisitor<'a> {
    fn new(
        path: &'a str,
        trait_names: &'a [&'a str],
        allowed_default_methods: &'a [(&'a str, &'a [&'a str])],
    ) -> Self {
        Self {
            path,
            trait_names,
            allowed_default_methods,
            offenders: Vec::new(),
        }
    }

    fn push_offender(&mut self, line: usize, trait_name: &str, method_name: &str) {
        self.offenders.push(BackendKindDispatchOffender {
            path: self.path.to_string(),
            line,
            pattern: format!("default method body `{trait_name}::{method_name}`"),
        });
    }

    fn default_method_is_allowed(&self, trait_name: &str, method_name: &str) -> bool {
        self.allowed_default_methods
            .iter()
            .find(|(allowed_trait, _)| *allowed_trait == trait_name)
            .is_some_and(|(_, methods)| methods.contains(&method_name))
    }
}

impl Visit<'_> for BackendTraitDefaultMethodVisitor<'_> {
    fn visit_item_mod(&mut self, node: &ItemMod) {
        if attrs_are_test_only(&node.attrs) {
            return;
        }
        visit::visit_item_mod(self, node);
    }

    fn visit_item_trait(&mut self, node: &syn::ItemTrait) {
        if attrs_are_test_only(&node.attrs) {
            return;
        }

        let trait_name = node.ident.to_string();
        if self.trait_names.iter().any(|name| *name == trait_name) {
            for item in &node.items {
                let TraitItem::Fn(method) = item else {
                    continue;
                };
                if method.default.is_some() {
                    let method_name = method.sig.ident.to_string();
                    if !self.default_method_is_allowed(&trait_name, &method_name) {
                        self.push_offender(
                            method.sig.ident.span().start().line,
                            &trait_name,
                            &method_name,
                        );
                    }
                }
            }
        }
        visit::visit_item_trait(self, node);
    }
}

fn collect_backend_trait_default_method_offenders(
    content: &str,
    path: &str,
    trait_names: &[&str],
) -> Vec<BackendKindDispatchOffender> {
    let parsed = match syn::parse_file(content) {
        Ok(parsed) => parsed,
        Err(err) => panic!("failed to parse source file {path}: {err}"),
    };
    let mut visitor = BackendTraitDefaultMethodVisitor::new(path, trait_names, &[]);
    visitor.visit_file(&parsed);
    visitor.offenders
}

/// `(trait name, allowed default method names)` pairs for one source file.
type TraitDefaultMethodAllowlist<'a> = &'a [(&'a str, &'a [&'a str])];

fn collect_backend_trait_unapproved_default_method_offenders(
    content: &str,
    path: &str,
    allowed_default_methods: TraitDefaultMethodAllowlist,
) -> Vec<BackendKindDispatchOffender> {
    let parsed = match syn::parse_file(content) {
        Ok(parsed) => parsed,
        Err(err) => panic!("failed to parse source file {path}: {err}"),
    };
    let trait_names: Vec<&str> = allowed_default_methods
        .iter()
        .map(|(trait_name, _)| *trait_name)
        .collect();
    let mut visitor =
        BackendTraitDefaultMethodVisitor::new(path, &trait_names, allowed_default_methods);
    visitor.visit_file(&parsed);
    visitor.offenders
}

struct RegistryFunctionDispatchVisitor<'a> {
    path: &'a str,
    function_names: &'a [&'a str],
    found: Vec<String>,
    offenders: Vec<BackendKindDispatchOffender>,
}

impl<'a> RegistryFunctionDispatchVisitor<'a> {
    fn new(path: &'a str, function_names: &'a [&'a str]) -> Self {
        Self {
            path,
            function_names,
            found: Vec::new(),
            offenders: Vec::new(),
        }
    }

    fn push_offender(&mut self, line: usize, function_name: &str, pattern: &str) {
        self.offenders.push(BackendKindDispatchOffender {
            path: self.path.to_string(),
            line,
            pattern: format!("{function_name}: {pattern}"),
        });
    }
}

impl Visit<'_> for RegistryFunctionDispatchVisitor<'_> {
    fn visit_item_mod(&mut self, node: &ItemMod) {
        if attrs_are_test_only(&node.attrs) {
            return;
        }
        visit::visit_item_mod(self, node);
    }

    fn visit_item_fn(&mut self, node: &ItemFn) {
        if attrs_are_test_only(&node.attrs) {
            return;
        }

        let function_name = node.sig.ident.to_string();
        if self
            .function_names
            .iter()
            .any(|target| *target == function_name)
        {
            self.found.push(function_name.clone());
            let body = node.block.to_token_stream().to_string();
            if body.contains("backend_kind") {
                self.push_offender(
                    node.sig.ident.span().start().line,
                    &function_name,
                    "registry dispatch uses backend_kind()",
                );
            }
            if !body.contains("DatabaseType") {
                self.push_offender(
                    node.sig.ident.span().start().line,
                    &function_name,
                    "registry dispatch does not mention concrete DatabaseType",
                );
            }
        }
        visit::visit_item_fn(self, node);
    }
}

fn collect_registry_function_dispatch_offenders(
    content: &str,
    path: &str,
    function_names: &[&str],
) -> Vec<BackendKindDispatchOffender> {
    let parsed = match syn::parse_file(content) {
        Ok(parsed) => parsed,
        Err(err) => panic!("failed to parse source file {path}: {err}"),
    };
    let mut visitor = RegistryFunctionDispatchVisitor::new(path, function_names);
    visitor.visit_file(&parsed);
    for function_name in function_names {
        if !visitor.found.iter().any(|found| found == function_name) {
            visitor.push_offender(0, function_name, "registry function is missing");
        }
    }
    visitor.offenders
}

#[test]
fn non_test_source_dispatches_backend_kind_with_exhaustive_match_only() {
    let manifest_dir = Path::new(env!("CARGO_MANIFEST_DIR"));
    let src_root = manifest_dir.join("src");

    let mut offenders = Vec::new();
    for file in collect_rust_files(&src_root) {
        if is_test_source_file(&file) {
            continue;
        }

        let content = match fs::read_to_string(&file) {
            Ok(content) => content,
            Err(err) => panic!("failed to read source file {}: {err}", file.display()),
        };
        let relative_path = match file.strip_prefix(manifest_dir) {
            Ok(path) => path.display().to_string(),
            Err(err) => panic!("failed to relativize {}: {err}", file.display()),
        };

        offenders.extend(collect_backend_kind_dispatch_offenders(
            &content,
            &relative_path,
        ));
    }

    assert!(
        offenders.is_empty(),
        "found non-exhaustive DatabaseBackendKind dispatch in non-test source files; \
         use an exhaustive `match db_type.backend_kind() {{ ... }}` so adding a new \
         backend kind forces a compile error at every decision site: {:?}",
        offenders
    );
}

#[test]
fn non_test_source_does_not_map_sql_dialect_to_concrete_database_type() {
    let manifest_dir = Path::new(env!("CARGO_MANIFEST_DIR"));
    let src_root = manifest_dir.join("src");

    let mut offenders = Vec::new();
    for file in collect_rust_files(&src_root) {
        if is_test_source_file(&file) {
            continue;
        }

        let content = match fs::read_to_string(&file) {
            Ok(content) => content,
            Err(err) => panic!("failed to read source file {}: {err}", file.display()),
        };
        let relative_path = match file.strip_prefix(manifest_dir) {
            Ok(path) => path.display().to_string(),
            Err(err) => panic!("failed to relativize {}: {err}", file.display()),
        };

        offenders.extend(collect_dialect_to_concrete_database_type_offenders(
            &content,
            &relative_path,
        ));
    }

    assert!(
        offenders.is_empty(),
        "found SqlDialect dispatch that returns a concrete DatabaseType in non-test source files; \
         dialects are families, not concrete databases, so callers must pass through the real \
         DatabaseType instead of silently falling back to MySQL or Oracle: {:?}",
        offenders
    );
}

#[test]
fn non_test_source_does_not_call_mysql_executor_default_methods() {
    let manifest_dir = Path::new(env!("CARGO_MANIFEST_DIR"));
    let src_root = manifest_dir.join("src");

    let mut offenders = Vec::new();
    for file in collect_rust_files(&src_root) {
        if is_test_source_file(&file) {
            continue;
        }

        let content = match fs::read_to_string(&file) {
            Ok(content) => content,
            Err(err) => panic!("failed to read source file {}: {err}", file.display()),
        };
        let relative_path = match file.strip_prefix(manifest_dir) {
            Ok(path) => path.display().to_string(),
            Err(err) => panic!("failed to relativize {}: {err}", file.display()),
        };

        offenders.extend(collect_mysql_executor_default_method_offenders(
            &content,
            &relative_path,
        ));
    }

    assert!(
        offenders.is_empty(),
        "found non-test calls to MysqlExecutor methods that silently default to MySQL; \
         use the matching *_for_db_type API so MariaDB and future MySQL-family databases \
         keep their concrete behavior: {:?}",
        offenders
    );
}

#[test]
fn non_test_source_does_not_call_database_connection_mysql_default_helpers() {
    let manifest_dir = Path::new(env!("CARGO_MANIFEST_DIR"));
    let src_root = manifest_dir.join("src");
    let forbidden_calls = [
        "DatabaseConnection::apply_mysql_session_settings(",
        "DatabaseConnection::apply_mysql_connection_encoding_with_settings(",
        "DatabaseConnection::reset_mysql_session_to_no_database(",
        "DatabaseConnection::apply_mysql_autocommit_setting(",
        "Self::apply_mysql_session_settings(",
        "Self::apply_mysql_connection_encoding_with_settings(",
        "Self::reset_mysql_session_to_no_database(",
        "Self::apply_mysql_autocommit_setting(",
    ];

    let mut offenders = Vec::new();
    for file in collect_rust_files(&src_root) {
        if is_test_source_file(&file) {
            continue;
        }
        let content = match fs::read_to_string(&file) {
            Ok(content) => content,
            Err(err) => panic!("failed to read source file {}: {err}", file.display()),
        };
        let relative_path = match file.strip_prefix(manifest_dir) {
            Ok(path) => path.display().to_string(),
            Err(err) => panic!("failed to relativize {}: {err}", file.display()),
        };

        for (line_index, line) in content.lines().enumerate() {
            if let Some(call) = forbidden_calls.iter().find(|call| line.contains(**call)) {
                offenders.push(format!("{}:{}: {}", relative_path, line_index + 1, call));
            }
        }
    }

    assert!(
        offenders.is_empty(),
        "found non-test calls to DatabaseConnection MySQL helpers that silently default to MySQL; \
         use the matching *_for_db_type API so MariaDB and future MySQL-family databases \
         keep concrete diagnostics and behavior: {:?}",
        offenders
    );
}

#[test]
fn non_test_ui_source_uses_backend_specs_instead_of_direct_database_type_dispatch() {
    let manifest_dir = Path::new(env!("CARGO_MANIFEST_DIR"));
    let ui_root = manifest_dir.join("src").join("ui");

    let mut offenders = Vec::new();
    for file in collect_rust_files(&ui_root) {
        if is_test_source_file(&file) {
            continue;
        }

        let content = match fs::read_to_string(&file) {
            Ok(content) => content,
            Err(err) => panic!("failed to read source file {}: {err}", file.display()),
        };
        let relative_path = match file.strip_prefix(manifest_dir) {
            Ok(path) => path.display().to_string(),
            Err(err) => panic!("failed to relativize {}: {err}", file.display()),
        };

        offenders.extend(collect_ui_database_type_dispatch_offenders(
            &content,
            &relative_path,
        ));
    }

    assert!(
        offenders.is_empty(),
        "found direct DatabaseType dispatch in non-test UI source files; \
         route UI behavior through DatabaseType backend specs/registries so a new \
         database type must define its own UI behavior: {:?}",
        offenders
    );
}

#[test]
fn non_test_non_ui_source_dispatches_database_type_with_exhaustive_match_only() {
    let manifest_dir = Path::new(env!("CARGO_MANIFEST_DIR"));
    let src_root = manifest_dir.join("src");
    let ui_root = src_root.join("ui");

    let mut offenders = Vec::new();
    for file in collect_rust_files(&src_root) {
        if file.starts_with(&ui_root) || is_test_source_file(&file) {
            continue;
        }

        let content = match fs::read_to_string(&file) {
            Ok(content) => content,
            Err(err) => panic!("failed to read source file {}: {err}", file.display()),
        };
        let relative_path = match file.strip_prefix(manifest_dir) {
            Ok(path) => path.display().to_string(),
            Err(err) => panic!("failed to relativize {}: {err}", file.display()),
        };

        offenders.extend(collect_db_database_type_dispatch_offenders(
            &content,
            &relative_path,
        ));
    }

    assert!(
        offenders.is_empty(),
        "found non-exhaustive DatabaseType dispatch in non-test non-UI source files; \
         use an exhaustive `match db_type {{ ... }}` without wildcard arms so \
         adding a new database type forces every concrete semantic decision to \
         be reviewed (UI code is held to the stricter backend-spec/registry rule): {:?}",
        offenders
    );
}

#[test]
fn non_test_source_does_not_alias_or_variant_import_dispatch_enums() {
    let manifest_dir = Path::new(env!("CARGO_MANIFEST_DIR"));
    let src_root = manifest_dir.join("src");

    let mut offenders = Vec::new();
    for file in collect_rust_files(&src_root) {
        if is_test_source_file(&file) {
            continue;
        }

        let content = match fs::read_to_string(&file) {
            Ok(content) => content,
            Err(err) => panic!("failed to read source file {}: {err}", file.display()),
        };
        let relative_path = match file.strip_prefix(manifest_dir) {
            Ok(path) => path.display().to_string(),
            Err(err) => panic!("failed to relativize {}: {err}", file.display()),
        };

        offenders.extend(collect_dispatch_enum_alias_import_offenders(
            &content,
            &relative_path,
        ));
    }

    assert!(
        offenders.is_empty(),
        "found alias/variant imports of dispatch enums in non-test source files; \
         the dispatch guards recognize decision sites by the enum names, so \
         `use ... as`, `type ... =`, and `use DatabaseType::...` imports would \
         let new-variant fallthrough bypass every guard: {:?}",
        offenders
    );
}

#[test]
fn non_test_source_does_not_use_self_patterns_inside_dispatch_enum_impls() {
    let manifest_dir = Path::new(env!("CARGO_MANIFEST_DIR"));
    let src_root = manifest_dir.join("src");

    let mut offenders = Vec::new();
    for file in collect_rust_files(&src_root) {
        if is_test_source_file(&file) {
            continue;
        }

        let content = match fs::read_to_string(&file) {
            Ok(content) => content,
            Err(err) => panic!("failed to read source file {}: {err}", file.display()),
        };
        let relative_path = match file.strip_prefix(manifest_dir) {
            Ok(path) => path.display().to_string(),
            Err(err) => panic!("failed to relativize {}: {err}", file.display()),
        };

        offenders.extend(collect_dispatch_enum_impl_self_pattern_offenders(
            &content,
            &relative_path,
        ));
    }

    assert!(
        offenders.is_empty(),
        "found `Self::` variant patterns inside dispatch enum impls in non-test source; \
         spell out the full enum name (`DatabaseType::Oracle`, `DbPoolSession::MySQL`) \
         so the text-based dispatch guards keep seeing every decision site: {:?}",
        offenders
    );
}

#[test]
fn database_type_cache_keys_are_unique_and_round_trip() {
    use space_query::db::DatabaseType;

    let mut seen: Vec<(u8, DatabaseType)> = Vec::new();
    for db_type in DatabaseType::ALL {
        let key = db_type.cache_key();
        if let Some((_, previous)) = seen.iter().find(|(existing, _)| *existing == key) {
            panic!(
                "cache_key {key} is shared by {previous:?} and {db_type:?}; \
                 per-DB intellisense caches and persisted db-type slots would collide, \
                 so every DatabaseType variant must declare a distinct cache_key"
            );
        }
        seen.push((key, db_type));
        assert_eq!(
            DatabaseType::from_cache_key(key),
            db_type,
            "from_cache_key must round-trip every DatabaseType variant"
        );
    }
}

#[test]
fn database_type_all_lists_every_database_type_variant_once() {
    let manifest_dir = Path::new(env!("CARGO_MANIFEST_DIR"));
    let path = manifest_dir.join("src").join("db").join("connection.rs");
    let content = match fs::read_to_string(&path) {
        Ok(content) => content,
        Err(err) => panic!("failed to read source file {}: {err}", path.display()),
    };

    let relative_path = "src/db/connection.rs";
    let variants = database_type_enum_variants(&content, relative_path);
    let all_variants = database_type_all_variants(&content, relative_path);

    assert_eq!(
        all_variants, variants,
        "DatabaseType::ALL must list every DatabaseType variant exactly once in enum order"
    );
}

#[test]
fn non_test_source_matches_physical_session_enums_without_wildcard_arms() {
    let manifest_dir = Path::new(env!("CARGO_MANIFEST_DIR"));
    let src_root = manifest_dir.join("src");

    let mut offenders = Vec::new();
    for file in collect_rust_files(&src_root) {
        if is_test_source_file(&file) {
            continue;
        }

        let content = match fs::read_to_string(&file) {
            Ok(content) => content,
            Err(err) => panic!("failed to read source file {}: {err}", file.display()),
        };
        let relative_path = match file.strip_prefix(manifest_dir) {
            Ok(path) => path.display().to_string(),
            Err(err) => panic!("failed to relativize {}: {err}", file.display()),
        };

        offenders.extend(collect_physical_session_enum_dispatch_offenders(
            &content,
            &relative_path,
        ));
    }

    assert!(
        offenders.is_empty(),
        "found wildcard physical DB session enum dispatch in non-test source files; \
         use exhaustive `match` arms for DbConnection/DbConnectionPool/DbPoolSession/DbSessionLease \
         so adding a new physical session variant forces every lifecycle decision to be reviewed: {:?}",
        offenders
    );
}

#[test]
fn backend_traits_with_required_policy_have_no_default_method_bodies() {
    let manifest_dir = Path::new(env!("CARGO_MANIFEST_DIR"));
    let trait_files = [
        (
            "src/db/query/execution_backend.rs",
            &["DbExecutionBackend"][..],
        ),
        (
            "src/ui/sql_editor/execution.rs",
            &["ExecutionWorkerBackend"][..],
        ),
        (
            "src/ui/sql_editor/mod.rs",
            &["ExplainPlanBackend", "TransactionActionBackend"][..],
        ),
        ("src/ui/main_window.rs", &["SchemaMetadataLoader"][..]),
        (
            "src/ui/sql_editor/intellisense/popup.rs",
            &["QuickDescribeBackend", "SignatureBackend"][..],
        ),
        (
            "src/ui/sql_editor/intellisense/helpers.rs",
            &["ColumnLoadBackend"][..],
        ),
        ("src/ui/object_browser.rs", &["ObjectBrowserDbBehavior"][..]),
    ];

    let mut offenders = Vec::new();
    for (relative_path, trait_names) in trait_files {
        let path = manifest_dir.join(relative_path);
        let content = match fs::read_to_string(&path) {
            Ok(content) => content,
            Err(err) => panic!("failed to read source file {}: {err}", path.display()),
        };
        offenders.extend(collect_backend_trait_default_method_offenders(
            &content,
            relative_path,
            trait_names,
        ));
    }

    assert!(
        offenders.is_empty(),
        "found default method bodies on backend policy traits; each backend must \
         state execution, session, transaction, metadata, and UI behavior explicitly \
         so a new DB kind cannot inherit a silent fallback: {:?}",
        offenders
    );
}

#[test]
fn core_backend_traits_allow_only_documented_derived_default_methods() {
    let manifest_dir = Path::new(env!("CARGO_MANIFEST_DIR"));
    let checks: &[(&str, TraitDefaultMethodAllowlist)] = &[
        (
            "src/db/connection.rs",
            &[(
                "DbBackend",
                &[
                    "choice_label",
                    "validate_session_time_zone",
                    "metadata_refresh_activity",
                    "metadata_refresh_activity_with_base",
                    "scope_switch_activity_message",
                    "scope_switch_failure_message",
                    "normalize_ssl_mode",
                ],
            )],
        ),
        (
            "src/db/transaction.rs",
            &[(
                "StatementSessionPostProcessor",
                &[
                    "may_need_preservation_after_statement",
                    "requires_transaction_decision_after_statement",
                ],
            )],
        ),
    ];

    let mut offenders = Vec::new();
    for (relative_path, allowed_defaults) in checks {
        let path = manifest_dir.join(relative_path);
        let content = match fs::read_to_string(&path) {
            Ok(content) => content,
            Err(err) => panic!("failed to read source file {}: {err}", path.display()),
        };
        offenders.extend(collect_backend_trait_unapproved_default_method_offenders(
            &content,
            relative_path,
            allowed_defaults,
        ));
    }

    assert!(
        offenders.is_empty(),
        "found unapproved default method bodies on core backend traits; only \
         documented derived helpers may have defaults, while DB/session policy \
         decisions must be implemented by each backend: {:?}",
        offenders
    );
}

const BACKEND_REGISTRY_FILES: &[(&str, &[&str])] = &[
    ("src/db/connection.rs", &["backend_for"]),
    (
        "src/db/query/execution_backend.rs",
        &["db_execution_backend_for"],
    ),
    (
        "src/db/transaction.rs",
        &["statement_session_post_processor_for"],
    ),
    (
        "src/ui/sql_editor/execution.rs",
        &["execution_worker_backend_for"],
    ),
    (
        "src/ui/sql_editor/mod.rs",
        &["explain_plan_backend_for", "transaction_action_backend_for"],
    ),
    ("src/ui/main_window.rs", &["schema_metadata_loader_for"]),
    (
        "src/ui/sql_editor/intellisense/popup.rs",
        &["quick_describe_backend_for", "signature_backend_for"],
    ),
    (
        "src/ui/sql_editor/intellisense/helpers.rs",
        &["column_load_backend_for"],
    ),
    ("src/ui/object_browser.rs", &["object_browser_behavior_for"]),
    ("src/ui/intellisense.rs", &["language_catalog_for_db_type"]),
    (
        "src/ui/syntax_highlight.rs",
        &[
            "function_catalog_for_db_type",
            "mysql_compatible_highlight_mode",
        ],
    ),
];

#[test]
fn ui_registry_allowlist_functions_are_all_required_registry_functions() {
    for name in UI_DATABASE_TYPE_REGISTRY_FUNCTIONS {
        let required = BACKEND_REGISTRY_FILES
            .iter()
            .any(|(_, functions)| functions.contains(name));
        assert!(
            required,
            "UI DatabaseType registry allowlist function `{name}` is missing from \
             BACKEND_REGISTRY_FILES; every allowlisted registry must also be required \
             to exist and dispatch on concrete DatabaseType, otherwise deleting or \
             family-shortcutting it would go unnoticed"
        );
    }
}

#[test]
fn backend_registry_functions_dispatch_on_concrete_database_type() {
    let manifest_dir = Path::new(env!("CARGO_MANIFEST_DIR"));

    let mut offenders = Vec::new();
    for (relative_path, function_names) in BACKEND_REGISTRY_FILES {
        let path = manifest_dir.join(relative_path);
        let content = match fs::read_to_string(&path) {
            Ok(content) => content,
            Err(err) => panic!("failed to read source file {}: {err}", path.display()),
        };
        offenders.extend(collect_registry_function_dispatch_offenders(
            &content,
            relative_path,
            function_names,
        ));
    }

    assert!(
        offenders.is_empty(),
        "found backend registry functions that do not dispatch on concrete \
         DatabaseType; registries must use exhaustive DatabaseType matches so \
         same-family DB variants still force an explicit review: {:?}",
        offenders
    );
}

#[test]
fn session_policy_and_ui_do_not_use_backend_kind_family_shortcuts() {
    let manifest_dir = Path::new(env!("CARGO_MANIFEST_DIR"));
    let paths = [
        manifest_dir
            .join("src")
            .join("db")
            .join("session_policy.rs"),
        manifest_dir.join("src").join("ui"),
    ];

    let mut offenders = Vec::new();
    for path in paths {
        let files = if path.is_dir() {
            collect_rust_files(&path)
        } else {
            vec![path]
        };

        for file in files {
            if is_test_source_file(&file) {
                continue;
            }
            let content = match fs::read_to_string(&file) {
                Ok(content) => content,
                Err(err) => panic!("failed to read source file {}: {err}", file.display()),
            };
            if !content.contains("backend_kind()") {
                continue;
            }
            let relative_path = match file.strip_prefix(manifest_dir) {
                Ok(path) => path.display().to_string(),
                Err(err) => panic!("failed to relativize {}: {err}", file.display()),
            };
            offenders.push(relative_path);
        }
    }

    assert!(
        offenders.is_empty(),
        "found backend_kind() family shortcuts in session policy or UI code; \
         use DatabaseType exhaustive registry dispatch or DbBackend policy methods \
         so same-family database variants still force review: {:?}",
        offenders
    );
}

#[test]
fn result_message_policy_does_not_use_backend_kind_family_shortcuts() {
    let manifest_dir = Path::new(env!("CARGO_MANIFEST_DIR"));
    let path = manifest_dir
        .join("src")
        .join("db")
        .join("query")
        .join("types.rs");
    let content = match fs::read_to_string(&path) {
        Ok(content) => content,
        Err(err) => panic!("failed to read source file {}: {err}", path.display()),
    };

    assert!(
        !content.contains("backend_kind()") && !content.contains("DatabaseBackendKind"),
        "result message policy must match concrete DatabaseType variants so a new database type \
         has to review user-facing transaction feedback instead of inheriting a backend-family \
         default"
    );
}

#[test]
fn query_execution_policy_does_not_use_backend_kind_family_shortcuts() {
    let manifest_dir = Path::new(env!("CARGO_MANIFEST_DIR"));
    let path = manifest_dir
        .join("src")
        .join("db")
        .join("query")
        .join("execution_backend.rs");
    let content = match fs::read_to_string(&path) {
        Ok(content) => content,
        Err(err) => panic!("failed to read source file {}: {err}", path.display()),
    };

    assert!(
        !content.contains("backend_kind()") && !content.contains("DatabaseBackendKind"),
        "query execution profile and timeout policy must match concrete DatabaseType variants so \
         a new database type has to review result routing and timeout wrapping behavior"
    );
}

#[test]
fn sql_classification_policy_does_not_use_backend_kind_family_shortcuts() {
    let manifest_dir = Path::new(env!("CARGO_MANIFEST_DIR"));
    let path = manifest_dir
        .join("src")
        .join("db")
        .join("sql_classification.rs");
    let content = match fs::read_to_string(&path) {
        Ok(content) => content,
        Err(err) => panic!("failed to read source file {}: {err}", path.display()),
    };

    assert!(
        !content.contains("backend_kind()") && !content.contains("DatabaseBackendKind"),
        "SQL classification feeds session reuse, cancel, and transaction safety policy; \
         match concrete DatabaseType variants so adding a new database type forces review"
    );
}

#[test]
fn sql_text_policy_does_not_use_sql_dialect_family_shortcuts() {
    let manifest_dir = Path::new(env!("CARGO_MANIFEST_DIR"));
    let path = manifest_dir.join("src").join("sql_text.rs");
    let content = match fs::read_to_string(&path) {
        Ok(content) => content,
        Err(err) => panic!("failed to read source file {}: {err}", path.display()),
    };

    assert!(
        !content.contains("sql_dialect()") && !content.contains("SqlDialect"),
        "shared SQL text policy must match concrete DatabaseType variants so keyword, comment, \
         and compatibility behavior cannot silently inherit a dialect-family default"
    );
}

#[test]
fn connection_mysql_session_transaction_helpers_do_not_use_backend_kind_family_shortcuts() {
    let manifest_dir = Path::new(env!("CARGO_MANIFEST_DIR"));
    let path = manifest_dir.join("src").join("db").join("connection.rs");
    let content = match fs::read_to_string(&path) {
        Ok(content) => content,
        Err(err) => panic!("failed to read source file {}: {err}", path.display()),
    };
    let relative_path = "src/db/connection.rs";

    for signature in [
        "fn ensure_connected_mysql_family(&self) -> Result<(), String>",
        "pub(crate) fn transaction_mode_statements_for_with_default(",
    ] {
        let body = rust_function_body_by_signature(&content, signature, relative_path);
        assert!(
            !body.contains("backend_kind()") && !body.contains("DatabaseBackendKind"),
            "{signature} must match concrete DatabaseType variants so MySQL/MariaDB session and \
             transaction behavior cannot silently apply to a newly added backend-family variant"
        );
    }
}

#[test]
fn ui_language_surfaces_do_not_use_sql_dialect_family_shortcuts() {
    let manifest_dir = Path::new(env!("CARGO_MANIFEST_DIR"));
    let paths = [
        manifest_dir.join("src").join("ui").join("intellisense.rs"),
        manifest_dir
            .join("src")
            .join("ui")
            .join("syntax_highlight.rs"),
    ];

    let mut offenders = Vec::new();
    for path in paths {
        let content = match fs::read_to_string(&path) {
            Ok(content) => content,
            Err(err) => panic!("failed to read source file {}: {err}", path.display()),
        };
        if content.contains("sql_dialect()") || content.contains("SqlDialect") {
            let relative_path = match path.strip_prefix(manifest_dir) {
                Ok(path) => path.display().to_string(),
                Err(err) => panic!("failed to relativize {}: {err}", path.display()),
            };
            offenders.push(relative_path);
        }
    }

    assert!(
        offenders.is_empty(),
        "UI language catalog and syntax highlight policy must match concrete DatabaseType variants \
         so adding a new database type forces review instead of inheriting a dialect-family default: {:?}",
        offenders
    );
}

#[test]
fn object_browser_mysql_family_actions_keep_concrete_db_type() {
    let manifest_dir = Path::new(env!("CARGO_MANIFEST_DIR"));
    let path = manifest_dir
        .join("src")
        .join("ui")
        .join("object_browser.rs");
    let content = match fs::read_to_string(&path) {
        Ok(content) => content,
        Err(err) => panic!("failed to read source file {}: {err}", path.display()),
    };
    let body = rust_function_body_by_signature(
        &content,
        "fn take_object_action_session(",
        "src/ui/object_browser.rs",
    );
    let metadata_session_body = rust_function_body_by_signature(
        &content,
        "fn acquire_mysql_metadata_session(",
        "src/ui/object_browser.rs",
    );
    let mysql_behavior_impl = content
        .split("impl ObjectBrowserDbBehavior for MysqlObjectBrowserBehavior")
        .nth(1)
        .expect("failed to find MySQL object-browser behavior impl");
    let metadata_cache_body = rust_function_body_by_signature(
        mysql_behavior_impl,
        "fn load_metadata_cache(",
        "src/ui/object_browser.rs",
    );

    assert!(
        body.contains("context.connection_info.db_type")
            && body.contains("db_type.is_same_type_as(expected_db_type)")
            && body.contains("expected_db_type.display_name()"),
        "MySQL/MariaDB object-browser actions must validate the concrete pool session DB type \
         against the active object-browser context and use the concrete display name"
    );
    assert!(
        metadata_session_body.contains("context.connection_info.db_type")
            && metadata_session_body.contains("db_type.is_same_type_as(expected_db_type)")
            && metadata_session_body.contains("expected_db_type.display_name()")
            && metadata_cache_body.contains("session_db_type.is_same_type_as(db_type)")
            && metadata_cache_body.contains("db_type.display_name()"),
        "MySQL/MariaDB object-browser metadata loading must validate concrete pool session DB \
         types and use the active context display name"
    );
    assert!(
        !content.contains("Expected MySQL object action session"),
        "object-browser MySQL-family action errors must not be hard-coded to MySQL; \
         MariaDB actions should report MariaDB and future same-family DB types must be reviewed"
    );
    assert!(
        !content.contains("expected MySQL object-browser metadata session"),
        "object-browser MySQL-family metadata warnings must not be hard-coded to MySQL"
    );
}

#[test]
fn shared_result_messages_are_emitted_through_result_messages_module() {
    let manifest_dir = Path::new(env!("CARGO_MANIFEST_DIR"));
    let src_root = manifest_dir.join("src");

    let mut offenders = Vec::new();
    for file in collect_rust_files(&src_root) {
        if is_test_source_file(&file) {
            continue;
        }

        let relative_path = match file.strip_prefix(manifest_dir) {
            Ok(path) => path.display().to_string(),
            Err(err) => panic!("failed to relativize {}: {err}", file.display()),
        };
        if relative_path == "src/db/query/types.rs" {
            continue;
        }

        let content = match fs::read_to_string(&file) {
            Ok(content) => content,
            Err(err) => panic!("failed to read source file {}: {err}", file.display()),
        };

        offenders.extend(collect_shared_result_message_literal_offenders(
            &content,
            &relative_path,
        ));
    }

    assert!(
        offenders.is_empty(),
        "found shared result message literals outside result_messages; \
         use crate::db::query::result_messages so all backends and UI status \
         checks stay consistent: {:?}",
        offenders
    );
}

#[test]
fn ui_source_does_not_hardcode_driver_error_markers() {
    let manifest_dir = Path::new(env!("CARGO_MANIFEST_DIR"));
    let ui_root = manifest_dir.join("src/ui");

    let mut offenders = Vec::new();
    for file in collect_rust_files(&ui_root) {
        if is_test_source_file(&file) {
            continue;
        }

        let relative_path = match file.strip_prefix(manifest_dir) {
            Ok(path) => path.display().to_string(),
            Err(err) => panic!("failed to relativize {}: {err}", file.display()),
        };
        // The per-backend execution worker implementations live here; their
        // backend-specific error handling legitimately names driver markers.
        if relative_path == "src/ui/sql_editor/execution.rs" {
            continue;
        }

        let content = match fs::read_to_string(&file) {
            Ok(content) => content,
            Err(err) => panic!("failed to read source file {}: {err}", file.display()),
        };

        offenders.extend(collect_driver_error_marker_literal_offenders(
            &content,
            &relative_path,
        ));
    }

    assert!(
        offenders.is_empty(),
        "found driver-specific error markers hardcoded in generic UI; \
         delegate message classification to the per-DB catalogs in \
         db::session_policy (message_indicates_query_cancel, \
         message_indicates_connection_loss, error_line_message_patterns) so a \
         new database type cannot be silently unclassified: {:?}",
        offenders
    );
}

#[test]
fn guard_detects_driver_error_marker_literal() {
    let offenders = collect_driver_error_marker_literal_offenders(
        r#"
        fn is_cancel(message: &str) -> bool {
            message.to_ascii_lowercase().contains("ora-01013")
        }
        "#,
        "snippet.rs",
    );
    assert_eq!(offenders.len(), 1, "offenders: {:?}", offenders);
}

#[test]
fn guard_allows_driver_error_marker_literal_in_test_only_items() {
    let offenders = collect_driver_error_marker_literal_offenders(
        r#"
        #[cfg(test)]
        mod tests {
            #[test]
            fn classifies_cancel() {
                assert!(is_cancel("ORA-01013: user requested cancel"));
            }
        }
        "#,
        "snippet.rs",
    );
    assert!(offenders.is_empty(), "offenders: {:?}", offenders);
}

#[test]
fn guard_detects_equality_comparison_on_backend_kind() {
    let offenders = collect_backend_kind_dispatch_offenders(
        r#"
        fn f(db_type: DatabaseType) -> bool {
            db_type.backend_kind() == DatabaseBackendKind::MySql
        }
        "#,
        "snippet.rs",
    );
    assert_eq!(offenders.len(), 1, "offenders: {:?}", offenders);
}

#[test]
fn guard_detects_negated_comparison_on_backend_kind() {
    let offenders = collect_backend_kind_dispatch_offenders(
        r#"
        fn f(db_type: DatabaseType) -> bool {
            db_type.backend_kind() != DatabaseBackendKind::Oracle
        }
        "#,
        "snippet.rs",
    );
    assert_eq!(offenders.len(), 1, "offenders: {:?}", offenders);
}

#[test]
fn guard_detects_matches_macro_on_backend_kind() {
    let offenders = collect_backend_kind_dispatch_offenders(
        r#"
        fn f(db_type: DatabaseType) -> bool {
            matches!(db_type.backend_kind(), DatabaseBackendKind::MySql)
        }
        "#,
        "snippet.rs",
    );
    assert_eq!(offenders.len(), 1, "offenders: {:?}", offenders);
}

#[test]
fn guard_detects_if_let_on_backend_kind() {
    let offenders = collect_backend_kind_dispatch_offenders(
        r#"
        fn f(db_type: DatabaseType) -> bool {
            if let DatabaseBackendKind::MySql = db_type.backend_kind() {
                return true;
            }
            false
        }
        "#,
        "snippet.rs",
    );
    assert_eq!(offenders.len(), 1, "offenders: {:?}", offenders);
}

#[test]
fn guard_detects_equality_comparison_on_sql_dialect() {
    let offenders = collect_backend_kind_dispatch_offenders(
        r#"
        fn f(db_type: DatabaseType) -> bool {
            db_type.sql_dialect() == SqlDialect::MySql
        }
        "#,
        "snippet.rs",
    );
    assert_eq!(offenders.len(), 1, "offenders: {:?}", offenders);
}

#[test]
fn guard_detects_sql_dialect_to_concrete_database_type_mapping() {
    let offenders = collect_dialect_to_concrete_database_type_offenders(
        r#"
        fn f(dialect: SqlDialect) -> DatabaseType {
            match dialect {
                SqlDialect::Oracle => DatabaseType::Oracle,
                SqlDialect::MySql => DatabaseType::MySQL,
            }
        }
        "#,
        "snippet.rs",
    );
    assert_eq!(offenders.len(), 2, "offenders: {:?}", offenders);
}

#[test]
fn guard_detects_mysql_executor_default_method_call() {
    let offenders = collect_mysql_executor_default_method_offenders(
        r#"
        fn f(conn: &mut mysql::PooledConn, sql: &str) {
            let _ = crate::db::query::mysql_executor::MysqlExecutor::execute(conn, sql);
            let _ = MysqlExecutor::is_use_statement(sql);
        }
        "#,
        "snippet.rs",
    );
    assert_eq!(offenders.len(), 2, "offenders: {:?}", offenders);
}

#[test]
fn guard_detects_wildcard_arm_in_backend_kind_match() {
    let offenders = collect_backend_kind_dispatch_offenders(
        r#"
        fn f(db_type: DatabaseType) -> bool {
            match db_type.backend_kind() {
                DatabaseBackendKind::MySql => true,
                _ => false,
            }
        }
        "#,
        "snippet.rs",
    );
    assert_eq!(offenders.len(), 1, "offenders: {:?}", offenders);
}

#[test]
fn guard_allows_exhaustive_backend_kind_match() {
    let offenders = collect_backend_kind_dispatch_offenders(
        r#"
        fn f(db_type: DatabaseType) -> bool {
            match db_type.backend_kind() {
                DatabaseBackendKind::MySql => true,
                DatabaseBackendKind::Oracle => false,
            }
        }
        "#,
        "snippet.rs",
    );
    assert!(offenders.is_empty(), "offenders: {:?}", offenders);
}

#[test]
fn message_guard_detects_shared_result_message_literal() {
    let offenders = collect_shared_result_message_literal_offenders(
        r#"
        fn f() -> &'static str {
            "Commit complete"
        }
        "#,
        "src/db/query/snippet.rs",
    );
    assert_eq!(offenders.len(), 1, "offenders: {:?}", offenders);
}

#[test]
fn backend_trait_default_guard_detects_default_method_body() {
    let offenders = collect_backend_trait_default_method_offenders(
        r#"
        trait ExecutionWorkerBackend {
            fn begin_execution(&self) {}
        }
        "#,
        "src/ui/sql_editor/execution.rs",
        &["ExecutionWorkerBackend"],
    );
    assert_eq!(offenders.len(), 1, "offenders: {:?}", offenders);
}

#[test]
fn backend_trait_default_allowlist_guard_detects_unapproved_default_method_body() {
    let offenders = collect_backend_trait_unapproved_default_method_offenders(
        r#"
        trait DbBackend {
            fn choice_label(&self) -> &'static str {
                "db"
            }

            fn apply_auto_commit(&self) {}
        }
        "#,
        "src/db/connection.rs",
        &[("DbBackend", &["choice_label"])],
    );
    assert_eq!(offenders.len(), 1, "offenders: {:?}", offenders);
}

#[test]
fn registry_function_guard_detects_backend_kind_dispatch() {
    let offenders = collect_registry_function_dispatch_offenders(
        r#"
        fn execution_worker_backend_for(db_type: DatabaseType) -> &'static dyn ExecutionWorkerBackend {
            match db_type.backend_kind() {
                DatabaseBackendKind::Oracle => &ORACLE_EXECUTION_WORKER_BACKEND,
                DatabaseBackendKind::MySql => &MYSQL_EXECUTION_WORKER_BACKEND,
            }
        }
        "#,
        "src/ui/sql_editor/execution.rs",
        &["execution_worker_backend_for"],
    );
    assert!(
        offenders
            .iter()
            .any(|offender| offender.pattern.contains("backend_kind")),
        "offenders: {:?}",
        offenders
    );
}

#[test]
fn ui_guard_detects_equality_comparison_on_database_type() {
    let offenders = collect_ui_database_type_dispatch_offenders(
        r#"
        fn f(db_type: DatabaseType) -> bool {
            db_type == DatabaseType::Oracle
        }
        "#,
        "src/ui/snippet.rs",
    );
    assert_eq!(offenders.len(), 1, "offenders: {:?}", offenders);
}

#[test]
fn ui_guard_detects_method_comparison_on_concrete_database_type() {
    let offenders = collect_ui_database_type_dispatch_offenders(
        r#"
        fn f(db_type: DatabaseType) -> bool {
            db_type.is_same_type_as(DatabaseType::Oracle)
        }
        "#,
        "src/ui/snippet.rs",
    );
    assert_eq!(offenders.len(), 1, "offenders: {:?}", offenders);
}

#[test]
fn db_guard_detects_equality_comparison_on_database_type() {
    let offenders = collect_db_database_type_dispatch_offenders(
        r#"
        fn f(db_type: DatabaseType) -> bool {
            db_type == DatabaseType::MariaDB
        }
        "#,
        "src/db/snippet.rs",
    );
    assert_eq!(offenders.len(), 1, "offenders: {:?}", offenders);
}

#[test]
fn db_guard_detects_method_comparison_on_concrete_database_type() {
    let offenders = collect_db_database_type_dispatch_offenders(
        r#"
        fn f(db_type: DatabaseType) -> bool {
            db_type.is_same_type_as(DatabaseType::MariaDB)
        }
        "#,
        "src/db/snippet.rs",
    );
    assert_eq!(offenders.len(), 1, "offenders: {:?}", offenders);
}

#[test]
fn db_guard_detects_wildcard_arm_in_database_type_match() {
    let offenders = collect_db_database_type_dispatch_offenders(
        r#"
        fn f(db_type: DatabaseType) -> bool {
            match db_type {
                DatabaseType::MariaDB => true,
                _ => false,
            }
        }
        "#,
        "src/db/snippet.rs",
    );
    assert_eq!(offenders.len(), 1, "offenders: {:?}", offenders);
}

#[test]
fn db_guard_allows_exhaustive_database_type_match() {
    let offenders = collect_db_database_type_dispatch_offenders(
        r#"
        fn f(db_type: DatabaseType) -> bool {
            match db_type {
                DatabaseType::MariaDB => true,
                DatabaseType::MySQL | DatabaseType::Oracle => false,
            }
        }
        "#,
        "src/db/snippet.rs",
    );
    assert!(offenders.is_empty(), "offenders: {:?}", offenders);
}

#[test]
fn db_guard_detects_nested_wildcard_arm_in_option_database_type_match() {
    let offenders = collect_db_database_type_dispatch_offenders(
        r#"
        fn f(db_type: Option<DatabaseType>) -> bool {
            match db_type {
                Some(DatabaseType::Oracle) => true,
                Some(_) => false,
                None => false,
            }
        }
        "#,
        "src/db/snippet.rs",
    );
    assert_eq!(offenders.len(), 1, "offenders: {:?}", offenders);
}

#[test]
fn db_guard_detects_nested_wildcard_arm_in_tuple_database_type_match() {
    let offenders = collect_db_database_type_dispatch_offenders(
        r#"
        fn f(db_type: DatabaseType, flag: bool) -> bool {
            match (db_type, flag) {
                (DatabaseType::Oracle, _) => true,
                (_, true) => false,
                (other, false) => other.is_relational(),
            }
        }
        "#,
        "src/db/snippet.rs",
    );
    assert_eq!(offenders.len(), 2, "offenders: {:?}", offenders);
}

#[test]
fn db_guard_detects_let_else_on_database_type() {
    let offenders = collect_db_database_type_dispatch_offenders(
        r#"
        fn f(db_type: DatabaseType) -> bool {
            let DatabaseType::Oracle = db_type else {
                return false;
            };
            true
        }
        "#,
        "src/db/snippet.rs",
    );
    assert_eq!(offenders.len(), 1, "offenders: {:?}", offenders);
}

#[test]
fn db_guard_allows_plain_let_with_database_type_annotation() {
    let offenders = collect_db_database_type_dispatch_offenders(
        r#"
        fn f(info: &ConnectionInfo) -> DatabaseType {
            let db_type: DatabaseType = info.db_type;
            db_type
        }
        "#,
        "src/db/snippet.rs",
    );
    assert!(offenders.is_empty(), "offenders: {:?}", offenders);
}

#[test]
fn alias_guard_detects_alias_variant_and_type_alias_imports() {
    let offenders = collect_dispatch_enum_alias_import_offenders(
        r#"
use crate::db::connection::DatabaseType as Dt;
use crate::db::connection::DatabaseType::Oracle;
use crate::db::connection::{
    DatabaseConnection,
    DatabaseBackendKind as Kind,
};
use crate::db::DbPoolSession::*;
pub(crate) type Kind2 = crate::db::connection::DatabaseType;
"#,
        "src/snippet.rs",
    );
    assert_eq!(offenders.len(), 5, "offenders: {:?}", offenders);
}

#[test]
fn alias_guard_allows_plain_imports_and_generic_aliases() {
    let offenders = collect_dispatch_enum_alias_import_offenders(
        r#"
use crate::db::connection::DatabaseType;
use crate::db::connection::DatabaseConnection;
use crate::db::connection::*;
type SharedDbTypes = Vec<DatabaseType>;

fn f() {
    use crate::db::connection::DatabaseType;
    let _ = DatabaseType::default();
}
"#,
        "src/snippet.rs",
    );
    assert!(offenders.is_empty(), "offenders: {:?}", offenders);
}

#[test]
fn self_pattern_guard_detects_self_patterns_inside_dispatch_enum_impl() {
    let offenders = collect_dispatch_enum_impl_self_pattern_offenders(
        r#"
        impl DatabaseType {
            fn is_oracle(self) -> bool {
                matches!(self, Self::Oracle)
            }

            fn family(self) -> u8 {
                match self {
                    Self::Oracle => 0,
                    Self::MySQL | Self::MariaDB => 1,
                }
            }
        }
        "#,
        "src/db/snippet.rs",
    );
    assert_eq!(offenders.len(), 3, "offenders: {:?}", offenders);
}

#[test]
fn self_pattern_guard_allows_full_enum_names_and_other_impls() {
    let offenders = collect_dispatch_enum_impl_self_pattern_offenders(
        r#"
        impl DatabaseType {
            fn family(self) -> u8 {
                match self {
                    DatabaseType::Oracle => 0,
                    DatabaseType::MySQL | DatabaseType::MariaDB => 1,
                }
            }
        }

        impl OracleDriverMode {
            fn is_thin(self) -> bool {
                matches!(self, Self::Thin)
            }
        }
        "#,
        "src/db/snippet.rs",
    );
    assert!(offenders.is_empty(), "offenders: {:?}", offenders);
}

#[test]
fn guard_detects_wildcard_arm_behind_intermediate_backend_kind_binding() {
    let offenders = collect_backend_kind_dispatch_offenders(
        r#"
        fn f(db_type: DatabaseType) -> bool {
            let kind = db_type.backend_kind();
            match kind {
                DatabaseBackendKind::MySql => true,
                _ => false,
            }
        }
        "#,
        "snippet.rs",
    );
    assert_eq!(offenders.len(), 1, "offenders: {:?}", offenders);
}

#[test]
fn db_guard_detects_rest_pattern_arm_in_option_database_type_match() {
    let offenders = collect_db_database_type_dispatch_offenders(
        r#"
        fn f(db_type: Option<DatabaseType>) -> bool {
            match db_type {
                Some(DatabaseType::Oracle) => true,
                Some(..) => false,
                None => false,
            }
        }
        "#,
        "src/db/snippet.rs",
    );
    assert_eq!(offenders.len(), 1, "offenders: {:?}", offenders);
}

#[test]
fn database_type_all_guard_detects_missing_variant() {
    let content = r#"
        pub enum DatabaseType {
            Oracle,
            MySQL,
            MariaDB,
        }

        impl DatabaseType {
            pub const ALL: [Self; 2] = [Self::Oracle, Self::MySQL];
        }
    "#;

    assert_ne!(
        database_type_all_variants(content, "snippet.rs"),
        database_type_enum_variants(content, "snippet.rs")
    );
}

#[test]
fn physical_session_guard_detects_wildcard_arm_in_session_enum_match() {
    let offenders = collect_physical_session_enum_dispatch_offenders(
        r#"
        fn f(session: DbPoolSession) -> bool {
            match session {
                DbPoolSession::Oracle(_) => true,
                _ => false,
            }
        }
        "#,
        "src/db/snippet.rs",
    );
    assert_eq!(offenders.len(), 1, "offenders: {:?}", offenders);
}

#[test]
fn physical_session_guard_detects_matches_macro_on_session_enum() {
    let offenders = collect_physical_session_enum_dispatch_offenders(
        r#"
        fn f(session: DbPoolSession) -> bool {
            matches!(session, DbPoolSession::Oracle(_))
        }
        "#,
        "src/db/snippet.rs",
    );
    assert_eq!(offenders.len(), 1, "offenders: {:?}", offenders);
}

#[test]
fn physical_session_guard_detects_if_let_on_session_enum() {
    let offenders = collect_physical_session_enum_dispatch_offenders(
        r#"
        fn f(session: DbPoolSession) -> bool {
            if let DbPoolSession::Oracle(_) = session {
                return true;
            }
            false
        }
        "#,
        "src/db/snippet.rs",
    );
    assert_eq!(offenders.len(), 1, "offenders: {:?}", offenders);
}

#[test]
fn physical_session_guard_detects_silent_nested_wildcard_arm() {
    let offenders = collect_physical_session_enum_dispatch_offenders(
        r#"
        fn f(result: Result<DbPoolSession, String>) -> bool {
            match result {
                Ok(DbPoolSession::Oracle(_)) => true,
                Ok(_) => false,
                Err(err) => {
                    log(err);
                    false
                }
            }
        }
        "#,
        "src/db/snippet.rs",
    );
    assert_eq!(offenders.len(), 1, "offenders: {:?}", offenders);
}

#[test]
fn physical_session_guard_allows_named_binding_mismatch_arm() {
    let offenders = collect_physical_session_enum_dispatch_offenders(
        r#"
        fn f(result: Result<DbPoolSession, String>) -> bool {
            match result {
                Ok(DbPoolSession::Oracle(_)) => true,
                Ok(other) => {
                    warn(other.db_type());
                    false
                }
                Err(err) => {
                    log(err);
                    false
                }
            }
        }
        "#,
        "src/db/snippet.rs",
    );
    assert!(offenders.is_empty(), "offenders: {:?}", offenders);
}

#[test]
fn physical_session_guard_allows_exhaustive_session_enum_match() {
    let offenders = collect_physical_session_enum_dispatch_offenders(
        r#"
        fn f(session: DbPoolSession) -> bool {
            match session {
                DbPoolSession::Oracle(_) => true,
                DbPoolSession::OracleThin(_) => true,
                DbPoolSession::MySQL { .. } => false,
            }
        }
        "#,
        "src/db/snippet.rs",
    );
    assert!(offenders.is_empty(), "offenders: {:?}", offenders);
}

#[test]
fn ui_guard_allows_database_type_identity_check_between_variables() {
    let offenders = collect_ui_database_type_dispatch_offenders(
        r#"
        fn f(left: DatabaseType, right: DatabaseType) -> bool {
            left.is_same_type_as(right)
        }
        "#,
        "src/ui/snippet.rs",
    );
    assert!(offenders.is_empty(), "offenders: {:?}", offenders);
}

#[test]
fn ui_guard_allows_backend_spec_lookup() {
    let offenders = collect_ui_database_type_dispatch_offenders(
        r#"
        fn f(db_type: DatabaseType) -> bool {
            db_type.connection_form_spec().show_driver_mode
        }
        "#,
        "src/ui/snippet.rs",
    );
    assert!(offenders.is_empty(), "offenders: {:?}", offenders);
}

#[test]
fn ui_guard_allows_exhaustive_database_type_backend_registry_match() {
    let offenders = collect_ui_database_type_dispatch_offenders(
        r#"
        fn execution_worker_backend_for(db_type: DatabaseType) -> &'static dyn ExecutionWorkerBackend {
            match db_type {
                DatabaseType::Oracle => &ORACLE_EXECUTION_WORKER_BACKEND,
                DatabaseType::MySQL => &MYSQL_EXECUTION_WORKER_BACKEND,
                DatabaseType::MariaDB => &MYSQL_EXECUTION_WORKER_BACKEND,
            }
        }
        "#,
        "src/ui/snippet.rs",
    );
    assert!(offenders.is_empty(), "offenders: {:?}", offenders);
}

#[test]
fn ui_guard_detects_grouped_database_type_backend_registry_match() {
    let offenders = collect_ui_database_type_dispatch_offenders(
        r#"
        fn execution_worker_backend_for(db_type: DatabaseType) -> &'static dyn ExecutionWorkerBackend {
            match db_type {
                DatabaseType::Oracle => &ORACLE_EXECUTION_WORKER_BACKEND,
                DatabaseType::MySQL | DatabaseType::MariaDB => &MYSQL_EXECUTION_WORKER_BACKEND,
            }
        }
        "#,
        "src/ui/snippet.rs",
    );
    assert_eq!(offenders.len(), 1, "offenders: {:?}", offenders);
}

#[test]
fn ui_guard_detects_wildcard_database_type_backend_registry_match() {
    let offenders = collect_ui_database_type_dispatch_offenders(
        r#"
        fn execution_worker_backend_for(db_type: DatabaseType) -> &'static dyn ExecutionWorkerBackend {
            match db_type {
                DatabaseType::Oracle => &ORACLE_EXECUTION_WORKER_BACKEND,
                _ => &MYSQL_EXECUTION_WORKER_BACKEND,
            }
        }
        "#,
        "src/ui/snippet.rs",
    );
    assert_eq!(offenders.len(), 1, "offenders: {:?}", offenders);
}

#[test]
fn guard_skips_test_only_code() {
    let offenders = collect_backend_kind_dispatch_offenders(
        r#"
        #[cfg(test)]
        mod tests {
            fn f(db_type: DatabaseType) -> bool {
                db_type.backend_kind() == DatabaseBackendKind::MySql
            }
        }
        "#,
        "snippet.rs",
    );
    assert!(offenders.is_empty(), "offenders: {:?}", offenders);
}

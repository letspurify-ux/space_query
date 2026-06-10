//! Guards the DB-backend dispatch convention: `DatabaseBackendKind` and
//! `SqlDialect` decisions in non-test source must use exhaustive `match`
//! expressions, never `==`/`!=` comparisons, `matches!`, `if let`, or wildcard
//! match arms. Equality-style checks compile silently when a new backend kind
//! or dialect is added and fall through to the wrong dialect family;
//! exhaustive matches turn every decision site into a compile error that
//! forces a per-site review.

use std::ffi::OsStr;
use std::fs;
use std::path::{Path, PathBuf};

use quote::ToTokens;
use syn::spanned::Spanned;
use syn::visit::{self, Visit};
use syn::{BinOp, Expr, ExprBinary, ExprLet, ExprMatch, ItemFn, ItemMod, Macro, Pat};

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

fn pattern_swallows_new_variants(pat: &Pat) -> bool {
    match pat {
        Pat::Wild(_) => true,
        // A bare lowercase binding (`kind => ...`) matches anything, exactly
        // like `_`, so it hides newly added backend kinds the same way.
        Pat::Ident(ident) => ident.subpat.is_none(),
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

    fn visit_expr_match(&mut self, node: &ExprMatch) {
        if mentions_backend_kind(&*node.expr) {
            for arm in &node.arms {
                if pattern_swallows_new_variants(&arm.pat) {
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

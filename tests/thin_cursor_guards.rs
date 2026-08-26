#![allow(
    clippy::cargo,
    clippy::expect_used,
    clippy::panic,
    clippy::pedantic,
    clippy::unwrap_used
)]

//! Guards the tns-thin cursor contract: a thin statement execution hands the
//! SERVER CURSOR back inside its result and closes nothing itself, so a call
//! that DROPS the result drops the cursor with it and the session keeps it
//! open until the session dies.
//!
//! The rule this file enforces is therefore about the RESULT, not about which
//! method was called: `OracleThinSession::execute_typed` and its siblings must
//! have their result BOUND, so the cursor in it can be closed
//! (`close_cursor_later` + a later flush). A call whose result is discarded —
//! `session.execute_typed(..)?;` as a statement, or `let _ = ...` — must use
//! `query_drop` instead, which is exactly `execute_typed` followed by
//! `close_cursor_later`.
//!
//! Why a guard and not a review note: the leak is invisible in every ordinary
//! way. It raises no error, shows nothing in `v$open_cursor` (the leaked
//! cursors carry no `sql_text`), and only surfaces as `ORA-01000` after enough
//! repetitions. It has now been shipped twice — the connect-time
//! `ALTER SESSION` (fixed 2026-06-10) and the F6 Explain Plan's
//! `EXPLAIN PLAN` on the connection's own, NON-POOLED session, where
//! `reset_before_reuse`'s sweep never runs.
//!
//! The OCI twin does not have the defect and cannot: `Connection::execute`
//! returns a `Statement` that closes when it goes out of scope. So a leak here
//! is also a divergence between the two Oracle drivers, which is the thing
//! this app's Oracle work exists to remove.

use std::fs;
use std::path::{Path, PathBuf};

use quote::ToTokens;
use syn::spanned::Spanned;
use syn::visit::{self, Visit};
use syn::{Expr, ExprMethodCall, ItemFn, ItemMod, Pat, Stmt};

/// Thin-session methods that return a result carrying an OPEN server cursor.
///
/// Every one of them is `execute_request` underneath, which allocates a cursor
/// server-side and — on the success path — leaves closing it to the caller.
/// The closing forms (`query_drop`, `execute_typed_fetch_all`,
/// `query_described_fetch_all_request*`) are deliberately absent: they close
/// what they opened, so discarding their result loses nothing.
const CURSOR_RETURNING_THIN_METHODS: &[&str] = &[
    "execute_typed",
    "execute_typed_with_implicit",
    "execute_request",
    "execute_request_without_prefetch",
    "execute_many",
];

#[allow(dead_code)]
#[derive(Debug)]
struct DiscardedThinCursor {
    path: String,
    line: usize,
    method: String,
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

fn attr_is_test_only(attr: &syn::Attribute) -> bool {
    if attr.path().is_ident("test") {
        return true;
    }

    if attr.path().is_ident("cfg") || attr.path().is_ident("cfg_attr") {
        return attr.meta.to_token_stream().to_string().contains("test");
    }

    false
}

fn attrs_are_test_only(attrs: &[syn::Attribute]) -> bool {
    attrs.iter().any(attr_is_test_only)
}

/// The cursor-returning call inside `expr`, if there is one.
///
/// The whole expression is searched rather than only its outermost call: the
/// discarding shape in the wild is a chain — `conn.execute_typed(..).map_err(..)?`
/// — where the cursor-returning call is several links down.
fn cursor_returning_call(expr: &Expr) -> Option<String> {
    struct Finder {
        found: Option<String>,
    }

    impl Visit<'_> for Finder {
        fn visit_expr_method_call(&mut self, node: &ExprMethodCall) {
            if self.found.is_none() {
                let method = node.method.to_string();
                if CURSOR_RETURNING_THIN_METHODS.contains(&method.as_str()) {
                    self.found = Some(method);
                }
            }
            visit::visit_expr_method_call(self, node);
        }
    }

    let mut finder = Finder { found: None };
    finder.visit_expr(expr);
    finder.found
}

struct DiscardedThinCursorVisitor<'a> {
    path: &'a str,
    offenders: Vec<DiscardedThinCursor>,
}

impl DiscardedThinCursorVisitor<'_> {
    fn check_discarded(&mut self, expr: &Expr) {
        if let Some(method) = cursor_returning_call(expr) {
            self.offenders.push(DiscardedThinCursor {
                path: self.path.to_string(),
                line: expr.span().start().line,
                method,
            });
        }
    }
}

impl Visit<'_> for DiscardedThinCursorVisitor<'_> {
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

    fn visit_stmt(&mut self, node: &Stmt) {
        match node {
            // `conn.execute_typed(..)?;` — a semicolon-terminated expression
            // statement throws its value away, and the cursor with it.
            Stmt::Expr(expr, Some(_)) => self.check_discarded(expr),
            // `let _ = conn.execute_typed(..);` — the same discard, spelled
            // as a binding that binds nothing.
            Stmt::Local(local) => {
                if matches!(local.pat, Pat::Wild(_)) {
                    if let Some(init) = local.init.as_ref() {
                        self.check_discarded(&init.expr);
                    }
                }
            }
            _ => {}
        }
        visit::visit_stmt(self, node);
    }
}

#[test]
fn a_thin_statement_result_is_never_dropped_with_its_server_cursor_inside() {
    let mut offenders: Vec<DiscardedThinCursor> = Vec::new();

    for path in collect_rust_files(Path::new("src")) {
        let display = path.display().to_string();
        let content = match fs::read_to_string(&path) {
            Ok(content) => content,
            Err(err) => panic!("failed to read {display}: {err}"),
        };
        let file = match syn::parse_file(&content) {
            Ok(file) => file,
            Err(err) => panic!("failed to parse {display}: {err}"),
        };

        let mut visitor = DiscardedThinCursorVisitor {
            path: &display,
            offenders: Vec::new(),
        };
        visitor.visit_file(&file);
        offenders.extend(visitor.offenders);
    }

    assert!(
        offenders.is_empty(),
        "a thin statement execution's result was discarded, which discards the open server \
         cursor inside it — use `query_drop` (execute + `close_cursor_later`) for a statement \
         run for its side effect, or bind the result and close its `cursor_id`:\n{offenders:#?}"
    );
}

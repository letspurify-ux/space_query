#![allow(
    clippy::cargo,
    clippy::expect_used,
    clippy::panic,
    clippy::pedantic,
    clippy::unwrap_used
)]

use std::ffi::OsStr;
use std::fs;
use std::path::{Path, PathBuf};

use quote::ToTokens;
use syn::spanned::Spanned;
use syn::visit::{self, Visit};
use syn::{Expr, ExprCall, ExprMethodCall, ItemFn, ItemMod, Macro};

#[allow(dead_code)]
#[derive(Debug)]
struct PanicSyntaxOffender {
    path: String,
    line: usize,
    syntax: String,
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

fn find_banned_panic_syntax(content: &str) -> Vec<&'static str> {
    const BANNED_PANIC_SYNTAX: [&str; 18] = [
        "panic!(",
        "panic_any(",
        "std::panic::panic_any",
        "todo!(",
        "unimplemented!(",
        "unreachable!(",
        "assert!(",
        "assert_eq!(",
        "assert_ne!(",
        ".unwrap(",
        ".expect(",
        ".expect_err(",
        ".unwrap_err(",
        ".unwrap_unchecked(",
        "Option::unwrap",
        "Option::expect",
        "Result::unwrap",
        "Result::expect",
    ];

    BANNED_PANIC_SYNTAX
        .iter()
        .filter(|pattern| content.contains(**pattern))
        .copied()
        .collect()
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

fn macro_panic_syntax(mac: &Macro) -> Option<String> {
    let ident = mac.path.segments.last()?.ident.to_string();
    match ident.as_str() {
        "panic" | "todo" | "unimplemented" | "unreachable" | "assert" | "assert_eq"
        | "assert_ne" => Some(format!("{ident}!")),
        _ => None,
    }
}

fn method_panic_syntax(node: &ExprMethodCall) -> Option<String> {
    match node.method.to_string().as_str() {
        "unwrap" | "expect" | "expect_err" | "unwrap_err" | "unwrap_unchecked" => {
            Some(format!(".{}()", node.method))
        }
        _ => None,
    }
}

fn call_path_segments(node: &ExprCall) -> Option<Vec<String>> {
    let Expr::Path(path) = node.func.as_ref() else {
        return None;
    };
    Some(
        path.path
            .segments
            .iter()
            .map(|segment| segment.ident.to_string())
            .collect(),
    )
}

fn path_ends_with(path: &[String], suffix: &[&str]) -> bool {
    path.len() >= suffix.len()
        && path[path.len() - suffix.len()..]
            .iter()
            .map(String::as_str)
            .eq(suffix.iter().copied())
}

fn call_panic_syntax(node: &ExprCall) -> Option<String> {
    let path = call_path_segments(node)?;
    if path.last().is_some_and(|segment| segment == "panic_any") {
        return Some("panic_any()".to_string());
    }

    for suffix in [
        &["Option", "unwrap"][..],
        &["Option", "expect"][..],
        &["Result", "unwrap"][..],
        &["Result", "expect"][..],
    ] {
        if path_ends_with(&path, suffix) {
            return Some(format!("{}()", suffix.join("::")));
        }
    }

    None
}

struct PanicSyntaxVisitor<'a> {
    path: &'a str,
    offenders: Vec<PanicSyntaxOffender>,
}

impl<'a> PanicSyntaxVisitor<'a> {
    fn new(path: &'a str) -> Self {
        Self {
            path,
            offenders: Vec::new(),
        }
    }

    fn push_offender(&mut self, line: usize, syntax: String) {
        self.offenders.push(PanicSyntaxOffender {
            path: self.path.to_string(),
            line,
            syntax,
        });
    }
}

impl Visit<'_> for PanicSyntaxVisitor<'_> {
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
        if let Some(syntax) = macro_panic_syntax(node) {
            self.push_offender(node.span().start().line, syntax);
        }
        visit::visit_macro(self, node);
    }

    fn visit_expr_method_call(&mut self, node: &ExprMethodCall) {
        if let Some(syntax) = method_panic_syntax(node) {
            self.push_offender(node.method.span().start().line, syntax);
        }
        visit::visit_expr_method_call(self, node);
    }

    fn visit_expr_call(&mut self, node: &ExprCall) {
        if let Some(syntax) = call_panic_syntax(node) {
            self.push_offender(node.span().start().line, syntax);
        }
        visit::visit_expr_call(self, node);
    }
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

fn collect_panic_syntax_offenders(
    src_root: &Path,
    relative_root: &Path,
) -> Vec<PanicSyntaxOffender> {
    let mut offenders = Vec::new();

    for file in collect_rust_files(src_root) {
        if is_test_source_file(&file) {
            continue;
        }

        let content = match fs::read_to_string(&file) {
            Ok(content) => content,
            Err(err) => panic!("failed to read source file {}: {err}", file.display()),
        };
        let parsed = match syn::parse_file(&content) {
            Ok(parsed) => parsed,
            Err(err) => panic!("failed to parse source file {}: {err}", file.display()),
        };
        let relative_path = match file.strip_prefix(relative_root) {
            Ok(path) => path.display().to_string(),
            Err(err) => panic!("failed to relativize {}: {err}", file.display()),
        };

        let mut visitor = PanicSyntaxVisitor::new(&relative_path);
        visitor.visit_file(&parsed);
        offenders.extend(visitor.offenders);
    }

    offenders
}

#[test]
fn non_test_source_does_not_use_panic_prone_syntax() {
    let manifest_dir = Path::new(env!("CARGO_MANIFEST_DIR"));
    let src_root = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let offenders = collect_panic_syntax_offenders(&src_root, manifest_dir);

    assert!(
        offenders.is_empty(),
        "found panic-prone syntax in non-test source files: {:?}",
        offenders
    );
}

#[test]
fn oracle_thin_non_test_source_does_not_use_panic_prone_syntax() {
    let manifest_dir = Path::new(env!("CARGO_MANIFEST_DIR"));
    let src_root = manifest_dir.join("crates/tns-thin/src");
    let offenders = collect_panic_syntax_offenders(&src_root, manifest_dir);

    assert!(
        offenders.is_empty(),
        "found panic-prone syntax in Oracle Thin non-test source files: {:?}",
        offenders
    );
}

#[test]
fn panic_syntax_detector_covers_common_panic_forms() {
    let sample = [
        "panic!(\"boom\")",
        "std::panic::panic_any(\"boom\")",
        "todo!()",
        "unimplemented!()",
        "unreachable!()",
        "assert!(ready)",
        "assert_eq!(left, right)",
        "value.unwrap()",
        "value.expect(\"present\")",
        "value.expect_err(\"error\")",
        "value.unwrap_err()",
        "unsafe { value.unwrap_unchecked() }",
        "Option::unwrap(value)",
        "Option::expect(value, \"present\")",
        "Result::unwrap(value)",
        "Result::expect(value, \"ok\")",
    ]
    .join("\n");

    let matched = find_banned_panic_syntax(&sample);

    assert!(
        matched.len() >= 16,
        "panic syntax detector missed expected forms: {:?}",
        matched
    );
}

#[test]
fn panic_ast_detector_covers_statement_and_expression_macros() {
    let parsed = syn::parse_file(
        r#"
        fn sample() {
            assert!(true);
            let _ = assert_eq!(1, 1);
        }
        "#,
    )
    .expect("parse panic macro sample");
    let mut visitor = PanicSyntaxVisitor::new("sample.rs");
    visitor.visit_file(&parsed);

    assert_eq!(visitor.offenders.len(), 2, "{:?}", visitor.offenders);
    assert!(visitor
        .offenders
        .iter()
        .any(|offender| offender.syntax == "assert!"));
    assert!(visitor
        .offenders
        .iter()
        .any(|offender| offender.syntax == "assert_eq!"));
}

use quote::ToTokens;
use std::collections::BTreeSet;
use std::ffi::OsStr;
use std::fs;
use std::path::{Path, PathBuf};
use syn::spanned::Spanned;
use syn::visit::{self, Visit};
use syn::{Attribute, ExprBinary, ImplItemFn, ItemFn, ItemMod, TraitItemFn};

#[derive(Debug, Clone, Eq, PartialEq, Ord, PartialOrd)]
struct ArithmeticSite {
    path: String,
    line: usize,
    kind: &'static str,
    expr: String,
}

fn collect_rust_files(root: &Path) -> Vec<PathBuf> {
    let mut files = Vec::new();
    let mut stack = vec![root.to_path_buf()];

    while let Some(dir) = stack.pop() {
        let entries = fs::read_dir(&dir)
            .unwrap_or_else(|err| panic!("failed to read directory {}: {err}", dir.display()));

        for entry in entries {
            let entry = entry.unwrap_or_else(|err| {
                panic!("failed to read directory entry in {}: {err}", dir.display())
            });
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

    files.sort();
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

fn attribute_marks_test(attr: &Attribute) -> bool {
    if attr.path().is_ident("test") {
        return true;
    }

    if attr.path().is_ident("cfg") {
        let tokens = attr.meta.to_token_stream().to_string();
        return tokens.contains("test");
    }

    false
}

fn attrs_mark_test(attrs: &[Attribute]) -> bool {
    attrs.iter().any(attribute_marks_test)
}

struct ArithmeticVisitor<'a> {
    file: &'a str,
    sites: Vec<ArithmeticSite>,
    skip_depth: usize,
}

impl<'a> ArithmeticVisitor<'a> {
    fn new(file: &'a str) -> Self {
        Self {
            file,
            sites: Vec::new(),
            skip_depth: 0,
        }
    }

    fn with_skip<T>(&mut self, should_skip: bool, visit_fn: impl FnOnce(&mut Self) -> T) -> T {
        if should_skip {
            self.skip_depth += 1;
        }
        let result = visit_fn(self);
        if should_skip {
            self.skip_depth = self.skip_depth.saturating_sub(1);
        }
        result
    }

    fn record(&mut self, span: proc_macro2::Span, kind: &'static str, expr: String) {
        if self.skip_depth > 0 {
            return;
        }

        self.sites.push(ArithmeticSite {
            path: self.file.to_string(),
            line: span.start().line,
            kind,
            expr,
        });
    }
}

impl<'ast> Visit<'ast> for ArithmeticVisitor<'_> {
    fn visit_item_mod(&mut self, node: &'ast ItemMod) {
        self.with_skip(attrs_mark_test(&node.attrs), |this| {
            visit::visit_item_mod(this, node);
        });
    }

    fn visit_item_fn(&mut self, node: &'ast ItemFn) {
        self.with_skip(attrs_mark_test(&node.attrs), |this| {
            visit::visit_item_fn(this, node);
        });
    }

    fn visit_impl_item_fn(&mut self, node: &'ast ImplItemFn) {
        self.with_skip(attrs_mark_test(&node.attrs), |this| {
            visit::visit_impl_item_fn(this, node);
        });
    }

    fn visit_trait_item_fn(&mut self, node: &'ast TraitItemFn) {
        self.with_skip(attrs_mark_test(&node.attrs), |this| {
            visit::visit_trait_item_fn(this, node);
        });
    }

    fn visit_expr_binary(&mut self, node: &'ast ExprBinary) {
        if self.skip_depth == 0 {
            let kind = match node.op {
                syn::BinOp::Div(_) => Some("div"),
                syn::BinOp::Rem(_) => Some("rem"),
                _ => None,
            };

            if let Some(kind) = kind {
                self.record(node.span(), kind, node.to_token_stream().to_string());
            }
        }

        visit::visit_expr_binary(self, node);
    }
}

fn collect_raw_arithmetic_sites() -> BTreeSet<ArithmeticSite> {
    let src_root = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut sites = BTreeSet::new();

    for file in collect_rust_files(&src_root) {
        if is_test_source_file(&file) {
            continue;
        }

        let raw = fs::read_to_string(&file)
            .unwrap_or_else(|err| panic!("failed to read source file {}: {err}", file.display()));
        let parsed = syn::parse_file(&raw)
            .unwrap_or_else(|err| panic!("failed to parse source file {}: {err}", file.display()));

        let relative = file
            .strip_prefix(Path::new(env!("CARGO_MANIFEST_DIR")))
            .unwrap_or_else(|err| panic!("failed to relativize {}: {err}", file.display()))
            .display()
            .to_string();
        if relative == "src/utils/arithmetic.rs" {
            continue;
        }

        let mut visitor = ArithmeticVisitor::new(&relative);
        visitor.visit_file(&parsed);
        sites.extend(visitor.sites);
    }

    sites
}

#[test]
fn source_does_not_use_raw_div_or_rem_outside_arithmetic_helpers() {
    let actual = collect_raw_arithmetic_sites();
    assert!(
        actual.is_empty(),
        "raw / and % are forbidden outside src/utils/arithmetic.rs; use arithmetic helpers instead. Found: {:?}",
        actual
    );
}

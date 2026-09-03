#![allow(
    clippy::cargo,
    clippy::expect_used,
    clippy::panic,
    clippy::pedantic,
    clippy::unwrap_used
)]

//! The grid's inline cell editor is the one `Fl_Input` this app destroys while
//! it is still the widget below the mouse.
//!
//! `Fl_Input` hides the mouse pointer on every keystroke it handles in that
//! position, and FLTK hands the pointer back only through an event delivered to
//! that same input. A destroyed widget receives none - so whoever destroys it
//! has to give the pointer back first, and `ResultTableWidget` does that in one
//! place. This guard keeps it one place: a second `Input::delete` elsewhere
//! would strand an invisible pointer on Windows all over again.

use std::fs;
use std::path::{Path, PathBuf};

use syn::visit::{self, Visit};
use syn::{Expr, ExprCall, ImplItemFn, ItemFn};

/// The one function allowed to destroy a transient text input.
const DISPOSAL_FN: &str = "dispose_inline_editor";

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

#[derive(Default)]
struct InputDeleteVisitor {
    enclosing_fn: Vec<String>,
    offenders: Vec<String>,
}

impl InputDeleteVisitor {
    fn record(&mut self, node: &ExprCall) {
        let Expr::Path(callee) = node.func.as_ref() else {
            return;
        };
        let path: Vec<String> = callee
            .path
            .segments
            .iter()
            .map(|segment| segment.ident.to_string())
            .collect();
        if path != ["Input", "delete"] {
            return;
        }

        let enclosing = self
            .enclosing_fn
            .last()
            .cloned()
            .unwrap_or_else(|| "<file scope>".to_string());
        if enclosing != DISPOSAL_FN {
            self.offenders.push(enclosing);
        }
    }
}

impl<'ast> Visit<'ast> for InputDeleteVisitor {
    fn visit_item_fn(&mut self, node: &'ast ItemFn) {
        self.enclosing_fn.push(node.sig.ident.to_string());
        visit::visit_item_fn(self, node);
        self.enclosing_fn.pop();
    }

    fn visit_impl_item_fn(&mut self, node: &'ast ImplItemFn) {
        self.enclosing_fn.push(node.sig.ident.to_string());
        visit::visit_impl_item_fn(self, node);
        self.enclosing_fn.pop();
    }

    fn visit_expr_call(&mut self, node: &'ast ExprCall) {
        self.record(node);
        visit::visit_expr_call(self, node);
    }
}

fn offending_functions(source: &str) -> Vec<String> {
    let file = syn::parse_file(source).expect("failed to parse Rust source");
    let mut visitor = InputDeleteVisitor::default();
    visitor.visit_file(&file);
    visitor.offenders
}

#[test]
fn transient_text_inputs_are_destroyed_in_one_place() {
    let src_root = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let manifest_dir = Path::new(env!("CARGO_MANIFEST_DIR"));
    let mut offenders = Vec::new();

    for file in collect_rust_files(&src_root) {
        let content = fs::read_to_string(&file)
            .unwrap_or_else(|err| panic!("failed to read source file {}: {err}", file.display()));

        for enclosing in offending_functions(&content) {
            let path = file.strip_prefix(manifest_dir).unwrap_or(&file);
            offenders.push(format!("{} in `{}`", path.display(), enclosing));
        }
    }

    assert!(
        offenders.is_empty(),
        "an `Input` is destroyed outside `{DISPOSAL_FN}`, which is where the mouse pointer \
         the input hid is handed back:\n{}",
        offenders.join("\n")
    );
}

/// The guard reads where the call sits, not whether the file mentions the name.
#[test]
fn guard_detects_an_input_destroyed_outside_the_disposal_path() {
    let planted = r#"
        impl Widget {
            fn dispose_inline_editor(input: Input) {
                Input::delete(input);
            }

            fn close_editor(input: Input) {
                Input::delete(input);
            }
        }
    "#;

    assert_eq!(
        offending_functions(planted),
        vec!["close_editor".to_string()]
    );
}

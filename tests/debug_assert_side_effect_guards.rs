use std::fs;
use std::path::{Path, PathBuf};

use syn::spanned::Spanned;
use syn::visit::{self, Visit};
use syn::{ExprMacro, Macro};

const BANNED_MUTATION_PATTERNS: [&str; 13] = [
    ".pop(",
    ".push(",
    ".remove(",
    ".take(",
    ".insert(",
    ".replace(",
    ".clear(",
    ".truncate(",
    ".retain(",
    ".append(",
    ".split_off(",
    ".drain(",
    ".swap_remove(",
];

#[allow(dead_code)]
#[derive(Debug)]
struct DebugAssertMutationOffender {
    path: String,
    line: usize,
    macro_name: String,
    pattern: &'static str,
    snippet: String,
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

fn line_start_offsets(source: &str) -> Vec<usize> {
    let mut starts = vec![0usize];
    for (idx, byte) in source.bytes().enumerate() {
        if byte == b'\n' {
            starts.push(idx + 1);
        }
    }
    starts
}

fn byte_offset_for_line_column(line_starts: &[usize], line: usize, column: usize) -> Option<usize> {
    let line_start = *line_starts.get(line.checked_sub(1)?)?;
    Some(line_start.saturating_add(column))
}

fn span_snippet(source: &str, line_starts: &[usize], span: proc_macro2::Span) -> Option<String> {
    let start = span.start();
    let end = span.end();
    let start_offset = byte_offset_for_line_column(line_starts, start.line, start.column)?;
    let end_offset = byte_offset_for_line_column(line_starts, end.line, end.column)?;
    source
        .get(start_offset..end_offset)
        .map(|snippet| snippet.to_string())
}

fn is_debug_assert_macro(mac: &Macro) -> Option<String> {
    let ident = mac.path.segments.last()?.ident.to_string();
    match ident.as_str() {
        "debug_assert" | "debug_assert_eq" | "debug_assert_ne" => Some(ident),
        _ => None,
    }
}

struct DebugAssertMutationVisitor<'a> {
    path: &'a str,
    source: &'a str,
    line_starts: Vec<usize>,
    offenders: Vec<DebugAssertMutationOffender>,
}

impl<'a> DebugAssertMutationVisitor<'a> {
    fn new(path: &'a str, source: &'a str) -> Self {
        Self {
            path,
            source,
            line_starts: line_start_offsets(source),
            offenders: Vec::new(),
        }
    }

    fn inspect_macro(&mut self, mac: &Macro) {
        let Some(macro_name) = is_debug_assert_macro(mac) else {
            return;
        };

        let snippet = span_snippet(self.source, &self.line_starts, mac.span())
            .unwrap_or_else(|| mac.tokens.to_string());
        let line = mac.span().start().line;

        for pattern in BANNED_MUTATION_PATTERNS {
            if snippet.contains(pattern) {
                self.offenders.push(DebugAssertMutationOffender {
                    path: self.path.to_string(),
                    line,
                    macro_name: macro_name.clone(),
                    pattern,
                    snippet: snippet.trim().to_string(),
                });
            }
        }
    }
}

impl Visit<'_> for DebugAssertMutationVisitor<'_> {
    fn visit_expr_macro(&mut self, node: &ExprMacro) {
        self.inspect_macro(&node.mac);
        visit::visit_expr_macro(self, node);
    }
}

#[test]
fn source_does_not_hide_state_mutation_inside_debug_assert_macros() {
    let manifest_dir = Path::new(env!("CARGO_MANIFEST_DIR"));
    let mut files = collect_rust_files(&manifest_dir.join("src"));
    files.extend(collect_rust_files(&manifest_dir.join("tests")));
    files.sort();

    let mut offenders = Vec::new();

    for file in files {
        let source = fs::read_to_string(&file)
            .unwrap_or_else(|err| panic!("failed to read source file {}: {err}", file.display()));
        let parsed = syn::parse_file(&source)
            .unwrap_or_else(|err| panic!("failed to parse source file {}: {err}", file.display()));
        let relative_path = file
            .strip_prefix(manifest_dir)
            .unwrap_or_else(|err| panic!("failed to relativize {}: {err}", file.display()))
            .display()
            .to_string();

        let mut visitor = DebugAssertMutationVisitor::new(&relative_path, &source);
        visitor.visit_file(&parsed);
        offenders.extend(visitor.offenders);
    }

    assert!(
        offenders.is_empty(),
        "state mutation inside debug_assert macros is forbidden because release drops those calls: {:?}",
        offenders
    );
}

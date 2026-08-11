#![allow(
    clippy::cargo,
    clippy::expect_used,
    clippy::panic,
    clippy::pedantic,
    clippy::unwrap_used
)]

use std::fs;
use std::path::{Path, PathBuf};

const NON_THREAD_SAFE_PATTERNS: [&str; 11] = [
    "Rc<",
    "Rc::new",
    "Rc::clone",
    "std::rc::Rc",
    "Rc<RefCell<",
    "Rc<Cell<",
    "Rc<UnsafeCell<",
    "RefCell",
    "std::cell::RefCell",
    "rc::Weak<",
    "std::rc::Weak",
];

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

    files
}

/// `thread_local!` state is per-thread by construction, so a `RefCell` inside
/// one is not shared state — it is the standard idiom for it (the debug-only
/// lock-order tracker keeps the locks the CURRENT thread holds that way).
/// Strip those blocks, and the import they need, before scanning; every other
/// use of the banned types still counts.
fn content_without_thread_local_state(content: &str) -> String {
    let mut kept = String::with_capacity(content.len());
    let mut rest = content;

    while let Some(start) = rest.find("thread_local!") {
        kept.push_str(&rest[..start]);
        let block = &rest[start..];
        let Some(open) = block.find('{') else {
            rest = "";
            break;
        };
        let mut depth = 0usize;
        let mut end = None;
        for (offset, ch) in block[open..].char_indices() {
            match ch {
                '{' => depth += 1,
                '}' => {
                    depth -= 1;
                    if depth == 0 {
                        end = Some(open + offset + 1);
                        break;
                    }
                }
                _ => {}
            }
        }
        match end {
            Some(end) => rest = &block[end..],
            None => {
                rest = "";
                break;
            }
        }
    }
    kept.push_str(rest);

    kept.lines()
        .filter(|line| line.trim() != "use std::cell::RefCell;")
        .collect::<Vec<_>>()
        .join("\n")
}

#[test]
fn source_does_not_use_rc_or_refcell() {
    let src_root = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut offenders = Vec::new();

    for file in collect_rust_files(&src_root) {
        let content = fs::read_to_string(&file)
            .unwrap_or_else(|err| panic!("failed to read source file {}: {err}", file.display()));

        let shared_state_content = content_without_thread_local_state(&content);
        if NON_THREAD_SAFE_PATTERNS
            .iter()
            .any(|pattern| shared_state_content.contains(pattern))
        {
            offenders.push(file);
        }
    }

    assert!(
        offenders.is_empty(),
        "found non-thread-safe shared state types in: {:?}",
        offenders
    );
}

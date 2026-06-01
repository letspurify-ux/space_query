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

#[test]
fn source_does_not_use_rc_or_refcell() {
    let src_root = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut offenders = Vec::new();

    for file in collect_rust_files(&src_root) {
        let content = fs::read_to_string(&file)
            .unwrap_or_else(|err| panic!("failed to read source file {}: {err}", file.display()));

        if NON_THREAD_SAFE_PATTERNS
            .iter()
            .any(|pattern| content.contains(pattern))
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

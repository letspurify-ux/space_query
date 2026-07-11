#![allow(
    clippy::cargo,
    clippy::expect_used,
    clippy::panic,
    clippy::pedantic,
    clippy::unwrap_used
)]

use std::fs;
use std::path::{Path, PathBuf};

const BANNED_FLTK_DIALOG_CALLS: [&str; 22] = [
    "alert_default(",
    "message_default(",
    "choice_default(",
    "choice2_default(",
    "input_default(",
    "password_default(",
    "fltk::dialog::alert(",
    "fltk::dialog::message(",
    "fltk::dialog::choice(",
    "fltk::dialog::choice2(",
    "fltk::dialog::input(",
    "fltk::dialog::password(",
    "dialog::alert(",
    "dialog::message(",
    "dialog::choice(",
    "dialog::choice2(",
    "dialog::input(",
    "dialog::password(",
    "use fltk::dialog::alert",
    "use fltk::dialog::message",
    "use fltk::dialog::choice",
    "use fltk::dialog::input",
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

    files.sort();
    files
}

#[test]
fn ui_code_does_not_call_default_fltk_message_dialogs() {
    let src_root = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let manifest_dir = Path::new(env!("CARGO_MANIFEST_DIR"));
    let mut offenders = Vec::new();

    for file in collect_rust_files(&src_root) {
        let content = fs::read_to_string(&file)
            .unwrap_or_else(|err| panic!("failed to read source file {}: {err}", file.display()));

        for (line_index, line) in content.lines().enumerate() {
            for pattern in BANNED_FLTK_DIALOG_CALLS {
                if line.contains(pattern) {
                    let path = file.strip_prefix(manifest_dir).unwrap_or(&file);
                    offenders.push(format!(
                        "{}:{} uses `{}`",
                        path.display(),
                        line_index + 1,
                        pattern
                    ));
                }
            }
        }
    }

    assert!(
        offenders.is_empty(),
        "FLTK common message dialogs must go through crate::ui::*_on_main wrappers:\n{}",
        offenders.join("\n")
    );
}

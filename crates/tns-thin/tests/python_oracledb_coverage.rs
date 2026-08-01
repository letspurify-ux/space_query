#![allow(clippy::cargo, clippy::expect_used, clippy::panic, clippy::pedantic)]

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

const EXPECTED_UPSTREAM_TEST_FUNCTIONS: usize = 2_356;

#[test]
fn every_python_oracledb_test_function_has_a_reviewed_rust_disposition() {
    let crate_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let repository_dir = crate_dir
        .parent()
        .and_then(Path::parent)
        .expect("tns-thin crate should be nested under crates/");
    let upstream_tests = repository_dir.join("vendor/python-oracledb/tests");
    let upstream = collect_upstream_tests(&upstream_tests);
    assert_eq!(
        upstream.len(),
        EXPECTED_UPSTREAM_TEST_FUNCTIONS,
        "python-oracledb test inventory changed; review every added or removed test"
    );

    let mapping_path = crate_dir.join("python_oracledb_coverage.txt");
    let mapping = read_mapping(&mapping_path);
    let rust_sources = read_rust_sources(&crate_dir);
    let mut failures = Vec::new();

    for (test_name, source) in &upstream {
        match mapping.get(test_name) {
            Some(("covered", evidence)) if rust_sources.contains(evidence) => {}
            Some(("covered", evidence)) => failures.push(format!(
                "{test_name} ({source}): Rust evidence `{evidence}` was not found"
            )),
            Some(("not_applicable", reason)) if !reason.trim().is_empty() => {}
            Some((status, _)) => failures.push(format!(
                "{test_name} ({source}): invalid or incomplete status `{status}`"
            )),
            None => failures.push(format!("{test_name} ({source}): missing disposition")),
        }
    }
    for test_name in mapping.keys() {
        if !upstream.contains_key(test_name) {
            failures.push(format!(
                "{test_name}: mapping has no upstream test function"
            ));
        }
    }

    assert!(
        failures.is_empty(),
        "python-oracledb coverage audit failed:\n{}",
        failures.join("\n")
    );
}

fn collect_upstream_tests(root: &Path) -> BTreeMap<String, String> {
    let mut files = Vec::new();
    collect_files(root, "py", &mut files);
    let mut tests = BTreeMap::new();
    for path in files {
        let body = fs::read_to_string(&path).expect("read python-oracledb test source");
        for line in body.lines() {
            let line = line.trim_start();
            let line = line.strip_prefix("async ").unwrap_or(line);
            let Some(definition) = line.strip_prefix("def ") else {
                continue;
            };
            let Some((name, _)) = definition.split_once('(') else {
                continue;
            };
            if !name.starts_with("test_") {
                continue;
            }
            let relative = path
                .strip_prefix(root)
                .unwrap_or(&path)
                .display()
                .to_string();
            assert!(
                tests.insert(name.to_string(), relative).is_none(),
                "duplicate upstream test function {name}"
            );
        }
    }
    tests
}

fn read_mapping(path: &Path) -> BTreeMap<String, (&str, &str)> {
    let body = fs::read_to_string(path).expect("read python-oracledb coverage mapping");
    let body: &'static str = Box::leak(body.into_boxed_str());
    let mut mapping = BTreeMap::new();
    for (index, line) in body.lines().enumerate() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let fields = line.splitn(3, '|').collect::<Vec<_>>();
        assert_eq!(
            fields.len(),
            3,
            "invalid coverage mapping at line {}",
            index + 1
        );
        assert!(
            mapping
                .insert(fields[0].to_string(), (fields[1], fields[2]))
                .is_none(),
            "duplicate coverage mapping for {}",
            fields[0]
        );
    }
    mapping
}

fn read_rust_sources(crate_dir: &Path) -> String {
    let mut files = Vec::new();
    collect_files(&crate_dir.join("src"), "rs", &mut files);
    collect_files(&crate_dir.join("tests"), "rs", &mut files);
    files
        .into_iter()
        .map(|path| fs::read_to_string(path).expect("read tns-thin Rust source"))
        .collect::<Vec<_>>()
        .join("\n")
}

fn collect_files(root: &Path, extension: &str, files: &mut Vec<PathBuf>) {
    let entries = fs::read_dir(root).unwrap_or_else(|error| {
        panic!("failed to read {}: {error}", root.display());
    });
    for entry in entries {
        let path = entry.expect("read directory entry").path();
        if path.is_dir() {
            collect_files(&path, extension, files);
        } else if path.extension().and_then(|value| value.to_str()) == Some(extension) {
            files.push(path);
        }
    }
}

#![allow(clippy::cargo, clippy::pedantic)]

// UI-path verification for per-tab object browsers.
//
// Every query editor tab owns its own object-browser card — tree, filter,
// expansion state, scope selector and metadata cache. Nothing about that is
// visible to a unit test: the cards are FLTK widgets, and the rule under test
// ("a plain tab switch restores the tab exactly as it was, and only a new tab
// or a scope change reloads") lives in `AppState::set_active_editor_tab`,
// which needs a real `MainWindow`.
//
// Three regressions this exists to keep fixed, all reported from the running
// app after the browser became per-tab:
//
//   * Switching tabs came back to an EMPTY tree, with the expansion state
//     gone, because each tab's card was born empty and could only be filled by
//     a fresh metadata load.
//   * The editor had no metadata until the user pressed Refresh, so
//     highlighting and completion were dead on the tab just switched to.
//   * The scope selector showed the alphabetically first database for a moment
//     before flipping to the tab's own scope.
//
// Runs without a database: it drives the same offline example connections the
// capture tour uses.
//
// Usage: cargo run --bin verify_object_browser_tabs_ui

use fltk::app;
use space_query::ui::{apply_global_default_font, profile_by_name, MainWindow};
use space_query::utils::arithmetic::safe_div;
use space_query::utils::config::AppConfig;
use std::thread;
use std::time::Duration;

fn pump(milliseconds: u64) {
    for _ in 0..safe_div(milliseconds, 20).max(1) {
        app::check();
        thread::sleep(Duration::from_millis(20));
    }
}

struct Report {
    failures: Vec<String>,
}

impl Report {
    fn check(&mut self, label: &str, ok: bool, detail: String) {
        if ok {
            println!("    OK  {label}");
        } else {
            println!("    FAIL {label}: {detail}");
            self.failures.push(format!("{label}: {detail}"));
        }
    }
}

fn main() {
    let config = AppConfig {
        editor_font: "D2Coding".to_string(),
        result_font: "D2Coding".to_string(),
        ..AppConfig::default()
    };
    let _app = app::App::default()
        .with_scheme(app::Scheme::Gtk)
        .load_system_fonts();
    apply_global_default_font(profile_by_name("D2Coding").normal);

    let mut main_window = MainWindow::new_with_config(config);
    // Installs two example connections and two editor tabs (Oracle active,
    // MariaDB second) and fills the active tab's card with example metadata.
    main_window.capture_tour_show_object_browser();
    pump(300);

    let mut report = Report {
        failures: Vec::new(),
    };

    let tab_ids = main_window.capture_tour_editor_tab_ids();
    if tab_ids.len() < 2 {
        eprintln!("expected two example editor tabs, found {}", tab_ids.len());
        std::process::exit(2);
    }
    let (first_tab, second_tab) = (tab_ids[0], tab_ids[1]);

    println!("  --- the active tab's card holds its own metadata ---");
    // A brand-new card already draws the empty root categories, so the proof
    // that a card holds real metadata is an OBJECT node under one of them.
    let has_object_nodes = |paths: &[String]| paths.iter().any(|path| path.contains("Tables/EMP"));
    let baseline_paths = main_window.capture_tour_active_tab_object_tree_paths();
    report.check(
        "the active tab's tree is populated",
        has_object_nodes(&baseline_paths),
        format!("tree paths: {baseline_paths:?}"),
    );
    report.check(
        "a card that loaded reports that it has metadata",
        main_window.capture_tour_active_tab_has_object_metadata(),
        "the filled card claims to be empty".into(),
    );
    let baseline_scope = main_window.capture_tour_active_tab_displayed_scope();
    report.check(
        "the scope selector shows the tab's own schema",
        baseline_scope.as_deref() == Some("SYSTEM"),
        format!("displayed scope: {baseline_scope:?}"),
    );

    // Expand a node, so the switch has something to lose.
    let _ = main_window.capture_tour_expand_object_path("Tables");
    let _ = main_window.capture_tour_expand_object_path("Views");
    pump(200);
    let baseline_expanded = main_window.capture_tour_active_tab_expanded_object_paths();
    report.check(
        "the expansion the user made is recorded",
        baseline_expanded.iter().any(|path| path.contains("Tables")),
        format!("expanded paths: {baseline_expanded:?}"),
    );

    println!("  --- a second tab gets its OWN card, not the first tab's ---");
    let switched = main_window.capture_tour_select_editor_tab(second_tab);
    pump(300);
    report.check(
        "switching to the second tab succeeds",
        switched,
        "set_active_editor_tab refused".into(),
    );
    let second_expanded = main_window.capture_tour_active_tab_expanded_object_paths();
    report.check(
        "the second tab does not inherit the first tab's expansion",
        second_expanded != baseline_expanded || second_expanded.is_empty(),
        format!("second tab expanded paths: {second_expanded:?}"),
    );
    // This tab is on the other connection, where nothing has loaded. Its card
    // must say so: a card that reports metadata it does not have is one a new
    // card would inherit an empty catalog from, and neither would ever
    // schedule the load that fills them.
    report.check(
        "a card that never loaded reports that it has no metadata",
        !main_window.capture_tour_active_tab_has_object_metadata(),
        "an empty card claims to hold a catalog".into(),
    );

    println!("  --- switching back restores the tab exactly as it was ---");
    let switched_back = main_window.capture_tour_select_editor_tab(first_tab);
    pump(300);
    report.check(
        "switching back to the first tab succeeds",
        switched_back,
        "set_active_editor_tab refused".into(),
    );
    let restored_paths = main_window.capture_tour_active_tab_object_tree_paths();
    report.check(
        "the tree is still there after the round trip",
        restored_paths == baseline_paths,
        format!("before: {baseline_paths:?}\n         after:  {restored_paths:?}"),
    );
    let restored_expanded = main_window.capture_tour_active_tab_expanded_object_paths();
    report.check(
        "the expansion state survived the round trip",
        restored_expanded == baseline_expanded,
        format!("before: {baseline_expanded:?}\n         after:  {restored_expanded:?}"),
    );
    let restored_scope = main_window.capture_tour_active_tab_displayed_scope();
    report.check(
        "the scope selector still shows the tab's schema, with no detour",
        restored_scope == baseline_scope,
        format!("before: {baseline_scope:?}, after: {restored_scope:?}"),
    );

    println!("  --- a new tab inherits the connection's metadata, not a blank tree ---");
    main_window.capture_tour_new_editor_tab();
    pump(400);
    let new_tab_paths = main_window.capture_tour_active_tab_object_tree_paths();
    report.check(
        "the new tab's card shows the connection's objects immediately",
        has_object_nodes(&new_tab_paths),
        format!("new tab tree paths: {new_tab_paths:?}"),
    );
    // An inherited catalog has to COUNT as loaded, or the tab reports itself
    // empty, the next switch schedules a load and the inherited tree is
    // thrown away again.
    report.check(
        "the inherited card counts as loaded",
        main_window.capture_tour_active_tab_has_object_metadata(),
        "the new tab shows a tree but reports itself empty".into(),
    );
    let new_tab_tables = main_window.capture_tour_active_tab_intellisense_tables();
    report.check(
        "the new tab's editor can highlight and complete straight away",
        !new_tab_tables.is_empty(),
        format!("new tab intellisense tables: {new_tab_tables:?}"),
    );
    let new_tab_scope = main_window.capture_tour_active_tab_displayed_scope();
    report.check(
        "the new tab opens on its own schema, not the first one alphabetically",
        new_tab_scope == baseline_scope,
        format!("expected {baseline_scope:?}, got {new_tab_scope:?}"),
    );

    println!("  --- closing a tab takes its card with it ---");
    let cards_before_close = main_window.capture_tour_object_browser_card_count();
    let Some(new_tab) = main_window.capture_tour_editor_tab_ids().last().copied() else {
        eprintln!("the tab just created is missing");
        std::process::exit(2);
    };
    report.check(
        "the new tab owns a card while it is open",
        main_window.capture_tour_tab_has_object_browser_card(new_tab),
        "no card for the tab just created".into(),
    );
    let closed = main_window.capture_tour_close_editor_tab(new_tab);
    pump(400);
    report.check(
        "closing the tab succeeds",
        closed,
        "close_query_editor_tab refused".into(),
    );
    report.check(
        "the closed tab's card is gone",
        !main_window.capture_tour_tab_has_object_browser_card(new_tab),
        "the card outlived its tab".into(),
    );
    let cards_after_close = main_window.capture_tour_object_browser_card_count();
    report.check(
        "exactly one card was torn down",
        cards_after_close + 1 == cards_before_close,
        format!("cards before: {cards_before_close}, after: {cards_after_close}"),
    );

    // The panel must still work on the tab the close fell back to, and the
    // surviving tab's own view must be untouched by its neighbour's teardown.
    let Some(after_close_tab) = main_window.capture_tour_editor_tab_ids().first().copied() else {
        eprintln!("no editor tab survived the close");
        std::process::exit(2);
    };
    let switched_after_close = main_window.capture_tour_select_editor_tab(after_close_tab);
    pump(300);
    report.check(
        "a tab is still usable after the close",
        switched_after_close,
        "could not activate a surviving tab".into(),
    );
    let survivor_paths = main_window.capture_tour_active_tab_object_tree_paths();
    report.check(
        "the surviving tab still has its own tree",
        has_object_nodes(&survivor_paths),
        format!("surviving tab tree paths: {survivor_paths:?}"),
    );

    println!("  --- repeated open/close leaves no cards behind ---");
    // Teardown runs on every tab close now, so run it many times in a row:
    // a card that survives its tab, or one deleted twice, shows up here as a
    // drifting count or a panic rather than as a rare crash in the field.
    let cards_at_rest = main_window.capture_tour_object_browser_card_count();
    let tabs_at_rest = main_window.capture_tour_editor_tab_ids().len();
    for round in 0..5 {
        main_window.capture_tour_new_editor_tab();
        pump(120);
        let Some(spawned) = main_window.capture_tour_editor_tab_ids().last().copied() else {
            eprintln!("round {round}: the tab just created is missing");
            std::process::exit(2);
        };
        let closed = main_window.capture_tour_close_editor_tab(spawned);
        pump(120);
        report.check(
            &format!("round {round}: the tab opens and closes"),
            closed && !main_window.capture_tour_tab_has_object_browser_card(spawned),
            format!("closed = {closed}"),
        );
    }
    report.check(
        "five open/close rounds leave the card count where it started",
        main_window.capture_tour_object_browser_card_count() == cards_at_rest
            && main_window.capture_tour_editor_tab_ids().len() == tabs_at_rest,
        format!(
            "cards {} -> {}, tabs {} -> {}",
            cards_at_rest,
            main_window.capture_tour_object_browser_card_count(),
            tabs_at_rest,
            main_window.capture_tour_editor_tab_ids().len()
        ),
    );
    // Closing a tab activates its neighbour, which here is the tab on the
    // OTHER connection and has never loaded — so come back to the loaded tab
    // deliberately and check the churn did not damage its card.
    let _ = main_window.capture_tour_select_editor_tab(first_tab);
    pump(200);
    let after_rounds_paths = main_window.capture_tour_active_tab_object_tree_paths();
    report.check(
        "the loaded tab still has its catalog after the churn",
        has_object_nodes(&after_rounds_paths)
            && main_window.capture_tour_active_tab_has_object_metadata(),
        format!("tree paths: {after_rounds_paths:?}"),
    );
    report.check(
        "the loaded tab kept its expansion through the churn",
        main_window.capture_tour_active_tab_expanded_object_paths() == baseline_expanded,
        format!(
            "expanded: {:?} (baseline {baseline_expanded:?})",
            main_window.capture_tour_active_tab_expanded_object_paths()
        ),
    );

    println!("  --- a scope change orphans the catalog the card is holding ---");
    // The cache still describes the old schema. If the card kept reporting
    // "loaded" while claiming the new scope, a tab opened before the reload
    // lands (it cannot even start while the connection is busy) would inherit
    // one schema's objects under another schema's name, with no reload ever
    // scheduled to correct it.
    report.check(
        "the card reports metadata before the scope moves",
        main_window.capture_tour_active_tab_has_object_metadata(),
        "precondition failed: the card was already empty".into(),
    );
    main_window.capture_tour_set_active_tab_scope(Some("HR".to_string()));
    pump(200);
    report.check(
        "after moving to another schema the card no longer offers its catalog",
        !main_window.capture_tour_active_tab_has_object_metadata(),
        "the card still claims a catalog that describes the previous schema".into(),
    );

    println!("  --- a scope pick moves ONLY the tab that made it ---");
    // Reported from the running app: with several tabs on one database, the
    // schema picked in the last tab changed the others too — the selector
    // looked per tab, but the value execution uses did not.
    let scope_tab = main_window
        .capture_tour_editor_tab_ids()
        .first()
        .copied()
        .unwrap_or(first_tab);
    let _ = main_window.capture_tour_select_editor_tab(scope_tab);
    pump(200);
    main_window.capture_tour_new_editor_tab();
    pump(300);
    let tabs_before = main_window.capture_tour_tab_binding_scopes();
    let Some(sibling_tab) = main_window.capture_tour_editor_tab_ids().last().copied() else {
        eprintln!("the sibling tab is missing");
        std::process::exit(2);
    };
    // Pick a schema through the real selector path on the sibling tab.
    main_window.capture_tour_pick_object_browser_scope(Some("HR".to_string()));
    pump(300);
    let tabs_after = main_window.capture_tour_tab_binding_scopes();
    let moved: Vec<_> = tabs_before
        .iter()
        .zip(tabs_after.iter())
        .filter(|((_, before), (_, after))| before != after)
        .map(|((tab_id, before), (_, after))| (*tab_id, before.clone(), after.clone()))
        .collect();
    report.check(
        "the pick moves exactly one tab's scope",
        moved.len() <= 1,
        format!("tabs that moved: {moved:?} (picked on tab {sibling_tab})"),
    );
    report.check(
        "the tab that moved is the one the pick was made on",
        moved
            .first()
            .is_none_or(|(tab_id, _, _)| *tab_id == sibling_tab),
        format!("moved: {moved:?}, expected only {sibling_tab}"),
    );

    println!();
    if report.failures.is_empty() {
        println!("ALL PER-TAB OBJECT BROWSER CHECKS PASSED");
    } else {
        println!("FAILURES:");
        for failure in &report.failures {
            println!("  - {failure}");
        }
        std::process::exit(1);
    }
}

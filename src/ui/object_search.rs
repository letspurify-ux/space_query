//! Name search over the object metadata the browser has already cached.
//!
//! This is deliberately *not* a server query. The tree already holds every
//! object of the connection's current scope, so a search over it answers
//! instantly and can never disagree with what the tree shows. What it adds over
//! the tree's filter box is a flat, ranked list that can be driven entirely from
//! the keyboard: type a few letters, press Enter, land in the source.

use crate::db::PackageRoutine;
use crate::ui::object_browser::{ObjectBrowserWidget, ObjectCache, ObjectItem};

/// Upper bound on returned hits. A scope with tens of thousands of objects
/// would otherwise build a browser list nobody scrolls through.
pub const MAX_OBJECT_SEARCH_HITS: usize = 200;

/// Tree categories, in the order the tree itself lists them. Ties in match
/// quality are broken by this order so results stay predictable.
const CATEGORY_ORDER: [&str; 9] = [
    "TABLES",
    "VIEWS",
    "PROCEDURES",
    "FUNCTIONS",
    "PACKAGES",
    "SEQUENCES",
    "TRIGGERS",
    "EVENTS",
    "SYNONYMS",
];

/// One search result: what the user reads, and WHAT IT IS.
///
/// The two are separate fields on purpose. `display_name` is presentation —
/// a package member is shown as `PKG.ROUTINE` so the user can tell which
/// package they picked — and [`Self::item`] is identity. This used to hold
/// only the text and recover the identity from it by splitting on the first
/// `.`, which is a guess: a `.` is legal INSIDE an Oracle or MySQL catalog
/// name (`"MY.PKG"`), so a package whose own name carries one came back as a
/// member of a package that does not exist, and a member of such a package
/// lost both halves. Nothing parses the display text any more; the identity is
/// built from the parts the cache already keeps apart.
///
/// The `item` field is private and the constructors below are the only way to
/// make one, so a hit cannot exist whose text and identity disagree.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ObjectSearchHit {
    item: ObjectItem,
    /// Name as the user should read it: `PKG.ROUTINE` for a package member.
    pub display_name: String,
    /// Single-word kind shown next to the name (`Table`, `Procedure`, …).
    pub kind_label: String,
}

impl ObjectSearchHit {
    /// A hit for one object listed under `category`.
    fn simple(category: &str, object_name: String) -> Self {
        Self {
            display_name: object_name.clone(),
            kind_label: kind_label(category).to_string(),
            item: ObjectItem::Simple {
                object_type: category.to_string(),
                object_name,
            },
        }
    }

    /// A hit for one member of `package_name`.
    ///
    /// The routine KIND comes from the catalog row through the browser's own
    /// normaliser, not from [`Self::kind_label`]: the label is title-cased for
    /// display, and reading the kind back out of it produced a value no
    /// consumer knows — `title_case("")` answers `Object`, which uppercases to
    /// `OBJECT` and matches neither an action arm nor the `UNKNOWN` that asks
    /// the server.
    fn package_member(package_name: &str, routine: &PackageRoutine) -> Self {
        Self {
            display_name: qualified_member_name(package_name, &routine.name),
            kind_label: title_case(&routine.routine_type),
            item: ObjectItem::PackageRoutine {
                package_name: package_name.to_string(),
                routine_name: routine.name.clone(),
                routine_type: ObjectBrowserWidget::package_routine_item_type(&routine.routine_type),
            },
        }
    }

    /// What the browser should act on — the same value its own tree hands to
    /// the context menu and the default action.
    pub fn to_object_item(&self) -> ObjectItem {
        self.item.clone()
    }

    /// One row of the results list.
    ///
    /// Object names are data, not markup, but FLTK's browser reads a leading
    /// `@` in a field as a format code — a quoted identifier like `"@RATE"`
    /// would lose its name. Doubling it is FLTK's own escape.
    pub fn browser_line(&self) -> String {
        format!(
            "{}\t{}",
            escape_browser_field(&self.display_name),
            escape_browser_field(&self.kind_label)
        )
    }
}

/// How a package member is spelled in the list — the ONE place the two names
/// are joined, so what the matcher searches and what the hit shows cannot
/// drift apart.
fn qualified_member_name(package_name: &str, routine_name: &str) -> String {
    format!("{package_name}.{routine_name}")
}

/// FLTK only parses format codes at the start of a field, so only a leading
/// `@` needs doubling; escaping every one would show the extra characters.
fn escape_browser_field(value: &str) -> String {
    match value.strip_prefix('@') {
        Some(rest) => format!("@@{rest}"),
        None => value.to_string(),
    }
}

/// Where a query matched a name. Lower is better.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum MatchRank {
    Exact,
    Prefix,
    Substring,
}

fn match_rank(name: &str, query: &str) -> Option<MatchRank> {
    let name = name.to_lowercase();
    if name == query {
        return Some(MatchRank::Exact);
    }
    if name.starts_with(query) {
        return Some(MatchRank::Prefix);
    }
    if name.contains(query) {
        return Some(MatchRank::Substring);
    }
    None
}

fn category_position(category: &str) -> usize {
    CATEGORY_ORDER
        .iter()
        .position(|known| *known == category)
        .unwrap_or(CATEGORY_ORDER.len())
}

fn kind_label(category: &str) -> &'static str {
    match category {
        "TABLES" => "Table",
        "VIEWS" => "View",
        "PROCEDURES" => "Procedure",
        "FUNCTIONS" => "Function",
        "PACKAGES" => "Package",
        "SEQUENCES" => "Sequence",
        "TRIGGERS" => "Trigger",
        "EVENTS" => "Event",
        "SYNONYMS" => "Synonym",
        _ => "Object",
    }
}

/// Rank the cached objects of one scope against `query`.
///
/// An empty query lists everything (capped), which is what makes the dialog
/// useful before a single key is typed. Matching is case-insensitive; a package
/// member matches on its own name and on `PACKAGE.MEMBER`.
pub fn search(cache: &ObjectCache, query: &str, limit: usize) -> Vec<ObjectSearchHit> {
    let query = query.trim().to_lowercase();
    let mut scored: Vec<(MatchRank, usize, usize, ObjectSearchHit)> = Vec::new();

    let mut consider = |category: &str, object_name: &str| {
        let rank = if query.is_empty() {
            MatchRank::Prefix
        } else {
            match match_rank(object_name, &query) {
                Some(rank) => rank,
                None => return,
            }
        };
        scored.push((
            rank,
            object_name.chars().count(),
            category_position(category),
            ObjectSearchHit::simple(category, object_name.to_string()),
        ));
    };

    for (category, names) in [
        ("TABLES", &cache.tables),
        ("VIEWS", &cache.views),
        ("PROCEDURES", &cache.procedures),
        ("FUNCTIONS", &cache.functions),
        ("PACKAGES", &cache.packages),
        ("SEQUENCES", &cache.sequences),
        ("TRIGGERS", &cache.triggers),
        ("EVENTS", &cache.events),
        ("SYNONYMS", &cache.synonyms),
    ] {
        for name in names.iter() {
            consider(category, name);
        }
    }

    // Package members are searchable by their bare name, but always shown
    // qualified: `PKG.ROUTINE` is what the user needs to recognise which one
    // they picked when several packages export the same routine name.
    let mut package_names: Vec<&String> = cache.package_routines.keys().collect();
    package_names.sort();
    for package_name in package_names {
        let Some(routines) = cache.package_routines.get(package_name) else {
            continue;
        };
        for routine in routines {
            let qualified = qualified_member_name(package_name, &routine.name);
            let rank = if query.is_empty() {
                Some(MatchRank::Prefix)
            } else {
                match_rank(&routine.name, &query).or_else(|| match_rank(&qualified, &query))
            };
            let Some(rank) = rank else {
                continue;
            };
            scored.push((
                rank,
                qualified.chars().count(),
                category_position("PACKAGES"),
                ObjectSearchHit::package_member(package_name, routine),
            ));
        }
    }

    scored.sort_by(|left, right| {
        left.0
            .cmp(&right.0)
            .then(left.1.cmp(&right.1))
            .then(left.2.cmp(&right.2))
            .then(left.3.display_name.cmp(&right.3.display_name))
    });
    scored.truncate(limit);
    scored.into_iter().map(|(_, _, _, hit)| hit).collect()
}

fn title_case(value: &str) -> String {
    let mut chars = value.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().collect::<String>() + &chars.as_str().to_lowercase(),
        None => "Object".to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::PackageRoutine;
    use std::collections::HashMap;

    fn cache() -> ObjectCache {
        let mut package_routines = HashMap::new();
        package_routines.insert(
            "PKG_ORDERS".to_string(),
            vec![
                PackageRoutine {
                    name: "PLACE_ORDER".to_string(),
                    routine_type: "PROCEDURE".to_string(),
                },
                PackageRoutine {
                    name: "ORDER_TOTAL".to_string(),
                    routine_type: "FUNCTION".to_string(),
                },
            ],
        );
        ObjectCache {
            tables: vec!["ORDERS".to_string(), "ORDER_ITEMS".to_string()],
            views: vec!["V_ORDERS".to_string()],
            procedures: vec!["REBUILD_ORDERS".to_string()],
            functions: vec!["ORDER_COUNT".to_string()],
            sequences: vec!["ORDER_SEQ".to_string()],
            triggers: Vec::new(),
            events: Vec::new(),
            synonyms: Vec::new(),
            packages: vec!["PKG_ORDERS".to_string()],
            package_routines,
            table_columns: std::collections::HashMap::new(),
        }
    }

    fn names(query: &str) -> Vec<String> {
        search(&cache(), query, MAX_OBJECT_SEARCH_HITS)
            .into_iter()
            .map(|hit| hit.display_name)
            .collect()
    }

    #[test]
    fn an_exact_name_wins_over_a_longer_prefix_match() {
        let hits = names("orders");
        assert_eq!(hits.first().map(String::as_str), Some("ORDERS"));
    }

    #[test]
    fn a_prefix_match_ranks_above_a_substring_match() {
        let hits = names("order");
        // `ORDER_ITEMS` is the longest prefix match and `V_ORDERS` the shortest
        // substring match, so length cannot be what puts the prefix first.
        let prefix_position = hits
            .iter()
            .position(|name| name == "ORDER_ITEMS")
            .expect("prefix match missing");
        let substring_position = hits
            .iter()
            .position(|name| name == "V_ORDERS")
            .expect("substring match missing");
        assert!(prefix_position < substring_position);
    }

    #[test]
    fn matching_ignores_case_in_both_directions() {
        assert!(names("ORDER_ITEMS").contains(&"ORDER_ITEMS".to_string()));
        assert!(names("order_items").contains(&"ORDER_ITEMS".to_string()));
    }

    #[test]
    fn every_category_is_searched() {
        let hits = names("order");
        for expected in [
            "ORDERS",
            "V_ORDERS",
            "REBUILD_ORDERS",
            "ORDER_COUNT",
            "ORDER_SEQ",
            "PKG_ORDERS",
        ] {
            assert!(hits.contains(&expected.to_string()), "missing {expected}");
        }
    }

    #[test]
    fn a_package_member_is_found_by_its_bare_name_and_shown_qualified() {
        assert!(names("place_order").contains(&"PKG_ORDERS.PLACE_ORDER".to_string()));
    }

    #[test]
    fn a_package_member_is_found_by_its_qualified_name() {
        assert!(names("pkg_orders.order_total").contains(&"PKG_ORDERS.ORDER_TOTAL".to_string()));
    }

    #[test]
    fn an_empty_query_lists_everything() {
        assert_eq!(names("").len(), 9);
    }

    #[test]
    fn a_blank_query_is_treated_as_empty() {
        assert_eq!(names("   ").len(), 9);
    }

    #[test]
    fn a_name_that_matches_nothing_returns_nothing() {
        assert!(names("zzz").is_empty());
    }

    #[test]
    fn the_limit_is_honored() {
        assert_eq!(search(&cache(), "order", 2).len(), 2);
    }

    #[test]
    fn shorter_names_come_first_within_the_same_rank() {
        let hits = names("order");
        let short = hits.iter().position(|name| name == "ORDERS");
        let long = hits.iter().position(|name| name == "ORDER_ITEMS");
        assert!(short < long);
    }

    #[test]
    fn a_simple_hit_converts_to_a_simple_object_item() {
        let hit = search(&cache(), "orders", 1).remove(0);
        match hit.to_object_item() {
            ObjectItem::Simple {
                object_type,
                object_name,
            } => {
                assert_eq!(object_type, "TABLES");
                assert_eq!(object_name, "ORDERS");
            }
            _ => panic!("expected a simple object"),
        }
    }

    #[test]
    fn a_package_member_hit_converts_to_a_package_routine_item() {
        let hit = search(&cache(), "place_order", 1).remove(0);
        match hit.to_object_item() {
            ObjectItem::PackageRoutine {
                package_name,
                routine_name,
                routine_type,
            } => {
                assert_eq!(package_name, "PKG_ORDERS");
                assert_eq!(routine_name, "PLACE_ORDER");
                assert_eq!(routine_type, "PROCEDURE");
            }
            _ => panic!("expected a package routine"),
        }
    }

    #[test]
    fn the_package_itself_stays_a_simple_object() {
        let hit = search(&cache(), "pkg_orders", 1).remove(0);
        assert!(matches!(hit.to_object_item(), ObjectItem::Simple { .. }));
    }

    #[test]
    fn each_hit_carries_a_readable_kind_label() {
        let hit = search(&cache(), "v_orders", 1).remove(0);
        assert_eq!(hit.kind_label, "View");
        let member = search(&cache(), "order_total", 1).remove(0);
        assert_eq!(member.kind_label, "Function");
    }

    #[test]
    fn a_browser_line_is_name_then_kind() {
        let hit = search(&cache(), "orders", 1).remove(0);
        assert_eq!(hit.browser_line(), "ORDERS\tTable");
    }

    #[test]
    fn a_name_starting_with_the_format_character_is_escaped() {
        let hit = ObjectSearchHit::simple("TABLES", "@RATE".to_string());
        assert_eq!(hit.browser_line(), "@@RATE\tTable");
    }

    #[test]
    fn a_format_character_inside_a_name_is_left_alone() {
        // FLTK stops parsing at the first non-format character, so an interior
        // `@` is already literal — doubling it would show two.
        let hit = ObjectSearchHit::simple("TABLES", "DB@LINK".to_string());
        assert_eq!(hit.browser_line(), "DB@LINK\tTable");
    }

    /// A `.` is legal INSIDE a catalog name, so it cannot be read as the
    /// separator between a package and its member. Splitting the display text
    /// on the first one turned a package called `MY.PKG` into a member `PKG`
    /// of a package `MY` — an object that does not exist — and the action that
    /// followed opened whatever `MY` happened to be.
    #[test]
    fn a_dot_inside_a_package_name_does_not_make_it_a_member() {
        let mut cache = cache();
        cache.packages.push("MY.PKG".to_string());

        let hit = search(&cache, "my.pkg", MAX_OBJECT_SEARCH_HITS)
            .into_iter()
            .find(|hit| hit.display_name == "MY.PKG")
            .expect("the dotted package is searchable");

        assert_eq!(
            hit.to_object_item(),
            ObjectItem::Simple {
                object_type: "PACKAGES".to_string(),
                object_name: "MY.PKG".to_string(),
            }
        );
    }

    /// The member of such a package keeps BOTH names whole — the identity is
    /// built from the two the cache already holds apart, never re-split out of
    /// the `MY.PKG.RUN` the list shows.
    #[test]
    fn a_member_of_a_dotted_package_keeps_both_names_whole() {
        let mut cache = cache();
        cache.packages.push("MY.PKG".to_string());
        cache.package_routines.insert(
            "MY.PKG".to_string(),
            vec![PackageRoutine {
                name: "RUN".to_string(),
                routine_type: "PROCEDURE".to_string(),
            }],
        );

        let hit = search(&cache, "my.pkg.run", MAX_OBJECT_SEARCH_HITS)
            .into_iter()
            .find(|hit| hit.display_name == "MY.PKG.RUN")
            .expect("the dotted package's member is searchable");

        assert_eq!(
            hit.to_object_item(),
            ObjectItem::PackageRoutine {
                package_name: "MY.PKG".to_string(),
                routine_name: "RUN".to_string(),
                routine_type: "PROCEDURE".to_string(),
            }
        );
    }

    /// The routine kind travels from the catalog row, not back out of the
    /// title-cased label the list shows. A catalog value the label mapper does
    /// not know (`title_case("")` answers `Object`) used to become the item's
    /// routine type.
    #[test]
    fn the_routine_kind_comes_from_the_catalog_not_from_the_label() {
        let mut cache = cache();
        cache.package_routines.insert(
            "PKG_ODD".to_string(),
            vec![PackageRoutine {
                name: "MYSTERY".to_string(),
                routine_type: String::new(),
            }],
        );

        let hit = search(&cache, "mystery", MAX_OBJECT_SEARCH_HITS)
            .into_iter()
            .find(|hit| hit.display_name == "PKG_ODD.MYSTERY")
            .expect("the member is searchable");

        assert_eq!(hit.kind_label, "Object");
        match hit.to_object_item() {
            // `UNKNOWN` is the one value that means "ask the server which kind
            // this is"; `OBJECT` — what the label round trip produced — means
            // nothing to any consumer, so its menu entry matched no action.
            ObjectItem::PackageRoutine { routine_type, .. } => {
                assert_eq!(routine_type, "UNKNOWN")
            }
            other => panic!("expected a package routine, got {other:?}"),
        }
    }

    #[test]
    fn a_large_scope_searches_fast_enough_for_every_keystroke() {
        // Some Oracle schemas really do hold tens of thousands of objects, and
        // this runs on the UI thread once per keystroke.
        let big = ObjectCache {
            tables: (0..50_000).map(|n| format!("TBL_{n:06}")).collect(),
            ..ObjectCache::default()
        };
        for (label, query) in [("empty", ""), ("prefix", "tbl_0001"), ("miss", "zzzz")] {
            let started = std::time::Instant::now();
            let hits = search(&big, query, MAX_OBJECT_SEARCH_HITS);
            let elapsed = started.elapsed();
            println!(
                "{label:>6} query over 50k objects: {elapsed:?} ({} hits)",
                hits.len()
            );
            assert!(
                elapsed < std::time::Duration::from_secs(2),
                "{label} query took {elapsed:?}"
            );
        }
    }

    #[test]
    fn an_empty_cache_searches_without_panicking() {
        assert!(search(&ObjectCache::default(), "anything", 10).is_empty());
        assert!(search(&ObjectCache::default(), "", 10).is_empty());
    }
}

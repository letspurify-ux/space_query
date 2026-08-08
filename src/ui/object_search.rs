//! Name search over the object metadata the browser has already cached.
//!
//! This is deliberately *not* a server query. The tree already holds every
//! object of the connection's current scope, so a search over it answers
//! instantly and can never disagree with what the tree shows. What it adds over
//! the tree's filter box is a flat, ranked list that can be driven entirely from
//! the keyboard: type a few letters, press Enter, land in the source.

use crate::ui::object_browser::{ObjectCache, ObjectItem};

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

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ObjectSearchHit {
    /// Tree category in upper case, e.g. `TABLES` — the same string
    /// `ObjectItem::Simple::object_type` carries.
    pub category: String,
    /// Name as the user should read it: `PKG.ROUTINE` for a package member.
    pub display_name: String,
    /// Single-word kind shown next to the name (`Table`, `Procedure`, …).
    pub kind_label: String,
}

impl ObjectSearchHit {
    pub fn to_object_item(&self) -> ObjectItem {
        match self.display_name.split_once('.') {
            Some((package_name, routine_name)) if self.category == "PACKAGES" => {
                ObjectItem::PackageRoutine {
                    package_name: package_name.to_string(),
                    routine_name: routine_name.to_string(),
                    routine_type: self.kind_label.to_uppercase(),
                }
            }
            _ => ObjectItem::Simple {
                object_type: self.category.clone(),
                object_name: self.display_name.clone(),
            },
        }
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

    let mut consider = |category: &str, display_name: String, matched_on: &str| {
        let rank = if query.is_empty() {
            MatchRank::Prefix
        } else {
            match match_rank(matched_on, &query) {
                Some(rank) => rank,
                None => return,
            }
        };
        scored.push((
            rank,
            display_name.chars().count(),
            category_position(category),
            ObjectSearchHit {
                category: category.to_string(),
                display_name,
                kind_label: kind_label(category).to_string(),
            },
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
            consider(category, name.clone(), name);
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
            let qualified = format!("{}.{}", package_name, routine.name);
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
                ObjectSearchHit {
                    category: "PACKAGES".to_string(),
                    display_name: qualified,
                    kind_label: title_case(&routine.routine_type),
                },
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
        let hit = ObjectSearchHit {
            category: "TABLES".to_string(),
            display_name: "@RATE".to_string(),
            kind_label: "Table".to_string(),
        };
        assert_eq!(hit.browser_line(), "@@RATE\tTable");
    }

    #[test]
    fn a_format_character_inside_a_name_is_left_alone() {
        // FLTK stops parsing at the first non-format character, so an interior
        // `@` is already literal — doubling it would show two.
        let hit = ObjectSearchHit {
            category: "TABLES".to_string(),
            display_name: "DB@LINK".to_string(),
            kind_label: "Table".to_string(),
        };
        assert_eq!(hit.browser_line(), "DB@LINK\tTable");
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

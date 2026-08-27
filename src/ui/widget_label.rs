//! Names, made safe for widgets that PARSE the string they are handed.
//!
//! Three widgets in this app read structure out of a label, and every one of
//! them was being handed a NAME the app did not author — a table column, a
//! schema, a connection the user typed, an object out of the catalog:
//!
//! - An FLTK menu label is parsed: `&` marks an accelerator, `/` opens a
//!   submenu, `_` draws a divider, `\` escapes the next character. A `/` in a
//!   name therefore turns ONE entry into a submenu and changes how many items
//!   the menu holds.
//! - `fltk::menu::MenuExt::add_choice` splits its argument on `|` BEFORE FLTK
//!   sees it, and no escape reaches that split — `\|` still becomes two items,
//!   the first ending in a stray backslash (measured). [`add_menu_item`] uses
//!   `add`, which does not split, so a name may hold a `|` at all.
//! - An FLTK tree path is split on `/`, so a table named `A/B` became a folder
//!   `A` holding a leaf `B` — and MERGED with a real table called `A`, leaving
//!   the actual table unreachable in the browser (measured).
//!
//! Why any of that matters beyond looks: a menu selection is resolved by the
//! item's INDEX, or by reading its text back and matching it to a name. Both
//! break when one name becomes two items. The user picks a column and the
//! import writes into the next one; the user picks a schema and the browser
//! moves to another.
//!
//! **What is deliberately NOT escaped: `@`.** FLTK draws a label whose text has
//! an `@` in it as a SYMBOL rather than as text — `a@b` measures the width of
//! `a` plus a 16-pixel glyph, while the literal reads `a@@b` (measured). Two
//! reasons to leave it: it is a DRAW-time rule, so it changes neither the item
//! count nor the text the widget stores, and therefore corrupts no identity;
//! and doubling it WOULD, because `@@` survives into the stored text
//! (`Choice::text` gives back `@@circle`, not `@circle`) and the scope picker
//! resolves its selection by comparing that text to a schema name. Encoding it
//! safely means taking identity off the widget entirely, which is a larger
//! change than the defects above. A name holding `@` still selects the right
//! thing; it draws wrong, and that is the honest limit here.

use fltk::prelude::MenuExt;

/// `name`, escaped so an FLTK menu draws it as itself and holds it as ONE item.
///
/// Exactly the four characters FLTK's own menu parser consumes. Escaping them
/// is transparent to identity: the widget stores the UNESCAPED text, so
/// `Choice::text` gives back the name for every one of them (measured), which
/// is what the pickers that match a label to a name depend on.
pub fn menu_item_label(name: &str) -> String {
    let mut escaped = String::with_capacity(name.len());
    for ch in name.chars() {
        if matches!(ch, '&' | '/' | '\\' | '_') {
            escaped.push('\\');
        }
        escaped.push(ch);
    }
    escaped
}

/// Add `name` to `menu` as exactly ONE item, whatever the text is.
///
/// The only way this app should put a name in a menu. `add_choice` cannot do
/// it — it splits on `|` before any escape applies — so the API choice is part
/// of the rule rather than something a call site has to remember.
///
/// The item's callback FORWARDS to the widget's, and that is not decoration:
/// `MenuExt::add` always installs one, and FLTK dispatches a pick as
/// `if (value_->callback_) value_->do_callback(this); else do_callback();`
/// (`Fl_Menu_.cxx`). An item callback therefore SUPPRESSES the widget's — the
/// pickers here all set theirs on the widget, so an empty item callback would
/// have silently stopped the import dialog refreshing its summary, the
/// connection dropdown switching cards, and the scope picker reacting at all.
pub fn add_menu_item<M: MenuExt>(menu: &mut M, name: &str) {
    menu.add(
        &menu_item_label(name),
        fltk::enums::Shortcut::None,
        fltk::menu::MenuFlag::Normal,
        |picked| picked.do_callback(),
    );
}

/// `name`, escaped so it is ONE segment of an FLTK tree path.
///
/// A tree path is split on `/`, and `\` escapes it. The item's own label comes
/// back unescaped, so everything that reads an object's name off a tree item
/// keeps reading the name.
pub fn tree_path_segment(name: &str) -> String {
    let mut escaped = String::with_capacity(name.len());
    for ch in name.chars() {
        if matches!(ch, '/' | '\\') {
            escaped.push('\\');
        }
        escaped.push(ch);
    }
    escaped
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_menu_label_escapes_what_fltk_parses_and_nothing_else() {
        assert_eq!(menu_item_label("a&b/c_d\\e"), r"a\&b\/c\_d\\e");
        // `|` is not escaped: no escape survives `add_choice`'s split, and
        // `add` — which is what `add_menu_item` uses — does not split at all.
        assert_eq!(menu_item_label("a|b"), "a|b");
        // `@` is a draw-time symbol, not something the parser consumes.
        assert_eq!(menu_item_label("a@b"), "a@b");
        assert_eq!(menu_item_label("PLAIN"), "PLAIN");
    }

    #[test]
    fn a_tree_segment_escapes_the_path_separator() {
        assert_eq!(tree_path_segment("A/B"), r"A\/B");
        assert_eq!(tree_path_segment(r"A\B"), r"A\\B");
        assert_eq!(tree_path_segment("PLAIN"), "PLAIN");
        // A menu's accelerator and divider characters are not a tree's; a name
        // holding them is one segment already.
        assert_eq!(tree_path_segment("a&b_c"), "a&b_c");
    }
}

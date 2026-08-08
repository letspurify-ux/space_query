//! Code snippets (live templates): an abbreviation the editor expands into a
//! statement skeleton whose placeholders `Tab` walks through.
//!
//! The expansion itself is pure text work and lives here; the editor owns the
//! caret and the session state.
//!
//! Placeholders are written `${name}` in the template body. Nothing else in a
//! body is special, and a body never contains a literal `${`.

use crate::ui::sql_editor::SqlEditorWidget;
use crate::ui::text_buffer_access;
use fltk::prelude::*;

/// One template: the word typed in the editor, and the body it becomes.
pub(crate) struct Snippet {
    pub(crate) abbreviation: &'static str,
    pub(crate) description: &'static str,
    pub(crate) body: &'static str,
}

/// The built-in templates, in the order the reference dialog lists them.
///
/// Bodies are written the way the formatter writes SQL — uppercase keywords,
/// one clause per line — so an expansion does not need reformatting.
pub(crate) const SNIPPETS: &[Snippet] = &[
    Snippet {
        abbreviation: "sel",
        description: "SELECT with a WHERE clause",
        body: "SELECT *\nFROM ${table}\nWHERE ${condition}",
    },
    Snippet {
        abbreviation: "selc",
        description: "Row count",
        body: "SELECT COUNT(*)\nFROM ${table}\nWHERE ${condition}",
    },
    Snippet {
        abbreviation: "ins",
        description: "INSERT with an explicit column list",
        body: "INSERT INTO ${table} (${columns})\nVALUES (${values})",
    },
    Snippet {
        abbreviation: "upd",
        description: "UPDATE with a WHERE clause",
        body: "UPDATE ${table}\nSET ${assignment}\nWHERE ${condition}",
    },
    Snippet {
        abbreviation: "del",
        description: "DELETE with a WHERE clause",
        body: "DELETE FROM ${table}\nWHERE ${condition}",
    },
    Snippet {
        abbreviation: "join",
        description: "Inner join with an alias",
        body: "JOIN ${table} ${alias} ON ${condition}",
    },
    Snippet {
        abbreviation: "ljoin",
        description: "Left outer join with an alias",
        body: "LEFT JOIN ${table} ${alias} ON ${condition}",
    },
    Snippet {
        abbreviation: "case",
        description: "CASE expression",
        body: "CASE WHEN ${condition} THEN ${value} ELSE ${default} END",
    },
    Snippet {
        abbreviation: "ct",
        description: "CREATE TABLE",
        body: "CREATE TABLE ${table} (\n    ${column} ${data_type}\n)",
    },
    Snippet {
        abbreviation: "beg",
        description: "Anonymous PL/SQL block",
        body: "BEGIN\n    ${statement};\nEND;",
    },
    Snippet {
        abbreviation: "ife",
        description: "PL/SQL IF statement",
        body: "IF ${condition} THEN\n    ${statement};\nEND IF;",
    },
    Snippet {
        abbreviation: "forl",
        description: "PL/SQL numeric FOR loop",
        body: "FOR ${index} IN 1 .. ${bound} LOOP\n    ${statement};\nEND LOOP;",
    },
];

/// The template `word` triggers, matched without regard to case.
pub(crate) fn snippet_for(word: &str) -> Option<&'static Snippet> {
    if word.is_empty() {
        return None;
    }
    SNIPPETS
        .iter()
        .find(|snippet| snippet.abbreviation.eq_ignore_ascii_case(word))
}

/// What the editor inserts, plus what `Tab` needs to find the placeholders
/// again after the user has typed over the ones before them.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct SnippetExpansion {
    /// The text that replaces the abbreviation.
    pub(crate) text: String,
    /// Byte range of the first placeholder inside `text`, which the editor
    /// selects so typing replaces it. `None` for a body without placeholders.
    pub(crate) first_placeholder: Option<(usize, usize)>,
    /// The placeholders after the first, each with the literal text that
    /// separates it from the one before.
    pub(crate) remaining: Vec<SnippetPlaceholder>,
    /// The literal text after the last placeholder. `Tab` past the last
    /// placeholder puts the caret at the end of it.
    pub(crate) tail: String,
}

/// A placeholder the editor has yet to visit.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct SnippetPlaceholder {
    /// Literal template text between the previous placeholder and this one.
    /// The editor searches for it forward from the caret, which is how the
    /// placeholder is relocated after the text before it changed length.
    pub(crate) separator: String,
    /// The name the placeholder was expanded with, still in the buffer unless
    /// the user typed over it.
    pub(crate) default: String,
}

/// Split a template body into its literal parts and placeholder names.
///
/// A `${` that is never closed is literal text, so a malformed body degrades
/// to plain insertion instead of eating the rest of the template.
fn split_body(body: &str) -> (Vec<String>, Vec<String>) {
    let mut literals = vec![String::new()];
    let mut placeholders = Vec::new();
    let mut rest = body;
    while let Some(open) = rest.find("${") {
        let Some(close) = rest[open + 2..].find('}') else {
            break;
        };
        let name = &rest[open + 2..open + 2 + close];
        if let Some(literal) = literals.last_mut() {
            literal.push_str(&rest[..open]);
        }
        placeholders.push(name.to_string());
        literals.push(String::new());
        rest = &rest[open + 2 + close + 1..];
    }
    if let Some(literal) = literals.last_mut() {
        literal.push_str(rest);
    }
    (literals, placeholders)
}

/// Re-indent a template body so its continuation lines line up with the line
/// the abbreviation was typed on.
fn indent_body(body: &str, indent: &str) -> String {
    if indent.is_empty() || !body.contains('\n') {
        return body.to_string();
    }
    body.replace('\n', &format!("\n{indent}"))
}

/// The text and placeholder plan for `snippet`, indented to `indent`.
pub(crate) fn expand(snippet: &Snippet, indent: &str) -> SnippetExpansion {
    let (literals, placeholder_names) = split_body(&indent_body(snippet.body, indent));

    let mut text = String::new();
    let mut first_placeholder = None;
    let mut remaining = Vec::new();
    for (index, name) in placeholder_names.iter().enumerate() {
        let separator = literals.get(index).cloned().unwrap_or_default();
        text.push_str(&separator);
        if index == 0 {
            first_placeholder = Some((text.len(), name.len()));
        } else {
            remaining.push(SnippetPlaceholder {
                separator,
                default: name.clone(),
            });
        }
        text.push_str(name);
    }
    let tail = literals
        .get(placeholder_names.len())
        .cloned()
        .unwrap_or_default();
    text.push_str(&tail);

    SnippetExpansion {
        text,
        first_placeholder,
        remaining,
        tail,
    }
}

/// Where the next placeholder sits now, searching forward from `origin`.
///
/// The text before the placeholder has usually changed length by the time
/// `Tab` is pressed — the user just typed over the previous placeholder — so
/// the separator literal is what re-anchors it. `None` means the separator is
/// no longer there (the user edited the template apart), and the caller ends
/// the session rather than guessing.
pub(crate) fn locate_placeholder(
    text: &str,
    origin: usize,
    placeholder: &SnippetPlaceholder,
) -> Option<(usize, usize)> {
    let start = find_separator_end(text, origin, &placeholder.separator)?;
    let end = start + placeholder.default.len();
    if text.get(start..end) == Some(placeholder.default.as_str()) {
        Some((start, end))
    } else {
        // The default is gone (the user typed over it before tabbing here), so
        // there is nothing to select — just put the caret where it began.
        Some((start, start))
    }
}

/// Where the caret goes when `Tab` leaves the last placeholder: past the
/// template's trailing literal, or where it already is when there is none.
pub(crate) fn locate_tail_end(text: &str, origin: usize, tail: &str) -> Option<usize> {
    find_separator_end(text, origin, tail)
}

fn find_separator_end(text: &str, origin: usize, separator: &str) -> Option<usize> {
    if origin > text.len() {
        return None;
    }
    if separator.is_empty() {
        return Some(origin);
    }
    let offset = text.get(origin..)?.find(separator)?;
    Some(origin + offset + separator.len())
}

/// The template the editor is standing in the middle of.
///
/// Only what `Tab` still has to do is kept: the placeholders not yet visited
/// and the literal that ends the body. Positions are deliberately absent —
/// they would be wrong the moment the user typed — so each step re-anchors
/// itself on the separator literal ahead of the caret.
pub(crate) struct SnippetSession {
    remaining: Vec<SnippetPlaceholder>,
    tail: String,
    /// Start of the placeholder the last step selected. A `Tab` pressed before
    /// it means the user has moved out of the template, and the session ends
    /// instead of dragging the caret forward from wherever they now are.
    anchor: i32,
}

/// How far ahead of the caret a placeholder is looked for. A template body is
/// short; this only has to also cover whatever the user typed into the
/// placeholders before it.
const SNIPPET_SEARCH_WINDOW: i32 = 65_536;

/// The trailing run of identifier characters in `text` — the abbreviation the
/// user just typed.
fn trailing_word(text: &str) -> &str {
    let start = text
        .char_indices()
        .rev()
        .take_while(|(_, character)| character.is_ascii_alphanumeric() || *character == '_')
        .last()
        .map_or(text.len(), |(index, _)| index);
    &text[start..]
}

/// The whitespace `text` begins with, which continuation lines inherit.
fn leading_whitespace(text: &str) -> &str {
    let end = text
        .find(|character: char| !character.is_whitespace())
        .unwrap_or(text.len());
    &text[..end]
}

impl SqlEditorWidget {
    /// Replace the abbreviation before the cursor with its template and select
    /// the first placeholder.
    ///
    /// Returns false when the word before the cursor is not an abbreviation,
    /// so the caller can fall back to whatever the key normally does.
    pub(crate) fn expand_snippet_at_cursor(&self) -> bool {
        let mut buffer = self.buffer.clone();
        let mut editor = self.editor.clone();
        let caret = editor.insert_position();
        let line_start =
            text_buffer_access::line_start(&buffer, Some(&self.highlight_shadow), caret);
        let line_prefix = text_buffer_access::text_range(
            &buffer,
            Some(&self.highlight_shadow),
            line_start,
            caret,
        );
        let word = trailing_word(&line_prefix);
        let Some(snippet) = snippet_for(word) else {
            return false;
        };
        let expansion = expand(snippet, leading_whitespace(&line_prefix));
        let Ok(word_len) = i32::try_from(word.len()) else {
            return false;
        };

        let start = caret - word_len;
        buffer.replace(start, caret, &expansion.text);
        let anchor = match expansion.first_placeholder {
            Some((offset, length)) => {
                let placeholder_start = start + offset as i32;
                let placeholder_end = placeholder_start + length as i32;
                buffer.select(placeholder_start, placeholder_end);
                editor.set_insert_position(placeholder_end);
                placeholder_start
            }
            None => {
                let end = start + expansion.text.len() as i32;
                editor.set_insert_position(end);
                end
            }
        };
        editor.show_insert_position();

        let has_more = !expansion.remaining.is_empty() || !expansion.tail.is_empty();
        let session = has_more.then_some(SnippetSession {
            remaining: expansion.remaining,
            tail: expansion.tail,
            anchor,
        });
        *self
            .snippet_session
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = session;

        true
    }

    /// Whether a template is open, and so `Tab` belongs to it.
    pub(crate) fn snippet_session_is_active(&self) -> bool {
        self.snippet_session
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .is_some()
    }

    /// Move to the next placeholder, or past the end of the template when the
    /// last one has been visited.
    ///
    /// Returns false when the template can no longer be followed — the user
    /// edited its literal text away, or moved the cursor out of it — after
    /// ending the session, so `Tab` goes back to meaning what it always did.
    pub(crate) fn advance_snippet_placeholder(&self) -> bool {
        let Some(mut session) = self
            .snippet_session
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .take()
        else {
            return false;
        };
        let mut buffer = self.buffer.clone();
        let mut editor = self.editor.clone();
        let caret = editor.insert_position();
        let origin = match buffer.selection_position() {
            Some((start, end)) => caret.max(start).max(end),
            None => caret,
        };
        if origin < session.anchor {
            return false;
        }

        let (window, window_start) = text_buffer_access::bounded_text_window(
            &buffer,
            Some(&self.highlight_shadow),
            origin,
            origin.saturating_add(SNIPPET_SEARCH_WINDOW),
        );
        let Ok(relative_origin) = usize::try_from(origin - window_start) else {
            return false;
        };

        if session.remaining.is_empty() {
            let Some(end) = locate_tail_end(&window, relative_origin, &session.tail) else {
                return false;
            };
            buffer.unselect();
            editor.set_insert_position(window_start + end as i32);
            editor.show_insert_position();
            return true;
        }

        let placeholder = session.remaining.remove(0);
        let Some((start, end)) = locate_placeholder(&window, relative_origin, &placeholder) else {
            return false;
        };
        let start = window_start + start as i32;
        let end = window_start + end as i32;
        if start == end {
            buffer.unselect();
        } else {
            buffer.select(start, end);
        }
        editor.set_insert_position(end);
        editor.show_insert_position();

        session.anchor = start;
        if !session.remaining.is_empty() || !session.tail.is_empty() {
            *self
                .snippet_session
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(session);
        }
        true
    }

    /// Leave the template. The text stays as it is; only the `Tab` binding
    /// goes back to normal.
    pub(crate) fn cancel_snippet_session(&self) {
        *self
            .snippet_session
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = None;
    }
}

/// The list the `Code Snippets` help dialog shows.
pub(crate) fn reference_text() -> String {
    let mut text = String::from(
        "Code Snippets (live templates)\n\n\
         Type the abbreviation in the SQL editor and press Tab to expand it.\n\
         Ctrl+J expands it even while the completion popup is open, and opens\n\
         this list when there is no abbreviation before the cursor.\n\n\
         Tab moves to the next placeholder, Esc leaves the template.\n\n",
    );
    for snippet in SNIPPETS {
        text.push_str(&format!(
            "{}  -  {}\n",
            snippet.abbreviation, snippet.description
        ));
        for line in snippet.body.lines() {
            text.push_str(&format!("    {line}\n"));
        }
        text.push('\n');
    }
    text
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn abbreviations_are_matched_without_regard_to_case() {
        assert_eq!(snippet_for("SEL").map(|s| s.abbreviation), Some("sel"));
        assert_eq!(snippet_for("sel").map(|s| s.abbreviation), Some("sel"));
        assert!(snippet_for("select").is_none());
        assert!(snippet_for("").is_none());
    }

    #[test]
    fn abbreviations_are_unique() {
        let mut seen = Vec::new();
        for snippet in SNIPPETS {
            assert!(
                !seen.contains(&snippet.abbreviation),
                "duplicate abbreviation {}",
                snippet.abbreviation
            );
            seen.push(snippet.abbreviation);
        }
    }

    #[test]
    fn every_body_has_at_least_one_placeholder() {
        for snippet in SNIPPETS {
            let expansion = expand(snippet, "");
            assert!(
                expansion.first_placeholder.is_some(),
                "{} expands without a placeholder",
                snippet.abbreviation
            );
        }
    }

    #[test]
    fn expansion_inserts_placeholder_names_and_selects_the_first() {
        let snippet = snippet_for("sel").expect("sel");
        let expansion = expand(snippet, "");

        assert_eq!(expansion.text, "SELECT *\nFROM table\nWHERE condition");
        let (start, len) = expansion.first_placeholder.expect("first placeholder");
        assert_eq!(&expansion.text[start..start + len], "table");
        assert_eq!(
            expansion.remaining,
            vec![SnippetPlaceholder {
                separator: "\nWHERE ".to_string(),
                default: "condition".to_string(),
            }]
        );
        assert_eq!(expansion.tail, "");
    }

    #[test]
    fn a_trailing_literal_becomes_the_tail() {
        let snippet = snippet_for("beg").expect("beg");
        let expansion = expand(snippet, "");

        assert_eq!(expansion.text, "BEGIN\n    statement;\nEND;");
        assert_eq!(expansion.tail, ";\nEND;");
    }

    #[test]
    fn continuation_lines_take_the_indent_of_the_line_the_abbreviation_was_on() {
        let snippet = snippet_for("sel").expect("sel");
        let expansion = expand(snippet, "    ");

        assert_eq!(
            expansion.text,
            "SELECT *\n    FROM table\n    WHERE condition"
        );
        assert_eq!(expansion.remaining[0].separator, "\n    WHERE ".to_string());
    }

    #[test]
    fn a_single_line_body_ignores_the_indent() {
        let snippet = snippet_for("join").expect("join");
        assert_eq!(expand(snippet, "        ").text, expand(snippet, "").text);
    }

    #[test]
    fn placeholders_are_relocated_after_the_text_before_them_changed_length() {
        let placeholder = SnippetPlaceholder {
            separator: "\nWHERE ".to_string(),
            default: "condition".to_string(),
        };
        // The user replaced `table` with a much longer name and pressed Tab.
        let text = "SELECT *\nFROM warehouse_inventory_snapshot\nWHERE condition";
        let caret = text.find("\nWHERE").expect("where");

        let located = locate_placeholder(text, caret, &placeholder).expect("located");
        assert_eq!(&text[located.0..located.1], "condition");
    }

    #[test]
    fn a_placeholder_whose_default_is_gone_gets_an_empty_caret_position() {
        let placeholder = SnippetPlaceholder {
            separator: "\nWHERE ".to_string(),
            default: "condition".to_string(),
        };
        let text = "SELECT *\nFROM emp\nWHERE ";

        let located = locate_placeholder(text, 0, &placeholder).expect("located");
        assert_eq!(located, (text.len(), text.len()));
    }

    #[test]
    fn a_separator_the_user_deleted_ends_the_session() {
        let placeholder = SnippetPlaceholder {
            separator: "\nWHERE ".to_string(),
            default: "condition".to_string(),
        };
        assert_eq!(
            locate_placeholder("SELECT * FROM emp", 0, &placeholder),
            None
        );
    }

    #[test]
    fn the_search_never_looks_behind_the_caret() {
        let placeholder = SnippetPlaceholder {
            separator: " ON ".to_string(),
            default: "condition".to_string(),
        };
        let text = "JOIN dept d ON condition";
        let before = text.find(" ON ").expect("on");

        assert!(locate_placeholder(text, before + 1, &placeholder).is_none());
    }

    #[test]
    fn the_tail_moves_the_caret_past_the_template() {
        let text = "BEGIN\n    do_it();\nEND;";
        let caret = text.find("();").expect("call") + 2;

        let end = locate_tail_end(text, caret, ";\nEND;").expect("tail");
        assert_eq!(end, text.len());
        assert_eq!(locate_tail_end(text, caret, ""), Some(caret));
        assert_eq!(locate_tail_end(text, text.len() + 1, ""), None);
    }

    #[test]
    fn an_unclosed_placeholder_stays_literal() {
        let snippet = Snippet {
            abbreviation: "x",
            description: "malformed",
            body: "SELECT ${broken",
        };
        let expansion = expand(&snippet, "");

        assert_eq!(expansion.text, "SELECT ${broken");
        assert_eq!(expansion.first_placeholder, None);
        assert!(expansion.remaining.is_empty());
    }

    #[test]
    fn every_abbreviation_that_a_keyword_also_answers_to_is_reachable_by_tab() {
        // `sel` is an abbreviation here and a prefix of SELECT in the
        // completion popup, so both features want the same keystroke. They
        // split it: Tab expands the template, Enter takes the completion.
        // Listing the overlap here is what makes that split look deliberate to
        // the next reader instead of arbitrary.
        let overlapping: Vec<&str> = SNIPPETS
            .iter()
            .map(|snippet| snippet.abbreviation)
            .filter(|abbreviation| {
                let upper = abbreviation.to_ascii_uppercase();
                crate::sql_text::ORACLE_SQL_KEYWORDS
                    .iter()
                    .any(|keyword| keyword.starts_with(&upper))
            })
            .collect();

        assert!(
            overlapping.contains(&"sel"),
            "`sel` no longer collides with a keyword, so the Tab/Enter split \
             would need a different reason: {overlapping:?}"
        );
        for abbreviation in overlapping {
            assert!(
                snippet_for(abbreviation).is_some(),
                "`{abbreviation}` is shadowed by a keyword and has no template to reach"
            );
        }
    }

    #[test]
    fn the_reference_lists_every_snippet() {
        let text = reference_text();
        for snippet in SNIPPETS {
            assert!(
                text.contains(snippet.abbreviation),
                "{} missing from the reference",
                snippet.abbreviation
            );
            assert!(text.contains(snippet.description));
        }
    }
}

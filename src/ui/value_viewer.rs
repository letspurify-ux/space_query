//! The big view of one grid cell — read it, format it, and (in edit mode) edit
//! it.
//!
//! The grid draws a cell in one clipped line. A CLOB, a JSON document or an XML
//! payload is unreadable there and, before this, uneditable anywhere: the inline
//! editor is one line wide and the old cell popup was a read-only `TextDisplay`.
//!
//! Two rules shape everything below.
//!
//! **Formatting is a view, never an edit.** `Format` shows an indented copy in a
//! read-only buffer; clearing it returns the exact bytes the user was editing.
//! Save always writes the raw buffer. Opening a CLOB, pressing `Format` to read
//! it and then saving can therefore never rewrite the whitespace in the
//! database.
//!
//! **The formatters move whitespace and nothing else.** Neither one parses into
//! a document model and re-serialises it: JSON through `serde_json::Value` would
//! reorder object keys and round-trip long numbers through `f64`, and XML
//! pretty-printers routinely eat significant text. Both here re-indent between
//! tokens they can prove are structural, and leave everything else byte for
//! byte.

use crate::ui::center_on_main;
use crate::ui::constants::{BUTTON_HEIGHT, BUTTON_WIDTH};
use crate::ui::font_settings::FontProfile;
use crate::ui::theme;
use crate::utils::arithmetic::safe_div;
use fltk::{
    app,
    button::{Button, CheckButton},
    enums::{Align, FrameType},
    frame::Frame,
    group::Group,
    prelude::*,
    text::{TextBuffer, TextDisplay, TextEditor, WrapMode},
    window::Window,
};
use std::sync::{Arc, Mutex};

/// How wide one indent step is in the formatted view.
const INDENT: &str = "  ";

/// The shape a value has, as far as the formatters are concerned.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ValueFormat {
    Json,
    Xml,
    Plain,
}

/// What `value` looks like — and therefore whether `Format` does anything.
///
/// This only reports a format when the corresponding formatter accepts the
/// text, so a value that is detected is a value that can be formatted.
pub fn detect_value_format(value: &str) -> ValueFormat {
    let trimmed = value.trim_start();
    // Validate only. Detection runs on the UI thread before the window is
    // drawn, so building the whole indented copy here — and throwing it away —
    // would pay for the formatting twice on exactly the large values this
    // feature exists for. Anything that does not even start like JSON or XML
    // costs one character.
    if trimmed.starts_with('{') || trimmed.starts_with('[') {
        if json_tokens(value).is_some() {
            return ValueFormat::Json;
        }
        return ValueFormat::Plain;
    }
    if trimmed.starts_with('<') {
        if xml_nodes(value).is_some() {
            return ValueFormat::Xml;
        }
        return ValueFormat::Plain;
    }
    ValueFormat::Plain
}

/// The indented form of `value` when it is well-formed JSON, else `None`.
///
/// Whitespace outside string literals is the only thing this rewrites. Every
/// token — every key, every number, every escape — is copied through verbatim,
/// so key order and numeric text survive exactly.
pub fn format_json(value: &str) -> Option<String> {
    let tokens = json_tokens(value)?;
    let mut out = String::with_capacity(value.len() + safe_div(value.len(), 4));
    let mut depth = 0usize;
    let mut previous: Option<&str> = None;

    for token in &tokens {
        let token = token.as_str();
        let is_close = matches!(token, "}" | "]");
        if is_close {
            depth = depth.saturating_sub(1);
        }

        match previous {
            None => {}
            Some(",") => {
                out.push('\n');
                push_indent(&mut out, depth);
            }
            Some(":") => out.push(' '),
            Some(previous_token) => {
                if token == "," || token == ":" {
                    // Nothing between a value and the punctuation that follows.
                } else if matches!(previous_token, "{" | "[")
                    && is_close
                    && json_brackets_match(previous_token, token)
                {
                    // Keep an empty object/array on one line: `{}`, `[]`.
                } else {
                    out.push('\n');
                    push_indent(&mut out, depth);
                }
            }
        }

        out.push_str(token);
        if matches!(token, "{" | "[") {
            depth = depth.saturating_add(1);
        }
        previous = Some(token);
    }

    Some(out)
}

fn json_brackets_match(open: &str, close: &str) -> bool {
    matches!((open, close), ("{", "}") | ("[", "]"))
}

fn push_indent(out: &mut String, depth: usize) {
    for _ in 0..depth {
        out.push_str(INDENT);
    }
}

/// What the scanner will accept next.
///
/// The `*OrClose` variants exist only right after an opening bracket: that is
/// the one position where a closing bracket is legal, and separating it from
/// "just past a comma" is what rejects a trailing comma.
#[derive(Clone, Copy, PartialEq, Eq)]
enum JsonExpect {
    /// A value, and nothing else.
    Value,
    /// A value, or the `]` of an empty array.
    ValueOrClose,
    /// An object key, and nothing else.
    Key,
    /// An object key, or the `}` of an empty object.
    KeyOrClose,
    Colon,
    CommaOrClose,
    /// The one top-level value is complete; anything further is a second
    /// document.
    Done,
}

/// Splits `value` into JSON tokens, or `None` when it is not well-formed JSON.
///
/// This validates as it goes so a caller holding `Some` can re-space the tokens
/// without inventing structure that was not in the input. It is a state machine
/// rather than a set of local checks because the states are what encode the
/// rules that are easy to miss otherwise: an object member is `key : value`,
/// and a closing bracket is legal after an opening one but not after a comma.
fn json_tokens(value: &str) -> Option<Vec<String>> {
    let bytes = value.as_bytes();
    let mut tokens: Vec<String> = Vec::new();
    let mut stack: Vec<u8> = Vec::new();
    let mut index = 0usize;
    let mut expect = JsonExpect::Value;

    // Where the scanner goes after a completed value or container.
    let after_value = |stack: &Vec<u8>| {
        if stack.is_empty() {
            JsonExpect::Done
        } else {
            JsonExpect::CommaOrClose
        }
    };

    while index < bytes.len() {
        let byte = bytes[index];
        if byte.is_ascii_whitespace() {
            index += 1;
            continue;
        }

        match byte {
            b'{' | b'[' => {
                if !matches!(expect, JsonExpect::Value | JsonExpect::ValueOrClose) {
                    return None;
                }
                stack.push(byte);
                tokens.push((byte as char).to_string());
                expect = if byte == b'{' {
                    JsonExpect::KeyOrClose
                } else {
                    JsonExpect::ValueOrClose
                };
                index += 1;
            }
            b'}' => {
                if !matches!(expect, JsonExpect::KeyOrClose | JsonExpect::CommaOrClose)
                    || stack.pop() != Some(b'{')
                {
                    return None;
                }
                tokens.push("}".to_string());
                expect = after_value(&stack);
                index += 1;
            }
            b']' => {
                if !matches!(expect, JsonExpect::ValueOrClose | JsonExpect::CommaOrClose)
                    || stack.pop() != Some(b'[')
                {
                    return None;
                }
                tokens.push("]".to_string());
                expect = after_value(&stack);
                index += 1;
            }
            b',' => {
                if expect != JsonExpect::CommaOrClose {
                    return None;
                }
                tokens.push(",".to_string());
                expect = if stack.last() == Some(&b'{') {
                    JsonExpect::Key
                } else {
                    JsonExpect::Value
                };
                index += 1;
            }
            b':' => {
                if expect != JsonExpect::Colon {
                    return None;
                }
                tokens.push(":".to_string());
                expect = JsonExpect::Value;
                index += 1;
            }
            b'"' => {
                let is_key = matches!(expect, JsonExpect::Key | JsonExpect::KeyOrClose);
                if !is_key && !matches!(expect, JsonExpect::Value | JsonExpect::ValueOrClose) {
                    return None;
                }
                let end = json_string_end(bytes, index)?;
                tokens.push(value.get(index..end)?.to_string());
                expect = if is_key {
                    JsonExpect::Colon
                } else {
                    after_value(&stack)
                };
                index = end;
            }
            _ => {
                if !matches!(expect, JsonExpect::Value | JsonExpect::ValueOrClose) {
                    return None;
                }
                let end = json_scalar_end(bytes, index);
                let literal = value.get(index..end)?;
                if !json_scalar_is_valid(literal) {
                    return None;
                }
                tokens.push(literal.to_string());
                expect = after_value(&stack);
                index = end;
            }
        }
    }

    (stack.is_empty() && expect == JsonExpect::Done).then_some(tokens)
}

/// The index just past the closing quote of the string literal at `start`.
fn json_string_end(bytes: &[u8], start: usize) -> Option<usize> {
    let mut index = start + 1;
    while index < bytes.len() {
        match bytes[index] {
            b'\\' => index += 2,
            b'"' => return Some(index + 1),
            // A raw control character is not legal inside a JSON string.
            control if control < 0x20 => return None,
            _ => index += 1,
        }
    }
    None
}

/// The index just past the bare literal (number, `true`, `false`, `null`) at
/// `start`.
fn json_scalar_end(bytes: &[u8], start: usize) -> usize {
    let mut index = start;
    while index < bytes.len() {
        let byte = bytes[index];
        if byte.is_ascii_whitespace() || matches!(byte, b',' | b':' | b'}' | b']') {
            break;
        }
        index += 1;
    }
    index
}

fn json_scalar_is_valid(literal: &str) -> bool {
    if matches!(literal, "true" | "false" | "null") {
        return true;
    }
    json_number_is_valid(literal)
}

fn json_number_is_valid(literal: &str) -> bool {
    let bytes = literal.as_bytes();
    let mut index = 0usize;
    if bytes.first() == Some(&b'-') {
        index += 1;
    }
    let integer_start = index;
    while bytes.get(index).is_some_and(u8::is_ascii_digit) {
        index += 1;
    }
    if index == integer_start {
        return false;
    }
    // JSON forbids leading zeros ("01"), but allows a bare "0".
    if bytes[integer_start] == b'0' && index - integer_start > 1 {
        return false;
    }
    if bytes.get(index) == Some(&b'.') {
        index += 1;
        let fraction_start = index;
        while bytes.get(index).is_some_and(u8::is_ascii_digit) {
            index += 1;
        }
        if index == fraction_start {
            return false;
        }
    }
    if matches!(bytes.get(index), Some(b'e' | b'E')) {
        index += 1;
        if matches!(bytes.get(index), Some(b'+' | b'-')) {
            index += 1;
        }
        let exponent_start = index;
        while bytes.get(index).is_some_and(u8::is_ascii_digit) {
            index += 1;
        }
        if index == exponent_start {
            return false;
        }
    }
    index == bytes.len()
}

/// The indented form of `value` when it is well-formed XML, else `None`.
///
/// An element whose content is only other elements gets its children put on
/// their own lines. An element that contains any non-whitespace text is copied
/// through untouched, because in XML that whitespace is content and moving it
/// changes the document.
pub fn format_xml(value: &str) -> Option<String> {
    let nodes = xml_nodes(value)?;
    let mut out = String::with_capacity(value.len() + safe_div(value.len(), 4));
    let mut depth = 0usize;
    let mut index = 0usize;

    while index < nodes.len() {
        let node = &nodes[index];
        match node {
            XmlNode::Text(text) => {
                // Whitespace-only text between elements is layout, not content;
                // the indentation replaces it. Anything else is content and is
                // emitted with the element run it belongs to (see below).
                if !text.trim().is_empty() {
                    out.push_str(text);
                }
                index += 1;
            }
            XmlNode::Close(raw) => {
                depth = depth.saturating_sub(1);
                push_xml_line(&mut out, depth, raw);
                index += 1;
            }
            XmlNode::Open(raw) => {
                if let Some(end) = xml_mixed_content_run_end(&nodes, index) {
                    // Mixed content: copy the whole element verbatim.
                    push_xml_line_start(&mut out, depth);
                    for node in &nodes[index..end] {
                        out.push_str(node.raw());
                    }
                    index = end;
                } else {
                    push_xml_line(&mut out, depth, raw);
                    depth = depth.saturating_add(1);
                    index += 1;
                }
            }
            XmlNode::SelfClosing(raw)
            | XmlNode::Comment(raw)
            | XmlNode::Declaration(raw)
            | XmlNode::CData(raw) => {
                push_xml_line(&mut out, depth, raw);
                index += 1;
            }
        }
    }

    Some(out)
}

/// When the element opening at `start` holds non-whitespace text of its own,
/// the index just past its closing tag; otherwise `None`.
///
/// Returning `None` is the signal to indent the element's children. Returning a
/// range is the signal to copy that range through unchanged.
///
/// "Of its own" is the whole subtlety: only text that is a *direct* child
/// counts. A grandchild's text makes that grandchild verbatim, not its
/// ancestors — otherwise a single `<b>1</b>` deep in the tree would force the
/// entire document onto one line.
fn xml_mixed_content_run_end(nodes: &[XmlNode], start: usize) -> Option<usize> {
    let mut depth = 0usize;
    let mut has_direct_text = false;
    for (offset, node) in nodes.iter().enumerate().skip(start) {
        match node {
            XmlNode::Open(_) => depth += 1,
            XmlNode::Close(_) => {
                depth = depth.saturating_sub(1);
                if depth == 0 {
                    return has_direct_text.then_some(offset + 1);
                }
            }
            XmlNode::Text(text) => {
                if depth == 1 && !text.trim().is_empty() {
                    has_direct_text = true;
                }
            }
            XmlNode::SelfClosing(_)
            | XmlNode::Comment(_)
            | XmlNode::Declaration(_)
            | XmlNode::CData(_) => {}
        }
    }
    None
}

fn push_xml_line_start(out: &mut String, depth: usize) {
    if !out.is_empty() {
        out.push('\n');
    }
    push_indent(out, depth);
}

fn push_xml_line(out: &mut String, depth: usize, raw: &str) {
    push_xml_line_start(out, depth);
    out.push_str(raw);
}

#[derive(Debug)]
enum XmlNode {
    Declaration(String),
    Comment(String),
    CData(String),
    Open(String),
    Close(String),
    SelfClosing(String),
    Text(String),
}

impl XmlNode {
    fn raw(&self) -> &str {
        match self {
            Self::Declaration(raw)
            | Self::Comment(raw)
            | Self::CData(raw)
            | Self::Open(raw)
            | Self::Close(raw)
            | Self::SelfClosing(raw)
            | Self::Text(raw) => raw,
        }
    }
}

/// Splits `value` into XML nodes, or `None` when it is not well-formed enough
/// to re-indent (unbalanced tags, stray `<`, more than one root element).
fn xml_nodes(value: &str) -> Option<Vec<XmlNode>> {
    let mut nodes = Vec::new();
    let mut stack: Vec<String> = Vec::new();
    let mut rest = value;
    let mut root_elements = 0usize;

    while !rest.is_empty() {
        if let Some(open_at) = rest.find('<') {
            if open_at > 0 {
                let (text, tail) = rest.split_at(open_at);
                if stack.is_empty() && !text.trim().is_empty() {
                    // Text outside the root element.
                    return None;
                }
                nodes.push(XmlNode::Text(text.to_string()));
                rest = tail;
                continue;
            }
        } else {
            if stack.is_empty() && rest.trim().is_empty() {
                nodes.push(XmlNode::Text(rest.to_string()));
                break;
            }
            return None;
        }

        let (raw, tail) = xml_split_markup(rest)?;
        rest = tail;

        if let Some(body) = raw
            .strip_prefix("<?")
            .and_then(|body| body.strip_suffix("?>"))
        {
            let _ = body;
            nodes.push(XmlNode::Declaration(raw.to_string()));
        } else if raw.starts_with("<!--") {
            nodes.push(XmlNode::Comment(raw.to_string()));
        } else if raw.starts_with("<![CDATA[") {
            nodes.push(XmlNode::CData(raw.to_string()));
        } else if raw.starts_with("<!") {
            nodes.push(XmlNode::Declaration(raw.to_string()));
        } else if let Some(body) = raw.strip_prefix("</") {
            let name = xml_tag_name(body.strip_suffix('>')?)?;
            if stack.pop().as_deref() != Some(name.as_str()) {
                return None;
            }
            nodes.push(XmlNode::Close(raw.to_string()));
        } else if raw.ends_with("/>") {
            if stack.is_empty() {
                root_elements += 1;
                if root_elements > 1 {
                    return None;
                }
            }
            nodes.push(XmlNode::SelfClosing(raw.to_string()));
        } else {
            let body = raw.strip_prefix('<')?.strip_suffix('>')?;
            let name = xml_tag_name(body)?;
            if stack.is_empty() {
                root_elements += 1;
                if root_elements > 1 {
                    return None;
                }
            }
            stack.push(name);
            nodes.push(XmlNode::Open(raw.to_string()));
        }
    }

    if !stack.is_empty() || root_elements != 1 {
        return None;
    }
    Some(nodes)
}

/// Splits the markup starting at `<` from the rest of the document.
///
/// Quoted attribute values may contain `>`, and comments/CDATA end with their
/// own delimiters, so this cannot just look for the next `>`.
fn xml_split_markup(rest: &str) -> Option<(&str, &str)> {
    for (prefix, terminator) in [("<!--", "-->"), ("<![CDATA[", "]]>"), ("<?", "?>")] {
        if rest.starts_with(prefix) {
            let end = rest.find(terminator)? + terminator.len();
            return Some(rest.split_at(end));
        }
    }

    let bytes = rest.as_bytes();
    let mut index = 1usize;
    let mut quote: Option<u8> = None;
    while index < bytes.len() {
        let byte = bytes[index];
        match quote {
            Some(open) if byte == open => quote = None,
            Some(_) => {}
            None if byte == b'"' || byte == b'\'' => quote = Some(byte),
            None if byte == b'>' => return Some(rest.split_at(index + 1)),
            None if byte == b'<' => return None,
            None => {}
        }
        index += 1;
    }
    None
}

fn xml_tag_name(body: &str) -> Option<String> {
    let name: String = body
        .trim_start()
        .trim_end_matches('/')
        .chars()
        .take_while(|ch| !ch.is_whitespace())
        .collect();
    (!name.is_empty()).then_some(name)
}

/// `"12,345 chars · 24,690 bytes"` — the two numbers that decide whether a value
/// still fits where the user is putting it.
pub fn value_size_label(value: &str) -> String {
    format!(
        "{} chars · {} bytes",
        group_digits(value.chars().count()),
        group_digits(value.len())
    )
}

fn group_digits(count: usize) -> String {
    let digits = count.to_string();
    let mut grouped = String::with_capacity(digits.len() + safe_div(digits.len(), 3));
    for (index, digit) in digits.chars().enumerate() {
        if index > 0 && (digits.len() - index).is_multiple_of(3) {
            grouped.push(',');
        }
        grouped.push(digit);
    }
    grouped
}

/// Shared look for whichever text widget the window ends up using.
fn style_value_text_widget<W: DisplayExt>(
    widget: &mut W,
    font_profile: FontProfile,
    font_size: u32,
) {
    widget.set_color(theme::editor_bg());
    widget.set_text_color(theme::text_primary());
    widget.set_text_font(font_profile.normal);
    widget.set_text_size(font_size as i32);
    widget.wrap_mode(WrapMode::AtBounds, 0);
}

/// Opens the value window over `value` and blocks until it closes.
///
/// Returns `Some(new_value)` only when `editable` is set, the user pressed
/// `Save`, and the text actually changed; `None` in every other case, including
/// a save of unchanged text.
pub fn show(
    title: &str,
    value: &str,
    editable: bool,
    font_profile: FontProfile,
    font_size: u32,
) -> Option<String> {
    let current_group = Group::try_current();
    Group::set_current(None::<&Group>);

    let mut dialog = Window::default().with_size(760, 560).with_label(title);
    center_on_main(&mut dialog);
    dialog.set_color(theme::panel_raised());
    dialog.make_modal(true);

    let mut buffer = TextBuffer::default();
    buffer.set_text(value);

    // A read-only value gets a `TextDisplay`, not a disabled `TextEditor`.
    // FLTK has no read-only flag on the editor, and the widget that is already
    // display-only still selects, scrolls and copies — which is everything the
    // viewer needs.
    let mut editor: Option<TextEditor> = None;
    let mut display: Option<TextDisplay> = None;
    if editable {
        let mut widget = TextEditor::new(10, 10, 740, 470, None);
        style_value_text_widget(&mut widget, font_profile, font_size);
        theme::style_text_editor_scrollbars(&widget);
        widget.set_buffer(buffer.clone());
        editor = Some(widget);
    } else {
        let mut widget = TextDisplay::new(10, 10, 740, 470, None);
        style_value_text_widget(&mut widget, font_profile, font_size);
        theme::style_text_display_scrollbars(&widget);
        widget.set_buffer(buffer.clone());
        widget.set_tooltip("This result is not in edit mode, so the value is read-only.");
        display = Some(widget);
    }

    let detected = detect_value_format(value);
    let mut format_check = CheckButton::new(10, 490, 170, BUTTON_HEIGHT, None);
    format_check.set_label_color(theme::text_primary());
    format_check.set_color(theme::button_dark());
    theme::install_button_hover(&mut format_check);
    match detected {
        ValueFormat::Json => format_check.set_label(" Format (JSON)"),
        ValueFormat::Xml => format_check.set_label(" Format (XML)"),
        ValueFormat::Plain => {
            format_check.set_label(" Format");
            format_check.deactivate();
        }
    }
    format_check.set_tooltip(
        "Show an indented copy. This is a view only — saving always writes the value as typed.",
    );

    let mut size_label = Frame::new(190, 490, 360, BUTTON_HEIGHT, None);
    size_label.set_label_color(theme::text_secondary());
    size_label.set_align(Align::Inside | Align::Left);
    size_label.set_label(&value_size_label(value));

    // The text the user is editing, parked here while the formatted view is on
    // screen so clearing the checkbox restores it byte for byte.
    let raw_text: Arc<Mutex<Option<String>>> = Arc::new(Mutex::new(None));

    {
        let mut buffer_for_format = buffer.clone();
        let mut editor_for_format = editor.clone();
        let mut size_label_for_format = size_label.clone();
        let raw_text_for_format = raw_text.clone();
        format_check.set_callback(move |check| {
            if check.is_checked() {
                let raw = buffer_for_format.text();
                let formatted = match detect_value_format(&raw) {
                    ValueFormat::Json => format_json(&raw),
                    ValueFormat::Xml => format_xml(&raw),
                    ValueFormat::Plain => None,
                };
                let Some(formatted) = formatted else {
                    // Editing turned a formattable value into one that is not.
                    // Say nothing and stay on the raw text rather than showing
                    // a stale or invented "formatted" version.
                    check.set_checked(false);
                    return;
                };
                *raw_text_for_format
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(raw);
                buffer_for_format.set_text(&formatted);
                size_label_for_format.set_label(&value_size_label(&formatted));
                if let Some(widget) = editor_for_format.as_mut() {
                    widget.deactivate();
                }
            } else {
                let restored = raw_text_for_format
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner())
                    .take();
                if let Some(raw) = restored {
                    size_label_for_format.set_label(&value_size_label(&raw));
                    buffer_for_format.set_text(&raw);
                }
                if let Some(widget) = editor_for_format.as_mut() {
                    widget.activate();
                }
            }
            app::awake();
        });
    }

    let saved: Arc<Mutex<Option<String>>> = Arc::new(Mutex::new(None));

    let (primary_label, secondary_label) = if editable {
        ("Save", Some("Cancel"))
    } else {
        ("Close", None)
    };

    let primary_x = if secondary_label.is_some() { 545 } else { 650 };
    let mut primary_btn = Button::new(primary_x, 520, BUTTON_WIDTH, BUTTON_HEIGHT, None);
    primary_btn.set_label(primary_label);
    primary_btn.set_color(if editable {
        theme::button_primary()
    } else {
        theme::button_dark()
    });
    primary_btn.set_label_color(theme::text_primary());
    primary_btn.set_frame(FrameType::RFlatBox);
    theme::install_button_hover(&mut primary_btn);

    {
        let mut dialog_for_primary = dialog.clone();
        let buffer_for_primary = buffer.clone();
        let raw_text_for_primary = raw_text.clone();
        let saved_for_primary = saved.clone();
        primary_btn.set_callback(move |_| {
            if editable {
                // Save the value being edited, never the formatted view: the
                // formatted text only ever lives in the buffer, while the real
                // one waits in `raw_text`.
                let text = raw_text_for_primary
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner())
                    .clone()
                    .unwrap_or_else(|| buffer_for_primary.text());
                *saved_for_primary
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(text);
            }
            dialog_for_primary.hide();
            app::awake();
        });
    }

    let mut secondary_btn = secondary_label.map(|label| {
        let mut button = Button::new(650, 520, BUTTON_WIDTH, BUTTON_HEIGHT, None);
        button.set_label(label);
        button.set_color(theme::button_dark());
        button.set_label_color(theme::text_primary());
        button.set_frame(FrameType::RFlatBox);
        theme::install_button_hover(&mut button);
        button
    });
    if let Some(button) = secondary_btn.as_mut() {
        let mut dialog_for_secondary = dialog.clone();
        button.set_callback(move |_| {
            dialog_for_secondary.hide();
            app::awake();
        });
    }

    dialog.end();
    dialog.show();
    Group::set_current(current_group.as_ref());
    if let Some(widget) = editor.as_mut() {
        let _ = widget.take_focus();
    } else if let Some(widget) = display.as_mut() {
        let _ = widget.take_focus();
    }

    while dialog.shown() {
        app::wait();
    }

    // Explicitly destroy top-level dialog widgets to release native resources.
    Window::delete(dialog);

    let result = saved
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .take();
    result.filter(|text| text != value)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn json_token_texts(value: &str) -> Vec<String> {
        json_tokens(value).expect("value must tokenize as JSON")
    }

    /// A multi-megabyte value is handled in one linear pass, not quadratically.
    ///
    /// Detection runs on the UI thread before the window is drawn, so this is
    /// the size at which an accidental O(n²) would show up as a freeze. The
    /// bound is deliberately loose — it is here to catch a change of complexity,
    /// not to measure a machine. (Measured at ~65 ms detect / ~115 ms format for
    /// 2.4 MB in an unoptimised build.)
    #[test]
    fn a_multi_megabyte_value_is_scanned_in_one_linear_pass() {
        let unit = r#"{"id":123456,"name":"a name","tags":["x","y","z"],"ok":true},"#;
        let big = format!("[{}]", unit.repeat(40_000).trim_end_matches(','));
        assert!(big.len() > 2_000_000);

        let start = std::time::Instant::now();
        assert_eq!(detect_value_format(&big), ValueFormat::Json);
        let formatted = format_json(&big).expect("valid JSON");
        let elapsed = start.elapsed();
        assert!(formatted.len() > big.len());
        assert!(
            elapsed < std::time::Duration::from_secs(10),
            "detect + format of {} bytes took {elapsed:?}",
            big.len()
        );

        // A value that does not even start like JSON or XML costs one character,
        // however long it is.
        let plain = "x".repeat(big.len());
        let start = std::time::Instant::now();
        assert_eq!(detect_value_format(&plain), ValueFormat::Plain);
        assert!(start.elapsed() < std::time::Duration::from_millis(100));
    }

    #[test]
    fn format_json_indents_nested_containers() {
        let formatted = format_json(r#"{"a":1,"b":[2,3]}"#).expect("valid JSON");
        assert_eq!(
            formatted,
            "{\n  \"a\": 1,\n  \"b\": [\n    2,\n    3\n  ]\n}"
        );
    }

    #[test]
    fn format_json_keeps_empty_containers_on_one_line() {
        assert_eq!(
            format_json(r#"{"a":{},"b":[]}"#).expect("valid JSON"),
            "{\n  \"a\": {},\n  \"b\": []\n}"
        );
    }

    #[test]
    fn format_json_preserves_key_order_and_number_text() {
        // A document model would reorder keys and round-trip these numbers
        // through f64. Formatting must not touch either.
        let source =
            r#"{"z":1,"a":2,"big":123456789012345678901234567890,"pad":1.500,"exp":1e400}"#;
        let formatted = format_json(source).expect("valid JSON");
        assert_eq!(json_token_texts(source), json_token_texts(&formatted));
        assert!(formatted.contains("123456789012345678901234567890"));
        assert!(formatted.contains("1.500"));
        assert!(formatted.contains("1e400"));
        assert!(formatted.find("\"z\"") < formatted.find("\"a\""));
    }

    #[test]
    fn format_json_leaves_string_contents_untouched() {
        let source = r#"{"s":"a, b: {c} [d] \" \\ \n end","t":"  spaced  "}"#;
        let formatted = format_json(source).expect("valid JSON");
        assert!(formatted.contains(r#""a, b: {c} [d] \" \\ \n end""#));
        assert!(formatted.contains(r#""  spaced  ""#));
    }

    #[test]
    fn format_json_is_idempotent() {
        let source = r#"{"a":[1,{"b":null}],"c":true}"#;
        let once = format_json(source).expect("valid JSON");
        let twice = format_json(&once).expect("formatted JSON stays valid");
        assert_eq!(once, twice);
    }

    #[test]
    fn format_json_accepts_and_rejects_documents_by_json_grammar() {
        // Documents that MUST be accepted.
        for valid in [
            r#"{}"#,
            r#"[]"#,
            r#"[[[[1]]]]"#,
            r#"{"a":{"b":{"c":[]}}}"#,
            r#"{"":""}"#,
            r#"{"a":"\u00e9"}"#,
            r#"{"a":-0}"#,
            r#"{"a":0.0e-0}"#,
            r#"{"a":"\\"}"#,
            r#"{"a":"}"}"#,
            r#"{"a":"[,:]"}"#,
            "{\n\t\"a\" : 1\r\n}",
            r#"[1,"two",true,false,null,{"k":[]}]"#,
        ] {
            assert!(format_json(valid).is_some(), "rejected valid JSON: {valid}");
        }
        // Documents that MUST be rejected.
        for invalid in [
            r#"{"a":1}x"#,
            r#"{"a":1}{"b":2}"#,
            r#"{"a"}"#,
            r#"{"a":1,"b"}"#,
            r#"[1:2]"#,
            r#"{1:2}"#,
            r#"{"a":"unterminated"#,
            "{\"a\":\"raw\nnewline\"}",
            r#"{"a":,}"#,
            r#"[,]"#,
            r#"{,}"#,
            r#"{"a":1]"#,
            r#"[1}"#,
            r#"{"a":tru}"#,
            r#"{"a":1e}"#,
            r#"{"a":.5}"#,
            r#"{"a":00}"#,
        ] {
            assert!(
                format_json(invalid).is_none(),
                "accepted invalid JSON: {invalid}"
            );
        }
    }

    #[test]
    fn format_xml_accepts_and_rejects_documents_by_xml_wellformedness() {
        for valid in [
            "<a/>",
            "<a></a>",
            "<a><b/><c/></a>",
            "<a x=\"&lt;\"/>",
            "<?xml version=\"1.0\"?><a/>",
            "<a><!-- c --><b/></a>",
            "<a><![CDATA[<b>]]></a>",
            "  <a/>  ",
            "<a><b>text</b></a>",
        ] {
            assert!(
                format_xml(valid).is_some(),
                "rejected well-formed XML: {valid}"
            );
        }
        for invalid in [
            "<a><b></a></b>",
            "<a>",
            "</a>",
            "<a/><b/>",
            "<a/>tail",
            "lead<a/>",
            "<a><!-- unterminated",
            "<a x=\"unterminated/>",
            "",
            "   ",
        ] {
            assert!(
                format_xml(invalid).is_none(),
                "accepted ill-formed XML: {invalid}"
            );
        }
    }

    #[test]
    fn format_json_rejects_invalid_documents() {
        for invalid in [
            "{",
            "{\"a\":}",
            "{\"a\":1,}",
            "[1,2",
            "{'a':1}",
            "{\"a\" 1}",
            "{} {}",
            "",
            "{\"a\":01}",
            "{\"a\":1.}",
            "{\"a\":+1}",
            "nul",
            "[1,,2]",
        ] {
            assert!(
                format_json(invalid).is_none(),
                "{invalid:?} must not format as JSON"
            );
        }
    }

    #[test]
    fn format_json_accepts_top_level_scalars_only_inside_containers() {
        // A bare scalar is legal JSON but there is nothing to indent, and the
        // detector never routes one here.
        assert_eq!(format_json("123").as_deref(), Some("123"));
        assert_eq!(format_json("\"x\"").as_deref(), Some("\"x\""));
    }

    #[test]
    fn format_xml_indents_element_only_content() {
        let formatted = format_xml("<root><a><b>1</b></a><c/></root>").expect("well-formed XML");
        assert_eq!(
            formatted,
            "<root>\n  <a>\n    <b>1</b>\n  </a>\n  <c/>\n</root>"
        );
    }

    #[test]
    fn format_xml_leaves_mixed_content_alone() {
        // The spaces around <b> are content. Re-indenting would change the
        // document, so the whole element is copied verbatim.
        let source = "<root><p>before <b>bold</b> after</p></root>";
        let formatted = format_xml(source).expect("well-formed XML");
        assert!(formatted.contains("<p>before <b>bold</b> after</p>"));
    }

    #[test]
    fn format_xml_keeps_attributes_comments_and_cdata() {
        let source = concat!(
            "<?xml version=\"1.0\"?><root><a x=\"1 > 0\" y='b'/>",
            "<!-- note --><d><![CDATA[ <raw> ]]></d></root>"
        );
        let formatted = format_xml(source).expect("well-formed XML");
        assert!(formatted.contains("<a x=\"1 > 0\" y='b'/>"));
        assert!(formatted.contains("<!-- note -->"));
        assert!(formatted.contains("<![CDATA[ <raw> ]]>"));
        assert!(formatted.starts_with("<?xml version=\"1.0\"?>"));
    }

    #[test]
    fn format_xml_is_idempotent() {
        let source = "<root><a><b>1</b></a><c/></root>";
        let once = format_xml(source).expect("well-formed XML");
        let twice = format_xml(&once).expect("formatted XML stays well-formed");
        assert_eq!(once, twice);
    }

    #[test]
    fn format_xml_rejects_ill_formed_documents() {
        for invalid in [
            "<root>",
            "<root></other>",
            "<a/><b/>",
            "text <a/>",
            "<root",
            "",
            "<root><a></root></a>",
        ] {
            assert!(
                format_xml(invalid).is_none(),
                "{invalid:?} must not format as XML"
            );
        }
    }

    #[test]
    fn detect_value_format_reports_only_what_can_be_formatted() {
        assert_eq!(detect_value_format(r#"{"a":1}"#), ValueFormat::Json);
        assert_eq!(detect_value_format("  [1,2]  "), ValueFormat::Json);
        assert_eq!(detect_value_format("<a><b/></a>"), ValueFormat::Xml);
        assert_eq!(detect_value_format("plain text"), ValueFormat::Plain);
        // Looks like JSON, is not: the button must stay disabled rather than
        // silently doing nothing.
        assert_eq!(detect_value_format("{not json"), ValueFormat::Plain);
        assert_eq!(detect_value_format("<not xml"), ValueFormat::Plain);
        assert_eq!(detect_value_format(""), ValueFormat::Plain);
    }

    #[test]
    fn value_size_label_counts_characters_and_bytes_separately() {
        assert_eq!(value_size_label(""), "0 chars · 0 bytes");
        assert_eq!(value_size_label("abc"), "3 chars · 3 bytes");
        // Hangul is three bytes per character.
        assert_eq!(value_size_label("한글"), "2 chars · 6 bytes");
        assert_eq!(
            value_size_label(&"x".repeat(1234567)),
            "1,234,567 chars · 1,234,567 bytes"
        );
    }
}

//! Keyboard-first object search: type a few letters, press Enter, land in the
//! object's source. Backed by [`crate::ui::object_search`], so it only ever
//! shows what the browser already cached for the current scope.

use fltk::{
    app,
    browser::HoldBrowser,
    button::Button,
    enums::{Align, CallbackTrigger, Event, Key},
    frame::Frame,
    group::{Flex, FlexType},
    input::Input,
    prelude::*,
    window::Window,
};
use std::sync::{Arc, Mutex};

use crate::ui::center_on_main;
use crate::ui::constants::*;
use crate::ui::object_browser::ObjectCache;
use crate::ui::object_search::{search, ObjectSearchHit, MAX_OBJECT_SEARCH_HITS};
use crate::ui::theme;

/// Show the dialog and return the object the user picked, if any.
pub fn show(cache: &ObjectCache, scope: Option<&str>) -> Option<ObjectSearchHit> {
    let current_group = fltk::group::Group::try_current();
    fltk::group::Group::set_current(None::<&fltk::group::Group>);

    let width = 480;
    let height = 420;
    let title = match scope {
        Some(scope) if !scope.trim().is_empty() => format!("Go to Object — {scope}"),
        _ => "Go to Object".to_string(),
    };
    let mut dialog = Window::default()
        .with_size(width, height)
        .with_label(&title);
    center_on_main(&mut dialog);
    dialog.set_color(theme::panel_raised());
    dialog.make_modal(true);

    let mut form = Flex::new(
        DIALOG_MARGIN,
        DIALOG_MARGIN,
        width - DIALOG_MARGIN * 2,
        height - DIALOG_MARGIN * 2,
        None,
    );
    form.set_type(FlexType::Column);
    form.set_spacing(DIALOG_SPACING);

    let mut search_input = Input::default();
    search_input.set_color(theme::input_bg());
    search_input.set_text_color(theme::text_primary());
    theme::apply_text_input_inset(&mut search_input);
    search_input.set_tooltip("Type part of an object name");
    // Refilter on every keystroke, not only on Enter.
    search_input.set_trigger(CallbackTrigger::Changed);
    form.fixed(&search_input, INPUT_ROW_HEIGHT);

    let mut browser = HoldBrowser::default();
    browser.set_color(theme::input_bg());
    browser.set_selection_color(theme::selection_strong());
    // Name column, then kind. `browser_line` writes the same two fields, and
    // FLTK gives the last column whatever width is left.
    browser.set_column_widths(&[300]);
    // `browser_line` escapes what FLTK would read as a format code; the format
    // character itself cannot be turned off through this binding.
    browser.set_column_char('\t');
    theme::style_browser_scrollbars(&browser);

    let mut status = Frame::default();
    status.set_label_color(theme::text_muted());
    status.set_align(Align::Left | Align::Inside);
    form.fixed(&status, LABEL_ROW_HEIGHT);

    let mut button_row = Flex::default();
    button_row.set_type(FlexType::Row);
    button_row.set_spacing(DIALOG_SPACING);
    Frame::default();
    let mut open_btn = Button::default().with_label("Open");
    open_btn.set_color(theme::button_dark());
    open_btn.set_label_color(theme::text_primary());
    theme::install_button_hover(&mut open_btn);
    button_row.fixed(&open_btn, BUTTON_WIDTH);
    let mut cancel_btn = Button::default().with_label("Cancel");
    cancel_btn.set_color(theme::button_dark());
    cancel_btn.set_label_color(theme::text_primary());
    theme::install_button_hover(&mut cancel_btn);
    button_row.fixed(&cancel_btn, BUTTON_WIDTH);
    button_row.end();
    form.fixed(&button_row, BUTTON_ROW_HEIGHT);

    form.end();
    dialog.end();
    fltk::group::Group::set_current(current_group.as_ref());

    let hits: Arc<Mutex<Vec<ObjectSearchHit>>> = Arc::new(Mutex::new(Vec::new()));
    let result: Arc<Mutex<Option<ObjectSearchHit>>> = Arc::new(Mutex::new(None));

    // Shared, not cloned: a scope with tens of thousands of objects would
    // otherwise be copied once per closure.
    let cache = Arc::new(cache.clone());
    let repopulate = {
        let hits = hits.clone();
        let cache = cache.clone();
        move |browser: &mut HoldBrowser, status: &mut Frame, query: &str| {
            let found = search(cache.as_ref(), query, MAX_OBJECT_SEARCH_HITS);
            browser.clear();
            for hit in &found {
                browser.add(&hit.browser_line());
            }
            if !found.is_empty() {
                browser.select(1);
            }
            status.set_label(&summary_label(found.len()));
            status.redraw();
            *hits.lock().unwrap_or_else(|poisoned| poisoned.into_inner()) = found;
        }
    };

    {
        let mut browser = browser.clone();
        let mut status = status.clone();
        repopulate(&mut browser, &mut status, "");
    }

    {
        let mut browser = browser.clone();
        let mut status = status.clone();
        let repopulate = repopulate.clone();
        search_input.set_callback(move |input| {
            repopulate(&mut browser, &mut status, &input.value());
            app::awake();
        });
    }

    let accept = {
        let hits = hits.clone();
        let result = result.clone();
        let browser = browser.clone();
        let mut dialog = dialog.clone();
        move || {
            let selected = browser.value();
            if selected <= 0 {
                return;
            }
            let Ok(index) = usize::try_from(selected - 1) else {
                return;
            };
            let picked = hits
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .get(index)
                .cloned();
            if let Some(picked) = picked {
                *result
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(picked);
                dialog.hide();
                app::awake();
            }
        }
    };

    {
        let mut accept = accept.clone();
        browser.set_callback(move |browser| {
            // A double click is the list's own "open this" gesture.
            if app::event_clicks() {
                accept();
            } else {
                browser.redraw();
            }
        });
    }

    {
        let mut accept = accept.clone();
        open_btn.set_callback(move |_| accept());
    }

    {
        let mut dialog_for_cancel = dialog.clone();
        cancel_btn.set_callback(move |_| {
            dialog_for_cancel.hide();
            app::awake();
        });
    }

    // The arrows have to be caught on the input itself. `Fl_Input` ignores Up
    // and Down, and FLTK then offers the key to each parent in turn — where
    // `Fl_Group::handle` reads it as focus navigation and moves focus into the
    // list. A handler on the window would never see them. Claiming the key here
    // is what keeps the caret in the search box while the selection moves.
    {
        let mut accept = accept.clone();
        let mut browser_for_keys = browser.clone();
        let mut dialog_for_keys = dialog.clone();
        search_input.handle(move |_, event| {
            if event != Event::KeyDown {
                return false;
            }
            match app::event_key() {
                Key::Escape => {
                    dialog_for_keys.hide();
                    app::awake();
                    true
                }
                Key::Enter | Key::KPEnter => {
                    accept();
                    true
                }
                Key::Down => {
                    move_selection(&mut browser_for_keys, 1);
                    true
                }
                Key::Up => {
                    move_selection(&mut browser_for_keys, -1);
                    true
                }
                _ => false,
            }
        });
    }

    // The same keys once focus has moved off the search box — onto the list or a
    // button. Those widgets handle the arrows themselves, so only Enter and
    // Escape are worth claiming here.
    {
        let mut accept = accept.clone();
        dialog.handle(move |window, event| match event {
            Event::KeyDown => match app::event_key() {
                Key::Escape => {
                    window.hide();
                    app::awake();
                    true
                }
                Key::Enter | Key::KPEnter => {
                    accept();
                    true
                }
                _ => false,
            },
            _ => false,
        });
    }

    {
        let mut dialog_for_close = dialog.clone();
        dialog.set_callback(move |_| {
            dialog_for_close.hide();
            app::awake();
        });
    }

    dialog.show();
    search_input.take_focus().ok();

    while dialog.shown() {
        app::wait();
    }

    // Explicitly destroy top-level dialog widgets to release native resources.
    Window::delete(dialog);

    let picked = result
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .take();
    picked
}

fn move_selection(browser: &mut HoldBrowser, delta: i32) {
    let size = browser.size();
    if size == 0 {
        return;
    }
    let next = (browser.value() + delta).clamp(1, size);
    browser.select(next);
    // Selecting does not scroll, so arrowing past the last visible row would
    // leave the highlight off screen. Only scroll when it actually left.
    if !browser.displayed(next) {
        browser.make_visible(next);
    }
    browser.redraw();
}

fn summary_label(count: usize) -> String {
    match count {
        0 => "No matching objects in this scope".to_string(),
        1 => "1 object".to_string(),
        MAX_OBJECT_SEARCH_HITS => format!("First {MAX_OBJECT_SEARCH_HITS} objects — keep typing"),
        other => format!("{other} objects"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_empty_result_says_so_rather_than_showing_a_zero() {
        assert_eq!(summary_label(0), "No matching objects in this scope");
    }

    #[test]
    fn a_single_hit_is_not_pluralised() {
        assert_eq!(summary_label(1), "1 object");
    }

    #[test]
    fn a_normal_count_is_pluralised() {
        assert_eq!(summary_label(7), "7 objects");
    }

    #[test]
    fn a_capped_result_admits_that_it_is_truncated() {
        assert!(summary_label(MAX_OBJECT_SEARCH_HITS).contains("keep typing"));
    }
}

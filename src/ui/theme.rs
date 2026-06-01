use fltk::{
    browser::HoldBrowser,
    enums::{Color, FrameType},
    prelude::*,
    text::{TextDisplay, TextEditor},
    tree::Tree,
    valuator::Scrollbar,
};

// Windows 11-inspired dark palette tuned for FLTK widgets.

pub fn app_background() -> Color {
    Color::from_rgb(32, 32, 32)
}

pub fn app_foreground() -> Color {
    Color::from_rgb(243, 243, 243)
}

pub fn window_bg() -> Color {
    Color::from_rgb(32, 32, 32)
}

pub fn panel_bg() -> Color {
    Color::from_rgb(38, 38, 38)
}

pub fn panel_alt() -> Color {
    Color::from_rgb(45, 45, 45)
}

pub fn panel_raised() -> Color {
    Color::from_rgb(52, 52, 52)
}

pub fn input_bg() -> Color {
    Color::from_rgb(46, 46, 46)
}

pub fn editor_bg() -> Color {
    Color::from_rgb(24, 24, 24)
}

pub fn border() -> Color {
    Color::from_rgb(64, 64, 64)
}

pub fn text_primary() -> Color {
    Color::from_rgb(243, 243, 243)
}

pub fn text_secondary() -> Color {
    Color::from_rgb(210, 210, 210)
}

pub fn text_muted() -> Color {
    Color::from_rgb(168, 168, 168)
}

pub fn accent() -> Color {
    Color::from_rgb(0, 120, 212)
}

pub fn selection_soft() -> Color {
    Color::from_rgb(45, 90, 140)
}

pub fn selection_strong() -> Color {
    accent()
}

pub fn button_primary() -> Color {
    accent()
}

pub fn button_secondary() -> Color {
    Color::from_rgb(58, 58, 58)
}

pub fn button_subtle() -> Color {
    Color::from_rgb(50, 50, 50)
}

pub fn button_success() -> Color {
    Color::from_rgb(16, 124, 16)
}

pub fn button_warning() -> Color {
    Color::from_rgb(202, 80, 16)
}

pub fn button_cancel() -> Color {
    button_warning()
}

pub fn button_danger() -> Color {
    Color::from_rgb(232, 17, 35)
}

pub fn table_header_bg() -> Color {
    panel_alt()
}

pub fn table_cell_bg() -> Color {
    panel_bg()
}

pub fn table_border() -> Color {
    border()
}

pub fn tree_connector() -> Color {
    Color::from_rgb(82, 82, 82)
}

pub fn scrollbar_thumb() -> Color {
    Color::from_rgb(88, 88, 88)
}

pub fn scrollbar_track() -> Color {
    panel_raised()
}

pub fn style_scrollbar(scrollbar: &mut Scrollbar) {
    scrollbar.set_color(scrollbar_track());
    scrollbar.set_selection_color(scrollbar_thumb());
    scrollbar.set_slider_frame(FrameType::RFlatBox);
    scrollbar.redraw();
}

pub fn style_table_scrollbars<T: TableExt>(table: &T) {
    let mut scrollbar = table.scrollbar();
    style_scrollbar(&mut scrollbar);
    let mut hscrollbar = table.hscrollbar();
    style_scrollbar(&mut hscrollbar);
}

pub fn style_browser_scrollbars(browser: &HoldBrowser) {
    let mut scrollbar = browser.scrollbar();
    style_scrollbar(&mut scrollbar);
    let mut hscrollbar = browser.hscrollbar();
    style_scrollbar(&mut hscrollbar);
}

pub fn style_tree_scrollbars(tree: &mut Tree) {
    let Some(group) = tree.as_group() else {
        return;
    };

    for idx in 0..group.children() {
        let Some(child) = group.child(idx) else {
            continue;
        };
        if tree.is_scrollbar(&child) {
            unsafe {
                let mut scrollbar = Scrollbar::from_widget_ptr(child.as_widget_ptr());
                style_scrollbar(&mut scrollbar);
            }
        }
    }
}

fn style_group_children_as_scrollbars<W: WidgetExt>(widget: &W) {
    let Some(group) = widget.as_group() else {
        return;
    };

    for idx in 0..group.children() {
        let Some(child) = group.child(idx) else {
            continue;
        };
        unsafe {
            let mut scrollbar = Scrollbar::from_widget_ptr(child.as_widget_ptr());
            style_scrollbar(&mut scrollbar);
        }
    }
}

pub fn style_text_display_scrollbars(display: &TextDisplay) {
    style_group_children_as_scrollbars(display);
}

pub fn style_text_editor_scrollbars(editor: &TextEditor) {
    style_group_children_as_scrollbars(editor);
}

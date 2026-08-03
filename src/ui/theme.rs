use fltk::{
    app,
    browser::HoldBrowser,
    draw::{draw_box, draw_polygon, draw_text2, set_draw_color, set_font},
    enums::{Align, Color, Event, FrameType},
    menu::Choice,
    prelude::*,
    text::{TextDisplay, TextEditor},
    tree::Tree,
    valuator::Scrollbar,
};

// Windows 11-inspired dark palette tuned for FLTK widgets.

pub const CHOICE_TEXT_LEFT_PADDING: i32 = 10;
const INPUT_NATIVE_TEXT_LEFT_OFFSET: i32 = 1;
const INPUT_TEXT_RIGHT_PADDING: i32 = 6;
const INPUT_FRAME_LEFT_INSET: i32 = CHOICE_TEXT_LEFT_PADDING - INPUT_NATIVE_TEXT_LEFT_OFFSET;
const INPUT_FRAME_WIDTH_INSET: i32 = INPUT_FRAME_LEFT_INSET + INPUT_TEXT_RIGHT_PADDING;

fn draw_text_input_box(x: i32, y: i32, w: i32, h: i32, color: Color) {
    draw_box(FrameType::RFlatBox, x, y, w, h, color);
}

pub fn register_text_input_frame() {
    app::set_frame_type_cb(
        FrameType::FreeBoxType,
        draw_text_input_box,
        INPUT_FRAME_LEFT_INSET,
        0,
        INPUT_FRAME_WIDTH_INSET,
        0,
    );
}

pub fn apply_text_input_inset<W: WidgetExt>(input: &mut W) {
    input.set_frame(FrameType::FreeBoxType);
}

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

pub fn text_error() -> Color {
    Color::from_rgb(232, 17, 35)
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
    panel_raised()
}

pub fn button_secondary() -> Color {
    panel_raised()
}

pub fn button_subtle() -> Color {
    panel_raised()
}

pub fn button_success() -> Color {
    panel_raised()
}

pub fn button_warning() -> Color {
    panel_raised()
}

pub fn button_cancel() -> Color {
    button_warning()
}

pub fn button_cancel_active() -> Color {
    Color::from_rgb(202, 80, 16)
}

pub fn button_danger() -> Color {
    panel_raised()
}

pub fn button_dark() -> Color {
    input_bg()
}

fn style_choice_with_background(choice: &mut Choice, background: Color) {
    const ARROW_WIDTH: i32 = 20;

    choice.set_color(background);
    choice.set_text_color(text_primary());
    choice.set_selection_color(selection_soft());
    choice.set_frame(FrameType::RFlatBox);
    choice.draw(|choice| {
        draw_box(
            choice.frame(),
            choice.x(),
            choice.y(),
            choice.w(),
            choice.h(),
            choice.color(),
        );
        let arrow_width = ARROW_WIDTH.min(choice.w().max(0));
        set_font(choice.text_font(), choice.text_size());
        set_draw_color(if choice.active_r() {
            choice.text_color()
        } else {
            text_muted()
        });
        if let Some(value) = choice.choice() {
            let content_width = choice.w().saturating_sub(arrow_width).max(0);
            let text_left_padding = CHOICE_TEXT_LEFT_PADDING.min(content_width);
            draw_text2(
                &value,
                choice.x().saturating_add(text_left_padding),
                choice.y(),
                content_width.saturating_sub(text_left_padding),
                choice.h(),
                Align::Left | Align::Inside,
            );
        }

        let arrow_x = choice.x() + choice.w() - arrow_width / 2;
        let arrow_y = choice.y() + choice.h() / 2;
        draw_polygon(
            arrow_x - 3,
            arrow_y - 2,
            arrow_x + 3,
            arrow_y - 2,
            arrow_x,
            arrow_y + 2,
        );
    });
}

pub fn style_choice(choice: &mut Choice) {
    style_choice_with_background(choice, input_bg());
}

pub fn hover_feedback_color(base: Color) -> Color {
    const HOVER_CHANNEL_DELTA: u8 = 12;

    let (red, green, blue) = base.to_rgb();
    Color::from_rgb(
        red.saturating_add(HOVER_CHANNEL_DELTA),
        green.saturating_add(HOVER_CHANNEL_DELTA),
        blue.saturating_add(HOVER_CHANNEL_DELTA),
    )
}

#[derive(Default)]
pub struct HoverFeedbackState {
    base: Option<Color>,
}

impl HoverFeedbackState {
    pub fn update<W: WidgetExt>(&mut self, widget: &mut W, event: Event) {
        match event {
            Event::Enter | Event::Move if widget.active_r() => {
                let current = widget.color();
                let base = self
                    .base
                    .filter(|base| current == hover_feedback_color(*base))
                    .unwrap_or(current);
                let hover = hover_feedback_color(base);
                self.base = Some(base);
                if current != hover {
                    widget.set_color(hover);
                    widget.redraw();
                }
            }
            Event::Enter | Event::Move | Event::Leave | Event::Deactivate | Event::Hide => {
                if let Some(base) = self.base.take() {
                    if widget.color() == hover_feedback_color(base) {
                        widget.set_color(base);
                        widget.redraw();
                    }
                }
            }
            _ => {}
        }
    }
}

fn install_hover_feedback<W: WidgetBase>(widget: &mut W) {
    let mut state = HoverFeedbackState::default();
    widget.handle(move |widget, event| {
        state.update(widget, event);
        false
    });
}

pub fn install_button_hover<W: WidgetBase>(widget: &mut W) {
    install_hover_feedback(widget);
}

pub fn install_choice_hover<W: WidgetBase>(widget: &mut W) {
    install_hover_feedback(widget);
}

pub fn status_bar_default() -> Color {
    panel_raised()
}

pub fn status_connected() -> Color {
    Color::from_rgb(74, 222, 128)
}

pub fn status_disconnected() -> Color {
    Color::from_rgb(255, 107, 107)
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
            // SAFETY: `child` is owned by the live FLTK group and
            // `tree.is_scrollbar` verifies its runtime widget type before the
            // pointer is wrapped as a `Scrollbar`.
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
        // SAFETY: FLTK text display/editor groups contain scrollbar children;
        // `child` is owned by the live group for the duration of this wrapper,
        // and this helper is called only for those FLTK widget types.
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn button_and_status_bar_default_colors_match_panel_raised() {
        let expected = (52, 52, 52);

        for color in [
            button_primary(),
            button_secondary(),
            button_subtle(),
            button_success(),
            button_warning(),
            button_cancel(),
            button_danger(),
            status_bar_default(),
        ] {
            assert_eq!(color.to_rgb(), expected);
        }
    }

    #[test]
    fn dark_button_color_matches_input_background() {
        assert_eq!(button_dark().to_rgb(), (46, 46, 46));
    }

    #[test]
    fn text_input_frame_aligns_with_choice_text_padding() {
        assert_eq!(
            INPUT_FRAME_LEFT_INSET + INPUT_NATIVE_TEXT_LEFT_OFFSET,
            CHOICE_TEXT_LEFT_PADDING
        );
        assert_eq!(
            INPUT_FRAME_WIDTH_INSET - INPUT_FRAME_LEFT_INSET,
            INPUT_TEXT_RIGHT_PADDING
        );
    }

    #[test]
    fn hover_feedback_color_brightens_each_channel() {
        assert_eq!(hover_feedback_color(button_subtle()).to_rgb(), (64, 64, 64));
        assert_eq!(
            hover_feedback_color(Color::from_rgb(250, 1, 100)).to_rgb(),
            (255, 13, 112)
        );
    }

    #[test]
    fn active_cancel_color_preserves_the_existing_orange() {
        assert_eq!(button_cancel_active().to_rgb(), (202, 80, 16));
    }

    #[test]
    fn connection_status_colors_match_dark_theme_palette() {
        assert_eq!(status_connected().to_rgb(), (74, 222, 128));
        assert_eq!(status_disconnected().to_rgb(), (255, 107, 107));
    }
}

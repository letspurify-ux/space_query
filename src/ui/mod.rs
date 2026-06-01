pub mod connection_dialog;
pub mod constants;
pub mod find_replace;
pub mod font_settings;
pub mod intellisense;
pub mod intellisense_context;
pub mod log_viewer;
pub mod main_window;
pub mod menu;
pub mod object_browser;
pub(crate) mod object_drag_payload;
pub mod query_history;
pub mod query_tabs;
pub mod result_table;
pub mod result_tabs;
pub mod settings_dialog;
pub(crate) mod sql_depth;
pub mod sql_editor;
pub mod syntax_highlight;
pub(crate) mod text_buffer_access;
pub mod theme;

use fltk::{app, prelude::WidgetExt, window::Window};

use crate::utils::arithmetic::safe_div;

pub use connection_dialog::*;
pub use find_replace::*;
pub use font_settings::*;
pub use intellisense::*;
pub use main_window::*;
pub use menu::*;
pub use object_browser::*;
pub use query_history::*;
pub use query_tabs::*;
pub use result_table::*;
pub use result_tabs::*;
pub use settings_dialog::*;
pub use sql_editor::*;
pub use syntax_highlight::*;

#[derive(Clone)]
pub struct ResultTabRequest {
    pub label: String,
    pub result: crate::db::QueryResult,
}

pub fn center_on_main(window: &mut Window) {
    // NOTE: fltk-rs의 center_of()는 참조 위젯이 Window 타입이면
    // wx/wy를 0으로 고정해 실제 화면 위치를 무시하는 버그가 있음.
    // 메인 윈도우 좌표를 직접 읽어 set_pos()로 설정한다.
    let target = if let Some(main) = app::widget_from_id::<Window>("main_window") {
        if main.as_widget_ptr() != window.as_widget_ptr() {
            Some((main.x(), main.y(), main.width(), main.height()))
        } else {
            None
        }
    } else {
        app::first_window().map(|main| (main.x(), main.y(), main.width(), main.height()))
    };

    let (x, y) = if let Some((mx, my, mw, mh)) = target {
        (
            mx + safe_div(mw - window.width(), 2),
            my + safe_div(mh - window.height(), 2),
        )
    } else {
        let (sw, sh) = app::screen_size();
        (
            safe_div((sw as i32) - window.width(), 2),
            safe_div((sh as i32) - window.height(), 2),
        )
    };
    window.set_pos(x, y);
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct PopupAnchorSnapshot {
    x: i32,
    y: i32,
    w: i32,
    h: i32,
    shown: bool,
    visible: bool,
}

impl PopupAnchorSnapshot {
    pub(crate) fn capture<W: WidgetExt>(anchor: &W) -> Option<Self> {
        if anchor.was_deleted() {
            return None;
        }

        let window = anchor.top_window()?;
        Some(Self {
            x: window.x_root(),
            y: window.y_root(),
            w: window.w(),
            h: window.h(),
            shown: window.shown(),
            visible: window.visible(),
        })
    }

    pub(crate) fn still_matches<W: WidgetExt>(self, anchor: &W) -> bool {
        Self::capture(anchor)
            .filter(|current| current.shown && current.visible)
            .is_some_and(|current| current == self)
    }
}

use fltk::{
    app,
    browser::HoldBrowser,
    draw,
    enums::{Event, FrameType},
    misc::Tooltip,
    prelude::*,
    window::Window,
};
use space_query::{
    db::{ColumnInfo, QueryResult},
    ui::{
        apply_global_default_font,
        constants::{BUTTON_HEIGHT, TAB_HEADER_HEIGHT},
        intellisense::input_caret_popup_anchor,
        log_viewer::LogViewerDialog,
        profile_by_name, show_settings_dialog, theme, ConnectionDialog, IntellisensePopup,
        MainWindow, QueryHistoryDialog, SignatureLabel, SignatureOverload, SignaturePopup,
    },
    utils::{arithmetic::safe_div, logging, AppConfig},
};
use std::{
    fs::File,
    io::Write,
    sync::{Arc, Mutex, OnceLock},
    thread,
    time::Duration,
};

fn pump(milliseconds: u64) {
    for _ in 0..safe_div(milliseconds, 20).max(1) {
        app::check();
        thread::sleep(Duration::from_millis(20));
    }
}

fn fail(message: impl std::fmt::Display) -> ! {
    eprintln!("{message}");
    std::process::exit(1);
}

fn capture_rgb<W: WindowExt>(window: &mut W) -> (Vec<u8>, i32, i32) {
    app::flush();
    window.make_current();
    let image =
        draw::capture_window(window).unwrap_or_else(|err| fail(format!("capture window: {err}")));
    (image.to_rgb_data(), image.data_w(), image.data_h())
}

fn fill_missing_pixels(canvas: &mut [u8], fallback: &[u8]) {
    for (target, source) in canvas.chunks_exact_mut(3).zip(fallback.chunks_exact(3)) {
        if target == [0, 0, 0] && source != [0, 0, 0] {
            target.copy_from_slice(source);
        }
    }
}

fn capture_complete_rgb<W: WindowExt>(window: &mut W) -> (Vec<u8>, i32, i32) {
    let (mut canvas, width, height) = capture_rgb(window);
    for _ in 0..2 {
        window.set_damage(true);
        window.redraw();
        app::redraw();
        pump(120);
        let (frame, frame_width, frame_height) = capture_rgb(window);
        if frame_width != width || frame_height != height {
            fail("capture dimensions changed while redrawing");
        }
        fill_missing_pixels(&mut canvas, &frame);
    }
    (canvas, width, height)
}

type MainCapture = (Vec<u8>, i32, i32);
// macOS FLTK captures can contain only the regions redrawn in the current frame.
// Keep the last complete main-window frame to fill unchanged pixels.
static LAST_MAIN_CAPTURE: OnceLock<Mutex<Option<MainCapture>>> = OnceLock::new();

fn save_ppm(path: &str, data: &[u8], width: i32, height: i32) {
    let mut file = File::create(path).unwrap_or_else(|err| fail(format!("create capture: {err}")));
    write!(file, "P6\n{width} {height}\n255\n")
        .unwrap_or_else(|err| fail(format!("write PPM header: {err}")));
    file.write_all(data)
        .unwrap_or_else(|err| fail(format!("write PPM pixels: {err}")));
}

fn save_main(path: &str) {
    let mut window =
        app::widget_from_id::<Window>("main_window").unwrap_or_else(|| fail("main window"));
    window.set_damage(true);
    window.redraw();
    app::redraw();
    pump(250);
    let (mut data, width, height) = capture_complete_rgb(&mut window);
    let mut previous = LAST_MAIN_CAPTURE
        .get_or_init(|| Mutex::new(None))
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    if let Some((previous_data, previous_width, previous_height)) = previous.as_ref() {
        if *previous_width == width && *previous_height == height {
            fill_missing_pixels(&mut data, previous_data);
        }
    }
    *previous = Some((data.clone(), width, height));
    save_ppm(path, &data, width, height);
}

fn save_main_part(path: &str, x: i32, y: i32, width: i32, height: i32) {
    let mut window =
        app::widget_from_id::<Window>("main_window").unwrap_or_else(|| fail("main window"));
    window.set_damage(true);
    window.redraw();
    app::redraw();
    pump(400);
    let (data, full_width, full_height) = capture_complete_rgb(&mut window);
    if !(x >= 0 && y >= 0 && x + width <= full_width && y + height <= full_height) {
        fail("capture rectangle is outside the main window");
    }
    let mut cropped = vec![0_u8; (width * height * 3) as usize];
    for row in 0..height {
        let source = (((y + row) * full_width + x) * 3) as usize;
        let target = (row * width * 3) as usize;
        let length = (width * 3) as usize;
        cropped[target..target + length].copy_from_slice(&data[source..source + length]);
    }
    save_ppm(path, &cropped, width, height);
}

fn window_by_label(label: &str) -> Option<Window> {
    let mut current = app::first_window().map(|window| unsafe { Window::from_widget(window) });
    while let Some(window) = current {
        current = app::next_window(&window).map(|next| unsafe { Window::from_widget(next) });
        if window.label() == label {
            return Some(window);
        }
    }
    None
}

fn window_by_root_bounds(
    position: (i32, i32),
    dimensions: (i32, i32),
    excluded: &Window,
) -> Option<Window> {
    let excluded_ptr = excluded.as_widget_ptr();
    let mut current = app::first_window().map(|window| unsafe { Window::from_widget(window) });
    while let Some(window) = current {
        current = app::next_window(&window).map(|next| unsafe { Window::from_widget(next) });
        if window.as_widget_ptr() != excluded_ptr
            && (window.x_root(), window.y_root()) == position
            && (window.w(), window.h()) == dimensions
        {
            return Some(window);
        }
    }
    None
}

fn verify_control_heights(group: fltk::group::Group, context: &str) {
    fn verify<W: WidgetExt>(widget: &W, kind: &str, context: &str) {
        if widget.w() > 0 && widget.h() > 0 && widget.h() != BUTTON_HEIGHT {
            fail(format!(
                "{context} {kind} {:?} has height {}, expected {BUTTON_HEIGHT}",
                widget.label(),
                widget.h()
            ));
        }
    }

    for child in group.into_iter() {
        if let Some(choice) = fltk::misc::InputChoice::from_dyn_widget(&child) {
            verify(&choice, "input choice", context);
            verify(&choice.input(), "input choice text field", context);
            verify(&choice.menu_button(), "input choice menu button", context);
        } else if let Some(check) = fltk::button::CheckButton::from_dyn_widget(&child) {
            verify(&check, "checkbox", context);
        } else if let Some(button) = fltk::button::Button::from_dyn_widget(&child) {
            verify(&button, "button", context);
        } else if let Some(choice) = fltk::menu::Choice::from_dyn_widget(&child) {
            verify(&choice, "choice", context);
        } else if let Some(input) = fltk::input::SecretInput::from_dyn_widget(&child) {
            verify(&input, "secret input", context);
        } else if let Some(input) = fltk::input::IntInput::from_dyn_widget(&child) {
            verify(&input, "integer input", context);
        } else if let Some(input) = fltk::input::Input::from_dyn_widget(&child) {
            verify(&input, "input", context);
        } else if let Some(button) = fltk::menu::MenuButton::from_dyn_widget(&child) {
            verify(&button, "menu button", context);
        }

        if let Some(group) = child.as_group() {
            verify_control_heights(group, context);
        }
    }
}

fn verify_tab_header_heights(group: fltk::group::Group, context: &str) {
    for child in group.into_iter() {
        if let Some(tabs) = fltk::group::Tabs::from_dyn_widget(&child) {
            for tab_child in tabs.clone().into_iter() {
                let actual_height = tab_child.y().saturating_sub(tabs.y());
                if actual_height != TAB_HEADER_HEIGHT {
                    fail(format!(
                        "{context} tab {:?} has header height {actual_height}, expected {TAB_HEADER_HEIGHT}",
                        tab_child.label()
                    ));
                }
            }
        }

        if let Some(group) = child.as_group() {
            verify_tab_header_heights(group, context);
        }
    }
}

fn capture_active_dialog(expected_label: &str, path: &str) {
    let mut window = window_by_label(expected_label)
        .unwrap_or_else(|| fail(format!("missing dialog: {expected_label}")));
    if let Some(group) = window.as_group() {
        verify_control_heights(group.clone(), expected_label);
        verify_tab_header_heights(group, expected_label);
    }
    let (data, width, height) = capture_complete_rgb(&mut window);
    save_ppm(path, &data, width, height);
    window.hide();
}

fn select_first_browser_in_active_dialog(expected_label: &str) {
    fn visit(group: fltk::group::Group) -> Option<HoldBrowser> {
        for child in group.into_iter() {
            if let Some(browser) = HoldBrowser::from_dyn_widget(&child) {
                return Some(browser);
            }
            if let Some(group) = child.as_group() {
                if let Some(browser) = visit(group) {
                    return Some(browser);
                }
            }
        }
        None
    }

    let Some(window) = window_by_label(expected_label) else {
        return;
    };
    if let Some(group) = window.as_group() {
        if let Some(mut browser) = visit(group) {
            browser.select(1);
            browser.do_callback();
        }
    }
}

fn make_result(columns: &[(&str, &str)], rows: &[&[&str]], sql: &str) -> QueryResult {
    QueryResult {
        sql: sql.to_string(),
        columns: columns
            .iter()
            .map(|(name, data_type)| ColumnInfo {
                name: (*name).to_string(),
                data_type: (*data_type).to_string(),
                kind: space_query::db::SqlValueKind::Unknown,
            })
            .collect(),
        rows: rows
            .iter()
            .map(|row| row.iter().map(|value| (*value).to_string()).collect())
            .collect(),
        row_count: rows.len(),
        execution_time: Duration::from_millis(18),
        message: format!("{} rows selected", rows.len()),
        is_select: true,
        success: true,
    }
}

fn composite_popup<W: WindowExt>(
    canvas: &mut [u8],
    canvas_width: i32,
    canvas_height: i32,
    main_x: i32,
    main_y: i32,
    popup_window: &mut W,
) {
    let offset_x = popup_window.x_root() - main_x;
    let offset_y = popup_window.y_root() - main_y;
    let (popup_data, popup_width, popup_height) = capture_complete_rgb(popup_window);
    for y in 0..popup_height {
        let target_y = offset_y + y;
        if !(0..canvas_height).contains(&target_y) {
            continue;
        }
        for x in 0..popup_width {
            let target_x = offset_x + x;
            if !(0..canvas_width).contains(&target_x) {
                continue;
            }
            let source = ((y * popup_width + x) * 3) as usize;
            let target = ((target_y * canvas_width + target_x) * 3) as usize;
            canvas[target..target + 3].copy_from_slice(&popup_data[source..source + 3]);
        }
    }
}

fn capture_intellisense(main_window: &mut MainWindow) {
    let sql = "SELECT e.empno,\n       e.ename,\n       e.\nFROM emp e\nWHERE e.sal > 2000;";
    let cursor = sql
        .find("e.\nFROM")
        .unwrap_or_else(|| fail("completion cursor")) as i32
        + 2;
    let editor = main_window.capture_tour_set_sql(sql, Some(cursor));
    pump(300);

    let mut main =
        app::widget_from_id::<Window>("main_window").unwrap_or_else(|| fail("main window"));
    let main_x = main.x_root();
    let main_y = main.y_root();
    main.set_damage(true);
    main.redraw();
    app::redraw();
    pump(200);
    let (mut canvas, width, height) = capture_complete_rgb(&mut main);

    let mut popup = IntellisensePopup::new();
    let popup_width = 320;
    let popup_height = 8 * (16 + 6) + 10;
    let (cursor_x, cursor_y) = editor.position_to_xy(cursor);
    let editor_window = editor.window().unwrap_or_else(|| fail("editor window"));
    let win_x = editor_window.x_root();
    let win_y = editor_window.y_root();
    let max_x = (win_x + editor_window.w() - popup_width).max(win_x);
    let max_y = (win_y + editor_window.h() - popup_height).max(win_y);
    let popup_x = (win_x + cursor_x).clamp(win_x, max_x);
    let popup_y = (win_y + cursor_y + 20).clamp(win_y, max_y);
    popup.show_suggestions(
        vec![
            "EMPNO".into(),
            "ENAME".into(),
            "JOB".into(),
            "MGR".into(),
            "HIREDATE".into(),
            "SAL".into(),
            "COMM".into(),
            "DEPTNO".into(),
        ],
        popup_x,
        popup_y,
    );
    pump(300);
    let mut popup_window = app::first_window().unwrap_or_else(|| fail("intellisense popup"));
    composite_popup(
        &mut canvas,
        width,
        height,
        main_x,
        main_y,
        &mut popup_window,
    );
    save_ppm("/tmp/space-query-intellisense.ppm", &canvas, width, height);
    popup.hide();
}

fn capture_signature_popup(main_window: &mut MainWindow) {
    let sql = "DECLARE\n    discounted_price NUMBER;\nBEGIN\n    discounted_price := ROUND(1234.567, 2);\nEND;\n/";
    let cursor = sql.find(", 2").unwrap_or_else(|| fail("signature cursor")) as i32 + 3;
    let open_paren = sql
        .find("ROUND(")
        .unwrap_or_else(|| fail("signature anchor")) as i32
        + 5;
    let editor = main_window.capture_tour_set_sql(sql, Some(cursor));
    pump(300);

    let mut main =
        app::widget_from_id::<Window>("main_window").unwrap_or_else(|| fail("main window"));
    let main_x = main.x_root();
    let main_y = main.y_root();
    main.set_damage(true);
    main.redraw();
    app::redraw();
    pump(200);
    let (mut canvas, width, height) = capture_complete_rgb(&mut main);

    let signature = "ROUND(number [, integer])";
    let number_start = signature
        .find("number")
        .unwrap_or_else(|| fail("signature first argument"));
    let integer_start = signature
        .find("integer")
        .unwrap_or_else(|| fail("signature second argument"));
    let arg_spans = vec![
        (number_start, number_start + "number".len()),
        (integer_start, integer_start + "integer".len()),
    ];
    let label = SignatureLabel {
        text: signature.to_string(),
        arg_spans: arg_spans.clone(),
        overloads: vec![SignatureOverload {
            arg_spans,
            required_args: 1,
            variadic_arg: None,
        }],
    };
    let mut popup = SignaturePopup::new();
    let _ = popup.show(&editor, &label, 1, open_paren);
    pump(300);
    let mut popup_window = app::first_window().unwrap_or_else(|| fail("signature popup"));
    composite_popup(
        &mut canvas,
        width,
        height,
        main_x,
        main_y,
        &mut popup_window,
    );
    save_ppm("/tmp/space-query-signature.ppm", &canvas, width, height);
    popup.hide();
}

fn capture_formatting(main_window: &mut MainWindow) {
    let sql = "select e.empno,e.ename,d.dname,e.sal from emp e join dept d on e.deptno=d.deptno where e.sal>2000 and d.loc='SEOUL' order by e.sal desc;";
    let _ = main_window.capture_tour_set_sql(sql, Some(0));
    pump(250);
    save_main("/tmp/space-query-formatting-before.ppm");
    main_window.capture_tour_format_sql();
    pump(350);
    save_main("/tmp/space-query-formatting-after.ppm");
}

fn capture_object_browser(main_window: &mut MainWindow) {
    fn visible_filter_input(group: fltk::group::Group) -> Option<fltk::input::Input> {
        for child in group.into_iter() {
            if let Some(input) = fltk::input::Input::from_dyn_widget(&child) {
                if input.visible_r()
                    && input.tooltip().as_deref() == Some("Type to filter objects...")
                {
                    return Some(input);
                }
            }
            if let Some(group) = child.as_group() {
                if let Some(input) = visible_filter_input(group) {
                    return Some(input);
                }
            }
        }
        None
    }

    fn nearest_aligned_choice_above(
        group: fltk::group::Group,
        filter: &fltk::input::Input,
        candidate: &mut Option<fltk::menu::Choice>,
    ) {
        for child in group.into_iter() {
            if let Some(choice) = fltk::menu::Choice::from_dyn_widget(&child) {
                let aligned = choice.visible_r()
                    && choice.x() + theme::CHOICE_TEXT_LEFT_PADDING
                        == filter.x() + filter.frame().dx() + 1
                    && choice.y() < filter.y();
                if aligned
                    && candidate
                        .as_ref()
                        .is_none_or(|current| choice.y() > current.y())
                {
                    *candidate = Some(choice);
                }
            }
            if let Some(group) = child.as_group() {
                nearest_aligned_choice_above(group, filter, candidate);
            }
        }
    }

    let _ = main_window.capture_tour_set_sql(
        "SELECT e.empno, e.ename, e.job, e.sal\nFROM emp e\nORDER BY e.empno;",
        Some(0),
    );
    main_window.capture_tour_show_object_browser();
    pump(500);
    save_main("/tmp/space-query-object-browser-full.ppm");
    let capture_scale = std::env::var("SPACE_QUERY_CAPTURE_UI_SCALE")
        .ok()
        .and_then(|value| value.parse::<u32>().ok())
        .unwrap_or(100);
    if capture_scale <= 100 {
        save_main_part("/tmp/space-query-object-browser.ppm", 0, 70, 250, 705);
    }

    let main_window = app::widget_from_id::<Window>("main_window")
        .unwrap_or_else(|| fail("main window is missing"));
    let main_group = main_window
        .as_group()
        .unwrap_or_else(|| fail("main window group is missing"));
    verify_control_heights(main_group.clone(), "main window");
    verify_tab_header_heights(main_group.clone(), "main window");
    let mut filter = visible_filter_input(main_group.clone())
        .unwrap_or_else(|| fail("visible object browser filter input is missing"));
    let mut scope = None;
    nearest_aligned_choice_above(main_group, &filter, &mut scope);
    let scope = scope.unwrap_or_else(|| fail("visible object browser scope choice is missing"));
    if filter.frame() != FrameType::FreeBoxType
        || filter.x() + filter.frame().dx() + 1 != scope.x() + theme::CHOICE_TEXT_LEFT_PADDING
    {
        fail("object browser filter text is not aligned with the scope choice");
    }
    filter.set_value("Filter");
    filter.redraw();
    pump(80);
    save_main_part(
        "/tmp/space-query-object-browser-filter-alignment.ppm",
        0,
        70,
        250,
        110,
    );
    filter.set_value("");
    filter.redraw();
    pump(40);
}

fn result_page_control_sizes() -> Vec<(i32, i32)> {
    let mut sizes = [
        "result_page_first",
        "result_page_previous",
        "result_page_next",
        "result_page_last",
    ]
    .iter()
    .map(|widget_id| {
        let button = app::widget_from_id::<fltk::button::Button>(widget_id)
            .unwrap_or_else(|| fail(format!("{widget_id} control is missing")));
        (button.w(), button.h())
    })
    .collect::<Vec<_>>();
    let unit = app::widget_from_id::<fltk::menu::Choice>("result_page_unit")
        .unwrap_or_else(|| fail("result page unit control is missing"));
    sizes.push((unit.w(), unit.h()));
    sizes
}

fn assert_result_page_control_layout(expected_sizes: &[(i32, i32)]) {
    let actual_sizes = result_page_control_sizes();
    if actual_sizes != expected_sizes {
        fail(format!(
            "result page controls changed size: expected {expected_sizes:?}, got {actual_sizes:?}"
        ));
    }

    let clear = app::widget_from_id::<fltk::button::Button>("result_clear_all")
        .unwrap_or_else(|| fail("result clear button is missing"));
    let page_control = app::widget_from_id::<fltk::group::Flex>("result_page_controls")
        .unwrap_or_else(|| fail("result page control flex is missing"));
    let first = app::widget_from_id::<fltk::button::Button>("result_page_first")
        .unwrap_or_else(|| fail("result first-page button is missing"));
    let last = app::widget_from_id::<fltk::button::Button>("result_page_last")
        .unwrap_or_else(|| fail("result last-page button is missing"));
    let unit = app::widget_from_id::<fltk::menu::Choice>("result_page_unit")
        .unwrap_or_else(|| fail("result page unit control is missing"));
    let one_tab = app::widget_from_id::<fltk::button::CheckButton>("result_one_tab_per_query")
        .unwrap_or_else(|| fail("result one-tab-per-query control is missing"));
    let available_center_twice = clear.x() + clear.w() + one_tab.x();
    let flex_center_twice = page_control.x() * 2 + page_control.w();
    if (available_center_twice - flex_center_twice).abs() > 1 {
        fail("result page flex is not centered between the adjacent toolbar controls");
    }

    let required_width = expected_sizes.iter().map(|(width, _)| width).sum::<i32>()
        + page_control.spacing()
            * i32::try_from(expected_sizes.len().saturating_sub(1)).unwrap_or(0);
    let should_show = page_control.w() >= required_width;
    let buttons_visibility_matches = [
        "result_page_first",
        "result_page_previous",
        "result_page_next",
        "result_page_last",
    ]
    .iter()
    .all(|widget_id| {
        app::widget_from_id::<fltk::button::Button>(widget_id)
            .is_some_and(|button| button.visible() == should_show)
    });
    if !buttons_visibility_matches || unit.visible() != should_show {
        fail(format!(
            "result page control visibility does not match available width {}",
            page_control.w()
        ));
    }
    if !should_show {
        return;
    }

    let controls_center_twice = first.x() + last.x() + last.w();
    if (flex_center_twice - controls_center_twice).abs() > 1 {
        fail(format!(
            "result page controls are not centered: flex center x2={flex_center_twice}, controls center x2={controls_center_twice}"
        ));
    }
}

fn assert_result_page_control_feedback() {
    fn assert_control<W: WidgetBase>(control: &mut W) {
        let base = control.color();
        let _ = control.handle_event(Event::Enter);
        pump(20);
        if control.color() != theme::hover_feedback_color(base) {
            fail("result page control did not apply its hover color");
        }
        let _ = control.handle_event(Event::Leave);
        pump(20);
        if control.color() != base {
            fail("result page control did not restore its default color");
        }
    }

    let mut next = app::widget_from_id::<fltk::button::Button>("result_page_next")
        .unwrap_or_else(|| fail("result next-page button is missing"));
    let mut unit = app::widget_from_id::<fltk::menu::Choice>("result_page_unit")
        .unwrap_or_else(|| fail("result page unit control is missing"));
    assert_control(&mut next);
    assert_control(&mut unit);
}

fn assert_hover_feedback<W: WidgetBase>(control: &mut W, name: &str) {
    let base = control.color();
    let _ = control.handle_event(Event::Enter);
    pump(20);
    if control.color() != theme::hover_feedback_color(base) {
        fail(format!("{name} did not apply its hover color"));
    }
    let _ = control.handle_event(Event::Leave);
    pump(20);
    if control.color() != base {
        fail(format!("{name} did not restore its default color"));
    }
}

fn assert_standard_button_hover_feedback() {
    let mut clear = app::widget_from_id::<fltk::button::Button>("result_clear_all")
        .unwrap_or_else(|| fail("result clear button is missing"));
    let mut one_tab = app::widget_from_id::<fltk::button::CheckButton>("result_one_tab_per_query")
        .unwrap_or_else(|| fail("result one-tab-per-query control is missing"));
    assert_hover_feedback(&mut clear, "result clear button");
    assert_hover_feedback(&mut one_tab, "result one-tab-per-query control");

    let clear_base = clear.color();
    let _ = clear.handle_event(Event::Enter);
    clear.set_color(theme::selection_soft());
    let _ = clear.handle_event(Event::Leave);
    if clear.color() != theme::selection_soft() {
        fail("button hover restored over a runtime color change");
    }
    clear.set_color(clear_base);
    clear.redraw();
}

fn assert_additional_control_hover_feedback() {
    let mut isolation = app::widget_from_id::<fltk::menu::Choice>("query_transaction_isolation")
        .unwrap_or_else(|| fail("query transaction-isolation choice is missing"));
    let unit = app::widget_from_id::<fltk::menu::Choice>("result_page_unit")
        .unwrap_or_else(|| fail("result page unit control is missing"));
    if isolation.color() != theme::input_bg()
        || isolation.color() != unit.color()
        || isolation.text_color() != unit.text_color()
        || isolation.selection_color() != unit.selection_color()
        || isolation.frame() != unit.frame()
    {
        fail("query choice styling does not match the result page unit choice");
    }
    let isolation_was_active = isolation.active();
    if !isolation_was_active {
        isolation.activate();
    }
    assert_hover_feedback(&mut isolation, "query transaction-isolation choice");
    if !isolation_was_active {
        isolation.deactivate();
    }

    let mut vertical_splitter = app::widget_from_id::<fltk::frame::Frame>("main_vertical_splitter")
        .unwrap_or_else(|| fail("main vertical splitter is missing"));
    let mut query_result_splitter =
        app::widget_from_id::<fltk::frame::Frame>("query_result_splitter")
            .unwrap_or_else(|| fail("query/result splitter is missing"));
    assert_hover_feedback(&mut vertical_splitter, "main vertical splitter");
    assert_hover_feedback(&mut query_result_splitter, "query/result splitter");

    let mut cancel = app::widget_from_id::<fltk::button::Button>("query_cancel")
        .unwrap_or_else(|| fail("query cancel button is missing"));
    let _ = cancel.handle_event(Event::Enter);
    pump(120);
    if cancel.color() != theme::hover_feedback_color(theme::button_cancel()) {
        fail("query cancel hover was not composed with its activity color");
    }
    let _ = cancel.handle_event(Event::Leave);
    pump(120);
    if cancel.color() != theme::button_cancel() {
        fail("query cancel did not restore its activity color after hover");
    }
}

fn capture_result_page_resizes(capture_paths: [&str; 2], expected_sizes: &[(i32, i32)]) {
    let mut window =
        app::widget_from_id::<Window>("main_window").unwrap_or_else(|| fail("main window"));
    let original_bounds = (window.x(), window.y(), window.w(), window.h());
    assert_result_page_control_layout(expected_sizes);

    for ((width, height), capture_path) in [(1000, 700), (800, 600)].into_iter().zip(capture_paths)
    {
        window.resize(original_bounds.0, original_bounds.1, width, height);
        pump(300);
        assert_result_page_control_layout(expected_sizes);
        save_main(capture_path);
    }

    window.resize(
        original_bounds.0,
        original_bounds.1,
        original_bounds.2,
        original_bounds.3,
    );
    pump(300);
    assert_result_page_control_layout(expected_sizes);
}

fn capture_result_grid(main_window: &mut MainWindow) {
    let columns = [
        ("EMPNO", "NUMBER"),
        ("ENAME", "VARCHAR2"),
        ("JOB", "VARCHAR2"),
        ("DEPTNO", "NUMBER"),
        ("SAL", "NUMBER"),
        ("HIREDATE", "DATE"),
    ];
    let rows: &[&[&str]] = &[
        &["7369", "SMITH", "CLERK", "20", "800", "1980-12-17"],
        &["7499", "ALLEN", "SALESMAN", "30", "1600", "1981-02-20"],
        &["7521", "WARD", "SALESMAN", "30", "1250", "1981-02-22"],
        &["7566", "JONES", "MANAGER", "20", "2975", "1981-04-02"],
        &["7654", "MARTIN", "SALESMAN", "30", "1250", "1981-09-28"],
        &["7698", "BLAKE", "MANAGER", "30", "2850", "1981-05-01"],
        &["7782", "CLARK", "MANAGER", "10", "2450", "1981-06-09"],
        &["7788", "SCOTT", "ANALYST", "20", "3000", "1987-04-19"],
        &["7839", "KING", "PRESIDENT", "10", "5000", "1981-11-17"],
        &["7844", "TURNER", "SALESMAN", "30", "1500", "1981-09-08"],
        &["7876", "ADAMS", "CLERK", "20", "1100", "1987-05-23"],
        &["7900", "JAMES", "CLERK", "30", "950", "1981-12-03"],
        &["7902", "FORD", "ANALYST", "20", "3000", "1981-12-03"],
        &["7934", "MILLER", "CLERK", "10", "1300", "1982-01-23"],
    ];
    main_window
        .capture_tour_show_result(
            "Result",
            make_result(&columns, rows, "SELECT * FROM EMP ORDER BY EMPNO"),
            false,
            Some((1, 1, 3, 2)),
        )
        .unwrap_or_else(|err| fail(format!("show result: {err}")));
    pump(350);
    save_main("/tmp/space-query-main.ppm");
    save_main("/tmp/space-query-result-grid.ppm");

    let default_unit = app::widget_from_id::<fltk::menu::Choice>("result_page_unit")
        .and_then(|choice| choice.choice());
    if default_unit.as_deref() != Some("500") {
        fail(format!(
            "result page unit should default to 500, got {default_unit:?}"
        ));
    }
    let expected_page_control_sizes = result_page_control_sizes();
    capture_result_page_resizes(
        [
            "/tmp/space-query-result-resized-1000.ppm",
            "/tmp/space-query-result-resized-800.ppm",
        ],
        &expected_page_control_sizes,
    );
    assert_result_page_control_feedback();
    assert_standard_button_hover_feedback();
    assert_additional_control_hover_feedback();

    let mut unit = app::widget_from_id::<fltk::menu::Choice>("result_page_unit")
        .unwrap_or_else(|| fail("result page unit control is missing"));
    unit.set_value(0);
    for (widget_id, capture_path) in [
        (
            "result_page_next",
            Some("/tmp/space-query-result-paging.ppm"),
        ),
        ("result_page_previous", None),
        ("result_page_last", None),
        ("result_page_first", None),
    ] {
        let mut button = app::widget_from_id::<fltk::button::Button>(widget_id)
            .unwrap_or_else(|| fail(format!("{widget_id} control is missing")));
        button.do_callback();
        pump(80);
        if let Some(capture_path) = capture_path {
            save_main(capture_path);
        }
    }
}

fn capture_table_browse_input_popup(
    input: &fltk::input::Input,
    popup: &Arc<Mutex<IntellisensePopup>>,
    label: &str,
    capture_path: &str,
) {
    let input_window = input
        .window()
        .unwrap_or_else(|| fail(format!("{label} input window is missing")));
    let input_left = input_window.x_root() + input.x();
    let (caret_x, input_top, input_bottom) = input_caret_popup_anchor(input);
    if caret_x
        <= input_left
            .saturating_add(input.frame().dx())
            .saturating_add(1)
    {
        fail(format!(
            "{label} caret anchor {caret_x} did not advance past the non-empty input's text origin"
        ));
    }
    let (actual_position, popup_dimensions, visible) = {
        let popup = popup
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        (
            popup.popup_position(),
            popup.popup_dimensions(),
            popup.is_visible(),
        )
    };
    if !visible {
        fail(format!("{label} completion popup is not visible"));
    }
    let screen = app::screen_num(caret_x, input_bottom);
    let (screen_x, _, screen_width, _) = app::screen_work_area(screen);
    let max_x = screen_x
        .saturating_add(screen_width)
        .saturating_sub(popup_dimensions.0)
        .max(screen_x);
    let expected_x = caret_x.clamp(screen_x, max_x);
    let aligned_below = actual_position.1 == input_bottom;
    let aligned_above = actual_position.1 + popup_dimensions.1 == input_top;
    println!(
        "{label} input_left={input_left} caret_x={caret_x} popup={actual_position:?} size={popup_dimensions:?}"
    );
    if actual_position.0 != expected_x || (!aligned_below && !aligned_above) {
        fail(format!(
            "{label} popup position {actual_position:?} size {popup_dimensions:?} is not aligned with caret {caret_x} and input vertical bounds ({input_top}, {input_bottom})"
        ));
    }

    let mut main =
        app::widget_from_id::<Window>("main_window").unwrap_or_else(|| fail("main window"));
    let main_x = main.x_root();
    let main_y = main.y_root();
    let (mut canvas, width, height) = capture_complete_rgb(&mut main);
    let mut popup_window = window_by_root_bounds(actual_position, popup_dimensions, &main)
        .unwrap_or_else(|| fail(format!("{label} popup window")));
    composite_popup(
        &mut canvas,
        width,
        height,
        main_x,
        main_y,
        &mut popup_window,
    );
    save_ppm(capture_path, &canvas, width, height);
    popup
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .hide();
}

fn show_table_browse_popup_through_input_handler(
    input: &fltk::input::Input,
    popup: &Arc<Mutex<IntellisensePopup>>,
    label: &str,
) {
    popup
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .hide();
    let mut input = input.clone();
    input
        .take_focus()
        .unwrap_or_else(|err| fail(format!("focus {label} input: {err}")));
    pump(40);
    // KeyUp intentionally returns false so FLTK may continue normal
    // propagation after the completion popup has been refreshed.
    let _ = input.handle_event(Event::KeyUp);
    pump(80);
    let popup_visible = popup
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .is_visible();
    if !popup_visible {
        fail(format!(
            "{label} popup was not shown through the input key-up handler"
        ));
    }
    if !input.has_focus() {
        fail(format!(
            "{label} input did not retain focus after showing the popup"
        ));
    }
    let _ = input.handle_event(Event::KeyUp);
    pump(80);
    if !input.has_focus() {
        fail(format!(
            "{label} input lost focus before the next key-up event"
        ));
    }
}

fn assert_unfocused_table_browse_input_does_not_reclaim_focus(
    unfocused_input: &fltk::input::Input,
    unfocused_popup: &Arc<Mutex<IntellisensePopup>>,
    focused_input: &fltk::input::Input,
    label: &str,
) {
    unfocused_popup
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .hide();
    let mut focused_input = focused_input.clone();
    focused_input
        .take_focus()
        .unwrap_or_else(|err| fail(format!("focus peer input for {label}: {err}")));
    pump(40);

    let mut unfocused_input = unfocused_input.clone();
    let _ = unfocused_input.handle_event(Event::Paste);
    pump(80);
    if unfocused_popup
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .is_visible()
    {
        fail(format!("unfocused {label} input reopened its popup"));
    }
    if !focused_input.has_focus() {
        fail(format!(
            "unfocused {label} input reclaimed focus from its peer"
        ));
    }
}

fn assert_table_browse_suppressed_contexts(
    input: &fltk::input::Input,
    popup: &Arc<Mutex<IntellisensePopup>>,
    label: &str,
) {
    let mut input = input.clone();
    input
        .take_focus()
        .unwrap_or_else(|err| fail(format!("focus {label} suppressed-context input: {err}")));
    for (value, context) in [
        ("topic='", "string literal"),
        ("DEPTNO = 20 AND ", "empty identifier prefix"),
    ] {
        input.set_value(value);
        let _ = input.set_position(i32::try_from(input.value().len()).unwrap_or(i32::MAX));
        let _ = input.handle_event(Event::Paste);
        pump(80);
        if popup
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .is_visible()
        {
            fail(format!("{label} popup opened in {context}"));
        }
        if !input.has_focus() {
            fail(format!("{label} {context} check lost input focus"));
        }
    }
}

fn capture_table_browse_popup(main_window: &mut MainWindow, verify_input_handlers: bool) {
    let columns = [
        ("EMPNO", "NUMBER"),
        ("ENAME", "VARCHAR2"),
        ("JOB", "VARCHAR2"),
        ("DEPTNO", "NUMBER"),
        ("SAL", "NUMBER"),
    ];
    let rows: &[&[&str]] = &[
        &["7369", "SMITH", "CLERK", "20", "800"],
        &["7499", "ALLEN", "SALESMAN", "30", "1600"],
        &["7521", "WARD", "SALESMAN", "30", "1250"],
        &["7566", "JONES", "MANAGER", "20", "2975"],
    ];
    let (where_input, popup) = main_window
        .capture_tour_show_table_browse_popup(make_result(
            &columns,
            rows,
            "SELECT * FROM SCOTT.EMP ORDER BY EMPNO",
        ))
        .unwrap_or_else(|err| fail(format!("show table browse popup: {err}")));
    if verify_input_handlers {
        show_table_browse_popup_through_input_handler(&where_input, &popup, "WHERE");
        let (order_input, order_popup) = main_window
            .capture_tour_show_table_browse_order_popup()
            .unwrap_or_else(|err| fail(format!("show ORDER BY popup: {err}")));
        show_table_browse_popup_through_input_handler(&order_input, &order_popup, "ORDER BY");
        assert_unfocused_table_browse_input_does_not_reclaim_focus(
            &where_input,
            &popup,
            &order_input,
            "WHERE",
        );
        assert_unfocused_table_browse_input_does_not_reclaim_focus(
            &order_input,
            &order_popup,
            &where_input,
            "ORDER BY",
        );
        assert_table_browse_suppressed_contexts(&where_input, &popup, "WHERE");
        order_popup
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .hide();
        return;
    }
    pump(350);
    let scale = std::env::var("SPACE_QUERY_CAPTURE_UI_SCALE")
        .ok()
        .and_then(|value| value.parse::<u32>().ok())
        .unwrap_or(100);
    capture_table_browse_input_popup(
        &where_input,
        &popup,
        "WHERE",
        &format!("/tmp/space-query-table-browse-popup-{scale}.ppm"),
    );
    let (order_input, order_popup) = main_window
        .capture_tour_show_table_browse_order_popup()
        .unwrap_or_else(|err| fail(format!("show ORDER BY popup: {err}")));
    if (where_input.w() - order_input.w()).abs() > 1 {
        fail(format!(
            "table browse inputs are not evenly split: WHERE={} ORDER BY={}",
            where_input.w(),
            order_input.w()
        ));
    }
    pump(250);
    capture_table_browse_input_popup(
        &order_input,
        &order_popup,
        "ORDER BY",
        &format!("/tmp/space-query-table-browse-order-popup-{scale}.ppm"),
    );
    pump(120);
    save_main("/tmp/space-query-table-browse.ppm");
}

fn capture_result_editing(main_window: &mut MainWindow) {
    let columns = [
        ("ROWID", "ROWID"),
        ("EMPNO", "NUMBER"),
        ("ENAME", "VARCHAR2"),
        ("JOB", "VARCHAR2"),
        ("SAL", "NUMBER"),
        ("DEPTNO", "NUMBER"),
    ];
    let rows: &[&[&str]] = &[
        &["AAAPr9AAEAAAACXAAA", "7369", "SMITH", "CLERK", "800", "20"],
        &[
            "AAAPr9AAEAAAACXAAB",
            "7499",
            "ALLEN",
            "SALESMAN",
            "1600",
            "30",
        ],
        &[
            "AAAPr9AAEAAAACXAAC",
            "7566",
            "JONES",
            "MANAGER",
            "2975",
            "20",
        ],
        &[
            "AAAPr9AAEAAAACXAAD",
            "7788",
            "SCOTT",
            "ANALYST",
            "3000",
            "20",
        ],
        &[
            "AAAPr9AAEAAAACXAAE",
            "7839",
            "KING",
            "PRESIDENT",
            "5000",
            "10",
        ],
    ];
    main_window
        .capture_tour_show_result(
            "Result",
            make_result(
                &columns,
                rows,
                "SELECT ROWID, EMPNO, ENAME, JOB, SAL, DEPTNO FROM EMP",
            ),
            false,
            None,
        )
        .unwrap_or_else(|err| fail(format!("show editable result: {err}")));
    pump(350);

    let mut edit_mode = app::widget_from_id::<fltk::button::CheckButton>("result_edit_mode")
        .unwrap_or_else(|| fail("result edit-mode control is missing"));
    if !edit_mode.visible() || edit_mode.value() {
        fail("result edit mode should initially be visible and unchecked");
    }
    let expected_page_control_sizes = result_page_control_sizes();
    save_main("/tmp/space-query-result-editing-off.ppm");
    capture_result_page_resizes(
        [
            "/tmp/space-query-result-editing-off-resized-1000.ppm",
            "/tmp/space-query-result-editing-off-resized-800.ppm",
        ],
        &expected_page_control_sizes,
    );

    edit_mode.set(true);
    edit_mode.do_callback();
    pump(300);
    if !edit_mode.value() {
        fail("result edit mode should be checked after enabling it");
    }
    for widget_id in [
        "result_edit_insert",
        "result_edit_delete",
        "result_edit_save",
        "result_edit_cancel",
    ] {
        let button = app::widget_from_id::<fltk::button::Button>(widget_id)
            .unwrap_or_else(|| fail(format!("{widget_id} control is missing")));
        if !button.visible() {
            fail(format!(
                "{widget_id} control should be visible in edit mode"
            ));
        }
    }
    save_main("/tmp/space-query-result-editing.ppm");
    capture_result_page_resizes(
        [
            "/tmp/space-query-result-editing-resized-1000.ppm",
            "/tmp/space-query-result-editing-resized-800.ppm",
        ],
        &expected_page_control_sizes,
    );

    edit_mode.set(false);
    edit_mode.do_callback();
    pump(300);
    if edit_mode.value() {
        fail("result edit mode should be unchecked after disabling it");
    }
    for widget_id in [
        "result_edit_insert",
        "result_edit_delete",
        "result_edit_save",
        "result_edit_cancel",
    ] {
        let button = app::widget_from_id::<fltk::button::Button>(widget_id)
            .unwrap_or_else(|| fail(format!("{widget_id} control is missing")));
        if button.visible() {
            fail(format!(
                "{widget_id} control should be hidden outside edit mode"
            ));
        }
    }
    assert_result_page_control_layout(&expected_page_control_sizes);
    save_main("/tmp/space-query-result-editing-off-again.ppm");
}

fn capture_session_activity(main_window: &mut MainWindow) {
    let columns = [
        ("CONNECTION", "VARCHAR2"),
        ("DATABASE", "VARCHAR2"),
        ("POOL SIZE", "NUMBER"),
        ("TAB", "VARCHAR2"),
        ("RESULT TAB", "VARCHAR2"),
        ("STATE", "VARCHAR2"),
        ("CURRENT ACTIVITY", "VARCHAR2"),
        ("SQL PREVIEW", "VARCHAR2"),
        ("FETCHED ROWS", "NUMBER"),
        ("ELAPSED", "VARCHAR2"),
    ];
    let rows: &[&[&str]] = &[
        &[
            "Local Oracle",
            "Oracle",
            "12",
            "Query 1",
            "1",
            "Fetching",
            "Fetching query rows",
            "SELECT * FROM EMP",
            "2,000",
            "4s",
        ],
        &[
            "Local Oracle",
            "Oracle",
            "12",
            "Query 2",
            "—",
            "Executing",
            "Fetching query rows",
            "SELECT * FROM DEPT",
            "0",
            "1s",
        ],
        &[
            "Local Oracle",
            "Oracle",
            "12",
            "Query 3",
            "2",
            "Idle",
            "Fetching query rows",
            "SELECT * FROM SALGRADE",
            "120",
            "2m 18s",
        ],
    ];
    main_window
        .capture_tour_show_result(
            "Session Activity",
            make_result(&columns, rows, "SESSION ACTIVITY"),
            false,
            None,
        )
        .unwrap_or_else(|err| fail(format!("show session activity: {err}")));
    pump(350);
    save_main("/tmp/space-query-session-activity.ppm");
}

fn capture_dialogs(config: &AppConfig) {
    app::add_timeout3(0.45, |_| {
        capture_active_dialog("Connect to Database", "/tmp/space-query-connect.ppm")
    });
    let _ = ConnectionDialog::show_with_registry(Arc::new(Mutex::new(Vec::new())));

    app::add_timeout3(0.45, |_| {
        capture_active_dialog("Settings", "/tmp/space-query-settings.ppm")
    });
    let _ = show_settings_dialog(config);

    let _ = space_query::ui::query_history::clear_history();
    let _ = QueryHistoryDialog::add_to_history(
        "SELECT e.empno, e.ename, d.dname, e.sal\nFROM emp e\nJOIN dept d ON d.deptno = e.deptno\nWHERE e.sal > 2000\nORDER BY e.sal DESC;",
        18,
        6,
        "Local Oracle",
        None,
        true,
        "6 rows selected",
    );
    let _ = QueryHistoryDialog::add_to_history(
        "SELECT * FROM missing_table;",
        4,
        0,
        "Local Oracle",
        None,
        false,
        "ORA-00942: table or view does not exist",
    );
    pump(500);
    app::add_timeout3(0.20, |_| {
        select_first_browser_in_active_dialog("Query History")
    });
    app::add_timeout3(0.65, |_| {
        capture_active_dialog("Query History", "/tmp/space-query-query-history.ppm")
    });
    let _ = QueryHistoryDialog::show_with_registry(Arc::new(Mutex::new(Vec::new())));

    let _ = logging::clear_log();
    logging::log_info("app", "SPACE Query started with Oracle Thin support");
    logging::log_info("connection", "Connected to Local Oracle (Oracle)");
    logging::log_debug("query", "Fetched the first 1,000 rows for Result 1");
    logging::log_warning(
        "pool",
        "One result tab is retaining a pooled session for lazy fetch",
    );
    logging::log_error("query", "ORA-00942: table or view does not exist");
    let _ = logging::flush_log_writer();
    app::add_timeout3(0.20, |_| {
        select_first_browser_in_active_dialog("Application Log")
    });
    app::add_timeout3(0.65, |_| {
        capture_active_dialog("Application Log", "/tmp/space-query-application-log.ppm")
    });
    LogViewerDialog::show(Arc::new(Mutex::new(Vec::new())));
}

fn main() {
    let capture_mode = std::env::args().nth(1);
    let ui_scale_percent = std::env::var("SPACE_QUERY_CAPTURE_UI_SCALE")
        .ok()
        .and_then(|value| value.parse::<u32>().ok())
        .map(AppConfig::clamp_ui_scale_percent)
        .unwrap_or(100);
    let config = AppConfig {
        editor_font: "D2Coding".to_string(),
        result_font: "D2Coding".to_string(),
        ui_font_size: 16,
        editor_font_size: 16,
        result_font_size: 16,
        ui_scale_percent,
        ..AppConfig::default()
    };
    if !matches!(
        capture_mode.as_deref(),
        Some("result-paging" | "result-editing" | "connection-dialog" | "settings-dialog")
    ) {
        config
            .save()
            .unwrap_or_else(|err| fail(format!("save isolated capture config: {err}")));
    }

    let _app = app::App::default()
        .with_scheme(app::Scheme::Gtk)
        .load_system_fonts();
    apply_global_default_font(profile_by_name("D2Coding").normal);
    app::set_font_size(16);
    Tooltip::set_font(profile_by_name("D2Coding").normal);
    Tooltip::set_font_size(16);
    let (bg_r, bg_g, bg_b) = theme::app_background().to_rgb();
    app::background(bg_r, bg_g, bg_b);
    let (fg_r, fg_g, fg_b) = theme::app_foreground().to_rgb();
    app::foreground(fg_r, fg_g, fg_b);
    app::set_frame_type2(FrameType::UpBox, FrameType::RFlatBox);
    app::set_frame_type2(FrameType::DownBox, FrameType::RFlatBox);
    theme::register_text_input_frame();
    app::set_frame_border_radius_max(8);

    let mut main_window = MainWindow::new_with_config(config.clone());
    main_window.setup_callbacks();
    main_window.show();
    pump(300);

    if capture_mode.as_deref() == Some("connection-dialog") {
        app::add_timeout3(0.45, |_| {
            capture_active_dialog("Connect to Database", "/tmp/space-query-connect.ppm")
        });
        let _ = ConnectionDialog::show_with_registry(Arc::new(Mutex::new(Vec::new())));
        app::quit();
        return;
    }
    if capture_mode.as_deref() == Some("settings-dialog") {
        app::add_timeout3(0.45, |_| {
            capture_active_dialog("Settings", "/tmp/space-query-settings.ppm")
        });
        let _ = show_settings_dialog(&config);
        app::quit();
        return;
    }
    if capture_mode.as_deref() == Some("object-browser") {
        capture_object_browser(&mut main_window);
        app::quit();
        return;
    }
    if capture_mode.as_deref() == Some("result-paging") {
        capture_object_browser(&mut main_window);
        capture_result_grid(&mut main_window);
        app::quit();
        return;
    }
    if capture_mode.as_deref() == Some("result-editing") {
        capture_object_browser(&mut main_window);
        capture_result_editing(&mut main_window);
        app::quit();
        return;
    }
    if capture_mode.as_deref() == Some("table-browse-popup") {
        capture_table_browse_popup(&mut main_window, false);
        app::quit();
        return;
    }
    if capture_mode.as_deref() == Some("table-browse-input-regression") {
        capture_table_browse_popup(&mut main_window, true);
        app::quit();
        return;
    }
    capture_object_browser(&mut main_window);
    capture_intellisense(&mut main_window);
    capture_signature_popup(&mut main_window);
    capture_formatting(&mut main_window);
    capture_result_grid(&mut main_window);
    capture_table_browse_popup(&mut main_window, false);
    capture_result_editing(&mut main_window);
    capture_session_activity(&mut main_window);
    capture_dialogs(&config);
    app::quit();
}

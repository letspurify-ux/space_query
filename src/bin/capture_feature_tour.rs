use fltk::{
    app, browser::HoldBrowser, draw, enums::FrameType, misc::Tooltip, prelude::*, window::Window,
};
use space_query::{
    db::{ColumnInfo, QueryResult},
    ui::{
        apply_global_default_font, log_viewer::LogViewerDialog, profile_by_name,
        show_settings_dialog, theme, ConnectionDialog, IntellisensePopup, MainWindow,
        QueryHistoryDialog, SignatureLabel, SignatureOverload, SignaturePopup,
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

fn capture_active_dialog(expected_label: &str, path: &str) {
    let mut window = window_by_label(expected_label)
        .unwrap_or_else(|| fail(format!("missing dialog: {expected_label}")));
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
    save_main("/tmp/space-query-result-grid.ppm");
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
            true,
            None,
        )
        .unwrap_or_else(|err| fail(format!("show editable result: {err}")));
    pump(350);
    save_main("/tmp/space-query-result-editing.ppm");
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
    config
        .save()
        .unwrap_or_else(|err| fail(format!("save isolated capture config: {err}")));

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
    app::set_frame_border_radius_max(8);

    let mut main_window = MainWindow::new_with_config(config.clone());
    main_window.setup_callbacks();
    main_window.show();
    pump(300);
    save_main("/tmp/space-query-main.ppm");

    if std::env::args().nth(1).as_deref() == Some("object-browser") {
        capture_object_browser(&mut main_window);
        app::quit();
        return;
    }

    capture_intellisense(&mut main_window);
    capture_signature_popup(&mut main_window);
    capture_object_browser(&mut main_window);
    capture_formatting(&mut main_window);
    capture_result_grid(&mut main_window);
    capture_result_editing(&mut main_window);
    capture_session_activity(&mut main_window);
    capture_dialogs(&config);
    app::quit();
}

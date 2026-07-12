use fltk::{
    app, browser::HoldBrowser, draw, enums::FrameType, misc::Tooltip, prelude::*, window::Window,
};
use space_query::{
    db::{ColumnInfo, QueryResult},
    ui::{
        log_viewer::LogViewerDialog, profile_by_name, show_settings_dialog, theme,
        ConnectionDialog, IntellisensePopup, MainWindow, QueryHistoryDialog,
    },
    utils::{logging, AppConfig},
};
use std::{
    fs::File,
    io::Write,
    sync::{Arc, Mutex},
    thread,
    time::Duration,
};

fn pump(milliseconds: u64) {
    for _ in 0..(milliseconds / 20).max(1) {
        app::check();
        thread::sleep(Duration::from_millis(20));
    }
}

fn capture_rgb<W: WindowExt>(window: &mut W) -> (Vec<u8>, i32, i32) {
    app::flush();
    let image = draw::capture_window(window).expect("capture window");
    (image.to_rgb_data(), image.data_w(), image.data_h())
}

fn save_ppm(path: &str, data: &[u8], width: i32, height: i32) {
    let mut file = File::create(path).expect("create capture");
    write!(file, "P6\n{width} {height}\n255\n").expect("write PPM header");
    file.write_all(data).expect("write PPM pixels");
}

fn save_main(path: &str) {
    let mut window = app::widget_from_id::<Window>("main_window").expect("main window");
    window.hide();
    app::flush();
    window.show();
    window.set_damage(true);
    window.redraw();
    app::redraw();
    pump(250);
    let _ = capture_rgb(&mut window);
    pump(150);
    let (data, width, height) = capture_rgb(&mut window);
    save_ppm(path, &data, width, height);
}

fn save_main_part(path: &str, x: i32, y: i32, width: i32, height: i32) {
    let mut window = app::widget_from_id::<Window>("main_window").expect("main window");
    window.set_damage(true);
    window.redraw();
    app::redraw();
    pump(400);
    let (data, full_width, full_height) = capture_rgb(&mut window);
    assert!(x >= 0 && y >= 0 && x + width <= full_width && y + height <= full_height);
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
        .unwrap_or_else(|| panic!("missing dialog: {expected_label}"));
    let (data, width, height) = capture_rgb(&mut window);
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

fn capture_intellisense(main_window: &mut MainWindow) {
    let sql = "SELECT e.empno,\n       e.ename,\n       e.\nFROM emp e\nWHERE e.sal > 2000;";
    let cursor = sql.find("e.\nFROM").expect("completion cursor") as i32 + 2;
    let editor = main_window.capture_tour_set_sql(sql, Some(cursor));
    pump(300);

    let mut main = app::widget_from_id::<Window>("main_window").expect("main window");
    let main_x = main.x_root();
    let main_y = main.y_root();
    main.set_damage(true);
    main.redraw();
    app::redraw();
    pump(200);
    let _ = capture_rgb(&mut main);
    pump(120);
    let (mut canvas, width, height) = capture_rgb(&mut main);

    let mut popup = IntellisensePopup::new();
    let popup_width = 320;
    let popup_height = 8 * (16 + 6) + 10;
    let (cursor_x, cursor_y) = editor.position_to_xy(cursor);
    let editor_window = editor.window().expect("editor window");
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
    let mut popup_window = app::first_window().expect("intellisense popup");
    let offset_x = popup_window.x_root() - main_x;
    let offset_y = popup_window.y_root() - main_y;
    let (popup_data, popup_width, popup_height) = capture_rgb(&mut popup_window);
    for y in 0..popup_height {
        let target_y = offset_y + y;
        if !(0..height).contains(&target_y) {
            continue;
        }
        for x in 0..popup_width {
            let target_x = offset_x + x;
            if !(0..width).contains(&target_x) {
                continue;
            }
            let source = ((y * popup_width + x) * 3) as usize;
            let target = ((target_y * width + target_x) * 3) as usize;
            canvas[target..target + 3].copy_from_slice(&popup_data[source..source + 3]);
        }
    }
    save_ppm("/tmp/space-query-intellisense.ppm", &canvas, width, height);
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
    save_main_part("/tmp/space-query-object-browser.ppm", 0, 70, 250, 705);
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
            "EMP rows",
            make_result(&columns, rows, "SELECT * FROM EMP ORDER BY EMPNO"),
            false,
        )
        .expect("show result");
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
            "Editable EMP",
            make_result(
                &columns,
                rows,
                "SELECT ROWID, EMPNO, ENAME, JOB, SAL, DEPTNO FROM EMP",
            ),
            true,
        )
        .expect("show editable result");
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
        ("FETCHED ROWS", "NUMBER"),
        ("ELAPSED", "VARCHAR2"),
    ];
    let rows: &[&[&str]] = &[
        &[
            "Local Oracle",
            "Oracle",
            "12",
            "Query 1",
            "Result 1",
            "Fetching",
            "Lazy fetch",
            "2,000",
            "00:04",
        ],
        &[
            "Local Oracle",
            "Oracle",
            "12",
            "Query 2",
            "—",
            "Executing",
            "SELECT",
            "—",
            "00:01",
        ],
        &[
            "Local Oracle",
            "Oracle",
            "12",
            "Query 3",
            "Result 2",
            "Idle",
            "Retained session",
            "120",
            "02:18",
        ],
    ];
    main_window
        .capture_tour_show_result(
            "Session Activity",
            make_result(&columns, rows, "SESSION ACTIVITY"),
            false,
        )
        .expect("show session activity");
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
        true,
        "6 rows selected",
    );
    let _ = QueryHistoryDialog::add_to_history(
        "SELECT * FROM missing_table;",
        4,
        0,
        "Local Oracle",
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
    let mut config = AppConfig::default();
    config.editor_font = "D2Coding".to_string();
    config.result_font = "D2Coding".to_string();
    config.ui_font_size = 16;
    config.editor_font_size = 16;
    config.result_font_size = 16;
    config.recent_connections.clear();
    config.recent_sql_files.clear();
    config.last_connection = None;
    config.save().expect("save isolated capture config");

    let _app = app::App::default()
        .with_scheme(app::Scheme::Gtk)
        .load_system_fonts();
    app::set_font(profile_by_name("D2Coding").normal);
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
    capture_object_browser(&mut main_window);
    capture_formatting(&mut main_window);
    capture_result_grid(&mut main_window);
    capture_result_editing(&mut main_window);
    capture_session_activity(&mut main_window);
    capture_dialogs(&config);
    app::quit();
}

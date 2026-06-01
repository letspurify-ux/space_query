use crate::ui::{theme, MainWindow};
use crate::utils::{self, AppConfig};
use fltk::{app, enums::FrameType};

pub struct StartupContext {
    pub config: AppConfig,
    pub crash_report: Option<String>,
}

pub struct App;

impl App {
    pub fn new() -> Self {
        Self
    }

    fn bootstrap() -> StartupContext {
        let config = AppConfig::load();
        let crash_report = utils::logging::take_crash_log();

        StartupContext {
            config,
            crash_report,
        }
    }

    pub fn run(&self) {
        let startup = Self::bootstrap();

        let app = app::App::default()
            .with_scheme(app::Scheme::Gtk)
            .load_system_fonts();

        configure_fltk_globals(&startup.config);

        let current_group = fltk::group::Group::try_current();
        fltk::group::Group::set_current(None::<&fltk::group::Group>);

        let mut main_window = MainWindow::new_with_config(startup.config);
        main_window.setup_callbacks();
        main_window.show();

        if let Some(crash_report) = startup.crash_report.as_deref() {
            MainWindow::show_previous_crash_report(crash_report);
        }

        match app.run() {
            Ok(()) => {}
            Err(err) => {
                utils::logging::log_error("app", &format!("App run error: {err}"));
                eprintln!("Failed to run app: {err}");
            }
        }

        if let Some(ref group) = current_group {
            fltk::group::Group::set_current(Some(group));
        }
    }
}

impl Default for App {
    fn default() -> Self {
        Self::new()
    }
}

pub(crate) fn configure_fltk_globals(config: &AppConfig) {
    let ui_size = config.ui_font_size.clamp(8, 24) as i32;
    app::set_font_size(ui_size);
    fltk::misc::Tooltip::set_font_size(ui_size);

    let (bg_r, bg_g, bg_b) = theme::app_background().to_rgb();
    app::background(bg_r, bg_g, bg_b);
    let (fg_r, fg_g, fg_b) = theme::app_foreground().to_rgb();
    app::foreground(fg_r, fg_g, fg_b);

    app::set_frame_type2(FrameType::UpBox, FrameType::RFlatBox);
    app::set_frame_type2(FrameType::DownBox, FrameType::RFlatBox);
    app::set_frame_border_radius_max(8);
}

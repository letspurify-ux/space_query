// Unified UI sizing constants for design consistency across all components.

// -- Button sizing --

/// Standard button width used in all dialogs and toolbars.
pub const BUTTON_WIDTH: i32 = 90;

/// Compact button width for secondary actions (e.g. "Close").
pub const BUTTON_WIDTH_SMALL: i32 = 70;

/// Wide button width for primary actions needing more label space.
pub const BUTTON_WIDTH_LARGE: i32 = 110;

/// Width for the transaction isolation selector in the query toolbar.
pub const TRANSACTION_ISOLATION_CHOICE_WIDTH: i32 = 230;

/// Width for the transaction access mode selector in the query toolbar.
pub const TRANSACTION_ACCESS_CHOICE_WIDTH: i32 = 130;

/// Uniform button height across the entire UI.
pub const BUTTON_HEIGHT: i32 = 28;

// -- Form row heights --

/// Height of a row containing a text input field.
pub const INPUT_ROW_HEIGHT: i32 = 30;

/// Height of a row containing action buttons at the bottom of a dialog.
pub const BUTTON_ROW_HEIGHT: i32 = 28;

/// Height of a row containing only a label/frame.
pub const LABEL_ROW_HEIGHT: i32 = 22;

/// Height of a row containing a checkbox.
pub const CHECKBOX_ROW_HEIGHT: i32 = 28;

// -- Spacing and margins --

/// Outer margin inside dialog windows (distance from edge to content).
pub const DIALOG_MARGIN: i32 = 10;

/// Vertical spacing between rows in dialogs and forms.
pub const DIALOG_SPACING: i32 = 8;

/// Horizontal spacing between buttons in toolbars.
pub const TOOLBAR_SPACING: i32 = 6;

// -- Form label widths --

/// Width of left-side labels in form layouts (e.g. "Username:", "Host:").
pub const FORM_LABEL_WIDTH: i32 = 100;

// -- Numeric input widths --

/// Width for small numeric input fields (e.g. font size, timeout).
pub const NUMERIC_INPUT_WIDTH: i32 = 70;

// -- Layout constants --

/// Height of the application menu bar.
pub const MENU_BAR_HEIGHT: i32 = 30;

/// Height of the application status bar.
pub const STATUS_BAR_HEIGHT: i32 = 25;

/// Height of the filter input in the object browser.
pub const FILTER_INPUT_HEIGHT: i32 = 28;

// -- Table constants --

/// Row header width in result tables.
pub const TABLE_ROW_HEADER_WIDTH: i32 = 55;

/// Column header height in result tables.
pub const TABLE_COL_HEADER_HEIGHT: i32 = 28;

/// Default row height in result tables.
pub const TABLE_ROW_HEIGHT: i32 = 26;

/// Cell text padding (left/right) in result tables.
pub const TABLE_CELL_PADDING: i32 = 4;

/// Default maximum number of characters shown per result cell.
pub const RESULT_CELL_MAX_DISPLAY_CHARS_DEFAULT: u32 = 50;

/// Minimum allowed maximum for result cell preview length.
pub const RESULT_CELL_MAX_DISPLAY_CHARS_MIN: u32 = 8;

/// Maximum allowed maximum for result cell preview length.
pub const RESULT_CELL_MAX_DISPLAY_CHARS_MAX: u32 = 10_000;

// -- Result tabs --

/// Height of tab headers in the result tabs widget.
pub const TAB_HEADER_HEIGHT: i32 = 25;

/// Inner padding for the script output display.
pub const SCRIPT_OUTPUT_PADDING: i32 = 6;

/// Hard cap for script output buffer length to prevent unbounded growth.
pub const SCRIPT_OUTPUT_MAX_CHARS: usize = 2_000_000;

/// Target length after trimming script output buffer.
pub const SCRIPT_OUTPUT_TRIM_TARGET_CHARS: usize = 1_500_000;

// -- Splitter sizes --

/// Width of the main horizontal splitter between object browser and editor.
pub const MAIN_SPLITTER_WIDTH: i32 = 6;

/// Height of the query/results horizontal splitter in the right tile.
pub const QUERY_SPLIT_BAR_HEIGHT: i32 = 6;

/// Height of the result toolbar row.
pub const RESULT_TOOLBAR_HEIGHT: i32 = 34;

/// Minimum height for the query editor pane.
pub const MIN_QUERY_HEIGHT: i32 = 160;

/// Minimum height for the results body (excluding toolbar).
pub const MIN_RESULTS_BODY_HEIGHT: i32 = 160;

/// Minimum total height for the results section.
pub const MIN_RESULTS_HEIGHT: i32 = RESULT_TOOLBAR_HEIGHT + MIN_RESULTS_BODY_HEIGHT;

// -- Default font size --

/// Default font size used when no config value is available.
pub const DEFAULT_FONT_SIZE: i32 = 16;

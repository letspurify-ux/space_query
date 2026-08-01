pub fn display_version() -> &'static str {
    option_env!("SPACE_QUERY_DISPLAY_VERSION").unwrap_or(env!("CARGO_PKG_VERSION"))
}

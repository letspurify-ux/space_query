impl SqlEditorWidget {
    fn bounded_text_window(
        buffer: &TextBuffer,
        text_shadow: &Arc<Mutex<HighlightShadowState>>,
        start: i32,
        end: i32,
    ) -> (String, i32) {
        text_buffer_access::bounded_text_window(buffer, Some(text_shadow), start, end)
    }

    fn word_at_cursor(
        buffer: &TextBuffer,
        text_shadow: &Arc<Mutex<HighlightShadowState>>,
        cursor_pos: i32,
    ) -> (String, usize, usize) {
        let buffer_len = buffer.length().max(0);
        if buffer_len == 0 {
            return (String::new(), 0, 0);
        }
        let cursor_pos = cursor_pos.clamp(0, buffer_len);
        let start = (cursor_pos - INTELLISENSE_WORD_WINDOW).max(0);
        let end = (cursor_pos + INTELLISENSE_WORD_WINDOW).min(buffer_len);
        let (text, start) = Self::bounded_text_window(buffer, text_shadow, start, end);
        if text.is_empty() {
            let cursor = cursor_pos.max(0) as usize;
            return (String::new(), cursor, cursor);
        }
        let rel_cursor =
            Self::clamp_to_char_boundary_local(&text, (cursor_pos - start).max(0) as usize);
        let (word, rel_start, rel_end) = get_word_at_cursor(&text, rel_cursor);
        let abs_start = start as usize + rel_start;
        let abs_end = start as usize + rel_end;
        (word, abs_start, abs_end)
    }

    fn quoted_identifier_bounds_at(text: &str, rel_pos: usize) -> Option<(usize, usize)> {
        if text.is_empty() {
            return None;
        }

        let rel_pos = Self::clamp_to_char_boundary_local(text, rel_pos.min(text.len()));
        let mut idx = 0usize;

        while idx < text.len() {
            let ch = text.get(idx..)?.chars().next()?;
            if !matches!(ch, '"' | '`') {
                idx += ch.len_utf8();
                continue;
            }
            let quote = ch;

            let start = idx;
            idx += quote.len_utf8();

            while idx < text.len() {
                let cur = text.get(idx..)?.chars().next()?;
                if cur == quote {
                    let next_idx = idx + cur.len_utf8();
                    if next_idx < text.len() && text.get(next_idx..)?.starts_with(quote) {
                        idx = next_idx + quote.len_utf8();
                        continue;
                    }
                    let end = next_idx;
                    if rel_pos >= start && rel_pos <= end {
                        return Some((start, end));
                    }
                    idx = end;
                    break;
                }
                idx += cur.len_utf8();
            }

            if idx >= text.len() {
                return None;
            }
        }

        None
    }

    fn find_quoted_segment_start(
        text: &str,
        segment_end: usize,
        delimiter: char,
    ) -> Option<usize> {
        if segment_end == 0 {
            return None;
        }
        let prefix = text.get(..segment_end)?;
        let mut active_start: Option<usize> = None;
        let mut iter = prefix.char_indices().peekable();

        while let Some((idx, ch)) = iter.next() {
            if ch != delimiter {
                continue;
            }

            if let Some(start_idx) = active_start {
                if iter.peek().is_some_and(|(_, next)| *next == delimiter) {
                    iter.next();
                    continue;
                }
                if idx + ch.len_utf8() == segment_end {
                    return Some(start_idx);
                }
                active_start = None;
            } else {
                active_start = Some(idx);
            }
        }

        None
    }

    fn identifier_at_position_in_text(
        text: &str,
        rel_pos: usize,
    ) -> Option<(String, usize, usize)> {
        if text.is_empty() {
            return None;
        }

        let rel_pos = Self::clamp_to_char_boundary_local(text, rel_pos.min(text.len()));

        if let Some((start, end)) = Self::quoted_identifier_bounds_at(text, rel_pos) {
            let raw = text.get(start..end)?;
            let word = Self::strip_identifier_quotes(raw);
            if !word.is_empty() {
                return Some((word, start, end));
            }
        }

        if Self::has_unbalanced_identifier_quotes(text.get(..rel_pos).unwrap_or(text)) {
            return None;
        }

        let anchor = if rel_pos < text.len() {
            let ch = text.get(rel_pos..)?.chars().next()?;
            if sql_text::is_identifier_char(ch) {
                Some(rel_pos)
            } else {
                None
            }
        } else {
            None
        }
        .or_else(|| {
            if rel_pos == 0 {
                None
            } else {
                text.get(..rel_pos)
                    .and_then(|prefix| prefix.char_indices().next_back())
                    .and_then(|(prev_start, ch)| {
                        if sql_text::is_identifier_char(ch) {
                            Some(prev_start)
                        } else {
                            None
                        }
                    })
            }
        })?;

        let mut start = anchor;
        while start > 0 {
            let Some((prev_start, ch)) = text
                .get(..start)
                .and_then(|prefix| prefix.char_indices().next_back())
            else {
                break;
            };
            if sql_text::is_identifier_char(ch) {
                start = prev_start;
            } else {
                break;
            }
        }

        let mut end = anchor;
        while end < text.len() {
            let Some(ch) = text.get(end..).and_then(|suffix| suffix.chars().next()) else {
                break;
            };
            if sql_text::is_identifier_char(ch) {
                end += ch.len_utf8();
            } else {
                break;
            }
        }

        let word = text.get(start..end)?.to_string();
        if word.is_empty() {
            None
        } else {
            Some((word, start, end))
        }
    }

    fn identifier_at_position(
        buffer: &TextBuffer,
        text_shadow: &Arc<Mutex<HighlightShadowState>>,
        pos: i32,
    ) -> Option<(String, i32, i32)> {
        Self::identifier_at_position_with_raw(buffer, text_shadow, pos)
            .map(|(word, _, start, end)| (word, start, end))
    }

    fn identifier_at_position_with_raw(
        buffer: &TextBuffer,
        text_shadow: &Arc<Mutex<HighlightShadowState>>,
        pos: i32,
    ) -> Option<(String, String, i32, i32)> {
        let buffer_len = buffer.length().max(0);
        if buffer_len == 0 {
            return None;
        }
        let pos = pos.clamp(0, buffer_len);
        let line_start = text_buffer_access::line_start(buffer, Some(text_shadow), pos).max(0);
        let line_end = text_buffer_access::line_end(buffer, Some(text_shadow), pos).max(line_start);
        let text = text_buffer_access::text_range(buffer, Some(text_shadow), line_start, line_end);
        if text.is_empty() {
            return None;
        }

        let rel_pos = (pos - line_start).max(0) as usize;
        let (word, start, end) = Self::identifier_at_position_in_text(&text, rel_pos)?;
        let raw_word = text.get(start..end)?.to_string();
        Some((
            word,
            raw_word,
            line_start + start as i32,
            line_start + end as i32,
        ))
    }

    fn object_context_reference_at_position_in_text(
        text: &str,
        rel_pos: usize,
    ) -> Option<(String, usize, usize)> {
        let (_, start, end) = Self::identifier_at_position_in_text(text, rel_pos)?;
        if Self::identifier_is_followed_by_member_access_dot(text, end) {
            return None;
        }
        let local_aliases = super::query_text::collect_local_alias_context(text);
        if local_aliases.is_declaration_range(start, end) {
            return None;
        }
        let raw_word = text.get(start..end)?;
        let reference = Self::raw_qualifier_before_word_in_text(text, start)
            .map(|qualifier| format!("{}.{}", qualifier, raw_word))
            .unwrap_or_else(|| raw_word.to_string());
        Some((reference, start, end))
    }

    fn identifier_is_followed_by_member_access_dot(text: &str, end: usize) -> bool {
        text.get(end..)
            .map(str::trim_start)
            .is_some_and(|suffix| suffix.starts_with('.'))
    }

    fn object_context_reference_at_position(
        buffer: &TextBuffer,
        text_shadow: &Arc<Mutex<HighlightShadowState>>,
        pos: i32,
    ) -> Option<(String, i32, i32)> {
        let buffer_len = buffer.length().max(0);
        if buffer_len == 0 {
            return None;
        }
        let pos = pos.clamp(0, buffer_len);
        let line_start = text_buffer_access::line_start(buffer, Some(text_shadow), pos).max(0);
        let line_end = text_buffer_access::line_end(buffer, Some(text_shadow), pos).max(line_start);
        let text = text_buffer_access::text_range(buffer, Some(text_shadow), line_start, line_end);
        if text.is_empty() {
            return None;
        }

        let rel_pos = (pos - line_start).max(0) as usize;
        let (reference, start, end) =
            Self::object_context_reference_at_position_in_text(&text, rel_pos)?;
        Some((reference, line_start + start as i32, line_start + end as i32))
    }

    fn quick_describe_type_priority(object_type: &str) -> i32 {
        match object_type.to_uppercase().as_str() {
            "TABLE" => 0,
            "VIEW" => 1,
            "FUNCTION" => 2,
            "PROCEDURE" => 3,
            "SEQUENCE" => 4,
            "PACKAGE" => 5,
            "PACKAGE BODY" => 6,
            _ => 50,
        }
    }

    fn format_argument_type_for_quick_describe(arg: &ProcedureArgument) -> String {
        if let Some(pls_type) = arg.pls_type.as_deref() {
            let trimmed = pls_type.trim();
            if !trimmed.is_empty() {
                return trimmed.to_string();
            }
        }

        if let Some(data_type) = arg.data_type.as_deref() {
            let upper = data_type.trim().to_uppercase();
            if upper == "NUMBER" {
                if let (Some(p), Some(s)) = (arg.data_precision, arg.data_scale) {
                    return format!("NUMBER({},{})", p, s);
                }
                if let Some(p) = arg.data_precision {
                    return format!("NUMBER({})", p);
                }
                return "NUMBER".to_string();
            }

            if matches!(
                upper.as_str(),
                "VARCHAR2" | "NVARCHAR2" | "VARCHAR" | "CHAR" | "NCHAR" | "RAW"
            ) {
                if let Some(len) = arg.data_length {
                    return format!("{}({})", upper, len.max(1));
                }
                return upper;
            }

            return upper;
        }

        if let Some(type_name) = arg.type_name.as_deref() {
            if let Some(owner) = arg.type_owner.as_deref() {
                return format!("{}.{}", owner, type_name);
            }
            return type_name.to_string();
        }

        "UNKNOWN".to_string()
    }

    /// Build a one-line signature (`NAME(p1 IN TYPE, p2 OUT TYPE) RETURN TYPE`)
    /// for the parameter-hint popup, recording the span of each positional
    /// argument. Uses the first overload, mirroring the quick-describe view.
    fn build_signature_label(
        name: &str,
        arguments: &[ProcedureArgument],
    ) -> crate::ui::intellisense::SignatureLabel {
        let selected_overload = arguments.first().and_then(|arg| arg.overload);
        let selected: Vec<&ProcedureArgument> = arguments
            .iter()
            .filter(|arg| arg.overload == selected_overload)
            .collect();

        let mut text = name.to_uppercase();
        text.push('(');
        let mut arg_spans: Vec<(usize, usize)> = Vec::new();
        let mut return_type: Option<String> = None;
        let mut first = true;

        for arg in &selected {
            let is_return = arg.position == 0 && arg.name.is_none();
            let type_display = Self::format_argument_type_for_quick_describe(arg);
            if is_return {
                return_type = Some(type_display);
                continue;
            }
            if !first {
                text.push_str(", ");
            }
            first = false;
            let arg_name = arg
                .name
                .clone()
                .unwrap_or_else(|| format!("ARG{}", arg.position.max(1)));
            let direction = arg.in_out.clone().unwrap_or_else(|| "IN".to_string());
            let start = text.len();
            text.push_str(&format!("{} {} {}", arg_name, direction.trim(), type_display));
            arg_spans.push((start, text.len()));
        }

        text.push(')');
        if let Some(return_type) = return_type {
            text.push_str(&format!(" RETURN {}", return_type));
        }

        crate::ui::intellisense::SignatureLabel { text, arg_spans }
    }

    fn format_routine_details(
        qualified_name: &str,
        routine_type: &str,
        arguments: &[ProcedureArgument],
    ) -> String {
        let mut details = format!(
            "=== {} {} ===\n\n",
            routine_type.to_uppercase(),
            qualified_name.to_uppercase()
        );

        if arguments.is_empty() {
            details.push_str("No argument metadata found.\n");
            return details;
        }

        let selected_overload = arguments.first().and_then(|arg| arg.overload);
        let selected: Vec<&ProcedureArgument> = arguments
            .iter()
            .filter(|arg| arg.overload == selected_overload)
            .collect();

        if let Some(overload) = selected_overload {
            details.push_str(&format!("Overload: {}\n\n", overload));
        }

        details.push_str(&format!(
            "{:<24} {:<12} {}\n",
            "Argument", "Direction", "Type"
        ));
        details.push_str(&format!("{}\n", "-".repeat(72)));

        let mut return_type: Option<String> = None;
        for arg in selected {
            let is_return = arg.position == 0 && arg.name.is_none();
            let type_display = Self::format_argument_type_for_quick_describe(arg);
            if is_return {
                return_type = Some(type_display);
                continue;
            }
            let arg_name = arg
                .name
                .clone()
                .unwrap_or_else(|| format!("ARG{}", arg.position.max(1)));
            let direction = arg.in_out.clone().unwrap_or_else(|| "IN".to_string());
            details.push_str(&format!(
                "{:<24} {:<12} {}\n",
                arg_name, direction, type_display
            ));
        }

        if let Some(return_type) = return_type {
            details.push_str(&format!("\nReturn Type: {}\n", return_type));
        }

        details
    }

    fn format_sequence_details(info: &SequenceInfo) -> String {
        let mut details = format!("=== Sequence Info: {} ===\n\n", info.name.to_uppercase());
        details.push_str(&format!("{:<18} {}\n", "Min Value", info.min_value));
        details.push_str(&format!("{:<18} {}\n", "Max Value", info.max_value));
        details.push_str(&format!("{:<18} {}\n", "Increment By", info.increment_by));
        details.push_str(&format!("{:<18} {}\n", "Cycle", info.cycle_flag));
        details.push_str(&format!("{:<18} {}\n", "Order", info.order_flag));
        details.push_str(&format!("{:<18} {}\n", "Cache Size", info.cache_size));
        details.push_str(&format!("{:<18} {}\n", "Last Number", info.last_number));
        details.push_str("\nNote: LAST_NUMBER is the next value to be generated.\n");
        details
    }

    fn split_quick_describe_lookup_parts(value: &str) -> Option<Vec<&str>> {
        let trimmed = value.trim();
        if trimmed.is_empty() {
            return Some(Vec::new());
        }

        let mut parts = Vec::new();
        let mut start = 0usize;
        let mut active_quote: Option<char> = None;
        let mut chars = trimmed.char_indices().peekable();

        while let Some((idx, ch)) = chars.next() {
            if let Some(quote) = active_quote {
                if ch == quote {
                    if chars.peek().is_some_and(|(_, next)| *next == quote) {
                        chars.next();
                    } else {
                        active_quote = None;
                    }
                }
                continue;
            }

            if matches!(ch, '"' | '`') {
                active_quote = Some(ch);
            } else if ch == '.' {
                parts.push(trimmed[start..idx].trim());
                start = idx + ch.len_utf8();
            }
        }

        if active_quote.is_some() {
            return None;
        }

        parts.push(trimmed[start..].trim());
        Some(parts)
    }

    fn quote_quick_describe_lookup_part(value: &str) -> String {
        format!("\"{}\"", value.replace('"', "\"\""))
    }

    fn normalize_quick_describe_lookup_part(value: &str) -> Option<String> {
        let trimmed = value.trim();
        if trimmed.is_empty() {
            None
        } else if sql_text::is_quoted_identifier(trimmed) {
            Some(Self::quote_quick_describe_lookup_part(
                &Self::strip_identifier_quotes(trimmed),
            ))
        } else if trimmed.starts_with('"')
            || trimmed.ends_with('"')
            || trimmed.contains('"')
            || trimmed.starts_with('`')
            || trimmed.ends_with('`')
            || trimmed.contains('`')
        {
            None
        } else {
            Some(trimmed.to_uppercase())
        }
    }

    fn normalize_quick_describe_lookup_identifier_parts(value: &str) -> Option<(String, usize)> {
        let parts = Self::split_quick_describe_lookup_parts(value)?;
        if parts.is_empty() {
            return None;
        }

        let mut normalized = Vec::with_capacity(parts.len());
        for part in parts {
            normalized.push(Self::normalize_quick_describe_lookup_part(part)?);
        }
        let part_count = normalized.len();
        Some((normalized.join("."), part_count))
    }

    fn normalize_quick_describe_lookup_identifier(value: &str) -> Option<String> {
        Self::normalize_quick_describe_lookup_identifier_parts(value)
            .map(|(identifier, _)| identifier)
    }

    fn is_canonical_quick_describe_lookup_part(value: &str) -> bool {
        let trimmed = value.trim();
        !trimmed.is_empty()
            && trimmed == trimmed.to_uppercase()
            && trimmed.chars().all(|ch| {
                ch.is_ascii_uppercase() || ch.is_ascii_digit() || matches!(ch, '_' | '$' | '#')
            })
    }

    fn normalize_quick_describe_current_schema_part(value: &str) -> Option<String> {
        let trimmed = value.trim();
        if trimmed.is_empty() {
            None
        } else if sql_text::is_quoted_identifier(trimmed) {
            Some(Self::quote_quick_describe_lookup_part(
                &Self::strip_identifier_quotes(trimmed),
            ))
        } else if trimmed.starts_with('"')
            || trimmed.ends_with('"')
            || trimmed.contains('"')
            || trimmed.starts_with('`')
            || trimmed.ends_with('`')
            || trimmed.contains('`')
        {
            None
        } else if Self::is_canonical_quick_describe_lookup_part(trimmed) {
            Some(trimmed.to_string())
        } else {
            Some(Self::quote_quick_describe_lookup_part(trimmed))
        }
    }

    fn quick_describe_current_schema(tracked_schema: Option<&str>) -> Option<String> {
        tracked_schema
            .map(str::trim)
            .filter(|schema| !schema.is_empty())
            .and_then(Self::normalize_quick_describe_current_schema_part)
    }

    fn quick_describe_lookup_name(
        object_name: &str,
        qualifier: Option<&str>,
        current_schema: Option<&str>,
    ) -> String {
        let Some(object_name) = Self::normalize_quick_describe_lookup_identifier(object_name) else {
            return String::new();
        };
        if let Some(qualifier) = qualifier {
            if let Some(qualifier) = Self::normalize_quick_describe_lookup_identifier(qualifier) {
                return format!("{}.{}", qualifier, object_name);
            }
            return String::new();
        }

        current_schema
            .and_then(Self::normalize_quick_describe_current_schema_part)
            .map(|schema| format!("{}.{}", schema, object_name))
            .unwrap_or(object_name)
    }

    fn quick_describe_package_lookup_names(
        qualifier: Option<&str>,
        current_schema: Option<&str>,
    ) -> Vec<String> {
        let Some((package_name, part_count)) =
            qualifier.and_then(Self::normalize_quick_describe_lookup_identifier_parts)
        else {
            return Vec::new();
        };

        if part_count > 1 {
            return vec![package_name];
        }

        if let Some(schema) =
            current_schema.and_then(Self::normalize_quick_describe_current_schema_part)
        {
            return vec![format!("{}.{}", schema, package_name)];
        }

        vec![package_name]
    }

    fn describe_object(
        conn: &Connection,
        object_name: &str,
        qualifier: Option<&str>,
        tracked_schema: Option<&str>,
    ) -> Result<QuickDescribeData, String> {
        let current_schema = Self::quick_describe_current_schema(tracked_schema);
        let lookup_name =
            Self::quick_describe_lookup_name(object_name, qualifier, current_schema.as_deref());
        if lookup_name.is_empty() {
            return Err("Object not found or not accessible".to_string());
        }

        if let Ok(columns) = ObjectBrowser::get_table_structure(conn, &lookup_name) {
            if !columns.is_empty() {
                return Ok(QuickDescribeData::TableColumns(columns));
            }
        }

        let object_types_result =
            ObjectBrowser::get_object_types(conn, &lookup_name).map_err(|err| err.to_string());
        if let Ok(mut object_types) = object_types_result.as_ref().cloned() {
            if !object_types.is_empty() {
                object_types
                    .sort_by_key(|object_type| Self::quick_describe_type_priority(object_type));

                for object_type in object_types {
                    let object_type_upper = object_type.to_uppercase();
                    match object_type_upper.as_str() {
                        "TABLE" | "VIEW" => {
                            if let Ok(columns) =
                                ObjectBrowser::get_table_structure(conn, &lookup_name)
                            {
                                if !columns.is_empty() {
                                    return Ok(QuickDescribeData::TableColumns(columns));
                                }
                            }
                        }
                        "FUNCTION" | "PROCEDURE" => {
                            let args = ObjectBrowser::get_procedure_arguments(conn, &lookup_name)
                                .unwrap_or_default();
                            let content = Self::format_routine_details(
                                &lookup_name,
                                &object_type_upper,
                                &args,
                            );
                            return Ok(QuickDescribeData::Text {
                                title: format!("Describe: {} ({})", lookup_name, object_type_upper),
                                content,
                            });
                        }
                        "SEQUENCE" => {
                            if let Ok(info) = ObjectBrowser::get_sequence_info(conn, &lookup_name) {
                                return Ok(QuickDescribeData::Text {
                                    title: format!("Describe: {} (SEQUENCE)", lookup_name),
                                    content: Self::format_sequence_details(&info),
                                });
                            }
                        }
                        "PACKAGE" => {
                            if let Ok(ddl) = ObjectBrowser::get_package_spec_ddl(conn, &lookup_name)
                            {
                                return Ok(QuickDescribeData::Text {
                                    title: format!("Describe: {} (PACKAGE)", lookup_name),
                                    content: ddl,
                                });
                            }
                        }
                        _ => {
                            if let Ok(ddl) = ObjectBrowser::get_object_ddl(
                                conn,
                                &object_type_upper,
                                &lookup_name,
                            ) {
                                return Ok(QuickDescribeData::Text {
                                    title: format!(
                                        "Describe: {} ({})",
                                        lookup_name, object_type_upper
                                    ),
                                    content: ddl,
                                });
                            }
                        }
                    }
                }
            }
        }

        let routine_lookup_name =
            Self::normalize_quick_describe_lookup_identifier(object_name).unwrap_or_default();
        let routine_match_name = Self::strip_identifier_quotes(&routine_lookup_name);
        let routine_match_exact = routine_lookup_name.trim_start().starts_with('"');
        for package_name_upper in
            Self::quick_describe_package_lookup_names(qualifier, current_schema.as_deref())
        {
            if let Ok(routines) = ObjectBrowser::get_package_routines(conn, &package_name_upper) {
                if let Some(routine) = routines.iter().find(|routine| {
                    if routine_match_exact {
                        routine.name == routine_match_name
                    } else {
                        routine.name.eq_ignore_ascii_case(&routine_match_name)
                    }
                }) {
                    let args = ObjectBrowser::get_package_procedure_arguments(
                        conn,
                        &package_name_upper,
                        &routine_lookup_name,
                    )
                    .map_err(|err| err.to_string())?;
                    let qualified_name = format!("{}.{}", package_name_upper, routine_lookup_name);
                    let content =
                        Self::format_routine_details(&qualified_name, &routine.routine_type, &args);
                    return Ok(QuickDescribeData::Text {
                        title: format!(
                            "Describe: {} ({})",
                            qualified_name,
                            routine.routine_type.to_uppercase()
                        ),
                        content,
                    });
                }
            }
        }

        object_types_result?;

        Err(format!(
            "Object not found or not accessible: {}",
            lookup_name
        ))
    }

    fn describe_thin_object(
        conn: &mut tns_thin::OracleThinSession,
        object_name: &str,
        qualifier: Option<&str>,
        tracked_schema: Option<&str>,
    ) -> Result<QuickDescribeData, String> {
        let current_schema = Self::quick_describe_current_schema(tracked_schema);
        let lookup_name =
            Self::quick_describe_lookup_name(object_name, qualifier, current_schema.as_deref());
        if lookup_name.is_empty() {
            return Err("Object not found or not accessible".to_string());
        }

        if let Ok(columns) = ObjectBrowser::get_thin_table_structure(conn, &lookup_name) {
            if !columns.is_empty() {
                return Ok(QuickDescribeData::TableColumns(columns));
            }
        }

        let object_types_result = ObjectBrowser::get_thin_object_types(conn, &lookup_name);
        if let Ok(mut object_types) = object_types_result.as_ref().cloned() {
            if !object_types.is_empty() {
                object_types
                    .sort_by_key(|object_type| Self::quick_describe_type_priority(object_type));

                for object_type in object_types {
                    let object_type_upper = object_type.to_uppercase();
                    match object_type_upper.as_str() {
                        "TABLE" | "VIEW" => {
                            if let Ok(columns) =
                                ObjectBrowser::get_thin_table_structure(conn, &lookup_name)
                            {
                                if !columns.is_empty() {
                                    return Ok(QuickDescribeData::TableColumns(columns));
                                }
                            }
                        }
                        "FUNCTION" | "PROCEDURE" => {
                            let args =
                                ObjectBrowser::get_thin_procedure_arguments(conn, &lookup_name)
                                    .unwrap_or_default();
                            let content = Self::format_routine_details(
                                &lookup_name,
                                &object_type_upper,
                                &args,
                            );
                            return Ok(QuickDescribeData::Text {
                                title: format!("Describe: {} ({})", lookup_name, object_type_upper),
                                content,
                            });
                        }
                        "SEQUENCE" => {
                            if let Ok(info) =
                                ObjectBrowser::get_thin_sequence_info(conn, &lookup_name)
                            {
                                return Ok(QuickDescribeData::Text {
                                    title: format!("Describe: {} (SEQUENCE)", lookup_name),
                                    content: Self::format_sequence_details(&info),
                                });
                            }
                        }
                        "PACKAGE" => {
                            if let Ok(ddl) =
                                ObjectBrowser::get_thin_object_ddl(conn, "PACKAGE", &lookup_name)
                            {
                                return Ok(QuickDescribeData::Text {
                                    title: format!("Describe: {} (PACKAGE)", lookup_name),
                                    content: ddl,
                                });
                            }
                        }
                        _ => {
                            if let Ok(ddl) = ObjectBrowser::get_thin_object_ddl(
                                conn,
                                &object_type_upper,
                                &lookup_name,
                            ) {
                                return Ok(QuickDescribeData::Text {
                                    title: format!(
                                        "Describe: {} ({})",
                                        lookup_name, object_type_upper
                                    ),
                                    content: ddl,
                                });
                            }
                        }
                    }
                }
            }
        }

        let routine_lookup_name =
            Self::normalize_quick_describe_lookup_identifier(object_name).unwrap_or_default();
        let routine_match_name = Self::strip_identifier_quotes(&routine_lookup_name);
        let routine_match_exact = routine_lookup_name.trim_start().starts_with('"');
        for package_name_upper in
            Self::quick_describe_package_lookup_names(qualifier, current_schema.as_deref())
        {
            if let Ok(routines) =
                ObjectBrowser::get_thin_package_routines(conn, &package_name_upper)
            {
                if let Some(routine) = routines.iter().find(|routine| {
                    if routine_match_exact {
                        routine.name == routine_match_name
                    } else {
                        routine.name.eq_ignore_ascii_case(&routine_match_name)
                    }
                }) {
                    let args = ObjectBrowser::get_thin_package_procedure_arguments(
                        conn,
                        &package_name_upper,
                        &routine_lookup_name,
                    )?;
                    let qualified_name = format!("{}.{}", package_name_upper, routine_lookup_name);
                    let content =
                        Self::format_routine_details(&qualified_name, &routine.routine_type, &args);
                    return Ok(QuickDescribeData::Text {
                        title: format!(
                            "Describe: {} ({})",
                            qualified_name,
                            routine.routine_type.to_uppercase()
                        ),
                        content,
                    });
                }
            }
        }

        object_types_result?;

        Err(format!(
            "Object not found or not accessible: {}",
            lookup_name
        ))
    }

    fn context_before_cursor(
        buffer: &TextBuffer,
        text_shadow: &Arc<Mutex<HighlightShadowState>>,
        cursor_pos: i32,
        preferred_db_type: Option<crate::db::connection::DatabaseType>,
    ) -> String {
        let buffer_len = buffer.length().max(0);
        let cursor_pos = cursor_pos.clamp(0, buffer_len);
        let start = (cursor_pos - INTELLISENSE_CONTEXT_WINDOW).max(0);
        let (window, window_start) =
            Self::bounded_text_window(buffer, text_shadow, start, cursor_pos);
        if window.is_empty() {
            return String::new();
        }

        let mut rel_cursor = (cursor_pos - window_start).max(0) as usize;
        if rel_cursor > window.len() {
            rel_cursor = window.len();
        }
        let rel_cursor = Self::clamp_to_char_boundary_local(&window, rel_cursor);
        let before_cursor = window.get(..rel_cursor).unwrap_or("");
        if let Some((stmt_start, _)) = super::query_text::simple_single_statement_bounds(before_cursor)
        {
            return before_cursor.get(stmt_start..).unwrap_or("").to_string();
        }
        let (stmt_start, _) = Self::statement_bounds_in_text_for_db_type(
            before_cursor,
            before_cursor.len(),
            preferred_db_type,
        );
        before_cursor.get(stmt_start..).unwrap_or("").to_string()
    }

    fn clamp_to_char_boundary_local(text: &str, idx: usize) -> usize {
        let mut idx = idx.min(text.len());
        if text.is_char_boundary(idx) {
            return idx;
        }

        // Clamp invalid UTF-8 byte offsets to the previous valid boundary.
        while idx > 0 && !text.is_char_boundary(idx) {
            idx -= 1;
        }
        idx
    }

    fn raw_cursor_position(buffer: &TextBuffer, pos: i32) -> i32 {
        let buffer_len = buffer.length().max(0);
        pos.clamp(0, buffer_len)
    }

    fn raw_cursor_byte_offset(pos: i32, buffer_len: i32) -> usize {
        pos.clamp(0, buffer_len.max(0)) as usize
    }

    pub(super) fn cursor_position(buffer: &TextBuffer, pos: i32) -> (i32, usize) {
        let buffer_len = buffer.length().max(0);
        let cursor_pos = Self::raw_cursor_position(buffer, pos);
        let cursor_byte = Self::raw_cursor_byte_offset(cursor_pos, buffer_len);
        (cursor_pos, cursor_byte)
    }

    pub(super) fn editor_cursor_position(editor: &TextEditor, buffer: &TextBuffer) -> (i32, usize) {
        Self::cursor_position(buffer, editor.insert_position())
    }

    #[cfg(test)]
    fn statement_context_in_text(text: &str, cursor_pos: usize) -> String {
        Self::statement_context_in_text_for_db_type(text, cursor_pos, None)
    }

    #[cfg(test)]
    fn statement_context_in_text_for_db_type(
        text: &str,
        cursor_pos: usize,
        preferred_db_type: Option<crate::db::connection::DatabaseType>,
    ) -> String {
        if text.is_empty() {
            return String::new();
        }
        let cursor_pos = cursor_pos.min(text.len());
        let start_candidate = cursor_pos.saturating_sub(INTELLISENSE_STATEMENT_WINDOW as usize);
        let end_candidate = cursor_pos
            .saturating_add(INTELLISENSE_STATEMENT_WINDOW as usize)
            .min(text.len());
        let bytes = text.as_bytes();
        let start = bytes[..start_candidate]
            .iter()
            .rposition(|&b| b == b'\n')
            .map(|idx| idx + 1)
            .unwrap_or(0);
        let end = bytes[end_candidate..]
            .iter()
            .position(|&b| b == b'\n')
            .map(|idx| end_candidate + idx)
            .unwrap_or(text.len());
        let window = text.get(start..end).unwrap_or("");
        if let Some((stmt_start, stmt_end)) = super::query_text::simple_single_statement_bounds(window)
        {
            return window.get(stmt_start..stmt_end).unwrap_or("").to_string();
        }
        let rel_cursor = cursor_pos.saturating_sub(start).min(window.len());
        let (stmt_start, stmt_end) =
            Self::statement_bounds_in_text_for_db_type(window, rel_cursor, preferred_db_type);
        window.get(stmt_start..stmt_end).unwrap_or("").to_string()
    }

    #[cfg(test)]
    fn context_before_cursor_in_text(text: &str, cursor_pos: usize) -> String {
        Self::context_before_cursor_in_text_for_db_type(text, cursor_pos, None)
    }

    #[cfg(test)]
    fn context_before_cursor_in_text_for_db_type(
        text: &str,
        cursor_pos: usize,
        preferred_db_type: Option<crate::db::connection::DatabaseType>,
    ) -> String {
        let cursor_pos = Self::clamp_to_char_boundary_local(text, cursor_pos.min(text.len()));
        let start = cursor_pos.saturating_sub(INTELLISENSE_CONTEXT_WINDOW as usize);
        let start = Self::clamp_to_char_boundary_local(text, start);
        let window = text.get(start..cursor_pos).unwrap_or("");
        let (stmt_start, _) =
            Self::statement_bounds_in_text_for_db_type(window, window.len(), preferred_db_type);
        window.get(stmt_start..).unwrap_or("").to_string()
    }

    fn should_skip_leading_intellisense_context_line(line: &str) -> bool {
        let trimmed = line.trim();
        trimmed.is_empty() || trimmed.starts_with("--") || Self::is_sqlplus_command_line(trimmed)
    }

    fn normalize_intellisense_context(
        text: &str,
        cursor_byte: usize,
    ) -> NormalizedIntellisenseContext {
        let cursor_byte = Self::clamp_to_char_boundary_local(text, cursor_byte.min(text.len()));
        let before_cursor = text.get(..cursor_byte).unwrap_or("");
        let stripped_cursor = Self::strip_sqlplus_prompt_prefixes(before_cursor).len();
        let text = Self::strip_sqlplus_prompt_prefixes(text);
        let cursor_byte =
            Self::clamp_to_char_boundary_local(&text, stripped_cursor.min(text.len()));
        let mut normalized = String::with_capacity(text.len());
        let mut raw_offset = 0usize;
        let mut normalized_cursor = 0usize;
        let mut cursor_recorded = false;
        let mut skipping_prefix = true;

        for segment in text.split_inclusive('\n') {
            let segment_start = raw_offset;
            raw_offset += segment.len();

            let (line, line_end) = if let Some(stripped) = segment.strip_suffix('\n') {
                (stripped, "\n")
            } else {
                (segment, "")
            };

            if skipping_prefix && Self::should_skip_leading_intellisense_context_line(line) {
                if !cursor_recorded && cursor_byte <= raw_offset {
                    normalized_cursor = normalized.len();
                    cursor_recorded = true;
                }
                continue;
            }
            skipping_prefix = false;

            if !cursor_recorded && cursor_byte <= raw_offset {
                let cursor_in_segment = cursor_byte.saturating_sub(segment_start).min(segment.len());
                let cursor_in_line = cursor_in_segment.min(line.len());
                normalized_cursor = normalized.len() + cursor_in_line;
                cursor_recorded = true;
            }

            normalized.push_str(line);
            normalized.push_str(line_end);
        }

        if !cursor_recorded {
            normalized_cursor = normalized.len();
        }

        let normalized_cursor = Self::clamp_to_char_boundary_local(
            &normalized,
            normalized_cursor.min(normalized.len()),
        );
        NormalizedIntellisenseContext {
            text: normalized,
            cursor_byte: normalized_cursor,
        }
    }

    fn normalize_intellisense_context_text(text: &str) -> String {
        Self::normalize_intellisense_context(text, text.len()).text
    }

    #[cfg(test)]
    fn normalize_intellisense_context_with_cursor(
        text: &str,
        cursor_byte: usize,
    ) -> (String, usize) {
        let normalized = Self::normalize_intellisense_context(text, cursor_byte);
        (normalized.text, normalized.cursor_byte)
    }

    fn strip_sqlplus_prompt_prefixes(text: &str) -> String {
        let mut normalized = String::with_capacity(text.len());
        let mut saw_sql_prompt = false;

        for segment in text.split_inclusive('\n') {
            let (line, line_end) = if let Some(stripped) = segment.strip_suffix('\n') {
                (stripped, "\n")
            } else {
                (segment, "")
            };

            let stripped_line = if let Some(stripped) = Self::strip_sqlplus_sql_prompt_prefix(line)
            {
                saw_sql_prompt = true;
                stripped
            } else if saw_sql_prompt {
                Self::strip_sqlplus_numbered_prompt_prefix(line).unwrap_or(line)
            } else {
                line
            };
            normalized.push_str(stripped_line);
            normalized.push_str(line_end);
        }

        normalized
    }

    fn strip_sqlplus_sql_prompt_prefix(line: &str) -> Option<&str> {
        let bytes = line.as_bytes();
        let mut idx = 0usize;

        while idx < bytes.len() && bytes[idx].is_ascii_whitespace() {
            idx += 1;
        }

        if bytes.get(idx..idx + 4).is_some_and(|slice| {
            slice[0].eq_ignore_ascii_case(&b'S')
                && slice[1].eq_ignore_ascii_case(&b'Q')
                && slice[2].eq_ignore_ascii_case(&b'L')
                && slice[3] == b'>'
        }) {
            idx += 4;
            while idx < bytes.len() && bytes[idx].is_ascii_whitespace() {
                idx += 1;
            }
            return Some(&line[idx..]);
        }

        None
    }

    fn strip_sqlplus_numbered_prompt_prefix(line: &str) -> Option<&str> {
        let bytes = line.as_bytes();
        let mut idx = 0usize;

        while idx < bytes.len() && bytes[idx].is_ascii_whitespace() {
            idx += 1;
        }

        let number_start = idx;
        while idx < bytes.len() && bytes[idx].is_ascii_digit() {
            idx += 1;
        }
        if idx > number_start {
            let mut sep = idx;
            while sep < bytes.len() && bytes[sep].is_ascii_whitespace() {
                sep += 1;
            }
            let whitespace_count = sep.saturating_sub(idx);
            if whitespace_count >= 2 {
                return Some(&line[sep..]);
            }
        }

        None
    }

    fn is_sqlplus_command_line(trimmed_line: &str) -> bool {
        crate::ui::sql_editor::query_text::is_sqlplus_command_line(trimmed_line)
    }

    // 문장 경계 계산은 실행/포맷 공통 규칙을 공유하기 위해 `query_text` 유틸을 사용합니다.
    #[cfg(test)]
    fn statement_bounds_in_text(text: &str, cursor_pos: usize) -> (usize, usize) {
        Self::statement_bounds_in_text_for_db_type(text, cursor_pos, None)
    }

    fn statement_bounds_in_text_for_db_type(
        text: &str,
        cursor_pos: usize,
        preferred_db_type: Option<crate::db::connection::DatabaseType>,
    ) -> (usize, usize) {
        crate::ui::sql_editor::query_text::statement_bounds_in_text_for_db_type(
            text,
            cursor_pos,
            preferred_db_type,
        )
    }

    fn strip_identifier_quotes(value: &str) -> String {
        sql_text::strip_identifier_quotes(value)
    }

    fn qualifier_before_word(
        buffer: &TextBuffer,
        text_shadow: &Arc<Mutex<HighlightShadowState>>,
        word_start: usize,
    ) -> Option<String> {
        if word_start == 0 {
            return None;
        }
        let buffer_len = buffer.length().max(0) as usize;
        if word_start > buffer_len {
            return None;
        }
        let start = word_start
            .saturating_sub(INTELLISENSE_QUALIFIER_WINDOW as usize)
            .min(word_start);
        let (text, start) = Self::bounded_text_window(
            buffer,
            text_shadow,
            start as i32,
            (word_start as i32).max(0),
        );
        let mut rel_word_start = (word_start as i32 - start).max(0) as usize;
        if rel_word_start > text.len() {
            rel_word_start = text.len();
        }
        rel_word_start = Self::clamp_to_char_boundary_local(&text, rel_word_start);
        Self::qualifier_before_word_in_text(&text, rel_word_start)
    }

    fn raw_qualifier_before_word(
        buffer: &TextBuffer,
        text_shadow: &Arc<Mutex<HighlightShadowState>>,
        word_start: usize,
    ) -> Option<String> {
        if word_start == 0 {
            return None;
        }
        let buffer_len = buffer.length().max(0) as usize;
        if word_start > buffer_len {
            return None;
        }
        let start = word_start
            .saturating_sub(INTELLISENSE_QUALIFIER_WINDOW as usize)
            .min(word_start);
        let (text, start) = Self::bounded_text_window(
            buffer,
            text_shadow,
            start as i32,
            (word_start as i32).max(0),
        );
        let mut rel_word_start = (word_start as i32 - start).max(0) as usize;
        if rel_word_start > text.len() {
            rel_word_start = text.len();
        }
        rel_word_start = Self::clamp_to_char_boundary_local(&text, rel_word_start);
        Self::raw_qualifier_before_word_in_text(&text, rel_word_start)
    }

    fn qualifier_before_word_in_text(text: &str, rel_word_start: usize) -> Option<String> {
        if rel_word_start == 0 {
            return None;
        }
        let bytes = text.as_bytes();

        // IntelliSense qualifier must be strict `qualifier.<cursor>` form.
        // Do not allow whitespace around `.` so cases like `e .|` / `e. |`
        // are treated as non-qualified context.
        if bytes.get(rel_word_start.saturating_sub(1)) != Some(&b'.') {
            return None;
        }
        let idx = rel_word_start - 1;

        let qualifier_candidate = text.get(..idx)?;
        if Self::has_unbalanced_identifier_quotes(qualifier_candidate) {
            return None;
        }

        let mut segments = Vec::new();
        let mut segment_end = idx;

        loop {
            let (segment, segment_start) =
                Self::parse_qualifier_segment_before_dot(text, segment_end)?;
            if segment.is_empty() {
                return None;
            }
            segments.push(segment);

            if segment_start == 0 {
                break;
            }
            if bytes.get(segment_start - 1) != Some(&b'.') {
                break;
            }
            segment_end = segment_start - 1;
            if segment_end == 0 {
                return None;
            }
        }

        if segments.is_empty() {
            return None;
        }

        segments.reverse();
        Some(segments.join("."))
    }

    fn raw_qualifier_before_word_in_text(text: &str, rel_word_start: usize) -> Option<String> {
        if rel_word_start == 0 {
            return None;
        }
        let bytes = text.as_bytes();
        if bytes.get(rel_word_start.saturating_sub(1)) != Some(&b'.') {
            return None;
        }
        let idx = rel_word_start - 1;

        let qualifier_candidate = text.get(..idx)?;
        if Self::has_unbalanced_identifier_quotes(qualifier_candidate) {
            return None;
        }

        let mut segment_end = idx;

        let first_segment_start = loop {
            let (_, segment_start) = Self::parse_qualifier_segment_before_dot(text, segment_end)?;

            if segment_start == 0 {
                break segment_start;
            }
            if bytes.get(segment_start - 1) != Some(&b'.') {
                break segment_start;
            }
            segment_end = segment_start - 1;
            if segment_end == 0 {
                return None;
            }
        };

        text.get(first_segment_start..idx)
            .filter(|qualifier| !qualifier.is_empty())
            .map(ToString::to_string)
    }

    fn parse_qualifier_segment_before_dot(
        text: &str,
        segment_end: usize,
    ) -> Option<(String, usize)> {
        if segment_end == 0 {
            return None;
        }

        let last_char = text.get(..segment_end)?.chars().next_back();
        if matches!(last_char, Some(')')) {
            let open_idx = Self::find_open_paren_for_qualifier_expression(text, segment_end)?;
            return Self::parse_qualifier_segment_before_dot(text, open_idx);
        }
        if let Some(delimiter) = last_char.filter(|ch| matches!(ch, '"' | '`')) {
            let start = Self::find_quoted_segment_start(text, segment_end, delimiter)?;
            let quoted = text.get(start..segment_end)?;
            let qualifier = Self::strip_identifier_quotes(quoted);
            if qualifier.is_empty() {
                return None;
            }
            return Some((qualifier, start));
        }

        let mut start = segment_end;
        for (pos, ch) in text.get(..segment_end)?.char_indices().rev() {
            if sql_text::is_identifier_char(ch) {
                start = pos;
            } else {
                break;
            }
        }
        if start == segment_end {
            return None;
        }

        let segment = text.get(start..segment_end)?;
        let starts_with_valid_ident_char = segment
            .chars()
            .next()
            .is_some_and(sql_text::is_identifier_start_char);
        if !starts_with_valid_ident_char {
            return None;
        }

        Some((segment.to_string(), start))
    }

    fn find_open_paren_for_qualifier_expression(text: &str, segment_end: usize) -> Option<usize> {
        let mut depth = 0usize;
        for (pos, ch) in text.get(..segment_end)?.char_indices().rev() {
            match ch {
                ')' => depth = depth.saturating_add(1),
                '(' => {
                    depth = depth.saturating_sub(1);
                    if depth == 0 {
                        return Some(pos);
                    }
                }
                _ => {}
            }
        }

        None
    }

    fn has_unbalanced_identifier_quotes(text: &str) -> bool {
        let mut chars = text.chars().peekable();
        let mut active_quote: Option<char> = None;
        while let Some(ch) = chars.next() {
            if !matches!(ch, '"' | '`') {
                continue;
            }

            if active_quote == Some(ch) {
                if chars.peek().copied() == Some(ch) {
                    chars.next();
                } else {
                    active_quote = None;
                }
            } else if active_quote.is_none() {
                active_quote = Some(ch);
            }
        }
        active_quote.is_some()
    }

    fn try_fast_path_intellisense_filter(
        editor: &TextEditor,
        buffer: &TextBuffer,
        text_shadow: &Arc<Mutex<HighlightShadowState>>,
        intellisense_popup: &Arc<Mutex<IntellisensePopup>>,
        runtime: &Arc<IntellisenseRuntimeState>,
        cursor_pos: i32,
        key: Key,
        typed_char: Option<char>,
    ) -> bool {
        if !intellisense_popup
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .is_visible()
        {
            return false;
        }

        let Some(range) = runtime.completion_range() else {
            return false;
        };
        let start = range.start();
        let end = range.end();

        let cursor = cursor_pos.max(0) as usize;
        if !Self::is_cursor_within_completion_range(cursor, start, end, key, typed_char) {
            return false;
        }

        if !Self::is_fast_filter_key(key, typed_char) {
            return false;
        }

        // Fast path: keep existing suggestions and just filter by the current in-range prefix.
        // This avoids re-tokenizing/re-analyzing SQL on each extra identifier keystroke.
        let prefix = Self::prefix_in_completion_range(buffer, text_shadow, start, cursor_pos);
        let qualifier = Self::qualifier_before_word(buffer, text_shadow, start);
        if Self::should_hide_fast_path_after_delete(&prefix, qualifier.as_deref(), key) {
            intellisense_popup
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .hide();
            runtime.clear_completion_range();
            return true;
        }
        {
            let mut popup = intellisense_popup
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            popup.filter_visible_suggestions_by_prefix(&prefix);
            if !popup.is_visible() {
                runtime.clear_completion_range();
            } else {
                let (popup_width, popup_height) = popup.popup_dimensions();
                let (popup_x, popup_y) =
                    Self::popup_screen_position(editor, cursor_pos, popup_width, popup_height);
                popup.set_position(popup_x, popup_y);
                runtime.set_completion_range(Some(IntellisenseCompletionRange::new(
                    start,
                    cursor.max(start),
                )));
            }
        }
        true
    }

    fn popup_screen_position(
        editor: &TextEditor,
        cursor_pos: i32,
        popup_width: i32,
        popup_height: i32,
    ) -> (i32, i32) {
        let (cursor_x, cursor_y) = editor.position_to_xy(cursor_pos);
        let (win_x, win_y) = editor
            .window()
            .map(|win| (win.x_root(), win.y_root()))
            .unwrap_or((0, 0));

        let mut popup_x = win_x + cursor_x;
        let mut popup_y = win_y + cursor_y + Self::INTELLISENSE_POPUP_Y_OFFSET;

        if let Some(win) = editor.window() {
            let win_w = win.w();
            let win_h = win.h();
            let max_x = (win_x + win_w - popup_width).max(win_x);
            let max_y = (win_y + win_h - popup_height).max(win_y);
            popup_x = popup_x.clamp(win_x, max_x);
            popup_y = popup_y.clamp(win_y, max_y);
        }

        (popup_x, popup_y)
    }

    fn is_cursor_within_completion_range(
        cursor: usize,
        start: usize,
        end: usize,
        key: Key,
        typed_char: Option<char>,
    ) -> bool {
        if cursor >= start && cursor <= end {
            return true;
        }

        // Allow forward typing past the previous end only for identifier-extension input.
        cursor > end
            && typed_char.is_some_and(Self::is_completion_prefix_char)
            && !matches!(key, Key::BackSpace | Key::Delete)
    }

    fn is_fast_filter_key(key: Key, typed_char: Option<char>) -> bool {
        if matches!(key, Key::BackSpace | Key::Delete) {
            return true;
        }
        typed_char.is_some_and(Self::is_completion_prefix_char)
    }

    fn is_completion_prefix_char(ch: char) -> bool {
        sql_text::is_identifier_char(ch) || matches!(ch, '"' | '`')
    }

    fn should_force_full_analysis(ch: char) -> bool {
        ch == '.'
            || ch.is_whitespace()
            || matches!(
                ch,
                ',' | '(' | ')' | '+' | '-' | '*' | '/' | '%' | '=' | '!' | '<' | '>' | ';' | ':'
            )
    }

    fn has_min_intellisense_prefix(word: &str) -> bool {
        let mut chars = word.chars();
        chars.next().is_some() && chars.next().is_some()
    }

    fn should_hide_fast_path_after_delete(prefix: &str, qualifier: Option<&str>, key: Key) -> bool {
        matches!(key, Key::BackSpace | Key::Delete)
            && qualifier.is_none()
            && !Self::has_min_intellisense_prefix(prefix)
    }

    fn should_ignore_keyup_after_manual_trigger(
        key: Key,
        original_key: Key,
        ctrl_or_cmd: bool,
    ) -> bool {
        ctrl_or_cmd && Self::shortcut_key_for_layout(key, original_key) == Key::from_char(' ')
    }

    fn shortcut_key_for_layout(key: Key, original_key: Key) -> Key {
        if (0..=0x7f).contains(&key.bits()) {
            key
        } else {
            original_key
        }
    }

    fn matches_alpha_shortcut(key: Key, ascii: char) -> bool {
        key == Key::from_char(ascii.to_ascii_lowercase())
            || key == Key::from_char(ascii.to_ascii_uppercase())
    }

    fn should_auto_trigger_intellisense_for_forced_char(
        word: &str,
        qualifier: Option<&str>,
    ) -> bool {
        qualifier.is_some() || Self::has_min_intellisense_prefix(word)
    }

    fn prefix_in_completion_range(
        buffer: &TextBuffer,
        text_shadow: &Arc<Mutex<HighlightShadowState>>,
        start: usize,
        cursor_pos: i32,
    ) -> String {
        let cursor = cursor_pos.max(0) as usize;
        let end = cursor.max(start);
        let text = text_buffer_access::text_range(buffer, Some(text_shadow), start as i32, end as i32);
        Self::completion_prefix_from_range_text(&text)
    }

    fn completion_prefix_from_range_text(text: &str) -> String {
        if matches!(text.chars().next(), Some('"') | Some('`')) {
            return text.to_string();
        }

        text.chars()
            .filter(|ch| Self::is_completion_prefix_char(*ch))
            .collect()
    }

    fn char_before_cursor(
        buffer: &TextBuffer,
        text_shadow: &Arc<Mutex<HighlightShadowState>>,
        cursor_pos: i32,
    ) -> Option<char> {
        if cursor_pos <= 0 {
            return None;
        }
        let start = (cursor_pos - 4).max(0);
        let text = text_buffer_access::text_range(buffer, Some(text_shadow), start, cursor_pos);
        text.chars().next_back()
    }

    fn non_whitespace_char_before_cursor(
        buffer: &TextBuffer,
        text_shadow: &Arc<Mutex<HighlightShadowState>>,
        cursor_pos: i32,
    ) -> Option<char> {
        if cursor_pos <= 0 {
            return None;
        }
        let start = (cursor_pos - INTELLISENSE_CONTEXT_WINDOW).max(0);
        let text = text_buffer_access::text_range(buffer, Some(text_shadow), start, cursor_pos);
        text.chars().rev().find(|ch| !ch.is_whitespace())
    }

    #[cfg(test)]
    fn non_whitespace_char_before_cursor_in_text(text: &str, cursor_pos: usize) -> Option<char> {
        if text.is_empty() || cursor_pos == 0 {
            return None;
        }
        let cursor_pos = cursor_pos.min(text.len());
        let text = text.get(..cursor_pos).unwrap_or("");
        text.chars().rev().find(|ch| !ch.is_whitespace())
    }

    fn typed_char_from_key_event(
        event_text: &str,
        key: Key,
        shift: bool,
        char_before_cursor: Option<char>,
    ) -> Option<char> {
        if let Some(ch) = event_text.chars().next() {
            return Some(ch);
        }

        if key == Key::from_char('-') {
            // FLTK can report '_' as key '-' with empty event_text when Shift state is
            // already released in KeyUp. Infer from the actual inserted buffer character.
            if let Some(prev) = char_before_cursor {
                if prev == '_' || prev == '-' {
                    return Some(prev);
                }
            }
            if shift {
                return Some('_');
            }
            return Some('-');
        }

        None
    }

    fn is_modifier_key(key: Key) -> bool {
        matches!(
            key,
            Key::ShiftL
                | Key::ShiftR
                | Key::ControlL
                | Key::ControlR
                | Key::AltL
                | Key::AltR
                | Key::MetaL
                | Key::MetaR
                | Key::CapsLock
        )
    }
}

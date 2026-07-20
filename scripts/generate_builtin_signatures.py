#!/usr/bin/env python3
"""Generate the built-in function signature catalog from official manuals.

The input files are downloaded snapshots of Oracle 26ai SQL Quick Reference,
MySQL 8.0 Reference Manual pages, and MariaDB's Markdown documentation. Oracle
signatures absent from the quick reference are taken from the 26ai SQL Language
Reference. The generated Rust is written to stdout so repository updates can
still be applied with the normal patch workflow.
"""

from __future__ import annotations

import argparse
import html
import json
import os
import re
import sys
from collections import defaultdict
from pathlib import Path


ORACLE_EXTRA_SIGNATURES = {
    "APPENDCHILDXML": "APPENDCHILDXML(XMLType_instance, XPath_string, value_expr [, namespace_string])",
    "CLASSIFIER": "CLASSIFIER()",
    "DELETEXML": "DELETEXML(XMLType_instance, XPath_string [, namespace_string])",
    "FIRST": "FIRST(expr [, offset])",
    "INSERTCHILDXML": "INSERTCHILDXML(XMLType_instance, XPath_string, child_expr, value_expr [, namespace_string])",
    "INSERTCHILDXMLAFTER": "INSERTCHILDXMLAFTER(XMLType_instance, XPath_string, child_expr, value_expr [, namespace_string])",
    "INSERTCHILDXMLBEFORE": "INSERTCHILDXMLBEFORE(XMLType_instance, XPath_string, child_expr, value_expr [, namespace_string])",
    "INSERTXMLBEFORE": "INSERTXMLBEFORE(XMLType_instance, XPath_string, value_expr [, namespace_string])",
    "LAST": "LAST(expr [, offset])",
    "LENGTHB": "LENGTHB(char)",
    "MATCH_NUMBER": "MATCH_NUMBER()",
    "NEXT": "NEXT(expr [, offset])",
    "ODCINUMBERLIST": "ODCINUMBERLIST(number [, number ]...)",
    "PREV": "PREV(expr [, offset])",
    "SYS_ROW_ETAG": "SYS_ROW_ETAG(column_name [, column_name]...)",
    "UPDATEXML": "UPDATEXML(XMLType_instance, XPath_string, value_expr [, XPath_string, value_expr ]...)",
    "XMLATTRIBUTES": "XMLATTRIBUTES(value_expr [ AS identifier ] [, value_expr [ AS identifier ] ]...)",
    "XMLROOT": "XMLROOT(value_expr, VERSION string [ STANDALONE { YES | NO | VALUE } ])",
    "XMLTYPE": "XMLTYPE(xmlData [, schema [, validated [, wellformed ]]])",
}

ORACLE_SIGNATURE_OVERRIDES = {
    "COALESCE": "COALESCE(expr1, expr2 [, expr]...)",
    "DOMAIN_NAME": "DOMAIN_NAME(expr [, expr]...)",
    "DOMAIN_ORDER": "DOMAIN_ORDER(expr [, expr]...)",
    "FIRST_VALUE": [
        "FIRST_VALUE(expr) [ { RESPECT | IGNORE } NULLS ] OVER { window_name | (analytic_clause) }",
        "FIRST_VALUE(expr [ { RESPECT | IGNORE } NULLS ]) OVER { window_name | (analytic_clause) }",
    ],
    "JSON_VALUE": "JSON_VALUE(expr [ FORMAT JSON ] [, JSON_basic_path_expression] [ JSON_passing_clause ] [ JSON_value_returning_clause ] [ JSON_value_on_error_clause ] [ JSON_value_on_empty_clause ] [ JSON_value_on_mismatch_clause ] [ TYPE ( { STRICT | LAX } ) ])",
    "LAG": [
        "LAG(value_expr [, offset [, default]]) [ { RESPECT | IGNORE } NULLS ] OVER { window_name | ([window_name] [query_partition_clause] order_by_clause) }",
        "LAG(value_expr [ { RESPECT | IGNORE } NULLS ] [, offset [, default]]) OVER { window_name | ([window_name] [query_partition_clause] order_by_clause) }",
    ],
    "LAST_VALUE": [
        "LAST_VALUE(expr) [ { RESPECT | IGNORE } NULLS ] OVER { window_name | (analytic_clause) }",
        "LAST_VALUE(expr [ { RESPECT | IGNORE } NULLS ]) OVER { window_name | (analytic_clause) }",
    ],
    "LEAD": [
        "LEAD(value_expr [, offset [, default]]) [ { RESPECT | IGNORE } NULLS ] OVER { window_name | ([window_name] [query_partition_clause] order_by_clause) }",
        "LEAD(value_expr [ { RESPECT | IGNORE } NULLS ] [, offset [, default]]) OVER { window_name | ([window_name] [query_partition_clause] order_by_clause) }",
    ],
    "LISTAGG": "LISTAGG([ALL | DISTINCT] measure_expr [, 'delimiter'] [listagg_overflow_clause]) [WITHIN GROUP order_by_clause] [OVER { window_name | ([window_name] query_partition_clause) }] [FILTER (WHERE condition)]",
    "MEDIAN": "MEDIAN(expr) [OVER { window_name | ([window_name] query_partition_clause) }] [FILTER (WHERE condition)]",
    "PERCENT_RANK": [
        "PERCENT_RANK(expr [, expr]...) WITHIN GROUP (ORDER BY expr [DESC | ASC] [NULLS { FIRST | LAST }] [, expr [DESC | ASC] [NULLS { FIRST | LAST }]]...)",
        "PERCENT_RANK() OVER { window_name | ([window_name] [query_partition_clause] order_by_clause) }",
    ],
    "STATS_F_TEST": [
        "STATS_F_TEST(expr1, expr2 [, { STATISTIC | DF_NUM | DF_DEN | ONE_SIDED_SIG }, expr3]) [ FILTER ( WHERE condition ) ]",
        "STATS_F_TEST(expr1, expr2 [, TWO_SIDED_SIG]) [ FILTER ( WHERE condition ) ]",
    ],
    "STATS_MW_TEST": [
        "STATS_MW_TEST(expr1, expr2 [, ONE_SIDED_SIG, expr3]) [ FILTER ( WHERE condition ) ]",
        "STATS_MW_TEST(expr1, expr2 [, { STATISTIC | U_STATISTIC | TWO_SIDED_SIG }]) [ FILTER ( WHERE condition ) ]",
    ],
    "STATS_T_TEST_INDEP": [
        "STATS_T_TEST_INDEP(expr1, expr2 [, { STATISTIC | ONE_SIDED_SIG }, expr3]) [ FILTER ( WHERE condition ) ]",
        "STATS_T_TEST_INDEP(expr1, expr2 [, { TWO_SIDED_SIG | DF }]) [ FILTER ( WHERE condition ) ]",
    ],
    "STATS_T_TEST_INDEPU": [
        "STATS_T_TEST_INDEPU(expr1, expr2 [, { STATISTIC | ONE_SIDED_SIG }, expr3]) [ FILTER ( WHERE condition ) ]",
        "STATS_T_TEST_INDEPU(expr1, expr2 [, { TWO_SIDED_SIG | DF }]) [ FILTER ( WHERE condition ) ]",
    ],
    "STATS_T_TEST_ONE": "STATS_T_TEST_ONE(expr1 [, expr2] [, { STATISTIC | ONE_SIDED_SIG | TWO_SIDED_SIG | DF }]) [ FILTER ( WHERE condition ) ]",
    "STATS_T_TEST_PAIRED": "STATS_T_TEST_PAIRED(expr1, expr2 [, { STATISTIC | ONE_SIDED_SIG | TWO_SIDED_SIG | DF }]) [ FILTER ( WHERE condition ) ]",
    "TRIM": [
        "TRIM(trim_source)",
        "TRIM({ { LEADING | TRAILING | BOTH } [ trim_character ] | trim_character } FROM trim_source)",
    ],
    "XMLTABLE": [
        "XMLTABLE(XQuery_string XMLTABLE_options)",
        "XMLTABLE(XMLnamespaces_clause, XQuery_string XMLTABLE_options)",
    ],
}

for _retail_name in (
    "RETAIL_WEEK_START_DATE",
    "RETAIL_WEEK_END_DATE",
    "RETAIL_MONTH_START_DATE",
    "RETAIL_MONTH_END_DATE",
    "RETAIL_QUARTER_START_DATE",
    "RETAIL_QUARTER_END_DATE",
    "RETAIL_YEAR_START_DATE",
    "RETAIL_YEAR_END_DATE",
):
    ORACLE_SIGNATURE_OVERRIDES[_retail_name] = (
        f"{_retail_name}(dtexpr [, is_restated])"
    )

MYSQL_EXTRA_SIGNATURES = {
    # ROW is a documented row-constructor expression, but the built-in table
    # does not repeat it as a scalar function entry.
    "ROW": "ROW(value1, value2 [, value]...)",
}

MYSQL_SIGNATURE_OVERRIDES = {
    "TRIM": [
        "TRIM(str)",
        "TRIM({ BOTH | LEADING | TRAILING } [remstr] FROM str)",
        "TRIM(remstr FROM str)",
    ],
}


ARGUMENT_SEPARATOR_KEYWORDS = {
    "CAST": ("AS",),
    "CONVERT": ("USING",),
    "EXTRACT": ("FROM",),
    "MID": ("FROM", "FOR"),
    "POSITION": ("IN",),
    "SUBSTR": ("FROM", "FOR"),
    "SUBSTRING": ("FROM", "FOR"),
    "TRANSLATE": ("USING",),
    "TREAT": ("AS",),
    "TRIM": ("FROM",),
    "XMLCAST": ("AS",),
}


MARIADB_SIGNATURE_OVERRIDES = {
    # The current documentation block is mislabeled AES_ENCRYPT, while the
    # description and examples document the AES_DECRYPT form below.
    "AES_DECRYPT": "AES_DECRYPT(crypt_str, key_str [, iv [, mode]])",
    "CRC32": ["CRC32(expr)", "CRC32(par, expr)"],
    "CRC32C": ["CRC32C(expr)", "CRC32C(par, expr)"],
    "EQUALS": "EQUALS(g1, g2)",
    "JSON_MERGE_PATCH": "JSON_MERGE_PATCH(json_doc, json_doc [, json_doc]...)",
    "JSON_MERGE_PRESERVE": "JSON_MERGE_PRESERVE(json_doc, json_doc [, json_doc]...)",
    "MASTER_GTID_WAIT": "MASTER_GTID_WAIT(gtid_list [, timeout])",
    "MEDIAN": "MEDIAN(median_expression) OVER ([partition_clause])",
    "PERCENTILE_CONT": "PERCENTILE_CONT(fraction) WITHIN GROUP (ORDER BY sort_expression) OVER ([partition_clause])",
    "PERCENTILE_DISC": "PERCENTILE_DISC(fraction) WITHIN GROUP (ORDER BY sort_expression) OVER ([partition_clause])",
    "ST_MLINEFROMWKB": "ST_MLINEFROMWKB(wkb [, srid])",
    "ST_MPOINTFROMWKB": "ST_MPOINTFROMWKB(wkb [, srid])",
    "ST_MPOLYFROMTEXT": "ST_MPOLYFROMTEXT(wkt [, srid])",
    "ST_MPOLYFROMWKB": "ST_MPOLYFROMWKB(wkb [, srid])",
    "ST_MULTILINESTRINGFROMWKB": "ST_MULTILINESTRINGFROMWKB(wkb [, srid])",
    "ST_MULTIPOINTFROMWKB": "ST_MULTIPOINTFROMWKB(wkb [, srid])",
    "ST_MULTIPOLYGONFROMTEXT": "ST_MULTIPOLYGONFROMTEXT(wkt [, srid])",
    "ST_MULTIPOLYGONFROMWKB": "ST_MULTIPOLYGONFROMWKB(wkb [, srid])",
    "TRIM": [
        "TRIM(str)",
        "TRIM(remstr FROM str)",
    ],
}


NON_CALL_MARIADB_ENTRIES = {
    "CASE",
    "DIV",
    "IN",
    "IS",
    "LIKE",
    "REGEXP",
    "RLIKE",
    "XOR",
}

NON_CALL_MYSQL_ENTRIES = {"EXISTS", "IN"}


def clean_markup(value: str) -> str:
    value = re.sub(r"<.*?>", "", value, flags=re.S)
    return " ".join(html.unescape(value).split())


def normalized_code(value: str) -> str:
    value = re.sub(r"<.*?>", "", value, flags=re.S)
    value = html.unescape(value)
    return " ".join(value.split())


def normalize_signature(value: str) -> str:
    value = " ".join(value.split()).strip().rstrip(",;").rstrip()
    # Some manuals write an optional argument as `arg, [, optional]`. The
    # comma before the optional group is duplicated and is not valid SQL.
    value = re.sub(r",\s*\[\s*,\s*", " [, ", value)
    return value


def matching_call(value: str, name: str) -> tuple[int, int] | None:
    match = re.search(rf"(?i)(?<![A-Z0-9_]){re.escape(name)}\s*\(", value)
    if not match:
        return None
    open_paren = value.find("(", match.start())
    depth = 0
    quote = None
    escaped = False
    for index in range(open_paren, len(value)):
        char = value[index]
        if quote:
            if escaped:
                escaped = False
            elif char == "\\":
                escaped = True
            elif char == quote:
                quote = None
            continue
        if char in {"'", '"'}:
            quote = char
        elif char == "(":
            depth += 1
        elif char == ")":
            depth -= 1
            if depth == 0:
                return match.start(), index + 1
    return None


def split_top_level_overloads(value: str) -> list[str]:
    parts = []
    start = 0
    depths = {"(": 0, "[": 0, "{": 0}
    closing = {")": "(", "]": "[", "}": "{"}
    quote = None
    escaped = False
    for index, char in enumerate(value):
        if quote:
            if escaped:
                escaped = False
            elif char == "\\":
                escaped = True
            elif char == quote:
                quote = None
            continue
        if char in {"'", '"'}:
            quote = char
        elif char in depths:
            depths[char] += 1
        elif char in closing:
            opener = closing[char]
            depths[opener] = max(0, depths[opener] - 1)
        elif char == "|" and not any(depths.values()):
            part = value[start:index].strip()
            if part:
                parts.append(part)
            start = index + 1
    part = value[start:].strip()
    if part:
        parts.append(part)
    return parts


ORACLE_GRAMMAR_CALL_WORDS = {
    "COLUMNS",
    "FILTER",
    "GROUP",
    "KEEP",
    "OVER",
    "PARTITION",
}

ORACLE_GROUP_HEADINGS = {
    "CALENDAR",
    "CALENDAR_ADD_X_PERIODS",
    "CALENDAR_START_END",
    "CALENDAR_X_OF_Y",
    "DATEDIFF",
    "FISCAL",
    "FISCAL_ADD_X_PERIODS",
    "FISCAL_START_END",
    "FISCAL_X_OF_Y",
    "RETAIL",
    "RETAIL_ADD_X_PERIODS",
    "RETAIL_START_END",
    "RETAIL_X_OF_Y",
}

# The quick reference illustration has `DOMAIN DISPLAY` and an obsolete
# `domain_name` placeholder. The detailed reference and all examples use the
# variadic expression form below.
ORACLE_QUICK_REFERENCE_SIGNATURE_FIXES = {
    "DOMAIN_DISPLAY": "DOMAIN_DISPLAY(expr [, expr]...)",
}

# FIRST and LAST are documented through KEEP (DENSE_RANK ...) syntax in the
# quick reference rather than as calls. Their callable row-pattern forms come
# from the SQL Language Reference and are listed in ORACLE_EXTRA_SIGNATURES.
ORACLE_NON_CALL_SIGNATURE_HEADINGS = {"FIRST", "LAST"}

ORACLE_CALL_STYLE_CONDITIONS = {
    "EQUALS_PATH",
    "JSON_EQUAL",
    "JSON_EXISTS",
    "JSON_TEXTCONTAINS",
    "REGEXP_LIKE",
    "UNDER_PATH",
}

ORACLE_CONDITION_SIGNATURE_OVERRIDES = {
    "JSON_EQUAL": "JSON_EQUAL(json1, json2 [ { ERROR | TRUE | FALSE } ON ERROR ])",
    "JSON_EXISTS": "JSON_EXISTS(expr [ FORMAT JSON ], JSON_basic_path_expression [ PASSING expr AS identifier [, expr AS identifier ]...] [ { ERROR | TRUE | FALSE } ON ERROR ] [ TYPE ( { STRICT | LAX } ) ] [ { ERROR | TRUE | FALSE } ON EMPTY ])",
    "UNDER_PATH": [
        "UNDER_PATH(column, path_string [, correlation_integer])",
        "UNDER_PATH(column, levels, path_string [, correlation_integer])",
    ],
}


def oracle_signatures(path: Path) -> dict[str, list[str]]:
    source = path.read_text()
    grouped: dict[str, list[str]] = defaultdict(list)
    documented_headings = set()
    resolved_headings = set()
    for section in re.findall(r'<div class="section">(.*?)</div>', source, re.S):
        heading = re.search(r'<p class="subhead2".*?</p>', section, re.S)
        syntax = re.search(r"<pre[^>]*>(.*?)</pre>", section, re.S)
        if not heading or not syntax:
            continue
        heading_names = []
        for link_text in re.findall(r"<a [^>]*>(.*?)</a>", heading.group(), re.S):
            match = re.match(r"([A-Z][A-Z0-9_]*)", clean_markup(link_text))
            if match:
                heading_names.append(match.group(1))
        if not heading_names:
            match = re.match(r"([A-Z][A-Z0-9_]*)", clean_markup(heading.group()))
            if match:
                heading_names.append(match.group(1))
        documented_headings.update(heading_names)
        signature = normalize_signature(normalized_code(syntax.group(1)))
        direct_names = {
            name
            for name in re.findall(r"(?<![A-Z0-9_])([A-Z][A-Z0-9_]*)\s*\(", signature)
            if name not in ORACLE_GRAMMAR_CALL_WORDS
        }
        shared_groups = []
        for match in re.finditer(r"\{\s*([^{}]+?)\s*\}\s*\(", signature):
            alternatives = re.findall(r"\b[A-Z][A-Z0-9_]*\b", match.group(1))
            if any(name in alternatives for name in heading_names) or any(
                name in ORACLE_GROUP_HEADINGS for name in heading_names
            ):
                shared_groups.append((match.span(), alternatives))

        names = set()
        for heading_name in heading_names:
            if (
                heading_name in direct_names
                or any(heading_name in alternatives for _, alternatives in shared_groups)
                or re.match(rf"^{re.escape(heading_name)}(?:\b|\s)", signature)
            ):
                names.add(heading_name)
        for _, alternatives in shared_groups:
            names.update(alternatives)
        if any(name in ORACLE_GROUP_HEADINGS for name in heading_names):
            names.update(direct_names)

        for name in names:
            canonical = signature
            call = matching_call(signature, name)
            if call and call[0] > 0:
                canonical = signature[call[0] : call[1]]
            elif not call:
                for span, alternatives in shared_groups:
                    if name in alternatives:
                        canonical = signature[: span[0]] + name + signature[span[1] - 1 :]
                        break
            if canonical not in grouped[name]:
                grouped[name].append(canonical)
            if name in heading_names:
                resolved_headings.add(name)

        for name in heading_names:
            if name not in ORACLE_QUICK_REFERENCE_SIGNATURE_FIXES:
                continue
            fixed = normalize_signature(ORACLE_QUICK_REFERENCE_SIGNATURE_FIXES[name])
            if fixed not in grouped[name]:
                grouped[name].append(fixed)
            resolved_headings.add(name)

    unresolved_headings = documented_headings.difference(
        resolved_headings,
        ORACLE_GROUP_HEADINGS,
        ORACLE_NON_CALL_SIGNATURE_HEADINGS,
    )
    if unresolved_headings:
        raise ValueError(
            "unresolved Oracle quick-reference headings: "
            + ", ".join(sorted(unresolved_headings))
        )

    for name, signature in ORACLE_EXTRA_SIGNATURES.items():
        grouped[name].append(normalize_signature(signature))
    for name, override in ORACLE_SIGNATURE_OVERRIDES.items():
        values = override if isinstance(override, list) else [override]
        grouped[name] = [normalize_signature(value) for value in values]
    return {
        name: list(
            dict.fromkeys(
                overload
                for value in values
                for overload in split_top_level_overloads(value)
            )
        )
        for name, values in grouped.items()
    }


def oracle_condition_signatures(path: Path) -> dict[str, list[str]]:
    source = path.read_text()
    signatures: dict[str, list[str]] = {}
    for section in re.findall(r'<div class="section">(.*?)</div>', source, re.S):
        heading = re.search(r'<p class="subhead2".*?</p>', section, re.S)
        syntax = re.search(r"<pre[^>]*>(.*?)</pre>", section, re.S)
        if not heading or not syntax:
            continue
        heading_text = clean_markup(heading.group())
        match = re.match(r"([A-Z][A-Z0-9_]*)\s+condition\b", heading_text, re.I)
        if not match:
            continue
        name = match.group(1).upper()
        if name not in ORACLE_CALL_STYLE_CONDITIONS:
            continue
        value = normalize_signature(normalized_code(syntax.group(1)))
        call = matching_call(value, name)
        if not call:
            raise ValueError(f"Oracle condition signature not found: {name}")
        signatures[name] = split_top_level_overloads(value[call[0] : call[1]])

    missing = ORACLE_CALL_STYLE_CONDITIONS.difference(signatures)
    if missing:
        raise ValueError(
            "unresolved Oracle call-style conditions: " + ", ".join(sorted(missing))
        )
    for name, override in ORACLE_CONDITION_SIGNATURE_OVERRIDES.items():
        values = override if isinstance(override, list) else [override]
        signatures[name] = [normalize_signature(value) for value in values]
    return signatures


def mysql_index_rows(path: Path) -> dict[str, list[str]]:
    source = path.read_text()
    rows = {}
    for heading in re.findall(r'<th scope="row">(.*?)</th>', source, re.S):
        link = re.search(r'<a class="link" href="([^"]+)">', heading)
        if not link:
            continue
        href = link.group(1)
        for raw_name in re.findall(r'<code class="literal">(.*?)</code>', heading, re.S):
            name = clean_markup(raw_name)
            if re.fullmatch(r"[A-Za-z_][A-Za-z0-9_]*\(\)", name):
                rows.setdefault(name[:-2].upper(), [])
                if href not in rows[name[:-2].upper()]:
                    rows[name[:-2].upper()].append(href)
    return rows


def mysql_signatures(index_path: Path, pages_dir: Path) -> dict[str, list[str]]:
    signatures = {}
    for name, hrefs in mysql_index_rows(index_path).items():
        # The reference table also lists operators and server-internal helpers.
        # Neither is a user-callable built-in function: MySQL rejects the latter
        # with ER_NATIVE_FCT_NAME_COLLISION ("Access ... is rejected").
        if name in NON_CALL_MYSQL_ENTRIES or all(
            href.startswith("internal-functions.html#") for href in hrefs
        ):
            continue
        candidates = []
        for href in hrefs:
            if href.startswith("internal-functions.html#"):
                continue
            page_name, fragment = href.split("#", 1)
            source = (pages_dir / page_name).read_text()
            start = re.search(
                rf'(?:<a name="{re.escape(fragment)}"></a>|id="{re.escape(fragment)}")',
                source,
            )
            if not start:
                raise ValueError(f"MySQL section not found: {href}")
            tail = source[start.end() :]
            next_section = re.search(r'<a name="(?:function|operator)_[^"]+"></a>', tail)
            section = tail[: next_section.start()] if next_section else tail

            paragraph_candidates = []
            for paragraph in section.split("</p>"):
                if paragraph_candidates and '<a class="indexterm"' in paragraph:
                    break
                all_values = [
                    normalized_code(raw)
                    for raw in re.findall(
                        r'<a class="link"[^>]*>(.*?)</a>', paragraph, re.S
                    )
                ]
                values = [
                    value
                    for value in all_values
                    if re.match(rf"(?i){re.escape(name)}\s*\(", value)
                ]
                if not values:
                    if paragraph_candidates:
                        break
                    continue
                paragraph_text = normalized_code(paragraph)
                if len(all_values) == 1 and re.match(
                    rf"(?i)^{re.escape(name)}\s*\(", paragraph_text
                ):
                    paragraph_candidates.append(normalize_signature(paragraph_text))
                else:
                    paragraph_candidates.extend(map(normalize_signature, values))

            pre_candidates = []
            for raw in re.findall(
                r'<pre[^>]*><code[^>]*>(.*?)</code></pre>', section, re.S
            ):
                value = normalized_code(raw)
                call = matching_call(value, name)
                if not call or value[: call[0]].strip().upper() not in {"", "INT", "STRING"}:
                    continue
                tail_after_call = value[call[1] :].lstrip()
                if ";" in value or tail_after_call.startswith(("=", ":=")):
                    continue
                pre_candidates.append(normalize_signature(value[call[0] :]))

            empty_call = re.compile(rf"(?i)^{re.escape(name)}\s*\(\s*\)$")
            if paragraph_candidates and not all(
                empty_call.fullmatch(candidate) for candidate in paragraph_candidates
            ):
                candidates.extend(paragraph_candidates)
            elif pre_candidates:
                candidates.extend(pre_candidates)
            else:
                candidates.extend(paragraph_candidates)
        if not candidates:
            raise ValueError(f"MySQL signature not found: {name} ({', '.join(hrefs)})")
        signatures[name] = list(
            dict.fromkeys(
                overload
                for candidate in candidates
                for overload in split_top_level_overloads(candidate)
            )
        )
    for name, signature in MYSQL_EXTRA_SIGNATURES.items():
        signatures[name] = [normalize_signature(signature)]
    for name, override in MYSQL_SIGNATURE_OVERRIDES.items():
        values = override if isinstance(override, list) else [override]
        signatures[name] = [normalize_signature(value) for value in values]
    return signatures


def mariadb_index_rows(path: Path) -> dict[str, str]:
    rows = {}
    for line in path.read_text().splitlines():
        if not line.startswith("| [") or "](" not in line:
            continue
        raw_name, remainder = line[3:].split("](", 1)
        name = raw_name.replace("\\_", "_").strip()
        href = remainder.split(")", 1)[0]
        if re.fullmatch(r"[A-Za-z_][A-Za-z0-9_]*", name):
            upper = name.upper()
            if upper in {"UUIDV4", "UUIDV7"}:
                upper = upper.replace("UUIDV", "UUID_V")
            rows[upper] = href
    return rows


def markdown_syntax_blocks(source: str) -> list[str]:
    section = re.search(
        r"^#{2,4} Syntax\s*(.*?)(?=^#{2,4} |\Z)", source, re.S | re.M | re.I
    )
    if not section:
        return []
    content = section.group(1)
    current_tab = re.search(
        r'\{% tab title="Current" %\}(.*?)\{% endtab %\}', content, re.S
    )
    if current_tab:
        content = current_tab.group(1)
    return [block.strip() for block in re.findall(r"```[^\n]*\n(.*?)```", content, re.S)]


def canonical_markdown_signatures(block: str, name: str) -> list[str]:
    signatures = []
    cursor = 0
    while cursor < len(block):
        call = matching_call(block[cursor:], name)
        if not call:
            break
        start, end = (cursor + call[0], cursor + call[1])
        line_end = block.find("\n", end)
        if line_end < 0:
            line_end = len(block)
        remaining = block[end:]
        suffix = re.match(r"\s+(?:OVER|WITHIN\s+GROUP|FILTER)\b", remaining, re.I)
        if suffix:
            next_call = matching_call(remaining[suffix.end() :], name)
            suffix_end = suffix.end() + next_call[0] if next_call else len(remaining)
            line_end = end + suffix_end
        tail = block[end:line_end].strip().rstrip(",;").rstrip()
        signature = block[start:end]
        if tail and (suffix or not re.search(r"[A-Za-z_][A-Za-z0-9_]*\s*\(", tail)):
            signature += " " + tail
        signatures.append(normalize_signature(signature))
        cursor = max(line_end + 1, end)
    return signatures


def alias_target(source: str) -> tuple[str, str] | None:
    match = re.search(
        r"(?i)(?:synonym|alias) for \[([^]]+)\]\(([^)]+\.md)\)", source
    )
    if not match:
        return None
    return match.group(1).replace("\\_", "_").replace("()", ""), match.group(2)


def mariadb_signatures(index_path: Path, pages_dir: Path) -> dict[str, list[str]]:
    rows = mariadb_index_rows(index_path)
    signatures = {}
    unresolved = []
    for name, href in rows.items():
        if name in NON_CALL_MARIADB_ENTRIES:
            continue
        source = (pages_dir / os.path.basename(href)).read_text()
        # MariaDB's function index includes four Spider UDFs. They are installed
        # by the optional storage engine and are not server built-ins.
        if re.search(r"(?i)\bUDF\b|user-defined-functions\.md", source):
            continue
        if name in MARIADB_SIGNATURE_OVERRIDES:
            override = MARIADB_SIGNATURE_OVERRIDES[name]
            values = override if isinstance(override, list) else [override]
            signatures[name] = [normalize_signature(value) for value in values]
            continue
        blocks = markdown_syntax_blocks(source)
        candidates = [
            candidate
            for block in blocks
            for candidate in canonical_markdown_signatures(block, name)
        ]
        if candidates:
            signatures[name] = list(
                dict.fromkeys(
                    overload
                    for candidate in candidates
                    for overload in split_top_level_overloads(candidate)
                )
            )
            continue

        alias = alias_target(source)
        if alias:
            target_name, target_href = alias
            target_source = (pages_dir / os.path.basename(target_href)).read_text()
            target_blocks = markdown_syntax_blocks(target_source)
            target_candidates = [
                candidate
                for block in target_blocks
                for candidate in canonical_markdown_signatures(block, target_name)
            ]
            if target_candidates:
                signatures[name] = []
                for signature in target_candidates:
                    signature = re.sub(
                        rf"(?i)(?<![A-Z0-9_]){re.escape(target_name)}(?=\s*\()",
                        name,
                        signature,
                    )
                    signatures[name].extend(split_top_level_overloads(signature))
                signatures[name] = list(dict.fromkeys(signatures[name]))
                continue

        unresolved.append((name, href))

    if unresolved:
        for name, href in unresolved:
            print(f"unresolved MariaDB function: {name} ({href})", file=sys.stderr)
        raise ValueError(f"{len(unresolved)} MariaDB signatures unresolved")
    return signatures


def balanced_signature(value: str) -> bool:
    pairs = {")": "(", "]": "[", "}": "{"}
    stack = []
    quote = None
    escaped = False
    for char in value:
        if quote:
            if escaped:
                escaped = False
            elif char == "\\":
                escaped = True
            elif char == quote:
                quote = None
            continue
        if char in {"'", '"'}:
            quote = char
        elif char in pairs.values():
            stack.append(char)
        elif char in pairs:
            if not stack or stack.pop() != pairs[char]:
                return False
    return not stack and quote is None


def validate_catalogs(catalogs: dict[str, dict[str, list[str]]]) -> None:
    expected_sizes = {"ORACLE": 463, "MYSQL": 408, "MARIADB": 469}
    for dialect, expected_size in expected_sizes.items():
        actual_size = len(catalogs[dialect])
        if actual_size != expected_size:
            raise ValueError(
                f"{dialect} catalog has {actual_size} entries; expected {expected_size}"
            )

    non_call_oracle = {
        ("JSON_ARRAY", "JSON [ JSON_ARRAY_content ]"),
        ("JSON_OBJECT", "JSON { JSON_OBJECT_content }"),
    }
    for dialect, signatures in catalogs.items():
        for name, syntaxes in signatures.items():
            if not syntaxes:
                raise ValueError(f"{dialect} {name} has no signatures")
            if len(syntaxes) != len(set(syntaxes)):
                raise ValueError(f"{dialect} {name} has duplicate overloads")
            for syntax in syntaxes:
                if syntax != normalize_signature(syntax):
                    raise ValueError(f"{dialect} {name} is not normalized: {syntax}")
                if not balanced_signature(syntax):
                    raise ValueError(f"{dialect} {name} is unbalanced: {syntax}")
                if re.search(r",\s*\[\s*,", syntax):
                    raise ValueError(f"{dialect} {name} has a duplicate comma: {syntax}")
                if (name, syntax) in non_call_oracle:
                    continue
                if not re.match(rf"(?i)^{re.escape(name)}(?:\s*\(|\b)", syntax):
                    raise ValueError(
                        f"{dialect} {name} signature has an invalid head: {syntax}"
                    )


def rust_string(value: str) -> str:
    return json.dumps(value, ensure_ascii=False)


def emit_name_array(name: str, signatures: dict[str, list[str]]) -> None:
    print(f"pub const {name}: &[&str] = &[")
    for function_name in sorted(signatures):
        print(f"    {rust_string(function_name)},")
    print("];\n")


def emit_signature_array(name: str, signatures: dict[str, list[str]]) -> None:
    print(f"const {name}: &[BuiltinSignature] = &[")
    for function_name, syntaxes in sorted(signatures.items()):
        print("    BuiltinSignature {")
        print(f"        name: {rust_string(function_name)},")
        print("        syntaxes: &[")
        for syntax in syntaxes:
            print(f"            {rust_string(syntax)},")
        print("        ],")
        keywords = ARGUMENT_SEPARATOR_KEYWORDS.get(function_name, ())
        joined_keywords = ", ".join(rust_string(keyword) for keyword in keywords)
        print(f"        argument_separator_keywords: &[{joined_keywords}],")
        print("    },")
    print("];\n")


def emit_rust(catalogs: dict[str, dict[str, list[str]]]) -> None:
    print("// @generated by scripts/generate_builtin_signatures.py; do not edit by hand.")
    print("// Sources: Oracle AI Database 26ai SQL Language and Quick References,")
    print("// MySQL 8.0 Reference Manual, and MariaDB Server 12.2 documentation.\n")
    print("use crate::db::DatabaseType;")
    print(
        "use crate::ui::intellisense::{signature_overload_from_syntax, SignatureLabel};\n"
    )
    print("#[derive(Clone, Copy, Debug, PartialEq, Eq)]")
    print("struct BuiltinSignature {")
    print("    name: &'static str,")
    print("    syntaxes: &'static [&'static str],")
    print("    argument_separator_keywords: &'static [&'static str],")
    print("}\n")
    for dialect in ("ORACLE", "MYSQL", "MARIADB"):
        emit_name_array(f"{dialect}_FUNCTIONS", catalogs[dialect])
        emit_signature_array(f"{dialect}_SIGNATURES", catalogs[dialect])

    print(
        r"""fn signatures_for(db_type: DatabaseType) -> &'static [BuiltinSignature] {
    match db_type {
        DatabaseType::Oracle => ORACLE_SIGNATURES,
        DatabaseType::MySQL => MYSQL_SIGNATURES,
        DatabaseType::MariaDB => MARIADB_SIGNATURES,
    }
}

#[cfg(test)]
pub(crate) fn builtin_signature_syntaxes(
    db_type: DatabaseType,
    name: &str,
) -> Option<&'static [&'static str]> {
    Some(builtin_signature(db_type, name)?.syntaxes)
}

fn builtin_signature(db_type: DatabaseType, name: &str) -> Option<&'static BuiltinSignature> {
    let upper = name.to_ascii_uppercase();
    let table = signatures_for(db_type);
    let index = table.binary_search_by_key(&upper.as_str(), |entry| entry.name).ok()?;
    Some(&table[index])
}

pub(crate) fn builtin_signature_argument_separator_keywords(
    db_type: DatabaseType,
    name: &str,
) -> Option<&'static [&'static str]> {
    Some(builtin_signature(db_type, name)?.argument_separator_keywords)
}

pub(crate) fn builtin_signature_label(
    db_type: DatabaseType,
    name: &str,
) -> Option<SignatureLabel> {
    let entry = builtin_signature(db_type, name)?;
    let mut text = String::new();
    let mut arg_spans = Vec::new();
    let mut overloads = Vec::new();
    for (overload_index, syntax) in entry.syntaxes.iter().enumerate() {
        if overload_index > 0 {
            text.push('\n');
        }
        let offset = text.len();
        text.push_str(syntax);
        let overload = signature_overload_from_syntax(
            syntax,
            offset,
            entry.argument_separator_keywords,
        );
        if overload_index == 0 {
            arg_spans.clone_from(&overload.arg_spans);
        }
        overloads.push(overload);
    }
    Some(SignatureLabel {
        text,
        arg_spans,
        overloads,
    })
}
"""
    )


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("--oracle-html", type=Path, required=True)
    parser.add_argument("--oracle-conditions-html", type=Path, required=True)
    parser.add_argument("--mysql-index", type=Path, required=True)
    parser.add_argument("--mysql-pages", type=Path, required=True)
    parser.add_argument("--mariadb-index", type=Path, required=True)
    parser.add_argument("--mariadb-pages", type=Path, required=True)
    args = parser.parse_args()

    oracle = oracle_signatures(args.oracle_html)
    oracle.update(oracle_condition_signatures(args.oracle_conditions_html))
    catalogs = {
        "ORACLE": oracle,
        "MYSQL": mysql_signatures(args.mysql_index, args.mysql_pages),
        "MARIADB": mariadb_signatures(args.mariadb_index, args.mariadb_pages),
    }
    validate_catalogs(catalogs)
    emit_rust(catalogs)


if __name__ == "__main__":
    main()

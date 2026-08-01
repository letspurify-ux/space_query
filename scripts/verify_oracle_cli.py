#!/usr/bin/env python3
"""Compare production Oracle grids with SQL*Plus for every test fixture."""

from __future__ import annotations

import argparse
from datetime import datetime
from decimal import Decimal, InvalidOperation
from html.parser import HTMLParser
import json
import os
from pathlib import Path
import re
import subprocess
import tempfile


REPO_ROOT = Path(__file__).resolve().parents[1]
SNAPSHOT_BIN = REPO_ROOT / "target/debug/oracle_compare_test_all"
CONTAINER_FIXTURE_ROOT = "/tmp/space_query_repo"
DATETIME_FORMATS = (
    "%Y-%m-%d %H:%M:%S.%f",
    "%Y-%m-%d %H:%M:%S",
    "%Y-%m-%d",
)


class SqlPlusTableParser(HTMLParser):
    def __init__(self) -> None:
        super().__init__(convert_charrefs=True)
        self.tables: list[dict[str, list]] = []
        self._table: list[list[str]] | None = None
        self._row: list[str] | None = None
        self._cell: list[str] | None = None
        self._cell_right_aligned = False

    def handle_starttag(self, tag: str, attrs: list[tuple[str, str | None]]) -> None:
        if tag == "table":
            self._table = []
        elif tag == "tr" and self._table is not None:
            self._row = []
        elif tag in {"th", "td"} and self._row is not None:
            self._cell = []
            self._cell_right_aligned = dict(attrs).get("align") == "right"
        elif tag == "br" and self._cell is not None:
            self._cell.append("\n")

    def handle_data(self, data: str) -> None:
        if self._cell is not None:
            self._cell.append(data)
        elif self._table is None:
            for _ in range(data.lower().count("no rows selected")):
                self.tables.append({"columns": None, "rows": []})

    def handle_endtag(self, tag: str) -> None:
        if tag in {"th", "td"} and self._cell is not None and self._row is not None:
            raw_value = "".join(self._cell)
            if tag == "th" or self._cell_right_aligned:
                value = raw_value.strip()
            else:
                value = raw_value
                if value.startswith("\r\n"):
                    value = value[2:]
                elif value.startswith("\n"):
                    value = value[1:]
                if value.endswith("\r\n"):
                    value = value[:-2]
                elif value.endswith("\n"):
                    value = value[:-1]
            if value == "\u00a0":
                value = ""
            self._row.append(value)
            self._cell = None
            self._cell_right_aligned = False
        elif tag == "tr" and self._row is not None and self._table is not None:
            self._table.append(self._row)
            self._row = None
        elif tag == "table" and self._table is not None:
            if self._table:
                self.tables.append(
                    {"columns": self._table[0], "rows": self._table[1:]}
                )
            self._table = None


def run(command: list[str], **kwargs: object) -> subprocess.CompletedProcess[str]:
    return subprocess.run(
        command,
        cwd=REPO_ROOT,
        check=False,
        text=True,
        **kwargs,
    )


def fixture_paths() -> list[Path]:
    return sorted(
        path
        for path in (REPO_ROOT / "test").iterdir()
        if path.is_file() and path.suffix in {".sql", ".txt"}
    )


def oracle_env() -> dict[str, str]:
    result = os.environ.copy()
    result.setdefault("ORACLE_TEST_HOST", "127.0.0.1")
    result.setdefault("ORACLE_TEST_PORT", "1521")
    result.setdefault("ORACLE_TEST_SERVICE_NAME", "FREE")
    result.setdefault("ORACLE_TEST_USERNAME", "system")
    result.setdefault("ORACLE_TEST_PASSWORD", "password")
    result.setdefault(
        "ORACLE_CLIENT_LIB_DIR",
        str(Path.home() / ".local/share/oracle/instantclient_23_3"),
    )
    return result


def production_child(
    path: str,
    output_path: Path,
    driver: str = "oci",
    format_layout: str | None = None,
) -> None:
    command = [str(SNAPSHOT_BIN), "--child", driver, path, str(output_path)]
    if format_layout is not None:
        command.extend(["--format", format_layout])
    result = run(
        command,
        env=oracle_env(),
        stdout=subprocess.DEVNULL,
        stderr=subprocess.PIPE,
    )
    if result.returncode != 0:
        raise RuntimeError(
            f"production {driver} child failed for {path} with {result.returncode}:\n"
            f"{result.stderr}"
        )


def production_snapshot(
    path: Path, driver: str = "oci", format_layout: str | None = None
) -> dict:
    with tempfile.TemporaryDirectory(prefix="space_query_oracle_cli_") as temp_dir:
        cleanup_path = Path(temp_dir) / "cleanup.json"
        snapshot_path = Path(temp_dir) / "snapshot.json"
        production_child("__cleanup__", cleanup_path)
        production_child(
            str(path.relative_to(REPO_ROOT)),
            snapshot_path,
            driver,
            format_layout,
        )
        snapshot = json.loads(snapshot_path.read_text())
    if snapshot["failures"]:
        raise RuntimeError(
            f"production {driver} failures for {path}:\n"
            + "\n".join(snapshot["failures"])
        )
    return snapshot


def cleanup_for_sqlplus() -> None:
    with tempfile.TemporaryDirectory(prefix="space_query_oracle_cleanup_") as temp_dir:
        production_child("__cleanup__", Path(temp_dir) / "cleanup.json")


def sqlplus_snapshot(path: Path) -> list[dict[str, list]]:
    relative = path.relative_to(REPO_ROOT).as_posix()
    auto_commit = path.name != "final.sql"
    wrapper = [
        "SET ECHO OFF",
        "SET TERMOUT ON",
        "SET FEEDBACK ON",
        "SET VERIFY OFF",
        "SET HEADING ON",
        "SET PAGESIZE 50000",
        "SET LINESIZE 32767",
        "SET NUMWIDTH 40",
        "SET LONG 100000000",
        "SET LONGCHUNKSIZE 32767",
        "SET NULL NULL",
        f"SET AUTOCOMMIT {'ON' if auto_commit else 'OFF'}",
        # SQL*Plus prompts for the prose fragment "& catch" in test5.txt.
        # Preserve the literal value used by the noninteractive production path.
        "SET DEFINE OFF",
        'DEFINE catch = "& catch"',
        "SET DEFINE ON",
        "ALTER SESSION SET NLS_DATE_FORMAT='YYYY-MM-DD HH24:MI:SS';",
        "ALTER SESSION SET NLS_TIMESTAMP_FORMAT='YYYY-MM-DD HH24:MI:SS.FF6';",
        (
            "ALTER SESSION SET NLS_TIMESTAMP_TZ_FORMAT="
            "'YYYY-MM-DD HH24:MI:SS.FF6 TZR';"
        ),
        "SET MARKUP HTML ON SPOOL OFF ENTMAP ON",
    ]
    if auto_commit:
        wrapper.extend(["BEGIN DBMS_RANDOM.SEED(424242); END;", "/"])
    wrapper.extend([f'@"{relative}"', "EXIT"])
    result = run(
        [
            "docker",
            "exec",
            "-i",
            "-w",
            CONTAINER_FIXTURE_ROOT,
            "oracle",
            "sqlplus",
            "-S",
            "system/password@FREE",
        ],
        input="\n".join(wrapper) + "\n",
        stdout=subprocess.PIPE,
        stderr=subprocess.STDOUT,
    )
    if result.returncode != 0:
        raise RuntimeError(
            f"SQL*Plus failed for {relative} with {result.returncode}:\n{result.stdout}"
        )
    parser = SqlPlusTableParser()
    parser.feed(result.stdout)
    return parser.tables


def prepare_container_fixtures() -> None:
    result = run(
        [
            "docker",
            "exec",
            "-u",
            "0",
            "oracle",
            "sh",
            "-lc",
            f"rm -rf {CONTAINER_FIXTURE_ROOT} && mkdir -p {CONTAINER_FIXTURE_ROOT}",
        ],
        stdout=subprocess.PIPE,
        stderr=subprocess.STDOUT,
    )
    if result.returncode != 0:
        raise RuntimeError(f"failed to prepare Oracle fixture directory:\n{result.stdout}")
    result = run(
        [
            "docker",
            "cp",
            str(REPO_ROOT / "test"),
            f"oracle:{CONTAINER_FIXTURE_ROOT}/test",
        ],
        stdout=subprocess.PIPE,
        stderr=subprocess.STDOUT,
    )
    if result.returncode != 0:
        raise RuntimeError(f"failed to copy Oracle fixtures:\n{result.stdout}")


def without_synthetic_rowid(grid: dict) -> dict[str, list]:
    columns = list(grid["columns"])
    rows = [list(row) for row in grid["rows"]]
    sql_has_rowid = re.search(r"\browid\b", grid["sql"], re.IGNORECASE) is not None
    if not sql_has_rowid and "ROWID" in columns:
        index = columns.index("ROWID")
        columns.pop(index)
        for row in rows:
            row.pop(index)
    return {"sql": grid["sql"], "columns": columns, "rows": rows}


def normalize_sqlplus_print_grid(production: dict, official: dict) -> dict:
    if (
        production["sql"].lstrip().upper().startswith("PRINT ")
        and production["columns"] == ["NAME", "VALUE"]
        and len(production["rows"]) == 1
        and isinstance(official["columns"], list)
        and len(official["columns"]) == 1
        and len(official["rows"]) == 1
        and len(official["rows"][0]) == 1
    ):
        return {
            "columns": ["NAME", "VALUE"],
            "rows": [[official["columns"][0], official["rows"][0][0]]],
        }
    return official


def cursor_cell(value: str) -> dict | None:
    try:
        parsed = json.loads(value)
    except (json.JSONDecodeError, TypeError):
        return None
    if (
        isinstance(parsed, dict)
        and isinstance(parsed.get("columns"), list)
        and isinstance(parsed.get("rows"), list)
    ):
        return parsed
    return None


def consume_sqlplus_cursor_grid(
    expected: dict,
    official: list[dict[str, list]],
    start: int,
    path: Path,
) -> tuple[dict[str, list], int]:
    rows: list[list] = []
    index = start
    if (
        not expected["rows"]
        and index < len(official)
        and official[index]["columns"] is None
        and not official[index]["rows"]
    ):
        index += 1
    while len(rows) < len(expected["rows"]):
        if index >= len(official):
            raise AssertionError(f"{path}: SQL*Plus cursor output ended early")
        table = official[index]
        columns = table["columns"]
        if columns is None or len(columns) != len(expected["columns"]):
            raise AssertionError(
                f"{path}: SQL*Plus cursor column count differs: "
                f"production={len(expected['columns'])}, "
                f"SQL*Plus={None if columns is None else len(columns)}"
            )
        index += 1
        for official_row in table["rows"]:
            if len(rows) >= len(expected["rows"]):
                raise AssertionError(f"{path}: SQL*Plus cursor has extra rows")
            if len(official_row) != len(expected["columns"]):
                raise AssertionError(
                    f"{path}: SQL*Plus cursor row cell count differs: "
                    f"production={len(expected['columns'])}, "
                    f"SQL*Plus={len(official_row)}"
                )
            expected_row = expected["rows"][len(rows)]
            rebuilt_row: list = list(official_row)
            for cell_index, expected_cell in enumerate(expected_row):
                nested = (
                    expected_cell
                    if isinstance(expected_cell, dict)
                    and isinstance(expected_cell.get("columns"), list)
                    and isinstance(expected_cell.get("rows"), list)
                    else cursor_cell(expected_cell)
                    if isinstance(expected_cell, str)
                    else None
                )
                if nested is None:
                    continue
                if not re.fullmatch(
                    r"CURSOR STATEMENT\s*:\s*\d(?:\s*\d)*",
                    official_row[cell_index].strip(),
                    re.IGNORECASE,
                ):
                    continue
                rebuilt, index = consume_sqlplus_cursor_grid(
                    nested, official, index, path
                )
                rebuilt_row[cell_index] = rebuilt
            rows.append(rebuilt_row)
    return {"columns": expected["columns"], "rows": rows}, index


def normalize_sqlplus_cursor_grids(
    path: Path,
    production: list[dict],
    official: list[dict[str, list]],
) -> list[dict[str, list]]:
    normalized: list[dict[str, list]] = []
    index = 0
    for grid in production:
        has_cursor = any(
            cursor_cell(cell) is not None
            for row in grid["rows"]
            for cell in row
            if isinstance(cell, str)
        )
        if not has_cursor:
            if index >= len(official):
                raise AssertionError(f"{path}: SQL*Plus output ended early")
            normalized.append(official[index])
            index += 1
            continue
        rebuilt, index = consume_sqlplus_cursor_grid(grid, official, index, path)
        rebuilt["rows"] = [
            [
                json.dumps(cell, separators=(",", ":"))
                if isinstance(cell, dict)
                else cell
                for cell in row
            ]
            for row in rebuilt["rows"]
        ]
        normalized.append(rebuilt)
    if index != len(official):
        raise AssertionError(
            f"{path}: SQL*Plus has {len(official) - index} unclaimed grid(s)"
        )
    return normalized


def without_sqlplus_compute_separator(
    production_rows: list[list[str]], official_rows: list[list[str]]
) -> list[list[str]]:
    if len(official_rows) != len(production_rows) + 1:
        return official_rows
    filtered = [
        row
        for row in official_rows
        if not (
            any(cell and set(cell) == {"-"} for cell in row)
            and all(not cell.strip(" \u00a0") or set(cell) == {"-"} for cell in row)
        )
    ]
    return filtered if len(filtered) == len(production_rows) else official_rows


def parse_datetime(value: str) -> datetime | None:
    fractional = re.fullmatch(
        r"(\d{4}-\d{2}-\d{2} \d{2}:\d{2}:\d{2})\.(\d{1,9})", value
    )
    if fractional is not None:
        base = datetime.strptime(fractional.group(1), "%Y-%m-%d %H:%M:%S")
        return base.replace(
            microsecond=int(fractional.group(2)[:6].ljust(6, "0"))
        )
    for date_format in DATETIME_FORMATS:
        try:
            return datetime.strptime(value, date_format)
        except ValueError:
            pass
    return None


def cells_match(column: str, production: str, official: str) -> bool:
    if production == official:
        return True
    if official.replace("\n", "") == production:
        return True
    if ("\t" in production or "\t" in official) and re.sub(
        r"[ \t]+", " ", production
    ).rstrip(" \r\n") == re.sub(r"[ \t]+", " ", official).rstrip(" \r\n"):
        return True
    if production.rstrip(" ") == official:
        return True
    try:
        if json.loads(production) == json.loads(official):
            return True
    except (json.JSONDecodeError, TypeError):
        pass
    if column.upper() == "ROWID":
        return bool(production.strip()) and bool(official.strip())
    production_time = parse_datetime(production)
    official_time = parse_datetime(official)
    if production_time is not None and official_time is not None:
        return abs((production_time - official_time).total_seconds()) <= 600
    production_numeric = production
    official_numeric = official
    grouped_number = r"[+-]?\d{1,3}(?:,\d{3})+(?:\.\d+)?"
    if re.fullmatch(grouped_number, production_numeric):
        production_numeric = production_numeric.replace(",", "")
    if re.fullmatch(grouped_number, official_numeric):
        official_numeric = official_numeric.replace(",", "")
    try:
        production_number = Decimal(production_numeric)
        official_number = Decimal(official_numeric)
    except InvalidOperation:
        return False
    if production_number == official_number:
        return True
    if "BINARY_FLOAT" in column.upper():
        scale = max(abs(production_number), abs(official_number), Decimal(1))
        return abs(production_number - official_number) <= scale * Decimal("1e-6")
    production_digits = len(production_number.as_tuple().digits)
    official_digits = len(official_number.as_tuple().digits)
    if min(production_digits, official_digits) < 7:
        return False
    production_ulp = Decimal(1).scaleb(production_number.as_tuple().exponent)
    official_ulp = Decimal(1).scaleb(official_number.as_tuple().exponent)
    return abs(production_number - official_number) <= max(
        production_ulp, official_ulp
    ) / 2


def columns_match(production: list[str], official: list[str]) -> bool:
    return len(production) == len(official)


def compare(path: Path, production: dict, official: list[dict[str, list]]) -> tuple[int, int]:
    production_grids = [without_synthetic_rowid(grid) for grid in production["grids"]]
    official = normalize_sqlplus_cursor_grids(path, production_grids, official)
    if len(production_grids) != len(official):
        raise AssertionError(
            f"{path}: grid count differs: production={len(production_grids)}, "
            f"SQL*Plus={len(official)}"
        )

    cell_count = 0
    for grid_index, (production_grid, official_grid) in enumerate(
        zip(production_grids, official, strict=True)
    ):
        official_grid = normalize_sqlplus_print_grid(production_grid, official_grid)
        official_grid["rows"] = without_sqlplus_compute_separator(
            production_grid["rows"], official_grid["rows"]
        )
        official_columns = official_grid["columns"]
        official_is_empty_placeholder = (
            official_columns is None and not official_grid["rows"]
        )
        if official_is_empty_placeholder and production_grid["rows"]:
            raise AssertionError(
                f"{path}: grid #{grid_index} is empty in SQL*Plus but production has "
                f"{len(production_grid['rows'])} row(s)"
            )
        if not official_is_empty_placeholder and not columns_match(
            production_grid["columns"], official_columns
        ):
            raise AssertionError(
                f"{path}: grid #{grid_index} columns differ:\n"
                f"production={production_grid['columns']!r}\n"
                f"SQL*Plus={official_grid['columns']!r}"
            )
        if len(production_grid["rows"]) != len(official_grid["rows"]):
            raise AssertionError(
                f"{path}: grid #{grid_index} row count differs: "
                f"production={len(production_grid['rows'])}, "
                f"SQL*Plus={len(official_grid['rows'])}"
            )
        for row_index, (production_row, official_row) in enumerate(
            zip(production_grid["rows"], official_grid["rows"], strict=True)
        ):
            if len(production_row) != len(official_row):
                raise AssertionError(
                    f"{path}: grid #{grid_index} row #{row_index} cell count differs: "
                    f"production={len(production_row)}, SQL*Plus={len(official_row)}"
                )
            for column_index, (production_cell, official_cell) in enumerate(
                zip(production_row, official_row, strict=True)
            ):
                column = production_grid["columns"][column_index]
                if not cells_match(column, production_cell, official_cell):
                    raise AssertionError(
                        f"{path}: grid #{grid_index}, row #{row_index}, "
                        f"column {column!r} differs: production={production_cell!r}, "
                        f"SQL*Plus={official_cell!r}"
                    )
                cell_count += 1
    return len(production_grids), cell_count


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("paths", nargs="*", type=Path)
    args = parser.parse_args()
    paths = args.paths or fixture_paths()
    if not SNAPSHOT_BIN.is_file():
        raise RuntimeError(
            f"{SNAPSHOT_BIN} does not exist; build oracle_compare_test_all first"
        )
    prepare_container_fixtures()
    total_grids = 0
    total_cells = 0
    for path in paths:
        path = path if path.is_absolute() else REPO_ROOT / path
        production = production_snapshot(path)
        cleanup_for_sqlplus()
        official = sqlplus_snapshot(path)
        grid_count, cell_count = compare(path, production, official)
        total_grids += grid_count
        total_cells += cell_count
        print(
            f"PASS {path.relative_to(REPO_ROOT)}: "
            f"{grid_count} grid(s), {cell_count} cell(s)"
        )
    print(
        f"PASS Oracle production/SQL*Plus: {len(paths)} file(s), "
        f"{total_grids} grid(s), {total_cells} cell(s)"
    )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())

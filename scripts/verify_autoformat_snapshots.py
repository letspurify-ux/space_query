#!/usr/bin/env python3
"""Compare production result grids before and after SQL auto-formatting."""

from __future__ import annotations

import argparse
from pathlib import Path
import re

import verify_mysql_cli as mysql
import verify_oracle_cli as oracle


LAYOUTS = ("wrapped", "stacked")
SQL_DEFINITION_QUERY = re.compile(
    r"\s*SHOW\s+(?:CREATE\b|EVENTS\b|TRIGGERS\b)", re.IGNORECASE
)
SQL_TOKEN = re.compile(
    r"""
    '(?:''|\\.|[^'])*'
    | "(?:""|\\.|[^"])*"
    | `(?:``|\\.|[^`])*`
    | [A-Za-z_][$#A-Za-z0-9_]*
    | \d+(?:\.\d+)?
    | <=>|<=|>=|<>|!=|:=|<<|>>|&&|\|\|
    | \S
    """,
    re.VERBOSE | re.DOTALL,
)


def normalized_mysql_rows(grid: dict) -> list[list[str]]:
    rows = grid["rows"]
    sql = grid["sql"]
    if sql.strip().upper().startswith("SHOW PROCESSLIST"):
        rows = mysql.active_processlist_rows(grid["columns"], rows)
    if re.match(r"\s*SHOW\s+OPEN\s+TABLES\b", sql, re.IGNORECASE):
        rows = sorted(rows)
    return rows


def sql_definition_cells_match(
    sql: str, before: str, after: str
) -> bool:
    if SQL_DEFINITION_QUERY.match(sql) is None:
        return False
    before_datetimes = mysql.EMBEDDED_DATETIME.findall(before)
    after_datetimes = mysql.EMBEDDED_DATETIME.findall(after)
    if len(before_datetimes) != len(after_datetimes):
        return False
    for before_value, after_value in zip(
        before_datetimes, after_datetimes, strict=True
    ):
        before_time = mysql.parse_datetime(before_value)
        after_time = mysql.parse_datetime(after_value)
        if (
            before_time is None
            or after_time is None
            or abs((before_time - after_time).total_seconds()) > 600
        ):
            return False
    before = mysql.EMBEDDED_DATETIME.sub("<datetime>", before)
    after = mysql.EMBEDDED_DATETIME.sub("<datetime>", after)
    return SQL_TOKEN.findall(before) == SQL_TOKEN.findall(after)


def compare(
    label: str,
    path: Path,
    baseline: dict,
    formatted: dict,
    oracle_mode: bool,
) -> tuple[int, int, int, int]:
    baseline_grids = baseline["grids"]
    formatted_grids = formatted["grids"]
    if len(baseline_grids) != len(formatted_grids):
        raise AssertionError(
            f"{path}: {label} grid count differs: "
            f"before={len(baseline_grids)}, after={len(formatted_grids)}"
        )

    cell_count = 0
    definition_formatting_cells = 0
    changed_column_labels = 0
    for grid_index, (before_grid, after_grid) in enumerate(
        zip(baseline_grids, formatted_grids, strict=True)
    ):
        if len(before_grid["columns"]) != len(after_grid["columns"]):
            raise AssertionError(
                f"{path}: {label} grid #{grid_index} column count differs: "
                f"before={len(before_grid['columns'])}, "
                f"after={len(after_grid['columns'])}"
            )
        changed_column_labels += sum(
            before_column != after_column
            for before_column, after_column in zip(
                before_grid["columns"], after_grid["columns"], strict=True
            )
        )
        before_rows = (
            before_grid["rows"]
            if oracle_mode
            else normalized_mysql_rows(before_grid)
        )
        after_rows = (
            after_grid["rows"]
            if oracle_mode
            else normalized_mysql_rows(after_grid)
        )
        if len(before_rows) != len(after_rows):
            raise AssertionError(
                f"{path}: {label} grid #{grid_index} row count differs: "
                f"before={len(before_rows)}, after={len(after_rows)}"
            )
        for row_index, (before_row, after_row) in enumerate(
            zip(before_rows, after_rows, strict=True)
        ):
            if len(before_row) != len(after_row):
                raise AssertionError(
                    f"{path}: {label} grid #{grid_index} row #{row_index} "
                    f"cell count differs: before={len(before_row)}, "
                    f"after={len(after_row)}"
                )
            for column_index, (before_cell, after_cell) in enumerate(
                zip(before_row, after_row, strict=True)
            ):
                column = before_grid["columns"][column_index]
                matches = (
                    oracle.cells_match(column, before_cell, after_cell)
                    if oracle_mode
                    else mysql.cells_match(
                        before_grid["sql"], column, before_cell, after_cell
                    )
                )
                if (
                    not matches
                    and not oracle_mode
                    and sql_definition_cells_match(
                        before_grid["sql"], before_cell, after_cell
                    )
                ):
                    definition_formatting_cells += 1
                    matches = True
                if not matches:
                    raise AssertionError(
                        f"{path}: {label} grid #{grid_index}, row #{row_index}, "
                        f"column {column!r} differs: "
                        f"before={before_cell!r}, after={after_cell!r}"
                    )
                cell_count += 1
    return (
        len(baseline_grids),
        cell_count,
        definition_formatting_cells,
        changed_column_labels,
    )


def verify_oracle(
    paths: list[Path], driver: str, layouts: list[str]
) -> None:
    if not oracle.SNAPSHOT_BIN.is_file():
        raise RuntimeError(
            f"{oracle.SNAPSHOT_BIN} does not exist; "
            "build oracle_compare_test_all first"
        )
    totals = {layout: [0, 0, 0, 0] for layout in layouts}
    for path in paths:
        baseline = oracle.production_snapshot(path, driver)
        for layout in layouts:
            formatted = oracle.production_snapshot(path, driver, layout)
            grids, cells, definition_cells, column_labels = compare(
                f"oracle/{driver}/{layout}", path, baseline, formatted, True
            )
            totals[layout][0] += grids
            totals[layout][1] += cells
            totals[layout][2] += definition_cells
            totals[layout][3] += column_labels
            print(
                f"PASS {driver}/{layout} {path.relative_to(oracle.REPO_ROOT)}: "
                f"{grids} grid(s), {cells} cell(s)"
            )
    for layout in layouts:
        grids, cells, definition_cells, column_labels = totals[layout]
        print(
            f"PASS oracle/{driver}/{layout} before/after auto-format: "
            f"{len(paths)} file(s), {grids} grid(s), {cells} cell(s), "
            f"{definition_cells} formatting-only definition cell(s), "
            f"{column_labels} changed generated column label(s)"
        )


def verify_mysql_family(
    db_type: str, paths: list[Path], layouts: list[str]
) -> None:
    if not mysql.SNAPSHOT_BIN.is_file():
        raise RuntimeError(
            f"{mysql.SNAPSHOT_BIN} does not exist; "
            "build mysql_fixture_snapshot first"
        )
    config = mysql.DATABASES[db_type]
    totals = {layout: [0, 0, 0, 0] for layout in layouts}
    for path in paths:
        baseline = mysql.production_snapshot(db_type, config, path)
        for layout in layouts:
            formatted = mysql.production_snapshot(
                db_type, config, path, layout
            )
            grids, cells, definition_cells, column_labels = compare(
                f"{db_type}/{layout}", path, baseline, formatted, False
            )
            totals[layout][0] += grids
            totals[layout][1] += cells
            totals[layout][2] += definition_cells
            totals[layout][3] += column_labels
            print(
                f"PASS {db_type}/{layout} "
                f"{path.relative_to(mysql.REPO_ROOT)}: "
                f"{grids} grid(s), {cells} cell(s)"
            )
    for layout in layouts:
        grids, cells, definition_cells, column_labels = totals[layout]
        print(
            f"PASS {db_type}/{layout} before/after auto-format: "
            f"{len(paths)} file(s), {grids} grid(s), {cells} cell(s), "
            f"{definition_cells} formatting-only definition cell(s), "
            f"{column_labels} changed generated column label(s)"
        )


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument(
        "--db-type", choices=("oracle", "mariadb", "mysql"), required=True
    )
    parser.add_argument("--oracle-driver", choices=("oci", "thin"), default="oci")
    parser.add_argument(
        "--layout", choices=LAYOUTS, action="append", dest="layouts"
    )
    parser.add_argument("paths", nargs="*", type=Path)
    args = parser.parse_args()
    layouts = args.layouts or list(LAYOUTS)

    if args.db_type == "oracle":
        paths = args.paths or oracle.fixture_paths()
        paths = [
            path if path.is_absolute() else oracle.REPO_ROOT / path
            for path in paths
        ]
        verify_oracle(paths, args.oracle_driver, layouts)
    else:
        config = mysql.DATABASES[args.db_type]
        paths = args.paths or mysql.fixture_paths(config["fixture_dir"])
        paths = [
            path if path.is_absolute() else mysql.REPO_ROOT / path
            for path in paths
        ]
        verify_mysql_family(args.db_type, paths, layouts)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())

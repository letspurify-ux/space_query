#!/usr/bin/env python3
"""Compare production MySQL/MariaDB grids with the official CLI client."""

from __future__ import annotations

import argparse
from datetime import datetime
from decimal import Decimal, InvalidOperation
import json
import os
from pathlib import Path
import re
import subprocess
import tempfile
import uuid
import xml.etree.ElementTree as ET


REPO_ROOT = Path(__file__).resolve().parents[1]
SNAPSHOT_BIN = REPO_ROOT / "target/debug/mysql_fixture_snapshot"
NIL_ATTRIBUTE = "{http://www.w3.org/2001/XMLSchema-instance}nil"
DATETIME_FORMATS = (
    "%Y-%m-%d %H:%M:%S.%f",
    "%Y-%m-%d %H:%M:%S",
    "%Y-%m-%d",
    "%H:%M:%S.%f",
    "%H:%M:%S",
)
EMBEDDED_DATETIME = re.compile(
    r"\d{4}-\d{2}-\d{2} \d{2}:\d{2}:\d{2}(?:\.\d{1,6})?"
)
DATABASES = {
    "mariadb": {
        "fixture_dir": "test_mariadb",
        "container": "space-query-mariadb122",
        "client": "mariadb",
        "port": "3306",
        "container_port": "3306",
        "database": "query_tool_test",
        "user": "root",
        "password": "password",
    },
    "mysql": {
        "fixture_dir": "test_mysql",
        "container": "space-query-mysql80",
        "client": "mysql",
        "port": "3307",
        "container_port": "3306",
        "database": "query_tool_mysql8",
        "user": "root",
        "password": "spacequery",
    },
}


def run(command: list[str], **kwargs: object) -> subprocess.CompletedProcess:
    return subprocess.run(command, cwd=REPO_ROOT, check=False, **kwargs)


def fixture_paths(fixture_dir: str) -> list[Path]:
    return sorted(
        path
        for path in (REPO_ROOT / fixture_dir).iterdir()
        if path.is_file() and path.suffix in {".sql", ".txt"}
    )


def production_env(config: dict[str, str]) -> dict[str, str]:
    result = os.environ.copy()
    result["SPACE_QUERY_TEST_MYSQL_HOST"] = "127.0.0.1"
    result["SPACE_QUERY_TEST_MYSQL_PORT"] = config["port"]
    result["SPACE_QUERY_TEST_MYSQL_DATABASE"] = config["database"]
    result["SPACE_QUERY_TEST_MYSQL_USER"] = config["user"]
    result["SPACE_QUERY_TEST_MYSQL_PASSWORD"] = config["password"]
    return result


def production_snapshot(
    db_type: str,
    config: dict[str, str],
    path: Path,
    format_layout: str | None = None,
) -> dict:
    with tempfile.TemporaryDirectory(prefix=f"space_query_{db_type}_cli_") as temp_dir:
        output_path = Path(temp_dir) / "snapshot.json"
        command = [
            str(SNAPSHOT_BIN),
            db_type,
            str(path.relative_to(REPO_ROOT)),
            str(output_path),
        ]
        if format_layout is not None:
            command.extend(["--format", format_layout])
        result = run(
            command,
            env=production_env(config),
            stdout=subprocess.DEVNULL,
            stderr=subprocess.PIPE,
        )
        if result.returncode != 0:
            raise RuntimeError(
                f"production {db_type} snapshot failed for {path} "
                f"with {result.returncode}:\n"
                + result.stderr.decode("utf-8", errors="replace")
            )
        snapshot = json.loads(output_path.read_text())
    if snapshot["failures"]:
        raise RuntimeError(
            f"production {db_type} failures for {path}:\n"
            + "\n".join(snapshot["failures"])
        )
    return snapshot


def parse_xml_resultsets(output: bytes, path: Path) -> list[dict[str, list]]:
    text = output.decode("utf-8")
    chunks = re.findall(r"<resultset\b.*?</resultset>", text, re.DOTALL)
    grids: list[dict[str, list]] = []
    for chunk in chunks:
        root = ET.fromstring(encode_xml_forbidden_controls(chunk))
        rows: list[list[str]] = []
        columns: list[str] | None = None
        for row_element in root.findall("row"):
            fields = row_element.findall("field")
            field_names = [field.get("name", "") for field in fields]
            if columns is None:
                columns = field_names
            elif columns != field_names:
                raise AssertionError(
                    f"{path}: official CLI changed columns within one resultset"
                )
            rows.append(
                [
                    "NULL"
                    if field.get(NIL_ATTRIBUTE) == "true"
                    else field.text or ""
                    for field in fields
                ]
            )
        grids.append(
            {
                "statement": root.get("statement", ""),
                "columns": columns,
                "rows": rows,
            }
        )
    return grids


def encode_xml_forbidden_controls(value: str) -> str:
    return "".join(
        f"__SPACE_QUERY_CONTROL_{ord(character):02X}__"
        if ord(character) < 32 and character not in "\t\n\r"
        else character
        for character in value
    )


def official_snapshot(config: dict[str, str], path: Path) -> list[dict[str, list]]:
    prefix = (
        "SET SESSION sql_mode='TRADITIONAL';\n"
        "SET SESSION time_zone='+00:00';\n"
        "SET NAMES utf8mb4;\n"
    )
    sql = prefix.encode() + path.read_bytes()
    result = run(
        [
            "docker",
            "exec",
            "-i",
            config["container"],
            config["client"],
            "--xml",
            "--raw",
            "--binary-as-hex",
            "--binary-mode",
            "--default-character-set=utf8mb4",
            "-h127.0.0.1",
            f"-P{config['container_port']}",
            f"-u{config['user']}",
            f"-p{config['password']}",
            config["database"],
        ],
        input=sql,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
    )
    if result.returncode != 0:
        raise RuntimeError(
            f"official {config['client']} failed for {path} with "
            f"{result.returncode}:\n"
            + result.stderr.decode("utf-8", errors="replace")
        )
    return parse_xml_resultsets(result.stdout, path)


def parse_datetime(value: str) -> datetime | None:
    for date_format in DATETIME_FORMATS:
        try:
            return datetime.strptime(value, date_format)
        except ValueError:
            pass
    return None


def embedded_datetimes_match(production: str, official: str) -> bool:
    production_values = EMBEDDED_DATETIME.findall(production)
    official_values = EMBEDDED_DATETIME.findall(official)
    if not production_values or len(production_values) != len(official_values):
        return False
    if EMBEDDED_DATETIME.sub("<datetime>", production) != EMBEDDED_DATETIME.sub(
        "<datetime>", official
    ):
        return False
    return all(
        (production_time := parse_datetime(production_value)) is not None
        and (official_time := parse_datetime(official_value)) is not None
        and abs((production_time - official_time).total_seconds()) <= 600
        for production_value, official_value in zip(
            production_values, official_values, strict=True
        )
    )


def without_analyze_timings(value):
    if isinstance(value, dict):
        return {
            key: without_analyze_timings(nested)
            for key, nested in value.items()
            if not key.lower().endswith("_time_ms")
        }
    if isinstance(value, list):
        return [without_analyze_timings(nested) for nested in value]
    return value


def normalize_unordered_customer_orders(column: str, value):
    if (
        column.lower() == "customer_doc"
        and isinstance(value, dict)
        and isinstance(value.get("latest_orders"), list)
    ):
        value = dict(value)
        value["latest_orders"] = sorted(
            value["latest_orders"],
            key=lambda item: json.dumps(item, sort_keys=True, separators=(",", ":")),
        )
    return value


def innodb_status_shape(value: str) -> str:
    value = EMBEDDED_DATETIME.sub("<datetime>", value)
    value = re.sub(r"0x[0-9A-Fa-f]+", "<hex>", value)
    value = re.sub(
        r"(LIST OF TRANSACTIONS FOR EACH SESSION:\n).*?(\n--------\nFILE I/O)",
        r"\1<session-transactions>\2",
        value,
        flags=re.DOTALL,
    )
    return re.sub(r"(?<![A-Za-z_])[-+]?\d+(?:\.\d+)?", "<number>", value)


def active_processlist_rows(
    columns: list[str], rows: list[list[str]]
) -> list[list[str]]:
    indexes = {column.upper(): index for index, column in enumerate(columns)}
    command_index = indexes.get("COMMAND")
    info_index = indexes.get("INFO")
    if command_index is None or info_index is None:
        return rows
    active = [
        list(row)
        for row in rows
        if row[command_index].upper() == "QUERY"
        and row[info_index].strip().upper().startswith("SHOW PROCESSLIST")
    ]
    for row in active:
        for name in ["ID", "HOST", "TIME", "STATE", "PROGRESS"]:
            if (index := indexes.get(name)) is not None:
                row[index] = "<runtime>"
    return active


def cells_match(sql: str, column: str, production: str, official: str) -> bool:
    if production == official:
        return True
    if re.match(r"\s*SHOW\s+CREATE\s+USER\b", sql, re.IGNORECASE):
        verifier = re.compile(
            r"( IDENTIFIED WITH '[^']+' AS ').*?(' REQUIRE )", re.DOTALL
        )
        if verifier.sub(r"\1<password-verifier>\2", production) == verifier.sub(
            r"\1<password-verifier>\2", official
        ):
            return True
    if (
        re.match(r"\s*EXPLAIN\s+ANALYZE\b", sql, re.IGNORECASE)
        and column.upper() == "EXPLAIN"
        and re.sub(r"actual time=[^ )]+", "actual time=<runtime>", production)
        == re.sub(r"actual time=[^ )]+", "actual time=<runtime>", official)
    ):
        return True
    if encode_xml_forbidden_controls(production) == official:
        return True
    if "\x00" in production and encode_xml_forbidden_controls(
        production.replace("\x00", " ")
    ) == official:
        return True
    if (
        re.search(
            r"\bSHOW\s+GLOBAL\s+STATUS\s+LIKE\s+['\"]UPTIME['\"]",
            sql,
            re.IGNORECASE,
        )
        and column.upper() == "VALUE"
    ):
        try:
            return abs(Decimal(production) - Decimal(official)) <= 600
        except InvalidOperation:
            return False
    if (
        re.match(r"\s*CHECKSUM\s+TABLE\b", sql, re.IGNORECASE)
        and column.upper() == "CHECKSUM"
    ):
        return production.isdigit() and official.isdigit()
    if "UUID" in column.upper():
        try:
            production_uuid = uuid.UUID(production)
            official_uuid = uuid.UUID(official)
            if production_uuid.version == official_uuid.version:
                return True
        except ValueError:
            pass
    if (
        column.upper() == "STATUS"
        and "INNODB MONITOR OUTPUT" in production
        and "INNODB MONITOR OUTPUT" in official
        and innodb_status_shape(production) == innodb_status_shape(official)
    ):
        return True
    if re.fullmatch(r"0x(?:[0-9A-Fa-f]{2})*", official):
        decoded = bytes.fromhex(official[2:]).decode("utf-8", errors="replace")
        if production == decoded:
            return True
    try:
        production_json = json.loads(production)
        official_json = json.loads(official)
        production_json = normalize_unordered_customer_orders(column, production_json)
        official_json = normalize_unordered_customer_orders(column, official_json)
        if column.upper() == "ANALYZE":
            production_json = without_analyze_timings(production_json)
            official_json = without_analyze_timings(official_json)
        if production_json == official_json:
            return True
    except (json.JSONDecodeError, TypeError):
        pass
    production_time = parse_datetime(production)
    official_time = parse_datetime(official)
    if production_time is not None and official_time is not None:
        return abs((production_time - official_time).total_seconds()) <= 600
    if embedded_datetimes_match(production, official):
        return True
    try:
        return Decimal(production) == Decimal(official)
    except InvalidOperation:
        return False


def compare(
    db_type: str,
    path: Path,
    production: dict,
    official: list[dict[str, list]],
) -> tuple[int, int]:
    production_grids = production["grids"]
    if len(production_grids) != len(official):
        raise AssertionError(
            f"{path}: grid count differs: production={len(production_grids)}, "
            f"{config_client_name(db_type)}={len(official)}"
        )
    cell_count = 0
    for grid_index, (production_grid, official_grid) in enumerate(
        zip(production_grids, official, strict=True)
    ):
        official_columns = official_grid["columns"]
        if official_columns is None:
            if production_grid["rows"]:
                raise AssertionError(
                    f"{path}: grid #{grid_index} is empty in "
                    f"{config_client_name(db_type)} but production has rows"
                )
        elif production_grid["columns"] != official_columns:
            raise AssertionError(
                f"{path}: grid #{grid_index} columns differ:\n"
                f"production={production_grid['columns']!r}\n"
                f"{config_client_name(db_type)}={official_columns!r}"
            )
        production_rows = production_grid["rows"]
        official_rows = official_grid["rows"]
        if production_grid["sql"].strip().upper().startswith("SHOW PROCESSLIST"):
            production_rows = active_processlist_rows(
                production_grid["columns"], production_rows
            )
            official_rows = active_processlist_rows(
                production_grid["columns"], official_rows
            )
        if re.match(
            r"\s*SHOW\s+OPEN\s+TABLES\b", production_grid["sql"], re.IGNORECASE
        ):
            production_rows = sorted(production_rows)
            official_rows = sorted(official_rows)
        if len(production_rows) != len(official_rows):
            raise AssertionError(
                f"{path}: grid #{grid_index} row count differs: "
                f"production={len(production_rows)}, "
                f"{config_client_name(db_type)}={len(official_rows)}"
            )
        for row_index, (production_row, official_row) in enumerate(
            zip(production_rows, official_rows, strict=True)
        ):
            if len(production_row) != len(official_row):
                raise AssertionError(
                    f"{path}: grid #{grid_index} row #{row_index} cell count "
                    f"differs: production={len(production_row)}, "
                    f"{config_client_name(db_type)}={len(official_row)}"
                )
            for column_index, (production_cell, official_cell) in enumerate(
                zip(production_row, official_row, strict=True)
            ):
                column = production_grid["columns"][column_index]
                if not cells_match(
                    production_grid["sql"], column, production_cell, official_cell
                ):
                    raise AssertionError(
                        f"{path}: grid #{grid_index}, row #{row_index}, "
                        f"column {column!r} differs: "
                        f"production={production_cell!r}, "
                        f"{config_client_name(db_type)}={official_cell!r}"
                    )
                cell_count += 1
    return len(production_grids), cell_count


def config_client_name(db_type: str) -> str:
    return DATABASES[db_type]["client"]


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--db-type", choices=sorted(DATABASES), required=True)
    parser.add_argument("paths", nargs="*", type=Path)
    args = parser.parse_args()
    config = DATABASES[args.db_type]
    paths = args.paths or fixture_paths(config["fixture_dir"])
    if not SNAPSHOT_BIN.is_file():
        raise RuntimeError(
            f"{SNAPSHOT_BIN} does not exist; build mysql_fixture_snapshot first"
        )

    total_grids = 0
    total_cells = 0
    for path in paths:
        path = path if path.is_absolute() else REPO_ROOT / path
        production = production_snapshot(args.db_type, config, path)
        official = official_snapshot(config, path)
        grid_count, cell_count = compare(args.db_type, path, production, official)
        total_grids += grid_count
        total_cells += cell_count
        print(
            f"PASS {path.relative_to(REPO_ROOT)}: "
            f"{grid_count} grid(s), {cell_count} cell(s)"
        )
    print(
        f"PASS {args.db_type} production/{config['client']}: "
        f"{len(paths)} file(s), {total_grids} grid(s), {total_cells} cell(s)"
    )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())

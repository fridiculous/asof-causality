#!/usr/bin/env -S uv run --script
# /// script
# requires-python = ">=3.11"
# dependencies = []
# ///
"""Rebuild the checked-in ALFRED PAYEMS revision fixtures.

The minimal fixture proves one real ALFRED correction where the January 2019
PAYEMS observation differs between the 2020-02-01 and 2020-03-01 vintages. The
large fixture expands that into monthly 2020-2021 vintages so sensitivity runs
have enough corrections to produce meaningful bucketed output. The script uses
only Python's standard library so reviewers can regenerate the fixtures without
API keys or project-specific dependencies.
"""

from __future__ import annotations

import argparse
import csv
import difflib
import io
import os
import shutil
import ssl
import subprocess
import sys
import time
from dataclasses import dataclass
from datetime import date, timedelta
from pathlib import Path
from urllib.error import URLError
from urllib.request import Request, urlopen


ROOT = Path(__file__).resolve().parents[1]
DEFAULT_MINIMAL_FIXTURE = ROOT / "examples" / "alfred-payems-revision.pipe"
DEFAULT_LARGE_FIXTURE = ROOT / "examples" / "alfred-payems-revisions-2020.pipe"

PAYEMS_ALFRED_URL = (
    "https://alfred.stlouisfed.org/graph/alfredgraph.csv"
    "?id=PAYEMS&cosd={start_date}&coed={end_date}"
    "&vintage_date={vintage_date}&revision_date={vintage_date}"
)

OBSERVATION_DATE = "2019-01-01"
INITIAL_VINTAGE_DATE = "2020-02-01"
REVISED_VINTAGE_DATE = "2020-03-01"
LARGE_OBSERVATION_START = "2019-01-01"
LARGE_OBSERVATION_END = "2021-12-01"
LARGE_VINTAGE_START = "2020-02-01"
LARGE_VINTAGE_END = "2021-12-01"

HEADER = """# Real-data fixture derived from public ALFRED PAYEMS vintages.
# Feature: PAYEMS payroll level from ALFRED.
# Correction target: PAYEMS observation 2019-01-01 changes from the
# 2020-02-01 vintage to the 2020-03-01 vintage.
# Times use YYYYMMDDHHMM integers. PAYEMS observations are dated at 15:00 on
# the observation date and received at 09:00 on the ALFRED vintage date.
# This fixture demonstrates a real correction, not just next-vintage lateness.
# event_id|observed_time|received_time|sequence|role|symbol|payload
"""

LARGE_HEADER = """# Larger real-data fixture derived from public ALFRED PAYEMS vintages.
# Feature: PAYEMS payroll level from monthly ALFRED vintages.
# Observation window: 2019-01-01 through 2021-12-01.
# Vintage window: 2020-02-01 through 2021-12-01.
# Rows include first-seen observations as feature events and changed values as
# feature_correction events. Predictions are emitted mid-month between vintages.
# Times use YYYYMMDDHHMM integers. PAYEMS observations are dated at 15:00 on
# the observation date and received at 09:00 on the ALFRED vintage date.
# event_id|observed_time|received_time|sequence|role|symbol|payload
"""


@dataclass(frozen=True)
class Revision:
    observation_date: str
    initial_vintage_date: str
    initial_value: str
    revised_vintage_date: str
    revised_value: str


@dataclass(frozen=True)
class PayemsChange:
    observation_date: str
    vintage_date: str
    value: str
    previous_value: str | None


def main() -> int:
    parser = argparse.ArgumentParser(
        description="Rebuild the ALFRED PAYEMS real-revision example pipe."
    )
    parser.add_argument(
        "--variant",
        choices=("minimal", "large"),
        default="minimal",
        help="fixture variant to build",
    )
    parser.add_argument(
        "--out",
        type=Path,
        help="write generated pipe output to this path instead of stdout",
    )
    parser.add_argument(
        "--check",
        action="store_true",
        help="compare generated output with the checked-in fixture",
    )
    parser.add_argument(
        "--fixture",
        type=Path,
        help="fixture path used by --check",
    )

    args = parser.parse_args()
    generated = build_pipe(args.variant)
    fixture = args.fixture or default_fixture(args.variant)

    if args.check:
        expected = fixture.read_text(encoding="utf-8")
        if generated != expected:
            diff = difflib.unified_diff(
                expected.splitlines(keepends=True),
                generated.splitlines(keepends=True),
                fromfile=str(fixture),
                tofile="regenerated-from-fred-alfred-payems",
            )
            sys.stderr.writelines(diff)
            return 1
        print(f"ok: {fixture} matches public ALFRED source data")
        return 0

    if args.out:
        args.out.write_text(generated, encoding="utf-8")
        print(f"wrote {args.out}")
    else:
        print(generated, end="")

    return 0


def default_fixture(variant: str) -> Path:
    if variant == "large":
        return DEFAULT_LARGE_FIXTURE
    return DEFAULT_MINIMAL_FIXTURE


def build_pipe(variant: str) -> str:
    if variant == "large":
        return build_large_pipe()
    return build_minimal_pipe()


def build_minimal_pipe() -> str:
    revision = find_revision()
    rows = [
        format_initial_feature(revision, sequence=1),
        format_prediction("p_after_initial_before_revision", "2020-02-14", sequence=2),
        format_feature_correction(revision, sequence=3),
        format_prediction("p_after_revision", "2020-03-16", sequence=4),
    ]
    return HEADER + "\n".join(rows) + "\n"


def build_large_pipe() -> str:
    seen: dict[str, str] = {}
    rows: list[str] = []
    sequence = 1

    for vintage_date in month_starts(LARGE_VINTAGE_START, LARGE_VINTAGE_END):
        changes = payems_changes_for_vintage(vintage_date, seen)
        for change in changes:
            rows.append(format_payems_change(change, sequence))
            sequence += 1

        prediction_date = add_days(vintage_date, 14)
        rows.append(
            format_prediction(
                f"p_payems_midmonth_{compact_date(prediction_date)}",
                prediction_date,
                sequence,
            )
        )
        sequence += 1

    return LARGE_HEADER + "\n".join(rows) + "\n"


def find_revision() -> Revision:
    initial_value = fetch_payems_value(INITIAL_VINTAGE_DATE, OBSERVATION_DATE)
    revised_value = fetch_payems_value(REVISED_VINTAGE_DATE, OBSERVATION_DATE)
    if initial_value == revised_value:
        raise ValueError(
            "expected PAYEMS revision did not appear: "
            f"{INITIAL_VINTAGE_DATE} and {REVISED_VINTAGE_DATE} both returned {initial_value}"
        )

    return Revision(
        observation_date=OBSERVATION_DATE,
        initial_vintage_date=INITIAL_VINTAGE_DATE,
        initial_value=initial_value,
        revised_vintage_date=REVISED_VINTAGE_DATE,
        revised_value=revised_value,
    )


def fetch_payems_value(vintage_date: str, observation_date: str) -> str:
    snapshot = fetch_payems_snapshot(vintage_date, "2019-01-01", "2019-03-01")
    if observation_date in snapshot:
        return snapshot[observation_date]
    raise ValueError(f"PAYEMS {observation_date} not found in vintage {vintage_date}")


def payems_changes_for_vintage(
    vintage_date: str,
    seen: dict[str, str],
) -> list[PayemsChange]:
    snapshot = fetch_payems_snapshot(
        vintage_date,
        LARGE_OBSERVATION_START,
        LARGE_OBSERVATION_END,
    )
    changes = []
    for observation_date, value in sorted(snapshot.items()):
        previous_value = seen.get(observation_date)
        if previous_value != value:
            changes.append(
                PayemsChange(
                    observation_date=observation_date,
                    vintage_date=vintage_date,
                    value=value,
                    previous_value=previous_value,
                )
            )
            seen[observation_date] = value
    return changes


def fetch_payems_snapshot(
    vintage_date: str,
    start_date: str,
    end_date: str,
) -> dict[str, str]:
    rows = fetch_csv(
        PAYEMS_ALFRED_URL.format(
            start_date=start_date,
            end_date=end_date,
            vintage_date=vintage_date,
        )
    )
    value_column = single_value_column(rows, "observation_date")
    vintage = parse_date(vintage_date)
    return {
        row["observation_date"]: row[value_column]
        for row in rows
        if row[value_column] != "."
        and parse_date(row["observation_date"]) < vintage
    }


def fetch_csv(url: str) -> list[dict[str, str]]:
    body = fetch_url(url)
    return list(csv.DictReader(io.StringIO(body)))


def fetch_url(url: str) -> str:
    if shutil.which("curl"):
        return fetch_url_with_curl(url)
    return fetch_url_with_urllib(url)


def fetch_url_with_curl(url: str) -> str:
    result = subprocess.run(
        [
            "curl",
            "--fail",
            "--http1.1",
            "--location",
            "--silent",
            "--show-error",
            "--max-time",
            "30",
            "--retry",
            "3",
            "--retry-all-errors",
            "--retry-delay",
            "1",
            url,
        ],
        check=False,
        capture_output=True,
        text=True,
    )
    if result.returncode != 0:
        raise RuntimeError(f"curl failed for {url}: {result.stderr.strip()}")
    return result.stdout


def fetch_url_with_urllib(url: str) -> str:
    request = Request(url, headers={"User-Agent": "asof-causality-repro/1.0"})
    last_error: BaseException | None = None

    for attempt in range(1, 4):
        try:
            with urlopen(request, timeout=90, context=ssl_context()) as response:
                return response.read().decode("utf-8-sig")
        except (TimeoutError, URLError) as error:
            last_error = error
            if attempt == 3:
                break
            time.sleep(attempt)

    raise RuntimeError(f"failed to fetch {request.full_url}") from last_error


def ssl_context() -> ssl.SSLContext:
    cafile = ca_bundle_path()
    if cafile:
        return ssl.create_default_context(cafile=cafile)
    return ssl.create_default_context()


def ca_bundle_path() -> str | None:
    for key in ("SSL_CERT_FILE", "REQUESTS_CA_BUNDLE"):
        value = os.environ.get(key)
        if value and Path(value).is_file():
            return value

    for candidate in (
        "/etc/ssl/cert.pem",
        "/private/etc/ssl/cert.pem",
        "/opt/homebrew/etc/ca-certificates/cert.pem",
    ):
        if Path(candidate).is_file():
            return candidate

    return None


def single_value_column(rows: list[dict[str, str]], date_column: str) -> str:
    if not rows:
        raise ValueError("CSV response contained no rows")
    columns = [column for column in rows[0] if column != date_column]
    if len(columns) != 1:
        raise ValueError(f"expected one value column, found {columns}")
    return columns[0]


def format_initial_feature(revision: Revision, sequence: int) -> str:
    event_id = (
        f"payems_{compact_date(revision.observation_date)}_"
        f"v{compact_date(revision.initial_vintage_date)}"
    )
    observed_time = f"{compact_date(revision.observation_date)}1500"
    received_time = f"{compact_date(revision.initial_vintage_date)}0900"
    payload = (
        f"score={revision.initial_value},series=PAYEMS,value={revision.initial_value},"
        f"vintage={revision.initial_vintage_date},source=ALFRED"
    )
    return f"{event_id}|{observed_time}|{received_time}|{sequence}|feature|PAYEMS|{payload}"


def format_feature_correction(revision: Revision, sequence: int) -> str:
    event_id = (
        f"payems_{compact_date(revision.observation_date)}_"
        f"v{compact_date(revision.revised_vintage_date)}_revision"
    )
    observed_time = f"{compact_date(revision.observation_date)}1500"
    received_time = f"{compact_date(revision.revised_vintage_date)}0900"
    payload = (
        f"score={revision.revised_value},series=PAYEMS,value={revision.revised_value},"
        f"previous_vintage_value={revision.initial_value},"
        f"vintage={revision.revised_vintage_date},source=ALFRED"
    )
    return (
        f"{event_id}|{observed_time}|{received_time}|"
        f"{sequence}|feature_correction|PAYEMS|{payload}"
    )


def format_payems_change(change: PayemsChange, sequence: int) -> str:
    observed_time = f"{compact_date(change.observation_date)}1500"
    received_time = f"{compact_date(change.vintage_date)}0900"
    if change.previous_value is None:
        event_id = (
            f"payems_{compact_date(change.observation_date)}_"
            f"v{compact_date(change.vintage_date)}"
        )
        payload = (
            f"score={change.value},series=PAYEMS,value={change.value},"
            f"vintage={change.vintage_date},source=ALFRED"
        )
        return (
            f"{event_id}|{observed_time}|{received_time}|"
            f"{sequence}|feature|PAYEMS|{payload}"
        )

    event_id = (
        f"payems_{compact_date(change.observation_date)}_"
        f"v{compact_date(change.vintage_date)}_revision"
    )
    payload = (
        f"score={change.value},series=PAYEMS,value={change.value},"
        f"previous_vintage_value={change.previous_value},"
        f"vintage={change.vintage_date},source=ALFRED"
    )
    return (
        f"{event_id}|{observed_time}|{received_time}|"
        f"{sequence}|feature_correction|PAYEMS|{payload}"
    )


def format_prediction(event_id: str, prediction_date: str, sequence: int) -> str:
    prediction_time = f"{compact_date(prediction_date)}1600"
    return (
        f"{event_id}|{prediction_time}|{prediction_time}|"
        f"{sequence}|prediction|PAYEMS|"
    )


def compact_date(value: str) -> str:
    return value.replace("-", "")


def parse_date(value: str) -> date:
    year, month, day = value.split("-")
    return date(int(year), int(month), int(day))


def add_days(value: str, days: int) -> str:
    return (parse_date(value) + timedelta(days=days)).isoformat()


def month_starts(start: str, end: str) -> list[str]:
    current = parse_date(start)
    final = parse_date(end)
    values = []
    while current <= final:
        values.append(current.isoformat())
        year = current.year + (1 if current.month == 12 else 0)
        month = 1 if current.month == 12 else current.month + 1
        current = date(year, month, 1)
    return values


if __name__ == "__main__":
    raise SystemExit(main())

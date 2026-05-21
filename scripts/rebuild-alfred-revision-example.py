#!/usr/bin/env -S uv run --script
# /// script
# requires-python = ">=3.11"
# dependencies = []
# ///
"""Rebuild the checked-in ALFRED PAYEMS revision fixture.

This fixture is intentionally small: it proves one real ALFRED correction where
the January 2019 PAYEMS observation differs between the 2020-02-01 and
2020-03-01 vintages. It uses only Python's standard library so reviewers can
regenerate the fixture without API keys or project-specific dependencies.
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
from pathlib import Path
from urllib.error import URLError
from urllib.request import Request, urlopen


ROOT = Path(__file__).resolve().parents[1]
DEFAULT_FIXTURE = ROOT / "examples" / "alfred-payems-revision.pipe"

PAYEMS_ALFRED_URL = (
    "https://alfred.stlouisfed.org/graph/alfredgraph.csv"
    "?id=PAYEMS&cosd=2019-01-01&coed=2019-03-01"
    "&vintage_date={vintage_date}&revision_date={vintage_date}"
)

OBSERVATION_DATE = "2019-01-01"
INITIAL_VINTAGE_DATE = "2020-02-01"
REVISED_VINTAGE_DATE = "2020-03-01"

HEADER = """# Real-data fixture derived from public ALFRED PAYEMS vintages.
# Feature: PAYEMS payroll level from ALFRED.
# Correction target: PAYEMS observation 2019-01-01 changes from the
# 2020-02-01 vintage to the 2020-03-01 vintage.
# Times use YYYYMMDDHHMM integers. PAYEMS observations are dated at 15:00 on
# the observation date and received at 09:00 on the ALFRED vintage date.
# This fixture demonstrates a real correction, not just next-vintage lateness.
# event_id|observed_time|received_time|sequence|role|symbol|payload
"""


@dataclass(frozen=True)
class Revision:
    observation_date: str
    initial_vintage_date: str
    initial_value: str
    revised_vintage_date: str
    revised_value: str


def main() -> int:
    parser = argparse.ArgumentParser(
        description="Rebuild the ALFRED PAYEMS real-revision example pipe."
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
        default=DEFAULT_FIXTURE,
        help="fixture path used by --check",
    )

    args = parser.parse_args()
    generated = build_pipe()

    if args.check:
        expected = args.fixture.read_text(encoding="utf-8")
        if generated != expected:
            diff = difflib.unified_diff(
                expected.splitlines(keepends=True),
                generated.splitlines(keepends=True),
                fromfile=str(args.fixture),
                tofile="regenerated-from-fred-alfred-payems",
            )
            sys.stderr.writelines(diff)
            return 1
        print(f"ok: {args.fixture} matches public ALFRED source data")
        return 0

    if args.out:
        args.out.write_text(generated, encoding="utf-8")
        print(f"wrote {args.out}")
    else:
        print(generated, end="")

    return 0


def build_pipe() -> str:
    revision = find_revision()
    rows = [
        format_initial_feature(revision, sequence=1),
        format_prediction("p_after_initial_before_revision", "2020-02-14", sequence=2),
        format_feature_correction(revision, sequence=3),
        format_prediction("p_after_revision", "2020-03-16", sequence=4),
    ]
    return HEADER + "\n".join(rows) + "\n"


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
    rows = fetch_csv(PAYEMS_ALFRED_URL.format(vintage_date=vintage_date))
    value_column = single_value_column(rows, "observation_date")
    for row in rows:
        if row["observation_date"] == observation_date:
            value = row[value_column]
            if value == ".":
                raise ValueError(
                    f"PAYEMS {observation_date} missing in vintage {vintage_date}"
                )
            return value
    raise ValueError(f"PAYEMS {observation_date} not found in vintage {vintage_date}")


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


def format_prediction(event_id: str, prediction_date: str, sequence: int) -> str:
    prediction_time = f"{compact_date(prediction_date)}1600"
    return (
        f"{event_id}|{prediction_time}|{prediction_time}|"
        f"{sequence}|prediction|PAYEMS|"
    )


def compact_date(value: str) -> str:
    return value.replace("-", "")


if __name__ == "__main__":
    raise SystemExit(main())

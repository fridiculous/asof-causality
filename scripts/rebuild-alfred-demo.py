#!/usr/bin/env python3
"""Rebuild the checked-in ALFRED/FRED real-data causality fixture.

This script intentionally uses only the Python standard library so a reviewer
can regenerate the fixture without project-specific dependencies or API keys.
"""

from __future__ import annotations

import argparse
import csv
import difflib
import io
import os
import ssl
import sys
from dataclasses import dataclass
from datetime import date
from decimal import Decimal, ROUND_HALF_UP
from pathlib import Path
from urllib.request import Request, urlopen


ROOT = Path(__file__).resolve().parents[1]
DEFAULT_FIXTURE = ROOT / "examples" / "alfred-dgs10-sp500.pipe"

DGS10_ALFRED_URL = (
    "https://alfred.stlouisfed.org/graph/alfredgraph.csv"
    "?id=DGS10&cosd=2020-03-10&coed=2020-03-20"
    "&vintage_date={vintage_date}&revision_date={vintage_date}"
)
SP500_FRED_URL = (
    "https://fred.stlouisfed.org/graph/fredgraph.csv"
    "?id=SP500&cosd=2020-03-16&coed=2020-03-20"
)

DGS10_VINTAGE_DATES = (
    "2020-03-12",
    "2020-03-13",
    "2020-03-16",
    "2020-03-17",
    "2020-03-18",
    "2020-03-19",
    "2020-03-20",
)
SP500_PREDICTION_DATES = (
    "2020-03-16",
    "2020-03-17",
    "2020-03-18",
    "2020-03-19",
)

HEADER = """# Real-data fixture derived from public ALFRED/FRED daily data.
# Feature: DGS10 daily yield change from ALFRED vintages.
# Prediction: daily SP500 risk-on/risk-off stance before the next DGS10 vintage is available.
# Outcome: next trading-day SP500 return from FRED.
# Times use YYYYMMDDHHMM integers. DGS10 observations are dated at 15:00 on
# the observation date, before a naive 16:00 close prediction, and received at
# 09:00 on the next ALFRED vintage date.
# SP500 predictions are emitted at 16:00. Outcomes are attached after the next
# close so they cannot affect prediction state.
# event_id|observed_time|received_time|sequence|role|symbol|payload
"""


@dataclass(frozen=True)
class FeatureRow:
    observation_date: str
    vintage_date: str
    value: Decimal
    previous: Decimal


@dataclass(frozen=True)
class PredictionRow:
    prediction_date: str
    sequence: int


def main() -> int:
    parser = argparse.ArgumentParser(
        description="Rebuild examples/alfred-dgs10-sp500.pipe from public FRED/ALFRED CSVs."
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
                tofile="regenerated-from-fred-alfred",
            )
            sys.stderr.writelines(diff)
            return 1
        print(f"ok: {args.fixture} matches public FRED/ALFRED source data")
        return 0

    if args.out:
        args.out.write_text(generated, encoding="utf-8")
        print(f"wrote {args.out}")
    else:
        print(generated, end="")

    return 0


def build_pipe() -> str:
    features = [latest_dgs10_feature(vintage_date) for vintage_date in DGS10_VINTAGE_DATES]
    sp500 = fetch_sp500_closes()
    rows: list[str] = []
    prediction_rows: dict[str, PredictionRow] = {}
    sequence = 1

    for feature in features:
        while (
            len(prediction_rows) < len(SP500_PREDICTION_DATES)
            and next_prediction_date(prediction_rows) < feature.vintage_date
        ):
            prediction_date = next_prediction_date(prediction_rows)
            prediction = PredictionRow(prediction_date=prediction_date, sequence=sequence)
            prediction_rows[prediction_date] = prediction
            rows.append(format_prediction(prediction))
            sequence += 1

        rows.append(format_feature(feature, sequence))
        sequence += 1

    while len(prediction_rows) < len(SP500_PREDICTION_DATES):
        prediction_date = next_prediction_date(prediction_rows)
        prediction = PredictionRow(prediction_date=prediction_date, sequence=sequence)
        prediction_rows[prediction_date] = prediction
        rows.append(format_prediction(prediction))
        sequence += 1

    for prediction_date in SP500_PREDICTION_DATES:
        prediction = prediction_rows[prediction_date]
        outcome_date = next_sp500_date(sp500, prediction_date)
        rows.append(format_outcome(prediction, outcome_date, sp500, sequence))
        sequence += 1

    return HEADER + "\n".join(rows) + "\n"


def latest_dgs10_feature(vintage_date: str) -> FeatureRow:
    rows = fetch_csv(DGS10_ALFRED_URL.format(vintage_date=vintage_date))
    value_column = single_value_column(rows, "observation_date")
    vintage = parse_date(vintage_date)
    observations = [
        (row["observation_date"], parse_decimal(row[value_column]))
        for row in rows
        if row[value_column] != "." and parse_date(row["observation_date"]) < vintage
    ]

    if len(observations) < 2:
        raise ValueError(f"not enough DGS10 observations before vintage {vintage_date}")

    observation_date, value = observations[-1]
    _, previous = observations[-2]
    return FeatureRow(
        observation_date=observation_date,
        vintage_date=vintage_date,
        value=value,
        previous=previous,
    )


def fetch_sp500_closes() -> dict[str, Decimal]:
    rows = fetch_csv(SP500_FRED_URL)
    return {
        row["observation_date"]: parse_decimal(row["SP500"])
        for row in rows
        if row["SP500"] != "."
    }


def fetch_csv(url: str) -> list[dict[str, str]]:
    request = Request(url, headers={"User-Agent": "asof-causality-repro/1.0"})
    with urlopen(request, timeout=30, context=ssl_context()) as response:
        body = response.read().decode("utf-8-sig")
    return list(csv.DictReader(io.StringIO(body)))


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


def next_prediction_date(prediction_rows: dict[str, PredictionRow]) -> str:
    return SP500_PREDICTION_DATES[len(prediction_rows)]


def next_sp500_date(sp500: dict[str, Decimal], prediction_date: str) -> str:
    later_dates = [sp500_date for sp500_date in sorted(sp500) if sp500_date > prediction_date]
    if not later_dates:
        raise ValueError(f"missing SP500 outcome date after {prediction_date}")
    return later_dates[0]


def format_feature(feature: FeatureRow, sequence: int) -> str:
    event_id = f"dgs10_{compact_date(feature.observation_date)}_v{compact_date(feature.vintage_date)}"
    observed_time = f"{compact_date(feature.observation_date)}1500"
    received_time = f"{compact_date(feature.vintage_date)}0900"
    score = feature.value - feature.previous
    payload = (
        f"score={format_decimal(score)},series=DGS10,"
        f"value={format_decimal(feature.value)},previous={format_decimal(feature.previous)},"
        "source=ALFRED"
    )
    return f"{event_id}|{observed_time}|{received_time}|{sequence}|feature|SP500|{payload}"


def format_prediction(prediction: PredictionRow) -> str:
    suffix = "_before_vintage" if prediction.prediction_date == "2020-03-18" else ""
    event_id = f"p_{compact_date(prediction.prediction_date)}_close{suffix}"
    prediction_time = f"{compact_date(prediction.prediction_date)}1600"
    return (
        f"{event_id}|{prediction_time}|{prediction_time}|"
        f"{prediction.sequence}|prediction|SP500|"
    )


def format_outcome(
    prediction: PredictionRow,
    outcome_date: str,
    sp500: dict[str, Decimal],
    sequence: int,
) -> str:
    event_id = f"sp500_outcome_{compact_date(outcome_date)}"
    observed_time = f"{compact_date(outcome_date)}1600"
    received_time = f"{compact_date(outcome_date)}1700"
    return_ratio = sp500[outcome_date] / sp500[prediction.prediction_date]
    return_bps = (return_ratio - Decimal("1")) * Decimal("10000")
    prediction_id = f"p_{compact_date(prediction.prediction_date)}_close"
    if prediction.prediction_date == "2020-03-18":
        prediction_id += "_before_vintage"
    prediction_key = (
        f"{compact_date(prediction.prediction_date)}1600:"
        f"{prediction.sequence}:{prediction_id}"
    )
    payload = f"return_bps={format_bps(return_bps)},prediction_replay_key={prediction_key}"
    return f"{event_id}|{observed_time}|{received_time}|{sequence}|outcome|SP500|{payload}"


def parse_date(value: str) -> date:
    year, month, day = value.split("-")
    return date(int(year), int(month), int(day))


def compact_date(value: str) -> str:
    return value.replace("-", "")


def parse_decimal(value: str) -> Decimal:
    return Decimal(value.strip())


def format_decimal(value: Decimal) -> str:
    rounded = value.quantize(Decimal("0.01"), rounding=ROUND_HALF_UP)
    return format(rounded.normalize(), "f")


def format_bps(value: Decimal) -> str:
    return format(value.quantize(Decimal("0.01"), rounding=ROUND_HALF_UP), "f")


if __name__ == "__main__":
    raise SystemExit(main())

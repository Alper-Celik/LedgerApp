# SPDX-FileCopyrightText: 2026 Alper Çelik <alper@alper-celik.dev>
#
# SPDX-License-Identifier: AGPL-3.0-or-later

"""The ledger read API of the assistant's Python sandbox.

Every figure this module answers comes from the app's own read commands: each
function posts a request to the main thread, which runs the same tool executor
the model uses and answers with JSON. The sandbox itself has an in-memory
filesystem only — there is no network, no subprocess, and no way to reach the
ledger file, the disk or the process the app runs in.

The ledger functions are coroutines because the answer arrives from another
thread::

    rows = await ledger.query("SELECT account, sum(position) GROUP BY account")
    for account in await ledger.accounts():
        print(account)

Tables and charts are collected for the transcript: ``ledger.table(name, data)``
adds a table and ``ledger.chart({...})`` adds one chart (the last call in a run
wins). ``print`` writes to the run's stdout, which comes back with the result.

Amounts are exact decimal strings in every answer, and the chart and table
values are written with ``str``/``format`` rather than rounded through a float.
"""

import itertools
import json
from decimal import Decimal

import ledger_rpc

#: The kinds the app's chart wrappers can draw.
KINDS = ("line", "bar", "area", "donut")

#: What one run keeps, so a runaway loop cannot fill the transcript.
MAX_ROWS = 500
MAX_COLUMNS = 40
MAX_CELL = 2000
MAX_TABLES = 20
MAX_SERIES = 12
MAX_POINTS = 500

_tables = []
_chart = None


class LedgerError(RuntimeError):
    """A read the app refused; the message is the app's own sentence."""


async def _call(name, **args):
    """Run one read through the app and return its parsed answer."""
    # The bridge hands back a JS object, so its fields are JsProxy values: read
    # `ok` as a bool and the payload as native text before parsing it. (`str()` of
    # the object itself would be "[object Object]".)
    answer = await ledger_rpc.call(name, json.dumps(args, default=str))
    text = str(answer.text)
    if not bool(answer.ok):
        try:
            message = json.loads(text).get("error", text)
        except (TypeError, ValueError):
            message = text
        raise LedgerError(message or "`%s` failed" % name)
    return json.loads(text)


async def accounts():
    """Every account with its balance as an exact decimal string."""
    return await _call("accounts")


async def account_transactions(account):
    """The transactions of one account, newest first."""
    return await _call("account_transactions", account=account)


async def journal_files():
    """The journal files the loaded ledger includes."""
    return await _call("journal_files")


async def file_text(path):
    """The text of one journal file, by the path `journal_files` gives."""
    return await _call("file_text", path=path)


async def budget_range(start=None, to=None, **kwargs):
    """The envelope budget over a window of consecutive months.

    ``start``/``to`` are months as ``YYYY-MM``; both are optional and default
    to the ledger's newest budget months. ``from_`` is accepted for ``start``
    because ``from`` is a Python keyword.
    """
    if start is None:
        start = kwargs.pop("from_", None)
    if kwargs:
        raise TypeError("budget_range() got unexpected arguments: %s" % ", ".join(kwargs))
    args = {}
    if start is not None:
        args["from"] = start
    if to is not None:
        args["to"] = to
    return await _call("budget_range", **args)


async def index_series():
    """The price index series imported into the app, with their ids."""
    return await _call("index_series")


async def lifestyle_report(months, index):
    """Spending over the newest `months` months, restated with the index series `index`."""
    return await _call("lifestyle_report", months=months, index=index)


async def price_targets():
    """Every commodity the ledger holds or quotes, with its newest price."""
    return await _call("price_targets")


async def query(bql, max_rows=200):
    """Run one BQL statement and return its `columns`, `rows` and `truncated`."""
    return await _call("query", bql=bql, max_rows=max_rows)


async def df(bql, max_rows=200):
    """Run one BQL statement and return the answer as a pandas DataFrame.

    pandas is loaded on demand from the wheel vendored beside the runtime the
    first time it is used, so this works offline; a sandbox that cannot load it
    raises a `RuntimeError`, and `query` and `table` work without it.
    """
    pandas = await _pandas()
    answer = await query(bql, max_rows=max_rows)
    return pandas.DataFrame(answer.get("rows", []), columns=answer.get("columns", []))


async def _pandas():
    """The pandas module, loading the wheel on first use."""
    try:
        import pandas

        return pandas
    except ImportError:
        pass
    try:
        import pyodide_js

        await pyodide_js.loadPackage("pandas")
        import pandas

        return pandas
    except Exception as cause:
        raise RuntimeError(
            "pandas is not available in this sandbox (%s); the wheel is shipped with the "
            "app, so this is a local failure, not a network one. Use ledger.query() and "
            "ledger.table() instead." % cause
        ) from cause


def _text(value):
    """One cell or chart value as the string the app should show."""
    if value is None:
        return ""
    if isinstance(value, Decimal):
        return format(value, "f")
    if isinstance(value, bool):
        return "true" if value else "false"
    if isinstance(value, float):
        return repr(value)
    if isinstance(value, int):
        return str(value)
    if isinstance(value, str):
        return value
    if hasattr(value, "isoformat"):
        return value.isoformat()
    return str(value)


def _clip(text):
    """A cell short enough to keep in a transcript table."""
    return text if len(text) <= MAX_CELL else text[:MAX_CELL] + "…"


def _frame_columns(data):
    return [str(column) for column in data.columns]


def _is_frame(value):
    return hasattr(value, "columns") and hasattr(value, "itertuples") and hasattr(value, "index")


def _rows_of(data, columns):
    """Normalise a table's data into a column list and a list of string rows."""
    if _is_frame(data):
        names = _frame_columns(data)
        rows = [
            [_text(value) for value in row]
            for row in data.itertuples(index=False, name=None)
        ]
        return names, rows

    if isinstance(data, dict):
        names = [str(key) for key in data.keys()]
        rows = [
            [_text(value) for value in row]
            for row in itertools.zip_longest(*data.values(), fillvalue=None)
        ]
        return names, rows

    if isinstance(data, (list, tuple)):
        entries = list(data)
        if entries and isinstance(entries[0], dict):
            keys = []
            for row in entries:
                for key in row.keys():
                    if key not in keys:
                        keys.append(key)
            names = [str(key) for key in keys]
            rows = [[_text(row.get(key)) for key in keys] for row in entries]
            return names, rows
        if entries and isinstance(entries[0], (list, tuple)):
            width = max(len(row) for row in entries)
            names = (
                [str(column) for column in columns]
                if columns
                else ["c%d" % (index + 1) for index in range(width)]
            )
            return names, [[_text(value) for value in row] for row in entries]
        if entries:
            # A flat list is one column: `ledger.table("Months", ["2026-01", ...])`.
            names = [str(columns[0])] if columns else ["value"]
            return names, [[_text(value)] for value in entries]
        names = [str(column) for column in columns] if columns else []
        return names, []

    raise TypeError(
        "table data must be a DataFrame, a list of rows/dicts, or a dict of columns, not %s"
        % type(data).__name__
    )


def table(name=None, data=None, columns=None):
    """Add a table to the transcript.

    ``ledger.table(frame)`` names it ``Table N``; ``ledger.table("Name", frame)``
    names it. ``data`` may be a DataFrame, a list of dicts, a list of rows (with
    ``columns`` when it has none), a flat list (one column), or a dict of
    columns. Every cell is written as text, so a decimal stays exact.
    """
    if data is None and not isinstance(name, str):
        data, name = name, None
    if data is None:
        raise TypeError("table() needs data: a DataFrame, a list of rows/dicts, or a dict of columns")
    if len(_tables) >= MAX_TABLES:
        raise ValueError(
            "a run may add at most %d tables (%d already added)" % (MAX_TABLES, len(_tables))
        )

    names, rows = _rows_of(data, columns)
    truncated = False
    if len(names) > MAX_COLUMNS:
        names = names[:MAX_COLUMNS]
        rows = [row[:MAX_COLUMNS] for row in rows]
        truncated = True
    if len(rows) > MAX_ROWS:
        rows = rows[:MAX_ROWS]
        truncated = True

    kept = []
    for row in rows:
        cells = [_clip(_text(value)) for value in row]
        if len(cells) < len(names):
            cells += [""] * (len(names) - len(cells))
        kept.append(cells[: len(names)])

    if isinstance(name, str) and name.strip():
        label = name.strip()
    else:
        label = "Table %d" % (len(_tables) + 1)
    _tables.append(
        {
            "name": label,
            "columns": names,
            "rows": kept,
            "truncated": truncated,
        }
    )
    return None


#: `ledger.show(...)` is the same call under a second name.
show = table


def _points(points):
    """One series' points, normalised to ``{"label", "value"}`` string pairs."""
    if isinstance(points, dict):
        points = list(points.items())
    if not isinstance(points, (list, tuple)):
        raise TypeError(
            "`points` must be a list of {'label', 'value'} dicts, (label, value) pairs, "
            "or a dict of label: value"
        )
    out = []
    for point in points:
        if isinstance(point, dict):
            label, value = point.get("label"), point.get("value")
        elif isinstance(point, (list, tuple)) and len(point) == 2:
            label, value = point
        else:
            raise TypeError("each point must be a {'label', 'value'} dict or a (label, value) pair")
        out.append({"label": _text(label), "value": _text(value)})
    return out


def chart(spec=None, **fields):
    """Draw one chart in the transcript.

    ``spec`` is a dict::

        ledger.chart({
            "title": "Spending by month",
            "kind": "bar",            # line, bar, area or donut
            "currency": "EUR",
            "series": [
                {"name": "Spent", "points": [("2026-01", "120.00"), ("2026-02", "98.50")]},
            ],
        })

    Values may be Decimal, int, float or strings; they are written as decimal
    strings, with a Decimal's digits kept exactly. A donut draws the first
    series as its slices. Only one chart is kept per run — the last call wins.
    """
    global _chart
    if spec is None:
        spec = {}
    if not isinstance(spec, dict):
        raise TypeError("chart() takes a dict, e.g. ledger.chart({'kind': 'bar', ...})")
    if fields:
        spec = {**spec, **fields}

    kind = str(spec.get("kind", "")).strip().lower()
    if kind not in KINDS:
        raise ValueError("`kind` must be one of %s" % ", ".join(KINDS))

    currency = spec.get("currency")
    if not isinstance(currency, str) or not currency.strip():
        raise ValueError("`currency` must be the ISO code or commodity the amounts are written in")

    raw_series = spec.get("series")
    if isinstance(raw_series, dict):
        raw_series = [raw_series]
    if not isinstance(raw_series, (list, tuple)) or not raw_series:
        raise ValueError("`series` must be a non-empty list of {'name', 'points'} dicts")
    if len(raw_series) > MAX_SERIES:
        raise ValueError("a chart may have at most %d series" % MAX_SERIES)

    series = []
    for index, entry in enumerate(raw_series):
        if not isinstance(entry, dict):
            raise TypeError("each series must be a dict with `name` and `points`")
        name = entry.get("name")
        if not isinstance(name, str) or not name.strip():
            name = "Series %d" % (index + 1)
        points = _points(entry.get("points"))
        if len(points) > MAX_POINTS:
            raise ValueError(
                "a series may have at most %d points; aggregate the data before charting it"
                % MAX_POINTS
            )
        series.append({"name": name.strip(), "points": points})

    drawn = {"kind": kind, "currency": currency.strip(), "series": series}
    title = spec.get("title")
    if isinstance(title, str) and title.strip():
        drawn["title"] = title.strip()

    _chart = drawn
    return None


def _drain():
    """The tables and chart this run collected, as JSON, then reset the store."""
    global _tables, _chart
    payload = {"tables": _tables, "chart": _chart}
    _tables = []
    _chart = None
    return json.dumps(payload)

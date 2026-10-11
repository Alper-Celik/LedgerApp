<!--
SPDX-FileCopyrightText: 2026 Alper Çelik <alper@alper-celik.dev>

SPDX-License-Identifier: AGPL-3.0-or-later
-->

# rustledger-envelope

Envelope budgeting for [rustledger](https://rustledger.github.io), as a WASM
directive plugin.

The plugin mirrors your expense accounts into an `Equity:Envelope:` namespace and
generates the monthly assignment, spending, reconciliation and rollover entries
that make the ledger behave like an envelope budget: every category gets a
`balance`, and money is moved between categories and the pool by the plugin
rather than by hand.

```beancount
plugin "envelope.wasm"

2026-10-01 custom "envelope" "start date" "2026-10"
2026-10-01 custom "envelope" "budget account" "Assets:Cash"
2026-10-01 custom "envelope" "budget account" "Liabilities:TR:.*"
2026-10-01 custom "envelope" "fund" Expenses:Groceries 4000.00 TRY
2026-10-01 custom "envelope" "rollover" Expenses:Groceries TRUE
2026-10-01 custom "envelope" "rollover" Expenses:Fees FALSE
```

## Where everything lives

| Account | Meaning |
| --- | --- |
| `Equity:Envelope:<path>` | the category for the expense account `<path>` (`Expenses:Groceries:Bread` → `Equity:Envelope:Groceries:Bread`) |
| `Equity:Envelope:To-Be-Budgeted` | money not yet assigned, per commodity |
| `Equity:Envelope:Money` | the contra of your on-budget money; it is what makes `To-Be-Budgeted` a real figure |
| `Equity:Envelope:Uncategorized` | postings on the bare expense root (`Expenses 12.00 TRY`) |

Everything the plugin generates is inside that one prefix, so a single string is
enough for any other tool — a report, an app, a query — to ignore the whole
budget. The namespace is net-worth neutral and sums to zero:
`Σ Equity:Envelope:* = 0`.

A category's *balance* is the sum over its subtree, so funding
`Expenses:Groceries` while spending lands on `Expenses:Groceries:Bread` reports a
single number for `Groceries`, and the same number for every parent above it.
The assertions the plugin emits assert that subtree sum too, matching the way
beancount and rustledger check a `balance` (an account plus its subaccounts).

## The month

Each month the plugin generates entries for, in this order, all dated the month's
last day:

| Entry | What it does |
| --- | --- |
| backing | mirrors the change in your on-budget money, which funds `To-Be-Budgeted` |
| funding | the assignments in force (`fund`, inherited until replaced) |
| activity | your spending, mirrored category by category |
| adjustment | `adjust`, `zero` and `pad` operations |
| rollover | the boundary policy (`rollover`) |

`To-Be-Budgeted = on-budget money − Σ category balances`, per commodity; that is
the number to watch, and the plugin warns when it goes negative. Assertions are
dated the **first day of the next month** so they see the whole month, sweep
included, and rustledger's own validation then checks the plugin's arithmetic.

Every generated transaction carries the tag `#envelope` and the metadata
`envelope-kind` (`Envelope funding`, `Envelope activity`, …) and `envelope-month`
(`2026-10`), so a query or a UI can find, group or hide them without parsing
narrations.

## Directives

All keys are `custom "envelope"` directives. Dated: for every key, the newest
directive at or before a month is the one in force for it.

| Key | Arguments | Meaning |
| --- | --- | --- |
| `start date` | `"YYYY-MM"` | first budget month (**required**) |
| `end date` | `"YYYY-MM"` | last budget month; default: `months ahead` past the ledger's newest directive |
| `months ahead` | number | months to budget past the newest directive (default 0) |
| `budget account` | regex | an account holding spendable money (**required**, repeatable) |
| `income account` | regex | read and validated for fava-envelope compatibility; availability comes from on-budget balances |
| `fund` | account, amount | assign this month and every later month until replaced (`0.00` stops) |
| `adjust` | account, amount | one-off signed change for that month only |
| `zero` | account | release the category subtree's whole balance to `To-Be-Budgeted` |
| `pad` | account, amount | set the subtree balance in that commodity to `amount` |
| `pad` | account | beancount's bare `pad`: resolved from the month's `balance` for the same category |
| `balance` | account, amount | assert the subtree balance at month end (emitted as a `balance` directive) |
| `rollover` | account, `TRUE`\|`FALSE` | carry the balance into the next month, or reset it to zero at the boundary |
| `rollover` | account, amount | carry up to the amount, sweeping the excess back to `To-Be-Budgeted` |
| `map` | regex, account | send matching postings to that category instead of their own (first match wins) |
| `include starting balance` | bool | count money held before `start date` as available (default `FALSE`) |
| `assert` | bool | emit a month-end assertion per category and commodity (default `TRUE`) |
| `strict` | bool | warn about spending in a category that was never assigned (default `FALSE`) |

A key this plugin does not implement is a warning, not an error: the same
directives are read by
[fava-envelope](https://github.com/memst/fava-envelope), which has keys of its
own. A value of the wrong kind, a malformed month or a missing argument is an
error, as are two `rollover` declarations on the same category's ancestor or
descendant line, a bare `pad` without a following `balance`, and two bare `pad`s
before one `balance` — the last two mirroring beancount's `E2003` / `E2004`.

## Currencies are never converted

An envelope holds positions per commodity, exactly like a beancount account. A
posting consumes the commodity of its weight — `units × cost`, else
`units × price`, else `units` — so a 150 USD purchase in an AAPL lot consumes USD,
and a TRY envelope never quietly absorbs a USD figure. Buying currency to fund a
foreign-currency envelope is an ordinary on-budget transfer, and if you fund USD
beyond the USD you hold, the warning says so in USD while TRY stays untouched.

## Building

```sh
nix develop                       # provides cargo, rustc and lld
cargo build --target wasm32-unknown-unknown --release
# target/wasm32-unknown-unknown/release/rustledger_envelope.wasm
```

The nix `rustc` ships the `wasm32-unknown-unknown` standard library but no
`rust-lld`, which is why the devShell carries `lld` and `.cargo/config.toml` points
the wasm target at `wasm-ld`.

## Coverage

The nix `rustc` ships no `llvm-tools-preview`, so the LLVM tools come from nixpkgs —
pinned to the LLVM rustc was built against (`rustc -vV | grep LLVM`):

```sh
nix shell nixpkgs#cargo-llvm-cov nixpkgs#llvmPackages_21.llvm -c bash -c '
  export LLVM_COV=$(command -v llvm-cov) LLVM_PROFDATA=$(command -v llvm-profdata)
  cargo llvm-cov --summary-only                 # per-file table
  cargo llvm-cov --text --show-missing-lines    # annotated source, gaps listed
'
```

What the tests are for, in order of what they protect: `model.rs` and `validation_tests.rs`
pin the arithmetic (the weights a posting is worth, the rollover policies, the pool
identity, and the whole emitted ledger passing rustledger's own validation), `config.rs`
pins the directive language including every rejection, and the remaining branches are
error paths — most of the residue is defensive code the parser makes unreachable
(e.g. a `zero` naming a non-expense account) plus `unreachable!()` arms.

## How it runs inside rustledger

The plugin is a directive plugin and rustledger runs it in the **post-booking**
pass, so it sees booked postings with their costs and prices filled in. Late
validation runs after it, so every generated transaction is checked for balance
and currency constraints like any other: a wrong sign shows up as the ledger's own
error rather than as a wrong number in a report. The plugin never modifies what
you wrote — it keeps every input directive and inserts its own.

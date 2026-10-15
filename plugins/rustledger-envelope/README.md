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
plugin "rustledger_envelope.wasm"

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

## Worked example

Two months, one category that carries and one that resets, all in TRY:

```beancount
plugin "rustledger_envelope.wasm"

2026-01-01 custom "envelope" "start date" "2026-01"
2026-01-01 custom "envelope" "end date" "2026-02"
2026-01-01 custom "envelope" "budget account" "^Assets:Bank"

2026-01-01 custom "envelope" "fund" Expenses:Groceries 4000.00 TRY
2026-01-01 custom "envelope" "fund" Expenses:Fees 200.00 TRY
2026-01-01 custom "envelope" "rollover" Expenses:Fees FALSE
```

with five ordinary transactions in your ledger: a 30000.00 TRY salary on
2026-01-02, 2500.00 TRY of groceries on 2026-01-10, a 40.00 TRY bank fee on
2026-01-20, 4600.00 TRY of groceries on 2026-02-06 and another 40.00 TRY fee on
2026-02-20. Nothing else changes in your file.

The plugin opens its own accounts and inserts four transactions per month:

```beancount
2026-01-01 open Equity:Envelope:Fees
2026-01-01 open Equity:Envelope:Groceries
2026-01-01 open Equity:Envelope:Money
2026-01-01 open Equity:Envelope:To-Be-Budgeted

2026-01-31 * "Envelope backing 2026-01" #envelope
  Equity:Envelope:Money                 -27460.00 TRY  ; the month's change in on-budget money…
  Equity:Envelope:To-Be-Budgeted         27460.00 TRY  ; …mirrored, so the pool is real money

2026-01-31 * "Envelope funding 2026-01" #envelope
  Equity:Envelope:Fees                     200.00 TRY
  Equity:Envelope:To-Be-Budgeted          -200.00 TRY
  Equity:Envelope:Groceries               4000.00 TRY
  Equity:Envelope:To-Be-Budgeted         -4000.00 TRY

2026-01-31 * "Envelope activity 2026-01" #envelope
  Equity:Envelope:Fees                     -40.00 TRY
  Equity:Envelope:To-Be-Budgeted            40.00 TRY
  Equity:Envelope:Groceries              -2500.00 TRY
  Equity:Envelope:To-Be-Budgeted          2500.00 TRY

2026-01-31 * "Envelope rollover 2026-01" #envelope
  Equity:Envelope:Fees                    -160.00 TRY  ; `rollover FALSE`: reset to zero…
  Equity:Envelope:To-Be-Budgeted           160.00 TRY  ; …so the unspent 160 goes back
```

and asserts the month-end balances a day later, where rustledger's own validation
checks them (`assert` defaults to `TRUE`):

| Date | Account | Balance |
| --- | --- | --- |
| 2026-02-01 | `Equity:Envelope:Groceries` | 1500.00 TRY |
| 2026-02-01 | `Equity:Envelope:To-Be-Budgeted` | 25960.00 TRY |
| 2026-03-01 | `Equity:Envelope:Groceries` | 900.00 TRY |
| 2026-03-01 | `Equity:Envelope:To-Be-Budgeted` | 21920.00 TRY |

Read as the budget table, which is just a query over those accounts:

| Month | Category | Budgeted | Spent | Balance |
| --- | --- | --- | --- | --- |
| 2026-01 | Groceries | 4000.00 | −2500.00 | **1500.00** |
| 2026-01 | Fees | 200.00 | −40.00 | 0.00 (reset, 160.00 back to the pool) |
| 2026-02 | Groceries | 4000.00 | −4600.00 | **900.00** (1500.00 carried) |
| 2026-02 | Fees | 200.00 | −40.00 | 0.00 |
| 2026-02 | To-Be-Budgeted | — | — | **21920.00** |

`Groceries` shows the carry (`rollover` defaults to `TRUE`), `Fees` shows the
reset, and `To-Be-Budgeted` is an account balance rather than a figure someone
has to recompute. These exact numbers are pinned by the
`the_readme_worked_example_holds` test, so the documentation cannot drift from
the plugin.

Other things the same mechanism gives you:

| You want | You write |
| --- | --- |
| move 500 TRY of Groceries back to the pool this month | `2026-12-01 custom "envelope" "adjust" Expenses:Groceries -500.00 TRY` |
| empty an old category at year end | `2026-12-31 custom "envelope" "zero" Expenses:Old-Stuff` |
| say "this category holds 250 EUR on 2027-01-01 and make it so" | `2027-01-01 custom "envelope" "pad" Expenses:Travel 250.00 EUR` |
| only carry up to 5000 TRY of Fun | `2026-10-01 custom "envelope" "rollover" Expenses:Fun 5000.00 TRY` |
| fund a USD envelope from USD you bought, not from TRY | `2026-10-01 custom "envelope" "fund" Expenses:Vps 20.00 USD` (the USD pool is fed by the FX purchase itself) |

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

An envelope holds positions per commodity, exactly like a beancount account, and
the two sides of the budget are measured differently on purpose:

* **spending** consumes a posting's weight — `units × cost`, else `units × price`,
  else `units` — so a 10 AAPL purchase at 150.00 USD consumes 1500.00 USD, and a
  USD expense in a TRY ledger never quietly lands in a TRY envelope;
* **money on hand** is measured in what the account *holds* (its units), so
  buying 100.00 USD with 3500.00 TRY puts **100.00 USD** in the pool, not the TRY
  that USD cost. Otherwise a USD envelope could never be funded from a USD
  purchase and every USD category would look permanently short.

Because of that, funding USD beyond the USD you hold is reported in USD while TRY
stays untouched — which is precisely the case you fix by buying currency, an
ordinary on-budget transfer in your ledger.

## Getting it

The plugin ships as a release asset: publishing a release builds it and attaches
`rustledger_envelope.wasm` (plus a `.sha256` beside it) to that release.

```sh
curl -LO https://github.com/<owner>/<repo>/releases/latest/download/rustledger_envelope.wasm
curl -LO https://github.com/<owner>/<repo>/releases/latest/download/rustledger_envelope.wasm.sha256
sha256sum -c rustledger_envelope.wasm.sha256
```

Put it next to your journal and reference it from there — a `plugin` path is
resolved relative to the file that declares it:

```beancount
plugin "rustledger_envelope.wasm"
```

Pre-releases run the same pipeline, so a release candidate can be attached and
tried without becoming `latest`. To build it yourself instead, see
[Building](#building).

## Building

The plugin is a nix package:

```sh
nix build .#rustledger-envelope       # → result/lib/rustledger_envelope.wasm
```

The derivation is offline and reproducible: its inputs are pinned by the crate's
`Cargo.lock` (vendored by `rustPlatform.importCargoLock`), the compiler is
nixpkgs' rustc — which already ships the `wasm32-unknown-unknown` standard library,
so no rust overlay is needed — and the link step uses `lld`'s `wasm-ld`, because
that rustc has no `rust-lld` of its own. The crate's test suite runs inside the
build (`doCheck`), so a successful `nix build` means the tests passed *and* the
module is the very file a release carries.

To work on the crate without the app's heavy shell (webkit, gtk, node):

```sh
nix develop .#plugin
cargo test                                                    # native
cargo build --release --target wasm32-unknown-unknown         # the plugin
```

With a plain Rust toolchain instead: `rustup target add wasm32-unknown-unknown &&
cargo build --release --target wasm32-unknown-unknown` — rustup's rustc finds its
own `rust-lld`, so nothing else is required.

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

## Licence

The plugin's source is **AGPL-3.0-or-later** — the repository's licence, carried
in the SPDX header of every file.

The *built* `.wasm` is a combined work, and one of the libraries it links is not
permissive, so the artifact cannot be offered under a more permissive licence
than the source:

| Bundled | Licence |
| --- | --- |
| `rustledger-plugin-types` (the plugin ABI: `PluginInput`/`PluginOutput`, `wasm_plugin_main!`) | GPL-3.0-only |
| `rust_decimal` | MIT |
| `regex-lite` | MIT OR Apache-2.0 |
| their transitive dependencies | permissive (MIT / Apache-2.0), except the above |

`rustledger-loader` and `rustledger-validate` (test-only here) are GPL-3.0-only as
well. A distributed module therefore carries GPL-3.0 terms — in practice
AGPL-3.0 *without* the "or later", since an "only" component forbids choosing a
later version — and the permissive components' notices have to travel with it.

Wanting a permissive plugin (MIT or Apache-2.0) is a different design, not a
header change: it means dropping the GPL dependency, i.e. declaring the ABI types
and the `alloc`/`process`/`__rustledger_abi_version` exports by hand (they are
documented in rustledger's custom-plugins guide, ~200 lines), after which nothing
GPL-linked remains and the crate can be relicensed freely.

## How it runs inside rustledger

The plugin is a directive plugin and rustledger runs it in the **post-booking**
pass, so it sees booked postings with their costs and prices filled in. Late
validation runs after it, so every generated transaction is checked for balance
and currency constraints like any other: a wrong sign shows up as the ledger's own
error rather than as a wrong number in a report. The plugin never modifies what
you wrote — it keeps every input directive and inserts its own.

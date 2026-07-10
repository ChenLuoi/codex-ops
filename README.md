# codex-ops

`codex-ops` is a Rust CLI for local Codex auth profiles, session usage, and
server rate-limit workflows.

The public command and Rust crate name are both `codex-ops`. The npm package is
only a thin distribution shim: it detects the current platform, finds the
prebuilt Rust binary from an optional platform package, and forwards argv,
stdio, signals, and exit codes. It does not expose a JavaScript import API and
does not contain JavaScript business logic.

## Usage

After the npm package is published, run it directly with:

```bash
npx codex-ops
```

After the crate is published, Rust users can install the same CLI with:

```bash
cargo install codex-ops
```

Local development in this repository uses Cargo for the product binary:

```bash
rtk cargo test
rtk cargo build --release
rtk target/release/codex-ops --help
```

The npm shim can be tested against the local release binary:

```bash
rtk env CODEX_OPS_RUST_BINARY=target/release/codex-ops npm run smoke:npm-shim
```

The Rust CLI fixture smoke runs as an integration test:

```bash
rtk npm run smoke:rust-cli
```

Published npm installs support Node.js `>=20.12.0` for the shim. Local
development requires a Rust stable toolchain; Node.js `>=20.12.0` is only
needed for npm shim, packaging, and release helper scripts.

## Commands

```bash
codex-ops --help
codex-ops auth status
codex-ops auth status --auth-file ~/.codex/auth.json
codex-ops auth status --json
codex-ops auth save
codex-ops auth list
codex-ops auth select
codex-ops auth select --account-id <account-id>
codex-ops auth remove
codex-ops auth remove --account-id <account-id> --yes
codex-ops doctor
codex-ops fast status
codex-ops fast on --at 2026-05-10T09:00:00Z
codex-ops fast off --at 2026-05-10T10:00:00Z
codex-ops fast history
codex-ops fast candidates --last 7d --json
codex-ops stat
codex-ops stat --start 2026-05-01 --end 2026-05-12 --group-by day
codex-ops stat --group-by hour
codex-ops stat --group-by week
codex-ops stat --group-by month
codex-ops stat --group-by model
codex-ops stat --group-by model --reasoning-effort
codex-ops stat --group-by cwd
codex-ops stat --group-by account
codex-ops stat --account-id <account-id>
codex-ops stat --all --group-by model --format csv
codex-ops stat --today
codex-ops stat --month --format markdown
codex-ops stat --last 30d --format json
codex-ops stat --last 2w --format csv
codex-ops stat --group-by model --sort credits --limit 5
codex-ops stat --limit-window 7d
codex-ops stat --limit-window 5h --group-by model
codex-ops stat --limit-window 7d --group-by account --format json
codex-ops stat --verbose
codex-ops stat --usage-mode-history-file ~/.codex/codex-ops/usage-mode-history.json
codex-ops stat sessions --top 10
codex-ops stat sessions --sort time --limit 10
codex-ops stat sessions session-a --last 30d
codex-ops stat sessions session-a --format json --limit 20
codex-ops stat sessions --last 30d --format json
codex-ops limit current
codex-ops limit windows --window 7d
codex-ops limit trend --window 5h
codex-ops limit resets --window 7d --early-only
codex-ops limit samples --window 5h --format json
```

### Auth

Syntax:

```bash
codex-ops auth status
codex-ops auth save
codex-ops auth list
codex-ops auth select
codex-ops auth remove
```

Auth commands read `auth.json` from `$CODEX_HOME/auth.json` by default, or
`~/.codex/auth.json` when `CODEX_HOME` is not set. It expects the fixed Codex
auth structure and decodes `tokens.id_token` without verifying the signature.
`auth status` prints only the key account fields: account ID, key ID, name,
email, user ID, plan, and organizations. It never prints the raw ID token.

`auth save` persists the entire current `auth.json` under the profile store
using the account ID as the unique key. By default the store is
`$CODEX_HOME/codex-ops/auth-profiles`; `--auth-file` only changes which
auth file is read. Use `--store-dir` to choose a different profile store.
`auth list` only shows the current profile and readable persisted profiles. If a
persisted profile cannot be decoded, it is listed under skipped profiles instead
of failing the whole command. `auth select` switches to a persisted profile; in
an interactive terminal it uses an Up/Down/Enter selection list, saves the
current `auth.json` first, then replaces `auth.json` with the selected persisted
content. The first switch also initializes
`$CODEX_HOME/codex-ops/auth-account-history.json` from the current
`auth.json`, then records each successful `auth select` timestamp so usage can be
attributed back to the active account. `--store-dir` only moves saved auth
profiles; use `--account-history-file` if the account history itself should live
somewhere else. `auth remove` shows an interactive multi-select list where Space
toggles entries and Enter confirms the selection, then asks for a second
confirmation before deleting persisted copies. The interactive remove list does
not offer the currently active profile, and cancelling an interactive prompt
leaves auth files, saved profiles, and account history unchanged.

Options:

| Option | Behavior |
| --- | --- |
| `--auth-file <path>` | Use a specific `auth.json` file. |
| `--codex-home <path>` | Read `<path>/auth.json`. Ignored when `--auth-file` is supplied. |
| `--store-dir <path>` | Use a specific auth profile store directory for `save`, `list`, `select`, and `remove`. |
| `--account-history-file <path>` | Use a specific auth account history file for `select`. |
| `-j, --json` | Print JSON output with the summarized auth fields. |
| `--include-token-claims` | Include the decoded JWT header and claims in JSON output. |
| `-A, --account-id <id>` | Select or remove a specific persisted profile. |
| `-y, --yes` | Skip confirmation when removing with `--account-id`. |

### Doctor

Syntax:

```bash
codex-ops doctor
codex-ops doctor --json
```

`doctor` checks local Codex Ops configuration and data. It reports Node.js
version, auth file decode status, sessions directory readability, helper
directory state, recent token usage, recent rate-limit samples, and embedded
pricing metadata. It does not read or validate any old anchor store.

`Recent rate limits` scans the last 7 days of local session JSONL for
`payload.rate_limits` samples. If the sessions directory is readable but no
samples are observed, the check is a warning with a clear no observed rate
limits message.

Options:

| Option | Behavior |
| --- | --- |
| `--auth-file <path>` | Use a specific `auth.json` file. |
| `--codex-home <path>` | Resolve default auth and session paths under this Codex home. |
| `--sessions-dir <path>` | Use a specific sessions directory. |
| `-j, --json` | Print JSON output. |

### Usage Mode

Syntax:

```bash
codex-ops fast status
codex-ops fast on
codex-ops fast off
codex-ops fast history
codex-ops fast candidates
```

`fast on/off/status/history` records local usage attribution only. It does not
change Codex settings, does not call a network service, and does not turn any
server-side mode on or off. The history file is used later by `stat` to price
usage that happened while local fast attribution was on.

By default the history file is
`$CODEX_HOME/codex-ops/usage-mode-history.json`, or
`~/.codex/codex-ops/usage-mode-history.json` when `CODEX_HOME` is not set. Use
`--usage-mode-history-file` to read or write a different file.

Examples:

```bash
codex-ops fast on
codex-ops fast on --at 2026-05-10T09:00:00Z
codex-ops fast off --at 2026-05-10T10:00:00Z
codex-ops fast status --json
codex-ops fast history
codex-ops fast candidates --last 7d
```

Fast options:

| Option | Behavior |
| --- | --- |
| `--at <time>` | Switch timestamp for `on` and `off`; omitted means now. |
| `--usage-mode-history-file <path>` | Use a specific local usage mode history file. |
| `-j, --json` | Print JSON output. |

### Stat

Syntax:

```bash
codex-ops stat [view] [session]
```

`stat` reads Codex session JSONL files from `~/.codex/sessions` by default.
Use `--codex-home` or `--sessions-dir` to point it at another Codex data
directory. The default scanner reads rollout files in the requested range and
checks older rollout files in a bounded lookback window by their last
`token_count` timestamp before deciding whether to read them. The lookback is
`min(max((end - start) / 2, 7 days), 30 days)`.
Use `-F, --full-scan` when you need exact local `token_count` results across
long sessions that may have started before the requested range. Full scan checks
all rollout files before the requested range by last `token_count` timestamp.
Date-ranged non-full-scan table and Markdown output includes a reminder, and
JSON output includes the same message in `warnings`.
Use `--group-by account` or `--account-id <id>` to initialize/read
`auth-account-history.json` and attribute `token_count` events by the account
active at each event timestamp.

Views:

| Command | Output |
| --- | --- |
| `codex-ops stat` | Aggregate token usage by the resolved `group-by` value. |
| `codex-ops stat sessions` | Top sessions by credits by default. |
| `codex-ops stat sessions <session-id>` | Event-level token usage timeline for one session. |

Time range options:

| Option | Behavior |
| --- | --- |
| `-s, --start <time>` | Start time. Date-only values start at local `00:00:00.000`. |
| `-e, --end <time>` | End time. Date-only values end at local `23:59:59.999`. |
| `-t, --today` | Current local day through now. |
| `--yesterday` | Previous local day. |
| `-m, --month` | Current local calendar month through now. |
| `-L, --last <duration>` | Recent duration such as `12h`, `7d`, `2w`, or `1mo`. |
| `-a, --all` | Scan and include all session usage records without date pruning. |

When `--group-by` is not supplied, `stat` chooses a default from the resolved
time range: ranges up to 48 hours use `hour`, ranges up to 31 days use `day`,
ranges up to six calendar months use `week`, and longer ranges use `month`.
`--month` remains grouped by `day` by default, while `--all` defaults to
`month`.

Aggregation and shaping options:

| Option | Behavior |
| --- | --- |
| `-g, --group-by <group>` | Aggregate by `hour`, `day`, `week`, `month`, `model`, `cwd`, or `account`. Ignored by `sessions` views. |
| `--limit-window <window>` | Aggregate usage by observed server rate-limit windows: `5h` or `7d`. |
| `-S, --sort <sort>` | Sort rows by `time`, `tokens`, `credits`, `calls`, or `sessions`. |
| `-n, --limit <n>` | Cap output rows. For `sessions <session-id>`, this caps displayed events while totals still cover the whole matched session. |
| `-T, --top <n>` | Session-list row count. When both `--top` and `--limit` are supplied to `stat sessions`, `--top` wins. |
| `-d, --detail` | Show full event-level rows for `stat sessions <session-id>`. |
| `-F, --full-scan` | Scan all session files instead of pruning by date. |
| `-r, --reasoning-effort` | When grouping by `model`, append Codex reasoning effort to the model key. |
| `-A, --account-id <id>` | Only include usage attributed to an account id. |
| `--usage-mode-history-file <path>` | Apply local fast attribution history when estimating credits and USD. |

When `--reasoning-effort` is combined with `--group-by model`, Codex reasoning
effort is appended when present, for example `gpt-5.5-high` or
`gpt-5.5-xhigh`. Fast-attributed usage is grouped under a distinct model key
such as `gpt-5.5-fast`; with reasoning effort enabled this becomes
`gpt-5.5-fast-high`. Pricing still uses the base model name plus the local
usage mode.

`stat --limit-window 5h|7d` joins token usage with observed server
rate-limit windows from local JSONL. Without `--group-by`, it emits one row per
observed window. With `--group-by model|cwd|account`, it emits flat
`(window_id, group_key)` rows. It does not guess windows that were not observed;
usage that cannot be placed in an observed window is reported in an
`observed=false` unobserved row with diagnostics. Time groupings such as
`hour`, `day`, `week`, and `month` are not valid with `--limit-window`.

When a usage mode history is present, `stat` applies fast multipliers only from
that local history. Token totals, call counts, session counts, and rate-limit
percent values are unchanged. `--group-by model` displays fast-attributed calls
separately, for example `gpt-5.5-fast`. The current fast multipliers are 2.0x
credits for gpt-5.4 and 2.5x credits for gpt-5.5; other models default to 1.0x
unless the embedded rate card defines another multiplier. Human-readable output
and default JSON diagnostics do not print the history file path; verbose JSON
diagnostics include it.

`fast candidates` is read-only and detection-only. It never writes usage
mode history and never runs `fast on` or `fast off` for you. It always
uses the 5-hour (`5h`) rate-limit window and rejects `--limit-window`; there is
no `--window`, `--capacity`, or `--auto-capacity` option for this view. The
detector scores contiguous session segments within a rollout instead of single
calls, so delayed or batched 5-hour usage-percent changes are evaluated against
the segment's accumulated usage. A candidate segment must contain at least three
usage calls and more than the minimum 1 percentage-point usage step. When
multiple sessions are active in the same 5-hour interval, the observed
`used_percent` delta is split across sessions by their normal-credit share
before scoring. The output labels rows as candidates, includes confidence and
reason fields, and prints manual `codex-ops fast on/off --at ...` command
hints for review. Default output avoids full source file paths; verbose JSON can
include file path evidence for debugging.

Examples:

```bash
codex-ops stat --all --usage-mode-history-file ~/.codex/codex-ops/usage-mode-history.json
codex-ops fast candidates --last 7d
codex-ops fast candidates --start 2026-05-10T00:00:00Z --end 2026-05-10T04:00:00Z --json
```

Output options:

| Option | Behavior |
| --- | --- |
| `-f, --format <format>` | Output `table`, `json`, `csv`, or `markdown`. |
| `-j, --json` | Alias for `--format json`. |
| `-v, --verbose` | Include scan and parsing diagnostics in table output. JSON output always includes diagnostics. |

Diagnostics include scanned/skipped directories, read/skipped files, read lines,
invalid JSON lines, token-count events, included usage events, skipped-event
reasons, and file read concurrency.

Credits are estimated from the token counters in each session. Cached input
tokens are billed at the cached-input rate; regular input credits use
`max(inputTokens - cachedInputTokens, 0)`. USD estimates use `25 credits = $1`.
When a model has no configured price, it is excluded from Credits and listed in
an unpriced-model breakdown with a stub you can fill into
`data/codex-rate-card.json`.
JSON output includes the same information under `unpricedModels`.

### Rate Limits

Syntax:

```bash
codex-ops limit current
codex-ops limit windows
codex-ops limit trend
codex-ops limit resets
codex-ops limit samples
```

`limit` reads local Codex session JSONL files and parses
`payload.rate_limits`. It does not call Codex, OpenAI, or any network service.
Reports are based only on observed local samples and show server-provided
percentages and reset timestamps for 5-hour (`5h`) and 7-day (`7d`) windows.
Human-readable and CSV time fields are displayed in the local timezone; JSON
timestamps remain RFC 3339 UTC values. It does not infer absolute quota units.
Except for `limit current`, limit subcommands default to the 7-day window.
Use `--window 5h` to inspect the 5-hour window.
`limit current` and `limit windows` join each displayed reset cycle with local
token-count records and append actual total token, credit, and USD usage for
that cycle without treating those values as the server quota size.
`limit current` always reads the last 7 days and does not accept `--start`,
`--end`, or `--last`. Other limit commands read the last 30 days of local
session data by default when no explicit `--start`, `--end`, or `--last` is
supplied. Supplying only `--end` uses a 30-day lookback ending at that time.

If the latest data has `rate_limits:null`, lacks a window, or no sample exists
in the requested range, the relevant output is marked `unobserved` instead of
falling back to a guessed window. For each account/plan/limit/window partition,
`current` shows the latest observed logical quota cycle; a later reset cycle
replaces an earlier one even when the earlier cycle's reset timestamp is still
in the future. If that latest cycle has ended, `current` marks it with an
`expired` row status. Samples with
`window_minutes <= 0` are treated as invalid and ignored. The current table,
CSV, and Markdown outputs include `Window minutes` so a nonstandard `primary`
window is visible instead of being mistaken for the standard 5-hour window, and
they show the reset timestamp without a separate reset-seconds column. `limit
windows` JSON keeps the machine-readable `id` field, while table, CSV, and
Markdown output omit that long identifier and start with the
window/account/plan/limit columns. `limit windows` merges reset timestamps within
60 seconds into one logical window, so server-side reset jitter does not split a
single quota window into many rows. Derived reports (`current`, `windows`,
`trend`, and `resets`) ignore inactive quota streams whose samples stay at 0%
and whose reset timestamp rolls forward with each sample; `limit samples` still
shows those raw observations for debugging. Windows, current rows, trend changes,
and reset events are partitioned by account, plan, limit id, and window length,
so samples from different server quota streams are not compared against each
other.
`limit trend` is a change-point timeline built from observed rate-limit
vectors. Repeated token-count snapshots inside the same rollout are compressed,
expired reset windows are ignored, and reset timestamps within 60 seconds are
treated as the same logical window. Within a logical window the displayed
progress keeps the highest observed used percent, so stale snapshots from
parallel sessions do not create false decreases. A selected window is only
shown when that window's displayed value or reset time changes; sibling-window
activity is not emitted as an extra row.
`limit resets` only emits reset transitions inside one quota stream. A reset
event requires a changed reset timestamp, a lower used percent, and a next reset
time that is still active for the next sample; reset timestamp jitter within 60
seconds is ignored. Verbose JSON diagnostics include counts for samples and
reset events whose limit id was missing, because those unknown-limit rows are
less precise than named quota streams.
The old `--group-by hour|day` bucket mode is no longer supported.

Examples:

```bash
codex-ops limit current
codex-ops limit current --window 5h --json
codex-ops limit windows --window 7d --format markdown
codex-ops limit trend --window 5h
codex-ops limit trend --window 7d --format csv
codex-ops limit resets --window 7d
codex-ops limit resets --window 7d --early-only
codex-ops limit samples --window 5h --format json
```

Limit reports read `~/.codex/sessions` by default. Use `--codex-home` or
`--sessions-dir` for alternate local data. When an account history file exists,
samples are attributed by the account active at each sample timestamp; use
`--account-id <id>` to filter to one account. `current` uses a fixed 7-day
lookback. Other limit commands default to the last 30 days; use `-L, --last`,
`--start`, or `--end` to narrow or pin their range.

Limit commands:

| Command | Output |
| --- | --- |
| `codex-ops limit current` | Current-cycle snapshot from the fixed 7-day lookback; each partition shows its latest logical cycle, marked active or expired, plus token/credit/USD usage for that cycle. |
| `codex-ops limit windows` | Observed server windows inferred from sample reset times, with token/credit/USD usage for each window; quota window defaults to `7d`; JSON includes `id`, non-JSON output omits it. |
| `codex-ops limit trend` | Used-percent change timeline for one selected quota window; quota window defaults to `7d`. |
| `codex-ops limit resets` | Reset events for one selected quota window, including early reset detection; quota window defaults to `7d`. |
| `codex-ops limit samples` | Raw rate-limit samples after filters; quota window defaults to `7d`. |

Limit options:

| Option | Behavior |
| --- | --- |
| `--window <window>` | Include only `5h` or `7d` samples/windows; non-current limit commands default to `7d`. |
| `--early-only` | For `resets`, include only resets before the prior reset time. |
| `-A, --account-id <id>` | Only include one account id. |
| `--account-history-file <path>` | Use a specific auth account history file. |
| `--codex-home <path>` | Resolve sessions and account history under this Codex home. |
| `--sessions-dir <path>` | Use a specific sessions directory. |
| `-s, --start <time>` | Start time; not accepted by `current`. |
| `-e, --end <time>` | End time; not accepted by `current`. |
| `-L, --last <duration>` | Recent duration such as `12h`, `30d`, `2w`, or `1mo`; overrides the default 30-day range; not accepted by `current`. |
| `-f, --format <format>` | Output `table`, `json`, `csv`, or `markdown`. |
| `-j, --json` | Alias for `--format json`. |
| `-v, --verbose` | Include scan diagnostics; with JSON, include source file/line evidence. |

Pricing data is statically embedded from `data/codex-rate-card.json`. The
current snapshot source is OpenAI Help Center Codex rate card, checked
2026-07-10.

| Model | Input / 1M | Cached input / 1M | Output / 1M | Note |
| --- | ---: | ---: | ---: | --- |
| GPT-5.6 Sol | 250 credits | 25 credits | 1,500 credits |  |
| GPT-5.6 Terra | 125 credits | 12.50 credits | 125 credits |  |
| GPT-5.6 Luna | 50 credits | 5 credits | 300 credits |  |
| GPT-5.5 | 125 credits | 12.50 credits | 750 credits | fast attribution multiplier 2.5x |
| GPT-5.4 | 62.50 credits | 6.250 credits | 375 credits | fast attribution multiplier 2.0x |
| GPT-5.4-mini | 18.75 credits | 1.875 credits | 113 credits |  |
| GPT-5.3-Codex | 43.75 credits | 4.375 credits | 350 credits |  |
| GPT-5.2 | 43.75 credits | 4.375 credits | 350 credits |  |
| GPT-5.3-Codex-Spark | 0 credits | 0 credits | 0 credits | research preview; charged at 0 credits |
| GPT-Image-2 (image) | 200 credits | 50 credits | 750 credits |  |
| GPT-Image-2 (text) | 125 credits | 31.25 credits | 250 credits |  |

## Development

The CLI implementation lives in standard Cargo source paths. Keep business
logic in Rust; JavaScript is reserved for the npm shim and release helper
scripts.

```bash
rtk cargo fmt --check
rtk cargo test
rtk cargo build --release
rtk npm run release:check
rtk npm run smoke:rust-cli
rtk env CODEX_OPS_RUST_BINARY=target/release/codex-ops npm run smoke:npm-shim
rtk npm run bench:rust
```

The repository also includes a `justfile` for local orchestration. Recipes only
compose existing Cargo and npm commands; assertions and fixture behavior belong
in Rust tests or dedicated helper scripts. In this workspace, run recipes
through RTK:

```bash
rtk just --list
rtk just test
rtk just build
rtk just smoke
rtk just bench
rtk just release-check
```

The default benchmark command is Rust-only and uses the synthetic fixture in
`test/fixtures/rust-run`. Larger 100x benchmark data is local-only and must not
be committed.

Node scripts under `scripts/` are reserved for npm shim smoke and npm release
packaging. Default CLI smoke and benchmark coverage should stay in Rust tests
or Rust helper binaries.

## Package Layout

```text
src/main.rs                      Rust CLI process entry
src/lib.rs                       Rust command parsing and dispatch
src/*.rs                         Rust business modules
src/bin/codex-ops-bench.rs       Local Rust benchmark smoke helper
bin/codex-ops.js                 npm shim entrypoint
npm/<target>/package.json        npm platform package manifests
justfile                         local command orchestration
scripts/*.mjs                    npm shim/release helpers
test/fixtures/rust-run/          synthetic fixture data only
```

The published Cargo crate exposes only the `codex-ops` binary. Local helper
binaries, npm packaging assets, CI configuration, task documents, and synthetic
fixtures are excluded from the crate package; they remain repository-only
development assets.

## Release

GitHub Actions builds release artifacts for:

```text
linux-x64-gnu
linux-arm64-gnu
linux-x64-musl
linux-arm64-musl
darwin-x64
darwin-arm64
win32-x64-msvc
```

Linux npm packages are split by libc. GNU/glibc Linux installs the `*-gnu`
package for native performance, while musl Linux installs the static `*-musl`
package for Alpine-style environments.

The main `codex-ops` npm package depends on scoped `@codexops/*` platform
packages through `optionalDependencies`. The release workflow is tag-driven:
push a `vX.Y.Z` tag matching `package.json` / `Cargo.toml`, and Actions builds
the platform binaries, npm tarballs, crate archive, `release-manifest.json`, and
top-level `SHA256SUMS` once. Those assets are uploaded to a draft GitHub
Release before registry publishing starts.

Publishing to crates.io and npm is gated by the GitHub `release` environment.
After approval, the workflow publishes the crate, then the scoped platform npm
tarballs, then the main npm tarball. npm publish steps reuse the previously
packed tarballs and skip package versions that already exist, so a failed
registry publish can be retried from the same tag. When all registry publishes
finish, the draft GitHub Release is published.

Before publishing, recheck npm platform package name availability, crates.io
token access, GitHub `release` environment approval, and whether any legacy
migration package or alias is needed.

## Data Safety

`codex-ops` reads local Codex files such as `$CODEX_HOME/auth.json`,
`$CODEX_HOME/sessions`, and `$CODEX_HOME/codex-ops/*`. Do not commit real
auth files, raw session JSONL, account IDs, tokens, cwd values, or user content.
Use only synthetic fixtures under `test/fixtures/**`.

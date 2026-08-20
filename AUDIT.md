# Audit: `${?}` optional-predicate tail scanner

Verification of an external code review, plus findings the review missed.
Every claim below was reproduced by compiling and running the stated
`query!` invocation and printing `.sql()` — none is inferred from reading
code. Baseline at audit time: 207 tests green, working tree clean.

Date: 2026-08-20. Commit: `9546831`.

## Root cause

`predicate_tail_end` ([sqlx_dyn_macros/src/parse.rs:618](sqlx_dyn_macros/src/parse.rs#L618))
scans `rest.to_ascii_uppercase()` — the raw template text — for `)`, `;`,
`JOINERS` and `CLAUSE_ENDS`. It has no notion of SQL literals, so any of
those tokens appearing inside a string literal, a quoted identifier, or a
dollar-quoted body terminates the predicate at the wrong offset.

The crate already owns the right tool: `strip_literals`
([parse.rs:322](sqlx_dyn_macros/src/parse.rs#L322)) upper-cases the text,
blanks literals and quoted identifiers, skips interpolation markers, and
documents that **byte offsets are preserved** — each input byte maps to
exactly one output byte, so an offset found in the stripped view indexes
the original. `clause_map` already consumes it. `predicate_tail_end` does
not.

Consequence: the reviewer's items 1, 2 and 3 are one defect, not three.

## Severity table

| # | Defect | Repro'd | Severity | Origin |
|---|---|---|---|---|
| A | `AND`/`OR`/`WHERE` inside a string literal splits the predicate mid-string | yes | **critical** | missed by review |
| B | `)` / `;` inside a literal truncates the tail | yes | high | review #2 |
| C | Escape-truncation check counts only `'` parity | yes | high | review #3 |
| D | `CLAUSE_ENDS` missing `FOR`, `WINDOW`, `ON` | yes | high | review #1 |
| E | Fragments hide clause boundaries from `${?}` bookkeeping | yes | high | review #4 |
| F | Rust comments inside `${...}` rejected | yes | medium | review |
| G | `find_comment` literal rules diverge from `strip_literals` | by code | medium | review |
| H | `.gitignore` `.*` makes CI unaddable | yes | medium | review, mis-scoped |
| I | Docs: license, e2e count | yes | low | review |
| J | 63-clause bitmask cap | documented | low | review, over-rated |

A–D share the root cause above and are fixed by one change.

## A — critical: `AND`/`OR` in a literal (missed by the review)

`JOINERS` = `["AND", "OR", "WHERE", "HAVING"]`
([parse.rs:514](sqlx_dyn_macros/src/parse.rs#L514)) is checked on the same
unlexed text as `CLAUSE_ENDS`. These four words are far likelier to appear
inside user string literals than `WINDOW` or `FETCH`. The review probed only
the `CLAUSE_ENDS` path and never tested this one, although the code is shared
([parse.rs:642](sqlx_dyn_macros/src/parse.rs#L642)).

```rust
let v: Option<i32> = None;
query!("SELECT * FROM t WHERE a = ${?v} || ' AND '").sql()
// -> "SELECT * FROM t WHERE '"          dangling WHERE + unterminated literal

query!("SELECT * FROM t WHERE name = ${?v} || ' OR admin'").sql()
// -> "SELECT * FROM t WHERE admin'"
```

Sharpest variant — mixed `Some`/`None`, where a bind survives and a
predicate silently vanishes:

```rust
let n: Option<i32> = None;
let w = Some(2i32);
query!("SELECT * FROM t WHERE a = ${?n} || ' AND ' AND b = ${?w}").sql()
// -> "SELECT * FROM t WHERE ' AND b = $1"
```

`$1` is bound and the query is dispatched, but predicate `a` is gone along
with an unbalanced quote. This is the only observed case where a live bind
path is corrupted.

All-`Some` paths were checked separately and are correct; corruption
requires at least one `None`.

## B — `)` / `;` inside a literal

```rust
let v: Option<i32> = None;
query!("SELECT * FROM t WHERE a = ${?v} || ')'").sql()
// -> "SELECT * FROM t)'"        Some(v) -> "... WHERE a = $1 || ')'"  (correct)

query!("SELECT * FROM t WHERE a = ${?v} || ';'").sql()
// -> "SELECT * FROM t;'"
```

Dollar-quoted bodies fail the same way:

```rust
query!("SELECT * FROM t WHERE a = ${?v} || $q$)$q$").sql()
// -> "SELECT * FROM t)$q$"
```

## C — escape-truncation check only counts `'`

`tail_is_truncated_by_escape` ([parse.rs:588](sqlx_dyn_macros/src/parse.rs#L588))
decides "is this escape inside my predicate" by counting `'` parity, missing
quoted identifiers and dollar quotes.

```rust
let v: Option<i32> = None;
let z = 5i32;
query!("SELECT * FROM t WHERE a = ${?v} || \"$${z}\"").sql()
// -> "SELECT * FROM t${z}\""
```

## D — `CLAUSE_ENDS` gaps

`CLAUSE_ENDS` ([parse.rs:556](sqlx_dyn_macros/src/parse.rs#L556)) omits `FOR`,
`WINDOW` and `ON`. Trailing top-level clauses get swallowed:

```rust
let v: Option<i32> = None;
query!("SELECT * FROM t WHERE a = ${?v} FOR UPDATE").sql()
// -> "SELECT * FROM t"                     locking clause silently dropped
// Some(v) -> "SELECT * FROM t WHERE a = $1 FOR UPDATE"

query!("UPDATE t SET x = 1 WHERE y = ${?v} ON CONFLICT DO NOTHING").sql()
// -> "UPDATE t SET x = 1"

query!("SELECT * FROM t WHERE a = ${?v} WINDOW w AS (PARTITION BY b)").sql()
// -> "SELECT * FROM t)"
```

Lower-case is equally affected (`for update` -> `select * from t`).

### Why the review's fix for D is wrong

The review's primary recommendation — "add `FOR`, `WINDOW`, `ON` at minimum" —
treats the symptom and **enlarges defect B**. Every keyword added to the list
gains a false positive inside literals. That is already observable today:

```rust
query!("SELECT * FROM t WHERE a = ${?v} || 'LIMIT'").sql()
// -> "SELECT * FROM t"       because LIMIT is in CLAUSE_ENDS
```

Add `FOR` and `|| ' FOR '` breaks identically. The review noted the better
approach parenthetically but ranked whack-a-mole first. Correct order: make
the scanner literal-aware (fixes A–C and makes D safe), *then* extend the
keyword list.

## E — fragments hide clause boundaries

`marker_span` ([parse.rs:398](sqlx_dyn_macros/src/parse.rs#L398)) skips `#{...}`
when building the clause map. That is right for `${...}` (Rust text) but a
fragment marker hides *SQL* text.

```rust
const F: SqlFragment = sql_fragment!("1 UNION SELECT * FROM u");
let p: Option<i32> = None;
let q: Option<i32> = Some(2);
query!("SELECT * FROM t WHERE a = ${?p} AND #{F} AND b = ${?q}").sql()
// -> "SELECT * FROM t WHERE 1 UNION SELECT * FROM u AND b = $1"
```

Invalid: the `UNION` inside `F` opens a new predicate list, so `q` should be
introduced by `WHERE`, not joined with `AND`.

Not closable by a scanner fix — a `SqlFragment` is an opaque runtime value
and the macro cannot see through it in general. This is an API decision:
either reject `${?}` in templates that also contain `#{}` in the same clause,
or document a hard constraint ("fragments combined with `${?}` must not
contain top-level `UNION`/`INTERSECT`/`EXCEPT`/`;`"). Conflicts with the
crate's stated "reject when unsure" model as it stands.

## F — Rust comments inside `${...}`

`find_close` ([parse.rs:822](sqlx_dyn_macros/src/parse.rs#L822)) tracks braces,
brackets and string literals but not Rust comments.

```rust
query!("SELECT ${/* } */ 1}")
// compile error: invalid Rust expression inside `${...}`: `/*`
```

False rejection of valid Rust. Fails at compile time, so no wrong SQL.

## G — `find_comment` diverges from `strip_literals`

`find_comment` handles only `'`/`"` with doubling; it does not treat `\` as an
escape and does not know dollar quotes, while `strip_literals` does both.
`split_predicate` runs `find_comment` on raw pending text, so dollar-quoted
bodies can trigger spurious comment rejections. Under
`standard_conforming_strings = off`, some valid `E'...'` templates are
rejected. False rejections only — no wrong SQL. Both should route through one
lexer.

## H — `.gitignore` blocks CI (mis-scoped by the review)

The review reported the pattern as `.**` and called it "odd/too broad", and
separately reported "No CI config found" as an independent item. Actual
content is `.*`, and the two items are cause and effect:

```
$ git check-ignore -v .github/workflows/ci.yml
.gitignore:3:.*	.github/workflows/ci.yml
```

CI cannot be committed until the pattern is narrowed. Fix `.gitignore`
before adding any workflow.

## I — docs

- [README.md:423](README.md#L423) states `MIT OR Apache-2.0`; `Cargo.toml:8`
  states `license = "MIT"` and only `LICENSE-MIT` exists on disk. The
  authoritative choice is the maintainer's.
- README says e2e adds 15 tests; actual count is 16.

## J — 63-clause bitmask cap (over-rated)

`Predicates::emitted` is a `u64` ([sqlx_dyn/src/optional.rs](sqlx_dyn/src/optional.rs)),
so clause indices above 63 degrade to "already emitted" — i.e. to the
hand-written joiner, not to wrong SQL. Documented, and unreachable in
realistic SQL.

The review's suggested replacement — an `Option<u32>` "last opened clause" —
**does not work**. `Predicates` tracks several independent predicate lists
concurrently (a `WHERE` plus a `HAVING`, the halves of a `UNION`), as that
module documents. A single last-opened field cannot represent independent
per-clause state; clauses are not entered in strict sequence. Keep the
bitmask.

## Review assessment

No false high-severity claims; all four reproduced byte-for-byte. The
weaknesses are in diagnosis rather than facts:

1. Items 1–3 are one defect, presented as three.
2. The headline fix for item 1 worsens item 2.
3. The `JOINERS` path was never probed; it holds the worst case (A).
4. `.gitignore` and "no CI" were listed as unrelated minor items.
5. The item-J remedy is incompatible with the module's multi-clause design.

## Status

Fixed in this pass:

- **A, B, C, D** — `predicate_tail_end` and `tail_is_truncated_by_escape` now
  scan the offset-preserving `strip_literals` view instead of raw text, and
  `CLAUSE_ENDS` gained `FOR`, `WINDOW`, `ON` (safe only because the scan is now
  literal-aware). C changed from silently corrupt SQL to a compile-time
  rejection, matching the crate's "reject when unsure" model.
- **H** — `.gitignore` `.*` narrowed to `/.claude/`; `.github/` is committable
  again, ignore set otherwise unchanged.
- **I** — README test counts corrected to 216 / 16.

Coverage added: 9 regression tests in
[sqlx_dyn/tests/optional.rs](sqlx_dyn/tests/optional.rs) (literal-borne
joiners, brackets, separators, dollar quotes, clause keywords; genuine
subquery brackets and trailing `FOR`/`ON CONFLICT`/`WINDOW` clauses; both
`Some` and `None` for each), plus two compile-fail fixtures for the
quoted-identifier and dollar-quote escape overlaps. Suite: 216 non-e2e tests
green, clippy clean, allocation parity unchanged.

Edge cases attacked after the fix and confirmed correct: bare trailing `$`/`#`
in a tail, non-ASCII tail bytes (offset parity), empty tail, tail that is
itself a marker, `ON` of a preceding `JOIN`, `format` not matching `FOR`,
`"for"` as a quoted identifier.

Outstanding:

- **E** — needs the API decision below; no code change made.
- **F, G** — false rejections, not wrong SQL; untouched.
- **I** — license discrepancy left as-is: `README.md` says
  `MIT OR Apache-2.0`, `Cargo.toml` says `MIT`, only `LICENSE-MIT` exists.
  Which is authoritative is the maintainer's call, and it is a licensing
  statement rather than a code defect.
- **J** — keep the bitmask; no change.

## Fix order

1. **H** — unblock CI (`.gitignore`).
2. **A, B, C, D** — route `predicate_tail_end` and
   `tail_is_truncated_by_escape` through the offset-preserving
   `strip_literals` view, then extend `CLAUSE_ENDS`. One change, four
   defects. Regression tests for each repro above, both `Some` and `None`.
3. **G** — unify `find_comment` onto the same lexer.
4. **F** — skip Rust comments in `find_close`, or emit a clear error.
5. **E** — API decision required before any code change.
6. **I** — docs, once the license question is settled.

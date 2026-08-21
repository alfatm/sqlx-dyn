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

> The last two sentences were wrong. The backslash handling produced false
> *acceptances* that emitted silently wrong SQL, and "both should route through
> one lexer" was the right instinct but was not acted on until N below.

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

Coverage added for E: 4 unit tests in
[sqlx_dyn_macros/src/parse.rs](sqlx_dyn_macros/src/parse.rs) (all fragments
shipped in this repo accepted; clause boundaries explicitly accepted, CTE body
included; unbalanced brackets rejected including the equal-count form; brackets
inside literals treated as data), 7 integration tests in
[sqlx_dyn/tests/optional.rs](sqlx_dyn/tests/optional.rs) covering both CTE
shapes — fragment-as-CTE and fragment-as-body — and 1 compile-fail fixture.

Coverage for the documented limit: 3 tests in
[sqlx_dyn/tests/optional.rs](sqlx_dyn/tests/optional.rs) pin the *broken* output
for a fragment that opens a top-level clause, the corrected form with the
boundary moved into the template, and the harmless case where the template has
no `${?...}` at all. Both the break and the workaround are also runnable
doctests in the crate docs, so `cargo test` fails if either stops behaving as
documented.

## `clause_map`'s depth rule: wrong, and the audit that cleared it was wrong too

Two earlier revisions of this section got this wrong in opposite directions.
The first recorded "deleting `depth == 0` in `clause_map` fails nothing" as a
coverage gap. The second called that framing wrong and concluded the guard was
*redundant but load-bearing in principle* — "there is no observable difference
to assert". The guard was in fact a defect, and the argument that cleared it had
the mechanism backwards.

The defect: a subquery's `WHERE` got no clause index of its own, so a predicate
inside the subquery and one after the closing `)` shared a clause. Codegen
records the first optional carrying `WHERE`/`HAVING` as its clause's introducer,
and that first optional was the *inner* one. Dropping it left the introducer
unclaimed, and the outer predicate — the first emitted in the shared clause —
fired it:

```
WHERE EXISTS (SELECT 1 FROM u WHERE k = ${?k}) AND b = ${?b}
  k = None, b = Some  ->  ... EXISTS (SELECT 1 FROM u) WHERE b = $1
```

A second top-level `WHERE`, which Postgres rejects. Two levels of nesting fail
the same way.

Where the clearing argument went wrong: it asked whether the *second* predicate
could carry a `WHERE` joiner, correctly answered no — it carries the written
`AND` — and stopped. But an introducer is not claimed by the predicate that
recorded it. It is claimed by whichever predicate is emitted first in the
clause. The two conditions are not mutually exclusive at all; they are held by
two different predicates. "Modelling `clause_map` in both variants confirms the
clause grouping really does differ" was the point at which the search should
have continued, not concluded.

The 15 shapes searched were all shapes where the nested boundary sits *between*
the predicates and the enclosing scope never closes between them. None closed a
nested scope and then continued the enclosing clause, which is the shape that
observes the difference.

Fixed by making `clause_map` scope-aware instead of depth-guarded: every
boundary keyword gets an index including nested ones, `(` pushes the open
clause, and `)` restores it and records a start at the bracket so an offset past
the subquery maps back to the enclosing clause. Both halves are required —
without nested indices the two predicates still share a clause; without the
restore the outer predicate lands in the inner clause and the same `WHERE` fires
one scope too low.

Two regression tests in
[sqlx_dyn/tests/optional.rs](sqlx_dyn/tests/optional.rs) cover the four-way
combination and the two-level nesting. Four mutations — restoring the depth
guard, dropping the restore, restoring without recording the start, and
restoring to `clause - 1` instead of the enclosing clause — each fail both.
Every other template shape is byte-identical: verified by diffing the emitted
SQL for a nested optional alone, two optionals both nested, and an outer
optional followed by a nested one, across all combinations.

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

## Second review pass

All five reported items reproduced. One was mis-severitied: item 2 emits invalid
SQL, not a false rejection.

| # | Item | Repro'd | Real severity | Fixed |
|---|---|---|---|---|
| 1 | bracket/comma scan before the marker on raw SQL | yes | medium (false reject) | yes |
| 2 | fragment with a leading `AND`/`OR` | yes | **high (invalid SQL)** | yes |
| 3 | Rust comments inside `${...}` | yes | medium (false reject) | yes |
| 4 | `find_comment` on raw text, no dollar quotes | yes | medium (false reject) | yes |
| 5 | brackets inside a SQL comment in a fragment | yes | low (false reject) | yes |

Items 1, 4 and 5 were the same root cause as the first pass — structure matched
on raw text rather than the offset-preserving stripped view — in three places the
first pass did not reach. `split_predicate` now derives its bracket/comma scan,
operator check and comment check from `upper`, and a new `blank_comments` helper
blanks SQL comments in that view for the fragment bracket check.

Item 2 was broader than reported: *any* fragment directly following an optional
predicate misbehaves, not only one starting with `AND`. But the plain-fragment
form (`WHERE a = ${?x} #{F}`) is malformed in the `Some` branch too — verified:
`WHERE a = $1 deleted_at IS NULL` — so the template itself is wrong, not the
codegen. Only a leading `AND`/`OR` *looks* correct while breaking, so that is
what `sql_fragment!` now rejects. Checked first that no fragment in the repo
starts with a joiner.

Item 3 fixed in `find_close` by skipping Rust line and (nesting) block comments.

Coverage: 5 integration tests, 3 macro unit tests, 1 compile-fail fixture. Real
violations confirmed still rejected — `ANY(${?v})`, `make_interval(days => ..)`,
genuine SQL comments before a marker, genuinely unbalanced fragment brackets,
`android`/`origin`/`ORDER BY` not mistaken for leading joiners.

Suite: 241 non-e2e tests, clippy clean, all three examples run.

## Third review pass

One high-severity finding, confirmed and fixed; the rest documentation drift.

**High — a SQL comment inside a fragment.** A template using `${?...}` may not
contain a comment, because one can hide the joiner between predicates. A
fragment is opaque to that check, so a comment inside one reopened the hole:

```rust
const F: SqlFragment = sql_fragment!("c = 1 --");
query!("SELECT * FROM t WHERE a = ${?x} AND #{F} AND b = 1").sql()
// -> "SELECT * FROM t WHERE a = $1 AND c = 1 -- AND b = 1"
```

`AND b = 1` is commented out; PostgreSQL accepts it and returns more rows than
the template asks for — silent wrong rows, the class this crate exists to avoid.
Broader than reported: the `None` branch is affected too, block comments behave
the same, and it does not require `${?...}` in the template at all (a trailing
comment swallows following text regardless).

Resolved by **stripping** rather than rejecting. A comment annotates the
fragment; it is not SQL the fragment contributes, so blanking it removes the
hazard while the fragment keeps working — no reason to make the author choose
between a comment and a usable fragment. Blanked to spaces, never deleted:
`c = 1/*x*/AND d = 2` would otherwise collapse to `1AND`, which the unit tests
pin. Literal positions come from the `strip_literals` view, so `'--'` and
`$tag$--$tag$` pass through untouched.

> Superseded in part by the fourth pass below: the layering described here was
> itself wrong, and the edge-trimming it mentions has been removed.

An **unterminated** `/*` is still rejected — there is no end to strip up to, so
it would swallow whatever follows the marker. Nested unclosed forms
(`/* a /* b */`) are caught too.

Also fixed: `SqlFragment::new`'s doc now lists all three checks it bypasses
(comment, leading joiner, brackets) rather than brackets alone; the
`fragment_brackets_unbalanced` doc block, which had been left attached to
`fragment_starts_with_joiner`; the `sql_fragment!` doc, which listed only the
bracket check; the README scope line claiming "no SQL parsing beyond locating
markers", which stopped being true once the stripped-view scanning landed; and
the leading-joiner error message, which showed a fragmentary `FROM t AND b = 1`.

One reported item was judged inaccurate: the claim that `find_comment`'s doc
misleads about backslash handling. I said the doc described its own choice
correctly and that the real issue was only the *divergence* from
`strip_literals`.

> That defence does not survive the fifth pass. `find_comment` never sees raw
> SQL — both callers hand it a `strip_literals` view — so its own careful
> backslash reasoning was unreachable, and the divergence it documented was
> load-bearing in the wrong direction. See N.

CI added at [.github/workflows/ci.yml](.github/workflows/ci.yml): clippy and the
workspace suite with `-D warnings`, plus a separate e2e job. Both non-e2e
commands verified locally; the e2e job is unverified here as it needs Docker.

Coverage: 4 macro unit tests (blank-not-delete, no-op when comment-free,
literals preserved, unterminated rejected), 3 integration tests (comment
stripped, token separation kept, literal markers survive), 1 compile-fail
fixture. Suite: 248 non-e2e tests, clippy clean under `-D warnings` — verified
by exit code, which caught a `?`-operator lint in the new code.


---

# Fourth review pass

## K. Comments and literals were scanned in layers — high, fixed

`fragment_comments_blanked` and `fragment_comment_unterminated` were built as a
pipeline: `strip_literals` first, then look for comments in its output. That is
wrong in principle, not just in detail. **Literals and comments are mutually
exclusive contexts, and each hides the other's delimiters.** A scanner that
resolves one before the other has already lost the information the second needs.

`strip_literals` has no concept of a comment, so a quote written *inside* one
opened a literal that ran past the comment's terminator:

| Fragment | Before | Correct |
| --- | --- | --- |
| `c = 1 /* it's */ AND d = 2` | rejected: "unterminated" | accepted |
| `/* ' */ a = 1` | rejected: "unterminated" | accepted |
| `/* $tag$ */ a = 1` | rejected: "unterminated" | accepted |
| `c = 1 -- it's` | `c = 1      's` | `c = 1` |
| `c = 1 -- 'x' = 'y'` | `c = 1    'x'   'y'` | `c = 1` |
| `c = 1 /* "x" */ AND d = 2` | `c = 1    "x"    AND d = 2` | `c = 1  AND d = 2` |
| `c = 1 -- é` | `c = 1    é` | `c = 1` |

Two distinct failures, both confirmed by running the code:

- **False rejection.** A closed comment containing one unpaired quote or a
  `$tag$` reads as unterminated. `/* it's */` is an ordinary comment, so this
  rejected valid, idiomatic SQL.
- **Comment text reaching the query.** Where the phantom literal was blanked by
  `strip_literals`, the mask-diff in `fragment_comments_blanked` could no longer
  tell comment bytes from literal bytes and left them in place. The `é` case had
  a second cause: `strip_literals` blanks non-ASCII bytes, so the diff read them
  as already-blank and skipped them.

The existing tests missed all of it because every comment fixture was
quote-free — precisely the case the layering handles correctly.

**Fix.** One stateful pass, `FragmentLex::scan`, emitting both views at once:
`comments_blanked` (source with comment bytes spaced out, literals verbatim) and
`structure` (uppercased, literals *and* comments spaced out). Offsets are
preserved byte for byte in both. All four fragment checks now read it;
`blank_comments` is gone. Inside a comment, quotes, `$tag$` and non-ASCII bytes
are comment text and only the terminator is sought — which is what the pipeline
could not express.

## L. Whitespace was trimmed only when a comment was present — medium, fixed

`fragment_comments_blanked` ended in `.trim()`, and the macro applied it via
`unwrap_or`, so a fragment's outer whitespace was dropped **iff** it contained a
comment. A fragment is spliced verbatim, so those edges are the separators
between it and the template:

```rust
const F: SqlFragment = sql_fragment!(" a = 1 -- x");
query!("SELECT * FROM t WHERE#{F} AND b = 1")
// before: SELECT * FROM t WHEREa = 1 AND b = 1
```

Adding a comment silently changed the emitted SQL elsewhere in the fragment.
Trimming removed; whitespace is now never touched. The visible cost is that a
blanked comment leaves a run of spaces in the output — which is the same
already-accepted cost as `1/*x*/AND` not collapsing, and is now pinned by tests
and shown in the docs.

## M. A fragment contributing no SQL — low, fixed

`sql_fragment!("-- only a note")` blanked to the empty string and was accepted,
splicing nothing. `WHERE #{F}` then reached PostgreSQL as a bare `WHERE`: a
runtime error naming the template, not the fragment behind it. Now rejected at
compile time by `fragment_is_empty`, which reads the comment-blanked text so the
check is on what the fragment would actually contribute.

## Coverage

New: 4 macro unit tests (quote/`$tag$`/quoted-identifier inside a comment,
non-ASCII comment bytes, empty fragments, `--` not closing a block comment),
3 integration tests (quote inside a comment across `Some`/`None`, edge
whitespace preserved), 1 compile-fail fixture (`fragment_only_a_comment`). The
existing blanked-comment assertions were updated to the untrimmed output.

Suite: 253 non-e2e tests; `cargo clippy --workspace --all-targets` clean under
`-D warnings`, verified by exit code. Every repro in the report was run against
the built macro before and after.


---

# Fifth review pass

## N. `\` was treated as an escape in every quoted literal — high, fixed

Both scanners advanced past `\x` inside `'...'` and `"..."`. Under PostgreSQL's
default `standard_conforming_strings = on` that is wrong, and the consequence is
the opposite of what the previous passes claimed.

Verified against PostgreSQL 16 rather than from memory:

| Query | Result | Meaning |
| --- | --- | --- |
| `select 'a\'` | `a\` | ordinary literal: `\` is data, literal is **complete** |
| `select 'a\' -- c` | `a\` | the `--` is a **real comment** |
| `select 1 as "a\" -- c` | `1` | quoted identifiers never escape with `\` |
| `select E'a\' -- c'` | `a' -- c` | extended string: `\'` **is** an escape |
| `select text'a\'` | `a\` | a *type-prefixed* literal does **not** escape |
| `select U&'a\'` | error | `\` starts a codepoint there, never a quote escape |

So `'a\'` ends at the second quote, and everything after it is SQL. Treating the
literal as still open hid that SQL from every check built on these views:

```rust
const F: SqlFragment = sql_fragment!(r"s = 'a\' -- c");
query!("SELECT * FROM t WHERE a = ${?x} AND #{F} AND b = 1")
// emitted: ... s = 'a\' -- c AND b = 1     <- `AND b = 1` commented out
```

Four confirmed consequences, all reproduced before the fix:

- **Silently wrong rows.** A comment survived into the query and commented out
  the predicate after it. Both in a fragment and — bypassing the whole-template
  comment ban — in a `${?...}` template directly.
- **Unterminated `/*` accepted.** `sql_fragment!(r"s = 'a\' /*")` compiled.
- **Unbalanced bracket accepted.** `sql_fragment!(r"s = 'a\' )")` compiled, and
  the stray `)` closed a bracket belonging to the template.
- **Quoted identifiers.** `"a\"` needed no ambiguity argument at all — backslash
  is never an escape there.

**My earlier reasoning was wrong, not merely incomplete.** Passes three and four
recorded this as a deliberate trade whose cost was a false *rejection* — "a
compile error on working SQL, never silently wrong SQL". I reasoned about the
scan running *past* a closing quote and stopping too late, but never traced what
it swallows on the way: if that text holds a comment, a bracket or an unclosed
`/*`, over-extension hides it. That is a false *acceptance*. The direction I
called safe was the unsafe one.

**Fix.** One shared pair of helpers, `opens_extended_string` and `quoted_end`,
used by both `FragmentLex` and `strip_literals`, so the two can no longer
disagree about where a literal ends:

- ordinary `'...'` / `"..."` — only a doubled quote escapes;
- `E'...'` / `e'...'` — backslash escapes, preserved as before;
- the `E` must be a standalone token, since `text'a\'` does not escape;
- `U&'...'` needs nothing: `\` there is a codepoint escape, and a trailing `\`
  is an error in PostgreSQL, not a quote escape.

`find_comment` lost its own quote scanning entirely. Both callers pass a
`strip_literals` view, so its literal handling was unreachable code defending a
divergence that no longer exists. `standard_conforming_strings = off` is not
supported; the ordinary-literal reading is the documented default.

## O. Repeated scans in `sql_fragment!` — minor, fixed

The macro called five free functions, each running its own `FragmentLex::scan`.
Now it scans once and calls methods on the result. The free functions remain as
`one_shot` wrappers under `#[cfg(test)]`, where a scan per assertion costs
nothing and reads better.

Also fixed: the `E` prefix no longer survives in the `structure` view as a stray
keyword byte.

## Coverage

New: 3 macro unit tests (no escape in ordinary literals and quoted identifiers;
escape preserved in `E'...'`/`e'...'`; only a standalone `E` prefixes, covering
the start-of-input boundary), 2 integration tests (fragment across `Some`/`None`,
`E'...'` preserved), 1 compile-fail fixture for the template-level bypass.

The three new unit tests were mutation-checked: restoring `let escapes = true`
makes two of them fail, so they discriminate rather than merely pass.

Suite: 258 non-e2e tests at the time of that pass; clippy clean under `-D warnings`, verified by exit
code.

`cargo fmt --all --check` is still not gated in CI, and this is deliberate. It
fails on files predating the workflow — `examples/`, `tests/e2e.rs`,
`tests/expand.rs` and four others — none of which this pass touched.
Reformatting them would be unrelated churn in a review-fix commit, so the gate
is left out with a comment in the workflow saying why. Every file this pass
*did* touch is rustfmt-clean.

## Sixth review pass

Five low findings, all confirmed by repro before any change. No highs or mediums
this round.

## P. rustdoc was broken in `sqlx_dyn_macros` — low, fixed

`sql_fragment!`'s public doc linked to five `parse::fragment_*` functions that
pass O had moved inside `#[cfg(test)] mod one_shot`, plus `parse::FragmentLex` in
a private module. Six rustdoc errors:

```
$ RUSTDOCFLAGS="-D warnings" cargo doc --workspace --no-deps
error: unresolved link to `parse::fragment_comments_blanked`
error: public documentation for `sql_fragment` links to private item `parse::FragmentLex`
```

Self-inflicted by pass O and not caught because CI does not run `cargo doc`. The
links are gone; the doc now names the checks in prose and points at
`--document-private-items` for the implementation. Fixed by removing links, not
by re-exporting: the scan is an implementation detail and should stay private.

Gating `cargo doc` in CI is a separate change and is not part of this pass — the
same reasoning as the `cargo fmt` gate, but with the opposite conclusion
available, since `cargo doc` now passes on the whole workspace. Left for the
next commit that touches CI.

## Q. `opens_extended_string` ignored non-ASCII token boundaries — low, fixed

The standalone-`E` check tested `is_ascii_alphanumeric() || b'_'`, so a non-ASCII
byte before the `E` read as "not identifier material" and the `E` passed as a
prefix:

```rust
sql_fragment!(r"s = éE'a\' -- c")   // -> "s = éE'a\\' -- c", comment not stripped
```

The literal over-extended past its closing quote and took the comment with it.
Low because `éE` is not a real type name, so the SQL was already invalid — but
the failure direction was wrong: false acceptance, not false rejection. A
non-ASCII byte now blocks the prefix. Whether `éE` names a type cannot be decided
in a lexer that does not know the schema, and the conservative reading is the one
that does not hide SQL.

## R. Unclosed `'` / `"` / `$tag$` were accepted — low, fixed

`sql_fragment!("'")` and `sql_fragment!(r"$q$abc")` both compiled. This
contradicted the model the other checks follow: unclosed `/*`, unbalanced
brackets, empty fragment and leading joiner were all rejected, but an unclosed
literal — which over-extends in exactly the same way, one level out — was not.

Now rejected via `FragmentLex::literal_unterminated`. `quoted_end` returns
`Option<usize>` instead of the `bytes.len()` sentinel, because a literal closing
on the final byte and a literal that never closes both ended at `bytes.len()` and
only the caller knows whether the difference matters. `strip_literals` keeps the
consume-to-end behaviour explicitly.

This cannot produce silent wrong rows — PostgreSQL rejects the statement. It is
rejected anyway because a fragment is validated so that it is safe to splice
anywhere, and this one is not: whether it parses depends on the template that
happens to follow it.

## S. `U&'...'` — low, no code change needed

The report suggested a special case. None is required, and adding one would be
wrong. `U&'...'` uses `\` for codepoints (`\0041`), never to escape a quote, so
it is already lexed correctly as an ordinary literal — `U&'a\0041' -- c` strips
its comment, and `U&'a\'` is a PostgreSQL error ("invalid Unicode escape",
verified against PostgreSQL 16 in the previous pass), not an escape the lexer
could mishandle. Documented in the literal-end table rather than special-cased.

## T. Markers are found before SQL is lexed — low, documented

`parse_template` scans the raw template, so a `${...}` inside a string literal or
a SQL comment still interpolates. This is intended — interpolation is a text
layer above SQL, which is why `$${` and `##{` escapes exist at all — but it was
only implied by the escaping example, never stated. Now stated in both the crate
docs and the README, next to the lexical rules it is easy to confuse with.

## Coverage

New: 3 macro unit tests (non-ASCII before `E`; unclosed literals across `'`,
`"`, `$tag$` and a doubled-quote case; closed literals not misreported, covering
the close-on-last-byte boundary), 3 integration tests (`éE`, `U&`, literal
closing at the fragment's edge), 1 compile-fail fixture for the unclosed literal.

Both code fixes were mutation-checked: dropping the non-ASCII guard fails
`a_non_ascii_byte_before_e_does_not_make_it_a_prefix`, and hardcoding
`literal_unterminated` to `false` fails `an_unclosed_literal_is_rejected`.

Suite: 264 non-e2e tests at the time of that pass. clippy clean under `-D warnings` and `cargo doc` clean
under `RUSTDOCFLAGS="-D warnings"`, both verified by exit code.

Not addressed: version is still 0.1.2. Bumping it is a release decision, not a
review fix, and belongs with whoever cuts the release.


# Seventh review pass

Two findings, both reproduced before any change. The first is the first genuine
high since the fourth pass.

## U. The predicate tail ignored bracket depth — high, fixed

`predicate_tail_end` stopped at *any* `)` and at *any* clause keyword, including
ones inside a group the tail had opened itself. The code comment claimed a
bracket "always ends the predicate: the marker is inside something we did not
open" — untrue, since a `(` can open after the marker, in the tail.

Reproduced:

```
SELECT * FROM o WHERE total = ${?x} + coalesce(tax, 0)
  Some -> SELECT * FROM o WHERE total = $1 + coalesce(tax, 0)
  None -> SELECT * FROM o)

SELECT * FROM t WHERE a = ${?x} IN (SELECT 1 UNION SELECT 2)
  Some -> SELECT * FROM t WHERE a = $1 IN (SELECT 1 UNION SELECT 2)
  None -> SELECT * FROM t UNION SELECT 2)
```

The group was cut in half and its `)` stayed behind as mandatory text. Not
silent wrong rows — PostgreSQL rejects the statement — but the same class as the
earlier highs: compile-time acceptance producing broken runtime SQL, and a
direct breach of "reject when unsure". `coalesce(...)` in a predicate is
ordinary SQL, not an exotic shape.

The report offered two fixes: (A) reject a balanced group in the tail, (B) track
depth and only honour `)`/keywords at depth zero. B, because depth tracking is
needed either way to tell a `)` closing *our* group from one closing a group
opened before the marker — and once the scanner has that, refusing balanced
groups would be a deliberate extra rejection buying nothing. B also preserves
`EXISTS (SELECT 1 WHERE k = ${?x})`, where the `)` is at depth zero.

Square brackets count toward depth too: a subscript is a group like any other.

One case the report's sketch did not cover: a `(` in the tail that never closes.
The tail must not take text past it — the marker then sits inside a group opened
before it, and swallowing the rest would hide SQL. `unclosed_at` records where
such a group opened and the tail ends there. That template is already malformed
in the `Some` branch too (`SELECT * FROM t WHERE a = $1 + f(b`), so no valid
query is affected; what matters is that the `None` branch does not look
well-formed when it is not. Pinned by
`a_group_left_open_in_the_tail_is_not_swallowed`.

## V. `split_predicate` ignored square brackets — low, fixed

`SELECT * FROM t WHERE a = 1 AND b[1,2] = ${?x}` was rejected: the comma inside
the subscript read as a top-level list separator. False rejection, not wrong SQL,
so low — but it is the same bracket logic as U, and fixed in the same pass.
`'(' | '['` and `')' | ']'` now both count, and the unbalanced-group fallback
looks for either opener.

## Coverage

New: 5 integration tests — a group opened in the tail taken whole; a keyword
inside such a group not read as a clause boundary; a real clause keyword after
the group still ending the predicate (three shapes: `ORDER BY`, nested
`f(g(h, 1), 2) AND`, `b[1] AND`); an unclosed group in the tail not swallowed;
a subscript before the marker counting toward depth.

All four mutations were checked and each fails a test: making `)` end the tail
unconditionally fails three of the new tests; dropping the `depth == 0` guard on
keywords fails the `UNION`-in-subquery test; replacing the `unclosed_at`
fallback with `stripped.len()` fails the open-group test; and dropping `[`/`]`
from `split_predicate` fails the subscript test at compile time.

Suite: 270 non-e2e tests at the time of that pass. clippy clean under `-D warnings`, `cargo doc` clean
under `RUSTDOCFLAGS="-D warnings"`, all three examples run, both touched files
rustfmt-clean — all verified by exit code.

Docs: the `predicate_tail` doc comment stated the old (wrong) rule and now states
the depth-relative one; the crate docs and README gained the trailing-text rule
with the three shapes above as runnable assertions.

Not addressed: version is still 0.1.2 — but this pass fixes a high, so a release
cut from it is a bugfix release and the bump belongs with it.


## W. A real marker inside the predicate tail was not rejected — high, fixed

Found in the same place as U, but pre-existing rather than introduced by it.

```
SELECT * FROM t WHERE a = ${?x} || ${y}
  Some -> SELECT * FROM t WHERE a = $1 || $2
  None -> SELECT * FROM t $1

SELECT * FROM t WHERE a = ${?x} + f(${y})
  None -> SELECT * FROM t($1)
```

`predicate_tail` stops at the next interpolation, which is right — its text is
not ours to copy. What was missing is the distinction between *why* it stopped.
If a boundary was passed first, the interpolation belongs to the SQL after the
predicate and both parts stand alone (`a = ${?x} AND b = ${y}` is fine). If the
tail reached the interpolation without a boundary, the predicate straddles it and
cannot be emitted or removed as one unit.

`predicate_tail_end` now returns `(offset, stopped_at_boundary)` and
`predicate_tail` returns a `Tail { len, split_by_interpolation }`. The report
proposed a three-variant enum (`Boundary` / `EndOfInput` / `HitMarker`); a bool
is enough, because `EndOfInput` versus `HitMarker` is already answered by
`limit < rest.len()` at the call site and a third variant would only duplicate
it.

An unbalanced `(` counts as a boundary here: the marker sits inside a group
opened before it, so text past that `(` is not part of this predicate and an
interpolation out there is not ours.

### `tail_is_truncated_by_escape` deleted

The report predicted the stop-reason check would subsume it, and it does. That
function existed only because the old code could not tell "stopped at an escape"
from "stopped at a boundary", so it inferred the answer from an open literal at
the end of the captured tail. All three `opt_escape_*` compile-fail fixtures
still fail, now on the general check, and their `.stderr` files were regenerated.
Made unused by this edit, so removed per the dead-code rule.

This also fixed a case none of those fixtures covered:
`WHERE a = ${?x} $${z}` — an escape with no boundary before it and no literal
around it — emitted `SELECT * FROM t ${z}`. It is now rejected.

### Where the report's fix was too broad

Rejecting *every* interpolation reached without a boundary breaks two existing,
documented, tested patterns:

```
WHERE org = ${org} AND a = ${?x} #{ORDER_BY_ID}   -> ... ORDER BY id
WHERE a = ${?n} #{ACTIVE}                          -> ... WHERE deleted_at IS NULL
```

A `#{...}` fragment's SQL is opaque to the scanner by design, and it may *be* the
boundary that ends the predicate. Whether it is instead a continuation
(`SqlFragment::new("|| 'x'")`, which does produce `SELECT * FROM t || 'x'`)
cannot be decided from the template, and may be chosen at runtime. So a bare
`#{` is exempt, and the case falls under the constraint the crate already
documents rather than enforces for fragments alongside `${?...}`. Both forms of
that constraint are now stated: no top-level clause boundary from a fragment, and
— new — a fragment in a predicate's trailing text must be a boundary rather than
a continuation.

Escapes are *not* exempt: `##{` is literal text, not SQL, so it can never be the
boundary.

## Coverage

New: 1 integration test (a boundary before the next marker keeps both predicates
whole, covering both `AND` and a depth-zero `)`), 1 compile-fail fixture
(`opt_marker_in_predicate_tail`), and a comment on
`a_trailing_clause_fragment_survives_a_dropped_optional` recording why the
fragment case is deliberately exempt. Three `opt_escape_*` fixtures updated:
`.stderr` regenerated for the new message, and their comments no longer describe
the deleted open-literal heuristic.

Three mutations checked, each fails a test: hardcoding `split_by_interpolation`
to `false` fails the compile-fail fixture; dropping the `!stopped_at_boundary`
guard fails the safe-shape tests; dropping the `#{` exemption fails the two
fragment tests. The latter two fail at compile time, since the mutation makes
valid templates error.

Suite: 274 non-e2e tests at the time of that pass. clippy clean under `-D warnings`, `cargo doc` clean
under `RUSTDOCFLAGS="-D warnings"`, all three examples run, touched files
rustfmt-clean — all verified by exit code.

Docs: the crate docs and README gained the rejection with its safe counterpart,
and the fragment section gained the mirror obligation in both files.


# Eighth review pass

One critical, found by review of the pass-seven fix. The guard it added did not
cover the case its own test fixture claimed as covered.

## X. Interpolation nested in a tail-opened group escaped the guard — critical, fixed

```
SELECT * FROM t WHERE a = ${?x} + f(${y})
  Some -> SELECT * FROM t WHERE a = $1 + f($2)
  None -> SELECT * FROM t($1)
```

Reproduced before any change, and it is the exact template
`opt_marker_in_predicate_tail.rs` asserted was caught. That comment was written
from a probe run *before* the `#{` exemption landed and never re-checked — the
claim was false when committed. Recorded here because a fixture comment that
overstates coverage is worse than no comment: it stops the next reader from
testing the case.

Root cause, as the review stated it: `unclosed_at` is set only by a depth-zero
`(`, and the scan starts at depth zero relative to the marker, so it only ever
records a group the *tail itself* opened. A group opened before the marker
closes through the depth-zero `)` branch. The pass-seven code returned
`(at, true)` — "stopped at a boundary" — for that state, and the doc comment
justified it with the inverted premise that the group was opened before the
marker. So when a tail-opened group swallowed the capping interpolation, the
predicate was declared separable when it was not.

The minimal fix is to report no boundary there. `len` still cuts at the `(`, and
the "no interpolation at all" case (`a = ${?x} + f(b`) is unchanged because the
`capped` guard gates it — verified byte-identical output before and after.

`(usize, bool)` became `(usize, TailStop)` with `Boundary` / `EndOfText` /
`InsideGroup`. Pass seven rejected an enum on the grounds that it duplicated
`limit < rest.len()` at the call site; that was wrong. `InsideGroup` is a third
state neither the bool nor the call-site check can express, and conflating it
with `Boundary` is precisely this bug.

### A second case the review did not flag

The review kept `f(#{FRAG})` exempt, "consistent with the design". It is not:

```
SELECT * FROM t WHERE a = ${?x} + f(#{F})   // F = "1"
  None -> SELECT * FROM t(1)
```

The exemption exists because a top-level fragment may *be* the clause boundary
that ends the predicate. One nested in a tail-opened group cannot be — it is
positionally inside the predicate whatever SQL it carries — so the premise does
not hold and the exemption should not apply. It is now conditioned on
`TailStop::EndOfText`, which is exactly "the tail reached the interpolation at
its own top level".

### The nit was wrong

The review called `limit < rest.len()` redundant with `next_marker` semantics,
"not load-bearing". Tested: replacing it with `true` breaks six valid templates.
`next_marker` returns `None` when the tail holds no interpolation and
`unwrap_or` then sets `limit == rest.len()`, so the guard is what separates "no
interpolation" from "capped by one". Kept, with a comment recording the test so
it is not removed later.

## Coverage

New: 2 compile-fail fixtures — `opt_marker_in_tail_group` (`f(${y})`) and
`opt_fragment_in_tail_group` (`f(#{F})`), the latter documenting why a nested
fragment differs from a top-level one. `opt_marker_in_predicate_tail`'s false
coverage claim removed.

Three mutations checked, each fails a fixture: reverting `InsideGroup` to
`Boundary`; dropping the `EndOfText` condition from the fragment exemption; and
removing the exemption entirely, which breaks the two valid top-level fragment
tests at compile time.

Suite: 276 non-e2e tests. clippy clean under `-D warnings`, `cargo doc` clean
under `RUSTDOCFLAGS="-D warnings"`, rustfmt-clean — verified by exit code.

Docs: the crate docs gained the nested `${...}` and `#{...}` cases as
`compile_fail` examples; `docs/guide.md` had its `#{...}` sentence corrected,
since it stated the exemption without the top-level qualifier.

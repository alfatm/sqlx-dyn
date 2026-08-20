# sqlx_dyn

SQL templates for PostgreSQL. The template is SQL; user input cannot become
SQL text.

```rust
const KIND: SqlFragment = sql_fragment!("'document'");

let rows = sqlx_dyn::query!(r#"
    SELECT id, name FROM users
    WHERE org_id = ${org}
      AND kind = #{KIND}
      AND age >= ${?min_age}
    ORDER BY id
"#)
.fetch_all(&pool)
.await?;
```

## The missing link between `sqlx::query!` and plain SQL

Some queries resolve their shape at runtime: sort column, active filters, `IN`
list length, presence of a `WHERE` clause.

`sqlx::query!` cannot express these. It validates SQL against the schema at
compile time, and that check requires the string to be complete at compile
time. So `query!` covers exactly the queries whose text is fully known.

Before 0.9 the fallback was `format!`. That frees the shape, but it also moves
the data/SQL boundary onto the caller: whatever you interpolate becomes SQL
text.

```rust
// sqlx 0.8: two optional filters, assembled as a string
let mut w = Vec::new();
if let Some(v) = name { w.push(format!("name ILIKE '{v}'")); }
if let Some(v) = min_age { w.push(format!("age >= {v}")); }
let clause = if w.is_empty() { String::new() }
             else { format!(" WHERE {}", w.join(" AND ")) };
sqlx::query(&format!("SELECT id, name FROM users{clause} ORDER BY id"))
```

Every value here is SQL text, so correctness now depends on what the caller
passed.

sqlx 0.9 removes that option. `sqlx::query()` takes `impl SqlSafeStr`, which is
implemented for `&'static str` only, so the code above no longer compiles. The
supported route is `QueryBuilder`, which turns the query into a call sequence.
The same two filters:

```rust
let mut b = QueryBuilder::<Postgres>::new("SELECT id, name FROM users");
let mut first = true;
if let Some(v) = name {
    b.push(if first { " WHERE " } else { " AND " });
    first = false;
    b.push("name ILIKE ");
    b.push_bind(v);
}
if let Some(v) = min_age {
    b.push(if first { " WHERE " } else { " AND " });
    b.push("age >= ");
    b.push_bind(v);
}
b.push(" ORDER BY id");
```

Values are safe here, since binds stay binds. The costs are three:

- The query is no longer readable as SQL.
- `WHERE` versus `AND` is tracked by hand, through the `first` flag.
- `push` accepts any `String`, so the data/SQL boundary is a convention rather
  than a compile-time constraint.

`sqlx_dyn` holds all three properties at once: runtime shape, readable SQL, and
a compiler-enforced data/SQL boundary. You write the query as SQL and mark the
varying parts; the macro emits the `QueryBuilder` calls above.

```rust
query!(r#"
    SELECT id, name FROM users
    WHERE name ILIKE ${?name}
      AND age >= ${?min_age}
    ORDER BY id
"#)
```

This produces byte-identical SQL to the hand-written block, for every
combination of present and absent filters.

Sort order uses the same mechanism. Fragments are constants, so the choice is a
`match`:

```rust
const BY_NAME: SqlFragment = sql_fragment!("name ASC");
const BY_DATE: SqlFragment = sql_fragment!("created_at DESC");

let order = match sort { Sort::Name => BY_NAME, Sort::Date => BY_DATE };
query!("SELECT id, name FROM users ORDER BY #{order}")
```

Against the `QueryBuilder` version, this adds three compile-time checks:

- SQL text is type-restricted. `push` accepts any `String`. `#{}` accepts only
  `SqlFragment`, constructed from `&'static str`. A runtime string in SQL
  position fails to compile. See
  [SQL injection model](#sql-injection-model).
- Marker positions are validated. If the macro cannot determine what a `${?}`
  would remove, it rejects the template: inside a function call, on one side of
  `BETWEEN`, in the `SELECT` list. The error names the position.
- Parameter numbering is not hand-maintained. `$N` is assigned by
  `QueryBuilder` as binds are pushed, so edits cannot desynchronize numbering
  from bind order.

The expansion is those same `QueryBuilder` calls, with no intermediate `String`
and no `format!`. `tests/allocations.rs` fails if the macro ever allocates more
than the hand-written equivalent.

## Keep using `sqlx::query!` for static queries

`query!` checks columns and types against the schema at compile time. This
crate does not read the schema; decoding goes through `FromRow`, so a renamed
column fails at runtime.

Use `query!` for fixed SQL. Use this crate when the query text is not fully
known at compile time: conditional filters, caller-selected sort columns,
variable-length `IN` lists.

## Interpolation

| Syntax     | Becomes                      | Accepts           | User input  |
| ---------- | ---------------------------- | ----------------- | ----------- |
| `${expr}`  | `push_bind(expr)`            | `Encode + Type`   | ✅ safe     |
| `${?expr}` | predicate, dropped on `None` | `Option<T>`       | ✅ safe     |
| `#{expr}`  | `push(expr.as_sql())`        | `SqlFragmentLike` | ⚠️ SQL text |

PostgreSQL parameter numbering (`$1`, `$2`, …) is done by `QueryBuilder`. This
crate never generates `$N` itself, so adding a fragment never shifts the
numbering of your binds.

## Optional predicates

`${?expr}` takes an `Option`. `None` drops the predicate together with its
`AND`/`OR`, and `WHERE` disappears when nothing is left. The template reads as
the finished SQL.

```rust
query!(r#"
    SELECT id, name FROM users
    WHERE name ILIKE ${?name}
      AND age >= ${?min_age}
      AND organization_id = ${?org}
    ORDER BY id
"#)
```

`ORDER BY id` survives in every case. The `WHERE` varies.

- all `Some`: `WHERE name ILIKE $1 AND age >= $2 AND organization_id = $3`
- only `min_age`: `WHERE age >= $1`
- all `None`: no `WHERE` clause at all

A dropped predicate leaves no hole in the numbering, because `QueryBuilder`
assigns `$N` as binds are pushed.

Unconditional predicates can sit on either side of an optional one. The joining
keyword comes from what survived, so a dangling `AND` cannot occur.

```rust
// missing == None
query!("SELECT * FROM t WHERE a = ${?missing} AND b IS NULL")
// -> "SELECT * FROM t WHERE b IS NULL"     (not "... FROM t AND b IS NULL")
```

Removal is well defined only for a whole top-level predicate. `${?...}` must be
the right-hand side of a comparison (`=`, `<>`, `<`, `>`, `<=`, `>=`, `LIKE`,
`ILIKE`) introduced by `WHERE`, `AND`, or `OR`. Anything else is a compile
error rather than silently broken SQL.

```rust
query!("SELECT * FROM t WHERE age BETWEEN ${?lo} AND ${?hi}");
// error: `${?...}` must be the right-hand side of a comparison,
//        but the SQL before it ends with `BETWEEN`.
```

This crate does not parse SQL, so it refuses positions it cannot reason about
rather than guessing how much text to remove. For those, use a plain `${...}`
bind or `builder_mut()`.

## SQL injection model

`${expr}` is safe for untrusted data. The value is sent out-of-band as a
parameter and is never parsed as SQL.

`#{expr}` is SQL text, so it is restricted to `SqlFragmentLike`. That trait is
sealed. It is implemented for `SqlFragment` and references to it, and cannot be
implemented downstream. `SqlFragment` is constructible only from a
`&'static str`, so this does not compile:

```rust
let filter: String = request.filter;
query!("SELECT * FROM users WHERE #{filter}");
// error: `String` does not implement `SqlFragmentLike`
```

There is no `From<String> for SqlFragment` and no safe constructor taking a
runtime string. Dynamic choices are expressed by selecting among constants.

```rust
const ORDER_CREATED: SqlFragment = sql_fragment!("created_at DESC");
const ORDER_NAME: SqlFragment = sql_fragment!("name ASC");

let order = match sort {
    Sort::Created => ORDER_CREATED,
    Sort::Name => ORDER_NAME,
};
query!("SELECT * FROM users ORDER BY #{order}")
```

Internally a fragment is a `Cow<'static, str>`. Static fragments borrow, so
they allocate nothing and leak nothing. Runtime fragments are out of scope for
this version. Adding them later would require an explicitly marked
constructor.

### What this guarantees, precisely

What is ruled out is the accidental path. There is no coercion, no
`From<String>`, and no `Display` blanket impl. Because the trait is sealed,
there is also no downstream impl. A runtime string reaches SQL text only if the
author writes it deliberately. `${expr}` carries no such caveat and is safe
unconditionally.

Two deliberate routes stay open by design.

- `SqlFragment::new(s.leak())`. `str::leak` is safe stable Rust. Closing this
  route would mean rejecting `&'static str`, the only thing a fragment holds.
  Being a `const fn`, `new` also cannot run the bracket check that
  `sql_fragment!` applies ("Fragments and optional predicates" below). Prefer
  the macro.
- `builder_mut().push(s)`. `QueryBuilder::push` takes `impl Display`. The
  escape hatch keeps unusual SQL expressible.

Both are visible at the call site, so neither happens by mistake. Treat them as
you would `unsafe`. Untrusted text belongs in `${expr}`.

## API

| Item                     | Purpose                             |
| ------------------------ | ----------------------------------- |
| `query!(sql)`            | untyped; rows as `PgRow`            |
| `query_as!(T, sql)`      | rows decoded via `T: FromRow`       |
| `query_scalar!(sql)`     | single column                       |
| `sql_fragment!(literal)` | `SqlFragment` from a string literal |
| `SqlFragment`            | the raw-SQL type                    |
| `SqlFragmentLike`        | its sealed trait                    |

### Fragments and optional predicates

A `#{...}` marker is opaque to the template scanner. It sees the marker, never
the SQL the fragment will supply — the fragment may be a `const`, a function
call, or a `match` arm chosen at runtime. Clause bookkeeping for `${?...}` is
therefore built from the template alone.

`sql_fragment!` checks one thing: that the fragment's brackets balance within
it. An unmatched bracket is not just broken SQL of the author's own making — it
reaches into the _template's_ nesting, where a `)` can close a construct the
fragment never opened. `sql_fragment!("a = 1) AND (b = 2")` is rejected at
compile time even though its bracket counts match.

Clause keywords are **not** checked, because they cannot be judged from the
fragment alone. How deep a fragment lands is a property of the template:

```rust
// The body is the reusable half; the template supplies the brackets.
const TREE: SqlFragment = sql_fragment!(
    "SELECT id FROM t WHERE parent IS NULL \
     UNION ALL \
     SELECT c.id FROM t c JOIN tree ON c.parent = tree.id"
);
query!("WITH RECURSIVE tree AS (#{TREE}) SELECT * FROM tree WHERE id = ${?id}");
```

That `UNION ALL` is top-level within the fragment and nested once the template
wraps it, so rejecting it would be a false rejection of valid SQL.

The consequence is a constraint this crate documents rather than enforces:

> A fragment used in a template that also contains `${?...}` must not introduce
> a **top-level** clause boundary — `UNION`, `INTERSECT`, `EXCEPT`, `HAVING`,
> `QUALIFY` or `;` — at the depth the template inserts it.

#### What goes wrong

The boundary opens a predicate list the template never counted, so the next
optional predicate is bookkept against the wrong clause:

```rust
const F: SqlFragment = SqlFragment::new("deleted_at IS NULL UNION SELECT x FROM u");
query!("SELECT x FROM t WHERE a = ${?p} AND #{F} AND b = ${?q}").sql();

// p = None, q = Some(2):
// SELECT x FROM t WHERE deleted_at IS NULL UNION SELECT x FROM u AND b = $1
//                       ^^^^^ first select                       ^^^^^^^^^
//                                             second select: needs WHERE, got AND
```

The `UNION` starts a second select whose `WHERE` has not been written yet, but
the macro thinks `b` still belongs to the first select — where the mandatory
`WHERE` is already emitted — so it joins with the written `AND`.

#### How to recognise it

Three signals, in order of how early you see them:

1. `.sql()` shows a clause keyword coming _from a fragment_ with an `AND`/`OR`
   after it that has no `WHERE` in between.
2. PostgreSQL fails with a syntax error at or after the boundary. It never
   returns wrong rows for this — the statement does not parse.
3. It appears only for some `Option` combinations: all-`Some` or all-`None`
   often produce valid SQL, and one mixed case does not.

#### How to fix it

A fragment is for the part you reuse — a predicate, an ordering, a join. The
query's _shape_, including any `UNION`, belongs in the template. Splitting it
that way makes the fragment more useful, not less: the same predicate can then
apply on both sides of the boundary.

```rust
// Before: the fragment carries both a predicate and the query's shape, so the
// scanner cannot see the UNION and `q` gets the wrong joiner.
const F: SqlFragment = SqlFragment::new("deleted_at IS NULL UNION SELECT x FROM u");
query!("SELECT x FROM t WHERE a = ${?p} AND #{F} AND b = ${?q}");

// After: the template owns the UNION; the fragment is just the predicate, and
// it is reused in both selects.
const ACTIVE: SqlFragment = sql_fragment!("deleted_at IS NULL");
query!("SELECT x FROM t WHERE a = ${?p} AND #{ACTIVE} \
        UNION SELECT x FROM u WHERE #{ACTIVE} AND b = ${?q}");
// p = None, q = Some(2):
// SELECT x FROM t WHERE deleted_at IS NULL
//   UNION SELECT x FROM u WHERE deleted_at IS NULL AND b = $1
```

If a fragment genuinely must carry a boundary — a generated statement, say —
drop `${?...}` from that template: with plain binds `${...}` there is no
bookkeeping to invalidate. Or assemble that query with `builder_mut()`.

`sql_fragment!` **strips** SQL comments from a fragment. A comment annotates the
fragment; it is not SQL the fragment contributes. Left in place, a trailing `--`
would comment out the template text following the marker — PostgreSQL accepts
that, and the query silently matches different rows.

```rust
const F: SqlFragment = sql_fragment!("c = 1 -- why");
query!("SELECT * FROM t WHERE a = ${?x} AND #{F} AND b = 1");
// -> SELECT * FROM t WHERE a = $1 AND c = 1 AND b = 1
//    the comment is gone; `AND b = 1` survives
```

Comments are blanked to spaces, never deleted, so the tokens they separated stay
separated: `c = 1/* note */AND d = 2` keeps its gap instead of collapsing into
`1AND`. A `--` or `/*` inside a string literal or dollar-quoted body is data and
passes through untouched.

An **unterminated** `/*` is rejected rather than stripped: there is no end to
strip up to, so it would swallow whatever follows the marker.

`sql_fragment!` also rejects a fragment that _starts_ with `AND`/`OR`, for the
same reason: the joiner says how the fragment is combined, not what it is, and
only the template can hand a dropped `WHERE` over to it. Write
`WHERE a = ${?x} AND #{F}`, not `WHERE a = ${?x} #{AND_F}`.

Two cases that look similar but are fine:

- **A fragment's own leading `WHERE`** — `sql_fragment!("WHERE x = 1")` opens the
  very clause the surrounding predicates already belong to, so bookkeeping and
  SQL agree.
- **A boundary nested in brackets** — a subquery or CTE body keeps the
  template's top-level clause count unchanged. That is why `sql_fragment!` does
  not reject boundaries at all.

Both are pinned by tests in `tests/optional.rs`, alongside the broken case
above.

Each macro returns a wrapper (`DynQuery`, `DynQueryAs<T>`, `DynQueryScalar`)
with consuming `fetch_all` / `fetch_one` / `fetch_optional` / `execute` methods
that take an executor, mirroring sqlx. Decoding is delegated entirely to
`sqlx::FromRow`; there is no row mapping here.

`DynQueryScalar` has no `execute`, because sqlx's `QueryScalar` provides none.
`.sql()` returns the assembled SQL for debugging and tests, and needs no type
annotation on any of the three wrappers.

`query_scalar!` takes no type argument: the column type is fixed at the
`fetch_*` call site (`let n: i64 = ...fetch_one(pool).await?`), mirroring
`sqlx::query_scalar`.

## Examples

```bash
cargo run -p sqlx_dyn --example motivating   # the spec's motivating case
cargo run -p sqlx_dyn --example advanced     # filters, pagination, CTEs, arrays
cargo run -p sqlx_dyn --example batch        # INSERT/UPSERT/bulk UPDATE
```

Each example asserts on the SQL it builds, so they fail loudly if codegen
changes rather than quietly documenting something untrue.

- [advanced.rs](sqlx_dyn/examples/advanced.rs): conditional `WHERE` via
  `builder_mut()`, safe dynamic `ORDER BY`, keyset pagination, CTEs + JOINs,
  array binds, optional params, transactions.
- [batch.rs](sqlx_dyn/examples/batch.rs): `push_values` multi-row INSERT,
  parameter-limit chunking, UPSERT with a shared `ON CONFLICT` fragment, `ANY`
  vs expanded IN-lists, `UPDATE ... FROM (VALUES ...)`.

Conditional filters are `${?expr}` (see above). For conditions that do not fit
the "right-hand side of a top-level comparison" shape, `builder_mut()` hands
back the underlying `sqlx::QueryBuilder`; bind numbering continues across that
boundary, so `${...}` in the template and manual `push_bind` calls share one
counter.

There is still no declarative WHERE-DSL and none is planned (§17).

## Evaluation semantics

Each interpolation is evaluated exactly once, in source order, at the point it
appears. Expressions are not deduplicated.

```rust
query!("SELECT * FROM foo WHERE a = ${next_id()} OR b = ${next_id()}")
// calls next_id() twice, produces $1 and $2
```

## Escaping

`$${` yields a literal `${` and `##{` yields a literal `#{`. A `$` or `#` that
is not followed by `{` is never special, so `SELECT $1` and `a # b` need no
escaping.

## Generated code

`query!("SELECT * FROM users WHERE id = ${id} AND #{FILTER}")` expands to
roughly:

```rust
{
    let mut b = QueryBuilder::<Postgres>::new("SELECT * FROM users WHERE id = ");
    b.push_bind(id);
    b.push(" AND ");
    b.push(SqlFragmentLike::as_sql(&FILTER));
    DynQuery::new(b)
}
```

The leading literal chunk seeds `QueryBuilder::new`, static chunks stay string
literals, and the macro itself builds no intermediate `String` and calls no
`format!`.

Cost is identical to hand-written `QueryBuilder` calls. That parity is
asserted by `tests/allocations.rs`, which counts allocations through an
instrumented `GlobalAlloc` and fails if the macro ever allocates more than the
builder calls it expands to:

```
$ cargo test --test allocations -- --nocapture --test-threads=1
macro=7 manual=7
optional=7 equivalent=7
```

The absolute number depends on sqlx internals, so treat it as a reading from
that test rather than a fixed property. The test asserts the relationship,
which is what the spec requires.

This is more than `format!` + a raw query would cost, because `QueryBuilder` in
sqlx 0.9 stores its SQL in an `Arc<String>` that grows per `push` and maintains
a `PgArguments` buffer. The macro adds nothing on top of that; the overhead
belongs to `QueryBuilder`. Note that `format!` + a raw query no longer
compiles in sqlx 0.9 without an explicit `AssertSqlSafe`, so treat it as a
reference point rather than an option. Both figures are noise next to one
network round trip.

## Scope

PostgreSQL only. No compile-time SQL validation, no schema introspection, no
`DATABASE_URL` at build time. There is no SQL parser: the template is scanned
only as far as `${?...}` demands — string literals, dollar-quoted bodies,
comments, bracket depth and clause keywords — which is enough to decide what a
dropped predicate takes with it, and nothing more.
The public API is shaped so other `sqlx::Database` backends can be added later,
but v1 does not abstract over them.

Requires sqlx 0.9 (`QueryBuilder` lost its `'args` lifetime in 0.9; this crate
does not compile against 0.8).

## Tests

```bash
cargo test              # parser unit tests, SQL-shape tests, trybuild, doctests
cargo run -p sqlx_dyn --example motivating
```

Integration tests assert on generated SQL and typecheck the `fetch_*`/`execute`
paths against a real `PgPool` without connecting.

The injection model is tested from both sides:

- `tests/injection.rs` feeds classic payloads (`'; DROP TABLE users; --`,
  `1' OR '1'='1`, and values containing `${`/`#{`/`$1`) through `${...}` and
  asserts they never appear in `.sql()` and never add a parameter slot.
- `tests/compile_fail/` covers the rejected inputs: `String`, `&str`,
  `&'static str`, `Cow<str>`, `Box<str>`, a `format!` result, and an arbitrary
  `Display` type each fail with `E0277` naming `SqlFragmentLike`. The `Display`
  case matters because `QueryBuilder::push` itself accepts `impl Display`.
  `downstream_fragment_impl.rs` covers the sealing itself. A newtype around
  `String` that tries to implement the trait fails on the private supertrait.
- `tests/binds.rs` reads the encoded `PgArguments` back out and asserts
  parameter count, types, order, and values. Every other test asserts the SQL
  text, which cannot distinguish `x = ${a} AND y = ${b}` from
  `x = ${b} AND y = ${a}`.
- `tests/e2e.rs` runs generated queries against a real PostgreSQL 16 in Docker,
  via `testcontainers`. It is behind a feature so the default `cargo test`
  needs no daemon:

  ```sh
  cargo test                     # 248 tests, no Docker
  cargo test --features e2e      # + 16 tests against a real server
  ```

  This is the only layer that proves the generated SQL is valid PostgreSQL
  rather than merely the text intended. The server is the oracle. It covers
  every survival combination of a three-filter template, casts and
  concatenation after a dropped predicate, `WHERE`/`HAVING`/`UNION` as
  independent lists, and injection payloads arriving as data. Reverting either
  optional-predicate fix makes it fail with a real server error (`operator
does not exist: uuid = text`), which is the check no text-comparison test
  can make.

One limit worth stating plainly: that bind values travel out-of-band is a
property of sqlx's wire protocol. These tests verify the values reach sqlx
correctly and that the server treats them as data, not the wire encoding
itself.

## Getting started

```toml
[dependencies]
sqlx_dyn = "0.1.2"
sqlx = { version = "0.9", features = ["postgres", "runtime-tokio"] }
```

Three macros cover most of it: `query!`, `query_as!`, and `query_scalar!`. Each
takes SQL as written. Start by replacing one query whose `WHERE` clause you
assemble by hand. That is where the difference shows first.

## License

MIT

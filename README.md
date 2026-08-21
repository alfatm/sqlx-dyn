# sqlx_dyn

[![CI](https://github.com/alfatm/sqlx-dyn/actions/workflows/ci.yml/badge.svg)](https://github.com/alfatm/sqlx-dyn/actions/workflows/ci.yml)
[![crates.io](https://shields.io/crates/v/sqlx_dyn)](https://crates.io/crates/sqlx_dyn)
[![docs.rs](https://shields.io/docs.rs/sqlx_dyn)](https://docs.rs/sqlx_dyn)
[![MIT](https://shields.io/crates/l/sqlx_dyn)](LICENSE-MIT)

Template macros over `sqlx::QueryBuilder` for PostgreSQL. Write SQL almost as
it is, and interpolate Rust values as bind parameters, optional predicates, or
pre-approved SQL fragments — no `format!`, no hand-rolled `push`/`push_bind`.
Runtime shape, compile-time discipline. Postgres only.

`sqlx::query!` checks queries against the schema at compile time, but the SQL
has to be a compile-time string and a live database is required at build time.
`QueryBuilder` accepts runtime SQL, but the assembly is hand-rolled:

```rust
use sqlx::QueryBuilder;

let mut qb = QueryBuilder::<sqlx::Postgres>::new(
    "SELECT id, name FROM users WHERE org_id = ",
);
qb.push_bind(org);
if let Some(min_age) = min_age {
    qb.push(" AND age >= ");
    qb.push_bind(min_age);
}
let rows = qb.build().fetch_all(&pool).await?;
```

This crate keeps the SQL where it belongs — in the template:

```rust
use sqlx_dyn::{query, sql_fragment, SqlFragment};

const KIND: SqlFragment = sql_fragment!("'document'");

let rows = query!(r#"
    SELECT id, name FROM users
    WHERE org_id = ${org}
      AND kind = #{KIND}
      AND age >= ${?min_age}
    ORDER BY id
"#)
.fetch_all(&pool)
.await?;
```

`min_age = None` drops `AND age >= $N` together with its `AND`, and the
`WHERE` goes too if nothing is left. The macro emits `QueryBuilder` calls —
no intermediate `String`, no `format!` — and binds travel out-of-band as
parameters.

## Markers

| Syntax     | Becomes            | Accepts                     | Untrusted input |
| ---------- | ------------------ | --------------------------- | --------------- |
| `${expr}`  | bind parameter     | anything `Encode + Type`    | ✅ safe         |
| `${?expr}` | optional predicate | `Option<T>`                 | ✅ safe         |
| `#{expr}`  | SQL text           | `SqlFragment` only (sealed) | ⚠️ SQL text     |

- Parameter numbering (`$1`, `$2`, …) is done by `QueryBuilder`; this crate
  never generates a `$N` itself, so splicing in a fragment never shifts your
  binds.
- `#{expr}` cannot carry user input: `SqlFragmentLike` is sealed and a
  fragment is only ever built from a `&'static str`, so `#{user_input}` is a
  compile error, not a convention. Two deliberate paths out — `str::leak` and
  `builder_mut()` — are visible at the call site; treat them like `unsafe`.

## Optional predicates

```rust
use sqlx_dyn::query;

let name: Option<&str> = None;
let min_age: Option<i32> = Some(18);

query!(r#"
    SELECT id FROM users
    WHERE name = ${?name}
      AND age >= ${?min_age}
    ORDER BY id
"#).sql()
// "SELECT id FROM users WHERE age >= $1 ORDER BY id"
// With min_age = None as well: "SELECT id FROM users ORDER BY id"
```

`${?expr}` must sit on the right-hand side of a comparison under `WHERE`,
`HAVING`, `AND`, or `OR`; the text after the marker belongs to the predicate
up to the next top-level `AND`/`OR`, clause keyword, or `)`. Anything else — a
second marker inside the predicate's tail, both sides of a `BETWEEN`, a SQL
comment in the template — is a compile error rather than silently wrong SQL.
The full rules, with examples, live in the [API reference](#documentation).

## Getting started

```toml
[dependencies]
sqlx_dyn = "0.1.4"
sqlx = { version = "0.9", features = ["postgres", "runtime-tokio"] }
```

Three macros cover the entry points: `query!` (rows as `PgRow`), `query_as!`
(rows decoded into `T: FromRow`), and `query_scalar!` (one column; takes no
type argument — the type is pinned at the `fetch_*` call site, as in
`sqlx::query_scalar`).

For static SQL, keep using `sqlx::query!`: it checks columns and types
against the schema, and this crate does not read the schema, so a renamed
column fails at runtime. Use this crate when the query text is not fully
known at compile time: conditional filters, caller-selected sort columns,
variable-length `IN` lists. No `DATABASE_URL` at build time; execution is
delegated to sqlx entirely.

## Escaping and evaluation

- A literal `${` is written `$${`, a literal `#{` is written `##{`; a `$` or
  `#` not followed by `{` is never special.
- Every interpolation is evaluated exactly once, in source order, at the point
  where it appears. Repeated expressions are not deduplicated.

## Documentation

- [API reference](https://docs.rs/sqlx_dyn): every marker, the injection
  model, and the `${?...}` rules, with compiled examples. Also available
  offline via `cargo doc --open`.
- [Guide](https://github.com/alfatm/sqlx-dyn/blob/master/docs/guide.md): why
  this crate exists, what the `QueryBuilder` code it replaces looks like, the
  fragment constraints, generated code, and the test strategy.
- [Examples](https://github.com/alfatm/sqlx-dyn/tree/master/sqlx_dyn/examples):
  `motivating`, `advanced`, `batch`.

## License

MIT

# sqlx_dyn

The missing link between `sqlx::query!` and plain SQL: between a query the
compiler checks against your schema, and one you build with `format!`. Built
at runtime, checked at compile time. Postgres only.

sqlx 0.9 implements `SqlSafeStr` for `&'static str` only, so `format!` is out.
SQL text comes only from `SqlFragment`, built from `&'static str` and nothing
else. A misplaced `${?}` is a compile error.

This can be done with the help of `QueryBuilder`, but I want SQL-like syntax:

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

`min_age = None` drops `AND age >= $2` together with its `AND`. The macro emits
`QueryBuilder` calls, with no intermediate `String` and no `format!`.

## Interpolation

| Syntax     | Becomes                      | Accepts           | User input  |
| ---------- | ---------------------------- | ----------------- | ----------- |
| `${expr}`  | `push_bind(expr)`            | `Encode + Type`   | ✅ safe     |
| `${?expr}` | predicate, dropped on `None` | `Option<T>`       | ✅ safe     |
| `#{expr}`  | `push(expr.as_sql())`        | `SqlFragmentLike` | ⚠️ SQL text |

PostgreSQL parameter numbering (`$1`, `$2`, …) is done by `QueryBuilder`. This
crate never generates `$N` itself, so adding a fragment never shifts the
numbering of your binds.

## Getting started

```toml
[dependencies]
sqlx_dyn = "0.1.2"
sqlx = { version = "0.9", features = ["postgres", "runtime-tokio"] }
```

Three macros cover most of it: `query!`, `query_as!`, and `query_scalar!`. Each
takes SQL as written. Start by replacing one query whose `WHERE` clause you
assemble by hand. That is where the difference shows first.

For static SQL, keep using `sqlx::query!`. It checks columns and types against
the schema at compile time; this crate does not read the schema, so a renamed
column fails at runtime. Use this crate when the query text is not fully known
at compile time: conditional filters, caller-selected sort columns,
variable-length `IN` lists.

## Documentation

- [API reference][docs]: every marker, the injection model, and the rules for
  `${?...}`, with compiled examples. Also available offline via
  `cargo doc --open`.
- [Guide][guide]: why this crate exists, what the `QueryBuilder` code it
  replaces looks like, the fragment constraints, generated code, and the test
  strategy.
- [Examples][examples]: `motivating`, `advanced`, `batch`.

[docs]: https://docs.rs/sqlx_dyn
[guide]: https://github.com/alfatm/sqlx-dyn/blob/master/docs/guide.md
[examples]: https://github.com/alfatm/sqlx-dyn/tree/master/sqlx_dyn/examples

## License

MIT

//! Integration tests for the `sqlx_dyn` macros.
//!
//! There is no database here, so every assertion is on the SQL text `.sql()`
//! yields. The `fetch_*`/`execute` surface is covered by functions that compile
//! but are never called.

use std::cell::Cell;

use sqlx_dyn::{query, query_as, query_scalar, sql_fragment, SqlFragment};

const ACTIVE: SqlFragment = sql_fragment!("deleted_at IS NULL");
const ORDER_BY_NAME: SqlFragment = sql_fragment!("ORDER BY name ASC");

#[derive(sqlx::FromRow)]
struct User {
    #[allow(dead_code)]
    id: i64,
    #[allow(dead_code)]
    name: String,
}

// 1. a plain query, no interpolation
#[test]
fn plain_query_is_verbatim() {
    let q = query!("SELECT id, name FROM users");
    assert_eq!(q.sql(), "SELECT id, name FROM users");
}

// 2. a single bind
#[test]
fn single_bind_becomes_dollar_one() {
    let id: i64 = 7;
    let q = query!("SELECT * FROM users WHERE id = ${id}");
    assert_eq!(q.sql(), "SELECT * FROM users WHERE id = $1");
}

// 3. multiple binds keep source order and numbering
#[test]
fn multiple_binds_are_numbered_in_order() {
    let name = "ada";
    let age: i32 = 36;
    let q = query!("SELECT * FROM users WHERE name = ${name} AND age > ${age}");
    assert_eq!(q.sql(), "SELECT * FROM users WHERE name = $1 AND age > $2");
}

// 4. the `${&reference}` form
#[test]
fn reference_bind_form() {
    let name = String::from("ada");
    let q = query!("SELECT * FROM users WHERE name = ${&name}");
    assert_eq!(q.sql(), "SELECT * FROM users WHERE name = $1");
}

// 5. complex expressions inside `${...}`
#[test]
// `opt` is deliberately a known `None` so the test needs no runtime input; the
// point is that the call expression survives interpolation.
#[allow(clippy::unnecessary_literal_unwrap)]
fn complex_expressions_bind() {
    struct Foo;
    impl Foo {
        fn bar(&self) -> i64 {
            1
        }
    }
    let foo = Foo;
    let v: Vec<i32> = vec![1, 2, 3];
    let opt: Option<i32> = None;

    let q = query!("SELECT ${foo.bar()}, ${v.len() as i32}, ${opt.unwrap_or_default()}");
    assert_eq!(q.sql(), "SELECT $1, $2, $3");
}

// 6. a single fragment inlines verbatim
#[test]
fn single_fragment_is_inlined() {
    let q = query!("SELECT * FROM users WHERE #{ACTIVE}");
    assert_eq!(q.sql(), "SELECT * FROM users WHERE deleted_at IS NULL");
}

// 7. multiple fragments
#[test]
fn multiple_fragments_are_inlined() {
    let q = query!("SELECT * FROM users WHERE #{ACTIVE} #{ORDER_BY_NAME}");
    assert_eq!(
        q.sql(),
        "SELECT * FROM users WHERE deleted_at IS NULL ORDER BY name ASC"
    );
}

// 8. fragments consume no `$N` slot
#[test]
fn fragments_do_not_consume_parameter_slots() {
    let min: i32 = 18;
    let max: i32 = 65;
    let q = query!(
        "SELECT * FROM users WHERE age > ${min} AND #{ACTIVE} AND age < ${max} #{ORDER_BY_NAME}"
    );
    assert_eq!(
        q.sql(),
        "SELECT * FROM users WHERE age > $1 AND deleted_at IS NULL AND age < $2 ORDER BY name ASC"
    );
}

// 9. query_as! with a FromRow struct
#[test]
fn query_as_builds_sql() {
    let id: i64 = 3;
    let q = query_as!(
        User,
        "SELECT id, name FROM users WHERE id = ${id} AND #{ACTIVE}"
    );
    assert_eq!(
        q.sql(),
        "SELECT id, name FROM users WHERE id = $1 AND deleted_at IS NULL"
    );
}

// 10. query_scalar!
#[test]
fn query_scalar_builds_sql() {
    let id: i64 = 3;
    // The scalar type lives on `fetch_*` rather than on the struct, so no
    // annotation is needed merely to inspect the SQL.
    let q = query_scalar!("SELECT count(*) FROM users WHERE owner = ${id} AND #{ACTIVE}");
    assert_eq!(
        q.sql(),
        "SELECT count(*) FROM users WHERE owner = $1 AND deleted_at IS NULL"
    );
}

// 11. the executable surface type-checks (compiles, never runs — no database).
#[allow(dead_code)]
async fn typecheck_query(pool: &sqlx::PgPool) -> sqlx::Result<()> {
    let id: i64 = 1;
    let _: Vec<sqlx::postgres::PgRow> = query!("SELECT id FROM users WHERE id = ${id}")
        .fetch_all(pool)
        .await?;
    let _: sqlx::postgres::PgRow = query!("SELECT id FROM users WHERE id = ${id}")
        .fetch_one(pool)
        .await?;
    let _: Option<sqlx::postgres::PgRow> = query!("SELECT id FROM users WHERE id = ${id}")
        .fetch_optional(pool)
        .await?;
    let _: sqlx::postgres::PgQueryResult = query!("DELETE FROM users WHERE id = ${id}")
        .execute(pool)
        .await?;
    Ok(())
}

#[allow(dead_code)]
async fn typecheck_query_as(pool: &sqlx::PgPool) -> sqlx::Result<()> {
    let id: i64 = 1;
    let _: Vec<User> = query_as!(User, "SELECT id, name FROM users WHERE id = ${id}")
        .fetch_all(pool)
        .await?;
    let _: User = query_as!(User, "SELECT id, name FROM users WHERE id = ${id}")
        .fetch_one(pool)
        .await?;
    let _: Option<User> = query_as!(User, "SELECT id, name FROM users WHERE id = ${id}")
        .fetch_optional(pool)
        .await?;
    let _: sqlx::postgres::PgQueryResult = query_as!(User, "DELETE FROM users WHERE id = ${id}")
        .execute(pool)
        .await?;
    Ok(())
}

// `DynQueryScalar` deliberately has no `execute`.
#[allow(dead_code)]
async fn typecheck_query_scalar(pool: &sqlx::PgPool) -> sqlx::Result<()> {
    let id: i64 = 1;
    let _: Vec<i64> = query_scalar!("SELECT id FROM users WHERE id = ${id}")
        .fetch_all(pool)
        .await?;
    let _: i64 = query_scalar!("SELECT id FROM users WHERE id = ${id}")
        .fetch_one(pool)
        .await?;
    let _: Option<i64> = query_scalar!("SELECT id FROM users WHERE id = ${id}")
        .fetch_optional(pool)
        .await?;
    Ok(())
}

// 12. escaping
#[test]
fn escaped_markers_become_literal() {
    let q = query!("SELECT '$${not_a_bind}', '##{not_a_fragment}'");
    assert_eq!(q.sql(), "SELECT '${not_a_bind}', '#{not_a_fragment}'");
}

// 13. bare `$` / `#` are never special
#[test]
fn bare_dollar_and_hash_pass_through() {
    let q = query!("SELECT $1, a # b, c::text, $");
    assert_eq!(q.sql(), "SELECT $1, a # b, c::text, $");
}

// 14. a PostgreSQL array bind
#[test]
fn array_any_bind() {
    let values: Vec<i64> = vec![1, 2, 3];
    let q = query!("SELECT * FROM users WHERE id = ANY(${&values})");
    assert_eq!(q.sql(), "SELECT * FROM users WHERE id = ANY($1)");
}

// 15. every interpolation is evaluated exactly once, in source order
#[test]
fn interpolations_evaluate_exactly_once() {
    fn bump(counter: &Cell<i32>) -> i32 {
        counter.set(counter.get() + 1);
        counter.get()
    }

    let counter = Cell::new(0);
    let q = query!("SELECT ${bump(&counter)}, ${bump(&counter)}");
    assert_eq!(q.sql(), "SELECT $1, $2");
    assert_eq!(counter.get(), 2);
}

// 16. a bind may reference a local that is gone before the builder is used: in
// sqlx 0.9 `push_bind` takes the value with a free method lifetime.
#[test]
fn bind_of_short_lived_reference() {
    let q = {
        let local = String::from("ada");
        query!("SELECT * FROM users WHERE name = ${local.as_str()}")
    };
    assert_eq!(q.sql(), "SELECT * FROM users WHERE name = $1");
}

// 17. a raw-string template keeps newlines and indentation
#[test]
fn multiline_raw_string_preserves_layout() {
    let id: i64 = 42;
    let q = query!(
        r#"
    SELECT id, name
    FROM users
    WHERE id = ${id}
      AND #{ACTIVE}
"#
    );
    assert_eq!(
        q.sql(),
        "\n    SELECT id, name\n    FROM users\n    WHERE id = $1\n      AND deleted_at IS NULL\n"
    );
}

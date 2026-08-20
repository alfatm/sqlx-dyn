//! Batch operations: multi-row INSERT, UPSERT, IN-lists, and UPDATE ... FROM.
//!
//! These are the cases where the macro alone is not enough, because the number of
//! rows is a runtime value. The pattern is always the same: start the statement
//! with `query!`, then drive the variable-length part through `builder_mut()`.
//! Bind numbering continues seamlessly across that boundary.
//!
//! Run: `cargo run -p sqlx_dyn --example batch`

use sqlx_dyn::{query, sql_fragment, SqlFragment};

struct NewUser<'a> {
    name: &'a str,
    email: &'a str,
    age: i32,
}

// ---------------------------------------------------------------------------
// 1. Multi-row INSERT
// ---------------------------------------------------------------------------

/// `push_values` writes `($1, $2, $3), ($4, $5, $6), ...` and binds every value.
/// Nothing is formatted into the SQL text.
///
/// # Panics
///
/// `sqlx::QueryBuilder::push_values` carries a `debug_assert!` that fires on an
/// empty iterator (it would otherwise emit `VALUES ` with no tuples, which
/// Postgres rejects). Guard empty input at the call site — see `insert_chunked`.
fn insert_many(users: &[NewUser<'_>]) -> String {
    assert!(!users.is_empty(), "insert_many requires at least one row");

    let mut q = query!("INSERT INTO users (name, email, age) ");

    q.builder_mut().push_values(users, |mut row, user| {
        row.push_bind(user.name)
            .push_bind(user.email)
            .push_bind(user.age);
    });

    q.builder_mut().push(" RETURNING id");
    q.sql()
}

/// Postgres caps a statement at 65535 parameters. With N columns per row, chunk
/// the input at `65535 / N` rows so a large batch cannot fail at runtime.
///
/// `chunks` never yields an empty slice, and an empty input yields no chunks at
/// all, so `insert_many`'s non-empty precondition holds by construction.
fn insert_chunked(users: &[NewUser<'_>], columns: usize) -> Vec<String> {
    const MAX_PARAMS: usize = 65535;
    let per_chunk = MAX_PARAMS / columns;
    users.chunks(per_chunk).map(insert_many).collect()
}

fn multi_row_insert() {
    let users = [
        NewUser { name: "ada", email: "ada@example.com", age: 36 },
        NewUser { name: "alan", email: "alan@example.com", age: 41 },
    ];

    assert_eq!(
        insert_many(&users),
        "INSERT INTO users (name, email, age) VALUES ($1, $2, $3), ($4, $5, $6) RETURNING id"
    );

    // Empty input: `push_values` would build `VALUES ` with no tuples, and sqlx
    // panics via debug_assert instead of emitting invalid SQL. The caller must
    // check up front. The hook is silenced so the expected panic is not
    // printed.
    let previous = std::panic::take_hook();
    std::panic::set_hook(Box::new(|_| {}));
    let empty = std::panic::catch_unwind(|| insert_many(&[]));
    std::panic::set_hook(previous);
    assert!(
        empty.is_err(),
        "empty batch must not silently produce invalid SQL"
    );

    // Chunking does it for you: no chunks means no statements.
    assert!(insert_chunked(&[], 3).is_empty());

    // 3 columns, 2 rows — one statement.
    assert_eq!(insert_chunked(&users, 3).len(), 1);
}

// ---------------------------------------------------------------------------
// 2. UPSERT with a shared conflict clause
// ---------------------------------------------------------------------------

/// The `ON CONFLICT` clause is fixed SQL, so it lives in a fragment and is reused
/// by every upsert against this table.
const UPSERT_USER: SqlFragment = sql_fragment!(
    "ON CONFLICT (email) DO UPDATE SET \
     name = EXCLUDED.name, age = EXCLUDED.age, updated_at = now()"
);

fn upsert_many(users: &[NewUser<'_>]) -> String {
    let mut q = query!("INSERT INTO users (name, email, age) ");

    q.builder_mut().push_values(users, |mut row, user| {
        row.push_bind(user.name)
            .push_bind(user.email)
            .push_bind(user.age);
    });

    // The fragment carries SQL syntax; append it explicitly.
    let b = q.builder_mut();
    b.push(" ");
    b.push(UPSERT_USER.as_str());
    q.sql()
}

fn upsert() {
    let users = [NewUser { name: "ada", email: "ada@example.com", age: 36 }];
    let sql = upsert_many(&users);
    assert!(sql.contains("VALUES ($1, $2, $3)"), "{sql}");
    assert!(sql.contains("ON CONFLICT (email) DO UPDATE SET"), "{sql}");
    assert!(sql.contains("name = EXCLUDED.name"), "{sql}");
}

// ---------------------------------------------------------------------------
// 3. IN-lists: array bind vs. expanded tuple
// ---------------------------------------------------------------------------

/// Preferred on Postgres: one bind for the whole list. The prepared statement is
/// identical regardless of how many ids there are, so the plan cache is reused.
fn in_list_via_array(ids: &[i64]) -> String {
    query!("SELECT * FROM users WHERE id = ANY(${ids})").sql()
}

/// The portable alternative, when `ANY` is not an option: `separated` expands to
/// one parameter per element. Note this produces a *different* statement for each
/// list length, which defeats statement caching.
fn in_list_expanded(ids: &[i64]) -> String {
    let mut q = query!("SELECT * FROM users WHERE id IN (");
    {
        let b = q.builder_mut();
        let mut list = b.separated(", ");
        for id in ids {
            list.push_bind(*id);
        }
    }
    q.builder_mut().push(")");
    q.sql()
}

fn in_lists() {
    // The same SQL for any number of ids.
    assert_eq!(
        in_list_via_array(&[1, 2, 3]),
        "SELECT * FROM users WHERE id = ANY($1)"
    );
    assert_eq!(
        in_list_via_array(&[1]),
        "SELECT * FROM users WHERE id = ANY($1)"
    );

    // Expanded form: the parameter count follows the input length.
    assert_eq!(
        in_list_expanded(&[1, 2, 3]),
        "SELECT * FROM users WHERE id IN ($1, $2, $3)"
    );
    // An empty list yields `IN ()`, which Postgres rejects — check before
    // building.
    assert_eq!(in_list_expanded(&[]), "SELECT * FROM users WHERE id IN ()");
}

// ---------------------------------------------------------------------------
// 4. Batch UPDATE via UPDATE ... FROM (VALUES ...)
// ---------------------------------------------------------------------------

/// Updating many rows to *different* values in one round trip: build a VALUES
/// list and join against it. Casts are needed because parameters in a VALUES
/// list have no inferable type.
fn bulk_update(updates: &[(i64, i32)]) -> String {
    let mut q = query!("UPDATE users SET age = v.age FROM (VALUES ");

    {
        let b = q.builder_mut();
        let mut rows = b.separated(", ");
        for (id, age) in updates {
            // Each tuple appends its own parenthesised group.
            rows.push("(");
            rows.push_bind_unseparated(*id);
            rows.push_unseparated("::bigint, ");
            rows.push_bind_unseparated(*age);
            rows.push_unseparated("::int)");
        }
    }

    q.builder_mut()
        .push(") AS v(id, age) WHERE users.id = v.id");
    q.sql()
}

fn bulk_update_example() {
    let sql = bulk_update(&[(1, 30), (2, 40)]);
    assert_eq!(
        sql,
        "UPDATE users SET age = v.age FROM (VALUES ($1::bigint, $2::int), \
         ($3::bigint, $4::int)) AS v(id, age) WHERE users.id = v.id"
    );
}

// ---------------------------------------------------------------------------
// 5. Execution surface
// ---------------------------------------------------------------------------

#[allow(dead_code)]
mod execution {
    use super::{NewUser, UPSERT_USER};
    use sqlx_dyn::query;

    /// Chunked insert inside one transaction: either all chunks land or none do.
    async fn insert_all(pool: &sqlx::PgPool, users: &[NewUser<'_>]) -> sqlx::Result<u64> {
        const COLUMNS: usize = 3;
        const MAX_PARAMS: usize = 65535;

        let mut tx = pool.begin().await?;
        let mut total = 0u64;

        for chunk in users.chunks(MAX_PARAMS / COLUMNS) {
            if chunk.is_empty() {
                continue;
            }
            let mut q = query!("INSERT INTO users (name, email, age) ");
            q.builder_mut().push_values(chunk, |mut row, user| {
                row.push_bind(user.name)
                    .push_bind(user.email)
                    .push_bind(user.age);
            });
            let b = q.builder_mut();
            b.push(" ");
            b.push(UPSERT_USER.as_str());

            total += q.execute(&mut *tx).await?.rows_affected();
        }

        tx.commit().await?;
        Ok(total)
    }

    async fn delete_by_ids(pool: &sqlx::PgPool, ids: &[i64]) -> sqlx::Result<u64> {
        let result = query!("DELETE FROM users WHERE id = ANY(${ids})")
            .execute(pool)
            .await?;
        Ok(result.rows_affected())
    }
}

fn main() {
    multi_row_insert();
    upsert();
    in_lists();
    bulk_update_example();

    println!("all batch examples asserted OK");
    println!();

    let users = [
        NewUser { name: "ada", email: "ada@example.com", age: 36 },
        NewUser { name: "alan", email: "alan@example.com", age: 41 },
    ];
    println!("--- multi-row INSERT ---\n{}\n", insert_many(&users));
    println!("--- UPSERT ---\n{}\n", upsert_many(&users));
    println!("--- bulk UPDATE ---\n{}", bulk_update(&[(1, 30), (2, 40)]));
}

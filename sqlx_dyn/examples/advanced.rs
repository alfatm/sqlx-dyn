//! Advanced usage patterns.
//!
//! Every function asserts its own SQL, so `cargo run --example advanced` is a
//! self-check: if the crate's codegen changes, these fail loudly instead of
//! quietly documenting something wrong. Nothing here connects to a database.
//!
//! Run: `cargo run -p sqlx_dyn --example advanced`

use sqlx_dyn::{query, query_as, query_scalar, sql_fragment, SqlFragment};

// ---------------------------------------------------------------------------
// 1. Conditional filters (dynamic WHERE)
// ---------------------------------------------------------------------------

/// Conditional filters come from `${?expr}`: the predicate is emitted only when the
/// `Option` is `Some`, and the joining `AND`/`WHERE` is placed to match. The
/// template stays readable as plain SQL.
#[derive(Default)]
struct UserFilter<'a> {
    name: Option<&'a str>,
    min_age: Option<i32>,
    org: Option<i64>,
}

fn find_users(filter: &UserFilter<'_>) -> String {
    // `${?expr}` takes an `Option`: `None` removes the whole predicate along
    // with its `AND`, and `WHERE` disappears if nothing survived. The template
    // reads as finished SQL — no `if let`, no `WHERE true` placeholder.
    query!(
        r#"
        SELECT id, name FROM users
        WHERE name ILIKE ${?filter.name.map(|n| format!("%{n}%"))}
          AND age >= ${?filter.min_age}
          AND organization_id = ${?filter.org}
        ORDER BY id
    "#
    )
    .sql()
}

fn conditional_filters() {
    // No filters: `WHERE` disappears entirely, and `ORDER BY` still follows.
    assert_eq!(
        norm(find_users(&UserFilter::default())),
        "SELECT id, name FROM users ORDER BY id"
    );

    // One filter: it introduces the clause with `WHERE`, not `AND`.
    assert_eq!(
        norm(find_users(&UserFilter {
            min_age: Some(18),
            ..Default::default()
        })),
        "SELECT id, name FROM users WHERE age >= $1 ORDER BY id"
    );

    // Middle filter only — `WHERE` again, and bind `$1`, because the preceding
    // predicate was dropped.
    assert_eq!(
        norm(find_users(&UserFilter {
            org: Some(7),
            ..Default::default()
        })),
        "SELECT id, name FROM users WHERE organization_id = $1 ORDER BY id"
    );

    // Two of three: the first survivor gets `WHERE`, the second gets `AND`.
    assert_eq!(
        norm(find_users(&UserFilter {
            min_age: Some(18),
            org: Some(7),
            ..Default::default()
        })),
        "SELECT id, name FROM users WHERE age >= $1 AND organization_id = $2 ORDER BY id"
    );

    // All three, numbered in template order.
    assert_eq!(
        norm(find_users(&UserFilter {
            name: Some("ada"),
            min_age: Some(18),
            org: Some(7),
        })),
        "SELECT id, name FROM users WHERE name ILIKE $1 \
         AND age >= $2 AND organization_id = $3 ORDER BY id"
    );
}

// ---------------------------------------------------------------------------
// 2. Safe dynamic ORDER BY + keyset pagination
// ---------------------------------------------------------------------------

/// Sort columns are SQL syntax, so they must come from a closed set of fragments.
/// A user-supplied string can never reach `#{...}`.
#[derive(Clone, Copy)]
enum SortBy {
    Newest,
    Name,
    Relevance,
}

impl SortBy {
    /// Parsing untrusted input into an enum is the safe boundary: the string is
    /// validated here and never becomes SQL.
    fn parse(raw: &str) -> Option<Self> {
        match raw {
            "newest" => Some(Self::Newest),
            "name" => Some(Self::Name),
            "relevance" => Some(Self::Relevance),
            _ => None,
        }
    }

    fn fragment(self) -> SqlFragment {
        const NEWEST: SqlFragment = sql_fragment!("created_at DESC, id DESC");
        const NAME: SqlFragment = sql_fragment!("name ASC, id ASC");
        const RELEVANCE: SqlFragment = sql_fragment!("rank DESC, id DESC");
        match self {
            Self::Newest => NEWEST,
            Self::Name => NAME,
            Self::Relevance => RELEVANCE,
        }
    }
}

/// Collapses whitespace so expectations do not have to mirror a template's layout.
///
/// Templates here are indented raw strings; `QueryBuilder` keeps that indentation in
/// the SQL, which Postgres does not care about.
fn norm(s: String) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Keyset pagination: the cursor is a *value* (binds), the sort order is *syntax*
/// (fragment from a closed set).
fn paginated_sql(sort_param: &str, cursor_id: Option<i64>, limit: i32) -> String {
    let order = SortBy::parse(sort_param)
        .unwrap_or(SortBy::Newest)
        .fragment();

    // The cursor is an optional predicate and the sort order is a fragment, so
    // both stay in the template. Only `LIMIT` needs an escape hatch: it is not a
    // predicate, so `${?...}` cannot express it.
    let mut q = query!(
        "SELECT id, name FROM posts
         WHERE published
           AND id > ${?cursor_id}
         ORDER BY #{order}"
    );
    q.builder_mut().push(" LIMIT ").push_bind(limit);
    q.sql()
}

fn dynamic_order_and_pagination() {
    // First page, known sort order.
    assert_eq!(
        norm(paginated_sql("name", None, 20)),
        "SELECT id, name FROM posts WHERE published ORDER BY name ASC, id ASC LIMIT $1"
    );

    // Later page: the cursor binds as $1, the limit becomes $2.
    assert_eq!(
        norm(paginated_sql("newest", Some(100), 20)),
        "SELECT id, name FROM posts WHERE published AND id > $1 \
         ORDER BY created_at DESC, id DESC LIMIT $2"
    );

    // Hostile sort parameter: rejected by the enum, fallback to the default.
    // The payload never appears in the SQL.
    let sql = norm(paginated_sql("id; DROP TABLE posts; --", None, 10));
    assert_eq!(
        sql,
        "SELECT id, name FROM posts WHERE published ORDER BY created_at DESC, id DESC LIMIT $1"
    );
    assert!(!sql.contains("DROP"));
}

// ---------------------------------------------------------------------------
// 3. CTEs, JOINs, and composed fragments
// ---------------------------------------------------------------------------

/// Fragments compose: a fragment can hold a whole join clause or predicate, and
/// several can be combined in one template.
fn cte_and_joins(org: i64, since_days: i32) -> String {
    const ACTIVE_ONLY: SqlFragment = sql_fragment!("u.deleted_at IS NULL");
    const JOIN_PROFILE: SqlFragment = sql_fragment!("LEFT JOIN profiles p ON p.user_id = u.id");
    const REPRESENTABLE: SqlFragment =
        sql_fragment!("kind::text IN ('document', 'template', 'i18n')");

    let q = query!(
        r#"
        WITH recent AS (
            SELECT user_id, COUNT(*)::int AS n
            FROM audit_log
            WHERE create_time >= now() - make_interval(days => ${since_days})
              AND #{REPRESENTABLE}
            GROUP BY user_id
        )
        SELECT u.id, u.name, p.avatar_url, COALESCE(r.n, 0) AS events
        FROM users u
        #{JOIN_PROFILE}
        LEFT JOIN recent r ON r.user_id = u.id
        WHERE u.organization_id = ${org}
          AND #{ACTIVE_ONLY}
        ORDER BY events DESC
    "#
    );
    q.sql()
}

fn ctes_and_joins() {
    let sql = cte_and_joins(7, 30);
    // Binds are numbered in template order, regardless of fragments in between.
    assert!(sql.contains("make_interval(days => $1)"), "{sql}");
    assert!(sql.contains("u.organization_id = $2"), "{sql}");
    // Fragments are inlined verbatim.
    assert!(sql.contains("kind::text IN ('document', 'template', 'i18n')"));
    assert!(sql.contains("LEFT JOIN profiles p ON p.user_id = u.id"));
    assert!(sql.contains("u.deleted_at IS NULL"));
    // The raw string's formatting is preserved, so EXPLAIN output stays readable.
    assert!(sql.contains("WITH recent AS ("));
}

// ---------------------------------------------------------------------------
// 4. Arrays, ANY, and optional parameters without branching
// ---------------------------------------------------------------------------

/// Postgres arrays let a variable-length IN-list stay a single bind, which avoids
/// both string building and per-item parameter slots.
fn array_membership(ids: &[i64], kinds: &[String]) -> String {
    let q = query!(
        r#"
        SELECT id FROM records
        WHERE author = ANY(${ids})
          AND kind = ANY(${kinds})
    "#
    );
    q.sql()
}

/// `IS NULL OR` is the idiomatic way to make a filter optional without changing
/// the SQL shape — one plan, one prepared statement, regardless of input.
/// The `IS NULL OR` idiom, shown for contrast — **not** the recommended way to write
/// an optional filter here.
///
/// `${?expr}` (section 1) does the same job with one bind instead of two and a
/// template that reads as the finished SQL. This form keeps the predicate in the SQL
/// unconditionally, which is only worth it when you specifically want one cached plan
/// for every combination of filters.
///
/// It also demonstrates the evaluation rule: `${name}` written twice is bound twice,
/// producing two parameters. Interpolations are never deduplicated.
fn optional_without_branching(name: Option<&str>, min_age: Option<i32>) -> String {
    let q = query!(
        r#"
        SELECT id FROM users
        WHERE (${name} IS NULL OR name = ${name})
          AND (${min_age} IS NULL OR age >= ${min_age})
    "#
    );
    q.sql()
}

/// The same two filters written the recommended way, for comparison.
fn optional_the_preferred_way(name: Option<&str>, min_age: Option<i32>) -> String {
    query!(
        r#"
        SELECT id FROM users
        WHERE name = ${?name}
          AND age >= ${?min_age}
    "#
    )
    .sql()
}

fn arrays_and_optionals() {
    // One bind per array, not per element.
    let sql = array_membership(&[1, 2, 3], &["a".into()]);
    assert!(sql.contains("author = ANY($1)"), "{sql}");
    assert!(sql.contains("kind = ANY($2)"), "{sql}");

    // Every interpolation is evaluated and bound separately, so `${name}` twice
    // yields TWO parameters — four in total for two filters.
    let sql = optional_without_branching(Some("ada"), None);
    assert!(sql.contains("($1 IS NULL OR name = $2)"), "{sql}");
    assert!(sql.contains("($3 IS NULL OR age >= $4)"), "{sql}");

    // `${?...}` expresses the same intent with one bind per surviving filter and
    // no parenthesised placeholders in the SQL.
    assert_eq!(
        norm(optional_the_preferred_way(Some("ada"), None)),
        "SELECT id FROM users WHERE name = $1"
    );
    assert_eq!(
        norm(optional_the_preferred_way(None, None)),
        "SELECT id FROM users"
    );
}

// ---------------------------------------------------------------------------
// 5. Typed rows, scalars, and reuse across call sites
// ---------------------------------------------------------------------------

#[derive(sqlx::FromRow)]
struct UserRow {
    #[allow(dead_code)]
    id: i64,
    #[allow(dead_code)]
    name: String,
}

/// A shared fragment used by several queries: define it once at module level and
/// reference it wherever the same predicate is needed.
const VISIBLE: SqlFragment = sql_fragment!("deleted_at IS NULL AND published");

fn typed_and_scalar(org: i64) {
    let rows = query_as!(
        UserRow,
        "SELECT id, name FROM users WHERE organization_id = ${org} AND #{VISIBLE}"
    );
    assert_eq!(
        rows.sql(),
        "SELECT id, name FROM users WHERE organization_id = $1 \
         AND deleted_at IS NULL AND published"
    );

    // query_scalar! takes no type argument; the column type is fixed at fetch_*.
    let count = query_scalar!("SELECT COUNT(*) FROM users WHERE #{VISIBLE}");
    assert_eq!(
        count.sql(),
        "SELECT COUNT(*) FROM users WHERE deleted_at IS NULL AND published"
    );
}

// ---------------------------------------------------------------------------
// 6. Execution surface: pools, transactions, connections
// ---------------------------------------------------------------------------

/// Anything implementing `sqlx::Executor` works, exactly as with plain sqlx.
/// Compiled but never called — there is no database here.
#[allow(dead_code)]
mod execution {
    use super::{UserRow, VISIBLE};
    use sqlx_dyn::{query, query_as, query_scalar};

    async fn from_pool(pool: &sqlx::PgPool) -> sqlx::Result<Vec<UserRow>> {
        query_as!(UserRow, "SELECT id, name FROM users WHERE #{VISIBLE}")
            .fetch_all(pool)
            .await
    }

    /// Inside a transaction, pass `&mut *tx`.
    async fn in_transaction(pool: &sqlx::PgPool, org: i64) -> sqlx::Result<i64> {
        let mut tx = pool.begin().await?;

        let affected = query!("UPDATE users SET seen = now() WHERE organization_id = ${org}")
            .execute(&mut *tx)
            .await?
            .rows_affected();

        let total: i64 = query_scalar!("SELECT COUNT(*) FROM users WHERE #{VISIBLE}")
            .fetch_one(&mut *tx)
            .await?;

        tx.commit().await?;
        let _ = affected;
        Ok(total)
    }

    /// A single connection works too.
    async fn on_connection(
        conn: &mut sqlx::PgConnection,
        id: i64,
    ) -> sqlx::Result<Option<UserRow>> {
        query_as!(UserRow, "SELECT id, name FROM users WHERE id = ${id}")
            .fetch_optional(conn)
            .await
    }
}

fn main() {
    conditional_filters();
    dynamic_order_and_pagination();
    ctes_and_joins();
    arrays_and_optionals();
    typed_and_scalar(7);

    println!("all advanced examples asserted OK");
    println!();
    println!("--- conditional filters (all three) ---");
    println!(
        "{}",
        find_users(&UserFilter {
            name: Some("ada"),
            min_age: Some(18),
            org: Some(7),
        })
    );
    println!();
    println!("--- CTE + joins ---");
    println!("{}", cte_and_joins(7, 30));
}

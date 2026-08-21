//! End-to-end tests against a real PostgreSQL server.
//!
//! Every other test in the crate asserts the SQL *text* the macro assembles.
//! That proves the text is what was intended, but not that it is **valid
//! PostgreSQL** — and the defects these tests were written for (a cast left
//! glued to a table name, a `WHERE` grown in after `HAVING`, a dangling `AND`)
//! were all visually plausible text that the server would reject. These tests
//! close the gap: the server is the oracle.
//!
//! Requires Docker. The container is started once and shared by every test in
//! this file, because starting Postgres costs incomparably more than any
//! individual query.
//!
//! A shared pool also requires a shared **runtime**: sqlx keeps background
//! tasks on the runtime that created the pool, so a pool built inside a single
//! `#[tokio::test]` stops working as soon as that test's runtime is dropped.
//! Each test therefore runs its body on one long-lived runtime owned by this
//! module, and is declared `#[test]` rather than `#[tokio::test]`.
//!
//! Run with: `cargo test --features e2e --test e2e`
//!
//! The container is held in a `static` that is never dropped, so it is not
//! removed when the test binary exits. `testcontainers` normally delegates
//! cleanup to its Ryuk reaper; if that is disabled in your environment,
//! stray containers will pile up:
//!
//! ```sh
//! docker ps -aq --filter ancestor=postgres:16-alpine | xargs -r docker rm -f
//! ```

use std::future::Future;
use std::sync::OnceLock;

use sqlx::{PgPool, Row};
use sqlx_dyn::{query, query_as, query_scalar, sql_fragment, SqlFragment};
use testcontainers_modules::postgres::Postgres as PostgresImage;
use testcontainers_modules::testcontainers::runners::AsyncRunner;
use testcontainers_modules::testcontainers::{ContainerAsync, ImageExt};

/// Pinned to a modern server; the module's default tag is Postgres 11.
const PG_TAG: &str = "16-alpine";

struct Server {
    runtime: tokio::runtime::Runtime,
    pool: PgPool,
    /// Held only to keep the container alive for the lifetime of the process.
    _container: ContainerAsync<PostgresImage>,
}

static SERVER: OnceLock<Server> = OnceLock::new();

/// Starts Postgres once and returns the shared server, creating the fixture
/// schema on first use.
///
/// Panics rather than skipping when Docker is unavailable: a silently skipped
/// integration suite is indistinguishable from a passing one.
fn server() -> &'static Server {
    SERVER.get_or_init(|| {
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .enable_all()
            .build()
            .expect("build test runtime");

        let (pool, container) = runtime.block_on(async {
            let container = PostgresImage::default()
                .with_tag(PG_TAG)
                .start()
                .await
                .expect("start postgres container (is Docker running?)");
            let url = format!(
                "postgres://postgres:postgres@{}:{}/postgres",
                container.get_host().await.expect("container host"),
                container
                    .get_host_port_ipv4(5432)
                    .await
                    .expect("container port"),
            );
            let pool = PgPool::connect(&url).await.expect("connect to postgres");
            schema(&pool).await;
            (pool, container)
        });

        Server {
            runtime,
            pool,
            _container: container,
        }
    })
}

/// Runs `body` against the shared pool on the shared runtime.
///
/// Tests call this instead of `#[tokio::test]` so every query runs on the same
/// runtime the pool was created on.
fn with_pool<F, Fut>(body: F)
where
    F: FnOnce(&'static PgPool) -> Fut,
    Fut: Future<Output = ()>,
{
    let server = server();
    server.runtime.block_on(body(&server.pool));
}

async fn schema(pool: &PgPool) {
    sqlx::raw_sql(
        r#"
        CREATE TABLE users (
            id              bigserial PRIMARY KEY,
            name            text NOT NULL,
            age             int,
            organization_id bigint,
            kind            text NOT NULL DEFAULT 'document',
            external_id     uuid,
            deleted_at      timestamptz
        );
        CREATE TABLE events (
            id      bigserial PRIMARY KEY,
            user_id bigint NOT NULL REFERENCES users(id),
            action  text NOT NULL
        );
        INSERT INTO users (name, age, organization_id, kind, external_id) VALUES
            ('ada',    36, 1, 'document', '00000000-0000-0000-0000-000000000001'),
            ('grace',  45, 1, 'template', '00000000-0000-0000-0000-000000000002'),
            ('alan',   41, 2, 'document', NULL),
            ('edsger', 72, 2, 'i18n',     NULL);
        INSERT INTO events (user_id, action) VALUES
            (1, 'create'), (1, 'update'), (2, 'create'), (3, 'delete');
        "#,
    )
    .execute(pool)
    .await
    .expect("create fixture schema");
}

/// Asserts that the SQL is valid PostgreSQL without executing it.
///
/// `PREPARE` makes the server parse and plan the statement — exactly the check
/// wanted for templates whose result set is beside the point. Prepared
/// statements are connection-bound, so this runs inside a transaction that is
/// then rolled back: both statements are pinned to one connection, and the
/// name is deallocated afterwards.
async fn assert_valid(pool: &PgPool, sql: &str) {
    let mut tx = pool.begin().await.expect("begin");
    // `AssertSqlSafe` is mandatory because the statement text is built at
    // runtime. It is safe here for exactly the reason the guard asks about: the
    // text comes from this crate's own macro, and every user value in it is a
    // bind parameter.
    let prepare = sqlx::AssertSqlSafe(format!("PREPARE __check AS {sql}"));
    let outcome = sqlx::raw_sql(prepare).execute(&mut *tx).await;
    // `DEALLOCATE` is not transactional, so the name is dropped explicitly on
    // success.
    if outcome.is_ok() {
        sqlx::raw_sql("DEALLOCATE __check")
            .execute(&mut *tx)
            .await
            .expect("deallocate");
    }
    tx.rollback().await.expect("rollback");
    if let Err(err) = outcome {
        panic!("server rejected generated SQL:\n  {sql}\n  {err}");
    }
}

// ---------------------------------------------------------------------------
// Optional predicates: every survival combination must be valid and correct
// ---------------------------------------------------------------------------

#[test]
fn optional_predicates_run_for_every_combination() {
    with_pool(|pool| async move {
        for (name, min_age, org) in [
            (None, None, None),
            (Some("ada"), None, None),
            (None, Some(40), None),
            (None, None, Some(1i64)),
            (Some("ada"), Some(30), None),
            (Some("ada"), None, Some(1)),
            (None, Some(40), Some(2)),
            (Some("alan"), Some(40), Some(2)),
        ] {
            let rows = query!(
                r#"
                SELECT id, name FROM users
                WHERE name = ${?name}
                  AND age >= ${?min_age}
                  AND organization_id = ${?org}
                ORDER BY id
                "#
            )
            .fetch_all(pool)
            .await
            .unwrap_or_else(|err| panic!("{name:?}/{min_age:?}/{org:?}: {err}"));

            // Cross-check the row count against the same filter applied in
            // Rust.
            let expected = [
                ("ada", 36, 1i64),
                ("grace", 45, 1),
                ("alan", 41, 2),
                ("edsger", 72, 2),
            ]
            .iter()
            .filter(|(n, a, o)| {
                name.is_none_or(|want| *n == want)
                    && min_age.is_none_or(|want| *a >= want)
                    && org.is_none_or(|want| *o == want)
            })
            .count();
            assert_eq!(
                rows.len(),
                expected,
                "{name:?}/{min_age:?}/{org:?} returned {} rows, expected {expected}",
                rows.len()
            );
        }
    });
}

#[test]
fn a_dropped_predicate_leaves_no_dangling_keyword() {
    with_pool(|pool| async move {
        let gone: Option<i32> = None;

        // The optional owned the `WHERE`; the literal predicate after it must
        // inherit it.
        let rows = query!("SELECT id FROM users WHERE age >= ${?gone} AND deleted_at IS NULL")
            .fetch_all(pool)
            .await
            .expect("dropped optional followed by a literal predicate");
        assert_eq!(rows.len(), 4);
    });
}

#[test]
fn a_cast_after_the_marker_vanishes_with_its_predicate() {
    with_pool(|pool| async move {
        // The regression this pins: `None` used to emit `FROM users::uuid`.
        let missing: Option<&str> = None;
        let rows = query!("SELECT id FROM users WHERE external_id = ${?missing}::uuid")
            .fetch_all(pool)
            .await
            .expect("dropped predicate must not leave the cast behind");
        assert_eq!(rows.len(), 4);

        let present: Option<&str> = Some("00000000-0000-0000-0000-000000000001");
        let rows = query!("SELECT id FROM users WHERE external_id = ${?present}::uuid")
            .fetch_all(pool)
            .await
            .expect("surviving predicate keeps its cast");
        assert_eq!(rows.len(), 1);
    });
}

#[test]
fn a_concatenation_tail_vanishes_with_its_predicate() {
    with_pool(|pool| async move {
        let prefix: Option<&str> = Some("ad");
        let rows = query!("SELECT id FROM users WHERE name LIKE ${?prefix} || '%'")
            .fetch_all(pool)
            .await
            .expect("LIKE with a concatenated wildcard");
        assert_eq!(rows.len(), 1);

        let none: Option<&str> = None;
        let rows = query!("SELECT id FROM users WHERE name LIKE ${?none} || '%'")
            .fetch_all(pool)
            .await
            .expect("dropped predicate must not leave `|| '%'` behind");
        assert_eq!(rows.len(), 4);
    });
}

// ---------------------------------------------------------------------------
// Several predicate lists in one template
// ---------------------------------------------------------------------------

#[test]
fn where_and_having_are_independent_on_the_server() {
    with_pool(|pool| async move {
        for (org, min_events) in [
            (None, None),
            (Some(1i64), None),
            (None, Some(2i64)),
            (Some(1), Some(1)),
        ] {
            let rows = query!(
                r#"
                SELECT u.id, count(e.id) AS n
                FROM users u
                LEFT JOIN events e ON e.user_id = u.id
                WHERE u.organization_id = ${?org}
                GROUP BY u.id
                HAVING count(e.id) >= ${?min_events}
                ORDER BY u.id
                "#
            )
            .fetch_all(pool)
            .await
            .unwrap_or_else(|err| panic!("org={org:?} min_events={min_events:?}: {err}"));

            // Every returned group must satisfy whichever filters were
            // active.
            for row in &rows {
                let n: i64 = row.get("n");
                if let Some(min) = min_events {
                    assert!(n >= min, "group with {n} events passed HAVING >= {min}");
                }
            }
        }
    });
}

#[test]
fn union_branches_each_get_their_own_where() {
    with_pool(|pool| async move {
        for (a, b) in [
            (None, None),
            (Some(1i64), None),
            (None, Some(2i64)),
            (Some(1), Some(2)),
        ] {
            let sql = query!(
                r#"
                SELECT id FROM users WHERE organization_id = ${?a}
                UNION
                SELECT user_id FROM events WHERE user_id = ${?b}
                "#
            );
            let text = sql.sql();
            sql.fetch_all(pool)
                .await
                .unwrap_or_else(|err| panic!("a={a:?} b={b:?}: {err}\n  {text}"));
        }
    });
}

#[test]
fn a_subquery_where_does_not_open_a_new_clause() {
    with_pool(|pool| async move {
        let org: Option<i64> = None;

        // The `WHERE` inside `EXISTS` is nested: dropping the outer optional
        // must remove the outer `WHERE` and leave the inner one alone.
        let rows = query!(
            r#"
            SELECT id FROM users u
            WHERE EXISTS (SELECT 1 FROM events e WHERE e.user_id = u.id)
              AND u.organization_id = ${?org}
            ORDER BY id
            "#
        )
        .fetch_all(pool)
        .await
        .expect("nested WHERE must survive a dropped outer predicate");
        assert_eq!(rows.len(), 3, "users 1, 2 and 3 have events");
    });
}

// ---------------------------------------------------------------------------
// Binds carry values, not SQL
// ---------------------------------------------------------------------------

#[test]
fn injection_payloads_are_data_on_the_server() {
    with_pool(|pool| async move {
        for payload in [
            "'; DROP TABLE users; --",
            "1' OR '1'='1",
            "${name}",
            "#{FILTER}",
            "$1",
        ] {
            let found: i64 = query_scalar!("SELECT count(*) FROM users WHERE name = ${payload}")
                .fetch_one(pool)
                .await
                .unwrap_or_else(|err| panic!("payload {payload:?}: {err}"));
            assert_eq!(found, 0, "payload {payload:?} matched a row");
        }

        // The table is still there; had the first payload been parsed as SQL,
        // it would not be.
        let total: i64 = query_scalar!("SELECT count(*) FROM users")
            .fetch_one(pool)
            .await
            .expect("users table intact");
        assert_eq!(total, 4);
    });
}

#[test]
fn bind_order_matches_the_template() {
    with_pool(|pool| async move {
        // Had the binds been swapped, this would return a row instead of
        // nothing, even though the SQL text would be byte-for-byte the same.
        let rows =
            query!("SELECT id FROM users WHERE name = ${\"ada\"} AND kind = ${\"template\"}")
                .fetch_all(pool)
                .await
                .expect("two text binds in order");
        assert!(rows.is_empty(), "ada is a document, not a template");

        let rows =
            query!("SELECT id FROM users WHERE name = ${\"grace\"} AND kind = ${\"template\"}")
                .fetch_all(pool)
                .await
                .expect("two text binds in order");
        assert_eq!(rows.len(), 1);
    });
}

// ---------------------------------------------------------------------------
// Fragments, arrays and typed macros
// ---------------------------------------------------------------------------

#[test]
fn fragments_do_not_disturb_bind_numbering() {
    with_pool(|pool| async move {
        const ACTIVE: SqlFragment = sql_fragment!("deleted_at IS NULL");
        const REPRESENTABLE: SqlFragment = sql_fragment!("kind IN ('document', 'template')");

        let org: i64 = 1;
        let min_age: i32 = 30;
        let rows = query!(
            r#"
            SELECT id FROM users
            WHERE organization_id = ${org}
              AND #{ACTIVE}
              AND age >= ${min_age}
              AND #{REPRESENTABLE}
            ORDER BY id
            "#
        )
        .fetch_all(pool)
        .await
        .expect("binds numbered across interleaved fragments");
        assert_eq!(rows.len(), 2, "ada and grace");
    });
}

#[test]
fn array_binds_are_one_parameter() {
    with_pool(|pool| async move {
        let ids: Vec<i64> = vec![1, 2, 3];

        let found: i64 = query_scalar!("SELECT count(*) FROM users WHERE id = ANY(${&ids})")
            .fetch_one(pool)
            .await
            .expect("array bind");
        assert_eq!(found, 3);
    });
}

#[test]
fn query_as_decodes_rows() {
    #[derive(sqlx::FromRow, Debug)]
    struct UserRow {
        id: i64,
        name: String,
    }

    with_pool(|pool| async move {
        let min_age: Option<i32> = Some(40);
        let rows = query_as!(
            UserRow,
            "SELECT id, name FROM users WHERE age >= ${?min_age} ORDER BY id"
        )
        .fetch_all(pool)
        .await
        .expect("decode into UserRow");

        assert_eq!(rows.len(), 3);
        assert_eq!(rows[0].name, "grace");
        assert!(rows[0].id > 0);
    });
}

#[test]
fn query_scalar_infers_from_the_fetch_site() {
    with_pool(|pool| async move {
        let org: Option<i64> = Some(1);

        let count: i64 =
            query_scalar!("SELECT count(*) FROM users WHERE organization_id = ${?org}")
                .fetch_one(pool)
                .await
                .expect("i64 count");
        assert_eq!(count, 2);

        let name: String = query_scalar!("SELECT name FROM users WHERE id = ${1i64}")
            .fetch_one(pool)
            .await
            .expect("String scalar");
        assert_eq!(name, "ada");

        let missing: Option<String> = query_scalar!("SELECT name FROM users WHERE id = ${999i64}")
            .fetch_optional(pool)
            .await
            .expect("fetch_optional");
        assert_eq!(missing, None);
    });
}

#[test]
fn query_as_execute_runs_the_statement_and_ignores_the_row_type() {
    #[derive(sqlx::FromRow)]
    struct Unused {
        #[allow(dead_code)]
        id: i64,
    }

    with_pool(|pool| async move {
        // `sqlx::QueryAs` has no `execute`, so `DynQueryAs::execute`
        // deliberately builds an untyped query. This asserts that the statement
        // really runs and reports a row count, and that the decode type is
        // simply unused: `UPDATE` returns no `id` column, so `build_query_as`
        // would fail here.
        let mut tx = pool.begin().await.expect("begin");
        let org: Option<i64> = Some(1);
        let affected = query_as!(
            Unused,
            "UPDATE users SET age = age + 1 WHERE organization_id = ${?org}"
        )
        .execute(&mut *tx)
        .await
        .expect("query_as!(...).execute() must run the statement")
        .rows_affected();
        assert_eq!(affected, 2);
        tx.rollback().await.expect("rollback");
    });
}

#[test]
fn execute_reports_affected_rows() {
    with_pool(|pool| async move {
        // Isolated in a transaction so the fixture data stays untouched for
        // other tests.
        let mut tx = pool.begin().await.expect("begin");
        let org: Option<i64> = Some(2);
        let affected = query!("UPDATE users SET age = age + 1 WHERE organization_id = ${?org}")
            .execute(&mut *tx)
            .await
            .expect("update")
            .rows_affected();
        assert_eq!(affected, 2);
        tx.rollback().await.expect("rollback");
    });
}

// ---------------------------------------------------------------------------
// The server parses every template the unit tests assert
// ---------------------------------------------------------------------------

#[test]
fn representative_templates_are_valid_postgresql() {
    with_pool(|pool| async move {
        const ACTIVE: SqlFragment = sql_fragment!("deleted_at IS NULL");

        let none: Option<i32> = None;
        let some: Option<i32> = Some(1);
        let n64: Option<i64> = Some(1);

        // Each entry is a template rendered in the shape a caller would get;
        // `PREPARE` makes the server parse and plan it.
        let sqls = vec![
            query!("SELECT id FROM users WHERE age = ${?none}").sql(),
            query!("SELECT id FROM users WHERE age = ${?some}").sql(),
            query!("SELECT id FROM users WHERE age = ${?none} AND deleted_at IS NULL").sql(),
            query!("SELECT id FROM users WHERE age = ${?none} AND #{ACTIVE}").sql(),
            query!("SELECT id FROM users WHERE age = ${?none} ORDER BY id").sql(),
            query!("SELECT kind, count(*) FROM users WHERE age = ${?none} GROUP BY kind").sql(),
            query!(
                "SELECT kind, count(*) FROM users WHERE age = ${?none} \
                 GROUP BY kind HAVING count(*) > ${?n64}"
            )
            .sql(),
            query!("SELECT id FROM users WHERE external_id = ${?none}::uuid").sql(),
            query!("SELECT id FROM users WHERE name LIKE ${?none} || '%'").sql(),
            query!("SELECT id FROM users WHERE lower(name) = ${?none}").sql(),
            query!("SELECT id FROM users WHERE age <> ${?none} OR age > ${?some}").sql(),
            query!(
                "SELECT id FROM users WHERE age = ${?none} \
                 UNION SELECT user_id FROM events WHERE user_id = ${?n64}"
            )
            .sql(),
            query!(
                "SELECT id FROM users u WHERE EXISTS \
                 (SELECT 1 FROM events e WHERE e.user_id = u.id AND e.id = ${?n64})"
            )
            .sql(),
        ];

        for sql in sqls {
            assert_valid(pool, &sql).await;
        }
    });
}

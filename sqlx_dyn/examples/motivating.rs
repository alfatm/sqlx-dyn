//! The motivating example: no `format!`, no hand-written QueryBuilder.
//! Compiles but does not run — there is no database in CI.

use sqlx_dyn::{query, query_as, query_scalar, sql_fragment, SqlFragment};

const REPRESENTABLE_KINDS: SqlFragment = sql_fragment!(
    "kind::text IN \
    ('document', 'template', 'i18n', 'document-menu', 'template-param', 'content-rule')"
);

#[derive(sqlx::FromRow)]
struct StatRow {
    #[allow(dead_code)]
    action_type: String,
    #[allow(dead_code)]
    kind: String,
    #[allow(dead_code)]
    count: i32,
}

async fn audit_stats(
    pool: &sqlx::PgPool,
    all_ids: Vec<i64>,
    period_days: i32,
) -> sqlx::Result<Vec<StatRow>> {
    query_as!(
        StatRow,
        r#"
    SELECT
        action_type::text AS action_type,
        kind::text AS kind,
        COUNT(*)::int AS count
    FROM system.record_audit_log
    WHERE author = ANY(${&all_ids})
      AND create_time >= now() - make_interval(days => ${period_days})
      AND #{REPRESENTABLE_KINDS}
    GROUP BY action_type, kind
"#
    )
    .fetch_all(pool)
    .await
}

async fn user_count(pool: &sqlx::PgPool, organization_id: i64) -> sqlx::Result<i64> {
    query_scalar!(
        r#"
        SELECT COUNT(*)
        FROM users
        WHERE organization_id = ${organization_id}
    "#
    )
    .fetch_one(pool)
    .await
}

fn main() {
    // Prove the SQL assembles without a connection.
    let all_ids: Vec<i64> = vec![1, 2, 3];
    let q = query!(r#"SELECT * FROM t WHERE author = ANY(${&all_ids}) AND #{REPRESENTABLE_KINDS}"#);
    println!("{}", q.sql());

    let _ = audit_stats;
    let _ = user_count;
}

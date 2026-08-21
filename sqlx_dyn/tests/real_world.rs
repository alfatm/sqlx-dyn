//! Rewrites of real `sqlx::query(&format!(...))` call sites taken from a
//! production backend, with assertions that the generated SQL matches what the
//! hand-written version produced.
//!
//! Each test quotes the original code in a comment so the two can be compared
//! without leaving the file. The original SQL is reproduced verbatim as the
//! expected value — that is the whole point: a rewrite must not change the
//! query.
//!
//! Source: `crates/server/src/services/{audit,templates,content_rules,i18n}.rs`
//! of an internal CMS backend (sqlx 0.8; this crate targets 0.9, so those call
//! sites would need an upgrade before adoption — see the note at the end).

use sqlx_dyn::{query, query_scalar, sql_fragment, SqlFragment};

/// Collapses whitespace so a formatted raw-string template can be compared
/// against equally formatted original SQL.
fn norm(s: impl AsRef<str>) -> String {
    s.as_ref().split_whitespace().collect::<Vec<_>>().join(" ")
}

// ---------------------------------------------------------------------------
// 1. Audit statistics — the canonical case
// ---------------------------------------------------------------------------
//
// Original (audit.rs:133 and 599):
//
//     const REPRESENTABLE_KINDS_BARE: &str = "kind::text IN \
//         ('document', 'template', 'i18n', 'document-menu', 'template-param', 'content-rule')";
//
//     let stat_rows = sqlx::query(&format!(
//         "SELECT action_type::text AS action_type, kind::text AS kind, COUNT(*)::int AS count
//          FROM system.record_audit_log
//          WHERE author = ANY($1)
//            AND create_time >= now() - make_interval(days => $2)
//            AND {REPRESENTABLE_KINDS_BARE}
//          GROUP BY action_type, kind"
//     ))
//     .bind(&all_ids)
//     .bind(period_days)
//     .fetch_all(&self.pool)
//
// Two `.bind()` calls whose order must be kept in sync by hand with `$1`/`$2`,
// plus a `format!` that interpolates a constant into the SQL text.

/// A bare `&str` becomes a typed fragment, so it is only ever used as SQL
/// syntax and cannot be mistaken for a value.
const REPRESENTABLE_KINDS: SqlFragment = sql_fragment!(
    "kind::text IN \
     ('document', 'template', 'i18n', 'document-menu', 'template-param', 'content-rule')"
);

fn audit_stats_sql(all_ids: &[i64], period_days: i32) -> String {
    query!(
        r#"
        SELECT action_type::text AS action_type, kind::text AS kind, COUNT(*)::int AS count
        FROM system.record_audit_log
        WHERE author = ANY(${all_ids})
          AND create_time >= now() - make_interval(days => ${period_days})
          AND #{REPRESENTABLE_KINDS}
        GROUP BY action_type, kind
    "#
    )
    .sql()
}

#[test]
fn audit_stats_matches_original() {
    // Verbatim from audit.rs:600-605, with the constant expanded.
    let original = "SELECT action_type::text AS action_type, kind::text AS kind, COUNT(*)::int AS count
             FROM system.record_audit_log
             WHERE author = ANY($1)
               AND create_time >= now() - make_interval(days => $2)
               AND kind::text IN ('document', 'template', 'i18n', 'document-menu', 'template-param', 'content-rule')
             GROUP BY action_type, kind";

    assert_eq!(norm(audit_stats_sql(&[1, 2], 30)), norm(original));
}

// ---------------------------------------------------------------------------
// 2. The same query with LIMIT — three binds, one shared fragment
// ---------------------------------------------------------------------------
//
// Original (audit.rs:614): an identical WHERE clause plus `LIMIT $3`. The
// fragment is reused, and the third bind had to be numbered by hand.

fn audit_top_records_sql(all_ids: &[i64], period_days: i32, limit: i64) -> String {
    query!(
        r#"
        SELECT workspace, record_id, kind::text AS kind, COUNT(*)::int AS count,
               MAX(create_time) AS last_time,
               (array_agg(display_key ORDER BY create_time DESC))[1] AS display_key
        FROM system.record_audit_log
        WHERE author = ANY(${all_ids})
          AND create_time >= now() - make_interval(days => ${period_days})
          AND #{REPRESENTABLE_KINDS}
        GROUP BY workspace, record_id, kind
        ORDER BY count DESC, last_time DESC
        LIMIT ${limit}
    "#
    )
    .sql()
}

#[test]
fn audit_top_records_matches_original() {
    let original = "SELECT workspace, record_id, kind::text AS kind, COUNT(*)::int AS count,
                    MAX(create_time) AS last_time,
                    (array_agg(display_key ORDER BY create_time DESC))[1] AS display_key
             FROM system.record_audit_log
             WHERE author = ANY($1)
               AND create_time >= now() - make_interval(days => $2)
               AND kind::text IN ('document', 'template', 'i18n', 'document-menu', 'template-param', 'content-rule')
             GROUP BY workspace, record_id, kind
             ORDER BY count DESC, last_time DESC
             LIMIT $3";

    assert_eq!(norm(audit_top_records_sql(&[1], 30, 20)), norm(original));
}

#[test]
fn fragment_reuse_does_not_disturb_bind_numbering() {
    // The fragment sits between binds 2 and 3 in both queries; `$N` is assigned
    // by QueryBuilder as binds are pushed, so inserting SQL text cannot shift
    // it.
    let two = audit_stats_sql(&[1], 30);
    assert!(two.contains("ANY($1)") && two.contains("days => $2"));
    assert!(!two.contains("$3"), "only two binds: {two}");

    let three = audit_top_records_sql(&[1], 30, 20);
    assert!(three.contains("ANY($1)") && three.contains("days => $2"));
    assert!(three.contains("LIMIT $3"), "{three}");
}

// ---------------------------------------------------------------------------
// 3. The recurring "list + count" pair
// ---------------------------------------------------------------------------
//
// Original (templates.rs:231/276 and content_rules.rs:236/273 — the same shape
// in both files):
//
//     let where_clause = format!("{LIST_WHERE}{}", filter.where_clause);
//     let count_sql = format!("SELECT COUNT(*) FROM record t WHERE {where_clause}");
//
// `LIST_WHERE` is a constant predicate; `filter.where_clause` is a pre-built
// fragment that already starts with " AND ...". Both are SQL text, so both
// become fragments — and the count query stops being a second `format!`.

const LIST_WHERE: SqlFragment = sql_fragment!("t.kind = 'template' AND t.deleted_at IS NULL");

fn list_count_sql(extra: Option<SqlFragment>) -> String {
    // An optional *fragment* is not `${?}` (that is for binds); pick between
    // fragments instead, which keeps the closed-set guarantee.
    let extra = extra.unwrap_or(sql_fragment!("true"));
    query_scalar!(
        r#"
        SELECT COUNT(*) FROM record t
        WHERE #{LIST_WHERE}
          AND #{extra}
    "#
    )
    .sql()
}

#[test]
fn list_count_matches_original_shape() {
    assert_eq!(
        norm(list_count_sql(None)),
        norm("SELECT COUNT(*) FROM record t WHERE t.kind = 'template' AND t.deleted_at IS NULL AND true")
    );

    const PUBLISHED: SqlFragment = sql_fragment!("t.published");
    assert_eq!(
        norm(list_count_sql(Some(PUBLISHED))),
        norm("SELECT COUNT(*) FROM record t WHERE t.kind = 'template' AND t.deleted_at IS NULL AND t.published")
    );
}

// ---------------------------------------------------------------------------
// 4. Conditional filters that used to be `where_parts.push(format!(...))`
// ---------------------------------------------------------------------------
//
// Original shape (i18n.rs:1083, content_issues.rs:402):
//
//     let mut where_parts: Vec<String> = Vec::new();
//     if let Some(kind) = kind { where_parts.push(format!("r.kind = ${}", next_param)); ... }
//     let where_clause = match conditions.is_empty() {
//         true => String::new(),
//         false => format!("WHERE {}", conditions.join(" AND ")),
//     };
//
// The `Vec<String>`, the manual parameter counter and the empty-list special
// case all collapse into `${?...}`.

fn issues_sql(kind: Option<&str>, workspace: Option<&str>, min_count: Option<i32>) -> String {
    query!(
        r#"
        SELECT id, kind, workspace FROM content_issue
        WHERE kind = ${?kind}
          AND workspace = ${?workspace}
          AND count >= ${?min_count}
        ORDER BY id
    "#
    )
    .sql()
}

#[test]
fn conditional_filters_replace_where_parts_vec() {
    // Nothing set: `WHERE` disappears; the original code handled this with an
    // explicit `is_empty()` branch.
    assert_eq!(
        norm(issues_sql(None, None, None)),
        "SELECT id, kind, workspace FROM content_issue ORDER BY id"
    );

    // One set: `WHERE` is introduced and numbered `$1` — no counter to keep.
    assert_eq!(
        norm(issues_sql(None, Some("main"), None)),
        "SELECT id, kind, workspace FROM content_issue WHERE workspace = $1 ORDER BY id"
    );

    // All set: numbering follows template order.
    assert_eq!(
        norm(issues_sql(Some("i18n"), Some("main"), Some(5))),
        "SELECT id, kind, workspace FROM content_issue \
         WHERE kind = $1 AND workspace = $2 AND count >= $3 ORDER BY id"
    );

    // Middle one dropped: the survivors renumber themselves.
    assert_eq!(
        norm(issues_sql(Some("i18n"), None, Some(5))),
        "SELECT id, kind, workspace FROM content_issue \
         WHERE kind = $1 AND count >= $2 ORDER BY id"
    );
}

// ---------------------------------------------------------------------------
// 5. Array filters `= ANY($n::text[])`
// ---------------------------------------------------------------------------
//
// Original (i18n.rs:1907):
//
//     where_parts.push(format!("r.data->>'{field}' = ANY(${next_param}::text[])"));
//
// Two interpolations into SQL text: `{field}` (a column path) and
// `{next_param}` (a hand-counted parameter number). The parameter number goes
// away entirely; the field name, being SQL syntax, must be a fragment from a
// closed set.

/// Column paths come from a closed set, so a hostile `field` cannot reach the
/// SQL.
#[derive(Clone, Copy)]
enum I18nField {
    Locale,
    Namespace,
}

impl I18nField {
    fn fragment(self) -> SqlFragment {
        const LOCALE: SqlFragment = sql_fragment!("r.data->>'locale'");
        const NAMESPACE: SqlFragment = sql_fragment!("r.data->>'namespace'");
        match self {
            Self::Locale => LOCALE,
            Self::Namespace => NAMESPACE,
        }
    }
}

fn i18n_any_sql(field: I18nField, values: &[String]) -> String {
    let column = field.fragment();
    query!(
        r#"
        SELECT id FROM record r
        WHERE #{column} = ANY(${values}::text[])
    "#
    )
    .sql()
}

#[test]
fn array_filter_matches_original() {
    assert_eq!(
        norm(i18n_any_sql(I18nField::Locale, &["ru".into()])),
        norm("SELECT id FROM record r WHERE r.data->>'locale' = ANY($1::text[])")
    );
    assert_eq!(
        norm(i18n_any_sql(I18nField::Namespace, &["ui".into()])),
        norm("SELECT id FROM record r WHERE r.data->>'namespace' = ANY($1::text[])")
    );
}

// ---------------------------------------------------------------------------
// 6. What must stay a `format!`
// ---------------------------------------------------------------------------
//
// Original (tests/migrations.rs:33-47):
//
//     sqlx::query(&format!("DROP DATABASE IF EXISTS \"{name}\"")).execute(...)
//     sqlx::query(&format!("CREATE DATABASE \"{name}\"")).execute(...)
//
// Postgres does not accept parameters for DDL identifiers, so no bind can
// express this. It is out of scope for this crate by design, and the test
// records that rather than pretending otherwise.

#[test]
fn ddl_identifiers_are_out_of_scope() {
    // A database name is an identifier, not a value: `CREATE DATABASE $1` is a
    // syntax error in Postgres no matter how the SQL is assembled. Such call
    // sites keep using `format!` with a validated identifier.
    //
    // Forcing them through this crate would mean making the name a fragment —
    // and `SqlFragment` has no constructor for runtime strings, which is
    // exactly the point: the crate refuses to launder an identifier into SQL.
    const CREATE_TEST_DB: SqlFragment = sql_fragment!("CREATE DATABASE \"cms_test_fixed\"");
    assert_eq!(
        query!("#{CREATE_TEST_DB}").sql(),
        "CREATE DATABASE \"cms_test_fixed\""
    );
}

// ---------------------------------------------------------------------------
// 7. Hand-counted placeholder numbers — the strongest case for the macro
// ---------------------------------------------------------------------------
//
// Original (audit.rs:542-568). One condition, `input.day.is_some()`, is checked
// THREE times: once to pick the SQL fragment, once to pick the placeholder
// number, and once to decide whether to bind. Any two of the three drifting
// apart is a runtime error that no happy-path test catches.
//
//     let day_filter = match input.day.is_some() {
//         true => " AND al.create_time >= ($2::date)::timestamp AT TIME ZONE 'UTC' \
//                   AND al.create_time < ($2::date + 1)::timestamp AT TIME ZONE 'UTC'",
//         false => "",
//     };
//     let feed_sql = format!(
//         "SELECT ... FROM system.record_audit_log al{AUTHOR_JOINS}
//          WHERE al.author = ANY($1) AND {REPRESENTABLE_KINDS}{day_filter}
//          ORDER BY al.create_time DESC
//          LIMIT ${}",
//         match input.day.is_some() { true => 3, false => 2 },   // <-- hand-counted
//     );
//     let mut feed_query = sqlx::query(&feed_sql).bind(&all_ids);
//     if let Some(day) = input.day.as_ref() { feed_query = feed_query.bind(day); }
//     let feed_rows = feed_query.bind(i64::from(limit))

const AUDIT_KINDS: SqlFragment = sql_fragment!(
    "al.kind::text IN ('document', 'template', 'i18n', 'document-menu', \
     'template-param', 'content-rule')"
);

fn audit_feed_sql(all_ids: &[i64], day: Option<&str>, limit: i64) -> String {
    query!(
        r#"
        SELECT al.record_id, al.create_time
        FROM system.record_audit_log al
        WHERE al.author = ANY(${all_ids})
          AND #{AUDIT_KINDS}
          AND al.create_time >= ${?day}
        ORDER BY al.create_time DESC
        LIMIT ${limit}
    "#
    )
    .sql()
}

#[test]
fn trailing_bind_renumbers_itself_when_optional_predicate_drops() {
    // Without `day`: LIMIT takes $2 — the original code's `false => 2` branch.
    let without = norm(audit_feed_sql(&[1], None, 20));
    assert!(without.contains("ANY($1)"), "{without}");
    assert!(without.ends_with("LIMIT $2"), "{without}");
    assert!(!without.contains("create_time >="), "{without}");

    // With `day`: LIMIT shifts to $3 — the original code's `true => 3` branch,
    // except nothing had to be counted by hand.
    let with = norm(audit_feed_sql(&[1], Some("2026-01-01"), 20));
    assert!(with.contains("ANY($1)"), "{with}");
    assert!(with.contains("al.create_time >= $2"), "{with}");
    assert!(with.ends_with("LIMIT $3"), "{with}");
}

#[test]
fn one_condition_is_stated_once_not_three_times() {
    // The point of the rewrite: `day` appears in the source exactly once, so the
    // fragment, the placeholder number and the bind cannot disagree.
    for day in [None, Some("2026-01-01")] {
        let sql = norm(audit_feed_sql(&[1], day, 20));
        let binds = sql.matches('$').count();
        let expected = if day.is_some() { 3 } else { 2 };
        assert_eq!(binds, expected, "placeholder count must match binds: {sql}");
    }
}

// ---------------------------------------------------------------------------
// 8. Known limitation: one value in two places is bound twice
// ---------------------------------------------------------------------------
//
// The original `day_filter` referenced `$2` twice (`>=` and `<`) for a single
// bound value. Since every interpolation is evaluated and bound independently
// (no deduplication — see the crate docs), the rewrite sends the value twice.
// The SQL is correct; it just uses one parameter more than the hand-written
// form.

#[test]
fn repeated_value_binds_once_per_interpolation() {
    fn range_sql(day: Option<&str>) -> String {
        query!(
            r#"
            SELECT * FROM t
            WHERE create_time >= ${?day}
              AND create_time < ${?day}
        "#
        )
        .sql()
    }

    // Two separate parameters for one value — documented, not accidental.
    assert_eq!(
        norm(range_sql(Some("2026-01-01"))),
        "SELECT * FROM t WHERE create_time >= $1 AND create_time < $2"
    );
    // Both vanish together, so the clause cannot be left half-built.
    assert_eq!(norm(range_sql(None)), "SELECT * FROM t");
}

//! The injection model.
//!
//! Two halves:
//!   - `${expr}` must never reach the SQL text, whatever the value contains.
//!   - `#{expr}` must accept only a `SqlFragment` (the compile-fail cases live in
//!     `tests/compile_fail/`).

use sqlx_dyn::{query, query_as, query_scalar, sql_fragment, SqlFragment};

/// Classic payloads. If any of them ever shows up in `.sql()`, the bind path is
/// broken and the crate is injectable.
const PAYLOADS: &[&str] = &[
    "1' OR '1'='1",
    "'; DROP TABLE users; --",
    "\\'; DROP TABLE users; --",
    "1; DELETE FROM users",
    "admin'--",
    "' UNION SELECT password FROM users --",
    "$1",
    "${nested}",
    "#{nested}",
    "1/*comment*/OR/**/1=1",
    "'||(SELECT password FROM users)||'",
    "\0truncated",
];

#[test]
fn bound_values_never_appear_in_sql_text() {
    for payload in PAYLOADS {
        let q = query!("SELECT * FROM users WHERE name = ${payload}");
        assert_eq!(
            q.sql(),
            "SELECT * FROM users WHERE name = $1",
            "payload leaked into the SQL: {payload:?}"
        );
        assert!(
            !q.sql().contains("DROP") && !q.sql().contains("UNION"),
            "payload leaked into the SQL: {payload:?}"
        );
    }
}

#[test]
fn payload_in_bind_does_not_add_parameters() {
    // A value containing `$1`/`${...}` must not be re-read as template text.
    let payload = "${evil} $1 $2 #{evil}";
    let q = query!("SELECT ${payload}, ${payload}");
    assert_eq!(q.sql(), "SELECT $1, $2");
}

#[test]
fn payload_cannot_close_a_string_literal_in_surrounding_sql() {
    // The bind sits in a quoted context in the template; the value must not be
    // able to escape it, because it is not substituted textually at all.
    let payload = "' OR 1=1 --";
    let q = query!("SELECT * FROM t WHERE a = ${payload} AND b = 'literal'");
    assert_eq!(
        q.sql(),
        "SELECT * FROM t WHERE a = $1 AND b = 'literal'"
    );
}

#[test]
fn payloads_are_inert_across_all_three_macros() {
    #[derive(sqlx::FromRow)]
    struct Row {
        #[allow(dead_code)]
        id: i64,
    }

    let payload = "'; DROP TABLE users; --";

    let a = query!("SELECT id FROM t WHERE x = ${payload}");
    assert_eq!(a.sql(), "SELECT id FROM t WHERE x = $1");

    let b = query_as!(Row, "SELECT id FROM t WHERE x = ${payload}");
    assert_eq!(b.sql(), "SELECT id FROM t WHERE x = $1");

    let c = query_scalar!("SELECT COUNT(*) FROM t WHERE x = ${payload}");
    assert_eq!(c.sql(), "SELECT COUNT(*) FROM t WHERE x = $1");
}

#[test]
fn fragment_text_is_the_only_thing_inlined_verbatim() {
    // Fragments are SQL by design; this pins down that they inline exactly and
    // that a bind beside them still keeps its parameter slot.
    const F: SqlFragment = sql_fragment!("kind IN ('a','b')");
    let v: i64 = 1;
    let q = query!("WHERE #{F} AND id = ${v}");
    assert_eq!(q.sql(), "WHERE kind IN ('a','b') AND id = $1");
}

#[test]
fn escaped_marker_in_template_is_not_an_interpolation() {
    // `$${` must stay literal text and must not create a bind slot — otherwise
    // the author could be surprised about which values are parameterised.
    let q = query!("SELECT '$${x}' AS lit");
    assert_eq!(q.sql(), "SELECT '${x}' AS lit");
    assert!(!q.sql().contains("$1"));
}

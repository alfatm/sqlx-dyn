//! Integration tests for optional predicates `${?expr}`.
//!
//! There is no database, so every assertion checks the SQL text from
//! `.sql()`. `${?expr}` takes an `Option<T>`: `Some(v)` keeps the predicate
//! and binds `v`, `None` drops the predicate together with the `AND`/`OR`/
//! `WHERE` that introduces it — and drops the `WHERE` entirely if no
//! predicate survived.

use std::cell::Cell;

use sqlx_dyn::{query, query_as, query_scalar, sql_fragment, SqlFragment};

const ACTIVE: SqlFragment = sql_fragment!("deleted_at IS NULL");

#[derive(sqlx::FromRow)]
struct User {
    #[allow(dead_code)]
    id: i64,
    #[allow(dead_code)]
    name: String,
}

// 1. single optional predicate: present, or gone along with its `WHERE`

#[test]
fn single_optional_some_keeps_predicate() {
    let x = Some(1i32);
    let q = query!("SELECT * FROM t WHERE col = ${?x}");
    assert_eq!(q.sql(), "SELECT * FROM t WHERE col = $1");
}

#[test]
fn single_optional_none_removes_where() {
    let x: Option<i32> = None;
    let q = query!("SELECT * FROM t WHERE col = ${?x}");
    assert_eq!(q.sql(), "SELECT * FROM t");
}

// 2. an unconditional bind keeps `WHERE` alive; only the `AND` collapses

#[test]
fn required_then_optional_some() {
    let org = 7i64;
    let name = Some("ada");
    let q = query!("SELECT * FROM t WHERE org = ${org} AND name = ${?name}");
    assert_eq!(q.sql(), "SELECT * FROM t WHERE org = $1 AND name = $2");
}

#[test]
fn required_then_optional_none() {
    let org = 7i64;
    let name: Option<&str> = None;
    let q = query!("SELECT * FROM t WHERE org = ${org} AND name = ${?name}");
    assert_eq!(q.sql(), "SELECT * FROM t WHERE org = $1");
}

// 3. two optionals after an unconditional predicate — all four combinations

#[test]
fn required_then_two_optionals_both_some() {
    let org = 7i64;
    let name = Some("ada");
    let age = Some(36i32);
    let q = query!("SELECT * FROM t WHERE org = ${org} AND name = ${?name} AND age = ${?age}");
    assert_eq!(
        q.sql(),
        "SELECT * FROM t WHERE org = $1 AND name = $2 AND age = $3"
    );
}

#[test]
fn required_then_two_optionals_second_none() {
    let org = 7i64;
    let name = Some("ada");
    let age: Option<i32> = None;
    let q = query!("SELECT * FROM t WHERE org = ${org} AND name = ${?name} AND age = ${?age}");
    assert_eq!(q.sql(), "SELECT * FROM t WHERE org = $1 AND name = $2");
}

#[test]
fn required_then_two_optionals_first_none() {
    let org = 7i64;
    let name: Option<&str> = None;
    let age = Some(36i32);
    let q = query!("SELECT * FROM t WHERE org = ${org} AND name = ${?name} AND age = ${?age}");
    assert_eq!(q.sql(), "SELECT * FROM t WHERE org = $1 AND age = $2");
}

#[test]
fn required_then_two_optionals_both_none() {
    let org = 7i64;
    let name: Option<&str> = None;
    let age: Option<i32> = None;
    let q = query!("SELECT * FROM t WHERE org = ${org} AND name = ${?name} AND age = ${?age}");
    assert_eq!(q.sql(), "SELECT * FROM t WHERE org = $1");
}

// 4. two optionals and nothing unconditional: `WHERE` belongs to whichever
// survived first, so a dropped first predicate must not leave an `AND`.

#[test]
fn two_optionals_both_some() {
    let x = Some("ada");
    let y = Some(1i32);
    let q = query!("SELECT * FROM t WHERE a = ${?x} AND b = ${?y}");
    assert_eq!(q.sql(), "SELECT * FROM t WHERE a = $1 AND b = $2");
}

#[test]
fn two_optionals_second_none() {
    let x = Some("ada");
    let y: Option<i32> = None;
    let q = query!("SELECT * FROM t WHERE a = ${?x} AND b = ${?y}");
    assert_eq!(q.sql(), "SELECT * FROM t WHERE a = $1");
}

#[test]
fn two_optionals_first_none_introduces_second_with_where() {
    let x: Option<&str> = None;
    let y = Some(1i32);
    let q = query!("SELECT * FROM t WHERE a = ${?x} AND b = ${?y}");
    // The surviving predicate is introduced by `WHERE`, not by the
    // template's own `AND`.
    assert_eq!(q.sql(), "SELECT * FROM t WHERE b = $1");
}

#[test]
fn two_optionals_both_none_removes_where() {
    let x: Option<&str> = None;
    let y: Option<i32> = None;
    let q = query!("SELECT * FROM t WHERE a = ${?x} AND b = ${?y}");
    assert_eq!(q.sql(), "SELECT * FROM t");
}

// 5. `$N` numbering follows the surviving binds, not the template positions

#[test]
fn dropped_predicate_does_not_consume_a_parameter_slot() {
    let x: Option<&str> = None;
    let y = Some(1i32);
    let q = query!("SELECT * FROM t WHERE a = ${?x} AND b = ${?y}");
    assert_eq!(q.sql(), "SELECT * FROM t WHERE b = $1");
}

#[test]
fn numbering_closes_the_gap_of_a_middle_predicate() {
    let a = Some(1i32);
    let b: Option<i32> = None;
    let c = Some(3i32);
    let q = query!("SELECT * FROM t WHERE a = ${?a} AND b = ${?b} AND c = ${?c}");
    assert_eq!(q.sql(), "SELECT * FROM t WHERE a = $1 AND c = $2");
}

// 6. `OR` as a joiner

#[test]
fn or_joiner_both_some() {
    let x = Some("ada");
    let y = Some(1i32);
    let q = query!("SELECT * FROM t WHERE a = ${?x} OR b = ${?y}");
    assert_eq!(q.sql(), "SELECT * FROM t WHERE a = $1 OR b = $2");
}

#[test]
fn or_joiner_first_none_switches_to_where() {
    let x: Option<&str> = None;
    let y = Some(1i32);
    let q = query!("SELECT * FROM t WHERE a = ${?x} OR b = ${?y}");
    assert_eq!(q.sql(), "SELECT * FROM t WHERE b = $1");
}

#[test]
fn or_joiner_second_none() {
    let x = Some("ada");
    let y: Option<i32> = None;
    let q = query!("SELECT * FROM t WHERE a = ${?x} OR b = ${?y}");
    assert_eq!(q.sql(), "SELECT * FROM t WHERE a = $1");
}

#[test]
fn or_joiner_both_none() {
    let x: Option<&str> = None;
    let y: Option<i32> = None;
    let q = query!("SELECT * FROM t WHERE a = ${?x} OR b = ${?y}");
    assert_eq!(q.sql(), "SELECT * FROM t");
}

// 7. comparison operators other than `=`

#[test]
fn ilike_not_equal_and_gte_operators() {
    let n = Some("ada");
    let m = Some(2i32);
    let k = Some(3i32);
    let q = query!("SELECT * FROM t WHERE n ILIKE ${?n} AND m <> ${?m} AND k >= ${?k}");
    assert_eq!(
        q.sql(),
        "SELECT * FROM t WHERE n ILIKE $1 AND m <> $2 AND k >= $3"
    );
}

#[test]
fn ilike_not_equal_and_gte_all_none() {
    let n: Option<&str> = None;
    let m: Option<i32> = None;
    let k: Option<i32> = None;
    let q = query!("SELECT * FROM t WHERE n ILIKE ${?n} AND m <> ${?m} AND k >= ${?k}");
    assert_eq!(q.sql(), "SELECT * FROM t");
}

#[test]
fn like_and_lte_and_lt_operators() {
    let a = Some("a%");
    let b = Some(1i32);
    let c = Some(2i32);
    let q = query!("SELECT * FROM t WHERE a LIKE ${?a} AND b <= ${?b} AND c < ${?c}");
    assert_eq!(q.sql(), "SELECT * FROM t WHERE a LIKE $1 AND b <= $2 AND c < $3");
}

#[test]
fn not_equal_bang_form_and_gt() {
    let a = Some(1i32);
    let b = Some(2i32);
    let q = query!("SELECT * FROM t WHERE a != ${?a} AND b > ${?b}");
    assert_eq!(q.sql(), "SELECT * FROM t WHERE a != $1 AND b > $2");
}

// 8. text after the predicates survives, including when all of them drop

#[test]
fn trailing_clause_survives_with_predicate() {
    let x = Some(1i32);
    let q = query!("SELECT * FROM t WHERE a = ${?x} ORDER BY id LIMIT 10");
    assert_eq!(q.sql(), "SELECT * FROM t WHERE a = $1 ORDER BY id LIMIT 10");
}

#[test]
fn trailing_clause_survives_without_predicate() {
    let x: Option<i32> = None;
    let q = query!("SELECT * FROM t WHERE a = ${?x} ORDER BY id LIMIT 10");
    assert_eq!(q.sql(), "SELECT * FROM t ORDER BY id LIMIT 10");
}

#[test]
fn trailing_clause_survives_when_all_of_two_drop() {
    let x: Option<i32> = None;
    let y: Option<i32> = None;
    let q = query!("SELECT * FROM t WHERE a = ${?x} AND b = ${?y} ORDER BY id");
    assert_eq!(q.sql(), "SELECT * FROM t ORDER BY id");
}

// 9. `${?x}` mixed with `#{fragment}`

#[test]
fn fragment_predicate_then_optional_some() {
    let x = Some(1i32);
    let q = query!("SELECT * FROM t WHERE #{ACTIVE} AND a = ${?x}");
    assert_eq!(
        q.sql(),
        "SELECT * FROM t WHERE deleted_at IS NULL AND a = $1"
    );
}

#[test]
fn fragment_predicate_keeps_where_when_optional_drops() {
    let x: Option<i32> = None;
    let q = query!("SELECT * FROM t WHERE #{ACTIVE} AND a = ${?x}");
    assert_eq!(q.sql(), "SELECT * FROM t WHERE deleted_at IS NULL");
}

#[test]
fn fragment_after_optional_and_required_bind() {
    let org = 7i64;
    let x = Some(1i32);
    let q = query!("SELECT * FROM t WHERE org = ${org} AND a = ${?x} AND #{ACTIVE}");
    assert_eq!(
        q.sql(),
        "SELECT * FROM t WHERE org = $1 AND a = $2 AND deleted_at IS NULL"
    );
}

#[test]
fn fragment_order_by_after_dropped_optional() {
    let org = 7i64;
    let x: Option<i32> = None;
    let q = query!("SELECT * FROM t WHERE org = ${org} AND a = ${?x} #{ORDER_BY_ID}");
    assert_eq!(q.sql(), "SELECT * FROM t WHERE org = $1 ORDER BY id");
}

const ORDER_BY_ID: SqlFragment = sql_fragment!("ORDER BY id");

// 10. the two other macro entry points

#[test]
fn query_as_with_optional_some() {
    let id = Some(1i64);
    let q = query_as!(User, "SELECT id, name FROM t WHERE id = ${?id}");
    assert_eq!(q.sql(), "SELECT id, name FROM t WHERE id = $1");
}

#[test]
fn query_as_with_optional_none() {
    let id: Option<i64> = None;
    let q = query_as!(User, "SELECT id, name FROM t WHERE id = ${?id}");
    assert_eq!(q.sql(), "SELECT id, name FROM t");
}

#[test]
fn query_scalar_with_optional_some() {
    let id = Some(1i64);
    let q = query_scalar!("SELECT count(*) FROM t WHERE id = ${?id}");
    assert_eq!(q.sql(), "SELECT count(*) FROM t WHERE id = $1");
}

#[test]
fn query_scalar_with_optional_none() {
    let id: Option<i64> = None;
    let q = query_scalar!("SELECT count(*) FROM t WHERE id = ${?id}");
    assert_eq!(q.sql(), "SELECT count(*) FROM t");
}

// The optional path routes every piece through `Predicates`, so the
// executable surface is worth typechecking too (compiles, never runs — there
// is no database).
#[allow(dead_code)]
async fn typecheck_optional_execution(pool: &sqlx::PgPool) -> sqlx::Result<()> {
    let id: Option<i64> = None;
    let _: Vec<sqlx::postgres::PgRow> = query!("SELECT id FROM t WHERE id = ${?id}")
        .fetch_all(pool)
        .await?;
    let _: Vec<User> = query_as!(User, "SELECT id, name FROM t WHERE id = ${?id}")
        .fetch_all(pool)
        .await?;
    let _: Vec<i64> = query_scalar!("SELECT id FROM t WHERE id = ${?id}")
        .fetch_all(pool)
        .await?;
    let _: sqlx::postgres::PgQueryResult = query!("DELETE FROM t WHERE id = ${?id}")
        .execute(pool)
        .await?;
    Ok(())
}

// 11. removing a predicate must not leave whitespace debris

#[test]
fn no_whitespace_debris_in_single_line_templates() {
    // All combinations of a three-predicate template; a joiner's own
    // indentation is consumed at parse time, so the result must not contain a
    // double space or stray leading/trailing whitespace inside the SQL.
    for a in [Some(1i32), None] {
        for b in [Some(2i32), None] {
            for c in [Some(3i32), None] {
                let sql =
                    query!("SELECT * FROM t WHERE a = ${?a} AND b = ${?b} OR c = ${?c} ORDER BY id")
                        .sql();
                assert!(!sql.contains("  "), "double space in {sql:?}");
                assert_eq!(sql, sql.trim(), "stray outer whitespace in {sql:?}");
            }
        }
    }
}

#[test]
fn no_blank_lines_from_dropped_predicates_in_multiline_template() {
    let a: Option<i32> = None;
    let b: Option<i32> = None;
    let sql = query!(
        r#"SELECT id
    FROM t
    WHERE a = ${?a}
      AND b = ${?b}
    ORDER BY id"#
    )
    .sql();
    assert_eq!(sql, "SELECT id\n    FROM t\n    ORDER BY id");
    assert!(
        !sql.lines().any(|line| line.trim().is_empty()),
        "blank line in {sql:?}"
    );
}

#[test]
fn surviving_predicates_are_joined_by_a_single_space() {
    // The whitespace *before* every joiner is consumed at parse time —
    // including the newline and indentation before `WHERE` — and re-emitted as
    // a single space. So a multi-line predicate list collapses onto the line
    // of the preceding SQL instead of leaving ragged holes where predicates
    // were removed.
    let a = Some(1i32);
    let b: Option<i32> = None;
    let c = Some(3i32);
    let sql = query!(
        r#"SELECT id
    FROM t
    WHERE a = ${?a}
      AND b = ${?b}
      AND c = ${?c}
    ORDER BY id"#
    )
    .sql();
    assert_eq!(
        sql,
        "SELECT id\n    FROM t WHERE a = $1 AND c = $2\n    ORDER BY id"
    );
}

// 12. the expression is evaluated exactly once, survive or not

#[test]
fn optional_expression_is_evaluated_exactly_once_when_some() {
    fn bump(counter: &Cell<i32>) -> Option<i32> {
        counter.set(counter.get() + 1);
        Some(counter.get())
    }

    let counter = Cell::new(0);
    let q = query!("SELECT * FROM t WHERE a = ${?bump(&counter)}");
    assert_eq!(q.sql(), "SELECT * FROM t WHERE a = $1");
    assert_eq!(counter.get(), 1);
}

#[test]
fn optional_expression_is_evaluated_exactly_once_when_none() {
    fn bump_none(counter: &Cell<i32>) -> Option<i32> {
        counter.set(counter.get() + 1);
        None
    }

    let counter = Cell::new(0);
    let q = query!("SELECT * FROM t WHERE a = ${?bump_none(&counter)}");
    assert_eq!(q.sql(), "SELECT * FROM t");
    // The predicate is gone, but the expression still ran — exactly once.
    assert_eq!(counter.get(), 1);
}

#[test]
fn each_optional_expression_is_evaluated_once_in_source_order() {
    fn bump(counter: &Cell<i32>) -> Option<i32> {
        counter.set(counter.get() + 1);
        Some(counter.get())
    }

    let counter = Cell::new(0);
    let q = query!("SELECT * FROM t WHERE a = ${?bump(&counter)} AND b = ${?bump(&counter)}");
    assert_eq!(q.sql(), "SELECT * FROM t WHERE a = $1 AND b = $2");
    assert_eq!(counter.get(), 2);
}

// `$${?x}` is an escape, not an optional predicate.
#[test]
fn escaped_optional_marker_is_literal() {
    let q = query!("SELECT '$${?x}' FROM t");
    assert_eq!(q.sql(), "SELECT '${?x}' FROM t");
}

// ---------------------------------------------------------------------------
// Regression: unconditional SQL after a dropped optional predicate
// ---------------------------------------------------------------------------
//
// An optional predicate owns the `WHERE`. When it is dropped, any predicate
// that follows must be introduced by `WHERE` instead of keeping its literal
// `AND`, otherwise the SQL comes out as `... FROM t AND b IS NULL`.

#[test]
fn dropped_optional_hands_where_to_following_literal_predicate() {
    let x: Option<i32> = None;
    assert_eq!(
        query!("SELECT * FROM t WHERE a = ${?x} AND b IS NULL").sql(),
        "SELECT * FROM t WHERE b IS NULL"
    );
}

#[test]
fn surviving_optional_keeps_following_and() {
    let x: Option<i32> = Some(1);
    assert_eq!(
        query!("SELECT * FROM t WHERE a = ${?x} AND b IS NULL").sql(),
        "SELECT * FROM t WHERE a = $1 AND b IS NULL"
    );
}

#[test]
fn dropped_optional_hands_where_to_following_fragment() {
    const ACTIVE: SqlFragment = sql_fragment!("deleted_at IS NULL");
    let x: Option<i32> = None;
    assert_eq!(
        query!("SELECT * FROM t WHERE a = ${?x} AND #{ACTIVE}").sql(),
        "SELECT * FROM t WHERE deleted_at IS NULL"
    );
}

#[test]
fn dropped_optional_hands_where_to_following_bind() {
    let x: Option<i32> = None;
    let y = 5i64;
    assert_eq!(
        query!("SELECT * FROM t WHERE a = ${?x} AND b = ${y}").sql(),
        "SELECT * FROM t WHERE b = $1"
    );
}

#[test]
fn dropped_optional_with_or_joiner_following() {
    let x: Option<i32> = None;
    assert_eq!(
        query!("SELECT * FROM t WHERE a = ${?x} OR b IS NULL").sql(),
        "SELECT * FROM t WHERE b IS NULL"
    );
}

#[test]
fn order_by_after_optional_is_not_mistaken_for_or() {
    // `ORDER` starts with `OR`; the word-boundary check must not split it.
    let x: Option<i32> = None;
    assert_eq!(
        query!("SELECT * FROM t WHERE a = ${?x} ORDER BY id").sql(),
        "SELECT * FROM t ORDER BY id"
    );
    let x: Option<i32> = Some(1);
    assert_eq!(
        query!("SELECT * FROM t WHERE a = ${?x} ORDER BY id").sql(),
        "SELECT * FROM t WHERE a = $1 ORDER BY id"
    );
}

#[test]
fn group_by_and_limit_survive_dropped_optional() {
    let x: Option<i32> = None;
    assert_eq!(
        query!("SELECT k, count(*) FROM t WHERE a = ${?x} GROUP BY k LIMIT 10").sql(),
        "SELECT k, count(*) FROM t GROUP BY k LIMIT 10"
    );
}

// --- text to the right of the marker (casts, concatenation) ---

#[test]
fn cast_after_the_marker_vanishes_with_the_predicate() {
    // Regression: the cast used to be emitted unconditionally and stuck to
    // the table name, yielding `SELECT * FROM t::uuid`.
    let x: Option<&str> = None;
    assert_eq!(
        query!("SELECT * FROM t WHERE a = ${?x}::uuid").sql(),
        "SELECT * FROM t"
    );
    let x: Option<&str> = Some("k");
    assert_eq!(
        query!("SELECT * FROM t WHERE a = ${?x}::uuid").sql(),
        "SELECT * FROM t WHERE a = $1::uuid"
    );
}

#[test]
fn cast_does_not_attach_to_the_preceding_predicate() {
    let x: Option<&str> = None;
    assert_eq!(
        query!("SELECT * FROM t WHERE org = 1 AND a = ${?x}::uuid").sql(),
        "SELECT * FROM t WHERE org = 1"
    );
}

#[test]
fn concatenation_after_the_marker_vanishes_too() {
    let x: Option<&str> = None;
    assert_eq!(
        query!("SELECT * FROM t WHERE a LIKE ${?x} || '%'").sql(),
        "SELECT * FROM t"
    );
    let x: Option<&str> = Some("k");
    assert_eq!(
        query!("SELECT * FROM t WHERE a LIKE ${?x} || '%'").sql(),
        "SELECT * FROM t WHERE a LIKE $1 || '%'"
    );
}

#[test]
fn predicate_after_a_dropped_cast_predicate_is_promoted_to_where() {
    let x: Option<&str> = None;
    assert_eq!(
        query!("SELECT * FROM t WHERE a = ${?x}::uuid AND b = 1").sql(),
        "SELECT * FROM t WHERE b = 1"
    );
}

#[test]
fn clause_keyword_ends_the_tail_not_swallowed_by_it() {
    let x: Option<&str> = None;
    assert_eq!(
        query!("SELECT * FROM t WHERE a = ${?x}::uuid ORDER BY id").sql(),
        "SELECT * FROM t ORDER BY id"
    );
    let x: Option<&str> = Some("k");
    assert_eq!(
        query!("SELECT * FROM t WHERE a = ${?x}::uuid ORDER BY id").sql(),
        "SELECT * FROM t WHERE a = $1::uuid ORDER BY id"
    );
}

#[test]
fn tail_stops_at_the_next_interpolation() {
    // The tail must never swallow another marker: that would turn a bind into
    // literal SQL text.
    let x: Option<i32> = Some(1);
    let y: i32 = 2;
    assert_eq!(
        query!("SELECT * FROM t WHERE a = ${?x} AND b = ${y}").sql(),
        "SELECT * FROM t WHERE a = $1 AND b = $2"
    );
}

#[test]
fn tail_stops_at_a_closing_paren() {
    let x: Option<i32> = Some(1);
    assert_eq!(
        query!("SELECT * FROM t WHERE EXISTS (SELECT 1 FROM u WHERE k = ${?x}::int)").sql(),
        "SELECT * FROM t WHERE EXISTS (SELECT 1 FROM u WHERE k = $1::int)"
    );
}

// --- SQL comments (rejected only where the `${?...}` logic relies on them) ---

#[test]
fn comment_lookalike_inside_a_literal_is_not_rejected() {
    // `'a--b'` is data, not a comment; the guard must skip string literals.
    let x: Option<i32> = Some(1);
    assert_eq!(
        query!("SELECT * FROM t WHERE note = 'a--b' AND col = ${?x}").sql(),
        "SELECT * FROM t WHERE note = 'a--b' AND col = $1"
    );
}

#[test]
fn comments_are_still_allowed_without_optional_predicates() {
    // Nothing is removed here, so a comment cannot hide a joiner. The newline
    // that terminates it is preserved — that is what keeps the SQL valid.
    let id: i64 = 1;
    assert_eq!(
        query!("SELECT * FROM t -- note\n WHERE id = ${id}").sql(),
        "SELECT * FROM t -- note\n WHERE id = $1"
    );
}

// --- one predicate list per template ---

#[test]
fn nested_where_in_a_subquery_is_not_a_second_clause() {
    // The `WHERE` inside `EXISTS` is nested, not a second top-level list, so
    // it must not trip the single-clause guard.
    let x: Option<i32> = Some(1);
    assert_eq!(
        query!("SELECT * FROM t WHERE EXISTS (SELECT 1 FROM u WHERE u.t = t.id) AND k = ${?x}")
            .sql(),
        "SELECT * FROM t WHERE EXISTS (SELECT 1 FROM u WHERE u.t = t.id) AND k = $1"
    );
}

#[test]
fn a_subquery_where_may_hold_the_optional_itself() {
    let x: Option<i32> = Some(2);
    assert_eq!(
        query!("SELECT * FROM t WHERE EXISTS (SELECT 1 FROM u WHERE k = ${?x})").sql(),
        "SELECT * FROM t WHERE EXISTS (SELECT 1 FROM u WHERE k = $1)"
    );
}

#[test]
fn a_keyword_inside_a_literal_is_not_a_second_clause() {
    let x: Option<i32> = Some(1);
    assert_eq!(
        query!("SELECT * FROM t WHERE tag = 'union having' AND k = ${?x}").sql(),
        "SELECT * FROM t WHERE tag = 'union having' AND k = $1"
    );
}

#[test]
fn multiple_clauses_are_fine_without_optional_predicates() {
    let a: i64 = 1;
    assert_eq!(
        query!("SELECT x FROM t WHERE a = ${a} UNION SELECT x FROM u WHERE b = 2").sql(),
        "SELECT x FROM t WHERE a = $1 UNION SELECT x FROM u WHERE b = 2"
    );
}

// --- several predicate lists, each with its own joiner bookkeeping ---

#[test]
fn each_union_branch_introduces_its_own_where() {
    // Regression: the `WHERE` of clause 2 belonged to `b`; when `b` was
    // dropped, `c` inherited its literal `AND` and gave `FROM u AND c = $2`.
    let a: Option<i64> = Some(1);
    let b: Option<i64> = None;
    let c: Option<i64> = Some(3);
    assert_eq!(
        query!(
            "SELECT * FROM t WHERE a = ${?a} \
             UNION SELECT * FROM u WHERE b = ${?b} AND c = ${?c}"
        )
        .sql(),
        "SELECT * FROM t WHERE a = $1 UNION SELECT * FROM u WHERE c = $2"
    );
}

#[test]
fn an_unclaimed_where_does_not_migrate_into_a_later_clause() {
    // Regression: the `WHERE` left unused by the dropped `x` grew in after
    // `HAVING`, giving `GROUP BY k HAVING c = 1 WHERE d = $1`.
    let x: Option<i64> = None;
    let y: Option<i64> = Some(5);
    assert_eq!(
        query!("SELECT k FROM t WHERE a = ${?x} GROUP BY k HAVING c = 1 AND d = ${?y}").sql(),
        "SELECT k FROM t GROUP BY k HAVING c = 1 AND d = $1"
    );
}

#[test]
fn an_unclaimed_where_does_not_produce_a_second_where_after_union() {
    // Regression: used to give
    // `... UNION SELECT * FROM u WHERE b = 1 WHERE c = $1`.
    let x: Option<i64> = None;
    let y: Option<i64> = Some(2);
    assert_eq!(
        query!(
            "SELECT * FROM t WHERE a = ${?x} UNION SELECT * FROM u WHERE b = 1 AND c = ${?y}"
        )
        .sql(),
        "SELECT * FROM t UNION SELECT * FROM u WHERE b = 1 AND c = $1"
    );
}

#[test]
fn where_and_having_are_independent_lists() {
    let n: Option<i64> = Some(5);
    assert_eq!(
        query!("SELECT k FROM t WHERE a = ${?n} GROUP BY k HAVING count(*) > ${?n}").sql(),
        "SELECT k FROM t WHERE a = $1 GROUP BY k HAVING count(*) > $2"
    );
}

#[test]
fn having_survives_when_the_where_predicate_drops() {
    let gone: Option<i64> = None;
    let kept: Option<i64> = Some(2);
    assert_eq!(
        query!("SELECT k FROM t WHERE a = ${?gone} GROUP BY k HAVING count(*) > ${?kept}").sql(),
        "SELECT k FROM t GROUP BY k HAVING count(*) > $1"
    );
}

#[test]
fn two_statements_each_get_their_own_where() {
    let a: Option<i64> = Some(1);
    let b: Option<i64> = Some(2);
    assert_eq!(
        query!("SELECT * FROM t WHERE a = ${?a}; SELECT * FROM u WHERE b = ${?b}").sql(),
        "SELECT * FROM t WHERE a = $1; SELECT * FROM u WHERE b = $2"
    );
}

#[test]
fn a_dropped_predicate_in_the_first_statement_does_not_affect_the_second() {
    let a: Option<i64> = None;
    let b: Option<i64> = Some(2);
    assert_eq!(
        query!("SELECT * FROM t WHERE a = ${?a}; SELECT * FROM u WHERE b = ${?b}").sql(),
        "SELECT * FROM t; SELECT * FROM u WHERE b = $1"
    );
}

#[test]
fn intersect_and_except_also_open_new_lists() {
    let x: Option<i64> = None;
    let y: Option<i64> = Some(1);
    assert_eq!(
        query!("SELECT k FROM t WHERE a = ${?x} INTERSECT SELECT k FROM u WHERE b = ${?y}").sql(),
        "SELECT k FROM t INTERSECT SELECT k FROM u WHERE b = $1"
    );
    assert_eq!(
        query!("SELECT k FROM t WHERE a = ${?x} EXCEPT SELECT k FROM u WHERE b = ${?y}").sql(),
        "SELECT k FROM t EXCEPT SELECT k FROM u WHERE b = $1"
    );
}

// --- HAVING and balanced groups left of the operator ---

#[test]
fn having_introduces_a_predicate_list_like_where() {
    let n: Option<i64> = Some(5);
    assert_eq!(
        query!("SELECT a FROM t GROUP BY a HAVING count(*) >= ${?n}").sql(),
        "SELECT a FROM t GROUP BY a HAVING count(*) >= $1"
    );
    let n: Option<i64> = None;
    assert_eq!(
        query!("SELECT a FROM t GROUP BY a HAVING count(*) >= ${?n}").sql(),
        "SELECT a FROM t GROUP BY a"
    );
}

#[test]
fn having_joins_a_second_predicate_with_and() {
    let n: Option<i64> = Some(5);
    assert_eq!(
        query!("SELECT a FROM t GROUP BY a HAVING count(*) >= ${?n} AND sum(b) > 0").sql(),
        "SELECT a FROM t GROUP BY a HAVING count(*) >= $1 AND sum(b) > 0"
    );
}

#[test]
fn a_balanced_call_on_the_left_of_the_operator_is_allowed() {
    // `lower(a)` is the left operand, not an enclosing group: removing the
    // whole predicate is still unambiguous.
    let x: Option<&str> = Some("k");
    assert_eq!(
        query!("SELECT * FROM t WHERE lower(a) = ${?x}").sql(),
        "SELECT * FROM t WHERE lower(a) = $1"
    );
    let x: Option<&str> = None;
    assert_eq!(
        query!("SELECT * FROM t WHERE lower(a) = ${?x}").sql(),
        "SELECT * FROM t"
    );
}

// --- comparison operators require a token boundary ---

#[test]
fn a_column_ending_in_like_is_not_a_like_operator() {
    // Regression: `body_upper.ends_with("LIKE")` matched `dislike`, emitting
    // `WHERE dislike $1` with no operator at all.
    let x: Option<i32> = Some(1);
    assert_eq!(
        query!("SELECT * FROM t WHERE dislike_count = ${?x}").sql(),
        "SELECT * FROM t WHERE dislike_count = $1"
    );
}

// --- literals cannot produce a phantom joiner ---

#[test]
fn a_joiner_word_inside_a_literal_operand_is_ignored() {
    // Regression: the `or` in `'p or q'` was picked as the joiner, and the cut
    // landed in the middle of the literal, leaving `... AND 'p`.
    let u: Option<i32> = Some(1);
    assert_eq!(
        query!("SELECT * FROM t WHERE tag = 1 AND 'p or q' < ${?u}").sql(),
        "SELECT * FROM t WHERE tag = 1 AND 'p or q' < $1"
    );
    let u: Option<i32> = None;
    assert_eq!(
        query!("SELECT * FROM t WHERE tag = 1 AND 'p or q' < ${?u}").sql(),
        "SELECT * FROM t WHERE tag = 1"
    );
}

#[test]
fn a_joiner_word_inside_a_quoted_identifier_is_ignored() {
    let u: Option<i32> = None;
    assert_eq!(
        query!("SELECT * FROM t WHERE k = 1 AND \"c or d\" = ${?u}").sql(),
        "SELECT * FROM t WHERE k = 1"
    );
}

// --- clause boundaries must not be seen where there are none ---

#[test]
fn a_derived_table_keeps_the_enclosing_clause() {
    let n: Option<i64> = None;
    assert_eq!(
        query!("SELECT * FROM (SELECT x FROM u WHERE y = 1) v WHERE v.x = ${?n}").sql(),
        "SELECT * FROM (SELECT x FROM u WHERE y = 1) v"
    );
}

#[test]
fn a_cte_with_its_own_where_does_not_shift_the_outer_clause() {
    let s: Option<i64> = Some(1);
    assert_eq!(
        query!("WITH c AS (SELECT x FROM u WHERE y = 1) SELECT * FROM c WHERE x = ${?s}").sql(),
        "WITH c AS (SELECT x FROM u WHERE y = 1) SELECT * FROM c WHERE x = $1"
    );
}

#[test]
fn a_clause_keyword_inside_a_literal_does_not_start_a_clause() {
    let n: Option<i64> = None;
    assert_eq!(
        query!("SELECT * FROM t WHERE tag = 'union where' AND b = ${?n}").sql(),
        "SELECT * FROM t WHERE tag = 'union where'"
    );
}

#[test]
fn a_union_branch_without_an_optional_leaves_the_other_alone() {
    let n: Option<i64> = None;
    assert_eq!(
        query!("SELECT x FROM t WHERE a = 1 UNION SELECT x FROM u WHERE b = ${?n}").sql(),
        "SELECT x FROM t WHERE a = 1 UNION SELECT x FROM u"
    );
}

#[test]
fn order_by_after_a_multi_clause_template_survives() {
    let n: Option<i64> = None;
    assert_eq!(
        query!("SELECT x FROM t WHERE a = ${?n} UNION SELECT x FROM u WHERE b = ${?n} ORDER BY 1")
            .sql(),
        "SELECT x FROM t UNION SELECT x FROM u ORDER BY 1"
    );
}

// --- fragments mixed with optional predicates ---
//
// If there is even one `${?...}`, codegen routes *every* push — fragments
// included — through `Predicates`, so these paths differ from the ones
// without optionals.

#[test]
fn a_fragment_becomes_the_only_predicate_when_the_optional_drops() {
    let n: Option<i64> = None;
    assert_eq!(
        query!("SELECT * FROM t WHERE #{ACTIVE} AND a = ${?n}").sql(),
        "SELECT * FROM t WHERE deleted_at IS NULL"
    );
}

#[test]
fn a_fragment_after_a_dropped_optional_inherits_the_where() {
    let n: Option<i64> = None;
    assert_eq!(
        query!("SELECT * FROM t WHERE a = ${?n} AND #{ACTIVE}").sql(),
        "SELECT * FROM t WHERE deleted_at IS NULL"
    );
}

#[test]
fn a_trailing_clause_fragment_survives_a_dropped_optional() {
    const ORDER: SqlFragment = sql_fragment!("ORDER BY id");
    let n: Option<i64> = None;
    assert_eq!(
        query!("SELECT * FROM t WHERE a = ${?n} #{ORDER}").sql(),
        "SELECT * FROM t ORDER BY id"
    );
}

#[test]
fn a_fragment_between_two_optionals_keeps_its_joiner() {
    let s: Option<i64> = Some(1);
    let n: Option<i64> = None;
    assert_eq!(
        query!("SELECT * FROM t WHERE a = ${?s} AND #{ACTIVE} AND b = ${?n}").sql(),
        "SELECT * FROM t WHERE a = $1 AND deleted_at IS NULL"
    );
}

#[test]
fn a_fragment_opens_the_second_clause_when_the_first_drops() {
    let n: Option<i64> = None;
    let s: Option<i64> = Some(1);
    assert_eq!(
        query!(
            "SELECT x FROM t WHERE a = ${?n} \
             UNION SELECT x FROM u WHERE #{ACTIVE} AND b = ${?s}"
        )
        .sql(),
        "SELECT x FROM t UNION SELECT x FROM u WHERE deleted_at IS NULL AND b = $1"
    );
}

#[test]
fn a_fragment_opens_a_having_clause_while_where_vanishes() {
    let n: Option<i64> = None;
    let s: Option<i64> = Some(1);
    assert_eq!(
        query!(
            "SELECT k FROM t WHERE a = ${?n} \
             GROUP BY k HAVING #{ACTIVE} AND count(*) > ${?s}"
        )
        .sql(),
        "SELECT k FROM t GROUP BY k HAVING deleted_at IS NULL AND count(*) > $1"
    );
}

// --- the template is scanned as SQL, but the markers contain Rust ---

#[test]
fn an_apostrophe_inside_a_marker_does_not_open_a_sql_literal() {
    // Regression: a Rust lifetime inside a marker opened a phantom string
    // literal that ran to the end of the template and blanked out every later
    // clause keyword. Both markers then shared one clause, and the second
    // `WHERE` became a dangling `AND`.
    fn pick(_s: &str) -> Option<i32> {
        None
    }
    let y: Option<i32> = None;
    let z: Option<i32> = Some(8);
    assert_eq!(
        query!(
            "SELECT id FROM t WHERE a = ${?pick(<&'static str>::default())} \
             UNION SELECT id FROM u WHERE b = ${?y} AND c = ${?z}"
        )
        .sql(),
        "SELECT id FROM t UNION SELECT id FROM u WHERE c = $1"
    );
}

#[test]
fn a_char_literal_inside_a_marker_is_not_a_sql_quote() {
    let v: Option<i32> = Some(1);
    let w: Option<i32> = Some(2);
    assert_eq!(
        query!(
            "SELECT id FROM t WHERE a = ${?\"x'y\".find('\\'').map(|n| n as i32)} \
             UNION SELECT id FROM u WHERE b = ${?v} AND c = ${?w}"
        )
        .sql(),
        "SELECT id FROM t WHERE a = $1 UNION SELECT id FROM u WHERE b = $2 AND c = $3"
    );
}

// --- SQL literal forms the scanner must understand ---

#[test]
fn a_backslash_escaped_quote_does_not_end_a_literal() {
    // Regression: `E'it\'s OR zzz'` was cut at `\'`, the `OR` inside the
    // literal was taken as a joiner, and the SQL broke off mid-string.
    let p: Option<i32> = None;
    assert_eq!(
        query!(r"SELECT * FROM t WHERE a = E'it\'s OR zzz' AND b = ${?p}").sql(),
        r"SELECT * FROM t WHERE a = E'it\'s OR zzz'"
    );
    let p: Option<i32> = Some(1);
    assert_eq!(
        query!(r"SELECT * FROM t WHERE a = E'it\'s OR zzz' AND b = ${?p}").sql(),
        r"SELECT * FROM t WHERE a = E'it\'s OR zzz' AND b = $1"
    );
}

#[test]
fn a_doubled_quote_stays_inside_the_literal() {
    let p: Option<i32> = None;
    assert_eq!(
        query!("SELECT * FROM t WHERE a = 'it''s OR zzz' AND b = ${?p}").sql(),
        "SELECT * FROM t WHERE a = 'it''s OR zzz'"
    );
}

#[test]
fn comment_like_text_inside_a_literal_after_the_marker_is_data() {
    // Regression: the whole-template comment check must not reject `'--'`; it
    // is data, not a comment, even after a `${?..}` marker.
    let p = Some(1i32);
    assert_eq!(
        query!("SELECT * FROM t WHERE a = ${?p} AND note = '--'").sql(),
        "SELECT * FROM t WHERE a = $1 AND note = '--'"
    );
}

#[test]
fn a_dollar_quoted_string_hides_its_contents() {
    // Regression: a `(` inside `$$ ... $$` raised the paren depth forever, so
    // no clause boundary was seen after it and all markers collapsed into a
    // single clause.
    let p: Option<i32> = Some(1);
    let q: Option<i32> = None;
    let r: Option<i32> = Some(3);
    assert_eq!(
        query!(
            "SELECT $$ ( $$ AS s FROM t WHERE a = ${?p} \
             UNION SELECT 'x' AS s FROM u WHERE b = ${?q} AND c = ${?r}"
        )
        .sql(),
        "SELECT $$ ( $$ AS s FROM t WHERE a = $1 UNION SELECT 'x' AS s FROM u WHERE c = $2"
    );
}

#[test]
fn a_tagged_dollar_quoted_string_hides_its_contents() {
    let p: Option<i32> = None;
    assert_eq!(
        query!("SELECT $tag$ OR ( $tag$ AS s FROM t WHERE a = ${?p}").sql(),
        "SELECT $tag$ OR ( $tag$ AS s FROM t"
    );
}

#[test]
fn a_positional_parameter_is_not_a_dollar_quote_tag() {
    // `$1` must not be read as an opening `$tag$`, otherwise everything after
    // it disappears.
    let p: Option<i32> = Some(1);
    let q = query!("SELECT * FROM t WHERE a = ${?p} AND b IS NULL");
    assert_eq!(q.sql(), "SELECT * FROM t WHERE a = $1 AND b IS NULL");
}

// --- escapes must not be scattered by a predicate tail ---

#[test]
fn an_escape_after_a_predicate_is_expanded_normally() {
    // Regression: the tail's `find("${")` matched inside `$${`, stealing its
    // leading `$`, and the rest was re-read as a live marker — yielding
    // `'$$2'`, which Postgres accepts with silently wrong contents.
    let x: Option<&str> = Some("v");
    assert_eq!(
        query!("SELECT * FROM t WHERE a = ${?x} AND b = '$${z}'").sql(),
        "SELECT * FROM t WHERE a = $1 AND b = '${z}'"
    );
    let x: Option<&str> = None;
    assert_eq!(
        query!("SELECT * FROM t WHERE a = ${?x} AND b = '$${z}'").sql(),
        "SELECT * FROM t WHERE b = '${z}'"
    );
}

#[test]
fn an_escape_renders_the_same_with_and_without_an_optional_predicate() {
    let x: Option<i32> = Some(1);
    let with = query!("SELECT * FROM t WHERE a = ${?x} AND b = '$${z}'").sql();
    let without = query!("SELECT * FROM t WHERE a = 1 AND b = '$${z}'").sql();
    assert!(with.ends_with("b = '${z}'"), "{with}");
    assert!(without.ends_with("b = '${z}'"), "{without}");
}

#[test]
fn an_escaped_marker_does_not_shift_the_clause_of_a_later_predicate() {
    // An escape is literal text, but its `$${` would be scanned as SQL while
    // building the clause map, putting the predicate in a clause before its
    // own `WHERE`.
    let y: Option<i32> = Some(1);
    assert_eq!(
        query!("SELECT '$${?x}' AS s FROM t WHERE a = ${?y}").sql(),
        "SELECT '${?x}' AS s FROM t WHERE a = $1"
    );
    let y: Option<i32> = None;
    assert_eq!(
        query!("SELECT '$${?x}' AS s FROM t WHERE a = ${?y}").sql(),
        "SELECT '${?x}' AS s FROM t"
    );
}

// --- the predicate tail is matched on SQL, never on literal data ---
//
// The tail scanner looks for `)`, `;`, a joiner or a clause keyword to find
// where the predicate ends. Every one of those can also appear *inside* a
// string literal, a quoted identifier or a dollar-quoted body, where it is
// data. Matching there cut the predicate mid-literal, leaving a dangling
// joiner and an unterminated quote.

#[test]
fn a_joiner_inside_a_literal_does_not_end_the_predicate() {
    // Worst case: `AND` is data here, but stopping at it left
    // `SELECT * FROM t WHERE '` — a dangling `WHERE` plus an open literal.
    let x: Option<i32> = None;
    assert_eq!(
        query!("SELECT * FROM t WHERE a = ${?x} || ' AND '").sql(),
        "SELECT * FROM t"
    );
    let x = Some(1i32);
    assert_eq!(
        query!("SELECT * FROM t WHERE a = ${?x} || ' AND '").sql(),
        "SELECT * FROM t WHERE a = $1 || ' AND '"
    );
}

#[test]
fn a_joiner_inside_a_literal_does_not_strand_a_later_predicate() {
    // The sharpest form: the later bind survives and is dispatched, so a
    // mis-cut tail loses predicate `a` while `$1` still binds.
    let a: Option<i32> = None;
    let b = Some(2i32);
    assert_eq!(
        query!("SELECT * FROM t WHERE a = ${?a} || ' AND ' AND b = ${?b}").sql(),
        "SELECT * FROM t WHERE b = $1"
    );
}

#[test]
fn a_bracket_or_separator_inside_a_literal_does_not_end_the_predicate() {
    let x: Option<i32> = None;
    assert_eq!(
        query!("SELECT * FROM t WHERE a = ${?x} || ')'").sql(),
        "SELECT * FROM t"
    );
    assert_eq!(
        query!("SELECT * FROM t WHERE a = ${?x} || ';'").sql(),
        "SELECT * FROM t"
    );
    // Same token, dollar-quoted body.
    assert_eq!(
        query!("SELECT * FROM t WHERE a = ${?x} || $q$)$q$").sql(),
        "SELECT * FROM t"
    );
    let x = Some(1i32);
    assert_eq!(
        query!("SELECT * FROM t WHERE a = ${?x} || ')'").sql(),
        "SELECT * FROM t WHERE a = $1 || ')'"
    );
}

#[test]
fn a_clause_keyword_inside_a_literal_does_not_end_the_predicate() {
    let x: Option<i32> = None;
    assert_eq!(
        query!("SELECT * FROM t WHERE a = ${?x} || 'LIMIT'").sql(),
        "SELECT * FROM t"
    );
    let x = Some(1i32);
    assert_eq!(
        query!("SELECT * FROM t WHERE a = ${?x} || 'LIMIT'").sql(),
        "SELECT * FROM t WHERE a = $1 || 'LIMIT'"
    );
}

#[test]
fn a_real_bracket_still_ends_the_predicate() {
    // The literal-aware scan must not lose the genuine cases: this `)` closes
    // a subquery the marker sits inside.
    let x: Option<i32> = None;
    assert_eq!(
        query!("SELECT * FROM t WHERE id IN (SELECT id FROM u WHERE a = ${?x})").sql(),
        "SELECT * FROM t WHERE id IN (SELECT id FROM u)"
    );
    let x = Some(1i32);
    assert_eq!(
        query!("SELECT * FROM t WHERE id IN (SELECT id FROM u WHERE a = ${?x})").sql(),
        "SELECT * FROM t WHERE id IN (SELECT id FROM u WHERE a = $1)"
    );
}

// --- trailing top-level clauses are mandatory SQL, not predicate tail ---

#[test]
fn a_locking_clause_survives_a_dropped_predicate() {
    // `FOR UPDATE` was swallowed with the predicate: the lock silently
    // disappeared, which changes what the statement does rather than
    // breaking it.
    let x: Option<i32> = None;
    assert_eq!(
        query!("SELECT * FROM t WHERE a = ${?x} FOR UPDATE").sql(),
        "SELECT * FROM t FOR UPDATE"
    );
    let x = Some(1i32);
    assert_eq!(
        query!("SELECT * FROM t WHERE a = ${?x} FOR UPDATE").sql(),
        "SELECT * FROM t WHERE a = $1 FOR UPDATE"
    );
}

#[test]
fn a_locking_clause_survives_in_lower_case_too() {
    let x: Option<i32> = None;
    assert_eq!(
        query!("select * from t where a = ${?x} for update").sql(),
        "select * from t for update"
    );
}

#[test]
fn an_on_conflict_clause_survives_a_dropped_predicate() {
    let x: Option<i32> = None;
    assert_eq!(
        query!("UPDATE t SET x = 1 WHERE y = ${?x} ON CONFLICT DO NOTHING").sql(),
        "UPDATE t SET x = 1 ON CONFLICT DO NOTHING"
    );
}

#[test]
fn a_window_clause_survives_a_dropped_predicate() {
    let x: Option<i32> = None;
    assert_eq!(
        query!("SELECT * FROM t WHERE a = ${?x} WINDOW w AS (PARTITION BY b)").sql(),
        "SELECT * FROM t WINDOW w AS (PARTITION BY b)"
    );
    let x = Some(1i32);
    assert_eq!(
        query!("SELECT * FROM t WHERE a = ${?x} WINDOW w AS (PARTITION BY b)").sql(),
        "SELECT * FROM t WHERE a = $1 WINDOW w AS (PARTITION BY b)"
    );
}

// --- CTE fragments ---
//
// Two shapes, and the split between them is the whole point: a CTE can be the
// fragment (`#{CTE} SELECT ...`) or the fragment can be its body
// (`WITH t AS (#{body}) ...`). The second puts a top-level `UNION ALL` inside the
// fragment, which is legal precisely because the template wraps it in brackets —
// how deep a fragment lands is a property of the template, not the fragment.
//
// Note what these do *not* prove: a fragment's text never reaches `clause_map`,
// so they cannot distinguish a correct clause map from a broken one. They pin the
// emitted SQL for shapes that must keep working.

const CTE: SqlFragment =
    sql_fragment!("WITH active AS (SELECT id FROM u WHERE deleted_at IS NULL)");

#[test]
fn a_cte_fragment_heads_the_statement_and_leaves_the_predicate_alone() {
    let x = Some(1i32);
    assert_eq!(
        query!("#{CTE} SELECT * FROM t WHERE a = ${?x}").sql(),
        "WITH active AS (SELECT id FROM u WHERE deleted_at IS NULL) \
         SELECT * FROM t WHERE a = $1"
    );
    let x: Option<i32> = None;
    assert_eq!(
        query!("#{CTE} SELECT * FROM t WHERE a = ${?x}").sql(),
        "WITH active AS (SELECT id FROM u WHERE deleted_at IS NULL) SELECT * FROM t"
    );
}

#[test]
fn a_cte_bodys_where_does_not_open_a_clause_for_the_outer_predicates() {
    // If the CTE's own `WHERE` were counted, the first optional would land in a
    // later clause than the `WHERE` that introduces it, and `b` would join with
    // a dangling `AND` once `a` dropped.
    let a: Option<i32> = None;
    let b = Some(2i32);
    assert_eq!(
        query!("#{CTE} SELECT * FROM t WHERE a = ${?a} AND b = ${?b}").sql(),
        "WITH active AS (SELECT id FROM u WHERE deleted_at IS NULL) \
         SELECT * FROM t WHERE b = $1"
    );
}

#[test]
fn a_recursive_cte_fragment_keeps_its_union_nested() {
    const TREE: SqlFragment = sql_fragment!(
        "WITH RECURSIVE tree AS (\
             SELECT id FROM t WHERE parent IS NULL \
             UNION ALL \
             SELECT c.id FROM t c JOIN tree ON c.parent = tree.id\
         )"
    );
    let x = Some(1i32);
    assert_eq!(
        query!("#{TREE} SELECT * FROM tree WHERE id = ${?x}").sql(),
        "WITH RECURSIVE tree AS (SELECT id FROM t WHERE parent IS NULL \
         UNION ALL SELECT c.id FROM t c JOIN tree ON c.parent = tree.id) \
         SELECT * FROM tree WHERE id = $1"
    );
    let x: Option<i32> = None;
    assert_eq!(
        query!("#{TREE} SELECT * FROM tree WHERE id = ${?x}").sql(),
        "WITH RECURSIVE tree AS (SELECT id FROM t WHERE parent IS NULL \
         UNION ALL SELECT c.id FROM t c JOIN tree ON c.parent = tree.id) \
         SELECT * FROM tree"
    );
}

#[test]
fn a_cte_body_with_its_own_having_leaves_the_outer_having_separate() {
    const COUNTS: SqlFragment = sql_fragment!(
        "WITH counts AS (SELECT k, count(*) n FROM t GROUP BY k HAVING count(*) > 1)"
    );
    // The nested `HAVING` must not be mistaken for the statement's own, or the
    // outer `HAVING` predicate would be bookkept against the CTE's clause.
    let n: Option<i64> = Some(5);
    assert_eq!(
        query!("#{COUNTS} SELECT k FROM counts GROUP BY k HAVING sum(n) > ${?n}").sql(),
        "WITH counts AS (SELECT k, count(*) n FROM t GROUP BY k HAVING count(*) > 1) \
         SELECT k FROM counts GROUP BY k HAVING sum(n) > $1"
    );
    let n: Option<i64> = None;
    assert_eq!(
        query!("#{COUNTS} SELECT k FROM counts GROUP BY k HAVING sum(n) > ${?n}").sql(),
        "WITH counts AS (SELECT k, count(*) n FROM t GROUP BY k HAVING count(*) > 1) \
         SELECT k FROM counts GROUP BY k"
    );
}

#[test]
fn a_cte_fragment_composes_with_a_predicate_fragment() {
    let x: Option<i32> = None;
    assert_eq!(
        query!("#{CTE} SELECT * FROM t WHERE a = ${?x} AND #{ACTIVE}").sql(),
        "WITH active AS (SELECT id FROM u WHERE deleted_at IS NULL) \
         SELECT * FROM t WHERE deleted_at IS NULL"
    );
}

#[test]
fn a_cte_body_can_itself_be_the_fragment() {
    // The reusable half is the body; the template supplies `WITH .. AS (..)`.
    // Its `UNION ALL` is top-level *within the fragment*, so a check that
    // rejected boundaries outright would reject this valid composition.
    const BODY: SqlFragment = sql_fragment!(
        "SELECT id FROM t WHERE parent IS NULL \
         UNION ALL \
         SELECT c.id FROM t c JOIN tree ON c.parent = tree.id"
    );
    let x = Some(1i32);
    assert_eq!(
        query!("WITH RECURSIVE tree AS (#{BODY}) SELECT * FROM tree WHERE id = ${?x}").sql(),
        "WITH RECURSIVE tree AS (SELECT id FROM t WHERE parent IS NULL \
         UNION ALL SELECT c.id FROM t c JOIN tree ON c.parent = tree.id) \
         SELECT * FROM tree WHERE id = $1"
    );
    let x: Option<i32> = None;
    assert_eq!(
        query!("WITH RECURSIVE tree AS (#{BODY}) SELECT * FROM tree WHERE id = ${?x}").sql(),
        "WITH RECURSIVE tree AS (SELECT id FROM t WHERE parent IS NULL \
         UNION ALL SELECT c.id FROM t c JOIN tree ON c.parent = tree.id) \
         SELECT * FROM tree"
    );
}

#[test]
fn a_subquery_body_fragment_keeps_the_outer_predicates_intact() {
    // Same shape, one level down: the fragment is an `IN (...)` body.
    const IDS: SqlFragment =
        sql_fragment!("SELECT id FROM u WHERE active UNION SELECT id FROM v");
    let a: Option<i32> = None;
    let b = Some(2i32);
    assert_eq!(
        query!("SELECT * FROM t WHERE id IN (#{IDS}) AND a = ${?a} AND b = ${?b}").sql(),
        "SELECT * FROM t WHERE id IN (SELECT id FROM u WHERE active \
         UNION SELECT id FROM v) AND b = $1"
    );
}

// --- the documented limit: a fragment that opens a top-level clause ---
//
// These tests pin the *broken* output, not the desired one. They exist so the
// limit is demonstrable rather than folk knowledge, and so that a future fix
// announces itself by failing here.
//
// `SqlFragment::new` is used deliberately: `sql_fragment!` cannot reject this
// (how deep a fragment lands is up to the template, so a `UNION` inside one is
// legal — see `a_cte_body_can_itself_be_the_fragment`), and `new` is the
// documented escape hatch.

#[test]
fn a_fragment_opening_a_top_level_clause_breaks_the_joiner() {
    // The `UNION` inside the fragment starts a second select. The template
    // scanner never saw it, so `q` is bookkept against the first select's
    // clause, which the mandatory `WHERE` already opened -> it emits the
    // written `AND`. The second select needs its own `WHERE`.
    // A plausible mistake: the fragment carries a predicate *and* the query's
    // shape, instead of just the predicate.
    const BAD: SqlFragment =
        SqlFragment::new("deleted_at IS NULL UNION SELECT x FROM u");
    let p: Option<i32> = None;
    let q = Some(2i32);
    assert_eq!(
        query!("SELECT x FROM t WHERE a = ${?p} AND #{BAD} AND b = ${?q}").sql(),
        // Invalid SQL: PostgreSQL rejects `FROM u AND b = $1`.
        "SELECT x FROM t WHERE deleted_at IS NULL UNION SELECT x FROM u AND b = $1"
    );
}

#[test]
fn the_same_boundary_in_the_template_is_handled_correctly() {
    // The workaround, and why it is not a downgrade: the fragment keeps the
    // reusable predicate while the template owns the query's shape, so the same
    // predicate can be applied on *both* sides of the boundary. `q` then belongs
    // to the second select's clause and correctly supplies its `WHERE`.
    let p: Option<i32> = None;
    let q = Some(2i32);
    assert_eq!(
        query!(
            "SELECT x FROM t WHERE a = ${?p} AND #{ACTIVE} \
             UNION SELECT x FROM u WHERE #{ACTIVE} AND b = ${?q}"
        )
        .sql(),
        "SELECT x FROM t WHERE deleted_at IS NULL \
         UNION SELECT x FROM u WHERE deleted_at IS NULL AND b = $1"
    );
    // Both surviving: the first select's predicate keeps its own `AND`.
    let p = Some(1i32);
    assert_eq!(
        query!(
            "SELECT x FROM t WHERE a = ${?p} AND #{ACTIVE} \
             UNION SELECT x FROM u WHERE #{ACTIVE} AND b = ${?q}"
        )
        .sql(),
        "SELECT x FROM t WHERE a = $1 AND deleted_at IS NULL \
         UNION SELECT x FROM u WHERE deleted_at IS NULL AND b = $2"
    );
}

#[test]
fn a_fragment_boundary_is_harmless_without_optional_predicates() {
    // Nothing to bookkeep: with no `${?...}` in the template, every push is
    // unconditional and the fragment's own structure is the author's business.
    const BAD: SqlFragment =
        SqlFragment::new("deleted_at IS NULL UNION SELECT x FROM u");
    assert_eq!(
        query!("SELECT x FROM t WHERE #{BAD}").sql(),
        "SELECT x FROM t WHERE deleted_at IS NULL UNION SELECT x FROM u"
    );
}

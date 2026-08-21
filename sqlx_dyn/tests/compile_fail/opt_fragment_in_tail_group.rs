// A `#{...}` at the top level of a predicate's tail is accepted, because the
// fragment may be the clause boundary that ends the predicate — see
// `a_trailing_clause_fragment_survives_a_dropped_optional`. Nested in a group
// the tail opened, it cannot be that boundary: it is positionally inside the
// predicate whatever SQL it carries, and dropping the predicate emitted
// `SELECT * FROM t(1)`.
use sqlx_dyn::{query, sql_fragment, SqlFragment};

fn main() {
    const F: SqlFragment = sql_fragment!("1");
    let x: Option<i32> = None;
    query!("SELECT * FROM t WHERE a = ${?x} + f(#{F})");
}

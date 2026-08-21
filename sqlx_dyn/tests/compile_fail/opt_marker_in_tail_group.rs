// The same rule inside a group the tail opened itself. An unbalanced `(` is not
// a boundary: the group was opened after the marker, so `${y}` is nested within
// this predicate, and dropping it emitted `SELECT * FROM t($1)`.
//
// A `#{...}` in this position is rejected too, unlike one at the top level of
// the tail: a fragment there may *be* the clause boundary that ends the
// predicate, but one nested in a tail-opened group cannot be, whatever SQL it
// carries.
use sqlx_dyn::query;

fn main() {
    let x: Option<i32> = None;
    let y: i32 = 9;
    query!("SELECT * FROM t WHERE a = ${?x} + f(${y})");
}

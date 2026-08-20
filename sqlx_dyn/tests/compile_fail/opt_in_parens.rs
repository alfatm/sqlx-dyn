// A bracket between the joiner and the marker means the predicate is not the
// whole clause; removing it would leave unbalanced SQL.
use sqlx_dyn::query;

fn main() {
    let x: Option<i32> = None;
    query!("SELECT * FROM t WHERE (a = ${?x} OR b = 1)");
}

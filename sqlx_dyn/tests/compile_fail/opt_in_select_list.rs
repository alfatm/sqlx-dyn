// `${?...}` needs a predicate to live in; a SELECT list has no
// `WHERE`/`AND`/`OR` to fold into, so there is no way to know how much SQL to
// remove.
use sqlx_dyn::query;

fn main() {
    let extra: Option<i32> = None;
    query!("SELECT id, ${?extra} FROM t");
}

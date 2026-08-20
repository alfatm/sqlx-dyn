// A function argument is not a top-level predicate.
use sqlx_dyn::query;

fn main() {
    let d: Option<i32> = None;
    query!("SELECT * FROM t WHERE d >= make_interval(days => ${?d})");
}

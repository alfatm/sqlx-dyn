// `${?}` has no expression to evaluate.
use sqlx_dyn::query;

fn main() {
    query!("SELECT * FROM t WHERE a = ${?}");
}

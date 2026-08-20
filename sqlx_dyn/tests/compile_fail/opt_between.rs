// `BETWEEN` needs two operands, so removing either half would leave
// `WHERE age BETWEEN $1` or `WHERE $1`.
use sqlx_dyn::query;

fn main() {
    let lo: Option<i32> = None;
    let hi: Option<i32> = None;
    query!("SELECT * FROM t WHERE age BETWEEN ${?lo} AND ${?hi}");
}

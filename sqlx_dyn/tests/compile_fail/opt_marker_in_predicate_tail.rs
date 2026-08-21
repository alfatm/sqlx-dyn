// A predicate must be removable as one piece. Here the tail reaches `${y}`
// without passing a clause boundary, so the bind sits inside the same predicate:
// dropping the predicate on `None` left `SELECT * FROM t $1`, which the `Some`
// branch gives no hint of.
use sqlx_dyn::query;

fn main() {
    let x: Option<i32> = None;
    let y: i32 = 9;
    query!("SELECT * FROM t WHERE a = ${?x} || ${y}");
}

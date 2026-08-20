// The same danger with a `/* ... */` comment.
use sqlx_dyn::query;

fn main() {
    let x: Option<i32> = None;
    query!("SELECT * FROM t WHERE a = 1 /* or */ AND col = ${?x}");
}

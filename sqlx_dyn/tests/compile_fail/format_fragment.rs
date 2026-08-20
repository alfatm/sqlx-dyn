//! The result of `format!` must not be usable as raw SQL — that is the most
//! likely accidental injection path.
use sqlx_dyn::query;

fn main() {
    let table = "users";
    let _q = query!("SELECT * FROM t WHERE #{format!(\"{} = 1\", table)}");
}

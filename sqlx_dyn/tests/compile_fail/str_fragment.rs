//! `&str` must not be usable as raw SQL either.

use sqlx_dyn::query;

fn main() {
    let filter: &str = "1=1 OR true";
    let _q = query!("SELECT * FROM users WHERE #{filter}");
}

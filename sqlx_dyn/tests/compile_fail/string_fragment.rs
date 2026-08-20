//! `String` must not be usable as raw SQL: that is the injection guard itself.

use sqlx_dyn::query;

fn main() {
    let filter = String::from("1=1 OR true");
    let _q = query!("SELECT * FROM users WHERE #{filter}");
}

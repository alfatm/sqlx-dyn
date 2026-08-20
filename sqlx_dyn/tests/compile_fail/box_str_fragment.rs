//! `Box<str>` must not be usable as raw SQL.
use sqlx_dyn::query;

fn main() {
    let filter: Box<str> = String::from("1=1 OR true").into_boxed_str();
    let _q = query!("SELECT * FROM users WHERE #{filter}");
}

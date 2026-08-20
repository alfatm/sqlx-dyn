//! An arbitrary `Display` type must not be usable as raw SQL.
//! QueryBuilder::push takes an `impl Display`, so this checks that the crate
//! narrows it.
use sqlx_dyn::query;
use std::fmt;

struct Evil;
impl fmt::Display for Evil {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("1=1 OR true")
    }
}

fn main() {
    let _q = query!("SELECT * FROM users WHERE #{Evil}");
}

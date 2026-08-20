//! `${}` has no expression to bind.

use sqlx_dyn::query;

fn main() {
    let _q = query!("SELECT * FROM users WHERE id = ${}");
}

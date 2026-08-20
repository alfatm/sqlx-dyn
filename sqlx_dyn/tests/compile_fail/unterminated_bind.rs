//! A `${` with no closing brace is a template error.

use sqlx_dyn::query;

fn main() {
    let _q = query!("SELECT * FROM users WHERE id = ${id");
}

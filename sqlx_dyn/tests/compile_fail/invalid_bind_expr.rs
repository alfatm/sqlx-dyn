//! Garbage Rust inside `${...}` is reported by the macro, not by rustc.

use sqlx_dyn::query;

fn main() {
    let _q = query!("SELECT * FROM users WHERE id = ${foo +}");
}

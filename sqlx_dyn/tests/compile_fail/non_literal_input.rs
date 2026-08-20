//! The template must be a string literal, so it can never be built at runtime.

use sqlx_dyn::query;

fn main() {
    let some_variable = "SELECT 1";
    let _q = query!(some_variable);
}

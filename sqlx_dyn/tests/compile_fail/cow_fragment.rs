//! `Cow<str>` must not be usable as raw SQL, even though `SqlFragment` stores a
//! `Cow` internally.
use sqlx_dyn::query;
use std::borrow::Cow;

fn main() {
    let filter: Cow<'static, str> = Cow::Owned(String::from("1=1 OR true"));
    let _q = query!("SELECT * FROM users WHERE #{filter}");
}

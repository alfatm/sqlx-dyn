// `SqlFragmentLike` is sealed by a private supertrait. Without that seal, a
// newtype around `String` could opt itself in and defeat the whole `#{...}`
// guard.
use sqlx_dyn::{query, SqlFragmentLike};

struct Evil(String);

impl SqlFragmentLike for Evil {
    fn as_sql(&self) -> &str {
        &self.0
    }
}

fn main() {
    let attacker = Evil(String::from("1=1 OR 1=1 --"));
    query!("SELECT * FROM users WHERE #{attacker}");
}

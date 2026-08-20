//! Compile-fail tests: guarantees that exist only as type errors.
//!
//! Run with `TRYBUILD=overwrite` to regenerate the `.stderr` files.

#[test]
fn compile_fail() {
    let t = trybuild::TestCases::new();
    t.compile_fail("tests/compile_fail/*.rs");
}

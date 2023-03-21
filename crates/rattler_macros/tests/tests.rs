#[test]
fn tests() {
    let t = trybuild::TestCases::new();
    t.pass("tests/01-sorted-enum.rs");
    t.pass("tests/02-sorted-struct.rs");
    t.compile_fail("tests/03-out-of-order-enum.rs");
    t.compile_fail("tests/04-out-of-order-struct.rs");
}

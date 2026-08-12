#[test]
fn invalid_api_traits_are_rejected() {
    let tests = trybuild::TestCases::new();
    tests.compile_fail("tests/ui/*.rs");
}

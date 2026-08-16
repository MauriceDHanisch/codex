use super::v8_thread_pool_size_from_env;

#[test]
fn v8_thread_pool_size_defaults_to_platform_value() {
    assert_eq!(v8_thread_pool_size_from_env(None).expect("unset value"), 0);
}

#[test]
fn v8_thread_pool_size_accepts_positive_integer() {
    assert_eq!(
        v8_thread_pool_size_from_env(Some("8")).expect("positive value"),
        8
    );
}

#[test]
fn v8_thread_pool_size_rejects_zero_and_invalid_values() {
    assert!(v8_thread_pool_size_from_env(Some("0")).is_err());
    assert!(v8_thread_pool_size_from_env(Some("many")).is_err());
}

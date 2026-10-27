use sim_p2p_t2u::T2uNetStat;

#[test]
fn native_status_layout_matches_shipped_wrapper() {
    assert_eq!(std::mem::size_of::<T2uNetStat>(), 88);
    assert_eq!(std::mem::offset_of!(T2uNetStat, port), 20);
    assert_eq!(std::mem::offset_of!(T2uNetStat, ip6), 44);
    assert_eq!(std::mem::offset_of!(T2uNetStat, remote_version), 84);
}

#[test]
fn refuses_relative_sdk_path_before_loading_native_code() {
    assert!(
        sim_p2p_t2u::Client::load(
            std::path::Path::new("libt2u.dll"),
            "example.invalid",
            1250,
            "test"
        )
        .is_err()
    );
}

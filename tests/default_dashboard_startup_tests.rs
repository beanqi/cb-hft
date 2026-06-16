use cb_hft::runtime::RuntimeOptions;

#[test]
fn empty_cli_args_start_dashboard_mode() {
    let opts = RuntimeOptions::parse_args(std::iter::empty::<&str>()).unwrap();

    assert!(opts.dashboard);
    assert!(!opts.market_data);
    assert!(!opts.order_entry);
}

#[test]
fn dashboard_start_child_args_disable_dashboard_and_enable_fix_pipeline() {
    let opts = RuntimeOptions::parse_args(["--no-dashboard", "--with-order-entry"]).unwrap();

    assert!(!opts.dashboard);
    assert!(opts.market_data);
    assert!(opts.order_entry);
}

fn main() {
    if let Err(err) = cb_hft::runtime::run_from_env() {
        eprintln!("cb-hft error: {err}");
        std::process::exit(1);
    }
}

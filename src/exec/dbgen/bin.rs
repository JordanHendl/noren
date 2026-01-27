use noren::tools::dbgen;

fn main() {
    if dbgen::run_cli().is_err() {
        std::process::exit(1);
    }
}

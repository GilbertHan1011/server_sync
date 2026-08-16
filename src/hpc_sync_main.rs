mod hpc_sync;

fn main() {
    std::process::exit(hpc_sync::cli::run());
}

#[cfg(unix)]
fn main() -> std::process::ExitCode {
    // The cargo shim is a symlink to this binary.
    if ferry::shim::invoked_as_cargo() {
        return ferry::shim::main();
    }
    ferry::cli::main()
}

#[cfg(not(unix))]
fn main() {}

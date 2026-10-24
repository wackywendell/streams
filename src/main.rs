mod cli;

#[cfg(test)]
#[path = "../tests/support/herdr.rs"]
mod fake_herdr;

fn main() -> std::process::ExitCode {
    cli::run()
}

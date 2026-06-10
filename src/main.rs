use std::process::ExitCode;

fn main() -> ExitCode {
    match gh_secrets::cli::run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(err) => {
            eprintln!("error: {err:#}");
            ExitCode::from(1)
        }
    }
}

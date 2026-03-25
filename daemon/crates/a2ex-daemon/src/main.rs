use std::process::ExitCode;

use a2ex_daemon::{DaemonConfig, init_tracing, run_until_shutdown_signal};

#[tokio::main]
async fn main() -> ExitCode {
    init_tracing();

    match DaemonConfig::from_env() {
        Ok(config) => match run_until_shutdown_signal(config).await {
            Ok(_) => ExitCode::SUCCESS,
            Err(error) => {
                eprintln!("{error}");
                ExitCode::FAILURE
            }
        },
        Err(error) => {
            eprintln!("{error}");
            ExitCode::FAILURE
        }
    }
}

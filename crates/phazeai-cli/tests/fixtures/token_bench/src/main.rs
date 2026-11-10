use std::process::ExitCode;

use tally::{config, log, server, store};

fn main() -> ExitCode {
    let config = match config::Config::from_env() {
        Ok(config) => config,
        Err(err) => {
            eprintln!("tally: {err}");
            return ExitCode::from(2);
        }
    };
    log::set_level(config.log_level);
    log::info(&format!(
        "starting on {}:{} (up to {} counters)",
        config.host, config.port, config.max_names
    ));

    let store = store::Store::new(config.max_names);
    match server::serve(&config, store) {
        Ok(()) => ExitCode::SUCCESS,
        Err(err) => {
            log::error(&format!("server stopped: {err}"));
            ExitCode::FAILURE
        }
    }
}

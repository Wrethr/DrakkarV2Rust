mod cli;
mod config;
mod build;
mod worker;
mod error;
mod depfile;
mod platform;

use std::process;

fn main() {
    let result = cli::run();
    match result {
        Ok(code) => process::exit(code),
        Err(e) => {
            eprintln!("\x1b[31merror:\x1b[0m {}", e);
            process::exit(1);
        }
    }
}

//! Binary entry point: collect args, dispatch, map errors to exit codes.

use std::process::ExitCode;

use git_rs::cli;
use git_rs::error::GitError;

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match cli::dispatch(&args) {
        Ok(()) => ExitCode::SUCCESS,
        Err(err) => {
            eprintln!("fatal: {err}");
            ExitCode::from(exit_code(&err))
        }
    }
}

/// 0 success, 1 generic failure (invalid input), 128 fatal.
fn exit_code(err: &GitError) -> u8 {
    match err {
        GitError::Invalid(_) => 1,
        GitError::NotFound(_) | GitError::Corrupt(_) | GitError::Io { .. } => 128,
    }
}

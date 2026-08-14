//! Binary entry point: collect args, dispatch, map errors to exit codes.

use std::process::ExitCode;

use git_rs::cli;
use git_rs::error::GitError;

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match cli::dispatch(&args) {
        Ok(()) => ExitCode::SUCCESS,
        Err(err) => {
            match &err {
                // Invalid/Failure errors print bare -- real git omits the
                // fatal: prefix for these (e.g. ignored-paths add error,
                // probed). An empty message is a sentinel for commands that
                // already printed their own output (commit's empty-commit
                // notices go to stdout, exit 1; merge conflict blocks go to
                // stdout/stderr, exit 1).
                GitError::Invalid(msg) if !msg.is_empty() => eprintln!("{err}"),
                GitError::Failure(msg) if !msg.is_empty() => eprintln!("{err}"),
                GitError::Fatal(msg) if !msg.is_empty() => eprintln!("fatal: {err}"),
                GitError::NotFound(msg) | GitError::Corrupt(msg) if !msg.is_empty() => {
                    eprintln!("fatal: {msg}")
                }
                GitError::Io { .. } => eprintln!("fatal: {err}"),
                _ => {}
            }
            ExitCode::from(exit_code(&err))
        }
    }
}

/// 0 success, 1 generic failure (invalid input), 2 merge's non-fatal
/// failure, 128 fatal.
fn exit_code(err: &GitError) -> u8 {
    match err {
        GitError::Invalid(_) => 1,
        GitError::Failure(_) => 2,
        GitError::Fatal(_) | GitError::NotFound(_) | GitError::Corrupt(_) | GitError::Io { .. } => {
            128
        }
    }
}

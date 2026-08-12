//! Hand-written CLI parsing and command dispatch.
//!
//! No `clap`, no derive macros: a static command table plus explicit
//! argument handling keeps behavior predictable and dependency-free.

use crate::commands::hash_object::{run_cat_file, run_hash_object};
use crate::error::{GitError, Result};

/// A registered subcommand.
pub struct Command {
    /// The command name as typed on the command line.
    pub name: &'static str,
    /// Usage line shown in help.
    pub usage: &'static str,
    /// One-line description shown in the command list.
    pub help: &'static str,
    /// Command implementation. Receives args without the command name.
    pub run: fn(&[String]) -> Result<()>,
}

/// Command table. Commands are registered here as they are implemented.
pub static COMMANDS: &[Command] = &[
    Command {
        name: "help",
        usage: "git-rs help [<command>]",
        help: "show help for git-rs or a specific command",
        run: run_help,
    },
    Command {
        name: "hash-object",
        usage: "git-rs hash-object [-w] [--stdin] <file>",
        help: "compute the object id for a file, optionally writing it",
        run: run_hash_object,
    },
    Command {
        name: "cat-file",
        usage: "git-rs cat-file (-t | -s | -p) <object>",
        help: "print the type, size, or content of an object",
        run: run_cat_file,
    },
];

/// Dispatch raw arguments (without the program name) to a command.
pub fn dispatch(args: &[String]) -> Result<()> {
    let Some((name, rest)) = args.split_first() else {
        print_usage();
        return Ok(());
    };
    match name.as_str() {
        "--help" | "-h" => run_help(rest),
        "--version" | "-V" => {
            println!("git-rs {}", env!("CARGO_PKG_VERSION"));
            Ok(())
        }
        name => match COMMANDS.iter().find(|cmd| cmd.name == name) {
            Some(cmd) => (cmd.run)(rest),
            None => Err(GitError::Invalid(format!("unknown command: {name}"))),
        },
    }
}

fn run_help(args: &[String]) -> Result<()> {
    match args.first() {
        Some(name) => match COMMANDS.iter().find(|cmd| cmd.name == name) {
            Some(cmd) => {
                println!("usage: {}", cmd.usage);
                println!();
                println!("{}", cmd.help);
                Ok(())
            }
            None => Err(GitError::Invalid(format!("unknown command: {name}"))),
        },
        None => {
            print_usage();
            Ok(())
        }
    }
}

/// Print the usage listing for every registered command.
pub fn print_usage() {
    println!("usage: git-rs <command> [<args>]");
    println!();
    println!("commands:");
    for cmd in COMMANDS {
        println!("   {:<22} {}", cmd.name, cmd.help);
    }
}

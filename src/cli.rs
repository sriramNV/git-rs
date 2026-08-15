//! Hand-written CLI parsing and command dispatch.
//!
//! No `clap`, no derive macros: a static command table plus explicit
//! argument handling keeps behavior predictable and dependency-free.

use crate::commands::add::run_add;
use crate::commands::branch::run_branch;
use crate::commands::checkout::run_checkout;
use crate::commands::commit::run_commit;
use crate::commands::diff::run_diff;
use crate::commands::hash_object::{run_cat_file, run_hash_object, run_update_ref};
use crate::commands::log::run_log;
use crate::commands::merge::{run_merge, run_merge_base};
use crate::commands::rebase::run_rebase;
use crate::commands::reset::run_reset;
use crate::commands::stash::run_stash;
use crate::commands::show::run_show;
use crate::commands::status::run_status;
use crate::commands::tag::run_tag;
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
    Command {
        name: "update-ref",
        usage: "git-rs update-ref [-m <reason>] <ref> <new> [<old>]",
        help: "update a ref atomically, with an optional expected old value",
        run: run_update_ref,
    },
    Command {
        name: "add",
        usage: "git-rs add <pathspec>...",
        help: "stage worktree changes into the index",
        run: run_add,
    },
    Command {
        name: "status",
        usage: "git-rs status [--short]",
        help: "show the working tree status (short format)",
        run: run_status,
    },
    Command {
        name: "diff",
        usage: "git-rs diff [--cached|--staged] [-- <paths>]",
        help: "show changes between the worktree, index, and HEAD",
        run: run_diff,
    },
    Command {
        name: "commit",
        usage: "git-rs commit -m <msg> [-m <msg>] [-a]",
        help: "record changes into the repository",
        run: run_commit,
    },
    Command {
        name: "log",
        usage: "git-rs log [--oneline] [-n <k>] [--all] [--graph]",
        help: "show commit history (oneline format)",
        run: run_log,
    },
    Command {
        name: "show",
        usage: "git-rs show [<rev>]",
        help: "show a commit's header and change summary",
        run: run_show,
    },
    Command {
        name: "branch",
        usage: "git-rs branch [<name>] [-a] [-d] [-D]",
        help: "list, create, or delete branches",
        run: run_branch,
    },
    Command {
        name: "tag",
        usage: "git-rs tag [<name>] [-a] [-m <msg>] [-l] [-d]",
        help: "create, list, or delete tags",
        run: run_tag,
    },
    Command {
        name: "checkout",
        usage: "git-rs checkout [-b <name>] [-f] [-q] <branch|tag|sha>",
        help: "switch branches or restore working tree files",
        run: run_checkout,
    },
    Command {
        name: "reset",
        usage: "git-rs reset [--soft|--mixed|--hard] [<commit>]",
        help: "reset current HEAD to the specified state",
        run: run_reset,
    },
    Command {
        name: "merge",
        usage: "git-rs merge [--abort] [-q] <branch|tag|sha>",
        help: "merge another commit into the current branch",
        run: run_merge,
    },
    Command {
        name: "merge-base",
        usage: "git-rs merge-base <rev1> <rev2>",
        help: "find the common ancestor of two revisions",
        run: run_merge_base,
    },
    Command {
        name: "rebase",
        usage: "git-rs rebase <upstream> | --continue | --abort | --skip",
        help: "replay commits onto another branch",
        run: run_rebase,
    },
    Command {
        name: "stash",
        usage: "git-rs stash [list|pop|drop] [<stash>]",
        help: "stash the changes in a dirty working directory",
        run: run_stash,
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

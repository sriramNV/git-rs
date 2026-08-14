//! Command implementations. One file per command group; thin wrappers over
//! the state and algorithm modules.

pub mod add;
pub mod branch;
pub mod checkout;
pub mod commit;
pub mod diff;
pub mod hash_object;
pub mod log;
pub mod reset;
pub mod show;
pub mod status;
pub mod tag;

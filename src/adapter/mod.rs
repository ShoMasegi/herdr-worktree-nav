//! Implementations of the ports against the real world.
//!
//! This is the only module allowed to touch a process or a socket. `scripts/check-invariants.sh`
//! enforces that.

pub mod gh_cli;
pub mod git_cli;
pub mod herdr_socket;

pub use gh_cli::GhCli;
pub use git_cli::GitCli;
pub use herdr_socket::SocketHerdr;

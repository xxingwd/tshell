//! Shared host IO without a dependency on a UI or terminal implementation.
mod identity;
pub use identity::HostId;
pub mod config;
pub mod diagnostic;
pub mod files;
pub mod forwarding;
pub mod sftp;
pub mod ssh;
pub use config::SshConfig;
pub use diagnostic::{Diagnostic, Failure};

pub mod command;
pub mod path;
pub use command::{CommandRequest, Host};
pub use path::{HostPath, PosixPath};

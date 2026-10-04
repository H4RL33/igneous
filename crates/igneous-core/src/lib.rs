//! The parts of Igneous that touch the filesystem: vault-relative paths, ignore
//! rules, byte-faithful text files, guarded writes, watching, and the settings
//! stored in `<vault>/.igneous/`.
//!
//! Nothing in this crate depends on GTK.

#![forbid(unsafe_code)]

pub mod datefmt;
pub mod fs;
pub mod ignore;
pub mod path;
pub mod settings;
pub mod text;
pub mod vault;
pub mod watch;

pub use path::VaultPath;
pub use text::TextFile;
pub use vault::Vault;

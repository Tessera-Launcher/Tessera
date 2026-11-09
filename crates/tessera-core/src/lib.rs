//! tessera's core: everything that does not need a terminal.
//!
//! Discovery, launch-argument assembly, downloads, Java runtimes, accounts,
//! tokens and cleanup live here. The terminal front-end (`tessera-tui`) is one
//! consumer; a future GUI is another.

pub mod accounts;
pub mod aot;
pub mod app_ids;
pub mod cleanup;
pub mod discover;
pub mod fastmenu;
pub mod install;
pub mod java;
pub mod launch;
pub mod logfile;
pub mod login;
pub mod paths;
pub mod session;
pub mod settings;
pub mod version_tree;

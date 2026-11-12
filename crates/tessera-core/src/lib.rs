//! tessera's core: every piece of logic that is not tied to a window.
//!
//! Discovery, launch-argument assembly, downloads, Java runtimes, accounts,
//! tokens and cleanup live here. The desktop front-end (`tessera-gui`) is the
//! only consumer.

pub mod accounts;
pub mod aot;
pub mod app;
pub mod app_ids;
pub mod audio;
pub mod cleanup;
pub mod discover;
pub mod error;
pub mod fastmenu;
pub mod fsutil;
pub mod http;
pub mod input;
pub mod instance;
pub mod install;
pub mod java;
pub mod launch;
pub mod logfile;
pub mod login;
pub mod music;
pub mod paths;
pub mod session;
pub mod settings;
pub mod task;
pub mod version;
pub mod version_tree;

//! One file per full-window screen. The state each screen draws lives in
//! `tessera_core::app`; these modules only translate it into GPUI elements.

pub(crate) mod accounts;
pub(crate) mod download;
pub(crate) mod home;
pub(crate) mod java;
pub(crate) mod login;

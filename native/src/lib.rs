//! Shared library for the native Linux x86_64 targets.
//!
//! Backs both the `native` binary (boot menu, storage scan, Lua shell) and the
//! `lua` binary (standalone Lua shell / `.lua` script runner). The kernel
//! handles networking here: [`net`] discovers routes and runs a std UDP TFTP
//! client, while [`fetch`] holds the `dhcp` / `fetch()` host callbacks and
//! [`term`] provides raw-terminal input with signal-safe cleanup.

pub mod cli;
pub mod fetch;
pub mod net;
pub mod term;

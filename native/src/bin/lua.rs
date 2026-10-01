//! Standalone Lua shell / script runner (`bin/lua`).
//!
//! Built as part of the `native` package; the implementation lives in
//! [`native::cli`].
//!
//! Usage:
//! - `lua` — interactive Lua shell (no boot menu)
//! - `lua path/to/script.lua` — run the script and exit

fn main() {
    native::cli::run_lua_cli();
}

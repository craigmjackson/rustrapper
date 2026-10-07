//! Standalone Lua shell / script runner (the `bin/lua` binary).
//!
//! With a `.lua` file argument the script is run through the built-in
//! interpreter and the process exits with status 0 on success or 1 on error.
//! With no argument the interactive Lua shell starts directly — no boot menu.

use common::print;

use crate::{fetch, net, term};

/// Entry point for the `lua` binary: run the given script file, or start the
/// interactive shell when no argument is given.
pub fn run_lua_cli() -> ! {
    match std::env::args().nth(1) {
        Some(path) => run_script(&path),
        None => run_repl(),
    }
}

/// Run a `.lua` file and exit. The terminal is left in its normal (cooked)
/// mode, so output is written newline-for-newline and can be redirected.
fn run_script(path: &str) -> ! {
    let src = match std::fs::read(path) {
        Ok(src) => src,
        Err(e) => {
            eprintln!("lua: cannot read {}: {}", path, e);
            std::process::exit(1);
        }
    };
    print::init(term::raw_putc);
    let mut state = lua::LuaState::new();
    state.set_dhcp(Some(net::dhcp_fn));
    state.set_dhcp_info(Some(fetch::dhcp_info));
    state.set_dhcp_values(Some(fetch::dhcp_values));
    match state.run_with_fetch_load(&src, term::raw_putc, fetch::fetch_file, fetch::load_file) {
        Ok(()) => std::process::exit(0),
        Err(e) => {
            let mut buf = [0u8; 256];
            let n = lua::eval::error_bytes(&state, e, &mut buf);
            let msg = std::str::from_utf8(&buf[..n]).unwrap_or("error");
            eprintln!("lua: {}: {}", path, msg);
            std::process::exit(1);
        }
    }
}

/// Start the interactive Lua shell and exit when the user types `exit` (or
/// Ctrl-D). Networking starts disabled, like every other target: run `dhcp`
/// first to enable `fetch()` / `dofile()`.
fn run_repl() -> ! {
    term::init();
    print::puts("Rustrapper Lua\n");
    let mut state = lua::LuaState::new();
    state.register_builtins(print::putc);
    state.set_fetch(None);
    state.set_dhcp(Some(net::dhcp_fn));
    state.set_load(Some(fetch::load_file));
    state.set_dhcp_info(Some(fetch::dhcp_info));
    state.set_dhcp_values(Some(fetch::dhcp_values));
    lua::repl::repl_loop(&mut state, term::get_key, print::putc, print::puts);
    term::shutdown(0)
}

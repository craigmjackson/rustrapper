#[test]
fn print_lua_state_size() {
    eprintln!("LuaState size = {} bytes", core::mem::size_of::<crate::LuaState>());
}

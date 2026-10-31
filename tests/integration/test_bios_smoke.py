from conftest import spawn, boot_and_select, cleanup


def test_bios_lua_shell_smoke():
    child = spawn("i386-bios")
    try:
        boot_and_select(child, "3", timeout=120)
        child.expect_exact("Lua Shell", timeout=30)
        child.sendline('print(1 + 2)')
        child.expect_exact("3", timeout=10)
        child.sendline("exit")
        child.expect_exact("Choose:", timeout=10)
    finally:
        cleanup(child)

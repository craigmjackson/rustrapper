import os
import pytest

from conftest import spawn, boot_and_select, cleanup

QEMU_AARCH64 = "/usr/bin/qemu-system-aarch64"


@pytest.mark.skipif(not os.path.exists(QEMU_AARCH64), reason="qemu-system-aarch64 not installed")
def test_arm64_bare_lua_shell_smoke():
    child = spawn("aarch64-bare")
    try:
        boot_and_select(child, "3", timeout=120)
        child.expect_exact("Lua Shell", timeout=30)
        child.sendline('print(1 + 2)')
        child.expect_exact("3", timeout=10)
        child.sendline("exit")
    finally:
        cleanup(child)

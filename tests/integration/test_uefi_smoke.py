import os
import pytest

from conftest import spawn, cleanup

OVMF = "/usr/share/edk2/x64/OVMF.4m.fd"


@pytest.mark.skipif(not os.path.exists(OVMF), reason="OVMF not installed")
def test_uefi_lua_shell_smoke():
    child = spawn("x86_64-uefi")
    try:
        child.expect_exact("Choose:", timeout=120)
        child.sendline("3")
        child.expect_exact("Lua Shell", timeout=30)
        child.sendline('print(1 + 2)')
        child.expect_exact("3", timeout=10)
        child.sendline("exit")
    finally:
        cleanup(child)

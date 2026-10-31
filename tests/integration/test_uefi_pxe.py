import os
import pytest

from conftest import spawn, cleanup

OVMF = "/usr/share/edk2/x64/OVMF.4m.fd"


@pytest.mark.skipif(not os.path.exists(OVMF), reason="OVMF not installed")
def test_uefi_pxe_end_to_end():
    child = spawn("x86_64-uefi")
    try:
        child.expect_exact("Choose:", timeout=120)
        child.sendline("2")
        child.expect_exact("Hello from Lua!", timeout=180)
        child.expect_exact("fib(10) = 55", timeout=30)
    finally:
        cleanup(child)

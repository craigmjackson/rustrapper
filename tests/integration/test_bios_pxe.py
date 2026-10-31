import pytest

from conftest import spawn, boot_and_select, cleanup


def test_bios_pxe_end_to_end():
    child = spawn("i386-bios")
    try:
        boot_and_select(child, "2", timeout=120)
        child.expect_exact("Hello from Lua!", timeout=120)
        child.expect_exact("fib(10) = 55", timeout=30)
        child.expect_exact("fetch test.txt:", timeout=30)
        child.expect_exact("21 bytes", timeout=30)
    finally:
        cleanup(child)

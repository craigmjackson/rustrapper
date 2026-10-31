"""pytest fixtures for QEMU-based integration tests.

Makefile is the single source of truth for QEMU flags: tests spawn
`make run-<target>` and interact via pexpect. No QEMU flags are duplicated
here.
"""

import os
import signal
import subprocess
import time

import pexpect
import pytest

ROOT = os.path.dirname(os.path.dirname(os.path.dirname(os.path.abspath(__file__))))


def _kill_qemu():
    """Best-effort cleanup of any qemu processes started by the tests."""
    for name in ("qemu-system-x86_64", "qemu-system-aarch64", "qemu-system-i386"):
        subprocess.run(["pkill", "-f", name], stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)


@pytest.fixture(scope="session", autouse=True)
def pxe_server():
    """Start the OpenWrt PXE server once for the whole session."""
    subprocess.run(["make", "pxe-start"], cwd=ROOT, check=False, timeout=300)
    yield
    subprocess.run(["make", "pxe-stop"], cwd=ROOT, check=False, timeout=60)
    _kill_qemu()


def spawn(target, timeout=180):
    """Spawn `make run-<target>` and return a pexpect child."""
    child = pexpect.spawn(
        f"make run-{target}",
        cwd=ROOT,
        timeout=timeout,
        encoding="utf-8",
    )
    return child


def boot_and_select(child, choice="3", timeout=120):
    """Wait for the menu, then select an option."""
    child.expect("Choose:", timeout=timeout)
    child.sendline(choice)
    return child


def cleanup(child):
    try:
        child.sendintr()
    except Exception:
        pass
    try:
        child.terminate(force=True)
    except Exception:
        pass
    _kill_qemu()

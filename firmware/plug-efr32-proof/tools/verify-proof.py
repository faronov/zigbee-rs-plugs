#!/usr/bin/env python3
"""Post-link checks for the BRD4181A always-on End Device plug proof."""

from __future__ import annotations

import pathlib
import re
import subprocess
import sys

FLASH_START = 0x0000_4000
FLASH_END = 0x0007_8000
APP_NV_START = 0x0007_8000
APP_NV_END = 0x0007_C000
SECURITY_NV_START = 0x0007_C000
SECURITY_NV_END = 0x0008_0000
RAM_START = 0x2000_0000
RAM_END = 0x2001_0000


def fail(message: str) -> "NoReturn":
    raise SystemExit(f"verify-proof: FAIL: {message}")


def tool(name: str, *args: str) -> str:
    try:
        return subprocess.check_output([name, *args], text=True, stderr=subprocess.STDOUT)
    except (FileNotFoundError, subprocess.CalledProcessError) as error:
        fail(f"{name} failed: {error}")


def symbols(elf: pathlib.Path) -> tuple[dict[str, int], dict[str, int], str]:
    plain = tool("arm-none-eabi-nm", "-n", str(elf))
    sized = tool("arm-none-eabi-nm", "-S", "-n", str(elf))
    demangled = tool("arm-none-eabi-nm", "-C", str(elf))
    values: dict[str, int] = {}
    sizes: dict[str, int] = {}
    for line in plain.splitlines():
        match = re.match(r"^([0-9a-fA-F]+)\s+\w\s+(\S+)$", line)
        if match:
            values[match.group(2)] = int(match.group(1), 16)
    for line in sized.splitlines():
        match = re.match(
            r"^([0-9a-fA-F]+)\s+([0-9a-fA-F]+)\s+\w\s+(\S+)$", line
        )
        if match:
            sizes[match.group(3)] = int(match.group(2), 16)
    return values, sizes, demangled


def size_usage(elf: pathlib.Path) -> tuple[int, int, int, int]:
    output = tool("arm-none-eabi-size", str(elf))
    lines = [line.split() for line in output.splitlines() if line.strip()]
    if len(lines) < 2 or len(lines[-1]) < 6:
        fail("could not parse arm-none-eabi-size")
    text, data, bss = map(int, lines[-1][:3])
    return text, data, bss, text + data


def load_segments(elf: pathlib.Path) -> list[tuple[int, int, int, int]]:
    output = tool("arm-none-eabi-readelf", "-lW", str(elf))
    segments = []
    for line in output.splitlines():
        fields = line.split()
        if fields and fields[0] == "LOAD" and len(fields) >= 6:
            segments.append(
                (
                    int(fields[2], 16),  # virtual address
                    int(fields[3], 16),  # physical/load address
                    int(fields[4], 16),  # file size
                    int(fields[5], 16),  # memory size
                )
            )
    if not segments:
        fail("ELF has no LOAD segments")
    return segments


def source_checks(root: pathlib.Path) -> None:
    main = (root / "src/main.rs").read_text()
    board = (root.parent.parent / "boards/efr32mg21-brd4181a-plug/src/lib.rs").read_text()
    storage = (
        root.parent.parent / "products/plug-efr32-proof/src/storage.rs"
    ).read_text()

    for required in (
        "Efr32s2Mac",
        "AlwaysOnEndDeviceApp",
        "AlwaysOnEndDevicePlugApp",
        "DeviceType::EndDevice",
        "build_into",
        "aes_startup_known_answer_test",
    ):
        if required not in main:
            fail(f"composition source lacks {required}")
    for forbidden in (
        "ParentRouterApp",
        "RelayRouterApp",
        "PersistentChildren",
        "NoChildren",
        "ChildTableStore",
        "DeviceType::Router",
    ):
        if forbidden in main:
            fail(f"end-device composition contains forbidden {forbidden}")

    pin_contract = (
        "LED_PORT: Port = Port::B",
        "LED_PIN: u8 = 0",
        "BUTTON_PORT: Port = Port::D",
        "BUTTON_PIN: u8 = 2",
        "RELAY_PORT: Port = Port::C",
        "RELAY_PIN: u8 = 3",
        "RELAY_EXPANSION_HEADER_PIN: u8 = 10",
        "relay.into_push_pull(RELAY_INACTIVE_LEVEL)",
    )
    for required in pin_contract:
        if required not in board:
            fail(f"board source lacks pin/safety contract: {required}")
    if "child_store" in storage:
        fail("child persistence leaked into end-device composition")


def cargo_feature_checks(root: pathlib.Path) -> None:
    try:
        feature_tree = subprocess.check_output(
            [
                "cargo",
                "tree",
                "--locked",
                "--manifest-path",
                str(root / "Cargo.toml"),
                "-e",
                "features",
                "-i",
                "zigbee-runtime",
            ],
            cwd=root,
            text=True,
            stderr=subprocess.STDOUT,
        )
    except (FileNotFoundError, subprocess.CalledProcessError) as error:
        fail(f"cargo feature inspection failed: {error}")
    if 'zigbee-runtime feature "router"' in feature_tree:
        fail("End Device proof enables zigbee-runtime/router")


def main() -> None:
    if len(sys.argv) != 2:
        fail("usage: verify-proof.py <firmware.elf>")
    elf = pathlib.Path(sys.argv[1]).resolve()
    if not elf.is_file():
        fail(f"ELF not found: {elf}")
    root = pathlib.Path(__file__).resolve().parents[1]
    source_checks(root)
    cargo_feature_checks(root)

    header = tool("arm-none-eabi-readelf", "-h", str(elf))
    if "Machine:" not in header or "ARM" not in header:
        fail("ELF is not ARM")

    values, sizes, demangled = symbols(elf)
    expected = {
        "__vector_table": FLASH_START,
        "_app_nv_start_": APP_NV_START,
        "_app_nv_end_": APP_NV_END,
        "_security_nv_start_": SECURITY_NV_START,
        "_security_nv_end_": SECURITY_NV_END,
        "_stack_start": RAM_END,
    }
    for name, value in expected.items():
        if values.get(name) != value:
            fail(f"{name} is {values.get(name)!r}, expected 0x{value:08x}")

    for marker in ("EFR32_ALWAYS_ON_END_DEVICE", "EFR32S2_MAC_SOFTWARE_AES_KAT"):
        if marker not in values:
            fail(f"missing retained marker {marker}")
    if sizes.get("__INTERRUPTS") != 51 * 4:
        fail(f"interrupt table is {sizes.get('__INTERRUPTS')} bytes, expected 204")
    if values.get("__INTERRUPTS") != FLASH_START + 64:
        fail("peripheral vectors do not immediately follow the 16 core vectors")
    if "FRC_PRI" not in values or values.get("FRC_PRI") == values.get("DefaultHandler"):
        fail("FRC_PRI does not resolve to the Efr32s2Mac handler")
    for required in (
        "zigbee_mac::efr32s2::Efr32s2Mac",
        "zigbee_runtime::role::EndDevice",
        "zigbee_crypto::SoftwareAes128",
        "aes::soft",
    ):
        if required not in demangled:
            fail(f"linked image lacks required composition symbol {required}")
    for forbidden in (
        "zigbee_runtime::role::RelayRouter",
        "zigbee_runtime::role::Router",
        "ParentRouterApp",
        "RelayRouterApp",
        "PersistentChildren",
        "ChildTableJournal",
    ):
        if forbidden in demangled:
            fail(f"linked End Device image unexpectedly contains {forbidden}")
    for forbidden in ("HardwareAes128", "zigbee_mac::telink", "tlsr8258_hal"):
        if forbidden in demangled:
            fail(f"linked EFR32 image unexpectedly contains {forbidden}")

    segments = load_segments(elf)
    flash_loads = [
        (paddr, filesz) for _vaddr, paddr, filesz, _memsz in segments if paddr < RAM_START
    ]
    if not flash_loads:
        fail("no flash load segment")
    image_end = max(address + length for address, length in flash_loads)
    if image_end > FLASH_END:
        fail(f"image ends at 0x{image_end:08x}, overlapping app journal")
    for address, length in flash_loads:
        if length and not (FLASH_START <= address < FLASH_END):
            fail(f"flash LOAD at 0x{address:08x} is outside application region")

    ram_segments = [
        (vaddr, memsz)
        for vaddr, _paddr, _filesz, memsz in segments
        if RAM_START <= vaddr < RAM_END
    ]
    if any(address + length > RAM_END for address, length in ram_segments):
        fail("RAM LOAD exceeds 64 KiB")

    text, data, bss, flash_payload = size_usage(elf)
    ram_static = data + bss
    binary_span = image_end - FLASH_START
    print("verify-proof: PASS")
    print(f"  flash payload (text+data): {flash_payload} bytes")
    print(f"  linked binary span:        {binary_span} bytes")
    print(f"  static RAM (data+bss):     {ram_static} bytes")
    print(f"  size detail: text={text}, data={data}, bss={bss}")
    print("  role: AlwaysOnEndDeviceApp / EndDevice (no routing or child journal)")
    print("  Cargo: zigbee-runtime/router feature excluded")
    print("  AES: RustCrypto software provider, startup AES-128 KAT retained")
    print("  pins: PD2 button, PB0 status LED, PC3/EXP10 relay proof output")


if __name__ == "__main__":
    main()

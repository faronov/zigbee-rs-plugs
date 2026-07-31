#!/usr/bin/env bash
# TLSR8258 firmware build/check helper for zigbee-rs-plugs.
#
# EXPERIMENTAL: every artifact this script produces is unproven on
# hardware. It never flashes a device — see docs/safety.md for the manual
# bring-up procedure this repository currently requires. This script only
# builds, and independently checks the post-link partition/RAM/cache/RF-DMA
# layout of, a firmware ELF against the canonical `link/tlsr8258-*.x`
# scripts (see `link/README.md`).
#
# The helper accepts an explicit crate directory, binary name, and
# flash-layout selection rather than hardcoding a product. The selected
# script is passed to the crate's build.rs through TLSR8258_LINKER_SCRIPT;
# generated linker files never modify the source tree.
set -euo pipefail

ROOT_DIR="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)"
DEFAULT_TOOLCHAIN="${ROOT_DIR}/.toolchains/tc32-stage2-tc32-45"
TC32_TOOLCHAIN="${TC32_TOOLCHAIN:-$DEFAULT_TOOLCHAIN}"
CARGO_BIN="${CARGO_BIN:-$TC32_TOOLCHAIN/bin/cargo}"
LLVM_NM="${LLVM_NM:-$TC32_TOOLCHAIN/llvm/bin/llvm-nm}"
LLVM_OBJCOPY="${LLVM_OBJCOPY:-$TC32_TOOLCHAIN/llvm/bin/llvm-objcopy}"

usage() {
    cat >&2 <<'EOF'
usage: scripts/tlsr8258-firmware.sh <check|build> <crate-dir> <binary-name> <512k|1m> <product-feature>

  check         cargo check --release --locked --no-default-features --features
                <product-feature> --bin <binary-name>
  build         cargo rustc --release --locked --no-default-features --features
                <product-feature> (bounded flags below), objcopy to .bin,
                then verify the post-link partition/RAM/cache/RF-DMA layout
                against link/tlsr8258-<512k|1m>.x

  <crate-dir>       firmware crate manifest directory (relative to the repo
                    root, or absolute)
  <binary-name>     the crate's `[[bin]]` target name
  <512k|1m>         which canonical linker script from link/ this build must
                    link against
  <product-feature> exactly one of the crate's product Cargo features (this
                    crate has no `default` feature — see its `src/main.rs`
                    compile_error! guard — so this must always be given
                    explicitly)

Environment:
  TC32_TOOLCHAIN  modern-tc32 (https://github.com/modern-tc32) toolchain
                  root. Default: ROOT/.toolchains/tc32-stage2-tc32-45
  CARGO_BIN, LLVM_NM, LLVM_OBJCOPY  override individual tool paths

This script never flashes a device. It intentionally has no `flash`
command: no flashing procedure for these boards has been proven from this
repository (see docs/safety.md). Every .bin it produces is EXPERIMENTAL.
EOF
    exit 2
}

require_file() {
    if [[ ! -e "$1" ]]; then
        echo "missing $2: $1" >&2
        exit 1
    fi
}

# Bounded, deterministic release flags. Never widen these to `opt-level=3`
# or drop `codegen-units=1`/`lto=fat`: this workspace's firmware targets a
# 512 KiB/1 MiB flash budget, and non-deterministic codegen would make the
# post-link layout checks below flaky.
RELEASE_RUSTFLAGS=(-C lto=fat -C opt-level=s -C codegen-units=1)

link_script_for() {
    case "$1" in
        512k) echo "${ROOT_DIR}/link/tlsr8258-512k.x" ;;
        1m) echo "${ROOT_DIR}/link/tlsr8258-1m.x" ;;
        *)
            echo "unknown link layout '$1' (expected 512k or 1m)" >&2
            exit 2
            ;;
    esac
}

verify_layout() {
    local elf="$1"
    local bin="$2"
    local layout="$3"
    local ramcode_start=0 ramcode_end=0 ramcode_aligned=0
    local ictag_start=0 ictag_end=0 icache_data_end=0
    local sdata=0 ebss=0 svc_bottom=0
    local rf_dma_start=0 rf_dma_end=0
    local rf_rx_buf=0 rf_tx_buf=0 rf_ack_tx_buf=0
    local app_nv_start=0 app_nv_end=0
    local security_nv_start=0 security_nv_end=0
    local factory_data_start=0 factory_data_end=0
    local flash_capacity=0
    local candidate_energy_start=0 candidate_energy_end=0
    local value name

    while read -r value _ name; do
        case "$name" in
            _ramcode_start_) ramcode_start=$((16#$value)) ;;
            _ramcode_end_) ramcode_end=$((16#$value)) ;;
            _ramcode_size_align_256_) ramcode_aligned=$((16#$value)) ;;
            _ictag_start_) ictag_start=$((16#$value)) ;;
            _ictag_end_) ictag_end=$((16#$value)) ;;
            _icache_data_end_) icache_data_end=$((16#$value)) ;;
            _sdata) sdata=$((16#$value)) ;;
            _ebss) ebss=$((16#$value)) ;;
            _svc_stack_bottom) svc_bottom=$((16#$value)) ;;
            _rf_dma_start_) rf_dma_start=$((16#$value)) ;;
            _rf_dma_end_) rf_dma_end=$((16#$value)) ;;
            *RF_RX_BUF) rf_rx_buf=$((16#$value)) ;;
            *RF_TX_BUF) rf_tx_buf=$((16#$value)) ;;
            *RF_ACK_TX_BUF) rf_ack_tx_buf=$((16#$value)) ;;
            _app_nv_start_) app_nv_start=$((16#$value)) ;;
            _app_nv_end_) app_nv_end=$((16#$value)) ;;
            _security_nv_start_) security_nv_start=$((16#$value)) ;;
            _security_nv_end_) security_nv_end=$((16#$value)) ;;
            _factory_data_start_) factory_data_start=$((16#$value)) ;;
            _factory_data_end_) factory_data_end=$((16#$value)) ;;
            _flash_capacity_) flash_capacity=$((16#$value)) ;;
            _candidate_energy_start_) candidate_energy_start=$((16#$value)) ;;
            _candidate_energy_end_) candidate_energy_end=$((16#$value)) ;;
        esac
    done < <("$LLVM_NM" "$elf")

    # RAM code / instruction-cache layout (unchanged from zigbee-rs's
    # proven products/tlsr8258-tb04/link/memory.x checks).
    if (( ramcode_end > 0x8000 )); then
        printf 'layout-check FAIL: .ram_code ends at 0x%X, after .text base 0x8000\n' \
            "$ramcode_end" >&2
        exit 1
    fi
    if (( ebss > svc_bottom )); then
        printf 'layout-check FAIL: .bss ends at 0x%X, stack starts at 0x%X\n' \
            "$ebss" "$svc_bottom" >&2
        exit 1
    fi
    if (( rf_dma_start < icache_data_end || rf_dma_end > svc_bottom )); then
        printf 'layout-check FAIL: .rf_dma [0x%X..0x%X) is outside DMA-safe RAM [0x%X..0x%X)\n' \
            "$rf_dma_start" "$rf_dma_end" "$icache_data_end" "$svc_bottom" >&2
        exit 1
    fi
    local dma_starts=("$rf_rx_buf" "$rf_tx_buf" "$rf_ack_tx_buf")
    local dma_lengths=$((2 * 144))
    local dma_sizes=("$dma_lengths" 144 144)
    local dma_names=("RF_RX_BUF" "RF_TX_BUF" "RF_ACK_TX_BUF")
    local i j start end other_start other_end
    for i in 0 1 2; do
        start="${dma_starts[$i]}"
        end=$((start + dma_sizes[i]))
        if (( start == 0 || start % 4 != 0 || start < rf_dma_start || end > rf_dma_end )); then
            printf 'layout-check FAIL: %s [0x%X..0x%X) is missing, misaligned, or outside .rf_dma\n' \
                "${dma_names[$i]}" "$start" "$end" >&2
            exit 1
        fi
        for ((j = i + 1; j < 3; j++)); do
            other_start="${dma_starts[$j]}"
            other_end=$((other_start + dma_sizes[j]))
            if (( start < other_end && other_start < end )); then
                printf 'layout-check FAIL: %s overlaps %s\n' \
                    "${dma_names[$i]}" "${dma_names[$j]}" >&2
                exit 1
            fi
        done
    done
    local expected_tag_start=$((0x840000 + ramcode_aligned))
    local expected_data_start=$((expected_tag_start + 0x900))
    if (( ictag_start != expected_tag_start || ictag_end != ictag_start + 0x100 )); then
        echo "layout-check FAIL: invalid instruction-cache tag reservation" >&2
        exit 1
    fi
    if (( icache_data_end != expected_data_start || sdata < icache_data_end )); then
        echo "layout-check FAIL: writable data overlaps the instruction cache" >&2
        exit 1
    fi
    if (( ramcode_end - ramcode_start < 0x100 )); then
        echo "layout-check FAIL: flash routines are not retained in RAM code" >&2
        exit 1
    fi
    if ! "$LLVM_NM" -C "$elf" | awk '
        /zigbee_mac::telink::imp::TelinkMac/ { found = 1 }
        END { exit(found ? 0 : 1) }
    '; then
        echo "layout-check FAIL: firmware does not link the reusable Telink MAC" >&2
        exit 1
    fi
    if "$LLVM_NM" -C "$elf" | awk '
        /Tlsr8258Mac/ { found = 1 }
        END { exit(found ? 0 : 1) }
    '; then
        echo "layout-check FAIL: firmware links legacy lab radio code" >&2
        exit 1
    fi

    # Product-owned flash partitions (this script's addition over
    # zigbee-rs's tools/tlsr8258-firmware.sh, which only checked the
    # security journal because tlsr8258-tb04 has no separate app-NV
    # partition).
    if (( app_nv_start == 0 || app_nv_end == 0 || security_nv_start == 0 )); then
        echo "layout-check FAIL: linker script did not export _app_nv_*/_security_nv_* symbols" >&2
        exit 1
    fi
    if (( app_nv_end != security_nv_start )); then
        printf 'layout-check FAIL: application-NV [..0x%X) does not end where the security journal [0x%X..) begins\n' \
            "$app_nv_end" "$security_nv_start" >&2
        exit 1
    fi
    if (( security_nv_end > factory_data_start )); then
        printf 'layout-check FAIL: security journal ends at 0x%X, after factory data starts at 0x%X\n' \
            "$security_nv_end" "$factory_data_start" >&2
        exit 1
    fi
    if (( factory_data_end > flash_capacity )); then
        printf 'layout-check FAIL: factory data ends at 0x%X, after flash capacity 0x%X\n' \
            "$factory_data_end" "$flash_capacity" >&2
        exit 1
    fi
    if [[ "$layout" == "1m" ]]; then
        if (( candidate_energy_start == 0 || candidate_energy_end == 0 )); then
            echo "layout-check FAIL: 1m layout must export the disabled _candidate_energy_* symbols" >&2
            exit 1
        fi
        if (( candidate_energy_start < security_nv_end || candidate_energy_end > factory_data_start )); then
            echo "layout-check FAIL: candidate energy region is not disjoint from security journal/factory data" >&2
            exit 1
        fi
    fi

    local size
    size=$(wc -c < "$bin" | tr -d ' ')
    if (( size > app_nv_start )); then
        printf 'layout-check FAIL: image is %d bytes, application-NV partition starts at 0x%X\n' \
            "$size" "$app_nv_start" >&2
        exit 1
    fi

    printf 'layout-check OK (EXPERIMENTAL, not hardware-proven): image=%d B app_nv=[0x%X..0x%X) security_nv=[0x%X..0x%X) factory_data=[0x%X..0x%X) capacity=0x%X ram_code=%d B rf_dma=[0x%X..0x%X) (rx=0x%X tx=0x%X ack=0x%X)\n' \
        "$size" "$app_nv_start" "$app_nv_end" "$security_nv_start" "$security_nv_end" \
        "$factory_data_start" "$factory_data_end" "$flash_capacity" \
        "$((ramcode_end - ramcode_start))" "$rf_dma_start" "$rf_dma_end" \
        "$rf_rx_buf" "$rf_tx_buf" "$rf_ack_tx_buf"
}

[[ $# -eq 5 ]] || usage
command="$1"
crate_dir="$2"
binary_name="$3"
link_layout="$4"
product_feature="$5"

if [[ "$crate_dir" != /* ]]; then
    crate_dir="${ROOT_DIR}/${crate_dir}"
fi
link_script="$(link_script_for "$link_layout")"
require_file "$link_script" "canonical linker script"
require_file "$crate_dir/Cargo.toml" "Cargo manifest"
require_file "$CARGO_BIN" "tc32 cargo (modern-tc32 toolchain)"

# Always use a known absolute target directory. Firmware crates are workspace
# members, so Cargo would otherwise place artifacts at the workspace root
# even when invoked from the member directory.
target_root="${CARGO_TARGET_DIR:-${ROOT_DIR}/target}"
if [[ "$target_root" != /* ]]; then
    target_root="${ROOT_DIR}/${target_root}"
fi
target_dir="${target_root}/tc32-unknown-none-elf/release"
elf="${target_dir}/${binary_name}"
bin="${elf}.bin"

case "$command" in
    check)
        (
            cd "$crate_dir"
            CARGO_TARGET_DIR="$target_root" \
                TLSR8258_LINKER_SCRIPT="$link_script" \
                "$CARGO_BIN" check --release --locked --no-default-features \
                    --features "$product_feature" --bin "$binary_name"
        )
        ;;
    build)
        echo "EXPERIMENTAL: building unproven ${link_layout} firmware '${binary_name}' (feature '${product_feature}') — do not flash without following docs/safety.md" >&2
        (
            cd "$crate_dir"
            CARGO_TARGET_DIR="$target_root" \
                TLSR8258_LINKER_SCRIPT="$link_script" \
                "$CARGO_BIN" rustc --release --locked --no-default-features \
                    --features "$product_feature" --bin "$binary_name" \
                    -- "${RELEASE_RUSTFLAGS[@]}"
        )
        require_file "$LLVM_OBJCOPY" "llvm-objcopy"
        require_file "$LLVM_NM" "llvm-nm"
        "$LLVM_OBJCOPY" -O binary "$elf" "$bin"
        verify_layout "$elf" "$bin" "$link_layout"
        echo "$bin"
        ;;
    *)
        usage
        ;;
esac

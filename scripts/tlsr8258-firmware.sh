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
                verify the post-link partition/RAM/cache/RF-DMA layout against
                link/tlsr8258-<512k|1m>.x, assert the typed-Router symbol gate
                (parent path present; End Device Timeout client + EndDevice/
                RelayRouter roles absent), and write <binary>.size.json
                (image size, app-NV budget, headroom, sha256)

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

# Portable SHA-256 of a file: Linux CI has `sha256sum`, macOS has
# `shasum -a 256`. Emits the bare hex digest on stdout.
sha256_of() {
    if command -v sha256sum >/dev/null 2>&1; then
        sha256sum "$1" | awk '{print $1}'
    else
        shasum -a 256 "$1" | awk '{print $1}'
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

verify_symbols() {
    local elf="$1"
    # Demangled symbol table. The typed-`Router` migration (zigbee-rs
    # `role::Router` + `build_router_into`) must be visible in the *linked*
    # image, not just the source: a mains plug is a genuine parent router and
    # must never regress to the default leaf `EndDevice` (whose R22 End Device
    # Timeout CLIENT lifecycle would otherwise be pulled in) or the
    # forwarding-only `RelayRouter`. This gate reads the same `llvm-nm -C`
    # output the layout check uses and asserts both halves of that contract.
    local syms
    syms="$("$LLVM_NM" -C "$elf")"

    # ── hardware AES required; software fallback forbidden ──────────────────
    # Check the linked ELF, not Cargo metadata: `aes` can remain in the
    # dependency graph for host/test APIs while LTO removes its implementation
    # from a correctly wired production image.
    if grep -Eq 'aes::soft|SoftwareAes128' <<<"$syms"; then
        echo "symbol-gate FAIL: production image still links software AES" >&2
        exit 1
    fi
    if ! grep -q 'HardwareAes128' <<<"$syms"; then
        echo "symbol-gate FAIL: production image is missing TLSR8258 hardware AES" >&2
        exit 1
    fi
    if ! grep -q 'install_aes_engine' <<<"$syms"; then
        echo "symbol-gate FAIL: production image never installs the TLSR8258 AES engine" >&2
        exit 1
    fi

    # ── parent-present ───────────────────────────────────────────────────────
    # The device is monomorphized for `zigbee_runtime::role::Router`, it links
    # the concrete child-serving path (`handle_child_rejoin_request`), and it
    # starts the NWK as a router (`nlme_start_router`). All three are required:
    # the role tag alone would not prove the parent server code was retained.
    local present_all=(
        "zigbee_runtime::role::Router"
        "handle_child_rejoin_request"
        "zigbee_nwk::NwkLayer"
    )
    local missing=""
    local needle
    for needle in "${present_all[@]}"; do
        if ! grep -qF -- "$needle" <<<"$syms"; then
            missing+=" $needle"
        fi
    done
    # `nlme_start_router` proves the NWK router-start path is linked; keep it a
    # required parent signal too.
    if ! grep -qF -- "nlme_start_router" <<<"$syms"; then
        missing+=" nlme_start_router"
    fi
    if [[ -n "$missing" ]]; then
        echo "symbol-gate FAIL: typed Router build is missing parent-present symbol(s):${missing}" >&2
        exit 1
    fi

    # ── leaf/relay-absent ────────────────────────────────────────────────────
    # A `Router` monomorphization must materialize neither the `EndDevice`/
    # `RelayRouter` role nor any method of the End Device Timeout *client*
    # lifecycle (owned solely by `EndDeviceRole` upstream — see zigbee-rs
    # `zigbee-runtime/src/role.rs`). Each of these appearing would mean the plug
    # silently linked a leaf/relay code path.
    local absent_all=(
        "zigbee_runtime::role::EndDevice"
        "zigbee_runtime::role::RelayRouter"
        "begin_end_device_timeout_negotiation"
        "resume_end_device_timeout"
        "advance_end_device_timeout"
        "service_end_device_timeout"
        "apply_end_device_timeout_change"
        "send_ed_timeout_request_tracked"
        "reset_end_device_timeout_state"
        "note_end_device_poll"
        "record_end_device_keepalive"
    )
    local leaked=""
    for needle in "${absent_all[@]}"; do
        if grep -qF -- "$needle" <<<"$syms"; then
            leaked+=" $needle"
        fi
    done
    if [[ -n "$leaked" ]]; then
        echo "symbol-gate FAIL: typed Router build unexpectedly links leaf/relay symbol(s):${leaked}" >&2
        exit 1
    fi

    echo "symbol-gate OK (EXPERIMENTAL, not hardware-proven): hardware AES present and software AES absent; parent Router path present (role::Router, handle_child_rejoin_request, nlme_start_router); ED-timeout client + EndDevice/RelayRouter roles absent"
}

verify_layout() {
    local elf="$1"
    local bin="$2"
    local layout="$3"
    local feature="$4"
    local json_out="$5"
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

    # Machine-readable size/budget artifact. The image budget is the start of
    # the product-owned application-NV partition: the `.bin` must always fit
    # strictly below it (already hard-failed above), so `headroom_bytes` is the
    # remaining flash before the image would collide with persistent state.
    # This is EXPERIMENTAL metadata for CI trend tracking, never a hardware
    # guarantee.
    if [[ -n "$json_out" ]]; then
        local sha
        sha="$(sha256_of "$bin")"
        local headroom=$((app_nv_start - size))
        cat >"$json_out" <<EOF
{
  "experimental": true,
  "hardware_proven": false,
  "product": "$feature",
  "layout": "$layout",
  "image_bytes": $size,
  "budget_bytes": $app_nv_start,
  "headroom_bytes": $headroom,
  "app_nv_start": "0x$(printf '%X' "$app_nv_start")",
  "security_nv": ["0x$(printf '%X' "$security_nv_start")", "0x$(printf '%X' "$security_nv_end")"],
  "factory_data": ["0x$(printf '%X' "$factory_data_start")", "0x$(printf '%X' "$factory_data_end")"],
  "flash_capacity": "0x$(printf '%X' "$flash_capacity")",
  "ram_code_bytes": $((ramcode_end - ramcode_start)),
  "sha256": "$sha"
}
EOF
        printf 'size-json written: %s (image=%d B budget=%d B headroom=%d B sha256=%s)\n' \
            "$json_out" "$size" "$app_nv_start" "$headroom" "$sha"
    fi
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
size_json="${elf}.size.json"

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
        verify_layout "$elf" "$bin" "$link_layout" "$product_feature" "$size_json"
        verify_symbols "$elf"
        echo "$bin"
        ;;
    *)
        usage
        ;;
esac

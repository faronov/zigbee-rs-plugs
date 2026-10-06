/* Canonical TLSR8258 linker script for 1 MiB parts.
 *
 * Shared across every 1 MiB product in this workspace — do not fork a
 * per-product copy. Model-specific selection happens in each firmware
 * crate's own `build.rs`, which copies exactly one of the two canonical
 * scripts under `link/` to `OUT_DIR/memory.x` (see `link/README.md`).
 *
 * Cache/RAM/RF-DMA layout is carried over unmodified from the proven
 * zigbee-rs `products/tlsr8258-tb04/link/memory.x`:
 *   0x840000 + A         RAM-code backing end / I-cache tag start
 *   0x840100 + A         I-cache tag end / I-cache data start
 *   0x840900 + A         I-cache data end / .data start
 *   0x850000             top of SRAM
 * A is the RAM-code preload size rounded up to 256 bytes.
 *
 * Flash partitions (`zigbee_plug_hardware::TLSR8258_1M_LAYOUT`):
 *   firmware                0x000000..0x070000  (this script's FLASH region)
 *   child-table journal      0x070000..0x072000  (two-sector durable journal)
 *   application NV          0x072000..0x074000  (product-owned log NV)
 *   security journal         0x074000..0x076000  (Zigbee security counters)
 *   candidate energy journal 0x096000..0x0FC000  (disabled/unproven, see below)
 *   factory/read-only        0x0FE000..0x100000  (documented, not writable here)
 *   capacity                   0x100000            (1 MiB)
 *
 * The firmware image must end strictly before `_child_nv_start_`: it must never
 * overlap any durable journal.
 *
 * `_candidate_energy_start_`/`_candidate_energy_end_` only document the
 * catalogued `0x96000..0xFC000` candidate region. Nothing in this script
 * or the workspace's Rust code reserves RAM, MEMORY space, or a write path
 * for it: it remains disabled and read-only by convention until a product
 * adopts it with its own verified persistence or staging policy.
 */
MEMORY
{
    FLASH : ORIGIN = 0x00000000, LENGTH = 0x70000
    RAM   : ORIGIN = 0x00840000, LENGTH = 0x10000
}

ENTRY(_reset_vector);

SECTIONS
{
    .vectors :
    {
        KEEP(*(.vectors));
        KEEP(*(.vectors.*));
    } > FLASH

    .ram_code :
    {
        _ramcode_start_ = .;
        *(.ram_code .ram_code.*);
        _ramcode_end_ = .;
    } > FLASH
    . = ALIGN(4);
    _rstored_ = .;
    _ramcode_size_ = .;
    _ramcode_size_div_16_ = (. + 15) / 16;
    _ramcode_size_div_256_ = (. + 255) / 256;
    _ramcode_size_div_16_align_256_ = ((. + 255) / 256) * 16;
    _ramcode_size_align_256_ = _ramcode_size_div_16_align_256_ * 16;

    .text 0x8000 :
    {
        *(.text._start);
        *(.text._start.*);
        *(.text .text.*);
        *(.rodata .rodata.*);
        *(.ARM.exidx .ARM.exidx.*);
    } > FLASH
    . = ALIGN(4);
    _dstored_ = .;
    _code_size_ = .;

    _ictag_start_ = 0x840000 + _ramcode_size_align_256_;
    _ictag_end_ = _ictag_start_ + 0x100;
    _icache_data_start_ = _ictag_end_;
    _icache_data_end_ = _icache_data_start_ + 0x800;
    _sram_data_start_ = 0x840900 + _ramcode_size_align_256_;

    .data _sram_data_start_ : AT(_dstored_)
    {
        _sdata = .;
        *(.data .data.*);
        . = ALIGN(4);
        _edata = .;
    } > RAM

    .bss (NOLOAD) :
    {
        . = ALIGN(4);
        _sbss = .;
        *(.bss .bss.*);
        *(.bss.irq_stk);
        *(COMMON);
        . = ALIGN(4);
        _ebss = .;
    } > RAM

    .rf_dma (NOLOAD) :
    {
        . = ALIGN(4);
        _rf_dma_start_ = .;
        KEEP(*(.rf_dma));
        . = ALIGN(4);
        _rf_dma_end_ = .;
    } > RAM

    /* Keep a 16 KiB SVC stack and 1 KiB IRQ stack at the top of SRAM. */
    _svc_stack_bottom = 0x0084BC00;
    _svc_stack_top    = 0x0084FC00;
    _irq_stack_bottom = 0x0084FC00;
    _irq_stack_top    = 0x00850000;
    _stack_top = _svc_stack_top;

    _bin_size_ = _code_size_ + SIZEOF(.data);
    _bin_size_div_16 = (_bin_size_ + 15) / 16;
    _etext = _dstored_;

    /* Product-owned flash partitions. Keep in sync with
     * `zigbee_plug_hardware::TLSR8258_1M_LAYOUT`. */
    _child_nv_start_ = 0x70000;
    _child_nv_end_ = 0x72000;
    _app_nv_start_ = 0x72000;
    _app_nv_end_ = 0x74000;
    _security_nv_start_ = 0x74000;
    _security_nv_end_ = 0x76000;
    /* Disabled/unproven — documentation only, see file header. */
    _candidate_energy_start_ = 0x96000;
    _candidate_energy_end_ = 0xFC000;
    _factory_data_start_ = 0xFE000;
    _factory_data_end_ = 0x100000;
    _flash_capacity_ = 0x100000;

    _ramcode_stored_ = LOADADDR(.ram_code);
    _start_data_ = _sdata;
    _end_data_ = _edata;
    _start_bss_ = _sbss;
    _end_bss_ = _ebss;
    _stack_end_ = _stack_top;
    _custom_stored_ = _etext;
    _start_custom_data_ = _edata;
    _end_custom_data_ = _edata;
    _start_custom_bss_ = _ebss;
    _end_custom_bss_ = _ebss;

    _assert_ramcode_fits = ASSERT(_ramcode_end_ <= 0x8000,
        "ERROR: .ram_code overflows the absolute .text base at FLASH+0x8000");
    _assert_cache_layout = ASSERT(_sdata >= _icache_data_end_,
        "ERROR: .data overlaps the TLSR8258 I-cache tag/data reservation");
    _assert_bss_under_stack = ASSERT(_ebss <= _svc_stack_bottom,
        "ERROR: .bss/.data extends into the SVC stack region");
    _assert_dma_outside_cache = ASSERT(_rf_dma_start_ >= _icache_data_end_,
        "ERROR: .rf_dma overlaps the TLSR8258 I-cache tag/data reservation");
    _assert_dma_under_stack = ASSERT(_rf_dma_end_ <= _svc_stack_bottom,
        "ERROR: .rf_dma extends into the SVC stack region");
    _assert_image_below_child_nv = ASSERT(_bin_size_ < _child_nv_start_,
        "ERROR: firmware image must end before the child-table journal at 0x70000");
    _assert_partitions_ordered = ASSERT(
        _child_nv_start_ <= _child_nv_end_
        && _child_nv_end_ == _app_nv_start_
        && _app_nv_start_ <= _app_nv_end_
        && _app_nv_end_ == _security_nv_start_
        && _security_nv_start_ <= _security_nv_end_
        && _security_nv_end_ <= _candidate_energy_start_
        && _candidate_energy_start_ <= _candidate_energy_end_
        && _candidate_energy_end_ <= _factory_data_start_
        && _factory_data_start_ <= _factory_data_end_
        && _factory_data_end_ <= _flash_capacity_,
        "ERROR: flash partitions are not disjoint and ascending");

    /DISCARD/ :
    {
        *(.ARM.attributes);
        *(.comment);
    }
}

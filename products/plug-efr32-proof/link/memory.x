/* BRD4181A / EFR32MG21A020F512IM32 smart-plug proof.
 *
 * 0x00000000..0x00003FFF  reserved bootloader
 * 0x00004000..0x00077FFF  application (464 KiB)
 * 0x00078000..0x0007BFFF  app-state journal (2 x 8 KiB)
 * 0x0007C000..0x0007FFFF  security journal (2 x 8 KiB)
 * 0x20000000..0x2000FFFF  SRAM (64 KiB)
 */
MEMORY
{
    FLASH       : ORIGIN = 0x00004000, LENGTH = 0x00074000
    APP_NV      : ORIGIN = 0x00078000, LENGTH = 0x00004000
    SECURITY_NV : ORIGIN = 0x0007C000, LENGTH = 0x00004000
    RAM         : ORIGIN = 0x20000000, LENGTH = 0x00010000
}

_app_nv_start_ = ORIGIN(APP_NV);
_app_nv_end_ = ORIGIN(APP_NV) + LENGTH(APP_NV);
_security_nv_start_ = ORIGIN(SECURITY_NV);
_security_nv_end_ = ORIGIN(SECURITY_NV) + LENGTH(SECURITY_NV);
_flash_capacity_ = 0x00080000;

ASSERT(ORIGIN(FLASH) + LENGTH(FLASH) == ORIGIN(APP_NV),
       "application flash must end at app journal")
ASSERT(ORIGIN(APP_NV) + LENGTH(APP_NV) == ORIGIN(SECURITY_NV),
       "app and security journals must be adjacent and disjoint")
ASSERT(ORIGIN(SECURITY_NV) + LENGTH(SECURITY_NV) == _flash_capacity_,
       "security journal must end at physical flash capacity")
ASSERT(LENGTH(APP_NV) == 2 * 0x2000,
       "app journal requires two physical 8 KiB sectors")
ASSERT(LENGTH(SECURITY_NV) == 2 * 0x2000,
       "security journal requires two physical 8 KiB sectors")

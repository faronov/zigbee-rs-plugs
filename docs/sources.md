# Hardware and protocol evidence

Implementation is original. The following public sources are used only for
hardware connections, product identifiers, protocol constants, and safety
boundaries:

- Legacy BL0937 pin map:
  [`devbis/zigbee-rs@2d8214c`](https://github.com/devbis/zigbee-rs/commit/2d8214cfbb4abef329f3f01cd1e1631733dd61c7).
- BL0942 TS011F board connections and known fingerprints:
  [`slacky1965/tuya_plug_ts011f_zrd@d2e0eaa`](https://github.com/slacky1965/tuya_plug_ts011f_zrd/tree/d2e0eaa47539dd75229b12cde347ba9150c348a9).
- Experimental 512 KiB stock identity:
  [`slacky1965/tuya_plug_ts011f_zrd@5f8f1ea`](https://github.com/slacky1965/tuya_plug_ts011f_zrd/tree/5f8f1ea3332ec069343ab3323675b98e5c80c7c1).
- BL0942 register protocol:
  [Belling BL0942 V1.06 datasheet](https://www.belling.com.cn/media/file_object/bel_product/BL0942/datasheet/BL0942_V1.06_en.pdf).
- Zigbee stack and TLSR8258 HAL:
  [`faronov/zigbee-rs@b97c749`](https://github.com/faronov/zigbee-rs/commit/b97c749a66799dfafe9096bb16e4893f45519d5e).
- ZiGate Trust Center authentication fix:
  [`fairecasoimeme/ZiGate v3.1d...v3.1e`](https://github.com/fairecasoimeme/ZiGate/compare/v3.1d...v3.1e).
- Zbeacon TS011F BL0937 pin map and 512 KiB geometry: a user-owned full
  stock flash dump, SHA-256
  `ede1ab20ae7c05f26de25dcf84e0ed964f3b9da918b431de3ca767c9c243c7cd`.
  The dump contains live Zigbee identity/security state and is deliberately
  not included in this repository.

No stock OTA identity is treated as permission to flash. Physical PCB
inspection, a preserved flash dump, JEDEC verification, and a matching product
target remain mandatory.

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
  [`faronov/zigbee-rs@1c79264`](https://github.com/faronov/zigbee-rs/commit/1c79264a3014ec0a442ccae3860306605d1cdb90).

No stock OTA identity is treated as permission to flash. Physical PCB
inspection, a preserved flash dump, JEDEC verification, and a matching product
target remain mandatory.

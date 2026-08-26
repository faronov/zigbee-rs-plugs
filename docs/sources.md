# Hardware, protocol, and implementation evidence

Implementation in this repository is original. Public sources are used for
hardware connections, product identifiers, protocol constants, and safety
boundaries.

## Current experiment-branch implementation

- Shared plug application:
  [`apps/plug-router`](../apps/plug-router/).
- TLSR8258 composition:
  [`firmware/tlsr8258-plug`](../firmware/tlsr8258-plug/).
- EFR32 BRD4181A board contract:
  [`boards/efr32mg21-brd4181a-plug`](../boards/efr32mg21-brd4181a-plug/).
- EFR32 proof product/layout:
  [`products/plug-efr32-proof`](../products/plug-efr32-proof/).
- Core router frontend:
  [`apps/router@002d479`](https://github.com/faronov/zigbee-rs/tree/002d47923ab910afd46b771cc1b4d2eaee1696c9/apps/router).
- Core runtime, EFR32/TLSR8258 HALs, and MAC backends:
  [`faronov/zigbee-rs@002d479`](https://github.com/faronov/zigbee-rs/tree/002d47923ab910afd46b771cc1b4d2eaee1696c9).

The manifests pin this exact core revision as a Git dependency. The public
zigbee-rs GitHub Pages book reflects the deployed main branch and will not show
this migration until the core branch is merged and Pages is deployed.

## External hardware and protocol sources

- Legacy BL0937 pin map:
  [`devbis/zigbee-rs@2d8214c`](https://github.com/devbis/zigbee-rs/commit/2d8214cfbb4abef329f3f01cd1e1631733dd61c7).
- BL0942 TS011F connections and known fingerprints:
  [`slacky1965/tuya_plug_ts011f_zrd@d2e0eaa`](https://github.com/slacky1965/tuya_plug_ts011f_zrd/tree/d2e0eaa47539dd75229b12cde347ba9150c348a9).
- Experimental 512 KiB `_TZ3000_gjnozsaz` stock identity:
  [`slacky1965/tuya_plug_ts011f_zrd@5f8f1ea`](https://github.com/slacky1965/tuya_plug_ts011f_zrd/tree/5f8f1ea3332ec069343ab3323675b98e5c80c7c1).
- BL0942 register protocol:
  [Belling BL0942 V1.06 datasheet](https://www.belling.com.cn/media/file_object/bel_product/BL0942/datasheet/BL0942_V1.06_en.pdf).
- ZiGate Trust Center authentication fix:
  [`fairecasoimeme/ZiGate v3.1d...v3.1e`](https://github.com/fairecasoimeme/ZiGate/compare/v3.1d...v3.1e).
- Zbeacon TS011F BL0937 pin map, calibration, and 512 KiB geometry: a
  user-owned full stock dump, SHA-256
  `ede1ab20ae7c05f26de25dcf84e0ed964f3b9da918b431de3ca767c9c243c7cd`.
  It contains live Zigbee identity/security state and is deliberately not
  included in this repository.

No stock OTA fingerprint is permission to flash. Physical PCB inspection, a
preserved dump, live JEDEC verification, and a matching product target remain
mandatory.

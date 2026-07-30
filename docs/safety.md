# Safety

Smart plugs contain non-isolated mains-voltage circuitry.

- Disconnect the plug before opening it.
- Do not power an exposed PCB from mains while attaching a programmer.
- Use an isolated low-voltage programming setup and verify the board's test
  points before connecting it.
- Start every new build with the relay output driven to its inactive level.
- Read and preserve a complete flash dump before any erase or write.
- Verify the flash JEDEC ID, PCB layout, metering IC marking, relay pin, and
  active levels. A Tuya manufacturer string is not proof of hardware identity.
- Do not use an OTA image intended for another flash size or stock image
  identity.
- Treat overload protection in custom firmware as an additional safeguard,
  not a certified protective device.

The project currently publishes no plug firmware or OTA image.

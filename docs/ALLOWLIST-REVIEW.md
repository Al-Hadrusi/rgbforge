# Hardware allow-list review

Every driver has an allow-list: a function that every byte sent to the device must pass.
It is the last line of defence, so it is where a reviewer should look first. This page
records what each list permits, what it was checked against, and what is still open.

**Status:** reviewed once by an automated reviewer that was given the code (not the
author's conclusions) and compared it with OpenRGB and liquidctl source. Not yet reviewed
by an independent human. If you know one of these protocols, please check the list for it
and open an issue (or a private security report for anything dangerous).

References: OpenRGB `Controllers/` (<https://gitlab.com/CalcProgrammer1/OpenRGB>),
liquidctl `liquidctl/driver/` (<https://github.com/liquidctl/liquidctl>).

| Driver | Allow-list | Allowed | Never reachable | Checked against |
|---|---|---|---|---|
| ENE RAM (`ene_ram.rs`) | `write_ok`, `pointer_ok` | name/LED-count reads; direct mode 0/1; apply 0x01; the stick's own colour block (5 LEDs at 0x8000, 10 at 0x8100) | save (0x80A0=0xAA), slot/address 0x80F8/0x80F9, effect/mode registers, any address outside 0x70-0x77; needs SPD write-protect on and an administrator's switch | `ENESMBusController.h/.cpp`, `ENESMBusInterface_i2c_smbus.cpp` |
| Sapphire GPU (`sapphire.rs`) | `write_allowed`, `reg_allowed` | 0x0F (0/1), 0x10 (known modes), 0x1A-0x1C colours, address 0x28 on the DM OEM bus only; needs an administrator's switch | every other register, the SMU bus | `SapphireNitroGlowV3Controller.h`, `pci_ids.h` |
| ASUS Aura (`aura.rs`) | `allowed` | firmware/config reads; direct effect (0x35 board / 0x3B addressable); bounded direct colours | save `EC 3F 55`, hardware effects | `AsusAuraMainboardController.cpp`, `AsusAuraAddressableController.cpp` |
| Corsair (`corsair.rs`) | `allowed` | software mode `38 ch 02`, colours `32` (≤50, ≤204 LEDs), commit `33 FF`, channels the device has | reset `37` (part of storing hardware effects), fan/pump/temperature/firmware | `CorsairLightingNodeController.cpp`, `commander_pro.py` |
| NZXT (`nzxt.rs`) | `allowed` | accessory query; colours `22 10/11`; apply `22 A0` with OpenRGB's exact bytes; one channel bit | pump/fan `60/62/72`, hardware effect modes | `NZXTHue2Controller.cpp`, `kraken3.py`, `smart_device.py` |
| Lian Li (`lianli.rs`) | `allowed` | start/colours/commit for channels 0-3; ≤96 LEDs; R+G+B ≤ 460 per LED | anything else | `LianLiUniHubSLV2Controller.cpp` |
| ASRock (`asrock.rs`) | `allowed` | zone colour writes, config reads | save `00 12`, header write `00 15` | `ASRockPolychromeUSBController.cpp` |
| Gigabyte (`gigabyte.rs`) | `allowed` | identify; static board effect; commit; per-LED mode for D_LED1/2; valid LED-count codes; bounded colours | save to flash `CC 5E`, persistent lighting `CC 47`, calibration `CC 33` | `GigabyteRGBFusion2USBController.cpp/.h` |
| Razer (`razer.rs`) | `allowed` | firmware read; matrix rows within the model's size; custom-frame mode with NOSTORE | storing effects, everything else | `RazerController.cpp`, `RazerDevices.cpp` |

## Fixed after the review

- ENE: a chip reporting more LEDs than its layout holds could have pushed colour bytes
  into the mode registers (0x8020-0x8023). Now refused.
- Corsair: channel reset (0x37), which reference tools only send while storing a hardware
  effect that persists across power-offs, was sent at start-up. Removed.
- Lian Li: the LED power budget was only applied by the packet builder. Now in the guard.
- Aura: stand-alone ARGB controllers were sent the motherboard effect command.
- Gigabyte: 0x32/0x34/0x58/0x59 payloads were unchecked, and setting LED counts zeroed
  headers 3-6.
- NZXT: any channel mask and any apply bytes passed.
- Sapphire: register values were unchecked.

## Still open (low severity)

- Aura: bounds are not tied to each device's own channel and LED counts; the 0x19AF
  interface (usage page 0xFF72) is not checked; one firmware (AULA3-AR32) numbers
  channels differently.
- Gigabyte 0x5711: board zones 8-10 use a different mask and commit.
- ASRock: the speed byte and zone number are not tied to the board's zones.
- Razer: the data-size byte and checksum are not checked by the guard.
- ENE: chips that report a known name are trusted; OpenRGB also excludes Micron parts by a
  string this driver can't read.
- Every protocol detail above comes from reverse-engineered references, not vendor
  documentation, and most drivers have not been tested on real hardware.

//! What the kernel exposes for each touchpad, for `apex doctor`.
//!
//! APEX ships one Hyprland input block for every machine (tap-to-click on,
//! libinput's default LRM tap-button map), so when two-finger tap produces a
//! right click on one laptop and nothing on another, the difference is not in
//! our configuration — it is in what the kernel driver advertises for that
//! device. This module reports those advertised capabilities so the answer is
//! measured rather than guessed.
//!
//! What decides whether two-finger tap can work:
//!
//! * libinput counts fingers either from per-finger multitouch slots
//!   (`ABS_MT_SLOT`) or, when it has none it can trust, from the "fake touch"
//!   codes `BTN_TOOL_DOUBLETAP`/`TRIPLETAP`/`QUADTAP`/`QUINTTAP`.
//! * On a device flagged `INPUT_PROP_SEMI_MT` the two reported points are only
//!   the bounding box of the contact, and since libinput 1.1.5 libinput stops
//!   interpreting those points and treats the device as single-touch with
//!   extra finger detection. Two- and three-finger tap and two-finger scroll
//!   still work there; what semi-mt loses is pinch gestures and some palm
//!   detection. So semi-mt on its own does **not** rule out two-finger tap —
//!   the absence of `BTN_TOOL_DOUBLETAP` does.
//!
//! Everything here is read-only, needs no root, and uses no library beyond
//! std: `/proc/bus/input/devices` for enumeration and
//! `/sys/class/input/<eventN>/device/` for the authoritative bitmasks.
//!
//! One limit worth stating: sysfs publishes the capability *bitmasks* only, not
//! the `absinfo` ranges, so we can see that a device has `ABS_MT_SLOT` but not
//! how many slots it has. Reading the count needs an `EVIOCGABS` ioctl on the
//! event node. The report therefore says slots are present, never how many.

use std::path::Path;

const PROC_INPUT_DEVICES: &str = "/proc/bus/input/devices";
const SYS_CLASS_INPUT: &str = "/sys/class/input";

// linux/input-event-codes.h. Bit indices into the matching capability mask.
const INPUT_PROP_DIRECT: usize = 0x01;
const INPUT_PROP_BUTTONPAD: usize = 0x02;
const INPUT_PROP_SEMI_MT: usize = 0x03;
const ABS_X: usize = 0x00;
const ABS_Y: usize = 0x01;
const ABS_MT_SLOT: usize = 0x2f;
const REL_X: usize = 0x00;
const REL_Y: usize = 0x01;
const BTN_LEFT: usize = 0x110;
const BTN_RIGHT: usize = 0x111;
const BTN_TOOL_PEN: usize = 0x140;
const BTN_TOOL_FINGER: usize = 0x145;
const BTN_TOOL_DOUBLETAP: usize = 0x14d;

/// Test one bit in a kernel capability bitmask.
///
/// `input_print_bitmap()` in `drivers/input/input.c` walks down from the
/// highest non-zero `long` to index 0 and prints each as lowercase hex
/// separated by single spaces. Two consequences the caller must not get wrong:
/// the **low-order word is last**, and leading all-zero words are not printed
/// at all, so a mask's word count says nothing about which bits exist.
///
/// A `long` is 64 bits on x86_64, the only architecture APEX-OS builds for.
///
/// Confirmed against this machine's ELAN touchpad, whose `capabilities/key`
/// reads `e520 10000 0 0 0 0`: only with the low word last does `0xe520` sit at
/// bit offset 5*64, decoding to BTN_TOOL_FINGER (0x145), BTN_TOOL_QUINTTAP
/// (0x148), BTN_TOUCH (0x14a) and BTN_TOOL_DOUBLETAP/TRIPLETAP/QUADTAP
/// (0x14d-0x14f), with `0x10000` at offset 4*64 decoding to BTN_LEFT (0x110).
/// Read low-word-first the same file would claim a set of KEY_* keyboard keys.
pub fn mask_bit(mask: &str, bit: usize) -> bool {
    let words: Vec<&str> = mask.split_whitespace().collect();
    let word = bit / 64;
    if word >= words.len() {
        return false;
    }
    match u64::from_str_radix(words[words.len() - 1 - word], 16) {
        Ok(v) => (v >> (bit % 64)) & 1 == 1,
        // Not hex: the file is not a kernel bitmask. Report "not set" rather
        // than inventing a capability.
        Err(_) => false,
    }
}

/// One `/proc/bus/input/devices` record, reduced to the fields this check uses.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct InputRecord {
    /// `N: Name="..."`, quotes stripped.
    pub name: String,
    /// The `eventN` handler from `H: Handlers=`, empty when the record has none.
    pub event: String,
    /// `S: Sysfs=`, a path relative to `/sys`.
    pub sysfs: String,
    /// `B: PROP=`, the `INPUT_PROP_*` bitmask.
    pub props: String,
    /// `B: ABS=`, the `ABS_*` bitmask.
    pub abs: String,
    /// `B: KEY=`, the `KEY_*`/`BTN_*` bitmask.
    pub key: String,
    /// `B: REL=`, the `REL_*` bitmask.
    pub rel: String,
}

/// Parse `/proc/bus/input/devices`.
///
/// The kernel writes one `I:`-led record per device and separates records with
/// a blank line. We key off `I:` rather than the blank line so a file without
/// the trailing separator still yields its last record. Unknown tags are
/// skipped: this format gains lines over time and an unrecognised one is not an
/// error.
///
/// A `B:` line is emitted only when that mask has a non-zero bit, so a missing
/// `B: ABS=` means "no absolute axes" and is correctly left as the empty string
/// (which [`mask_bit`] reads as all-clear).
pub fn parse_input_devices(contents: &str) -> Vec<InputRecord> {
    let mut out = Vec::new();
    let mut cur = InputRecord::default();
    let mut open = false;

    for raw in contents.lines() {
        let line = raw.trim();
        let Some((tag, rest)) = line.split_once(':') else {
            continue;
        };
        let rest = rest.trim();
        match tag {
            "I" => {
                if open {
                    out.push(std::mem::take(&mut cur));
                }
                open = true;
            }
            "N" => {
                cur.name = rest
                    .strip_prefix("Name=")
                    .unwrap_or(rest)
                    .trim_matches('"')
                    .to_string()
            }
            "S" => cur.sysfs = rest.strip_prefix("Sysfs=").unwrap_or(rest).to_string(),
            "H" => {
                let handlers = rest.strip_prefix("Handlers=").unwrap_or(rest);
                // A device can carry several handlers (`kbd event3 leds`); the
                // event node is the one that identifies it under /sys/class/input.
                if let Some(ev) = handlers.split_whitespace().find(|h| {
                    h.strip_prefix("event")
                        .is_some_and(|n| !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit()))
                }) {
                    cur.event = ev.to_string();
                }
            }
            "B" => {
                if let Some(v) = rest.strip_prefix("PROP=") {
                    cur.props = v.trim().to_string();
                } else if let Some(v) = rest.strip_prefix("ABS=") {
                    cur.abs = v.trim().to_string();
                } else if let Some(v) = rest.strip_prefix("KEY=") {
                    cur.key = v.trim().to_string();
                } else if let Some(v) = rest.strip_prefix("REL=") {
                    cur.rel = v.trim().to_string();
                }
            }
            _ => {}
        }
    }
    if open {
        out.push(cur);
    }
    out
}

/// True when this record is the kind of device libinput drives as a touchpad.
///
/// Matched by capability, never by the name string. The name is a driver
/// convention, and the failure this whole check exists to explain — a touchpad
/// whose own driver did not bind and which came up through PS/2 mouse
/// emulation — can still read "Touchpad" while exposing nothing a touchpad
/// needs.
///
/// The rule mirrors udev's `input_id` builtin, which is what sets
/// `ID_INPUT_TOUCHPAD` and therefore decides what libinput treats as a
/// touchpad in the first place:
///
/// * `ABS_X` and `ABS_Y` — absolute position. This alone rejects a mouse or a
///   TrackPoint, which report `REL_X`/`REL_Y` and have an empty ABS mask.
/// * not `INPUT_PROP_DIRECT` — that property marks a touchscreen, which also
///   has `ABS_X`/`ABS_Y` and may advertise `BTN_TOOL_FINGER`.
/// * not `BTN_TOOL_PEN` — a graphics tablet, even a touch-capable one.
/// * `BTN_TOOL_FINGER` or `INPUT_PROP_BUTTONPAD` — finger-operated. The
///   buttonpad alternative catches clickpads whose driver omits the tool bit.
pub fn is_touchpad(rec: &InputRecord) -> bool {
    mask_bit(&rec.abs, ABS_X)
        && mask_bit(&rec.abs, ABS_Y)
        && !mask_bit(&rec.props, INPUT_PROP_DIRECT)
        && !mask_bit(&rec.key, BTN_TOOL_PEN)
        && (mask_bit(&rec.key, BTN_TOOL_FINGER) || mask_bit(&rec.props, INPUT_PROP_BUTTONPAD))
}

/// True for a device that moves the pointer with relative motion: a mouse, a
/// TrackPoint, or a touchpad that fell back to PS/2 mouse emulation.
///
/// Used only to explain a machine where no touchpad was found at all.
pub fn is_relative_pointer(rec: &InputRecord) -> bool {
    mask_bit(&rec.rel, REL_X) && mask_bit(&rec.rel, REL_Y) && mask_bit(&rec.key, BTN_LEFT)
}

/// Render the touchpad section as `(ok, text)` pairs for `line()`.
///
/// `ok` marks a capability that is present and usable; a `false` here is
/// information about the hardware, never an apexd fault, which matches how the
/// rest of `apex doctor` reports.
pub fn report_lines(records: &[InputRecord]) -> Vec<(bool, String)> {
    let pads: Vec<&InputRecord> = records.iter().filter(|r| is_touchpad(r)).collect();

    if pads.is_empty() {
        let others: Vec<&str> = records
            .iter()
            .filter(|r| is_relative_pointer(r))
            .map(|r| r.name.as_str())
            .collect();
        return vec![if others.is_empty() {
            (
                false,
                "touchpad: none found, and no pointing device at all — nothing to diagnose"
                    .to_string(),
            )
        } else {
            (
                false,
                format!(
                    "touchpad: none found — the only pointing devices are relative/mouse devices ({}). \
                     A built-in touchpad that enumerates this way is in PS/2 mouse emulation because \
                     its own driver never bound, and libinput gives such a device no tap and no \
                     multi-finger behaviour at all; look for the bind failure in \
                     `journalctl -k | grep -iE 'i2c_hid|elan|synaptics|psmouse'`",
                    others.join(", ")
                ),
            )
        }];
    }

    let mut out = Vec::new();
    for rec in pads {
        let slots = mask_bit(&rec.abs, ABS_MT_SLOT);
        let semi_mt = mask_bit(&rec.props, INPUT_PROP_SEMI_MT);
        let buttonpad = mask_bit(&rec.props, INPUT_PROP_BUTTONPAD);
        let doubletap = mask_bit(&rec.key, BTN_TOOL_DOUBLETAP);
        let right_button = mask_bit(&rec.key, BTN_RIGHT);

        // libinput trusts slot positions only on a device that is not semi-mt;
        // everywhere else the finger count comes from the fake-touch bits.
        let counts_two_fingers = doubletap || (slots && !semi_mt);

        let node = if rec.event.is_empty() {
            "no event node".to_string()
        } else {
            format!("/dev/input/{}", rec.event)
        };
        let sysfs = if rec.sysfs.is_empty() {
            "sysfs path unknown".to_string()
        } else {
            format!("/sys{}", rec.sysfs)
        };
        let name = if rec.name.is_empty() {
            "(unnamed)"
        } else {
            &rec.name
        };
        out.push((true, format!("touchpad: {name} ({node}, {sysfs})")));

        out.push((
            slots,
            format!(
                "  multitouch slots (ABS_MT_SLOT): {}",
                if slots {
                    "present — the kernel can track fingers individually"
                } else {
                    "absent — single-touch, the kernel reports one position only"
                }
            ),
        ));

        let basis = if semi_mt {
            "bounding box only (INPUT_PROP_SEMI_MT), so libinput ignores the slot positions"
        } else if slots {
            "per-finger slots"
        } else {
            "no slots"
        };
        out.push((
            counts_two_fingers,
            format!(
                "  finger counting: {basis}; BTN_TOOL_DOUBLETAP {}",
                if doubletap { "present" } else { "absent" }
            ),
        ));

        out.push((
            buttonpad || right_button,
            format!(
                "  button layout: {}",
                if buttonpad {
                    "clickpad (INPUT_PROP_BUTTONPAD) — one button under the pad, every other button is software-emulated"
                } else if right_button {
                    "separate physical buttons (BTN_RIGHT present)"
                } else {
                    "neither INPUT_PROP_BUTTONPAD nor BTN_RIGHT — no hardware right click exists"
                }
            ),
        ));

        let right_click_fallback = if buttonpad {
            "pressing the bottom-right corner of the pad (libinput's software button area)"
        } else if right_button {
            "the physical right button"
        } else {
            "unavailable in hardware — only a keyboard binding is left"
        };

        out.push(if counts_two_fingers {
            (
                true,
                format!(
                    "  => two-finger tap is possible on this hardware. If right click still does not \
                     work the cause is above the kernel: check that `libinput list-devices` lists \
                     this device with Capabilities 'pointer' and 'Tap-to-click: enabled', and that \
                     `hyprctl devices` shows it as a touchpad so Hyprland's input rules reach it. \
                     Right click also works by {right_click_fallback}"
                ),
            )
        } else {
            (
                false,
                format!(
                    "  => two-finger tap cannot work on this hardware: the kernel never reports a \
                     second finger, so libinput can only offer one-finger tap. Right click is {right_click_fallback}"
                ),
            )
        });
    }
    out
}

/// Read this machine's input devices and render the section.
///
/// Never panics and never fails the command: an unreadable `/proc` is itself
/// reported as a line.
pub fn doctor_lines() -> Vec<(bool, String)> {
    match std::fs::read_to_string(PROC_INPUT_DEVICES) {
        Ok(text) => {
            let mut records = parse_input_devices(&text);
            for rec in &mut records {
                refresh_from_sysfs(rec, Path::new(SYS_CLASS_INPUT));
            }
            report_lines(&records)
        }
        Err(e) => vec![(
            false,
            format!(
                "touchpad: {PROC_INPUT_DEVICES} unreadable ({e}) — no touchpad facts available"
            ),
        )],
    }
}

/// Replace a record's masks with the `/sys/class/input/<eventN>/device` copies.
///
/// Both sources print the same kernel bitmaps, but sysfs always has the file
/// whereas `/proc` omits an all-zero mask, and sysfs is per-attribute so a
/// truncated or reordered `/proc` read cannot skew one device's answer. The
/// device name is deliberately not re-read: `N: Name=` and the sysfs `name`
/// attribute are the same `dev->name` string.
///
/// When sysfs is unreadable — no event node, a restricted mount — the `/proc`
/// values are kept, so the check still reports something true.
fn refresh_from_sysfs(rec: &mut InputRecord, class_input: &Path) {
    if rec.event.is_empty() {
        return;
    }
    let base = class_input.join(&rec.event).join("device");
    for (field, name) in [
        (&mut rec.props, "properties"),
        (&mut rec.abs, "capabilities/abs"),
        (&mut rec.key, "capabilities/key"),
        (&mut rec.rel, "capabilities/rel"),
    ] {
        if let Ok(s) = std::fs::read_to_string(base.join(name)) {
            *field = s.trim().to_string();
        }
    }
}

// ── Tests ────────────────────────────────────────────────────────────────────
#[cfg(test)]
mod tests {
    use super::*;

    /// Verbatim records from `/proc/bus/input/devices` on the development
    /// machine (ThinkPad, AMD, kernel-provided I2C-HID ELAN touchpad plus a
    /// PS/2 TrackPoint). Unrelated records — buttons, keyboard, audio jacks —
    /// were dropped; the ones kept are unedited, including trailing spaces on
    /// the `H:` lines.
    const REAL_PROC: &str = "\
I: Bus=0011 Vendor=0002 Product=000a Version=0063
N: Name=\"TPPS/2 Elan TrackPoint\"
P: Phys=isa0060/serio1/input0
S: Sysfs=/devices/platform/i8042/serio1/input/input6
U: Uniq=
H: Handlers=event5 mouse0 
B: PROP=21
B: EV=7
B: KEY=70000 0 0 0 0
B: REL=3

I: Bus=0018 Vendor=04f3 Product=320b Version=0100
N: Name=\"ELAN06DA:00 04F3:320B Mouse\"
P: Phys=i2c-ELAN06DA:00
S: Sysfs=/devices/platform/AMDI0010:01/i2c-1/i2c-ELAN06DA:00/0018:04F3:320B.0001/input/input13
U: Uniq=
H: Handlers=event6 mouse1 
B: PROP=0
B: EV=17
B: KEY=30000 0 0 0 0
B: REL=3
B: MSC=10

I: Bus=0018 Vendor=04f3 Product=320b Version=0100
N: Name=\"ELAN06DA:00 04F3:320B Touchpad\"
P: Phys=i2c-ELAN06DA:00
S: Sysfs=/devices/platform/AMDI0010:01/i2c-1/i2c-ELAN06DA:00/0018:04F3:320B.0001/input/input15
U: Uniq=
H: Handlers=event10 mouse2 
B: PROP=5
B: EV=1b
B: KEY=e520 10000 0 0 0 0
B: ABS=2e0800000000003
B: MSC=20

I: Bus=0018 Vendor=1da0 Product=8007 Version=0100
N: Name=\"PRT0818:00 1DA0:8007\"
P: Phys=i2c-PRT0818:00
S: Sysfs=/devices/platform/AMDI0010:00/i2c-0/i2c-PRT0818:00/0018:1DA0:8007.0002/input/input16
U: Uniq=
H: Handlers=event7 mouse3 
B: PROP=2
B: EV=1b
B: KEY=400 0 0 0 0 0
B: ABS=260800000000003
B: MSC=20
";

    fn record(name: &str) -> InputRecord {
        parse_input_devices(REAL_PROC)
            .into_iter()
            .find(|r| r.name == name)
            .unwrap_or_else(|| panic!("no record named {name}"))
    }

    // ── mask_bit: word order ────────────────────────────────────────────────

    #[test]
    fn the_low_order_word_is_the_last_one() {
        // Two words, only the high one set: bit 64, not bit 0.
        assert!(mask_bit("1 0", 64));
        assert!(!mask_bit("1 0", 0));
        // The mirror image.
        assert!(mask_bit("0 1", 0));
        assert!(!mask_bit("0 1", 64));
    }

    #[test]
    fn bits_are_addressed_across_word_boundaries() {
        // Word 2 (bits 128..191), word 1 (64..127), word 0 (0..63).
        let mask = "8000000000000000 0 8000000000000001";
        assert!(mask_bit(mask, 191), "top bit of the highest word");
        assert!(mask_bit(mask, 63), "top bit of the low word");
        assert!(mask_bit(mask, 0), "bottom bit of the low word");
        assert!(!mask_bit(mask, 64), "word 1 is empty");
        assert!(!mask_bit(mask, 127), "word 1 is empty");
        // Past the printed words: the kernel omits leading zero words, so an
        // index beyond them is simply clear, not an error.
        assert!(!mask_bit(mask, 192));
        assert!(!mask_bit(mask, 100_000));
    }

    #[test]
    fn a_single_word_mask_behaves_like_word_zero() {
        // The touchpad's ABS mask is one word wide and must still decode
        // ABS_MT_SLOT at bit 0x2f.
        assert!(mask_bit("2e0800000000003", ABS_MT_SLOT));
        assert!(mask_bit("2e0800000000003", ABS_X));
        assert!(mask_bit("2e0800000000003", ABS_Y));
    }

    #[test]
    fn malformed_and_empty_masks_report_nothing_set() {
        for mask in ["", "   ", "zzzz", "0x10", "1 nonsense", "-1", "\n"] {
            for bit in [0usize, 1, 2, 47, 325, 333] {
                assert!(
                    !mask_bit(mask, bit),
                    "{mask:?} bit {bit} must read as clear"
                );
            }
        }
        // A value wider than a u64 is not a kernel word; refuse it rather than
        // truncating into a wrong answer.
        assert!(!mask_bit("fffffffffffffffff", 0));
    }

    // ── capability decoding on the real samples ─────────────────────────────

    #[test]
    fn the_real_touchpad_has_multitouch_slots_and_is_a_clickpad() {
        let tp = record("ELAN06DA:00 04F3:320B Touchpad");
        assert_eq!(tp.event, "event10");
        assert_eq!(
            tp.sysfs,
            "/devices/platform/AMDI0010:01/i2c-1/i2c-ELAN06DA:00/0018:04F3:320B.0001/input/input15"
        );
        assert!(
            mask_bit(&tp.abs, ABS_MT_SLOT),
            "ABS_MT_SLOT set in 2e0800000000003"
        );
        assert!(
            !mask_bit(&tp.props, INPUT_PROP_SEMI_MT),
            "PROP=5 has no semi-mt bit"
        );
        assert!(
            mask_bit(&tp.props, INPUT_PROP_BUTTONPAD),
            "PROP=5 is bits 0 and 2"
        );
        assert!(mask_bit(&tp.key, BTN_TOOL_FINGER));
        assert!(mask_bit(&tp.key, BTN_TOOL_DOUBLETAP));
        assert!(mask_bit(&tp.key, BTN_LEFT));
        // A clickpad has no separate right button.
        assert!(!mask_bit(&tp.key, BTN_RIGHT));
    }

    #[test]
    fn semi_mt_and_buttonpad_are_distinct_neighbouring_bits() {
        // PROP=8 is bit 3 alone: semi-mt without buttonpad.
        assert!(mask_bit("8", INPUT_PROP_SEMI_MT));
        assert!(!mask_bit("8", INPUT_PROP_BUTTONPAD));
        // PROP=4 is bit 2 alone: buttonpad without semi-mt.
        assert!(mask_bit("4", INPUT_PROP_BUTTONPAD));
        assert!(!mask_bit("4", INPUT_PROP_SEMI_MT));
        // PROP=d is bits 0, 2 and 3: a semi-mt clickpad.
        assert!(mask_bit("d", INPUT_PROP_BUTTONPAD));
        assert!(mask_bit("d", INPUT_PROP_SEMI_MT));
    }

    #[test]
    fn a_single_touch_pad_has_no_slot_bit() {
        // ABS_X|ABS_Y|ABS_PRESSURE|ABS_TOOL_WIDTH, the classic PS/2 Synaptics
        // single-touch axis set, with nothing at 0x2f.
        let abs = "11000003";
        assert!(mask_bit(abs, ABS_X) && mask_bit(abs, ABS_Y));
        assert!(!mask_bit(abs, ABS_MT_SLOT));
    }

    // ── classification ──────────────────────────────────────────────────────

    #[test]
    fn the_real_touchpad_classifies_as_a_touchpad() {
        assert!(is_touchpad(&record("ELAN06DA:00 04F3:320B Touchpad")));
    }

    #[test]
    fn a_trackpoint_is_not_a_touchpad() {
        // The TrackPoint is a relative device with no ABS mask at all, and its
        // PROP=21 is INPUT_PROP_POINTER plus INPUT_PROP_POINTING_STICK.
        let tp = record("TPPS/2 Elan TrackPoint");
        assert_eq!(tp.abs, "");
        assert!(!is_touchpad(&tp));
        assert!(is_relative_pointer(&tp));
    }

    #[test]
    fn the_touchpads_own_mouse_node_is_not_a_touchpad() {
        // hid-multitouch exposes a second, relative node for the same physical
        // device. It has BTN_LEFT/BTN_RIGHT and REL axes but no ABS axes, so
        // reporting it as a touchpad would double-count and give wrong answers.
        let mouse = record("ELAN06DA:00 04F3:320B Mouse");
        assert!(!is_touchpad(&mouse));
        assert!(is_relative_pointer(&mouse));
    }

    #[test]
    fn a_touchscreen_is_not_a_touchpad() {
        // Same MT axes as the touchpad, but INPUT_PROP_DIRECT (PROP=2) and only
        // BTN_TOUCH in KEY. Without the DIRECT test this device would be
        // reported as a second touchpad on every machine that has one.
        let ts = record("PRT0818:00 1DA0:8007");
        assert!(
            mask_bit(&ts.abs, ABS_MT_SLOT),
            "it really does have MT slots"
        );
        assert!(mask_bit(&ts.props, INPUT_PROP_DIRECT));
        assert!(!is_touchpad(&ts));
    }

    #[test]
    fn a_graphics_tablet_is_not_a_touchpad() {
        let tablet = InputRecord {
            name: "Wacom Intuos".into(),
            abs: "3".into(),
            // BTN_TOOL_PEN (0x140) and BTN_TOOL_FINGER (0x145) in word 5.
            key: "21 0 0 0 0 0".into(),
            ..Default::default()
        };
        assert!(mask_bit(&tablet.key, BTN_TOOL_PEN));
        assert!(mask_bit(&tablet.key, BTN_TOOL_FINGER));
        assert!(!is_touchpad(&tablet));
    }

    #[test]
    fn a_clickpad_without_the_tool_finger_bit_still_counts() {
        let pad = InputRecord {
            name: "odd clickpad".into(),
            abs: "3".into(),
            key: "0 10000 0 0 0 0".into(),
            props: "4".into(),
            ..Default::default()
        };
        assert!(!mask_bit(&pad.key, BTN_TOOL_FINGER));
        assert!(is_touchpad(&pad));
    }

    // ── parsing ─────────────────────────────────────────────────────────────

    #[test]
    fn every_record_in_the_real_sample_is_parsed() {
        let recs = parse_input_devices(REAL_PROC);
        assert_eq!(recs.len(), 4);
        assert_eq!(
            recs.iter().map(|r| r.event.as_str()).collect::<Vec<_>>(),
            ["event5", "event6", "event10", "event7"]
        );
    }

    #[test]
    fn a_name_containing_colons_survives() {
        // "ELAN06DA:00 04F3:320B Touchpad" has colons inside the quoted name;
        // splitting on every colon instead of the first would truncate it.
        assert_eq!(
            record("ELAN06DA:00 04F3:320B Touchpad").name,
            "ELAN06DA:00 04F3:320B Touchpad"
        );
    }

    #[test]
    fn the_event_handler_is_picked_out_of_a_crowded_handler_list() {
        let recs = parse_input_devices(
            "I: Bus=0011\nN: Name=\"AT Translated Set 2 keyboard\"\nH: Handlers=sysrq kbd leds event3 \nB: EV=120013\n",
        );
        assert_eq!(recs[0].event, "event3");
    }

    #[test]
    fn a_handler_that_merely_starts_with_event_is_not_an_event_node() {
        let recs = parse_input_devices("I: Bus=1\nN: Name=\"x\"\nH: Handlers=eventful mouse0 \n");
        assert_eq!(recs[0].event, "", "`eventful` is not `eventN`");
    }

    #[test]
    fn a_final_record_without_a_trailing_blank_line_is_kept() {
        let recs = parse_input_devices("I: Bus=1\nN: Name=\"only\"\nH: Handlers=event0");
        assert_eq!(recs.len(), 1);
        assert_eq!(recs[0].name, "only");
    }

    #[test]
    fn malformed_input_yields_no_records_and_does_not_panic() {
        for text in ["", "\n\n\n", "garbage", "N: Name=\"orphan\"\n", ": \n:::\n"] {
            let recs = parse_input_devices(text);
            assert!(
                recs.iter().all(|r| !is_touchpad(r)),
                "{text:?} produced a touchpad"
            );
        }
        assert!(parse_input_devices("").is_empty());
        // A record header with no body is still a record, just an empty one.
        assert_eq!(parse_input_devices("I: Bus=1").len(), 1);
    }

    // ── the rendered report ─────────────────────────────────────────────────

    fn render(records: &[InputRecord]) -> String {
        report_lines(records)
            .iter()
            .map(|(ok, what)| format!("[{}] {what}", if *ok { "PASS" } else { "WARN" }))
            .collect::<Vec<_>>()
            .join("\n")
    }

    #[test]
    fn the_real_machine_reports_one_touchpad_that_can_two_finger_tap() {
        let out = render(&parse_input_devices(REAL_PROC));
        assert_eq!(
            out.matches("touchpad: ").count(),
            1,
            "the touchscreen and the mouse node must not appear"
        );
        assert!(out.contains("ELAN06DA:00 04F3:320B Touchpad (/dev/input/event10,"));
        assert!(out.contains("multitouch slots (ABS_MT_SLOT): present"));
        assert!(out.contains("BTN_TOOL_DOUBLETAP present"));
        assert!(out.contains("clickpad (INPUT_PROP_BUTTONPAD)"));
        assert!(out.contains("=> two-finger tap is possible"));
        assert!(
            !out.contains("[WARN]"),
            "nothing about this device is degraded:\n{out}"
        );
    }

    #[test]
    fn a_single_touch_pad_without_doubletap_is_reported_as_impossible() {
        // No ABS_MT_SLOT and no BTN_TOOL_DOUBLETAP: the kernel can never say
        // "two fingers", so libinput has nothing to build a two-finger tap on.
        let pad = InputRecord {
            name: "Generic PS/2 Clickpad".into(),
            event: "event4".into(),
            sysfs: "/devices/platform/i8042/serio1/input/input4".into(),
            abs: "11000003".into(),
            key: "0 10000 0 0 0 0".into(),
            props: "5".into(),
            ..Default::default()
        };
        let out = render(&[pad]);
        assert!(out.contains("multitouch slots (ABS_MT_SLOT): absent"));
        assert!(out.contains("finger counting: no slots; BTN_TOOL_DOUBLETAP absent"));
        assert!(out.contains("[WARN]   => two-finger tap cannot work on this hardware"));
        assert!(out.contains("pressing the bottom-right corner of the pad"));
    }

    #[test]
    fn a_semi_mt_pad_with_doubletap_can_still_two_finger_tap() {
        // libinput 1.1.5+ stops interpreting semi-mt slot positions and counts
        // fingers from the fake-touch bits instead, so two-finger tap works.
        // Reporting semi-mt as fatal here would send the user hunting a
        // hardware limit that is not the cause.
        let pad = InputRecord {
            name: "SynPS/2 Synaptics TouchPad".into(),
            event: "event4".into(),
            sysfs: "/devices/platform/i8042/serio1/input/input4".into(),
            abs: "260800011000003".into(),
            key: "e420 10000 0 0 0 0".into(),
            props: "d".into(),
            ..Default::default()
        };
        assert!(mask_bit(&pad.props, INPUT_PROP_SEMI_MT));
        let out = render(&[pad]);
        assert!(out.contains("bounding box only (INPUT_PROP_SEMI_MT)"));
        assert!(out.contains("BTN_TOOL_DOUBLETAP present"));
        assert!(out.contains("=> two-finger tap is possible"));
    }

    #[test]
    fn a_semi_mt_pad_without_doubletap_cannot() {
        let pad = InputRecord {
            name: "semi-mt, no fake touches".into(),
            event: "event4".into(),
            abs: "260800011000003".into(),
            key: "20 10000 0 0 0 0".into(),
            props: "d".into(),
            ..Default::default()
        };
        let out = render(&[pad]);
        assert!(out.contains("[WARN]   => two-finger tap cannot work on this hardware"));
    }

    #[test]
    fn a_pad_with_physical_buttons_names_the_right_button_as_the_fallback() {
        let pad = InputRecord {
            name: "old touchpad".into(),
            event: "event4".into(),
            abs: "11000003".into(),
            // BTN_LEFT, BTN_RIGHT, BTN_TOOL_FINGER; no BTN_TOOL_DOUBLETAP.
            key: "20 30000 0 0 0 0".into(),
            props: "0".into(),
            ..Default::default()
        };
        let out = render(&[pad]);
        assert!(out.contains("separate physical buttons (BTN_RIGHT present)"));
        assert!(out.contains("Right click is the physical right button"));
    }

    #[test]
    fn a_machine_with_no_touchpad_says_so_and_names_the_mouse_devices() {
        // The PS/2 fallback case: the pad came up as a plain mouse, which is
        // why tap-to-click and two-finger tap do nothing at all.
        let recs: Vec<InputRecord> = parse_input_devices(REAL_PROC)
            .into_iter()
            .filter(|r| !is_touchpad(r))
            .collect();
        let out = render(&recs);
        assert!(out.starts_with("[WARN] touchpad: none found"));
        assert!(out.contains("TPPS/2 Elan TrackPoint"));
        assert!(out.contains("ELAN06DA:00 04F3:320B Mouse"));
        assert!(out.contains("PS/2 mouse emulation"));
    }

    #[test]
    fn a_machine_with_no_input_devices_at_all_reports_one_plain_line() {
        let out = render(&[]);
        assert_eq!(
            out,
            "[WARN] touchpad: none found, and no pointing device at all — nothing to diagnose"
        );
    }

    #[test]
    fn a_touchpad_missing_its_event_node_still_renders() {
        let pad = InputRecord {
            name: "no handler".into(),
            abs: "2e0800000000003".into(),
            key: "e520 10000 0 0 0 0".into(),
            props: "5".into(),
            ..Default::default()
        };
        let out = render(&[pad]);
        assert!(out.contains("(no event node, sysfs path unknown)"));
    }

    #[test]
    fn doctor_lines_never_panics_on_this_machine() {
        // Reads the live /proc and /sys as an ordinary user. The content is
        // machine-dependent, so only the invariants are asserted.
        let lines = doctor_lines();
        assert!(!lines.is_empty(), "the section must always say something");
        assert!(lines
            .iter()
            .all(|(_, what)| what.starts_with("touchpad: ") || what.starts_with("  ")));
    }
}

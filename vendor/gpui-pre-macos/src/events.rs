use gpui::{
    Capslock, KeyDownEvent, KeyUpEvent, Keystroke, Modifiers, ModifiersChangedEvent, MouseButton,
    MouseDownEvent, MouseExitEvent, MouseMoveEvent, MousePressureEvent, MouseUpEvent,
    NavigationDirection, PinchEvent, Pixels, PlatformInput, PressureStage, ScrollDelta,
    ScrollWheelEvent, TouchPhase, point, px,
};

use crate::{
    LMGetKbdType, NSStringExt, TISCopyCurrentKeyboardInputSource,
    TISCopyCurrentKeyboardLayoutInputSource, TISGetInputSourceProperty, UCKeyTranslate,
    kTISPropertyInputSourceID, kTISPropertyUnicodeKeyLayoutData,
};
use cocoa::{
    appkit::{NSEvent, NSEventModifierFlags, NSEventPhase, NSEventType},
    base::{YES, id},
};
use core_foundation::data::{CFDataGetBytePtr, CFDataRef};
use core_graphics::event::CGKeyCode;
use objc::{msg_send, sel, sel_impl};
use std::{borrow::Cow, cell::Cell, ffi::c_void};

thread_local! {
    // Other GPUI consumers retain native Option behavior unless they opt in.
    static OPTION_AS_ALT: Cell<Option<OptionAsAlt>> = const { Cell::new(Some(OptionAsAlt::None)) };
}

/// Which Option keys act as terminal Alt rather than native text input.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OptionAsAlt {
    None,
    Both,
    Left,
    Right,
}

impl OptionAsAlt {
    pub fn uses_alt(self, side: Option<gpui::OptionSide>) -> bool {
        match self {
            Self::None => false,
            Self::Both => true,
            Self::Left => matches!(side, Some(gpui::OptionSide::Left | gpui::OptionSide::Both)),
            Self::Right => matches!(side, Some(gpui::OptionSide::Right | gpui::OptionSide::Both)),
        }
    }
}

/// `None` follows the keyboard layout; `Some` selects the Option keys to use as Alt.
pub fn set_option_as_alt(value: Option<OptionAsAlt>) {
    OPTION_AS_ALT.set(value);
}

fn layout_uses_option_as_alt(id: &str) -> bool {
    matches!(id, "com.apple.keylayout.US" | "com.apple.keylayout.USInternational" | "com.apple.keylayout.ABC" | "com.apple.keylayout.ABC-India")
}

/// Resolved against the current layout so input-source switches take effect immediately.
pub fn option_as_alt() -> OptionAsAlt {
    OPTION_AS_ALT.get().unwrap_or_else(|| unsafe {
        // The active source, not an IME's underlying ASCII keyboard layout.
        let source = TISCopyCurrentKeyboardInputSource();
        if source.is_null() {
            return OptionAsAlt::None;
        }
        let id = TISGetInputSourceProperty(source, kTISPropertyInputSourceID as *const c_void);
        let enabled = !id.is_null() && layout_uses_option_as_alt(id.to_str());
        let _: () = msg_send![source, release];
        if enabled { OptionAsAlt::Both } else { OptionAsAlt::None }
    })
}

fn option_side(modifiers: NSEventModifierFlags) -> Option<gpui::OptionSide> {
    // NX_DEVICELALTKEYMASK and NX_DEVICERALTKEYMASK from IOLLEvent.h.
    match (modifiers.bits() & 0x20 != 0, modifiers.bits() & 0x40 != 0) {
        (true, true) => Some(gpui::OptionSide::Both),
        (true, false) => Some(gpui::OptionSide::Left),
        (false, true) => Some(gpui::OptionSide::Right),
        (false, false) => None,
    }
}

const BACKSPACE_KEY: u16 = 0x7f;
const SPACE_KEY: u16 = b' ' as u16;
const ENTER_KEY: u16 = 0x0d;
const NUMPAD_ENTER_KEY: u16 = 0x03;
pub(crate) const ESCAPE_KEY: u16 = 0x1b;
const TAB_KEY: u16 = 0x09;
const SHIFT_TAB_KEY: u16 = 0x19;

pub fn key_to_native(key: &str) -> Cow<'_, str> {
    use cocoa::appkit::*;
    let code = match key {
        "space" => SPACE_KEY,
        "backspace" => BACKSPACE_KEY,
        "escape" => ESCAPE_KEY,
        "up" => NSUpArrowFunctionKey,
        "down" => NSDownArrowFunctionKey,
        "left" => NSLeftArrowFunctionKey,
        "right" => NSRightArrowFunctionKey,
        "pageup" => NSPageUpFunctionKey,
        "pagedown" => NSPageDownFunctionKey,
        "home" => NSHomeFunctionKey,
        "end" => NSEndFunctionKey,
        "delete" => NSDeleteFunctionKey,
        "insert" => NSHelpFunctionKey,
        "f1" => NSF1FunctionKey,
        "f2" => NSF2FunctionKey,
        "f3" => NSF3FunctionKey,
        "f4" => NSF4FunctionKey,
        "f5" => NSF5FunctionKey,
        "f6" => NSF6FunctionKey,
        "f7" => NSF7FunctionKey,
        "f8" => NSF8FunctionKey,
        "f9" => NSF9FunctionKey,
        "f10" => NSF10FunctionKey,
        "f11" => NSF11FunctionKey,
        "f12" => NSF12FunctionKey,
        "f13" => NSF13FunctionKey,
        "f14" => NSF14FunctionKey,
        "f15" => NSF15FunctionKey,
        "f16" => NSF16FunctionKey,
        "f17" => NSF17FunctionKey,
        "f18" => NSF18FunctionKey,
        "f19" => NSF19FunctionKey,
        "f20" => NSF20FunctionKey,
        "f21" => NSF21FunctionKey,
        "f22" => NSF22FunctionKey,
        "f23" => NSF23FunctionKey,
        "f24" => NSF24FunctionKey,
        "f25" => NSF25FunctionKey,
        "f26" => NSF26FunctionKey,
        "f27" => NSF27FunctionKey,
        "f28" => NSF28FunctionKey,
        "f29" => NSF29FunctionKey,
        "f30" => NSF30FunctionKey,
        "f31" => NSF31FunctionKey,
        "f32" => NSF32FunctionKey,
        "f33" => NSF33FunctionKey,
        "f34" => NSF34FunctionKey,
        "f35" => NSF35FunctionKey,
        _ => return Cow::Borrowed(key),
    };
    Cow::Owned(String::from_utf16(&[code]).unwrap())
}

unsafe fn read_modifiers(native_event: id) -> Modifiers {
    unsafe {
        let modifiers = native_event.modifierFlags();
        let control = modifiers.contains(NSEventModifierFlags::NSControlKeyMask);
        let alt = modifiers.contains(NSEventModifierFlags::NSAlternateKeyMask);
        let shift = modifiers.contains(NSEventModifierFlags::NSShiftKeyMask);
        let command = modifiers.contains(NSEventModifierFlags::NSCommandKeyMask);
        let function = modifiers.contains(NSEventModifierFlags::NSFunctionKeyMask);

        Modifiers {
            control,
            alt,
            shift,
            platform: command,
            function,
        }
    }
}

pub(crate) unsafe fn platform_input_from_native(
    native_event: id,
    window_height: Option<Pixels>,
) -> Option<PlatformInput> {
    unsafe {
        let event_type = native_event.eventType();

        // Filter out event types that aren't in the NSEventType enum.
        // See https://github.com/servo/cocoa-rs/issues/155#issuecomment-323482792 for details.
        match event_type as u64 {
            0 | 21 | 32 | 33 | 35 | 36 | 37 => {
                return None;
            }
            _ => {}
        }

        match event_type {
            NSEventType::NSFlagsChanged => {
                Some(PlatformInput::ModifiersChanged(ModifiersChangedEvent {
                    modifiers: read_modifiers(native_event),
                    capslock: Capslock {
                        on: native_event
                            .modifierFlags()
                            .contains(NSEventModifierFlags::NSAlphaShiftKeyMask),
                    },
                }))
            }
            NSEventType::NSKeyDown => Some(PlatformInput::KeyDown(KeyDownEvent {
                keystroke: parse_keystroke(native_event),
                is_held: native_event.isARepeat() == YES,
                prefer_character_input: false,
            })),
            NSEventType::NSKeyUp => Some(PlatformInput::KeyUp(KeyUpEvent {
                keystroke: parse_keystroke(native_event),
            })),
            NSEventType::NSLeftMouseDown
            | NSEventType::NSRightMouseDown
            | NSEventType::NSOtherMouseDown => {
                let button = match native_event.buttonNumber() {
                    0 => MouseButton::Left,
                    1 => MouseButton::Right,
                    2 => MouseButton::Middle,
                    3 => MouseButton::Navigate(NavigationDirection::Back),
                    4 => MouseButton::Navigate(NavigationDirection::Forward),
                    // Other mouse buttons aren't tracked currently
                    _ => return None,
                };
                window_height.map(|window_height| {
                    PlatformInput::MouseDown(MouseDownEvent {
                        button,
                        position: point(
                            px(native_event.locationInWindow().x as f32),
                            // MacOS screen coordinates are relative to bottom left
                            window_height - px(native_event.locationInWindow().y as f32),
                        ),
                        modifiers: read_modifiers(native_event),
                        click_count: native_event.clickCount() as usize,
                        first_mouse: false,
                    })
                })
            }
            NSEventType::NSLeftMouseUp
            | NSEventType::NSRightMouseUp
            | NSEventType::NSOtherMouseUp => {
                let button = match native_event.buttonNumber() {
                    0 => MouseButton::Left,
                    1 => MouseButton::Right,
                    2 => MouseButton::Middle,
                    3 => MouseButton::Navigate(NavigationDirection::Back),
                    4 => MouseButton::Navigate(NavigationDirection::Forward),
                    // Other mouse buttons aren't tracked currently
                    _ => return None,
                };

                window_height.map(|window_height| {
                    PlatformInput::MouseUp(MouseUpEvent {
                        button,
                        position: point(
                            px(native_event.locationInWindow().x as f32),
                            window_height - px(native_event.locationInWindow().y as f32),
                        ),
                        modifiers: read_modifiers(native_event),
                        click_count: native_event.clickCount() as usize,
                    })
                })
            }
            NSEventType::NSEventTypePressure => {
                let stage = native_event.stage();
                let pressure = native_event.pressure();

                window_height.map(|window_height| {
                    PlatformInput::MousePressure(MousePressureEvent {
                        stage: match stage {
                            1 => PressureStage::Normal,
                            2 => PressureStage::Force,
                            _ => PressureStage::Zero,
                        },
                        pressure,
                        modifiers: read_modifiers(native_event),
                        position: point(
                            px(native_event.locationInWindow().x as f32),
                            window_height - px(native_event.locationInWindow().y as f32),
                        ),
                    })
                })
            }
            // Some mice (like Logitech MX Master) send navigation buttons as swipe events
            NSEventType::NSEventTypeSwipe => {
                let navigation_direction = match native_event.phase() {
                    NSEventPhase::NSEventPhaseEnded => match native_event.deltaX() {
                        x if x > 0.0 => Some(NavigationDirection::Back),
                        x if x < 0.0 => Some(NavigationDirection::Forward),
                        _ => return None,
                    },
                    _ => return None,
                };

                match navigation_direction {
                    Some(direction) => window_height.map(|window_height| {
                        PlatformInput::MouseDown(MouseDownEvent {
                            button: MouseButton::Navigate(direction),
                            position: point(
                                px(native_event.locationInWindow().x as f32),
                                window_height - px(native_event.locationInWindow().y as f32),
                            ),
                            modifiers: read_modifiers(native_event),
                            click_count: 1,
                            first_mouse: false,
                        })
                    }),
                    _ => None,
                }
            }
            NSEventType::NSEventTypeMagnify => window_height.map(|window_height| {
                let phase = match native_event.phase() {
                    NSEventPhase::NSEventPhaseMayBegin | NSEventPhase::NSEventPhaseBegan => {
                        TouchPhase::Started
                    }
                    NSEventPhase::NSEventPhaseEnded => TouchPhase::Ended,
                    _ => TouchPhase::Moved,
                };

                let magnification = native_event.magnification() as f32;

                PlatformInput::Pinch(PinchEvent {
                    position: point(
                        px(native_event.locationInWindow().x as f32),
                        window_height - px(native_event.locationInWindow().y as f32),
                    ),
                    delta: magnification,
                    modifiers: read_modifiers(native_event),
                    phase,
                })
            }),
            NSEventType::NSScrollWheel => window_height.map(|window_height| {
                let phase = match native_event.phase() {
                    NSEventPhase::NSEventPhaseMayBegin | NSEventPhase::NSEventPhaseBegan => {
                        TouchPhase::Started
                    }
                    NSEventPhase::NSEventPhaseEnded => TouchPhase::Ended,
                    _ => TouchPhase::Moved,
                };

                let raw_data = point(
                    native_event.scrollingDeltaX() as f32,
                    native_event.scrollingDeltaY() as f32,
                );

                let delta = if native_event.hasPreciseScrollingDeltas() == YES {
                    ScrollDelta::Pixels(raw_data.map(px))
                } else {
                    ScrollDelta::Lines(raw_data)
                };

                PlatformInput::ScrollWheel(ScrollWheelEvent {
                    position: point(
                        px(native_event.locationInWindow().x as f32),
                        window_height - px(native_event.locationInWindow().y as f32),
                    ),
                    delta,
                    touch_phase: phase,
                    modifiers: read_modifiers(native_event),
                })
            }),
            NSEventType::NSLeftMouseDragged
            | NSEventType::NSRightMouseDragged
            | NSEventType::NSOtherMouseDragged => {
                let pressed_button = match native_event.buttonNumber() {
                    0 => MouseButton::Left,
                    1 => MouseButton::Right,
                    2 => MouseButton::Middle,
                    3 => MouseButton::Navigate(NavigationDirection::Back),
                    4 => MouseButton::Navigate(NavigationDirection::Forward),
                    // Other mouse buttons aren't tracked currently
                    _ => return None,
                };

                window_height.map(|window_height| {
                    PlatformInput::MouseMove(MouseMoveEvent {
                        pressed_button: Some(pressed_button),
                        position: point(
                            px(native_event.locationInWindow().x as f32),
                            window_height - px(native_event.locationInWindow().y as f32),
                        ),
                        modifiers: read_modifiers(native_event),
                    })
                })
            }
            NSEventType::NSMouseMoved => window_height.map(|window_height| {
                PlatformInput::MouseMove(MouseMoveEvent {
                    position: point(
                        px(native_event.locationInWindow().x as f32),
                        window_height - px(native_event.locationInWindow().y as f32),
                    ),
                    pressed_button: None,
                    modifiers: read_modifiers(native_event),
                })
            }),
            NSEventType::NSMouseExited => window_height.map(|window_height| {
                PlatformInput::MouseExited(MouseExitEvent {
                    position: point(
                        px(native_event.locationInWindow().x as f32),
                        window_height - px(native_event.locationInWindow().y as f32),
                    ),

                    pressed_button: None,
                    modifiers: read_modifiers(native_event),
                })
            }),
            _ => None,
        }
    }
}

unsafe fn parse_keystroke(native_event: id) -> Keystroke {
    unsafe {
        use cocoa::appkit::*;

        let characters = native_event
            .charactersIgnoringModifiers()
            .to_str()
            .to_string();
        let mut key_char = None;
        let mut unshifted_key = None;
        let first_char = characters.chars().next().map(|ch| ch as u16);
        let modifiers = native_event.modifierFlags();

        let control = modifiers.contains(NSEventModifierFlags::NSControlKeyMask);
        let alt = modifiers.contains(NSEventModifierFlags::NSAlternateKeyMask);
        let mut shift = modifiers.contains(NSEventModifierFlags::NSShiftKeyMask);
        let command = modifiers.contains(NSEventModifierFlags::NSCommandKeyMask);
        let function = modifiers.contains(NSEventModifierFlags::NSFunctionKeyMask)
            && first_char
                .is_none_or(|ch| !(NSUpArrowFunctionKey..=NSModeSwitchFunctionKey).contains(&ch));

        #[allow(non_upper_case_globals)]
        let key = match first_char {
            Some(SPACE_KEY) => {
                key_char = Some(" ".to_string());
                "space".to_string()
            }
            Some(TAB_KEY) => {
                key_char = Some("\t".to_string());
                "tab".to_string()
            }
            Some(ENTER_KEY) | Some(NUMPAD_ENTER_KEY) => {
                key_char = Some("\n".to_string());
                "enter".to_string()
            }
            Some(BACKSPACE_KEY) => "backspace".to_string(),
            Some(ESCAPE_KEY) => "escape".to_string(),
            Some(SHIFT_TAB_KEY) => "tab".to_string(),
            Some(NSUpArrowFunctionKey) => "up".to_string(),
            Some(NSDownArrowFunctionKey) => "down".to_string(),
            Some(NSLeftArrowFunctionKey) => "left".to_string(),
            Some(NSRightArrowFunctionKey) => "right".to_string(),
            Some(NSPageUpFunctionKey) => "pageup".to_string(),
            Some(NSPageDownFunctionKey) => "pagedown".to_string(),
            Some(NSHomeFunctionKey) => "home".to_string(),
            Some(NSEndFunctionKey) => "end".to_string(),
            Some(NSDeleteFunctionKey) => "delete".to_string(),
            // Observed Insert==NSHelpFunctionKey not NSInsertFunctionKey.
            Some(NSHelpFunctionKey) => "insert".to_string(),
            Some(NSF1FunctionKey) => "f1".to_string(),
            Some(NSF2FunctionKey) => "f2".to_string(),
            Some(NSF3FunctionKey) => "f3".to_string(),
            Some(NSF4FunctionKey) => "f4".to_string(),
            Some(NSF5FunctionKey) => "f5".to_string(),
            Some(NSF6FunctionKey) => "f6".to_string(),
            Some(NSF7FunctionKey) => "f7".to_string(),
            Some(NSF8FunctionKey) => "f8".to_string(),
            Some(NSF9FunctionKey) => "f9".to_string(),
            Some(NSF10FunctionKey) => "f10".to_string(),
            Some(NSF11FunctionKey) => "f11".to_string(),
            Some(NSF12FunctionKey) => "f12".to_string(),
            Some(NSF13FunctionKey) => "f13".to_string(),
            Some(NSF14FunctionKey) => "f14".to_string(),
            Some(NSF15FunctionKey) => "f15".to_string(),
            Some(NSF16FunctionKey) => "f16".to_string(),
            Some(NSF17FunctionKey) => "f17".to_string(),
            Some(NSF18FunctionKey) => "f18".to_string(),
            Some(NSF19FunctionKey) => "f19".to_string(),
            Some(NSF20FunctionKey) => "f20".to_string(),
            Some(NSF21FunctionKey) => "f21".to_string(),
            Some(NSF22FunctionKey) => "f22".to_string(),
            Some(NSF23FunctionKey) => "f23".to_string(),
            Some(NSF24FunctionKey) => "f24".to_string(),
            Some(NSF25FunctionKey) => "f25".to_string(),
            Some(NSF26FunctionKey) => "f26".to_string(),
            Some(NSF27FunctionKey) => "f27".to_string(),
            Some(NSF28FunctionKey) => "f28".to_string(),
            Some(NSF29FunctionKey) => "f29".to_string(),
            Some(NSF30FunctionKey) => "f30".to_string(),
            Some(NSF31FunctionKey) => "f31".to_string(),
            Some(NSF32FunctionKey) => "f32".to_string(),
            Some(NSF33FunctionKey) => "f33".to_string(),
            Some(NSF34FunctionKey) => "f34".to_string(),
            Some(NSF35FunctionKey) => "f35".to_string(),
            _ => {
                // Cases to test when modifying this:
                //
                //           qwerty key | none | cmd   | cmd-shift
                // * Armenian         s | ս    | cmd-s | cmd-shift-s  (layout is non-ASCII, so we use cmd layout)
                // * Dvorak+QWERTY    s | o    | cmd-s | cmd-shift-s  (layout switches on cmd)
                // * Ukrainian+QWERTY s | с    | cmd-s | cmd-shift-s  (macOS reports cmd-s instead of cmd-S)
                // * Czech            7 | ý    | cmd-ý | cmd-7        (layout has shifted numbers)
                // * Norwegian        7 | 7    | cmd-7 | cmd-/        (macOS reports cmd-shift-7 instead of cmd-/)
                // * Russian          7 | 7    | cmd-7 | cmd-&        (shift-7 is . but when cmd is down, should use cmd layout)
                // * German QWERTZ    ; | ö    | cmd-ö | cmd-Ö        (Zed's shift special case only applies to a-z)
                //
                let mut chars_ignoring_modifiers =
                    chars_for_modified_key(native_event.keyCode(), NO_MOD);
                let mut chars_with_shift =
                    chars_for_modified_key(native_event.keyCode(), SHIFT_MOD);
                let always_use_cmd_layout = always_use_command_layout();

                // Handle Dvorak+QWERTY / Russian / Armenian
                if command || always_use_cmd_layout {
                    let chars_with_cmd = chars_for_modified_key(native_event.keyCode(), CMD_MOD);
                    let chars_with_both =
                        chars_for_modified_key(native_event.keyCode(), CMD_MOD | SHIFT_MOD);

                    // We don't do this in the case that the shifted command key generates
                    // the same character as the unshifted command key (Norwegian, e.g.)
                    if chars_with_both != chars_with_cmd {
                        chars_with_shift = chars_with_both;

                    // Handle edge-case where cmd-shift-s reports cmd-s instead of
                    // cmd-shift-s (Ukrainian, etc.)
                    } else if chars_with_cmd.to_ascii_uppercase() != chars_with_cmd {
                        chars_with_shift = chars_with_cmd.to_ascii_uppercase();
                    }
                    chars_ignoring_modifiers = chars_with_cmd;
                }

                // Fn can accompany printable keys from programmable keyboards.
                if !control && !command {
                    let caps_lock = modifiers.contains(NSEventModifierFlags::NSAlphaShiftKeyMask);
                    let mods = text_modifiers(shift, alt && !option_as_alt().uses_alt(option_side(modifiers)), caps_lock);

                    // Keep dead keys for NSTextInputClient instead of sending their
                    // spacing accent to key-down listeners before composition starts.
                    let text = translate_key(native_event.keyCode(), mods, false);
                    // Input methods post finished text on a key code that types
                    // something else; NSTextInputClient types the event's own text.
                    // Option is left out: as Alt it leaves mods but not the event's text.
                    let event_text = native_event.characters();
                    let event_text = event_text.to_str();
                    let posted = !alt && !event_text.is_empty() && event_text != text;
                    key_char = (!text.is_empty() && !posted).then_some(text);
                }

                if shift
                    && chars_ignoring_modifiers
                        .chars()
                        .all(|c| c.is_ascii_lowercase())
                {
                    chars_ignoring_modifiers
                } else if shift {
                    shift = false;
                    if !chars_ignoring_modifiers.is_empty() {
                        unshifted_key = Some(chars_ignoring_modifiers);
                    }
                    chars_with_shift
                } else {
                    chars_ignoring_modifiers
                }
            }
        };

        Keystroke {
            modifiers: Modifiers {
                control,
                alt,
                shift,
                platform: command,
                function,
            },
            key,
            key_char,
            unshifted_key,
            option_side: option_side(modifiers),
        }
    }
}

fn always_use_command_layout() -> bool {
    if chars_for_modified_key(0, NO_MOD).is_ascii() {
        return false;
    }

    chars_for_modified_key(0, CMD_MOD).is_ascii()
}

const NO_MOD: u32 = 0;
const CMD_MOD: u32 = 1;
const SHIFT_MOD: u32 = 2;
const OPTION_MOD: u32 = 8;
const CAPS_LOCK_MOD: u32 = 4;

/// The modifiers that change the text a key types. Caps Lock changes the
/// text, though not the key.
fn text_modifiers(shift: bool, alt: bool, caps_lock: bool) -> u32 {
    let mut mods = NO_MOD;
    if shift {
        mods |= SHIFT_MOD;
    }
    if alt {
        mods |= OPTION_MOD;
    }
    if caps_lock {
        mods |= CAPS_LOCK_MOD;
    }
    mods
}

fn chars_for_modified_key(code: CGKeyCode, modifiers: u32) -> String {
    translate_key(code, modifiers, true)
}

fn translate_key(code: CGKeyCode, modifiers: u32, resolve_dead_keys: bool) -> String {
    // Values from: https://github.com/phracker/MacOSX-SDKs/blob/master/MacOSX10.6.sdk/System/Library/Frameworks/Carbon.framework/Versions/A/Frameworks/HIToolbox.framework/Versions/A/Headers/Events.h#L126
    // shifted >> 8 for UCKeyTranslate
    const CG_SPACE_KEY: u16 = 49;
    // https://github.com/phracker/MacOSX-SDKs/blob/master/MacOSX10.6.sdk/System/Library/Frameworks/CoreServices.framework/Versions/A/Frameworks/CarbonCore.framework/Versions/A/Headers/UnicodeUtilities.h#L278
    #[allow(non_upper_case_globals)]
    const kUCKeyActionDown: u16 = 0;
    #[allow(non_upper_case_globals)]
    const kUCKeyTranslateNoDeadKeysMask: u32 = 0;

    let keyboard_type = unsafe { LMGetKbdType() as u32 };
    const BUFFER_SIZE: usize = 4;
    let mut dead_key_state = 0;
    let mut buffer: [u16; BUFFER_SIZE] = [0; BUFFER_SIZE];
    let mut buffer_size: usize = 0;

    let keyboard = unsafe { TISCopyCurrentKeyboardLayoutInputSource() };
    if keyboard.is_null() {
        return "".to_string();
    }
    let layout_data = unsafe {
        TISGetInputSourceProperty(keyboard, kTISPropertyUnicodeKeyLayoutData as *const c_void)
            as CFDataRef
    };
    if layout_data.is_null() {
        unsafe {
            let _: () = msg_send![keyboard, release];
        }
        return "".to_string();
    }
    let keyboard_layout = unsafe { CFDataGetBytePtr(layout_data) };

    unsafe {
        UCKeyTranslate(
            keyboard_layout as *const c_void,
            code,
            kUCKeyActionDown,
            modifiers,
            keyboard_type,
            kUCKeyTranslateNoDeadKeysMask,
            &mut dead_key_state,
            BUFFER_SIZE,
            &mut buffer_size as *mut usize,
            &mut buffer as *mut u16,
        );
        if resolve_dead_keys && dead_key_state != 0 {
            UCKeyTranslate(
                keyboard_layout as *const c_void,
                CG_SPACE_KEY,
                kUCKeyActionDown,
                modifiers,
                keyboard_type,
                kUCKeyTranslateNoDeadKeysMask,
                &mut dead_key_state,
                BUFFER_SIZE,
                &mut buffer_size as *mut usize,
                &mut buffer as *mut u16,
            );
        }
        let _: () = msg_send![keyboard, release];
    }
    String::from_utf16(&buffer[..buffer_size]).unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    // AppKit's synthetic event creation must not race between tests.
    static NATIVE_KEY_TEST: std::sync::Mutex<()> = std::sync::Mutex::new(());

    fn native_key(code: u16, modifiers: NSEventModifierFlags) -> Keystroke {
        native_key_with_text(code, modifiers, &chars_for_modified_key(code, SHIFT_MOD))
    }

    fn native_key_with_text(code: u16, modifiers: NSEventModifierFlags, text: &str) -> Keystroke {
        use cocoa::{
            base::{nil, NO},
            foundation::{NSAutoreleasePool, NSPoint, NSString},
        };
        unsafe {
            let pool = NSAutoreleasePool::new(nil);
            let chars = NSString::alloc(nil).init_str(text);
            let event = NSEvent::keyEventWithType_location_modifierFlags_timestamp_windowNumber_context_characters_charactersIgnoringModifiers_isARepeat_keyCode_(
                nil, NSEventType::NSKeyDown, NSPoint::new(0., 0.), modifiers,
                0., 0, nil, chars, chars, NO, code,
            );
            let keystroke = parse_keystroke(event);
            let _: () = msg_send![chars, release];
            pool.drain();
            keystroke
        }
    }

    #[test]
    fn shifted_punctuation_matches_base_key_and_shifted_symbol() {
        use gpui::KeybindingKeystroke;
        let _guard = NATIVE_KEY_TEST.lock().unwrap();
        let mut checked = 0;
        // Brackets, equals, minus, and semicolon on a US keyboard. Translation
        // follows the active layout without changing the user's input source.
        for code in [30, 33, 24, 27, 41] {
            let base = chars_for_modified_key(code, CMD_MOD);
            let mut shifted = chars_for_modified_key(code, CMD_MOD | SHIFT_MOD);
            if shifted == base {
                shifted = chars_for_modified_key(code, SHIFT_MOD);
            }
            if base.is_empty() || base == shifted || base.chars().all(|c| c.is_ascii_lowercase()) {
                continue;
            }
            let event = native_key(code, NSEventModifierFlags::NSCommandKeyMask | NSEventModifierFlags::NSShiftKeyMask);
            assert_eq!(event.key, shifted);
            assert!(!event.modifiers.shift);
            assert_eq!(event.key_char, None);
            for binding in [format!("cmd-shift-{base}"), format!("cmd-{shifted}")] {
                let target = KeybindingKeystroke::from_keystroke(Keystroke::parse(&binding).unwrap());
                assert!(event.should_match(&target), "{event:?} should match {binding}");
            }
            let unshifted = KeybindingKeystroke::from_keystroke(Keystroke::parse(&format!("cmd-{base}")).unwrap());
            assert!(!event.should_match(&unshifted));
            checked += 1;
        }
        assert!(checked > 0, "active layout must have a shifted punctuation key");
    }

    #[test]
    fn shifted_alias_preserves_text_and_other_modifiers() {
        use gpui::KeybindingKeystroke;
        let _guard = NATIVE_KEY_TEST.lock().unwrap();
        set_option_as_alt(Some(OptionAsAlt::None));
        for code in [30, 33, 24, 27] {
            let event = native_key(code, NSEventModifierFlags::NSShiftKeyMask);
            let text = translate_key(code, SHIFT_MOD, false);
            assert_eq!(event.key_char, (!text.is_empty()).then_some(text));

            let event = native_key(code, NSEventModifierFlags::NSShiftKeyMask | NSEventModifierFlags::NSAlternateKeyMask | NSEventModifierFlags::NSFunctionKeyMask);
            assert!(event.modifiers.alt);
            assert!(event.modifiers.function);
            let text = translate_key(code, SHIFT_MOD | OPTION_MOD, false);
            assert_eq!(event.key_char, (!text.is_empty()).then_some(text));
            if let Some(base) = &event.unshifted_key {
                let binding = format!("alt-fn-shift-{base}");
                let target = KeybindingKeystroke::from_keystroke(Keystroke::parse(&binding).unwrap());
                assert!(event.should_match(&target));
                let target = KeybindingKeystroke::from_keystroke(Keystroke::parse(&format!("shift-{base}")).unwrap());
                assert!(!event.should_match(&target));
            }
        }
        let event = native_key(30, NSEventModifierFlags::NSCommandKeyMask);
        assert_eq!(event.unshifted_key, None);
    }

    #[test]
    fn automatic_option_as_alt_follows_us_layouts() {
        for id in ["US", "USInternational", "ABC", "ABC-India"] {
            assert!(layout_uses_option_as_alt(&format!("com.apple.keylayout.{id}")));
        }
        for id in ["Turkish-Q-PC", "German", "ABC-QWERTZ", "ABC-AZERTY", "ABC-other", "ABCother", ""] {
            assert!(!layout_uses_option_as_alt(&format!("com.apple.keylayout.{id}")));
        }
        assert!(!layout_uses_option_as_alt("com.apple.inputmethod.Kotoeri.RomajiTyping.Japanese"));
    }

    #[test]
    fn explicit_option_as_alt_overrides_layout_detection() {
        for value in [OptionAsAlt::None, OptionAsAlt::Both, OptionAsAlt::Left, OptionAsAlt::Right] {
            set_option_as_alt(Some(value));
            assert_eq!(option_as_alt(), value);
        }
        set_option_as_alt(Some(OptionAsAlt::None));
    }

    #[test]
    fn option_sides_control_text_without_changing_shortcuts() {
        use gpui::{KeybindingKeystroke, OptionSide};
        let _guard = NATIVE_KEY_TEST.lock().unwrap();
        let alt = NSEventModifierFlags::NSAlternateKeyMask;
        for (bits, side) in [(0, None), (0x20, Some(OptionSide::Left)), (0x40, Some(OptionSide::Right)), (0x60, Some(OptionSide::Both))] {
            let flags = alt | NSEventModifierFlags::from_bits_retain(bits);
            assert_eq!(option_side(flags), side);
            for setting in [OptionAsAlt::None, OptionAsAlt::Both, OptionAsAlt::Left, OptionAsAlt::Right] {
                set_option_as_alt(Some(setting));
                let as_alt = match setting {
                    OptionAsAlt::None => false,
                    OptionAsAlt::Both => true,
                    OptionAsAlt::Left => bits & 0x20 != 0,
                    OptionAsAlt::Right => bits & 0x40 != 0,
                };
                assert_eq!(setting.uses_alt(side), as_alt);
                for code in [0x23, 0x0e] { // P and the E dead key on US layouts.
                    let event = native_key(code, flags);
                    assert_eq!(event.option_side, side);
                    let text = translate_key(code, if as_alt { NO_MOD } else { OPTION_MOD }, false);
                    assert_eq!(event.key_char, (!text.is_empty()).then_some(text));
                    let target = KeybindingKeystroke::from_keystroke(Keystroke::parse(&format!("alt-{}", event.key)).unwrap());
                    assert!(event.should_match(&target));
                    let mut other = event.clone();
                    other.option_side = None;
                    assert_eq!(event, other);
                    assert_eq!(gpui::hash(&event), gpui::hash(&other));
                }
            }
        }
        set_option_as_alt(Some(OptionAsAlt::None));
    }

    #[test]
    fn caps_lock_goes_into_the_typed_text() {
        assert_eq!(text_modifiers(false, false, false), NO_MOD);
        assert_eq!(text_modifiers(false, false, true), CAPS_LOCK_MOD);
        assert_eq!(text_modifiers(true, false, true), SHIFT_MOD | CAPS_LOCK_MOD);
        assert_eq!(text_modifiers(false, true, true), OPTION_MOD | CAPS_LOCK_MOD);
    }

    #[test]
    fn posted_text_is_left_to_text_input() {
        let _guard = NATIVE_KEY_TEST.lock().unwrap();
        let none = NSEventModifierFlags::empty();
        // Input methods post finished text on a key code that types something else.
        assert_eq!(native_key_with_text(0x00, none, "ê").key_char, None);
        // A key code no keyboard sends.
        assert_eq!(native_key_with_text(0x6C, none, "ê").key_char, None);
        // The space bar.
        assert_eq!(native_key_with_text(0x31, none, "ê").key_char, None);
        assert_eq!(native_key_with_text(0x00, NSEventModifierFlags::NSShiftKeyMask, "Ê").key_char, None);
        assert_eq!(native_key_with_text(0x00, none, "ường").key_char, None);
    }

    #[test]
    fn typed_text_keeps_its_key_char() {
        let _guard = NATIVE_KEY_TEST.lock().unwrap();
        set_option_as_alt(Some(OptionAsAlt::None));
        for (flags, mods) in [
            (NSEventModifierFlags::empty(), NO_MOD),
            (NSEventModifierFlags::NSShiftKeyMask, SHIFT_MOD),
            (NSEventModifierFlags::NSAlphaShiftKeyMask, CAPS_LOCK_MOD),
        ] {
            for code in 0..=50 {
                let text = translate_key(code, mods, false);
                // Return and other control keys take the named-key branch.
                if text.is_empty() || text.chars().any(char::is_control) {
                    continue;
                }
                assert_eq!(native_key_with_text(code, flags, &text).key_char, Some(text), "code {code}");
            }
        }
        // An event without text keeps what its key types.
        let own = translate_key(0x00, NO_MOD, false);
        assert_eq!(native_key_with_text(0x00, NSEventModifierFlags::empty(), "").key_char, Some(own.clone()));
        // Option as Alt drops Option from the key's text but not from the event's characters.
        set_option_as_alt(Some(OptionAsAlt::Both));
        assert_eq!(native_key_with_text(0x00, NSEventModifierFlags::NSAlternateKeyMask, "å").key_char, Some(own));
        set_option_as_alt(Some(OptionAsAlt::None));
    }
}

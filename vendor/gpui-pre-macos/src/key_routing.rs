use gpui::Keystroke;

/// Whether AppKit's input context gets the key before the application does.
pub(crate) fn input_method_first(key: &Keystroke, composing: bool, ime_printable: bool) -> bool {
    // Return/keypad Enter and Tab carry control text ("\n"/"\t"), unlike arrows.
    // IMEs still need these keys: missing Shift+Return makes Rime see a lone Shift
    // and switch language when Shift is released. Unhandled keys use the existing
    // doCommandBySelector/application fallback.
    let non_printing = key
        .key_char
        .as_ref()
        .is_none_or(|text| text.chars().all(char::is_control));
    composing
        || ime_printable
        || (non_printing
            && !key.modifiers.control
            && !key.modifiers.function
            && !key.modifiers.platform)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(binding: &str, text: Option<&str>) -> Keystroke {
        let mut key = Keystroke::parse(binding).unwrap();
        key.key_char = text.map(str::to_owned);
        key
    }

    #[test]
    fn enter_and_tab_reach_the_input_method_without_marked_text() {
        // macOS normalizes Return/keypad Enter to Some("\n") and Tab to Some("\t").
        // Rime must see Shift+Return between Shift down/up to avoid toggling ASCII mode.
        for (binding, text) in [("enter", "\n"), ("shift-enter", "\n"), ("tab", "\t")] {
            assert!(
                input_method_first(&key(binding, Some(text)), false, false),
                "{binding}"
            );
        }
    }

    #[test]
    fn non_printing_navigation_keeps_input_method_priority() {
        for binding in ["shift-tab", "left", "down", "escape", "backspace"] {
            assert!(
                input_method_first(&key(binding, None), false, false),
                "{binding}"
            );
        }
    }

    #[test]
    fn control_function_and_command_shortcuts_keep_application_priority() {
        for (binding, text) in [
            ("ctrl-enter", Some("\n")),
            ("cmd-enter", Some("\n")),
            ("ctrl-tab", Some("\t")),
            ("cmd-`", Some("`")),
            ("fn-enter", Some("\n")),
            ("ctrl-left", None),
        ] {
            assert!(
                !input_method_first(&key(binding, text), false, false),
                "{binding}"
            );
        }
    }

    #[test]
    fn printable_text_retains_its_existing_input_method_policy() {
        let printable = key("j", Some("j"));
        assert!(!input_method_first(&printable, false, false));
        assert!(input_method_first(&printable, false, true));
        assert!(input_method_first(&printable, true, false));
        assert!(!input_method_first(&key("space", Some(" ")), false, false));
    }
}

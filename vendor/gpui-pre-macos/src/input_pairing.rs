// HERDR: when a view's text input context has lost the text input system.

/// What AppKit and the Text Services Manager report for a view, read before a
/// key reaches the input method.
#[derive(Clone, Copy)]
pub(crate) struct InputContext {
    /// The application is active.
    pub app_active: bool,
    /// The view's window is the key window.
    pub window_key: bool,
    /// The view is its window's first responder.
    pub first_responder: bool,
    /// The view's context is AppKit's current input context.
    pub current: bool,
    /// The process has an active Text Services Manager document.
    pub document: bool,
}

/// Whether the view's context must be paired with the text input system again.
///
/// The view that receives the keys owns the current input context, and that
/// context owns the process's active document. Only that view is judged: any
/// other is not the one being typed into, and activating its context would
/// take the pairing from the view that is. A paired context is left alone,
/// because activating it again would end a composition in progress.
pub(crate) fn lost(context: InputContext) -> bool {
    context.app_active
        && context.window_key
        && context.first_responder
        && !(context.current && context.document)
}

#[cfg(test)]
mod tests {
    use super::*;

    const PAIRED: InputContext = InputContext {
        app_active: true,
        window_key: true,
        first_responder: true,
        current: true,
        document: true,
    };

    #[test]
    fn a_paired_context_is_left_alone() {
        assert!(!lost(PAIRED));
    }

    #[test]
    fn a_key_window_without_an_active_document_is_paired_again() {
        // The state behind "nothing composes until the app is reactivated":
        // the context is current, yet no document is active.
        assert!(lost(InputContext { document: false, ..PAIRED }));
    }

    #[test]
    fn a_context_that_is_not_current_is_paired_again() {
        assert!(lost(InputContext { current: false, ..PAIRED }));
        assert!(lost(InputContext { current: false, document: false, ..PAIRED }));
    }

    #[test]
    fn a_view_that_is_not_typed_into_is_left_alone() {
        // In the background, behind a sheet or dialog, or beside another
        // first responder, the missing pairing is normal.
        for context in [
            InputContext { app_active: false, ..PAIRED },
            InputContext { window_key: false, ..PAIRED },
            InputContext { first_responder: false, ..PAIRED },
        ] {
            for (current, document) in [(true, true), (true, false), (false, true), (false, false)] {
                assert!(!lost(InputContext { current, document, ..context }));
            }
        }
    }
}

// HERDR: drain foreground input without losing IME commits.
//
// When the foreground task budget runs out, the upstream loop takes every
// queued input message before anything else. With an IME, a key can queue a
// WM_IME_COMPOSITION carrying the result string. If the next key is taken
// before that notification is handled, the IME starts a new composition and
// ImmGetCompositionStringW(GCS_RESULTSTR) returns 0 bytes: the committed text
// is lost. Handle pending IME notifications before each input message.

pub(crate) enum ForegroundMessageKind {
    Ime,
    Input,
}

/// Drain foreground input while keeping IME notifications ahead of the next key.
pub(crate) fn drain_foreground_input<T>(
    mut peek: impl FnMut(ForegroundMessageKind) -> Option<T>,
    mut dispatch: impl FnMut(T),
) {
    loop {
        while let Some(message) = peek(ForegroundMessageKind::Ime) {
            dispatch(message);
        }
        let Some(message) = peek(ForegroundMessageKind::Input) else {
            break;
        };
        dispatch(message);
    }
}

#[cfg(test)]
mod tests {
    use std::cell::{Cell, RefCell};
    use std::collections::VecDeque;

    use super::{ForegroundMessageKind, drain_foreground_input};

    #[derive(Clone, Copy, Debug, PartialEq)]
    enum Message {
        Key(char),
        ImeResult,
    }

    /// Models the IMM result string: a key queues a composition notification
    /// and the next key overwrites the result if it is read too late.
    #[test]
    fn handles_ime_result_before_the_next_key() {
        let queue = RefCell::new(VecDeque::from([Message::Key(' '), Message::Key('d')]));
        let result = RefCell::new(String::new());
        let committed = RefCell::new(Vec::new());
        let keys = Cell::new(0);

        drain_foreground_input(
            |kind| {
                let mut queue = queue.borrow_mut();
                let index = queue.iter().position(|message| match kind {
                    ForegroundMessageKind::Ime => *message == Message::ImeResult,
                    ForegroundMessageKind::Input => matches!(message, Message::Key(_)),
                })?;
                queue.remove(index)
            },
            |message| match message {
                Message::Key(key) => {
                    keys.set(keys.get() + 1);
                    // The first key commits the conversion; any later key starts
                    // a new composition and clears the pending result.
                    *result.borrow_mut() = if key == ' ' { "今".into() } else { String::new() };
                    if key == ' ' {
                        queue.borrow_mut().push_back(Message::ImeResult);
                    }
                }
                Message::ImeResult => committed.borrow_mut().push(result.borrow().clone()),
            },
        );

        assert_eq!(keys.get(), 2);
        assert_eq!(*committed.borrow(), ["今"]);
    }

    #[test]
    fn stops_when_both_queues_are_empty() {
        let mut calls = 0;
        drain_foreground_input(
            |_| {
                calls += 1;
                None::<()>
            },
            |_| unreachable!(),
        );
        assert_eq!(calls, 2);
    }
}

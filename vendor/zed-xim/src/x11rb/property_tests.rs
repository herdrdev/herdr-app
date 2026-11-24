use super::*;

fn queued_messages(append_during_drain: bool) {
    // Run explicitly on an isolated X server, never the user's display.
    let display =
        std::env::var("XIM_TEST_DISPLAY").expect("set XIM_TEST_DISPLAY to a private display");
    let (conn, screen) = x11rb::connect(Some(&display)).unwrap();
    let window = conn.generate_id().unwrap();
    conn.create_window(
        COPY_DEPTH_FROM_PARENT,
        window,
        conn.setup().roots[screen].root,
        0,
        0,
        1,
        1,
        0,
        WindowClass::INPUT_ONLY,
        0,
        &Default::default(),
    )
    .unwrap()
    .check()
    .unwrap();
    let atom = conn
        .intern_atom(false, b"_HERDR_XIM_DRAIN_TEST")
        .unwrap()
        .reply()
        .unwrap()
        .atom;
    conn.change_property8(
        PropMode::REPLACE,
        window,
        atom,
        AtomEnum::STRING,
        b"AAAABBBB",
    )
    .unwrap()
    .check()
    .unwrap();
    let first = conn
        .get_property(true, window, atom, AtomEnum::ANY, 0, 1)
        .unwrap()
        .reply()
        .unwrap();
    assert_eq!(first.value, b"AAAA");
    assert_eq!(first.bytes_after, 4);
    if append_during_drain {
        // Deterministically interleave the server's append between the two reads.
        conn.change_property8(PropMode::APPEND, window, atom, AtomEnum::STRING, b"CCCC")
            .unwrap()
            .check()
            .unwrap();
    }
    restore_property_tail(&conn, window, atom, 4).unwrap();
    let remaining = conn
        .get_property(false, window, atom, AtomEnum::ANY, 0, u32::MAX)
        .unwrap()
        .reply()
        .unwrap();
    std::println!(
        "remaining property: {}",
        String::from_utf8_lossy(&remaining.value)
    );
    let expected: &[u8] = if append_during_drain {
        b"BBBBCCCC"
    } else {
        b"BBBB"
    };
    assert_eq!(remaining.value, expected);
    // Each queued notification must still retrieve exactly its own message.
    for message in expected.chunks_exact(4) {
        let next = conn
            .get_property(true, window, atom, AtomEnum::ANY, 0, 1)
            .unwrap()
            .reply()
            .unwrap();
        assert_eq!(next.value, message);
        if next.bytes_after > 0 {
            restore_property_tail(&conn, window, atom, 4).unwrap();
        }
    }
    let empty = conn
        .get_property(false, window, atom, AtomEnum::ANY, 0, u32::MAX)
        .unwrap()
        .reply()
        .unwrap();
    assert_eq!(empty.type_, u32::from(AtomEnum::NONE));
    conn.destroy_window(window).unwrap().check().unwrap();
}

#[test]
#[ignore = "requires an isolated X server via XIM_TEST_DISPLAY"]
fn append_between_property_reads_preserves_each_message_once() {
    queued_messages(true);
}

#[test]
#[ignore = "requires an isolated X server via XIM_TEST_DISPLAY"]
fn reused_property_preserves_queued_fcitx4_replies() {
    queued_messages(false);
}

// A review that answers a tab bar request (Annotate diff, Annotate reply) is that
// button's tab in Herdr: its browser tab carries the button's name. Other reviews,
// such as those an agent opens itself, keep the name "Plannotator".
const BUTTONS = {
    review: { action: 'code-review', tab: 'Annotate diff' },
    last: { action: 'annotate-last', tab: 'Annotate reply' },
};

export function createRequestNamer({ now = Date.now, within = 60000 } = {}) {
    let pending;
    return {
        /** One of the request commands is starting: name its review. */
        expect(command) {
            const button = BUTTONS[command];
            pending = button && { ...button, until: now() + within };
        },
        /** A `plannotator:session-opened` event, with `tabName` when it answers the request. */
        name(event) {
            if (!pending || event?.action !== pending.action || now() >= pending.until) return event;
            const tabName = pending.tab;
            pending = undefined;
            return { ...event, tabName };
        },
    };
}

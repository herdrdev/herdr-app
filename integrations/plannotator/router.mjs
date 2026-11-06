// Review identity comes from Plannotator, not its reusable listening address.
export function createReviewRouter(session, { open, close, missingDelay = 1000 }) {
    const records = new Map(), queues = new Map(), waiting = new Map();
    let stopped = false;
    const owns = event => event?.piSessionId === session && typeof event.id === 'string' && typeof event.url === 'string';
    const forget = record => { clearTimeout(record.expiry); records.delete(record.event.id); };
    async function dispose(record) {
        if (record.opening) await record.opening;
        if (record.receipt?.kind === 'embedded') record.closing ??= Promise.resolve().then(() => close(record.receipt));
        await record.closing;
        if (record.consumed) forget(record);
    }
    return {
        opened(event) {
            if (stopped || !owns(event) || records.has(event.id)) return;
            const record = { event, closed: false, consumed: false };
            records.set(event.id, record);
            const waiter = waiting.get(event.url)?.shift();
            if (waiter) waiter(record);
            else {
                const queue = queues.get(event.url) ?? [];
                queue.push(record); queues.set(event.url, queue);
            }
        },
        async closed(event) {
            if (!owns(event)) return;
            const record = records.get(event.id);
            if (!record || record.event.url !== event.url) return;
            record.closed = true;
            if (!record.consumed) {
                record.expiry ??= setTimeout(() => {
                    const queue = queues.get(event.url);
                    if (queue) {
                        const remaining = queue.filter(item => item !== record);
                        if (remaining.length) queues.set(event.url, remaining); else queues.delete(event.url);
                    }
                    forget(record);
                }, 30000).unref();
            }
            await dispose(record);
        },
        async request(url) {
            if (stopped) return { kind: 'closed' };
            const queue = queues.get(url);
            let record = queue?.shift();
            if (!queue?.length) queues.delete(url);
            if (!record) record = await new Promise(resolve => {
                const list = waiting.get(url) ?? [];
                const finish = value => {
                    clearTimeout(timer);
                    const index = list.indexOf(finish);
                    if (index !== -1) list.splice(index, 1);
                    if (!list.length) waiting.delete(url);
                    resolve(value);
                };
                const timer = setTimeout(() => finish(undefined), missingDelay);
                list.push(finish); waiting.set(url, list);
            });
            if (stopped) return { kind: 'closed' };
            if (!record) return { kind: 'external' }; // Archive and other non-review URLs.
            record.consumed = true;
            clearTimeout(record.expiry);
            if (!record.closed) {
                record.opening = Promise.resolve().then(() => open(record.event, () => record.closed)).then(receipt => { record.receipt = receipt; });
                await record.opening;
            }
            if (record.closed) { await dispose(record); return { kind: 'closed', review: record.event.id }; }
            return { kind: record.receipt.kind, ...(record.receipt.pane ? { pane: record.receipt.pane } : {}), review: record.event.id };
        },
        async shutdown() {
            stopped = true;
            for (const list of [...waiting.values()]) for (const finish of [...list]) finish(undefined);
            const results = await Promise.allSettled([...records.values()].map(record => { record.closed = true; return dispose(record); }));
            for (const record of records.values()) clearTimeout(record.expiry);
            records.clear(); queues.clear();
            const errors = results.filter(result => result.status === 'rejected').map(result => result.reason);
            if (errors.length) throw new AggregateError(errors, 'Could not close every Plannotator pane');
        },
    };
}

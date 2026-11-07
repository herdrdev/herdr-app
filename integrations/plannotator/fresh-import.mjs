// Keep this import native: Pi's TypeScript loader caches transformed imports across reloads.
export function importFresh(url) {
    return import(url);
}

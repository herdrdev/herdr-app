# Interface languages

Herdr follows the system's preferred interface languages at startup. Settings >
Look offers Follow system, English, 简体中文 and 繁體中文. A changed preference
applies after reopening the app; it never restarts the core or its terminals.
`settings.json` accepts `system`, `en`, `zh-Hans`, `zh-Hant`, and the legacy
Simplified Chinese alias `zh-CN`. Unknown strings follow the system. If none of
the preferred languages is supported, the interface uses English.

## Adding a language

1. Copy the English `.ftl` files in `defaults/locales/en` to a language folder.
   Translate entire messages, retaining message IDs and named variables. Use
   Fluent selectors for plural rules. Review the retro layout and access keys too.
2. Register the code, native name and detection aliases in `src/i18n.rs`.
   Settings reads this registry automatically. The build embeds all catalogs.
3. Run `just test`, check native rendering on supported operating systems,
   and inspect glyph fallback, wrapping, zoom and long labels. Right-to-left
   languages additionally require layout work; the current UI is left-to-right.

## Adding messages

Use semantic IDs scoped to a feature. Rust calls `i18n::text` or
`i18n::translate`; JavaScript imports `t as tr` from `herdr`. Pass user data as
named variables. Never translate arbitrary strings in a renderer, recursively
translate parameters, or use translated labels as command IDs, persistence keys,
comparison values or game-state keys. Missing or invalid translations fall back
to English and finally the message ID, with a deduplicated diagnostic.

The effective locale stays fixed for the entire launch. Settings compares the
saved preference to `settings().language_preference` (the startup preference),
not to the effective locale: switching from an explicit language to Follow
system is a preference change even when both currently resolve identically.

Rust tests validate Fluent syntax, duplicate IDs, message/variable parity and
formatting. JS harnesses use an English-only test double; it is not a second
production translation engine. Personal mods can use `locale()` and their own
dictionaries; no third-party catalog registration API is provided.

## Attribution

Simplified Chinese translations were adapted from
[airfloats' PR #495](https://github.com/herdrdev/herdr-app/pull/495), commit
`0a9d623b320b40888731df56e26250f81ccad597`. Its global render-time text replacement
hook is not used. Additional messages and Traditional Chinese terminology are
maintained in this repository's Fluent catalogs.

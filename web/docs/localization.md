# Localization

The console supports `en`, `zh-CN`, `zh-Hant`, `ja`, `ko`, and `es` through i18next's typed selector API. English is the bootstrap catalog; each other language is a complete nested catalog loaded on demand. There is no partial override catalog. Repository prose and code comments stay English; deliberate localized UI text stays localized.

## Selection and loading

Language preference is either system selection or an explicit supported language. System mode follows browser language preferences, normalizing tags and scripts: Simplified Chinese is used for bare Chinese, Traditional Chinese for Hant and Taiwan/Hong Kong/Macao regions; unsupported choices fall back to English. The selected preference persists beside other browser preferences when storage permits.

Loading and committing are separate. While a requested catalog loads, the committed language remains visible. A stale load cannot activate a superseded preference; a successful current load commits catalog and formatters together. Failures retain the previous catalog and expose recovery. Failed loads have a 15-second retry window. Changes do not remount the connection, current page, editor drafts, OAuth steps, or standing notices.

## Authored messages

Use typed selector calls for authored copy. Maintain every supported catalog's structure, interpolation variables, plural forms, and rich-slot contracts together. Keep static language names in language metadata so users can select a language without loading its catalog. Gateway messages, upstream/user names, identifiers, URLs, and protocol tokens are foreign text and are not translated.

Rich text uses declared `[[slot]]` markers and typed JSX slot values. Slot metadata owns the expected names; never inject raw HTML or reinterpret user text as message syntax. Machine values use LTR isolation, while user/foreign text uses automatic bidirectional isolation. CSS and layout use logical directions where appropriate.

Use the formatter functions associated with the committed language for numbers, durations, and timestamps. Dates use the user's timezone with a Gregorian calendar and 24-hour clock. UTC accounting-day boundaries do not change when the display locale changes. Generated error or validation wording must use the same authored message vocabulary rather than synthesizing an English fallback.

## Verification

Run `npm run check` from the repository root. It includes TypeScript and `npm run check:i18n -w web`, which checks catalog shape, placeholders, plurals, rich slots, and imported message ownership. The production build also runs the catalog gate. Add UI copy to catalogs and metadata as needed, rather than hiding messages in components or introducing untyped translation keys.

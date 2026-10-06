[pattern] Typed IPC events should keep the generic topic envelope stable while making each topic payload strongly typed and tested with representative wire JSON.
[anti-pattern] A feature-specific manual log publisher is not a substitute for a bounded tracing-to-IPC bridge when the product contract promises recent server logs.
[learning] New IPC error variants must be audited through server error codes, CLI exit mapping, JSON output, README docs, and integration tests in the same iteration.
[anti-pattern] Sleep-based readiness in integration helpers hides races; fake IPC/OBS servers should expose explicit readiness and shutdown handles.
[pattern] Public CLI/IPC contracts work best when error code, exit mapping, JSON envelope, docs, and integration tests are changed as one compatibility unit.
[anti-pattern] Redacting only at the final presentation layer leaves non-JSON output and IPC payloads dependent on upstream safety; sanitize at the payload/log boundary too.
[anti-pattern] Hand-built protocol fixtures can document an API the server does not actually emit; public wire contracts need server-path integration assertions.
[learning] Redaction helpers drift when string-message and structured-JSON sanitizers live separately; keep one shared policy and make repeated redaction idempotent.
[anti-pattern] Public IPC protocol types should not import OBS client internals; keep conversion at a server/domain adapter boundary so wire contracts do not drift with implementation refactors.
[learning] State subscription tests must account for the initial snapshot pushed on subscribe before asserting later state-change broadcasts.
[anti-pattern] Moving a cross-layer adapter into `domain` does not fix dependency direction if it imports both implementation internals and public wire types; use server/application adapters or pure domain events.
[anti-pattern] Broadcasting state after an event without refreshing or mutating the cached snapshot creates false confidence; tests must assert the actual changed state, not just that a broadcast arrived.
[learning] Source-scanning architecture guards that inspect only `use` imports miss fully-qualified path references; boundary tests need parser support or broader production-source matching.
[pattern] TUI color comes from derived tokens, not literals: `Theme::surface`/`surface_hi`/`track`/`fade`/`gradient`/`on_color` are computed from the palette roles, so every built-in and custom theme restyles at once; overlays fill `surface_hi`, panels `surface`, meter empties `track`, and state chips go through `chrome::badge`/`chrome::meter`.
[learning] Any text a completion menu writes back into a command line must round-trip through that line's parser: an unquoted spaced candidate (`:scene Cam 1`) parses as two arguments and fails, so completion has to quote names the tokenizer would split, and every offered candidate should be pinned by a parse round-trip test.
[pattern] Palette target resolution order is exact shortcut/alias/name, then case-insensitive alias/name, then 1-based row number (scenes only) — names always win over row numbers, and both quote styles (`"`/`'`) are equivalent in the tokenizer.

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

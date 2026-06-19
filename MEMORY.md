[pattern] Typed IPC events should keep the generic topic envelope stable while making each topic payload strongly typed and tested with representative wire JSON.
[anti-pattern] A feature-specific manual log publisher is not a substitute for a bounded tracing-to-IPC bridge when the product contract promises recent server logs.
[learning] New IPC error variants must be audited through server error codes, CLI exit mapping, JSON output, README docs, and integration tests in the same iteration.
[anti-pattern] Sleep-based readiness in integration helpers hides races; fake IPC/OBS servers should expose explicit readiness and shutdown handles.
[pattern] Public CLI/IPC contracts work best when error code, exit mapping, JSON envelope, docs, and integration tests are changed as one compatibility unit.
[anti-pattern] Redacting only at the final presentation layer leaves non-JSON output and IPC payloads dependent on upstream safety; sanitize at the payload/log boundary too.

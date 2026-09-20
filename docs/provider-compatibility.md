# Provider Compatibility Matrix

M1-011 verifies OpenAI-compatible model clients and production provider API
against a reusable local mock server. Tests are offline and use only the Rust
standard library for provider HTTP fixtures.

| Capability | Fixture | Expected result |
|---|---|---|
| Normal completion | JSON completion | Content and `stop` normalized |
| Measured usage | Prompt, completion, total, cached tokens | Exact values, `estimated=false` |
| Missing usage | No usage object | Zero tokens, `estimated=true` |
| Streaming SSE | Content and final usage events | Content and usage aggregated through `[DONE]` |
| Fragmented SSE | Event split across HTTP chunks | Complete content reconstructed |
| Stream disconnect | Connection closes before `[DONE]` | `provider_unavailable` |
| Tool call | OpenAI function call response | Name and JSON arguments normalized; never executed |
| Authentication | HTTP 401 | `authentication_failed` |
| Rate limit | HTTP 429 | `rate_limited` |
| Timeout | Delayed response | `timeout` |
| Context overflow | HTTP 400 `context_length_exceeded` | `context_too_large` |
| Provider unavailable | HTTP 503 | `provider_unavailable` |
| Malformed response | Invalid JSON | `invalid_response` |

## Production API and Persistence

`tests/provider_compatibility.rs` creates providers and models through
`ai_team::api::providers::router`, then invokes chat, streaming, and tool probe
routes. Successful probes persist one `provider_probes` row and update the
matching verified capability to `supported`. Replaying the same probe with the
same idempotency key returns the stored JSON response without a second provider
request or probe row.

Authentication, rate limit, timeout, context overflow, and provider unavailable
responses persist normalized error codes and set chat capability to
`unsupported`. Rate limiting returns the standard API error envelope with a
matching `x-request-id`; other probe failures return the persisted probe result.
Assertions reject raw provider error markers from API responses and test output.

Error assertions require `Display` to contain only normalized error kinds. Mock
provider messages and raw provider codes must not appear.

Run model, API, and persistence compatibility checks with PostgreSQL available:

```sh
DATABASE_URL=postgres://ai_team:ai_team_dev@127.0.0.1:5432/ai_team \
  cargo test --test provider_compatibility -- --test-threads=1 --nocapture
```

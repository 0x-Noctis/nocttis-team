# Provider Compatibility Matrix

M1-011A verifies OpenAI-compatible model clients against a reusable local mock
server. Tests are offline and use only the Rust standard library for HTTP
fixtures.

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

Error assertions require `Display` to contain only normalized error kinds. Mock
provider messages and raw provider codes must not appear.

Run model-level compatibility checks:

```sh
cargo test --test provider_compatibility -- --nocapture
```

API routes and probe persistence are intentionally excluded until M1-009 is
integrated. This foundation does not mark the full M1-011 task complete.

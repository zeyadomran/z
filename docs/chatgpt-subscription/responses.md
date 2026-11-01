# Direct subscription Responses adapter

Verified **2026-11-01**. Read [qualification gates](README.md) and [auth](auth.md).
Examples are proposed wire shapes, not executed requests or live qualification.

## Catalog and HTTP boundary

Use `GET https://api.openai.com/v1/models` with
`Authorization: Bearer <selected-profile-access-token>`. This route documents a
`models` array, not the API-key catalog's assumed `data` shape. Preserve ordering,
display entries with `visibility: "list"` using `display_name`, and submit the
chosen `slug`. Refresh when accounts change; cache with registration and retrieval
time. Do not infer context limits or tools from a display name or a different
account's catalog.

The documented catalog supplies no context limits or tool-capability fields.
Harness policy: require host configuration per exact slug for input context
capacity and response headroom, with recorded source/qualification provenance.
Reject missing, zero, inconsistent, or invalid limits before sending. Budget
instructions, schemas, history and the entire next result batch within that
capacity; this is a local admission limit, not an upstream output-token cap.
Model-specific function support must pass qualification. Do not use undocumented
catalog fields or guessed defaults; any future metadata source needs verification.

Inference is `POST https://api.openai.com/v1/responses`, headers
`Authorization: Bearer <same-profile-access-token>` and
`Content-Type: application/json`, with SSE response consumption. The documented
direct route needs no partner key, `ChatGPT-Account-Id`, organization/project
header, or Codex-specific beta header. Do not derive extra headers from token
claims. Never use ChatGPT `backend-api` URLs.
[Models and inference](https://developers.openai.com/siwc/token-sharing-open-source/models-and-inference).

## Request allowlist

Harness policy: start with exactly `model`, array `input`, `instructions`, `tools`,
`store: false`, and `stream: true`. Optional model settings need separate documented
route/model support before admission; the generic Responses schema is not a
subscription-route allowlist. Use developer messages or `instructions` for the
stable prefix, not explicit `{type:"message",role:"system"}` items.

The preview rejects these fields:

```text
background, conversation, max_output_tokens, max_tool_calls, metadata,
moderation, multi_agent, prompt, prompt_cache_retention, safety_identifier,
temperature, top_logprobs, top_p, truncation, user
```

Also omit HTTP `previous_response_id`; replay required history. No promise of an
upstream output-token cap follows from local time, buffer, request, or tool limits.
Function/custom tools must be grouped in namespaces or supplied through
`additional_tools` input items. Choose static namespaces for M1b. Exclude hosted
MCP, tool search, programmatic tool calling, native computer use, file search,
Code Interpreter, and image generation. Web search is account/model-policy gated.
Text/image/file inputs are model-dependent; no Files upload API, audio/video, or
transcription route is granted. v0.1's initial contract is text and local functions.
[Preview restrictions](https://developers.openai.com/siwc/token-sharing-open-source/preview-limitations).

## Function and opaque continuation

An illustrative minimal read-tool request (replace the model placeholder with a
selected catalog slug):

```json
{
  "model": "<ACCOUNT_MODEL_SLUG>",
  "instructions": "Inspect the requested workspace file using the read tool.",
  "input": [{"role": "user", "content": "Explain README.md."}],
  "tools": [{
    "type": "namespace",
    "name": "workspace",
    "description": "Approved local workspace operations.",
    "tools": [{
      "type": "function",
      "name": "read_file",
      "description": "Read a permitted workspace-relative UTF-8 file.",
      "parameters": {
        "type": "object",
        "properties": {"path": {"type": "string"}},
        "required": ["path"],
        "additionalProperties": false
      }
    }]
  }],
  "store": false,
  "stream": true
}
```

The namespace wire schema is documented in
[function calling](https://developers.openai.com/api/docs/guides/function-calling#defining-namespaces).
Harness execution still validates namespace/name/arguments and applies existing
scope/approval policy; schema acceptance grants no host permission.

After `response.completed`, persist the complete ordered output. A function item
uses `type: "function_call"`, `name`, `namespace` when provided, JSON-string
`arguments`, and provider `call_id` (distinct from item `id`). Append a matching
result after the original output, for example:

```json
{"type":"function_call_output","call_id":"<RETURNED_CALL_ID>","output":"<SERIALIZED_TOOL_RESULT>"}
```

Then send old input + complete output + all batch results as the next `input`,
with unchanged instructions and tool definitions. Keep each returned identifier
and namespace, rather than reconstructing a flattened tool name. Custom-tool
items use `custom_tool_call` with string `input`; their matching result uses
`custom_tool_call_output` and the same `call_id`. M1b can implement function tools
only and explicitly reject unexpected executable custom-tool content.
The plan's harness-owned tool acceptance is satisfied by this wire `function`
round trip; wire `custom_tool_call` support is not an additional M1b requirement.
[Responses item schemas](https://developers.openai.com/api/reference/resources/responses/methods/create).

Preserve all required reasoning items, including opaque `encrypted_content`,
item IDs and original order; do not decrypt, summarize, regenerate, or move them
across accounts/providers. The current reasoning guide says stateless responses
include encrypted reasoning by default; `include: ["reasoning.encrypted_content"]`
is a legacy compatibility option, not mandatory. Keep the minimal allowlist
without `include` initially; missing required continuation is a qualification
failure, not permission to drop it. Generic examples using `store: true`, scalar
input, or `previous_response_id` do not override this route's restrictions.
[Stateless reasoning](https://developers.openai.com/api/docs/guides/reasoning#preserve-reasoning-without-stored-responses).

For streaming reasoning, retain the complete item from `response.output_item.done`,
including its final `encrypted_content`; `.added` can contain an incomplete value.
Never reconstruct opaque state from `.added` plus text deltas.
[Completed reasoning items](https://developers.openai.com/api/reference/resources/responses/subresources/input_items/methods/list).
Harness policy: collect done items by output index and ID. At `response.completed`,
reconcile their identities, order and types with the terminal output, retaining
the done item's opaque value. If the terminal output also supplies that value it
must match. Missing done reasoning items, conflicting IDs/values, or inconsistent
order fail the response before any tool executes. Item completion alone never
authorizes execution without the successful response terminal event.

Harness policy: assemble arbitrary SSE chunks without executing partial calls.
Only `response.completed` authorizes complete-output inspection; refusal is not
task success. `response.failed`, `response.incomplete`, explicit error, or EOF
without completion discards execution proposals. Quota errors may arrive after
text has streamed. Persist a completed batch before execution; record results
incrementally; finish every call truthfully before continuation. Resume never
reexecutes completed effects. Preserve the plan's cancellation and unknown-outcome
recovery rules; local disconnect does not prove upstream cancellation or no usage.

## Error classification

Record sanitized HTTP status, structured `error.code`/`error.param`, request ID,
and whether failure preceded SSE or arrived as `response.failed`. Admission may
return `{"detail":"..."}` instead of `error`; diagnostic text is not a stable code.
The official error list is not exhaustive. The fallback rows below are harness
policy, not a claim that all future errors have known meanings.

| Exact code / condition | Harness outcome and action |
| --- | --- |
| `subscription_sharing_user_not_eligible` (403) | Access blocked; explain user/workspace/policy restriction; no OAuth loop |
| `subscription_sharing_usage_limit_exceeded` (429) | Suspend subscription requests; link Usage; do not guess reset time or total-plan exhaustion |
| `subscription_sharing_usage_unavailable` (503) | Preserve credentials; bounded backoff |
| `subscription_sharing_user_unavailable` (503) | Same transient handling for user/workspace lookup |
| `subscription_sharing_unsupported_capability` (400) | Diagnose `error.param`; correct feature/model/service-tier configuration; no identical retry |
| `subscription_sharing_route_not_supported` (403) | Correct method/endpoint; repeated sign-in is not the fix |
| `subscription_sharing_invalid_user` (401) | Run the bounded 401 recovery policy below; never loop OAuth |
| `chatpass_v2_scope_not_authorized`, `chatpass_v2_invalid_authorization_context` (403) | Check registration and grant configuration; no retry/billing switch |
| Other 401, with or without a structured code | Run the bounded 401 recovery policy below |
| Admission 403 without code | Stop for grant/policy/region diagnosis; no automatic OAuth or retry |
| Admission 503 without code | Route unavailable or not enabled; preserve credentials, bounded retry then surface blocker |
| Verified context-overflow code, including `context_length_exceeded` | Return recoverable `context_limit`; no identical retry or partial tool execution |
| Other unknown structured code/status | Preserve sanitized evidence; stop with an actionable provider error unless a verified transient classification permits bounded retry; never infer eligibility or erase tokens from an unknown code alone |

Harness 401 recovery: check the saved direct-use scope first; missing permission
requires explicit consent. Otherwise acquire the account refresh lock, reload the
generation, and perform at most one refresh (or reuse a newer validated generation
another process already saved). A successful refresh permits one retry of the
rejected provider request, counted in the existing attempt budget. A second 401
suspends the route for diagnosis with the preserved request ID, without another
automatic refresh. In particular, repeated `subscription_sharing_invalid_user`
after successful refresh must not prompt sign-in: for that error, ask for sign-in
only after confirmed revocation or a terminal refresh error. Terminal refresh
errors clear unusable credentials and require fresh
OAuth with the retained registration. Transient or uncertain refresh outcomes
follow [the refresh policy](auth.md#rotation-persistence-and-logout); they do not
authorize inference retries. Never replay tools or replace a stored completed
assistant response as part of authentication recovery.

For unknown 429s do not invent subscription exhaustion; retain the error and apply
bounded transient retry only when classified as throttling. Keep the plan's
two-retry ceiling and honor usable Retry-After without violating the turn budget.
Never retry tool execution as an HTTP retry. Missing permission requires explicit
consent; revoked/terminal refresh credentials follow [auth.md](auth.md).
[Official error/recovery mapping](https://developers.openai.com/siwc/token-sharing-open-source/errors-and-recovery).

## Usage and billing identity

Store available per-attempt Responses `usage`: `input_tokens`, `output_tokens`,
`total_tokens`, `input_tokens_details.cached_tokens`, and
`output_tokens_details.reasoning_tokens`, preserving raw supported details.
Reasoning tokens are a subset of output, cached tokens a subset of input; do not
add them again. Missing usage stays unknown; cumulative snapshots replace prior
snapshots. These counters do not establish remaining subscription allowance or
the amount charged. [Usage schema](https://developers.openai.com/api/reference/resources/responses#responses/object-usage).

Display provider/registration/model and the subscription route, with
[Manage usage](https://chatgpt.com/settings/usage). OpenAI's usage settings control
app limits and permission to use available credits. Plus has a shared five-hour
limit across apps; the documented five-hour limit does not apply to Pro, which
does not imply unlimited usage. Do not calculate plan percentage, reset time, or
money from API token prices. The docs do not establish a public remaining-quota
endpoint for this integration.
[Usage settings](https://developers.openai.com/siwc/token-sharing-open-source/profiles-and-sessions#tracking-usage),
[usage UX](https://developers.openai.com/siwc/ui-ux-guidelines).

Harness policy: never automatically change provider, account, or billing route.
ChatGPT-managed credit permission is distinct from API-key credits. For the
subscription acceptance run, verify the intended plan/settings and record the
available usage evidence without claiming the response counters prove funding.
No purchase or credit enablement is part of this research.

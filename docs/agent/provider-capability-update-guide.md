# Provider Capability Update Guide

This guide is the maintainer contract for model capability changes that affect
Libra Code runtime behavior. Keep code, tests, and this document synchronized in
the same change.

## Reasoning Variants

Reasoning / thinking support is surfaced through provider transforms, not the
static capability table. The source of truth is
`src/internal/ai/providers/transform.rs`:

- `reasoning_ids::ANTHROPIC`
- `reasoning_ids::OPENAI`
- `reasoning_ids::DEEPSEEK`
- `reasoning_ids::KIMI`
- `reasoning_ids::GEMINI`
- `reasoning_ids::ZHIPU`

Each slice contains lower-case model-id substrings. Matching is
case-insensitive `contains`, so dated or hosted aliases keep matching when they
include the canonical id prefix.

Canonical examples currently covered by the regression suite:

- `claude-opus-4`
- `gpt-5`
- `deepseek-reasoner`
- `kimi-k2`
- `gemini-2.5`
- `glm-4.5`

## Adding A Reasoning Model

1. Add the shortest stable model-id substring to the matching `reasoning_ids`
   slice in `src/internal/ai/providers/transform.rs`.
2. Extend
   `variants_surface_reasoning_for_known_thinking_models` in
   `tests/ai_provider_transform_test.rs` with a representative provider/model
   pair.
3. If the model is also a known static binding, update
   `src/internal/ai/providers/capability.rs` so user-facing validation and cost
   metadata stay accurate. This table is not the source of truth for reasoning
   variants.
4. Run:

   ```bash
   cargo test --test ai_provider_transform_test variants_surface_reasoning_for_known_thinking_models
   ```

## Adding A New Variant

1. Add a stable snake-case identifier to `variant` in
   `src/internal/ai/providers/transform.rs`.
2. Implement the provider-specific `variants(model_id)` behavior in the
   relevant transform.
3. Add focused coverage in `tests/ai_provider_transform_test.rs`.
4. Document whether the variant feeds only the profile loader or also changes
   request/response behavior.

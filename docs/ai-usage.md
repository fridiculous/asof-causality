# AI Usage

AI tools may be used to help build, document, and test this repository.

The project itself treats LLMs as optional sidecar components, not as trusted
hot-path infrastructure.

## Current State

The initial framework does not call any LLM provider and does not require an API
key. All fixture data is synthetic and local.

## Planned LLM Sidecar Rules

When added, LLM outputs should include:

- provider and model name
- prompt template identifier or hash
- source event IDs
- latency
- cost estimate when available
- failure status when the request fails or times out

LLM outputs must not block ingest. They should be cached and treated as analyst
context rather than deterministic truth.

## Prompt Injection Policy

Repository docs and fixtures must not include instructions aimed at manipulating
human or automated evaluators. Documentation should explain the system honestly
and directly.

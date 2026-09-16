---
title: Phoenix Assistant UI and App Server Event Contract
updated: 2026-09-16
---

# Phoenix Assistant UI and App Server Event Contract

Phoenix is the conversational surface inside Labby's web UI. Codex App Server is the event authority. The UI must preserve the event stream's chronology instead of grouping tool activity into a synthetic block before or after the response.

Official protocol reference: https://developers.openai.com/codex/app-server.md

## Conversation invariants

- The composer remains editable while a turn is running.
- If App Server exposes turn steering, submitting during an active turn steers that turn.
- User guidance sent while a turn is active is persisted into the visible transcript.
- Attachment-only turns are valid.
- Pasting files into the composer adds them to the attachment tray.
- Assistant text, reasoning, tools, hooks, Skills, subagents, web searches, file changes, commands, usage/status events, and other safely parseable App Server items appear at the point in the timeline where the server emitted them.
- Consecutive calls of the same activity key collapse into one graph node. Details stay collapsed by default.
- Tool/event nodes are icon-only at rest; labels/details are available through accessible labels, title/hover affordances, and expansion.
- Reasoning is available as the server emits it, but collapsed by default. Labby does not fabricate hidden model reasoning.

## Event ordering

The backend stamps every retained event with a monotonic `sequence` and `received_at_ms`. The frontend merges those events with timestamped user/assistant messages. App Server `item/agentMessage/delta` notifications become assistant-text chunks so a tool invoked between two chunks is visibly between those chunks.

The turn collector must prefer the complete chronological delta stream when App Server returns multiple `agentMessage` items. Treating the last completed `agentMessage` item as the whole answer truncates earlier text and is a regression.

## App Server item taxonomy

Phoenix recognizes the documented item families, including:

- `agentMessage`
- `plan`
- `reasoning`
- `commandExecution`
- `fileChange`
- `mcpToolCall`
- `dynamicToolCall`
- `collabToolCall` (subagents)
- `webSearch`
- `imageView`
- `contextCompaction`

It also preserves supported hook/model/status events. Unknown started/completed item types are shown as a generic icon node instead of being silently discarded.

## Attachments

Codex App Server's official turn inputs are text, image URL, and local image. The browser cannot safely hand arbitrary local paths to a remote container, so Labby transports:

- PNG/JPEG/WebP images as data URLs, up to 5 MiB each.
- supported audio as a compatibility input for deployed App Server builds that advertise audio.
- UTF-8 text/code files up to 512 KiB by decoding them server-side and converting them into a documented text input containing the filename and content.

Unsupported binary files are rejected with a user-visible explanation rather than silently dropped. A turn requires text or at least one valid attachment. Maximum attachment count remains four.

## Context meter

Use App Server's aggregate `tokenUsage.total.totalTokens` when present. Only fall back to summing the latest input/output counters when the aggregate total is unavailable. Using only the last-turn counters makes the context bar appear to move backward or report a false low value.

## Verification oracles

The focused suite covers:

- multi-item assistant responses cannot lose their first chunk;
- chronological `text -> tool -> text` rendering;
- reasoning remains collapsed in its chronological slot;
- consecutive identical MCP calls group into one icon-only node;
- hooks and subagents get distinct timeline semantics;
- aggregate context usage parsing;
- attachment-only and text-file input conversion;
- status capabilities truthfully report supported inputs/events.

Browser regression coverage should additionally exercise paste, steering while streaming, expansion/collapse, keyboard focus, and screenshot comparison against the approved Phoenix mock.

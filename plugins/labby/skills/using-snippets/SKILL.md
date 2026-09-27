---
name: using-snippets
description: Use when discovering, running, creating, editing, promoting, validating, testing, explaining, or removing Labby Code Mode snippets; or when turning a successful Code Mode execution into a reusable workflow.
---

# Using Snippets

## Overview

Labby snippets are saved Code Mode workflows. Use this skill for both existing snippets and authoring new ones. Pick gateway MCP tools, fill their schema-typed params, call them from one async JavaScript arrow function, and return structured JSON. Keep snippet business logic in the snippet body; use Labby's snippets dispatch/CLI/MCP actions to store, validate, test, and execute it.

## First Checks

Use `$using-codemode` before authoring any snippet that calls upstream tools. Search
the live catalog with `codemode.search()` and inspect the selected path with
`codemode.describe()`, then copy the returned ID, path, signature, and generated
parameter docs; use the upstream's help/schema action where applicable. Never
guess tool IDs or parameters.

When a Labby source checkout is available, resolve its Git root and read these
paths relative to it:

- `docs/snippets/README.md`
- `docs/services/SNIPPETS.md`
- `crates/labby/src/dispatch/snippets/`

If those paths are unavailable, treat the live gateway and `labby snippet --help` as the source of
truth. Do not invent snippet actions, flags, tool ids, or schemas from memory.

## Find and run a snippet

List visible snippets with `labby snippet list --json` or the `snippets.list` service action. Inspect the chosen snippet's name, description, inputs, and body with `labby snippet get <name> --json` before running it. Check declared inputs and current authority. Run with `labby snippet run <name> --param key=value` or `snippets.exec` with the same name and input object. Keep output bounded, and report the returned evidence or error without claiming an upstream mutation from a timeout alone.

For an existing snippet's schema or exact command grammar, use `labby snippet --help` and the live `snippets.schema` action. MCP/API reads and execution require `lab:admin`; only list, help, and schema are non-admin.

## Author and maintain

Read [references/authoring.md](references/authoring.md) for Markdown format, typed inputs, validation, testing, promotion, execution patterns, and removal. Search and describe upstream tools through `$using-codemode` before writing their calls.

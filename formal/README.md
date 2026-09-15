# Labby Formal Models

These artifacts are executable abstractions for falsification and replay. They
do not prove the compiled product conforms to the model; the separate C1 lane
must drive the real process through public boundaries and compare observations.

## Browser request lifecycle

The `browser_request` model is grounded in the production Browser Bridge page
call lifecycle:

- `crates/labby-browser/src/hub.rs` publishes each pending call with its exact
  connection generation, and installs cancellation cleanup before the next
  await.
- Completion is accepted only for a still-pending call owned by the current
  browser connection generation. Acceptance removes pending state before the
  reply is delivered.
- Caller cancellation, timeout, document/catalog invalidation, disconnect, and
  connection replacement remove the owned pending call. A later completion is
  rejected and cannot replace the terminal observation.
- A stale connection generation cannot complete a current call or disconnect
  its replacement. Production generations are fresh UUIDs; the model therefore
  rejects generation reuse.
- Admission and dispatch are separate model steps. Cancellation before dispatch
  means page code could not have run. Cancellation after dispatch does not claim
  to undo possible page-side effects.
- Timeout is accepted only after dispatch. This matches production, where the
  timeout wraps the reply wait only after `try_send` successfully delivers the
  page call; an admitted request that has not crossed that boundary cannot time
  out as page execution.

The five active properties in `invariants.toml` cover single terminal authority,
terminal cleanup, immutable terminal results, exact generation ownership, and
the pre-dispatch cancellation boundary. Each property is bound to a Labby-owned
Stateright handle. The T1 harness performs deterministic, single-threaded BFS
over two opaque request IDs and two fresh connection generations, subject to
explicit depth, state, action, and wall-clock bounds. A completed run therefore
reports bounded evidence, never a universal proof. Each property also has an
independent corruption test in `labby-model`; the hand-authored scenarios remain
passing golden traces rather than proofs.

### Intentional limits

- Requests carry only opaque identifiers. Arguments, results, credentials,
  origins, document contents, and other sensitive payloads are outside the
  model.
- Capacity limits, wall-clock timing, channel delivery failure, SQLite audit
  cleanup, and scheduler interleavings remain implementation/conformance
  concerns. `Timeout` is a controlled event, not a modeled clock.
- Terminal history is retained as a verification witness even though production
  removes the in-memory pending entry. Production's metadata-only audit record
  is the corresponding durable observation.
- Rejected stale and late events are modeled outcomes, not invariant failures.
  A failure occurs only if such an event changes authoritative state.
- The scenario runner currently evaluates finite safety traces. Eventual cleanup
  is not claimed as liveness; C1 must check bounded real-process cleanup with
  fixture barriers and deadlines.
- Stateright BFS has no seeded traversal mode. A supplied seed is rejected as an
  unsupported plan input rather than ignored.

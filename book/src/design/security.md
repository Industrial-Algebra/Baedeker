# Security Considerations

Baedeker executes WebAssembly, which is designed to be a safe sandbox. This page
records how Baedeker upholds that and where its limits are. Baedeker is
language-runtime infrastructure: its binary parsing, malformed-input handling,
and validation exist to improve safe execution behavior, not for offensive use.

## The sandbox contract

A correctly validated WebAssembly module, executed by a conformant engine,
cannot:

- read or write memory outside its linear memories (bounds-checked access
  traps),
- call functions it did not import or export (type-checked indirect calls),
- run forever (interpreter fuel caps execution),
- recurse without bound (`MAX_CALL_DEPTH = 512`).

Baedeker upholds these through validation + categorised traps + fuel.

## Resource limits

| Limit | Mechanism |
|---|---|
| Execution time | `Store::set_fuel(Some(n))` — stops with `FuelExhausted` |
| Recursion depth | `MAX_CALL_DEPTH = 512` — traps on exhaustion |
| Memory access | bounds-checked; traps `OutOfBoundsMemoryAccess` |
| Table access | bounds-checked; sparse-capable storage |
| Integer conversion | out-of-range float→int traps `IntegerOverflow` |

An embedder running untrusted modules sets fuel before execution and treats any
trap as a guest error, not a host crash.

## Malformed input

Decode and validation reject malformed or non-conformant binaries with
structured errors carrying byte offsets. There is no `unsafe` in `baedeker-core`
outside performance-critical interpreter dispatch (each such block carries a
`SAFETY:` comment). OOM on pathologically large inputs is guarded (the fuzz
targets harden the decoder against allocation bombs).

## GPU offload

GPU dispatch is sandboxed to the host module's handle tables: a guest cannot
forge buffer or kernel handles, and every binding is bounds-checked against
guest memory and buffer sizes. The verification layers (see
[Verification](./verification.md)) address GPU numerical correctness, which is
orthogonal to the memory sandbox.

## Known limitations

- WebAssembly 3.0 proposals (tail calls, exception handling, memory64, GC,
  threads) are **not** implemented; modules using them are rejected at
  validation. See [Roadmap](./roadmap.md).
- Fuel accounting is instruction-count-based, not wall-clock; a host that needs
  wall-clock deadlines should layer its own watchdog.
- Spectre-class side channels are out of scope for a software interpreter; rely
  on process isolation for cross-tenant workloads.

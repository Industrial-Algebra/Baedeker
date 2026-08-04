# The Register IR

WebAssembly specifies a stack machine: instructions consume operands from and
push results to an implicit operand stack. Baedeker does not execute that stack
machine directly. Instead, `lower_module` translates it into a **register IR** —
a representation where values live in named registers and control flow is
explicit.

## Why lower to registers

A register IR separates *what a value is* from *how it flows*:

- **Phi-copy joins.** When control flow converges (end of an `if`/`else`, a loop
  back-edge, a `br_table` target), each branch produces its values into
  registers, and the join copies them into the continuation's expected
  locations. This makes polymorphic-stack and multi-value joins mechanical.
- **No implicit stack.** The interpreter never reconstructs operand-stack
  depths; each instruction reads its inputs from explicit register slots.
- **Type-checked once.** Validation runs over the stack machine; the register IR
  inherits well-typedness, so execution trusts the IR shape.

## Branch values and block types

WebAssembly blocks carry result types. Baedeker carries branch values through
the register IR: a `br` to a target with arity *n* copies *n* registers into the
target's incoming slots. `br_table` joins require a consistent arity across all
targets and per-target subtype conformance — a property the official `br_table`
spec tests exercise heavily.

## Funcref identity

Reference values carry an `(instance, function)` pair rather than a bare index,
so funcref identity is meaningful across linked modules. Instance 0 is the
default for unlinked execution; linking rewrites references to the resolved
instance.

The register IR is serializable (behind the `serde` feature), which is what the
[ahead-of-time](../guide/aot.md) pipeline builds on.

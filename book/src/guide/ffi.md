# Embedding via FFI

`baedeker-ffi` exposes the engine as a C ABI (`staticlib` + `cdylib`) for host
embedding from C, C++, Swift, Kotlin, or any language with C interop. A
`build.rs` runs `cbindgen` to generate `baedeker.h` from the Rust source.

## Handle model

The FFI is handle-based to keep ownership on the Rust side:

| Handle | Owns |
|---|---|
| `BaedekerModule` | an `Arc<RegModule>` (decoded + validated + lowered) |
| `BaedekerInstance` | a `Store` + module reference |

Compile a module once, instantiate it many times. Every FFI entry returns a
status enum; on failure a thread-local last-error string is available via
`baedeker_last_error`.

## A C end-to-end run

```c
#include "baedeker.h"

BaedekerModule *mod = baedeker_module_compile(wasm_bytes, len);
BaedekerInstance *inst = baedeker_instance_new(mod);

BaedekerValue args[2] = {{.tag = BAEDeker_I32, .i32 = 20},
                         {.tag = BAEDeker_I32, .i32 = 22}};
BaedekerValue out[1];
size_t n = baedeker_instance_call(inst, "add", args, 2, out, 1);
assert(out[0].i32 == 42);
```

(See `crates/baedeker-ffi/tests/smoke.c` for the full, compiling version.)

## Memory, fuel, and sizes

The FFI exposes memory read/write (`baedeker_instance_memory_data/read/write`),
fuel (`baedeker_instance_set_fuel`), and host-function registration. Size
parameters are `uint64_t` from day one, anticipating the memory64 proposal.

## The value union

`BaedekerValue` is an extensible tagged union:

```c
typedef enum { BAEDeker_I32, BAEDeker_I64, BAEDeker_F32,
               BAEDeker_F64, BAEDeker_V128 } BaedekerValueTag;

typedef struct {
    BaedekerValueTag tag;   // a plain uint8_t — see the ABI note below
    union { int32_t i32; int64_t i64; float f32;
            double f64; uint8_t v128[16]; } u;
} BaedekerValue;
```

The `tag` is a plain `uint8_t`, not a C `enum`: pre-C23 C enums are `int`-sized
(four bytes), which would read three bytes of undefined padding against Rust's
one-byte discriminant. cbindgen is post-processed to emit the tag as a typed
enum on both branches.

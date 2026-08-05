# Apple Platforms (Swift)

Baedeker ships a Swift Package Manager package in `swift/` that wraps the FFI
into an idiomatic Swift API. It builds a multi-platform XCFramework so the same
engine runs on macOS, iOS device, and iOS simulator.

## The Swift API

| Type | Role |
|---|---|
| `BaedekerModule` | compiles or loads an AOT module |
| `BaedekerInstance` | instantiates and calls exports |
| `WasmValue` | the value enum (including `simd16` for v128) |
| `BaedekerError` | mapped from the FFI status + last-error |

```swift
let module = try BaedekerModule.compile(wasmBytes)
let instance = try BaedekerInstance(module: module)
let result = try instance.call("add", args: [.i32(20), .i32(22)])
// result == [.i32(42)]
```

`callAsync` dispatches on a background queue for non-blocking host integration.

## Host functions

Host functions are Swift closures bridged through a C trampoline:

```swift
instance.registerHostFunction("env", "log") { args in
    print("guest called log:", args)
    return []
}
```

The trampoline retains the closure for the instance's lifetime.

## Building the XCFramework

`swift/build-xcframework.sh` builds three slices — macOS universal, iOS device,
iOS simulator — and stitches them into a `.xcframework`. The package's
`module.modulemap` is staged so SPM can consume the static library. Run it from
the repo root; the script is executable and committed.

## Fuel and memory

`instance.setFuel(_:)`, `instance.memoryData()`, and the memory read/write APIs
mirror the FFI surface. Fuel is the primary lever for bounding untrusted guest
execution on mobile.

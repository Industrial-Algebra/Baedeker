# Baedeker Swift Package

Idiomatic Swift bindings for the [Baedeker](https://github.com/Industrial-Algebra/Baedeker)
WebAssembly runtime, for Apple platforms (macOS 13+, iOS 16+).

```swift
import Baedeker

let module = try BaedekerModule(aotArtifact: artifactData)   // or compilingWasm:
let instance = try BaedekerInstance(module: module)

try instance.registerHostFunction(
    module: "env", name: "mul",
    parameters: [.i32, .i32], results: [.i32]
) { args in
    guard case let .i32(a) = args[0], case let .i32(b) = args[1] else {
        throw BaedekerError.hostError("bad args")
    }
    return [.i32(a * b)]
}

let results = try instance.call("mul6x7")          // [.i32(42)]
let asyncResults = try await instance.callAsync("mul6x7")

try instance.writeMemory(offset: 64, data: payload)
let bytes = try instance.readMemory(offset: 64, count: payload.count)
try instance.withUnsafeMemoryBytes { raw in … }    // zero-copy borrow

instance.fuelLimit = 1_000_000                     // bound untrusted guests
```

## Building the Rust library

The package links a gitignored `Vendor/CBaedeker.xcframework`. From the repo
root, with Xcode installed:

```bash
swift/build-xcframework.sh
cd swift && swift build && swift test
```

The script builds `baedeker-ffi` for macOS (arm64 + x86_64 universal), iOS
devices (arm64), and the iOS simulator (arm64 + x86_64 universal), then packs
them with `xcodebuild -create-xcframework`.

## Using from an Xcode project

1. Run `swift/build-xcframework.sh` (and re-run it whenever the Rust side
   changes — e.g. as an Xcode "Run Script" build phase or a pre-build step in
   your CI).
2. Add the `swift/` directory as a local Swift package dependency.
3. Add `Baedeker` to your target's linked libraries.

## AOT workflow for iOS

Compile the WASM binary to an artifact at build time, bundle the artifact,
and load it on device — no WASM parsing on the phone, faster startup:

```bash
cargo run -p baedeker-cli -- compile module.wasm -o module.bdkaot
```

```swift
let artifact = try Data(contentsOf: Bundle.main.url(forResource: "module", withExtension: "bdkaot")!)
let module = try BaedekerModule(aotArtifact: artifact)
```

Artifacts are version-locked to the runtime — rebuild them when you update
Baedeker, and only load artifacts produced from validated modules.

## Notes

- Instances are not thread-safe; `callAsync` serializes calls on a private
  queue per instance.
- Reference-typed values (funcref/externref) are not yet representable across
  the API.
- The raw memory pointer from `withUnsafeMemoryBytes` is invalidated by any
  subsequent call into the instance (memory may grow and reallocate).

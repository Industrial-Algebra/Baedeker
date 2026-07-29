import Foundation
import CBaedeker

/// A compiled WebAssembly module (decoded, validated, lowered).
///
/// Compile once, instantiate many times. Instantiated modules hold their own
/// reference, so a `BaedekerModule` may be released while instances live on.
public final class BaedekerModule {
    let handle: OpaquePointer

    /// Decode, validate, and lower a WASM binary.
    public init(compilingWasm bytes: Data) throws {
        var handle: OpaquePointer?
        let status = bytes.withUnsafeBytes { buffer in
            baedeker_module_compile(
                buffer.baseAddress?.assumingMemoryBound(to: UInt8.self),
                buffer.count,
                &handle
            )
        }
        try BaedekerError.check(status)
        self.handle = handle!
    }

    /// Load a pre-compiled AOT artifact (see `aotArtifact()` and the
    /// `baedeker-cli compile` command). Skips decode/validate/lower.
    ///
    /// - Warning: only load artifacts produced from validated modules by
    ///   this runtime's serializer. Artifacts are compiled code, not input.
    public init(aotArtifact bytes: Data) throws {
        var handle: OpaquePointer?
        let status = bytes.withUnsafeBytes { buffer in
            baedeker_module_from_aot(
                buffer.baseAddress?.assumingMemoryBound(to: UInt8.self),
                buffer.count,
                &handle
            )
        }
        try BaedekerError.check(status)
        self.handle = handle!
    }

    /// Serialize the module into an AOT artifact for bundling.
    public func aotArtifact() -> Data {
        let size = baedeker_module_write_aot(handle, nil, 0)
        var data = Data(count: size)
        data.withUnsafeMutableBytes { buffer in
            _ = baedeker_module_write_aot(
                handle,
                buffer.baseAddress?.assumingMemoryBound(to: UInt8.self),
                buffer.count
            )
        }
        return data
    }

    deinit {
        baedeker_module_free(handle)
    }
}

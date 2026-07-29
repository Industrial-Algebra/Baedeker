import Foundation
import CBaedeker

/// An instantiated module with its own store.
///
/// Not thread-safe: use one instance from one thread at a time, or route
/// calls through `callAsync`, which serializes them on the instance's
/// private queue.
public final class BaedekerInstance {
    let handle: OpaquePointer

    /// Serializes all calls made through `callAsync`.
    private let queue = DispatchQueue(label: "com.industrialalgebra.baedeker.instance")

    /// Retained host-function closures, released at deinit.
    private var hostBoxes: [HostFunctionBox] = []

    /// Instantiate a compiled module.
    public init(module: BaedekerModule) throws {
        var handle: OpaquePointer?
        let status = baedeker_instance_new(module.handle, &handle)
        try BaedekerError.check(status)
        self.handle = handle!
    }

    deinit {
        baedeker_instance_free(handle)
    }

    /// The instance's instruction fuel budget (`nil` = unlimited, the
    /// default). Setting a budget bounds runaway guest code.
    public var fuelLimit: UInt64? {
        get { _fuelLimit }
        set {
            _fuelLimit = newValue
            let encoded = newValue.map { Int64(bitPattern: $0) } ?? -1
            baedeker_instance_set_fuel(handle, encoded)
        }
    }
    private var _fuelLimit: UInt64?

    /// Call an exported function.
    public func call(_ exportName: String, arguments: [WasmValue] = []) throws -> [WasmValue] {
        var cArgs = arguments.map { $0.cValue }
        var resultCount = 0

        // First pass: arity check without buffers also surfaces errors.
        var probe = [BaedekerValue]()
        let status: BaedekerStatus = exportName.withCString { name in
            cArgs.withUnsafeMutableBufferPointer { args in
                // Reserve up-front for the common case; resized below.
                probe = [BaedekerValue](repeating: BaedekerValue(), count: 16)
                return baedeker_instance_call(
                    handle, name,
                    args.baseAddress, args.count,
                    &probe, probe.count,
                    &resultCount
                )
            }
        }
        try BaedekerError.check(status)
        if resultCount <= probe.count {
            return probe.prefix(resultCount).map { WasmValue(cValue: $0) }
        }
        var results = [BaedekerValue](repeating: BaedekerValue(), count: resultCount)
        let retryStatus: BaedekerStatus = exportName.withCString { name in
            cArgs.withUnsafeMutableBufferPointer { args in
                baedeker_instance_call(
                    handle, name,
                    args.baseAddress, args.count,
                    &results, results.count,
                    &resultCount
                )
            }
        }
        try BaedekerError.check(retryStatus)
        return results.map { WasmValue(cValue: $0) }
    }

    /// Variadic sugar for `call(_:arguments:)`.
    public func call(_ exportName: String, _ arguments: WasmValue...) throws -> [WasmValue] {
        try call(exportName, arguments: arguments)
    }

    /// Call an export off the calling thread. Calls through this API are
    /// serialized per instance; mix with direct `call` only from one thread.
    public func callAsync(_ exportName: String, arguments: [WasmValue] = []) async throws -> [WasmValue] {
        try await withCheckedThrowingContinuation { continuation in
            queue.async {
                continuation.resume(with: Result {
                    try self.call(exportName, arguments: arguments)
                })
            }
        }
    }

    /// Register a Swift closure as a host function for one of the instance's
    /// imports. The closure receives arguments matching `parameters` and must
    /// return values matching `results`; thrown errors trap the guest with
    /// the error's description as the message.
    public func registerHostFunction(
        module: String,
        name: String,
        parameters: [WasmValueType],
        results: [WasmValueType],
        _ body: @escaping ([WasmValue]) throws -> [WasmValue]
    ) throws {
        let box = HostFunctionBox(body)
        let opaque = Unmanaged.passRetained(box).toOpaque()
        let status: BaedekerStatus = module.withCString { moduleName in
            name.withCString { functionName in
                parameters.map { $0.rawValue }.withUnsafeBufferPointer { params in
                    results.map { $0.rawValue }.withUnsafeBufferPointer { resultTypes in
                        baedeker_instance_register_host_func(
                            handle,
                            moduleName,
                            functionName,
                            params.baseAddress, params.count,
                            resultTypes.baseAddress, resultTypes.count,
                            hostTrampoline,
                            opaque
                        )
                    }
                }
            }
        }
        do {
            try BaedekerError.check(status)
        } catch {
            Unmanaged<HostFunctionBox>.fromOpaque(opaque).release()
            throw error
        }
        hostBoxes.append(box)
    }

    // MARK: - Linear memory

    /// Size of linear memory (index 0) in bytes, 0 if the instance has none.
    public var memoryByteCount: UInt64 {
        var data: UnsafeMutablePointer<UInt8>?
        var length: UInt64 = 0
        let status = baedeker_instance_memory_data(handle, &data, &length)
        guard status == BaedekerStatus_Ok else { return 0 }
        return length
    }

    /// Copy bytes out of linear memory.
    public func readMemory(offset: UInt64, count: Int) throws -> Data {
        var data = Data(count: count)
        let status = data.withUnsafeMutableBytes { buffer in
            baedeker_instance_memory_read(
                handle, offset,
                buffer.baseAddress?.assumingMemoryBound(to: UInt8.self),
                UInt64(buffer.count)
            )
        }
        try BaedekerError.check(status)
        return data
    }

    /// Copy bytes into linear memory.
    public func writeMemory(offset: UInt64, data: Data) throws {
        let status = data.withUnsafeBytes { buffer in
            baedeker_instance_memory_write(
                handle, offset,
                buffer.baseAddress?.assumingMemoryBound(to: UInt8.self),
                UInt64(buffer.count)
            )
        }
        try BaedekerError.check(status)
    }

    /// Borrow linear memory as raw bytes for the duration of the closure.
    /// The pointer is invalidated by any subsequent call into the instance
    /// (memory may grow and reallocate) — never escape it.
    public func withUnsafeMemoryBytes<R>(
        _ body: (UnsafeRawBufferPointer) throws -> R
    ) throws -> R {
        var data: UnsafeMutablePointer<UInt8>?
        var length: UInt64 = 0
        let status = baedeker_instance_memory_data(handle, &data, &length)
        try BaedekerError.check(status)
        let buffer = UnsafeRawBufferPointer(start: data, count: Int(length))
        return try body(buffer)
    }
}

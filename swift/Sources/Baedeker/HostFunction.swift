import CBaedeker

/// Retained box for a Swift host-function closure, referenced by the C
/// registration's `user_data`.
final class HostFunctionBox {
    let body: ([WasmValue]) throws -> [WasmValue]

    init(_ body: @escaping ([WasmValue]) throws -> [WasmValue]) {
        self.body = body
    }
}

/// The C callback trampoline shared by every Swift host function. Converts
/// arguments, invokes the boxed closure, marshals results, and translates
/// thrown Swift errors into a `HostError` status with the message set.
let hostTrampoline: BaedekerHostFn = { args, nArgs, results, nResults, userData in
    guard let userData = userData else {
        "host function invoked without context".withCString { baedeker_set_last_error($0) }
        return BaedekerStatus_HostError
    }
    let box = Unmanaged<HostFunctionBox>.fromOpaque(userData).takeUnretainedValue()
    let swiftArgs = (0 ..< nArgs).map { WasmValue(cValue: args![$0]) }
    do {
        let output = try box.body(swiftArgs)
        guard output.count == nResults else {
            "host function returned \(output.count) results, expected \(nResults)"
                .withCString { baedeker_set_last_error($0) }
            return BaedekerStatus_HostError
        }
        for (index, value) in output.enumerated() {
            results![index] = value.cValue
        }
        return BaedekerStatus_Ok
    } catch {
        String(describing: error).withCString { baedeker_set_last_error($0) }
        return BaedekerStatus_HostError
    }
}

import CBaedeker

/// Errors thrown by the Baedeker API, carrying the runtime's message.
public enum BaedekerError: Error, Equatable {
    /// The binary failed to decode (also: rejected AOT artifact).
    case decode(String)
    /// The module failed validation.
    case validation(String)
    /// The validated module failed to lower to register IR.
    case lowering(String)
    /// Instantiation failed (imports, segments, start function).
    case instantiation(String)
    /// Execution trapped; the message names the trap kind.
    case trap(String)
    /// The instance's fuel budget was exhausted.
    case fuelExhausted(String)
    /// Bad argument or handle usage by the caller.
    case usage(String)
    /// The operation is not supported by this API version.
    case unsupported(String)
    /// A host function callback returned a failure.
    case hostError(String)
    /// Another runtime failure (see the message).
    case runtime(String)
    /// A panic was caught at the FFI boundary (a bug — please report).
    case panic(String)
}

extension BaedekerError {
    /// The calling thread's last-error message from the runtime.
    static func lastErrorMessage() -> String {
        var buffer = [Int8](repeating: 0, count: 1024)
        let length = baedeker_last_error(&buffer, buffer.count)
        guard length > 0 else { return "unknown error" }
        return String(cString: buffer)
    }

    init(status: BaedekerStatus) {
        let message = BaedekerError.lastErrorMessage()
        switch status {
        case BaedekerStatus_Decode: self = .decode(message)
        case BaedekerStatus_Validation: self = .validation(message)
        case BaedekerStatus_Lowering: self = .lowering(message)
        case BaedekerStatus_Instantiation: self = .instantiation(message)
        case BaedekerStatus_Trap: self = .trap(message)
        case BaedekerStatus_FuelExhausted: self = .fuelExhausted(message)
        case BaedekerStatus_Usage: self = .usage(message)
        case BaedekerStatus_Unsupported: self = .unsupported(message)
        case BaedekerStatus_HostError: self = .hostError(message)
        case BaedekerStatus_Runtime: self = .runtime(message)
        case BaedekerStatus_Panic: self = .panic(message)
        default: self = .runtime("unexpected status \(status.rawValue): \(message)")
        }
    }

    /// Throw the corresponding error unless the status is `Ok`.
    static func check(_ status: BaedekerStatus) throws {
        if status != BaedekerStatus_Ok {
            throw BaedekerError(status: status)
        }
    }
}

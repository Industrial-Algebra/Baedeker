import CBaedeker

/// A WebAssembly value. Reference types (funcref/externref) are not
/// representable in this version of the API.
public enum WasmValue: Equatable {
    case i32(Int32)
    case i64(Int64)
    case f32(Float)
    case f64(Double)
    /// A 128-bit vector as 16 raw little-endian bytes; lane interpretation
    /// happens per operation.
    case v128(SIMD16<UInt8>)

    /// The value's type tag.
    public var type: WasmValueType {
        switch self {
        case .i32: return .i32
        case .i64: return .i64
        case .f32: return .f32
        case .f64: return .f64
        case .v128: return .v128
        }
    }

    init(cValue: BaedekerValue) {
        switch cValue.tag {
        case WasmValueType.i32.rawValue: self = .i32(cValue.data.i32_)
        case WasmValueType.i64.rawValue: self = .i64(cValue.data.i64_)
        case WasmValueType.f32.rawValue: self = .f32(cValue.data.f32_)
        case WasmValueType.f64.rawValue: self = .f64(cValue.data.f64_)
        case WasmValueType.v128.rawValue:
            let t = cValue.data.v128
            self = .v128(SIMD16(t.0, t.1, t.2, t.3, t.4, t.5, t.6, t.7,
                                t.8, t.9, t.10, t.11, t.12, t.13, t.14, t.15))
        default: preconditionFailure("unknown BaedekerValue tag \(cValue.tag)")
        }
    }

    /// A copy of the value in the C ABI representation.
    var cValue: BaedekerValue {
        var value = BaedekerValue()
        value.tag = type.rawValue
        switch self {
        case .i32(let v):
            value.data.i32_ = v
        case .i64(let v):
            value.data.i64_ = v
        case .f32(let v):
            value.data.f32_ = v
        case .f64(let v):
            value.data.f64_ = v
        case .v128(let v):
            value.data.v128 = (v[0], v[1], v[2], v[3], v[4], v[5], v[6], v[7],
                               v[8], v[9], v[10], v[11], v[12], v[13], v[14], v[15])
        }
        return value
    }
}

/// The WebAssembly value types, for declaring host function signatures.
public enum WasmValueType: UInt8, Equatable, CaseIterable {
    case i32 = 0
    case i64 = 1
    case f32 = 2
    case f64 = 3
    case v128 = 4
}

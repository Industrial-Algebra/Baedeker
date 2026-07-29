import XCTest
@testable import Baedeker

final class BaedekerTests: XCTestCase {
    // (module
    //   (import "env" "mul" (func $mul (param i32 i32) (result i32)))
    //   (memory (export "memory") 1)
    //   (func (export "mul6x7") (result i32)
    //     i32.const 6 i32.const 7 call $mul))
    static let moduleBytes = Data([
        0x00, 0x61, 0x73, 0x6d, 0x01, 0x00, 0x00, 0x00, 0x01, 0x0b, 0x02, 0x60,
        0x02, 0x7f, 0x7f, 0x01, 0x7f, 0x60, 0x00, 0x01, 0x7f, 0x02, 0x0b, 0x01,
        0x03, 0x65, 0x6e, 0x76, 0x03, 0x6d, 0x75, 0x6c, 0x00, 0x00, 0x03, 0x02,
        0x01, 0x01, 0x05, 0x03, 0x01, 0x00, 0x01, 0x07, 0x13, 0x02, 0x06, 0x6d,
        0x65, 0x6d, 0x6f, 0x72, 0x79, 0x02, 0x00, 0x06, 0x6d, 0x75, 0x6c, 0x36,
        0x78, 0x37, 0x00, 0x01, 0x0a, 0x0a, 0x01, 0x08, 0x00, 0x41, 0x06, 0x41,
        0x07, 0x10, 0x00, 0x0b,
    ])

    private func makeInstance() throws -> BaedekerInstance {
        let module = try BaedekerModule(compilingWasm: Self.moduleBytes)
        let instance = try BaedekerInstance(module: module)
        try instance.registerHostFunction(
            module: "env", name: "mul",
            parameters: [.i32, .i32], results: [.i32]
        ) { args in
            guard case let .i32(a) = args[0], case let .i32(b) = args[1] else {
                throw BaedekerError.hostError("unexpected argument types")
            }
            return [.i32(a * b)]
        }
        return instance
    }

    func testCompileCallAndHostClosure() throws {
        let instance = try makeInstance()
        let results = try instance.call("mul6x7")
        XCTAssertEqual(results, [.i32(42)])
    }

    func testAOTArtifactRoundtrip() throws {
        let module = try BaedekerModule(compilingWasm: Self.moduleBytes)
        let artifact = module.aotArtifact()
        XCTAssertGreaterThan(artifact.count, 11)
        // BDKAOT1 magic + u32 format version 1.
        XCTAssertEqual(artifact.prefix(7), Data("BDKAOT1".utf8))

        let aotModule = try BaedekerModule(aotArtifact: artifact)
        let instance = try BaedekerInstance(module: aotModule)
        try instance.registerHostFunction(
            module: "env", name: "mul",
            parameters: [.i32, .i32], results: [.i32]
        ) { args in
            guard case let .i32(a) = args[0], case let .i32(b) = args[1] else {
                throw BaedekerError.hostError("unexpected argument types")
            }
            return [.i32(a * b)]
        }
        XCTAssertEqual(try instance.call("mul6x7"), [.i32(42)])

        XCTAssertThrowsError(try BaedekerModule(aotArtifact: Data("junk".utf8))) { error in
            guard case BaedekerError.decode = error else {
                return XCTFail("expected decode error, got \(error)")
            }
        }
    }

    func testMemoryAPIs() throws {
        let instance = try makeInstance()
        XCTAssertEqual(instance.memoryByteCount, 65536)

        try instance.writeMemory(offset: 64, data: Data([0xde, 0xad, 0xbe, 0xef]))
        XCTAssertEqual(try instance.readMemory(offset: 64, count: 4),
                       Data([0xde, 0xad, 0xbe, 0xef]))

        try instance.withUnsafeMemoryBytes { bytes in
            XCTAssertEqual(bytes[64], 0xde)
        }

        XCTAssertThrowsError(try instance.readMemory(offset: 65533, count: 4)) { error in
            guard case BaedekerError.usage = error else {
                return XCTFail("expected usage error, got \(error)")
            }
        }
    }

    func testAsyncCall() async throws {
        let instance = try makeInstance()
        let results = try await instance.callAsync("mul6x7")
        XCTAssertEqual(results, [.i32(42)])
    }

    func testUnknownExportIsRuntimeError() throws {
        let instance = try makeInstance()
        XCTAssertThrowsError(try instance.call("nope")) { error in
            guard case BaedekerError.runtime(let message) = error else {
                return XCTFail("expected runtime error, got \(error)")
            }
            XCTAssertTrue(message.contains("UnknownExport"), "message: \(message)")
        }
    }

    func testHostFunctionErrorSurfaces() throws {
        let module = try BaedekerModule(compilingWasm: Self.moduleBytes)
        let instance = try BaedekerInstance(module: module)
        try instance.registerHostFunction(
            module: "env", name: "mul",
            parameters: [.i32, .i32], results: [.i32]
        ) { _ in
            throw BaedekerError.hostError("host says no")
        }
        XCTAssertThrowsError(try instance.call("mul6x7")) { error in
            guard case BaedekerError.hostError(let message) = error else {
                return XCTFail("expected hostError, got \(error)")
            }
            XCTAssertTrue(message.contains("host says no"), "message: \(message)")
        }
    }
}

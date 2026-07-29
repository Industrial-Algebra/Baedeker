#ifndef BAEDEKER_H
#define BAEDEKER_H

#pragma once

#include <stdarg.h>
#include <stdbool.h>
#include <stddef.h>
#include <stdint.h>
#include <stdlib.h>

/**
 * Status codes returned by every `baedeker_*` function.
 */
enum BaedekerStatus
#if defined(__cplusplus) || __STDC_VERSION__ >= 202311L
  : uint8_t
#endif // defined(__cplusplus) || __STDC_VERSION__ >= 202311L
 {
  /**
   * Success.
   */
  BaedekerStatus_Ok = 0,
  /**
   * The binary failed to decode.
   */
  BaedekerStatus_Decode = 1,
  /**
   * The module failed validation.
   */
  BaedekerStatus_Validation = 2,
  /**
   * The validated module failed to lower to register IR.
   */
  BaedekerStatus_Lowering = 3,
  /**
   * Instantiation failed (imports, segments, start function).
   */
  BaedekerStatus_Instantiation = 4,
  /**
   * Execution trapped (use `baedeker_last_error` for the trap kind).
   */
  BaedekerStatus_Trap = 5,
  /**
   * The instance's fuel budget was exhausted.
   */
  BaedekerStatus_FuelExhausted = 6,
  /**
   * Bad argument or handle usage by the caller.
   */
  BaedekerStatus_Usage = 7,
  /**
   * The requested operation is not supported by this FFI version.
   */
  BaedekerStatus_Unsupported = 8,
  /**
   * A host function callback returned a failure.
   */
  BaedekerStatus_HostError = 9,
  /**
   * Another runtime failure (see `baedeker_last_error`).
   */
  BaedekerStatus_Runtime = 10,
  /**
   * A panic was caught at the FFI boundary (a bug — please report).
   */
  BaedekerStatus_Panic = 255,
};
#ifndef __cplusplus
#if __STDC_VERSION__ >= 202311L
typedef enum BaedekerStatus BaedekerStatus;
#else
typedef uint8_t BaedekerStatus;
#endif // __STDC_VERSION__ >= 202311L
#endif // __cplusplus

/**
 * Type tag for [`BaedekerValue`], and the value-type encoding used when
 * declaring host function signatures.
 */
enum BaedekerValueTag
#if defined(__cplusplus) || __STDC_VERSION__ >= 202311L
  : uint8_t
#endif // defined(__cplusplus) || __STDC_VERSION__ >= 202311L
 {
  BaedekerValueTag_I32 = 0,
  BaedekerValueTag_I64 = 1,
  BaedekerValueTag_F32 = 2,
  BaedekerValueTag_F64 = 3,
  BaedekerValueTag_V128 = 4,
};
#ifndef __cplusplus
#if __STDC_VERSION__ >= 202311L
typedef enum BaedekerValueTag BaedekerValueTag;
#else
typedef uint8_t BaedekerValueTag;
#endif // __STDC_VERSION__ >= 202311L
#endif // __cplusplus

/**
 * Opaque handle to an instantiated module with its own store.
 */
typedef struct BaedekerInstance BaedekerInstance;

/**
 * Opaque handle to a compiled module (decoded, validated, lowered).
 */
typedef struct BaedekerModule BaedekerModule;

/**
 * Payload of a [`BaedekerValue`]; the active field is selected by the tag.
 * Floats are carried as C `float`/`double` values, vectors as 16 raw bytes.
 */
typedef union BaedekerValueData {
  int32_t i32_;
  int64_t i64_;
  float f32_;
  double f64_;
  uint8_t v128[16];
} BaedekerValueData;

/**
 * A WebAssembly value across the FFI boundary. Reference types (funcref /
 * externref) are not representable in this FFI version.
 */
typedef struct BaedekerValue {
  BaedekerValueTag tag;
  union BaedekerValueData data;
} BaedekerValue;

/**
 * Host function callback (nullable). `args`/`results` carry exactly the
 * declared signature arity. Return `BaedekerStatusOk` on success; any other
 * status traps the calling WASM function, with `baedeker_set_last_error`
 * providing the message when set.
 */
typedef BaedekerStatus (*BaedekerHostFn)(const struct BaedekerValue *args,
                                         size_t n_args,
                                         struct BaedekerValue *results,
                                         size_t n_results,
                                         void *user_data);

#ifdef __cplusplus
extern "C" {
#endif // __cplusplus

/**
 * The Baedeker version string (static storage, do not free).
 */
const char *baedeker_version(void);

/**
 * Decode, validate, and lower a WASM binary. `bytes` may be freed after this
 * returns. On success `*out` receives an owned module handle.
 *
 * # Safety
 * `bytes` must be valid for `len` bytes; `out` must be a valid pointer.
 */
BaedekerStatus baedeker_module_compile(const uint8_t *bytes,
                                       size_t len,
                                       struct BaedekerModule **out);

/**
 * Load a pre-compiled AOT artifact (see `baedeker_core::aot`) instead of a
 * WASM binary. Artifacts skip decode/validate/lower at load time; only load
 * artifacts produced from validated modules by this runtime's serializer.
 *
 * # Safety
 * `bytes` must be valid for `len` bytes; `out` must be a valid pointer.
 */
BaedekerStatus baedeker_module_from_aot(const uint8_t *bytes,
                                        size_t len,
                                        struct BaedekerModule **out);

/**
 * Free a module handle (null is allowed).
 *
 * # Safety
 * `module` must be a handle from `baedeker_module_compile`, freed at most once.
 */
void baedeker_module_free(struct BaedekerModule *module);

/**
 * Instantiate a compiled module. The instance owns its store; the module
 * handle may be freed afterwards (the instance keeps its own reference).
 *
 * # Safety
 * `module` must be a valid module handle; `out` must be a valid pointer.
 */
BaedekerStatus baedeker_instance_new(const struct BaedekerModule *module,
                                     struct BaedekerInstance **out);

/**
 * Free an instance handle (null is allowed). The instance must not be freed
 * while one of its host callbacks is executing.
 *
 * # Safety
 * `instance` must be a handle from `baedeker_instance_new`, freed at most once.
 */
void baedeker_instance_free(struct BaedekerInstance *instance);

/**
 * Set the instance's instruction fuel budget. A negative value means
 * unlimited (the default).
 *
 * # Safety
 * `instance` must be a valid instance handle.
 */
BaedekerStatus baedeker_instance_set_fuel(struct BaedekerInstance *instance, int64_t fuel);

/**
 * Register a host function for one of the instance's imports. Must be called
 * after instantiation and before the import is called. `params`/`results`
 * are arrays of `BaedekerValueTag` bytes describing the WASM signature.
 *
 * # Safety
 * All pointers must be valid; `callback` must remain callable for the
 * instance's lifetime.
 */
BaedekerStatus baedeker_instance_register_host_func(struct BaedekerInstance *instance,
                                                    const char *module,
                                                    const char *name,
                                                    const uint8_t *params,
                                                    size_t n_params,
                                                    const uint8_t *results,
                                                    size_t n_results,
                                                    BaedekerHostFn callback,
                                                    void *user_data);

/**
 * Call an exported function. `args` must match the export's parameter types.
 * Up to `results_cap` results are written to `results`; `*n_results_out`
 * always receives the export's true result count (pass `results = null`
 * with `results_cap = 0` to query arity without buffers).
 *
 * # Safety
 * All pointers must be valid for their respective counts.
 */
BaedekerStatus baedeker_instance_call(struct BaedekerInstance *instance,
                                      const char *name,
                                      const struct BaedekerValue *args,
                                      size_t n_args,
                                      struct BaedekerValue *results,
                                      size_t results_cap,
                                      size_t *n_results_out);

/**
 * Borrow the instance's linear memory (index 0) as a raw pointer and length
 * in bytes. The pointer is invalidated by ANY subsequent call into the
 * instance (memory may grow and reallocate) and by freeing the instance.
 *
 * # Safety
 * `data` and `len` must be valid pointers; the returned pointer must not be
 * used after any further call on this instance.
 */
BaedekerStatus baedeker_instance_memory_data(struct BaedekerInstance *instance,
                                             uint8_t **data,
                                             uint64_t *len);

/**
 * Copy `len` bytes out of linear memory starting at `offset`.
 *
 * # Safety
 * `dst` must be writable for `len` bytes.
 */
BaedekerStatus baedeker_instance_memory_read(struct BaedekerInstance *instance,
                                             uint64_t offset,
                                             uint8_t *dst,
                                             uint64_t len);

/**
 * Copy `len` bytes into linear memory starting at `offset`.
 *
 * # Safety
 * `src` must be valid for `len` bytes.
 */
BaedekerStatus baedeker_instance_memory_write(struct BaedekerInstance *instance,
                                              uint64_t offset,
                                              const uint8_t *src,
                                              uint64_t len);

/**
 * Copy the calling thread's error message into `buf`, returning the message
 * length excluding the NUL terminator (0 when there is no message). When the
 * message exceeds `buf_cap - 1` it is truncated; the buffer is always
 * NUL-terminated when `buf_cap > 0`.
 *
 * # Safety
 * `buf` must point to at least `buf_cap` writable bytes, or be null (in
 * which case the required length is still returned).
 */
size_t baedeker_last_error(char *buf, size_t buf_cap);

/**
 * Set the calling thread's error message from a C string. Intended for host
 * function callbacks: when a callback returns a non-`Ok` status, the FFI
 * reads this message to build the runtime error.
 *
 * # Safety
 * `message` must be a valid NUL-terminated string, or null to clear.
 */
void baedeker_set_last_error(const char *message);

#ifdef __cplusplus
}  // extern "C"
#endif  // __cplusplus

#endif  /* BAEDEKER_H */

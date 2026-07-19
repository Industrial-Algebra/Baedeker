;; Linear memory: store/load, growth, bounds traps, data segments.

(module
  (memory 1)

  ;; i32 store/load round-trip.
  (func (export "store_i32") (param i32 i32)
    local.get 0
    local.get 1
    i32.store)
  (func (export "load_i32") (param i32) (result i32)
    local.get 0
    i32.load)

  ;; Width and sign extension cohort.
  (func (export "store_i64") (param i32 i64)
    local.get 0
    local.get 1
    i64.store)
  (func (export "load_i64") (param i32) (result i64)
    local.get 0
    i64.load)
  (func (export "load_i32_8s") (param i32) (result i32)
    local.get 0
    i32.load8_s)
  (func (export "load_i32_8u") (param i32) (result i32)
    local.get 0
    i32.load8_u)
  (func (export "load_i32_16s") (param i32) (result i32)
    local.get 0
    i32.load16_s)
  (func (export "load_i32_16u") (param i32) (result i32)
    local.get 0
    i32.load16_u)
  (func (export "load_i64_32s") (param i32) (result i64)
    local.get 0
    i64.load32_s)
  (func (export "load_i64_32u") (param i32) (result i64)
    local.get 0
    i64.load32_u)

  ;; Narrow stores truncate.
  (func (export "store_i32_8") (param i32 i32)
    local.get 0
    local.get 1
    i32.store8)
  (func (export "store_i32_16") (param i32 i32)
    local.get 0
    local.get 1
    i32.store16)

  ;; Float round-trip.
  (func (export "store_f64") (param i32 f64)
    local.get 0
    local.get 1
    f64.store)
  (func (export "load_f64") (param i32) (result f64)
    local.get 0
    f64.load)

  ;; Static offset in the memarg participates in the address.
  (func (export "load_i32_offset4") (param i32) (result i32)
    local.get 0
    i32.load offset=4)

  ;; size/grow cohort.
  (func (export "mem_size") (result i32)
    memory.size)
  (func (export "mem_grow") (param i32) (result i32)
    local.get 0
    memory.grow)
  (func (export "grow_then_write") (param i32 i32) (result i32)
    (local i32)
    local.get 0
    memory.grow
    drop
    local.get 0
    i32.const 65536
    i32.mul
    local.get 1
    i32.store
    local.get 0
    i32.const 65536
    i32.mul
    i32.load)
)

(assert_return (invoke "mem_size") (i32.const 1))
(assert_return (invoke "store_i32" (i32.const 0) (i32.const 42)) )
(assert_return (invoke "load_i32" (i32.const 0)) (i32.const 42))
(assert_return (invoke "store_i32" (i32.const 4) (i32.const -7)) )
(assert_return (invoke "load_i32" (i32.const 4)) (i32.const -7))
(assert_return (invoke "load_i32_offset4" (i32.const 0)) (i32.const -7))

(assert_return (invoke "store_i64" (i32.const 16) (i64.const 0x1122334455667788)) )
(assert_return (invoke "load_i64" (i32.const 16)) (i64.const 0x1122334455667788))
(assert_return (invoke "load_i32" (i32.const 16)) (i32.const 0x55667788))

(assert_return (invoke "store_i32" (i32.const 24) (i32.const -1)) )
(assert_return (invoke "load_i32_8s" (i32.const 24)) (i32.const -1))
(assert_return (invoke "load_i32_8u" (i32.const 24)) (i32.const 255))
(assert_return (invoke "load_i32_16s" (i32.const 24)) (i32.const -1))
(assert_return (invoke "load_i32_16u" (i32.const 24)) (i32.const 65535))
(assert_return (invoke "load_i64_32s" (i32.const 24)) (i64.const -1))
(assert_return (invoke "load_i64_32u" (i32.const 24)) (i64.const 4294967295))

(assert_return (invoke "store_i32_8" (i32.const 32) (i32.const 0x1FF)) )
(assert_return (invoke "load_i32_8u" (i32.const 32)) (i32.const 0xFF))
(assert_return (invoke "store_i32_16" (i32.const 34) (i32.const 0x1ABCD)) )
(assert_return (invoke "load_i32_16u" (i32.const 34)) (i32.const 0xABCD))

(assert_return (invoke "store_f64" (i32.const 40) (f64.const 2.5)) )
(assert_return (invoke "load_f64" (i32.const 40)) (f64.const 2.5))

(assert_return (invoke "mem_grow" (i32.const 2)) (i32.const 1))
(assert_return (invoke "mem_size") (i32.const 3))
(assert_return (invoke "grow_then_write" (i32.const 1) (i32.const 77)) (i32.const 77))

;; Out-of-bounds traps: memory is 4 pages (262144 bytes) after the grows.
(assert_trap (invoke "load_i32" (i32.const 262144)) "out of bounds memory access")
(assert_trap (invoke "store_i32" (i32.const 262144) (i32.const 1)) "out of bounds memory access")
(assert_trap (invoke "load_i32" (i32.const 262141)) "out of bounds memory access")

;; Active data segments initialize memory at instantiation.
(module
  (memory 1)
  (data (i32.const 16) "\2a\00\00\00\ff")
  (data (i32.const 64) "hello")

  (func (export "load_i32") (param i32) (result i32)
    local.get 0
    i32.load)
  (func (export "load_i32_8u") (param i32) (result i32)
    local.get 0
    i32.load8_u)
)

(assert_return (invoke "load_i32" (i32.const 16)) (i32.const 42))
(assert_return (invoke "load_i32_8u" (i32.const 20)) (i32.const 255))
(assert_return (invoke "load_i32_8u" (i32.const 64)) (i32.const 104))
(assert_return (invoke "load_i32_8u" (i32.const 68)) (i32.const 111))
(assert_return (invoke "load_i32_8u" (i32.const 69)) (i32.const 0))

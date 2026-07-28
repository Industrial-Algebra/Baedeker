;; Bulk memory: memory.init, data.drop, memory.copy, memory.fill.

(module
  (memory 1)
  (data $greeting "hello wasm")

  ;; memory.init from a passive segment.
  (func (export "init") (param i32 i32 i32)
    local.get 0
    local.get 1
    local.get 2
    memory.init $greeting)
  (func (export "drop_data")
    data.drop $greeting)
  (func (export "load8") (param i32) (result i32)
    local.get 0
    i32.load8_u)

  ;; memory.copy within memory (overlap-safe).
  (func (export "copy") (param i32 i32 i32)
    local.get 0
    local.get 1
    local.get 2
    memory.copy)

  ;; memory.fill with the low byte of the value.
  (func (export "fill") (param i32 i32 i32)
    local.get 0
    local.get 1
    local.get 2
    memory.fill)
)

(assert_return (invoke "init" (i32.const 0) (i32.const 0) (i32.const 5)))
(assert_return (invoke "load8" (i32.const 0)) (i32.const 104))
(assert_return (invoke "load8" (i32.const 4)) (i32.const 111))
(assert_return (invoke "init" (i32.const 16) (i32.const 6) (i32.const 4)))
(assert_return (invoke "load8" (i32.const 16)) (i32.const 119))
(assert_return (invoke "load8" (i32.const 19)) (i32.const 109))

;; Overlapping copy forward (dst > src): must behave as if via temp.
(assert_return (invoke "fill" (i32.const 32) (i32.const 65) (i32.const 4)))
(assert_return (invoke "load8" (i32.const 35)) (i32.const 65))
(assert_return (invoke "copy" (i32.const 34) (i32.const 32) (i32.const 4)))
(assert_return (invoke "load8" (i32.const 36)) (i32.const 65))
(assert_return (invoke "load8" (i32.const 37)) (i32.const 65))

;; data.drop releases the segment; memory.init afterwards traps.
(assert_return (invoke "drop_data"))
(assert_trap (invoke "init" (i32.const 48) (i32.const 0) (i32.const 1)) "out of bounds memory access")

;; Out-of-bounds memory.copy and memory.fill trap.
(assert_trap (invoke "copy" (i32.const 65535) (i32.const 0) (i32.const 2)) "out of bounds memory access")
(assert_trap (invoke "fill" (i32.const 65535) (i32.const 0) (i32.const 2)) "out of bounds memory access")

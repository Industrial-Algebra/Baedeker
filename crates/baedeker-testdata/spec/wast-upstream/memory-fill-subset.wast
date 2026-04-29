;; Sources:
;; - https://github.com/WebAssembly/spec/blob/main/test/core/bulk-memory/memory_fill.wast
;; - https://github.com/WebAssembly/spec/blob/main/test/core/multi-memory/memory_fill0.wast

(module
  (memory $m0 0)
  (memory $m1 0)
  (memory $m2 1)

  (func (export "fill-nonzero-memory") (param $dst i32) (param $val i32) (param $len i32)
    (memory.fill $m2 (local.get $dst) (local.get $val) (local.get $len)))
  (func (export "load8_u") (param i32) (result i32)
    (i32.load8_u $m2 (local.get 0))))

(assert_invalid
  (module
    (memory 1)
    (func
      (memory.fill 1 (i32.const 10) (i32.const 20) (i32.const 30))))
  "unknown memory"
)

(assert_invalid
  (module
    (memory 1)
    (func
      (memory.fill (i32.const 10) (f32.const 20) (i32.const 30))))
  "type mismatch"
)

(assert_invalid
  (module
    (memory 1)
    (func
      (memory.fill (i32.const 10) (i32.const 20) (i64.const 30))))
  "type mismatch"
)

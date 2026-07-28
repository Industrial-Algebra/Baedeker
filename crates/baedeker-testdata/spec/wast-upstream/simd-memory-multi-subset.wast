;; Source: https://github.com/WebAssembly/spec/blob/main/test/core/simd/simd_memory-multi.wast

(module
  (memory 1)
  (memory $m 1)

  (func (param $v v128)
    (drop (v128.load8_lane $m 1 (i32.const 0) (local.get $v)))
    (drop (v128.load8_lane 1 offset=0 align=1 1 (i32.const 0) (local.get $v)))
    (v128.store8_lane $m 1 (i32.const 0) (local.get $v))
    (v128.store8_lane 1 offset=0 align=1 1 (i32.const 0) (local.get $v))))

(assert_invalid
  (module
    (memory 1)
    (func (param $v v128)
      (drop (v128.load8_lane 1 1 (i32.const 0) (local.get $v)))))
  "unknown memory 1"
)

(assert_invalid
  (module
    (memory 1)
    (func (param $v v128)
      (v128.store8_lane 1 1 (i32.const 0) (local.get $v))))
  "unknown memory 1"
)

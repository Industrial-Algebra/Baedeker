;; Sources:
;; - https://github.com/WebAssembly/spec/blob/main/test/core/simd/simd_load32_lane.wast
;; - https://github.com/WebAssembly/spec/blob/main/test/core/simd/simd_load64_lane.wast
;; - https://github.com/WebAssembly/spec/blob/main/test/core/simd/simd_store32_lane.wast
;; - https://github.com/WebAssembly/spec/blob/main/test/core/simd/simd_store64_lane.wast

(module
  (memory 1)
  (func (param $x v128) (result v128)
    (v128.load32_lane 0 (i32.const 0) (local.get $x)))
  (func (param $x v128) (result v128)
    (v128.load64_lane align=8 1 (i32.const 0) (local.get $x)))
  (func (param $x v128)
    (v128.store32_lane 3 (i32.const 0) (local.get $x)))
  (func (param $x v128)
    (v128.store64_lane align=8 1 (i32.const 0) (local.get $x))))

(assert_invalid
  (module (memory 1)
    (func (param $x v128) (result v128)
      (v128.load32_lane 0 (local.get $x) (i32.const 0))))
  "type mismatch"
)

(assert_invalid
  (module (memory 1)
    (func (param $x v128) (result v128)
      (v128.load32_lane 4 (i32.const 0) (local.get $x))))
  "invalid lane index"
)

(assert_invalid
  (module (memory 1)
    (func (param $x v128) (result v128)
      (v128.load64_lane align=16 0 (i32.const 0) (local.get $x))))
  "alignment must not be larger than natural"
)

(assert_invalid
  (module (memory 1)
    (func (param $x v128) (result v128)
      (v128.store32_lane 4 (i32.const 0) (local.get $x))))
  "invalid lane index"
)

(assert_invalid
  (module (memory 1)
    (func (param $x v128) (result v128)
      (v128.store64_lane 0 (local.get $x) (i32.const 0))))
  "type mismatch"
)
